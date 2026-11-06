//! 集成测试公共夹具：临时目录 + 内存审计/回收站/配额。

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use skiff_fs::{
    Fs, FsConfig, MemoryAuditSink, MemoryQuotaTracker, MemoryTrashStore, Principal, TrashConfig,
};

pub struct Harness {
    /// 保持临时目录生命周期；对外路径一律从 `base`（canonical）派生。
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub fs: Arc<Fs>,
    pub audit: Arc<MemoryAuditSink>,
    pub trash: Arc<MemoryTrashStore>,
    pub quota: Arc<MemoryQuotaTracker>,
}

impl Harness {
    pub fn new() -> Self {
        Self::with_config(|cfg| {
            let _ = cfg;
        })
    }

    /// 允许测试按需收窄配额/上限。
    pub fn with_config(f: impl FnOnce(&mut FsConfig)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = canonical_base(tmp.path());
        let data = base.join("data");
        let sites = base.join("sites");
        let trash_dir = base.join("trash");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&sites).unwrap();
        std::fs::create_dir_all(&trash_dir).unwrap();

        let mut cfg = FsConfig {
            site_root: sites.to_string_lossy().to_string(),
            trash: TrashConfig {
                root: trash_dir,
                retention_secs: 30 * 86_400,
                per_root_bytes: 1024 * 1024 * 1024,
                total_bytes: 10 * 1024 * 1024 * 1024,
                per_entry_bytes: 2 * 1024 * 1024 * 1024,
                max_entries: 10_000,
            },
            ..FsConfig::default()
        };
        f(&mut cfg);

        let audit = Arc::new(MemoryAuditSink::new());
        let trash = Arc::new(MemoryTrashStore::new());
        let quota = Arc::new(MemoryQuotaTracker::new());
        let fs = Fs::new(cfg, audit.clone(), trash.clone(), quota.clone());
        Self {
            _tmp: tmp,
            base,
            fs: Arc::new(fs),
            audit,
            trash,
            quota,
        }
    }

    pub fn base(&self) -> PathBuf {
        self.base.clone()
    }

    pub fn data(&self) -> PathBuf {
        self.base.join("data")
    }

    pub fn sites(&self) -> PathBuf {
        self.base.join("sites")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.base.join("trash")
    }

    /// 插件主体：默认只授权 `data/`。
    pub fn plugin(&self, id: &str) -> Principal {
        Principal::plugin(id, vec![self.data()], Some(self.data()))
    }

    /// 插件主体：额外授权站点根（用于受保护路径测试）。
    pub fn plugin_with_sites(&self, id: &str) -> Principal {
        Principal::plugin(id, vec![self.data(), self.sites()], Some(self.data()))
    }

    pub fn admin(&self) -> Principal {
        Principal::admin("root")
    }

    /// 把 `data/` 下的相对路径转成绝对字符串。
    pub fn data_path(&self, rel: &str) -> String {
        self.data().join(rel).to_string_lossy().to_string()
    }

    pub fn site_path(&self, rel: &str) -> String {
        self.sites().join(rel).to_string_lossy().to_string()
    }

    /// 写一个文件（绕过服务，用于准备夹具）。
    pub fn seed(&self, abs: &str, content: &[u8]) {
        let path = PathBuf::from(abs);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    pub fn audit_events(&self) -> Vec<skiff_fs::AuditEvent> {
        self.audit.events()
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

pub fn ctx() -> skiff_fs::OpContext {
    skiff_fs::OpContext::at("req-test-1", 1_000)
}

/// 授权根在 `Principal` 里走 `canonicalize`，测试原始路径必须与它同一坐标系。
///
/// Windows 上 `canonicalize` 会做两件测试路径没做的事：展开 8.3 短名
/// （CI 的 `TEMP` 是 `C:\Users\RUNNER~1\...`）并返回 `\\?\` verbatim 前缀；
/// 直接拿 `TempDir::path()` 拼原始路径，`path.starts_with(root)` 会判成越界，
/// 与产品逻辑无关。这里统一成 `simplify_windows` 后的 canonical 形态。
pub fn canonical_base(p: &std::path::Path) -> PathBuf {
    let canon = std::fs::canonicalize(p).unwrap();
    #[cfg(windows)]
    {
        let s = canon.to_string_lossy().to_string();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    canon
}
