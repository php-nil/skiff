//! 宿主路径授权（`path:` 对象 grant）的唯一判定实现。
//!
//! 官方插件（files / nginx / monitor）需要在 `data/` 之外读写宿主文件。能力面
//! 仍然是同一条 `fs` 路由（权限串也不变：`fs:read` / `fs:write` / `fs:list` /
//! `fs:mkdir` / `fs:remove` / `fs:rename`），但**绝对路径分支**额外要求
//! 安装时确认过的 `path:` 对象授权根命中——「有 fs:read」和「能读宿主 /etc/nginx」
//! 是两把钥匙，缺一不可（与 docker 的 perm + object grant 同一口径）。
//!
//! 判定顺序（缺一不可）：
//! 1. 形式：绝对路径、无 `..`、无 NUL；
//! 2. 存在性：目标（或创建时的父目录）必须能 canonicalize —— 软链一律按真实路径说话；
//! 3. 归属：canonical 目标必须落在某个 `path:` 授权根的 canonical 路径之下。
//!
//! 授权根在**每次调用**时重新 canonicalize：管理员后来把目录搬走或删掉，授权即失效，
//! 不会因为库里存了旧字符串而继续放行。

use std::path::{Path, PathBuf};

use skiff_model::{any_root_contains, has_parent_segment, hits_a_root_itself, is_absolute};
use skiff_store::Grant;

pub const PATH_PREFIX: &str = "path:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// 形式非法（相对路径、含 `..`、含 NUL、空）
    Illegal(&'static str),
    /// 目标不在任何授权根内
    Outside,
    /// 目标或父目录不存在 / 不可 canonicalize
    Missing(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Illegal(m) => write!(f, "{m}"),
            ResolveError::Outside => write!(f, "路径不在已授权的 path: 根内"),
            ResolveError::Missing(m) => write!(f, "路径不可用: {m}"),
        }
    }
}

/// 从 grant 列表里取出 `path:` 授权根（原始字符串，未 canonicalize）。
pub fn roots_of(grants: &[Grant]) -> Vec<String> {
    grants
        .iter()
        .filter(|g| g.kind == "object" && g.value.starts_with(PATH_PREFIX))
        .map(|g| g.value[PATH_PREFIX.len()..].to_string())
        .collect()
}

/// 把授权根转成 canonical 路径。不存在 / 不是目录的一律丢弃（授权随之失效）。
pub fn canonical_roots(grants: &[Grant]) -> Vec<PathBuf> {
    roots_of(grants)
        .into_iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .filter(|p| p.is_dir())
        .collect()
}

/// 目标是否落在任一授权根内（允许等于根）。
pub fn inside_any(roots: &[PathBuf], target: &Path) -> bool {
    any_root_contains(roots, target)
}

/// 目标是否**就是**某个授权根本身。
///
/// 删除与改名额外要求这一条：只授权了 `/var/log` 时，插件不该能一次把整个
/// `/var/log` 删掉或改名——那是「授权一个目录」和「授权毁掉这个目录」的区别。
pub fn is_root_itself(roots: &[PathBuf], target: &Path) -> bool {
    hits_a_root_itself(roots, target)
}

