//! 菜单深链 `/{p}/m/...`：解析到既有页面 handler，旧路由保持可用（契约 §7.2）。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;

pub async fn dispatch(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(rest): Path<String>,
    Query(incoming): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let rest = rest.trim_matches('/');
    let mut q: HashMap<String, String> = incoming;
    // 契约深链：`core/sites/env/<id>` 与 `core/sites/list/<domain>` 带上末级参数。
    if let Some(id) = rest.strip_prefix("core/sites/env/") {
        if !id.is_empty() && !id.contains('/') {
            return handlers::sites::runtime_detail(
                State(state),
                AuthUser(user),
                Path(id.to_string()),
                Query(q),
            )
            .await;
        }
    }
    if let Some(domain) = rest.strip_prefix("core/sites/list/") {
        if !domain.is_empty() && !domain.contains('/') {
            return handlers::sites::detail(
                State(state),
                AuthUser(user),
                Path(domain.to_string()),
                Query(q),
            )
            .await;
        }
    }
    // `core/tasks/<id>`：任务详情深链（id 里含 `:`，不含 `/`）。
    if let Some(id) = rest.strip_prefix("core/tasks/") {
        if !id.is_empty() && !id.contains('/') {
            return handlers::tasks::detail(
                State(state),
                AuthUser(user),
                Path(id.to_string()),
                Query(HashMap::new()),
            )
            .await;
        }
    }
    match rest {
        "overview" => handlers::dashboard::index(State(state), AuthUser(user)).await,
        "core/tasks" => handlers::tasks::list(State(state), AuthUser(user), Query(q)).await,
        "core/files" => handlers::files::page(State(state), AuthUser(user), Query(q)).await,
        "core/files/trash" => handlers::files::trash_page(State(state), AuthUser(user)).await,
        "core/files/quotas" => handlers::files::quotas_page(State(state), AuthUser(user)).await,
        "core/sites/list" => handlers::sites::list(State(state), AuthUser(user), Query(q)).await,
        "core/sites/env" => handlers::sites::env_page(State(state), AuthUser(user), Query(q)).await,
        "core/sites/web" => handlers::nginx::index(State(state), AuthUser(user), Query(q)).await,
        "core/sites/new" => {
            handlers::sites::new_page_get(State(state), AuthUser(user), Query(q)).await
        }
        "core/docker/containers" => {
            handlers::containers::list(State(state), AuthUser(user), Query(q)).await
        }
        "core/docker/compose" => {
            handlers::compose::page(State(state), AuthUser(user), Query(q)).await
        }
        "core/docker/settings" => {
            handlers::docker_settings::page(State(state), AuthUser(user), Query(q)).await
        }
        "core/docker/images" => handlers::resources::images(State(state), AuthUser(user)).await,
        "core/docker/networks" => handlers::resources::networks(State(state), AuthUser(user)).await,
        "core/docker/volumes" => handlers::resources::volumes(State(state), AuthUser(user)).await,
        "core/plugins/installed" | "core/plugins/discover" | "core/plugins/updates" => {
            let tab = rest.rsplit('/').next().unwrap_or("installed");
            q.insert("tab".into(), tab.into());
            handlers::plugins::list(State(state), AuthUser(user), Query(q)).await
        }
        "core/settings/security" => {
            handlers::settings::page(State(state), AuthUser(user), Query(HashMap::new())).await
        }
        "core/settings/menu" => {
            handlers::settings::menu_page(State(state), AuthUser(user), Query(HashMap::new())).await
        }
        "core/settings/audit" => handlers::settings::audit(State(state), AuthUser(user)).await,
        other if other.starts_with("plugin/") => {
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
                    AuthUser(user),
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
                AuthUser(user),
                Path((id.to_string(), Some(local.to_string()), l3)),
                Query(q),
            )
            .await
        }
        _ => Err(AppError::NotFound("菜单不存在".into())),
    }
}
