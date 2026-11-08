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

use crate::runtime_catalog;
use crate::AppState;

pub const NGINX_CONTAINER: &str = "skiff-web-nginx";
/// 每个 PHP 实例一个独立容器：`skiff-php-<name>`。
pub const PHP_CONTAINER_PREFIX: &str = "skiff-php";
/// OpenResty 官方镜像，固定 digest（不复用 1Panel 的 tag）。
pub const DEFAULT_NGINX_IMAGE: &str =
    "openresty/openresty:alpine@sha256:2cb90d64192934c26b3749cf744f47edd59277787fe761bd109eeebdc3e2214a";
pub const DEFAULT_PHP_IMAGE: &str = "php:8.5-fpm-alpine";
/// nginx 用 host 网络直接监听宿主 80；443 预留给 HTTPS。
pub const DEFAULT_PORT: u16 = 80;
pub const DEFAULT_PHP_VERSION: &str = "8.5";
/// 内核内置可安装的 PHP 版本目录由 `runtime_catalog` 单一维护。
/// P1 默认 PHP 运行环境 id（与 `site_runtimes` 种入行一致）。
pub const PHP_RUNTIME_ID: &str = "php-8.5";
/// 容器内 `/www` 的宿主来源由 `SiteSettings::web_root` 决定。
pub const CONTAINER_WEB_ROOT: &str = "/www";
/// 每版本独立 socket 的容器内根目录：`/www/runtime/php-<version>/php-fpm.sock`。
pub const CONTAINER_RUNTIME_ROOT: &str = "/www/runtime";
/// socket 固定属组：openresty 镜像的 worker 与 php 镜像都带 `nobody`（gid 65534）。
/// 0660 只放行该组，本机普通用户连不上。
pub const SOCKET_GID: u32 = 65534;
pub const SOCKET_MODE: u32 = 0o660;
/// PHP-FPM 默认池配置在容器内的位置（用宿主生成的文件覆盖它）。
pub const CONTAINER_PHP_POOL: &str = "/usr/local/etc/php-fpm.d/www.conf";
/// `fastcgi_params` 的容器内绝对路径。
///
/// vhost 里的 `include` 必须写绝对路径：主配置编辑会用
/// `nginx -t -c /www/nginx/nginx.conf.candidate` 校验候选文件，此时配置目录变成
/// `/www/nginx`，相对 `include fastcgi_params` 会解析成 `/www/nginx/fastcgi_params`
/// 并报 No such file，导致任何合法修改都被误判为失败。
pub const CONTAINER_FASTCGI_PARAMS: &str = "/usr/local/openresty/nginx/conf/fastcgi_params";
/// 单文件日志轮转阈值（内核自己轮转，不依赖宿主 logrotate）。
pub const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// 内置默认 PHP 实例名（`name` 与 id 一致）。
pub fn default_php_name() -> String {
    format!("php-{DEFAULT_PHP_VERSION}")
}

/// 容器名只从 `<kind>/<name>` 推导：`skiff-<kind>-<name>`。
pub fn runtime_container_name(kind: &str, name: &str) -> String {
    format!("skiff-{kind}-{name}")
}

/// 探测单个运行环境的原始容器状态。
///
/// 三种「不在运行」必须分开：Docker 不可达/列表失败是 `unknown`（不知道），
/// 列表能查但没这个容器是 `missing`（没装），容器在但状态非 running 由
/// [`runtime_effective_state`] 归一成 `stopped`。
fn probe_container_state(docker_known: bool, state: Option<&str>) -> String {
    if !docker_known {
        return "unknown".into();
    }
    match state {
        None => "missing".into(),
        Some(s) if s.trim().is_empty() => "unknown".into(),
        Some(s) => s.to_string(),
    }
}

/// Docker 原始状态归一为界面使用的业务状态；未知值单独保留，不与 running 合并。
fn runtime_effective_state(raw: &str) -> &'static str {
    match raw {
        "running" => "running",
        "stopped" | "exited" | "dead" | "created" | "paused" | "restarting" | "removing" => {
            "stopped"
        }
        "missing" => "missing",
        _ => "unknown",
    }
}

/// 从 `repo@sha256:...` 取出摘要部分；已是摘要时原样返回。
fn digest_sha256(digest: &str) -> &str {
    match digest.find("@sha256:") {
        Some(i) => &digest[i + 1..],
        None => digest.trim(),
    }
}

/// 用于拉取与建容器的固定引用：`image@sha256:...`；无 digest 时保持 tag。
fn pinned_image_ref(image: &str, digest: &str) -> String {
    let sha = digest_sha256(digest);
    if sha.is_empty() || image.contains('@') {
        image.to_string()
    } else {
        format!("{image}@{sha}")
    }
}

/// PHP 实例 socket 的容器内路径：`/www/runtime/<kind>/<name>/php-fpm.sock`。
pub fn runtime_container_socket(kind: &str, name: &str) -> String {
    format!("{CONTAINER_RUNTIME_ROOT}/{kind}/{name}/php-fpm.sock")
}

/// 内容一致时跳过写入；内容变化或文件缺失时原子落盘并设置权限。
fn write_if_changed(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    let unchanged = std::fs::read(path).map(|old| old == bytes).unwrap_or(false);
    if !unchanged {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
                .map_err(|e| format!("设置 {} 权限失败：{e}", tmp.display()))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, path).map_err(|e| format!("替换 {} 失败：{e}", path.display()))?;
    }
    Ok(())
}

/// 站点目录内的固定子目录（一站一目录，整目录备份即整站备份）。
pub const SITE_INDEX_DIR: &str = "index";
pub const SITE_LOG_DIR: &str = "log";
pub const SITE_SSL_DIR: &str = "ssl";
pub const SITE_CONF_DIR: &str = "conf";

/// 公共配置目录里的站点 include 入口：主配置 `include /etc/nginx/conf.d/*.conf`
/// 先加载本文件，再由它把各站点自己的 `conf/*.conf` 拉进来。
/// 文件名保持 `10-` 前缀，排在 `00-skiff-default.conf` / `00-skiff-fallback.conf` 之后。
pub const SITES_INCLUDE_CONF: &str = "10-skiff-sites.conf";

const KEY_PREFIX: &str = "sites.image_prefix";
const KEY_NGINX_IMAGE: &str = "sites.nginx_image";
const KEY_PHP_IMAGE: &str = "sites.php_image";
const KEY_MIRROR: &str = "sites.registry_mirror";
const KEY_WAF_MODE: &str = "sites.waf_mode";

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
    pub nginx_image: String,
    pub php_image: String,
    pub port: u16,
    pub registry_mirror: String,
    pub waf_mode: String,
}

/// 站点文档根（`index/`）；files 插件与「目录」页的授权根只能是它。
pub fn site_index_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_INDEX_DIR)
}

pub fn site_log_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_LOG_DIR)
}

pub fn site_ssl_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_SSL_DIR)
}

pub fn site_conf_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_CONF_DIR)
}

impl SiteSettings {
    fn container_rel(&self, host: &Path) -> String {
        let rel = host.strip_prefix(&self.web_root).unwrap_or(host);
        // 容器内一律 POSIX 路径：Windows 开发机上 `Path` 会带反斜杠。
        let rel = rel.to_string_lossy().replace('\\', "/");
        format!("{CONTAINER_WEB_ROOT}/{}", rel.trim_start_matches('/'))
    }

