//! 回收站模型（契约 §5）：条目元数据、存储 trait、保留期与淘汰策略。
//!
//! 本模块只定义**结构与策略**，不直接移动文件：实际 `rename`/跨盘复制由
//! `ops` 执行（需要路径锁）。持久化由 [`TrashStore`] 抽象，P1 提供内存实现。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TrashKind {
    File,
    Dir,
    Symlink,
}

impl TrashKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TrashKind::File => "file",
            TrashKind::Dir => "dir",
            TrashKind::Symlink => "symlink",
        }
    }
}

/// 一条回收站条目。字段与契约 §5.1 对齐；`quota_root` 是内部扩展，用于按
/// 授权根统计容量（契约要求「每个授权根 1 GiB」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrashEntry {
    pub id: String,
    pub actor_type: String,
    pub actor_id: String,
    pub original_path: String,
    pub trash_path: String,
    pub kind: TrashKind,
    pub size: u64,
    /// 普通文件个数；恢复时按此回填配额文件数计数。
    pub files: u64,
    pub mode: Option<u32>,
    /// `uid:gid`；非 Unix 平台为 `None`。
    pub owner: Option<String>,
    pub sha256: Option<String>,
    pub site: Option<String>,
    pub protected: bool,
    pub deleted_at: i64,
    pub expires_at: i64,
    pub nlink: u64,
    pub restored_at: Option<i64>,
    pub purged_at: Option<i64>,
    pub quota_root: String,
}

impl TrashEntry {
    pub fn is_live(&self) -> bool {
        self.restored_at.is_none() && self.purged_at.is_none()
    }
}

/// 回收站元数据存储。写入/恢复/彻底删除的状态流转都经由此 trait；
/// 持久化实现在 `skiff-store` 的 `db::trash`（`trash_entries` 表），
/// 由 `skiff-app` 的 `DbTrashStore` 注入。
pub trait TrashStore: Send + Sync {
    fn insert(&self, entry: TrashEntry);
    fn get(&self, id: &str) -> Option<TrashEntry>;
    /// 移除并返回条目（彻底删除成功后调用）。
    fn remove(&self, id: &str) -> Option<TrashEntry>;
    fn mark_restored(&self, id: &str, ts: i64) -> Option<TrashEntry>;
    fn mark_purged(&self, id: &str, ts: i64) -> Option<TrashEntry>;
    fn list(&self) -> Vec<TrashEntry>;

    fn total_usage(&self) -> u64 {
        self.list()
            .iter()
            .filter(|e| e.is_live())
            .map(|e| e.size)
            .sum()
    }

    fn usage_for_root(&self, quota_root: &str) -> u64 {
        self.list()
            .iter()
            .filter(|e| e.is_live() && e.quota_root == quota_root)
            .map(|e| e.size)
            .sum()
    }

    fn expired(&self, now: i64) -> Vec<TrashEntry> {
        let mut rows: Vec<TrashEntry> = self
            .list()
            .into_iter()
            .filter(|e| e.is_live() && e.expires_at <= now)
            .collect();
        rows.sort_by(|a, b| a.deleted_at.cmp(&b.deleted_at).then(a.id.cmp(&b.id)));
        rows
    }
}

/// 内存实现：测试与开发期用，重启即丢（真实实现由 SQLite 提供）。
#[derive(Debug, Default)]
pub struct MemoryTrashStore {
    rows: Mutex<Vec<TrashEntry>>,
}

impl MemoryTrashStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TrashStore for MemoryTrashStore {
    fn insert(&self, entry: TrashEntry) {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry);
    }

    fn get(&self, id: &str) -> Option<TrashEntry> {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|e| e.id == id)
            .cloned()
    }

    fn remove(&self, id: &str) -> Option<TrashEntry> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        rows.iter().position(|e| e.id == id).map(|i| rows.remove(i))
    }

    fn mark_restored(&self, id: &str, ts: i64) -> Option<TrashEntry> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.iter_mut().find(|e| e.id == id)?;
        row.restored_at = Some(ts);
        Some(row.clone())
    }

    fn mark_purged(&self, id: &str, ts: i64) -> Option<TrashEntry> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.iter_mut().find(|e| e.id == id)?;
        row.purged_at = Some(ts);
        Some(row.clone())
    }

    fn list(&self) -> Vec<TrashEntry> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner()).clone();
        rows.sort_by(|a, b| a.deleted_at.cmp(&b.deleted_at).then(a.id.cmp(&b.id)));
        rows
    }
}

/// 生成回收站 id：`trash-<秒级时间戳>-<进程内序号>`，字典序随删除时间单调。
pub fn new_trash_id(now: i64) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("trash-{now}-{seq:06}")
}

