//! 能力面 handler：identity / config / fs / docker / net。
//!
//! fs 与 interop 的**判定**住在 `skiff-plugin`（能力面的正确归属地，且判定与
//! IO 细节与平台无关，Windows 也能单测）；这里只做「取会话上下文 → 调判定 →
//! 组装响应 → 审计」。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::db_async;
use skiff_app::AppState;
use skiff_docker::{Actor, Verb};
use skiff_store::Grant;

use skiff_plugin::capability as cap;

// 能力面 handler 的薄适配层：把 axum 的 `State<Arc<AppState>>` 收敛成
// `&impl CapabilityHost`。能力面的实现因此不必依赖 `skiff-app`——否则
// `skiff-app → skiff-plugin → skiff-app` 成环。
pub(super) async fn fs_read(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_read(&*state, req).await
}
pub(super) async fn fs_write(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_write(&*state, req).await
}
pub(super) async fn fs_list(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_list(&*state, req).await
}
pub(super) async fn fs_mkdir(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_mkdir(&*state, req).await
}
pub(super) async fn fs_remove(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_remove(&*state, req).await
}
pub(super) async fn fs_rename(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    cap::fs_rename(&*state, req).await
}

pub(super) async fn identity_self(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let Some(id) = req.extensions().get::<cap::PluginId>().map(|p| p.0.clone()) else {
        return (StatusCode::FORBIDDEN, "未认证").into_response();
    };
    let plugin = {
        let db = Arc::clone(state.db());
        let id = id.clone();
        db_async::run(move || db.plugin(&id)).await
    };
    match plugin {
        Ok(Ok(Some(p))) => Json(json!({
            "id": p.id,
            "version": p.version,
            "api": p.api,
            // `granted` 保留原形态（`perm:fs:read` / `object:path:/etc`）用于兼容；
            // 新代码用下面两个**已拆开**的字段，免得调用方自己剥前缀。
            "granted": p.grants.iter().map(|g| format!("{}:{}", g.kind, g.value)).collect::<Vec<_>>(),
            "permissions": p.grants.iter().filter(|g| g.kind == "perm").map(|g| g.value.clone()).collect::<Vec<_>>(),
            "objects": p.grants.iter().filter(|g| g.kind == "object").map(|g| g.value.clone()).collect::<Vec<_>>(),
        }))
        .into_response(),
        Ok(Ok(None)) => (StatusCode::NOT_FOUND, "插件不存在").into_response(),
        // 「查库失败」不是「插件不存在」：不能把基础设施故障伪装成 404。
        Ok(Err(e)) => {
            tracing::error!(plugin = %id, error = %e, "读取插件信息失败");
            (StatusCode::INTERNAL_SERVER_ERROR, "读取插件信息失败").into_response()
        }
        Err(e) => {
            tracing::error!(plugin = %id, error = %e, "投递插件信息查询失败");
            (StatusCode::INTERNAL_SERVER_ERROR, "读取插件信息失败").into_response()
        }
    }
}

/// `config_schema` 声明项的取值。内核代管于 `skiff.db`（§5.2），插件运行时一次性取回。
///
/// 与 SDK `Plugin::config()` 一一对应——此前 SDK 打 `/api/v1/capability/config` 而内核
/// 没有这条路由，插件只要读到配置就拿到 404，属于「文档有、代码没有」的硬缺口。
///
/// `secret` 项**原样返回**：调用方已被 UID + Bearer token 双因素绑定为插件自身，
/// 值本身也不在插件可写目录里；但**禁止**把值写进日志或审计 detail。
pub(super) async fn config_get(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let rows = {
        let db = Arc::clone(state.db());
        let id = id.clone();
        db_async::run(move || db.plugin_config(&id)).await
    };
    let rows = match rows {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            let db = Arc::clone(state.db());
            let actor = format!("plugin:{id}");
            let detail = e.to_string();
            let audited = db_async::run(move || {
                db.audit(&actor, "uds", "capability.config", "", "fail", &detail)
            })
            .await;
            match audited {
                Ok(Ok(())) => {}
                Ok(Err(ae)) => tracing::error!(error = %ae, "能力面配置审计写入失败"),
                Err(ae) => tracing::error!(error = %ae, "投递能力面配置审计失败"),
            }
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取插件配置失败").into_response();
        }
        Err(e) => {
            tracing::error!(plugin = %id, error = %e, "投递插件配置查询失败");
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取插件配置失败").into_response();
        }
    };
    let mut obj = serde_json::Map::with_capacity(rows.len());
    for (k, v, _secret) in rows {
        obj.insert(k, Value::String(v));
    }
    // 只记条数，绝不记 key 之外的任何内容（secret 值不得入审计）。
    let db = Arc::clone(state.db());
    let actor = format!("plugin:{id}");
    let detail = format!("{} 项", obj.len());
    let audited =
        db_async::run(move || db.audit(&actor, "uds", "capability.config", "", "ok", &detail))
            .await;
    match audited {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!(error = %e, "能力面配置审计写入失败"),
        Err(e) => tracing::error!(error = %e, "投递能力面配置审计失败"),
    }
    Json(Value::Object(obj)).into_response()
}

