//! 内核定时任务的接线层：SQLite 仓储适配、1s tick 驱动、内置执行体与插件门。
//!
//! 调度语义（tick / 单飞 / 超时 / 缓冲 / 回执）全部在 `skiff-sched`；这里只做三件事：
//!
//! 1. **仓储适配**：把 `Task` / `RunRecord` / `AuditEvent` 映射到 `skiff-store`
//!    的 `scheduled_tasks` / `task_runs` / `audit`，不在存储层解释调度规则；
//! 2. **执行装配**：tick 只出票，执行体放到独立 tokio 任务，晚到结果按 `run_id`
//!    丢弃（超时已结算后 `finish` 会拒绝重复结算）；
//! 3. **能力面**：`scheduler` 插件能力（`task.*`）与站点备份开关的唯一实现。
//!
//! 放在 `skiff-app` 而不是 `crates/core`：`crates/core` 是可执行 crate（无 lib），
//! 而 Web 与插件 UDS 都要通过 `AppState` 取到任务服务；`app` 是唯一同时被两者
//! 依赖的装配层，且不会让 `skiff-store` 反向依赖 `skiff-sched`。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use skiff_plugin::supervisor::{Supervisor, SupervisorError};
use skiff_sched::{
    AuditActor, AuditEvent, AuditOutcome, Batch, BufferSignal, Clock, ErrorClass, ErrorPhase,
    ExecError, ExecOutput, PluginGate, PluginStatus, PushOutcome, PushReport, RejectReason,
    RunContext, RunRecord, RunTicket, SampleTicket, Schedule, Scheduler, SchedulerConfig,
    SystemClock, Task, TaskError, TaskFilter, TaskKind, TaskPatch, TaskSource, TaskSpec, TaskState,
    TaskStore, Trigger,
};
use skiff_store::{Db, Grant, PluginRecord, ScheduledTaskRow, TaskRunRow};
use tokio::sync::Mutex;

/// 内核插件 API 版本（manifest `api` 不匹配即视为不可用）。
pub const KERNEL_API: &str = "1.2";
/// 内置任务名：站点备份。
pub const BUILTIN_SITE_BACKUP: &str = "site-backup";
/// 内置任务名：数据库整理。
pub const BUILTIN_DB_VACUUM: &str = "db-vacuum";
/// 内置任务 id：数据库整理（全局唯一）。
pub const DB_VACUUM_TASK_ID: &str = "builtin:db-vacuum";

/// 站点备份任务 id。内置任务以 `(kind, source)` 唯一，id 里带域名只为可读性。
pub fn site_backup_task_id(domain: &str) -> String {
    format!("builtin:{BUILTIN_SITE_BACKUP}:{domain}")
}

