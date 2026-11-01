//! askama 模板上下文。模板文件在 `crates/web/templates/`（见 `askama.toml`）。

use askama::Template;

/// 所有页面共用的头部上下文。
#[derive(Debug, Clone)]
pub struct PageCtx {
    pub prefix: String,
    pub username: String,
    pub role: String,
    pub csrf: String,
    pub dev_http: bool,
    pub version: String,
    pub docker_ok: bool,
    pub must_change: bool,
    pub active: String,
    /// 顶栏「插件菜单」：已启用且有 UI 入口的插件（manifest `[[ui]].label` 当菜单名）。
    pub nav_plugins: Vec<PluginNav>,
}

/// 顶栏里的一条插件菜单项。
#[derive(Debug, Clone)]
pub struct PluginNav {
    pub id: String,
    pub label: String,
    /// 当前页是否就是这个插件（`active` 记作 `plugin:<id>`）。
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct ContainerRowView {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub project: String,
    pub protected: bool,
}

#[derive(Debug, Clone)]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    pub status: String,
    pub enabled: bool,
    pub key_id: String,
    pub grants: String,
    pub signed: bool,
    pub reduced: bool,
    pub running: bool,
    /// manifest `ui[[ui]]` 的第一个入口（相对 `ui/` 的路径）；空串表示无 UI。
    pub ui_path: String,
    pub ui_label: String,
}

#[derive(Debug, Clone)]
pub struct GrantRow {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub ts: String,
    pub actor: String,
    pub ip: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct KeyRow {
    pub key_id: String,
    pub label: String,
    pub fingerprint_short: String,
}

#[derive(Debug, Clone)]
pub struct ResourceRow {
    pub id: String,
    pub name: String,
    pub meta: String,
    pub warn: String,
}

#[derive(Debug, Clone)]
pub struct MountView {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct PortView {
    pub host: String,
    pub container: String,
    pub protocol: String,
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    pub dev_http: bool,
    pub prefix: String,
    pub error: String,
    pub csrf: String,
    pub username: String,
    pub next: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub ctx: PageCtx,
    pub running: usize,
    pub total: usize,
    pub image_count: String,
    pub plugin_count: usize,
    pub uptime: String,
    pub docker_reason: String,
    pub containers: Vec<ContainerRowView>,
    pub audit: Vec<AuditRow>,
    pub error: String,
}

#[derive(Template)]
#[template(path = "containers.html")]
pub struct ContainersPage {
    pub ctx: PageCtx,
    pub can_operate: bool,
    pub containers: Vec<ContainerRowView>,
    pub docker_reason: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "containers_rows.html")]
pub struct ContainerRows {
    pub ctx: PageCtx,
    pub can_operate: bool,
    pub containers: Vec<ContainerRowView>,
}

#[derive(Template)]
#[template(path = "container_detail.html")]
pub struct ContainerDetailPage {
    pub ctx: PageCtx,
    pub container: ContainerRowView,
    pub image: String,
    pub cmd: String,
    pub created: String,
    pub ports: Vec<PortView>,
    pub mounts: Vec<MountView>,
    pub networks: Vec<String>,
    pub labels: Vec<(String, String)>,
    pub health: String,
    pub can_exec: bool,
    pub can_operate: bool,
    pub docker_reason: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "resources.html")]
pub struct ResourcesPage {
    pub ctx: PageCtx,
    pub section: String,
    pub title: String,
    pub rows: Vec<ResourceRow>,
    pub can_admin: bool,
    pub docker_reason: String,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "plugins.html")]
pub struct PluginsPage {
    pub ctx: PageCtx,
    pub plugins: Vec<PluginView>,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
    pub allow_unsigned: bool,
    pub reduced_allowed: bool,
}

#[derive(Debug, Clone)]
pub struct PermissionRow {
    pub value: String,
    pub capability: String,
    pub verb: String,
}

#[derive(Template)]
#[template(path = "plugin_confirm.html")]
pub struct PluginConfirmPage {
    pub ctx: PageCtx,
    pub token: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: String,
    pub key_id: String,
    pub signed: bool,
    pub permissions: Vec<PermissionRow>,
    /// manifest `resource_hints` + 由 `[interop].consumes` 派生的 `plugin:` 授权：
    /// 安装时逐条确认的对象授权（`path:` / `container:` / `project:` / `plugin:`）
    pub resources: Vec<String>,
    /// 已声明（可被其他插件调用）的接口，每项 `method · label`。
    pub interop_provides: Vec<String>,
    /// 已声明允许调用的目标接口，每项 `<plugin-id>:<method>`。
    pub interop_consumes: Vec<String>,
    pub requires: String,
    pub egress: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "plugin_detail.html")]
pub struct PluginDetailPage {
    pub ctx: PageCtx,
    pub plugin: PluginView,
    /// 沙箱 iframe 的 URL（已带 `?nonce=`，跨插件跳转时另带 `&open=`）
    pub iframe_src: String,
    pub rpc_url: String,
    pub nonce: String,
    /// 允许 iframe 请求跳转的目标插件白名单（逗号分隔的 id），供 `skiff.open` 校验
    pub nav_plugins: String,
    /// 本插件对外提供的接口（`method · label`），空表示不供其他插件调用。
    pub interop_provides: Vec<String>,
    /// 本插件声明的可调用目标（`<plugin-id>:<method>`），空表示不调别的插件。
    pub interop_consumes: Vec<String>,
    pub grants: Vec<GrantRow>,
    pub can_admin: bool,
    pub error: String,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsPage {
    pub ctx: PageCtx,
    pub entry_prefix: String,
    pub totp_enabled: bool,
    pub totp_secret: String,
    pub totp_uri: String,
    pub backup_codes: Vec<String>,
    pub keys: Vec<KeyRow>,
    pub is_admin: bool,
    pub error: String,
    pub notice: String,
    pub force_change: bool,
}

#[derive(Template)]
#[template(path = "audit.html")]
pub struct AuditPage {
    pub ctx: PageCtx,
    pub rows: Vec<AuditRow>,
    pub error: String,
}

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    pub code: u16,
    pub title: String,
    pub message: String,
    pub prefix: String,
}

/// 安全入口前缀的进程级副本。
///
/// 错误页要在拿不到 `AppState` 的场景下也能渲染（中间件提前拒绝、路由 404、
/// 会话过期），而 `IntoResponse` 没有状态入参；前缀在一次启动内是常量，
/// 由 `router()` 组装时写入一次即可。
static ENTRY_PREFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_entry_prefix(p: &str) {
    let _ = ENTRY_PREFIX.set(p.to_string());
}

pub fn entry_prefix() -> &'static str {
    ENTRY_PREFIX.get().map(|s| s.as_str()).unwrap_or("")
}
