//! 站点对象生命周期：建站/删除、域名别名、vhost、TLS、日志与目录浏览。
//!
//! 运行环境与容器编排在 `sites::runtime`；目录布局在 `sites::layout`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::ContainerRow;
use skiff_model::{is_absolute, normalize_dir, within, within_root};
use skiff_store::{Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use super::*;
use crate::AppState;

/// nginx 激活结果：平滑 reload（已自证 worker 换代）或回退到容器重启。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NginxActivation {
    Reloaded,
    Restarted,
}

/// nginx 激活动作的抽象：生产走 Docker；单测注入内存实现复现并发重启窗口。
#[allow(async_fn_in_trait)]
pub(crate) trait NginxControl {
    async fn container_running(&self) -> Result<bool, String>;
    async fn worker_generation(&self) -> Result<String, String>;
    async fn reload(&self) -> Result<(), String>;
    async fn restart(&self) -> Result<(), String>;
}

/// reload 后等待 worker 换代的轮询：50ms × 20 = 最多 1s。
const RELOAD_CONFIRM_TRIES: u32 = 20;
const RELOAD_CONFIRM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// 重载优先的激活流程（调用方必须持有 [`super::nginx_write_lock`]）。
///
/// - 容器未运行：直接走重启（重启也能把停止的容器拉起来）；
/// - reload 返回失败：回退重启；
/// - reload 成功但 worker 未换代：master 没有采纳新配置，回退重启，
///   让原有「重启后仍起不来就回滚」的失败语义继续兜底。
///
/// 不能只以「进程还活着」判断 reload 生效：master 读到坏配置时会继续用旧
/// worker 服务，进程同样活着。worker PID 集合换代才证明新配置已被加载。
pub(crate) async fn activate_with<N: NginxControl>(nginx: &N) -> Result<NginxActivation, String> {
    if nginx.container_running().await? {
        match nginx.worker_generation().await {
            Ok(before) if !before.is_empty() => match nginx.reload().await {
                Ok(()) => {
                    for _ in 0..RELOAD_CONFIRM_TRIES {
                        tokio::time::sleep(RELOAD_CONFIRM_INTERVAL).await;
                        if let Ok(after) = nginx.worker_generation().await {
                            if !after.is_empty() && after != before {
                                tracing::info!(
                                    before = %before,
                                    after = %after,
                                    "nginx 已平滑 reload（worker PID 换代）"
                                );
                                return Ok(NginxActivation::Reloaded);
                            }
                        }
                    }
                    tracing::warn!(
                        before = %before,
                        "nginx reload 后 worker 未换代，回退重启容器"
                    );
                }
                Err(e) => tracing::warn!(error = %e, "nginx reload 失败，回退重启容器"),
            },
            Ok(_) => tracing::warn!("未读到 nginx worker 进程，回退重启容器"),
            Err(e) => tracing::warn!(error = %e, "读取 nginx worker 进程失败，回退重启容器"),
        }
    }
    nginx.restart().await.map(|()| NginxActivation::Restarted)
}

/// 生产环境的 nginx 激活实现。
struct DockerNginx<'a> {
    state: &'a AppState,
}

impl NginxControl for DockerNginx<'_> {
    async fn container_running(&self) -> Result<bool, String> {
        let rows = self
            .state
            .docker()
            .list_containers()
            .await
            .map_err(|e| format!("读取容器列表失败：{e}"))?;
        let c = rows
            .iter()
            .find(|c| c.name == NGINX_CONTAINER)
            .ok_or_else(|| "nginx 运行环境未安装".to_string())?;
        Ok(c.state == "running")
    }

    async fn worker_generation(&self) -> Result<String, String> {
        nginx_worker_generation(self.state).await
    }

    async fn reload(&self) -> Result<(), String> {
        let first = match self
            .state
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec!["nginx".into(), "-s".into(), "reload".into()],
            )
            .await
        {
            Ok((0, _)) => return Ok(()),
            Ok((_, out)) => format!("nginx -s reload：{}", out.trim()),
            Err(e) => format!("nginx -s reload：{e}"),
        };
        // OpenResty 镜像的入口脚本名是 `openresty`，不同构建的 PATH 可能只暴露其一。
        if let Ok((0, _)) = self
            .state
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec!["openresty".into(), "-s".into(), "reload".into()],
            )
            .await
        {
            return Ok(());
        }
        // 最后按主配置里的 pid 文件直接给 master 发 SIGHUP（`nginx -s` 内部也是这么做的）。
        let hup = self
            .state
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec![
                    "sh".into(),
                    "-c".into(),
                    "kill -HUP \"$(cat /var/run/nginx.pid)\"".into(),
                ],
            )
            .await;
        match hup {
            Ok((0, _)) => Ok(()),
            Ok((_, out)) => Err(format!("{first}；kill -HUP：{}", out.trim())),
            Err(e) => Err(format!("{first}；kill -HUP：{e}")),
        }
    }

    async fn restart(&self) -> Result<(), String> {
        self.state
            .restart_nginx_locked()
            .await
            .map_err(|e| e.to_string())
    }
}

/// 容器内 nginx worker 进程集合（排序后）。
///
/// master 是容器 PID 1，`children` 直接给出 worker / cache manager；老内核没有该文件
/// （procfs 的 `stat` size 恒为 0，不能用 `-s` 判断）或不可读时，退化为按进程名扫描。
/// 探测只做「读文件 + 打标记」，判定留在 [`parse_worker_probe`] 里，便于单测覆盖。
async fn nginx_worker_generation(state: &AppState) -> Result<String, String> {
    const SCRIPT: &str = r#"if [ -r /proc/1/task/1/children ] && [ -n "$(cat /proc/1/task/1/children 2>/dev/null)" ]; then
    for p in $(cat /proc/1/task/1/children); do echo "C $p"; done
else
    for d in /proc/[0-9]*; do
        p=${d#/proc/}
        [ -r "$d/comm" ] || continue
        c=$(cat "$d/comm" 2>/dev/null) || continue
        [ -n "$c" ] && echo "S $p $c"
    done
fi"#;
    let (code, out) = state
        .docker()
        .exec_capture(
            NGINX_CONTAINER,
            vec!["sh".into(), "-c".into(), SCRIPT.into()],
        )
        .await
        .map_err(|e| format!("读取 nginx worker 进程失败：{e}"))?;
    if code != 0 {
        return Err(format!("读取 nginx worker 进程失败：{}", out.trim()));
    }
    let pids = parse_worker_probe(&out);
    if pids.is_empty() {
        return Err(format!("未读到 nginx worker 进程：{}", out.trim()));
    }
    Ok(pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" "))
}

/// 进程名是否属于 nginx 家族。OpenResty 镜像里 master 与 worker 的 `comm` 都是
/// `openresty`，只匹配 `nginx` 会让退化路径恒空、reload 分支永远走不到。
pub(crate) fn is_nginx_comm(comm: &str) -> bool {
    matches!(comm.trim(), "nginx" | "openresty")
}

/// 解析 worker 探测输出（纯函数）。
///
/// 每行两种形态：
/// - `C <pid>`：`/proc/1/task/1/children` 里的进程号；
/// - `S <pid> <comm>`：`children` 不可读/为空时，按 `comm` 退化扫描的结果。
///
/// 返回排序去重后的 worker PID 集合。master（PID 1）一律排除；退化路径只认
/// [`is_nginx_comm`] 的进程名，其余（`sh` / `cat` 等）丢弃。
pub(crate) fn parse_worker_probe(output: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("C") => {
                if let Some(pid) = parts.next().and_then(|p| p.parse::<u32>().ok()) {
                    if pid != 1 {
                        pids.push(pid);
                    }
                }
            }
            Some("S") => {
                let pid = parts.next().and_then(|p| p.parse::<u32>().ok());
                let comm = parts.next();
                if let (Some(pid), Some(comm)) = (pid, comm) {
                    if pid != 1 && is_nginx_comm(comm) {
                        pids.push(pid);
                    }
                }
            }
            _ => {}
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// 成对替换证书的失败落点。
///
/// 失败时**不允许**留下「vhost/库引用一个已不存在的 cert.pem」的悬空状态：
/// 原来有成对文件就恢复旧内容，原来没有就清理成「无证书」并由调用方同步
/// vhost 与库里的 HTTPS 标记。
#[derive(Debug)]
pub(crate) enum CertPairError {
    /// 替换前有成对文件：两个文件已恢复为替换前内容。
    Restored(SiteError),
    /// 替换前没有成对文件：已清理为「无证书」。
    Cleared(SiteError),
}

impl CertPairError {
    /// `true` 表示失败后处于「无证书」，调用方需要把 vhost/库同步成 HTTPS 关闭。
    pub(crate) fn was_cleared(&self) -> bool {
        matches!(self, Self::Cleared(_))
    }

    pub(crate) fn into_site_error(self) -> SiteError {
        match self {
            Self::Restored(e) | Self::Cleared(e) => e,
        }
    }
}

fn read_backup(path: &Path) -> Result<Option<Vec<u8>>, SiteError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SiteError::Internal(format!(
            "备份 {} 失败：{e}",
            path.display()
        ))),
    }
}

/// 恢复成备份内容；原本不存在则删除当前文件。
fn restore_backup(path: &Path, old: Option<&[u8]>, mode: u32) {
    match old {
        Some(bytes) => {
            if let Err(e) = crate::atomic::write_atomic(path, bytes, mode) {
                tracing::error!(path = %path.display(), error = %e, "恢复证书文件失败");
            }
        }
        None => remove_quietly(path),
    }
}

fn remove_quietly(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(path = %path.display(), error = %e, "清理临时/新证书文件失败");
        }
    }
}