    pub fn container_sites_dir(&self) -> String {
        self.container_rel(&self.sites_dir)
    }
    pub fn container_conf_dir(&self) -> &'static str {
        "/etc/nginx/conf.d"
    }

    /// 宿主站点目录 → 容器内路径；调用方保证 `host` 在 `web_root` 内。
    pub fn container_path(&self, host: &Path) -> String {
        self.container_rel(host)
    }

    /// 站点日志：`<站点目录>/log/access.log|error.log`。
    pub fn log_file(&self, site_dir: &Path, kind: LogKind) -> PathBuf {
        site_log_dir(site_dir).join(format!("{}.log", kind.as_str()))
    }

    pub fn container_log(&self, site_dir: &Path, kind: LogKind) -> String {
        self.container_rel(&self.log_file(site_dir, kind))
    }

    /// 某环境实例的独立运行目录：`<runtime>/<kind>/<name>/`。
    pub fn runtime_home(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_dir.join(kind).join(name)
    }

    /// PHP 实例 socket 的宿主路径（0660 root:nobody）。
    pub fn runtime_socket(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("php-fpm.sock")
    }

    /// PHP 实例的 FPM 池配置（宿主文件，覆盖镜像默认 www.conf）。
    pub fn runtime_pool_conf(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("www.conf")
    }

    /// PHP 实例的独立日志目录。
    pub fn runtime_log_dir(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("log")
    }

    /// socket 的容器内路径；vhost 的 `fastcgi_pass` 必须用它。
    pub fn runtime_socket_container(&self, kind: &str, name: &str) -> String {
        runtime_container_socket(kind, name)
    }

    /// nginx 全局日志目录（错误日志与访问汇总）。
    pub fn global_log_dir(&self) -> PathBuf {
        self.web_root.join("log")
    }

    /// 内核生成并挂载进 nginx 容器的主配置。
    pub fn nginx_main_conf(&self) -> PathBuf {
        self.web_root.join("nginx").join("nginx.conf")
    }

    pub fn container_nginx_main_conf(&self) -> &'static str {
        "/usr/local/openresty/nginx/conf/nginx.conf"
    }

    /// WAF 规则目录（预留；本期不加载、不拦截）。
    pub fn waf_rules_dir(&self) -> PathBuf {
        self.web_root.join("skiff").join("waf").join("rules")
    }

    /// 内核随包分发的本地工具目录：`<web_root>/skiff/tools`。
    ///
    /// 该目录整体挂载进 PHP 容器（容器内 `/www/skiff/tools`），扩展安装器从这里
    /// 读取，不再在运行时访问 GitHub。
    pub fn tools_dir(&self) -> PathBuf {
        self.web_root.join("skiff").join("tools")
    }

    /// 本地 mlocati 扩展安装器（宿主路径）。
    pub fn php_extension_installer(&self) -> PathBuf {
        self.tools_dir().join("install-php-extensions")
    }

    /// 宿主侧系统包缓存（alpine `apk` / debian `apt` 下载的包文件）。
    ///
    /// 挂进每个 PHP 容器的 `/var/cache/apk`。仅挂载目录还不够：apk-tools 3 在
    /// `/etc/apk/cache` 不存在时会回退到该静态目录，但默认不缓存 `.apk`；安装器
    /// prelude 会写 `/etc/apk/config` 打开 `cache-packages`，并设置
    /// `IPE_KEEP_SYSPKG_CACHE=1` 阻止收尾清理，才能让重建/同机多环境复用
    /// 已下载的依赖包。
    pub fn system_pkg_cache_dir(&self) -> PathBuf {
        self.runtime_dir.join("cache").join("apk")
    }

    /// 站点证书：`<站点目录>/ssl/cert.pem|key.pem`。
    pub fn cert_file(&self, site_dir: &Path) -> PathBuf {
        site_ssl_dir(site_dir).join("cert.pem")
    }

    pub fn key_file(&self, site_dir: &Path) -> PathBuf {
        site_ssl_dir(site_dir).join("key.pem")
    }

    /// 站点 vhost：`<站点目录>/conf/<域名>.conf`。
    pub fn vhost_file(&self, site_dir: &Path, domain: &str) -> PathBuf {
        site_conf_dir(site_dir).join(format!("{domain}.conf"))
    }

    pub fn vhost_disabled(&self, site_dir: &Path, domain: &str) -> PathBuf {
        site_conf_dir(site_dir).join(format!("{domain}.conf.disabled"))
    }

    /// 站点 include 入口内容：通配加载各站点自己的 `conf/*.conf`。
    ///
    /// `.conf.disabled` 不匹配 `*.conf`，因此停用站点天然不会被 nginx 加载。
    pub fn sites_include_conf(&self) -> String {
        format!(
            "# Skiff 站点 vhost 入口：每个站点的配置在自己的 conf/ 下，\n\
             # 备份 {sites}/<域名>/ 即备份整站。\n\
             include {sites}/*/conf/*.conf;\n",
            sites = self.container_sites_dir()
        )
    }
}

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
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
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
/// 内存态即可：日志是过程信息，重启后面板不再展示旧日志；可重试的来源是库里的
/// 运行环境行与容器实际状态，不依赖本结构持久化。
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
    fn new(runtime_id: &str, kind: &str) -> Self {
        let ts = chrono::Local::now().timestamp();
        let mut job = Self {
            runtime_id: runtime_id.to_string(),
            kind: kind.to_string(),
            ext_keys: Vec::new(),
            state: RuntimeJobState::Queued,
            step: "排队".into(),
            progress: 0,
            error: String::new(),
            logs: Vec::new(),
            started_at: ts,
            updated_at: ts,
        };
        job.note("任务已创建，等待执行");
        job
    }

    pub fn active(&self) -> bool {
        matches!(
            self.state,
            RuntimeJobState::Queued | RuntimeJobState::Running
        )
    }

    fn note(&mut self, line: &str) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        self.logs.push(format!("[{stamp}] {line}"));
        // 日志只保留最近 300 行：安装输出可能很长，轮询响应要可控。
        let excess = self.logs.len().saturating_sub(300);
        if excess > 0 {
            self.logs.drain(..excess);
        }
        self.updated_at = chrono::Local::now().timestamp();
    }

    fn advance(&mut self, step: &str, progress: u8, line: &str) {
        if self.active() {
            self.state = RuntimeJobState::Running;
        }
        self.step = step.to_string();
        self.progress = progress.min(100);
        self.note(line);
    }

    fn finish_ok(&mut self, line: &str) {
        self.state = RuntimeJobState::Done;
        self.step = "完成".into();
        self.progress = 100;
        self.error.clear();
        self.note(line);
    }

    fn finish_err(&mut self, error: &str) {
        self.state = RuntimeJobState::Failed;
        self.step = "失败".into();
        self.error = error.to_string();
        self.note(&format!("失败：{error}"));
    }

    pub fn log_text(&self) -> String {
        self.logs.join("\n")
    }
}

/// 按 runtime id 保存最近一次任务（同一环境同时只允许一个任务）。
#[derive(Debug, Default)]
pub struct RuntimeJobs {
    jobs: std::collections::HashMap<String, RuntimeJob>,
}

impl RuntimeJobs {
    pub fn get(&self, id: &str) -> Option<&RuntimeJob> {
        self.jobs.get(id)
    }

    pub fn list(&self) -> Vec<RuntimeJob> {
        let mut out: Vec<RuntimeJob> = self.jobs.values().cloned().collect();
        out.sort_by_key(|j| std::cmp::Reverse(j.updated_at));
        out
    }

    fn begin(&mut self, id: &str, kind: &str) -> Result<(), SiteError> {
        if self.jobs.get(id).is_some_and(RuntimeJob::active) {
            return Err(SiteError::Conflict(
                "该运行环境已有任务在执行，请等待完成".into(),
            ));
        }
        self.jobs.insert(id.to_string(), RuntimeJob::new(id, kind));
        Ok(())
    }

    fn begin_ext(&mut self, id: &str, ext_key: &str) -> Result<(), SiteError> {
        self.begin_exts(id, &[ext_key.to_string()])
    }

    /// 批量扩展任务：一次记录全部目标 token，日志合并到同一任务。
    fn begin_exts(&mut self, id: &str, keys: &[String]) -> Result<(), SiteError> {
        self.begin(id, "extensions")?;
        if let Some(job) = self.jobs.get_mut(id) {
            job.ext_keys = keys.to_vec();
            job.note(&format!(
                "目标扩展（{} 项）：{}",
                keys.len(),
                keys.join("、")
            ));
        }
        Ok(())
    }

    fn update(&mut self, id: &str, f: impl FnOnce(&mut RuntimeJob)) {
        if let Some(job) = self.jobs.get_mut(id) {
            f(job);
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
    pub waf_mode: String,
    pub waf_rules_dir: String,
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
// 域名 / 版本 / vhost（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// 域名白名单：小写字母、数字、`-`、`.`；每段非空且不以 `-` 开头/结尾。
///
/// 拒绝 `..`、`/`、端口、通配符与空白；同时限制总长 253、单段 63。
pub fn validate_domain(raw: &str) -> Result<String, SiteError> {
    let d = raw.trim().to_ascii_lowercase();
    if d.is_empty() {
        return Err(SiteError::BadRequest("域名不能为空".into()));
    }
    if d.len() > 253 {
        return Err(SiteError::BadRequest("域名长度超过 253".into()));
    }
    if d.contains('/') || d.contains('\\') || d.contains(' ') || d.contains('_') {
        return Err(SiteError::BadRequest(
            "域名只能包含字母、数字、点和中划线".into(),
        ));
    }
    let labels: Vec<&str> = d.split('.').collect();
    if labels.len() < 2 {
        return Err(SiteError::BadRequest(
            "域名至少包含两段（如 example.com）".into(),
        ));
    }
    for label in &labels {
        if label.is_empty() {
            return Err(SiteError::BadRequest("域名不能包含空段".into()));
        }
        if label.len() > 63 {
            return Err(SiteError::BadRequest("域名单段长度超过 63".into()));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(SiteError::BadRequest(
                "域名每段不能以中划线开头或结尾".into(),
            ));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(SiteError::BadRequest(
                "域名只能包含字母、数字、点和中划线".into(),
            ));
        }
    }
    // 纯数字最后一段（IPv4）不接受：站点按域名建目录，IP 访问请用默认站点。
    if labels.last().is_some_and(|l| {
        l.chars().all(|c| c.is_ascii_digit()) && labels[0].chars().all(|c| c.is_ascii_digit())
    }) {
        return Err(SiteError::BadRequest(
            "请使用域名而不是 IP 地址创建站点".into(),
        ));
    }
    Ok(d)
}

/// 批量别名解析：主域名与别名一次提交，去重与语法校验都收在这里。
///
/// `raw` 里每个元素可能是一行粘贴内容（换行/逗号/空格混合），也可能是单行输入。
/// 重复别名与「别名等于主域名」一律拒绝——不静默丢弃，避免用户以为已经加上。
pub fn normalize_site_aliases(domain: &str, raw: &[String]) -> Result<Vec<String>, SiteError> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        for token in item
            .split(|c: char| c == ',' || c == '，' || c == ';' || c == '；' || c.is_whitespace())
        {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let alias = validate_domain(token)?;
            if alias == domain {
                return Err(SiteError::BadRequest("别名不能与主域名相同".into()));
            }
            if out.contains(&alias) {
                return Err(SiteError::Conflict(format!("域名重复：{alias}")));
            }
            out.push(alias);
        }
    }
    Ok(out)
}

/// 站点目录：用户只填目录名（如 `example.com`），前缀由内核自动补全。
///
/// 空值表示与主域名同名；只允许单段目录名，拒绝 `/`、`..` 与空白。
pub fn site_root_for(
    sites_dir: &Path,
    domain: &str,
    slug: Option<&str>,
) -> Result<PathBuf, SiteError> {
    let Some(slug) = slug.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(sites_dir.join(domain));
    };
    if slug == "." || slug == ".." || slug.contains(['/', '\\']) || slug.contains("..") {
        return Err(SiteError::BadRequest(
            "站点目录只能填写目录名，不能包含 / 或 ..".into(),
        ));
    }
    if slug.chars().any(char::is_whitespace)
        || !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(SiteError::BadRequest(
            "站点目录只能包含字母、数字、点、中划线与下划线".into(),
        ));
    }
    Ok(sites_dir.join(slug))
}