/// 把插件给的请求路径解析成可操作的真实路径。
///
/// `create = true` 表示目标可以尚不存在（写文件 / 建目录 / 改名目标），此时校验父目录。
pub fn resolve(roots: &[PathBuf], requested: &str, create: bool) -> Result<PathBuf, ResolveError> {
    if requested.is_empty() {
        return Err(ResolveError::Illegal("路径不能为空"));
    }
    if requested.contains('\0') {
        return Err(ResolveError::Illegal("路径含 NUL"));
    }
    if has_parent_segment(requested) {
        return Err(ResolveError::Illegal("路径不得含 .."));
    }
    if !is_absolute(requested) {
        return Err(ResolveError::Illegal("宿主路径必须是绝对路径"));
    }
    let path = Path::new(requested);

    if create {
        let parent = path.parent().ok_or(ResolveError::Illegal("路径无父目录"))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(ResolveError::Illegal("路径缺少文件名"))?;
        if name.is_empty() || name == "." || name == "/" {
            return Err(ResolveError::Illegal("路径缺少文件名"));
        }
        let canon_parent =
            std::fs::canonicalize(parent).map_err(|e| ResolveError::Missing(e.to_string()))?;
        let mut target = canon_parent.join(name);
        // 目标已存在（比如是个软链）时按真实路径重新判定：只校验父目录会被
        // 「根内放一个指向根外的软链」绕过。
        if target.exists() {
            target =
                std::fs::canonicalize(&target).map_err(|e| ResolveError::Missing(e.to_string()))?;
        }
        if inside_any(roots, &target) {
            Ok(target)
        } else {
            Err(ResolveError::Outside)
        }
    } else {
        let target =
            std::fs::canonicalize(path).map_err(|e| ResolveError::Missing(e.to_string()))?;
        if inside_any(roots, &target) {
            Ok(target)
        } else {
            Err(ResolveError::Outside)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(value: &str) -> Grant {
        Grant::object(value.to_string())
    }

    fn tmp_root(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join(name);
        std::fs::create_dir_all(&sub).unwrap();
        (dir, sub)
    }

    #[test]
    fn roots_come_only_from_path_grants() {
        let grants = vec![
            grant("path:/var/log"),
            grant("container:web"),
            grant("path:/opt"),
        ];
        assert_eq!(roots_of(&grants), vec!["/var/log", "/opt"]);
    }

    /// 授权根不存在时授权自动失效（不是「先记着以后算」）。
    #[test]
    fn missing_roots_are_dropped() {
        let grants = vec![grant("path:/definitely/not/here")];
        assert!(canonical_roots(&grants).is_empty());
    }

    #[test]
    fn resolve_rejects_relative_dotdot_and_outside() {
        let (_dir, root) = tmp_root("a");
        let roots = canonical_roots(&[grant(&format!("path:{}", root.display()))]);

        assert_eq!(
            resolve(&roots, "relative/x", false),
            Err(ResolveError::Illegal("宿主路径必须是绝对路径"))
        );
        assert_eq!(
            resolve(&roots, "/etc/../passwd", false),
            Err(ResolveError::Illegal("路径不得含 .."))
        );
        let outside = std::env::temp_dir().join("skiff-fsgrant-outside.txt");
        std::fs::write(&outside, b"x").unwrap();
        assert_eq!(
            resolve(&roots, &outside.display().to_string(), false),
            Err(ResolveError::Outside)
        );
    }

    #[test]
    fn resolve_accepts_inside_and_create_under_root() {
        let (_dir, root) = tmp_root("b");
        let roots = canonical_roots(&[grant(&format!("path:{}", root.display()))]);
        let file = root.join("x.conf");
        std::fs::write(&file, b"ok").unwrap();
        let target = resolve(&roots, &file.display().to_string(), false).unwrap();
        assert!(target.starts_with(roots[0].as_path()));

        // 写新文件：目标还不存在，按父目录判定
        let fresh = root.join("new.conf");
        assert!(resolve(&roots, &fresh.display().to_string(), true).is_ok());
    }

    /// 删除/改名不允许把授权根本身当目标。
    #[test]
    fn deleting_the_granted_root_itself_is_rejected() {
        let (_dir, root) = tmp_root("c");
        let roots = canonical_roots(&[grant(&format!("path:{}", root.display()))]);
        let canon_root = roots[0].clone();
        assert!(inside_any(&roots, &canon_root));
        assert!(is_root_itself(&roots, &canon_root));
        let child = canon_root.join("x");
        std::fs::write(&child, b"x").unwrap();
        assert!(!is_root_itself(&roots, &child));
    }
}
