//! 内核自身发起的宿主命令：唯一入口。
//!
//! **谁都不许再直接写 `tokio::process::Command` / `std::process::Command`**：执行通道
//! 是 `skiff_host`（起进程 / 读流 / 超时 / 取消 / 清理只有一份），这里只补四件
//! 「在面板语境里才成立」的事：
//!
//! 1. **提交作业**：命令是一次耗时动作，过程要能在任务中心看见（`jobs` / `job_logs`）；
//! 2. **过程行进作业日志**（`out` / `err` 流）——装包装到哪一步、备份打了多久都看得见；
//! 3. **取消标记接上通道**（250ms 轮询），命中即终止瞬态单元；
//! 4. **错误转成中文一句话**，调用方只管结算。
//!
//! 三类调用方共用本文的同一份执行体 [`drive_exec_job`]：
//!
//! | 调用方 | 授权 | 作业发起者 | 入口 |
//! | --- | --- | --- | --- |
//! | 插件 | `exec:run` + `bin:` 三道闸（`execgrant`） | 内核代提交 | `kernel::exec` 路由 |
//! | 内核计划任务 | 无（内核是授出方），仍逐次审计 | 调度器 | [`KernelExec::run`] |
//! | 内核安装/站点动作 | 无，通常连作业都不需要 | 各动作 | [`AppState::run_host_in_job`] |
//!
//! 区别是「要不要授权」与「谁发起」，**不是「怎么执行」**：起进程、读流、超时、
//! 取消、清理只有 `skiff_host` 一份实现，记录只有 `audit` 与 `jobs` 两处。

use serde_json::{json, Value};
use skiff_host::{HostOutput, HostRunError, HostSpec};
use skiff_store::Db;

use crate::jobs::{JobError, JobService, JobSubmit, KIND_HOST_EXEC, TARGET_HOST};
use crate::AppState;

/// 单条命令最多写进作业日志的行数。
///
/// `apt-get install` 的输出能到几千行，全量落库会让轮询响应变成几 MB。超出后只写
/// 一条省略提示，真实日志仍在命令自己的目标里。
pub const JOB_LOG_LINE_CAP: usize = 2000;

/// 单路输出在作业结果里保留的上限（字符边界截断）。
pub const OUTPUT_CAP: usize = 64 * 1024;

/// 内核自己发起的一次命令作业。
///
/// `target_id` 决定并发边界：作业表对同一 `(target_type, target_id)` 只允许一条活跃
/// 作业，因此同一个计划任务天然串行（不同站点备份互不阻塞）。
pub struct KernelExec<'a> {
    /// 任务中心展示名。
    pub title: String,
    /// 作业目标 id（计划任务用任务 id，安装用安装项 id）。
    pub target_id: String,
    /// 审计 detail 里的来源标记（`sched` / `install` / `site`）。
    pub origin: &'a str,
    /// 绝对路径；内核不查授权，但要求调用方给确定的二进制，不做 PATH 查找。
    pub binary: &'a str,
    pub argv: Vec<String>,
    /// 命令运行上限（秒）。与通道超时同一个值，超时即停单元。
    pub timeout_secs: u64,
}

/// 一次内核命令作业的终态。
pub struct KernelExecOutcome {
    pub job_id: String,
    pub status: String,
    pub success: bool,
    pub code: Option<i64>,
    pub stderr: String,
    pub error_class: Option<String>,
    pub error_detail: Option<String>,
}

