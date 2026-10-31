//! 请求处理器：各页面的 SSR 与 JSON API，以及跨 handler 的共享工具
//! （页面上下文与渲染、表单/Cookie/请求头解析、`actor` 与 `resolve_container`）。

pub mod api;
pub mod auth;
pub mod containers;
pub mod dashboard;
pub mod meta;
pub mod plugins;
pub mod resources;
pub mod settings;

use std::collections::HashMap;

use askama::Template;
use axum::http::header::{CONTENT_TYPE, COOKIE, HOST};
use axum::http::request::Parts;
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::error::AppError;
use crate::state::{AppState, ClientInfo, CurrentUser};
use crate::templates::PageCtx;
use crate::util::{form_first, parse_form};
use skiff_store::Role;

pub fn page_ctx(state: &AppState, user: &CurrentUser, active: &str) -> PageCtx {
    PageCtx {
        prefix: state.cfg.entry_prefix.clone(),
        username: user.user.username.clone(),
        role: user.role().to_string(),
        csrf: state.csrf_token(&user.session_token),
        dev_http: state.cfg.dev_http,
        version: state.version.clone(),
        docker_ok: state.docker.available(),
        must_change: user.must_change_password(),
        active: active.to_string(),
    }
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

/// 登录页 CSRF：双提交 Cookie（匿名会话）。
pub const LOGIN_CSRF_COOKIE: &str = "skiff_login";

pub fn simple_cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    let mut c = format!("{name}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    c
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

pub fn require_admin(user: &CurrentUser) -> Result<(), AppError> {
    if user.role() == Role::Admin {
        Ok(())
    } else {
        Err(AppError::Forbidden("该操作仅限管理员".into()))
    }
}

pub fn actor(user: &CurrentUser) -> skiff_docker::Actor {
    skiff_docker::Actor::User {
        username: user.user.username.clone(),
        role: user.role(),
    }
}

/// 容器解析（id 全称 / 名称 / id 前缀）：全仓唯一实现。
/// 替换掉此前散在 containers.rs / api.rs 的 6 处相同闭包——改匹配规则只改这里。
pub async fn resolve_container(
    state: &AppState,
    id: &str,
) -> Result<skiff_docker::client::ContainerRow, AppError> {
    let containers = state
        .docker
        .list_containers()
        .await
        .map_err(|e| AppError::Unavailable(e.to_string()))?;
    match_container(containers, id)
}

/// 纯匹配逻辑（与 Docker 列表解耦，便于单测）。
///
/// 顺序是语义的一部分：**精确 id / 名称优先于前缀**。此前单趟 `find` 让列表顺序决定结果，
/// 会出现「查 `abc` 却操作了 `abc123`」——精确命中被前面的前缀命中吃掉。前缀不唯一时
/// 报歧义而不是静默取第一个：容器操作是破坏性的，猜错比拒绝更糟。
pub fn match_container(
    containers: Vec<skiff_docker::client::ContainerRow>,
    id: &str,
) -> Result<skiff_docker::client::ContainerRow, AppError> {
    if id.is_empty() {
        return Err(AppError::BadRequest("容器引用不能为空".into()));
    }
    // 阶段一：精确 id / 名称。必须扫完整份列表才能进下一阶段，否则「前缀命中在前、
    // 精确命中在后」时仍会先返回前缀那个。
    if let Some(pos) = containers.iter().position(|c| c.id == id || c.name == id) {
        return Ok(containers[pos].clone());
    }
    // 阶段二：id 前缀，且要求唯一
    let mut hits = containers.iter().filter(|c| c.id.starts_with(id));
    let first = hits
        .next()
        .ok_or_else(|| AppError::NotFound(format!("容器不存在: {id}")))?;
    if hits.next().is_some() {
        return Err(AppError::BadRequest(format!(
            "容器引用有歧义，请提供更长的 id 前缀: {id}"
        )));
    }
    Ok(first.clone())
}

pub fn container_view(
    c: &skiff_docker::client::ContainerRow,
) -> crate::templates::ContainerRowView {
    crate::templates::ContainerRowView {
        id: c.id.clone(),
        short_id: c.id.chars().take(12).collect(),
        name: c.name.clone(),
        image: crate::util::truncate(&c.image, 60),
        state: c.state.clone(),
        status: crate::util::truncate(&c.status, 60),
        project: c.project.clone().unwrap_or_default(),
        protected: c.protected,
    }
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
    use skiff_docker::client::ContainerRow;

    fn row(id: &str, name: &str) -> ContainerRow {
        ContainerRow {
            id: id.into(),
            name: name.into(),
            image: "img".into(),
            state: "running".into(),
            status: "Up".into(),
            labels: Default::default(),
            project: None,
            owner: None,
            protected: false,
        }
    }

    /// 列表顺序里前缀命中的排在精确命中之前，必须仍返回精确的那个。
    #[test]
    fn exact_id_wins_over_earlier_prefix_hit() {
        let list = vec![row("abc1234", "web"), row("abc", "api")];
        let got = match_container(list, "abc").unwrap();
        assert_eq!(got.id, "abc");
    }

    #[test]
    fn exact_name_match_wins() {
        let list = vec![row("aaa111", "api"), row("bbb222", "web")];
        let got = match_container(list, "web").unwrap();
        assert_eq!(got.id, "bbb222");
    }

    #[test]
    fn unique_prefix_resolves() {
        let list = vec![row("deadbeef", "web"), row("cafe0000", "api")];
        let got = match_container(list, "dead").unwrap();
        assert_eq!(got.id, "deadbeef");
    }

    /// 前缀歧义必须拒绝：猜错会把操作打到另一个容器上。
    #[test]
    fn ambiguous_prefix_is_rejected() {
        let list = vec![row("abc111", "web"), row("abc222", "api")];
        let err = match_container(list, "abc").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)), "实际: {err:?}");
    }

    #[test]
    fn unknown_reference_is_not_found() {
        let list = vec![row("abc111", "web")];
        assert!(matches!(
            match_container(list, "zzz").unwrap_err(),
            AppError::NotFound(_)
        ));
    }

    #[test]
    fn empty_reference_is_rejected() {
        let list = vec![row("abc111", "web")];
        assert!(matches!(
            match_container(list, "").unwrap_err(),
            AppError::BadRequest(_)
        ));
    }

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
