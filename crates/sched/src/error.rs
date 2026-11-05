//! 调度层的统一错误类型。
//!
//! 字段级错误（id/kind/source/schedule/payload/…）都带上原始值，方便调用方在表单里
//! 内联提示；协议错误与存储错误分开，避免把「执行体回执越界」误当成参数问题。

use crate::TaskState;

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("任务 id 非法（允许 [A-Za-z0-9._:-]，1–96 字符）：{0}")]
    InvalidId(String),
    #[error("任务 kind 非法（应为 builtin:<name> 或 plugin:<id>:<method>）：{0}")]
    InvalidKind(String),
    #[error(
        "任务 source 非法（应为 site:<domain> / docker:<container> / plugin:<id> / system）：{0}"
    )]
    InvalidSource(String),
    #[error("调度表达式非法：{0}")]
    InvalidSchedule(String),
    #[error("payload 非法：{0}")]
    InvalidPayload(String),
    #[error("超时配置非法：{0} 秒（允许 1–600）")]
    InvalidTimeout(u64),
    #[error("重试配置非法：{0}")]
    InvalidRetry(String),
    #[error("缓冲配置非法：{0}")]
    InvalidBuffer(String),
    #[error("任务不存在：{0}")]
    NotFound(String),
    #[error("任务 id 已被占用：{0}")]
    DuplicateId(String),
    #[error("任务正忙：{0}")]
    Busy(String),
    #[error("任务已停用：{0}")]
    Disabled(String),
    #[error("任务状态迁移非法：{from:?} -> {to:?}")]
    InvalidTransition { from: TaskState, to: TaskState },
    #[error("运行协议错误：{0}")]
    Protocol(String),
    #[error("存储错误：{0}")]
    Store(String),
    #[error("采样点超过单点上限：{bytes} B > {limit} B")]
    PointTooLarge { bytes: usize, limit: usize },
    #[error("缓冲已满：{0}")]
    BufferFull(String),
}
