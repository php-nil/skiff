use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/containers`：容器列表（只读）。
///
/// 与旧页面同一份 `list_containers` + `container_view`，支持 `filter=error`；
/// Docker 不可用时返回空列表与原因（页面同口径），不臆造状态。
pub async fn containers(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ContainerListQuery>,
) -> Result<Response, ApiError> {
    let docker_ok = state.docker().available();
    let rows = if docker_ok {
        state
            .docker()
            .list_containers()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
    } else {
        Vec::new()
    };
    let filter = match q.filter.as_deref() {
        Some("error") => "error",
        _ => "",
    };
    let items: Vec<ContainerItem> = rows
        .iter()
        .filter(|c| filter.is_empty() || handlers::container_abnormal(&c.state))
        .map(container_item)
        .collect();
    let total = items.len();
    let start = match q.cursor.as_deref() {
        Some(cursor) => items
            .iter()
            .position(|c| c.id == cursor)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let end = (start + q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)).min(total);
    let next_cursor = if end < total && end > start {
        items.get(end - 1).map(|c| c.id.clone())
    } else {
        None
    };
    let page: Vec<ContainerItem> = items.into_iter().skip(start).take(end - start).collect();
    let mut resp = Json(json!({
        "data": {
            "items": page,
            "total": total,
            "next_cursor": next_cursor,
            "filter": filter,
            "docker_available": docker_ok,
            "docker_reason": state.docker_unavailable_reason(),
            "can_operate": user.can(skiff_model::Permission::DockerOperate),
            "can_delete": user.can(skiff_model::Permission::DockerLifecycle),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/containers/{id}`：容器详情（只读）。
