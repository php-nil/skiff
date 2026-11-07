//! 调度循环（契约 §5）与内存计量（契约 §4.2）。
//!
//! 设计要点：
//!
//! - **tick 只产票，不执行**：`tick` 返回 [`RunTicket`] / [`SampleTicket`]，上层跑完调用
//!   [`Scheduler::finish`] / [`Scheduler::apply_sample`] 结算。这样同步/异步执行体都能接，
//!   本 crate 也不用引 tokio。
//! - **单飞**：每个任务同一时刻最多一个在飞批次；`concurrency=skip` 直接记 skipped_busy，
//!   `queue_one` 只保留一个待执行槽位。
//! - **超时不杀**：超时只释放单飞锁、记 timeout 并进入 60s 冷却；取消是协作式的
//!   （`RunContext::is_cancelled`），由执行体自己检查。
//! - **全局计量**：先按单任务容量、再按插件配额、最后按全局上限三层校验；全局超限先丢
//!   `drop_oldest` 任务的最旧点，仍不够才拒收，并暂停「插件来源 + reject」的采样直到回落。

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::audit::{AuditAction, AuditActor, AuditEvent, AuditOutcome};
use crate::buffer::{
    AggregateRule, Buffer, BufferConfig, BufferSignal, CommitOutcome, OverflowPolicy, PushOutcome,
    PushReport, RejectReason, SnapshotMeta, POINT_MAX_BYTES,
};
use crate::clock::Clock;
use crate::error::TaskError;
use crate::exec::{
    Batch, ErrorClass, ErrorPhase, ExecError, ExecOutput, RunContext, RunRecord, RunResult,
    RunTicket, SampleContext, SampleTicket, Trigger, Verdict,
};
use crate::model::{
    BlockedReason, Concurrency, LastRun, LifecycleState, MissedPolicy, Schedule, Task, TaskId,
    TaskKind, TaskPatch, TaskState, CLOCK_SKEW_PAUSE_SECS,
};
use crate::schedule::ParsedSchedule;
use crate::store::{MemoryTaskStore, TaskStore};

pub const GLOBAL_SOFT_LIMIT_BYTES: usize = 64 * 1024 * 1024;
pub const GLOBAL_LIMIT_BYTES: usize = 128 * 1024 * 1024;
pub const PLUGIN_LIMIT_BYTES: usize = 16 * 1024 * 1024;
pub const PLUGIN_HARD_LIMIT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// 全局软上限：超过即进入降级（暂停插件来源 + reject 策略的采样）。
    pub global_soft_limit_bytes: usize,
    /// 全局硬上限：超过先腾挪 drop_oldest，再拒收新点。
    pub global_limit_bytes: usize,
    /// 单插件配额。
    pub plugin_limit_bytes: usize,
    /// 单插件配置上限（配置校验用）。
    pub plugin_hard_limit_bytes: usize,
    pub cooldown_secs: i64,
    pub max_catch_up: u64,
    pub max_missed_scan: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            global_soft_limit_bytes: GLOBAL_SOFT_LIMIT_BYTES,
            global_limit_bytes: GLOBAL_LIMIT_BYTES,
            plugin_limit_bytes: PLUGIN_LIMIT_BYTES,
            plugin_hard_limit_bytes: PLUGIN_HARD_LIMIT_BYTES,
            cooldown_secs: 60,
            max_catch_up: 10,
            max_missed_scan: 10_000,
        }
    }
}

impl SchedulerConfig {
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.global_soft_limit_bytes == 0
            || self.global_soft_limit_bytes > self.global_limit_bytes
        {
            return Err(TaskError::InvalidBuffer(
                "全局软上限必须在 (0, 硬上限] 之间".to_string(),
            ));
        }
        if self.plugin_limit_bytes == 0 || self.plugin_limit_bytes > self.plugin_hard_limit_bytes {
            return Err(TaskError::InvalidBuffer(
                "插件配额必须在 (0, 插件上限] 之间".to_string(),
            ));
        }
        if self.cooldown_secs < 1 {
            return Err(TaskError::InvalidBuffer(
                "cooldown_secs 必须至少 1s".to_string(),
            ));
        }
        if self.max_catch_up == 0 {
            return Err(TaskError::InvalidBuffer(
                "max_catch_up 必须至少 1".to_string(),
            ));
        }
        if self.max_missed_scan == 0 {
            return Err(TaskError::InvalidBuffer(
                "max_missed_scan 必须至少 1".to_string(),
            ));
        }
        Ok(())
    }
}

/// 插件是否可执行。生产实现由 `skiff-plugin` 提供。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Ready,
    Disabled,
    NotInstalled,
    ApiMismatch,
}

pub trait PluginGate {
    fn plugin_status(&self, plugin_id: &str) -> PluginStatus;
}

/// 只用于内置任务或测试：所有插件都视为就绪。
#[derive(Debug, Clone, Copy, Default)]
pub struct AllPluginsReady;

impl PluginGate for AllPluginsReady {
    fn plugin_status(&self, _plugin_id: &str) -> PluginStatus {
        PluginStatus::Ready
    }
}

#[derive(Debug, Default)]
pub struct TickReport {
    pub runs: Vec<RunTicket>,
    pub samples: Vec<SampleTicket>,
    pub records: Vec<RunRecord>,
    pub audits: Vec<AuditEvent>,
}

#[derive(Debug)]
pub struct FinishOutcome {
    pub record: RunRecord,
    pub next: Option<RunTicket>,
    pub audits: Vec<AuditEvent>,
}

/// 更新前需要对比的旧配置，用来决定哪些运行时状态要重建。
struct OldSpec {
    schedule: Schedule,
    buffer: Option<BufferConfig>,
    enabled: bool,
}

