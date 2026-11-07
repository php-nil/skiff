//! `com.skiff.ftp`：按站点创建 pure-ftpd 虚拟账号。
//!
//! 所有特权动作都委托给内核 `ftp` 能力面；插件本身不执行宿主命令、不落
//! 账号密码。密码从 UI 经内核 RPC 到插件，再原样交给内核；插件不写日志、
//! 不持久化。账号列表由内核读 pure-pw 用户库返回，插件保持无状态，重装后
//! 仍能看到既有账号。

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
        "restart" => restart(&p),
        "config" => Ok(config(&p)),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

fn config(plugin: &Plugin) -> Value {
    let map = common::load_config(plugin)
        .into_iter()
        .map(|(k, v)| (k, Value::from(v)))
        .collect::<serde_json::Map<String, Value>>();
    Value::Object(map)
}

fn status(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("status", Value::Null)
        .map_err(common::rpc_err)
}

fn install(plugin: &Plugin) -> Result<Value, RpcError> {
    let cfg = common::load_config(plugin);
    let start = common::cfg_u64(&cfg, "passive_port_start", 30000);
    let end = common::cfg_u64(&cfg, "passive_port_end", 30100);
    let tls = common::cfg_str(&cfg, "tls_mode", "optional");
    let guard = common::cfg_bool(&cfg, "login_guard", true);
    plugin
        .ftp_call(
            "install",
            json!({
                "passive_port_start": start,
                "passive_port_end": end,
                "tls_mode": tls,
                "login_guard": guard,
            }),
        )
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

fn restart(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .ftp_call("service.restart", Value::Null)
        .map_err(common::rpc_err)
}
