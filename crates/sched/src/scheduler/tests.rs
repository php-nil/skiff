//! 调度循环的演算测试：所有时间都由 [`ManualClock`] 注入，不启动任何真实执行体。

use std::cell::Cell;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::*;
use crate::audit::{AuditAction, AuditActor, AuditOutcome};
use crate::buffer::{BufferConfig, OverflowPolicy, PushOutcome, RejectReason};
use crate::clock::ManualClock;
use crate::error::TaskError;
use crate::exec::{
    Batch, ErrorClass, ErrorPhase, ExecError, ExecOutput, Executor, RunContext, RunResult,
    SampleContext, Trigger, Verdict,
};
use crate::model::{
    BlockedReason, Concurrency, MissedPolicy, RetryPolicy, Schedule, TaskKind, TaskSource,
    TaskSpec, TaskState,
};

fn builtin(id: &str, name: &str, secs: u64) -> TaskSpec {
    TaskSpec::new(
        id,
        TaskKind::builtin(name).unwrap(),
        TaskSource::System,
        Schedule::interval(secs),
    )
    .unwrap()
}

fn plugin_task(id: &str, plugin: &str, method: &str, secs: u64) -> TaskSpec {
    TaskSpec::new(
        id,
        TaskKind::plugin(plugin, method).unwrap(),
        TaskSource::parse(&format!("plugin:{plugin}")).unwrap(),
        Schedule::interval(secs),
    )
    .unwrap()
}

fn cron_task(id: &str, expr: &str) -> TaskSpec {
    TaskSpec::new(
        id,
        TaskKind::builtin("cron-job").unwrap(),
        TaskSource::System,
        Schedule::cron(expr),
    )
    .unwrap()
}

fn buffered(overflow: OverflowPolicy) -> BufferConfig {
    BufferConfig {
        capacity_bytes: 4096,
        overflow,
        ..BufferConfig::default()
    }
}

fn scheduler(cfg: SchedulerConfig, clock: &ManualClock) -> Scheduler {
    Scheduler::in_memory(cfg, clock).unwrap()
}

fn naive(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(hh, mm, ss)
        .unwrap()
}

fn missed_spec(policy: MissedPolicy) -> TaskSpec {
    let mut spec = builtin("t1", "job", 60);
    spec.missed = policy;
    spec
}

#[test]
fn single_flight_skips_and_records_busy() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched.register(builtin("t1", "job", 5), 0, &clock).unwrap();

    clock.set(5);
    let first = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(first.runs.len(), 1);
    let run_id = first.runs[0].context.run_id.clone();

    // 上一轮仍在运行：到点后不产生新票据，而是记一条 skipped_busy
    clock.set(10);
    let second = sched.tick(&clock, &AllPluginsReady);
    assert!(second.runs.is_empty());
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0].result, RunResult::SkippedBusy);

    let finished = sched
        .finish(&run_id, Ok(ExecOutput::default()), 10)
        .unwrap()
        .record;
    assert_eq!(finished.result, RunResult::Ok);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Idle));
    assert_eq!(sched.history("t1", 10).len(), 2);
}

#[test]
fn timeout_enters_sixty_second_cooldown() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched.register(builtin("t1", "job", 30), 0, &clock).unwrap();

    clock.set(30);
    let report = sched.tick(&clock, &AllPluginsReady);
    let run_id = report.runs[0].context.run_id.clone();

    let record = sched.timeout(&run_id, 30).unwrap();
    assert_eq!(record.result, RunResult::Timeout);
    assert_eq!(record.error_class, Some(ErrorClass::Timeout));

    // 超时已结算后，插件晚到的回执必须按 run_id 丢弃，不能覆盖终态。
    let late = sched.finish(&run_id, Ok(ExecOutput::default()), 31);
    assert!(matches!(late, Err(TaskError::Protocol(_))));
    // 冷却期内任务处于失败态，晚到回执不能把它改回运行/成功。
    assert_eq!(sched.task_state("t1"), Some(TaskState::Failed));

    // 冷却期（60s）内再次到点 → 跳过
    clock.set(60);
    let skipped = sched.tick(&clock, &AllPluginsReady);
    assert!(skipped.runs.is_empty());
    assert_eq!(skipped.records.len(), 1);
    assert!(skipped.records[0]
        .error_detail
        .as_deref()
        .unwrap()
        .contains("冷却"));

    // 冷却结束（t=90）后恢复调度
    clock.set(90);
    let resumed = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(resumed.runs.len(), 1);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Running));
}

