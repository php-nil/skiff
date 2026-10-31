//! 信任键（TOFU，§4.3）：首次见到的公钥入库，同 key_id 后续导入不覆盖。

use super::{now, Db, Result};
use rusqlite::{params, OptionalExtension};

impl Db {
    pub fn trust_key(&self, key_id: &str, public_key_hex: &str, label: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO trusted_keys(key_id, public_key, label, added_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(key_id) DO NOTHING",
            params![key_id, public_key_hex, label, now()],
        )?;
        Ok(())
    }

    pub fn trusted_key(&self, key_id: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let v = conn
            .query_row(
                "SELECT public_key FROM trusted_keys WHERE key_id = ?1",
                params![key_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn list_trusted_keys(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT key_id, public_key, label FROM trusted_keys ORDER BY added_at")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn remove_trusted_key(&self, key_id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM trusted_keys WHERE key_id = ?1",
            params![key_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_key_is_first_use_only() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.trusted_key("k1").unwrap().is_none());
        db.trust_key("k1", "aaaa", "作者甲").unwrap();
        assert_eq!(db.trusted_key("k1").unwrap().as_deref(), Some("aaaa"));
        // 同一 key_id 再导入不覆盖（TOFU）：换公钥必须先显式删除
        db.trust_key("k1", "bbbb", "冒充者").unwrap();
        assert_eq!(db.trusted_key("k1").unwrap().as_deref(), Some("aaaa"));
        assert_eq!(db.list_trusted_keys().unwrap().len(), 1);
        db.remove_trusted_key("k1").unwrap();
        assert!(db.trusted_key("k1").unwrap().is_none());
    }
}
