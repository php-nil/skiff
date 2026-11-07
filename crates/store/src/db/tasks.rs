//! 内核定时任务的持久化（`docs/KERNEL_SCHEDULER_CONTRACT.md` §7）。
//!
//! 这里只存「任务定义 + 运行历史」，**不认识调度语义**：`schedule/payload/retry/buffer`
//! 一律按 JSON 文本原样保存，枚举到字符串的映射由 `skiff-app::tasks` 适配器负责。
//! 这样 `skiff-store` 不必反向依赖 `skiff-sched`，也不会把调度规则漏进存储层。

use super::{Db, Result};
use crate::model::AuditEntry;
use rusqlite::{params, OptionalExtension};

/// 一条任务定义（`scheduled_tasks`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskRow {
    pub id: String,
    /// `builtin:<name>` / `plugin:<id>:<method>`。
    pub kind: String,
    /// `site:<domain>` / `docker:<id>` / `plugin:<id>` / `system`。
    pub source: String,
    /// `Schedule` 的 JSON。
    pub schedule_json: String,
    pub enabled: bool,
    pub payload_json: String,
    pub timeout_secs: i64,
    /// `RetryPolicy` 的 JSON。
    pub retry_json: String,
    pub concurrency: String,
    pub missed: String,
    /// `BufferConfig` 的 JSON；`NULL` 表示无缓冲。
    pub buffer_json: Option<String>,
    /// `active` / `blocked` / `disabled_by_system`。
    pub lifecycle: String,
    pub blocked_reason: Option<String>,
    /// `LastRun` 的 JSON。
    pub last_run_json: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 一条运行历史（`task_runs`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRunRow {
    pub id: String,
    pub task_id: String,
    pub trigger: String,
    pub attempt: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub result: String,
    pub error_class: Option<String>,
    pub error_detail: Option<String>,
    pub duration_ms: i64,
    pub request_id: Option<String>,
    pub consumed_upto: Option<i64>,
    pub output_bytes: i64,
    pub missed_count: i64,
}

