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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use tracing::{info, warn};

use crate::error::StoreError;
use crate::Result;

mod audit;
mod backup_codes;
pub mod jobs;
pub mod maintenance;
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

/// 当前二进制支持的 schema 版本。迁移只向前，版本高于此值拒绝启动。
pub const CURRENT_SCHEMA_VERSION: i64 = 19;

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

    fn migrate(&self) -> Result<()> {
        let conn = self.conn();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
        let steps: [(i64, &str); 19] = [
            (1, SCHEMA_V1),
            (2, SCHEMA_V2),
            (3, SCHEMA_V3),
            (4, SCHEMA_V4),
            (5, SCHEMA_V5),
            (6, SCHEMA_V6),
            (7, SCHEMA_V7),
            (8, SCHEMA_V8),
            (9, SCHEMA_V9),
            (10, SCHEMA_V10),
            (11, SCHEMA_V11),
            (12, SCHEMA_V12),
            (13, SCHEMA_V13),
            (14, SCHEMA_V14),
            (15, SCHEMA_V15),
            (16, SCHEMA_V16),
            (17, SCHEMA_V17),
            (18, SCHEMA_V18),
            (19, SCHEMA_V19),
        ];
        for (to, schema) in steps {
            if version >= to {
                continue;
            }
            Self::migrate_step(&conn, version, to, schema)?;
        }
        Ok(())
    }

    /// 单步迁移：DDL 与 `user_version` 在同一事务内提交。
    ///
    /// 之前两条独立语句会让「列已加、版本未推进」成为可能，重启重跑
    /// `ALTER TABLE ADD COLUMN` 直接 `duplicate column name`，面板永久起不来。
    /// V8/V11 重建表需要在事务外关外键（`PRAGMA foreign_keys` 在事务内是 no-op）。
    fn migrate_step(conn: &Connection, from: i64, to: i64, schema: &str) -> Result<()> {
        let fk_off = matches!(to, 8 | 11 | 18);
        if fk_off {
            conn.execute_batch("PRAGMA foreign_keys=OFF")?;
        }
        let result = (|| -> Result<()> {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            conn.execute_batch(schema)?;
            conn.execute_batch(&format!("PRAGMA user_version = {to}"))?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        })();
        if let Err(e) = result {
            if let Err(re) = conn.execute_batch("ROLLBACK") {
                warn!(error = %re, to, "schema 迁移回滚失败");
            }
            if fk_off {
                let _ = conn.execute_batch("PRAGMA foreign_keys=ON");
            }
            return Err(e);
        }
        if fk_off {
            conn.execute_batch("PRAGMA foreign_keys=ON")?;
        }
        info!(from, to, "schema 迁移完成");
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
/// 同一时间最多一个默认站点，切换时由 `set_site_default` 在同一事务内先清旧再置新
/// （单条 `UPDATE ... CASE` 会因逐行检查 UNIQUE 索引而在「切回小 rowid」时失败）。
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
/// - `scheduled_tasks`：任务 id 唯一；插件任务的唯一性由 id 里的 `local_key` 保证，
///   不能给 `(kind, source)` 建唯一索引（同一插件/站点允许挂多条任务，见 v12）。
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

/// v11：运行环境新增 Rust 类型与固定端口列。
///
/// Rust 不像 PHP-FPM 有 unix socket：每个环境固定一个 host 网络端口
/// （`port`），站点 vhost 反代到它。kind 的 CHECK 约束必须整表重建才能扩到
/// `rust`，顺手补上 `port` 与既有 `extensions` 列。
const SCHEMA_V11: &str = r#"
PRAGMA foreign_keys=OFF;
CREATE TABLE site_runtimes_v11 (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('php','node','go','rust')),
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  image TEXT NOT NULL,
  image_digest TEXT NOT NULL DEFAULT '',
  desired_state TEXT NOT NULL DEFAULT 'running'
    CHECK(desired_state IN ('running','stopped')),
  last_status TEXT NOT NULL DEFAULT 'stopped'
    CHECK(last_status IN ('installing','running','stopped','error')),
  socket_path TEXT,
  port INTEGER,
  is_default INTEGER NOT NULL DEFAULT 0,
  config_json TEXT NOT NULL DEFAULT '{}',
  extensions TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(kind, name)
);
INSERT INTO site_runtimes_v11(id, kind, name, version, image, image_digest, desired_state,
  last_status, socket_path, port, is_default, config_json, extensions, created_at, updated_at)
