//! `com.skiff.firewall`：宿主机防火墙与 fail2ban 管理。
//!
//! **纯插件实现**（零内核 firewall 宿主命令编排）：所有 nftables/iptables 规则、
//! fail2ban 全套经内核 exec 能力面提交，任务中心自动有运行记录。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

// ── 常量 ──
const NFT: &str = "/usr/sbin/nft";
const IPTABLES: &str = "/usr/sbin/iptables";
const FAIL2BAN_CLIENT: &str = "/usr/bin/fail2ban-client";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const TEE: &str = "/usr/bin/tee";
const RM: &str = "/usr/bin/rm";
const APT_GET: &str = "/usr/bin/apt-get";
const LS: &str = "/bin/ls";
const CAT: &str = "/bin/cat";
const DPKG_QUERY: &str = "/usr/bin/dpkg-query";
const IPT_CHAIN: &str = "SKIFF";
const JAIL_DIR: &str = "/etc/fail2ban/jail.d";
const JAIL_PREFIX: &str = "skiff-";
const F2B_SERVICE: &str = "fail2ban";

#[derive(Copy, Clone, Debug)]
enum Backend {
    Nft,
    Iptables,
}

// ── exec 辅助 ──
fn exec_ok(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    t: u64,
) -> Result<String, RpcError> {
    let out = p
        .host_exec_full(binary, argv, stdin, Some(t))
        .map_err(common::rpc_err)?;
    if out.get("success").and_then(Value::as_bool) != Some(true) {
        let code = out.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
        let detail = out
            .get("stderr")
            .and_then(Value::as_str)
            .or_else(|| out.get("stdout").and_then(Value::as_str))
            .unwrap_or("未知")
            .trim()
            .to_string();
        return Err(RpcError::internal(format!(
            "{binary} 退出码 {code}：{detail}"
        )));
    }
    Ok(out
        .get("stdout")
        .and_then(Value::as_str)
        .or_else(|| out.get("stderr").and_then(Value::as_str))
        .unwrap_or("")
        .to_string())
}
fn exec_any(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    t: u64,
) -> Result<String, RpcError> {
    let out = p
        .host_exec_full(binary, argv, stdin, Some(t))
        .map_err(common::rpc_err)?;
    Ok(out
        .get("stdout")
        .and_then(Value::as_str)
        .or_else(|| out.get("stderr").and_then(Value::as_str))
        .unwrap_or("")
        .to_string())
}
fn command_works(p: &Plugin, binary: &str, argv: &[&str]) -> bool {
    p.host_exec_full(binary, argv, None, Some(30))
        .map(|o| o.get("success").and_then(Value::as_bool) == Some(true))
        .unwrap_or(false)
}
fn detect_backend(p: &Plugin) -> Option<Backend> {
    if command_works(p, NFT, &["--version"]) {
        return Some(Backend::Nft);
    }
    if command_works(p, IPTABLES, &["--version"]) {
        return Some(Backend::Iptables);
    }
    None
}
fn fail2ban_installed(p: &Plugin) -> bool {
    command_works(p, DPKG_QUERY, &["-s", "fail2ban"])
        || command_works(p, FAIL2BAN_CLIENT, &["--version"])
}
/// 受管链读取：成功返回原始输出，失败（常见是全新机器上表/链还不存在）返回 None。
fn nft_chain_dump(p: &Plugin) -> Option<String> {
    p.host_exec_full(
        NFT,
        &["-a", "list", "chain", "inet", "skiff", "input"],
        None,
        Some(30),
    )
    .ok()
    .filter(|o| o.get("success").and_then(Value::as_bool) == Some(true))
    .and_then(|o| o.get("stdout").and_then(Value::as_str).map(str::to_string))
}

