//! 壳页路由表与页面取数地址：`GET /ui/v1/routes`、`GET /ui/v1/page`。
//!
//! 为什么必须在服务端：路由是菜单的投影，菜单树本来就由内核算
//! （`skiff_app::menu::build`）。壳脚本再抄一份「菜单键 → 路径 → 页面来源」的映射，
//! 等于把同一份事实写两遍——改一个菜单项要同时改内核与脚本，而脚本那份无法在 CI
//! 里验证，只能靠人工点。这里把它收回两件事：
//!
//! - `/routes`：一次下发完整的 budding `routes` 数组（菜单树 + 详情/子视图隐藏路由）；
//! - `/page`：前端只报「当前相对路径 + 查询串」，服务端回答「页面结构去哪取」。
//!
//! 前端因此不再持有任何「哪个页面取哪个端点」的知识，只按地址取 JSON。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::Query;
use axum::routing::get;
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use super::*;

/// `/ui/v1` 在站点根下的固定挂载点（与壳页 `data-api-base` 同一事实）。
const API: &str = "/ui/v1";

/// 通用页标记：页面结构由 `/ui/v1/page` 按路径分派后取回。
const SKIFF_PAGE: &str = "skiff:page";

// ---- 路由表 ------------------------------------------------------------------

/// 语义图标名 → mdi 字形。
///
/// 图标名来自内核菜单（`mdi` 语义名），字形是壳页的展示细节；未登记的语义名
/// 直接省略 `icon` 字段，避免导航渲染出空白占位。
const MENU_ICONS: &[(&str, &str)] = &[
    ("overview", "mdi-view-dashboard"),
    ("globe", "mdi-web"),
    ("docker", "mdi-docker"),
    ("plug", "mdi-puzzle"),
    ("tools", "mdi-wrench"),
    ("settings", "mdi-cog"),
    ("container", "mdi-cube-outline"),
    ("shield", "mdi-shield-outline"),
    ("lock", "mdi-lock-outline"),
    ("network", "mdi-lan"),
    ("user", "mdi-account-outline"),
    ("activity", "mdi-pulse"),
    ("list", "mdi-format-list-bulleted"),
    ("archive", "mdi-archive-outline"),
    ("clock", "mdi-clock-outline"),
    ("file", "mdi-file-document"),
    ("dot", "mdi-circle"),
    ("folder", "mdi-folder-outline"),
    ("disk", "mdi-harddisk"),
    ("image", "mdi-image-outline"),
    ("hard-drive", "mdi-harddisk"),
];

fn icon_for(semantic: &str) -> Option<&'static str> {
    MENU_ICONS
        .iter()
        .find(|(name, _)| *name == semantic)
        .map(|(_, glyph)| *glyph)
}

/// 页面来源：决定路由 `schema` 字段取什么值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageKind {
    /// 通用页：`skiff:page`，前端转 `/ui/v1/page` 由服务端分派。
    Page,
    /// 静态布局资产，相对 `static/budding/pages/`。
    Asset(&'static str),
    /// 前端运行时特例（终端 wsUrl、安装确认内存态、插件随包 schema）。
    Special(&'static str),
}

impl PageKind {
    fn schema(self, prefix: &str) -> String {
        match self {
            Self::Page => SKIFF_PAGE.to_string(),
            Self::Asset(name) => format!("{prefix}/static/budding/pages/{name}"),
            Self::Special(tag) => format!("skiff:{tag}"),
        }
    }
}

/// 已迁移到 budding 的内置菜单键 →（相对入口前缀的路径，页面来源）。
///
/// 空路径 = 入口根（概览）。未登记的键保留内核深链，走 `href` 跳转（插件功能页与
/// 尚未迁移的旧页面）。
const BUILTIN_PAGES: &[(&str, &str, PageKind)] = &[
    ("overview", "", PageKind::Page),
    ("core/sites/list", "/sites/list", PageKind::Page),
    ("core/sites/web", "/sites/web", PageKind::Page),
    ("core/sites/env", "/sites/runtimes", PageKind::Page),
    (
        "core/docker/containers",
        "/docker/containers",
        PageKind::Page,
    ),
    (
        "core/docker/images",
        "/docker/images",
        PageKind::Asset("docker-images.json"),
    ),
    (
        "core/docker/networks",
        "/docker/networks",
        PageKind::Asset("docker-networks.json"),
    ),
    (
        "core/docker/volumes",
        "/docker/volumes",
        PageKind::Asset("docker-volumes.json"),
    ),
    ("core/docker/compose", "/docker/compose", PageKind::Page),
    (
        "core/plugins/installed",
        "/plugins/installed",
        PageKind::Page,
    ),
    ("core/plugins/discover", "/plugins/discover", PageKind::Page),
    ("core/plugins/updates", "/plugins/updates", PageKind::Page),
    ("core/plugins/store", "/plugins/store", PageKind::Page),
    ("core/tasks", "/tasks", PageKind::Page),
    ("core/jobs", "/jobs", PageKind::Page),
    ("core/files", "/files", PageKind::Page),
    ("core/files/trash", "/files/trash", PageKind::Page),
    ("core/files/quotas", "/files/quotas", PageKind::Page),
    (
        "core/settings/security",
        "/settings/security",
        PageKind::Page,
    ),
    ("core/settings/menu", "/settings/menu", PageKind::Page),
    ("core/settings/ui", "/settings/ui", PageKind::Page),
    ("core/docker/settings", "/docker/settings", PageKind::Page),
    ("core/settings/audit", "/settings/audit", PageKind::Page),
];

fn builtin_page(key: &str) -> Option<(&'static str, PageKind)> {
    BUILTIN_PAGES
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, path, kind)| (*path, *kind))
}