SELECT id, kind, name, version, image, image_digest, desired_state, last_status,
  socket_path, NULL, is_default, config_json, extensions, created_at, updated_at
FROM site_runtimes;
DROP TABLE site_runtimes;
ALTER TABLE site_runtimes_v11 RENAME TO site_runtimes;
CREATE UNIQUE INDEX IF NOT EXISTS idx_runtime_default
  ON site_runtimes(kind) WHERE is_default = 1;
PRAGMA foreign_keys=ON;
"#;

/// v12：去掉 `scheduled_tasks(kind, source)` 唯一索引。
///
/// v10 建的该索引把「同一插件方法/同一站点只能有一条任务」写死，但插件任务的
/// 身份是 id 里的 `local_key`（`plugin:<id>:<local_key>`），同一 `(kind, source)`
/// 挂多条是合法配置（实测第二个任务被 `UNIQUE constraint failed` 拒绝）。
const SCHEMA_V12: &str = r#"
DROP INDEX IF EXISTS idx_scheduled_tasks_kind_source;
"#;

/// v13：标记「迁移种入、从未安装」的 PHP 占位行，列表不再展示这类种子。
///
/// 只有 `image_digest` 为空（从未成功安装）且无站点引用的种子行才降级为
/// `origin = 'seed'`；用户实际安装过或站点正在使用的行保持 `instance`，
/// 避免把真实实例误判成模板。
const SCHEMA_V13: &str = r#"
ALTER TABLE site_runtimes ADD COLUMN origin TEXT NOT NULL DEFAULT 'instance';
UPDATE site_runtimes
   SET origin = 'seed'
 WHERE kind = 'php'
   AND id IN ('php-8.5', 'php-8.4', 'php-8.3')
   AND name = id
   AND image_digest = ''
   AND config_json = '{}'
   AND extensions = ''
   AND NOT EXISTS (SELECT 1 FROM sites WHERE sites.runtime_id = site_runtimes.id);
UPDATE site_runtimes
   SET socket_path = NULL
 WHERE origin = 'seed';
"#;

/// v14：内核文件管理第二步——回收站元数据与目录树配额缓存。
///
/// 两表都由 `skiff-fs` 的 trait 适配器读写；`fs_quotas` 同时承载上限与增量
/// 缓存用量，`scanned_at` 记录最近一次全量重扫时间。
const SCHEMA_V14: &str = r#"
CREATE TABLE IF NOT EXISTS trash_entries (
  id TEXT PRIMARY KEY,
  actor_type TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  original_path TEXT NOT NULL,
  trash_path TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL,
  files INTEGER NOT NULL DEFAULT 0,
  mode INTEGER,
  owner TEXT,
  sha256 TEXT,
  site TEXT,
  protected INTEGER NOT NULL DEFAULT 0,
  deleted_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  nlink INTEGER NOT NULL DEFAULT 1,
  restored_at INTEGER,
  purged_at INTEGER,
  quota_root TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_trash_expires
  ON trash_entries(expires_at);
CREATE INDEX IF NOT EXISTS idx_trash_deleted
  ON trash_entries(deleted_at);
CREATE INDEX IF NOT EXISTS idx_trash_actor
  ON trash_entries(actor_type, actor_id, deleted_at);

CREATE TABLE IF NOT EXISTS fs_quotas (
  root_path TEXT PRIMARY KEY,
  max_bytes INTEGER,
  max_files INTEGER,
  used_bytes INTEGER NOT NULL DEFAULT 0,
  used_files INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  scanned_at INTEGER
);
"#;

/// v15：任务中心——一次性作业与过程日志。
///
/// `idx_jobs_active_target` 是部分唯一索引：同一目标同时只允许一个活跃作业，
/// 由 SQLite 原子保证，重启后仍有效。
const SCHEMA_V15: &str = r#"
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  target_type TEXT NOT NULL,
  target_id TEXT NOT NULL,
  title TEXT NOT NULL,
  params_json TEXT NOT NULL DEFAULT '{}',
  phase TEXT NOT NULL,
  phase_label TEXT NOT NULL,
  progress INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL CHECK(status IN ('queued','running','succeeded','failed','cancelled')),
  cancellable INTEGER NOT NULL DEFAULT 0,
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  created_by TEXT NOT NULL,
  created_by_role TEXT NOT NULL,
  requester_ip TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  started_at INTEGER,
  finished_at INTEGER,
  duration_ms INTEGER NOT NULL DEFAULT 0,
  error_class TEXT,
  error_detail TEXT,
  result_json TEXT NOT NULL DEFAULT '{}',
  log_bytes INTEGER NOT NULL DEFAULT 0,
  log_seq INTEGER NOT NULL DEFAULT 0,
  retry_of TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_jobs_active_target
  ON jobs(target_type, target_id) WHERE status IN ('queued','running');
CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, created_at DESC);

