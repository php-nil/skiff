//! 商店目录客户端（Skiff-server P1：只读接入，不做安装）。
//!
//! 职责只有四件事：按渠道拉取 `channels/<name>/{manifest.json,manifest.json.sig,index.json}`、
//! 用固定 Ed25519 公钥验签、做 `seq` 防回滚与摘要校验、把最近一次成功快照落本地缓存。
//! 任何一步失败都**不使用**新清单：页面继续展示上次成功快照并标注状态。
//!
//! 本模块不碰现有插件安装/信任链路：它只产出展示用快照，不写业务表；唯一写库的是
//! `catalog.refresh` 失败审计（写失败只记日志，不改变拒绝语义）。

pub mod fetch;
pub mod model;
pub mod store;
pub mod verify;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::SecondsFormat;
use sha2::{Digest, Sha256};
use skiff_plugin::install::version_gt;
use skiff_store::Db;
use tracing::{error, info, warn};

use fetch::{Conditional, SharedTransport, UreqTransport};
use model::{Index, Manifest, PluginEntry, SignatureFile, UpgradeEntry, BLOB_PREFIX};

/// 单次 HTTP 请求的默认硬超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// JSON 类资源（`manifest.json` / `.sig` / `index.json`）的本地缓存有效期：一天。
/// `blobs/sha256/*` 内容寻址，视为永久；「刷新」按钮显式绕过。
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 拉取失败（离线 / 校验失败）后的重试间隔：5 分钟，失败不顶满一天。
pub const FAILURE_TTL: Duration = Duration::from_secs(5 * 60);
/// 镜像连续失败后的退避基数：10s → 20s → 40s …（只影响自动刷新，手动刷新绕过）。
const MIRROR_BACKOFF_BASE: Duration = Duration::from_secs(10);
/// 单个镜像的退避上限：到期后自动刷新会再给它一次机会。
const MIRROR_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// 目录接入的运行期配置（由 `Config` 的 `[catalog]` 段派生）。
#[derive(Debug, Clone)]
pub struct CatalogSettings {
    /// 默认 `false`：不拉取、不显示商店入口。
    pub enabled: bool,
    pub channel: String,
    /// 镜像基地址；按序回退，末尾斜杠会被忽略。
    pub mirrors: Vec<String>,
    /// 固定 Ed25519 公钥（64 位十六进制）；缺省回落到内置常量。
    pub public_key: Option<String>,
    /// 仅开发模式允许：接受未签名渠道（启动期校验由 core 配置层负责）。
    pub allow_unsigned: bool,
    pub timeout: Duration,
    /// 额外信任的 CA 根（来自 `[catalog].ca_bundle`）。空 = 只用内置根。
    /// 只追加信任锚：仍是 https、仍做完整证书链与主机名校验。
    pub extra_roots: Vec<ureq::rustls::pki_types::CertificateDer<'static>>,
}

impl Default for CatalogSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: "stable".into(),
            mirrors: Vec::new(),
            public_key: None,
            allow_unsigned: false,
            timeout: DEFAULT_TIMEOUT,
            extra_roots: Vec::new(),
        }
    }
}

impl CatalogSettings {
    pub fn public_key_hex(&self) -> Option<&str> {
        self.public_key.as_deref().or(verify::BUILTIN_PUBLIC_KEY)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogState {
    /// 配置未开启，页面不出现。
    Disabled,
    /// 本次成功拉取并校验通过。
    Fresh,
    /// 条件请求 304 或未配置镜像，展示缓存。
    Cached,
    /// 网络/HTTP 失败，展示缓存（无缓存则空态）。
    Offline,
    /// 验签、防回滚、摘要或结构校验失败：拒绝新清单，保留旧快照。
    Rejected,
}

impl CatalogState {
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Rejected)
    }

    pub fn is_stale(self) -> bool {
        matches!(self, Self::Cached | Self::Offline | Self::Rejected)
    }
}

/// 页面展示的一行（四类条目共用一套列）。
#[derive(Debug, Clone, Default)]
pub struct CatalogItem {
    pub name: String,
    pub version: String,
    /// 来源：渠道名、镜像引用或镜像地址。
    pub source: String,
    pub updated_at: String,
    pub compatibility: String,
    pub note: String,
    /// 非空时按警示展示（已撤回 / 已 EOL 等）。
    pub warn: String,
    /// 该行是否为当前可安装版本（商店页选择器与安装 POST 同源）。
    pub installable: bool,
    /// 按钮文案：安装 / 升级。
    pub action: String,
    /// 不可安装或推荐版本说明（按钮旁展示）。
    pub install_note: String,
}

#[derive(Debug, Clone, Default)]
pub struct CatalogEntries {
    pub plugins: Vec<CatalogItem>,
    pub apps: Vec<CatalogItem>,
    pub runtimes: Vec<CatalogItem>,
    pub upgrades: Vec<CatalogItem>,
}

impl CatalogEntries {
    pub fn total(&self) -> usize {
        self.plugins.len() + self.apps.len() + self.runtimes.len() + self.upgrades.len()
    }
}

/// 一次只读快照：页面渲染只依赖它，不直接接触 HTTP/缓存细节。
#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub state: CatalogState,
    pub channel: String,
    pub seq: Option<u64>,
    /// 上次成功拉取时间（UTC RFC3339）。
    pub fetched_at: String,
    /// 实际命中的镜像基地址。
    pub source: String,
    pub message: String,
    pub entries: CatalogEntries,
    /// 是否允许未签名渠道（页面顶部需给出警示）。
    pub allow_unsigned: bool,
    /// 是否配置了固定公钥。
    pub key_configured: bool,
}

