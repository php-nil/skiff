//! 商店清单签名协议的唯一实现。
//!
//! 背景：内核 `app/catalog/verify.rs` 与发布工具 `tools/skiff-server/src/sign.rs`
//! 曾各写一份 `key_id_of` 与签名输入前缀，靠注释约定一致——这正是 wire 当年
//! SDK/内核双份漂移的同款模式。这里把「域前缀 + key_id + 签名输入」收成唯一来源，
//! 两侧都从这里取值；协议变更只改这一处。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// 签名域前缀（含结尾 NUL）：与其它签名用途隔离，防止跨域复用。
pub const SIGNING_DOMAIN: &[u8] = b"SKIFF-SERVER-MANIFEST-v1\0";

/// `key_id = sha256(公钥 32 字节)` 的小写十六进制。
pub fn key_id_of(public_key: &[u8; 32]) -> String {
    hex::encode(Sha256::digest(public_key))
}

/// 待签名/待验签的字节串：域前缀 + 清单原始字节。
pub fn signing_input(manifest_bytes: &[u8]) -> Vec<u8> {
    let mut input = Vec::with_capacity(SIGNING_DOMAIN.len() + manifest_bytes.len());
    input.extend_from_slice(SIGNING_DOMAIN);
    input.extend_from_slice(manifest_bytes);
    input
}

/// 十六进制公钥 → 32 字节，并提前拒绝非法曲线点与**小阶点**（配置写错时立刻报错）。
pub fn parse_public_key(raw: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(raw.trim()).map_err(|_| "公钥必须是十六进制".to_string())?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "公钥必须是 32 字节（64 位十六进制）".to_string())?;
    let key =
        VerifyingKey::from_bytes(&bytes).map_err(|_| "公钥不是合法 Ed25519 点".to_string())?;
    // 全 0 压缩点能通过曲线方程，但属于小阶点：接受它等于接受可被伪造的密钥。
    if key.is_weak() {
        return Err("公钥是小阶点".to_string());
    }
    Ok(bytes)
}

/// 对 [`signing_input`] 的 detached Ed25519 签名做验签。
pub fn verify_manifest_signature(
    public_key: &[u8; 32],
    manifest_bytes: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    let signature =
        Signature::from_slice(signature).map_err(|_| "签名必须是 64 字节".to_string())?;
    let key =
        VerifyingKey::from_bytes(public_key).map_err(|_| "公钥不是合法 Ed25519 点".to_string())?;
    key.verify(&signing_input(manifest_bytes), &signature)
        .map_err(|_| "Ed25519 验签失败".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_input_is_domain_prefixed() {
        let input = signing_input(b"{}");
        assert_eq!(&input[..SIGNING_DOMAIN.len()], SIGNING_DOMAIN);
        assert_eq!(&input[SIGNING_DOMAIN.len()..], b"{}");
    }

    #[test]
    fn key_id_is_sha256_of_public_bytes() {
        let key = [7u8; 32];
        assert_eq!(key_id_of(&key), hex::encode(Sha256::digest(key)));
        assert_eq!(key_id_of(&key).len(), 64);
    }

    #[test]
    fn public_key_parse_rejects_bad_hex_and_bad_point() {
        assert!(parse_public_key("zz").is_err());
        assert!(parse_public_key("00").is_err());
        // 全 0 不是合法曲线点；合法点用压缩形式的小阶/普通点亦可，这里只验拒绝面。
        assert!(parse_public_key(&"00".repeat(32)).is_err());
    }
}