CREATE TABLE IF NOT EXISTS job_logs (
  job_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  at INTEGER NOT NULL,
  stream TEXT NOT NULL,
  line TEXT NOT NULL,
  PRIMARY KEY (job_id, seq)
);
"#;

/// v16：配额全量统计可能因条目数超过 `max_dir_entries` 而截断。
/// `truncated=1` 表示 `used_bytes/used_files` 只是下界（仍在增长），
/// 页面/API 必须标注「不完整」，写路径不得据此拒绝写入。
const SCHEMA_V16: &str = r#"
ALTER TABLE fs_quotas ADD COLUMN truncated INTEGER NOT NULL DEFAULT 0;
"#;

/// v17：任务中心详情要如实展示「退出码」。
///
/// 只有由单条命令决定成败的作业（如 PHP 扩展安装/卸载）才有确定退出码；
/// 其余作业（Docker API 拉起、多步骤安装）保持 NULL，页面显示 `-`，
/// 不用 0 冒充「成功」。
const SCHEMA_V17: &str = r#"
ALTER TABLE jobs ADD COLUMN exit_code INTEGER;
"#;

/// v18：删除站点表里从未使用的 `app_command` / `app_port` 死模型。
///
/// 不能直接 `ALTER TABLE ... DROP COLUMN`：这两列被旧的表级 CHECK 引用，
/// SQLite 会拒绝。这里按 v15 的整表重建方式重建 `sites`，数据原样搬移；
/// 重建期间关闭外键，避免 `DROP TABLE` 触发 `site_aliases` 的级联删除。
const SCHEMA_V18: &str = r#"
CREATE TABLE sites_v18 (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  domain TEXT NOT NULL UNIQUE,
  root_dir TEXT NOT NULL,
  site_type TEXT NOT NULL DEFAULT 'runtime'
    CHECK(site_type IN ('runtime','static','proxy')),
  runtime_id TEXT REFERENCES site_runtimes(id) ON DELETE RESTRICT,
  proxy_target TEXT,
  proxy_websocket INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'running'
    CHECK(status IN ('running','stopped','error')),
  is_default INTEGER NOT NULL DEFAULT 0,
  https INTEGER NOT NULL DEFAULT 0,
  force_https INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK(
    (site_type = 'runtime' AND runtime_id IS NOT NULL)
    OR (site_type = 'static' AND runtime_id IS NULL)
    OR (site_type = 'proxy' AND runtime_id IS NULL AND proxy_target IS NOT NULL)
  ),
  CHECK(site_type = 'proxy' OR proxy_target IS NULL)
);
INSERT INTO sites_v18(id, domain, root_dir, site_type, runtime_id, proxy_target,
                      proxy_websocket, status, is_default, https, force_https,
                      created_at, updated_at)
  SELECT id, domain, root_dir, site_type, runtime_id, proxy_target,
         proxy_websocket, status, is_default, https, force_https,
         created_at, updated_at
  FROM sites;
