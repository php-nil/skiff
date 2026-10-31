//! settings 键值：安全入口前缀、实例 id、CSRF 密钥等首启生成值。

use super::{now, Db, Result};
use rusqlite::{params, OptionalExtension};

impl Db {
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let v = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO settings(key, value, updated_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now()],
        )?;
        Ok(())
    }

    /// 首次写入才生效（安全入口前缀、实例 id 等首启生成值）。
    pub fn get_or_init_setting(&self, key: &str, init: impl FnOnce() -> String) -> Result<String> {
        if let Some(v) = self.get_setting(key)? {
            return Ok(v);
        }
        let v = init();
        self.set_setting(key, &v)?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_init_once() {
        let db = Db::open_in_memory().unwrap();
        let a = db.get_or_init_setting("entry", || "abc".into()).unwrap();
        let b = db.get_or_init_setting("entry", || "zzz".into()).unwrap();
        assert_eq!(a, "abc");
        assert_eq!(b, "abc");
    }
}
