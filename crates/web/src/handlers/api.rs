//! `/api/v1`：与 UI 共用同一套会话、RBAC、审计与三层策略。
//!
//! 认证：会话 Cookie，或 `Authorization: Bearer <session token / API token>`。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use skiff_docker::Verb;
use skiff_model::Scope;

use crate::error::{ApiError, AppError};
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::auth::{LoginError, LoginRequest};
use crate::state::AppState;

/// JSON API 的登录请求体（wire 形状）。领域形状见 `skiff_app::auth::LoginRequest`。
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub otp: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    parts: Parts,
    Json(req): Json<LoginBody>,
) -> Result<Response, ApiError> {
    let ip = handlers::client_ip(&state, &parts);
    // 判定流程只有一份（`skiff_app::auth::login`），与 UI 表单登录共用。
    let req = LoginRequest {
        username: req.username,
        password: req.password,
        otp: req.otp,
    };
    let success = state.login(&req, &ip, "api", "api").await.map_err(|e| {
        ApiError(match e {
            LoginError::RateLimited(wait) => {
                AppError::Unavailable(format!("登录失败次数过多，请 {wait} 秒后重试"))
            }
            LoginError::Invalid(message) => AppError::Unauthorized(message),
            LoginError::Internal(message) => AppError::Internal(message),
        })
    })?;

    let mut resp = Json(json!({
        "token": success.session_token,
        "csrf_token": success.csrf_token,
        "expires_at": success.expires_at,
        "user": { "username": success.username, "role": success.role.to_string() },
    }))
    .into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &success.session_token,
            state.cfg.session.abs_secs,
            !state.cfg.dev_http,
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}

pub async fn containers(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    handlers::require_scope(&user, Scope::DockerRead)?;
    let list = state
        .docker()
        .list_containers()
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    Ok(Json(json!({ "containers": list })))
}

pub async fn container(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    handlers::require_scope(&user, Scope::DockerRead)?;
    let row = handlers::resolve_container(&state, &id).await?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            Verb::ContainerInspect,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let inspect = state
        .docker()
        .inspect_container(&row.id)
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    Ok(Json(json!({ "container": row, "inspect": inspect })))
}

pub async fn restart_container(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<serde_json::Value>, ApiError> {
    handlers::require_scope(&user, Scope::DockerWrite)?;
    let row = handlers::resolve_container(&state, &id).await?;
    let obj = handlers::object_ref(&row);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::Restart, Some(&obj))
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let result = state
        .docker()
        .execute(Verb::Restart, Some(&obj), skiff_docker::DockerParams::None)
        .await
        .map(|_| ());
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::Restart.audit_action(),
        &row.name,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    Ok(Json(json!({ "status": "accepted", "id": row.id })))
}

pub async fn plugins(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    handlers::require_scope(&user, Scope::PluginsRead)?;
    let list = state
        .db()
        .list_plugins()
        .map_err(|e| ApiError(AppError::Internal(e.to_string())))?;
    let out: Vec<serde_json::Value> = list
        .into_iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "version": p.version,
                "api": p.api,
                "enabled": p.enabled,
                "status": p.status,
                "key_id": p.key_id,
                "grants": p.grants.iter().map(|g| format!("{}:{}", g.kind, g.value)).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "plugins": out })))
}

/// 插件菜单注册项（§4.1 `ui`）：前端据此以沙箱 iframe 加载。
pub async fn plugin_ui_index(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    handlers::require_scope(&user, Scope::PluginsRead)?;
    let mut items = Vec::new();
    for p in state
        .db()
        .list_plugins()
        .map_err(|e| ApiError(AppError::Internal(e.to_string())))?
    {
        if !p.enabled {
            continue;
        }
        let Ok(m) = skiff_pkgfmt::Manifest::parse(&p.manifest) else {
            continue;
        };
        for entry in m.ui {
            items.push(json!({
                "plugin": p.id,
                "label": entry.label,
                "url": format!(
                    "/{}/plugins/{}/ui/{}",
                    state.cfg.entry_prefix, p.id, entry.path
                ),
            }));
        }
    }
    Ok(Json(json!({ "items": items })))
}
