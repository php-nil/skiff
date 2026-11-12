//! SPA 壳页回落（URL 契约 §2.1；SDUI 集成 D13）。
//!
//! 只由 `Router::fallback` 触发，即**所有已注册路由都没匹配上**时才进入：
//! 仅 `GET` + `Accept` 含 `text/html` + 命中白名单前缀（按路径段边界，
//! `/{p}/sitesfoo` 不算 `/{p}/sites`）三者同时满足才返回壳页；
//! 其余一律沿用原有 404 错误页。写请求永不回落。
//!
//! 壳页本批只给挂载点、占位提示与产物 script 引用：产物入库由另一路负责，
//! 这里不复制静态文件，也不改 `assets.rs` / `build.rs`。

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::ACCEPT;
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
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
fn accepts_html(parts: &Parts) -> bool {
    parts
        .headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.to_ascii_lowercase().contains("text/html"))
}

/// 路由兜底：命中白名单返回壳页，否则保持原有的 404 行为。
pub async fn fallback(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    let path = parts.uri.path();
    if parts.method != Method::GET
        || !accepts_html(&parts)
        || !is_shell_path(&state.cfg.entry_prefix, path)
    {
        return AppError::NotFound("404 Not Found".into()).into_response();
    }
    shell_for(&state, &parts)
}

/// 「路径存在但方法不匹配」兜底（axum 默认直接 405，`Router::fallback` 不会触发）。
///
/// 白名单页面上的 GET 导航也可能撞到只注册了 POST 的路由模式，例如
/// `/p/containers/{id}/{action}`；这类深链同样应拿到壳页。写请求与非 HTML
/// `Accept` 仍保持 405，不回落。
pub async fn method_not_allowed(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    if parts.method == Method::GET
        && accepts_html(&parts)
        && is_shell_path(&state.cfg.entry_prefix, parts.uri.path())
    {
        return shell_for(&state, &parts);
    }
    (StatusCode::METHOD_NOT_ALLOWED, "405 Method Not Allowed").into_response()
}

/// 壳页交付：受会话保护，未登录仍按页面惯例 303 到登录页
/// （`/ui/v1/*` 的 401 只适用于 JSON）。
fn shell_for(state: &AppState, parts: &Parts) -> Response {
    if parts.extensions.get::<CurrentUser>().is_none() {
        let next = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        return AppError::Redirect(format!(
            "/{}/login?next={}",
            state.cfg.entry_prefix,
            crate::util::encode_uri_component(&next)
        ))
        .into_response();
    }
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
/// - 只用普通 `<script src>`（内联脚本会被全局 CSP `script-src 'self'` 拦掉）；
///   budding 会按 `document.currentScript` 推导同目录的 `budding.css`，无需 `<link>`。
/// - 产物稳定别名是 `budding.min.js`；版本化文件名到位后按 D11 换成带 `?v=` 的版本化引用。
/// - 初始化参数（history 路由的 `routerBase`、API 基址）先以 data 属性挂在挂载点上，
///   前端产物接入时读取，避免在壳页内联脚本。
pub fn shell_html(prefix: &str) -> String {
    let prefix = escape_html(prefix);
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Skiff</title>
</head>
<body>
<div id="skiff-root" data-prefix="/{prefix}" data-api-base="/ui/v1" data-router-mode="history">
  <main data-skiff-placeholder>
    <h1>该功能正在适配中</h1>
    <p>页面正在切换到新的前端运行时，当前为占位视图。</p>
  </main>
</div>
<script src="/{prefix}/static/budding/budding.min.js"></script>
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
    fn shell_html_references_stable_alias_and_placeholder() {
        let html = shell_html("p");
        assert!(html.contains("该功能正在适配中"));
        assert!(html.contains(r#"<script src="/p/static/budding/budding.min.js"></script>"#));
        assert!(html.contains(r#"data-api-base="/ui/v1""#));
        // 不许出现内联脚本（全局 CSP 会拦），也不引用外部加载器
        assert!(!html.contains("<script>"));
        assert!(!html.contains("dist/loader"));
    }
}
