//! 安装 / 升级 / 卸载（§4.3、§4.5）。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use skiff_pkgfmt::package::{verify_and_stage, VerifyOptions, DEFAULT_LIMITS};
use skiff_pkgfmt::Manifest;
use skiff_store::{Db, Grant, PluginRecord};
use tracing::{info, warn};

use crate::trust::TrustDecision;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("包校验失败: {0}")]
    Package(#[from] skiff_pkgfmt::PkgError),
    #[error("存储错误: {0}")]
    Store(#[from] skiff_store::StoreError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("拒绝安装: {0}")]
    Rejected(String),
    #[error("平台不支持: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// 信任库中该 key_id 对应的公钥（无则需 allow_unsigned 或先导入）
    pub trusted_public_key: Option<String>,
    pub allow_unsigned: bool,
    /// 用户确认后的权限子集（`cap:verb`）；未列出的权限不授予
    pub granted_permissions: Vec<String>,
    /// 对象 grant（`container:<id>` / `project:<name>`）
    pub object_grants: Vec<String>,
    /// 内核当前形态提供的原子能力（native: 四项；Docker 受限: docker/identity）
    pub available_capabilities: Vec<skiff_pkgfmt::manifest::Capability>,
    /// 是否允许降级沙箱（landlock/cgroups 未实现时的显式开关）
    pub allow_reduced_sandbox: bool,
    pub arch: String,
    /// 审计执行者（`cli` / 面板用户名 / `official-sync`），用于升级补授留痕。
    pub actor: String,
}

#[derive(Debug, Clone)]
pub struct InstalledInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub key_id: String,
    pub signature_verified: bool,
    pub grants: Vec<Grant>,
    pub reduced_sandbox: bool,
    pub warnings: Vec<String>,
}

/// 组装安装选项（CLI 与 Web 共用同一份）。
///
/// 能力集随**部署形态**、架构随**宿主**、降级沙箱开关来自**配置**——这三项
/// 过去在 CLI（`skiff plugin install`）与 Web（上传 → 确认页 → 安装）各写一遍，
/// 漏改一处就会让「同一个包在两个入口装出不同结果」，而且事后极难复现。
///
/// 两条链路的差别只有两处，由调用方给定：
/// - 用户确认了哪些权限与对象（CLI 来自 `--grant`/交互，Web 来自确认页表单）；
/// - 有效未签名开关（CLI 是 `--allow-unsigned || cfg.allow_unsigned`，
///   Web 是 `cfg.allow_unsigned`）。
pub fn install_options(
    trust: &TrustDecision,
    granted_permissions: Vec<String>,
    object_grants: Vec<String>,
    allow_unsigned: bool,
    form: skiff_model::Form,
    allow_reduced_sandbox: bool,
    actor: &str,
) -> InstallOptions {
    InstallOptions {
        trusted_public_key: trust.trusted_key.clone(),
        allow_unsigned,
        granted_permissions,
        object_grants,
        available_capabilities: skiff_pkgfmt::manifest::Capability::available_in(form),
        allow_reduced_sandbox,
        arch: skiff_pkgfmt::host_arch(),
        actor: actor.to_string(),
    }
}

/// 插件目录布局（§5.2）：`versions/` 代码只读、`data/` 业务数据、`cache/`、`run/`。
pub fn plugin_root(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("plugins").join(id)
}

/// 某个已安装版本的代码目录：`<plugin_root>/versions/<version>`。
///
/// 版本目录布局只在这里维护，启动、校验、UI/schema 托管统一走本函数。
pub fn plugin_version_dir(data_dir: &Path, id: &str, version: &str) -> PathBuf {
    plugin_root(data_dir, id).join("versions").join(version)
}

/// 某个已安装版本的 UI 资产目录：`<version_dir>/ui`。
pub fn plugin_ui_dir(data_dir: &Path, id: &str, version: &str) -> PathBuf {
    plugin_version_dir(data_dir, id, version).join("ui")
}

/// 按插件 id 稳定分配 UID/GID（200000–200999，不进 docker 组）。
pub fn plugin_uid(id: &str) -> u32 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    let digest = h.finalize();
    let n = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    200_000 + (n % 1000)
}

#[cfg(unix)]
const SHARED_GROUP: &str = "skiff-plugins";

