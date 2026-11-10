//! `ftp` 能力面：宿主机 pure-ftpd 的安装、站点账号与 uid/gid 对齐。
//!
//! 与 `firewall` 能力面一样，这是**宿主特权**原语：装包、写 `/etc/pure-ftpd`、
//! 调 `pure-pw` 都需要 root，插件进程始终降权运行。能力面的判定、参数校验与
//! 命令编排全部收在这里（`crates/plugin`），内核只提供两条通用通道：
//!
//! - [`FtpHost::run`]：以固定二进制 + 参数数组执行特权命令（不经 shell）；
//! - 鉴权/审计/站点数据查询：由内核 handler 完成，本模块只吃 [`FtpContext`]，
//!   不碰数据库。
//!
//! 设计边界：
//! - 站点快照由内核注入（`FtpSite`），插件不查库，也不自己推导容器名；
//! - 命令一律用参数数组执行，禁止拼 shell；密码只经 stdin 交给 `pure-pw`，
//!   不入 argv、日志或审计；
//! - 服务配置以内核文件 `/etc/skiff/ftp-service.json` 为**唯一真相来源**；
//! - 站点账号的 chroot 根固定为 `<site_root>/index`，只认内核注入的站点与
//!   `sites_root` 下的真实目录；FTP 账号看不到 `conf/`、`log/`、`ssl/`，
//!   也到不了别的站点。
//!
//! Windows 开发机没有这些宿主命令，非 Linux 构建只保留纯校验逻辑（单测在这里
//! 跑，真实动作一律被 `#[cfg(target_os = "linux")]` 挡住）。
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::future::Future;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::pin::Pin;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "linux", test))]
use serde_json::json;
use serde_json::Value;
#[cfg(target_os = "linux")]
use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

const PASSWD_FILE: &str = "/etc/pure-ftpd/pureftpd.passwd";
const PDB_FILE: &str = "/etc/pure-ftpd/pureftpd.pdb";
const CONF_DIR: &str = "/etc/pure-ftpd/conf";
const DEFAULT_FILE: &str = "/etc/default/pure-ftpd-common";
const TLS_PEM: &str = "/etc/ssl/private/pure-ftpd.pem";
const JAIL_FILE: &str = "/etc/fail2ban/jail.d/skiff-pureftpd.conf";
const SERVICE: &str = "pure-ftpd";
const LOGIN_PREFIX: &str = "skiff_";
const DEFAULT_PORT_START: u16 = 30000;
const DEFAULT_PORT_END: u16 = 30100;
/// `/etc/pure-ftpd/conf/Umask` 的格式是「两个三位八进制数、空白分隔」
/// （wrapper 的正则硬要求），由 wrapper 拼成 `-U <files>:<dirs>`。
///
/// pure-ftpd 以 `0777` 为基准减 umask，**文件也吃同一基准**：
/// 只写 `002 002` 时上传文件会落成 `0775`（带执行位），与 [`align_tree`] 归一后
/// 的 `0664` 不一致。文件掩码取 `113` → `0777 & !0113 = 0664`；目录仍是 `002`
/// → `0775`，再加上父目录继承的 setgid 即 `2775`，与 align 结果同形，
/// 新建文件无需等归一。
const UMASK: &str = "113 002";
/// `pure-pw` 在 Debian/Ubuntu 落在 `/usr/bin`，部分发行版在 `/usr/sbin`。
const PURE_PW_CANDIDATES: [&str; 2] = ["/usr/bin/pure-pw", "/usr/sbin/pure-pw"];
/// Debian 的认证链由 `/etc/pure-ftpd/auth` 下的符号链接决定（conf 值只是开关）。
const AUTH_DIR: &str = "/etc/pure-ftpd/auth";
/// 指向 PureDB 的链接必须用相对路径，与包自带链接保持同一约定。
const AUTH_DB_LINK: &str = "../conf/PureDB";
/// 包自带的 Unix/PAM 认证链接：虚拟账号不应走 PAM。
const AUTH_STALE: [&str; 2] = ["65unix", "70pam"];
/// 内核持久化的 FTP 服务配置；`/etc/skiff` 在 systemd unit 的 `ReadWritePaths` 内。
const SERVICE_CONFIG_FILE: &str = "/etc/skiff/ftp-service.json";
/// 允许作为 TLS 源文件的最大尺寸，避免把大文件读进内存。
const MAX_TLS_SOURCE_BYTES: u64 = 1024 * 1024;
const DEFAULT_LISTEN_PORT: u16 = 21;
const DEFAULT_MAX_CLIENTS: u32 = 30;
const DEFAULT_MAX_PER_IP: u32 = 3;

/// 宿主命令输出（非零退出也算成功执行，由调用方判断 `success`）。
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 特权命令执行通道。内核实现它（systemd-run 瞬态单元），本模块只声明契约。
pub trait FtpHost: Send + Sync {
    fn run<'a>(
        &'a self,
        cmd: &'a str,
        args: &'a [String],
        stdin: Option<&'a str>,
        timeout_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>>;
}

/// 站点快照，由内核侧从 DB 读出后注入，插件不查库。
pub struct FtpSite {
    pub domain: String,
    /// `runtime` / `static` / `proxy`。
    pub site_type: String,
    pub root_dir: String,
    /// 仅 `runtime` 站点有值；运行环境缺失时为 `missing`。
    pub runtime_kind: Option<String>,
    /// 仅 `runtime` 站点有值。
    pub runtime_name: Option<String>,
    /// 仅 `runtime` 站点有值，内核按现有规则算好。
    pub container: Option<String>,
}

/// 能力面所需的站点快照与站点根目录。
pub struct FtpContext {
    pub sites: Vec<FtpSite>,
    pub sites_root: std::path::PathBuf,
}

/// 能力面错误。`status` 用 HTTP 语义，内核 handler 直接映射成响应码。
#[derive(Debug)]
pub struct FtpError {
    pub status: StatusCode,
    pub message: String,
}

impl FtpError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    pub fn deny(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

/// FTP 服务配置。字段与 `service.configure` 入参同名；未传字段沿用已持久化值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ServiceConfig {
    listen_port: u16,
    passive_port_start: u16,
    passive_port_end: u16,
    max_clients: u32,
    max_per_ip: u32,
    /// `off` / `optional`（显式 TLS，可选）/ `required`（显式 TLS，强制）。
    tls_mode: String,
    tls_cert_path: Option<String>,
    tls_key_path: Option<String>,
    /// fail2ban 专用 jail 开关。关闭只移除本插件 jail，不卸载 fail2ban。
    login_guard: bool,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            listen_port: DEFAULT_LISTEN_PORT,
            passive_port_start: DEFAULT_PORT_START,
            passive_port_end: DEFAULT_PORT_END,
            max_clients: DEFAULT_MAX_CLIENTS,
            max_per_ip: DEFAULT_MAX_PER_IP,
            tls_mode: "optional".into(),
            tls_cert_path: None,
            tls_key_path: None,
            login_guard: true,
        }
    }
}

/// 能力面入口。`method` 与 manifest 的 `ftp` verbs 一一对应。
pub async fn dispatch<H: FtpHost>(
    host: &H,
    ctx: &FtpContext,
    method: &str,
    params: &Value,
) -> Result<Value, FtpError> {
    dispatch_host(host, ctx, method, params).await
}

#[cfg(not(target_os = "linux"))]
async fn dispatch_host<H: FtpHost>(
    _host: &H,
    _ctx: &FtpContext,
    _method: &str,
    _params: &Value,
) -> Result<Value, FtpError> {
    Err(FtpError::unavailable("ftp 能力面仅在 Linux 宿主机可用"))
}

#[cfg(target_os = "linux")]
async fn dispatch_host<H: FtpHost>(
    host: &H,
    ctx: &FtpContext,
    method: &str,
    params: &Value,
) -> Result<Value, FtpError> {
    match method {
        "status" => status(host).await,
        "install" => install(host, params).await,
        "uninstall" => uninstall(host, params).await,
        "sites.list" => sites_list(ctx),
        "accounts.list" => accounts_list(ctx),
        "account.create" => account_create(host, ctx, params).await,
        "account.password" => account_password(host, params).await,
        "account.delete" => account_delete(host, params).await,
        "account.set_enabled" => account_set_enabled(host, params).await,
        "service.configure" => service_configure(host, params).await,
        "service.restart" => service_restart(host).await,
        other => Err(FtpError::deny(format!("未知 ftp 方法：{other}"))),
    }
}

// ───────────────────────────── 纯校验（跨平台可单测） ─────────────────────────

/// 账号名：字母数字开头，1–32 位，只允许 `[A-Za-z0-9_-]`。
fn valid_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.len() <= 32 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 密码：8–128 字节，禁止控制字符（防 stdin 注入与日志污染）。
fn valid_password(pw: &str) -> bool {
    (8..=128).contains(&pw.len()) && !pw.chars().any(|c| c.is_control())
}

/// 被动端口：高端口、闭区间、跨度 ≤ 500，避免占满宿主端口表。
fn valid_port_range(start: u16, end: u16) -> bool {
    start >= 1024 && end >= 1024 && start < end && (end - start) <= 500
}

