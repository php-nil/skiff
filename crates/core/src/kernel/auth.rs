//! kernel.sock 的两道闸：协议版本 + 双重鉴权（peer uid ↔ 插件 id + Bearer token）。

use super::PeerUid;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use skiff_app::db_async;
use skiff_app::AppState;

/// 协议版本闸：插件必须声明与内核一致的协议版本（§4.2）。
///
/// 缺版本头 = 老插件（协议头是后加的）；版本不符 = 插件与内核不同步。
/// 两种情况都在这里明确拒绝，而不是让它撞上 404 或缺字段的含糊响应。
pub(super) async fn protocol_guard(req: Request<Body>, next: axum::middleware::Next) -> Response {
    let declared = req
        .headers()
        .get(skiff_wire::PROTOCOL_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u32>().ok());
    if let Err(e) = skiff_wire::check_protocol(declared) {
        return (StatusCode::UPGRADE_REQUIRED, e.to_string()).into_response();
    }
    next.run(req).await
}

/// 双重鉴权：`SO_PEERCRED` 的 uid 必须映射到某个已安装插件，
/// 且请求带的 Bearer token 必须与该插件的进程级令牌一致。
///
/// 单一因素都不授权：socket 可达不等于有权限，token 对也不等于来源可信。
pub(super) async fn plugin_auth(
    State(state): State<Arc<AppState>>,
    mut req: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let pid = req.extensions().get::<PeerUid>().map(|p| p.0);
    let Some(uid) = pid else {
        return (StatusCode::FORBIDDEN, "缺少 peer 凭据").into_response();
    };
    // UID → 插件 id
    let plugin_id = match uid_to_plugin(&state, uid).await {
        Some(id) => id,
        None => {
            let db = Arc::clone(state.db());
            let actor = format!("uid:{uid}");
            let audited = db_async::run(move || {
                db.audit(
                    &actor,
                    "uds",
                    "capability.auth",
                    "",
                    "deny",
                    "UID 未映射到插件",
                )
            })
            .await;
            match audited {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!(error = %e, "能力面鉴权审计写入失败"),
                Err(e) => tracing::error!(error = %e, "投递能力面鉴权审计失败"),
            }
            return (StatusCode::FORBIDDEN, "UID 未映射到任何插件").into_response();
        }
    };
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .trim()
        .to_string();
    let expected = state
        .supervisor()
        .token_of(&plugin_id)
        .await
        .unwrap_or_default();
    if expected.is_empty() || !skiff_identity::token_matches(&expected, &token) {
        let db = Arc::clone(state.db());
        let actor = format!("plugin:{plugin_id}");
        let target = plugin_id.clone();
        let audited = db_async::run(move || {
            db.audit(
                &actor,
                "uds",
                "capability.auth",
                &target,
                "deny",
                "token 不匹配",
            )
        })
        .await;
        match audited {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(error = %e, "能力面鉴权审计写入失败"),
            Err(e) => tracing::error!(error = %e, "投递能力面鉴权审计失败"),
        }
        return (StatusCode::FORBIDDEN, "插件身份令牌无效").into_response();
    }
    req.extensions_mut()
        .insert(skiff_plugin::capability::PluginId(plugin_id));
    next.run(req).await
}

async fn uid_to_plugin(state: &AppState, uid: u32) -> Option<String> {
    let db = Arc::clone(state.db());
    let plugins = db_async::run(move || db.list_plugins()).await.ok()?.ok()?;
    plugins
        .into_iter()
        .find(|p| skiff_plugin::install::plugin_uid(&p.id) == uid)
        .map(|p| p.id)
}