/// 站点类型白名单：只接受三类，大小写归一。
pub fn validate_site_type(raw: &str) -> Result<&'static str, SiteError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        TYPE_RUNTIME => Ok(TYPE_RUNTIME),
        TYPE_STATIC => Ok(TYPE_STATIC),
        TYPE_PROXY => Ok(TYPE_PROXY),
        other => Err(SiteError::BadRequest(format!(
            "未知站点类型：{other}（应为 runtime / static / proxy）"
        ))),
    }
}

/// 反向代理上游：必须是 `http(s)://host[:port][/path]`，禁止 userinfo、空白与裸路径。
pub fn validate_proxy_target(raw: &str) -> Result<String, SiteError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(SiteError::BadRequest("上游地址不能为空".into()));
    }
    if t.chars().any(char::is_whitespace) {
        return Err(SiteError::BadRequest("上游地址不能包含空白".into()));
    }
    let (scheme, rest) = t
        .split_once("://")
        .ok_or_else(|| SiteError::BadRequest("上游地址必须以 http:// 或 https:// 开头".into()))?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(SiteError::BadRequest(
            "上游地址只支持 http:// 或 https://".into(),
        ));
    }
    if rest.is_empty() {
        return Err(SiteError::BadRequest("上游地址缺少主机".into()));
    }
    if rest.contains('@') {
        return Err(SiteError::BadRequest("上游地址不能包含用户名/密码".into()));
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        return Err(SiteError::BadRequest("上游地址缺少主机".into()));
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(SiteError::BadRequest("上游主机名包含非法字符".into()));
    }
    if let Some(port) = authority.split_once(':').map(|(_, p)| p) {
        let port: u16 = port
            .parse()
            .map_err(|_| SiteError::BadRequest("上游端口必须是 1-65535".into()))?;
        if port == 0 {
            return Err(SiteError::BadRequest("上游端口必须是 1-65535".into()));
        }
    }
    // 统一小写 scheme，其余保留用户写法（路径大小写敏感）。
    let normalized = format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        rest.trim_end_matches('/')
    );
    Ok(normalized)
}

/// Node/Go 站点端口占位校验（P1 只落字段；P2 才实际分配）。范围与 P2 契约一致。
pub fn validate_app_port(raw: i64) -> Result<u16, SiteError> {
    if !(1024..=65535).contains(&raw) {
        return Err(SiteError::BadRequest(
            "应用端口必须在 1024-65535 之间".into(),
        ));
    }
    Ok(raw as u16)
}

/// TLS 参数：证书/私钥使用**容器内** `/www/...` 路径。
#[derive(Debug, Clone)]
pub struct VhostTls {
    pub cert: String,
    pub key: String,
    /// 80 端口是否 301 到 443。
    pub force_https: bool,
}