/// 插件运行组 `skiff-plugins` 的 gid；组不存在时返回 `None`（开发机手跑的情形）。
///
/// 插件的主 gid 是自己的独立 gid，**必须**把运行组作为附加组补上，否则：
/// - 进不去 `/run/skiff`（0711，靠其他位可穿越，但 socket 仍按组判权）；
/// - 打不开 `kernel.sock`（0660 root:skiff-plugins），所有能力调用一律 EACCES。
///
/// 这一条只在真机（unix）上生效，Windows 开发机上永远走 `None` 分支。
#[cfg(unix)]
pub fn shared_gid() -> Option<u32> {
    let name = std::ffi::CString::new(SHARED_GROUP).ok()?;
    // 不能用 getgrnam：它返回进程级静态区（musl 下是无锁的 static char*/group，
    // 每次调用 free 重用同一个指针）。并发 spawn 会让两个线程对同一指针双重 free，
    // 直接打坏 mallocng 元数据 → 内核进程 SIGSEGV/SIGBUS（2026-11-06 测试机多次崩溃）。
    // getgrnam_r 结果全落调用方缓冲区，才是可重入的。
    let mut buf = vec![0u8; 1024];
    loop {
        let mut gr: libc::group = unsafe { std::mem::zeroed() };
        let mut out: *mut libc::group = std::ptr::null_mut();
        // SAFETY: name 是合法 C 字符串；gr/buf 都是本线程独占的有效内存，
        // getgrnam_r 只会写入这两块区域，失败时 out 保持 null。
        let rc = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                &mut gr,
                buf.as_mut_ptr().cast::<libc::c_char>(),
                buf.len(),
                &mut out,
            )
        };
        if rc == libc::ERANGE && buf.len() < 64 * 1024 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || out.is_null() {
            return None;
        }
        // SAFETY: out 指向上面的 gr，成功时 gr_gid 已由 getgrnam_r 填好。
        return Some(unsafe { (*out).gr_gid });
    }
}

#[cfg(not(unix))]
pub fn shared_gid() -> Option<u32> {
    None
}

