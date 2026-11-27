//! 菜单渲染模型：一级分组、链接与空态（壳页侧栏投影）。

#[allow(unused_imports)]
use super::super::*;

/// `GET /ui/v1/menu` 的响应数据：内核菜单树的 JSON 投影（契约 §1.1 的三级归属不变）。
///
/// 只做 wire 形状转换，不另造菜单语义：L1/L2 的可见性、顺序、角色与能力门、
/// 用户隐藏偏好全部由 `skiff_app::menu::build` 决定，与 SSR 导航同源。
#[derive(Debug, Serialize)]
pub struct MenuData {
    pub groups: Vec<MenuGroupData>,
}

#[derive(Debug, Serialize)]
pub struct MenuGroupData {
    /// 稳定键：固定 L1 id（`overview` / `sites` / ...）。
    pub key: String,
    pub label: String,
    pub icon: String,
    /// 非空 = 一级直达链接（概览）；纯分组为空串。
    pub href: String,
    pub items: Vec<MenuLinkData>,
    /// 插件支撑型一级的空态（`工具` 无可见提供者时）。
    pub empty: Option<MenuEmptyData>,
}

#[derive(Debug, Serialize)]
pub struct MenuLinkData {
    /// 稳定键：`core/...` 或 `plugin/<id>/<l2>`。
    pub key: String,
    pub label: String,
    pub icon: String,
    /// 规范深链：内置项为 `/{p}/...`；插件功能页为原生 `/{p}/plugins/{id}/{view}`。
    pub href: String,
    pub plugin: bool,
    /// 插件 id（内置页面为空串）。
    pub plugin_id: String,
    /// 插件默认视图名（内置页面为空串）。
    pub default_view: String,
    /// 插件视图列表：壳页据此生成路由与页内标签。
    pub views: Vec<MenuViewData>,
}

#[derive(Debug, Serialize)]
pub struct MenuViewData {
    pub view: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct MenuEmptyData {
    pub text: String,
    pub href: String,
    pub action: String,
}
