//! 插件仓储：记录、grant、内核代管配置（config_schema 取值的第一轨）。

use super::{now, Db, Result};
use crate::model::PluginRecord;
use rusqlite::{params, OptionalExtension};
use skiff_model::Grant;

impl Db {
    pub fn upsert_plugin(&self, p: &PluginRecord) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        tx.execute(
            "INSERT INTO plugins(id, name, version, api, key_id, manifest, enabled, status,
                                 status_detail, active_version, installed_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name, version = excluded.version, api = excluded.api,
                key_id = excluded.key_id, manifest = excluded.manifest,
                enabled = excluded.enabled, status = excluded.status,
                status_detail = excluded.status_detail, active_version = excluded.active_version,
                updated_at = excluded.updated_at",
            params![
                p.id,
                p.name,
                p.version,
                p.api,
                p.key_id,
                p.manifest,
                p.enabled as i64,
                p.status,
                p.status_detail,
                p.active_version,
                ts
            ],
        )?;
        tx.execute(
            "DELETE FROM plugin_grants WHERE plugin_id = ?1",
            params![p.id],
        )?;
        for g in &p.grants {
            tx.execute(
                "INSERT OR IGNORE INTO plugin_grants(plugin_id, kind, value, created_at)
                 VALUES(?1, ?2, ?3, ?4)",
                params![p.id, g.kind, g.value, ts],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn plugin(&self, id: &str) -> Result<Option<PluginRecord>> {
        // 注意：这里的 guard 必须在调用 self.grants() 之前释放。
        // std::sync::Mutex 不可重入，持锁再进 self.xxx() 会永久卡死（不是报错，是卡死）。
        let base = {
            let conn = self.conn();
            conn.query_row(
                "SELECT id, name, version, api, key_id, manifest, enabled, status, status_detail,
                        active_version, installed_at
                 FROM plugins WHERE id = ?1",
                params![id],
                |r| {
                    Ok(PluginRecord {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        version: r.get(2)?,
                        api: r.get(3)?,
                        key_id: r.get(4)?,
                        manifest: r.get(5)?,
                        enabled: r.get::<_, i64>(6)? != 0,
                        status: r.get(7)?,
                        status_detail: r.get(8)?,
                        active_version: r.get(9)?,
                        installed_at: r.get(10)?,
                        grants: Vec::new(),
                    })
                },
            )
            .optional()?
        };
        let Some(mut p) = base else { return Ok(None) };
        p.grants = self.grants(id)?;
        Ok(Some(p))
    }

    pub fn list_plugins(&self) -> Result<Vec<PluginRecord>> {
        let ids: Vec<String> = {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT id FROM plugins ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(p) = self.plugin(&id)? {
                out.push(p);
            }
        }
        Ok(out)
    }

    pub fn set_plugin_status(&self, id: &str, status: &str, detail: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE plugins SET status = ?2, status_detail = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, status, detail, now()],
        )?;
        Ok(())
    }