struct ActiveRun {
    run_id: String,
    started_at: i64,
    attempt: u32,
    trigger: Trigger,
    snapshot: SnapshotMeta,
    missed_count: u64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct TaskRuntime {
    state: TaskState,
    last_observed_at: i64,
    skew_reported: bool,
    clock_paused: bool,
    parsed: Option<ParsedSchedule>,
    next_boundary: Option<i64>,
    next_due: Option<i64>,
    next_sample_at: Option<i64>,
    sample_in_flight: bool,
    sample_failures: u64,
    active: Option<ActiveRun>,
    pending_one: bool,
    cooldown_until: i64,
    retry_at: Option<i64>,
    attempt: u32,
    catch_up_remaining: u64,
    run_seq: u64,
    buffer: Option<Buffer>,
}

pub struct Scheduler<S: TaskStore = MemoryTaskStore> {
    store: S,
    cfg: SchedulerConfig,
    runtime: HashMap<String, TaskRuntime>,
    global_bytes: usize,
    plugin_bytes: HashMap<String, usize>,
    degraded: bool,
}

impl Scheduler<MemoryTaskStore> {
    pub fn in_memory(cfg: SchedulerConfig, clock: &dyn Clock) -> Result<Self, TaskError> {
        Self::new(MemoryTaskStore::new(), cfg, clock)
    }
}

impl<S: TaskStore> Scheduler<S> {
    pub fn new(store: S, cfg: SchedulerConfig, clock: &dyn Clock) -> Result<Self, TaskError> {
        cfg.validate()?;
        let now = clock.now();
        let mut runtime = HashMap::new();
        let tasks = store.list_tasks();
        for task in tasks {
            let rt = build_runtime(&task, now, clock);
            runtime.insert(task.spec.id.to_string(), rt);
        }
        let mut scheduler = Self {
            store,
            cfg,
            runtime,
            global_bytes: 0,
            plugin_bytes: HashMap::new(),
            degraded: false,
        };
        // 缓冲只活在内存里：从持久化层装载后必然为空，按契约记一次审计。
        let buffered: Vec<String> = scheduler
            .store
            .list_tasks()
            .into_iter()
            .filter(|t| t.spec.buffer.is_some())
            .map(|t| t.spec.id.to_string())
            .collect();
        for id in buffered {
            let _ = scheduler.store.append_audit(task_event(
                now,
                AuditAction::BufferLostOnRestart,
                &id,
                AuditOutcome::Skipped,
                Some("内存缓冲在进程重启后已清空".to_string()),
            ));
        }
        Ok(scheduler)
    }

