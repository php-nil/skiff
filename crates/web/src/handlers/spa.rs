//! SPA 壳页回落（URL 契约 §2.1；SDUI 集成 D13）。
//!
//! 只由 `Router::fallback` 触发，即**所有已注册路由都没匹配上**时才进入：
//! 仅 `GET` + 命中白名单前缀（按路径段边界，`/{p}/sitesfoo` 不算 `/{p}/sites`）
//! 才进入页面分支。未认证一律 303 到登录页（与 `/files` 等注册路由一致，不先看
//! `Accept`）；已认证仍需 `Accept` 含 `text/html` 才给壳页，其余沿用 404。
//! 写请求永不回落。
//!
//! 壳页本批只给挂载点、占位提示与产物 script 引用：产物入库由另一路负责，
//! 这里不复制静态文件，也不改 `assets.rs` / `build.rs`。

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::ACCEPT;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use crate::error::AppError;
use crate::state::{AppState, CurrentUser};
use crate::util::escape_html;

/// 可回落到壳页的页面前缀（不含入口根，根单独精确匹配）。
/// 与 `docs/URL_CONTRACT.md` §2.1 的表一一对应，增删必须同步契约与测试。
const SHELL_PAGES: &[&str] = &[
    "sites",
    "containers",
    "images",
    "networks",
    "volumes",
    "compose",
    "runtimes",
    "files",
    "plugins",
    "store",
    "settings",
    "audit",
    "tasks",
    "jobs",
];

/// 路径是否命中壳页白名单（按路径段边界匹配）。
pub fn is_shell_path(prefix: &str, path: &str) -> bool {
    let root = format!("/{prefix}");
    if path == root {
        return true;
    }
    let Some(rest) = path
        .strip_prefix(root.as_str())
        .and_then(|r| r.strip_prefix('/'))
    else {
        return false;
    };
    let first = rest.split('/').next().unwrap_or_default();
    SHELL_PAGES.contains(&first)
}

/// `Accept` 是否接受 HTML（浏览器地址栏导航的标准请求头）。
pub(crate) fn accepts_html(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.to_ascii_lowercase().contains("text/html"))
}

/// 路由兜底：命中白名单返回壳页，否则保持原有的 404 行为。
pub async fn fallback(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    let path = parts.uri.path();
    if parts.method != Method::GET || !is_shell_path(&state.cfg.entry_prefix, path) {
        return AppError::NotFound("404 Not Found".into()).into_response();
    }
    // 未认证的 GET 不论 Accept 一律回登录页：`/files` 这类有注册路由的页面本来就
    // 303，而 `/settings` 只靠回落；若在认证前先卡 Accept，会让两组前缀行为分裂。
    if parts.extensions.get::<CurrentUser>().is_none() {
        return login_redirect(&state, &parts);
    }
    if !accepts_html(&parts.headers) {
        return AppError::NotFound("404 Not Found".into()).into_response();
    }
    shell_page(&state)
}