/// 站点备份开关的只读状态，供页面渲染；不暴露 `skiff-sched` 类型给 Web。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteBackupStatus {
    pub enabled: bool,
    pub task_id: String,
    pub last_at: Option<i64>,
    pub last_result: Option<String>,
    pub next_at: Option<i64>,
}

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
        self.put_row(&task)
    }

    fn delete_task(&mut self, id: &str) -> Result<Option<Task>, TaskError> {
        let row = self
            .db
            .delete_scheduled_task(id)
            .map_err(|e| TaskError::Store(e.to_string()))?;
        Ok(row.as_ref().and_then(task_of))
    }

    fn append_run(&mut self, run: RunRecord) -> Result<(), TaskError> {
        self.db
            .append_task_run(&run_row(&run))
            .map_err(|e| TaskError::Store(e.to_string()))
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
        self.db
            .audit(
                &event.actor.as_string(),
                "sched",
                event.action_str(),
                &event.target,
                event.outcome.as_str(),
                event.detail.as_deref().unwrap_or(""),
            )
            .map_err(|e| TaskError::Store(e.to_string()))
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

fn enum_str<T: serde::Serialize>(v: &T) -> String {
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

fn run_row(run: &RunRecord) -> TaskRunRow {
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

// ---------------------------------------------------------------------------
// 插件门
// ---------------------------------------------------------------------------

/// 基于 `PluginRecord` 的可用性判定：未安装 / 未启用 / API 不匹配都算不可用。
struct Gate {
    db: Arc<Db>,
}

impl PluginGate for Gate {
    fn plugin_status(&self, plugin_id: &str) -> PluginStatus {
        match self.db.plugin(plugin_id) {
            Ok(Some(p)) => {
                if !p.enabled {
                    PluginStatus::Disabled
                } else if p.api != KERNEL_API {
                    PluginStatus::ApiMismatch
                } else {
                    PluginStatus::Ready
                }
            }
            _ => PluginStatus::NotInstalled,
        }
    }
}

// ---------------------------------------------------------------------------
// 任务服务
// ---------------------------------------------------------------------------

pub struct TaskService {
    db: Arc<Db>,
    supervisor: Arc<Supervisor>,
    clock: SystemClock,
    scheduler: Mutex<Scheduler<SqliteTaskStore>>,
    /// 正在作为任务执行目标被调用的插件 id。std Mutex 而不是 tokio Mutex：
    /// 守卫要在 `Drop` 里清理，而 `Drop` 不能 await（超时/取消时也要摘除）。
    executing: std::sync::Mutex<std::collections::HashSet<String>>,
    /// `buffer.push` 的每任务限速窗口：`(秒序号, 该秒已收点数)`。
    push_window: std::sync::Mutex<std::collections::HashMap<String, (i64, u32)>>,
    web_root: PathBuf,
    data_dir: PathBuf,
}

/// 每任务 `buffer.push` 限速：100 点/秒（契约 §10.2）。
const PUSH_PER_SECOND: u32 = 100;

/// 插件执行忙碌标记的 RAII 守卫：无论正常返回、超时还是 future 被丢弃，
/// `Drop` 都会把它从集合里摘掉。
struct BusyGuard<'a> {
    svc: &'a TaskService,
    plugin: String,
}

impl<'a> BusyGuard<'a> {
    fn enter(svc: &'a TaskService, plugin: &str) -> Self {
        if let Ok(mut set) = svc.executing.lock() {
            set.insert(plugin.to_string());
        }
        Self {
            svc,
            plugin: plugin.to_string(),
        }
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.svc.executing.lock() {
            set.remove(&self.plugin);
        }
    }
}

impl TaskService {
    /// 打开任务服务：装载库内任务（运行态只存内存，重启天然回 `idle`），
    /// 补最后一次 `buffer.lost_on_restart`，并种下内置 `db-vacuum`。
    pub fn open(
        db: Arc<Db>,
        supervisor: Arc<Supervisor>,
        web_root: PathBuf,
        data_dir: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let clock = SystemClock;
        let mut scheduler = Scheduler::new(
            SqliteTaskStore::new(Arc::clone(&db)),
            SchedulerConfig::default(),
            &clock,
        )
        .map_err(|e| e.to_string())?;
        let _ = db.mark_tasks_loaded();
        // 在把 scheduler 交给 tokio Mutex 之前种内置任务：`open` 既可能在运行时内
        // 调用（bootstrap），也可能在运行时外（单测），不能依赖 `blocking_lock`。
        seed_builtins(&mut scheduler, &clock)?;

        Ok(Arc::new(Self {
            db,
            supervisor,
            clock,
            scheduler: Mutex::new(scheduler),
            executing: std::sync::Mutex::new(std::collections::HashSet::new()),
            push_window: std::sync::Mutex::new(std::collections::HashMap::new()),
            web_root,
            data_dir,
        }))
    }

    /// 插件是否正在作为任务执行目标。调度能力面据此拒绝重入。
    pub fn is_executing(&self, plugin_id: &str) -> bool {
        self.executing
            .lock()
            .map(|s| s.contains(plugin_id))
            .unwrap_or(false)
    }

    pub fn db_handle(&self) -> &Arc<Db> {
        &self.db
    }

    /// 1s tick：拿票 → 执行体独立 task → 结算。**不在锁内执行任何 RPC**。
    pub async fn tick_once(self: &Arc<Self>) {
        let mut runs = Vec::new();
        let mut samples = Vec::new();
        {
            let mut sched = self.scheduler.lock().await;
            let gate = Gate {
                db: Arc::clone(&self.db),
            };
            let report = sched.tick(&self.clock, &gate);
            for rec in report.records {
                let _ = self.persist_record(rec);
            }
            for ev in report.audits {
                let _ = self.persist_audit(ev);
            }
            for ticket in report.runs {
                let task_id = ticket.context.task_id.to_string();
                let timeout = sched
                    .get(&task_id)
                    .map(|t| t.spec.timeout_secs)
                    .unwrap_or(30);
                runs.push((ticket, timeout));
            }
            for ticket in report.samples {
                let task_id = ticket.context.task_id.to_string();
                let plugin = sched
                    .get(&task_id)
                    .and_then(|t| t.spec.plugin_id().map(str::to_string));
                samples.push((ticket, plugin));
            }
        }

        for (ticket, timeout_secs) in runs {
            let me = Arc::clone(self);
            tokio::spawn(async move { me.drive_run(ticket, timeout_secs).await });
        }
        for (ticket, plugin) in samples {
            let Some(plugin) = plugin else { continue };
            let me = Arc::clone(self);
            tokio::spawn(async move { me.drive_sample(ticket, plugin).await });
        }
    }

    /// 跑一张票：超时即结算 `timeout`；执行体即使晚到返回，`finish` 也会因
    /// `run_id` 已结算而拒绝写入（缓冲与历史都不会被二次消费）。
    pub async fn drive_run(self: Arc<Self>, ticket: RunTicket, timeout_secs: u64) {
        let run_id = ticket.context.run_id.clone();
        let outcome = tokio::time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            self.execute_ticket(&ticket),
        )
        .await;
        let mut sched = self.scheduler.lock().await;
        let now = self.clock.now();
        match outcome {
            Ok(result) => {
                if let Err(e) = sched.finish(&run_id, result, now) {
                    tracing::warn!(run_id, error = %e, "运行结算被拒绝（结果已过期或重复）");
                }
            }
            Err(_) => {
                if let Err(e) = sched.timeout(&run_id, now) {
                    tracing::warn!(run_id, error = %e, "超时结算失败");
                }
            }
        }
    }

    async fn drive_sample(self: Arc<Self>, ticket: SampleTicket, plugin: String) {
        let task_id = ticket.context.task_id.to_string();
        let method = ticket.context.method.clone();
        let result = self
            .call_plugin_raw(
                &plugin,
                &method,
                json!({ "task_id": task_id, "at": ticket.at }),
            )
            .await
            .map(point_of);
        let mut sched = self.scheduler.lock().await;
        if let Err(e) = sched.apply_sample(&ticket, result, self.clock.now()) {
            tracing::warn!(task = %ticket.context.task_id, error = %e, "采样结果写入失败");
        }
    }

    fn persist_record(&self, rec: RunRecord) -> Result<(), TaskError> {
        self.db
            .append_task_run(&run_row(&rec))
            .map_err(|e| TaskError::Store(e.to_string()))
    }

    fn persist_audit(&self, ev: AuditEvent) -> Result<(), TaskError> {
        self.db
            .audit(
                &ev.actor.as_string(),
                "sched",
                ev.action_str(),
                &ev.target,
                ev.outcome.as_str(),
                ev.detail.as_deref().unwrap_or(""),
            )
            .map_err(|e| TaskError::Store(e.to_string()))
    }

    async fn execute_ticket(&self, ticket: &RunTicket) -> Result<ExecOutput, ExecError> {
        match &ticket.context.kind {
            TaskKind::Builtin { name } => {
                let ctx = BuiltinCtx::from(&ticket.context);
                let name = name.clone();
                let db = Arc::clone(&self.db);
                let web_root = self.web_root.clone();
                let data_dir = self.data_dir.clone();
                tokio::task::spawn_blocking(move || {
                    run_builtin(&name, &ctx, &db, &web_root, &data_dir)
                })
                .await
                .map_err(|e| {
                    ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string())
                })?
            }
            TaskKind::Plugin { id, method } => {
                // 插件执行期间标记为「正作为目标运行」：此时它不能再回调
                // `scheduler` 能力面（否则「插件任务 → 注册新任务」会自我繁殖）。
                let _busy = BusyGuard::enter(self, id);
                let params = json!({
                    "task_id": ticket.context.task_id.to_string(),
                    "run_id": ticket.context.run_id,
                    "trigger": ticket.context.trigger.as_str(),
                    "attempt": ticket.context.attempt,
                    "started_at": ticket.context.started_at,
                    "snapshot_id": ticket.context.snapshot_id,
                    "missed_count": ticket.context.missed_count,
                    "payload": ticket.context.payload,
                    "points": batch_points(&ticket.batch),
                });
                let value = self
                    .call_plugin_raw(id, method, params)
                    .await
                    .map_err(|e| e.with_plugin(id.clone()))?;
                serde_json::from_value::<ExecOutput>(value).map_err(|e| {
                    ExecError::protocol(format!("插件回执不符合协议: {e}")).with_plugin(id.clone())
                })
            }
        }
    }

    async fn call_plugin_raw(
        &self,
        plugin: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, ExecError> {
        let rec = self.db.plugin(plugin).ok().flatten().ok_or_else(|| {
            ExecError::new(
                ErrorClass::Unavailable,
                ErrorPhase::Ensure,
                format!("插件 {plugin} 未安装"),
            )
            .with_plugin(plugin)
        })?;
        match self.supervisor.call(&rec, method, params).await {
            Ok(v) => Ok(v),
            Err(e) => Err(supervisor_error(e, plugin)),
        }
    }

    /// 手动触发并等待结算，返回运行记录视图（插件能力 `task.run` 用）。
    pub async fn run_task_now(&self, id: &str, actor: AuditActor) -> Result<Value, String> {
        let ticket = {
            let mut sched = self.scheduler.lock().await;
            let gate = Gate {
                db: Arc::clone(&self.db),
            };
            sched
                .run_now(id, actor, self.clock.now(), &gate)
                .map_err(|e| e.to_string())?
        };
        let run_id = ticket.context.run_id.clone();
        let timeout_secs = {
            let sched = self.scheduler.lock().await;
            sched.get(id).map(|t| t.spec.timeout_secs).unwrap_or(30)
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            self.execute_ticket(&ticket),
        )
        .await;
        let mut sched = self.scheduler.lock().await;
        let now = self.clock.now();
        let record = match outcome {
            Ok(result) => {
                sched
                    .finish(&run_id, result, now)
                    .map_err(|e| e.to_string())?
                    .record
            }
            Err(_) => sched.timeout(&run_id, now).map_err(|e| e.to_string())?,
        };
        Ok(run_view(&record))
    }

    // -- 站点备份开关 -------------------------------------------------------

    pub async fn site_backup_status(&self, domain: &str) -> SiteBackupStatus {
        let id = site_backup_task_id(domain);
        let sched = self.scheduler.lock().await;
        match sched.get(&id) {
            Some(task) => SiteBackupStatus {
                enabled: task.spec.enabled,
                task_id: id.clone(),
                last_at: task.last_run.as_ref().map(|l| l.at),
                last_result: task
                    .last_run
                    .as_ref()
                    .map(|l| l.result.as_str().to_string()),
                next_at: sched.next_run_at(&id),
            },
            None => SiteBackupStatus {
                task_id: id,
                ..Default::default()
            },
        }
    }

    /// 打开/关闭某站点的 `site-backup`。开启时直接注册内核任务，不跳计划任务页。
    pub async fn set_site_backup(
        &self,
        domain: &str,
        enabled: bool,
        admin: &str,
    ) -> Result<SiteBackupStatus, String> {
        if self
            .db
            .site_by_domain(domain)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err("站点不存在".to_string());
        }
        let id = site_backup_task_id(domain);
        let now = self.clock.now();
        let mut sched = self.scheduler.lock().await;
        match sched.get(&id) {
            Some(task) => {
                let mut patch = TaskPatch {
                    enabled: Some(enabled),
                    ..Default::default()
                };
                // 旧记录可能没带域名（早期手工注册），补一次 payload。
                if task.spec.payload.get("domain").and_then(Value::as_str) != Some(domain) {
                    patch.payload = Some(json!({ "domain": domain }));
                }
                sched
                    .update(&id, &patch, now, &self.clock)
                    .map_err(|e| e.to_string())?;
            }
            None => {
                if !enabled {
                    return Ok(SiteBackupStatus {
                        task_id: id,
                        ..Default::default()
                    });
                }
                let mut spec = TaskSpec::new(
                    id.clone(),
                    TaskKind::builtin(BUILTIN_SITE_BACKUP).map_err(|e| e.to_string())?,
                    TaskSource::Site(domain.to_string()),
                    Schedule::cron("0 3 * * *"),
                )
                .map_err(|e| e.to_string())?;
                spec.payload = json!({ "domain": domain });
                spec.created_by = format!("admin:{admin}");
                spec.created_at = now;
                spec.updated_at = now;
                sched
                    .register(spec, now, &self.clock)
                    .map_err(|e| e.to_string())?;
            }
        }
        drop(sched);
        Ok(self.site_backup_status(domain).await)
    }

    // -- Web 管理面 ---------------------------------------------------------
    //
    // 与插件能力面共用同一份 `Scheduler`，但**不再做来源隔离**：Web 侧的授权
    // 由 Session + Admin 角色判定（viewer 只读），任务来源不是访问控制维度。

    /// 任务列表（含状态、下次执行）。
    pub async fn list_views(&self) -> Vec<Value> {
        let sched = self.scheduler.lock().await;
        sched
            .list(&TaskFilter::default())
            .iter()
            .map(|t| task_view(&sched, t))
            .collect()
    }

    /// 任务详情：配置 + 最近 `limit` 次运行。
    pub async fn detail_view(&self, id: &str, limit: usize) -> Option<Value> {
        let sched = self.scheduler.lock().await;
        let task = sched.get(id)?;
        Some(json!({
            "task": task_view(&sched, &task),
            "runs": sched.history(id, limit).iter().map(run_view).collect::<Vec<_>>(),
        }))
    }

    /// 启停任务（Web 写操作）。返回最新任务视图。
    pub async fn set_enabled_task(&self, id: &str, enabled: bool) -> Result<Value, String> {
        let now = self.clock.now();
        let mut sched = self.scheduler.lock().await;
        let task = sched
            .set_enabled(id, enabled, now, &self.clock)
            .map_err(|e| e.to_string())?;
        Ok(task_view(&sched, &task))
    }

    /// 删除任务。运行历史保留（与契约一致：历史由保留期巡检统一清理），
    /// 因此删除后只是不再出现在任务列表。
    pub async fn delete_task(&self, id: &str) -> Result<(), String> {
        let now = self.clock.now();
        let mut sched = self.scheduler.lock().await;
        sched.delete(id, now).map_err(|e| e.to_string())
    }

    // -- 插件能力面 ---------------------------------------------------------

    /// `scheduler` 能力：插件（cron 管理 UI）读写任务的唯一入口。
    ///
    /// 边界：
    /// - `scheduler:read/manage/run` 只作用于**自己来源**的任务；
    /// - `scheduler:admin` 是内核指定的「任务管理 UI」权限（cron 专属），可看/管全量；
    /// - `task.register` 的 kind/source/id 由内核按调用方身份注入，插件无法伪造。
    pub async fn capability(
        &self,
        caller: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let read = self.has_perm(caller, "scheduler:read");
        let manage = self.has_perm(caller, "scheduler:manage");
        let run = self.has_perm(caller, "scheduler:run");
        let admin = self.has_perm(caller, "scheduler:admin");
        match method {
            "task.list" => {
                if !read {
                    return Err("缺少 scheduler:read".into());
                }
                let filter = TaskFilter {
                    source: params
                        .get("source")
                        .and_then(Value::as_str)
                        .and_then(|s| TaskSource::parse(s).ok()),
                    enabled: params.get("enabled").and_then(Value::as_bool),
                    ..Default::default()
                };
                let sched = self.scheduler.lock().await;
                let tasks = sched.list(&filter);
                let tasks: Vec<Task> = if admin {
                    tasks
                } else {
                    tasks
                        .into_iter()
                        .filter(|t| t.spec.source.plugin_id() == Some(caller))
                        .collect()
                };
                Ok(json!({
                    "tasks": tasks.iter().map(|t| task_view(&sched, t)).collect::<Vec<_>>(),
                }))
            }
            "task.get" => {
                if !read {
                    return Err("缺少 scheduler:read".into());
                }
                let id = require_id(&params)?;
                let sched = self.scheduler.lock().await;
                let task = sched.get(&id).ok_or_else(|| "任务不存在".to_string())?;
                if !admin && task.spec.source.plugin_id() != Some(caller) {
                    return Err("只能查看自己来源的任务".into());
                }
                Ok(json!({
                    "task": task_view(&sched, &task),
                    "runs": sched.history(&id, 20).iter().map(run_view).collect::<Vec<_>>(),
                }))
            }
            "task.history" => {
                if !read {
                    return Err("缺少 scheduler:read".into());
                }
                let id = require_id(&params)?;
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                let sched = self.scheduler.lock().await;
                let owned = sched
                    .get(&id)
                    .and_then(|t| t.spec.source.plugin_id().map(str::to_string));
                if !admin && owned.as_deref() != Some(caller) {
                    return Err("只能查看自己来源的任务".into());
                }
                Ok(
                    json!({ "runs": sched.history(&id, limit).iter().map(run_view).collect::<Vec<_>>() }),
                )
            }
            "buffer.push" => {
                if !manage {
                    return Err("缺少 scheduler:manage".into());
                }
                let id = require_id(&params)?;
                let point = params
                    .get("point")
                    .cloned()
                    .ok_or_else(|| "缺少 point".to_string())?;
                self.ensure_owned(caller, &id).await?;
                let now = self.clock.now();
                if !self.allow_push(&id, now) {
                    return Err(format!(
                        "buffer.push 超过限速（每任务 {PUSH_PER_SECOND} 点/秒）"
                    ));
                }
                let mut sched = self.scheduler.lock().await;
                let report = sched.push(&id, point, now).map_err(|e| e.to_string())?;
                Ok(push_report_view(&report))
            }
            "buffer.stat" => {
                if !read {
                    return Err("缺少 scheduler:read".into());
                }
                let id = require_id(&params)?;
                self.ensure_owned(caller, &id).await?;
                let sched = self.scheduler.lock().await;
                let stat = sched
                    .buffer_stat(&id)
                    .ok_or_else(|| "任务不存在".to_string())?;
                Ok(json!({ "stat": stat }))
            }
            "task.register" => {
                if !manage {
                    return Err("缺少 scheduler:manage".into());
                }
                self.register_plugin_task(caller, params).await
            }
            "task.update" => {
                if !manage {
                    return Err("缺少 scheduler:manage".into());
                }
                let id = require_id(&params)?;
                if !admin {
                    self.ensure_owned(caller, &id).await?;
                }
                let patch: TaskPatch = serde_json::from_value(
                    params.get("patch").cloned().unwrap_or_else(|| json!({})),
                )
                .map_err(|e| format!("patch 非法: {e}"))?;
                let mut sched = self.scheduler.lock().await;
                let task = sched
                    .update(&id, &patch, self.clock.now(), &self.clock)
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "task": task_view(&sched, &task) }))
            }
            "task.delete" => {
                if !manage {
                    return Err("缺少 scheduler:manage".into());
                }
                let id = require_id(&params)?;
                if !admin {
                    self.ensure_owned(caller, &id).await?;
                }
                let mut sched = self.scheduler.lock().await;
                sched
                    .delete(&id, self.clock.now())
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "deleted": id }))
            }
            "task.run" => {
                if !run {
                    return Err("缺少 scheduler:run".into());
                }
                let id = require_id(&params)?;
                if !admin {
                    self.ensure_owned(caller, &id).await?;
                }
                let actor = AuditActor::Plugin(caller.to_string());
                let record = self.run_task_now(&id, actor).await?;
                Ok(json!({ "run": record }))
            }
            other => Err(format!("未知的 scheduler 方法: {other}")),
        }
    }

    async fn register_plugin_task(&self, caller: &str, params: Value) -> Result<Value, String> {
        let local_key = params
            .get("local_key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if local_key.is_empty()
            || local_key.len() > 64
            || !local_key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err("local_key 必须是 1–64 位 [A-Za-z0-9._-]".into());
        }
        let method = params
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if method.is_empty() {
            return Err("method 不能为空".into());
        }
        // `target` 只允许 `scheduler:admin`（内核指定的任务管理 UI）指定：
        // 普通插件注册的任务只能指向自己，避免借 UI 无授权调度他人。
        let target = params
            .get("target")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let owner = match target {
            Some(target) if self.has_perm(caller, "scheduler:admin") => target,
            _ => caller,
        };
        if owner.chars().count() > 128 || !valid_plugin_id(owner) {
            return Err("target 插件 id 非法".into());
        }
        let id = format!("plugin:{caller}:{local_key}");
        let kind =
            TaskKind::parse(&format!("plugin:{owner}:{method}")).map_err(|e| e.to_string())?;

        let payload = params
            .get("payload")
            .cloned()
            .map(|mut v| {
                if let Some(o) = v.as_object_mut() {
                    if !o.contains_key("key") {
                        o.insert("key".into(), Value::String(local_key.clone()));
                    }
                }
                v
            })
            .unwrap_or_else(|| json!({ "key": local_key }));
        if !payload.is_object() {
            return Err("payload 必须是 JSON 对象".into());
        }

        let schedule = match params.get("schedule") {
            Some(v) if v.is_object() => {
                serde_json::from_value(v.clone()).map_err(|e| format!("schedule 非法: {e}"))?
            }
            _ => {
                let expr = params
                    .get("expr")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "缺少 schedule 或 expr".to_string())?;
                Schedule::cron(expr)
            }
        };
        let now = self.clock.now();
        let mut sched = self.scheduler.lock().await;
        if sched.get(&id).is_some() {
            let patch = TaskPatch {
                schedule: Some(schedule),
                payload: Some(payload),
                enabled: params.get("enabled").and_then(Value::as_bool),
                timeout_secs: params.get("timeout_secs").and_then(Value::as_u64),
                ..Default::default()
            };
            let task = sched
                .update(&id, &patch, now, &self.clock)
                .map_err(|e| e.to_string())?;
            return Ok(json!({
                "created": false,
                "id": id,
                "task": task_view(&sched, &task),
            }));
        }

        // 每插件任务上限（契约 §10.2：≤20）。任务归属按 `owner` 计：
        // 管理 UI 代他人注册时，配额记在目标插件名下。
        let owned = sched
            .list(&TaskFilter::default())
            .into_iter()
            .filter(|t| t.spec.source.plugin_id() == Some(owner))
            .count();
        if owned >= 20 {
            return Err("每插件最多注册 20 条任务".into());
        }
        let mut spec = TaskSpec::new(
            id.clone(),
            kind,
            TaskSource::Plugin(owner.to_string()),
            schedule,
        )
        .map_err(|e| e.to_string())?;
        spec.payload = payload;
        spec.created_by = format!("plugin:{caller}");
        spec.created_at = now;
        spec.updated_at = now;
        let task = sched
            .register(spec, now, &self.clock)
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "created": true,
            "id": id,
            "task": task_view(&sched, &task),
        }))
    }

    /// 限速窗口按秒滚动；只统计真正进入 scheduler 的点（参数校验失败不计）。
    fn allow_push(&self, id: &str, now: i64) -> bool {
        let Ok(mut windows) = self.push_window.lock() else {
            return false;
        };
        let slot = windows.entry(id.to_string()).or_insert((now, 0));
        if slot.0 != now {
            *slot = (now, 0);
        }
        if slot.1 >= PUSH_PER_SECOND {
            return false;
        }
        slot.1 += 1;
        true
    }

    async fn ensure_owned(&self, caller: &str, id: &str) -> Result<(), String> {
        let sched = self.scheduler.lock().await;
        let task = sched.get(id).ok_or_else(|| "任务不存在".to_string())?;
        if task.spec.source.plugin_id() != Some(caller) {
            return Err("只能管理本插件来源的任务".into());
        }
        Ok(())
    }

    fn has_perm(&self, plugin: &str, perm: &str) -> bool {
        self.db
            .plugin(plugin)
            .ok()
            .flatten()
            .map(|p: PluginRecord| {
                p.grants
                    .iter()
                    .any(|g: &Grant| g.kind == "perm" && g.value == perm)
            })
            .unwrap_or(false)
    }
}

