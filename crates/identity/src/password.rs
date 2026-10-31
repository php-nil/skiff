use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

/// argon2id 参数：RFC 9106 第二推荐（m=64MiB, t=3, p=4），见 §6.1。
/// 单次哈希瞬时内存 ~70MB，属预期（§7 登录峰值口径）。
pub const ARGON2_M_COST_KIB: u32 = 64 * 1024;
pub const ARGON2_T_COST: u32 = 3;
pub const ARGON2_P_COST: u32 = 4;

fn hasher() -> Argon2<'static> {
    let params = Params::new(ARGON2_M_COST_KIB, ARGON2_T_COST, ARGON2_P_COST, None)
        .expect("argon2 参数合法");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_password(password: &str) -> Result<String, String> {
    if password.len() < 8 {
        return Err("口令长度至少 8 位".into());
    }
    let salt = SaltString::generate(&mut OsRng);
    hasher()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("口令哈希失败: {e}"))
}

/// 校验口令。`stored` 为 PHC 字符串。恒定时间比较由 argon2 crate 内部完成。
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    // 参数来自库内 PHC 串，允许历史参数差异。
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verify_roundtrip() {
        let h = hash_password("correct-horse-battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(h.contains("m=65536,t=3,p=4"), "参数必须与 §6.1 一致: {h}");
        assert!(verify_password("correct-horse-battery", &h));
        assert!(!verify_password("wrong-password", &h));
        assert!(!verify_password(
            "correct-horse-battery",
            "not-a-phc-string"
        ));
    }

    #[test]
    fn short_password_rejected() {
        assert!(hash_password("short").is_err());
    }
}
