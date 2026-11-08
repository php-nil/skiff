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

use skiff_app::nginx::{NginxLogQuery, NginxParams};

fn allowed_tab(tab: &str) -> &'static str {
    match tab {
        "config" => "config",
        "logs" | "log" => "logs",
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
    // 保存失败时不跳转，直接用用户提交的文本渲染配置页，避免把正在编辑的内容冲掉。
    submitted: Option<String>,
) -> Result<Response, AppError> {
    let tab = allowed_tab(q.get("tab").map(String::as_str).unwrap_or("status"));
    // 日志按需加载：只有 `load=1` 才读文件；参数走查询串，后端按条件读尾部。
    let logs = state.nginx_logs(&NginxLogQuery {
        loaded: q
            .get("load")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false),
        kind: if q.get("type").map(String::as_str) == Some("error") {
            "error".into()
        } else {
            "access".into()
        },
        keyword: q.get("q").cloned().unwrap_or_default(),
        level: q
            .get("level")
            .map(String::as_str)
            .unwrap_or("all")
            .to_string(),
        status: q.get("status").cloned().unwrap_or_default(),
        page_lines: q
            .get("lines")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(200),
        offset: q
            .get("offset")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(100_000),
    });
    let mut config = state.nginx_config_view().await.map_err(|e| match e {
        skiff_app::sites::SiteError::Internal(m) => AppError::Internal(m),
        skiff_app::sites::SiteError::BadRequest(m) => AppError::BadRequest(m),
        skiff_app::sites::SiteError::Unavailable(m) => AppError::Unavailable(m),
        skiff_app::sites::SiteError::Conflict(m) => AppError::Conflict(m),
    })?;
    if let Some(text) = submitted {
        config.main_content = text;
    }
    handlers::render(NginxPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/web"),
        status: state.nginx_status().await,
        install: state.site_install_progress().await,
        config,
        logs,
        modules: state.nginx_modules().await,
        tab: tab.to_string(),
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
        None,
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
                "/{prefix}/sites/web?tab=config&notice={}",
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
            // 校验失败直接回显配置页，保留用户本次提交的内容；跳转会重新从磁盘读取，冲掉编辑。
            let mut q = HashMap::new();
            q.insert("tab".to_string(), "config".to_string());
            page(state, user, &q, msg, String::new(), Some(content)).await
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
                "/{prefix}/sites/web?tab=config&notice={}",
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
                "/{prefix}/sites/web?tab=config&error={}",
                encode_uri_component(&msg)
            )))
        }
    }
}
