//! 任务模型（契约 §3）：`TaskId` / `TaskKind` / `TaskSource` / `Schedule` / `TaskSpec`，
//! 以及生命期状态、运行态状态机、修改补丁与列表过滤。
//!
//! 所有字段级校验都集中在 [`TaskSpec::validate`]：创建、修改、启用、从持久化层加载
//! 时走同一份规则，避免「装的时候能进、跑的时候才炸」。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::buffer::BufferConfig;
use crate::error::TaskError;
use crate::exec::{ErrorClass, RunResult, Trigger};

pub const TASK_ID_MAX_LEN: usize = 96;
pub const PLUGIN_METHOD_MAX_LEN: usize = 64;
pub const PAYLOAD_MAX_BYTES: usize = 4 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const MAX_TIMEOUT_SECS: u64 = 600;
pub const MIN_INTERVAL_SECS: u64 = 5;
pub const MAX_JITTER_SECS: u64 = 600;
pub const MAX_RETRIES: u32 = 10;
/// 时钟回拨超过该阈值就暂停任务，等时钟恢复（契约 §5.2）。
pub const CLOCK_SKEW_PAUSE_SECS: i64 = 600;

fn is_token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= TASK_ID_MAX_LEN
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

fn is_scope_value(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// 全局唯一任务 id，字符集与长度按契约 §3.1 收紧。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TaskId(String);

impl TaskId {
    pub fn new(value: impl Into<String>) -> Result<Self, TaskError> {
        let value = value.into();
        if !is_id(&value) {
            return Err(TaskError::InvalidId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TaskId {
    type Err = TaskError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for TaskId {
    type Error = TaskError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TaskId> for String {
    fn from(value: TaskId) -> Self {
        value.0
    }
}

/// 执行体种类：`builtin:<name>` 或 `plugin:<plugin_id>:<method>`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum TaskKind {
    Builtin { name: String },
    Plugin { id: String, method: String },
}

impl TaskKind {
    pub fn builtin(name: impl Into<String>) -> Result<Self, TaskError> {
        let name = name.into();
        if !is_token(&name, PLUGIN_METHOD_MAX_LEN) {
            return Err(TaskError::InvalidKind(format!("builtin:{name}")));
        }
        Ok(Self::Builtin { name })
    }

    pub fn plugin(id: impl Into<String>, method: impl Into<String>) -> Result<Self, TaskError> {
        let id = id.into();
        let method = method.into();
        if !is_token(&id, PLUGIN_METHOD_MAX_LEN) || !is_token(&method, PLUGIN_METHOD_MAX_LEN) {
            return Err(TaskError::InvalidKind(format!("plugin:{id}:{method}")));
        }
        if reserved_method(&method) {
            return Err(TaskError::InvalidKind(format!(
                "插件任务不能调用调度器自身入口：{method}"
            )));
        }
        Ok(Self::Plugin { id, method })
    }

    pub fn parse(s: &str) -> Result<Self, TaskError> {
        if let Some(name) = s.strip_prefix("builtin:") {
            return Self::builtin(name);
        }
        if let Some(rest) = s.strip_prefix("plugin:") {
            if let Some((id, method)) = rest.rsplit_once(':') {
                return Self::plugin(id, method);
            }
        }
        Err(TaskError::InvalidKind(s.to_string()))
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin { .. })
    }

    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            Self::Plugin { id, .. } => Some(id),
            Self::Builtin { .. } => None,
        }
    }

    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Plugin { method, .. } => Some(method),
            Self::Builtin { .. } => None,
        }
    }

    pub fn to_string_value(&self) -> String {
        match self {
            Self::Builtin { name } => format!("builtin:{name}"),
            Self::Plugin { id, method } => format!("plugin:{id}:{method}"),
        }
    }
}

/// 调度器自身的 RPC 入口不允许被任务当成执行体，避免自调死循环。
fn reserved_method(method: &str) -> bool {
    method == "scheduler" || method.starts_with("task.") || method.starts_with("buffer.")
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_value())
    }
}

