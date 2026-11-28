//! 三级菜单模型、声明合并与用户偏好（`docs/contract/MENU_CONTRACT.md` P1）。
//!
//! 这里是**唯一**的菜单合成入口：固定七一级 + 内置二级 + 已启用插件声明，
//! 再套上当前用户的顺序/隐藏/固定/布局偏好。模板只消费 `PageMenu`，
//! 不再自己拼导航（旧的 `handlers::nav` 已删除）。

mod decl;
mod mount;
mod prefs;
mod render;
mod settings;
mod workbench;

// --- 外部（web crate 等）需要的 pub 接口 ---
pub use decl::sort_items;
pub use mount::{mount_views, MountView};
pub use render::{build, MenuEmpty, MenuGroup, MenuLink, PageMenu};
pub use settings::{
    apply_action, cleanup_plugin_prefs, settings_view, SettingsGroup, SettingsItem,
};
pub use workbench::{
    default_plugin_view, plugin_info_menu, resolve_plugin_view, TabItem, WorkbenchTabs,
};

/// 偏好表内的键；仅 `menu.prefs` 一行，JSON 内容由本模块解释。
pub const PREFS_KEY: &str = "menu.prefs";

/// 菜单领域错误：只区分数据层失败与偏好序列化失败，HTTP 语义由 web 层映射。
#[derive(Debug)]
pub enum MenuError {
    Store(skiff_store::StoreError),
    Internal(String),
}

impl std::fmt::Display for MenuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuError::Store(e) => write!(f, "{e}"),
            MenuError::Internal(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for MenuError {}

impl From<skiff_store::StoreError> for MenuError {
    fn from(e: skiff_store::StoreError) -> Self {
        MenuError::Store(e)
    }
}

/// 顶栏「应用」菜单与插件挂载共用的条目。
#[derive(Debug, Clone)]
pub struct UiPluginEntry {
    pub id: String,
    /// manifest 里的插件名。
    pub name: String,
    /// 首个 `[[ui]]` 的展示名；未填时回落插件名。
    pub label: String,
}

/// 已启用且有 UI 入口的插件（顶栏「应用」菜单与插件挂载的公共数据源）。
///
/// 只列已启用、带 `[[ui]]` 的插件。入口归属完全由 manifest 的 `parent` 决定，
/// 这里不做任何「某个插件例外」的硬过滤。DB 读失败或 manifest 解析失败时跳过该项，
/// 坏包不拖垮整站导航。
pub fn enabled_ui_plugins(state: &crate::AppState) -> Vec<UiPluginEntry> {
    let Ok(list) = state.db().list_plugins() else {
        return Vec::new();
    };
    list.iter()
        .filter(|p| p.enabled)
        .filter_map(|p| {
            let m = skiff_pkgfmt::Manifest::parse(&p.manifest).ok()?;
            let ui = m.ui.first()?;
            let label = if ui.label.is_empty() {
                p.name.clone()
            } else {
                ui.label.clone()
            };
            Some(UiPluginEntry {
                id: p.id.clone(),
                name: p.name.clone(),
                label,
            })
        })
        .collect()
}

/// 旧 `active` 字符串 → 新菜单键（契约 §7.2）。未知值原样返回。
pub fn canonical_active(active: &str) -> String {
    match active {
        "dashboard" => "overview".into(),
        "containers" => "core/docker/containers".into(),
        "images" => "core/docker/images".into(),
        "networks" => "core/docker/networks".into(),
        "volumes" => "core/docker/volumes".into(),
        "compose" => "core/docker/compose".into(),
        "plugins" => "core/plugins/installed".into(),
        "settings" => "core/settings/security".into(),
        "tasks" => "core/tasks/scheduled".into(),
        "jobs" => "core/tasks/jobs".into(),
        "audit" => "core/tasks/audit".into(),
        "menu-settings" => "core/settings/menu".into(),
        other => other.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use decl::{plugin_menu_decls, Decl, L1_OVERVIEW, L1_TOOLS};
    use prefs::{effective_hidden, MenuPrefs};

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
            schema: None,
            items: Vec::new(),
        }
    }

    #[test]
    fn plugin_keys_are_namespaced_and_legacy_maps() {
        // 旧包没有 `[[menu]]`：内核只做「一级 = 工具 + 视图平铺」，不替它猜归属
        // （曾经的 `com.skiff.nginx → 网站` 特例已随插件内核化删除）。
        let m = skiff_pkgfmt::Manifest {
            id: "com.example.legacy".into(),
            name: "Legacy".into(),
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
                label: "旧视图".into(),
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
            task: Vec::new(),
            cron: Vec::new(),
            memory_limit_mb: None,
            pids_limit: None,
        };
        let decls = plugin_menu_decls(&m, "com.example.legacy", "p", true, "admin");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].key, "plugin/com.example.legacy/legacy");
        assert_eq!(decls[0].parent, L1_TOOLS);
        assert_eq!(
            workbench::plugin_primary_key("com.example.legacy", &m),
            decls[0].key
        );
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
                href: "/".into(),
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
                href: "/containers".into(),
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
        use settings::drop_plugin_keys;
        let mut order = BTreeMap::new();
        order.insert("tools".to_string(), vec!["plugin/x/menu".to_string()]);
        let mut prefs = MenuPrefs {
            hidden: vec!["plugin/x/menu".into(), "core/tasks/audit".into()],
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

    // --- 测试辅助 ---

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

    fn apply_order<T: Clone>(
        items: Vec<T>,
        user_order: Option<&Vec<String>>,
        key: impl Fn(&T) -> String,
    ) -> Vec<T> {
        let Some(wanted) = user_order else {
            return items;
        };
        let default: Vec<String> = items.iter().map(&key).collect();
        let by_key: std::collections::HashMap<String, T> =
            items.into_iter().map(|i| (key(&i), i)).collect();
        let mut out: Vec<T> = Vec::new();
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        for k in wanted {
            if let Some(item) = by_key.get(k) {
                if used.insert(k.clone()) {
                    out.push(item.clone());
                }
            }
        }
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
}
