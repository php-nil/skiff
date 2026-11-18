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
    pub menu: skiff_app::menu::PageMenu,
    /// 是否管理员（「设置 → 信任公钥」只对管理员出现）。
    pub is_admin: bool,
    /// 顶栏「插件菜单」：已启用且有 UI 入口的插件（manifest `[[ui]].label` 当菜单名）。
    /// 既用于「插件」二级菜单，也被插件详情页当 `skiff.open` 白名单。
    pub nav_plugins: Vec<PluginNav>,
    /// 服务端一次性提示：本次渲染消费，用于写操作后的反馈。
    pub flash_notice: String,
    pub flash_error: String,
}

/// 内核升级只读段（P3）：当前版本 vs 渠道可用版本，手动下载入口。
#[derive(Debug, Clone, Default)]
pub struct UpgradeHint {
    pub current: String,
    pub arch: String,
    /// 渠道里高于当前版本的可用版本；空表示「无可用升级」。
    pub available: String,
    pub channel: String,
    pub min_from: String,
    pub changelog_url: String,
    /// 空态/不可下载时的一句提示。
    pub note: String,
    /// 是否显示下载按钮（有可用版本、公钥已配置且目录未被拒）。
    pub can_download: bool,
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
    /// 内核托管但缺少保护标签的存量容器（页面提示需重建后生效）。
    pub unprotected_kernel: bool,
}

#[derive(Debug, Clone)]
pub struct PluginView {
    pub id: String,
    pub name: String,
    pub running: bool,
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
    pub action: String,
    pub target: String,
    pub result: String,
}

