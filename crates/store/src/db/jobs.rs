//! 一次性作业（任务中心）的持久化：`jobs` / `job_logs`。
//!
//! 与 `scheduled_tasks` / `task_runs` 刻意分开：前者是「周期配置 + 运行历史」，
//! 这里是「提交一次的作业 + 可流式追加的过程日志」。作业行是可变的活状态
//! （阶段、进度、取消标记），日志行只追加、按字节封顶后从最旧开始轮转。
//!
//! 设计约束：
//! - `seq` 单调递增且**不因轮转重编号**：客户端游标（`Last-Event-ID`）始终是
//!   「已读到的最大 seq」，轮转后只需告知「更早日志已截断」而不是让游标跳位。
//! - 同目标同活跃作业唯一由部分索引 `idx_jobs_active_target` 保证，重启后仍有效。
//! - 清理分批执行，避免长事务持有 SQLite 写锁。

use super::{Db, Result};
use crate::error::StoreError;
use rusqlite::{params, OptionalExtension};

/// 单任务日志行数上限。
pub const MAX_LOG_LINES: i64 = 4_000;
/// 单任务日志字节上限（2 MiB）。
pub const MAX_LOG_BYTES: i64 = 2 * 1024 * 1024;
/// 作业历史保留天数（终态）。
pub const JOB_RETENTION_DAYS: i64 = 7;
/// 作业历史最多保留条数（终态）。
pub const JOB_KEEP_MAX: i64 = 200;

pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_SUCCEEDED: &str = "succeeded";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_CANCELLED: &str = "cancelled";

pub fn is_active_status(status: &str) -> bool {
    matches!(status, STATUS_QUEUED | STATUS_RUNNING)
}

/// 一条作业（`jobs`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub id: String,
    pub kind: String,
    pub target_type: String,
    pub target_id: String,
    pub title: String,
    pub params_json: String,
    pub phase: String,
    pub phase_label: String,
    pub progress: i64,
    pub status: String,
    pub cancellable: bool,
    pub cancel_requested: bool,
    pub created_by: String,
    pub created_by_role: String,
    pub requester_ip: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: i64,
    pub error_class: Option<String>,
    pub error_detail: Option<String>,
    pub result_json: String,
    pub log_bytes: i64,
    pub log_seq: i64,
    pub retry_of: Option<String>,
    /// 决定该作业成败的那条命令的退出码；无单条命令语义时为 `None`（页面显示 `-`）。
    pub exit_code: Option<i64>,
}

/// 一行作业日志（`job_logs`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLogRow {
    pub job_id: String,
    pub seq: i64,
    pub at: i64,
    pub stream: String,
    pub line: String,
}

/// 追加日志的结果；`truncated_before` 为轮转后现存的最小 seq。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogAppend {
    pub last_seq: i64,
    pub truncated_before: Option<i64>,
}

/// 一批增量日志 + 截断提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLogBatch {
    /// 现存最小 seq；请求起点更早时客户端应提示「更早日志已截断」。
    pub min_seq: Option<i64>,
    pub max_seq: i64,
    pub total_bytes: i64,
    pub entries: Vec<JobLogRow>,
}

const JOB_COLUMNS: &str = "id, kind, target_type, target_id, title, params_json, phase,
    phase_label, progress, status, cancellable, cancel_requested, created_by,
    created_by_role, requester_ip, created_at, started_at, finished_at, duration_ms,
    error_class, error_detail, result_json, log_bytes, log_seq, retry_of, exit_code";

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        target_type: r.get(2)?,
        target_id: r.get(3)?,
        title: r.get(4)?,
        params_json: r.get(5)?,
        phase: r.get(6)?,
        phase_label: r.get(7)?,
        progress: r.get(8)?,
        status: r.get(9)?,
        cancellable: r.get::<_, i64>(10)? != 0,
        cancel_requested: r.get::<_, i64>(11)? != 0,
        created_by: r.get(12)?,
        created_by_role: r.get(13)?,
        requester_ip: r.get(14)?,
        created_at: r.get(15)?,
        started_at: r.get(16)?,
        finished_at: r.get(17)?,
        duration_ms: r.get(18)?,
        error_class: r.get(19)?,
        error_detail: r.get(20)?,
        result_json: r.get(21)?,
        log_bytes: r.get(22)?,
        log_seq: r.get(23)?,
        retry_of: r.get(24)?,
        exit_code: r.get(25)?,
    })
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