/// 按需建受管表/链（契约 §4：表/链不存在时创建，默认策略 accept）。
///
/// 先探测再创建，避免每次列表都跑两条必然失败的 `add`；nft 的 `add table` /
/// `add chain` 在已存在时报「File exists」，这里只在探测不到时才执行，创建后再复核一次。
fn ensure_nft(p: &Plugin) -> Result<(), RpcError> {
    if nft_chain_dump(p).is_some() {
        return Ok(());
    }
    let _ = p.host_exec_full(NFT, &["add", "table", "inet", "skiff"], None, Some(30));
    // 带 hook 的基链才会真正按规则过滤入站流量；参数逐个传，不拼 shell。
    let _ = p.host_exec_full(
        NFT,
        &[
            "add", "chain", "inet", "skiff", "input", "{", "type", "filter", "hook", "input",
            "priority", "0", ";", "}",
        ],
        None,
        Some(30),
    );
    if nft_chain_dump(p).is_none() {
        return Err(RpcError::internal(
            "创建 nftables 受管表 inet skiff / 链 input 失败",
        ));
    }
    Ok(())
}

/// iptables 受管链按需创建并挂到 INPUT（幂等；已有跳转时不重复插入）。
fn ensure_iptables(p: &Plugin) -> Result<(), RpcError> {
    // `-N` 在链已存在时退出码非 0，属预期，这里不当作失败。
    let _ = p.host_exec_full(IPTABLES, &["-N", IPT_CHAIN], None, Some(30));
    let jumped = p
        .host_exec_full(IPTABLES, &["-C", "INPUT", "-j", IPT_CHAIN], None, Some(30))
        .ok()
        .and_then(|o| o.get("success").and_then(Value::as_bool))
        .unwrap_or(false);
    if !jumped {
        exec_ok(
            p,
            IPTABLES,
            &["-I", "INPUT", "1", "-j", IPT_CHAIN],
            None,
            30,
        )?;
    }
    Ok(())
}

fn ensure_managed_chain(p: &Plugin, backend: Backend) -> Result<(), RpcError> {
    match backend {
        Backend::Nft => ensure_nft(p),
        Backend::Iptables => ensure_iptables(p),
    }
}
fn fail2ban_running(p: &Plugin) -> bool {
    exec_any(p, SYSTEMCTL, &["is-active", F2B_SERVICE], None, 10)
        .unwrap_or_default()
        .trim()
        == "active"
}
fn write_managed_file(p: &Plugin, path: &str, content: &str) -> Result<(), RpcError> {
    exec_ok(p, TEE, &[path], Some(content), 30).map(|_| ())
}
fn restart_fail2ban(p: &Plugin) -> Result<(), RpcError> {
    exec_ok(p, SYSTEMCTL, &["restart", F2B_SERVICE], None, 300).map(|_| ())
}

// ── 纯逻辑 ──
fn enabled_jails(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            cur = Some(t[1..t.len() - 1].to_string());
            continue;
        }
        if let Some(sec) = cur.as_ref() {
            let mut kv = t.splitn(2, '=');
            let Some(k) = kv.next().map(str::trim) else {
                continue;
            };
            let Some(v) = kv.next().map(str::trim) else {
                continue;
            };
            if k.eq_ignore_ascii_case("enabled")
                && matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on")
                && !out.contains(sec)
            {
                out.push(sec.clone());
            }
        }
    }
    out
}

