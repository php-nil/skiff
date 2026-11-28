//! 站点详情页 · 备份 tab（备份计划 + 归档恢复）。

use serde_json::{json, Value};

use super::super::schema_kit::{self as kit, or_dash, text_line};
use super::*;

pub fn section(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    let backup = &data.backup;
    let mut body = Vec::new();
    if !backup.available {
        body.push(kit::alert(
            "warning",
            "",
            "内核任务服务未初始化，站点备份暂不可用。",
        ));
    } else {
        let interval = if backup.interval.is_empty() {
            "daily"
        } else {
            backup.interval.as_str()
        };
        let time = if backup.time.is_empty() {
            "03:00"
        } else {
            backup.time.as_str()
        };
        let interval_label = match interval {
            "daily" => "每天",
            "weekly" => "每周一",
            "monthly" => "每月 1 日",
            other => other,
        };
        body.push(text_line(if backup.enabled {
            format!("备份已开启（{interval_label} {time}）")
        } else {
            "备份未开启".to_string()
        }));
        body.push(text_line(format!(
            "上次：{}",
            if backup.last_text.is_empty() {
                "从未执行"
            } else {
                backup.last_text.as_str()
            }
        )));
        body.push(text_line(format!("下次：{}", or_dash(&backup.next_text))));
        if data.can_manage {
            body.push(site_btn(
                if backup.enabled {
                    "关闭备份"
                } else {
                    "开启备份"
                },
                json!({
                    "op": "backup",
                    "enabled": !backup.enabled,
                    "id": site.id,
                    "domain": site.domain,
                    "interval": interval,
                    "time": time
                }),
                if backup.enabled { "warning" } else { "primary" },
                None,
            ));
            body.push(kit::form(
                site_write_action(json!({
                    "op": "backup",
                    "id": site.id,
                    "domain": site.domain,
                    "enabled": backup.enabled
                })),
                vec![
                    json!({
                        "type": "select",
                        "props": {
                            "name": "interval",
                            "label": "备份间隔",
                            "items": [
                                { "title": "每天", "value": "daily" },
                                { "title": "每周一", "value": "weekly" },
                                { "title": "每月 1 日", "value": "monthly" }
                            ],
                            "defaultValue": interval
                        }
                    }),
                    json!({
                        "type": "input-text",
                        "props": {
                            "name": "time",
                            "label": "执行时间（HH:MM）",
                            "defaultValue": time,
                            "rules": {
                                "pattern": "^([01]\\d|2[0-3]):[0-5]\\d$",
                                "messages": { "pattern": "时间格式应为 HH:MM（例如 03:00）" }
                            }
                        }
                    }),
                ],
                "保存备份计划",
                Some(json!({ "interval": interval, "time": time })),
            ));
        }
    }
    vec![kit::card("站点备份", body), archive_card(data)]
}

/// 归档与恢复：列出已有 `.tar`，逐个给「恢复」入口。
///
/// 恢复是覆盖式动作，确认框要求手输域名——与删除站点同一档；执行体在任务服务里
/// 先预检、再打恢复前快照、最后解包，所以这里不需要再让用户选「要不要备份」。
fn archive_card(data: &SiteDetailData) -> Value {
    let site = &data.site;
    let mut rows = Vec::new();
    if data.backup.archives.is_empty() {
        rows.push(text_line(
            "还没有归档。开启备份计划后由调度器生成；恢复前建议先有一份较新的归档。",
        ));
    } else {
        rows.push(text_line(format!(
            "可用归档（最近 {} 份，按时间倒序）",
            data.backup.archives.len()
        )));
        for archive in &data.backup.archives {
            let mut line = vec![
                json!({ "type": "text", "props": { "text": archive.name, "weight": "medium" } }),
                json!({ "type": "text", "props": { "text": archive.size_text } }),
                json!({ "type": "text", "props": { "text": archive.modified_text } }),
            ];
            if data.can_manage {
                line.push(json!({
                    "type": "button",
                    "props": { "text": "恢复", "color": "danger", "variant": "text" },
                    "onEvent": { "click": { "actions": [
                        {
                            "type": "confirm",
                            "props": {
                                "title": "从归档恢复",
                                "content": format!(
                                    "会用 {} 覆盖站点目录：同名文件被替换，归档里没有的文件保留。恢复前会自动打一份快照，过程在后台执行，进度与结果见任务中心。",
                                    archive.name
                                ),
                                "color": "error",
                                "confirmText": "确认恢复",
                                "requireText": site.domain
                            }
                        },
                        site_write_action(json!({
                            "op": "restore",
                            "id": site.id,
                            "domain": site.domain,
                            "archive": archive.name
                        }))
                    ] } }
                }));
            }
            rows.push(kit::flex(json!({ "gap": 12, "align": "center" }), line));
        }
    }
    kit::card("归档与恢复", rows)
}
