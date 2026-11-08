//! 结构化错误码：与 `docs/KERNEL_FS_CONTRACT.md` §7.3 对齐。
//!
//! 这里是**唯一**的错误码来源：能力面（UDS）、HTTP 管理面和审计都从这里取
//! `code()`，避免同一种失败在不同层被译成不同字符串。

use std::fmt;

/// 文件系统原语错误。`code()` 返回契约里的稳定错误码，`http_status()` 只给
/// 管理面做映射，本 crate 不依赖任何 HTTP 框架。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    /// `fs.denied`：未授权 / 路径形式非法（相对路径、`..`、NUL、空串、非 UTF-8）
    Denied(String),
    /// `fs.escape`：路径越界（含父目录符号链接逃逸）
    Escape(String),
    /// `fs.protected`：受保护路径未开写权限
    Protected(String),
    /// `fs.not_found`
    NotFound(String),
    /// `fs.conflict`：目标已存在 / 目标是目录或不是目录
    Conflict(String),
    /// `fs.busy`：路径锁等待超时
    Busy(String),
    /// `fs.too_large`：单次请求或目录复制规模超限
    TooLarge(String),
    /// `fs.file_too_large`：单文件超限
    FileTooLarge(String),
    /// `fs.too_many_entries`：目录条目数超限
    TooManyEntries(String),
    /// `fs.quota_exceeded`：授权根配额不足
    QuotaExceeded(String),
    /// `fs.trash_full`：回收站全局配额不足
    TrashFull(String),
    /// `fs.unsupported`：操作不支持（如跨 scope 改名）
    Unsupported(String),
    /// `fs.read_only`：目标所在文件系统只读（宿主 `ProtectSystem`、只读挂载等策略限制）
    ReadOnly(String),
    /// `fs.no_space`：宿主磁盘空间不足，或文件系统配额（`ENOSPC` / `EDQUOT`）已用尽
    NoSpace(String),
    /// 未归类的 I/O 故障；只有这一类映射到 500
    Io(String),
}