impl Db {
    /// 插入或更新任务定义（按 `id` 主键）。任务身份是 `id`：同一 `(kind, source)`
    /// 允许挂多条（插件任务靠 id 里的 `local_key` 区分），因此没有该组合的唯一索引。
    pub fn put_scheduled_task(&self, t: &ScheduledTaskRow) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO scheduled_tasks(id, kind, source, schedule_json, enabled, payload_json,
                                          timeout_secs, retry_json, concurrency, missed, buffer_json,
                                          lifecycle, blocked_reason, last_run_json, created_by,
                                          created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                kind = excluded.kind, source = excluded.source,
                schedule_json = excluded.schedule_json, enabled = excluded.enabled,
                payload_json = excluded.payload_json, timeout_secs = excluded.timeout_secs,
                retry_json = excluded.retry_json, concurrency = excluded.concurrency,
                missed = excluded.missed, buffer_json = excluded.buffer_json,
                lifecycle = excluded.lifecycle, blocked_reason = excluded.blocked_reason,
                last_run_json = excluded.last_run_json, updated_at = excluded.updated_at",
            params![
                t.id,
                t.kind,
                t.source,
                t.schedule_json,
                t.enabled as i64,
                t.payload_json,
                t.timeout_secs,
                t.retry_json,
                t.concurrency,
                t.missed,
                t.buffer_json,
                t.lifecycle,
                t.blocked_reason,
                t.last_run_json,
                t.created_by,
                t.created_at,
                t.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn scheduled_tasks(&self) -> Result<Vec<ScheduledTaskRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, kind, source, schedule_json, enabled, payload_json, timeout_secs,
                    retry_json, concurrency, missed, buffer_json, lifecycle, blocked_reason,
                    last_run_json, created_by, created_at, updated_at
             FROM scheduled_tasks ORDER BY id",
        )?;
        let rows = stmt.query_map([], task_from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn scheduled_task(&self, id: &str) -> Result<Option<ScheduledTaskRow>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT id, kind, source, schedule_json, enabled, payload_json, timeout_secs,
                        retry_json, concurrency, missed, buffer_json, lifecycle, blocked_reason,
                        last_run_json, created_by, created_at, updated_at
                 FROM scheduled_tasks WHERE id = ?1",
                params![id],
                task_from_row,
            )
            .optional()?)
    }

    /// 删除任务定义；历史与审计保留（保留期由调用方的维护巡检负责）。
    pub fn delete_scheduled_task(&self, id: &str) -> Result<Option<ScheduledTaskRow>> {
        let existing = self.scheduled_task(id)?;
        if existing.is_some() {
            let conn = self.conn();
            conn.execute("DELETE FROM scheduled_tasks WHERE id = ?1", params![id])?;
        }
        Ok(existing)
    }

    pub fn append_task_run(&self, r: &TaskRunRow) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR REPLACE INTO task_runs(id, task_id, trigger, attempt, started_at,
                                              finished_at, result, error_class, error_detail,
                                              duration_ms, request_id, consumed_upto, output_bytes,
                                              missed_count)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                r.id,
                r.task_id,
                r.trigger,
                r.attempt,
                r.started_at,
                r.finished_at,
                r.result,
                r.error_class,
                r.error_detail,
                r.duration_ms,
                r.request_id,
                r.consumed_upto,
                r.output_bytes,
                r.missed_count
            ],
        )?;
        Ok(())
    }

    /// 按结束时间倒序取最近 `limit` 条。
    pub fn task_runs(&self, task_id: &str, limit: usize) -> Result<Vec<TaskRunRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, trigger, attempt, started_at, finished_at, result, error_class,
                    error_detail, duration_ms, request_id, consumed_upto, output_bytes, missed_count
             FROM task_runs WHERE task_id = ?1
             ORDER BY finished_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![task_id, limit as i64], run_from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 清理早于 `before` 的运行历史（契约：默认保留 7 天）。
    pub fn prune_task_runs(&self, before: i64) -> Result<usize> {
        let conn = self.conn();
        Ok(conn.execute(
            "DELETE FROM task_runs WHERE finished_at < ?1",
            params![before],
        )?)
    }

    /// 调度器写进 `audit` 表的流水，按动作前缀回收，供任务页展示。
    pub fn sched_audits(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, ts, actor, ip, action, target, result, detail
             FROM audit
             WHERE action LIKE 'task.%' OR action LIKE 'buffer.%'
                OR action = 'clock_skew' OR action = 'scheduler.deny'
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(AuditEntry {
                id: r.get(0)?,
                ts: r.get(1)?,
                actor: r.get(2)?,
                ip: r.get(3)?,
                action: r.get(4)?,
                target: r.get(5)?,
                result: r.get(6)?,
                detail: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 启动校正没有 SQL 要做：运行态（`running`）只活在内存，库里根本没有该列；
    /// `blocked` 是插件可用性等外部事实的缓存，装载后会按当前插件状态重新判定。
    /// 保留显式占位让调用方一眼看到「这里刻意什么都不写」。
    pub fn mark_tasks_loaded(&self) -> Result<()> {
        Ok(())
    }
}

fn task_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTaskRow> {
    Ok(ScheduledTaskRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        source: r.get(2)?,
        schedule_json: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        payload_json: r.get(5)?,
        timeout_secs: r.get(6)?,
        retry_json: r.get(7)?,
        concurrency: r.get(8)?,
        missed: r.get(9)?,
        buffer_json: r.get(10)?,
        lifecycle: r.get(11)?,
        blocked_reason: r.get(12)?,
        last_run_json: r.get(13)?,
        created_by: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}

fn run_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRunRow> {
    Ok(TaskRunRow {
        id: r.get(0)?,
        task_id: r.get(1)?,
        trigger: r.get(2)?,
        attempt: r.get(3)?,
        started_at: r.get(4)?,
        finished_at: r.get(5)?,
        result: r.get(6)?,
        error_class: r.get(7)?,
        error_detail: r.get(8)?,
        duration_ms: r.get(9)?,
        request_id: r.get(10)?,
        consumed_upto: r.get(11)?,
        output_bytes: r.get(12)?,
        missed_count: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_row(id: &str, kind: &str, source: &str) -> ScheduledTaskRow {
        ScheduledTaskRow {
            id: id.into(),
            kind: kind.into(),
            source: source.into(),
            schedule_json: r#"{"Cron":{"expr":"0 3 * * *","tz":null,"jitter_secs":0}}"#.into(),
            enabled: true,
            payload_json: "{}".into(),
            timeout_secs: 30,
            retry_json: "{}".into(),
            concurrency: "skip".into(),
            missed: "skip".into(),
            buffer_json: None,
            lifecycle: "active".into(),
            blocked_reason: None,
            last_run_json: None,
            created_by: "system".into(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn run_row(id: &str, task_id: &str, finished_at: i64) -> TaskRunRow {
        TaskRunRow {
            id: id.into(),
            task_id: task_id.into(),
            trigger: "schedule".into(),
            attempt: 0,
            started_at: finished_at - 1,
            finished_at,
            result: "ok".into(),
            error_class: None,
            error_detail: None,
            duration_ms: 1000,
            request_id: None,
            consumed_upto: None,
            output_bytes: 0,
            missed_count: 0,
        }
    }

    #[test]
    fn tasks_roundtrip_and_delete() {
        let db = Db::open_in_memory().unwrap();
        db.put_scheduled_task(&task_row(
            "builtin:db-vacuum",
            "builtin:db-vacuum",
            "system",
        ))
        .unwrap();
        assert_eq!(db.scheduled_tasks().unwrap().len(), 1);
        let got = db.scheduled_task("builtin:db-vacuum").unwrap().unwrap();
        assert_eq!(got.timeout_secs, 30);
        assert!(got.enabled);

        db.delete_scheduled_task("builtin:db-vacuum").unwrap();
        assert!(db.scheduled_task("builtin:db-vacuum").unwrap().is_none());
    }

    #[test]
    fn duplicate_kind_and_source_allowed_with_distinct_ids() {
        let db = Db::open_in_memory().unwrap();
        db.put_scheduled_task(&task_row(
            "plugin:com.skiff.cron:1",
            "plugin:com.skiff.cron:noop",
            "plugin:com.skiff.cron",
        ))
        .unwrap();
        // v12 起同一 (kind, source) 允许挂多条：任务身份是 id 里的 local_key。
        db.put_scheduled_task(&task_row(
            "plugin:com.skiff.cron:2",
            "plugin:com.skiff.cron:noop",
            "plugin:com.skiff.cron",
        ))
        .expect("同一 (kind,source) 的第二个任务不应被拒绝");
        assert_eq!(db.scheduled_tasks().unwrap().len(), 2);
    }

    #[test]
    fn run_history_is_newest_first_and_limited() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..5 {
            db.append_task_run(&run_row(&format!("r{i}"), "t", 10 + i))
                .unwrap();
        }
        let runs = db.task_runs("t", 3).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].id, "r4");
        assert_eq!(runs[2].id, "r2");
    }

    #[test]
    fn prune_removes_old_runs_only() {
        let db = Db::open_in_memory().unwrap();
        db.append_task_run(&run_row("old", "t", 10)).unwrap();
        db.append_task_run(&run_row("new", "t", 100)).unwrap();
        assert_eq!(db.prune_task_runs(50).unwrap(), 1);
        assert_eq!(db.task_runs("t", 10).unwrap().len(), 1);
    }
}
