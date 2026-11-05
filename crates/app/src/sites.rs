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
//! PHP socket 在 `/www/php/php-fpm.sock`。
//!
//! 两个容器都整目录挂载 `/www`，并使用 host 网络：nginx 直接监听宿主 80，
//! PHP-FPM 只走 unix socket，不再暴露 9000。容器内路径一律由
//! [`SiteSettings::container_path`] 从宿主路径换算，禁止手拼。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::{ContainerRow, ContainerSpec, DockerParams, Mount, Verb};
use skiff_model::{is_absolute, normalize_dir, within, within_root};
use skiff_store::Site;

use crate::AppState;

pub const NGINX_CONTAINER: &str = "skiff-web-nginx";
pub const PHP_CONTAINER: &str = "skiff-web-php";
/// OpenResty 官方镜像，固定 digest（不复用 1Panel 的 tag）。
pub const DEFAULT_NGINX_IMAGE: &str =
    "openresty/openresty:alpine@sha256:2cb90d64192934c26b3749cf744f47edd59277787fe761bd109eeebdc3e2214a";
pub const DEFAULT_PHP_IMAGE: &str = "php:8.5-fpm-alpine";
/// nginx 用 host 网络直接监听宿主 80；443 预留给 HTTPS。
pub const DEFAULT_PORT: u16 = 80;
pub const DEFAULT_PHP_VERSION: &str = "8.5";
/// 容器内 `/www` 的宿主来源由 `SiteSettings::web_root` 决定。
pub const CONTAINER_WEB_ROOT: &str = "/www";
pub const CONTAINER_SOCKET: &str = "/www/php/php-fpm.sock";
/// socket 固定属组：openresty 镜像的 worker 与 php 镜像都带 `nobody`（gid 65534）。
/// 0660 只放行该组，本机普通用户连不上。
pub const SOCKET_GID: u32 = 65534;
pub const SOCKET_MODE: u32 = 0o660;
/// PHP-FPM 默认池配置在容器内的位置（用宿主生成的文件覆盖它）。
pub const CONTAINER_PHP_POOL: &str = "/usr/local/etc/php-fpm.d/www.conf";
/// 单文件日志轮转阈值（内核自己轮转，不依赖宿主 logrotate）。
pub const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

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
    pub php_dir: PathBuf,
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

    /// 由内核生成、覆盖镜像默认 `www.conf` 的 PHP-FPM 池配置。
    pub fn php_pool_conf(&self) -> PathBuf {
        self.php_dir.join("www.conf")
    }

    /// WAF 规则目录（预留；本期不加载、不拦截）。
    pub fn waf_rules_dir(&self) -> PathBuf {
        self.web_root.join("skiff").join("waf").join("rules")
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

/// 运行环境探测结果（列表页/环境页共用）。
#[derive(Debug, Clone)]
pub struct SiteEnvStatus {
    pub docker_available: bool,
    pub docker_reason: String,
    pub nginx_state: String,
    pub php_state: String,
    pub installed: bool,
    pub port: u16,
    pub port_message: String,
    pub nginx_image: String,
    pub php_image: String,
    pub waf_mode: String,
    pub waf_rules_dir: String,
}

impl SiteEnvStatus {
    pub fn ready(&self) -> bool {
        self.docker_available
            && self.installed
            && self.nginx_state == "running"
            && self.php_state == "running"
    }
}

#[derive(Debug, Clone)]
pub struct NewSite {
    pub domain: String,
    pub root_dir: Option<String>,
    pub php_version: String,
    pub https: bool,
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

/// 只支持环境实际安装的 PHP 版本；目前固定 8.5 系列。
pub fn validate_php_version(raw: &str, installed: &str) -> Result<String, SiteError> {
    let v = raw.trim();
    if v.is_empty() {
        return Ok(DEFAULT_PHP_VERSION.to_string());
    }
    let major_minor = if v.starts_with("8.5") {
        "8.5"
    } else {
        return Err(SiteError::BadRequest(format!(
            "当前运行环境只安装了 PHP {installed}，不支持 {v}"
        )));
    };
    Ok(major_minor.to_string())
}

/// TLS 参数：证书/私钥使用**容器内** `/www/...` 路径。
#[derive(Debug, Clone)]
pub struct VhostTls {
    pub cert: String,
    pub key: String,
    /// 80 端口是否 301 到 443。
    pub force_https: bool,
}

/// 生成 nginx/OpenResty vhost。路径里的站点目录已换算为容器内 `/www/...`；
/// PHP 只走 unix socket，日志落站点自己的 `log/` 目录。
pub fn render_vhost(
    domain: &str,
    container_root: &str,
    access_log: &str,
    error_log: &str,
) -> String {
    render_vhost_with(domain, &[], container_root, access_log, error_log, None)
}

/// 同 `render_vhost`，但把别名并入 `server_name`，并按需追加 443 server。
pub fn render_vhost_with(
    domain: &str,
    aliases: &[String],
    container_root: &str,
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
        push_site_body(&mut out, container_root, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl;\n");
        out.push_str(&format!("    server_name {names};\n"));
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, container_root, access_log, error_log);
    }
    out
}

fn push_site_body(out: &mut String, container_root: &str, access_log: &str, error_log: &str) {
    out.push_str(&format!("    root {container_root};\n"));
    out.push_str("    index index.php index.html;\n");
    out.push_str(&format!("    access_log {access_log};\n"));
    out.push_str(&format!("    error_log {error_log};\n"));
    out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
    out.push_str(
        "    location ~ \\.php$ {\n\
         \x20       include fastcgi_params;\n\
         \x20       fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n",
    );
    out.push_str(&format!("        fastcgi_pass unix:{CONTAINER_SOCKET};\n"));
    out.push_str("        fastcgi_index index.php;\n    }\n");
    out.push_str("}\n");
}

fn default_probe_php() -> &'static str {
    "<?php\n// Skiff 站点探针：确认 PHP-FPM 正常渲染。\nphpinfo();\n"
}

