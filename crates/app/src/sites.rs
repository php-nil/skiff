//! 内核自带「网站」：运行环境（OpenResty + PHP-FPM）与站点闭环。
//!
//! 分层：这里只依赖下层的 `skiff_store` / `skiff_docker`，不含路由与模板；
//! 所有动词都经 `DockerService::execute` 单点分发，不自己拼 Engine HTTP。
//!
//! 宿主目录约定（`web_root` 默认 `/www`，**一个站点一个目录**）：
//!
//! ```text
//! /www/sites/<域名>/
//!   index/  网站根目录（文档根）
//!   log/    access.log / error.log
//!   ssl/    cert.pem / key.pem
//!   conf/   <域名>.conf（停止时改名 .conf.disabled）
//! ```
//!
//! 备份 `/www/sites/<域名>/` 即备份整站。内核公共配置（默认站、兜底站、站点
//! include 入口）仍在 `/www/nginx/conf.d`（挂到容器 `/etc/nginx/conf.d`）；
//! 每个 PHP 实例一个独立 socket：`/www/runtime/php/<name>/php-fpm.sock`。
//!
//! nginx 容器整目录挂载 `/www`，PHP 容器同样挂载 `/www`（每版本一个容器、
//! 一个池配置与一个 socket）。nginx 使用 host 网络直接监听宿主 80；PHP-FPM
//! 只走 unix socket，不暴露 9000。容器内路径一律由
//! [`SiteSettings::container_path`] 从宿主路径换算，禁止手拼。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::{ContainerRow, ContainerSpec, DockerParams, Mount, Verb};
use skiff_model::{is_absolute, normalize_dir, within, within_root};
use skiff_store::{
    NewRuntime, NewSite as StoreNewSite, Runtime, Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC,
};

use crate::jobs as app_jobs;
use crate::runtime_catalog;
use crate::AppState;

mod health;
mod layout;
mod logs;
mod spec;
pub use health::*;
pub use layout::*;
pub use logs::*;
pub use spec::*;

pub const NGINX_CONTAINER: &str = "skiff-web-nginx";
/// 每个 PHP 实例一个独立容器：`skiff-php-<name>`。
pub const PHP_CONTAINER_PREFIX: &str = "skiff-php";

// 扩展 exec 的硬超时默认值在 `WebConfig`（单扩展 15 分钟、批量 30 分钟），
// 可用 `ext_exec_timeout_secs` / `ext_batch_timeout_secs` 或环境变量覆盖。
// 超时只停止等待（Docker 无 kill exec 接口），按失败结算并保留已采集输出尾巴。
/// 超时错误标记：`job_finish_err` 依据它把 `error_class` 记为 `timeout`。
const TIMEOUT_MARK: &str = "timeout: ";

/// 超时展示：秒级配置（测试用）不能显示成「0 分钟」。
fn fmt_timeout(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{secs} 秒")
    }
}

/// 任务日志时间戳（本地时区，仅展示用）。
fn fmt_log_time(at: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(at, 0)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// 任务中心错误 → 站点错误：忙碌时给出可行动提示。
fn site_error_from_job(e: app_jobs::JobError) -> SiteError {
    match e {
        app_jobs::JobError::Busy(m) => SiteError::Conflict(format!(
            "该对象已有任务在执行（{m}），请等待完成或取消后再试"
        )),
        app_jobs::JobError::NotFound(m) | app_jobs::JobError::BadRequest(m) => {
            SiteError::BadRequest(m)
        }
        app_jobs::JobError::Store(m) => SiteError::Internal(m),
    }
}
/// 单文件日志轮转阈值（内核自己轮转，不依赖宿主 logrotate）。
pub const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// 站点文档根目录名（`index`）；其余固定子目录见 `sites::layout`。
pub use skiff_model::SITE_INDEX_DIR;

/// 公共配置目录里的站点 include 入口：主配置 `include /etc/nginx/conf.d/*.conf`
/// 先加载本文件，再由它把各站点自己的 `conf/*.conf` 拉进来。
/// 文件名保持 `10-` 前缀，排在 `00-skiff-default.conf` / `00-skiff-fallback.conf` 之后。
pub const SITES_INCLUDE_CONF: &str = "10-skiff-sites.conf";

const KEY_PREFIX: &str = "sites.image_prefix";
const KEY_NGINX_IMAGE: &str = "sites.nginx_image";
const KEY_PHP_IMAGE: &str = "sites.php_image";
const KEY_MIRROR: &str = "sites.registry_mirror";

/// 默认站点 vhost：有默认站点时由代码生成，无默认站点时该文件不存在。
const DEFAULT_CONF: &str = "00-skiff-default.conf";
/// 兜底 vhost：始终存在。**无默认站点**时它是 80 端口的默认 server，未命中任何
/// 站点域名的裸访问返回 404；**有默认站点**时 `default_server` 由默认站接管，
/// 未知 Host 落到默认站（1Panel 行为），不再经过本文件。
const FALLBACK_CONF: &str = "00-skiff-fallback.conf";
// `server_name` 不能用 `_`：默认站 vhost 也用 `_`，nginx 会报
// "conflicting server name \"_\" on 0.0.0.0:80"。本文件只负责「没有默认站时的
// 裸访问 404」，用一个不会与站点域名冲突的保留名即可。
const FALLBACK_VHOST: &str =
    "# Skiff 兜底站点：无默认站点时，未命中任何站点域名的裸访问返回 404。\n\
     server {\n    listen 80;\n    server_name skiff-fallback;\n    return 404;\n}\n";

/// 站点的宿主路径与镜像配置（默认值即宿主直装形态）。
///
/// `conf_dir` 是**内核公共配置目录**（默认站、兜底站、站点 include 入口），
/// 不是某个站点自己的 `conf/`；站点级路径都由站点目录派生。
#[derive(Debug, Clone)]
pub struct SiteSettings {
    pub web_root: PathBuf,
    pub sites_dir: PathBuf,
    pub conf_dir: PathBuf,
    /// 运行环境根目录；每个 PHP 版本在 `runtime/php-<version>/` 下独立成套。
    pub runtime_dir: PathBuf,
    /// 数据根：PHP 容器身份初始化脚本等宿主托管文件放在这里。
    pub data_dir: PathBuf,
    pub nginx_image: String,
    pub php_image: String,
    pub port: u16,
    pub registry_mirror: String,
}

/// 删除站点确认页要展示的「具体量」：目录体积、文件数、关联任务与运行环境绑定。
#[derive(Debug, Clone, Default)]
pub struct SiteDeleteStats {
    pub dir_bytes: u64,
    pub file_count: u64,
    pub task_count: usize,
    /// 绑定的运行环境名；无绑定为空。
    pub runtime_name: String,
    /// 绑定运行环境的 kind（`php` / `rust` …）；无绑定为空。
    pub runtime_kind: String,
}

/// 判断宿主路径是否位于某个站点的 `index/` 子树（`<web_root>/sites/<domain>/index/...`）。
pub use skiff_model::is_site_index_path;

/// SSL 标签展示用的证书状态。
#[derive(Debug, Clone)]
pub struct SiteTlsInfo {
    pub enabled: bool,
    pub force_https: bool,
    pub cert_path: String,
    pub key_path: String,
    pub cert_exists: bool,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub expires: Option<String>,
}

/// 证书文本字段（`openssl x509` 的解析结果），缓存的就是这一小块。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TlsCertText {
    subject: Option<String>,
    issuer: Option<String>,
    expires: Option<String>,
}

/// `(mtime, size)`：证书/私钥被改写、重新生成或删除时至少有一项变化。
type FileStamp = (Option<std::time::SystemTime>, u64);

#[derive(Debug, Clone)]
struct TlsInfoCacheEntry {
    cert: FileStamp,
    key: FileStamp,
    text: TlsCertText,
}

/// 证书解析缓存上限：站点数远小于它，超限时顺手回收已删除站点的条目。
const TLS_INFO_CACHE_MAX: usize = 256;

/// SSL 标签的进程内缓存（key = 证书路径）。
///
/// 详情页每次渲染都会问一次证书信息，而 `openssl x509` 是 fork + PEM 解析，
/// 比一次 `stat` 贵几个数量级。失效判定用证书与私钥的 `mtime` + `size`：
/// 上传、重新生成自签证书都会改写文件；站点删除后文件不存在，走
/// `AppState::site_tls_info` 的「不存在」分支并清掉旧条目。
static TLS_INFO_CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, TlsInfoCacheEntry>>> =
    std::sync::OnceLock::new();

fn tls_info_cache() -> &'static std::sync::Mutex<HashMap<PathBuf, TlsInfoCacheEntry>> {
    // 缓存只是性能优化：锁中毒时取回内部数据继续用，不能因此让证书页永久 500。
    TLS_INFO_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn file_stamp(path: &Path) -> FileStamp {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    }
}

/// 解析证书文本字段。失败（openssl 缺失/输出异常）时返回空字段，不影响页面。
fn parse_cert_text(cert: &Path) -> TlsCertText {
    let mut text = TlsCertText::default();
    let Ok(out) = std::process::Command::new("openssl")
        .args([
            "x509",
            "-in",
            &cert.display().to_string(),
            "-noout",
            "-subject",
            "-issuer",
            "-enddate",
        ])
        .output()
    else {
        return text;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(v) = line.strip_prefix("subject=") {
            text.subject = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("issuer=") {
            text.issuer = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("notAfter=") {
            text.expires = Some(v.trim().to_string());
        }
    }
    text
}

/// 命中缓存直接返回，否则跑一次 openssl 再写回。
///
/// 锁只保护 map 读写，**不跨 openssl 调用持有**：并发未命中时最多重复解析一次，
/// 也不会把等锁的线程（含 async worker）堵在 fork 上。
fn cached_cert_text(cert: &Path, key: &Path) -> TlsCertText {
    let cert_stamp = file_stamp(cert);
    let key_stamp = file_stamp(key);
    {
        let map = tls_info_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = map.get(cert) {
            if entry.cert == cert_stamp && entry.key == key_stamp {
                return entry.text.clone();
            }
        }
    }
    let text = parse_cert_text(cert);
    let mut map = tls_info_cache().lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= TLS_INFO_CACHE_MAX {
        map.retain(|path, _| path.exists());
    }
    map.insert(
        cert.to_path_buf(),
        TlsInfoCacheEntry {
            cert: cert_stamp,
            key: key_stamp,
            text: text.clone(),
        },
    );
    text
}

/// 校验证书与私钥：能解析、且公钥一致。返回中文错误供表单展示。
fn verify_cert_pair(cert: &Path, key: &Path) -> Result<(), String> {
    let out = |args: &[&str]| -> Result<String, String> {
        let o = std::process::Command::new("openssl")
            .args(args)
            .output()
            .map_err(|e| format!("无法执行 openssl（请安装 openssl）：{e}"))?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&o.stdout).to_string())
    };
    let cert_s = cert.to_string_lossy().to_string();
    let key_s = key.to_string_lossy().to_string();
    out(&["x509", "-in", &cert_s, "-noout"]).map_err(|e| format!("证书无法解析：{e}"))?;
    out(&["pkey", "-in", &key_s, "-noout"]).map_err(|e| format!("私钥无法解析：{e}"))?;
    let cert_pub = out(&["x509", "-in", &cert_s, "-noout", "-pubkey"])
        .map_err(|e| format!("读取证书公钥失败：{e}"))?;
    let key_pub =
        out(&["pkey", "-in", &key_s, "-pubout"]).map_err(|e| format!("读取私钥公钥失败：{e}"))?;
    if cert_pub.trim() != key_pub.trim() {
        return Err("证书与私钥不匹配".into());
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        tracing::debug!(path = %path.display(), error = %e, "设置权限失败");
    }
}

#[cfg(not(unix))]
fn restrict_mode(_path: &Path, _mode: u32) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Access,
    Error,
}

impl LogKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LogKind::Access => "access",
            LogKind::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Ok,
    Fail,
}

impl StepState {
    pub fn as_str(self) -> &'static str {
        match self {
            StepState::Pending => "pending",
            StepState::Running => "running",
            StepState::Ok => "ok",
            StepState::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallStep {
    pub name: String,
    pub state: StepState,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct InstallProgress {
    pub running: bool,
    pub done: bool,
    pub error: String,
    pub steps: Vec<InstallStep>,
}

/// 运行环境安装互斥表：键为 `kind:name`。
///
/// 只防「同一实例被并发安装」；不同实例互不影响。守卫 drop 时自动释放，
/// 安装失败也不会把实例永久锁死。
#[derive(Default)]
pub struct InstallLocks {
    busy: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl std::fmt::Debug for InstallLocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallLocks").finish_non_exhaustive()
    }
}

pub struct InstallLock<'a> {
    locks: &'a InstallLocks,
    key: String,
}

impl Drop for InstallLock<'_> {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.locks.busy.lock() {
            busy.remove(&self.key);
        }
    }
}

/// 运行环境后台任务的状态（列表与抽屉共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeJobState {
    /// 已创建，等待执行。
    Queued,
    /// 执行中（`step` 说明当前阶段）。
    Running,
    Done,
    Failed,
}

impl RuntimeJobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Running => "执行中",
            Self::Done => "已完成",
            Self::Failed => "失败",
        }
    }
}

/// 单个运行环境的异步任务：安装 / 扩展变更。
///
/// 这是任务中心作业在运行环境页上的**投影**：事实源是 `jobs` / `job_logs` 表，
/// 投影只为保持列表与抽屉既有轮询契约（`runtime_job` / `runtime_jobs_snapshot`）。
#[derive(Debug, Clone)]
pub struct RuntimeJob {
    pub runtime_id: String,
    /// install / extensions
    pub kind: String,
    /// 本次任务涉及的扩展 token（批量时多个）；安装/扩展任务用于 UI 高亮。
    pub ext_keys: Vec<String>,
    pub state: RuntimeJobState,
    /// 当前阶段（排队 / 拉镜像 / 创建容器 / 启动 / 安装扩展 / …）。
    pub step: String,
    pub progress: u8,
    pub error: String,
    pub logs: Vec<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

impl RuntimeJob {
    pub fn active(&self) -> bool {
        matches!(
            self.state,
            RuntimeJobState::Queued | RuntimeJobState::Running
        )
    }

    pub fn log_text(&self) -> String {
        self.logs.join("\n")
    }

    /// 从任务中心作业投影（`logs` 已按时间升序格式化）。
    fn from_job(job: &app_jobs::Job, logs: Vec<String>, now: i64) -> Self {
        let state = match job.status {
            app_jobs::JobStatus::Queued => RuntimeJobState::Queued,
            app_jobs::JobStatus::Running => RuntimeJobState::Running,
            app_jobs::JobStatus::Succeeded => RuntimeJobState::Done,
            app_jobs::JobStatus::Failed | app_jobs::JobStatus::Cancelled => RuntimeJobState::Failed,
        };
        Self {
            runtime_id: job.target_id.clone(),
            kind: if job.kind.contains(".ext.") {
                "extensions".into()
            } else {
                "install".into()
            },
            ext_keys: job.ext_keys(),
            state,
            step: job.phase_label.clone(),
            progress: job.progress.clamp(0, 100) as u8,
            error: job.error_detail.clone().unwrap_or_default(),
            logs,
            started_at: job.started_at.unwrap_or(job.created_at),
            updated_at: job.finished_at.unwrap_or(now),
        }
    }
}

impl InstallLocks {
    pub fn try_begin(&self, key: &str) -> Option<InstallLock<'_>> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.insert(key.to_string()) {
            return None;
        }
        Some(InstallLock {
            locks: self,
            key: key.to_string(),
        })
    }
}

/// 运行环境探测结果（列表页/环境页共用）。
#[derive(Debug, Clone)]
pub struct SiteEnvStatus {
    pub docker_available: bool,
    pub docker_reason: String,
    pub nginx_state: String,
    /// Web 服务容器实际镜像 id（列表「Web 服务」行展示；缺失时为空）。
    pub nginx_image_id: String,
    pub installed: bool,
    pub port: u16,
    pub port_message: String,
    pub nginx_image: String,
    pub php_image: String,
}

impl SiteEnvStatus {
    /// **仅 Web 服务（OpenResty）就绪**。PHP/Node/Go/Rust 是独立运行环境，
    /// 由建站时按所选实例单独校验，不做任何合并判断。
    pub fn ready(&self) -> bool {
        self.docker_available && self.installed && self.nginx_state == "running"
    }
}

#[derive(Debug, Clone)]
pub struct NewSite {
    pub domain: String,
    pub root_dir: Option<String>,
    /// runtime / static / proxy
    pub site_type: String,
    /// 仅 runtime：绑定的运行环境 id。
    pub runtime_id: Option<String>,
    /// 仅 proxy：`http(s)://host[:port][/path]`。
    pub proxy_target: String,
    /// 仅 proxy：是否升级 WebSocket。
    pub proxy_websocket: bool,
    /// 附加域名：只写 vhost `server_name` 与别名表，不改主域名/目录。
    pub aliases: Vec<String>,
}

/// 目录浏览条目（只读）。
#[derive(Debug, Clone)]
pub struct SiteDirEntry {
    pub name: String,
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
    /// UNIX 秒；格式化由展示层负责。
    pub modified: i64,
}

/// 站点根目录（或子目录）的只读列表。
#[derive(Debug, Clone)]
pub struct SiteDirListing {
    pub rel: String,
    pub parent: String,
    pub at_root: bool,
    pub entries: Vec<SiteDirEntry>,
}

#[derive(Debug, Clone)]
/// 重试任务的执行计划：在插入新行之前算好，避免留下没有执行器的 queued 僵尸。
enum RetryPlan {
    Install,
    Single {
        token: String,
        add: bool,
    },
    Apply(Vec<String>),
    Batch {
        desired: Vec<String>,
        targets: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum SiteError {
    BadRequest(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}

impl std::fmt::Display for SiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SiteError::BadRequest(m) => write!(f, "{m}"),
            SiteError::Conflict(m) => write!(f, "{m}"),
            SiteError::Unavailable(m) => write!(f, "{m}"),
            SiteError::Internal(m) => write!(f, "{m}"),
        }
    }
}

/// 删除运行环境的结果：`completed` 是已完成的清理步骤，`skipped` 是因环境限制
/// 无法执行、需要用户知晓的步骤（例如 Docker 不可用时未能确认容器）。
#[derive(Debug, Default, Clone)]
pub struct RuntimeDeleteReport {
    pub completed: Vec<String>,
    pub skipped: Vec<String>,
}

/// 删除中途失败的错误：把已完成步骤一并返回，避免「记录删了但容器/目录还在」。
fn partial_delete_error(completed: &[String], message: String) -> SiteError {
    let done = if completed.is_empty() {
        "无".to_string()
    } else {
        completed.join("；")
    };
    SiteError::Unavailable(format!(
        "{message}；已完成：{done}；运行环境记录已保留，可修复后重试"
    ))
}

// ---------------------------------------------------------------------------
// 域名 / 版本 / vhost：纯函数已抽到 `sites_vhost`，这里再导出保持旧调用路径
// ---------------------------------------------------------------------------

use crate::sites_vhost::{default_probe_html, default_probe_php};
pub use crate::sites_vhost::{
    normalize_site_aliases, render_default_vhost, render_php_pool, render_php_pool_with,
    render_site_vhost, site_root_for, validate_domain, validate_proxy_target, validate_site_type,
    VhostKind, VhostTls,
};

/// 一个可直接查看/编辑的 PHP 配置文件。
#[derive(Debug, Clone)]
pub struct PhpConfigFile {
    /// `php.ini` / `www.conf` / `conf.d/<name>.ini`。
    pub key: String,
    pub label: String,
    pub host_path: String,
    pub container_path: String,
    pub exists: bool,
    pub editable: bool,
    /// 与页面「PHP 参数」表单的权威关系说明。
    pub note: String,
    /// 保存时的容器校验警告（原文）。`PHP Warning` 不阻止保存，但要在界面回显。
    pub warnings: Vec<String>,
}

/// 配置文件正文（打开编辑器时返回）。
#[derive(Debug, Clone)]
pub struct PhpConfigFileView {
    pub file: PhpConfigFile,
    pub content: String,
}

/// `php -i` 实际生效值 vs `php.ini` 文件值的并排展示项。
#[derive(Debug, Clone)]
pub struct PhpIniValue {
    pub key: String,
    pub file_value: String,
    pub effective: String,
    pub source: String,
}

/// 解析 `php -i` 的 `key => local => master` 行，取 local 值（运行时实际值）。
fn parse_php_i(out: &str) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for line in out.lines() {
        let mut parts = line.split("=>");
        let key = parts.next().unwrap_or_default().trim();
        let local = parts.next().map(str::trim).unwrap_or_default();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'))
        {
            continue;
        }
        let value = if local.eq_ignore_ascii_case("no value") {
            parts.next().map(str::trim).unwrap_or_default()
        } else {
            local
        };
        values.push((key.to_string(), value.to_string()));
    }
    values
}

/// PHP ini 语法错误只打印到 stdout，退出码仍为 0（`php -c bad.ini -r ...` 会继续执行
/// `-r` 代码），必须显式识别 `syntax error`，否则坏配置会被当成合法保存。
fn php_output_has_syntax_error(out: &str) -> bool {
    out.to_ascii_lowercase().contains("syntax error")
}

/// 从容器校验输出里挑出警告原文（`PHP Warning` / `Warning:` 行），去重保序。
///
/// 警告不阻止保存（用户要求放宽口径），但必须让用户看到，避免“保存成功但启动时报警”。
fn php_validation_warnings(out: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut warnings = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() || !line.to_ascii_lowercase().contains("warning") {
            continue;
        }
        if seen.insert(line.to_string()) {
            warnings.push(line.to_string());
        }
    }
    warnings
}

/// 解析 ini 文件里的 `key = value` 赋值行（忽略注释与 section 头）。
fn parse_ini_assignments(text: &str) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('[') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                values.push((key.to_string(), value.trim().trim_matches('"').to_string()));
            }
        }
    }
    values
}

/// PHP 运行环境的专属配置（落库为 `site_runtimes.config_json`，不复制到别处）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpRuntimeConfig {
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: u64,
    pub pm: String,
    pub max_children: u32,
    pub start_servers: u32,
    pub min_spare_servers: u32,
    pub max_spare_servers: u32,
    pub max_requests: u32,
}

