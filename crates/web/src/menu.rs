//! 三级菜单模型、声明合并与用户偏好（`docs/MENU_CONTRACT.md` P1）。
//!
//! 这里是**唯一**的菜单合成入口：固定六一级 + 内置二级 + 已启用插件声明，
//! 再套上当前用户的顺序/隐藏/固定/布局偏好。模板只消费 `PageMenu`，
//! 不再自己拼导航（旧的 `handlers::nav` 已删除）。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::state::{AppState, CurrentUser};

/// 偏好表内的键；仅 `menu.prefs` 一行，JSON 内容由本模块解释。
pub const PREFS_KEY: &str = "menu.prefs";
/// 全局默认布局（P1 只读，管理员 UI 留 P3）。缺省 `left`（左右 / 侧栏）。
pub const GLOBAL_LAYOUT_KEY: &str = "ui.menu.layout";
/// 内容宽度缺省值：`auto` 铺满可用宽度。
pub const DEFAULT_CONTENT_WIDTH: &str = "auto";
/// 「固定」内容宽度的缺省上限（px）。
pub const DEFAULT_CONTENT_MAX: u32 = 1440;
/// 「固定」内容宽度允许的范围（px）。
pub const CONTENT_MAX_RANGE: std::ops::RangeInclusive<u32> = 960..=4096;

/// 一级菜单稳定键（`root/<id>` 的 `<id>` 部分）。契约 §1.2。
pub const L1_OVERVIEW: &str = "overview";
pub const L1_SITES: &str = "sites";
pub const L1_DOCKER: &str = "docker";
pub const L1_PLUGINS: &str = "plugins";
pub const L1_TOOLS: &str = "tools";
pub const L1_SETTINGS: &str = "settings";

const L1_LOCKED: &[&str] = &[L1_OVERVIEW, L1_SETTINGS];
const L2_LOCKED: &[&str] = &["core/settings/menu"];

/// 旧 `active` 字符串 → 新菜单键（契约 §7.2）。未知值原样返回。
pub fn canonical_active(active: &str) -> String {
    match active {
        "dashboard" => "overview".into(),
        "containers" => "core/docker/containers".into(),
        "images" => "core/docker/images".into(),
        "networks" => "core/docker/networks".into(),
        "volumes" => "core/docker/volumes".into(),
        "plugins" => "core/plugins/installed".into(),
        "settings" => "core/settings/security".into(),
        "audit" => "core/settings/audit".into(),
        "menu-settings" => "core/settings/menu".into(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct MenuLink {
    pub key: String,
    pub label: String,
    pub href: String,
    pub active: bool,
    pub icon: String,
    /// 插件声明的入口（视觉上仍与内核页面区分）。
    pub plugin: bool,
    pub pinnable: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone)]
pub struct MenuEmpty {
    pub text: String,
    pub href: String,
    pub action: String,
}

#[derive(Debug, Clone)]
pub struct MenuGroup {
    pub key: String,
    pub label: String,
    pub icon: String,
    /// 空串 = 纯分组；非空 = 一级直接链接（概览）。
    pub href: String,
    pub active: bool,
    pub items: Vec<MenuLink>,
    pub empty: Option<MenuEmpty>,
}

/// 模板消费的最终菜单。
#[derive(Debug, Clone)]
pub struct PageMenu {
    pub layout: String,
    /// 内容宽度：`auto` 铺满；`fixed` 使用 `max_width`。
    pub width: String,
    /// `width == fixed` 时的最大宽度（px）。
    pub max_width: u32,
    pub groups: Vec<MenuGroup>,
    pub pinned: Vec<MenuLink>,
}

/// 插件工作台里的一个三级标签（契约 §3.1 L3 标签栏）。
#[derive(Debug, Clone)]
pub struct TabItem {
    /// 稳定键：`plugin/<plugin-id>/<l2>/<l3>`。
    pub key: String,
    pub local: String,
    pub label: String,
    pub href: String,
    pub icon: String,
    /// 传给插件 iframe 的视图名（manifest `view`，缺省等于 L3 `id`）。
    pub tab: String,
    /// 该标签使用的包内 `ui/` 相对文件（`ui/<view>.html`）。
    pub ui: String,
}

/// 一次 L3 解析结果：L2 身份 + 全部可见标签 + 当前选中项。
#[derive(Debug, Clone)]
pub struct WorkbenchTabs {
    pub l2_key: String,
    pub l2_label: String,
    pub items: Vec<TabItem>,
    pub active_key: String,
    /// 当前 L3 的局部 id；无 L3 时为空。
    pub active_local: String,
    /// 当前传给插件的 `tab` 值；单视图 L2 为隐式主视图名。
    pub active_tab: String,
    /// 当前应加载的 `ui/` 相对路径。
    pub active_ui: String,
}

/// 固定页槽位（`[[mount]]`）的渲染视图（契约 §5）。
#[derive(Debug, Clone)]
pub struct MountView {
    pub slot: String,
    /// 槽位的中文标题（内核负责，插件不能改）。
    pub slot_label: String,
    pub plugin: String,
    pub title: String,
    pub src: String,
    pub height: String,
    pub refresh: u64,
    pub nonce: String,
    pub rpc_url: String,
    pub csrf: String,
    /// `skiff.open` 白名单：已启用且有 UI 入口的插件 id。
    pub plugins: String,
}

/// 一次 iframe 的 nonce；每次渲染重新生成（P1 插件详情同口径）。
fn make_nonce() -> String {
    skiff_identity::new_token()
        .replace(['-', '_'], "")
        .chars()
        .take(24)
        .collect()
}

fn is_menu_local(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    s.len() <= 32 && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 解析菜单定位串：接受局部 id（`sites`）或完整稳定键（`plugin/<id>/sites`）。
/// 指向别的插件命名空间时返回 `None`，避免跨插件误命中。
pub fn menu_local(plugin_id: &str, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if let Some(rest) = raw.strip_prefix(&format!("plugin/{plugin_id}/")) {
        rest.split('/').next().unwrap_or("")
    } else if raw.contains('/') {
        return None;
    } else {
        raw
    };
    is_menu_local(candidate).then(|| candidate.to_string())
}

fn roles_allow(roles: Option<&[String]>, role: &str) -> bool {
    match roles {
        None => true,
        Some(roles) => roles.iter().any(|r| r == role),
    }
}

fn gates_allow(gates: &[String], docker_ok: bool) -> bool {
    gates.iter().all(|g| match g.as_str() {
        "docker_available" => docker_ok,
        _ => true,
    })
}

/// 视图名 → 包内 `ui/` 相对文件（`docs/PLUGIN_UI_CONTRACT.md`）。
fn view_file(view: &str) -> String {
    format!("{view}.html")
}

fn item_view(it: &skiff_pkgfmt::MenuItem) -> &str {
    it.view.as_deref().unwrap_or(it.id.as_str())
}

/// 老包（只有 `[[ui]]`）的视图映射：
/// 第一个入口 = `main`（沿用原 `ui/index.html` 路径），其余入口按文件名降级为独立视图。
fn legacy_workbench(
    plugin_id: &str,
    prefix: &str,
    m: &skiff_pkgfmt::Manifest,
    requested: Option<&str>,
) -> Option<WorkbenchTabs> {
    let first = m.ui.first()?;
    let key = format!("plugin/{plugin_id}/legacy");
    let label = if first.label.trim().is_empty() {
        m.name.clone()
    } else {
        first.label.clone()
    };
    let mut ordered: Vec<&skiff_pkgfmt::UiEntry> = m.ui.iter().skip(1).collect();
    ordered.sort_by(|a, b| (&a.label, &a.path).cmp(&(&b.label, &b.path)));
    let items: Vec<TabItem> = ordered
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let view = slug(&u.path, i);
            TabItem {
                key: format!("{key}/{view}"),
                local: view.clone(),
                label: if u.label.trim().is_empty() {
                    m.name.clone()
                } else {
                    u.label.clone()
                },
                href: format!("/{prefix}/m/{key}/{view}"),
                icon: icon_or_dot(u.icon.as_deref()),
                tab: view.clone(),
                ui: u.path.clone(),
            }
        })
        .collect();

    let requested = requested.map(str::trim).filter(|s| !s.is_empty());
    let active = match requested {
        None | Some("main") => None,
        Some(raw) => Some(items.iter().position(|t| t.local == raw)?),
    };
    let (active_key, active_local, active_tab, active_ui) = match active {
        None => (
            key.clone(),
            String::new(),
            "main".into(),
            first.path.clone(),
        ),
        Some(i) => {
            let t = &items[i];
            (t.key.clone(), t.local.clone(), t.tab.clone(), t.ui.clone())
        }
    };
    Some(WorkbenchTabs {
        l2_key: key,
        l2_label: label,
        items,
        active_key,
        active_local,
        active_tab,
        active_ui,
    })
}

fn declared_tabs(
    plugin_id: &str,
    prefix: &str,
    decl: &skiff_pkgfmt::ManifestMenu,
    ordered: &[&skiff_pkgfmt::MenuItem],
    active_index: Option<usize>,
) -> WorkbenchTabs {
    let key = format!("plugin/{plugin_id}/{}", decl.id);
    let items: Vec<TabItem> = ordered
        .iter()
        .map(|it| {
            let view = item_view(it);
            TabItem {
                key: format!("{key}/{}", it.id),
                local: it.id.clone(),
                label: it.label.clone(),
                href: format!("/{prefix}/m/{key}/{}", it.id),
                icon: icon_or_dot(it.icon.as_deref()),
                tab: view.to_string(),
                ui: view_file(view),
            }
        })
        .collect();
    let (active_key, active_local, active_tab, active_ui) = match active_index {
        Some(i) => {
            let t = &items[i];
            (t.key.clone(), t.local.clone(), t.tab.clone(), t.ui.clone())
        }
        None => (
            key.clone(),
            String::new(),
            decl.view.clone(),
            view_file(&decl.view),
        ),
    };
    WorkbenchTabs {
        l2_key: key,
        l2_label: decl.label.clone(),
        items,
        active_key,
        active_local,
        active_tab,
        active_ui,
    }
}

fn visible_menu<'a>(
    m: &'a skiff_pkgfmt::Manifest,
    role: &str,
    docker_ok: bool,
) -> Vec<&'a skiff_pkgfmt::ManifestMenu> {
    let mut out: Vec<&skiff_pkgfmt::ManifestMenu> = m
        .menu
        .iter()
        .filter(|d| roles_allow(d.roles.as_deref(), role))
        .filter(|d| gates_allow(&d.gates, docker_ok))
        .collect();
    out.sort_by(|a, b| (a.order, &a.label, &a.id).cmp(&(b.order, &b.label, &b.id)));
    out
}

