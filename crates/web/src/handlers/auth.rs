use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::AppError;
use crate::handlers::{self, LOGIN_CSRF_COOKIE};
use crate::middleware::{check_csrf, AuthUser};
use crate::state::auth::{LoginError, LoginRequest};
use crate::state::AppState;
use crate::templates::LoginPage;
use crate::util::{form_first, parse_cookies, parse_form, try_parse_form};
use skiff_identity::{token_matches, SignedTokenError};

/// 登录页 CSRF token 有效期（与 Cookie Max-Age 一致）。
const LOGIN_CSRF_TTL_SECS: i64 = 600;

/// 签发登录页 CSRF token。
///
/// 签名密钥取持久化的 `security.csrf_secret`（重启后不变），所以重启前打开、
/// 尚未提交的登录页仍能通过校验。
fn issue_login_csrf(state: &AppState) -> String {
    skiff_identity::signed_token(&state.csrf_secret, "login", chrono::Utc::now().timestamp())
}

pub async fn login_page(
    State(state): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    parts: Parts,
) -> Result<Response, AppError> {
    if parts
        .extensions
        .get::<crate::state::CurrentUser>()
        .is_some()
    {
        return Ok(handlers::redirect(format!("/{}", state.cfg.entry_prefix)));
    }
    let csrf = issue_login_csrf(&state);
    // `Secure` 只在客户端到面板确实是 HTTPS 时才加：浏览器会直接丢弃「在明文响应里
    // 收到」的 Secure Cookie。这里读可信代理解析后的 scheme（`X-Forwarded-Proto`），
    // 而不是只看 `dev_http` 开关。
    let secure = handlers::is_secure_request(&state, &parts);
    let page = LoginPage {
        prefix: state.cfg.entry_prefix.clone(),
        error: String::new(),
        csrf: csrf.clone(),
        username: q.get("u").cloned().unwrap_or_default(),
        next: q.get("next").cloned().unwrap_or_default(),
    };
    let body = handlers::render(page)?;
    let mut resp = body;
    resp.headers_mut().append(
        SET_COOKIE,
        handlers::simple_cookie(LOGIN_CSRF_COOKIE, &csrf, LOGIN_CSRF_TTL_SECS, secure)
            .parse()
            .unwrap(),
    );
    Ok(resp)
}

pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    // 登录是未认证入口：坏编码直接 400，不做替换字符降级（旧实现还会 panic）。
    let form = try_parse_form(&body)?;
    let ip = handlers::client_ip(&state, &parts);
    let secure = handlers::is_secure_request(&state, &parts);

    // 登录 CSRF：**表单里的签名 token 是唯一权威来源**。
    //
    // 旧实现要求 `skiff_login` Cookie 必须存在且与表单值一致（双提交）。浏览器可能
    // 因为 Secure/SameSite 策略、Cookie 过期（10 分钟）、bfcache 回退复用旧页面、
    // 多标签页覆盖等原因不回传该 Cookie，一旦缺失登录就必然失败且只给「刷新重试」。
    // 现在 Cookie 降级为兼容信号，缺失也不再影响校验。
    let form_csrf = form_first(&form, "csrf");
    let now = chrono::Utc::now().timestamp();
    match skiff_identity::verify_signed_token(
        &state.csrf_secret,
        "login",
        &form_csrf,
        LOGIN_CSRF_TTL_SECS,
        now,
    ) {
        Ok(()) => {}
        Err(SignedTokenError::Expired) => {
            return Err(AppError::Forbidden(
                "登录页已过期（超过 10 分钟），请刷新页面后重试".into(),
            ));
        }
        Err(SignedTokenError::Invalid) => {
            return Err(AppError::Forbidden(
                "CSRF 校验失败，请刷新页面后重试".into(),
            ));
        }
    }

    // 来源判定：优先看浏览器自带的 `Sec-Fetch-Site` / `Origin` / `Referer`；
    // 三者都没有（老客户端、命令行）时退回双提交兼容路径。
    let cookie_csrf = parse_cookies(parts.headers.get("cookie").and_then(|v| v.to_str().ok()))
        .get(LOGIN_CSRF_COOKIE)
        .cloned()
        .unwrap_or_default();
    match handlers::login_origin_allowed(&state, &parts) {
        Some(true) => {}
        Some(false) => {
            return Err(AppError::Forbidden("跨站登录请求被拒绝".into()));
        }
        None => {
            if cookie_csrf.is_empty() || !token_matches(&cookie_csrf, &form_csrf) {
                return Err(AppError::Forbidden(
                    "CSRF 校验失败，请刷新页面后重试".into(),
                ));
            }
        }
    }

    let username = form_first(&form, "username");
    let next = form_first(&form, "next");
    let req = LoginRequest {
        username: username.clone(),
        password: form_first(&form, "password"),
        otp: form_first(&form, "otp"),
    };

    // 判定流程只有一份（`skiff_app::auth::login`），这里只负责把结果渲染成页面。
    let success = match state
        .login(&req, &ip, "session", &handlers::host_header(&parts))
        .await
    {
        Ok(s) => s,
        Err(LoginError::RateLimited(wait)) => {
            return Err(AppError::Unavailable(format!(
                "登录失败次数过多，请 {wait} 秒后重试"
            )));
        }
        Err(LoginError::Invalid(message)) => {
            // 错误页换一张新 token，并同步刷新兼容 Cookie（旧 token 可能已接近过期，
            // 直接沿用会让用户的下一次提交卡在时效边界上）。
            let csrf = issue_login_csrf(&state);
            let page = LoginPage {
                prefix: state.cfg.entry_prefix.clone(),
                error: message,
                csrf: csrf.clone(),
                username,
                next,
            };
            let mut resp = handlers::render(page)?;
            resp.headers_mut().append(
                SET_COOKIE,
                handlers::simple_cookie(LOGIN_CSRF_COOKIE, &csrf, LOGIN_CSRF_TTL_SECS, secure)
                    .parse()
                    .unwrap(),
            );
            return Ok(resp);
        }
        Err(LoginError::Internal(msg)) => return Err(AppError::Internal(msg)),
    };

    let target = if success.must_change_password {
        format!("/{}/settings?force=1", state.cfg.entry_prefix)
    } else if next.starts_with('/') && !next.starts_with("//") {
        next
    } else {
        format!("/{}", state.cfg.entry_prefix)
    };

    let mut resp = handlers::redirect(target);
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &success.session_token,
            state.cfg.session.abs_secs,
            secure,
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    state
        .logout(&user.user.username, &user.session_id, &ip)
        .map_err(AppError::Internal)?;

    let mut resp = handlers::redirect(format!("/{}/login", state.cfg.entry_prefix));
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

