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
    match state.db().plugin(&id) {
        Ok(Some(p)) => Json(json!({
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
        _ => (StatusCode::NOT_FOUND, "插件不存在").into_response(),
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
    let rows = match state.db().plugin_config(&id) {
        Ok(v) => v,
        Err(e) => {
            let _ = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                "capability.config",
                "",
                "fail",
                &e.to_string(),
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "读取插件配置失败").into_response();
        }
    };
    let mut obj = serde_json::Map::with_capacity(rows.len());
    for (k, v, _secret) in rows {
        obj.insert(k, Value::String(v));
    }
    // 只记条数，绝不记 key 之外的任何内容（secret 值不得入审计）。
    let _ = state.db().audit(
        &format!("plugin:{id}"),
        "uds",
        "capability.config",
        "",
        "ok",
        &format!("{} 项", obj.len()),
    );
    Json(Value::Object(obj)).into_response()
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
    let target = parse_target(&body);

    let grants: Vec<Grant> = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        _ => Vec::new(),
    };
    let actor = Actor::Plugin {
        id: id.clone(),
        grants,
    };
    if let Err(e) = state.cfg.policy.check(&actor, verb, target.as_ref()) {
        let _ = state.db().audit(
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
        );
        return (StatusCode::FORBIDDEN, e.to_string()).into_response();
    }

    // 动词 → Docker 方法的映射只在 `Docker::execute` 一处，这里不再自己 match
    let result = state
        .docker()
        .execute(verb, target.as_ref(), skiff_docker::DockerParams::None)
        .await;

    match result {
        Ok(v) => {
            let _ = state.db().audit(
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
            );
            Json(json!({ "result": v })).into_response()
        }
        Err(e) => {
            // 「动词需要专用参数」不是引擎故障，如实回 501 而不是 502
            let status = match e {
                skiff_docker::DockerError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::BAD_GATEWAY,
            };
            let _ = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                "",
                if status == StatusCode::NOT_IMPLEMENTED {
                    "unsupported"
                } else {
                    "fail"
                },
                &e.to_string(),
            );
            (status, e.to_string()).into_response()
        }
    }
}

/// 解析 `target`。缺省或显式 `null` 表示「无目标」（如 `containers.list`）。
///
/// 不能把 `null` 当成 id/name 全空的容器：策略层会要求 `object:container:` 这条
/// 永远匹配不上的授权，插件侧只能看到误导性的 403（2026-11-03 实测：
/// nginx `sites` 静默降级、`reload` 报「缺少授权 object:container:」）。
fn parse_target(body: &Value) -> Option<skiff_docker::ObjectRef> {
    let t = body.get("target")?;
    if t.is_null() {
        return None;
    }
    Some(skiff_docker::ObjectRef {
        id: t
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        name: t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        labels: t
            .get("labels")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
    })
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
    let _ = state.db().audit(
        &format!("plugin:{id}"),
        "uds",
        "net.apply",
        "",
        "unsupported",
        "nftables 出网收口未实现（M4 门禁）",
    );
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
    fn absent_or_null_target_means_no_target() {
        assert!(parse_target(&json!({ "verb": "containers.list" })).is_none());
        assert!(parse_target(&json!({ "verb": "containers.list", "target": null })).is_none());
    }

    #[test]
    fn object_target_is_parsed() {
        let t = parse_target(&json!({
            "target": { "id": "abc123", "name": "nginx", "labels": { "app": "nginx" } }
        }))
        .expect("object target");
        assert_eq!(t.id, "abc123");
        assert_eq!(t.name, "nginx");
        assert_eq!(t.labels.get("app").map(String::as_str), Some("nginx"));
    }
}
