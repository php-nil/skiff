//! `/ui/v1` 的 wire 形状（DTO）：只描述请求/响应字段，不含业务逻辑。

#[allow(unused_imports)]
use super::*;

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
    pub primary: String,
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

/// 概览卡片的计数（只读；数据来自 `dashboard::load_overview` 的既有查询）。
#[derive(Debug, Serialize)]
pub struct OverviewCounts {
    pub collected_at: String,
    pub docker: OverviewDocker,
    pub containers: OverviewContainers,
    pub sites: OverviewSites,
    pub plugins: OverviewPlugins,
    pub runtimes: OverviewRuntimes,
    /// `overview.health` 槽位的提供者（已启用插件）。
    ///
    /// 新壳不渲染 iframe 槽位：前端拿到插件 id 后走统一 RPC 取数，再用 schema 渲染卡片。
    /// 没有可用提供者时为 `null`。
    pub health_plugin: Option<String>,
    /// 没有可用提供者时的可读原因（已停用 / 未安装声明槽位的插件）。
    ///
    /// 前端在 `health_plugin` 为空时渲染原因卡片，不静默消失；取数失败的原因
    /// 由 RPC 错误信封带回，不复用本字段。
    pub health_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OverviewDocker {
    pub status: OverviewStatus,
    pub reason: String,
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Serialize)]
pub struct OverviewStatus {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct OverviewContainers {
    pub running: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewSites {
    pub total: usize,
    pub abnormal: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewPlugins {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewRuntimes {
    pub total: usize,
    pub abnormal: usize,
    pub uninstalled: usize,
}

/// 列表接口公共查询参数（D10）：`limit` 默认 50、上限 200；`cursor` 为上一页末项的
/// 不透明键（站点 `created_at:id`，运行环境 `id`）。
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// 站点列表关键字，与页面同名参数一致（域名 / 目录 / 接入方式 / 别名）。
    pub q: Option<String>,
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub code: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteHttps {
    pub enabled: bool,
    pub force_https: bool,
    pub label: String,
    pub class: String,
}

/// 站点列表项（字段冻结，只增不改；与页面 `SiteView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteItem {
    pub domain: String,
    pub root_dir: String,
    pub index_dir: String,
    pub site_type: String,
    pub type_label: String,
    pub runtime_id: String,
    pub runtime_label: String,
    pub runtime_name: String,
    pub runtime_shared_hint: String,
    pub proxy_target: String,
    pub proxy_websocket: bool,
    pub endpoint: String,
    pub status: StatusDto,
    pub https: SiteHttps,
    pub is_default: bool,
    pub aliases: Vec<String>,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
}

/// 站点详情里的证书信息（只读）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteSsl {
    pub enabled: bool,
    pub force_https: bool,
    pub cert_exists: bool,
    pub cert_path: String,
    pub key_path: String,
    pub subject: String,
    pub issuer: String,
    pub expires: String,
}

/// 站点详情里绑定的运行环境（只读快照）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteRuntimeRef {
    pub id: String,
    pub name: String,
    pub kind: StatusDto,
    pub version: String,
    pub state: StatusDto,
    pub site_count: usize,
}

/// 站点 vhost 配置快照；读取失败时 `error` 非空，其余字段为空。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteConfigInfo {
    pub path: String,
    pub content: String,
    pub active: bool,
    pub error: String,
}

/// 站点备份开关快照（任务服务未初始化时 `enabled=false`、`available=false`）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteBackupInfo {
    pub available: bool,
    pub enabled: bool,
    pub task_id: String,
    pub last_text: String,
    pub next_text: String,
}

/// 站点详情响应数据（列表项字段 + 证书 + 运行环境状态 + 配置/备份 + 写权限）。
#[derive(Debug, Serialize)]
pub struct SiteDetailData {
    #[serde(flatten)]
    pub site: SiteItem,
    pub ssl: SiteSsl,
    pub runtime: Option<SiteRuntimeRef>,
    pub runtime_down: bool,
    pub runtime_alert: String,
    pub config: SiteConfigInfo,
    pub backup: SiteBackupInfo,
    pub can_manage: bool,
}

