//! 执行体接口、回执、运行记录（契约 §4.3 / §5.4 / §7.1）。
//!
//! 执行是**两段式**：采样（可选）与批量执行。内核只把只读批次交出去，执行体用
//! [`Verdict`] 声明消费到哪；没给回执等价于 `keep`（保守，不丢数据）。
//!
//! 真正的 async 插件 RPC 在 `skiff-core` 里用 `Executor` 包一层，本 crate 保持同步。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::buffer::{SamplePoint, POINT_OVERHEAD_BYTES};
use crate::model::{Task, TaskId, TaskKind, TaskSource};

/// 触发方式；重试与补跑单独标出来，便于审计/历史区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Schedule,
    Manual,
    CatchUp,
    Retry,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Schedule => "schedule",
            Trigger::Manual => "manual",
            Trigger::CatchUp => "catch_up",
            Trigger::Retry => "retry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Ok,
    Fail,
    Timeout,
    SkippedBusy,
    Cancelled,
}

impl RunResult {
    pub fn as_str(self) -> &'static str {
        match self {
            RunResult::Ok => "ok",
            RunResult::Fail => "fail",
            RunResult::Timeout => "timeout",
            RunResult::SkippedBusy => "skipped_busy",
            RunResult::Cancelled => "cancelled",
        }
    }
}

/// 失败分类决定是否重试（契约 §5.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Spawn,
    Timeout,
    Unavailable,
    InvalidParams,
    ExecutorFailed,
    Protocol,
    Cancelled,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorClass::Spawn => "spawn",
            ErrorClass::Timeout => "timeout",
            ErrorClass::Unavailable => "unavailable",
            ErrorClass::InvalidParams => "invalid_params",
            ErrorClass::ExecutorFailed => "executor_failed",
            ErrorClass::Protocol => "protocol",
            ErrorClass::Cancelled => "cancelled",
        }
    }

    /// 只有拉起 / 超时 / 暂不可用属于「瞬时」，可以重试。
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            ErrorClass::Spawn | ErrorClass::Timeout | ErrorClass::Unavailable
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPhase {
    Ensure,
    Health,
    Call,
    Verdict,
}

impl ErrorPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorPhase::Ensure => "ensure",
            ErrorPhase::Health => "health",
            ErrorPhase::Call => "call",
            ErrorPhase::Verdict => "verdict",
        }
    }
}

/// 可定位的失败：任务 / 来源在 `RunContext` 上，这里补插件 id、request id 与阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecError {
    pub class: ErrorClass,
    pub message: String,
    pub phase: ErrorPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ExecError {
    pub fn new(class: ErrorClass, phase: ErrorPhase, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            phase,
            plugin_id: None,
            request_id: None,
        }
    }

    pub fn with_plugin(mut self, plugin_id: impl Into<String>) -> Self {
        self.plugin_id = Some(plugin_id.into());
        self
    }

    pub fn with_request(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Protocol, ErrorPhase::Verdict, message)
    }
}

/// 执行体回执。`persisted_upto` 必须落在本批次范围内，越界按协议失败且缓冲不动。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Persist { persisted_upto: u64 },
    Keep,
    Discard,
    Partial { persisted_upto: u64 },
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Persist { .. } => "persist",
            Verdict::Keep => "keep",
            Verdict::Discard => "discard",
            Verdict::Partial { .. } => "partial",
        }
    }

    pub fn persisted_upto(&self) -> Option<u64> {
        match self {
            Verdict::Persist { persisted_upto } | Verdict::Partial { persisted_upto } => {
                Some(*persisted_upto)
            }
            Verdict::Keep | Verdict::Discard => None,
        }
    }

    pub fn requires_watermark(&self) -> bool {
        matches!(self, Verdict::Persist { .. } | Verdict::Partial { .. })
    }
}

/// 执行体返回值。`verdict: None` 等价于 `keep`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub output_bytes: usize,
}

/// 交给执行体的只读批次。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub snapshot_id: u64,
    pub points: Vec<SamplePoint>,
    /// 是否因为 `max_batch_bytes` 被截断（剩余点下一轮继续）。
    pub truncated: bool,
}

impl Batch {
    pub fn empty() -> Self {
        Self {
            snapshot_id: 0,
            points: Vec::new(),
            truncated: false,
        }
    }

    pub fn first_seq(&self) -> Option<u64> {
        self.points.first().map(|p| p.seq)
    }

    pub fn last_seq(&self) -> Option<u64> {
        self.points.last().map(|p| p.seq)
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 批次体积估算：序列化字节 + 每点 32 B 管理开销，与缓冲计量口径一致。
    pub fn bytes(&self) -> usize {
        self.points
            .iter()
            .map(|p| {
                serde_json::to_vec(&p.data)
                    .map(|v| v.len() + POINT_OVERHEAD_BYTES)
                    .unwrap_or(POINT_OVERHEAD_BYTES)
            })
            .sum()
    }
}

/// 运行上下文：执行体据此定位任务、来源、插件与取消位。
#[derive(Debug)]
pub struct RunContext {
    pub run_id: String,
    pub task_id: TaskId,
    pub kind: TaskKind,
    pub source: TaskSource,
    pub trigger: Trigger,
    pub attempt: u32,
    pub started_at: i64,
    pub snapshot_id: u64,
    pub missed_count: u64,
    pub payload: Value,
    cancelled: Arc<AtomicBool>,
}

impl RunContext {
    pub fn new(
        task: &Task,
        run_id: String,
        trigger: Trigger,
        attempt: u32,
        started_at: i64,
        snapshot_id: u64,
        missed_count: u64,
    ) -> Self {
        Self {
            run_id,
            task_id: task.spec.id.clone(),
            kind: task.spec.kind.clone(),
            source: task.spec.source.clone(),
            trigger,
            attempt,
            started_at,
            snapshot_id,
            missed_count,
            payload: task.spec.payload.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancellation_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
}

/// 一次待执行：上层拿到它去跑执行体，跑完把结果交回 [`crate::Scheduler::finish`]。
#[derive(Debug)]
pub struct RunTicket {
    pub context: RunContext,
    pub batch: Batch,
}

#[derive(Debug, Clone)]
pub struct SampleContext {
    pub task_id: TaskId,
    pub method: String,
    pub at: i64,
}

/// 一次待采样。与执行一样是两段式，避免把插件 RPC 塞进调度器内部。
#[derive(Debug, Clone)]
pub struct SampleTicket {
    pub context: SampleContext,
    pub at: i64,
}

/// 执行体接口：`sample` 产出一个采样点，`execute` 消费一批点并给回执。
pub trait Executor {
    fn sample(&mut self, ctx: &SampleContext) -> Result<Option<Value>, ExecError>;

    fn execute(&mut self, ctx: &RunContext, batch: &Batch) -> Result<ExecOutput, ExecError>;
}

/// 一次执行历史（契约 §7.1 `task_runs` 的字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub task_id: TaskId,
    pub trigger: Trigger,
    pub attempt: u32,
    pub started_at: i64,
    pub finished_at: i64,
    pub result: RunResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<ErrorClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    pub duration_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_upto: Option<u64>,
    pub output_bytes: usize,
    pub missed_count: u64,
}
