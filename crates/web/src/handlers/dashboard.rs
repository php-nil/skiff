use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::{AuditRow, DashboardPage};

/// `/{prefix}/` → `/{prefix}`（307 保留方法与查询串）。
pub async fn root_redirect(State(state): State<Arc<AppState>>) -> Response {
    axum::response::Redirect::temporary(&format!("/{}", state.cfg.entry_prefix)).into_response()
}

pub async fn index(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let docker_ok = state.docker().available();
    let containers = if docker_ok {
        state.docker().list_containers().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let running = containers.iter().filter(|c| c.state == "running").count();
    let image_count = if docker_ok {
        match state.docker().list_images().await {
            Ok(v) => v.len().to_string(),
            Err(_) => "-".into(),
        }
    } else {
        "-".into()
    };
    let plugins = state.db().list_plugins()?;
    let audit = state
        .db()
        .recent_audit(6)?
        .into_iter()
        .map(|a| AuditRow {
            ts: chrono::DateTime::from_timestamp(a.ts, 0)
                .map(|d| d.format("%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            actor: a.actor,
            ip: a.ip,
            action: a.action,
            target: a.target,
            result: a.result,
            detail: crate::util::truncate(&a.detail, 80),
        })
        .collect();

    let page = DashboardPage {
        ctx: handlers::page_ctx(&state, &user, "dashboard"),
        running,
        total: containers.len(),
        image_count,
        plugin_count: plugins.len(),
        uptime: format_uptime(state.started_at.elapsed().as_secs()),
        docker_reason: state.docker_unavailable_reason().to_string(),
        containers: containers
            .iter()
            .take(8)
            .map(handlers::container_view)
            .collect(),
        audit,
        error: String::new(),
    };
    handlers::render(page)
}

fn format_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}天 {h}小时")
    } else if h > 0 {
        format!("{h}小时 {m}分")
    } else {
        format!("{m}分")
    }
}
