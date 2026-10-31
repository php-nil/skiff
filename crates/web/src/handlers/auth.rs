use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::AppError;
use crate::handlers::{self, LOGIN_CSRF_COOKIE};
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::LoginPage;
use crate::util::{form_first, parse_cookies, parse_form};
use skiff_identity::token_matches;

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
    let csrf = skiff_identity::new_token();
    let secure = !state.cfg.dev_http;
    let page = LoginPage {
        dev_http: state.cfg.dev_http,
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
        handlers::simple_cookie(LOGIN_CSRF_COOKIE, &csrf, 600, secure)
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
    let form = parse_form(&body);
    let ip = handlers::client_ip(&state, &parts);
    let secure = !state.cfg.dev_http;

    // CSRF 双提交（匿名会话）
    let cookies = parse_cookies(parts.headers.get("cookie").and_then(|v| v.to_str().ok()));
    let cookie_csrf = cookies.get(LOGIN_CSRF_COOKIE).cloned().unwrap_or_default();
    let form_csrf = form_first(&form, "csrf");
    if cookie_csrf.is_empty() || !token_matches(&cookie_csrf, &form_csrf) {
        return Err(AppError::Forbidden("CSRF 校验失败，请刷新页面重试".into()));
    }

    if let Err(wait) = state.check_login_allowed(&ip).await {
        let _ = state.db.audit(
            &form_first(&form, "username"),
            &ip,
            "auth.login",
            "session",
            "blocked",
            &format!("限流：还需等待 {wait}s"),
        );
        return Err(AppError::Unavailable(format!(
            "登录失败次数过多，请 {wait} 秒后重试"
        )));
    }

    let username = form_first(&form, "username");
    let password = form_first(&form, "password");
    let otp = form_first(&form, "otp");
    let next = form_first(&form, "next");

    let _permit = state
        .login_semaphore
        .acquire()
        .await
        .map_err(|_| AppError::Internal("登录并发控制不可用".into()))?;

    let user = state.db.user_by_name(&username)?;
    let mut ok = false;
    if let Some(u) = &user {
        if skiff_identity::verify_password(&password, &u.password_hash) {
            if u.totp_enabled {
                let secret = u.totp_secret.clone().unwrap_or_default();
                let now = chrono::Utc::now().timestamp().max(0) as u64;
                if skiff_identity::verify_totp(&secret, &otp, now, 1) {
                    ok = true;
                } else if !otp.is_empty() {
                    let hash = skiff_identity::totp::hash_backup_code(&otp);
                    if state.db.consume_backup_code(u.id, &hash)? {
                        ok = true;
                    }
                }
            } else {
                ok = true;
            }
        }
    }

    if !ok {
        state.record_login_failure(&ip).await;
        let _ = state
            .db
            .audit(&username, &ip, "auth.login", "session", "fail", "");
        let page = LoginPage {
            dev_http: state.cfg.dev_http,
            prefix: state.cfg.entry_prefix.clone(),
            error: if user.map(|u| u.totp_enabled).unwrap_or(false) {
                "用户名、口令或动态码不正确".into()
            } else {
                "用户名或口令不正确".into()
            },
            csrf: cookie_csrf,
            username,
            next,
        };
        return handlers::render(page);
    }

    let user = user.expect("verified user exists");
    state.reset_login_failures(&ip).await;
    let tokens = state.new_session_tokens();
    let now = chrono::Utc::now().timestamp();
    state.db.create_session(
        &tokens.session_id,
        user.id,
        &tokens.session_token_hash,
        &ip,
        &handlers::host_header(&parts),
        now + state.cfg.session.idle_secs,
        now + state.cfg.session.abs_secs,
    )?;
    let _ = state
        .db
        .audit(&user.username, &ip, "auth.login", "session", "ok", "");

    let target = if user.must_change_password {
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
            &tokens.session_token,
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
    state.db.revoke_session(&user.session_id)?;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state
        .db
        .audit(&user.user.username, &ip, "auth.logout", "session", "ok", "");

    let mut resp = handlers::redirect(format!("/{}/login", state.cfg.entry_prefix));
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::clear_cookie(state.cookie_name(), !state.cfg.dev_http)
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
        return Err(AppError::BadRequest("两次输入的新口令不一致".into()));
    }
    if !skiff_identity::verify_password(&old, &user.user.password_hash) {
        return Err(AppError::BadRequest("当前口令不正确".into()));
    }
    let hash = skiff_identity::hash_password(&new).map_err(AppError::BadRequest)?;
    state.db.set_password(user.user.id, &hash)?;
    state.db.revoke_all_sessions(user.user.id)?;
    // 改密后换一个新会话，避免用户被自己的操作踢下线
    let tokens = state.new_session_tokens();
    let now = chrono::Utc::now().timestamp();
    let ip = handlers::client_ip(&state, &parts);
    state.db.create_session(
        &tokens.session_id,
        user.user.id,
        &tokens.session_token_hash,
        &ip,
        &handlers::host_header(&parts),
        now + state.cfg.session.idle_secs,
        now + state.cfg.session.abs_secs,
    )?;
    let _ = state.db.audit(
        &user.user.username,
        &ip,
        "auth.password.change",
        "user",
        "ok",
        "",
    );

    // 首启文件改密成功后删除（§5.2）
    let initial = state.cfg.data_dir.join("initial-admin.txt");
    if initial.exists() {
        let _ = std::fs::remove_file(&initial);
    }

    let mut resp = handlers::redirect(format!("/{}", state.cfg.entry_prefix));
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &tokens.session_token,
            state.cfg.session.abs_secs,
            !state.cfg.dev_http,
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
    state.db.set_totp(user.user.id, Some(&secret), false)?;
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
    state.db.replace_backup_codes(user.user.id, &hashes)?;
    state.db.set_totp(user.user.id, Some(&secret), true)?;
    let _ = state.db.audit(
        &user.user.username,
        "local",
        "auth.totp.enable",
        "user",
        "ok",
        "",
    );
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
