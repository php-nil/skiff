//! 会话令牌与 Cookie 形态（§6.1、§6.2）。

use crate::token::{new_token, sha256_hex};

/// 会话过期口径：空闲 30 分钟、绝对 12 小时。改密后撤销全部会话。
#[derive(Debug, Clone, Copy)]
pub struct SessionLimits {
    pub idle_secs: i64,
    pub abs_secs: i64,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            idle_secs: 30 * 60,
            abs_secs: 12 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionTokens {
    pub session_id: String,
    /// 下发给浏览器的 32 字节随机令牌
    pub session_token: String,
    /// 入库的 SHA-256（服务端不存明文）
    pub session_token_hash: String,
    /// CSRF 双提交令牌（随页面下发，写操作回传）
    pub csrf_token: String,
}

impl SessionTokens {
    pub fn new() -> Self {
        let session_token = new_token();
        let session_token_hash = sha256_hex(&session_token);
        Self {
            session_id: new_token()[..22].to_string(),
            session_token,
            session_token_hash,
            csrf_token: new_token(),
        }
    }
}

impl Default for SessionTokens {
    fn default() -> Self {
        Self::new()
    }
}

/// 生产形态用 `__Host-` 前缀（强制 Secure + 无 Domain + Path=/）；
/// 仅 loopback 明文开发模式改用非 `__Host-` 的 cookie 名（§6.2）。
pub fn session_cookie_name(dev_http: bool) -> &'static str {
    if dev_http {
        "skiff_dev_session"
    } else {
        "__Host-skiff_session"
    }
}

pub fn session_cookie(name: &str, token: &str, max_age: i64, secure: bool) -> String {
    let mut c = format!("{name}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}");
    if secure {
        c.push_str("; Secure");
    }
    c
}

pub fn clear_cookie(name: &str, secure: bool) -> String {
    let mut c = format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    if secure {
        c.push_str("; Secure");
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_flags_follow_transport() {
        let c = session_cookie("__Host-skiff_session", "tok", 600, true);
        assert!(c.contains("Secure"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("SameSite=Strict"));
        assert!(c.contains("Path=/"));
        assert!(!session_cookie("skiff_dev_session", "tok", 600, false).contains("Secure"));
        assert_eq!(session_cookie_name(true), "skiff_dev_session");
        assert_eq!(session_cookie_name(false), "__Host-skiff_session");
    }

    #[test]
    fn token_hash_is_stored_not_plaintext() {
        let t = SessionTokens::new();
        assert_ne!(t.session_token, t.session_token_hash);
        assert_eq!(t.session_token_hash, sha256_hex(&t.session_token));
    }
}
