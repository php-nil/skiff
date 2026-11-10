//! 任务存储适配：`TaskStore` 的 SQLite 实现，以及行与领域对象的双向映射。
//!
//! 本模块只做持久化搬运，不解释调度与执行语义；tick、单飞、超时、回执都在根模块。

use std::sync::Arc;

use serde_json::Value;
use skiff_sched::{
    AuditActor, AuditEvent, AuditOutcome, RunRecord, Task, TaskError, TaskKind, TaskSource,
    TaskSpec, TaskStore, Trigger,
};
use skiff_store::{Db, ScheduledTaskRow, TaskRunRow};

// ---------------------------------------------------------------------------
// SQLite 仓储适配
// ---------------------------------------------------------------------------

/// `TaskStore` 的 SQLite 实现。读写都走 `Arc<Db>`，`&mut self` 只是 trait 形状。
#[derive(Clone)]
pub struct SqliteTaskStore {
    db: Arc<Db>,
}

impl SqliteTaskStore {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    fn put_row(&self, task: &Task) -> Result<(), TaskError> {
        self.db
            .put_scheduled_task(&row_of(task)?)
            .map_err(|e| TaskError::Store(e.to_string()))
    }
}

impl TaskStore for SqliteTaskStore {
    fn list_tasks(&self) -> Vec<Task> {
        match self.db.scheduled_tasks() {
            Ok(rows) => rows.iter().filter_map(task_of).collect(),
            Err(e) => {
                tracing::warn!(error = %e, "读取定时任务失败");
                Vec::new()
            }
        }
    }

    fn get_task(&self, id: &str) -> Option<Task> {
        match self.db.scheduled_task(id) {
            Ok(row) => row.as_ref().and_then(task_of),
            Err(e) => {
                tracing::warn!(error = %e, task = id, "读取定时任务失败");
                None
            }
        }
    }

    fn put_task(&mut self, task: Task) -> Result<(), TaskError> {
        // 调度器对持久化结果只做 `let _ =`，失败必须在存储层留下痕迹，
        // 否则任务停在内存里、重启即丢，运维完全看不到。
        let out = self.put_row(&task);
        if let Err(e) = &out {
            tracing::warn!(error = %e, task = %task.id(), "持久化定时任务失败");
        }
        out
    }

    fn delete_task(&mut self, id: &str) -> Result<Option<Task>, TaskError> {
        let row = self
            .db
            .delete_scheduled_task(id)
            .map_err(|e| TaskError::Store(e.to_string()))?;
        Ok(row.as_ref().and_then(task_of))
    }

    fn append_run(&mut self, run: RunRecord) -> Result<(), TaskError> {
        let out = self
            .db
            .append_task_run(&run_row(&run))
            .map_err(|e| TaskError::Store(e.to_string()));
        if let Err(e) = &out {
            tracing::warn!(error = %e, task = %run.task_id, run = %run.id, "写入任务运行记录失败");
        }
        out
    }

    fn list_runs(&self, task_id: &str, limit: usize) -> Vec<RunRecord> {
        match self.db.task_runs(task_id, limit) {
            Ok(rows) => rows.iter().filter_map(run_of).collect(),
            Err(e) => {
                tracing::warn!(error = %e, task = task_id, "读取运行历史失败");
                Vec::new()
            }
        }
    }

    fn append_audit(&mut self, event: AuditEvent) -> Result<(), TaskError> {
        let out = self
            .db
            .audit(
                &event.actor.as_string(),
                "sched",
                event.action_str(),
                &event.target,
                event.outcome.as_str(),
                event.detail.as_deref().unwrap_or(""),
            )
            .map_err(|e| TaskError::Store(e.to_string()));
        if let Err(e) = &out {
            tracing::warn!(error = %e, target = %event.target, "写入调度审计失败");
        }
        out
    }

    fn list_audits(&self, limit: usize) -> Vec<AuditEvent> {
        self.db
            .sched_audits(limit)
            .unwrap_or_default()
            .into_iter()
            .filter_map(audit_of)
            .collect()
    }
}

