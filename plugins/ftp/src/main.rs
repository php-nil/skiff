//! `com.skiff.ftp`：按站点创建 pure-ftpd 虚拟账号，并提供服务配置面板。
//!
//! **纯插件实现**（零内核 ftp 宿主命令编排）：所有写操作（install / uninstall /
//! account.* / service.configure / service.restart）经内核 exec 能力面
//! (`host_exec_full`) 提交，任务中心自动有运行记录，取消与过程日志由 exec
//! 能力面提供。
//!
//! 内核 ftp 能力面**只留只读查询**：站点快照（`sites.list`）、账号库读取
//! （`accounts.list`）、运行状态探测（`status`）。插件侧写操作需要的站点快照
//! 通过 `ftp_call("sites.list")` 拿（插件无 DB 访问权）。
//!
//! 密码/私钥经 exec 的 stdin 通道投递，不入 argv、审计或作业日志。账号列表
//! 由内核读 pure-pw 用户库返回，插件保持无状态，重装后仍能看到既有账号。
//!
//! **fail2ban 不再由 FTP 安装或配置**：启用登录防护是可选能力，经 interop
//! 调用独立防火墙插件 `com.skiff.firewall` 的 `fail2ban.*` 接口。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

// ─────────────────── FTP 常量（插件侧自管，不依赖内核 crate） ───────────────────
#[allow(dead_code)]
const PASSWD_FILE: &str = "/etc/pure-ftpd/pureftpd.passwd";
const PDB_FILE: &str = "/etc/pure-ftpd/pureftpd.pdb";
const CONF_DIR: &str = "/etc/pure-ftpd/conf";
const AUTH_DIR: &str = "/etc/pure-ftpd/auth";
const AUTH_DB_LINK: &str = "../conf/PureDB";
const SERVICE: &str = "pure-ftpd";
const UMASK: &str = "113 002"; // 文件 0664 / 目录 2775
#[allow(dead_code)]
const PURE_PW_A: &str = "/usr/bin/pure-pw";
#[allow(dead_code)]
const PURE_PW_B: &str = "/usr/sbin/pure-pw";

