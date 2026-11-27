//! SQLite 连接与锁车道。
//!
//! 本文件只装连接面：`open` / `configure` / `conn_lane` / `plugin_rev`。表级读写在
//! `db/` 各聚合子模块（清单以下面的 `mod` 声明为准，不在此重复列举——手维护的枚举
//! 会随拆分腐烂）；schema 迁移在 `db/migrations.rs`，DDL 正文在
//! `db/migrations/schema.rs`；备份与审计裁剪是长操作，走 `db/maintenance.rs`。
//!
//! 设计约束（§5.2）：
//! - 单连接 + 互斥。Skiff 的写入是低频控制面操作（登录、审计、插件状态），
//!   单连接足以覆盖 MVP 并发；读多写少场景下 WAL 已足够。
//! - 迁移幂等、向前；启动自动执行并写 schema 版本（`PRAGMA user_version`）。
//! - 活动库禁止直接 tar 备份；备份走 `VACUUM INTO`（见 `Db::backup_to`）。
//! - **持锁禁止再进 `self.xxx()`**：`std::sync::Mutex` 不可重入，会永久卡死。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tracing::warn;

use crate::Result;

mod audit;
mod backup_codes;
pub mod jobs;
pub mod maintenance;
mod migrations;
mod plugins;
mod prefs;
pub mod quotas;
pub mod runtimes;
mod sessions;
mod settings;
pub mod sites;
pub mod tasks;
mod tokens;
pub mod trash;
mod trust;
mod users;

#[cfg(test)]
mod tests;

/// 当前二进制支持的 schema 版本。迁移只向前，版本高于此值拒绝启动。
pub const CURRENT_SCHEMA_VERSION: i64 = 22;

pub struct Db {
    conn: Mutex<Connection>,
    path: PathBuf,
    /// 插件记录（含 grants / enabled）的写版本号。任何会改变插件鉴权输入的写入
    /// 都会自增，进程内授权快照据此失效；这样失效点不依赖调用方是否记得清缓存。
    plugin_rev: AtomicU64,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Db> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        Self::configure(&conn)?;
        migrations::apply(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
            path,
            plugin_rev: AtomicU64::new(0),
        })
    }

    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        migrations::apply(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
            plugin_rev: AtomicU64::new(0),
        })
    }

    fn configure(conn: &Connection) -> Result<()> {
        conn.busy_timeout(Duration::from_secs(5))?;
        // journal_mode 是返回行的 pragma，必须用 query_row。
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") && !mode.eq_ignore_ascii_case("memory") {
            warn!(mode = %mode, "WAL 未启用（可能是不支持的文件系统）");
        }
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 插件鉴权相关写入的版本号（进程内单调递增，不落库）。
    ///
    /// 授权快照把它当作缓存键：读到的 rev 与快照里的一致才复用。
    pub fn plugin_rev(&self) -> u64 {
        self.plugin_rev.load(Ordering::Acquire)
    }

    /// 插件记录/授权发生写入后调用。私有：只有本模块与子模块 `plugins` 会改这些表。
    fn bump_plugin_rev(&self) {
        self.plugin_rev.fetch_add(1, Ordering::AcqRel);
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn_lane("short")
    }

    /// 按调用车道取连接，并在等待超过阈值时留痕。
    ///
    /// `lane` 只有两个值：`short`（请求/短事务，经 `db_async` 投递）与
    /// `maintenance`（备份、审计裁剪等长操作，见 [`crate::maintenance`]）。
    /// 日志用于区分「谁在占锁」，不要把维护等待误算进短事务池。
    fn conn_lane(&self, lane: &'static str) -> std::sync::MutexGuard<'_, Connection> {
        // 单连接 + 短事务；锁中毒意味着有 handler panic，直接取回内部值继续，
        // 避免一次 panic 让整个面板永久 500。
        let started = Instant::now();
        let guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let waited = started.elapsed();
        if waited >= Duration::from_millis(50) {
            warn!(
                lane,
                waited_ms = waited.as_millis() as u64,
                "Db 连接等待超过 50ms"
            );
        }
        guard
    }
}
