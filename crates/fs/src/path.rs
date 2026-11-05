//! 路径规范化、授权判定与站点归属（契约 §2.2、§3.1、§8.1）。
//!
//! 三条硬规则：
//! 1. **形式**：只接受绝对路径；`..`、NUL、空串、非 UTF-8 组件一律拒绝；
//!    重复分隔符与 `.` 段被 lexical 归一（不触发任何系统调用）。
//! 2. **授权**：插件目标必须落在某个授权根内；根外一律 `fs.escape`。
//! 3. **符号链接永不跟随**：父目录链中出现符号链接即拒绝（防逃逸与 TOCTOU 预检），
//!    最后一段是否允许是链接本身由 [`PathMode`] 决定。

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::{io_error, FsError};
use crate::subject::Principal;

/// 站点根默认值（见 `docs/SITE_RUNTIME_CONTRACT.md`）。
pub const DEFAULT_SITE_ROOT: &str = "/www/sites";

/// 受保护子目录：插件默认只读，写入需 `fs:write-protected` 单独勾选。
pub const PROTECTED_SUBDIRS: [&str; 3] = ["log", "ssl", "conf"];

/// 路径用途决定「最后一段」的校验方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// 必须存在；最后一段是符号链接则拒绝（读/写/列目录）。
    Existing,
    /// 必须存在；最后一段可以是符号链接（stat/remove/rename 源）。
    ExistingLink,
    /// 可以不存在；已存在时最后一段是符号链接则拒绝（写新文件/建目录）。
    Create,
    /// 可以不存在；已存在时按链接本身处理（改名/复制目标）。
    CreateLink,
}

impl PathMode {
    pub fn must_exist(self) -> bool {
        matches!(self, PathMode::Existing | PathMode::ExistingLink)
    }

    pub fn allow_final_symlink(self) -> bool {
        matches!(self, PathMode::ExistingLink | PathMode::CreateLink)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Dir,
    Symlink,
    Other,
    Missing,
}

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    /// lexical 归一后的绝对路径（锁 key、审计路径都用它）。
    pub path: PathBuf,
    pub kind: PathKind,
    /// 命中的授权根；管理员为 `None`。
    pub grant_root: Option<PathBuf>,
    /// 路径命中的站点域名（`/www/sites/<domain>/...`）。
    pub site: Option<String>,
    /// 是否落在受保护子目录（`log/ssl/conf`）。
    pub protected: bool,
}

/// lexical 规范化：不触碰文件系统，因此不会跟随任何符号链接。
pub fn normalize_absolute(raw: &str) -> Result<PathBuf, FsError> {
    if raw.is_empty() {
        return Err(FsError::Denied("路径不能为空".into()));
    }
    if raw.contains('\0') {
        return Err(FsError::Denied("路径含 NUL".into()));
    }
    let input = Path::new(raw);
    if !input.is_absolute() {
        return Err(FsError::Denied("路径必须是绝对路径".into()));
    }

    let mut prefix = None;
    let mut absolute = false;
    let mut normals: Vec<&std::ffi::OsStr> = Vec::new();
    for c in input.components() {
        match c {
            Component::Prefix(p) => prefix = Some(p.as_os_str()),
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::Denied("路径不得含 ..".into()));
            }
            Component::Normal(n) => {
                if n.to_str().is_none() {
                    return Err(FsError::Denied("路径含非 UTF-8 组件".into()));
                }
                normals.push(n);
            }
        }
    }
    if !absolute {
        return Err(FsError::Denied("路径必须是绝对路径".into()));
    }

    let mut out = PathBuf::new();
    if let Some(p) = prefix {
        out.push(p);
    }
    out.push(std::path::MAIN_SEPARATOR.to_string());
    for n in normals {
        out.push(n);
    }
    Ok(simplify_windows(out))
}

/// 对给定路径做授权与「不跟随符号链接」校验，返回可操作的真实路径。
///
/// 注意：本函数只做**预检**；实际打开/写入在 `ops` 里用 `O_NOFOLLOW`（Unix）
/// 或 `symlink_metadata` 再校验（Windows）收口。残留 TOCTOU 窗口与根治方案
/// 见 `docs/KERNEL_FS_IMPL_NOTES.md`。
pub fn resolve(
    principal: &Principal,
    raw: &str,
    mode: PathMode,
    site_root: &str,
) -> Result<ResolvedPath, FsError> {
    let path = normalize_absolute(raw)?;
    let grant_root = principal.root_of(&path).cloned();

    if !principal.subject().is_admin() && grant_root.is_none() {
        return Err(if principal.has_host_grants() {
            FsError::Escape("路径不在已授权的 path: 根内".into())
        } else {
            FsError::Denied("没有 path: 宿主路径授权".into())
        });
    }

    // 父目录链：插件必须逐段确认「真实目录、非符号链接」。管理员不受根限制，
    // 不做链式检查（没有可逃逸的授权边界），但仍不跟随最后一段链接。
    if let Some(root) = grant_root.as_deref() {
        check_ancestors(&path, root, mode)?;
    }

    let kind = match fs::symlink_metadata(&path) {
        Ok(meta) => {
            let ft = meta.file_type();
            let kind = if ft.is_symlink() {
                PathKind::Symlink
            } else if ft.is_dir() {
                PathKind::Dir
            } else if ft.is_file() {
                PathKind::File
            } else {
                PathKind::Other
            };
            if kind == PathKind::Symlink && !mode.allow_final_symlink() {
                return Err(FsError::Escape(format!(
                    "不跟随符号链接: {}",
                    path.display()
                )));
            }
            kind
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if mode.must_exist() {
                return Err(FsError::NotFound(format!("{} 不存在", path.display())));
            }
            PathKind::Missing
        }
        Err(e) => return Err(io_error(&path, e)),
    };

    let site = site_of(&path, site_root);
    let protected = protected_subdir(&path, site_root).is_some();
    Ok(ResolvedPath {
        path,
        kind,
        grant_root,
        site,
        protected,
    })
}

