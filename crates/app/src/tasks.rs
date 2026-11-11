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
//!
//! 仓储适配（`TaskStore` 实现与行映射）拆在 `tasks/store.rs`；本文件保留常量、
//! 对内暴露的只读状态，以及全部执行/触发语义（tick 驱动、内置执行体、插件门、视图）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use skiff_plugin::supervisor::{Supervisor, SupervisorError};
use skiff_sched::{
    AuditActor, Batch, BufferConfig, BufferSignal, Clock, ErrorClass, ErrorPhase, ExecError,
    ExecOutput, PluginGate, PluginStatus, PushOutcome, PushReport, RejectReason, RunContext,
    RunRecord, RunTicket, SampleTicket, Schedule, Scheduler, SchedulerConfig, SystemClock, Task,
    TaskError, TaskFilter, TaskKind, TaskPatch, TaskSource, TaskSpec, TaskState, Trigger,
};
use skiff_store::{Db, Grant, PluginRecord};
use tokio::sync::Mutex;

/// 内核插件 API 版本（manifest `api` 不匹配即视为不可用）。
pub const KERNEL_API: &str = "1.2";
/// 内置任务名：站点备份。
pub const BUILTIN_SITE_BACKUP: &str = "site-backup";
/// 内置任务名：数据库整理。
pub const BUILTIN_DB_VACUUM: &str = "db-vacuum";
/// 内置任务 id：数据库整理（全局唯一）。
pub const DB_VACUUM_TASK_ID: &str = "builtin:db-vacuum";
/// 内置任务名：回收站到期清理。
pub const BUILTIN_TRASH_PRUNE: &str = "trash-prune";
/// 内置任务 id：回收站到期清理（全局唯一）。
pub const TRASH_PRUNE_TASK_ID: &str = "builtin:trash-prune";
/// 内置任务名：审计按保留期清理。
pub const BUILTIN_AUDIT_PRUNE: &str = "audit-prune";
/// 内置任务 id：审计清理（全局唯一）。
pub const AUDIT_PRUNE_TASK_ID: &str = "builtin:audit-prune";
/// 审计保留天数：只读审计不采样、保留完整追溯，靠时间窗裁剪。
pub const AUDIT_RETENTION_DAYS: i64 = 90;

/// 保留截止时间：`now - 90 天`（秒）。删除条件是严格 `ts < cutoff`，
/// 即恰好 90 天前的记录仍在窗口内。
fn audit_cutoff(now: i64) -> i64 {
    now.saturating_sub(AUDIT_RETENTION_DAYS * 24 * 3600)
}

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

mod store;