/// 页内互斥子视图（L3）：只有隐藏路由需要它，默认视图就是 L2 自身。
struct Subview {
    /// 路径模式，`:name` 为参数段（与 budding 路由的模式语法一致）。
    pattern: &'static [&'static str],
    /// 侧栏高亮落点（L2 路径）。
    menu: &'static str,
    title: &'static str,
    /// （标题，附加路径段）；第一个是默认视图，不单独登记路由。
    tabs: &'static [(&'static str, &'static [&'static str])],
}

const SUBVIEWS: &[Subview] = &[
    Subview {
        pattern: &["sites", "web"],
        menu: "/sites/web",
        title: "Web 服务",
        tabs: &[
            ("状态", &[]),
            ("配置", &["config"]),
            ("日志", &["logs"]),
            ("模块", &["modules"]),
        ],
    },
    Subview {
        pattern: &["sites", ":id"],
        menu: "/sites/list",
        title: "站点详情",
        tabs: &[
            ("概览", &[]),
            ("域名", &["domains"]),
            ("配置", &["config"]),
            ("证书", &["ssl"]),
            ("备份", &["backup"]),
            ("日志", &["logs"]),
        ],
    },
    Subview {
        pattern: &["sites", "runtimes", ":id"],
        menu: "/sites/runtimes",
        title: "运行环境详情",
        tabs: &[
            ("基础信息", &[]),
            ("配置", &["config"]),
            ("扩展", &["extensions"]),
            ("站点", &["sites"]),
            ("管理", &["manage"]),
            ("日志", &["logs"]),
        ],
    },
];

/// 菜单不出现、但地址栏直达与列表行内跳转都要命中的详情路由。
///
/// 列口径：（路径模式，标题，页面来源，侧栏高亮落点）。
const DETAIL_ROUTES: &[(&str, &str, PageKind, &str)] = &[
    ("/sites/new", "新建站点", PageKind::Page, "/sites/list"),
    ("/sites/:id", "站点详情", PageKind::Page, "/sites/list"),
    (
        "/sites/runtimes/:id",
        "运行环境详情",
        PageKind::Page,
        "/sites/runtimes",
    ),
    (
        "/docker/containers/new",
        "新建容器",
        PageKind::Asset("docker-container-new.json"),
        "/docker/containers",
    ),
    (
        "/docker/containers/:id",
        "容器详情",
        PageKind::Page,
        "/docker/containers",
    ),
    ("/tasks/:id", "任务详情", PageKind::Page, "/tasks"),
    ("/jobs/:id", "作业详情", PageKind::Page, "/jobs"),
    (
        "/docker/containers/:id/terminal",
        "容器终端",
        PageKind::Special("terminal"),
        "/docker/containers",
    ),
    (
        "/docker/containers/:id/logs",
        "容器日志",
        PageKind::Page,
        "/docker/containers",
    ),
    ("/jobs/:id/logs", "作业日志", PageKind::Page, "/jobs"),
    ("/files/view", "代码查看器", PageKind::Page, "/files"),
    ("/files/trash", "回收站", PageKind::Page, "/files"),
    ("/files/quotas", "配额管理", PageKind::Page, "/files"),
    (
        "/settings/menu",
        "菜单设置",
        PageKind::Page,
        "/settings/security",
    ),
    (
        "/docker/settings",
        "Docker 设置",
        PageKind::Page,
        "/docker/settings",
    ),
    (
        "/settings/ui",
        "界面设置",
        PageKind::Page,
        "/settings/security",
    ),
    (
        "/plugins/:id",
        "插件详情",
        PageKind::Page,
        "/plugins/installed",
    ),
    (
        "/plugins/store/confirm",
        "安装确认",
        PageKind::Special("store-confirm"),
        "/plugins/store",
    ),
];

