//! 安装 / 升级 / 卸载（§4.3、§4.5）。

use std::fs;
use std::path::{Path, PathBuf};

use skiff_pkgfmt::package::{verify_and_stage, VerifyOptions, DEFAULT_LIMITS};
use skiff_pkgfmt::Manifest;
use skiff_store::{Db, Grant, PluginRecord};
use tracing::{info, warn};

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

/// 插件目录布局（§5.2）：`versions/` 代码只读、`data/` 业务数据、`cache/`、`run/`。
pub fn plugin_root(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("plugins").join(id)
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

/// 插件运行组 `skiff-plugins` 的 gid；组不存在时返回 `None`（开发机手跑的情形）。
///
/// 插件的主 gid 是自己的独立 gid，**必须**把运行组作为附加组补上，否则：
/// - 进不去 `/run/skiff`（0711，靠其他位可穿越，但 socket 仍按组判权）；
/// - 打不开 `kernel.sock`（0660 root:skiff-plugins），所有能力调用一律 EACCES。
///
/// 这一条只在真机（unix）上生效，Windows 开发机上永远走 `None` 分支。
#[cfg(unix)]
pub fn shared_gid() -> Option<u32> {
    let name = std::ffi::CString::new("skiff-plugins").ok()?;
    // SAFETY: name 是合法 C 字符串；getgrnam 返回静态区指针，只读访问 gid。
    let gr = unsafe { libc::getgrnam(name.as_ptr()) };
    if gr.is_null() {
        return None;
    }
    Some(unsafe { (*gr).gr_gid })
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
    // 沙箱降级：landlock/cgroups 未实现，只有显式声明不需要完整沙箱才允许装
    let reduced_sandbox = !meta.manifest.requires_full_sandbox;
    if meta.manifest.requires_full_sandbox && !opts.allow_reduced_sandbox {
        return Err(PluginError::Rejected(
            "该插件要求完整沙箱（requires_full_sandbox = true），当前实现尚未提供 landlock/cgroups；\
             需 admin 显式开启 allow_reduced_sandbox 且插件声明 false 才可降级安装"
                .into(),
        ));
    }
    if meta.manifest.requires_full_sandbox && opts.allow_reduced_sandbox {
        warnings.push("沙箱降级：仅独立 UID + 文件权限，UI 需持续标红".into());
    }
    if !meta.signature_verified {
        warnings.push("包未经验签（allow_unsigned）：仅限开发调试，生产环境必须导入公钥".into());
    }

    // 版本策略：低于或等于已装版本默认拒绝（灾难恢复入口不在此实现）
    if let Some(existing) = db.plugin(&meta.manifest.id)? {
        if !version_gt(&meta.manifest.version, &existing.version) {
            return Err(PluginError::Rejected(format!(
                "已安装版本 {} 不低于包内版本 {}；升级只接受更高版本（灾难恢复入口另设）",
                existing.version, meta.manifest.version
            )));
        }
        if existing.key_id != meta.manifest.key_id && !opts.allow_unsigned {
            return Err(PluginError::Rejected(
                "升级包签名公钥与已装版本不一致：换密钥视为新信任，需重新确认指纹".into(),
            ));
        }
    }

    // 权限确认：只授予用户确认过的子集，且必须是 manifest 声明过的
    let declared = meta.manifest.permission_strings();
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
        if !(o.starts_with("container:") || o.starts_with("project:")) {
            return Err(PluginError::Rejected(format!("对象 grant 形式非法: {o}")));
        }
        grants.push(Grant::object(o.clone()));
    }

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
    let version_dir = root.join("versions").join(&meta.manifest.version);
    if version_dir.exists() {
        fs::remove_dir_all(&version_dir)?;
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
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_plugin_ownership(root: &Path, id: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let uid = plugin_uid(id) as u32;
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
    Ok(())
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
            fs::remove_dir_all(&p)?;
        }
    }
    if delete_data {
        let data = root.join("data");
        if data.exists() {
            fs::remove_dir_all(&data)?;
        }
        let _ = fs::remove_dir_all(&root);
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