impl CatalogSnapshot {
    fn empty(settings: &CatalogSettings, state: CatalogState, message: String) -> Self {
        Self {
            state,
            channel: settings.channel.clone(),
            seq: None,
            fetched_at: String::new(),
            source: String::new(),
            message,
            entries: CatalogEntries::default(),
            allow_unsigned: settings.allow_unsigned,
            key_configured: settings.public_key_hex().is_some(),
        }
    }
}

/// 确认性问题（验签/防回滚/摘要/结构）——不再尝试下一个镜像，直接拒绝。
struct Rejected {
    kind: RejectKind,
    detail: String,
}

impl Rejected {
    fn signature(detail: impl Into<String>) -> Self {
        Self {
            kind: RejectKind::Signature,
            detail: detail.into(),
        }
    }

    fn seq(detail: impl Into<String>) -> Self {
        Self {
            kind: RejectKind::Seq,
            detail: detail.into(),
        }
    }

    fn digest(detail: impl Into<String>) -> Self {
        Self {
            kind: RejectKind::Digest,
            detail: detail.into(),
        }
    }
}

/// 拒绝原因分类：审计里落 `reason=<code>`，页面仍用 `detail` 原文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejectKind {
    /// 验签失败、未签名、缺公钥或签名文件损坏。
    Signature,
    /// `seq` 小于已见值（防回滚）。
    Seq,
    /// 清单/索引结构不符、大小或 sha256 不一致。
    Digest,
}

impl RejectKind {
    fn code(self) -> &'static str {
        match self {
            Self::Signature => "signature",
            Self::Seq => "seq",
            Self::Digest => "digest",
        }
    }
}

/// 该镜像不可用（网络、HTTP 状态、解析）——按序回退下一个镜像。
struct Unavailable(String);

enum Failure {
    Rejected(Rejected),
    Unavailable(Unavailable),
}

impl From<Rejected> for Failure {
    fn from(value: Rejected) -> Self {
        Self::Rejected(value)
    }
}

impl From<Unavailable> for Failure {
    fn from(value: Unavailable) -> Self {
        Self::Unavailable(value)
    }
}

/// 已验证快照里的一个插件条目（安装链路用）。
#[derive(Debug, Clone)]
pub struct PluginPackage {
    pub entry: PluginEntry,
    /// 命中快照的镜像基地址（blob 从同一来源下载）。
    pub base: String,
    pub channel: String,
    /// 该 `(id, version)` 已被服务端吊销时的原因。
    pub revoked: Option<String>,
}

/// 已验证快照里的一个内核升级条目（P3 只读段：提示与手动下载，不执行升级）。
#[derive(Debug, Clone)]
pub struct UpgradePackage {
    pub entry: UpgradeEntry,
    /// 命中快照的镜像基地址（blob 从同一来源下载）。
    pub base: String,
    pub channel: String,
}

