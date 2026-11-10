//! `firewall` 能力面：宿主机 nftables/iptables 规则与 fail2ban jail 管理。
//!
//! 与 `ftp` 能力面一样，这是**宿主特权**原语：改防火墙与 `/etc/fail2ban`
//! 需要 root，插件进程始终降权运行。能力面的判定、参数校验与命令编排全部
//! 收在这里（`crates/plugin`），内核只提供两条通用通道：
//!
//! - [`FirewallHost::run`]：以固定二进制 + 参数数组执行特权命令（不经 shell）；
//! - 鉴权/审计/授权查询：由内核 handler 完成，本模块不碰数据库。
//!
//! 设计边界：
//! - 只管理**自己的**对象：nft `inet skiff` 表里的 `input` 链、iptables 的
//!   `SKIFF` 链、`/etc/fail2ban/jail.d/skiff-<jail>.conf`。宿主机既有规则只读；
//! - nft 规则用 `comment "skiff-<id>"` 标识，删除按 handle；iptables 按
//!   `SKIFF` 链内的序号删除；
//! - 默认策略保持 accept：只做「显式放行/显式封禁」，不把主机一键切断；
//! - 所有动态值先做严格格式校验再进 argv；不拼接 shell 字符串。

use std::future::Future;
use std::pin::Pin;

use axum::http::StatusCode;
use serde_json::{json, Value};

/// 宿主命令输出（非零退出也算成功执行，由调用方判断 `success`）。
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 特权命令执行通道。内核实现它（systemd-run 瞬态单元），本模块只声明契约。
pub trait FirewallHost: Send + Sync {
    fn run<'a>(
        &'a self,
        cmd: &'a str,
        args: &'a [String],
        stdin: Option<&'a str>,
        timeout_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>>;
}

/// 能力面错误。`status` 用 HTTP 语义，内核 handler 直接映射成响应码。
#[derive(Debug)]
pub struct FirewallError {
    pub status: StatusCode,
    pub message: String,
}

impl FirewallError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
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

const NFT: &str = "/usr/sbin/nft";
const IPTABLES: &str = "/usr/sbin/iptables";
const FAIL2BAN_CLIENT: &str = "/usr/bin/fail2ban-client";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const TEE: &str = "/usr/bin/tee";
const RM: &str = "/usr/bin/rm";
const APT_GET: &str = "/usr/bin/apt-get";
const LS: &str = "/bin/ls";
const CAT: &str = "/bin/cat";

const IPT_CHAIN: &str = "SKIFF";
const JAIL_DIR: &str = "/etc/fail2ban/jail.d";
/// 本能力面写入的 jail 文件前缀；删除只认这个前缀，避免误删宿主配置。
const JAIL_PREFIX: &str = "skiff-";

/// 能力面入口。`method` 与 manifest 的 `firewall` verbs 一一对应。
pub async fn dispatch<H: FirewallHost>(
    host: &H,
    method: &str,
    params: &Value,
) -> Result<Value, FirewallError> {
    match method {
        "status" => status(host).await,
        "rules.list" => rules_list(host).await,
        "rule.add" => rule_add(host, params).await,
        "rule.remove" => rule_remove(host, params).await,
        "port.allow" => port_allow(host, params).await,
        "fail2ban.status" => fail2ban_status(host).await,
        "fail2ban.install" => fail2ban_install(host).await,
        "fail2ban.start" => fail2ban_start(host).await,
        "fail2ban.stop" => fail2ban_stop(host).await,
        "fail2ban.uninstall" => fail2ban_uninstall(host).await,
        "fail2ban.jail.ensure" => fail2ban_jail_ensure(host, params).await,
        "fail2ban.jail.remove" => fail2ban_jail_remove(host, params).await,
        "fail2ban.banned" => fail2ban_banned(host, params).await,
        "fail2ban.ban" => fail2ban_set_ban(host, params, true).await,
        "fail2ban.unban" => fail2ban_set_ban(host, params, false).await,
        other => Err(FirewallError::bad_request(format!(
            "未知的 firewall 方法：{other}"
        ))),
    }
}

// ───────────────────────────── 能力面方法 ─────────────────────────────

async fn status<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    let nft = command_works(host, NFT, &["--version".to_string()]).await;
    let iptables = command_works(host, IPTABLES, &["--version".to_string()]).await;
    let backend = if nft {
        Some("nftables")
    } else if iptables {
        Some("iptables")
    } else {
        None
    };
    let rules = if backend.is_some() {
        rules_list(host)
            .await
            .unwrap_or_else(|_| json!({ "items": [] }))
    } else {
        json!({ "items": [] })
    };
    let f2b = fail2ban_status(host)
        .await
        .unwrap_or_else(|e| json!({ "installed": null, "running": null, "error": e.message }));
    Ok(json!({
        "backend": backend,
        "nftables": nft,
        "iptables": iptables,
        "managed_rules": rules.get("items").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
        "fail2ban": f2b,
        "warnings": if backend.is_none() {
            json!(["宿主机未找到 nft 或 iptables，无法管理防火墙规则"])
        } else {
            json!([])
        },
    }))
}

async fn rules_list<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    let Some(backend) = detect_backend(host).await else {
        return Ok(json!({ "backend": null, "items": [] }));
    };
    let items = match backend {
        Backend::Nft => {
            let out = exec_ok(
                host,
                NFT,
                &[
                    "-a".to_string(),
                    "list".to_string(),
                    "chain".to_string(),
                    "inet".to_string(),
                    "skiff".to_string(),
                    "input".to_string(),
                ],
                None,
                30,
            )
            .await;
            match out {
                Ok(text) => parse_nft_rules(&text),
                Err(_) => Vec::new(),
            }
        }
        Backend::Iptables => {
            let out = exec_ok(
                host,
                IPTABLES,
                &["-S".to_string(), IPT_CHAIN.to_string()],
                None,
                30,
            )
            .await;
            match out {
                Ok(text) => parse_iptables_rules(&text),
                Err(_) => Vec::new(),
            }
        }
    };
    Ok(json!({ "backend": backend.as_str(), "items": items }))
}

async fn rule_add<H: FirewallHost>(host: &H, params: &Value) -> Result<Value, FirewallError> {
    let spec = RuleSpec::parse(params)?;
    let id = new_rule_id();
    let backend = ensure_backend(host).await?;
    match backend {
        Backend::Nft => add_nft_rule(host, &spec, &id).await?,
        Backend::Iptables => add_iptables_rule(host, &spec, &id).await?,
    }
    Ok(json!({
        "ok": true,
        "backend": backend.as_str(),
        "id": id,
        "rule": spec.to_json(&id),
    }))
}

