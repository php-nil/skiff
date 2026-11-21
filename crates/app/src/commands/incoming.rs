//! 暂存目录的过期文件清理。
//!
//! 商店安装与升级下载都要先把包写进暂存目录再原子改名；中途失败或进程被杀会留下
//! 半成品。清理逻辑此前在两处各写一份（扩展名白名单不同：商店是 `.wbpkg`/`.part`，
//! 升级是 `.tar.gz`/`.part`），规则却完全一样——按「文件名后缀 + mtime 超龄」判定。
//! 这里收成一处，调用方只声明自己认的扩展名。

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tracing::warn;

use crate::admin_fs::AdminFs;

/// 清理 `dir` 下扩展名命中 `extensions` 且 mtime 超过 `ttl` 的文件，返回删除条数。
///
/// 判定口径（两处调用方原本各自实现，规则一致）：
///
/// - 只看文件名后缀，不看目录层级（暂存目录是平铺的）；
/// - 读不到 mtime 或 mtime 晚于当前时间（时钟回拨）时**保守跳过**，避免误删新文件；
/// - 目录不删，只删普通文件。
///
/// 删除经内核文件通道（每个被清掉的暂存包一条 `fs.remove` 审计）。
/// `now` 显式传入以便单测精确控制年龄，不依赖修改 mtime。
pub fn cleanup_expired(
    kfs: &AdminFs,
    dir: &Path,
    now: SystemTime,
    ttl: Duration,
    extensions: &[&str],
) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !extensions.iter().any(|ext| name.ends_with(ext)) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        // 读不到 mtime 或 mtime 在当前时间之后（时钟回拨）时保守跳过，避免误删新文件。
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < ttl {
            continue;
        }
        match kfs.remove_kernel(&path) {
            Ok(()) => removed += 1,
            Err(e) => warn!(path = %path.display(), error = %e, "清理过期暂存文件失败"),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    /// 清理用例的通道夹具：数据根与站点根都在临时目录下，审计写内存库。
    fn admin(dir: &Path) -> AdminFs {
        AdminFs::with_paths(
            dir.to_path_buf(),
            dir.join("www"),
            Arc::new(skiff_store::Db::open_in_memory().unwrap()),
        )
    }

    /// 只删命中扩展名且超龄的普通文件；目录与非暂存文件一律不动。
    #[test]
    fn removes_only_expired_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("a.wbpkg");
        let fresh = dir.path().join("b.wbpkg");
        let other = dir.path().join("c.txt");
        let sub = dir.path().join("d.wbpkg");
        for f in [&stale, &fresh, &other] {
            fs::write(f, b"x").unwrap();
        }
        fs::create_dir_all(&sub).unwrap();

        let now = SystemTime::now();
        let ttl = Duration::from_secs(3600);
        // 把 `stale` 的 mtime 推到 2 小时前，其余保持现在。
        let old = now - Duration::from_secs(2 * 3600);
        filetime_set(&stale, old);

        let removed = cleanup_expired(
            &admin(dir.path()),
            dir.path(),
            now,
            ttl,
            &[".wbpkg", ".part"],
        );
        assert_eq!(removed, 1);
        assert!(!stale.exists());
        assert!(fresh.is_file(), "未超龄的文件不得删除");
        assert!(other.is_file(), "非暂存文件不得删除");
        assert!(sub.is_dir(), "目录不得删除");
    }

    /// 时钟回拨（mtime 晚于当前时间）时保守跳过，不误删新文件。
    #[test]
    fn skips_files_newer_than_now() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.part");
        fs::write(&f, b"x").unwrap();
        let now = SystemTime::now();
        let removed = cleanup_expired(
            &admin(dir.path()),
            dir.path(),
            now - Duration::from_secs(3600),
            Duration::from_secs(60),
            &[".part"],
        );
        assert_eq!(removed, 0);
        assert!(f.is_file());
    }

    /// 目录不存在时返回 0，不报错（调用方在启动路径上调用，不能因此失败）。
    #[test]
    fn missing_dir_is_not_an_error() {
        // 通道夹具本身要落在真实目录上；被清理的目录不存在才是本用例的前提。
        let dir = tempfile::tempdir().unwrap();
        let removed = cleanup_expired(
            &admin(dir.path()),
            Path::new("/nonexistent/skiff/incoming"),
            SystemTime::now(),
            Duration::from_secs(60),
            &[".part"],
        );
        assert_eq!(removed, 0);
    }

    /// 改 mtime：只用于构造超龄样本，不依赖文件内容。
    fn filetime_set(path: &Path, t: SystemTime) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(t).unwrap();
    }
}
