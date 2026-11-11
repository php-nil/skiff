//! 请求签名、公钥白名单与 nonce 去重（§17.2）。
//!
//! 身份即公钥：主体标识是 `key_id = sha256(公钥)`；没有账号、会话或 token。
//! 白名单每次请求重读（两个小 JSON），改文件即生效、无需重启；读取失败按 503
//! 失败关闭，绝不放行。

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{path_only, query_only, ApiError, ApiServer};

/// API 请求签名域：与清单签名域（`SKIFF-SERVER-MANIFEST-v1`）分开，避免跨协议重放。
pub const DOMAIN: &[u8] = b"SKIFF-SERVER-API-v1\0";
/// 允许的时钟偏斜（秒）。
pub const MAX_SKEW_SECS: i64 = 300;
const MAX_NONCE_BYTES: usize = 128;
const NONCE_TTL_SECS: i64 = 7200;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorKey {
    pub key_id: String,
    pub public_key: String,
    #[serde(default)]
    pub name: String,
    /// 允许提交的插件 id 前缀（反向域名，如 `com.example`）。
    #[serde(default)]
    pub id_prefixes: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerKey {
    pub key_id: String,
    pub public_key: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorsFile {
    schema: String,
    authors: Vec<AuthorKey>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewersFile {
    schema: String,
    reviewers: Vec<ReviewerKey>,
}

const AUTHORS_SCHEMA: &str = "skiff.server.authors/v1";
const REVIEWERS_SCHEMA: &str = "skiff.server.reviewers/v1";

pub fn load_authors(path: &Path) -> Result<Vec<AuthorKey>> {
    let text = fs::read_to_string(path).with_context(|| format!("读取失败: {}", path.display()))?;
    let file: AuthorsFile =
        serde_json::from_str(&text).with_context(|| format!("解析失败: {}", path.display()))?;
    if file.schema != AUTHORS_SCHEMA {
        bail!("authors.json schema 必须是 {AUTHORS_SCHEMA}");
    }
    for author in &file.authors {
        validate_key(&author.key_id, &author.public_key)?;
        if author.disabled {
            continue;
        }
        if author.id_prefixes.is_empty() {
            bail!(
                "作者 {} 缺少 id_prefixes（无法确定可提交的命名空间）",
                author.key_id
            );
        }
        for prefix in &author.id_prefixes {
            if !valid_prefix(prefix) {
                bail!("作者 {} 的 id_prefixes 非法: {prefix}", author.key_id);
            }
        }
    }
    Ok(file.authors)
}

pub fn load_reviewers(path: &Path) -> Result<Vec<ReviewerKey>> {
    let text = fs::read_to_string(path).with_context(|| format!("读取失败: {}", path.display()))?;
    let file: ReviewersFile =
        serde_json::from_str(&text).with_context(|| format!("解析失败: {}", path.display()))?;
    if file.schema != REVIEWERS_SCHEMA {
        bail!("reviewers.json schema 必须是 {REVIEWERS_SCHEMA}");
    }
    for reviewer in &file.reviewers {
        validate_key(&reviewer.key_id, &reviewer.public_key)?;
    }
    Ok(file.reviewers)
}

fn validate_key(key_id: &str, public_key: &str) -> Result<()> {
    if key_id.len() != 64
        || !key_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("key_id 必须是 64 位小写十六进制");
    }
    let bytes = hex::decode(public_key).context("公钥必须是十六进制")?;
    let public: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("公钥必须是 32 字节"))?;
    VerifyingKey::from_bytes(&public).context("公钥不是合法 Ed25519 点")?;
    let actual = hex::encode(Sha256::digest(public));
    if actual != key_id {
        bail!("key_id {key_id} 与公钥指纹 {actual} 不一致");
    }
    Ok(())
}

fn valid_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 128
        && !prefix.starts_with('.')
        && !prefix.ends_with('.')
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// 通过鉴权的主体。
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub key_id: String,
    pub public_key: String,
    pub name: String,
    pub author: Option<AuthorKey>,
    pub reviewer: bool,
}

impl Authenticated {
    pub fn require_author(&self) -> Result<&AuthorKey, ApiError> {
        self.author.as_ref().ok_or_else(ApiError::forbidden)
    }