/// 插件 id 词法：与 manifest `id` 一致（字母/数字/`.`/`_`/`-`，≤128）。
fn valid_plugin_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 种下内置任务：目前只有全局 `db-vacuum`（每周日 04:00）。
/// 幂等：已有记录直接跳过，不覆盖用户改过的调度。
fn seed_builtins(
    sched: &mut Scheduler<SqliteTaskStore>,
    clock: &SystemClock,
) -> Result<(), String> {
    if sched.get(DB_VACUUM_TASK_ID).is_some() {
        return Ok(());
    }
    let mut spec = TaskSpec::new(
        DB_VACUUM_TASK_ID,
        TaskKind::builtin(BUILTIN_DB_VACUUM).map_err(|e| e.to_string())?,
        TaskSource::System,
        Schedule::cron("0 4 * * 0"),
    )
    .map_err(|e| e.to_string())?;
    spec.payload = json!({});
    spec.created_by = "system".into();
    spec.created_at = clock.now();
    spec.updated_at = spec.created_at;
    sched
        .register(spec, clock.now(), clock)
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 内置执行体
// ---------------------------------------------------------------------------

/// 从 `RunContext` 摘出的可 `Send` 副本（`RunContext` 自带取消位，不能直接跨线程）。
struct BuiltinCtx {
    task_id: String,
    run_id: String,
    trigger: Trigger,
    attempt: u32,
    started_at: i64,
    snapshot_id: u64,
    missed_count: u64,
    payload: Value,
}

impl From<&RunContext> for BuiltinCtx {
    fn from(ctx: &RunContext) -> Self {
        Self {
            task_id: ctx.task_id.to_string(),
            run_id: ctx.run_id.clone(),
            trigger: ctx.trigger,
            attempt: ctx.attempt,
            started_at: ctx.started_at,
            snapshot_id: ctx.snapshot_id,
            missed_count: ctx.missed_count,
            payload: ctx.payload.clone(),
        }
    }
}

fn run_builtin(
    name: &str,
    ctx: &BuiltinCtx,
    db: &Db,
    web_root: &Path,
    data_dir: &Path,
) -> Result<ExecOutput, ExecError> {
    tracing::debug!(
        task = %ctx.task_id,
        run = %ctx.run_id,
        trigger = ctx.trigger.as_str(),
        attempt = ctx.attempt,
        started_at = ctx.started_at,
        snapshot_id = ctx.snapshot_id,
        missed = ctx.missed_count,
        builtin = name,
        "执行内置任务"
    );
    match name {
        BUILTIN_SITE_BACKUP => site_backup(ctx, db, web_root, data_dir),
        BUILTIN_DB_VACUUM => db_vacuum(ctx, db, data_dir),
        other => Err(ExecError::new(
            ErrorClass::Unavailable,
            ErrorPhase::Ensure,
            format!("未注册的内置任务: {other}"),
        )),
    }
}

fn site_backup(
    ctx: &BuiltinCtx,
    db: &Db,
    web_root: &Path,
    data_dir: &Path,
) -> Result<ExecOutput, ExecError> {
    let domain = ctx
        .payload
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ExecError::new(
                ErrorClass::InvalidParams,
                ErrorPhase::Call,
                "payload 缺少 domain",
            )
        })?;
    let site = db
        .site_by_domain(domain)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?
        .ok_or_else(|| {
            ExecError::new(
                ErrorClass::InvalidParams,
                ErrorPhase::Call,
                format!("站点 {domain} 不存在"),
            )
        })?;
    let root = PathBuf::from(&site.root_dir);
    let root = if root.is_absolute() {
        root
    } else {
        web_root.join(root)
    };
    if !root.is_dir() {
        return Err(ExecError::new(
            ErrorClass::InvalidParams,
            ErrorPhase::Call,
            format!("站点目录不存在: {}", root.display()),
        ));
    }

    let dir = data_dir.join("backups").join("sites").join(domain);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    let ts = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("{ts}.tar"));
    let file = std::fs::File::create(&path)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    let mut builder = tar::Builder::new(file);
    builder
        .append_dir_all(".", &root)
        .and_then(|_| builder.into_inner().map(|_| ()))
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    let bytes = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    Ok(ExecOutput {
        verdict: None,
        detail: Some(path.display().to_string()),
        output_bytes: bytes,
    })
}

