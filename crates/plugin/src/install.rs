//! 安装 / 升级 / 卸载（§4.3、§4.5）。

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
) -> InstallOptions {
    InstallOptions {
        trusted_public_key: trust.trusted_key.clone(),
        allow_unsigned,
        granted_permissions,
        object_grants,
        available_capabilities: skiff_pkgfmt::manifest::Capability::available_in(form),
        allow_reduced_sandbox,
        arch: skiff_pkgfmt::host_arch(),
    }
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
        // 对象 grant 的三形态与 manifest `resource_hints` 共用同一份形式校验：
        // `path:` 只做形式校验（绝对路径 / 无 .. / 非 `/`），存在性由内核每次调用
        // canonicalize 判定——允许「先装插件、后建目录」。
        skiff_pkgfmt::manifest::validate_resource(o)
            .map_err(|e| PluginError::Rejected(format!("对象 grant {o} 非法: {e}")))?;
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

    fn opts() -> InstallOptions {
        InstallOptions {
            trusted_public_key: None,
            allow_unsigned: true,
            granted_permissions: vec!["docker:list".into()],
            object_grants: vec![],
            available_capabilities: skiff_pkgfmt::manifest::Capability::all().to_vec(),
            allow_reduced_sandbox: true,
            arch: skiff_pkgfmt::host_arch(),
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

    /// 同版与降级必须拒（灾难恢复入口另设，不在安装路径里偷偷降级）。
    #[test]
    fn downgrade_and_same_version_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.demo";

        install_package(&db, data_dir, &build_pkg(data_dir, id, "0.2.0"), &opts()).unwrap();
        let e = install_package(&db, data_dir, &build_pkg(data_dir, id, "0.2.0"), &opts())
            .unwrap_err()
            .to_string();
        assert!(e.contains("升级只接受更高版本"), "同版应拒: {e}");
        let e = install_package(&db, data_dir, &build_pkg(data_dir, id, "0.1.0"), &opts())
            .unwrap_err()
            .to_string();
        assert!(e.contains("升级只接受更高版本"), "降级应拒: {e}");
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
