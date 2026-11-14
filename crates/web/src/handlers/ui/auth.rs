use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

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
        theme: SessionTheme {
            primary: skiff_app::theme::primary(&state),
        },
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
    guard::csrf(&state, &user, &parts)?;
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

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/auth/login", get(login_config).post(login_submit))
        .route("/ui/v1/session", get(session))
        .route("/ui/v1/session/logout", post(logout))
}
