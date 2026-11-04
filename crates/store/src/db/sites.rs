//! 网站站点的持久化（`sites` 表）。
//!
//! 站点是内核能力，不归插件所有；这里只负责行级 CRUD，vhost 生成与容器编排
//! 在 `skiff-app::sites`。`domain` 唯一约束由数据库保证。

use super::{now, Db, Result};
use crate::error::StoreError;
use rusqlite::{params, OptionalExtension, Row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub id: i64,
    pub domain: String,
    pub root_dir: String,
    pub php_version: String,
    /// running / stopped / error
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 默认站点标记：库是唯一状态来源，vhost 文件只是它的投影。
    pub is_default: bool,
    /// 是否启用 443（证书文件在 `/www/certs/<域名>`）。
    pub https: bool,
    /// 是否把 80 强制跳转到 443。
    pub force_https: bool,
}

fn row_to_site(r: &Row<'_>) -> rusqlite::Result<Site> {
    Ok(Site {
        id: r.get(0)?,
        domain: r.get(1)?,
        root_dir: r.get(2)?,
        php_version: r.get(3)?,
        status: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        is_default: r.get::<_, i64>(7)? != 0,
        https: r.get::<_, i64>(8)? != 0,
        force_https: r.get::<_, i64>(9)? != 0,
    })
}

