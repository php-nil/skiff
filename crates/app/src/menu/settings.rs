//! 菜单设置页：偏好修改（上移/下移/隐藏/显示/固定/重置）+ 插件卸载残留清理。

use crate::{AppState, CurrentUser};

use super::prefs::{
    arrange, effective_hidden, load_prefs, materialize, move_item, save_prefs, MenuPrefs,
};

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

fn is_locked(key: &str) -> bool {
    super::decl::L1_LOCKED.contains(&key) || super::decl::L2_LOCKED.contains(&key)
}

/// 菜单设置页：列出当前用户可见与隐藏的全部 L1/L2。
pub fn settings_view(state: &AppState, user: &CurrentUser) -> Vec<SettingsGroup> {
    let prefs = load_prefs(state, user.user.id);
    let decls = super::decl::declarations(state, user);
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

/// 处理菜单设置页的一个动作。非法键直接忽略，不把用户卡在坏状态。
pub fn apply_action(
    state: &AppState,
    user: &CurrentUser,
    action: &str,
    key: &str,
) -> Result<(), super::MenuError> {
    if action == "reset" {
        state
            .db()
            .delete_user_pref(user.user.id, super::PREFS_KEY)?;
        return Ok(());
    }

    let decls = super::decl::declarations(state, user);
    let mut prefs = materialize(state, user, &decls);
    match action {
        "up" | "down" => move_item(&mut prefs, &decls, key, action == "up"),
        "hide" => {
            if !is_locked(key) {
                if let Some(d) = super::render::find_decl(&decls, key) {
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
            if let Some(d) = super::render::find_decl(&decls, key) {
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
///
/// 逐个用户读改写；单条失败不影响其余用户，最终把清理结果写成
/// `plugin.menu.cleanup` 审计（契约要求卸载时必须留痕）。
pub fn cleanup_plugin_prefs(db: &skiff_store::Db, plugin_id: &str) {
    let (cleaned, failed) = match db.list_prefs_by_key(super::PREFS_KEY) {
        Ok(rows) => scrub_prefs(db, plugin_id, rows),
        Err(e) => {
            tracing::warn!(error = %e, plugin = %plugin_id, "读取菜单偏好失败，跳过插件菜单残留清理");
            audit_cleanup(db, plugin_id, "fail", &format!("读取菜单偏好失败：{e}"));
            return;
        }
    };
    audit_cleanup(
        db,
        plugin_id,
        if failed == 0 { "ok" } else { "partial" },
        &format!("users={cleaned} failed={failed}"),
    );
}

fn scrub_prefs(db: &skiff_store::Db, plugin_id: &str, rows: Vec<(i64, String)>) -> (usize, usize) {
    let (mut cleaned, mut failed) = (0usize, 0usize);
    for (user_id, raw) in rows {
        let Ok(mut prefs) = serde_json::from_str::<MenuPrefs>(&raw) else {
            failed += 1;
            continue;
        };
        drop_plugin_keys(&mut prefs, plugin_id);
        let written = serde_json::to_string(&prefs)
            .map_err(|e| e.to_string())
            .and_then(|json| {
                db.set_user_pref(user_id, super::PREFS_KEY, &json)
                    .map_err(|e| e.to_string())
            });
        match written {
            Ok(()) => cleaned += 1,
            Err(e) => {
                tracing::warn!(error = %e, user = user_id, plugin = %plugin_id, "清理菜单偏好失败");
                failed += 1;
            }
        }
    }
    (cleaned, failed)
}

fn audit_cleanup(db: &skiff_store::Db, plugin_id: &str, result: &str, detail: &str) {
    if let Err(e) = db.audit(
        "system",
        "-",
        "plugin.menu.cleanup",
        plugin_id,
        result,
        detail,
    ) {
        tracing::error!(error = %e, "菜单清理审计写入失败");
    }
}

pub(super) fn drop_plugin_keys(prefs: &mut MenuPrefs, plugin_id: &str) {
    let prefix = format!("plugin/{plugin_id}/");
    prefs.hidden.retain(|k| !k.starts_with(&prefix));
    prefs.pinned.retain(|k| !k.starts_with(&prefix));
    for list in prefs.order.values_mut() {
        list.retain(|k| !k.starts_with(&prefix));
    }
}
