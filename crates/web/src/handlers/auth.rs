use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::response::{Html, IntoResponse, Response};

use crate::error::AppError;
use crate::handlers::{self, LOGIN_CSRF_COOKIE};
use crate::util::{escape_html, parse_cookies};
use skiff_app::AppState;
use skiff_identity::{token_matches, SignedTokenError};

/// 登录页 CSRF token 有效期（与 Cookie Max-Age 一致）。
pub(crate) const LOGIN_CSRF_TTL_SECS: i64 = 600;

/// 签发登录页 CSRF token。
///
/// 签名密钥取持久化的 `security.csrf_secret`（重启后不变），所以重启前打开、
/// 尚未提交的登录页仍能通过校验。
pub(crate) fn issue_login_csrf(state: &AppState) -> String {
    skiff_identity::signed_token(&state.csrf_secret, "login", chrono::Utc::now().timestamp())
}

/// 登录 CSRF 判定（表单路径与 `/ui/v1/auth/login` JSON 路径共用）。
///
/// 签名表单 token 是唯一权威来源；Cookie 只在浏览器完全没有来源头时作兼容信号。
pub(crate) fn verify_login_csrf(
    state: &AppState,
    parts: &Parts,
    form_csrf: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().timestamp();
    match skiff_identity::verify_signed_token(
        &state.csrf_secret,
        "login",
        form_csrf,
        LOGIN_CSRF_TTL_SECS,
        now,
    ) {
        Ok(()) => {}
        Err(SignedTokenError::Expired) => {
            return Err(AppError::Csrf(
                "登录页已过期（超过 10 分钟），请刷新页面后重试".into(),
            ));
        }
        Err(SignedTokenError::Invalid) => {
            return Err(AppError::Csrf("CSRF 校验失败，请刷新页面后重试".into()));
        }
    }

    // 来源判定：优先看浏览器自带的 `Sec-Fetch-Site` / `Origin` / `Referer`；
    // 三者都没有（老客户端、命令行）时退回双提交兼容路径。
    let cookie_csrf = parse_cookies(parts.headers.get("cookie").and_then(|v| v.to_str().ok()))
        .get(LOGIN_CSRF_COOKIE)
        .cloned()
        .unwrap_or_default();
    match handlers::login_origin_allowed(state, parts) {
        Some(true) => Ok(()),
        Some(false) => Err(AppError::Forbidden("跨站登录请求被拒绝".into())),
        None => {
            if cookie_csrf.is_empty() || !token_matches(&cookie_csrf, form_csrf) {
                Err(AppError::Csrf("CSRF 校验失败，请刷新页面后重试".into()))
            } else {
                Ok(())
            }
        }
    }
}

/// 登录成功后的落点（表单路径与 JSON 路径共用）。
pub(crate) fn login_redirect_target(
    state: &AppState,
    next: &str,
    must_change_password: bool,
) -> String {
    if must_change_password {
        format!("/{}/settings/security?force=1", state.cfg.entry_prefix)
    } else if next.starts_with('/') && !next.starts_with("//") {
        next.to_string()
    } else {
        format!("/{}", state.cfg.entry_prefix)
    }
}

/// budding 登录页壳：未认证可渲染（不经过 SPA 回落的 303 分支）。
///
/// 页面只给挂载点与产物引用；CSRF、用户/跳转参数由启动脚本通过
/// `GET /ui/v1/auth/login` 与挂载点 data 属性获取。
pub fn login_shell_html(prefix: &str, username: &str, next: &str) -> String {
    let prefix = escape_html(prefix);
    let username = escape_html(username);
    let next = escape_html(next);
    let ver = crate::assets::BUDDING_VERSION;
    let shell = crate::assets::SHELL_VERSION;
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>登录 · Skiff</title>
<link rel="icon" type="image/svg+xml" href="/{prefix}/static/favicon.svg?v=2">
<script src="/{prefix}/static/theme-init.js"></script>
<link rel="stylesheet" href="/{prefix}/static/skiff.css?v={shell}">
</head>
<body class="skiff-login-shell">
<div class="login-layout">
  <aside class="login-brand-col">
    <div class="brand-logo" aria-hidden="true">
      <svg viewBox="0 0 28 28" fill="currentColor"><path d="M3.5 17.8q10.5-6.6 21 0-10.5 6.6-21 0Z"/><circle cx="14" cy="10.4" r="2.6"/></svg>
    </div>
    <h1 class="brand-title">Skiff · 轻舟</h1>
    <p class="brand-subtitle">Web 服务、Docker 与插件收进同一个二进制，一条命令完成部署。</p>
    <ul class="brand-features">
      <li><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="m9 12 2 2 4-4"/><circle cx="12" cy="12" r="9"/></svg>单二进制，零依赖部署</li>
      <li><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3.5" y="9" width="5" height="5" rx="1"/><rect x="9.5" y="9" width="5" height="5" rx="1"/><rect x="9.5" y="3.8" width="5" height="5" rx="1"/><rect x="15.5" y="9" width="5" height="5" rx="1"/></svg>Docker 容器与编排一体化</li>
      <li><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M9 3v4.5M15 3v4.5M7 7.5h10v3.2a5 5 0 0 1-5 5 5 5 0 0 1-5-5zM12 15.7V21"/></svg>插件体系按需扩展能力</li>
    </ul>
  </aside>
  <section class="login-form-col">
    <div class="login-form-inner">
      <header class="login-form-head">
        <p class="login-eyebrow">SKIFF CONSOLE</p>
        <h2>欢迎回来</h2>
        <p class="login-form-sub">使用管理员账号登录，继续管理这台服务器。</p>
      </header>
      <div id="skiff-login" data-prefix="/{prefix}" data-api-base="/ui/v1" data-username="{username}" data-next="{next}" data-budding-css="/{prefix}/static/budding/budding-{ver}.min.css?v={ver}"></div>
      <p class="login-foot">Skiff · 轻舟 · 单二进制 Linux 服务器管理面板</p>
    </div>
  </section>
</div>
<button type="button" class="login-theme-toggle" data-theme-toggle aria-pressed="false" aria-label="切换主题">
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>
  <span class="theme-when-light">深色</span><span class="theme-when-dark">浅色</span>
</button>
<script src="/{prefix}/static/budding/budding-{ver}.min.js?v={ver}"></script>
<script src="/{prefix}/static/form-enter-submit.js?v={shell}"></script>
<script src="/{prefix}/static/budding/budding-login.js?v={shell}"></script>
<script src="/{prefix}/static/theme-toggle.js?v={shell}"></script>
</body>
</html>
"#
    )
}

pub async fn login_page(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    parts: Parts,
) -> Result<Response, AppError> {
    if parts.extensions.get::<skiff_app::CurrentUser>().is_some() {
        return Ok(handlers::redirect(format!("/{}", state.cfg.entry_prefix)));
    }
    let username = q
        .get("username")
        .or_else(|| q.get("u"))
        .cloned()
        .unwrap_or_default();
    let next = q.get("next").cloned().unwrap_or_default();
    let body = login_shell_html(&state.cfg.entry_prefix, &username, &next);
    Ok((
        [
            ("cache-control", "no-store"),
            ("content-type", "text/html; charset=utf-8"),
        ],
        Html(body),
    )
        .into_response())
}
