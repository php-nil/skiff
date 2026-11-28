//! 用户偏好持久化 + 顺序合并。
//!
//! `MenuPrefs` 是 `menu.prefs` 表一行的 JSON 内容：保存用户对菜单的顺序/隐藏/固定。
//! `arrange` 负责把「全量声明」套上用户偏好——已知项按用户顺序，新项按 manifest
//! 顺序插到最近锚点后，保证升级引入的新入口不会被丢到末尾。

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{AppState, CurrentUser};

use super::decl::{default_order, Decl};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuPrefs {
    pub v: u32,
    /// `root` 为一级顺序，其余键为一级 id。
    pub order: BTreeMap<String, Vec<String>>,
    pub hidden: Vec<String>,
    pub pinned: Vec<String>,
    /// 首次保存后为 true：此后 `default_visible = false` 不再自动隐藏。
    pub initialized: bool,
}

pub(super) fn load_prefs(state: &AppState, user_id: i64) -> MenuPrefs {
    state
        .db()
        .get_user_pref(user_id, super::PREFS_KEY)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(super) fn save_prefs(
    state: &AppState,
    user: &CurrentUser,
    prefs: &MenuPrefs,
) -> Result<(), super::MenuError> {
    let json = serde_json::to_string(prefs)
        .map_err(|e| super::MenuError::Internal(format!("菜单偏好序列化失败: {e}")))?;
    state
        .db()
        .set_user_pref(user.user.id, super::PREFS_KEY, &json)?;
    Ok(())
}

/// 合并声明与用户顺序：已知项按用户顺序，新增项按 manifest 顺序插到相邻锚点后。
pub(super) fn arrange(
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
    let by_key: std::collections::HashMap<String, T> =
        items.into_iter().map(|i| (key(&i), i)).collect();
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

/// 首次保存时把「隐式默认」写实，避免之后改 `default_visible` 又突然冒出来。
pub(super) fn materialize(state: &AppState, user: &CurrentUser, decls: &[Decl]) -> MenuPrefs {
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

pub(super) fn effective_hidden(decls: &[Decl], prefs: &MenuPrefs) -> HashSet<String> {
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
    for k in super::decl::L1_LOCKED
        .iter()
        .chain(super::decl::L2_LOCKED.iter())
    {
        hidden.remove(*k);
    }
    hidden
}

/// 把 `key` 放到目标列表的相邻位置；一级与二级分别处理。
pub(super) fn move_item(prefs: &mut MenuPrefs, decls: &[Decl], key: &str, up: bool) {
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