/// 一条 budding 路由。可选字段不写就不下发（`hidden=false` 也不写）。
#[derive(Debug, Clone, Serialize)]
pub(super) struct RouteData {
    pub path: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<RouteData>>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl RouteData {
    fn new(path: String, title: impl Into<String>) -> Self {
        Self {
            path,
            title: title.into(),
            schema: None,
            icon: None,
            hidden: false,
            href: None,
            target: None,
            meta: None,
            children: None,
        }
    }

    fn schema(mut self, schema: String) -> Self {
        self.schema = Some(schema);
        self
    }

    /// 未登记的语义图标不落字段（`None` 会被 `skip_serializing_if` 吃掉）。
    fn icon(mut self, semantic: &str) -> Self {
        self.icon = icon_for(semantic).map(str::to_string);
        self
    }

    fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    fn meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(meta);
        self
    }

    fn children(mut self, children: Vec<RouteData>) -> Self {
        self.children = Some(children);
        self
    }

    /// 内核深链：未迁移的菜单项直接跳旧页面（同标签页）。
    fn link(mut self, href: &str) -> Self {
        self.href = Some(href.to_string());
        self.target = Some("_self".to_string());
        self
    }
}

/// 插件菜单键的视图元数据（与 `/ui/v1/menu` 同源，逐项由 manifest 解析）。
struct PluginEntry {
    plugin_id: String,
    default_view: String,
    views: Vec<MenuViewData>,
}

fn plugin_view_path(prefix: &str, plugin_id: &str, view: &str) -> String {
    format!(
        "{prefix}/plugins/{}/{}",
        crate::util::encode_uri_component(plugin_id),
        crate::util::encode_uri_component(view)
    )
}

/// 一个菜单项 → 一条路由。插件功能页走 `/plugins/<id>/<view>`；未迁移页面给 `href`。
fn menu_route(
    prefix: &str,
    key: &str,
    label: &str,
    icon: &str,
    href: &str,
    plugins: &HashMap<String, PluginEntry>,
) -> Option<RouteData> {
    if key.starts_with("plugin/") {
        // 拿不到视图元数据就整项不出：路径拼不完整，也不能退回 `href`（那是旧 iframe 入口）。
        let meta = plugins.get(key)?;
        let path = plugin_view_path(prefix, &meta.plugin_id, &meta.default_view);
        // 侧栏高亮必须落在「这个插件自己那一项」上：`/plugins` 是所有插件页的公共前缀，
        // 指到那里会让「插件 → 已安装」跟着一起高亮。
        return Some(
            RouteData::new(path.clone(), label)
                .schema(PageKind::Special("plugin-view").schema(prefix))
                .icon(icon)
                .meta(json!({ "activeMenu": path })),
        );
    }
    if let Some((path, kind)) = builtin_page(key) {
        let full = if path.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}{path}")
        };
        return Some(
            RouteData::new(full, label)
                .schema(kind.schema(prefix))
                .icon(icon),
        );
    }
    if href.is_empty() {
        return None;
    }
    Some(
        RouteData::new(
            format!(
                "{prefix}/__menu-item/{}",
                crate::util::encode_uri_component(key)
            ),
            label,
        )
        .icon(icon)
        .link(href),
    )
}

