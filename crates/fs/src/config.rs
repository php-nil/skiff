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

/// 授权根用量计数器（写前预检 + 写后复核）。
///
/// 真实实现由 `skiff-store` 提供（异步刷新，≤60s 误差）；本 crate 只提供
/// 内存实现供测试与独立运行。`adjust` 的 `delta` 为有符号字节数。
pub trait QuotaTracker: Send + Sync {
    fn usage(&self, root: &std::path::Path) -> u64;
    fn adjust(&self, root: &std::path::Path, delta: i64);
}

/// 内存计数：进程生命周期内准确，重启即清零（设计上由真实实现替代）。
#[derive(Debug, Default)]
pub struct MemoryQuotaTracker {
    usage: std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>,
}

impl MemoryQuotaTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 以当前磁盘实际占用初始化某个根（`walk` 的简化版本：只统计普通文件）。
    pub fn seed_from_disk(&self, root: &std::path::Path) -> std::io::Result<u64> {
        let mut total = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let meta = entry.metadata()?;
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    total += meta.len();
                }
            }
        }
        *self
            .usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(root.to_path_buf())
            .or_insert(0) = total;
        Ok(total)
    }
}

impl QuotaTracker for MemoryQuotaTracker {
    fn usage(&self, root: &std::path::Path) -> u64 {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(root)
            .copied()
            .unwrap_or(0)
    }

    fn adjust(&self, root: &std::path::Path, delta: i64) {
        let mut map = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let slot = map.entry(root.to_path_buf()).or_insert(0);
        let next = slot.saturating_add_signed(delta);
        *slot = next;
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
