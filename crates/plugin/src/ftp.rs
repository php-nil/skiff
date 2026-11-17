//! `ftp` 能力面：**只保留只读查询**（服务状态、站点快照、账号库）。
//!
//! 写操作（`install` / `uninstall` / `account.*` / `service.*`）已全部迁到插件侧
//! `plugins/ftp` 经内核 exec 能力面编排；进入本模块 `dispatch` 的写方法统一拒绝。
//!
//! 内核侧只做三件事：查授权、审计、提供只读探测通道（`systemctl is-active` /
//! `dpkg-query` / `fail2ban-client`）。站点快照由内核从 DB 读出后注入
//! （`FtpContext`），插件不查库。方法清单见 `docs/contract/KERNEL_FTP_CONTRACT.md`。
//!
//! Windows 开发机没有这些宿主命令，非 Linux 构建只保留纯校验逻辑（单测在这里跑，
//! 真实动作一律被 `#[cfg(target_os = "linux")]` 挡住）。
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::future::Future;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::pin::Pin;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use serde_json::json;
use serde_json::Value;
#[cfg(target_os = "linux")]
use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

const PASSWD_FILE: &str = "/etc/pure-ftpd/pureftpd.passwd";
const PDB_FILE: &str = "/etc/pure-ftpd/pureftpd.pdb";
const CONF_DIR: &str = "/etc/pure-ftpd/conf";
const JAIL_FILE: &str = "/etc/fail2ban/jail.d/skiff-pureftpd.conf";
const SERVICE: &str = "pure-ftpd";
const LOGIN_PREFIX: &str = "skiff_";
const DEFAULT_PORT_START: u16 = 30000;
const DEFAULT_PORT_END: u16 = 30100;
/// 内核持久化的 FTP 服务配置；`/etc/skiff` 在 systemd unit 的 `ReadWritePaths` 内。
const SERVICE_CONFIG_FILE: &str = "/etc/skiff/ftp-service.json";
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

/// 只读探测通道。内核实现它（`skiff_host`），本模块只声明契约。
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
    pub fn deny(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
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

/// FTP 服务配置。只读查询在未安装时回传已持久化的配置快照。
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
    _params: &Value,
) -> Result<Value, FtpError> {
    match method {
        // ── 只读查询：留内核 ftp 能力面（站点快照、账号库、运行状态） ──
        "status" => status(host).await,
        "sites.list" => sites_list(ctx),
        "accounts.list" => accounts_list(ctx),
        // ── 写操作：已迁移到插件侧 exec 能力面编排 ──
        // 内核 ftp 能力面不再执行宿主命令，插件须经 exec 能力面提交。
        other => Err(FtpError::deny(format!(
            "方法 `ftp.{other}` 已迁移到插件侧 exec 能力面编排，请更新调用方"
        ))),
    }
}

// ───────────────────────────── 纯校验（跨平台可单测） ─────────────────────────

/// 归一化 TLS 模式；`explicit` 是 UI 对「显式 TLS、可选」的别名。
fn normalize_tls_mode(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "0" | "false" => Some("off"),
        "optional" | "explicit" | "1" | "true" => Some("optional"),
        "required" | "2" => Some("required"),
        _ => None,
    }
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

// ───────────────────────────── Linux 实现 ─────────────────────────

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

// ───────────────────────────── 配置持久化与状态探测 ─────────────────────────

#[cfg(target_os = "linux")]
async fn load_service_config() -> ServiceConfig {
    let Ok(raw) = tokio::fs::read_to_string(SERVICE_CONFIG_FILE).await else {
        return ServiceConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
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

// ───────────────────────────── 站点根 ─────────────────────────

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
