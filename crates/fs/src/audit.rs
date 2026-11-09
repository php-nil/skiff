//! 审计事件结构与分级策略（契约 §4）。
//!
//! 本 crate 只定义**结构**与**聚合器**，不落库：持久化由 trait [`AuditSink`]
//! 抽象，本 crate 提供内存实现与 `Noop` 实现；落库实现在 `skiff-app` 的
//! `DbAuditSink`（写 `audit` 表，`ip` 列记来源 `web`）。
//!
//! 铁律：`AuditEvent` **没有内容字段**，也不允许把文件正文塞进 `detail`；
//! `sha256` 只用于完整性核对（见 `docs/KERNEL_FS_IMPL_NOTES.md` 接线说明）。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// 单主体每的分钟聚合桶上限（契约 §4.2）。
pub const AGG_BUCKETS_PER_SUBJECT_MINUTE: usize = 1000;

/// 分钟桶宽度。
pub const AGG_WINDOW_SECS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    Read,
    Write,
    List,
    Mkdir,
    Remove,
    Rename,
    Copy,
    Stat,
    TrashList,
    TrashRestore,
    TrashPurge,
    TrashClear,
    QuotaScan,
    QuotaSet,
    QuotaCancel,
    Chmod,
    Chown,
    Archive,
    Extract,
    /// 聚合桶超限时的丢弃标记。
    SampledOut,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Read => "fs.read",
            AuditAction::Write => "fs.write",
            AuditAction::List => "fs.list",
            AuditAction::Mkdir => "fs.mkdir",
            AuditAction::Remove => "fs.remove",
            AuditAction::Rename => "fs.rename",
            AuditAction::Copy => "fs.copy",
            AuditAction::Stat => "fs.stat",
            AuditAction::TrashList => "fs.recycle.list",
            AuditAction::TrashRestore => "fs.recycle.restore",
            AuditAction::TrashPurge => "fs.recycle.purge",
            AuditAction::TrashClear => "fs.recycle.clear",
            AuditAction::QuotaScan => "fs.quota.scan",
            AuditAction::QuotaSet => "fs.quota.set",
            AuditAction::QuotaCancel => "fs.quota.cancel",
            AuditAction::Chmod => "fs.chmod",
            AuditAction::Chown => "fs.chown",
            AuditAction::Archive => "fs.archive",
            AuditAction::Extract => "fs.extract",
            AuditAction::SampledOut => "audit.sampled_out",
        }
    }

    /// 破坏性操作永不聚合（契约 §4.2）。
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            AuditAction::Remove
                | AuditAction::Rename
                | AuditAction::Copy
                | AuditAction::TrashRestore
                | AuditAction::TrashPurge
                | AuditAction::TrashClear
                | AuditAction::QuotaSet
                | AuditAction::QuotaCancel
                | AuditAction::Chmod
                | AuditAction::Chown
                | AuditAction::Archive
                | AuditAction::Extract
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Ok,
    Deny,
    Fail,
}

impl AuditResult {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditResult::Ok => "ok",
            AuditResult::Deny => "deny",
            AuditResult::Fail => "fail",
        }
    }
}

/// 一条审计记录。字段与契约 §4.1 一一对应；`count` 是聚合行的扩展字段
/// （逐条行为 1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEvent {
    pub ts: i64,
    pub actor_type: String,
    pub actor_id: String,
    pub action: String,
    pub src_path: Option<String>,
    pub dst_path: Option<String>,
    pub bytes: u64,
    pub result: String,
    pub code: Option<String>,
    pub sha256: Option<String>,
    pub site: Option<String>,
    pub protected: bool,
    pub request_id: Option<String>,
    pub detail: Option<String>,
    pub count: u64,
}

impl AuditEvent {
    /// 逐条事件的最小构造；其余字段用 `with_*` 链式补齐。
    pub fn new(
        actor_type: impl Into<String>,
        actor_id: impl Into<String>,
        action: AuditAction,
        result: AuditResult,
        ts: i64,
    ) -> Self {
        Self {
            ts,
            actor_type: actor_type.into(),
            actor_id: actor_id.into(),
            action: action.as_str().to_string(),
            src_path: None,
            dst_path: None,
            bytes: 0,
            result: result.as_str().to_string(),
            code: None,
            sha256: None,
            site: None,
            protected: false,
            request_id: None,
            detail: None,
            count: 1,
        }
    }

