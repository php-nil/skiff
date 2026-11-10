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
use tracing::{debug, info};

use self::output::PluginOutput;

/// 插件子进程输出采集。
///
/// 插件 stdout/stderr 若直连 `/dev/null`，启动横幅、panic 与初始化失败原因在
/// `journalctl -u skiff` 里全部丢失；若把原始字节直接拼进内核 stdout，多插件输出
/// 会互相穿插并绕开日志框架。这里逐行经 `tracing` 输出（一条事件一条记录），
/// 同时对单行长度与速率设上限，避免插件高频刷屏把 journald / 磁盘写爆或拖慢内核。
///
/// 非 unix 宿主不会拉起插件进程，这些采集件只在测试里使用；`allow(dead_code)`
/// 只覆盖「非 unix 且非测试」这一种构建，不掩盖真实未使用。
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
mod output {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tracing::{info, warn};

    /// 单行最多保留的字节数（超出截断并标注）。
    const OUTPUT_LINE_MAX_BYTES: usize = 1024;
    /// 令牌桶容量（突发允许的行数）。
    const OUTPUT_BURST: u32 = 128;
    /// 令牌桶稳态速率（行/秒）。
    const OUTPUT_RATE_PER_SEC: u32 = 20;
    /// 保留的 stderr 尾部字节数：拉起失败时原样带进错误与审计。
    const STDERR_TAIL_MAX_BYTES: usize = 2048;

    /// 单个插件进程的输出采集状态，按插件 id 保存在 `Supervisor` 上，**跨插件重启保留**。
    ///
    /// - `tail`：本次运行最近一段 stderr（有界环形缓冲，默认 2KB）；
    /// - `last_exit`：上一次运行结束/失败时的 stderr 尾巴。每次新拉起会把旧尾巴冻结到
    ///   这里并清空当前缓冲，所以新运行的输出不会覆盖上一次崩溃原因；
    /// - `emitted` / `suppressed`：实际转出与因限速丢弃的行数（用于自检与收尾汇总）。
    #[derive(Default)]
    pub(super) struct PluginOutput {
        tail: std::sync::Mutex<VecDeque<u8>>,
        last_exit: std::sync::Mutex<Option<String>>,
        emitted: AtomicU64,
        suppressed: AtomicU64,
    }

    impl PluginOutput {
        /// 新一次拉起前调用：冻结上一次的 stderr 尾巴并清空当前缓冲。
        ///
        /// 冻结的内容同时以 `warn` 写进日志，保证「上一次崩溃尾巴」不会因为进程重启
        /// 或缓冲复用而消失。
        pub(super) fn begin_run(&self, id: &str) {
            let previous = self.take_tail();
            if previous.is_empty() {
                return;
            }
            warn!(plugin = %id, "上一次运行的 stderr 尾部: {previous}");
            if let Ok(mut last) = self.last_exit.lock() {
                *last = Some(previous);
            }
        }

        /// 追加一行 stderr 到有界环形缓冲（原始字节，显示时再做 lossy 解码）。
        pub(super) fn push_stderr(&self, line: &[u8]) {
            let Ok(mut tail) = self.tail.lock() else {
                return;
            };
            for &b in line {
                if tail.len() >= STDERR_TAIL_MAX_BYTES {
                    tail.pop_front();
                }
                tail.push_back(b);
            }
            if tail.len() >= STDERR_TAIL_MAX_BYTES {
                tail.pop_front();
            }
            tail.push_back(b'\n');
        }

        pub(super) fn tail_text(&self) -> String {
            let Ok(tail) = self.tail.lock() else {
                return String::new();
            };
            String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<u8>>())
                .trim()
                .to_string()
        }

