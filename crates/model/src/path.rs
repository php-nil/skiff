//! 路径词汇：纯代数，**零 I/O**（不碰文件系统）。
//!
//! 为什么单独抽一层：仓库里有三处各写各的路径判定——内核的 `policy::PathGuard`
//! （Docker 绑定挂载）、内核的 `plugin::fsgrant`（插件 `path:` 授权）、插件侧的
//! `skiff-plugin-common`（插件自己的前置快筛）。三处各写一遍前缀匹配，最容易出的
//! 事故是「字符串前缀相同但不是目录包含」：`/etc/nginx/conf.d-evil` 落在
//! `/etc/nginx/conf.d` 的字符串前缀下，却**不在**该目录内。
//!
//! 口径分工（不要混用）：
//! - **字符串级**（[`within`] / [`within_any`]）：两侧都当作已规范化的绝对路径，
//!   只做组件边界比较，不碰文件系统。插件侧用它做**快筛**（fail fast，早点报错）。
//! - **canonical 级**（[`within_root`] / [`is_root_path`]）：要求两侧都已是真实
//!   路径（`canonicalize` 过），按路径组件比较，符号链接逃逸由内核的
//!   `fsgrant` / `PathGuard` 在解析阶段挡住。
//!
//! 插件侧的字符串快筛**不是**授权判定：真正的判定在内核（`skiff_plugin::fsgrant`），
//! 它会重新 canonicalize 后再比对。插件侧快筛与内核判定口径不同是有意的分工，
//! 不是「两套规则」。

use std::path::{Component, Path, PathBuf};

/// 宿主绝对路径判定（平台宽容）。
///
/// 生产只跑 Linux（`/` 开头即绝对）；同时接受平台 `Path::is_absolute()`，
/// 是为了让开发机（Windows）与 CI 不会把 `path:/proc` 这类合法 Linux 路径
/// 判成相对路径。
pub fn is_absolute(raw: &str) -> bool {
    raw.starts_with('/') || Path::new(raw).is_absolute()
}

/// 容器内路径判定（严格 POSIX）。
///
/// 与 [`is_absolute`] 的区别：容器内路径只可能是 Linux 路径，`C:\x` 这种
/// Windows 写法在容器里没有意义，必须拒掉——所以这里不认平台 `is_absolute()`。
pub fn is_posix_absolute(raw: &str) -> bool {
    raw.starts_with('/')
}

/// 路径是否含 `..` **段**（组件级，不是子串）。
///
/// `/etc/a/../b` 含父目录段；`/etc/a..b` 只是一个名字里带两个点的普通文件。
/// 子串判定会把后者一并拒掉，那是误杀。
pub fn has_parent_segment(raw: &str) -> bool {
    Path::new(raw)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

/// 目录尾斜杠归一（`/` 自身保留），避免拼出 `//`。
pub fn normalize_dir(raw: &str) -> String {
    let d = raw.trim();
    if d == "/" {
        return "/".into();
    }
    d.trim_end_matches('/').to_string()
}

/// 路径是否落在根内（**字符串级**，根自身算在内）。
///
/// 组件边界敏感：`/etc/nginx/conf.d-evil` **不**在 `/etc/nginx/conf.d` 内。
pub fn within(path: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    if root == "/" {
        return path.starts_with('/');
    }
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false)
}

/// 路径是否落在任一根内（字符串级）。
pub fn within_any(path: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| within(path, r))
}

/// 上一级目录；已在授权根上（或父级越过根）时返回 `None`。
pub fn parent_within(path: &str, roots: &[String]) -> Option<String> {
    let norm = normalize_dir(path);
    if roots.iter().any(|r| normalize_dir(r) == norm) {
        return None;
    }
    let (parent, _) = norm.rsplit_once('/')?;
    if parent.is_empty() {
        return None;
    }
    let parent = parent.to_string();
    within_any(&parent, roots).then_some(parent)
}