async fn rule_remove<H: FirewallHost>(host: &H, params: &Value) -> Result<Value, FirewallError> {
    let id = require_str(params, "id")?;
    if !valid_rule_id(&id) {
        return Err(FirewallError::bad_request("规则 id 非法"));
    }
    let Some(backend) = detect_backend(host).await else {
        return Err(FirewallError::unavailable("宿主机未找到 nft 或 iptables"));
    };
    let removed = match backend {
        Backend::Nft => remove_nft_rule(host, &id).await?,
        Backend::Iptables => remove_iptables_rule(host, &id).await?,
    };
    if !removed {
        return Err(FirewallError::bad_request(format!("未找到规则：{id}")));
    }
    Ok(json!({ "ok": true, "backend": backend.as_str(), "id": id }))
}

async fn port_allow<H: FirewallHost>(host: &H, params: &Value) -> Result<Value, FirewallError> {
    let port = require_port(params)?;
    let protocol = params
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp")
        .trim()
        .to_ascii_lowercase();
    let protocols: Vec<&str> = match protocol.as_str() {
        "tcp" => vec!["tcp"],
        "udp" => vec!["udp"],
        "both" => vec!["tcp", "udp"],
        other => {
            return Err(FirewallError::bad_request(format!(
                "protocol 只支持 tcp / udp / both，收到 {other}"
            )))
        }
    };
    let mut added = Vec::new();
    for proto in protocols {
        let spec = RuleSpec {
            protocol: Some(proto.to_string()),
            port: Some(port),
            source: None,
            action: "accept".to_string(),
        };
        let id = new_rule_id();
        let backend = ensure_backend(host).await?;
        match backend {
            Backend::Nft => add_nft_rule(host, &spec, &id).await?,
            Backend::Iptables => add_iptables_rule(host, &spec, &id).await?,
        }
        added.push(json!({ "id": id, "protocol": proto, "port": port }));
    }
    Ok(json!({ "ok": true, "added": added }))
}

async fn fail2ban_status<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    let installed = command_works(host, FAIL2BAN_CLIENT, &["--version".to_string()]).await;
    if !installed {
        return Ok(json!({
            "installed": false,
            "running": false,
            "state": "missing",
            "reason": "未安装 fail2ban（找不到 fail2ban-client）",
            "jails": [],
            "service": "fail2ban",
        }));
    }
    // `systemctl is-active` 在 inactive 时以非零码退出，用 stdout 区分「已停」与「启动失败」。
    let (running, reason) = match command_output(
        host,
        SYSTEMCTL,
        &["is-active".to_string(), "fail2ban".to_string()],
    )
    .await
    {
        Ok(o) if o.success && o.stdout.trim() == "active" => (true, String::new()),
        Ok(o) => (false, state_reason(o.stdout.trim(), o.stderr.trim())),
        Err(e) => (false, format!("无法查询服务状态：{}", e.message)),
    };
    let jails = if running {
        active_jails(host).await
    } else {
        Vec::new()
    };
    Ok(json!({
        "installed": true,
        "running": running,
        "state": if running { "running" } else { "stopped" },
        "reason": reason,
        "jails": jails,
        "service": "fail2ban",
    }))
}

/// 把 `systemctl is-active` 的原始状态翻成可读原因；未知状态原样带出，便于排障。
fn state_reason(stdout: &str, stderr: &str) -> String {
    match stdout {
        "inactive" => "服务未运行".to_string(),
        "failed" => "服务启动失败（systemd 状态 failed）".to_string(),
        "activating" => "服务正在启动".to_string(),
        "deactivating" => "服务正在停止".to_string(),
        "" if !stderr.is_empty() => stderr.to_string(),
        "" => "服务未运行".to_string(),
        other => other.to_string(),
    }
}

async fn require_fail2ban_installed<H: FirewallHost>(
    host: &H,
    action: &str,
) -> Result<(), FirewallError> {
    if !command_works(host, FAIL2BAN_CLIENT, &["--version".to_string()]).await {
        return Err(FirewallError::unavailable(format!(
            "fail2ban 未安装，无法{action}；请先安装"
        )));
    }
    Ok(())
}

async fn fail2ban_start<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    require_fail2ban_installed(host, "启动").await?;
    // `enable --now` 对已在运行的服务是 no-op，改过的 jail.d 不会被重读；
    // 开机自启与「本次生效」拆成两步，restart 负责后者。
    exec_ok(host, SYSTEMCTL, &svec(&["enable", "fail2ban"]), None, 120)
        .await
        .map_err(|e| FirewallError::internal(format!("启动 fail2ban 失败：{}", e.message)))?;
    restart_and_verify(host).await
}

async fn fail2ban_stop<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    require_fail2ban_installed(host, "停止").await?;
    exec_ok(
        host,
        SYSTEMCTL,
        &svec(&["disable", "--now", "fail2ban"]),
        None,
        120,
    )
    .await
    .map_err(|e| FirewallError::internal(format!("停止 fail2ban 失败：{}", e.message)))?;
    fail2ban_status(host).await
}

/// 卸载宿主 fail2ban：先停服务，再 `apt-get purge`，最后回报真实状态。
///
/// 只删除本能力面写入的 `skiff-defaults.conf`；其它 `skiff-<jail>.conf`
/// 是用户显式创建的配置，保留在原地（重装后仍可复用）。
async fn fail2ban_uninstall<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    require_fail2ban_installed(host, "卸载").await?;
    // 停止失败不阻断卸载，但要如实带进返回值，避免「看起来干净」。
    let stop_warning = exec_ok(
        host,
        SYSTEMCTL,
        &svec(&["disable", "--now", "fail2ban"]),
        None,
        120,
    )
    .await
    .err()
    .map(|e| e.message);
    let defaults = format!("{JAIL_DIR}/{JAIL_PREFIX}defaults.conf");
    let _ = exec_ok(host, RM, &svec(&["-f", &defaults]), None, 30).await;
    let out = exec(
        host,
        APT_GET,
        &svec(&["purge", "-y", "fail2ban"]),
        None,
        900,
    )
    .await?;
    if !out.success {
        let detail = if out.stderr.trim().is_empty() {
            out.stdout.trim().to_string()
        } else {
            out.stderr.trim().to_string()
        };
        return Err(FirewallError::internal(format!(
            "卸载 fail2ban 失败：{detail}"
        )));
    }
    let mut result = fail2ban_status(host).await?;
    if let Some(warning) = stop_warning {
        result["warning"] = json!(format!("停止服务时出现问题：{warning}"));
    }
    Ok(result)
}

