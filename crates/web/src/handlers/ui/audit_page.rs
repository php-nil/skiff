//! 审计页的 UI Schema（服务端产出）。
//!
//! 只读页：布局随关键字与游标变化，故由服务端出 JSON；权限由 handler 的
//! `AuditRead` 前置校验，本模块不判定权限。

use serde_json::{json, Value};

use super::schema_kit::{self, TableOpts};
use super::AuditItem;

/// 审计列表页。
///
/// `items` 是本页已取到的记录（一次给全，不走组件游标）；翻页仍由整页跳转完成，
/// 因为审计游标与筛选条件指纹绑定，换页必须回到服务端重新签名。
pub fn audit_page_schema(
    items: &[AuditItem],
    keyword: &str,
    has_more: bool,
    next_cursor: Option<&str>,
    prefix: &str,
) -> Value {
    let rows: Vec<Value> = items
        .iter()
        .map(|a| {
            json!({
                "key": a.id.to_string(),
                "ts_label": a.ts_label,
                "actor": a.actor,
                "ip": a.ip,
                "action": a.action,
                "target": a.target,
                "result": a.result,
                "detail": a.detail
            })
        })
        .collect();
    let base = format!("{prefix}/settings/audit");
    let mut body = vec![
        schema_kit::search_form(
            "关键字（操作者 / IP / 动作 / 对象 / 详情）",
            keyword,
            &base,
            true,
        ),
        schema_kit::table(
            json!([
                { "key": "ts_label", "label": "时间", "width": 180 },
                { "key": "actor", "label": "操作者", "width": 100 },
                { "key": "ip", "label": "IP", "width": 130 },
                { "key": "action", "label": "动作", "width": 190 },
                { "key": "target", "label": "对象", "width": 170 },
                { "key": "result", "label": "结果", "width": 100, "align": "center" }
            ]),
            json!(rows),
            "key",
            TableOpts {
                empty_text: Some("暂无审计记录".to_string()),
                height: Some(420),
                // 原始 JSON 挂展开行：放单元格会把列宽挤坏。
                expand: Some(json!({
                    "type": "code",
                    "props": {
                        "modelValue": "${detail}",
                        "language": "json",
                        "readonly": true,
                        "lineNumbers": true,
                        "maxHeight": 220
                    }
                })),
            },
        ),
    ];
    if has_more {
        if let Some(cursor) = next_cursor {
            body.push(json!({
                "type": "button",
                "props": { "text": "下一页（更早记录）", "variant": "tonal" },
                "onEvent": {
                    "click": {
                        "actions": [{
                            "type": "skiffSearch",
                            "props": { "base": base, "q": keyword, "cursor": cursor }
                        }]
                    }
                }
            }));
        }
    }
    let tail = if has_more {
        "，还有更早记录。"
    } else {
        "，已到末尾。"
    };
    body.push(schema_kit::text_line(format!(
        "本页 {} 条{tail}{}",
        items.len(),
        if keyword.is_empty() {
            String::new()
        } else {
            format!("关键字：{keyword}")
        }
    )));
    schema_kit::page("审计日志", "按时间倒序检索，本页只读", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64) -> AuditItem {
        AuditItem {
            id,
            ts: "2026-11-22T00:00:00Z".into(),
            ts_label: "2026-11-22 08:00:00".into(),
            actor: "admin".into(),
            ip: "10.0.0.1".into(),
            action: "site.create".into(),
            target: "a.com".into(),
            result: "ok".into(),
            detail: "{}".into(),
        }
    }

    /// 「下一页」只在本页之外还有记录时出现，且带上游标继续走同一个搜索动作。
    #[test]
    fn next_button_only_when_more() {
        let last = audit_page_schema(&[item(2)], "", false, None, "/p").to_string();
        assert!(!last.contains("下一页"));
        assert!(last.contains("已到末尾"));
        let more = audit_page_schema(&[item(2)], "", true, Some("c1"), "/p").to_string();
        assert!(more.contains("下一页（更早记录）"));
        assert!(more.contains("\"cursor\":\"c1\""));
    }

    /// 行数据用服务端格式化好的 `ts_label`，空值由服务端给 `—`。
    #[test]
    fn rows_carry_server_side_time_label() {
        let schema = audit_page_schema(&[item(7)], "", false, None, "/p");
        let row = &schema["body"][1]["props"]["items"][0];
        assert_eq!(row["ts_label"], "2026-11-22 08:00:00");
        assert_eq!(row["key"], "7");
    }

    /// 关键字为空时不写 `q` 参数，避免把 `?q=` 空串带进地址栏。
    #[test]
    fn next_button_keeps_keyword() {
        let schema = audit_page_schema(&[item(1)], "a.com", true, Some("c2"), "/p").to_string();
        assert!(schema.contains("\"q\":\"a.com\""));
        assert!(schema.contains("关键字：a.com"));
    }
}
