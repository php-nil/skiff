use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::handlers;
use crate::menu;
use crate::middleware::{check_csrf, AdminUser, AuthUser};
use crate::state::{AppState, CurrentUser};
use crate::templates::{AuditPage, AuditRow, KeyRow, MenuSettingsPage, SettingsPage};
use crate::util::{form_first, parse_form};

pub async fn render_settings(
    state: &Arc<AppState>,
    user: &CurrentUser,
    error: &str,
    notice: &str,
    totp: Option<(String, String)>,
    backup_codes: Vec<String>,
    force: bool,
) -> Result<Response, AppError> {
    let keys = state
        .db()
        .list_trusted_keys()?
        .into_iter()
        .map(|(key_id, _pk, label)| KeyRow {
            fingerprint_short: key_id.chars().take(16).collect(),
            key_id,
            label,
        })
        .collect();

    let security = skiff_app::views::settings::account_security(state, user.user.id)?
        .ok_or_else(|| AppError::NotFound("用户不存在".into()))?;

    let page = SettingsPage {
        ctx: handlers::page_ctx(state, user, "settings"),
        entry_prefix: state.cfg.entry_prefix.clone(),
        listen_addr: state.cfg.listen.ip().to_string(),
        listen_port: state.cfg.listen.port(),
        tls_cert: state
            .cfg
            .tls_cert
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        tls_key: state
            .cfg
            .tls_key
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        config_path: state.cfg.config_path.display().to_string(),
        // 三态回显：真 TLS → on；明文但非 loopback 只能是显式关闭 → off；
        // 反代形态与 loopback 调试保持 auto（保存时不会把配置误写成显式 off）。
        tls_mode: if state.cfg.tls_enabled {
            "on"
        } else if !state.cfg.dev_http {
            "auto"
        } else if !state.cfg.listen.ip().is_loopback() {
            "off"
        } else {
            "auto"
        }
        .to_string(),
        // 反代形态：本进程不终止 TLS、浏览器侧仍是 HTTPS，且配置了可信反代。
        plaintext_behind_proxy: !state.cfg.dev_http
            && !state.cfg.tls_enabled
            && !state.cfg.trusted_proxies.is_empty(),
        totp_enabled: security.totp_enabled,
        totp_secret: totp.as_ref().map(|(s, _)| s.clone()).unwrap_or_default(),
        totp_uri: totp.as_ref().map(|(_, u)| u.clone()).unwrap_or_default(),
        backup_codes,
        keys,
        is_admin: user.can(skiff_model::Permission::Admin),
        error: error.to_string(),
        notice: notice.to_string(),
        force_change: force || user.must_change_password(),
        // 文件仍存在说明初始口令尚未被修改；只提示，不拦截。
        initial_admin_pending: state.cfg.data_dir.join("initial-admin.txt").exists(),
    };
    handlers::render(page)
}

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, AppError> {
    let force = q.get("force").map(|v| v == "1").unwrap_or(false);
    // 错误提示走会话 flash（见 `settings_error_redirect`），URL 只保留页面上下文。
    render_settings(&state, &user, "", "", None, Vec::new(), force).await
}

/// 设置 → 监听端口 / 安全入口前缀 / TLS 开关。
///
/// 落盘到 `config.toml`，但**不热改**当前进程：路由前缀与监听 socket 都在启动时固定，
/// 统一要求重启。为防改前缀把自己锁在外面，保存时把旧前缀记为 24 小时兼容别名，
/// 重启后由路由层 302 到新入口。
pub async fn panel_update(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;

    let updated = match apply_panel_update(&state, &user, &form).await {
        Ok(msg) => msg,
        Err(msg) => return render_settings(&state, &user, &msg, "", None, Vec::new(), false).await,
    };
    render_settings(&state, &user, "", &updated, None, Vec::new(), false).await
}

