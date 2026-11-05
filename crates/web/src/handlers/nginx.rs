//! 「网站 → Nginx」页面：状态 / 配置 / 日志 / 模块。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::handlers::sites::require_admin;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::NginxPage;
use crate::util::{encode_uri_component, form_first, parse_form};

use skiff_app::nginx::NginxParams;

fn allowed_tab(tab: &str) -> &'static str {
    match tab {
        "config" => "config",
        "logs" => "logs",
        "modules" => "modules",
        _ => "status",
    }
}

async fn page(
    state: Arc<AppState>,
    user: crate::state::CurrentUser,
    q: &HashMap<String, String>,
    error: String,
    notice: String,
) -> Result<Response, AppError> {
    let tab = allowed_tab(q.get("tab").map(String::as_str).unwrap_or("status"));
    let config = state.nginx_config_view().await.map_err(|e| match e {
        skiff_app::sites::SiteError::Internal(m) => AppError::Internal(m),
        skiff_app::sites::SiteError::BadRequest(m) => AppError::BadRequest(m),
        skiff_app::sites::SiteError::Unavailable(m) => AppError::Unavailable(m),
        skiff_app::sites::SiteError::Conflict(m) => AppError::Conflict(m),
    })?;
    handlers::render(NginxPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/nginx"),
        status: state.nginx_status().await,
        config,
        logs: state.nginx_logs(q.get("q").map(String::as_str).unwrap_or("")),
        modules: state.nginx_modules().await,
        tab: tab.to_string(),
        keyword: q.get("q").cloned().unwrap_or_default(),
        can_admin: user.can(skiff_model::Permission::Admin),
        error,
        notice,
    })
}

pub async fn index(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    page(
        state,
        user,
        &q,
        q.get("error").cloned().unwrap_or_default(),
        q.get("notice").cloned().unwrap_or_default(),
    )
    .await
}

pub async fn save_main(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let content = form_first(&form, "content");
    let ip = handlers::client_ip(&state, &parts);
    let prefix = state.cfg.entry_prefix.clone();
    match state.save_nginx_main(&content).await {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "nginx.config.save",
                "nginx.conf",
                "ok",
                "",
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/nginx?tab=config&notice={}",
                encode_uri_component("主配置已通过 nginx -t 并重载")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "nginx.config.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/nginx?tab=config&error={}",
                encode_uri_component(&msg)
            )))
        }
    }
}

pub async fn save_params(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let params = NginxParams {
        worker_processes: form_first(&form, "worker_processes").trim().to_string(),
        worker_connections: form_first(&form, "worker_connections")
            .trim()
            .parse()
            .unwrap_or(1024),
        keepalive_timeout: form_first(&form, "keepalive_timeout")
            .trim()
            .parse()
            .unwrap_or(65),
        client_max_body_size: form_first(&form, "client_max_body_size").trim().to_string(),
        gzip: form_first(&form, "gzip") == "on",
        gzip_types: form_first(&form, "gzip_types").trim().to_string(),
    };
    let ip = handlers::client_ip(&state, &parts);
    let prefix = state.cfg.entry_prefix.clone();
    match state.save_nginx_params(&params).await {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "nginx.params.save",
                "nginx.conf",
                "ok",
                "",
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/nginx?tab=config&notice={}",
                encode_uri_component("性能参数已保存并通过 nginx -t，随后重载")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "nginx.params.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/nginx?tab=config&error={}",
                encode_uri_component(&msg)
            )))
        }
    }
}
