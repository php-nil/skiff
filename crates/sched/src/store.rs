//! 持久化边界的 trait 与内存实现。
//!
//! SQLite 接线时在 `skiff-store` 里实现同一个 trait 即可，调度器不需要知道表结构。
//! 运行历史与审计都由 store 负责保留策略（契约：运行历史 7 天、审计长期保留）。

use std::collections::{BTreeMap, HashMap};

use crate::audit::AuditEvent;
use crate::error::TaskError;
use crate::exec::RunRecord;
use crate::model::Task;

pub trait TaskStore {
    fn list_tasks(&self) -> Vec<Task>;
    fn get_task(&self, id: &str) -> Option<Task>;
    fn put_task(&mut self, task: Task) -> Result<(), TaskError>;
    /// 删除任务定义；历史与审计仍保留（由 store 决定保留期）。
    fn delete_task(&mut self, id: &str) -> Result<Option<Task>, TaskError>;
    fn append_run(&mut self, run: RunRecord) -> Result<(), TaskError>;
    /// 最近的运行记录，按时间倒序，最多 `limit` 条。
    fn list_runs(&self, task_id: &str, limit: usize) -> Vec<RunRecord>;
    fn append_audit(&mut self, event: AuditEvent) -> Result<(), TaskError>;
    fn list_audits(&self, limit: usize) -> Vec<AuditEvent>;
}

/// 纯内存实现：测试替身，也可用于 `--dev` 模式的临时运行。
#[derive(Debug, Default)]
pub struct MemoryTaskStore {
    tasks: BTreeMap<String, Task>,
    runs: HashMap<String, Vec<RunRecord>>,
    audits: Vec<AuditEvent>,
}

impl MemoryTaskStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn audit_len(&self) -> usize {
        self.audits.len()
    }
}

impl TaskStore for MemoryTaskStore {
    fn list_tasks(&self) -> Vec<Task> {
        self.tasks.values().cloned().collect()
    }

    fn get_task(&self, id: &str) -> Option<Task> {
        self.tasks.get(id).cloned()
    }

    fn put_task(&mut self, task: Task) -> Result<(), TaskError> {
        self.tasks.insert(task.spec.id.to_string(), task);
        Ok(())
    }

    fn delete_task(&mut self, id: &str) -> Result<Option<Task>, TaskError> {
        Ok(self.tasks.remove(id))
    }

    fn append_run(&mut self, run: RunRecord) -> Result<(), TaskError> {
        self.runs
            .entry(run.task_id.to_string())
            .or_default()
            .push(run);
        Ok(())
    }

    fn list_runs(&self, task_id: &str, limit: usize) -> Vec<RunRecord> {
        self.runs
            .get(task_id)
            .map(|runs| runs.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    fn append_audit(&mut self, event: AuditEvent) -> Result<(), TaskError> {
        self.audits.push(event);
        Ok(())
    }

    fn list_audits(&self, limit: usize) -> Vec<AuditEvent> {
        self.audits.iter().rev().take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::exec::{RunRecord, RunResult, Trigger};
    use crate::model::{Schedule, TaskKind, TaskSource, TaskSpec};

    fn sample_task() -> Task {
        let spec = TaskSpec::new(
            "t1",
            TaskKind::builtin("site-backup").unwrap(),
            TaskSource::System,
            Schedule::interval(60),
        )
        .unwrap();
        Task::new(spec)
    }

    fn sample_run() -> RunRecord {
        RunRecord {
            id: "r1".to_string(),
            task_id: "t1".parse().unwrap(),
            trigger: Trigger::Manual,
            attempt: 1,
            started_at: 10,
            finished_at: 20,
            result: RunResult::Ok,
            error_class: None,
            error_detail: None,
            duration_ms: 10_000,
            request_id: None,
            consumed_upto: Some(1),
            output_bytes: 0,
            missed_count: 0,
        }
    }

    #[test]
    fn store_roundtrip_and_reverse_order() {
        let mut store = MemoryTaskStore::new();
        let task = sample_task();
        store.put_task(task.clone()).unwrap();
        assert_eq!(store.get_task("t1"), Some(task));

        let mut first = sample_run();
        first.id = "r1".to_string();
        let mut second = sample_run();
        second.id = "r2".to_string();
        store.append_run(first).unwrap();
        store.append_run(second).unwrap();
        let runs = store.list_runs("t1", 2);
        assert_eq!(runs[0].id, "r2");
        assert_eq!(runs[1].id, "r1");

        store
            .append_audit(AuditEvent::new(
                0,
                crate::audit::AuditAction::TaskCreate,
                crate::audit::AuditActor::System,
                "t1",
                crate::audit::AuditOutcome::Ok,
                Some(json!({"x":1}).to_string()),
            ))
            .unwrap();
        assert_eq!(store.list_audits(1).len(), 1);
        assert_eq!(store.audit_len(), 1);
    }
}
