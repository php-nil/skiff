//! 令牌：32 字节随机、base64url 编码；服务端只存 SHA-256（§6.1）。

use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// 带时效签名 token 的校验结果。`Expired` 单独暴露，便于页面给出
/// 「刷新后重试」而不是笼统的 CSRF 失败提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedTokenError {
    Invalid,
    Expired,
}

/// 生成 `<nonce>.<unix_ts>.<hmac>` 形式的签名 token。
///
/// 签名密钥由调用方提供（登录 CSRF 用持久化的 `security.csrf_secret`），
/// 因此服务重启后旧页面仍可校验；时效在 `verify_signed_token` 的 `ttl_secs` 判定。
pub fn signed_token(secret: &str, purpose: &str, now: i64) -> String {
    let nonce = new_token();
    let payload = format!("{nonce}.{now}");
    let sig = sign(secret, purpose, &payload);
    format!("{payload}.{sig}")
}

/// 校验签名与时效。先验签名（恒定时间），再判过期/未来时间。
pub fn verify_signed_token(
    secret: &str,
    purpose: &str,
    token: &str,
    ttl_secs: i64,
    now: i64,
) -> Result<(), SignedTokenError> {
    let mut parts = token.split('.');
    let (Some(nonce), Some(ts), Some(sig), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(SignedTokenError::Invalid);
    };
    if nonce.is_empty() {
        return Err(SignedTokenError::Invalid);
    }
    let ts: i64 = ts.parse().map_err(|_| SignedTokenError::Invalid)?;
    let expected = sign(secret, purpose, &format!("{nonce}.{ts}"));
    if !token_matches(&expected, sig) {
        return Err(SignedTokenError::Invalid);
    }
    if now.saturating_sub(ts) > ttl_secs {
        return Err(SignedTokenError::Expired);
    }
    // 允许 60 秒时钟偏差，超过则视为伪造的未来 token。
    if ts.saturating_sub(now) > 60 {
        return Err(SignedTokenError::Invalid);
    }
    Ok(())
}

fn sign(secret: &str, purpose: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC 接受任意长度密钥");
    mac.update(purpose.as_bytes());
    mac.update(b"|");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// 生成 32 字节随机令牌（base64url，43 字符）。
pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    B64.encode(buf)
}

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    let mut h = Sha256::new();
    h.update(data.as_ref());
    hex::encode(h.finalize())
}

/// 恒定时间字符串比较（长度不同直接 false）。
pub fn token_matches(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_url_safe() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn constant_time_compare() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "abcd"));
    }

    #[test]
    fn signed_token_rejects_tamper_and_wrong_scope() {
        let t = signed_token("secret", "login", 1000);
        assert_eq!(
            verify_signed_token("secret", "login", &t, 600, 1000),
            Ok(())
        );
        assert_eq!(
            verify_signed_token("other", "login", &t, 600, 1000),
            Err(SignedTokenError::Invalid)
        );
        assert_eq!(
            verify_signed_token("secret", "other", &t, 600, 1000),
            Err(SignedTokenError::Invalid)
        );
        let tampered = format!("{}x", &t[..t.len() - 1]);
        assert_eq!(
            verify_signed_token("secret", "login", &tampered, 600, 1000),
            Err(SignedTokenError::Invalid)
        );
    }

    #[test]
    fn signed_token_expires_and_rejects_future_time() {
        let t = signed_token("secret", "login", 1000);
        assert_eq!(
            verify_signed_token("secret", "login", &t, 600, 1600),
            Ok(())
        );
        assert_eq!(
            verify_signed_token("secret", "login", &t, 600, 1601),
            Err(SignedTokenError::Expired)
        );
        let future = signed_token("secret", "login", 2000);
        assert_eq!(
            verify_signed_token("secret", "login", &future, 600, 1000),
            Err(SignedTokenError::Invalid)
        );
    }
}
