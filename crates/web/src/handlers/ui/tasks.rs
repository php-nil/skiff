use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/tasks`：定时任务列表（只读）。
///
/// `with_schema=true` 时额外带回页面骨架（页面加载用），表格翻页不带该参数。
pub async fn tasks(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<PageListQuery>,
) -> Result<Response, ApiError> {
    let svc = handlers::tasks::service(&state).map_err(ApiError)?;
    let keyword = q.q.as_deref().unwrap_or("").trim().to_string();
    let needle = keyword.to_lowercase();
    let views = svc.list_views().await;
    let rows: Vec<(String, TaskItem)> = views
        .iter()
        .map(task_item)
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.id.to_lowercase().contains(&needle)
                || t.target.to_lowercase().contains(&needle)
                || t.source.to_lowercase().contains(&needle)
        })
        .map(|t| (t.id.clone(), t))
        .collect();
    let (items, next_cursor, total) = paginate(rows, clamp_limit(q.limit), q.cursor.as_deref());
    let can_admin = user.can(skiff_model::Permission::Admin);
    let schema = q.with_schema.then(|| {
        let prefix = format!("/{}", state.cfg.entry_prefix);
        super::tasks_page::tasks_page_schema(can_admin, &keyword, &prefix)
    });
    let mut body = json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "can_admin": can_admin,
        }
    });
    if let Some(schema) = schema {
        body["schema"] = schema;
    }
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/tasks/{id}`：任务详情 + 最近 50 次执行（只读）。
pub async fn task_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    Query(q): Query<PageListQuery>,
) -> Result<Response, ApiError> {
    let svc = handlers::tasks::service(&state).map_err(ApiError)?;
    let view = svc
        .detail_view(&id, 50)
        .await
        .ok_or_else(|| ApiError(AppError::NotFound(format!("任务不存在：{id}"))))?;
    let task = task_item(&view["task"]);
    let payload = view["task"]["payload"].clone();
    let runs: Vec<TaskRunItem> = view["runs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| {
                    let r = handlers::tasks::run_row(v);
                    TaskRunItem {
                        trigger: r.trigger,
                        started_text: r.started_text,
                        duration: r.duration,
                        result: r.result,
                        result_class: r.result_class,
                        detail: r.detail,
                        missed: r.missed,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let can_admin = user.can(skiff_model::Permission::Admin);
    let schema = q
        .with_schema
        .then(|| super::tasks_page::task_detail_schema(&task, &payload, &runs, can_admin));
    let mut body = json!({
        "data": {
            "task": task,
            "payload": payload,
            "runs": runs,
            "can_admin": can_admin,
        }
    });
    if let Some(schema) = schema {
        body["schema"] = schema;
    }
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/tasks/{id}/toggle`：启用 / 停用（`TASK_TOGGLE`）。
pub async fn task_toggle(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<EnabledBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::TASK_TOGGLE)?;
    skiff_app::commands::task::toggle(
        &state,
        skiff_app::commands::task::ToggleTask {
            id: id.clone(),
            enabled: body.enabled,
            actor: handlers::tasks::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(task_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "id": id, "enabled": body.enabled }))
}

/// `POST /ui/v1/tasks/{id}/run`：立即运行（`TASK_RUN`）。
pub async fn task_run(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::TASK_RUN)?;
    skiff_app::commands::task::run(
        &state,
        skiff_app::commands::task::RunTask {
            id: id.clone(),
            actor: handlers::tasks::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(task_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "id": id, "started": true }))
}

/// `POST /ui/v1/tasks/{id}/delete`：删除任务（`TASK_DELETE`；内置任务由命令层拒绝）。
pub async fn task_delete(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::TASK_DELETE)?;
    skiff_app::commands::task::delete(
        &state,
        skiff_app::commands::task::DeleteTask {
            id: id.clone(),
            actor: handlers::tasks::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(task_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "id": id, "removed": true }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/tasks", get(tasks))
        .route("/ui/v1/tasks/{id}", get(task_detail))
        .route("/ui/v1/tasks/{id}/toggle", post(task_toggle))
        .route("/ui/v1/tasks/{id}/run", post(task_run))
        .route("/ui/v1/tasks/{id}/delete", post(task_delete))
}
