//! 插件与商店：插件列表 / 授权 / 菜单声明、商店条目、升级与安装确认。

#[allow(unused_imports)]
use super::super::*;

/// 插件列表的页签与页面骨架开关（`tab` 只切内容块，不换路由）。
#[derive(Debug, Default, Deserialize)]
pub struct PluginListQuery {
    #[serde(default)]
    pub tab: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

/// 插件列表/信息页共用的一条插件投影。
#[derive(Debug, Serialize)]
pub struct PluginItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    pub status: StatusDto,
    pub enabled: bool,
    /// 完整公钥指纹（未签名包为空串）。
    pub key_id: String,
    pub signed: bool,
    pub reduced: bool,
    pub running: bool,
    pub requires: String,
    pub permissions: String,
    pub grants: String,
    pub ui_path: String,
    pub takedown_note: String,
}

#[derive(Debug, Serialize)]
pub struct PluginGrantItem {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct PluginMenuLinkItem {
    pub label: String,
    pub href: String,
    pub icon: String,
}

#[derive(Debug, Serialize)]
pub struct PluginMenuGroupItem {
    pub label: String,
    pub href: String,
    pub icon: String,
    pub children: Vec<PluginMenuLinkItem>,
}

#[derive(Debug, Serialize)]
pub struct PluginInfoData {
    pub plugin: PluginItem,
    pub grants: Vec<PluginGrantItem>,
    pub interop_provides: Vec<String>,
    pub interop_consumes: Vec<String>,
    pub menus: Vec<PluginMenuGroupItem>,
    pub entry_href: String,
    pub can_admin: bool,
    pub allow_unsigned: bool,
    pub reduced_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct PluginDeleteBody {
    #[serde(default)]
    pub delete_data: bool,
}

/// 商店目录的一行（四类条目共用；字段与 `CatalogItem` 同源）。
#[derive(Debug, Serialize)]
pub struct StoreItem {
    pub name: String,
    pub version: String,
    pub source: String,
    pub updated_at: String,
    pub compatibility: String,
    pub note: String,
    pub warn: String,
    pub installable: bool,
    pub action: String,
    pub install_note: String,
}

#[derive(Debug, Serialize)]
pub struct StoreSectionItem {
    pub title: String,
    pub hint: String,
    pub installable: bool,
    pub items: Vec<StoreItem>,
}

#[derive(Debug, Serialize)]
pub struct StoreUpgradeItem {
    pub current: String,
    pub arch: String,
    pub available: String,
    pub channel: String,
    pub min_from: String,
    pub changelog_url: String,
    pub note: String,
    pub can_download: bool,
}

#[derive(Debug, Deserialize)]
pub struct StoreInstallBody {
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PluginConfirmBody {
    pub token: String,
    #[serde(default)]
    pub grant: Vec<String>,
    #[serde(default)]
    pub resource: Vec<String>,
    #[serde(default)]
    pub extra_resource: String,
}
