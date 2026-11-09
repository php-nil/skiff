//! 会话仓储（空闲续期 + 绝对过期双窗口，§6.1）。

use super::{now, Db, Result};
use crate::model::Session;
use rusqlite::{params, OptionalExtension};
use skiff_model::Role;

/// `verify_session` 的联表查询行：会话字段 + 用户名字/角色。
/// 单独起别名是为了让 `query_row` 的闭包签名可读（clippy::type_complexity）。
type SessionRow = (String, i64, String, String, i64, i64, i64, i64, String);

/// 会话续期写节流窗口（秒）。
///
/// 中间件对每个带 Cookie 的请求都会调 `verify_session`，此前每调一次就 UPDATE
/// 一次 `last_seen/idle_expires`——静态资源也会触发写，单连接的 SQLite 因此
/// 被高频写锁占住。改为距上次写入不足该窗口时直接跳过续期：
///
/// * 只影响「续期写」的频率，不改变过期判定本身——过期仍以库里的
///   `idle_expires/abs_expires` 为准，`abs_expires` 完全不受影响。
/// * 代价：空闲窗口的有效上限最多缩短一个窗口长度（活动被延后到最多 30s
///   之后才落库）。默认空闲窗口至少是分钟级，这个误差可接受。
const TOUCH_INTERVAL_SECS: i64 = 30;

impl Db {
    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        &self,
        id: &str,
        user_id: i64,
        token_hash: &str,
        ip: &str,
        user_agent: &str,
        idle_expires: i64,
        abs_expires: i64,
    ) -> Result<()> {
        let conn = self.conn();
        let ts = now();
        conn.execute(
            "INSERT INTO sessions(id, user_id, token_hash, created_at, last_seen, idle_expires,
                                  abs_expires, ip, user_agent)
             VALUES(?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                user_id,
                token_hash,
                ts,
                idle_expires,
                abs_expires,
                ip,
                user_agent
            ],
        )?;
        Ok(())
    }

    /// 校验会话并按需续期空闲窗口。返回 `None` 表示无效/过期/已撤销。
    ///
    /// 续期写按 `TOUCH_INTERVAL_SECS` 节流：窗口内的重复请求只读不写。
    pub fn verify_session(&self, token_hash: &str, idle_window: i64) -> Result<Option<Session>> {
        let conn = self.conn();
        let ts = now();
        let row: Option<SessionRow> = conn
            .query_row(
                "SELECT s.id, s.user_id, u.username, u.role, s.created_at, s.last_seen,
                        s.idle_expires, s.abs_expires, s.ip
                 FROM sessions s JOIN users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.revoked = 0",
                params![token_hash],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ))
                },
            )
            .optional()?;

        let Some((
            id,
            user_id,
            username,
            role,
            created_at,
            last_seen,
            idle_expires,
            abs_expires,
            ip,
        )) = row
        else {
            return Ok(None);
        };
        if ts >= idle_expires || ts >= abs_expires {
            conn.execute("UPDATE sessions SET revoked = 1 WHERE id = ?1", params![id])?;
            return Ok(None);
        }
        // 窗口内的请求不做续期写：返回库里已存的过期时间，语义与「刚续期过」一致。
        let (seen, new_idle) = if ts.saturating_sub(last_seen) < TOUCH_INTERVAL_SECS {
            (last_seen, idle_expires)
        } else {
            let new_idle = (ts + idle_window).min(abs_expires);
            conn.execute(
                "UPDATE sessions SET last_seen = ?2, idle_expires = ?3 WHERE id = ?1",
                params![id, ts, new_idle],
            )?;
            (ts, new_idle)
        };
        Ok(Some(Session {
            id,
            user_id,
            username,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
            created_at,
            last_seen: seen,
            idle_expires: new_idle,
            abs_expires,
            ip,
        }))
    }

    pub fn revoke_session(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute("UPDATE sessions SET revoked = 1 WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 改密后撤销该用户全部会话（§6.1）。
    pub fn revoke_all_sessions(&self, user_id: i64) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE sessions SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
            params![user_id],
        )?;
        Ok(())
    }

    pub fn purge_expired_sessions(&self) -> Result<usize> {
        let conn = self.conn();
        let ts = now();
        let n = conn.execute(
            "DELETE FROM sessions WHERE abs_expires < ?1 OR idle_expires < ?1",
            params![ts],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_verify_renews_idle_and_revokes_on_expiry() {
        let db = Db::open_in_memory().unwrap();
        let uid = db.create_user("a", "h", Role::Admin, false).unwrap();
        let ts = now();
        db.create_session("s1", uid, "hash-token", "127.0.0.1", "ua", ts + 2, ts + 100)
            .unwrap();
        // 把 last_seen 推出续期节流窗口，确保这次校验真的会续期。
        db.conn()
            .execute(
                "UPDATE sessions SET last_seen = ?1 WHERE id = 's1'",
                params![ts - TOUCH_INTERVAL_SECS - 1],
            )
            .unwrap();
        let s = db.verify_session("hash-token", 60).unwrap().unwrap();
        assert_eq!(s.username, "a");
        // 空闲窗口被续期，但不超过绝对过期
        assert!(s.idle_expires > ts);
        assert!(s.idle_expires <= s.abs_expires);

        // 手动把 idle 过期时间推到过去
        db.conn()
            .execute(
                "UPDATE sessions SET idle_expires = ?1 WHERE id = 's1'",
                params![ts - 1],
            )
            .unwrap();
        assert!(db.verify_session("hash-token", 60).unwrap().is_none());
        let revoked: i64 = db
            .conn()
            .query_row("SELECT revoked FROM sessions WHERE id = 's1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(revoked, 1);
    }

    #[test]
    fn session_verify_skips_touch_within_window() {
        let db = Db::open_in_memory().unwrap();
        let uid = db.create_user("b", "h", Role::Admin, false).unwrap();
        let ts = now();
        db.create_session("s2", uid, "tok2", "127.0.0.1", "ua", ts + 60, ts + 600)
            .unwrap();

        // 窗口内的请求只读不写：返回库里已存的 last_seen/idle_expires。
        let s = db.verify_session("tok2", 60).unwrap().unwrap();
        assert_eq!(s.last_seen, ts);
        assert_eq!(s.idle_expires, ts + 60);

        // 推出窗口后恢复续期写。
        db.conn()
            .execute(
                "UPDATE sessions SET last_seen = ?1 WHERE id = 's2'",
                params![ts - TOUCH_INTERVAL_SECS - 1],
            )
            .unwrap();
        let s = db.verify_session("tok2", 120).unwrap().unwrap();
        assert!(s.last_seen >= ts);
        assert!(s.idle_expires > ts + 60);
        assert!(s.idle_expires <= s.abs_expires);
    }
}
