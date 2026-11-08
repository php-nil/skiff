//! 请求级中间件（由内到外）：可信代理解析（`client_info`）→ 会话（`load_session`）→ 安全入口（`entry_guard`），
//! 以及仅挂在非流式路由组的 `request_timeout`。`CatchPanic` 在 `lib.rs::router()` 挂最外层，不在此文件。
//!
//! CSRF 只提供 `check_csrf` 给 handler 调用，不做成 layer：token 在表单 body 里，
//! 做成中间件就得在校验前把 body 读一遍，handler 再用还得重新解析。

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::header::{
    CONTENT_SECURITY_POLICY, COOKIE, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::{AppState, ClientInfo, CurrentUser};
use crate::util::parse_cookies;
use skiff_model::{Permission, ScopeSet};

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
     form-action 'self'; object-src 'none'";

/// CSP 头只构造一次：每个 HTML 响应都要带它，而字符串解析是纯浪费。
fn csp_header() -> &'static HeaderValue {
    static CSP_VALUE: OnceLock<HeaderValue> = OnceLock::new();
    CSP_VALUE.get_or_init(|| HeaderValue::from_static(CSP))
}

/// 只有健康检查不需要安全入口前缀；静态资源与插件 UI 都走 `/{前缀}/...`，
/// 由下面的前缀判断放行，不再保留根路径旁路。
fn is_exempt(path: &str) -> bool {
    path == "/healthz"
}

/// 非流式请求的处理时长上限。等待对象是 Docker Engine 之类的下游——它们不该
/// 无限期占着一个连接。**不挂到 SSE / WS 路由**：那两条的生命周期由连接本身决定，
/// 套超时等于每隔几十秒踢一次用户。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn timeout_after(dur: Duration, req: Request, next: Next) -> Response {
    match tokio::time::timeout(dur, next.run(req)).await {
        Ok(resp) => resp,
        Err(_) => AppError::Timeout(format!(
            "请求处理超过 {} 秒仍未完成，已中止（下游可能无响应）",
            dur.as_secs()
        ))
        .into_response(),
    }
}

/// 请求超时（见 `REQUEST_TIMEOUT`）。丢弃 future 即取消下游调用，不留悬挂任务。
pub async fn request_timeout(req: Request, next: Next) -> Response {
    timeout_after(REQUEST_TIMEOUT, req, next).await
}

/// 安全入口（§6.3）：非前缀一律 404（不是 403），只作隐蔽入口、不替代认证。
pub async fn entry_guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !is_exempt(&path) {
        let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
        let prefix_ok = first == state.cfg.entry_prefix
            || state.cfg.entry_prefix_previous.as_deref() == Some(first)
            || (first == "api" && path.starts_with("/api/v1/"));
        if !prefix_ok {
            return AppError::NotFound("404 Not Found".into()).into_response();
        }
    }
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    // 默认 DENY；插件 UI 页面需要被面板自己的 iframe 嵌入，handler 已显式设成
    // SAMEORIGIN，这里不能无条件覆盖，否则插件功能页会被浏览器以 XFO 拦截。
    headers
        .entry(X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    if !headers.contains_key(CONTENT_SECURITY_POLICY) {
        headers.insert(CONTENT_SECURITY_POLICY, csp_header().clone());
    }
    resp
}

/// 可信代理解析（§6.2）：只有直连 peer 命中白名单才采信 `X-Forwarded-*`，否则忽略。
pub async fn client_info(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0)
        .or_else(|| req.extensions().get::<SocketAddr>().copied());

    let trusted = peer
        .map(|p| {
            state
                .cfg
                .trusted_proxies
                .iter()
                .any(|c| c.contains(&p.ip()))
        })
        .unwrap_or(false);

    let mut forwarded_ignored = true;
    let mut ip = peer
        .map(|p| p.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let mut scheme = if state.cfg.dev_http { "http" } else { "https" }.to_string();

    if trusted {
        // 客户端伪造头在可信代理命中时才被读取；取第一跳（最左侧真实客户端）。
        if let Some(xff) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(first) = xff.split(',').next() {
                let candidate = first.trim();
                if candidate.parse::<std::net::IpAddr>().is_ok() {
                    ip = candidate.to_string();
                    forwarded_ignored = false;
                }
            }
        }
        if let Some(proto) = req
            .headers()
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
        {
            let proto = proto.split(',').next().unwrap_or("").trim();
            if proto == "http" || proto == "https" {
                scheme = proto.to_string();
                forwarded_ignored = false;
            }
        }
    }
    // 未命中可信代理：清理客户端伪造头，避免进入审计/限流/Cookie 逻辑
    if forwarded_ignored {
        req.headers_mut().remove("x-forwarded-for");
        req.headers_mut().remove("x-forwarded-proto");
        req.headers_mut().remove("x-real-ip");
    }

    req.extensions_mut().insert(ClientInfo {
        ip,
        scheme,
        forwarded_ignored,
    });
    next.run(req).await
}

