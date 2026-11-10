//! 随包官方插件同步（升级安装补齐插件代码）。
//!
//! `install.sh` 只把发布包 `official/*.wbpkg` 复制到 `<data_dir>/official/`，不负责
//! 把它们装进已安装插件；早期升级因此只换了内核二进制，插件 UI/代码仍停在旧版本。
//!
//! 这里在启动早期对 `official/` 做一次一致性比对：**只处理库内已存在的插件**，
//! 版本更高或同版本内容不同才调用 `install_package` 重装。复用安装器的既有语义：
//! `data/` 不动、沿用管理员既有授权，并按新 manifest 补齐新增声明项（敏感档写
//! `plugin.grant_upgrade` 审计）；停用状态原样保留。
//! 单个包失败只告警并继续，不阻断面板启动。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use skiff_pkgfmt::package::{PackageMeta, FILES_NAME};
use skiff_store::Db;
use tracing::{info, warn};

use crate::install::{install_options, install_package, plugin_root, version_gt, InstallOptions};

/// 一次同步的结果（调用方主要用于日志汇总与回归断言）。
#[derive(Debug, Default)]
pub struct OfficialSyncReport {
    /// 已更新（升级或同版本内容替换）的插件 id
    pub updated: Vec<String>,
    /// 版本与内容都一致的插件 id
    pub unchanged: Vec<String>,
    /// 有意跳过的项（未安装、包版本低于已装版本等）
    pub skipped: Vec<String>,
    /// 失败项：`(包路径, 错误说明)`
    pub failed: Vec<(String, String)>,
}

/// 同步 `<data_dir>/official/` 下的官方插件包。
///
/// 返回报告而不返回错误：任何单包问题都只记入 `failed`，由调用方决定如何呈现；
/// 启动路径必须继续。
pub fn sync_official_plugins(
    db: &Db,
    data_dir: &Path,
    allow_unsigned: bool,
    form: skiff_model::Form,
    allow_reduced_sandbox: bool,
) -> OfficialSyncReport {
    let mut report = OfficialSyncReport::default();
    let official = data_dir.join("official");
    if !official.is_dir() {
        return report;
    }

    let mut packages: Vec<PathBuf> = match fs::read_dir(&official) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("wbpkg"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(e) => {
            warn!(
                dir = %official.display(),
                error = %e,
                "读取官方插件目录失败，跳过随包同步"
            );
            return report;
        }
    };
    packages.sort();

    for package in packages {
        match sync_one(
            db,
            data_dir,
            &package,
            allow_unsigned,
            form,
            allow_reduced_sandbox,
        ) {
            Ok(SyncOutcome::Updated {
                id,
                from,
                to,
                same_version,
            }) => {
                if same_version {
                    info!(plugin = %id, version = %to, "官方插件内容已随发布包更新（同版本重装）");
                } else {
                    info!(plugin = %id, from = %from, to = %to, "官方插件已随发布包升级");
                }
                report.updated.push(id);
            }
            Ok(SyncOutcome::Unchanged(id)) => report.unchanged.push(id),
            Ok(SyncOutcome::Skipped(reason)) => report.skipped.push(reason),
            Err(e) => {
                warn!(
                    package = %package.display(),
                    error = %e,
                    "官方插件同步失败（不阻断启动）；可人工重试: skiff plugin install \"{}\"",
                    package.display()
                );
                report.failed.push((package.display().to_string(), e));
            }
        }
    }

    if !report.failed.is_empty() {
        warn!(
            failed = report.failed.len(),
            "官方插件同步存在失败项；面板继续启动。修好包后可执行 \
             `skiff plugin install <official/...>` 或在面板「插件 → 安装」重试\
             （若发布包更换了签名密钥，需按新指纹重新确认）"
        );
    }
    report
}

enum SyncOutcome {
    Updated {
        id: String,
        from: String,
        to: String,
        same_version: bool,
    },
    Unchanged(String),
    Skipped(String),
}

fn sync_one(
    db: &Db,
    data_dir: &Path,
    package: &Path,
    allow_unsigned: bool,
    form: skiff_model::Form,
    allow_reduced_sandbox: bool,
) -> std::result::Result<SyncOutcome, String> {
    let trust = crate::trust::resolve(package, db, allow_unsigned).map_err(|e| e.to_string())?;
    let meta = &trust.meta;
    let id = meta.manifest.id.clone();
    let version = meta.manifest.version.clone();

    // 未安装的官方包不自动安装：首次安装必须逐条确认授权，这个门槛不能由启动流程绕过。
    let Some(prev) = db.plugin(&id).map_err(|e| e.to_string())? else {
        return Ok(SyncOutcome::Skipped(format!("{id} 未安装，跳过")));
    };
    if !version_gt(&version, &prev.version) && version != prev.version {
        warn!(
            plugin = %id,
            installed = %prev.version,
            package = %version,
            "官方插件包版本低于已装版本，跳过（不接受降级）"
        );
        return Ok(SyncOutcome::Skipped(format!(
            "{id} 已装 {}，包内 {}，跳过降级",
            prev.version, version
        )));
    }
    if version == prev.version && installed_version_matches(data_dir, meta) {
        return Ok(SyncOutcome::Unchanged(id));
    }

    let opts: InstallOptions = install_options(
        &trust,
        Vec::new(),
        Vec::new(),
        allow_unsigned,
        form,
        allow_reduced_sandbox,
        "official-sync",
    );
    let info = install_package(db, data_dir, package, &opts).map_err(|e| e.to_string())?;

    // 安装器把记录重置为 installed/enabled=true；官方同步是后台行为，不得把管理员
    // 显式停用的插件重新启用。
    if !prev.enabled {
        db.set_plugin_enabled(&info.id, false)
            .map_err(|e| e.to_string())?;
        db.set_plugin_status(&info.id, "disabled", "")
            .map_err(|e| e.to_string())?;
    }

    Ok(SyncOutcome::Updated {
        same_version: prev.version == info.version,
        id: info.id,
        from: prev.version,
        to: info.version,
    })
}

