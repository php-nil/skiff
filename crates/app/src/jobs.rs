//! 任务中心：一次性异步作业的领域服务。
//!
//! 与 `tasks`（周期调度）刻意分开：这里承载「提交一次、长耗时、可看过程日志」的作业，
//! 事实源是 `store::jobs` 的两张表。`JobService` 只做四件事：
//!
//! 1. **提交**：同目标活跃作业由部分唯一索引原子拒绝；
//! 2. **推进**：阶段/进度/可取消标记在真实执行边界更新，不用定时器造假进度；
//! 3. **日志**：逐行追加，超限由存储层从最旧轮转，`seq` 不重编号；
//! 4. **结算**：成功 / 失败 / 取消三态，取消是协作式的（见 `cancel`）。
//!
//! 取消语义：能打断的调用在执行体里检查 `cancel_requested`；`docker exec` 这种
//! 无法中断的步骤会把 `cancellable` 置 0 并在 UI 上如实标注，另配硬超时兜底。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use skiff_store::jobs::{
    JobLogBatch, JobRow, JOB_KEEP_MAX, JOB_RETENTION_DAYS, STATUS_CANCELLED, STATUS_FAILED,
    STATUS_QUEUED, STATUS_RUNNING, STATUS_SUCCEEDED,
};
use skiff_store::{Db, StoreError};

/// 作业目标类型：与前端筛选项、`jobs.target_type` 同一口径。
pub const TARGET_RUNTIME: &str = "runtime";
pub const TARGET_CONTAINER: &str = "container";
pub const TARGET_IMAGE: &str = "image";
pub const TARGET_COMPOSE: &str = "compose";
pub const TARGET_SITE: &str = "site";
pub const TARGET_SYSTEM: &str = "system";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// 同目标已有活跃作业；消息里带 job id 与当前阶段，便于直接提示用户。
    Busy(String),
    NotFound(String),
    BadRequest(String),
    Store(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(m) | Self::NotFound(m) | Self::BadRequest(m) | Self::Store(m) => {
                f.write_str(m)
            }
        }
    }
}

impl From<StoreError> for JobError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Busy(m) => Self::Busy(m),
            StoreError::NotFound(m) => Self::NotFound(m),
            other => Self::Store(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            STATUS_QUEUED => Self::Queued,
            STATUS_RUNNING => Self::Running,
            STATUS_SUCCEEDED => Self::Succeeded,
            STATUS_FAILED => Self::Failed,
            STATUS_CANCELLED => Self::Cancelled,
            _ => Self::Failed,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => STATUS_QUEUED,
            Self::Running => STATUS_RUNNING,
            Self::Succeeded => STATUS_SUCCEEDED,
            Self::Failed => STATUS_FAILED,
            Self::Cancelled => STATUS_CANCELLED,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Running => "执行中",
            Self::Succeeded => "已完成",
            Self::Failed => "失败",
            Self::Cancelled => "已取消",
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// 一条作业的领域视图。
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub target_type: String,
    pub target_id: String,
    pub title: String,
    pub params: Value,
    pub phase: String,
    pub phase_label: String,
    pub progress: i64,
    pub status: JobStatus,
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
    pub result: Value,
    pub log_bytes: i64,
    pub log_seq: i64,
    pub retry_of: Option<String>,
}

impl Job {
    pub fn active(&self) -> bool {
        matches!(self.status, JobStatus::Queued | JobStatus::Running)
    }

