//! 站点备份归档：目录约定、归档名校验与列表。
//!
//! 归档本体由 `tasks::TaskService::site_backup` 提交 `tar` 作业生成（内核不在进程内
//! 打包）；恢复的作业编排在 `crate::tasks`——那里才有 `db` / `jobs` / `web_root`。
//! 本模块只放**不持状态**的纯逻辑，便于直接单测。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 归档目录：`<data_dir>/backups/sites/<domain>`，与备份任务同一处。
pub fn archive_dir(data_dir: &Path, domain: &str) -> PathBuf {
    data_dir.join("backups").join("sites").join(domain)
}

/// 归档名校验：只接受本目录下的一层文件名，且必须以 `.tar` 结尾。
///
/// 恢复是破坏性动作，名称来自请求体，必须挡住 `../../etc/passwd` 这类穿越；
/// 前导 `.` 一并拒绝，避免把隐藏文件当归档。
pub fn validate_archive_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("归档文件名不能为空".to_string());
    }
    if name.len() > 128 {
        return Err("归档文件名过长（上限 128 字节）".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("归档文件名不能包含路径分隔符或 `..`".to_string());
    }
    if name.starts_with('.') {
        return Err("归档文件名不能以 `.` 开头".to_string());
    }
    if !name.ends_with(".tar") {
        return Err("只支持 `.tar` 归档（站点备份的产物格式）".to_string());
    }
    Ok(())
}

/// 一份站点归档的只读描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteArchive {
    pub name: String,
    pub bytes: u64,
    /// Unix 秒；读不到时记 0（列表仍展示，排序按 0 处理）。
    pub modified: i64,
}

/// 最近 `limit` 份归档，按修改时间从新到旧。
///
/// 目录不存在、读失败都返回空表：备份目录尚未生成属于常态，不该让站点详情页报错。
pub fn list_archives(data_dir: &Path, domain: &str, limit: usize) -> Vec<SiteArchive> {
    let dir = archive_dir(data_dir, domain);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<SiteArchive> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            validate_archive_name(&name).ok()?;
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some(SiteArchive {
                name,
                bytes: meta.len(),
                modified,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| b.name.cmp(&a.name))
    });
    out.truncate(limit);
    out
}

/// 恢复前快照的命名：`<YYYYMMDD-HHMMSS>-pre-restore.tar`。
///
/// 复用同一目录便于在列表里一眼看到「恢复前状态」；名字里带 `-pre-restore`
/// 是为了和调度器产出的纯时间戳归档区分开。
pub fn pre_restore_name(stamp: &str) -> String {
    format!("{stamp}-pre-restore.tar")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_name_rejects_traversal_and_other_formats() {
        assert!(validate_archive_name("20261121-030000.tar").is_ok());
        assert!(validate_archive_name("20261121-030000-pre-restore.tar").is_ok());
        for bad in [
            "",
            "../20261121-030000.tar",
            "sub/20261121-030000.tar",
            "sub\\20261121-030000.tar",
            ".hidden.tar",
            "20261121-030000.tar.gz",
            "20261121-030000",
            "..",
        ] {
            assert!(
                validate_archive_name(bad).is_err(),
                "应拒绝非法归档名: {bad:?}"
            );
        }
    }

    #[test]
    fn list_archives_sorts_newest_first_and_skips_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let archive = archive_dir(data, "example.com");
        std::fs::create_dir_all(&archive).unwrap();
        for (name, body) in [
            ("20260101-000000.tar", "old"),
            ("20260301-000000.tar", "new"),
            ("notes.txt", "skip"),
        ] {
            std::fs::write(archive.join(name), body).unwrap();
        }
        let items = list_archives(data, "example.com", 10);
        assert_eq!(
            items.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["20260301-000000.tar", "20260101-000000.tar"]
        );
        assert_eq!(items[0].bytes, 3);
    }

    #[test]
    fn missing_directory_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_archives(dir.path(), "no-such.example", 10).is_empty());
    }
}