/// blob 下载失败的原因：调用方据此区分「下载失败」与「摘要/大小不符」。
#[derive(Debug)]
pub enum DownloadError {
    /// 网络、HTTP 状态或非 200。
    Download(String),
    /// 条目缺摘要/URL/大小，或下载内容与目录声明不符。
    Digest(String),
    /// 写临时文件失败。
    Io(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download(m) | Self::Digest(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DownloadError {}

/// 内存快照及其原始索引：安装链路既要展示行，也要条目里的摘要/URL/大小。
struct Memo {
    at: Instant,
    snapshot: CatalogSnapshot,
    index: Option<Index>,
    base: String,
    /// 这次结果是否算「成功」（Fresh / Cached）。失败只冷却 [`FAILURE_TTL`]。
    ok: bool,
}

/// 单个镜像的连续失败计数与下次可尝试时间（进程内状态，重启清零）。
#[derive(Debug, Clone, Copy)]
struct MirrorHealth {
    failures: u32,
    retry_at: Instant,
}

pub struct CatalogService {
    settings: CatalogSettings,
    root: PathBuf,
    transport: SharedTransport,
    public_key: Option<[u8; 32]>,
    memo: Mutex<Option<Memo>>,
    /// 按镜像基地址记录的失败退避，避免慢/死镜像每次自动刷新都白等一个超时。
    health: Mutex<HashMap<String, MirrorHealth>>,
    /// 失败审计出口。未装配时只记日志：目录判定不依赖数据库可用性。
    audit_db: OnceLock<Arc<Db>>,
}

impl CatalogService {
    /// 生产装配：缓存根为 `<data_dir>/catalog`，传输走 ureq。
    pub fn new(settings: CatalogSettings, data_dir: &Path) -> Arc<Self> {
        let transport: SharedTransport =
            Arc::new(UreqTransport::new(settings.timeout, &settings.extra_roots));
        Self::build(settings, data_dir.join("catalog"), transport)
    }

    /// 测试装配：注入假传输层，绝不触网。
    pub fn with_transport(
        settings: CatalogSettings,
        root: PathBuf,
        transport: SharedTransport,
    ) -> Arc<Self> {
        Self::build(settings, root, transport)
    }

    fn build(settings: CatalogSettings, root: PathBuf, transport: SharedTransport) -> Arc<Self> {
        let public_key = settings
            .public_key_hex()
            .and_then(|raw| verify::parse_public_key(raw).ok());
        if !settings.extra_roots.is_empty() {
            // 只报数量：证书内容与路径都不进日志。
            info!(
                roots = settings.extra_roots.len(),
                "catalog 已加载额外信任 CA 根（仍强制 https）"
            );
        }
        Arc::new(Self {
            settings,
            root,
            transport,
            public_key,
            memo: Mutex::new(None),
            health: Mutex::new(HashMap::new()),
            audit_db: OnceLock::new(),
        })
    }

    /// 该镜像当前是否允许自动刷新尝试（退避未到期则跳过）。
    fn mirror_ready(&self, base: &str) -> bool {
        match self.health.lock() {
            Ok(h) => h.get(base).is_none_or(|m| m.retry_at <= Instant::now()),
            // 锁中毒不应阻断拉取：按可用处理，让正常路径继续。
            Err(_) => true,
        }
    }

    /// 记录一次「镜像不可用」并进入指数退避；成功一次即清零。
    fn mark_mirror_failed(&self, base: &str) {
        if let Ok(mut health) = self.health.lock() {
            let failures = health.get(base).map_or(0, |m| m.failures) + 1;
            let exp = failures.saturating_sub(1).min(5);
            let delay = MIRROR_BACKOFF_BASE
                .saturating_mul(1u32 << exp)
                .min(MIRROR_BACKOFF_MAX);
            health.insert(
                base.to_string(),
                MirrorHealth {
                    failures,
                    retry_at: Instant::now() + delay,
                },
            );
        }
    }

    fn mark_mirror_ok(&self, base: &str) {
        if let Ok(mut health) = self.health.lock() {
            health.remove(base);
        }
    }

    /// 装配审计出口（启动期调用一次）。重复装配无副作用。
    pub fn set_audit_db(&self, db: Arc<Db>) {
        let _ = self.audit_db.set(db);
    }

    /// 验收/审计口径：`catalog.refresh` 失败落一条审计；写失败只记日志，
    /// 拒绝语义与页面展示不受影响。
    fn audit_refresh_failure(&self, detail: &str) {
        let Some(db) = self.audit_db.get() else {
            return;
        };
        if let Err(e) = db.audit(
            "system",
            "-",
            "catalog.refresh",
            &self.settings.channel,
            "fail",
            detail,
        ) {
            error!(
                error = %e,
                action = "catalog.refresh",
                channel = %self.settings.channel,
                "审计写入失败（目录判定已按原结果继续）"
            );
        }
    }

    pub fn settings(&self) -> &CatalogSettings {
        &self.settings
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// 取当前快照。`force` 为 true 时绕过 TTL、失败冷却与镜像退避（页面「刷新」）。
    ///
    /// 本方法会做阻塞网络 I/O，调用方必须放进 `spawn_blocking`。
    pub fn snapshot(&self, force: bool) -> CatalogSnapshot {
        self.refresh(force).0
    }

    /// 在已验证快照里找插件条目（安装链路用）。
    ///
    /// 未启用、校验失败、没有可用镜像来源时返回 `None`：这些状态下页面不提供
    /// 安装入口，命令层也不能凭一份未验证的目录下载任何东西。
    pub fn plugin_package(&self, id: &str, version: Option<&str>) -> Option<PluginPackage> {
        let (snapshot, index, base) = self.refresh(false);
        if matches!(
            snapshot.state,
            CatalogState::Disabled | CatalogState::Rejected
        ) || base.is_empty()
        {
            return None;
        }
        let index = index?;
        let entry = index
            .plugins
            .iter()
            .filter(|p| p.id == id && version.map(|v| p.version == v).unwrap_or(true))
            .max_by(|a, b| version_cmp(&a.version, &b.version))?
            .clone();
        let revoked = index
            .revoked(&entry.id, &entry.version)
            .map(|r| r.reason.clone());
        Some(PluginPackage {
            entry,
            base,
            channel: index.channel.clone(),
            revoked,
        })
    }

    /// 可安装版本：未 yank、未命中 `revoked[]` 的最大版本。
    ///
    /// 商店页选择器与安装 POST 共用这一处判定（页面显示哪个版本就装哪个）。
    pub fn installable_package(&self, id: &str) -> Option<PluginPackage> {
        let (snapshot, index, base) = self.refresh(false);
        if matches!(
            snapshot.state,
            CatalogState::Disabled | CatalogState::Rejected
        ) || base.is_empty()
        {
            return None;
        }
        let index = index?;
        let version = installable_versions(&snapshot.entries.plugins).remove(id)?;
        let entry = index
            .plugins
            .iter()
            .find(|p| p.id == id && p.version == version)?
            .clone();
        Some(PluginPackage {
            entry,
            base,
            channel: index.channel.clone(),
            revoked: None,
        })
    }

    /// 已安装插件能否升级：返回当前可安装版本（未 yank/吊销的最大版本）。
    ///
    /// 与 [`Self::installable_package`] 同源；只读内存快照或本地缓存，不联网。
    pub fn cached_installable_version(&self, id: &str) -> Option<String> {
        if !self.settings.enabled {
            return None;
        }
        if let Ok(memo) = self.memo.lock() {
            if let Some(index) = memo.as_ref().and_then(|m| m.index.as_ref()) {
                return installable_version_in(index, id);
            }
        }
        let cached = self.cached_for_current_key(&self.settings.channel)?;
        installable_version_in(&cached.index, id)
    }

    /// 已安装插件在**不联网**前提下的撤包提示：只读内存快照或本地缓存。
    ///
    /// 页面用它给「已安装但已被撤包/吊销」的插件加一条提示；离线、无镜像、
    /// 目录未刷新时同样可用，因为信息来自上次验签通过的签名索引。
    pub fn cached_takedown(&self, id: &str, version: &str) -> Option<String> {
        if !self.settings.enabled {
            return None;
        }
        if let Ok(memo) = self.memo.lock() {
            if let Some(index) = memo.as_ref().and_then(|m| m.index.as_ref()) {
                return takedown_note(index, id, version);
            }
        }
        let cached = self.cached_for_current_key(&self.settings.channel)?;
        takedown_note(&cached.index, id, version)
    }

    /// 在已验证快照里找指定版本、当前架构的内核升级条目。
    ///
    /// 与 `plugin_package` 同一取舍：目录未启用、被拒绝或没有可用镜像来源时返回
    /// `None`——页面据此安静展示「无可用升级」，命令层也不能凭未验证目录下载。
    pub fn upgrade_package(&self, version: &str, arch: &str) -> Option<UpgradePackage> {
        let (snapshot, index, base) = self.refresh(false);
        if matches!(
            snapshot.state,
            CatalogState::Disabled | CatalogState::Rejected
        ) || base.is_empty()
        {
            return None;
        }
        let index = index?;
        let entry = index
            .upgrades
            .iter()
            .find(|u| u.version == version && u.arch == arch && u.channel == index.channel)?
            .clone();
        Some(UpgradePackage {
            entry,
            base,
            channel: index.channel.clone(),
        })
    }

    /// 渠道清单里高于 `current` 的最高升级条目；没有则返回 `None`（页面空态）。
    pub fn latest_upgrade(&self, current: &str, arch: &str) -> Option<UpgradePackage> {
        let (snapshot, index, base) = self.refresh(false);
        if matches!(
            snapshot.state,
            CatalogState::Disabled | CatalogState::Rejected
        ) || base.is_empty()
        {
            return None;
        }
        let index = index?;
        let entry = index
            .upgrades
            .iter()
            .filter(|u| u.arch == arch && u.channel == index.channel)
            .filter(|u| version_gt(&u.version, current))
            .max_by(|a, b| version_cmp(&a.version, &b.version))?
            .clone();
        Some(UpgradePackage {
            entry,
            base,
            channel: index.channel.clone(),
        })
    }

    /// 按条目摘要下载插件包到 `dest`：先写 `<dest>.part`，大小与 sha256 都通过才 rename。
    ///
    /// 任何失败都不留文件（`dest` 与 `.part` 都清掉）——安装链路不允许半装状态。
    pub fn download_blob(&self, package: &PluginPackage, dest: &Path) -> Result<(), DownloadError> {
        let entry = &package.entry;
        self.download_entry(
            &package.base,
            entry.pkg_sha256.as_deref(),
            entry.pkg_url.as_deref(),
            entry.size,
            dest,
        )
    }

    /// 按条目摘要下载升级包到 `dest`，校验口径与插件包完全一致。
    pub fn download_upgrade(
        &self,
        package: &UpgradePackage,
        dest: &Path,
    ) -> Result<(), DownloadError> {
        let entry = &package.entry;
        self.download_entry(
            &package.base,
            entry.pkg_sha256.as_deref(),
            entry.pkg_url.as_deref(),
            entry.size,
            dest,
        )
    }

    fn download_entry(
        &self,
        base: &str,
        sha: Option<&str>,
        url_path: Option<&str>,
        size: Option<u64>,
        dest: &Path,
    ) -> Result<(), DownloadError> {
        let sha = sha.ok_or_else(|| DownloadError::Digest("目录条目缺少内容摘要".into()))?;
        let url_path =
            url_path.ok_or_else(|| DownloadError::Digest("目录条目缺少内容寻址 URL".into()))?;
        let size = size.ok_or_else(|| DownloadError::Digest("目录条目缺少字节数".into()))?;
        let expected = format!("{BLOB_PREFIX}{sha}");
        if url_path != expected {
            return Err(DownloadError::Digest(format!(
                "内容寻址 URL 与摘要不一致: {url_path}"
            )));
        }

        let url = format!("{}/{url_path}", base.trim_end_matches('/'));
        let part = dest.with_extension("part");
        let result = self.store_blob(&url, sha, size, &part, dest);
        if result.is_err() {
            let _ = fs::remove_file(&part);
            let _ = fs::remove_file(dest);
        }
        result
    }

    fn store_blob(
        &self,
        url: &str,
        sha: &str,
        size: u64,
        part: &Path,
        dest: &Path,
    ) -> Result<(), DownloadError> {
        let reply = self
            .transport
            .get_blob(url)
            .map_err(|e| DownloadError::Download(format!("下载失败: {e}")))?;
        if reply.status != 200 {
            return Err(DownloadError::Download(format!(
                "下载失败: HTTP {}",
                reply.status
            )));
        }
        if reply.body.len() as u64 != size {
            return Err(DownloadError::Digest(format!(
                "包大小与目录不符：目录 {size} 字节，实得 {} 字节",
                reply.body.len()
            )));
        }
        let actual = hex::encode(Sha256::digest(&reply.body));
        if actual != sha {
            return Err(DownloadError::Digest(format!(
                "包摘要与目录不符：目录 {sha}，实得 {actual}"
            )));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| DownloadError::Io(e.to_string()))?;
        }
        fs::write(part, &reply.body).map_err(|e| DownloadError::Io(e.to_string()))?;
        set_private(part)?;
        fs::rename(part, dest).map_err(|e| DownloadError::Io(e.to_string()))
    }

    fn refresh(&self, force: bool) -> (CatalogSnapshot, Option<Index>, String) {
        if !self.settings.enabled {
            return (
                CatalogSnapshot::empty(
                    &self.settings,
                    CatalogState::Disabled,
                    "商店目录未启用".into(),
                ),
                None,
                String::new(),
            );
        }
        if !force {
            if let Ok(memo) = self.memo.lock() {
                if let Some(m) = memo.as_ref() {
                    // 成功按一天复用；失败只冷却 5 分钟，允许较快重试。
                    let ttl = if m.ok { DEFAULT_TTL } else { FAILURE_TTL };
                    if m.at.elapsed() < ttl {
                        return (m.snapshot.clone(), m.index.clone(), m.base.clone());
                    }
                }
            }
            // 进程重启后内存为空：磁盘快照未过 JSON TTL 时直接复用，不联网。
            if let Some(cached) = self.cached_for_current_key(&self.settings.channel) {
                if cache_fresh(&cached.meta.fetched_at) {
                    return self.cached_snapshot(
                        &cached,
                        CatalogState::Cached,
                        "本地缓存未过期（JSON TTL 一天），直接复用；点「刷新」可强制重拉",
                    );
                }
            }
        }
        let (snap, index, base) = self.load(force);
        let ok = !matches!(snap.state, CatalogState::Offline | CatalogState::Rejected);
        if let Ok(mut memo) = self.memo.lock() {
            *memo = Some(Memo {
                at: Instant::now(),
                snapshot: snap.clone(),
                index: index.clone(),
                base: base.clone(),
                ok,
            });
        }
        (snap, index, base)
    }

    /// 当前配置的公钥指纹；未配置公钥（仅开发模式允许未签名渠道）时为 `None`。
    fn current_key_id(&self) -> Option<String> {
        self.public_key.as_ref().map(verify::key_id_of)
    }

    /// 读缓存，但只接受「写入时的公钥指纹 == 当前配置」的那份。
    ///
    /// 换钥后旧快照不能再用：既不展示旧条目，也不允许凭旧条目安装；同时让它
    /// 等价于「没有缓存」，条件请求头自然为空 → 忽略 ETag 强制重拉新清单。
    fn cached_for_current_key(&self, channel: &str) -> Option<store::CachedCatalog> {
        let cached = store::load(&self.root, channel)?;
        let current = self.current_key_id();
        if cached.meta.key_id == current {
            return Some(cached);
        }
        info!(
            channel = %channel,
            cached_key = cached.meta.key_id.as_deref().unwrap_or("-"),
            current_key = current.as_deref().unwrap_or("-"),
            "商店目录公钥已变更，丢弃旧缓存并强制重新拉取"
        );
        None
    }

    fn load(&self, force: bool) -> (CatalogSnapshot, Option<Index>, String) {
        let channel = &self.settings.channel;
        let cached = self.cached_for_current_key(channel);
        if self.settings.mirrors.is_empty() {
            return match cached {
                Some(c) => self.cached_snapshot(
                    &c,
                    CatalogState::Cached,
                    "未配置镜像地址，正在使用本地缓存",
                ),
                None => (
                    CatalogSnapshot::empty(
                        &self.settings,
                        CatalogState::Rejected,
                        "尚未配置镜像地址，也没有本地缓存；请在「设置 → 商店目录」填写镜像地址后刷新".into(),
                    ),
                    None,
                    String::new(),
                ),
            };
        }

        // 自动刷新：跳过仍在退避期的镜像，避免每次 TTL 到期都白等一个超时。
        // 手动刷新（`force`）绕过退避，用户点击即视为「现在就重试」。
        let ready: Vec<&String> = if force {
            self.settings.mirrors.iter().collect()
        } else {
            self.settings
                .mirrors
                .iter()
                .filter(|m| self.mirror_ready(m.trim_end_matches('/')))
                .collect()
        };
        let targets: Vec<&String> = if !force && ready.is_empty() {
            match cached.as_ref() {
                // 全部退避中且已有快照：直接展示缓存，不再发请求；失败审计已在上次尝试记过，
                // 这里不再重复写，避免每次页面访问都刷一条。
                Some(c) => {
                    return self.cached_snapshot(
                        c,
                        CatalogState::Offline,
                        "镜像源暂不可达（退避中），正在使用本地缓存；点「刷新」可立即重试",
                    );
                }
                // 没有缓存时至少给第一个镜像一次机会，避免首次访问就永久空态。
                None => self.settings.mirrors.iter().take(1).collect(),
            }
        } else {
            ready
        };

        let mut errors: Vec<String> = Vec::new();
        for mirror in targets {
            let base = mirror.trim_end_matches('/');
            if base.is_empty() {
                continue;
            }
            match self.sync_one(base, cached.as_ref()) {
                Ok(triple) => {
                    self.mark_mirror_ok(base);
                    return triple;
                }
                Err(Failure::Rejected(r)) => {
                    warn!(channel = %channel, source = base, reason = %r.kind.code(), detail = %r.detail, "商店目录被拒绝");
                    self.audit_refresh_failure(&format!("reason={} {}", r.kind.code(), r.detail));
                    return match cached {
                        Some(c) => self.cached_snapshot(
                            &c,
                            CatalogState::Rejected,
                            format!("校验失败：{}；已拒绝该清单并保留上次成功快照", r.detail),
                        ),
                        None => (
                            CatalogSnapshot::empty(
                                &self.settings,
                                CatalogState::Rejected,
                                format!("校验失败：{}", r.detail),
                            ),
                            None,
                            String::new(),
                        ),
                    };
                }
                Err(Failure::Unavailable(u)) => {
                    self.mark_mirror_failed(base);
                    errors.push(format!("{base}: {}", u.0));
                }
            }
        }

        let detail = errors.join("；");
        self.audit_refresh_failure(&format!("reason=offline {detail}"));
        match cached {
            Some(c) => self.cached_snapshot(
                &c,
                CatalogState::Offline,
                format!("镜像源不可达，正在使用本地缓存；点「刷新」可立即重试（{detail}）"),
            ),
            None => (
                CatalogSnapshot::empty(
                    &self.settings,
                    CatalogState::Offline,
                    format!("尚未连上任何镜像源，也没有本地缓存；请在「设置 → 商店目录」填写镜像地址后刷新（{detail}）"),
                ),
                None,
                String::new(),
            ),
        }
    }

    fn sync_one(
        &self,
        base: &str,
        cached: Option<&store::CachedCatalog>,
    ) -> Result<(CatalogSnapshot, Option<Index>, String), Failure> {
        let channel = &self.settings.channel;
        let manifest_url = format!("{base}/channels/{channel}/manifest.json");
        let cond = Conditional {
            etag: cached.and_then(|c| c.meta.etag_manifest.as_deref()),
            last_modified: cached.and_then(|c| c.meta.last_modified_manifest.as_deref()),
        };
        let reply = self
            .transport
            .get(&manifest_url, &cond)
            .map_err(|e| Unavailable(format!("manifest 拉取失败: {e}")))?;
        let (manifest_bytes, etag_manifest, last_modified_manifest, not_modified) = match reply
            .status
        {
            200 => (reply.body, reply.etag, reply.last_modified, false),
            304 => {
                let c = cached
                    .ok_or_else(|| Unavailable("manifest 返回 304，但本地没有可复用快照".into()))?;
                (
                    c.manifest_bytes.clone(),
                    c.meta.etag_manifest.clone(),
                    c.meta.last_modified_manifest.clone(),
                    true,
                )
            }
            code => return Err(Unavailable(format!("manifest HTTP {code}")).into()),
        };

        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| Unavailable(format!("manifest 解析失败: {e}")))?;
        manifest
            .validate()
            .map_err(|e| Rejected::digest(format!("manifest 校验失败: {e}")))?;
        if manifest.channel != *channel {
            return Err(Rejected::digest(format!(
                "manifest 渠道 {} 与配置渠道 {channel} 不一致",
                manifest.channel
            ))
            .into());
        }

        // 304 表示内容未变：缓存里的清单写入前已经验过签，直接复用原始签名，
        // 不再发一次签名请求（也避免把「304 但签名丢了」误判成未签名渠道）。
        let signature = if not_modified {
            cached.and_then(|c| c.signature.clone())
        } else {
            let fetched = self.fetch_signature(&manifest_url)?;
            self.check_signature(
                &manifest_bytes,
                &manifest,
                fetched.as_ref().map(|(_, sig)| sig),
            )?;
            fetched.map(|(raw, _)| raw)
        };

        // 防回滚：seq 一旦见过更大的值，小值直接拒绝（不覆盖缓存）。
        if let Some(c) = cached {
            if manifest.seq < c.meta.seq {
                return Err(Rejected::seq(format!(
                    "清单 seq {} 小于已见值 {}（防回滚）",
                    manifest.seq, c.meta.seq
                ))
                .into());
            }
        }

        let index_url = format!("{base}/channels/{channel}/index.json");
        let cond = Conditional {
            etag: cached.and_then(|c| c.meta.etag_index.as_deref()),
            last_modified: cached.and_then(|c| c.meta.last_modified_index.as_deref()),
        };
        let reply = self
            .transport
            .get(&index_url, &cond)
            .map_err(|e| Unavailable(format!("index 拉取失败: {e}")))?;
        let (index_bytes, etag_index, last_modified_index) = match reply.status {
            200 => (reply.body, reply.etag, reply.last_modified),
            304 => {
                let c = cached
                    .ok_or_else(|| Unavailable("index 返回 304，但本地没有可复用快照".into()))?;
                (
                    c.index_bytes.clone(),
                    c.meta.etag_index.clone(),
                    c.meta.last_modified_index.clone(),
                )
            }
            code => return Err(Unavailable(format!("index HTTP {code}")).into()),
        };

        if index_bytes.len() as u64 != manifest.index.size {
            return Err(Rejected::digest(format!(
                "索引大小与 manifest 声明不符：声明 {} 字节，实得 {} 字节",
                manifest.index.size,
                index_bytes.len()
            ))
            .into());
        }
        let actual = hex::encode(Sha256::digest(&index_bytes));
        if actual != manifest.index.sha256 {
            return Err(Rejected::digest(format!(
                "索引摘要与 manifest 不符：声明 {}，实得 {actual}",
                manifest.index.sha256
            ))
            .into());
        }

        let index: Index = serde_json::from_slice(&index_bytes)
            .map_err(|e| Rejected::digest(format!("索引解析失败: {e}")))?;
        if index.channel != manifest.channel || index.seq != manifest.seq {
            return Err(Rejected::digest(format!(
                "索引与 manifest 不一致：索引 {}/seq {}，manifest {}/seq {}",
                index.channel, index.seq, manifest.channel, manifest.seq
            ))
            .into());
        }
        index
            .validate(Some(&store::blob_dir(&self.root)))
            .map_err(|e| Rejected::digest(format!("索引条目校验失败: {e}")))?;

        let fetched_at = now_rfc3339();
        let meta = store::CacheMeta {
            channel: channel.clone(),
            seq: manifest.seq,
            fetched_at: fetched_at.clone(),
            source: base.to_string(),
            // 记录写入时生效的公钥指纹，供下次载入判断「是否已换钥」。
            key_id: self.current_key_id(),
            etag_manifest,
            last_modified_manifest,
            etag_index,
            last_modified_index,
        };
        if let Err(e) = store::save(
            &self.root,
            channel,
            &manifest_bytes,
            // 保存原始签名字节：重新序列化会改变字段顺序，验签输入就对不上了。
            signature.as_deref(),
            &index_bytes,
            &meta,
        ) {
            // 缓存写失败不影响本次展示，但下次仍需重新拉取。
            warn!(error = %e, "商店目录缓存写入失败");
        }

        let (state, message) = if not_modified {
            (
                CatalogState::Cached,
                format!("目录未更新（304），展示 {base} 的本地缓存"),
            )
        } else {
            info!(channel = %channel, seq = manifest.seq, source = base, "商店目录已更新");
            (
                CatalogState::Fresh,
                format!("已从 {base} 更新到 seq {}", manifest.seq),
            )
        };
        Ok((
            self.snapshot_from(
                &manifest,
                &index,
                state,
                message,
                base.to_string(),
                fetched_at,
            ),
            Some(index),
            base.to_string(),
        ))
    }

    /// 返回签名文件的**原始字节**与解析结果：缓存必须落原始字节，
    /// 重新序列化会改变字段顺序导致下次验签失败。
    fn fetch_signature(
        &self,
        manifest_url: &str,
    ) -> Result<Option<(Vec<u8>, SignatureFile)>, Failure> {
        let url = format!("{manifest_url}.sig");
        let reply = self
            .transport
            .get(&url, &Conditional::default())
            .map_err(|e| Unavailable(format!("签名拉取失败: {e}")))?;
        match reply.status {
            200 => match serde_json::from_slice::<SignatureFile>(&reply.body) {
                Ok(sig) => Ok(Some((reply.body, sig))),
                Err(e) => Err(Rejected::signature(format!("签名文件解析失败: {e}")).into()),
            },
            404 => Ok(None),
            code => Err(Unavailable(format!("签名 HTTP {code}")).into()),
        }
    }

    fn check_signature(
        &self,
        manifest_bytes: &[u8],
        manifest: &Manifest,
        signature: Option<&SignatureFile>,
    ) -> Result<(), Failure> {
        let unsigned = signature.map(|s| s.is_placeholder()).unwrap_or(true);
        if unsigned {
            return if self.settings.allow_unsigned {
                Ok(())
            } else {
                Err(
                    Rejected::signature("渠道未签名或签名为占位，默认拒绝（仅开发模式可显式允许）")
                        .into(),
                )
            };
        }
        let Some(key) = self.public_key.as_ref() else {
            return Err(Rejected::signature("渠道已签名，但客户端未配置固定 Ed25519 公钥").into());
        };
        verify::verify_manifest(key, manifest_bytes, manifest, signature.unwrap())
            .map_err(|e| Rejected::signature(format!("验签失败: {e}")).into())
    }

    fn cached_snapshot(
        &self,
        cached: &store::CachedCatalog,
        state: CatalogState,
        message: impl Into<String>,
    ) -> (CatalogSnapshot, Option<Index>, String) {
        let mut snap = self.snapshot_from(
            &cached.manifest,
            &cached.index,
            state,
            message.into(),
            cached.meta.source.clone(),
            cached.meta.fetched_at.clone(),
        );
        snap.seq = Some(cached.manifest.seq);
        (snap, Some(cached.index.clone()), cached.meta.source.clone())
    }

    fn snapshot_from(
        &self,
        manifest: &Manifest,
        index: &Index,
        state: CatalogState,
        message: String,
        source: String,
        fetched_at: String,
    ) -> CatalogSnapshot {
        CatalogSnapshot {
            state,
            channel: manifest.channel.clone(),
            seq: Some(manifest.seq),
            fetched_at,
            source,
            message,
            entries: entries_of(index),
            allow_unsigned: self.settings.allow_unsigned,
            key_configured: self.public_key.is_some(),
        }
    }
}

/// 撤包提示的唯一口径：`revoked[]` 优先于 `yanked`，两者都不命中则返回 `None`。
fn takedown_note(index: &Index, id: &str, version: &str) -> Option<String> {
    if let Some(entry) = index.revoked(id, version) {
        return Some(format!("该版本已被服务端撤回：{}", entry.reason));
    }
    let entry = index
        .plugins
        .iter()
        .find(|p| p.id == id && p.version == version && p.yanked)?;
    Some(match entry.yank_reason.as_deref() {
        Some(reason) if !reason.trim().is_empty() => {
            format!("该版本已被服务端下架：{reason}")
        }
        _ => "该版本已被服务端下架（yanked），请升级到新版本".to_string(),
    })
}

/// 应用模板提示（R2e）：独立吊销命名空间优先于 `yanked`。
fn app_note(index: &Index, entry: &crate::catalog::model::AppEntry) -> String {
    if let Some(revoked) = index.revoked_app(&entry.id, &entry.version) {
        return format!("已撤回：{}", revoked.reason);
    }
    if entry.yanked {
        return match entry.yank_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => format!("已下架：{reason}"),
            _ => "已下架".into(),
        };
    }
    String::new()
}

/// 运行环境提示（R2e）：吊销 > 下架 > EOL。
fn runtime_note(index: &Index, entry: &crate::catalog::model::RuntimeEntry) -> String {
    if let Some(revoked) = index.revoked_runtime(&entry.kind, &entry.version) {
        return format!("已撤回：{}", revoked.reason);
    }
    if entry.yanked {
        return match entry.yank_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => format!("已下架：{reason}"),
            _ => "已下架".into(),
        };
    }
    entry
        .eol
        .as_ref()
        .map(|eol| format!("已 EOL（{eol}）"))
        .unwrap_or_default()
}