/// 会话加载：解析 Cookie → 校验 → 注入 `CurrentUser`；首登强制改密（§5.2）。
pub async fn load_session(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    // `Cookie` 在 HTTP/2 里允许拆成多个头字段（RFC 9113 §8.1.2.5），hyper 不会自动
    // 合并，`get()` 只返回第一个。这里把全部字段拼起来再解析，避免会话 Cookie 恰好
    // 落在第二个字段时被当成「未登录」。
    let cookie_header: Vec<&str> = req
        .headers()
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    let cookie_header = cookie_header.join("; ");
    let cookies = parse_cookies(if cookie_header.is_empty() {
        None
    } else {
        Some(cookie_header.as_str())
    });
    if let Some(token) = cookies.get(state.cookie_name()) {
        let hash = skiff_identity::sha256_hex(token);
        if let Ok(Some(session)) = state
            .db()
            .verify_session(&hash, state.cfg.session.idle_secs)
        {
            if let Ok(Some(user)) = state.db().user_by_id(session.user_id) {
                let current = CurrentUser {
                    user,
                    session_id: session.id.clone(),
                    session_token: token.clone(),
                    scopes: None,
                };
                let needs_change = current.must_change_password();
                req.extensions_mut().insert(current.clone());

                if needs_change {
                    let path = req.uri().path().to_string();
                    let prefix = &state.cfg.entry_prefix;
                    let allowed = path.starts_with(&format!("/{prefix}/static/"))
                        || path.starts_with("/api/")
                        || path == format!("/{prefix}/login")
                        || path == format!("/{prefix}/password")
                        || path == format!("/{prefix}/logout")
                        || path == format!("/{prefix}/settings");
                    if !allowed {
                        return AppError::Redirect(format!("/{prefix}/settings?force=1"))
                            .into_response();
                    }
                }
            }
        }
    }
    next.run(req).await
}

/// 已登录用户提取器。未登录时：页面请求 303 到登录页，API 请求 401。
pub struct AuthUser(pub CurrentUser);

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        if let Some(u) = parts.extensions.get::<CurrentUser>() {
            return Ok(AuthUser(u.clone()));
        }
        // API Token（Bearer）路径
        if let Some(auth) = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(tok) = auth.strip_prefix("Bearer ") {
                let token = tok.trim().to_string();
                let hash = skiff_identity::sha256_hex(&token);
                if let Ok(Some((user, scopes))) = state.db().verify_api_token(&hash) {
                    // scope 解析是严格的（§6.1）：拼错的 token 必须立刻失败并留审计，
                    // 而不是被当成「空权限」或（更糟）「全权限」。
                    match ScopeSet::parse(&scopes) {
                        Ok(set) => {
                            return Ok(AuthUser(CurrentUser {
                                user,
                                session_id: "api-token".into(),
                                session_token: token,
                                scopes: Some(set),
                            }))
                        }
                        Err(e) => {
                            let ip = parts
                                .extensions
                                .get::<ClientInfo>()
                                .map(|c| c.ip.clone())
                                .unwrap_or_else(|| "unknown".into());
                            let _ = state.db().audit(
                                &user.username,
                                &ip,
                                "auth.token_scope",
                                &user.username,
                                "deny",
                                &e,
                            );
                            return Err(AppError::Unauthorized(format!("API Token 配置非法：{e}")));
                        }
                    }
                }
                // API 登录签发的会话令牌同样可作为 Bearer 使用
                if let Ok(Some(session)) = state
                    .db()
                    .verify_session(&hash, state.cfg.session.idle_secs)
                {
                    if let Ok(Some(user)) = state.db().user_by_id(session.user_id) {
                        return Ok(AuthUser(CurrentUser {
                            user,
                            session_id: session.id,
                            session_token: token,
                            scopes: None,
                        }));
                    }
                }
            }
        }
        if parts.uri.path().starts_with("/api/") {
            Err(AppError::Unauthorized("需要登录".into()))
        } else {
            let next = parts
                .uri
                .path_and_query()
                .map(|p| p.as_str().to_string())
                .unwrap_or_default();
            Err(AppError::Redirect(format!(
                "/{}/login?next={}",
                state.cfg.entry_prefix,
                // Location 头要的是 URL 编码，不是 HTML 转义
                crate::util::encode_uri_component(&next)
            )))
        }
    }
}

/// 权限提取器的公共实现：判定走 `skiff_model::Permission` 这张唯一矩阵。
///
/// 此前这里写的是 `role == Admin`，与 `model::rbac` 的矩阵、`containers.rs` 的
/// `is_admin() || role == Operator` 各写一份；现在全部收敛到一处。
async fn require_permission_extractor(
    parts: &mut axum::http::request::Parts,
    state: &Arc<AppState>,
    perm: Permission,
) -> Result<CurrentUser, AppError> {
    let AuthUser(u) = AuthUser::from_request_parts(parts, state).await?;
    if u.can(perm) {
        Ok(u)
    } else {
        Err(AppError::Forbidden(format!(
            "角色 {} 无权执行 {}",
            u.role(),
            perm.as_str()
        )))
    }
}

/// 仅 admin 可用的提取器（§6.3：危险操作 admin 专属）。
pub struct AdminUser(pub CurrentUser);

impl FromRequestParts<Arc<AppState>> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(AdminUser(
            require_permission_extractor(parts, state, Permission::Admin).await?,
        ))
    }
}

/// 插件管理权限的提取器（安装 / 启停 / 卸载 / RPC）。
///
/// 语义上对应 `Permission::PluginManage` 而不是笼统的 admin——矩阵放开
/// 「谁能管插件」时，这里自动跟随，不必再改一遍路由。
pub struct PluginManagerUser(pub CurrentUser);

impl FromRequestParts<Arc<AppState>> for PluginManagerUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(PluginManagerUser(
            require_permission_extractor(parts, state, Permission::PluginManage).await?,
        ))
    }
}

/// CSRF 校验：双提交令牌 = sha256(secret || session token)，随表单/头回传（§6.2）。
pub fn check_csrf(state: &AppState, user: &CurrentUser, provided: &str) -> Result<(), AppError> {
    let expected = state.csrf_token(&user.session_token);
    if skiff_identity::token_matches(&expected, provided) {
        Ok(())
    } else {
        Err(AppError::Forbidden("CSRF 校验失败，请刷新页面重试".into()))
    }
}