pub use store::SqliteTaskStore;
use store::{enum_str, run_row};

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
    /// 已超时结算、但插件执行体仍未返回的任务：`task_id -> run_id`。
    /// 在回执到达（执行体结束）或插件进程重启（调用报错）前禁止同一任务重入。
    reentry_block: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// `buffer.push` 的每任务限速窗口：`(秒序号, 该秒已收点数)`。
    push_window: std::sync::Mutex<std::collections::HashMap<String, (i64, u32)>>,
    web_root: PathBuf,
    data_dir: PathBuf,
    trash_retention_days: u32,
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
    /// 补最后一次 `buffer.lost_on_restart`，并种下内置任务。
    pub fn open(
        db: Arc<Db>,
        supervisor: Arc<Supervisor>,
        web_root: PathBuf,
        data_dir: PathBuf,
        trash_retention_days: u32,
    ) -> Result<Arc<Self>, String> {
        let clock = SystemClock;
        let mut scheduler = Scheduler::new(
            SqliteTaskStore::new(Arc::clone(&db)),
            SchedulerConfig::default(),
            &clock,
        )
        .map_err(|e| e.to_string())?;
        if let Err(e) = db.mark_tasks_loaded() {
            tracing::warn!(error = %e, "标记任务已加载失败");
        }
        // 在把 scheduler 交给 tokio Mutex 之前种内置任务：`open` 既可能在运行时内
        // 调用（bootstrap），也可能在运行时外（单测），不能依赖 `blocking_lock`。
        seed_builtins(&mut scheduler, &clock)?;
        // 插件把「按什么节奏驱动我」声明在 manifest `[[task]]` 里；内核在这里种任务。
        // 插件是 on-demand 进程，没有「启动时自注册」的时机，声明式种入是唯一单一路径。
        seed_manifest_tasks(&mut scheduler, &db, &clock);

        Ok(Arc::new(Self {
            db,
            supervisor,
            clock,
            scheduler: Mutex::new(scheduler),
            executing: std::sync::Mutex::new(std::collections::HashSet::new()),
            reentry_block: std::sync::Mutex::new(std::collections::HashMap::new()),
            push_window: std::sync::Mutex::new(std::collections::HashMap::new()),
            web_root,
            data_dir,
            trash_retention_days,
        }))
    }

    /// 插件是否正在作为任务执行目标。调度能力面据此拒绝重入。
    pub fn is_executing(&self, plugin_id: &str) -> bool {
        self.executing
            .lock()
            .map(|s| s.contains(plugin_id))
            .unwrap_or(false)
    }

    /// 记录「已超时但执行体未返回」：回执到达或插件重启前禁止同一任务重入。
    fn block_reentry(&self, task_id: &str, run_id: &str) {
        if let Ok(mut map) = self.reentry_block.lock() {
            map.insert(task_id.to_string(), run_id.to_string());
        }
    }

    /// 执行体结束（拿到回执或调用报错）后解除封锁；只解除同一次运行，
    /// 避免晚到的旧回执把新一轮的封锁误清掉。
    fn release_reentry(&self, task_id: &str, run_id: &str) -> bool {
        let Ok(mut map) = self.reentry_block.lock() else {
            return false;
        };
        if map.get(task_id).map(String::as_str) != Some(run_id) {
            return false;
        }
        map.remove(task_id);
        true
    }

    /// 该任务当前是否被上一轮超时运行阻塞；返回阻塞的 `run_id`。
    fn reentry_blocked_by(&self, task_id: &str) -> Option<String> {
        self.reentry_block
            .lock()
            .ok()
            .and_then(|m| m.get(task_id).cloned())
    }

    /// 把「重入被拒」结算成一条失败记录，错误码 `reentry_blocked`。
    async fn settle_reentry_blocked(&self, run_id: &str, task_id: &str, prev_run: &str) {
        let error = ExecError::new(
            ErrorClass::ReentryBlocked,
            ErrorPhase::Call,
            format!("上一次运行 {prev_run} 已超时但插件尚未回执（或进程未重启），拒绝重入"),
        );
        tracing::warn!(
            task = task_id,
            run_id,
            prev_run,
            "任务重入被拒：上一轮仍在等待回执"
        );
        self.settle_run(run_id, Err(error)).await;
    }

    async fn settle_run(&self, run_id: &str, result: Result<ExecOutput, ExecError>) {
        let mut sched = self.scheduler.lock().await;
        let now = self.clock.now();
        if let Err(e) = sched.finish(run_id, result, now) {
            tracing::warn!(run_id, error = %e, "运行结算被拒绝（结果已过期或重复）");
        }
    }

    async fn settle_run_timeout(&self, run_id: &str) {
        let mut sched = self.scheduler.lock().await;
        let now = self.clock.now();
        if let Err(e) = sched.timeout(run_id, now) {
            tracing::warn!(run_id, error = %e, "超时结算失败");
        }
    }

    pub fn db_handle(&self) -> &Arc<Db> {
        &self.db
    }

    /// Web 安装/升级后即时同步 manifest 任务，不必重启内核。
    pub async fn sync_manifest_tasks(&self) {
        let mut sched = self.scheduler.lock().await;
        seed_manifest_tasks(&mut sched, &self.db, &self.clock);
    }

    /// 卸载插件后回收其名下全部任务（含 cron 等 UI 代建的同 source 任务）。
    pub async fn drop_plugin_tasks(&self, plugin_id: &str) {
        let mut sched = self.scheduler.lock().await;
        let now = self.clock.now();
        for task in sched.list(&TaskFilter::default()) {
            if task.spec.plugin_id() != Some(plugin_id) {
                continue;
            }
            if let Err(e) = sched.delete(task.spec.id.as_str(), now) {
                tracing::warn!(task = %task.spec.id, error = %e, "卸载插件时回收任务失败");
            }
        }
    }

    /// 1s tick：拿票 → 执行体独立 task → 结算。**不在锁内执行任何 RPC**。
    ///
    /// 串行不变量：票据（以及 `Running`/`Queued` 状态与 `last_run`）全部在 `sched.tick`
    /// 内、调度锁下产出；锁外只做两件事——把 `RunRecord`/审计物化落盘、派发执行体。
    /// 因此即使落盘改到后台，第二个 tick 也拿不到同一任务的第二张票；tick 循环本身是
    /// 单任务顺序 `await`（`core::server::spawn_task_loop`），不会并发重入。
    pub async fn tick_once(self: &Arc<Self>) {
        let mut runs = Vec::new();
        let mut samples = Vec::new();
        let (records, audits) = {
            let mut sched = self.scheduler.lock().await;
            let gate = Gate {
                db: Arc::clone(&self.db),
            };
            let report = sched.tick(&self.clock, &gate);
            let records = report.records;
            let audits = report.audits;
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
            (records, audits)
        };

        // 落盘与调度判定解耦：锁外、有界阻塞线程上把运行记录与审计写库。
        let db = Arc::clone(&self.db);
        let persisted = crate::db_async::run(move || {
            for rec in records {
                if let Err(e) = db.append_task_run(&run_row(&rec)) {
                    tracing::warn!(error = %e, "任务运行记录落盘失败");
                }
            }
            for ev in audits {
                if let Err(e) = db.audit(
                    &ev.actor.as_string(),
                    "sched",
                    ev.action_str(),
                    &ev.target,
                    ev.outcome.as_str(),
                    ev.detail.as_deref().unwrap_or(""),
                ) {
                    tracing::error!(error = %e, "任务审计落盘失败");
                }
            }
        })
        .await;
        if let Err(e) = persisted {
            tracing::error!(error = %e, "投递任务落盘失败");
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
    ///
    /// 插件任务超时后**不丢弃**执行体：内核继续等它的回执，在此之前同一任务
    /// 一律按 `reentry_blocked` 拒绝重入；插件进程重启会让调用报错，同样解除封锁。
    pub async fn drive_run(self: Arc<Self>, ticket: RunTicket, timeout_secs: u64) {
        let run_id = ticket.context.run_id.clone();
        let task_id = ticket.context.task_id.to_string();
        let is_plugin = matches!(&ticket.context.kind, TaskKind::Plugin { .. });
        let dur = Duration::from_secs(timeout_secs.max(1));

        if let Some(prev_run) = self.reentry_blocked_by(&task_id) {
            self.settle_reentry_blocked(&run_id, &task_id, &prev_run)
                .await;
            return;
        }

        if !is_plugin {
            // 内置任务：沿用「超时即丢执行体」的语义（spawn_blocking 也无法强杀）。
            match tokio::time::timeout(dur, self.execute_ticket(&ticket)).await {
                Ok(result) => self.settle_run(&run_id, result).await,
                Err(_) => self.settle_run_timeout(&run_id).await,
            }
            return;
        }

        // 插件任务：把执行体放进独立 task，超时只中断「等待」而不取消调用本身，
        // 这样回执真正到达时才能解除重入封锁。
        let (tx, rx) = tokio::sync::oneshot::channel();
        let exec = {
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                let _ = tx.send(me.execute_ticket(&ticket).await);
            })
        };
        match tokio::time::timeout(dur, rx).await {
            Ok(Ok(result)) => self.settle_run(&run_id, result).await,
            Ok(Err(_)) => {
                let error = ExecError::new(
                    ErrorClass::ExecutorFailed,
                    ErrorPhase::Call,
                    "执行体异常退出（未返回回执）",
                );
                self.settle_run(&run_id, Err(error)).await;
            }
            Err(_) => {
                self.block_reentry(&task_id, &run_id);
                self.settle_run_timeout(&run_id).await;
                let me = Arc::clone(&self);
                tokio::spawn(async move {
                    // 等回执或插件重启导致的调用失败，随后解除封锁。
                    let _ = exec.await;
                    if me.release_reentry(&task_id, &run_id) {
                        tracing::info!(task = %task_id, run_id = %run_id, "超时运行的迟到回执已到达，解除重入封锁");
                    }
                });
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

    async fn execute_ticket(&self, ticket: &RunTicket) -> Result<ExecOutput, ExecError> {
        match &ticket.context.kind {
            TaskKind::Builtin { name } => {
                let ctx = BuiltinCtx::from(&ticket.context);
                let name = name.clone();
                let db = Arc::clone(&self.db);
                let web_root = self.web_root.clone();
                let data_dir = self.data_dir.clone();
                let retention = self.trash_retention_days;
                tokio::task::spawn_blocking(move || {
                    run_builtin(&name, &ctx, &db, &web_root, &data_dir, retention)
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
    ///
    /// 与 `drive_run` 同一条重入规则：上一轮超时但未回执时直接拒绝，
    /// 错误串以 `sched.reentry_blocked` 开头，调用方据此返回 409/失败回执。
    pub async fn run_task_now(
        self: &Arc<Self>,
        id: &str,
        actor: AuditActor,
    ) -> Result<Value, String> {
        if let Some(prev_run) = self.reentry_blocked_by(id) {
            return Err(format!(
                "sched.reentry_blocked: 上一次运行 {prev_run} 已超时但插件尚未回执（或进程未重启），拒绝重入"
            ));
        }
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
        let task_id = ticket.context.task_id.to_string();
        let is_plugin = matches!(&ticket.context.kind, TaskKind::Plugin { .. });
        let timeout_secs = {
            let sched = self.scheduler.lock().await;
            sched.get(id).map(|t| t.spec.timeout_secs).unwrap_or(30)
        };
        let dur = Duration::from_secs(timeout_secs.max(1));
        let mut late_receipt = None;
        let outcome: Option<Result<ExecOutput, ExecError>> = if !is_plugin {
            tokio::time::timeout(dur, self.execute_ticket(&ticket))
                .await
                .ok()
        } else {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let exec = {
                let me = Arc::clone(self);
                tokio::spawn(async move {
                    let _ = tx.send(me.execute_ticket(&ticket).await);
                })
            };
            match tokio::time::timeout(dur, rx).await {
                Ok(Ok(result)) => Some(result),
                Ok(Err(_)) => Some(Err(ExecError::new(
                    ErrorClass::ExecutorFailed,
                    ErrorPhase::Call,
                    "执行体异常退出（未返回回执）",
                ))),
                Err(_) => {
                    self.block_reentry(&task_id, &run_id);
                    late_receipt = Some(exec);
                    None
                }
            }
        };
        let record = {
            let mut sched = self.scheduler.lock().await;
            let now = self.clock.now();
            match outcome {
                Some(result) => {
                    sched
                        .finish(&run_id, result, now)
                        .map_err(|e| e.to_string())?
                        .record
                }
                None => sched.timeout(&run_id, now).map_err(|e| e.to_string())?,
            }
        };
        if let Some(exec) = late_receipt {
            let me = Arc::clone(self);
            tokio::spawn(async move {
                let _ = exec.await;
                if me.release_reentry(&task_id, &run_id) {
                    tracing::info!(task = %task_id, run_id = %run_id, "超时运行的迟到回执已到达，解除重入封锁");
                }
            });
        }
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

    /// 级联删除：内核维护路径（删站）清理 `builtin:` 任务时用，绕过内置保护。
    /// 只给「前置对象已经删除」的级联场景，不暴露给插件与非内核调用方。
    pub async fn delete_task_cascade(&self, id: &str) -> Result<(), String> {
        let now = self.clock.now();
        let mut sched = self.scheduler.lock().await;
        sched.delete_cascade(id, now).map_err(|e| e.to_string())
    }

    // -- 插件能力面 ---------------------------------------------------------

    /// `scheduler` 能力：插件（cron 管理 UI）读写任务的唯一入口。
    ///
    /// 边界：
    /// - `scheduler:read/manage/run` 只作用于**自己来源**的任务；
    /// - `scheduler:admin` 是内核指定的「任务管理 UI」权限（cron 专属），可看/管全量；
    /// - `task.register` 的 kind/source/id 由内核按调用方身份注入，插件无法伪造。
    pub async fn capability(
        self: &Arc<Self>,
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
                // `kind` 只允许内核补种（manifest 改 method）使用；插件不得改派自己的执行体。
                if patch.kind.is_some() {
                    return Err("patch.kind 不可修改（契约 §10.2）".into());
                }
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
                // 内置任务先于归属校验拒绝：不论调用方是谁，都不能删内核种入的任务。
                ensure_not_builtin(&id)?;
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
    if sched.get(DB_VACUUM_TASK_ID).is_none() {
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
    }
    if sched.get(TRASH_PRUNE_TASK_ID).is_none() {
        let mut spec = TaskSpec::new(
            TRASH_PRUNE_TASK_ID,
            TaskKind::builtin(BUILTIN_TRASH_PRUNE).map_err(|e| e.to_string())?,
            TaskSource::System,
            Schedule::cron("30 4 * * *"),
        )
        .map_err(|e| e.to_string())?;
        spec.payload = json!({});
        spec.created_by = "system".into();
        spec.created_at = clock.now();
        spec.updated_at = spec.created_at;
        sched
            .register(spec, clock.now(), clock)
            .map_err(|e| e.to_string())?;
    }
    if sched.get(AUDIT_PRUNE_TASK_ID).is_none() {
        let mut spec = TaskSpec::new(
            AUDIT_PRUNE_TASK_ID,
            TaskKind::builtin(BUILTIN_AUDIT_PRUNE).map_err(|e| e.to_string())?,
            TaskSource::System,
            Schedule::cron("0 5 * * *"),
        )
        .map_err(|e| e.to_string())?;
        spec.payload = json!({});
        spec.created_by = "system".into();
        spec.created_at = clock.now();
        spec.updated_at = spec.created_at;
        sched
            .register(spec, clock.now(), clock)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// manifest 种入的任务用这个 actor 标记，便于区分「内核按声明种的」与
/// 「插件自己通过能力面注册的」——升级后声明消失时只回收前者。
const MANIFEST_TASK_ACTOR: &str = "system:manifest";

/// 按已安装插件的 manifest `[[task]]` 补种/刷新任务，并回收孤儿任务。
///
/// - 缺失项：按声明注册。
/// - 已存在：比对声明与库内 spec，有差异就 `update`（调度/执行方法/payload/超时/缓冲），
///   **保留**用户侧的启用状态、`created_by`、`created_at` 与运行历史——不能用「删了重建」，
///   那会丢历史并把管理员停用的任务重新启用。
/// - 回收两类：插件已卸载（含 cron 等代建的 `source=plugin:<id>` 任务），
///   以及 manifest 已不再声明的内核种子任务。
fn seed_manifest_tasks(sched: &mut Scheduler<SqliteTaskStore>, db: &Db, clock: &SystemClock) {
    let now = clock.now();
    let installed = match db.list_plugins() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(error = %e, "读取插件列表失败，跳过 manifest 任务种入");
            return;
        }
    };
    let mut expected: Vec<String> = Vec::new();
    for rec in &installed {
        let manifest = match skiff_plugin::install::manifest_of(rec) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(plugin = %rec.id, error = %e, "manifest 不可解析，跳过任务种入");
                continue;
            }
        };
        if !manifest.cron.is_empty() {
            tracing::warn!(
                plugin = %rec.id,
                "manifest `cron` 已停用（改用 [[task]]），本次忽略"
            );
        }
        for t in &manifest.task {
            let id = format!("plugin:{}:{}", rec.id, t.key);
            expected.push(id.clone());
            let declared = match task_spec_from_manifest(rec, t, now) {
                Ok(spec) => spec,
                Err(e) => {
                    tracing::warn!(plugin = %rec.id, task = %id, error = %e, "manifest 任务声明不可用");
                    continue;
                }
            };
            let existing = sched.get(&id).map(|task| task.spec.clone());
            let Some(existing) = existing else {
                match sched.register(declared, now, clock) {
                    Ok(_) => {
                        tracing::info!(plugin = %rec.id, task = %id, "按 manifest 种入内核任务");
                    }
                    Err(e) => {
                        tracing::warn!(plugin = %rec.id, task = %id, error = %e, "manifest 任务种入失败");
                    }
                }
                continue;
            };
            // 升级后声明可能变了：刷新内核侧 spec，但不动启停状态与历史。
            let (patch, fields) = manifest_task_patch(&existing, &declared);
            if fields.is_empty() {
                continue;
            }
            match sched.update(&id, &patch, now, clock) {
                Ok(_) => {
                    tracing::info!(
                        plugin = %rec.id,
                        task = %id,
                        changed = %fields.join(","),
                        "按 manifest 刷新内核任务（保留启停状态与历史）"
                    );
                }
                Err(e) => {
                    tracing::warn!(plugin = %rec.id, task = %id, error = %e, "manifest 任务刷新失败");
                }
            }
        }
    }

    for task in sched.list(&TaskFilter::default()) {
        let Some(pid) = task.spec.plugin_id() else {
            continue;
        };
        let installed_here = installed.iter().any(|r| r.id == pid);
        let manifest_owned = task.spec.created_by == MANIFEST_TASK_ACTOR;
        let declared = expected
            .iter()
            .any(|id| id.as_str() == task.spec.id.as_str());
        if !installed_here || (manifest_owned && !declared) {
            if let Err(e) = sched.delete(task.spec.id.as_str(), now) {
                tracing::warn!(task = %task.spec.id, error = %e, "孤儿插件任务回收失败");
            }
        }
    }
}

/// 比对库内 spec 与 manifest 声明，只把「声明能决定」的字段放进补丁。
///
/// 刻意不含 `enabled` / `created_by` / `created_at` / `last_run` / `state`：
/// 这些属于用户与运行态，升级刷新不得重置。返回变更字段名仅用于日志。
fn manifest_task_patch(existing: &TaskSpec, declared: &TaskSpec) -> (TaskPatch, Vec<&'static str>) {
    let mut patch = TaskPatch::default();
    let mut fields = Vec::new();
    if existing.kind != declared.kind {
        patch.kind = Some(declared.kind.clone());
        fields.push("kind");
    }
    if existing.schedule != declared.schedule {
        patch.schedule = Some(declared.schedule.clone());
        fields.push("schedule");
    }
    if existing.payload != declared.payload {
        patch.payload = Some(declared.payload.clone());
        fields.push("payload");
    }
    if existing.timeout_secs != declared.timeout_secs {
        patch.timeout_secs = Some(declared.timeout_secs);
        fields.push("timeout_secs");
    }
    match (declared.buffer.as_ref(), existing.buffer.as_ref()) {
        (Some(d), Some(e)) if d != e => {
            patch.buffer = Some(d.clone());
            fields.push("buffer");
        }
        (Some(d), None) => {
            patch.buffer = Some(d.clone());
            fields.push("buffer");
        }
        (None, Some(_)) => {
            patch.clear_buffer = true;
            fields.push("buffer");
        }
        _ => {}
    }
    (patch, fields)
}

/// manifest 声明 → `TaskSpec`。这里做二次校验：manifest 只保形式合法，
/// 调度器限额（超时/缓冲）由 `TaskSpec::validate` 在 `register` 时兜底。
fn task_spec_from_manifest(
    rec: &PluginRecord,
    t: &skiff_pkgfmt::manifest::ManifestTask,
    now: i64,
) -> Result<TaskSpec, TaskError> {
    let kind = TaskKind::plugin(&rec.id, &t.method)?;
    let schedule = match (&t.expr, t.interval_secs) {
        (Some(expr), None) => Schedule::cron(expr.clone()),
        (None, Some(secs)) => Schedule::interval(secs),
        // pkgfmt 已经在解析期拦住这两种情况；这里保留明确错误而不是 panic。
        _ => {
            return Err(TaskError::InvalidSchedule(
                "manifest 任务必须且只能声明 expr 或 interval_secs".into(),
            ))
        }
    };
    let id = format!("plugin:{}:{}", rec.id, t.key);
    let mut spec = TaskSpec::new(id, kind, TaskSource::Plugin(rec.id.clone()), schedule)?;
    spec.enabled = t.enabled;
    spec.payload = json!({ "key": t.key });
    spec.timeout_secs = t.timeout_secs.unwrap_or(skiff_sched::DEFAULT_TIMEOUT_SECS);
    spec.created_by = MANIFEST_TASK_ACTOR.into();
    spec.created_at = now;
    spec.updated_at = now;
    if let Some(method) = &t.sample_method {
        spec.buffer = Some(BufferConfig {
            sample_method: Some(method.clone()),
            sample_interval_secs: t.sample_interval_secs.unwrap_or(30),
            capacity_bytes: t
                .capacity_bytes
                .unwrap_or(skiff_sched::DEFAULT_CAPACITY_BYTES),
            max_batch_bytes: t
                .max_batch_bytes
                .unwrap_or(skiff_sched::DEFAULT_MAX_BATCH_BYTES),
            ..BufferConfig::default()
        });
    }
    Ok(spec)
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
    db: &Arc<Db>,
    web_root: &Path,
    data_dir: &Path,
    trash_retention_days: u32,
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
        BUILTIN_TRASH_PRUNE => trash_prune(ctx, db, web_root, data_dir, trash_retention_days),
        BUILTIN_AUDIT_PRUNE => audit_prune(db),
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
    let started = Instant::now();
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
    tracing::info!(
        lane = "maintenance",
        op = "site_backup",
        duration_ms = started.elapsed().as_millis() as u64,
        bytes,
        domain,
        "维护任务完成"
    );
    Ok(ExecOutput {
        verdict: None,
        detail: Some(path.display().to_string()),
        output_bytes: bytes,
    })
}

fn db_vacuum(ctx: &BuiltinCtx, db: &Db, data_dir: &Path) -> Result<ExecOutput, ExecError> {
    let started = Instant::now();
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
    tracing::info!(
        lane = "maintenance",
        op = "db_backup",
        duration_ms = started.elapsed().as_millis() as u64,
        bytes,
        "维护任务完成"
    );
    Ok(ExecOutput {
        verdict: None,
        detail: Some(path.display().to_string()),
        output_bytes: bytes,
    })
}

fn trash_prune(
    ctx: &BuiltinCtx,
    db: &Arc<Db>,
    web_root: &Path,
    data_dir: &Path,
    trash_retention_days: u32,
) -> Result<ExecOutput, ExecError> {
    let started = Instant::now();
    let _ = ctx;
    // 任务服务与 Web 页面共用同一个 `data/trash` 目录与 `trash_entries` 表。
    let admin = crate::admin_fs::AdminFs::with_options(
        data_dir.to_path_buf(),
        web_root.to_path_buf(),
        Arc::clone(db),
        trash_retention_days,
    );
    let bytes = admin
        .trash_prune()
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.message()))?;
    tracing::info!(
        lane = "maintenance",
        op = "trash_prune",
        duration_ms = started.elapsed().as_millis() as u64,
        bytes,
        "维护任务完成"
    );
    Ok(ExecOutput {
        verdict: None,
        detail: Some(format!("released_bytes={bytes}")),
        output_bytes: bytes as usize,
    })
}

/// 审计保留清理：按 90 天时间窗删除。删多少条写进任务详情（可在任务运行记录里追溯），
/// 失败则整次运行记失败，由调度器留痕。
fn audit_prune(db: &Db) -> Result<ExecOutput, ExecError> {
    let started = Instant::now();
    let cutoff = audit_cutoff(Local::now().timestamp());
    let deleted = db
        .purge_audit_before(cutoff)
        .map_err(|e| ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, e.to_string()))?;
    tracing::info!(
        lane = "maintenance",
        op = "audit_prune",
        duration_ms = started.elapsed().as_millis() as u64,
        deleted,
        cutoff,
        retention_days = AUDIT_RETENTION_DAYS,
        "审计保留清理完成"
    );
    Ok(ExecOutput {
        verdict: None,
        detail: Some(format!("deleted={deleted} cutoff={cutoff}")),
        output_bytes: 0,
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

/// 内置任务（`builtin:`）删除保护。服务层与插件能力面共用同一口径：
/// 允许停用/立即执行，不允许删除（由内核 seed 重建，删了等于能力缺席）。
fn ensure_not_builtin(id: &str) -> Result<(), String> {
    if id.starts_with("builtin:") {
        return Err("内置任务不可删除（只能停用）".to_string());
    }
    Ok(())
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
    use skiff_store::{NewSite, TaskRunRow};

    fn service(dir: &tempfile::TempDir) -> Arc<TaskService> {
        let db = Arc::new(Db::open(dir.path().join("skiff.db")).unwrap());
        db.create_site(&NewSite {
            domain: "example.com".into(),
            root_dir: dir.path().join("www/example.com").display().to_string(),
            site_type: skiff_store::TYPE_STATIC.into(),
            runtime_id: None,
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
            30,
        )
        .unwrap()
    }

    fn manifest_with_task(
        version: &str,
        expr: &str,
        method: &str,
        sample_method: &str,
        sample_secs: u64,
        timeout: u64,
    ) -> String {
        let key_id = "0".repeat(64);
        format!(
            r#"id = "com.example.demo"
name = "Demo"
version = "{version}"
key_id = "{key_id}"
api = "1.0"
run = "on-demand"
requires = ["fs"]

[[task]]
key = "sample"
method = "{method}"
expr = "{expr}"
sample_method = "{sample_method}"
sample_interval_secs = {sample_secs}
timeout_secs = {timeout}
"#
        )
    }

    fn plugin_record(version: &str, manifest: &str) -> PluginRecord {
        PluginRecord {
            id: "com.example.demo".into(),
            name: "Demo".into(),
            version: version.into(),
            api: KERNEL_API.into(),
            key_id: "0".repeat(64),
            manifest: manifest.into(),
            enabled: true,
            status: "installed".into(),
            status_detail: String::new(),
            active_version: version.into(),
            installed_at: 0,
            grants: vec![],
        }
    }

    /// 升级刷新：manifest 改了调度/方法/采样/超时 → 库内 spec 跟着更新；
    /// 但管理员停用的状态、created_by 与运行历史必须原样保留（不是删了重建）。
    #[tokio::test]
    async fn manifest_task_refresh_updates_spec_and_keeps_user_state() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let db = Arc::clone(svc.db_handle());
        let task_id = "plugin:com.example.demo:sample";

        db.upsert_plugin(&plugin_record(
            "0.1.0",
            &manifest_with_task("0.1.0", "*/5 * * * *", "execute", "sample", 60, 30),
        ))
        .unwrap();
        svc.sync_manifest_tasks().await;

        let created_at = {
            let sched = svc.scheduler.lock().await;
            let task = sched.get(task_id).expect("任务应被种入");
            assert_eq!(task.spec.schedule, Schedule::cron("*/5 * * * *"));
            assert_eq!(task.spec.timeout_secs, 30);
            assert_eq!(task.spec.created_by, MANIFEST_TASK_ACTOR);
            assert!(task.spec.enabled);
            task.spec.created_at
        };

        // 管理员停用 + 一条运行历史。
        {
            let mut sched = svc.scheduler.lock().await;
            let now = svc.clock.now();
            sched
                .update(
                    task_id,
                    &TaskPatch {
                        enabled: Some(false),
                        ..Default::default()
                    },
                    now,
                    &svc.clock,
                )
                .unwrap();
        }
        db.append_task_run(&TaskRunRow {
            id: "run-1".into(),
            task_id: task_id.into(),
            trigger: "schedule".into(),
            attempt: 1,
            started_at: 100,
            finished_at: 101,
            result: "ok".into(),
            error_class: None,
            error_detail: None,
            duration_ms: 1000,
            request_id: None,
            consumed_upto: None,
            output_bytes: 0,
            missed_count: 0,
        })
        .unwrap();

        // 升级：调度、执行方法、采样方法/间隔、超时全变。
        db.upsert_plugin(&plugin_record(
            "0.2.0",
            &manifest_with_task("0.2.0", "*/10 * * * *", "run", "probe", 120, 45),
        ))
        .unwrap();
        svc.sync_manifest_tasks().await;

        let sched = svc.scheduler.lock().await;
        let task = sched.get(task_id).expect("刷新不应删任务");
        assert_eq!(task.spec.schedule, Schedule::cron("*/10 * * * *"));
        assert_eq!(task.spec.timeout_secs, 45);
        assert_eq!(
            task.spec.kind,
            TaskKind::plugin("com.example.demo", "run").unwrap()
        );
        let buf = task.spec.buffer.as_ref().expect("缓冲声明应保留");
        assert_eq!(buf.sample_method.as_deref(), Some("probe"));
        assert_eq!(buf.sample_interval_secs, 120);
        // 用户态与历史不受刷新影响。
        assert!(!task.spec.enabled, "管理员停用的状态不能被刷新重新启用");
        assert_eq!(task.spec.created_by, MANIFEST_TASK_ACTOR);
        assert_eq!(task.spec.created_at, created_at);
        drop(sched);

        let row = db.scheduled_task(task_id).unwrap().expect("任务行仍在");
        assert!(!row.enabled);
        assert_eq!(db.task_runs(task_id, 10).unwrap().len(), 1);
    }

    /// 升级后 manifest 不再声明任务 → 内核种子任务被回收。
    #[tokio::test]
    async fn manifest_task_removed_on_upgrade_is_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let db = Arc::clone(svc.db_handle());
        let task_id = "plugin:com.example.demo:sample";

        db.upsert_plugin(&plugin_record(
            "0.1.0",
            &manifest_with_task("0.1.0", "*/5 * * * *", "execute", "sample", 60, 30),
        ))
        .unwrap();
        svc.sync_manifest_tasks().await;
        assert!(svc.scheduler.lock().await.get(task_id).is_some());

        db.upsert_plugin(&plugin_record(
            "0.2.0",
            r#"id = "com.example.demo"
name = "Demo"
version = "0.2.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.0"
run = "on-demand"
requires = ["fs"]
"#,
        ))
        .unwrap();
        svc.sync_manifest_tasks().await;

        assert!(svc.scheduler.lock().await.get(task_id).is_none());
        assert!(db.scheduled_task(task_id).unwrap().is_none());
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
            30,
        )
        .unwrap();
        let sched = reloaded.scheduler.blocking_lock();
        assert!(sched.get(DB_VACUUM_TASK_ID).is_some());
        assert!(sched.get(TRASH_PRUNE_TASK_ID).is_some());
        assert!(sched.get(AUDIT_PRUNE_TASK_ID).is_some());
    }

    /// 保留窗口是 90 天整：`cutoff = now - 90d`；删除条件是严格小于，
    /// 所以恰好 90 天前的记录仍在窗口内（边界由 store 的 `ts < cutoff` 保证）。
    #[test]
    fn audit_retention_window_is_ninety_days() {
        let now = 1_800_000_000_i64;
        let cutoff = audit_cutoff(now);
        assert_eq!(cutoff, now - 90 * 24 * 3600);
        // 超长的时间戳不会因为乘法溢出而回绕。
        assert_eq!(audit_cutoff(i64::MIN / 2), i64::MIN / 2 - 90 * 24 * 3600);
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

    #[tokio::test]
    async fn builtin_task_delete_is_rejected_but_disable_and_run_are_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let ui = "com.skiff.cron";
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
                    Grant::perm("scheduler:run"),
                    Grant::perm("scheduler:admin"),
                ],
            })
            .unwrap();

        // 服务层（Web 删除入口）：拒绝且行仍在。
        let err = svc.delete_task(DB_VACUUM_TASK_ID).await.unwrap_err();
        assert!(err.contains("内置任务不可删除"), "{err}");
        assert!(svc
            .db_handle()
            .scheduled_task(DB_VACUUM_TASK_ID)
            .unwrap()
            .is_some());

        // 插件能力面（含 scheduler:admin 的 UI 也不能删）。
        let err = svc
            .capability(ui, "task.delete", json!({ "id": DB_VACUUM_TASK_ID }))
            .await
            .unwrap_err();
        assert!(err.contains("内置任务不可删除"), "{err}");
        assert!(svc
            .db_handle()
            .scheduled_task(DB_VACUUM_TASK_ID)
            .unwrap()
            .is_some());

        // 普通任务仍可删。
        svc.capability(
            ui,
            "task.register",
            json!({
                "target": "com.skiff.testkit",
                "local_key": "probe",
                "method": "probe",
                "expr": "0 5 * * *",
                "payload": {},
            }),
        )
        .await
        .unwrap();
        let normal = format!("plugin:{ui}:probe");
        svc.capability(ui, "task.delete", json!({ "id": normal }))
            .await
            .unwrap();
        assert!(svc.db_handle().scheduled_task(&normal).unwrap().is_none());

        // 内置任务可停用/启用（结论：可停用）。
        let off = svc
            .set_enabled_task(DB_VACUUM_TASK_ID, false)
            .await
            .unwrap();
        assert_eq!(off["enabled"], false);
        let on = svc.set_enabled_task(DB_VACUUM_TASK_ID, true).await.unwrap();
        assert_eq!(on["enabled"], true);

        // 内置任务可立即执行（结论：可 run-now）。
        let run = svc
            .run_task_now(DB_VACUUM_TASK_ID, AuditActor::Admin("admin".into()))
            .await
            .unwrap();
        assert_eq!(run["trigger"], "manual");
        assert_eq!(run["result"], "ok");
    }

    /// 重入状态机：上一轮超时结算（单飞已释放、插件尚未回执）→ 同一任务的新一轮
    /// 必须立刻以 `reentry_blocked` 拒绝，而不是再叠加一次执行；回执到达
    /// （或插件重启令调用报错）后解除封锁，任务恢复可执行。
    #[tokio::test]
    async fn timed_out_run_blocks_reentry_until_receipt_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let db = Arc::clone(svc.db_handle());
        let task_id = "plugin:com.example.demo:sample";
        db.upsert_plugin(&plugin_record(
            "0.1.0",
            &manifest_with_task("0.1.0", "*/5 * * * *", "execute", "sample", 60, 30),
        ))
        .unwrap();
        svc.sync_manifest_tasks().await;

        // 1) 取一张票，并模拟「已超时结算」：上一轮 run 仍在等插件回执。
        let ticket = {
            let mut sched = svc.scheduler.lock().await;
            sched
                .run_now(
                    task_id,
                    AuditActor::Admin("admin".into()),
                    svc.clock.now(),
                    &Gate {
                        db: Arc::clone(&svc.db),
                    },
                )
                .unwrap()
        };
        let prev_run = "run-prev-timeout".to_string();
        svc.block_reentry(task_id, &prev_run);

        // 2) 新一轮：立即结算为失败并带结构化错误码，未真正调用插件。
        let run_id = ticket.context.run_id.clone();
        Arc::clone(&svc).drive_run(ticket, 30).await;
        let runs = db.task_runs(task_id, 10).unwrap();
        let rejected = runs
            .iter()
            .find(|r| r.id == run_id)
            .expect("重入被拒也要留痕");
        assert_eq!(rejected.result, "fail");
        assert_eq!(rejected.error_class.as_deref(), Some("reentry_blocked"));
        assert!(
            rejected
                .error_detail
                .as_deref()
                .unwrap_or("")
                .contains(&prev_run),
            "{:?}",
            rejected.error_detail
        );

        // 3) 回执到达 → 解除封锁，同一任务恢复可执行（此处插件不可拉起，只验证不再被重入拒绝）。
        assert!(svc.release_reentry(task_id, &prev_run));
        let out = svc
            .run_task_now(task_id, AuditActor::Admin("admin".into()))
            .await
            .unwrap();
        assert_ne!(out["error_class"], "reentry_blocked", "{out}");
    }

    /// 串行不变量：落盘移出调度锁后，并发的两轮 tick 仍只能为同一任务出一张票。
    ///
    /// 票据与 `Running`/`Queued` 状态在 `sched.tick` 内、锁下产出；锁外只是把
    /// `RunRecord` 物化写库，因此第二个 tick 看到的是「已取走」，不会重复执行。
    #[tokio::test]
    async fn concurrent_ticks_issue_at_most_one_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let svc = service(&dir);
        let task_id = "builtin:test-tick-once";
        {
            let mut sched = svc.scheduler.lock().await;
            let mut spec = TaskSpec::new(
                task_id,
                TaskKind::builtin(BUILTIN_AUDIT_PRUNE).unwrap(),
                // source 用 Site 而不是 System：内置任务按 `(kind, source)` 去重，
                // System + audit-prune 会命中启动时种下的那条并发起「更新」。
                TaskSource::Site("tick-once-test".into()),
                Schedule::interval(5),
            )
            .unwrap();
            spec.payload = json!({});
            spec.created_by = "system".into();
            spec.created_at = svc.clock.now();
            spec.updated_at = spec.created_at;
            sched.register(spec, svc.clock.now(), &svc.clock).unwrap();
        }

        // interval 的最小粒度是 5 s，首次触发落在下一个整 5 秒边界。
        tokio::time::sleep(Duration::from_millis(5200)).await;
        let a = Arc::clone(&svc);
        let b = Arc::clone(&svc);
        let (ra, rb) = tokio::join!(
            tokio::spawn(async move { a.tick_once().await }),
            tokio::spawn(async move { b.tick_once().await })
        );
        ra.unwrap();
        rb.unwrap();

        // 等执行体结算写回（内置任务在 spawn_blocking 里跑，audit-prune 很快）。
        tokio::time::sleep(Duration::from_millis(600)).await;
        let runs = svc.db_handle().task_runs(task_id, 10).unwrap();
        assert_eq!(
            runs.len(),
            1,
            "并发 tick 只能出一张票，实际 {} 条：{:?}",
            runs.len(),
            runs.iter().map(|r| (&r.id, &r.result)).collect::<Vec<_>>()
        );
    }
}