/// vhost 的三种业务形态；容器内路径由调用方换算，生成函数不再硬编码 socket。
#[derive(Debug, Clone)]
pub enum VhostKind<'a> {
    /// 静态站：只读文档根，不接 PHP。
    Static { root: &'a str },
    /// 运行环境站（PHP）：fastcgi 指向运行环境登记的 socket。
    Php { root: &'a str, socket: &'a str },
    /// 运行环境站（Rust）：独立进程监听固定端口，vhost 反代过去。
    Process { root: &'a str, port: u16 },
    /// 反向代理：不落文档根，只转发到上游。
    Proxy { target: &'a str, websocket: bool },
}

/// 生成站点 vhost。路径里的站点目录已换算为容器内 `/www/...`；
/// PHP 的 socket 来自 `site_runtimes.socket_path`，不在此硬编码。
pub fn render_site_vhost(
    domain: &str,
    aliases: &[String],
    kind: VhostKind<'_>,
    access_log: &str,
    error_log: &str,
    tls: Option<&VhostTls>,
) -> String {
    let names = if aliases.is_empty() {
        domain.to_string()
    } else {
        format!("{} {}", domain, aliases.join(" "))
    };
    let mut out = String::new();
    if tls.is_some_and(|t| t.force_https) {
        // 强制 HTTPS：80 只做跳转，不再渲染站点内容。
        out.push_str("server {\n    listen 80;\n");
        out.push_str(&format!("    server_name {names};\n"));
        out.push_str("    return 301 https://$host$request_uri;\n}\n");
    } else {
        out.push_str("server {\n    listen 80;\n");
        out.push_str(&format!("    server_name {names};\n"));
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl;\n");
        out.push_str(&format!("    server_name {names};\n"));
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    out
}

fn push_site_body(out: &mut String, kind: &VhostKind<'_>, access_log: &str, error_log: &str) {
    out.push_str(&format!("    access_log {access_log};\n"));
    out.push_str(&format!("    error_log {error_log};\n"));
    match kind {
        VhostKind::Static { root } => {
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    index index.html;\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
        }
        VhostKind::Php { root, socket } => {
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    index index.php index.html;\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
            out.push_str(&format!(
                "    location ~ \\.php$ {{\n\
                 \x20       include {CONTAINER_FASTCGI_PARAMS};\n\
                 \x20       fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n"
            ));
            out.push_str(&format!("        fastcgi_pass unix:{socket};\n"));
            out.push_str("        fastcgi_index index.php;\n    }\n");
        }
        VhostKind::Process { root, port } => {
            // Rust 进程自带内容与路由：文档根只作兜底静态资源，主路径反代到端口。
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    location / {\n");
            out.push_str(&format!("        proxy_pass http://127.0.0.1:{port};\n"));
            out.push_str("        proxy_http_version 1.1;\n");
            out.push_str("        proxy_set_header Host $host;\n");
            out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
            out.push_str("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n");
            out.push_str("        proxy_set_header X-Forwarded-Proto $scheme;\n");
            out.push_str("        proxy_set_header Upgrade $http_upgrade;\n");
            out.push_str("        proxy_set_header Connection \"upgrade\";\n");
            out.push_str("        proxy_read_timeout 3600s;\n    }\n");
        }
        VhostKind::Proxy { target, websocket } => {
            out.push_str("    location / {\n");
            out.push_str(&format!("        proxy_pass {target};\n"));
            out.push_str("        proxy_http_version 1.1;\n");
            out.push_str("        proxy_set_header Host $host;\n");
            out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
            out.push_str("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n");
            out.push_str("        proxy_set_header X-Forwarded-Proto $scheme;\n");
            if *websocket {
                out.push_str("        proxy_set_header Upgrade $http_upgrade;\n");
                out.push_str("        proxy_set_header Connection \"upgrade\";\n");
            }
            out.push_str("        proxy_read_timeout 3600s;\n");
            out.push_str("    }\n");
        }
    }
    out.push_str("}\n");
}

fn default_probe_php() -> &'static str {
    "<?php\n// Skiff 站点探针：确认 PHP-FPM 正常渲染。\nphpinfo();\n"
}

fn default_probe_html() -> &'static str {
    "<!doctype html>\n<html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
     <title>Skiff 静态站点</title></head>\n<body><h1>站点已就绪</h1>\
     <p>把您的静态文件放入 <code>index/</code> 目录即可。</p></body></html>\n"
}

/// 默认站点 vhost：直接访问 IP:80 时落到该站点的 `public`。
///
/// `default_server` 只能有一个；本文件存在时由它接管裸访问，不存在时由
/// `00-skiff-fallback.conf`（按文件名先于站点 vhost 加载）返回 404。
pub fn render_default_vhost(
    domain: &str,
    kind: VhostKind<'_>,
    access_log: &str,
    error_log: &str,
    tls: Option<&VhostTls>,
) -> String {
    let mut out = format!("# Skiff 默认站点：直接访问 IP:80 时展示 {domain}。\n");
    if tls.is_some_and(|t| t.force_https) {
        out.push_str("server {\n    listen 80 default_server;\n    server_name _;\n");
        out.push_str("    return 301 https://$host$request_uri;\n}\n");
    } else {
        out.push_str("server {\n    listen 80 default_server;\n    server_name _;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl default_server;\n    server_name _;\n");
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    out
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
        let jobs = self.runtime_jobs.lock().await.list();
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
                let _ = self
                    .db()
                    .set_runtime_status(&rt.id, &rt.desired_state, &cached);
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
                    let kind = out
                        .get(indices[0])
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
                self.remove_existing_container(&c.id)
                    .await
                    .map_err(SiteError::Internal)
            }
            _ => Err(SiteError::BadRequest(format!(
                "不支持的 Web 服务操作：{action}"
            ))),
        }
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
        self.docker()
            .execute(verb, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| SiteError::Unavailable(format!("容器 {name} 操作失败：{e}")))
    }

    /// 运行环境启停/重建/同步。Node/Go 在 P1 明确返回「P2 未实现」。
    ///
    /// `install` / `rebuild` 只登记后台任务并立即返回；页面轮询任务状态与日志。
    pub async fn runtime_action(self: &Arc<Self>, id: &str, action: &str) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境将在 P2 接入，P1 不提供安装/启停/重建",
                rt.kind
            )));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        match action {
            "install" | "rebuild" => {
                // 重建与安装同一条异步链路：默认读取库里的扩展集并重放。
                self.start_runtime_install(id).await?;
                return Ok(());
            }
            "start" => {
                self.docker_runtime_action(&container, Verb::Start).await?;
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
                self.db()
                    .set_runtime_status(id, "running", "running")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "sync" => {
                let current = self.runtime_view(id).await?;
                // 以容器实测为准，纠正「期望运行但实际停了」的漂移缓存。
                if current.effective_state == "running" {
                    let _ = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "running");
                } else if current.effective_state == "stopped" {
                    let _ = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "stopped");
                }
            }
            other => return Err(SiteError::BadRequest(format!("未知运行环境操作：{other}"))),
        }
        Ok(())
    }

    /// 最近一次任务快照（列表/抽屉轮询）。
    pub async fn runtime_job(&self, id: &str) -> Option<RuntimeJob> {
        self.runtime_jobs.lock().await.get(id).cloned()
    }

    /// 全部运行环境任务（只含有任务记录的环境）。
    pub async fn runtime_jobs_snapshot(&self) -> Vec<RuntimeJob> {
        self.runtime_jobs.lock().await.list()
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

    async fn job_advance(&self, id: &str, step: &str, progress: u8, line: &str) {
        self.runtime_jobs
            .lock()
            .await
            .update(id, |job| job.advance(step, progress, line));
    }

    async fn job_note(&self, id: &str, line: &str) {
        self.runtime_jobs
            .lock()
            .await
            .update(id, |job| job.note(line));
    }

    async fn job_finish_ok(&self, id: &str, line: &str) {
        self.runtime_jobs
            .lock()
            .await
            .update(id, |job| job.finish_ok(line));
    }

    async fn job_finish_err(&self, id: &str, error: &str) {
        self.runtime_jobs
            .lock()
            .await
            .update(id, |job| job.finish_err(error));
    }

    /// 把命令输出按行写入任务日志（只保留尾部，防止一次 pull 撑爆轮询响应）。
    async fn job_log_output(&self, id: &str, output: &str) {
        let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = &lines[lines.len().saturating_sub(30)..];
        for line in tail {
            self.job_note(id, &format!("  {line}")).await;
        }
    }

    /// 启动一次运行环境安装（异步）。只登记任务，立即返回。
    pub async fn start_runtime_install(self: &Arc<Self>, id: &str) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境将在 P2 接入，P1 不提供安装",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        self.runtime_jobs.lock().await.begin(id, "install")?;
        let _ = self.db().set_runtime_status(id, "running", "installing");
        let state = self.clone();
        let rid = id.to_string();
        tokio::spawn(async move {
            state.run_runtime_install_job(&rid).await;
        });
        Ok(())
    }

    async fn run_runtime_install_job(&self, id: &str) {
        let result = self.install_runtime_task(id).await;
        match result {
            Ok(()) => {
                self.job_finish_ok(id, "安装完成，运行环境已就绪").await;
                let _ = self.db().set_runtime_status(id, "running", "running");
                // 新容器可能换了 id；清一次缓存，让下一次渲染重新读取真实版本。
            }
            Err(err) => {
                self.job_finish_err(id, &err).await;
                // 记录失败但保留「期望运行」，这样重试入口与漂移提示仍然成立。
                let _ = self.db().set_runtime_status(id, "running", "error");
            }
        }
    }

    /// 拉镜像：先 `inspect`，本地已有就直接回填 digest 并跳过 `ImagePull`。
    ///
    /// - 登记 digest 为空（新环境）：采用本地 tag 的真实 digest 落库，不再冷拉。
    /// - 登记 digest 与本地一致：跳过拉取。
    /// - 登记 digest 与本地不一致（tag 漂移）：按登记 digest 拉取固定版本并提示。
    async fn ensure_runtime_image(
        &self,
        job_id: &str,
        image: &str,
        registered_digest: &str,
    ) -> Result<String, String> {
        let registered = digest_sha256(registered_digest);
        let local = self
            .docker()
            .inspect_image(image)
            .await
            .ok()
            .and_then(|i| i.primary_digest());
        if let Some(local_digest) = &local {
            if registered.is_empty() {
                self.job_note(
                    job_id,
                    &format!(
                        "镜像 {image} 本地已存在（{local_digest}），回填 image_digest 并跳过拉取"
                    ),
                )
                .await;
                return Ok(local_digest.clone());
            }
            if digest_sha256(local_digest) == registered {
                self.job_note(
                    job_id,
                    &format!("镜像 {image} 已存在且与登记 digest 一致（{local_digest}），跳过拉取"),
                )
                .await;
                return Ok(local_digest.clone());
            }
            self.job_note(
                job_id,
                &format!(
                    "镜像 {image} 与登记值漂移：本地 {local_digest}，登记 {registered_digest}；按登记 digest 拉取"
                ),
            )
            .await;
        } else if !registered.is_empty() {
            self.job_note(
                job_id,
                &format!("本地无镜像 {image}，按登记 digest 拉取固定版本"),
            )
            .await;
        }
        let pull_ref = if registered.is_empty() {
            image.to_string()
        } else {
            pinned_image_ref(image, registered_digest)
        };
        self.job_advance(
            job_id,
            "拉取镜像",
            30,
            &format!("拉取镜像 {pull_ref}（首次可能较慢）"),
        )
        .await;
        self.docker()
            .execute(
                Verb::ImagePull,
                None,
                DockerParams::ImagePull(pull_ref.clone()),
            )
            .await
            .map_err(|e| format!("拉取 {pull_ref} 失败：{e}"))?;
        let digest = self
            .docker()
            .inspect_image(&pull_ref)
            .await
            .ok()
            .and_then(|i| i.primary_digest());
        let digest = match digest {
            Some(d) if !registered.is_empty() && digest_sha256(&d) != registered => {
                return Err(format!(
                    "拉取 {pull_ref} 后 digest 与登记值不一致：期望 {registered_digest}，实际 {d}"
                ));
            }
            Some(d) => d,
            None if !registered.is_empty() => registered_digest.to_string(),
            None => {
                self.job_note(
                    job_id,
                    &format!("警告：{pull_ref} 拉取成功但读不到 repo digest，image_digest 暂为空"),
                )
                .await;
                String::new()
            }
        };
        self.job_note(
            job_id,
            &format!("镜像 {pull_ref} 已就绪（digest {digest}）"),
        )
        .await;
        Ok(digest)
    }

    /// 安装主体：目录 → FPM 池 → 拉镜像 → 建/启容器 → socket → 扩展。
    async fn install_runtime_task(&self, id: &str) -> Result<(), String> {
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

        self.job_advance(id, "检查 Docker", 8, "检查 Docker 可用性")
            .await;
        if !self.docker().available() {
            return Err(format!(
                "Docker 不可用：{}",
                self.docker_unavailable_reason()
            ));
        }
        let s = self.site_settings();
        if rt.kind == skiff_store::KIND_RUST {
            return self.install_rust_runtime_task(id, &rt, &s).await;
        }
        // PHP 扩展安装器随包分发：先落到共享 web_root，容器内 `/www/skiff/tools`
        // 直接可见；运行时不再依赖 GitHub。
        self.ensure_php_extension_installer(&s)
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let cfg = PhpRuntimeConfig::from_json(&rt.config_json);
        cfg.validate().map_err(|e| e.to_string())?;

        self.job_advance(
            id,
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
        std::fs::write(
            s.runtime_pool_conf(&rt.kind, &rt.name),
            render_php_pool_with(&cfg, &socket_container),
        )
        .map_err(|e| format!("写入 PHP-FPM 配置失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            id,
            "创建容器",
            50,
            &format!("创建容器 {container}（已有同名容器会先移除）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(&c.id)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.php_spec_for(
            &s,
            &rt.kind,
            &rt.name,
            &pinned_image_ref(&image, &image_digest),
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        let enabled = runtime_catalog::split_extensions(&rt.extensions);
        if enabled.is_empty() {
            self.job_advance(
                id,
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
                id,
                "安装扩展",
                65,
                &format!("安装 {} 个扩展：{}", enabled.len(), enabled.join("、")),
            )
            .await;
            self.run_extension_scripts(id, &container, &enabled, &[], 65, 92)
                .await?;
            self.job_advance(id, "重载 PHP-FPM", 96, "重启容器以加载扩展")
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
        id: &str,
        rt: &Runtime,
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
        self.job_advance(id, "准备工程", 15, "写入最小 HTTP 工程（无第三方依赖）")
            .await;
        std::fs::write(app_dir.join("Cargo.toml"), rust_cargo_manifest())
            .map_err(|e| format!("写入 Cargo.toml 失败：{e}"))?;
        std::fs::write(
            app_dir.join("src/main.rs"),
            rust_main_source(&rt.version, port),
        )
        .map_err(|e| format!("写入 main.rs 失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            id,
            "创建容器",
            50,
            &format!("创建容器 {container}（host 网络，端口 {port}）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(&c.id)
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
            id,
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
        self.job_note(id, &format!("端口 {port} 已就绪，Rust 进程返回内容"))
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
            let (code, out) = self
                .docker()
                .exec_capture(container, vec!["sh".into(), "-lc".into(), script])
                .await
                .map_err(|e| format!("批量安装扩展执行失败：{e}"))?;
            self.job_log_output(job_id, &out).await;
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
            let (code, out) = self
                .docker()
                .exec_capture(container, vec!["sh".into(), "-lc".into(), script])
                .await
                .map_err(|e| format!("批量停用扩展执行失败：{e}"))?;
            self.job_log_output(job_id, &out).await;
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
    ) -> Result<bool, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(
                "Node.js/Go 运行环境将在 P2 接入，暂不支持扩展管理".into(),
            ));
        }
        let desired = runtime_catalog::normalize_extensions(selected);
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if desired == current {
            return Ok(false);
        }
        self.runtime_jobs.lock().await.begin(id, "extensions")?;
        let state = self.clone();
        let rid = id.to_string();
        tokio::spawn(async move {
            state.run_runtime_extensions_job(&rid, desired).await;
        });
        Ok(true)
    }

    async fn run_runtime_extensions_job(&self, id: &str, desired: Vec<String>) {
        let result = self.extensions_task(id, &desired).await;
        match result {
            Ok(()) => {
                self.job_finish_ok(id, "扩展已更新，PHP-FPM 已重载").await;
                let _ = self.db().set_runtime_status(id, "running", "running");
            }
            Err(err) => {
                self.job_finish_err(id, &err).await;
                // 扩展失败不改变容器可用性：状态回 running，错误留在任务里可重试。
                let _ = self.db().set_runtime_status(id, "running", "running");
            }
        }
    }

    /// 安装单个扩展（目录项或 `pecl:...`）。异步任务，立即返回；已启用返回 false。
    pub async fn install_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
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
        self.runtime_jobs.lock().await.begin_ext(id, token)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        tokio::spawn(async move {
            state.run_single_extension_job(&rid, &target, true).await;
        });
        Ok(true)
    }

    /// 卸载单个扩展：删 conf.d 里的 ini → 重启容器 → 从库里的启用集合移除。
    pub async fn uninstall_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
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
        self.runtime_jobs.lock().await.begin_ext(id, token)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        tokio::spawn(async move {
            state.run_single_extension_job(&rid, &target, false).await;
        });
        Ok(true)
    }

    /// 按名字安装 PECL 扩展（版本可选），例如 `swoole` / `swoole 5.1.2`。
    pub async fn install_pecl_extension(
        self: &Arc<Self>,
        id: &str,
        name: &str,
        version: &str,
    ) -> Result<bool, SiteError> {
        let token = runtime_catalog::pecl_token(name, version);
        if runtime_catalog::parse_pecl_token(&token).is_none() {
            return Err(SiteError::BadRequest(
                "PECL 扩展名只能包含小写字母、数字与下划线".into(),
            ));
        }
        self.install_extension(id, &token).await
    }

    /// 批量勾选安装/卸载：一次任务内按目录顺序串行执行，日志合并，只重启一次容器。
    ///
    /// 返回真正发生变化的扩展数（全部已处于目标状态时为 0，不创建任务）。
    pub async fn apply_extension_selection(
        self: &Arc<Self>,
        id: &str,
        add: bool,
        keys: &[String],
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
        self.runtime_jobs.lock().await.begin_exts(id, &changed)?;
        let state = self.clone();
        let rid = id.to_string();
        let targets = changed.clone();
        tokio::spawn(async move {
            state
                .run_batch_extensions_job(&rid, &desired, &targets)
                .await;
        });
        Ok(changed.len())
    }

    async fn run_batch_extensions_job(&self, id: &str, desired: &[String], targets: &[String]) {
        let result = self.extensions_task(id, desired).await;
        match result {
            Ok(()) => {
                self.job_finish_ok(
                    id,
                    &format!(
                        "已应用 {} 项扩展变更（{}）；PHP-FPM 已重载",
                        targets.len(),
                        targets.join("、")
                    ),
                )
                .await;
                let _ = self.db().set_runtime_status(id, "running", "running");
            }
            Err(err) => {
                self.job_finish_err(id, &err).await;
                let _ = self.db().set_runtime_status(id, "running", "running");
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

    async fn run_single_extension_job(&self, id: &str, token: &str, add: bool) {
        let result = self.single_extension_task(id, token, add).await;
        let label = runtime_catalog::extension_label(token);
        match result {
            Ok(()) => {
                self.job_finish_ok(
                    id,
                    &format!(
                        "扩展 {label} {}，PHP-FPM 已重载",
                        if add { "已启用" } else { "已停用" }
                    ),
                )
                .await;
                let _ = self.db().set_runtime_status(id, "running", "running");
            }
            Err(err) => {
                self.job_finish_err(id, &err).await;
                let _ = self.db().set_runtime_status(id, "running", "running");
            }
        }
    }

    async fn single_extension_task(&self, id: &str, token: &str, add: bool) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{id}"))?;
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
        self.job_advance(
            id,
            if add { "安装扩展" } else { "卸载扩展" },
            15,
            &format!("{label}：{script}"),
        )
        .await;
        let (code, out) = self
            .docker()
            .exec_capture(&container, vec!["sh".into(), "-lc".into(), script])
            .await
            .map_err(|e| format!("{label} 执行失败：{e}"))?;
        self.job_log_output(id, &out).await;
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
        self.job_advance(id, "重载 PHP-FPM", 80, "重启容器以加载扩展变更")
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
            .set_runtime_extensions(id, &joined)
            .map_err(|e| format!("写入扩展集合失败：{e}"))?;
        Ok(())
    }

    async fn extensions_task(&self, id: &str, desired: &[String]) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{id}"))?;
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
            self.job_advance(id, "准备重建", 10, "容器不存在，改为完整安装并重放扩展")
                .await;
            let joined = runtime_catalog::join_extensions(desired);
            self.db()
                .set_runtime_extensions(id, &joined)
                .map_err(|e| format!("写入扩展集合失败：{e}"))?;
            let _ = self
                .runtime_installs
                .try_begin(&format!("{}:{}", rt.kind, rt.name));
            return self.install_runtime_task(id).await;
        }

        self.job_advance(id, "应用扩展变更", 20, "对比扩展集合，执行启用/停用")
            .await;
        self.run_extension_scripts(id, &container, desired, &current, 20, 60)
            .await?;
        self.job_advance(id, "重载 PHP-FPM", 85, "重启容器以加载扩展变更")
            .await;
        self.docker_runtime_action(&container, Verb::Restart)
            .await
            .map_err(|e| format!("重启 PHP-FPM 失败：{e}"))?;
        let s = self.site_settings();
        self.secure_php_socket(&s, &rt.kind, &rt.name)
            .await
            .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        let joined = runtime_catalog::join_extensions(desired);
        self.db()
            .set_runtime_extensions(id, &joined)
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
        // Rust 是「独立进程 + 固定端口」：建环境时一次性分配，冲突直接拒绝。
        let port = if kind == skiff_store::KIND_RUST {
            let used: Vec<u16> = self
                .db()
                .list_runtimes()
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .iter()
                .filter_map(|r| r.port)
                .collect();
            Some(runtime_catalog::allocate_rust_port(&used).ok_or_else(|| {
                SiteError::Conflict(format!(
                    "Rust 端口区间 {}–{} 已用满，请先删除不再使用的环境",
                    *runtime_catalog::RUST_PORT_RANGE.start(),
                    *runtime_catalog::RUST_PORT_RANGE.end()
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
            self.start_runtime_install(&rt.id).await?;
        }
        Ok((rt, started))
    }

    /// 保存 PHP 专属配置：校验 → 写池配置 → reload FPM → 落库；任一步失败回滚文件。
    pub async fn save_php_runtime_config(
        &self,
        id: &str,
        cfg: &PhpRuntimeConfig,
    ) -> Result<RuntimeView, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境将在 P2 接入，P1 不提供专属配置",
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
        std::fs::write(&pool, render_php_pool_with(cfg, &socket_container))
            .map_err(|e| SiteError::Internal(format!("写入 PHP-FPM 配置失败：{e}")))?;
        if let Err(e) = self
            .docker_runtime_action(&runtime_container_name(&rt.kind, &rt.name), Verb::Restart)
            .await
        {
            if let Some(bytes) = &original {
                let _ = std::fs::write(&pool, bytes);
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

    /// 解绑单个站点：停用其 vhost 并转为静态（文件不删）；PHP 源码不会被静态直出。
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
                let _ = self
                    .docker()
                    .exec_capture(&c.id, vec!["nginx".into(), "-s".into(), "reload".into()])
                    .await;
            }
        }
        Ok(())
    }

    /// 删除运行环境：有站点绑定时拒绝并列出域名，绝不隐式解绑/级联删除。
    ///
    /// 真正清理的顺序：容器 stop → remove → 运行目录（含 FPM 池配置、socket、
    /// 日志与 Rust 工程）→ DB 行。任一步失败就停下并保留 DB 行，错误里列出
    /// 已完成项，避免「记录删了、容器和目录还在」的静默残留。
    /// 删除不区分 kind：Node/Go 只是未实现安装，已登记的运行环境仍可删除。
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
                    self.docker()
                        .execute(Verb::Stop, Some(&obj), DockerParams::None)
                        .await
                        .map_err(|e| {
                            partial_delete_error(
                                &report.completed,
                                format!("停止容器 {container} 失败：{e}"),
                            )
                        })?;
                    report.completed.push(format!("容器 {container} 已停止"));
                }
                self.docker()
                    .execute(
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
        if rt.kind == skiff_store::KIND_RUST {
            let s = self.site_settings();
            let path = s.runtime_log_dir(&rt.kind, &rt.name).join("server.log");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(tail);
            return Ok(lines[start..].join("\n"));
        }
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Ok("（P2 未实现：Node.js/Go 运行环境尚未接入）".into());
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

/// PHP-FPM 池配置：只监听 unix socket（不暴露 9000），socket 目录在共享的 `/www`。
pub fn render_php_pool_with(cfg: &PhpRuntimeConfig, socket: &str) -> String {
    format!(
        "[www]\n\
         user = www-data\n\
         group = www-data\n\
         listen = {socket}\n\
         listen.owner = root\n\
         ; nginx worker 以 nobody(65534) 运行；只给同组读写，others 无权限。\n\
         listen.group = nobody\n\
         listen.mode = 0660\n\
         pm = {pm}\n\
         pm.max_children = {max_children}\n\
         pm.start_servers = {start_servers}\n\
         pm.min_spare_servers = {min_spare_servers}\n\
         pm.max_spare_servers = {max_spare_servers}\n\
         pm.max_requests = {max_requests}\n\
         php_admin_value[memory_limit] = {memory_limit}\n\
         php_admin_value[upload_max_filesize] = {upload_max_filesize}\n\
         php_admin_value[post_max_size] = {post_max_size}\n\
         php_admin_value[max_execution_time] = {max_execution_time}\n\
         ; 镜像 docker.conf 默认 access.log = /proc/self/fd/2，访问日志经容器 stdout\n\
         ; 落到 dockerd/shims；nginx 侧已记录同一请求，这里显式关闭以免每请求重复写。\n\
         access.log = /dev/null\n\
         catch_workers_output = yes\n",
        pm = cfg.pm,
        max_children = cfg.max_children,
        start_servers = cfg.start_servers,
        min_spare_servers = cfg.min_spare_servers,
        max_spare_servers = cfg.max_spare_servers,
        max_requests = cfg.max_requests,
        memory_limit = cfg.memory_limit,
        upload_max_filesize = cfg.upload_max_filesize,
        post_max_size = cfg.post_max_size,
        max_execution_time = cfg.max_execution_time,
    )
}

/// 安装/默认池：使用默认配置与共享安装 socket。
pub fn render_php_pool() -> String {
    render_php_pool_with(
        &PhpRuntimeConfig::default(),
        &runtime_container_socket(skiff_store::KIND_PHP, &default_php_name()),
    )
}

// ---------------------------------------------------------------------------
// AppState 上的网站服务
// ---------------------------------------------------------------------------

impl AppState {
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
            nginx_image: pick(KEY_NGINX_IMAGE, DEFAULT_NGINX_IMAGE),
            php_image: pick(KEY_PHP_IMAGE, DEFAULT_PHP_IMAGE),
            port,
            registry_mirror: db
                .get_setting(KEY_MIRROR)
                .ok()
                .flatten()
                .unwrap_or_default(),
            waf_mode: db
                .get_setting(KEY_WAF_MODE)
                .ok()
                .flatten()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "off".into()),
            web_root,
        }
    }

    /// 把随包分发的 mlocati 扩展安装器写到 `<web_root>/skiff/tools/`。
    ///
    /// PHP 容器整目录挂载 `web_root`，因此容器内固定路径是
    /// `/www/skiff/tools/install-php-extensions`。内容一致时不重复写，避免每次
    /// 扩展任务都改文件 mtime。
    fn ensure_php_extension_installer(&self, s: &SiteSettings) -> Result<PathBuf, String> {
        let path = s.php_extension_installer();
        write_if_changed(
            &path,
            runtime_catalog::PHP_EXTENSION_INSTALLER.as_bytes(),
            0o755,
        )?;
        Ok(path)
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
        let waf_rules_dir = s.waf_rules_dir().display().to_string();
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
            waf_mode: s.waf_mode,
            waf_rules_dir,
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
            &s.waf_rules_dir(),
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
            .docker()
            .execute(
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
            self.remove_existing_container(&c.id).await?;
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

    async fn remove_existing_container(&self, id: &str) -> Result<(), String> {
        let obj = skiff_docker::ObjectRef {
            id: id.to_string(),
            name: String::new(),
            labels: BTreeMap::new(),
        };
        self.docker()
            .execute(
                Verb::ContainerRemove,
                Some(&obj),
                DockerParams::ContainerRemove { force: true },
            )
            .await
            .map(|_| ())
            .map_err(|e| format!("移除旧容器失败：{e}"))
    }

    async fn ensure_container(&self, spec: &ContainerSpec) -> Result<(), String> {
        let created = self
            .docker()
            .execute(
                Verb::ContainerCreate,
                None,
                DockerParams::ContainerCreate(Box::new(spec.clone())),
            )
            .await
            .map_err(|e| e.to_string())?;
        let id = created
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return Err("create 未返回容器 id".into());
        }
        let obj = skiff_docker::ObjectRef {
            id,
            name: spec.name.clone().unwrap_or_default(),
            labels: BTreeMap::new(),
        };
        self.docker()
            .execute(Verb::Start, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| format!("启动容器失败：{e}"))
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

    fn nginx_spec(&self, s: &SiteSettings) -> ContainerSpec {
        ContainerSpec {
            image: s.nginx_image.clone(),
            name: Some(NGINX_CONTAINER.to_string()),
            command: None,
            entrypoint: None,
            env: BTreeMap::new(),
            labels: skiff_labels("nginx"),
            // host 网络：直接监听宿主 80，不再做 8080→80 端口映射。
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![
                Mount::Bind {
                    source: s.web_root.display().to_string(),
                    target: CONTAINER_WEB_ROOT.to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.conf_dir.display().to_string(),
                    target: s.container_conf_dir().to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.nginx_main_conf().display().to_string(),
                    target: s.container_nginx_main_conf().to_string(),
                    read_only: false,
                },
            ],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        }
    }

    fn php_spec_for(&self, s: &SiteSettings, kind: &str, name: &str, image: &str) -> ContainerSpec {
        ContainerSpec {
            image: image.to_string(),
            name: Some(runtime_container_name(kind, name)),
            command: None,
            entrypoint: None,
            env: BTreeMap::new(),
            labels: skiff_labels("php"),
            // 只走 unix socket，不暴露 9000；host 网络避免额外 bridge。
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![
                Mount::Bind {
                    source: s.web_root.display().to_string(),
                    target: CONTAINER_WEB_ROOT.to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.runtime_pool_conf(kind, name).display().to_string(),
                    target: CONTAINER_PHP_POOL.to_string(),
                    read_only: true,
                },
                Mount::Bind {
                    source: s.system_pkg_cache_dir().display().to_string(),
                    target: "/var/cache/apk".to_string(),
                    read_only: false,
                },
            ],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        }
    }

    fn rust_spec_for(&self, s: &SiteSettings, name: &str, image: &str, port: u16) -> ContainerSpec {
        let manifest = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/app/Cargo.toml",
            skiff_store::KIND_RUST
        );
        let log = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/log/server.log",
            skiff_store::KIND_RUST
        );
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), port.to_string());
        ContainerSpec {
            image: image.to_string(),
            name: Some(runtime_container_name(skiff_store::KIND_RUST, name)),
            // 独立进程 + host 网络端口；容器重启后由 restart=unless-stopped 拉起。
            command: Some(rust_run_command(&manifest, &log)),
            entrypoint: None,
            env,
            labels: skiff_labels("runtime"),
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![Mount::Bind {
                source: s.web_root.display().to_string(),
                target: CONTAINER_WEB_ROOT.to_string(),
                read_only: false,
            }],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        }
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
                "{} 运行环境将在 P2 接入，当前不能生成 vhost",
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
                Some(skiff_store::KIND_RUST) => VhostKind::Process {
                    root: &root,
                    port: runtime.as_ref().and_then(|r| r.port).ok_or_else(|| {
                        SiteError::Unavailable("Rust 运行环境缺少端口，请重新安装".into())
                    })?,
                },
                Some(skiff_store::KIND_NODE) | Some(skiff_store::KIND_GO) => {
                    return Err(SiteError::Unavailable(
                        "Node.js / Go 运行环境将在 P2 接入，当前不能生成 vhost".into(),
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
                Some(skiff_store::KIND_RUST) => VhostKind::Process {
                    root: &root,
                    port: runtime.as_ref().and_then(|r| r.port).ok_or_else(|| {
                        SiteError::Unavailable("Rust 运行环境缺少端口，请重新安装".into())
                    })?,
                },
                Some(skiff_store::KIND_NODE) | Some(skiff_store::KIND_GO) => {
                    return Err(SiteError::Unavailable(
                        "Node.js / Go 运行环境将在 P2 接入，当前不能生成 vhost".into(),
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
        self.docker()
            .execute(Verb::Restart, Some(&obj), DockerParams::None)
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
            if let Ok(out) = std::process::Command::new("openssl")
                .args([
                    &"x509".to_string(),
                    &"-in".to_string(),
                    &cert.display().to_string(),
                    &"-noout".to_string(),
                    &"-subject".to_string(),
                    &"-issuer".to_string(),
                    &"-enddate".to_string(),
                ])
                .output()
            {
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                for line in text.lines() {
                    if let Some(v) = line.strip_prefix("subject=") {
                        info.subject = Some(v.trim().to_string());
                    } else if let Some(v) = line.strip_prefix("issuer=") {
                        info.issuer = Some(v.trim().to_string());
                    } else if let Some(v) = line.strip_prefix("notAfter=") {
                        info.expires = Some(v.trim().to_string());
                    }
                }
            }
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
            let _ = std::fs::remove_file(&tmp_cert);
            let _ = std::fs::remove_file(&tmp_key);
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
                let _ = std::fs::remove_file(p);
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
                        let _ = std::fs::write(p, bytes);
                    }
                    None => {
                        let _ = std::fs::remove_file(p);
                    }
                }
            }
        };

        self.db()
            .set_site_tls(domain, https, force_https)
            .map_err(|e| SiteError::Internal(format!("写入 HTTPS 状态失败：{e}")))?;
        if let Err(e) = self.write_site_vhost(&s, domain) {
            restore(&snapshot);
            let _ = self.db().set_site_tls(domain, old_https, old_force);
            return Err(e);
        }
        if let Err(e) = self.sync_default_conf(&s) {
            restore(&snapshot);
            let _ = self.db().set_site_tls(domain, old_https, old_force);
            return Err(SiteError::Internal(format!("写入默认站点配置失败：{e}")));
        }
        if let Err(e) = self.validate_nginx().await {
            // 校验失败时运行中的 nginx 还没加载新配置，恢复文件即可；
            // 这里**不能再重启**——若失败源于别的坏 conf，重启会让现网 nginx 起不来。
            restore(&snapshot);
            let _ = self.db().set_site_tls(domain, old_https, old_force);
            return Err(e);
        }
        if let Err(e) = self.restart_nginx().await {
            restore(&snapshot);
            let _ = self.db().set_site_tls(domain, old_https, old_force);
            let _ = self.restart_nginx().await;
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
            let _ = std::fs::remove_file(&disabled);
        } else {
            std::fs::write(&disabled, vhost)
                .map_err(|e| SiteError::Internal(format!("写入站点配置失败：{e}")))?;
            let _ = std::fs::remove_file(&conf);
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
                let _ = std::fs::remove_file(&disabled);
            } else {
                std::fs::write(&disabled, vhost)?;
                let _ = std::fs::remove_file(&conf);
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

        // 类型专属校验。P1 允许 PHP 与 Rust 运行环境；Node/Go 一律如实拒绝。
        let mut runtime_id = None;
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
                        "{} 运行环境将在 P2 接入，当前版本不能创建该类站点",
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
                let index = site_index_dir(&root).join("index.php");
                if !index.exists() {
                    std::fs::write(&index, default_probe_php())
                        .map_err(|e| SiteError::Internal(format!("写入 index.php 失败：{e}")))?;
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

        let site = db
            .create_site(&StoreNewSite {
                domain: domain.clone(),
                root_dir: root.display().to_string(),
                site_type: site_type.into(),
                runtime_id,
                app_command: None,
                app_port: None,
                proxy_target,
                proxy_websocket,
            })
            .map_err(|e| SiteError::Conflict(format!("保存站点失败：{e}")))?;
        if let Err(e) = db.add_site_aliases(&domain, &aliases) {
            // 别名写失败时把站点行一起撤回，避免半成品。
            let _ = db.delete_site(&domain);
            return Err(SiteError::Conflict(format!("保存站点别名失败：{e}")));
        }

        // 保证公共配置（默认站投影 + 兜底站 + 站点 include 入口）齐备。
        let rollback = |conf: &Path| {
            let _ = self.db().delete_site(&domain);
            let _ = std::fs::remove_file(conf);
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
            let _ = db.clear_site_aliases(&site.domain);
            let _ = db.add_site_aliases(&site.domain, previous);
            let _ = this.write_site_vhost(&s, &site.domain);
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
            // PHP 站点不能在 FPM 停着时假装启动：先核对运行环境与 socket。
            let rid = site
                .runtime_id
                .as_deref()
                .ok_or_else(|| SiteError::BadRequest("PHP 站点未绑定运行环境".into()))?;
            let rt = self
                .db()
                .runtime_by_id(rid)
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))?;
            if rt.kind != skiff_store::KIND_PHP || rt.last_status != "running" {
                return Err(SiteError::Unavailable(format!(
                    "运行环境 {} 未运行，请先启动运行环境",
                    rt.version
                )));
            }
            let socket = rt.socket_path.as_deref().ok_or_else(|| {
                SiteError::Unavailable("运行环境缺少 socket 路径，请重新安装".into())
            })?;
            if !Path::new(socket).exists() {
                return Err(SiteError::Unavailable(format!(
                    "PHP socket 未就绪：{socket}，请先启动运行环境"
                )));
            }
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
                    let _ = std::fs::rename(&conf, &disabled);
                }
                return Err(SiteError::Conflict(format!(
                    "拒绝启动：站点配置未通过全局 nginx -t（{e}）"
                )));
            }
            if let Err(e) = self.restart_nginx().await {
                if activated {
                    let _ = std::fs::rename(&conf, &disabled);
                    let _ = self.restart_nginx().await;
                }
                return Err(SiteError::Unavailable(format!("启动站点失败：{e}")));
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let env = self.site_env_status().await;
            if env.installed && env.nginx_state != "running" {
                if activated {
                    let _ = std::fs::rename(&conf, &disabled);
                    let _ = self.restart_nginx().await;
                }
                return Err(SiteError::Unavailable(format!(
                    "启动站点失败：nginx 未能保持运行（当前状态 {}）",
                    env.nginx_state
                )));
            }
        } else if conf.exists() {
            std::fs::rename(&conf, &disabled)
                .map_err(|e| SiteError::Internal(format!("停用配置失败：{e}")))?;
            self.restart_nginx().await?;
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
                let _ = std::fs::remove_file(&conf);
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

    pub async fn delete_site(&self, domain: &str) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
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
            let source = format!("site:{}", site.domain);
            let stale: Vec<String> = self
                .db()
                .scheduled_tasks()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.source == source)
                .map(|t| t.id)
                .collect();
            for id in stale {
                if let Err(e) = tasks.delete_task_cascade(&id).await {
                    tracing::warn!(task = %id, error = %e, "删除站点后清理定时任务失败");
                }
            }
        }
        Ok(())
    }

    /// 读取站点日志：宿主文件、时间倒序、可选关键字过滤；读取前先做内核轮转。
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
            if let Err(e) = rotate_if_large(&p, LOG_ROTATE_BYTES) {
                tracing::warn!(path = %p.display(), error = %e, "日志轮转失败");
            }
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
        let _ = std::fs::remove_file(&candidate);
        let _ = std::fs::remove_file(&main);
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
            let _ = std::fs::remove_file(&tmp);
            return Err(SiteError::Internal(format!("替换配置失败：{e}")));
        }
        if !active {
            return Ok(VhostSaveOutcome { reloaded: false });
        }
        if let Err(e) = self.restart_nginx().await {
            // 回滚文件内容，保证磁盘与正在运行的 nginx 保持一致。
            let _ = std::fs::write(&path, original);
            return Err(SiteError::Unavailable(format!(
                "配置已回滚到保存前内容：重载 nginx 失败：{e}"
            )));
        }
        // Docker restart 返回成功不代表 nginx 配置有效：容器可能在启动后立即退出。
        // 留出极短启动窗口再核对状态，失败则回滚并尽量把旧配置重新拉起。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let env = self.site_env_status().await;
        if env.installed && env.nginx_state != "running" {
            let _ = std::fs::write(&path, original);
            let _ = self.restart_nginx().await;
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
            stream
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

/// 只有自家 nginx 容器处于 running 才认为 80 由自己持有。
/// 停止/退出/正在重启的容器不构成「端口已占」，必须重新探测。
fn nginx_container_running(rows: &[ContainerRow]) -> bool {
    rows.iter()
        .any(|c| c.name == NGINX_CONTAINER && c.state == "running")
}

/// 探测宿主端口：绑定失败即视为被占用，并尽力给出占用进程。
pub fn check_port_free(port: u16) -> Result<(), String> {
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(e) => Err(format!(
            "宿主端口 {port} 已被占用（{}），请先停掉占用进程再安装/建站：{e}",
            port_owner(port)
        )),
    }
}

/// 批量探测（目前 80；HTTPS 落地后加 443）。
pub fn ensure_ports_free(ports: &[u16]) -> Result<(), String> {
    for p in ports {
        check_port_free(*p)?;
    }
    Ok(())
}

#[cfg(unix)]
fn port_owner(port: u16) -> String {
    let hex = format!("{port:04X}");
    let mut inodes = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(f) {
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 10 {
                    continue;
                }
                if cols[1].rsplit(':').next() == Some(hex.as_str()) && cols[3] == "0A" {
                    inodes.push(cols[9].to_string());
                }
            }
        }
    }
    if inodes.is_empty() {
        return "未找到监听进程（可能在其他网络命名空间）".into();
    }
    let targets: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else {
                    continue;
                };
                if targets
                    .iter()
                    .any(|t| t.as_str() == target.to_string_lossy().as_ref())
                {
                    let comm = std::fs::read_to_string(entry.path().join("comm"))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|_| "未知进程".into());
                    return format!("pid {name} {comm}");
                }
            }
        }
    }
    "未能解析占用进程（需要 root 读 /proc）".into()
}

