//! 维护车道（maintenance lane）：备份、审计裁剪等**长持锁**操作。
//!
//! 与 `db_async` 的短事务车道分开表达，避免以后误以为「加大 permit / 单独限流」能解决
//! 维护窗口内的排队——permit 只约束调用方显式投递的短事务，维护路径不经过它，真正的
//! 串行点是 `Db` 的连接互斥（见 `docs/ARCH_REVIEW.md` §15.5，唯一口径）。
//!
//! 约束：
//! - 维护操作**进程内永远串行**（[`serial_lock`]），备份与审计裁剪不会并行；
//! - 维护操作记 `lane = "maintenance"` 的结构化日志（耗时 / 行数 / 字节），与短事务区分；
//! - 跨进程（CLI `skiff backup` 等）仍由 SQLite `busy_timeout` 协调，进程内锁管不到。
//!
//! 备份连接的选型（后续步骤落地）：WAL 库**不要用 `SQLITE_OPEN_READ_ONLY` 打开**，
//! 只读连接在缺少 `-shm`/`-wal` 写权限时会失败；用普通连接、只发读/VACUUM INTO，
//! 并把 `busy_timeout` 设得比主连接短，遇到跨进程争用立即报错而不是叠加等待。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use tracing::{info, warn};

use super::{Db, Result};

/// 进程内维护串行锁：同一时刻只允许一个维护操作。
static SERIAL: Mutex<()> = Mutex::new(());

/// 审计裁剪单批行数：把一次大 DELETE 拆成多个小事务，锁只在一批内持有。
const AUDIT_PRUNE_BATCH: usize = 5000;
/// 批间让出时长：给短事务车道一个插入窗口，避免维护把锁一直攥在手里。
/// 取值以「等待方足够拿到一次锁」为准，不追求更长；单批本身才是等待上界。
const BATCH_YIELD: Duration = Duration::from_millis(5);

fn serial_lock() -> MutexGuard<'static, ()> {
    // 与 `Db::conn` 同口径：锁中毒说明之前有 panic，取回内部值继续，避免永久卡死。
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

impl Db {
    /// 一致性快照备份（§5.2）：用 `VACUUM INTO`，禁止对活动库直接 tar。
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<()> {
        let _serial = serial_lock();
        let started = Instant::now();
        let dest = dest.as_ref();
        // 目标必须是活动库之外的路径：否则下面会先 unlink 正在使用的库文件，
        // 进程继续写已删除 inode，重启后数据全丢。
        let dest_cmp = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
        let src_cmp = self
            .path
            .canonicalize()
            .unwrap_or_else(|_| self.path.clone());
        if dest_cmp == src_cmp {
            return Err(super::StoreError::Conflict(
                "备份目标不能是活动数据库本身".into(),
            ));
        }
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        let conn = self.conn_lane("maintenance");
        let dest_str = dest.to_string_lossy().to_string();
        conn.execute("VACUUM INTO ?1", params![dest_str])?;
        // 必须打开备份文件本身校验；在源连接上跑 integrity_check 查的是活动库。
        let check: String =
            Connection::open_with_flags(dest, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            drop(conn);
            let _ = std::fs::remove_file(dest);
            let err = super::StoreError::Decode(format!("备份库完整性检查失败: {check}"));
            warn!(
                lane = "maintenance",
                op = "backup",
                duration_ms = elapsed_ms(started),
                error = %err,
                "维护操作失败"
            );
            return Err(err);
        }
        drop(conn);
        let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        info!(
            lane = "maintenance",
            op = "backup",
            duration_ms = elapsed_ms(started),
            bytes,
            "维护操作完成"
        );
        Ok(())
    }

    /// 保留策略：删除 `ts < cutoff` 的审计记录，返回删除条数。
    ///
    /// 分批删除：每批 [`AUDIT_PRUNE_BATCH`] 行、一个自动提交事务，批间让出
    /// [`BATCH_YIELD`]。每批持锁时间与单批行数成正比，短事务车道可在间隙插入；
    /// 部分完成是安全的（幂等 + 保留窗口），崩溃后已提交批次保留，下次继续。
    pub fn purge_audit_before(&self, cutoff: i64) -> Result<usize> {
        let _serial = serial_lock();
        let started = Instant::now();
        let mut total = 0usize;
        let mut batches = 0u32;
        loop {
            let deleted = {
                let conn = self.conn_lane("maintenance");
                conn.execute(
                    "DELETE FROM audit WHERE id IN (
                         SELECT id FROM audit WHERE ts < ?1 ORDER BY ts LIMIT ?2
                     )",
                    params![cutoff, AUDIT_PRUNE_BATCH as i64],
                )?
            };
            total += deleted;
            batches += 1;
            if deleted < AUDIT_PRUNE_BATCH {
                break;
            }
            std::thread::sleep(BATCH_YIELD);
        }
        info!(
            lane = "maintenance",
            op = "audit_prune",
            duration_ms = elapsed_ms(started),
            rows = total,
            batches,
            "维护操作完成"
        );
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 保留清理按时间窗删除：窗口内保留、窗口外删除，且重复执行幂等。
    #[test]
    fn purge_audit_before_deletes_only_out_of_window_rows() {
        let db = Db::open_in_memory().unwrap();
        for i in 0..3 {
            db.audit("a", "1.2.3.4", "login", &format!("s{i}"), "ok", "")
                .unwrap();
        }
        // 截止时间为 0：没有记录早于它，保留全部。
        assert_eq!(db.purge_audit_before(0).unwrap(), 0);
        assert_eq!(db.recent_audit(10).unwrap().len(), 3);

        // 截止时间取未来：全部删除。
        assert_eq!(db.purge_audit_before(i64::MAX).unwrap(), 3);
        assert_eq!(db.recent_audit(10).unwrap().len(), 0);

        // 幂等：再跑一次不重复计数。
        assert_eq!(db.purge_audit_before(i64::MAX).unwrap(), 0);
    }

    /// 超过单批上限的裁剪：分批删完、计数累计，窗口内的行保留。
    #[test]
    fn purge_audit_before_batches_large_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skiff.db");
        let db = Db::open(&path).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("BEGIN").unwrap();
            let mut stmt = conn
                .prepare(
                    "INSERT INTO audit(ts, actor, ip, action, target, result, detail)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .unwrap();
            for i in 0..12_345 {
                stmt.execute(params![1, "a", "1.2.3.4", "old", format!("t{i}"), "ok", ""])
                    .unwrap();
            }
            for i in 0..5 {
                stmt.execute(params![
                    1000,
                    "a",
                    "1.2.3.4",
                    "keep",
                    format!("k{i}"),
                    "ok",
                    ""
                ])
                .unwrap();
            }
            drop(stmt);
            conn.execute_batch("COMMIT").unwrap();
        }

        assert_eq!(db.purge_audit_before(100).unwrap(), 12_345);
        assert_eq!(db.recent_audit(10).unwrap().len(), 5);
        // 已清空：再跑一次不重复计数。
        assert_eq!(db.purge_audit_before(100).unwrap(), 0);
    }
}