#[test]
fn missed_policy_skip_once_and_catch_up_all() {
    // skip：只跑一次，缺失量记在 missed_count
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(missed_spec(MissedPolicy::Skip), 0, &clock)
        .unwrap();
    clock.set(3600);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].context.trigger, Trigger::Schedule);
    assert_eq!(report.runs[0].context.missed_count, 59);

    // once：补跑一次
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(missed_spec(MissedPolicy::Once), 0, &clock)
        .unwrap();
    clock.set(3600);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].context.trigger, Trigger::CatchUp);
    assert_eq!(report.runs[0].context.missed_count, 59);
    let out = sched
        .finish(
            &report.runs[0].context.run_id,
            Ok(ExecOutput::default()),
            3600,
        )
        .unwrap();
    assert!(out.next.is_none());

    // all：内置任务补跑到 max_catch_up（默认 10）
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(missed_spec(MissedPolicy::All), 0, &clock)
        .unwrap();
    clock.set(3600);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].context.trigger, Trigger::CatchUp);
    assert_eq!(report.runs[0].context.missed_count, 50);

    let mut ticket = report.runs.into_iter().next().unwrap();
    let mut runs = 0;
    loop {
        runs += 1;
        let out = sched
            .finish(&ticket.context.run_id, Ok(ExecOutput::default()), 3600)
            .unwrap();
        match out.next {
            Some(next) => ticket = next,
            None => break,
        }
    }
    assert_eq!(runs, 10);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Idle));
}

#[test]
fn concurrent_tasks_do_not_block_each_other() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched.register(builtin("a", "job-a", 5), 0, &clock).unwrap();
    sched.register(builtin("b", "job-b", 5), 0, &clock).unwrap();

    clock.set(5);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 2);
    assert_eq!(sched.task_state("a"), Some(TaskState::Running));
    assert_eq!(sched.task_state("b"), Some(TaskState::Running));

    let a_id = report
        .runs
        .iter()
        .find(|r| r.context.task_id.as_str() == "a")
        .unwrap()
        .context
        .run_id
        .clone();
    sched.finish(&a_id, Ok(ExecOutput::default()), 5).unwrap();

    clock.set(10);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].context.task_id.as_str(), "a");
    assert_eq!(report.records.len(), 1);
    assert_eq!(report.records[0].task_id.as_str(), "b");
}

#[test]
fn manual_trigger_runs_and_rejects_while_busy() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched.register(builtin("t1", "job", 60), 0, &clock).unwrap();

    let ticket = sched
        .run_now(
            "t1",
            AuditActor::Admin("alice".to_string()),
            0,
            &AllPluginsReady,
        )
        .unwrap();
    assert_eq!(ticket.context.trigger, Trigger::Manual);

    let busy = sched.run_now(
        "t1",
        AuditActor::Admin("alice".to_string()),
        0,
        &AllPluginsReady,
    );
    assert!(matches!(busy, Err(TaskError::Busy(_))));

    let out = sched
        .finish(&ticket.context.run_id, Ok(ExecOutput::default()), 0)
        .unwrap();
    assert_eq!(out.record.trigger, Trigger::Manual);

    let triggers: Vec<_> = sched
        .audits(20)
        .into_iter()
        .filter(|a| a.action == AuditAction::TaskTrigger)
        .collect();
    assert_eq!(triggers.len(), 2);
    assert!(triggers.iter().any(|a| a.outcome == AuditOutcome::Ok));
    assert!(triggers.iter().any(|a| a.outcome == AuditOutcome::Skipped));
}