pub fn install_package(
    db: &Db,
    data_dir: &Path,
    package: &Path,
    opts: &InstallOptions,
) -> Result<InstalledInfo> {
    let mut warnings = Vec::new();

    let verify_opts = VerifyOptions {
        trusted_public_key: opts.trusted_public_key.clone(),
        allow_unsigned: opts.allow_unsigned,
        limits: DEFAULT_LIMITS,
        arch: opts.arch.clone(),
    };
    let staging_root = data_dir.join("plugins").join(".incoming");
    fs::create_dir_all(&staging_root)?;
    let unpacked = verify_and_stage(package, &staging_root, &verify_opts)?;
    let meta = unpacked.meta.clone();

    // 能力校验：装前拦，而不是装上再坏（§4.1）
    for cap in &meta.manifest.requires {
        if !opts.available_capabilities.contains(cap) {
            return Err(PluginError::Rejected(format!(
                "插件需要能力 {}，当前部署形态不提供（能力面收缩见 §5.4）",
                cap.as_str()
            )));
        }
    }
    // 沙箱降级：landlock/cgroups 未实现，完整沙箱插件默认拒装，allow_reduced_sandbox 打开后放行
    let reduced_sandbox = !meta.manifest.requires_full_sandbox;
    if meta.manifest.requires_full_sandbox && !opts.allow_reduced_sandbox {
        return Err(PluginError::Rejected(
            "该插件要求完整沙箱（requires_full_sandbox = true），当前实现尚未提供 landlock/cgroups；\
             需 admin 显式开启 allow_reduced_sandbox 才可降级安装"
                .into(),
        ));
    }
    if meta.manifest.requires_full_sandbox && opts.allow_reduced_sandbox {
        warnings.push("沙箱降级：仅独立 UID + 文件权限（landlock/cgroups 未实现）".into());
    }
    if !meta.signature_verified {
        warnings.push("包未经验签（allow_unsigned）：仅限开发调试，生产环境必须导入公钥".into());
    }

    // 版本策略：同版本视为重装（原位覆盖版本目录），低于已装版本拒绝
    // （灾难恢复入口不在此实现）。
    let existing = db.plugin(&meta.manifest.id)?;
    if let Some(prev) = &existing {
        if !version_gt(&meta.manifest.version, &prev.version)
            && meta.manifest.version != prev.version
        {
            return Err(PluginError::Rejected(format!(
                "已安装版本 {} 高于包内版本 {}；不接受降级（同版本可直接重装）",
                prev.version, meta.manifest.version
            )));
        }
        if prev.key_id != meta.manifest.key_id && !opts.allow_unsigned {
            return Err(PluginError::Rejected(
                "升级包签名公钥与已装版本不一致：换密钥视为新信任，需重新确认指纹".into(),
            ));
        }
    }

    // 授权来源分两条：
    // - 首次安装：只授予用户确认过的子集，且必须是 manifest 声明过的；
    // - 重装/升级：保留库内既有 grants（既有授权绝不清空），按新 manifest 的声明集过滤，
    //   并把**相对已装版本新声明**的能力动词与对象 hint 补齐——上限始终是新 manifest，
    //   不会出现越权授权；老声明里被用户拒绝过的项不翻案（只在旧 manifest 未声明时才算新增）。
    //   补齐项必须留痕（见下方 `plugin.grant_upgrade` 审计），不静默扩权。
    let declared = meta.manifest.permission_strings();
    let prev_manifest = match &existing {
        Some(prev) => match Manifest::parse(&prev.manifest) {
            Ok(m) => Some(m),
            Err(e) => {
                // 旧 manifest 读不出来就无法判断「新增」，按保守口径不补授。
                warn!(plugin = %prev.id, error = %e, "旧 manifest 不可解析，升级不自动补授新增权限");
                None
            }
        },
        None => None,
    };
    let prev_declared = prev_manifest.as_ref().map(|m| m.permission_strings());
    // 旧 manifest 声明过的对象 hint（含 `[interop].consumes` 派生项）：只有**新 manifest
    // 新增**的 hint 才补授；旧 manifest 已声明但用户当初没勾的 hint 同样不翻案。
    let prev_hints = prev_manifest.as_ref().map(|m| {
        let mut set: BTreeSet<String> = m.resource_hints.iter().cloned().collect();
        set.extend(m.interop_resource_hints());
        set
    });
    let mut grant_upgrade: Vec<String> = Vec::new();
    let mut object_upgrade: Vec<String> = Vec::new();
    let grants = match &existing {
        Some(prev) => {
            let mut kept = prev
                .grants
                .iter()
                .filter(|g| g.kind != "perm" || declared.iter().any(|d| d == &g.value))
                .cloned()
                .collect::<Vec<Grant>>();
            let newly_declared: Vec<&String> = match &prev_declared {
                Some(old) => declared.iter().filter(|p| !old.contains(p)).collect(),
                None => Vec::new(),
            };
            for p in newly_declared {
                if kept.iter().any(|g| g.kind == "perm" && g.value == *p) {
                    continue;
                }
                grant_upgrade.push(p.clone());
                kept.push(Grant::perm(p.clone()));
            }

            // 对象授权（`resource_hints` + 由 `[interop].consumes` 派生的 `plugin:`）同样
            // 只补「相对旧 manifest 新增」的项：只增不减，已有授权不动；manifest 移除 hint
            // 不自动回收——回收只走显式卸载。形式校验放在落盘前，失败即整体拒绝，
            // 不会留下「文件已换、授权没补」的半完成状态。
            // 旧 manifest 不可解析时无法判断新增，保守不补（与权限动词同口径）。
            if let Some(old_hints) = &prev_hints {
                let mut hints = meta.manifest.resource_hints.clone();
                hints.extend(meta.manifest.interop_resource_hints());
                for h in hints {
                    skiff_pkgfmt::manifest::validate_resource(&h)
                        .map_err(|e| PluginError::Rejected(format!("对象 grant {h} 非法: {e}")))?;
                    if old_hints.contains(&h) {
                        continue;
                    }
                    if kept.iter().any(|g| g.kind == "object" && g.value == h) {
                        continue;
                    }
                    object_upgrade.push(h.clone());
                    kept.push(Grant::object(h));
                }
            }
            kept
        }
        None => {
            let mut grants = Vec::new();
            for p in &opts.granted_permissions {
                if !declared.contains(p) {
                    return Err(PluginError::Rejected(format!(
                        "权限 {p} 未在 manifest 中声明，拒绝授予"
                    )));
                }
                grants.push(Grant::perm(p.clone()));
            }
            for o in &opts.object_grants {
                // 对象 grant 的三形态与 manifest `resource_hints` 共用同一份形式校验：
                // `path:` 只做形式校验（绝对路径 / 无 .. / 非 `/`），存在性由内核每次调用
                // canonicalize 判定——允许「先装插件、后建目录」。
                skiff_pkgfmt::manifest::validate_resource(o)
                    .map_err(|e| PluginError::Rejected(format!("对象 grant {o} 非法: {e}")))?;
                grants.push(Grant::object(o.clone()));
            }
            grants
        }
    };

    // UID 冲突：uid 由 id 哈希取模 1000 得到，撞车时两个插件就是「同一个身份」，
    // 彼此的 data/ 都是 0700 同 uid，互相可读——隔离假成立。装前拦下，别等真机发现。
    let want_uid = plugin_uid(&meta.manifest.id);
    if let Some(owner) = db
        .list_plugins()?
        .into_iter()
        .find(|p| p.id != meta.manifest.id && plugin_uid(&p.id) == want_uid)
    {
        return Err(PluginError::Rejected(format!(
            "插件 UID 冲突：{} 与已安装的 {} 都映射到 uid {}（只有 200000–200999 一千个槽位），             两者会共用文件权限身份；安装已中止。",
            meta.manifest.id, owner.id, want_uid
        )));
    }

    // 落盘：versions/{version}（N-1 保留）
    let root = plugin_root(data_dir, &meta.manifest.id);
    for sub in ["versions", "data", "cache", "run"] {
        fs::create_dir_all(root.join(sub))?;
    }
    let version_dir = plugin_version_dir(data_dir, &meta.manifest.id, &meta.manifest.version);
    if version_dir.exists() {
        remove_tree(&version_dir)?;
    }
    let (meta, staging) = unpacked.persist();
    match fs::rename(&staging, &version_dir) {
        Ok(()) => {}
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(PluginError::Io(e));
        }
    }
    prune_versions(&root, 2)?;
    set_plugin_ownership(&root, &meta.manifest.id)?;

    let record = PluginRecord {
        id: meta.manifest.id.clone(),
        name: meta.manifest.name.clone(),
        version: meta.manifest.version.clone(),
        api: meta.manifest.api.clone(),
        key_id: meta.manifest.key_id.clone(),
        manifest: String::from_utf8_lossy(&meta.manifest_raw).to_string(),
        enabled: true,
        status: "installed".into(),
        status_detail: String::new(),
        active_version: meta.manifest.version.clone(),
        installed_at: chrono::Utc::now().timestamp(),
        grants: grants.clone(),
    };
    db.upsert_plugin(&record)?;
    db.audit(
        "system",
        "local",
        "plugin.install",
        &record.id,
        "ok",
        &format!(
            "version={} key_id={} signed={} grants={}",
            record.version,
            record.key_id,
            meta.signature_verified,
            grants.len()
        ),
    )?;
    info!(plugin = %record.id, version = %record.version, "插件安装完成");

    // 升级补齐授权不是静默行为：只读动词记日志；写/删除/执行类动词与任何新增对象授权
    // 额外落一条审计，写清执行者、新增项、来源（manifest 新声明 / `resource_hints`）、
    // 以及来源版本，便于事后核对「这台机器什么时候、因哪条 hint 扩的权」。
    if !grant_upgrade.is_empty() || !object_upgrade.is_empty() {
        let version_from = existing
            .as_ref()
            .map(|p| p.version.as_str())
            .unwrap_or("(未安装)");
        let mut added = grant_upgrade.clone();
        added.extend(object_upgrade.iter().cloned());
        let added = added.join(",");
        let sensitive = grant_upgrade
            .iter()
            .filter(|p| is_sensitive_perm(p))
            .cloned()
            .collect::<Vec<String>>();
        let mut reasons = Vec::new();
        if !grant_upgrade.is_empty() {
            reasons.push("manifest 新声明");
        }
        if !object_upgrade.is_empty() {
            reasons.push("resource_hints/[interop]");
        }
        let reason = reasons.join(" + ");
        if sensitive.is_empty() && object_upgrade.is_empty() {
            info!(
                plugin = %record.id,
                from = version_from,
                to = %record.version,
                added = %added,
                "升级按 manifest 补齐授权（只读类）"
            );
        } else {
            warn!(
                plugin = %record.id,
                from = version_from,
                to = %record.version,
                added = %added,
                sensitive = %sensitive.join(","),
                reason = %reason,
                "升级补授授权"
            );
            let sensitive_note = if sensitive.is_empty() {
                String::new()
            } else {
                format!("；敏感: {}", sensitive.join(","))
            };
            db.audit(
                &opts.actor,
                "local",
                "plugin.grant_upgrade",
                &record.id,
                "ok",
                &format!(
                    "升级补授授权: {added}；来源: {reason}；版本 {version_from} → {}{sensitive_note}",
                    record.version
                ),
            )?;
        }
    }

    Ok(InstalledInfo {
        id: record.id,
        name: record.name,
        version: record.version,
        key_id: record.key_id,
        signature_verified: meta.signature_verified,
        grants,
        reduced_sandbox,
        warnings,
    })
}

