use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_app::commands::docker::{self as docker_cmd, NamedResource};
use skiff_docker::Verb;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::util::{form_first, parse_form};

pub async fn pull_image(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ImagePull, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let image = form_first(&form, "image").trim().to_string();
    if image.is_empty() {
        return Err(AppError::BadRequest("镜像名不能为空".into()));
    }

    let prefix = &state.cfg.entry_prefix;
    match docker_cmd::image_pull(
        &state,
        NamedResource {
            name: image.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(_) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("已拉取 {image}"),
            format!("/{prefix}/settings/docker"),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/settings/docker"),
        )),
    }
}

pub async fn remove_image(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ImageRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    docker_cmd::image_remove(
        &state,
        NamedResource {
            name: id,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    Ok(handlers::redirect(format!(
        "/{}/settings/docker",
        state.cfg.entry_prefix
    )))
}

pub(crate) fn network_view(n: skiff_docker::NetworkRow) -> crate::templates::NetworkRowView {
    use crate::templates::{NetworkContainerView, NetworkRowView};

    let builtin = n.builtin();
    let can_remove = n.can_remove();
    // 未知（Docker 未返回）必须显式显示「未知」，不能显示 0 或留空。
    let ipam_driver = match n.ipam_driver.as_deref() {
        Some(d) if !d.is_empty() => d.to_string(),
        Some(_) => "未知".to_string(),
        None => "未知".to_string(),
    };
    let subnet = match n.subnets.as_ref() {
        Some(v) if v.is_empty() => "—（无子网）".to_string(),
        Some(v) => v.join("，"),
        None => "未知".to_string(),
    };
    let gateway = match n.gateways.as_ref() {
        Some(v) if v.is_empty() => "—（无网关）".to_string(),
        Some(v) => v.join("，"),
        None => "未知".to_string(),
    };
    let internal = match n.internal {
        Some(true) => "内部".to_string(),
        Some(false) => "外部".to_string(),
        None => "未知".to_string(),
    };
    let attachable = match n.attachable {
        Some(true) => "可附加".to_string(),
        Some(false) => "不可附加".to_string(),
        None => "未知".to_string(),
    };
    let containers_known = n.containers.is_some();
    let container_count = match n.containers.as_ref() {
        Some(v) => format!("{} 个", v.len()),
        None => "未知".to_string(),
    };
    let containers = n
        .containers
        .unwrap_or_default()
        .into_iter()
        .map(|c| NetworkContainerView {
            name: c.container,
            ipv4: c.ipv4.unwrap_or_else(|| "—".into()),
            ipv6: c.ipv6.unwrap_or_else(|| "—".into()),
            mac: c.mac.unwrap_or_else(|| "—".into()),
        })
        .collect();
    NetworkRowView {
        id: n.id,
        name: n.name,
        driver: n.driver,
        scope: n.scope,
        ipam_driver,
        subnet,
        gateway,
        internal,
        attachable,
        container_count,
        containers_known,
        containers,
        builtin,
        can_remove,
    }
}

/// 网络/卷名：只允许字母数字与 `_-.`（旧页面与 `/ui/v1` 共用同一判定）。
pub(crate) fn validate_name(kind: &str, name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        return Err(AppError::BadRequest(format!(
            "{kind}名只允许字母数字与 _-. "
        )));
    }
    Ok(())
}

pub async fn create_network(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::NetworkCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let name = form_first(&form, "name").trim().to_string();
    validate_name("网络", &name)?;
    docker_cmd::network_create(
        &state,
        NamedResource {
            name,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    Ok(handlers::redirect(format!(
        "/{}/settings/docker",
        state.cfg.entry_prefix
    )))
}

pub async fn remove_network(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::NetworkRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    docker_cmd::network_remove(
        &state,
        NamedResource {
            name: id,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    Ok(handlers::redirect(format!(
        "/{}/settings/docker",
        state.cfg.entry_prefix
    )))
}

pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::VolumeCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let name = form_first(&form, "name").trim().to_string();
    validate_name("卷", &name)?;
    docker_cmd::volume_create(
        &state,
        NamedResource {
            name,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    Ok(handlers::redirect(format!(
        "/{}/settings/docker",
        state.cfg.entry_prefix
    )))
}

pub async fn remove_volume(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(name): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::VolumeRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    docker_cmd::volume_remove(
        &state,
        NamedResource {
            name,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    Ok(handlers::redirect(format!(
        "/{}/settings/docker",
        state.cfg.entry_prefix
    )))
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::resources::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;

/// Docker 命令错误映射：唯一实现在 [`crate::error::map_docker_cmd`]。
pub(crate) use crate::error::map_docker_cmd as map_err;

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, id: &str) -> skiff_docker::NetworkRow {
        skiff_docker::NetworkRow {
            id: id.into(),
            name: name.into(),
            driver: "bridge".into(),
            scope: "local".into(),
            ipam_driver: None,
            subnets: None,
            gateways: None,
            internal: None,
            attachable: None,
            containers: None,
        }
    }

    #[test]
    fn builtin_networks_cannot_be_removed_from_view() {
        // 判定在 `NetworkRow`，视图只消费 can_remove。
        assert!(!network_view(row("bridge", "abc")).can_remove);
        assert!(network_view(row("app-net", "def")).can_remove);
    }
}