fn sorted_items(decl: &skiff_pkgfmt::ManifestMenu) -> Vec<&skiff_pkgfmt::MenuItem> {
    let mut ordered: Vec<&skiff_pkgfmt::MenuItem> = decl.items.iter().collect();
    ordered.sort_by(|a, b| (a.order, &a.label, &a.id).cmp(&(b.order, &b.label, &b.id)));
    ordered
}

/// 解析插件 L2 下的 L3 标签栏（角色/能力过滤 + 深链选中）。
///
/// 返回 `None` 表示：插件停用、L2 未声明、角色不可见、能力门不满足，
/// 或请求的 L3 不存在——调用方统一按 404 处理，不泄露不可见菜单。
pub fn workbench_tabs(
    state: &AppState,
    user: &CurrentUser,
    plugin_id: &str,
    l2_raw: &str,
    l3_raw: Option<&str>,
) -> Option<WorkbenchTabs> {
    let rec = state.db().plugin(plugin_id).ok().flatten()?;
    if !rec.enabled {
        return None;
    }
    let manifest = skiff_pkgfmt::Manifest::parse(&rec.manifest).ok()?;
    let local = menu_local(plugin_id, l2_raw)?;
    if manifest.menu.is_empty() {
        if local != "legacy" {
            return None;
        }
        return legacy_workbench(
            plugin_id,
            &state.cfg.entry_prefix,
            &manifest,
            l3_raw.map(str::trim).filter(|s| !s.is_empty()),
        );
    }
    let decl = visible_menu(&manifest, user.role().as_str(), state.docker().available())
        .into_iter()
        .find(|d| d.id == local)?;

    let ordered = sorted_items(decl);
    let requested = l3_raw.map(str::trim).filter(|s| !s.is_empty());
    let active = if ordered.is_empty() {
        // 单视图 L2：深链里的 L3 只能是隐式主视图，避免出现假标签。
        match requested {
            None => None,
            Some(raw) if raw == decl.view => None,
            Some(_) => return None,
        }
    } else {
        match requested {
            Some(raw) => Some(ordered.iter().position(|it| it.id == raw)?),
            None => Some(0),
        }
    };
    Some(declared_tabs(
        plugin_id,
        &state.cfg.entry_prefix,
        decl,
        &ordered,
        active,
    ))
}

/// 解析规范视图路由 `/{p}/plugins/{id}/{view}`：按视图名找到所属 L2/L3。
///
/// 视图名在一个插件内唯一，因此不会出现跨 L2 歧义；未知视图返回 `None`（404）。
pub fn resolve_plugin_view(
    state: &AppState,
    user: &CurrentUser,
    plugin_id: &str,
    view_raw: &str,
) -> Option<WorkbenchTabs> {
    let rec = state.db().plugin(plugin_id).ok().flatten()?;
    if !rec.enabled {
        return None;
    }
    let manifest = skiff_pkgfmt::Manifest::parse(&rec.manifest).ok()?;
    let view = menu_local(plugin_id, view_raw)?;
    if manifest.menu.is_empty() {
        return legacy_workbench(plugin_id, &state.cfg.entry_prefix, &manifest, Some(&view));
    }
    for decl in visible_menu(&manifest, user.role().as_str(), state.docker().available()) {
        if decl.items.is_empty() {
            if decl.view == view {
                return Some(declared_tabs(
                    plugin_id,
                    &state.cfg.entry_prefix,
                    decl,
                    &[],
                    None,
                ));
            }
            continue;
        }
        let ordered = sorted_items(decl);
        if let Some(i) = ordered.iter().position(|it| item_view(it) == view) {
            return Some(declared_tabs(
                plugin_id,
                &state.cfg.entry_prefix,
                decl,
                &ordered,
                Some(i),
            ));
        }
    }
    None
}