// ── 15 条 handler（独立函数，match 只调） ──
fn h_status(p: &Plugin) -> Result<Value, RpcError> {
    let nft = command_works(p, NFT, &["--version"]);
    let iptables = command_works(p, IPTABLES, &["--version"]);
    let backend = if nft {
        Some("nftables")
    } else if iptables {
        Some("iptables")
    } else {
        None
    };
    let rules = match detect_backend(p) {
        Some(Backend::Nft) => exec_ok(
            p,
            NFT,
            &["-a", "list", "chain", "inet", "skiff", "input"],
            None,
            30,
        )
        .ok()
        .map(|t| parse_nft_rules(&t))
        .unwrap_or_default(),
        Some(Backend::Iptables) => exec_ok(p, IPTABLES, &["-S", IPT_CHAIN], None, 30)
            .ok()
            .map(|t| parse_iptables_rules(&t))
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let f2b_installed = fail2ban_installed(p);
    let f2b_running = fail2ban_running(p);
    let warnings: Vec<&str> = if backend.is_none() {
        vec!["宿主机未找到 nft 或 iptables"]
    } else {
        Vec::new()
    };
    Ok(json!({
        "backend": backend,
        // 面板按 *_text 直接展示；保留原始键给需要判断的调用方。
        "backend_text": backend.unwrap_or("未检测到（需要 nft 或 iptables）"),
        "nftables": nft,
        "nftables_text": if nft { "已安装" } else { "未安装" },
        "iptables": iptables,
        "iptables_text": if iptables { "已安装" } else { "未安装" },
        "managed_rules": rules.len(),
        "fail2ban": json!({ "installed": f2b_installed, "running": f2b_running }),
        "fail2ban_text": match (f2b_installed, f2b_running) {
            (true, true) => "已安装 · 运行中",
            (true, false) => "已安装 · 未运行",
            _ => "未安装",
        },
        "warnings": warnings,
        "warnings_any": !warnings.is_empty(),
        "warnings_text": warnings.join("；"),
    }))
}
/// nft 规则解析：只保留真正的规则行（带 `# handle <n>`），跳过表/链声明与右花括号。
///
/// UI 的列按 `action_text / protocol_text / port_text / source_text / id` 取值，
/// `id` 用 handle 号，删除时按 handle 精确定位，不再 flush 整条链。
fn parse_nft_rules(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let raw = line.trim();
            // nft 会给链本身也打 handle（`chain input { # handle 1`），
            // 它不是规则，混进列表会出现一行全空的可删项。
            if raw.starts_with('}') || raw.contains("chain ") {
                return None;
            }
            let (_, handle) = raw.rsplit_once("# handle ")?;
            let id = handle.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let mut protocol_text = String::new();
            let mut port_text = String::new();
            let mut source_text = String::new();
            let mut action_text = String::from("放行");
            let tokens: Vec<&str> = raw.split_whitespace().collect();
            for (i, tok) in tokens.iter().enumerate() {
                match *tok {
                    "tcp" | "udp" | "icmp" | "icmpv6" => protocol_text = (*tok).to_string(),
                    "dport" => port_text = tokens.get(i + 1).copied().unwrap_or("").to_string(),
                    "saddr" => source_text = tokens.get(i + 1).copied().unwrap_or("").to_string(),
                    "accept" => action_text = "放行".to_string(),
                    "drop" => action_text = "丢弃".to_string(),
                    "reject" => action_text = "拒绝".to_string(),
                    _ => {}
                }
            }
            Some(json!({
                "id": id,
                "source": "nftables",
                "raw": raw,
                "action_text": action_text,
                "protocol_text": protocol_text,
                "port_text": port_text,
                "source_text": source_text
            }))
        })
        .collect()
}

/// iptables `-S` 解析：跳过 `-N/-P` 等结构行，只保留 `-A SKIFF ...` 规则；
/// `id` 用去掉 `-A SKIFF ` 前缀的规则描述，删除时原样拼回 `-D`。
fn parse_iptables_rules(text: &str) -> Vec<Value> {
    let prefix = format!("-A {IPT_CHAIN} ");
    text.lines()
        .filter_map(|line| {
            let raw = line.trim();
            let spec = raw.strip_prefix(prefix.as_str())?;
            let tokens: Vec<&str> = spec.split_whitespace().collect();
            let mut protocol_text = String::new();
            let mut port_text = String::new();
            let mut source_text = String::new();
            let mut action_text = String::new();
            for (i, tok) in tokens.iter().enumerate() {
                match *tok {
                    "-p" => protocol_text = tokens.get(i + 1).copied().unwrap_or("").to_string(),
                    "--dport" => port_text = tokens.get(i + 1).copied().unwrap_or("").to_string(),
                    "-s" => source_text = tokens.get(i + 1).copied().unwrap_or("").to_string(),
                    "-j" => {
                        action_text = match tokens.get(i + 1).copied().unwrap_or("ACCEPT") {
                            "DROP" => "丢弃".to_string(),
                            "REJECT" => "拒绝".to_string(),
                            _ => "放行".to_string(),
                        }
                    }
                    _ => {}
                }
            }
            Some(json!({
                "id": spec,
                "source": "iptables",
                "raw": raw,
                "action_text": action_text,
                "protocol_text": protocol_text,
                "port_text": port_text,
                "source_text": source_text
            }))
        })
        .collect()
}
fn h_rules_list(p: &Plugin) -> Result<Value, RpcError> {
    match detect_backend(p) {
        Some(Backend::Nft) => {
            ensure_nft(p)?;
            let out = exec_ok(
                p,
                NFT,
                &["-a", "list", "chain", "inet", "skiff", "input"],
                None,
                30,
            )?;
            Ok(json!({ "backend": "nftables", "items": parse_nft_rules(&out) }))
        }
        Some(Backend::Iptables) => {
            ensure_iptables(p)?;
            let out = exec_ok(p, IPTABLES, &["-S", IPT_CHAIN], None, 30)?;
            Ok(json!({ "backend": "iptables", "items": parse_iptables_rules(&out) }))
        }
        None => Ok(json!({ "backend": Value::Null, "items": [] })),
    }
}
/// 数值参数既可能来自 JSON 数字，也可能来自 schema 表单的字符串
/// （`input-text` / `number` 的 `${field}` 都可能是字符串）。
fn u64_param(params: &Value, key: &str, default: u64) -> u64 {
    match params.get(key) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse::<u64>().unwrap_or(default),
        _ => default,
    }
}

