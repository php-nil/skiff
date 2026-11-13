//! 审计流水（§6.3）：谁、从哪、对什么、结果如何。

use super::{now, Db, Result};
use crate::model::AuditEntry;
use rusqlite::params;

impl Db {
    pub fn audit(
        &self,
        actor: &str,
        ip: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO audit(ts, actor, ip, action, target, result, detail)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![now(), actor, ip, action, target, result, detail],
        )?;
        Ok(())
    }

    pub fn recent_audit(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, ts, actor, ip, action, target, result, detail
             FROM audit ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], read_entry)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 审计 keyset 分页（SDUI D10）：`(ts, id)` 倒序，`before` 为上一页末项的排序键。
    ///
    /// `keyword` 非空时对 actor / ip / action / target / detail 做 ASCII 不区分大小写的
    /// 子串匹配（`instr(lower(...))`，不引入 `LIKE` 通配符转义问题）。排序与范围全部落在
    /// 既有 `idx_audit_ts(ts DESC, id DESC)` 上；关键字只做行内过滤，不新增索引。
    /// `limit` 是上限，调用方多要一条来判定是否还有下一页。
    pub fn audit_page(
        &self,
        keyword: &str,
        before: Option<(i64, i64)>,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, ts, actor, ip, action, target, result, detail
             FROM audit
             WHERE (?1 = ''
                    OR instr(lower(actor), lower(?1)) > 0
                    OR instr(lower(ip), lower(?1)) > 0
                    OR instr(lower(action), lower(?1)) > 0
                    OR instr(lower(target), lower(?1)) > 0
                    OR instr(lower(detail), lower(?1)) > 0)
               AND (?2 IS NULL OR ts < ?2 OR (ts = ?2 AND id < ?3))
             ORDER BY ts DESC, id DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                keyword,
                before.map(|c| c.0),
                before.map(|c| c.1),
                limit as i64
            ],
            read_entry,
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

fn read_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        id: r.get(0)?,
        ts: r.get(1)?,
        actor: r.get(2)?,
        ip: r.get(3)?,
        action: r.get(4)?,
        target: r.get(5)?,
        result: r.get(6)?,
        detail: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::Role;

    #[test]
    fn audit_and_counts() {
        let db = Db::open_in_memory().unwrap();
        db.create_user("a", "h", Role::Admin, false).unwrap();
        db.audit("a", "1.2.3.4", "login", "session", "ok", "")
            .unwrap();
        let rows = db.recent_audit(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "login");
        assert_eq!(db.count_users().unwrap(), 1);
    }

    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        let username = "admin";
        db.audit(username, "10.0.0.1", "login", "session", "ok", "ua=curl")
            .unwrap();
        db.audit(username, "10.0.0.2", "site.create", "example.com", "ok", "")
            .unwrap();
        db.audit(
            "ops",
            "10.0.0.3",
            "site.delete",
            "old.com",
            "fail",
            "权限不足",
        )
        .unwrap();
        db
    }

    /// keyset 倒序 + 多取一条判定「还有下一页」的调用口径。
    #[test]
    fn audit_page_walks_backwards_with_keyset() {
        let db = seeded();
        let first = db.audit_page("", None, 2).unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].action, "site.delete");
        assert_eq!(first[1].action, "site.create");
        let older = db
            .audit_page("", Some((first[1].ts, first[1].id)), 2)
            .unwrap();
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].action, "login");
        // 已到末页：再往后没有数据。
        assert!(db
            .audit_page("", Some((older[0].ts, older[0].id)), 2)
            .unwrap()
            .is_empty());
    }

    /// 关键字覆盖 actor / ip / action / target / detail，且 ASCII 不区分大小写。
    #[test]
    fn audit_page_filters_every_searchable_column() {
        let db = seeded();
        for (keyword, expected) in [
            ("ADMIN", 2),       // actor
            ("10.0.0.2", 1),    // ip
            ("SITE.CREATE", 1), // action
            ("example", 1),     // target
            ("权限", 1),        // detail
        ] {
            assert_eq!(
                db.audit_page(keyword, None, 10).unwrap().len(),
                expected,
                "关键字 {keyword}"
            );
        }
    }

    /// `%` / `_` 是普通字符：instr 不把关键字当 LIKE 通配符。
    #[test]
    fn audit_page_treats_wildcards_literally() {
        let db = Db::open_in_memory().unwrap();
        db.audit("a", "1.1.1.1", "login", "session", "ok", "")
            .unwrap();
        db.audit("a", "1.1.1.1", "file.upload", "report%.txt", "ok", "")
            .unwrap();
        db.audit("a", "1.1.1.1", "file.upload", "file_01.txt", "ok", "")
            .unwrap();
        assert_eq!(db.audit_page("%", None, 10).unwrap().len(), 1);
        assert_eq!(db.audit_page("_", None, 10).unwrap().len(), 1);
    }
}