    pub fn config(&self) -> &SchedulerConfig {
        &self.cfg
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn list(&self, filter: &crate::model::TaskFilter) -> Vec<Task> {
        self.store
            .list_tasks()
            .into_iter()
            .filter(|t| filter.matches(t))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.store.get_task(id)
    }

    pub fn history(&self, id: &str, limit: usize) -> Vec<RunRecord> {
        self.store.list_runs(id, limit)
    }

    pub fn audits(&self, limit: usize) -> Vec<AuditEvent> {
        self.store.list_audits(limit)
    }

    pub fn task_state(&self, id: &str) -> Option<TaskState> {
        self.runtime.get(id).map(|rt| rt.state)
    }

    /// 下一次计划触发时间（本地时间戳）；停用/不可解析/未排程时为 `None`。
    pub fn next_run_at(&self, id: &str) -> Option<i64> {
        self.runtime.get(id).and_then(|rt| rt.next_due)
    }

    pub fn sample_failures(&self, id: &str) -> u64 {
        self.runtime
            .get(id)
            .map(|rt| rt.sample_failures)
            .unwrap_or(0)
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub fn global_bytes(&self) -> usize {
        self.global_bytes
    }

    pub fn plugin_bytes(&self, plugin_id: &str) -> usize {
        self.plugin_bytes.get(plugin_id).copied().unwrap_or(0)
    }

    pub fn buffer_stat(&self, id: &str) -> Option<crate::buffer::BufferStat> {
        self.runtime
            .get(id)
            .and_then(|rt| rt.buffer.as_ref().map(Buffer::stat))
    }

    /// 注册或更新任务：内置任务按 `kind + source` 幂等（重复注册按更新处理），
    /// 插件任务按 `id`（`plugin:<id>:<local_key>`）幂等。
    pub fn register(
        &mut self,
        spec: crate::model::TaskSpec,
        now: i64,
        clock: &dyn Clock,
    ) -> Result<Task, TaskError> {
        spec.validate()?;
        // 幂等键：内置任务按 `(kind, source)`（同一站点只能有一条 site-backup）；
        // 插件任务按 `id`（`plugin:<id>:<local_key>` 已编码唯一键），否则同一插件
        // 对同一目标的多条任务会互相覆盖。
        let existing = if spec.kind.is_builtin() {
            self.store
                .list_tasks()
                .into_iter()
                .find(|t| t.spec.kind == spec.kind && t.spec.source == spec.source)
                .map(|t| t.spec.id.to_string())
        } else {
            self.store
                .get_task(spec.id.as_str())
                .map(|t| t.spec.id.to_string())
        };

        if let Some(existing_id) = existing {
            let mut task = self
                .store
                .get_task(&existing_id)
                .ok_or_else(|| TaskError::NotFound(existing_id.clone()))?;
            let old_schedule = task.spec.schedule.clone();
            let old_buffer = task.spec.buffer.clone();
            let old_enabled = task.spec.enabled;
            let created_at = task.spec.created_at;
            let created_by = task.spec.created_by.clone();

            task.spec = spec;
            task.spec.id = TaskId::new(existing_id)?;
            task.spec.created_at = created_at;
            task.spec.created_by = created_by;
            task.spec.updated_at = now;
            task.spec.validate()?;
            self.store.put_task(task.clone())?;
            self.reconfigure_runtime(
                &task,
                OldSpec {
                    schedule: old_schedule,
                    buffer: old_buffer,
                    enabled: old_enabled,
                },
                now,
                clock,
                true,
            );
            self.audit_event(
                now,
                AuditAction::TaskUpdate,
                AuditActor::System,
                task.spec.id.as_str(),
                AuditOutcome::Ok,
                Some("重复注册按更新处理".to_string()),
            );
            Ok(task)
        } else {
            if self.store.get_task(spec.id.as_str()).is_some() {
                return Err(TaskError::DuplicateId(spec.id.to_string()));
            }
            let mut task = Task::new(spec);
            task.spec.created_at = now;
            task.spec.updated_at = now;
            self.store.put_task(task.clone())?;
            let rt = build_runtime(&task, now, clock);
            self.runtime.insert(task.spec.id.to_string(), rt);
            self.audit_event(
                now,
                AuditAction::TaskCreate,
                AuditActor::System,
                task.spec.id.as_str(),
                AuditOutcome::Ok,
                None,
            );
            Ok(task)
        }
    }

    pub fn update(
        &mut self,
        id: &str,
        patch: &TaskPatch,
        now: i64,
        clock: &dyn Clock,
    ) -> Result<Task, TaskError> {
        let mut task = self
            .store
            .get_task(id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        let old_schedule = task.spec.schedule.clone();
        let old_buffer = task.spec.buffer.clone();
        let old_enabled = task.spec.enabled;
        if !patch.apply_to(&mut task.spec) {
            return Ok(task);
        }
        task.spec.updated_at = now;
        task.spec.validate()?;
        self.store.put_task(task.clone())?;
        self.reconfigure_runtime(
            &task,
            OldSpec {
                schedule: old_schedule,
                buffer: old_buffer,
                enabled: old_enabled,
            },
            now,
            clock,
            false,
        );
        self.audit_event(
            now,
            AuditAction::TaskUpdate,
            AuditActor::System,
            task.spec.id.as_str(),
            AuditOutcome::Ok,
            None,
        );
        Ok(task)
    }

    pub fn set_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        now: i64,
        clock: &dyn Clock,
    ) -> Result<Task, TaskError> {
        let patch = TaskPatch {
            enabled: Some(enabled),
            ..TaskPatch::default()
        };
        self.update(id, &patch, now, clock)
    }

    pub fn delete(&mut self, id: &str, now: i64) -> Result<(), TaskError> {
        // `builtin:` 由内核种入（如 db-vacuum / site-backup），删掉只会让能力缺席，
        // 重启才由 seed_builtins 重建。UI 隐藏按钮不是安全边界，删除入口必须自己拒。
        if id.starts_with("builtin:") {
            return Err(TaskError::BuiltinProtected(id.to_string()));
        }
        let task = self
            .store
            .delete_task(id)?
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        if let Some(rt) = self.runtime.remove(id) {
            if let Some(active) = &rt.active {
                active.cancel.store(true, Ordering::SeqCst);
            }
            if let Some(buffer) = rt.buffer {
                let bytes = buffer.bytes();
                self.global_bytes = self.global_bytes.saturating_sub(bytes);
                self.modify_plugin_bytes(task.spec.plugin_id(), -(bytes as i64));
            }
        }
        self.audit_event(
            now,
            AuditAction::TaskDelete,
            AuditActor::System,
            id,
            AuditOutcome::Ok,
            None,
        );
        Ok(())
    }

    /// 写一个采样点。三层配额在写前校验，写后更新全局/插件计量。
    pub fn push(&mut self, id: &str, data: Value, now: i64) -> Result<PushReport, TaskError> {
        let task = self
            .store
            .get_task(id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        let buffer_cfg = task
            .spec
            .buffer
            .as_ref()
            .ok_or_else(|| TaskError::Protocol(format!("任务 {id} 未声明缓冲")))?;
        let point_bytes = Buffer::estimate_bytes(&data);
        let plugin = task.spec.plugin_id().map(str::to_string);

        if point_bytes > POINT_MAX_BYTES {
            self.audit_event(
                now,
                AuditAction::BufferPointTooLarge,
                AuditActor::System,
                id,
                AuditOutcome::Fail,
                Some(format!("{point_bytes} B > {POINT_MAX_BYTES} B")),
            );
            return Ok(reject_report(RejectReason::PointTooLarge, point_bytes));
        }
        // 全局降级期间，插件来源 + reject 策略的采样暂停。
        if self.degraded
            && task.spec.source.is_plugin()
            && buffer_cfg.overflow == OverflowPolicy::Reject
        {
            self.audit_event(
                now,
                AuditAction::BufferOverflow,
                AuditActor::System,
                id,
                AuditOutcome::Skipped,
                Some("全局缓冲降级，暂停插件 reject 采样".to_string()),
            );
            return Ok(reject_report(RejectReason::GlobalDegraded, point_bytes));
        }
        if let Some(plugin_id) = &plugin {
            let used = self.plugin_bytes.get(plugin_id).copied().unwrap_or(0);
            if used + point_bytes > self.cfg.plugin_limit_bytes {
                self.audit_event(
                    now,
                    AuditAction::BufferOverflow,
                    AuditActor::System,
                    id,
                    AuditOutcome::Skipped,
                    Some(format!("插件 {plugin_id} 缓冲配额已满")),
                );
                return Ok(reject_report(RejectReason::PluginQuota, point_bytes));
            }
        }
        if self.global_bytes + point_bytes > self.cfg.global_limit_bytes {
            let deficit = self.global_bytes + point_bytes - self.cfg.global_limit_bytes;
            self.evict_for(deficit, id, now);
            if self.global_bytes + point_bytes > self.cfg.global_limit_bytes {
                self.audit_event(
                    now,
                    AuditAction::BufferOverflow,
                    AuditActor::System,
                    id,
                    AuditOutcome::Fail,
                    Some("全局缓冲达到硬上限，拒收新点".to_string()),
                );
                return Ok(reject_report(RejectReason::GlobalDegraded, point_bytes));
            }
        }

        let report = {
            let rt = self
                .runtime
                .get_mut(id)
                .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
            match rt.buffer.as_mut() {
                Some(buffer) => buffer.push(now, data, now),
                None => return Err(TaskError::Protocol(format!("任务 {id} 未声明缓冲"))),
            }
        };

        if let PushOutcome::Accepted { .. } = &report.outcome {
            self.global_bytes = (self.global_bytes as i64 + report.net_bytes).max(0) as usize;
            self.modify_plugin_bytes(plugin.as_deref(), report.net_bytes);
        }
        self.refresh_degraded(now);
        for signal in &report.signals {
            if let Some(event) = signal_event(id, signal, now) {
                let _ = self.store.append_audit(event);
            }
        }
        Ok(report)
    }

    /// 到点扫描：只产出票据与跳过记录，不执行。
    pub fn tick(&mut self, clock: &dyn Clock, gate: &dyn PluginGate) -> TickReport {
        let now = clock.now();
        let cfg = self.cfg;
        let mut ids: Vec<String> = self.runtime.keys().cloned().collect();
        ids.sort();
        let mut report = TickReport::default();
        for id in ids {
            let Some(mut task) = self.store.get_task(&id) else {
                continue;
            };
            let original = task.clone();
            let out = {
                let Some(rt) = self.runtime.get_mut(&id) else {
                    continue;
                };
                process_task(
                    &mut task,
                    rt,
                    &TickCtx {
                        cfg: &cfg,
                        now,
                        clock,
                        gate,
                    },
                )
            };
            if out.task_changed || task != original {
                let _ = self.store.put_task(task);
            }
            for record in out.records {
                let _ = self.store.append_run(record.clone());
                report.records.push(record);
            }
            for event in out.audits {
                let _ = self.store.append_audit(event.clone());
                report.audits.push(event);
            }
            if let Some(run) = out.run {
                report.runs.push(run);
            }
            if let Some(sample) = out.sample {
                report.samples.push(sample);
            }
        }
        report
    }

    pub fn run_now(
        &mut self,
        id: &str,
        actor: AuditActor,
        now: i64,
        gate: &dyn PluginGate,
    ) -> Result<RunTicket, TaskError> {
        let task = self
            .store
            .get_task(id)
            .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
        if !task.spec.enabled {
            self.audit_event(
                now,
                AuditAction::TaskTrigger,
                actor,
                id,
                AuditOutcome::Fail,
                Some("任务已停用".to_string()),
            );
            return Err(TaskError::Disabled(id.to_string()));
        }
        if task.state == LifecycleState::Blocked {
            let reason = task
                .blocked_reason
                .map(BlockedReason::hint)
                .unwrap_or("任务被阻塞");
            self.audit_event(
                now,
                AuditAction::TaskTrigger,
                actor,
                id,
                AuditOutcome::Fail,
                Some(reason.to_string()),
            );
            return Err(TaskError::Disabled(format!("{id}：{reason}")));
        }
        if let TaskKind::Plugin { id: plugin_id, .. } = &task.spec.kind {
            let status = gate.plugin_status(plugin_id);
            if status != PluginStatus::Ready {
                self.audit_event(
                    now,
                    AuditAction::TaskTrigger,
                    actor,
                    id,
                    AuditOutcome::Fail,
                    Some(format!("插件 {plugin_id} 不可用")),
                );
                return Err(TaskError::Disabled(format!("插件 {plugin_id} 不可用")));
            }
        }

        let ticket = {
            let rt = self
                .runtime
                .get_mut(id)
                .ok_or_else(|| TaskError::NotFound(id.to_string()))?;
            if rt.active.is_some() || rt.pending_one {
                None
            } else {
                make_ticket(&task, rt, Trigger::Manual, 1, now, 0)
            }
        };
        match ticket {
            Some(ticket) => {
                self.audit_event(
                    now,
                    AuditAction::TaskTrigger,
                    actor,
                    id,
                    AuditOutcome::Ok,
                    Some(format!("run_id={}", ticket.context.run_id)),
                );
                Ok(ticket)
            }
            None => {
                self.audit_event(
                    now,
                    AuditAction::TaskTrigger,
                    actor,
                    id,
                    AuditOutcome::Skipped,
                    Some("任务仍在运行".to_string()),
                );
                Err(TaskError::Busy(id.to_string()))
            }
        }
    }

    /// 结算一次执行；未知或已结算的 run_id 会得到协议错误（防止重复消费）。
    pub fn finish(
        &mut self,
        run_id: &str,
        result: Result<ExecOutput, ExecError>,
        now: i64,
    ) -> Result<FinishOutcome, TaskError> {
        let id = self
            .runtime
            .iter()
            .find(|(_, rt)| {
                rt.active
                    .as_ref()
                    .map(|a| a.run_id.as_str() == run_id)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
            .ok_or_else(|| TaskError::Protocol(format!("未知或已结算的运行：{run_id}")))?;
        let mut task = self
            .store
            .get_task(&id)
            .ok_or_else(|| TaskError::NotFound(id.clone()))?;
        let cfg = self.cfg;
        let plugin = task.spec.plugin_id().map(str::to_string);

        let completed = {
            let rt = self
                .runtime
                .get_mut(&id)
                .ok_or_else(|| TaskError::NotFound(id.clone()))?;
            let active = rt
                .active
                .take()
                .ok_or_else(|| TaskError::Protocol(format!("运行已结算：{run_id}")))?;
            match result {
                Ok(output) => {
                    let commit = match rt.buffer.as_mut() {
                        Some(buffer) => {
                            buffer.commit(active.snapshot.id, output.verdict.clone(), now)
                        }
                        None => Ok(empty_commit()),
                    };
                    match commit {
                        Ok(outcome) => {
                            let consumed =
                                output.verdict.as_ref().and_then(Verdict::persisted_upto);
                            complete_success(&mut task, rt, active, output, &outcome, consumed, now)
                        }
                        Err(err) => {
                            let error = ExecError::protocol(format!("回执结算失败：{err}"));
                            complete_failure(&mut task, rt, active, error, now, cfg)
                        }
                    }
                }
                Err(error) => complete_failure(&mut task, rt, active, error, now, cfg),
            }
        };

        let _ = self.store.put_task(task);
        if completed.removed_bytes > 0 {
            self.global_bytes = self.global_bytes.saturating_sub(completed.removed_bytes);
            self.modify_plugin_bytes(plugin.as_deref(), -(completed.removed_bytes as i64));
            self.refresh_degraded(now);
        }

        let task_id = id;
        let mut audits = Vec::new();
        for signal in &completed.signals {
            if let Some(event) = signal_event(&task_id, signal, now) {
                audits.push(event);
            }
        }
        let run_outcome = match completed.record.result {
            RunResult::Ok => AuditOutcome::Ok,
            RunResult::SkippedBusy | RunResult::Cancelled => AuditOutcome::Skipped,
            RunResult::Fail | RunResult::Timeout => AuditOutcome::Fail,
        };
        audits.push(task_event(
            now,
            AuditAction::TaskRun,
            &task_id,
            run_outcome,
            completed.record.error_detail.clone(),
        ));
        for event in &audits {
            let _ = self.store.append_audit(event.clone());
        }
        let _ = self.store.append_run(completed.record.clone());
        Ok(FinishOutcome {
            record: completed.record,
            next: completed.next,
            audits,
        })
    }

    /// 超时结算：中断等待，不杀执行体；记 timeout 并进入冷却。
    pub fn timeout(&mut self, run_id: &str, now: i64) -> Result<RunRecord, TaskError> {
        let error = ExecError::new(
            ErrorClass::Timeout,
            ErrorPhase::Call,
            "执行超时；等待被中断，执行体不杀",
        );
        Ok(self.finish(run_id, Err(error), now)?.record)
    }

    /// 协作式取消：置位取消 token；执行体自行检查并尽快返回。
    pub fn cancel(&mut self, id: &str) -> bool {
        match self.runtime.get_mut(id) {
            Some(rt) => match &rt.active {
                Some(active) => {
                    active.cancel.store(true, Ordering::SeqCst);
                    true
                }
                None => false,
            },
            None => false,
        }
    }

    /// 采样回填：`Ok(Some(value))` 写入缓冲，`Ok(None)` 表示本次无点可采（不算失败）。
    pub fn apply_sample(
        &mut self,
        ticket: &SampleTicket,
        result: Result<Option<Value>, ExecError>,
        now: i64,
    ) -> Result<Option<PushReport>, TaskError> {
        let id = ticket.context.task_id.to_string();
        {
            let rt = self
                .runtime
                .get_mut(&id)
                .ok_or_else(|| TaskError::NotFound(id.clone()))?;
            rt.sample_in_flight = false;
            if result.is_err() {
                rt.sample_failures += 1;
            }
        }
        match result {
            Ok(Some(value)) => self.push(&id, value, now).map(Some),
            _ => Ok(None),
        }
    }

    fn reconfigure_runtime(
        &mut self,
        task: &Task,
        old: OldSpec,
        now: i64,
        clock: &dyn Clock,
        force_state_refresh: bool,
    ) {
        let id = task.spec.id.to_string();
        if let std::collections::hash_map::Entry::Vacant(slot) = self.runtime.entry(id.clone()) {
            slot.insert(build_runtime(task, now, clock));
            return;
        }
        if old.buffer != task.spec.buffer {
            let removed = {
                let rt = self
                    .runtime
                    .get_mut(&id)
                    .expect("runtime presence checked above");
                rt.buffer.take()
            };
            if let Some(buffer) = removed {
                let bytes = buffer.bytes();
                self.global_bytes = self.global_bytes.saturating_sub(bytes);
                self.modify_plugin_bytes(task.spec.plugin_id(), -(bytes as i64));
            }
            let rt = self
                .runtime
                .get_mut(&id)
                .expect("runtime presence checked above");
            rt.buffer = task
                .spec
                .buffer
                .as_ref()
                .map(|cfg| Buffer::new(cfg.clone(), aggregate_rule(task)));
            rt.next_sample_at = task.spec.buffer.as_ref().and_then(|cfg| {
                cfg.sample_method
                    .as_ref()
                    .map(|_| now + cfg.sample_interval_secs as i64)
            });
            rt.sample_in_flight = false;
        }
        if old.schedule != task.spec.schedule {
            let rt = self
                .runtime
                .get_mut(&id)
                .expect("runtime presence checked above");
            rt.parsed = ParsedSchedule::parse(&task.spec.schedule).ok();
            reset_cursor(task, rt, clock, now);
        }
        if old.enabled != task.spec.enabled || force_state_refresh {
            let rt = self
                .runtime
                .get_mut(&id)
                .expect("runtime presence checked above");
            if task.spec.enabled {
                if task.state != LifecycleState::Blocked {
                    rt.state = TaskState::Idle;
                }
            } else {
                rt.state = TaskState::Disabled;
                rt.pending_one = false;
                if let Some(active) = &rt.active {
                    active.cancel.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// 全局硬上限腾挪：只动 `drop_oldest` 的任务，从最旧点开始丢。
    fn evict_for(&mut self, deficit: usize, except: &str, now: i64) {
        let mut keys: Vec<String> = self
            .runtime
            .keys()
            .filter(|k| k.as_str() != except)
            .cloned()
            .collect();
        keys.sort();
        let mut freed = 0usize;
        for key in keys {
            if freed >= deficit {
                break;
            }
            let removed = {
                let Some(rt) = self.runtime.get_mut(&key) else {
                    continue;
                };
                match rt.buffer.as_mut() {
                    Some(buffer) if buffer.config().overflow == OverflowPolicy::DropOldest => {
                        let target = buffer.bytes().saturating_sub(deficit - freed);
                        buffer.drop_oldest_until(target)
                    }
                    _ => (0, 0),
                }
            };
            let (points, bytes) = removed;
            if bytes == 0 {
                continue;
            }
            freed += bytes;
            self.global_bytes = self.global_bytes.saturating_sub(bytes);
            if let Some(plugin) = self
                .store
                .get_task(&key)
                .and_then(|t| t.spec.plugin_id().map(str::to_string))
            {
                self.modify_plugin_bytes(Some(&plugin), -(bytes as i64));
            }
            self.audit_event(
                now,
                AuditAction::BufferOverflow,
                AuditActor::System,
                &key,
                AuditOutcome::Skipped,
                Some(format!("全局上限腾挪：丢弃 {points} 点 / {bytes} B")),
            );
        }
    }

    fn refresh_degraded(&mut self, now: i64) {
        let soft = self.cfg.global_soft_limit_bytes;
        if !self.degraded && self.global_bytes >= soft {
            self.degraded = true;
            self.audit_event(
                now,
                AuditAction::BufferOverflow,
                AuditActor::System,
                "global",
                AuditOutcome::Skipped,
                Some(format!(
                    "全局缓冲达到软上限：{}/{} B",
                    self.global_bytes, soft
                )),
            );
        } else if self.degraded && (self.global_bytes as f64) <= (soft as f64) * 0.9 {
            self.degraded = false;
            self.audit_event(
                now,
                AuditAction::BufferRecovered,
                AuditActor::System,
                "global",
                AuditOutcome::Ok,
                Some(format!("全局缓冲回落到 {}/{} B", self.global_bytes, soft)),
            );
        }
    }

    fn modify_plugin_bytes(&mut self, plugin: Option<&str>, delta: i64) {
        if let Some(plugin_id) = plugin {
            let entry = self.plugin_bytes.entry(plugin_id.to_string()).or_insert(0);
            *entry = (*entry as i64 + delta).max(0) as usize;
        }
    }

    fn audit_event(
        &mut self,
        at: i64,
        action: AuditAction,
        actor: AuditActor,
        target: &str,
        outcome: AuditOutcome,
        detail: Option<String>,
    ) {
        let _ = self
            .store
            .append_audit(AuditEvent::new(at, action, actor, target, outcome, detail));
    }
}

struct TickCtx<'a> {
    cfg: &'a SchedulerConfig,
    now: i64,
    clock: &'a dyn Clock,
    gate: &'a dyn PluginGate,
}

#[derive(Default)]
struct TaskTick {
    run: Option<RunTicket>,
    sample: Option<SampleTicket>,
    records: Vec<RunRecord>,
    audits: Vec<AuditEvent>,
    task_changed: bool,
}

fn process_task(task: &mut Task, rt: &mut TaskRuntime, ctx: &TickCtx<'_>) -> TaskTick {
    let mut out = TaskTick::default();
    let now = ctx.now;

    // 1) 时钟回拨：先告警一次；回拨超过 10 分钟继续暂停，直到时钟恢复。
    if now < rt.last_observed_at {
        if !rt.skew_reported {
            rt.skew_reported = true;
            out.audits.push(task_event(
                now,
                AuditAction::ClockSkew,
                task.spec.id.as_str(),
                AuditOutcome::Fail,
                Some(format!("系统时钟从 {} 回拨到 {now}", rt.last_observed_at)),
            ));
        }
        if rt.last_observed_at - now > CLOCK_SKEW_PAUSE_SECS {
            rt.clock_paused = true;
            rt.state = TaskState::Blocked;
            if task.state != LifecycleState::Blocked {
                task.state = LifecycleState::Blocked;
                task.blocked_reason = Some(BlockedReason::ClockSkew);
                out.task_changed = true;
            }
        }
        return out;
    }
    if rt.clock_paused {
        rt.clock_paused = false;
        rt.skew_reported = false;
        rt.last_observed_at = now;
        rt.state = TaskState::Idle;
        task.state = LifecycleState::Active;
        task.blocked_reason = None;
        reset_cursor(task, rt, ctx.clock, now);
        out.task_changed = true;
    } else {
        rt.skew_reported = false;
        rt.last_observed_at = now;
    }

    // 2) 表达式非法（只可能来自持久化层，注册时已校验）。
    if rt.parsed.is_none() {
        rt.state = TaskState::Blocked;
        if task.state != LifecycleState::Blocked
            || task.blocked_reason != Some(BlockedReason::InvalidSchedule)
        {
            task.state = LifecycleState::Blocked;
            task.blocked_reason = Some(BlockedReason::InvalidSchedule);
            out.task_changed = true;
            out.audits.push(task_event(
                now,
                AuditAction::TaskBlocked,
                task.spec.id.as_str(),
                AuditOutcome::Fail,
                Some(BlockedReason::InvalidSchedule.hint().to_string()),
            ));
        }
        return out;
    }

    // 3) 停用 / 系统停用。
    if !task.spec.enabled || task.state == LifecycleState::DisabledBySystem {
        rt.state = TaskState::Disabled;
        return out;
    }

    // 4) 插件可用性。
    if let TaskKind::Plugin { id, .. } = &task.spec.kind {
        let status = ctx.gate.plugin_status(id);
        if status != PluginStatus::Ready {
            let reason = match status {
                PluginStatus::Ready => unreachable!("上面已排除 Ready"),
                PluginStatus::Disabled => BlockedReason::PluginDisabled,
                PluginStatus::NotInstalled => BlockedReason::NotInstalled,
                PluginStatus::ApiMismatch => BlockedReason::ApiMismatch,
            };
            rt.state = TaskState::Blocked;
            if task.state != LifecycleState::Blocked || task.blocked_reason != Some(reason) {
                task.state = LifecycleState::Blocked;
                task.blocked_reason = Some(reason);
                out.task_changed = true;
                out.audits.push(task_event(
                    now,
                    AuditAction::TaskBlocked,
                    task.spec.id.as_str(),
                    AuditOutcome::Fail,
                    Some(reason.hint().to_string()),
                ));
            }
            return out;
        }
        if task.state == LifecycleState::Blocked {
            task.state = LifecycleState::Active;
            task.blocked_reason = None;
            rt.state = TaskState::Idle;
            reset_cursor(task, rt, ctx.clock, now);
            out.task_changed = true;
        }
    }

    // 5) 采样（与执行独立）。
    if let Some(cfg) = &task.spec.buffer {
        if cfg.sample_method.is_some() && !rt.sample_in_flight {
            if let Some(next) = rt.next_sample_at {
                if now >= next {
                    rt.sample_in_flight = true;
                    rt.next_sample_at = Some(now + cfg.sample_interval_secs as i64);
                    out.sample = Some(SampleTicket {
                        context: SampleContext {
                            task_id: task.spec.id.clone(),
                            method: cfg.sample_method.clone().unwrap_or_default(),
                            at: now,
                        },
                        at: now,
                    });
                }
            }
        }
    }

    // 6) 重试优先：有待重试时不再处理调度到期，避免同任务两条在飞。
    if let Some(retry_at) = rt.retry_at {
        if now >= retry_at {
            rt.retry_at = None;
            let attempt = rt.attempt.max(1);
            match make_ticket(task, rt, Trigger::Retry, attempt, now, 0) {
                Some(ticket) => out.run = Some(ticket),
                None => out.records.push(skipped_record(
                    task,
                    rt,
                    Trigger::Retry,
                    now,
                    "任务仍在运行",
                )),
            }
        }
        return out;
    }

    // 7) 调度到期。
    let Some(next_due) = rt.next_due else {
        return out;
    };
    if now < next_due {
        return out;
    }
    let Some(next_boundary) = rt.next_boundary else {
        reset_cursor(task, rt, ctx.clock, now);
        return out;
    };
    if now < rt.cooldown_until {
        out.records.push(skipped_record(
            task,
            rt,
            Trigger::Schedule,
            now,
            "超时冷却中",
        ));
        reset_cursor(task, rt, ctx.clock, now);
        return out;
    }
    let Some(parsed) = rt.parsed.as_ref() else {
        return out;
    };
    let (count, last_boundary) = parsed.count_due(
        ctx.clock,
        next_boundary,
        task.spec.id.as_str(),
        now,
        ctx.cfg.max_missed_scan,
    );
    if count == 0 {
        reset_cursor(task, rt, ctx.clock, now);
        return out;
    }

    // 8) 并发策略。
    if rt.active.is_some() {
        match task.spec.concurrency {
            Concurrency::Skip => {
                out.records.push(skipped_record(
                    task,
                    rt,
                    Trigger::Schedule,
                    now,
                    "上一轮仍在运行（skip）",
                ));
            }
            Concurrency::QueueOne => {
                if rt.pending_one {
                    out.records.push(skipped_record(
                        task,
                        rt,
                        Trigger::Schedule,
                        now,
                        "待执行队列已满（queue_one）",
                    ));
                } else {
                    rt.pending_one = true;
                }
            }
        }
        reset_cursor(task, rt, ctx.clock, now);
        return out;
    }

    // 9) 错过策略。
    let runs = match task.spec.missed {
        MissedPolicy::Skip | MissedPolicy::Once => 1,
        MissedPolicy::All => count.min(ctx.cfg.max_catch_up),
    };
    let missed = count - runs;
    let trigger = match task.spec.missed {
        MissedPolicy::Skip => Trigger::Schedule,
        MissedPolicy::Once | MissedPolicy::All => Trigger::CatchUp,
    };
    rt.catch_up_remaining = runs - 1;
    rt.state = TaskState::Queued;
    advance_cursor(rt, last_boundary, ctx.clock, task.spec.id.as_str());

    match make_ticket(task, rt, trigger, 1, now, missed) {
        Some(ticket) => out.run = Some(ticket),
        None => out
            .records
            .push(skipped_record(task, rt, trigger, now, "任务仍在运行")),
    }
    out
}

fn build_runtime(task: &Task, now: i64, clock: &dyn Clock) -> TaskRuntime {
    let parsed = ParsedSchedule::parse(&task.spec.schedule).ok();
    let (next_boundary, next_due) = parsed
        .as_ref()
        .and_then(|p| p.first_after(clock, now, task.spec.id.as_str()))
        .map(|(boundary, due)| (Some(boundary), Some(due)))
        .unwrap_or((None, None));
    let buffer = task
        .spec
        .buffer
        .as_ref()
        .map(|cfg| Buffer::new(cfg.clone(), aggregate_rule(task)));
    let next_sample_at = task.spec.buffer.as_ref().and_then(|cfg| {
        cfg.sample_method
            .as_ref()
            .map(|_| now + cfg.sample_interval_secs as i64)
    });
    TaskRuntime {
        state: if task.spec.enabled {
            TaskState::Idle
        } else {
            TaskState::Disabled
        },
        last_observed_at: now,
        skew_reported: false,
        clock_paused: false,
        parsed,
        next_boundary,
        next_due,
        next_sample_at,
        sample_in_flight: false,
        sample_failures: 0,
        active: None,
        pending_one: false,
        cooldown_until: 0,
        retry_at: None,
        attempt: 0,
        catch_up_remaining: 0,
        run_seq: 0,
        buffer,
    }
}

fn aggregate_rule(task: &Task) -> Option<AggregateRule> {
    task.spec
        .payload
        .get("aggregate")
        .and_then(Value::as_str)
        .and_then(AggregateRule::parse)
}

fn reset_cursor(task: &Task, rt: &mut TaskRuntime, clock: &dyn Clock, now: i64) {
    let candidate = rt
        .parsed
        .as_ref()
        .and_then(|parsed| parsed.first_after(clock, now, task.spec.id.as_str()));
    match candidate {
        Some((boundary, due)) => {
            rt.next_boundary = Some(boundary);
            rt.next_due = Some(due);
        }
        None => {
            rt.next_boundary = None;
            rt.next_due = None;
        }
    }
}

fn advance_cursor(rt: &mut TaskRuntime, last_boundary: i64, clock: &dyn Clock, task_id: &str) {
    let candidate = rt.parsed.as_ref().and_then(|parsed| {
        parsed
            .next_boundary_after(clock, last_boundary)
            .map(|boundary| (boundary, parsed.due_at(boundary, task_id)))
    });
    match candidate {
        Some((boundary, due)) => {
            rt.next_boundary = Some(boundary);
            rt.next_due = Some(due);
        }
        None => {
            rt.next_boundary = None;
            rt.next_due = None;
        }
    }
}

fn make_ticket(
    task: &Task,
    rt: &mut TaskRuntime,
    trigger: Trigger,
    attempt: u32,
    now: i64,
    missed_count: u64,
) -> Option<RunTicket> {
    if rt.active.is_some() {
        return None;
    }
    let (meta, points) = match (task.spec.buffer.as_ref(), rt.buffer.as_mut()) {
        (Some(cfg), Some(buffer)) => {
            let snapshot = buffer.snapshot(cfg.max_batch_bytes);
            (snapshot.meta, snapshot.points)
        }
        _ => (SnapshotMeta::empty(), Vec::new()),
    };
    rt.run_seq += 1;
    let run_id = format!("{}:{now}:{}", task.spec.id.as_str(), rt.run_seq);
    let context = RunContext::new(
        task,
        run_id.clone(),
        trigger,
        attempt,
        now,
        meta.id,
        missed_count,
    );
    let cancel = context.cancellation_token();
    rt.active = Some(ActiveRun {
        run_id,
        started_at: now,
        attempt,
        trigger,
        snapshot: meta,
        missed_count,
        cancel,
    });
    rt.state = TaskState::Running;
    Some(RunTicket {
        context,
        batch: Batch {
            snapshot_id: meta.id,
            points,
            truncated: meta.truncated,
        },
    })
}

struct Completed {
    record: RunRecord,
    next: Option<RunTicket>,
    removed_bytes: usize,
    signals: Vec<BufferSignal>,
}

fn complete_success(
    task: &mut Task,
    rt: &mut TaskRuntime,
    active: ActiveRun,
    output: ExecOutput,
    outcome: &CommitOutcome,
    consumed: Option<u64>,
    now: i64,
) -> Completed {
    let duration_ms = (now - active.started_at).max(0) * 1000;
    rt.state = TaskState::Idle;
    rt.attempt = 0;
    rt.retry_at = None;
    let record = RunRecord {
        id: active.run_id,
        task_id: task.spec.id.clone(),
        trigger: active.trigger,
        attempt: active.attempt,
        started_at: active.started_at,
        finished_at: now,
        result: RunResult::Ok,
        error_class: None,
        error_detail: output.detail,
        duration_ms,
        request_id: None,
        consumed_upto: consumed,
        output_bytes: output.output_bytes,
        missed_count: active.missed_count,
    };
    task.last_run = Some(LastRun {
        at: now,
        result: RunResult::Ok,
        duration_ms,
        trigger: active.trigger,
    });
    let next = pending_ticket(task, rt, now);
    Completed {
        record,
        next,
        removed_bytes: outcome.removed_bytes,
        signals: outcome.signals.clone(),
    }
}

fn complete_failure(
    task: &mut Task,
    rt: &mut TaskRuntime,
    active: ActiveRun,
    error: ExecError,
    now: i64,
    cfg: SchedulerConfig,
) -> Completed {
    let duration_ms = (now - active.started_at).max(0) * 1000;
    let result = match error.class {
        ErrorClass::Timeout => RunResult::Timeout,
        ErrorClass::Cancelled => RunResult::Cancelled,
        _ => RunResult::Fail,
    };
    let retry_index = active.attempt.saturating_sub(1) as usize;
    // attempt 从 1 开始，retry.max 表示「允许的额外尝试次数」：
    // max=1 时第一次失败可以再试一次，第二次失败不再重试。
    let retryable = error.class.is_transient()
        && task.spec.retry.allows(error.class)
        && active.attempt <= task.spec.retry.max;
    if retryable {
        let backoff = task.spec.retry.backoff_for(retry_index);
        rt.attempt = active.attempt + 1;
        let wait = if error.class == ErrorClass::Timeout {
            backoff.max(cfg.cooldown_secs)
        } else {
            backoff
        };
        rt.retry_at = Some(now + wait);
        rt.state = TaskState::Failed;
    } else {
        rt.attempt = 0;
        rt.catch_up_remaining = 0;
        rt.state = if error.class == ErrorClass::Cancelled {
            TaskState::Idle
        } else {
            TaskState::Failed
        };
    }
    if error.class == ErrorClass::Timeout {
        rt.cooldown_until = now + cfg.cooldown_secs;
    }
    let next = if retryable {
        None
    } else {
        pending_ticket(task, rt, now)
    };
    let mut detail = format!(
        "[{}:{}] {}",
        error.phase.as_str(),
        error.class.as_str(),
        error.message
    );
    if let Some(plugin_id) = &error.plugin_id {
        detail.push_str(&format!("；plugin={plugin_id}"));
    }
    if let Some(request_id) = &error.request_id {
        detail.push_str(&format!("；request={request_id}"));
    }
    let record = RunRecord {
        id: active.run_id,
        task_id: task.spec.id.clone(),
        trigger: active.trigger,
        attempt: active.attempt,
        started_at: active.started_at,
        finished_at: now,
        result,
        error_class: Some(error.class),
        error_detail: Some(detail),
        duration_ms,
        request_id: error.request_id,
        consumed_upto: None,
        output_bytes: 0,
        missed_count: active.missed_count,
    };
    task.last_run = Some(LastRun {
        at: now,
        result,
        duration_ms,
        trigger: active.trigger,
    });
    Completed {
        record,
        next,
        removed_bytes: 0,
        signals: Vec::new(),
    }
}

fn pending_ticket(task: &Task, rt: &mut TaskRuntime, now: i64) -> Option<RunTicket> {
    if !task.spec.enabled {
        rt.catch_up_remaining = 0;
        rt.pending_one = false;
        return None;
    }
    if rt.catch_up_remaining > 0 {
        rt.catch_up_remaining -= 1;
        return make_ticket(task, rt, Trigger::CatchUp, 1, now, 0);
    }
    if rt.pending_one {
        rt.pending_one = false;
        return make_ticket(task, rt, Trigger::Schedule, 1, now, 0);
    }
    None
}

fn skipped_record(
    task: &Task,
    rt: &mut TaskRuntime,
    trigger: Trigger,
    now: i64,
    reason: &str,
) -> RunRecord {
    rt.run_seq += 1;
    RunRecord {
        id: format!("{}:{now}:skip{}", task.spec.id.as_str(), rt.run_seq),
        task_id: task.spec.id.clone(),
        trigger,
        attempt: 0,
        started_at: now,
        finished_at: now,
        result: RunResult::SkippedBusy,
        error_class: None,
        error_detail: Some(reason.to_string()),
        duration_ms: 0,
        request_id: None,
        consumed_upto: None,
        output_bytes: 0,
        missed_count: 0,
    }
}

fn empty_commit() -> CommitOutcome {
    CommitOutcome {
        removed_points: 0,
        removed_bytes: 0,
        remaining_points: 0,
        remaining_bytes: 0,
        signals: Vec::new(),
    }
}

fn reject_report(reason: RejectReason, bytes: usize) -> PushReport {
    PushReport {
        outcome: PushOutcome::Rejected { reason, bytes },
        signals: Vec::new(),
        net_bytes: 0,
    }
}

fn task_event(
    at: i64,
    action: AuditAction,
    target: &str,
    outcome: AuditOutcome,
    detail: Option<String>,
) -> AuditEvent {
    AuditEvent::new(at, action, AuditActor::System, target, outcome, detail)
}

fn signal_event(id: &str, signal: &BufferSignal, now: i64) -> Option<AuditEvent> {
    match signal {
        BufferSignal::HighWatermark => Some(task_event(
            now,
            AuditAction::BufferHighWatermark,
            id,
            AuditOutcome::Ok,
            Some("缓冲达到 80% 高水位".to_string()),
        )),
        BufferSignal::Overflow => Some(task_event(
            now,
            AuditAction::BufferOverflow,
            id,
            AuditOutcome::Skipped,
            Some("缓冲溢出：按策略丢弃/聚合/拒收".to_string()),
        )),
        BufferSignal::Recovered => Some(task_event(
            now,
            AuditAction::BufferRecovered,
            id,
            AuditOutcome::Ok,
            Some("缓冲已回落到高水位以下".to_string()),
        )),
        BufferSignal::PointTooLarge => Some(task_event(
            now,
            AuditAction::BufferPointTooLarge,
            id,
            AuditOutcome::Fail,
            Some("单点超过 64 KiB 上限".to_string()),
        )),
    }
}

#[cfg(test)]
mod tests;