#[derive(Debug, Clone)]
pub struct KeyRow {
    pub key_id: String,
    pub label: String,
    pub fingerprint_short: String,
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
    /// 列表合并列用的运行环境实例名（如 `php-8.5`）；运行环境记录缺失时为空。
    pub runtime_name: String,
    /// 共享环境提示：绑定站点数 > 0 时说明重启/升级/改端口影响全部。
    pub runtime_shared_hint: String,
    /// 仅 proxy：上游地址。
    pub proxy_target: String,
    /// 仅 proxy：是否升级 WebSocket。
    pub proxy_websocket: bool,
    /// running / stopped / error
    pub status: String,
    pub status_label: String,
    /// 状态徽标配色（ok / danger / 空），列表与详情共用同一套徽标样式。
    pub status_class: String,
    /// 巡检判定不可用的原因；空串表示正常。非空时状态列为「维护中」。
    pub maintenance_reason: String,
    /// 证书/HTTPS 徽标文案与配色，与运行状态同形呈现。
    pub https_label: String,
    pub https_class: String,
    pub created: String,
    pub is_default: bool,
    /// 附加域名（vhost 的 `server_name` 扩展）。
    pub aliases: Vec<String>,
    /// 展示用逗号串；无别名时为空。
    pub aliases_text: String,
    /// 列表页别名预览（最多两个）；无别名时为空，模板据此整行不渲染。
    pub alias_preview: String,
    /// 预览之外的别名数量，用于 `+N` 提示。
    pub alias_more: usize,
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

/// 向导第二步可选的一个运行环境（只含已就绪实例）。
#[derive(Debug, Clone)]
pub struct RuntimeOptionView {
    pub id: String,
    pub label: String,
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
    /// 站点备份开关（内核 `site-backup` 任务在该站点的投影）。
    pub backup: SiteBackupView,
    pub error: String,
    pub notice: String,
}

/// 站点详情页的备份区块。`task_id` 为空表示尚未建立任务（关闭态）。
#[derive(Debug, Clone, Default)]
pub struct SiteBackupView {
    pub enabled: bool,
    pub task_id: String,
    pub last_text: String,
    pub next_text: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SiteEnvView {
    pub docker_available: bool,
    pub docker_reason: String,
    pub nginx_state: String,
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
#[allow(dead_code)]
pub struct RuntimeRow {
    pub id: String,
    /// php / node / go / rust，模板按类型分支。
    pub kind_key: String,
    pub kind_label: String,
    /// 本地实例名（目录/容器名的唯一来源）。
    pub name: String,
    pub container_name: String,
    /// 安装目标版本（如 8.5）或运行中读到的真实版本（如 8.5.12）。
    pub version: String,
    /// 同上，已按运行状态决定展示值。
    pub version_display: String,
    /// `目标 8.5 · php:8.5-fpm-alpine` / `未安装` / `版本未读取`。
    pub version_note: String,
    /// 镜像元数据里读到的版本（可能为空）。
    pub actual_version: String,
    /// 容器是否存在（停止但存在也算存在；缺失才是「未安装」）。
    pub container_exists: bool,
    pub image: String,
    /// 容器实际镜像 id（短值）与 digest 短值，详情抽屉展示。
    pub image_id: String,
    pub image_id_full: String,
    pub image_digest: String,
    pub image_digest_full: String,
    /// 登记镜像与容器镜像不一致的提示。
    pub image_mismatch: bool,
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
    /// Rust 固定端口；其他类型为空。
    pub port: String,
    /// 容器地址（`ip:port`）；未分配时为空。
    pub container_address: String,
    /// 对外发布宿主端口；空表示默认不公开。
    pub publish_port: String,
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

/// 抽屉里可勾选/取消的扩展项。
#[derive(Debug, Clone)]
pub struct ExtensionOptionView {
    pub key: String,
    pub label: String,
    pub description: String,
    pub enabled: bool,
    pub default_on: bool,
    pub builtin: bool,
    /// 数据库 / 图片 / 加密 / 缓存 / 其他（来自单一来源目录）。
    pub category: String,
    /// 镜像内置 / 源码内置 / PECL。
    pub source: String,
    /// 支持热装（安装后重启容器生效）。
    pub hot: bool,
    /// 编译较重，确认弹窗标预计耗时。
    pub heavy: bool,
    /// 重项预计耗时文案；轻量项为空。
    pub estimated_time: String,
    /// 依赖提示（「将临时安装 X/Y」）；不可用项不进入清单。
    pub support_note: String,
}

#[derive(Template)]
#[template(path = "nginx.html")]
pub struct NginxPage {
    pub ctx: PageCtx,
    pub status: skiff_app::nginx::NginxStatusView,
    pub config: skiff_app::nginx::NginxConfigView,
    pub logs: skiff_app::nginx::NginxLogView,
    pub modules: skiff_app::nginx::NginxModuleView,
    /// Web 服务容器安装进度（安装/重建时的分步状态）。
    pub install: skiff_app::sites::InstallProgress,
    /// status / config / logs / modules
    pub tab: String,
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
    /// 目录体积（人类可读）。
    pub dir_size: String,
    pub file_count: u64,
    pub task_count: usize,
    /// 绑定的运行环境（kind + name）；无绑定为空串。
    pub runtime_label: String,
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
    /// 尚未安装（没有容器）的运行环境数量；不计入异常，但也不能说成「全部运行中」。
    pub runtime_uninstalled: usize,
    pub abnormal_total: usize,
    /// 最多展示若干条异常容器；完整列表走 `containers?filter=error`。
    pub abnormal: Vec<ContainerRowView>,
    pub uptime: String,
    pub docker_reason: String,
    pub todos: Vec<DashboardTodo>,
    pub audit: Vec<AuditRow>,
    /// `overview.health` 槽位挂载；非空时替换内核默认健康块。
    pub mounts_health: Vec<skiff_app::menu::MountView>,
    /// `overview.actions` / `overview.side` 槽位挂载。
    pub mounts_actions: Vec<skiff_app::menu::MountView>,
    pub mounts_side: Vec<skiff_app::menu::MountView>,
    pub error: String,
}

#[derive(Template)]
#[template(path = "containers_rows.html")]
pub struct ContainerRows {
    pub ctx: PageCtx,
    pub can_operate: bool,
    pub can_delete: bool,
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
    pub can_delete: bool,
    pub docker_reason: String,
    pub error: String,
    pub limits: ResourceLimitsForm,
    pub limits_effective: Vec<(String, String)>,
}

/// 资源限制表单的原始输入值（空串 = Engine 默认 / 不限制，由页面文案解释）。
#[derive(Debug, Clone, Default)]
pub struct ResourceLimitsForm {
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
}

/// 新建容器表单；出错时保留用户原始输入。
#[derive(Debug, Clone, Default)]
pub struct ContainerFormView {
    pub image: String,
    pub name: String,
    pub ports: String,
    pub env: String,
    pub binds: String,
    pub volumes: String,
    pub restart: String,
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
    pub networks: Vec<String>,
}

#[derive(Template)]
#[template(path = "container_new.html")]
pub struct ContainerNewPage {
    pub ctx: PageCtx,
    pub form: ContainerFormView,
    pub network_options: Vec<(String, bool)>,
    pub docker_reason: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ComposeServiceView {
    pub service: String,
    pub container: String,
    pub id: String,
    pub state: String,
    pub status: String,
    pub image: String,
    pub managed: bool,
    pub logs: String,
}

#[derive(Debug, Clone)]
pub struct ComposeProjectView {
    pub name: String,
    pub managed: bool,
    pub services: usize,
    pub running: usize,
}

#[derive(Debug, Clone)]
pub struct ComposeReportRow {
    pub kind: String,
    pub name: String,
    pub status: String,
    pub error: String,
}

#[derive(Template)]
#[template(path = "compose.html")]
pub struct ComposePage {
    pub ctx: PageCtx,
    /// 部署 / 起停 / 删除属生命周期动词（admin 专属）。
    pub can_admin: bool,
    pub docker_reason: String,
    pub error: String,
    pub notice: String,
    pub compose_text: String,
    pub project_input: String,
    pub validation: Vec<String>,
    pub report_title: String,
    pub report: Vec<ComposeReportRow>,
    pub projects: Vec<ComposeProjectView>,
    pub selected_project: String,
    pub services: Vec<ComposeServiceView>,
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
    /// 当前监听的 IP 与端口（拆分展示，便于端口占用校验）。
    pub listen_addr: String,
    pub listen_port: u16,
    /// TLS 三态：`auto` / `on` / `off`（与 `config.toml` 的 `tls_enabled` 对应）。
    pub tls_mode: String,
    pub tls_cert: String,
    pub tls_key: String,
    /// 配置文件落盘路径（设置页展示 + 提示）。
    pub config_path: String,
    /// 明文反代形态：TLS 由 trusted_proxies 中的反代终止。
    pub plaintext_behind_proxy: bool,
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

#[derive(Debug, Clone, Default)]
pub struct TaskRow {
    pub id: String,
    pub name: String,
    pub expr: String,
    pub next_text: String,
    pub source: String,
    pub target: String,
    pub method: String,
    pub enabled: bool,
    pub state_label: String,
    pub state_class: String,
    pub last_text: String,
    pub last_class: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TaskRunRow {
    pub trigger: String,
    pub started_text: String,
    pub duration: String,
    pub result: String,
    pub result_class: String,
    pub detail: String,
    pub missed: i64,
}

/// 任务中心列表行。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct JobRowView {
    pub id: String,
    pub title: String,
    pub kind_label: String,
    pub target_label: String,
    pub status_label: String,
    pub status_class: String,
    pub phase_label: String,
    pub progress: i64,
    pub indeterminate: bool,
    pub duration: String,
    pub created_by: String,
    pub created_at: String,
    pub active: bool,
}

/// 任务详情（含取消/重试的可行动状态）。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct JobDetailView {
    pub id: String,
    pub title: String,
    pub kind_label: String,
    pub target_label: String,
    pub status_label: String,
    pub status_class: String,
    pub phase_label: String,
    pub progress: i64,
    pub indeterminate: bool,
    pub cancellable: bool,
    pub cancel_requested: bool,
    pub created_by: String,
    pub created_by_role: String,
    pub requester_ip: String,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration: String,
    /// 触发来源（手动提交 / 重试），无值时由 handler 兜底。
    pub trigger: String,
    /// 退出码文本：无单条命令语义时为 `-`。
    pub exit_code: String,
    pub error_class: String,
    pub error_detail: String,
    pub result_json: String,
    pub params_json: String,
    pub retry_of: String,
    pub can_cancel: bool,
    pub can_retry: bool,
    /// 轮询/SSE 的起始游标：页面首次加载时补历史用。
    pub log_cursor: i64,
    /// 首屏日志尾巴（已格式化为 `[HH:MM:SS] 行`），避免详情页首包空着。
    pub log_text: String,
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
