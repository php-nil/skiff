//! 目录树配额：写前预检、文件数上限、复制预检。

mod support;

use skiff_fs::{WriteMode, WriteRequest};
use support::{ctx, Harness};

fn write_req(path: &str, data: &[u8]) -> WriteRequest {
    WriteRequest {
        path: path.to_string(),
        data: data.to_vec(),
        mode: WriteMode::Replace,
    }
}

#[test]
fn quota_set_rejects_writes_over_byte_limit() {
    let h = Harness::new();
    let admin = h.admin();
    let root = h.data().display().to_string();
    let q =
        h.fs.quota_set(&admin, &ctx(), &root, Some(100), None)
            .unwrap();
    assert_eq!(q.root, root);
    assert_eq!(q.used_bytes, 0);

    let p = h.plugin("com.example.demo");
    let a = h.data_path("a.bin");
    let b = h.data_path("b.bin");
    assert_eq!(
        h.fs.write(&p, &ctx(), &write_req(&a, &[0u8; 80]))
            .unwrap()
            .size,
        80
    );
    let err =
        h.fs.write(&p, &ctx(), &write_req(&b, &[0u8; 40]))
            .unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");
    assert!(err.message().contains("容量配额超限"), "{}", err.message());

    // 覆盖写不新增文件数；本次仍在字节上限内。
    h.fs.write(&p, &ctx(), &write_req(&a, &[0u8; 60])).unwrap();
    let snap = h.fs.quota_list(&admin).unwrap();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].used_bytes, 60);
    assert_eq!(snap[0].max_bytes, Some(100));
}

#[test]
fn quota_set_rejects_new_file_by_count_but_allows_overwrite() {
    let h = Harness::new();
    let admin = h.admin();
    let root = h.data().display().to_string();
    h.fs.quota_set(&admin, &ctx(), &root, None, Some(2))
        .unwrap();

    let p = h.plugin("com.example.demo");
    let a = h.data_path("a.txt");
    let b = h.data_path("b.txt");
    let c = h.data_path("c.txt");
    h.fs.write(&p, &ctx(), &write_req(&a, b"a")).unwrap();
    h.fs.write(&p, &ctx(), &write_req(&b, b"b")).unwrap();
    let err = h.fs.write(&p, &ctx(), &write_req(&c, b"c")).unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");
    assert!(
        err.message().contains("文件数配额超限"),
        "{}",
        err.message()
    );
    // 覆盖已有文件不增加文件数。
    h.fs.write(&p, &ctx(), &write_req(&a, b"aa")).unwrap();
    let snap = h.fs.quota_list(&admin).unwrap();
    assert_eq!(snap[0].used_files, 2);
}

#[test]
fn copy_is_prechecked_against_destination_quota() {
    let h = Harness::new();
    let admin = h.admin();
    let root = h.data().display().to_string();
    let p = h.plugin("com.example.demo");

    let src = h.data_path("src.bin");
    h.fs.write(&p, &ctx(), &write_req(&src, &[0u8; 40]))
        .unwrap();
    h.fs.quota_set(&admin, &ctx(), &root, Some(100), None)
        .unwrap();

    h.fs.copy(&p, &ctx(), &src, &h.data_path("one.bin"), false)
        .unwrap();
    let err =
        h.fs.copy(&p, &ctx(), &src, &h.data_path("two.bin"), false)
            .unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");
    assert!(!std::path::Path::new(&h.data_path("two.bin")).exists());
}

#[test]
fn longest_matching_quota_root_wins() {
    let h = Harness::new();
    let admin = h.admin();
    let outer = h.data().display().to_string();
    let inner = h.data().join("inner");
    std::fs::create_dir_all(&inner).unwrap();
    h.fs.quota_set(&admin, &ctx(), &outer, Some(1000), None)
        .unwrap();
    h.fs.quota_set(&admin, &ctx(), &inner.display().to_string(), Some(50), None)
        .unwrap();

    let p = h.plugin("com.example.demo");
    let path = inner.join("x.bin").display().to_string();
    let err =
        h.fs.write(&p, &ctx(), &write_req(&path, &[0u8; 80]))
            .unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");
    assert!(err.message().contains("inner"), "{}", err.message());
}

#[test]
fn quota_cancel_keeps_usage_but_stops_enforcing() {
    let h = Harness::new();
    let admin = h.admin();
    let root = h.data().display().to_string();
    h.fs.quota_set(&admin, &ctx(), &root, Some(10), None)
        .unwrap();
    let p = h.plugin("com.example.demo");
    let a = h.data_path("a.bin");
    let err =
        h.fs.write(&p, &ctx(), &write_req(&a, &[0u8; 20]))
            .unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");

    let q = h.fs.quota_cancel(&admin, &ctx(), &root).unwrap();
    assert_eq!((q.max_bytes, q.max_files), (None, None));
    h.fs.write(&p, &ctx(), &write_req(&a, &[0u8; 20])).unwrap();
    let snap = h.fs.quota_list(&admin).unwrap();
    assert_eq!(snap[0].used_bytes, 20, "取消上限后计数继续");
}
