//! 属性类操作：`chmod` / `chown`。
//!
//! 与其它动词共用同一条链路：`resolve`（不跟随符号链接）→ 受保护路径硬拒 →
//! 授权（站点 `log/ssl/conf` 需额外开关）→ 路径写锁 → 实际 syscall → 审计。
//! Web 层不得自行调用 syscall 或复制判定。
//!
//! 安全闸（已批准）：
//! - 受保护路径照旧拒绝；
//! - `chmod` **禁止 setuid（0o4000）**，允许 setgid（0o2000）——站点目录靠 2775
//!   共享，setuid 打到世界可写目录是真风险；
//! - `chown` 允许任意 `uid:gid`，但改到 uid 0 时审计单独标注属主为 root。

use std::fs;

use crate::audit::AuditAction;
use crate::error::{io_error, FsError};
use crate::path::{PathKind, PathMode};
use crate::subject::Principal;

use super::{io, ok_args, OpContext};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChmodOutcome {
    pub path: String,
    pub mode: u32,
    pub previous: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChownOutcome {
    pub path: String,
    pub owner: String,
    pub root_owned: bool,
}

impl super::Fs {
    /// 修改权限位（Unix）。只接受 `0o0000..=0o7777`；setuid 一律拒绝。
    pub fn chmod(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw: &str,
        mode: u32,
    ) -> Result<ChmodOutcome, FsError> {
        if mode & !0o7777 != 0 {
            return self.fail(
                p,
                ctx,
                AuditAction::Chmod,
                raw,
                FsError::Denied("权限位必须在 0000-7777 之间（例如 0755、2775）".into()),
            );
        }
        if mode & 0o4000 != 0 {
            return self.fail(
                p,
                ctx,
                AuditAction::Chmod,
                raw,
                FsError::Denied(
                    "不允许设置 setuid 位（4000）：setuid 落到世界可写目录会带来提权风险；需要共享目录请用 setgid（2775）".into(),
                ),
            );
        }
        let r = match self.resolve(p, raw, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Chmod, raw, e),
        };
        if r.kind != PathKind::File && r.kind != PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Chmod,
                &r,
                FsError::Unsupported("只支持修改普通文件与目录的权限".into()),
            );
        }
        if let Err(e) = self.ensure_not_protected(&r, "修改权限") {
            return self.fail_resolved(p, ctx, AuditAction::Chmod, &r, e);
        }
        if let Err(e) = self.ensure_write_allowed(p, &r) {
            return self.fail_resolved(p, ctx, AuditAction::Chmod, &r, e);
        }

        let out = self.locks.with_write(
            std::slice::from_ref(&r.path),
            self.config().lock_timeout,
            || -> Result<Option<u32>, FsError> {
                let meta = fs::symlink_metadata(&r.path).map_err(|e| io_error(&r.path, e))?;
                if meta.file_type().is_symlink() {
                    return Err(FsError::Escape(format!(
                        "不跟随符号链接: {}",
                        r.path.display()
                    )));
                }
                let previous = io::mode_of(&meta);
                set_mode(&r.path, mode)?;
                Ok(previous)
            },
        );
        let previous = match out {
            Ok(Ok(prev)) => prev,
            Ok(Err(e)) | Err(e) => {
                return self.fail_resolved(p, ctx, AuditAction::Chmod, &r, e);
            }
        };

        let mut a = ok_args(AuditAction::Chmod);
        a.set_from_resolved(&r);
        a.detail = Some(format!(
            "mode=0o{:04o}->0o{:04o}",
            previous.unwrap_or(0),
            mode
        ));
        self.audit(p, ctx, a);
        Ok(ChmodOutcome {
            path: r.path.display().to_string(),
            mode,
            previous,
        })
    }

    /// 修改属主/属组（Unix）。任意 `uid:gid` 允许；uid 0 在审计里单独标注。
    pub fn chown(
        &self,
        p: &Principal,
        ctx: &OpContext,
        raw: &str,
        uid: u32,
        gid: u32,
    ) -> Result<ChownOutcome, FsError> {
        let r = match self.resolve(p, raw, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Chown, raw, e),
        };
        if r.kind != PathKind::File && r.kind != PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Chown,
                &r,
                FsError::Unsupported("只支持修改普通文件与目录的属主".into()),
            );
        }
        if let Err(e) = self.ensure_not_protected(&r, "修改属主") {
            return self.fail_resolved(p, ctx, AuditAction::Chown, &r, e);
        }
        if let Err(e) = self.ensure_write_allowed(p, &r) {
            return self.fail_resolved(p, ctx, AuditAction::Chown, &r, e);
        }

        let out = self.locks.with_write(
            std::slice::from_ref(&r.path),
            self.config().lock_timeout,
            || -> Result<Option<String>, FsError> {
                let meta = fs::symlink_metadata(&r.path).map_err(|e| io_error(&r.path, e))?;
                if meta.file_type().is_symlink() {
                    return Err(FsError::Escape(format!(
                        "不跟随符号链接: {}",
                        r.path.display()
                    )));
                }
                let previous = io::owner_of(&meta);
                set_owner(&r.path, uid, gid)?;
                Ok(previous)
            },
        );
        let previous = match out {
            Ok(Ok(prev)) => prev,
            Ok(Err(e)) | Err(e) => {
                return self.fail_resolved(p, ctx, AuditAction::Chown, &r, e);
            }
        };

        let owner = format!("{uid}:{gid}");
        let root_owned = uid == 0;
        let mut a = ok_args(AuditAction::Chown);
        a.set_from_resolved(&r);
        a.detail = Some(format!(
            "owner={owner}, previous={}, root_owner={root_owned}",
            previous.as_deref().unwrap_or("-")
        ));
        self.audit(p, ctx, a);
        Ok(ChownOutcome {
            path: r.path.display().to_string(),
            owner,
            root_owned,
        })
    }
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) -> Result<(), FsError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(path, e))
}

#[cfg(not(unix))]
fn set_mode(path: &std::path::Path, _mode: u32) -> Result<(), FsError> {
    Err(FsError::Unsupported(format!(
        "当前平台不支持修改权限: {}",
        path.display()
    )))
}

#[cfg(unix)]
fn set_owner(path: &std::path::Path, uid: u32, gid: u32) -> Result<(), FsError> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| io_error(path, e))
}

#[cfg(not(unix))]
fn set_owner(path: &std::path::Path, _uid: u32, _gid: u32) -> Result<(), FsError> {
    Err(FsError::Unsupported(format!(
        "当前平台不支持修改属主: {}",
        path.display()
    )))
}