    /// 参数里的扩展 token（安装/卸载任务用于 UI 高亮）。
    pub fn ext_keys(&self) -> Vec<String> {
        self.params
            .get("ext_keys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn elapsed_secs(&self, now: i64) -> i64 {
        match (self.started_at, self.finished_at) {
            (Some(s), Some(f)) => (f - s).max(0),
            (Some(s), None) => (now - s).max(0),
            _ => 0,
        }
    }

    fn from_row(r: JobRow) -> Self {
        Self {
            id: r.id,
            kind: r.kind,
            target_type: r.target_type,
            target_id: r.target_id,
            title: r.title,
            params: serde_json::from_str(&r.params_json).unwrap_or_else(|_| json!({})),
            phase: r.phase,
            phase_label: r.phase_label,
            progress: r.progress,
            status: JobStatus::parse(&r.status),
            cancellable: r.cancellable,
            cancel_requested: r.cancel_requested,
            created_by: r.created_by,
            created_by_role: r.created_by_role,
            requester_ip: r.requester_ip,
            created_at: r.created_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
            duration_ms: r.duration_ms,
            error_class: r.error_class,
            error_detail: r.error_detail,
            result: serde_json::from_str(&r.result_json).unwrap_or_else(|_| json!({})),
            log_bytes: r.log_bytes,
            log_seq: r.log_seq,
            retry_of: r.retry_of,
        }
    }
}

/// 提交参数。
#[derive(Debug, Clone)]
pub struct JobSubmit {
    pub kind: String,
    pub target_type: String,
    pub target_id: String,
    pub title: String,
    pub params: Value,
    pub created_by: String,
    pub created_by_role: String,
    pub requester_ip: String,
    /// 初始是否可取消；执行体会在进入不可中断步骤时改回 false。
    pub cancellable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// 尚未开始，直接结算为已取消。
    CancelledNow,
    /// 已记录取消请求，执行体在下一个边界停止。
    Requested,
    /// 已是终态，无需处理。
    Terminal,
}

/// 作业服务句柄（`AppState` 持有）。内部只缓存「目标 → 活跃作业 id」的路由，
/// 事实源始终是数据库；重启后缓存为空会自动回落查询。
pub struct JobService {
    db: Arc<Db>,
    route: Mutex<HashMap<(String, String), String>>,
    seq: AtomicU64,
}

impl JobService {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            route: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn new_id(&self) -> String {
        let ts = Self::now();
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        format!("job-{ts}-{nanos:x}{n:x}")
    }

    /// 提交一条作业。同目标已有活跃作业时返回 `JobError::Busy`（带可行动信息）。
    ///
    /// 提交前顺带做一次分批清理；失败不影响提交本身。
    pub fn submit(&self, s: JobSubmit) -> Result<Job, JobError> {
        self.prune_quietly();
        let id = self.new_id();
        let now = Self::now();
        let row = JobRow {
            id: id.clone(),
            kind: s.kind,
            target_type: s.target_type.clone(),
            target_id: s.target_id.clone(),
            title: s.title,
            params_json: s.params.to_string(),
            phase: "queued".into(),
            phase_label: "排队".into(),
            progress: 0,
            status: STATUS_QUEUED.into(),
            cancellable: s.cancellable,
            cancel_requested: false,
            created_by: s.created_by,
            created_by_role: s.created_by_role,
            requester_ip: s.requester_ip,
            created_at: now,
            started_at: None,
            finished_at: None,
            duration_ms: 0,
            error_class: None,
            error_detail: None,
            result_json: "{}".into(),
            log_bytes: 0,
            log_seq: 0,
            retry_of: None,
        };
        self.db.insert_job(&row)?;
        self.cache_route(&s.target_type, &s.target_id, &id);
        let job = self
            .db
            .job(&id)?
            .map(Job::from_row)
            .ok_or_else(|| JobError::Store("作业写入后读回失败".into()))?;
        self.note(&id, "任务已创建，等待执行");
        Ok(job)
    }

    /// 重试：**新建一条作业**，`retry_of` 指向原作业，参数原样重放。
    pub fn retry(
        &self,
        id: &str,
        created_by: &str,
        created_by_role: &str,
        requester_ip: &str,
    ) -> Result<Job, JobError> {
        let old = self
            .db
            .job(id)?
            .map(Job::from_row)
            .ok_or_else(|| JobError::NotFound(format!("作业不存在：{id}")))?;
        if old.active() {
            return Err(JobError::Busy(format!(
                "作业 {} 仍在执行中（{}），无需重试",
                old.id, old.phase_label
            )));
        }
        self.prune_quietly();
        let new_id = self.new_id();
        let now = Self::now();
        let row = JobRow {
            id: new_id.clone(),
            kind: old.kind,
            target_type: old.target_type.clone(),
            target_id: old.target_id.clone(),
            title: old.title,
            params_json: old.params.to_string(),
            phase: "queued".into(),
            phase_label: "排队".into(),
            progress: 0,
            status: STATUS_QUEUED.into(),
            cancellable: true,
            cancel_requested: false,
            created_by: created_by.to_string(),
            created_by_role: created_by_role.to_string(),
            requester_ip: requester_ip.to_string(),
            created_at: now,
            started_at: None,
            finished_at: None,
            duration_ms: 0,
            error_class: None,
            error_detail: None,
            result_json: "{}".into(),
            log_bytes: 0,
            log_seq: 0,
            retry_of: Some(old.id.clone()),
        };
        self.db.insert_job(&row)?;
        self.cache_route(&old.target_type, &old.target_id, &new_id);
        self.note(&new_id, &format!("重试作业，来源 {}", old.id));
        self.db
            .job(&new_id)?
            .map(Job::from_row)
            .ok_or_else(|| JobError::Store("作业写入后读回失败".into()))
    }

