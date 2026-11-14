use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/menu`：budding 壳的菜单数据（需登录，401 口径同 `/ui/v1/session`）。
///
/// 菜单数据是路由表的来源：前端据 `key` 映射到已迁移的 budding 页面。
/// 插件项下发的 `href` 直接指向原生 `/{p}/plugins/{id}/{view}`，不再落到
/// `/{p}/m/plugin/...` 旧工作台；旧路径只保留给显式 `?legacy=1` 兼容请求。
pub async fn menu(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Json<serde_json::Value> {
    let prefix = state.cfg.entry_prefix.clone();
    let page = skiff_app::menu::build(&state, &user, "");
    let groups = page
        .groups
        .into_iter()
        .map(|group| MenuGroupData {
            key: group.key,
            label: group.label,
            icon: group.icon,
            href: group.href,
            items: group
                .items
                .into_iter()
                .map(|item| {
                    let (plugin_id, default_view, views) = if item.plugin {
                        plugin_menu_views(&state, &item.key)
                    } else {
                        (String::new(), String::new(), Vec::new())
                    };
                    // 插件菜单在原生命令下是 `/{p}/plugins/{id}/{view}`；
                    // 只有 manifest 解析失败（拿不到视图）时才保留内核旧深链兜底。
                    let href = if !plugin_id.is_empty() && !default_view.is_empty() {
                        format!("/{prefix}/plugins/{plugin_id}/{default_view}")
                    } else {
                        item.href
                    };
                    MenuLinkData {
                        key: item.key,
                        label: item.label,
                        icon: item.icon,
                        href,
                        plugin: item.plugin,
                        plugin_id,
                        default_view,
                        views,
                    }
                })
                .collect(),
            empty: group.empty.map(|empty| MenuEmptyData {
                text: empty.text,
                href: empty.href,
                action: empty.action,
            }),
        })
        .collect();
    Json(json!({ "data": MenuData { groups } }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ui/v1/menu", get(menu))
}
