//! 操作面：读写、分页、列表、复制、改名、硬链接、原子性与锁（契约 §3）。

mod support;

use std::fs;
use std::sync::Arc;

use skiff_fs::{PathKind, ReadRequest, WriteMode, WriteRequest};
use support::{ctx, Harness};

fn write_req(path: &str, data: &[u8], mode: WriteMode) -> WriteRequest {
    WriteRequest {
        path: path.to_string(),
        data: data.to_vec(),
        mode,
    }
}

fn read_full(h: &Harness, p: &skiff_fs::Principal, path: &str) -> Vec<u8> {
    let chunk =
        h.fs.read(
            p,
            &ctx(),
            &ReadRequest {
                path: path.to_string(),
                offset: 0,
                max_bytes: 0,
            },
        )
        .unwrap();
    chunk.bytes
}

#[test]
fn write_read_append_and_stat_hash() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = h.data_path("notes.txt");

    let out =
        h.fs.write(&p, &ctx(), &write_req(&path, b"abc", WriteMode::Replace))
            .unwrap();
    assert_eq!(out.size, 3);
    assert!(out.created);
    assert!(out.atomic);
    assert_eq!(out.sha256, skiff_fs::sha256_hex(b"abc"));
    assert_eq!(read_full(&h, &p, &path), b"abc");

    let appended =
        h.fs.write(&p, &ctx(), &write_req(&path, b"def", WriteMode::Append))
            .unwrap();
    assert!(!appended.atomic);
    assert_eq!(appended.size, 6);
    assert_eq!(read_full(&h, &p, &path), b"abcdef");

    let info =
        h.fs.stat(
            &p,
            &ctx(),
            &skiff_fs::StatRequest {
                path: path.clone(),
                with_hash: true,
            },
        )
        .unwrap();
    assert_eq!(info.kind, PathKind::File);
    assert_eq!(info.size, 6);
    assert_eq!(
        info.sha256.as_deref(),
        Some("bef57ec7f53a6d40beb640a780a639c83bc29ac8a9816f1fc6c5c6dcd93c4721")
    );
}

#[test]
fn read_is_paged_and_truncated_flag_is_honest() {
    let h = Harness::with_config(|c| c.max_request_bytes = 8);
    let p = h.plugin("com.example.demo");
    let path = h.data_path("big.txt");
    h.seed(&path, b"0123456789");

    let first =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: path.clone(),
                offset: 0,
                max_bytes: 100,
            },
        )
        .unwrap();
    assert_eq!(first.bytes, b"01234567");
    assert!(first.truncated, "请求超过上限必须标记 truncated");
    assert!(!first.eof);

    let tail =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: path.clone(),
                offset: 8,
                max_bytes: 8,
            },
        )
        .unwrap();
    assert_eq!(tail.bytes, b"89");
    assert!(tail.eof);
    assert!(!tail.truncated);
}

#[test]
fn read_of_directory_is_conflict() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    fs::create_dir_all(h.data().join("sub")).unwrap();
    let err =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: h.data_path("sub"),
                offset: 0,
                max_bytes: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.conflict");
}

#[test]
fn request_and_file_size_limits_are_distinct_errors() {
    let h = Harness::with_config(|c| {
        c.max_request_bytes = 8;
        c.max_file_bytes = 32;
    });
    let p = h.plugin("com.example.demo");
    let too_big_request =
        h.fs.write(
            &p,
            &ctx(),
            &write_req(&h.data_path("a.txt"), &[0u8; 9], WriteMode::Replace),
        )
        .unwrap_err();
    assert_eq!(too_big_request.code(), "fs.too_large");

    // 先删掉请求上限的影响：分两次追加，第二次触发单文件上限
    h.fs.write(
        &p,
        &ctx(),
        &write_req(&h.data_path("b.txt"), &[0u8; 8], WriteMode::Replace),
    )
    .unwrap();
    for _ in 0..3 {
        h.fs.write(
            &p,
            &ctx(),
            &write_req(&h.data_path("b.txt"), &[0u8; 8], WriteMode::Append),
        )
        .unwrap();
    }
    let err =
        h.fs.write(
            &p,
            &ctx(),
            &write_req(&h.data_path("b.txt"), &[0u8; 8], WriteMode::Append),
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.file_too_large");
}

#[test]
fn grant_root_quota_is_enforced() {
    let h = Harness::with_config(|c| c.max_root_bytes = 15);
    let p = h.plugin("com.example.demo");
    h.fs.write(
        &p,
        &ctx(),
        &write_req(&h.data_path("q1.txt"), &[1u8; 10], WriteMode::Replace),
    )
    .unwrap();
    let err =
        h.fs.write(
            &p,
            &ctx(),
            &write_req(&h.data_path("q2.txt"), &[2u8; 10], WriteMode::Replace),
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.quota_exceeded");
}

#[test]
fn list_is_sorted_and_paged() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    for name in ["c.txt", "a.txt", "b.txt"] {
        h.seed(&h.data_path(name), b"x");
    }
    fs::create_dir_all(h.data().join("dir")).unwrap();

    let page1 =
        h.fs.list(
            &p,
            &ctx(),
            &skiff_fs::ListRequest {
                path: h.data().to_string_lossy().to_string(),
                page: 1,
                page_size: 2,
            },
        )
        .unwrap();
    assert_eq!(page1.total, 4);
    assert_eq!(
        page1
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "b.txt"]
    );
    assert!(page1.has_more);

    let page2 =
        h.fs.list(
            &p,
            &ctx(),
            &skiff_fs::ListRequest {
                path: h.data().to_string_lossy().to_string(),
                page: 2,
                page_size: 2,
            },
        )
        .unwrap();
    assert_eq!(
        page2
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["c.txt", "dir"]
    );
    assert!(!page2.has_more);
}