/// 默认站点 vhost：直接访问 IP:80 时落到该站点的 `public`。
///
/// `default_server` 只能有一个；本文件存在时由它接管裸访问，不存在时由
/// `00-skiff-fallback.conf`（按文件名先于站点 vhost 加载）返回 404。
pub fn render_default_vhost(
    domain: &str,
    container_root: &str,
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
        push_site_body(&mut out, container_root, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl default_server;\n    server_name _;\n");
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, container_root, access_log, error_log);
    }
    out
}

/// PHP-FPM 池配置：只监听 unix socket（不暴露 9000），socket 目录在共享的 `/www`。
pub fn render_php_pool() -> String {
    format!(
        "[www]\n\
         user = www-data\n\
         group = www-data\n\
         listen = {CONTAINER_SOCKET}\n\
         listen.owner = root\n\
         ; nginx worker 以 nobody(65534) 运行；只给同组读写，others 无权限。\n\
         listen.group = nobody\n\
         listen.mode = 0660\n\
         pm = dynamic\n\
         pm.max_children = 20\n\
         pm.start_servers = 3\n\
         pm.min_spare_servers = 2\n\
         pm.max_spare_servers = 8\n\
         catch_workers_output = yes\n"
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
            php_dir: web_root.join("php"),
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

    pub async fn site_env_status(&self) -> SiteEnvStatus {
        let s = self.site_settings();
        let mut nginx_state = "missing".to_string();
        let mut php_state = "missing".to_string();
        if self.docker().available() {
            if let Ok(rows) = self.docker().list_containers().await {
                for c in rows {
                    if c.name == NGINX_CONTAINER {
                        nginx_state = if c.state.is_empty() {
                            "unknown".into()
                        } else {
                            c.state.clone()
                        };
                    }
                    if c.name == PHP_CONTAINER {
                        php_state = if c.state.is_empty() {
                            "unknown".into()
                        } else {
                            c.state
                        };
                    }
                }
            }
        }
        let installed = nginx_state != "missing" && php_state != "missing";
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
            php_state,
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
    pub fn start_site_env_install(self: &Arc<Self>) {
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
                    "拉取 OpenResty 镜像",
                    "拉取 PHP 镜像",
                    "准备目录与 PHP socket 配置",
                    "创建并启动 nginx（host 网络）",
                    "创建并启动 PHP-FPM（unix socket）",
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
            let result = state.run_site_env_install().await;
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

    async fn run_site_env_install(&self) -> Result<(), String> {
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

        // 绑定挂载的源目录必须先存在，否则 Docker 直接 400；权限 0755 让容器内
        // 非 root 的 nginx/php 进程可读。PHP socket 池配置也在这一步落盘。
        // 只建公共目录；站点级 index/log/ssl/conf 由建站时创建。
        let dirs = [
            &s.web_root,
            &s.sites_dir,
            &s.conf_dir,
            &s.php_dir,
            &s.waf_rules_dir(),
        ];
        for dir in dirs {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return fail(0, format!("创建 {} 失败：{e}", dir.display())).await;
            }
        }
        if let Err(e) = std::fs::write(s.php_pool_conf(), render_php_pool()) {
            return fail(
                3,
                format!(
                    "写入 PHP-FPM 池配置 {} 失败：{e}",
                    s.php_pool_conf().display()
                ),
            )
            .await;
        }
        if let Err(e) = self.write_default_vhost(&s) {
            return fail(0, format!("写入默认配置失败：{e}")).await;
        }

        self.mark_step(1, StepState::Running, format!("registry: {registry}"))
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
                1,
                format!("拉取 {} 失败（registry: {registry}）：{e}", s.nginx_image),
            )
            .await;
        }
        self.mark_step(1, StepState::Ok, format!("{} 已就绪", s.nginx_image))
            .await;

        let php_registry = registry_of(&s.php_image);
        self.mark_step(2, StepState::Running, format!("registry: {php_registry}"))
            .await;
        if let Err(e) = self
            .docker()
            .execute(
                Verb::ImagePull,
                None,
                DockerParams::ImagePull(s.php_image.clone()),
            )
            .await
        {
            return fail(
                2,
                format!("拉取 {} 失败（registry: {php_registry}）：{e}", s.php_image),
            )
            .await;
        }
        self.mark_step(2, StepState::Ok, format!("{} 已就绪", s.php_image))
            .await;

        self.mark_step(3, StepState::Running, "").await;
        // 按当前 `web_root` 布局重建 vhost；顺序放在启动 nginx 之前，nginx 起来时
        // 即可直接加载。
        let rebuilt = match self.rewrite_site_vhosts(&s) {
            Ok(n) => n,
            Err(e) => return fail(3, format!("重建站点配置失败：{e}")).await,
        };
        self.mark_step(
            3,
            StepState::Ok,
            format!("PHP socket: {CONTAINER_SOCKET}，重建 {rebuilt} 个 vhost"),
        )
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

        self.mark_step(5, StepState::Running, "准备 PHP-FPM 容器")
            .await;
        if let Some(c) = existing.iter().find(|c| c.name == PHP_CONTAINER) {
            self.remove_existing_container(&c.id).await?;
        }
        if let Err(e) = self.ensure_container(&self.php_spec(&s)).await {
            return fail(5, format!("创建/启动 PHP-FPM 失败：{e}")).await;
        }
        // 不用 0666 兜底：等 socket 出现后由内核收口到 0660 + 固定 gid。
        if let Err(e) = self.secure_php_socket(&s).await {
            return fail(5, format!("收紧 PHP socket 权限失败：{e}")).await;
        }
        self.mark_step(
            5,
            StepState::Ok,
            format!("{PHP_CONTAINER}，socket {CONTAINER_SOCKET} 0660"),
        )
        .await;

        self.mark_step(6, StepState::Running, "").await;
        if let Err(e) = self.write_default_vhost(&s) {
            return fail(6, format!("写入默认配置失败：{e}")).await;
        }
        self.mark_step(6, StepState::Ok, s.conf_dir.display().to_string())
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
    async fn secure_php_socket(&self, s: &SiteSettings) -> Result<(), String> {
        let path = s.php_dir.join("php-fpm.sock");
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
            ],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        }
    }

    fn php_spec(&self, s: &SiteSettings) -> ContainerSpec {
        ContainerSpec {
            image: s.php_image.clone(),
            name: Some(PHP_CONTAINER.to_string()),
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
                    source: s.php_pool_conf().display().to_string(),
                    target: CONTAINER_PHP_POOL.to_string(),
                    read_only: true,
                },
            ],
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

    /// 把 `00-skiff-default.conf` 与库里的默认站点对齐：有则生成，无则删除。
    fn sync_default_conf(&self, s: &SiteSettings) -> std::io::Result<()> {
        let conf = s.conf_dir.join(DEFAULT_CONF);
        match self.db().default_site() {
            Ok(Some(site)) => {
                let site_dir = PathBuf::from(&site.root_dir);
                let container_root = s.container_path(&site_index_dir(&site_dir));
                let tls = self.site_tls(s, &site);
                std::fs::write(
                    conf,
                    render_default_vhost(
                        &site.domain,
                        &container_root,
                        &s.container_log(&site_dir, LogKind::Access),
                        &s.container_log(&site_dir, LogKind::Error),
                        tls.as_ref(),
                    ),
                )
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
        let vhost = render_vhost_with(
            &site.domain,
            &aliases,
            &s.container_path(&site_index_dir(&site_dir)),
            &s.container_log(&site_dir, LogKind::Access),
            &s.container_log(&site_dir, LogKind::Error),
            tls.as_ref(),
        );
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
            let vhost = render_vhost_with(
                &site.domain,
                &aliases,
                &s.container_path(&site_index_dir(&site_dir)),
                &s.container_log(&site_dir, LogKind::Access),
                &s.container_log(&site_dir, LogKind::Error),
                tls.as_ref(),
            );
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
        // 别名先做语法与去重校验，再查库冲突；拒绝与主域名重复。
        let mut aliases: Vec<String> = Vec::new();
        for raw in &req.aliases {
            let alias = validate_domain(raw)?;
            if alias == domain {
                return Err(SiteError::BadRequest("别名不能与主域名相同".into()));
            }
            if aliases.contains(&alias) {
                return Err(SiteError::Conflict(format!("别名重复：{alias}")));
            }
            aliases.push(alias);
        }
        let s = self.site_settings();
        let env = self.site_env_status().await;
        if !env.ready() {
            return Err(SiteError::Unavailable(
                "运行环境未就绪：请先在「网站 → 运行环境」安装/启动 nginx 与 PHP-FPM".into(),
            ));
        }
        let php_installed = DEFAULT_PHP_VERSION;
        let php_version = validate_php_version(&req.php_version, php_installed)?;
        if req.https {
            // 一期只做明文；显式告知而不是悄悄忽略。
            return Err(SiteError::BadRequest(
                "当前版本只支持 HTTP 明文站点；HTTPS 证书接入尚未开放".into(),
            ));
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
        // 一站一目录：index/（文档根）、log/、ssl/、conf/ 全部在站点目录内。
        for dir in [
            &root,
            &site_index_dir(&root),
            &site_log_dir(&root),
            &site_ssl_dir(&root),
            &site_conf_dir(&root),
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| SiteError::Internal(format!("创建 {} 失败：{e}", dir.display())))?;
        }
        let index = site_index_dir(&root).join("index.php");
        if !index.exists() {
            std::fs::write(&index, default_probe_php())
                .map_err(|e| SiteError::Internal(format!("写入 index.php 失败：{e}")))?;
        }

        // 保证公共配置（默认站投影 + 兜底站 + 站点 include 入口）齐备。
        self.write_default_vhost(&s)
            .map_err(|e| SiteError::Internal(format!("写入公共配置失败：{e}")))?;
        let vhost = render_vhost_with(
            &domain,
            &aliases,
            &s.container_path(&site_index_dir(&root)),
            &s.container_log(&root, LogKind::Access),
            &s.container_log(&root, LogKind::Error),
            None,
        );
        let conf = s.vhost_file(&root, &domain);
        std::fs::write(&conf, &vhost)
            .map_err(|e| SiteError::Internal(format!("写入 vhost 失败：{e}")))?;

        if let Err(e) = self.restart_nginx().await {
            let _ = std::fs::remove_file(&conf);
            return Err(e);
        }

        match self
            .db()
            .create_site(&domain, &root.display().to_string(), &php_version)
        {
            Ok(site) => {
                if let Err(e) = self.db().add_site_aliases(&domain, &aliases) {
                    // 别名写失败时把站点行与 vhost 一起撤回，避免半成品。
                    let _ = self.db().delete_site(&domain);
                    let _ = std::fs::remove_file(&conf);
                    return Err(SiteError::Conflict(format!("保存站点别名失败：{e}")));
                }
                Ok(site)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&conf);
                Err(SiteError::Conflict(format!("保存站点失败：{e}")))
            }
        }
    }

    pub fn site_aliases(&self, domain: &str) -> Vec<String> {
        self.db().site_aliases(domain).unwrap_or_default()
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
        if running {
            if !conf.exists() && disabled.exists() {
                std::fs::rename(&disabled, &conf)
                    .map_err(|e| SiteError::Internal(format!("启用配置失败：{e}")))?;
            }
            if !conf.exists() {
                return Err(SiteError::Internal(
                    "站点配置缺失：请重新安装运行环境以重建 vhost".into(),
                ));
            }
        } else if conf.exists() {
            std::fs::rename(&conf, &disabled)
                .map_err(|e| SiteError::Internal(format!("停用配置失败：{e}")))?;
        }
        self.restart_nginx().await?;
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
            let site_dir = PathBuf::from(&site.root_dir);
            let container_root = s.container_path(&site_index_dir(&site_dir));
            let tls = self.site_tls(&s, &site);
            std::fs::write(
                &conf,
                render_default_vhost(
                    &site.domain,
                    &container_root,
                    &s.container_log(&site_dir, LogKind::Access),
                    &s.container_log(&site_dir, LogKind::Error),
                    tls.as_ref(),
                ),
            )
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

    /// 保存 vhost 并 reload；reload 失败时把文件回滚到保存前内容。
    pub async fn save_site_vhost(&self, domain: &str, content: &str) -> Result<(), SiteError> {
        if content.trim().is_empty() {
            return Err(SiteError::BadRequest("配置内容不能为空".into()));
        }
        if !content.contains("server") {
            return Err(SiteError::BadRequest(
                "配置至少需要包含一个 server 块".into(),
            ));
        }
        let (path, _) = self.site_vhost_path(domain)?;
        let original = std::fs::read(&path)
            .map_err(|e| SiteError::Internal(format!("读取原配置失败：{e}")))?;
        // 先写临时文件再 rename，避免半截文件被 nginx 加载。
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, content)
            .map_err(|e| SiteError::Internal(format!("写入临时配置失败：{e}")))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(SiteError::Internal(format!("替换配置失败：{e}")));
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
        Ok(())
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

fn skiff_labels(component: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("io.skiff.project".to_string(), "sites".to_string()),
        ("io.skiff.component".to_string(), component.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn php_version_only_matches_installed() {
        assert_eq!(validate_php_version("8.5", "8.5").unwrap(), "8.5");
        assert_eq!(validate_php_version("8.5.14", "8.5").unwrap(), "8.5");
        assert!(validate_php_version("7.4", "8.5").is_err());
        assert!(validate_php_version("8.4", "8.5").is_err());
    }

    #[test]
    fn vhost_points_fastcgi_at_unix_socket() {
        let v = render_vhost(
            "example.com",
            "/www/sites/example.com/index",
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
        );
        assert!(v.contains("server_name example.com;"));
        assert!(v.contains("root /www/sites/example.com/index;"));
        assert!(v.contains("fastcgi_pass unix:/www/php/php-fpm.sock;"));
        assert!(!v.contains("9000"));
        assert!(v.contains("SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
        assert!(v.contains("access_log /www/sites/example.com/log/access.log;"));
        assert!(v.contains("error_log /www/sites/example.com/log/error.log;"));
        assert!(!v.contains("/www/logs/"), "不得再引用旧的集中日志目录");
        assert!(!v.contains("/public"), "不得再引用旧的 public 根目录");
        assert!(!v.contains("listen 443"));
        assert!(v.contains("listen 80;"));
    }

    fn settings_at(root: &Path) -> SiteSettings {
        SiteSettings {
            web_root: root.to_path_buf(),
            sites_dir: root.join("sites"),
            conf_dir: root.join("nginx").join("conf.d"),
            php_dir: root.join("php"),
            nginx_image: DEFAULT_NGINX_IMAGE.into(),
            php_image: DEFAULT_PHP_IMAGE.into(),
            port: DEFAULT_PORT,
            registry_mirror: String::new(),
            waf_mode: "off".into(),
        }
    }

    #[test]
    fn container_path_is_under_shared_web_root() {
        let s = settings_at(Path::new("/www"));
        assert_eq!(
            s.container_path(Path::new("/www/sites/example.com")),
            "/www/sites/example.com"
        );
        assert_eq!(s.container_sites_dir(), "/www/sites");
        assert_eq!(s.php_pool_conf(), PathBuf::from("/www/php/www.conf"));
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
        assert!(pool.contains("listen = /www/php/php-fpm.sock"));
        assert!(pool.contains("listen.group = nobody"));
        assert!(pool.contains("listen.mode = 0660"));
        assert!(!pool.contains("0666"), "socket 不得 world 可写");
        assert!(!pool.contains("9000"));
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
        let v = render_vhost_with(
            "example.com",
            &[],
            "/www/sites/example.com/index",
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
        let v = render_vhost_with(
            "example.com",
            &[],
            "/www/sites/example.com/index",
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
            "/www/sites/demo.local/index",
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
            "/www/sites/demo.local/index",
            "/www/sites/demo.local/log/access.log",
            "/www/sites/demo.local/log/error.log",
            None,
        );
        assert!(v.contains("listen 80 default_server;"));
        assert!(v.contains("server_name _;"));
        assert!(v.contains("root /www/sites/demo.local/index;"));
        assert!(v.contains("fastcgi_pass unix:/www/php/php-fpm.sock;"));

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
            state: state.into(),
            status: String::new(),
            labels: BTreeMap::new(),
            project: None,
            owner: None,
            protected: false,
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
}
