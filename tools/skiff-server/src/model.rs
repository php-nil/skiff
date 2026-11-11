//! Skiff-server P0 清单对象模型与语义校验。
//!
//! JSON Schema 负责结构与枚举；本模块负责 schema 表达不了的规则：反向域名 id、
//! 语义化版本、64 位小写十六进制、UTC 时间戳、内容寻址 URL，以及源数据与产物
//! 两种形态的字段差异（源数据用 `asset`，产物用摘要 + 相对 URL + size）。

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const CHANNEL_SCHEMA: &str = "skiff.server.channel/v1";
pub const INDEX_SCHEMA: &str = "skiff.server.index/v1";
pub const MANIFEST_SCHEMA: &str = "skiff.server.manifest/v1";
pub const BLOB_PREFIX: &str = "blobs/sha256/";

pub const KNOWN_CHANNELS: &[&str] = &["stable", "night"];
pub const KNOWN_CAPABILITIES: &[&str] = &[
    "docker",
    "identity",
    "fs",
    "net",
    "interop",
    "scheduler",
    "ftp",
    "firewall",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelFile {
    pub schema: String,
    pub channel: String,
    pub seq: u64,
    pub generated_at: String,
}

impl ChannelFile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != CHANNEL_SCHEMA {
            bail!(
                "渠道配置 schema 必须是 {CHANNEL_SCHEMA}，实为 {}",
                self.schema
            );
        }
        validate_channel(&self.channel)?;
        if self.seq == 0 {
            bail!("seq 必须从 1 开始");
        }
        validate_timestamp(&self.generated_at)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEntry {
    pub id: String,
    pub version: String,
    pub api: String,
    pub channel: String,
    pub key_id: String,
    #[serde(default)]
    pub requires: Vec<String>,
    pub summary: String,
    pub published_at: String,
    #[serde(default)]
    pub yanked: bool,
    /// 撤包原因（可选）。R1 新增字段：旧客户端忽略未知字段，因此不升 schema 版本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yank_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
}

impl PluginEntry {
    fn validate_common(&self) -> Result<()> {
        validate_id(&self.id, "插件 id")?;
        validate_semver(&self.version, "插件版本")?;
        validate_api(&self.api)?;
        validate_channel(&self.channel)?;
        validate_hex64(&self.key_id, "插件 key_id")?;
        validate_summary(&self.summary, "插件 summary")?;
        validate_timestamp(&self.published_at)?;
        if let Some(reason) = &self.yank_reason {
            validate_summary(reason, "插件 yank_reason")?;
        }
        let mut seen = BTreeSet::new();
        for capability in &self.requires {
            if !KNOWN_CAPABILITIES.contains(&capability.as_str()) {
                bail!("插件 {} 声明了未知能力面: {capability}", self.id);
            }
            if !seen.insert(capability.as_str()) {
                bail!("插件 {} 的能力面重复: {capability}", self.id);
            }
        }
        Ok(())
    }

    /// 源数据形态：必须给出本地 asset，摘要字段允许缺省（由生成器派生）。
    pub fn validate_source(&self) -> Result<()> {
        self.validate_common()?;
        let asset = self
            .asset
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("插件 {} 的源条目缺少 asset", self.id))?;
        validate_asset_path(asset)?;
        Ok(())
    }

    /// 产物形态：必须有摘要、内容寻址 URL 与大小，且不得残留 asset。
    pub fn validate_index(&self) -> Result<()> {
        self.validate_common()?;
        validate_digest_fields(
            &format!("插件 {}", self.id),
            self.pkg_sha256.as_deref(),
            self.pkg_url.as_deref(),
            self.size,
            self.asset.as_deref(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppImage {
    #[serde(rename = "ref")]
    pub reference: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppEnv {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppPort {
    pub container: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppEntry {
    pub id: String,
    pub version: String,
    pub summary: String,
    pub min_kernel: String,
    pub images: Vec<AppImage>,
    pub env: Vec<AppEnv>,
    pub ports: Vec<AppPort>,
    pub volumes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compose_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
}

impl AppEntry {
    fn validate_common(&self) -> Result<()> {
        validate_id(&self.id, "应用 id")?;
        validate_semver(&self.version, "应用版本")?;
        validate_semver(&self.min_kernel, "min_kernel")?;
        validate_summary(&self.summary, "应用 summary")?;
        if self.images.is_empty() {
            bail!("应用 {} 至少需要一个镜像", self.id);
        }
        for image in &self.images {
            if image.reference.trim().is_empty() {
                bail!("应用 {} 存在空镜像引用", self.id);
            }
            validate_sha256_digest(&image.digest, "镜像 digest")?;
        }
        let mut keys = BTreeSet::new();
        for env in &self.env {
            let key = env.key.trim();
            if key.is_empty() || key.chars().count() > 64 {
                bail!("应用 {} 的环境变量名非法: {:?}", self.id, env.key);
            }
            if !keys.insert(key) {
                bail!("应用 {} 的环境变量重复: {key}", self.id);
            }
        }
        for port in &self.ports {
            if port.container == 0 {
                bail!("应用 {} 存在非法端口 0", self.id);
            }
            if port.protocol != "tcp" && port.protocol != "udp" {
                bail!(
                    "应用 {} 的端口协议必须是 tcp/udp: {}",
                    self.id,
                    port.protocol
                );
            }
        }
        for volume in &self.volumes {
            if !volume.starts_with('/') || volume.contains("..") {
                bail!(
                    "应用 {} 的卷必须是不含 .. 的绝对容器路径: {volume}",
                    self.id
                );
            }
        }
        Ok(())
    }

    pub fn validate_source(&self) -> Result<()> {
        self.validate_common()?;
        let asset = self
            .asset
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("应用 {} 的源条目缺少 asset", self.id))?;
        validate_asset_path(asset)
    }

    pub fn validate_index(&self) -> Result<()> {
        self.validate_common()?;
        validate_digest_fields(
            &format!("应用 {}", self.id),
            self.compose_sha256.as_deref(),
            self.compose_url.as_deref(),
            self.size,
            self.asset.as_deref(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEntry {
    pub kind: String,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    pub eol: Option<String>,
    pub api_min: String,
    pub default: bool,
}

impl RuntimeEntry {
    pub fn validate(&self) -> Result<()> {
        if !["php", "node", "rust"].contains(&self.kind.as_str()) {
            bail!("运行环境 kind 必须是 php/node/rust: {}", self.kind);
        }
        validate_plain_version(&self.version, "运行环境版本")?;
        if self.image.trim().is_empty() {
            bail!("运行环境镜像不能为空");
        }
        validate_sha256_digest(&self.image_digest, "镜像 digest")?;
        if let Some(eol) = &self.eol {
            validate_date(eol)?;
        }
        validate_api(&self.api_min)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeEntry {
    pub version: String,
    pub arch: String,
    pub channel: String,
    pub min_from: String,
    pub changelog_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pkg_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
}

impl UpgradeEntry {
    fn validate_common(&self) -> Result<()> {
        validate_semver(&self.version, "升级版本")?;
        if self.arch != "x86_64" && self.arch != "aarch64" {
            bail!("升级架构必须是 x86_64/aarch64: {}", self.arch);
        }
        validate_channel(&self.channel)?;
        validate_semver(&self.min_from, "min_from")?;
        if !self.changelog_url.starts_with("https://") {
            bail!("changelog_url 必须是 https 地址: {}", self.changelog_url);
        }
        Ok(())
    }

    pub fn validate_source(&self) -> Result<()> {
        self.validate_common()?;
        let asset = self
            .asset
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("升级条目 {} 缺少 asset", self.version))?;
        validate_asset_path(asset)
    }

    pub fn validate_index(&self) -> Result<()> {
        self.validate_common()?;
        validate_digest_fields(
            &format!("升级条目 {}", self.version),
            self.pkg_sha256.as_deref(),
            self.pkg_url.as_deref(),
            self.size,
            self.asset.as_deref(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokedEntry {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexFile {
    pub schema: String,
    pub channel: String,
    pub seq: u64,
    pub generated_at: String,
    pub plugins: Vec<PluginEntry>,
    pub apps: Vec<AppEntry>,
    pub runtimes: Vec<RuntimeEntry>,
    pub upgrades: Vec<UpgradeEntry>,
    pub revoked: Vec<RevokedEntry>,
}

impl IndexFile {
    pub fn empty(channel: &str, seq: u64, generated_at: &str) -> Self {
        Self {
            schema: INDEX_SCHEMA.to_string(),
            channel: channel.to_string(),
            seq,
            generated_at: generated_at.to_string(),
            plugins: Vec::new(),
            apps: Vec::new(),
            runtimes: Vec::new(),
            upgrades: Vec::new(),
            revoked: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != INDEX_SCHEMA {
            bail!("索引 schema 必须是 {INDEX_SCHEMA}，实为 {}", self.schema);
        }
        validate_channel(&self.channel)?;
        if self.seq == 0 {
            bail!("索引 seq 必须从 1 开始");
        }
        validate_timestamp(&self.generated_at)?;
        for entry in &self.plugins {
            entry.validate_index()?;
        }
        for entry in &self.apps {
            entry.validate_index()?;
        }
        for entry in &self.runtimes {
            entry.validate()?;
        }
        for entry in &self.upgrades {
            entry.validate_index()?;
        }
        for entry in &self.revoked {
            validate_id(&entry.id, "吊销 id")?;
            validate_summary(&entry.reason, "吊销原因")?;
            validate_timestamp(&entry.at)?;
            if let Some(sha) = &entry.sha256 {
                validate_hex64(sha, "吊销 sha256")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestIndexRef {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    pub schema: String,
    pub channel: String,
    pub seq: u64,
    pub generated_at: String,
    pub key_id: String,
    pub index: ManifestIndexRef,
}

impl ManifestFile {
    pub fn validate(&self) -> Result<()> {
        if self.schema != MANIFEST_SCHEMA {
            bail!(
                "manifest schema 必须是 {MANIFEST_SCHEMA}，实为 {}",
                self.schema
            );
        }
        validate_channel(&self.channel)?;
        if self.seq == 0 {
            bail!("manifest seq 必须从 1 开始");
        }
        validate_timestamp(&self.generated_at)?;
        validate_hex64(&self.key_id, "manifest key_id")?;
        if self.index.path != "index.json" {
            bail!("manifest.index.path 必须是 index.json");
        }
        validate_hex64(&self.index.sha256, "index 摘要")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureFile {
    pub alg: String,
    pub key_id: String,
    pub sig: String,
}

impl SignatureFile {
    pub fn validate(&self) -> Result<()> {
        if self.alg != "ed25519" {
            bail!("签名算法必须是 ed25519: {}", self.alg);
        }
        validate_hex64(&self.key_id, "签名 key_id")?;
        if !self.sig.is_empty() {
            validate_hex_bytes(&self.sig, 64, "Ed25519 签名")?;
        }
        Ok(())
    }

    pub fn is_placeholder(&self) -> bool {
        self.sig.is_empty() || self.key_id.chars().all(|c| c == '0')
    }
}

fn validate_digest_fields(
    label: &str,
    sha256: Option<&str>,
    url: Option<&str>,
    size: Option<u64>,
    asset: Option<&str>,
) -> Result<()> {
    if asset.is_some() {
        bail!("{label} 的产物条目不得包含 asset（asset 只属于源数据）");
    }
    let sha = sha256.ok_or_else(|| anyhow::anyhow!("{label} 缺少内容摘要"))?;
    validate_hex64(sha, "内容摘要")?;
    let url = url.ok_or_else(|| anyhow::anyhow!("{label} 缺少内容寻址 URL"))?;
    if url != blob_url(sha) {
        bail!("{label} 的内容寻址 URL 与摘要不一致: {url}");
    }
    size.ok_or_else(|| anyhow::anyhow!("{label} 缺少字节数"))?;
    Ok(())
}

pub fn blob_url(sha256: &str) -> String {
    format!("{BLOB_PREFIX}{sha256}")
}

pub fn validate_blob_url(url: &str) -> Result<()> {
    let hex = url
        .strip_prefix(BLOB_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("内容 URL 必须以 {BLOB_PREFIX} 开头: {url}"))?;
    validate_hex64(hex, "内容 URL 摘要")
}

pub fn validate_channel(value: &str) -> Result<()> {
    if !KNOWN_CHANNELS.contains(&value) {
        bail!("未知渠道: {value}（只允许 stable/night）");
    }
    Ok(())
}

pub fn validate_id(value: &str, label: &str) -> Result<()> {
    if value.len() < 4 || value.len() > 128 {
        bail!("{label} 长度必须在 4..=128: {value}");
    }
    if !value.contains('.') {
        bail!("{label} 必须是反向域名形式: {value}");
    }
    if value.starts_with('.') || value.ends_with('.') || value.contains("..") {
        bail!("{label} 不能以点开头/结尾，也不能含连续点: {value}");
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        bail!("{label} 只允许小写字母、数字、点和连字符: {value}");
    }
    Ok(())
}

pub fn validate_semver(value: &str, label: &str) -> Result<()> {
    let core = value.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        bail!("{label} 必须是 x.y.z 形式: {value}");
    }
    for part in parts {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            bail!("{label} 必须是 x.y.z 形式: {value}");
        }
    }
    Ok(())
}

pub fn validate_plain_version(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > 32 || value.contains(char::is_whitespace) {
        bail!("{label} 必须是 1..=32 个非空白字符: {value}");
    }
    Ok(())
}

pub fn validate_api(value: &str) -> Result<()> {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        bail!("api 必须是 主版本.次版本: {value}");
    }
    Ok(())
}

pub fn validate_summary(value: &str, label: &str) -> Result<()> {
    let len = value.chars().count();
    if len == 0 || len > 200 {
        bail!("{label} 长度必须在 1..=200: {value}");
    }
    Ok(())
}

pub fn validate_hex64(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("{label} 必须是 64 位小写十六进制: {value}");
    }
    Ok(())
}

fn validate_hex_bytes(value: &str, bytes: usize, label: &str) -> Result<()> {
    if value.len() != bytes * 2 || hex::decode(value).is_err() {
        bail!("{label} 必须是 {} 字节的十六进制串", bytes);
    }
    Ok(())
}

pub fn validate_sha256_digest(value: &str, label: &str) -> Result<()> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow::anyhow!("{label} 必须以 sha256: 开头: {value}"))?;
    validate_hex64(hex, label)
}

pub fn validate_asset_path(value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') {
        bail!("asset 必须是相对路径且不含反斜杠: {value}");
    }
    for part in value.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            bail!("asset 不能含空段、. 或 ..: {value}");
        }
    }
    Ok(())
}

pub fn validate_timestamp(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        bail!("时间戳必须是 UTC RFC3339（YYYY-MM-DDTHH:MM:SSZ）: {value}");
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        let text = std::str::from_utf8(&bytes[range]).ok()?;
        if !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let (hour, minute, second) = (number(11..13), number(14..16), number(17..19));
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (year, month, day, hour, minute, second)
    else {
        bail!("时间戳含非法数字: {value}");
    };
    if year < 1970
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        bail!("时间戳超出合法范围: {value}");
    }
    Ok(())
}

pub fn validate_date(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        bail!("日期必须是 YYYY-MM-DD: {value}");
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        let text = std::str::from_utf8(&bytes[range]).ok()?;
        if !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let (Some(_year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        bail!("日期含非法数字: {value}");
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        bail!("日期超出合法范围: {value}");
    }
    Ok(())
}
