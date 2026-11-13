//! 证书与备份：SSL 自签/上传/强制跳转/关闭，以及站点备份开关。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_app::actions as act;
use skiff_app::commands::site as site_cmd;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::SiteBackupView;
use crate::util::{encode_uri_component, form_first, parse_form};

use super::*;

/// SSL 标签：生成自签 / 上传 PEM / 强制跳转 / 关闭 / 删除证书。
pub async fn ssl_action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::SITE_SSL)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action");
    let prefix = &state.cfg.entry_prefix;
    let target = format!("/{prefix}/sites/{}?tab=ssl", encode_uri_component(&domain));
    let op = match action.as_str() {
        "generate" => site_cmd::SslOp::Generate,
        "upload" => site_cmd::SslOp::Upload {
            cert_pem: form_first(&form, "cert_pem"),
            key_pem: form_first(&form, "key_pem"),
        },
        "force" => site_cmd::SslOp::Force {
            enabled: matches!(
                form_first(&form, "enabled").trim(),
                "1" | "on" | "true" | "yes"
            ),
        },
        "disable" => site_cmd::SslOp::Disable,
        "delete_cert" => site_cmd::SslOp::DeleteCert,
        _ => {
            return Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                format!("未知 SSL 操作：{action}"),
                target,
            ))
        }
    };
    let result = site_cmd::ssl(
        &state,
        site_cmd::SiteSsl {
            domain: domain.clone(),
            op,
            actor: actor(&state, &user, &parts),
        },
    )
    .await;
    match result {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "SSL 设置已更新并重载 nginx",
            target,
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            target,
        )),
    }
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

/// 站点备份开关的视图。任务服务未初始化（旧库/未接线）时渲染为「不可用」，
/// 不把整页拖成 503——站点页的主要信息仍然可用。
pub(crate) async fn backup_view(state: &AppState, domain: &str) -> SiteBackupView {
    let Some(tasks) = state.tasks() else {
        return SiteBackupView::default();
    };
    let status = tasks.site_backup_status(domain).await;
    SiteBackupView {
        enabled: status.enabled,
        task_id: status.task_id,
        last_text: status
            .last_at
            .map(|at| {
                format!(
                    "{} · {}",
                    fmt_ts(at),
                    status.last_result.as_deref().unwrap_or("-")
                )
            })
            .unwrap_or_else(|| "从未执行".to_string()),
        next_text: status
            .next_at
            .map(fmt_ts)
            .unwrap_or_else(|| "-".to_string()),
    }
}

/// 站点备份开关：直接开关内核 `site-backup` 任务，不跳计划任务页。
pub async fn set_backup(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::SITE_SET_BACKUP)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let enabled = matches!(
        form_first(&form, "enabled").trim(),
        "1" | "on" | "true" | "yes"
    );
    let svc = state
        .tasks()
        .ok_or_else(|| AppError::Unavailable("内核任务服务未初始化".into()))?;
    match site_cmd::backup(
        &state,
        svc,
        site_cmd::SetSiteBackup {
            domain: domain.clone(),
            enabled,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            if enabled {
                "站点备份已开启（每日 03:00）"
            } else {
                "站点备份已关闭"
            },
            format!(
                "/{}/sites/{}",
                state.cfg.entry_prefix,
                encode_uri_component(&domain)
            ),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!(
                "/{}/sites/{}",
                state.cfg.entry_prefix,
                encode_uri_component(&domain)
            ),
        )),
    }
}
