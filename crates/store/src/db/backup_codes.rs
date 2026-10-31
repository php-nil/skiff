//! TOTP 备份码（改密/换设备兜底）。

use super::{Db, Result};
use rusqlite::params;

impl Db {
    pub fn replace_backup_codes(&self, user_id: i64, hashes: &[String]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM backup_codes WHERE user_id = ?1",
            params![user_id],
        )?;
        for h in hashes {
            tx.execute(
                "INSERT INTO backup_codes(user_id, code_hash) VALUES(?1, ?2)",
                params![user_id, h],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 消费一枚未用过的备份码；成功返回 true。
    pub fn consume_backup_code(&self, user_id: i64, code_hash: &str) -> Result<bool> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE backup_codes SET used = 1
             WHERE user_id = ?1 AND code_hash = ?2 AND used = 0",
            params![user_id, code_hash],
        )?;
        Ok(n == 1)
    }
}
