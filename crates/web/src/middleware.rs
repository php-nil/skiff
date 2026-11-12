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
use skiff_app::db_async;
use skiff_model::{Form, Permission, ScopeSet};

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

/// 文件能力面门禁（§5.1/§5.4）：`SKIFF_FORM=docker` 的能力面只有 docker/identity，
/// 内核不提供 fs。路由层（`/{p}/files*`）与菜单深链（`/{p}/m/core/files*`）共用这处判定——
/// 受限形态下既不能渲染入口，也不能让直达 URL 借道列出容器内 `/`、`/www`。
pub fn check_fs_capability(state: &AppState) -> Result<(), AppError> {
    if state.cfg.form == Form::DockerRestricted {
        return Err(AppError::Forbidden(
            "当前部署形态不提供文件管理能力（1Panel 受限形态能力面仅 docker/identity）".into(),
        ));
    }
    Ok(())
}

/// `check_fs_capability` 的中间件形态：只挂在文件管理路由组上。
pub async fn fs_capability(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Err(e) = check_fs_capability(&state) {
        return e.into_response();
    }
    next.run(req).await
}

/// 安全入口（§6.3）：非前缀一律 404（不是 403），只作隐蔽入口、不替代认证。
pub async fn entry_guard(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if !is_exempt(&path) {
        let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
        let prefix_ok = first == state.cfg.entry_prefix
            || state.cfg.entry_prefix_previous.as_deref() == Some(first)
            || (first == "api" && path.starts_with("/api/v1/"))
            // `/ui/v1` 是 budding 前端的 JSON 命名空间，与 `/api/v1` 并列放行；
            // `/ui/` 下其它形态仍一律 404（URL 契约 §2）。
            || (first == "ui" && path.starts_with("/ui/v1/"));
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
        // 这里必须区分「没有会话」与「查不到库」：前者按未登录继续，后者是依赖故障。
        // 旧实现用 `if let Ok(Some(..))` 把 Err 一并吞成未登录——数据库挂掉时所有
        // 已登录用户被静默踢回登录页，日志里一条痕迹都没有。
        //
        // 同步 SQLite 投递到有界阻塞线程（§15）；错误与状态码口径保持不变：
        // 业务 Err 与投递失败都按「依赖故障」返回 503。
        let session = {
            let db = Arc::clone(state.db());
            let hash = hash.clone();
            let idle = state.cfg.session.idle_secs;
            match db_async::run(move || db.verify_session(&hash, idle)).await {
                Ok(Ok(session)) => session,
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "校验会话失败，数据库不可用");
                    return AppError::Unavailable("会话校验失败：数据库暂不可用".into())
                        .into_response();
                }
                Err(e) => {
                    tracing::error!(error = %e, "投递会话校验失败");
                    return AppError::Unavailable("会话校验失败：数据库暂不可用".into())
                        .into_response();
                }
            }
        };
        if let Some(session) = session {
            let user = {
                let db = Arc::clone(state.db());
                let uid = session.user_id;
                match db_async::run(move || db.user_by_id(uid)).await {
                    Ok(Ok(user)) => user,
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "读取会话用户失败，数据库不可用");
                        return AppError::Unavailable("会话校验失败：数据库暂不可用".into())
                            .into_response();
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "投递会话用户查询失败");
                        return AppError::Unavailable("会话校验失败：数据库暂不可用".into())
                            .into_response();
                    }
                }
            };
            // 会话指向的用户已被删除：按未登录处理（不是故障）。
            if let Some(user) = user {
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
                        // JSON 命名空间只出状态码与信封，不能 303 到页面。
                        || path.starts_with("/ui/")
                        || path == format!("/{prefix}/login")
                        || path == format!("/{prefix}/password")
                        || path == format!("/{prefix}/logout")
                        || path == format!("/{prefix}/settings")
                        // 旧版设置页承载口令 / TOTP 表单，强改密期间必须可达。
                        || path.starts_with(&format!("/{prefix}/m/core/settings/"));
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
                // 与 Cookie 路径同口径：查询失败（Err）是依赖故障，必须 503 + 日志，
                // 不能和「token 不存在」（Ok(None)）一起掉进「需要登录」。
                let token_lookup = {
                    let db = Arc::clone(state.db());
                    let hash = hash.clone();
                    db_async::run(move || db.verify_api_token(&hash)).await
                };
                match token_lookup {
                    Ok(Ok(Some((user, scopes)))) => {
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
                                let db = Arc::clone(state.db());
                                let actor = user.username.clone();
                                let target = user.username.clone();
                                let detail = e.clone();
                                let audited = db_async::run(move || {
                                    db.audit(
                                        &actor,
                                        &ip,
                                        "auth.token_scope",
                                        &target,
                                        "deny",
                                        &detail,
                                    )
                                })
                                .await;
                                match audited {
                                    Ok(Ok(())) => {}
                                    Ok(Err(ae)) => {
                                        tracing::error!(error = %ae, "API Token scope 拒绝审计写入失败")
                                    }
                                    Err(ae) => {
                                        tracing::error!(error = %ae, "投递 API Token 审计失败")
                                    }
                                }
                                return Err(AppError::Unauthorized(format!(
                                    "API Token 配置非法：{e}"
                                )));
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "校验 API Token 失败，数据库不可用");
                        return Err(AppError::Unavailable(
                            "API Token 校验失败：数据库暂不可用".into(),
                        ));
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "投递 API Token 校验失败");
                        return Err(AppError::Unavailable(
                            "API Token 校验失败：数据库暂不可用".into(),
                        ));
                    }
                }
                // API 登录签发的会话令牌同样可作为 Bearer 使用
                let session_lookup = {
                    let db = Arc::clone(state.db());
                    let hash = hash.clone();
                    let idle = state.cfg.session.idle_secs;
                    db_async::run(move || db.verify_session(&hash, idle)).await
                };
                match session_lookup {
                    Ok(Ok(Some(session))) => {
                        let uid = session.user_id;
                        let user = {
                            let db = Arc::clone(state.db());
                            db_async::run(move || db.user_by_id(uid)).await
                        };
                        match user {
                            Ok(Ok(Some(user))) => {
                                return Ok(AuthUser(CurrentUser {
                                    user,
                                    session_id: session.id,
                                    session_token: token,
                                    scopes: None,
                                }))
                            }
                            // 令牌有效但用户已删除：按未登录处理。
                            Ok(Ok(None)) => {}
                            Ok(Err(e)) => {
                                tracing::error!(error = %e, "读取令牌用户失败，数据库不可用");
                                return Err(AppError::Unavailable(
                                    "会话校验失败：数据库暂不可用".into(),
                                ));
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "投递令牌用户查询失败");
                                return Err(AppError::Unavailable(
                                    "会话校验失败：数据库暂不可用".into(),
                                ));
                            }
                        }
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "校验会话令牌失败，数据库不可用");
                        return Err(AppError::Unavailable("会话校验失败：数据库暂不可用".into()));
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "投递会话令牌校验失败");
                        return Err(AppError::Unavailable("会话校验失败：数据库暂不可用".into()));
                    }
                }
            }
        }
        if parts.uri.path().starts_with("/api/") || parts.uri.path().starts_with("/ui/v1/") {
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

/// `/ui/v1/*` 的认证提取器：判定复用 `AuthUser`（Cookie / Bearer 同一套），
/// 但把失败统一映射成 JSON 信封（401/403/503），**不产生页面 303**（URL 契约 §6）。
pub struct UiAuthUser(pub CurrentUser);

impl FromRequestParts<Arc<AppState>> for UiAuthUser {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        match AuthUser::from_request_parts(parts, state).await {
            Ok(AuthUser(user)) => Ok(UiAuthUser(user)),
            Err(e) => Err(crate::error::ApiError(e)),
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
        Err(AppError::Csrf("CSRF 校验失败，请刷新页面重试".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// SDUI 切换前的护栏（D12）：全局 CSP 只允许保持现状，任何放宽都必须先改这里
    /// 并走评审。字符串按指令顺序逐字写死，改一个字符就会失败。
    const EXPECTED_GLOBAL_CSP: &str = "default-src 'self'; script-src 'self'; \
         style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
         frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'";

    /// 指令 → 来源列表，用于逐条比对（顺序无关）。
    fn directives(policy: &str) -> BTreeMap<String, String> {
        policy
            .split(';')
            .filter_map(|d| {
                let d = d.trim();
                if d.is_empty() {
                    return None;
                }
                let mut parts = d.split_whitespace();
                let name = parts.next()?.to_string();
                Some((name, parts.collect::<Vec<_>>().join(" ")))
            })
            .collect()
    }

    /// 下发给浏览器的头必须与常量完全一致，不允许中间件里再拼一次。
    #[test]
    fn global_csp_header_matches_constant() {
        assert_eq!(CSP, EXPECTED_GLOBAL_CSP);
        assert_eq!(csp_header().to_str().unwrap(), EXPECTED_GLOBAL_CSP);
    }

    /// 逐条比对现有指令集：数量、名称、来源都不许变。
    #[test]
    fn global_csp_directives_are_not_loosened() {
        let got = directives(CSP);
        assert_eq!(got, directives(EXPECTED_GLOBAL_CSP));
        assert_eq!(got.len(), 9, "指令数量变化需评审: {got:?}");
        for name in [
            "default-src",
            "script-src",
            "style-src",
            "img-src",
            "connect-src",
            "frame-ancestors",
            "base-uri",
            "form-action",
            "object-src",
        ] {
            assert!(got.contains_key(name), "缺少指令 {name}");
        }

        assert_eq!(got["default-src"], "'self'");
        assert_eq!(got["script-src"], "'self'", "脚本不得放开内联或外链");
        assert_eq!(got["connect-src"], "'self'", "连接只能同源");
        assert_eq!(got["frame-ancestors"], "'none'");
        assert_eq!(got["base-uri"], "'none'");
        assert_eq!(got["form-action"], "'self'");
        assert_eq!(got["object-src"], "'none'");
        // 现有样式内联是 Vuetify 之前的页面需要的，保持原样即可
        assert_eq!(got["style-src"], "'self' 'unsafe-inline'");

        assert!(!CSP.contains("'unsafe-eval'"), "不得放开 eval");
        assert!(!CSP.contains("'unsafe-hashes'"));
        assert!(!CSP.contains('*'), "不得出现通配来源");
        assert!(!CSP.contains("http:") && !CSP.contains("https:"));
        assert!(!CSP.contains("blob:"));
    }
}
