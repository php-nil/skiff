//! TOTP（RFC 6238，SHA-1 / 30s / 6 位）与一次性备份码（§6.1）。
//!
//! 只实现验证所需的最小面：密钥生成（base32 展示）、验证（±1 时间步窗口）、
//! otpauth URI（供用户手动录入或自行生成二维码）。

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::token::sha256_hex;

const STEP: u64 = 30;
const DIGITS: u32 = 6;

// ---------------- base32（RFC 4648，无 padding） ----------------

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            let idx = ((buffer >> (bits - 5)) & 0x1f) as usize;
            out.push(ALPHABET[idx] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        let idx = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.chars() {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            '=' | ' ' => continue,
            _ => return None,
        };
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// 20 字节随机密钥，base32 展示（160 bit，RFC 4226 推荐长度）。
pub fn generate_totp_secret() -> String {
    let mut buf = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut buf);
    base32_encode(&buf)
}

fn hotp(secret: &[u8], counter: u64) -> Option<u32> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let code = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    Some(code % 10u32.pow(DIGITS))
}

pub fn totp_now(secret_b32: &str, unix_secs: u64) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    hotp(&secret, unix_secs / STEP).map(|c| format!("{c:06}"))
}

/// 校验，允许前后各一个时间步（时钟漂移容差）。
pub fn verify_totp(secret_b32: &str, code: &str, unix_secs: u64, window: u64) -> bool {
    let code: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(secret) = base32_decode(secret_b32) else {
        return false;
    };
    let counter = unix_secs / STEP;
    let mut ok = false;
    for delta in 0..=window {
        for sign in [1i64, -1i64] {
            if delta == 0 && sign < 0 {
                continue;
            }
            let c = counter as i64 + (delta as i64 * sign);
            if c < 0 {
                continue;
            }
            if let Some(expected) = hotp(&secret, c as u64) {
                let expected = format!("{expected:06}");
                ok |= bool::from(expected.as_bytes().ct_eq(code.as_bytes()));
            }
        }
    }
    ok
}

pub fn otpauth_uri(issuer: &str, account: &str, secret_b32: &str) -> String {
    let label = urlencode(&format!("{issuer}:{account}"));
    let issuer_q = urlencode(issuer);
    format!(
        "otpauth://totp/{label}?secret={secret_b32}&issuer={issuer_q}&algorithm=SHA1&digits=6&period=30"
    )
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------- 备份码 ----------------

fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn hash_code(code: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"skiff-backup-code\0");
    h.update(normalize_code(code).as_bytes());
    hex::encode(h.finalize())
}

/// 生成 10 枚形如 `a1b2-c3d4` 的备份码，返回（明文，哈希）。
/// 明文只在生成时展示一次，库内只存哈希。
pub fn generate_backup_codes(count: usize) -> (Vec<String>, Vec<String>) {
    let mut plain = Vec::with_capacity(count);
    let mut hashes = Vec::with_capacity(count);
    let mut rng = rand::thread_rng();
    for _ in 0..count {
        let mut buf = [0u8; 5];
        rng.fill_bytes(&mut buf);
        let raw = hex::encode(buf); // 10 hex chars
        let code = format!("{}-{}", &raw[..5], &raw[5..]);
        hashes.push(hash_code(&code));
        plain.push(code);
    }
    (plain, hashes)
}

/// 找到匹配的备份码下标（用于标记已用）。无匹配返回 None。
pub fn verify_backup_code(code: &str, hashes: &[String]) -> Option<usize> {
    let h = hash_code(code);
    let mut found = None;
    for (i, stored) in hashes.iter().enumerate() {
        if bool::from(stored.as_bytes().ct_eq(h.as_bytes())) {
            found = Some(i);
        }
    }
    found
}

pub fn hash_backup_code(code: &str) -> String {
    hash_code(code)
}

/// 会话/令牌哈希复用（供上层统一口径）。
pub fn sha256_of(data: &str) -> String {
    sha256_hex(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_roundtrip() {
        let data = b"12345678901234567890";
        let enc = base32_encode(data);
        assert_eq!(enc, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode(&enc).unwrap(), data);
    }

    #[test]
    fn rfc6238_vectors_6_digits() {
        // RFC 6238 附录 B 使用 20 字节 ASCII 密钥；此处截取 6 位结果。
        let secret = base32_encode(b"12345678901234567890");
        assert_eq!(totp_now(&secret, 59).unwrap(), "287082");
        assert_eq!(totp_now(&secret, 1111111109).unwrap(), "081804");
        assert_eq!(totp_now(&secret, 1234567890).unwrap(), "005924");
    }

    #[test]
    fn verify_accepts_window_and_rejects_garbage() {
        let secret = base32_encode(b"12345678901234567890");
        let now = 1_234_567_890u64;
        let code = totp_now(&secret, now).unwrap();
        assert!(verify_totp(&secret, &code, now, 1));
        assert!(verify_totp(&secret, &code, now + 30, 1));
        assert!(!verify_totp(&secret, &code, now + 300, 1));
        assert!(!verify_totp(&secret, "abcdef", now, 1));
        assert!(!verify_totp(&secret, "12345", now, 1));
    }

    #[test]
    fn backup_codes_hash_and_match() {
        let (plain, hashes) = generate_backup_codes(10);
        assert_eq!(plain.len(), 10);
        assert_eq!(hashes.len(), 10);
        assert!(plain[0].contains('-'));
        assert_eq!(verify_backup_code(&plain[3], &hashes), Some(3));
        // 大小写与分隔符不敏感
        let normalized = plain[3].replace('-', "").to_uppercase();
        assert_eq!(verify_backup_code(&normalized, &hashes), Some(3));
        assert_eq!(verify_backup_code("0000-0000", &hashes), None);
    }

    #[test]
    fn otpauth_uri_is_well_formed() {
        let uri = otpauth_uri("Skiff", "admin", "JBSWY3DPEHPK3PXP");
        assert!(uri.starts_with("otpauth://totp/Skiff%3Aadmin?"));
        assert!(uri.contains("secret=JBSWY3DPEHPK3PXP"));
        assert!(uri.contains("issuer=Skiff"));
    }
}
