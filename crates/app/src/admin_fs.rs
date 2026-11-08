//! 管理员侧文件服务：把 `skiff-fs` 接线到内核 Web 层。
//!
//! 设计口径（已批准）：内核持有的是 **fs 能力面的管理员客户端**，不是把宿主
//! 文件业务塞回内核业务层——授权、原子写、路径锁、审计全部由 `skiff-fs` 完成，
//! 本模块只负责三件事：
//!
//! 1. 用管理员主体（`Subject::Admin`）调用原语；
//! 2. 在 IO 前对内核数据根（`data/`、`data/plugins`、`data/official`）做硬拒，
//!    即使管理员不受 `path:` 授权根限制；
//! 3. 把审计事件落到现有 `audit` 表（actor=管理员，来源列写 `web`）。
//!
//! 回收站与配额：`FsConfig` 已带 `trash` 与 `QuotaTracker`，第一步页面删除走
//! `permanent = true`（无回收站 UI）；第二步接入 SQLite 回收站时把这里换成
//! `permanent = false` 即可，调用方不需要改。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;
use skiff_fs::{
    AuditEvent, AuditSink, ClearTrashOutcome, Fs, FsConfig, FsError, ListRequest, OpContext,
    Principal, QuotaLimit, QuotaSnapshot, QuotaTracker, QuotaUsage, ReadRequest, RestoreOutcome,
    TrashConfig, TrashEntry, TrashKind, TrashStore, WriteMode, WriteRequest, MAX_REQUEST_BYTES,
};
use skiff_store::{Db, QuotaRow, TrashRow};

use crate::WebConfig;

/// 文件原语错误在本模块公开：Web 层把 `code()/message()` 直接映射成 JSON。
pub use skiff_fs::FsError as FsErr;

/// 页面可编辑文件上限：与单次读写上限一致（256 KiB）。
pub const MAX_EDIT_BYTES: usize = MAX_REQUEST_BYTES;

