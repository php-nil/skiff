//! 回收站：恢复、彻底删除、保留期、按根与全局配额（契约 §5）。

mod support;

use std::fs;
use std::path::PathBuf;

use skiff_fs::{OpContext, TrashStore};
use support::{ctx, Harness};

fn seed_file(h: &Harness, rel: &str, size: usize) -> String {
    let path = h.data_path(rel);
    h.seed(&path, &vec![7u8; size]);
    path
}

#[test]
fn remove_moves_to_trash_and_restore_brings_back() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = seed_file(&h, "a.txt", 10);

    let out = h.fs.remove(&p, &ctx(), &path, false).unwrap();
    let id = out.trash_id.clone().unwrap();
    assert!(!std::path::Path::new(&path).exists(), "原文件应已移走");

    let entry = h.trash.get(&id).unwrap();
    assert_eq!(entry.size, 10);
    assert_eq!(entry.quota_root, h.data().to_string_lossy());
    assert!(PathBuf::from(&entry.trash_path).exists());

    h.fs.restore(&p, &ctx(), &id, false).unwrap();
    assert_eq!(fs::read(&path).unwrap(), vec![7u8; 10]);
    assert!(h.trash.get(&id).unwrap().restored_at.is_some());
    assert_eq!(h.trash.total_usage(), 0);
    // 已恢复的条目不能再次恢复/彻底删除
    assert_eq!(
        h.fs.restore(&p, &ctx(), &id, false).unwrap_err().code(),
        "fs.conflict"
    );
    assert_eq!(
        h.fs.purge(&p, &ctx(), &id).unwrap_err().code(),
        "fs.conflict"
    );
}

#[test]
fn restore_refuses_existing_destination() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = seed_file(&h, "a.txt", 5);
    let id =
        h.fs.remove(&p, &ctx(), &path, false)
            .unwrap()
            .trash_id
            .unwrap();
    h.seed(&path, b"new");
    assert_eq!(
        h.fs.restore(&p, &ctx(), &id, false).unwrap_err().code(),
        "fs.conflict"
    );
    assert_eq!(fs::read(&path).unwrap(), b"new");
}

#[test]
fn directory_can_be_trashed_and_restored() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("dir/f.txt"), b"x");
    let out = h.fs.remove(&p, &ctx(), &h.data_path("dir"), false).unwrap();
    assert_eq!(out.kind, skiff_fs::PathKind::Dir);
    h.fs.restore(&p, &ctx(), &out.trash_id.unwrap(), false)
        .unwrap();
    assert!(h.data().join("dir/f.txt").exists());
}

#[test]
fn purge_removes_trash_object_and_metadata() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = seed_file(&h, "a.txt", 3);
    let id =
        h.fs.remove(&p, &ctx(), &path, false)
            .unwrap()
            .trash_id
            .unwrap();
    let tpath = h.trash.get(&id).unwrap().trash_path;
    h.fs.purge(&p, &ctx(), &id).unwrap();
    assert!(!PathBuf::from(tpath).exists());
    assert!(h.trash.get(&id).is_none());
    assert_eq!(h.trash.total_usage(), 0);
}

#[test]
fn purge_of_other_subject_is_denied() {
    let h = Harness::new();
    let owner = h.plugin("com.example.owner");
    let other = h.plugin("com.example.other");
    let path = seed_file(&h, "a.txt", 3);
    let id =
        h.fs.remove(&owner, &ctx(), &path, false)
            .unwrap()
            .trash_id
            .unwrap();
    assert_eq!(
        h.fs.purge(&other, &ctx(), &id).unwrap_err().code(),
        "fs.denied"
    );
    assert_eq!(
        h.fs.restore(&other, &ctx(), &id, false).unwrap_err().code(),
        "fs.denied"
    );
}

#[test]
fn retention_prune_removes_expired_entries() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = seed_file(&h, "a.txt", 3);
    let id =
        h.fs.remove(&p, &OpContext::at("r1", 1_000), &path, false)
            .unwrap()
            .trash_id
            .unwrap();
    let retention = 30 * 86_400;
    let removed =
        h.fs.prune_trash(&OpContext::at("r2", 1_000 + retention + 1))
            .unwrap();
    assert_eq!(removed, 1);
    assert!(h.trash.get(&id).is_none());
    let events = h.audit_events();
    let prune_events: Vec<_> = events
        .iter()
        .filter(|e| e.action == "trash.purge" && e.actor_type == "system")
        .collect();
    assert_eq!(prune_events.len(), 1);
}

#[test]
fn per_root_quota_evicts_oldest() {
    let h = Harness::with_config(|c| {
        c.trash.per_root_bytes = 100;
        c.trash.max_entries = 10_000;
    });
    let p = h.plugin("com.example.demo");
    let first = seed_file(&h, "one.bin", 60);
    let id1 =
        h.fs.remove(&p, &OpContext::at("r1", 100), &first, false)
            .unwrap()
            .trash_id
            .unwrap();
    let t1 = h.trash.get(&id1).unwrap().trash_path;
    let second = seed_file(&h, "two.bin", 60);
    let out =
        h.fs.remove(&p, &OpContext::at("r2", 200), &second, false)
            .unwrap();
    let id2 = out.trash_id.unwrap();

    assert!(h.trash.get(&id1).is_none(), "最旧条目应被淘汰");
    assert!(h.trash.get(&id2).is_some());
    assert!(!PathBuf::from(t1).exists());
    let evicted = h
        .audit_events()
        .into_iter()
        .any(|e| e.action == "trash.purge" && e.actor_type == "system");
    assert!(evicted, "配额淘汰需留下审计");
}

#[test]
fn global_quota_rejects_new_trash_entry() {
    let h = Harness::with_config(|c| c.trash.total_bytes = 100);
    let p = h.plugin("com.example.demo");
    let a = seed_file(&h, "a.bin", 60);
    h.fs.remove(&p, &ctx(), &a, false).unwrap();
    let b = seed_file(&h, "b.bin", 60);
    let err = h.fs.remove(&p, &ctx(), &b, false).unwrap_err();
    assert_eq!(err.code(), "fs.trash_full");
}

#[test]
fn oversized_entry_is_rejected_with_permanent_hint() {
    let h = Harness::with_config(|c| c.trash.per_entry_bytes = 10);
    let p = h.plugin("com.example.demo");
    let path = seed_file(&h, "big.bin", 20);
    let err = h.fs.remove(&p, &ctx(), &path, false).unwrap_err();
    assert_eq!(err.code(), "fs.too_large");
    assert!(err.message().contains("permanent"));
}

#[test]
fn plugin_cannot_restore_into_unauthed_path() {
    let h = Harness::new();
    let owner = h.plugin("com.example.owner");
    let path = seed_file(&h, "a.txt", 3);
    let id =
        h.fs.remove(&owner, &ctx(), &path, false)
            .unwrap()
            .trash_id
            .unwrap();

    // 另一个主体连自己条目都拿不到；同主体但授权根被撤销的场景由 resolve 拒绝
    let revoked = skiff_fs::Principal::plugin("com.example.owner", Vec::new(), None);
    assert_eq!(
        h.fs.restore(&revoked, &ctx(), &id, false)
            .unwrap_err()
            .code(),
        "fs.denied"
    );
}