pub(crate) async fn apply_panel_update(
    state: &Arc<AppState>,
    user: &CurrentUser,
    form: &std::collections::HashMap<String, Vec<String>>,
) -> Result<String, String> {
    use skiff_app::panel_settings::{
        check_port_free, toml_string, validate_entry_prefix, validate_port, write_config_updates,
    };

    let addr_raw = form_first(form, "listen_addr").trim().to_string();
    let ip: IpAddr = addr_raw
        .parse()
        .map_err(|_| format!("监听地址非法（应为 IPv4/IPv6 字面量）: {addr_raw}"))?;
    let port: u16 = form_first(form, "listen_port")
        .trim()
        .parse()
        .map_err(|_| "监听端口必须为 1–65535 的整数".to_string())?;
    let port = validate_port(port)?;
    let addr = SocketAddr::new(ip, port);
    // 当前正在监听的同一地址/端口直接放行，否则探测绑定。
    check_port_free(addr, state.cfg.listen)?;

    let prefix = validate_entry_prefix(&form_first(form, "entry_prefix"))?;

    let tls_mode = match form_first(form, "tls_mode").trim() {
        "" | "auto" => "auto",
        "on" => "on",
        "off" => "off",
        other => return Err(format!("TLS 模式非法: {other}（只允许 auto / on / off）")),
    };
    let tls_cert = form_first(form, "tls_cert").trim().to_string();
    let tls_key = form_first(form, "tls_key").trim().to_string();
    // 启用 TLS（或自动模式下填了证书）时，两个路径必须同时存在且可读。
    let cert_paths_used = tls_mode == "on" || (!tls_cert.is_empty() && !tls_key.is_empty());
    if cert_paths_used && (tls_cert.is_empty() || tls_key.is_empty()) {
        return Err(
            "已选择启用 TLS，但证书或私钥路径为空；请填写两个路径，或改为关闭 TLS（明文监听）"
                .into(),
        );
    }
    if cert_paths_used {
        for (label, p) in [("证书", &tls_cert), ("私钥", &tls_key)] {
            if !std::path::Path::new(p).is_file() {
                return Err(format!("TLS {label}文件不存在或不可读: {p}"));
            }
        }
    }
    // 自动模式：非 loopback 且既无证书、又无可信反代白名单，重启会拒绝启动；
    // 保存时就拦下来，避免「保存成功但下次起不来」。
    if tls_mode == "auto"
        && !ip.is_loopback()
        && !cert_paths_used
        && state.cfg.trusted_proxies.is_empty()
    {
        return Err(
            "自动模式下监听非 loopback，且没有证书也没有 trusted_proxies：重启会拒绝启动。\
             请选择启用 TLS、关闭 TLS（仅内网），或先配置 trusted_proxies"
                .into(),
        );
    }

    let mut updates: Vec<(&str, Option<String>)> = vec![
        ("listen", Some(toml_string(&addr.to_string()))),
        ("entry_prefix", Some(toml_string(&prefix))),
        (
            "tls_enabled",
            match tls_mode {
                "on" => Some("true".into()),
                "off" => Some("false".into()),
                // 恢复自动推断：删掉显式行
                _ => None,
            },
        ),
    ];
    if cert_paths_used {
        updates.push(("tls_cert", Some(toml_string(&tls_cert))));
        updates.push(("tls_key", Some(toml_string(&tls_key))));
    }
    write_config_updates(&state.cfg.config_path, &updates)?;

    // 保存即把旧前缀记为兼容别名；重启后路由层会 302，而不是直接 404 把人锁在外面。
    if prefix != state.cfg.entry_prefix {
        state
            .db()
            .set_setting(
                skiff_app::panel_settings::KEY_ENTRY_PREFIX_PREVIOUS,
                &state.cfg.entry_prefix,
            )
            .map_err(|e| format!("保存旧入口别名失败: {e}"))?;
        state
            .db()
            .set_setting(
                skiff_app::panel_settings::KEY_ENTRY_PREFIX_PREVIOUS_UNTIL,
                &(chrono::Utc::now().timestamp()
                    + skiff_app::panel_settings::ENTRY_PREFIX_GRACE_SECS)
                    .to_string(),
            )
            .map_err(|e| format!("保存旧入口兼容窗口失败: {e}"))?;
    }
    state
        .db()
        .set_setting(skiff_app::panel_settings::KEY_ENTRY_PREFIX, &prefix)
        .map_err(|e| format!("保存入口前缀失败: {e}"))?;

    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.panel.update",
        &format!("listen={addr} prefix={prefix} tls={tls_mode}"),
        "ok",
        &format!("config={}", state.cfg.config_path.display()),
    );

    let (tls_label, url) = match tls_mode {
        "on" => (
            "已启用，由本进程终止 TLS",
            format!("https://{addr}/{prefix}/login"),
        ),
        "off" => (
            "已关闭，直接监听明文 HTTP",
            format!("http://{addr}/{prefix}/login"),
        ),
        _ => (
            "自动（有证书即 HTTPS，反代形态保持明文）",
            format!("{addr}/{prefix}/login"),
        ),
    };
    let mut msg = format!(
        "已写入 {}：监听 {addr}，入口前缀 /{prefix}，TLS {tls_label}。新地址 {url} \
         需要重启 skiff 服务后才生效；重启前当前入口仍然可用。",
        state.cfg.config_path.display(),
    );
    if prefix != state.cfg.entry_prefix {
        msg.push_str(&format!(
            " 重启后旧入口 /{} 会在 24 小时内 302 跳转到新入口，作为防锁死兜底。",
            state.cfg.entry_prefix
        ));
    }
    if tls_mode == "off" && !ip.is_loopback() {
        msg.push_str(
            "【安全提示】已关闭 TLS 且直接监听非 loopback 地址：管理面为明文 HTTP，\
             仅限内网或反向代理之后使用；登录 Cookie 会降级为 skiff_dev_session（无 Secure）。",
        );
    }
    Ok(msg)
}

