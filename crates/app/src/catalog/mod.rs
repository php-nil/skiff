//! 商店目录客户端（Skiff-server P1：只读接入，不做安装）。
//!
//! 职责只有四件事：按渠道拉取 `channels/<name>/{manifest.json,manifest.json.sig,index.json}`、
//! 用固定 Ed25519 公钥验签、做 `seq` 防回滚与摘要校验、把最近一次成功快照落本地缓存。
//! 任何一步失败都**不使用**新清单：页面继续展示上次成功快照并标注状态。
//!
//! 本模块不碰现有插件安装/信任链路：它只产出展示用快照，不写库、不触发安装。

pub mod fetch;
pub mod model;
pub mod store;
pub mod verify;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::SecondsFormat;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use fetch::{Conditional, SharedTransport, UreqTransport};
use model::{Index, Manifest, SignatureFile};

/// 单次 HTTP 请求的默认硬超时。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// 页面连续访问时复用内存快照的时长；「刷新」按钮显式绕过。
pub const DEFAULT_TTL: Duration = Duration::from_secs(30);

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
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "未启用",
            Self::Fresh => "已更新",
            Self::Cached => "目录未更新",
            Self::Offline => "离线",
            Self::Rejected => "校验失败",
        }
    }

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
struct Rejected(String);
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

pub struct CatalogService {
    settings: CatalogSettings,
    root: PathBuf,
    transport: SharedTransport,
    public_key: Option<[u8; 32]>,
    memo: Mutex<Option<(Instant, CatalogSnapshot)>>,
}

impl CatalogService {
    /// 生产装配：缓存根为 `<data_dir>/catalog`，传输走 ureq。
    pub fn new(settings: CatalogSettings, data_dir: &Path) -> Arc<Self> {
        let transport: SharedTransport = Arc::new(UreqTransport::new(settings.timeout));
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
        Arc::new(Self {
            settings,
            root,
            transport,
            public_key,
            memo: Mutex::new(None),
        })
    }