async fn fail2ban_install<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    let before = command_works(host, FAIL2BAN_CLIENT, &["--version".to_string()]).await;
    if !before {
        let args: Vec<String> = ["install", "-y", "--no-install-recommends", "fail2ban"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let first = exec(host, APT_GET, &args, None, 900).await?;
        if !first.success {
            let detail = first.stderr.trim().to_string();
            if detail.contains("Unable to locate package")
                || detail.contains("has no installation candidate")
            {
                let update: Vec<String> = vec!["update".to_string()];
                exec_ok(host, APT_GET, &update, None, 900).await?;
                exec_ok(host, APT_GET, &args, None, 900).await?;
            } else {
                return Err(FirewallError::internal(format!(
                    "安装 fail2ban 失败：{}",
                    if detail.is_empty() {
                        first.stdout.trim().to_string()
                    } else {
                        detail
                    }
                )));
            }
        }
        // 新装 fail2ban 时显式关闭 sshd jail，避免顺手把宿主的 SSH 登录封掉。
        // 已存在 fail2ban 的机器不改它的既有配置。
        let content = "[sshd]\nenabled = false\n";
        write_managed_file(
            host,
            &format!("{JAIL_DIR}/{JAIL_PREFIX}defaults.conf"),
            content,
        )
        .await?;
    }
    // 包安装的 postinst 会先把服务拉起来，此时 `enable --now` 是 no-op：
    // 刚写的 jail.d 不会被读取，sshd 之类镜像自带 jail 会继续生效。
    // 因此这里只负责开机自启，生效交给 restart_and_verify 的显式重启与复核。
    exec_ok(host, SYSTEMCTL, &svec(&["enable", "fail2ban"]), None, 300).await?;
    restart_and_verify(host).await
}

async fn fail2ban_jail_ensure<H: FirewallHost>(
    host: &H,
    params: &Value,
) -> Result<Value, FirewallError> {
    let jail = require_jail(params)?;
    let port = params.get("port").and_then(Value::as_str).unwrap_or("");
    if !port.is_empty() && !valid_port_spec(port) {
        return Err(FirewallError::bad_request(
            "port 只能是数字、逗号与连字符（如 21 或 30000-30100）",
        ));
    }
    let filter = params
        .get("filter")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if filter.is_empty() || !valid_name(&filter, 64) {
        return Err(FirewallError::bad_request(
            "filter 必填，且只能包含字母、数字、下划线与连字符",
        ));
    }
    let backend = params
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("systemd")
        .trim()
        .to_string();
    if !valid_backend(&backend) {
        return Err(FirewallError::bad_request(
            "backend 只支持 auto/systemd/polling/pyinotify/gamin",
        ));
    }
    let logpath = params
        .get("logpath")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if !logpath.is_empty() && (!logpath.starts_with('/') || !safe_text(&logpath, 255)) {
        return Err(FirewallError::bad_request(
            "logpath 必须是以 / 开头的安全路径",
        ));
    }
    let maxretry = params.get("maxretry").and_then(Value::as_u64).unwrap_or(3);
    let findtime = params
        .get("findtime")
        .and_then(Value::as_u64)
        .unwrap_or(600);
    let bantime = params.get("bantime").and_then(Value::as_u64).unwrap_or(600);
    if !(1..=100).contains(&maxretry) {
        return Err(FirewallError::bad_request("maxretry 需在 1–100"));
    }
    if findtime == 0 || findtime > 86400 || bantime == 0 || bantime > 31_536_000 {
        return Err(FirewallError::bad_request(
            "findtime 需在 1–86400 秒，bantime 需在 1–31536000 秒",
        ));
    }

    let mut text = String::new();
    text.push_str(&format!("[{jail}]\n"));
    text.push_str("enabled = true\n");
    if !port.is_empty() {
        text.push_str(&format!("port = {port}\n"));
    }
    text.push_str(&format!("filter = {filter}\n"));
    text.push_str(&format!("backend = {backend}\n"));
    if !logpath.is_empty() {
        text.push_str(&format!("logpath = {logpath}\n"));
    }
    text.push_str(&format!("maxretry = {maxretry}\n"));
    text.push_str(&format!("findtime = {findtime}\n"));
    text.push_str(&format!("bantime = {bantime}\n"));

    let file = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    write_managed_file(host, &file, &text).await?;
    restart_fail2ban(host).await?;
    Ok(json!({
        "ok": true,
        "jail": jail,
        "file": file,
        "restarted": true,
        "config": {
            "port": port,
            "filter": filter,
            "backend": backend,
            "maxretry": maxretry,
            "findtime": findtime,
            "bantime": bantime,
        },
    }))
}

async fn fail2ban_jail_remove<H: FirewallHost>(
    host: &H,
    params: &Value,
) -> Result<Value, FirewallError> {
    let jail = require_jail(params)?;
    let file = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    let args = vec!["-f".to_string(), file.clone()];
    exec_ok(host, RM, &args, None, 30).await?;
    restart_fail2ban(host).await?;
    Ok(json!({ "ok": true, "jail": jail, "removed": file }))
}

async fn fail2ban_banned<H: FirewallHost>(
    host: &H,
    params: &Value,
) -> Result<Value, FirewallError> {
    let jail = require_jail(params)?;
    let out = exec_ok(
        host,
        FAIL2BAN_CLIENT,
        &["status".to_string(), jail.clone()],
        None,
        30,
    )
    .await?;
    Ok(json!({
        "jail": jail,
        "banned": parse_banned(&out),
    }))
}

async fn fail2ban_set_ban<H: FirewallHost>(
    host: &H,
    params: &Value,
    ban: bool,
) -> Result<Value, FirewallError> {
    let jail = require_jail(params)?;
    let ip = require_ip(params)?;
    let verb = if ban { "banip" } else { "unbanip" };
    exec_ok(
        host,
        FAIL2BAN_CLIENT,
        &[
            "set".to_string(),
            jail.clone(),
            verb.to_string(),
            ip.clone(),
        ],
        None,
        30,
    )
    .await?;
    Ok(json!({ "ok": true, "jail": jail, "ip": ip, "banned": ban }))
}

// ───────────────────────────── 规则实现 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Nft,
    Iptables,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Backend::Nft => "nftables",
            Backend::Iptables => "iptables",
        }
    }
}

#[derive(Debug, Clone)]
struct RuleSpec {
    /// `tcp` / `udp`；`None` 表示不限定协议（仅用于无端口的封禁/放行）。
    protocol: Option<String>,
    port: Option<u16>,
    source: Option<String>,
    /// `accept` / `drop`。
    action: String,
}