#[cfg(not(unix))]
fn port_owner(_port: u16) -> String {
    "当前平台无法解析占用进程".into()
}

/// 读取日志尾部：`keyword` 为空不过滤；输出时间倒序（最新在前）。
fn read_log(path: &Path, tail: usize, keyword: &str) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    let kw = keyword.trim().to_lowercase();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| kw.is_empty() || l.to_lowercase().contains(&kw))
        .rev()
        .take(tail)
        .collect();
    Ok(lines.join("\n"))
}

/// 内核轮转：扫描日志目录，对超过阈值的 `.log` 做 copytruncate。
///
/// 由内核巡检（30s 一次）调用，不依赖宿主 logrotate；nginx 一直持有文件 fd，
/// copytruncate 保留 inode，不会因重开日志丢行。
pub fn rotate_logs(dir: &Path, max_bytes: u64) -> std::io::Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut rotated = 0;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "log") && rotate_if_large(&path, max_bytes)? {
            rotated += 1;
        }
    }
    Ok(rotated)
}

/// 扫描 `sites/` 下一层站点，各自轮转 `log/` 目录（内核巡检用）。
pub fn rotate_site_logs(sites_dir: &Path, max_bytes: u64) -> std::io::Result<usize> {
    if !sites_dir.is_dir() {
        return Ok(0);
    }
    let mut rotated = 0;
    for entry in std::fs::read_dir(sites_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rotated += rotate_logs(&site_log_dir(&path), max_bytes)?;
        }
    }
    Ok(rotated)
}

