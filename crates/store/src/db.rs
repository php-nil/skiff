//! SQLite 连接、迁移与备份。按聚合拆分的仓储方法在 `db/` 子模块：
//! settings / users / backup_codes / sessions / tokens / plugins / trust / audit。
//!
//! 设计约束（§5.2）：
//! - 单连接 + 互斥。Skiff 的写入是低频控制面操作（登录、审计、插件状态），
//!   单连接足以覆盖 MVP 并发；读多写少场景下 WAL 已足够。
//! - 迁移幂等、向前；启动自动执行并写 schema 版本（`PRAGMA user_version`）。
//! - 活动库禁止直接 tar 备份；备份走 `VACUUM INTO`（见 `Db::backup_to`）。
//! - **持锁禁止再进 `self.xxx()`**：`std::sync::Mutex` 不可重入，会永久卡死。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use tracing::{info, warn};

use crate::error::StoreError;
use crate::Result;

mod audit;
mod backup_codes;
mod plugins;
mod prefs;
pub mod runtimes;
mod sessions;
mod settings;
pub mod sites;
pub mod tasks;
mod tokens;
mod trust;
mod users;

/// 当前二进制支持的 schema 版本。迁移只向前，版本高于此值拒绝启动。
pub const CURRENT_SCHEMA_VERSION: i64 = 10;

pub struct Db {
    conn: Mutex<Connection>,
    path: PathBuf,
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
        let db = Db {
            conn: Mutex::new(conn),
            path,
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        let db = Db {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        };
        db.migrate()?;
        Ok(db)
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

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // 单连接 + 短事务；锁中毒意味着有 handler panic，直接取回内部值继续，
        // 避免一次 panic 让整个面板永久 500。
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)?;
            conn.execute_batch("PRAGMA user_version = 1")?;
            info!(from = version, to = 1, "schema 迁移完成");
        }
        if version < 2 {
            conn.execute_batch(SCHEMA_V2)?;
            conn.execute_batch("PRAGMA user_version = 2")?;
            info!(from = version, to = 2, "schema 迁移完成");
        }
        if version < 3 {
            conn.execute_batch(SCHEMA_V3)?;
            conn.execute_batch("PRAGMA user_version = 3")?;
            info!(from = version, to = 3, "schema 迁移完成");
        }
        if version < 4 {
            conn.execute_batch(SCHEMA_V4)?;
            conn.execute_batch("PRAGMA user_version = 4")?;
            info!(from = version, to = 4, "schema 迁移完成");
        }
        if version < 5 {
            conn.execute_batch(SCHEMA_V5)?;
            conn.execute_batch("PRAGMA user_version = 5")?;
            info!(from = version, to = 5, "schema 迁移完成");
        }
        if version < 6 {
            conn.execute_batch(SCHEMA_V6)?;
            conn.execute_batch("PRAGMA user_version = 6")?;
            info!(from = version, to = 6, "schema 迁移完成");
        }
        if version < 7 {
            conn.execute_batch(SCHEMA_V7)?;
            conn.execute_batch("PRAGMA user_version = 7")?;
            info!(from = version, to = 7, "schema 迁移完成");
        }
        if version < 8 {
            conn.execute_batch(SCHEMA_V8)?;
            conn.execute_batch("PRAGMA user_version = 8")?;
            info!(from = version, to = 8, "schema 迁移完成");
        }
        if version < 9 {
            conn.execute_batch(SCHEMA_V9)?;
            conn.execute_batch("PRAGMA user_version = 9")?;
            info!(from = version, to = 9, "schema 迁移完成");
        }
        if version < 10 {
            conn.execute_batch(SCHEMA_V10)?;
            conn.execute_batch("PRAGMA user_version = 10")?;
            info!(from = version, to = 10, "schema 迁移完成");
        }
        Ok(())
    }

    /// 一致性快照备份（§5.2）：用 `VACUUM INTO`，禁止对活动库直接 tar。
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<()> {
        let dest = dest.as_ref();
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        let conn = self.conn();
        let dest_str = dest.to_string_lossy().to_string();
        conn.execute("VACUUM INTO ?1", params![dest_str])?;
        let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(StoreError::Decode(format!("备份库完整性检查失败: {check}")));
        }
        Ok(())
    }
}

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  username TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL CHECK(role IN ('admin','operator','viewer')),
  totp_secret TEXT,
  totp_enabled INTEGER NOT NULL DEFAULT 0,
  must_change_password INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS backup_codes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  code_hash TEXT NOT NULL,
  used INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_backup_codes_user ON backup_codes(user_id);

CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  token_hash TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  last_seen INTEGER NOT NULL,
  idle_expires INTEGER NOT NULL,
  abs_expires INTEGER NOT NULL,
  ip TEXT NOT NULL,
  user_agent TEXT NOT NULL DEFAULT '',
  revoked INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_token ON sessions(token_hash);

CREATE TABLE IF NOT EXISTS api_tokens (
  id TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  token_hash TEXT NOT NULL UNIQUE,
  scopes TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  last_used INTEGER,
  revoked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS trusted_keys (
  key_id TEXT PRIMARY KEY,
  public_key TEXT NOT NULL,
  label TEXT NOT NULL DEFAULT '',
  added_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugins (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  api TEXT NOT NULL,
  key_id TEXT NOT NULL,
  manifest TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'installed',
  status_detail TEXT NOT NULL DEFAULT '',
  active_version TEXT NOT NULL,
  installed_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin_grants (
  plugin_id TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  value TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (plugin_id, kind, value)
);

CREATE TABLE IF NOT EXISTS plugin_config (
  plugin_id TEXT NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  secret INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (plugin_id, key)
);

CREATE TABLE IF NOT EXISTS audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ts INTEGER NOT NULL,
  actor TEXT NOT NULL,
  ip TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT NOT NULL DEFAULT '',
  result TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(ts DESC);
"#;

/// v2：按用户的界面偏好（菜单顺序/隐藏/固定/布局）。
///
/// 放在独立表而不是 `settings`：`settings` 是全局键值，菜单偏好必须按用户隔离；
/// 卸载插件时只清 `key = 'menu.prefs'` 的行，不碰全局配置。
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS user_prefs (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, key)
);
CREATE INDEX IF NOT EXISTS idx_user_prefs_key ON user_prefs(key);
"#;

/// v3：内核自带「网站」的站点表。
///
/// 域名做唯一约束（同一域名只能有一个 vhost）；根目录与 PHP 版本是站点级配置，
/// 运行状态只读容器/配置推导后落库，方便列表页直接展示而不每次 inspect。
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS sites (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  domain TEXT NOT NULL UNIQUE,
  root_dir TEXT NOT NULL,
  php_version TEXT NOT NULL DEFAULT '8.5',
  status TEXT NOT NULL DEFAULT 'running'
    CHECK(status IN ('running','stopped','error')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sites_created ON sites(created_at DESC);
"#;

/// v4：站点「默认站点」标记。
///
/// 状态只存库（`is_default`），vhost 文件是它的投影；部分唯一索引保证
/// 同一时间最多一个默认站点，切换时由 `set_site_default` 在同一语句内清旧置新。
const SCHEMA_V4: &str = r#"
ALTER TABLE sites ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_default ON sites(is_default) WHERE is_default = 1;
"#;

// v5：站点别名。独立小表，不改 `sites` 表结构；站点删除时级联清理。
const SCHEMA_V5: &str = r#"
CREATE TABLE IF NOT EXISTS site_aliases (
  domain TEXT PRIMARY KEY,
  site_domain TEXT NOT NULL REFERENCES sites(domain) ON DELETE CASCADE,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_site_aliases_site ON site_aliases(site_domain);
"#;

// v6：站点 HTTPS 状态。证书文件落在 `/www/sites/<域名>/ssl`，库里只存开关；
// `https` 表示启用 443，`force_https` 表示 80 强制跳转 443。
const SCHEMA_V6: &str = r#"
ALTER TABLE sites ADD COLUMN https INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sites ADD COLUMN force_https INTEGER NOT NULL DEFAULT 0;
"#;

/// v7：运行环境单一事实源 + 三类站点。
///
/// 测试期直接落新结构：`sites` 与 `site_aliases` 重建，不再保留旧 `php_version`
/// 字段，也不写逐行搬迁逻辑（旧站点数据不跨版本保留）。
///
/// `site_runtimes` 的约束：
/// - `(kind, version)` 唯一；`id = <kind>-<version>` 由服务层保证；
/// - 同一 `kind` 至多一个默认版本（部分唯一索引）；
/// - `socket_path` 仅 PHP 有意义，vhost 由它换算容器路径。
const SCHEMA_V7: &str = r#"
CREATE TABLE IF NOT EXISTS site_runtimes (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('php','node','go')),
  version TEXT NOT NULL,
  image TEXT NOT NULL,
  image_digest TEXT NOT NULL DEFAULT '',
  desired_state TEXT NOT NULL DEFAULT 'running'
    CHECK(desired_state IN ('running','stopped')),
  last_status TEXT NOT NULL DEFAULT 'stopped'
    CHECK(last_status IN ('installing','running','stopped','error')),
  socket_path TEXT,
  is_default INTEGER NOT NULL DEFAULT 0,
  config_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(kind, version)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_default
  ON site_runtimes(kind) WHERE is_default = 1;

-- 旧站点/别名表整表重建（测试期，不做旧布局迁移）。
DROP TABLE IF EXISTS site_aliases;
DROP TABLE IF EXISTS sites;

CREATE TABLE sites (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  domain TEXT NOT NULL UNIQUE,
  root_dir TEXT NOT NULL,
  site_type TEXT NOT NULL DEFAULT 'runtime'
    CHECK(site_type IN ('runtime','static','proxy')),
  runtime_id TEXT REFERENCES site_runtimes(id) ON DELETE RESTRICT,
  app_command TEXT,
  app_port INTEGER,
  proxy_target TEXT,
  proxy_websocket INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'running'
    CHECK(status IN ('running','stopped','error')),
  is_default INTEGER NOT NULL DEFAULT 0,
  https INTEGER NOT NULL DEFAULT 0,
  force_https INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  -- runtime 必须有环境；static/proxy 不得有环境。
  CHECK(
    (site_type = 'runtime' AND runtime_id IS NOT NULL)
    OR (site_type = 'static' AND runtime_id IS NULL)
    OR (site_type = 'proxy' AND runtime_id IS NULL AND proxy_target IS NOT NULL)
  ),
  -- 应用进程字段只属于 runtime 站点（P2 Node/Go）。
  CHECK(site_type = 'runtime' OR (app_command IS NULL AND app_port IS NULL)),
  -- 上游地址只属于 proxy。
  CHECK(site_type = 'proxy' OR proxy_target IS NULL)
);
CREATE INDEX IF NOT EXISTS idx_sites_created ON sites(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_sites_runtime ON sites(runtime_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_default ON sites(is_default) WHERE is_default = 1;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_app_port ON sites(app_port) WHERE app_port IS NOT NULL;

CREATE TABLE site_aliases (
  domain TEXT PRIMARY KEY,
  site_domain TEXT NOT NULL REFERENCES sites(domain) ON DELETE CASCADE,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_site_aliases_site ON site_aliases(site_domain);

-- 种入 P1 唯一的 PHP 运行环境；镜像与 socket 由服务层按当前 web_root 校准。
INSERT OR IGNORE INTO site_runtimes(id, kind, version, image, image_digest, desired_state,
  last_status, socket_path, is_default, config_json, created_at, updated_at)
VALUES('php-8.5', 'php', '8.5', 'php:8.5-fpm-alpine', '', 'running', 'stopped',
  '/www/php/php-fpm.sock', 1, '{}',
  CAST(strftime('%s','now') AS INTEGER), CAST(strftime('%s','now') AS INTEGER));
"#;

/// v8：运行环境引入实例名 `name`（路径与容器名的唯一来源）。
///
/// `name` 与 `version` 解耦：`version` 表示镜像/上游版本，`name` 是本地实例标识。
/// 同一 kind 内 `name` 唯一；**不再限制同 kind 同版本**（`php-8.5-a` + `8.5` 合法）。
/// 旧库按 `id` 回填（内置 `php-8.5` 即 `name=php-8.5`），并把它的 socket 收敛到
/// `/www/runtime/php/<name>/php-fpm.sock`。
const SCHEMA_V8: &str = r#"
PRAGMA foreign_keys=OFF;
CREATE TABLE site_runtimes_v8 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('php','node','go')),
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  image TEXT NOT NULL,
  image_digest TEXT NOT NULL DEFAULT '',
  desired_state TEXT NOT NULL DEFAULT 'running'
    CHECK(desired_state IN ('running','stopped')),
  last_status TEXT NOT NULL DEFAULT 'stopped'
    CHECK(last_status IN ('installing','running','stopped','error')),
  socket_path TEXT,
  is_default INTEGER NOT NULL DEFAULT 0,
  config_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(kind, name)
);
INSERT INTO site_runtimes_v8(id, kind, name, version, image, image_digest, desired_state,
  last_status, socket_path, is_default, config_json, created_at, updated_at)
SELECT id, kind, id, version, image, image_digest, desired_state, last_status,
  CASE WHEN kind = 'php' AND socket_path IS NOT NULL
       THEN '/www/runtime/php/' || id || '/php-fpm.sock'
       ELSE socket_path END,
  is_default, config_json, created_at, updated_at
FROM site_runtimes;
DROP TABLE site_runtimes;
ALTER TABLE site_runtimes_v8 RENAME TO site_runtimes;
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_default
  ON site_runtimes(kind) WHERE is_default = 1;
PRAGMA foreign_keys=ON;
"#;

/// v9：运行环境已启用的 PHP 扩展（逗号分隔的 catalog key）。
///
/// 扩展清单本身是代码里的单一来源（`skiff-app::runtime_catalog`），库里只存
/// 「这个实例勾了哪些」。重建容器时按此重放，新增/删除后由服务层写回。
const SCHEMA_V9: &str = r#"
ALTER TABLE site_runtimes ADD COLUMN extensions TEXT NOT NULL DEFAULT '';
"#;

/// v10：内核定时任务。任务定义与运行历史分表；调度语义（cron 表达式、重试、
/// 缓冲配置）一律以 JSON 文本存放，存储层不解释。
///
/// - `scheduled_tasks`：`(kind, source)` 唯一——同一内置任务/站点/插件只能有一条；
///   插件任务用带 `local_key` 的 `id` 区分（`plugin:<id>:<local_key>`）。
/// - `task_runs`：每次执行一条，保留期由维护巡检按 7 天清理。
const SCHEMA_V10: &str = r#"
CREATE TABLE IF NOT EXISTS scheduled_tasks (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  source TEXT NOT NULL,
  schedule_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  payload_json TEXT NOT NULL DEFAULT '{}',
  timeout_secs INTEGER NOT NULL DEFAULT 30,
  retry_json TEXT NOT NULL DEFAULT '{}',
  concurrency TEXT NOT NULL DEFAULT 'skip',
  missed TEXT NOT NULL DEFAULT 'skip',
  buffer_json TEXT,
  lifecycle TEXT NOT NULL DEFAULT 'active',
  blocked_reason TEXT,
  last_run_json TEXT,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_scheduled_tasks_kind_source
  ON scheduled_tasks(kind, source);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_source
  ON scheduled_tasks(source);
CREATE INDEX IF NOT EXISTS idx_scheduled_tasks_enabled
  ON scheduled_tasks(enabled);

CREATE TABLE IF NOT EXISTS task_runs (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  trigger TEXT NOT NULL,
  attempt INTEGER NOT NULL DEFAULT 0,
  started_at INTEGER NOT NULL,
  finished_at INTEGER NOT NULL,
  result TEXT NOT NULL,
  error_class TEXT,
  error_detail TEXT,
  duration_ms INTEGER NOT NULL,
  request_id TEXT,
  consumed_upto INTEGER,
  output_bytes INTEGER NOT NULL DEFAULT 0,
  missed_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_task_runs_task
  ON task_runs(task_id, finished_at DESC);
CREATE INDEX IF NOT EXISTS idx_task_runs_finished
  ON task_runs(finished_at DESC);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::Role;

    #[test]
    fn migration_is_idempotent_and_records_version() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.migrate().is_ok());
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);
    }

    /// 迁移链落新结构：旧站点表被重建，同时种入内置 PHP 运行环境（name 即标识）。
    #[test]
    fn migrations_rebuild_sites_and_seed_php_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute_batch(SCHEMA_V5).unwrap();
            conn.execute_batch(SCHEMA_V6).unwrap();
            conn.execute_batch("PRAGMA user_version = 6").unwrap();
            conn.execute(
                "INSERT INTO sites(domain, root_dir, php_version, status, created_at, updated_at)
                 VALUES('legacy.local', '/www/sites/legacy.local', '8.5', 'running', 1, 1)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);
        assert!(db.site_by_domain("legacy.local").unwrap().is_none());
        let seeded = db.runtime_by_id("php-8.5").unwrap().unwrap();
        assert_eq!(seeded.kind, "php");
        assert_eq!(seeded.name, "php-8.5");
        assert!(seeded.is_default);
        assert_eq!(
            seeded.socket_path.as_deref(),
            Some("/www/runtime/php/php-8.5/php-fpm.sock")
        );
    }

    #[test]
    fn backup_is_consistent_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("skiff.db");
        let db = Db::open(&db_path).unwrap();
        db.create_user("a", "h", Role::Admin, false).unwrap();
        let dest = dir.path().join("backup.db");
        db.backup_to(&dest).unwrap();
        assert!(dest.exists());
        let restored = Db::open(&dest).unwrap();
        assert_eq!(restored.count_users().unwrap(), 1);
    }
}
