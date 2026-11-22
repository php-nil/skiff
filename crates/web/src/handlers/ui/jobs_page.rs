//! 作业中心与作业详情的 UI Schema（服务端产出）。
//!
//! 只读为主（取消/重试在详情页）：结构随 `can_cancel` / `can_retry` / 步骤与日志
//! 内容变化，静态资产表达不了，故由服务端出 JSON。取消 / 重试由声明式动作链完成。

use serde_json::{json, Value};

use super::schema_kit::{self, TableOpts};

/// 步骤结束时间的展示文案：数字按 epoch、字符串按 RFC3339 转本地时间。
///
/// 时区不可判定时原样显示，宁可露出原始值也不显示可能差一小时的时间
/// （与 `crate::util::fmt_local_time` 的口径一致）。
fn step_time(value: Option<&Value>) -> String {
    match value {
        Some(Value::Number(n)) => crate::util::fmt_local_time(n.as_i64(), "%Y-%m-%d %H:%M:%S", "—"),
        Some(Value::String(s)) if !s.is_empty() => chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|_| s.clone()),
        _ => String::new(),
    }
}

fn step_state(state: &str) -> &'static str {
    match state {
        "ok" => "完成",
        "running" => "进行中",
        "failed" => "失败",
        _ => "待执行",
    }
}

/// 作业列表页。
pub fn jobs_page_schema(prefix: &str) -> Value {
    let mut table = schema_kit::list_table(
        json!([
            { "key": "id", "label": "作业" },
            { "key": "title", "label": "标题" },
            { "key": "kind_label", "label": "类型" },
            { "key": "target_label", "label": "目标" },
            { "key": "status_label", "label": "状态" },
            { "key": "duration", "label": "耗时" },
            { "key": "created_by", "label": "提交人" },
            { "key": "created_at", "label": "创建时间" }
        ]),
        "/jobs",
        "id",
        json!({}),
        TableOpts {
            empty_text: Some("还没有长耗时任务，安装运行环境等操作会在这里显示进度".to_string()),
            ..TableOpts::default()
        },
    );
    table["onEvent"] = json!({
        "rowClick": {
            "actions": [{ "type": "navigate", "props": { "url": format!("{prefix}/jobs/${{id}}") } }]
        }
    });
    schema_kit::page("作业中心", "列表只读；取消与重试在作业详情页", vec![table])
}