        /// 取出并清空当前 stderr 尾巴。
        pub(super) fn take_tail(&self) -> String {
            let Ok(mut tail) = self.tail.lock() else {
                return String::new();
            };
            let text = String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<u8>>())
                .trim()
                .to_string();
            tail.clear();
            text
        }

        pub(super) fn record_exit(&self, tail: &str) {
            if tail.is_empty() {
                return;
            }
            if let Ok(mut last) = self.last_exit.lock() {
                *last = Some(tail.to_string());
            }
        }

        pub(super) fn last_exit(&self) -> Option<String> {
            self.last_exit.lock().ok().and_then(|s| s.clone())
        }

        /// 实际转出的行数与因限速丢弃的行数（收尾时汇总到日志）。
        pub(super) fn stats(&self) -> (u64, u64) {
            (
                self.emitted.load(Ordering::Relaxed),
                self.suppressed.load(Ordering::Relaxed),
            )
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub(super) enum StreamKind {
        Stdout,
        Stderr,
    }

    impl StreamKind {
        fn as_str(self) -> &'static str {
            match self {
                StreamKind::Stdout => "stdout",
                StreamKind::Stderr => "stderr",
            }
        }
    }

    /// 令牌桶：允许 `OUTPUT_BURST` 行突发，之后按 `OUTPUT_RATE_PER_SEC` 行/秒放行。
    struct RateBudget {
        tokens: f64,
        last: Instant,
    }

    impl RateBudget {
        fn new() -> Self {
            Self {
                tokens: f64::from(OUTPUT_BURST),
                last: Instant::now(),
            }
        }

        fn take(&mut self) -> bool {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last).as_secs_f64();
            self.last = now;
            self.tokens = (self.tokens + elapsed * f64::from(OUTPUT_RATE_PER_SEC))
                .min(f64::from(OUTPUT_BURST));
            if self.tokens >= 1.0 {
                self.tokens -= 1.0;
                true
            } else {
                false
            }
        }
    }

    /// 把一条插件输出转成一条结构化日志（stdout → info，stderr → warn）。
    fn emit_line(
        id: &str,
        stream: StreamKind,
        line: &[u8],
        truncated: bool,
        output: &PluginOutput,
        budget: &mut RateBudget,
        skipped: &mut u64,
    ) {
        if matches!(stream, StreamKind::Stderr) {
            output.push_stderr(line);
        }
        if !budget.take() {
            *skipped += 1;
            output.suppressed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        output.emitted.fetch_add(1, Ordering::Relaxed);
        let text = String::from_utf8_lossy(line);
        let suffix = if truncated {
            " …(单行超长已截断)"
        } else {
            ""
        };
        match stream {
            StreamKind::Stdout => info!(plugin = %id, stream = "stdout", "{text}{suffix}"),
            StreamKind::Stderr => warn!(plugin = %id, stream = "stderr", "{text}{suffix}"),
        }
    }

    /// 启动一路输出读取任务：分块读取 → 按行切分 → 限额转出。
    ///
    /// 用分块读取而不是 `lines()`，是为了在插件写出超长单行时把内存占用钉在
    /// `OUTPUT_LINE_MAX_BYTES`，不让它先把内核堆撑大再做截断。
    pub(super) fn spawn_output_pump<R>(
        id: String,
        stream: StreamKind,
        mut reader: R,
        output: Arc<PluginOutput>,
    ) -> tokio::task::JoinHandle<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;

            let mut chunk = vec![0u8; 4096];
            let mut line = Vec::with_capacity(OUTPUT_LINE_MAX_BYTES);
            let mut truncated = false;
            let mut budget = RateBudget::new();
            let mut skipped = 0u64;
            loop {
                let n = match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                for &b in &chunk[..n] {
                    if b == b'\n' {
                        emit_line(
                            &id,
                            stream,
                            &line,
                            truncated,
                            &output,
                            &mut budget,
                            &mut skipped,
                        );
                        line.clear();
                        truncated = false;
                    } else if line.len() < OUTPUT_LINE_MAX_BYTES {
                        line.push(b);
                    } else {
                        truncated = true;
                    }
                }
            }
            if !line.is_empty() || truncated {
                emit_line(
                    &id,
                    stream,
                    &line,
                    truncated,
                    &output,
                    &mut budget,
                    &mut skipped,
                );
            }
            if skipped > 0 {
                warn!(
                    plugin = %id,
                    stream = stream.as_str(),
                    skipped,
                    "插件输出超出限速，已丢弃部分行（只保底采集，不阻塞插件）"
                );
            }
        })
    }

    /// 等读取任务收尾；超时则中止，避免进程卡死时留下泄留任务。
    pub(super) async fn drain_pumps(pumps: &mut Vec<tokio::task::JoinHandle<()>>) {
        for mut handle in pumps.drain(..) {
            if tokio::time::timeout(Duration::from_millis(300), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
    }
}

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
    child: ChildGuard,
    socket: PathBuf,
    token: String,
    last_used: Instant,
    /// 本次运行的输出采集状态（stderr 尾巴 + 计数）。
    output: Arc<PluginOutput>,
    /// stdout/stderr 的读取任务；进程退出后应尽快收尾。
    pumps: Vec<tokio::task::JoinHandle<()>>,
}