/// `scheduler` 能力面：任务列表/注册/更新/运行/缓冲。
///
/// 两条硬边界在**路由层**先拦，避免把非法调用送进调度器：
/// - 未初始化任务服务 → 503；
/// - 插件正作为任务执行目标（含 interop 与插件任务调用） → 403 + `scheduler.deny`。
///   否则「插件任务执行 → 回调 scheduler 注册新任务」会自我繁殖。
pub(super) async fn scheduler_call(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let audit = |action: &str, result: &str, detail: &str| {
        if let Err(e) =
            state
                .db()
                .audit(&format!("plugin:{id}"), "uds", action, &id, result, detail)
        {
            tracing::error!(error = %e, action, "能力面调度审计写入失败");
        }
    };

    let Some(tasks) = state.tasks() else {
        audit("scheduler.deny", "fail", "任务服务未初始化");
        return (StatusCode::SERVICE_UNAVAILABLE, "任务服务未初始化").into_response();
    };
    let busy = state.is_interop_busy(&id).await || tasks.is_executing(&id);
    if busy {
        audit("scheduler.deny", "deny", "插件正在作为任务执行目标");
        return (
            StatusCode::FORBIDDEN,
            "插件正在执行任务，不能再调用 scheduler",
        )
            .into_response();
    }

    let body = cap::read_json(req).await;
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
    match tasks.capability(&id, method, params).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            // 授权/归属类错误记 deny，参数类错误记 fail：审计要能区分「越权」与「写错」。
            let denied = e.contains("缺少") || e.contains("只能") || e.contains("未知");
            audit(
                "scheduler.deny",
                if denied { "deny" } else { "fail" },
                &format!("{method}: {e}"),
            );
            let status = if denied {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, e).into_response()
        }
    }
}