/// 作业详情页：信息 / 参数 / 步骤 / 结果 / 日志尾巴 + 可用操作。
pub fn job_detail_schema(data: &Value, prefix: &str) -> Value {
    let text = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let flag = |key: &str| data.get(key).and_then(Value::as_bool).unwrap_or(false);
    let dash = |value: String| {
        if value.is_empty() {
            "—".to_string()
        } else {
            value
        }
    };
    let id = text("id");
    let title = text("title");
    let phase = text("phase_label");
    let mut body = vec![
        schema_kit::detail_lines(
            "作业信息",
            vec![
                format!("标题：{title}（{id}）"),
                format!(
                    "类型：{} · 目标：{}",
                    text("kind_label"),
                    text("target_label")
                ),
                format!(
                    "状态：{}{}",
                    text("status_label"),
                    if phase.is_empty() {
                        String::new()
                    } else {
                        format!(" · {phase}")
                    }
                ),
                format!(
                    "进度：{}%{}",
                    data.get("progress").and_then(Value::as_i64).unwrap_or(0),
                    if flag("indeterminate") {
                        "（不确定）"
                    } else {
                        ""
                    }
                ),
                format!(
                    "创建：{} · 提交人：{}",
                    dash(text("created_at")),
                    text("created_by")
                ),
                format!("开始：{}", dash(text("started_at"))),
                format!("结束：{}", dash(text("finished_at"))),
                format!(
                    "耗时：{} · 退出码：{}",
                    dash(text("duration")),
                    dash(
                        data.get("exit_code")
                            .map(|v| v.to_string())
                            .unwrap_or_default()
                    )
                ),
                format!("触发：{}", dash(text("trigger"))),
                format!(
                    "错误：{}",
                    dash(match (text("error_class"), text("error_detail")) {
                        (class, detail) if class.is_empty() && detail.is_empty() => String::new(),
                        (class, detail) => format!("{class} {detail}").trim().to_string(),
                    })
                ),
            ],
        ),
        schema_kit::detail_lines(
            "参数",
            vec![format!(
                "params = {}",
                data.get("params")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "null".to_string())
            )],
        ),
    ];
    let steps: Vec<String> = data
        .get("steps")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|s| {
                    let name = s.get("name").and_then(Value::as_str).unwrap_or("");
                    let state = step_state(s.get("state").and_then(Value::as_str).unwrap_or(""));
                    let detail = s.get("detail").and_then(Value::as_str).unwrap_or("");
                    let finished = step_time(s.get("finished_at"));
                    format!(
                        "· {name}：{state}{}{}",
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!("（{detail}）")
                        },
                        if finished.is_empty() {
                            String::new()
                        } else {
                            format!(" · 结束 {finished}")
                        }
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if !steps.is_empty() {
        body.push(schema_kit::detail_lines("步骤", steps));
    }
    body.push(schema_kit::detail_lines(
        "结果",
        vec![format!(
            "result = {}",
            data.get("result")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".to_string())
        )],
    ));
    let logs = text("logs");
    let log_lines: Vec<String> = logs
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect();
    body.push(json!({
        "type": "card",
        "props": { "title": format!("日志（首屏尾巴 {} 行）", log_lines.len()) },
        "content": {
            "body": if log_lines.is_empty() {
                vec![schema_kit::text_line("暂无日志")]
            } else {
                vec![json!({
                    "type": "code",
                    "props": {
                        "modelValue": log_lines.join("\n"),
                        "language": "plaintext",
                        "readonly": true,
                        "height": 320,
                        "lineNumbers": true,
                        "wrap": false,
                        "copyable": true,
                        "ariaLabel": "作业日志（首屏）"
                    }
                })]
            }
        }
    }));
    body.push(json!({
        "type": "flex",
        "props": { "gap": 12, "wrap": true },
        "content": {
            "body": [{
                "type": "link",
                "props": {
                    "text": "查看完整日志（按需加载）",
                    "href": format!("{prefix}/jobs/{}/logs", crate::util::encode_uri_component(&id))
                }
            }]
        }
    }));
    let can_admin = flag("can_admin");
    let can_cancel = flag("can_cancel");
    let can_retry = flag("can_retry");
    if can_admin && (can_cancel || can_retry) {
        let mut actions = Vec::new();
        if can_cancel {
            actions.push(json!({
                "type": "button",
                "props": {
                    "text": if flag("cancel_requested") { "已请求取消" } else { "请求取消" },
                    "color": "warning"
                },
                "onEvent": {
                    "click": {
                        "actions": [
                            {
                                "type": "confirm",
                                "props": {
                                    "title": "取消作业",
                                    "content": "确认请求取消该作业？已在执行中的步骤可能仍需等待收尾。"
                                }
                            },
                            {
                                "type": "request",
                                "props": {
                                    "api": {
                                        "method": "POST",
                                        "url": format!("{}/jobs/{}/cancel", schema_kit::API_BASE, crate::util::encode_uri_component(&id))
                                    },
                                    "messages": { "success": "已请求取消", "error": "取消作业失败" }
                                }
                            },
                            { "type": "skiffReload" }
                        ]
                    }
                }
            }));
        }
        if can_retry {
            actions.push(json!({
                "type": "button",
                "props": { "text": "重试作业", "color": "primary" },
                "onEvent": {
                    "click": {
                        "actions": [
                            {
                                "type": "request",
                                "props": {
                                    "api": {
                                        "method": "POST",
                                        "url": format!("{}/jobs/{}/retry", schema_kit::API_BASE, crate::util::encode_uri_component(&id))
                                    },
                                    "assign": "job",
                                    "messages": { "success": "已创建重试作业", "error": "重试失败" }
                                }
                            },
                            {
                                "type": "navigate",
                                "props": { "url": "${prefix}/jobs/${job.data.id}" }
                            }
                        ]
                    }
                }
            }));
        }
        body.push(json!({
            "type": "card",
            "props": { "title": "作业操作" },
            "content": {
                "body": [{ "type": "flex", "props": { "gap": 8, "wrap": true }, "content": { "body": actions } }]
            }
        }));
    } else if can_admin {
        body.push(schema_kit::text_line(
            "当前状态下没有可用的取消或重试操作。",
        ));
    } else {
        body.push(json!({
            "type": "alert",
            "props": { "type": "info", "text": "取消与重试需要管理员权限。" }
        }));
    }
    schema_kit::page(format!("{title} · 作业"), "只读详情", body)
}