impl Running {
    /// 仅 unix 拉起路径与测试会构造 `Running`。
    #[cfg(any(unix, test))]
    fn new(
        child: ChildGuard,
        socket: PathBuf,
        token: String,
        output: Arc<PluginOutput>,
        pumps: Vec<tokio::task::JoinHandle<()>>,
    ) -> Self {
        Self {
            child,
            socket,
            token,
            last_used: Instant::now(),
            output,
            pumps,
        }
    }
}

/// 子进程所有权：`tokio::process::Child` 被丢弃时**不会**杀进程。
///
/// 只要 `Running` 不是经 `terminate` 正常结束就被丢弃（重复插入覆盖、`ensure` 的调用方
/// future 在拉起中途被取消），子进程就失去归属、永久泄漏。Drop 里补一刀 SIGKILL，
/// 保证无主孤儿子进程被立即回收；对已经退出的进程再调 `start_kill` 是无害的。
struct ChildGuard(Option<tokio::process::Child>);

impl std::ops::Deref for ChildGuard {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("ChildGuard 只在 Drop 中取走进程")
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("ChildGuard 只在 Drop 中取走进程")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.start_kill();
        }
    }
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
    /// 按插件 id 的 spawn 门：同一插件的拉起必须串行，否则并发 `ensure` 各起一个进程。
    spawn_gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// 按插件 id 的输出采集状态；跨插件重启保留，用于冻结上一次 stderr 尾巴。
    outputs: Mutex<HashMap<String, Arc<PluginOutput>>>,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig, db: Arc<Db>) -> Self {
        Self {
            cfg,
            db,
            running: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            spawn_gates: Mutex::new(HashMap::new()),
            outputs: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &SupervisorConfig {
        &self.cfg
    }

    /// 取（必要时创建）该插件的输出采集状态。
    #[cfg(unix)]
    async fn plugin_output(&self, id: &str) -> Arc<PluginOutput> {
        self.outputs
            .lock()
            .await
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// 上一次运行结束时的 stderr 尾巴（崩溃诊断用；新运行不会覆盖它）。
    pub async fn last_exit_output(&self, id: &str) -> Option<String> {
        self.outputs
            .lock()
            .await
            .get(id)
            .and_then(|out| out.last_exit())
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
        self.ensure_spawned(rec, || async {
            let manifest =
                manifest_of(rec).map_err(|e| SupervisorError::Manifest(e.to_string()))?;
            self.spawn(rec, &manifest).await
        })
        .await
    }

    /// `ensure` 的可注入版本：`spawn` 只在确实需要拉起时调用一次（测试可传计数替身）。
    ///
    /// 「快路径复查 → 取门 → 门内复查」三道关，保证同一插件同一时刻只有一个进程在拉起；
    /// 已在跑的直接复用。没有这层门时，并发 `ensure` 会在锁外各 spawn 一个进程，
    /// 后被插入 `running` 的那份覆盖先前那份、先前进程无人回收（2026-11-06 测试机
    /// 反复出现同一插件多个 pid、进程泄漏）。
    async fn ensure_spawned<F, Fut>(
        &self,
        rec: &PluginRecord,
        spawn: F,
    ) -> Result<PathBuf, SupervisorError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Running, SupervisorError>>,
    {
        self.reap_exited(&rec.id).await;
        {
            let mut running = self.running.lock().await;
            if let Some(r) = running.get_mut(&rec.id) {
                r.last_used = Instant::now();
                return Ok(r.socket.clone());
            }
        }

        let gate = self.spawn_gate(&rec.id).await;
        let _gate = gate.lock().await;

        // 等门期间别人可能已经拉起：直接复用，不要新起。
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

        match spawn().await {
            Ok(r) => {
                let mut running = self.running.lock().await;
                let socket = r.socket.clone();
                running.insert(rec.id.clone(), r);
                self.failures.lock().await.remove(&rec.id);
                // 进程已经拉起：状态回写失败不能反过来让调用方以为启动失败，
                // 否则会重试/误判，而实际进程已在跑。记日志后照常返回 socket。
                if let Err(e) = self.db.set_plugin_status(&rec.id, "running", "") {
                    tracing::warn!(plugin = %rec.id, error = %e, "插件状态回写失败（进程已运行）");
                }
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

    /// 摘掉已经退出的运行态（外部 kill、插件自崩）。
    ///
    /// `ensure` 原先只按 `running` 里有没有条目判断“是否在跑”，外部杀掉子进程后
    /// 下一个 RPC 会一直连死 socket（Connection refused）：既不复活，也不把崩溃前的
    /// stderr 转出。复用前先 `try_wait`，已退出就摘掉条目；尾巴留在共享的
    /// `PluginOutput` 里，由下一次 `spawn` 的 `begin_run` 冻结并写进日志。
    async fn reap_exited(&self, id: &str) {
        let dead = {
            let mut running = self.running.lock().await;
            let exited = running
                .get_mut(id)
                .is_some_and(|r| matches!(r.child.try_wait(), Ok(Some(_))));
            if exited {
                running.remove(id)
            } else {
                None
            }
        };
        if let Some(mut r) = dead {
            // 进程已退出，读端很快 EOF；先收尾读取任务，保证尾巴完整。
            output::drain_pumps(&mut r.pumps).await;
            let tail = r.output.tail_text();
            let (emitted, suppressed) = r.output.stats();
            debug!(plugin = %id, emitted, suppressed, tail, "插件进程已退出，输出采集收尾");
            let _ = std::fs::remove_file(&r.socket);
            let _ = self.db.set_plugin_status(id, "installed", "");
        }
    }

    /// 取（必要时创建）该插件的 spawn 门。
    async fn spawn_gate(&self, id: &str) -> Arc<Mutex<()>> {
        let mut gates = self.spawn_gates.lock().await;
        gates
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
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
        // 进程退出后读端会收到 EOF；顺带把当前 stderr 尾巴留作诊断。
        output::drain_pumps(&mut r.pumps).await;
        let tail = r.output.take_tail();
        r.output.record_exit(&tail);
        let (emitted, suppressed) = r.output.stats();
        debug!(plugin = id, emitted, suppressed, "插件输出采集收尾");
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
            // stdout/stderr 不再丢给 /dev/null：由下方读取任务逐行转进 tracing
            //（落到 journald），插件启动横幅、panic 与初始化失败原因因此可见。
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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

        let mut child =
            ChildGuard(Some(cmd.spawn().map_err(|e| {
                SupervisorError::Spawn(format!("{}: {e}", bin.display()))
            })?));

        // 收集 stdout/stderr：旧尾巴先冻结，再挂读取任务（分块读 + 限速转出）。
        let output = self.plugin_output(&rec.id).await;
        output.begin_run(&rec.id);
        let mut pumps = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            pumps.push(output::spawn_output_pump(
                rec.id.clone(),
                output::StreamKind::Stdout,
                stdout,
                output.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            pumps.push(output::spawn_output_pump(
                rec.id.clone(),
                output::StreamKind::Stderr,
                stderr,
                output.clone(),
            ));
        }

        // 一次性身份令牌经 stdin 交付（不落环境变量，退出即失效）
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let line = format!("{token}\n");
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

        let deadline = Instant::now() + self.cfg.health_timeout;
        let failure = loop {
            if ipc::healthz(&socket).await {
                break None;
            }
            // 起不来的插件通常很快就退出（缺依赖、端口冲突、panic）：提前收口，
            // 不必空等满 5s，并把 stderr 尾巴带进错误里。
            if let Ok(Some(status)) = child.try_wait() {
                break Some(format!("插件进程提前退出（{status}）"));
            }
            if Instant::now() >= deadline {
                let _ = child.kill().await;
                break Some("5s 内 /healthz 未通过，判拉起失败".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };

        if let Some(reason) = failure {
            output::drain_pumps(&mut pumps).await;
            let tail = output.take_tail();
            output.record_exit(&tail);
            let detail = if tail.is_empty() {
                reason
            } else {
                format!("{reason}；stderr 尾部: {tail}")
            };
            tracing::error!(plugin = %rec.id, "{detail}");
            return Err(SupervisorError::Spawn(detail));
        }

        info!(plugin = %rec.id, pid = ?child.id(), "插件已拉起（reduced sandbox）");
        Ok(Running::new(child, socket, token, output, pumps))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn rec(id: &str) -> PluginRecord {
        PluginRecord {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            api: "v1".into(),
            key_id: String::new(),
            manifest: "{}".into(),
            enabled: true,
            status: "installed".into(),
            status_detail: String::new(),
            active_version: "1.0.0".into(),
            installed_at: 0,
            grants: Vec::new(),
        }
    }

    fn supervisor(dir: &std::path::Path) -> Arc<Supervisor> {
        Arc::new(Supervisor::new(
            SupervisorConfig::new(dir, dir.join("run")),
            Arc::new(Db::open_in_memory().unwrap()),
        ))
    }

    /// 替身进程：真 child 只为验证所有权与回收，不依赖 Docker / 真插件。
    #[cfg(unix)]
    fn fake_child() -> ChildGuard {
        ChildGuard(Some(
            tokio::process::Command::new("sleep")
                .arg("60")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("测试替身 sleep 拉起失败"),
        ))
    }

    #[cfg(windows)]
    fn fake_child() -> ChildGuard {
        ChildGuard(Some(
            tokio::process::Command::new("cmd")
                .args(["/C", "ping", "-n", "120", "127.0.0.1"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("测试替身 ping 拉起失败"),
        ))
    }

    /// 同一插件并发 ensure：只允许拉起一次，其余在门内复查后复用同一 socket。
    #[tokio::test]
    async fn concurrent_ensure_spawns_once() {
        let dir = tempfile::tempdir().unwrap();
        let sup = supervisor(dir.path());
        let r = rec("com.skiff.concurrent");
        let spawned = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..16 {
            let sup = sup.clone();
            let r = r.clone();
            let spawned = spawned.clone();
            tasks.push(tokio::spawn(async move {
                sup.ensure_spawned(&r, || {
                    spawned.fetch_add(1, Ordering::SeqCst);
                    async {
                        // 留出窗口，让其余调用都卡在门上，而不是走快路径
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(Running::new(
                            fake_child(),
                            sup.socket_path(&r.id),
                            "test".into(),
                            Arc::new(PluginOutput::default()),
                            Vec::new(),
                        ))
                    }
                })
                .await
            }));
        }

        let mut sockets = Vec::new();
        for t in tasks {
            sockets.push(t.await.unwrap().unwrap());
        }

        assert_eq!(
            spawned.load(Ordering::SeqCst),
            1,
            "同一插件并发 ensure 只应拉起一个进程"
        );
        assert!(sockets.iter().all(|s| *s == sockets[0]));
        assert!(sup.is_running("com.skiff.concurrent").await);
    }

    /// 外部 kill 后 supervisor 仍持有旧条目：下一次 ensure 必须摘掉并重新拉起，
    /// 而不是把死 socket 直接交给调用方（否则 RPC 只会收到 Connection refused）。
    #[tokio::test]
    async fn ensure_respawns_after_child_exit() {
        let dir = tempfile::tempdir().unwrap();
        let sup = supervisor(dir.path());
        let r = rec("com.skiff.exited");

        let mut guard = exited_child();
        let _ = guard.wait().await;
        {
            let mut running = sup.running.lock().await;
            running.insert(
                r.id.clone(),
                Running::new(
                    guard,
                    sup.socket_path(&r.id),
                    "old".into(),
                    Arc::new(PluginOutput::default()),
                    Vec::new(),
                ),
            );
        }

        let spawned = Arc::new(AtomicUsize::new(0));
        let socket = sup
            .ensure_spawned(&r, || {
                spawned.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(Running::new(
                        live_child(),
                        sup.socket_path(&r.id),
                        "new".into(),
                        Arc::new(PluginOutput::default()),
                        Vec::new(),
                    ))
                }
            })
            .await
            .unwrap();

        assert_eq!(
            spawned.load(Ordering::SeqCst),
            1,
            "已退出的旧条目必须被摘掉并重新拉起"
        );
        assert_eq!(socket, sup.socket_path(&r.id));
        assert!(sup.is_running("com.skiff.exited").await);
    }

    /// 立即退出的替身进程：模拟被外部 kill 的子进程。
    #[cfg(unix)]
    fn exited_child() -> ChildGuard {
        ChildGuard(Some(
            tokio::process::Command::new("sh")
                .args(["-c", "exit 7"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("测试替身 sh 拉起失败"),
        ))
    }

    #[cfg(windows)]
    fn exited_child() -> ChildGuard {
        ChildGuard(Some(
            tokio::process::Command::new("cmd")
                .args(["/C", "exit 7"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("测试替身 cmd 拉起失败"),
        ))
    }

    /// 仍在运行的替身进程：Windows 下用 `timeout` 撑住，Unix 复用 `fake_child`。
    #[cfg(unix)]
    fn live_child() -> ChildGuard {
        fake_child()
    }

    #[cfg(windows)]
    fn live_child() -> ChildGuard {
        ChildGuard(Some(
            tokio::process::Command::new("cmd")
                .args(["/C", "timeout /T 60 /NOBREAK"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("测试替身 timeout 拉起失败"),
        ))
    }

    /// 没有归属的子进程：`Running` 被直接丢弃时必须被回收，而不是继续在后台跑。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropped_running_reaps_orphan_child() {
        let pid = {
            let r = Running::new(
                fake_child(),
                PathBuf::from("/tmp/skiff-test.sock"),
                "test".into(),
                Arc::new(PluginOutput::default()),
                Vec::new(),
            );
            let pid = r.child.id().expect("子进程应有 pid");
            drop(r);
            pid
        };

        // SIGKILL 送达后进程最多短暂停留为僵尸，不能继续是活的睡眠态。
        let mut alive = true;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => {
                    let state = stat
                        .rsplit(')')
                        .next()
                        .and_then(|s| s.trim().chars().next());
                    if matches!(state, Some('Z') | Some('X')) {
                        alive = false;
                        break;
                    }
                }
                Err(_) => {
                    alive = false;
                    break;
                }
            }
        }
        assert!(!alive, "丢弃 Running 后孤儿子进程仍存活（pid {pid}）");
    }

    /// 跨平台的「同时往 stdout / stderr 各写一行」的替身进程。
    fn noisy_child() -> tokio::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.args([
                "/C",
                "echo plugin-stdout-line & echo plugin-stderr-line 1>&2",
            ]);
            c
        };
        #[cfg(unix)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "echo plugin-stdout-line; echo plugin-stderr-line >&2"]);
            c
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("测试替身进程拉起失败")
    }

    /// 回归：插件的 stdout/stderr 确实被读取任务采集并转出（不是只改重定向方向）。
    #[tokio::test]
    async fn output_pump_collects_stdout_and_stderr() {
        let mut child = noisy_child();
        let output = Arc::new(PluginOutput::default());
        let mut pumps = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            pumps.push(output::spawn_output_pump(
                "t".into(),
                output::StreamKind::Stdout,
                stdout,
                output.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            pumps.push(output::spawn_output_pump(
                "t".into(),
                output::StreamKind::Stderr,
                stderr,
                output.clone(),
            ));
        }
        let _ = child.wait().await;
        output::drain_pumps(&mut pumps).await;

        let tail = output.tail_text();
        assert!(
            tail.contains("plugin-stderr-line"),
            "stderr 未被采集：{tail:?}"
        );
        assert_eq!(output.stats().0, 2, "stdout 与 stderr 各应转出一条");
    }

    /// 回归：插件重启时上一次 stderr 尾巴被冻结保留，不被新运行的输出覆盖。
    #[tokio::test]
    async fn previous_stderr_tail_survives_restart() {
        let output = PluginOutput::default();
        output.push_stderr(b"old-crash-reason");
        output.begin_run("t");
        assert!(
            output.tail_text().is_empty(),
            "新运行应从空缓冲开始，避免把旧输出算成新崩溃原因"
        );
        assert_eq!(output.last_exit().as_deref(), Some("old-crash-reason"));

        output.push_stderr(b"new-run-output");
        assert_eq!(
            output.last_exit().as_deref(),
            Some("old-crash-reason"),
            "新运行的输出不应覆盖上一次崩溃尾巴"
        );
    }
}
