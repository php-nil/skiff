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
    AuditEvent, AuditSink, Fs, FsConfig, FsError, ListRequest, MemoryQuotaTracker,
    MemoryTrashStore, OpContext, Principal, ReadRequest, TrashConfig, WriteMode, WriteRequest,
    MAX_REQUEST_BYTES,
};
use skiff_store::Db;

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
    data_dir: PathBuf,
    seq: AtomicU64,
}

impl AdminFs {
    pub fn new(cfg: &WebConfig, db: Arc<Db>) -> Self {
        Self::with_paths(cfg.data_dir.clone(), cfg.web_root.clone(), db)
    }

    pub fn with_paths(data_dir: PathBuf, web_root: PathBuf, db: Arc<Db>) -> Self {
        let site_root = web_root.join("sites");
        let fs_cfg = FsConfig {
            site_root: site_root.to_string_lossy().to_string(),
            trash: TrashConfig {
                root: data_dir.join("trash"),
                ..TrashConfig::default()
            },
            ..FsConfig::default()
        };
        Self {
            fs: Fs::new(
                fs_cfg,
                Arc::new(DbAuditSink { db }),
                Arc::new(MemoryTrashStore::new()),
                Arc::new(MemoryQuotaTracker::new()),
            ),
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

    /// 第一步删除为永久删除（无回收站 UI）。第二步接入 SQLite 回收站时改为 `permanent = false`。
    pub fn remove(&self, user: &str, path: &str) -> Result<(), FsError> {
        self.guard(path, false, true)?;
        let p = Self::principal(user);
        let ctx = self.ctx();
        self.fs.remove(&p, &ctx, path, true)?;
        Ok(())
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
}