pub async fn container_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ContainerInspect,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let inspect = state
        .docker()
        .inspect_container(&row.id)
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;

    let ports = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.ports.clone())
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let (port, proto) = k.split_once('/').unwrap_or((k.as_str(), "tcp"));
                    let host = v
                        .as_ref()
                        .and_then(|b| b.first())
                        .and_then(|b| b.host_port.clone())
                        .unwrap_or_default();
                    ContainerPort {
                        host,
                        container: port.to_string(),
                        protocol: proto.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mounts = inspect
        .mounts
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| ContainerMount {
                    kind: m.typ.clone().map(|t| t.to_string()).unwrap_or_default(),
                    source: m.source.clone().unwrap_or_default(),
                    target: m.destination.clone().unwrap_or_default(),
                    read_only: m.rw == Some(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let networks = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.networks.clone())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let labels = inspect
        .config
        .as_ref()
        .and_then(|c| c.labels.clone())
        .map(|m| {
            let mut pairs: Vec<ContainerLabel> = m
                .into_iter()
                .map(|(key, value)| ContainerLabel { key, value })
                .collect();
            pairs.sort_by(|a, b| a.key.cmp(&b.key));
            pairs
        })
        .unwrap_or_default();
    let cmd = inspect
        .config
        .as_ref()
        .and_then(|c| c.cmd.clone())
        .map(|v| v.join(" "))
        .unwrap_or_default();
    let health = inspect
        .state
        .as_ref()
        .and_then(|s| s.health.as_ref())
        .and_then(|h| h.status)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());

    let hc = inspect.host_config.unwrap_or_default();
    let mb = |bytes: Option<i64>| match bytes {
        Some(v) if v < 0 => "不限制".to_string(),
        Some(0) | None => "默认".to_string(),
        Some(v) => format!("{} MB", v / (1024 * 1024)),
    };
    let limits_effective = vec![
        ContainerLimitLine {
            label: "CPU 份额".into(),
            value: hc
                .cpu_shares
                .map(|v| v.to_string())
                .filter(|v| v != "0")
                .unwrap_or_else(|| "默认".into()),
        },
        ContainerLimitLine {
            label: "CPU 周期".into(),
            value: hc
                .cpu_period
                .map(|v| format!("{v} μs"))
                .filter(|v| v != "0 μs")
                .unwrap_or_else(|| "默认".into()),
        },
        ContainerLimitLine {
            label: "CPU 配额".into(),
            value: match hc.cpu_quota {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => format!("{v} μs/周期"),
            },
        },
        ContainerLimitLine {
            label: "内存上限".into(),
            value: mb(hc.memory),
        },
        ContainerLimitLine {
            label: "内存+交换".into(),
            value: mb(hc.memory_swap),
        },
        ContainerLimitLine {
            label: "PIDs 上限".into(),
            value: match hc.pids_limit {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => v.to_string(),
            },
        },
    ];
    let limits = ContainerLimits {
        cpu_shares: hc.cpu_shares.map(|v| v.to_string()).unwrap_or_default(),
        cpu_period: hc.cpu_period.map(|v| v.to_string()).unwrap_or_default(),
        cpu_quota: hc.cpu_quota.map(|v| v.to_string()).unwrap_or_default(),
        memory_limit_mb: hc
            .memory
            .filter(|v| *v >= 0)
            .map(|v| (v / (1024 * 1024)).to_string())
            .unwrap_or_default(),
        memory_swap_mb: hc
            .memory_swap
            .map(|v| {
                if v < 0 {
                    v.to_string()
                } else {
                    (v / (1024 * 1024)).to_string()
                }
            })
            .unwrap_or_default(),
        pids_limit: hc.pids_limit.map(|v| v.to_string()).unwrap_or_default(),
    };

    let data = ContainerDetailData {
        container: container_item(&row),
        image: row.image.clone(),
        cmd,
        created_at: inspect.created.clone().unwrap_or_default(),
        health,
        ports,
        mounts,
        networks,
        labels,
        limits,
        limits_effective,
        can_exec: user.can(skiff_model::Permission::DockerExec),
        can_operate: user.can(skiff_model::Permission::DockerOperate),
        can_delete: user.can(skiff_model::Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/containers/{id}/logs?tail=`：容器日志尾部快照（只读）。
///
/// 与旧 SSE 流同策略（`Verb::ContainerLogs` → `DockerRead`），只是改成一次性取尾行；
/// 页面按需刷新，不建立长连接。
pub async fn container_logs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ContainerLogs,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let tail = q
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(500)
        .clamp(1, 2000);
    let text = state
        .docker()
        .logs(&row.id, tail)
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    Ok(Json(json!({
        "data": {
            "id": row.id,
            "name": row.name,
            "state": row.state,
            "tail": tail,
            "text": text,
        }
    })))
}

/// `POST /ui/v1/containers/{id}/{action}`：启动/停止/重启/删除。
///
/// 与旧 htmx 路径同一策略判定、同一审计与同一命令；CSRF 用 `/ui/v1` 统一的
/// `x-csrf-token` 头（与登出同源），仅传输层改为 JSON。
pub async fn container_action(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    let verb = handlers::containers::action_verb(&action).map_err(ApiError)?;
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    let obj = handlers::object_ref(&row);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), verb, Some(&obj))
        .map_err(|e| {
            state.audit_or_log(
                &user.user.username,
                &handlers::client_ip(&state, &parts),
                &verb.audit_action(),
                &row.name,
                "deny",
                &e.to_string(),
            );
            ApiError(AppError::Forbidden(e.to_string()))
        })?;
    let cmd = skiff_app::commands::docker::ContainerAction {
        target: skiff_app::commands::docker::ContainerTarget::new(
            row.name.clone(),
            row.short_id(),
            obj,
        ),
        verb,
        actor: handlers::containers::actor(&state, &user, &parts),
    };
    skiff_app::commands::docker::container_action(&state, cmd)
        .await
        .map_err(|e| ApiError(handlers::containers::map_err(e)))?;
    let removed = verb == skiff_docker::Verb::ContainerRemove;
    let mut resp = Json(json!({
        "data": {
            "id": row.id,
            "name": row.name,
            "action": action,
            "removed": removed,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/containers`：新建容器。
///
/// 直接构造命令层的 `SpecFields`，与旧表单路径共用同一套解析与 `validate_spec`，
/// 但不再先把 JSON 降级成 `HashMap` 表单。
pub async fn container_create(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ContainerCreateBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ContainerCreate,
            None,
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let spec = handlers::containers::spec_from_fields(handlers::containers::SpecFields {
        image: body.image.clone(),
        name: body.name.clone(),
        restart: body.restart.clone(),
        binds: body.binds.clone(),
        volumes: body.volumes.clone(),
        env: body.env.clone(),
        ports: body.ports.clone(),
        networks: body.networks.clone(),
        memory_limit_mb: body.memory_limit_mb.clone(),
        pids_limit: body.pids_limit.clone(),
        cpu_shares: body.cpu_shares.clone(),
        cpu_period: body.cpu_period.clone(),
        cpu_quota: body.cpu_quota.clone(),
        memory_swap_mb: body.memory_swap_mb.clone(),
    })
    .map_err(ApiError)?;
    state
        .cfg
        .policy
        .validate_spec(&spec, &handlers::actor(&user))
        .map_err(|e| ApiError(AppError::BadRequest(e.to_string())))?;
    let id = skiff_app::commands::docker::container_create(
        &state,
        skiff_app::commands::docker::CreateContainer {
            spec,
            actor: handlers::containers::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::containers::map_err(e)))?;
    let mut resp = Json(json!({ "data": { "id": id } })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/containers/{id}/limits`：资源限制热更新（同 `containers.update`）。
pub async fn container_limits_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<ContainerLimitsBody>,
) -> Result<Response, ApiError> {
    guard::csrf(&state, &user, &parts)?;
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    let obj = handlers::object_ref(&row);
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ContainerUpdate,
            Some(&obj),
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let limits = handlers::containers::limits_from_fields(handlers::containers::LimitFields {
        cpu_shares: body.cpu_shares.clone(),
        cpu_period: body.cpu_period.clone(),
        cpu_quota: body.cpu_quota.clone(),
        memory_limit_mb: body.memory_limit_mb.clone(),
        memory_swap_mb: body.memory_swap_mb.clone(),
        pids_limit: body.pids_limit.clone(),
    })
    .map_err(ApiError)?;
    state
        .cfg
        .policy
        .validate_limits(&limits)
        .map_err(|e| ApiError(AppError::BadRequest(e.to_string())))?;
    skiff_app::commands::docker::container_update(
        &state,
        skiff_app::commands::docker::UpdateContainer {
            target: skiff_app::commands::docker::ContainerTarget::new(
                row.name.clone(),
                row.short_id(),
                obj,
            ),
            limits,
            actor: handlers::containers::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::containers::map_err(e)))?;
    let mut resp = Json(json!({
        "data": { "id": row.id, "name": row.name, "updated": true }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/containers", get(containers).post(container_create))
        .route("/ui/v1/containers/{id}", get(container_detail))
        .route("/ui/v1/containers/{id}/logs", get(container_logs))
        .route(
            "/ui/v1/containers/{id}/limits",
            post(container_limits_update),
        )
        .route("/ui/v1/containers/{id}/{action}", post(container_action))
}