pub async fn change_password(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let old = form_first(&form, "old_password");
    let new = form_first(&form, "new_password");
    let confirm = form_first(&form, "confirm_password");

    if new != confirm {
        return Err(AppError::Redirect(handlers::settings_error_redirect(
            &state,
            &user,
            "两次输入的新口令不一致",
        )));
    }
    // argon2 是同步的 CPU/内存密集计算，不能占用 async worker。
    let verified = handlers::blocking_fs({
        let old = old.to_string();
        let stored = user.user.password_hash.clone();
        move || skiff_identity::verify_password(&old, &stored)
    })
    .await?;
    if !verified {
        return Err(AppError::Redirect(handlers::settings_error_redirect(
            &state,
            &user,
            "当前口令不正确",
        )));
    }
    let hash = handlers::blocking_fs({
        let new = new.to_string();
        move || skiff_identity::hash_password(&new).map_err(AppError::BadRequest)
    })
    .await??;
    // 改密后换一个新会话，避免用户被自己的操作踢下线（写库与轮换收口在用例层）。
    let ip = handlers::client_ip(&state, &parts);
    let tokens = state
        .rotate_password_session(
            user.user.id,
            &user.user.username,
            &hash,
            &ip,
            &handlers::host_header(&parts),
        )
        .map_err(AppError::Internal)?;

    // 首启文件改密成功后删除（§5.2）
    let initial = state.cfg.data_dir.join("initial-admin.txt");
    if initial.exists() {
        if let Err(e) = std::fs::remove_file(&initial) {
            tracing::debug!(path = %initial.display(), error = %e, "删除首启口令文件失败");
        }
    }

    let mut resp = handlers::redirect(format!("/{}", state.cfg.entry_prefix));
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &tokens.session_token,
            state.cfg.session.abs_secs,
            handlers::is_secure_request(&state, &parts),
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}

pub async fn setup_totp(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let secret = skiff_identity::generate_totp_secret();
    state
        .save_totp_secret(user.user.id, &secret, false)
        .map_err(AppError::Internal)?;
    let uri = skiff_identity::otpauth_uri("Skiff", &user.user.username, &secret);
    crate::handlers::settings::render_settings(
        &state,
        &user,
        "",
        "扫描/录入密钥后输入动态码完成启用",
        Some((secret, uri)),
        Vec::new(),
        false,
    )
    .await
}

pub async fn enable_totp(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let code = form_first(&form, "code");
    let secret = user.user.totp_secret.clone().unwrap_or_default();
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    if secret.is_empty() {
        return Err(AppError::BadRequest("请先生成 TOTP 密钥".into()));
    }
    if !skiff_identity::verify_totp(&secret, &code, now, 1) {
        return Err(AppError::BadRequest("动态码不正确".into()));
    }
    let (plain, hashes) = skiff_identity::generate_backup_codes(10);
    state
        .enable_totp(user.user.id, &user.user.username, &secret, &hashes)
        .map_err(AppError::Internal)?;
    crate::handlers::settings::render_settings(
        &state,
        &user,
        "",
        "TOTP 已启用；备份码只显示这一次，请立即保存",
        None,
        plain,
        false,
    )
    .await
}
