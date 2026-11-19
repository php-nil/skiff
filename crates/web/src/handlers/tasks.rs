//! 内核定时任务的读模型：`/ui/v1/tasks` 的列表行与执行历史行。
//!
//! 任务服务未初始化时直接 503；任务来源只影响展示，不作为访问控制维度。

use std::sync::Arc;

use serde_json::Value;

use crate::error::AppError;
use crate::state::AppState;
use crate::templates::{TaskRow, TaskRunRow};

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::tasks::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;

/// 只读页面取任务服务；写路径统一走命令层。
pub(crate) fn service(state: &AppState) -> Result<&Arc<skiff_app::tasks::TaskService>, AppError> {
    state
        .tasks()
        .ok_or_else(|| AppError::Unavailable("内核任务服务未初始化".into()))
}

fn state_label(v: &Value) -> (String, String) {
    let lifecycle = v["lifecycle"].as_str().unwrap_or("");
    let runtime_state = v["state"].as_str().unwrap_or("");
    if lifecycle == "blocked" || runtime_state == "blocked" {
        return ("已阻塞".into(), "warn".into());
    }
    if runtime_state == "running" {
        return ("执行中".into(), "warn".into());
    }
    if !v["enabled"].as_bool().unwrap_or(false) {
        return ("已停用".into(), "muted".into());
    }
    ("待执行".into(), "ok".into())
}

pub(crate) fn row_of(v: &Value) -> TaskRow {
    let id = v["id"].as_str().unwrap_or("").to_string();
    let (state_text, state_class) = state_label(v);
    let (last_text, last_class) = match v.get("last").and_then(|l| l.as_object()) {
        Some(l) => {
            let ok = l.get("result").and_then(Value::as_str) == Some("ok");
            (
                format!(
                    "{} · {}",
                    l.get("at_text").and_then(Value::as_str).unwrap_or("-"),
                    if ok { "成功" } else { "失败" }
                ),
                if ok { "ok" } else { "danger" }.to_string(),
            )
        }
        None => ("从未执行".to_string(), "muted".to_string()),
    };
    TaskRow {
        builtin: id.starts_with("builtin:"),
        id,
        name: v["name"].as_str().unwrap_or("").to_string(),
        expr: v["expr"].as_str().unwrap_or("").to_string(),
        next_text: v
            .get("next_run")
            .and_then(|n| n.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        source: v["source"].as_str().unwrap_or("-").to_string(),
        target: v["target"].as_str().unwrap_or("-").to_string(),
        method: v["method"].as_str().unwrap_or("-").to_string(),
        enabled: v["enabled"].as_bool().unwrap_or(false),
        state_label: state_text,
        state_class,
        last_text,
        last_class,
    }
}

pub(crate) fn run_row(v: &Value) -> TaskRunRow {
    let result = v["result"].as_str().unwrap_or("");
    let (result_text, class) = match result {
        "ok" => ("成功", "ok"),
        "timeout" => ("超时", "warn"),
        "skipped" => ("跳过", "muted"),
        _ => ("失败", "danger"),
    };
    let trigger = match v["trigger"].as_str().unwrap_or("") {
        "schedule" => "定时",
        "manual" => "手动",
        "retry" => "重试",
        other => other,
    };
    let detail = v["error_detail"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    TaskRunRow {
        trigger: trigger.to_string(),
        started_text: v["started_text"].as_str().unwrap_or("-").to_string(),
        duration: format!(
            "{} ms",
            v["duration_ms"].as_i64().unwrap_or_default().max(0)
        ),
        result: result_text.to_string(),
        result_class: class.to_string(),
        detail,
        missed: v["missed_count"].as_i64().unwrap_or(0),
    }
}