/// Umask 配置值：两个空白分隔的三位八进制数（wrapper 的硬格式）。
#[cfg(test)]
fn valid_umask(value: &str) -> bool {
    let fields: Vec<&str> = value.split_whitespace().collect();
    fields.len() == 2
        && fields
            .iter()
            .all(|f| f.len() == 3 && f.bytes().all(|b| (b'0'..=b'7').contains(&b)))
}

/// 按候选顺序选出存在的 `pure-pw` 路径；`exists` 由调用方注入，便于单测。
fn pure_pw_from(exists: impl Fn(&str) -> bool) -> Option<&'static str> {
    PURE_PW_CANDIDATES.into_iter().find(|p| exists(p))
}

/// 稳定且可读的 FTP 登录名：`skiff_<域名slug>_<hash4>_<账号>`。
///
/// 域名 slug 只用于可读性；hash4 防止 `a-b` 与 `a.b` 这类清洗碰撞。
fn ftp_login(domain: &str, account: &str) -> String {
    let mut slug = String::new();
    for c in domain.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').chars().take(16).collect::<String>();
    format!("{LOGIN_PREFIX}{}_{}_{}", slug, fnv4(domain), account)
}

fn fnv4(input: &str) -> String {
    let mut h: u32 = 2_166_136_261;
    for b in input.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    format!("{h:08x}").chars().take(4).collect()
}

/// 归一化 TLS 模式；`explicit` 是 UI 对「显式 TLS、可选」的别名。
fn normalize_tls_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" => Some("off"),
        "optional" | "explicit" | "1" | "true" => Some("optional"),
        "required" | "2" => Some("required"),
        _ => None,
    }
}

/// TLS 源文件路径：POSIX 绝对路径、无 `..`、无控制字符、长度受限。
fn valid_tls_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.starts_with('/')
        && !path.ends_with('/')
        && !path.chars().any(|c| c.is_control())
        && !path.split('/').any(|seg| seg == "..")
}

/// 配置校验（跨平台纯函数）。错误文案直接回给插件 UI。
fn validate_service_config(cfg: &ServiceConfig) -> Result<(), String> {
    if cfg.listen_port == 0 {
        return Err("监听端口需在 1–65535 之间".into());
    }
    if !valid_port_range(cfg.passive_port_start, cfg.passive_port_end) {
        return Err("被动端口范围非法（需 1024–65535，start<end，跨度≤500）".into());
    }
    if !(1..=1000).contains(&cfg.max_clients) {
        return Err("最大连接数需在 1–1000 之间".into());
    }
    if !(1..=cfg.max_clients).contains(&cfg.max_per_ip) {
        return Err("每 IP 最大连接数需在 1–最大连接数之间".into());
    }
    if normalize_tls_mode(&cfg.tls_mode).is_none() {
        return Err("TLS 模式只支持 off / optional / required".into());
    }
    match (&cfg.tls_cert_path, &cfg.tls_key_path) {
        (Some(cert), Some(key)) => {
            if !valid_tls_source_path(cert) {
                return Err(format!("TLS 证书路径非法：{cert}（需绝对路径且不含 ..）"));
            }
            if !valid_tls_source_path(key) {
                return Err(format!("TLS 私钥路径非法：{key}（需绝对路径且不含 ..）"));
            }
        }
        (None, None) => {}
        _ => {
            return Err("TLS 证书与私钥路径必须同时配置或同时留空".into());
        }
    }
    Ok(())
}

/// 把请求参数叠加到已持久化配置上；未出现的字段保持不变。
fn merge_service_config(
    persisted: &ServiceConfig,
    params: &Value,
) -> Result<ServiceConfig, FtpError> {
    let mut cfg = persisted.clone();
    if let Some(v) = params.get("listen_port") {
        let n = v
            .as_u64()
            .filter(|n| *n <= u16::MAX as u64)
            .ok_or_else(|| FtpError::bad("listen_port 需为 1–65535 的整数"))?;
        cfg.listen_port = n as u16;
    }
    if let Some(v) = params.get("passive_port_start") {
        let n = v
            .as_u64()
            .filter(|n| *n <= u16::MAX as u64)
            .ok_or_else(|| FtpError::bad("passive_port_start 需为 1–65535 的整数"))?;
        cfg.passive_port_start = n as u16;
    }
    if let Some(v) = params.get("passive_port_end") {
        let n = v
            .as_u64()
            .filter(|n| *n <= u16::MAX as u64)
            .ok_or_else(|| FtpError::bad("passive_port_end 需为 1–65535 的整数"))?;
        cfg.passive_port_end = n as u16;
    }
    if let Some(v) = params.get("max_clients") {
        cfg.max_clients = v
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64)
            .ok_or_else(|| FtpError::bad("max_clients 需为整数"))? as u32;
    }
    if let Some(v) = params.get("max_per_ip") {
        cfg.max_per_ip = v
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64)
            .ok_or_else(|| FtpError::bad("max_per_ip 需为整数"))? as u32;
    }
    if let Some(v) = params.get("tls_mode") {
        let raw = v
            .as_str()
            .ok_or_else(|| FtpError::bad("tls_mode 需为字符串"))?;
        let mode = normalize_tls_mode(raw)
            .ok_or_else(|| FtpError::bad(format!("未知 tls_mode：{raw}")))?;
        cfg.tls_mode = mode.to_string();
    }
    for (key, slot) in [
        ("tls_cert_path", &mut cfg.tls_cert_path),
        ("tls_key_path", &mut cfg.tls_key_path),
    ] {
        if let Some(v) = params.get(key) {
            *slot = match v {
                Value::Null => None,
                Value::String(s) if s.trim().is_empty() => None,
                Value::String(s) => Some(s.trim().to_string()),
                _ => return Err(FtpError::bad(format!("{key} 需为字符串或 null"))),
            };
        }
    }
    if let Some(v) = params.get("login_guard") {
        cfg.login_guard = v
            .as_bool()
            .ok_or_else(|| FtpError::bad("login_guard 需为布尔值"))?;
    }
    Ok(cfg)
}

/// 解析账号行：返回（登录名，是否启用）。禁用行以 `#` 前缀保留。
fn account_line_state(line: &str) -> Option<(&str, bool)> {
    let (content, enabled) = match line.strip_prefix('#') {
        Some(rest) => (rest, false),
        None => (line, true),
    };
    if !content.starts_with(LOGIN_PREFIX) {
        return None;
    }
    content.split(':').next().map(|login| (login, enabled))
}

/// 解析 `/proc/net/tcp*`：返回（本地端口，状态码）。
fn parse_net_tcp(text: &str) -> Vec<(u16, u8)> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let Some((_, port)) = fields[1].rsplit_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port, 16) else {
            continue;
        };
        let Ok(state) = u8::from_str_radix(fields[3], 16) else {
            continue;
        };
        out.push((port, state));
    }
    out
}

/// 从 `fail2ban-client status <jail>` 输出提取（当前封禁，累计封禁）。
fn parse_fail2ban_status(text: &str) -> (Option<u32>, Option<u32>) {
    fn field(text: &str, label: &str) -> Option<u32> {
        text.lines()
            .find(|l| l.contains(label))
            .and_then(|l| l.split_once(':'))
            .and_then(|(_, v)| v.trim().parse().ok())
    }
    (field(text, "Currently banned"), field(text, "Total banned"))
}

/// 从 `fail2ban-client get <jail> banip --with-time` 输出取最近一条封禁记录。
fn parse_ban_with_time(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.to_ascii_lowercase().contains("no ip"))
        .rfind(|l| {
            l.split_whitespace().next().is_some_and(|ip| {
                ip.chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')
            })
        })
        .map(|l| l.to_string())
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ───────────────────────────── 宿主命令封装 ─────────────────────────────

/// 原样返回命令输出，供 `systemctl is-active` / `fail2ban-client status` /
/// `dpkg-query` 这类「退出码即结论」的探测使用。
async fn run_raw<H: FtpHost>(
    host: &H,
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    secs: u64,
) -> Result<CommandOutput, FtpError> {
    host.run(cmd, args, stdin, secs)
        .await
        .map_err(FtpError::internal)
}

/// 非零退出转成可读错误；成功时返回 stdout（为空则取 stderr）。
async fn run_ok<H: FtpHost>(
    host: &H,
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    secs: u64,
) -> Result<String, FtpError> {
    let out = run_raw(host, cmd, args, stdin, secs).await?;
    if !out.success {
        let detail = if out.stderr.trim().is_empty() {
            out.stdout.trim().to_string()
        } else {
            out.stderr.trim().to_string()
        };
        return Err(FtpError::internal(format!("{cmd} 执行失败：{detail}")));
    }
    Ok(if out.stdout.trim().is_empty() {
        out.stderr
    } else {
        out.stdout
    })
}

// ───────────────────────────── Linux 实现 ─────────────────────────

#[cfg(target_os = "linux")]
fn pure_pw_path() -> Option<&'static str> {
    pure_pw_from(|p| Path::new(p).exists())
}

