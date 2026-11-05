//! 请求处理器：各页面的 SSR 与 JSON API，以及跨 handler 的共享工具
//! （页面上下文与渲染、表单/Cookie/请求头解析、`actor` 与 `resolve_container`）。

pub mod api;
pub mod auth;
pub mod containers;
pub mod dashboard;
pub mod docker_settings;
pub mod menu;
pub mod meta;
pub mod nginx;
pub mod plugins;
pub mod resources;
pub mod settings;
pub mod sites;
pub mod tasks;

use std::collections::HashMap;

use askama::Template;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST};
use axum::http::request::Parts;
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::error::AppError;
use crate::state::{AppState, ClientInfo, CurrentUser};
use crate::templates::PageCtx;
use crate::util::{form_first, parse_form};

pub fn page_ctx(state: &AppState, user: &CurrentUser, active: &str) -> PageCtx {
    let nav_plugins = nav_plugins(state, active);
    let is_admin = user.can(skiff_model::Permission::Admin);
    PageCtx {
        prefix: state.cfg.entry_prefix.clone(),
        username: user.user.username.clone(),
        role: user.role().to_string(),
        csrf: state.csrf_token(&user.session_token),
        dev_http: state.cfg.dev_http,
        version: state.version.clone(),
        docker_ok: state.docker().available(),
        must_change: user.must_change_password(),
        active: active.to_string(),
        menu: crate::menu::build(state, user, active),
        is_admin,
        nav_plugins,
    }
}

/// 顶栏「应用」菜单的数据源：把 manifest `[[ui]].label` 直接挂到导航栏，
/// 省得每次切换都经「插件中心」中转。
///
/// 只列**已启用且有 UI 入口**的插件。DB 读失败或某个 manifest 解析失败时跳过该项
/// （导航少几项不影响页面本身渲染，坏包也不该拖垮整站导航）。
pub fn nav_plugins(state: &AppState, active: &str) -> Vec<crate::templates::PluginNav> {
    let Ok(list) = state.db().list_plugins() else {
        return Vec::new();
    };
    list.iter()
        .filter(|p| p.enabled)
        .filter_map(|p| {
            let m = skiff_pkgfmt::Manifest::parse(&p.manifest).ok()?;
            let ui = m.ui.first()?;
            let label = if ui.label.is_empty() {
                p.name.clone()
            } else {
                ui.label.clone()
            };
            Some(crate::templates::PluginNav {
                // 详情页把 active 记作 `plugin:<id>`，其余页面不会命中。
                active: active == format!("plugin:{}", p.id),
                id: p.id.clone(),
                label,
            })
        })
        .collect()
}

pub fn render<T: Template>(t: T) -> Result<Response, AppError> {
    let body = t.render().map_err(|e| AppError::Internal(e.to_string()))?;
    Ok((
        [
            ("cache-control", "no-store"),
            ("content-type", "text/html; charset=utf-8"),
        ],
        Html(body),
    )
        .into_response())
}

pub fn redirect(to: impl Into<String>) -> Response {
    Redirect::to(&to.into()).into_response()
}

/// 解析表单体。大小上限由 axum 默认 body 限制（2MB）兜底——表单只承载 CSRF 与少量字段，
/// 远超所需；插件包上传走独立路由并显式抬升上限（见 `router()` 里的 `DefaultBodyLimit`）。
pub async fn read_form(body: String) -> HashMap<String, Vec<String>> {
    parse_form(&body)
}

pub fn field(form: &HashMap<String, Vec<String>>, key: &str) -> String {
    form_first(form, key)
}

pub fn client_ip(state: &AppState, parts: &Parts) -> String {
    let _ = state;
    parts
        .extensions
        .get::<ClientInfo>()
        .map(|c| c.ip.clone())
        .unwrap_or_else(|| "unknown".into())
}

pub fn htmx_request(parts: &Parts) -> bool {
    parts
        .headers
        .get("hx-request")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// API scope 闸门（§6.1）：token 权限 = 角色权限 ∩ scope。
/// 会话认证（`scopes == None`）不受影响，角色限制仍由 `policy.check` 单独负责。
pub fn require_scope(user: &CurrentUser, scope: skiff_model::Scope) -> Result<(), AppError> {
    if user.allows_scope(scope) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "该 API Token 缺少 {scope} scope"
        )))
    }
}