#[cfg(unix)]
#[test]
fn list_reports_symlink_kind_and_survives_dangling_link() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("real.txt"), b"x");
    std::os::unix::fs::symlink("real.txt", h.data().join("link.txt")).unwrap();
    // 悬空链接不能让整个目录列表失败（此前 DirEntry::metadata 会跟随链接报 NotFound）。
    std::os::unix::fs::symlink("missing-target", h.data().join("dangling")).unwrap();

    let page =
        h.fs.list(
            &p,
            &ctx(),
            &skiff_fs::ListRequest {
                path: h.data().to_string_lossy().to_string(),
                page: 1,
                page_size: 10,
            },
        )
        .unwrap();
    assert_eq!(page.total, 3);
    let link = page.entries.iter().find(|e| e.name == "link.txt").unwrap();
    assert_eq!(link.kind, PathKind::Symlink);
    assert_eq!(link.link_target.as_deref(), Some("real.txt"));
    let dangling = page.entries.iter().find(|e| e.name == "dangling").unwrap();
    assert_eq!(dangling.kind, PathKind::Symlink);
}

#[test]
fn mkdir_and_conflict() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    h.fs.mkdir(&p, &ctx(), &h.data_path("sub")).unwrap();
    assert_eq!(
        h.fs.mkdir(&p, &ctx(), &h.data_path("sub"))
            .unwrap_err()
            .code(),
        "fs.conflict"
    );
    // 父目录不存在 → not_found（不自动 create_dir_all）
    assert_eq!(
        h.fs.mkdir(&p, &ctx(), &h.data_path("a/b/c"))
            .unwrap_err()
            .code(),
        "fs.not_found"
    );
}

#[test]
fn copy_file_and_directory() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("src/a.txt"), b"AAA");
    h.seed(&h.data_path("src/sub/b.txt"), b"BBB");

    let out =
        h.fs.copy(&p, &ctx(), &h.data_path("src"), &h.data_path("dst"), false)
            .unwrap();
    assert_eq!(out.entries, 3, "目录 + 两个文件");
    assert_eq!(read_full(&h, &p, &h.data_path("dst/sub/b.txt")), b"BBB");

    assert_eq!(
        h.fs.copy(
            &p,
            &ctx(),
            &h.data_path("src/a.txt"),
            &h.data_path("dst/a.txt"),
            false
        )
        .unwrap_err()
        .code(),
        "fs.conflict"
    );
}

#[test]
fn copy_respects_entry_limit() {
    let h = Harness::with_config(|c| c.copy_max_entries = 3);
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("src/a.txt"), b"a");
    h.seed(&h.data_path("src/b.txt"), b"b");
    h.seed(&h.data_path("src/c.txt"), b"c");
    h.seed(&h.data_path("src/d.txt"), b"d");
    let err =
        h.fs.copy(&p, &ctx(), &h.data_path("src"), &h.data_path("dst"), false)
            .unwrap_err();
    assert_eq!(err.code(), "fs.too_large");
}

/// 目录复制走「同父临时目录 + 原子 rename」：条目超限中途失败时，目标位置不得
/// 出现半成品目录，也不得残留 `.skiff-tmp-*` 临时物。
#[test]
fn copy_dir_failure_leaves_no_partial_target() {
    let h = Harness::with_config(|c| c.copy_max_entries = 2);
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("src/a.txt"), b"a");
    h.seed(&h.data_path("src/b.txt"), b"b");
    h.seed(&h.data_path("src/c.txt"), b"c");
    let err =
        h.fs.copy(&p, &ctx(), &h.data_path("src"), &h.data_path("dst"), false)
            .unwrap_err();
    assert_eq!(err.code(), "fs.too_large");
    assert!(
        !std::path::Path::new(&h.data_path("dst")).exists(),
        "失败时目标目录不得出现"
    );
    let leftovers: Vec<String> = std::fs::read_dir(h.data())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".skiff-tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "残留临时物: {leftovers:?}");
}

