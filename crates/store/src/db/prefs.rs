//! 按用户的界面偏好（菜单顺序/隐藏/固定/布局）。
//!
//! 只存 JSON 字符串，语义由 `skiff-web::menu` 解释；store 不依赖菜单模型，
//! 避免把 UI 契约下沉到存储层。卸载插件后的孤儿项由 web 层重写 JSON 清理。

use super::{now, Db, Result};
use rusqlite::{params, OptionalExtension};

impl Db {
    pub fn get_user_pref(&self, user_id: i64, key: &str) -> Result<Option<String>> {
        let conn = self.conn();
        let v = conn
            .query_row(
                "SELECT value_json FROM user_prefs WHERE user_id = ?1 AND key = ?2",
                params![user_id, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn set_user_pref(&self, user_id: i64, key: &str, value_json: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO user_prefs(user_id, key, value_json, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, key) DO UPDATE SET
               value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![user_id, key, value_json, now()],
        )?;
        Ok(())
    }

    pub fn delete_user_pref(&self, user_id: i64, key: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM user_prefs WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
        )?;
        Ok(())
    }

    /// 读取所有用户的同键偏好（卸载插件清理用）。
    pub fn list_prefs_by_key(&self, key: &str) -> Result<Vec<(i64, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT user_id, value_json FROM user_prefs WHERE key = ?1")?;
        let rows = stmt.query_map(params![key], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::Role;

    #[test]
    fn prefs_are_scoped_to_user_and_upsert() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_user("a", "h", Role::Admin, false).unwrap();
        let b = db.create_user("b", "h", Role::Viewer, false).unwrap();

        db.set_user_pref(a, "menu.prefs", r#"{"v":1}"#).unwrap();
        db.set_user_pref(b, "menu.prefs", r#"{"v":2}"#).unwrap();
        assert_eq!(
            db.get_user_pref(a, "menu.prefs").unwrap().as_deref(),
            Some(r#"{"v":1}"#)
        );

        db.set_user_pref(a, "menu.prefs", r#"{"v":3}"#).unwrap();
        assert_eq!(
            db.get_user_pref(a, "menu.prefs").unwrap().as_deref(),
            Some(r#"{"v":3}"#)
        );

        let all = db.list_prefs_by_key("menu.prefs").unwrap();
        assert_eq!(all.len(), 2);

        db.delete_user_pref(a, "menu.prefs").unwrap();
        assert!(db.get_user_pref(a, "menu.prefs").unwrap().is_none());
        assert!(db.get_user_pref(b, "menu.prefs").unwrap().is_some());
    }
}
