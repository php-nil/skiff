//! 菜单渲染：把「全量声明」套上用户偏好和活跃态，产出模板消费的 `PageMenu`。

use crate::{AppState, CurrentUser};

use super::decl::Decl;
use super::prefs::{arrange, effective_hidden, load_prefs};

/// 模板消费的菜单条目类型。
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
    pub groups: Vec<MenuGroup>,
    pub pinned: Vec<MenuLink>,
}

fn pinned(prefs: &super::prefs::MenuPrefs, key: &str) -> bool {
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
    super::canonical_active(active)
}

pub(super) fn find_decl<'a>(decls: &'a [Decl], key: &str) -> Option<&'a Decl> {
    decls
        .iter()
        .flat_map(|l1| std::iter::once(l1).chain(l1.children.iter()))
        .find(|d| d.key == key)
}

fn empty_state(state: &AppState, l1: &str) -> Option<MenuEmpty> {
    let (text, action) = match l1 {
        super::decl::L1_TOOLS => (
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
                                super::decl::L1_TOOLS == l1
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
    let href = format!("/{}/plugins/{tab}", state.cfg.entry_prefix);
    Some(MenuEmpty {
        text: text.into(),
        href,
        action: action.into(),
    })
}

/// 渲染用：当前页菜单（只含可见项）。
pub fn build(state: &AppState, user: &CurrentUser, active: &str) -> PageMenu {
    let prefs = load_prefs(state, user.user.id);
    let decls = super::decl::declarations(state, user);
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

    PageMenu { groups, pinned }
}
