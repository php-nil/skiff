//! 回收站元数据仓储（schema v14 的 `trash_entries` 表）。
//!
//! 只做行级 CRUD，不判断业务状态；`is_live` 由 `skiff-fs` 的模型层解释。

use rusqlite::{params, OptionalExtension, Row};

use crate::Result;

/// `trash_entries` 的一行。字段与 `skiff_fs::TrashEntry` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRow {
    pub id: String,
    pub actor_type: String,
    pub actor_id: String,
    pub original_path: String,
    pub trash_path: String,
    pub kind: String,
    pub size: u64,
    pub files: u64,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub sha256: Option<String>,
    pub site: Option<String>,
    pub protected: bool,
    pub deleted_at: i64,
    pub expires_at: i64,
    pub nlink: u64,
    pub restored_at: Option<i64>,
    pub purged_at: Option<i64>,
    pub quota_root: String,
}

impl TrashRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            actor_type: row.get(1)?,
            actor_id: row.get(2)?,
            original_path: row.get(3)?,
            trash_path: row.get(4)?,
            kind: row.get(5)?,
            size: row.get::<_, i64>(6)?.max(0) as u64,
            files: row.get::<_, i64>(7)?.max(0) as u64,
            mode: row.get(8)?,
            owner: row.get(9)?,
            sha256: row.get(10)?,
            site: row.get(11)?,
            protected: row.get::<_, i64>(12)? != 0,
            deleted_at: row.get(13)?,
            expires_at: row.get(14)?,
            nlink: row.get::<_, i64>(15)?.max(0) as u64,
            restored_at: row.get(16)?,
            purged_at: row.get(17)?,
            quota_root: row.get(18)?,
        })
    }
}

const SELECT_COLS: &str = "id, actor_type, actor_id, original_path, trash_path, kind, size, \
    files, mode, owner, sha256, site, protected, deleted_at, expires_at, nlink, restored_at, \
    purged_at, quota_root";

impl super::Db {
    pub fn trash_insert(&self, row: &TrashRow) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO trash_entries(
                id, actor_type, actor_id, original_path, trash_path, kind, size, files,
                mode, owner, sha256, site, protected, deleted_at, expires_at, nlink,
                restored_at, purged_at, quota_root
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                row.id,
                row.actor_type,
                row.actor_id,
                row.original_path,
                row.trash_path,
                row.kind,
                row.size as i64,
                row.files as i64,
                row.mode,
                row.owner,
                row.sha256,
                row.site,
                row.protected as i64,
                row.deleted_at,
                row.expires_at,
                row.nlink as i64,
                row.restored_at,
                row.purged_at,
                row.quota_root,
            ],
        )?;
        Ok(())
    }

    pub fn trash_get(&self, id: &str) -> Result<Option<TrashRow>> {
        let conn = self.conn();
        let sql = format!("SELECT {SELECT_COLS} FROM trash_entries WHERE id = ?1");
        Ok(conn
            .query_row(&sql, params![id], TrashRow::from_row)
            .optional()?)
    }

    /// 所有仍可用条目（未恢复、未彻底删除），按删除时间升序（淘汰策略依赖顺序）。
    pub fn trash_live(&self) -> Result<Vec<TrashRow>> {
        let conn = self.conn();
        let sql = format!(
            "SELECT {SELECT_COLS} FROM trash_entries
             WHERE restored_at IS NULL AND purged_at IS NULL
             ORDER BY deleted_at ASC, id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], TrashRow::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn trash_remove(&self, id: &str) -> Result<Option<TrashRow>> {
        let conn = self.conn();
        let existing = {
            let sql = format!("SELECT {SELECT_COLS} FROM trash_entries WHERE id = ?1");
            conn.query_row(&sql, params![id], TrashRow::from_row)
                .optional()?
        };
        if existing.is_some() {
            conn.execute("DELETE FROM trash_entries WHERE id = ?1", params![id])?;
        }
        Ok(existing)
    }

    pub fn trash_mark_restored(&self, id: &str, ts: i64) -> Result<Option<TrashRow>> {
        self.trash_mark("restored_at", id, ts)
    }

    pub fn trash_mark_purged(&self, id: &str, ts: i64) -> Result<Option<TrashRow>> {
        self.trash_mark("purged_at", id, ts)
    }

    fn trash_mark(&self, column: &str, id: &str, ts: i64) -> Result<Option<TrashRow>> {
        let conn = self.conn();
        conn.execute(
            &format!("UPDATE trash_entries SET {column} = ?1 WHERE id = ?2"),
            params![ts, id],
        )?;
        let sql = format!("SELECT {SELECT_COLS} FROM trash_entries WHERE id = ?1");
        Ok(conn
            .query_row(&sql, params![id], TrashRow::from_row)
            .optional()?)
    }
}
