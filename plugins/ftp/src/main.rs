//! `com.skiff.ftp`：按站点创建 pure-ftpd 虚拟账号，并提供服务配置面板。
//!
//! 所有特权动作都委托给内核 `ftp` 能力面；插件本身不执行宿主命令、不落
//! 账号密码。密码从 UI 经内核 RPC 到插件，再原样交给内核；插件不写日志、
//! 不持久化密码。账号列表由内核读 pure-pw 用户库返回，插件保持无状态，
//! 重装后仍能看到既有账号。
//!
//! 服务配置（监听端口、被动端口、连接上限、TLS 与登录防护）由内核
//! `/etc/skiff/ftp-service.json` 持久化，是本插件唯一的真相来源；插件不再
//! 写本地 `service-config.json`，只把表单原样交给 `service.configure`。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

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
                "login_guard": true,
            }),
            json!({ "error": e.message }),
        ),
    };
    let mut map = cfg.as_object().cloned().unwrap_or_default();
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
        "login_guard",
    ];
    let mut payload = serde_json::Map::new();
    for key in keys {
        if let Some(value) = params.get(key) {
            payload.insert(key.into(), value.clone());
        }
    }
    if payload.is_empty() {
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
    // 内核从 `/etc/skiff/ftp-service.json` 读取已保存配置，插件不再叠加本地覆盖值。
    plugin
        .ftp_call("install", json!({}))
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