/// Windows 上杀软/索引服务短暂占用刚写入的文件时 `remove_dir_all` 会返回
/// `Directory not empty`（os error 145）；用有界退避重试，最终仍失败才报错。
fn remove_dir_all_retry(path: &Path) -> std::io::Result<()> {
    let mut delay = std::time::Duration::from_millis(20);
    for attempt in 0..6 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 5 => {
                tracing::debug!(path = %path.display(), error = %e, "删除站点目录失败，重试");
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn rotate_if_large(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.len() < max_bytes {
        return Ok(false);
    }
    let rotated = path.with_extension(format!(
        "{}.1",
        path.extension().and_then(|e| e.to_str()).unwrap_or("log")
    ));
    std::fs::copy(path, &rotated)?;
    std::fs::File::create(path)?;
    Ok(true)
}

/// 内核自建容器的统一标签：`project=sites` 用于识别归属，`protected=true`
/// 保证面板不能停/删自己托管的 Web 服务与运行时容器。
fn skiff_labels(component: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("io.skiff.project".to_string(), "sites".to_string()),
        ("io.skiff.component".to_string(), component.to_string()),
        (skiff_model::LABEL_PROTECTED.to_string(), "true".to_string()),
    ])
}

/// 取镜像的 registry 主机名，失败文案里必须带上它。
pub fn registry_of(image: &str) -> String {
    let mut parts = image.split('/');
    let first = parts.next().unwrap_or("");
    let has_path = parts.next().is_some();
    if has_path && (first.contains('.') || first.contains(':')) {
        first.to_string()
    } else {
        "docker.io".to_string()
    }
}