#[derive(Debug, Serialize)]
pub struct Listing {
    /// 归一后的目录绝对路径。
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
    /// 条目数超过一页，页面只展示第一页。
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub dir: bool,
    pub symlink: bool,
    pub size: u64,
    pub mtime: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct FileText {
    pub path: String,
    pub text: String,
    /// 磁盘上的真实大小。
    pub size: u64,
    /// 超过编辑上限、只返回了第一段；页面必须只读。
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct WriteResult {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// 删除结果：`trash_id` 非空表示已移入回收站，页面据此提示可恢复。
#[derive(Debug, Serialize)]
pub struct RemoveResult {
    pub path: String,
    pub trash_id: Option<String>,
}

/// 回收站条目视图（不暴露内部落点 `trash_path`）。
#[derive(Debug, Serialize)]
pub struct TrashItem {
    pub id: String,
    pub original_path: String,
    pub kind: String,
    pub size: u64,
    pub files: u64,
    pub deleted_at: i64,
    pub expires_at: i64,
    pub actor_type: String,
    pub actor_id: String,
    pub site: Option<String>,
}

impl From<TrashEntry> for TrashItem {
    fn from(e: TrashEntry) -> Self {
        Self {
            id: e.id,
            original_path: e.original_path,
            kind: e.kind.as_str().to_string(),
            size: e.size,
            files: e.files,
            deleted_at: e.deleted_at,
            expires_at: e.expires_at,
            actor_type: e.actor_type,
            actor_id: e.actor_id,
            site: e.site,
        }
    }
}

fn entry_from_row(row: TrashRow) -> TrashEntry {
    let kind = match row.kind.as_str() {
        "dir" => TrashKind::Dir,
        "symlink" => TrashKind::Symlink,
        _ => TrashKind::File,
    };
    TrashEntry {
        id: row.id,
        actor_type: row.actor_type,
        actor_id: row.actor_id,
        original_path: row.original_path,
        trash_path: row.trash_path,
        kind,
        size: row.size,
        files: row.files,
        mode: row.mode,
        owner: row.owner,
        sha256: row.sha256,
        site: row.site,
        protected: row.protected,
        deleted_at: row.deleted_at,
        expires_at: row.expires_at,
        nlink: row.nlink,
        restored_at: row.restored_at,
        purged_at: row.purged_at,
        quota_root: row.quota_root,
    }
}

fn row_from_entry(e: TrashEntry) -> TrashRow {
    TrashRow {
        id: e.id,
        actor_type: e.actor_type,
        actor_id: e.actor_id,
        original_path: e.original_path,
        trash_path: e.trash_path,
        kind: e.kind.as_str().to_string(),
        size: e.size,
        files: e.files,
        mode: e.mode,
        owner: e.owner,
        sha256: e.sha256,
        site: e.site,
        protected: e.protected,
        deleted_at: e.deleted_at,
        expires_at: e.expires_at,
        nlink: e.nlink,
        restored_at: e.restored_at,
        purged_at: e.purged_at,
        quota_root: e.quota_root,
    }
}

/// 回收站落点：SQLite `trash_entries`。写入失败不阻断文件操作，但下次列表
/// 会缺失该条目——调用方仍应先落盘成功再报告删除成功。
struct DbTrashStore {
    db: Arc<Db>,
}

impl TrashStore for DbTrashStore {
    fn insert(&self, entry: TrashEntry) {
        let _ = self.db.trash_insert(&row_from_entry(entry));
    }

    fn get(&self, id: &str) -> Option<TrashEntry> {
        self.db.trash_get(id).ok().flatten().map(entry_from_row)
    }

    fn remove(&self, id: &str) -> Option<TrashEntry> {
        self.db.trash_remove(id).ok().flatten().map(entry_from_row)
    }

    fn mark_restored(&self, id: &str, ts: i64) -> Option<TrashEntry> {
        self.db
            .trash_mark_restored(id, ts)
            .ok()
            .flatten()
            .map(entry_from_row)
    }

    fn mark_purged(&self, id: &str, ts: i64) -> Option<TrashEntry> {
        self.db
            .trash_mark_purged(id, ts)
            .ok()
            .flatten()
            .map(entry_from_row)
    }

    fn list(&self) -> Vec<TrashEntry> {
        self.db
            .trash_live()
            .map(|rows| rows.into_iter().map(entry_from_row).collect())
            .unwrap_or_default()
    }
}

/// 配额缓存：SQLite `fs_quotas`。读路径每次查库（配额行数量很小）；
/// 写路径每次成功操作后增量修正，另有显式全量重扫。
struct DbQuotaTracker {
    db: Arc<Db>,
}

impl DbQuotaTracker {
    fn row_to_snapshot(row: QuotaRow) -> QuotaSnapshot {
        QuotaSnapshot {
            root: row.root_path,
            max_bytes: row.max_bytes,
            max_files: row.max_files,
            used_bytes: row.used_bytes,
            used_files: row.used_files,
            updated_at: row.scanned_at,
        }
    }
}

impl QuotaTracker for DbQuotaTracker {
    fn usage(&self, root: &Path) -> u64 {
        self.usage_stats(root).used_bytes
    }

    fn adjust(&self, root: &Path, delta: i64) {
        self.adjust_stats(root, delta, 0);
    }

    fn usage_stats(&self, root: &Path) -> QuotaUsage {
        self.db
            .fs_quota_get(&root.display().to_string())
            .ok()
            .flatten()
            .map(|row| QuotaUsage {
                used_bytes: row.used_bytes,
                used_files: row.used_files,
            })
            .unwrap_or_default()
    }

    fn adjust_stats(&self, root: &Path, delta_bytes: i64, delta_files: i64) {
        let _ = self
            .db
            .fs_quota_adjust(&root.display().to_string(), delta_bytes, delta_files);
    }

    fn limit_for(&self, path: &Path) -> Option<QuotaLimit> {
        let rows = self.db.fs_quota_list().ok()?;
        rows.into_iter()
            .filter(|row| row.max_bytes.is_some() || row.max_files.is_some())
            .filter(|row| path.starts_with(Path::new(&row.root_path)))
            .max_by_key(|row| row.root_path.len())
            .map(|row| QuotaLimit {
                root: PathBuf::from(row.root_path),
                max_bytes: row.max_bytes,
                max_files: row.max_files,
            })
    }

    fn set_usage(&self, root: &Path, usage: QuotaUsage) {
        let _ = self.db.fs_quota_set_usage(
            &root.display().to_string(),
            usage.used_bytes,
            usage.used_files,
            skiff_fs::now_secs(),
        );
    }

    fn set_limits(&self, root: &Path, max_bytes: Option<u64>, max_files: Option<u64>) {
        let _ = self
            .db
            .fs_quota_set_limits(&root.display().to_string(), max_bytes, max_files);
    }

    fn clear_limits(&self, root: &Path) {
        let _ = self.db.fs_quota_clear_limits(&root.display().to_string());
    }

    fn snapshot(&self) -> Vec<QuotaSnapshot> {
        self.db
            .fs_quota_list()
            .map(|rows| rows.into_iter().map(Self::row_to_snapshot).collect())
            .unwrap_or_default()
    }
}

/// `skiff-fs` 审计落点：写现有 `audit` 表，`ip` 列记来源 `web`。
struct DbAuditSink {
    db: Arc<Db>,
}

impl AuditSink for DbAuditSink {
    fn record(&self, ev: AuditEvent) {
        let target = ev
            .dst_path
            .clone()
            .or_else(|| ev.src_path.clone())
            .unwrap_or_default();
        let detail = serde_json::json!({
            "channel": "web",
            "actor_type": ev.actor_type,
            "code": ev.code,
            "src": ev.src_path,
            "dst": ev.dst_path,
            "bytes": ev.bytes,
            "sha256": ev.sha256,
            "site": ev.site,
            "protected": ev.protected,
            "request_id": ev.request_id,
            "count": ev.count,
            "detail": ev.detail,
            "ts": ev.ts,
        })
        .to_string();
        // 审计写入失败不阻断文件操作（best-effort，与能力面一致）。
        let _ = self.db.audit(
            &ev.actor_id,
            "web",
            &ev.action,
            &target,
            &ev.result,
            &detail,
        );
    }
}

pub struct AdminFs {
    fs: Fs,
    db: Arc<Db>,
    data_dir: PathBuf,
    seq: AtomicU64,
}

impl AdminFs {
    pub fn new(cfg: &WebConfig, db: Arc<Db>) -> Self {
        Self::with_options(
            cfg.data_dir.clone(),
            cfg.web_root.clone(),
            db,
            cfg.trash_retention_days,
        )
    }

    pub fn with_paths(data_dir: PathBuf, web_root: PathBuf, db: Arc<Db>) -> Self {
        Self::with_options(data_dir, web_root, db, 30)
    }

    pub fn with_options(
        data_dir: PathBuf,
        web_root: PathBuf,
        db: Arc<Db>,
        trash_retention_days: u32,
    ) -> Self {
        let site_root = web_root.join("sites");
        let fs_cfg = FsConfig {
            site_root: site_root.to_string_lossy().to_string(),
            trash: TrashConfig {
                root: data_dir.join("trash"),
                retention_secs: i64::from(trash_retention_days.max(1)) * 86_400,
                ..TrashConfig::default()
            },
            ..FsConfig::default()
        };
        Self {
            fs: Fs::new(
                fs_cfg,
                Arc::new(DbAuditSink {
                    db: Arc::clone(&db),
                }),
                Arc::new(DbTrashStore {
                    db: Arc::clone(&db),
                }),
                Arc::new(DbQuotaTracker {
                    db: Arc::clone(&db),
                }),
            ),
            db,
            data_dir,
            seq: AtomicU64::new(1),
        }
    }

    fn principal(user: &str) -> Principal {
        Principal::admin(user)
    }

    fn ctx(&self) -> OpContext {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        OpContext::new(format!("web-fs-{}-{}", skiff_fs::now_secs(), n))
    }

    /// 内核数据根硬拒：administrator 也不可读写 `data/`、`data/plugins`、`data/official`。
    ///
    /// 先做 lexical 校验（与 `skiff-fs` 的路径规则一致），再对「已存在的目标 / 待创建
    /// 目标的父目录」做 canonical 化，防止通过符号链接绕回数据根。
    ///
    /// `reject_root` 只对变更操作打开：浏览 `/` 是合理的，但改名/删除/新建到根上
    /// 没有语义（`skiff-fs` 也会因缺父目录而拒绝）。
    fn guard(&self, raw: &str, create: bool, reject_root: bool) -> Result<(), FsError> {
        if raw.is_empty() {
            return Err(FsError::Denied("路径不能为空".into()));
        }
        if raw.contains('\0') || raw.contains("..") {
            return Err(FsError::Denied("路径含非法片段".into()));
        }
        let p = Path::new(raw);
        if !p.is_absolute() {
            return Err(FsError::Denied("路径必须是绝对路径".into()));
        }
        let probe = if p.exists() {
            std::fs::canonicalize(p).ok()
        } else if create {
            p.parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .map(|parent| match p.file_name() {
                    Some(name) => parent.join(name),
                    None => parent,
                })
        } else {
            None
        };
        let probe = match probe {
            Some(p) => p,
            None => {
                return Err(FsError::Denied(format!(
                    "无法解析目标路径（父目录不存在或不可访问）: {raw}"
                )))
            }
        };
        if skiff_plugin::fsgrant::is_protected(&self.data_dir, &probe) {
            return Err(FsError::Denied(
                "内核数据目录受保护，文件管理不能访问（skiff.db / plugins / official）".into(),
            ));
        }
        if reject_root && is_root(&probe) {
            return Err(FsError::Denied("不能操作根目录".into()));
        }
        Ok(())
    }

    fn is_protected_child(&self, child: &Path) -> bool {
        skiff_plugin::fsgrant::hides_listed_entry(&self.data_dir, child)
    }

    pub fn list(&self, user: &str, path: &str) -> Result<Listing, FsError> {
        self.guard(path, false, false)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        let page = self.fs.list(
            &p,
            &ctx,
            &ListRequest {
                path: path.to_string(),
                page: 1,
                page_size: 1000,
            },
        )?;
        // 归一后的目录路径：用于拼子项与硬拒判定（list 本身不返回目录名）。
        let dir = self.fs.resolve(&p, path, skiff_fs::PathMode::Existing)?;
        let mut entries = Vec::with_capacity(page.entries.len());
        for e in page.entries {
            let child = dir.path.join(&e.name);
            if self.is_protected_child(&child) {
                continue;
            }
            entries.push(DirEntry {
                name: e.name,
                dir: e.kind == skiff_fs::PathKind::Dir,
                symlink: e.kind == skiff_fs::PathKind::Symlink,
                size: e.size,
                mtime: e.mtime,
            });
        }
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(Listing {
            path: dir.path.display().to_string(),
            parent: dir.path.parent().map(|p| p.display().to_string()),
            entries,
            truncated: page.has_more || page.total > 1000,
        })
    }

    /// 读取文本文件。二进制与超限文件返回 `truncated`/错误信息，页面据此只读。
    pub fn read(&self, user: &str, path: &str) -> Result<FileText, FsError> {
        self.guard(path, false, false)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        let chunk = self.fs.read(
            &p,
            &ctx,
            &ReadRequest {
                path: path.to_string(),
                offset: 0,
                max_bytes: MAX_EDIT_BYTES,
            },
        )?;
        let text = String::from_utf8(chunk.bytes).map_err(|_| {
            FsError::Unsupported("二进制文件不支持在页面编辑（请用下载或命令行）".into())
        })?;
        Ok(FileText {
            path: chunk.path,
            text,
            size: chunk.size,
            truncated: !chunk.eof,
        })
    }

    /// 原子替换写。超限只读文件在此再次拒绝，避免「读到一半再保存」把原文件截断。
    pub fn write(&self, user: &str, path: &str, content: &str) -> Result<WriteResult, FsError> {
        self.guard(path, true, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.is_dir() {
                return Err(FsError::Conflict("目标是目录，不能写入".into()));
            }
            if meta.len() > MAX_EDIT_BYTES as u64 {
                return Err(FsError::Denied(
                    "文件超过 256 KiB 编辑上限，页面只读；请用命令行或下载后处理".into(),
                ));
            }
        }
        if content.len() > MAX_EDIT_BYTES {
            return Err(FsError::TooLarge(format!(
                "内容 {} 字节超过 {} 字节上限",
                content.len(),
                MAX_EDIT_BYTES
            )));
        }
        let out = self.fs.write(
            &p,
            &ctx,
            &WriteRequest {
                path: path.to_string(),
                data: content.as_bytes().to_vec(),
                mode: WriteMode::Replace,
            },
        )?;
        Ok(WriteResult {
            path: out.path,
            size: out.size,
            sha256: out.sha256,
        })
    }

    pub fn mkdir(&self, user: &str, path: &str) -> Result<String, FsError> {
        self.guard(path, true, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.mkdir(&p, &ctx, path).map(|o| o.path)
    }

    pub fn rename(&self, user: &str, from: &str, to: &str) -> Result<(), FsError> {
        self.guard(from, false, true)?;
        self.guard(to, true, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.rename(&p, &ctx, from, to, false)?;
        Ok(())
    }

    /// 删除改为移入回收站；返回 `trash_id` 供页面提示「可恢复」。
    pub fn remove(&self, user: &str, path: &str) -> Result<RemoveResult, FsError> {
        self.guard(path, false, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        let out = self.fs.remove(&p, &ctx, path, false)?;
        Ok(RemoveResult {
            path: out.path,
            trash_id: out.trash_id,
        })
    }

    /// 回收站列表（管理员可见全部；插件条目只读展示，恢复仍受路径硬拒）。
    pub fn trash_list(&self, user: &str) -> Result<Vec<TrashItem>, FsError> {
        let p = Self::principal(user);
        let ctx = self.ctx();
        let rows = self.fs.trash_list(&p, &ctx)?;
        Ok(rows.into_iter().map(TrashItem::from).collect())
    }

    /// 恢复原位置。恢复前再次做受保护路径硬拒，避免旧条目绕过页面守卫。
    pub fn trash_restore(
        &self,
        user: &str,
        id: &str,
        overwrite: bool,
    ) -> Result<RestoreOutcome, FsError> {
        let entry = self
            .db
            .trash_get(id)
            .map_err(|e| FsError::Io(format!("读取回收站条目失败: {e}")))?
            .ok_or_else(|| FsError::NotFound("回收站条目不存在或已清理".into()))?;
        self.guard(&entry.original_path, true, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.restore(&p, &ctx, id, overwrite)
    }

    /// 彻底删除单条。
    pub fn trash_purge(&self, user: &str, id: &str) -> Result<(), FsError> {
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.purge(&p, &ctx, id)?;
        Ok(())
    }

    /// 清空回收站（逐条彻底删除，失败条目保留并回报）。
    pub fn trash_clear(&self, user: &str) -> Result<ClearTrashOutcome, FsError> {
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.clear_trash(&p, &ctx)
    }

    /// 到期清理（内置任务 `trash-prune` 调用）。返回释放字节数。
    pub fn trash_prune(&self) -> Result<u64, FsError> {
        let ctx = self.ctx();
        self.fs.prune_trash(&ctx)
    }

    /// 配额列表：只展示已设置上限的条目；受保护路径同样过滤掉（防御旧数据/手工写库）。
    pub fn quota_list(&self, user: &str) -> Result<Vec<QuotaSnapshot>, FsError> {
        let p = Self::principal(user);
        let rows = self.fs.quota_list(&p)?;
        Ok(rows
            .into_iter()
            .filter(|q| q.max_bytes.is_some() || q.max_files.is_some())
            .filter(|q| self.guard(&q.root, false, true).is_ok())
            .collect())
    }

    pub fn quota_set(
        &self,
        user: &str,
        root: &str,
        max_bytes: Option<u64>,
        max_files: Option<u64>,
    ) -> Result<QuotaSnapshot, FsError> {
        self.guard(root, false, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.quota_set(&p, &ctx, root, max_bytes, max_files)
    }

    pub fn quota_cancel(&self, user: &str, root: &str) -> Result<QuotaSnapshot, FsError> {
        self.guard(root, false, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.quota_cancel(&p, &ctx, root)
    }

    pub fn quota_scan(&self, user: &str, root: &str) -> Result<QuotaSnapshot, FsError> {
        self.guard(root, false, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.quota_scan(&p, &ctx, root)
    }
}

fn is_root(p: &Path) -> bool {
    p.parent().is_none() || p.components().count() <= 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn db() -> Arc<Db> {
        Arc::new(Db::open_in_memory().unwrap())
    }

    fn new_admin(web: &Path, data: &Path) -> AdminFs {
        AdminFs::with_paths(data.to_path_buf(), web.to_path_buf(), db())
    }

    #[test]
    fn round_trip_list_read_write_rename_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let file = web.join("sites/a.conf");
        fs::write(&file, "old").unwrap();

        let listed = admin.list("admin", web.to_str().unwrap()).unwrap();
        assert!(listed.entries.iter().any(|e| e.name == "sites"));

        let read = admin.read("admin", file.to_str().unwrap()).unwrap();
        assert_eq!(read.text, "old");
        assert!(!read.truncated);

        admin.write("admin", file.to_str().unwrap(), "new").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");

        let to = web.join("sites/b.conf");
        admin
            .rename("admin", file.to_str().unwrap(), to.to_str().unwrap())
            .unwrap();
        assert!(!file.exists());
        assert!(to.exists());

        admin.remove("admin", to.to_str().unwrap()).unwrap();
        assert!(!to.exists());
    }

    #[test]
    fn data_root_is_hard_denied_even_for_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(data.join("plugins")).unwrap();
        fs::write(data.join("skiff.db"), b"db").unwrap();
        // 通过软链绕回数据根也必须被拒。
        #[cfg(unix)]
        std::os::unix::fs::symlink(data.join("skiff.db"), web.join("evil.db")).unwrap();

        let admin = new_admin(&web, &data);
        let denied = admin
            .read("admin", data.join("skiff.db").to_str().unwrap())
            .unwrap_err();
        assert!(matches!(denied, FsError::Denied(_)));
        let denied_plugins = admin
            .list("admin", data.join("plugins").to_str().unwrap())
            .unwrap_err();
        assert!(matches!(denied_plugins, FsError::Denied(_)));
        let denied_write = admin
            .write("admin", data.join("skiff.db").to_str().unwrap(), "x")
            .unwrap_err();
        assert!(matches!(denied_write, FsError::Denied(_)));
        let denied_root = admin.remove("admin", "/").unwrap_err();
        assert!(matches!(denied_root, FsError::Denied(_)));

        // 列表里不能出现数据根的任何子项。
        let listed = admin.list("admin", web.to_str().unwrap()).unwrap();
        assert!(!listed.entries.iter().any(|e| e.name == "evil.db"));
    }

    #[test]
    fn oversized_file_is_readonly_and_write_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let big = web.join("big.log");
        let content = "x".repeat(MAX_EDIT_BYTES + 10);
        fs::write(&big, &content).unwrap();

        let read = admin.read("admin", big.to_str().unwrap()).unwrap();
        assert!(read.truncated);
        assert_eq!(read.text.len(), MAX_EDIT_BYTES);

        let err = admin
            .write("admin", big.to_str().unwrap(), "truncated")
            .unwrap_err();
        assert!(matches!(err, FsError::Denied(_)));
        assert_eq!(fs::read_to_string(&big).unwrap(), content);
    }

    // ---- 回收站 ---------------------------------------------------------

    #[test]
    fn remove_moves_to_trash_and_restore_brings_back() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let file = web.join("sites/keep.txt");
        fs::write(&file, "hello").unwrap();

        let out = admin.remove("admin", file.to_str().unwrap()).unwrap();
        assert!(out.trash_id.is_some(), "删除必须进回收站");
        assert!(!file.exists());

        let items = admin.trash_list("admin").unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].original_path.ends_with("keep.txt"));
        assert_eq!(items[0].kind, "file");
        assert_eq!(items[0].files, 1);

        let id = out.trash_id.unwrap();
        admin.trash_restore("admin", &id, false).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        assert!(
            admin.trash_list("admin").unwrap().is_empty(),
            "恢复后离开回收站"
        );
    }

    #[test]
    fn restore_conflict_needs_explicit_overwrite_and_parent_missing_is_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites/sub")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let file = web.join("sites/sub/conflict.txt");
        fs::write(&file, "old").unwrap();
        let out = admin.remove("admin", file.to_str().unwrap()).unwrap();
        let id = out.trash_id.unwrap();

        fs::write(&file, "new").unwrap();
        let err = admin.trash_restore("admin", &id, false).unwrap_err();
        assert!(matches!(err, FsError::Conflict(_)), "{err:?}");
        assert!(err.message().contains("同名对象"));

        admin.trash_restore("admin", &id, true).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "old",
            "覆盖恢复取回收站版本"
        );

        // 父目录被删后恢复失败要有可读提示，不能静默。
        let nested = web.join("sites/sub/nested.txt");
        fs::write(&nested, "x").unwrap();
        let out = admin.remove("admin", nested.to_str().unwrap()).unwrap();
        let id = out.trash_id.unwrap();
        fs::remove_dir_all(web.join("sites/sub")).unwrap();
        let err = admin.trash_restore("admin", &id, false).unwrap_err();
        assert!(
            err.message().contains("父目录") || err.message().contains("parent"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn trash_purge_and_clear_remove_objects() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let a = web.join("sites/a.txt");
        let b = web.join("sites/b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let id = admin.remove("admin", a.to_str().unwrap()).unwrap().trash_id;
        admin.remove("admin", b.to_str().unwrap()).unwrap();

        admin.trash_purge("admin", id.as_deref().unwrap()).unwrap();
        assert_eq!(admin.trash_list("admin").unwrap().len(), 1);

        let out = admin.trash_clear("admin").unwrap();
        assert_eq!(out.removed, 1);
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        assert!(admin.trash_list("admin").unwrap().is_empty());
    }

    #[test]
    fn restore_of_protected_original_path_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("skiff.db"), b"db").unwrap();
        let db = db();
        let admin = AdminFs::with_paths(data.clone(), web.clone(), db.clone());
        let row = TrashRow {
            id: "legacy-1".into(),
            actor_type: "admin".into(),
            actor_id: "admin".into(),
            original_path: data.join("skiff.db").display().to_string(),
            trash_path: data.join("trash/legacy-1").display().to_string(),
            kind: "file".into(),
            size: 2,
            files: 1,
            mode: None,
            owner: None,
            sha256: None,
            site: None,
            protected: false,
            deleted_at: 1_000,
            expires_at: 2_000,
            nlink: 1,
            restored_at: None,
            purged_at: None,
            quota_root: String::new(),
        };
        db.trash_insert(&row).unwrap();
        let err = admin.trash_restore("admin", "legacy-1", false).unwrap_err();
        assert!(matches!(err, FsError::Denied(_)), "{err:?}");
    }

    // ---- 配额 -----------------------------------------------------------

    #[test]
    fn quota_set_rejects_writes_and_scan_updates_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let admin = new_admin(&web, &data);
        let root = web.join("sites").display().to_string();

        let q = admin.quota_set("admin", &root, Some(200), Some(1)).unwrap();
        assert_eq!(q.max_bytes, Some(200));
        assert_eq!(q.max_files, Some(1));

        let first = web.join("sites/first.bin");
        admin
            .write("admin", first.to_str().unwrap(), &"x".repeat(120))
            .unwrap();
        let second = web.join("sites/second.bin");
        let err = admin
            .write("admin", second.to_str().unwrap(), &"x".repeat(120))
            .unwrap_err();
        assert!(matches!(err, FsError::QuotaExceeded(_)), "{err:?}");
        assert!(err.message().contains("体积") || err.message().contains("容量"));

        let scan = admin.quota_scan("admin", &root).unwrap();
        assert_eq!(scan.used_bytes, 120);
        assert_eq!(scan.used_files, 1);
        assert!(admin.quota_list("admin").unwrap().len() == 1);

        let cancel = admin.quota_cancel("admin", &root).unwrap();
        assert_eq!((cancel.max_bytes, cancel.max_files), (None, None));
        admin
            .write("admin", second.to_str().unwrap(), &"x".repeat(150))
            .unwrap();
        assert!(
            admin.quota_list("admin").unwrap().is_empty(),
            "取消后不再展示"
        );
    }

    #[test]
    fn quota_on_protected_root_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let web = tmp.path().join("www");
        let data = tmp.path().join("skiff");
        fs::create_dir_all(web.join("sites")).unwrap();
        fs::create_dir_all(data.join("plugins")).unwrap();
        let admin = new_admin(&web, &data);
        let err = admin
            .quota_set(
                "admin",
                data.join("plugins").to_str().unwrap(),
                Some(1),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, FsError::Denied(_)));
    }
}