/// 菜单树 → 路由表。`plugins` 为插件菜单键的视图元数据（由 handler 预先解析）。
fn build_routes(
    prefix: &str,
    menu: &skiff_app::menu::PageMenu,
    plugins: &HashMap<String, PluginEntry>,
) -> Vec<RouteData> {
    let mut routes = Vec::new();
    for (index, group) in menu.groups.iter().enumerate() {
        let order = index * 10;
        if group.items.is_empty() {
            // 一级直达（概览）：分组自身就是地址。
            if let Some(leaf) = menu_route(
                prefix,
                &group.key,
                &group.label,
                &group.icon,
                &group.href,
                plugins,
            ) {
                routes.push(leaf.meta(json!({ "order": order })));
            }
            continue;
        }
        let mut children: Vec<RouteData> = group
            .items
            .iter()
            .filter_map(|item| {
                menu_route(
                    prefix,
                    &item.key,
                    &item.label,
                    &item.icon,
                    &item.href,
                    plugins,
                )
            })
            .collect();
        // 工具空态：保留内核空态文案，指向已原生渲染的插件中心。
        if children.is_empty() {
            if let Some(empty) = &group.empty {
                children.push(
                    RouteData::new(
                        format!(
                            "{prefix}/__menu-empty/{}",
                            crate::util::encode_uri_component(&group.key)
                        ),
                        empty.text.clone(),
                    )
                    .schema(SKIFF_PAGE.to_string()),
                );
            }
        }
        if children.is_empty() {
            continue;
        }
        // 分组本身不是地址：用入口前缀下的保留段承载菜单层级，子项是绝对路径。
        routes.push(
            RouteData::new(
                format!(
                    "{prefix}/__menu/{}",
                    crate::util::encode_uri_component(&group.key)
                ),
                group.label.clone(),
            )
            .icon(&group.icon)
            .meta(json!({ "order": order }))
            .children(children),
        );
    }
    // 三级子视图：默认视图就是 L2 自身，其余各登记一条隐藏路由（不进侧栏）。
    // 参数段（`:id`）原样留在模式里，由 budding 路由按实际地址匹配。
    for sub in SUBVIEWS {
        let base = sub.pattern.join("/");
        let active_menu = format!("{prefix}{}", sub.menu);
        for (title, segs) in sub.tabs.iter().skip(1) {
            let mut path = format!("{prefix}/{base}");
            if !segs.is_empty() {
                path.push('/');
                path.push_str(&segs.join("/"));
            }
            routes.push(
                RouteData::new(path, format!("{} · {title}", sub.title))
                    .hidden()
                    .schema(SKIFF_PAGE.to_string())
                    .meta(json!({ "activeMenu": active_menu })),
            );
        }
    }
    for (path, title, kind, menu) in DETAIL_ROUTES {
        routes.push(
            RouteData::new(format!("{prefix}{path}"), *title)
                .hidden()
                .schema(kind.schema(prefix))
                .meta(json!({ "activeMenu": format!("{prefix}{menu}") })),
        );
    }
    // 插件功能页：每个非默认视图一条隐藏路由；页面结构由随包 schema 提供，
    // 壳页只做数据绑定与 RPC 转发。
    for entry in plugins.values() {
        let active = plugin_view_path(prefix, &entry.plugin_id, &entry.default_view);
        for view in &entry.views {
            if view.view == entry.default_view {
                continue;
            }
            routes.push(
                RouteData::new(
                    plugin_view_path(prefix, &entry.plugin_id, &view.view),
                    format!("{} · {}", entry.plugin_id, view.label),
                )
                .hidden()
                .schema(PageKind::Special("plugin-view").schema(prefix))
                .meta(json!({ "activeMenu": active.clone() })),
            );
        }
    }
    routes
}

/// 插件菜单键的视图元数据；失败时返回 `None`（该菜单项整项不出路由）。
fn plugin_entry(state: &AppState, key: &str) -> Option<PluginEntry> {
    let (plugin_id, default_view, views) = plugin_menu_views(state, key);
    if plugin_id.is_empty() || default_view.is_empty() {
        return None;
    }
    Some(PluginEntry {
        plugin_id,
        default_view,
        views,
    })
}

/// `GET /ui/v1/routes`：壳页路由表（需登录）。
///
/// 菜单可见性、顺序、角色门与用户隐藏偏好全部来自 `skiff_app::menu::build`，
/// 与 SSR 导航同源；这里只做「菜单 → budding 路由」的形状转换。
pub async fn routes(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Json<serde_json::Value> {
    let prefix = state.cfg.entry_prefix.clone();
    let menu = skiff_app::menu::build(&state, &user, "");
    let mut plugins: HashMap<String, PluginEntry> = HashMap::new();
    for group in &menu.groups {
        for item in &group.items {
            if item.plugin {
                if let Some(entry) = plugin_entry(&state, &item.key) {
                    plugins.insert(item.key.clone(), entry);
                }
            }
        }
    }
    Json(json!({ "data": { "routes": build_routes(&prefix, &menu, &plugins) } }))
}

// ---- 页面取数地址 ------------------------------------------------------------

/// `/ui/v1/page` 的查询：当前**相对入口前缀**的路径与原始查询串。
///
/// 两者分开传而不是让服务端从 `Referer` 猜：壳页在客户端路由里跳转时浏览器地址栏
/// 才是唯一事实，服务端只该被动接受。
#[derive(Debug, Default, Deserialize)]
pub(super) struct PageQuery {
    #[serde(default)]
    pub path: Option<String>,
    /// 原始查询串（不带 `?`），按页面需要选择性透传。
    #[serde(default)]
    pub search: Option<String>,
}

/// 页面结构从哪来。`schema_url` 与 `special` 互斥。
#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub(super) struct PageSource {
    /// 页面 JSON 的取数地址（`/ui/v1/...` 端点、静态资产或入口前缀下的旧 JSON 端点）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_url: Option<String>,
    /// 前端运行时特例：`terminal` / `store-confirm` / `plugin-view` / `missing-path`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub special: Option<&'static str>,
}

impl PageSource {
    fn url(url: String) -> Self {
        Self {
            schema_url: Some(url),
            special: None,
        }
    }