impl KernelExec<'_> {
    /// 提交作业 → 审计 → 执行并结算 → 读回终态。
    ///
    /// 提交失败（同目标已有活跃作业 / 库写失败）返回 `Err`，因为此时**什么都没跑**；
    /// 跑起来之后的失败走 [`KernelExecOutcome`]，调用方据此决定任务运行记录的错误类。
    pub async fn run(&self, db: &Db, jobs: &JobService) -> Result<KernelExecOutcome, String> {
        let risk = skiff_model::bin_risk(self.binary);
        let job = jobs
            .submit(JobSubmit {
                kind: KIND_HOST_EXEC.to_string(),
                target_type: TARGET_HOST.to_string(),
                target_id: self.target_id.clone(),
                title: self.title.clone(),
                // 与插件那条路同形：不含 stdin，含 argv 与风险档，事后可复查。
                params: json!({
                    "origin": self.origin,
                    "binary": self.binary,
                    "argv": self.argv,
                    "timeout_sec": self.timeout_secs,
                    "risk": risk.as_str(),
                }),
                created_by: crate::admin_fs::KERNEL_ACTOR.to_string(),
                created_by_role: crate::admin_fs::KERNEL_ACTOR.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(|e| match e {
                JobError::Busy(m) => format!("该目标已有命令在执行：{m}"),
                other => other.to_string(),
            })?;

        let job_id = job.id.clone();
        if let Err(e) = db.audit(
            crate::admin_fs::KERNEL_ACTOR,
            "sched",
            "exec.run",
            "",
            "ok",
            &kernel_detail(self, risk, &job_id),
        ) {
            // 审计写失败不改变执行结果，但必须留痕，不能静默丢弃。
            tracing::error!(error = %e, job = %job_id, "内核命令审计写入失败");
        }

        drive_exec_job(
            jobs,
            &job_id,
            self.binary,
            &self.argv,
            self.timeout_secs,
            None,
        )
        .await;

        let job = jobs
            .get(&job_id)
            .ok_or_else(|| format!("作业 {job_id} 已结束但读回失败"))?;
        Ok(KernelExecOutcome {
            success: job
                .result
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            code: job.exit_code,
            stderr: job
                .result
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            error_class: job.error_class.clone(),
            error_detail: job.error_detail.clone(),
            status: job.status.as_str().to_string(),
            job_id,
        })
    }
}

/// 审计 detail：二进制 + 风险档 + 作业 id + argv 全文（截断）+ 超时 + 来源。
fn kernel_detail(spec: &KernelExec<'_>, risk: skiff_model::BinRisk, job_id: &str) -> String {
    let mut detail = format!(
        "bin={} risk={} job={job_id} origin={} target={}",
        spec.binary,
        risk.as_str(),
        spec.origin,
        spec.target_id
    );
    if !spec.argv.is_empty() {
        let (shown, cut) = clip_text(spec.argv.join(" "), 512);
        detail.push_str(&format!(" argv={shown}"));
        if cut {
            detail.push_str("…(截断)");
        }
    }
    detail.push_str(&format!(" timeout={}s", spec.timeout_secs));
    detail
}

/// 命令作业的执行体：跑命令 → 逐行写运行日志 → 结算。
///
/// **插件提交与内核计划任务共用这一份**：失败分类直接映射到任务中心的
/// `error_class`——`timeout`（超时）、`cancelled`（取消，走独立状态）、`channel`
/// （起进程/管道失败）、`exit`（命令跑了但非零退出）。这四类对用户要采取的动作完全不同。
pub async fn drive_exec_job(
    jobs: &JobService,
    job_id: &str,
    bin: &str,
    argv: &[String],
    timeout_secs: u64,
    stdin: Option<String>,
) {
    jobs.set_phase(job_id, "run", "执行中", -1, true);

    let cancel = || jobs.cancel_requested(job_id);
    let mut written = 0usize;
    let result = skiff_host::run_stream(
        HostSpec::privileged(bin, argv)
            .prefix("skiff-exec")
            .description("skiff host exec")
            .stdin(stdin.as_deref())
            .timeout(timeout_secs),
        Some(&cancel),
        |stream, line| {
            if written >= JOB_LOG_LINE_CAP {
                return;
            }
            written += 1;
            jobs.log(job_id, stream, line);
        },
    )
    .await;

    if written >= JOB_LOG_LINE_CAP {
        jobs.note(
            job_id,
            &format!(
                "输出超过 {JOB_LOG_LINE_CAP} 行，后续输出已省略（完整内容见命令自身的日志目标）"
            ),
        );
    }

    match result {
        Ok(out) => {
            let code = out.code;
            if let Some(c) = code {
                jobs.set_exit_code(job_id, c as i64);
            }
            let (stdout, out_cut) = clip_text(out.stdout, OUTPUT_CAP);
            let (stderr, err_cut) = clip_text(out.stderr, OUTPUT_CAP);
            let payload = json!({
                "success": out.success,
                "code": code,
                "stdout": stdout,
                "stderr": stderr,
                "truncated": { "stdout": out_cut, "stderr": err_cut },
            });
            if out.success {
                // `finish_ok` 内部把 phase 置 `done`、progress 置 100，不必先写一遍。
                jobs.finish_ok(job_id, "命令执行完成", payload);
            } else {
                let detail = match code {
                    Some(c) => format!("命令退出码 {c}"),
                    None => "命令被信号终止（无退出码）".to_string(),
                };
                // 失败也保留 stdout/stderr：编排下一句要靠 stderr 判断。
                jobs.finish_err_result(job_id, "exit", &detail, payload);
            }
        }
        Err(failure) => {
            let msg = failure.message(bin);
            settle_channel_failure(jobs, job_id, failure, msg);
        }
    }
}

/// 通道层失败的结算（超时/取消/起不来）。
fn settle_channel_failure(jobs: &JobService, job_id: &str, failure: HostRunError, msg: String) {
    match failure {
        HostRunError::Cancelled => {
            jobs.finish_cancelled(job_id, "已取消，已尝试终止运行时单元");
        }
        HostRunError::Timeout { .. } => {
            jobs.finish_err(job_id, "timeout", &msg);
        }
        HostRunError::Spawn(_) | HostRunError::Io(_) => {
            jobs.finish_err(job_id, "channel", &msg);
        }
    }
}

/// 按**字符边界**截断（`String` 的字节切片会 panic 在多字节字符中间）。
pub fn clip_text(s: String, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        return (s, false);
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

impl AppState {
    /// 在作业里执行一条宿主命令。
    ///
    /// 失败**不结算作业**：调用方知道自己在第几步、该写什么错误码（`exit` /
    /// `timeout` / 安装失败），这里只把「为什么没跑成」翻译成中文。
    pub async fn run_host_in_job(
        &self,
        job_id: &str,
        spec: HostSpec<'_>,
    ) -> Result<HostOutput, String> {
        let cmd = spec.cmd.to_string();
        let cancel = || self.jobs().cancel_requested(job_id);

        let mut written = 0usize;
        let result = skiff_host::run_stream(spec, Some(&cancel), |stream, line| {
            if written >= JOB_LOG_LINE_CAP {
                return;
            }
            written += 1;
            self.jobs().log(job_id, stream, line);
        })
        .await;

        if written >= JOB_LOG_LINE_CAP {
            self.jobs().note(
                job_id,
                &format!("输出超过 {JOB_LOG_LINE_CAP} 行，后续输出已省略"),
            );
        }
        result.map_err(|e| e.message(&cmd))
    }

    /// 内核命令作业（计划任务等）：提交 + 审计 + 执行 + 结算，返回终态。
    pub async fn exec_kernel(&self, spec: KernelExec<'_>) -> Result<KernelExecOutcome, String> {
        spec.run(&self.db, self.jobs()).await
    }
}

/// 作业之外的执行：只读探测或短命令，不写作业日志、不跟取消。
///
/// 返回 `HostRunError` 而不是字符串：需要区分「超时（结果未知）」与「起不来」
/// 的调用方（如 `systemctl restart docker`）自己决定文案，别在这里糊成一句。
pub async fn run_host(spec: HostSpec<'_>) -> Result<HostOutput, skiff_host::HostRunError> {
    skiff_host::run(spec).await
}

/// `run_host` 的字符串错误版，给「失败即整段放弃」的调用方用。
pub async fn run_host_msg(spec: HostSpec<'_>) -> Result<HostOutput, String> {
    let cmd = spec.cmd.to_string();
    skiff_host::run(spec).await.map_err(|e| e.message(&cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::state_at;
    use serde_json::json;

    fn submit(state: &AppState) -> String {
        state
            .jobs()
            .submit(JobSubmit {
                kind: "site.cert".into(),
                target_type: "site".into(),
                target_id: "example.com".into(),
                title: "生成证书".into(),
                params: json!({}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: "127.0.0.1".into(),
                cancellable: true,
            })
            .unwrap()
            .id
    }

    /// 起不来时只返回错误，不替调用方结算作业：结算带错误码与步骤上下文，
    /// 只有调用方知道该写什么。
    #[tokio::test]
    async fn spawn_failure_is_translated_and_leaves_job_open() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let job_id = submit(&state);

        let args: Vec<String> = Vec::new();
        let err = state
            .run_host_in_job(&job_id, HostSpec::direct("/definitely/not/here", &args))
            .await
            .unwrap_err();

        // 中文文案里带命令本身，排障不用再回查提交参数。
        assert!(err.contains("执行"), "unexpected: {err}");
        assert!(err.contains("/definitely/not/here"), "unexpected: {err}");
        assert!(!state.jobs().get(&job_id).unwrap().status.terminal());
    }

    /// 内核命令作业：提交后可读回同一条 `host.exec` 作业，且审计里有 `exec.run`。
    /// 执行本身在非 Linux 上必然失败（通道只在 Linux 可用），失败也必须结算，
    /// 不能留下「永远进行中」的作业。
    #[tokio::test]
    async fn kernel_exec_settles_the_job_and_audits() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let out = state
            .exec_kernel(KernelExec {
                title: "测试命令".into(),
                target_id: "builtin:demo".into(),
                origin: "sched",
                binary: "/definitely/not/here",
                argv: vec!["--version".into()],
                timeout_secs: 5,
            })
            .await
            .expect("提交阶段不应失败");

        assert!(!out.success);
        assert!(!out.stderr.is_empty() || out.error_detail.is_some());
        let job = state.jobs().get(&out.job_id).unwrap();
        assert_eq!(job.kind, KIND_HOST_EXEC);
        assert_eq!(job.target_id, "builtin:demo");
        assert!(job.status.terminal(), "作业必须结算");
        // 审计：谁发起、跑什么、哪个作业。
        let rows = state.db().recent_audit(20).unwrap();
        let hit = rows.iter().find(|a| a.action == "exec.run");
        let hit = hit.expect("缺少 exec.run 审计");
        assert_eq!(hit.actor, "system");
        assert!(hit.detail.contains("origin=sched"), "{}", hit.detail);
        assert!(hit.detail.contains(&out.job_id), "{}", hit.detail);
    }

    #[test]
    fn clip_text_keeps_char_boundaries() {
        let s = "封禁".repeat(4);
        let (out, cut) = clip_text(s.clone(), 7);
        assert!(cut);
        assert!(s.starts_with(&out));
    }
}