/// CSRF 校验（请求头形式）：`fetch` 类调用把令牌放 `x-csrf-token`，不塞进 JSON 体。
/// 与表单路径共用 `check_csrf`，判定只有一份。
pub fn check_csrf_header(
    state: &AppState,
    user: &CurrentUser,
    parts: &Parts,
) -> Result<(), AppError> {
    let provided = parts
        .headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    crate::middleware::check_csrf(state, user, provided)
}

pub fn is_secure_request(state: &AppState, parts: &Parts) -> bool {
    if state.cfg.dev_http {
        return false;
    }
    match parts.extensions.get::<ClientInfo>() {
        Some(ci) => ci.scheme == "https",
        None => true,
    }
}

/// WebSocket `Origin` 校验（§6.2：必须与面板预期 origin 完全一致）。
///
/// 三个收紧点，都是原先「`if let Some(origin)` 才校验」留下的口子：
/// - **缺失 Origin 直接拒绝**。浏览器按 RFC 6455 必定发送该头，缺失只可能来自非浏览器
///   客户端——对 `exec` 这种拿容器 root shell 的入口，不该把「没带」当成「同源」。
/// - `http://{host}` 仅在 `dev_http` 开发模式下接受；生产环境接受明文 origin，等于允许
///   一个跑在 http 上的页面驱动 wss 终端。
/// - 配置了 `external_url` 时它是权威来源，`Host` 头只用于同源候选。
pub fn ws_origin_allowed(
    origin: Option<&str>,
    host: &str,
    external_url: Option<&str>,
    dev_http: bool,
) -> bool {
    let Some(origin) = origin.filter(|o| !o.is_empty()) else {
        return false;
    };
    if let Some(ext) = external_url {
        let ext = ext.trim_end_matches('/');
        if !ext.is_empty() && origin == ext {
            return true;
        }
    }
    if host.is_empty() {
        return false;
    }
    origin == format!("https://{host}") || (dev_http && origin == format!("http://{host}"))
}

/// 登录页 CSRF：签名表单 token 为主、Cookie 仅作兼容信号。
pub const LOGIN_CSRF_COOKIE: &str = "skiff_login";

pub fn simple_cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    let mut c = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    c
}

/// 登录 POST 的来源判定（返回 `None` 表示浏览器未提供任何来源信息）。
///
/// 优先看 `Sec-Fetch-Site`：`same-origin`/`none` 放行，`cross-site`/`same-site`
/// 拒绝；没有该头时退回 `Origin` / `Referer` 与当前 Host 比较。这样即使
/// `skiff_login` Cookie 被浏览器丢弃，签名表单 token + 同源判定仍能登录；
/// 而跨站表单即使带着合法 token 也会被拒。
pub fn login_origin_allowed(state: &AppState, parts: &Parts) -> Option<bool> {
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
    };
    if let Some(site) = header("sec-fetch-site") {
        return Some(matches!(site, "same-origin" | "none"));
    }
    if let Some(origin) = header("origin") {
        return Some(origin_is_panel(
            state,
            origin.trim_end_matches('/'),
            &host_header(parts),
        ));
    }
    if let Some(referer) = header("referer") {
        if let Some((scheme, rest)) = referer.split_once("://") {
            let host = rest.split('/').next().unwrap_or("");
            if host.is_empty() {
                return Some(false);
            }
            return Some(origin_is_panel(
                state,
                &format!("{scheme}://{host}"),
                &host_header(parts),
            ));
        }
        return Some(false);
    }
    None
}

fn origin_is_panel(state: &AppState, origin: &str, host: &str) -> bool {
    if let Some(ext) = state.cfg.external_url.as_deref() {
        let ext = ext.trim_end_matches('/');
        if !ext.is_empty() && origin == ext {
            return true;
        }
    }
    !host.is_empty()
        && (origin == format!("https://{host}")
            || (state.cfg.dev_http && origin == format!("http://{host}")))
}

pub fn cookie_header(name: &str, value: &str) -> (axum::http::HeaderName, String) {
    (COOKIE, format!("{name}={value}"))
}

pub fn host_header(parts: &Parts) -> String {
    parts
        .headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

pub fn content_type(parts: &Parts) -> String {
    parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// 角色→权限判定：查 `skiff_model::Permission` 这张唯一的矩阵。
///
/// 这里**不再**有「仅限管理员」之类的独立判断——那类判断此前散在
/// `require_admin`（粗粒度 admin）、`containers.rs` 的 `is_admin() || role == Operator`
/// （UI 按钮显隐）三处，各改各的。现在统一问 `Permission`：
/// 判定层（`skiff_policy::Policy::check`）与 UI 层（按钮显隐）读同一张表。
pub fn require_permission(
    user: &CurrentUser,
    perm: skiff_model::Permission,
) -> Result<(), AppError> {
    if user.can(perm) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "角色 {} 无权执行 {}",
            user.role(),
            perm.as_str()
        )))
    }
}

