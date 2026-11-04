//! 插件 Supervisor：拉起 / 健康检查 / 空闲回收 / 崩溃退避与熔断 / 降权 / 沙箱边界。
//!
//! 拉起顺序（§4.2）：预建 `/run/skiff/plugins/{id}/`（0700）→ 继承 stdin 一次性交付
//! 身份 token → `setgid`/`setuid` 到插件独立 UID → `PR_SET_NO_NEW_PRIVS` +
//! `PR_SET_PDEATHSIG` + `setrlimit` → 5s 内 `GET /healthz` 不通过即判失败。
//!
//! **未实现的沙箱能力**（landlock / cgroups v2 / nftables 出网收口）见 crate 文档；
//! 本模块只承诺「独立 UID + 文件权限 + 资源上限」这一层。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use skiff_store::{Db, PluginRecord};
use tokio::sync::Mutex;
use tracing::info;

use crate::install::manifest_of;
#[cfg(unix)]
use crate::install::{plugin_root, plugin_uid};
use crate::ipc;

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// `/run/skiff`
    pub run_root: PathBuf,
    /// `/www/skiff`（SKIFF_DATA_DIR）
    pub data_root: PathBuf,
    pub kernel_socket: PathBuf,
    pub health_timeout: Duration,
    pub allow_reduced_sandbox: bool,
}