/// 端口：非法或缺省按 0 处理，由调用方统一报「端口需在 1..65535」。
fn port_param(params: &Value) -> u64 {
    u64_param(params, "port", 0)
}

fn h_rule_add(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let Some(backend) = detect_backend(p) else {
        return Err(RpcError::internal("未找到 nft 或 iptables"));
    };
    ensure_managed_chain(p, backend)?;
    let port = port_param(params);
    if !(1..=65535).contains(&port) {
        return Err(RpcError::internal("端口需在 1..65535"));
    }
    let proto = params
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("accept");
    let source = params
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let act = if matches!(action, "drop" | "reject") {
        action
    } else {
        "accept"
    };
    let port_str = port.to_string();
    let act_upper = act.to_uppercase();
    // 表单的「全部」= TCP + UDP，逐协议下发；nft 不接受 `all dport`。
    let protos: Vec<&str> = if proto == "all" {
        vec!["tcp", "udp"]
    } else {
        vec![proto]
    };
    for proto in &protos {
        match backend {
            Backend::Nft => {
                let mut args: Vec<&str> = vec!["add", "rule", "inet", "skiff", "input"];
                if let Some(src) = source {
                    args.extend(["ip", "saddr", src]);
                }
                args.extend([proto, "dport", &port_str, act]);
                exec_ok(p, NFT, &args, None, 30)?;
            }
            Backend::Iptables => {
                let mut args: Vec<&str> = vec![
                    "-A", IPT_CHAIN, "-p", proto, "--dport", &port_str, "-j", &act_upper,
                ];
                if let Some(src) = source {
                    args.push("-s");
                    args.push(src);
                }
                exec_ok(p, IPTABLES, &args, None, 30)?;
            }
        }
    }
    Ok(json!({ "added": true }))
}
fn h_rule_remove(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let Some(backend) = detect_backend(p) else {
        return Err(RpcError::internal("未找到 nft 或 iptables"));
    };
    ensure_managed_chain(p, backend)?;
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let port = port_param(params);
    let proto = params
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    let port_str = port.to_string();
    match backend {
        // 按 handle 精确删一条；旧接口只带 port 时退化为删除该端口的第一条规则
        // （仍不 flush 整链，避免误删其它规则）。
        Backend::Nft => {
            if id.is_empty() {
                if port == 0 {
                    return Err(RpcError::internal("删除规则缺少 id 或 port"));
                }
                return h_rule_remove_nft_by_port(p, proto, &port_str);
            }
            exec_ok(
                p,
                NFT,
                &["delete", "rule", "inet", "skiff", "input", "handle", &id],
                None,
                30,
            )?;
        }
        Backend::Iptables => {
            if id.is_empty() {
                if port == 0 {
                    return Err(RpcError::internal("删除规则缺少 id 或 port"));
                }
                exec_ok(
                    p,
                    IPTABLES,
                    &[
                        "-D", IPT_CHAIN, "-p", proto, "--dport", &port_str, "-j", "ACCEPT",
                    ],
                    None,
                    30,
                )?;
            } else {
                let mut args: Vec<&str> = vec!["-D", IPT_CHAIN];
                args.extend(id.split_whitespace());
                exec_ok(p, IPTABLES, &args, None, 30)?;
            }
        }
    };
    Ok(json!({ "removed": true }))
}