pub(super) fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn row_of(task: &Task) -> Result<ScheduledTaskRow, TaskError> {
    let spec = &task.spec;
    Ok(ScheduledTaskRow {
        id: spec.id.to_string(),
        kind: spec.kind.to_string_value(),
        source: spec.source.to_string_value(),
        schedule_json: serde_json::to_string(&spec.schedule)
            .map_err(|e| TaskError::Store(e.to_string()))?,
        enabled: spec.enabled,
        payload_json: serde_json::to_string(&spec.payload)
            .map_err(|e| TaskError::Store(e.to_string()))?,
        timeout_secs: spec.timeout_secs as i64,
        retry_json: serde_json::to_string(&spec.retry)
            .map_err(|e| TaskError::Store(e.to_string()))?,
        concurrency: enum_str(&spec.concurrency),
        missed: enum_str(&spec.missed),
        buffer_json: spec
            .buffer
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| TaskError::Store(e.to_string()))?,
        lifecycle: enum_str(&task.state),
        blocked_reason: task.blocked_reason.as_ref().map(enum_str),
        last_run_json: task
            .last_run
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| TaskError::Store(e.to_string()))?,
        created_by: spec.created_by.clone(),
        created_at: spec.created_at,
        updated_at: spec.updated_at,
    })
}

fn task_of(row: &ScheduledTaskRow) -> Option<Task> {
    let parsed = (|| {
        let kind = TaskKind::parse(&row.kind).ok()?;
        let source = TaskSource::parse(&row.source).ok()?;
        let spec = TaskSpec {
            id: skiff_sched::TaskId::new(row.id.clone()).ok()?,
            kind,
            source,
            schedule: serde_json::from_str(&row.schedule_json).ok()?,
            enabled: row.enabled,
            payload: serde_json::from_str(&row.payload_json).unwrap_or(Value::Null),
            timeout_secs: row.timeout_secs as u64,
            retry: serde_json::from_str(&row.retry_json).unwrap_or_default(),
            concurrency: serde_json::from_value(Value::String(row.concurrency.clone()))
                .unwrap_or_default(),
            missed: serde_json::from_value(Value::String(row.missed.clone())).unwrap_or_default(),
            buffer: row
                .buffer_json
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            created_by: row.created_by.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        };
        let state = serde_json::from_value(Value::String(row.lifecycle.clone())).ok()?;
        let blocked_reason = row
            .blocked_reason
            .as_ref()
            .and_then(|s| serde_json::from_value(Value::String(s.clone())).ok());
        let last_run = row
            .last_run_json
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());
        Some(Task {
            spec,
            state,
            blocked_reason,
            last_run,
        })
    })();
    if parsed.is_none() {
        tracing::warn!(task = %row.id, "任务行不可解析，已跳过");
    }
    parsed
}

pub(super) fn run_row(run: &RunRecord) -> TaskRunRow {
    TaskRunRow {
        id: run.id.clone(),
        task_id: run.task_id.to_string(),
        trigger: run.trigger.as_str().to_string(),
        attempt: run.attempt as i64,
        started_at: run.started_at,
        finished_at: run.finished_at,
        result: run.result.as_str().to_string(),
        error_class: run.error_class.map(|c| c.as_str().to_string()),
        error_detail: run.error_detail.clone(),
        duration_ms: run.duration_ms,
        request_id: run.request_id.clone(),
        consumed_upto: run.consumed_upto.map(|v| v as i64),
        output_bytes: run.output_bytes as i64,
        missed_count: run.missed_count as i64,
    }
}