/// 插件的默认功能页视图（信息页「打开界面」入口 / `plugin/<id>` 深链落点）。
pub fn default_plugin_view(
    state: &AppState,
    user: &CurrentUser,
    plugin_id: &str,
) -> Option<WorkbenchTabs> {
    let rec = state.db().plugin(plugin_id).ok().flatten()?;
    if !rec.enabled {
        return None;
    }
    let manifest = skiff_pkgfmt::Manifest::parse(&rec.manifest).ok()?;
    if manifest.menu.is_empty() {
        return legacy_workbench(plugin_id, &state.cfg.entry_prefix, &manifest, None);
    }
    let decl =
        *visible_menu(&manifest, user.role().as_str(), state.docker().available()).first()?;
    let view = if decl.items.is_empty() {
        decl.view.clone()
    } else {
        item_view(sorted_items(decl).first()?).to_string()
    };
    resolve_plugin_view(state, user, plugin_id, &view)
}

/// 插件信息页的「关联菜单」：与导航同源的角色/能力过滤，链接指向菜单深链。
#[derive(Debug, Clone)]
pub struct InfoMenuLink {
    pub label: String,
    pub href: String,
    pub icon: String,
}

#[derive(Debug, Clone)]
pub struct InfoMenuGroup {
    pub label: String,
    pub href: String,
    pub icon: String,
    pub children: Vec<InfoMenuLink>,
}

/// 返回 `(菜单分组, 默认功能页 URL)`。无 UI 的插件返回空菜单与空 URL。
pub fn plugin_info_menu(
    state: &AppState,
    user: &CurrentUser,
    plugin_id: &str,
    manifest: &skiff_pkgfmt::Manifest,
) -> (Vec<InfoMenuGroup>, String) {
    let prefix = &state.cfg.entry_prefix;
    if manifest.menu.is_empty() {
        let Some(first) = manifest.ui.first() else {
            return (Vec::new(), String::new());
        };
        let key = format!("plugin/{plugin_id}/legacy");
        let group = InfoMenuGroup {
            label: if first.label.trim().is_empty() {
                manifest.name.clone()
            } else {
                first.label.clone()
            },
            href: format!("/{prefix}/m/{key}"),
            icon: icon_or_dot(first.icon.as_deref()),
            children: manifest
                .ui
                .iter()
                .skip(1)
                .enumerate()
                .map(|(i, u)| InfoMenuLink {
                    label: if u.label.trim().is_empty() {
                        manifest.name.clone()
                    } else {
                        u.label.clone()
                    },
                    href: format!("/{prefix}/m/{key}/{}", slug(&u.path, i)),
                    icon: icon_or_dot(u.icon.as_deref()),
                })
                .collect(),
        };
        return (vec![group], format!("/{prefix}/plugins/{plugin_id}/main"));
    }

    let mut groups = Vec::new();
    let mut entry = String::new();
    for decl in visible_menu(manifest, user.role().as_str(), state.docker().available()) {
        let ordered = sorted_items(decl);
        let key = format!("plugin/{plugin_id}/{}", decl.id);
        let view = if ordered.is_empty() {
            decl.view.clone()
        } else {
            item_view(ordered[0]).to_string()
        };
        if entry.is_empty() {
            entry = format!("/{prefix}/plugins/{plugin_id}/{view}");
        }
        groups.push(InfoMenuGroup {
            label: decl.label.clone(),
            href: format!("/{prefix}/m/{key}"),
            icon: icon_or_dot(decl.icon.as_deref()),
            children: ordered
                .iter()
                .map(|it| InfoMenuLink {
                    label: it.label.clone(),
                    href: format!("/{prefix}/m/{key}/{}", it.id),
                    icon: icon_or_dot(it.icon.as_deref()),
                })
                .collect(),
        });
    }
    (groups, entry)
}

fn slot_label(slot: &str) -> &'static str {
    match slot {
        "overview.health" => "系统状态",
        "overview.actions" => "快捷操作",
        "overview.side" => "补充信息",
        "sites.header" => "站点概览",
        _ => "插件挂载",
    }
}