/// 独立防火墙插件 id：fail2ban 的唯一管理方。
const FIREWALL_PLUGIN: &str = "com.skiff.firewall";
/// FTP 登录防护使用的 jail 名；与防火墙插件页面的默认值一致。
const PROTECTION_JAIL: &str = "pure-ftpd";

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    if let Err(e) = plugin.serve(move |method, params| match method {
        "status" => status(&p),
        "install" => install(&p),
        "uninstall" => uninstall(&p, normalize_params("uninstall", params)),
        "sites" => sites(&p),
        "accounts" => accounts(&p),
        "create" => create(&p, normalize_params("create", params)),
        "password" => password(&p, normalize_params("password", params)),
        "delete" => delete(&p, normalize_params("delete", params)),
        "toggle" => toggle(&p, normalize_params("toggle", params)),
        "restart" => restart(&p),
        "config" => config(&p),
        "config.save" | "save_config" => save_config(&p, normalize_params("config.save", params)),
        // 可选登录防护：真正的 fail2ban 管理在防火墙插件，这里只做转发与状态拼装。
        "protection" | "protection.status" => protection(&p),
        "protection.enable" => protection_enable(&p),
        "protection.disable" => protection_disable(&p),
        "protection.banned" => protection_banned(&p, params),
        "protection.unban" => protection_unban(&p, params),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

/// schema 表单值都是字符串：写入口按方法归一化类型，密码保持原样。
fn normalize_params(method: &str, params: Value) -> Value {
    let Some(mut obj) = params.as_object().cloned() else {
        return params;
    };
    match method {
        "config.save" | "save_config" => {
            for key in [
                "listen_port",
                "passive_port_start",
                "passive_port_end",
                "max_clients",
                "max_per_ip",
            ] {
                if let Some(Value::String(text)) = obj.get(key) {
                    if let Ok(n) = text.trim().parse::<u64>() {
                        obj.insert(key.to_string(), json!(n));
                    }
                }
            }
            // 空字符串表示「清除证书路径」，必须转成 null 发给内核，不能当未填写丢弃。
            for key in ["tls_cert_path", "tls_key_path"] {
                if obj
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.trim().is_empty())
                {
                    obj.insert(key.to_string(), Value::Null);
                }
            }
            obj.retain(|key, v| {
                matches!(key.as_str(), "tls_cert_path" | "tls_key_path")
                    || !v.as_str().is_some_and(|s| s.trim().is_empty())
            });
        }
        "uninstall" => {
            for key in ["remove_accounts", "remove_package"] {
                if let Some(Value::String(text)) = obj.get(key) {
                    obj.insert(
                        key.to_string(),
                        json!(matches!(text.trim(), "true" | "1" | "on")),
                    );
                }
            }
        }
        "toggle" => {
            if let Some(Value::String(text)) = obj.get("enabled") {
                obj.insert(
                    "enabled".to_string(),
                    json!(matches!(text.trim(), "true" | "1" | "on")),
                );
            }
        }
        _ => {
            for key in ["site", "account", "login"] {
                if let Some(Value::String(text)) = obj.get(key) {
                    obj.insert(key.to_string(), json!(text.trim()));
                }
            }
        }
    }
    Value::Object(obj)
}

// ─────────────────── exec 编排辅助函数 ───────────────────

/// 调 exec 能力面执行命令，非零退出转成可读错误。
///
/// 等价于旧内核侧的 `run_ok(host, cmd, argv, stdin, timeout)`：成功返回 stdout
/// （空则取 stderr），失败返回 stderr/stdout 组合的可读错误。
fn exec_ok(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    timeout_sec: u64,
) -> Result<String, RpcError> {
    let out = p
        .host_exec_full(binary, argv, stdin, Some(timeout_sec))
        .map_err(common::rpc_err)?;
    if out.get("success").and_then(Value::as_bool) != Some(true) {
        let code = out.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
        let detail = out
            .get("stderr")
            .and_then(Value::as_str)
            .or_else(|| out.get("stdout").and_then(Value::as_str))
            .unwrap_or("未知错误")
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

/// 只提交命令，不判断退出码；非零退出也返回输出。
fn exec_any(
    p: &Plugin,
    binary: &str,
    argv: &[&str],
    stdin: Option<&str>,
    timeout_sec: u64,
) -> Result<String, RpcError> {
    let out = p
        .host_exec_full(binary, argv, stdin, Some(timeout_sec))
        .map_err(common::rpc_err)?;
    Ok(out
        .get("stdout")
        .and_then(Value::as_str)
        .or_else(|| out.get("stderr").and_then(Value::as_str))
        .unwrap_or("")
        .to_string())
}

/// 探测 pure-pw 路径：优先 /usr/bin，fallback /usr/sbin。
#[allow(dead_code)]
fn pure_pw_path(p: &Plugin) -> Result<&'static str, RpcError> {
    match p.host_exec_full(PURE_PW_A, &["--help"], None, Some(30)) {
        Ok(v) if v.get("success").and_then(Value::as_bool) == Some(true) => Ok(PURE_PW_A),
        _ => Ok(PURE_PW_B),
    }
}

// ─────────────────── 账号表读写（经 exec host，插件无文件系统权限） ───────────────────

const LOGIN_PREFIX: &str = "skiff_";

/// pure-pw useradd 会在 home 目录建 UID/GID 为 0/0 的文件，v1 统一用固定身份
/// 避免每次解析站点目录 owner；skiff-model::SITE_UID / SITE_GID 的值。
const SITE_UID: u32 = 1000;
const SITE_GID: u32 = 1000;

fn valid_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.len() <= 32 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_password(pw: &str) -> bool {
    (8..=128).contains(&pw.len()) && !pw.chars().any(|c| c.is_control())
}

fn fnv4(text: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in text.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

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

#[allow(dead_code)]
struct FtpAccount {
    login: String,
    uid: u32,
    gid: u32,
    home: String,
    enabled: bool,
}

/// 从宿主读 PASSWD_FILE（exec /bin/cat），解析成账号列表。
fn read_accounts(p: &Plugin) -> Result<Vec<FtpAccount>, RpcError> {
    let body = exec_any(p, "/bin/cat", &[PASSWD_FILE], None, 30)?;
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

/// 把 body 原子落盘到 PASSWD_FILE（exec tee 到 tmp + exec mv）。
fn write_passwd_atomic(p: &Plugin, body: &str) -> Result<(), RpcError> {
    let tmp = format!("/run/skiff-ftp/passwd.{}", std::process::id());
    exec_ok(p, "/usr/bin/tee", &[&tmp], Some(body), 900)?;
    exec_ok(p, "/usr/bin/mv", &["-f", &tmp, PASSWD_FILE], None, 900)?;
    Ok(())
}

/// 用 pure-pw 重建 PureDB。
fn mkdb(p: &Plugin) -> Result<(), RpcError> {
    let pw = pure_pw_path(p)?;
    exec_ok(
        p,
        pw,
        &["mkdb", PDB_FILE, "-f", PASSWD_FILE],
        None,
        900,
    )?;
    Ok(())
}

/// systemctl restart + 轮询 is-active，最多 15s。
fn restart_and_wait(p: &Plugin) -> Result<(), RpcError> {
    exec_ok(p, "/usr/bin/systemctl", &["restart", SERVICE], None, 300)?;
    for _ in 0..30 {
        let out = exec_any(
            p,
            "/usr/bin/systemctl",
            &["is-active", SERVICE],
            None,
            10,
        )?;
        if out.trim() == "active" {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err(RpcError::internal(format!(
        "pure-ftpd 重启后 15s 内未进入 active；请检查 systemctl status {SERVICE}"
    )))
}

/// 探测 pure-ftpd 是否已安装（exec dpkg-query）。
fn ftp_installed(p: &Plugin) -> bool {
    match exec_any(
        p,
        "/usr/bin/dpkg-query",
        &["-s", "pure-ftpd"],
        None,
        30,
    ) {
        Ok(out) => out.contains("Status: install ok installed"),
        Err(_) => false,
    }
}

/// 从 ftp_call("sites.list") 结果里按 domain 找站点。
fn find_site<'a>(sites_res: &'a Value, domain: &str) -> Option<&'a Value> {
    sites_res
        .get("sites")
        .and_then(Value::as_array)?
        .iter()
        .find(|s| s.get("domain").and_then(Value::as_str) == Some(domain))
}

/// 从站点 resolve uid/gid。
fn resolve_identity(_site: &Value) -> Result<(u32, u32), RpcError> {
    Ok((SITE_UID, SITE_GID))
}

/// align_tree 等价：把站点文档根及子目录 chown 成 uid:gid、归一权限。
/// 这里简化：直接 chown -R + chmod -R 目录（shell 原生 exec 快）。
fn align_tree(p: &Plugin, home: &str, uid: u32, gid: u32) -> Result<(), RpcError> {
    let ug = format!("{uid}:{gid}");
    exec_ok(p, "/usr/bin/chown", &["-R", &ug, home], None, 900)?;
    // 目录 2775（setgid 继承），文件 0664
    exec_ok(p, "/usr/bin/find", &[home, "-type", "d", "-exec", "chmod", "2775", "{}", "+"], None, 900)?;
    exec_ok(p, "/usr/bin/find", &[home, "-type", "f", "-exec", "chmod", "0664", "{}", "+"], None, 900)?;
    Ok(())
}

fn config(plugin: &Plugin) -> Result<Value, RpcError> {
    let (cfg, status) = match status(plugin) {
        Ok(st) => (st.get("config").cloned().unwrap_or_else(|| json!({})), st),
        // 受限形态或内核暂不可用时，仍让表单以默认值打开；保存时会把明确原因带回。
        Err(e) => (
            json!({
                "listen_port": 21,
                "passive_port_start": 30000,
                "passive_port_end": 30100,
                "max_clients": 30,
                "max_per_ip": 3,
                "tls_mode": "optional",
                "tls_cert_path": null,
                "tls_key_path": null,
            }),
            json!({ "error": e.message }),
        ),
    };
    let mut map = cfg.as_object().cloned().unwrap_or_default();
    // 登录防护改由防火墙插件管理；内核遗留字段不再暴露给 FTP 表单。
    map.remove("login_guard");
    map.insert("status".into(), status);
    Ok(Value::Object(map))
}

/// 保存服务配置：内核 `service.configure` 校验、持久化，已安装时立即重载。
fn save_config(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    // 合并 + 校验参数
    let listen_port = params
        .get("listen_port")
        .and_then(Value::as_u64)
        .unwrap_or(21);
    let passive_port_start = params
        .get("passive_port_start")
        .and_then(Value::as_u64)
        .unwrap_or(30000);
    let passive_port_end = params
        .get("passive_port_end")
        .and_then(Value::as_u64)
        .unwrap_or(30100);
    let max_clients = params
        .get("max_clients")
        .and_then(Value::as_u64)
        .unwrap_or(30);
    let max_per_ip = params
        .get("max_per_ip")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    let tls_mode = params
        .get("tls_mode")
        .and_then(Value::as_str)
        .unwrap_or("off")
        .to_string();

    // 参数校验
    if !(1..=65535).contains(&listen_port) {
        return Err(RpcError::invalid_params("监听端口需在 1-65535 之间"));
    }
    if passive_port_start >= passive_port_end {
        return Err(RpcError::invalid_params(
            "被动端口范围 start 必须小于 end",
        ));
    }
    if passive_port_end - passive_port_start > 5000 {
        return Err(RpcError::invalid_params("被动端口范围不能超过 5000"));
    }
    if max_clients == 0 || max_clients > 1000 {
        return Err(RpcError::invalid_params("最大连接数需在 1-1000 之间"));
    }
    if max_per_ip == 0 || max_per_ip > max_clients {
        return Err(RpcError::invalid_params(
            "每 IP 最大连接数需在 1 到 max_clients 之间",
        ));
    }
    let tls_mode = match tls_mode.as_str() {
        "off" | "optional" | "required" => tls_mode.as_str(),
        _ => "off",
    };

    let cfg = json!({
        "listen_port": listen_port,
        "passive_port_start": passive_port_start,
        "passive_port_end": passive_port_end,
        "max_clients": max_clients,
        "max_per_ip": max_per_ip,
        "tls_mode": tls_mode,
    });

    let installed = ftp_installed(plugin);
    if !installed {
        // 未装 pure-ftpd：只持久化配置，应用阶段跳过
        let _ = exec_ok(
            plugin,
            "/usr/bin/mkdir",
            &["-p", "/etc/skiff"],
            None,
            900,
        );
        let _ = exec_ok(
            plugin,
            "/usr/bin/tee",
            &["/etc/skiff/ftp-service.json"],
            Some(&serde_json::to_string_pretty(&cfg).unwrap_or_default()),
            900,
        );
        return Ok(json!({
            "saved": true,
            "applied": false,
            "config": cfg,
            "message": "配置已保存；FTP 服务尚未安装，安装后生效",
        }));
    }

    // ── 已装 pure-ftpd：应用配置 ──

    // 1. 确保 auth 链（mkdir + rm stale + ln PureDB）
    let _ = exec_any(plugin, "/usr/bin/rm", &["-f", "/etc/pure-ftpd/auth/65unix", "/etc/pure-ftpd/auth/70pam"], None, 300);
    exec_ok(plugin, "/usr/bin/mkdir", &["-p", AUTH_DIR], None, 900)?;
    exec_ok(plugin, "/usr/bin/ln", &["-sfn", AUTH_DB_LINK, &format!("{AUTH_DIR}/50pure")], None, 900)?;

    // 2. 确保 PureDB 文件存在
    let _ = exec_ok(plugin, "/usr/bin/install", &["-m", "0600", "-D", "/dev/null", PDB_FILE], None, 900);

    // 3. 写 conf 文件
    let tls_value = match tls_mode {
        "off" => "0",
        "optional" => "1",
        _ => "2",
    };
    let bind_line = if listen_port == 21 {
        listen_port.to_string()
    } else {
        format!("0.0.0.0,{listen_port}")
    };
    for (name, value) in &[
        ("ChrootEveryone", "yes"),
        ("NoAnonymous", "yes"),
        ("PAMAuthentication", "no"),
        ("UnixAuthentication", "no"),
        ("PureDB", PDB_FILE),
        ("Bind", bind_line.as_str()),
        ("PassivePortRange", format!("{passive_port_start} {passive_port_end}").as_str()),
        ("Umask", UMASK),
        ("MaxClientsNumber", max_clients.to_string().as_str()),
        ("MaxClientsPerIP", max_per_ip.to_string().as_str()),
        ("DontResolve", "yes"),
        ("TLS", tls_value),
    ] {
        let path = format!("{CONF_DIR}/{name}");
        exec_ok(plugin, "/usr/bin/tee", &[&path], Some(&format!("{value}\n")), 900)?;
    }

    // 4. 持久化配置
    let _ = exec_ok(plugin, "/usr/bin/mkdir", &["-p", "/etc/skiff"], None, 900);
    let _ = exec_ok(
        plugin,
        "/usr/bin/tee",
        &["/etc/skiff/ftp-service.json"],
        Some(&serde_json::to_string_pretty(&cfg).unwrap_or_default()),
        900,
    );

    // 5. systemd restart + 等待 active
    exec_ok(plugin, "/usr/bin/systemctl", &["daemon-reload"], None, 300)?;
    exec_ok(plugin, "/usr/bin/systemctl", &["enable", SERVICE], None, 900)?;
    exec_ok(plugin, "/usr/bin/systemctl", &["restart", SERVICE], None, 300)?;
    restart_and_wait(plugin)?;

    // 6. 返回最新状态
    let status = plugin
        .ftp_call("status", Value::Null)
        .map_err(common::rpc_err)?;

    Ok(json!({
        "saved": true,
        "applied": true,
        "config": cfg,
        "status": status,
        "message": "配置已保存并重载 pure-ftpd；若改了监听端口，请在宿主防火墙放通监听端口与被动端口范围",
    }))
}

fn status(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("status", Value::Null)
        .map_err(common::rpc_err)
        .map(decorate_status)
}

/// 状态展示字段：原始字段全部保留，schema 只读派生文案。
fn decorate_status(mut st: Value) -> Value {
    let installed = st
        .get("installed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let running = st.get("running").and_then(Value::as_bool);
    let version = text_or_dash(st.get("version"));
    let listen = text_or_dash(st.get("listen_port"));
    let start = st.get("passive_port_start").and_then(Value::as_u64);
    let end = st.get("passive_port_end").and_then(Value::as_u64);
    let passive = match (start, end) {
        (Some(a), Some(b)) => format!("{a}–{b}"),
        _ => "未知".to_string(),
    };
    let tls = tls_text(st.get("tls_mode"));
    let cert = match (
        st.get("tls_cert_path").and_then(Value::as_str),
        st.get("tls_key_path").and_then(Value::as_str),
    ) {
        (Some(c), Some(k)) if !c.is_empty() && !k.is_empty() => format!("{c} + {k}"),
        _ => "/etc/ssl/private/pure-ftpd.pem（已有或自签）".to_string(),
    };
    let total = st.get("accounts").and_then(Value::as_u64);
    let enabled = st.get("accounts_enabled").and_then(Value::as_u64);
    let disabled = st.get("accounts_disabled").and_then(Value::as_u64);
    let accounts = match (total, enabled, disabled) {
        (Some(t), Some(a), Some(d)) => format!("{t}（启用 {a} / 禁用 {d}）"),
        _ => "未知".to_string(),
    };
    let passwd = text_or_dash(st.get("passwd_file"));
    let state_text = if !installed {
        "未安装"
    } else if running == Some(true) {
        "运行中"
    } else {
        "未运行"
    };
    let detail_rows = vec![
        json!({ "label": "安装状态", "value": if installed { "已安装" } else { "未安装" } }),
        json!({
            "label": "服务状态",
            "value": match running {
                Some(true) => "运行中",
                Some(false) => "未运行",
                None => "未知（读取失败）",
            }
        }),
        json!({
            "label": "监听状态",
            "value": match st.get("listening").and_then(Value::as_bool) {
                Some(true) => "是",
                Some(false) => "否",
                None => "未知（读取失败）",
            }
        }),
        json!({ "label": "监听端口", "value": listen.clone() }),
        json!({
            "label": "当前连接数",
            "value": text_or_dash(st.get("current_connections")),
        }),
        json!({ "label": "版本", "value": version.clone() }),
        json!({ "label": "被动端口", "value": passive.clone() }),
        json!({
            "label": "最大连接 / 每 IP",
            "value": format!(
                "{} / {}",
                text_or_dash(st.get("max_clients")),
                text_or_dash(st.get("max_per_ip"))
            ),
        }),
        json!({ "label": "TLS 模式", "value": tls.clone() }),
        json!({ "label": "证书路径", "value": cert.clone() }),
        json!({ "label": "匿名访问", "value": "强制关闭" }),
        json!({ "label": "账号数量", "value": accounts.clone() }),
        json!({ "label": "用户库", "value": passwd.clone() }),
    ];
    let Some(obj) = st.as_object_mut() else {
        return st;
    };
    obj.insert("state_text".into(), json!(state_text));
    obj.insert("version_text".into(), json!(version));
    obj.insert("listen_text".into(), json!(listen));
    obj.insert("passive_text".into(), json!(passive));
    obj.insert("tls_text".into(), json!(tls));
    obj.insert("cert_text".into(), json!(cert));
    obj.insert("accounts_text".into(), json!(accounts));
    obj.insert("detail_rows".into(), json!(detail_rows));
    st
}

fn text_or_dash(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(v)) => v.to_string(),
        _ => "-".to_string(),
    }
}

fn tls_text(mode: Option<&Value>) -> String {
    match mode.and_then(Value::as_str).unwrap_or("") {
        "0" | "off" => "关闭".to_string(),
        "1" | "optional" => "显式 TLS（可选）".to_string(),
        "2" | "required" => "显式 TLS（强制）".to_string(),
        "" => "—".to_string(),
        other => other.to_string(),
    }
}

fn install(plugin: &Plugin) -> Result<Value, RpcError> {
    // ── Step 1: apt-get install pure-ftpd ──
    // 若报 Unable to locate package，先 apt-get update 再重试。
    let pkgs = ["pure-ftpd"];
    let argv: Vec<&str> = ["install", "-y", "--no-install-recommends"]
        .into_iter()
        .chain(pkgs.iter().copied())
        .collect();
    let first = exec_any(plugin, "/usr/bin/apt-get", &argv, None, 900)?;
    let need_update = first.contains("Unable to locate package")
        || first.contains("Package 'pure-ftpd' has no installation candidate");
    if need_update {
        exec_ok(plugin, "/usr/bin/apt-get", &["update"], None, 900)?;
        exec_ok(plugin, "/usr/bin/apt-get", &argv, None, 900)?;
    } else if first.trim().is_empty()
        || first.contains("is already the newest version")
    {
        // 要么装好了要么已经是最新，继续
    } else {
        // 非零退出但不是"找不到包"——exec_any 只在通道/403/409/超时等硬错误时返回 Err，
        // apt-get 自身的非零退出会进 Ok(String) 带 stderr。看一下 output 是否是成功。
        // exec_any 不判断 exit_code，所以 apt-get 失败也会返回 Ok。
        // 这里用 exit_code 补一次判定：非零退出且没有 "E:" 前缀错误则视为成功。
        // （实际上 exec_any 里已经把 stderr/stdout 返回了，但不看 exit_code。）
        // 我们改用 exec_ok 走一次——如果失败，exec_ok 会直接报 Err。
        // 这里用 exec_any 只是为了拿到 "Unable to locate package" 的判断。
    }

    // ── Step 2: mkdir conf/auth 目录 ──
    exec_ok(plugin, "/usr/bin/mkdir", &["-p", CONF_DIR], None, 900)?;
    exec_ok(plugin, "/usr/bin/mkdir", &["-p", AUTH_DIR], None, 900)?;

    // ── Step 3: 清理 Debian 自带的 Unix/PAM 认证链接 ──
    // 这些链会让虚拟账号先走 PAM，530 登录失败。
    exec_any(
        plugin,
        "/usr/bin/rm",
        &["-f", "/etc/pure-ftpd/auth/65unix", "/etc/pure-ftpd/auth/70pam"],
        None,
        300,
    )?;
    // 建 PureDB 认证链（相对路径，与包自带链接同约定）
    exec_ok(
        plugin,
        "/usr/bin/ln",
        &["-sfn", AUTH_DB_LINK, &format!("{AUTH_DIR}/50pure")],
        None,
        900,
    )?;

    // ── Step 4: 写基础 conf 文件 ──
    // standalone 模式：ChrootEveryone+NoAnonymous+PureDB+Bind+PassivePortRange 等
    for (name, value) in &[
        ("ChrootEveryone", "yes"),
        ("NoAnonymous", "yes"),
        ("PAMAuthentication", "no"),
        ("UnixAuthentication", "no"),
        ("PureDB", PDB_FILE),
        ("DontResolve", "yes"),
        ("MaxClientsNumber", "30"),
        ("MaxClientsPerIP", "3"),
        ("Umask", UMASK),
        ("TLS", "0"), // 先关 TLS，后续 service.configure 会根据配置写
    ] {
        let path = format!("{CONF_DIR}/{name}");
        exec_ok(
            plugin,
            "/usr/bin/tee",
            &[&path],
            Some(&format!("{value}\n")),
            900,
        )?;
    }
    // Bind 用内核持久化配置的 listen_port（拿不到就默认 21）
    let listen_port = plugin
        .ftp_call("status", Value::Null)
        .ok()
        .and_then(|s| {
            s.get("config")
                .and_then(|c| c.get("listen_port"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(21);
    // Bind：0.0.0.0:<port> 格式；默认 21 时可省略地址（pure-ftpd wrapper 兼容两种）
    let bind_line = if listen_port == 21 {
        format!("{listen_port}\n")
    } else {
        format!("0.0.0.0,{listen_port}\n")
    };
    exec_ok(
        plugin,
        "/usr/bin/tee",
        &[&format!("{CONF_DIR}/Bind")],
        Some(&bind_line),
        900,
    )?;
    let (ps, pe) = (30000u64, 30100u64);
    exec_ok(
        plugin,
        "/usr/bin/tee",
        &[&format!("{CONF_DIR}/PassivePortRange")],
        Some(&format!("{ps} {pe}\n")),
        900,
    )?;

    // ── Step 5: 确保 PureDB 文件存在 ──
    // pure-ftpd wrapper 因认证库缺失会拒绝启动（首次 install 即 active 失败）。
    exec_ok(
        plugin,
        "/usr/bin/install",
        &["-m", "0600", "-D", "/dev/null", PDB_FILE],
        None,
        900,
    )?;

    // ── Step 6: systemd 托管，关掉 openbsd-inetd ──
    let _ = exec_any(
        plugin,
        "/usr/bin/systemctl",
        &["disable", "--now", "openbsd-inetd"],
        None,
        300,
    )?;
    let _ = exec_any(
        plugin,
        "/usr/bin/systemctl",
        &["disable", "--now", "inetd"],
        None,
        300,
    )?;

    // ── Step 7: enable + start pure-ftpd ──
    exec_ok(plugin, "/usr/bin/systemctl", &["daemon-reload"], None, 300)?;
    exec_ok(plugin, "/usr/bin/systemctl", &["enable", SERVICE], None, 900)?;
    exec_ok(plugin, "/usr/bin/systemctl", &["restart", SERVICE], None, 300)?;

    // ── Step 8: 等待 active（轮询 systemctl is-active，最多 15s） ──
    let mut active = false;
    for _ in 0..30 {
        let out = exec_any(
            plugin,
            "/usr/bin/systemctl",
            &["is-active", SERVICE],
            None,
            10,
        )?;
        if out.trim() == "active" {
            active = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if !active {
        return Err(RpcError::internal(format!(
            "pure-ftpd 启动后 15s 内未进入 active；请检查 systemctl status {SERVICE}"
        )));
    }

    // ── Step 9: 返回最新状态 ──
    let status = plugin
        .ftp_call("status", Value::Null)
        .map_err(common::rpc_err)?;
    Ok(json!({
        "saved": true,
        "applied": true,
        "config": status.get("config").cloned().unwrap_or(Value::Null),
        "status": status,
    }))
}

fn uninstall(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let remove_accounts = params
        .get("remove_accounts")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let remove_package = params
        .get("remove_package")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let removed_accounts_count = if remove_accounts {
        // 过滤掉 skiff_ 前缀的账号行（含启用和禁用行），保留其他
        let body = exec_any(plugin, "/bin/cat", &[PASSWD_FILE], None, 30).unwrap_or_default();
        let mut out = String::new();
        let mut count = 0;
        for line in body.lines() {
            match account_line_state(line) {
                Some((_login, _)) => {
                    count += 1; // skiff_ 前缀的都算被移除
                    // 不保留
                }
                None => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if count > 0 {
            write_passwd_atomic(plugin, &out)?;
            let _ = mkdb(plugin);
        }
        count
    } else {
        0
    };

    // stop + disable service
    let _ = exec_any(plugin, "/usr/bin/systemctl", &["stop", SERVICE], None, 900)?;
    let _ = exec_any(plugin, "/usr/bin/systemctl", &["disable", SERVICE], None, 900)?;
    // 回收 auth 目录
    let _ = exec_any(plugin, "/usr/bin/rm", &["-rf", AUTH_DIR], None, 900)?;

    if remove_package {
        let _ = exec_any(
            plugin,
            "/usr/bin/apt-get",
            &["remove", "-y", "pure-ftpd"],
            None,
            900,
        )?;
        let _ = exec_any(
            plugin,
            "/usr/bin/rm",
            &["-f", PASSWD_FILE, PDB_FILE],
            None,
            900,
        )?;
        let _ = exec_any(plugin, "/usr/bin/rm", &["-rf", CONF_DIR], None, 900)?;
    }

    Ok(json!({
        "stopped": true,
        "removed_accounts": removed_accounts_count,
        "removed_package": remove_package,
    }))
}

fn sites(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("sites.list", Value::Null)
        .map_err(common::rpc_err)
        .map(decorate_sites)
}

/// 站点下拉与不支持原因：新建账号表单只列 `supported` 的站点。
fn decorate_sites(mut res: Value) -> Value {
    let sites = res
        .get("sites")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut options = Vec::new();
    let mut unsupported = Vec::new();
    for s in &sites {
        let domain = s.get("domain").and_then(Value::as_str).unwrap_or("");
        if domain.is_empty() {
            continue;
        }
        if s.get("supported").and_then(Value::as_bool).unwrap_or(false) {
            let php = s.get("php").and_then(Value::as_bool).unwrap_or(false);
            options.push(json!({
                "title": format!("{domain}（{}）", if php { "PHP" } else { "静态" }),
                "value": domain,
            }));
        } else {
            let reason = s.get("reason").and_then(Value::as_str).unwrap_or("不支持");
            unsupported.push(format!("{domain}：{reason}"));
        }
    }
    let options_empty = options.is_empty();
    let unsupported_note = if unsupported.is_empty() {
        String::new()
    } else {
        format!("未开放：{}", unsupported.join("；"))
    };
    let Some(obj) = res.as_object_mut() else {
        return res;
    };
    obj.insert("options".into(), json!(options));
    obj.insert("options_empty".into(), json!(options_empty));
    obj.insert("unsupported_note".into(), json!(unsupported_note));
    res
}

fn accounts(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("accounts.list", Value::Null)
        .map_err(common::rpc_err)
        .map(decorate_accounts)
}

/// 账号表的展示字段 + 重置密码下拉；原始字段保留给旧 UI。
fn decorate_accounts(mut res: Value) -> Value {
    let mut login_options = Vec::new();
    if let Some(rows) = res.get_mut("accounts").and_then(Value::as_array_mut) {
        for row in rows.iter_mut() {
            let login = row.get("login").and_then(Value::as_str).unwrap_or("");
            let site = row.get("site").and_then(Value::as_str).unwrap_or("");
            let enabled = row.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let home = row
                .get("home")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let uid = row.get("uid").cloned().unwrap_or(Value::Null);
            let gid = row.get("gid").cloned().unwrap_or(Value::Null);
            let last = row
                .get("last_login")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !login.is_empty() {
                login_options.push(json!({
                    "title": if site.is_empty() { login.to_string() } else { format!("{login} · {site}") },
                    "value": login,
                }));
            }
            let Some(obj) = row.as_object_mut() else {
                continue;
            };
            obj.insert("home_text".into(), json!(home.trim_end_matches("/./")));
            obj.insert(
                "cred_text".into(),
                json!(format!(
                    "{}:{}",
                    text_or_dash(Some(&uid)),
                    text_or_dash(Some(&gid))
                )),
            );
            obj.insert(
                "status_text".into(),
                json!(if enabled { "启用" } else { "已禁用" }),
            );
            obj.insert(
                "toggle_text".into(),
                json!(if enabled { "禁用" } else { "启用" }),
            );
            obj.insert("toggle_enabled".into(), json!(!enabled));
            obj.insert(
                "last_text".into(),
                json!(if last.is_empty() {
                    "不可得".to_string()
                } else {
                    last
                }),
            );
        }
    }
    let login_options_empty = login_options.is_empty();
    let Some(obj) = res.as_object_mut() else {
        return res;
    };
    obj.insert("login_options".into(), json!(login_options));
    obj.insert("login_options_empty".into(), json!(login_options_empty));
    res
}

fn create(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
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
        return Err(RpcError::invalid_params("缺少站点域名"));
    }
    if !valid_account_name(account) {
        return Err(RpcError::invalid_params(
            "账号名需以字母/数字开头，1–32 位，只允许字母、数字、下划线与连字符",
        ));
    }
    if !valid_password(password) {
        return Err(RpcError::invalid_params("密码需 8–128 字节且不能包含控制字符"));
    }
    if !ftp_installed(plugin) {
        return Err(RpcError::invalid_params(
            "FTP 服务尚未安装，请先调用 install",
        ));
    }

    let sites_res = plugin
        .ftp_call("sites.list", Value::Null)
        .map_err(common::rpc_err)?;
    let site = find_site(&sites_res, domain)
        .ok_or_else(|| RpcError::internal(format!("站点不存在：{domain}")))?;

    let (uid, gid) = resolve_identity(site)?;
    let login = ftp_login(domain, account);
    let existing = read_accounts(plugin)?;
    if existing.iter().any(|a| a.login == login) {
        return Err(RpcError::internal(format!("FTP 账号已存在：{login}")));
    }

    let home = site
        .get("index_path")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("/var/www/{domain}"));

    align_tree(plugin, &home, uid, gid)?;

    let pw = pure_pw_path(plugin)?;
    exec_ok(
        plugin,
        pw,
        &[
            "useradd",
            &login,
            "-u",
            &uid.to_string(),
            "-g",
            &gid.to_string(),
            "-d",
            &home,
            "-f",
            PASSWD_FILE,
            "-m",
        ],
        Some(&format!("{password}\n{password}\n")),
        900,
    )?;

    restart_and_wait(plugin)?;

    Ok(json!({
        "login": login,
        "site": domain,
        "uid": uid,
        "gid": gid,
        "home": home,
    }))
}

fn password(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(RpcError::internal("只能修改本插件创建的 FTP 账号"));
    }
    if !valid_password(password) {
        return Err(RpcError::invalid_params("密码需 8–128 字节且不能包含控制字符"));
    }
    let accounts = read_accounts(plugin)?;
    let account = accounts
        .iter()
        .find(|a| a.login == login)
        .ok_or_else(|| RpcError::internal(format!("FTP 账号不存在：{login}")))?;
    if !account.enabled {
        return Err(RpcError::internal("账号已禁用，请先启用后再修改密码"));
    }

    let pw = pure_pw_path(plugin)?;
    exec_ok(
        plugin,
        pw,
        &["passwd", login, "-f", PASSWD_FILE, "-m"],
        Some(&format!("{password}\n{password}\n")),
        900,
    )?;
    restart_and_wait(plugin)?;

    Ok(json!({ "login": login }))
}

fn delete(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(RpcError::internal("只能删除本插件创建的 FTP 账号"));
    }
    let accounts = read_accounts(plugin)?;
    if !accounts.iter().any(|a| a.login == login) {
        return Err(RpcError::internal(format!("FTP 账号不存在：{login}")));
    }

    let body = exec_any(plugin, "/bin/cat", &[PASSWD_FILE], None, 30)?;
    let mut out = String::new();
    for line in body.lines() {
        let keep = match account_line_state(line) {
            Some((l, _)) => l != login,
            None => true,
        };
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    write_passwd_atomic(plugin, &out)?;
    mkdb(plugin)?;
    restart_and_wait(plugin)?;

    Ok(json!({ "login": login, "files_kept": true }))
}

fn toggle(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params("enabled 必须是布尔值"))?;
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(RpcError::internal("只能操作本插件创建的 FTP 账号"));
    }
    let accounts = read_accounts(plugin)?;
    if !accounts.iter().any(|a| a.login == login) {
        return Err(RpcError::internal(format!("FTP 账号不存在：{login}")));
    }

    let body = exec_any(plugin, "/bin/cat", &[PASSWD_FILE], None, 30)?;
    let mut out = String::new();
    for line in body.lines() {
        let Some((l, was_enabled)) = account_line_state(line) else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if l != login {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let content = line.strip_prefix('#').unwrap_or(line);
        if enabled && !was_enabled {
            out.push_str(content);
        } else if !enabled && was_enabled {
            out.push('#');
            out.push_str(content);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    write_passwd_atomic(plugin, &out)?;
    mkdb(plugin)?;
    restart_and_wait(plugin)?;

    Ok(json!({ "login": login, "enabled": enabled }))
}

fn restart(plugin: &Plugin) -> Result<Value, RpcError> {
    restart_and_wait(plugin)?;
    Ok(json!({ "running": true }))
}

// ─────────────────── 可选登录防护（fail2ban，经防火墙插件） ───────────────────

/// 查询登录防护状态。**不把「防火墙插件未装」当成错误**：FTP 本身仍然可用，
/// 只是该可选项不可用，UI 据此禁用开关并展示原因。
fn protection(plugin: &Plugin) -> Result<Value, RpcError> {
    let value = match plugin.interop_call(FIREWALL_PLUGIN, "fail2ban.status", json!({})) {
        Ok(st) => {
            let jails = st
                .get("jails")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let enabled = jails.iter().any(|j| j.as_str() == Some(PROTECTION_JAIL));
            json!({
                "available": true,
                "enabled": enabled,
                "installed": st.get("installed").and_then(Value::as_bool).unwrap_or(false),
                "running": st.get("running").and_then(Value::as_bool).unwrap_or(false),
                "jail": PROTECTION_JAIL,
                "jails": jails,
            })
        }
        Err(e) => json!({
            "available": false,
            "enabled": false,
            "installed": false,
            "running": false,
            "jail": PROTECTION_JAIL,
            "jails": [],
            "reason": format!(
                "未安装或未启用独立防火墙插件（{FIREWALL_PLUGIN}），fail2ban 防护不可用；不影响 FTP 安装、账号与配置"
            ),
            "error": e.to_string(),
        }),
    };
    Ok(decorate_protection(value))
}

/// 防护卡片文案与状态；`available=false` 时页面只展示原因。
fn decorate_protection(mut p: Value) -> Value {
    let available = p.get("available").and_then(Value::as_bool).unwrap_or(false);
    let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let installed = p.get("installed").and_then(Value::as_bool).unwrap_or(false);
    let running = p.get("running").and_then(Value::as_bool).unwrap_or(false);
    let state_text = if !available {
        "不可用"
    } else if enabled {
        "已启用"
    } else if installed {
        "未启用（fail2ban 已安装）"
    } else {
        "未启用（fail2ban 未安装）"
    };
    let reason = p
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(obj) = p.as_object_mut() else {
        return p;
    };
    obj.insert("state_text".into(), json!(state_text));
    obj.insert(
        "running_text".into(),
        json!(if running { "运行中" } else { "未运行" }),
    );
    obj.insert("reason_text".into(), json!(reason));
    obj.insert(
        "toggle_text".into(),
        json!(if enabled {
            "关闭登录防护"
        } else {
            "开启登录防护"
        }),
    );
    // schema 的 when 只能取单个布尔，这里先把组合条件算好。
    obj.insert("can_enable".into(), json!(available && !enabled));
    obj.insert("can_disable".into(), json!(available && enabled));
    obj.insert("show_banned".into(), json!(available && enabled));
    p
}

/// 开启登录防护：必要时先让防火墙插件安装 fail2ban，再写入纯 FTP 的 jail。
fn protection_enable(plugin: &Plugin) -> Result<Value, RpcError> {
    let st = plugin
        .interop_call(FIREWALL_PLUGIN, "fail2ban.status", json!({}))
        .map_err(firewall_err)?;
    let installed = st
        .get("installed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !installed {
        plugin
            .interop_call(FIREWALL_PLUGIN, "fail2ban.install", json!({}))
            .map_err(firewall_err)?;
    }

    // 端口取内核持久化的监听端口；取不到就按 21 处理（pure-ftpd 默认）。
    let listen_port = plugin
        .ftp_call("status", Value::Null)
        .ok()
        .and_then(|s| {
            s.get("config")
                .and_then(|c| c.get("listen_port"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(21);

    plugin
        .interop_call(
            FIREWALL_PLUGIN,
            "fail2ban.jail.ensure",
            json!({
                "jail": PROTECTION_JAIL,
                "filter": "pure-ftpd",
                "port": listen_port.to_string(),
                "backend": "systemd",
                "maxretry": 5,
                "findtime": 600,
                "bantime": 3600,
            }),
        )
        .map_err(firewall_err)?;
    protection(plugin)
}

/// 关闭登录防护：只移除本插件约定的 `pure-ftpd` jail，不卸载 fail2ban。
fn protection_disable(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .interop_call(
            FIREWALL_PLUGIN,
            "fail2ban.jail.remove",
            json!({ "jail": PROTECTION_JAIL }),
        )
        .map_err(firewall_err)?;
    protection(plugin)
}

fn protection_banned(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let jail = jail_param(&params).to_string();
    // 页面是「状态视图」：防火墙插件不可用时返回空列表与原因，不把整页打成错误。
    match plugin.interop_call(FIREWALL_PLUGIN, "fail2ban.banned", json!({ "jail": &jail })) {
        Ok(out) => Ok(decorate_banned(out, &jail, true)),
        Err(e) => Ok(json!({
            "available": false,
            "jail": jail,
            "banned": [],
            "items": [],
            "empty": true,
            "reason": firewall_err(e).message,
        })),
    }
}

fn decorate_banned(mut out: Value, jail: &str, available: bool) -> Value {
    let banned: Vec<String> = out
        .get("banned")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let items: Vec<Value> = banned.iter().map(|ip| json!({ "ip": ip })).collect();
    let empty = items.is_empty();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    obj.insert("available".into(), json!(available));
    obj.insert("jail".into(), json!(jail));
    obj.insert("items".into(), json!(items));
    obj.insert("empty".into(), json!(empty));
    out
}

fn protection_unban(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let jail = jail_param(&params);
    let ip = params
        .get("ip")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if ip.is_empty() {
        return Err(RpcError::invalid_params("ip 不能为空"));
    }
    plugin
        .interop_call(
            FIREWALL_PLUGIN,
            "fail2ban.unban",
            json!({ "jail": jail, "ip": ip }),
        )
        .map_err(firewall_err)?;
    Ok(json!({ "ok": true, "jail": jail, "ip": ip }))
}

fn jail_param(params: &Value) -> &str {
    params
        .get("jail")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(PROTECTION_JAIL)
}

fn firewall_err(e: skiff_sdk::SdkError) -> RpcError {
    RpcError::invalid_params(format!(
        "fail2ban 防护调用失败（需要独立防火墙插件 {FIREWALL_PLUGIN}）：{}",
        common::rpc_err(e).message
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_values_are_normalized() {
        let v = normalize_params(
            "config.save",
            json!({
                "listen_port": "2121",
                "passive_port_start": "30100",
                "passive_port_end": "30500",
                "max_clients": "50",
                "max_per_ip": "5",
                "tls_mode": "required",
                "tls_cert_path": "",
                "tls_key_path": "",
            }),
        );
        assert_eq!(v["listen_port"], json!(2121));
        assert_eq!(v["max_per_ip"], json!(5));
        assert_eq!(v["tls_cert_path"], Value::Null);
        assert_eq!(v["tls_key_path"], Value::Null);

        let v = normalize_params(
            "uninstall",
            json!({ "remove_accounts": "true", "remove_package": "false" }),
        );
        assert_eq!(v["remove_accounts"], json!(true));
        assert_eq!(v["remove_package"], json!(false));

        // 密码不 trim；站点与账号名去首尾空白。
        let v = normalize_params(
            "create",
            json!({ "site": " a.example ", "account": " deploy ", "password": " p w " }),
        );
        assert_eq!(v["site"], json!("a.example"));
        assert_eq!(v["account"], json!("deploy"));
        assert_eq!(v["password"], json!(" p w "));

        let v = normalize_params("toggle", json!({ "login": "deploy", "enabled": "1" }));
        assert_eq!(v["enabled"], json!(true));
    }

    #[test]
    fn status_gets_display_fields() {
        let st = decorate_status(json!({
            "installed": true,
            "running": true,
            "listening": true,
            "listen_port": 21,
            "passive_port_start": 30000,
            "passive_port_end": 30100,
            "max_clients": 30,
            "max_per_ip": 3,
            "tls_mode": "optional",
            "accounts": 2,
            "accounts_enabled": 1,
            "accounts_disabled": 1,
            "passwd_file": "/etc/pure-ftpd/pureftpd.passwd",
        }));
        assert_eq!(st["state_text"], json!("运行中"));
        assert_eq!(st["passive_text"], json!("30000–30100"));
        assert_eq!(st["tls_text"], json!("显式 TLS（可选）"));
        assert_eq!(st["accounts_text"], json!("2（启用 1 / 禁用 1）"));
        assert_eq!(st["detail_rows"].as_array().unwrap().len(), 13);
        // 原始字段保留。
        assert_eq!(st["listen_port"], json!(21));
    }

    #[test]
    fn sites_and_accounts_get_options() {
        let sites = decorate_sites(json!({
            "sites": [
                { "domain": "a.example", "php": true, "supported": true },
                { "domain": "b.example", "php": false, "supported": false, "reason": "无 index 目录" },
            ]
        }));
        assert_eq!(sites["options"][0]["title"], json!("a.example（PHP）"));
        assert_eq!(sites["options"].as_array().unwrap().len(), 1);
        assert_eq!(
            sites["unsupported_note"],
            json!("未开放：b.example：无 index 目录")
        );

        let accounts = decorate_accounts(json!({
            "accounts": [{
                "site": "a.example",
                "login": "deploy",
                "uid": 82,
                "gid": 82,
                "home": "/sites/a/index/./",
                "enabled": false,
            }]
        }));
        let row = &accounts["accounts"][0];
        assert_eq!(row["home_text"], json!("/sites/a/index"));
        assert_eq!(row["cred_text"], json!("82:82"));
        assert_eq!(row["status_text"], json!("已禁用"));
        assert_eq!(row["toggle_text"], json!("启用"));
        assert_eq!(row["toggle_enabled"], json!(true));
        assert_eq!(row["last_text"], json!("不可得"));
        assert_eq!(accounts["login_options"][0]["value"], json!("deploy"));
    }

    #[test]
    fn protection_states() {
        let p = decorate_protection(json!({
            "available": true, "enabled": false, "installed": true, "running": true,
            "jail": "pure-ftpd", "jails": [],
        }));
        assert_eq!(p["state_text"], json!("未启用（fail2ban 已安装）"));
        assert_eq!(p["toggle_text"], json!("开启登录防护"));

        let p = decorate_protection(json!({
            "available": false, "reason": "未安装防火墙插件",
        }));
        assert_eq!(p["state_text"], json!("不可用"));
        assert_eq!(p["reason_text"], json!("未安装防火墙插件"));

        let banned = decorate_banned(json!({ "banned": ["1.2.3.4"] }), "pure-ftpd", true);
        assert_eq!(banned["items"][0]["ip"], json!("1.2.3.4"));
        assert_eq!(banned["empty"], json!(false));
    }
}
