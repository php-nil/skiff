//! 迁移用例：幂等、单步失败整步回滚、旧库逐版升级到最新形状。
//!
//! 逐版升级的用例直接执行历史 DDL 常量构造旧库，再走 `Db::open` 的真实迁移路径，
//! 所以新增台阶时它们会自动把「新库从哪些旧版本升上来」覆盖一遍。

use super::schema::*;
use super::*;
use crate::Db;
use crate::ORIGIN_SEED;

#[test]
fn migration_is_idempotent_and_records_version() {
    let db = Db::open_in_memory().unwrap();
    assert!(apply(&db.conn()).is_ok());
    let v: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, CURRENT_SCHEMA_VERSION);
}

/// 单步迁移失败时必须整体回滚：DDL 与 user_version 都不能留下。
#[test]
fn migration_step_rolls_back_ddl_and_version_on_failure() {
    let db = Db::open_in_memory().unwrap();
    let conn = db.conn();
    let before: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let err = step(
        &conn,
        before,
        16,
        "CREATE TABLE migrate_probe(id INTEGER); INSERT INTO no_such_table VALUES(1);",
    );
    assert!(err.is_err());
    let after: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before);
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'migrate_probe'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0, "失败步骤建的表必须被回滚");
}

/// 迁移链落新结构：旧站点表被重建，内置 PHP 行被标记为未安装种子。
#[test]
fn migrations_rebuild_sites_and_mark_php_seed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skiff.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute_batch(SCHEMA_V4).unwrap();
        conn.execute_batch(SCHEMA_V5).unwrap();
        conn.execute_batch(SCHEMA_V6).unwrap();
        conn.execute_batch("PRAGMA user_version = 6").unwrap();
        conn.execute(
            "INSERT INTO sites(domain, root_dir, php_version, status, created_at, updated_at)
                 VALUES('legacy.local', '/www/sites/legacy.local', '8.5', 'running', 1, 1)",
            [],
        )
        .unwrap();
    }
    let db = Db::open(&path).unwrap();
    let v: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, CURRENT_SCHEMA_VERSION);
    assert!(db.site_by_domain("legacy.local").unwrap().is_none());
    let seeded = db.runtime_by_id("php-8.5").unwrap().unwrap();
    assert_eq!(seeded.kind, "php");
    assert_eq!(seeded.name, "php-8.5");
    assert!(seeded.is_default);
    assert_eq!(seeded.origin, ORIGIN_SEED);
    assert!(
        seeded.socket_path.is_none(),
        "从未安装的种子行不应登记 socket 路径"
    );
}

#[test]
fn v13_database_upgrades_through_latest_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skiff.db");
    {
        let conn = Connection::open(&path).unwrap();
        for batch in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
            SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
        ] {
            conn.execute_batch(batch).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 13").unwrap();
    }
    let db = Db::open(&path).unwrap();
    let v: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, CURRENT_SCHEMA_VERSION);
    let exists = |name: &str| -> bool {
        db.conn()
            .query_row("SELECT 1 FROM sqlite_master WHERE name=?1", [name], |_| {
                Ok(())
            })
            .is_ok()
    };
    assert!(exists("trash_entries"));
    assert!(exists("fs_quotas"));
    // v15：任务中心的两张表与「同目标仅一个活跃作业」的部分唯一索引。
    assert!(exists("jobs"));
    assert!(exists("job_logs"));
    assert!(exists("idx_jobs_active_target"));
}

/// v18：已部署的 v17 库物理上还留着 `app_command` / `app_port` 两列。
/// 迁移必须把表重建掉，且站点与别名数据原样保留。
#[test]
fn v17_database_drops_dead_site_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skiff.db");
    {
        let conn = Connection::open(&path).unwrap();
        for batch in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
            SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15,
            SCHEMA_V16, SCHEMA_V17,
        ] {
            conn.execute_batch(batch).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 17").unwrap();
        conn.execute(
                "INSERT INTO sites(domain, root_dir, site_type, app_command, app_port, status,
                                   created_at, updated_at)
                 VALUES('keep.local', '/www/sites/keep.local', 'static', NULL, NULL, 'running', 1, 1)",
                [],
            )
            .unwrap();
        conn.execute(
            "INSERT INTO site_aliases(domain, site_domain, created_at)
                 VALUES('alias.local', 'keep.local', 1)",
            [],
        )
        .unwrap();
    }
    let db = Db::open(&path).unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);

    let names: Vec<String> = {
        let conn = db.conn();
        let mut stmt = conn.prepare("PRAGMA table_info(sites)").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert!(!names.iter().any(|n| n == "app_command"), "{names:?}");
    assert!(!names.iter().any(|n| n == "app_port"), "{names:?}");

    let site = db.site_by_domain("keep.local").unwrap().unwrap();
    assert_eq!(site.site_type, "static");
    assert_eq!(db.site_aliases("keep.local").unwrap(), vec!["alias.local"]);
}

/// v19：keyset 分页索引必须覆盖 `(时间, id)`，迁移前已有的审计/作业数据原样保留。
#[test]
fn v18_database_upgrades_keyset_indexes_and_keeps_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skiff.db");
    {
        let conn = Connection::open(&path).unwrap();
        for batch in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8,
            SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13, SCHEMA_V14, SCHEMA_V15,
            SCHEMA_V16, SCHEMA_V17, SCHEMA_V18,
        ] {
            conn.execute_batch(batch).unwrap();
        }
        conn.execute_batch("PRAGMA user_version = 18").unwrap();
        conn.execute(
            "INSERT INTO audit(ts, actor, ip, action, target, result, detail)
                 VALUES(100, 'admin', '127.0.0.1', 'login', 'session', 'ok', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs(id, kind, target_type, target_id, title, phase, phase_label,
                                   status, created_by, created_by_role, created_at)
                 VALUES('j1', 'image.pull', 'image', 'php:8.5', '拉取镜像', 'pull', '拉取',
                        'failed', 'admin', 'admin', 200)",
            [],
        )
        .unwrap();
    }
    let db = Db::open(&path).unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);

    let index_sql = |name: &str| -> String {
        db.conn()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert!(
        index_sql("idx_audit_ts").contains("id DESC"),
        "审计索引缺 id 排序键"
    );
    assert!(
        index_sql("idx_jobs_created").contains("id DESC"),
        "作业列表索引缺 id 排序键"
    );
    assert!(
        index_sql("idx_jobs_status").contains("id DESC"),
        "作业状态索引缺 id 排序键"
    );

    // 索引真的被 keyset 查询用上，而不只是建了放着。
    let plan = |sql: &str| -> String {
        let conn = db.conn();
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        stmt.query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join(" | ")
    };
    assert!(
        plan("SELECT id FROM audit ORDER BY ts DESC, id DESC LIMIT 5").contains("idx_audit_ts"),
        "审计 keyset 查询未走新索引"
    );
    assert!(
        plan("SELECT id FROM jobs WHERE status='failed' ORDER BY created_at DESC, id DESC LIMIT 5")
            .contains("idx_jobs_status"),
        "作业状态筛选未走新索引"
    );

    assert_eq!(db.recent_audit(10).unwrap().len(), 1);
    assert!(db.job("j1").unwrap().is_some());
}
