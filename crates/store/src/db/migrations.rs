//! Schema 迁移驱动：台阶表与单步事务。
//!
//! 迁移只向前；DDL 正文在 `schema.rs`，本文件只管「从哪个版本推到哪个版本」。
//! 库版本高于 `CURRENT_SCHEMA_VERSION` 时拒绝启动（旧二进制打不开新库）。

use rusqlite::Connection;
use tracing::{info, warn};

use super::CURRENT_SCHEMA_VERSION;
use crate::error::StoreError;
use crate::Result;

mod schema;

#[cfg(test)]
mod tests;

/// 把库推进到 `CURRENT_SCHEMA_VERSION`。
///
/// 每步幂等（`version >= to` 直接跳过），重复调用安全。
pub(super) fn apply(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    // 新增台阶 = 末尾加一行，数组长度由条目数推出，不另维护版本总数。
    let steps = [
        (1, schema::SCHEMA_V1),
        (2, schema::SCHEMA_V2),
        (3, schema::SCHEMA_V3),
        (4, schema::SCHEMA_V4),
        (5, schema::SCHEMA_V5),
        (6, schema::SCHEMA_V6),
        (7, schema::SCHEMA_V7),
        (8, schema::SCHEMA_V8),
        (9, schema::SCHEMA_V9),
        (10, schema::SCHEMA_V10),
        (11, schema::SCHEMA_V11),
        (12, schema::SCHEMA_V12),
        (13, schema::SCHEMA_V13),
        (14, schema::SCHEMA_V14),
        (15, schema::SCHEMA_V15),
        (16, schema::SCHEMA_V16),
        (17, schema::SCHEMA_V17),
        (18, schema::SCHEMA_V18),
        (19, schema::SCHEMA_V19),
        (20, schema::SCHEMA_V20),
        (21, schema::SCHEMA_V21),
        (22, schema::SCHEMA_V22),
    ];
    for (to, ddl) in steps {
        if version >= to {
            continue;
        }
        step(conn, version, to, ddl)?;
    }
    Ok(())
}

/// 单步迁移：DDL 与 `user_version` 在同一事务内提交。
///
/// 之前两条独立语句会让「列已加、版本未推进」成为可能，重启重跑
/// `ALTER TABLE ADD COLUMN` 直接 `duplicate column name`，面板永久起不来。
/// V8/V11 重建表需要在事务外关外键（`PRAGMA foreign_keys` 在事务内是 no-op）。
fn step(conn: &Connection, from: i64, to: i64, ddl: &str) -> Result<()> {
    let fk_off = matches!(to, 8 | 11 | 18);
    if fk_off {
        conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    }
    let result = (|| -> Result<()> {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        conn.execute_batch(ddl)?;
        conn.execute_batch(&format!("PRAGMA user_version = {to}"))?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(e) = result {
        if let Err(re) = conn.execute_batch("ROLLBACK") {
            warn!(error = %re, to, "schema 迁移回滚失败");
        }
        if fk_off {
            let _ = conn.execute_batch("PRAGMA foreign_keys=ON");
        }
        return Err(e);
    }
    if fk_off {
        conn.execute_batch("PRAGMA foreign_keys=ON")?;
    }
    info!(from, to, "schema 迁移完成");
    Ok(())
}