/// 收集某个槽位的挂载：已启用 + 角色可见 + `ui` 合法的插件声明。
///
/// 插件停用/未授权时返回空列表，由调用方回落到内核自带内容。
pub fn mount_views(state: &AppState, user: &CurrentUser, slot: &str) -> Vec<MountView> {
    let Ok(list) = state.db().list_plugins() else {
        return Vec::new();
    };
    let role = user.role().as_str();
    let mut out = Vec::new();
    for p in list.iter().filter(|p| p.enabled) {
        let Ok(m) = skiff_pkgfmt::Manifest::parse(&p.manifest) else {
            continue;
        };
        for d in m.mount.iter().filter(|d| d.slot == slot) {
            if !roles_allow(d.roles.as_deref(), role) {
                continue;
            }
            let height = match d.height.as_deref() {
                Some("sm") => "sm",
                Some("lg") => "lg",
                _ => "md",
            };
            let refresh = match d.refresh.unwrap_or(0) {
                30 => 30,
                60 => 60,
                300 => 300,
                _ => 0,
            };
            let nonce = make_nonce();
            let src = format!(
                "/{}/plugins/{}/ui/{}?nonce={}&slot={}",
                state.cfg.entry_prefix, p.id, d.ui, nonce, slot
            );
            out.push(MountView {
                slot: slot.to_string(),
                slot_label: slot_label(slot).to_string(),
                plugin: p.id.clone(),
                title: p.name.clone(),
                src,
                height: height.to_string(),
                refresh,
                nonce,
                rpc_url: format!("/{}/plugins/{}/rpc", state.cfg.entry_prefix, p.id),
                csrf: state.csrf_token(&user.session_token),
                plugins: crate::handlers::nav_plugins(state, "")
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            });
        }
    }
    out.sort_by(|a, b| a.plugin.cmp(&b.plugin));
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuPrefs {
    pub v: u32,
    /// `top` / `left`。
    pub layout: Option<String>,
    /// 内容宽度：`auto` / `fixed`。
    pub content_width: Option<String>,
    /// `content_width = fixed` 时的最大宽度（px）。
    pub content_max: Option<u32>,
    /// `root` 为一级顺序，其余键为一级 id。
    pub order: BTreeMap<String, Vec<String>>,
    pub hidden: Vec<String>,
    pub pinned: Vec<String>,
    /// 首次保存后为 true：此后 `default_visible = false` 不再自动隐藏。
    pub initialized: bool,
}

#[derive(Debug, Clone)]
struct Decl {
    key: String,
    parent: String,
    label: String,
    icon: String,
    order: i32,
    href: String,
    plugin: bool,
    pinnable: bool,
    default_visible: bool,
    /// `None` = 全角色；`Some([])` = 任何角色不可见。
    roles: Option<Vec<String>>,
    children: Vec<Decl>,
}

impl Decl {
    fn owned_by(&self, plugin_id: &str) -> bool {
        self.key.starts_with(&format!("plugin/{plugin_id}/"))
    }
}

/// 菜单设置页的一行（包含被隐藏项）。
#[derive(Debug, Clone)]
pub struct SettingsItem {
    pub key: String,
    pub label: String,
    pub href: String,
    pub hidden: bool,
    pub pinned: bool,
    pub pinnable: bool,
    pub plugin: bool,
    pub icon: String,
}

#[derive(Debug, Clone)]
pub struct SettingsGroup {
    pub key: String,
    pub label: String,
    pub items: Vec<SettingsItem>,
}

/// 渲染用：当前页菜单（只含可见项）。
pub fn build(state: &AppState, user: &CurrentUser, active: &str) -> PageMenu {
    let prefs = load_prefs(state, user.user.id);
    let layout = effective_layout(state, &prefs);
    let width = effective_width(&prefs);
    let max_width = effective_max_width(&prefs);
    let decls = declarations(state, user);
    let (visible, _) = arrange(&decls, &prefs, user);
    let active_key = resolve_plugin_active(&visible, active);
    let hidden = effective_hidden(&decls, &prefs);

    let groups: Vec<MenuGroup> = visible
        .iter()
        .filter(|l1| !hidden.contains(&l1.key))
        .map(|l1| {
            let items = l1
                .children
                .iter()
                .filter(|i| !hidden.contains(&i.key))
                .map(|i| link(i, &active_key, |k| pinned(&prefs, k)))
                .collect::<Vec<_>>();
            let empty = if items.is_empty() {
                empty_state(state, l1.key.as_str())
            } else {
                None
            };
            MenuGroup {
                key: l1.key.clone(),
                label: l1.label.clone(),
                icon: l1.icon.clone(),
                href: l1.href.clone(),
                active: is_group_active(l1, &active_key),
                items,
                empty,
            }
        })
        .collect();

    let all: Vec<&Decl> = visible
        .iter()
        .flat_map(|l1| std::iter::once(l1).chain(l1.children.iter()))
        .collect();
    let pinned = prefs
        .pinned
        .iter()
        .filter(|k| !hidden.contains(*k))
        .filter_map(|k| all.iter().find(|d| &d.key == k).copied())
        .filter(|d| d.pinnable)
        .map(|d| link(d, &active_key, |_| true))
        .collect();

    PageMenu {
        layout,
        width,
        max_width,
        groups,
        pinned,
    }
}

/// 菜单设置页：列出当前用户可见与隐藏的全部 L1/L2。
pub fn settings_view(state: &AppState, user: &CurrentUser) -> Vec<SettingsGroup> {
    let prefs = load_prefs(state, user.user.id);
    let decls = declarations(state, user);
    let (visible, _) = arrange(&decls, &prefs, user);
    let hidden = effective_hidden(&decls, &prefs);
    visible
        .iter()
        .map(|l1| {
            let mut items: Vec<SettingsItem> = l1
                .children
                .iter()
                .map(|d| SettingsItem {
                    key: d.key.clone(),
                    label: d.label.clone(),
                    href: d.href.clone(),
                    hidden: hidden.contains(&d.key),
                    pinned: prefs.pinned.iter().any(|k| k == &d.key),
                    pinnable: d.pinnable,
                    plugin: d.plugin,
                    icon: d.icon.clone(),
                })
                .collect();
            // 一级在前，二级按当前顺序；设置页不重排，交给上移/下移。
            items.insert(
                0,
                SettingsItem {
                    key: l1.key.clone(),
                    label: format!("{}（一级）", l1.label),
                    href: l1.href.clone(),
                    hidden: hidden.contains(&l1.key),
                    pinned: false,
                    pinnable: false,
                    plugin: false,
                    icon: l1.icon.clone(),
                },
            );
            SettingsGroup {
                key: l1.key.clone(),
                label: l1.label.clone(),
                items,
            }
        })
        .collect()
}

pub fn current_layout(state: &AppState, user: &CurrentUser) -> String {
    effective_layout(state, &load_prefs(state, user.user.id))
}

pub fn current_width(state: &AppState, user: &CurrentUser) -> String {
    effective_width(&load_prefs(state, user.user.id))
}

pub fn current_max_width(state: &AppState, user: &CurrentUser) -> u32 {
    effective_max_width(&load_prefs(state, user.user.id))
}

/// 处理菜单设置页的一个动作。非法键直接忽略，不把用户卡在坏状态。
pub fn apply_action(
    state: &AppState,
    user: &CurrentUser,
    action: &str,
    key: &str,
    layout: Option<&str>,
    width: Option<&str>,
    max: Option<&str>,
) -> Result<(), AppError> {
    if action == "reset" {
        state.db().delete_user_pref(user.user.id, PREFS_KEY)?;
        return Ok(());
    }

    let decls = declarations(state, user);
    let mut prefs = materialize(state, user, &decls);
    match action {
        "layout" => {
            if let Some(l) = layout {
                if matches!(l, "top" | "left") {
                    prefs.layout = Some(l.to_string());
                }
            }
        }
        "width" => {
            if let Some(w) = width {
                if matches!(w, "auto" | "fixed") {
                    prefs.content_width = Some(w.to_string());
                }
            }
            if let Some(raw) = max.and_then(|v| v.trim().parse::<u32>().ok()) {
                if CONTENT_MAX_RANGE.contains(&raw) {
                    prefs.content_max = Some(raw);
                }
            }
        }
        "up" | "down" => move_item(&mut prefs, &decls, key, action == "up"),
        "hide" => {
            if !is_locked(key) {
                if let Some(d) = find_decl(&decls, key) {
                    if d.pinnable {
                        prefs.pinned.retain(|k| k != key);
                    }
                }
                if !prefs.hidden.iter().any(|k| k == key) {
                    prefs.hidden.push(key.to_string());
                }
            }
        }
        "show" => prefs.hidden.retain(|k| k != key),
        "pin" | "unpin" => {
            if let Some(d) = find_decl(&decls, key) {
                if d.pinnable {
                    if action == "pin" {
                        if !prefs.pinned.iter().any(|k| k == key) {
                            prefs.pinned.push(key.to_string());
                        }
                    } else {
                        prefs.pinned.retain(|k| k != key);
                    }
                }
            }
        }
        _ => {}
    }
    save_prefs(state, user, &prefs)
}

/// 插件卸载后清掉所有用户偏好里的残留键（契约 §2.6）。
pub fn cleanup_plugin_prefs(db: &skiff_store::Db, plugin_id: &str) {
    let Ok(rows) = db.list_prefs_by_key(PREFS_KEY) else {
        return;
    };
    for (user_id, raw) in rows {
        let Ok(mut prefs) = serde_json::from_str::<MenuPrefs>(&raw) else {
            continue;
        };
        drop_plugin_keys(&mut prefs, plugin_id);
        if let Ok(json) = serde_json::to_string(&prefs) {
            let _ = db.set_user_pref(user_id, PREFS_KEY, &json);
        }
    }
}

fn drop_plugin_keys(prefs: &mut MenuPrefs, plugin_id: &str) {
    let prefix = format!("plugin/{plugin_id}/");
    prefs.hidden.retain(|k| !k.starts_with(&prefix));
    prefs.pinned.retain(|k| !k.starts_with(&prefix));
    for list in prefs.order.values_mut() {
        list.retain(|k| !k.starts_with(&prefix));
    }
}

/// 插件详情页的当前菜单键（旧 `plugin:<id>` 的落点）：声明菜单取第一项，
/// 否则落到 `legacy`。找不到返回 `overview`。
pub fn plugin_primary_key(plugin_id: &str, manifest: &skiff_pkgfmt::Manifest) -> String {
    if let Some(m) = manifest.menu.first() {
        return format!("plugin/{plugin_id}/{}", m.id);
    }
    if !manifest.ui.is_empty() {
        return format!("plugin/{plugin_id}/legacy");
    }
    "overview".into()
}

fn pinned(prefs: &MenuPrefs, key: &str) -> bool {
    prefs.pinned.iter().any(|k| k == key)
}

fn link(d: &Decl, active: &str, pinned: impl Fn(&str) -> bool) -> MenuLink {
    MenuLink {
        key: d.key.clone(),
        label: d.label.clone(),
        href: d.href.clone(),
        active: active == d.key || active.starts_with(&(d.key.clone() + "/")),
        icon: d.icon.clone(),
        plugin: d.plugin,
        pinnable: d.pinnable,
        pinned: pinned(&d.key),
    }
}

fn is_group_active(g: &Decl, active: &str) -> bool {
    if !g.href.is_empty() {
        return active == g.key;
    }
    // 内置二级是 `core/<l1>/...`；插件二级是 `plugin/<plugin>/...`，
    // 只能逐项判断归属（插件不携带 L1 前缀）。
    active.starts_with(&format!("core/{}/", g.key))
        || g.children
            .iter()
            .any(|c| active == c.key || active.starts_with(&(c.key.clone() + "/")))
}

fn is_locked(key: &str) -> bool {
    L1_LOCKED.contains(&key) || L2_LOCKED.contains(&key)
}

fn resolve_plugin_active(visible: &[Decl], active: &str) -> String {
    if let Some(id) = active.strip_prefix("plugin:") {
        for l1 in visible {
            for it in &l1.children {
                if it.owned_by(id) {
                    return it.key.clone();
                }
            }
        }
    }
    canonical_active(active)
}

fn find_decl<'a>(decls: &'a [Decl], key: &str) -> Option<&'a Decl> {
    decls
        .iter()
        .flat_map(|l1| std::iter::once(l1).chain(l1.children.iter()))
        .find(|d| d.key == key)
}

