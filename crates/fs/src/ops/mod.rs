//! 内核文件原语操作面（契约 §3）：`list / stat / read / write / mkdir / rename /
//! copy / remove`，外加回收站 `restore / purge / prune`。
//!
//! 每笔操作都走同一条链路：`resolve`（授权 + 不跟随链接）→ 路径锁 → 实际 I/O
//! → 审计（成功逐条或聚合，失败逐条）。管理面与插件面的差异只在 `Principal`。

mod archive;
mod attr;
mod copy;
mod io;
mod mutate;

use std::path::PathBuf;
use std::sync::Arc;

use crate::audit::{AuditAction, AuditEvent, AuditResult, AuditSink};
use crate::config::{FsConfig, QuotaLimit, QuotaSnapshot, QuotaTracker, QuotaUsage};
use crate::error::{io_error, FsError};
use crate::lock::PathLocks;
use crate::path::{self, PathKind, PathMode, ResolvedPath};
use crate::subject::{Principal, Scope};
use crate::trash::{TrashEntry, TrashStore};

pub use archive::{ArchiveFormat, ArchiveOutcome, ArchiveRequest, ExtractOutcome, ExtractRequest};
pub use attr::{ChmodOutcome, ChownOutcome};
pub use copy::CopyLimits;

/// 一次调用的请求上下文：`request_id` 贯穿审计与错误返回；`now` 可注入便于测试。
#[derive(Debug, Clone)]
pub struct OpContext {
    pub request_id: String,
    pub now: i64,
}