/// canonical 路径是否落在 canonical 根内（组件级比较，根自身算在内）。
///
/// 两侧必须都已经是真实路径：调用方负责 `canonicalize`。
pub fn within_root(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

/// canonical 路径是否**就是**某个根本身。
///
/// 「授权一个目录」与「授权毁掉这个目录」是两件事：删除/改名必须靠这条把
/// 授权根本身挡掉。
pub fn is_root_path(root: &Path, target: &Path) -> bool {
    target == root
}

/// canonical 根 + canonical 目标 → 是否命中任一根 / 是否命中根本身。
///
/// 判定用的一律是 canonical 路径：符号链接逃逸在解析阶段就被挡住，
/// 这里只做最后的归属比对。
pub fn any_root_contains(roots: &[PathBuf], target: &Path) -> bool {
    roots.iter().any(|r| within_root(r, target))
}

/// 目标是否**命中某个根本身**（而不是它下面的东西）。
pub fn hits_a_root_itself(roots: &[PathBuf], target: &Path) -> bool {
    roots.iter().any(|r| is_root_path(r, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_is_component_boundary_sensitive() {
        assert!(within("/etc/nginx/conf.d", "/etc/nginx/conf.d"));
        assert!(within("/etc/nginx/conf.d/x.conf", "/etc/nginx/conf.d"));
        assert!(within("/x", "/"));
        // 前缀相同但不是目录包含 → 必须拒
        assert!(!within("/etc/nginx/conf.d-evil", "/etc/nginx/conf.d"));
        assert!(!within("/etc/nginx/nginx.conf", "/etc/nginx/conf.d"));
        assert!(!within("/etc", ""));
        assert!(!within_any("/etc", &[]));
    }

    #[test]
    fn within_any_accepts_several_roots() {
        let roots = vec!["/etc/nginx/conf.d".to_string(), "/var/www".to_string()];
        assert!(within_any("/etc/nginx/conf.d", &roots));
        assert!(within_any("/var/www/html", &roots));
        assert!(!within_any("/etc/nginx/nginx.conf", &roots));
    }

    #[test]
    fn absolute_accepts_linux_paths_on_any_platform() {
        assert!(is_absolute("/etc/nginx"));
        assert!(is_absolute("/proc"));
        assert!(!is_absolute("etc/nginx"));
        assert!(!is_absolute(""));
    }

    #[test]
    fn posix_absolute_rejects_windows_style_paths() {
        assert!(is_posix_absolute("/etc/nginx"));
        assert!(!is_posix_absolute("etc/nginx"));
        assert!(!is_posix_absolute("C:\\Windows"));
    }

    #[test]
    fn parent_segment_is_component_level_not_substring() {
        assert!(has_parent_segment("/etc/a/../b"));
        assert!(has_parent_segment("../b"));
        assert!(has_parent_segment("/etc/.."));
        // 名字里带两个点但不是父目录段 → 放行
        assert!(!has_parent_segment("/etc/a..b"));
    }

    #[test]
    fn normalize_dir_keeps_root_and_drops_trailing_slash() {
        assert_eq!(normalize_dir("/etc/nginx/conf.d/"), "/etc/nginx/conf.d");
        assert_eq!(normalize_dir("/"), "/");
        assert_eq!(normalize_dir("  /var/www  "), "/var/www");
    }

    #[test]
    fn parent_within_stops_at_the_granted_root() {
        let roots = vec!["/var/www".to_string()];
        assert_eq!(
            parent_within("/var/www/html", &roots).as_deref(),
            Some("/var/www")
        );
        assert_eq!(parent_within("/var/www", &roots), None);
        assert_eq!(
            parent_within("/var/www/html/css", &roots).as_deref(),
            Some("/var/www/html")
        );
        assert_eq!(parent_within("/etc", &roots), None);
    }

    #[test]
    fn canonical_containment_distinguishes_root_from_children() {
        let roots = vec![PathBuf::from("/etc/nginx/conf.d")];
        assert!(any_root_contains(&roots, Path::new("/etc/nginx/conf.d")));
        assert!(any_root_contains(&roots, Path::new("/etc/nginx/conf.d/x")));
        assert!(!any_root_contains(
            &roots,
            Path::new("/etc/nginx/nginx.conf")
        ));
        assert!(hits_a_root_itself(&roots, Path::new("/etc/nginx/conf.d")));
        assert!(!hits_a_root_itself(
            &roots,
            Path::new("/etc/nginx/conf.d/x")
        ));
    }
}
