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
        "status" => call(&p, "status", json!({})).map(decorate_fw_status),
        "rules.list" => call(&p, "rules.list", json!({})).map(decorate_rules),
        "rule.add" => call(&p, "rule.add", normalize_params("rule.add", params)),
        "rule.remove" => call(&p, "rule.remove", params),
        "port.allow" => call(&p, "port.allow", normalize_params("port.allow", params)),
        "fail2ban.status" => call(&p, "fail2ban.status", json!({})).map(decorate_f2b_status),
        "fail2ban.install" => call(&p, "fail2ban.install", json!({})),
        "fail2ban.start" => call(&p, "fail2ban.start", json!({})),
        "fail2ban.stop" => call(&p, "fail2ban.stop", json!({})),
        "fail2ban.uninstall" => call(&p, "fail2ban.uninstall", json!({})),
        "fail2ban.jail.ensure" => call(
            &p,
            "fail2ban.jail.ensure",
            normalize_params("fail2ban.jail.ensure", params),
        ),
        "fail2ban.jail.remove" => call(&p, "fail2ban.jail.remove", params),
        "fail2ban.banned" => call(&p, "fail2ban.banned", params),
        "fail2ban.ban" => call(&p, "fail2ban.ban", params),
        "fail2ban.unban" => call(&p, "fail2ban.unban", params),
        // 新壳 fail2ban 页的按需读接口：未指定 jail 时返回空结果，
        // 避免 schema 的静态数据源在未选 jail 时报错。
        "banned" => banned(&p, &params),
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

/// schema 表单值都是字符串：写入口按方法把已知数值字段转成整数，并丢弃空字符串。
///
/// 空字符串在这条链路上等价于「未填写」：`rule.add` 的 `source`、`jail.ensure` 的
/// `logpath` 都允许缺省；保留 `""` 会被内核参数校验当成非法值。
fn normalize_params(method: &str, params: Value) -> Value {
    let Some(mut obj) = params.as_object().cloned() else {
        return params;
    };
    obj.retain(|_, v| !v.as_str().is_some_and(|s| s.trim().is_empty()));
    let numeric: &[&str] = match method {
        "rule.add" | "port.allow" => &["port"],
        "fail2ban.jail.ensure" => &["maxretry", "findtime", "bantime"],
        _ => &[],
    };
    for key in numeric {
        if let Some(Value::String(text)) = obj.get(*key) {
            if let Ok(n) = text.trim().parse::<u64>() {
                obj.insert((*key).to_string(), json!(n));
            }
        }
    }
    Value::Object(obj)
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => "-".to_string(),
    }
}

/// 规则/状态页的展示字段：内核返回值原样保留，只补可读文案。
fn decorate_fw_status(mut st: Value) -> Value {
    let backend = st.get("backend").cloned().unwrap_or(Value::Null);
    let nft = st.get("nftables").and_then(Value::as_bool).unwrap_or(false);
    let ipt = st.get("iptables").and_then(Value::as_bool).unwrap_or(false);
    let f2b = st.get("fail2ban").cloned().unwrap_or_else(|| json!({}));
    let f2b_state = f2b
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    let f2b_text = match f2b_state {
        "running" => "运行中",
        "stopped" => "已安装未运行",
        _ => "未安装",
    };
    let warnings = st
        .get("warnings")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_default();
    let Some(obj) = st.as_object_mut() else {
        return st;
    };
    obj.insert(
        "backend_text".into(),
        json!(if backend.is_null() {
            "不可用".to_string()
        } else {
            text_of(Some(&backend))
        }),
    );
    obj.insert(
        "nftables_text".into(),
        json!(if nft { "可用" } else { "未找到" }),
    );
    obj.insert(
        "iptables_text".into(),
        json!(if ipt { "可用" } else { "未找到" }),
    );
    obj.insert("fail2ban_text".into(), json!(f2b_text));
    obj.insert("warnings_any".into(), json!(!warnings.is_empty()));
    obj.insert("warnings_text".into(), json!(warnings));
    st
}

fn decorate_rules(mut res: Value) -> Value {
    if let Some(items) = res.get_mut("items").and_then(Value::as_array_mut) {
        for row in items.iter_mut() {
            let action = row.get("action").and_then(Value::as_str).unwrap_or("");
            let protocol = row.get("protocol").cloned().unwrap_or(Value::Null);
            let port = row.get("port").cloned().unwrap_or(Value::Null);
            let source = row.get("source").cloned().unwrap_or(Value::Null);
            let action_text = match action {
                "accept" => "放行",
                "drop" => "拦截",
                _ => "-",
            };
            let Some(obj) = row.as_object_mut() else {
                continue;
            };
            obj.insert("action_text".into(), json!(action_text));
            obj.insert("protocol_text".into(), json!(text_of(Some(&protocol))));
            obj.insert("port_text".into(), json!(text_of(Some(&port))));
            obj.insert("source_text".into(), json!(text_of(Some(&source))));
        }
    }
    res
}

