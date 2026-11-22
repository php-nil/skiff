//! 插件中心与插件信息页的 Schema。

use super::schema_kit::{self as kit, API_BASE};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false)
}

fn list_text(v: &Value, pointer: &str) -> Vec<String> {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 插件列表：已安装 / 发现 / 可更新三个页签共用一张页，页签只切内容块。
pub fn plugins_schema(data: &Value, tab: &str) -> Value {
    let plugins = data
        .get("plugins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let can_admin = flag(data, "/can_admin");

    let installed = format!("已安装 ({})", plugins.len());
    let mut body = vec![
        kit::flex(
            12,
            vec![
                kit::link(&installed, "${prefix}/plugins/installed"),
                kit::link("发现", "${prefix}/plugins/discover"),
                kit::link("可更新", "${prefix}/plugins/updates"),
            ],
        ),
        json!({ "type": "card", "content": { "body": [kit::text_line(format!(
            "已启用 {} 个 · 已停用 {} 个；启停与卸载只对管理员开放。",
            data.pointer("/enabled_count").and_then(Value::as_i64).unwrap_or(0),
            data.pointer("/disabled_count").and_then(Value::as_i64).unwrap_or(0)
        ))] } }),
    ];

    if tab == "discover" {
        body.push(kit::alert(
            "info",
            "发现",
            "上传已验签 .wbpkg 尚未原生渲染；商店安装已可用。旧版发现页不再作为入口。",
        ));
        return kit::page("插件中心", "发现", body);
    }
    if tab == "updates" {
        body.push(kit::alert(
            "info",
            "可更新",
            "内核尚未接入更新检查源；下方只列出目录已撤回/下架的已装版本。",
        ));
        for p in plugins
            .iter()
            .filter(|p| !text(p, "/takedown_note").is_empty())
        {
            body.push(plugin_card(p, false, false));
        }
        return kit::page("插件中心", "可更新", body);
    }

    if plugins.is_empty() {
        body.push(json!({ "type": "card", "content": { "body": [
            kit::text_line("还没有安装插件；到「发现」上传已验签的 .wbpkg。")
        ] } }));
    } else {
        for p in &plugins {
            body.push(plugin_card(p, can_admin, can_admin));
        }
    }
    kit::page("插件中心", "安装、授权、启停与更新", body)
}

fn plugin_card(p: &Value, can_admin: bool, include_delete: bool) -> Value {
    let mut lines = vec![
        format!(
            "标识：{} · 版本 {} · API {}",
            text(p, "/id"),
            text(p, "/version"),
            text(p, "/api")
        ),
        format!("状态：{}", text(p, "/status/label")),
        format!(
            "签名：{} · 沙箱：{}",
            if flag(p, "/signed") {
                "已验签"
            } else {
                "未验签"
            },
            if flag(p, "/reduced") {
                "降权"
            } else {
                "降权（要求完整，landlock/cgroups 未实现）"
            }
        ),
        format!("宿主能力：{}", or_none(text(p, "/requires"))),
        format!("动作权限：{}", or_none(text(p, "/permissions"))),
        format!("对象授权：{}", or_none(text(p, "/grants"))),
    ];
    let takedown = text(p, "/takedown_note");
    if !takedown.is_empty() {
        lines.push(format!("下游提示：{takedown}"));
    }
    let mut body = vec![
        kit::detail_lines(text(p, "/name"), lines),
        kit::link(
            "信息与管理页",
            format!("${{prefix}}/plugins/{}", enc(text(p, "/id"))),
        ),
    ];
    if can_admin {
        body.push(kit::flex(8, plugin_actions(p, include_delete, false)));
    }
    json!({ "type": "card", "content": { "body": body } })
}

/// 插件写操作：启停与卸载都由 `request` 声明；卸载后的落点按页面区分
/// （列表页重取本页，详情页回到插件中心）。
fn plugin_actions(p: &Value, include_delete: bool, detail: bool) -> Vec<Value> {
    let id = text(p, "/id");
    let enc_id = enc(id);
    let mut actions = vec![kit::action_button(
        if flag(p, "/enabled") {
            "停用插件"
        } else {
            "启用插件"
        },
        vec![
            json!({
                "type": "request",
                "props": {
                    "api": {
                        "method": "POST",
                        "url": format!("{API_BASE}/plugins/{enc_id}/toggle")
                    },
                    "messages": { "success": "插件状态已更新", "error": "插件启停失败" }
                }
            }),
            json!({ "type": "skiffReload" }),
        ],
        "tonal",
    )];
    if include_delete {
        for (label, delete_data, variant) in [
            ("卸载（保留数据）", false, "tonal"),
            ("卸载并删除数据", true, ""),
        ] {
            let content = if delete_data {
                format!("确认卸载 {id} 并删除其数据目录？此操作不可恢复。")
            } else {
                format!("确认卸载 {id}？数据目录会保留。")
            };
            let mut chain = vec![
                json!({
                    "type": "confirm",
                    "props": { "title": "卸载插件", "content": content, "color": "error" }
                }),
                json!({
                    "type": "request",
                    "props": {
                        "api": {
                            "method": "POST",
                            "url": format!("{API_BASE}/plugins/{enc_id}/delete"),
                            "data": { "delete_data": delete_data }
                        },
                        "messages": { "success": "插件已卸载", "error": "卸载失败" }
                    }
                }),
            ];
            chain.push(if detail {
                json!({ "type": "navigate", "props": { "url": "${prefix}/plugins/installed" } })
            } else {
                json!({ "type": "skiffReload" })
            });
            actions.push(kit::action_button(label, chain, variant));
        }
    }
    actions
}

/// 插件信息与管理页：身份、授权、接口、声明的功能页与管理员动作。
pub fn plugin_info_schema(data: &Value) -> Value {
    let p = data.get("plugin").cloned().unwrap_or(Value::Null);
    let mut body = vec![kit::detail_lines(
        "身份",
        vec![
            format!("标识：{}", text(&p, "/id")),
            format!(
                "名称：{} · 版本 {} · API {}",
                text(&p, "/name"),
                text(&p, "/version"),
                text(&p, "/api")
            ),
            format!("状态：{}", text(&p, "/status/label")),
            format!(
                "签名状态：{}",
                if flag(&p, "/signed") {
                    "已验签"
                } else {
                    "未验签"
                }
            ),
            format!(
                "密钥指纹：{}",
                if text(&p, "/key_id").is_empty() {
                    "未签名包"
                } else {
                    text(&p, "/key_id")
                }
            ),
            format!(
                "沙箱模式：{}",
                if flag(&p, "/reduced") {
                    "降权沙箱"
                } else {
                    "降权沙箱（要求完整沙箱；本版 landlock/cgroups 未实现）"
                }
            ),
            format!("声明能力面：{}", or_none(text(&p, "/requires"))),
            format!("动作权限：{}", or_none(text(&p, "/permissions"))),
        ],
    )];
    let takedown = text(&p, "/takedown_note");
    if !takedown.is_empty() {
        body.push(kit::alert("warning", "", takedown));
    }

    let grants: Vec<String> = data
        .get("grants")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|g| format!("{}:{}", text(g, "/kind"), text(g, "/value")))
                .collect()
        })
        .unwrap_or_default();
    body.push(kit::detail_lines(
        "对象授权",
        if grants.is_empty() {
            vec!["无对象授权".to_string()]
        } else {
            grants
        },
    ));

    let provides = list_text(data, "/interop_provides");
    let consumes = list_text(data, "/interop_consumes");
    body.push(kit::detail_lines(
        "插件间接口",
        vec![
            format!(
                "提供：{}",
                if provides.is_empty() {
                    "无".to_string()
                } else {
                    provides.join("；")
                }
            ),
            format!(
                "允许调用：{}",
                if consumes.is_empty() {
                    "无".to_string()
                } else {
                    consumes.join("；")
                }
            ),
        ],
    ));

    let menus = data
        .get("menus")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if menus.is_empty() {
        body.push(json!({ "type": "card", "content": { "body": [
            kit::text_line("该插件没有声明菜单或功能页。")
        ] } }));
    } else {
        let mut declared = Vec::new();
        for group in &menus {
            declared.push(text(group, "/label").to_string());
            if let Some(children) = group.get("children").and_then(Value::as_array) {
                for child in children {
                    declared.push(format!("　↳ {}", text(child, "/label")));
                }
            }
        }
        body.push(kit::detail_lines("声明的功能页", declared));
        body.push(kit::alert(
            "info",
            "功能页由随包 Schema 原生渲染",
            "已声明 schema 的功能页从左侧菜单进入，由壳页直接渲染；未声明 schema 的旧视图仍保留在旧 SSR 工作台（显式 ?legacy=1），不再作为面板内入口。",
        ));
    }

    if flag(data, "/can_admin") {
        body.push(kit::flex(8, plugin_actions(&p, true, true)));
    }
    body.push(kit::link("返回插件中心", "${prefix}/plugins/installed"));

    kit::page(
        format!("{} · 插件", text(&p, "/name")),
        text(&p, "/id"),
        body,
    )
}

fn or_none(value: &str) -> String {
    if value.is_empty() {
        "无".to_string()
    } else {
        value.to_string()
    }
}

fn enc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