/// 302 Found（旧入口兼容用；`handlers::redirect` 是 303，仅用于表单提交后）。
fn found(location: String) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

/// 旧入口根：302 到新前缀首页。
pub async fn legacy_prefix_root(State(state): State<Arc<AppState>>) -> Response {
    found(format!("/{}", state.cfg.entry_prefix))
}

/// 旧入口深链：302 到新前缀下的同路径（只做 GET，避免把 POST 表单静默改写）。
pub async fn legacy_prefix_redirect(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
) -> Response {
    found(format!(
        "/{}/{}",
        state.cfg.entry_prefix,
        rest.trim_start_matches('/')
    ))
}

pub async fn import_key(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let public_hex = form_first(&form, "public_key").trim().to_lowercase();
    let label = form_first(&form, "label").trim().to_string();
    let vk = skiff_pkgfmt::decode_public_key(&public_hex)
        .map_err(|e| AppError::BadRequest(format!("公钥非法: {e}")))?;
    let key_id = skiff_pkgfmt::key_id_of(&vk.to_bytes());
    state.db().trust_key(&key_id, &public_hex, &label)?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.key.import",
        &key_id,
        "ok",
        &label,
    );
    render_settings(
        &state,
        &user,
        "",
        &format!("已导入公钥，指纹 {key_id}（请与发布者公布值逐位核对）"),
        None,
        Vec::new(),
        false,
    )
    .await
}

pub async fn remove_key(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(key_id): Path<String>,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state.db().remove_trusted_key(&key_id)?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.key.remove",
        &key_id,
        "ok",
        "撤销后该公钥签名的包将无法安装/升级",
    );
    render_settings(&state, &user, "", "已撤销该公钥", None, Vec::new(), false).await
}

/// 设置 → 菜单设置（按用户，scope = 当前账号）。
/// `GET /{p}/settings/menu` 规范入口：交付 budding 壳页。
///
/// 旧 SSR 保留在 `GET /{p}/m/core/settings/menu`（菜单深链），供无 JS 兜底。
pub async fn menu_shell(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Response, AppError> {
    Ok(crate::handlers::spa::shell_page(&state))
}

/// `GET /{p}/settings/ui` 规范入口：交付 budding 壳页（界面/主题设置）。
pub async fn ui_theme_shell(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Response, AppError> {
    Ok(crate::handlers::spa::shell_page(&state))
}

pub async fn menu_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    handlers::render(MenuSettingsPage {
        ctx: handlers::page_ctx(&state, &user, "menu-settings"),
        groups: menu::settings_view(&state, &user),
        notice: String::new(),
    })
}

/// 菜单设置写操作：SSR 表单与 `/ui/v1/settings/menu` 共用的唯一业务入口。
///
/// 偏好校验/落盘仍在 `skiff_app::menu::apply_action`，这里只补同一条审计。
pub(crate) fn apply_menu_update(
    state: &AppState,
    user: &CurrentUser,
    action: &str,
    key: &str,
) -> Result<(), AppError> {
    menu::apply_action(state, user, action, key)?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.menu.update",
        action,
        "ok",
        key,
    );
    Ok(())
}

pub async fn menu_update(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action");
    let key = form_first(&form, "key");
    apply_menu_update(&state, &user, &action, &key)?;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        "已保存菜单偏好",
        format!("/{}/settings/menu", state.cfg.entry_prefix),
    ))
}

pub async fn audit(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    // 审计读取对应 `Permission::AuditRead`（operator/admin），viewer 不可见。
    // 此前只用 `AuthUser`，等于把矩阵里的这一行架空了。
    handlers::require_permission(&user, skiff_model::Permission::AuditRead)?;
    let rows = state
        .db()
        .recent_audit(200)?
        .into_iter()
        .map(|a| AuditRow {
            ts: chrono::DateTime::from_timestamp(a.ts, 0)
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            actor: a.actor,
            ip: a.ip,
            action: a.action,
            target: a.target,
            result: a.result,
            detail: crate::util::truncate(&a.detail, 120),
        })
        .collect();
    handlers::render(AuditPage {
        ctx: handlers::page_ctx(&state, &user, "audit"),
        rows,
        error: String::new(),
    })
}