fn decorate_f2b_status(mut st: Value) -> Value {
    let state = st
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("missing")
        .to_string();
    let reason = st
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let jail_rows: Vec<Value> = st
        .get("jails")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|name| json!({ "name": name }))
                .collect()
        })
        .unwrap_or_default();
    let jail_rows_empty = jail_rows.is_empty();
    let Some(obj) = st.as_object_mut() else {
        return st;
    };
    obj.insert(
        "state_text".into(),
        json!(match state.as_str() {
            "running" => "运行中",
            "stopped" => "已安装未运行",
            _ => "未安装",
        }),
    );
    obj.insert("missing".into(), json!(state == "missing"));
    obj.insert("stopped".into(), json!(state == "stopped"));
    obj.insert("reason_text".into(), json!(reason));
    obj.insert("jail_rows".into(), json!(jail_rows));
    obj.insert("jail_rows_empty".into(), json!(jail_rows_empty));
    st
}

/// 单 jail 封禁列表；未指定 jail 时返回空结果（schema 数据源无 `when`，不能报错）。
fn banned(p: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let jail = params
        .get("jail")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if jail.is_empty() {
        return Ok(json!({ "jail": "", "items": [], "empty": true }));
    }
    let res = call(p, "fail2ban.banned", json!({ "jail": jail }))?;
    let items: Vec<Value> = res
        .get("banned")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|ip| json!({ "jail": jail, "ip": ip }))
                .collect()
        })
        .unwrap_or_default();
    let empty = items.is_empty();
    Ok(json!({ "jail": jail, "items": items, "empty": empty }))
}

/// 能力面错误统一转成插件 RPC 错误；调用方（interop）拿到的就是内核状态码语义。
fn call(p: &Plugin, method: &str, params: Value) -> Result<Value, RpcError> {
    p.firewall_call(method, params).map_err(common::rpc_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_values_are_normalized() {
        let v = normalize_params(
            "rule.add",
            json!({ "port": "8080", "source": "", "protocol": "tcp", "action": "accept" }),
        );
        assert_eq!(v["port"], json!(8080));
        assert!(v.get("source").is_none(), "空字符串应等价于未填写");
        assert_eq!(v["protocol"], json!("tcp"));

        // fail2ban 的 port 可以是范围/服务名，保持字符串。
        let v = normalize_params(
            "fail2ban.jail.ensure",
            json!({ "jail": "sshd", "maxretry": "5", "port": "ssh" }),
        );
        assert_eq!(v["maxretry"], json!(5));
        assert_eq!(v["port"], json!("ssh"));
    }

    #[test]
    fn rule_rows_get_readable_text() {
        let res = decorate_rules(json!({
            "items": [
                { "id": 1, "action": "accept", "protocol": null, "port": 80, "source": null },
                { "id": 2, "action": "drop", "protocol": "tcp", "port": null, "source": "10.0.0.0/8" },
            ],
            "total": 2,
        }));
        assert_eq!(res["items"][0]["action_text"], json!("放行"));
        assert_eq!(res["items"][0]["protocol_text"], json!("-"));
        assert_eq!(res["items"][0]["port_text"], json!("80"));
        assert_eq!(res["items"][0]["source_text"], json!("-"));
        assert_eq!(res["items"][1]["action_text"], json!("拦截"));
        assert_eq!(res["items"][1]["port_text"], json!("-"));
        assert_eq!(res["items"][1]["source_text"], json!("10.0.0.0/8"));
        // 原始字段保留给旧 UI。
        assert_eq!(res["items"][0]["port"], json!(80));
    }

    #[test]
    fn f2b_status_decorates_state() {
        let st = decorate_f2b_status(json!({
            "state": "stopped",
            "installed": true,
            "running": false,
            "jails": ["sshd"],
        }));
        assert_eq!(st["state_text"], json!("已安装未运行"));
        assert_eq!(st["stopped"], json!(true));
        assert_eq!(st["missing"], json!(false));
        assert_eq!(st["jail_rows"][0]["name"], json!("sshd"));
        assert_eq!(st["jail_rows_empty"], json!(false));
    }

    #[test]
    fn firewall_status_keeps_raw_fields() {
        let st = decorate_fw_status(json!({
            "backend": "nftables",
            "nftables": true,
            "iptables": false,
            "fail2ban": { "state": "running" },
            "warnings": ["nft 与 iptables 同时存在"],
        }));
        assert_eq!(st["backend_text"], json!("nftables"));
        assert_eq!(st["nftables_text"], json!("可用"));
        assert_eq!(st["iptables_text"], json!("未找到"));
        assert_eq!(st["fail2ban_text"], json!("运行中"));
        assert_eq!(st["warnings_any"], json!(true));
        assert_eq!(st["warnings_text"], json!("nft 与 iptables 同时存在"));
        assert_eq!(st["backend"], json!("nftables"));
    }
}
