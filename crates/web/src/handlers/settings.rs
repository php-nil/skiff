use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AdminUser, AuthUser};
use crate::state::{AppState, CurrentUser};
use crate::templates::{AuditPage, AuditRow, KeyRow, SettingsPage};
use crate::util::{form_first, parse_form};

pub async fn render_settings(
    state: &Arc<AppState>,
    user: &CurrentUser,
    error: &str,
    notice: &str,
    totp: Option<(String, String)>,
    backup_codes: Vec<String>,
    force: bool,
) -> Result<Response, AppError> {
    let keys = state
        .db
        .list_trusted_keys()?
        .into_iter()
        .map(|(key_id, _pk, label)| KeyRow {
            fingerprint_short: key_id.chars().take(16).collect(),
            key_id,
            label,
        })
        .collect();

    let user_row = state
        .db
        .user_by_id(user.user.id)?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

    let page = SettingsPage {
        ctx: handlers::page_ctx(state, user, "settings"),
        entry_prefix: state.cfg.entry_prefix.clone(),
        totp_enabled: user_row.totp_enabled,
        totp_secret: totp.as_ref().map(|(s, _)| s.clone()).unwrap_or_default(),
        totp_uri: totp.as_ref().map(|(_, u)| u.clone()).unwrap_or_default(),
        backup_codes,
        keys,
        is_admin: user.is_admin(),
        error: error.to_string(),
        notice: notice.to_string(),
        force_change: force || user.must_change_password(),
    };
    handlers::render(page)
}

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let force = q.get("force").map(|v| v == "1").unwrap_or(false);
    render_settings(&state, &user, "", "", None, Vec::new(), force).await
}

pub async fn import_key(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let public_hex = form_first(&form, "public_key").trim().to_lowercase();
    let label = form_first(&form, "label").trim().to_string();
    let vk = skiff_pkgfmt::decode_public_key(&public_hex)
        .map_err(|e| AppError::BadRequest(format!("公钥非法: {e}")))?;
    let key_id = skiff_pkgfmt::key_id_of(&vk.to_bytes());
    state.db.trust_key(&key_id, &public_hex, &label)?;
    let _ = state.db.audit(
        &user.user.username,
        "local",
        "settings.key.import",
        &key_id,
        "ok",
        &label,
    );
    render_settings(
        &state,
        &user,
        "",
        &format!("已导入公钥，指纹 {key_id}（请与发布者公布值逐位核对）"),
        None,
        Vec::new(),
        false,
    )
    .await
}

pub async fn remove_key(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(key_id): Path<String>,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state.db.remove_trusted_key(&key_id)?;
    let _ = state.db.audit(
        &user.user.username,
        "local",
        "settings.key.remove",
        &key_id,
        "ok",
        "撤销后该公钥签名的包将无法安装/升级",
    );
    render_settings(&state, &user, "", "已撤销该公钥", None, Vec::new(), false).await
}

pub async fn audit(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let rows = state
        .db
        .recent_audit(200)?
        .into_iter()
        .map(|a| AuditRow {
            ts: chrono::DateTime::from_timestamp(a.ts, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            actor: a.actor,
            ip: a.ip,
            action: a.action,
            target: a.target,
            result: a.result,
            detail: crate::util::truncate(&a.detail, 120),
        })
        .collect();
    handlers::render(AuditPage {
        ctx: handlers::page_ctx(&state, &user, "audit"),
        rows,
        error: String::new(),
    })
}