#[test]
fn retries_only_transient_errors_and_honors_backoff() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 60);
    spec.retry = RetryPolicy::with_backoff(1, vec![10]);
    sched.register(spec, 0, &clock).unwrap();

    clock.set(60);
    let report = sched.tick(&clock, &AllPluginsReady);
    let run_id = report.runs[0].context.run_id.clone();

    // 瞬时错误 → 记失败并安排退避重试
    let err = ExecError::new(ErrorClass::Unavailable, ErrorPhase::Call, "插件暂不可用")
        .with_plugin("mon");
    let out = sched.finish(&run_id, Err(err), 60).unwrap();
    assert_eq!(out.record.result, RunResult::Fail);
    assert!(out.next.is_none());
    assert!(out
        .record
        .error_detail
        .as_deref()
        .unwrap()
        .contains("plugin=mon"));

    clock.set(70);
    let report = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(report.runs.len(), 1);
    assert_eq!(report.runs[0].context.trigger, Trigger::Retry);
    assert_eq!(report.runs[0].context.attempt, 2);
    let out = sched
        .finish(
            &report.runs[0].context.run_id,
            Ok(ExecOutput::default()),
            70,
        )
        .unwrap();
    assert_eq!(out.record.result, RunResult::Ok);

    // 非瞬时错误不重试
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t2", "job2", 60);
    spec.retry = RetryPolicy::with_backoff(5, vec![10]);
    sched.register(spec, 0, &clock).unwrap();

    clock.set(60);
    let report = sched.tick(&clock, &AllPluginsReady);
    let err = ExecError::new(ErrorClass::InvalidParams, ErrorPhase::Call, "参数非法");
    let out = sched
        .finish(&report.runs[0].context.run_id, Err(err), 60)
        .unwrap();
    assert_eq!(out.record.result, RunResult::Fail);
    assert!(out.next.is_none());
    clock.set(70);
    assert!(sched.tick(&clock, &AllPluginsReady).runs.is_empty());
    assert_eq!(sched.history("t2", 10).len(), 1);
}

#[test]
fn queue_one_keeps_a_single_pending_slot() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 5);
    spec.concurrency = Concurrency::QueueOne;
    sched.register(spec, 0, &clock).unwrap();

    clock.set(5);
    let first = sched.tick(&clock, &AllPluginsReady);
    let run_id = first.runs[0].context.run_id.clone();

    // 第一次到点：只占一个待执行槽位，不记跳过
    clock.set(10);
    let second = sched.tick(&clock, &AllPluginsReady);
    assert!(second.runs.is_empty());
    assert!(second.records.is_empty());

    // 槽位已满：第二次到点记 skipped
    clock.set(15);
    let third = sched.tick(&clock, &AllPluginsReady);
    assert!(third.runs.is_empty());
    assert_eq!(third.records.len(), 1);

    let out = sched
        .finish(&run_id, Ok(ExecOutput::default()), 15)
        .unwrap();
    let next = out.next.unwrap();
    assert_eq!(next.context.trigger, Trigger::Schedule);
    assert_eq!(
        sched
            .finish(&next.context.run_id, Ok(ExecOutput::default()), 15)
            .unwrap()
            .record
            .result,
        RunResult::Ok
    );
}

#[test]
fn global_soft_limit_degrades_plugin_reject_sampling() {
    let clock = ManualClock::new(0);
    let cfg = SchedulerConfig {
        global_soft_limit_bytes: 100,
        global_limit_bytes: 10_000,
        plugin_limit_bytes: 10_000,
        plugin_hard_limit_bytes: 10_000,
        ..SchedulerConfig::default()
    };
    let mut sched = scheduler(cfg, &clock);

    let mut spec = plugin_task("t1", "mon", "collect", 60);
    spec.buffer = Some(BufferConfig {
        overflow: OverflowPolicy::Reject,
        ..buffered(OverflowPolicy::Reject)
    });
    sched.register(spec, 0, &clock).unwrap();

    // 39 B/点：3 点 117 B 越过软上限 100 B
    for _ in 0..3 {
        assert!(sched
            .push("t1", json!({ "v": 1 }), 0)
            .unwrap()
            .outcome
            .is_accepted());
    }
    assert!(sched.is_degraded());
    assert_eq!(sched.global_bytes(), 117);

    // 降级期间：插件来源 + reject 采样被暂停
    let report = sched.push("t1", json!({ "v": 1 }), 0).unwrap();
    assert!(matches!(
        report.outcome,
        PushOutcome::Rejected {
            reason: RejectReason::GlobalDegraded,
            ..
        }
    ));

    // 一轮 Discard 消费掉全部点 → 回落后解除降级
    clock.set(60);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(tick.runs[0].batch.points.len(), 3);
    let out = sched
        .finish(
            &tick.runs[0].context.run_id,
            Ok(ExecOutput {
                verdict: Some(Verdict::Discard),
                detail: None,
                output_bytes: 0,
            }),
            60,
        )
        .unwrap();
    assert_eq!(out.record.result, RunResult::Ok);
    assert!(!sched.is_degraded());
    assert_eq!(sched.global_bytes(), 0);

    assert!(sched
        .push("t1", json!({ "v": 1 }), 60)
        .unwrap()
        .outcome
        .is_accepted());
}

