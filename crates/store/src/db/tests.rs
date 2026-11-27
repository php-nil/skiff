//! `Db` 跨模块用例：备份快照、回收站与配额往返。
//!
//! 走 `Db` 公开面验证「写入后能读回、状态迁移不丢字段」；表级细节用例在各子模块内。

use super::*;
use crate::StoreError;
use crate::TrashRow;
use skiff_model::Role;

#[test]
fn backup_to_rejects_active_db_and_writes_valid_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skiff.db");
    let db = Db::open(&path).unwrap();
    assert!(matches!(db.backup_to(&path), Err(StoreError::Conflict(_))));

    let dest = dir.path().join("snapshot.db");
    db.backup_to(&dest).unwrap();
    let conn = Connection::open(&dest).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, CURRENT_SCHEMA_VERSION);
}

#[test]
fn trash_rows_roundtrip_and_state_transitions() {
    let db = Db::open_in_memory().unwrap();
    let row = TrashRow {
        id: "t1".into(),
        actor_type: "admin".into(),
        actor_id: "admin".into(),
        original_path: "/www/sites/a/x.txt".into(),
        trash_path: "/data/trash/2026/11/t1".into(),
        kind: "file".into(),
        size: 12,
        files: 1,
        mode: Some(0o644),
        owner: Some("1000:1000".into()),
        sha256: None,
        site: Some("a".into()),
        protected: false,
        deleted_at: 100,
        expires_at: 200,
        nlink: 1,
        restored_at: None,
        purged_at: None,
        quota_root: "/www/sites/a".into(),
    };
    db.trash_insert(&row).unwrap();
    assert_eq!(db.trash_get("t1").unwrap().unwrap(), row);
    assert_eq!(db.trash_live().unwrap().len(), 1);
    db.trash_mark_restored("t1", 101).unwrap();
    assert!(db.trash_live().unwrap().is_empty());
    let restored = db.trash_get("t1").unwrap().unwrap();
    assert_eq!(restored.restored_at, Some(101));
    db.trash_remove("t1").unwrap();
    assert!(db.trash_get("t1").unwrap().is_none());
}

#[test]
fn quota_adjust_set_cancel_roundtrip() {
    let db = Db::open_in_memory().unwrap();
    assert_eq!(db.fs_quota_get("/www/sites/a").unwrap(), None);
    db.fs_quota_adjust("/www/sites/a", 100, 2).unwrap();
    let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
    assert_eq!((row.used_bytes, row.used_files), (100, 2));
    assert_eq!((row.max_bytes, row.max_files), (None, None));

    db.fs_quota_set("/www/sites/a", Some(500), Some(10), 100, 2, true, 300)
        .unwrap();
    let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
    assert_eq!((row.max_bytes, row.max_files), (Some(500), Some(10)));
    assert_eq!(row.scanned_at, Some(300));
    assert!(row.truncated, "截断标记必须落库，否则页面刷新后丢失");
    db.fs_quota_set_usage("/www/sites/a", 100, 2, false, 400)
        .unwrap();
    assert!(!db.fs_quota_get("/www/sites/a").unwrap().unwrap().truncated);

    db.fs_quota_adjust("/www/sites/a", -150, 1).unwrap();
    let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
    assert_eq!((row.used_bytes, row.used_files), (0, 3));
    assert_eq!(row.max_bytes, Some(500), "增量修正不能丢上限");

    db.fs_quota_clear_limits("/www/sites/a").unwrap();
    let row = db.fs_quota_get("/www/sites/a").unwrap().unwrap();
    assert_eq!((row.max_bytes, row.max_files), (None, None));
    assert_eq!((row.used_bytes, row.used_files), (0, 3));
}

#[test]
fn backup_is_consistent_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("skiff.db");
    let db = Db::open(&db_path).unwrap();
    db.create_user("a", "h", Role::Admin, false).unwrap();
    let dest = dir.path().join("backup.db");
    db.backup_to(&dest).unwrap();
    assert!(dest.exists());
    let restored = Db::open(&dest).unwrap();
    assert_eq!(restored.count_users().unwrap(), 1);
}