    fn special(tag: &'static str) -> Self {
        Self {
            schema_url: None,
            special: Some(tag),
        }
    }
}

/// 查询串的键值表（值已 percent-decode）。
struct Search(HashMap<String, String>);

impl Search {
    fn parse(raw: &str) -> Self {
        let mut map = HashMap::new();
        for pair in raw.trim_start_matches('?').split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            map.insert(decode_component(key), decode_component(value));
        }
        Self(map)
    }

    fn get(&self, key: &str) -> &str {
        self.0.get(key).map(String::as_str).unwrap_or("")
    }

    /// 取非空值，空则用默认值。
    fn or(&self, key: &str, fallback: &str) -> String {
        let value = self.get(key);
        if value.is_empty() {
            fallback.to_string()
        } else {
            value.to_string()
        }
    }
}

/// `application/x-www-form-urlencoded` 值解码：`+` 为空格，`%XX` 为字节。
fn decode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn enc(value: &str) -> String {
    crate::util::encode_uri_component(value)
}

/// `name=value` 片段，`value` 为空时整段省略（调用方负责拼 `?` / `&`）。
fn kv(name: &str, value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        format!("{name}={}", enc(value))
    }
}

/// 把若干片段拼成 `?a=1&b=2`，空片段自动跳过。
fn query(parts: &[String]) -> String {
    let mut out = String::new();
    for part in parts.iter().filter(|p| !p.is_empty()) {
        out.push(if out.is_empty() { '?' } else { '&' });
        out.push_str(part);
    }
    out
}

fn api(path: &str) -> String {
    format!("{API}{path}")
}

/// 站点详情：`tab` 缺省即概览，未知值原样透传（端点自身按概览兜底）。
fn site_detail(id: &str, tab: &str) -> PageSource {
    let tab = tab.to_ascii_lowercase();
    PageSource::url(format!(
        "{}{}",
        api(&format!("/sites/{}", enc(id))),
        query(&[kv("tab", &tab)])
    ))
}

/// 站点访问日志：`part` 只认 `access` / `error`，`tail` 缺省 200。
fn site_logs(id: &str, s: &Search) -> PageSource {
    let part = if s.get("part") == "error" {
        "error"
    } else {
        "access"
    };
    PageSource::url(format!(
        "{}{}",
        api(&format!("/sites/{}/logs", enc(id))),
        query(&[
            format!("part={part}"),
            format!("tail={}", enc(&s.or("tail", "200"))),
            kv("q", s.get("q")),
            "with_schema=true".to_string(),
        ])
    ))
}

/// 运行环境详情：`tab` 缺省 `overview`，`?file=` 只在配置视图里有意义。
fn runtime_detail(id: &str, tab: &str, s: &Search) -> PageSource {
    let tab = tab.to_ascii_lowercase();
    let tab = if tab.is_empty() {
        "overview".to_string()
    } else {
        tab
    };
    PageSource::url(format!(
        "{}{}",
        api(&format!("/runtimes/{}", enc(id))),
        query(&[
            format!("tab={}", enc(&tab)),
            "with_schema=true".to_string(),
            kv("file", s.get("file")),
        ])
    ))
}

/// Web 服务页：视图来自第三段（白名单），日志视图才带筛选项。
fn web(view: &str, s: &Search) -> PageSource {
    let view = match view.to_ascii_lowercase().as_str() {
        "config" => "config",
        "logs" => "logs",
        "modules" => "modules",
        _ => "status",
    };
    let mut parts = vec![format!("view={view}"), "with_schema=true".to_string()];
    if view == "logs" {
        parts.push("load=1".to_string());
        parts.push(format!("kind={}", enc(&s.or("type", "access"))));
        parts.push(format!("lines={}", enc(&s.or("lines", "200"))));
        parts.push(format!("level={}", enc(&s.or("level", "all"))));
        parts.push(kv("status", s.get("status")));
        parts.push(kv("q", s.get("q")));
    }
    PageSource::url(format!("{}{}", api("/web"), query(&parts)))
}

