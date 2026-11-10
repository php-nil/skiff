//! 目录树配额仓储（schema v14 的 `fs_quotas` 表）。
//!
//! 表里同时保存「上限」和「缓存用量」。用量由 `skiff-fs` 在每次成功操作后
//! 增量修正；`scanned_at` 记录最近一次全量重扫时间，`NULL` 表示从未重扫。

use rusqlite::{params, OptionalExtension, Row};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRow {
    pub root_path: String,
    pub max_bytes: Option<u64>,
    pub max_files: Option<u64>,
    pub used_bytes: u64,
    pub used_files: u64,
    pub updated_at: i64,
    pub scanned_at: Option<i64>,
    /// 最近一次全量统计是否因条目数超限而截断（用量只是下界）。
    pub truncated: bool,
}

impl QuotaRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            root_path: row.get(0)?,
            max_bytes: row.get::<_, Option<i64>>(1)?.map(|v| v.max(0) as u64),
            max_files: row.get::<_, Option<i64>>(2)?.map(|v| v.max(0) as u64),
            used_bytes: row.get::<_, i64>(3)?.max(0) as u64,
            used_files: row.get::<_, i64>(4)?.max(0) as u64,
            updated_at: row.get(5)?,
            scanned_at: row.get(6)?,
            truncated: row.get::<_, i64>(7)? != 0,
        })
    }
}

const SELECT_COLS: &str =
    "root_path, max_bytes, max_files, used_bytes, used_files, updated_at, scanned_at, truncated";

impl super::Db {
    pub fn fs_quota_list(&self) -> Result<Vec<QuotaRow>> {
        let conn = self.conn();
        let sql = format!("SELECT {SELECT_COLS} FROM fs_quotas ORDER BY root_path ASC");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], QuotaRow::from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn fs_quota_get(&self, root_path: &str) -> Result<Option<QuotaRow>> {
        let conn = self.conn();
        let sql = format!("SELECT {SELECT_COLS} FROM fs_quotas WHERE root_path = ?1");
        Ok(conn
            .query_row(&sql, params![root_path], QuotaRow::from_row)
            .optional()?)
    }

    /// 设置/修改上限并写入一次全量统计结果。
    #[allow(clippy::too_many_arguments)]
    pub fn fs_quota_set(
        &self,
        root_path: &str,
        max_bytes: Option<u64>,
        max_files: Option<u64>,
        used_bytes: u64,
        used_files: u64,
        truncated: bool,
        scanned_at: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fs_quotas(root_path, max_bytes, max_files, used_bytes, used_files,
                truncated, updated_at, scanned_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?7)
             ON CONFLICT(root_path) DO UPDATE SET
                max_bytes = excluded.max_bytes,
                max_files = excluded.max_files,
                used_bytes = excluded.used_bytes,
                used_files = excluded.used_files,
                truncated = excluded.truncated,
                updated_at = excluded.updated_at,
                scanned_at = excluded.scanned_at",
            params![
                root_path,
                max_bytes.map(|v| v as i64),
                max_files.map(|v| v as i64),
                used_bytes as i64,
                used_files as i64,
                i64::from(truncated),
                scanned_at,
            ],
        )?;
        Ok(())
    }

    /// 仅刷新全量统计结果，保留既有上限。
    pub fn fs_quota_set_usage(
        &self,
        root_path: &str,
        used_bytes: u64,
        used_files: u64,
        truncated: bool,
        scanned_at: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fs_quotas(root_path, max_bytes, max_files, used_bytes, used_files,
                truncated, updated_at, scanned_at)
             VALUES (?1,NULL,NULL,?2,?3,?4,?5,?5)
             ON CONFLICT(root_path) DO UPDATE SET
                used_bytes = excluded.used_bytes,
                used_files = excluded.used_files,
                truncated = excluded.truncated,
                updated_at = excluded.updated_at,
                scanned_at = excluded.scanned_at",
            params![
                root_path,
                used_bytes as i64,
                used_files as i64,
                i64::from(truncated),
                scanned_at
            ],
        )?;
        Ok(())
    }

    /// 仅写上限，保留既有用量（调用方应已先 `set_usage`）。
    pub fn fs_quota_set_limits(
        &self,
        root_path: &str,
        max_bytes: Option<u64>,
        max_files: Option<u64>,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fs_quotas(root_path, max_bytes, max_files, used_bytes, used_files,
                updated_at, scanned_at)
             VALUES (?1,?2,?3,0,0,?4,NULL)
             ON CONFLICT(root_path) DO UPDATE SET
                max_bytes = excluded.max_bytes,
                max_files = excluded.max_files,
                updated_at = excluded.updated_at",
            params![
                root_path,
                max_bytes.map(|v| v as i64),
                max_files.map(|v| v as i64),
                super::now(),
            ],
        )?;
        Ok(())
    }

    /// 增量修正缓存用量；没有行时按「未设置上限」建行。
    pub fn fs_quota_adjust(
        &self,
        root_path: &str,
        delta_bytes: i64,
        delta_files: i64,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fs_quotas(root_path, max_bytes, max_files, used_bytes, used_files,
                updated_at, scanned_at)
             VALUES (?1,NULL,NULL,MAX(0,?2),MAX(0,?3),?4,NULL)
             ON CONFLICT(root_path) DO UPDATE SET
                used_bytes = MAX(0, used_bytes + ?2),
                used_files = MAX(0, used_files + ?3),
                updated_at = ?4",
            params![root_path, delta_bytes, delta_files, super::now()],
        )?;
        Ok(())
    }

    /// 取消上限但保留行与用量，页面继续显示「未设置上限」。
    pub fn fs_quota_clear_limits(&self, root_path: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO fs_quotas(root_path, max_bytes, max_files, used_bytes, used_files,
                updated_at, scanned_at)
             VALUES (?1,NULL,NULL,0,0,?2,NULL)
             ON CONFLICT(root_path) DO UPDATE SET
                max_bytes = NULL,
                max_files = NULL,
                updated_at = excluded.updated_at",
            params![root_path, super::now()],
        )?;
        Ok(())
    }
}