impl SupervisorConfig {
    pub fn new(data_root: impl Into<PathBuf>, run_root: impl Into<PathBuf>) -> Self {
        let run_root = run_root.into();
        Self {
            kernel_socket: run_root.join("kernel.sock"),
            run_root,
            data_root: data_root.into(),
            health_timeout: Duration::from_secs(5),
            allow_reduced_sandbox: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("插件未安装: {0}")]
    NotInstalled(String),
    #[error("平台不支持: {0}")]
    Unsupported(String),
    #[error("拉起失败: {0}")]
    Spawn(String),
    #[error("熔断中（连续失败 {0} 次）")]
    CircuitOpen(u32),
    #[error("manifest 非法: {0}")]
    Manifest(String),
    #[error("IPC: {0}")]
    Ipc(#[from] ipc::IpcError),
    #[error("存储: {0}")]
    Store(#[from] skiff_store::StoreError),
}

struct Running {
    child: tokio::process::Child,
    socket: PathBuf,
    token: String,
    last_used: Instant,
}

#[derive(Default)]
struct Failure {
    count: u32,
    blocked_until: Option<Instant>,
}

pub struct Supervisor {
    cfg: SupervisorConfig,
    db: Arc<Db>,
    running: Mutex<HashMap<String, Running>>,
    failures: Mutex<HashMap<String, Failure>>,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig, db: Arc<Db>) -> Self {
        Self {
            cfg,
            db,
            running: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &SupervisorConfig {
        &self.cfg
    }

    /// 插件身份令牌（kernel.sock 侧校验 UID ↔ 插件 id 与 token，§4.2）。
    pub async fn token_of(&self, id: &str) -> Option<String> {
        self.running.lock().await.get(id).map(|r| r.token.clone())
    }

    pub async fn is_running(&self, id: &str) -> bool {
        self.running.lock().await.contains_key(id)
    }

    /// 插件自己的 socket：`/run/skiff/plugins/{id}/plugin.sock`。
    ///
    /// 只在一处定义——SDK 侧绑定的是 Supervisor 通过 `SKIFF_PLUGIN_SOCKET`
    /// 传进去的同一个路径，两边各拼一次就会出现「进程起来了但探不到」。
    pub fn socket_path(&self, id: &str) -> PathBuf {
        self.cfg
            .run_root
            .join("plugins")
            .join(id)
            .join("plugin.sock")
    }

    /// 确保插件已拉起，返回 socket 路径。
    pub async fn ensure(&self, rec: &PluginRecord) -> Result<PathBuf, SupervisorError> {
        {
            let mut running = self.running.lock().await;
            if let Some(r) = running.get_mut(&rec.id) {
                r.last_used = Instant::now();
                return Ok(r.socket.clone());
            }
        }
        {
            let failures = self.failures.lock().await;
            if let Some(f) = failures.get(&rec.id) {
                if let Some(until) = f.blocked_until {
                    if Instant::now() < until {
                        return Err(SupervisorError::CircuitOpen(f.count));
                    }
                }
                if f.count >= 3 {
                    return Err(SupervisorError::CircuitOpen(f.count));
                }
            }
        }

        let manifest = manifest_of(rec).map_err(|e| SupervisorError::Manifest(e.to_string()))?;
        match self.spawn(rec, &manifest).await {
            Ok(r) => {
                let mut running = self.running.lock().await;
                let socket = r.socket.clone();
                running.insert(rec.id.clone(), r);
                self.failures.lock().await.remove(&rec.id);
                self.db.set_plugin_status(&rec.id, "running", "")?;
                Ok(socket)
            }
            Err(e) => {
                let mut failures = self.failures.lock().await;
                let f = failures.entry(rec.id.clone()).or_default();
                f.count += 1;
                let backoff = match f.count {
                    1 => Duration::from_millis(500),
                    2 => Duration::from_secs(2),
                    _ => Duration::from_secs(8),
                };
                f.blocked_until = Some(Instant::now() + backoff);
                let count = f.count;
                drop(failures);
                let detail = format!("拉起失败（第 {count} 次）：{e}");
                let status = if count >= 3 { "error" } else { "degraded" };
                let _ = self.db.set_plugin_status(&rec.id, status, &detail);
                let _ = self
                    .db
                    .audit("system", "local", "plugin.spawn", &rec.id, "fail", &detail);
                Err(e)
            }
        }
    }

    /// 调用插件 RPC（自动拉起）。超时/错误不熔断进程本身，由 ensure 的失败计数负责。
    pub async fn call(
        &self,
        rec: &PluginRecord,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, SupervisorError> {
        let socket = self.ensure(rec).await?;
        if let Some(r) = self.running.lock().await.get_mut(&rec.id) {
            r.last_used = Instant::now();
        }
        Ok(ipc::rpc_call(&socket, method, params).await?)
    }

    pub async fn stop(&self, id: &str) {
        let Some(mut r) = self.running.lock().await.remove(id) else {
            return;
        };
        terminate(&mut r.child).await;
        let _ = std::fs::remove_file(&r.socket);
        let _ = self.db.set_plugin_status(id, "installed", "");
        info!(plugin = id, "插件进程已停止");
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.running.lock().await.keys().cloned().collect();
        for id in ids {
            self.stop(&id).await;
        }
    }

    /// 空闲回收（默认 300s，`resident` 除外）。由后台 tick 调用。
    pub async fn reap_idle(&self) {
        let now = Instant::now();
        let mut to_stop = Vec::new();
        {
            let running = self.running.lock().await;
            for (id, r) in running.iter() {
                let Ok(Some(rec)) = self.db.plugin(id) else {
                    continue;
                };
                let Ok(manifest) = manifest_of(&rec) else {
                    continue;
                };
                if manifest.run == skiff_pkgfmt::manifest::RunMode::Resident {
                    continue;
                }
                let idle = Duration::from_secs(manifest.idle_timeout_sec);
                if now.duration_since(r.last_used) >= idle {
                    to_stop.push(id.clone());
                }
            }
        }
        for id in to_stop {
            self.stop(&id).await;
            let _ = self
                .db
                .audit("system", "local", "plugin.reap", &id, "ok", "idle timeout");
        }
    }

    #[cfg(unix)]
    async fn spawn(
        &self,
        rec: &PluginRecord,
        manifest: &skiff_pkgfmt::Manifest,
    ) -> Result<Running, SupervisorError> {
        use std::process::Stdio;

        if !self.cfg.allow_reduced_sandbox && manifest.requires_full_sandbox {
            return Err(SupervisorError::Unsupported(
                "landlock/cgroups 尚未实现，拒绝以非降级方式拉起要求完整沙箱的插件".into(),
            ));
        }

        let root = plugin_root(&self.cfg.data_root, &rec.id);
        let bin = root
            .join("versions")
            .join(&rec.active_version)
            .join("bin")
            .join(skiff_pkgfmt::host_arch())
            .join("plugin");
        if !bin.exists() {
            return Err(SupervisorError::Spawn(format!(
                "可执行文件不存在: {}",
                bin.display()
            )));
        }

        let socket = self.socket_path(&rec.id);
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SupervisorError::Spawn(format!("创建运行目录失败: {e}")))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
                if let Ok(c) = std::ffi::CString::new(parent.to_string_lossy().as_bytes()) {
                    // SAFETY: 合法 C 字符串；非 root 运行时失败，仅告警。
                    let rc = unsafe {
                        libc::chown(c.as_ptr(), plugin_uid(&rec.id), plugin_uid(&rec.id))
                    };
                    if rc != 0 {
                        tracing::warn!(dir = %parent.display(), "运行目录 chown 失败（非 root 运行？）");
                    }
                }
            }
        }
        let _ = std::fs::remove_file(&socket);

        let token = hex::random_token();
        let data_dir = root.join("data");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&cache_dir).ok();
        let run_dir = root.join("run");
        std::fs::create_dir_all(&run_dir).ok();

        let granted = rec
            .grants
            .iter()
            .filter(|g| g.kind == "perm")
            .map(|g| g.value.clone())
            .collect::<Vec<_>>()
            .join(",");

        let uid = plugin_uid(&rec.id);
        // 运行组（若有）作为附加组带上：kernel.sock 是 0660 root:skiff-plugins
        let shared = crate::install::shared_gid();
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.env_clear()
            .env("SKIFF_PLUGIN_ID", &rec.id)
            .env("SKIFF_PLUGIN_SOCKET", &socket)
            .env("SKIFF_PLUGIN_DATA_DIR", &data_dir)
            .env("SKIFF_PLUGIN_CACHE_DIR", &cache_dir)
            .env("SKIFF_PLUGIN_RUN_DIR", &run_dir)
            .env("SKIFF_KERNEL_SOCKET", &self.cfg.kernel_socket)
            .env("SKIFF_PLUGIN_API", &manifest.api)
            .env("SKIFF_GRANTED_PERMS", &granted)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .current_dir(&run_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let nofile = 256u64;
        let nproc = manifest.pids_limit.unwrap_or(128);
        let as_limit = 256u64 * 1024 * 1024;
        // SAFETY: pre_exec 中只调用 async-signal-safe 的系统调用。
        unsafe {
            cmd.pre_exec(move || {
                // 顺序不能反：附加组必须在还是 root 时设置（setgid/setuid 之后就无权改）。
                // 顺带清掉从父进程继承来的附加组，root 的那些组不该漏给插件。
                if let Some(gid) = shared {
                    let groups = [gid];
                    if libc::setgroups(1, groups.as_ptr()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if libc::setgid(uid) != 0 || libc::setuid(uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // RLIMIT_AS 只对 C/Rust 插件安全；托管运行时（Go/JVM/.NET）需 M4 实测后再放开。
                let limits = [
                    (libc::RLIMIT_AS, as_limit, as_limit),
                    (libc::RLIMIT_NOFILE, nofile, nofile),
                    (libc::RLIMIT_NPROC, nproc, nproc),
                    (libc::RLIMIT_FSIZE, 512 * 1024 * 1024, 512 * 1024 * 1024),
                    (libc::RLIMIT_CORE, 0, 0),
                ];
                for (res, soft, hard) in limits {
                    let rl = libc::rlimit {
                        rlim_cur: soft,
                        rlim_max: hard,
                    };
                    if libc::setrlimit(res, &rl) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                libc::umask(0o077);
                Ok(())
            });
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| SupervisorError::Spawn(format!("{}: {e}", bin.display())))?;

        // 一次性身份令牌经 stdin 交付（不落环境变量，退出即失效）
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let line = format!("{token}\n");
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

        let deadline = Instant::now() + self.cfg.health_timeout;
        loop {
            if ipc::healthz(&socket).await {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill().await;
                return Err(SupervisorError::Spawn(
                    "5s 内 /healthz 未通过，判拉起失败".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        info!(plugin = %rec.id, pid = ?child.id(), "插件已拉起（reduced sandbox）");
        Ok(Running {
            child,
            socket,
            token,
            last_used: Instant::now(),
        })
    }

    #[cfg(not(unix))]
    async fn spawn(
        &self,
        _rec: &PluginRecord,
        _manifest: &skiff_pkgfmt::Manifest,
    ) -> Result<Running, SupervisorError> {
        Err(SupervisorError::Unsupported(
            "插件进程外隔离与降权只在 Linux/unix 宿主可用".into(),
        ))
    }
}

#[cfg(unix)]
async fn terminate(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: 向自己拉起的子进程发信号。
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            let _ = child.kill().await;
        }
    }
}

#[cfg(not(unix))]
async fn terminate(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

#[cfg(unix)]
mod hex {
    pub fn random_token() -> String {
        use rand::RngCore;
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        hex_encode(&buf)
    }
    fn hex_encode(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}

/// 运行目录清理（重启即清 `run/`，§5.2）。
///
/// 重建后必须把属主还回去：`remove_dir_all` + `create_dir_all` 出来的是 root 0700，
/// 而插件是以独立 UID 跑的——不 chown 的话，重启一次插件就写不进自己的 run/，
/// 表现是「昨天还好，今天启动就报权限错」。
pub fn clean_run_dirs(data_root: &Path) {
    let plugins = data_root.join("plugins");
    let Ok(entries) = std::fs::read_dir(&plugins) else {
        return;
    };
    for e in entries.flatten() {
        let run = e.path().join("run");
        if !run.is_dir() {
            continue;
        }
        let _ = std::fs::remove_dir_all(&run);
        let _ = std::fs::create_dir_all(&run);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let id = e.file_name().to_string_lossy().to_string();
            let uid = plugin_uid(&id);
            let _ = std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700));
            if let Ok(c) = std::ffi::CString::new(run.to_string_lossy().as_bytes()) {
                // SAFETY: 合法 C 字符串；非 root 环境失败也无妨（那本来就没有降权）。
                unsafe { libc::chown(c.as_ptr(), uid, uid) };
            }
        }
    }
}