#[test]
fn global_hard_limit_evicts_drop_oldest_tasks_first() {
    let clock = ManualClock::new(0);
    let cfg = SchedulerConfig {
        global_soft_limit_bytes: 50,
        global_limit_bytes: 100,
        plugin_limit_bytes: 10_000,
        plugin_hard_limit_bytes: 10_000,
        ..SchedulerConfig::default()
    };
    let mut sched = scheduler(cfg, &clock);

    let mut a = builtin("a", "job-a", 60);
    a.buffer = Some(buffered(OverflowPolicy::DropOldest));
    let mut b = builtin("b", "job-b", 60);
    b.buffer = Some(buffered(OverflowPolicy::DropOldest));
    sched.register(a, 0, &clock).unwrap();
    sched.register(b, 0, &clock).unwrap();

    sched.push("a", json!({ "v": 1 }), 0).unwrap();
    sched.push("a", json!({ "v": 1 }), 0).unwrap();
    assert_eq!(sched.global_bytes(), 78);

    // 117 B > 100 B：先从 drop_oldest 的 a 丢最旧点腾出 39 B
    let report = sched.push("b", json!({ "v": 1 }), 0).unwrap();
    assert!(report.outcome.is_accepted());
    let stat = sched.buffer_stat("a").unwrap();
    assert_eq!(stat.points, 1);
    assert_eq!(stat.dropped_points, 1);
    assert_eq!(sched.global_bytes(), 78);
    assert_eq!(sched.buffer_stat("b").unwrap().points, 1);
}

#[test]
fn invalid_watermark_fails_protocol_without_touching_buffer() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 60);
    spec.buffer = Some(buffered(OverflowPolicy::DropOldest));
    sched.register(spec, 0, &clock).unwrap();
    sched.push("t1", json!({ "v": 1 }), 0).unwrap();
    sched.push("t1", json!({ "v": 1 }), 0).unwrap();

    clock.set(60);
    let tick = sched.tick(&clock, &AllPluginsReady);
    let run = &tick.runs[0];
    assert_eq!(run.batch.points.len(), 2);
    let tail = run.batch.last_seq().unwrap();

    let out = sched
        .finish(
            &run.context.run_id,
            Ok(ExecOutput {
                verdict: Some(Verdict::Persist {
                    persisted_upto: tail + 10,
                }),
                detail: None,
                output_bytes: 0,
            }),
            60,
        )
        .unwrap();
    assert_eq!(out.record.result, RunResult::Fail);
    assert_eq!(out.record.error_class, Some(ErrorClass::Protocol));
    assert_eq!(sched.buffer_stat("t1").unwrap().points, 2);
    assert_eq!(sched.global_bytes(), 78);

    // 同一批次不允许重复结算
    let again = sched.finish(&run.context.run_id, Ok(ExecOutput::default()), 60);
    assert!(matches!(again, Err(TaskError::Protocol(_))));

    // 下一轮用合法水位结算，缓冲被清空
    clock.set(120);
    let tick = sched.tick(&clock, &AllPluginsReady);
    let tail = tick.runs[0].batch.last_seq().unwrap();
    let out = sched
        .finish(
            &tick.runs[0].context.run_id,
            Ok(ExecOutput {
                verdict: Some(Verdict::Persist {
                    persisted_upto: tail,
                }),
                detail: None,
                output_bytes: 0,
            }),
            120,
        )
        .unwrap();
    assert_eq!(out.record.result, RunResult::Ok);
    assert_eq!(out.record.consumed_upto, Some(tail));
    assert_eq!(sched.buffer_stat("t1").unwrap().points, 0);
    assert_eq!(sched.global_bytes(), 0);
}

