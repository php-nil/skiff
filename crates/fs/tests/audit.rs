//! 审计结构与分级策略（契约 §4）：不记内容、失败逐条、读多聚合。

mod support;

use std::sync::Arc;

use skiff_fs::{
    AggregatingSink, AuditSink, Fs, FsConfig, MemoryQuotaTracker, MemoryTrashStore, ReadRequest,
    TrashConfig, WriteMode, WriteRequest,
};
use support::{canonical_base, ctx, Harness};

#[test]
fn success_and_denied_events_carry_expected_fields() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let path = h.data_path("a.txt");
    h.fs.write(
        &p,
        &ctx(),
        &WriteRequest {
            path: path.clone(),
            data: b"secret-content".to_vec(),
            mode: WriteMode::Replace,
        },
    )
    .unwrap();

    let write_ev = h
        .audit_events()
        .into_iter()
        .find(|e| e.action == "fs.write")
        .unwrap();
    assert_eq!(write_ev.actor_type, "plugin");
    assert_eq!(write_ev.actor_id, "com.example.demo");
    assert_eq!(write_ev.request_id.as_deref(), Some("req-test-1"));
    assert_eq!(write_ev.bytes, 14);
    assert!(write_ev.sha256.is_some());
    assert_eq!(write_ev.result, "ok");

    // 越权读：审计必须是 deny，且带错误码与路径
    let outside = h.base().join("outside.txt");
    std::fs::write(&outside, b"x").unwrap();
    let err =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: outside.to_string_lossy().to_string(),
                offset: 0,
                max_bytes: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.escape");
    let denied = h
        .audit_events()
        .into_iter()
        .find(|e| e.action == "fs.read" && e.result == "deny")
        .unwrap();
    assert_eq!(denied.code.as_deref(), Some("fs.escape"));
    assert!(denied.src_path.is_some());
}

#[test]
fn audit_rows_never_include_file_content() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let secret = "TOP-SECRET-BODY";
    h.fs.write(
        &p,
        &ctx(),
        &WriteRequest {
            path: h.data_path("a.txt"),
            data: secret.as_bytes().to_vec(),
            mode: WriteMode::Replace,
        },
    )
    .unwrap();

    let rows = h.audit_events();
    assert!(!rows.is_empty());
    let json = serde_json::to_string(&rows).unwrap();
    assert!(!json.contains(secret), "审计不得记录内容");
    // 只记录摘要
    assert!(json.contains("sha256"));
}

#[test]
fn aggregating_sink_merges_reads_but_not_removes() {
    let tmp = tempfile::tempdir().unwrap();
    let base = canonical_base(tmp.path());
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let trash_dir = base.join("trash");
    std::fs::create_dir_all(&trash_dir).unwrap();

    let inner = Arc::new(skiff_fs::MemoryAuditSink::new());
    let agg: Arc<dyn AuditSink> = Arc::new(AggregatingSink::new(inner.clone()));
    let cfg = FsConfig {
        trash: TrashConfig {
            root: trash_dir,
            ..TrashConfig::default()
        },
        ..FsConfig::default()
    };
    let fs = Fs::new(
        cfg,
        agg.clone(),
        Arc::new(MemoryTrashStore::new()),
        Arc::new(MemoryQuotaTracker::new()),
    );

    let p = skiff_fs::Principal::plugin("com.example.demo", vec![data.clone()], Some(data.clone()));
    let target = data.join("a.txt").to_string_lossy().to_string();
    fs.write(
        &p,
        &ctx(),
        &WriteRequest {
            path: target.clone(),
            data: b"0123456789".to_vec(),
            mode: WriteMode::Replace,
        },
    )
    .unwrap();
    for _ in 0..5 {
        fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: target.clone(),
                offset: 0,
                max_bytes: 10,
            },
        )
        .unwrap();
    }
    fs.remove(&p, &ctx(), &target, false).unwrap();
    agg.flush();

    let rows = inner.events();
    let read_aggs: Vec<_> = rows.iter().filter(|e| e.action == "fs.read#agg").collect();
    assert_eq!(read_aggs.len(), 1, "同分钟同路径读应合并");
    assert_eq!(read_aggs[0].count, 5);
    assert_eq!(read_aggs[0].bytes, 50);
    assert!(
        rows.iter().any(|e| e.action == "fs.remove"),
        "删除必须逐条落库"
    );
}
