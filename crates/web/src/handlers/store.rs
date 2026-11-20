//! 商店目录快照与升级包下载：`/ui/v1/store` 的数据装配与审计。
//!
//! 只消费 `CatalogService::snapshot`：拉取/验签/缓存/防回滚都在 `skiff_app::catalog`
//! 内完成。安装与升级包的「下载 → 摘要/大小 → 验签 → 暂存」由
//! `skiff_app::commands::store` 负责，本模块只做权限判定与审计。

use std::collections::HashMap;
use std::sync::Arc;

use skiff_app::catalog::CatalogState;
use skiff_model::Permission;

use crate::error::AppError;
use crate::handlers;
use crate::templates::UpgradeHint;
use skiff_app::{AppState, CurrentUser};

/// 商店安装的统一审计动作：成功暂存、各类拒绝都落这一条。
const AUDIT_ACTION: &str = "store.install";

/// 内核升级只读段的审计动作：手动下载成功与各类拒绝都落这一条。
const AUDIT_UPGRADE: &str = "upgrade.download";

/// 商店只读视图：`/ui/v1/store` 的数据装配。
pub(crate) struct StoreView {
    pub snapshot: skiff_app::catalog::CatalogSnapshot,
    pub plugins: Vec<skiff_app::catalog::CatalogItem>,
    pub upgrade: UpgradeHint,
    pub can_install: bool,
}

/// 拉取目录快照并装配视图（含插件行的可安装状态与升级提示）。
pub(crate) async fn load_view(state: &Arc<AppState>, force: bool) -> Result<StoreView, AppError> {
    if !state.cfg.catalog.enabled {
        return Err(AppError::NotFound("商店目录未启用".into()));
    }
    let worker = Arc::clone(state);
    let (snap, latest) = handlers::blocking_fs(move || {
        let catalog = worker.catalog();
        let snap = catalog.snapshot(force);
        let latest = catalog.latest_upgrade(
            skiff_app::commands::upgrade::KERNEL_VERSION,
            skiff_app::commands::upgrade::host_arch(),
        );
        (snap, latest)
    })
    .await?;

    // 只有公钥已配置且目录没有被拒时才给安装入口：未配置公钥的目录无法完成包验签。
    let can_install = snap.key_configured
        && !matches!(snap.state, CatalogState::Rejected | CatalogState::Disabled);
    let available = latest
        .as_ref()
        .map(|p| p.entry.version.clone())
        .unwrap_or_default();
    let can_download = !available.is_empty() && can_install;
    let upgrade = UpgradeHint {
        current: skiff_app::commands::upgrade::KERNEL_VERSION.to_string(),
        arch: skiff_app::commands::upgrade::host_arch().to_string(),
        available,
        channel: latest
            .as_ref()
            .map(|p| p.channel.clone())
            .unwrap_or_else(|| snap.channel.clone()),
        min_from: latest
            .as_ref()
            .map(|p| p.entry.min_from.clone())
            .unwrap_or_default(),
        changelog_url: latest
            .as_ref()
            .map(|p| p.entry.changelog_url.clone())
            .unwrap_or_default(),
        note: if latest.is_none() {
            "无可用升级".into()
        } else if !snap.key_configured {
            "未配置固定公钥，不提供升级下载".into()
        } else if matches!(snap.state, CatalogState::Rejected | CatalogState::Disabled) {
            "目录校验失败，暂不提供升级下载".into()
        } else {
            String::new()
        },
        can_download,
    };
    // 版本选择器：一个 id 只有一个可安装版本（未 yank、未吊销的最大版本）；
    // 与安装 POST 共用 `installable_versions`，页面显示哪个就装哪个。
    let mut plugins = snap.entries.plugins.clone();
    let installable = skiff_app::catalog::installable_versions(&plugins);
    let newest = skiff_app::catalog::newest_versions(&plugins);
    // 最高版本行的撤回/下架原因：同 id 其它版本没有自带原因时复用它。
    let newest_warn: HashMap<String, String> = plugins
        .iter()
        .filter(|r| !r.warn.is_empty())
        .filter(|r| newest.get(&r.name).is_some_and(|v| v == &r.version))
        .map(|r| (r.name.clone(), r.warn.clone()))
        .collect();
    for r in plugins.iter_mut() {
        r.installable = installable.get(&r.name).is_some_and(|v| v == &r.version);
        if r.installable {
            // 已装旧版本时按钮改「升级」；同版本重装仍是「安装」。
            // 已装版本高于可安装版本时不提供降级入口（后端同样拒绝）。
            if let Ok(Some(installed)) = state.db().plugin(&r.name) {
                if installed.version != r.version {
                    if skiff_plugin::install::version_gt(&r.version, &installed.version) {
                        r.action = "升级".into();
                    } else {
                        r.installable = false;
                        r.install_note = format!(
                            "已安装 {} 高于目录可安装版本 {}，不接受降级",
                            installed.version, r.version
                        );
                    }
                }
            }
        } else if !r.warn.is_empty() {
            // 本行自带撤回/下架原因：按钮旁直接说明。
            r.install_note = r.warn.clone();
        } else if let Some(chosen) = installable.get(&r.name) {
            r.install_note = format!("请安装 {chosen}");
        } else {
            // 全部不可安装：最高版本行没有自身原因时回落到该 id 的最高版本原因，
            // 再没有就保持通用文案，按钮始终禁用。
            r.install_note = newest_warn
                .get(&r.name)
                .cloned()
                .unwrap_or_else(|| "目录中没有可安装的版本".into());
        }
    }
    Ok(StoreView {
        snapshot: snap,
        plugins,
        upgrade,
        can_install,
    })
}
/// 一次升级包下载的结果：成功暂存，或已落审计的业务失败（由调用方决定提示方式）。
pub(crate) enum UpgradeDownloadOutcome {
    Staged(skiff_app::commands::upgrade::StagedUpgrade),
    Failed(String),
}