impl RuleSpec {
    fn parse(params: &Value) -> Result<Self, FirewallError> {
        let action = params
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("accept")
            .trim()
            .to_ascii_lowercase();
        if action != "accept" && action != "drop" {
            return Err(FirewallError::bad_request("action 只支持 accept / drop"));
        }
        let protocol = match params
            .get("protocol")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase())
        {
            Some(p) if p == "tcp" || p == "udp" => Some(p),
            Some(p) if p == "all" || p.is_empty() => None,
            Some(other) => {
                return Err(FirewallError::bad_request(format!(
                    "protocol 只支持 tcp / udp / all，收到 {other}"
                )))
            }
            None => None,
        };
        let port = match params.get("port") {
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| FirewallError::bad_request("port 必须是整数"))?;
                if !(1..=65535).contains(&n) {
                    return Err(FirewallError::bad_request("port 需在 1–65535"));
                }
                if protocol.is_none() {
                    return Err(FirewallError::bad_request(
                        "指定 port 时必须同时指定 tcp/udp 协议",
                    ));
                }
                Some(n as u16)
            }
            None => None,
        };
        let source = match params.get("source").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => {
                let s = s.trim().to_string();
                if !valid_cidr(&s) {
                    return Err(FirewallError::bad_request("source 必须是合法 IP 或 CIDR"));
                }
                Some(s)
            }
            _ => None,
        };
        if port.is_none() && source.is_none() {
            return Err(FirewallError::bad_request(
                "规则至少要有 port 或 source 之一，拒绝空规则",
            ));
        }
        Ok(Self {
            protocol,
            port,
            source,
            action,
        })
    }

    fn to_json(&self, id: &str) -> Value {
        json!({
            "id": id,
            "action": self.action,
            "protocol": self.protocol,
            "port": self.port,
            "source": self.source,
        })
    }
}

async fn ensure_backend<H: FirewallHost>(host: &H) -> Result<Backend, FirewallError> {
    let backend = detect_backend(host)
        .await
        .ok_or_else(|| FirewallError::unavailable("宿主机未找到 nft 或 iptables"))?;
    match backend {
        Backend::Nft => {
            // 已存在时 nft 返回非零；这里只保证「存在」。
            let _ = exec(
                host,
                NFT,
                &svec(&["add", "table", "inet", "skiff"]),
                None,
                30,
            )
            .await;
            let chain = "{ type filter hook input priority 0 ; policy accept ; }".to_string();
            let args = vec![
                "add".to_string(),
                "chain".to_string(),
                "inet".to_string(),
                "skiff".to_string(),
                "input".to_string(),
                chain,
            ];
            let _ = exec(host, NFT, &args, None, 30).await;
        }
        Backend::Iptables => {
            let _ = exec(host, IPTABLES, &svec(&["-N", IPT_CHAIN]), None, 30).await;
            let probe = exec(
                host,
                IPTABLES,
                &svec(&["-C", "INPUT", "-j", IPT_CHAIN]),
                None,
                30,
            )
            .await?;
            if !probe.success {
                exec_ok(
                    host,
                    IPTABLES,
                    &svec(&["-I", "INPUT", "1", "-j", IPT_CHAIN]),
                    None,
                    30,
                )
                .await?;
            }
        }
    }
    Ok(backend)
}

async fn add_nft_rule<H: FirewallHost>(
    host: &H,
    spec: &RuleSpec,
    id: &str,
) -> Result<(), FirewallError> {
    let mut args = svec(&["add", "rule", "inet", "skiff", "input"]);
    if let Some(src) = &spec.source {
        args.push(if src.contains(':') { "ip6" } else { "ip" }.to_string());
        args.push("saddr".to_string());
        args.push(src.clone());
    }
    if let Some(proto) = &spec.protocol {
        args.push(proto.clone());
        if let Some(port) = spec.port {
            args.push("dport".to_string());
            args.push(port.to_string());
        }
    }
    args.push(spec.action.clone());
    args.push("comment".to_string());
    // nft 的 comment 必须是带引号的字符串；argv 原样透传，引号要一起带上。
    args.push(format!("\"{id}\""));
    exec_ok(host, NFT, &args, None, 30).await?;
    Ok(())
}

async fn add_iptables_rule<H: FirewallHost>(
    host: &H,
    spec: &RuleSpec,
    id: &str,
) -> Result<(), FirewallError> {
    let mut args = svec(&["-A", IPT_CHAIN]);
    if let Some(src) = &spec.source {
        args.push("-s".to_string());
        args.push(src.clone());
    }
    if let Some(proto) = &spec.protocol {
        args.push("-p".to_string());
        args.push(proto.clone());
        if let Some(port) = spec.port {
            args.push("--dport".to_string());
            args.push(port.to_string());
        }
    }
    // `-m comment` 必须出现在 `-j` 之前，否则 iptables 会把它当成目标之后的未知选项。
    args.push("-m".to_string());
    args.push("comment".to_string());
    args.push("--comment".to_string());
    args.push(id.to_string());
    args.push("-j".to_string());
    args.push(spec.action.to_ascii_uppercase());
    exec_ok(host, IPTABLES, &args, None, 30).await?;
    Ok(())
}

async fn remove_nft_rule<H: FirewallHost>(host: &H, id: &str) -> Result<bool, FirewallError> {
    let out = exec_ok(
        host,
        NFT,
        &[
            "-a".to_string(),
            "list".to_string(),
            "chain".to_string(),
            "inet".to_string(),
            "skiff".to_string(),
            "input".to_string(),
        ],
        None,
        30,
    )
    .await?;
    let Some(handle) = find_nft_handle(&out, id) else {
        return Ok(false);
    };
    let args = vec![
        "delete".to_string(),
        "rule".to_string(),
        "inet".to_string(),
        "skiff".to_string(),
        "input".to_string(),
        "handle".to_string(),
        handle.to_string(),
    ];
    exec_ok(host, NFT, &args, None, 30).await?;
    Ok(true)
}

async fn remove_iptables_rule<H: FirewallHost>(host: &H, id: &str) -> Result<bool, FirewallError> {
    let out = exec_ok(
        host,
        IPTABLES,
        &["-S".to_string(), IPT_CHAIN.to_string()],
        None,
        30,
    )
    .await?;
    let Some(index) = find_iptables_index(&out, id) else {
        return Ok(false);
    };
    exec_ok(
        host,
        IPTABLES,
        &["-D".to_string(), IPT_CHAIN.to_string(), index.to_string()],
        None,
        30,
    )
    .await?;
    Ok(true)
}

// ───────────────────────────── 解析与校验 ─────────────────────────────

fn parse_nft_rules(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("table")
            || line.starts_with("chain")
            || line.starts_with('}')
        {
            continue;
        }
        let Some(handle) = line
            .rsplit_once("# handle ")
            .and_then(|(_, n)| n.trim().parse::<u64>().ok())
        else {
            continue;
        };
        let Some(id) = extract_comment(line) else {
            continue;
        };
        if !valid_rule_id(&id) {
            continue;
        }
        let action = if line.contains(" accept") {
            "accept"
        } else if line.contains(" drop") {
            "drop"
        } else {
            "-"
        };
        let protocol = if line.contains(" tcp ") || line.starts_with("tcp ") {
            Some("tcp")
        } else if line.contains(" udp ") || line.starts_with("udp ") {
            Some("udp")
        } else {
            None
        };
        out.push(json!({
            "id": id,
            "handle": handle,
            "action": action,
            "protocol": protocol,
            "port": token_after(line, "dport "),
            "source": token_after(line, "saddr "),
        }));
    }
    out.sort_by_key(|v| v.get("handle").and_then(Value::as_u64).unwrap_or(0));
    out
}