fn run_of(row: &TaskRunRow) -> Option<RunRecord> {
    let parse = |v: &str| serde_json::from_value::<Trigger>(Value::String(v.to_string())).ok();
    Some(RunRecord {
        id: row.id.clone(),
        task_id: skiff_sched::TaskId::new(row.task_id.clone()).ok()?,
        trigger: parse(&row.trigger)?,
        attempt: row.attempt as u32,
        started_at: row.started_at,
        finished_at: row.finished_at,
        result: serde_json::from_value(Value::String(row.result.clone())).ok()?,
        error_class: row
            .error_class
            .as_ref()
            .and_then(|s| serde_json::from_value(Value::String(s.clone())).ok()),
        error_detail: row.error_detail.clone(),
        duration_ms: row.duration_ms,
        request_id: row.request_id.clone(),
        consumed_upto: row.consumed_upto.map(|v| v as u64),
        output_bytes: row.output_bytes as usize,
        missed_count: row.missed_count as u64,
    })
}

fn audit_of(e: skiff_store::AuditEntry) -> Option<AuditEvent> {
    let action = e.action.parse_action()?;
    let actor = if e.actor == "system" {
        AuditActor::System
    } else if let Some(id) = e.actor.strip_prefix("plugin:") {
        AuditActor::Plugin(id.to_string())
    } else if let Some(name) = e.actor.strip_prefix("admin:") {
        AuditActor::Admin(name.to_string())
    } else if e.actor == "admin" {
        AuditActor::Admin(String::new())
    } else {
        return None;
    };
    let outcome = match e.result.as_str() {
        "ok" => AuditOutcome::Ok,
        "fail" => AuditOutcome::Fail,
        "skipped" => AuditOutcome::Skipped,
        _ => return None,
    };
    Some(AuditEvent {
        at: e.ts,
        action,
        actor,
        target: e.target,
        outcome,
        detail: Some(e.detail).filter(|s| !s.is_empty()),
    })
}

/// 审计动作字符串 → 枚举。`skiff-sched` 未暴露 `FromStr`，这里只覆盖写库时的集合。
trait ParseAction {
    fn parse_action(&self) -> Option<skiff_sched::AuditAction>;
}

impl ParseAction for String {
    fn parse_action(&self) -> Option<skiff_sched::AuditAction> {
        use skiff_sched::AuditAction as A;
        Some(match self.as_str() {
            "task.create" => A::TaskCreate,
            "task.update" => A::TaskUpdate,
            "task.delete" => A::TaskDelete,
            "task.trigger" => A::TaskTrigger,
            "task.run" => A::TaskRun,
            "task.blocked" => A::TaskBlocked,
            "buffer.high_watermark" => A::BufferHighWatermark,
            "buffer.overflow" => A::BufferOverflow,
            "buffer.recovered" => A::BufferRecovered,
            "buffer.point_too_large" => A::BufferPointTooLarge,
            "buffer.lost_on_restart" => A::BufferLostOnRestart,
            "clock_skew" => A::ClockSkew,
            "scheduler.deny" => A::SchedulerDeny,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::enum_str;
    use skiff_sched::{BlockedReason, Concurrency, MissedPolicy, TaskState};

    /// `enum_str` 依赖 `serde_json::to_value().as_str()`，只对 unit variant 成立；
    /// 带载荷的变体会静默落空串。逐个变体钉住这个约定：新增带载荷变体时先在这里红。
    #[test]
    fn enum_str_requires_unit_variants() {
        for v in [Concurrency::Skip, Concurrency::QueueOne] {
            assert!(!enum_str(&v).is_empty(), "Concurrency::{v:?} 落库为空串");
        }
        for v in [MissedPolicy::Skip, MissedPolicy::Once, MissedPolicy::All] {
            assert!(!enum_str(&v).is_empty(), "MissedPolicy::{v:?} 落库为空串");
        }
        for v in [
            BlockedReason::NotInstalled,
            BlockedReason::PluginDisabled,
            BlockedReason::ApiMismatch,
            BlockedReason::InvalidSchedule,
            BlockedReason::ClockSkew,
            BlockedReason::MissingExecutor,
        ] {
            assert!(!enum_str(&v).is_empty(), "BlockedReason::{v:?} 落库为空串");
        }
        for v in [
            TaskState::Idle,
            TaskState::Queued,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Failed,
            TaskState::Disabled,
        ] {
            assert!(!enum_str(&v).is_empty(), "TaskState::{v:?} 落库为空串");
        }
    }
}
