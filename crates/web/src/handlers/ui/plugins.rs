use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/plugins`：已安装插件列表（含运行态与可管理标记）。
pub async fn plugins(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let mut views = handlers::plugins::plugin_views(&state).map_err(ApiError)?;
    for v in views.iter_mut() {
        v.running = state.supervisor().is_running(&v.id).await;
    }
    let enabled_count = views.iter().filter(|v| v.enabled).count();
    let disabled_count = views.len().saturating_sub(enabled_count);
    let can_admin = user.can(skiff_model::Permission::PluginManage);
    let data = json!({
        "plugins": views.into_iter().map(plugin_item).collect::<Vec<_>>(),
        "enabled_count": enabled_count,
        "disabled_count": disabled_count,
        "can_admin": can_admin,
        "allow_unsigned": state.cfg.allow_unsigned,
        "reduced_allowed": state.cfg.allow_reduced_sandbox,
    });
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/plugins/{id}`：插件信息与管理页快照（不含 iframe）。
pub async fn plugin_info(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let rec = state
        .db()
        .plugin(&id)
        .map_err(|e| ApiError(AppError::from(e)))?
        .ok_or_else(|| ApiError(AppError::NotFound(format!("插件不存在: {id}"))))?;
    let mut view = handlers::plugins::plugin_view(&state, &rec).map_err(ApiError)?;
    view.running = state.supervisor().is_running(&view.id).await;
    let manifest = skiff_pkgfmt::Manifest::parse(&rec.manifest).ok();
    let (menus, entry_href) = manifest
        .as_ref()
        .map(|m| crate::menu::plugin_info_menu(&state, &user, &id, m))
        .unwrap_or_default();
    let (interop_provides, interop_consumes) = manifest
        .as_ref()
        .map(handlers::plugins::interop_rows)
        .unwrap_or_default();
    let grants = rec
        .grants
        .iter()
        .map(|g| PluginGrantItem {
            kind: g.kind.clone(),
            value: g.value.clone(),
        })
        .collect();
    let menus = menus
        .into_iter()
        .map(|g| PluginMenuGroupItem {
            label: g.label,
            href: g.href,
            icon: g.icon,
            children: g
                .children
                .into_iter()
                .map(|c| PluginMenuLinkItem {
                    label: c.label,
                    href: c.href,
                    icon: c.icon,
                })
                .collect(),
        })
        .collect();
    let data = PluginInfoData {
        plugin: plugin_item(view),
        grants,
        interop_provides,
        interop_consumes,
        menus,
        entry_href,
        can_admin: user.can(skiff_model::Permission::PluginManage),
        allow_unsigned: state.cfg.allow_unsigned,
        reduced_allowed: state.cfg.allow_reduced_sandbox,
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/plugins/{id}/toggle`：启用 / 停用（复用 `apply_toggle` 的停进程与审计）。
pub async fn plugin_toggle(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::require_permission(&user, skiff_model::Permission::PluginManage).map_err(ApiError)?;
    let ip = handlers::client_ip(&state, &parts);
    let enabled = handlers::plugins::apply_toggle(&state, &user, &id, &ip)
        .await
        .map_err(ApiError)?;
    let mut resp = Json(json!({ "data": { "id": id, "enabled": enabled } })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/plugins/{id}/delete`：卸载（复用 `apply_remove` 的清理与审计）。
pub async fn plugin_delete(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<PluginDeleteBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::require_permission(&user, skiff_model::Permission::PluginManage).map_err(ApiError)?;
    let ip = handlers::client_ip(&state, &parts);
    handlers::plugins::apply_remove(&state, &user, &id, body.delete_data, &ip)
        .await
        .map_err(ApiError)?;
    let mut resp = Json(json!({
        "data": {
            "id": id,
            "redirect_to": format!("/{}/plugins", state.cfg.entry_prefix),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/plugins/confirm`：权限确认后安装商店 / 上传的暂存包。
///
/// 与旧确认页共用 `verify_for_confirm` + `commit_install`（同一信任判定、审计与清理）。
pub async fn plugin_confirm(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<PluginConfirmBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::require_permission(&user, skiff_model::Permission::PluginManage).map_err(ApiError)?;
    let token = body.token.trim();
    if token.len() != 24 || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError(AppError::BadRequest("确认令牌非法".into())));
    }
    let incoming = state.cfg.data_dir.join("plugins").join(".incoming");
    let path = incoming.join(format!("{token}.wbpkg"));
    if path.parent() != Some(incoming.as_path()) || !path.exists() {
        return Err(ApiError(AppError::BadRequest(
            "上传文件已过期，请重新上传".into(),
        )));
    }
    let trust = handlers::plugins::verify_for_confirm(&state, &path).map_err(ApiError)?;
    let mut objects = body.resource.clone();
    for part in body
        .extra_resource
        .split(['\n', '\r', ',', ' ', '\t'])
        .map(str::trim)
    {
        if !part.is_empty() {
            objects.push(part.to_string());
        }
    }
    objects.sort();
    objects.dedup();
    for o in &objects {
        skiff_pkgfmt::manifest::validate_resource(o)
            .map_err(|e| ApiError(AppError::BadRequest(format!("对象授权 {o} 非法：{e}"))))?;
    }
    let opts = skiff_plugin::install_options(
        &trust,
        body.grant.clone(),
        objects,
        state.cfg.allow_unsigned,
        state.cfg.form,
        state.cfg.allow_reduced_sandbox,
        &user.user.username,
    );
    let ip = handlers::client_ip(&state, &parts);
    let id = trust.meta.manifest.id.clone();
    handlers::plugins::commit_install(&state, &user, &ip, &path, &opts, &id)
        .await
        .map_err(ApiError)?;
    let mut resp = Json(json!({
        "data": {
            "id": id,
            "redirect_to": format!("/{}/plugins", state.cfg.entry_prefix),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/plugins", get(plugins))
        .route("/ui/v1/plugins/confirm", post(plugin_confirm))
        .route("/ui/v1/plugins/{id}", get(plugin_info))
        .route("/ui/v1/plugins/{id}/toggle", post(plugin_toggle))
        .route("/ui/v1/plugins/{id}/delete", post(plugin_delete))
}
