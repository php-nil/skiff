//! 概览页 Schema：只读计数卡 + 系统状态（主机监控插件 RPC 结果）。

use super::schema_kit as kit;
use serde_json::{json, Value};

fn num(v: &Value, pointer: &str) -> i64 {
    v.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false)
}

fn metric_card(title: &str, value: i64, suffix: &str, caption: &str, color: &str) -> Value {
    let mut stat = json!({ "value": value, "label": caption, "suffix": suffix });
    if !color.is_empty() {
        stat["color"] = json!(color);
    }
    json!({
        "type": "card",
        "props": { "title": title, "variant": "outlined" },
        "content": { "body": [ { "type": "statistic", "props": stat } ] }
    })
}

/// `data`：`/ui/v1/overview` 的 `data`；`health`：RPC 采样或 `{error}`。
pub fn overview_schema(data: &Value, health: &Value) -> Value {
    let mut body = Vec::new();
    let docker_code = text(data, "/docker/status/code");
    if !docker_code.is_empty() && docker_code != "available" {
        let reason = text(data, "/docker/reason");
        body.push(kit::alert(
            "warning",
            &format!("Docker {}", text(data, "/docker/status/label")),
            if reason.is_empty() {
                "Docker 守护进程当前不可达，容器相关操作会返回 503。"
            } else {
                reason
            },
        ));
    }

    let site_abnormal = num(data, "/sites/abnormal");
    let runtime_abnormal = num(data, "/runtimes/abnormal");
    let site_caption = if site_abnormal > 0 {
        format!("{site_abnormal} 个异常")
    } else {
        "全部正常".to_string()
    };
    let plugin_caption = format!(
        "已启用 {} · 停用 {}",
        num(data, "/plugins/enabled"),
        num(data, "/plugins/disabled")
    );
    let runtime_caption = format!(
        "异常 {} · 未安装 {}",
        runtime_abnormal,
        num(data, "/runtimes/uninstalled")
    );
    body.push(json!({
        "type": "grid",
        "props": { "columns": 2, "gap": 16 },
        "content": { "body": [
            metric_card(
                "容器",
                num(data, "/containers/running"),
                " 个",
                &format!("运行中 · 共 {} 个", num(data, "/containers/total")),
                ""
            ),
            metric_card(
                "网站",
                num(data, "/sites/total"),
                " 个",
                &site_caption,
                if site_abnormal > 0 { "warning" } else { "success" }
            ),
            metric_card(
                "插件",
                num(data, "/plugins/total"),
                " 个",
                &plugin_caption,
                ""
            ),
            metric_card(
                "运行环境",
                num(data, "/runtimes/total"),
                " 个",
                &runtime_caption,
                if runtime_abnormal > 0 { "warning" } else { "" }
            )
        ] }
    }));

    let health_error = text(health, "/error");
    if !health_error.is_empty() {
        body.push(kit::card(
            "系统状态",
            vec![
                kit::text_line(format!("暂不可用：{health_error}")),
                kit::text_line(
                    "请检查主机监控插件是否已启用、授权根（path:/proc）是否完整，以及当前角色是否具备插件管理权限。",
                ),
            ],
        ));
    } else if !health.is_null() {
        if flag(health, "/cpu_high") || flag(health, "/mem_high") {
            body.push(kit::alert(
                "warning",
                "",
                format!(
                    "主机资源超过阈值：CPU {}，内存 {}",
                    text(health, "/cpu_text"),
                    text(health, "/mem_text")
                ),
            ));
        }
        let stat = |value: &str, label: &str, high: bool| {
            let mut props = json!({ "value": value, "label": label });
            if high {
                props["color"] = json!("warning");
            }
            json!({ "type": "statistic", "props": props })
        };
        body.push(kit::card(
            "系统状态",
            vec![
                json!({
                    "type": "grid",
                    "props": { "columns": 3, "gap": 12 },
                    "content": { "body": [
                        stat(text(health, "/cpu_text"), "CPU", flag(health, "/cpu_high")),
                        stat(text(health, "/mem_text"), "内存", flag(health, "/mem_high")),
                        stat(text(health, "/load_text"), "负载", false)
                    ] }
                }),
                kit::text_line(format!(
                    "运行时长 {} · 采集时间 {}（UTC）",
                    text(health, "/uptime_text"),
                    text(health, "/t_text")
                )),
            ],
        ));
    }

    body.push(kit::text_line(format!(
        "采集时间 {}",
        text(data, "/collected_at")
    )));

    kit::page(
        "概览",
        "只读计数，不做实时刷新；趋势图表待定（D14–D16），当前不展示",
        body,
    )
}
