//! `com.skiff.firewall`：宿主机防火墙与 fail2ban 管理。
//!
//! **纯插件实现**（零内核 firewall 宿主命令编排）：所有 nftables/iptables 规则、
//! fail2ban 全套经内核 exec 能力面提交，任务中心自动有运行记录。

use std::sync::{Mutex, OnceLock};

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

/// 进程级缓存：防火墙后端探测结果。
///
/// on-demand 模式下插件进程长驻，这些值在单次进程生命周期内不会变
/// （装/卸 nftables、fail2ban 都是管理员显式操作，且装好后插件需要重启才生效），
/// 所以用 `OnceLock` 缓存，首次探测后直接复用。
///
/// 前端每次打开页面会并发调 `status` + `rules.list`，不缓存的话单次刷新 ≈ 10+ 条
/// exec 进作业中心，而 `exec` 能力面对同一插件只允许一条活跃作业（target_id 唯一），
/// 并发的会被 409 拒——这正是「作业中心一堆失败」和「页面慢」的共同根因。
#[derive(Copy, Clone)]
struct BackendProbe {
    nftables: bool,
    iptables: bool,
    selected: Option<Backend>,
}
fn backend_probe(p: &Plugin) -> BackendProbe {
    static CACHE: OnceLock<BackendProbe> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let nft = command_works(p, NFT, &["--version"]);
        let ipt = command_works(p, IPTABLES, &["--version"]);
        let selected = if nft {
            Some(Backend::Nft)
        } else if ipt {
            Some(Backend::Iptables)
        } else {
            None
        };
        BackendProbe {
            nftables: nft,
            iptables: ipt,
            selected,
        }
    })
}
#[derive(Clone, Copy)]
struct F2bProbe {
    installed: bool,
}

/// fail2ban 安装探测缓存。
///
/// **与后端探测不同，这份缓存必须可失效**：装/卸 fail2ban 正是本插件自己的动作，
/// 而 on-demand 插件进程长驻（`idle_timeout_sec = 300`），若像后端那样只缓存一次，
/// 面板在装完之后会继续显示「未安装」，直到进程重启。2026-11-26 上机实测到该现象
/// （安装前后同一进程内 `installed` 恒为 false，`dpkg -l` 已是 ii）。
static F2B_PROBE_CACHE: Mutex<Option<F2bProbe>> = Mutex::new(None);

fn f2b_probe(p: &Plugin) -> F2bProbe {
    let mut guard = F2B_PROBE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = *guard {
        return v;
    }
    let probe = F2bProbe {
        installed: command_works(p, DPKG_QUERY, &["-s", "fail2ban"])
            || command_works(p, FAIL2BAN_CLIENT, &["--version"]),
    };
    *guard = Some(probe);
    probe
}