#[test]
fn sample_ticket_pushes_point_and_counts_failures() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 3600);
    spec.buffer = Some(BufferConfig {
        sample_method: Some("sample".to_string()),
        sample_interval_secs: 30,
        capacity_bytes: 4096,
        ..BufferConfig::default()
    });
    sched.register(spec, 0, &clock).unwrap();

    clock.set(30);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(tick.samples.len(), 1);
    assert_eq!(tick.samples[0].context.method, "sample");
    let report = sched
        .apply_sample(&tick.samples[0], Ok(Some(json!({ "v": 3 }))), 30)
        .unwrap()
        .unwrap();
    assert!(report.outcome.is_accepted());
    assert_eq!(sched.buffer_stat("t1").unwrap().points, 1);
    assert_eq!(sched.sample_failures("t1"), 0);

    clock.set(60);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(tick.samples.len(), 1);
    let err = ExecError::new(ErrorClass::Unavailable, ErrorPhase::Call, "采样失败");
    assert!(sched
        .apply_sample(&tick.samples[0], Err(err), 60)
        .unwrap()
        .is_none());
    assert_eq!(sched.sample_failures("t1"), 1);
}

#[test]
fn cron_tick_fires_at_injected_local_minute() {
    // UTC+8：本地 2026-11-05 03:29:30
    let offset: i32 = 8 * 3600;
    let start = naive(2026, 11, 5, 3, 29, 30).and_utc().timestamp() - i64::from(offset);
    let clock = ManualClock::with_offset(start, offset);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(cron_task("t1", "* * * * *"), start, &clock)
        .unwrap();

    assert!(sched.tick(&clock, &AllPluginsReady).runs.is_empty());

    // 本地 03:30:00 到点
    clock.set(start + 30);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(tick.runs.len(), 1);
    assert_eq!(clock.to_local(start + 30), naive(2026, 11, 5, 3, 30, 0));

    // 结束上一轮，避免单飞把下一分钟误判成忙碌
    sched
        .finish(
            &tick.runs[0].context.run_id,
            Ok(ExecOutput::default()),
            start + 30,
        )
        .unwrap();

    // 同一分钟内不重复
    clock.set(start + 60);
    assert!(sched.tick(&clock, &AllPluginsReady).runs.is_empty());

    // 03:31:00 再次到点
    clock.set(start + 90);
    assert_eq!(sched.tick(&clock, &AllPluginsReady).runs.len(), 1);
}

#[test]
fn clock_rollback_pauses_then_recovers() {
    let clock = ManualClock::new(1000);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(builtin("t1", "job", 60), 1000, &clock)
        .unwrap();

    // 回拨 700s（> 600s 阈值）→ 暂停并落 blocked
    clock.set(300);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Blocked));
    assert_eq!(
        sched.get("t1").unwrap().blocked_reason,
        Some(BlockedReason::ClockSkew)
    );
    assert!(tick
        .audits
        .iter()
        .any(|a| a.action == AuditAction::ClockSkew));

    // 时钟恢复 → 解除阻塞并重算游标（不补跑）
    clock.set(1001);
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Idle));
    assert_eq!(sched.get("t1").unwrap().blocked_reason, None);
    assert!(tick.runs.is_empty());
}

struct StaticGate(Cell<PluginStatus>);

impl PluginGate for StaticGate {
    fn plugin_status(&self, _plugin_id: &str) -> PluginStatus {
        self.0.get()
    }
}

