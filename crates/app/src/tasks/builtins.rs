//! 内置任务执行体：DB 整理 / 回收站清理 / 审计清理。
//!
//! 调度层只出票（`RunTicket`），执行体放到独立 tokio 任务，晚到结果按 `run_id` 丢弃。
//! 站点备份走 exec 作业（见 `TaskService::site_backup`），不走这里的内置执行路径。

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use serde_json::Value;
use skiff_sched::{ErrorClass, ErrorPhase, ExecError, ExecOutput, RunContext, Trigger};
use skiff_store::Db;

use super::{
    audit_cutoff, AUDIT_RETENTION_DAYS, BUILTIN_AUDIT_PRUNE, BUILTIN_DB_VACUUM,
    BUILTIN_SITE_BACKUP, BUILTIN_TRASH_PRUNE,
};

// ---------------------------------------------------------------------------
// BuiltinCtx：从 RunContext 摘出的 Send 副本
// ---------------------------------------------------------------------------

/// 从 `RunContext` 摘出的可 `Send` 副本（`RunContext` 自带取消位，不能直接跨线程）。
pub(crate) struct BuiltinCtx {
    pub(crate) task_id: String,
    pub(crate) run_id: String,
    pub(crate) trigger: Trigger,
    pub(crate) attempt: u32,
    pub(crate) started_at: i64,
    pub(crate) snapshot_id: u64,
    pub(crate) missed_count: u64,
    pub(crate) payload: Value,
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

// ---------------------------------------------------------------------------
// 内置执行入口 + 辅助函数
// ---------------------------------------------------------------------------

pub(crate) fn run_builtin(
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
        // 站点备份有外部命令，走 exec 作业（见 `TaskService::site_backup`）；
        // 落到这里说明分发出了问题，宁可报错也不要绕过通道。
        BUILTIN_SITE_BACKUP => Err(ExecError::new(
            ErrorClass::ExecutorFailed,
            ErrorPhase::Ensure,
            format!("{BUILTIN_SITE_BACKUP}：该任务必须经 exec 通道执行"),
        )),
        BUILTIN_DB_VACUUM => db_vacuum(ctx, db, web_root, data_dir, trash_retention_days),
        BUILTIN_TRASH_PRUNE => trash_prune(ctx, db, web_root, data_dir, trash_retention_days),
        BUILTIN_AUDIT_PRUNE => audit_prune(db),
        other => Err(ExecError::new(
            ErrorClass::Unavailable,
            ErrorPhase::Ensure,
            format!("未注册的内置任务: {other}"),
        )),
    }
}

/// 内置任务失败的统一描述：`<任务>：<动作>失败：<目标>（<下一步>）：<原因>`。
///
/// 调度层只在前面拼 `[phase:class]`、不吞内层文案（`sched::scheduler` 的失败记录），
/// 因此「哪个任务 / 卡在哪一步 / 用户能做什么」必须在这里一次说清；
/// `error_class` / `error_detail` 等结构化字段不变。
pub(crate) fn builtin_error(
    task: &str,
    action: &str,
    target: Option<&Path>,
    hint: Option<&str>,
    cause: impl std::fmt::Display,
) -> ExecError {
    let mut message = format!("{task}：{action}失败");
    if let Some(target) = target {
        message.push_str(&format!("：{}", target.display()));
    }
    if let Some(hint) = hint {
        message.push_str(&format!("（{hint}）"));
    }
    message.push_str(&format!("：{cause}"));
    ExecError::new(ErrorClass::ExecutorFailed, ErrorPhase::Call, message)
}

/// 打包用的 `tar`：只认绝对路径，不做 PATH 查找。
///
/// 备份曾经由 `tar` crate 在进程内打包——那是内核自己在做文件 IO，既绕开了 exec
/// 通道（没有作业、没有过程日志、超时杀不掉），也不受「通道唯一」约束。现在改成
/// 跑 `tar` 二进制：命令怎么跑只有一份实现，过程与退出码都进任务中心。
pub(crate) fn tar_binary() -> Option<&'static str> {
    ["/usr/bin/tar", "/bin/tar"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
}

/// 一次内核命令作业失败原因的短描述（取作业自己的 error_detail，退到 stderr 末行）。
pub(crate) fn exec_failure_reason(outcome: &crate::host::KernelExecOutcome) -> String {
    outcome
        .error_detail
        .clone()
        .filter(|d| !d.is_empty())
        .or_else(|| outcome.stderr.lines().last().map(str::to_string))
        .unwrap_or_else(|| format!("退出码 {}", outcome.code.unwrap_or(-1)))
}

// ---------------------------------------------------------------------------
// 具体内置任务实现
// ---------------------------------------------------------------------------

pub(crate) fn db_vacuum(
    ctx: &BuiltinCtx,
    db: &Arc<Db>,
    web_root: &Path,
    data_dir: &Path,
    trash_retention_days: u32,
) -> Result<ExecOutput, ExecError> {
    let started = Instant::now();
    let _ = ctx;
    let dir = data_dir.join("backups").join("db");
    // 与站点备份/回收站清理同口径：备份目录走内核 fs 原语（`fs.mkdir` 审计）。
    crate::admin_fs::AdminFs::with_options(
        data_dir.to_path_buf(),
        web_root.to_path_buf(),
        Arc::clone(db),
        trash_retention_days,
    )
    .ensure_dir_kernel(&dir)
    .map_err(|e| {
        builtin_error(
            BUILTIN_DB_VACUUM,
            "创建数据库备份目录",
            Some(&dir),
            Some("检查数据目录空间与属主权限；若目标被同名文件占用，先移走或改名"),
            e,
        )
    })?;
    let ts = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = dir.join(format!("skiff-{ts}.db"));
    db.backup_to(&path).map_err(|e| {
        builtin_error(
            BUILTIN_DB_VACUUM,
            "生成数据库快照",
            Some(&path),
            Some("检查数据目录空间与属主权限后重试；若目标文件冲突，稍后重试"),
            e,
        )
    })?;
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

pub(crate) fn trash_prune(
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
    let bytes = admin.trash_prune().map_err(|e| {
        builtin_error(
            BUILTIN_TRASH_PRUNE,
            "清理回收站到期文件",
            None,
            Some("检查 data/trash 的属主与写权限后重试"),
            format!("{}（{}）", e.message(), e.code()),
        )
    })?;
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
pub(crate) fn audit_prune(db: &Db) -> Result<ExecOutput, ExecError> {
    let started = Instant::now();
    let cutoff = audit_cutoff(Local::now().timestamp());
    let deleted = db.purge_audit_before(cutoff).map_err(|e| {
        builtin_error(
            BUILTIN_AUDIT_PRUNE,
            "按保留期删除审计记录",
            None,
            Some("检查数据库文件与 WAL 的属主权限后重试"),
            e,
        )
    })?;
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
