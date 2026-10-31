//! API Token（机器访问，scope 白名单随 token 存储）。

use super::{now, Db, Result};
use crate::model::User;
use rusqlite::{params, OptionalExtension};

impl Db {
    pub fn create_api_token(
        &self,
        id: &str,
        user_id: i64,
        name: &str,
        token_hash: &str,
        scopes: &str,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO api_tokens(id, user_id, name, token_hash, scopes, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, user_id, name, token_hash, scopes, now()],
        )?;
        Ok(())
    }

    pub fn verify_api_token(&self, token_hash: &str) -> Result<Option<(User, String)>> {
        // 同上：guard 必须在 self.user_by_id() 之前释放
        let row: Option<(i64, String)> = {
            let conn = self.conn();
            let row = conn
                .query_row(
                    "SELECT user_id, scopes FROM api_tokens WHERE token_hash = ?1 AND revoked = 0",
                    params![token_hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if row.is_some() {
                conn.execute(
                    "UPDATE api_tokens SET last_used = ?2 WHERE token_hash = ?1",
                    params![token_hash, now()],
                )?;
            }
            row
        };
        let Some((user_id, scopes)) = row else {
            return Ok(None);
        };
        Ok(self.user_by_id(user_id)?.map(|u| (u, scopes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::Role;

    // 回归：组合查询曾写出「持锁再进 self.xxx()」导致进程永久卡死；
    // 这里覆盖 verify_api_token 的联表路径，防止再犯。
    #[test]
    fn api_token_verify_joins_user_with_scopes() {
        let db = Db::open_in_memory().unwrap();
        let uid = db.create_user("ci", "h", Role::Operator, false).unwrap();
        db.create_api_token("t1", uid, "ci", "hash-abc", "docker:read,audit")
            .unwrap();

        let (user, scopes) = db
            .verify_api_token("hash-abc")
            .unwrap()
            .expect("token 应有效");
        assert_eq!(user.username, "ci");
        assert_eq!(scopes, "docker:read,audit");
        // 未知 token 不报错，返回 None
        assert!(db.verify_api_token("nope").unwrap().is_none());
    }
}
