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
        "uninstall" => uninstall(&p, params),
        "sites" => sites(&p),
        "accounts" => accounts(&p),
        "create" => create(&p, params),
        "password" => password(&p, params),
        "delete" => delete(&p, params),
        "toggle" => toggle(&p, params),
        "restart" => restart(&p),
        "config" => config(&p),
        "config.save" | "save_config" => save_config(&p, params),
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
}

fn accounts(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("accounts.list", Value::Null)
        .map_err(common::rpc_err)
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
    match plugin.interop_call(FIREWALL_PLUGIN, "fail2ban.status", json!({})) {
        Ok(st) => {
            let jails = st
                .get("jails")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let enabled = jails.iter().any(|j| j.as_str() == Some(PROTECTION_JAIL));
            Ok(json!({
                "available": true,
                "enabled": enabled,
                "installed": st.get("installed").and_then(Value::as_bool).unwrap_or(false),
                "running": st.get("running").and_then(Value::as_bool).unwrap_or(false),
                "jail": PROTECTION_JAIL,
                "jails": jails,
            }))
        }
        Err(e) => Ok(json!({
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
        })),
    }
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
    let jail = jail_param(&params);
    let mut out = plugin
        .interop_call(FIREWALL_PLUGIN, "fail2ban.banned", json!({ "jail": jail }))
        .map_err(firewall_err)?;
    if let Value::Object(map) = &mut out {
        map.insert("available".into(), Value::Bool(true));
    }
    Ok(out)
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
