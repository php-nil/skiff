//! 插件生命周期命令：安装、卸载、启停的唯一业务实现。
//!
//! 过去 Web（上传 → 确认页）与 CLI（`skiff plugin ...`）各写一份，已出现可观测漂移：
//! CLI 卸载不清菜单偏好、任务回收两条路径、审计 detail 格式两份。这里把「装完/卸完
//! 到底要做什么」收成一处；调用方只保留参数解析、权限判定与话术。
//!
//! CLI 是独立进程，没有 `AppState`/Supervisor/任务服务，因此这些函数只依赖
//! `&Db` + 数据根，任务服务作为可选协作者传入。

use std::path::Path;
use std::sync::Arc;

use skiff_plugin::{InstallOptions, InstalledInfo};
use skiff_store::Db;

use crate::tasks::TaskService;

#[derive(Debug)]
pub enum PluginCommandError {
    NotFound(String),
    Rejected(String),
}

impl std::fmt::Display for PluginCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(m) | Self::Rejected(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PluginCommandError {}

/// 安装审计 detail：Web 与 CLI 共用同一格式，便于事后按字段检索。
pub fn install_detail(info: &InstalledInfo) -> String {
    format!(
        "version={} key_id={} grants={}",
        info.version,
        info.key_id,
        info.grants.len()
    )
}

/// 卸载审计 detail。
pub fn uninstall_detail(delete_data: bool) -> String {
    format!("delete_data={delete_data}")
}

/// 安装包并清理内核自己的 `.incoming` 暂存。同步版本供 CLI 使用（无任务服务）。
///
/// 审计不在这里写：Web 走 `state.audit_or_log`、CLI 直写库，但 detail 用上面的
/// 同一个函数生成。
///
/// **只删内核落下的暂存副本，且只在装成功之后**（两条都必要）：
/// - 路径条件：`package` 可能是用户/发行包给的路径（CLI 常直接指
///   `/www/skiff/official/com.skiff.<x>-<ver>.wbpkg`）。那是发行包自带的官方包，
///   既是装失败后重试的唯一副本，也是启动时比对官方版本、做随包更新的来源。
/// - 结果条件：装失败还删源包＝「拒装一次，源包就没了」，用户连改配置重试的机会都没有
///   （2026-11-23 实测：开 `allow_reduced_sandbox` 之前连拒 8 次，
///   `/www/skiff/official` 被清空）。
pub fn install(
    db: &Db,
    data_dir: &Path,
    package: &Path,
    opts: &InstallOptions,
    kfs: &crate::admin_fs::AdminFs,
) -> Result<InstalledInfo, PluginCommandError> {
    let staged = package.starts_with(data_dir.join("plugins").join(".incoming"));
    let result = skiff_plugin::install_package(db, data_dir, package, opts)
        .map_err(|e| PluginCommandError::Rejected(e.to_string()));
    // 暂存包是内核自己落下的中间产物：装成功后经内核通道删除（留一条 `fs.remove`）。
    if result.is_ok() && staged && package.exists() {
        let _ = kfs.remove_kernel(package);
    }
    result
}

/// Web 安装：在同步安装之后种入 manifest 声明的内核任务（不要求重启）。
pub async fn install_with_tasks(
    db: &Db,
    data_dir: &Path,
    package: &Path,
    opts: &InstallOptions,
    tasks: Option<&Arc<TaskService>>,
    kfs: &crate::admin_fs::AdminFs,
) -> Result<InstalledInfo, PluginCommandError> {
    let info = install(db, data_dir, package, opts, kfs)?;
    if let Some(tasks) = tasks {
        tasks.sync_manifest_tasks().await;
    }
    Ok(info)
}

/// 卸载收尾（不含停进程）：删包/数据 → 回收任务 → 清菜单偏好。
///
/// 返回回收的任务条数。同步版本供 CLI 使用，按库内 `source=plugin:<id>` 直删。
pub fn remove(
    db: &Db,
    data_dir: &Path,
    id: &str,
    delete_data: bool,
) -> Result<usize, PluginCommandError> {
    skiff_plugin::uninstall_package(db, data_dir, id, delete_data)
        .map_err(|e| PluginCommandError::Rejected(e.to_string()))?;
    let dropped = drop_plugin_tasks_db(db, id)?;
    // 契约 §2.6：卸载后清掉所有用户偏好里该插件的顺序/隐藏/固定残留。
    crate::menu::cleanup_plugin_prefs(db, id);
    Ok(dropped)
}

/// Web 卸载：有任务服务时走调度器（内存态一并更新），否则回退库内直删。
pub async fn remove_with_tasks(
    db: &Db,
    data_dir: &Path,
    id: &str,
    delete_data: bool,
    tasks: Option<&Arc<TaskService>>,
) -> Result<usize, PluginCommandError> {
    let Some(tasks) = tasks else {
        return remove(db, data_dir, id, delete_data);
    };
    skiff_plugin::uninstall_package(db, data_dir, id, delete_data)
        .map_err(|e| PluginCommandError::Rejected(e.to_string()))?;
    let dropped = tasks.drop_plugin_tasks(id).await;
    crate::menu::cleanup_plugin_prefs(db, id);
    Ok(dropped)
}

/// 启用/停用（停进程由调用方在停用前完成）。
pub fn set_enabled(db: &Db, id: &str, enabled: bool) -> Result<(), PluginCommandError> {
    if db
        .plugin(id)
        .map_err(|e| PluginCommandError::Rejected(e.to_string()))?
        .is_none()
    {
        return Err(PluginCommandError::NotFound(format!("插件不存在: {id}")));
    }
    db.set_plugin_enabled_state(id, enabled)
        .map_err(|e| PluginCommandError::Rejected(e.to_string()))
}

/// 无任务服务时的库内回收：`source = plugin:<id>`。
fn drop_plugin_tasks_db(db: &Db, id: &str) -> Result<usize, PluginCommandError> {
    let source = format!("plugin:{id}");
    let mut dropped = 0usize;
    for task in db
        .scheduled_tasks()
        .map_err(|e| PluginCommandError::Rejected(e.to_string()))?
    {
        if task.source == source {
            db.delete_scheduled_task(&task.id)
                .map_err(|e| PluginCommandError::Rejected(e.to_string()))?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn opts() -> InstallOptions {
        InstallOptions {
            trusted_public_key: None,
            allow_unsigned: false,
            granted_permissions: Vec::new(),
            object_grants: Vec::new(),
            available_capabilities: Vec::new(),
            allow_reduced_sandbox: true,
            arch: skiff_pkgfmt::host_arch(),
            actor: "test".into(),
        }
    }

    /// 拒装不得删源包：CLI 常直接把发行包自带的
    /// `/www/skiff/official/com.skiff.<x>-<ver>.wbpkg` 交给安装器，那是装失败后
    /// 重试的唯一副本、也是启动时比对官方版本的来源。装不上再删掉它，用户就只能
    /// 重新上传（2026-11-23 实测：沙箱开关没开时连拒 8 次，官方包目录被清空）。
    #[test]
    fn rejected_install_keeps_the_source_package() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        fs::create_dir_all(data.join("plugins")).unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let kfs =
            crate::admin_fs::AdminFs::with_paths(data.clone(), tmp.path().join("www"), db.clone());

        // 两种来路都要保住：发行包的官方目录、内核自己的上传暂存目录。
        for (tag, pkg) in [
            (
                "官方包目录",
                tmp.path()
                    .join("official")
                    .join("com.skiff.demo-0.1.0.wbpkg"),
            ),
            (
                "上传暂存",
                data.join("plugins").join(".incoming").join("tok24.wbpkg"),
            ),
        ] {
            fs::create_dir_all(pkg.parent().unwrap()).unwrap();
            // 不是合法 wbpkg：安装必然在校验阶段被拒。
            fs::write(&pkg, b"not a real wbpkg").unwrap();
            let err = install(&db, &data, &pkg, &opts(), &kfs).unwrap_err();
            assert!(
                matches!(err, PluginCommandError::Rejected(_)),
                "{tag}：应按「拒装」返回，实际 {err}"
            );
            assert!(
                pkg.exists(),
                "{tag}：装失败后源包必须还在 {}",
                pkg.display()
            );
        }
    }
}