    /// 阶段推进。真实执行步骤调用；`cancellable=false` 表示当前步骤不可中断。
    pub fn set_phase(&self, id: &str, phase: &str, label: &str, progress: i64, cancellable: bool) {
        if let Err(e) = self.db.update_job_phase(
            id,
            phase,
            label,
            progress.clamp(-1, 100),
            cancellable,
            Self::now(),
        ) {
            tracing::warn!(job = id, error = %e, "作业阶段写入失败");
        }
        self.cache_route_by_id(id);
    }

    /// 当前阶段是否可中断（进入/退出 `docker exec` 等不可中断步骤时切换）。
    pub fn set_cancellable(&self, id: &str, cancellable: bool) {
        if let Ok(Some(job)) = self.db.job(id) {
            if let Err(e) = self.db.update_job_phase(
                id,
                &job.phase,
                &job.phase_label,
                job.progress,
                cancellable,
                Self::now(),
            ) {
                tracing::warn!(job = id, error = %e, "作业可中断标记写入失败");
            }
        }
    }

    /// 追加一行日志。
    pub fn log(&self, id: &str, stream: &str, line: &str) {
        if let Err(e) =
            self.db
                .append_job_logs(id, Self::now(), &[(stream.to_string(), line.to_string())])
        {
            tracing::warn!(job = id, error = %e, "作业日志写入失败");
        }
    }

    pub fn note(&self, id: &str, line: &str) {
        self.log(id, "note", line);
    }

    /// 批量追加多行（命令输出的尾巴），一次事务落盘。
    pub fn log_lines(&self, id: &str, stream: &str, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let entries: Vec<(String, String)> = lines
            .iter()
            .map(|l| (stream.to_string(), l.clone()))
            .collect();
        if let Err(e) = self.db.append_job_logs(id, Self::now(), &entries) {
            tracing::warn!(job = id, error = %e, "作业日志批量写入失败");
        }
    }

    pub fn finish_ok(&self, id: &str, line: &str, result: Value) {
        self.note(id, line);
        let now = Self::now();
        let duration = self.duration_ms(id, now);
        if let Err(e) = self.db.finish_job(
            id,
            STATUS_SUCCEEDED,
            None,
            None,
            &result.to_string(),
            now,
            duration,
        ) {
            tracing::warn!(job = id, error = %e, "作业成功收尾写入失败");
        }
        self.forget_route(id);
    }

    pub fn finish_err(&self, id: &str, class: &str, detail: &str) {
        self.note(id, &format!("失败：{detail}"));
        let now = Self::now();
        let duration = self.duration_ms(id, now);
        if let Err(e) = self.db.finish_job(
            id,
            STATUS_FAILED,
            Some(class),
            Some(detail),
            "{}",
            now,
            duration,
        ) {
            tracing::warn!(job = id, error = %e, "作业失败收尾写入失败");
        }
        self.forget_route(id);
    }

    pub fn finish_cancelled(&self, id: &str, detail: &str) {
        self.note(id, detail);
        let now = Self::now();
        let duration = self.duration_ms(id, now);
        if let Err(e) = self.db.finish_job(
            id,
            STATUS_CANCELLED,
            Some("cancelled"),
            Some(detail),
            "{}",
            now,
            duration,
        ) {
            tracing::warn!(job = id, error = %e, "作业取消收尾写入失败");
        }
        self.forget_route(id);
    }

