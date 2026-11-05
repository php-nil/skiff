//! 审计事件的**结构**，不负责落库（契约 §7.2）。
//!
//! 只记元数据：payload、采样点内容一律不进审计（可能含站点/容器标识，属敏感面）。
//! 高频采样也不逐点审计，只有缓冲治理（高水位/溢出/恢复）才产生事件。

use serde::{Deserialize, Serialize};

/// 审计动作；`as_str` 与契约 §7.2 的 action 命名一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    TaskCreate,
    TaskUpdate,
    TaskDelete,
    TaskTrigger,
    TaskRun,
    TaskBlocked,
    BufferHighWatermark,
    BufferOverflow,
    BufferRecovered,
    BufferPointTooLarge,
    BufferLostOnRestart,
    ClockSkew,
    SchedulerDeny,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::TaskCreate => "task.create",
            AuditAction::TaskUpdate => "task.update",
            AuditAction::TaskDelete => "task.delete",
            AuditAction::TaskTrigger => "task.trigger",
            AuditAction::TaskRun => "task.run",
            AuditAction::TaskBlocked => "task.blocked",
            AuditAction::BufferHighWatermark => "buffer.high_watermark",
            AuditAction::BufferOverflow => "buffer.overflow",
            AuditAction::BufferRecovered => "buffer.recovered",
            AuditAction::BufferPointTooLarge => "buffer.point_too_large",
            AuditAction::BufferLostOnRestart => "buffer.lost_on_restart",
            AuditAction::ClockSkew => "clock_skew",
            AuditAction::SchedulerDeny => "scheduler.deny",
        }
    }
}

/// 审计主体；`Admin("alice")` 渲染为 `admin:alice`，与契约的 actor 口径一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActor {
    Admin(String),
    Plugin(String),
    System,
}

impl AuditActor {
    pub fn as_string(&self) -> String {
        match self {
            AuditActor::Admin(name) if name.is_empty() => "admin".to_string(),
            AuditActor::Admin(name) => format!("admin:{name}"),
            AuditActor::Plugin(id) => format!("plugin:{id}"),
            AuditActor::System => "system".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Ok,
    Fail,
    Skipped,
}

impl AuditOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditOutcome::Ok => "ok",
            AuditOutcome::Fail => "fail",
            AuditOutcome::Skipped => "skipped",
        }
    }
}

/// 一条审计事件。`target` 是任务 id（缓冲治理也以任务为 target，全局事件用 `global`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub at: i64,
    pub action: AuditAction,
    pub actor: AuditActor,
    pub target: String,
    pub outcome: AuditOutcome,
    pub detail: Option<String>,
}

impl AuditEvent {
    pub fn new(
        at: i64,
        action: AuditAction,
        actor: AuditActor,
        target: impl Into<String>,
        outcome: AuditOutcome,
        detail: Option<String>,
    ) -> Self {
        Self {
            at,
            action,
            actor,
            target: target.into(),
            outcome,
            detail,
        }
    }

    /// 动作名，便于上层写 audit 表时直接取 `action` 字段。
    pub fn action_str(&self) -> &'static str {
        self.action.as_str()
    }
}
