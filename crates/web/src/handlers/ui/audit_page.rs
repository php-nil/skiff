//! 审计页的 UI Schema（服务端产出）。
//!
//! 只读页：布局随关键字与游标变化，故由服务端出 JSON；权限由 handler 的
//! `AuditRead` 前置校验，本模块不判定权限。

use serde_json::{json, Value};
use skiff_model::AuditCategory;

use super::schema_kit::{self, TableOpts};
use super::AuditItem;

/// 关键字 + 分类的筛选表单。
///
/// 提交与翻页都走宿主的 `skiffSearch`：它读表单里的 `q` 与 `cat`，整页跳转，
/// 保证地址栏、筛选条件与已取结果三者一致。
fn filter_form(keyword: &str, category: Option<AuditCategory>, base: &str) -> Value {
    let mut items: Vec<(&str, &str)> = vec![("全部分类", "")];
    items.extend(AuditCategory::ALL.iter().map(|c| (c.label(), c.key())));
    let selected = category.map(|c| c.key()).unwrap_or("");
    json!({
        "type": "form",
        "onEvent": {
            "submit": [{
                "type": "skiffSearch",
                "props": { "base": base }
            }]
        },
        "props": {
            "actionsAlign": "end",
            // 提交只跑 `onEvent.submit` 的动作链，组件自身不发请求（同 `search_form`）。
            "submitMode": "event"
        },
        "body": [
            {
                "type": "input-text",
                "props": {
                    "name": "q",
                    "label": "关键字（操作者 / IP / 动作 / 对象 / 详情）",
                    "defaultValue": keyword
                }
            },
            schema_kit::select("cat", "分类", selected, items)
        ],
        "content": {
            "actions": [
                {
                    "type": "button",
                    "props": { "text": "重置", "variant": "text" },
                    "action": {
                        "type": "skiffSearch",
                        "props": { "base": base, "q": "", "cat": "" }
                    }
                },
                {
                    "type": "button",
                    "props": { "text": "查询", "variant": "tonal" },
                    "action": { "type": "submit" }
                }
            ]
        }
    })
}

/// 审计列表页。
///
/// `items` 是本页已取到的记录（一次给全，不走组件游标）；翻页仍由整页跳转完成，
/// 因为审计游标与筛选条件指纹绑定，换页必须回到服务端重新签名。
pub fn audit_page_schema(
    items: &[AuditItem],
    keyword: &str,
    category: Option<AuditCategory>,
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
                "category_label": a.category_label,
                "actor": a.actor,
                "ip": a.ip,
                "action": a.action,
                "target": a.target,
                "result": a.result,
                "detail": a.detail
            })
        })
        .collect();
    let base = format!("{prefix}/tasks/audit");
    let category_key = category.map(|c| c.key()).unwrap_or("");
    let mut body = vec![
        filter_form(keyword, category, &base),
        schema_kit::table(
            json!([
                { "key": "ts_label", "label": "时间", "width": 180 },
                { "key": "category_label", "label": "分类", "width": 110 },
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
                            "props": { "base": base, "q": keyword, "cat": category_key, "cursor": cursor }
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
    let filters = {
        let mut parts = Vec::new();
        if !keyword.is_empty() {
            parts.push(format!("关键字：{keyword}"));
        }
        if let Some(category) = category {
            parts.push(format!("分类：{}", category.label()));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("，筛选条件：{}", parts.join("；"))
        }
    };
    body.push(schema_kit::text_line(format!(
        "本页 {} 条{tail}{filters}",
        items.len()
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
            category: "site".into(),
            category_label: "站点".into(),
            actor: "admin".into(),
            ip: "10.0.0.1".into(),
            action: "site.create".into(),
            target: "a.com".into(),
            result: "ok".into(),
            detail: "{}".into(),
        }
    }

    /// 「下一页」只在本页之外还有记录时出现，且带上游标与当前分类继续走同一个搜索动作。
    #[test]
    fn next_button_only_when_more() {
        let last = audit_page_schema(&[item(2)], "", None, false, None, "/p").to_string();
        assert!(!last.contains("下一页"));
        assert!(last.contains("已到末尾"));
        let more = audit_page_schema(
            &[item(2)],
            "",
            Some(AuditCategory::Site),
            true,
            Some("c1"),
            "/p",
        )
        .to_string();
        assert!(more.contains("下一页（更早记录）"));
        assert!(more.contains("\"cursor\":\"c1\""));
        assert!(more.contains("\"cat\":\"site\""), "翻页必须保留分类");
    }

    /// 行数据用服务端格式化好的 `ts_label`，空值由服务端给 `—`。
    #[test]
    fn rows_carry_server_side_time_label() {
        let schema = audit_page_schema(&[item(7)], "", None, false, None, "/p");
        let row = &schema["body"][1]["props"]["items"][0];
        assert_eq!(row["ts_label"], "2026-11-22 08:00:00");
        assert_eq!(row["category_label"], "站点");
        assert_eq!(row["key"], "7");
    }

    /// 关键与分类都为空的跳转不带参数；分类选中时下拉默认值同步。
    #[test]
    fn filter_form_carries_keyword_and_category() {
        let schema = audit_page_schema(
            &[item(1)],
            "a.com",
            Some(AuditCategory::Task),
            true,
            Some("c2"),
            "/p",
        )
        .to_string();
        assert!(schema.contains("\"q\":\"a.com\""));
        assert!(schema.contains("\"cat\":\"task\""));
        assert!(schema.contains("分类：任务"));
        assert!(schema.contains("\"defaultValue\":\"task\""));
        assert!(schema.contains("全部分类"));
    }
}
