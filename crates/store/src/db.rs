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
mod sessions;
mod settings;
pub mod sites;
mod tokens;
mod trust;
mod users;

/// 当前二进制支持的 schema 版本。迁移只向前，版本高于此值拒绝启动。
pub const CURRENT_SCHEMA_VERSION: i64 = 6;

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

// v6：站点 HTTPS 状态。证书文件落在 `/www/certs/<域名>`，库里只存开关；
// `https` 表示启用 443，`force_https` 表示 80 强制跳转 443。
const SCHEMA_V6: &str = r#"
ALTER TABLE sites ADD COLUMN https INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sites ADD COLUMN force_https INTEGER NOT NULL DEFAULT 0;
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

    /// v3 → v6 只加列/加表，不重建站点表：原站点行必须原样保留，新列取默认值。
    #[test]
    fn schema_v3_sites_survive_upgrade_to_v6() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute_batch(SCHEMA_V5).unwrap();
            conn.execute_batch("PRAGMA user_version = 5").unwrap();
            conn.execute(
                "INSERT INTO sites(domain, root_dir, php_version, status, created_at, updated_at)
                 VALUES('keep.local', '/www/sites/keep.local', '8.5', 'running', 1, 1)",
                [],
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 6);
        let site = db.site_by_domain("keep.local").unwrap().unwrap();
        assert_eq!(site.root_dir, "/www/sites/keep.local");
        assert!(!site.is_default);
        assert!(!site.https);
        assert!(!site.force_https);
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