#[cfg(target_os = "linux")]
async fn status<H: FtpHost>(host: &H) -> Result<Value, FtpError> {
    let cfg = load_service_config().await;
    let installed = Path::new("/usr/sbin/pure-ftpd").exists();
    let running = systemctl_active_opt(host, SERVICE).await;
    let enabled = systemctl_enabled_opt(host, SERVICE).await;
    let version = if installed {
        run_raw(
            host,
            "/usr/bin/dpkg-query",
            &svec(&["-W", "-f=${Version}", "pure-ftpd"]),
            None,
            120,
        )
        .await
        .ok()
        .filter(|o| o.success)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
    } else {
        None
    };
    let (start, end) = if installed {
        read_port_range().await
    } else {
        (cfg.passive_port_start, cfg.passive_port_end)
    };
    let listen_port = if installed {
        read_conf("Bind")
            .await
            .and_then(|v| v.rsplit(',').next()?.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_LISTEN_PORT)
    } else {
        cfg.listen_port
    };
    let tls_mode = if installed {
        normalize_tls_mode(&read_conf("TLS").await.unwrap_or_else(|| "off".into())).unwrap_or("off")
    } else {
        normalize_tls_mode(&cfg.tls_mode).unwrap_or("off")
    };
    let max_clients = if installed {
        read_conf("MaxClientsNumber")
            .await
            .and_then(|v| v.parse::<u32>().ok())
    } else {
        Some(cfg.max_clients)
    };
    let max_per_ip = if installed {
        read_conf("MaxClientsPerIP")
            .await
            .and_then(|v| v.parse::<u32>().ok())
    } else {
        Some(cfg.max_per_ip)
    };
    let tcp = if installed { tcp_snapshot() } else { None };
    let listening = if installed {
        tcp.as_ref().map(|entries| {
            entries
                .iter()
                .any(|(port, state)| *port == listen_port && *state == 0x0A)
        })
    } else {
        Some(false)
    };
    let current_connections = if installed {
        tcp.as_ref().map(|entries| {
            entries
                .iter()
                .filter(|(port, state)| *port == listen_port && *state == 0x01)
                .count() as u32
        })
    } else {
        Some(0)
    };
    let accounts = read_accounts().unwrap_or_default();
    let accounts_total = accounts.len();
    let accounts_enabled = accounts.iter().filter(|a| a.enabled).count();
    let fail2ban = fail2ban_status(host).await;
    let config = serde_json::to_value(&cfg).unwrap_or_else(|_| json!({}));
    Ok(json!({
        "installed": installed,
        "running": running,
        "enabled": enabled,
        "listening": listening,
        "listen_port": listen_port,
        "current_connections": current_connections,
        "version": version,
        "passive_port_start": start,
        "passive_port_end": end,
        "max_clients": max_clients,
        "max_per_ip": max_per_ip,
        "tls_mode": tls_mode,
        "tls_cert_path": cfg.tls_cert_path,
        "tls_key_path": cfg.tls_key_path,
        "anonymous": false,
        "fail2ban": fail2ban,
        "accounts": accounts_total,
        "accounts_enabled": accounts_enabled,
        "accounts_disabled": accounts_total - accounts_enabled,
        "config": config,
        "passwd_file": PASSWD_FILE,
        "pdb_file": PDB_FILE,
    }))
}

#[cfg(target_os = "linux")]
async fn install<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let persisted = load_service_config().await;
    let cfg = merge_service_config(&persisted, params)?;
    validate_service_config(&cfg).map_err(FtpError::bad)?;
    apply_service_config(host, &cfg, true).await
}

/// 保存配置并在服务已安装时立即应用；未安装时只落盘，安装后沿用。
#[cfg(target_os = "linux")]
async fn service_configure<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let persisted = load_service_config().await;
    let cfg = merge_service_config(&persisted, params)?;
    validate_service_config(&cfg).map_err(FtpError::bad)?;
    apply_service_config(host, &cfg, false).await
}

#[cfg(target_os = "linux")]
async fn apply_service_config<H: FtpHost>(
    host: &H,
    cfg: &ServiceConfig,
    allow_install: bool,
) -> Result<Value, FtpError> {
    validate_tls_material(cfg).await?;
    let installed = Path::new("/usr/sbin/pure-ftpd").exists();
    if !installed && !allow_install {
        save_service_config(cfg).await?;
        return Ok(json!({
            "saved": true,
            "applied": false,
            "config": cfg,
            "status": status(host).await?,
        }));
    }

    let had_f2b = fail2ban_client_path().is_some();
    if !installed {
        let mut pkgs = vec!["pure-ftpd"];
        if cfg.login_guard && !had_f2b {
            pkgs.push("fail2ban");
        }
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend(pkgs.iter().copied());
        let args = svec(&args);
        let mut out = run_ok(host, "/usr/bin/apt-get", &args, None, 900)
            .await
            .map_err(|e| FtpError::internal(format!("安装 pure-ftpd 失败：{}", e.message)))?;
        if out.contains("Unable to locate package") {
            run_ok(host, "/usr/bin/apt-get", &svec(&["update"]), None, 900)
                .await
                .map_err(|e| FtpError::internal(format!("apt-get update 失败：{}", e.message)))?;
            out = run_ok(host, "/usr/bin/apt-get", &args, None, 900)
                .await
                .map_err(|e| FtpError::internal(format!("安装 pure-ftpd 失败：{}", e.message)))?;
        }
        if !Path::new("/usr/sbin/pure-ftpd").exists() {
            return Err(FtpError::internal(format!(
                "pure-ftpd 安装后仍不存在：{out}"
            )));
        }
    }

    // standalone 模式：Debian 默认即 standalone；显式固化，避免被 inetd 抢占端口。
    ensure_default_file(host).await?;
    // 若安装时仍拉入了 openbsd-inetd，停用它；pure-ftpd 由 systemd 托管。
    let _ = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["disable", "--now", "openbsd-inetd"]),
        None,
        300,
    )
    .await;
    let _ = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["disable", "--now", "inetd"]),
        None,
        300,
    )
    .await;

    write_conf(host, "ChrootEveryone", "yes").await?;
    write_conf(host, "NoAnonymous", "yes").await?;
    write_conf(host, "PAMAuthentication", "no").await?;
    write_conf(host, "UnixAuthentication", "no").await?;
    write_conf(host, "PureDB", PDB_FILE).await?;
    // MinUID 不写死：pure-ftpd 默认 1000。站点统一身份固定 201000，天然高于该阈值；
    // 这里仍取「当前值与本次身份取小」的口径（只降不升），兼容历史上被下调过的机器。
    // Debian 的 pure-ftpd-wrapper 只认 `conf/Bind`（`-S [addr,]port`）；写 `conf/Port` 会被当作未知项。
    write_conf(host, "Bind", &cfg.listen_port.to_string()).await?;
    write_conf(
        host,
        "PassivePortRange",
        &format!("{} {}", cfg.passive_port_start, cfg.passive_port_end),
    )
    .await?;
    write_conf(host, "Umask", UMASK).await?;
    write_conf(host, "MaxClientsNumber", &cfg.max_clients.to_string()).await?;
    write_conf(host, "MaxClientsPerIP", &cfg.max_per_ip.to_string()).await?;
    write_conf(host, "DontResolve", "yes").await?;
    let tls_value = match normalize_tls_mode(&cfg.tls_mode).unwrap_or("off") {
        "off" => "0",
        "optional" => "1",
        _ => "2",
    };
    write_conf(host, "TLS", tls_value).await?;
    // Debian 的认证链只认 /etc/pure-ftpd/auth 下的符号链接：只写 conf/PureDB 不生效，
    // 包自带的 65unix / 70pam 还会让虚拟账号先走 PAM，最终 530。
    let stale: Vec<String> = AUTH_STALE
        .iter()
        .map(|n| format!("{AUTH_DIR}/{n}"))
        .collect();
    let _ = run_raw(
        host,
        "/usr/bin/rm",
        &["-f".to_string(), stale[0].clone(), stale[1].clone()],
        None,
        900,
    )
    .await;
    // 卸载会整目录回收 `/etc/pure-ftpd/auth`，干净系统首装时该目录也不存在；
    // 直接 ln 会以 "No such file or directory" 失败，这里先补建。
    // `/etc` 在主服务里是只读（ProtectSystem=full），mk 必须走宿主通道。
    run_ok(host, "/usr/bin/mkdir", &svec(&["-p", AUTH_DIR]), None, 900)
        .await
        .map_err(|e| FtpError::internal(format!("创建 {AUTH_DIR} 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/ln",
        &svec(&["-sfn", AUTH_DB_LINK, &format!("{AUTH_DIR}/50pure")]),
        None,
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("启用 PureDB 认证失败：{}", e.message)))?;
    // 无账号时 PureDB 文件并不存在，Debian 的 pure-ftpd wrapper 会因认证库缺失
    // 拒绝启动（首次 install 即 active 失败）；先落一个空库，建号仍走 `pure-pw useradd -m`。
    ensure_pdb_present(host).await?;
    ensure_tls_pem(host, cfg).await?;

    if cfg.login_guard {
        ensure_login_guard(host, had_f2b, cfg.listen_port).await?;
    } else {
        disable_login_guard(host).await?;
    }

    // 先落盘再重启：即使重启失败，用户下次也能沿用这次配置继续修。
    save_service_config(cfg).await?;
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["daemon-reload"]),
        None,
        300,
    )
    .await
    .map_err(|e| FtpError::internal(format!("systemctl daemon-reload 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["enable", SERVICE]),
        None,
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("启用 pure-ftpd 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["restart", SERVICE]),
        None,
        300,
    )
    .await
    .map_err(|e| FtpError::internal(format!("启动 pure-ftpd 失败：{}", e.message)))?;
    if !systemctl_active(host, SERVICE).await {
        return Err(FtpError::internal("pure-ftpd 启动后未进入 active"));
    }
    Ok(json!({
        "saved": true,
        "applied": true,
        "config": cfg,
        "status": status(host).await?,
    }))
}

#[cfg(target_os = "linux")]
async fn uninstall<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let remove_accounts = params
        .get("remove_accounts")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let remove_package = params
        .get("remove_package")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let accounts = read_accounts().unwrap_or_default();
    if remove_accounts {
        // 同时回收启用行与 `#` 前缀的禁用行，保留非本插件账号。
        let _ = rewrite_accounts(host, |line| account_line_state(line).is_none()).await;
        let _ = mkdb(host).await;
    }
    let _ = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["stop", SERVICE]),
        None,
        900,
    )
    .await;
    let _ = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["disable", SERVICE]),
        None,
        900,
    )
    .await;
    let _ = disable_login_guard(host).await;
    // install 会在 auth 目录建 `50pure` 符号链接；卸载后该目录必须一并回收，
    // 否则留下指向前 PureDB 的悬空链接。保留包（remove_package=false）时也回收：
    // 服务已停，重装会由 install 重新建目录与链接。
    let _ = run_raw(host, "/usr/bin/rm", &svec(&["-rf", AUTH_DIR]), None, 900).await;
    if remove_package {
        let _ = run_raw(
            host,
            "/usr/bin/apt-get",
            &svec(&["remove", "-y", "pure-ftpd"]),
            None,
            900,
        )
        .await;
        let _ = run_raw(
            host,
            "/usr/bin/rm",
            &svec(&["-f", PASSWD_FILE, PDB_FILE]),
            None,
            900,
        )
        .await;
        let _ = run_raw(host, "/usr/bin/rm", &svec(&["-rf", CONF_DIR]), None, 900).await;
    }
    Ok(json!({
        "stopped": true,
        "removed_accounts": if remove_accounts { accounts.len() } else { 0 },
        "removed_package": remove_package,
    }))
}