/// 把 `key` 放到目标列表的相邻位置；一级与二级分别处理。
fn move_item(prefs: &mut MenuPrefs, decls: &[Decl], key: &str, up: bool) {
    let Some(parent) = decls
        .iter()
        .find(|l1| l1.key == key)
        .map(|_| "root".to_string())
        .or_else(|| {
            decls
                .iter()
                .find(|l1| l1.children.iter().any(|c| c.key == key))
                .map(|l1| l1.key.clone())
        })
    else {
        return;
    };
    let default_keys: Vec<String> = if parent == "root" {
        decls.iter().map(|d| d.key.clone()).collect()
    } else {
        decls
            .iter()
            .find(|l1| l1.key == parent)
            .map(|l1| l1.children.iter().map(|c| c.key.clone()).collect())
            .unwrap_or_default()
    };
    let list = prefs.order.entry(parent).or_insert(default_keys);
    if list.len() < 2 {
        return;
    }
    let Some(pos) = list.iter().position(|k| k == key) else {
        return;
    };
    let target = if up {
        pos.checked_sub(1)
    } else if pos + 1 < list.len() {
        Some(pos + 1)
    } else {
        None
    };
    if let Some(t) = target {
        list.swap(pos, t);
    }
}

fn effective_hidden(decls: &[Decl], prefs: &MenuPrefs) -> HashSet<String> {
    let mut hidden: HashSet<String> = prefs.hidden.iter().cloned().collect();
    if !prefs.initialized {
        for l1 in decls {
            for d in std::iter::once(l1).chain(l1.children.iter()) {
                if !d.default_visible {
                    hidden.insert(d.key.clone());
                }
            }
        }
    }
    // 锁定的项永远不隐藏（防止旧偏好把导航锁死）。
    for k in L1_LOCKED.iter().chain(L2_LOCKED.iter()) {
        hidden.remove(*k);
    }
    hidden
}

/// 首次保存时把「隐式默认」写实，避免之后改 `default_visible` 又突然冒出来。
fn materialize(state: &AppState, user: &CurrentUser, decls: &[Decl]) -> MenuPrefs {
    let mut prefs = load_prefs(state, user.user.id);
    let hidden = effective_hidden(decls, &prefs);
    let (visible, order) = arrange(decls, &prefs, user);
    prefs.v = 1;
    prefs.initialized = true;
    prefs.hidden = hidden.into_iter().collect();
    prefs.hidden.sort();
    prefs.order = order;
    // 只保留仍存在的固定项。
    let known: HashSet<&str> = visible
        .iter()
        .flat_map(|l1| std::iter::once(l1).chain(l1.children.iter()))
        .map(|d| d.key.as_str())
        .collect();
    prefs.pinned.retain(|k| known.contains(k.as_str()));
    prefs
}