impl Default for PhpRuntimeConfig {
    fn default() -> Self {
        Self {
            memory_limit: "256M".into(),
            upload_max_filesize: "64M".into(),
            post_max_size: "64M".into(),
            max_execution_time: 30,
            pm: "dynamic".into(),
            max_children: 20,
            start_servers: 3,
            min_spare_servers: 2,
            max_spare_servers: 8,
            max_requests: 500,
        }
    }
}

impl PhpRuntimeConfig {
    /// 从 `config_json` 容错读取：缺字段用默认值，坏字段在 `validate` 里报错。
    pub fn from_json(raw: &str) -> Self {
        let mut cfg = Self::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            return cfg;
        };
        if let Some(s) = v.get("memory_limit").and_then(|v| v.as_str()) {
            cfg.memory_limit = s.into();
        }
        if let Some(s) = v.get("upload_max_filesize").and_then(|v| v.as_str()) {
            cfg.upload_max_filesize = s.into();
        }
        if let Some(s) = v.get("post_max_size").and_then(|v| v.as_str()) {
            cfg.post_max_size = s.into();
        }
        if let Some(n) = v.get("max_execution_time").and_then(|v| v.as_u64()) {
            cfg.max_execution_time = n;
        }
        if let Some(s) = v.get("pm").and_then(|v| v.as_str()) {
            cfg.pm = s.into();
        }
        if let Some(n) = v.get("max_children").and_then(|v| v.as_u64()) {
            cfg.max_children = n as u32;
        }
        if let Some(n) = v.get("start_servers").and_then(|v| v.as_u64()) {
            cfg.start_servers = n as u32;
        }
        if let Some(n) = v.get("min_spare_servers").and_then(|v| v.as_u64()) {
            cfg.min_spare_servers = n as u32;
        }
        if let Some(n) = v.get("max_spare_servers").and_then(|v| v.as_u64()) {
            cfg.max_spare_servers = n as u32;
        }
        if let Some(n) = v.get("max_requests").and_then(|v| v.as_u64()) {
            cfg.max_requests = n as u32;
        }
        cfg
    }

    /// 原样序列化；只写已通过 `validate` 的配置。
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "memory_limit": self.memory_limit,
            "upload_max_filesize": self.upload_max_filesize,
            "post_max_size": self.post_max_size,
            "max_execution_time": self.max_execution_time,
            "pm": self.pm,
            "max_children": self.max_children,
            "start_servers": self.start_servers,
            "min_spare_servers": self.min_spare_servers,
            "max_spare_servers": self.max_spare_servers,
            "max_requests": self.max_requests,
        })
        .to_string()
    }

    /// 数值/尺寸范围校验；任何一项不合法都拒绝落盘，避免 FPM 起不来。
    pub fn validate(&self) -> Result<(), SiteError> {
        let mem = parse_size_mb(&self.memory_limit)?;
        let upload = parse_size_mb(&self.upload_max_filesize)?;
        let post = parse_size_mb(&self.post_max_size)?;
        if !(32..=2048).contains(&mem) {
            return Err(SiteError::BadRequest(
                "memory_limit 必须在 32M-2048M 之间".into(),
            ));
        }
        if !(1..=1024).contains(&upload) {
            return Err(SiteError::BadRequest(
                "upload_max_filesize 必须在 1M-1024M 之间".into(),
            ));
        }
        if post < upload {
            return Err(SiteError::BadRequest(
                "post_max_size 不能小于 upload_max_filesize".into(),
            ));
        }
        if post > 2048 {
            return Err(SiteError::BadRequest("post_max_size 不能超过 2048M".into()));
        }
        if !(10..=600).contains(&self.max_execution_time) {
            return Err(SiteError::BadRequest(
                "max_execution_time 必须在 10-600 秒之间".into(),
            ));
        }
        if !matches!(self.pm.as_str(), "dynamic" | "ondemand") {
            return Err(SiteError::BadRequest(
                "FPM 进程模式只支持 dynamic / ondemand".into(),
            ));
        }
        if !(2..=100).contains(&self.max_children) {
            return Err(SiteError::BadRequest(
                "FPM 最大子进程数必须在 2-100 之间".into(),
            ));
        }
        if !(1..=self.max_children).contains(&self.start_servers) {
            return Err(SiteError::BadRequest(
                "FPM 启动进程数必须介于 1 和最大子进程数之间".into(),
            ));
        }
        if !(1..=self.start_servers).contains(&self.min_spare_servers) {
            return Err(SiteError::BadRequest(
                "FPM 最小空闲进程必须介于 1 和启动进程数之间".into(),
            ));
        }
        if !(self.min_spare_servers..=self.max_children).contains(&self.max_spare_servers) {
            return Err(SiteError::BadRequest(
                "FPM 最大空闲进程必须介于最小空闲进程和最大子进程数之间".into(),
            ));
        }
        if !(100..=10000).contains(&self.max_requests) {
            return Err(SiteError::BadRequest(
                "FPM max_requests 必须在 100-10000 之间".into(),
            ));
        }
        Ok(())
    }
}

/// Node.js 运行环境的专属配置（落库为 `site_runtimes.config_json`）。
///
/// Node 与 PHP 的差异都收敛在这里：不走 FastCGI，而是「启动命令 + 监听端口 + 反代」；
/// 端口由环境分配（`site_runtimes.port`），这里只描述进程怎么起、怎么探活、带哪些环境变量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRuntimeConfig {
    /// 容器内工作目录下的启动命令（argv 语义，按空白切分，不经宿主 shell）。
    pub command: String,
    /// HTTP 健康检查路径，默认 `/`；安装/重启后按它轮询。
    pub health_path: String,
    /// `production` / `development` / `test`，注入 `NODE_ENV`。
    pub node_env: String,
    /// 追加环境变量（`KEY=VALUE`），与 `PORT` / `NODE_ENV` / `HOST` 一起注入。
    pub env: Vec<(String, String)>,
}

impl Default for NodeRuntimeConfig {
    fn default() -> Self {
        Self {
            command: "npm start".into(),
            health_path: "/".into(),
            node_env: "production".into(),
            env: Vec::new(),
        }
    }
}

impl NodeRuntimeConfig {
    /// 从 `config_json` 容错读取：缺字段用默认值，坏字段在 `validate` 里报错。
    pub fn from_json(raw: &str) -> Self {
        let mut cfg = Self::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            return cfg;
        };
        if let Some(s) = v.get("command").and_then(|v| v.as_str()) {
            cfg.command = s.into();
        }
        if let Some(s) = v.get("health_path").and_then(|v| v.as_str()) {
            cfg.health_path = s.into();
        }
        if let Some(s) = v.get("node_env").and_then(|v| v.as_str()) {
            cfg.node_env = s.into();
        }
        if let Some(map) = v.get("env").and_then(|v| v.as_object()) {
            cfg.env = map
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            cfg.env.sort_by(|a, b| a.0.cmp(&b.0));
        }
        cfg
    }

    pub fn to_json(&self) -> String {
        let env: std::collections::BTreeMap<&str, &str> = self
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        serde_json::json!({
            "command": self.command,
            "health_path": self.health_path,
            "node_env": self.node_env,
            "env": env,
        })
        .to_string()
    }

    /// 启动命令切分为 argv。未通过校验时返回错误，不允许把任意 shell 串塞进容器。
    pub fn argv(&self) -> Result<Vec<String>, SiteError> {
        let argv: Vec<String> = self
            .command
            .split_whitespace()
            .map(ToString::to_string)
            .collect();
        if argv.is_empty() {
            return Err(SiteError::BadRequest("启动命令不能为空".into()));
        }
        Ok(argv)
    }

    /// 页面 textarea ↔ 结构化 env 的互转：每行 `KEY=VALUE`，`#` 开头为注释。
    pub fn env_text(&self) -> String {
        self.env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set_env_text(&mut self, text: &str) -> Result<(), SiteError> {
        let mut env = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| SiteError::BadRequest(format!("环境变量需为 KEY=VALUE：{line}")))?;
            let key = key.trim();
            let value = value.trim();
            if env.iter().any(|(k, _): &(String, String)| k == key) {
                return Err(SiteError::BadRequest(format!("环境变量重复：{key}")));
            }
            env.push((key.to_string(), value.to_string()));
        }
        env.sort_by(|a, b| a.0.cmp(&b.0));
        self.env = env;
        Ok(())
    }

    /// 命令/路径/环境变量的边界校验；任何一项失败都拒绝落盘。
    pub fn validate(&self) -> Result<(), SiteError> {
        if self.command.len() > 512 {
            return Err(SiteError::BadRequest("启动命令不能超过 512 字符".into()));
        }
        let argv = self.argv()?;
        if argv.len() > 32 {
            return Err(SiteError::BadRequest("启动命令参数不能超过 32 个".into()));
        }
        for token in &argv {
            if token.is_empty()
                || token.len() > 128
                || !token.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '_' | '-' | '/' | ':' | '@' | '=' | '+')
                })
            {
                return Err(SiteError::BadRequest(format!(
                    "启动命令包含不支持的字符：{token}"
                )));
            }
        }
        let path = self.health_path.trim();
        if !path.starts_with('/') || path.len() > 128 {
            return Err(SiteError::BadRequest(
                "健康检查路径必须以 / 开头且不超过 128 字符".into(),
            ));
        }
        if !path.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '?' | '&' | '=' | '%')
        }) {
            return Err(SiteError::BadRequest(format!(
                "健康检查路径包含不支持的字符：{path}"
            )));
        }
        if !matches!(
            self.node_env.as_str(),
            "production" | "development" | "test"
        ) {
            return Err(SiteError::BadRequest(
                "NODE_ENV 只支持 production / development / test".into(),
            ));
        }
        if self.env.len() > 32 {
            return Err(SiteError::BadRequest("环境变量不能超过 32 项".into()));
        }
        for (key, value) in &self.env {
            if key.is_empty()
                || key.len() > 64
                || !key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                || key.starts_with(|c: char| c.is_ascii_digit())
            {
                return Err(SiteError::BadRequest(format!(
                    "环境变量名不合法：{key}（只允许大写字母、数字、下划线，且不以数字开头）"
                )));
            }
            if matches!(key.as_str(), "PORT" | "NODE_ENV" | "HOST") {
                return Err(SiteError::BadRequest(format!(
                    "{key} 由内核注入，不能重复配置"
                )));
            }
            if value.len() > 1024 || value.contains('\n') || value.contains('\r') {
                return Err(SiteError::BadRequest(format!(
                    "环境变量 {key} 的值不能包含换行且不超过 1024 字符"
                )));
            }
        }
        Ok(())
    }
}

/// `64M` / `1G` / `1024K` → MiB（向下取整，最小 1）。
fn parse_size_mb(raw: &str) -> Result<u64, SiteError> {
    let s = raw.trim().to_ascii_uppercase();
    let (num, unit) = match s.chars().last() {
        Some('K') | Some('M') | Some('G') => (s[..s.len() - 1].to_string(), s.chars().last()),
        _ => (s.clone(), None),
    };
    let n: u64 = num
        .parse()
        .map_err(|_| SiteError::BadRequest(format!("无法解析尺寸：{raw}")))?;
    let mb = match unit {
        Some('K') => (n / 1024).max(1),
        Some('G') => n.saturating_mul(1024),
        _ => n,
    };
    Ok(mb)
}

/// 镜像版本缓存查询结果。
pub(crate) enum ImageVersionHit {
    /// 正缓存命中：image_id 不可变，值不需要 TTL。
    Known(String),
    /// 正缓存的「读不到」或负缓存仍在有效期内：本轮不再 inspect。
    Suppressed,
    /// 未命中或负缓存过期：需要 inspect。
    Miss,
}

/// 进程内镜像版本缓存查询（纯函数，便于单测）。
pub(crate) fn image_version_lookup(
    cache: &HashMap<String, (Option<String>, std::time::Instant)>,
    image_id: &str,
    now: std::time::Instant,
) -> ImageVersionHit {
    const NEGATIVE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
    match cache.get(image_id) {
        Some((Some(v), _)) => ImageVersionHit::Known(v.clone()),
        Some((None, at)) if now.duration_since(*at) < NEGATIVE_TTL => ImageVersionHit::Suppressed,
        _ => ImageVersionHit::Miss,
    }
}

/// 站点 vhost 保存结果：`reloaded` 表示确实对运行中的 nginx 发起了重载。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VhostSaveOutcome {
    pub reloaded: bool,
}

/// 隔离校验用的临时主配置：只 include 候选 vhost，不加载线上站点 include。
///
/// 站点停止时配置是 `.conf.disabled`，不在线上 include 内；直接用 `nginx -t`
/// 只能校验现值、无法校验候选内容。这里用独立 main 显式 include 候选文件，
/// 校验通过才替换线上文件，避免「先写进 include 再测」把全局打挂。
fn vhost_check_main(candidate_container: &str) -> String {
    format!(
        "worker_processes 1;\n\
         error_log /tmp/skiff-vhost-check-error.log;\n\
         pid /tmp/skiff-vhost-check.pid;\n\
         events {{ worker_connections 64; }}\n\
         http {{\n\
         \x20   include /usr/local/openresty/nginx/conf/mime.types;\n\
         \x20   include {candidate_container};\n\
         }}\n"
    )
}

/// 启动运行环境站点前的就绪检查：环境必须处于 running；PHP 还要求 FPM socket 已就绪。
///
/// Rust 等「独立进程 + 固定端口」的运行环境没有 socket，不能套用 PHP 的条件，
/// 否则运行正常的 Rust 环境站点会被误判为「运行环境未运行」而拒绝启动。
fn ensure_runtime_ready_for_start(rt: &Runtime) -> Result<(), SiteError> {
    if rt.last_status != "running" {
        return Err(SiteError::Unavailable(format!(
            "运行环境 {} 未运行，请先启动运行环境",
            rt.version
        )));
    }
    if rt.kind == skiff_store::KIND_PHP {
        let socket = rt
            .socket_path
            .as_deref()
            .ok_or_else(|| SiteError::Unavailable("运行环境缺少 socket 路径，请重新安装".into()))?;
        if !Path::new(socket).exists() {
            return Err(SiteError::Unavailable(format!(
                "PHP socket 未就绪：{socket}，请先启动运行环境"
            )));
        }
    }
    Ok(())
}

impl AppState {
    /// 运行环境列表：库是事实源，容器状态只作实时探测（漂移徽章的依据）。
    pub async fn runtime_views(&self) -> Result<Vec<RuntimeView>, SiteError> {
        let runtimes = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        let docker_available = self.docker().available();
        let mut docker_known = docker_available;
        let rows = if docker_available {
            match self.docker().list_containers().await {
                Ok(rows) => rows,
                Err(_) => {
                    // Docker 可以连上但列表失败：状态未知，不能当成「容器不存在」。
                    docker_known = false;
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let jobs = self.runtime_jobs_snapshot().await;
        let s = self.site_settings();
        let mut out = Vec::new();
        // image_id → 待 inspect 的视图下标（同一镜像多实例只 inspect 一次）。
        let mut pending_images: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for rt in runtimes {
            let implemented = runtime_catalog::is_implemented(&rt.kind);
            let container_name = if implemented {
                runtime_container_name(&rt.kind, &rt.name)
            } else {
                format!("skiff-runtime-{}", rt.id)
            };
            let container_row = rows.iter().find(|c| c.name == container_name);
            let container_exists = container_row.is_some();
            let image_id = container_row
                .map(|c| c.image_id.clone())
                .unwrap_or_default();
            let container_image = container_row.map(|c| c.image.clone()).unwrap_or_default();
            // 容器可能登记为 `image@digest`；比较时只看仓库:tag 部分，避免误报漂移。
            let container_ref_base = container_image.split('@').next().unwrap_or("");
            let image_mismatch =
                container_exists && !container_image.is_empty() && container_ref_base != rt.image;
            let container_state =
                probe_container_state(docker_known, container_row.map(|c| c.state.as_str()));
            let job = jobs.iter().find(|j| j.runtime_id == rt.id).cloned();
            let job_active = job.as_ref().is_some_and(RuntimeJob::active);
            let job_failed = job
                .as_ref()
                .is_some_and(|j| j.state == RuntimeJobState::Failed);
            let effective_state = if !implemented {
                "unimplemented".to_string()
            } else if job_active {
                "installing".to_string()
            } else if job_failed && container_state != "running" {
                // 安装失败且容器没起来：列表状态要能看出失败，而不是笼统的「未安装」。
                "error".to_string()
            } else {
                runtime_effective_state(&container_state).to_string()
            };
            let socket_host = rt.socket_path.clone().unwrap_or_default();
            let socket_container = if socket_host.is_empty() {
                String::new()
            } else {
                s.container_path(Path::new(&socket_host))
            };
            let socket_exists = !socket_host.is_empty() && Path::new(&socket_host).exists();
            let socket_mode = socket_mode(&socket_host);
            let site_count = self
                .db()
                .runtime_site_count(&rt.id)
                .map_err(|e| SiteError::Internal(e.to_string()))?;
            let drift = implemented
                && docker_known
                && !job_active
                && rt.desired_state == "running"
                && effective_state != "running"
                && effective_state != "missing"
                && effective_state != "unknown";
            // 写回缓存状态；'unimplemented' 不是合法枚举值，跳过；任务执行中由任务收尾写入。
            // Docker 不可达时状态未知，不能把 unknown 写回成 stopped，避免污染下次判断。
            if implemented && !job_active && docker_known {
                let cached = if matches!(
                    effective_state.as_str(),
                    "running" | "stopped" | "error" | "installing"
                ) {
                    effective_state.clone()
                } else {
                    "stopped".into()
                };
                if let Err(e) = self
                    .db()
                    .set_runtime_status(&rt.id, &rt.desired_state, &cached)
                {
                    tracing::warn!(runtime = %rt.id, error = %e, "回写运行环境状态失败");
                }
            }
            // 真实版本来自镜像元数据（image_id 不可变），与容器是否运行无关：
            // 停止但容器仍在时照样显示真实版本。全程不进容器执行进程。
            let actual_version =
                if !image_id.is_empty() && runtime_catalog::version_env_key(&rt.kind).is_some() {
                    let cache = self.image_versions.lock().await;
                    match image_version_lookup(&cache, &image_id, std::time::Instant::now()) {
                        ImageVersionHit::Known(v) => Some(v),
                        ImageVersionHit::Suppressed => None,
                        ImageVersionHit::Miss => {
                            pending_images
                                .entry(image_id.clone())
                                .or_default()
                                .push(out.len());
                            None
                        }
                    }
                } else {
                    None
                };
            let extensions = runtime_catalog::split_extensions(&rt.extensions);
            let image_digest = rt.image_digest.clone();
            out.push(RuntimeView {
                runtime: rt,
                implemented,
                effective_state,
                container_name,
                container_state,
                drift,
                site_count,
                socket_host,
                socket_container,
                socket_exists,
                socket_mode,
                actual_version,
                image_id,
                image_mismatch,
                image_digest,
                container_exists,
                extensions,
                job,
            });
        }

        // 未命中时每个 image_id 只 inspect 一次；失败写入 60 秒负缓存，不阻塞后续渲染。
        for (image_id, indices) in pending_images {
            let version = self
                .docker()
                .inspect_image(&image_id)
                .await
                .ok()
                .and_then(|info| {
                    let kind = indices
                        .first()
                        .and_then(|i| out.get(*i))
                        .map(|v| v.runtime.kind.as_str())
                        .unwrap_or_default();
                    let key = runtime_catalog::version_env_key(kind)?;
                    info.env_value(key)
                });
            self.image_versions.lock().await.insert(
                image_id.clone(),
                (version.clone(), std::time::Instant::now()),
            );
            for idx in indices {
                if let Some(view) = out.get_mut(idx) {
                    view.actual_version = version.clone();
                }
            }
        }
        Ok(out)
    }

    pub async fn runtime_view(&self, id: &str) -> Result<RuntimeView, SiteError> {
        self.runtime_views()
            .await?
            .into_iter()
            .find(|v| v.runtime.id == id)
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))
    }

    /// 独立 Web 服务（OpenResty）操作。与任何 PHP/Node/Go/Rust 行完全解耦：
    /// 安装只拉 nginx 镜像、只建 nginx 容器，不触碰语言运行环境。
    pub async fn web_env_action(self: &Arc<Self>, action: &str) -> Result<(), SiteError> {
        match action {
            "install" | "rebuild" => {
                self.start_web_env_install();
                Ok(())
            }
            "start" | "stop" | "restart" => {
                let verb = match action {
                    "start" => Verb::Start,
                    "stop" => Verb::Stop,
                    _ => Verb::Restart,
                };
                self.docker_runtime_action(NGINX_CONTAINER, verb).await
            }
            "delete" => {
                let rows = self
                    .docker()
                    .list_containers()
                    .await
                    .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
                let Some(c) = rows.iter().find(|c| c.name == NGINX_CONTAINER) else {
                    return Ok(());
                };
                self.remove_existing_container(c)
                    .await
                    .map_err(SiteError::Internal)
            }
            _ => Err(SiteError::BadRequest(format!(
                "不支持的 Web 服务操作：{action}"
            ))),
        }
    }

    /// 内核自身容器操作的唯一入口：先经 `policy.check(Actor::System, …)`，
    /// 再交给 `Docker::execute` 单点分发。
    ///
    /// 站点与运行环境的每一处容器动作都必须走这里：`Actor::System` 只放行内核
    /// 托管的对象（`io.skiff.project=sites`），外部面板对象仍被拒。用户角色鉴权
    /// 由调用方 handler 负责，这里只做对象面收口。
    async fn kernel_execute(
        &self,
        verb: Verb,
        target: Option<&skiff_docker::ObjectRef>,
        params: DockerParams,
    ) -> Result<serde_json::Value, String> {
        self.cfg
            .policy
            .check(&skiff_model::Actor::System, verb, target)
            .map_err(|e| e.to_string())?;
        self.docker()
            .execute(verb, target, params)
            .await
            .map_err(|e| e.to_string())
    }

    async fn docker_runtime_action(&self, name: &str, verb: Verb) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows.iter().find(|c| c.name == name).ok_or_else(|| {
            SiteError::Unavailable(format!("容器 {name} 未安装，请先安装运行环境"))
        })?;
        let obj = skiff_docker::ObjectRef {
            id: c.id.clone(),
            name: c.name.clone(),
            labels: c.labels.clone(),
        };
        self.kernel_execute(verb, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| SiteError::Unavailable(format!("容器 {name} 操作失败：{e}")))
    }

