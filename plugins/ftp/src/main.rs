//! `com.skiff.ftp`：按站点创建 pure-ftpd 虚拟账号，并提供服务配置面板。
//!
//! 所有特权动作都委托给内核 `ftp` 能力面；插件本身不执行宿主命令、不落
//! 账号密码。密码从 UI 经内核 RPC 到插件，再原样交给内核；插件不写日志、
//! 不持久化密码。账号列表由内核读 pure-pw 用户库返回，插件保持无状态，
//! 重装后仍能看到既有账号。
//!
//! 服务配置（监听端口、被动端口、连接上限、TLS）由内核
//! `/etc/skiff/ftp-service.json` 持久化，是本插件唯一的真相来源；插件不再
//! 写本地 `service-config.json`，只把表单原样交给 `service.configure`。
//!
//! **fail2ban 不再由 FTP 安装或配置**：启用登录防护是可选能力，经 interop
//! 调用独立防火墙插件 `com.skiff.firewall` 的 `fail2ban.*` 接口。未安装防火墙
//! 插件时该功能不可用，但 FTP 的安装、账号与配置不受影响。内核侧旧的
//! `login_guard` 字段仍在，本插件显式传 `false`，避免内核继续代管 fail2ban。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

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
    let keys = [
        "listen_port",
        "passive_port_start",
        "passive_port_end",
        "max_clients",
        "max_per_ip",
        "tls_mode",
        "tls_cert_path",
        "tls_key_path",
    ];
    let mut payload = serde_json::Map::new();
    for key in keys {
        if let Some(value) = params.get(key) {
            payload.insert(key.into(), value.clone());
        }
    }
    // 旧内核仍接受 `login_guard`；显式关掉，避免它继续安装/维护 fail2ban。
    // 登录防护现在由防火墙插件按需开启（`protection.enable`）。
    payload.insert("login_guard".into(), Value::Bool(false));
    if payload.len() == 1 {
        return Err(RpcError::invalid_params("没有可保存的配置项"));
    }
    let res = plugin
        .ftp_call("service.configure", Value::Object(payload))
        .map_err(common::rpc_err)?;
    let applied = res["applied"].as_bool().unwrap_or(false);
    Ok(json!({
        "ok": true,
        "saved": true,
        "applied": applied,
        "reloaded": applied,
        "config": res.get("config").cloned().unwrap_or(Value::Null),
        "status": res.get("status").cloned().unwrap_or(Value::Null),
        "message": if applied {
            "配置已保存并重载 pure-ftpd；若改了监听端口，请在宿主防火墙放通监听端口与被动端口范围"
        } else {
            "配置已保存；FTP 服务尚未安装，安装后生效"
        },
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
    // 内核从 `/etc/skiff/ftp-service.json` 读取已保存配置，插件不再叠加本地覆盖值；
    // `login_guard=false` 阻止旧内核顺带安装 fail2ban（登录防护走防火墙插件）。
    plugin
        .ftp_call("install", json!({ "login_guard": false }))
        .map_err(common::rpc_err)
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
    plugin
        .ftp_call(
            "uninstall",
            json!({
                "remove_accounts": remove_accounts,
                "remove_package": remove_package,
            }),
        )
        .map_err(common::rpc_err)
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
    let site = params.get("site").and_then(Value::as_str).unwrap_or("");
    let account = params.get("account").and_then(Value::as_str).unwrap_or("");
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if site.is_empty() || account.is_empty() || password.is_empty() {
        return Err(RpcError::invalid_params("站点、账号与密码不能为空"));
    }
    plugin
        .ftp_call(
            "account.create",
            json!({ "site": site, "account": account, "password": password }),
        )
        .map_err(common::rpc_err)
}

fn password(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params.get("login").and_then(Value::as_str).unwrap_or("");
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if login.is_empty() || password.is_empty() {
        return Err(RpcError::invalid_params("登录名与新密码不能为空"));
    }
    plugin
        .ftp_call(
            "account.password",
            json!({ "login": login, "password": password }),
        )
        .map_err(common::rpc_err)
}

fn delete(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params.get("login").and_then(Value::as_str).unwrap_or("");
    if login.is_empty() {
        return Err(RpcError::invalid_params("登录名不能为空"));
    }
    plugin
        .ftp_call("account.delete", json!({ "login": login }))
        .map_err(common::rpc_err)
}

fn toggle(plugin: &Plugin, params: Value) -> Result<Value, RpcError> {
    let login = params.get("login").and_then(Value::as_str).unwrap_or("");
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params("enabled 必须是布尔值"))?;
    if login.is_empty() {
        return Err(RpcError::invalid_params("登录名不能为空"));
    }
    plugin
        .ftp_call(
            "account.set_enabled",
            json!({ "login": login, "enabled": enabled }),
        )
        .map_err(common::rpc_err)
}

fn restart(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("service.restart", Value::Null)
        .map_err(common::rpc_err)
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