impl FromStr for TaskKind {
    type Err = TaskError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for TaskKind {
    type Error = TaskError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TaskKind> for String {
    fn from(value: TaskKind) -> Self {
        value.to_string_value()
    }
}

/// 任务作用域：站点 / 容器 / 插件 / 系统。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum TaskSource {
    Site(String),
    Docker(String),
    Plugin(String),
    System,
}

impl TaskSource {
    pub fn parse(s: &str) -> Result<Self, TaskError> {
        if s == "system" {
            return Ok(Self::System);
        }
        if let Some(v) = s.strip_prefix("site:") {
            return scope(Self::Site, v, s);
        }
        if let Some(v) = s.strip_prefix("docker:") {
            return scope(Self::Docker, v, s);
        }
        if let Some(v) = s.strip_prefix("plugin:") {
            if !is_token(v, PLUGIN_METHOD_MAX_LEN) {
                return Err(TaskError::InvalidSource(s.to_string()));
            }
            return Ok(Self::Plugin(v.to_string()));
        }
        Err(TaskError::InvalidSource(s.to_string()))
    }

    pub fn to_string_value(&self) -> String {
        match self {
            Self::Site(v) => format!("site:{v}"),
            Self::Docker(v) => format!("docker:{v}"),
            Self::Plugin(v) => format!("plugin:{v}"),
            Self::System => "system".to_string(),
        }
    }

    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            Self::Plugin(id) => Some(id),
            _ => None,
        }
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self, Self::Plugin(_))
    }
}

fn scope(make: fn(String) -> TaskSource, value: &str, raw: &str) -> Result<TaskSource, TaskError> {
    if !is_scope_value(value) {
        return Err(TaskError::InvalidSource(raw.to_string()));
    }
    Ok(make(value.to_string()))
}

impl fmt::Display for TaskSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_value())
    }
}

impl FromStr for TaskSource {
    type Err = TaskError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for TaskSource {
    type Error = TaskError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<TaskSource> for String {
    fn from(value: TaskSource) -> Self {
        value.to_string_value()
    }
}

/// cron 时区：P1 只有宿主本地一种，显式建模是为了让未来扩展不改变 JSON 结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronTz {
    #[default]
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Epoch,
}

/// 调度计划：5 段 cron 或固定间隔。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    Cron {
        expr: String,
        #[serde(default)]
        tz: CronTz,
        #[serde(default)]
        jitter_secs: u64,
    },
    Interval {
        secs: u64,
        #[serde(default)]
        align: Align,
    },
}

impl Schedule {
    pub fn cron(expr: impl Into<String>) -> Self {
        Self::Cron {
            expr: expr.into(),
            tz: CronTz::Local,
            jitter_secs: 0,
        }
    }

    pub fn interval(secs: u64) -> Self {
        Self::Interval {
            secs,
            align: Align::Epoch,
        }
    }