/// 判断权限动词是否属于「写/删除/执行」档。
///
/// 只看动词最后一段（`containers.exec` → `exec`、`scheduler:manage` → `manage`）；
/// 只读动词（`list` / `read` / `stat` / `logs` 等）不在表内，升级补授时只记日志。
fn is_sensitive_perm(perm: &str) -> bool {
    const SENSITIVE: &[&str] = &[
        "write",
        "delete",
        "remove",
        "rm",
        "exec",
        "execute",
        "run",
        "start",
        "stop",
        "restart",
        "kill",
        "create",
        "update",
        "set",
        "put",
        "upload",
        "apply",
        "restore",
        "manage",
        "grant",
        "install",
        "uninstall",
        "reload",
        "truncate",
        "move",
        "rename",
        "chmod",
        "chown",
        "call",
    ];
    let verb = perm.rsplit_once(':').map(|(_, v)| v).unwrap_or(perm);
    let seg = verb.rsplit('.').next().unwrap_or(verb);
    SENSITIVE.contains(&seg)
}

fn prune_versions(root: &Path, keep: usize) -> Result<()> {
    let versions = root.join("versions");
    let mut dirs: Vec<(String, PathBuf)> = fs::read_dir(&versions)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    while dirs.len() > keep {
        let (name, path) = dirs.remove(0);
        warn!(version = %name, "删除超出 N-1 保留窗口的旧版本目录");
        remove_tree(&path)?;
    }
    Ok(())
}

