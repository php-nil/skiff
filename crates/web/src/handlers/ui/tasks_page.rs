//! 定时任务页的 UI Schema（服务端产出）。
//!
//! 列表与详情都由服务端出 JSON：结构随 `can_admin`、任务状态与执行记录变化，
//! 静态资产表达不了。写操作仍走宿主的 `skiffTaskWrite`，权限与审计口径不变。

use serde_json::{json, Value};

use super::schema_kit::{self, TableOpts};
use super::{TaskItem, TaskRunItem};

fn or_dash(text: &str) -> &str {
    if text.is_empty() {
        "—"
    } else {
        text
    }
}

/// 行内按钮（`rowActions` 的元素是完整节点，不是描述对象）。
fn row_button(text: &str, op: &str) -> Value {
    json!({
        "type": "button",
        "props": { "text": text, "variant": "text", "color": "primary" },
        "onEvent": {
            "click": {
                "actions": [{
                    "type": "skiffTaskWrite",
                    "props": { "op": op, "id": "${id}", "enabled": "${enabled}", "name": "${name}" }
                }]
            }
        }
    })
}

/// 任务列表页。
pub fn tasks_page_schema(can_admin: bool, keyword: &str, prefix: &str) -> Value {
    let base = format!("{prefix}/tasks");
    let mut body = vec![schema_kit::search_form(
        "关键字（任务 / id / 目标 / 来源）",
        keyword,
        &base,
        false,
    )];
    if !keyword.is_empty() {
        // 清除筛选必须整页跳转：内页 link 只改地址栏，结果不会重新取。
        body.push(json!({
            "type": "flex",
            "props": { "gap": 12, "wrap": true, "align": "center" },
            "content": {
                "body": [
                    {
                        "type": "button",
                        "props": { "text": "清除筛选", "variant": "text" },
                        "action": { "type": "skiffSearch", "props": { "base": base, "q": "" } }
                    },
                    { "type": "text", "props": { "text": format!("筛选条件：{keyword}"), "size": "small" } }
                ]
            }
        }));
    }
    let empty_text = if keyword.is_empty() {
        "还没有计划任务，站点详情页可开启每日备份".to_string()
    } else {
        format!("没有匹配「{keyword}」的任务")
    };
    let mut table = schema_kit::list_table(
        json!([
            { "key": "name", "label": "任务" },
            { "key": "method", "label": "方式" },
            { "key": "target", "label": "目标" },
            { "key": "expr", "label": "调度" },
            { "key": "next_text", "label": "下次运行" },
            { "key": "state_label", "label": "状态" },
            { "key": "last_text", "label": "最近结果" }
        ]),
        "/tasks",
        "id",
        json!({ "q": keyword }),
        TableOpts {
            empty_text: Some(empty_text),
            ..TableOpts::default()
        },
    );
    table["onEvent"] = json!({
        "rowClick": {
            "actions": [{ "type": "navigate", "props": { "url": format!("{prefix}/tasks/${{id}}") } }]
        }
    });
    if can_admin {
        table["props"]["rowActions"] = json!([
            row_button("切换启停", "toggle"),
            row_button("立即运行", "run")
        ]);
    }
    body.push(table);
    if !can_admin {
        body.push(json!({
            "type": "alert",
            "props": { "type": "info", "text": "启停、运行与删除需要管理员权限。" }
        }));
    }
    schema_kit::page("定时任务", "启停与运行结果均回写既有任务审计", body)
}

