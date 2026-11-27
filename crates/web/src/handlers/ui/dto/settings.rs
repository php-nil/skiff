//! 设置域：面板项、账号与密钥、菜单设置、Docker 设置、主题、改密与 TOTP。

#[allow(unused_imports)]
use super::super::*;

/// 设置页「后台端口 / 安全入口 / TLS」区块（字段冻结，只增不改）。
#[derive(Debug, Serialize)]
pub struct SettingsPanelItem {
    pub listen_addr: String,
    pub listen_port: u16,
    pub entry_prefix: String,
    /// `auto` / `on` / `off`，与旧页面三态回显同口径。
    pub tls_mode: String,
    pub tls_cert: String,
    pub tls_key: String,
    pub config_path: String,
    pub plaintext_behind_proxy: bool,
    /// 首启口令文件仍在：提示尽快改密（只提示，不拦截）。
    pub initial_admin_pending: bool,
}

/// 设置页账户安全区块（口令 / TOTP 表单仍在旧版设置页）。
#[derive(Debug, Serialize)]
pub struct SettingsAccountItem {
    pub totp_enabled: bool,
    pub must_change_password: bool,
    /// 已生成但未启用的 TOTP 密钥（仅本人可见；空串表示没有待启用的密钥）。
    pub totp_pending_secret: String,
    /// 待启用密钥的 otpauth URI（与 secret 同时为空/非空）。
    pub totp_pending_uri: String,
}

/// 信任公钥行（仅管理员可见，非管理员返回空数组）。
#[derive(Debug, Serialize)]
pub struct SettingsKeyItem {
    pub key_id: String,
    pub fingerprint_short: String,
    pub label: String,
}

/// 设置页一次性提示（本次读取消费，刷新不重放）。
#[derive(Debug, Serialize)]
pub struct SettingsFlashItem {
    pub notice: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsData {
    pub panel: SettingsPanelItem,
    pub account: SettingsAccountItem,
    pub keys: Vec<SettingsKeyItem>,
    pub can_admin: bool,
    pub flash: SettingsFlashItem,
}

#[derive(Debug, Deserialize)]
pub struct SettingsPanelBody {
    #[serde(default)]
    pub listen_addr: String,
    #[serde(default)]
    pub listen_port: String,
    #[serde(default)]
    pub entry_prefix: String,
    #[serde(default)]
    pub tls_mode: String,
    #[serde(default)]
    pub tls_cert: String,
    #[serde(default)]
    pub tls_key: String,
}

#[derive(Debug, Serialize)]
pub struct MenuSettingsItem {
    pub key: String,
    pub label: String,
    pub href: String,
    pub hidden: bool,
    pub pinned: bool,
    pub pinnable: bool,
    pub plugin: bool,
    pub icon: String,
}

#[derive(Debug, Serialize)]
pub struct MenuSettingsGroup {
    pub key: String,
    pub label: String,
    pub items: Vec<MenuSettingsItem>,
}

#[derive(Debug, Serialize)]
pub struct MenuSettingsData {
    pub groups: Vec<MenuSettingsGroup>,
}

#[derive(Debug, Deserialize)]
pub struct MenuUpdateBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct DockerSettingsData {
    pub registry_mirror: String,
    pub image_prefix: String,
    /// 运行环境版本清单的**生效地址**（只读）：固定为 `<镜像基址>/channels/<channel>/runtime-index/php.json`，
    /// 基址取 `[catalog].mirrors` 首项；为空表示未配置镜像源，只用内置副本。
    pub runtime_index_url: String,
    pub can_admin: bool,
}

/// `GET /ui/v1/settings/ui`：壳主题快照（全局生效）。
#[derive(Debug, Serialize)]
pub struct UiThemeData {
    pub primary: String,
    pub menu: String,
    pub page: String,
    pub menu_custom: bool,
    pub menu_text: String,
    pub page_text: String,
    pub on_primary: String,
    pub adjusted: bool,
    pub can_admin: bool,
}

/// `POST /ui/v1/settings/ui` 的请求体。
#[derive(Debug, Deserialize)]
pub struct UiThemeUpdateBody {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub menu: String,
    #[serde(default)]
    pub page: String,
    #[serde(default)]
    pub menu_custom: bool,
}

#[derive(Debug, Deserialize)]
pub struct DockerSettingsBody {
    #[serde(default)]
    pub registry_mirror: String,
    #[serde(default)]
    pub image_prefix: String,
}

#[derive(Debug, Deserialize)]
pub struct PasswordBody {
    #[serde(default)]
    pub old_password: String,
    #[serde(default)]
    pub new_password: String,
    #[serde(default)]
    pub confirm_password: String,
}

#[derive(Debug, Deserialize)]
pub struct TotpEnableBody {
    #[serde(default)]
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct KeyImportBody {
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub label: String,
}