/// `POST /ui/v1/sites` 的请求体：字段与旧新建表单同名。
#[derive(Debug, Default, Deserialize)]
pub struct SiteCreateBody {
    #[serde(default)]
    pub site_type: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub aliases_bulk: String,
    #[serde(default)]
    pub root_dir: String,
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub proxy_target: String,
    #[serde(default)]
    pub proxy_websocket: bool,
}

/// `POST /ui/v1/sites/{domain}/default`：设为 / 取消默认站点（`SITE_SET_DEFAULT`）。
#[derive(Debug, Default, Deserialize)]
pub struct EnabledBody {
    #[serde(default)]
    pub enabled: bool,
}

/// `POST /ui/v1/sites/{domain}/domains`：新增 / 删除别名（`SITE_UPDATE_DOMAINS`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteDomainBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub alias: String,
}

/// `POST /ui/v1/sites/{domain}/config`：保存 vhost 配置（`SITE_SAVE_CONFIG`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteConfigBody {
    #[serde(default)]
    pub content: String,
}

/// `POST /ui/v1/sites/{domain}/ssl`：证书生成 / 上传 / 强制跳转 / 关闭 / 删除（`SITE_SSL`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteSslBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cert_pem: String,
    #[serde(default)]
    pub key_pem: String,
}

/// `POST /ui/v1/sites/{domain}/delete`：删除站点，服务端强制域名确认（`SITE_DELETE`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteDeleteBody {
    #[serde(default)]
    pub confirm: String,
}

/// `POST /ui/v1/sites/batch`：批量启停（`SITE_BATCH_CREATE`；批量删除仍由命令层拒绝）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteBatchBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub domains: Vec<String>,
}

/// 运行环境最近一次异步任务（安装/扩展）；字段与页面抽屉同源。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeJobItem {
    pub state: String,
    pub label: String,
    pub step: String,
    pub progress: u8,
    pub error: String,
    pub active: bool,
    pub failed: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub started_at: String,
    pub updated_at: String,
}

/// 运行环境列表项（字段冻结，只增不改；与页面 `RuntimeRow` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeItem {
    pub id: String,
    pub kind: StatusDto,
    pub name: String,
    pub container_name: String,
    pub version: String,
    pub version_display: String,
    pub version_note: String,
    pub image: String,
    pub state: StatusDto,
    pub state_class: String,
    pub drift: bool,
    pub site_count: usize,
    pub container_state: String,
    pub endpoint_short: String,
    pub endpoint_full: String,
    pub implemented: bool,
    pub is_default: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
    pub updated_at: String,
    pub job: Option<RuntimeJobItem>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ContainerListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// `error` 只保留异常容器（概览待办深链），其余值视为不筛选。
    pub filter: Option<String>,
}

