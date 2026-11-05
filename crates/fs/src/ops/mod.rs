//! 内核文件原语操作面（契约 §3）：`list / stat / read / write / mkdir / rename /
//! copy / remove`，外加回收站 `restore / purge / prune`。
//!
//! 每笔操作都走同一条链路：`resolve`（授权 + 不跟随链接）→ 路径锁 → 实际 I/O
//! → 审计（成功逐条或聚合，失败逐条）。管理面与插件面的差异只在 `Principal`。

mod copy;
mod io;
mod mutate;

use std::path::PathBuf;
use std::sync::Arc;

use crate::audit::{AuditAction, AuditEvent, AuditResult, AuditSink};
use crate::config::{FsConfig, QuotaTracker};
use crate::error::{io_error, FsError};
use crate::lock::PathLocks;
use crate::path::{self, PathKind, PathMode, ResolvedPath};
use crate::subject::{Principal, Scope};
use crate::trash::TrashStore;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameOutcome {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOutcome {
    pub from: String,
    pub to: String,
    pub bytes: u64,
    pub entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    pub trash_id: String,
    pub restored_to: String,
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

    pub(crate) fn quota_usage(&self, root: &std::path::Path) -> u64 {
        self.quota.usage(root)
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

    /// 配额 key：插件按命中授权根；管理员按站点目录（无站点则为空）。
    pub(crate) fn quota_key(&self, p: &Principal, r: &ResolvedPath) -> Option<PathBuf> {
        if p.subject().is_admin() {
            let site = r.site.as_ref()?;
            Some(PathBuf::from(&self.cfg.site_root).join(site))
        } else {
            r.grant_root.clone()
        }
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
