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
    /// 未归类的 I/O 故障；`code()` 走 `fs.denied` 之外的中性码
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
            FsError::Io(_) => "fs.io",
        }
    }

    /// 管理面 HTTP 状态码。`fs.io` 不在契约表内，按 500 处理。
    pub fn http_status(&self) -> u16 {
        match self {
            FsError::Denied(_) | FsError::Escape(_) | FsError::Protected(_) => 403,
            FsError::NotFound(_) => 404,
            FsError::Conflict(_) => 409,
            FsError::Busy(_) => 423,
            FsError::TooLarge(_) | FsError::FileTooLarge(_) | FsError::TooManyEntries(_) => 413,
            FsError::QuotaExceeded(_) | FsError::TrashFull(_) => 507,
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
            | FsError::Io(m) => m,
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
    use std::io::ErrorKind;
    let ctx = format!("{}: {e}", path.display());
    match e.kind() {
        ErrorKind::NotFound => FsError::NotFound(ctx),
        ErrorKind::PermissionDenied => FsError::Denied(ctx),
        ErrorKind::AlreadyExists => FsError::Conflict(ctx),
        _ => FsError::Io(ctx),
    }
}
