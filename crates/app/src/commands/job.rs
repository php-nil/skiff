//! 作业（异步任务）的管理员写命令：取消与重试。
//!
//! 状态校验仍在 `JobService::cancel` / `AppState::retry_job`；命令只补审计，
//! 保证换传输端点也不会漏掉留痕。

use std::sync::Arc;

use crate::commands::Actor;
use crate::jobs::{CancelOutcome, Job, JobError};
use crate::sites::SiteError;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct CancelJob {
    pub id: String,
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct RetryJob {
    pub id: String,
    pub role: String,
    pub actor: Actor,
}

fn cancel_message(outcome: CancelOutcome) -> &'static str {
    match outcome {
        CancelOutcome::CancelledNow => "已取消（任务尚未开始）",
        CancelOutcome::Requested => "已请求取消，将在当前步骤结束后停止",
        CancelOutcome::Terminal => "任务已结束，无需取消",
    }
}

pub async fn cancel(state: &Arc<AppState>, cmd: CancelJob) -> Result<CancelOutcome, JobError> {
    let outcome = state.jobs().cancel(&cmd.id)?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "job.cancel",
        &cmd.id,
        "ok",
        cancel_message(outcome),
    );
    Ok(outcome)
}

pub async fn retry(state: &Arc<AppState>, cmd: RetryJob) -> Result<Job, SiteError> {
    let job = state
        .retry_job(&cmd.id, &cmd.actor.username, &cmd.role, &cmd.actor.ip)
        .await?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "job.retry",
        &cmd.id,
        "ok",
        &job.id,
    );
    Ok(job)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::commands::test_support::state_at;
    use crate::jobs::{JobSubmit, TARGET_RUNTIME};

    fn actor() -> Actor {
        Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        }
    }

    fn submit(state: &Arc<AppState>) -> Job {
        state
            .jobs()
            .submit(JobSubmit {
                kind: "runtime.ext.install".into(),
                target_type: TARGET_RUNTIME.into(),
                target_id: "rt-1".into(),
                title: "安装 PHP 扩展 redis".into(),
                params: json!({"ext_keys": ["redis"]}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: "127.0.0.1".into(),
                cancellable: true,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn cancel_settles_queued_job_and_audits() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let job = submit(&state);

        let outcome = cancel(
            &state,
            CancelJob {
                id: job.id.clone(),
                actor: actor(),
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome, CancelOutcome::CancelledNow);

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "job.cancel");
        assert_eq!(rows[0].target, job.id);
        assert_eq!(rows[0].result, "ok");
    }

    #[tokio::test]
    async fn retry_terminal_job_creates_new_job_and_audits() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        state
            .db()
            .create_runtime(&skiff_store::NewRuntime {
                id: "rt-1".into(),
                kind: "php".into(),
                name: "rt-1".into(),
                version: "8.3".into(),
                image: "php:8.3-fpm-alpine".into(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
        let job = submit(&state);
        state.jobs().finish_err(&job.id, "script", "退出码 1");

        let retried = retry(
            &state,
            RetryJob {
                id: job.id.clone(),
                role: "admin".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap();
        assert_ne!(retried.id, job.id);

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "job.retry");
        assert_eq!(rows[0].target, job.id);
        assert_eq!(rows[0].result, "ok");
        assert_eq!(rows[0].detail, retried.id);
    }
}
