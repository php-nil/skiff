pub use skiff_model::{Grant, Role};

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub totp_secret: Option<String>,
    pub totp_enabled: bool,
    pub must_change_password: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: i64,
    pub username: String,
    pub role: Role,
    pub created_at: i64,
    pub last_seen: i64,
    pub idle_expires: i64,
    pub abs_expires: i64,
    pub ip: String,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub id: i64,
    pub ts: i64,
    pub actor: String,
    pub ip: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    pub key_id: String,
    /// 包内 `manifest.toml` 原始字节解码后的文本（签名输入的原始来源）。
    pub manifest: String,
    pub enabled: bool,
    /// installed / enabled / disabled / error
    pub status: String,
    pub status_detail: String,
    pub active_version: String,
    pub installed_at: i64,
    pub grants: Vec<Grant>,
}
