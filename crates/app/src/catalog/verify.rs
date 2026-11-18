//! 清单验签：签名对象是 `manifest.json` 的**原始字节**，前缀域固定，
//! `key_id = sha256(公钥)` 小写十六进制。域前缀、key_id 与签名输入由
//! `skiff-sign` 唯一实现，发布工具与内核共用同一份协议。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use super::model::{Manifest, SignatureFile};

pub use skiff_sign::{key_id_of, parse_public_key, signing_input, SIGNING_DOMAIN};

/// 内置公钥。仓库**不放真实私钥**，因此这里默认没有可用的内置公钥：
/// 未配置 `catalog.public_key` 时只能接受未签名渠道（且需开发模式显式允许）。
pub const BUILTIN_PUBLIC_KEY: Option<&str> = None;

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
