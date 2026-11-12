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

use skiff_docker::{ContainerSpec, DockerParams, Mount, Verb};
use skiff_store::{NewSite as StoreNewSite, Runtime};

use crate::jobs as app_jobs;
use crate::runtime_catalog;
use crate::AppState;

mod crud;
mod health;
mod layout;
mod logs;
mod runtime;
mod spec;
pub use health::*;
pub use layout::*;
pub use logs::*;
pub use spec::*;

/// nginx 配置/证书写入与重载的进程内串行锁。
///
/// 证书与私钥是两个文件，各自 `rename` 之间存在一个极短窗口。任何在这期间
/// 重新读取配置的 nginx（保存 vhost、TLS 变更、证书上传、巡检）都可能读到
/// 「新证书 + 旧私钥」并报 `key values mismatch`。因此：
/// - 写对的一方持锁完成两次 `rename` 与随后的 reload；
/// - 所有会让 nginx 重新加载配置的动作（含定时巡检）都先取同一把锁。
pub fn nginx_write_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

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

    pub fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
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

/// 站点表单字段：领域错误携带它，展示层据此高亮，禁止按中文文案反推。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteField {
    Domain,
    RootDir,
    RuntimeId,
    ProxyTarget,
    Aliases,
}

impl SiteField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::RootDir => "root_dir",
            Self::RuntimeId => "runtime_id",
            Self::ProxyTarget => "proxy_target",
            Self::Aliases => "aliases",
        }
    }
}

/// 站点领域错误。`Field` 只附加可判定字段，不改变原变体、文案与传输分类。
#[derive(Debug, Clone)]
pub enum SiteError {
    BadRequest(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
    Field(SiteField, Box<SiteError>),
}

impl SiteError {
    /// 给错误附加表单字段；已有字段时保持最内层判定。
    pub fn at(field: SiteField, error: SiteError) -> SiteError {
        match error {
            SiteError::Field(..) => error,
            other => SiteError::Field(field, Box::new(other)),
        }
    }

    pub fn field(&self) -> Option<SiteField> {
        match self {
            SiteError::Field(field, _) => Some(*field),
            _ => None,
        }
    }

    /// 去掉字段包装，得到原有传输分类；用于传输层映射。
    pub fn unfield(self) -> SiteError {
        match self {
            SiteError::Field(_, inner) => inner.unfield(),
            other => other,
        }
    }
}

impl std::fmt::Display for SiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SiteError::BadRequest(m) => write!(f, "{m}"),
            SiteError::Conflict(m) => write!(f, "{m}"),
            SiteError::Unavailable(m) => write!(f, "{m}"),
            SiteError::Internal(m) => write!(f, "{m}"),
            SiteError::Field(_, inner) => write!(f, "{inner}"),
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

#[cfg(test)]
mod tests;
