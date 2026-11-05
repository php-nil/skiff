//! 内核定时任务调度的**纯逻辑层**（`docs/KERNEL_SCHEDULER_CONTRACT.md`）。
//!
//! 这份 crate 只回答四件事：任务长什么样、缓冲怎么收/怎么放、什么时候该跑、跑完怎么结算。
//! 不碰 SQLite、不碰 Docker、不碰 UDS、不碰 HTTP——这些都由上层接线：
//!
//! - 持久化：`TaskStore`（本 crate 只给内存实现，SQLite 由 `skiff-store` 接入）；
//! - 插件唤醒：`PluginGate`（是否安装/启用/API 兼容由 `skiff-plugin` 回答）；
//! - 执行：`Executor`（真正的 async RPC / 内置实现在 `skiff-core` 里拼装）；
//! - 时间：`Clock`（生产用 `SystemClock`，测试用 `ManualClock`，cron 按注入时区解释）。
//!
//! 调度循环刻意做成**同步**的：[`Scheduler::tick`] 只产出待执行的 [`RunTicket`]，
//! 谁去执行、怎么 await、超时怎么掐都由上层决定，执行完调用 [`Scheduler::finish`] 结算。
//! 这样本 crate 不需要引入 tokio，也让「单飞 / 超时 / 冷却 / 重试」能在纯演算测试里验证。
//!
//! 缓冲只活在内存里（契约 §4.6）：进程重启即清空，本 crate 不做 spill。

mod audit;
mod buffer;
mod clock;
mod error;
mod exec;
mod model;
mod schedule;
mod scheduler;
mod store;

pub use audit::{AuditAction, AuditActor, AuditEvent, AuditOutcome};
pub use buffer::{
    AggregateRule, Buffer, BufferConfig, BufferError, BufferSignal, BufferStat, CommitOutcome,
    OverflowPolicy, PushOutcome, PushReport, RejectReason, SamplePoint, Snapshot, SnapshotMeta,
    DEFAULT_AGGREGATE_WINDOW_SECS, DEFAULT_CAPACITY_BYTES, DEFAULT_MAX_BATCH_BYTES,
    MAX_CAPACITY_BYTES, POINT_MAX_BYTES,
};
pub use clock::{Clock, ManualClock, SystemClock};
pub use error::TaskError;
pub use exec::{
    Batch, ErrorClass, ErrorPhase, ExecError, ExecOutput, Executor, RunContext, RunRecord,
    RunResult, RunTicket, SampleContext, SampleTicket, Trigger, Verdict,
};
pub use model::{
    Align, BlockedReason, Concurrency, CronTz, LastRun, LifecycleState, MissedPolicy, RetryPolicy,
    Schedule, Task, TaskFilter, TaskId, TaskKind, TaskPatch, TaskSource, TaskSpec, TaskState,
    CLOCK_SKEW_PAUSE_SECS, DEFAULT_TIMEOUT_SECS, MAX_JITTER_SECS, MAX_RETRIES, MAX_TIMEOUT_SECS,
    MIN_INTERVAL_SECS, PAYLOAD_MAX_BYTES, PLUGIN_METHOD_MAX_LEN, TASK_ID_MAX_LEN,
};
pub use scheduler::{
    AllPluginsReady, FinishOutcome, PluginGate, PluginStatus, Scheduler, SchedulerConfig,
    TickReport,
};
pub use store::{MemoryTaskStore, TaskStore};