#[test]
fn plugin_unavailable_blocks_until_gate_ready() {
    let clock = ManualClock::new(0);
    let gate = StaticGate(Cell::new(PluginStatus::NotInstalled));
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    sched
        .register(plugin_task("t1", "mon", "collect", 60), 0, &clock)
        .unwrap();

    clock.set(60);
    let tick = sched.tick(&clock, &gate);
    assert!(tick.runs.is_empty());
    assert_eq!(sched.task_state("t1"), Some(TaskState::Blocked));
    assert_eq!(
        sched.get("t1").unwrap().blocked_reason,
        Some(BlockedReason::NotInstalled)
    );
    assert!(tick
        .audits
        .iter()
        .any(|a| a.action == AuditAction::TaskBlocked));

    gate.0.set(PluginStatus::Ready);
    clock.set(61);
    sched.tick(&clock, &gate);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Idle));

    clock.set(120);
    let tick = sched.tick(&clock, &gate);
    assert_eq!(tick.runs.len(), 1);
    assert_eq!(sched.task_state("t1"), Some(TaskState::Running));
}

#[test]
fn disabled_task_is_not_scheduled_and_manual_trigger_rejected() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 60);
    spec.enabled = false;
    sched.register(spec, 0, &clock).unwrap();

    clock.set(60);
    assert!(sched.tick(&clock, &AllPluginsReady).runs.is_empty());
    assert_eq!(sched.task_state("t1"), Some(TaskState::Disabled));
    assert!(matches!(
        sched.run_now(
            "t1",
            AuditActor::Admin("root".to_string()),
            60,
            &AllPluginsReady
        ),
        Err(TaskError::Disabled(_))
    ));

    // 重新启用后恢复调度
    sched.set_enabled("t1", true, 120, &clock).unwrap();
    let tick = sched.tick(&clock, &AllPluginsReady);
    assert_eq!(tick.runs.len(), 1);
}

struct CollectExecutor {
    calls: Cell<u32>,
}

impl Executor for CollectExecutor {
    fn sample(&mut self, _ctx: &SampleContext) -> Result<Option<Value>, ExecError> {
        Ok(Some(json!({ "v": 7 })))
    }

    fn execute(&mut self, _ctx: &RunContext, batch: &Batch) -> Result<ExecOutput, ExecError> {
        self.calls.set(self.calls.get() + 1);
        Ok(ExecOutput {
            verdict: batch.last_seq().map(|seq| Verdict::Persist {
                persisted_upto: seq,
            }),
            detail: Some(format!("处理 {} 点", batch.len())),
            output_bytes: batch.bytes(),
        })
    }
}

#[test]
fn executor_trait_roundtrips_batch_and_watermark() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let mut spec = builtin("t1", "job", 60);
    spec.buffer = Some(buffered(OverflowPolicy::DropOldest));
    sched.register(spec, 0, &clock).unwrap();
    sched.push("t1", json!({ "v": 1 }), 0).unwrap();
    sched.push("t1", json!({ "v": 2 }), 0).unwrap();

    clock.set(60);
    let tick = sched.tick(&clock, &AllPluginsReady);
    let ticket = &tick.runs[0];
    let mut executor = CollectExecutor {
        calls: Cell::new(0),
    };
    let output = executor.execute(&ticket.context, &ticket.batch).unwrap();
    let out = sched
        .finish(&ticket.context.run_id, Ok(output), 60)
        .unwrap();
    assert_eq!(out.record.result, RunResult::Ok);
    assert_eq!(out.record.consumed_upto, ticket.batch.last_seq());
    assert_eq!(executor.calls.get(), 1);
    assert_eq!(sched.buffer_stat("t1").unwrap().points, 0);
}

#[test]
fn delete_protects_builtin_but_cascade_removes_it() {
    let clock = ManualClock::new(0);
    let mut sched = scheduler(SchedulerConfig::default(), &clock);
    let spec = builtin("builtin:site-backup:probe.test", "site-backup", 3600);
    sched.register(spec, 0, &clock).unwrap();

    // 用户/插件入口：内置任务必须拒绝
    let err = sched
        .delete("builtin:site-backup:probe.test", 0)
        .unwrap_err();
    assert!(matches!(err, TaskError::BuiltinProtected(_)));
    assert!(sched.get("builtin:site-backup:probe.test").is_some());

    // 内核级联路径（删站）：任务与调度器内存一并移除
    sched
        .delete_cascade("builtin:site-backup:probe.test", 0)
        .unwrap();
    assert!(sched.get("builtin:site-backup:probe.test").is_none());
    assert!(sched
        .delete_cascade("builtin:site-backup:probe.test", 0)
        .is_err());
}
