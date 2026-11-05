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
    /// 合并后的三级菜单（固定一级 + 内置二级 + 插件声明 + 用户偏好）。
    pub menu: crate::menu::PageMenu,
    /// 是否管理员（「设置 → 信任公钥」只对管理员出现）。
    pub is_admin: bool,
    /// 顶栏「插件菜单」：已启用且有 UI 入口的插件（manifest `[[ui]].label` 当菜单名）。
    /// 既用于「插件」二级菜单，也被插件详情页当 `skiff.open` 白名单。
    pub nav_plugins: Vec<PluginNav>,
}

#[derive(Debug, Clone, Template)]
#[template(path = "menu_settings.html")]
pub struct MenuSettingsPage {
    pub ctx: PageCtx,
    pub groups: Vec<crate::menu::SettingsGroup>,
    pub layout: String,
    /// 内容宽度：`auto` / `fixed`。
    pub width: String,
    /// 「固定」时的最大宽度（px）。
    pub max_width: u32,
    pub notice: String,
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
    /// 完整公钥指纹（信息页展示；空串 = 未签名包）。
    pub key_id_full: String,
    pub grants: String,
    /// manifest `requires`（宿主能力面，如 docker/identity）。
    pub requires: String,
    /// manifest `permissions`（动作面，如 fs:read）。
    pub permissions: String,
    pub signed: bool,
    pub reduced: bool,
    pub running: bool,
    /// manifest `[[ui]]` 的第一个入口（相对 `ui/` 的路径）；空串表示无 UI。
    pub ui_path: String,
}

#[derive(Debug, Clone)]
pub struct GrantRow {
    pub kind: String,
    pub value: String,
}

