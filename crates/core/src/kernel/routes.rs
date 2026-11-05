//! 能力面路由表：路由常量 + 中间件顺序。
//!
//! 路由常量来自 skiff-wire，与插件 SDK 共用一份；写死字面量曾导致
//! SDK 调用 `capability/config` 而内核没有该路由（永久 404）。

use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use skiff_app::AppState;
use skiff_wire::routes as wire;

use super::{auth, capability, interop};

/// 能力面路由表。
///
/// **中间件顺序**：axum 的 `Router::layer` 是「后加的先跑」（后加 = 更外层）。
/// 所以协议版本闸放在**最后**挂载，保证它先于鉴权执行——版本不符的插件即使
/// token 正确也不该被服务，否则它会在拿到「404 / 字段缺失」这种含糊响应后
/// 继续按旧语义行事。
pub(super) fn build(state: Arc<AppState>) -> Router {
    Router::new()
        .route(wire::CAP_IDENTITY_SELF, post(capability::identity_self))
        .route(wire::CAP_CONFIG, post(capability::config_get))
        .route(wire::CAP_FS_READ, post(capability::fs_read))
        .route(wire::CAP_FS_WRITE, post(capability::fs_write))
        .route(wire::CAP_FS_LIST, post(capability::fs_list))
        .route(wire::CAP_FS_MKDIR, post(capability::fs_mkdir))
        .route(wire::CAP_FS_REMOVE, post(capability::fs_remove))
        .route(wire::CAP_FS_RENAME, post(capability::fs_rename))
        .route(wire::CAP_DOCKER_CALL, post(capability::docker_call))
        .route(wire::CAP_INTEROP_CALL, post(interop::interop_call))
        .route(wire::CAP_INTEROP_CATALOG, post(interop::interop_catalog))
        .route(wire::CAP_SCHEDULER_CALL, post(capability::scheduler_call))
        .route(wire::CAP_NET_APPLY, post(capability::net_unimplemented))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::plugin_auth,
        ))
        .layer(axum::middleware::from_fn(auth::protocol_guard))
        .with_state(state)
}