    /// 运行环境启停/重建/同步。Go 未接入，明确返回「尚未接入」。
    ///
    /// `install` / `rebuild` 只登记后台任务并立即返回；页面轮询任务状态与日志。
    pub async fn runtime_action(
        self: &Arc<Self>,
        id: &str,
        action: &str,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境尚未接入，不提供安装/启停/重建",
                rt.kind
            )));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        match action {
            "install" | "rebuild" => {
                // 重建与安装同一条异步链路：默认读取库里的扩展集并重放。
                self.start_runtime_install(id, actor, role).await?;
                return Ok(());
            }
            "start" => {
                self.docker_runtime_action(&container, Verb::Start).await?;
                if let Err(e) = self.node_health_gate(&rt).await {
                    let _ = self.db().set_runtime_status(id, "running", "error");
                    return Err(e);
                }
                self.db()
                    .set_runtime_status(id, "running", "running")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "stop" => {
                // 停止不换版本、不改绑定：绑定站点继续指向本版本 socket，访问返回
                // 502/503；站点页会给出「前往运行环境页」的入口。
                self.docker_runtime_action(&container, Verb::Stop).await?;
                self.db()
                    .set_runtime_status(id, "stopped", "stopped")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "restart" => {
                self.docker_runtime_action(&container, Verb::Restart)
                    .await?;
                if let Err(e) = self.node_health_gate(&rt).await {
                    let _ = self.db().set_runtime_status(id, "running", "error");
                    return Err(e);
                }
                self.db()
                    .set_runtime_status(id, "running", "running")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "sync" => {
                let current = self.runtime_view(id).await?;
                // 以容器实测为准，纠正「期望运行但实际停了」的漂移缓存。
                if current.effective_state == "running" {
                    if let Err(e) = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "running")
                    {
                        tracing::warn!(runtime = id, error = %e, "回写运行环境状态失败");
                    }
                } else if current.effective_state == "stopped" {
                    if let Err(e) = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "stopped")
                    {
                        tracing::warn!(runtime = id, error = %e, "回写运行环境状态失败");
                    }
                }
            }
            other => return Err(SiteError::BadRequest(format!("未知运行环境操作：{other}"))),
        }
        Ok(())
    }

    /// Node 进程启动后的健康闸门：PHP / Rust 不需要（PHP 有 socket 探活，Rust 安装期
    /// 已等端口），只有 Node 需要在「手动启动/重启」时确认应用真的能响应。
    async fn node_health_gate(&self, rt: &Runtime) -> Result<(), SiteError> {
        if rt.kind != skiff_store::KIND_NODE {
            return Ok(());
        }
        let port = rt
            .port
            .ok_or_else(|| SiteError::Unavailable("Node.js 环境缺少端口，请重建".into()))?;
        let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
        if wait_http_ready(port, &cfg.health_path, std::time::Duration::from_secs(60)).await {
            Ok(())
        } else {
            Err(SiteError::Unavailable(format!(
                "Node.js 进程未通过健康检查（{}），请查看运行环境日志",
                cfg.health_path
            )))
        }
    }

    /// 最近一次任务快照（列表/抽屉轮询）。
    pub async fn runtime_job(&self, id: &str) -> Option<RuntimeJob> {
        let job = self
            .jobs()
            .latest_for_target(app_jobs::TARGET_RUNTIME, id)?;
        Some(self.runtime_job_view(&job, 300))
    }

    /// 全部运行环境任务（只含有任务记录的环境）。
    pub async fn runtime_jobs_snapshot(&self) -> Vec<RuntimeJob> {
        self.jobs()
            .list_for_target_type(app_jobs::TARGET_RUNTIME, 200)
            .iter()
            .map(|j| self.runtime_job_view(j, 60))
            .collect()
    }

    /// 作业 → 运行环境页投影；日志按时间升序带上 `[HH:MM:SS]` 前缀。
    fn runtime_job_view(&self, job: &app_jobs::Job, log_limit: i64) -> RuntimeJob {
        let logs = self
            .db()
            .job_log_tail(&job.id, log_limit)
            .unwrap_or_default()
            .into_iter()
            .map(|e| format!("[{}] {}", fmt_log_time(e.at), e.line))
            .collect();
        RuntimeJob::from_job(job, logs, chrono::Local::now().timestamp())
    }

    /// 容器是否被进行中的内核任务占用（Docker 页删除前的 BusyGuard）。
    ///
    /// 与 `delete_site_runtime` 同一口径：运行环境有安装/扩展任务、Web 服务
    /// 正在安装或重建时，都不允许从 Docker 页直接删除对应容器。
    pub async fn container_busy_reason(&self, container: &str) -> Option<String> {
        if container == NGINX_CONTAINER && self.site_install_progress().await.running {
            return Some("Web 服务正在安装或重建中，暂不能删除容器".into());
        }
        let runtimes = self.db().list_runtimes().unwrap_or_default();
        for rt in runtimes {
            if runtime_container_name(&rt.kind, &rt.name) != container {
                continue;
            }
            if self
                .runtime_job(&rt.id)
                .await
                .is_some_and(|job| job.active())
            {
                return Some(format!(
                    "运行环境 {} 有进行中的任务，暂不能删除容器",
                    rt.name
                ));
            }
        }
        None
    }

    /// 阶段推进。参数是**任务中心作业 id**（不是 runtime id），进度只在真实步骤落库。
    async fn job_advance(&self, job_id: &str, step: &str, progress: u8, line: &str) {
        self.job_advance_with(job_id, step, progress, line, true)
            .await;
    }

    /// 不可中断步骤（`docker exec` 安装脚本）推进：期间取消按钮应隐藏。
    async fn job_advance_blocking(&self, job_id: &str, step: &str, progress: u8, line: &str) {
        self.job_advance_with(job_id, step, progress, line, false)
            .await;
    }

    async fn job_advance_with(
        &self,
        job_id: &str,
        step: &str,
        progress: u8,
        line: &str,
        cancellable: bool,
    ) {
        self.jobs()
            .set_phase(job_id, step, step, progress as i64, cancellable);
        self.jobs().note(job_id, line);
    }

    async fn job_note(&self, job_id: &str, line: &str) {
        self.jobs().note(job_id, line);
    }

    async fn job_finish_ok(&self, job_id: &str, line: &str) {
        self.jobs().finish_ok(job_id, line, serde_json::json!({}));
    }

    async fn job_finish_err(&self, job_id: &str, error: &str) {
        match error.strip_prefix(TIMEOUT_MARK) {
            Some(msg) => self.job_finish_err_class(job_id, "timeout", msg).await,
            None => self.job_finish_err_class(job_id, "failed", error).await,
        }
    }

    async fn job_finish_err_class(&self, job_id: &str, class: &str, error: &str) {
        self.jobs().finish_err(job_id, class, error);
    }

    /// 执行体在阶段边界检查取消标记。
    async fn job_cancelled(&self, job_id: &str) -> bool {
        self.jobs().cancel_requested(job_id)
    }

    /// 把命令输出按行写入任务日志（只保留尾部，防止一次 pull 撑爆轮询响应）。
    async fn job_log_output(&self, job_id: &str, output: &str) {
        let mut lines: Vec<String> = output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| format!("  {l}"))
            .collect();
        let keep = 30;
        if lines.len() > keep {
            lines.drain(..lines.len() - keep);
        }
        self.jobs().log_lines(job_id, "out", &lines);
    }

    /// 启动一次运行环境安装（异步）。只登记作业，立即返回。
    pub async fn start_runtime_install(
        self: &Arc<Self>,
        id: &str,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境尚未接入，不提供安装",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!("安装运行环境 {}", rt.name),
                params: serde_json::json!({
                    "runtime_id": id,
                    "kind": rt.kind,
                    "name": rt.name,
                    "version": rt.version,
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        if let Err(e) = self.db().set_runtime_status(id, "running", "installing") {
            tracing::warn!(runtime = id, error = %e, "写入运行环境安装中状态失败");
        }
        let state = self.clone();
        let rid = id.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state.run_runtime_install_job(&rid, &job_id).await;
        });
        Ok(())
    }

    /// 修改进程型环境的固定端口：校验区间/占用 → 落库 → 重写绑定站点 vhost →
    /// 重建容器。端口只属于环境，站点侧不持有，避免出现第二处端口来源。
    pub async fn change_runtime_port(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let used: Vec<u16> = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .into_iter()
            .filter(|r| r.id != rt.id)
            .filter_map(|r| r.port)
            .collect();
        validate_process_port(&rt.kind, port, &used).map_err(SiteError::BadRequest)?;
        if rt.port == Some(port) {
            return Ok(());
        }
        // 已有任务时不能改端口：库与 vhost 先变、容器却没重建会留下半成品状态。
        if self.runtime_job(id).await.is_some_and(|j| j.active()) {
            return Err(SiteError::Conflict(
                "该运行环境有进行中的任务，请等待任务结束后再改端口".into(),
            ));
        }
        self.db()
            .set_runtime_port(id, port)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // vhost 里写死了旧端口，必须一起重写，否则站点反代到一个不再监听的端点。
        // 端口已在库中更新，这里只告警不中断：nginx 未启动或重载失败时仍要让容器按新端口重建，
        // 否则会出现「库/容器新端口、nginx 旧端口」的半成品状态。
        let s = self.site_settings();
        let domains = self
            .db()
            .runtime_site_domains(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        for domain in &domains {
            if let Err(e) = self.write_site_vhost(&s, domain) {
                tracing::warn!(runtime = id, site = %domain, error = %e, "改端口后重写站点 vhost 失败");
            }
        }
        if !domains.is_empty() {
            if let Err(e) = self.validate_nginx().await {
                tracing::warn!(runtime = id, error = %e, "改端口后 nginx 配置校验失败");
            }
            if let Err(e) = self.restart_nginx().await {
                tracing::warn!(runtime = id, error = %e, "改端口后重载 nginx 失败");
            }
        }
        // 容器 Env 里的 PORT 来自环境行，必须重建才能换端口。
        self.start_runtime_install(id, actor, role).await?;
        Ok(())
    }

    /// 克隆进程型环境：同版本/镜像、独立代码目录、自动分配新端口，用于并跑第二个应用。
    ///
    /// PHP 是共享 FPM 池，克隆没有「第二个进程」语义，暂不支持。
    pub async fn clone_runtime(
        self: &Arc<Self>,
        id: &str,
        raw_name: &str,
        actor: &str,
        role: &str,
    ) -> Result<Runtime, SiteError> {
        let src = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !matches!(
            src.kind.as_str(),
            skiff_store::KIND_NODE | skiff_store::KIND_RUST
        ) {
            return Err(SiteError::BadRequest(format!(
                "{} 运行环境暂不支持克隆",
                runtime_catalog::kind_label(&src.kind)
            )));
        }
        let s = self.site_settings();
        let name = {
            let raw = raw_name.trim();
            if raw.is_empty() {
                format!("{}-copy", src.name)
            } else {
                raw.to_string()
            }
        };
        skiff_store::validate_runtime_name(&name)
            .map_err(|e| SiteError::BadRequest(format!("环境名不合法：{e}")))?;
        let dir = s.runtime_home(&src.kind, &name);
        if dir.exists() {
            return Err(SiteError::Conflict(format!(
                "目标目录已存在：{}，请换一个名字",
                dir.display()
            )));
        }
        if self
            .db()
            .runtime_by_name(&src.kind, &name)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(SiteError::Conflict(format!(
                "{} 环境已存在：{name}",
                runtime_catalog::kind_label(&src.kind)
            )));
        }
        let used: Vec<u16> = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .into_iter()
            .filter(|r| r.kind == src.kind)
            .filter_map(|r| r.port)
            .collect();
        let port = runtime_catalog::allocate_port(&src.kind, &used).ok_or_else(|| {
            SiteError::Conflict(format!(
                "{} 端口区间已用满，请先删除不再使用的环境",
                runtime_catalog::kind_label(&src.kind)
            ))
        })?;
        let image = runtime_catalog::image_for(&src.kind, &src.version, &s.php_image);
        let cloned = self
            .db()
            .create_runtime(&NewRuntime {
                id: format!("{}-{}", src.kind, name),
                kind: src.kind.clone(),
                name,
                version: src.version.clone(),
                image,
                image_digest: String::new(),
                socket_path: None,
                port: Some(port),
                is_default: false,
            })
            .map_err(|e| SiteError::Conflict(format!("创建运行环境失败：{e}")))?;
        if let Err(e) = self.db().set_runtime_config(&cloned.id, &src.config_json) {
            tracing::warn!(runtime = %cloned.id, error = %e, "克隆运行环境配置失败");
        }
        if let Err(e) = copy_runtime_app(
            &s.runtime_home(&src.kind, &src.name),
            &s.runtime_home(&cloned.kind, &cloned.name),
        ) {
            tracing::warn!(runtime = %cloned.id, error = %e, "克隆代码目录失败，将使用自带脚手架");
        }
        self.start_runtime_install(&cloned.id, actor, role).await?;
        Ok(cloned)
    }

    /// 按需探测「应用就绪」：容器 running 只说明进程活着，不代表端口/HTTP 已能服务。
    pub async fn probe_runtime_health(&self, id: &str) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !matches!(
            rt.kind.as_str(),
            skiff_store::KIND_NODE | skiff_store::KIND_RUST
        ) {
            return Err(SiteError::BadRequest(format!(
                "{} 运行环境没有独立应用进程",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        let port = rt.port.ok_or_else(|| {
            SiteError::Unavailable("运行环境缺少固定端口，请重新安装".to_string())
        })?;
        let container = runtime_container_name(&rt.kind, &rt.name);
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("Docker 不可达：{e}")))?;
        match rows.iter().find(|c| c.name == container) {
            None => {
                return Err(SiteError::Unavailable(
                    "容器不存在，请先安装运行环境".into(),
                ))
            }
            Some(c) if c.state != "running" => {
                return Err(SiteError::Unavailable(format!(
                    "容器未运行（{}），应用不可用",
                    c.state
                )))
            }
            Some(_) => {}
        }
        if rt.kind == skiff_store::KIND_NODE {
            let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
            if http_probe_ok(port, &cfg.health_path).await {
                Ok(format!("应用就绪：HTTP {} 返回 2xx/3xx", cfg.health_path))
            } else {
                Err(SiteError::Unavailable(format!(
                    "容器在运行，但应用未就绪：HTTP {} 无 2xx/3xx 响应（端口 {port}）",
                    cfg.health_path
                )))
            }
        } else if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            Ok(format!("应用就绪：端口 {port} 已监听"))
        } else {
            Err(SiteError::Unavailable(format!(
                "容器在运行，但端口 {port} 未监听"
            )))
        }
    }

    async fn run_runtime_install_job(&self, runtime_id: &str, job_id: &str) {
        let result = self.install_runtime_task(runtime_id, job_id).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，运行环境可能处于中间状态，可重试安装");
                } else {
                    self.job_finish_ok(job_id, "安装完成，运行环境已就绪").await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "写入运行环境就绪状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，运行环境可能处于中间状态，可重试安装");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                // 记录失败但保留「期望运行」，这样重试入口与漂移提示仍然成立。
                if let Err(e) = self.db().set_runtime_status(runtime_id, "running", "error") {
                    tracing::warn!(runtime = runtime_id, error = %e, "写入运行环境失败状态失败");
                }
            }
        }
    }

    /// 安装主体：目录 → FPM 池 → 拉镜像 → 建/启容器 → socket → 扩展。
    async fn install_runtime_task(&self, id: &str, job_id: &str) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{id}"))?;
        let key = format!("{}:{}", rt.kind, rt.name);
        let _guard = self
            .runtime_installs
            .try_begin(&key)
            .ok_or_else(|| "该环境正在安装，请稍后再试".to_string())?;

        self.job_advance(job_id, "检查 Docker", 8, "检查 Docker 可用性")
            .await;
        if !self.docker().available() {
            return Err(format!(
                "Docker 不可用：{}",
                self.docker_unavailable_reason()
            ));
        }
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        let s = self.site_settings();
        if rt.kind == skiff_store::KIND_RUST {
            return self.install_rust_runtime_task(&rt, job_id, &s).await;
        }
        if rt.kind == skiff_store::KIND_NODE {
            return self.install_node_runtime_task(&rt, job_id, &s).await;
        }
        // PHP 扩展安装器随包分发：先落到共享 web_root，容器内 `/www/skiff/tools`
        // 直接可见；运行时不再依赖 GitHub。
        self.ensure_php_extension_installer(&s)
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let cfg = PhpRuntimeConfig::from_json(&rt.config_json);
        cfg.validate().map_err(|e| e.to_string())?;

        self.job_advance(
            job_id,
            "准备目录与配置",
            15,
            "创建实例目录并写入 PHP-FPM 池配置",
        )
        .await;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 PHP 日志目录失败：{e}"))?;
        // apk 包缓存挂载源必须存在（Docker 不会替我们建宿主机目录）。
        std::fs::create_dir_all(s.system_pkg_cache_dir())
            .map_err(|e| format!("创建依赖包缓存目录失败：{e}"))?;
        let socket_container = runtime_container_socket(&rt.kind, &rt.name);
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)?;
        self.ensure_php_account_init(&s)
            .map_err(|e| format!("准备 PHP 容器身份初始化脚本失败：{e}"))?;
        // 标记“本实例创建时已带 php.ini / conf.d 挂载”：只有安装流程写这个标记，
        // 保存配置时补目录不写，旧实例的“编辑不生效”提示就不会被误清掉。
        std::fs::write(
            s.runtime_php_dir(&rt.kind, &rt.name)
                .join(PHP_CONFIG_SENTINEL),
            "1\n",
        )
        .map_err(|e| format!("写入 PHP 配置挂载标记失败：{e}"))?;
        std::fs::write(
            s.runtime_pool_conf(&rt.kind, &rt.name),
            render_php_pool_with(&cfg, &socket_container),
        )
        .map_err(|e| format!("写入 PHP-FPM 配置失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;
        let pinned = pinned_image_ref(&image, &image_digest);
        // 统一身份依赖镜像「root 启动 + 官方入口」；不满足直接失败，不猜。
        self.verify_php_image_identity(&pinned).await?;
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            50,
            &format!("创建容器 {container}（已有同名容器会先移除）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.php_spec_for(&s, &rt.kind, &rt.name, &pinned))
            .await
            .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        let enabled = runtime_catalog::split_extensions(&rt.extensions);
        if enabled.is_empty() {
            self.job_advance(
                job_id,
                "启动 PHP-FPM",
                62,
                "收紧 socket 权限（0660 root:nobody）",
            )
            .await;
            self.secure_php_socket(&s, &rt.kind, &rt.name)
                .await
                .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        } else {
            // 有扩展时跳过安装期的第一次 socket 收紧：反正随后要重启容器，
            // 重启后统一 chmod，少等一轮 socket。
            self.job_advance(
                job_id,
                "安装扩展",
                65,
                &format!("安装 {} 个扩展：{}", enabled.len(), enabled.join("、")),
            )
            .await;
            self.run_extension_scripts(job_id, &container, &enabled, &[], 65, 92)
                .await?;
            self.job_advance(job_id, "重载 PHP-FPM", 96, "重启容器以加载扩展")
                .await;
            self.docker_runtime_action(&container, Verb::Restart)
                .await
                .map_err(|e| format!("重载 PHP-FPM 失败：{e}"))?;
            self.secure_php_socket(&s, &rt.kind, &rt.name)
                .await
                .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        }

        // 以本次安装的镜像与 socket 为准写回库；name、扩展与默认标记保持不变。
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: Some(s.runtime_socket(&rt.kind, &rt.name).display().to_string()),
                port: None,
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// Rust 环境安装：写最小 std HTTP 工程 → 拉镜像 → 容器内 `cargo run --release`
    /// → 等端口监听。日志重定向到 `/www/runtime/rust/<name>/log/server.log`。
    async fn install_rust_runtime_task(
        &self,
        rt: &Runtime,
        job_id: &str,
        s: &SiteSettings,
    ) -> Result<(), String> {
        let port = rt
            .port
            .ok_or_else(|| "Rust 环境缺少固定端口，请删除后重新创建".to_string())?;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        let app_dir = dir.join("app");
        std::fs::create_dir_all(app_dir.join("src"))
            .map_err(|e| format!("创建 Rust 工程目录失败：{e}"))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Rust 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（无第三方依赖）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的工程，重建不能覆盖。
        seed_file(
            &app_dir.join("Cargo.toml"),
            rust_cargo_manifest().as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 Cargo.toml 失败：{e}"))?;
        seed_file(
            &app_dir.join("src/main.rs"),
            rust_main_source(&rt.version, port).as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 main.rs 失败：{e}"))?;
        // 入口脚本是内核资产，每次安装/重建都刷新。
        std::fs::write(dir.join("entry.sh"), rust_entry_script())
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            50,
            &format!("创建容器 {container}（host 网络，端口 {port}）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.rust_spec_for(
            s,
            &rt.name,
            &pinned_image_ref(&image, &image_digest),
            port,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "构建并启动进程",
            70,
            &format!("cargo run --release（监听 127.0.0.1:{port}），首次编译需等待"),
        )
        .await;
        if !wait_tcp_port(port, std::time::Duration::from_secs(240)).await {
            let log =
                std::fs::read_to_string(s.runtime_log_dir(&rt.kind, &rt.name).join("server.log"))
                    .unwrap_or_default();
            let tail: Vec<&str> = log
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!(
                "端口 {port} 在 240 秒内未监听，构建或启动失败：\n{}",
                tail.join("\n")
            ));
        }
        self.job_note(job_id, &format!("端口 {port} 已就绪，Rust 进程返回内容"))
            .await;
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: None,
                port: Some(port),
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// Node.js 环境安装：写最小 HTTP 工程 → 拉镜像 → host 网络起单进程 →
    /// HTTP 健康检查。日志重定向到 `/www/runtime/node/<name>/log/server.log`。
    ///
    /// 与 PHP 的差异：没有 FPM 池/socket，也没有「多站点共享一个池」的模型；
    /// 一个环境固定一个端口、一个常驻进程、一个代码目录，绑定站点只是域名反代。
    async fn install_node_runtime_task(
        &self,
        rt: &Runtime,
        job_id: &str,
        s: &SiteSettings,
    ) -> Result<(), String> {
        let port = rt
            .port
            .ok_or_else(|| "Node.js 环境缺少固定端口，请删除后重新创建".to_string())?;
        let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
        cfg.validate()
            .map_err(|e| format!("Node.js 配置不合法：{e}"))?;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        let app_dir = dir.join("app");
        std::fs::create_dir_all(&app_dir).map_err(|e| format!("创建 Node 工程目录失败：{e}"))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Node 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（npm start）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的代码，重建不能覆盖。
        seed_file(
            &app_dir.join("package.json"),
            node_package_manifest().as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 package.json 失败：{e}"))?;
        seed_file(
            &app_dir.join("server.js"),
            node_server_source(&rt.version, port).as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 server.js 失败：{e}"))?;
        // 入口脚本是内核资产，每次安装/重建都刷新。
        write_if_changed(&dir.join("entry.sh"), node_entry_script().as_bytes(), 0o755)
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            55,
            &format!("创建容器 {container}（host 网络，端口 {port}）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.node_spec_for(
            s,
            &rt.name,
            &pinned_image_ref(&image, &image_digest),
            port,
            &cfg,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "启动 Node 进程",
            78,
            &format!(
                "{}（监听 127.0.0.1:{port}，健康检查 {}）",
                cfg.command, cfg.health_path
            ),
        )
        .await;
        if !wait_http_ready(port, &cfg.health_path, std::time::Duration::from_secs(180)).await {
            let log =
                std::fs::read_to_string(s.runtime_log_dir(&rt.kind, &rt.name).join("server.log"))
                    .unwrap_or_default();
            let tail: Vec<&str> = log
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!(
                "端口 {port} 在 180 秒内未通过健康检查（{}），启动失败：\n{}",
                cfg.health_path,
                tail.join("\n")
            ));
        }
        self.job_note(
            job_id,
            &format!("端口 {port} 已就绪，Node 进程通过健康检查"),
        )
        .await;
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: None,
                port: Some(port),
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// 批量启用/停用扩展：安装与卸载各只开**一次** `docker exec`。
    ///
    /// 安装主路径把多个包名一次交给 mlocati 安装器；回退路径按「union 依赖 →
    /// 逐项编译 → 末尾一次清理」，并保留按扩展的分段标记。卸载把多个 ini
    /// 删除合并到一次 exec。
    async fn run_extension_scripts(
        &self,
        job_id: &str,
        container: &str,
        enabled: &[String],
        previous: &[String],
        base: u8,
        span: u8,
    ) -> Result<(), String> {
        let mut to_install: Vec<String> = Vec::new();
        for key in enabled {
            if previous.iter().any(|p| p == key) {
                continue;
            }
            // 镜像内置扩展（opcache / curl）已随 PHP 编入并默认启用，
            // 再执行 docker-php-ext-enable 会因找不到 .so 而失败（实测 exit 1）。
            if runtime_catalog::php_extension(key).is_some_and(|d| d.builtin) {
                let label = runtime_catalog::extension_label(key);
                self.job_note(job_id, &format!("{label}：镜像内置已启用，跳过"))
                    .await;
                continue;
            }
            if runtime_catalog::extension_install_script(key).is_none() {
                return Err(format!("扩展 {key} 没有可执行的安装脚本"));
            }
            to_install.push(key.clone());
        }
        if !to_install.is_empty() {
            let labels: Vec<String> = to_install
                .iter()
                .map(|k| runtime_catalog::extension_label(k))
                .collect();
            let script = runtime_catalog::batch_install_script(&to_install)
                .ok_or_else(|| "批量安装脚本生成失败".to_string())?;
            self.job_note(
                job_id,
                &format!(
                    "批量启用 {} 项：{}（单次 exec，安装器一次传多个包名）",
                    to_install.len(),
                    labels.join("、")
                ),
            )
            .await;
            self.jobs().set_cancellable(job_id, false);
            let (code, out, timed_out) = self
                .docker()
                .exec_capture_timeout(
                    container,
                    vec!["sh".into(), "-lc".into(), script],
                    self.cfg.ext_batch_timeout,
                )
                .await
                .map_err(|e| format!("批量安装扩展执行失败：{e}"))?;
            self.jobs().set_exit_code(job_id, code);
            self.job_log_output(job_id, &out).await;
            self.jobs().set_cancellable(job_id, true);
            if timed_out {
                return Err(format!(
                    "{TIMEOUT_MARK}批量安装扩展超过 {} 未结束，已按超时失败结算；\
                     Docker 无法中断 exec，容器内脚本可能仍在运行，建议重建运行环境。",
                    fmt_timeout(self.cfg.ext_batch_timeout)
                ));
            }
            if self.job_cancelled(job_id).await {
                return Err("已取消".into());
            }
            if code != 0 {
                return Err(format!(
                    "批量安装扩展失败（exit {code}）：{}（主路径与回退路径输出见任务日志）",
                    labels.join("、")
                ));
            }
            let progress = base.saturating_add(span / 2);
            self.job_advance(
                job_id,
                "安装扩展",
                progress,
                &format!("已启用 {} 项：{}", to_install.len(), labels.join("、")),
            )
            .await;
        }

        let mut to_remove: Vec<String> = Vec::new();
        for key in previous {
            if enabled.iter().any(|e| e == key) {
                continue;
            }
            // 内置扩展没有独立 .so，docker-php-ext-disable 同样会失败，直接跳过。
            if runtime_catalog::php_extension(key).is_some_and(|d| d.builtin) {
                continue;
            }
            if runtime_catalog::disable_script(key).is_none() {
                return Err(format!("扩展 {key} 不支持停用"));
            }
            to_remove.push(key.clone());
        }
        if !to_remove.is_empty() {
            let labels: Vec<String> = to_remove
                .iter()
                .map(|k| runtime_catalog::extension_label(k))
                .collect();
            let script = runtime_catalog::batch_disable_script(&to_remove)
                .ok_or_else(|| "批量停用脚本生成失败".to_string())?;
            self.job_note(
                job_id,
                &format!(
                    "批量停用 {} 项：{}（单次 exec）",
                    to_remove.len(),
                    labels.join("、")
                ),
            )
            .await;
            self.jobs().set_cancellable(job_id, false);
            let (code, out, timed_out) = self
                .docker()
                .exec_capture_timeout(
                    container,
                    vec!["sh".into(), "-lc".into(), script],
                    self.cfg.ext_batch_timeout,
                )
                .await
                .map_err(|e| format!("批量停用扩展执行失败：{e}"))?;
            self.jobs().set_exit_code(job_id, code);
            self.job_log_output(job_id, &out).await;
            self.jobs().set_cancellable(job_id, true);
            if timed_out {
                return Err(format!(
                    "{TIMEOUT_MARK}批量停用扩展超过 {} 未结束，已按超时失败结算；\
                     Docker 无法中断 exec，容器内脚本可能仍在运行，建议重建运行环境。",
                    fmt_timeout(self.cfg.ext_batch_timeout)
                ));
            }
            if self.job_cancelled(job_id).await {
                return Err("已取消".into());
            }
            if code != 0 {
                return Err(format!(
                    "批量停用扩展失败（exit {code}）：{}",
                    labels.join("、")
                ));
            }
            self.job_note(job_id, &format!("已停用 {} 项", to_remove.len()))
                .await;
        }
        Ok(())
    }

    /// 提交扩展变更（异步）。返回 `true` 表示确实启动了任务；集合未变化时返回 `false`。
    pub async fn update_runtime_extensions(
        self: &Arc<Self>,
        id: &str,
        selected: &[String],
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(
                "只有 PHP 运行环境支持扩展管理".into(),
            ));
        }
        let desired = runtime_catalog::normalize_extensions(selected);
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if desired == current {
            return Ok(false);
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.apply".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!("更新 {} 的 PHP 扩展", rt.name),
                params: serde_json::json!({"runtime_id": id, "ext_keys": desired}),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_runtime_extensions_job(&rid, &job_id, desired)
                .await;
        });
        Ok(true)
    }

    async fn run_runtime_extensions_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: Vec<String>,
    ) {
        let result = self.extensions_task(runtime_id, job_id, &desired).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_ok(job_id, "扩展已更新，PHP-FPM 已重载")
                        .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                // 扩展失败不改变容器可用性：状态回 running，错误留在任务里可重试。
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    /// 安装单个扩展（目录项或 `pecl:...`）。异步任务，立即返回；已启用返回 false。
    pub async fn install_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let token = token.trim();
        if runtime_catalog::extension_install_script(token).is_none() {
            return Err(SiteError::BadRequest(format!("不支持安装扩展：{token}")));
        }
        if let Some(def) = runtime_catalog::php_extension(token) {
            if def.builtin {
                return Err(SiteError::BadRequest(format!(
                    "{} 为镜像内置扩展，无需安装",
                    runtime_catalog::extension_label(token)
                )));
            }
            let base = runtime_catalog::base_image_of(&rt.image);
            if let Some((code, reason)) =
                runtime_catalog::extension_block_reason(def, &rt.version, base)
            {
                return Err(SiteError::BadRequest(format!("{code}: {reason}")));
            }
        }
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if current.iter().any(|k| k == token) {
            return Ok(false);
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "安装 PHP 扩展 {}（{}）",
                    runtime_catalog::extension_label(token),
                    rt.name
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": [token],
                    "action": "install",
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_single_extension_job(&rid, &job_id, &target, true)
                .await;
        });
        Ok(true)
    }

    /// 卸载单个扩展：删 conf.d 里的 ini → 重启容器 → 从库里的启用集合移除。
    pub async fn uninstall_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let token = token.trim();
        if runtime_catalog::php_extension(token).is_some_and(|e| e.builtin) {
            return Err(SiteError::BadRequest(format!(
                "{} 编译进镜像，不能卸载；如需关闭请修改 PHP 配置",
                runtime_catalog::extension_label(token)
            )));
        }
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if !current.iter().any(|k| k == token) {
            return Ok(false);
        }
        if runtime_catalog::disable_script(token).is_none() {
            return Err(SiteError::BadRequest(format!("不支持卸载扩展：{token}")));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.uninstall".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "卸载 PHP 扩展 {}（{}）",
                    runtime_catalog::extension_label(token),
                    rt.name
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": [token],
                    "action": "uninstall",
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_single_extension_job(&rid, &job_id, &target, false)
                .await;
        });
        Ok(true)
    }

    /// 按名字安装 PECL 扩展（版本可选），例如 `swoole` / `swoole 5.1.2`。
    pub async fn install_pecl_extension(
        self: &Arc<Self>,
        id: &str,
        name: &str,
        version: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let token = runtime_catalog::pecl_token(name, version);
        if runtime_catalog::parse_pecl_token(&token).is_none() {
            return Err(SiteError::BadRequest(
                "PECL 扩展名只能包含小写字母、数字与下划线".into(),
            ));
        }
        self.install_extension(id, &token, actor, role).await
    }

    /// 批量勾选安装/卸载：一次任务内按目录顺序串行执行，日志合并，只重启一次容器。
    ///
    /// 返回真正发生变化的扩展数（全部已处于目标状态时为 0，不创建任务）。
    pub async fn apply_extension_selection(
        self: &Arc<Self>,
        id: &str,
        add: bool,
        keys: &[String],
        actor: &str,
        role: &str,
    ) -> Result<usize, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let mut changed: Vec<String> = Vec::new();
        for raw in keys {
            let token = raw.trim();
            if token.is_empty() {
                continue;
            }
            if add {
                if runtime_catalog::extension_install_script(token).is_none() {
                    return Err(SiteError::BadRequest(format!("不支持安装扩展：{token}")));
                }
                if let Some(def) = runtime_catalog::php_extension(token) {
                    if def.builtin {
                        return Err(SiteError::BadRequest(format!(
                            "{} 为镜像内置扩展，无需安装",
                            runtime_catalog::extension_label(token)
                        )));
                    }
                    let base = runtime_catalog::base_image_of(&rt.image);
                    if let Some((code, reason)) =
                        runtime_catalog::extension_block_reason(def, &rt.version, base)
                    {
                        return Err(SiteError::BadRequest(format!("{code}: {reason}")));
                    }
                }
                if !current.iter().any(|k| k == token) && !changed.iter().any(|k| k == token) {
                    changed.push(token.to_string());
                }
            } else {
                if runtime_catalog::php_extension(token).is_some_and(|e| e.builtin) {
                    return Err(SiteError::BadRequest(format!(
                        "{} 编译进镜像，不能卸载；如需关闭请修改 PHP 配置",
                        runtime_catalog::extension_label(token)
                    )));
                }
                if runtime_catalog::disable_script(token).is_none() {
                    return Err(SiteError::BadRequest(format!("不支持卸载扩展：{token}")));
                }
                if current.iter().any(|k| k == token) && !changed.iter().any(|k| k == token) {
                    changed.push(token.to_string());
                }
            }
        }
        if changed.is_empty() {
            return Ok(0);
        }
        // 目标集合：安装=当前∪选择；卸载=当前−选择。交给同一个串行任务执行。
        let mut desired = current;
        if add {
            for key in &changed {
                if !desired.iter().any(|k| k == key) {
                    desired.push(key.clone());
                }
            }
            desired = runtime_catalog::normalize_extensions(&desired);
        } else {
            desired.retain(|k| !changed.iter().any(|c| c == k));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: if add {
                    "runtime.ext.install".into()
                } else {
                    "runtime.ext.uninstall".into()
                },
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "{} {} 的 {} 项 PHP 扩展",
                    if add { "批量安装" } else { "批量卸载" },
                    rt.name,
                    changed.len()
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": changed,
                    "action": if add { "install" } else { "uninstall" },
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let targets = changed.clone();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_batch_extensions_job(&rid, &job_id, &desired, &targets)
                .await;
        });
        Ok(changed.len())
    }

    /// 重试历史任务：与正常提交共用同一组执行器，避免「只插行不执行」留下 queued 僵尸。
    ///
    /// - 只有当前代码仍有执行器的类型可重试；其余直接拒绝，**不插入新行**。
    /// - 同目标已有活跃任务时，`JobService::retry` 经唯一索引返回 Busy（明确拒绝并给原因），
    ///   不做排队；取消旧任务后即可再次提交。
    pub async fn retry_job(
        self: &Arc<Self>,
        id: &str,
        actor: &str,
        role: &str,
        ip: &str,
    ) -> Result<app_jobs::Job, SiteError> {
        let old = self
            .jobs()
            .get(id)
            .ok_or_else(|| SiteError::BadRequest(format!("任务不存在：{id}")))?;
        let runtime_id = old
            .params
            .get("runtime_id")
            .and_then(|v| v.as_str())
            .unwrap_or(old.target_id.as_str())
            .to_string();
        let ext_keys: Vec<String> = old
            .params
            .get("ext_keys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // 先算好执行计划：未知类型或运行环境已不存在时不插入重试行。
        let plan = match old.kind.as_str() {
            "runtime.install" => RetryPlan::Install,
            "runtime.ext.apply" => RetryPlan::Apply(ext_keys),
            "runtime.ext.install" | "runtime.ext.uninstall" => {
                if self
                    .db()
                    .runtime_by_id(&runtime_id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?
                    .is_none()
                {
                    return Err(SiteError::BadRequest(format!(
                        "运行环境不存在：{runtime_id}"
                    )));
                }
                let add = old
                    .params
                    .get("action")
                    .and_then(|v| v.as_str())
                    .map(|a| a == "install")
                    .unwrap_or(old.kind.ends_with(".install"));
                if ext_keys.is_empty() {
                    return Err(SiteError::BadRequest(
                        "原任务没有记录扩展集合，无法重试".into(),
                    ));
                }
                if ext_keys.len() == 1 {
                    // 单个扩展与正常提交一致，走单扩展执行器。
                    RetryPlan::Single {
                        token: ext_keys[0].clone(),
                        add,
                    }
                } else {
                    let rt = self
                        .db()
                        .runtime_by_id(&runtime_id)
                        .map_err(|e| SiteError::Internal(e.to_string()))?
                        .ok_or_else(|| {
                            SiteError::BadRequest(format!("运行环境不存在：{runtime_id}"))
                        })?;
                    // 目标集合与正常提交一致：安装=当前∪选择；卸载=当前−选择。
                    let mut desired = runtime_catalog::split_extensions(&rt.extensions);
                    if add {
                        for key in &ext_keys {
                            if !desired.iter().any(|k| k == key) {
                                desired.push(key.clone());
                            }
                        }
                        desired = runtime_catalog::normalize_extensions(&desired);
                    } else {
                        desired.retain(|k| !ext_keys.iter().any(|t| t == k));
                    }
                    RetryPlan::Batch {
                        desired,
                        targets: ext_keys,
                    }
                }
            }
            other => {
                return Err(SiteError::BadRequest(format!(
                    "任务类型 {other} 不支持重试（当前没有对应执行器）"
                )))
            }
        };

        let job = self
            .jobs()
            .retry(id, actor, role, ip)
            .map_err(site_error_from_job)?;
        let job_id = job.id.clone();
        let state = self.clone();
        match plan {
            RetryPlan::Install => {
                if let Err(e) = self
                    .db()
                    .set_runtime_status(&runtime_id, "running", "installing")
                {
                    tracing::warn!(runtime = %runtime_id, error = %e, "重试时写入运行环境状态失败");
                }
                tokio::spawn(async move {
                    state.run_runtime_install_job(&runtime_id, &job_id).await;
                });
            }
            RetryPlan::Single { token, add } => {
                tokio::spawn(async move {
                    state
                        .run_single_extension_job(&runtime_id, &job_id, &token, add)
                        .await;
                });
            }
            RetryPlan::Apply(desired) => {
                tokio::spawn(async move {
                    state
                        .run_runtime_extensions_job(&runtime_id, &job_id, desired)
                        .await;
                });
            }
            RetryPlan::Batch { desired, targets } => {
                tokio::spawn(async move {
                    state
                        .run_batch_extensions_job(&runtime_id, &job_id, &desired, &targets)
                        .await;
                });
            }
        }
        Ok(job)
    }

    async fn run_batch_extensions_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: &[String],
        targets: &[String],
    ) {
        let result = self.extensions_task(runtime_id, job_id, desired).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_ok(
                        job_id,
                        &format!(
                            "已应用 {} 项扩展变更（{}）；PHP-FPM 已重载",
                            targets.len(),
                            targets.join("、")
                        ),
                    )
                    .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    fn php_runtime_for_extension(&self, id: &str) -> Result<Runtime, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(
                "只有 PHP 运行环境支持扩展管理".into(),
            ));
        }
        Ok(rt)
    }

    async fn run_single_extension_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        token: &str,
        add: bool,
    ) {
        let result = self
            .single_extension_task(runtime_id, job_id, token, add)
            .await;
        let label = runtime_catalog::extension_label(token);
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展状态可能未生效，请核对后重试");
                } else {
                    self.job_finish_ok(
                        job_id,
                        &format!(
                            "扩展 {label} {}，PHP-FPM 已重载",
                            if add { "已启用" } else { "已停用" }
                        ),
                    )
                    .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展状态可能未生效，请核对后重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    async fn single_extension_task(
        &self,
        runtime_id: &str,
        job_id: &str,
        token: &str,
        add: bool,
    ) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(runtime_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{runtime_id}"))?;
        let label = runtime_catalog::extension_label(token);
        self.ensure_php_extension_installer(&self.site_settings())
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let container = runtime_container_name(&rt.kind, &rt.name);
        let exists = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| format!("读取容器列表失败：{e}"))?
            .iter()
            .any(|c| c.name == container && c.state == "running");
        if !exists {
            return Err(format!(
                "PHP 容器未运行，无法热装扩展；请先安装/启动运行环境（{container}）"
            ));
        }
        let script = if add {
            runtime_catalog::extension_install_script(token)
                .ok_or_else(|| format!("扩展 {label} 没有可执行的安装脚本"))?
        } else {
            runtime_catalog::disable_script(token)
                .ok_or_else(|| format!("扩展 {label} 不支持卸载"))?
        };
        self.job_advance_blocking(
            job_id,
            if add { "安装扩展" } else { "卸载扩展" },
            15,
            &format!("{label}：{script}"),
        )
        .await;
        let (code, out, timed_out) = self
            .docker()
            .exec_capture_timeout(
                &container,
                vec!["sh".into(), "-lc".into(), script],
                self.cfg.ext_exec_timeout,
            )
            .await
            .map_err(|e| format!("{label} 执行失败：{e}"))?;
        self.jobs().set_exit_code(job_id, code);
        self.job_log_output(job_id, &out).await;
        // 脚本结束（无论成败）后重新允许取消。
        self.jobs().set_cancellable(job_id, true);
        if timed_out {
            return Err(format!(
                "{TIMEOUT_MARK}扩展 {label} 执行超过 {} 未结束，已按超时失败结算；\
                 Docker 无法中断 exec，容器内脚本可能仍在运行，建议重载或重建运行环境。\
                 已采集输出尾部见任务日志。",
                fmt_timeout(self.cfg.ext_exec_timeout)
            ));
        }
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        if code != 0 {
            // 必须回传真实报错（pecl 常见缺依赖/版本不存在），不能只报「安装失败」。
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            let start = lines.len().saturating_sub(20);
            let err_code = if add {
                runtime_catalog::pecl_failure_code(&out)
            } else {
                None
            };
            let prefix = match err_code {
                Some(c) => format!(
                    "{c}：PHP {} 下扩展 {label} 可能尚未就绪（PECL 滞后于新版 PHP）；可稍后重试或指定兼容版本。\n",
                    rt.version
                ),
                None => String::new(),
            };
            return Err(format!(
                "{prefix}扩展 {label} {}失败（exit {code}）：\n{}",
                if add { "安装" } else { "卸载" },
                lines[start..].join("\n")
            ));
        }
        self.job_advance(job_id, "重载 PHP-FPM", 80, "重启容器以加载扩展变更")
            .await;
        self.docker_runtime_action(&container, Verb::Restart)
            .await
            .map_err(|e| format!("重启 PHP-FPM 失败：{e}"))?;
        let s = self.site_settings();
        self.secure_php_socket(&s, &rt.kind, &rt.name)
            .await
            .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        let mut next = current;
        if add {
            next.push(token.to_string());
        } else {
            next.retain(|k| k != token);
        }
        let joined = runtime_catalog::join_extensions(&next);
        self.db()
            .set_runtime_extensions(runtime_id, &joined)
            .map_err(|e| format!("写入扩展集合失败：{e}"))?;
        Ok(())
    }

    async fn extensions_task(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: &[String],
    ) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(runtime_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{runtime_id}"))?;
        self.ensure_php_extension_installer(&self.site_settings())
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let container = runtime_container_name(&rt.kind, &rt.name);
        let exists = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| format!("读取容器列表失败：{e}"))?
            .iter()
            .any(|c| c.name == container);

        // 容器不存在时，先把期望集合落库，再走完整安装（安装阶段会重放扩展）。
        if !exists {
            self.job_advance(job_id, "准备重建", 10, "容器不存在，改为完整安装并重放扩展")
                .await;
            let joined = runtime_catalog::join_extensions(desired);
            self.db()
                .set_runtime_extensions(runtime_id, &joined)
                .map_err(|e| format!("写入扩展集合失败：{e}"))?;
            // 这里不预先抢安装锁：`install_runtime_task` 自己会 `try_begin`，
            // 先抢再立刻丢弃的守卫既没有互斥效果，还会让嵌套获取误判「正在安装」。
            return self.install_runtime_task(runtime_id, job_id).await;
        }

        self.job_advance(job_id, "应用扩展变更", 20, "对比扩展集合，执行启用/停用")
            .await;
        self.run_extension_scripts(job_id, &container, desired, &current, 20, 60)
            .await?;
        self.job_advance(job_id, "重载 PHP-FPM", 85, "重启容器以加载扩展变更")
            .await;
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        self.docker_runtime_action(&container, Verb::Restart)
            .await
            .map_err(|e| format!("重启 PHP-FPM 失败：{e}"))?;
        let s = self.site_settings();
        self.secure_php_socket(&s, &rt.kind, &rt.name)
            .await
            .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        let joined = runtime_catalog::join_extensions(desired);
        self.db()
            .set_runtime_extensions(runtime_id, &joined)
            .map_err(|e| format!("写入扩展集合失败：{e}"))?;
        Ok(())
    }

    /// 新建运行环境（类型 + 版本目录 + 可选扩展）并异步安装。
    ///
    /// 同步部分只做校验与落库（名称唯一、版本在目录内、目录不覆盖）；安装本身
    /// 交给后台任务。Node.js/Go 在 P1 只登记不安装，返回 `false`。
    pub async fn add_runtime(
        self: &Arc<Self>,
        kind: &str,
        raw_name: &str,
        version: &str,
        extensions: &[String],
        actor: &str,
        role: &str,
    ) -> Result<(Runtime, bool), SiteError> {
        let kind = kind.trim();
        let def = runtime_catalog::kind_def(kind)
            .ok_or_else(|| SiteError::BadRequest(format!("未知运行环境类型：{kind}")))?;
        let version = version.trim();
        if !runtime_catalog::version_supported(kind, version) {
            return Err(SiteError::BadRequest(format!(
                "{} 版本无效：请从版本下拉中选择（可选：{}）",
                def.label,
                def.versions.join(" / ")
            )));
        }
        let raw_name = raw_name.trim();
        let name = if raw_name.is_empty() {
            runtime_catalog::default_name(kind, version)
        } else {
            skiff_store::validate_runtime_name(raw_name)
                .map_err(|e| SiteError::BadRequest(e.to_string()))?
                .to_string()
        };
        let mut inherit_default = false;
        if let Some(existing) = self
            .db()
            .runtime_by_name(kind, &name)
            .map_err(|e| SiteError::Internal(e.to_string()))?
        {
            // 迁移种入、从未安装且无站点引用的模板行可被同名新建接管；
            // 其它已存在的实例仍按重名拒绝。
            let seed_free = existing.origin == skiff_store::ORIGIN_SEED
                && self
                    .db()
                    .runtime_site_count(&existing.id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?
                    == 0;
            if seed_free {
                inherit_default = existing.is_default;
                self.db()
                    .delete_runtime(&existing.id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            } else {
                return Err(SiteError::Conflict(format!(
                    "同类型下已存在名为 {name} 的运行环境，请换一个名字"
                )));
            }
        }
        let s = self.site_settings();
        if runtime_catalog::is_implemented(kind) {
            let dir = s.runtime_home(kind, &name);
            if dir.exists() {
                return Err(SiteError::Conflict(format!(
                    "目标目录已存在：{}，请换一个名字（不会覆盖或自动加后缀）",
                    dir.display()
                )));
            }
        }
        let image = runtime_catalog::image_for(kind, version, &s.php_image);
        let enabled = if kind == skiff_store::KIND_PHP {
            runtime_catalog::normalize_extensions(extensions)
        } else {
            Vec::new()
        };
        if kind == skiff_store::KIND_PHP {
            if let Err((code, reason)) =
                runtime_catalog::validate_selection(version, &image, &enabled)
            {
                return Err(SiteError::BadRequest(format!("{code}: {reason}")));
            }
        }
        // Node.js / Rust 是「独立进程 + 固定端口」：建环境时一次性分配，冲突直接拒绝。
        // PHP 走 socket，不占宿主端口。
        let port = if let Some(range) = runtime_catalog::port_range(kind) {
            let used: Vec<u16> = self
                .db()
                .list_runtimes()
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .iter()
                .filter_map(|r| r.port)
                .collect();
            Some(runtime_catalog::allocate_port(kind, &used).ok_or_else(|| {
                SiteError::Conflict(format!(
                    "{} 端口区间 {}–{} 已用满，请先删除不再使用的环境",
                    runtime_catalog::kind_label(kind),
                    *range.start(),
                    *range.end()
                ))
            })?)
        } else {
            None
        };
        // PHP 沿用「实例名即 id」的既有约定；其它类型加前缀避免与 PHP 实例名撞车。
        let id = if kind == skiff_store::KIND_PHP {
            name.clone()
        } else {
            format!("{kind}-{name}")
        };
        let rt = self
            .db()
            .create_runtime(&NewRuntime {
                id: id.clone(),
                kind: kind.to_string(),
                name: name.clone(),
                version: version.to_string(),
                image,
                image_digest: String::new(),
                socket_path: if kind == skiff_store::KIND_PHP {
                    Some(s.runtime_socket(kind, &name).display().to_string())
                } else {
                    None
                },
                port,
                is_default: inherit_default,
            })
            .map_err(|e| SiteError::Conflict(e.to_string()))?;
        let joined = runtime_catalog::join_extensions(&enabled);
        self.db()
            .set_runtime_extensions(&rt.id, &joined)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        let started = runtime_catalog::is_implemented(kind);
        if started {
            self.start_runtime_install(&rt.id, actor, role).await?;
        }
        Ok((rt, started))
    }

    /// 保存 PHP 专属配置：校验 → 写池配置 → reload FPM → 落库；任一步失败回滚文件。
    pub async fn save_php_runtime_config(
        &self,
        id: &str,
        cfg: &PhpRuntimeConfig,
        actor: &str,
    ) -> Result<RuntimeView, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境不使用 php.ini / FPM 专属配置",
                rt.kind
            )));
        }
        cfg.validate()?;
        let s = self.site_settings();
        let socket_host = rt
            .socket_path
            .clone()
            .unwrap_or_else(|| s.runtime_socket(&rt.kind, &rt.name).display().to_string());
        let socket_container = s.container_path(Path::new(&socket_host));
        std::fs::create_dir_all(s.runtime_home(&rt.kind, &rt.name))
            .map_err(|e| SiteError::Internal(format!("创建运行目录失败：{e}")))?;
        let pool = s.runtime_pool_conf(&rt.kind, &rt.name);
        let original = std::fs::read(&pool).ok();
        // 用户可见写操作统一走 skiff-fs：原子替换 + 审计，不在 app 层直接 std::fs::write。
        self.admin_fs()
            .write(
                actor,
                &pool.display().to_string(),
                &render_php_pool_with(cfg, &socket_container),
            )
            .map_err(|e| SiteError::Internal(format!("写入 PHP-FPM 配置失败：{e}")))?;
        if let Err(e) = self
            .docker_runtime_action(&runtime_container_name(&rt.kind, &rt.name), Verb::Restart)
            .await
        {
            if let Some(bytes) = &original {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    if let Err(we) = self
                        .admin_fs()
                        .write(actor, &pool.display().to_string(), text)
                    {
                        tracing::error!(path = %pool.display(), error = %we, "回滚 PHP 池配置写入失败");
                    }
                }
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚：重载 PHP-FPM 失败：{e}"
            )));
        }
        self.db()
            .set_runtime_config(id, &cfg.to_json())
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        self.runtime_view(id).await
    }

    /// 保存 Node.js 专属配置。启动命令/环境变量在容器的 `command` 与 `Env` 里，
    /// 只能重建容器才生效，所以这里只落库并返回「是否有变化」，由调用方决定是否
    /// 触发重建任务（与 PHP 写池配置后热重启不同）。
    pub async fn save_node_runtime_config(
        &self,
        id: &str,
        cfg: &NodeRuntimeConfig,
        actor: &str,
    ) -> Result<(RuntimeView, bool), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_NODE {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境没有 Node.js 专属配置",
                rt.kind
            )));
        }
        cfg.validate()?;
        let previous = NodeRuntimeConfig::from_json(&rt.config_json);
        let changed = &previous != cfg;
        if changed {
            self.db()
                .set_runtime_config(id, &cfg.to_json())
                .map_err(|e| SiteError::Internal(e.to_string()))?;
            tracing::info!(actor = %actor, runtime = %id, "Node.js 运行环境配置已更新");
        }
        Ok((self.runtime_view(id).await?, changed))
    }

    /// 实例创建时是否已带 `php.ini` / `conf.d` 挂载（决定配置编辑是否真生效）。
    ///
    /// 旧实例（早于配置文件编辑功能）容器里没有这两个挂载，编辑能保存但不生效；
    /// 详情页据此提示“重建后生效”，避免用户对着能编辑的界面猜。
    pub fn php_config_mounts_current(&self, id: &str) -> Result<bool, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        Ok(s.runtime_php_dir(&rt.kind, &rt.name)
            .join(PHP_CONFIG_SENTINEL)
            .exists())
    }

    /// 列出 PHP 实例可直接查看/编辑的配置文件。
    ///
    /// 当前实际存在三类：`php.ini`（宿主新建，挂到容器主配置）、FPM 池 `www.conf`
    /// （宿主生成，覆盖镜像默认池）、`conf.d/*.ini`（宿主托管扩展 ini，扫在镜像
    /// `conf.d` 之后）。镜像里由扩展安装器写入的 ini 不在宿主侧，不在此列。
    pub fn php_config_files(&self, id: &str) -> Result<Vec<PhpConfigFile>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let mut files = vec![
            PhpConfigFile {
                key: "php.ini".into(),
                label: "php.ini".into(),
                host_path: s.runtime_php_ini(&rt.kind, &rt.name).display().to_string(),
                container_path: CONTAINER_PHP_INI.into(),
                exists: s.runtime_php_ini(&rt.kind, &rt.name).exists(),
                editable: true,
                note:
                    "PHP 主配置；页面「PHP 参数」中的 memory_limit 等四项写在 FPM 池，优先级更高。"
                        .into(),
                warnings: Vec::new(),
            },
            PhpConfigFile {
                key: "www.conf".into(),
                label: "www.conf（FPM 池）".into(),
                host_path: s
                    .runtime_pool_conf(&rt.kind, &rt.name)
                    .display()
                    .to_string(),
                container_path: CONTAINER_PHP_POOL.into(),
                exists: s.runtime_pool_conf(&rt.kind, &rt.name).exists(),
                editable: true,
                note: "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。"
                    .into(),
                warnings: Vec::new(),
            },
        ];
        let confd = s.runtime_php_confd(&rt.kind, &rt.name);
        if let Ok(entries) = std::fs::read_dir(&confd) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".ini") {
                    continue;
                }
                files.push(PhpConfigFile {
                    key: format!("conf.d/{name}"),
                    label: format!("扩展 {name}"),
                    host_path: entry.path().display().to_string(),
                    container_path: format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    exists: true,
                    editable: true,
                    note: "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。".into(),
                    warnings: Vec::new(),
                });
            }
        }
        Ok(files)
    }

    /// 读一个可用编辑器打开的配置文件。
    pub fn read_php_config_file(
        &self,
        id: &str,
        key: &str,
        actor: &str,
    ) -> Result<PhpConfigFileView, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        let content = if path.exists() {
            self.admin_fs()
                .read(actor, &path.display().to_string())
                .map_err(|e| SiteError::Internal(format!("读取 {key} 失败：{e}")))?
                .text
        } else {
            String::new()
        };
        Ok(PhpConfigFileView {
            file: meta,
            content,
        })
    }

    /// 保存配置文件：先写候选 → 容器内语法校验 → 原子落盘 → 重启 FPM；失败回滚。
    pub async fn save_php_config_file(
        &self,
        id: &str,
        key: &str,
        content: &str,
        actor: &str,
    ) -> Result<PhpConfigFile, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        // 旧实例可能还没有宿主托管的 php/ 目录：先补齐，否则候选文件无处落盘。
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)
            .map_err(|e| SiteError::Internal(format!("准备 PHP 配置目录失败：{e}")))?;
        if content.len() > 256 * 1024 {
            return Err(SiteError::BadRequest(
                "配置文件超过 256 KiB 编辑上限".into(),
            ));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        let running = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器状态失败：{e}")))?
            .iter()
            .any(|c| c.name == container && c.state == "running");
        if !running {
            return Err(SiteError::Unavailable(
                "运行环境未在运行，无法在容器内校验配置；请先启动实例".into(),
            ));
        }
        // 候选文件放在 web_root 内，容器内可直接读到同一个文件。
        let candidate = s
            .runtime_php_dir(&rt.kind, &rt.name)
            .join(format!("{key}.candidate"));
        self.admin_fs()
            .write(actor, &candidate.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入候选文件失败：{e}")))?;
        let candidate_container = s.container_path(&candidate);
        let (code, out) = match key {
            "www.conf" => {
                // php-fpm -t 只接受主配置；用临时主配置 include 候选池文件。
                let main = s
                    .runtime_php_dir(&rt.kind, &rt.name)
                    .join("php-fpm-test.conf");
                let main_text = format!(
                    "[global]\n\
                     error_log = /dev/stderr\n\
                     include = {candidate_container}\n"
                );
                self.admin_fs()
                    .write(actor, &main.display().to_string(), &main_text)
                    .map_err(|e| SiteError::Internal(format!("写入校验主配置失败：{e}")))?;
                let main_container = s.container_path(&main);
                self.docker()
                    .exec_capture(
                        &container,
                        vec![
                            "/usr/local/sbin/php-fpm".into(),
                            "-t".into(),
                            "-y".into(),
                            main_container,
                        ],
                    )
                    .await
                    .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?
            }
            _ => self
                .docker()
                .exec_capture(
                    &container,
                    vec![
                        "php".into(),
                        "-c".into(),
                        candidate_container,
                        "-r".into(),
                        "echo \"skiff-ok\";".into(),
                    ],
                )
                .await
                .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?,
        };
        // 只拦语法错误与启动失败：退出码非 0，或（非池配置）没跑进 `-r` 就退出，
        // 或输出里出现 `syntax error`。最后一条不能省：PHP 的 ini 语法错误只打印到
        // stdout（`PHP:  syntax error, unexpected ...`），进程仍会执行 `-r` 并返回 0，
        // 只看退出码/`skiff-ok` 会把坏配置放过去（上机 FS 验收复现）。
        // `PHP Warning` 不阻止保存，但要收进 warnings 原文回显给用户。
        let invalid = code != 0
            || php_output_has_syntax_error(&out)
            || (key != "www.conf" && !out.contains("skiff-ok"));
        if invalid {
            let reason = out.trim();
            return Err(SiteError::BadRequest(if reason.is_empty() {
                "配置语法校验失败，已拒绝保存".into()
            } else {
                format!("配置语法校验失败，已拒绝保存：{reason}")
            }));
        }
        let warnings = php_validation_warnings(&out);
        let original = std::fs::read(&path).ok();
        self.admin_fs()
            .write(actor, &path.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入 {key} 失败：{e}")))?;
        // 重启后必须复核容器仍在 running：Docker restart 返回成功不代表 FPM
        // 没被坏配置带崩（与 nginx 主配置保存同一条静默路径）。
        let problem = match self.docker_runtime_action(&container, Verb::Restart).await {
            Err(e) => Some(format!("{e}")),
            Ok(()) => {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                match self.docker().list_containers().await {
                    Ok(rows)
                        if rows
                            .iter()
                            .any(|c| c.name == container && c.state == "running") =>
                    {
                        None
                    }
                    Ok(_) => {
                        Some("容器重启后未保持 running（配置可能导致 FPM 启动失败）".to_string())
                    }
                    Err(e) => Some(format!("重启后核对容器状态失败：{e}")),
                }
            }
        };
        if let Some(reason) = problem {
            match &original {
                Some(bytes) if !bytes.is_empty() => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        if let Err(we) =
                            self.admin_fs()
                                .write(actor, &path.display().to_string(), text)
                        {
                            tracing::error!(path = %path.display(), error = %we, "回滚站点配置写入失败");
                        }
                    }
                }
                _ => {
                    if let Err(e) = self.admin_fs().remove(actor, &path.display().to_string()) {
                        tracing::debug!(path = %path.display(), error = %e, "回滚新增的 PHP 配置文件失败");
                    }
                }
            }
            if let Err(e) = self.docker_runtime_action(&container, Verb::Restart).await {
                tracing::error!(error = %e, runtime = %container, "回滚 PHP 配置后重启运行环境失败");
            }
            return Err(SiteError::Unavailable(format!(
                "保存后重启 PHP-FPM 失败，已回滚 {key}：{reason}"
            )));
        }
        Ok(PhpConfigFile {
            exists: true,
            warnings,
            ..meta
        })
    }

    /// 读取容器内 `php -i` 的实际生效值，并与宿主 `php.ini` 文件值并排返回。
    ///
    /// 只在用户点「刷新生效值」时调用，不在页面渲染路径执行容器命令。
    pub async fn php_effective_values(&self, id: &str) -> Result<Vec<PhpIniValue>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let container = runtime_container_name(&rt.kind, &rt.name);
        let (code, out) = self
            .docker()
            .exec_capture(&container, vec!["php".into(), "-i".into()])
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取 php -i 失败：{e}")))?;
        if code != 0 {
            return Err(SiteError::Unavailable(format!(
                "php -i 退出码 {code}，无法读取生效值"
            )));
        }
        let effective = parse_php_i(&out);
        let file_text =
            std::fs::read_to_string(s.runtime_php_ini(&rt.kind, &rt.name)).unwrap_or_default();
        let file_values = parse_ini_assignments(&file_text);
        let managed = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
        ];
        let keys = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
            "display_errors",
            "error_reporting",
            "date.timezone",
            "opcache.enable",
        ];
        Ok(keys
            .iter()
            .map(|key| {
                let eff = effective
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（未读取）".into());
                let file = file_values
                    .iter()
                    .rev()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（文件未设置）".into());
                let source = if managed.contains(key) && file != "（文件未设置）" {
                    "FPM 池 php_admin_value 优先；文件值仅在其他 SAPI 生效".to_string()
                } else if file != "（文件未设置）" && file == eff {
                    "php.ini".to_string()
                } else if file != "（文件未设置）" {
                    "其他 conf.d / 镜像默认覆盖".to_string()
                } else {
                    "镜像默认或其他 conf.d".to_string()
                };
                PhpIniValue {
                    key: key.to_string(),
                    file_value: file,
                    effective: eff,
                    source,
                }
            })
            .collect())
    }

    fn php_runtime(&self, id: &str) -> Result<Runtime, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境没有 PHP 专属配置",
                rt.kind
            )));
        }
        Ok(rt)
    }

    /// 把配置 key 解析成宿主路径；只允许固定两项与 `conf.d/<name>.ini` 单层文件名。
    fn php_config_target(
        &self,
        rt: &Runtime,
        s: &SiteSettings,
        key: &str,
    ) -> Result<(PathBuf, PhpConfigFile), SiteError> {
        let (path, container_path, label, note) = match key {
            "php.ini" => (
                s.runtime_php_ini(&rt.kind, &rt.name),
                CONTAINER_PHP_INI.to_string(),
                "php.ini".to_string(),
                "PHP 主配置；页面「PHP 参数」中的四项写在 FPM 池，优先级更高。",
            ),
            "www.conf" => (
                s.runtime_pool_conf(&rt.kind, &rt.name),
                CONTAINER_PHP_POOL.to_string(),
                "www.conf（FPM 池）".to_string(),
                "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。",
            ),
            other => {
                let name = other
                    .strip_prefix("conf.d/")
                    .ok_or_else(|| SiteError::BadRequest(format!("未知配置文件：{other}")))?;
                let p = Path::new(name);
                if name.is_empty()
                    || p.components().count() != 1
                    || !name.ends_with(".ini")
                    || name.contains('\\')
                {
                    return Err(SiteError::BadRequest("扩展 ini 文件名不合法".into()));
                }
                (
                    s.runtime_php_confd(&rt.kind, &rt.name).join(name),
                    format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    format!("扩展 {name}"),
                    "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。",
                )
            }
        };
        Ok((
            path.clone(),
            PhpConfigFile {
                key: key.to_string(),
                label,
                host_path: path.display().to_string(),
                container_path,
                exists: path.exists(),
                editable: true,
                note: note.to_string(),
                warnings: Vec::new(),
            },
        ))
    }

    pub async fn unbind_runtime_site(
        &self,
        runtime_id: &str,
        domain: &str,
    ) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if site.runtime_id.as_deref() != Some(runtime_id) {
            return Err(SiteError::BadRequest(format!(
                "站点 {domain} 未绑定到该运行环境"
            )));
        }
        let s = self.site_settings();
        let site_dir = PathBuf::from(&site.root_dir);
        let vhost = s.vhost_file(&site_dir, domain);
        let disabled = s.vhost_disabled(&site_dir, domain);
        if vhost.exists() {
            std::fs::rename(&vhost, &disabled)
                .map_err(|e| SiteError::Internal(format!("停用 vhost 失败：{e}")))?;
        }
        self.db()
            .set_site_static_unbound(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // 停用 vhost 后重载 nginx；容器不在运行则留待下次启动加载。
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows
                .iter()
                .find(|c| c.name == NGINX_CONTAINER && c.state == "running")
            {
                if let Err(e) = self
                    .docker()
                    .exec_capture(&c.id, vec!["nginx".into(), "-s".into(), "reload".into()])
                    .await
                {
                    tracing::warn!(container = %c.name, error = %e, "nginx -s reload 失败");
                }
            }
        }
        Ok(())
    }

    /// 删除运行环境：有站点绑定时拒绝并列出域名，绝不隐式解绑/级联删除。
    ///
    /// 真正清理的顺序：容器 stop → remove → 运行目录（含 FPM 池配置、socket、
    /// 日志与 Rust/Node 工程）→ DB 行。任一步失败就停下并保留 DB 行，错误里列出
    /// 已完成项，避免「记录删了、容器和目录还在」的静默残留。
    /// 删除不区分 kind：Go 只是未实现安装，已登记的运行环境仍可删除。
    pub async fn delete_site_runtime(&self, id: &str) -> Result<RuntimeDeleteReport, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let domains = self
            .db()
            .runtime_site_domains(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        if !domains.is_empty() {
            return Err(SiteError::Conflict(format!(
                "运行环境仍被以下站点绑定：{}；请先删除这些站点后再删除运行环境",
                domains.join("、")
            )));
        }
        if self.runtime_job(id).await.is_some_and(|j| j.active()) {
            return Err(SiteError::Conflict(
                "该运行环境有进行中的任务，请等待任务结束后再删除".into(),
            ));
        }

        let s = self.site_settings();
        let container = runtime_container_name(&rt.kind, &rt.name);
        let mut report = RuntimeDeleteReport::default();

        if self.docker().available() {
            let rows = self.docker().list_containers().await.map_err(|e| {
                partial_delete_error(
                    &report.completed,
                    format!("读取容器列表失败，未清理任何容器：{e}"),
                )
            })?;
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                let obj = skiff_docker::ObjectRef {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    labels: c.labels.clone(),
                };
                if matches!(c.state.as_str(), "running" | "restarting" | "paused") {
                    self.kernel_execute(Verb::Stop, Some(&obj), DockerParams::None)
                        .await
                        .map_err(|e| {
                            partial_delete_error(
                                &report.completed,
                                format!("停止容器 {container} 失败：{e}"),
                            )
                        })?;
                    report.completed.push(format!("容器 {container} 已停止"));
                }
                self.kernel_execute(
                    Verb::ContainerRemove,
                    Some(&obj),
                    DockerParams::ContainerRemove { force: true },
                )
                .await
                .map_err(|e| {
                    partial_delete_error(
                        &report.completed,
                        format!("删除容器 {container} 失败：{e}"),
                    )
                })?;
                report.completed.push(format!("容器 {container} 已删除"));
            } else {
                report
                    .completed
                    .push(format!("容器 {container} 不存在（无需删除）"));
            }
        } else {
            // Docker 不可用无法确认容器是否存在；不阻塞目录与记录清理，但明确告知。
            report.skipped.push(format!(
                "Docker 不可用（{}），未确认/清理容器 {container}",
                self.docker_unavailable_reason()
            ));
        }

        let dir = s.runtime_home(&rt.kind, &rt.name);
        if dir.exists() {
            remove_dir_all_retry(&dir).map_err(|e| {
                partial_delete_error(
                    &report.completed,
                    format!("删除运行环境目录 {} 失败：{e}", dir.display()),
                )
            })?;
            report.completed.push(format!(
                "运行环境目录已删除（含 FPM 池配置与 socket）：{}",
                dir.display()
            ));
        } else {
            report
                .completed
                .push("运行环境目录不存在（无需删除）".into());
        }

        self.db().delete_runtime(id).map_err(|e| {
            partial_delete_error(&report.completed, format!("删除运行环境记录失败：{e}"))
        })?;
        report.completed.push("数据库记录已删除".into());
        Ok(report)
    }

    /// 运行环境日志：PHP-FPM 走容器日志（stdout/stderr）；Rust 读宿主日志文件。
    pub async fn runtime_logs(&self, id: &str, tail: usize) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if matches!(
            rt.kind.as_str(),
            skiff_store::KIND_RUST | skiff_store::KIND_NODE
        ) {
            let s = self.site_settings();
            let path = s.runtime_log_dir(&rt.kind, &rt.name).join("server.log");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(tail);
            return Ok(lines[start..].join("\n"));
        }
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Ok("（Go 运行环境尚未接入）".into());
        }
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == runtime_container_name(&rt.kind, &rt.name))
            .ok_or_else(|| {
                SiteError::Unavailable("PHP-FPM 容器未安装，请先在运行环境页安装".into())
            })?;
        self.docker()
            .logs(&c.id, tail)
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取 PHP-FPM 日志失败：{e}")))
    }

    /// 用户主动点击「实测版本」时，才进容器执行一次；2 秒超时。
    /// 结果只代表当前运行中的容器，不写缓存、不参与列表渲染。
    pub async fn probe_runtime_version(&self, id: &str) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == runtime_container_name(&rt.kind, &rt.name))
            .filter(|c| c.state == "running")
            .ok_or_else(|| {
                SiteError::Unavailable("容器未运行，无法实测版本（镜像元数据仍可显示）".into())
            })?;
        let (cmd, strip): (Vec<String>, &str) = match rt.kind.as_str() {
            skiff_store::KIND_PHP => (
                vec!["php".into(), "-r".into(), "echo PHP_VERSION;".into()],
                "",
            ),
            skiff_store::KIND_RUST => (vec!["rustc".into(), "--version".into()], "rustc "),
            skiff_store::KIND_NODE => (vec!["node".into(), "-v".into()], ""),
            _ => (vec!["go".into(), "version".into()], ""),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.docker().exec_capture(&c.id, cmd),
        )
        .await
        .map_err(|_| SiteError::Unavailable("2 秒内未取到实测版本（容器可能繁忙）".into()))?
        .map_err(|e| SiteError::Unavailable(format!("实测版本失败：{e}")))?;
        let (code, out) = result;
        if code != 0 {
            return Err(SiteError::Unavailable(format!(
                "实测版本失败（exit {code}）：{}",
                out.lines().next().unwrap_or("")
            )));
        }
        let value = out
            .trim()
            .strip_prefix(strip)
            .unwrap_or(out.trim())
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if value.is_empty() {
            return Err(SiteError::Unavailable("实测版本输出为空".into()));
        }
        Ok(value)
    }
}

