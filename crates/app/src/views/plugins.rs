//! 插件行读模型：SSR 页面与 `/ui/v1` 共用同一份取数与派生。
//!
//! 只装业务数据；`running` 由调用方按 Supervisor 实际状态填（这里默认 false）。

use skiff_pkgfmt::Manifest;
use skiff_store::PluginRecord;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    pub status: String,
    pub enabled: bool,
    /// 完整公钥指纹（信息页展示；空串 = 未签名包）。
    pub key_id_full: String,
    pub grants: String,
    /// manifest `requires`（宿主能力面，如 docker/identity）。
    pub requires: String,
    /// manifest `permissions`（动作面，如 fs:read）。
    pub permissions: String,
    pub signed: bool,
    pub reduced: bool,
    pub running: bool,
    /// manifest `[[ui]]` 的第一个入口（相对 `ui/` 的路径）；空串表示无 UI。
    pub ui_path: String,
    /// 已安装版本被服务端撤包/吊销时的提示（来自本地签名索引快照，不触网）。
    pub takedown_note: String,
}

pub fn plugin_row(
    state: &AppState,
    p: &PluginRecord,
) -> Result<PluginRow, skiff_store::StoreError> {
    let signed = !p.key_id.is_empty() && state.db().trusted_key(&p.key_id)?.is_some();
    let manifest = Manifest::parse(&p.manifest).ok();
    let reduced = manifest
        .as_ref()
        .map(|m| !m.requires_full_sandbox)
        .unwrap_or(true);
    let ui = manifest.as_ref().and_then(|m| m.ui.first());
    Ok(PluginRow {
        id: p.id.clone(),
        name: p.name.clone(),
        version: p.version.clone(),
        api: p.api.clone(),
        status: p.status.clone(),
        enabled: p.enabled,
        key_id_full: p.key_id.clone(),
        grants: p
            .grants
            .iter()
            .map(|g| format!("{}:{}", g.kind, g.value))
            .collect::<Vec<_>>()
            .join(", "),
        requires: manifest
            .as_ref()
            .map(|m| m.requires_str().join(", "))
            .unwrap_or_default(),
        permissions: manifest
            .as_ref()
            .map(|m| m.permission_strings().join(", "))
            .unwrap_or_default(),
        signed,
        reduced,
        running: false,
        ui_path: ui.map(|u| u.path.clone()).unwrap_or_default(),
        takedown_note: {
            // 已装版本被撤包时给出明确升级指向：有可安装新版本就报版本号，
            // 没有就说明当前无可装版本（不把用户留到点「安装」才被拒）。
            let mut note = state
                .catalog()
                .cached_takedown(&p.id, &p.version)
                .unwrap_or_default();
            if !note.is_empty() {
                match state.catalog().cached_installable_version(&p.id) {
                    Some(v) if skiff_plugin::install::version_gt(&v, &p.version) => {
                        note.push_str(&format!("；请升级到 {v}"))
                    }
                    _ => note.push_str("；当前没有可安装的新版本"),
                }
            }
            note
        },
    })
}

pub fn plugin_rows(state: &AppState) -> Result<Vec<PluginRow>, skiff_store::StoreError> {
    state
        .db()
        .list_plugins()?
        .iter()
        .map(|p| plugin_row(state, p))
        .collect()
}