/// 无 id 时的 nft 退化删除：先读 handle 列表，删掉该端口的第一条。
fn h_rule_remove_nft_by_port(p: &Plugin, proto: &str, port_str: &str) -> Result<Value, RpcError> {
    let out = exec_ok(
        p,
        NFT,
        &["-a", "list", "chain", "inet", "skiff", "input"],
        None,
        30,
    )?;
    let needle = format!("{proto} dport {port_str}");
    let handle = out
        .lines()
        .find(|l| l.contains(needle.as_str()) && l.contains("# handle "))
        .and_then(|l| l.rsplit_once("# handle "))
        .map(|(_, h)| h.trim().to_string())
        .ok_or_else(|| RpcError::internal("未找到对应的 nft 规则"))?;
    exec_ok(
        p,
        NFT,
        &[
            "delete", "rule", "inet", "skiff", "input", "handle", &handle,
        ],
        None,
        30,
    )?;
    Ok(json!({ "removed": true }))
}

fn h_port_allow(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let Some(backend) = detect_backend(p) else {
        return Err(RpcError::internal("未找到 nft 或 iptables"));
    };
    ensure_managed_chain(p, backend)?;
    let port = port_param(params);
    if !(1..=65535).contains(&port) {
        return Err(RpcError::internal("端口需在 1..65535"));
    }
    let proto = params
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    // 表单提供「TCP + UDP」合并项，逐协议各加一条，不能把 both 当协议名传给 nft。
    let protos: Vec<&str> = if proto == "both" {
        vec!["tcp", "udp"]
    } else {
        vec![proto]
    };
    let port_str = port.to_string();
    for proto in &protos {
        match backend {
            Backend::Nft => exec_ok(
                p,
                NFT,
                &[
                    "add", "rule", "inet", "skiff", "input", proto, "dport", &port_str, "accept",
                ],
                None,
                30,
            )?,
            Backend::Iptables => exec_ok(
                p,
                IPTABLES,
                &[
                    "-A", IPT_CHAIN, "-p", proto, "--dport", &port_str, "-j", "ACCEPT",
                ],
                None,
                30,
            )?,
        };
    }
    Ok(json!({ "port": port, "protocol": proto, "allowed": true }))
}
fn h_f2b_status(p: &Plugin) -> Result<Value, RpcError> {
    let installed = fail2ban_installed(p);
    let running = if installed {
        fail2ban_running(p)
    } else {
        false
    };
    let jails = if installed {
        let listing = exec_any(p, LS, &["-1", JAIL_DIR], None, 30).unwrap_or_default();
        let mut out = Vec::new();
        for name in listing.lines().map(str::trim) {
            if !name.starts_with(JAIL_PREFIX) || !name.ends_with(".conf") {
                continue;
            }
            let text =
                exec_any(p, CAT, &[&format!("{JAIL_DIR}/{name}")], None, 30).unwrap_or_default();
            for j in enabled_jails(&text) {
                if !out.contains(&j) {
                    out.push(j);
                }
            }
        }
        out.sort();
        out
    } else {
        Vec::new()
    };
    Ok(json!({ "installed": installed, "running": running, "jails": jails }))
}
fn h_f2b_install(p: &Plugin) -> Result<Value, RpcError> {
    if fail2ban_installed(p) {
        return Ok(json!({ "installed": true, "already": true }));
    }
    let out = exec_any(
        p,
        APT_GET,
        &["install", "-y", "--no-install-recommends", "fail2ban"],
        None,
        900,
    )?;
    if out.contains("Unable to locate package") {
        exec_ok(p, APT_GET, &["update"], None, 900)?;
        exec_ok(
            p,
            APT_GET,
            &["install", "-y", "--no-install-recommends", "fail2ban"],
            None,
            900,
        )?;
    }
    let _ = exec_ok(p, "/usr/bin/mkdir", &["-p", JAIL_DIR], None, 900);
    exec_ok(p, SYSTEMCTL, &["enable", F2B_SERVICE], None, 300)?;
    Ok(json!({ "installed": true }))
}
fn h_f2b_start(p: &Plugin) -> Result<Value, RpcError> {
    exec_ok(p, SYSTEMCTL, &["start", F2B_SERVICE], None, 300)?;
    Ok(json!({ "running": true }))
}
fn h_f2b_stop(p: &Plugin) -> Result<Value, RpcError> {
    exec_ok(p, SYSTEMCTL, &["stop", F2B_SERVICE], None, 300)?;
    Ok(json!({ "running": false }))
}
fn h_f2b_uninstall(p: &Plugin) -> Result<Value, RpcError> {
    let _ = exec_any(p, SYSTEMCTL, &["stop", F2B_SERVICE], None, 900);
    let _ = exec_any(p, SYSTEMCTL, &["disable", F2B_SERVICE], None, 900);
    let _ = exec_any(p, APT_GET, &["remove", "-y", "fail2ban"], None, 900);
    Ok(json!({ "removed": true }))
}
fn h_f2b_jail_ensure(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let backend = params
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    let port = params
        .get("port")
        .and_then(Value::as_str)
        .unwrap_or("ftp")
        .to_string();
    let maxretry = u64_param(params, "maxretry", 5);
    let findtime = u64_param(params, "findtime", 600);
    let bantime = u64_param(params, "bantime", 3600);
    if jail.is_empty() {
        return Err(RpcError::invalid_params("缺少 jail 名称"));
    }
    if !fail2ban_installed(p) {
        return Err(RpcError::internal("fail2ban 尚未安装"));
    }
    let content = format!("[{jail}]\nenabled = true\nport = {port}\nbackend = {backend}\nmaxretry = {maxretry}\nfindtime = {findtime}\nbantime = {bantime}\n");
    let _ = exec_ok(p, "/usr/bin/mkdir", &["-p", JAIL_DIR], None, 900);
    let path = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    write_managed_file(p, &path, &content)?;
    restart_fail2ban(p)?;
    Ok(json!({ "ensured": true, "jail": jail, "path": path }))
}
fn h_f2b_jail_remove(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if jail.is_empty() {
        return Err(RpcError::invalid_params("缺少 jail 名称"));
    }
    let path = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    let _ = exec_any(p, RM, &["-f", &path], None, 900);
    if fail2ban_installed(p) {
        restart_fail2ban(p)?;
    }
    Ok(json!({ "removed": true, "jail": jail }))
}
fn h_f2b_banned(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if jail.is_empty() {
        return Err(RpcError::invalid_params("缺少 jail 名称"));
    }
    if !fail2ban_installed(p) {
        return Ok(json!({ "banned": [], "available": false }));
    }
    let out = exec_any(p, FAIL2BAN_CLIENT, &["get", &jail, "banned"], None, 30)?;
    let mut ips = Vec::new();
    for line in out.lines() {
        for tok in line.split([' ', ',', '\t']) {
            if tok.chars().filter(|c| *c == '.').count() == 3 {
                ips.push(tok.to_string());
            }
        }
    }
    Ok(json!({ "jail": jail, "banned": ips }))
}
fn h_f2b_ban(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ip = params
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if jail.is_empty() || ip.is_empty() {
        return Err(RpcError::invalid_params("jail 和 ip 都不能为空"));
    }
    if !fail2ban_installed(p) {
        return Err(RpcError::internal("fail2ban 尚未安装"));
    }
    exec_ok(p, FAIL2BAN_CLIENT, &["set", &jail, "banip", &ip], None, 30)?;
    Ok(json!({ "banned": true, "jail": jail, "ip": ip }))
}
fn h_f2b_unban(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let ip = params
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if jail.is_empty() || ip.is_empty() {
        return Err(RpcError::invalid_params("jail 和 ip 都不能为空"));
    }
    if !fail2ban_installed(p) {
        return Err(RpcError::internal("fail2ban 尚未安装"));
    }
    exec_ok(
        p,
        FAIL2BAN_CLIENT,
        &["set", &jail, "unbanip", &ip],
        None,
        30,
    )?;
    Ok(json!({ "unbanned": true, "jail": jail, "ip": ip }))
}

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let handler_plugin = plugin.clone();
    if let Err(e) = plugin.serve(move |method, params| {
        let p = &handler_plugin;
        match method {
            "status" => h_status(p),
            "rules.list" => h_rules_list(p),
            "rule.add" => h_rule_add(p, &params),
            "rule.remove" => h_rule_remove(p, &params),
            "port.allow" => h_port_allow(p, &params),
            "fail2ban.status" => h_f2b_status(p),
            "fail2ban.install" => h_f2b_install(p),
            "fail2ban.start" => h_f2b_start(p),
            "fail2ban.stop" => h_f2b_stop(p),
            "fail2ban.uninstall" => h_f2b_uninstall(p),
            "fail2ban.jail.ensure" => h_f2b_jail_ensure(p, &params),
            "fail2ban.jail.remove" => h_f2b_jail_remove(p, &params),
            "fail2ban.banned" => h_f2b_banned(p, &params),
            "fail2ban.ban" => h_f2b_ban(p, &params),
            "fail2ban.unban" => h_f2b_unban(p, &params),
            "banned" => Ok(json!({"available": false, "banned": []})),
            "config" => Ok(Value::Object(
                common::load_config(p)
                    .into_iter()
                    .map(|(k, v)| (k, Value::from(v)))
                    .collect(),
            )),
            other => Err(RpcError::method_not_found(other)),
        }
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

#[allow(dead_code)]
fn _normalize_params(_method: &str, params: Value) -> Value {
    let Some(mut obj) = params.as_object().cloned() else {
        return params;
    };
    obj.retain(|_, v| !v.as_str().is_some_and(|s| s.trim().is_empty()));
    obj.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nft_rules_skip_structure_and_expose_columns() {
        let text = "table inet skiff {\n\tchain input { # handle 1\n\t\ttype filter hook input priority filter; policy accept;\n\t\ttcp dport 18081 accept # handle 2\n\t\tip saddr 10.0.0.0/8 tcp dport 22 drop # handle 7\n\t}\n}";
        let items = parse_nft_rules(text);
        assert_eq!(items.len(), 2, "链自身的 handle 不能当规则");
        assert_eq!(items[0]["id"], "2");
        assert_eq!(items[0]["action_text"], "放行");
        assert_eq!(items[0]["protocol_text"], "tcp");
        assert_eq!(items[0]["port_text"], "18081");
        assert_eq!(items[1]["id"], "7");
        assert_eq!(items[1]["action_text"], "丢弃");
        assert_eq!(items[1]["source_text"], "10.0.0.0/8");
    }

    #[test]
    fn port_param_accepts_number_and_form_string() {
        assert_eq!(port_param(&json!({ "port": 8080 })), 8080);
        assert_eq!(port_param(&json!({ "port": " 8080 " })), 8080);
        assert_eq!(port_param(&json!({ "port": "" })), 0);
        assert_eq!(port_param(&json!({})), 0);
        assert_eq!(u64_param(&json!({ "maxretry": "7" }), "maxretry", 5), 7);
    }

    #[test]
    fn iptables_rules_keep_only_append_lines() {
        let text = "-N SKIFF\n-P SKIFF ACCEPT\n-A SKIFF -p tcp --dport 8080 -j ACCEPT\n-A SKIFF -p udp --dport 53 -j DROP\n";
        let items = parse_iptables_rules(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "-p tcp --dport 8080 -j ACCEPT");
        assert_eq!(items[0]["protocol_text"], "tcp");
        assert_eq!(items[0]["port_text"], "8080");
        assert_eq!(items[0]["action_text"], "放行");
        assert_eq!(items[1]["action_text"], "丢弃");
    }
}