impl Db {
    /// 插入一条作业。`(target_type,target_id)` 已有活跃作业时返回 `StoreError::Busy`。
    pub fn insert_job(&self, j: &JobRow) -> Result<()> {
        let conn = self.conn();
        let res = conn.execute(
            "INSERT INTO jobs(id, kind, target_type, target_id, title, params_json, phase,
                              phase_label, progress, status, cancellable, cancel_requested,
                              created_by, created_by_role, requester_ip, created_at,
                              started_at, finished_at, duration_ms, error_class, error_detail,
                              result_json, log_bytes, log_seq, retry_of, exit_code)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                    ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
            params![
                j.id,
                j.kind,
                j.target_type,
                j.target_id,
                j.title,
                j.params_json,
                j.phase,
                j.phase_label,
                j.progress,
                j.status,
                j.cancellable as i64,
                j.cancel_requested as i64,
                j.created_by,
                j.created_by_role,
                j.requester_ip,
                j.created_at,
                j.started_at,
                j.finished_at,
                j.duration_ms,
                j.error_class,
                j.error_detail,
                j.result_json,
                j.log_bytes,
                j.log_seq,
                j.retry_of,
                j.exit_code,
            ],
        );
        let unique = matches!(&res, Err(err) if is_unique_violation(err));
        // 释放连接锁后再查活跃作业，Mutex 不可重入。
        drop(conn);
        match res {
            Ok(_) => Ok(()),
            Err(_) if unique => {
                let active = self
                    .active_job_for_target(&j.target_type, &j.target_id)
                    .ok()
                    .flatten();
                Err(StoreError::Busy(match active {
                    Some(a) => format!("{}（{}）", a.id, a.phase_label),
                    None => "该目标已有活跃作业".to_string(),
                }))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// 阶段推进：首次推进时补 `started_at`，并把状态置为 `running`。
    pub fn update_job_phase(
        &self,
        id: &str,
        phase: &str,
        phase_label: &str,
        progress: i64,
        cancellable: bool,
        now: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE jobs
                SET phase = ?2, phase_label = ?3, progress = ?4, cancellable = ?5,
                    status = 'running',
                    started_at = COALESCE(started_at, ?6)
              WHERE id = ?1 AND status IN ('queued','running')",
            params![id, phase, phase_label, progress, cancellable as i64, now],
        )?;
        Ok(())
    }

    /// 记录退出码：只写 `exit_code`，不改状态/阶段（由收尾逻辑决定成败）。
    pub fn set_job_exit_code(&self, id: &str, code: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE jobs SET exit_code = ?2 WHERE id = ?1",
            params![id, code],
        )?;
        Ok(())
    }