/// 概览「需要处理」的一条待办：按影响排序后渲染，链接直达已筛选的目标页。
#[derive(Debug, Clone)]
pub struct DashboardTodo {
    /// `danger` / `warn` / `info`，同时决定排序与左侧色条。
    pub level: String,
    pub title: String,
    pub detail: String,
    pub href: String,
    pub action: String,
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

#[derive(Debug, Clone)]
pub struct SiteView {
    pub domain: String,
    /// 站点目录（`/www/sites/<域名>`，整站备份单元）。
    pub root_dir: String,
    /// 文档根（`/www/sites/<域名>/index`），目录页与文件能力的授权根。
    pub index_dir: String,
    /// runtime / static / proxy
    pub site_type: String,
    pub type_label: String,
    /// 仅 runtime：绑定的运行环境与展示名。
    pub runtime_id: String,
    pub runtime_label: String,
    /// 仅 proxy：上游地址。
    pub proxy_target: String,
    /// 仅 proxy：是否升级 WebSocket。
    pub proxy_websocket: bool,
    /// 列表里的「类型详情」列：PHP 版本 / 上游 / 静态目录。
    pub endpoint: String,
    /// running / stopped / error
    pub status: String,
    pub status_label: String,
    /// 状态徽标配色（ok / danger / 空），列表与详情共用同一套徽标样式。
    pub status_class: String,
    /// 证书/HTTPS 徽标文案与配色，与运行状态同形呈现。
    pub https_label: String,
    pub https_class: String,
    pub created: String,
    pub is_default: bool,
    /// 附加域名（vhost 的 `server_name` 扩展）。
    pub aliases: Vec<String>,
    /// 展示用逗号串；无别名时为空。
    pub aliases_text: String,
    /// 是否启用 443。
    pub https: bool,
    /// 是否 80 强制跳转 443。
    pub force_https: bool,
}

/// SSL 标签：证书路径、主体、到期与开关状态。
#[derive(Debug, Clone, Default)]
pub struct SiteTlsView {
    pub enabled: bool,
    pub force_https: bool,
    pub cert_path: String,
    pub key_path: String,
    pub cert_exists: bool,
    pub subject: String,
    pub issuer: String,
    pub expires: String,
}

#[derive(Debug, Clone, Default)]
pub struct SiteNewForm {
    /// runtime / static / proxy
    pub site_type: String,
    pub domain: String,
    /// 逐行填写的别名（前端可「新增一行」）。
    pub aliases: Vec<String>,
    /// 批量粘贴的别名原文（换行/逗号/空格混合），提交时与 `aliases` 合并。
    pub aliases_bulk: String,
    /// 站点目录名（只填目录名，`/www/sites/` 前缀由内核自动补全）。
    pub root_dir: String,
    /// 仅 runtime：运行环境 id。
    pub runtime_id: String,
    /// 仅 proxy：上游地址。
    pub proxy_target: String,
    /// 仅 proxy：是否升级 WebSocket。
    pub proxy_websocket: bool,
}

/// 向导第二步可选的一个运行环境。
#[derive(Debug, Clone)]
pub struct RuntimeOptionView {
    pub id: String,
    pub label: String,
    /// P2 预留（Node/Go）在 P1 不可继续。
    pub disabled: bool,
    pub disabled_reason: String,
}

#[derive(Debug, Clone)]
pub struct DirEntryView {
    pub name: String,
    /// 已做 URI 编码的 `path` 查询值。
    pub url: String,
    pub is_dir: bool,
    pub size: String,
    pub modified: String,
}

#[derive(Debug, Clone, Default)]
pub struct DirListingView {
    pub rel: String,
    /// 已做 URI 编码的上级目录查询值。
    pub parent_url: String,
    pub at_root: bool,
    pub entries: Vec<DirEntryView>,
}

#[derive(Template)]
#[template(path = "site_detail.html")]
pub struct SiteDetailPage {
    pub ctx: PageCtx,
    pub site: SiteView,
    pub env: SiteEnvView,
    pub can_admin: bool,
    /// overview / domains / config / dir / ssl
    pub tab: String,
    pub config_path: String,
    pub config_content: String,
    pub config_active: bool,
    pub dir: DirListingView,
    pub ssl: SiteTlsView,
    /// PHP 站点绑定的运行环境未运行：访问会 502/503，页面给出回环境页入口。
    pub runtime_down: bool,
    pub runtime_alert: String,
    pub runtime_env_href: String,
    pub error: String,
    pub notice: String,
}

#[derive(Debug, Clone)]
pub struct SiteEnvView {
    pub docker_available: bool,
    pub docker_reason: String,
    pub nginx_state: String,
    pub php_state: String,
    pub installed: bool,
    pub ready: bool,
    pub port: u16,
    pub nginx_image: String,
    pub php_image: String,
}

#[derive(Template)]
#[template(path = "sites.html")]
pub struct SitesPage {
    pub ctx: PageCtx,
    pub sites: Vec<SiteView>,
    pub env: SiteEnvView,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
    pub keyword: String,
    /// 新建站点弹窗的表单（与服务端校验失败时的回填同源）。
    pub form: SiteNewForm,
    pub runtimes: Vec<RuntimeOptionView>,
    /// 弹窗是否直接展开（服务端校验失败时保持打开）。
    pub form_open: bool,
    pub form_error: String,
    /// 服务端字段级错误：`domain` / `aliases` / `root_dir` / `runtime_id` / `proxy_target`。
    pub form_error_field: String,
    /// 展示用站点根前缀（如 `/www/sites/`），前端只填目录名。
    pub sites_prefix: String,
}

/// 运行环境列表/详情行（业务视角；容器状态只作为漂移提示）。
#[derive(Debug, Clone)]
pub struct RuntimeRow {
    pub id: String,
    pub kind_label: String,
    /// 本地实例名（目录/容器名的唯一来源）。
    pub name: String,
    pub container_name: String,
    pub version: String,
    pub image: String,
    pub state_label: String,
    /// ok / warn / danger / muted，模板据此上色。
    pub state_class: String,
    pub drift: bool,
    pub site_count: usize,
    /// 底层容器的实测状态（Docker 页同源）。
    pub container_state: String,
    /// 列表「接入方式」短标识（sock / :8080），完整路径见 `endpoint_full`。
    pub endpoint_short: String,
    /// 悬浮提示与抽屉里的完整路径。
    pub endpoint_full: String,
    pub implemented: bool,
    pub is_default: bool,
    pub detail_href: String,
    /// 同一底层容器的 Docker 页深链（双向跳转）。
    pub docker_href: String,
    /// 最近一次异步任务（排队/拉镜像/创建容器/启动/失败）。
    pub job_progress: u8,
    pub job_error: String,
    pub job_active: bool,
}

#[derive(Debug, Clone)]
pub struct BoundSiteView {
    pub domain: String,
    pub type_label: String,
    pub status_label: String,
    pub href: String,
}

impl From<&skiff_app::sites::PhpRuntimeConfig> for PhpConfigView {
    fn from(c: &skiff_app::sites::PhpRuntimeConfig) -> Self {
        Self {
            memory_limit: c.memory_limit.clone(),
            upload_max_filesize: c.upload_max_filesize.clone(),
            post_max_size: c.post_max_size.clone(),
            max_execution_time: c.max_execution_time,
            pm: c.pm.clone(),
            max_children: c.max_children,
            start_servers: c.start_servers,
            min_spare_servers: c.min_spare_servers,
            max_spare_servers: c.max_spare_servers,
            max_requests: c.max_requests,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhpConfigView {
    pub memory_limit: String,
    pub upload_max_filesize: String,
    pub post_max_size: String,
    pub max_execution_time: u64,
    pub pm: String,
    pub max_children: u32,
    pub start_servers: u32,
    pub min_spare_servers: u32,
    pub max_spare_servers: u32,
    pub max_requests: u32,
}

/// 新建运行环境弹窗的类型选项（版本来自 `runtime_catalog` 单一来源）。
#[derive(Debug, Clone)]
pub struct RuntimeKindView {
    pub key: String,
    pub label: String,
    pub versions: Vec<String>,
    /// P2 预留类型：可选择登记，但不安装容器。
    pub implemented: bool,
}

/// 抽屉里可勾选/取消的扩展项。
#[derive(Debug, Clone)]
pub struct ExtensionOptionView {
    pub key: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub default_on: bool,
    pub builtin: bool,
}

/// 运行环境异步任务的展示快照（列表/抽屉轮询）。
#[derive(Debug, Clone, Default)]
pub struct RuntimeJobView {
    /// queued / running / done / failed
    pub state: String,
    pub state_label: String,
    /// 当前阶段（拉镜像/创建容器/启动/安装扩展…）。
    pub step: String,
    pub progress: u8,
    pub error: String,
    pub log: String,
    pub active: bool,
    pub failed: bool,
    pub kind: String,
}

#[derive(Template)]
#[template(path = "runtime_detail.html")]
pub struct RuntimeDetailPage {
    pub ctx: PageCtx,
    pub runtime: RuntimeRow,
    pub config: PhpConfigView,
    pub bound_sites: Vec<BoundSiteView>,
    pub socket_host: String,
    pub socket_container: String,
    pub socket_exists: bool,
    pub socket_mode: String,
    /// 扩展清单（单一来源渲染）：勾选状态来自库。
    pub extensions: Vec<ExtensionOptionView>,
    pub extensions_href: String,
    /// 最近一次安装/扩展任务；`has_job` 为 false 时 `job` 是空占位。
    pub has_job: bool,
    pub job: RuntimeJobView,
    pub job_url: String,
    pub logs: String,
    pub logs_error: String,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "site_env.html")]
pub struct SiteEnvPage {
    pub ctx: PageCtx,
    pub env: SiteEnvView,
    pub runtimes: Vec<RuntimeRow>,
    pub can_admin: bool,
    /// 新建弹窗：类型/版本目录（来自 `runtime_catalog`）。
    pub kinds: Vec<RuntimeKindView>,
    /// 同一目录的 JSON 版本，供弹窗按类型重建版本下拉。
    pub catalog_json: String,
    /// 新建弹窗：可选扩展与默认勾选。
    pub extensions: Vec<ExtensionOptionView>,
    /// 列表轮询地址。
    pub jobs_href: String,
    pub add_href: String,
    /// 基础环境安装入口（从标题栏移入弹窗）。
    pub base_install_href: String,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "nginx.html")]
pub struct NginxPage {
    pub ctx: PageCtx,
    pub status: skiff_app::nginx::NginxStatusView,
    pub config: skiff_app::nginx::NginxConfigView,
    pub logs: skiff_app::nginx::NginxLogView,
    pub modules: skiff_app::nginx::NginxModuleView,
    /// status / config / logs / modules
    pub tab: String,
    pub keyword: String,
    /// tail 行数上限（50–500）。
    pub tail: usize,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "site_settings.html")]
pub struct SiteSettingsPage {
    pub ctx: PageCtx,
    pub env: SiteEnvView,
    pub registry_mirror: String,
    pub image_prefix: String,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "site_logs.html")]
