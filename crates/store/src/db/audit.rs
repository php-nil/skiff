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
        let rows = stmt.query_map(params![limit as i64], |r| {
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
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 保留策略：删除 `ts < cutoff` 的审计记录，返回删除条数。
    ///
    /// 幂等：重复执行只会删除新落入窗口外的记录，已删的不会重复计数。
    pub fn purge_audit_before(&self, cutoff: i64) -> Result<usize> {
        let conn = self.conn();
        let deleted = conn.execute("DELETE FROM audit WHERE ts < ?1", params![cutoff])?;
        Ok(deleted)
    }
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

    /// 保留清理按时间窗删除：窗口内保留、窗口外删除，且重复执行幂等。
    #[test]
    fn purge_audit_before_deletes_only_out_of_window_rows() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..3 {
            db.audit("a", "1.2.3.4", "login", &format!("s{i}"), "ok", "")
                .unwrap();
        }
        // 截止时间为 0：没有记录早于它，保留全部。
        assert_eq!(db.purge_audit_before(0).unwrap(), 0);
        assert_eq!(db.recent_audit(10).unwrap().len(), 3);

        // 截止时间取未来：全部删除。
        assert_eq!(db.purge_audit_before(i64::MAX).unwrap(), 3);
        assert_eq!(db.recent_audit(10).unwrap().len(), 0);

        // 幂等：再跑一次不重复计数。
        assert_eq!(db.purge_audit_before(i64::MAX).unwrap(), 0);
    }
}
