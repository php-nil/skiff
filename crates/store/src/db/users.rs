//! 用户仓储（谁存谁定义：`User` 在 `crate::model`）。

use super::{now, Db, Result};
use crate::error::StoreError;
use crate::model::User;
use rusqlite::{params, OptionalExtension};
use skiff_model::Role;

impl Db {
    pub fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
        must_change_password: bool,
    ) -> Result<i64> {
        let conn = self.conn();
        let ts = now();
        conn.execute(
            "INSERT INTO users(username, password_hash, role, must_change_password, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?5)",
            params![username, password_hash, role.as_str(), must_change_password as i64, ts],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _) if err.code == rusqlite::ErrorCode::ConstraintViolation => {
                StoreError::Conflict(format!("用户 {username} 已存在"))
            }
            other => other.into(),
        })?;
        Ok(conn.last_insert_rowid())
    }

    pub fn user_by_name(&self, username: &str) -> Result<Option<User>> {
        let conn = self.conn();
        let user = conn
            .query_row(
                "SELECT id, username, password_hash, role, totp_secret, totp_enabled,
                        must_change_password, created_at
                 FROM users WHERE username = ?1",
                params![username],
                row_to_user,
            )
            .optional()?;
        Ok(user)
    }

    pub fn user_by_id(&self, id: i64) -> Result<Option<User>> {
        let conn = self.conn();
        let user = conn
            .query_row(
                "SELECT id, username, password_hash, role, totp_secret, totp_enabled,
                        must_change_password, created_at
                 FROM users WHERE id = ?1",
                params![id],
                row_to_user,
            )
            .optional()?;
        Ok(user)
    }

    pub fn set_password(&self, user_id: i64, password_hash: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE users SET password_hash = ?2, must_change_password = 0, updated_at = ?3 WHERE id = ?1",
            params![user_id, password_hash, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {user_id}")));
        }
        Ok(())
    }

    /// 管理员/运维重置口令（`skiff admin reset-password`）。
    ///
    /// 与 [`Self::set_password`] 只差 `must_change_password` 的取值，但这个差别是语义性的：
    /// 自助改密是用户**自己**刚定了一个口令，标志清 0；重置是别人设的临时口令（会写进
    /// `initial-admin.txt`，可能被同机器上的其他人看到），置 1 强制首登改密。
    ///
    /// 会话失效不在这里做——那是另一张表，由调用方显式调 `revoke_all_sessions`，免得仓储层
    /// 出现隐式跨表写。
    pub fn reset_password(&self, user_id: i64, password_hash: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE users SET password_hash = ?2, must_change_password = 1, updated_at = ?3 WHERE id = ?1",
            params![user_id, password_hash, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(format!("user {user_id}")));
        }
        Ok(())
    }

    pub fn set_totp(&self, user_id: i64, secret: Option<&str>, enabled: bool) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE users SET totp_secret = ?2, totp_enabled = ?3, updated_at = ?4 WHERE id = ?1",
            params![user_id, secret, enabled as i64, now()],
        )?;
        Ok(())
    }

    pub fn list_users(&self) -> Result<Vec<User>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, username, password_hash, role, totp_secret, totp_enabled,
                    must_change_password, created_at
             FROM users ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_user)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn count_users(&self) -> Result<i64> {
        let conn = self.conn();
        Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)
    }
}

fn row_to_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        id: r.get(0)?,
        username: r.get(1)?,
        password_hash: r.get(2)?,
        role: Role::parse(&r.get::<_, String>(3)?).unwrap_or(Role::Viewer),
        totp_secret: r.get(4)?,
        totp_enabled: r.get::<_, i64>(5)? != 0,
        must_change_password: r.get::<_, i64>(6)? != 0,
        created_at: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_roundtrip_and_conflict() {
        let db = Db::open_in_memory().unwrap();
        let id = db.create_user("admin", "hash", Role::Admin, true).unwrap();
        let u = db.user_by_name("admin").unwrap().unwrap();
        assert_eq!(u.id, id);
        assert_eq!(u.role, Role::Admin);
        assert!(u.must_change_password);
        assert!(matches!(
            db.create_user("admin", "h", Role::Admin, false),
            Err(StoreError::Conflict(_))
        ));
        db.set_password(id, "hash2").unwrap();
        assert_eq!(
            db.user_by_name("admin").unwrap().unwrap().password_hash,
            "hash2"
        );
        assert!(
            !db.user_by_name("admin")
                .unwrap()
                .unwrap()
                .must_change_password
        );
    }

    /// 自助改密与运维重置对 `must_change_password` 的处理必须相反：前者是用户自己刚定的
    /// 口令（清 0），后者是别人设的临时口令（置 1，强制首登改密）。
    #[test]
    fn reset_password_forces_change_while_self_change_clears_it() {
        let db = Db::open_in_memory().unwrap();
        let id = db.create_user("admin", "h1", Role::Admin, false).unwrap();
        db.set_password(id, "h2").unwrap();
        assert!(
            !db.user_by_name("admin")
                .unwrap()
                .unwrap()
                .must_change_password
        );

        db.reset_password(id, "h3").unwrap();
        let u = db.user_by_name("admin").unwrap().unwrap();
        assert_eq!(u.password_hash, "h3");
        assert!(u.must_change_password, "重置后必须强制首登改密");

        assert!(matches!(
            db.reset_password(9999, "h4"),
            Err(StoreError::NotFound(_))
        ));
    }
}