    /// 请求取消。排队中的作业立即结算；执行中的只置标记，由执行体在边界停止。
    pub fn cancel(&self, id: &str) -> Result<CancelOutcome, JobError> {
        let job = self
            .db
            .job(id)?
            .map(Job::from_row)
            .ok_or_else(|| JobError::NotFound(format!("作业不存在：{id}")))?;
        if job.status.terminal() {
            return Ok(CancelOutcome::Terminal);
        }
        if job.status == JobStatus::Queued {
            self.note(id, "已取消（尚未开始执行）");
            self.finish_cancelled(id, "已取消（尚未开始执行）");
            return Ok(CancelOutcome::CancelledNow);
        }
        self.db.request_job_cancel(id)?;
        self.note(
            id,
            if job.cancellable {
                "已请求取消，将在当前步骤结束后停止"
            } else {
                "已请求取消；当前步骤不可中断，将在该步骤结束后停止"
            },
        );
        Ok(CancelOutcome::Requested)
    }

    /// 执行体轮询取消标记（每个阶段边界一次）。
    pub fn cancel_requested(&self, id: &str) -> bool {
        self.db
            .job(id)
            .ok()
            .flatten()
            .map(|j| j.cancel_requested)
            .unwrap_or(false)
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.db.job(id).ok().flatten().map(Job::from_row)
    }

    pub fn latest_for_target(&self, target_type: &str, target_id: &str) -> Option<Job> {
        self.db
            .latest_job_for_target(target_type, target_id)
            .ok()
            .flatten()
            .map(Job::from_row)
    }

    pub fn list(&self, limit: i64) -> Vec<Job> {
        self.db
            .list_jobs(limit)
            .unwrap_or_default()
            .into_iter()
            .map(Job::from_row)
            .collect()
    }

    pub fn list_for_target_type(&self, target_type: &str, limit: i64) -> Vec<Job> {
        self.db
            .list_jobs_for_target_type(target_type, limit)
            .unwrap_or_default()
            .into_iter()
            .map(Job::from_row)
            .collect()
    }

    /// 增量日志：`after` 为已读到的最大 seq；返回批次里带现存最小 seq，
    /// 起点已被轮转时调用方据此提示「更早日志已截断」。
    pub fn logs_since(&self, id: &str, after: i64, limit: i64) -> Option<JobLogBatch> {
        self.db.job_logs_since(id, after, limit).ok()
    }

    /// 首屏日志尾巴（按 `seq` 升序）：详情页直接渲染，后续增量交给 SSE。
    pub fn tail(&self, id: &str, limit: i64) -> Vec<skiff_store::jobs::JobLogRow> {
        self.db.job_log_tail(id, limit).unwrap_or_default()
    }

    /// 目标当前活跃作业 id（内存路由优先；缓存失效时回落数据库）。
    pub fn active_job_id(&self, target_type: &str, target_id: &str) -> Option<String> {
        let key = (target_type.to_string(), target_id.to_string());
        if let Some(id) = self
            .route
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            if self.get(&id).is_some_and(|j| j.active()) {
                return Some(id);
            }
        }
        let active = self
            .db
            .active_job_for_target(target_type, target_id)
            .ok()
            .flatten()?;
        self.cache_route(target_type, target_id, &active.id);
        Some(active.id)
    }

    /// 清理终态作业（提交时与启动时各一次），失败静默。
    pub fn prune_quietly(&self) {
        let cutoff = Self::now() - JOB_RETENTION_DAYS * 86_400;
        if let Err(e) = self.db.prune_jobs(cutoff, JOB_KEEP_MAX) {
            tracing::debug!(error = %e, "清理过期作业失败");
        }
    }

    fn duration_ms(&self, id: &str, now: i64) -> i64 {
        let started = self
            .db
            .job(id)
            .ok()
            .flatten()
            .and_then(|j| j.started_at)
            .unwrap_or(now);
        (now - started).max(0) * 1000
    }

    fn cache_route(&self, target_type: &str, target_id: &str, job_id: &str) {
        self.route.lock().unwrap_or_else(|e| e.into_inner()).insert(
            (target_type.to_string(), target_id.to_string()),
            job_id.to_string(),
        );
    }