/// 「路径存在但方法不匹配」兜底（axum 默认直接 405，`Router::fallback` 不会触发）。
///
/// 白名单页面上的 GET 导航也可能撞到只注册了 POST 的路由模式，例如
/// `/p/containers/{id}/{action}`；这类深链同样应拿到壳页。写请求与非 HTML
/// `Accept` 仍保持 405，不回落。
pub async fn method_not_allowed(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    if parts.method == Method::GET && is_shell_path(&state.cfg.entry_prefix, parts.uri.path()) {
        if parts.extensions.get::<CurrentUser>().is_none() {
            return login_redirect(&state, &parts);
        }
        if accepts_html(&parts.headers) {
            return shell_page(&state);
        }
    }
    (StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed").into_response()
}

/// 入口根 `/`（概览）同样交付壳页：已登录渲染 budding 概览，未登录 303 到登录页。
/// 概览的 SSR 版本仍由 `/{p}/m/overview` 菜单深链提供，直到插件槽位迁完。
pub async fn root_shell(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    shell_for(&state, &parts)
}

/// 壳页交付：受会话保护，未登录仍按页面惯例 303 到登录页
/// （`/ui/v1/*` 的 401 只适用于 JSON）。
fn shell_for(state: &AppState, parts: &Parts) -> Response {
    if parts.extensions.get::<CurrentUser>().is_none() {
        return login_redirect(state, parts);
    }
    shell_page(state)
}

/// 未认证页面请求统一跳登录页，`next` 保留原始 path + query。
fn login_redirect(state: &AppState, parts: &Parts) -> Response {
    let next = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    AppError::Redirect(format!(
        "/{}/login?next={}",
        state.cfg.entry_prefix,
        crate::util::encode_uri_component(&next)
    ))
    .into_response()
}

/// 已认证请求的壳页响应（供带能力面门禁的页面复用，如 `/{p}/files`）。
pub fn shell_page(state: &AppState) -> Response {
    let body = shell_html(&state.cfg.entry_prefix);
    (
        [
            ("cache-control", "no-store"),
            ("content-type", "text/html; charset=utf-8"),
        ],
        Html(body),
    )
        .into_response()
}

/// 极简壳页：挂载点 + 占位提示 + 产物 script。
///
/// - 只用普通 `<script src>`（内联脚本会被全局 CSP `script-src 'self'` 拦掉，
///   且 budding 依赖 `document.currentScript` 推导目录）。
/// - 产物按 D11 引用版本化文件名并带 `?v=`；稳定别名只作兼容入口，壳页不引用。
/// - CSS 文件名是 budding 构建期写死的 `budding.css`，版本化后无法自动推导，
///   因此通过 data 属性传给启动脚本显式 `css` 选项。
/// - 图标字体自 1.8.0 起由 budding 官方包自带（`Budding Icons` 子集），单独版本化入库并由壳页 `<link>` 引入；
///   子集只覆盖组件内部图标，壳页不声明自定义菜单图标。
/// - 初始化参数（入口前缀、API 基址）挂在挂载点上，由外部启动脚本
///   `budding-boot.js` 读取，避免内联脚本；路由表由 `/ui/v1/menu` 下发。
pub fn shell_html(prefix: &str) -> String {
    let prefix = escape_html(prefix);
    let ver = crate::assets::BUDDING_VERSION;
    let icons = crate::assets::BUDDING_ICONS_VERSION;
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Skiff</title>
<link rel="icon" type="image/svg+xml" href="/{prefix}/static/favicon.svg?v=1">
<link rel="stylesheet" href="/{prefix}/static/budding/budding-icons-{icons}.min.css?v={icons}">
</head>
<body>
<div id="skiff-root" data-prefix="/{prefix}" data-api-base="/ui/v1" data-router-mode="history" data-budding-version="{ver}" data-budding-css="/{prefix}/static/budding/budding-{ver}.min.css?v={ver}">
  <main data-skiff-placeholder>
    <h1>该功能正在适配中</h1>
    <p>页面正在切换到新的前端运行时，当前为占位视图。</p>
  </main>
</div>
<script src="/{prefix}/static/budding/budding-{ver}.min.js?v={ver}"></script>
<script src="/{prefix}/static/budding/budding-boot.js"></script>
</body>
</html>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_matches_by_segment_boundary() {
        assert!(is_shell_path("p", "/p"));
        assert!(is_shell_path("p", "/p/sites"));
        assert!(is_shell_path("p", "/p/sites/example.com/anything"));
        assert!(is_shell_path("p", "/p/files/a/b"));
        assert!(is_shell_path("p", "/p/jobs/1/logs/x"));
        // 前缀相同但不是整段：不得命中
        assert!(!is_shell_path("p", "/p/sitesfoo"));
        assert!(!is_shell_path("p", "/pp/sites"));
        assert!(!is_shell_path("p", "/p/unknown"));
        assert!(!is_shell_path("p", "/p/"));
        // 其它前缀形态不是页面
        assert!(!is_shell_path("p", "/ui/v1/session"));
        assert!(!is_shell_path("p", "/healthz"));
        assert!(!is_shell_path("p", "/p/static/budding/budding.min.js"));
    }

    #[test]
    fn shell_html_references_versioned_artifacts_and_placeholder() {
        let html = shell_html("p");
        assert!(html.contains("该功能正在适配中"));
        let ver = crate::assets::BUDDING_VERSION;
        assert!(html.contains(&format!(
            r#"<script src="/p/static/budding/budding-{ver}.min.js?v={ver}"></script>"#
        )));
        assert!(html.contains(&format!(
            r#"data-budding-css="/p/static/budding/budding-{ver}.min.css?v={ver}""#
        )));
        assert!(html.contains(r#"<script src="/p/static/budding/budding-boot.js"></script>"#));
        let icons = crate::assets::BUDDING_ICONS_VERSION;
        assert!(html.contains(&format!(
            r#"<link rel="stylesheet" href="/p/static/budding/budding-icons-{icons}.min.css?v={icons}">"#
        )));
        assert!(
            html.contains(
                r#"<link rel="icon" type="image/svg+xml" href="/p/static/favicon.svg?v=1">"#
            ),
            "壳页必须显式声明 favicon，避免浏览器回落探测 404"
        );
        // 壳页不引用稳定别名（D11），也不许出现内联脚本或外部加载器。
        assert!(!html.contains("budding.min.js"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("dist/loader"));
    }
}
