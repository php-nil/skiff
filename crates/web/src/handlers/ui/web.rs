use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/web`：Web 服务（OpenResty）状态、安装进度、配置与模块快照。
pub async fn web(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let status = state.nginx_status().await;
    let install = state.site_install_progress().await;
    let config = state
        .nginx_config_view()
        .await
        .map_err(|e| ApiError(handlers::sites::map_err(e)))?;
    let modules = state.nginx_modules().await;
    write_done(json!({
        "status": {
            "available": status.available,
            "reason": status.reason,
            "container_state": status.container_state,
            "version": status.version,
            "config_test": status.config_test,
            "uptime": status.uptime,
            "last_reload": status.last_reload,
            "probe_url": status.probe_url,
            "probe_error": status.probe_error,
            "conf_path": status.conf_path,
            "conf_has_stub": status.conf_has_stub,
            "active": status.active,
            "accepts": status.accepts,
            "handled": status.handled,
            "requests": status.requests,
            "reading": status.reading,
            "writing": status.writing,
            "waiting": status.waiting,
            "stub_reason": status.stub_reason,
        },
        "install": {
            "running": install.running,
            "done": install.done,
            "error": install.error,
            "steps": install.steps.iter().map(|s| json!({
                "name": s.name,
                "state": s.state.as_str(),
                "detail": s.detail,
            })).collect::<Vec<_>>(),
        },
        "config": {
            "main_path": config.main_path,
            "main_content": config.main_content,
            "include_path": config.include_path,
            "include_content": config.include_content,
            "sites": config.sites.iter().map(|s| json!({
                "domain": s.domain,
                "path": s.path,
                "content": s.content,
            })).collect::<Vec<_>>(),
            "params": {
                "worker_processes": config.params.worker_processes,
                "worker_connections": config.params.worker_connections,
                "keepalive_timeout": config.params.keepalive_timeout,
                "client_max_body_size": config.params.client_max_body_size,
                "gzip": config.params.gzip,
                "gzip_types": config.params.gzip_types,
            },
        },
        "modules": {
            "available": modules.available,
            "reason": modules.reason,
            "version": modules.version,
            "configure_args": modules.configure_args,
            "modules": modules.modules,
        },
        "can_admin": user.can(skiff_model::Permission::Admin),
    }))
}

/// `GET /ui/v1/web/logs`：按需读取日志尾部（未 `load` 时绝不触碰文件）。
pub async fn web_logs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<WebLogsQuery>,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let logs = state.nginx_logs(&skiff_app::nginx::NginxLogQuery {
        loaded: q.load == "1" || q.load == "true",
        kind: if q.kind == "error" {
            "error".into()
        } else {
            "access".into()
        },
        keyword: q.q,
        level: if q.level.is_empty() {
            "all".into()
        } else {
            q.level
        },
        status: q.status,
        page_lines: q.lines.unwrap_or(200),
        offset: q.offset.unwrap_or(0).min(100_000),
    });
    write_done(json!({
        "loaded": logs.loaded,
        "kind": logs.kind,
        "keyword": logs.keyword,
        "level": logs.level,
        "status": logs.status,
        "page_lines": logs.page_lines,
        "offset": logs.offset,
        "next_offset": logs.next_offset,
        "rows": logs.rows,
        "path": logs.path,
        "reason": logs.reason,
        "scanned_lines": logs.scanned_lines,
        "scanned_bytes": logs.scanned_bytes,
        "scanned_text": logs.scanned_text,
        "file_bytes": logs.file_bytes,
        "file_text": logs.file_text,
        "truncated": logs.truncated,
        "has_more": logs.has_more,
        "status_counts": logs.status_counts.iter().map(|c| json!({
            "label": c.label,
            "count": c.count,
        })).collect::<Vec<_>>(),
    }))
}

/// `POST /ui/v1/web/config`：主配置保存（同 `WEB_CONFIG_MAIN_SAVE`，共用 `apply_main`）。
pub async fn web_config_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<WebConfigBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::WEB_CONFIG_MAIN_SAVE)?;
    let ip = handlers::client_ip(&state, &parts);
    match handlers::nginx::apply_main(&state, &user, &ip, &body.content).await {
        Ok(message) => write_done(json!({ "message": message })),
        Err(message) => Err(ApiError(AppError::BadRequest(message))),
    }
}

/// `POST /ui/v1/web/params`：性能参数保存（同 `WEB_CONFIG_PARAMS_SAVE`，共用 `apply_params`）。
pub async fn web_params_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<WebParamsBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::WEB_CONFIG_PARAMS_SAVE)?;
    let ip = handlers::client_ip(&state, &parts);
    let params = skiff_app::nginx::NginxParams {
        worker_processes: body.worker_processes.trim().to_string(),
        worker_connections: body.worker_connections,
        keepalive_timeout: body.keepalive_timeout,
        client_max_body_size: body.client_max_body_size.trim().to_string(),
        gzip: body.gzip,
        gzip_types: body.gzip_types.trim().to_string(),
    };
    match handlers::nginx::apply_params(&state, &user, &ip, &params).await {
        Ok(message) => write_done(json!({ "message": message })),
        Err(message) => Err(ApiError(AppError::BadRequest(message))),
    }
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/web", get(web))
        .route("/ui/v1/web/logs", get(web_logs))
        .route("/ui/v1/web/config", post(web_config_save))
        .route("/ui/v1/web/params", post(web_params_save))
}