/// Rust 容器的启动命令。必须用非登录 shell：`sh -lc` 会重置 PATH，
/// rust 镜像里的 cargo 在 `/usr/local/cargo/bin`，实测退化成 `cargo: not found`（退出码 127）。
fn rust_run_command(manifest: &str, log: &str) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!("cargo run --release --manifest-path {manifest} >> {log} 2>&1"),
    ]
}

/// Rust P1 工程：无第三方依赖，`cargo run --release` 离线可编译。
fn rust_cargo_manifest() -> &'static str {
    "[package]\nname = \"skiff-runtime-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
     [[bin]]\nname = \"skiff-runtime-app\"\npath = \"src/main.rs\"\n"
}

/// 最小 HTTP 服务：监听 `$PORT`，任何请求返回一段带版本的环境标识文本。
fn rust_main_source(version: &str, port: u16) -> String {
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::TcpListener;

const VERSION: &str = "{version}";

fn main() {{
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or({port});
    let listener = TcpListener::bind(("0.0.0.0", port)).expect("bind port");
    println!("skiff rust runtime {{}} listening on {{}}", VERSION, port);
    for stream in listener.incoming() {{
        let mut stream = match stream {{
            Ok(s) => s,
            Err(e) => {{
                eprintln!("accept failed: {{e}}");
                continue;
            }}
        }};
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let body = format!(
            "Skiff Rust runtime ready\nversion: {{}}\nport: {{}}\n",
            VERSION, port
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {{}}\r\nConnection: close\r\n\r\n{{}}",
            body.as_bytes().len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }}
}}
"#
    )
}