#[test]
fn rename_rules() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    h.seed(&h.data_path("a.txt"), b"A");
    h.seed(&h.data_path("b.txt"), b"B");

    assert_eq!(
        h.fs.rename(
            &p,
            &ctx(),
            &h.data_path("a.txt"),
            &h.data_path("b.txt"),
            true
        )
        .unwrap_err()
        .code(),
        "fs.denied",
        "插件不得覆盖"
    );
    assert_eq!(
        h.fs.rename(
            &p,
            &ctx(),
            &h.data_path("a.txt"),
            &h.data_path("b.txt"),
            false
        )
        .unwrap_err()
        .code(),
        "fs.conflict"
    );
    h.fs.rename(
        &p,
        &ctx(),
        &h.data_path("a.txt"),
        &h.data_path("c.txt"),
        false,
    )
    .unwrap();
    assert_eq!(read_full(&h, &p, &h.data_path("c.txt")), b"A");

    // 管理员可覆盖普通文件
    h.seed(&h.data_path("a.txt"), b"NEW");
    h.fs.rename(
        &h.admin(),
        &ctx(),
        &h.data_path("a.txt"),
        &h.data_path("c.txt"),
        true,
    )
    .unwrap();
    assert_eq!(read_full(&h, &p, &h.data_path("c.txt")), b"NEW");
}

#[test]
fn hardlink_removal_keeps_other_link() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let a = h.data_path("a.txt");
    let b = h.data_path("b.txt");
    h.seed(&a, b"shared");
    fs::hard_link(&a, &b).unwrap();

    let out = h.fs.remove(&p, &ctx(), &a, false).unwrap();
    assert!(out.trash_id.is_some());
    assert_eq!(fs::read(&b).unwrap(), b"shared");
}

#[test]
fn atomic_replacement_has_no_torn_file() {
    // 2 核 Windows runner 上，8 线程 × 20 次同路径争锁（每次 Replace 都含 fsync）
    // 会顶满 5s 锁等待上限而偶发 `fs.busy`。降到 4 线程 × 10 次，并只在本用例内
    // 放大锁等待余量；生产默认 `lock_timeout` 保持不变。断言仍是「最终文件必须
    // 等于某次完整写入」，撕裂写依旧会被抓到。
    let h = Harness::with_config(|cfg| cfg.lock_timeout = std::time::Duration::from_secs(30));
    let p = Arc::new(h.plugin("com.example.demo"));
    let path = h.data_path("race.txt");
    let payloads: Vec<Vec<u8>> = (0..4u8).map(|i| vec![i; 1000 + i as usize]).collect();

    let mut handles = Vec::new();
    for payload in payloads.clone() {
        let fs = h.fs.clone();
        let p = p.clone();
        let path = path.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..10 {
                fs.write(
                    &p,
                    &support::ctx(),
                    &WriteRequest {
                        path: path.clone(),
                        data: payload.clone(),
                        mode: WriteMode::Replace,
                    },
                )
                .unwrap();
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let final_bytes = fs::read(&path).unwrap();
    assert!(
        payloads.contains(&final_bytes),
        "最终内容必须是某次完整写入，不能是半文件"
    );
}

#[test]
fn concurrent_swaps_do_not_deadlock_or_time_out() {
    let h = Harness::new();
    let p = Arc::new(h.admin());
    let a = h.data_path("a.txt");
    let b = h.data_path("b.txt");
    h.seed(&a, b"A");
    h.seed(&b, b"B");

    let mut handles = Vec::new();
    for (from, to) in [(a.clone(), b.clone()), (b.clone(), a.clone())] {
        let fs = h.fs.clone();
        let p = p.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..50 {
                let _ = fs.rename(&p, &support::ctx(), &from, &to, true);
            }
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    // 只要没有出现 fs.busy 即视为锁序正确（结果可能是 conflict/not_found）
    let busy = h
        .audit_events()
        .into_iter()
        .filter(|e| e.code.as_deref() == Some("fs.busy"))
        .count();
    assert_eq!(busy, 0);
}

#[cfg(unix)]
#[test]
fn atomic_write_preserves_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = h.data_path("mode.txt");
    h.seed(&path, b"old");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    h.fs.write(&p, &ctx(), &write_req(&path, b"new", WriteMode::Replace))
        .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn remove_permanent_requires_admin() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = h.data_path("x.txt");
    h.seed(&path, b"x");
    assert_eq!(
        h.fs.remove(&p, &ctx(), &path, true).unwrap_err().code(),
        "fs.denied"
    );
    h.fs.remove(&h.admin(), &ctx(), &path, true).unwrap();
    assert!(!std::path::Path::new(&path).exists());
}
