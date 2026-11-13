//! 「网站 → Nginx」页面：状态 / 配置 / 日志 / 模块。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use skiff_app::actions as act;

use crate::handlers::sites::require_admin;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::NginxPage;
use crate::util::{form_first, parse_form};

use skiff_app::nginx::{NginxLogQuery, NginxParams};

fn allowed_tab(tab: &str) -> &'static str {
    match tab {
        "config" => "config",
        "logs" | "log" => "logs",
        "modules" => "modules",
        _ => "status",
    }
}

/// 解析性能参数表单。
///
/// 数字字段非法时直接报错，不再 `unwrap_or(默认值)`：静默回落会让页面提示「已保存」，
/// 实际落库的却是默认值，用户无法察觉。
fn parse_params(form: &HashMap<String, Vec<String>>) -> Result<NginxParams, String> {
    let worker_connections = form_first(form, "worker_connections")
        .trim()
        .parse::<u32>()
        .map_err(|_| "worker_connections 需为 64–65535 的整数".to_string())?;
    let keepalive_timeout = form_first(form, "keepalive_timeout")
        .trim()
        .parse::<u32>()
        .map_err(|_| "keepalive_timeout 需为 0–3600 的整数".to_string())?;
    Ok(NginxParams {
        worker_processes: form_first(form, "worker_processes").trim().to_string(),
        worker_connections,
        keepalive_timeout,
        client_max_body_size: form_first(form, "client_max_body_size").trim().to_string(),
        gzip: form_first(form, "gzip") == "on",
        gzip_types: form_first(form, "gzip_types").trim().to_string(),
    })
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
    let mut config = state
        .nginx_config_view()
        .await
        .map_err(|e| match e.unfield() {
            skiff_app::sites::SiteError::Internal(m) => AppError::Internal(m),
            skiff_app::sites::SiteError::BadRequest(m) => AppError::BadRequest(m),
            skiff_app::sites::SiteError::Unavailable(m) => AppError::Unavailable(m),
            skiff_app::sites::SiteError::Conflict(m) => AppError::Conflict(m),
            skiff_app::sites::SiteError::Field(_, inner) => AppError::Internal(inner.to_string()),
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
    page(state, user, &q, String::new(), String::new(), None).await
}

/// `GET /{p}/sites/web` 规范入口：交付 budding 壳页（管理专属）。
///
/// 旧 SSR 保留在 `GET /{p}/m/core/sites/web`，作为菜单深链与无 JS 兜底。
pub async fn shell(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    Ok(crate::handlers::spa::shell_page(&state))
}

/// 主配置保存：SSR 表单与 `/ui/v1/web/config` 共用的唯一业务入口。
///
/// 校验（`nginx -t`）、原子写与 `nginx.config.save` 审计都在这里；调用方只决定
/// 「PRG 跳转 / 保留编辑内容回显」还是「JSON 信封」。
pub(crate) async fn apply_main(
    state: &AppState,
    user: &crate::state::CurrentUser,
    ip: &str,
    content: &str,
) -> Result<String, String> {
    match state.save_nginx_main(content).await {
        Ok(warnings) => {
            let detail = warnings.join("；");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.config.save",
                "nginx.conf",
                "ok",
                &detail,
            );
            Ok(if warnings.is_empty() {
                "主配置已通过 nginx -t 并重载".to_string()
            } else {
                format!(
                    "主配置已保存并重载；nginx -t 有 {} 条告警：{}",
                    warnings.len(),
                    detail
                )
            })
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.config.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Err(msg)
        }
    }
}

/// 性能参数保存：SSR 表单与 `/ui/v1/web/params` 共用的唯一业务入口。
pub(crate) async fn apply_params(
    state: &AppState,
    user: &crate::state::CurrentUser,
    ip: &str,
    params: &NginxParams,
) -> Result<String, String> {
    match state.save_nginx_params(params).await {
        Ok(warnings) => {
            let detail = warnings.join("；");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.params.save",
                "nginx.conf",
                "ok",
                &detail,
            );
            Ok(if warnings.is_empty() {
                "性能参数已保存并通过 nginx -t，随后重载".to_string()
            } else {
                format!(
                    "性能参数已保存并重载；nginx -t 有 {} 条告警：{}",
                    warnings.len(),
                    detail
                )
            })
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.params.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Err(msg)
        }
    }
}

pub async fn save_main(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::WEB_CONFIG_MAIN_SAVE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let content = form_first(&form, "content");
    let ip = handlers::client_ip(&state, &parts);
    let prefix = state.cfg.entry_prefix.clone();
    match apply_main(&state, &user, &ip, &content).await {
        Ok(notice) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            notice,
            format!("/{prefix}/sites/web?tab=config"),
        )),
        Err(msg) => {
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
    crate::authz::authorize(&user, act::WEB_CONFIG_PARAMS_SAVE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    let prefix = state.cfg.entry_prefix.clone();
    // 数字字段显式校验：以前 `unwrap_or(1024/65)` 会把「abc」静默存成默认值，
    // 页面显示成功、实际保存的不是用户输入的值。
    let params = match parse_params(&form) {
        Ok(p) => p,
        Err(msg) => {
            state.audit_or_log(
                &user.user.username,
                &ip,
                "nginx.params.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            return Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/web?tab=config"),
            ));
        }
    };
    match apply_params(&state, &user, &ip, &params).await {
        Ok(notice) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            notice,
            format!("/{prefix}/sites/web?tab=config"),
        )),
        Err(msg) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            msg,
            format!("/{prefix}/sites/web?tab=config"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
            .collect()
    }

    #[test]
    fn params_parse_rejects_non_numeric_numbers() {
        let err = parse_params(&form(&[
            ("worker_processes", "auto"),
            ("worker_connections", "abc"),
            ("keepalive_timeout", "65"),
            ("client_max_body_size", "16m"),
            ("gzip", "on"),
            ("gzip_types", "text/plain"),
        ]))
        .unwrap_err();
        assert!(err.contains("worker_connections"), "{err}");

        let err = parse_params(&form(&[
            ("worker_processes", "auto"),
            ("worker_connections", "2048"),
            ("keepalive_timeout", "x"),
            ("client_max_body_size", "16m"),
            ("gzip", "on"),
            ("gzip_types", "text/plain"),
        ]))
        .unwrap_err();
        assert!(err.contains("keepalive_timeout"), "{err}");
    }

    #[test]
    fn params_parse_keeps_submitted_values() {
        let p = parse_params(&form(&[
            ("worker_processes", " 4 "),
            ("worker_connections", "2048"),
            ("keepalive_timeout", "90"),
            ("client_max_body_size", "32m"),
            ("gzip", "on"),
            ("gzip_types", " text/css "),
        ]))
        .expect("应解析");
        assert_eq!(p.worker_processes, "4");
        assert_eq!(p.worker_connections, 2048);
        assert_eq!(p.keepalive_timeout, 90);
        assert_eq!(p.client_max_body_size, "32m");
        assert!(p.gzip);
        assert_eq!(p.gzip_types, "text/css");
    }
}