/// 以 TCP 连接探测端口是否已监听（Rust 等无 socket 环境的就绪判据）。
async fn wait_tcp_port(port: u16, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
            nginx_image: String::new(),
            php_image: String::new(),
            port: 80,
            registry_mirror: String::new(),
            waf_mode: String::new(),
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

        assert_eq!(validate_app_port(2048).unwrap(), 2048);
        assert!(validate_app_port(80).is_err());
        assert!(validate_app_port(70000).is_err());
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
            nginx_image: DEFAULT_NGINX_IMAGE.into(),
            php_image: DEFAULT_PHP_IMAGE.into(),
            port: DEFAULT_PORT,
            registry_mirror: String::new(),
            waf_mode: "off".into(),
        }
    }

    /// 第（无 Docker）测试用应用上下文：数据目录落在临时目录，避免污染源码树。
    fn app_state_at(root: &Path) -> Arc<AppState> {
        use crate::WebConfig;
        use skiff_docker::{DockerService, Form, Policy, Tier};
        use skiff_identity::SessionLimits;
        use skiff_plugin::{Supervisor, SupervisorConfig};
        let db = Arc::new(skiff_store::Db::open_in_memory().unwrap());
        let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
        let sup = Arc::new(Supervisor::new(
            SupervisorConfig::new(root.join("data"), root.join("run")),
            db.clone(),
        ));
        let cfg = WebConfig {
            data_dir: root.join("data"),
            web_root: root.join("www"),
            config_path: root.join("config.toml"),
            listen: "127.0.0.1:18080".parse().unwrap(),
            entry_prefix: "p".into(),
            entry_prefix_previous: None,
            tls_enabled: false,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: true,
            dev_http: true,
            trusted_proxies: vec![],
            external_url: None,
            tier: Tier::Basic,
            form: Form::Native,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            policy: Policy::new(Tier::Basic, Form::Native, vec![]).unwrap(),
            session: SessionLimits::default(),
            login_concurrency: 1,
        };
        AppState::new(db, docker, sup, cfg, "csrf-test".into(), "test")
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
            .runtime_jobs
            .lock()
            .await
            .begin("php-busy", "install")
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
                app_command: None,
                app_port: None,
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
                app_command: None,
                app_port: None,
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
                    app_command: None,
                    app_port: None,
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
        assert_eq!(
            s.waf_rules_dir(),
            PathBuf::from("/www/skiff/waf/rules"),
            "WAF 规则目录为预留位，本期不加载规则"
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
    fn runtime_job_state_transitions_follow_install_lifecycle() {
        let mut jobs = RuntimeJobs::default();
        jobs.begin("php-8.5", "install").expect("首单应入队");
        let queued = jobs.get("php-8.5").cloned().expect("任务应可读取");
        assert_eq!(queued.state, RuntimeJobState::Queued);
        assert!(queued.active(), "排队中视为活动任务");
        assert_eq!(queued.progress, 0);
        assert!(
            jobs.begin("php-8.5", "install").is_err(),
            "同一环境执行中必须拒绝重复任务"
        );

        jobs.update("php-8.5", |j| {
            j.advance("拉取镜像", 15, "pull php:8.5-fpm");
        });
        let running = jobs.get("php-8.5").cloned().unwrap();
        assert_eq!(running.state, RuntimeJobState::Running);
        assert_eq!(running.step, "拉取镜像");
        assert_eq!(running.progress, 15);
        assert!(running.active());

        jobs.update("php-8.5", |j| j.finish_err("Docker 不可用"));
        let failed = jobs.get("php-8.5").cloned().unwrap();
        assert_eq!(failed.state, RuntimeJobState::Failed);
        assert!(!failed.active(), "失败是终态，不再算活动任务");
        assert_eq!(failed.error, "Docker 不可用");
        assert!(
            failed.log_text().contains("Docker 不可用"),
            "失败原因必须进实时日志：{}",
            failed.log_text()
        );

        // 失败后可重试；新任务重新从排队开始，并带上新的 kind。
        assert!(jobs.begin("php-8.5", "extensions").is_ok());
        let retry = jobs.get("php-8.5").cloned().unwrap();
        assert_eq!(retry.state, RuntimeJobState::Queued);
        assert_eq!(retry.kind, "extensions");

        jobs.update("php-8.5", |j| j.finish_ok("完成"));
        let done = jobs.get("php-8.5").cloned().unwrap();
        assert_eq!(done.state, RuntimeJobState::Done);
        assert_eq!(done.progress, 100);
        assert!(!done.active());
        assert_eq!(jobs.list().len(), 1, "每个环境只保留最近一次任务");
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
    fn log_rotation_copytruncates() {
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
        let cmd = rust_run_command(
            "/www/runtime/rust/x/app/Cargo.toml",
            "/www/runtime/rust/x/log/server.log",
        );
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[1], "-c");
        assert!(cmd[2].starts_with("cargo run --release --manifest-path"));
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
                app_command: None,
                app_port: None,
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
