//! 内核定时任务的管理员写命令：启停、立即执行、删除。
//!
//! 调度语义仍在 `TaskService`；命令只负责取服务、调用与审计。缺少任务服务时返回
//! `Unavailable`，与页面原来的 503 口径一致。

use std::sync::Arc;

use serde_json::Value;

use crate::commands::Actor;
use crate::AppState;

#[derive(Debug)]
pub enum TaskCommandError {
    /// 内核任务服务未初始化（页面映射 503）。
    Unavailable(String),
    /// 调度器拒绝或执行失败（页面用 flash 呈现）。
    Failed(String),
}

impl std::fmt::Display for TaskCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToggleTask {
    pub id: String,
    pub enabled: bool,
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct RunTask {
    pub id: String,
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct DeleteTask {
    pub id: String,
    pub actor: Actor,
}

fn service(state: &AppState) -> Result<&Arc<crate::tasks::TaskService>, TaskCommandError> {
    state
        .tasks()
        .ok_or_else(|| TaskCommandError::Unavailable("内核任务服务未初始化".into()))
}

pub async fn toggle(state: &Arc<AppState>, cmd: ToggleTask) -> Result<Value, TaskCommandError> {
    let svc = service(state)?;
    let value = svc
        .set_enabled_task(&cmd.id, cmd.enabled)
        .await
        .map_err(TaskCommandError::Failed)?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "tasks.toggle",
        &cmd.id,
        "ok",
        if cmd.enabled { "启用" } else { "停用" },
    );
    Ok(value)
}

pub async fn run(state: &Arc<AppState>, cmd: RunTask) -> Result<Value, TaskCommandError> {
    let svc = service(state)?;
    let actor = skiff_sched::AuditActor::Admin(cmd.actor.username.clone());
    let result = svc.run_task_now(&cmd.id, actor).await;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "tasks.run",
        &cmd.id,
        if result.is_ok() { "ok" } else { "fail" },
        "",
    );
    result.map_err(TaskCommandError::Failed)
}

pub async fn delete(state: &Arc<AppState>, cmd: DeleteTask) -> Result<(), TaskCommandError> {
    let svc = service(state)?;
    let result = svc.delete_task(&cmd.id).await;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "tasks.delete",
        &cmd.id,
        if result.is_ok() { "ok" } else { "fail" },
        "",
    );
    result.map_err(TaskCommandError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::state_at;

    fn actor() -> Actor {
        Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        }
    }

    #[tokio::test]
    async fn missing_task_service_is_unavailable_and_not_audited() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = toggle(
            &state,
            ToggleTask {
                id: "builtin:disk".into(),
                enabled: true,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TaskCommandError::Unavailable(ref m) if m.contains("未初始化")));
        assert!(state.db().recent_audit(10).unwrap().is_empty());
    }
}
