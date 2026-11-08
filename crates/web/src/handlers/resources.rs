use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_docker::Verb;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{ResourceRow, ResourcesPage};
use crate::util::{form_first, human_size, parse_form};

fn page(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    section: &str,
    title: &str,
    rows: Vec<ResourceRow>,
    notice: &str,
    error: &str,
) -> Result<Response, AppError> {
    handlers::render(ResourcesPage {
        ctx: handlers::page_ctx(state, user, section),
        section: section.to_string(),
        title: title.to_string(),
        rows,
        can_admin: user.can(skiff_model::Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
        error: error.to_string(),
        notice: notice.to_string(),
    })
}

pub async fn images(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let rows = match state.docker().list_images().await {
        Ok(items) => items
            .into_iter()
            .map(|i| ResourceRow {
                id: i.id.clone(),
                name: if i.tags.is_empty() {
                    format!("<none> ({})", i.short_id())
                } else {
                    i.tags.join(", ")
                },
                meta: human_size(i.size),
                warn: String::new(),
            })
            .collect(),
        Err(e) => {
            return page(&state, &user, "images", "镜像", vec![], "", &e.to_string());
        }
    };
    page(&state, &user, "images", "镜像", rows, "", "")
}

pub async fn pull_image(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    // 授权先于参数校验：未授权者不该拿到参数层面的反馈。
    // 判定走 `policy.check`（内部查 `Permission` 矩阵），不再另设 `require_admin`。
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ImagePull, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let image = form_first(&form, "image").trim().to_string();
    if image.is_empty() {
        return Err(AppError::BadRequest("镜像名不能为空".into()));
    }

    let result = state.docker().pull_image(&image).await;
    let ip = handlers::client_ip(&state, &parts);
    let (result_str, detail) = match &result {
        Ok(()) => ("ok", String::new()),
        Err(e) => ("fail", e.to_string()),
    };
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::ImagePull.audit_action(),
        &image,
        result_str,
        &detail,
    );

    let rows = state
        .docker()
        .list_images()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|i| ResourceRow {
            id: i.id.clone(),
            name: if i.tags.is_empty() {
                i.id.clone()
            } else {
                i.tags.join(", ")
            },
            meta: human_size(i.size),
            warn: String::new(),
        })
        .collect();
    let notice = if result.is_ok() {
        format!("已拉取 {image}")
    } else {
        String::new()
    };
    let err = result.err().map(|e| e.to_string()).unwrap_or_default();
    page(&state, &user, "images", "镜像", rows, &notice, &err)
}

pub async fn remove_image(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ImageRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let result = state.docker().remove_image(&id, false).await;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::ImageRemove.audit_action(),
        &id,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    images(State(state), AuthUser(user)).await
}

fn network_page(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    rows: Vec<crate::templates::NetworkRowView>,
    notice: &str,
    error: &str,
) -> Result<Response, AppError> {
    handlers::render(crate::templates::NetworksPage {
        ctx: handlers::page_ctx(state, user, "networks"),
        rows,
        can_admin: user.can(skiff_model::Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
        error: error.to_string(),
        notice: notice.to_string(),
    })
}

fn network_view(n: skiff_docker::NetworkRow) -> crate::templates::NetworkRowView {
    use crate::templates::{NetworkContainerView, NetworkRowView};

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
    }
}

pub async fn networks(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    match state.docker().list_networks().await {
        Ok(items) => network_page(
            &state,
            &user,
            items.into_iter().map(network_view).collect(),
            "",
            "",
        ),
        Err(e) => network_page(&state, &user, Vec::new(), "", &e.to_string()),
    }
}

pub async fn create_network(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::NetworkCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let name = form_first(&form, "name").trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        return Err(AppError::BadRequest("网络名只允许字母数字与 _-. ".into()));
    }
    let result = state.docker().create_network(&name).await;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::NetworkCreate.audit_action(),
        &name,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    networks(State(state), AuthUser(user)).await
}

pub async fn remove_network(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::NetworkRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let result = state.docker().remove_network(&id).await;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::NetworkRemove.audit_action(),
        &id,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    networks(State(state), AuthUser(user)).await
}

pub async fn volumes(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let rows = state
        .docker()
        .list_volumes()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|v| ResourceRow {
            id: v.name.clone(),
            name: v.name,
            meta: format!("{} → {}", v.driver, v.mountpoint),
            warn: if v.host_binding {
                "宿主绑定语义".to_string()
            } else {
                String::new()
            },
        })
        .collect();
    page(&state, &user, "volumes", "卷", rows, "", "")
}

pub async fn create_volume(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::VolumeCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let name = form_first(&form, "name").trim().to_string();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        return Err(AppError::BadRequest("卷名只允许字母数字与 _-. ".into()));
    }
    let result = state.docker().create_volume(&name).await;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::VolumeCreate.audit_action(),
        &name,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    volumes(State(state), AuthUser(user)).await
}

pub async fn remove_volume(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(name): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::VolumeRemove, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    let result = state.docker().remove_volume(&name).await;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        &Verb::VolumeRemove.audit_action(),
        &name,
        if result.is_ok() { "ok" } else { "fail" },
        &result
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default(),
    );
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    volumes(State(state), AuthUser(user)).await
}