    pub fn validate(&self) -> Result<(), TaskError> {
        match self {
            Self::Cron {
                expr, jitter_secs, ..
            } => {
                if skiff_cron::CronExpr::parse(expr).is_none() {
                    return Err(TaskError::InvalidSchedule(format!(
                        "cron 表达式无法解析：{expr}"
                    )));
                }
                if *jitter_secs > MAX_JITTER_SECS {
                    return Err(TaskError::InvalidSchedule(format!(
                        "jitter_secs 必须在 0–{MAX_JITTER_SECS}：{jitter_secs}"
                    )));
                }
            }
            Self::Interval { secs, .. } => {
                if *secs < MIN_INTERVAL_SECS {
                    return Err(TaskError::InvalidSchedule(format!(
                        "interval 最小 {MIN_INTERVAL_SECS}s：{secs}"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn jitter_secs(&self) -> u64 {
        match self {
            Self::Cron { jitter_secs, .. } => *jitter_secs,
            Self::Interval { .. } => 0,
        }
    }
}

/// 重试策略。只有瞬时错误（spawn / timeout / unavailable）允许重试。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(default)]
    pub max: u32,
    #[serde(default)]
    pub backoff_secs: Vec<u64>,
    #[serde(default)]
    pub on: Vec<ErrorClass>,
}

impl RetryPolicy {
    pub fn with_backoff(max: u32, backoff_secs: Vec<u64>) -> Self {
        Self {
            max,
            backoff_secs,
            on: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), TaskError> {
        if self.max > MAX_RETRIES {
            return Err(TaskError::InvalidRetry(format!(
                "max 不能超过 {MAX_RETRIES}"
            )));
        }
        for secs in &self.backoff_secs {
            if *secs == 0 || *secs > 3600 {
                return Err(TaskError::InvalidRetry(format!(
                    "backoff_secs 必须在 1–3600：{secs}"
                )));
            }
        }
        for class in &self.on {
            if !class.is_transient() {
                return Err(TaskError::InvalidRetry(format!(
                    "{} 不是瞬时错误，不能配置重试",
                    class.as_str()
                )));
            }
        }
        Ok(())
    }

    /// 第 `retry_index` 次重试的退避秒数（0 表示第一次重试）；没配时用 30s。
    pub fn backoff_for(&self, retry_index: usize) -> i64 {
        self.backoff_secs
            .get(retry_index)
            .copied()
            .unwrap_or(DEFAULT_TIMEOUT_SECS) as i64
    }

    pub fn allows(&self, class: ErrorClass) -> bool {
        if self.on.is_empty() {
            class.is_transient()
        } else {
            self.on.contains(&class)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Concurrency {
    #[default]
    Skip,
    QueueOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedPolicy {
    #[default]
    Skip,
    Once,
    All,
}

/// blocked 的原因必须能落到列表页的修复建议（契约 §11.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    NotInstalled,
    PluginDisabled,
    ApiMismatch,
    InvalidSchedule,
    ClockSkew,
    MissingExecutor,
}

impl BlockedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockedReason::NotInstalled => "not_installed",
            BlockedReason::PluginDisabled => "plugin_disabled",
            BlockedReason::ApiMismatch => "api_mismatch",
            BlockedReason::InvalidSchedule => "invalid_schedule",
            BlockedReason::ClockSkew => "clock_skew",
            BlockedReason::MissingExecutor => "missing_executor",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            BlockedReason::NotInstalled => "插件未安装：安装对应插件后自动恢复",
            BlockedReason::PluginDisabled => "插件已停用：在插件中心启用后自动恢复",
            BlockedReason::ApiMismatch => "插件 API 与内核不兼容：升级插件或内核",
            BlockedReason::InvalidSchedule => "调度表达式非法：修改 schedule 后重新启用",
            BlockedReason::ClockSkew => "系统时钟回拨：时钟恢复后自动恢复",
            BlockedReason::MissingExecutor => "执行体缺失：重新注册任务",
        }
    }
}

/// 生命期状态（落库字段）：是否正常、被阻塞、被系统停用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Blocked,
    DisabledBySystem,
}

/// 运行态状态机（契约要求 idle/queued/running/blocked/failed/disabled）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Idle,
    Queued,
    Running,
    Blocked,
    Failed,
    Disabled,
}

impl TaskState {
    pub fn can_transition_to(self, to: Self) -> bool {
        if self == to {
            return true;
        }
        matches!(
            (self, to),
            (TaskState::Idle, TaskState::Queued)
                | (TaskState::Idle, TaskState::Running)
                | (TaskState::Idle, TaskState::Blocked)
                | (TaskState::Idle, TaskState::Disabled)
                | (TaskState::Queued, TaskState::Running)
                | (TaskState::Queued, TaskState::Idle)
                | (TaskState::Queued, TaskState::Blocked)
                | (TaskState::Queued, TaskState::Disabled)
                | (TaskState::Running, TaskState::Idle)
                | (TaskState::Running, TaskState::Failed)
                | (TaskState::Running, TaskState::Blocked)
                | (TaskState::Running, TaskState::Disabled)
                | (TaskState::Running, TaskState::Queued)
                | (TaskState::Failed, TaskState::Idle)
                | (TaskState::Failed, TaskState::Queued)
                | (TaskState::Failed, TaskState::Running)
                | (TaskState::Failed, TaskState::Blocked)
                | (TaskState::Failed, TaskState::Disabled)
                | (TaskState::Blocked, TaskState::Idle)
                | (TaskState::Blocked, TaskState::Queued)
                | (TaskState::Blocked, TaskState::Disabled)
                | (TaskState::Disabled, TaskState::Idle)
        )
    }

    pub fn transition(self, to: Self) -> Result<Self, TaskError> {
        if self.can_transition_to(to) {
            Ok(to)
        } else {
            Err(TaskError::InvalidTransition { from: self, to })
        }
    }
}

/// 任务配置（契约 §3 的 JSON 主体）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: TaskId,
    pub kind: TaskKind,
    pub source: TaskSource,
    pub schedule: Schedule,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub payload: Value,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub retry: RetryPolicy,
    #[serde(default)]
    pub concurrency: Concurrency,
    #[serde(default)]
    pub missed: MissedPolicy,
    #[serde(default)]
    pub buffer: Option<BufferConfig>,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_true() -> bool {
    true
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

impl TaskSpec {
    pub fn new(
        id: impl Into<String>,
        kind: TaskKind,
        source: TaskSource,
        schedule: Schedule,
    ) -> Result<Self, TaskError> {
        Ok(Self {
            id: TaskId::new(id)?,
            kind,
            source,
            schedule,
            enabled: true,
            payload: Value::Null,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            retry: RetryPolicy::default(),
            concurrency: Concurrency::Skip,
            missed: MissedPolicy::Skip,
            buffer: None,
            created_by: "system".to_string(),
            created_at: 0,
            updated_at: 0,
        })
    }

    pub fn validate(&self) -> Result<(), TaskError> {
        self.schedule.validate()?;
        if self.timeout_secs == 0 || self.timeout_secs > MAX_TIMEOUT_SECS {
            return Err(TaskError::InvalidTimeout(self.timeout_secs));
        }
        self.retry.validate()?;
        if let Some(buffer) = &self.buffer {
            buffer.validate()?;
        }
        let payload_bytes = serde_json::to_vec(&self.payload)
            .map_err(|e| TaskError::InvalidPayload(e.to_string()))?
            .len();
        if payload_bytes > PAYLOAD_MAX_BYTES {
            return Err(TaskError::InvalidPayload(format!(
                "payload 序列化后 {payload_bytes} B，超过 {PAYLOAD_MAX_BYTES} B"
            )));
        }
        if self.missed == MissedPolicy::All && (!self.kind.is_builtin() || self.retry.max > 0) {
            return Err(TaskError::InvalidRetry(
                "missed=all 仅允许内置任务且关闭重试".to_string(),
            ));
        }
        if let TaskKind::Plugin { id, .. } = &self.kind {
            match &self.source {
                TaskSource::Plugin(source_id) if source_id == id => {}
                other => {
                    return Err(TaskError::InvalidSource(format!(
                        "插件任务的 source 必须为 plugin:{id}，实际为 {other}"
                    )))
                }
            }
        }
        Ok(())
    }

    /// 插件任务所属插件；内置任务或无插件作用域返回 `None`。
    pub fn plugin_id(&self) -> Option<&str> {
        self.kind.plugin_id().or_else(|| self.source.plugin_id())
    }
}

/// 存储层的一条任务记录：配置 + 生命期状态 + 最近一次结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    #[serde(flatten)]
    pub spec: TaskSpec,
    #[serde(default = "default_lifecycle")]
    pub state: LifecycleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<BlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRun>,
}

fn default_lifecycle() -> LifecycleState {
    LifecycleState::Active
}

impl Task {
    pub fn new(spec: TaskSpec) -> Self {
        Self {
            spec,
            state: LifecycleState::Active,
            blocked_reason: None,
            last_run: None,
        }
    }

    pub fn id(&self) -> &TaskId {
        &self.spec.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastRun {
    pub at: i64,
    pub result: RunResult,
    pub duration_ms: i64,
    pub trigger: Trigger,
}

/// 修改补丁：只允许改调度参数与开关，不允许改 id/source（契约 §10.2）。
///
/// `kind` 只给内核补种用（manifest `[[task]]` 改了 `method` 时要同步目标方法）；
/// 插件 RPC 入口 `task.update` 显式拒绝该字段，不得由插件改派自己的执行体。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TaskKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Concurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missed: Option<MissedPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer: Option<BufferConfig>,
    /// 显式移除缓冲；与 `buffer: Some(...)` 互斥（后者的优先级更高）。
    #[serde(default)]
    pub clear_buffer: bool,
}

impl TaskPatch {
    /// 把补丁合入 spec，返回是否真的发生了变化。
    pub fn apply_to(&self, spec: &mut TaskSpec) -> bool {
        let mut changed = false;
        if let Some(kind) = &self.kind {
            if spec.kind != *kind {
                spec.kind = kind.clone();
                changed = true;
            }
        }
        if let Some(schedule) = &self.schedule {
            if spec.schedule != *schedule {
                spec.schedule = schedule.clone();
                changed = true;
            }
        }
        if let Some(enabled) = self.enabled {
            if spec.enabled != enabled {
                spec.enabled = enabled;
                changed = true;
            }
        }
        if let Some(payload) = &self.payload {
            if spec.payload != *payload {
                spec.payload = payload.clone();
                changed = true;
            }
        }
        if let Some(timeout) = self.timeout_secs {
            if spec.timeout_secs != timeout {
                spec.timeout_secs = timeout;
                changed = true;
            }
        }
        if let Some(retry) = &self.retry {
            if spec.retry != *retry {
                spec.retry = retry.clone();
                changed = true;
            }
        }
        if let Some(concurrency) = self.concurrency {
            if spec.concurrency != concurrency {
                spec.concurrency = concurrency;
                changed = true;
            }
        }
        if let Some(missed) = self.missed {
            if spec.missed != missed {
                spec.missed = missed;
                changed = true;
            }
        }
        if let Some(buffer) = &self.buffer {
            if spec.buffer.as_ref() != Some(buffer) {
                spec.buffer = Some(buffer.clone());
                changed = true;
            }
        } else if self.clear_buffer && spec.buffer.is_some() {
            spec.buffer = None;
            changed = true;
        }
        changed
    }
}

/// 列表过滤条件；所有字段为空表示不过滤。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskFilter {
    pub source: Option<TaskSource>,
    pub kind: Option<TaskKind>,
    pub enabled: Option<bool>,
    pub state: Option<LifecycleState>,
}

impl TaskFilter {
    pub fn matches(&self, task: &Task) -> bool {
        self.source
            .as_ref()
            .map(|v| v == &task.spec.source)
            .unwrap_or(true)
            && self
                .kind
                .as_ref()
                .map(|v| v == &task.spec.kind)
                .unwrap_or(true)
            && self.enabled.map(|v| v == task.spec.enabled).unwrap_or(true)
            && self.state.map(|v| v == task.state).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::buffer::OverflowPolicy;

    fn spec() -> TaskSpec {
        TaskSpec::new(
            "builtin:site-backup:example.com",
            TaskKind::builtin("site-backup").unwrap(),
            TaskSource::parse("site:example.com").unwrap(),
            Schedule::cron("30 3 * * *"),
        )
        .unwrap()
    }

    #[test]
    fn id_kind_source_roundtrip() {
        let id = TaskId::new("plugin:com.skiff.monitor:sample").unwrap();
        assert_eq!(id.as_str(), "plugin:com.skiff.monitor:sample");
        assert!(TaskId::new("").is_err());
        assert!(TaskId::new("has space").is_err());
        assert!(TaskId::new("x".repeat(97)).is_err());

        let kind = TaskKind::parse("plugin:com.skiff.monitor:sample").unwrap();
        assert_eq!(kind.plugin_id(), Some("com.skiff.monitor"));
        assert_eq!(kind.method(), Some("sample"));
        assert_eq!(kind.to_string_value(), "plugin:com.skiff.monitor:sample");
        assert!(TaskKind::parse("plugin:com.skiff.monitor:task.register").is_err());
        assert!(TaskKind::parse("nonsense").is_err());

        let source = TaskSource::parse("docker:web-1").unwrap();
        assert_eq!(source.to_string_value(), "docker:web-1");
        assert_eq!(TaskSource::parse("system").unwrap(), TaskSource::System);
        assert!(TaskSource::parse("site:").is_err());
    }

    #[test]
    fn interval_below_five_seconds_is_rejected() {
        let mut s = spec();
        s.schedule = Schedule::interval(4);
        assert!(matches!(s.validate(), Err(TaskError::InvalidSchedule(_))));
        s.schedule = Schedule::interval(5);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn payload_and_timeout_limits() {
        let mut s = spec();
        s.payload = json!({ "data": "x".repeat(5000) });
        assert!(matches!(s.validate(), Err(TaskError::InvalidPayload(_))));
        s.payload = json!({ "keep": 7 });
        s.timeout_secs = 601;
        assert!(matches!(s.validate(), Err(TaskError::InvalidTimeout(601))));
    }

    #[test]
    fn missed_all_only_for_builtin_without_retry() {
        let mut s = spec();
        s.missed = MissedPolicy::All;
        assert!(s.validate().is_ok());
        s.retry = RetryPolicy::with_backoff(1, vec![30]);
        assert!(s.validate().is_err());

        let plugin_spec = TaskSpec::new(
            "plugin:m:sample",
            TaskKind::plugin("m", "sample").unwrap(),
            TaskSource::parse("plugin:m").unwrap(),
            Schedule::interval(60),
        )
        .unwrap();
        let mut plugin_spec = plugin_spec;
        plugin_spec.missed = MissedPolicy::All;
        assert!(plugin_spec.validate().is_err());
    }

    #[test]
    fn plugin_source_must_match_plugin_kind() {
        let mut s = TaskSpec::new(
            "plugin:m:sample",
            TaskKind::plugin("m", "sample").unwrap(),
            TaskSource::parse("system").unwrap(),
            Schedule::interval(60),
        )
        .unwrap();
        assert!(matches!(s.validate(), Err(TaskError::InvalidSource(_))));
        s.source = TaskSource::Plugin("m".to_string());
        assert!(s.validate().is_ok());
    }

    #[test]
    fn spec_serializes_to_contract_shape() {
        let mut s = spec();
        s.buffer = Some(BufferConfig {
            sample_method: Some("sample".to_string()),
            sample_interval_secs: 30,
            capacity_bytes: 4 * 1024 * 1024,
            overflow: OverflowPolicy::DropOldest,
            retain_across_ticks: true,
            aggregate_window_secs: 60,
            max_batch_bytes: 512 * 1024,
        });
        let value = serde_json::to_value(&s).unwrap();
        assert_eq!(value["id"], "builtin:site-backup:example.com");
        assert_eq!(value["kind"], "builtin:site-backup");
        assert_eq!(value["source"], "site:example.com");
        assert_eq!(value["schedule"]["type"], "cron");
        assert_eq!(value["buffer"]["overflow"], "drop_oldest");
        let back: TaskSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn task_state_machine_rejects_illegal_moves() {
        assert!(TaskState::Idle.transition(TaskState::Running).is_ok());
        assert!(TaskState::Running.transition(TaskState::Disabled).is_ok());
        assert!(TaskState::Disabled.transition(TaskState::Running).is_err());
        assert!(TaskState::Blocked.transition(TaskState::Running).is_err());
    }

    #[test]
    fn patch_only_touches_whitelisted_fields() {
        let mut s = spec();
        let patch = TaskPatch {
            schedule: Some(Schedule::interval(60)),
            enabled: Some(false),
            ..TaskPatch::default()
        };
        assert!(patch.apply_to(&mut s));
        assert_eq!(s.schedule, Schedule::interval(60));
        assert!(!s.enabled);
        assert!(!patch.apply_to(&mut s));
    }
}