/// 升级包下载的共享业务入口：旧表单 GET 与 `/ui/v1` 共用同一权限、审计与校验。
///
/// 目录未启用 / 权限不足 / 未配置公钥仍以 `Err` 返回（页面 403/400、JSON 同码）；
/// 下载阶段失败返回 `Failed`，因为旧页面把它渲染成 flash 提示而不是错误页。
pub(crate) async fn apply_upgrade_download(
    state: &Arc<AppState>,
    user: &CurrentUser,
    ip: &str,
    version: &str,
) -> Result<UpgradeDownloadOutcome, AppError> {
    if !state.cfg.catalog.enabled {
        return Err(AppError::NotFound("商店目录未启用".into()));
    }
    if let Err(e) = handlers::require_permission(user, Permission::Admin) {
        audit_upgrade(state, user, ip, version, "deny", "reason=permission");
        return Err(e);
    }
    if state.cfg.catalog.public_key_hex().is_none() {
        audit_upgrade(state, user, ip, version, "deny", "reason=no_key");
        return Err(AppError::BadRequest(
            "未配置固定 Ed25519 公钥，不提供升级下载".into(),
        ));
    }

    let req = skiff_app::commands::upgrade::UpgradeDownloadRequest {
        version: Some(version.to_string()),
        arch: skiff_app::commands::upgrade::host_arch().to_string(),
        current: skiff_app::commands::upgrade::KERNEL_VERSION.to_string(),
        data_dir: state.cfg.data_dir.clone(),
    };
    let worker = Arc::clone(state);
    let result = handlers::blocking_fs(move || {
        skiff_app::commands::upgrade::download(worker.catalog(), &req)
    })
    .await?;

    match result {
        Ok(staged) => {
            audit_upgrade(
                state,
                user,
                ip,
                &staged.version,
                "ok",
                &format!(
                    "reason=staged channel={} source={} sha256={} size={} path={} reused={}",
                    staged.channel,
                    staged.source,
                    staged.sha256,
                    staged.size,
                    staged.path.display(),
                    staged.reused
                ),
            );
            Ok(UpgradeDownloadOutcome::Staged(staged))
        }
        Err(e) => {
            audit_upgrade(
                state,
                user,
                ip,
                version,
                "fail",
                &format!("reason={} {e}", e.reason()),
            );
            Ok(UpgradeDownloadOutcome::Failed(e.message()))
        }
    }
}
pub(crate) fn audit(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    target: &str,
    result: &str,
    detail: &str,
) {
    state.audit_or_log(
        &user.user.username,
        ip,
        AUDIT_ACTION,
        target,
        result,
        detail,
    );
}

fn audit_upgrade(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    target: &str,
    result: &str,
    detail: &str,
) {
    state.audit_or_log(
        &user.user.username,
        ip,
        AUDIT_UPGRADE,
        target,
        result,
        detail,
    );
}