fn parse_iptables_rules(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut index = 0u64;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("-A ") {
            continue;
        }
        index += 1;
        let Some(id) = extract_comment(line) else {
            continue;
        };
        if !valid_rule_id(&id) {
            continue;
        }
        let action = line
            .split_whitespace()
            .skip_while(|t| *t != "-j")
            .nth(1)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "-".to_string());
        let protocol = line
            .split_whitespace()
            .skip_while(|t| *t != "-p")
            .nth(1)
            .map(str::to_string);
        let port = token_after(line, "--dport ");
        let source = token_after(line, "-s ");
        out.push(json!({
            "id": id,
            "index": index,
            "action": action,
            "protocol": protocol,
            "port": port,
            "source": source,
        }));
    }
    out
}

fn extract_comment(line: &str) -> Option<String> {
    if let Some((_, rest)) = line.split_once("comment \"") {
        return rest.split_once('"').map(|(value, _)| value.to_string());
    }
    line.split_once("--comment ")
        .map(|(_, rest)| rest.split_whitespace().next().unwrap_or("").to_string())
        .filter(|s| !s.is_empty())
}

fn token_after(line: &str, needle: &str) -> Option<String> {
    let (_, rest) = line.split_once(needle)?;
    rest.split_whitespace()
        .next()
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
}

fn find_nft_handle(text: &str, id: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        if extract_comment(line).as_deref() != Some(id) {
            return None;
        }
        line.rsplit_once("# handle ")
            .and_then(|(_, n)| n.trim().parse::<u64>().ok())
    })
}

fn find_iptables_index(text: &str, id: &str) -> Option<u64> {
    let mut index = 0u64;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("-A ") {
            continue;
        }
        index += 1;
        if extract_comment(line).as_deref() == Some(id) {
            return Some(index);
        }
    }
    None
}

fn parse_jail_list(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("Jail list:") else {
        return Vec::new();
    };
    rest.lines()
        .next()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_banned(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("Banned IP list:") else {
        return Vec::new();
    };
    rest.lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn valid_rule_id(id: &str) -> bool {
    id.len() <= 40
        && id.starts_with(JAIL_PREFIX)
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn valid_name(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_backend(s: &str) -> bool {
    matches!(s, "auto" | "systemd" | "polling" | "pyinotify" | "gamin")
}

fn valid_port_spec(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '-')
}

fn valid_cidr(s: &str) -> bool {
    use std::net::IpAddr;
    match s.split_once('/') {
        Some((addr, prefix)) => {
            let Ok(ip) = addr.parse::<IpAddr>() else {
                return false;
            };
            let Ok(n) = prefix.parse::<u8>() else {
                return false;
            };
            match ip {
                IpAddr::V4(_) => n <= 32,
                IpAddr::V6(_) => n <= 128,
            }
        }
        None => s.parse::<IpAddr>().is_ok(),
    }
}

fn safe_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control())
}

fn require_str(params: &Value, key: &str) -> Result<String, FirewallError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| FirewallError::bad_request(format!("缺少 {key}")))
}

fn require_jail(params: &Value) -> Result<String, FirewallError> {
    let jail = require_str(params, "jail")?;
    if !valid_name(&jail, 32) {
        return Err(FirewallError::bad_request(
            "jail 只能包含字母、数字、下划线与连字符，长度 1–32",
        ));
    }
    Ok(jail)
}

fn require_port(params: &Value) -> Result<u16, FirewallError> {
    let n = params
        .get("port")
        .and_then(Value::as_u64)
        .ok_or_else(|| FirewallError::bad_request("缺少 port"))?;
    if !(1..=65535).contains(&n) {
        return Err(FirewallError::bad_request("port 需在 1–65535"));
    }
    Ok(n as u16)
}

fn require_ip(params: &Value) -> Result<String, FirewallError> {
    let ip = require_str(params, "ip")?;
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Err(FirewallError::bad_request("ip 不是合法 IPv4/IPv6 地址"));
    }
    Ok(ip)
}

/// 同一进程内的规则序号：Windows 的 `SystemTime` 粒度较粗，连续两次取纳秒可能相同，
/// 仅靠时间戳会生成重复 id。
static RULE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn new_rule_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = RULE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{JAIL_PREFIX}{nanos}-{seq}")
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ───────────────────────────── 宿主命令封装 ─────────────────────────────

async fn exec<H: FirewallHost>(
    host: &H,
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    secs: u64,
) -> Result<CommandOutput, FirewallError> {
    host.run(cmd, args, stdin, secs)
        .await
        .map_err(FirewallError::internal)
}

async fn exec_ok<H: FirewallHost>(
    host: &H,
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    secs: u64,
) -> Result<String, FirewallError> {
    let out = exec(host, cmd, args, stdin, secs).await?;
    if !out.success {
        let detail = if out.stderr.trim().is_empty() {
            out.stdout.trim().to_string()
        } else {
            out.stderr.trim().to_string()
        };
        return Err(FirewallError::internal(format!("{cmd} 执行失败：{detail}")));
    }
    Ok(out.stdout)
}

async fn command_output<H: FirewallHost>(
    host: &H,
    cmd: &str,
    args: &[String],
) -> Result<CommandOutput, FirewallError> {
    exec(host, cmd, args, None, 30).await
}

async fn command_works<H: FirewallHost>(host: &H, cmd: &str, args: &[String]) -> bool {
    command_output(host, cmd, args)
        .await
        .map(|o| o.success)
        .unwrap_or(false)
}

async fn detect_backend<H: FirewallHost>(host: &H) -> Option<Backend> {
    if command_works(host, NFT, &["--version".to_string()]).await {
        return Some(Backend::Nft);
    }
    if command_works(host, IPTABLES, &["--version".to_string()]).await {
        return Some(Backend::Iptables);
    }
    None
}

async fn write_managed_file<H: FirewallHost>(
    host: &H,
    path: &str,
    content: &str,
) -> Result<(), FirewallError> {
    exec_ok(host, TEE, &[path.to_string()], Some(content), 30)
        .await
        .map(|_| ())
}

async fn restart_fail2ban<H: FirewallHost>(host: &H) -> Result<(), FirewallError> {
    let args = svec(&["restart", "fail2ban"]);
    exec_ok(host, SYSTEMCTL, &args, None, 300).await.map(|_| ())
}

async fn append_managed_file<H: FirewallHost>(
    host: &H,
    path: &str,
    content: &str,
) -> Result<(), FirewallError> {
    exec_ok(host, TEE, &svec(&["-a", path]), Some(content), 30)
        .await
        .map(|_| ())
}

