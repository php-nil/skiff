//! 站点详情页 · 配置 tab。
//!
//! 伪静态 + 出口限速 + vhost 配置读写。

use serde_json::{json, Value};

use super::super::schema_kit::{self as kit, text_line, write_card};
use super::*;

pub fn section(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    let is_php = site.site_type == "runtime"
        && data.runtime.as_ref().map(|r| r.kind.code.as_str()) == Some("php");
    let rewrite = if site.rewrite.is_empty() {
        "none"
    } else {
        site.rewrite.as_str()
    };
    let limit_rate = site.limit_rate_kbps.to_string();

    let mut out = Vec::new();
    if data.can_manage {
        let mut fields = Vec::new();
        if is_php {
            fields.push(json!({
                "type": "select",
                "props": {
                    "name": "rewrite",
                    "label": "伪静态",
                    "items": [
                        { "title": "无（默认）", "value": "none" },
                        { "title": "WordPress", "value": "wordpress" },
                        { "title": "Laravel", "value": "laravel" },
                        { "title": "ThinkPHP", "value": "thinkphp" }
                    ],
                    "defaultValue": rewrite
                }
            }));
        }
        fields.push(json!({
            "type": "input-text",
            "props": {
                "name": "limit_rate_kbps",
                "label": "出口限速（KB/s，0 表示不限制）",
                "defaultValue": limit_rate
            }
        }));
        out.push(write_card(
            "伪静态与流量限制",
            if is_php {
                "PHP 伪静态预设与 nginx limit_rate；保存后重生成 vhost"
            } else {
                "nginx limit_rate；保存后重生成 vhost"
            },
            kit::form(
                site_write_action(json!({ "op": "policy", "id": site.id, "domain": site.domain })),
                fields,
                "保存策略",
                Some(json!({ "rewrite": rewrite, "limit_rate_kbps": limit_rate })),
            ),
        ));
    } else {
        out.push(kit::card(
            "伪静态与流量限制",
            vec![
                text_line(format!("伪静态：{rewrite}")),
                text_line(format!(
                    "出口限速：{}",
                    if site.limit_rate_kbps == 0 {
                        "不限制".to_string()
                    } else {
                        format!("{} KB/s", site.limit_rate_kbps)
                    }
                )),
            ],
        ));
    }

    if !data.config.error.is_empty() {
        out.push(kit::alert(
            "warning",
            "配置读取失败",
            data.config.error.clone(),
        ));
    } else {
        let path = if data.config.path.is_empty() {
            "—"
        } else {
            data.config.path.as_str()
        };
        let code_props = json!({
            "language": "nginx",
            "className": "skiff-code-no-lang",
            "modelValue": data.config.content
        });
        if data.can_manage {
            let mut props = code_props;
            props["name"] = json!("content");
            props["readonly"] = json!(false);
            props["height"] = json!(420);
            props["lineNumbers"] = json!(true);
            props["wrap"] = json!(false);
            props["copyable"] = json!(true);
            props["ariaLabel"] = json!("vhost 配置内容");
            props["rules"] = json!({
                "required": true,
                "messages": { "required": "配置内容不能为空" }
            });
            out.push(write_card(
                "vhost 配置",
                &format!(
                    "{path}{}",
                    if data.config.active {
                        " · 已启用"
                    } else {
                        " · 未启用"
                    }
                ),
                kit::form(
                    site_write_action(
                        json!({ "op": "config", "id": site.id, "domain": site.domain }),
                    ),
                    vec![json!({
                        "type": "code",
                        "className": "skiff-code-no-lang",
                        "props": props
                    })],
                    "保存配置",
                    None,
                ),
            ));
        } else {
            out.push(write_card(
                "vhost 配置",
                &format!("{path}（只读）"),
                json!({
                    "type": "code",
                    "className": "skiff-code-no-lang",
                    "props": code_props
                }),
            ));
        }
    }
    out
}
