//! Skiff（轻舟）内核文件系统原语库。
//!
//! 依据 `docs/KERNEL_FS_CONTRACT.md` 实现的**底层能力**，刻意不接入现有插件
//! 能力面：不依赖 `skiff-store` / `skiff-web` / `skiff-docker` / `skiff-plugin` /
//! `skiff-wire`，审计与回收站存储用 trait 抽象，本轮只提供内存实现。
//!
//! 能力面：
//! - [`Subject`] / [`Principal`]：管理员与插件的授权模型；
//! - [`path::normalize_absolute`] / [`Fs::resolve`]：lexical 归一 + 授权 + 不跟随符号链接；
//! - [`PathLocks`]：键控读写锁，双路径按字典序；
//! - [`Fs`]：`list/stat/read/write/mkdir/rename/copy/remove` 与回收站三操作；
//! - [`AuditEvent`] / [`AuditSink`] / [`AggregatingSink`]：审计结构与分级策略；
//! - [`TrashStore`] / [`TrashEntry`]：回收站元数据、保留期与配额淘汰。

mod audit;
mod config;
mod error;
mod hash;
mod lock;
mod ops;
mod path;
mod subject;
mod trash;

pub use audit::{
    should_aggregate, AggregatingSink, AuditAction, AuditEvent, AuditResult, AuditSink,
    MemoryAuditSink, NoopAudit, AGG_BUCKETS_PER_SUBJECT_MINUTE, AGG_WINDOW_SECS,
};
pub use config::{
    FsConfig, MemoryQuotaTracker, QuotaLimit, QuotaSnapshot, QuotaTracker, QuotaUsage, TrashConfig,
    COPY_MAX_DEPTH, COPY_MAX_ENTRIES, MAX_DIR_ENTRIES, MAX_FILE_BYTES, MAX_REQUEST_BYTES,
    MAX_ROOT_BYTES,
};
pub use error::FsError;
pub use hash::sha256_hex;
pub use lock::{PathLocks, DEFAULT_LOCK_TIMEOUT};
pub use ops::{
    now_secs, ClearTrashOutcome, CopyLimits, CopyOutcome, EntryInfo, Fs, ListPage, ListRequest,
    MkdirOutcome, OpContext, ReadChunk, ReadRequest, RemoveOutcome, RenameOutcome, RestoreOutcome,
    StatInfo, StatRequest, WriteMode, WriteOutcome, WriteRequest,
};
pub use path::{
    normalize_absolute, protected_hit, protected_subdir, site_of, PathKind, PathMode, ResolvedPath,
    DEFAULT_SITE_ROOT, PROTECTED_SUBDIRS,
};
pub use subject::{canonical_grant_roots, validate_grant_root, Principal, Scope, Subject};
pub use trash::{
    new_trash_id, select_evictions, trash_path, MemoryTrashStore, TrashEntry, TrashKind, TrashStore,
};