impl OpContext {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            now: now_secs(),
        }
    }

    pub fn at(request_id: impl Into<String>, now: i64) -> Self {
        Self {
            request_id: request_id.into(),
            now,
        }
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct ListRequest {
    pub path: String,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub kind: PathKind,
    pub size: u64,
    pub mtime: Option<i64>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub link_target: Option<String>,
    pub nlink: u64,
}

#[derive(Debug, Clone)]
pub struct ListPage {
    pub entries: Vec<EntryInfo>,
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub has_more: bool,
    /// 名称非 UTF-8 而被跳过的条目数（契约要求拒绝策略，不返回不可操作名字）。
    pub skipped_non_utf8: usize,
}

#[derive(Debug, Clone)]
pub struct StatRequest {
    pub path: String,
    /// 是否计算 sha256（默认不算；仅普通文件）。
    pub with_hash: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatInfo {
    pub path: String,
    pub kind: PathKind,
    pub size: u64,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub mtime: Option<i64>,
    pub nlink: u64,
    pub link_target: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub path: String,
    pub offset: u64,
    pub max_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ReadChunk {
    pub path: String,
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub size: u64,
    pub eof: bool,
    /// 请求超过单次上限、被截断时为 `true`（不静默放大）。
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 临时文件 + fsync + rename，原子替换。
    Replace,
    /// `O_APPEND`，显式非原子。
    Append,
    /// 定位写，显式非原子（契约未单列，见实现说明）。
    At(u64),
}

#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: WriteMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub path: String,
    pub bytes: u64,
    pub size: u64,
    pub sha256: String,
    pub atomic: bool,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkdirOutcome {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub path: String,
    pub kind: PathKind,
    pub permanent: bool,
    pub trash_id: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RenameOutcome {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CopyOutcome {
    pub from: String,
    pub to: String,
    pub bytes: u64,
    pub entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RestoreOutcome {
    pub trash_id: String,
    pub restored_to: String,
}

/// 清空回收站的结果：失败条目逐条保留，不静默吞掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearTrashOutcome {
    pub removed: u64,
    pub bytes: u64,
    pub failed: Vec<String>,
}

/// 文件原语服务。线程安全；内部状态只有路径锁表。
pub struct Fs {
    cfg: FsConfig,
    locks: PathLocks,
    audit: Arc<dyn AuditSink>,
    trash: Arc<dyn TrashStore>,
    quota: Arc<dyn QuotaTracker>,
}

impl Fs {
    pub fn new(
        cfg: FsConfig,
        audit: Arc<dyn AuditSink>,
        trash: Arc<dyn TrashStore>,
        quota: Arc<dyn QuotaTracker>,
    ) -> Self {
        Self {
            cfg,
            locks: PathLocks::new(),
            audit,
            trash,
            quota,
        }
    }

    pub fn config(&self) -> &FsConfig {
        &self.cfg
    }

    pub fn resolve(
        &self,
        p: &Principal,
        raw: &str,
        mode: PathMode,
    ) -> Result<ResolvedPath, FsError> {
        path::resolve(p, raw, mode, &self.cfg.site_root)
    }

    pub(crate) fn trash_store(&self) -> &dyn TrashStore {
        self.trash.as_ref()
    }

    pub(crate) fn quota(&self) -> &dyn QuotaTracker {
        self.quota.as_ref()
    }

    pub(crate) fn audit_sink_record(&self, event: crate::audit::AuditEvent) {
        self.audit.record(event);
    }

    // ---- list / stat / read / mkdir -------------------------------------

    pub fn list(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &ListRequest,
    ) -> Result<ListPage, FsError> {
        let r = match self.resolve(p, &req.path, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::List, &req.path, e),
        };
        if r.kind != PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::List,
                &r,
                FsError::Conflict("目标不是目录".into()),
            );
        }
        let out =
            self.locks
                .with_read(std::slice::from_ref(&r.path), self.cfg.lock_timeout, || {
                    self.list_inner(&r, req)
                })?;
        match out {
            Ok(page) => {
                let mut a = ok_args(AuditAction::List);
                a.set_from_resolved(&r);
                self.audit(p, ctx, a);
                Ok(page)
            }
            Err(e) => self.fail_resolved(p, ctx, AuditAction::List, &r, e),
        }
    }

    fn list_inner(&self, r: &ResolvedPath, req: &ListRequest) -> Result<ListPage, FsError> {
        let mut entries = Vec::new();
        let mut skipped_non_utf8 = 0usize;
        let mut total = 0usize;
        for item in std::fs::read_dir(&r.path).map_err(|e| io_error(&r.path, e))? {
            let item = item.map_err(|e| io_error(&r.path, e))?;
            total += 1;
            if total > self.cfg.max_dir_entries {
                return Err(FsError::TooManyEntries(format!(
                    "目录条目超过上限 {}",
                    self.cfg.max_dir_entries
                )));
            }
            let name = match item.file_name().into_string() {
                Ok(name) => name,
                Err(_) => {
                    skipped_non_utf8 += 1;
                    continue;
                }
            };
            let meta = item.metadata().map_err(|e| io_error(&item.path(), e))?;
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
            let link_target = if kind == PathKind::Symlink {
                std::fs::read_link(item.path())
                    .ok()
                    .and_then(|t| t.to_str().map(str::to_string))
            } else {
                None
            };
            entries.push(EntryInfo {
                name,
                kind,
                size: if kind == PathKind::File {
                    meta.len()
                } else {
                    0
                },
                mtime: io::mtime_of(&meta),
                mode: io::mode_of(&meta),
                owner: io::owner_of(&meta),
                link_target,
                nlink: io::nlink_of(&meta),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let page = req.page.max(1);
        let page_size = if req.page_size == 0 {
            100
        } else {
            req.page_size.min(500)
        };
        let start = (page - 1).saturating_mul(page_size);
        let end = (start + page_size).min(entries.len());
        let slice = if start < entries.len() {
            entries[start..end].to_vec()
        } else {
            Vec::new()
        };
        let has_more = end < entries.len();
        Ok(ListPage {
            entries: slice,
            page,
            page_size,
            total,
            has_more,
            skipped_non_utf8,
        })
    }

    pub fn stat(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &StatRequest,
    ) -> Result<StatInfo, FsError> {
        let r = match self.resolve(p, &req.path, PathMode::ExistingLink) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Stat, &req.path, e),
        };
        let out =
            self.locks
                .with_read(std::slice::from_ref(&r.path), self.cfg.lock_timeout, || {
                    self.stat_inner(&r, req.with_hash)
                })?;
        match out {
            Ok(info) => {
                let mut a = ok_args(AuditAction::Stat);
                a.set_from_resolved(&r);
                a.hash = info.sha256.clone();
                self.audit(p, ctx, a);
                Ok(info)
            }
            Err(e) => self.fail_resolved(p, ctx, AuditAction::Stat, &r, e),
        }
    }

    fn stat_inner(&self, r: &ResolvedPath, with_hash: bool) -> Result<StatInfo, FsError> {
        let meta = std::fs::symlink_metadata(&r.path).map_err(|e| io_error(&r.path, e))?;
        let link_target = if meta.file_type().is_symlink() {
            std::fs::read_link(&r.path)
                .ok()
                .and_then(|t| t.to_str().map(str::to_string))
        } else {
            None
        };
        let sha256 = if with_hash && meta.is_file() {
            let bytes = std::fs::read(&r.path).map_err(|e| io_error(&r.path, e))?;
            Some(crate::hash::sha256_hex(&bytes))
        } else {
            None
        };
        Ok(StatInfo {
            path: r.path.display().to_string(),
            kind: r.kind,
            size: if meta.is_file() { meta.len() } else { 0 },
            mode: io::mode_of(&meta),
            owner: io::owner_of(&meta),
            mtime: io::mtime_of(&meta),
            nlink: io::nlink_of(&meta),
            link_target,
            sha256,
        })
    }

    pub fn read(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &ReadRequest,
    ) -> Result<ReadChunk, FsError> {
        let r = match self.resolve(p, &req.path, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Read, &req.path, e),
        };
        if r.kind == PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Read,
                &r,
                FsError::Conflict("目标是目录".into()),
            );
        }
        let out =
            self.locks
                .with_read(std::slice::from_ref(&r.path), self.cfg.lock_timeout, || {
                    self.read_inner(&r, req)
                })?;
        match out {
            Ok(chunk) => {
                let mut a = ok_args(AuditAction::Read);
                a.set_from_resolved(&r);
                a.bytes = chunk.bytes.len() as u64;
                self.audit(p, ctx, a);
                Ok(chunk)
            }
            Err(e) => self.fail_resolved(p, ctx, AuditAction::Read, &r, e),
        }
    }

    fn read_inner(&self, r: &ResolvedPath, req: &ReadRequest) -> Result<ReadChunk, FsError> {
        let cap = self.cfg.max_request_bytes;
        let requested = if req.max_bytes == 0 {
            cap
        } else {
            req.max_bytes
        };
        let max = requested.min(cap);
        let truncated = requested > cap;
        let (bytes, size, eof) = io::read_range(&r.path, req.offset, max)?;
        Ok(ReadChunk {
            path: r.path.display().to_string(),
            bytes,
            offset: req.offset,
            size,
            eof,
            truncated,
        })
    }

    pub fn mkdir(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw: &str,
    ) -> Result<MkdirOutcome, FsError> {
        let r = match self.resolve(p, raw, PathMode::Create) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Mkdir, raw, e),
        };
        if r.kind != PathKind::Missing {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Mkdir,
                &r,
                FsError::Conflict("目标已存在".into()),
            );
        }
        let check = self.ensure_write_allowed(p, &r);
        if let Err(e) = check {
            return self.fail_resolved(p, ctx, AuditAction::Mkdir, &r, e);
        }
        let out =
            self.locks
                .with_write(std::slice::from_ref(&r.path), self.cfg.lock_timeout, || {
                    std::fs::create_dir(&r.path).map_err(|e| io_error(&r.path, e))
                })?;
        match out {
            Ok(()) => {
                let mut a = ok_args(AuditAction::Mkdir);
                a.set_from_resolved(&r);
                self.audit(p, ctx, a);
                Ok(MkdirOutcome {
                    path: r.path.display().to_string(),
                })
            }
            Err(e) => self.fail_resolved(p, ctx, AuditAction::Mkdir, &r, e),
        }
    }

    // ---- 授助与配额 -----------------------------------------------------

    /// 受保护路径（`log/ssl/conf`）：插件默认只读，需额外开关；管理员放行。
    pub(crate) fn ensure_write_allowed(
        &self,
        p: &Principal,
        r: &ResolvedPath,
    ) -> Result<(), FsError> {
        if r.protected && !p.subject().is_admin() && !p.allow_protected_write() {
            return Err(FsError::Protected(format!(
                "受保护路径（需 fs:write-protected）: {}",
                r.path.display()
            )));
        }
        Ok(())
    }

    /// 内核受保护路径：删除/改名/覆盖写一律硬拒（管理员也不例外）。
    ///
    /// 与 `ensure_write_allowed` 的站点 `log/ssl/conf` 是两套口径：这里保护的是
    /// 内核自身运行所需的配置/数据，属于「碰了就起不来」的路径。
    pub(crate) fn ensure_not_protected(
        &self,
        r: &ResolvedPath,
        action: &str,
    ) -> Result<(), FsError> {
        if let Some(root) = crate::path::protected_hit(&self.cfg.protected_paths, &r.path) {
            return Err(FsError::Protected(format!(
                "内核运行配置或数据目录受保护，不能{}: {}（命中 {}）",
                action,
                r.path.display(),
                root.display()
            )));
        }
        Ok(())
    }

    /// 配额 key：插件按命中授权根；管理员按站点目录（无站点则为空）。
    pub(crate) fn quota_key(&self, p: &Principal, r: &ResolvedPath) -> Option<PathBuf> {
        if p.subject().is_admin() {
            let site = r.site.as_ref()?;
            Some(PathBuf::from(&self.cfg.site_root).join(site))
        } else {
            r.grant_root.clone()
        }
    }

    /// 命中的配额规则：先取按树配置（最长祖先根），否则退回旧的
    /// 「授权根/站点目录 + 全局 max_root_bytes」口径。
    pub(crate) fn quota_target(&self, p: &Principal, r: &ResolvedPath) -> Option<QuotaLimit> {
        if let Some(limit) = self.quota.limit_for(&r.path) {
            return Some(limit);
        }
        self.quota_key(p, r).map(|root| QuotaLimit {
            root,
            max_bytes: Some(self.cfg.max_root_bytes),
            max_files: None,
        })
    }

    /// 写前配额预检：`delta_bytes/files` 是本次操作对目标树用量的增量。
    /// 超限时返回带根路径与具体数字的可读错误。
    pub(crate) fn enforce_quota(
        &self,
        target: Option<&QuotaLimit>,
        delta_bytes: i64,
        delta_files: i64,
    ) -> Result<(), FsError> {
        let Some(t) = target else {
            return Ok(());
        };
        let usage = self.quota.usage_stats(&t.root);
        let projected_bytes = usage.used_bytes.saturating_add_signed(delta_bytes);
        let projected_files = usage.used_files.saturating_add_signed(delta_files);
        if let Some(max) = t.max_bytes {
            if projected_bytes > max {
                return Err(FsError::QuotaExceeded(format!(
                    "目录容量配额超限：{} 写入后 {} 字节 > 上限 {} 字节",
                    t.root.display(),
                    projected_bytes,
                    max
                )));
            }
        }
        if let Some(max) = t.max_files {
            if projected_files > max {
                return Err(FsError::QuotaExceeded(format!(
                    "目录文件数配额超限：{} 写入后 {} 个文件 > 上限 {} 个",
                    t.root.display(),
                    projected_files,
                    max
                )));
            }
        }
        Ok(())
    }

    /// 回收站列表：管理员看全部；其他主体只看自己的条目。
    pub fn trash_list(&self, p: &Principal, ctx: &OpContext) -> Result<Vec<TrashEntry>, FsError> {
        let mut rows: Vec<TrashEntry> = self
            .trash_store()
            .list()
            .into_iter()
            .filter(|e| e.is_live() && can_touch_entry(p, e))
            .collect();
        rows.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then(b.id.cmp(&a.id)));
        let bytes: u64 = rows.iter().map(|e| e.size).sum();
        let mut a = ok_args(AuditAction::TrashList);
        a.bytes = bytes;
        a.detail = Some(format!("entries={}", rows.len()));
        self.audit(p, ctx, a);
        Ok(rows)
    }

    /// 清空回收站（仅管理员）。逐条彻底删除，失败的条目保留并回报。
    pub fn clear_trash(
        &self,
        p: &Principal,
        ctx: &OpContext,
    ) -> Result<ClearTrashOutcome, FsError> {
        if !p.subject().is_admin() {
            return Err(FsError::Denied("清空回收站仅管理员可用".into()));
        }
        let rows: Vec<TrashEntry> = self
            .trash_store()
            .list()
            .into_iter()
            .filter(|e| e.is_live())
            .collect();
        let mut out = ClearTrashOutcome {
            removed: 0,
            bytes: 0,
            failed: Vec::new(),
        };
        for entry in rows {
            let tpath = PathBuf::from(&entry.trash_path);
            let res =
                self.locks
                    .with_write(std::slice::from_ref(&tpath), self.cfg.lock_timeout, || {
                        io::remove_object(&tpath)
                    });
            match res {
                Ok(Ok(_)) => {
                    self.trash_store().remove(&entry.id);
                    out.removed += 1;
                    out.bytes = out.bytes.saturating_add(entry.size);
                }
                Ok(Err(e)) | Err(e) => {
                    out.failed.push(format!("{}: {}", entry.id, e.message()));
                }
            }
        }
        let mut a = AuditArgs::new(
            AuditAction::TrashClear,
            if out.failed.is_empty() {
                AuditResult::Ok
            } else {
                AuditResult::Fail
            },
        );
        a.bytes = out.bytes;
        a.detail = Some(format!(
            "removed={}, failed={}",
            out.removed,
            out.failed.len()
        ));
        self.audit(p, ctx, a);
        Ok(out)
    }

    /// 配额列表（管理员）。不审计，避免页面刷新产生噪音。
    pub fn quota_list(&self, p: &Principal) -> Result<Vec<QuotaSnapshot>, FsError> {
        if !p.subject().is_admin() {
            return Err(FsError::Denied("配额查看仅管理员可用".into()));
        }
        Ok(self.quota.snapshot())
    }

    /// 重新统计某个目录树并刷新缓存计数（成本 O(条目数)）。
    pub fn quota_scan(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw_root: &str,
    ) -> Result<QuotaSnapshot, FsError> {
        let r = match self.resolve(p, raw_root, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => {
                return Err(self.quota_fail(p, ctx, AuditAction::QuotaScan, raw_root, e));
            }
        };
        if r.kind != PathKind::Dir {
            let e = FsError::Conflict("配额根必须是目录".into());
            return Err(self.quota_fail_resolved(p, ctx, AuditAction::QuotaScan, &r, e));
        }
        let usage = match io::scan_usage(&r.path, self.cfg.max_dir_entries) {
            Ok(u) => u,
            Err(e) => return Err(self.quota_fail_resolved(p, ctx, AuditAction::QuotaScan, &r, e)),
        };
        self.quota.set_usage(&r.path, usage);
        let mut a = ok_args(AuditAction::QuotaScan);
        a.set_from_resolved(&r);
        a.bytes = usage.used_bytes;
        a.detail = Some(format!("files={}", usage.used_files));
        self.audit(p, ctx, a);
        Ok(self.snapshot_for(&r.path, usage))
    }

    /// 设置/修改配额：先全量统计一次，再写上限；已用量以扫描结果为准。
    pub fn quota_set(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw_root: &str,
        max_bytes: Option<u64>,
        max_files: Option<u64>,
    ) -> Result<QuotaSnapshot, FsError> {
        let r = match self.resolve(p, raw_root, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => {
                return Err(self.quota_fail(p, ctx, AuditAction::QuotaSet, raw_root, e));
            }
        };
        if r.kind != PathKind::Dir {
            let e = FsError::Conflict("配额根必须是目录".into());
            return Err(self.quota_fail_resolved(p, ctx, AuditAction::QuotaSet, &r, e));
        }
        if max_bytes.is_none() && max_files.is_none() {
            let e = FsError::Conflict("容量上限与文件数上限至少设置一项".into());
            return Err(self.quota_fail_resolved(p, ctx, AuditAction::QuotaSet, &r, e));
        }
        let usage = match io::scan_usage(&r.path, self.cfg.max_dir_entries) {
            Ok(u) => u,
            Err(e) => return Err(self.quota_fail_resolved(p, ctx, AuditAction::QuotaSet, &r, e)),
        };
        self.quota.set_usage(&r.path, usage);
        self.quota.set_limits(&r.path, max_bytes, max_files);
        let mut a = ok_args(AuditAction::QuotaSet);
        a.set_from_resolved(&r);
        a.bytes = usage.used_bytes;
        a.detail = Some(format!(
            "max_bytes={}, max_files={}, files={}",
            max_bytes
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            max_files
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into()),
            usage.used_files
        ));
        self.audit(p, ctx, a);
        Ok(self.snapshot_for(&r.path, usage))
    }

    /// 取消配额上限：保留用量计数，页面继续显示「未设置上限」。
    pub fn quota_cancel(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw_root: &str,
    ) -> Result<QuotaSnapshot, FsError> {
        let r = match self.resolve(p, raw_root, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => {
                return Err(self.quota_fail(p, ctx, AuditAction::QuotaCancel, raw_root, e));
            }
        };
        self.quota.clear_limits(&r.path);
        let usage = self.quota.usage_stats(&r.path);
        let mut a = ok_args(AuditAction::QuotaCancel);
        a.set_from_resolved(&r);
        a.bytes = usage.used_bytes;
        self.audit(p, ctx, a);
        Ok(self.snapshot_for(&r.path, usage))
    }

    fn snapshot_for(&self, root: &std::path::Path, usage: QuotaUsage) -> QuotaSnapshot {
        self.quota
            .snapshot()
            .into_iter()
            .find(|s| s.root == root.display().to_string())
            .unwrap_or_else(|| QuotaSnapshot {
                root: root.display().to_string(),
                max_bytes: None,
                max_files: None,
                used_bytes: usage.used_bytes,
                used_files: usage.used_files,
                updated_at: None,
            })
    }

    fn quota_fail(
        &self,
        p: &Principal,
        ctx: &OpContext,
        action: AuditAction,
        raw: &str,
        e: FsError,
    ) -> FsError {
        let _ = self.fail::<()>(p, ctx, action, raw, e.clone());
        e
    }

    fn quota_fail_resolved(
        &self,
        p: &Principal,
        ctx: &OpContext,
        action: AuditAction,
        r: &ResolvedPath,
        e: FsError,
    ) -> FsError {
        let _ = self.fail_resolved::<()>(p, ctx, action, r, e.clone());
        e
    }

    pub(crate) fn check_scope(
        &self,
        p: &Principal,
        from: &ResolvedPath,
        to: &ResolvedPath,
    ) -> Result<(), FsError> {
        let (a, b) = (p.scope_of(&from.path), p.scope_of(&to.path));
        if matches!(
            (a, b),
            (Some(Scope::Data), Some(Scope::Host)) | (Some(Scope::Host), Some(Scope::Data))
        ) {
            return Err(FsError::Unsupported(
                "禁止在 data/ 与宿主路径之间搬运（跨 scope）".into(),
            ));
        }
        Ok(())
    }

    // ---- 审计 -----------------------------------------------------------

    fn audit(&self, p: &Principal, ctx: &OpContext, a: AuditArgs) {
        let subject = p.subject();
        let mut ev = AuditEvent::new(
            subject.actor_type(),
            subject.actor_id(),
            a.action,
            a.result,
            ctx.now,
        )
        .request(Some(ctx.request_id.clone()))
        .protected(a.protected)
        .bytes(a.bytes);
        ev.src_path = a.src;
        ev.dst_path = a.dst;
        ev.site = a.site;
        ev.code = a.code;
        ev.sha256 = a.hash;
        ev.detail = a.detail;
        self.audit.record(ev);
    }

    fn fail<T>(
        &self,
        p: &Principal,
        ctx: &OpContext,
        action: AuditAction,
        raw: &str,
        e: FsError,
    ) -> Result<T, FsError> {
        let mut a = err_args(action, &e);
        a.src = Some(raw.to_string());
        a.site = path::site_of(std::path::Path::new(raw), &self.cfg.site_root);
        a.protected =
            path::protected_subdir(std::path::Path::new(raw), &self.cfg.site_root).is_some();
        self.audit(p, ctx, a);
        Err(e)
    }

    fn fail_resolved<T>(
        &self,
        p: &Principal,
        ctx: &OpContext,
        action: AuditAction,
        r: &ResolvedPath,
        e: FsError,
    ) -> Result<T, FsError> {
        let mut a = err_args(action, &e);
        a.set_from_resolved(r);
        self.audit(p, ctx, a);
        Err(e)
    }
}