pub struct SiteLogsPage {
    pub ctx: PageCtx,
    pub domain: String,
    /// 当前站点的日志目录（`/www/sites/<域名>/log`）。
    pub log_dir: String,
    pub access_log: String,
    pub error_log: String,
    pub keyword: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "site_delete.html")]
pub struct SiteDeletePage {
    pub ctx: PageCtx,
    pub domain: String,
    pub root_dir: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub ctx: PageCtx,
    pub running: usize,
    pub total: usize,
    pub image_count: String,
    pub plugin_total: usize,
    pub plugin_enabled: usize,
    /// 内核「网站」健康块：站点总数 / 非运行站点数（不依赖插件）。
    pub site_total: usize,
    pub site_abnormal: usize,
    /// 运行环境健康：总数 / 异常（未运行、漂移、容器缺失）。
    pub runtime_total: usize,
    pub runtime_abnormal: usize,
    pub abnormal_total: usize,
    /// 最多展示若干条异常容器；完整列表走 `containers?filter=error`。
    pub abnormal: Vec<ContainerRowView>,
    pub uptime: String,
    pub docker_reason: String,
    pub todos: Vec<DashboardTodo>,
    pub audit: Vec<AuditRow>,
    /// `overview.health` 槽位挂载；非空时替换内核默认健康块。
    pub mounts_health: Vec<crate::menu::MountView>,
    /// `overview.actions` / `overview.side` 槽位挂载。
    pub mounts_actions: Vec<crate::menu::MountView>,
    pub mounts_side: Vec<crate::menu::MountView>,
    pub error: String,
}

