//! 插件工作台（L2/L3 标签栏 + 视图解析 + 插件信息页菜单）。

use skiff_model::url::menu_href;

use crate::{AppState, CurrentUser};

use super::decl::{item_view, slug, sorted_items, view_file, visible_menu};

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

fn icon_or_dot(icon: Option<&str>) -> String {
    let name = icon.unwrap_or("dot");
    if skiff_pkgfmt::manifest::MENU_ICONS.contains(&name) {
        name.into()
    } else {
        "dot".into()
    }
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
                href: menu_href(prefix, &format!("{key}/{view}")),
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
                href: menu_href(prefix, &format!("plugin/{plugin_id}/{view}")),
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

/// 解析插件 L2 下的 L3 标签栏（角色/能力过滤 + 深链选中）。
///
/// 返回 `None` 表示：插件停用、L2 未声明、角色不可见、能力门不满足，
/// 或请求的 L3 不存在——调用方统一按 404 处理，不泄露不可见菜单。
#[allow(dead_code)]
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
    let local = super::menu_local(plugin_id, l2_raw)?;
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
    let view = super::menu_local(plugin_id, view_raw)?;
    if manifest.menu.is_empty() {
        return legacy_workbench(plugin_id, &state.cfg.entry_prefix, &manifest, Some(&view));
    }
    let visible = visible_menu(&manifest, user.role().as_str(), state.docker().available());
    // 1. 按 view 名精确匹配（URL 直接带 view）。
    for decl in &visible {
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
    // 2. fallback：按 L2 menu id 匹配（URL 带 L2 id，如 `/plugins/{pid}/sites`）。
    //    取其默认 view（L2 自己的 view，或第一个 L3 item）。
    for decl in &visible {
        if decl.id != view {
            continue;
        }
        if decl.items.is_empty() {
            return Some(declared_tabs(
                plugin_id,
                &state.cfg.entry_prefix,
                decl,
                &[],
                None,
            ));
        }
        let ordered = sorted_items(decl);
        let default_idx = 0;
        return Some(declared_tabs(
            plugin_id,
            &state.cfg.entry_prefix,
            decl,
            &ordered,
            Some(default_idx),
        ));
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
            href: menu_href(prefix, &key),
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
                    href: menu_href(prefix, &format!("{key}/{}", slug(&u.path, i))),
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
            href: menu_href(prefix, &key),
            icon: icon_or_dot(decl.icon.as_deref()),
            children: ordered
                .iter()
                .map(|it| InfoMenuLink {
                    label: it.label.clone(),
                    href: menu_href(prefix, &format!("{key}/{}", it.id)),
                    icon: icon_or_dot(it.icon.as_deref()),
                })
                .collect(),
        });
    }
    (groups, entry)
}

/// 插件详情页的当前菜单键（旧 `plugin:<id>` 的落点）：声明菜单取第一项，
/// 否则落到 `legacy`。找不到返回 `overview`。
#[allow(dead_code)]
pub fn plugin_primary_key(plugin_id: &str, manifest: &skiff_pkgfmt::Manifest) -> String {
    if let Some(m) = manifest.menu.first() {
        return format!("plugin/{plugin_id}/{}", m.id);
    }
    if !manifest.ui.is_empty() {
        return format!("plugin/{plugin_id}/legacy");
    }
    "overview".into()
}
