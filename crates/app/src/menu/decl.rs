//! 菜单声明模型（内核固定七一级 + 插件 manifest 合并）。
//!
//! 这里的 `Decl` 是菜单合成的中间态——既不是 manifest 的原始条目，也不是
//! 渲染用的 `MenuLink`/`PageMenu`。它承载「固定 L1 + 已启用插件 L2/L3」合并后、
//! 套用户偏好前的全量结构。

use std::collections::HashMap;

use skiff_model::url::menu_href;
use skiff_model::Form;

use crate::{AppState, CurrentUser};

/// 一级菜单稳定键（`root/<id>` 的 `<id>` 部分）。契约 §1.2。
pub const L1_OVERVIEW: &str = "overview";
pub const L1_SITES: &str = "sites";
pub const L1_DOCKER: &str = "docker";
pub const L1_PLUGINS: &str = "plugins";
pub const L1_TASKS: &str = "tasks";
pub const L1_TOOLS: &str = "tools";
pub const L1_SETTINGS: &str = "settings";

pub(super) const L1_LOCKED: &[&str] = &[L1_OVERVIEW, L1_SETTINGS];
pub(super) const L2_LOCKED: &[&str] = &["core/settings/menu"];

#[derive(Debug, Clone)]
pub struct Decl {
    pub key: String,
    pub parent: String,
    pub label: String,
    pub icon: String,
    pub order: i32,
    pub href: String,
    pub plugin: bool,
    pub pinnable: bool,
    pub default_visible: bool,
    /// `None` = 全角色；`Some([])` = 任何角色不可见。
    pub roles: Option<Vec<String>>,
    pub children: Vec<Decl>,
}

impl Decl {
    pub fn owned_by(&self, plugin_id: &str) -> bool {
        self.key.starts_with(&format!("plugin/{plugin_id}/"))
    }

    pub(super) fn roles_ok(&self, role: &str) -> bool {
        match &self.roles {
            None => true,
            Some(roles) => roles.iter().any(|r| r == role),
        }
    }
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

/// 菜单声明排序：`(order, label, id)`。SSR 导航与 `/ui/v1` 菜单共用同一顺序。
pub fn sort_decls(items: &mut [&skiff_pkgfmt::ManifestMenu]) {
    items.sort_by(|a, b| (a.order, &a.label, &a.id).cmp(&(b.order, &b.label, &b.id)));
}

/// L3 视图项排序：`(order, label, id)`。SSR 导航与 `/ui/v1` 菜单共用同一顺序。
pub fn sort_items(items: &mut [&skiff_pkgfmt::MenuItem]) {
    items.sort_by(|a, b| (a.order, &a.label, &a.id).cmp(&(b.order, &b.label, &b.id)));
}

pub(super) fn sorted_items(decl: &skiff_pkgfmt::ManifestMenu) -> Vec<&skiff_pkgfmt::MenuItem> {
    let mut ordered: Vec<&skiff_pkgfmt::MenuItem> = decl.items.iter().collect();
    sort_items(&mut ordered);
    ordered
}

pub(super) fn visible_menu<'a>(
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
    sort_decls(&mut out);
    out
}

pub(super) fn item_view(it: &skiff_pkgfmt::MenuItem) -> &str {
    it.view.as_deref().unwrap_or(it.id.as_str())
}

