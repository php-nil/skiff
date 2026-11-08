//! 容量与策略默认值（契约 §2.3、§3、§5.2、§7.1）。
//!
//! 所有数值集中在这里，便于后续配置化；数值沿用契约默认值。未实测的数字
//! 只作为**设计目标**，不当作已验证事实（见 `AGENTS.md` 数字纪律）。

use std::path::PathBuf;
use std::time::Duration;

use crate::lock::DEFAULT_LOCK_TIMEOUT;

/// 单次请求上限：256 KiB（P1 沿用现有 UDS 分帧）。
pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
/// 单文件上限：4 GiB。
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 单授权根配额：10 GiB。
pub const MAX_ROOT_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// 目录条目上限。
pub const MAX_DIR_ENTRIES: usize = 1_000_000;
/// 目录复制条目上限（契约 §3.1：>1000 项走异步任务，P1 直接拒绝）。
pub const COPY_MAX_ENTRIES: usize = 1_000;
/// 目录复制深度上限。
pub const COPY_MAX_DEPTH: usize = 32;

#[derive(Debug, Clone)]
pub struct TrashConfig {
    /// 回收站根目录。契约写作 `data/trash/`；实际数据根由接线方传入。
    pub root: PathBuf,
    /// 保留期：30 天。
    pub retention_secs: i64,
    /// 单授权根回收站配额：1 GiB（软配额，超限淘汰最旧）。
    pub per_root_bytes: u64,
    /// 全局回收站配额：10 GiB（硬配额，超限拒绝新删除）。
    pub total_bytes: u64,
    /// 单条目上限：2 GiB；超限直接拒绝并提示 `permanent`。
    pub per_entry_bytes: u64,
    /// 保留条目数上限。
    pub max_entries: usize,
}

impl Default for TrashConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from("/www/skiff/trash"),
            retention_secs: 30 * 24 * 60 * 60,
            per_root_bytes: 1024 * 1024 * 1024,
            total_bytes: 10 * 1024 * 1024 * 1024,
            per_entry_bytes: 2 * 1024 * 1024 * 1024,
            max_entries: 10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FsConfig {
    /// 站点根（用于 `site` 字段与受保护路径判定）。
    pub site_root: String,
    /// 单次读/写字节上限。
    pub max_request_bytes: usize,
    /// 单文件上限。
    pub max_file_bytes: u64,
    /// 插件授权根配额。
    pub max_root_bytes: u64,
    /// 目录条目上限（`list` 与递归复制）。
    pub max_dir_entries: usize,
    /// 目录复制条目上限。
    pub copy_max_entries: usize,
    /// 递归复制深度上限。
    pub copy_max_depth: usize,
    /// 路径锁等待上限。
    pub lock_timeout: Duration,
    pub trash: TrashConfig,
}

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            site_root: crate::path::DEFAULT_SITE_ROOT.to_string(),
            max_request_bytes: MAX_REQUEST_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
            max_root_bytes: MAX_ROOT_BYTES,
            max_dir_entries: MAX_DIR_ENTRIES,
            copy_max_entries: COPY_MAX_ENTRIES,
            copy_max_depth: COPY_MAX_DEPTH,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            trash: TrashConfig::default(),
        }
    }
}

/// 一条目录树配额规则。`root` 是命中该规则的最长祖先根；两个上限都可为 `None`
/// （单独统计用）。同一条目树被多条规则覆盖时，最长根优先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaLimit {
    pub root: PathBuf,
    pub max_bytes: Option<u64>,
    pub max_files: Option<u64>,
}

/// 目录树用量：字节数 = 普通文件大小之和；文件数 = 普通文件个数（目录/链接不计）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct QuotaUsage {
    pub used_bytes: u64,
    pub used_files: u64,
}

/// 配额列表视图：`updated_at` 是最近一次全量统计时间（`None` 表示仅增量计数）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QuotaSnapshot {
    pub root: String,
    pub max_bytes: Option<u64>,
    pub max_files: Option<u64>,
    pub used_bytes: u64,
    pub used_files: u64,
    pub updated_at: Option<i64>,
}

/// 授权根/目录树用量计数器（写前预检 + 写后复核）。
///
/// 真实实现由 `skiff-app` 的 SQLite 适配器提供；本 crate 只提供内存实现供测试
/// 与独立运行。`adjust*` 的 `delta` 为有符号值，不得下溢。
///
/// 新增的按树配额方法与持久化方法都带默认实现：只关心字节数的旧实现不受影响。
pub trait QuotaTracker: Send + Sync {
    fn usage(&self, root: &std::path::Path) -> u64;
    fn adjust(&self, root: &std::path::Path, delta: i64);

    fn usage_stats(&self, root: &std::path::Path) -> QuotaUsage {
        QuotaUsage {
            used_bytes: self.usage(root),
            used_files: 0,
        }
    }

    fn adjust_stats(&self, root: &std::path::Path, delta_bytes: i64, delta_files: i64) {
        let _ = delta_files;
        self.adjust(root, delta_bytes);
    }

