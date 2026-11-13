//! 内核定时任务的 Web 管理面（`/{prefix}/tasks`）。
//!
//! 与 cron 插件的区别：这里**不依赖插件**，任务服务未初始化时直接 503；
//! 只读用户（viewer）可以看列表/详情/历史，写操作一律 Admin + CSRF。
//! 任务来源只影响展示，不作为访问控制维度——权限由角色判定。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use serde_json::Value;

use skiff_app::actions as act;
use skiff_app::commands::{task as task_cmd, Actor};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{TaskDetailPage, TaskRow, TaskRunRow, TasksPage};
use crate::util::{form_first, parse_form};

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
pub(crate) fn actor(state: &AppState, user: &crate::state::CurrentUser, parts: &Parts) -> Actor {
    Actor {
        username: user.user.username.clone(),
        ip: handlers::client_ip(state, parts),
    }
}

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

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let svc = service(&state)?;
    let tasks = svc.list_views().await;
    let keyword = q.get("q").cloned().unwrap_or_default();
    let needle = keyword.trim().to_lowercase();
    let rows: Vec<TaskRow> = tasks
        .iter()
        .map(row_of)
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.id.to_lowercase().contains(&needle)
                || t.target.to_lowercase().contains(&needle)
                || t.source.to_lowercase().contains(&needle)
        })
        .collect();
    handlers::render(TasksPage {
        ctx: handlers::page_ctx(&state, &user, "core/tasks"),
        tasks: rows,
        can_admin: user.can(skiff_model::Permission::Admin),
        error: String::new(),
        notice: String::new(),
        keyword,
    })
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let svc = service(&state)?;
    let view = svc
        .detail_view(&id, 50)
        .await
        .ok_or_else(|| AppError::NotFound(format!("任务不存在：{id}")))?;
    let task = row_of(&view["task"]);
    let payload =
        serde_json::to_string_pretty(&view["task"]["payload"]).unwrap_or_else(|_| "{}".into());
    let runs = view["runs"]
        .as_array()
        .map(|a| a.iter().map(run_row).collect())
        .unwrap_or_default();
    handlers::render(TaskDetailPage {
        ctx: handlers::page_ctx(&state, &user, &format!("core/tasks/{id}")),
        task,
        payload,
        runs,
        can_admin: user.can(skiff_model::Permission::Admin),
        error: String::new(),
        notice: String::new(),
    })
}

fn back(id: &str) -> String {
    format!(
        "/{}/tasks/{}",
        crate::templates::entry_prefix(),
        crate::util::encode_uri_component(id)
    )
}

pub async fn toggle(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::TASK_TOGGLE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let enabled = matches!(
        form_first(&form, "enabled").trim(),
        "1" | "on" | "true" | "yes"
    );
    let cmd = task_cmd::ToggleTask {
        id: id.clone(),
        enabled,
        actor: actor(&state, &user, &parts),
    };
    match task_cmd::toggle(&state, cmd).await {
        Ok(_) => Ok(handlers::redirect(back(&id))),
        Err(task_cmd::TaskCommandError::Unavailable(m)) => Err(AppError::Unavailable(m)),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            back(&id),
        )),
    }
}

pub async fn run(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::TASK_RUN)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let cmd = task_cmd::RunTask {
        id: id.clone(),
        actor: actor(&state, &user, &parts),
    };
    match task_cmd::run(&state, cmd).await {
        Ok(_) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "任务已执行",
            back(&id),
        )),
        Err(task_cmd::TaskCommandError::Unavailable(m)) => Err(AppError::Unavailable(m)),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            back(&id),
        )),
    }
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::TASK_DELETE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let cmd = task_cmd::DeleteTask {
        id: id.clone(),
        actor: actor(&state, &user, &parts),
    };
    match task_cmd::delete(&state, cmd).await {
        Ok(()) => Ok(handlers::redirect(format!(
            "/{}/tasks",
            crate::templates::entry_prefix()
        ))),
        Err(task_cmd::TaskCommandError::Unavailable(m)) => Err(AppError::Unavailable(m)),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            back(&id),
        )),
    }
}