/// 审计字段草稿：避免每个调用点写 10 个参数。
pub(crate) struct AuditArgs {
    pub action: AuditAction,
    pub result: AuditResult,
    pub src: Option<String>,
    pub dst: Option<String>,
    pub site: Option<String>,
    pub protected: bool,
    pub bytes: u64,
    pub code: Option<String>,
    pub hash: Option<String>,
    pub detail: Option<String>,
}

impl AuditArgs {
    pub(crate) fn new(action: AuditAction, result: AuditResult) -> Self {
        Self {
            action,
            result,
            src: None,
            dst: None,
            site: None,
            protected: false,
            bytes: 0,
            code: None,
            hash: None,
            detail: None,
        }
    }

    pub(crate) fn set_from_resolved(&mut self, r: &ResolvedPath) {
        self.src = Some(r.path.display().to_string());
        self.site = r.site.clone();
        self.protected = r.protected;
    }

    pub(crate) fn destination(&mut self, r: &ResolvedPath) {
        self.dst = Some(r.path.display().to_string());
        if self.site.is_none() {
            self.site = r.site.clone();
        }
        self.protected |= r.protected;
    }
}

pub(crate) fn ok_args(action: AuditAction) -> AuditArgs {
    AuditArgs {
        action,
        result: AuditResult::Ok,
        src: None,
        dst: None,
        site: None,
        protected: false,
        bytes: 0,
        code: None,
        hash: None,
        detail: None,
    }
}

pub(crate) fn err_args(action: AuditAction, e: &FsError) -> AuditArgs {
    AuditArgs {
        action,
        result: if e.audit_result() == "deny" {
            AuditResult::Deny
        } else {
            AuditResult::Fail
        },
        src: None,
        dst: None,
        site: None,
        protected: false,
        bytes: 0,
        code: Some(e.code().to_string()),
        hash: None,
        detail: None,
    }
}

/// 管理员可操作全部条目；其他主体只能操作自己创建且未恢复/未清除的条目。
pub(crate) fn can_touch_entry(p: &Principal, entry: &TrashEntry) -> bool {
    p.subject().is_admin()
        || (p.subject().actor_type() == entry.actor_type
            && p.subject().actor_id() == entry.actor_id)
}