/// 运行环境列表/详情的展示模型（业务视角，容器状态只用于漂移提示）。
#[derive(Debug, Clone)]
pub struct RuntimeView {
    pub runtime: Runtime,
    /// running / stopped / error / missing / unimplemented
    pub effective_state: String,
    pub implemented: bool,
    pub container_name: String,
    pub container_state: String,
    /// 期望运行但容器未运行。
    pub drift: bool,
    pub site_count: usize,
    pub socket_host: String,
    pub socket_container: String,
    pub socket_exists: bool,
    pub socket_mode: String,
    /// 从镜像元数据读到的真实版本（如 `8.5.12`）；读不到为 None。
    pub actual_version: Option<String>,
    /// 容器实际镜像 id（`sha256:...`）；容器不存在为空。
    pub image_id: String,
    /// 登记镜像与容器实际镜像引用不一致（只提示，不自动修）。
    pub image_mismatch: bool,
    /// 安装时记录的 manifest digest（可能为空）。
    pub image_digest: String,
    pub container_exists: bool,
    /// 已启用的扩展 key（catalog 顺序）。
    pub extensions: Vec<String>,
    /// 最近一次异步任务（安装/扩展变更）；无任务时为 None。
    pub job: Option<RuntimeJob>,
}

impl RuntimeView {
    /// 列表是否展示该行：用户实例始终展示；种子行只在已安装或被站点引用时展示。
    pub fn is_listed(&self) -> bool {
        self.runtime.origin != skiff_store::ORIGIN_SEED
            || self.container_exists
            || self.site_count > 0
    }
}

