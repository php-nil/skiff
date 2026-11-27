//! 会话与登录：引导数据、壳主题与能力开关、登录页配置与提交体。

#[allow(unused_imports)]
use super::super::*;

/// `GET /ui/v1/session` 的响应数据。
#[derive(Debug, Serialize)]
pub struct SessionData {
    pub username: String,
    pub role: String,
    /// 与页面写操作同源的 CSRF 令牌（`AppState::csrf_token`）。
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
    /// 壳主题（全局）：壳页启动时直接落到 `admin.theme`，刷新后保持。
    pub theme: SessionTheme,
    /// 壳页启动时需要知道的能力开关（字段只增不改）。
    pub features: SessionFeatures,
}

#[derive(Debug, Serialize)]
pub struct SessionTheme {
    /// 校正后的主色（与菜单底色的对比度不低于 4.5:1）。
    pub primary: String,
    pub menu: String,
    pub page: String,
    pub menu_custom: bool,
    pub menu_text: String,
    pub page_text: String,
    pub on_primary: String,
    pub adjusted: bool,
}

#[derive(Debug, Serialize)]
pub struct SessionFeatures {
    /// 页面深链由内核回落壳页（URL 契约 §2.1）。
    pub shell_fallback: bool,
    /// 尚未适配的页面用统一占位呈现。
    pub placeholders: bool,
    /// 旧 SSR 插件页仍以 iframe 承载（仅兼容保留，不再是新前端入口）。
    pub plugin_iframe: bool,
}

/// `GET /ui/v1/auth/login` 的响应数据：登录页渲染前只需要 CSRF 与入口前缀。
#[derive(Debug, Serialize)]
pub struct LoginConfigData {
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
}

/// `POST /ui/v1/auth/login` 的请求体（wire 形状）。
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub otp: String,
    /// 登录页签发的签名 CSRF token（与表单路径同一个）。
    #[serde(default)]
    pub csrf: String,
    /// 登录成功后的回跳深链（只接受站内绝对路径）。
    #[serde(default)]
    pub next: String,
}
