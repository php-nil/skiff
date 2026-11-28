//! 固定页槽位（`[[mount]]`）的渲染视图（契约 §5）。

use crate::{AppState, CurrentUser};

/// 固定页槽位的渲染视图（契约 §5）。
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
    let nav_ids = super::enabled_ui_plugins(state)
        .into_iter()
        .map(|p| p.id)
        .collect::<Vec<_>>()
        .join(",");
    let mut out = Vec::new();
    for p in list.iter().filter(|p| p.enabled) {
        let Ok(m) = skiff_pkgfmt::Manifest::parse(&p.manifest) else {
            continue;
        };
        for d in m.mount.iter().filter(|d| d.slot == slot) {
            let roles_ok = d
                .roles
                .as_deref()
                .map(|roles| roles.iter().any(|r| r == role))
                .unwrap_or(true);
            if !roles_ok {
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
                plugins: nav_ids.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.plugin.cmp(&b.plugin));
    out
}