    pub fn src(mut self, path: impl Into<String>) -> Self {
        self.src_path = Some(path.into());
        self
    }

    pub fn dst(mut self, path: impl Into<String>) -> Self {
        self.dst_path = Some(path.into());
        self
    }

    pub fn bytes(mut self, bytes: u64) -> Self {
        self.bytes = bytes;
        self
    }

    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn sha256(mut self, hash: impl Into<String>) -> Self {
        self.sha256 = Some(hash.into());
        self
    }

    pub fn site(mut self, site: Option<String>) -> Self {
        self.site = site;
        self
    }

    pub fn protected(mut self, protected: bool) -> Self {
        self.protected = protected;
        self
    }

    pub fn request(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 审计落点。实现必须是线程安全的；写入失败**不阻断**文件操作（best-effort）。
pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);

    /// 把聚合缓冲落盘；逐条实现可保持默认空实现。
    fn flush(&self) {}
}

/// 不做任何事的落点（测试与「审计关闭」场景；生产不得使用）。
#[derive(Debug, Default)]
pub struct NoopAudit;

impl AuditSink for NoopAudit {
    fn record(&self, _event: AuditEvent) {}
}

/// 内存落点：测试与开发期用。
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl MemoryAuditSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl AuditSink for MemoryAuditSink {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }
}

/// 是否走聚合：破坏性操作、`deny/fail`、受保护路径写入一律逐条。
pub fn should_aggregate(event: &AuditEvent) -> bool {
    if event.result != AuditResult::Ok.as_str() || event.protected {
        return false;
    }
    matches!(
        event.action.as_str(),
        "fs.read" | "fs.list" | "fs.stat" | "fs.write"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    actor_type: String,
    actor_id: String,
    action: String,
    src_path: Option<String>,
    minute: i64,
}

struct Bucket {
    count: u64,
    sum_bytes: u64,
    first_ts: i64,
    last_ts: i64,
    site: Option<String>,
    request_id: Option<String>,
}

/// 聚合落点：逐条事件直通，高频读/写按 `(主体, 动作, 路径, 分钟)` 合并。
pub struct AggregatingSink {
    inner: Arc<dyn AuditSink>,
    state: Mutex<AggState>,
    cap: usize,
}

#[derive(Default)]
struct AggState {
    buckets: HashMap<BucketKey, Bucket>,
    subject_minute_buckets: HashMap<(String, String, i64), usize>,
    sampled_out: HashSet<(String, String, i64)>,
}

impl AggregatingSink {
    pub fn new(inner: Arc<dyn AuditSink>) -> Self {
        Self::with_cap(inner, AGG_BUCKETS_PER_SUBJECT_MINUTE)
    }

    pub fn with_cap(inner: Arc<dyn AuditSink>, cap: usize) -> Self {
        Self {
            inner,
            state: Mutex::new(AggState::default()),
            cap,
        }
    }

    /// 把缓冲里的聚合行交给下游。
    pub fn flush(&self) {
        let rows = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let rows = st.flush_rows();
            st.subject_minute_buckets.clear();
            st.sampled_out.clear();
            rows
        };
        for row in rows {
            self.inner.record(row);
        }
    }
}

impl AuditSink for AggregatingSink {
    fn record(&self, event: AuditEvent) {
        if !should_aggregate(&event) {
            self.inner.record(event);
            return;
        }
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let minute = event.ts / AGG_WINDOW_SECS;
        let key = BucketKey {
            actor_type: event.actor_type.clone(),
            actor_id: event.actor_id.clone(),
            action: event.action.clone(),
            src_path: event.src_path.clone(),
            minute,
        };
        let sm = (event.actor_type.clone(), event.actor_id.clone(), minute);
        if !st.buckets.contains_key(&key) {
            let used = st.subject_minute_buckets.get(&sm).copied().unwrap_or(0);
            if used >= self.cap {
                if st.sampled_out.insert(sm) {
                    let mut out = event;
                    out.action = AuditAction::SampledOut.as_str().to_string();
                    out.result = AuditResult::Fail.as_str().to_string();
                    out.detail = Some("分钟聚合桶超限，本条被丢弃".to_string());
                    out.count = 1;
                    self.inner.record(out);
                }
                return;
            }
            *st.subject_minute_buckets.entry(sm).or_insert(0) += 1;
            st.buckets.insert(
                key.clone(),
                Bucket {
                    count: 1,
                    sum_bytes: event.bytes,
                    first_ts: event.ts,
                    last_ts: event.ts,
                    site: event.site.clone(),
                    request_id: event.request_id.clone(),
                },
            );
            return;
        }
        if let Some(b) = st.buckets.get_mut(&key) {
            b.count += 1;
            b.sum_bytes = b.sum_bytes.saturating_add(event.bytes);
            b.last_ts = event.ts;
            b.request_id = event.request_id.clone();
        }
    }