fn load_prefs(state: &AppState, user_id: i64) -> MenuPrefs {
    state
        .db()
        .get_user_pref(user_id, PREFS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_prefs(state: &AppState, user: &CurrentUser, prefs: &MenuPrefs) -> Result<(), AppError> {
    let json = serde_json::to_string(prefs)
        .map_err(|e| AppError::Internal(format!("菜单偏好序列化失败: {e}")))?;
    state.db().set_user_pref(user.user.id, PREFS_KEY, &json)?;
    Ok(())
}

fn effective_layout(state: &AppState, prefs: &MenuPrefs) -> String {
    if let Some(l) = &prefs.layout {
        return l.clone();
    }
    state
        .db()
        .get_setting(GLOBAL_LAYOUT_KEY)
        .ok()
        .flatten()
        .filter(|l| matches!(l.as_str(), "top" | "left"))
        .unwrap_or_else(|| "left".into())
}

fn effective_width(prefs: &MenuPrefs) -> String {
    prefs
        .content_width
        .as_deref()
        .filter(|w| matches!(*w, "auto" | "fixed"))
        .unwrap_or(DEFAULT_CONTENT_WIDTH)
        .to_string()
}

fn effective_max_width(prefs: &MenuPrefs) -> u32 {
    prefs
        .content_max
        .filter(|v| CONTENT_MAX_RANGE.contains(v))
        .unwrap_or(DEFAULT_CONTENT_MAX)
}

/// 角色/能力门过滤。
fn declarations(state: &AppState, user: &CurrentUser) -> Vec<Decl> {
    let prefix = &state.cfg.entry_prefix;
    let href = |path: &str| format!("/{prefix}/{path}");
    let role = user.role().as_str();

    let mut root: Vec<Decl> = vec![
        Decl {
            key: L1_OVERVIEW.into(),
            parent: "root".into(),
            label: "概览".into(),
            icon: "overview".into(),
            order: 10,
            href: href("m/overview"),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: Vec::new(),
        },
        Decl {
            key: L1_SITES.into(),
            parent: "root".into(),
            label: "网站".into(),
            icon: "globe".into(),
            order: 20,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: vec![
                builtin(
                    "core/sites/list",
                    L1_SITES,
                    "网站列表",
                    "globe",
                    10,
                    href("m/core/sites/list"),
                ),
                builtin(
                    "core/sites/env",
                    L1_SITES,
                    "运行环境",
                    "container",
                    20,
                    href("m/core/sites/env"),
                ),
                builtin(
                    "core/sites/web",
                    L1_SITES,
                    "Web 服务",
                    "globe",
                    25,
                    href("m/core/sites/web"),
                ),
            ],
        },
        Decl {
            key: L1_DOCKER.into(),
            parent: "root".into(),
            label: "Docker".into(),
            icon: "docker".into(),
            order: 30,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: vec![
                builtin(
                    "core/docker/containers",
                    L1_DOCKER,
                    "容器",
                    "container",
                    10,
                    href("m/core/docker/containers"),
                ),
                builtin(
                    "core/docker/images",
                    L1_DOCKER,
                    "镜像",
                    "image",
                    20,
                    href("m/core/docker/images"),
                ),
                builtin(
                    "core/docker/networks",
                    L1_DOCKER,
                    "网络",
                    "network",
                    30,
                    href("m/core/docker/networks"),
                ),
                builtin(
                    "core/docker/volumes",
                    L1_DOCKER,
                    "卷",
                    "disk",
                    40,
                    href("m/core/docker/volumes"),
                ),
                builtin(
                    "core/docker/settings",
                    L1_DOCKER,
                    "设置",
                    "settings",
                    50,
                    href("m/core/docker/settings"),
                ),
            ],
        },
        Decl {
            key: L1_PLUGINS.into(),
            parent: "root".into(),
            label: "插件".into(),
            icon: "plug".into(),
            order: 40,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: vec![
                builtin(
                    "core/plugins/installed",
                    L1_PLUGINS,
                    "已安装",
                    "list",
                    10,
                    href("m/core/plugins/installed"),
                ),
                builtin(
                    "core/plugins/discover",
                    L1_PLUGINS,
                    "发现",
                    "activity",
                    20,
                    href("m/core/plugins/discover"),
                ),
                builtin(
                    "core/plugins/updates",
                    L1_PLUGINS,
                    "可更新",
                    "archive",
                    30,
                    href("m/core/plugins/updates"),
                ),
            ],
        },
        Decl {
            key: L1_TOOLS.into(),
            parent: "root".into(),
            label: "工具".into(),
            icon: "tools".into(),
            order: 50,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: {
                let mut items = vec![builtin(
                    "core/tasks",
                    L1_TOOLS,
                    "计划任务",
                    "clock",
                    10,
                    href("m/core/tasks"),
                )];
                // 文件管理只对管理员开放：viewer 看不到入口，直接访问也会 403。
                if user.role().as_str() == "admin" {
                    items.push(builtin(
                        "core/files",
                        L1_TOOLS,
                        "文件管理",
                        "folder",
                        20,
                        href("m/core/files"),
                    ));
                }
                items
            },
        },
        Decl {
            key: L1_SETTINGS.into(),
            parent: "root".into(),
            label: "设置".into(),
            icon: "settings".into(),
            order: 60,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: vec![
                builtin(
                    "core/settings/security",
                    L1_SETTINGS,
                    "安全设置",
                    "lock",
                    10,
                    href("m/core/settings/security"),
                ),
                builtin(
                    "core/settings/menu",
                    L1_SETTINGS,
                    "菜单设置",
                    "list",
                    20,
                    href("m/core/settings/menu"),
                ),
                builtin_role(
                    "core/settings/audit",
                    L1_SETTINGS,
                    "审计日志",
                    "shield",
                    30,
                    href("m/core/settings/audit"),
                    &["operator", "admin"],
                ),
            ],
        },
    ];
    root.retain(|l1| l1.children.iter().all(|c| c.roles_ok(role)) && l1.roles_ok(role));

    let mut plugin_decls: HashMap<String, Vec<Decl>> = HashMap::new();
    if let Ok(list) = state.db().list_plugins() {
        for p in &list {
            if !p.enabled {
                continue;
            }
            // 文件管理已由内核接管：插件包保留安装态与信息页，但不再声明菜单入口，
            // 避免同一能力出现两套 UI（旧深链由 `plugins::view` 301 到新页）。
            if p.id == "com.skiff.files" {
                continue;
            }
            let Ok(m) = skiff_pkgfmt::Manifest::parse(&p.manifest) else {
                continue;
            };
            let decls = plugin_menu_decls(&m, &p.id, prefix, state.docker().available(), role);
            for d in decls {
                plugin_decls.entry(d.parent.clone()).or_default().push(d);
            }
        }
    }
    for l1 in &mut root {
        if let Some(mut items) = plugin_decls.remove(&l1.key) {
            items.extend(std::mem::take(&mut l1.children));
            items.sort_by(default_order);
            l1.children = items;
        }
    }
    root
}

fn builtin(key: &str, parent: &str, label: &str, icon: &str, order: i32, href: String) -> Decl {
    Decl {
        key: key.into(),
        parent: parent.into(),
        label: label.into(),
        icon: icon.into(),
        order,
        href,
        plugin: false,
        pinnable: true,
        default_visible: true,
        roles: None,
        children: Vec::new(),
    }
}

fn builtin_role(
    key: &str,
    parent: &str,
    label: &str,
    icon: &str,
    order: i32,
    href: String,
    roles: &[&str],
) -> Decl {
    let mut d = builtin(key, parent, label, icon, order, href);
    d.roles = Some(roles.iter().map(|r| (*r).to_string()).collect());
    d
}

impl Decl {
    fn roles_ok(&self, role: &str) -> bool {
        match &self.roles {
            None => true,
            Some(roles) => roles.iter().any(|r| r == role),
        }
    }
}

fn default_order(a: &Decl, b: &Decl) -> std::cmp::Ordering {
    (a.order, a.plugin, &a.label, &a.key).cmp(&(b.order, b.plugin, &b.label, &b.key))
}

/// 插件 manifest → L2/L3 声明；老包走 legacy 降级（契约 §2.7）。
fn plugin_menu_decls(
    m: &skiff_pkgfmt::Manifest,
    plugin_id: &str,
    prefix: &str,
    docker_ok: bool,
    role: &str,
) -> Vec<Decl> {
    let mut out = Vec::new();
    if m.menu.is_empty() {
        let Some(ui) = m.ui.first() else {
            return out;
        };
        let parent = legacy_parent(plugin_id);
        let label = if ui.label.trim().is_empty() {
            m.name.clone()
        } else {
            ui.label.clone()
        };
        let key = format!("plugin/{plugin_id}/legacy");
        let mut children: Vec<Decl> =
            m.ui.iter()
                .skip(1)
                .enumerate()
                .map(|(i, u)| Decl {
                    key: format!("{key}/{}", slug(&u.path, i)),
                    parent: key.clone(),
                    label: if u.label.trim().is_empty() {
                        m.name.clone()
                    } else {
                        u.label.clone()
                    },
                    icon: icon_or_dot(u.icon.as_deref()),
                    order: 1000 + i as i32 * 10,
                    href: format!("/{prefix}/m/{key}/{}", slug(&u.path, i)),
                    plugin: true,
                    pinnable: true,
                    default_visible: true,
                    roles: None,
                    children: Vec::new(),
                })
                .collect();
        children.sort_by(default_order);
        out.push(Decl {
            key,
            parent,
            label,
            icon: icon_or_dot(ui.icon.as_deref()),
            order: legacy_order(plugin_id),
            href: format!("/{prefix}/m/plugin/{plugin_id}/legacy"),
            plugin: true,
            pinnable: true,
            default_visible: true,
            roles: None,
            children,
        });
        return out;
    }

    for d in &m.menu {
        let gates_ok = d.gates.iter().all(|g| match g.as_str() {
            "docker_available" => docker_ok,
            _ => true,
        });
        if !gates_ok {
            continue;
        }
        let key = format!("plugin/{plugin_id}/{}", d.id);
        let mut children: Vec<Decl> = d
            .items
            .iter()
            .map(|it| {
                let local = it.id.clone();
                Decl {
                    key: format!("{key}/{local}"),
                    parent: key.clone(),
                    label: it.label.clone(),
                    icon: icon_or_dot(it.icon.as_deref()),
                    order: it.order,
                    href: format!("/{prefix}/m/{key}/{local}"),
                    plugin: true,
                    pinnable: true,
                    default_visible: true,
                    roles: None,
                    children: Vec::new(),
                }
            })
            .collect();
        children.sort_by(default_order);
        out.push(Decl {
            key,
            parent: d.parent.clone(),
            label: d.label.clone(),
            icon: icon_or_dot(d.icon.as_deref()),
            order: d.order,
            href: format!("/{prefix}/m/plugin/{plugin_id}/{}", d.id),
            plugin: true,
            pinnable: d.pinnable,
            default_visible: d.default_visible,
            roles: d.roles.clone(),
            children,
        });
    }
    // 未授权不渲染：roles 为 None 全角色，Some([]) 任何角色都不可见。
    out.retain(|d| d.roles_ok(role));
    out
}

fn icon_or_dot(icon: Option<&str>) -> String {
    let name = icon.unwrap_or("dot");
    if skiff_pkgfmt::manifest::MENU_ICONS.contains(&name) {
        name.into()
    } else {
        "dot".into()
    }
}

fn legacy_parent(plugin_id: &str) -> String {
    match plugin_id {
        "com.skiff.nginx" => L1_SITES.into(),
        _ => L1_TOOLS.into(),
    }
}

fn legacy_order(plugin_id: &str) -> i32 {
    const OFFICIAL: &[&str] = &[
        "com.skiff.nginx",
        "com.skiff.files",
        "com.skiff.cron",
        "com.skiff.monitor",
        "com.skiff.guard",
    ];
    let idx = OFFICIAL
        .iter()
        .position(|id| *id == plugin_id)
        .unwrap_or(OFFICIAL.len());
    1000 + idx as i32 * 10
}

fn slug(path: &str, idx: usize) -> String {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .split('.')
        .next()
        .unwrap_or("");
    let mut out = String::new();
    for c in stem.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if c.is_ascii_uppercase() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        format!("ui-{idx}")
    } else {
        out
    }
}

/// 合并声明与用户顺序：已知项按用户顺序，新增项按 manifest 顺序插到相邻锚点后。
fn arrange(
    decls: &[Decl],
    prefs: &MenuPrefs,
    _user: &CurrentUser,
) -> (Vec<Decl>, BTreeMap<String, Vec<String>>) {
    let mut sorted = decls.to_vec();
    sorted.sort_by(default_order);
    let root = apply_order(sorted, prefs.order.get("root"), |d| d.key.clone());
    let mut order = BTreeMap::new();
    order.insert(
        "root".to_string(),
        root.iter().map(|d| d.key.clone()).collect(),
    );
    let visible = root
        .into_iter()
        .map(|mut l1| {
            let mut children = l1.children.clone();
            children.sort_by(default_order);
            let children = apply_order(children, prefs.order.get(&l1.key), |d| d.key.clone());
            order.insert(
                l1.key.clone(),
                children.iter().map(|d| d.key.clone()).collect(),
            );
            l1.children = children;
            l1
        })
        .collect();
    (visible, order)
}

fn apply_order<T: Clone>(
    items: Vec<T>,
    user_order: Option<&Vec<String>>,
    key: impl Fn(&T) -> String,
) -> Vec<T> {
    let Some(wanted) = user_order else {
        return items;
    };
    let default: Vec<String> = items.iter().map(&key).collect();
    let by_key: HashMap<String, T> = items.into_iter().map(|i| (key(&i), i)).collect();
    let mut out: Vec<T> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    for k in wanted {
        if let Some(item) = by_key.get(k) {
            if used.insert(k.clone()) {
                out.push(item.clone());
            }
        }
    }
    // 新项：按默认顺序找最近的「已在输出里」的同组锚点，插到它后面。
    for k in &default {
        if used.contains(k) {
            continue;
        }
        let Some(item) = by_key.get(k) else {
            continue;
        };
        let pos = default.iter().position(|d| d == k).unwrap_or(default.len());
        let anchor = default[..pos]
            .iter()
            .rev()
            .find(|d| used.contains(*d))
            .and_then(|d| out.iter().position(|x| &key(x) == d));
        match anchor {
            Some(i) => out.insert(i + 1, item.clone()),
            None => out.push(item.clone()),
        }
        used.insert(k.clone());
    }
    out
}

fn empty_state(state: &AppState, l1: &str) -> Option<MenuEmpty> {
    let (text, action) = match l1 {
        L1_TOOLS => (
            "还没有工具插件提供菜单。",
            "去插件中心启用文件/任务/监控等插件",
        ),
        _ => return None,
    };
    // 已安装但停用的同类插件：空态直接指向「已安装」，减少一次跳转。
    let installed = state
        .db()
        .list_plugins()
        .ok()
        .map(|list| {
            list.iter().any(|p| {
                !p.enabled
                    && skiff_pkgfmt::Manifest::parse(&p.manifest)
                        .map(|m| {
                            if m.menu.is_empty() {
                                legacy_parent(&p.id) == l1
                            } else {
                                m.menu.iter().any(|d| d.parent == l1)
                            }
                        })
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    let (text, tab, action) = if installed {
        ("已安装同类插件，但当前未启用。", "installed", "去启用")
    } else {
        (text, "discover", action)
    };
    Some(MenuEmpty {
        text: text.into(),
        href: format!("/{}/m/core/plugins/{tab}", state.cfg.entry_prefix),
        action: action.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu_decl(key: &str) -> skiff_pkgfmt::ManifestMenu {
        skiff_pkgfmt::ManifestMenu {
            id: key.into(),
            parent: "sites".into(),
            label: key.into(),
            order: 10,
            icon: Some("globe".into()),
            roles: None,
            gates: vec!["plugin_enabled".into()],
            default_visible: true,
            pinnable: true,
            view: "main".into(),
            items: Vec::new(),
        }
    }

    #[test]
    fn plugin_keys_are_namespaced_and_legacy_maps() {
        let m = skiff_pkgfmt::Manifest {
            id: "com.skiff.nginx".into(),
            name: "Nginx".into(),
            version: "1.0.0".into(),
            key_id: "0".repeat(64),
            api: "1.0".into(),
            run: Default::default(),
            resident_reason: None,
            requires_full_sandbox: true,
            requires: Vec::new(),
            permissions: Vec::new(),
            network_egress: Vec::new(),
            ui: vec![skiff_pkgfmt::UiEntry {
                label: "网站".into(),
                path: "index.html".into(),
                icon: Some("globe".into()),
            }],
            menu: Vec::new(),
            mount: Vec::new(),
            resource_hints: Vec::new(),
            interop: Default::default(),
            idle_timeout_sec: 300,
            max_restarts: 3,
            config_schema: BTreeMap::new(),
            cron: Vec::new(),
            memory_limit_mb: None,
            pids_limit: None,
        };
        let decls = plugin_menu_decls(&m, "com.skiff.nginx", "p", true, "admin");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].key, "plugin/com.skiff.nginx/legacy");
        assert_eq!(decls[0].parent, "sites");
        assert_eq!(plugin_primary_key("com.skiff.nginx", &m), decls[0].key);
    }

    #[test]
    fn declared_menu_keeps_l2_l3_and_docker_gate() {
        let mut m = skiff_pkgfmt::Manifest::parse(
            r#"id = "com.example.tool"
name = "Tool"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000001"
api = "1.2"
[[ui]]
label = "Tool"
path = "main.html"
[[menu]]
id = "main"
parent = "tools"
label = "工具"
order = 5
gates = ["docker_available"]
view = "main"
[[menu.items]]
id = "jobs"
label = "任务"
view = "jobs"
"#,
        )
        .unwrap();
        let decls = plugin_menu_decls(&m, "com.example.tool", "p", true, "admin");
        assert_eq!(decls[0].key, "plugin/com.example.tool/main");
        assert_eq!(
            decls[0].children[0].key,
            "plugin/com.example.tool/main/jobs"
        );
        assert_eq!(decls[0].order, 5);

        m.menu = vec![menu_decl("main")];
        m.menu[0].gates = vec!["docker_available".into()];
        let decls = plugin_menu_decls(&m, "com.example.tool", "p", false, "admin");
        assert!(decls.is_empty(), "docker 不可用时整项不出现");
    }

    #[test]
    fn order_keeps_user_order_and_inserts_new_items_after_anchor() {
        let mut prefs = MenuPrefs::default();
        prefs.order.insert(
            "docker".into(),
            vec![
                "core/docker/images".into(),
                "core/docker/containers".into(),
                "core/docker/volumes".into(),
            ],
        );
        let decls = vec![
            builtin(
                "core/docker/containers",
                "docker",
                "容器",
                "container",
                10,
                "/a".into(),
            ),
            builtin(
                "core/docker/images",
                "docker",
                "镜像",
                "image",
                20,
                "/b".into(),
            ),
            builtin(
                "core/docker/networks",
                "docker",
                "网络",
                "network",
                30,
                "/c".into(),
            ),
            builtin(
                "core/docker/volumes",
                "docker",
                "卷",
                "disk",
                40,
                "/d".into(),
            ),
        ];
        let out = apply_order(decls, prefs.order.get("docker"), |d| d.key.clone());
        let keys: Vec<&str> = out.iter().map(|d| d.key.as_str()).collect();
        // 已知项按用户顺序；新项 networks 按默认锚点（images）之后插入。
        assert_eq!(keys[0], "core/docker/images");
        assert_eq!(
            keys[1], "core/docker/networks",
            "新项插到默认锚点 images 之后"
        );
        assert_eq!(keys[2], "core/docker/containers");
        assert_eq!(keys[3], "core/docker/volumes");
        assert_eq!(keys.len(), 4);
    }

    #[test]
    fn hidden_and_locked_prefs_are_respected() {
        let decls = vec![
            Decl {
                key: L1_OVERVIEW.into(),
                parent: "root".into(),
                label: "概览".into(),
                icon: "overview".into(),
                order: 10,
                href: "/m/overview".into(),
                plugin: false,
                pinnable: false,
                default_visible: true,
                roles: None,
                children: Vec::new(),
            },
            Decl {
                key: "core/docker/containers".into(),
                parent: "docker".into(),
                label: "容器".into(),
                icon: "container".into(),
                order: 10,
                href: "/m/core/docker/containers".into(),
                plugin: false,
                pinnable: true,
                default_visible: true,
                roles: None,
                children: Vec::new(),
            },
        ];
        let prefs = MenuPrefs {
            hidden: vec![L1_OVERVIEW.into(), "core/docker/containers".into()],
            ..Default::default()
        };
        let hidden = effective_hidden(&decls, &prefs);
        assert!(!hidden.contains(L1_OVERVIEW), "锁定的概览不能隐藏");
        assert!(hidden.contains("core/docker/containers"));
    }

    #[test]
    fn default_visible_false_only_applies_before_first_save() {
        let decls = vec![Decl {
            key: "plugin/x/menu".into(),
            parent: "tools".into(),
            label: "X".into(),
            icon: "dot".into(),
            order: 10,
            href: String::new(),
            plugin: true,
            pinnable: true,
            default_visible: false,
            roles: None,
            children: Vec::new(),
        }];
        let hidden = effective_hidden(&decls, &MenuPrefs::default());
        assert!(hidden.contains("plugin/x/menu"));
        let mut saved = MenuPrefs {
            initialized: true,
            ..Default::default()
        };
        saved.hidden.clear();
        let hidden = effective_hidden(&decls, &saved);
        assert!(!hidden.contains("plugin/x/menu"));
    }

    #[test]
    fn cleanup_drops_plugin_keys() {
        let mut order = BTreeMap::new();
        order.insert("tools".to_string(), vec!["plugin/x/menu".to_string()]);
        let mut prefs = MenuPrefs {
            hidden: vec!["plugin/x/menu".into(), "core/settings/audit".into()],
            pinned: vec!["plugin/x/menu".into()],
            order,
            ..Default::default()
        };
        drop_plugin_keys(&mut prefs, "x");
        assert!(prefs.hidden.iter().all(|k| !k.starts_with("plugin/x/")));
        assert!(prefs.pinned.iter().all(|k| !k.starts_with("plugin/x/")));
        assert!(prefs
            .order
            .values()
            .flat_map(|v| v.iter())
            .all(|k| !k.starts_with("plugin/x/")));
    }
}