    /// 追加日志（事务内：写行 + 更新计数器 + 超限轮转）。
    pub fn append_job_logs(
        &self,
        job_id: &str,
        at: i64,
        entries: &[(String, String)],
    ) -> Result<LogAppend> {
        if entries.is_empty() {
            return Ok(LogAppend {
                last_seq: 0,
                truncated_before: None,
            });
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let (mut seq, mut bytes): (i64, i64) = tx.query_row(
            "SELECT log_seq, log_bytes FROM jobs WHERE id = ?1",
            params![job_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        for (stream, line) in entries {
            seq += 1;
            tx.execute(
                "INSERT INTO job_logs(job_id, seq, at, stream, line) VALUES(?1, ?2, ?3, ?4, ?5)",
                params![job_id, seq, at, stream, line],
            )?;
            bytes += (line.len() as i64) + (stream.len() as i64) + 24;
        }
        tx.execute(
            "UPDATE jobs SET log_seq = ?2, log_bytes = ?3 WHERE id = ?1",
            params![job_id, seq, bytes],
        )?;

        // 超限时从最旧删掉 25%，最多迭代 8 次，避免极端输入把事务拖长。
        let mut truncated_before = None;
        for _ in 0..8 {
            let (rows, sum_bytes): (i64, i64) = tx.query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(line) + LENGTH(stream) + 24), 0)
                   FROM job_logs WHERE job_id = ?1",
                params![job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if rows <= MAX_LOG_LINES && sum_bytes <= MAX_LOG_BYTES {
                bytes = sum_bytes;
                break;
            }
            let drop_n = (rows / 4).max(1);
            let cutoff: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM job_logs WHERE job_id = ?1 ORDER BY seq LIMIT 1 OFFSET ?2",
                    params![job_id, drop_n - 1],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(cutoff) = cutoff else { break };
            tx.execute(
                "DELETE FROM job_logs WHERE job_id = ?1 AND seq <= ?2",
                params![job_id, cutoff],
            )?;
            truncated_before = Some(cutoff + 1);
            let sum_bytes: i64 = tx.query_row(
                "SELECT COALESCE(SUM(LENGTH(line) + LENGTH(stream) + 24), 0)
                   FROM job_logs WHERE job_id = ?1",
                params![job_id],
                |r| r.get(0),
            )?;
            bytes = sum_bytes;
        }
        if truncated_before.is_none() {
            truncated_before = tx
                .query_row(
                    "SELECT MIN(seq) FROM job_logs WHERE job_id = ?1",
                    params![job_id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten();
        }
        tx.execute(
            "UPDATE jobs SET log_bytes = ?2 WHERE id = ?1",
            params![job_id, bytes],
        )?;
        tx.commit()?;
        Ok(LogAppend {
            last_seq: seq,
            truncated_before,
        })
    }

    /// 结算作业（幂等：已是终态时不覆盖）。
    #[allow(clippy::too_many_arguments)]
    pub fn finish_job(
        &self,
        id: &str,
        status: &str,
        error_class: Option<&str>,
        error_detail: Option<&str>,
        result_json: &str,
        finished_at: i64,
        duration_ms: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE jobs
                SET status = ?2, error_class = ?3, error_detail = ?4, result_json = ?5,
                    finished_at = ?6, duration_ms = ?7, cancellable = 0,
                    phase = CASE WHEN ?2 = 'succeeded' THEN 'done' ELSE phase END,
                    phase_label = CASE WHEN ?2 = 'succeeded' THEN '完成'
                                       WHEN ?2 = 'cancelled' THEN '已取消'
                                       WHEN ?2 = 'failed' THEN '失败'
                                       ELSE phase_label END,
                    progress = CASE WHEN ?2 = 'succeeded' THEN 100 ELSE progress END
              WHERE id = ?1 AND status IN ('queued','running')",
            params![
                id,
                status,
                error_class,
                error_detail,
                result_json,
                finished_at,
                duration_ms
            ],
        )?;
        Ok(())
    }

    /// 请求取消：只对活跃作业生效，返回是否真的写入了标记。
    pub fn request_job_cancel(&self, id: &str) -> Result<bool> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE jobs SET cancel_requested = 1
              WHERE id = ?1 AND status IN ('queued','running')",
            params![id],
        )?;
        Ok(n > 0)
    }

    pub fn job(&self, id: &str) -> Result<Option<JobRow>> {
        let conn = self.conn();
        let sql = format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1");
        Ok(conn.query_row(&sql, params![id], row_from).optional()?)
    }

    /// 某目标最新一条作业（活跃优先，其次按创建时间倒序）。
    pub fn latest_job_for_target(
        &self,
        target_type: &str,
        target_id: &str,
    ) -> Result<Option<JobRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {JOB_COLUMNS} FROM jobs
              WHERE target_type = ?1 AND target_id = ?2
              ORDER BY (status IN ('queued','running')) DESC, created_at DESC
              LIMIT 1"
        );
        Ok(conn
            .query_row(&sql, params![target_type, target_id], row_from)
            .optional()?)
    }

    pub fn active_job_for_target(
        &self,
        target_type: &str,
        target_id: &str,
    ) -> Result<Option<JobRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {JOB_COLUMNS} FROM jobs
              WHERE target_type = ?1 AND target_id = ?2 AND status IN ('queued','running')
              LIMIT 1"
        );
        Ok(conn
            .query_row(&sql, params![target_type, target_id], row_from)
            .optional()?)
    }

    pub fn list_jobs(&self, limit: i64) -> Result<Vec<JobRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {JOB_COLUMNS} FROM jobs
              ORDER BY (status IN ('queued','running')) DESC, created_at DESC
              LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit.max(1)], row_from)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_jobs_for_target_type(&self, target_type: &str, limit: i64) -> Result<Vec<JobRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE target_type = ?1
              ORDER BY created_at DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![target_type, limit.max(1)], row_from)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 增量读取日志（`seq > after`），附带现存边界供前端判断截断。
    pub fn job_logs_since(&self, job_id: &str, after: i64, limit: i64) -> Result<JobLogBatch> {
        let conn = self.conn();
        let (min_seq, max_seq, total_bytes): (Option<i64>, i64, i64) = conn.query_row(
            "SELECT MIN(seq), COALESCE(MAX(seq), 0), COALESCE(SUM(LENGTH(line) + LENGTH(stream) + 24), 0)
               FROM job_logs WHERE job_id = ?1",
            params![job_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mut stmt = conn.prepare(
            "SELECT job_id, seq, at, stream, line FROM job_logs
              WHERE job_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let entries = stmt
            .query_map(params![job_id, after.max(0), limit.max(1)], |r| {
                Ok(JobLogRow {
                    job_id: r.get(0)?,
                    seq: r.get(1)?,
                    at: r.get(2)?,
                    stream: r.get(3)?,
                    line: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(JobLogBatch {
            min_seq,
            max_seq,
            total_bytes,
            entries,
        })
    }

    /// 日志尾部（按 seq 升序返回最后 `limit` 行）。
    pub fn job_log_tail(&self, job_id: &str, limit: i64) -> Result<Vec<JobLogRow>> {
        let conn = self.conn();
        let max_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM job_logs WHERE job_id = ?1",
            params![job_id],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT job_id, seq, at, stream, line FROM job_logs
              WHERE job_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![job_id, (max_seq - limit.max(1)).max(0)], |r| {
            Ok(JobLogRow {
                job_id: r.get(0)?,
                seq: r.get(1)?,
                at: r.get(2)?,
                stream: r.get(3)?,
                line: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 分批清理终态作业：先删日志再删主行，单批 200 条，最多 10 批。
    ///
    /// 返回删除的作业条数。调用方在「提交时 + 启动时」各跑一次即可。
    pub fn prune_jobs(&self, cutoff_ts: i64, keep_max: i64) -> Result<usize> {
        let mut total = 0usize;
        for _ in 0..10 {
            let mut conn = self.conn();
            let ids: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT id FROM jobs
                      WHERE status NOT IN ('queued','running')
                        AND (COALESCE(finished_at, created_at) < ?1
                             OR id NOT IN (SELECT id FROM jobs ORDER BY created_at DESC LIMIT ?2))
                      LIMIT 200",
                )?;
                let rows = stmt.query_map(params![cutoff_ts, keep_max.max(1)], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<String>>>()?
            };
            if ids.is_empty() {
                break;
            }
            let tx = conn.transaction()?;
            for id in &ids {
                tx.execute("DELETE FROM job_logs WHERE job_id = ?1", params![id])?;
                tx.execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
            }
            tx.commit()?;
            total += ids.len();
            if ids.len() < 200 {
                break;
            }
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, target: &str, status: &str) -> JobRow {
        JobRow {
            id: id.into(),
            kind: "runtime.ext.install".into(),
            target_type: "runtime".into(),
            target_id: target.into(),
            title: "安装 PHP 扩展 redis".into(),
            params_json: "{}".into(),
            phase: "queued".into(),
            phase_label: "排队".into(),
            progress: 0,
            status: status.into(),
            cancellable: true,
            cancel_requested: false,
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            created_at: 1_700_000_000,
            started_at: None,
            finished_at: None,
            duration_ms: 0,
            error_class: None,
            error_detail: None,
            result_json: "{}".into(),
            log_bytes: 0,
            log_seq: 0,
            retry_of: None,
            exit_code: None,
        }
    }

    #[test]
    fn active_target_is_unique_until_terminal() {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(&sample("j1", "rt-1", STATUS_RUNNING))
            .unwrap();
        let err = db
            .insert_job(&sample("j2", "rt-1", STATUS_QUEUED))
            .unwrap_err();
        assert!(matches!(err, StoreError::Busy(_)));

        db.finish_job("j1", STATUS_SUCCEEDED, None, None, "{}", 1_700_000_100, 100)
            .unwrap();
        db.insert_job(&sample("j3", "rt-1", STATUS_QUEUED)).unwrap();
    }

    #[test]
    fn log_rotation_keeps_cursor_monotonic_and_reports_truncation() {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(&sample("j1", "rt-1", STATUS_RUNNING))
            .unwrap();
        let line = ("x".repeat(200), "out".to_string());
        let mut last = 0;
        for _ in 0..(MAX_LOG_LINES / 100 + 30) {
            let batch: Vec<(String, String)> = (0..100).map(|_| line.clone()).collect();
            last = db
                .append_job_logs("j1", 1_700_000_000, &batch)
                .unwrap()
                .last_seq;
        }
        assert_eq!(last, MAX_LOG_LINES + 3_000);
        let batch = db.job_logs_since("j1", 0, 10_000).unwrap();
        // 轮转只删最旧：游标不重编号，现存最小 seq 明显大于 1。
        assert!(batch.min_seq.unwrap() > 1);
        assert!(batch.max_seq == last);
        assert!(batch.entries.len() as i64 <= MAX_LOG_LINES);
        let bounds = db.job_logs_since("j1", last, 10).unwrap();
        assert!(bounds.entries.is_empty());
    }

    #[test]
    fn exit_code_round_trips_and_stays_null_when_unknown() {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(&sample("j1", "rt-1", STATUS_SUCCEEDED))
            .unwrap();
        // 未记录退出码：保持 NULL，由展示层显示 `-`。
        assert_eq!(db.job("j1").unwrap().unwrap().exit_code, None);
        db.set_job_exit_code("j1", 127).unwrap();
        assert_eq!(db.job("j1").unwrap().unwrap().exit_code, Some(127));
        // 只改退出码，不动状态。
        assert_eq!(db.job("j1").unwrap().unwrap().status, STATUS_SUCCEEDED);
    }

    #[test]
    fn prune_removes_old_finished_jobs_and_logs() {
        let db = Db::open_in_memory().unwrap();
        let mut old = sample("old", "rt-1", STATUS_SUCCEEDED);
        old.finished_at = Some(1_000);
        db.insert_job(&old).unwrap();
        db.append_job_logs("old", 1_000, &[("note".into(), "hi".into())])
            .unwrap();
        db.prune_jobs(2_000, 200).unwrap();
        assert!(db.job("old").unwrap().is_none());
        let left: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM job_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
    }
}