pub fn actor(user: &CurrentUser) -> skiff_docker::Actor {
    skiff_docker::Actor::User {
        username: user.user.username.clone(),
        role: user.role(),
    }
}

/// 容器解析（id 全称 / 名称 / id 前缀）：匹配规则在 `skiff-docker::resolve`，
/// 这里只负责取列表并把领域错误翻成 HTTP 错误。
pub async fn resolve_container(
    state: &AppState,
    id: &str,
) -> Result<skiff_docker::client::ContainerRow, AppError> {
    let containers = state
        .docker()
        .list_containers()
        .await
        .map_err(|e| AppError::Unavailable(e.to_string()))?;
    skiff_docker::match_container(&containers, id).map_err(|e| match e {
        skiff_docker::MatchError::Empty => AppError::BadRequest(e.to_string()),
        skiff_docker::MatchError::NotFound(_) => AppError::NotFound(e.to_string()),
        skiff_docker::MatchError::Ambiguous(_) => AppError::BadRequest(e.to_string()),
    })
}

pub fn container_view(
    c: &skiff_docker::client::ContainerRow,
) -> crate::templates::ContainerRowView {
    crate::templates::ContainerRowView {
        id: c.id.clone(),
        short_id: c.short_id(),
        name: c.name.clone(),
        image: crate::util::truncate(&c.image, 60),
        state: c.state.clone(),
        status: crate::util::truncate(&c.status, 60),
        project: c.project.clone().unwrap_or_default(),
        protected: c.protected,
    }
}

/// 「异常容器」的唯一判定：概览待办、异常列表与 `?filter=error` 共用。
///
/// `created` 是尚未启动的新容器，不算异常；除 `running` / `created` 外的
/// exited / dead / restarting / paused / removing 都需要人工确认。
pub fn container_abnormal(state: &str) -> bool {
    !matches!(state, "running" | "created")
}

pub fn object_ref(c: &skiff_docker::client::ContainerRow) -> skiff_docker::ObjectRef {
    skiff_docker::ObjectRef {
        id: c.id.clone(),
        name: c.name.clone(),
        labels: c.labels.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_abnormal_excludes_running_and_created() {
        assert!(!container_abnormal("running"));
        assert!(!container_abnormal("created"));
        for state in ["exited", "dead", "restarting", "paused", "removing"] {
            assert!(container_abnormal(state), "{state} 应判为异常");
        }
    }

    // 容器引用匹配（精确优先 / 前缀歧义 / 空引用）的用例在
    // `skiff-docker::resolve`——规则下沉到领域 crate 后，判定与用例同处一地。

    // ---- WebSocket Origin 校验 ----

    #[test]
    fn ws_rejects_missing_origin() {
        // 缺失 Origin 不能当成同源：exec 入口拿的是容器 root shell
        assert!(!ws_origin_allowed(None, "panel.example.com", None, false));
        assert!(!ws_origin_allowed(
            Some(""),
            "panel.example.com",
            None,
            true
        ));
    }

    #[test]
    fn ws_accepts_same_origin_https() {
        assert!(ws_origin_allowed(
            Some("https://panel.example.com"),
            "panel.example.com",
            None,
            false
        ));
    }

    #[test]
    fn ws_rejects_http_origin_outside_dev_mode() {
        assert!(!ws_origin_allowed(
            Some("http://panel.example.com"),
            "panel.example.com",
            None,
            false
        ));
        assert!(ws_origin_allowed(
            Some("http://127.0.0.1:8080"),
            "127.0.0.1:8080",
            None,
            true
        ));
    }

    #[test]
    fn ws_rejects_foreign_origin() {
        assert!(!ws_origin_allowed(
            Some("https://evil.example"),
            "panel.example.com",
            None,
            false
        ));
    }

    #[test]
    fn ws_accepts_configured_external_url_and_tolerates_trailing_slash() {
        assert!(ws_origin_allowed(
            Some("https://panel.example.com"),
            "127.0.0.1:8080",
            Some("https://panel.example.com/"),
            false
        ));
    }
}