impl FsError {
    pub fn code(&self) -> &'static str {
        match self {
            FsError::Denied(_) => "fs.denied",
            FsError::Escape(_) => "fs.escape",
            FsError::Protected(_) => "fs.protected",
            FsError::NotFound(_) => "fs.not_found",
            FsError::Conflict(_) => "fs.conflict",
            FsError::Busy(_) => "fs.busy",
            FsError::TooLarge(_) => "fs.too_large",
            FsError::FileTooLarge(_) => "fs.file_too_large",
            FsError::TooManyEntries(_) => "fs.too_many_entries",
            FsError::QuotaExceeded(_) => "fs.quota_exceeded",
            FsError::TrashFull(_) => "fs.trash_full",
            FsError::Unsupported(_) => "fs.unsupported",
            FsError::ReadOnly(_) => "fs.read_only",
            FsError::NoSpace(_) => "fs.no_space",
            FsError::Io(_) => "fs.io",
        }
    }

    /// 管理面 HTTP 状态码。`fs.read_only` / `fs.no_space` 是宿主环境限制，
    /// 归为可读的 4xx/507，不落到 500；只有真正未归类的 `fs.io` 才是 500。
    pub fn http_status(&self) -> u16 {
        match self {
            FsError::Denied(_) | FsError::Escape(_) | FsError::Protected(_) => 403,
            FsError::ReadOnly(_) => 403,
            FsError::NotFound(_) => 404,
            FsError::Conflict(_) => 409,
            FsError::Busy(_) => 423,
            FsError::TooLarge(_) | FsError::FileTooLarge(_) | FsError::TooManyEntries(_) => 413,
            FsError::QuotaExceeded(_) | FsError::TrashFull(_) | FsError::NoSpace(_) => 507,
            FsError::Unsupported(_) => 400,
            FsError::Io(_) => 500,
        }
    }

    /// 审计结论：越权类一律 `deny`，其它为 `fail`。成功路径由调用方写 `ok`。
    pub fn audit_result(&self) -> &'static str {
        match self {
            FsError::Denied(_) | FsError::Escape(_) | FsError::Protected(_) => "deny",
            _ => "fail",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            FsError::Denied(m)
            | FsError::Escape(m)
            | FsError::Protected(m)
            | FsError::NotFound(m)
            | FsError::Conflict(m)
            | FsError::Busy(m)
            | FsError::TooLarge(m)
            | FsError::FileTooLarge(m)
            | FsError::TooManyEntries(m)
            | FsError::QuotaExceeded(m)
            | FsError::TrashFull(m)
            | FsError::Unsupported(m)
            | FsError::ReadOnly(m)
            | FsError::NoSpace(m)
            | FsError::Io(m) => m,
        }
    }

    /// 在保留错误码的前提下追加可读说明（如解压回滚未能清理的路径），
    /// 让用户能在响应里看到「已创建但未清理」的半成品，而不是被静默吞掉。
    pub fn with_note(self, note: &str) -> FsError {
        use FsError::*;
        let join = |m: String| format!("{m}（{note}）");
        match self {
            Denied(m) => Denied(join(m)),
            Escape(m) => Escape(join(m)),
            Protected(m) => Protected(join(m)),
            NotFound(m) => NotFound(join(m)),
            Conflict(m) => Conflict(join(m)),
            Busy(m) => Busy(join(m)),
            TooLarge(m) => TooLarge(join(m)),
            FileTooLarge(m) => FileTooLarge(join(m)),
            TooManyEntries(m) => TooManyEntries(join(m)),
            QuotaExceeded(m) => QuotaExceeded(join(m)),
            TrashFull(m) => TrashFull(join(m)),
            Unsupported(m) => Unsupported(join(m)),
            ReadOnly(m) => ReadOnly(join(m)),
            NoSpace(m) => NoSpace(join(m)),
            Io(m) => Io(join(m)),
        }
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for FsError {}

/// 把 `std::io::Error` 归并到契约错误码。`path` 只用于补充可读信息，不参与判定。
pub(crate) fn io_error(path: &std::path::Path, e: std::io::Error) -> FsError {
    io_error_ctx(path, e, "")
}

/// 同 [`io_error`]，但保留调用点的动作描述（如「读取 tar.gz 失败」）。
pub(crate) fn io_error_ctx(path: &std::path::Path, e: std::io::Error, context: &str) -> FsError {
    use std::io::ErrorKind;
    if let Some(err) = classify_os_error(path, e.raw_os_error()) {
        return err;
    }
    let ctx = if context.is_empty() {
        format!("{}: {e}", path.display())
    } else {
        format!("{context}（{}）: {e}", path.display())
    };
    match e.kind() {
        ErrorKind::NotFound => FsError::NotFound(ctx),
        ErrorKind::PermissionDenied => FsError::Denied(format!("权限不足: {ctx}")),
        ErrorKind::AlreadyExists => FsError::Conflict(ctx),
        _ => FsError::Io(ctx),
    }
}

/// 把宿主挂载/容量类 OS 错误译成可读的 4xx/507，避免把「只读文件系统」
/// 「磁盘已满」这类环境限制当成内部故障（500）。
#[cfg(unix)]
fn classify_os_error(path: &std::path::Path, code: Option<i32>) -> Option<FsError> {
    let code = code?;
    let display = path.display();
    match code {
        libc::EROFS => Some(FsError::ReadOnly(format!(
            "该路径所在文件系统为只读，无法写入（宿主策略限制）: {display}"
        ))),
        libc::ENOSPC => Some(FsError::NoSpace(format!(
            "宿主磁盘空间不足，无法写入: {display}"
        ))),
        libc::EDQUOT => Some(FsError::NoSpace(format!(
            "文件系统配额已用尽，无法写入: {display}"
        ))),
        _ => None,
    }
}

#[cfg(not(unix))]
fn classify_os_error(_path: &std::path::Path, _code: Option<i32>) -> Option<FsError> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::path::Path;

    #[test]
    fn permission_denied_maps_to_403() {
        let err = io_error(
            Path::new("/etc/hosts"),
            std::io::Error::new(ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(err.code(), "fs.denied");
        assert_eq!(err.http_status(), 403);
        assert!(err.message().contains("权限不足"), "{}", err.message());
    }

    #[cfg(unix)]
    #[test]
    fn raw_eacces_maps_to_403() {
        let err = io_error(
            Path::new("/etc/hosts"),
            std::io::Error::from_raw_os_error(libc::EACCES),
        );
        assert_eq!(err.code(), "fs.denied");
        assert_eq!(err.http_status(), 403);
    }

    #[cfg(unix)]
    #[test]
    fn erofs_maps_to_403_with_readable_reason() {
        let err = io_error(
            Path::new("/etc/skiff-iter-probe.txt"),
            std::io::Error::from_raw_os_error(libc::EROFS),
        );
        assert_eq!(err.code(), "fs.read_only");
        assert_eq!(err.http_status(), 403);
        assert!(err.message().contains("只读"), "{}", err.message());
        assert!(
            err.message().contains("/etc/skiff-iter-probe.txt"),
            "{}",
            err.message()
        );
    }

    #[cfg(unix)]
    #[test]
    fn enospc_maps_to_507_with_readable_reason() {
        let err = io_error(
            Path::new("/www/probe.bin"),
            std::io::Error::from_raw_os_error(libc::ENOSPC),
        );
        assert_eq!(err.code(), "fs.no_space");
        assert_eq!(err.http_status(), 507);
        assert!(err.message().contains("空间不足"), "{}", err.message());
    }

    #[cfg(unix)]
    #[test]
    fn edquot_maps_to_507_with_readable_reason() {
        let err = io_error(
            Path::new("/www/probe.bin"),
            std::io::Error::from_raw_os_error(libc::EDQUOT),
        );
        assert_eq!(err.code(), "fs.no_space");
        assert_eq!(err.http_status(), 507);
        assert!(err.message().contains("配额"), "{}", err.message());
    }

    #[test]
    fn unknown_io_error_stays_500() {
        let err = io_error(
            Path::new("/www/probe.bin"),
            std::io::Error::other("device exploded"),
        );
        assert_eq!(err.code(), "fs.io");
        assert_eq!(err.http_status(), 500);
    }

    #[cfg(unix)]
    #[test]
    fn unknown_os_error_stays_500() {
        let err = io_error(
            Path::new("/www/probe.bin"),
            std::io::Error::from_raw_os_error(libc::EIO),
        );
        assert_eq!(err.code(), "fs.io");
        assert_eq!(err.http_status(), 500);
    }
}
