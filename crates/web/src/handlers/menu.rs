//! 菜单深链 `/{p}/m/...`。
//!
//! `overview` 概览 SSR 暂保留（直到插件槽位迁完）。
//! `plugin/*` 插件工作台仍由内核渲染（功能页与 schema 页）。
//! `core/*` 旧 SSR 深链已删除，返回 404。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{AuthUser, PluginManagerUser};
use crate::state::AppState;

/// 菜单深链 dispatch。
pub async fn dispatch(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(rest): Path<String>,
    Query(incoming): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let rest = rest.trim_matches('/');
    match rest {
        "overview" => handlers::dashboard::index(State(state), AuthUser(user)).await,
        other if other.starts_with("plugin/") => {
            // 功能页与 RPC 同权限：只有插件管理角色能进工作台 / schema 页。
            // 权限不足必须 403（不能靠视图解析失败伪装成 404）。
            if !user.can(skiff_model::Permission::PluginManage) {
                return Err(AppError::Forbidden("需要插件管理权限".into()));
            }
            plugin_workbench(
                State(state),
                PluginManagerUser(user),
                Path(other.to_string()),
                Query(incoming),
            )
            .await
        }
        other if other.starts_with("core/") => Err(AppError::NotFound(
            "旧 SSR 菜单已移除，请直接访问 budding 壳页规范路径".into(),
        )),
        _ => Err(AppError::NotFound("菜单不存在".into())),
    }
}

/// 插件工作台内部转发（功能页 + schema + RPC）。
async fn plugin_workbench(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path(other): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let mut parts = other.split('/');
    let _ = parts.next();
    let Some(id) = parts.next() else {
        return Err(AppError::NotFound("菜单不存在".into()));
    };
    let local = parts.next().unwrap_or("");
    if local.is_empty() {
        // `plugin/<id>` 是插件默认功能页入口（声明菜单取首个可见 L2 的
        // 首个视图；老包落到 `main` → 原 `ui/index.html`）。
        return handlers::plugins::menu_view(
            State(state),
            PluginManagerUser(user),
            Path((id.to_string(), None, None)),
            Query(q),
        )
        .await;
    }
    // L3 深链（契约 §3.3）：内核渲染标签栏，插件只渲染 iframe 内容。
    let l3 = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    if parts.next().is_some() {
        return Err(AppError::NotFound("菜单层级超过三级".into()));
    }
    handlers::plugins::menu_view(
        State(state),
        PluginManagerUser(user),
        Path((id.to_string(), Some(local.to_string()), l3)),
        Query(q),
    )
    .await
}