#[cfg(target_os = "linux")]
async fn service_restart<H: FtpHost>(host: &H) -> Result<Value, FtpError> {
    if !Path::new("/usr/sbin/pure-ftpd").exists() {
        return Err(FtpError::bad("pure-ftpd 尚未安装"));
    }
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["restart", SERVICE]),
        None,
        300,
    )
    .await
    .map_err(|e| FtpError::internal(format!("重启 pure-ftpd 失败：{}", e.message)))?;
    let active = systemctl_active(host, SERVICE).await;
    Ok(json!({ "running": active }))
}

// ───────────────────────────── 站点与账号 ─────────────────────────

#[cfg(target_os = "linux")]
fn sites_list(ctx: &FtpContext) -> Result<Value, FtpError> {
    let mut out = Vec::new();
    for s in &ctx.sites {
        let runtime_kind = if s.site_type == TYPE_RUNTIME {
            s.runtime_kind.clone().unwrap_or_else(|| "missing".into())
        } else {
            String::new()
        };
        let php = runtime_kind == "php";
        let supported = match &s.site_type[..] {
            TYPE_RUNTIME => php,
            TYPE_STATIC => true,
            TYPE_PROXY => false,
            _ => false,
        };
        let reason = if supported {
            ""
        } else if s.site_type == TYPE_PROXY {
            "反向代理站点没有可归属的文档根"
        } else {
            "该运行环境不是 PHP，v1 未验证其文件身份"
        };
        out.push(json!({
            "domain": s.domain,
            "site_type": s.site_type,
            "root_dir": s.root_dir,
            "index_dir": site_index_str(s),
            "php": php,
            "runtime_kind": runtime_kind,
            "container": s.container,
            "supported": supported,
            "reason": reason,
        }));
    }
    Ok(json!({ "sites": out }))
}

#[cfg(target_os = "linux")]
fn accounts_list(ctx: &FtpContext) -> Result<Value, FtpError> {
    let accounts = read_accounts().map_err(FtpError::internal)?;
    let mut out = Vec::new();
    for a in accounts {
        let home = normalize_home(&a.home);
        let site = ctx
            .sites
            .iter()
            .find(|s| normalize_home(&site_index_str(s)) == home)
            .map(|s| s.domain.clone())
            .unwrap_or_default();
        out.push(json!({
            "login": a.login,
            "site": site,
            "uid": a.uid,
            "gid": a.gid,
            "home": home,
            "enabled": a.enabled,
            // pure-ftpd 的 pureftpd.passwd 不记录最后登录；日志推导口径不稳定，
            // 因此明确回传「不可得」，不用 0 或空串冒充。
            "last_login": Value::Null,
            "last_login_source": "unavailable",
            "quota_supported": false,
            "rate_supported": false,
        }));
    }
    Ok(json!({ "accounts": out }))
}

#[cfg(target_os = "linux")]
async fn account_create<H: FtpHost>(
    host: &H,
    ctx: &FtpContext,
    params: &Value,
) -> Result<Value, FtpError> {
    let domain = params
        .get("site")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let account = params
        .get("account")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if domain.is_empty() {
        return Err(FtpError::bad("缺少站点域名"));
    }
    if !valid_account_name(account) {
        return Err(FtpError::bad(
            "账号名需以字母/数字开头，1–32 位，只允许字母、数字、下划线与连字符",
        ));
    }
    if !valid_password(password) {
        return Err(FtpError::bad("密码需 8–128 字节且不能包含控制字符"));
    }
    if !Path::new("/usr/sbin/pure-ftpd").exists() {
        return Err(FtpError::bad("FTP 服务尚未安装，请先调用 install"));
    }

    let site = ctx
        .sites
        .iter()
        .find(|s| s.domain == domain)
        .ok_or_else(|| FtpError::not_found(format!("站点不存在：{domain}")))?;
    let index = validate_index(ctx, site)?;
    let (uid, gid) = resolve_identity(site)?;
    ensure_min_uid(host, uid).await?;
    let login = ftp_login(domain, account);
    let existing = read_accounts().map_err(FtpError::internal)?;
    if existing.iter().any(|a| a.login == login) {
        return Err(FtpError {
            status: StatusCode::CONFLICT,
            message: format!("FTP 账号已存在：{login}"),
        });
    }

    // 只对齐本站点文档根；父目录、log/ssl/conf 与其它站点一律不碰。
    let index_for_task = index.clone();
    tokio::task::spawn_blocking(move || align_tree(&index_for_task, uid, gid))
        .await
        .map_err(|e| FtpError::internal(format!("对齐目录任务失败：{e}")))?
        .map_err(FtpError::internal)?;

    let home = index.to_string_lossy().to_string();
    let pw =
        pure_pw_path().ok_or_else(|| FtpError::bad("找不到 pure-pw，请确认 pure-ftpd 已安装"))?;
    let args = vec![
        "useradd".to_string(),
        login.clone(),
        "-u".to_string(),
        uid.to_string(),
        "-g".to_string(),
        gid.to_string(),
        "-d".to_string(),
        home.clone(),
        "-f".to_string(),
        PASSWD_FILE.to_string(),
        "-m".to_string(),
    ];
    run_ok(
        host,
        pw,
        &args,
        Some(&format!("{password}\n{password}\n")),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("创建 pure-pw 账号失败：{}", e.message)))?;
    restart_service(host).await?;

    Ok(json!({
        "login": login,
        "site": domain,
        "uid": uid,
        "gid": gid,
        "home": home,
    }))
}

#[cfg(target_os = "linux")]
async fn account_password<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(FtpError::deny("只能修改本插件创建的 FTP 账号"));
    }
    if !valid_password(password) {
        return Err(FtpError::bad("密码需 8–128 字节且不能包含控制字符"));
    }
    let accounts = read_accounts().map_err(FtpError::internal)?;
    let Some(account) = accounts.iter().find(|a| a.login == login) else {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    };
    if !account.enabled {
        return Err(FtpError::bad("账号已禁用，请先启用后再修改密码"));
    }
    let pw = pure_pw_path()
        .ok_or_else(|| FtpError::internal("找不到 pure-pw，请确认 pure-ftpd 已安装"))?;
    run_ok(
        host,
        pw,
        &svec(&["passwd", login, "-f", PASSWD_FILE, "-m"]),
        Some(&format!("{password}\n{password}\n")),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("修改 FTP 密码失败：{}", e.message)))?;
    restart_service(host).await?;
    Ok(json!({ "login": login }))
}

