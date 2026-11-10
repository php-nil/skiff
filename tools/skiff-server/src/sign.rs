//! 渠道 manifest 的签名接口。
//!
//! P0 默认使用未签名占位：`key_id` 全零、`sig` 为空串，产物只能本地演练，
//! 客户端与发布门禁必须拒绝。真实签名通过 `--sign-key` 注入 32 字节种子，
//! 签名对象是 manifest.json 的原始字节，前缀域固定。

use anyhow::{Context, Result};
use ed25519_dalek::{Signer as _, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const DOMAIN: &[u8] = b"SKIFF-SERVER-MANIFEST-v1\0";

#[derive(Debug, Clone)]
pub enum Signer {
    Unsigned,
    Ed25519 {
        key: Box<SigningKey>,
        key_id: String,
    },
}

impl Signer {
    /// `seed` 是 32 字节 Ed25519 种子的十六进制；`None` 表示未签名占位。
    pub fn from_seed_hex(seed: Option<&str>) -> Result<Self> {
        let Some(seed) = seed else {
            return Ok(Self::Unsigned);
        };
        let bytes = hex::decode(seed).context("--sign-key 必须是十六进制")?;
        let seed: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("--sign-key 必须是 32 字节（64 位十六进制）"))?;
        let key = Box::new(SigningKey::from_bytes(&seed));
        let key_id = key_id_of(&key.verifying_key());
        Ok(Self::Ed25519 { key, key_id })
    }

    pub fn key_id(&self) -> String {
        match self {
            Self::Unsigned => "0".repeat(64),
            Self::Ed25519 { key_id, .. } => key_id.clone(),
        }
    }

    pub fn is_unsigned(&self) -> bool {
        matches!(self, Self::Unsigned)
    }

    pub fn sign(&self, message: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Unsigned => None,
            Self::Ed25519 { key, .. } => Some(key.sign(message).to_bytes().to_vec()),
        }
    }

    pub fn public_key_hex(&self) -> Option<String> {
        match self {
            Self::Unsigned => None,
            Self::Ed25519 { key, .. } => Some(hex::encode(key.verifying_key().to_bytes())),
        }
    }
}

pub fn manifest_signing_input(manifest_bytes: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(DOMAIN.len() + manifest_bytes.len());
    input.extend_from_slice(DOMAIN);
    input.extend_from_slice(manifest_bytes);
    input
}

pub fn key_id_of(key: &VerifyingKey) -> String {
    hex::encode(Sha256::digest(key.to_bytes()))
}

/// 供测试与发布门禁复核签名使用。
pub fn verify_detached(public_key_hex: &str, message: &[u8], signature: &[u8]) -> Result<()> {
    let public = hex::decode(public_key_hex).context("公钥必须是十六进制")?;
    let public: [u8; 32] = public
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("公钥必须是 32 字节"))?;
    let key = VerifyingKey::from_bytes(&public).context("公钥不是合法 Ed25519 点")?;
    let signature = ed25519_dalek::Signature::from_slice(signature)
        .map_err(|_| anyhow::anyhow!("签名必须是 64 字节"))?;
    key.verify(message, &signature)
        .map_err(|_| anyhow::anyhow!("Ed25519 验签失败"))
}

pub fn is_placeholder_key(key_id: &str) -> bool {
    if key_id.len() != 64 {
        return true;
    }
    key_id.chars().all(|c| c == '0')
}
