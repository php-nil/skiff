use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::util::form_first;
use skiff_app::menu;
use skiff_app::{AppState, CurrentUser};
pub(crate) async fn apply_panel_update(
    state: &Arc<AppState>,
    user: &CurrentUser,
    client_ip: &str,
    form: &std::collections::HashMap<String, Vec<String>>,
) -> Result<String, String> {
    use skiff_app::panel_settings::{
        check_port_free, toml_string, validate_entry_prefix, validate_port,
        write_config_updates_via,
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
    write_config_updates_via(&state.cfg.config_path, &updates, |p, b| {
        state.admin_fs().write_kernel(p, b)
    })?;

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
        client_ip,
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
/// 菜单设置写操作的唯一业务入口（当前由 `/ui/v1/settings/menu` 调用）。
///
/// 偏好校验/落盘仍在 `skiff_app::menu::apply_action`，这里只补同一条审计。
/// `ip` 由调用方从请求 `Parts` 解析后传入：共享入口拿不到 `Parts`，
/// 不能再用 `"local"` 占位，否则审计页会把真实来源误报成本机。
pub(crate) fn apply_menu_update(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    action: &str,
    key: &str,
) -> Result<(), AppError> {
    menu::apply_action(state, user, action, key)?;
    state.audit_or_log(
        &user.user.username,
        ip,
        "settings.menu.update",
        action,
        "ok",
        key,
    );
    Ok(())
}