/// 成对替换 cert/key。**调用方必须持有 [`super::nginx_write_lock`]**：
/// 两次 `rename` 之间不允许任何 nginx reload 插入，否则会读到
/// 「新证书 + 旧私钥」这种不配对组合（`key values mismatch`）。
///
/// 替换前先把现有文件读进内存备份；任一 `rename` 失败时两个文件都回到替换前
/// 的内容（而不是只删掉新证书，留下 vhost 指向不存在的文件）。
pub(crate) fn replace_cert_pair(
    tmp_cert: &Path,
    tmp_key: &Path,
    cert: &Path,
    key: &Path,
) -> Result<(), CertPairError> {
    let cert_old = match read_backup(cert) {
        Ok(v) => v,
        Err(e) => {
            remove_quietly(tmp_cert);
            remove_quietly(tmp_key);
            return Err(CertPairError::Restored(e));
        }
    };
    let key_old = match read_backup(key) {
        Ok(v) => v,
        Err(e) => {
            remove_quietly(tmp_cert);
            remove_quietly(tmp_key);
            return Err(CertPairError::Restored(e));
        }
    };
    let had_pair = cert_old.is_some() && key_old.is_some();

    let fail = |e: std::io::Error, what: &str| -> CertPairError {
        restore_backup(cert, cert_old.as_deref(), 0o644);
        restore_backup(key, key_old.as_deref(), 0o600);
        remove_quietly(tmp_cert);
        remove_quietly(tmp_key);
        let err = SiteError::Internal(format!("{what}失败：{e}"));
        if had_pair {
            CertPairError::Restored(err)
        } else {
            CertPairError::Cleared(err)
        }
    };

    if let Err(e) = std::fs::rename(tmp_cert, cert) {
        return Err(fail(e, "保存证书"));
    }
    if let Err(e) = std::fs::rename(tmp_key, key) {
        return Err(fail(e, "保存私钥"));
    }
    restrict_mode(cert, 0o644);
    restrict_mode(key, 0o600);
    Ok(())
}

impl AppState {
    /// 运行环境页「镜像参数」保存：nginx / PHP 基础镜像标签。
    ///
    /// 键名只在 `KEY_NGINX_IMAGE` / `KEY_PHP_IMAGE` 定义，handler 不再写字面量。
    pub fn save_runtime_images(
        &self,
        nginx: Option<&str>,
        php: Option<&str>,
    ) -> Result<(), String> {
        if let Some(v) = nginx {
            self.db()
                .set_setting(KEY_NGINX_IMAGE, v.trim())
                .map_err(|e| e.to_string())?;
        }
        if let Some(v) = php {
            self.db()
                .set_setting(KEY_PHP_IMAGE, v.trim())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Docker 设置页的镜像前缀（写入前去掉尾部 `/`，与读取侧口径一致）。
    pub fn save_image_prefix(&self, raw: &str) -> Result<String, String> {
        let value = raw.trim().trim_end_matches('/').to_string();
        self.db()
            .set_setting(KEY_PREFIX, &value)
            .map_err(|e| e.to_string())?;
        Ok(value)
    }

    /// Docker 设置页的加速源（落 daemon.json 由 `apply_registry_mirror` 负责，这里只记设置）。
    pub fn save_registry_mirror(&self, raw: &str) -> Result<(), String> {
        self.db()
            .set_setting(KEY_MIRROR, raw)
            .map_err(|e| e.to_string())
    }

    /// 把「网站 → 设置」里的镜像源落到宿主 `/etc/docker/daemon.json`。
    pub async fn apply_registry_mirror(
        &self,
        raw: &str,
    ) -> Result<crate::docker_daemon::MirrorApply, String> {
        crate::docker_daemon::apply_registry_mirror(
            self.docker(),
            std::path::Path::new(crate::docker_daemon::DAEMON_JSON),
            raw,
        )
        .await
    }

    pub fn site_settings(&self) -> SiteSettings {
        let db = self.db();
        let prefix = db
            .get_setting(KEY_PREFIX)
            .ok()
            .flatten()
            .unwrap_or_default();
        let pick = |key: &str, default: &str| -> String {
            let v = db
                .get_setting(key)
                .ok()
                .flatten()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| default.to_string());
            if prefix.trim().is_empty() || v.contains('/') {
                v
            } else {
                format!("{}/{}", prefix.trim_end_matches('/'), v)
            }
        };
        let port = DEFAULT_PORT;
        let web_root = self.cfg.web_root.clone();
        SiteSettings {
            sites_dir: web_root.join("sites"),
            conf_dir: web_root.join("nginx").join("conf.d"),
            runtime_dir: web_root.join("runtime"),
            data_dir: self.cfg.data_dir.clone(),
            nginx_image: pick(KEY_NGINX_IMAGE, DEFAULT_NGINX_IMAGE),
            php_image: pick(KEY_PHP_IMAGE, DEFAULT_PHP_IMAGE),
            port,
            registry_mirror: db
                .get_setting(KEY_MIRROR)
                .ok()
                .flatten()
                .unwrap_or_default(),
            web_root,
        }
    }

    /// 核验实际拉取的 PHP 镜像满足前提：`Config.User` 为空、带 `docker-php-entrypoint`。
    ///
    /// 不满足时明确拒绝，不静默换号或改用别的启动路径。
    pub(crate) async fn verify_php_image_identity(&self, image: &str) -> Result<(), String> {
        let info = self
            .docker()
            .inspect_image(image)
            .await
            .map_err(|e| format!("读取 PHP 镜像元数据失败：{e}"))?;
        let user = info.user.as_deref().unwrap_or("").trim();
        if !user.is_empty() {
            return Err(format!(
                "PHP 镜像 {image} 的 Config.User 为 {user}（要求为空），容器内无法建立统一站点账号"
            ));
        }
        if !info
            .entrypoint
            .iter()
            .any(|e| e.contains("docker-php-entrypoint"))
        {
            return Err(format!(
                "PHP 镜像 {image} 缺少 docker-php-entrypoint，未按官方入口约定，拒绝使用"
            ));
        }
        Ok(())
    }

    /// 面板写入站点文档根后归还统一身份（仅 `sites/<domain>/index` 子树）。
    ///
    /// 只处理写入目标本身：单文件保存/上传/新建目录走这里，避免每次写都遍历整棵树。
    pub fn restore_site_index_owner(&self, path: &str) {
        let p = Path::new(path);
        if is_site_index_path(p) {
            lchown_site(p);
        }
    }

    /// 同 [`Self::restore_site_index_owner`]，但递归处理整棵子树。
    ///
    /// 用于复制、解压与重命名这类可能一次生成/移动大量文件的动作。
    pub fn restore_site_index_owner_tree(&self, path: &str) {
        let p = Path::new(path);
        if is_site_index_path(p) {
            align_site_index_owner(p);
        }
    }

    pub async fn site_env_status(&self) -> SiteEnvStatus {
        let s = self.site_settings();
        // 默认 unknown：Docker 不可达时不能声称「未安装」。
        let mut nginx_state = "unknown".to_string();
        let mut nginx_image_id = String::new();
        let mut docker_known = false;
        if self.docker().available() {
            if let Ok(rows) = self.docker().list_containers().await {
                docker_known = true;
                nginx_state = "missing".into();
                for c in rows {
                    if c.name == NGINX_CONTAINER {
                        nginx_state = if c.state.is_empty() {
                            "unknown".into()
                        } else {
                            c.state.clone()
                        };
                        nginx_image_id = c.image_id.clone();
                    }
                }
            }
        }
        // 只有「Web 服务」容器的存在性：PHP 等语言运行环境是独立行，不再参与该判断。
        let installed = docker_known && nginx_state != "missing";
        let port_message = if nginx_state == "running" {
            format!("宿主 {} 端口由 {NGINX_CONTAINER}（host 网络）监听", s.port)
        } else {
            // 未运行时要如实探测：端口可能正被别的进程占用，别给「已在监听」的假象。
            match check_port_free(s.port) {
                Ok(()) => format!("宿主端口 {} 可用（nginx 状态：{nginx_state}）", s.port),
                Err(e) => format!("nginx 状态：{nginx_state}；{e}"),
            }
        };
        SiteEnvStatus {
            docker_available: self.docker().available(),
            docker_reason: self.docker().unavailable_reason().to_string(),
            nginx_state,
            nginx_image_id,
            installed,
            port: s.port,
            port_message,
            nginx_image: s.nginx_image,
            php_image: s.php_image,
        }
    }

    pub async fn site_install_progress(&self) -> InstallProgress {
        self.site_env.lock().await.clone()
    }

    /// 后台安装运行环境：拉镜像 → 建网络 → 建/启容器 → 写默认 vhost。
    ///
    /// 拉取可能超过 HTTP handler 的 30s 超时，所以这里只负责启动任务并立即返回；
    /// 页面用 meta refresh 轮询 `site_install_progress()`。
    pub fn start_web_env_install(self: &Arc<Self>) {
        let state = self.clone();
        tokio::spawn(async move {
            {
                let mut p = state.site_env.lock().await;
                if p.running {
                    return;
                }
                p.running = true;
                p.done = false;
                p.error.clear();
                p.steps = [
                    "检查 Docker 与 80 端口",
                    "准备目录与 nginx 主配置",
                    "拉取 OpenResty 镜像",
                    "重建站点 vhost",
                    "创建并启动 nginx（host 网络）",
                    "写入默认站点与兜底配置",
                ]
                .iter()
                .map(|n| InstallStep {
                    name: n.to_string(),
                    state: StepState::Pending,
                    detail: String::new(),
                })
                .collect();
            }
            let result = state.run_web_env_install().await;
            let mut p = state.site_env.lock().await;
            p.running = false;
            p.done = true;
            if let Err(e) = result {
                p.error = e;
            }
        });
    }

    async fn mark_step(&self, idx: usize, state: StepState, detail: impl Into<String>) {
        let mut p = self.site_env.lock().await;
        if let Some(step) = p.steps.get_mut(idx) {
            step.state = state;
            step.detail = detail.into();
        }
    }

    /// 只安装/重建 OpenResty（Web 服务）。PHP、Node、Go、Rust 各自独立安装，互不连带。
    async fn run_web_env_install(&self) -> Result<(), String> {
        let s = self.site_settings();
        let registry = registry_of(&s.nginx_image);
        let fail = |idx: usize, msg: String| async move {
            self.mark_step(idx, StepState::Fail, msg.clone()).await;
            Err(msg)
        };

        self.mark_step(0, StepState::Running, "").await;
        if !self.docker().available() {
            return fail(
                0,
                format!("Docker 不可用：{}", self.docker_unavailable_reason()),
            )
            .await;
        }
        let existing = match self.docker().list_containers().await {
            Ok(rows) => rows,
            Err(e) => {
                return fail(0, format!("读取容器列表失败：{e}")).await;
            }
        };
        // host 网络下 nginx 直接占宿主 80。只有「自家 nginx 正在运行」才跳过探测；
        // 容器停着/已退出（例如上次端口冲突）仍要探测，避免重建后起不来却报成功。
        if nginx_container_running(&existing) {
            self.mark_step(
                0,
                StepState::Ok,
                format!("宿主 {} 端口由现有 nginx 容器监听", s.port),
            )
            .await;
        } else {
            if let Err(e) = check_port_free(s.port) {
                return fail(0, e).await;
            }
        }
        self.mark_step(0, StepState::Ok, "Docker socket 可用").await;

        self.mark_step(1, StepState::Running, "只准备 Web 服务目录，不拉起 PHP")
            .await;
        // 绑定挂载的源目录必须先存在，否则 Docker 直接 400；权限 0755 让容器内
        // 非 root 的 nginx 进程可读。PHP 实例的目录/池配置由各自的安装任务负责。
        // 只建公共目录；站点级 index/log/ssl/conf 由建站时创建。
        let dirs = [
            &s.web_root,
            &s.sites_dir,
            &s.conf_dir,
            &s.runtime_dir,
            &s.global_log_dir(),
        ];
        for dir in dirs {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return fail(1, format!("创建 {} 失败：{e}", dir.display())).await;
            }
        }
        if let Err(e) = crate::nginx::ensure_nginx_main_conf(&s, self.db()) {
            return fail(1, format!("写入 nginx 主配置失败：{e}")).await;
        }
        if let Err(e) = self.write_default_vhost(&s) {
            return fail(1, format!("写入默认配置失败：{e}")).await;
        }
        self.mark_step(1, StepState::Ok, s.conf_dir.display().to_string())
            .await;

        self.mark_step(2, StepState::Running, format!("registry: {registry}"))
            .await;
        if let Err(e) = self
            .kernel_execute(
                Verb::ImagePull,
                None,
                DockerParams::ImagePull(s.nginx_image.clone()),
            )
            .await
        {
            return fail(
                2,
                format!("拉取 {} 失败（registry: {registry}）：{e}", s.nginx_image),
            )
            .await;
        }
        self.mark_step(2, StepState::Ok, format!("{} 已就绪", s.nginx_image))
            .await;

        self.mark_step(3, StepState::Running, "").await;
        // 按当前 `web_root` 布局重建 vhost；顺序放在启动 nginx 之前，nginx 起来时
        // 即可直接加载。
        let rebuilt = match self.rewrite_site_vhosts(&s) {
            Ok(n) => n,
            Err(e) => return fail(3, format!("重建站点配置失败：{e}")).await,
        };
        self.mark_step(3, StepState::Ok, format!("重建 {rebuilt} 个 vhost"))
            .await;

        // 容器：已存在则按当前配置重建（网络/挂载/镜像可能变了）；不存在则新建。
        self.mark_step(4, StepState::Running, "host 网络，监听宿主 80")
            .await;
        if let Some(c) = existing.iter().find(|c| c.name == NGINX_CONTAINER) {
            self.remove_existing_container(c).await?;
        }
        if let Err(e) = self.ensure_container(&self.nginx_spec(&s)).await {
            return fail(4, format!("创建/启动 nginx 失败：{e}")).await;
        }
        self.mark_step(4, StepState::Ok, NGINX_CONTAINER.to_string())
            .await;

        self.mark_step(5, StepState::Running, "").await;
        if let Err(e) = self.write_default_vhost(&s) {
            return fail(5, format!("写入默认配置失败：{e}")).await;
        }
        self.mark_step(5, StepState::Ok, s.conf_dir.display().to_string())
            .await;
        Ok(())
    }