/// 删除目录树。Windows 上杀毒/索引会短暂占用刚写入的文件，`remove_dir_all`
/// 可能返回 `ERROR_DIR_NOT_EMPTY`（不是真有残留）；重试几次再放弃。
/// Linux 上首次即成功，无额外开销。
fn remove_tree(path: &Path) -> Result<()> {
    let mut delay = std::time::Duration::from_millis(50);
    for attempt in 0..5 {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if attempt < 4 => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(e) => return Err(PluginError::Io(e)),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_plugin_ownership(root: &Path, id: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let uid = plugin_uid(id);
    // 插件数据目录归插件 UID 所有；versions/ 仅 root 可写（插件只读）。
    for sub in ["data", "cache", "run"] {
        let p = root.join(sub);
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
        if let Ok(c) = std::ffi::CString::new(p.to_string_lossy().as_bytes()) {
            // SAFETY: c 是合法 C 字符串；失败仅记录，不阻断安装（非 root 环境必然失败）。
            let rc = unsafe { libc::chown(c.as_ptr(), uid, uid) };
            if rc != 0 {
                warn!(path = %p.display(), "chown 插件数据目录失败（非 root 运行？）");
            }
        }
    }
    let _ = fs::set_permissions(root.join("versions"), fs::Permissions::from_mode(0o755));
    // 版本目录来自 staging（0700），且解包不落 tar 的 mode 位；不放开这两处，
    // 降权后的插件进程连自己目录都穿不过去，拉起时直接 Permission denied。
    if let Ok(entries) = fs::read_dir(root.join("versions")) {
        for entry in entries.flatten() {
            let version_dir = entry.path();
            if !version_dir.is_dir() {
                continue;
            }
            let _ = fs::set_permissions(&version_dir, fs::Permissions::from_mode(0o755));
            mark_tree_executable(&version_dir.join("bin"));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn mark_tree_executable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
                mark_tree_executable(&p);
            } else {
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
            }
        }
    }
}

#[cfg(not(unix))]
fn set_plugin_ownership(_root: &Path, _id: &str) -> Result<()> {
    Ok(())
}

/// 卸载：停进程 + 删版本目录；`data/` 默认保留（§4.5）。
pub fn uninstall_package(db: &Db, data_dir: &Path, id: &str, delete_data: bool) -> Result<()> {
    let root = plugin_root(data_dir, id);
    if !root.exists() {
        return Err(PluginError::Rejected(format!("插件目录不存在: {id}")));
    }
    for sub in ["versions", "cache", "run"] {
        let p = root.join(sub);
        if p.exists() {
            remove_tree(&p)?;
        }
    }
    if delete_data {
        let data = root.join("data");
        if data.exists() {
            remove_tree(&data)?;
        }
        let _ = remove_tree(&root);
    } else {
        fs::create_dir_all(root.join("versions"))?;
    }
    db.delete_plugin(id)?;
    db.audit(
        "system",
        "local",
        "plugin.uninstall",
        id,
        "ok",
        &format!("delete_data={delete_data}"),
    )?;
    Ok(())
}

/// 粗略 semver 比较（只比较 x.y.z 数值段；预发布标签不参与排序）。
pub fn version_gt(a: &str, b: &str) -> bool {
    fn parts(v: &str) -> [u64; 3] {
        let mut out = [0u64; 3];
        let core = v.split(['-', '+']).next().unwrap_or(v);
        for (i, seg) in core.split('.').take(3).enumerate() {
            out[i] = seg.parse().unwrap_or(0);
        }
        out
    }
    parts(a) > parts(b)
}

pub fn manifest_of(rec: &PluginRecord) -> std::result::Result<Manifest, PluginError> {
    Manifest::parse(&rec.manifest)
        .map_err(|e| PluginError::Rejected(format!("库内 manifest 不可解析: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(version_gt("0.2.0", "0.1.9"));
        assert!(version_gt("1.0.0", "0.99.99"));
        assert!(!version_gt("0.1.0", "0.1.0"));
        assert!(!version_gt("0.0.9", "0.1.0"));
        assert!(version_gt("1.0.0-beta.2", "0.9.0"));
    }

    #[cfg(unix)]
    #[test]
    fn shared_gid_is_safe_under_concurrency() {
        // 并发 spawn 会多线程同时取插件组 gid；旧实现用 getgrnam（musl 下共享静态
        // 缓冲区且无锁，并发调用会 double free 打坏堆元数据并打死内核进程）。
        let expect = shared_gid();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        assert_eq!(shared_gid(), expect);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn uid_is_stable_and_in_range() {
        let a = plugin_uid("com.example.demo");
        let b = plugin_uid("com.example.demo");
        let c = plugin_uid("com.example.other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!((200_000..201_000).contains(&a));
    }
}

#[cfg(test)]
mod migrate_restore_tests {
    use super::*;

    fn build_pkg(dir: &Path, id: &str, version: &str) -> PathBuf {
        use skiff_pkgfmt::SigningKey;
        let key = SigningKey::generate();
        let src = dir.join(format!("src-{version}"));
        let arch = skiff_pkgfmt::host_arch();
        std::fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
        std::fs::write(src.join("bin").join(&arch).join("plugin"), b"test-binary").unwrap();
        let manifest = format!(
            r#"id = "{id}"
name = "Demo"
version = "{version}"
key_id = "{key_id}"
api = "1.0"
requires_full_sandbox = false
requires = ["docker"]
[[permissions]]
capability = "docker"
verbs = ["list"]
"#,
            id = id,
            version = version,
            key_id = key.key_id()
        );
        std::fs::write(src.join("manifest.toml"), manifest).unwrap();
        let out = dir.join(format!("pkg-{version}.wbpkg"));
        skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
        out
    }

    /// 可声明额外权限与 `resource_hints` 的包（重装/升级授权用例）。
    fn build_pkg_with(
        dir: &Path,
        id: &str,
        version: &str,
        perms: &[&str],
        hints: &[&str],
    ) -> PathBuf {
        build_pkg_full(dir, id, version, perms, hints, &[])
    }

    /// 同上，另可声明 `[interop].consumes`（派生 `plugin:<id>` hint）。
    fn build_pkg_full(
        dir: &Path,
        id: &str,
        version: &str,
        perms: &[&str],
        hints: &[&str],
        consumes: &[&str],
    ) -> PathBuf {
        use skiff_pkgfmt::SigningKey;
        let key = SigningKey::generate();
        // 目录/文件名不能带 `:`（Windows 非法字符），权限串里的冒号替换掉。
        let tag = format!(
            "{version}-{}-{}",
            perms.join("_").replace(':', "-"),
            consumes.len()
        );
        let src = dir.join(format!("src-{tag}"));
        let arch = skiff_pkgfmt::host_arch();
        std::fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
        std::fs::write(src.join("bin").join(&arch).join("plugin"), b"test-binary").unwrap();
        let requires = if consumes.is_empty() {
            r#"["docker"]"#
        } else {
            r#"["docker", "interop"]"#
        };
        let mut manifest = format!(
            r#"id = "{id}"
name = "Demo"
version = "{version}"
key_id = "{key_id}"
api = "1.0"
requires_full_sandbox = false
requires = {requires}
"#,
            key_id = key.key_id()
        );
        if !hints.is_empty() {
            let list = hints
                .iter()
                .map(|h| format!("\"{h}\""))
                .collect::<Vec<_>>()
                .join(", ");
            manifest.push_str(&format!("resource_hints = [{list}]\n"));
        }
        for p in perms {
            let (cap, verb) = p.split_once(':').expect("权限串必须是 cap:verb");
            manifest.push_str(&format!(
                "[[permissions]]\ncapability = \"{cap}\"\nverbs = [\"{verb}\"]\n"
            ));
        }
        if !consumes.is_empty() {
            let list = consumes
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            manifest.push_str(&format!("[interop]\nconsumes = [{list}]\n"));
        }
        std::fs::write(src.join("manifest.toml"), manifest).unwrap();
        let out = dir.join(format!("pkg-{tag}.wbpkg"));
        skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
        out
    }

    /// 断言授权集恰好等于 `expected`（顺序无关：`grants()` 按 kind/value 排序）。
    fn grants_are(db: &Db, id: &str, expected: &[(&str, &str)]) {
        let got = db.grants(id).unwrap();
        for (kind, value) in expected {
            assert!(
                got.iter().any(|g| g.kind == *kind && g.value == *value),
                "缺少授权 {kind}:{value}，实际 {got:?}"
            );
        }
        assert_eq!(got.len(), expected.len(), "授权项数量不符: {got:?}");
    }

    fn opts() -> InstallOptions {
        InstallOptions {
            trusted_public_key: None,
            allow_unsigned: true,
            granted_permissions: vec!["docker:list".into()],
            object_grants: vec![],
            available_capabilities: skiff_pkgfmt::manifest::Capability::all().to_vec(),
            allow_reduced_sandbox: true,
            arch: skiff_pkgfmt::host_arch(),
            actor: "test".into(),
        }
    }

    fn installed_versions(root: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(root.join("versions"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }

    /// 升级保 data、版本目录只留 N-1 窗口（§4.5 迁移恢复口径）。
    #[test]
    fn upgrade_preserves_data_and_keeps_n_minus_1_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.1.0"), &opts()).unwrap();
        let root = plugin_root(data_dir, id);
        let user_data = root.join("data").join("user.json");
        std::fs::write(&user_data, b"{\"keep\":true}").unwrap();

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.2.0"), &opts()).unwrap();
        assert!(user_data.exists(), "升级不得清掉插件 data/");
        assert_eq!(installed_versions(&root), vec!["0.1.0", "0.2.0"]);
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.2.0");

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.3.0"), &opts()).unwrap();
        assert!(user_data.exists(), "二次升级 data/ 仍须保留");
        assert_eq!(
            installed_versions(&root),
            vec!["0.2.0", "0.3.0"],
            "只保留 N-1 窗口：最早的 0.1.0 应被清理"
        );
    }

    /// 同版本按重装处理（保留既有授权），降级仍必须拒。
    #[test]
    fn downgrade_is_rejected_but_same_version_reinstalls() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.2.0"), &opts()).unwrap();
        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.2.0"), &opts())
            .expect("同版本应允许重装");
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.2.0");
        let e = install_package(&db, data_dir, &build_pkg(data_dir, id, "0.1.0"), &opts())
            .unwrap_err()
            .to_string();
        assert!(e.contains("不接受降级"), "降级应拒: {e}");
    }

    /// 装 → 授权（最小子集 + 对象）→ 同版本重装：既有授权原样保留，不回到 manifest 默认值；
    /// 旧 manifest 已声明、但用户当时没勾的动词（inspect）不翻案；新出现的对象 hint 补授。
    #[test]
    fn reinstall_same_version_preserves_existing_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut first = opts();
        first.granted_permissions = vec!["docker:list".into()];
        first.object_grants = vec!["container:abc".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(
                data_dir,
                id,
                "0.1.0",
                &["docker:list", "docker:inspect"],
                &[],
            ),
            &first,
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[("object", "container:abc"), ("perm", "docker:list")],
        );

        // 重装时调用方按 manifest 默认值提交（含 inspect 与新 hint）：
        // 既有项原样保留，inspect 属旧 manifest 已声明、用户当时未勾，不能翻案；
        // `container:def` 是新出现的 hint，按补齐语义加入。
        let mut again = opts();
        again.granted_permissions = vec!["docker:list".into(), "docker:inspect".into()];
        again.object_grants = vec!["container:def".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(
                data_dir,
                id,
                "0.1.0",
                &["docker:list", "docker:inspect"],
                &["container:def"],
            ),
            &again,
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[
                ("object", "container:abc"),
                ("object", "container:def"),
                ("perm", "docker:list"),
            ],
        );
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.1.0");
    }

    /// 升级新声明的能力动词要补齐授权（既有项不清空）；新出现的对象 hint 同步补齐，
    /// 敏感档（写/删除/执行）额外落一条 `plugin.grant_upgrade` 审计。
    #[test]
    fn upgrade_grants_newly_declared_permissions_and_audits_sensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut first = opts();
        first.granted_permissions = vec!["docker:list".into()];
        first.object_grants = vec!["container:abc".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &[]),
            &first,
        )
        .unwrap();

        // 0.2.0 新声明 restart（敏感）与 inspect（只读）。
        let mut upgraded = opts();
        upgraded.granted_permissions = vec!["docker:list".into()];
        upgraded.object_grants = vec!["container:def".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(
                data_dir,
                id,
                "0.2.0",
                &["docker:list", "docker:restart", "docker:inspect"],
                &["container:def"],
            ),
            &upgraded,
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[
                ("object", "container:abc"),
                ("object", "container:def"),
                ("perm", "docker:list"),
                ("perm", "docker:restart"),
                ("perm", "docker:inspect"),
            ],
        );
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.2.0");

        let rows = db.recent_audit(20).unwrap();
        let row = rows
            .iter()
            .find(|r| r.action == "plugin.grant_upgrade")
            .expect("敏感新增授权必须留审计");
        assert_eq!(row.target, id);
        assert!(row.detail.contains("docker:restart"), "{}", row.detail);
        // 审计列出本次补齐的全部新增动词，并标注其中哪些属于敏感档。
        assert!(row.detail.contains("docker:inspect"), "{}", row.detail);
        assert!(
            row.detail.contains("敏感: docker:restart"),
            "{}",
            row.detail
        );
        assert!(row.detail.contains("0.1.0"), "{}", row.detail);
        assert!(row.detail.contains("0.2.0"), "{}", row.detail);
    }

    /// 只读类的新声明：授权补齐但不落敏感审计。
    #[test]
    fn upgrade_grants_read_only_declaration_without_sensitive_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &[]),
            &opts(),
        )
        .unwrap();
        install_package(
            &db,
            data_dir,
            &build_pkg_with(
                data_dir,
                id,
                "0.2.0",
                &["docker:list", "docker:inspect"],
                &[],
            ),
            &opts(),
        )
        .unwrap();

        grants_are(
            &db,
            id,
            &[("perm", "docker:list"), ("perm", "docker:inspect")],
        );
        let rows = db.recent_audit(20).unwrap();
        assert!(
            rows.iter().all(|r| r.action != "plugin.grant_upgrade"),
            "只读补齐不应落敏感审计: {rows:?}"
        );
    }

    /// 升级后 manifest 不再声明某能力时，旧授权仍按声明集过滤掉（补齐不等于只增不减）。
    #[test]
    fn upgrade_drops_grants_no_longer_declared() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut first = opts();
        first.granted_permissions = vec!["docker:list".into(), "docker:inspect".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(
                data_dir,
                id,
                "0.1.0",
                &["docker:list", "docker:inspect"],
                &[],
            ),
            &first,
        )
        .unwrap();
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.2.0", &["docker:list"], &[]),
            &opts(),
        )
        .unwrap();

        grants_are(&db, id, &[("perm", "docker:list")]);
    }

    /// 旧 grants 里没有任何对象授权 → 升级按新 manifest 的 `resource_hints` 补授，
    /// 审计写清执行者、来源与具体 hint。
    #[test]
    fn upgrade_grants_new_resource_hint_and_audits() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &[]),
            &opts(),
        )
        .unwrap();
        grants_are(&db, id, &[("perm", "docker:list")]);

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.2.0", &["docker:list"], &["container:def"]),
            &opts(),
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[("object", "container:def"), ("perm", "docker:list")],
        );

        let rows = db.recent_audit(20).unwrap();
        let row = rows
            .iter()
            .find(|r| r.action == "plugin.grant_upgrade")
            .expect("对象 hint 补授必须留审计");
        assert_eq!(row.target, id);
        assert_eq!(row.actor, "test");
        assert!(row.detail.contains("container:def"), "{}", row.detail);
        assert!(row.detail.contains("resource_hints"), "{}", row.detail);
        assert!(row.detail.contains("0.1.0"), "{}", row.detail);
        assert!(row.detail.contains("0.2.0"), "{}", row.detail);
    }

    /// 已有 hint 不重复授予，也不产生额外审计噪声。
    #[test]
    fn upgrade_keeps_existing_hint_without_duplicate_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut first = opts();
        first.object_grants = vec!["container:abc".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &["container:abc"]),
            &first,
        )
        .unwrap();

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.2.0", &["docker:list"], &["container:abc"]),
            &opts(),
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[("object", "container:abc"), ("perm", "docker:list")],
        );

        let rows = db.recent_audit(20).unwrap();
        assert!(
            rows.iter().all(|r| r.action != "plugin.grant_upgrade"),
            "没有新增项就不该有补授审计: {rows:?}"
        );
    }

    /// manifest 移除 hint 不自动回收：只增不减，回收只走显式卸载。
    #[test]
    fn upgrade_does_not_revoke_removed_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut first = opts();
        first.object_grants = vec!["container:abc".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &["container:abc"]),
            &first,
        )
        .unwrap();

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.2.0", &["docker:list"], &[]),
            &opts(),
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[("object", "container:abc"), ("perm", "docker:list")],
        );

        let rows = db.recent_audit(20).unwrap();
        assert!(
            rows.iter().all(|r| r.action != "plugin.grant_upgrade"),
            "移除 hint 不应产生任何补授或回收审计: {rows:?}"
        );
    }

    /// 旧 manifest 已声明、但用户安装时没勾的 hint 不翻案：只有「新增声明」才补授。
    #[test]
    fn upgrade_does_not_regrant_declined_old_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        // 0.1.0 已声明 container:abc，但用户没勾（object_grants 为空）。
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &["container:abc"]),
            &opts(),
        )
        .unwrap();
        grants_are(&db, id, &[("perm", "docker:list")]);

        // 0.2.0 仍然只声明同一条 hint：不是新增，不补授。
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.2.0", &["docker:list"], &["container:abc"]),
            &opts(),
        )
        .unwrap();
        grants_are(&db, id, &[("perm", "docker:list")]);

        let rows = db.recent_audit(20).unwrap();
        assert!(
            rows.iter().all(|r| r.action != "plugin.grant_upgrade"),
            "用户没勾的旧 hint 不该被翻案补授: {rows:?}"
        );
    }

    /// `[interop].consumes` 派生的 `plugin:<id>` 同属 hint：老版本没声明、升级补上后
    /// 跨插件调用不再 403（对应「先装 ftp、后装 firewall」的升级场景）。
    #[test]
    fn upgrade_grants_interop_hint_for_new_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.ftp";

        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &[]),
            &opts(),
        )
        .unwrap();
        grants_are(&db, id, &[("perm", "docker:list")]);

        // 0.2.0 首次声明 `[interop]`：`interop:call` 与 `plugin:<id>` 都属新增。
        install_package(
            &db,
            data_dir,
            &build_pkg_full(
                data_dir,
                id,
                "0.2.0",
                &["docker:list", "interop:call"],
                &[],
                &["com.skiff.firewall:protect"],
            ),
            &opts(),
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[
                ("object", "plugin:com.skiff.firewall"),
                ("perm", "docker:list"),
                ("perm", "interop:call"),
            ],
        );

        let rows = db.recent_audit(20).unwrap();
        let row = rows
            .iter()
            .find(|r| r.action == "plugin.grant_upgrade")
            .expect("跨插件 hint 补授必须留审计");
        assert!(
            row.detail.contains("plugin:com.skiff.firewall"),
            "{}",
            row.detail
        );
        assert!(row.detail.contains("interop"), "{}", row.detail);
    }

    /// 敏感档判定只看动词最后一段。
    #[test]
    fn sensitive_perm_classification() {
        for p in [
            "scheduler:manage",
            "docker:restart",
            "docker:containers.exec",
            "fs:write",
            "fs:delete",
            "interop:call",
        ] {
            assert!(is_sensitive_perm(p), "{p} 应判为敏感");
        }
        for p in [
            "docker:list",
            "docker:inspect",
            "fs:read",
            "identity:self",
            "docker:logs",
        ] {
            assert!(!is_sensitive_perm(p), "{p} 应判为只读");
        }
    }

    /// 只有管理员显式卸载才会清空授权；卸载保留 data/ 也不影响这条口径。
    #[test]
    fn explicit_uninstall_clears_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        let mut grants = opts();
        grants.granted_permissions = vec!["docker:list".into()];
        grants.object_grants = vec!["container:abc".into()];
        install_package(
            &db,
            data_dir,
            &build_pkg_with(data_dir, id, "0.1.0", &["docker:list"], &[]),
            &grants,
        )
        .unwrap();
        grants_are(
            &db,
            id,
            &[("object", "container:abc"), ("perm", "docker:list")],
        );

        uninstall_package(&db, data_dir, id, false).unwrap();
        assert!(db.plugin(id).unwrap().is_none(), "库记录应删除");
        assert!(
            db.grants(id).unwrap().is_empty(),
            "显式卸载必须清空授权: {:?}",
            db.grants(id).unwrap()
        );
    }

    /// 卸载默认保 data/（§4.5）；显式 delete_data 才清干净。
    #[test]
    fn uninstall_keeps_data_unless_delete_data() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.1.0"), &opts()).unwrap();
        let root = plugin_root(data_dir, id);
        let user_data = root.join("data").join("user.json");
        std::fs::write(&user_data, b"{}").unwrap();

        uninstall_package(&db, data_dir, id, false).unwrap();
        assert!(user_data.exists(), "默认卸载必须保留 data/");
        // 卸载会清掉版本内容（允许留下空的 versions/ 占位）
        assert!(
            !root.join("versions").exists()
                || std::fs::read_dir(root.join("versions"))
                    .unwrap()
                    .next()
                    .is_none(),
            "版本内容应清空"
        );
        assert!(db.plugin(id).unwrap().is_none(), "库记录应删除");

        uninstall_package(&db, data_dir, id, true).unwrap();
        assert!(!root.exists(), "delete_data 应清掉整个插件目录");
    }
}