/// 当前地址 → 页面取数地址。
///
/// 匹配顺序即优先级：更具体的路径必须排在通配段之前（`/sites/web` 先于 `/sites/:id`，
/// `/plugins/store/confirm` 先于 `/plugins/:id/:view`）。
fn page_source(prefix: &str, raw_path: &str, s: &Search) -> PageSource {
    let trimmed = raw_path.trim_matches('/');
    let segs: Vec<&str> = if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').collect()
    };
    match segs.as_slice() {
        [] => PageSource::url(api("/overview?with_schema=true")),

        // 站点
        ["sites", "new"] => PageSource::url(format!(
            "{}{}",
            api("/pages/sites-new"),
            query(&[kv("type", s.get("type"))])
        )),
        ["sites", "list"] => PageSource::url(api("/sites?limit=1&with_schema=true")),
        ["sites", "web"] => web("status", s),
        ["sites", "web", view] => web(view, s),
        ["sites", "runtimes"] => PageSource::url(api("/runtimes?limit=1&with_schema=true")),
        ["sites", "runtimes", id] => runtime_detail(id, "overview", s),
        ["sites", "runtimes", id, "logs"] => PageSource::url(api(&format!(
            "/runtimes/{}/logs?tail=300&with_schema=true",
            enc(id)
        ))),
        ["sites", "runtimes", id, tab] => runtime_detail(id, tab, s),
        ["sites", id, "logs"] => site_logs(id, s),
        ["sites", id] => site_detail(id, ""),
        ["sites", id, tab] => site_detail(id, tab),

        // 容器与资源
        ["docker", "containers"] => {
            let filter = if s.get("filter") == "error" {
                "filter=error&"
            } else {
                ""
            };
            PageSource::url(api(&format!(
                "/containers?{filter}limit=1&with_schema=true"
            )))
        }
        ["docker", "containers", "new"] => PageSource::url(format!(
            "{prefix}/static/budding/pages/docker-container-new.json"
        )),
        // 终端的 ws 地址必须由浏览器按当前 origin 拼，服务端给不了。
        ["docker", "containers", _, "terminal"] => PageSource::special("terminal"),
        ["docker", "containers", id, "logs"] => PageSource::url(api(&format!(
            "/containers/{}/logs?tail=500&with_schema=true",
            enc(id)
        ))),
        ["docker", "containers", id] => {
            PageSource::url(api(&format!("/containers/{}?with_schema=true", enc(id))))
        }
        ["docker", "images"] => {
            PageSource::url(format!("{prefix}/static/budding/pages/docker-images.json"))
        }
        ["docker", "networks"] => PageSource::url(format!(
            "{prefix}/static/budding/pages/docker-networks.json"
        )),
        ["docker", "volumes"] => {
            PageSource::url(format!("{prefix}/static/budding/pages/docker-volumes.json"))
        }
        ["docker", "compose"] => PageSource::url(api("/compose?with_schema=true")),
        ["docker", "settings"] => PageSource::url(api("/settings/docker?with_schema=true")),

        // 任务与作业
        ["tasks"] => PageSource::url(format!(
            "{}{}",
            api("/tasks?limit=1&with_schema=true"),
            if s.get("q").is_empty() {
                String::new()
            } else {
                format!("&q={}", enc(s.get("q")))
            }
        )),
        ["tasks", id] => PageSource::url(api(&format!("/tasks/{}?with_schema=true", enc(id)))),
        ["jobs"] => PageSource::url(api("/jobs?limit=1&with_schema=true")),
        ["jobs", id] => PageSource::url(api(&format!("/jobs/{}?with_schema=true", enc(id)))),
        ["jobs", id, "logs"] => {
            PageSource::url(api(&format!("/jobs/{}/logs?with_schema=true", enc(id))))
        }

        // 文件
        ["files"] => PageSource::url(format!(
            "{}{}",
            api("/files"),
            query(&[kv("path", s.get("path")), "with_schema=true".to_string()])
        )),
        ["files", "trash"] => {
            PageSource::url(format!("{prefix}/files/trash/list?with_schema=true"))
        }
        ["files", "quotas"] => {
            PageSource::url(format!("{prefix}/files/quotas/list?with_schema=true"))
        }
        ["files", "view"] => {
            if s.get("path").is_empty() {
                PageSource::special("missing-path")
            } else {
                PageSource::url(api(&format!(
                    "/files/read?path={}&with_schema=true",
                    enc(s.get("path"))
                )))
            }
        }

        // 插件与商店
        ["plugins", "installed"] => PageSource::url(api("/plugins?tab=installed&with_schema=true")),
        ["plugins", tab @ ("discover" | "updates")] => {
            PageSource::url(api(&format!("/plugins?tab={tab}&with_schema=true")))
        }
        ["plugins", "store"] => PageSource::url(format!(
            "{}{}",
            api("/store"),
            query(&[
                if s.get("refresh") == "1" {
                    "refresh=1".to_string()
                } else {
                    String::new()
                },
                "with_schema=true".to_string(),
            ])
        )),
        ["plugins", "store", "confirm"] => PageSource::special("store-confirm"),
        ["plugins", id] => PageSource::url(api(&format!("/plugins/{}?with_schema=true", enc(id)))),
        // 插件功能页的结构来自随包 manifest，壳页负责 RPC 绑定，不给端点地址。
        ["plugins", _, _] => PageSource::special("plugin-view"),
        // 设置
        ["settings", "security"] => PageSource::url(api("/settings?with_schema=true")),
        ["settings", "menu"] => PageSource::url(api("/settings/menu?with_schema=true")),
        ["settings", "ui"] => PageSource::url(api("/settings/ui?with_schema=true")),
        ["settings", "audit"] => PageSource::url(format!(
            "{}{}",
            api("/audit"),
            query(&[
                "limit=50".to_string(),
                "with_schema=true".to_string(),
                kv("q", s.get("q")),
                kv("cursor", s.get("cursor")),
            ])
        )),

        // 工具空态：内核给出的空态文案落到插件中心。
        ["__menu-empty", _] => PageSource::url(api("/plugins?tab=installed&with_schema=true")),

        _ => PageSource::default(),
    }
}