/// 安装/卸载 fail2ban 后调用，让下一次探测重新落盘。
fn f2b_probe_invalidate() {
    *F2B_PROBE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ── exec 辅助 ──
//
// 两条通道分工（`docs/contract/KERNEL_EXEC_CONTRACT.md` §2）：
// - **轻量**（`host_exec_light`）：只读探测 / 版本查询。不建作业、不抢同插件唯一
//   活跃槽位，一页十几条探测可以并发跑；
// - **作业化**（`host_exec_full`）：有副作用、可能耗时（改规则、装包、重启服务），
//   过程输出与取消都在任务中心。

/// 只读探测：结果只看 `success`，不抛错（探测失败=不可用）。
fn light_probe(p: &Plugin, binary: &str, argv: &[&str]) -> bool {
    p.host_exec_light(binary, argv, Some(10))
        .map(|o| o.get("success").and_then(Value::as_bool) == Some(true))
        .unwrap_or(false)
}

/// 只读查询：成功返回 stdout，非零退出转成 `RpcError`（与 `exec_ok` 同口径）。
fn light_ok(p: &Plugin, binary: &str, argv: &[&str], t: u64) -> Result<String, RpcError> {
    let out = p
        .host_exec_light(binary, argv, Some(t))
        .map_err(common::rpc_err)?;
    if out.get("success").and_then(Value::as_bool) != Some(true) {
        let code = out.get("code").and_then(Value::as_i64).unwrap_or(-1);
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

/// 只读查询（不判成败）：`systemctl is-active` 这类「非零退出也是有效答案」的用。
fn light_any(p: &Plugin, binary: &str, argv: &[&str], t: u64) -> Result<String, RpcError> {
    let out = p
        .host_exec_light(binary, argv, Some(t))
        .map_err(common::rpc_err)?;
    Ok(out
        .get("stdout")
        .and_then(Value::as_str)
        .or_else(|| out.get("stderr").and_then(Value::as_str))
        .unwrap_or("")
        .to_string())
}

/// 作业化执行，原样交出 `(是否成功, 退出码, 合并输出)`。
///
/// 存在的理由：有些命令**既要看退出码、又要匹配输出文本**（`apt-get install` 失败时
/// 要从输出里认 `Unable to locate package` 才能决定要不要 update 后重试）。以前这种
/// 场合只能调 `exec_any`，于是退出码被丢掉——命令失败了照样往下跑，最后报出来的错
/// 指向别处（或干脆报成功）。2026-11-26 上机就踩到过这类「假成功」。
fn exec_raw(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    t: u64,
) -> Result<(bool, i64, String), RpcError> {
    let out = p
        .host_exec_full(binary, argv, stdin, Some(t))
        .map_err(common::rpc_err)?;
    let ok = out.get("success").and_then(Value::as_bool) == Some(true);
    let code = out.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
    let text = out
        .get("stdout")
        .and_then(Value::as_str)
        .or_else(|| out.get("stderr").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    Ok((ok, code, text))
}

/// 作业化执行：非零退出转成 `RpcError`，成功返回输出。
fn exec_ok(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    t: u64,
) -> Result<String, RpcError> {
    let (ok, code, text) = exec_raw(p, binary, argv, stdin, t)?;
    if !ok {
        return Err(RpcError::internal(format!(
            "{binary} 退出码 {code}：{}",
            if text.trim().is_empty() {
                "未知"
            } else {
                text.trim()
            }
        )));
    }
    Ok(text)
}

/// 作业化执行（不判成败）：`rm -f`、`systemctl stop` 这类「本来就允许失败」的用。
fn exec_any(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    t: u64,
) -> Result<String, RpcError> {
    Ok(exec_raw(p, binary, argv, stdin, t)?.2)
}
fn command_works(p: &Plugin, binary: &str, argv: &[&str]) -> bool {
    light_probe(p, binary, argv)
}
fn detect_backend(p: &Plugin) -> Option<Backend> {
    backend_probe(p).selected
}
fn fail2ban_installed(p: &Plugin) -> bool {
    f2b_probe(p).installed
}
/// 受管链读取：成功返回原始输出，失败（常见是全新机器上表/链还不存在）返回 None。
fn nft_chain_dump(p: &Plugin) -> Option<String> {
    light_ok(
        p,
        NFT,
        &["-a", "list", "chain", "inet", "skiff", "input"],
        10,
    )
    .ok()
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
    let jumped = light_probe(p, IPTABLES, &["-C", "INPUT", "-j", IPT_CHAIN]);
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
    light_any(p, SYSTEMCTL, &["is-active", F2B_SERVICE], 10)
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

/// 接管 fail2ban 后要关掉的**发行版默认 jail**。
///
/// Debian 的 `jail.d/defaults-debian.conf` 默认 `[sshd] enabled = true`：面板装完
/// fail2ban 就会开始封禁宿主的 SSH 来源，这不是管理员点「安装」时预期的事。契约
/// §3 FW-04 要求装完 `fail2ban-client status` 不含 sshd，靠的就是这份托管默认值。
const MANAGED_DEFAULTS_JAILS: &[&str] = &["sshd"];

/// `skiff-defaults.conf` 正文：只关默认 jail，不碰宿主自己的 `jail.local` 等文件。
fn managed_defaults_conf() -> String {
    let mut out = String::from(
        "# 由 Skiff 托管：只启用 skiff-<jail>.conf 里声明的 jail。\n\
         # 关闭发行版默认启用的 sshd jail，避免面板接管 fail2ban 后开始封禁宿主 SSH 来源。\n",
    );
    for jail in MANAGED_DEFAULTS_JAILS {
        out.push_str(&format!("[{jail}]\nenabled = false\n\n"));
    }
    out
}

fn managed_defaults_path() -> String {
    format!("{JAIL_DIR}/{JAIL_PREFIX}defaults.conf")
}

/// 解析 `fail2ban-client status` 的 `Jail list:\tsshd, skiff-ftp` 行。
fn jail_list_from_status(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        // 两种排版都要吃：`|- Jail list:\tsshd` 与 `` `- Jail list:\tsshd, skiff-ftp ``
        let t = line.trim().trim_start_matches(['`', '|', '-', ' ', '\t']);
        let Some(rest) = t.strip_prefix("Jail list:") else {
            continue;
        };
        for name in rest.split(',') {
            let name = name.trim();
            if !name.is_empty() && !out.iter().any(|x: &String| x == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// 复核结果：请求关掉、但实际仍在跑的 jail（宿主 `jail.d` 排序更晚的覆盖会赢）。
fn residual_jails(running: &[String], disabled: &[&str]) -> Vec<String> {
    running
        .iter()
        .filter(|r| disabled.iter().any(|d| d.eq_ignore_ascii_case(r)))
        .cloned()
        .collect()
}

/// 重启后取 `fail2ban-client status`：socket 未就绪时重试几次。
///
/// 台账有一条长期观察——全新安装后紧接着调 `fail2ban-client` 会连不上
/// `/var/run/fail2ban/fail2ban.sock`（首次重启后命中，1–2 秒后即正常）。这里做有限退避，
/// 拿不到就返回最后一次输出，由调用方按「复核失败」处理，不阻塞页面。
fn f2b_status_text(p: &Plugin) -> String {
    let mut last = String::new();
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
        if let Ok(text) = light_any(p, FAIL2BAN_CLIENT, &["status"], 30) {
            if text.contains("Jail list:") {
                return text;
            }
            last = text;
        }
    }
    last
}

// ── 15 条 handler（独立函数，match 只调） ──
fn h_status(p: &Plugin) -> Result<Value, RpcError> {
    // backend_probe 一次性拿到 nftables/iptables 的可用性与选中的后端，
    // 且结果缓存进程内复用——避免每次 status 都跑 4 条 exec 探测。
    let probe = backend_probe(p);
    let backend = probe.selected;
    let rules = match backend {
        Some(Backend::Nft) => light_ok(
            p,
            NFT,
            &["-a", "list", "chain", "inet", "skiff", "input"],
            10,
        )
        .ok()
        .map(|t| parse_nft_rules(&t))
        .unwrap_or_default(),
        Some(Backend::Iptables) => light_ok(p, IPTABLES, &["-S", IPT_CHAIN], 10)
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
        "backend": backend.map(|b| match b { Backend::Nft => "nftables", Backend::Iptables => "iptables" }),
        "backend_text": backend.map(|b| match b { Backend::Nft => "nftables", Backend::Iptables => "iptables" }).unwrap_or("未检测到（需要 nft 或 iptables）"),
        "nftables": probe.nftables,
        "nftables_text": if probe.nftables { "已安装" } else { "未安装" },
        "iptables": probe.iptables,
        "iptables_text": if probe.iptables { "已安装" } else { "未安装" },
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
            let out = light_ok(
                p,
                NFT,
                &["-a", "list", "chain", "inet", "skiff", "input"],
                10,
            )?;
            Ok(json!({ "backend": "nftables", "items": parse_nft_rules(&out) }))
        }
        Some(Backend::Iptables) => {
            ensure_iptables(p)?;
            let out = light_ok(p, IPTABLES, &["-S", IPT_CHAIN], 10)?;
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
    // 三态由 RPC 直接给出（面板 `ui/app.js` 优先读 `state`，缺失时才按 installed/running 折算）。
    let state = if !installed {
        "missing"
    } else if running {
        "running"
    } else {
        "stopped"
    };
    let reason = match state {
        "missing" => "未安装 fail2ban",
        "stopped" => "fail2ban 已安装但服务未运行",
        _ => "",
    };
    Ok(json!({
        "installed": installed,
        "running": running,
        "state": state,
        "reason": reason,
        "jails": jails,
    }))
}

fn h_f2b_install(p: &Plugin) -> Result<Value, RpcError> {
    if fail2ban_installed(p) {
        return Ok(json!({ "installed": true, "already": true }));
    }
    // 用 exec_raw 而不是 exec_any：失败时必须能认出来。老写法丢掉退出码，apt 失败后
    // 照样往下走，最后报的是 `systemctl enable` 的错（或什么都没报）。
    let (ok, code, out) = exec_raw(
        p,
        APT_GET,
        &["install", "-y", "--no-install-recommends", "fail2ban"],
        None,
        900,
    )?;
    if !ok {
        // 源里找不到包时先 update 再重试一次（离线/旧索引的常见情形）。
        if !out.contains("Unable to locate package") {
            return Err(RpcError::internal(format!(
                "apt-get install fail2ban 退出码 {code}：{}",
                out.trim()
            )));
        }
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
    // 先落托管默认值再启服务：否则 Debian 自带的 sshd jail 会先跑起来再被关掉。
    write_managed_file(p, &managed_defaults_path(), &managed_defaults_conf())?;
    exec_ok(p, SYSTEMCTL, &["enable", F2B_SERVICE], None, 300)?;
    // enable 只改开机自启，配置生效要显式 restart（契约 §3 FW-04）。
    restart_fail2ban(p)?;
    f2b_probe_invalidate();
    let jails = jail_list_from_status(&f2b_status_text(p));
    Ok(json!({
        "installed": true,
        "defaults_disabled": MANAGED_DEFAULTS_JAILS,
        "jails": jails,
        "residual_jails": residual_jails(&jails, MANAGED_DEFAULTS_JAILS),
    }))
}

fn h_f2b_start(p: &Plugin) -> Result<Value, RpcError> {
    if !fail2ban_installed(p) {
        return Err(RpcError::internal(
            "fail2ban 尚未安装，请先安装（fail2ban.install）",
        ));
    }
    // enable 保证开机自启；restart 让 reload 过的 jail 配置真正生效，随后复核实际 jail。
    exec_ok(p, SYSTEMCTL, &["enable", F2B_SERVICE], None, 300)?;
    restart_fail2ban(p)?;
    let jails = jail_list_from_status(&f2b_status_text(p));
    let residual = residual_jails(&jails, MANAGED_DEFAULTS_JAILS);
    Ok(json!({
        "running": true,
        "jails": jails,
        "residual_jails": residual,
    }))
}

fn h_f2b_stop(p: &Plugin) -> Result<Value, RpcError> {
    exec_ok(p, SYSTEMCTL, &["stop", F2B_SERVICE], None, 300)?;
    Ok(json!({ "running": false }))
}

fn h_f2b_uninstall(p: &Plugin) -> Result<Value, RpcError> {
    let _ = exec_any(p, SYSTEMCTL, &["stop", F2B_SERVICE], None, 900);
    let _ = exec_any(p, SYSTEMCTL, &["disable", F2B_SERVICE], None, 900);
    // purge 而不是 remove：`remove` 会把 `/etc/fail2ban` 留在宿主上（dpkg 状态 rc），
    // 下次装上时又带着旧配置回来。托管文件随包一起清掉，宿主自己的 jail 不受影响。
    //
    // 这一步**必须看退出码**：purge 失败（dpkg 锁被占、依赖破裂）时若吞掉错误直接返回
    // `removed: true`，面板会显示已卸载而包其实还在——包不在了才允许往下清理残留。
    exec_ok(p, APT_GET, &["purge", "-y", "fail2ban"], None, 900)?;
    let _ = exec_any(p, RM, &["-f", &managed_defaults_path()], None, 30);
    // dpkg 只在目录为空时删目录，而 purge 跑的时候托管文件还在 → `/etc/fail2ban/jail.d`
    // 会被留下（2026-11-26 上机实测：`/etc/fail2ban` 只剩空 `jail.d/`）。`rm -d` 只删**空**
    // 目录，宿主自己的 jail 文件还在就报错跳过——不留空壳，也不误删宿主文件。
    let _ = exec_any(p, RM, &["-df", JAIL_DIR, "/etc/fail2ban"], None, 30);
    f2b_probe_invalidate();
    Ok(json!({ "state": "missing", "removed": true }))
}
/// jail / filter 名：只允许 fail2ban 认得的字符集，顺带挡掉路径分隔符与换行
/// （名字会拼进 `jail.d/skiff-<jail>.conf` 路径与 ini 段名）。
fn valid_f2b_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 受管 jail 文件正文。
///
/// `filter` 必须落盘：fail2ban 缺省按 **jail 名**去找 `filter.d/<jail>.conf`，
/// 名字与 filter 不同时不写 `filter =` 行就起不来（2026-11-26 实测：`ftptest` →
/// `Found no accessible config files for 'filter.d/ftptest'`，整个服务 exit 255）。
fn jail_conf(
    jail: &str,
    filter: &str,
    port: &str,
    backend: &str,
    limits: (u64, u64, u64),
    logpath: Option<&str>,
) -> String {
    let (maxretry, findtime, bantime) = limits;
    let mut out = format!(
        "[{jail}]\nenabled = true\nfilter = {filter}\nport = {port}\nbackend = {backend}\n\
         maxretry = {maxretry}\nfindtime = {findtime}\nbantime = {bantime}\n"
    );
    if let Some(lp) = logpath {
        out.push_str(&format!("logpath = {lp}\n"));
    }
    out
}

/// `fail2ban-client -t`：在**重启服务之前**验证整份配置能不能被加载。
///
/// 这一步是本轮上机踩出来的：jail 写了 `backend = auto` 而对应日志文件不存在时，
/// fail2ban 不是「跳过这个 jail」而是**整个服务 exit 255**
/// （`Have not found any log file for <jail> jail`）。若直接 restart，服务会先躺下
/// 再回滚重启；预检能让服务全程保持在运行状态。退出码可靠（失败 255、成功 0），
/// 同时保留 stderr 原文给调用方看。
fn f2b_config_test(p: &Plugin) -> (bool, String) {
    match exec_raw(p, FAIL2BAN_CLIENT, &["-t"], None, 60) {
        Ok((ok, _, text)) => (ok, text.trim().to_string()),
        Err(e) => (false, e.message),
    }
}

/// filter 定义是否存在。Debian 把随包 filter 装进 `/etc/fail2ban/filter.d`，
/// 上游/其它发行版也可能放 `/usr/share/fail2ban/filter.d`，两处都认。
fn filter_exists(p: &Plugin, filter: &str) -> bool {
    ["/etc/fail2ban/filter.d", "/usr/share/fail2ban/filter.d"]
        .iter()
        .any(|dir| light_probe(p, LS, &[&format!("{dir}/{filter}.conf")]))
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
    // 缺省回落到 jail 名：与 fail2ban 的默认查找规则一致，老调用方（只传 jail）行为不变。
    let filter = params
        .get("filter")
        .and_then(Value::as_str)
        .unwrap_or(&jail)
        .to_string();
    let maxretry = u64_param(params, "maxretry", 5);
    let findtime = u64_param(params, "findtime", 600);
    let bantime = u64_param(params, "bantime", 3600);
    // `logpath` 是契约里声明过的可选参数，之前从未被读取（写了等于没写）。
    // `backend = auto` / `file` 时它是 fail2ban 找日志的唯一依据，缺了就是整服务起不来。
    let logpath = params
        .get("logpath")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if !valid_f2b_name(&jail) {
        return Err(RpcError::invalid_params(
            "jail 名只能包含字母、数字、点、下划线、连字符（1–64 字符）",
        ));
    }
    if !valid_f2b_name(&filter) {
        return Err(RpcError::invalid_params(
            "filter 名只能包含字母、数字、点、下划线、连字符（1–64 字符）",
        ));
    }
    if !fail2ban_installed(p) {
        return Err(RpcError::internal("fail2ban 尚未安装"));
    }
    // 先校验 filter 存在，再落盘。缺 filter 时 fail2ban 会**整个服务起不来**
    // （不是只跳过这个 jail），而错误现场只有 systemctl 的 stderr——提前挡住。
    if !filter_exists(p, &filter) {
        return Err(RpcError::internal(format!(
            "找不到 filter `{filter}`（应存在 /etc/fail2ban/filter.d/{filter}.conf 或 \
             /usr/share/fail2ban/filter.d/{filter}.conf）；jail 名与 filter 名不同时必须显式传 filter"
        )));
    }
    if let Some(lp) = logpath.as_deref() {
        if !light_probe(p, LS, &[lp]) {
            return Err(RpcError::internal(format!(
                "logpath `{lp}` 不存在；fail2ban 找不到日志文件会让整个服务起不来，\
                 请确认服务日志路径，或改用 backend = systemd"
            )));
        }
    }
    let content = jail_conf(
        &jail,
        &filter,
        &port,
        &backend,
        (maxretry, findtime, bantime),
        logpath.as_deref(),
    );
    let _ = exec_ok(p, "/usr/bin/mkdir", &["-p", JAIL_DIR], None, 900);
    let path = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    write_managed_file(p, &path, &content)?;
    // 落盘后先预检，再重启：坏配置当场回滚，服务全程不躺。
    let (tested, why) = f2b_config_test(p);
    if !tested {
        // 区分责任：删掉本文件再测一次——仍失败说明宿主既有 jail 配置本身有问题，
        // 那样就不该把这个锅算在本次调用头上（也不去动服务）。
        let _ = exec_any(p, RM, &["-f", &path], None, 30);
        let (still_bad, why2) = f2b_config_test(p);
        return Err(RpcError::internal(if still_bad {
            format!(
                "移除本次写入的 {path} 后配置预检仍失败，宿主既有 jail 配置有问题：{}",
                if why2.is_empty() { why } else { why2 }
            )
        } else {
            format!("jail `{jail}` 的配置未通过预检，已撤回 {path}：{why}")
        }));
    }
    // 预检通过后重启并复核：预检只保证「能被加载」，不保证这个 jail 在运行集合里
    // （宿主 `jail.d` 里更晚的文件可能把它改成 enabled = false）。
    if let Err(e) = restart_fail2ban(p) {
        rollback_jail(p, &path);
        return Err(RpcError::internal(format!(
            "jail `{jail}` 重启 fail2ban 失败，已回滚 {path}：{}",
            e.message
        )));
    }
    let jails = jail_list_from_status(&f2b_status_text(p));
    if !jails.iter().any(|j| j == &jail) {
        rollback_jail(p, &path);
        return Err(RpcError::internal(format!(
            "jail `{jail}` 写入后未出现在 fail2ban 的 jail 列表（当前：{}），已回滚 {path}——\
             常见原因是宿主 `jail.d` 里有排序更晚的文件把它关掉了",
            if jails.is_empty() {
                "无".to_string()
            } else {
                jails.join(", ")
            }
        )));
    }
    Ok(json!({
        "ensured": true,
        "jail": jail,
        "filter": filter,
        "logpath": logpath,
        "path": path,
        "jails": jails,
    }))
}

/// 删掉刚写的受管 jail 文件并尽力重启，让服务回到写入前的状态。
fn rollback_jail(p: &Plugin, path: &str) {
    let _ = exec_any(p, RM, &["-f", path], None, 30);
    let _ = restart_fail2ban(p);
}

fn h_f2b_jail_remove(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !valid_f2b_name(&jail) {
        return Err(RpcError::invalid_params(
            "jail 名只能包含字母、数字、点、下划线、连字符（1–64 字符）",
        ));
    }
    let path = format!("{JAIL_DIR}/{JAIL_PREFIX}{jail}.conf");
    // `rm -f` 对不存在的文件返回 0，所以这里可以判退出码：删不掉（权限/只读挂载）
    // 就必须报错，否则下面 restart 完 jail 还在，调用方却收到 removed: true。
    exec_ok(p, RM, &["-f", &path], None, 900)?;
    if fail2ban_installed(p) {
        restart_fail2ban(p)?;
        let jails = jail_list_from_status(&f2b_status_text(p));
        if jails.iter().any(|j| j == &jail) {
            return Err(RpcError::internal(format!(
                "jail `{jail}` 的文件已删除，但重启后仍在运行（当前：{}）——\
                 可能是宿主自己的 jail.d 配置又启用了它",
                jails.join(", ")
            )));
        }
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
    // 只读查询走 `light_ok`：它会检查退出码。以前用 `exec_any`，服务停摆时
    // `fail2ban-client` 返回非零、输出为空，面板就显示「0 个封禁」而不是报错。
    let out = light_ok(p, FAIL2BAN_CLIENT, &["get", &jail, "banned"], 30)?;
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

    #[test]
    fn jail_list_parses_both_status_layouts() {
        let one = "Status\n|- Number of jail:\t1\n`- Jail list:\tsshd\n";
        assert_eq!(jail_list_from_status(one), vec!["sshd".to_string()]);

        let two = "Status\n|- Number of jail:\t2\n`- Jail list:\tsshd, skiff-ftp\n";
        assert_eq!(
            jail_list_from_status(two),
            vec!["sshd".to_string(), "skiff-ftp".to_string()]
        );

        assert!(jail_list_from_status("Status\n|- Number of jail:\t0\n").is_empty());
    }

    #[test]
    fn managed_defaults_disable_distro_jails_only() {
        let conf = managed_defaults_conf();
        assert!(conf.contains("[sshd]"), "必须声明 sshd 段: {conf}");
        assert!(
            conf.contains("enabled = false"),
            "必须是关闭而不是启用: {conf}"
        );
        // 这份文件不许被自己的 enabled_jails 解析成「启用」，否则等于没关。
        assert!(
            enabled_jails(&conf).is_empty(),
            "托管默认值里不应有 enabled=true: {:?}",
            enabled_jails(&conf)
        );
        assert_eq!(
            managed_defaults_path(),
            "/etc/fail2ban/jail.d/skiff-defaults.conf"
        );
    }

    #[test]
    fn residual_jails_only_reports_requested_but_still_running() {
        let running = vec![
            "sshd".to_string(),
            "skiff-ftp".to_string(),
            "pure-ftpd".to_string(),
        ];
        assert_eq!(
            residual_jails(&running, MANAGED_DEFAULTS_JAILS),
            vec!["sshd".to_string()]
        );
        assert!(residual_jails(&[], MANAGED_DEFAULTS_JAILS).is_empty());
        assert!(residual_jails(&["skiff-ftp".to_string()], MANAGED_DEFAULTS_JAILS).is_empty());
    }

    #[test]
    fn jail_conf_always_declares_filter() {
        // filter 与 jail 同名也必须显式写：fail2ban 的默认规则是「按 jail 名找 filter」，
        // 一旦有人改了 jail 名却沿用旧 filter，缺这一行就是整个服务起不来。
        let conf = jail_conf("ftp", "pure-ftpd", "21", "auto", (5, 600, 3600), None);
        assert!(conf.contains("[ftp]"), "{conf}");
        assert!(
            conf.contains("filter = pure-ftpd"),
            "必须落盘 filter: {conf}"
        );
        assert!(conf.contains("enabled = true"), "{conf}");
        assert!(conf.contains("port = 21"), "{conf}");
        assert!(!conf.contains("logpath"), "没传就不写: {conf}");

        // 同名时也写出来，不依赖 fail2ban 的隐式回退。
        let same = jail_conf("pure-ftpd", "pure-ftpd", "ftp", "auto", (3, 60, 120), None);
        assert!(same.contains("filter = pure-ftpd"), "{same}");

        // logpath 传了必须落盘：backend=auto 时它是 fail2ban 唯一的日志依据。
        let with_log = jail_conf(
            "pure-ftpd",
            "pure-ftpd",
            "21",
            "auto",
            (5, 600, 3600),
            Some("/var/log/pure-ftpd/transfer.log"),
        );
        assert!(
            with_log.contains("logpath = /var/log/pure-ftpd/transfer.log"),
            "{with_log}"
        );

        // 写出来的受管文件必须能被自己的 enabled_jails 认成「启用」。
        assert_eq!(enabled_jails(&conf), vec!["ftp".to_string()]);
    }

    #[test]
    fn f2b_names_reject_path_separators() {
        for ok in ["ftp", "pure-ftpd", "skiff.ftp", "a_b"] {
            assert!(valid_f2b_name(ok), "{ok} 应合法");
        }
        for bad in [
            "",
            "../x",
            "a/b",
            "a b",
            "a\nb",
            "[x]",
            "a".repeat(65).as_str(),
        ] {
            assert!(!valid_f2b_name(bad), "{bad:?} 应被拒");
        }
    }
}