    pub fn require_reviewer(&self) -> Result<(), ApiError> {
        if self.reviewer {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
}

/// 校验签名并消费 nonce。任何 401/403 都只回同一个 `forbidden`/`unauthorized` 体。
pub fn authenticate(
    server: &ApiServer,
    request: &crate::http::Request,
    body: &[u8],
) -> Result<Authenticated, ApiError> {
    let key_id = request.header("x-skiff-key-id").unwrap_or("").trim();
    let timestamp = request.header("x-skiff-timestamp").unwrap_or("").trim();
    let nonce = request.header("x-skiff-nonce").unwrap_or("").trim();
    let signature = request.header("x-skiff-signature").unwrap_or("").trim();
    if key_id.is_empty() || timestamp.is_empty() || nonce.is_empty() || signature.is_empty() {
        return Err(ApiError::unauthorized());
    }
    let Ok(ts) = timestamp.parse::<i64>() else {
        return Err(ApiError::unauthorized());
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if (now - ts).abs() > MAX_SKEW_SECS {
        return Err(ApiError::unauthorized());
    }
    if nonce.len() > MAX_NONCE_BYTES
        || !nonce
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '=' | '+' | '/'))
    {
        return Err(ApiError::unauthorized());
    }

    let authors = load_authors(&server.authors).map_err(|_| ApiError::unavailable())?;
    let reviewers = load_reviewers(&server.reviewers).map_err(|_| ApiError::unavailable())?;
    let author = authors
        .iter()
        .find(|a| a.key_id == key_id && !a.disabled)
        .cloned();
    let reviewer = reviewers.iter().find(|r| r.key_id == key_id).cloned();
    if author.is_none() && reviewer.is_none() {
        return Err(ApiError::forbidden());
    }
    let public_key = author
        .as_ref()
        .map(|a| a.public_key.clone())
        .or_else(|| reviewer.as_ref().map(|r| r.public_key.clone()))
        .ok_or_else(ApiError::forbidden)?;
    if let Some(r) = &reviewer {
        if r.public_key != public_key {
            // 同一 key_id 两处公钥不一致属于配置错误，失败关闭。
            return Err(ApiError::unavailable());
        }
    }

    let signed = canonical(
        &request.method,
        path_only(&request.target),
        query_only(&request.target),
        &sha256_hex(body),
        timestamp,
        nonce,
    );
    verify_signature(&public_key, &signed, signature)?;
    if !accept_nonce(&server.state, nonce, ts) {
        return Err(ApiError::unauthorized());
    }

    Ok(Authenticated {
        key_id: key_id.to_string(),
        public_key,
        name: author
            .as_ref()
            .map(|a| a.name.clone())
            .or_else(|| reviewer.as_ref().map(|r| r.name.clone()))
            .unwrap_or_default(),
        author,
        reviewer: reviewer.is_some(),
    })
}

pub fn canonical(
    method: &str,
    path: &str,
    query: &str,
    body_sha256: &str,
    timestamp: &str,
    nonce: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(DOMAIN.len() + 256);
    out.extend_from_slice(DOMAIN);
    out.extend_from_slice(method.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(path.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(query.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(body_sha256.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(timestamp.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(nonce.as_bytes());
    out
}

fn verify_signature(
    public_key_hex: &str,
    message: &[u8],
    signature_hex: &str,
) -> Result<(), ApiError> {
    let public = hex::decode(public_key_hex).map_err(|_| ApiError::unauthorized())?;
    let public: [u8; 32] = public
        .as_slice()
        .try_into()
        .map_err(|_| ApiError::unauthorized())?;
    let key = VerifyingKey::from_bytes(&public).map_err(|_| ApiError::unauthorized())?;
    let sig = hex::decode(signature_hex).map_err(|_| ApiError::unauthorized())?;
    let sig = Signature::from_slice(&sig).map_err(|_| ApiError::unauthorized())?;
    key.verify(message, &sig)
        .map_err(|_| ApiError::unauthorized())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 进程内 single-flight：nonce 检查与写入必须原子，否则两个并发请求可同时通过。
static NONCE_LOCK: Mutex<()> = Mutex::new(());

/// 返回 true 表示新 nonce；false 表示重放。
fn accept_nonce(state: &Path, nonce: &str, ts: i64) -> bool {
    let dir = state.join("nonces");
    let Ok(_guard) = NONCE_LOCK.lock() else {
        return false;
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return false;
    };
    // 机会式清理：删除超过保留期的分片文件。
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .map(|age| age.as_secs() as i64 > NONCE_TTL_SECS)
            .unwrap_or(false)
        {
            let _ = fs::remove_file(&path);
        }
    }
    let hour = chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y%m%d%H").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let path = dir.join(hour);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == nonce) {
        return false;
    }
    let mut line = nonce.to_string();
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()))
        .is_ok()
}