impl Db {
    pub fn create_site(&self, domain: &str, root_dir: &str, php_version: &str) -> Result<Site> {
        let conn = self.conn();
        let ts = now();
        conn.execute(
            "INSERT INTO sites(domain, root_dir, php_version, status, created_at, updated_at)
             VALUES(?1, ?2, ?3, 'running', ?4, ?4)",
            params![domain, root_dir, php_version, ts],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                StoreError::Conflict(format!("站点域名已存在: {domain}"))
            }
            other => StoreError::Sqlite(other),
        })?;
        drop(conn);
        self.site_by_domain(domain)?
            .ok_or_else(|| StoreError::NotFound(domain.to_string()))
    }

    pub fn list_sites(&self) -> Result<Vec<Site>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, domain, root_dir, php_version, status, created_at, updated_at, is_default,
                    https, force_https
             FROM sites ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], row_to_site)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn site_by_domain(&self, domain: &str) -> Result<Option<Site>> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, domain, root_dir, php_version, status, created_at, updated_at, is_default,
                    https, force_https
             FROM sites WHERE domain = ?1",
            params![domain],
            row_to_site,
        )
        .optional()
        .map_err(StoreError::from)
    }

    /// 切换默认站点。单条 UPDATE 同时清旧置新，依赖 `idx_sites_default` 保证最多一个。
    pub fn set_site_default(&self, domain: &str, is_default: bool) -> Result<()> {
        let conn = self.conn();
        let ts = now();
        let n = if is_default {
            conn.execute(
                "UPDATE sites
                 SET is_default = CASE WHEN domain = ?1 THEN 1 ELSE 0 END, updated_at = ?2
                 WHERE is_default = 1 OR domain = ?1",
                params![domain, ts],
            )?
        } else {
            conn.execute(
                "UPDATE sites SET is_default = 0, updated_at = ?2 WHERE domain = ?1",
                params![domain, ts],
            )?
        };
        if n == 0 {
            return Err(StoreError::NotFound(domain.to_string()));
        }
        Ok(())
    }

    pub fn default_site(&self) -> Result<Option<Site>> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, domain, root_dir, php_version, status, created_at, updated_at, is_default,
                    https, force_https
             FROM sites WHERE is_default = 1",
            [],
            row_to_site,
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn set_site_status(&self, domain: &str, status: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE sites SET status = ?2, updated_at = ?3 WHERE domain = ?1",
            params![domain, status, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(domain.to_string()));
        }
        Ok(())
    }

    /// 更新 HTTPS 开关；`force_https` 只允许在启用时置 1（无证书时强制跳转必然 502）。
    pub fn set_site_tls(&self, domain: &str, https: bool, force_https: bool) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE sites SET https = ?2, force_https = ?3, updated_at = ?4 WHERE domain = ?1",
            params![
                domain,
                i64::from(https),
                i64::from(force_https && https),
                now()
            ],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(domain.to_string()));
        }
        Ok(())
    }

    pub fn delete_site(&self, domain: &str) -> Result<bool> {
        let conn = self.conn();
        // 别名由 `site_aliases.site_domain` 的外键级联清理（PRAGMA foreign_keys=ON）。
        let n = conn.execute("DELETE FROM sites WHERE domain = ?1", params![domain])?;
        Ok(n > 0)
    }

    /// 写入站点别名。域名唯一约束由 `site_aliases.domain` 主键保证。
    pub fn add_site_aliases(&self, site_domain: &str, aliases: &[String]) -> Result<()> {
        if aliases.is_empty() {
            return Ok(());
        }
        let conn = self.conn();
        let ts = now();
        for alias in aliases {
            conn.execute(
                "INSERT INTO site_aliases(domain, site_domain, created_at) VALUES(?1, ?2, ?3)",
                params![alias, site_domain, ts],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(err, _)
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    StoreError::Conflict(format!("别名域名已存在: {alias}"))
                }
                other => StoreError::Sqlite(other),
            })?;
        }
        Ok(())
    }

    pub fn site_aliases(&self, site_domain: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT domain FROM site_aliases WHERE site_domain = ?1 ORDER BY created_at ASC, domain ASC",
        )?;
        let rows = stmt.query_map(params![site_domain], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 该域名是否已被某个站点用作别名（返回站点主域名）。
    pub fn alias_owner(&self, domain: &str) -> Result<Option<String>> {
        let conn = self.conn();
        conn.query_row(
            "SELECT site_domain FROM site_aliases WHERE domain = ?1",
            params![domain],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(StoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_crud_and_unique_domain() {
        let db = Db::open_in_memory().unwrap();
        let s = db
            .create_site("example.com", "/www/sites/example.com", "8.5")
            .unwrap();
        assert_eq!(s.status, "running");
        assert_eq!(db.list_sites().unwrap().len(), 1);

        let dup = db.create_site("example.com", "/tmp/other", "8.5");
        assert!(matches!(dup, Err(StoreError::Conflict(_))));

        db.set_site_status("example.com", "stopped").unwrap();
        assert_eq!(
            db.site_by_domain("example.com").unwrap().unwrap().status,
            "stopped"
        );

        assert!(db.delete_site("example.com").unwrap());
        assert!(db.list_sites().unwrap().is_empty());
    }

    #[test]
    fn schema_v6_keeps_previous_data() {
        let db = Db::open_in_memory().unwrap();
        let v: i64 = db
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 6);
        assert!(db.list_sites().unwrap().is_empty());
    }

    /// HTTPS 开关只在启用时允许强制跳转；关闭 HTTPS 必须同时清掉强制位。
    #[test]
    fn tls_flags_are_consistent() {
        let db = Db::open_in_memory().unwrap();
        db.create_site("a.local", "/www/sites/a.local", "8.5")
            .unwrap();
        let s = db.site_by_domain("a.local").unwrap().unwrap();
        assert!(!s.https && !s.force_https);

        db.set_site_tls("a.local", true, true).unwrap();
        let s = db.site_by_domain("a.local").unwrap().unwrap();
        assert!(s.https && s.force_https);

        // 无证书却想直接开强制跳转 → 强制位被压回 0。
        db.set_site_tls("a.local", false, true).unwrap();
        let s = db.site_by_domain("a.local").unwrap().unwrap();
        assert!(!s.https && !s.force_https);

        assert!(matches!(
            db.set_site_tls("missing.local", true, false),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn aliases_are_unique_and_cascade_on_delete() {
        let db = Db::open_in_memory().unwrap();
        db.create_site("a.local", "/www/sites/a.local", "8.5")
            .unwrap();
        db.add_site_aliases("a.local", &["www.a.local".into(), "api.a.local".into()])
            .unwrap();
        assert_eq!(
            db.site_aliases("a.local").unwrap(),
            vec!["api.a.local".to_string(), "www.a.local".to_string()]
        );
        assert_eq!(
            db.alias_owner("www.a.local").unwrap().as_deref(),
            Some("a.local")
        );
        assert!(matches!(
            db.add_site_aliases("a.local", &["www.a.local".into()]),
            Err(StoreError::Conflict(_))
        ));

        db.delete_site("a.local").unwrap();
        assert!(db.site_aliases("a.local").unwrap().is_empty());
        assert!(db.alias_owner("www.a.local").unwrap().is_none());
    }

    #[test]
    fn default_site_is_exclusive_and_clearable() {
        let db = Db::open_in_memory().unwrap();
        db.create_site("a.local", "/www/sites/a.local", "8.5")
            .unwrap();
        db.create_site("b.local", "/www/sites/b.local", "8.5")
            .unwrap();

        db.set_site_default("a.local", true).unwrap();
        assert_eq!(db.default_site().unwrap().unwrap().domain, "a.local");

        // 互切：置 b 后 a 必须自动清掉，库里只能有一个 1。
        db.set_site_default("b.local", true).unwrap();
        assert_eq!(db.default_site().unwrap().unwrap().domain, "b.local");
        assert!(!db.site_by_domain("a.local").unwrap().unwrap().is_default);
        assert_eq!(
            db.list_sites()
                .unwrap()
                .iter()
                .filter(|s| s.is_default)
                .count(),
            1
        );

        // 解除默认。
        db.set_site_default("b.local", false).unwrap();
        assert!(db.default_site().unwrap().is_none());

        // 未知域名不能把默认位“设”出来。
        assert!(matches!(
            db.set_site_default("missing.local", true),
            Err(StoreError::NotFound(_))
        ));
    }
}
