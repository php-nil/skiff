use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/compose`：compose 项目列表；选中项目时附带服务与最近日志。
///
/// 与旧页面同一 `status_of`（容器标签归组，不持久化 compose 源文件）。
pub async fn compose(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ComposeQuery>,
) -> Result<Response, ApiError> {
    let selected = q.project.unwrap_or_default();
    let (projects, services, docker_error) = handlers::compose::status_of(&state, &selected).await;
    let projects: Vec<ComposeProjectItem> = projects
        .into_iter()
        .map(|p| ComposeProjectItem {
            name: p.name,
            managed: p.managed,
            services: p.services,
            running: p.running,
        })
        .collect();
    let services: Vec<ComposeServiceItem> = services
        .into_iter()
        .map(|s| ComposeServiceItem {
            service: s.service,
            container: s.container,
            id: s.id,
            state: StatusDto {
                code: s.state.clone(),
                label: container_state_label(&s.state).to_string(),
            },
            status: s.status,
            image: s.image,
            managed: s.managed,
            logs: s.logs,
        })
        .collect();
    let mut resp = Json(json!({
        "data": {
            "projects": projects,
            "services": services,
            "selected_project": selected,
            "docker_available": state.docker().available(),
            "docker_reason": docker_error,
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/compose/down`：清理内核托管项目（外部项目只读）。
pub async fn compose_down(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ComposeDownBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let project = skiff_app::commands::docker::prepare_project(&body.project)
        .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let value = skiff_app::commands::docker::compose_down(
        &state,
        skiff_app::commands::docker::ComposeDown {
            project: project.clone(),
            remove_volumes: body.remove_volumes,
            actor: handlers::compose::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let mut resp = Json(json!({
        "data": {
            "project": project,
            "remove_volumes": body.remove_volumes,
            "report": value,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/compose/validate`：只解析与校验，不触达 Docker，不改状态。
pub async fn compose_validate(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ComposeSourceBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let project = planned_compose(&state, &user, &body)?;
    let services: Vec<String> = project.services.iter().map(|s| s.service.clone()).collect();
    write_done(json!({
        "project": project.name,
        "services": services,
        "networks": project.networks,
        "volumes": project.volumes,
        "valid": true,
    }))
}

/// `POST /ui/v1/compose/up`：校验通过后部署项目；部分服务失败如实回传。
pub async fn compose_up(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ComposeSourceBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let project = planned_compose(&state, &user, &body)?;
    let name = project.name.clone();
    let value = skiff_app::commands::docker::compose_up(
        &state,
        skiff_app::commands::docker::ComposeUp {
            project,
            actor: handlers::compose::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let failed = value
        .get("failed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let services = value
        .get("services")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));
    write_done(json!({ "project": name, "failed": failed, "services": services }))
}

/// `POST /ui/v1/compose/{project}/{action}`：项目级启动/停止/重启。
///
/// 逐容器复用既有动词分发与权限判定；策略拒绝计入 `failed`，不中断其余容器。
pub async fn compose_project_action(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((project, action)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let verb = match action.as_str() {
        "start" => skiff_docker::Verb::Start,
        "stop" => skiff_docker::Verb::Stop,
        "restart" => skiff_docker::Verb::Restart,
        _ => return Err(ApiError(AppError::BadRequest("未知的项目动作".into()))),
    };
    let project = skiff_app::commands::docker::prepare_project(&project)
        .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let rows = state
        .docker()
        .list_containers()
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    let mut targets = Vec::new();
    let mut failed = 0usize;
    for row in rows
        .iter()
        .filter(|r| skiff_docker::project_of(&r.labels) == Some(project.as_str()))
    {
        let obj = handlers::object_ref(row);
        if state
            .cfg
            .policy
            .check(&handlers::actor(&user), verb, Some(&obj))
            .is_err()
        {
            failed += 1;
            continue;
        }
        targets.push(skiff_app::commands::docker::ContainerTarget::new(
            row.name.clone(),
            row.short_id(),
            obj,
        ));
    }
    let report = skiff_app::commands::docker::compose_project_action(
        &state,
        skiff_app::commands::docker::ProjectAction {
            project: project.clone(),
            targets,
            verb,
            actor: handlers::compose::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let mut resp = Json(json!({
        "data": {
            "project": project,
            "action": action,
            "done": report.done,
            "failed": failed + report.failed,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/compose", get(compose))
        .route("/ui/v1/compose/validate", post(compose_validate))
        .route("/ui/v1/compose/up", post(compose_up))
        .route("/ui/v1/compose/down", post(compose_down))
        .route(
            "/ui/v1/compose/{project}/{action}",
            post(compose_project_action),
        )
}
