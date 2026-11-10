//! `com.skiff.firewall`：宿主机防火墙与 fail2ban 管理。
//!
//! 插件本身不持有任何宿主特权：所有 nftables/iptables 与 `/etc/fail2ban`
//! 操作都经内核 `firewall` 能力面（`skiff_plugin::firewall`）执行。插件只做
//! 参数透传与 UI 编排，因此进程可以保持降权运行。
//!
//! 对外通过 interop 提供 `fail2ban.*` 接口，供 FTP 等插件按需调用；未安装本
//! 插件时调用方应把防护标记为「不可用」，而不是让自身功能失败。

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
        "status" => call(&p, "status", json!({})),
        "rules.list" => call(&p, "rules.list", json!({})),
        "rule.add" => call(&p, "rule.add", params),
        "rule.remove" => call(&p, "rule.remove", params),
        "port.allow" => call(&p, "port.allow", params),
        "fail2ban.status" => call(&p, "fail2ban.status", json!({})),
        "fail2ban.install" => call(&p, "fail2ban.install", json!({})),
        "fail2ban.start" => call(&p, "fail2ban.start", json!({})),
        "fail2ban.stop" => call(&p, "fail2ban.stop", json!({})),
        "fail2ban.uninstall" => call(&p, "fail2ban.uninstall", json!({})),
        "fail2ban.jail.ensure" => call(&p, "fail2ban.jail.ensure", params),
        "fail2ban.jail.remove" => call(&p, "fail2ban.jail.remove", params),
        "fail2ban.banned" => call(&p, "fail2ban.banned", params),
        "fail2ban.ban" => call(&p, "fail2ban.ban", params),
        "fail2ban.unban" => call(&p, "fail2ban.unban", params),
        "config" => Ok(Value::Object(
            common::load_config(&p)
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect(),
        )),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

/// 能力面错误统一转成插件 RPC 错误；调用方（interop）拿到的就是内核状态码语义。
fn call(p: &Plugin, method: &str, params: Value) -> Result<Value, RpcError> {
    p.firewall_call(method, params).map_err(common::rpc_err)
}