/// 作业日志页：聚合后的只读文本 + 返回 / 刷新入口。
///
/// 数据面（按 `after` 游标分批拉取、最多 10 页）在服务端完成，前端只渲染；
/// 刷新走宿主动作 `skiffReload`（重新拉取本页 Schema）。
pub fn job_logs_schema(
    job_id: &str,
    prefix: &str,
    lines: &[String],
    status_label: &str,
    truncated: bool,
) -> Value {
    let mut body = vec![schema_kit::flex(
        8,
        vec![
            schema_kit::link(
                "返回作业详情",
                format!(
                    "{prefix}/jobs/{}",
                    crate::util::encode_uri_component(job_id)
                ),
            ),
            schema_kit::action_button("刷新", json!({ "type": "skiffReload" }), "text"),
        ],
    )];
    if lines.is_empty() {
        body.push(schema_kit::alert("info", "", "暂无日志。"));
    } else {
        let mut viewer = schema_kit::code_block(&lines.join("\n"), 560);
        viewer["props"]["ariaLabel"] = json!("作业日志");
        body.push(viewer);
    }
    let status = if status_label.is_empty() {
        "—"
    } else {
        status_label
    };
    let mut summary = format!("共 {} 行 · 状态 {status}", lines.len());
    if truncated {
        summary.push_str(" · 仅加载前 10 页，可刷新继续");
    }
    body.push(schema_kit::small_text(summary));
    schema_kit::page(
        format!("作业日志 · {job_id}"),
        "按需加载（不做 SSE 实时流）",
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(overrides: Value) -> Value {
        let mut data = json!({
            "id": "j1",
            "title": "安装 PHP 8.3",
            "kind_label": "运行环境",
            "target_label": "php83",
            "status_label": "执行中",
            "progress": 40,
            "indeterminate": false,
            "created_by": "admin",
            "created_at": "2026-11-22T00:00:00Z",
            "duration": "",
            "exit_code": null,
            "trigger": "手动",
            "error_class": "",
            "error_detail": "",
            "params": {},
            "steps": [],
            "result": null,
            "logs": "",
            "can_admin": true,
            "can_cancel": false,
            "can_retry": false
        });
        if let (Some(target), Some(src)) = (data.as_object_mut(), overrides.as_object()) {
            for (k, v) in src {
                target.insert(k.clone(), v.clone());
            }
        }
        data
    }

    /// 只给「允许的那一个」操作：`can_retry` 与 `can_cancel` 各自独立。
    #[test]
    fn detail_shows_only_allowed_actions() {
        let retry = job_detail_schema(&detail(json!({ "can_retry": true })), "/p").to_string();
        assert!(retry.contains("重试作业"));
        assert!(!retry.contains("请求取消"));
        let cancel = job_detail_schema(&detail(json!({ "can_cancel": true })), "/p").to_string();
        assert!(cancel.contains("请求取消"));
        assert!(!cancel.contains("重试作业"));
    }

    /// viewer 既看不到操作卡，也不该看到「当前状态没有可用操作」这类管理员视角文案。
    #[test]
    fn detail_tells_viewer_it_needs_admin() {
        let schema = job_detail_schema(&detail(json!({ "can_admin": false })), "/p").to_string();
        assert!(schema.contains("需要管理员权限"));
        assert!(!schema.contains("作业操作"));
    }

    /// 步骤文案在服务端成形：状态中文化、结束时间补 `· 结束`。
    #[test]
    fn steps_are_formatted_on_the_server() {
        let schema = job_detail_schema(
            &detail(json!({
                "steps": [{ "name": "拉镜像", "state": "ok", "detail": "12s" }]
            })),
            "/p",
        )
        .to_string();
        assert!(schema.contains("· 拉镜像：完成（12s）"));
    }
}