/// 父目录链逐段检查（从授权根开始）。
fn check_ancestors(path: &Path, root: &Path, mode: PathMode) -> Result<(), FsError> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| FsError::Escape("路径不在已授权的 path: 根内".into()))?;
    let comps: Vec<Component<'_>> = rel.components().collect();
    if comps.is_empty() {
        // 目标就是授权根本身：根在构造时已 canonical，且必须是真实目录。
        return Ok(());
    }
    let mut cur = root.to_path_buf();
    for c in &comps[..comps.len() - 1] {
        let Component::Normal(name) = c else {
            // normalize 已排除 RootDir/CurDir/ParentDir
            return Err(FsError::Denied("路径形式非法".into()));
        };
        cur.push(name);
        match fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(FsError::Escape(format!(
                    "父目录是符号链接: {}",
                    cur.display()
                )));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(FsError::Conflict(format!(
                    "路径组件不是目录: {}",
                    cur.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(FsError::NotFound(format!("{} 不存在", cur.display())));
            }
            Err(e) => return Err(io_error(&cur, e)),
        }
    }
    // 父目录必须可写（创建类操作早失败，避免先写后错）。
    if matches!(mode, PathMode::Create | PathMode::CreateLink) {
        let parent = path
            .parent()
            .ok_or_else(|| FsError::Denied("路径无父目录".into()))?;
        let parent_meta = fs::metadata(parent).map_err(|e| io_error(parent, e))?;
        if !parent_meta.is_dir() {
            return Err(FsError::Conflict("父路径不是目录".into()));
        }
    }
    Ok(())
}

/// 从路径推导站点域名：`<site_root>/<domain>/...`。
pub fn site_of(path: &Path, site_root: &str) -> Option<String> {
    let rest = path.strip_prefix(Path::new(site_root)).ok()?;
    let mut comps = rest.components();
    match comps.next()? {
        Component::Normal(domain) => domain.to_str().map(str::to_string),
        _ => None,
    }
}

/// 命中受保护子目录时返回子目录名。
pub fn protected_subdir<'a>(path: &'a Path, site_root: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(Path::new(site_root)).ok()?;
    let mut comps = rest.components();
    comps.next()?; // 域名段
    match comps.next()? {
        Component::Normal(sub) => PROTECTED_SUBDIRS
            .into_iter()
            .find(|p| sub.to_str().is_some_and(|s| s == *p)),
        _ => None,
    }
}

/// 去掉 Windows canonicalize 产生的 `\\?\` 前缀，让「授权根」与「lexical 路径」
/// 能在同一坐标系里比较（否则 `starts_with` 永远为假）。
/// 非 Windows 平台不做任何事。
#[allow(unused_mut)]
pub(crate) fn simplify_windows(mut p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy().to_string();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            p = if let Some(unc) = rest.strip_prefix("UNC\\") {
                PathBuf::from(format!(r"\\{unc}"))
            } else {
                PathBuf::from(rest)
            };
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_dotdot_nul_and_empty() {
        assert!(matches!(normalize_absolute(""), Err(FsError::Denied(_))));
        assert!(matches!(
            normalize_absolute("relative/x"),
            Err(FsError::Denied(_))
        ));
        assert!(matches!(
            normalize_absolute("/etc/../passwd"),
            Err(FsError::Denied(_))
        ));
        assert!(matches!(
            normalize_absolute("/etc/\0passwd"),
            Err(FsError::Denied(_))
        ));
    }

    #[test]
    fn collapses_duplicate_separators_and_dot_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let root = simplify_windows(std::fs::canonicalize(tmp.path()).unwrap());
        let raw = format!(
            "{}{}{}",
            root.display(),
            std::path::MAIN_SEPARATOR,
            "a//./b"
        );
        let got = normalize_absolute(&raw).unwrap();
        assert_eq!(got, root.join("a").join("b"));
    }

    #[test]
    fn site_and_protected_paths_are_derived() {
        let p = Path::new("/www/sites/example.com/index/a.txt");
        assert_eq!(
            site_of(p, DEFAULT_SITE_ROOT).as_deref(),
            Some("example.com")
        );
        assert!(protected_subdir(p, DEFAULT_SITE_ROOT).is_none());
        for sub in ["log", "ssl", "conf"] {
            let p = PathBuf::from(format!("/www/sites/example.com/{sub}/x"));
            assert_eq!(protected_subdir(&p, DEFAULT_SITE_ROOT), Some(sub));
        }
        assert_eq!(
            site_of(Path::new("/www/sites/example.com"), DEFAULT_SITE_ROOT).as_deref(),
            Some("example.com")
        );
        assert!(site_of(Path::new("/etc/passwd"), DEFAULT_SITE_ROOT).is_none());
    }
}
