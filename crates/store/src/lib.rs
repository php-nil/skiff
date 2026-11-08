//! Skiff 存储层：SQLite 迁移与仓储。
//!
//! 设计约束（§5.2）：
//! - 唯一数据根下的 `skiff.db`，0600 root；插件降权后不可读。
//! - 迁移幂等、向前；启动自动执行并写 schema 版本（`PRAGMA user_version`）。
//! - 活动库禁止直接 tar 备份；备份走 `VACUUM INTO` / backup API（见 `Db::backup_to`）。

mod db;
mod error;
mod model;

pub use db::jobs;
pub use db::jobs::{
    is_active_status, JobLogBatch, JobLogRow, JobRow, LogAppend, JOB_KEEP_MAX, JOB_RETENTION_DAYS,
    MAX_LOG_BYTES, MAX_LOG_LINES,
};
pub use db::quotas::QuotaRow;
pub use db::runtimes::{
    validate_runtime_name, NewRuntime, Runtime, KIND_GO, KIND_NODE, KIND_PHP, KIND_RUST,
    ORIGIN_INSTANCE, ORIGIN_SEED,
};
pub use db::sites::{NewSite, Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};
pub use db::tasks::{ScheduledTaskRow, TaskRunRow};
pub use db::trash::TrashRow;
pub use db::{Db, CURRENT_SCHEMA_VERSION};
pub use error::StoreError;
pub use model::{AuditEntry, PluginRecord, Session, User};
// 角色/授权项定义在 skiff-model（零依赖，策略层可直接用）；这里转出保持既有 API
pub use skiff_model::{Grant, Role};

pub type Result<T> = std::result::Result<T, StoreError>;