fn socket_mode(path: &str) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return format!("{:04o}", meta.permissions().mode() & 0o777);
        }
    }
    let _ = path;
    "—".into()
}

// `render_php_pool_with` / `render_php_pool` 已抽到 `sites_vhost`，见文件上方再导出。

// ---------------------------------------------------------------------------
// AppState 上的网站服务
// ---------------------------------------------------------------------------

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
    async fn verify_php_image_identity(&self, image: &str) -> Result<(), String> {
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
    async fn remove_existing_container(&self, c: &ContainerRow) -> Result<(), String> {
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
    async fn secure_php_socket(
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
    fn render_vhost_for(
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
                std::fs::write(conf, content)
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

    pub async fn restart_nginx(&self) -> Result<(), SiteError> {
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
            cert_exists: cert.is_file() && key.is_file(),
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
        let mut san = format!("DNS:{}", site.domain);
        for a in &aliases {
            san.push_str(&format!(",DNS:{a}"));
        }
        let (cert2, key2) = (cert.clone(), key.clone());
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
            return Err(SiteError::Internal(format!(
                "生成自签证书失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        restrict_mode(&cert, 0o644);
        restrict_mode(&key, 0o600);
        self.apply_tls(domain, true, false).await
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
        let tmp_cert = dir.join("cert.pem.tmp");
        let tmp_key = dir.join("key.pem.tmp");
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
        std::fs::rename(&tmp_cert, &cert)
            .map_err(|e| SiteError::Internal(format!("保存证书失败：{e}")))?;
        std::fs::rename(&tmp_key, &key)
            .map_err(|e| SiteError::Internal(format!("保存私钥失败：{e}")))?;
        restrict_mode(&cert, 0o644);
        restrict_mode(&key, 0o600);
        self.apply_tls(domain, true, false).await
    }

    /// 关闭 HTTPS（保留证书文件）。
    pub async fn disable_tls(&self, domain: &str) -> Result<(), SiteError> {
        self.apply_tls(domain, false, false).await
    }

    /// 删除证书文件并关闭 HTTPS。
    pub async fn delete_cert(&self, domain: &str) -> Result<(), SiteError> {
        self.apply_tls(domain, false, false).await?;
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
        // 清理上传时的临时文件，避免半套。
        for name in ["cert.pem.tmp", "key.pem.tmp"] {
            let p = dir.join(name);
            if p.exists() {
                if let Err(e) = std::fs::remove_file(&p) {
                    tracing::debug!(path = %p.display(), error = %e, "清理上次残留的临时证书失败");
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

    /// 改写站点 vhost + 默认站 vhost，校验 `nginx -t` 后重启；任一步失败回滚文件与库状态。
    async fn apply_tls(
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
        if let Err(e) = self.validate_nginx().await {
            // 校验失败时运行中的 nginx 还没加载新配置，恢复文件即可；
            // 这里**不能再重启**——若失败源于别的坏 conf，重启会让现网 nginx 起不来。
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(e);
        }
        if let Err(e) = self.restart_nginx().await {
            restore(&snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            if let Err(re) = self.restart_nginx().await {
                tracing::error!(domain, error = %re, "回滚后重启 nginx 失败，现网可能仍加载失败配置");
            }
            return Err(e);
        }
        Ok(())
    }

    /// 按库里的最新状态重写单个站点 vhost（运行中写 .conf，停止写 .conf.disabled）。
    fn write_site_vhost(&self, s: &SiteSettings, domain: &str) -> Result<(), SiteError> {
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

    /// 设为/取消默认站点。库是状态源，conf 是投影：文件写成功并 reload 后才落库。
    pub async fn set_default_site(&self, domain: &str, enabled: bool) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let conf = s.conf_dir.join(DEFAULT_CONF);

        if enabled {
            if site.status != "running" {
                return Err(SiteError::BadRequest(
                    "站点已停止，请先启动再设为默认".into(),
                ));
            }
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
            let tls = self.site_tls(&s, &site);
            let content = self.render_default_vhost_for(&s, &site, tls.as_ref())?;
            std::fs::write(&conf, content)
                .map_err(|e| SiteError::Internal(format!("写入默认站点配置失败：{e}")))?;
            if let Err(e) = self.restart_nginx().await {
                // 重载失败则回滚文件，避免留下未生效的默认指向。
                if let Err(re) = std::fs::remove_file(&conf) {
                    tracing::error!(path = %conf.display(), error = %re, "回滚默认站点配置失败");
                }
                return Err(e);
            }
            self.db()
                .set_site_default(domain, true)
                .map_err(|e| SiteError::Internal(e.to_string()))?;
        } else {
            if !site.is_default {
                return Ok(());
            }
            if conf.exists() {
                std::fs::remove_file(&conf)
                    .map_err(|e| SiteError::Internal(format!("删除默认站点配置失败：{e}")))?;
            }
            self.restart_nginx().await?;
            self.db()
                .set_site_default(domain, false)
                .map_err(|e| SiteError::Internal(e.to_string()))?;
        }
        Ok(())
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

    /// 保存 vhost：先隔离校验候选内容，通过后才落盘；运行中的站点再重载，
    /// 停止的站点只落盘不重载（`.conf.disabled` 不在线上 include 内）。
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
        // 先写临时文件再 rename，避免半截文件被 nginx 加载。
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, content)
            .map_err(|e| SiteError::Internal(format!("写入临时配置失败：{e}")))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            if let Err(re) = std::fs::remove_file(&tmp) {
                tracing::debug!(path = %tmp.display(), error = %re, "清理临时配置失败");
            }
            return Err(SiteError::Internal(format!("替换配置失败：{e}")));
        }
        if !active {
            return Ok(VhostSaveOutcome { reloaded: false });
        }
        if let Err(e) = self.restart_nginx().await {
            // 回滚文件内容，保证磁盘与正在运行的 nginx 保持一致。
            if let Err(re) = std::fs::write(&path, original) {
                tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚到保存前内容：重载 nginx 失败：{e}"
            )));
        }
        // Docker restart 返回成功不代表 nginx 配置有效：容器可能在启动后立即退出。
        // 留出极短启动窗口再核对状态，失败则回滚并尽量把旧配置重新拉起。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let env = self.site_env_status().await;
        if env.installed && env.nginx_state != "running" {
            if let Err(re) = std::fs::write(&path, original) {
                tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
            }
            if let Err(re) = self.restart_nginx().await {
                tracing::error!(error = %re, "回滚后重载 nginx 失败");
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚到保存前内容：nginx 未能保持运行（当前状态 {}）",
                env.nginx_state
            )));
        }
        Ok(VhostSaveOutcome { reloaded: true })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn php_init_script_resolves_nobody_from_gid_not_family_name() {
        // Debian 的 65534 组名是 nogroup；脚本必须建 `nobody` 同 gid 别名，
        // 而不是按镜像家族写死组名（自建镜像同样适用）。
        assert!(PHP_ACCOUNT_INIT_SCRIPT.contains(&format!("SOCKET_GID={SOCKET_GID}")));
        let block = PHP_ACCOUNT_INIT_SCRIPT
            .split("池模板固定写 listen.group = nobody")
            .nth(1)
            .expect("初始化脚本必须包含 nobody 组别名段");
        // Debian（groupadd）与 Alpine（addgroup）各自有兜底，最后还有 /etc/group 直写；
        // 两条路径都只依赖 gid 65534，不依赖某个组名恰好存在。
        assert!(block.contains("groupadd -o -g \"$SOCKET_GID\" nobody"));
        assert!(block.contains("addgroup -g \"$SOCKET_GID\" -S nobody"));
        assert!(block.contains("'nobody:x:%s:\\n'"));
        // 别名段本身不得再按镜像家族分支：同一段逻辑覆盖两种家族与自建镜像。
        assert!(!block.contains("alpine-release"));
        assert!(!block.contains("debian_version"));
        // 模板不出现 nogroup：Debian 的组名细节只由初始化脚本用 gid 兜住。
        assert!(!block.contains("listen.group = nogroup"));
    }

    #[test]
    fn php_ini_scan_dir_lists_each_directory_once() {
        // 前导冒号在 PHP 里表示「沿用编译期默认扫描目录」；官方镜像的默认目录
        // 就是 conf.d，一旦保留就会重复扫描并让扩展 ini 加载两次。
        assert!(
            !PHP_INI_SCAN_DIR_VALUE.starts_with(':'),
            "前导冒号会重新引入默认目录，导致 conf.d 被扫两次：{PHP_INI_SCAN_DIR_VALUE}"
        );
        let dirs: Vec<&str> = PHP_INI_SCAN_DIR_VALUE.split(':').collect();
        assert_eq!(
            dirs,
            vec![CONTAINER_PHP_CONFD, CONTAINER_PHP_SKIFF_CONFD],
            "必须显式列出 conf.d 与 conf.d-skiff，且顺序为镜像目录在前"
        );
        // 空段等价于默认目录，同样会造成重复。
        assert!(
            dirs.iter().all(|d| !d.is_empty()),
            "扫描目录不能有空段：{dirs:?}"
        );
        let mut unique = dirs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), dirs.len(), "扫描目录不得重复：{dirs:?}");

        // 容器环境变量必须与常量一致，避免测试与真实 spec 分叉。
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = settings_at(dir.path());
        let php = state.php_spec_for(&s, "php", "scan1", "php:8.5-fpm-alpine");
        assert_eq!(
            php.env.get("PHP_INI_SCAN_DIR").map(String::as_str),
            Some(PHP_INI_SCAN_DIR_VALUE)
        );
    }

    #[test]
    fn timeout_message_never_renders_zero_minutes() {
        assert_eq!(fmt_timeout(std::time::Duration::from_secs(3)), "3 秒");
        assert_eq!(fmt_timeout(std::time::Duration::from_secs(90)), "90 秒");
        assert_eq!(
            fmt_timeout(std::time::Duration::from_secs(15 * 60)),
            "15 分钟"
        );
    }

    /// 证书解析缓存只按 `(mtime, size)` 命中：文件改写后必须重新解析，
    /// 否则换证后 SSL 标签会一直显示旧主体/旧到期时间。
    #[test]
    fn tls_cert_text_cache_invalidates_on_file_change() {
        let dir = std::env::temp_dir().join(format!("skiff-tls-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        std::fs::write(&cert, b"first").expect("写证书");
        std::fs::write(&key, b"k1").expect("写私钥");

        let (cert_stamp, key_stamp) = (file_stamp(&cert), file_stamp(&key));
        tls_info_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                cert.clone(),
                TlsInfoCacheEntry {
                    cert: cert_stamp,
                    key: key_stamp,
                    text: TlsCertText {
                        subject: Some("CN=old".into()),
                        issuer: None,
                        expires: None,
                    },
                },
            );

        // 文件未变：命中缓存，不跑 openssl。
        assert_eq!(
            cached_cert_text(&cert, &key).subject.as_deref(),
            Some("CN=old")
        );

        // 证书被改写（大小变化）：指纹失效，必须重新解析。
        std::fs::write(&cert, b"second-certificate").expect("覆写证书");
        assert_ne!(
            cached_cert_text(&cert, &key).subject.as_deref(),
            Some("CN=old"),
            "文件变化后不得继续返回旧解析结果"
        );

        // 站点/证书删除：条目不残留（`site_tls_info` 的不存在分支会这样清）。
        std::fs::remove_file(&cert).expect("删除证书");
        tls_info_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&cert);
        assert!(!tls_info_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&cert));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn runtime_container_state_distinguishes_unknown_missing_and_stopped() {
        // Docker 不可达/列表失败：状态未知，不能伪造成 missing 或 running。
        assert_eq!(probe_container_state(false, None), "unknown");
        assert_eq!(probe_container_state(false, Some("running")), "unknown");
        // Docker 可查但列表里没有该容器：确实没安装。
        assert_eq!(probe_container_state(true, None), "missing");
        // 容器存在但已退出/终止：归一成 stopped，界面显示「已停止」而不是「运行中」或「未知」。
        for raw in [
            "exited",
            "dead",
            "created",
            "paused",
            "restarting",
            "stopped",
        ] {
            assert_eq!(runtime_effective_state(raw), "stopped", "{raw}");
        }
        assert_eq!(runtime_effective_state("running"), "running");
        assert_eq!(runtime_effective_state(""), "unknown");
        assert_eq!(runtime_effective_state("weird-state"), "unknown");
    }

    #[test]
    fn runtime_site_defaults_follow_runtime_kind() {
        // PHP 站点：探针 index.php。
        let php = runtime_site_default_files(skiff_store::KIND_PHP, "8.5", None);
        assert_eq!(php.len(), 1);
        assert_eq!(php[0].0, "index.php");
        // 方案 A：Node.js / Rust 的代码固定在环境 `app/`，站点只是域名入口，不写脚手架。
        assert!(runtime_site_default_files(skiff_store::KIND_RUST, "1.83", Some(9101)).is_empty());
        assert!(runtime_site_default_files(skiff_store::KIND_NODE, "22", Some(9200)).is_empty());
        // 未接入的 Go 同样不给默认文件，避免伪造可用性。
        assert!(runtime_site_default_files("go", "1.24", None).is_empty());
    }

    #[test]
    fn node_entry_script_uses_fixed_app_dir() {
        // 与 PHP 不同：Node 入口脚本固定进入 `app/`，不读绑定站点标记，也不接 FastCGI。
        let script = node_entry_script();
        assert!(script.contains("APP=\"$DIR/app\""));
        assert!(
            !script.contains("$DIR/site"),
            "不得再按绑定站点切换源码目录"
        );
        assert!(script.contains("package.json"));
        assert!(script.contains("exec \"$@\""));
        assert!(!script.contains("php-fpm"));
    }

    #[test]
    fn node_config_defaults_are_single_process_contract() {
        let cfg = NodeRuntimeConfig::default();
        assert_eq!(cfg.command, "npm start");
        assert_eq!(cfg.health_path, "/");
        assert_eq!(cfg.node_env, "production");
        assert_eq!(cfg.argv().unwrap(), vec!["npm", "start"]);
        cfg.validate().expect("默认配置必须合法");
    }

    #[test]
    fn node_config_rejects_shell_metacharacters_and_reserved_env() {
        let cfg = NodeRuntimeConfig {
            command: "node server.js; rm -rf /www".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "分号必须被拒绝，不能进容器 shell");

        let cfg = NodeRuntimeConfig {
            command: "node server.js".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.argv().unwrap(), vec!["node", "server.js"]);

        let cfg = NodeRuntimeConfig {
            env: vec![("PORT".into(), "3000".into())],
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "PORT 由内核注入，不能覆盖");

        let cfg = NodeRuntimeConfig {
            env: vec![("lower".into(), "1".into())],
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "环境变量名必须大写");

        let cfg = NodeRuntimeConfig {
            health_path: "health".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "健康检查路径必须以 / 开头");
    }

    #[test]
    fn node_config_env_text_roundtrip() {
        let mut cfg = NodeRuntimeConfig::default();
        cfg.set_env_text("# 注释\nAPI_BASE=https://api.example.com\nFEATURE_X=1\n\n")
            .unwrap();
        assert_eq!(cfg.env.len(), 2);
        assert_eq!(
            cfg.env_text(),
            "API_BASE=https://api.example.com\nFEATURE_X=1"
        );
        let json = cfg.to_json();
        let back = NodeRuntimeConfig::from_json(&json);
        assert_eq!(back, cfg);
        assert!(cfg.set_env_text("NOPE").is_err(), "缺 = 必须报错");
        assert!(cfg.set_env_text("A=1\nA=2").is_err(), "重复键必须报错");
    }

    #[test]
    fn rust_entry_script_uses_fixed_app_dir() {
        // 源码目录固定为环境 `app/`：不再有“绑定站点即源码目录”的切换。
        let script = rust_entry_script();
        assert!(script.contains("APP=\"$DIR/app\""));
        assert!(!script.contains("$DIR/site"));
        assert!(script.contains("--manifest-path"));
    }

    #[test]
    fn process_port_must_stay_in_kind_range_and_be_free() {
        // 端口属于环境：必须落在该 kind 的区间内，且不能被其它环境占用。
        assert!(validate_process_port(skiff_store::KIND_NODE, 9200, &[]).is_ok());
        assert!(validate_process_port(skiff_store::KIND_NODE, 9199, &[]).is_err());
        assert!(validate_process_port(skiff_store::KIND_NODE, 9200, &[9200]).is_err());
        assert!(validate_process_port(skiff_store::KIND_RUST, 9100, &[9100]).is_err());
        // PHP 走 unix socket，不占宿主端口。
        assert!(validate_process_port(skiff_store::KIND_PHP, 9200, &[]).is_err());
    }

    #[test]
    fn clone_copy_skips_dependencies_and_keeps_sources() {
        // 克隆复制 `app/` 时必须跳过 node_modules / target，避免把依赖树复制一份。
        let base = std::env::temp_dir().join(format!("skiff-clone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let from = base.join("from/app");
        let to = base.join("to/app");
        std::fs::create_dir_all(from.join("src")).expect("mkdir src");
        std::fs::create_dir_all(from.join("node_modules/dep")).expect("mkdir modules");
        std::fs::write(from.join("package.json"), "{}").expect("write manifest");
        std::fs::write(from.join("src/main.js"), "x").expect("write source");
        std::fs::write(from.join("node_modules/dep/index.js"), "x").expect("write dep");
        copy_dir_skip(&from, &to, &["node_modules", "target"]).expect("copy");
        assert!(to.join("package.json").is_file());
        assert!(to.join("src/main.js").is_file());
        assert!(!to.join("node_modules").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn php_validation_warnings_only_collects_warning_lines() {
        // `PHP Warning` 不阻止保存，但必须原文回显；普通输出与错误行不混进 warnings。
        let out = "PHP Warning:  Module \"foo\" is already loaded in Unknown on line 0\n\
                   skiff-ok\n\
                   PHP Warning:  Module \"foo\" is already loaded in Unknown on line 0\n\
                   PHP Fatal error: Uncaught Exception\n";
        let warnings = php_validation_warnings(out);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("PHP Warning:"));
        assert!(!warnings[0].contains("skiff-ok"));
    }

    #[test]
    fn php_validation_rejects_invalid_output_even_when_exit_zero() {
        // PHP ini 语法错误只写到 stdout，且 `-r` 仍会执行打印标记得以返回 0；
        // 这种情况下必须判无效，否则坏配置会覆盖原文件。
        let out = "PHP:  syntax error, unexpected end of file in /probe/b.ini on line 2\nskiff-ok";
        assert!(php_output_has_syntax_error(out));
        let ok = "PHP Version => 8.5.0\nskiff-ok";
        assert!(!php_output_has_syntax_error(ok));
    }

    #[test]
    fn dir_usage_counts_nested_files_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("index/src")).unwrap();
        std::fs::write(dir.path().join("index/a.txt"), "12345").unwrap();
        std::fs::write(dir.path().join("index/src/main.rs"), "ok").unwrap();
        let (bytes, files) = dir_usage(dir.path());
        assert_eq!(files, 2);
        assert_eq!(bytes, 7);
    }

    #[test]
    fn domain_validation_rejects_unsafe_input() {
        assert_eq!(validate_domain("Example.COM").unwrap(), "example.com");
        assert_eq!(
            validate_domain("a-b.example.com").unwrap(),
            "a-b.example.com"
        );
        for bad in [
            "",
            "../etc",
            "a/b",
            "exa mple.com",
            "example",
            ".example.com",
            "example..com",
            "-example.com",
            "example-.com",
            "1.2.3.4",
            "under_score.com",
        ] {
            assert!(validate_domain(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn alias_batch_parsing_normalizes_and_rejects_duplicates() {
        // 一次粘贴多行/逗号/空格混合：统一拆分并小写归一。
        let ok = normalize_site_aliases(
            "example.com",
            &[
                "www.example.com, api.example.com".to_string(),
                "static.example.com\nAlt.Example.COM".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(
            ok,
            vec![
                "www.example.com",
                "api.example.com",
                "static.example.com",
                "alt.example.com"
            ]
        );
        // 重复别名明确拒绝，而不是静默去重。
        let dup = normalize_site_aliases(
            "example.com",
            &["www.example.com".to_string(), "WWW.example.com".to_string()],
        )
        .unwrap_err();
        assert!(
            matches!(&dup, SiteError::Conflict(m) if m.contains("域名重复")),
            "{dup:?}"
        );
        // 与主域名相同同样拒绝。
        assert!(normalize_site_aliases("example.com", &["example.com".to_string()]).is_err());
        // 空行/空值直接跳过。
        assert!(normalize_site_aliases("example.com", &[" , ".to_string()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn local_extension_installer_is_materialized_and_kept_in_sync() {
        let dir = std::env::temp_dir().join(format!(
            "skiff-ipe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = SiteSettings {
            web_root: dir.clone(),
            sites_dir: dir.join("sites"),
            conf_dir: dir.join("nginx").join("conf.d"),
            runtime_dir: dir.join("runtime"),
            data_dir: dir.join("data"),
            nginx_image: String::new(),
            php_image: String::new(),
            port: 80,
            registry_mirror: String::new(),
        }
        .php_extension_installer();
        write_if_changed(&path, b"# local v1\n", 0o755).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"# local v1\n");
        // 内容一致不重复写（mtime 不变）。
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        write_if_changed(&path, b"# local v1\n", 0o755).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        // 内容不同则原子替换。
        write_if_changed(&path, b"# local v2\n", 0o755).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"# local v2\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn site_root_defaults_to_domain_and_rejects_escape() {
        let dir = Path::new("/www/sites");
        // 空值 = 与主域名同名（前端自动填的兜底）。
        assert_eq!(
            site_root_for(dir, "example.com", None).unwrap(),
            dir.join("example.com")
        );
        assert_eq!(
            site_root_for(dir, "example.com", Some("  ")).unwrap(),
            dir.join("example.com")
        );
        assert_eq!(
            site_root_for(dir, "example.com", Some("blog")).unwrap(),
            dir.join("blog")
        );
        for bad in ["../etc", "a/b", "a b", "/abs", "..", "a..b"] {
            assert!(
                site_root_for(dir, "example.com", Some(bad)).is_err(),
                "{bad:?} 应被拒绝"
            );
        }
    }

    #[test]
    fn site_type_and_proxy_target_validation() {
        assert_eq!(validate_site_type("runtime").unwrap(), TYPE_RUNTIME);
        assert_eq!(validate_site_type("STATIC").unwrap(), TYPE_STATIC);
        assert_eq!(validate_site_type("proxy").unwrap(), TYPE_PROXY);
        assert!(validate_site_type("node").is_err());

        assert_eq!(
            validate_proxy_target("http://127.0.0.1:8080/").unwrap(),
            "http://127.0.0.1:8080"
        );
        for bad in [
            "",
            "127.0.0.1:8080",
            "ftp://x.example",
            "http://",
            "http://user:pw@host",
            "http://bad host",
            "http://host:0",
            "http://host:99999",
        ] {
            assert!(validate_proxy_target(bad).is_err(), "{bad:?} 应被拒绝");
        }
    }

    #[test]
    fn php_vhost_points_fastcgi_at_runtime_socket() {
        let v = render_site_vhost(
            "example.com",
            &[],
            VhostKind::Php {
                root: "/www/sites/example.com/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
            None,
        );
        assert!(v.contains("server_name example.com;"));
        assert!(v.contains("root /www/sites/example.com/index;"));
        assert!(v.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"));
        assert!(!v.contains("9000"));
        assert!(v.contains("SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
        // 主配置候选校验时配置目录会变成 /www/nginx，include 必须写绝对路径。
        assert!(v.contains(&format!("include {CONTAINER_FASTCGI_PARAMS};")));
        assert!(!v.contains("include fastcgi_params;"));
        assert!(v.contains("access_log /www/sites/example.com/log/access.log;"));
        assert!(v.contains("error_log /www/sites/example.com/log/error.log;"));
        assert!(!v.contains("/www/logs/"), "不得再引用旧的集中日志目录");
        assert!(!v.contains("/public"), "不得再引用旧的 public 根目录");
        assert!(!v.contains("listen 443"));
        assert!(v.contains("listen 80;"));
    }

    #[test]
    fn static_vhost_has_no_php_but_reads_index() {
        let v = render_site_vhost(
            "static.local",
            &[],
            VhostKind::Static {
                root: "/www/sites/static.local/index",
            },
            "/www/sites/static.local/log/access.log",
            "/www/sites/static.local/log/error.log",
            None,
        );
        assert!(v.contains("root /www/sites/static.local/index;"));
        assert!(v.contains("index index.html;"));
        assert!(v.contains("try_files $uri $uri/ =404;"));
        assert!(!v.contains("fastcgi_pass"), "静态站不得接 PHP");
        assert!(!v.contains("index.php"));
    }

    #[test]
    fn proxy_vhost_forwards_and_optionally_upgrades_websocket() {
        let ws = render_site_vhost(
            "proxy.local",
            &[],
            VhostKind::Proxy {
                target: "http://127.0.0.1:8080",
                websocket: true,
            },
            "/www/sites/proxy.local/log/access.log",
            "/www/sites/proxy.local/log/error.log",
            None,
        );
        assert!(ws.contains("proxy_pass http://127.0.0.1:8080;"));
        assert!(ws.contains("proxy_http_version 1.1;"));
        assert!(ws.contains("proxy_set_header Host $host;"));
        assert!(ws.contains("proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;"));
        assert!(ws.contains("proxy_set_header Upgrade $http_upgrade;"));
        assert!(ws.contains("proxy_set_header Connection \"upgrade\";"));
        assert!(!ws.contains("root "), "反代不落文档根");
        assert!(!ws.contains("fastcgi_pass"));

        let plain = render_site_vhost(
            "proxy.local",
            &[],
            VhostKind::Proxy {
                target: "https://api.example.com:8443/base",
                websocket: false,
            },
            "/www/sites/proxy.local/log/access.log",
            "/www/sites/proxy.local/log/error.log",
            None,
        );
        assert!(plain.contains("proxy_pass https://api.example.com:8443/base;"));
        assert!(!plain.contains("Upgrade $http_upgrade"));
    }

    #[test]
    fn php_config_validation_rejects_out_of_range_and_unsafe_values() {
        PhpRuntimeConfig::default().validate().unwrap();

        let cfg = PhpRuntimeConfig {
            memory_limit: "8M".into(),
            ..PhpRuntimeConfig::default()
        };
        assert!(cfg.validate().is_err(), "memory_limit 下限 32M");

        let cfg = PhpRuntimeConfig {
            memory_limit: "9999M".into(),
            ..PhpRuntimeConfig::default()
        };
        assert!(cfg.validate().is_err());

        let cfg = PhpRuntimeConfig {
            upload_max_filesize: "256M".into(),
            post_max_size: "64M".into(),
            ..PhpRuntimeConfig::default()
        };
        assert!(cfg.validate().is_err(), "post 不能小于 upload");

        let cfg = PhpRuntimeConfig {
            max_children: 100,
            start_servers: 120,
            ..PhpRuntimeConfig::default()
        };
        assert!(cfg.validate().is_err());

        let cfg = PhpRuntimeConfig {
            pm: "static".into(),
            ..PhpRuntimeConfig::default()
        };
        assert!(cfg.validate().is_err());

        // 合法配置可往返序列化，且不丢字段。
        let cfg = PhpRuntimeConfig {
            memory_limit: "512M".into(),
            upload_max_filesize: "128M".into(),
            post_max_size: "128M".into(),
            max_execution_time: 120,
            max_children: 40,
            start_servers: 5,
            min_spare_servers: 3,
            max_spare_servers: 12,
            max_requests: 800,
            ..PhpRuntimeConfig::default()
        };
        cfg.validate().unwrap();
        let back = PhpRuntimeConfig::from_json(&cfg.to_json());
        assert_eq!(back, cfg);
    }

    #[test]
    fn php_pool_uses_config_and_runtime_socket() {
        let cfg = PhpRuntimeConfig::default();
        let pool = render_php_pool_with(&cfg, "/www/runtime/php/php-8.5/php-fpm.sock");
        assert!(pool.contains("listen = /www/runtime/php/php-8.5/php-fpm.sock"));
        assert!(pool.contains("listen.mode = 0660"));
        assert!(pool.contains("pm = dynamic"));
        assert!(pool.contains("pm.max_children = 20"));
        assert!(pool.contains("php_admin_value[memory_limit] = 256M"));
        assert!(pool.contains("php_admin_value[upload_max_filesize] = 64M"));
        assert!(pool.contains("php_admin_value[max_execution_time] = 30"));
        assert!(
            pool.contains("access.log = /dev/null"),
            "必须覆盖镜像的 /proc/self/fd/2，避免每请求重复写访问日志"
        );
    }

    fn settings_at(root: &Path) -> SiteSettings {
        SiteSettings {
            web_root: root.to_path_buf(),
            sites_dir: root.join("sites"),
            conf_dir: root.join("nginx").join("conf.d"),
            runtime_dir: root.join("runtime"),
            data_dir: root.join("data"),
            nginx_image: DEFAULT_NGINX_IMAGE.into(),
            php_image: DEFAULT_PHP_IMAGE.into(),
            port: DEFAULT_PORT,
            registry_mirror: String::new(),
        }
    }

    /// 第（无 Docker）测试用应用上下文：数据目录落在临时目录，避免污染源码树。
    fn app_state_at(root: &Path) -> Arc<AppState> {
        crate::commands::test_support::state_at(root)
    }

    /// 内核自建容器（Web 服务 + 各运行环境）必须统一带保护标签，
    /// 否则面板能把自己的 nginx / PHP / Rust 容器停掉。
    #[test]
    fn kernel_container_specs_carry_protected_label() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = settings_at(dir.path());
        let nginx = state.nginx_spec(&s);
        let php = state.php_spec_for(&s, "php", "batch1", "php:8.5-fpm-alpine");
        let rust = state.rust_spec_for(&s, "rust-1.83", "rust:1.83", 18090);
        assert_eq!(nginx.name.as_deref(), Some(NGINX_CONTAINER));
        assert_eq!(php.name.as_deref(), Some("skiff-php-batch1"));
        assert_eq!(rust.name.as_deref(), Some("skiff-rust-rust-1.83"));
        for spec in [&nginx, &php, &rust] {
            assert_eq!(
                spec.labels
                    .get(skiff_model::LABEL_PROTECTED)
                    .map(String::as_str),
                Some("true"),
                "{} 缺 io.skiff.protected 标签",
                spec.name.as_deref().unwrap_or_default()
            );
            assert_eq!(
                spec.labels
                    .get(skiff_model::LABEL_PROJECT)
                    .map(String::as_str),
                Some("sites")
            );
        }
    }

    /// 删除容器前的 BusyGuard：运行环境有任务、Web 服务安装/重建中都应拒绝，
    /// 空闲时放行（正常删除路径）。
    #[tokio::test]
    async fn container_busy_reason_matches_kernel_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        state
            .db()
            .create_runtime(&skiff_store::NewRuntime {
                id: "php-busy".into(),
                kind: "php".into(),
                name: "busy".into(),
                version: "8.5".into(),
                image: "php:8.5-fpm-alpine".into(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
        let container = runtime_container_name("php", "busy");
        assert!(
            state.container_busy_reason(&container).await.is_none(),
            "空闲运行环境应允许删除"
        );
        state
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: "php-busy".into(),
                title: "安装运行环境 busy".into(),
                params: serde_json::json!({}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .unwrap();
        assert!(state
            .container_busy_reason(&container)
            .await
            .unwrap()
            .contains("进行中的任务"));
        assert!(
            state.container_busy_reason("someone-else").await.is_none(),
            "无关容器不受运行环境任务影响"
        );
        assert!(state.container_busy_reason(NGINX_CONTAINER).await.is_none());
        state.site_env.lock().await.running = true;
        assert!(state
            .container_busy_reason(NGINX_CONTAINER)
            .await
            .unwrap()
            .contains("Web 服务"));
    }

    #[test]
    fn vhost_check_main_isolates_candidate_from_live_include() {
        let text = vhost_check_main("/www/skiff/vhost-check/x.conf.candidate");
        assert!(text.contains("include /www/skiff/vhost-check/x.conf.candidate;"));
        // 绝不能把线上站点 include 拉进来，否则等于先写进线上再测。
        assert!(!text.contains("10-skiff-sites"));
        assert!(!text.contains("/www/sites/*/conf"));
    }

    #[test]
    fn vhost_check_failure_reports_line_and_diff() {
        let original = "server {\n    listen 80;\n    server_name a.local;\n}\n";
        let bad = "server {\n    listen 80;\n    server_name a.local;\n    this_is_invalid_directive_xyz;\n}\n";
        let out = "nginx: [emerg] unknown directive \"this_is_invalid_directive_xyz\" in /www/skiff/vhost-check/a.local-1.conf.candidate:5";
        let msg = format!(
            "站点配置校验未通过：{}；差异：{}",
            crate::nginx::error_line(out),
            crate::nginx::diff_summary(original, bad, 6)
        );
        assert!(msg.contains("unknown directive"), "{msg}");
        assert!(msg.contains("第 5 行"), "{msg}");
    }

    #[tokio::test]
    async fn stopped_site_save_does_not_persist_when_validation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        let site_dir = s.sites_dir.join("audit-env.local");
        std::fs::create_dir_all(site_conf_dir(&site_dir)).unwrap();
        let disabled = s.vhost_disabled(&site_dir, "audit-env.local");
        let original = "server {\n    listen 80;\n    server_name audit-env.local;\n}\n";
        std::fs::write(&disabled, original).unwrap();
        state
            .db()
            .create_site(&StoreNewSite {
                domain: "audit-env.local".into(),
                root_dir: site_dir.display().to_string(),
                site_type: TYPE_STATIC.into(),
                runtime_id: None,
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
        let bad = "server {\n    listen 80;\n    this_is_invalid_directive_xyz;\n}\n";
        let res = state.save_site_vhost("audit-env.local", bad).await;
        assert!(res.is_err(), "校验不可用/失败时必须返回错误");
        assert_eq!(
            std::fs::read_to_string(&disabled).unwrap(),
            original,
            "校验失败不得落盘"
        );
        assert!(!s.vhost_file(&site_dir, "audit-env.local").exists());
    }

    #[tokio::test]
    async fn start_site_does_not_activate_when_validation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        let site_dir = s.sites_dir.join("audit-env.local");
        std::fs::create_dir_all(site_conf_dir(&site_dir)).unwrap();
        let disabled = s.vhost_disabled(&site_dir, "audit-env.local");
        std::fs::write(
            &disabled,
            "server {\n    listen 80;\n    this_is_invalid_directive_xyz;\n}\n",
        )
        .unwrap();
        state
            .db()
            .create_site(&StoreNewSite {
                domain: "audit-env.local".into(),
                root_dir: site_dir.display().to_string(),
                site_type: TYPE_STATIC.into(),
                runtime_id: None,
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
        state
            .db()
            .set_site_status("audit-env.local", "stopped")
            .unwrap();
        let res = state.set_site_running("audit-env.local", true).await;
        assert!(res.is_err(), "非法配置必须拒绝启动");
        assert!(disabled.exists(), "拒绝启动后仍应保留停用态文件");
        assert!(
            !s.vhost_file(&site_dir, "audit-env.local").exists(),
            "拒绝启动不得把配置推进线上 include"
        );
        let site = state
            .db()
            .site_by_domain("audit-env.local")
            .unwrap()
            .unwrap();
        assert_eq!(site.status, "stopped", "拒绝启动不得改写运行状态");
    }

    #[test]
    fn runtime_site_start_guard_only_treats_php_as_socket_based() {
        let rt = |kind: &str, last_status: &str, socket: Option<&str>| Runtime {
            id: "probe".into(),
            kind: kind.into(),
            name: "probe".into(),
            version: "1.99".into(),
            image: String::new(),
            image_digest: String::new(),
            desired_state: "running".into(),
            last_status: last_status.into(),
            socket_path: socket.map(ToString::to_string),
            port: None,
            is_default: false,
            config_json: "{}".into(),
            extensions: String::new(),
            origin: "instance".into(),
            created_at: 0,
            updated_at: 0,
        };

        // Rust 运行环境没有 FPM socket，只要状态 running 就允许启动站点。
        ensure_runtime_ready_for_start(&rt(skiff_store::KIND_RUST, "running", None))
            .expect("运行中的 Rust 环境不得被 PHP socket 条件误拦");
        let stopped = ensure_runtime_ready_for_start(&rt(skiff_store::KIND_RUST, "stopped", None))
            .expect_err("停用的 Rust 环境必须拒绝启动");
        assert!(stopped.to_string().contains("未运行"), "{stopped}");

        // PHP 仍要求 socket 文件真实存在。
        let missing = ensure_runtime_ready_for_start(&rt(
            skiff_store::KIND_PHP,
            "running",
            Some("/nonexistent/php-fpm.sock"),
        ))
        .expect_err("socket 缺失必须拒绝启动");
        assert!(
            missing.to_string().contains("PHP socket 未就绪"),
            "{missing}"
        );

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("php-fpm.sock");
        std::fs::write(&sock, b"").unwrap();
        ensure_runtime_ready_for_start(&rt(
            skiff_store::KIND_PHP,
            "running",
            Some(&sock.display().to_string()),
        ))
        .expect("socket 就绪的 PHP 环境应允许启动");
    }

    #[test]
    fn site_vhost_routes_by_runtime_id_to_its_socket() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        // 同版本第二实例：socket 只从 name 推导，与内置实例分开。
        state
            .db()
            .create_runtime(&NewRuntime {
                id: "php-8.5-a".into(),
                kind: skiff_store::KIND_PHP.into(),
                name: "php-8.5-a".into(),
                version: "8.5".into(),
                image: "php:8.5-fpm-alpine".into(),
                image_digest: String::new(),
                socket_path: Some(s.runtime_socket("php", "php-8.5-a").display().to_string()),
                port: None,
                is_default: false,
            })
            .unwrap();

        let site = |domain: &str, runtime_id: &str| {
            state
                .db()
                .create_site(&StoreNewSite {
                    domain: domain.into(),
                    root_dir: s.sites_dir.join(domain).display().to_string(),
                    site_type: TYPE_RUNTIME.into(),
                    runtime_id: Some(runtime_id.into()),
                    proxy_target: None,
                    proxy_websocket: false,
                })
                .unwrap()
        };

        let custom = site("a.local", "php-8.5-a");
        let vhost = state.render_vhost_for(&s, &custom, &[], None).unwrap();
        assert!(
            vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5-a/php-fpm.sock;"),
            "站点必须按 runtime_id 指向所属实例 socket: {vhost}"
        );
        assert!(
            !vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"),
            "不得回落到默认实例: {vhost}"
        );

        let default_site = site("b.local", "php-8.5");
        let vhost = state
            .render_vhost_for(&s, &default_site, &[], None)
            .unwrap();
        assert!(
            vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"),
            "默认实例站点仍指向自己的 socket: {vhost}"
        );
    }

    #[test]
    fn container_path_is_under_shared_web_root() {
        let s = settings_at(Path::new("/www"));
        assert_eq!(
            s.container_path(Path::new("/www/sites/example.com")),
            "/www/sites/example.com"
        );
        assert_eq!(s.container_sites_dir(), "/www/sites");
        assert_eq!(
            s.runtime_pool_conf("php", "php-8.5"),
            PathBuf::from("/www/runtime/php/php-8.5/www.conf")
        );
        assert_eq!(
            s.runtime_socket("php", "php-8.5"),
            PathBuf::from("/www/runtime/php/php-8.5/php-fpm.sock")
        );
        let site_dir = Path::new("/www/sites/example.com");
        assert_eq!(
            s.container_log(site_dir, LogKind::Error),
            "/www/sites/example.com/log/error.log"
        );
        assert_eq!(
            s.container_path(&site_index_dir(site_dir)),
            "/www/sites/example.com/index"
        );
        assert_eq!(
            s.cert_file(site_dir),
            PathBuf::from("/www/sites/example.com/ssl/cert.pem")
        );
    }

    #[test]
    fn php_pool_only_uses_unix_socket() {
        let pool = render_php_pool();
        assert!(pool.contains("listen = /www/runtime/php/php-8.5/php-fpm.sock"));
        assert!(pool.contains("listen.group = nobody"));
        assert!(pool.contains("listen.mode = 0660"));
        assert!(!pool.contains("0666"), "socket 不得 world 可写");
        assert!(!pool.contains("9000"));
    }

    #[test]
    fn runtime_paths_and_container_names_derive_from_name_only() {
        let s = settings_at(Path::new("/www"));
        assert_eq!(
            runtime_container_name("php", "php-8.5"),
            "skiff-php-php-8.5"
        );
        assert_eq!(
            runtime_container_name("php", "php-8.5-a"),
            "skiff-php-php-8.5-a"
        );
        assert_eq!(
            runtime_container_name("node", "php-8.5"),
            "skiff-node-php-8.5",
            "跨 kind 同名容器天然分开"
        );
        assert_eq!(
            runtime_container_socket("php", "php-8.5-a"),
            "/www/runtime/php/php-8.5-a/php-fpm.sock"
        );
        assert_eq!(
            s.runtime_socket("php", "php-8.5-a"),
            PathBuf::from("/www/runtime/php/php-8.5-a/php-fpm.sock")
        );
        assert_eq!(
            s.runtime_home("php", "php-8.5-a"),
            PathBuf::from("/www/runtime/php/php-8.5-a")
        );
        assert_eq!(
            s.runtime_log_dir("php", "php-8.5-a"),
            PathBuf::from("/www/runtime/php/php-8.5-a/log")
        );
        // 同版本不同实例的 socket / 容器名必须不同，不能按版本号共用。
        assert_ne!(
            s.runtime_socket("php", "php-8.5"),
            s.runtime_socket("php", "php-8.5-a")
        );
        assert_ne!(
            runtime_container_name("php", "php-8.5"),
            runtime_container_name("php", "php-8.5-a")
        );
    }

    #[test]
    fn install_locks_are_mutually_exclusive_per_kind_name() {
        let locks = InstallLocks::default();
        let a = locks.try_begin("php:php-8.5-a").expect("首个安装应拿到锁");
        assert!(
            locks.try_begin("php:php-8.5-a").is_none(),
            "同一实例并发安装必须被拒绝"
        );
        assert!(
            locks.try_begin("php:php-8.5-b").is_some(),
            "不同实例可并行安装"
        );
        assert!(
            locks.try_begin("node:php-8.5-a").is_some(),
            "跨 kind 同名是不同实例"
        );
        drop(a);
        assert!(
            locks.try_begin("php:php-8.5-a").is_some(),
            "安装结束后锁应释放"
        );
    }

    #[test]
    fn runtime_job_projection_follows_job_center_status() {
        let svc = app_jobs::JobService::new(std::sync::Arc::new(
            skiff_store::Db::open_in_memory().unwrap(),
        ));
        let job = svc
            .submit(app_jobs::JobSubmit {
                kind: "runtime.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: "php-8.5".into(),
                title: "安装运行环境 php-8.5".into(),
                params: serde_json::json!({"runtime_id": "php-8.5"}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .unwrap();
        let queued = RuntimeJob::from_job(&svc.get(&job.id).unwrap(), Vec::new(), 0);
        assert_eq!(queued.state, RuntimeJobState::Queued);
        assert!(queued.active());
        assert_eq!(queued.kind, "install");

        svc.set_phase(&job.id, "拉取镜像", "拉取镜像", 15, true);
        svc.note(&job.id, "pull php:8.5-fpm");
        let running = RuntimeJob::from_job(&svc.get(&job.id).unwrap(), vec!["pull".into()], 0);
        assert_eq!(running.state, RuntimeJobState::Running);
        assert_eq!(running.step, "拉取镜像");
        assert_eq!(running.progress, 15);

        svc.finish_err(&job.id, "failed", "Docker 不可用");
        let failed = RuntimeJob::from_job(
            &svc.get(&job.id).unwrap(),
            vec!["[00:00:00] 失败：Docker 不可用".into()],
            0,
        );
        assert_eq!(failed.state, RuntimeJobState::Failed);
        assert!(!failed.active());
        assert_eq!(failed.error, "Docker 不可用");
        assert!(failed.log_text().contains("Docker 不可用"));
    }

    #[test]
    fn socket_mode_is_group_only() {
        assert_eq!(SOCKET_MODE, 0o660, "socket 必须是 0660");
        assert_eq!(SOCKET_MODE & 0o007, 0, "others 不得有任何权限");
        assert_eq!(SOCKET_GID, 65534, "固定属组 = 两镜像都有的 nobody");
    }

    #[test]
    fn image_is_pinned_to_digest() {
        assert!(DEFAULT_NGINX_IMAGE.starts_with("openresty/openresty:alpine@sha256:"));
        assert!(!DEFAULT_NGINX_IMAGE.contains("1panel"));
    }

    #[test]
    fn vhost_with_tls_adds_443_ssl_server() {
        let tls = VhostTls {
            cert: "/www/sites/example.com/ssl/cert.pem".into(),
            key: "/www/sites/example.com/ssl/key.pem".into(),
            force_https: false,
        };
        let v = render_site_vhost(
            "example.com",
            &[],
            VhostKind::Php {
                root: "/www/sites/example.com/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
            Some(&tls),
        );
        assert!(v.contains("listen 80;"));
        assert!(v.contains("listen 443 ssl;"));
        assert!(v.contains("ssl_certificate /www/sites/example.com/ssl/cert.pem;"));
        assert!(v.contains("ssl_certificate_key /www/sites/example.com/ssl/key.pem;"));
        assert!(!v.contains("return 301"));
    }

    #[test]
    fn force_https_redirects_80_to_443() {
        let tls = VhostTls {
            cert: "/www/sites/example.com/ssl/cert.pem".into(),
            key: "/www/sites/example.com/ssl/key.pem".into(),
            force_https: true,
        };
        let v = render_site_vhost(
            "example.com",
            &[],
            VhostKind::Php {
                root: "/www/sites/example.com/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
            Some(&tls),
        );
        assert!(v.contains("return 301 https://$host$request_uri;"));
        assert!(v.contains("listen 443 ssl;"));
    }

    #[test]
    fn default_vhost_with_tls_serves_443_default_server() {
        let tls = VhostTls {
            cert: "/www/sites/demo.local/ssl/cert.pem".into(),
            key: "/www/sites/demo.local/ssl/key.pem".into(),
            force_https: false,
        };
        let v = render_default_vhost(
            "demo.local",
            VhostKind::Php {
                root: "/www/sites/demo.local/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/demo.local/log/access.log",
            "/www/sites/demo.local/log/error.log",
            Some(&tls),
        );
        assert!(v.contains("listen 443 ssl default_server;"));
        assert!(v.contains("ssl_certificate /www/sites/demo.local/ssl/cert.pem;"));
    }

    #[test]
    fn default_vhost_serves_index_and_fallback_is_404() {
        let v = render_default_vhost(
            "demo.local",
            VhostKind::Php {
                root: "/www/sites/demo.local/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/demo.local/log/access.log",
            "/www/sites/demo.local/log/error.log",
            None,
        );
        assert!(v.contains("listen 80 default_server;"));
        assert!(v.contains("server_name _;"));
        assert!(v.contains("root /www/sites/demo.local/index;"));
        assert!(v.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"));

        // 兜底文件不能声明 default_server，否则与默认站点冲突；靠文件名排在站点
        // vhost 之前接管裸访问并返回 404。
        assert!(FALLBACK_VHOST.contains("return 404;"));
        assert!(!FALLBACK_VHOST.contains("default_server"));
        // 不能再用 `_`：默认站也用 `_`，nginx 会报重复 server_name 警告。
        assert!(FALLBACK_VHOST.contains("server_name skiff-fallback;"));
        assert!(!FALLBACK_VHOST.contains("server_name _;"));
    }

    #[test]
    fn log_reader_is_reverse_and_filtered() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("demo.access.log");
        std::fs::write(&p, "1 a\n2 b\n3 target\n4 b\n").unwrap();
        assert_eq!(read_log(&p, 2, "").unwrap(), "4 b\n3 target");
        assert_eq!(read_log(&p, 10, "target").unwrap(), "3 target");
        assert!(read_log(&p, 10, "missing").unwrap().is_empty());
    }

    #[test]
    fn log_reader_only_reads_tail_window() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.access.log");
        let line = format!("{}\n", "x".repeat(99));
        let mut text = String::from("EARLY-MARKER\n");
        while text.len() < (LOG_READ_MAX_BYTES as usize) + 4096 {
            text.push_str(&line);
        }
        text.push_str("LATE-MARKER\n");
        std::fs::write(&p, &text).unwrap();
        let out = read_log(&p, 5, "").unwrap();
        assert!(out.contains("LATE-MARKER"), "尾部窗口应包含最新行");
        assert!(!out.contains("EARLY-MARKER"), "窗口之外的内容不应被读入");
        assert!(out.lines().count() <= 5);
        // 关键字过滤同样只作用于尾部窗口，不回头扫描整个文件。
        assert!(read_log(&p, 5, "early-marker").unwrap().is_empty());
    }

    #[test]
    fn log_rotation_renames_and_recreates() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("demo.access.log");
        std::fs::write(&p, "x".repeat(64)).unwrap();
        assert!(rotate_if_large(&p, 32).unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap().len(), 0);
        let rotated = dir.path().join("demo.access.log.1");
        assert_eq!(std::fs::read_to_string(rotated).unwrap().len(), 64);
        assert!(!rotate_if_large(&p, 32).unwrap(), "空文件不再轮转");
    }

    #[test]
    fn rotate_logs_sweeps_only_large_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("a.access.log");
        let small = dir.path().join("b.error.log");
        let other = dir.path().join("c.txt");
        std::fs::write(&big, "x".repeat(64)).unwrap();
        std::fs::write(&small, "y".repeat(8)).unwrap();
        std::fs::write(&other, "z".repeat(64)).unwrap();
        assert_eq!(rotate_logs(dir.path(), 32).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&big).unwrap().len(), 0);
        assert!(big.with_extension("log.1").exists());
        assert_eq!(std::fs::read_to_string(&small).unwrap().len(), 8);
        assert_eq!(std::fs::read_to_string(&other).unwrap().len(), 64);
    }

    #[test]
    fn occupied_port_is_reported_with_process_hint() {
        let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let err = check_port_free(port).expect_err("已监听端口应报占用");
        assert!(err.contains(&port.to_string()), "错误信息应包含端口: {err}");
        drop(listener);
        check_port_free(port).expect("释放后端口应可用");
        assert!(ensure_ports_free(&[port]).is_ok());
    }

    fn container_row(name: &str, state: &str) -> ContainerRow {
        ContainerRow {
            id: "id".into(),
            name: name.into(),
            image: "img".into(),
            image_id: "sha256:deadbeef".into(),
            state: state.into(),
            status: String::new(),
            labels: BTreeMap::new(),
            project: None,
            owner: None,
            protected: false,
            unprotected_kernel: false,
        }
    }

    #[test]
    fn stopped_nginx_does_not_suppress_port_probe() {
        assert!(nginx_container_running(&[container_row(
            NGINX_CONTAINER,
            "running"
        )]));
        assert!(!nginx_container_running(&[container_row(
            NGINX_CONTAINER,
            "exited"
        )]));
        assert!(!nginx_container_running(&[container_row(
            NGINX_CONTAINER,
            "restarting"
        )]));
        assert!(!nginx_container_running(&[container_row(
            "other", "running"
        )]));
    }

    #[test]
    fn rust_runtime_uses_non_login_shell() {
        // 回归：`sh -lc` 会重置 PATH，cargo 不在默认 PATH 里，容器退出码 127。
        // 源码目录固定为环境 `app/`，由 entry.sh 直接进入。
        let cmd = rust_run_command(
            "/www/runtime/rust/x/entry.sh",
            "/www/runtime/rust/x/log/server.log",
        );
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        assert!(cmd[2].starts_with("sh /www/runtime/rust/x/entry.sh"));
        assert!(cmd[2].contains(">> /www/runtime/rust/x/log/server.log 2>&1"));
    }

    #[test]
    fn registry_is_reported_for_pull_failures() {
        assert_eq!(registry_of("nginx:alpine"), "docker.io");
        assert_eq!(registry_of("openresty/openresty:alpine"), "docker.io");
        assert_eq!(registry_of("php:8.5-fpm-alpine"), "docker.io");
        assert_eq!(
            registry_of("registry.example.com/nginx:alpine"),
            "registry.example.com"
        );
        assert_eq!(registry_of("localhost:5000/nginx"), "localhost:5000");
    }

    #[test]
    fn site_layout_has_one_directory_per_site() {
        let s = settings_at(Path::new("/www"));
        let site_dir = Path::new("/www/sites/example.com");
        assert_eq!(
            site_index_dir(site_dir),
            PathBuf::from("/www/sites/example.com/index")
        );
        assert_eq!(
            site_log_dir(site_dir),
            PathBuf::from("/www/sites/example.com/log")
        );
        assert_eq!(
            site_ssl_dir(site_dir),
            PathBuf::from("/www/sites/example.com/ssl")
        );
        assert_eq!(
            site_conf_dir(site_dir),
            PathBuf::from("/www/sites/example.com/conf")
        );
        assert_eq!(
            s.log_file(site_dir, LogKind::Access),
            PathBuf::from("/www/sites/example.com/log/access.log")
        );
        assert_eq!(
            s.log_file(site_dir, LogKind::Error),
            PathBuf::from("/www/sites/example.com/log/error.log")
        );
        assert_eq!(
            s.vhost_file(site_dir, "example.com"),
            PathBuf::from("/www/sites/example.com/conf/example.com.conf")
        );
        assert_eq!(
            s.vhost_disabled(site_dir, "example.com"),
            PathBuf::from("/www/sites/example.com/conf/example.com.conf.disabled")
        );
    }

    #[test]
    fn sites_include_entry_globs_per_site_conf() {
        let s = settings_at(Path::new("/www"));
        let text = s.sites_include_conf();
        assert!(text.contains("include /www/sites/*/conf/*.conf;"));
        assert!(text.contains("备份"));
        // 默认站/兜底站仍在内核公共目录，不参与该通配。
        assert!(!text.contains(DEFAULT_CONF));
        assert!(!text.contains(FALLBACK_CONF));
        assert_eq!(SITES_INCLUDE_CONF, "10-skiff-sites.conf");
    }

    #[test]
    fn rotate_site_logs_scans_each_site_log_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sites = dir.path().join("sites");
        let a_log = sites.join("a.example").join("log");
        let b_log = sites.join("b.example").join("log");
        std::fs::create_dir_all(&a_log).unwrap();
        std::fs::create_dir_all(&b_log).unwrap();
        std::fs::write(a_log.join("access.log"), "x".repeat(64)).unwrap();
        std::fs::write(b_log.join("error.log"), "y".repeat(64)).unwrap();
        std::fs::write(sites.join("readme.txt"), "z".repeat(64)).unwrap();
        assert_eq!(rotate_site_logs(&sites, 32).unwrap(), 2);
    }

    #[test]
    fn image_version_cache_positive_has_no_ttl_and_negative_expires() {
        let mut cache: HashMap<String, (Option<String>, std::time::Instant)> = HashMap::new();
        let t0 = std::time::Instant::now();
        // 正缓存：image_id 不可变，过了很久仍然命中。
        cache.insert("sha256:abc".into(), (Some("8.5.12".into()), t0));
        match image_version_lookup(
            &cache,
            "sha256:abc",
            t0 + std::time::Duration::from_secs(3600),
        ) {
            ImageVersionHit::Known(v) => assert_eq!(v, "8.5.12"),
            _ => panic!("正缓存不应过期"),
        }
        // 负缓存：60 秒内抑制重复 inspect。
        cache.insert("sha256:def".into(), (None, t0));
        assert!(matches!(
            image_version_lookup(
                &cache,
                "sha256:def",
                t0 + std::time::Duration::from_secs(59)
            ),
            ImageVersionHit::Suppressed
        ));
        assert!(matches!(
            image_version_lookup(
                &cache,
                "sha256:def",
                t0 + std::time::Duration::from_secs(61)
            ),
            ImageVersionHit::Miss
        ));
        // 未登记 image_id：未命中。
        assert!(matches!(
            image_version_lookup(&cache, "sha256:ghi", t0),
            ImageVersionHit::Miss
        ));
    }

    #[test]
    fn image_id_change_does_not_reuse_previous_version() {
        let mut cache: HashMap<String, (Option<String>, std::time::Instant)> = HashMap::new();
        let t0 = std::time::Instant::now();
        cache.insert("sha256:old".into(), (Some("8.5.11".into()), t0));
        // 重建后 image_id 变化：新 key 未命中，必须重新 inspect，不会沿用旧版本。
        assert!(matches!(
            image_version_lookup(&cache, "sha256:new", t0),
            ImageVersionHit::Miss
        ));
    }

    #[test]
    fn digest_normalizes_repo_digest_and_builds_pinned_ref() {
        let repo = "php@sha256:abc123";
        assert_eq!(digest_sha256(repo), "sha256:abc123");
        assert_eq!(digest_sha256("sha256:abc123"), "sha256:abc123");
        assert_eq!(digest_sha256(""), "");
        assert_eq!(
            pinned_image_ref("php:8.5-fpm-alpine", repo),
            "php:8.5-fpm-alpine@sha256:abc123"
        );
        // 已是固定引用或没有 digest 时保持原样。
        assert_eq!(
            pinned_image_ref("php:8.5-fpm-alpine@sha256:abc123", repo),
            "php:8.5-fpm-alpine@sha256:abc123"
        );
        assert_eq!(
            pinned_image_ref("php:8.5-fpm-alpine", ""),
            "php:8.5-fpm-alpine"
        );
    }

    #[tokio::test]
    async fn deleting_runtime_removes_directory_and_row() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        state
            .db()
            .ensure_runtime(&NewRuntime {
                id: "php-custom".into(),
                kind: skiff_store::KIND_PHP.into(),
                name: "php-custom".into(),
                version: "8.5".into(),
                image: DEFAULT_PHP_IMAGE.into(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
        let home = s.runtime_home(skiff_store::KIND_PHP, "php-custom");
        std::fs::create_dir_all(s.runtime_log_dir(skiff_store::KIND_PHP, "php-custom")).unwrap();
        std::fs::write(
            s.runtime_pool_conf(skiff_store::KIND_PHP, "php-custom"),
            "listen = /www/runtime/php/php-custom/php-fpm.sock\n",
        )
        .unwrap();

        let report = state.delete_site_runtime("php-custom").await.unwrap();
        assert!(!home.exists(), "运行环境目录（含 FPM 池配置）必须删除");
        assert!(
            state.db().runtime_by_id("php-custom").unwrap().is_none(),
            "DB 行必须删除"
        );
        assert!(
            report.skipped.iter().any(|s| s.contains("Docker 不可用")),
            "无 Docker 时必须说明容器未清理：{:?}",
            report.skipped
        );
        assert!(
            report.completed.iter().any(|s| s.contains("目录")),
            "报告必须记录已删除目录：{:?}",
            report.completed
        );
    }

    #[tokio::test]
    async fn deleting_runtime_with_bound_site_is_rejected_and_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        state
            .db()
            .ensure_runtime(&NewRuntime {
                id: "php-bound".into(),
                kind: skiff_store::KIND_PHP.into(),
                name: "php-bound".into(),
                version: "8.5".into(),
                image: DEFAULT_PHP_IMAGE.into(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
        let home = s.runtime_home(skiff_store::KIND_PHP, "php-bound");
        std::fs::create_dir_all(&home).unwrap();
        let site_dir = s.sites_dir.join("bound.local");
        state
            .db()
            .create_site(&StoreNewSite {
                domain: "bound.local".into(),
                root_dir: site_dir.display().to_string(),
                site_type: TYPE_RUNTIME.into(),
                runtime_id: Some("php-bound".into()),
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();

        let err = state.delete_site_runtime("php-bound").await.unwrap_err();
        let text = format!("{err}");
        assert!(matches!(err, SiteError::Conflict(_)), "{text}");
        assert!(text.contains("bound.local"), "{text}");
        assert!(home.exists(), "有绑定时不得删除目录");
        assert!(
            state.db().runtime_by_id("php-bound").unwrap().is_some(),
            "有绑定时不得删除记录"
        );
    }
}
