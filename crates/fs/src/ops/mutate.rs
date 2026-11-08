//! 变更类操作：`write / remove / rename / restore / purge / prune`。
//!
//! 写默认原子（tmp+fsync+rename）；`append`/定位写显式非原子并在审计里标注。
//! 删除默认进回收站，`permanent=true` 仅管理员可用。

use std::fs;
use std::path::{Path, PathBuf};

use crate::audit::{AuditAction, AuditEvent, AuditResult};
use crate::config::QuotaUsage;
use crate::error::{io_error, FsError};
use crate::hash::sha256_hex;
use crate::path::{PathKind, PathMode, ResolvedPath};
use crate::subject::Principal;
use crate::trash::{new_trash_id, select_evictions, trash_path, TrashEntry, TrashKind};

use super::copy::{self, CopyLimits};
use super::{
    can_touch_entry, io, now_secs, ok_args, OpContext, RemoveOutcome, RenameOutcome,
    RestoreOutcome, WriteMode, WriteOutcome, WriteRequest,
};

impl super::Fs {
    pub fn write(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &WriteRequest,
    ) -> Result<WriteOutcome, FsError> {
        if req.data.len() > self.config().max_request_bytes {
            return self.fail(
                p,
                ctx,
                AuditAction::Write,
                &req.path,
                FsError::TooLarge(format!(
                    "单次写入 {} 字节超过上限 {}",
                    req.data.len(),
                    self.config().max_request_bytes
                )),
            );
        }
        let r = match self.resolve(p, &req.path, PathMode::Create) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Write, &req.path, e),
        };
        if r.kind == PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Write,
                &r,
                FsError::Conflict("目标是目录".into()),
            );
        }
        if let Err(e) = self.ensure_write_allowed(p, &r) {
            return self.fail_resolved(p, ctx, AuditAction::Write, &r, e);
        }

        let old_size = existing_size(&r);
        let new_size = projected_size(old_size, req);
        if new_size > self.config().max_file_bytes {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Write,
                &r,
                FsError::FileTooLarge(format!(
                    "写后大小 {} 超过单文件上限 {}",
                    new_size,
                    self.config().max_file_bytes
                )),
            );
        }
        let quota_target = self.quota_target(p, &r);
        let delta_bytes = new_size as i64 - old_size as i64;
        let delta_files = if r.kind == PathKind::File { 0 } else { 1 };
        if let Err(e) = self.enforce_quota(quota_target.as_ref(), delta_bytes, delta_files) {
            return self.fail_resolved(p, ctx, AuditAction::Write, &r, e);
        }

        let out = self.locks.with_write(
            std::slice::from_ref(&r.path),
            self.config().lock_timeout,
            || -> Result<u64, FsError> {
                let existing = match fs::symlink_metadata(&r.path) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(FsError::Escape("不跟随符号链接".into()));
                    }
                    Ok(meta) => Some(meta),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(io_error(&r.path, e)),
                };
                match req.mode {
                    WriteMode::Replace => io::atomic_write(&r.path, &req.data, existing.as_ref())?,
                    WriteMode::Append => io::append(&r.path, &req.data)?,
                    WriteMode::At(offset) => io::write_at(&r.path, offset, &req.data)?,
                }
                let size = fs::symlink_metadata(&r.path)
                    .map_err(|e| io_error(&r.path, e))?
                    .len();
                Ok(size)
            },
        );
        let out = match out {
            Ok(Ok(size)) => size,
            Ok(Err(e)) | Err(e) => {
                return self.fail_resolved(p, ctx, AuditAction::Write, &r, e);
            }
        };

        if let Some(target) = quota_target.as_ref() {
            self.quota()
                .adjust_stats(&target.root, delta_bytes, delta_files);
        }
        let hash = sha256_hex(&req.data);
        let mut a = ok_args(AuditAction::Write);
        a.set_from_resolved(&r);
        a.bytes = req.data.len() as u64;
        a.hash = Some(hash.clone());
        a.detail = Some(format!(
            "atomic={}, mode={:?}",
            matches!(req.mode, WriteMode::Replace),
            req.mode
        ));
        self.audit(p, ctx, a);
        Ok(WriteOutcome {
            path: r.path.display().to_string(),
            bytes: req.data.len() as u64,
            size: out,
            sha256: hash,
            atomic: matches!(req.mode, WriteMode::Replace),
            created: old_size == 0 && r.kind == PathKind::Missing,
        })
    }

    pub fn remove(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw: &str,
        permanent: bool,
    ) -> Result<RemoveOutcome, FsError> {
        let r = match self.resolve(p, raw, PathMode::ExistingLink) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Remove, raw, e),
        };
        if permanent && !p.subject().is_admin() {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Remove,
                &r,
                FsError::Denied("permanent 仅管理员可用".into()),
            );
        }
        if p.is_grant_root_itself(&r.path) {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Remove,
                &r,
                FsError::Denied("不允许删除授权根本身".into()),
            );
        }
        if let Err(e) = self.ensure_write_allowed(p, &r) {
            return self.fail_resolved(p, ctx, AuditAction::Remove, &r, e);
        }
        let usage = match io::object_stats(&r.path, self.config().max_dir_entries) {
            Ok(usage) => usage,
            Err(e) => return self.fail_resolved(p, ctx, AuditAction::Remove, &r, e),
        };
        let quota_target = self.quota_target(p, &r);

        if permanent {
            let out = self.locks.with_write(
                std::slice::from_ref(&r.path),
                self.config().lock_timeout,
                || io::remove_object(&r.path),
            )?;
            match out {
                Ok(_) => {
                    if let Some(target) = quota_target.as_ref() {
                        self.quota().adjust_stats(
                            &target.root,
                            -(usage.used_bytes as i64),
                            -(usage.used_files as i64),
                        );
                    }
                    let mut a = ok_args(AuditAction::Remove);
                    a.set_from_resolved(&r);
                    a.bytes = usage.used_bytes;
                    a.detail = Some(format!("permanent=true, kind={}", kind_name(&r)));
                    self.audit(p, ctx, a);
                    Ok(RemoveOutcome {
                        path: r.path.display().to_string(),
                        kind: r.kind,
                        permanent: true,
                        trash_id: None,
                        bytes: usage.used_bytes,
                    })
                }
                Err(e) => self.fail_resolved(p, ctx, AuditAction::Remove, &r, e),
            }
        } else {
            self.remove_to_trash(p, ctx, &r, usage, quota_target)
        }
    }

    fn remove_to_trash(
        &self,
        p: &Principal,
        ctx: &OpContext,
        r: &ResolvedPath,
        usage: QuotaUsage,
        quota_target: Option<crate::config::QuotaLimit>,
    ) -> Result<RemoveOutcome, FsError> {
        let size = usage.used_bytes;
        let cfg = &self.config().trash;
        if size > cfg.per_entry_bytes {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Remove,
                r,
                FsError::TooLarge(format!(
                    "回收站单条目上限 {} 字节，请使用 permanent",
                    cfg.per_entry_bytes
                )),
            );
        }
        let root_key = quota_target
            .as_ref()
            .map(|t| t.root.display().to_string())
            .unwrap_or_default();
        let rows = self.trash_store().list();
        let evictions =
            select_evictions(&rows, &root_key, size, cfg.per_root_bytes, cfg.max_entries);
        let evict_bytes: u64 = rows
            .iter()
            .filter(|e| e.is_live() && evictions.contains(&e.id))
            .map(|e| e.size)
            .sum();
        let remaining = self.trash_store().total_usage().saturating_sub(evict_bytes);
        if remaining.saturating_add(size) > cfg.total_bytes {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Remove,
                r,
                FsError::TrashFull("回收站全局配额不足，请彻底删除旧条目".into()),
            );
        }

        let id = new_trash_id(ctx.now);
        let tpath = trash_path(&cfg.root, &id, ctx.now);
        if let Some(parent) = tpath.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return self.fail_resolved(p, ctx, AuditAction::Remove, r, io_error(parent, e));
            }
        }

        let mut locks: Vec<PathBuf> = vec![r.path.clone()];
        for e in rows.iter().filter(|e| evictions.contains(&e.id)) {
            locks.push(PathBuf::from(&e.trash_path));
        }
        let out = self
            .locks
            .with_write(&locks, self.config().lock_timeout, || {
                for e in rows.iter().filter(|e| evictions.contains(&e.id)) {
                    let path = PathBuf::from(&e.trash_path);
                    io::remove_object(&path)?;
                    self.trash_store().remove(&e.id);
                    self.audit_eviction(ctx, e);
                }
                let (_, crossed) = copy::move_object(
                    &r.path,
                    &tpath,
                    CopyLimits {
                        max_entries: cfg.max_entries,
                        max_depth: self.config().copy_max_depth,
                    },
                )?;
                Ok::<bool, FsError>(crossed)
            });
        let crossed = match out {
            Ok(Ok(crossed)) => crossed,
            Ok(Err(e)) | Err(e) => return self.fail_resolved(p, ctx, AuditAction::Remove, r, e),
        };

        let meta = fs::symlink_metadata(&tpath).map_err(|e| io_error(&tpath, e))?;
        let kind = if meta.file_type().is_symlink() {
            TrashKind::Symlink
        } else if meta.is_dir() {
            TrashKind::Dir
        } else {
            TrashKind::File
        };
        let sha256 = if kind == TrashKind::File {
            file_hash(&tpath).ok()
        } else {
            None
        };
        let entry = TrashEntry {
            id: id.clone(),
            actor_type: p.subject().actor_type().to_string(),
            actor_id: p.subject().actor_id().to_string(),
            original_path: r.path.display().to_string(),
            trash_path: tpath.display().to_string(),
            kind,
            size,
            files: usage.used_files,
            mode: io::mode_of(&meta),
            owner: io::owner_of(&meta),
            sha256,
            site: r.site.clone(),
            protected: r.protected,
            deleted_at: ctx.now,
            expires_at: ctx.now + cfg.retention_secs,
            nlink: io::nlink_of(&meta),
            restored_at: None,
            purged_at: None,
            quota_root: root_key,
        };
        self.trash_store().insert(entry);
        if let Some(target) = quota_target.as_ref() {
            self.quota().adjust_stats(
                &target.root,
                -(usage.used_bytes as i64),
                -(usage.used_files as i64),
            );
        }

        let mut a = ok_args(AuditAction::Remove);
        a.set_from_resolved(r);
        a.bytes = size;
        a.detail = Some(format!("trash_id={id}, crossed_fs={crossed}"));
        self.audit(p, ctx, a);
        Ok(RemoveOutcome {
            path: r.path.display().to_string(),
            kind: r.kind,
            permanent: false,
            trash_id: Some(id),
            bytes: size,
        })
    }

    pub fn rename(
        &self,
        p: &Principal,
        ctx: &OpContext,
        from_raw: &str,
        to_raw: &str,
        overwrite: bool,
    ) -> Result<RenameOutcome, FsError> {
        let from = match self.resolve(p, from_raw, PathMode::ExistingLink) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Rename, from_raw, e),
        };
        // 先判源：授权根本身不可改名，不因目标路径非法而改判为 escape。
        if p.is_grant_root_itself(&from.path) {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Rename,
                &from,
                FsError::Denied("不允许改名授权根本身".into()),
            );
        }
        let to = match self.resolve(p, to_raw, PathMode::CreateLink) {
            Ok(r) => r,
            Err(e) => {
                let mut args = super::err_args(AuditAction::Rename, &e);
                args.src = Some(from_raw.to_string());
                args.dst = Some(to_raw.to_string());
                self.audit(p, ctx, args);
                return Err(e);
            }
        };
        if overwrite && !p.subject().is_admin() {
            return self.rename_fail(
                p,
                ctx,
                &from,
                &to,
                FsError::Denied("overwrite 仅管理员可用".into()),
            );
        }
        if p.is_grant_root_itself(&to.path) {
            return self.rename_fail(
                p,
                ctx,
                &from,
                &to,
                FsError::Denied("不允许改名授权根本身".into()),
            );
        }
        for r in [&from, &to] {
            if let Err(e) = self.ensure_write_allowed(p, r) {
                return self.rename_fail(p, ctx, &from, &to, e);
            }
        }
        if let Err(e) = self.check_scope(p, &from, &to) {
            return self.rename_fail(p, ctx, &from, &to, e);
        }
        if to.kind != PathKind::Missing {
            if !overwrite {
                return self.rename_fail(
                    p,
                    ctx,
                    &from,
                    &to,
                    FsError::Conflict("目标已存在".into()),
                );
            }
            if to.kind == PathKind::Dir {
                return self.rename_fail(
                    p,
                    ctx,
                    &from,
                    &to,
                    FsError::Conflict("不允许覆盖目录".into()),
                );
            }
        }

        let src_usage = match io::object_stats(&from.path, self.config().max_dir_entries) {
            Ok(u) => u,
            Err(e) => return self.rename_fail(p, ctx, &from, &to, e),
        };
        let dest_old = if to.kind != PathKind::Missing {
            io::object_stats(&to.path, self.config().max_dir_entries).unwrap_or_default()
        } else {
            QuotaUsage::default()
        };
        let from_target = self.quota_target(p, &from);
        let to_target = self.quota_target(p, &to);
        let same_root = match (&from_target, &to_target) {
            (Some(a), Some(b)) => a.root == b.root,
            (None, None) => true,
            _ => false,
        };
        if !same_root {
            if let Err(e) = self.enforce_quota(
                to_target.as_ref(),
                src_usage.used_bytes as i64 - dest_old.used_bytes as i64,
                src_usage.used_files as i64 - dest_old.used_files as i64,
            ) {
                return self.rename_fail(p, ctx, &from, &to, e);
            }
        }

        let out = self.locks.with_write(
            &[from.path.clone(), to.path.clone()],
            self.config().lock_timeout,
            || -> Result<(), FsError> {
                if to.kind != PathKind::Missing {
                    io::remove_object(&to.path)?;
                }
                copy::move_object(
                    &from.path,
                    &to.path,
                    CopyLimits {
                        max_entries: self.config().copy_max_entries,
                        max_depth: self.config().copy_max_depth,
                    },
                )
                .map(|_| ())
            },
        )?;
        if let Err(e) = out {
            return self.rename_fail(p, ctx, &from, &to, e);
        }
        match (from_target.as_ref(), to_target.as_ref()) {
            (Some(f), Some(t)) if f.root == t.root => {
                if dest_old != QuotaUsage::default() {
                    self.quota().adjust_stats(
                        &t.root,
                        -(dest_old.used_bytes as i64),
                        -(dest_old.used_files as i64),
                    );
                }
            }
            (from_target, to_target) => {
                if let Some(t) = to_target {
                    self.quota().adjust_stats(
                        &t.root,
                        src_usage.used_bytes as i64 - dest_old.used_bytes as i64,
                        src_usage.used_files as i64 - dest_old.used_files as i64,
                    );
                }
                if let Some(f) = from_target {
                    self.quota().adjust_stats(
                        &f.root,
                        -(src_usage.used_bytes as i64),
                        -(src_usage.used_files as i64),
                    );
                }
            }
        }
        let mut a = ok_args(AuditAction::Rename);
        a.set_from_resolved(&from);
        a.destination(&to);
        a.detail = Some(format!("overwrite={overwrite}"));
        self.audit(p, ctx, a);
        Ok(RenameOutcome {
            from: from.path.display().to_string(),
            to: to.path.display().to_string(),
        })
    }

    fn rename_fail<T>(
        &self,
        p: &Principal,
        ctx: &OpContext,
        from: &ResolvedPath,
        to: &ResolvedPath,
        e: FsError,
    ) -> Result<T, FsError> {
        let mut a = super::err_args(AuditAction::Rename, &e);
        a.set_from_resolved(from);
        a.destination(to);
        self.audit(p, ctx, a);
        Err(e)
    }

    pub fn copy(
        &self,
        p: &Principal,
        ctx: &OpContext,
        from_raw: &str,
        to_raw: &str,
        overwrite: bool,
    ) -> Result<super::CopyOutcome, FsError> {
        let from = match self.resolve(p, from_raw, PathMode::ExistingLink) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Copy, from_raw, e),
        };
        let to = match self.resolve(p, to_raw, PathMode::CreateLink) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Copy, to_raw, e),
        };
        if overwrite && !p.subject().is_admin() {
            return self.copy_fail(
                p,
                ctx,
                &from,
                &to,
                FsError::Denied("overwrite 仅管理员可用".into()),
            );
        }
        if let Err(e) = self.ensure_write_allowed(p, &to) {
            return self.copy_fail(p, ctx, &from, &to, e);
        }
        if let Err(e) = self.check_scope(p, &from, &to) {
            return self.copy_fail(p, ctx, &from, &to, e);
        }
        if to.kind != PathKind::Missing && !overwrite {
            return self.copy_fail(p, ctx, &from, &to, FsError::Conflict("目标已存在".into()));
        }
        let quota_target = self.quota_target(p, &to);
        let dest_old = if overwrite && matches!(to.kind, PathKind::File | PathKind::Symlink) {
            io::object_stats(&to.path, self.config().max_dir_entries).unwrap_or_default()
        } else {
            QuotaUsage::default()
        };
        let src_usage = match io::object_stats(&from.path, self.config().max_dir_entries) {
            Ok(u) => u,
            Err(e) => return self.copy_fail(p, ctx, &from, &to, e),
        };
        if let Err(e) = self.enforce_quota(
            quota_target.as_ref(),
            src_usage.used_bytes as i64 - dest_old.used_bytes as i64,
            src_usage.used_files as i64 - dest_old.used_files as i64,
        ) {
            return self.copy_fail(p, ctx, &from, &to, e);
        }
        let limits = CopyLimits {
            max_entries: self.config().copy_max_entries,
            max_depth: self.config().copy_max_depth,
        };
        let out = self.locks.with_write(
            &[from.path.clone(), to.path.clone()],
            self.config().lock_timeout,
            || copy::copy_object(&from.path, &to.path, limits, overwrite),
        );
        let stats = match out {
            Ok(Ok(stats)) => stats,
            Ok(Err(e)) | Err(e) => return self.copy_fail(p, ctx, &from, &to, e),
        };
        if let Some(target) = quota_target.as_ref() {
            self.quota().adjust_stats(
                &target.root,
                stats.bytes as i64 - dest_old.used_bytes as i64,
                stats.files as i64 - dest_old.used_files as i64,
            );
        }
        let mut a = ok_args(AuditAction::Copy);
        a.set_from_resolved(&from);
        a.destination(&to);
        a.bytes = stats.bytes;
        a.detail = Some(format!("entries={}, overwrite={overwrite}", stats.entries));
        self.audit(p, ctx, a);
        Ok(super::CopyOutcome {
            from: from.path.display().to_string(),
            to: to.path.display().to_string(),
            bytes: stats.bytes,
            entries: stats.entries,
        })
    }

    fn copy_fail<T>(
        &self,
        p: &Principal,
        ctx: &OpContext,
        from: &ResolvedPath,
        to: &ResolvedPath,
        e: FsError,
    ) -> Result<T, FsError> {
        let mut a = super::err_args(AuditAction::Copy, &e);
        a.set_from_resolved(from);
        a.destination(to);
        self.audit(p, ctx, a);
        Err(e)
    }

    pub fn restore(
        &self,
        p: &Principal,
        ctx: &OpContext,
        trash_id: &str,
        overwrite: bool,
    ) -> Result<RestoreOutcome, FsError> {
        let entry = match self.trash_store().get(trash_id) {
            Some(e) => e,
            None => return Err(FsError::NotFound(format!("回收站条目不存在: {trash_id}"))),
        };
        if !entry.is_live() {
            return Err(FsError::Conflict("条目已恢复或已彻底删除".into()));
        }
        if !can_touch_entry(p, &entry) {
            return Err(FsError::Denied("只能恢复自己主体的回收站条目".into()));
        }
        if let Some(parent) = Path::new(&entry.original_path).parent() {
            if !parent.exists() {
                return Err(FsError::NotFound(format!(
                    "原路径的父目录已不存在：{}；请先重建目录后再恢复，或误删后从回收站复制",
                    parent.display()
                )));
            }
        }
        let target = self.resolve(p, &entry.original_path, PathMode::Create)?;
        if target.kind != PathKind::Missing && !overwrite {
            return Err(FsError::Conflict(
                "原路径已存在同名对象，不自动覆盖；如需替换请显式选择覆盖".into(),
            ));
        }
        if overwrite && target.kind == PathKind::Dir {
            return Err(FsError::Conflict("不允许覆盖目录".into()));
        }
        self.ensure_write_allowed(p, &target)?;
        let quota_target = self.quota_target(p, &target);
        let dest_old = if overwrite && target.kind != PathKind::Missing {
            io::object_stats(&target.path, self.config().max_dir_entries).unwrap_or_default()
        } else {
            QuotaUsage::default()
        };
        self.enforce_quota(
            quota_target.as_ref(),
            entry.size as i64 - dest_old.used_bytes as i64,
            entry.files as i64 - dest_old.used_files as i64,
        )?;
        let tpath = PathBuf::from(&entry.trash_path);
        if fs::symlink_metadata(&tpath).is_err() {
            return Err(FsError::NotFound(format!(
                "回收站对象已丢失: {}",
                tpath.display()
            )));
        }
        let locks = [target.path.clone(), tpath.clone()];
        let out = self
            .locks
            .with_write(&locks, self.config().lock_timeout, || {
                if target.kind != PathKind::Missing && overwrite {
                    io::remove_object(&target.path)?;
                }
                copy::move_object(
                    &tpath,
                    &target.path,
                    CopyLimits {
                        max_entries: self.config().copy_max_entries,
                        max_depth: self.config().copy_max_depth,
                    },
                )
                .map(|_| ())
            });
        match out {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => return Err(e),
        }
        self.trash_store().mark_restored(trash_id, now_secs());
        if let Some(target) = quota_target.as_ref() {
            self.quota().adjust_stats(
                &target.root,
                entry.size as i64 - dest_old.used_bytes as i64,
                entry.files as i64 - dest_old.used_files as i64,
            );
        }
        let mut a = ok_args(AuditAction::TrashRestore);
        a.src = Some(entry.trash_path.clone());
        a.dst = Some(entry.original_path.clone());
        a.site = entry.site.clone();
        a.protected = entry.protected;
        a.bytes = entry.size;
        self.audit(p, ctx, a);
        Ok(RestoreOutcome {
            trash_id: trash_id.to_string(),
            restored_to: target.path.display().to_string(),
        })
    }

    pub fn purge(&self, p: &Principal, ctx: &OpContext, trash_id: &str) -> Result<u64, FsError> {
        let entry = match self.trash_store().get(trash_id) {
            Some(e) => e,
            None => return Err(FsError::NotFound(format!("回收站条目不存在: {trash_id}"))),
        };
        if !entry.is_live() {
            return Err(FsError::Conflict("条目已恢复或已彻底删除".into()));
        }
        if !can_touch_entry(p, &entry) {
            return Err(FsError::Denied("只能彻底删除自己主体的回收站条目".into()));
        }
        let tpath = PathBuf::from(&entry.trash_path);
        let out = self.locks.with_write(
            std::slice::from_ref(&tpath),
            self.config().lock_timeout,
            || io::remove_object(&tpath),
        );
        match out {
            Ok(Ok(_)) => {}
            Ok(Err(e)) | Err(e) => return Err(e),
        }
        self.trash_store().remove(trash_id);
        let mut a = ok_args(AuditAction::TrashPurge);
        a.src = Some(entry.trash_path.clone());
        a.site = entry.site.clone();
        a.bytes = entry.size;
        a.detail = Some(format!("original={}", entry.original_path));
        self.audit(p, ctx, a);
        Ok(entry.size)
    }

    /// 到期清理（内置 `trash-prune` 任务调用）。审计主体为 `system`。
    pub fn prune_trash(&self, ctx: &OpContext) -> Result<u64, FsError> {
        let expired = self.trash_store().expired(ctx.now);
        let mut removed = 0u64;
        for entry in expired {
            let tpath = PathBuf::from(&entry.trash_path);
            let out = self.locks.with_write(
                std::slice::from_ref(&tpath),
                self.config().lock_timeout,
                || io::remove_object(&tpath),
            );
            match out {
                Ok(Ok(_)) => {}
                Ok(Err(e)) | Err(e) => {
                    self.audit_system(ctx, &entry, format!("{}: {}", e.code(), e.message()));
                    continue;
                }
            }
            self.trash_store().remove(&entry.id);
            removed += 1;
            self.audit_system(ctx, &entry, "ok".to_string());
        }
        Ok(removed)
    }

    fn audit_eviction(&self, ctx: &OpContext, entry: &TrashEntry) {
        let mut ev = AuditEvent::new(
            "system",
            "kernel",
            AuditAction::TrashPurge,
            AuditResult::Ok,
            ctx.now,
        )
        .request(Some(ctx.request_id.clone()))
        .bytes(entry.size);
        ev.src_path = Some(entry.trash_path.clone());
        ev.site = entry.site.clone();
        ev.detail = Some(format!("quota_eviction, original={}", entry.original_path));
        self.audit_sink_record(ev);
    }

    fn audit_system(&self, ctx: &OpContext, entry: &TrashEntry, detail: String) {
        let mut ev = AuditEvent::new(
            "system",
            "kernel",
            AuditAction::TrashPurge,
            if detail == "ok" {
                AuditResult::Ok
            } else {
                AuditResult::Fail
            },
            ctx.now,
        )
        .request(Some(ctx.request_id.clone()))
        .bytes(entry.size);
        ev.src_path = Some(entry.trash_path.clone());
        ev.site = entry.site.clone();
        ev.detail = Some(detail);
        self.audit_sink_record(ev);
    }
}

fn existing_size(r: &ResolvedPath) -> u64 {
    if r.kind == PathKind::File {
        fs::symlink_metadata(&r.path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    }
}

fn projected_size(old: u64, req: &WriteRequest) -> u64 {
    match req.mode {
        WriteMode::Replace => req.data.len() as u64,
        WriteMode::Append => old.saturating_add(req.data.len() as u64),
        WriteMode::At(offset) => old.max(offset.saturating_add(req.data.len() as u64)),
    }
}

fn kind_name(r: &ResolvedPath) -> &'static str {
    match r.kind {
        PathKind::File => "file",
        PathKind::Dir => "dir",
        PathKind::Symlink => "symlink",
        PathKind::Other => "other",
        PathKind::Missing => "missing",
    }
}

fn file_hash(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(sha256_hex(&buf))
}
