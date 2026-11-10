//! 文件能力面的管理员写命令：只把显式 DTO 转成一次 `AdminFs` 调用。
//!
//! 授权、路径代数、原子写、路径锁、回收站、配额与审计都在 `AdminFs`/`skiff-fs`；
//! 命令层不重复判定，也不改写 `FsError`——传输层按 `code()`/`http_status()` 原样映射
//! `{ok:false,error:{code,message}}`。DTO 只携带业务入参，不含展示字段。

use skiff_fs::{ClearTrashOutcome, FsError, QuotaSnapshot, RestoreOutcome};

use crate::admin_fs::{
    AdminFs, ArchiveResult, ChmodResult, ChownResult, CopyResult, ExtractResult, FileText, Listing,
    RemoveResult, TrashItem, WriteResult,
};

/// 命令名清单：与 [`crate::actions`] 文件类动作的 `command` 一一对应。
pub const NAMES: &[&str] = &[
    "list",
    "read",
    "write",
    "mkdir",
    "rename",
    "remove",
    "copy",
    "chmod",
    "chown",
    "archive",
    "extract",
    "upload",
    "stat",
    "list_dirs",
    "trash_list",
    "trash_restore",
    "trash_purge",
    "trash_clear",
    "quota_list",
    "quota_set",
    "quota_cancel",
    "quota_scan",
];