pub(super) async fn docker_call(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;
    let verb_str = body.get("verb").and_then(|v| v.as_str()).unwrap_or("");
    let Some(verb) = Verb::parse(verb_str) else {
        return (StatusCode::FORBIDDEN, "动词不在白名单").into_response();
    };

    // 授权判定只认**引擎状态**：容器类动词必须带 id，并由引擎返回的 name/labels
    // 重建 `ObjectRef`。请求体里的 name/labels 是调用方自报值，绝不能进判定——
    // 否则伪造 `io.skiff.owner` 就能对任意容器执行非破坏性动词。
    let target = if verb_targets_container(verb) {
        let Some(target_id) = body
            .get("target")
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            if let Err(w) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                "",
                "deny",
                "缺少容器 id（不接受只用名字）",
            ) {
                tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
            }
            return (StatusCode::BAD_REQUEST, "需要对象 id（不能只用名字）").into_response();
        };
        match state.docker().object_ref(target_id).await {
            Ok(t) => Some(t),
            Err(e) => {
                if let Err(w) = state.db().audit(
                    &format!("plugin:{id}"),
                    "uds",
                    &verb.audit_action(),
                    "",
                    "fail",
                    &e.to_string(),
                ) {
                    tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
                }
                return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
            }
        }
    } else {
        // 其余动词（列表/镜像/网络/卷等）在通用分发里没有容器对象可判定，
        // 执行层会返回「需要专用参数」；不把请求体目标带进判定。
        None
    };

    let grants: Vec<Grant> = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        Ok(None) => {
            // 鉴权中间件已确认插件存在；走到这里说明库与 UDS 会话不一致（刚被卸载）。
            if let Err(w) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                "",
                "deny",
                "插件记录不存在（可能已被卸载）",
            ) {
                tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
            }
            return (StatusCode::FORBIDDEN, "插件记录不存在").into_response();
        }
        // 查库失败不是越权：按依赖不可用回 503，不能伪装成 deny。
        Err(e) => {
            if let Err(w) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                "",
                "fail",
                &format!("读取插件授权失败: {e}"),
            ) {
                tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "读取插件授权失败，请稍后重试",
            )
                .into_response();
        }
    };
    let actor = Actor::Plugin {
        id: id.clone(),
        grants,
    };
    if let Err(e) = state.cfg.policy.check(&actor, verb, target.as_ref()) {
        if let Err(w) = state.db().audit(
            &format!("plugin:{id}"),
            "uds",
            &verb.audit_action(),
            target
                .as_ref()
                .map(|t| t.name.clone())
                .unwrap_or_default()
                .as_str(),
            "deny",
            &e.to_string(),
        ) {
            tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
        }
        return (StatusCode::FORBIDDEN, e.to_string()).into_response();
    }

    // `containers.logs` 的尾行数由参数传入；其余动词沿用强类型参数或 None。
    let params = match verb {
        Verb::ContainerLogs => skiff_docker::DockerParams::ContainerLogs {
            tail: body
                .get("tail")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(10_000) as usize,
        },
        _ => skiff_docker::DockerParams::None,
    };

    // 动词 → Docker 方法的映射只在 `Docker::execute` 一处，这里不再自己 match
    let result = state.docker().execute(verb, target.as_ref(), params).await;

    match result {
        Ok(v) => {
            if let Err(w) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                target
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default()
                    .as_str(),
                "ok",
                "",
            ) {
                tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
            }
            Json(json!({ "result": v })).into_response()
        }
        Err(e) => {
            // 「动词需要专用参数」不是引擎故障，如实回 501 而不是 502
            let status = match e {
                skiff_docker::DockerError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::BAD_GATEWAY,
            };
            if let Err(w) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                target
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default()
                    .as_str(),
                if status == StatusCode::NOT_IMPLEMENTED {
                    "unsupported"
                } else {
                    "fail"
                },
                &e.to_string(),
            ) {
                tracing::warn!(plugin = %id, error = %w, "写入能力面审计失败");
            }
            (status, e.to_string()).into_response()
        }
    }
}

/// 目标必须是**容器**的动词。只有这些动词才允许携带容器 id，并必须由引擎重建
/// `ObjectRef`；`exec.*` 的 id 是 exec 实例、镜像/网络/卷的引用也不是容器 id，
/// 不能走同一条重建路径。
fn verb_targets_container(verb: Verb) -> bool {
    use Verb::*;
    matches!(
        verb,
        ContainerInspect
            | ContainerLogs
            | ContainerStats
            | Start
            | Stop
            | Restart
            | Kill
            | Exec
            | ContainerUpdate
            | ContainerRemove
    )
}

/// `net.apply` 尚未实现（nftables 出网收口属 M4 硬门槛）：一律 501 + 审计，
/// 不做「假装成功」。
pub(super) async fn net_unimplemented(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    if let Err(e) = state.db().audit(
        &format!("plugin:{id}"),
        "uds",
        "net.apply",
        "",
        "unsupported",
        "nftables 出网收口未实现（M4 门禁）",
    ) {
        tracing::error!(error = %e, "能力面 net 审计写入失败");
    }
    (
        StatusCode::NOT_IMPLEMENTED,
        "net 能力未实现：nftables owner-match 收口属 M4 硬门槛（§9.1 #17）",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_container_verbs_require_engine_object() {
        assert!(verb_targets_container(Verb::Stop));
        assert!(verb_targets_container(Verb::ContainerInspect));
        assert!(verb_targets_container(Verb::ContainerLogs));
        assert!(!verb_targets_container(Verb::ExecInspect));
        assert!(!verb_targets_container(Verb::ImagePull));
        assert!(!verb_targets_container(Verb::ContainersList));
        assert!(!verb_targets_container(Verb::ComposeUp));
    }
}