/// 已装版本目录内容是否与包完全一致（文件集合 + 每个文件的 SHA-256）。
///
/// 包内 `files.sha256` / `sig` 是元数据，不落版本目录，比较时排除。
fn installed_version_matches(data_dir: &Path, meta: &PackageMeta) -> bool {
    let dir = plugin_root(data_dir, &meta.manifest.id)
        .join("versions")
        .join(&meta.manifest.version);
    if !dir.is_dir() {
        return false;
    }

    let expected: BTreeSet<&str> = meta
        .files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| *p != FILES_NAME)
        .collect();
    let mut actual = BTreeSet::new();
    if collect_files(&dir, &dir, &mut actual).is_err() {
        return false;
    }
    if actual.len() != expected.len() {
        return false;
    }
    for path in &actual {
        if !expected.contains(path.as_str()) {
            return false;
        }
    }
    for f in &meta.files {
        if f.path == FILES_NAME {
            continue;
        }
        if !file_hash_matches(&dir, &f.path, &f.sha256) {
            return false;
        }
    }
    true
}

fn collect_files(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(rel);
            }
        }
    }
    Ok(())
}

fn file_hash_matches(root: &Path, rel: &str, expected: &str) -> bool {
    let path = rel
        .split('/')
        .fold(root.to_path_buf(), |p, seg| p.join(seg));
    let Ok(bytes) = fs::read(&path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    hex::encode(hasher.finalize()) == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::{Form, Grant};
    use skiff_pkgfmt::SigningKey;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PKG_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn build_pkg(dir: &Path, id: &str, version: &str, ui: &str) -> PathBuf {
        let key = SigningKey::generate();
        let seq = PKG_SEQ.fetch_add(1, Ordering::Relaxed);
        let src = dir.join(format!("src-{seq}"));
        let arch = skiff_pkgfmt::host_arch();
        fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
        fs::write(src.join("bin").join(&arch).join("plugin"), b"demo").unwrap();
        fs::create_dir_all(src.join("ui")).unwrap();
        fs::write(src.join("ui").join("app.js"), ui.as_bytes()).unwrap();
        let manifest = format!(
            r#"id = "{id}"
name = "Demo"
version = "{version}"
key_id = "{}"
api = "1.0"
requires_full_sandbox = false
requires = ["docker"]

[[permissions]]
capability = "docker"
verbs = ["list"]
"#,
            key.key_id()
        );
        fs::write(src.join("manifest.toml"), manifest).unwrap();
        let out = dir.join(format!("{id}-{version}-{seq}.wbpkg"));
        skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
        out
    }

    fn install_opts() -> InstallOptions {
        InstallOptions {
            trusted_public_key: None,
            allow_unsigned: true,
            granted_permissions: vec!["docker:list".into()],
            object_grants: vec!["container:abc".into()],
            available_capabilities: skiff_pkgfmt::manifest::Capability::all().to_vec(),
            allow_reduced_sandbox: true,
            arch: skiff_pkgfmt::host_arch(),
            actor: "test".into(),
        }
    }

    fn grants(db: &Db, id: &str) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = db
            .grants(id)
            .unwrap()
            .into_iter()
            .map(|g: Grant| (g.kind, g.value))
            .collect();
        v.sort();
        v
    }

    fn put_official(official: &Path, pkg: &Path) -> PathBuf {
        fs::create_dir_all(official).unwrap();
        let dest = official.join(pkg.file_name().unwrap());
        fs::copy(pkg, &dest).unwrap();
        dest
    }

    /// 同版本但 UI 内容不同：同步必须替换版本目录，且 data/ 与既有授权原样保留。
    #[test]
    fn same_version_content_update_keeps_data_and_grants() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let old = build_pkg(data, id, "0.1.0", "old-ui");
        let installed = put_official(&official, &old);
        install_package(&db, data, &installed, &install_opts()).unwrap();

        let root = plugin_root(data, id);
        fs::write(root.join("data").join("user.json"), b"{\"keep\":true}").unwrap();
        let before = grants(&db, id);
        assert_eq!(before.len(), 2);

        let new = build_pkg(data, id, "0.1.0", "new-ui");
        let dest = official.join(installed.file_name().unwrap());
        fs::copy(&new, &dest).unwrap();

        let report = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(report.updated, vec![id.to_string()], "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");

        let ui = fs::read_to_string(root.join("versions/0.1.0/ui/app.js")).unwrap();
        assert_eq!(ui, "new-ui");
        assert_eq!(
            fs::read_to_string(root.join("data").join("user.json")).unwrap(),
            "{\"keep\":true}"
        );
        assert_eq!(grants(&db, id), before, "同步不得改变既有授权");
        assert!(db.plugin(id).unwrap().unwrap().enabled);
    }

    /// 幂等：第二次启动内容一致时不重装、不产生新版本目录。
    #[test]
    fn sync_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let pkg = build_pkg(data, id, "0.1.0", "ui");
        let installed = put_official(&official, &pkg);
        install_package(&db, data, &installed, &install_opts()).unwrap();

        let first = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(first.unchanged, vec![id.to_string()], "{first:?}");
        assert!(first.updated.is_empty(), "{first:?}");

        let second = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(second.unchanged, vec![id.to_string()], "{second:?}");
        assert!(second.updated.is_empty(), "{second:?}");
    }

    /// 未安装的不自动装；单个坏包只记录失败，不影响其他包的同步与返回。
    #[test]
    fn uninstalled_skipped_and_bad_package_is_isolated() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let old = build_pkg(data, id, "0.1.0", "old");
        let installed = put_official(&official, &old);
        install_package(&db, data, &installed, &install_opts()).unwrap();

        // 另一个未安装的官方包
        let other = build_pkg(data, "com.example.other", "0.1.0", "ui");
        put_official(&official, &other);

        // 坏包：不是合法的 wbpkg
        fs::create_dir_all(&official).unwrap();
        fs::write(
            official.join("com.skiff.broken-0.1.0.wbpkg"),
            b"not a package",
        )
        .unwrap();

        let new = build_pkg(data, id, "0.1.0", "new-content");
        let dest = official.join(installed.file_name().unwrap());
        fs::copy(&new, &dest).unwrap();

        let report = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(report.updated, vec![id.to_string()], "{report:?}");
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert!(report
            .skipped
            .iter()
            .any(|s| s.contains("com.example.other")));
        assert!(
            db.plugin("com.example.other").unwrap().is_none(),
            "不得自动安装"
        );
    }

    /// 升级路径：official/ 里同时有新老包时，装到高版本；随后老包按降级规则跳过。
    #[test]
    fn higher_version_updates_then_old_package_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let old = build_pkg(data, id, "0.1.0", "old-ui");
        let installed = put_official(&official, &old);
        install_package(&db, data, &installed, &install_opts()).unwrap();
        let root = plugin_root(data, id);
        fs::write(root.join("data").join("user.json"), b"keep").unwrap();
        let before = grants(&db, id);

        let new = build_pkg(data, id, "0.2.0", "new-ui");
        put_official(&official, &new);

        let report = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(report.updated, vec![id.to_string()], "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.2.0");
        assert_eq!(
            fs::read_to_string(root.join("versions/0.2.0/ui/app.js")).unwrap(),
            "new-ui"
        );
        assert_eq!(
            fs::read_to_string(root.join("data/user.json")).unwrap(),
            "keep"
        );
        assert_eq!(grants(&db, id), before);
    }

    /// 老包不能被当成「内容不同」而回滚到低版本。
    #[test]
    fn downgrade_package_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let new = build_pkg(data, id, "0.2.0", "ui");
        let installed = put_official(&official, &new);
        install_package(&db, data, &installed, &install_opts()).unwrap();

        let old = build_pkg(data, id, "0.1.0", "old-ui");
        put_official(&official, &old);

        let report = sync_official_plugins(&db, data, true, Form::Native, true);
        assert!(report.updated.is_empty(), "{report:?}");
        assert!(
            report.skipped.iter().any(|s| s.contains("跳过降级")),
            "{report:?}"
        );
        assert_eq!(db.plugin(id).unwrap().unwrap().version, "0.2.0");
    }

    /// 已停用的插件同步代码后仍保持停用（enabled=false / status=disabled）。
    #[test]
    fn sync_preserves_disabled_state() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        let db = Db::open_in_memory().unwrap();
        let id = "com.example.official";
        let official = data.join("official");

        let old = build_pkg(data, id, "0.1.0", "old");
        let installed = put_official(&official, &old);
        install_package(&db, data, &installed, &install_opts()).unwrap();
        db.set_plugin_enabled(id, false).unwrap();
        db.set_plugin_status(id, "disabled", "").unwrap();

        let new = build_pkg(data, id, "0.1.0", "new");
        let dest = official.join(installed.file_name().unwrap());
        fs::copy(&new, &dest).unwrap();

        let report = sync_official_plugins(&db, data, true, Form::Native, true);
        assert_eq!(report.updated, vec![id.to_string()], "{report:?}");
        let rec = db.plugin(id).unwrap().unwrap();
        assert!(!rec.enabled, "同步不得重新启用已停用插件");
        assert_eq!(rec.status, "disabled");
    }
}
