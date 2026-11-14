use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/images`：镜像列表（只读）。
pub async fn images(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_images()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
            .into_iter()
            .map(|i| {
                let name = if i.tags.is_empty() {
                    format!("<none> ({})", i.short_id())
                } else {
                    i.tags.join(", ")
                };
                (
                    i.id.clone(),
                    ResourceItem {
                        id: i.id,
                        name,
                        meta: crate::util::human_size(i.size),
                        warn: String::new(),
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/networks`：网络列表（只读）。
pub async fn networks(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_networks()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
            .into_iter()
            .map(|n| {
                let item = network_item(handlers::resources::network_view(n));
                (item.id.clone(), item)
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/volumes`：卷列表（只读）。
pub async fn volumes(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_volumes()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                let name = v.name.clone();
                (
                    name.clone(),
                    ResourceItem {
                        id: name.clone(),
                        name,
                        meta: format!("{} → {}", v.driver, v.mountpoint),
                        warn: if v.host_binding {
                            "宿主绑定语义".to_string()
                        } else {
                            String::new()
                        },
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/images/pull`：拉取镜像（同 `images.pull`）。
pub async fn image_pull(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ImagePullBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), skiff_docker::Verb::ImagePull, None)
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let image = body.image.trim().to_string();
    if image.is_empty() {
        return Err(ApiError(AppError::BadRequest("镜像名不能为空".into())));
    }
    skiff_app::commands::docker::image_pull(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: image.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "image": image, "pulled": true }))
}

/// `POST /ui/v1/images/{id}/remove`：删除镜像。
pub async fn image_remove(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ImageRemove,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    if id.trim().is_empty() {
        return Err(ApiError(AppError::BadRequest("镜像 id 不能为空".into())));
    }
    skiff_app::commands::docker::image_remove(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: id.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "id": id, "removed": true }))
}

/// `POST /ui/v1/networks`：创建网络（同 `networks.create`）。
pub async fn network_create(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<NameBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::NetworkCreate,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let name = body.name.trim().to_string();
    handlers::resources::validate_name("网络", &name).map_err(ApiError)?;
    skiff_app::commands::docker::network_create(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: name.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "name": name, "created": true }))
}

/// `POST /ui/v1/networks/{id}/remove`：删除网络。
pub async fn network_remove(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::NetworkRemove,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    skiff_app::commands::docker::network_remove(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: id.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "id": id, "removed": true }))
}

/// `POST /ui/v1/volumes`：创建卷（同 `volumes.create`）。
pub async fn volume_create(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<NameBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::VolumeCreate,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let name = body.name.trim().to_string();
    handlers::resources::validate_name("卷", &name).map_err(ApiError)?;
    skiff_app::commands::docker::volume_create(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: name.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "name": name, "created": true }))
}

/// `POST /ui/v1/volumes/{name}/remove`：删除卷。
pub async fn volume_remove(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(name): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::VolumeRemove,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    skiff_app::commands::docker::volume_remove(
        &state,
        skiff_app::commands::docker::NamedResource {
            name: name.clone(),
            actor: handlers::resources::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::resources::map_err(e)))?;
    write_done(json!({ "id": name, "removed": true }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/images", get(images))
        .route("/ui/v1/images/pull", post(image_pull))
        .route("/ui/v1/images/{id}/remove", post(image_remove))
        .route("/ui/v1/networks", get(networks).post(network_create))
        .route("/ui/v1/networks/{id}/remove", post(network_remove))
        .route("/ui/v1/volumes", get(volumes).post(volume_create))
        .route("/ui/v1/volumes/{name}/remove", post(volume_remove))
}