    /// 阶段推进没有携带目标信息：按 id 找到目标再刷新路由缓存。
    fn cache_route_by_id(&self, job_id: &str) {
        if let Ok(Some(j)) = self.db.job(job_id) {
            self.cache_route(&j.target_type, &j.target_id, job_id);
        }
    }

    fn forget_route(&self, job_id: &str) {
        let mut route = self.route.lock().unwrap_or_else(|e| e.into_inner());
        route.retain(|_, v| v != job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> JobService {
        JobService::new(Arc::new(Db::open_in_memory().unwrap()))
    }

    fn submit(s: &JobService, target: &str) -> Job {
        s.submit(JobSubmit {
            kind: "runtime.ext.install".into(),
            target_type: TARGET_RUNTIME.into(),
            target_id: target.into(),
            title: "安装 PHP 扩展 redis".into(),
            params: json!({"ext_keys": ["redis"]}),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap()
    }

    #[test]
    fn same_target_rejects_second_active_job_with_actionable_message() {
        let s = svc();
        let first = submit(&s, "rt-1");
        s.set_phase(&first.id, "extensions", "安装扩展", 65, false);
        let err = s
            .submit(JobSubmit {
                kind: "runtime.ext.install".into(),
                target_type: TARGET_RUNTIME.into(),
                target_id: "rt-1".into(),
                title: "安装 PHP 扩展 swoole".into(),
                params: json!({}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: "127.0.0.1".into(),
                cancellable: true,
            })
            .unwrap_err();
        match err {
            JobError::Busy(m) => {
                assert!(m.contains(&first.id));
                assert!(m.contains("安装扩展"));
            }
            other => panic!("expected busy, got {other:?}"),
        }
    }

    #[test]
    fn cancel_queued_job_settles_immediately() {
        let s = svc();
        let job = submit(&s, "rt-1");
        assert_eq!(s.cancel(&job.id).unwrap(), CancelOutcome::CancelledNow);
        let got = s.get(&job.id).unwrap();
        assert_eq!(got.status, JobStatus::Cancelled);
        assert!(s.active_job_id(TARGET_RUNTIME, "rt-1").is_none());
    }

    #[test]
    fn cancel_running_job_only_sets_flag_until_worker_stops() {
        let s = svc();
        let job = submit(&s, "rt-1");
        s.set_phase(&job.id, "extensions", "安装扩展", 65, false);
        assert_eq!(s.cancel(&job.id).unwrap(), CancelOutcome::Requested);
        assert!(s.cancel_requested(&job.id));
        assert_eq!(s.get(&job.id).unwrap().status, JobStatus::Running);
        s.finish_cancelled(&job.id, "已取消");
        assert_eq!(s.get(&job.id).unwrap().status, JobStatus::Cancelled);
    }

    #[test]
    fn retry_creates_new_job_and_links_back() {
        let s = svc();
        let job = submit(&s, "rt-1");
        s.finish_err(&job.id, "script", "安装脚本退出码 1");
        let retry = s
            .retry(&job.id, "operator", "operator", "10.0.0.2")
            .unwrap();
        assert_ne!(retry.id, job.id);
        assert_eq!(retry.retry_of.as_deref(), Some(job.id.as_str()));
        assert_eq!(retry.status, JobStatus::Queued);
        assert_eq!(retry.created_by, "operator");
        assert_eq!(retry.ext_keys(), vec!["redis".to_string()]);
    }

    #[test]
    fn logs_are_incremental_and_seq_never_rewinds() {
        let s = svc();
        let job = submit(&s, "rt-1");
        s.note(&job.id, "first");
        s.note(&job.id, "second");
        let all = s.logs_since(&job.id, 0, 100).unwrap();
        let seqs: Vec<i64> = all.entries.iter().map(|e| e.seq).collect();
        // 提交时已写入一条「任务已创建」，seq 从 1 开始单调递增。
        assert!(seqs.len() >= 3);
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
        // after 是「已读到的最大 seq」：取倒数第二条，应只补最后一条。
        let last = *seqs.last().unwrap();
        let one = s.logs_since(&job.id, last - 1, 100).unwrap();
        assert_eq!(one.entries.len(), 1);
        assert_eq!(one.entries[0].seq, last);
        assert!(s.logs_since(&job.id, last, 100).unwrap().entries.is_empty());
    }
}