    /// 命中 `path` 的配额规则（最长祖先根优先）。
    fn limit_for(&self, _path: &std::path::Path) -> Option<QuotaLimit> {
        None
    }

    /// 全量扫描后重写某个根的用量。
    fn set_usage(&self, _root: &std::path::Path, _usage: QuotaUsage) {}

    /// 设置/修改上限，保留已用量。
    fn set_limits(
        &self,
        _root: &std::path::Path,
        _max_bytes: Option<u64>,
        _max_files: Option<u64>,
    ) {
    }

    /// 取消上限（用量计数保留，页面继续显示「未设置上限」）。
    fn clear_limits(&self, _root: &std::path::Path) {}

    /// 配额页列表。
    fn snapshot(&self) -> Vec<QuotaSnapshot> {
        Vec::new()
    }
}

/// 内存计数：进程生命周期内准确，重启即清零（设计上由真实实现替代）。
#[derive(Debug, Default)]
pub struct MemoryQuotaTracker {
    usage: std::sync::Mutex<std::collections::HashMap<PathBuf, QuotaUsage>>,
    limits: std::sync::Mutex<std::collections::HashMap<PathBuf, QuotaLimit>>,
}

impl MemoryQuotaTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 以当前磁盘实际占用初始化某个根（只统计普通文件，不跟随符号链接）。
    pub fn seed_from_disk(&self, root: &std::path::Path) -> std::io::Result<u64> {
        let mut usage = QuotaUsage::default();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                // `DirEntry::metadata()` 会跟随链接；这里必须用 `symlink_metadata`。
                let meta = std::fs::symlink_metadata(entry.path())?;
                let ft = meta.file_type();
                if ft.is_dir() {
                    stack.push(entry.path());
                } else if ft.is_file() {
                    usage.used_bytes = usage.used_bytes.saturating_add(meta.len());
                    usage.used_files = usage.used_files.saturating_add(1);
                }
            }
        }
        self.set_usage(root, usage);
        Ok(usage.used_bytes)
    }

    pub fn set_limit(
        &self,
        root: &std::path::Path,
        max_bytes: Option<u64>,
        max_files: Option<u64>,
    ) {
        self.set_limits(root, max_bytes, max_files);
    }
}

impl QuotaTracker for MemoryQuotaTracker {
    fn usage(&self, root: &std::path::Path) -> u64 {
        self.usage_stats(root).used_bytes
    }

    fn adjust(&self, root: &std::path::Path, delta: i64) {
        self.adjust_stats(root, delta, 0);
    }

    fn usage_stats(&self, root: &std::path::Path) -> QuotaUsage {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(root)
            .copied()
            .unwrap_or_default()
    }

    fn adjust_stats(&self, root: &std::path::Path, delta_bytes: i64, delta_files: i64) {
        let mut map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let slot = map.entry(root.to_path_buf()).or_default();
        slot.used_bytes = slot.used_bytes.saturating_add_signed(delta_bytes);
        slot.used_files = slot.used_files.saturating_add_signed(delta_files);
    }

    fn limit_for(&self, path: &std::path::Path) -> Option<QuotaLimit> {
        let limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        limits
            .values()
            .filter(|l| path.starts_with(&l.root))
            .max_by_key(|l| l.root.as_os_str().len())
            .cloned()
    }

    fn set_usage(&self, root: &std::path::Path, usage: QuotaUsage) {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(root.to_path_buf(), usage);
    }

    fn set_limits(&self, root: &std::path::Path, max_bytes: Option<u64>, max_files: Option<u64>) {
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                root.to_path_buf(),
                QuotaLimit {
                    root: root.to_path_buf(),
                    max_bytes,
                    max_files,
                },
            );
    }

    fn clear_limits(&self, root: &std::path::Path) {
        self.limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(root);
    }

    fn snapshot(&self) -> Vec<QuotaSnapshot> {
        let usage = self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let limits = self
            .limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut roots: Vec<PathBuf> = usage.keys().cloned().collect();
        for root in limits.keys() {
            if !usage.contains_key(root) {
                roots.push(root.clone());
            }
        }
        roots.sort_by_key(|r| r.as_os_str().len());
        roots
            .into_iter()
            .map(|root| {
                let limit = limits.get(&root);
                let used = usage.get(&root).copied().unwrap_or_default();
                QuotaSnapshot {
                    root: root.display().to_string(),
                    max_bytes: limit.and_then(|l| l.max_bytes),
                    max_files: limit.and_then(|l| l.max_files),
                    used_bytes: used.used_bytes,
                    used_files: used.used_files,
                    updated_at: None,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_tracker_handles_signed_deltas() {
        let t = MemoryQuotaTracker::new();
        let root = std::path::Path::new("/tmp/x");
        t.adjust(root, 100);
        assert_eq!(t.usage(root), 100);
        t.adjust(root, -60);
        assert_eq!(t.usage(root), 40);
        t.adjust(root, -1000);
        assert_eq!(t.usage(root), 0, "不得下溢");
    }
}