/// 任务详情页（含最近执行记录）。
pub fn task_detail_schema(
    task: &TaskItem,
    payload: &Value,
    runs: &[TaskRunItem],
    can_admin: bool,
) -> Value {
    let mut body = vec![
        schema_kit::detail_lines(
            "任务信息",
            vec![
                format!("名称：{}（{}）", task.name, task.id),
                format!("方式：{} · 目标：{}", task.method, task.target),
                format!("调度：{}", task.expr),
                format!("下次运行：{}", or_dash(&task.next_text)),
                format!("状态：{}", task.state_label),
                format!("最近结果：{}", or_dash(&task.last_text)),
                format!(
                    "来源：{} · 内置：{}",
                    or_dash(&task.source),
                    if task.builtin { "是" } else { "否" }
                ),
            ],
        ),
        schema_kit::detail_lines(
            "参数",
            vec![format!(
                "payload = {}",
                serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
            )],
        ),
        json!({
            "type": "card",
            "props": { "title": format!("最近执行（{}）", runs.len()) },
            "content": {
                "body": [schema_kit::table(
                    json!([
                        { "key": "started_text", "label": "开始" },
                        { "key": "trigger", "label": "触发" },
                        { "key": "duration", "label": "耗时" },
                        { "key": "result", "label": "结果" },
                        { "key": "detail", "label": "说明" }
                    ]),
                    json!(runs),
                    "started_text",
                    TableOpts {
                        empty_text: Some("还没有运行记录".to_string()),
                        ..TableOpts::default()
                    },
                )]
            }
        }),
    ];
    if can_admin {
        let mut actions = vec![
            json!({
                "type": "button",
                "props": {
                    "text": if task.enabled { "停用任务" } else { "启用任务" },
                    "color": "secondary"
                },
                "onEvent": {
                    "click": {
                        "actions": [{
                            "type": "skiffTaskWrite",
                            "props": { "op": "toggle", "id": task.id, "enabled": !task.enabled }
                        }]
                    }
                }
            }),
            json!({
                "type": "button",
                "props": { "text": "立即运行", "color": "primary" },
                "onEvent": {
                    "click": {
                        "actions": [{ "type": "skiffTaskWrite", "props": { "op": "run", "id": task.id } }]
                    }
                }
            }),
        ];
        if !task.builtin {
            actions.push(json!({
                "type": "button",
                "props": { "text": "删除任务", "color": "danger", "variant": "text" },
                "onEvent": {
                    "click": {
                        "actions": [{
                            "type": "skiffTaskWrite",
                            "props": { "op": "delete", "id": task.id, "name": task.name }
                        }]
                    }
                }
            }));
        }
        body.push(json!({
            "type": "card",
            "props": { "title": "任务操作" },
            "content": {
                "body": [{ "type": "flex", "props": { "gap": 8, "wrap": true }, "content": { "body": actions } }]
            }
        }));
    } else {
        body.push(json!({
            "type": "alert",
            "props": { "type": "info", "text": "写操作需要管理员权限；当前为只读视图。" }
        }));
    }
    schema_kit::page(
        format!("{} · 定时任务", task.name),
        "启停、运行与删除均走 /ui/v1",
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(builtin: bool) -> TaskItem {
        TaskItem {
            id: "t1".into(),
            name: "每日备份".into(),
            expr: "0 3 * * *".into(),
            next_text: "明天 03:00".into(),
            source: "内置".into(),
            target: "站点 a.com".into(),
            method: "shell".into(),
            enabled: true,
            state_label: "启用".into(),
            state_class: "ok".into(),
            last_text: "成功".into(),
            last_class: "ok".into(),
            builtin,
        }
    }

    /// viewer 不得拿到行操作：入口由服务端裁掉，不依赖前端隐藏。
    #[test]
    fn list_hides_row_actions_for_viewer() {
        let schema = tasks_page_schema(false, "", "/p");
        let table = &schema["body"][1];
        assert!(table["props"].get("rowActions").is_none());
        assert!(
            schema.to_string().contains("需要管理员权限"),
            "viewer 应看到只读提示"
        );
    }

    /// 关键字必须同时进骨架与表格数据源，否则筛选后的骨架与数据不一致。
    #[test]
    fn list_keeps_keyword_in_source_params() {
        let schema = tasks_page_schema(true, "backup", "/p");
        let table = &schema["body"][2];
        assert_eq!(table["source"]["params"]["q"], "backup");
        assert_eq!(table["props"]["rowActions"].as_array().unwrap().len(), 2);
        assert!(schema.to_string().contains("没有匹配「backup」的任务"));
    }

    /// 内置任务不给删除入口（后端也会拒绝），自建任务才给。
    #[test]
    fn detail_offers_delete_only_for_custom_task() {
        let builtin = task_detail_schema(&task(true), &json!({}), &[], true).to_string();
        assert!(!builtin.contains("删除任务"));
        assert!(builtin.contains("立即运行"));
        let custom = task_detail_schema(&task(false), &json!({}), &[], true).to_string();
        assert!(custom.contains("删除任务"));
    }
}
