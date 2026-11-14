use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/jobs`：作业列表（只读，最近 200 条）。
pub async fn jobs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let rows: Vec<(String, JobItem)> = state
        .jobs()
        .list(200)
        .iter()
        .map(|job| {
            let item = job_item(job);
            (item.id.clone(), item)
        })
        .collect();
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "can_admin": user.can(skiff_model::Permission::Admin),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/jobs/{id}`：作业详情 + 首屏日志尾巴（纯文本）。
pub async fn job_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let job = handlers::jobs::load_job(&state, &id).map_err(ApiError)?;
    let tail = state.jobs().tail(&job.id, 200);
    let cursor = tail.last().map(|r| r.seq).unwrap_or(0);
    let log_text = tail
        .iter()
        .map(|r| format!("[{}] {}", handlers::jobs::fmt_log_time(r.at), r.line))
        .collect::<Vec<_>>()
        .join("\n");
    let view = handlers::jobs::detail_of(&job, cursor, log_text);
    let mut resp = Json(json!({
        "data": {
            "id": view.id,
            "title": view.title,
            "kind_label": view.kind_label,
            "target_label": view.target_label,
            "status_label": view.status_label,
            "status_class": view.status_class,
            "phase_label": view.phase_label,
            "progress": view.progress,
            "indeterminate": view.indeterminate,
            "cancellable": view.cancellable,
            "cancel_requested": view.cancel_requested,
            "created_by": view.created_by,
            "created_by_role": view.created_by_role,
            "requester_ip": view.requester_ip,
            "created_at": rfc3339(job.created_at),
            "started_at": job.started_at.map(rfc3339),
            "finished_at": job.finished_at.map(rfc3339),
            "duration": view.duration,
            "exit_code": view.exit_code,
            "trigger": view.trigger,
            "error_class": view.error_class,
            "error_detail": view.error_detail,
            "result": job.result.clone(),
            "params": job.params.clone(),
            "steps": job.steps.clone(),
            "retry_of": view.retry_of,
            // Web 安装包含拉镜像/重建容器，步骤不可中断；任务中心不展示取消入口。
            "can_cancel": view.can_cancel
                && job.kind != skiff_app::jobs::KIND_WEB_INSTALL,
            "can_retry": view.can_retry,
            "lookback": cursor,
            "logs": view.log_text,
            "can_admin": user.can(skiff_model::Permission::Admin),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/jobs/{id}/cancel`：请求取消（`JOB_CANCEL`）。
pub async fn job_cancel(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::JOB_CANCEL)?;
    let outcome = skiff_app::commands::job::cancel(
        &state,
        skiff_app::commands::job::CancelJob {
            id: id.clone(),
            actor: handlers::jobs::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(AppError::BadRequest(e.to_string())))?;
    let (code, message) = match outcome {
        skiff_app::jobs::CancelOutcome::CancelledNow => ("cancelled_now", "已取消（任务尚未开始）"),
        skiff_app::jobs::CancelOutcome::Requested => {
            ("requested", "已请求取消，将在当前步骤结束后停止")
        }
        skiff_app::jobs::CancelOutcome::Terminal => ("terminal", "任务已结束，无需取消"),
    };
    write_done(json!({ "id": id, "outcome": code, "message": message }))
}

/// `POST /ui/v1/jobs/{id}/retry`：重试作业（`JOB_RETRY`），返回新作业 id。
pub async fn job_retry(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::JOB_RETRY)?;
    let job = skiff_app::commands::job::retry(
        &state,
        skiff_app::commands::job::RetryJob {
            id: id.clone(),
            role: user.user.role.as_str().to_string(),
            actor: handlers::jobs::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "id": job.id, "retry_of": id }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/jobs", get(jobs))
        .route("/ui/v1/jobs/{id}", get(job_detail))
        .route("/ui/v1/jobs/{id}/cancel", post(job_cancel))
        .route("/ui/v1/jobs/{id}/retry", post(job_retry))
}
