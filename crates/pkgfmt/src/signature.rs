//! ed25519 签名与信任指纹（§4.1、§4.3）。
//!
//! 签名输入固定为：
//! `b"SKIFF-WBPKG-v1\0" || u64be(len(manifest)) || manifest || u64be(len(files.sha256)) || files.sha256`
//! ——长度前缀避免拼接歧义；验签前不得规范化 TOML 或重排清单。

use ed25519_dalek::{Signature, Signer, SigningKey as DalekSigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::{PkgError, Result};

pub const SIG_DOMAIN: &[u8] = b"SKIFF-WBPKG-v1\0";

pub struct SigningKey {
    inner: DalekSigningKey,
}

impl SigningKey {
    pub fn generate() -> Self {
        Self {
            inner: DalekSigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_seed_hex(seed: &str) -> Result<Self> {
        let bytes =
            hex::decode(seed.trim()).map_err(|_| PkgError::Signature("私钥必须是 hex".into()))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| PkgError::Signature("ed25519 私钥种子必须是 32 字节".into()))?;
        Ok(Self {
            inner: DalekSigningKey::from_bytes(&arr),
        })
    }

    pub fn seed_hex(&self) -> String {
        hex::encode(self.inner.to_bytes())
    }

    pub fn public_hex(&self) -> String {
        hex::encode(self.inner.verifying_key().to_bytes())
    }

    /// SHA-256(raw public key) 小写 hex，写入 manifest.key_id。
    pub fn key_id(&self) -> String {
        key_id_of(&self.inner.verifying_key().to_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.inner.sign(msg).to_bytes().to_vec()
    }
}

pub fn key_id_of(public_bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(public_bytes);
    hex::encode(h.finalize())
}

pub fn decode_public_key(public_hex: &str) -> Result<VerifyingKey> {
    let bytes =
        hex::decode(public_hex.trim()).map_err(|_| PkgError::Signature("公钥必须是 hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| PkgError::Signature("ed25519 公钥必须是 32 字节".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| PkgError::Signature(e.to_string()))
}

/// 构造签名输入（内核与 SDK 共用，禁止各自拼接）。
pub fn signing_input(manifest: &[u8], files_list: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIG_DOMAIN.len() + 16 + manifest.len() + files_list.len());
    out.extend_from_slice(SIG_DOMAIN);
    out.extend_from_slice(&(manifest.len() as u64).to_be_bytes());
    out.extend_from_slice(manifest);
    out.extend_from_slice(&(files_list.len() as u64).to_be_bytes());
    out.extend_from_slice(files_list);
    out
}

pub fn verify_bytes(public_hex: &str, msg: &[u8], sig: &[u8]) -> Result<()> {
    let vk = decode_public_key(public_hex)?;
    if sig.len() != 64 {
        return Err(PkgError::Signature(format!(
            "签名长度必须是 64 字节，实际 {}",
            sig.len()
        )));
    }
    let sig = Signature::from_slice(sig).map_err(|e| PkgError::Signature(e.to_string()))?;
    vk.verify_strict(msg, &sig)
        .map_err(|e| PkgError::Signature(format!("ed25519 验签失败: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let k = SigningKey::generate();
        let msg = b"hello";
        let sig = k.sign(msg);
        verify_bytes(&k.public_hex(), msg, &sig).unwrap();
        let mut tampered = msg.to_vec();
        tampered[0] ^= 0xff;
        assert!(verify_bytes(&k.public_hex(), &tampered, &sig).is_err());
    }

    #[test]
    fn key_id_is_sha256_of_public_key() {
        let k = SigningKey::generate();
        let vk = decode_public_key(&k.public_hex()).unwrap();
        assert_eq!(k.key_id(), key_id_of(&vk.to_bytes()));
        assert_eq!(k.key_id().len(), 64);
    }

    #[test]
    fn signing_input_is_unambiguous() {
        // 长度前缀保证 (a="ab", b="c") 与 (a="a", b="bc") 不会撞。
        let a = signing_input(b"ab", b"c");
        let b = signing_input(b"a", b"bc");
        assert_ne!(a, b);
        assert_eq!(&a[..SIG_DOMAIN.len()], SIG_DOMAIN);
    }

    #[test]
    fn seed_roundtrip() {
        let k = SigningKey::generate();
        let k2 = SigningKey::from_seed_hex(&k.seed_hex()).unwrap();
        assert_eq!(k.public_hex(), k2.public_hex());
        assert!(SigningKey::from_seed_hex("bad").is_err());
    }
}