    fn flush(&self) {
        AggregatingSink::flush(self);
    }
}

impl AggState {
    fn flush_rows(&mut self) -> Vec<AuditEvent> {
        let mut out = Vec::with_capacity(self.buckets.len());
        for (key, b) in self.buckets.drain() {
            let mut ev = AuditEvent::new(
                key.actor_type,
                key.actor_id,
                AuditAction::Read,
                AuditResult::Ok,
                b.last_ts,
            );
            ev.action = format!("{}#agg", key.action);
            ev.src_path = key.src_path;
            ev.bytes = b.sum_bytes;
            ev.site = b.site;
            ev.request_id = b.request_id;
            ev.count = b.count;
            ev.detail = Some(format!(
                "first_ts={}, last_ts={}, window={}s",
                b.first_ts, b.last_ts, AGG_WINDOW_SECS
            ));
            out.push(ev);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(action: AuditAction, ts: i64, bytes: u64) -> AuditEvent {
        AuditEvent::new("plugin", "com.example.demo", action, AuditResult::Ok, ts)
            .src("/www/sites/a/index/x.txt")
            .bytes(bytes)
    }

    #[test]
    fn destructive_and_denied_events_are_never_aggregated() {
        assert!(!should_aggregate(&ev(AuditAction::Remove, 100, 1)));
        assert!(!should_aggregate(&ev(AuditAction::Copy, 100, 1)));
        let mut denied = ev(AuditAction::Read, 100, 1);
        denied.result = AuditResult::Deny.as_str().to_string();
        assert!(!should_aggregate(&denied));
        let mut prot = ev(AuditAction::Write, 100, 1);
        prot.protected = true;
        assert!(!should_aggregate(&prot));
        assert!(should_aggregate(&ev(AuditAction::Read, 100, 1)));
    }

    #[test]
    fn same_minute_reads_merge_into_one_row() {
        let inner = Arc::new(MemoryAuditSink::new());
        let sink = AggregatingSink::new(inner.clone());
        sink.record(ev(AuditAction::Read, 100, 10));
        sink.record(ev(AuditAction::Read, 110, 20));
        sink.record(ev(AuditAction::Read, 200, 30)); // 下一个分钟桶
        assert!(inner.events().is_empty(), "未 flush 前不应落逐条");
        sink.flush();
        let rows = inner.events();
        assert_eq!(rows.len(), 2);
        let merged = rows.iter().find(|r| r.count == 2).unwrap();
        assert_eq!(merged.bytes, 30, "同一分钟两条合并为 10+20");
        let single = rows.iter().find(|r| r.count == 1).unwrap();
        assert_eq!(single.bytes, 30, "下一分钟桶保持单条");
        assert!(rows.iter().all(|r| r.action == "fs.read#agg"));
    }

    #[test]
    fn bucket_cap_drops_and_emits_sampled_out_once() {
        let inner = Arc::new(MemoryAuditSink::new());
        let sink = AggregatingSink::with_cap(inner.clone(), 2);
        for i in 0..4 {
            sink.record(ev(AuditAction::Read, 100, 1).src(format!("/p{i}")));
        }
        sink.flush();
        let rows = inner.events();
        let sampled: Vec<_> = rows
            .iter()
            .filter(|r| r.action == "audit.sampled_out")
            .collect();
        assert_eq!(sampled.len(), 1, "每分钟只记一次丢弃标记");
        assert_eq!(rows.iter().filter(|r| r.action == "fs.read#agg").count(), 2);
    }

    #[test]
    fn audit_row_has_no_content_field() {
        let secret = "TOP-SECRET-CONTENT";
        let row = ev(AuditAction::Write, 100, 5)
            .sha256(crate::hash::sha256_hex(secret.as_bytes()))
            .detail("atomic=true");
        let json = serde_json::to_string(&row).unwrap();
        assert!(!json.contains(secret));
        assert!(json.contains("sha256"));
    }
}
