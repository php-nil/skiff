//! 客户端侧的 Skiff-server 清单模型（`docs/SKIFF_SERVER.md` P1）。
//!
//! 只声明商店页要展示与校验的字段：服务端新增的字段被忽略（同一 schema 版本内
//! 向前兼容），但摘要/URL/大小这类**安全字段**一个都不能缺——缺了就无法做内容
//! 寻址校验。字段口径与服务端 `tools/skiff-server/src/model.rs` 保持一致。

use std::path::Path;

use serde::Deserialize;

pub const MANIFEST_SCHEMA: &str = "skiff.server.manifest/v1";
pub const INDEX_SCHEMA: &str = "skiff.server.index/v1";
pub const BLOB_PREFIX: &str = "blobs/sha256/";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub channel: String,
    pub seq: u64,
    pub generated_at: String,
    pub key_id: String,
    pub index: IndexRef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexRef {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// `manifest.json.sig`；缺文件按未签名处理，`sig` 为空也按未签名处理。
#[derive(Debug, Clone, Deserialize)]
pub struct SignatureFile {
    pub alg: String,
    pub key_id: String,
    pub sig: String,
}

impl SignatureFile {
    pub fn is_placeholder(&self) -> bool {
        self.sig.is_empty() || self.sig.len() != 128 || self.key_id.chars().all(|c| c == '0')
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    pub schema: String,
    pub channel: String,
    pub seq: u64,
    pub generated_at: String,
    #[serde(default)]
    pub plugins: Vec<PluginEntry>,
    #[serde(default)]
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub runtimes: Vec<RuntimeEntry>,
    #[serde(default)]
    pub upgrades: Vec<UpgradeEntry>,
    #[serde(default)]
    pub revoked: Vec<RevokedEntry>,
    /// 应用模板的吊销命名空间（R2e）；`revoked` 继续只表示插件。
    #[serde(default)]
    pub revoked_apps: Vec<RevokedEntry>,
    /// 运行环境的吊销命名空间（R2e）。
    #[serde(default)]
    pub revoked_runtimes: Vec<RevokedEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginEntry {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub api: String,
    #[serde(default)]
    pub requires: Vec<String>,
    pub summary: String,
    pub published_at: String,
    #[serde(default)]
    pub yanked: bool,
    /// 撤包原因（R1 新增可选字段；旧索引没有该字段，旧客户端忽略未知字段）。
    #[serde(default)]
    pub yank_reason: Option<String>,
    pub pkg_sha256: Option<String>,
    pub pkg_url: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub version: String,
    pub summary: String,
    pub min_kernel: String,
    pub compose_sha256: Option<String>,
    pub compose_url: Option<String>,
    pub size: Option<u64>,
    /// 单版本下架（R2e 可选字段；旧索引没有该字段）。
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub yank_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeEntry {
    pub kind: String,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    pub eol: Option<String>,
    pub api_min: String,
    #[serde(default)]
    pub default: bool,
    /// 单版本下架（R2e 可选字段；旧索引没有该字段）。
    #[serde(default)]
    pub yanked: bool,
    #[serde(default)]
    pub yank_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpgradeEntry {
    pub version: String,
    pub arch: String,
    pub channel: String,
    pub min_from: String,
    pub changelog_url: String,
    pub pkg_sha256: Option<String>,
    pub pkg_url: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RevokedEntry {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub sha256: Option<String>,
    pub reason: String,
    pub at: String,
}

impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(format!("manifest schema 非法: {}", self.schema));
        }
        validate_channel(&self.channel)?;
        if self.seq == 0 {
            return Err("manifest seq 必须从 1 开始".into());
        }
        validate_timestamp(&self.generated_at)?;
        validate_hex64(&self.key_id, "manifest key_id")?;
        if self.index.path != "index.json" {
            return Err(format!(
                "manifest.index.path 非 index.json: {}",
                self.index.path
            ));
        }
        validate_hex64(&self.index.sha256, "index 摘要")
    }
}

impl Index {
    pub fn validate(&self, blob_dir: Option<&Path>) -> Result<(), String> {
        if self.schema != INDEX_SCHEMA {
            return Err(format!("索引 schema 非法: {}", self.schema));
        }
        validate_channel(&self.channel)?;
        if self.seq == 0 {
            return Err("索引 seq 必须从 1 开始".into());
        }
        validate_timestamp(&self.generated_at)?;
        for entry in &self.plugins {
            validate_semver(&entry.version, "插件版本")?;
            validate_summary(&entry.summary, "插件 summary")?;
            validate_timestamp(&entry.published_at)?;
            check_package(
                &format!("插件 {}", entry.id),
                entry.pkg_sha256.as_deref(),
                entry.pkg_url.as_deref(),
                entry.size,
                blob_dir,
            )?;
        }
        for entry in &self.apps {
            validate_semver(&entry.version, "应用版本")?;
            validate_semver(&entry.min_kernel, "应用 min_kernel")?;
            validate_summary(&entry.summary, "应用 summary")?;
            check_package(
                &format!("应用 {}", entry.id),
                entry.compose_sha256.as_deref(),
                entry.compose_url.as_deref(),
                entry.size,
                blob_dir,
            )?;
        }
        for entry in &self.runtimes {
            if !["php", "node", "rust"].contains(&entry.kind.as_str()) {
                return Err(format!("运行环境 kind 非法: {}", entry.kind));
            }
            validate_plain_version(&entry.version, "运行环境版本")?;
            if entry.image.trim().is_empty() {
                return Err("运行环境镜像不能为空".into());
            }
            validate_sha256_digest(&entry.image_digest, "运行环境镜像 digest")?;
            validate_api(&entry.api_min)?;
            if let Some(eol) = &entry.eol {
                if eol.len() != 10 {
                    return Err(format!("运行环境 eol 必须是 YYYY-MM-DD: {eol}"));
                }
            }
        }
        for entry in &self.upgrades {
            validate_semver(&entry.version, "升级版本")?;
            if entry.arch != "x86_64" && entry.arch != "aarch64" {
                return Err(format!("升级架构非法: {}", entry.arch));
            }
            validate_semver(&entry.min_from, "升级 min_from")?;
            if !entry.changelog_url.starts_with("https://") {
                return Err(format!(
                    "changelog_url 必须是 https: {}",
                    entry.changelog_url
                ));
            }
            check_package(
                &format!("升级条目 {}", entry.version),
                entry.pkg_sha256.as_deref(),
                entry.pkg_url.as_deref(),
                entry.size,
                blob_dir,
            )?;
        }
        for entry in &self.revoked {
            validate_timestamp(&entry.at)?;
            validate_summary(&entry.reason, "吊销原因")?;
        }
        for entry in &self.revoked_apps {
            validate_timestamp(&entry.at)?;
            validate_summary(&entry.reason, "应用吊销原因")?;
        }
        for entry in &self.revoked_runtimes {
            validate_timestamp(&entry.at)?;
            validate_summary(&entry.reason, "运行环境吊销原因")?;
        }
        Ok(())
    }

    /// 该 `(id, version)` 是否已被服务端吊销。
    pub fn revoked(&self, id: &str, version: &str) -> Option<&RevokedEntry> {
        self.revoked
            .iter()
            .find(|r| r.id == id && r.version == version)
    }

    /// 应用模板的吊销命中（R2e 独立命名空间）。
    pub fn revoked_app(&self, id: &str, version: &str) -> Option<&RevokedEntry> {
        self.revoked_apps
            .iter()
            .find(|r| r.id == id && r.version == version)
    }

    /// 运行环境的吊销命中（`id` 是 `kind`）。
    pub fn revoked_runtime(&self, kind: &str, version: &str) -> Option<&RevokedEntry> {
        self.revoked_runtimes
            .iter()
            .find(|r| r.id == kind && r.version == version)
    }
}

/// 摘要字段三件套：摘要、内容寻址 URL、字节数必须同时存在且互相吻合。
/// `blob_dir` 是本地 `blobs/sha256/` 目录；只有 blob 已在本地时才复核哈希与大小。
fn check_package(
    label: &str,
    sha256: Option<&str>,
    url: Option<&str>,
    size: Option<u64>,
    blob_dir: Option<&Path>,
) -> Result<(), String> {
    let sha = sha256.ok_or_else(|| format!("{label} 缺少内容摘要"))?;
    validate_hex64(sha, "内容摘要").map_err(|e| format!("{label}: {e}"))?;
    let url = url.ok_or_else(|| format!("{label} 缺少内容寻址 URL"))?;
    let expected = format!("{BLOB_PREFIX}{sha}");
    if url != expected {
        return Err(format!("{label} 的内容寻址 URL 与摘要不一致: {url}"));
    }
    let size = size.ok_or_else(|| format!("{label} 缺少字节数"))?;
    let Some(dir) = blob_dir else {
        return Ok(());
    };
    let path = dir.join(sha);
    if !path.is_file() {
        return Ok(());
    }
    verify_local_blob(&path, sha, size).map_err(|e| format!("{label}: {e}"))
}

/// 本地已有 blob 时复核大小与 sha256：目录缓存被截断/替换必须当场拒绝。
fn verify_local_blob(path: &Path, sha256: &str, size: u64) -> Result<(), String> {
    use sha2::{Digest, Sha256};

    let bytes = std::fs::read(path).map_err(|e| format!("读取本地 blob 失败: {e}"))?;
    if bytes.len() as u64 != size {
        return Err(format!(
            "摘要与 blob 大小不符：清单 {size} 字节，本地 {} 字节",
            bytes.len()
        ));
    }
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != sha256 {
        return Err(format!("blob 摘要不符：清单 {sha256}，本地 {actual}"));
    }
    Ok(())
}

pub fn validate_channel(value: &str) -> Result<(), String> {
    if value != "stable" && value != "night" {
        return Err(format!("未知渠道: {value}（只允许 stable/night）"));
    }
    Ok(())
}

pub fn validate_hex64(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!("{label} 必须是 64 位小写十六进制: {value}"));
    }
    Ok(())
}

fn validate_sha256_digest(value: &str, label: &str) -> Result<(), String> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| format!("{label} 必须以 sha256: 开头: {value}"))?;
    validate_hex64(hex, label)
}