fn db_vacuum(ctx: &BuiltinCtx, db: &Db, data_dir: &Path) -> Result<ExecOutput, ExecError> {
    let _ = ctx;
    let dir = data_dir.join("backups").join("db");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    let ts = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("skiff-{ts}.db"));
    db.backup_to(&path)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    let bytes = std::fs::metadata(&path)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    Ok(ExecOutput {
        verdict: None,
        detail: Some(path.display().to_string()),
        output_bytes: bytes,
    })
}

fn batch_points(batch: &Batch) -> Value {
    serde_json::to_value(&batch.points).unwrap_or_else(|_| json!([]))
}

fn point_of(value: Value) -> Option<Value> {
    match value {
        Value::Null => None,
        Value::Object(mut o) => o.remove("point").or(Some(Value::Object(o))),
        other => Some(other),
    }
}

fn supervisor_error(e: SupervisorError, plugin: &str) -> ExecError {
    let class = match &e {
        SupervisorError::NotInstalled(_) | SupervisorError::CircuitOpen(_) => {
            ErrorClass::Unavailable
        }
        SupervisorError::Spawn(_) => ErrorClass::Spawn,
        SupervisorError::Manifest(_) | SupervisorError::Store(_) => ErrorClass::Protocol,
        SupervisorError::Unsupported(_) | SupervisorError::Ipc(_) => ErrorClass::ExecutorFailed,
    };
    ExecError::new(class, ErrorPhase::Call, e.to_string()).with_plugin(plugin)
}