/// 容器列表项（字段冻结，只增不改；与旧表格同一 `container_view`）。
#[derive(Debug, Clone, Serialize)]
pub struct ContainerItem {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub state: StatusDto,
    pub status: String,
    pub project: String,
    pub protected: bool,
    pub unprotected_kernel: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerPort {
    pub host: String,
    pub container: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerMount {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLabel {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLimitLine {
    pub label: String,
    pub value: String,
}

/// 资源限制的原始表单值（字符串保留「默认/不限制」语义）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ContainerLimits {
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
}

/// 容器详情（只读）：基本信息按块拆分，字段与旧详情页同源。
#[derive(Debug, Serialize)]
pub struct ContainerDetailData {
    pub container: ContainerItem,
    pub image: String,
    pub cmd: String,
    /// RFC3339 UTC（D9）；Docker 返回原值。
    pub created_at: String,
    pub health: String,
    pub ports: Vec<ContainerPort>,
    pub mounts: Vec<ContainerMount>,
    pub networks: Vec<String>,
    pub labels: Vec<ContainerLabel>,
    pub limits: ContainerLimits,
    pub limits_effective: Vec<ContainerLimitLine>,
    pub can_exec: bool,
    pub can_operate: bool,
    pub can_delete: bool,
    pub docker_reason: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeFileQuery {
    #[serde(default)]
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeCreateBody {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// 缺省 `None` 表示沿用目录默认扩展（与旧表单 `ext_form` 缺省语义一致）。
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeActionBody {
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct PhpConfigBody {
    #[serde(default)]
    pub memory_limit: String,
    #[serde(default)]
    pub upload_max_filesize: String,
    #[serde(default)]
    pub post_max_size: String,
    #[serde(default)]
    pub max_execution_time: Option<u64>,
    #[serde(default)]
    pub pm: String,
    #[serde(default)]
    pub max_children: Option<u32>,
    #[serde(default)]
    pub start_servers: Option<u32>,
    #[serde(default)]
    pub min_spare_servers: Option<u32>,
    #[serde(default)]
    pub max_spare_servers: Option<u32>,
    #[serde(default)]
    pub max_requests: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct NodeConfigBody {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub health_path: String,
    #[serde(default)]
    pub node_env: String,
    #[serde(default)]
    pub env: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimePortBody {
    pub port: u16,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeCloneBody {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeExtensionsBody {
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExtensionBatchBody {
    pub op: String,
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PeclBody {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigFileBody {
    pub key: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct RuntimeImagesBody {
    #[serde(default)]
    pub nginx_image: Option<String>,
    #[serde(default)]
    pub php_image: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WebLogsQuery {
    /// `1` / `true` 都表示读取；前端 URL 用 `load=1`。
    #[serde(default)]
    pub load: String,
    /// `access` | `error`；缺省按 access。
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub lines: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct WebConfigBody {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct WebParamsBody {
    #[serde(default)]
    pub worker_processes: String,
    pub worker_connections: u32,
    pub keepalive_timeout: u32,
    #[serde(default)]
    pub client_max_body_size: String,
    #[serde(default)]
    pub gzip: bool,
    #[serde(default)]
    pub gzip_types: String,
}

/// `POST /ui/v1/containers` 的请求体：与旧表单同名，空串按未填处理。
#[derive(Debug, Default, Deserialize)]
pub struct ContainerCreateBody {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ports: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub binds: String,
    #[serde(default)]
    pub volumes: String,
    #[serde(default)]
    pub restart: String,
    #[serde(default)]
    pub cpu_shares: String,
    #[serde(default)]
    pub cpu_period: String,
    #[serde(default)]
    pub cpu_quota: String,
    #[serde(default)]
    pub memory_limit_mb: String,
    #[serde(default)]
    pub memory_swap_mb: String,
    #[serde(default)]
    pub pids_limit: String,
    #[serde(default)]
    pub networks: Vec<String>,
}

/// `POST /ui/v1/containers/{id}/limits` 的请求体：空串沿用旧表单语义（0 = 清除）。
#[derive(Debug, Default, Deserialize)]
pub struct ContainerLimitsBody {
    #[serde(default)]
    pub cpu_shares: String,
    #[serde(default)]
    pub cpu_period: String,
    #[serde(default)]
    pub cpu_quota: String,
    #[serde(default)]
    pub memory_limit_mb: String,
    #[serde(default)]
    pub memory_swap_mb: String,
    #[serde(default)]
    pub pids_limit: String,
}

/// 镜像 / 网络 / 卷的通用行。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceItem {
    pub id: String,
    pub name: String,
    pub meta: String,
    pub warn: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkContainerItem {
    pub name: String,
    pub ipv4: String,
    pub ipv6: String,
    pub mac: String,
}

/// 网络行（字段冻结，只增不改；与旧页面 `NetworkRowView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkItem {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub ipam_driver: String,
    pub subnet: String,
    pub gateway: String,
    pub internal: String,
    pub attachable: String,
    pub container_count: String,
    pub containers_known: bool,
    pub containers: Vec<NetworkContainerItem>,
    pub builtin: bool,
    pub can_remove: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ImagePullBody {
    #[serde(default)]
    pub image: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct NameBody {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ComposeProjectItem {
    pub name: String,
    pub managed: bool,
    pub services: usize,
    pub running: usize,
}

#[derive(Debug, Serialize)]
pub struct ComposeServiceItem {
    pub service: String,
    pub container: String,
    pub id: String,
    pub state: StatusDto,
    pub status: String,
    pub image: String,
    pub managed: bool,
    pub logs: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ComposeQuery {
    pub project: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ComposeDownBody {
    pub project: String,
    #[serde(default)]
    pub remove_volumes: bool,
}

/// compose 源文本 + 可选项目名（与旧页面同名）。
#[derive(Debug, Default, Deserialize)]
pub struct ComposeSourceBody {
    #[serde(default)]
    pub compose: String,
    #[serde(default)]
    pub project: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskItem {
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

#[derive(Debug, Clone, Serialize)]
pub struct TaskRunItem {
    pub trigger: String,
    pub started_text: String,
    pub duration: String,
    pub result: String,
    pub result_class: String,
    pub detail: String,
    pub missed: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobItem {
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
    /// RFC3339 UTC（D9）。
    pub created_at: String,
    pub active: bool,
}

/// 审计列表查询参数：关键字 `q`、`limit`（默认 50 / 上限 200）与签名游标。
#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub q: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

/// 一条审计流水；`ts` 为 RFC3339 UTC（D9）。
#[derive(Debug, Serialize)]
pub struct AuditItem {
    pub id: i64,
    pub ts: String,
    pub actor: String,
    pub ip: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub detail: String,
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
    pub can_admin: bool,
}

/// `GET /ui/v1/settings/ui`：壳主题快照（全局生效）。
#[derive(Debug, Serialize)]
pub struct UiThemeData {
    pub primary: String,
    pub can_admin: bool,
}

/// `POST /ui/v1/settings/ui` 的请求体。
#[derive(Debug, Deserialize)]
pub struct UiThemeUpdateBody {
    #[serde(default)]
    pub primary: String,
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

#[derive(Debug, Serialize)]
pub struct FileRootItem {
    pub label: String,
    pub path: String,
}

/// 目录项：只出机器可读字段（时间按 D9 出 RFC3339 UTC），本地化归前端。
#[derive(Debug, Serialize)]
pub struct FileEntryItem {
    pub name: String,
    /// 绝对路径；前端直接用于下载 / 重命名 / 删除，不再自行拼接。
    pub path: String,
    pub dir: bool,
    pub symlink: bool,
    pub size: u64,
    pub mtime: Option<String>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub nlink: u64,
    pub link_target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FilesData {
    pub path: String,
    pub parent: Option<String>,
    pub roots: Vec<FileRootItem>,
    pub entries: Vec<FileEntryItem>,
    /// 条目数超过单页上限（1000），前端必须提示而不是静默截断。
    pub truncated: bool,
    pub max_edit_bytes: usize,
    /// 分片上传单片字节数，前端上传适配器按它切片。
    pub upload_chunk_bytes: usize,
    /// 是否具备 Admin（回收站/配额入口显隐；页面本身仍各自鉴权）。
    pub can_admin: bool,
}

#[derive(Debug, Deserialize)]
pub struct FilePathBody {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct FileRenameBody {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// `GET /ui/v1/menu` 的响应数据：内核菜单树的 JSON 投影（契约 §1.1 的三级归属不变）。
///
/// 只做 wire 形状转换，不另造菜单语义：L1/L2 的可见性、顺序、角色与能力门、
/// 用户隐藏偏好全部由 `skiff_app::menu::build` 决定，与 SSR 导航同源。
#[derive(Debug, Serialize)]
pub struct MenuData {
    pub groups: Vec<MenuGroupData>,
}

#[derive(Debug, Serialize)]
pub struct MenuGroupData {
    /// 稳定键：固定 L1 id（`overview` / `sites` / ...）。
    pub key: String,
    pub label: String,
    pub icon: String,
    /// 非空 = 一级直达链接（概览）；纯分组为空串。
    pub href: String,
    pub items: Vec<MenuLinkData>,
    /// 插件支撑型一级的空态（`工具` 无可见提供者时）。
    pub empty: Option<MenuEmptyData>,
}

#[derive(Debug, Serialize)]
pub struct MenuLinkData {
    /// 稳定键：`core/...` 或 `plugin/<id>/<l2>`。
    pub key: String,
    pub label: String,
    pub icon: String,
    /// 规范深链：内置项为 `/{p}/...`；插件功能页为原生 `/{p}/plugins/{id}/{view}`。
    pub href: String,
    pub plugin: bool,
    /// 插件 id（内置页面为空串）。
    pub plugin_id: String,
    /// 插件默认视图名（内置页面为空串）。
    pub default_view: String,
    /// 插件视图列表：壳页据此生成路由与页内标签。
    pub views: Vec<MenuViewData>,
}

#[derive(Debug, Serialize)]
pub struct MenuViewData {
    pub view: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct MenuEmptyData {
    pub text: String,
    pub href: String,
    pub action: String,
}