    /// 合并写启用标记与状态位：两列必须在同一条 `UPDATE` 里改。
    ///
    /// 分两次写会在第二次失败时留下「`enabled` 已变、`status` 未变」的窗口；单条 UPDATE
    /// 本身就是一个隐式事务，天然原子。状态映射也收在这里（启用 → `installed`，
    /// 停用 → `disabled`），避免各调用点各写一遍、写漏一处就不一致。
    pub fn set_plugin_enabled_state(&self, id: &str, enabled: bool) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE plugins SET enabled = ?2, status = ?3, status_detail = '', updated_at = ?4
             WHERE id = ?1",
            params![
                id,
                enabled as i64,
                if enabled { "installed" } else { "disabled" },
                now()
            ],
        )?;
        Ok(())
    }

    pub fn delete_plugin(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM plugins WHERE id = ?1", params![id])?;
        conn.execute(
            "DELETE FROM plugin_grants WHERE plugin_id = ?1",
            params![id],
        )?;
        conn.execute(
            "DELETE FROM plugin_config WHERE plugin_id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn grants(&self, plugin_id: &str) -> Result<Vec<Grant>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT kind, value FROM plugin_grants WHERE plugin_id = ?1 ORDER BY kind, value",
        )?;
        let rows = stmt.query_map(params![plugin_id], |r| {
            Ok(Grant {
                kind: r.get(0)?,
                value: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn add_grant(&self, plugin_id: &str, grant: &Grant) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT OR IGNORE INTO plugin_grants(plugin_id, kind, value, created_at)
             VALUES(?1, ?2, ?3, ?4)",
            params![plugin_id, grant.kind, grant.value, now()],
        )?;
        Ok(())
    }

    pub fn remove_grant(&self, plugin_id: &str, grant: &Grant) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM plugin_grants WHERE plugin_id = ?1 AND kind = ?2 AND value = ?3",
            params![plugin_id, grant.kind, grant.value],
        )?;
        Ok(())
    }

    pub fn set_plugin_config(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
        secret: bool,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO plugin_config(plugin_id, key, value, secret) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(plugin_id, key) DO UPDATE SET value = excluded.value, secret = excluded.secret",
            params![plugin_id, key, value, secret as i64],
        )?;
        Ok(())
    }

    pub fn plugin_config(&self, plugin_id: &str) -> Result<Vec<(String, String, bool)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT key, value, secret FROM plugin_config WHERE plugin_id = ?1 ORDER BY key",
        )?;
        let rows = stmt.query_map(params![plugin_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> PluginRecord {
        PluginRecord {
            id: id.into(),
            name: "Demo".into(),
            version: "0.1.0".into(),
            api: "1.0".into(),
            key_id: "k".into(),
            manifest: format!("id = \"{id}\""),
            enabled: true,
            status: "enabled".into(),
            status_detail: String::new(),
            active_version: "0.1.0".into(),
            installed_at: 0,
            grants: vec![],
        }
    }

    #[test]
    fn plugin_upsert_replaces_grants() {
        let db = Db::open_in_memory().unwrap();
        let mut p = record("com.example.demo");
        p.grants = vec![Grant::perm("docker:list"), Grant::object("container:abc")];
        db.upsert_plugin(&p).unwrap();
        assert_eq!(db.grants("com.example.demo").unwrap().len(), 2);

        p.grants = vec![Grant::perm("docker:list")];
        db.upsert_plugin(&p).unwrap();
        assert_eq!(db.grants("com.example.demo").unwrap().len(), 1);
        assert_eq!(
            db.plugin("com.example.demo").unwrap().unwrap().version,
            "0.1.0"
        );
    }

    #[test]
    fn plugin_enabled_state_writes_both_columns() {
        let db = Db::open_in_memory().unwrap();
        let p = record("com.example.toggle");
        db.upsert_plugin(&p).unwrap();

        db.set_plugin_enabled_state(&p.id, false).unwrap();
        let off = db.plugin(&p.id).unwrap().unwrap();
        assert!(!off.enabled);
        assert_eq!(off.status, "disabled");
        assert_eq!(off.status_detail, "");

        db.set_plugin_enabled_state(&p.id, true).unwrap();
        let on = db.plugin(&p.id).unwrap().unwrap();
        assert!(on.enabled);
        assert_eq!(on.status, "installed");
        assert_eq!(on.status_detail, "");
    }

    #[test]
    fn plugin_config_marks_secret_and_upserts() {
        let db = Db::open_in_memory().unwrap();
        let p = record("com.example.cfg");
        db.upsert_plugin(&p).unwrap();
        db.set_plugin_config(&p.id, "api_base", "https://example.com", false)
            .unwrap();
        db.set_plugin_config(&p.id, "token", "s3cret", true)
            .unwrap();
        db.set_plugin_config(&p.id, "api_base", "https://example.org", false)
            .unwrap();

        let cfg = db.plugin_config(&p.id).unwrap();
        assert_eq!(cfg.len(), 2);
        let api_base = cfg.iter().find(|(k, _, _)| k == "api_base").unwrap();
        assert_eq!(api_base.1, "https://example.org", "upsert 应覆盖旧值");
        assert!(!api_base.2);
        let token = cfg.iter().find(|(k, _, _)| k == "token").unwrap();
        assert!(token.2, "secret 标记必须落库，UI 据此打码");
    }
}