// ---------------------------------------------------------------------------
// 视图
// ---------------------------------------------------------------------------

fn fmt_ts(at: Option<i64>) -> Option<String> {
    at.and_then(|t| Local.timestamp_opt(t, 0).single())
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn schedule_view(task: &Task) -> (String, Value) {
    match &task.spec.schedule {
        Schedule::Cron { expr, .. } => (
            expr.clone(),
            serde_json::to_value(&task.spec.schedule).unwrap_or(Value::Null),
        ),
        Schedule::Interval { secs, .. } => (
            format!("每 {secs} 秒"),
            serde_json::to_value(&task.spec.schedule).unwrap_or(Value::Null),
        ),
    }
}

fn task_view(sched: &Scheduler<SqliteTaskStore>, task: &Task) -> Value {
    let (expr, schedule) = schedule_view(task);
    let next = sched.next_run_at(task.spec.id.as_str());
    let state = sched
        .task_state(task.spec.id.as_str())
        .unwrap_or(TaskState::Idle);
    let origin = if task.spec.created_by.starts_with("admin") {
        "ui"
    } else {
        task.spec.source.plugin_id().unwrap_or("system")
    };
    let plugin_id = task
        .spec
        .kind
        .plugin_id()
        .or_else(|| task.spec.source.plugin_id());
    json!({
        "id": task.spec.id.as_str(),
        "name": task.spec.payload.get("name").and_then(Value::as_str).unwrap_or(task.spec.id.as_str()),
        "expr": expr,
        "schedule": schedule,
        "enabled": task.spec.enabled,
        "state": enum_str(&state),
        "lifecycle": enum_str(&task.state),
        "blocked_reason": task.blocked_reason.as_ref().map(enum_str),
        "source": origin,
        "target": plugin_id,
        "method": task.spec.kind.method(),
        "key": task.spec.payload.get("key").and_then(Value::as_str).unwrap_or(""),
        "payload": task.spec.payload,
        "timeout_secs": task.spec.timeout_secs,
        "last": task.last_run.as_ref().map(|l| json!({
            "at": l.at,
            "at_text": fmt_ts(Some(l.at)),
            "result": l.result.as_str(),
            "detail": "",
        })),
        "next_run": next.map(|at| json!({ "at": at, "text": fmt_ts(Some(at)) })),
    })
}

fn push_report_view(report: &PushReport) -> Value {
    let outcome = match &report.outcome {
        PushOutcome::Accepted {
            seq,
            bytes,
            dropped_points,
            dropped_bytes,
            aggregated,
        } => json!({
            "accepted": true,
            "seq": seq,
            "bytes": bytes,
            "dropped_points": dropped_points,
            "dropped_bytes": dropped_bytes,
            "aggregated": aggregated,
        }),
        PushOutcome::Rejected { reason, bytes } => json!({
            "accepted": false,
            "reason": reject_reason_str(reason),
            "bytes": bytes,
        }),
    };
    json!({
        "outcome": outcome,
        "signals": report.signals.iter().map(buffer_signal_str).collect::<Vec<_>>(),
        "net_bytes": report.net_bytes,
    })
}

fn reject_reason_str(reason: &RejectReason) -> &'static str {
    match reason {
        RejectReason::PointTooLarge => "point_too_large",
        RejectReason::Full => "full",
        RejectReason::GlobalDegraded => "global_degraded",
        RejectReason::PluginQuota => "plugin_quota",
    }
}

