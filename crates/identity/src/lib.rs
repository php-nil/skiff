//! 认证与授权：口令哈希、会话令牌、TOTP、RBAC（§6.1、§6.3）。
//!
//! 只依赖纯 Rust 密码学实现，与 R1 例外清单一致（无 OpenSSL）。

mod password;
mod session;
mod token;
pub mod totp;

pub use password::{hash_password, verify_password};
pub use session::{
    clear_cookie, session_cookie, session_cookie_name, SessionLimits, SessionTokens,
};
// RBAC 词汇的 canonical 定义在 skiff-model；此处转出仅为兼容，新代码请直接 `use skiff_model::…`。
pub use skiff_model::{Permission, RoleExt};
pub use token::{
    new_token, sha256_hex, signed_token, token_matches, verify_signed_token, SignedTokenError,
};
pub use totp::{
    generate_backup_codes, generate_totp_secret, otpauth_uri, verify_backup_code, verify_totp,
};

// 同上：Role 的 canonical 定义在 skiff-model（经 store 转出保持既有 API）。
pub use skiff_store::Role;