/// 本能力面托管配置中显式启用的 jail 集合（`skiff-*.conf` 里 `enabled = true`）。
///
/// 只读文件名带 `JAIL_PREFIX` 的配置：宿主机自己的 `jail.local` / `defaults-debian.conf`
/// 不参与判定，避免把别人的选择当成我们的预期。
async fn managed_jails<H: FirewallHost>(host: &H) -> Vec<String> {
    let listing = match command_output(host, LS, &svec(&["-1", JAIL_DIR])).await {
        Ok(o) if o.success => o.stdout,
        _ => return Vec::new(),
    };
    let mut jails: Vec<String> = Vec::new();
    for name in listing.lines().map(str::trim) {
        if !name.starts_with(JAIL_PREFIX) || !name.ends_with(".conf") {
            continue;
        }
        let path = format!("{JAIL_DIR}/{name}");
        let text = match command_output(host, CAT, &[path]).await {
            Ok(o) if o.success => o.stdout,
            _ => continue,
        };
        for jail in enabled_jails(&text) {
            if !jails.contains(&jail) {
                jails.push(jail);
            }
        }
    }
    jails.sort();
    jails
}

/// 解析 ini 里 `enabled = true` 的段名（支持 fail2ban 的 `true/yes/1/on` 写法）。
fn enabled_jails(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut section: Option<String> = None;
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            section = rest.strip_suffix(']').map(|s| s.trim().to_string());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("enabled") {
            continue;
        }
        let on = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "1" | "on"
        );
        if on {
            if let Some(name) = &section {
                out.push(name.clone());
            }
        }
    }
    out
}

/// 服务实际启用的 jail（`fail2ban-client status` 的 Jail list）。
async fn active_jails<H: FirewallHost>(host: &H) -> Vec<String> {
    command_output(host, FAIL2BAN_CLIENT, &svec(&["status"]))
        .await
        .map(|o| parse_jail_list(&o.stdout))
        .unwrap_or_default()
}

/// 托管默认配置是否存在：存在才说明本能力面拥有 jail 集合的决定权。
async fn managed_defaults_text<H: FirewallHost>(host: &H) -> Option<String> {
    let path = format!("{JAIL_DIR}/{JAIL_PREFIX}defaults.conf");
    match command_output(host, CAT, &[path]).await {
        Ok(o) if o.success => Some(o.stdout),
        _ => None,
    }
}

/// 配置里是否已经出现过该段（不区分 true/false）。
fn has_section(text: &str, jail: &str) -> bool {
    let target = format!("[{jail}]");
    text.lines().any(|l| l.trim() == target)
}