#[derive(Debug, Clone)]
pub struct ListDir {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct ListDirs {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct ReadFile {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct WriteFile {
    pub user: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct MakeDir {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct RenameEntry {
    pub user: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct RemoveEntry {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct CopyEntry {
    pub user: String,
    pub from: String,
    pub to: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct ChmodEntry {
    pub user: String,
    pub path: String,
    pub mode: u32,
}

#[derive(Debug, Clone)]
pub struct ChownEntry {
    pub user: String,
    pub path: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone)]
pub struct ArchiveEntries {
    pub user: String,
    pub sources: Vec<String>,
    pub dest: String,
    pub format: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct ExtractArchive {
    pub user: String,
    pub archive: String,
    pub dest: String,
    pub overwrite: bool,
}

/// 上传分片：`data` 借用请求体，避免为大文件再复制一份。
#[derive(Debug)]
pub struct UploadChunk<'a> {
    pub user: String,
    pub path: String,
    pub offset: u64,
    pub data: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct StatPath {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct TrashList {
    pub user: String,
}

#[derive(Debug, Clone)]
pub struct TrashRestore {
    pub user: String,
    pub id: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone)]
pub struct TrashPurge {
    pub user: String,
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct TrashClear {
    pub user: String,
}

#[derive(Debug, Clone)]
pub struct QuotaList {
    pub user: String,
}

#[derive(Debug, Clone)]
pub struct QuotaSet {
    pub user: String,
    pub path: String,
    pub max_bytes: Option<u64>,
    pub max_files: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct QuotaCancel {
    pub user: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct QuotaScan {
    pub user: String,
    pub path: String,
}

pub fn list(fs: &AdminFs, cmd: ListDir) -> Result<Listing, FsError> {
    fs.list(&cmd.user, &cmd.path)
}

pub fn list_dirs(fs: &AdminFs, cmd: ListDirs) -> Result<Vec<String>, FsError> {
    fs.list_dirs(&cmd.user, &cmd.path)
}

pub fn read(fs: &AdminFs, cmd: ReadFile) -> Result<FileText, FsError> {
    fs.read(&cmd.user, &cmd.path)
}

pub fn write(fs: &AdminFs, cmd: WriteFile) -> Result<WriteResult, FsError> {
    fs.write(&cmd.user, &cmd.path, &cmd.content)
}

pub fn mkdir(fs: &AdminFs, cmd: MakeDir) -> Result<String, FsError> {
    fs.mkdir(&cmd.user, &cmd.path)
}

pub fn rename(fs: &AdminFs, cmd: RenameEntry) -> Result<(), FsError> {
    fs.rename(&cmd.user, &cmd.from, &cmd.to)
}

pub fn remove(fs: &AdminFs, cmd: RemoveEntry) -> Result<RemoveResult, FsError> {
    fs.remove(&cmd.user, &cmd.path)
}

pub fn copy(fs: &AdminFs, cmd: CopyEntry) -> Result<CopyResult, FsError> {
    fs.copy(&cmd.user, &cmd.from, &cmd.to, cmd.overwrite)
}

pub fn chmod(fs: &AdminFs, cmd: ChmodEntry) -> Result<ChmodResult, FsError> {
    fs.chmod(&cmd.user, &cmd.path, cmd.mode)
}

pub fn chown(fs: &AdminFs, cmd: ChownEntry) -> Result<ChownResult, FsError> {
    fs.chown(&cmd.user, &cmd.path, cmd.uid, cmd.gid)
}

pub fn archive(fs: &AdminFs, cmd: ArchiveEntries) -> Result<ArchiveResult, FsError> {
    fs.archive(
        &cmd.user,
        &cmd.sources,
        &cmd.dest,
        &cmd.format,
        cmd.overwrite,
    )
}

pub fn extract(fs: &AdminFs, cmd: ExtractArchive) -> Result<ExtractResult, FsError> {
    fs.extract(&cmd.user, &cmd.archive, &cmd.dest, cmd.overwrite)
}

pub fn upload(fs: &AdminFs, cmd: UploadChunk<'_>) -> Result<WriteResult, FsError> {
    fs.upload_chunk(&cmd.user, &cmd.path, cmd.offset, cmd.data)
}

pub fn stat(fs: &AdminFs, cmd: StatPath) -> Result<skiff_fs::StatInfo, FsError> {
    fs.stat(&cmd.user, &cmd.path)
}

pub fn trash_list(fs: &AdminFs, cmd: TrashList) -> Result<Vec<TrashItem>, FsError> {
    fs.trash_list(&cmd.user)
}

pub fn trash_restore(fs: &AdminFs, cmd: TrashRestore) -> Result<RestoreOutcome, FsError> {
    fs.trash_restore(&cmd.user, &cmd.id, cmd.overwrite)
}

pub fn trash_purge(fs: &AdminFs, cmd: TrashPurge) -> Result<(), FsError> {
    fs.trash_purge(&cmd.user, &cmd.id)
}

pub fn trash_clear(fs: &AdminFs, cmd: TrashClear) -> Result<ClearTrashOutcome, FsError> {
    fs.trash_clear(&cmd.user)
}

pub fn quota_list(fs: &AdminFs, cmd: QuotaList) -> Result<Vec<QuotaSnapshot>, FsError> {
    fs.quota_list(&cmd.user)
}

pub fn quota_set(fs: &AdminFs, cmd: QuotaSet) -> Result<QuotaSnapshot, FsError> {
    fs.quota_set(&cmd.user, &cmd.path, cmd.max_bytes, cmd.max_files)
}

pub fn quota_cancel(fs: &AdminFs, cmd: QuotaCancel) -> Result<QuotaSnapshot, FsError> {
    fs.quota_cancel(&cmd.user, &cmd.path)
}

pub fn quota_scan(fs: &AdminFs, cmd: QuotaScan) -> Result<QuotaSnapshot, FsError> {
    fs.quota_scan(&cmd.user, &cmd.path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::admin_fs::MAX_EDIT_BYTES;
    use crate::commands::test_support::state_at;
    use crate::AppState;

    fn setup(root: &std::path::Path) -> Arc<AppState> {
        std::fs::create_dir_all(root.join("www")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        state_at(root)
    }

    fn user() -> String {
        "admin".into()
    }

    #[test]
    fn write_read_roundtrip_reports_truncated_over_limit() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup(dir.path());
        let fs = state.admin_fs();
        let path = dir.path().join("www/hello.txt").display().to_string();

        let out = write(
            fs,
            WriteFile {
                user: user(),
                path: path.clone(),
                content: "hello".into(),
            },
        )
        .unwrap();
        assert_eq!(out.size, 5);
        let file = read(
            fs,
            ReadFile {
                user: user(),
                path: path.clone(),
            },
        )
        .unwrap();
        assert_eq!(file.text, "hello");
        assert!(!file.truncated);

        // 超过编辑上限的文件由底层截断并标记只读。
        let big = dir.path().join("www/big.log");
        std::fs::write(&big, "x".repeat(MAX_EDIT_BYTES + 10)).unwrap();
        let file = read(
            fs,
            ReadFile {
                user: user(),
                path: big.display().to_string(),
            },
        )
        .unwrap();
        assert!(file.truncated);
        assert_eq!(file.text.len(), MAX_EDIT_BYTES);
        assert_eq!(file.size as usize, MAX_EDIT_BYTES + 10);
    }

    #[test]
    fn denied_and_protected_paths_map_to_domain_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup(dir.path());
        let fs = state.admin_fs();

        let denied = read(
            fs,
            ReadFile {
                user: user(),
                path: "../etc/passwd".into(),
            },
        )
        .unwrap_err();
        assert_eq!(denied.code(), "fs.denied");
        assert_eq!(denied.http_status(), 403);

        let protected = write(
            fs,
            WriteFile {
                user: user(),
                path: dir.path().join("data/secret").display().to_string(),
                content: "x".into(),
            },
        )
        .unwrap_err();
        assert_eq!(protected.code(), "fs.denied");
        assert_eq!(protected.http_status(), 403);
    }

    #[test]
    fn trash_restore_purge_and_clear_are_domain_commands() {
        let dir = tempfile::tempdir().unwrap();
        let state = setup(dir.path());
        let fs = state.admin_fs();
        let a = dir.path().join("www/a.txt");
        let b = dir.path().join("www/b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        let out = remove(
            fs,
            RemoveEntry {
                user: user(),
                path: a.display().to_string(),
            },
        )
        .unwrap();
        let id = out.trash_id.expect("删除应移入回收站");
        assert!(!a.exists());
        let items = trash_list(fs, TrashList { user: user() }).unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].original_path.ends_with("a.txt"));

        trash_restore(
            fs,
            TrashRestore {
                user: user(),
                id,
                overwrite: false,
            },
        )
        .unwrap();
        assert!(a.exists());
        assert!(trash_list(fs, TrashList { user: user() })
            .unwrap()
            .is_empty());

        let id = remove(
            fs,
            RemoveEntry {
                user: user(),
                path: a.display().to_string(),
            },
        )
        .unwrap()
        .trash_id
        .unwrap();
        trash_purge(fs, TrashPurge { user: user(), id }).unwrap();
        assert!(!a.exists());
        assert!(trash_list(fs, TrashList { user: user() })
            .unwrap()
            .is_empty());

        for path in [&a, &b] {
            std::fs::write(path, "x").unwrap();
            remove(
                fs,
                RemoveEntry {
                    user: user(),
                    path: path.display().to_string(),
                },
            )
            .unwrap();
        }
        let cleared = trash_clear(fs, TrashClear { user: user() }).unwrap();
        assert_eq!(cleared.removed, 2);
        assert!(cleared.failed.is_empty());
        assert!(trash_list(fs, TrashList { user: user() })
            .unwrap()
            .is_empty());
    }
}