    /// 移除同名旧容器。对象必须由引擎返回的真实行重建——`Actor::System` 的
    /// allowlist 只认内核标签，空 labels 会被判成非托管对象而拒绝。
    pub(crate) async fn remove_existing_container(&self, c: &ContainerRow) -> Result<(), String> {
        let obj = skiff_docker::ObjectRef {
            id: c.id.clone(),
            name: c.name.clone(),
            labels: c.labels.clone(),
        };
        self.kernel_execute(
            Verb::ContainerRemove,
            Some(&obj),
            DockerParams::ContainerRemove { force: true },
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("移除旧容器失败：{e}"))
    }

    /// 等 PHP-FPM 生成 socket 后，由内核强制 `root:nobody` + 0660。
    ///
    /// 容器内 `listen.group` 已经指向 nobody，但镜像/版本差异可能让 FPM 落盘时
    /// 带上别的组；这里以宿主准绳再收一次，绝不允许 world 可写。
    pub(crate) async fn secure_php_socket(
        &self,
        s: &SiteSettings,
        kind: &str,
        name: &str,
    ) -> Result<(), String> {
        let path = s.runtime_socket(kind, name);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while !path.exists() {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("等待 {} 生成超时", path.display()));
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{chown, PermissionsExt};
            chown(&path, Some(0), Some(SOCKET_GID))
                .map_err(|e| format!("设置 socket 属组 {SOCKET_GID} 失败：{e}"))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))
                .map_err(|e| format!("设置 socket 权限 0660 失败：{e}"))?;
            let mode = std::fs::metadata(&path)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode()
                & 0o777;
            if mode != SOCKET_MODE {
                return Err(format!("socket 权限复核失败：期望 0660，实际 {mode:o}"));
            }
        }
        Ok(())
    }

    fn write_default_vhost(&self, s: &SiteSettings) -> std::io::Result<()> {
        std::fs::create_dir_all(&s.conf_dir)?;
        // 兜底文件与站点 include 入口始终存在；默认站点的投影以库里 `is_default` 为准。
        std::fs::write(s.conf_dir.join(FALLBACK_CONF), FALLBACK_VHOST)?;
        std::fs::write(s.conf_dir.join(SITES_INCLUDE_CONF), s.sites_include_conf())?;
        self.sync_default_conf(s)
    }

    fn bound_runtime(&self, site: &Site) -> Result<Runtime, SiteError> {
        let rid = site
            .runtime_id
            .as_deref()
            .ok_or_else(|| SiteError::BadRequest("运行环境站点未绑定运行环境".into()))?;
        self.db()
            .runtime_by_id(rid)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))
    }

    /// 读取站点绑定 PHP 运行环境的 socket：返回（宿主路径，容器内路径）。
    /// 拒绝未登记的 socket，避免 vhost 指向不存在的端点。
    fn php_socket_paths(
        &self,
        s: &SiteSettings,
        site: &Site,
    ) -> Result<(String, String), SiteError> {
        let rid = site
            .runtime_id
            .as_deref()
            .ok_or_else(|| SiteError::BadRequest("PHP 站点未绑定运行环境".into()))?;
        let rt = self
            .db()
            .runtime_by_id(rid)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境不提供 PHP-FPM socket",
                rt.kind
            )));
        }
        let host = rt
            .socket_path
            .clone()
            .unwrap_or_else(|| s.runtime_socket(&rt.kind, &rt.name).display().to_string());
        let container = s.container_path(Path::new(&host));
        Ok((host, container))
    }

    /// 按站点类型生成 vhost 内容（静态 / PHP / 反向代理）。
    pub(crate) fn render_vhost_for(
        &self,
        s: &SiteSettings,
        site: &Site,
        aliases: &[String],
        tls: Option<&VhostTls>,
    ) -> Result<String, SiteError> {
        let site_dir = PathBuf::from(&site.root_dir);
        let root = s.container_path(&site_index_dir(&site_dir));
        let access = s.container_log(&site_dir, LogKind::Access);
        let error = s.container_log(&site_dir, LogKind::Error);
        let runtime = if site.site_type == TYPE_RUNTIME {
            Some(self.bound_runtime(site)?)
        } else {
            None
        };
        let socket = if runtime
            .as_ref()
            .is_some_and(|r| r.kind == skiff_store::KIND_PHP)
        {
            Some(self.php_socket_paths(s, site)?.1)
        } else {
            None
        };
        let kind = match site.site_type.as_str() {
            TYPE_STATIC => VhostKind::Static { root: &root },
            TYPE_PROXY => VhostKind::Proxy {
                target: site.proxy_target.as_deref().unwrap_or(""),
                websocket: site.proxy_websocket,
            },
            _ => match runtime.as_ref().map(|r| r.kind.as_str()) {
                Some(kind @ (skiff_store::KIND_RUST | skiff_store::KIND_NODE)) => {
                    VhostKind::Process {
                        root: &root,
                        port: runtime.as_ref().and_then(|r| r.port).ok_or_else(|| {
                            SiteError::Unavailable(format!(
                                "{} 运行环境缺少端口，请重新安装",
                                runtime_catalog::kind_label(kind)
                            ))
                        })?,
                    }
                }
                Some(skiff_store::KIND_GO) => {
                    return Err(SiteError::Unavailable(
                        "Go 运行环境尚未接入，当前不能生成 vhost".into(),
                    ))
                }
                _ => VhostKind::Php {
                    root: &root,
                    socket: socket.as_deref().unwrap_or(""),
                },
            },
        };
        Ok(render_site_vhost(
            &site.domain,
            aliases,
            kind,
            &access,
            &error,
            tls,
        ))
    }

    /// 默认站点 vhost：与普通站点同一套类型分支，只把 listen 换成 `default_server`。
    fn render_default_vhost_for(
        &self,
        s: &SiteSettings,
        site: &Site,
        tls: Option<&VhostTls>,
    ) -> Result<String, SiteError> {
        let site_dir = PathBuf::from(&site.root_dir);
        let root = s.container_path(&site_index_dir(&site_dir));
        let access = s.container_log(&site_dir, LogKind::Access);
        let error = s.container_log(&site_dir, LogKind::Error);
        let runtime = if site.site_type == TYPE_RUNTIME {
            Some(self.bound_runtime(site)?)
        } else {
            None
        };
        let socket = if runtime
            .as_ref()
            .is_some_and(|r| r.kind == skiff_store::KIND_PHP)
        {
            Some(self.php_socket_paths(s, site)?.1)
        } else {
            None
        };
        let kind = match site.site_type.as_str() {
            TYPE_STATIC => VhostKind::Static { root: &root },
            TYPE_PROXY => VhostKind::Proxy {
                target: site.proxy_target.as_deref().unwrap_or(""),
                websocket: site.proxy_websocket,
            },
            _ => match runtime.as_ref().map(|r| r.kind.as_str()) {
                Some(kind @ (skiff_store::KIND_RUST | skiff_store::KIND_NODE)) => {
                    VhostKind::Process {
                        root: &root,
                        port: runtime.as_ref().and_then(|r| r.port).ok_or_else(|| {
                            SiteError::Unavailable(format!(
                                "{} 运行环境缺少端口，请重新安装",
                                runtime_catalog::kind_label(kind)
                            ))
                        })?,
                    }
                }
                Some(skiff_store::KIND_GO) => {
                    return Err(SiteError::Unavailable(
                        "Go 运行环境尚未接入，当前不能生成 vhost".into(),
                    ))
                }
                _ => VhostKind::Php {
                    root: &root,
                    socket: socket.as_deref().unwrap_or(""),
                },
            },
        };
        Ok(render_default_vhost(
            &site.domain,
            kind,
            &access,
            &error,
            tls,
        ))
    }

    /// 把 `00-skiff-default.conf` 与库里的默认站点对齐：有则生成，无则删除。
    fn sync_default_conf(&self, s: &SiteSettings) -> std::io::Result<()> {
        let conf = s.conf_dir.join(DEFAULT_CONF);
        match self.db().default_site() {
            Ok(Some(site)) => {
                let tls = self.site_tls(s, &site);
                let content = self
                    .render_default_vhost_for(s, &site, tls.as_ref())
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                write_if_changed(&conf, content.as_bytes(), 0o644).map_err(std::io::Error::other)
            }
            Ok(None) => {
                if conf.exists() {
                    std::fs::remove_file(&conf)?;
                }
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e.to_string())),
        }
    }

    /// 让 nginx 按当前配置生效（reload 优先，必要时回退重启）。
    ///
    /// 持锁执行：并发的保存/证书操作不会在同一个重启窗口里互相踩踏。
    async fn activate_nginx_locked(&self) -> Result<NginxActivation, String> {
        activate_with(&DockerNginx { state: self }).await
    }

    pub async fn restart_nginx(&self) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.restart_nginx_locked().await
    }

    /// 重启容器。**调用方必须已持有 [`super::nginx_write_lock`]**。
    async fn restart_nginx_locked(&self) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == NGINX_CONTAINER)
            .ok_or_else(|| SiteError::Unavailable("nginx 运行环境未安装".into()))?;
        let obj = skiff_docker::ObjectRef {
            id: c.id.clone(),
            name: c.name.clone(),
            labels: c.labels.clone(),
        };
        self.kernel_execute(Verb::Restart, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| SiteError::Unavailable(format!("重载 nginx 失败：{e}")))
    }

    /// 站点启用 HTTPS 且证书文件齐全时，返回容器内证书路径。
    fn site_tls(&self, s: &SiteSettings, site: &Site) -> Option<VhostTls> {
        if !site.https {
            return None;
        }
        let site_dir = PathBuf::from(&site.root_dir);
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        if !cert.is_file() || !key.is_file() {
            return None;
        }
        Some(VhostTls {
            cert: s.container_path(&cert),
            key: s.container_path(&key),
            force_https: site.force_https,
        })
    }

    /// 容器内 `nginx -t`；失败返回可读错误（调用方负责回滚）。
    pub async fn validate_nginx(&self) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.validate_nginx_locked().await
    }

    /// 同上。**调用方必须已持有 [`super::nginx_write_lock`]**，否则可能在证书
    /// 两次 rename 之间读到不配对的文件。
    async fn validate_nginx_locked(&self) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        if !nginx_container_running(&rows) {
            return Err(SiteError::Unavailable(
                "nginx 运行环境未启动，无法校验配置".into(),
            ));
        }
        let (code, out) = self
            .docker()
            .exec_capture(NGINX_CONTAINER, vec!["nginx".into(), "-t".into()])
            .await
            .map_err(|e| SiteError::Unavailable(format!("执行 nginx -t 失败：{e}")))?;
        if code == 0 {
            Ok(())
        } else {
            Err(SiteError::Conflict(format!(
                "nginx -t 未通过：{}",
                out.trim()
            )))
        }
    }

    /// 站点证书信息（供 SSL 标签展示）。
    pub fn site_tls_info(&self, domain: &str) -> Result<SiteTlsInfo, SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        let mut info = SiteTlsInfo {
            enabled: site.https,
            force_https: site.force_https,
            cert_path: cert.display().to_string(),
            key_path: key.display().to_string(),
            cert_exists: s.cert_ready(&site_dir),
            subject: None,
            issuer: None,
            expires: None,
        };
        if info.cert_exists {
            let text = cached_cert_text(&cert, &key);
            info.subject = text.subject;
            info.issuer = text.issuer;
            info.expires = text.expires;
        } else {
            // 站点删除或证书被删：清掉旧条目，避免同名目录下次复用时读到陈旧解析。
            tls_info_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&cert);
        }
        Ok(info)
    }

    /// 面板内生成自签证书（CN + SAN 都写域名与别名），再启用 HTTPS。
    pub async fn generate_self_signed(&self, domain: &str) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let aliases = self.site_aliases(domain);
        let dir = site_ssl_dir(&site_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| SiteError::Internal(format!("创建证书目录失败：{e}")))?;
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        // 先生成到进程内唯一临时名，再在锁内成对替换：openssl 生成期间不占锁，
        // 也不会让 nginx 读到「新证书 + 旧私钥」的中间态。
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let tmp_key = crate::atomic::sibling_temp(&key);
        let mut san = format!("DNS:{}", site.domain);
        for a in &aliases {
            san.push_str(&format!(",DNS:{a}"));
        }
        let (cert2, key2) = (tmp_cert.clone(), tmp_key.clone());
        let domain2 = site.domain.clone();
        let status = tokio::task::spawn_blocking(move || {
            std::process::Command::new("openssl")
                .args([
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-sha256",
                    "-days",
                    "825",
                    "-nodes",
                    "-keyout",
                    key2.to_str().unwrap_or_default(),
                    "-out",
                    cert2.to_str().unwrap_or_default(),
                    "-subj",
                    &format!("/CN={domain2}"),
                    "-addext",
                    &format!("subjectAltName={san}"),
                ])
                .output()
        })
        .await
        .map_err(|e| SiteError::Internal(format!("执行 openssl 失败：{e}")))?;
        let out = status
            .map_err(|e| SiteError::Internal(format!("无法执行 openssl（请安装 openssl）：{e}")))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&tmp_cert);
            let _ = std::fs::remove_file(&tmp_key);
            return Err(SiteError::Internal(format!(
                "生成自签证书失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let _guard = super::nginx_write_lock().lock().await;
        if let Err(failure) = replace_cert_pair(&tmp_cert, &tmp_key, &cert, &key) {
            if failure.was_cleared() {
                self.repair_no_cert_state(domain).await;
            }
            return Err(failure.into_site_error());
        }
        self.apply_tls_locked(domain, true, false).await
    }

    /// 上传证书与私钥（PEM 文本）；校验证书/私钥有效且配对后启用 HTTPS。
    pub async fn upload_cert(
        &self,
        domain: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if !cert_pem.contains("BEGIN CERTIFICATE") || !key_pem.contains("PRIVATE KEY") {
            return Err(SiteError::BadRequest(
                "证书需为 PEM（BEGIN CERTIFICATE），私钥需包含 PRIVATE KEY".into(),
            ));
        }
        let site_dir = PathBuf::from(&site.root_dir);
        let dir = site_ssl_dir(&site_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| SiteError::Internal(format!("创建证书目录失败：{e}")))?;
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        // 唯一临时名：同名并发上传各写各的，不会互相截断或抢先 rename。
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let tmp_key = crate::atomic::sibling_temp(&key);
        std::fs::write(&tmp_cert, cert_pem)
            .map_err(|e| SiteError::Internal(format!("写入证书失败：{e}")))?;
        std::fs::write(&tmp_key, key_pem)
            .map_err(|e| SiteError::Internal(format!("写入私钥失败：{e}")))?;
        restrict_mode(&tmp_cert, 0o644);
        restrict_mode(&tmp_key, 0o600);
        if let Err(e) = verify_cert_pair(&tmp_cert, &tmp_key) {
            if let Err(ce) = std::fs::remove_file(&tmp_cert) {
                tracing::debug!(path = %tmp_cert.display(), error = %ce, "清理临时证书失败");
            }
            if let Err(ke) = std::fs::remove_file(&tmp_key) {
                tracing::debug!(path = %tmp_key.display(), error = %ke, "清理临时私钥失败");
            }
            return Err(SiteError::BadRequest(e));
        }
        // 成对替换与 reload 必须在一把锁内完成：中间态对 nginx 不可见。
        let _guard = super::nginx_write_lock().lock().await;
        if let Err(failure) = replace_cert_pair(&tmp_cert, &tmp_key, &cert, &key) {
            if failure.was_cleared() {
                self.repair_no_cert_state(domain).await;
            }
            return Err(failure.into_site_error());
        }
        self.apply_tls_locked(domain, true, false).await
    }

    /// 关闭 HTTPS（保留证书文件）。
    pub async fn disable_tls(&self, domain: &str) -> Result<(), SiteError> {
        self.apply_tls(domain, false, false).await
    }

    /// 删除证书文件并关闭 HTTPS。
    pub async fn delete_cert(&self, domain: &str) -> Result<(), SiteError> {
        // 先关 HTTPS 再删文件，两步都在锁内：不会与上传/生成交错。
        let _guard = super::nginx_write_lock().lock().await;
        self.apply_tls_locked(domain, false, false).await?;
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let s = self.site_settings();
        let dir = site_ssl_dir(Path::new(&site.root_dir));
        // 只删证书文件，保留 ssl/ 目录（它是站点固定布局的一部分）。
        for p in [
            s.cert_file(Path::new(&site.root_dir)),
            s.key_file(Path::new(&site.root_dir)),
        ] {
            if p.exists() {
                std::fs::remove_file(&p)
                    .map_err(|e| SiteError::Internal(format!("删除证书文件失败：{e}")))?;
            }
        }
        // 清理上传留下的中间件（唯一名以 `.cert.pem.` / `.key.pem.` 开头），避免半套。
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let is_upload_tmp = (name.starts_with(".cert.pem.")
                    || name.starts_with(".key.pem."))
                    && name.ends_with(".tmp");
                if !is_upload_tmp {
                    continue;
                }
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    tracing::debug!(path = %entry.path().display(), error = %e, "清理残留的临时证书失败");
                }
            }
        }
        Ok(())
    }

    /// 切换「强制 HTTPS」；未启用 HTTPS 时拒绝。
    pub async fn set_force_https(&self, domain: &str, force: bool) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if force && !site.https {
            return Err(SiteError::BadRequest(
                "请先启用 HTTPS（上传或生成证书）".into(),
            ));
        }
        self.apply_tls(domain, site.https, force).await
    }

    /// 改写站点 vhost + 默认站 vhost，校验 `nginx -t` 后让 nginx 生效；任一步失败回滚文件与库状态。
    async fn apply_tls(
        &self,
        domain: &str,
        https: bool,
        force_https: bool,
    ) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.apply_tls_locked(domain, https, force_https).await
    }

    /// 同上。**调用方必须已持有 [`super::nginx_write_lock`]**（证书成对写入后还要
    /// 在同一个临界区内完成 reload）。
    async fn apply_tls_locked(
        &self,
        domain: &str,
        https: bool,
        force_https: bool,
    ) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let (old_https, old_force) = (site.https, site.force_https);
        let site_dir = PathBuf::from(&site.root_dir);
        if https && (!s.cert_file(&site_dir).is_file() || !s.key_file(&site_dir).is_file()) {
            return Err(SiteError::BadRequest(
                "缺少证书文件，请先上传或生成自签证书".into(),
            ));
        }
        let site_conf = if site.status == "running" {
            s.vhost_file(&site_dir, domain)
        } else {
            s.vhost_disabled(&site_dir, domain)
        };
        let default_conf = s.conf_dir.join(DEFAULT_CONF);
        let snapshot: Vec<(PathBuf, Option<Vec<u8>>)> = [&site_conf, &default_conf]
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(p).ok()))
            .collect();
        let restore = |snapshot: &[(PathBuf, Option<Vec<u8>>)]| {
            for (p, old) in snapshot {
                match old {
                    Some(bytes) => {
                        if let Err(e) = std::fs::write(p, bytes) {
                            tracing::error!(path = %p.display(), error = %e, "回滚站点配置失败");
                        }
                    }
                    None => {
                        if let Err(e) = std::fs::remove_file(p) {
                            tracing::error!(path = %p.display(), error = %e, "回滚站点配置失败（删除新增文件）");
                        }
                    }
                }
            }
        };

        self.db()
            .set_site_tls(domain, https, force_https)
            .map_err(|e| SiteError::Internal(format!("写入 HTTPS 状态失败：{e}")))?;
        if let Err(e) = self.write_site_vhost(&s, domain) {
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(e);
        }
        if let Err(e) = self.sync_default_conf(&s) {
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(SiteError::Internal(format!("写入默认站点配置失败：{e}")));
        }
        if let Err(e) = self.validate_nginx_locked().await {
            // 校验失败时运行中的 nginx 还没加载新配置，恢复文件即可；
            // 这里**不能再重启**——若失败源于别的坏 conf，重启会让现网 nginx 起不来。
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(e);
        }
        if let Err(e) = self.activate_nginx_locked().await {
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            if let Err(re) = self.restart_nginx_locked().await {
                tracing::error!(error = %re, "回滚后重启 nginx 失败，现网可能仍加载失败配置");
            }
            return Err(SiteError::Unavailable(e));
        }
        Ok(())
    }

    /// 证书替换失败且原本没有成对文件时的收尾：把库里的 HTTPS 标记与 vhost 里的
    /// `ssl_certificate` 引用一起改回「无证书」。否则会留下「vhost 声明证书、
    /// 文件却不存在」的悬空状态，下一次 `nginx -t` 直接失败。
    ///
    /// **调用方必须已持有 [`super::nginx_write_lock`]**。这里只做尽力而为的修复：
    /// 每一步失败都记日志，但原始错误优先返回给调用方。
    pub(crate) async fn repair_no_cert_state(&self, domain: &str) {
        if let Err(e) = self.db().set_site_tls(domain, false, false) {
            tracing::error!(domain, error = %e, "证书写入失败后回滚 HTTPS 状态失败");
        }
        let s = self.site_settings();
        if let Err(e) = self.write_site_vhost(&s, domain) {
            tracing::error!(domain, error = %e, "证书写入失败后重写站点配置失败");
        }
        if let Err(e) = self.sync_default_conf(&s) {
            tracing::error!(domain, error = %e, "证书写入失败后重写默认站点配置失败");
        }
        if let Err(e) = self.validate_nginx_locked().await {
            tracing::warn!(error = %e, "证书写入失败后 nginx -t 未通过（配置已不含证书引用）");
            return;
        }
        if let Err(e) = self.activate_nginx_locked().await {
            tracing::warn!(error = %e, "证书写入失败后 nginx 重载失败");
        }
    }

    /// 按库里的最新状态重写单个站点 vhost（运行中写 .conf，停止写 .conf.disabled）。
    pub(crate) fn write_site_vhost(&self, s: &SiteSettings, domain: &str) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        std::fs::create_dir_all(site_conf_dir(&site_dir))
            .map_err(|e| SiteError::Internal(format!("创建站点配置目录失败：{e}")))?;
        let aliases = self.site_aliases(&site.domain);
        let tls = self.site_tls(s, &site);
        let vhost = self.render_vhost_for(s, &site, &aliases, tls.as_ref())?;
        let conf = s.vhost_file(&site_dir, &site.domain);
        let disabled = s.vhost_disabled(&site_dir, &site.domain);
        if site.status == "running" {
            std::fs::write(&conf, vhost)
                .map_err(|e| SiteError::Internal(format!("写入站点配置失败：{e}")))?;
            if let Err(e) = std::fs::remove_file(&disabled) {
                tracing::debug!(path = %disabled.display(), error = %e, "清理已禁用的站点配置失败");
            }
        } else {
            std::fs::write(&disabled, vhost)
                .map_err(|e| SiteError::Internal(format!("写入站点配置失败：{e}")))?;
            if let Err(e) = std::fs::remove_file(&conf) {
                tracing::debug!(path = %conf.display(), error = %e, "清理生效中的站点配置失败");
            }
        }
        Ok(())
    }

    /// 按当前 `web_root` 布局重建所有已登记站点的 vhost（安装/迁移后调用）。
    /// 已停止的站点写成 `.conf.disabled`，与启停逻辑保持一致。
    fn rewrite_site_vhosts(&self, s: &SiteSettings) -> std::io::Result<usize> {
        let sites = self
            .db()
            .list_sites()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::create_dir_all(&s.conf_dir)?;
        std::fs::write(s.conf_dir.join(SITES_INCLUDE_CONF), s.sites_include_conf())?;
        for site in &sites {
            let site_dir = PathBuf::from(&site.root_dir);
            std::fs::create_dir_all(site_conf_dir(&site_dir))?;
            let aliases = self.site_aliases(&site.domain);
            let tls = self.site_tls(s, site);
            let vhost = self
                .render_vhost_for(s, site, &aliases, tls.as_ref())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let conf = s.vhost_file(&site_dir, &site.domain);
            let disabled = s.vhost_disabled(&site_dir, &site.domain);
            if site.status == "running" {
                std::fs::write(&conf, vhost)?;
                if let Err(e) = std::fs::remove_file(&disabled) {
                    tracing::debug!(path = %disabled.display(), error = %e, "清理已禁用的站点配置失败");
                }
            } else {
                std::fs::write(&disabled, vhost)?;
                if let Err(e) = std::fs::remove_file(&conf) {
                    tracing::debug!(path = %conf.display(), error = %e, "清理生效中的站点配置失败");
                }
            }
        }
        Ok(sites.len())
    }

    pub async fn create_site(&self, req: NewSite) -> Result<Site, SiteError> {
        let domain = validate_domain(&req.domain)?;
        let site_type = validate_site_type(&req.site_type)?;
        // 别名先做语法与去重校验，再查库冲突；拒绝与主域名重复。
        let aliases = normalize_site_aliases(&domain, &req.aliases)?;
        let s = self.site_settings();
        let env = self.site_env_status().await;
        if !env.docker_available || !env.installed || env.nginx_state != "running" {
            return Err(SiteError::Unavailable(
                "Web 服务（OpenResty）未就绪：请先在「网站 → 运行环境」安装/启动 Web 服务".into(),
            ));
        }

        // 类型专属校验。允许 PHP / Node.js / Rust 运行环境；Go 未接入，如实拒绝。
        let mut runtime_id = None;
        let mut runtime_kind = String::new();
        let mut runtime_version = String::new();
        let mut runtime_port = None;
        let mut proxy_target = None;
        let mut proxy_websocket = false;
        match site_type {
            TYPE_RUNTIME => {
                let rid = req
                    .runtime_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| SiteError::BadRequest("请选择运行环境".into()))?;
                let rt = self
                    .db()
                    .runtime_by_id(rid)
                    .map_err(|e| SiteError::Internal(e.to_string()))?
                    .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))?;
                if !runtime_catalog::is_implemented(&rt.kind) {
                    return Err(SiteError::Unavailable(format!(
                        "{} 运行环境尚未接入，当前版本不能创建该类站点",
                        rt.kind
                    )));
                }
                if rt.desired_state != "running" || rt.last_status != "running" {
                    return Err(SiteError::Unavailable(format!(
                        "语言运行环境 {} 当前未运行，请先在「运行环境」页启动",
                        rt.version
                    )));
                }
                if rt.kind == skiff_store::KIND_PHP {
                    let socket = rt.socket_path.as_deref().ok_or_else(|| {
                        SiteError::Unavailable("运行环境缺少 socket 路径，请先重新安装".into())
                    })?;
                    if !Path::new(socket).exists() {
                        return Err(SiteError::Unavailable(format!(
                            "PHP socket 未就绪：{socket}（请检查 FPM 是否已启动）"
                        )));
                    }
                } else if rt.port.is_none() {
                    return Err(SiteError::Unavailable(
                        "运行环境缺少固定端口，请先重新安装".into(),
                    ));
                }
                runtime_kind = rt.kind.clone();
                runtime_version = rt.version.clone();
                runtime_port = rt.port;
                runtime_id = Some(rid.to_string());
            }
            TYPE_PROXY => {
                let target = validate_proxy_target(&req.proxy_target)?;
                let authority = target
                    .split_once("://")
                    .map(|(_, r)| r)
                    .unwrap_or("")
                    .split('/')
                    .next()
                    .unwrap_or("");
                let host = authority.split(':').next().unwrap_or("");
                if host.eq_ignore_ascii_case(&domain)
                    || aliases.iter().any(|a| a.eq_ignore_ascii_case(host))
                {
                    return Err(SiteError::BadRequest(
                        "上游地址不能指向站点自身，否则会形成回环".into(),
                    ));
                }
                proxy_target = Some(target);
                proxy_websocket = req.proxy_websocket;
            }
            _ => {}
        }

        let db = self.db();
        if db
            .site_by_domain(&domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(SiteError::Conflict(format!("站点已存在：{domain}")));
        }
        for alias in &aliases {
            if db
                .site_by_domain(alias)
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .is_some()
            {
                return Err(SiteError::Conflict(format!("域名已被站点使用：{alias}")));
            }
            if let Some(owner) = db
                .alias_owner(alias)
                .map_err(|e| SiteError::Internal(e.to_string()))?
            {
                return Err(SiteError::Conflict(format!(
                    "域名已作为 {owner} 的别名存在：{alias}"
                )));
            }
        }
        let root = match req
            .root_dir
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            Some(raw) => {
                let normalized = normalize_dir(raw);
                // 站点目录必须直接挂在 sites/ 下一层：主配置用 `sites/*/conf/*.conf`
                // 通配加载，深层目录不会被扫到，因此在这里直接拒绝。
                let path = Path::new(&normalized);
                if !is_absolute(&normalized)
                    || !within(&normalized, &s.sites_dir.display().to_string())
                    || path.parent() != Some(s.sites_dir.as_path())
                {
                    return Err(SiteError::BadRequest(format!(
                        "站点目录必须是 {} 下的一级子目录",
                        s.sites_dir.display()
                    )));
                }
                PathBuf::from(normalized)
            }
            None => s.sites_dir.join(&domain),
        };
        // 一站一目录：log/、ssl/、conf/ 必建；index/ 只属于 runtime/static。
        for dir in [
            &root,
            &site_log_dir(&root),
            &site_ssl_dir(&root),
            &site_conf_dir(&root),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| SiteError::Internal(format!("创建 {} 失败：{e}", dir.display())))?;
        }
        match site_type {
            TYPE_RUNTIME => {
                std::fs::create_dir_all(site_index_dir(&root))
                    .map_err(|e| SiteError::Internal(format!("创建站点文档根失败：{e}")))?;
                // 按运行环境类型给初始文件：只有 PHP 写 index.php 探针；Node.js / Rust
                // 的代码固定在环境 `app/`，站点目录保持为空，不再写脚手架。
                for (rel, content) in
                    runtime_site_default_files(&runtime_kind, &runtime_version, runtime_port)
                {
                    let file = site_index_dir(&root).join(rel);
                    if let Some(parent) = file.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            SiteError::Internal(format!("创建 {} 失败：{e}", parent.display()))
                        })?;
                    }
                    if !file.exists() {
                        std::fs::write(&file, content).map_err(|e| {
                            SiteError::Internal(format!("写入 {} 失败：{e}", file.display()))
                        })?;
                    }
                }
            }
            TYPE_STATIC => {
                std::fs::create_dir_all(site_index_dir(&root))
                    .map_err(|e| SiteError::Internal(format!("创建站点文档根失败：{e}")))?;
                let index = site_index_dir(&root).join("index.html");
                if !index.exists() {
                    std::fs::write(&index, default_probe_html())
                        .map_err(|e| SiteError::Internal(format!("写入 index.html 失败：{e}")))?;
                }
            }
            _ => {}
        }
        // 文档根统一归属站点身份：宿主 FTP 与容器内 PHP 都以同一 uid 写这棵树。
        align_site_index_owner(&site_index_dir(&root));

        let site = db
            .create_site(&StoreNewSite {
                domain: domain.clone(),
                root_dir: root.display().to_string(),
                site_type: site_type.into(),
                runtime_id,
                proxy_target,
                proxy_websocket,
            })
            .map_err(|e| SiteError::Conflict(format!("保存站点失败：{e}")))?;
        if let Err(e) = db.add_site_aliases(&domain, &aliases) {
            // 别名写失败时把站点行一起撤回，避免半成品。
            if let Err(re) = db.delete_site(&domain) {
                tracing::error!(site = %domain, error = %re, "回滚站点行失败");
            }
            return Err(SiteError::Conflict(format!("保存站点别名失败：{e}")));
        }

        // 保证公共配置（默认站投影 + 兜底站 + 站点 include 入口）齐备。
        let rollback = |conf: &Path| {
            if let Err(e) = self.db().delete_site(&domain) {
                tracing::error!(site = %domain, error = %e, "回滚站点行失败");
            }
            if let Err(e) = std::fs::remove_file(conf) {
                tracing::error!(path = %conf.display(), error = %e, "回滚站点配置文件失败");
            }
        };
        if let Err(e) = self.write_default_vhost(&s) {
            rollback(&s.vhost_file(&root, &domain));
            return Err(SiteError::Internal(format!("写入公共配置失败：{e}")));
        }
        let vhost = match self.render_vhost_for(&s, &site, &aliases, None) {
            Ok(v) => v,
            Err(e) => {
                rollback(&s.vhost_file(&root, &domain));
                return Err(e);
            }
        };
        let conf = s.vhost_file(&root, &domain);
        if let Err(e) = std::fs::write(&conf, &vhost) {
            rollback(&conf);
            return Err(SiteError::Internal(format!("写入 vhost 失败：{e}")));
        }
        if let Err(e) = self.restart_nginx().await {
            rollback(&conf);
            return Err(e);
        }
        Ok(site)
    }

    pub fn site_aliases(&self, domain: &str) -> Vec<String> {
        self.db().site_aliases(domain).unwrap_or_default()
    }

    /// 站点详情页增删别名后统一走这里：重建 vhost → `nginx -t` → reload。
    /// 任一步失败都把库里的别名回滚到 `previous`，避免库与配置半套。
    async fn apply_site_aliases(&self, site: &Site, previous: &[String]) -> Result<(), SiteError> {
        let s = self.site_settings();
        let rollback = |this: &Self| {
            let db = this.db();
            if let Err(e) = db.clear_site_aliases(&site.domain) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点别名失败（清理）");
            }
            if let Err(e) = db.add_site_aliases(&site.domain, previous) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点别名失败（恢复）");
            }
            if let Err(e) = this.write_site_vhost(&s, &site.domain) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点配置失败");
            }
        };
        if let Err(e) = self.write_site_vhost(&s, &site.domain) {
            rollback(self);
            return Err(e);
        }
        if let Err(e) = self.validate_nginx().await {
            rollback(self);
            return Err(e);
        }
        if let Err(e) = self.restart_nginx().await {
            rollback(self);
            return Err(e);
        }
        Ok(())
    }

    /// 添加别名：语法 → 与主域名/已有别名/其它站点冲突，再落库并重建 vhost。
    pub async fn add_site_alias(&self, domain: &str, raw: &str) -> Result<String, SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let alias = validate_domain(raw)?;
        if alias == site.domain {
            return Err(SiteError::BadRequest("别名不能与主域名相同".into()));
        }
        let previous = self.site_aliases(&site.domain);
        if previous.contains(&alias) {
            return Err(SiteError::Conflict(format!("别名已存在：{alias}")));
        }
        let db = self.db();
        if db
            .site_by_domain(&alias)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(SiteError::Conflict(format!("域名已被站点使用：{alias}")));
        }
        if let Some(owner) = db
            .alias_owner(&alias)
            .map_err(|e| SiteError::Internal(e.to_string()))?
        {
            return Err(SiteError::Conflict(format!(
                "域名已作为 {owner} 的别名存在：{alias}"
            )));
        }
        db.add_site_aliases(&site.domain, std::slice::from_ref(&alias))
            .map_err(|e| SiteError::Conflict(format!("保存别名失败：{e}")))?;
        self.apply_site_aliases(&site, &previous).await?;
        Ok(alias)
    }

    /// 删除别名：主域名不可删；别名必须属于该站点。
    pub async fn remove_site_alias(&self, domain: &str, raw: &str) -> Result<String, SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let alias = validate_domain(raw)?;
        if alias == site.domain {
            return Err(SiteError::BadRequest("主域名不能作为别名删除".into()));
        }
        let previous = self.site_aliases(&site.domain);
        if !previous.contains(&alias) {
            return Err(SiteError::BadRequest(format!("别名不存在：{alias}")));
        }
        self.db()
            .remove_site_alias(&site.domain, &alias)
            .map_err(|e| SiteError::Internal(format!("删除别名失败：{e}")))?;
        self.apply_site_aliases(&site, &previous).await?;
        Ok(alias)
    }

    pub async fn set_site_running(&self, domain: &str, running: bool) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let conf = s.vhost_file(&site_dir, domain);
        let disabled = s.vhost_disabled(&site_dir, domain);
        if running && site.site_type == TYPE_RUNTIME {
            // 运行环境站点不能在环境停着时假装启动。PHP 还要求 FPM socket 存在；
            // Rust 是「独立进程 + 固定端口」，只看最后状态，不能套 PHP 的 socket 条件。
            let rid = site
                .runtime_id
                .as_deref()
                .ok_or_else(|| SiteError::BadRequest("运行环境站点未绑定运行环境".into()))?;
            let rt = self
                .db()
                .runtime_by_id(rid)
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))?;
            ensure_runtime_ready_for_start(&rt)?;
        }
        let mut activated = false;
        if running {
            // 先把即将被 include 的配置（停止态是 .conf.disabled）做一次隔离校验，
            // 非法内容绝不进入线上 include，避免启动即把全局 nginx 带入 crash-loop。
            let target = if conf.exists() {
                conf.clone()
            } else if disabled.exists() {
                disabled.clone()
            } else {
                return Err(SiteError::Internal(
                    "站点配置缺失：请重新安装运行环境以重建 vhost".into(),
                ));
            };
            let content = std::fs::read_to_string(&target)
                .map_err(|e| SiteError::Internal(format!("读取站点配置失败：{e}")))?;
            self.validate_site_vhost_content(&s, domain, &content, &content)
                .await
                .map_err(|e| SiteError::Conflict(format!("拒绝启动：站点配置未通过校验（{e}）")))?;
            if !conf.exists() && disabled.exists() {
                std::fs::rename(&disabled, &conf)
                    .map_err(|e| SiteError::Internal(format!("启用配置失败：{e}")))?;
                activated = true;
            }
            // 再跑一次完整的线上 include 校验：单站合法但多站 conflict 也要拦。
            if let Err(e) = self.validate_nginx().await {
                if activated {
                    if let Err(re) = std::fs::rename(&conf, &disabled) {
                        tracing::error!(error = %re, "回滚站点配置重命名失败");
                    }
                }
                return Err(SiteError::Conflict(format!(
                    "拒绝启动：站点配置未通过全局 nginx -t（{e}）"
                )));
            }
            if let Err(e) = self.restart_nginx().await {
                if activated {
                    if let Err(re) = std::fs::rename(&conf, &disabled) {
                        tracing::error!(error = %re, "回滚站点配置重命名失败");
                    }
                    if let Err(re) = self.restart_nginx().await {
                        tracing::error!(error = %re, "回滚后重载 nginx 失败");
                    }
                }
                return Err(SiteError::Unavailable(format!("启动站点失败：{e}")));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let env = self.site_env_status().await;
            if env.installed && env.nginx_state != "running" {
                if activated {
                    if let Err(re) = std::fs::rename(&conf, &disabled) {
                        tracing::error!(error = %re, "回滚站点配置重命名失败");
                    }
                    if let Err(re) = self.restart_nginx().await {
                        tracing::error!(error = %re, "回滚后重载 nginx 失败");
                    }
                }
                return Err(SiteError::Unavailable(format!(
                    "启动站点失败：nginx 未能保持运行（当前状态 {}）",
                    env.nginx_state
                )));
            }
        } else if conf.exists() {
            std::fs::rename(&conf, &disabled)
                .map_err(|e| SiteError::Internal(format!("停用配置失败：{e}")))?;
            // 与启动分支对称：reload 失败必须把 vhost 恢复回去，否则数据库仍记
            // running、磁盘上却已停用，之后任何一次 nginx 重启站点都会消失。
            if let Err(e) = self.restart_nginx().await {
                if let Err(re) = std::fs::rename(&disabled, &conf) {
                    tracing::error!(error = %re, "回滚站点停用失败");
                } else if let Err(re) = self.restart_nginx().await {
                    tracing::error!(error = %re, "回滚后重载 nginx 失败");
                }
                return Err(SiteError::Unavailable(format!("停用站点失败：{e}")));
            }
        }
        self.db()
            .set_site_status(&site.domain, if running { "running" } else { "stopped" })
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        Ok(())
    }

    /// 设为/取消默认站点。库是状态源，conf 是投影：**先落库、再写文件并 reload**；
    /// 文件或 reload 失败时把库与 conf 一起恢复到切换前，避免留下
    /// 「conf 已切、库没变」或反向的不一致。
    pub async fn set_default_site(&self, domain: &str, enabled: bool) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if enabled && site.status != "running" {
            return Err(SiteError::BadRequest(
                "站点已停止，请先启动再设为默认".into(),
            ));
        }
        if !enabled && !site.is_default {
            return Ok(());
        }
        let prev = self
            .db()
            .default_site()
            .map_err(|e| SiteError::Internal(e.to_string()))?;

        self.db()
            .set_site_default(domain, enabled)
            .map_err(|e| SiteError::Internal(e.to_string()))?;

        let applied = if enabled {
            self.write_default_conf(&s, &site).await
        } else {
            self.remove_default_conf(&s).await
        };
        if let Err(e) = applied {
            // 库已切换但文件/reload 没跟上：按切换前的默认站点回滚库，再让 conf
            // 重新与库对齐。reload 本身失败时 nginx 仍在跑旧配置，所以不需要再 reload。
            let restored = match prev.as_ref() {
                Some(prev) => self.db().set_site_default(&prev.domain, true),
                None => self.db().set_site_default(domain, false),
            };
            if let Err(re) = restored {
                tracing::error!(error = %re, "回滚默认站点库状态失败");
            }
            if let Err(re) = self.sync_default_conf(&s) {
                tracing::error!(error = %re, "回滚默认站点配置失败");
            }
            return Err(e);
        }
        Ok(())
    }

    /// 写 `00-skiff-default.conf` 并 reload nginx。
    async fn write_default_conf(&self, s: &SiteSettings, site: &Site) -> Result<(), SiteError> {
        std::fs::create_dir_all(&s.conf_dir)
            .map_err(|e| SiteError::Internal(format!("创建配置目录失败：{e}")))?;
        if !s.conf_dir.join(FALLBACK_CONF).exists() {
            std::fs::write(s.conf_dir.join(FALLBACK_CONF), FALLBACK_VHOST)
                .map_err(|e| SiteError::Internal(format!("写入兜底配置失败：{e}")))?;
        }
        if !s.conf_dir.join(SITES_INCLUDE_CONF).exists() {
            std::fs::write(s.conf_dir.join(SITES_INCLUDE_CONF), s.sites_include_conf())
                .map_err(|e| SiteError::Internal(format!("写入站点 include 失败：{e}")))?;
        }
        let tls = self.site_tls(s, site);
        let content = self.render_default_vhost_for(s, site, tls.as_ref())?;
        // 原子替换：写到一半失败时旧 conf 仍在，不会让 nginx 读到半截配置。
        write_if_changed(&s.conf_dir.join(DEFAULT_CONF), content.as_bytes(), 0o644)
            .map_err(|e| SiteError::Internal(format!("写入默认站点配置失败：{e}")))?;
        self.restart_nginx().await
    }

    /// 删除 `00-skiff-default.conf` 并 reload nginx。
    async fn remove_default_conf(&self, s: &SiteSettings) -> Result<(), SiteError> {
        let conf = s.conf_dir.join(DEFAULT_CONF);
        match std::fs::remove_file(&conf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(SiteError::Internal(format!("删除默认站点配置失败：{e}"))),
        }
        self.restart_nginx().await
    }

    /// 删除确认页的具体量：目录体积/文件数、关联定时任务数、运行环境绑定。
    pub fn site_delete_stats(&self, domain: &str) -> Result<SiteDeleteStats, SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let (dir_bytes, file_count) = dir_usage(Path::new(&site.root_dir));
        let source = format!("site:{}", site.domain);
        let task_count = self
            .db()
            .scheduled_tasks()
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .iter()
            .filter(|t| t.source == source)
            .count();
        let (runtime_name, runtime_kind) = if site.site_type == TYPE_RUNTIME {
            site.runtime_id
                .as_deref()
                .and_then(|rid| self.db().runtime_by_id(rid).ok().flatten())
                .map(|rt| (rt.name, rt.kind))
                .unwrap_or_default()
        } else {
            (String::new(), String::new())
        };
        Ok(SiteDeleteStats {
            dir_bytes,
            file_count,
            task_count,
            runtime_name,
            runtime_kind,
        })
    }

    pub async fn delete_site(&self, domain: &str) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        // 站点目录删除后不可恢复，先读出待级联清理由任务；此前用
        // `unwrap_or_default()` 会把「读库失败」当成「没有任务」而静默漏清理。
        let stale_tasks: Vec<String> = if self.tasks().is_some() {
            let source = format!("site:{}", site.domain);
            self.db()
                .scheduled_tasks()
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .into_iter()
                .filter(|t| t.source == source)
                .map(|t| t.id)
                .collect()
        } else {
            Vec::new()
        };
        // 删除默认站点时连同默认 conf 一起删，不能留下指向已删目录的 vhost。
        if site.is_default {
            let p = s.conf_dir.join(DEFAULT_CONF);
            if p.exists() {
                std::fs::remove_file(&p)
                    .map_err(|e| SiteError::Internal(format!("删除默认站点配置失败：{e}")))?;
            }
        }
        // 整站目录一次性删除：index/log/ssl/conf 随站点一起消失。
        // 用 canonical 路径比对（而不是字符串 `/` 前缀），既挡住任意路径删除，
        // 也保证开发机 Windows 路径下同样生效；站点目录本身不允许等于 sites/。
        let root = PathBuf::from(&site.root_dir);
        let sites_root = s
            .sites_dir
            .canonicalize()
            .unwrap_or_else(|_| s.sites_dir.clone());
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
        if root.exists() && root_canon != sites_root && within_root(&sites_root, &root_canon) {
            remove_dir_all_retry(&root).map_err(|e| {
                SiteError::Internal(format!("删除站点目录 {} 失败：{e}", root.display()))
            })?;
        }
        if let Err(e) = self.restart_nginx().await {
            tracing::warn!(error = %e, "删除站点后重载 nginx 失败");
        }
        self.db()
            .delete_site(&site.domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // 级联清理站点定时任务：否则 `builtin:site-backup:<域名>` 会留成指向已不存在
        // 站点的孤儿任务。站点备份是内核种入的 `builtin:` 任务，普通删除入口会被
        // 内置保护拒绝，这里必须走级联删除（仍经任务服务，保证调度器内存与库一致）。
        if let Some(tasks) = self.tasks() {
            for id in stale_tasks {
                if let Err(e) = tasks.delete_task_cascade(&id).await {
                    tracing::warn!(task = %id, error = %e, "删除站点后清理定时任务失败");
                }
            }
        }
        Ok(())
    }

    /// 读取站点日志：宿主文件、时间倒序、可选关键字过滤（均只作用于文件尾部窗口）。
    ///
    /// 读取本身不再触发轮转：轮转由 30s 巡检统一做（改名后让 nginx 重开日志），
    /// 避免「看一下日志就复制整个文件」的写放大。
    pub fn site_logs(
        &self,
        domain: &str,
        tail: usize,
        keyword: &str,
    ) -> Result<(String, String), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let read = |kind: LogKind| -> String {
            let p = s.log_file(&site_dir, kind);
            match read_log(&p, tail, keyword) {
                Ok(t) if t.is_empty() && !keyword.trim().is_empty() => {
                    format!("（没有匹配「{}」的日志行）", keyword.trim())
                }
                Ok(t) if t.is_empty() => "（暂无日志）".into(),
                Ok(t) => t,
                Err(e) => format!("（读取 {} 失败：{e}）", p.display()),
            }
        };
        Ok((read(LogKind::Access), read(LogKind::Error)))
    }

    /// 站点 vhost 的落盘路径：运行中读 `.conf`，已停止读 `.conf.disabled`。
    fn site_vhost_path(&self, domain: &str) -> Result<(PathBuf, bool), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let active = s.vhost_file(&site_dir, domain);
        if active.exists() {
            return Ok((active, true));
        }
        let disabled = s.vhost_disabled(&site_dir, domain);
        if disabled.exists() {
            return Ok((disabled, false));
        }
        Err(SiteError::BadRequest(format!(
            "站点 vhost 文件不存在：{}",
            active.display()
        )))
    }

    /// 读取当前 vhost 内容（用于配置文件页编辑）。
    pub fn read_site_vhost(&self, domain: &str) -> Result<(String, String, bool), SiteError> {
        let (path, active) = self.site_vhost_path(domain)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| SiteError::Internal(format!("读取 {} 失败：{e}", path.display())))?;
        Ok((path.display().to_string(), content, active))
    }

    /// 用候选内容做隔离校验：临时 main 只 include 候选文件，不触碰线上 include。
    async fn validate_site_vhost_content(
        &self,
        s: &SiteSettings,
        domain: &str,
        original: &str,
        content: &str,
    ) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        if !nginx_container_running(&rows) {
            return Err(SiteError::Unavailable(
                "Web 服务（OpenResty）未运行，无法校验配置".into(),
            ));
        }
        let dir = s.web_root.join("skiff").join("vhost-check");
        std::fs::create_dir_all(&dir)
            .map_err(|e| SiteError::Internal(format!("创建校验目录失败：{e}")))?;
        let stamp = chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default();
        let candidate = dir.join(format!("{domain}-{stamp}.conf.candidate"));
        let main = dir.join(format!("{domain}-{stamp}.check-main.candidate"));
        let container_candidate = s.container_path(&candidate);
        let container_main = s.container_path(&main);
        std::fs::write(&candidate, content)
            .map_err(|e| SiteError::Internal(format!("写入候选配置失败：{e}")))?;
        std::fs::write(&main, vhost_check_main(&container_candidate))
            .map_err(|e| SiteError::Internal(format!("写入校验主配置失败：{e}")))?;
        let result = self
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec![
                    "nginx".into(),
                    "-t".into(),
                    "-c".into(),
                    container_main.clone(),
                ],
            )
            .await;
        if let Err(ce) = std::fs::remove_file(&candidate) {
            tracing::debug!(path = %candidate.display(), error = %ce, "清理 nginx -t 候选配置失败");
        }
        if let Err(me) = std::fs::remove_file(&main) {
            tracing::debug!(path = %main.display(), error = %me, "清理 nginx -t 临时主配置失败");
        }
        let (code, out) =
            result.map_err(|e| SiteError::Unavailable(format!("执行 nginx -t 失败：{e}")))?;
        if code == 0 {
            Ok(())
        } else {
            Err(SiteError::Conflict(format!(
                "站点配置校验未通过：{}；差异：{}",
                crate::nginx::error_line(&out),
                crate::nginx::diff_summary(original, content, 6)
            )))
        }
    }

    /// 保存 vhost：先隔离校验候选内容，通过后才落盘；运行中的站点再让 nginx 生效
    /// （优先平滑 reload），停止的站点只落盘不重载（`.conf.disabled` 不在线上 include 内）。
    pub async fn save_site_vhost(
        &self,
        domain: &str,
        content: &str,
    ) -> Result<VhostSaveOutcome, SiteError> {
        if content.trim().is_empty() {
            return Err(SiteError::BadRequest("配置内容不能为空".into()));
        }
        if !content.contains("server") {
            return Err(SiteError::BadRequest(
                "配置至少需要包含一个 server 块".into(),
            ));
        }
        let s = self.site_settings();
        let (path, active) = self.site_vhost_path(domain)?;
        let original = std::fs::read(&path)
            .map_err(|e| SiteError::Internal(format!("读取原配置失败：{e}")))?;
        let original_text = String::from_utf8_lossy(&original).to_string();
        // 无论启停都先校验：停止态的文件不在 include 内，直接 reload 等于没测。
        self.validate_site_vhost_content(&s, domain, &original_text, content)
            .await?;
        // 写入与生效在同一把锁内：并发保存同一站点不会互相吃半套配置，也不会
        // 与本模块的证书成对写入（`upload_cert` / `generate_self_signed`）交错。
        let _guard = super::nginx_write_lock().lock().await;
        // 先写临时文件再 rename，避免半截文件被 nginx 加载；临时名进程内唯一，
        // 并发保存同一站点时不会出现「后写者删掉先写者临时文件」。
        crate::atomic::write_atomic(&path, content.as_bytes(), 0o644)
            .map_err(|e| SiteError::Internal(format!("替换配置失败：{e}")))?;
        if !active {
            return Ok(VhostSaveOutcome { reloaded: false });
        }
        match self.activate_nginx_locked().await {
            // reload 已由 worker 换代自证生效：服务不中断，也不必再等启动窗口。
            Ok(NginxActivation::Reloaded) => Ok(VhostSaveOutcome { reloaded: true }),
            Ok(NginxActivation::Restarted) => {
                // 回退重启：返回成功不代表 nginx 配置有效，容器仍可能在启动后立即退出。
                // 留出极短启动窗口再核对状态，失败则回滚并尽量把旧配置重新拉起。
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let env = self.site_env_status().await;
                if env.installed && env.nginx_state != "running" {
                    if let Err(re) = crate::atomic::write_atomic(&path, &original, 0o644) {
                        tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
                    }
                    if let Err(re) = self.restart_nginx_locked().await {
                        tracing::error!(error = %re, "回滚后重载 nginx 失败");
                    }
                    return Err(SiteError::Unavailable(format!(
                        "配置已回滚到保存前内容：nginx 未能保持运行（当前状态 {}）",
                        env.nginx_state
                    )));
                }
                Ok(VhostSaveOutcome { reloaded: true })
            }
            Err(e) => {
                // 回滚文件内容，保证磁盘与正在运行的 nginx 保持一致。
                if let Err(re) = crate::atomic::write_atomic(&path, &original, 0o644) {
                    tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
                }
                Err(SiteError::Unavailable(format!(
                    "配置已回滚到保存前内容：重载 nginx 失败：{e}"
                )))
            }
        }
    }

    /// 站点根目录只读浏览；`rel` 为空表示根。拒绝 `..` 与越界路径。
    pub fn site_dir_listing(&self, domain: &str, rel: &str) -> Result<SiteDirListing, SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        // 「站点根目录」只授权 `index/`：log/ 与 ssl/ 绝不能被目录浏览/文件能力读到。
        let root = site_index_dir(&site_dir);
        let rel_clean = rel.trim().trim_matches('/');
        if rel_clean.split('/').any(|seg| seg == "..") {
            return Err(SiteError::BadRequest("目录不能包含 ..".into()));
        }
        let target = if rel_clean.is_empty() {
            root.clone()
        } else {
            root.join(rel_clean)
        };
        // canonical 比较同时挡住 `..` 与符号链接逃逸。
        let site_canon = site_dir
            .canonicalize()
            .map_err(|e| SiteError::Internal(format!("解析站点目录失败：{e}")))?;
        let root_canon = root
            .canonicalize()
            .map_err(|e| SiteError::Internal(format!("站点缺少 index/ 目录：{e}")))?;
        if !within_root(&site_canon, &root_canon) {
            return Err(SiteError::BadRequest(
                "站点根目录（index/）超出站点目录".into(),
            ));
        }
        // `index/` 本身若是指向 log/ 或 ssl/ 的符号链接，同样拒绝。
        for (dir, label) in [
            (site_log_dir(&site_dir), "log/"),
            (site_ssl_dir(&site_dir), "ssl/"),
        ] {
            if let Ok(canon) = dir.canonicalize() {
                if within_root(&canon, &root_canon) {
                    return Err(SiteError::BadRequest(format!("站点根目录不能指向 {label}")));
                }
            }
        }
        let target_canon = target
            .canonicalize()
            .map_err(|e| SiteError::BadRequest(format!("目录不存在：{e}")))?;
        if !within_root(&root_canon, &target_canon) {
            return Err(SiteError::BadRequest("目录超出站点根目录（index/）".into()));
        }
        if !target.is_dir() {
            return Err(SiteError::BadRequest("目标不是目录".into()));
        }
        let mut entries = Vec::new();
        let rd = std::fs::read_dir(&target)
            .map_err(|e| SiteError::Internal(format!("读取目录失败：{e}")))?;
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let is_dir = meta.is_dir();
            let child_rel = if rel_clean.is_empty() {
                name.clone()
            } else {
                format!("{rel_clean}/{name}")
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push(SiteDirEntry {
                name,
                rel: child_rel,
                is_dir,
                size: if is_dir { 0 } else { meta.len() },
                modified,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let parent = rel_clean
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();
        Ok(SiteDirListing {
            at_root: rel_clean.is_empty(),
            rel: rel_clean.to_string(),
            parent,
            entries,
        })
    }

    /// 站点访问探针：用 Host 头访问本机 nginx 端口，返回状态码与响应开头。
    pub async fn probe_site(&self, domain: &str) -> Result<(u16, String), String> {
        let s = self.site_settings();
        let host = domain.to_string();
        let port = s.port;
        tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let addr = format!("127.0.0.1:{port}");
            let mut stream = std::net::TcpStream::connect(&addr)
                .map_err(|e| format!("连接 {addr} 失败：{e}"))?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .ok();
            let req = format!("GET / HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
            stream
                .write_all(req.as_bytes())
                .map_err(|e| format!("发送请求失败：{e}"))?;
            let mut buf = Vec::new();
            // 只取响应头部窗口做预览；此前 read_to_end 无上限，
            // 站点根放一个大文件时点一次探针就会吃满内核内存。
            stream
                .take(64 * 1024)
                .read_to_end(&mut buf)
                .map_err(|e| format!("读取响应失败：{e}"))?;
            let text = String::from_utf8_lossy(&buf).to_string();
            let code = text
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(0);
            let preview: String = text.chars().take(400).collect();
            Ok((code, preview))
        })
        .await
        .map_err(|e| format!("探针任务失败：{e}"))?
    }
}