#[cfg(target_os = "linux")]
async fn account_delete<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(FtpError::deny("只能删除本插件创建的 FTP 账号"));
    }
    if !read_accounts()
        .map_err(FtpError::internal)?
        .iter()
        .any(|a| a.login == login)
    {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    }
    rewrite_accounts(
        host,
        |line| !matches!(account_line_state(line), Some((name, _)) if name == login),
    )
    .await?;
    mkdb(host).await?;
    restart_service(host).await?;
    Ok(json!({ "login": login, "files_kept": true }))
}

/// 启用/禁用只改写 `pureftpd.passwd` 行的 `#` 前缀并重建 PureDB：
/// 禁用后 PureDB 里没有该账号，登录失败，但账号行与站点目录都保留。
#[cfg(target_os = "linux")]
async fn account_set_enabled<H: FtpHost>(host: &H, params: &Value) -> Result<Value, FtpError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| FtpError::bad("缺少 enabled（布尔值）"))?;
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(FtpError::deny("只能操作本插件创建的 FTP 账号"));
    }
    let accounts = read_accounts().map_err(FtpError::internal)?;
    if !accounts.iter().any(|a| a.login == login) {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    }
    toggle_account_line(host, login, enabled).await?;
    mkdb(host).await?;
    restart_service(host).await?;
    Ok(json!({ "login": login, "enabled": enabled }))
}

/// 遍历 passwd 文件，保留 `keep(line)` 为真的行，原子落盘。
#[cfg(target_os = "linux")]
async fn rewrite_accounts<H: FtpHost>(
    host: &H,
    keep: impl Fn(&str) -> bool,
) -> Result<(), FtpError> {
    let body = std::fs::read_to_string(PASSWD_FILE)
        .map_err(|e| FtpError::internal(format!("读取 {PASSWD_FILE} 失败：{e}")))?;
    let mut out = String::new();
    for line in body.lines() {
        if keep(line) {
            out.push_str(line);
            out.push('\n');
        }
    }
    write_passwd_host(host, &out).await
}