    pub fn settings(&self) -> &CatalogSettings {
        &self.settings
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// 取当前快照。`force` 为 true 时绕过内存 TTL（页面「刷新」）。
    ///
    /// 本方法会做阻塞网络 I/O，调用方必须放进 `spawn_blocking`。
    pub fn snapshot(&self, force: bool) -> CatalogSnapshot {
        if !self.settings.enabled {
            return CatalogSnapshot::empty(
                &self.settings,
                CatalogState::Disabled,
                "商店目录未启用".into(),
            );
        }
        if !force {
            if let Ok(memo) = self.memo.lock() {
                if let Some((at, snap)) = memo.as_ref() {
                    if at.elapsed() < DEFAULT_TTL {
                        return snap.clone();
                    }
                }
            }
        }
        let snap = self.load();
        if let Ok(mut memo) = self.memo.lock() {
            *memo = Some((Instant::now(), snap.clone()));
        }
        snap
    }

    fn load(&self) -> CatalogSnapshot {
        let channel = &self.settings.channel;
        let cached = store::load(&self.root, channel);
        if self.settings.mirrors.is_empty() {
            return match cached {
                Some(c) => {
                    self.cached_snapshot(&c, CatalogState::Cached, "未配置镜像地址，展示本地缓存")
                }
                None => CatalogSnapshot::empty(
                    &self.settings,
                    CatalogState::Rejected,
                    "未配置镜像地址，且没有本地缓存".into(),
                ),
            };
        }

        let mut errors: Vec<String> = Vec::new();
        for mirror in &self.settings.mirrors {
            let base = mirror.trim_end_matches('/');
            if base.is_empty() {
                continue;
            }
            match self.sync_one(base, cached.as_ref()) {
                Ok(snap) => return snap,
                Err(Failure::Rejected(r)) => {
                    warn!(channel = %channel, source = base, reason = %r.0, "商店目录被拒绝");
                    return match cached {
                        Some(c) => self.cached_snapshot(
                            &c,
                            CatalogState::Rejected,
                            format!("校验失败：{}；已拒绝该清单并保留上次成功快照", r.0),
                        ),
                        None => CatalogSnapshot::empty(
                            &self.settings,
                            CatalogState::Rejected,
                            format!("校验失败：{}", r.0),
                        ),
                    };
                }
                Err(Failure::Unavailable(u)) => errors.push(format!("{base}: {}", u.0)),
            }
        }

        let detail = errors.join("；");
        match cached {
            Some(c) => self.cached_snapshot(
                &c,
                CatalogState::Offline,
                format!("离线：{detail}；展示上次成功快照"),
            ),
            None => CatalogSnapshot::empty(
                &self.settings,
                CatalogState::Offline,
                format!("离线：{detail}；没有可用快照"),
            ),
        }
    }

    fn sync_one(
        &self,
        base: &str,
        cached: Option<&store::CachedCatalog>,
    ) -> Result<CatalogSnapshot, Failure> {
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
            .map_err(|e| Rejected(format!("manifest 校验失败: {e}")))?;
        if manifest.channel != *channel {
            return Err(Rejected(format!(
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
                return Err(Rejected(format!(
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
            return Err(Rejected(format!(
                "索引大小与 manifest 声明不符：声明 {} 字节，实得 {} 字节",
                manifest.index.size,
                index_bytes.len()
            ))
            .into());
        }
        let actual = hex::encode(Sha256::digest(&index_bytes));
        if actual != manifest.index.sha256 {
            return Err(Rejected(format!(
                "索引摘要与 manifest 不符：声明 {}，实得 {actual}",
                manifest.index.sha256
            ))
            .into());
        }

        let index: Index = serde_json::from_slice(&index_bytes)
            .map_err(|e| Rejected(format!("索引解析失败: {e}")))?;
        if index.channel != manifest.channel || index.seq != manifest.seq {
            return Err(Rejected(format!(
                "索引与 manifest 不一致：索引 {}/seq {}，manifest {}/seq {}",
                index.channel, index.seq, manifest.channel, manifest.seq
            ))
            .into());
        }
        index
            .validate(Some(&store::blob_dir(&self.root)))
            .map_err(|e| Rejected(format!("索引条目校验失败: {e}")))?;

        let fetched_at = now_rfc3339();
        let meta = store::CacheMeta {
            channel: channel.clone(),
            seq: manifest.seq,
            fetched_at: fetched_at.clone(),
            source: base.to_string(),
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
        Ok(self.snapshot_from(
            &manifest,
            &index,
            state,
            message,
            base.to_string(),
            fetched_at,
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
                Err(e) => Err(Rejected(format!("签名文件解析失败: {e}")).into()),
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
                    Rejected("渠道未签名或签名为占位，默认拒绝（仅开发模式可显式允许）".into())
                        .into(),
                )
            };
        }
        let Some(key) = self.public_key.as_ref() else {
            return Err(Rejected("渠道已签名，但客户端未配置固定 Ed25519 公钥".into()).into());
        };
        verify::verify_manifest(key, manifest_bytes, manifest, signature.unwrap())
            .map_err(|e| Rejected(format!("验签失败: {e}")).into())
    }

    fn cached_snapshot(
        &self,
        cached: &store::CachedCatalog,
        state: CatalogState,
        message: impl Into<String>,
    ) -> CatalogSnapshot {
        let mut snap = self.snapshot_from(
            &cached.manifest,
            &cached.index,
            state,
            message.into(),
            cached.meta.source.clone(),
            cached.meta.fetched_at.clone(),
        );
        snap.seq = Some(cached.manifest.seq);
        snap
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
                        (None, true) => "已撤回".into(),
                        (None, false) => String::new(),
                    },
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
                warn: String::new(),
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
                warn: r
                    .eol
                    .as_ref()
                    .map(|eol| format!("已 EOL（{eol}）"))
                    .unwrap_or_default(),
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
            })
            .collect(),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests;
