//! 清单验签：签名对象是 `manifest.json` 的**原始字节**，前缀域固定，
//! `key_id = sha256(公钥)` 小写十六进制。判定规则与服务端
//! `tools/skiff-server/src/sign.rs` 一致，任何不一致都按验签失败处理。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::model::{Manifest, SignatureFile};

pub const SIGNING_DOMAIN: &[u8] = b"SKIFF-SERVER-MANIFEST-v1\0";

/// 内置公钥。仓库**不放真实私钥**，因此这里默认没有可用的内置公钥：
/// 未配置 `catalog.public_key` 时只能接受未签名渠道（且需开发模式显式允许）。
pub const BUILTIN_PUBLIC_KEY: Option<&str> = None;

pub fn parse_public_key(raw: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(raw.trim()).map_err(|_| "公钥必须是十六进制".to_string())?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "公钥必须是 32 字节（64 位十六进制）".to_string())?;
    // 提前拒绝非法曲线点，避免配置写错直到拉取时才报错。
    VerifyingKey::from_bytes(&bytes).map_err(|_| "公钥不是合法 Ed25519 点".to_string())?;
    Ok(bytes)
}

pub fn key_id_of(public_key: &[u8; 32]) -> String {
    hex::encode(Sha256::digest(public_key))
}

pub fn signing_input(manifest_bytes: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(SIGNING_DOMAIN.len() + manifest_bytes.len());
    input.extend_from_slice(SIGNING_DOMAIN);
    input.extend_from_slice(manifest_bytes);
    input
}

/// 校验清单签名。任何一步不符都返回可读原因，调用方必须拒绝使用该清单。
pub fn verify_manifest(
    public_key: &[u8; 32],
    manifest_bytes: &[u8],
    manifest: &Manifest,
    sig: &SignatureFile,
) -> Result<(), String> {
    if sig.alg != "ed25519" {
        return Err(format!("签名算法必须是 ed25519: {}", sig.alg));
    }
    let expected_key_id = key_id_of(public_key);
    if manifest.key_id != expected_key_id {
        return Err(format!(
            "清单 key_id 与固定公钥不匹配（清单 {}，公钥 {}）",
            manifest.key_id, expected_key_id
        ));
    }
    if sig.key_id != manifest.key_id {
        return Err("签名 key_id 与清单 key_id 不一致".into());
    }
    let raw = hex::decode(&sig.sig).map_err(|_| "签名必须是十六进制".to_string())?;
    let signature = Signature::from_slice(&raw).map_err(|_| "签名必须是 64 字节".to_string())?;
    let key =
        VerifyingKey::from_bytes(public_key).map_err(|_| "公钥不是合法 Ed25519 点".to_string())?;
    key.verify(&signing_input(manifest_bytes), &signature)
        .map_err(|_| "Ed25519 验签失败".to_string())
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    pub struct TestKey {
        pub seed: [u8; 32],
        pub public_hex: String,
        key: SigningKey,
    }

    impl TestKey {
        pub fn key_id(&self) -> String {
            key_id_of(&self.key.verifying_key().to_bytes())
        }
    }

    /// 临时密钥对：只活在测试里，仓库不落任何真实私钥。
    pub fn test_key(seed: u8) -> TestKey {
        let seed = [seed; 32];
        let key = SigningKey::from_bytes(&seed);
        TestKey {
            seed,
            public_hex: hex::encode(key.verifying_key().to_bytes()),
            key,
        }
    }

    pub fn sign_hello(k: &TestKey, manifest_bytes: &[u8]) -> String {
        hex::encode(k.key.sign(&signing_input(manifest_bytes)).to_bytes())
    }
}
