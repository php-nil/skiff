use std::collections::HashMap;
use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/settings`：设置页只读快照。
///
/// 写操作按钮显隐只看 `can_admin`；口令与 TOTP 仍走旧版设置页，不在本接口返回秘密材料。
pub async fn settings(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let can_admin = user.can(skiff_model::Permission::Admin);
    let user_row = state
        .db()
        .user_by_id(user.user.id)
        .map_err(|e| ApiError(AppError::from(e)))?
        .ok_or_else(|| ApiError(AppError::NotFound("用户不存在".into())))?;
    let keys = if can_admin {
        state
            .db()
            .list_trusted_keys()
            .map_err(|e| ApiError(AppError::from(e)))?
            .into_iter()
            .map(|(key_id, _pk, label)| SettingsKeyItem {
                fingerprint_short: key_id.chars().take(16).collect(),
                key_id,
                label,
            })
            .collect()
    } else {
        Vec::new()
    };
    let tls_mode = if state.cfg.tls_enabled {
        "on"
    } else if !state.cfg.dev_http {
        "auto"
    } else if !state.cfg.listen.ip().is_loopback() {
        "off"
    } else {
        "auto"
    };
    let (notice, error) = match state.take_flash(&user.session_id) {
        Some(msg) => match msg.kind {
            skiff_app::FlashKind::Notice => (msg.text, String::new()),
            skiff_app::FlashKind::Error => (String::new(), msg.text),
        },
        None => (String::new(), String::new()),
    };
    let (pending_secret, pending_uri) = match user_row.totp_secret.as_deref() {
        Some(secret) if !user_row.totp_enabled => (
            secret.to_string(),
            skiff_identity::otpauth_uri("Skiff", &user.user.username, secret),
        ),
        _ => (String::new(), String::new()),
    };
    let data = SettingsData {
        panel: SettingsPanelItem {
            listen_addr: state.cfg.listen.ip().to_string(),
            listen_port: state.cfg.listen.port(),
            entry_prefix: state.cfg.entry_prefix.clone(),
            tls_mode: tls_mode.into(),
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
            plaintext_behind_proxy: !state.cfg.dev_http
                && !state.cfg.tls_enabled
                && !state.cfg.trusted_proxies.is_empty(),
            initial_admin_pending: state.cfg.data_dir.join("initial-admin.txt").exists(),
        },
        account: SettingsAccountItem {
            totp_enabled: user_row.totp_enabled,
            must_change_password: user.must_change_password(),
            totp_pending_secret: pending_secret.clone(),
            totp_pending_uri: pending_uri,
        },
        keys,
        can_admin,
        flash: SettingsFlashItem { notice, error },
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/panel`：保存监听地址 / 入口前缀 / TLS。
///
/// 复用旧表单的 `apply_panel_update`（同一校验、同一落盘、同一审计），只把传输层
/// 换成 JSON；CSRF 用 `/ui/v1` 统一的 `x-csrf-token` 头。
pub async fn settings_panel_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<SettingsPanelBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    guard::permission(&user, skiff_model::Permission::Admin)?;
    let mut form: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (key, value) in [
        ("listen_addr", body.listen_addr),
        ("listen_port", body.listen_port),
        ("entry_prefix", body.entry_prefix),
        ("tls_mode", body.tls_mode),
        ("tls_cert", body.tls_cert),
        ("tls_key", body.tls_key),
    ] {
        form.insert(key.to_string(), vec![value]);
    }
    let message = handlers::settings::apply_panel_update(&state, &user, &form)
        .await
        .map_err(|m| ApiError(AppError::BadRequest(m)))?;
    let mut resp = Json(json!({ "data": { "message": message } })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/settings/menu`：菜单设置页的只读快照（当前账号偏好）。
pub async fn settings_menu(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let groups = crate::menu::settings_view(&state, &user)
        .into_iter()
        .map(|g| MenuSettingsGroup {
            key: g.key,
            label: g.label,
            items: g
                .items
                .into_iter()
                .map(|i| MenuSettingsItem {
                    key: i.key,
                    label: i.label,
                    href: i.href,
                    hidden: i.hidden,
                    pinned: i.pinned,
                    pinnable: i.pinnable,
                    plugin: i.plugin,
                    icon: i.icon,
                })
                .collect(),
        })
        .collect();
    let data = MenuSettingsData {
        layout: crate::menu::current_layout(&state, &user),
        width: crate::menu::current_width(&state, &user),
        max_width: crate::menu::current_max_width(&state, &user),
        groups,
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/menu`：复用 `settings::apply_menu_update`（同一校验与审计）。
pub async fn settings_menu_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<MenuUpdateBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    handlers::settings::apply_menu_update(
        &state,
        &user,
        &body.action,
        &body.key,
        (!body.layout.is_empty()).then_some(body.layout.as_str()),
        (!body.width.is_empty()).then_some(body.width.as_str()),
        (!body.max.is_empty()).then_some(body.max.as_str()),
    )
    .map_err(ApiError)?;
    let mut resp = Json(json!({ "data": { "message": "已保存菜单偏好" } })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/settings/docker`：镜像加速源与镜像前缀快照。
///
/// 读取对已登录用户开放（与旧页一致，表单只对管理员可编辑）；写操作仍要求 Admin。
pub async fn settings_docker(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let data = DockerSettingsData {
        registry_mirror: state
            .db()
            .get_setting("sites.registry_mirror")
            .map_err(|e| ApiError(AppError::from(e)))?
            .unwrap_or_default(),
        image_prefix: state
            .db()
            .get_setting("sites.image_prefix")
            .map_err(|e| ApiError(AppError::from(e)))?
            .unwrap_or_default(),
        can_admin: user.can(skiff_model::Permission::Admin),
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/docker`：复用 `docker_settings::apply_settings`（同一落盘与审计）。
pub async fn settings_docker_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<DockerSettingsBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    guard::permission(&user, skiff_model::Permission::Admin)?;
    let mut form: HashMap<String, Vec<String>> = HashMap::new();
    form.insert("registry_mirror".into(), vec![body.registry_mirror]);
    form.insert("image_prefix".into(), vec![body.image_prefix]);
    let ip = handlers::client_ip(&state, &parts);
    let notice = handlers::docker_settings::apply_settings(&state, &user, &ip, &form)
        .await
        .map_err(ApiError)?;
    let mut resp = Json(json!({ "data": { "message": notice } })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/password`：改密，复用与会话轮换同一套实现。
///
/// 口令比较/hash 是 CPU 密集的同步计算，走 `blocking_fs` 线程池；成功后轮换会话
/// 并回写 Cookie，响应里带回新 CSRF，前端不必重新登录。
pub async fn settings_password_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<PasswordBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    if body.new_password != body.confirm_password {
        return Err(ApiError(AppError::BadRequest(
            "两次输入的新口令不一致".into(),
        )));
    }
    let verified = handlers::blocking_fs({
        let old = body.old_password.clone();
        let stored = user.user.password_hash.clone();
        move || skiff_identity::verify_password(&old, &stored)
    })
    .await?;
    if !verified {
        return Err(ApiError(AppError::BadRequest("当前口令不正确".into())));
    }
    let hash = handlers::blocking_fs({
        let new = body.new_password.clone();
        move || skiff_identity::hash_password(&new).map_err(AppError::BadRequest)
    })
    .await??;
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
    // 首启文件改密成功后删除（与 SSR 改密一致）。
    let initial = state.cfg.data_dir.join("initial-admin.txt");
    if initial.exists() {
        if let Err(e) = std::fs::remove_file(&initial) {
            tracing::debug!(path = %initial.display(), error = %e, "删除首启口令文件失败");
        }
    }
    let mut resp = Json(json!({
        "data": {
            "message": "口令已修改",
            "csrf_token": state.csrf_token(&tokens.session_token),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

/// `POST /ui/v1/settings/totp/setup`：生成并暂存 TOTP 密钥（未启用）。
///
/// 返回密钥与 otpauth URI 供前端原生展示；密钥在启用前可通过 `GET /settings` 的
/// `account.totp_pending_*` 再次取回，刷新不丢。
pub async fn settings_totp_setup(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    let secret = skiff_identity::generate_totp_secret();
    state
        .save_totp_secret(user.user.id, &secret, false)
        .map_err(|e| ApiError(AppError::Internal(e)))?;
    let uri = skiff_identity::otpauth_uri("Skiff", &user.user.username, &secret);
    let mut resp = Json(json!({
        "data": { "secret": secret, "otpauth_uri": uri, "message": "已生成密钥，请输入动态码完成启用" }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/totp/enable`：校验动态码并启用，返回一次性备份码。
pub async fn settings_totp_enable(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<TotpEnableBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    let secret = user.user.totp_secret.clone().unwrap_or_default();
    let now = chrono::Utc::now().timestamp().try_into().unwrap_or(0);
    if !skiff_identity::verify_totp(&secret, body.code.trim(), now, 1) {
        return Err(ApiError(AppError::BadRequest(
            "动态码不正确，请确认设备时间后重试".into(),
        )));
    }
    let (plain, hashes) = skiff_identity::generate_backup_codes(10);
    state
        .enable_totp(user.user.id, &user.user.username, &secret, &hashes)
        .map_err(|e| ApiError(AppError::Internal(e)))?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "auth.totp.enable",
        &user.user.username,
        "ok",
        "启用两步验证",
    );
    let mut resp = Json(json!({
        "data": {
            "backup_codes": plain,
            "message": "两步验证已启用，请保存备份码（只显示一次）"
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/settings/keys`：导入信任公钥（管理员），语义与 SSR 表单一致。
pub async fn settings_key_import(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<KeyImportBody>,
) -> Result<Response, ApiError> {
    if !user.can(skiff_model::Permission::Admin) {
        return Err(ApiError(AppError::Forbidden("需要管理员权限".into())));
    }
    guard::csrf(&state, &user, &parts)?;
    let public_hex = body.public_key.trim().to_lowercase();
    let label = body.label.trim().to_string();
    let vk = skiff_pkgfmt::decode_public_key(&public_hex)
        .map_err(|e| ApiError(AppError::BadRequest(format!("公钥非法: {e}"))))?;
    let key_id = skiff_pkgfmt::key_id_of(&vk.to_bytes());
    state
        .db()
        .trust_key(&key_id, &public_hex, &label)
        .map_err(|e| ApiError(AppError::from(e)))?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.key.import",
        &key_id,
        "ok",
        &label,
    );
    Ok(Json(json!({
        "data": { "key_id": key_id, "message": "已导入公钥（请与发布者公布值逐位核对）" }
    }))
    .into_response())
}

/// `POST /ui/v1/settings/keys/{id}`：撤销信任公钥（管理员），语义与 SSR 表单一致。
pub async fn settings_key_remove(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Path(key_id): Path<String>,
) -> Result<Response, ApiError> {
    if !user.can(skiff_model::Permission::Admin) {
        return Err(ApiError(AppError::Forbidden("需要管理员权限".into())));
    }
    guard::csrf(&state, &user, &parts)?;
    state
        .db()
        .remove_trusted_key(&key_id)
        .map_err(|e| ApiError(AppError::from(e)))?;
    state.audit_or_log(
        &user.user.username,
        "local",
        "settings.key.remove",
        &key_id,
        "ok",
        "撤销后该公钥签名的包将无法安装/升级",
    );
    Ok(Json(json!({ "data": { "message": "已撤销该公钥" } })).into_response())
}

// ---- 文件管理（第十一批：平铺 + 面包屑） --------------------------------

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/settings", get(settings))
        .route("/ui/v1/settings/panel", post(settings_panel_update))
        .route(
            "/ui/v1/settings/menu",
            get(settings_menu).post(settings_menu_update),
        )
        .route(
            "/ui/v1/settings/docker",
            get(settings_docker).post(settings_docker_update),
        )
        .route("/ui/v1/settings/password", post(settings_password_update))
        .route("/ui/v1/settings/totp/setup", post(settings_totp_setup))
        .route("/ui/v1/settings/totp/enable", post(settings_totp_enable))
        .route("/ui/v1/settings/keys", post(settings_key_import))
        .route("/ui/v1/settings/keys/{id}", post(settings_key_remove))
}