/// `GET /ui/v1/page?path=&search=`：当前地址对应的页面取数地址（需登录）。
///
/// 空响应（既无 `schema_url` 也无 `special`）表示地址没有对应页面，由壳页给统一提示。
pub async fn page(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Query(q): Query<PageQuery>,
) -> Json<serde_json::Value> {
    let path = q.path.unwrap_or_default();
    let search = Search::parse(q.search.as_deref().unwrap_or(""));
    let prefix = state.cfg.entry_prefix.clone();
    Json(json!({ "data": page_source(&prefix, &path, &search) }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/routes", get(routes))
        .route("/ui/v1/page", get(page))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, search: &str) -> PageSource {
        page_source("/p", path, &Search::parse(search))
    }

    fn url(path: &str, search: &str) -> String {
        src(path, search).schema_url.expect("应给出取数地址")
    }

    #[test]
    fn list_pages_point_at_their_ui_endpoint() {
        assert_eq!(url("/", ""), "/ui/v1/overview?with_schema=true");
        assert_eq!(
            url("/sites/list", ""),
            "/ui/v1/sites?limit=1&with_schema=true"
        );
        assert_eq!(
            url("/docker/containers", ""),
            "/ui/v1/containers?limit=1&with_schema=true"
        );
        assert_eq!(
            url("/docker/containers", "filter=error"),
            "/ui/v1/containers?filter=error&limit=1&with_schema=true"
        );
    }

    #[test]
    fn detail_pages_carry_path_params_into_the_endpoint() {
        assert_eq!(url("/sites/abc.example", ""), "/ui/v1/sites/abc.example");
        assert_eq!(
            url("/sites/abc.example/config", ""),
            "/ui/v1/sites/abc.example?tab=config"
        );
        assert_eq!(
            url("/sites/runtimes/rt1/manage", ""),
            "/ui/v1/runtimes/rt1?tab=manage&with_schema=true"
        );
        // 站点日志是独立端点，不能退化成 `?tab=logs`。
        assert_eq!(
            url("/sites/abc.example/logs", "part=error&tail=500&q=502"),
            "/ui/v1/sites/abc.example/logs?part=error&tail=500&q=502&with_schema=true"
        );
        assert_eq!(
            url("/sites/abc.example/logs", ""),
            "/ui/v1/sites/abc.example/logs?part=access&tail=200&with_schema=true"
        );
    }

    #[test]
    fn query_values_are_decoded_then_reencoded() {
        // `Search` 拿到的是 axum 解码后的原始串；回填 URL 必须重新编码。
        assert_eq!(
            url("/files", "path=%2Fvar%2Fwww%2Fa+b"),
            "/ui/v1/files?path=%2Fvar%2Fwww%2Fa%20b&with_schema=true"
        );
        assert_eq!(url("/files", ""), "/ui/v1/files?with_schema=true");
        assert_eq!(
            url("/tasks", "q=a%26b"),
            "/ui/v1/tasks?limit=1&with_schema=true&q=a%26b"
        );
    }

    #[test]
    fn web_page_whitelists_views_and_only_logs_carry_filters() {
        assert_eq!(
            url("/sites/web", ""),
            "/ui/v1/web?view=status&with_schema=true"
        );
        assert_eq!(
            url("/sites/web/modules", ""),
            "/ui/v1/web?view=modules&with_schema=true"
        );
        assert_eq!(
            url("/sites/web/unknown", ""),
            "/ui/v1/web?view=status&with_schema=true"
        );
        assert_eq!(
            url("/sites/web/logs", "type=error&lines=500&level=warn&status=500&q=oops"),
            "/ui/v1/web?view=logs&with_schema=true&load=1&kind=error&lines=500&level=warn&status=500&q=oops"
        );
    }

    #[test]
    fn shell_specials_stay_in_the_shell() {
        // 这三类结构离不开浏览器或内存态，不能由服务端给 URL。
        assert_eq!(
            src("/docker/containers/c1/terminal", "").special,
            Some("terminal")
        );
        assert_eq!(
            src("/plugins/store/confirm", "").special,
            Some("store-confirm")
        );
        assert_eq!(
            src("/plugins/com.skiff.files/main", "").special,
            Some("plugin-view")
        );
        assert_eq!(
            src("/files/view", "").special,
            Some("missing-path"),
            "缺 path 要给友好的缺参提示，而不是让端点报 400"
        );
    }

    #[test]
    fn legacy_json_endpoints_keep_the_entry_prefix() {
        // 这两个端点不在 `/ui/v1` 下，前缀不能丢。
        assert_eq!(
            url("/files/trash", ""),
            "/p/files/trash/list?with_schema=true"
        );
        assert_eq!(
            url("/files/quotas", ""),
            "/p/files/quotas/list?with_schema=true"
        );
        assert_eq!(
            url("/docker/images", ""),
            "/p/static/budding/pages/docker-images.json"
        );
    }

    #[test]
    fn unknown_path_has_no_source() {
        let empty = src("/nope/deep/path", "");
        assert!(empty.schema_url.is_none() && empty.special.is_none());
    }

    #[test]
    fn decode_component_handles_percent_and_plus() {
        assert_eq!(decode_component("a+b"), "a b");
        assert_eq!(decode_component("%2Fvar%2Fwww"), "/var/www");
        assert_eq!(decode_component("100%"), "100%", "残缺转义按字面保留");
        assert_eq!(decode_component("%zz"), "%zz");
    }

    fn menu_with(groups: Vec<skiff_app::menu::MenuGroup>) -> skiff_app::menu::PageMenu {
        skiff_app::menu::PageMenu {
            groups,
            pinned: Vec::new(),
        }
    }

    fn group(key: &str, items: Vec<skiff_app::menu::MenuLink>) -> skiff_app::menu::MenuGroup {
        skiff_app::menu::MenuGroup {
            key: key.to_string(),
            label: key.to_string(),
            icon: "globe".to_string(),
            href: String::new(),
            active: false,
            items,
            empty: None,
        }
    }

    fn item(key: &str, icon: &str) -> skiff_app::menu::MenuLink {
        skiff_app::menu::MenuLink {
            key: key.to_string(),
            label: key.to_string(),
            href: format!("/p/{key}"),
            active: false,
            icon: icon.to_string(),
            plugin: false,
            pinnable: true,
            pinned: false,
        }
    }

    #[test]
    fn routes_project_menu_tree_with_prefix_and_icons() {
        let menu = menu_with(vec![group("sites", vec![item("core/files", "file")])]);
        let routes = build_routes("/p", &menu, &HashMap::new());
        // 菜单路由在前（隐藏的详情/子视图路由随后追加）。
        let group_route = routes.first().expect("菜单至少投影出一条路由");
        assert_eq!(group_route.path, "/p/__menu/sites");
        assert_eq!(group_route.icon.as_deref(), Some("mdi-web"));
        assert!(!group_route.hidden, "菜单分组自身不该是隐藏路由");
        let children = group_route.children.as_ref().expect("分组必须有子项");
        assert_eq!(children[0].path, "/p/files");
        assert_eq!(children[0].schema.as_deref(), Some(SKIFF_PAGE));
        assert_eq!(children[0].icon.as_deref(), Some("mdi-file-document"));
    }

    #[test]
    fn unmigrated_menu_item_keeps_kernel_deeplink() {
        let menu = menu_with(vec![group("tools", vec![item("core/legacy", "tools")])]);
        let routes = build_routes("/p", &menu, &HashMap::new());
        let child = &routes[0].children.as_ref().unwrap()[0];
        assert_eq!(child.path, "/p/__menu-item/core%2Flegacy");
        assert_eq!(child.href.as_deref(), Some("/p/core/legacy"));
        assert_eq!(child.target.as_deref(), Some("_self"));
    }

    #[test]
    fn hidden_detail_routes_are_registered_outside_the_menu() {
        let menu = menu_with(Vec::new());
        let routes = build_routes("/p", &menu, &HashMap::new());
        let paths: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"/p/sites/:id"), "{paths:?}");
        assert!(
            paths.contains(&"/p/docker/containers/:id/terminal"),
            "{paths:?}"
        );
        // 子视图：默认视图不登记，其余每个 tab 一条。
        assert!(paths.contains(&"/p/sites/:id/config"), "{paths:?}");
        assert!(!paths.contains(&"/p/sites/:id/"), "{paths:?}");
        assert_eq!(
            routes
                .iter()
                .find(|r| r.path == "/p/sites/:id/logs")
                .and_then(|r| r.title.as_str().split(" · ").nth(1)),
            Some("日志")
        );
    }
}
