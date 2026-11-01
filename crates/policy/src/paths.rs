//! 宿主路径与卷校验（§5.5）。
//!
//! Docker Engine 只认**宿主路径**。面板代用户创建挂载时，授权与下传必须同一种语义：
//! - 目标存在：`canonicalize` 后必须仍在授权根内（符号链接逃逸因此被拒）；
//! - 目标不存在：规范化最近的已存在祖先，拒绝 `..` 与非目录组件，再按相对安全路径创建；
//! - 授权根及其组件须由管理员（root 或 skiff 进程属主）管理、不可被插件重命名/替换（unix 下检查 uid/权限位）。
//!
//! 已知局限（写进 §12 待测）：检查与使用之间仍有窗口。`open_dir_nofollow`（openat +
//! O_NOFOLLOW 逐段走）已实现但**未接入**判定链——`canonicalize` 只能保证「校验这一刻」
//! 没有符号链接逃逸；真正关窗口要把 fd 语义交给 Docker，或换成 `openat2(RESOLVE_BENEATH)`
//! （≥5.6）后配合巡检。对外不得声称已消除 TOCTOU。

use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    NotAbsolute(String),
    ParentTraversal(String),
    OutsideRoots(String),
    NotADirectory(String),
    Io(String),
    /// 授权根本身不安全（非管理员所有 / 组或其他人可写）
    UnsafeRoot(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::NotAbsolute(p) => write!(f, "路径必须是绝对路径: {p}"),
            PathError::ParentTraversal(p) => write!(f, "路径不允许包含 ..: {p}"),
            PathError::OutsideRoots(p) => write!(f, "路径不在任何授权根内: {p}"),
            PathError::NotADirectory(p) => write!(f, "不是目录: {p}"),
            PathError::Io(m) => write!(f, "io: {m}"),
            PathError::UnsafeRoot(p) => {
                write!(
                    f,
                    "授权根不安全（须 root/进程属主拥有且组/其他人不可写）: {p}"
                )
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PathGuard {
    roots: Vec<PathBuf>,
}

fn normalize_display(p: &Path) -> PathBuf {
    // Windows 的 canonicalize 会加 \\?\ 前缀；Docker 只在 unix 上用得到，这里仅统一比较口径。
    let s = p.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    p.to_path_buf()
}

impl PathGuard {
    pub fn new(roots: Vec<PathBuf>) -> Result<Self, PathError> {
        let mut normalized = Vec::new();
        for root in roots {
            if !root.is_absolute() {
                return Err(PathError::NotAbsolute(root.display().to_string()));
            }
            let canon = fs::canonicalize(&root)
                .map_err(|e| PathError::Io(format!("授权根不可解析 {}: {e}", root.display())))?;
            let canon = normalize_display(&canon);
            let meta = fs::metadata(&canon).map_err(|e| PathError::Io(e.to_string()))?;
            if !meta.is_dir() {
                return Err(PathError::NotADirectory(canon.display().to_string()));
            }
            check_root_safety(&canon)?;
            normalized.push(canon);
        }
        Ok(Self { roots: normalized })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    fn within_roots(&self, canon: &Path) -> bool {
        self.roots.iter().any(|r| canon.starts_with(r))
    }

    /// 校验并返回可下传给 Engine 的宿主路径。
    pub fn validate_bind_source(&self, source: &Path) -> Result<PathBuf, PathError> {
        if !source.is_absolute() {
            return Err(PathError::NotAbsolute(source.display().to_string()));
        }
        if self.roots.is_empty() {
            return Err(PathError::OutsideRoots(source.display().to_string()));
        }
        reject_parent_components(source)?;

        match fs::canonicalize(source) {
            Ok(canon) => {
                let canon = normalize_display(&canon);
                if !self.within_roots(&canon) {
                    return Err(PathError::OutsideRoots(canon.display().to_string()));
                }
                Ok(canon)
            }
            Err(_) => {
                // 目标不存在：找到最近的存在祖先，规范化后校验，再拼回剩余组件。
                let mut existing = source.to_path_buf();
                let mut tail: Vec<std::ffi::OsString> = Vec::new();
                loop {
                    if existing.exists() {
                        break;
                    }
                    let Some(name) = existing.file_name().map(|s| s.to_os_string()) else {
                        return Err(PathError::OutsideRoots(source.display().to_string()));
                    };
                    tail.push(name);
                    if !existing.pop() {
                        return Err(PathError::OutsideRoots(source.display().to_string()));
                    }
                }
                let canon = normalize_display(
                    &fs::canonicalize(&existing).map_err(|e| PathError::Io(e.to_string()))?,
                );
                if !self.within_roots(&canon) {
                    return Err(PathError::OutsideRoots(canon.display().to_string()));
                }
                let meta = fs::metadata(&canon).map_err(|e| PathError::Io(e.to_string()))?;
                if !meta.is_dir() {
                    return Err(PathError::NotADirectory(canon.display().to_string()));
                }
                let mut out = canon;
                for name in tail.into_iter().rev() {
                    if name == "." || name == ".." {
                        return Err(PathError::ParentTraversal(source.display().to_string()));
                    }
                    out.push(name);
                }
                reject_parent_components(&out)?;
                Ok(out)
            }
        }
    }

    /// 在授权根内创建目录（已存在则复用），返回校验后的路径。
    pub fn ensure_dir_under_root(&self, desired: &Path) -> Result<PathBuf, PathError> {
        let validated = self.validate_bind_source(desired)?;
        if validated.exists() {
            return Ok(validated);
        }
        fs::create_dir_all(&validated).map_err(|e| PathError::Io(e.to_string()))?;
        // 创建后重新校验：确认没有被中间目录换成符号链接。
        let again = self.validate_bind_source(&validated)?;
        if again != validated {
            return Err(PathError::OutsideRoots(again.display().to_string()));
        }
        Ok(validated)
    }
}

fn reject_parent_components(p: &Path) -> Result<(), PathError> {
    for c in p.components() {
        match c {
            Component::ParentDir => {
                return Err(PathError::ParentTraversal(p.display().to_string()))
            }
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::Normal(_) => {}
        }
    }
    Ok(())
}

#[cfg(unix)]
fn check_root_safety(root: &Path) -> Result<(), PathError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(root).map_err(|e| PathError::Io(e.to_string()))?;
    let mode = meta.mode();
    // SAFETY: geteuid 只读当前进程凭证，无副作用。
    let euid = unsafe { libc::geteuid() };
    // 拥有者须是管理员身份：root，或 skiff 进程自身属主（非 root 的开发/测试运行）。
    // 生产以 root 跑时 euid == 0，判定与「必须 root 拥有」等价；插件独立 UID
    // 自建的根仍然被拒——防的正是插件重命名/替换授权根。
    if (meta.uid() != 0 && meta.uid() != euid) || mode & 0o022 != 0 {
        return Err(PathError::UnsafeRoot(root.display().to_string()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_root_safety(_root: &Path) -> Result<(), PathError> {
    // Windows 开发机没有 root 语义；部署形态是 Linux，此处不做等价比对。
    Ok(())
}

/// Linux 上的逐段打开：每层都用 `O_NOFOLLOW`，防止「校验后中间目录被换成符号链接」。
/// 返回已打开的目录 fd（调用方可直接 fchdir / openat 使用）。
///
/// **尚未接入 `validate_bind_source`**：Docker 只接受路径而不是 fd，即使这里打开成功，
/// 挂载那一刻用的仍是字符串路径，窗口没有真正关掉。等 M5 评估「传 fd / 定期巡检」再决定，
/// 现在保留实现是为了不在下一次重构时重新发明一遍。
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn open_dir_nofollow(root: &Path, rel: &Path) -> Result<std::fs::File, PathError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::FromRawFd;

    reject_parent_components(rel)?;
    if rel.is_absolute() {
        return Err(PathError::NotAbsolute(rel.display().to_string()));
    }
    let croot = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| PathError::Io("root 含 NUL".into()))?;
    // SAFETY: croot 是合法 C 字符串；失败返回负值。
    let mut fd = unsafe {
        libc::open(
            croot.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(PathError::Io(format!(
            "open root 失败: {}",
            std::io::Error::last_os_error()
        )));
    }
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            unsafe { libc::close(fd) };
            return Err(PathError::ParentTraversal(rel.display().to_string()));
        };
        let cname =
            CString::new(name.as_bytes()).map_err(|_| PathError::Io("组件含 NUL".into()))?;
        // SAFETY: fd 有效；cname 合法；成功返回新的 fd。
        let next = unsafe {
            libc::openat(
                fd,
                cname.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        unsafe { libc::close(fd) };
        if next < 0 {
            return Err(PathError::Io(format!(
                "openat({}) 失败（符号链接或非目录）: {}",
                name.to_string_lossy(),
                std::io::Error::last_os_error()
            )));
        }
        fd = next;
    }
    // SAFETY: fd 由本函数拥有且未关闭。
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> (tempfile::TempDir, PathGuard) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("authorized");
        fs::create_dir_all(&root).unwrap();
        let g = PathGuard::new(vec![root]).unwrap();
        (dir, g)
    }

    #[test]
    fn existing_path_inside_root_ok() {
        let (dir, g) = guard();
        let inside = dir.path().join("authorized").join("data");
        fs::create_dir_all(&inside).unwrap();
        let out = g.validate_bind_source(&inside).unwrap();
        // 期望值也要走 canonicalize + normalize：Windows runner 的临时目录可能带
        // 8.3 短名（RUNNER~1）或大小写差异，直接 starts_with 未规范化的拼接路径会误红。
        let expect = normalize_display(&fs::canonicalize(dir.path().join("authorized")).unwrap());
        assert!(
            out.starts_with(&expect),
            "out={} expect={}",
            out.display(),
            expect.display()
        );
    }

    #[test]
    fn outside_root_rejected() {
        let (dir, g) = guard();
        let outside = dir.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        assert!(matches!(
            g.validate_bind_source(&outside).unwrap_err(),
            PathError::OutsideRoots(_)
        ));
    }

    #[test]
    fn parent_traversal_rejected() {
        let (dir, g) = guard();
        let sneaky = dir.path().join("authorized").join("..").join("authorized");
        assert!(matches!(
            g.validate_bind_source(&sneaky).unwrap_err(),
            PathError::ParentTraversal(_)
        ));
    }

    #[test]
    fn relative_path_rejected() {
        let (_dir, g) = guard();
        assert!(matches!(
            g.validate_bind_source(Path::new("relative/dir"))
                .unwrap_err(),
            PathError::NotAbsolute(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_rejected() {
        let (dir, g) = guard();
        let target = dir.path().join("outside");
        fs::create_dir_all(&target).unwrap();
        let link = dir.path().join("authorized").join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(g.validate_bind_source(&link).is_err());
    }

    /// 授权根组/其他人可写 → 必须拒（root 检查在 unix 真正生效的那半边）。
    #[cfg(unix)]
    #[test]
    fn world_writable_root_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("loose");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            PathGuard::new(vec![root]).unwrap_err(),
            PathError::UnsafeRoot(_)
        ));
    }

    #[test]
    fn nonexistent_path_inside_root_is_ok() {
        let (dir, g) = guard();
        let future = dir.path().join("authorized").join("new").join("deep");
        let out = g.validate_bind_source(&future).unwrap();
        assert!(out.ends_with("new/deep") || out.ends_with("new\\deep"));
    }

    #[test]
    fn no_roots_rejects_everything() {
        let g = PathGuard::new(vec![]).unwrap();
        let p = std::env::temp_dir();
        assert!(matches!(
            g.validate_bind_source(&p).unwrap_err(),
            PathError::OutsideRoots(_)
        ));
    }

    #[test]
    fn ensure_dir_creates_and_revalidates() {
        let (dir, g) = guard();
        let target = dir.path().join("authorized").join("created");
        let out = g.ensure_dir_under_root(&target).unwrap();
        assert!(out.is_dir());
    }
}