pub(super) fn view_file(view: &str) -> String {
    format!("{view}.html")
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

pub(super) fn default_order(a: &Decl, b: &Decl) -> std::cmp::Ordering {
    (a.order, a.plugin, &a.label, &a.key).cmp(&(b.order, b.plugin, &b.label, &b.key))
}

fn icon_or_dot(icon: Option<&str>) -> String {
    let name = icon.unwrap_or("dot");
    if skiff_pkgfmt::manifest::MENU_ICONS.contains(&name) {
        name.into()
    } else {
        "dot".into()
    }
}

pub(super) fn slug(path: &str, idx: usize) -> String {
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

/// 插件 manifest → L2/L3 声明；老包走 legacy 降级（契约 §2.6）。
pub(super) fn plugin_menu_decls(
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
        // 旧包（只声明 `[[ui]]`、没有 `[[menu]]`）统一挂到「工具」：它们没有声明归属，
        // 内核不替它猜一级菜单。
        let parent = L1_TOOLS.to_string();
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
                    href: menu_href(prefix, &format!("{key}/{}", slug(&u.path, i))),
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
            order: 1000,
            href: menu_href(prefix, &format!("plugin/{plugin_id}/legacy")),
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
                    href: menu_href(prefix, &format!("{key}/{local}")),
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
            href: menu_href(prefix, &format!("plugin/{plugin_id}/{}", d.id)),
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

/// 角色/能力门过滤 + 插件声明合并。返回全量声明（按默认顺序）。
pub(super) fn declarations(state: &AppState, user: &CurrentUser) -> Vec<Decl> {
    let prefix = &state.cfg.entry_prefix;
    let href = |path: &str| skiff_model::url::entry_href(prefix, path);
    let role = user.role().as_str();

    let mut root: Vec<Decl> = vec![
        Decl {
            key: L1_OVERVIEW.into(),
            parent: "root".into(),
            label: "概览".into(),
            icon: "overview".into(),
            order: 10,
            href: menu_href(prefix, "overview"),
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
                    href("sites/list"),
                ),
                builtin(
                    "core/sites/env",
                    L1_SITES,
                    "运行环境",
                    "container",
                    20,
                    href("sites/runtimes"),
                ),
                builtin(
                    "core/sites/web",
                    L1_SITES,
                    "Web 服务",
                    "globe",
                    25,
                    href("sites/web"),
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
                    href("docker/containers"),
                ),
                builtin(
                    "core/docker/images",
                    L1_DOCKER,
                    "镜像",
                    "image",
                    20,
                    href("docker/images"),
                ),
                builtin(
                    "core/docker/networks",
                    L1_DOCKER,
                    "网络",
                    "network",
                    30,
                    href("docker/networks"),
                ),
                builtin(
                    "core/docker/volumes",
                    L1_DOCKER,
                    "卷",
                    "disk",
                    40,
                    href("docker/volumes"),
                ),
                builtin(
                    "core/docker/compose",
                    L1_DOCKER,
                    "Compose",
                    "list",
                    45,
                    href("docker/compose"),
                ),
                builtin(
                    "core/docker/settings",
                    L1_DOCKER,
                    "设置",
                    "settings",
                    50,
                    href("docker/settings"),
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
            children: {
                let mut items = vec![
                    builtin(
                        "core/plugins/installed",
                        L1_PLUGINS,
                        "已安装",
                        "list",
                        10,
                        href("plugins/installed"),
                    ),
                    builtin(
                        "core/plugins/discover",
                        L1_PLUGINS,
                        "发现",
                        "activity",
                        20,
                        href("plugins/discover"),
                    ),
                    builtin(
                        "core/plugins/updates",
                        L1_PLUGINS,
                        "可更新",
                        "archive",
                        30,
                        href("plugins/updates"),
                    ),
                ];
                // 商店默认关闭：只有显式启用目录接入后才出现入口。
                if state.cfg.catalog.enabled {
                    items.push(builtin(
                        "core/plugins/store",
                        L1_PLUGINS,
                        "商店",
                        "archive",
                        40,
                        href("plugins/store"),
                    ));
                }
                items
            },
        },
        Decl {
            key: L1_TASKS.into(),
            parent: "root".into(),
            label: "任务审计".into(),
            icon: "activity".into(),
            order: 45,
            href: String::new(),
            plugin: false,
            pinnable: false,
            default_visible: true,
            roles: None,
            children: vec![
                builtin(
                    "core/tasks/scheduled",
                    L1_TASKS,
                    "计划任务",
                    "clock",
                    10,
                    href("tasks/scheduled"),
                ),
                builtin(
                    "core/tasks/jobs",
                    L1_TASKS,
                    "任务中心",
                    "activity",
                    20,
                    href("tasks/jobs"),
                ),
                builtin_role(
                    "core/tasks/audit",
                    L1_TASKS,
                    "审计日志",
                    "shield",
                    30,
                    href("tasks/audit"),
                    &["operator", "admin"],
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
                let mut items = Vec::new();
                // 文件管理只对管理员开放：viewer 看不到入口（壳页虽渲染，数据端点 403）。
                // 受限形态（1Panel Docker 包）能力面只有 docker/identity，没有 fs：
                // 入口同样隐藏；直达壳页虽渲染，`/ui/v1/files*` 数据端点由 `fs_capability` 拒绝。
                if user.role().as_str() == "admin" && state.cfg.form != Form::DockerRestricted {
                    // 回收站是「文件管理」的 L3 视图，不再作为独立二级菜单项。
                    // 回收站与配额都是壳页的 L3：`/{p}/files/trash`、`/{p}/files/quotas`。
                    let files = builtin(
                        "core/files",
                        L1_TOOLS,
                        "文件管理",
                        "folder",
                        20,
                        href("files"),
                    );
                    items.push(Decl {
                        children: vec![builtin(
                            "core/files/trash",
                            "core/files",
                            "回收站",
                            "archive",
                            10,
                            href("files/trash"),
                        )],
                        ..files
                    });
                    items.push(builtin(
                        "core/files/quotas",
                        L1_TOOLS,
                        "配额管理",
                        "hard-drive",
                        22,
                        href("files/quotas"),
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
                    href("settings/security"),
                ),
                builtin(
                    "core/settings/menu",
                    L1_SETTINGS,
                    "菜单设置",
                    "list",
                    20,
                    href("settings/menu"),
                ),
                builtin(
                    "core/settings/ui",
                    L1_SETTINGS,
                    "界面",
                    "image",
                    25,
                    href("settings/ui"),
                ),
            ],
        },
    ];
    // 角色门逐项过滤：受限子项只隐藏自己，不连坐整个一级。
    // （例：viewer 看不到「审计日志」，但仍应看到「设置」与「任务审计」的其他入口。）
    for l1 in &mut root {
        l1.children.retain(|c| c.roles_ok(role));
    }
    root.retain(|l1| l1.roles_ok(role));

    let mut plugin_decls: HashMap<String, Vec<Decl>> = HashMap::new();
    // 受限形态（`SKIFF_FORM=docker`）没有 fs 能力面：依赖 fs 的插件入口不渲染，
    // 深链由 `plugins::view` 回退到内核页；判定与 `middleware::check_fs_capability` 同源。
    let fs_ok = state.cfg.form != Form::DockerRestricted;
    if let Ok(list) = state.db().list_plugins() {
        for p in &list {
            if !p.enabled {
                continue;
            }
            let Ok(m) = skiff_pkgfmt::Manifest::parse(&p.manifest) else {
                continue;
            };
            if !fs_ok && m.requires.iter().any(|c| c.as_str() == "fs") {
                continue;
            }
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