/// 版本比较的唯一入口：与插件安装共用 `skiff_plugin::install::version_gt` 的语义。
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    if version_gt(a, b) {
        std::cmp::Ordering::Greater
    } else if version_gt(b, a) {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Equal
    }
}

/// 暂存包只给安装进程读：unix 下落 0600（与上传路径一致）。
fn set_private(path: &Path) -> Result<(), DownloadError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| DownloadError::Io(e.to_string()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// 四类条目 → 页面行。只做展示映射，不再引入新的语义。
fn entries_of(index: &Index) -> CatalogEntries {
    let updated = index.generated_at.clone();
    CatalogEntries {
        plugins: index
            .plugins
            .iter()
            .map(|p| {
                let revoked = index.revoked(&p.id, &p.version);
                let compatibility = if p.requires.is_empty() {
                    format!("API {}", p.api)
                } else {
                    format!("API {} · {}", p.api, p.requires.join(", "))
                };
                CatalogItem {
                    name: p.id.clone(),
                    version: p.version.clone(),
                    source: index.channel.clone(),
                    updated_at: p.published_at.clone(),
                    compatibility,
                    note: p.summary.clone(),
                    warn: match (revoked, p.yanked) {
                        (Some(r), _) => format!("已撤回：{}", r.reason),
                        (None, true) => match p.yank_reason.as_deref() {
                            Some(reason) if !reason.trim().is_empty() => {
                                format!("已撤回：{reason}")
                            }
                            _ => "已撤回".into(),
                        },
                        (None, false) => String::new(),
                    },
                    installable: true,
                    action: "安装".into(),
                    install_note: String::new(),
                }
            })
            .collect(),
        apps: index
            .apps
            .iter()
            .map(|a| CatalogItem {
                name: a.id.clone(),
                version: a.version.clone(),
                source: index.channel.clone(),
                updated_at: updated.clone(),
                compatibility: format!("内核 ≥ {}", a.min_kernel),
                note: a.summary.clone(),
                warn: app_note(index, a),
                installable: true,
                action: "安装".into(),
                install_note: String::new(),
            })
            .collect(),
        runtimes: index
            .runtimes
            .iter()
            .map(|r| CatalogItem {
                name: r.kind.clone(),
                version: r.version.clone(),
                source: r.image.clone(),
                updated_at: updated.clone(),
                compatibility: format!(
                    "内核 ≥ {}{}",
                    r.api_min,
                    if r.default { " · 默认" } else { "" }
                ),
                note: r.image_digest.clone(),
                warn: runtime_note(index, r),
                installable: true,
                action: "安装".into(),
                install_note: String::new(),
            })
            .collect(),
        upgrades: index
            .upgrades
            .iter()
            .map(|u| CatalogItem {
                name: "内核升级".into(),
                version: u.version.clone(),
                source: format!("{} · {}", u.channel, u.arch),
                updated_at: updated.clone(),
                compatibility: format!("≥ {} · {}", u.min_from, u.arch),
                note: u.changelog_url.clone(),
                warn: String::new(),
                installable: true,
                action: "安装".into(),
                install_note: String::new(),
            })
            .collect(),
    }
}

/// 同一 id 的可安装版本：`warn` 为空（未 yank、未命中 `revoked[]`）的最大版本。
///
/// **商店页选择器与安装 POST 共用这一处判定**：页面显示哪个版本，`prepare`
/// 就装哪个版本。`warn` 本身由 [`entries_of`] 从 `revoked[]` / `yanked` 推导，
/// 因此不存在第二套「最大版本」算法。
pub fn installable_versions(items: &[CatalogItem]) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for it in items.iter().filter(|i| i.warn.is_empty()) {
        match out.get(&it.name) {
            Some(cur) if version_cmp(&it.version, cur) != std::cmp::Ordering::Greater => {}
            _ => {
                out.insert(it.name.clone(), it.version.clone());
            }
        }
    }
    out
}

/// 同一 id 的最高版本（不看 yank/吊销）：用于「全部不可安装」时在原行展示原因。
pub fn newest_versions(items: &[CatalogItem]) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for it in items {
        match out.get(&it.name) {
            Some(cur) if version_cmp(&it.version, cur) != std::cmp::Ordering::Greater => {}
            _ => {
                out.insert(it.name.clone(), it.version.clone());
            }
        }
    }
    out
}

/// 索引级入口：与 [`installable_versions`] 同一口径（先映射成页面行再选最大）。
fn installable_version_in(index: &Index, id: &str) -> Option<String> {
    installable_versions(&entries_of(index).plugins).remove(id)
}

/// 磁盘快照是否仍在 JSON TTL 内。按 `meta.json` 的 `fetched_at` 判定，跨进程有效；
/// 时间戳缺失/无法解析一律视为过期（宁可多拉一次，不把坏元数据当新鲜）。
fn cache_fresh(fetched_at: &str) -> bool {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(fetched_at) else {
        return false;
    };
    let age = chrono::Utc::now().signed_duration_since(at.with_timezone(&chrono::Utc));
    age < chrono::Duration::from_std(DEFAULT_TTL).unwrap_or(chrono::Duration::days(1))
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests;