fn buffer_signal_str(signal: &BufferSignal) -> &'static str {
    match signal {
        BufferSignal::HighWatermark => "high_watermark",
        BufferSignal::Overflow => "overflow",
        BufferSignal::Recovered => "recovered",
        BufferSignal::PointTooLarge => "point_too_large",
    }
}

fn run_view(r: &RunRecord) -> Value {
    json!({
        "id": &r.id,
        "task_id": r.task_id.as_str(),
        "trigger": r.trigger.as_str(),
        "attempt": r.attempt,
        "started_at": r.started_at,
        "finished_at": r.finished_at,
        "started_text": fmt_ts(Some(r.started_at)),
        "finished_text": fmt_ts(Some(r.finished_at)),
        "result": r.result.as_str(),
        "error_class": r.error_class.as_ref().map(|c| c.as_str()),
        "error_detail": &r.error_detail,
        "duration_ms": r.duration_ms,
        "output_bytes": r.output_bytes,
        "missed_count": r.missed_count,
    })
}

fn require_id(params: &Value) -> Result<String, String> {
    params
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "缺少 id".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_store::NewSite;

    fn service(dir: &tempfile::TempDir) -> Arc<TaskService> {
        let db = Arc::new(Db::open(dir.path().join("skiff.db")).unwrap());
        db.create_site(&NewSite {
            domain: "example.com".into(),
            root_dir: dir.path().join("www/example.com").display().to_string(),
            site_type: skiff_store::TYPE_STATIC.into(),
            runtime_id: None,
            app_command: None,
            app_port: None,
            proxy_target: None,
            proxy_websocket: false,
        })
        .unwrap();
        std::fs::create_dir_all(dir.path().join("www/example.com")).unwrap();
        let supervisor = Arc::new(skiff_plugin::Supervisor::new(
            skiff_plugin::SupervisorConfig::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            db.clone(),
        ));
        TaskService::open(
            db,
            supervisor,
            dir.path().join("www"),
            dir.path().to_path_buf(),
        )
        .unwrap()
    }

    #[test]
    fn tasks_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let db = Arc::clone(svc.db_handle());
        drop(svc);

        let reloaded = TaskService::open(
            db,
            Arc::new(skiff_plugin::Supervisor::new(
                skiff_plugin::SupervisorConfig::new(
                    dir.path().to_path_buf(),
                    dir.path().to_path_buf(),
                ),
                Arc::new(Db::open(dir.path().join("skiff.db")).unwrap()),
            )),
            dir.path().join("www"),
            dir.path().to_path_buf(),
        )
        .unwrap();
        let sched = reloaded.scheduler.blocking_lock();
        assert!(sched.get(DB_VACUUM_TASK_ID).is_some());
    }

    #[tokio::test]
    async fn site_backup_switch_registers_and_disables_task() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);

        let on = svc
            .set_site_backup("example.com", true, "admin")
            .await
            .unwrap();
        assert!(on.enabled);
        assert_eq!(on.task_id, site_backup_task_id("example.com"));

        let off = svc
            .set_site_backup("example.com", false, "admin")
            .await
            .unwrap();
        assert!(!off.enabled);

        let unknown = svc.set_site_backup("nope.example", true, "admin").await;
        assert!(unknown.is_err());
    }

    #[tokio::test]
    async fn site_backup_task_writes_an_archive() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        svc.set_site_backup("example.com", true, "admin")
            .await
            .unwrap();
        let record = svc
            .run_task_now(
                &site_backup_task_id("example.com"),
                AuditActor::Admin("admin".into()),
            )
            .await
            .unwrap();
        assert_eq!(record["result"], "ok");
        let archived = dir
            .path()
            .join("backups/sites/example.com")
            .read_dir()
            .unwrap()
            .count();
        assert_eq!(archived, 1);
    }

    #[tokio::test]
    async fn buffer_push_and_stat_are_exposed_to_the_owning_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let plugin = "com.skiff.monitor";
        svc.db_handle()
            .upsert_plugin(&PluginRecord {
                id: plugin.into(),
                name: "monitor".into(),
                version: "0.1.0".into(),
                api: KERNEL_API.into(),
                key_id: "0".into(),
                manifest: String::new(),
                enabled: true,
                status: "enabled".into(),
                status_detail: String::new(),
                active_version: "0.1.0".into(),
                installed_at: 0,
                grants: vec![
                    Grant::perm("scheduler:read"),
                    Grant::perm("scheduler:manage"),
                ],
            })
            .unwrap();
        let id = format!("plugin:{plugin}:sample");
        {
            let mut sched = svc.scheduler.lock().await;
            let mut spec = TaskSpec::new(
                id.clone(),
                TaskKind::parse(&id).unwrap(),
                TaskSource::Plugin(plugin.into()),
                Schedule::cron("* * * * *"),
            )
            .unwrap();
            spec.buffer = Some(skiff_sched::BufferConfig::default());
            sched.register(spec, svc.clock.now(), &svc.clock).unwrap();
        }

        let stat = svc
            .capability(plugin, "buffer.stat", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(stat["stat"]["points"], 0);
        let pushed = svc
            .capability(
                plugin,
                "buffer.push",
                json!({ "id": id, "point": { "v": 1 } }),
            )
            .await
            .unwrap();
        assert_eq!(pushed["outcome"]["accepted"], true);
        let stat = svc
            .capability(plugin, "buffer.stat", json!({ "id": id }))
            .await
            .unwrap();
        assert_eq!(stat["stat"]["points"], 1);
        // 非本人任务不可读写。
        let denied = svc
            .capability(plugin, "buffer.stat", json!({ "id": "builtin:db-vacuum" }))
            .await;
        assert!(denied.is_err());
    }

    #[tokio::test]
    async fn admin_ui_can_register_a_task_for_another_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let ui = "com.skiff.cron";
        let target = "com.skiff.testkit";
        svc.db_handle()
            .upsert_plugin(&PluginRecord {
                id: ui.into(),
                name: "cron".into(),
                version: "0.1.0".into(),
                api: KERNEL_API.into(),
                key_id: "0".into(),
                manifest: String::new(),
                enabled: true,
                status: "enabled".into(),
                status_detail: String::new(),
                active_version: "0.1.0".into(),
                installed_at: 0,
                grants: vec![
                    Grant::perm("scheduler:read"),
                    Grant::perm("scheduler:manage"),
                    Grant::perm("scheduler:admin"),
                ],
            })
            .unwrap();
        svc.capability(
            ui,
            "task.register",
            json!({
                "target": target,
                "local_key": "probe",
                "method": "probe",
                "expr": "0 5 * * *",
                "payload": {},
            }),
        )
        .await
        .unwrap();
        let id = format!("plugin:{ui}:probe");
        let sched = svc.scheduler.lock().await;
        let task = sched.get(&id).expect("任务应已注册");
        // 归属必须与执行目标一致，否则模型校验会拒绝（source/kind 同 id）。
        assert_eq!(task.spec.source.plugin_id(), Some(target));
        assert_eq!(
            task.spec.kind.to_string_value(),
            format!("plugin:{target}:probe")
        );
    }

    #[test]
    fn buffer_push_rate_limit_is_per_task_and_per_second() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        for _ in 0..PUSH_PER_SECOND {
            assert!(svc.allow_push("t1", 1000));
        }
        assert!(!svc.allow_push("t1", 1000));
        assert!(svc.allow_push("t1", 1001));
        assert!(svc.allow_push("t2", 1000));
    }

    #[tokio::test]
    async fn plugin_gate_marks_unavailable_as_blocked_instead_of_failure() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        {
            let mut sched = svc.scheduler.lock().await;
            let spec = TaskSpec::new(
                "plugin:com.skiff.none:sample",
                TaskKind::parse("plugin:com.skiff.none:sample").unwrap(),
                TaskSource::Plugin("com.skiff.none".into()),
                Schedule::cron("* * * * *"),
            )
            .unwrap();
            let now = svc.clock.now();
            sched.register(spec, now, &svc.clock).unwrap();
        }
        // 直接 tick：插件未安装 → 任务被挡，不产生运行记录。
        Arc::clone(&svc).tick_once().await;
        let sched = svc.scheduler.lock().await;
        assert_eq!(
            sched.task_state("plugin:com.skiff.none:sample"),
            Some(TaskState::Blocked)
        );
        assert!(sched.history("plugin:com.skiff.none:sample", 10).is_empty());
    }
}