/// 显式重启 fail2ban 并复核实际启用的 jail 集合。
///
/// 重启是必须的：包安装的 postinst 已经拉起服务，`enable --now` 不会让新写的
/// `jail.d/*.conf` 生效。重启后对照本能力面托管配置：
/// - 预期 = `skiff-*.conf` 里 `enabled = true` 的段；
/// - 多出来的 jail（典型是 Debian 镜像自带的 `sshd`）先 `fail2ban-client stop`
///   立即生效；`skiff-defaults.conf` 里还没写过该段时，再追加 `enabled = false`，
///   最后重启让「关掉」落到配置层。
///
/// 返回体以**复核后的真实状态**为准：宿主的覆盖文件排序晚于我们的配置
/// （如 `zz-sshd.conf`）时，重启会把它重新启用，此时 `jails` 仍含该 jail。
/// 因此 `disabled_jails` 只登记「请求停用且复核已消失」的 jail；请求停用但
/// 复核后仍存在的写进 `residual_jails`，避免同一响应里两处自相矛盾。
///
/// 只有在托管默认配置存在时才做这一步：宿主机原本就装了 fail2ban 并自行配置
/// 的 jail 不属于本能力面的管辖范围，不能被静默关掉。
async fn restart_and_verify<H: FirewallHost>(host: &H) -> Result<Value, FirewallError> {
    restart_fail2ban(host).await?;
    let mut requested: Vec<String> = Vec::new();
    if let Some(defaults) = managed_defaults_text(host).await {
        let expected = managed_jails(host).await;
        let unexpected: Vec<String> = active_jails(host)
            .await
            .into_iter()
            .filter(|j| !expected.contains(j))
            .collect();
        if !unexpected.is_empty() {
            let mut extra = String::new();
            for jail in &unexpected {
                // stop 失败不阻断：配置层关掉后重启仍能生效。
                let _ = exec_ok(host, FAIL2BAN_CLIENT, &svec(&["stop", jail]), None, 60).await;
                if !has_section(&defaults, jail) {
                    extra.push_str(&format!("[{jail}]\nenabled = false\n"));
                }
            }
            if !extra.is_empty() {
                append_managed_file(
                    host,
                    &format!("{JAIL_DIR}/{JAIL_PREFIX}defaults.conf"),
                    &extra,
                )
                .await?;
            }
            restart_fail2ban(host).await?;
            requested = unexpected;
        }
    }
    let mut result = fail2ban_status(host).await?;
    if !requested.is_empty() {
        let active: Vec<String> = result["jails"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut disabled: Vec<String> = Vec::new();
        let mut residual: Vec<String> = Vec::new();
        for jail in requested {
            if active.iter().any(|a| a == &jail) {
                residual.push(jail);
            } else {
                disabled.push(jail);
            }
        }
        if !disabled.is_empty() {
            result["disabled_jails"] = json!(disabled);
        }
        if !residual.is_empty() {
            result["residual_jails"] = json!(residual);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假宿主的应答回调：避免 clippy::type_complexity。
    type Respond = Box<dyn Fn(&str, &[String]) -> CommandOutput + Send + Sync>;

    /// 假宿主：把收到的命令行记下来，按脚本返回输出。
    struct FakeHost {
        calls: std::sync::Mutex<Vec<String>>,
        respond: Respond,
    }

    impl FakeHost {
        fn new<F>(respond: F) -> Self
        where
            F: Fn(&str, &[String]) -> CommandOutput + Send + Sync + 'static,
        {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
                respond: Box::new(respond),
            }
        }
    }

    impl FirewallHost for FakeHost {
        fn run<'a>(
            &'a self,
            cmd: &'a str,
            args: &'a [String],
            _stdin: Option<&'a str>,
            _timeout_secs: u64,
        ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("{cmd} {}", args.join(" ")));
                Ok((self.respond)(cmd, args))
            })
        }
    }

    fn ok(stdout: &str) -> CommandOutput {
        CommandOutput {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    #[test]
    fn jail_and_port_validation() {
        assert!(valid_name("pure-ftpd", 32));
        assert!(!valid_name("../x", 32));
        assert!(valid_port_spec("21"));
        assert!(valid_port_spec("30000-30100,21"));
        assert!(!valid_port_spec("21;rm -rf /"));
        assert!(valid_cidr("1.2.3.4"));
        assert!(valid_cidr("10.0.0.0/8"));
        assert!(!valid_cidr("10.0.0.0/33"));
        assert!(valid_backend("systemd"));
        assert!(!valid_backend("../../x"));
    }

    #[test]
    fn parse_nft_rule_lines() {
        let text = "\
tcp dport 21 accept comment \"skiff-100\" # handle 3
ip saddr 10.0.0.0/8 drop comment \"host-1\" # handle 4
udp dport 53 drop comment \"skiff-101\" # handle 5
";
        let rules = parse_nft_rules(text);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["id"], "skiff-100");
        assert_eq!(rules[0]["handle"], 3);
        assert_eq!(rules[0]["port"], "21");
        assert_eq!(rules[1]["action"], "drop");
        assert_eq!(rules[1]["protocol"], "udp");
    }

    #[test]
    fn parse_iptables_rules_and_index() {
        let text = "\
-N SKIFF
-A SKIFF -p tcp -m tcp --dport 21 -j ACCEPT -m comment --comment skiff-1
-A SKIFF -s 10.0.0.5/32 -j DROP -m comment --comment skiff-2
";
        let rules = parse_iptables_rules(text);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["index"], 1);
        assert_eq!(rules[0]["port"], "21");
        assert_eq!(rules[1]["action"], "drop");
        assert_eq!(find_iptables_index(text, "skiff-2"), Some(2));
    }

    #[test]
    fn parse_fail2ban_output() {
        let status = "Status for the jail: sshd\n|- Filter\n`- Actions\n   `- Banned IP list:\t1.2.3.4 5.6.7.8\n";
        assert_eq!(parse_banned(status), vec!["1.2.3.4", "5.6.7.8"]);
        let list = "Status\n|- Number of jail:\t2\n`- Jail list:\tsshd, pure-ftpd\n";
        assert_eq!(parse_jail_list(list), vec!["sshd", "pure-ftpd"]);
    }

    #[test]
    fn rule_requires_port_or_source() {
        assert!(RuleSpec::parse(&json!({})).is_err());
        assert!(RuleSpec::parse(&json!({"port": 80})).is_err()); // 缺协议
        let spec = RuleSpec::parse(&json!({"protocol": "tcp", "port": 80})).unwrap();
        assert_eq!(spec.port, Some(80));
        let spec = RuleSpec::parse(&json!({"source": "10.0.0.0/8", "action": "drop"})).unwrap();
        assert_eq!(spec.action, "drop");
    }

    #[tokio::test]
    async fn fail2ban_status_reports_missing_with_reason() {
        let host = FakeHost::new(|_cmd, _args| CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "not found".into(),
        });
        let res = dispatch(&host, "fail2ban.status", &json!({}))
            .await
            .unwrap();
        assert_eq!(res["installed"], false);
        assert_eq!(res["running"], false);
        assert_eq!(res["state"], "missing");
        assert!(res["jails"].as_array().unwrap().is_empty());
        assert!(res["reason"].as_str().unwrap().contains("未安装"));
    }

    #[tokio::test]
    async fn fail2ban_status_reports_stopped_with_systemd_reason() {
        let host = FakeHost::new(|cmd, args| {
            if cmd.contains("fail2ban-client")
                && args.first().map(|s| s == "--version").unwrap_or(false)
            {
                return ok("Fail2Ban v1.0.2");
            }
            if cmd.contains("systemctl") {
                return CommandOutput {
                    success: false,
                    stdout: "failed\n".into(),
                    stderr: String::new(),
                };
            }
            ok("")
        });
        let res = dispatch(&host, "fail2ban.status", &json!({}))
            .await
            .unwrap();
        assert_eq!(res["installed"], true);
        assert_eq!(res["running"], false);
        assert_eq!(res["state"], "stopped");
        assert!(res["reason"].as_str().unwrap().contains("failed"));
        // 服务未运行时不调用 fail2ban-client status，避免把连接错误当成「没有 jail」。
        let calls = host.calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|c| c.ends_with(" status")));
    }

    #[tokio::test]
    async fn fail2ban_start_requires_installed() {
        let host = FakeHost::new(|_cmd, _args| CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        });
        let err = dispatch(&host, "fail2ban.start", &json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(err.message.contains("未安装"));
    }

    /// 只把 `enabled = true` 的段算作「预期启用」；带注释/引号/大小写都要能读。
    #[test]
    fn enabled_jails_reads_enabled_sections_only() {
        let text = "\
[sshd]
enabled = false

[pure-ftpd]
enabled = true
port = 21

[skip-me]
enabled = no
";
        assert_eq!(enabled_jails(text), vec!["pure-ftpd"]);
        assert!(has_section(text, "sshd"));
        assert!(!has_section(text, "nginx"));
    }

    /// 装完必须显式 restart；复核发现 sshd 仍在运行时 stop，且不重复追加已有关闭段。
    #[tokio::test]
    async fn fail2ban_install_restarts_and_disables_unexpected_jail() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let installed = Arc::new(AtomicBool::new(false));
        let sshd = Arc::new(AtomicBool::new(true));
        let (flag, sshd_flag) = (installed.clone(), sshd.clone());
        let host = FakeHost::new(move |cmd, args| {
            if cmd.contains("fail2ban-client") {
                match args.first().map(String::as_str) {
                    Some("--version") => {
                        return if flag.load(Ordering::SeqCst) {
                            ok("Fail2Ban v1.0.2")
                        } else {
                            CommandOutput {
                                success: false,
                                stdout: String::new(),
                                stderr: "not found".into(),
                            }
                        };
                    }
                    Some("stop") => {
                        sshd_flag.store(false, Ordering::SeqCst);
                        return ok("");
                    }
                    _ => {
                        return if sshd_flag.load(Ordering::SeqCst) {
                            ok("Status\n|- Number of jail:\t1\n`- Jail list:\tsshd\n")
                        } else {
                            ok("Status\n|- Number of jail:\t0\n`- Jail list:\t\n")
                        };
                    }
                }
            }
            if cmd.contains("apt-get") {
                flag.store(true, Ordering::SeqCst);
                return ok("");
            }
            if cmd.contains("systemctl") {
                return ok("active\n");
            }
            if cmd == LS {
                return ok("defaults-debian.conf\nskiff-defaults.conf\n");
            }
            if cmd == CAT {
                return ok("[sshd]\nenabled = false\n");
            }
            ok("")
        });

        let res = dispatch(&host, "fail2ban.install", &json!({}))
            .await
            .unwrap();
        assert_eq!(res["disabled_jails"], json!(["sshd"]));
        assert!(res["jails"].as_array().unwrap().is_empty());

        let calls = host.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.contains("enable fail2ban")));
        let restarts = calls
            .iter()
            .filter(|c| c.contains("restart fail2ban"))
            .count();
        assert!(restarts >= 2, "写入配置后必须重启两次并复核：{calls:?}");
        assert!(calls.iter().any(|c| c.ends_with("stop sshd")));
        // 默认配置里已有 [sshd] 段，不应再追加一份重复段。
        assert!(
            !calls.iter().any(|c| c.contains("tee -a")),
            "已有 [sshd] 段时不应重复追加：{calls:?}"
        );
    }

    /// 启动同样要 restart（enable --now 对已运行服务是 no-op），缺失的关闭段要补写。
    #[tokio::test]
    async fn fail2ban_start_restarts_and_appends_missing_disable_section() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let sshd = Arc::new(AtomicBool::new(true));
        let sshd_flag = sshd.clone();
        let host = FakeHost::new(move |cmd, args| {
            if cmd.contains("fail2ban-client") {
                match args.first().map(String::as_str) {
                    Some("--version") => return ok("Fail2Ban v1.0.2"),
                    Some("stop") => {
                        sshd_flag.store(false, Ordering::SeqCst);
                        return ok("");
                    }
                    _ => {
                        return if sshd_flag.load(Ordering::SeqCst) {
                            ok("Status\n|- Number of jail:\t1\n`- Jail list:\tsshd\n")
                        } else {
                            ok("Status\n|- Number of jail:\t0\n`- Jail list:\t\n")
                        };
                    }
                }
            }
            if cmd.contains("systemctl") {
                return ok("active\n");
            }
            if cmd == LS {
                return ok("skiff-defaults.conf\n");
            }
            if cmd == CAT {
                return ok("# skiff managed defaults\n");
            }
            ok("")
        });

        let res = dispatch(&host, "fail2ban.start", &json!({})).await.unwrap();
        assert_eq!(res["disabled_jails"], json!(["sshd"]));
        let calls = host.calls.lock().unwrap().clone();
        let restarts = calls
            .iter()
            .filter(|c| c.contains("restart fail2ban"))
            .count();
        assert_eq!(restarts, 2, "停用后要再重启一次：{calls:?}");
        assert!(calls
            .iter()
            .any(|c| c.contains("tee -a /etc/fail2ban/jail.d/skiff-defaults.conf")));
    }

    /// 宿主覆盖文件（如 `zz-sshd.conf`）排序晚于托管配置时，重启后 sshd 会回来：
    /// 返回体必须反映复核后的真实状态，不能 `disabled_jails` 与 `jails` 同时含它。
    #[tokio::test]
    async fn fail2ban_reports_residual_jail_when_host_override_wins() {
        let host = FakeHost::new(|cmd, args| {
            if cmd.contains("fail2ban-client") {
                return match args.first().map(String::as_str) {
                    Some("--version") => ok("Fail2Ban v1.0.2"),
                    Some("stop") => ok(""),
                    // 每次重启后 sshd 都因 zz-sshd.conf 重新启用。
                    _ => ok("Status\n|- Number of jail:\t1\n`- Jail list:\tsshd\n"),
                };
            }
            if cmd.contains("systemctl") {
                return ok("active\n");
            }
            if cmd == LS {
                return ok("skiff-defaults.conf\nzz-sshd.conf\n");
            }
            if cmd == CAT {
                return ok("# skiff managed defaults\n");
            }
            ok("")
        });

        let res = dispatch(&host, "fail2ban.start", &json!({})).await.unwrap();
        assert_eq!(
            res["jails"],
            json!(["sshd"]),
            "jails 必须是复核后的真实集合"
        );
        assert!(
            res.get("disabled_jails").is_none(),
            "仍在运行的 jail 不能登记为已停用：{res}"
        );
        assert_eq!(res["residual_jails"], json!(["sshd"]));
        let calls = host.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.ends_with("stop sshd")), "{calls:?}");
    }

    #[tokio::test]
    async fn fail2ban_start_leaves_host_jails_alone_without_managed_defaults() {
        let host = FakeHost::new(|cmd, args| {
            if cmd.contains("fail2ban-client") {
                return match args.first().map(String::as_str) {
                    Some("--version") => ok("Fail2Ban v1.0.2"),
                    _ => ok("Status\n|- Number of jail:\t1\n`- Jail list:\tsshd\n"),
                };
            }
            if cmd.contains("systemctl") {
                return ok("active\n");
            }
            if cmd == CAT {
                return CommandOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "No such file or directory".into(),
                };
            }
            ok("")
        });
        let res = dispatch(&host, "fail2ban.start", &json!({})).await.unwrap();
        assert_eq!(res["jails"], json!(["sshd"]));
        assert!(res.get("disabled_jails").is_none());
        let calls = host.calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|c| c.contains("stop ")), "{calls:?}");
        assert!(
            calls.iter().any(|c| c.contains("restart fail2ban")),
            "启动必须重启：{calls:?}"
        );
    }

    #[tokio::test]
    async fn fail2ban_uninstall_purges_and_reports_missing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let installed = Arc::new(AtomicBool::new(true));
        let flag = installed.clone();
        let host = FakeHost::new(move |cmd, args| {
            if cmd.contains("fail2ban-client")
                && args.first().map(|s| s == "--version").unwrap_or(false)
            {
                return if flag.load(Ordering::SeqCst) {
                    ok("Fail2Ban v1.0.2")
                } else {
                    CommandOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: "not found".into(),
                    }
                };
            }
            if cmd.contains("apt-get") && args.first().map(|s| s == "purge").unwrap_or(false) {
                flag.store(false, Ordering::SeqCst);
            }
            ok("")
        });
        let res = dispatch(&host, "fail2ban.uninstall", &json!({}))
            .await
            .unwrap();
        assert_eq!(res["state"], "missing");
        let calls = host.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.contains("disable --now fail2ban")));
        assert!(calls
            .iter()
            .any(|c| c.contains("rm -f /etc/fail2ban/jail.d/skiff-defaults.conf")));
        assert!(calls.iter().any(|c| c.contains("purge -y fail2ban")));
    }

    #[tokio::test]
    async fn port_allow_builds_one_rule_per_protocol() {
        let host = FakeHost::new(|cmd, args| {
            if args.iter().any(|a| a == "--version") {
                return ok(if cmd.contains("nft") {
                    "nftables 1.0"
                } else {
                    ""
                });
            }
            ok("")
        });
        let res = dispatch(
            &host,
            "port.allow",
            &json!({"port": 21, "protocol": "both"}),
        )
        .await
        .unwrap();
        assert_eq!(res["added"].as_array().unwrap().len(), 2);
        let calls = host.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|c| c.contains("tcp dport 21 accept")));
        assert!(calls.iter().any(|c| c.contains("udp dport 21 accept")));
    }
}
