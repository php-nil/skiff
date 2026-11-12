//! `/ui/v1`：budding 前端运行时（SDUI）的 JSON 命名空间。
//!
//! 与 `handlers::api`（`/api/v1` 机器 API）并列，但服务对象不同：这里只给页面渲染运行时用，
//! 会话 Cookie + CSRF 鉴权，**未认证一律 401 JSON**（`UiAuthUser` 提取器），不做页面 303；
//! 失败载荷沿用 `AppError` 的同一套信封（`code/status/message/field/retryable`），
//! 不另造错误形状（URL 契约 §6、SDUI 集成 §3.2/§5.2）。
//!
//! 本批只落地会话引导与登出：壳页启动时据此判定登录态、取 CSRF、入口前缀与能力开关。

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{ApiError, AppError};
use crate::handlers::auth::{
    issue_login_csrf, login_redirect_target, verify_login_csrf, LOGIN_CSRF_TTL_SECS,
};
use crate::handlers::{self, LOGIN_CSRF_COOKIE};
use crate::middleware::UiAuthUser;
use crate::state::auth::{LoginError, LoginRequest};
use crate::state::AppState;

/// 接口版本：与路径段 `v1` 一致，前端只做兼容判断（SDUI 集成 §3.7）。
pub const API_VERSION: &str = "v1";

/// `GET /ui/v1/session` 的响应数据。
#[derive(Debug, Serialize)]
pub struct SessionData {
    pub username: String,
    pub role: String,
    /// 与页面写操作同源的 CSRF 令牌（`AppState::csrf_token`）。
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
    /// 壳页启动时需要知道的能力开关（字段只增不改）。
    pub features: SessionFeatures,
}

#[derive(Debug, Serialize)]
pub struct SessionFeatures {
    /// 页面深链由内核回落壳页（URL 契约 §2.1）。
    pub shell_fallback: bool,
    /// 尚未适配的页面用统一占位呈现。
    pub placeholders: bool,
    /// 插件功能页仍走 iframe 外壳路由（PLUGIN_UI_CONTRACT）。
    pub plugin_iframe: bool,
}

/// `GET /ui/v1/auth/login` 的响应数据：登录页渲染前只需要 CSRF 与入口前缀。
#[derive(Debug, Serialize)]
pub struct LoginConfigData {
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
}

/// `GET /ui/v1/auth/login`：签发登录 CSRF（未认证可读）。
///
/// 与页面登录共用同一个签名 token 与 `skiff_login` 兼容 Cookie，不新造第二种
/// CSRF 生成方式；响应禁止缓存，避免代理把旧 token 回放给下一个用户。
pub async fn login_config(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    let csrf = issue_login_csrf(&state);
    let secure = handlers::is_secure_request(&state, &parts);
    let mut resp = Json(json!({
        "data": LoginConfigData {
            csrf_token: csrf.clone(),
            entry_prefix: state.cfg.entry_prefix.clone(),
            api_version: API_VERSION,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut().append(
        SET_COOKIE,
        handlers::simple_cookie(LOGIN_CSRF_COOKIE, &csrf, LOGIN_CSRF_TTL_SECS, secure)
            .parse()
            .unwrap(),
    );
    resp
}

/// `POST /ui/v1/auth/login` 的请求体（wire 形状）。
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub otp: String,
    /// 登录页签发的签名 CSRF token（与表单路径同一个）。
    #[serde(default)]
    pub csrf: String,
    /// 登录成功后的回跳深链（只接受站内绝对路径）。
    #[serde(default)]
    pub next: String,
}

/// `POST /ui/v1/auth/login`：budding 登录页的提交入口。
///
/// 认证判定、会话签发与 Cookie 形态完全复用现有表单链路（`AppState::login` +
/// `skiff_identity::session_cookie`），这里只把结果换成 JSON 与 `redirect_to`。
pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    parts: Parts,
    Json(body): Json<LoginBody>,
) -> Result<Response, ApiError> {
    verify_login_csrf(&state, &parts, &body.csrf)?;
    let ip = handlers::client_ip(&state, &parts);
    let req = LoginRequest {
        username: body.username.clone(),
        password: body.password,
        otp: body.otp,
    };
    let success = state
        .login(&req, &ip, "session", &handlers::host_header(&parts))
        .await
        .map_err(|e| {
            ApiError(match e {
                LoginError::RateLimited(wait) => {
                    AppError::Unavailable(format!("登录失败次数过多，请 {wait} 秒后重试"))
                }
                LoginError::Invalid(message) => AppError::Unauthorized(message),
                LoginError::Internal(message) => AppError::Internal(message),
            })
        })?;

    let target = login_redirect_target(&state, &body.next, success.must_change_password);
    let mut resp = Json(json!({
        "data": {
            "redirect_to": target,
            "username": success.username,
            "role": success.role.to_string(),
        }
    }))
    .into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &success.session_token,
            state.cfg.session.abs_secs,
            handlers::is_secure_request(&state, &parts),
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}

/// `GET /ui/v1/session`：当前用户、角色、CSRF、入口前缀与壳页能力开关。
///
/// 未登录由 `UiAuthUser` 返回 401 JSON（`auth.unauthenticated`）。
pub async fn session(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Json<serde_json::Value> {
    let data = SessionData {
        username: user.user.username.clone(),
        role: user.role().to_string(),
        csrf_token: state.csrf_token(&user.session_token),
        entry_prefix: state.cfg.entry_prefix.clone(),
        api_version: API_VERSION,
        features: SessionFeatures {
            shell_fallback: true,
            placeholders: true,
            plugin_iframe: true,
        },
    };
    Json(json!({ "data": data }))
}

/// `POST /ui/v1/session/logout`：撤销当前会话并清 Cookie。
///
/// 写路径 CSRF 与页面写操作同源：令牌放 `x-csrf-token` 头，判定复用
/// `middleware::check_csrf`（不新造第二套）。失败返回 403 JSON `auth.csrf_invalid`。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let ip = handlers::client_ip(&state, &parts);
    state
        .logout(&user.user.username, &user.session_id, &ip)
        .map_err(|e| ApiError(AppError::Internal(e)))?;

    let mut resp = Json(json!({ "data": { "ok": true } })).into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::clear_cookie(
            state.cookie_name(),
            handlers::is_secure_request(&state, &parts),
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}