fn validate_semver(value: &str, label: &str) -> Result<(), String> {
    let core = value.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!("{label} 必须是 x.y.z: {value}"));
    }
    Ok(())
}

fn validate_plain_version(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 32 || value.contains(char::is_whitespace) {
        return Err(format!("{label} 必须是 1..=32 个非空白字符: {value}"));
    }
    Ok(())
}

fn validate_api(value: &str) -> Result<(), String> {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 2
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!("api 必须是 主版本.次版本: {value}"));
    }
    Ok(())
}

fn validate_summary(value: &str, label: &str) -> Result<(), String> {
    let len = value.chars().count();
    if len == 0 || len > 200 {
        return Err(format!("{label} 长度必须在 1..=200"));
    }
    Ok(())
}

fn validate_timestamp(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return Err(format!(
            "时间戳必须是 UTC RFC3339（YYYY-MM-DDTHH:MM:SSZ）: {value}"
        ));
    }
    let digits = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&bytes[range])
            .ok()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    let (year, month, day) = (digits(0..4), digits(5..7), digits(8..10));
    let (hour, minute, second) = (digits(11..13), digits(14..16), digits(17..19));
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (year, month, day, hour, minute, second)
    else {
        return Err(format!("时间戳含非法数字: {value}"));
    };
    if year < 1970
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(format!("时间戳超出合法范围: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(sha: &str, size: u64) -> PluginEntry {
        PluginEntry {
            id: "com.example.hello".into(),
            version: "1.0.0".into(),
            api: "1.0".into(),
            requires: vec!["docker".into()],
            summary: "示例插件".into(),
            published_at: "2026-11-10T00:00:00Z".into(),
            yanked: false,
            yank_reason: None,
            pkg_sha256: Some(sha.into()),
            pkg_url: Some(format!("{BLOB_PREFIX}{sha}")),
            size: Some(size),
        }
    }

    fn index_with(entry: PluginEntry) -> Index {
        Index {
            schema: INDEX_SCHEMA.into(),
            channel: "stable".into(),
            seq: 1,
            generated_at: "2026-11-10T00:00:00Z".into(),
            plugins: vec![entry],
            apps: Vec::new(),
            runtimes: Vec::new(),
            upgrades: Vec::new(),
            revoked: Vec::new(),
            revoked_apps: Vec::new(),
            revoked_runtimes: Vec::new(),
        }
    }

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn rejects_url_digest_mismatch() {
        let mut entry = plugin(SHA, 3);
        entry.pkg_url = Some(format!("{BLOB_PREFIX}{}", "b".repeat(64)));
        let err = index_with(entry).validate(None).unwrap_err();
        assert!(err.contains("内容寻址 URL 与摘要不一致"), "{err}");
    }

    #[test]
    fn rejects_local_blob_size_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SHA), b"hello").unwrap();
        let err = index_with(plugin(SHA, 3))
            .validate(Some(dir.path()))
            .unwrap_err();
        assert!(err.contains("摘要与 blob 大小不符"), "{err}");
    }

    #[test]
    fn rejects_local_blob_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SHA), b"hello").unwrap();
        // 大小一致但内容不同：必须按摘要拒绝
        let err = index_with(plugin(SHA, 5))
            .validate(Some(dir.path()))
            .unwrap_err();
        assert!(err.contains("blob 摘要不符"), "{err}");
    }

    #[test]
    fn accepts_matching_local_blob() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let body = b"hello";
        let sha = hex::encode(Sha256::digest(body));
        std::fs::write(dir.path().join(&sha), body).unwrap();
        index_with(plugin(&sha, body.len() as u64))
            .validate(Some(dir.path()))
            .unwrap();
    }

    #[test]
    fn rejects_manifest_schema_and_seq() {
        let mut m = Manifest {
            schema: MANIFEST_SCHEMA.into(),
            channel: "stable".into(),
            seq: 1,
            generated_at: "2026-11-10T00:00:00Z".into(),
            key_id: "0".repeat(64),
            index: IndexRef {
                path: "index.json".into(),
                sha256: SHA.into(),
                size: 0,
            },
        };
        m.validate().unwrap();
        m.seq = 0;
        assert!(m.validate().is_err());
        m.seq = 1;
        m.schema = "skiff.server.manifest/v2".into();
        assert!(m.validate().is_err());
    }
}