/// 回收站内实际落点：`<root>/<yyyy>/<mm>/<id>`。
pub fn trash_path(root: &std::path::Path, id: &str, deleted_at: i64) -> std::path::PathBuf {
    let (year, month, _) = civil_from_days(deleted_at.div_euclid(86_400));
    root.join(format!("{year:04}"))
        .join(format!("{month:02}"))
        .join(id)
}

/// Unix 秒 → `(year, month, day)`，不引入 chrono。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// 选择要**立即彻底删除**的旧条目，为新删除腾出空间。
///
/// 规则（契约 §5.2）：
/// - 单授权根软配额：超过后淘汰该根最旧条目；
/// - 条目数上限：淘汰全局最旧条目。
///
/// 返回按淘汰顺序排列的 id；调用方负责先 purge 再放入新条目。
pub fn select_evictions(
    entries: &[TrashEntry],
    quota_root: &str,
    need: u64,
    per_root_bytes: u64,
    max_entries: usize,
) -> Vec<String> {
    let mut live: Vec<&TrashEntry> = entries.iter().filter(|e| e.is_live()).collect();
    live.sort_by(|a, b| a.deleted_at.cmp(&b.deleted_at).then(a.id.cmp(&b.id)));
    let mut out = Vec::new();

    let mut root_used: u64 = live
        .iter()
        .filter(|e| e.quota_root == quota_root)
        .map(|e| e.size)
        .sum();
    for e in live.iter().filter(|e| e.quota_root == quota_root) {
        if root_used.saturating_add(need) <= per_root_bytes {
            break;
        }
        out.push(e.id.clone());
        root_used = root_used.saturating_sub(e.size);
    }
    let kept: Vec<&TrashEntry> = live
        .iter()
        .copied()
        .filter(|e| !out.contains(&e.id))
        .collect();
    let mut count = kept.len();
    let mut idx = 0usize;
    while count + 1 > max_entries && idx < kept.len() {
        let id = kept[idx].id.clone();
        if !out.contains(&id) {
            out.push(id);
            count -= 1;
        }
        idx += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, root: &str, size: u64, deleted_at: i64) -> TrashEntry {
        TrashEntry {
            id: id.to_string(),
            actor_type: "plugin".into(),
            actor_id: "p".into(),
            original_path: format!("/x/{id}"),
            trash_path: format!("/trash/{id}"),
            kind: TrashKind::File,
            size,
            files: 1,
            mode: Some(0o644),
            owner: Some("0:0".into()),
            sha256: None,
            site: None,
            protected: false,
            deleted_at,
            expires_at: deleted_at + 30 * 86_400,
            nlink: 1,
            restored_at: None,
            purged_at: None,
            quota_root: root.to_string(),
        }
    }

    #[test]
    fn path_layout_uses_year_and_month() {
        let ts = 1_760_000_000; // 2025-10-09 UTC
        let p = trash_path(std::path::Path::new("/trash"), "trash-1", ts);
        let s = p.to_string_lossy();
        assert!(s.contains("2025"), "{s}");
        assert!(s.contains("10"), "{s}");
    }

    #[test]
    fn per_root_quota_evicts_oldest_first() {
        let rows = vec![
            entry("a", "/root", 40, 100),
            entry("b", "/root", 40, 200),
            entry("c", "/other", 40, 50),
        ];
        let evict = select_evictions(&rows, "/root", 30, 100, usize::MAX);
        assert_eq!(evict, vec!["a".to_string()], "先淘汰最旧，c 不在同一根");
    }

    #[test]
    fn max_entries_evicts_global_oldest() {
        let rows = vec![entry("old", "/a", 1, 10), entry("new", "/b", 1, 20)];
        let evict = select_evictions(&rows, "/c", 1, u64::MAX, 2);
        assert_eq!(evict, vec!["old".to_string()]);
    }

    #[test]
    fn expired_rows_are_oldest_first() {
        let store = MemoryTrashStore::new();
        store.insert(entry("a", "/r", 1, 100));
        store.insert(entry("b", "/r", 1, 200));
        let expired = store.expired(200 + 30 * 86_400);
        assert_eq!(expired.len(), 2);
        assert_eq!(expired[0].id, "a");
        assert_eq!(store.total_usage(), 2);
    }

    #[test]
    fn restored_rows_leave_usage() {
        let store = MemoryTrashStore::new();
        store.insert(entry("a", "/r", 10, 100));
        assert_eq!(store.usage_for_root("/r"), 10);
        store.mark_restored("a", 200);
        assert_eq!(store.usage_for_root("/r"), 0);
        assert_eq!(store.total_usage(), 0);
    }
}
