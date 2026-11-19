//! askama 模板上下文。模板文件在 `crates/web/templates/`（见 `askama.toml`）。
//!
//! 壳页已全部迁到 budding 运行时，内核只剩两类 askama 页面：
//! 错误页（`error.html`）与插件 iframe 工作台（`plugin_view.html`，页面自带
//! L3 标签栏，没有 schema 的旧插件视图走它）。

use askama::Template;

/// 插件工作台页面共用的头部上下文。
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
    /// 是否管理员。
    pub is_admin: bool,
    /// 顶栏「插件菜单」：已启用且有 UI 入口的插件（manifest `[[ui]].label` 当菜单名）。
    /// 同时被插件页当 `skiff.open` 白名单。
    pub nav_plugins: Vec<PluginNav>,
    /// 服务端一次性提示：本次渲染消费，用于写操作后的反馈。
    pub flash_notice: String,
    pub flash_error: String,
}

/// 内核升级只读段：当前版本 vs 渠道可用版本，手动下载入口。
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

/// 站点详情页的备份区块。`task_id` 为空表示尚未建立任务（关闭态）。
#[derive(Debug, Clone, Default)]
pub struct SiteBackupView {
    pub enabled: bool,
    pub task_id: String,
    pub last_text: String,
    pub next_text: String,
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

#[derive(Template, Debug, Clone)]
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
    pub tab: String,
    pub ui: String,
    /// 该标签对应的 iframe URL（已带 nonce/menu/tab）。
    pub src: String,
    pub active: bool,
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