DROP TABLE sites;
ALTER TABLE sites_v18 RENAME TO sites;
CREATE INDEX IF NOT EXISTS idx_sites_created ON sites(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_sites_runtime ON sites(runtime_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_default ON sites(is_default) WHERE is_default = 1;
"#;

/// v19：列表 keyset 分页（SDUI D10）。
///
/// 游标翻页的排序键是 `(时间, id)` 两列，旧索引只有时间列，第二列要靠回表过滤。
/// 这里把三张索引直接扩成完整排序键（旧索引是新的前缀，保留纯属冗余）：
/// - 审计：`(ts DESC, id DESC)`；
/// - 作业列表：`(created_at DESC, id DESC)`；
/// - 作业按状态筛选：`(status, created_at DESC, id DESC)`。
///
/// 只改索引，不动表结构与既有查询。
const SCHEMA_V19: &str = r#"
DROP INDEX IF EXISTS idx_audit_ts;
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(ts DESC, id DESC);

DROP INDEX IF EXISTS idx_jobs_created;
CREATE INDEX IF NOT EXISTS idx_jobs_created ON jobs(created_at DESC, id DESC);

DROP INDEX IF EXISTS idx_jobs_status;
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, created_at DESC, id DESC);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrashRow;
    use crate::ORIGIN_SEED;
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

    /// 单步迁移失败时必须整体回滚：DDL 与 user_version 都不能留下。
    #[test]
    fn migration_step_rolls_back_ddl_and_version_on_failure() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn();
        let before: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        let err = Db::migrate_step(
            &conn,
            before,
            16,
            "CREATE TABLE migrate_probe(id INTEGER); INSERT INTO no_such_table VALUES(1);",
        );
        assert!(err.is_err());
        let after: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, before);
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'migrate_probe'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "失败步骤建的表必须被回滚");
    }

    #[test]
    fn backup_to_rejects_active_db_and_writes_valid_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        let db = Db::open(&path).unwrap();
        assert!(matches!(db.backup_to(&path), Err(StoreError::Conflict(_))));

        let dest = dir.path().join("snapshot.db");
        db.backup_to(&dest).unwrap();
        let conn = Connection::open(&dest).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);
    }

    /// 迁移链落新结构：旧站点表被重建，内置 PHP 行被标记为未安装种子。
    #[test]
    fn migrations_rebuild_sites_and_mark_php_seed() {
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
        assert_eq!(seeded.origin, ORIGIN_SEED);
        assert!(
            seeded.socket_path.is_none(),
            "从未安装的种子行不应登记 socket 路径"
        );
    }

    #[test]
    fn v13_database_upgrades_through_latest_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 13").unwrap();
        }
        let db = Db::open(&path).unwrap();
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, CURRENT_SCHEMA_VERSION);
        let exists = |name: &str| -> bool {
            db.conn()
                .query_row("SELECT 1 FROM sqlite_master WHERE name=?1", [name], |_| {
                    Ok(())
                })
                .is_ok()
        };
        assert!(exists("trash_entries"));
        assert!(exists("fs_quotas"));
        // v15：任务中心的两张表与「同目标仅一个活跃作业」的部分唯一索引。
        assert!(exists("jobs"));
        assert!(exists("job_logs"));
        assert!(exists("idx_jobs_active_target"));
    }

    /// v18：已部署的 v17 库物理上还留着 `app_command` / `app_port` 两列。
    /// 迁移必须把表重建掉，且站点与别名数据原样保留。
    #[test]
    fn v17_database_drops_dead_site_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14,
                SCHEMA_V15, SCHEMA_V16, SCHEMA_V17,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 17").unwrap();
            conn.execute(
                "INSERT INTO sites(domain, root_dir, site_type, app_command, app_port, status,
                                   created_at, updated_at)
                 VALUES('keep.local', '/www/sites/keep.local', 'static', NULL, NULL, 'running', 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO site_aliases(domain, site_domain, created_at)
                 VALUES('alias.local', 'keep.local', 1)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let version: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let names: Vec<String> = {
            let conn = db.conn();
            let mut stmt = conn.prepare("PRAGMA table_info(sites)").unwrap();
            stmt.query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(!names.iter().any(|n| n == "app_command"), "{names:?}");
        assert!(!names.iter().any(|n| n == "app_port"), "{names:?}");

        let site = db.site_by_domain("keep.local").unwrap().unwrap();
        assert_eq!(site.site_type, "static");
        assert_eq!(db.site_aliases("keep.local").unwrap(), vec!["alias.local"]);
    }

    /// v19：keyset 分页索引必须覆盖 `(时间, id)`，迁移前已有的审计/作业数据原样保留。
    #[test]
    fn v18_database_upgrades_keyset_indexes_and_keeps_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14,
                SCHEMA_V15, SCHEMA_V16, SCHEMA_V17, SCHEMA_V18,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 18").unwrap();
            conn.execute(
                "INSERT INTO audit(ts, actor, ip, action, target, result, detail)
                 VALUES(100, 'admin', '127.0.0.1', 'login', 'session', 'ok', '')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO jobs(id, kind, target_type, target_id, title, phase, phase_label,
                                   status, created_by, created_by_role, created_at)
                 VALUES('j1', 'image.pull', 'image', 'php:8.5', '拉取镜像', 'pull', '拉取',
                        'failed', 'admin', 'admin', 200)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let version: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let index_sql = |name: &str| -> String {
            db.conn()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
                    [name],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert!(
            index_sql("idx_audit_ts").contains("id DESC"),
            "审计索引缺 id 排序键"
        );
        assert!(
            index_sql("idx_jobs_created").contains("id DESC"),
            "作业列表索引缺 id 排序键"
        );
        assert!(
            index_sql("idx_jobs_status").contains("id DESC"),
            "作业状态索引缺 id 排序键"
        );

        // 索引真的被 keyset 查询用上，而不只是建了放着。
        let plan = |sql: &str| -> String {
            let conn = db.conn();
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            stmt.query_map([], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
                .join(" | ")
        };
        assert!(
            plan("SELECT id FROM audit ORDER BY ts DESC, id DESC LIMIT 5").contains("idx_audit_ts"),
            "审计 keyset 查询未走新索引"
        );
        assert!(
            plan("SELECT id FROM jobs WHERE status='failed' ORDER BY created_at DESC, id DESC LIMIT 5")
                .contains("idx_jobs_status"),
            "作业状态筛选未走新索引"
        );

        assert_eq!(db.recent_audit(10).unwrap().len(), 1);
        assert!(db.job("j1").unwrap().is_some());
    }

    #[test]
    fn trash_rows_roundtrip_and_state_transitions() {
        let db = Db::open_in_memory().unwrap();
        let row = TrashRow {
            id: "t1".into(),
            actor_type: "admin".into(),
            actor_id: "admin".into(),
            original_path: "/www/sites/a/x.txt".into(),
            trash_path: "/data/trash/2026/11/t1".into(),
            kind: "file".into(),
            size: 12,
            files: 1,
            mode: Some(0o644),
            owner: Some("1000:1000".into()),
            sha256: None,
            site: Some("a".into()),
            protected: false,
            deleted_at: 100,
            expires_at: 200,
            nlink: 1,
            restored_at: None,
            purged_at: None,
            quota_root: "/www/sites/a".into(),
        };
        db.trash_insert(&row).unwrap();
        assert_eq!(db.trash_get("t1").unwrap().unwrap(), row);
        assert_eq!(db.trash_live().unwrap().len(), 1);
        db.trash_mark_restored("t1", 101).unwrap();
        assert!(db.trash_live().unwrap().is_empty());
        let restored = db.trash_get("t1").unwrap().unwrap();
        assert_eq!(restored.restored_at, Some(101));
        db.trash_remove("t1").unwrap();
        assert!(db.trash_get("t1").unwrap().is_none());
    }

    #[test]
    fn quota_adjust_set_cancel_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.fs_quota_get("/www/sites/a").unwrap(), None);
        db.fs_quota_adjust("/www/sites/a", 100, 2).unwrap();
        let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
        assert_eq!((row.used_bytes, row.used_files), (100, 2));
        assert_eq!((row.max_bytes, row.max_files), (None, None));

        db.fs_quota_set("/www/sites/a", Some(500), Some(10), 100, 2, true, 300)
            .unwrap();
        let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
        assert_eq!((row.max_bytes, row.max_files), (Some(500), Some(10)));
        assert_eq!(row.scanned_at, Some(300));
        assert!(row.truncated, "截断标记必须落库，否则页面刷新后丢失");
        db.fs_quota_set_usage("/www/sites/a", 100, 2, false, 400)
            .unwrap();
        assert!(!db.fs_quota_get("/www/sites/a").unwrap().unwrap().truncated);

        db.fs_quota_adjust("/www/sites/a", -150, 1).unwrap();
        let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
        assert_eq!((row.used_bytes, row.used_files), (0, 3));
        assert_eq!(row.max_bytes, Some(500), "增量修正不能丢上限");

        db.fs_quota_clear_limits("/www/sites/a").unwrap();
        let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
        assert_eq!((row.max_bytes, row.max_files), (None, None));
        assert_eq!((row.used_bytes, row.used_files), (0, 3));
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