#[derive(Template)]
#[template(path = "containers.html")]
pub struct ContainersPage {
    pub ctx: PageCtx,
    pub can_operate: bool,
    pub containers: Vec<ContainerRowView>,
    pub docker_reason: String,
    /// 空串表示不筛选；`error` 表示只看异常容器（概览深链用）。
    pub filter: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "containers_rows.html")]
pub struct ContainerRows {
    pub ctx: PageCtx,
    pub can_operate: bool,
    pub containers: Vec<ContainerRowView>,
    pub filter: String,
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
    pub enabled_count: usize,
    pub disabled_count: usize,
    /// `installed` / `discover` / `updates`（页内 tab，服务端渲染，无内联脚本）。
    pub tab: String,
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

/// 插件信息与管理页（内核渲染，`/{p}/plugins/{id}`）。
///
/// 只放「插件是什么、授权了什么、能做什么管理动作、关联哪些菜单」，
/// 不嵌 iframe——功能界面在 `/{p}/plugins/{id}/<view>`（`docs/PLUGIN_UI_CONTRACT.md`）。
#[derive(Template)]
#[template(path = "plugin_info.html")]
pub struct PluginInfoPage {
    pub ctx: PageCtx,
    pub plugin: PluginView,
    pub grants: Vec<GrantRow>,
    pub interop_provides: Vec<String>,
    pub interop_consumes: Vec<String>,
    /// 与导航同源的关联菜单；链接指向 `/{p}/m/plugin/<id>/...` 深链。
    pub menus: Vec<crate::menu::InfoMenuGroup>,
    /// 默认功能页入口（`/{p}/plugins/{id}/<view>`）；无 UI 时为空。
    pub entry_href: String,
    pub can_admin: bool,
    pub error: String,
    pub notice: String,
}

#[derive(Template)]
#[template(path = "plugin_view.html")]
pub struct PluginViewPage {
    pub ctx: PageCtx,
    pub plugin: PluginView,
    /// 返回插件信息页。
    pub info_href: String,
    /// 沙箱 iframe 的 URL（已带 `?nonce=`，跨插件跳转时另带 `&open=`）
    pub iframe_src: String,
    pub rpc_url: String,
    pub nonce: String,
    /// 允许 iframe 请求跳转的目标插件白名单（逗号分隔的 id），供 `skiff.open` 校验
    pub nav_plugins: String,
    /// 工作台标题（声明菜单取 L2 label，否则沿用 `[[ui]].label`）。
    pub workbench_title: String,
    /// L3 标签栏；为空表示该页没有三级菜单（单视图 L2 / 老包）。
    pub tabs: Vec<TabView>,
    /// L2 稳定键 `plugin/<id>/<l2>`；老包为 `plugin/<id>/legacy`。
    pub menu_key: String,
    /// 当前 L3 局部 id；无 L3 时为空，模板按它标记 `aria-current`。
    pub active_l3: String,
    /// 当前传给插件的 `tab` 值。
    pub active_tab: String,
    /// 当前加载的 `ui/` 相对路径（供 JS 判断是否要切换文件）。
    pub active_ui: String,
    pub error: String,
}

/// 工作台 L3 标签（L3 标签栏由内核渲染，插件只渲染 iframe 内容）。
#[derive(Debug, Clone)]
pub struct TabView {
    pub key: String,
    pub local: String,
    pub label: String,
    pub href: String,
    pub icon: String,
    pub tab: String,
    pub ui: String,
    /// 该标签对应的 iframe URL（已带 nonce/menu/tab）。
    pub src: String,
    pub active: bool,
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
    /// 初始口令文件仍在：给一条**非阻塞**的修改建议，不做任何跳转。
    pub initial_admin_pending: bool,
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
