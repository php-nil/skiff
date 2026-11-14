//! 设置页账户安全读模型：TOTP 开关与待确认 secret/otpauth URI。

use crate::AppState;

/// 账户安全展示数据。
pub struct AccountSecurity {
    pub totp_enabled: bool,
    /// 仅在已有待确认 secret 且尚未启用时非空。
    pub pending_secret: String,
    /// 与 `pending_secret` 同时非空。
    pub pending_uri: String,
}

/// 读取用户账户安全状态；用户不存在返回 `None`。
pub fn account_security(
    state: &AppState,
    user_id: i64,
) -> Result<Option<AccountSecurity>, skiff_store::StoreError> {
    let Some(user) = state.db().user_by_id(user_id)? else {
        return Ok(None);
    };
    let (pending_secret, pending_uri) = match user.totp_secret.as_deref() {
        Some(secret) if !user.totp_enabled => (
            secret.to_string(),
            skiff_identity::otpauth_uri("Skiff", &user.username, secret),
        ),
        _ => (String::new(), String::new()),
    };
    Ok(Some(AccountSecurity {
        totp_enabled: user.totp_enabled,
        pending_secret,
        pending_uri,
    }))
}