/// 把目标账号行改为启用/禁用，其余行原样保留。
#[cfg(target_os = "linux")]
async fn toggle_account_line<H: FtpHost>(
    host: &H,
    login: &str,
    enabled: bool,
) -> Result<(), FtpError> {
    let body = std::fs::read_to_string(PASSWD_FILE)
        .map_err(|e| FtpError::internal(format!("读取 {PASSWD_FILE} 失败：{e}")))?;
    let mut out = String::new();
    let mut found = false;
    for line in body.lines() {
        match account_line_state(line) {
            Some((name, _)) if name == login => {
                found = true;
                let content = line.strip_prefix('#').unwrap_or(line);
                if enabled {
                    out.push_str(content);
                } else {
                    out.push('#');
                    out.push_str(content);
                }
                out.push('\n');
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !found {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    }
    write_passwd_host(host, &out).await
}

/// 通过临时文件 + `install -m 0600` 覆盖 passwd，避免 `tee` 截断时留下半截账号表。
#[cfg(target_os = "linux")]
async fn write_passwd_host<H: FtpHost>(host: &H, body: &str) -> Result<(), FtpError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = format!("/tmp/skiff-ftp-passwd-{}-{n}.tmp", std::process::id());
    run_ok(
        host,
        "/usr/bin/tee",
        std::slice::from_ref(&tmp),
        Some(body),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入临时账号表失败：{}", e.message)))?;
    let result = run_ok(
        host,
        "/usr/bin/install",
        &svec(&["-m", "0600", &tmp, PASSWD_FILE]),
        None,
        900,
    )
    .await;
    let _ = run_raw(host, "/usr/bin/rm", &svec(&["-f", &tmp]), None, 900).await;
    result.map_err(|e| FtpError::internal(format!("更新 {PASSWD_FILE} 失败：{}", e.message)))?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn mkdb<H: FtpHost>(host: &H) -> Result<(), FtpError> {
    let pw = pure_pw_path()
        .ok_or_else(|| FtpError::internal("找不到 pure-pw，请确认 pure-ftpd 已安装"))?;
    run_ok(
        host,
        pw,
        &svec(&["mkdb", PDB_FILE, "-f", PASSWD_FILE]),
        None,
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("生成 PureDB 失败：{}", e.message)))?;
    Ok(())
}

/// 无账号时 PureDB 不存在，pure-ftpd 的 wrapper 会以认证库缺失拒绝启动。
/// `pure-pw mkdb` 在空账号表上也会生成可用空库；建号仍由 `pure-pw useradd -m` 更新。
#[cfg(target_os = "linux")]
async fn ensure_pdb_present<H: FtpHost>(host: &H) -> Result<(), FtpError> {
    if Path::new(PDB_FILE).exists() {
        return Ok(());
    }
    if !Path::new(PASSWD_FILE).exists() {
        // `/etc/pure-ftpd` 在主服务只读，建文件必须经宿主通道（install -m 0600 -D /dev/null）。
        run_ok(
            host,
            "/usr/bin/mkdir",
            &svec(&["-p", "/etc/pure-ftpd"]),
            None,
            900,
        )
        .await
        .map_err(|e| FtpError::internal(format!("创建 /etc/pure-ftpd 失败：{}", e.message)))?;
        run_ok(
            host,
            "/usr/bin/install",
            &svec(&["-m", "0600", "/dev/null", PASSWD_FILE]),
            None,
            900,
        )
        .await
        .map_err(|e| FtpError::internal(format!("创建 {PASSWD_FILE} 失败：{}", e.message)))?;
    }
    mkdb(host).await?;
    if !Path::new(PDB_FILE).exists() {
        return Err(FtpError::internal("生成 PureDB 后文件仍不存在"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn restart_service<H: FtpHost>(host: &H) -> Result<(), FtpError> {
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["restart", SERVICE]),
        None,
        300,
    )
    .await
    .map_err(|e| FtpError::internal(format!("重启 pure-ftpd 失败：{}", e.message)))?;
    Ok(())
}

// ───────────────────────────── 配置持久化与状态探测 ─────────────────────────

#[cfg(target_os = "linux")]
async fn load_service_config() -> ServiceConfig {
    let Ok(raw) = tokio::fs::read_to_string(SERVICE_CONFIG_FILE).await else {
        return ServiceConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

#[cfg(target_os = "linux")]
async fn save_service_config(cfg: &ServiceConfig) -> Result<(), FtpError> {
    let body = serde_json::to_string_pretty(cfg)
        .map_err(|e| FtpError::internal(format!("序列化 FTP 配置失败：{e}")))?;
    if let Some(parent) = Path::new(SERVICE_CONFIG_FILE).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| FtpError::internal(format!("创建 {} 失败：{e}", parent.display())))?;
    }
    let tmp = format!("{SERVICE_CONFIG_FILE}.tmp");
    tokio::fs::write(&tmp, body.as_bytes())
        .await
        .map_err(|e| FtpError::internal(format!("写入 {SERVICE_CONFIG_FILE} 失败：{e}")))?;
    tokio::fs::rename(&tmp, SERVICE_CONFIG_FILE)
        .await
        .map_err(|e| FtpError::internal(format!("落盘 {SERVICE_CONFIG_FILE} 失败：{e}")))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn fail2ban_client_path() -> Option<&'static str> {
    ["/usr/bin/fail2ban-client", "/usr/sbin/fail2ban-client"]
        .into_iter()
        .find(|p| Path::new(p).exists())
}

#[cfg(target_os = "linux")]
async fn fail2ban_status<H: FtpHost>(host: &H) -> Value {
    let installed = fail2ban_client_path().is_some();
    let running = if installed {
        systemctl_active_opt(host, "fail2ban").await
    } else {
        Some(false)
    };
    let jail_configured = tokio::fs::metadata(JAIL_FILE).await.is_ok();
    let mut jail_enabled = None;
    let mut currently_banned = None;
    let mut total_banned = None;
    let mut recent_ban = None;
    let mut recent_ban_source = "unavailable";

    if let (Some(client), Some(true)) = (fail2ban_client_path(), running) {
        match run_raw(host, client, &svec(&["status", "pure-ftpd"]), None, 120).await {
            Ok(o) if o.success => {
                jail_enabled = Some(true);
                let (banned, total) = parse_fail2ban_status(&o.stdout);
                currently_banned = banned;
                total_banned = total;
                match run_raw(
                    host,
                    client,
                    &svec(&["get", "pure-ftpd", "banip", "--with-time"]),
                    None,
                    120,
                )
                .await
                {
                    Ok(list) if list.success => {
                        if let Some(line) = parse_ban_with_time(&list.stdout) {
                            recent_ban = Some(line);
                            recent_ban_source = "fail2ban-client get pure-ftpd banip --with-time";
                        }
                    }
                    _ => {}
                }
            }
            Ok(o) => {
                let text = format!("{}{}", o.stdout, o.stderr);
                if text.contains("not found") || text.contains("no such jail") {
                    jail_enabled = Some(false);
                }
            }
            Err(_) => {}
        }
    }

    json!({
        "installed": installed,
        "running": running,
        "jail_configured": jail_configured,
        "jail_enabled": jail_enabled,
        "currently_banned": currently_banned,
        "total_banned": total_banned,
        "recent_ban": recent_ban,
        "recent_ban_source": recent_ban_source,
    })
}

#[cfg(target_os = "linux")]
async fn disable_login_guard<H: FtpHost>(host: &H) -> Result<(), FtpError> {
    if fail2ban_client_path().is_none() {
        return Ok(());
    }
    // 关掉的是本插件的 pure-ftpd jail；若同一文件里还带着安装时写的
    // `[sshd] enabled=false`，只保留这一段，避免顺手把宿主的 sshd 防护又打开。
    let existing = tokio::fs::read_to_string(JAIL_FILE)
        .await
        .unwrap_or_default();
    if existing.contains("[sshd]") {
        run_ok(
            host,
            "/usr/bin/tee",
            &[JAIL_FILE.to_string()],
            Some("[sshd]\nenabled = false\n"),
            900,
        )
        .await
        .map_err(|e| FtpError::internal(format!("更新 fail2ban jail 失败：{}", e.message)))?;
    } else {
        run_ok(host, "/usr/bin/rm", &svec(&["-f", JAIL_FILE]), None, 900)
            .await
            .map_err(|e| FtpError::internal(format!("移除 fail2ban jail 失败：{}", e.message)))?;
    }
    if systemctl_active_opt(host, "fail2ban").await == Some(true) {
        run_ok(
            host,
            "/usr/bin/systemctl",
            &svec(&["restart", "fail2ban"]),
            None,
            300,
        )
        .await
        .map_err(|e| FtpError::internal(format!("重启 fail2ban 失败：{}", e.message)))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn tcp_snapshot() -> Option<Vec<(u16, u8)>> {
    let mut entries = Vec::new();
    let mut read_any = false;
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            read_any = true;
            entries.extend(parse_net_tcp(&text));
        }
    }
    read_any.then_some(entries)
}

// ───────────────────────────── 站点根与身份 ─────────────────────────

#[cfg(target_os = "linux")]
fn validate_index(ctx: &FtpContext, site: &FtpSite) -> Result<PathBuf, FtpError> {
    let root = Path::new(&site.root_dir);
    let index = site_index_dir(root);
    let meta = std::fs::symlink_metadata(&index)
        .map_err(|e| FtpError::bad(format!("站点文档根不存在：{}（{e}）", index.display())))?;
    if meta.file_type().is_symlink() {
        return Err(FtpError::bad("文档根不能是符号链接"));
    }
    if !meta.is_dir() {
        return Err(FtpError::bad("文档根不是目录"));
    }
    let sites_root = std::fs::canonicalize(&ctx.sites_root)
        .map_err(|e| FtpError::internal(format!("站点根目录不可用：{e}")))?;
    let real_index =
        std::fs::canonicalize(&index).map_err(|e| FtpError::bad(format!("文档根解析失败：{e}")))?;
    if !real_index.starts_with(&sites_root) {
        return Err(FtpError::deny("文档根不在 /www/sites 下"));
    }
    let real_root =
        std::fs::canonicalize(root).map_err(|e| FtpError::bad(format!("站点根解析失败：{e}")))?;
    if !real_index.starts_with(&real_root) {
        return Err(FtpError::deny("文档根越出站点目录"));
    }
    Ok(real_index)
}

/// 所有可服务站点统一使用固定身份 `skiff-web` 201000:201000。
///
/// 不再按站点类型动态分配 `skiff-ftp`，也不探测容器内 `www-data`：
/// 宿主文档根、FTP 上传与 PHP-FPM 池共用同一 uid，`ls -ln` 可直接判断权限。
#[cfg(target_os = "linux")]
fn resolve_identity(site: &FtpSite) -> Result<(u32, u32), FtpError> {
    match &site.site_type[..] {
        TYPE_RUNTIME => match site.runtime_kind.as_deref() {
            Some("php") => Ok((skiff_model::SITE_UID, skiff_model::SITE_GID)),
            Some("missing") => Err(FtpError::bad("运行环境不存在")),
            Some(kind) => Err(FtpError::bad(format!(
                "运行环境类型 {kind} 的文件身份尚未验证，v1 只支持 PHP"
            ))),
            None => Err(FtpError::bad("站点未绑定运行环境")),
        },
        TYPE_STATIC => Ok((skiff_model::SITE_UID, skiff_model::SITE_GID)),
        TYPE_PROXY => Err(FtpError::bad("反向代理站点不能作为 FTP 文档根")),
        other => Err(FtpError::bad(format!("未知站点类型：{other}"))),
    }
}

/// 归一目标权限：目录固定 2775；文件保留可执行位（0775）否则 0664。
///
/// 与 [`UMASK`] 推导出的新建权限同形：FTP 上传的文件本身就是 0664、目录是
/// 2775，因此重复执行归一结果不变。
fn normalized_mode(is_dir: bool, old: u32) -> u32 {
    if is_dir {
        0o2775
    } else if old & 0o111 != 0 {
        0o775
    } else {
        0o664
    }
}

/// 递归对齐文档根：目录 2775、文件 0664（可执行文件 0775），属主/属组为目标身份。
/// 不跟随符号链接；`log/`、`ssl/`、`conf/` 不在路径内，天然不受影响。
#[cfg(target_os = "linux")]
fn align_tree(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let is_dir = meta.is_dir();
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
        .map_err(|e| format!("chown {} 失败：{e}", path.display()))?;
    let mut perms = meta.permissions();
    let old = perms.mode();
    perms.set_mode(normalized_mode(is_dir, old));
    std::fs::set_permissions(path, perms)
        .map_err(|e| format!("chmod {} 失败：{e}", path.display()))?;
    if is_dir {
        let entries =
            std::fs::read_dir(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("遍历 {} 失败：{e}", path.display()))?;
            let child = entry.path();
            let child_meta = std::fs::symlink_metadata(&child)
                .map_err(|e| format!("读取 {} 失败：{e}", child.display()))?;
            if child_meta.file_type().is_symlink() {
                continue;
            }
            align_tree(&child, uid, gid)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn site_index_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(skiff_model::SITE_INDEX_DIR)
}

#[cfg(target_os = "linux")]
fn site_index_str(site: &FtpSite) -> String {
    site_index_dir(Path::new(&site.root_dir))
        .to_string_lossy()
        .to_string()
}

// ───────────────────────────── 宿主状态与配置文件 ─────────────────────────

#[cfg(target_os = "linux")]
async fn systemctl_active_opt<H: FtpHost>(host: &H, service: &str) -> Option<bool> {
    let out = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["is-active", service]),
        None,
        120,
    )
    .await
    .ok()?;
    match out.stdout.trim() {
        "active" | "activating" | "reloading" => Some(true),
        "inactive" | "failed" | "deactivating" => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
async fn systemctl_active<H: FtpHost>(host: &H, service: &str) -> bool {
    systemctl_active_opt(host, service).await.unwrap_or(false)
}

#[cfg(target_os = "linux")]
async fn systemctl_enabled_opt<H: FtpHost>(host: &H, service: &str) -> Option<bool> {
    let out = run_raw(
        host,
        "/usr/bin/systemctl",
        &svec(&["is-enabled", service]),
        None,
        120,
    )
    .await
    .ok()?;
    match out.stdout.trim() {
        "enabled" | "enabled-runtime" => Some(true),
        "disabled" | "masked" | "masked-runtime" | "linked" | "linked-runtime" => Some(false),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
async fn read_conf(name: &str) -> Option<String> {
    tokio::fs::read_to_string(format!("{CONF_DIR}/{name}"))
        .await
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
async fn read_port_range() -> (u16, u16) {
    if let Some(raw) = read_conf("PassivePortRange").await {
        let mut parts = raw.split_whitespace();
        if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
            if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                return (a, b);
            }
        }
    }
    (DEFAULT_PORT_START, DEFAULT_PORT_END)
}

#[cfg(target_os = "linux")]
async fn write_conf<H: FtpHost>(host: &H, name: &str, value: &str) -> Result<(), FtpError> {
    let path = format!("{CONF_DIR}/{name}");
    run_ok(host, "/usr/bin/mkdir", &svec(&["-p", CONF_DIR]), None, 900)
        .await
        .map_err(|e| FtpError::internal(format!("创建 {CONF_DIR} 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/tee",
        &[path],
        Some(&format!("{value}\n")),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入配置 {name} 失败：{}", e.message)))?;
    Ok(())
}

/// pure-ftpd 的 `MinUID` 是全局配置：只允许它随实际站点身份**下调**。
///
/// 上调会让已有账号在下次登录时被拒，因此这里取「当前值」与「本次身份 uid」的较小者。
/// 统一身份固定 201000（高于默认 1000），正常情况下不会改写该配置；保留这段是为了
/// 兼容历史上被下调过的机器，避免装回默认值后旧账号登录失败。
#[cfg(target_os = "linux")]
async fn ensure_min_uid<H: FtpHost>(host: &H, uid: u32) -> Result<(), FtpError> {
    let current = read_conf("MinUID")
        .await
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(1000);
    if uid < current {
        write_conf(host, "MinUID", &uid.to_string()).await?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn ensure_default_file<H: FtpHost>(host: &H) -> Result<(), FtpError> {
    let old = tokio::fs::read_to_string(DEFAULT_FILE)
        .await
        .unwrap_or_default();
    let mut kept = Vec::new();
    let mut seen = false;
    for line in old.lines() {
        if line.starts_with("STANDALONE_OR_INETD=") {
            kept.push("STANDALONE_OR_INETD=standalone".to_string());
            seen = true;
        } else {
            kept.push(line.to_string());
        }
    }
    if !seen {
        kept.push("STANDALONE_OR_INETD=standalone".to_string());
    }
    let body = kept.join("\n") + "\n";
    if let Some(parent) = Path::new(DEFAULT_FILE).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_ok(host, "/usr/bin/mkdir", &svec(&["-p", &parent]), None, 900)
            .await
            .map_err(|e| FtpError::internal(format!("创建 {parent} 失败：{}", e.message)))?;
    }
    run_ok(
        host,
        "/usr/bin/tee",
        &[DEFAULT_FILE.to_string()],
        Some(&body),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入 {DEFAULT_FILE} 失败：{}", e.message)))?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn validate_tls_material(cfg: &ServiceConfig) -> Result<(), FtpError> {
    if normalize_tls_mode(&cfg.tls_mode).unwrap_or("off") == "off" {
        return Ok(());
    }
    let (Some(cert), Some(key)) = (&cfg.tls_cert_path, &cfg.tls_key_path) else {
        return Ok(());
    };
    validate_tls_source(cert, true).await?;
    validate_tls_source(key, false).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn validate_tls_source(path: &str, is_cert: bool) -> Result<(), FtpError> {
    let kind = if is_cert { "证书" } else { "私钥" };
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| FtpError::bad(format!("TLS {kind}路径不存在或不可读：{path}（{e}）")))?;
    if !meta.is_file() {
        return Err(FtpError::bad(format!("TLS {kind}路径不是文件：{path}")));
    }
    if meta.len() == 0 {
        return Err(FtpError::bad(format!("TLS {kind}文件为空：{path}")));
    }
    if meta.len() > MAX_TLS_SOURCE_BYTES {
        return Err(FtpError::bad(format!(
            "TLS {kind}文件超过 {} KB：{path}",
            MAX_TLS_SOURCE_BYTES / 1024
        )));
    }
    let body = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| FtpError::bad(format!("读取 TLS {kind}失败：{path}（{e}）")))?;
    let marker = if is_cert {
        "BEGIN CERTIFICATE"
    } else {
        "PRIVATE KEY"
    };
    if !body.contains(marker) {
        return Err(FtpError::bad(format!(
            "TLS {kind}不是合法 PEM（缺少 {marker}）：{path}"
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn ensure_tls_pem<H: FtpHost>(host: &H, cfg: &ServiceConfig) -> Result<(), FtpError> {
    if normalize_tls_mode(&cfg.tls_mode).unwrap_or("off") == "off" {
        return Ok(());
    }
    if let (Some(cert), Some(key)) = (&cfg.tls_cert_path, &cfg.tls_key_path) {
        let key_body = tokio::fs::read_to_string(key)
            .await
            .map_err(|e| FtpError::internal(format!("读取 TLS 私钥失败：{e}")))?;
        let cert_body = tokio::fs::read_to_string(cert)
            .await
            .map_err(|e| FtpError::internal(format!("读取 TLS 证书失败：{e}")))?;
        let mut pem = String::new();
        pem.push_str(key_body.trim_end());
        pem.push('\n');
        pem.push_str(cert_body.trim_end());
        pem.push('\n');
        return write_tls_pem(host, &pem).await;
    }
    if Path::new(TLS_PEM).exists() {
        return Ok(());
    }
    let hostname = tokio::fs::read_to_string("/etc/hostname")
        .await
        .unwrap_or_else(|_| "skiff.local".into())
        .trim()
        .to_string();
    let key = "/tmp/skiff-ftp-key.pem";
    let cert = "/tmp/skiff-ftp-cert.pem";
    run_ok(
        host,
        "/usr/bin/openssl",
        &svec(&[
            "req",
            "-x509",
            "-nodes",
            "-newkey",
            "rsa:2048",
            "-days",
            "3650",
            "-keyout",
            key,
            "-out",
            cert,
            "-subj",
            &format!("/CN={hostname}"),
        ]),
        None,
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("生成 TLS 自签证书失败：{}", e.message)))?;
    let key_body = tokio::fs::read_to_string(key)
        .await
        .map_err(|e| FtpError::internal(format!("读取 TLS 私钥失败：{e}")))?;
    let cert_body = tokio::fs::read_to_string(cert)
        .await
        .map_err(|e| FtpError::internal(format!("读取 TLS 证书失败：{e}")))?;
    let mut pem = String::new();
    pem.push_str(key_body.trim_end());
    pem.push('\n');
    pem.push_str(cert_body.trim_end());
    pem.push('\n');
    write_tls_pem(host, &pem).await?;
    let _ = tokio::fs::remove_file(key).await;
    let _ = tokio::fs::remove_file(cert).await;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn write_tls_pem<H: FtpHost>(host: &H, body: &str) -> Result<(), FtpError> {
    if let Some(parent) = Path::new(TLS_PEM).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_ok(host, "/usr/bin/mkdir", &svec(&["-p", &parent]), None, 900)
            .await
            .map_err(|e| FtpError::internal(format!("创建证书目录失败：{}", e.message)))?;
    }
    run_ok(
        host,
        "/usr/bin/tee",
        &[TLS_PEM.to_string()],
        Some(body),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入 {TLS_PEM} 失败：{}", e.message)))?;
    let _ = run_raw(host, "/usr/bin/chmod", &svec(&["600", TLS_PEM]), None, 900).await;
    Ok(())
}

/// fail2ban jail 内容。必须把 `port` 钉到实际监听端口：jail.conf 自带的
/// `[pure-ftpd] port = ftp,ftp-data,ftps,ftps-data`（20/21/989/990）只覆盖默认端口，
/// 一旦把 FTP 改到其它端口，封禁规则就落空——IP 显示已封但仍能登录。
fn jail_body(preserve_sshd: bool, listen_port: u16) -> String {
    let mut body = String::new();
    if preserve_sshd {
        body.push_str("[sshd]\nenabled = false\n\n");
    }
    body.push_str(&format!(
        "[pure-ftpd]\nenabled = true\nbackend = systemd\nport = {listen_port}\nmaxretry = 3\nfindtime = 10m\nbantime = 10m\n"
    ));
    body
}

#[cfg(target_os = "linux")]
async fn ensure_login_guard<H: FtpHost>(
    host: &H,
    had_f2b: bool,
    listen_port: u16,
) -> Result<(), FtpError> {
    if !Path::new("/usr/sbin/fail2ban-client").exists()
        && !Path::new("/usr/bin/fail2ban-client").exists()
    {
        return Err(FtpError::internal("fail2ban 未安装，无法启用登录尝试限制"));
    }
    if let Some(parent) = Path::new(JAIL_FILE).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_ok(host, "/usr/bin/mkdir", &svec(&["-p", &parent]), None, 900)
            .await
            .map_err(|e| FtpError::internal(format!("创建 fail2ban jail.d 失败：{}", e.message)))?;
    }
    // 仅在本次新装 fail2ban 时关闭默认 sshd jail，避免影响宿主既有登录；
    // 已装 fail2ban 的机器不动既有 jail，只追加 pure-ftpd。若之前本插件
    // 关闭过防护但保留了 `[sshd] enabled=false`，这里继续保留，避免反复开关漂移。
    let preserve_sshd = if !had_f2b {
        true
    } else {
        tokio::fs::read_to_string(JAIL_FILE)
            .await
            .unwrap_or_default()
            .contains("[sshd]")
    };
    let body = jail_body(preserve_sshd, listen_port);
    run_ok(
        host,
        "/usr/bin/tee",
        &[JAIL_FILE.to_string()],
        Some(&body),
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入 fail2ban jail 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["enable", "fail2ban"]),
        None,
        900,
    )
    .await
    .map_err(|e| FtpError::internal(format!("启用 fail2ban 失败：{}", e.message)))?;
    run_ok(
        host,
        "/usr/bin/systemctl",
        &svec(&["restart", "fail2ban"]),
        None,
        300,
    )
    .await
    .map_err(|e| FtpError::internal(format!("启动 fail2ban 失败：{}", e.message)))?;
    if !systemctl_active(host, "fail2ban").await {
        return Err(FtpError::internal(
            "fail2ban 未进入 active，登录尝试限制未生效",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct FtpAccount {
    login: String,
    uid: u32,
    gid: u32,
    home: String,
    enabled: bool,
}

#[cfg(target_os = "linux")]
fn read_accounts() -> Result<Vec<FtpAccount>, String> {
    let body = std::fs::read_to_string(PASSWD_FILE).unwrap_or_default();
    let mut out = Vec::new();
    for line in body.lines() {
        let Some((login, enabled)) = account_line_state(line) else {
            continue;
        };
        let content = if enabled {
            line
        } else {
            line.strip_prefix('#').unwrap_or(line)
        };
        let fields: Vec<&str> = content.splitn(7, ':').collect();
        if fields.len() < 6 {
            continue;
        }
        let Ok(uid) = fields[2].parse::<u32>() else {
            continue;
        };
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        out.push(FtpAccount {
            login: login.to_string(),
            uid,
            gid,
            home: fields[5].to_string(),
            enabled,
        });
    }
    out.sort_by(|a, b| a.login.cmp(&b.login));
    Ok(out)
}

#[cfg(target_os = "linux")]
fn normalize_home(home: &str) -> String {
    let mut h = home.trim().to_string();
    while h.ends_with("/./") {
        let keep = h.len() - 3;
        h.truncate(keep);
    }
    while h.ends_with('/') {
        h.pop();
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_strict() {
        assert!(valid_account_name("deploy"));
        assert!(valid_account_name("ci-1_x"));
        assert!(!valid_account_name(""));
        assert!(!valid_account_name("-bad"));
        assert!(!valid_account_name("has space"));
        assert!(!valid_account_name("a/b"));
        assert!(!valid_account_name(&"x".repeat(33)));
    }

    #[test]
    fn passwords_reject_control_chars() {
        assert!(valid_password("12345678"));
        assert!(valid_password("correct horse battery staple"));
        assert!(!valid_password("short"));
        assert!(!valid_password("bad\npass"));
        assert!(!valid_password("bad\0pass"));
        assert!(!valid_password(&"x".repeat(129)));
    }

    #[test]
    fn port_ranges_are_bounded() {
        assert!(valid_port_range(30000, 30100));
        assert!(!valid_port_range(30000, 30000));
        assert!(!valid_port_range(30000, 30600));
        assert!(!valid_port_range(21, 100));
        assert!(valid_port_range(65530, 65535));
    }

    #[test]
    fn pure_db_auth_wiring_matches_debian_layout() {
        assert_eq!(AUTH_DIR, "/etc/pure-ftpd/auth");
        assert_eq!(AUTH_DB_LINK, "../conf/PureDB");
        assert_eq!(AUTH_STALE, ["65unix", "70pam"]);
    }

    #[test]
    fn umask_conf_uses_wrapper_format() {
        assert!(valid_umask(UMASK));
        assert!(!valid_umask("002:002"));
        assert!(!valid_umask("02 02"));
        assert!(!valid_umask("008 002"));
        assert!(!valid_umask("002"));
    }

    /// pure-ftpd 以 0777 为基准减 umask（文件也吃这个基准），目录再继承父目录的
    /// setgid；结果必须与 `align_tree` 的归一目标同形，否则同一棵 `index/` 的
    /// 权限会取决于「有没有跑过 align」。
    #[test]
    fn umask_matches_align_tree_modes() {
        let mut fields = UMASK
            .split_whitespace()
            .map(|f| u32::from_str_radix(f, 8).expect("三位八进制"));
        let file_umask = fields.next().expect("文件掩码");
        let dir_umask = fields.next().expect("目录掩码");

        let uploaded_file = 0o777 & !file_umask;
        let created_dir = (0o777 & !dir_umask) | 0o2000; // 父目录 setgid 继承
        assert_eq!(uploaded_file, 0o664);
        assert_eq!(created_dir, 0o2775);

        // 归一幂等：把新建结果再喂给 align 不变。
        assert_eq!(normalized_mode(false, uploaded_file), uploaded_file);
        assert_eq!(normalized_mode(true, created_dir), created_dir);
        // 可执行文件保持可执行，其余文件归一为 0664。
        assert_eq!(normalized_mode(false, 0o775), 0o775);
        assert_eq!(normalized_mode(false, 0o664), 0o664);
    }

    #[test]
    fn pure_pw_prefers_usr_bin_then_usr_sbin() {
        let both = |p: &str| p == "/usr/bin/pure-pw" || p == "/usr/sbin/pure-pw";
        assert_eq!(pure_pw_from(both), Some("/usr/bin/pure-pw"));
        assert_eq!(
            pure_pw_from(|p| p == "/usr/sbin/pure-pw"),
            Some("/usr/sbin/pure-pw")
        );
        assert_eq!(pure_pw_from(|_| false), None);
    }

    #[test]
    fn login_names_are_stable_and_collision_aware() {
        let l1 = ftp_login("a.b", "deploy");
        assert_eq!(l1, ftp_login("a.b", "deploy"));
        assert!(l1.starts_with("skiff_a-b_"));
        assert!(l1.ends_with("_deploy"));
        assert_ne!(l1, ftp_login("a-b", "deploy"));
        assert!(ftp_login("very-long-domain-name.example.com", "deploy").len() <= 64);
    }

    #[test]
    fn service_config_defaults_and_validation() {
        let cfg = ServiceConfig::default();
        assert_eq!(cfg.listen_port, 21);
        assert_eq!(cfg.max_clients, 30);
        assert_eq!(cfg.max_per_ip, 3);
        assert!(cfg.login_guard);
        assert!(validate_service_config(&cfg).is_ok());

        let mut bad = cfg.clone();
        bad.listen_port = 0;
        assert!(validate_service_config(&bad).is_err());

        let mut bad = cfg.clone();
        bad.max_per_ip = 31;
        assert!(validate_service_config(&bad).is_err());

        let mut bad = cfg.clone();
        bad.tls_mode = "weird".into();
        assert!(validate_service_config(&bad).is_err());

        let mut bad = cfg;
        bad.tls_cert_path = Some("/etc/ssl/a.pem".into());
        assert!(validate_service_config(&bad).is_err());
    }

    #[test]
    fn service_config_merge_keeps_unspecified_fields() {
        let persisted = ServiceConfig::default();
        let cfg = merge_service_config(
            &persisted,
            &json!({
                "listen_port": 2121,
                "max_clients": 50,
                "max_per_ip": 5,
                "tls_mode": "explicit",
                "login_guard": false,
            }),
        )
        .expect("merge");
        assert_eq!(cfg.listen_port, 2121);
        assert_eq!(cfg.max_clients, 50);
        assert_eq!(cfg.max_per_ip, 5);
        assert_eq!(cfg.tls_mode, "optional");
        assert!(!cfg.login_guard);
        assert_eq!(cfg.passive_port_start, persisted.passive_port_start);
        assert!(validate_service_config(&cfg).is_ok());

        let err = merge_service_config(&persisted, &json!({ "listen_port": 70000 }));
        assert!(err.is_err());
        let err = merge_service_config(&persisted, &json!({ "max_per_ip": "x" }));
        assert!(err.is_err());
    }

    #[test]
    fn tls_source_paths_are_strict() {
        assert!(valid_tls_source_path("/etc/ssl/private/skiff.pem"));
        assert!(!valid_tls_source_path("relative.pem"));
        assert!(!valid_tls_source_path("/etc/../etc/shadow"));
        assert!(!valid_tls_source_path("/etc/ssl/"));
        assert!(!valid_tls_source_path(""));
    }

    #[test]
    fn disabled_account_lines_keep_identity() {
        let active = "skiff_a-b_deploy:$1$x:82:82:82:/www/sites/a.b/./::";
        let disabled = format!("#{active}");
        assert_eq!(account_line_state(active), Some(("skiff_a-b_deploy", true)));
        assert_eq!(
            account_line_state(&disabled),
            Some(("skiff_a-b_deploy", false))
        );
        assert_eq!(account_line_state("root:x:0:0::/root"), None);
    }

    #[test]
    fn net_tcp_parsing_counts_listen_and_established() {
        let sample = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1
   1: 0100007F:1538 0100007F:C350 01 00000000:00000000 00:00000000 00000000     0        0 2
   2: 0100007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 3
";
        let entries = parse_net_tcp(sample);
        assert_eq!(entries, vec![(5432, 0x0A), (5432, 0x01), (53, 0x0A)]);
    }

    #[test]
    fn jail_body_pins_configured_listen_port() {
        // 改了监听端口后，jail 必须覆盖 jail.conf 里的默认 ftp 端口，
        // 否则封禁规则只落在 20/21/989/990，显示已封但目标端口仍可登录。
        let body = jail_body(true, 2121);
        assert!(body.contains("port = 2121"), "{body}");
        assert!(body.contains("[sshd]\nenabled = false"), "{body}");
        assert!(body.contains("maxretry = 3"), "{body}");
        assert!(body.contains("bantime = 10m"), "{body}");

        let no_sshd = jail_body(false, 21);
        assert!(!no_sshd.contains("[sshd]"), "{no_sshd}");
        assert!(no_sshd.contains("port = 21"), "{no_sshd}");
    }

    #[test]
    fn fail2ban_status_and_recent_ban_parse() {
        let sample = "\
Status for the jail: pure-ftpd
|- Filter
|  |- Currently failed:\t2
|  `- Total failed:\t9
`- Actions
   |- Currently banned:\t1
   |- Total banned:\t4
   `- Banned IP list:\t203.0.113.7
";
        assert_eq!(parse_fail2ban_status(sample), (Some(1), Some(4)));

        let with_time = "\
203.0.113.7\t2026-11-08 09:12:33
198.51.100.9\t2026-11-07 22:41:02
";
        assert_eq!(
            parse_ban_with_time(with_time).as_deref(),
            Some("198.51.100.9\t2026-11-07 22:41:02")
        );
        assert_eq!(parse_ban_with_time("No IP found"), None);
    }
}
