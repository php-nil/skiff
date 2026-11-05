//! manifest.toml：包标识、能力、权限与运行配置（§4.1 表 1 / 表 2）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{PkgError, Result};

/// 内核并行兼容的插件 API 主版本（§11 决策 4：N-1）。
/// MVP 只发过 API 1，因此支持区间是 1.x；将来升 2 时这里扩成 `[1, 2]`。
pub const SUPPORTED_API_MAJOR: &[u32] = &[1];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Docker,
    Identity,
    Fs,
    Net,
    /// 插件间接口调用（`[interop]` + `/capability/interop/*`）。
    ///
    /// 不是宿主能力：它只让插件调用**另一个插件**声明过的接口，
    /// 仍受 `interop:call` 权限与 `plugin:<id>` 对象授权双闸约束。
    Interop,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Docker => "docker",
            Capability::Identity => "identity",
            Capability::Fs => "fs",
            Capability::Net => "net",
            Capability::Interop => "interop",
        }
    }

    /// 全部能力。
    pub fn all() -> &'static [Capability] {
        &[
            Capability::Docker,
            Capability::Identity,
            Capability::Fs,
            Capability::Net,
            Capability::Interop,
        ]
    }

    /// 该部署形态下可授予的能力面（§5.1）。
    ///
    /// 这是**唯一**一处形态→能力的映射。之前 CLI 与 Web 各写一份，
    /// 新增形态或能力时必忘一处，而漏掉的那侧会多给或少给能力。
    /// 受限形态（1Panel 商店 Docker 包）只保留 docker/identity：宿主路径与
    /// 出网收口在容器里无法可靠实施。
    pub fn available_in(form: skiff_model::Form) -> Vec<Capability> {
        match form {
            skiff_model::Form::Native => Capability::all().to_vec(),
            // 受限形态收缩为 docker/identity；interop 只在插件进程之间转发，
            // 不触碰宿主路径/出网，两种形态都提供。
            skiff_model::Form::DockerRestricted => vec![
                Capability::Docker,
                Capability::Identity,
                Capability::Interop,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum RunMode {
    #[default]
    OnDemand,
    Resident,
}

/// 插件清单里声明的一条权限。命名刻意不叫 `Permission`：
/// `skiff_model::Permission` 是「角色→动作」的 RBAC 枚举，两者同名会误读。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPerm {
    pub capability: Capability,
    pub verbs: Vec<String>,
}

/// 插件对外暴露的一个接口。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteropProvide {
    /// 目标插件 RPC 方法名（如 `backup`、`jobs.create`）
    pub method: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// 插件间接口声明（manifest `[interop]`）。
///
/// `provides` 是「我允许别人调用什么」，`consumes` 是「我要调用别人的什么」。
/// 调用是**单向、单层**的：A 调 B 时，B 不能再发起 interop（防环见内核）。
/// `consumes` 每项写成 `<插件 id>:<方法>`，也支持 `<插件 id>:*` 与 `*`。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteropDecl {
    #[serde(default)]
    pub provides: Vec<InteropProvide>,
    #[serde(default)]
    pub consumes: Vec<String>,
}

impl InteropDecl {
    pub fn is_empty(&self) -> bool {
        self.provides.is_empty() && self.consumes.is_empty()
    }

    /// `consumes` 是否允许调用 `target:method`。空清单 = 什么都不能调。
    pub fn allows(&self, target: &str, method: &str) -> bool {
        self.consumes.iter().any(|raw| {
            let raw = raw.trim();
            if raw == "*" {
                return true;
            }
            match raw.split_once(':') {
                Some((p, m)) => p == target && (m == "*" || m == method),
                None => false,
            }
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UiEntry {
    pub label: String,
    /// 相对 `ui/` 的页面路径，内核托管为 `/plugins/{id}/ui/{path}`
    pub path: String,
    #[serde(default)]
    pub icon: Option<String>,
}

/// 固定一级菜单 id：插件只能挂到这些父级下，不能新增一级。
pub const MENU_L1: &[&str] = &["sites", "tools", "plugins", "docker", "settings"];

/// 内核内置图标集（v1）。插件只能引用这里的名字，禁止外链/HTML/SVG 字符串。
pub const MENU_ICONS: &[&str] = &[
    "dot",
    "overview",
    "globe",
    "docker",
    "plug",
    "tools",
    "settings",
    "file",
    "clock",
    "activity",
    "shield",
    "container",
    "image",
    "network",
    "disk",
    "archive",
    "list",
    "user",
    "lock",
];

pub const MENU_ROLES: &[&str] = &["viewer", "operator", "admin"];
pub const MENU_GATES: &[&str] = &["plugin_enabled", "docker_available"];

/// 固定页槽位注册表（P1 只登记，不渲染）。
pub const MENU_SLOTS: &[&str] = &[
    "overview.health",
    "overview.actions",
    "overview.side",
    "sites.header",
];

pub const MENU_MAX_ITEMS: usize = 12;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MenuItem {
    pub id: String,
    pub label: String,
    #[serde(default = "default_menu_order")]
    pub order: i32,
    #[serde(default)]
    pub icon: Option<String>,
    /// 视图名；缺省等于 `id`。内核按 `ui/<view>.html` 托管（`docs/PLUGIN_UI_CONTRACT.md`）。
    #[serde(default)]
    pub view: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMenu {
    /// 本插件内稳定的局部 id，完整键由内核拼成 `plugin/<plugin-id>/<id>`。
    pub id: String,
    /// 只能是固定一级 id（见 `MENU_L1`）。
    pub parent: String,
    pub label: String,
    #[serde(default = "default_menu_order")]
    pub order: i32,
    #[serde(default)]
    pub icon: Option<String>,
    /// `None` = 全角色可见；`Some([])` = 任何角色不可见。
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    #[serde(default = "default_gates")]
    pub gates: Vec<String>,
    #[serde(default = "default_true")]
    pub default_visible: bool,
    #[serde(default = "default_true")]
    pub pinnable: bool,
    /// 该 L2 的默认视图；无 L3 时是唯一视图。缺省 `main`，映射 `ui/main.html`。
    #[serde(default = "default_menu_view")]
    pub view: String,
    #[serde(default)]
    pub items: Vec<MenuItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestMount {
    pub slot: String,
    pub ui: String,
    #[serde(default)]
    pub height: Option<String>,
    #[serde(default)]
    pub refresh: Option<u64>,
    #[serde(default)]
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigField {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

impl ConfigField {
    pub fn validate(&self, key: &str) -> Result<()> {
        const TYPES: &[&str] = &["string", "text", "number", "bool", "password", "url"];
        if !TYPES.contains(&self.kind.as_str()) {
            return Err(PkgError::Manifest(format!(
                "config_schema.{key} 的 type 不支持: {}（可选 {}）",
                self.kind,
                TYPES.join("/")
            )));
        }
        Ok(())
    }

    /// 校验一个**具体取值**是否符合声明的 `type`。
    ///
    /// CLI 写入与将来的 UI 表单共用这一处，避免两边各写一套弱校验
    /// （只校验类型名会导致 `number` 项被塞进任意字符串）。
    pub fn check_value(&self, key: &str, value: &str) -> Result<()> {
        let bad = |expected: &str| {
            Err(PkgError::Manifest(format!(
                "配置项 {key} 需要 {expected}，得到 {value:?}"
            )))
        };
        match self.kind.as_str() {
            "number" => {
                if value.trim().parse::<f64>().is_err() {
                    return bad("数字");
                }
            }
            "bool" => {
                if !matches!(value.trim(), "true" | "false") {
                    return bad("true/false");
                }
            }
            "url" => {
                let v = value.trim();
                if !(v.starts_with("http://") || v.starts_with("https://")) || v.len() > 2048 {
                    return bad("http(s) URL");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// SHA-256(raw public key) 的小写 hex
    pub key_id: String,
    pub api: String,
    #[serde(default)]
    pub run: RunMode,
    #[serde(default)]
    pub resident_reason: Option<String>,
    #[serde(default = "default_true")]
    pub requires_full_sandbox: bool,
    #[serde(default)]
    pub requires: Vec<Capability>,
    #[serde(default)]
    pub permissions: Vec<ManifestPerm>,
    #[serde(default)]
    pub network_egress: Vec<String>,
    #[serde(default)]
    pub ui: Vec<UiEntry>,
    /// 菜单声明：插件只能在固定一级下声明 L2/L3（见 `docs/MENU_CONTRACT.md`）。
    #[serde(default)]
    pub menu: Vec<ManifestMenu>,
    /// 固定页槽位声明（概览健康块等）；P1 只做解析与校验。
    #[serde(default)]
    pub mount: Vec<ManifestMount>,
    /// 安装确认页的**默认资源授权建议**（`path:/abs` / `container:<name|id>` /
    /// `project:<p>` / `plugin:<id>`）。
    ///
    /// 这里只是「建议」，装不装由 admin 在确认页勾选；内核只认安装时落库的
    /// `object:` grant。`path:` 项要求 `requires` 含 `fs`，`container:`/`project:`
    /// 要求含 `docker`，`plugin:` 要求含 `interop`。
    #[serde(default)]
    pub resource_hints: Vec<String>,
    /// 插件间接口声明：`provides`（对外接口）/ `consumes`（调用白名单）
    #[serde(default)]
    pub interop: InteropDecl,
    #[serde(default = "default_idle")]
    pub idle_timeout_sec: u64,
    #[serde(default = "default_restarts")]
    pub max_restarts: u32,
    #[serde(default)]
    pub config_schema: BTreeMap<String, ConfigField>,
    /// 周期任务：5 段 cron 表达式；到点拉起插件调 `tick`，跑完回收（§4.2）
    #[serde(default)]
    pub cron: Vec<String>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub pids_limit: Option<u64>,
}

fn default_true() -> bool {
    true
}
fn default_idle() -> u64 {
    300
}
fn default_restarts() -> u32 {
    3
}
fn default_menu_order() -> i32 {
    1000
}
fn default_gates() -> Vec<String> {
    vec!["plugin_enabled".into()]
}
fn default_menu_view() -> String {
    "main".into()
}

impl ManifestMenu {
    /// 该 L2 实际可寻址的视图名（无 L3 = 隐式单视图）。
    pub fn effective_views(&self) -> Vec<&str> {
        if self.items.is_empty() {
            vec![self.view.as_str()]
        } else {
            self.items
                .iter()
                .map(|it| it.view.as_deref().unwrap_or(it.id.as_str()))
                .collect()
        }
    }
}

fn api_minor(api: &str) -> u32 {
    api.split('.')
        .nth(1)
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(0)
}

/// `/{p}/plugins/{id}/<view>` 里被内核静态段占用的名字：视图名不能与之同名，
/// 否则 GET 与 POST/静态托管路由重叠，安装期就拒绝。
const RESERVED_VIEWS: &[&str] = &["ui", "rpc", "toggle", "remove"];

fn validate_view_name(view: &str) -> Result<()> {
    validate_menu_id(view, "MENU_VIEW_INVALID")?;
    if RESERVED_VIEWS.contains(&view) {
        return Err(PkgError::Manifest(format!(
            "MENU_VIEW_INVALID: 视图名 {view:?} 与内核静态路由冲突（保留 {:?}）",
            RESERVED_VIEWS
        )));
    }
    Ok(())
}

/// 菜单局部 id：小写字母/数字/中划线，1–32 字符；不能是保留的 `core`。
fn validate_menu_id(id: &str, code: &str) -> Result<()> {
    if id.is_empty() || id.len() > 32 {
        return Err(PkgError::Manifest(format!(
            "{code}: 菜单 id 长度必须 1–32: {id:?}"
        )));
    }
    if id == "core" {
        return Err(PkgError::Manifest(format!(
            "{code}: 插件不能注册内核命名空间 core"
        )));
    }
    let mut chars = id.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(PkgError::Manifest(format!(
            "{code}: 菜单 id 必须以小写字母或数字开头: {id:?}"
        )));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(PkgError::Manifest(format!(
            "{code}: 菜单 id 只允许 a-z 0-9 -: {id:?}"
        )));
    }
    if id.ends_with('-') {
        return Err(PkgError::Manifest(format!(
            "{code}: 菜单 id 不能以中划线结尾: {id:?}"
        )));
    }
    Ok(())
}

fn validate_menu_label(label: &str) -> Result<()> {
    let t = label.trim();
    if t.is_empty() || t.chars().count() > 32 {
        return Err(PkgError::Manifest(format!(
            "MENU_LABEL_TOO_LONG: label 长度必须 1–32: {label:?}"
        )));
    }
    if t.chars().any(|c| c.is_control()) {
        return Err(PkgError::Manifest(format!(
            "MENU_LABEL_TOO_LONG: label 不能含控制字符: {label:?}"
        )));
    }
    Ok(())
}

fn validate_menu_order(order: i32) -> Result<()> {
    if !(-10_000..=10_000).contains(&order) {
        return Err(PkgError::Manifest(format!(
            "MENU_ORDER_OUT_OF_RANGE: order 必须在 -10000..=10000，得到 {order}"
        )));
    }
    Ok(())
}

fn validate_roles(roles: Option<&[String]>, code: &str) -> Result<()> {
    if let Some(roles) = roles {
        for r in roles {
            if !MENU_ROLES.contains(&r.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "{code}: 未知角色 {r:?}（可选 {MENU_ROLES:?}）"
                )));
            }
        }
    }
    Ok(())
}

fn validate_menu_asset(path: &str) -> Result<()> {
    if path.is_empty()
        || skiff_model::is_posix_absolute(path)
        || skiff_model::has_parent_segment(path)
    {
        return Err(PkgError::Manifest(format!(
            "MENU_TARGET_INVALID: ui 必须是 ui/ 下的相对路径: {path:?}"
        )));
    }
    Ok(())
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest> {
        let m: Manifest = toml::from_str(text).map_err(|e| PkgError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err(PkgError::Manifest("name 长度必须 1–64".into()));
        }
        validate_semver(&self.version)?;
        validate_api(&self.api)?;
        validate_hex64(&self.key_id, "key_id")?;

        if self.run == RunMode::Resident
            && self
                .resident_reason
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(PkgError::Manifest(
                "run = \"resident\" 必须写 resident_reason（并计入资源预算）".into(),
            ));
        }

        let requires: BTreeSet<Capability> = self.requires.iter().copied().collect();
        if requires.len() != self.requires.len() {
            return Err(PkgError::Manifest("requires 存在重复能力".into()));
        }
        for p in &self.permissions {
            if !requires.contains(&p.capability) {
                return Err(PkgError::Manifest(format!(
                    "permissions 声明了未在 requires 中的能力 {}（离了它不能用，就该在 requires 里）",
                    p.capability.as_str()
                )));
            }
            if p.verbs.is_empty() {
                return Err(PkgError::Manifest(format!(
                    "{} 的 verbs 不能为空",
                    p.capability.as_str()
                )));
            }
            for v in &p.verbs {
                validate_verb(v)?;
            }
        }

        for e in &self.network_egress {
            validate_egress(e)?;
        }

        for u in &self.ui {
            // ui.path 是包内相对路径：不是 POSIX 绝对路径、不含 .. 段
            // （组件级判定，`a..b` 这种普通文件名不算越界）。
            if skiff_model::is_posix_absolute(&u.path)
                || skiff_model::has_parent_segment(&u.path)
                || u.path.is_empty()
            {
                return Err(PkgError::Manifest(format!(
                    "ui.path 必须是 ui/ 下的相对路径: {}",
                    u.path
                )));
            }
        }

        self.validate_menu()?;
        self.validate_mount()?;

        for c in &self.cron {
            validate_cron(c)?;
        }

        for h in &self.resource_hints {
            validate_resource(h)?;
            let needs = if h.starts_with("path:") {
                Capability::Fs
            } else if h.starts_with("plugin:") {
                Capability::Interop
            } else {
                Capability::Docker
            };
            if !requires.contains(&needs) {
                return Err(PkgError::Manifest(format!(
                    "resource_hints 声明了 {}，但 requires 里没有 {}",
                    h,
                    needs.as_str()
                )));
            }
        }

        self.validate_interop(&requires)?;

        for (k, f) in &self.config_schema {
            if k.is_empty()
                || !k
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                return Err(PkgError::Manifest(format!("config_schema 键名非法: {k}")));
            }
            f.validate(k)?;
        }

        if self.idle_timeout_sec < 30 {
            return Err(PkgError::Manifest("idle_timeout_sec 不得小于 30".into()));
        }
        Ok(())
    }

    /// 菜单声明校验（`docs/MENU_CONTRACT.md` §2.3）。
    fn validate_menu(&self) -> Result<()> {
        if self.menu.is_empty() {
            return Ok(());
        }
        if self.ui.is_empty() {
            return Err(PkgError::Manifest(
                "MENU_NO_UI_ENTRY: 声明 [[menu]] 的插件必须至少有一个 [[ui]] 入口".into(),
            ));
        }
        if api_minor(&self.api) < 2 {
            return Err(PkgError::Manifest(
                "MENU_API_VERSION: 菜单声明要求 api = \"1.2\"（旧包请继续用 [[ui]]）".into(),
            ));
        }
        let mut seen_l2 = BTreeSet::new();
        let mut seen_views = BTreeSet::new();
        for m in &self.menu {
            validate_menu_id(&m.id, "MENU_ID_INVALID")?;
            if !MENU_L1.contains(&m.parent.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "MENU_PARENT_UNKNOWN: parent 只能是固定一级 {:?}，得到 {:?}",
                    MENU_L1, m.parent
                )));
            }
            validate_menu_label(&m.label)?;
            validate_menu_order(m.order)?;
            // 图标名不在白名单不拒装：渲染层统一降级为 dot（未知即安全）。
            validate_roles(m.roles.as_deref(), "MENU_ROLE_INVALID")?;
            for g in &m.gates {
                if !MENU_GATES.contains(&g.as_str()) {
                    return Err(PkgError::Manifest(format!(
                        "MENU_GATE_INVALID: 未知 gate {g:?}（可选 {MENU_GATES:?}）"
                    )));
                }
            }
            validate_view_name(&m.view)?;
            if m.items.is_empty() && !seen_views.insert(m.view.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "MENU_VIEW_CONFLICT: 重复的视图名 {:?}",
                    m.view
                )));
            }
            if !seen_l2.insert(m.id.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "MENU_ID_CONFLICT: 重复的菜单 id {:?}",
                    m.id
                )));
            }
            if m.items.len() > MENU_MAX_ITEMS {
                return Err(PkgError::Manifest(format!(
                    "MENU_TOO_MANY_ITEMS: 一个 L2 最多 {MENU_MAX_ITEMS} 个 L3，得到 {}",
                    m.items.len()
                )));
            }
            let mut seen_l3 = BTreeSet::new();
            for it in &m.items {
                validate_menu_id(&it.id, "MENU_ID_INVALID")?;
                validate_menu_label(&it.label)?;
                validate_menu_order(it.order)?;
                if !seen_l3.insert(it.id.as_str()) {
                    return Err(PkgError::Manifest(format!(
                        "MENU_ID_CONFLICT: L2 {} 内重复的 L3 id {:?}",
                        m.id, it.id
                    )));
                }
                let view = it.view.as_deref().unwrap_or(it.id.as_str());
                validate_view_name(view)?;
                if !seen_views.insert(view) {
                    return Err(PkgError::Manifest(format!(
                        "MENU_VIEW_CONFLICT: 重复的视图名 {view:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// 固定页槽位校验（§5.2）。
    fn validate_mount(&self) -> Result<()> {
        if self.mount.is_empty() {
            return Ok(());
        }
        if self.ui.is_empty() {
            return Err(PkgError::Manifest(
                "MOUNT_NO_UI_ENTRY: 声明 [[mount]] 的插件必须至少有一个 [[ui]] 入口".into(),
            ));
        }
        if api_minor(&self.api) < 1 {
            return Err(PkgError::Manifest(
                "MOUNT_API_VERSION: 槽位声明要求 api = \"1.1\"".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for m in &self.mount {
            if !MENU_SLOTS.contains(&m.slot.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "MOUNT_SLOT_INVALID: 未知槽位 {:?}（可选 {MENU_SLOTS:?}）",
                    m.slot
                )));
            }
            if !seen.insert(m.slot.as_str()) {
                return Err(PkgError::Manifest(format!(
                    "MOUNT_SLOT_CONFLICT: 重复声明槽位 {:?}",
                    m.slot
                )));
            }
            validate_menu_asset(&m.ui)?;
            if let Some(h) = &m.height {
                if !matches!(h.as_str(), "sm" | "md" | "lg") {
                    return Err(PkgError::Manifest(format!(
                        "MOUNT_HEIGHT_INVALID: height 只能是 sm/md/lg，得到 {h:?}"
                    )));
                }
            }
            if let Some(r) = m.refresh {
                if ![0, 30, 60, 300].contains(&r) {
                    return Err(PkgError::Manifest(
                        "MOUNT_REFRESH_INVALID: refresh 只能是 0/30/60/300".into(),
                    ));
                }
            }
            validate_roles(m.roles.as_deref(), "MOUNT_ROLE_INVALID")?;
        }
        Ok(())
    }

    /// 校验菜单/槽位引用的 `ui/` 文件确实在包内。
    ///
    /// `Manifest::parse` 只做形式校验（不知道包内容）；安装验包时用这里的
    /// `exists` 回调把「声明了但没打进包」的菜单挡在安装前。
    pub fn validate_menu_assets(&self, exists: impl Fn(&str) -> bool) -> Result<()> {
        for m in &self.menu {
            for view in m.effective_views() {
                if !exists(&format!("{view}.html")) {
                    return Err(PkgError::Manifest(format!(
                        "MENU_VIEW_MISSING: 视图 {view} 的文件不存在于包内 ui/：{view}.html"
                    )));
                }
            }
        }
        for m in &self.mount {
            if !exists(&m.ui) {
                return Err(PkgError::Manifest(format!(
                    "MOUNT_TARGET_INVALID: 槽位 {} 的 ui 不存在于包内 ui/：{}",
                    m.slot, m.ui
                )));
            }
        }
        Ok(())
    }

    /// `[interop]` 的形式与联动校验。
    fn validate_interop(&self, requires: &BTreeSet<Capability>) -> Result<()> {
        if self.interop.is_empty() {
            return Ok(());
        }
        if !requires.contains(&Capability::Interop) {
            return Err(PkgError::Manifest(
                "声明了 [interop] 但 requires 里没有 interop".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for p in &self.interop.provides {
            validate_verb(&p.method)?;
            if !seen.insert(p.method.clone()) {
                return Err(PkgError::Manifest(format!(
                    "interop.provides 重复声明方法: {}",
                    p.method
                )));
            }
            if let Some(label) = &p.label {
                if label.trim().is_empty() || label.len() > 64 {
                    return Err(PkgError::Manifest(format!(
                        "interop.provides.{} 的 label 长度必须 1–64",
                        p.method
                    )));
                }
            }
        }
        for raw in &self.interop.consumes {
            let raw = raw.trim();
            // 全量白名单：`*` 单独成项，不写成 `<插件>:<方法>`。
            if raw == "*" {
                continue;
            }
            let Some((plugin, method)) = raw.split_once(':') else {
                return Err(PkgError::Manifest(format!(
                    "interop.consumes 必须是 <插件 id>:<方法>（或 * ）: {raw:?}"
                )));
            };
            if plugin.is_empty() || method.is_empty() {
                return Err(PkgError::Manifest(format!(
                    "interop.consumes 的插件 id/方法不能为空: {raw:?}"
                )));
            }
            if plugin != "*" {
                validate_id(plugin)
                    .map_err(|e| PkgError::Manifest(format!("interop.consumes {raw:?}: {e}")))?;
            }
            if method != "*" {
                validate_verb(method)
                    .map_err(|e| PkgError::Manifest(format!("interop.consumes {raw:?}: {e}")))?;
            }
        }
        if !self.interop.consumes.is_empty()
            && !self
                .permissions
                .iter()
                .any(|p| p.capability == Capability::Interop && p.verbs.iter().any(|v| v == "call"))
        {
            return Err(PkgError::Manifest(
                "声明了 interop.consumes，必须同时申请 permissions: interop:call".into(),
            ));
        }
        Ok(())
    }

    /// 由 `interop.consumes` 派生的对象授权建议（`plugin:<id>` / `plugin:*`）。
    ///
    /// 调用方必须拿到 `plugin:` 授权才能跨插件调用；这里把白名单翻译成安装
    /// 确认页的默认勾选项，避免「装完才发现少勾一条」。
    pub fn interop_resource_hints(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .interop
            .consumes
            .iter()
            .filter_map(|raw| {
                let raw = raw.trim();
                let plugin = raw.split_once(':').map(|(p, _)| p).unwrap_or(raw);
                if plugin.is_empty() {
                    None
                } else {
                    Some(format!("plugin:{plugin}"))
                }
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// 已声明的权限串（`cap:verb`），安装时逐条向用户确认。
    pub fn permission_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.permissions {
            for v in &p.verbs {
                out.push(format!("{}:{}", p.capability.as_str(), v));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn requires_str(&self) -> Vec<&'static str> {
        self.requires.iter().map(|c| c.as_str()).collect()
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() < 4 || id.len() > 128 {
        return Err(PkgError::Manifest("id 长度必须 4–128".into()));
    }
    if !id.contains('.') {
        return Err(PkgError::Manifest(
            "id 必须是反向域名风格（如 com.example.nginx）".into(),
        ));
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    if !ok || id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return Err(PkgError::Manifest(format!("id 非法: {id}")));
    }
    Ok(())
}

fn validate_semver(v: &str) -> Result<()> {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(PkgError::Manifest(format!(
            "version 必须是 x.y.z 形式: {v}"
        )));
    }
    Ok(())
}

fn validate_api(api: &str) -> Result<()> {
    let mut it = api.split('.');
    let major: u32 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PkgError::Manifest(format!("api 版本非法: {api}")))?;
    let minor_ok = it
        .next()
        .map(|s| s.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false);
    if it.next().is_some() || !minor_ok {
        return Err(PkgError::Manifest(format!(
            "api 版本必须是 主.次（如 1.0）: {api}"
        )));
    }
    if !SUPPORTED_API_MAJOR.contains(&major) {
        return Err(PkgError::Manifest(format!(
            "插件 API 主版本 {major} 不被内核支持（支持 {SUPPORTED_API_MAJOR:?}）"
        )));
    }
    Ok(())
}

fn validate_hex64(v: &str, field: &str) -> Result<()> {
    if v.len() != 64
        || !v
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(PkgError::Manifest(format!(
            "{field} 必须是 64 位小写 hex（SHA-256 指纹）: {v}"
        )));
    }
    Ok(())
}

fn validate_verb(v: &str) -> Result<()> {
    let ok = !v.is_empty()
        && v.len() <= 64
        && v.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        });
    if ok {
        Ok(())
    } else {
        Err(PkgError::Manifest(format!("权限动词非法: {v}")))
    }
}

fn validate_egress(e: &str) -> Result<()> {
    if e == "unrestricted" {
        return Ok(());
    }
    let (cidr, port) = match e.rsplit_once(':') {
        Some((c, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| PkgError::Manifest(format!("network_egress 端口非法: {e}")))?;
            if port == 0 {
                return Err(PkgError::Manifest(format!("network_egress 端口非法: {e}")));
            }
            (c, Some(port))
        }
        None => (e, None),
    };
    let _ = port;
    parse_cidr(cidr).ok_or_else(|| {
        PkgError::Manifest(format!(
            "network_egress 只支持 IP/CIDR[:端口] 或 unrestricted（不做域名放行）: {e}"
        ))
    })?;
    Ok(())
}

fn parse_cidr(s: &str) -> Option<()> {
    let (ip, prefix) = match s.split_once('/') {
        Some((i, p)) => {
            let prefix: u8 = p.parse().ok()?;
            (i, Some(prefix))
        }
        None => (s, None),
    };
    let addr: std::net::IpAddr = ip.parse().ok()?;
    match addr {
        std::net::IpAddr::V4(_) => {
            if let Some(p) = prefix {
                if p > 32 {
                    return None;
                }
            }
        }
        std::net::IpAddr::V6(_) => {
            if let Some(p) = prefix {
                if p > 128 {
                    return None;
                }
            }
        }
    }
    Some(())
}

/// 校验一条资源授权值（安装确认页、CLI 与 manifest `resource_hints` 共用一份）。
///
/// 四种形态：`path:<绝对路径>`（宿主路径授权根）、`container:<id|name>`、
/// `project:<name>`、`plugin:<插件 id>`（或 `plugin:*`）。这里只做形式校验：
/// 路径是否存在、目标插件是否安装，都由内核在**每次调用**时判定。
pub fn validate_resource(value: &str) -> Result<()> {
    let bad = |why: &str| {
        Err(PkgError::Manifest(format!(
            "资源授权 {value:?} 非法：{why}"
        )))
    };
    if value.is_empty() || value.len() > 1024 || value.contains('\0') {
        return bad("长度或字符非法");
    }
    let Some((kind, rest)) = value.split_once(':') else {
        return bad("缺类型前缀（path: / container: / project:）");
    };
    match kind {
        "path" => {
            if skiff_model::has_parent_segment(rest) {
                return bad("路径不得含 ..");
            }
            if rest.trim_matches('/').is_empty() {
                return bad("不得把文件系统根 / 整棵授权");
            }
            // 生产环境只跑 Linux（`/` 即绝对）；同时接受平台 `Path::is_absolute()`，
            // 免得开发/CI 在 Windows 上把 `path:/proc` 这类合法 Linux 路径判成相对路径
            // （与 `skiff_model::path::is_absolute` 同一口径）。
            if !skiff_model::is_absolute(rest) {
                return bad("必须是绝对路径");
            }
            Ok(())
        }
        "container" | "project" => {
            let ok = !rest.is_empty()
                && rest.len() <= 128
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
            if ok {
                Ok(())
            } else {
                bad("对象名非法")
            }
        }
        "plugin" => {
            if rest == "*" {
                return Ok(());
            }
            if rest.is_empty() || rest.contains('*') {
                return bad("插件 id 非法（通配只能整条写成 plugin:*）");
            }
            validate_id(rest)
        }
        other => bad(&format!("未知类型 {other}")),
    }
}

fn validate_cron(expr: &str) -> Result<()> {
    // 语义校验与运行时共用 `skiff-cron`：只查字符会在 `60 * * * *`、`*/0` 这类
    // 「装了但永远不触发」的表达式上放行，错误要到用户发现任务没跑才暴露。
    if skiff_cron::CronExpr::parse(expr).is_none() {
        return Err(PkgError::Manifest(format!(
            "cron 表达式非法（只支持 5 段：分 时 日 月 周）: {expr}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
id = "com.example.nginx"
name = "Nginx 管理"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000001"
api = "1.0"
run = "on-demand"
requires = ["docker"]
network_egress = ["10.0.0.0/8:443"]
idle_timeout_sec = 300
max_restarts = 3

[[permissions]]
capability = "docker"
verbs = ["containers.list", "containers.logs", "containers.exec"]

[[ui]]
label = "Nginx"
path = "index.html"

[config_schema.site_name]
type = "string"
label = "站点名"

[config_schema.token]
type = "password"
secret = true
"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!(m.id, "com.example.nginx");
        assert_eq!(m.requires, vec![Capability::Docker]);
        assert_eq!(m.permission_strings().len(), 3);
        assert!(m.requires_full_sandbox);
        assert_eq!(m.run, RunMode::OnDemand);
    }

    #[test]
    fn rejects_bad_id_version_api() {
        let bad = OK.replace("com.example.nginx", "nginx");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace("0.1.0", "0.1");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace("\"1.0\"", "\"2.0\"");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace(
            "0000000000000000000000000000000000000000000000000000000000000001",
            "ZZ",
        );
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn resident_requires_reason() {
        let bad = OK.replace("run = \"on-demand\"", "run = \"resident\"");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("resident_reason"), "{e}");
        // 注意：新键必须插在第一个 [table] 之前，否则 TOML 会把它算进那张表里
        let good = OK.replace(
            "run = \"on-demand\"",
            "run = \"resident\"\nresident_reason = \"需要常驻采集指标\"",
        );
        assert!(Manifest::parse(&good).is_ok());
    }

    #[test]
    fn permission_must_be_in_requires() {
        let bad = OK.replace("requires = [\"docker\"]", "requires = []");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("requires"), "{e}");
    }

    #[test]
    fn egress_rejects_domains() {
        let bad = OK.replace("10.0.0.0/8:443", "example.com:443");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("network_egress"), "{e}");
        let bad = OK.replace("10.0.0.0/8:443", "10.0.0.0/33:443");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn unknown_fields_rejected() {
        let bad = format!("{OK}\nprivileged = true\n");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn cron_and_ui_paths_validated() {
        let bad = OK.replace("path = \"index.html\"", "path = \"/etc/passwd\"");
        assert!(Manifest::parse(&bad).is_err());
        // cron 是顶层键，必须插在第一个 [table] 之前；这里挂在 network_egress 后面。
        let with_cron = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\ncron = [\"*/5 * * * *\"]",
        );
        assert_ne!(with_cron, OK, "样例里找不到插入点，测试本身要修");
        assert!(Manifest::parse(&with_cron).is_ok());
        let bad_cron = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\ncron = [\"*/5 * *\"]",
        );
        assert!(Manifest::parse(&bad_cron).is_err());
    }

    /// 资源授权建议：形式要合法，且与 `requires` 对得上（少了对应能力就等于装完必拒）。
    #[test]
    fn resource_hints_must_match_requires() {
        let with_docker_hint = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\nresource_hints = [\"container:nginx\"]",
        );
        assert_ne!(with_docker_hint, OK, "样例里找不到插入点，测试本身要修");
        assert!(Manifest::parse(&with_docker_hint).is_ok());

        // `path:` 建议需要 fs 能力；requires 只有 docker 时必须报错而不是静默忽略
        let abs = std::env::temp_dir().join("skiff-hint-root");
        // Windows 的 temp 路径带反斜杠，直接拼进 TOML 会被当成转义序列（
        // `C:\Users` 里的 `\U`），解析直接失败——先按 TOML 基本字符串转义。
        let abs_lit = abs.display().to_string().replace('\\', "\\\\");
        let bad = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            &format!(
                "network_egress = [\"10.0.0.0/8:443\"]\nresource_hints = [\"path:{abs_lit}\"]"
            ),
        );
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("requires"), "{e}");

        let bad_shape = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\nresource_hints = [\"path:etc\"]",
        );
        assert!(Manifest::parse(&bad_shape).is_err());
    }

    #[test]
    fn validate_resource_shapes() {
        let abs = std::env::temp_dir().join("skiff-hint-ok");
        assert!(validate_resource(&format!("path:{}", abs.display())).is_ok());
        // 官方插件的 Linux 路径在 Windows 开发机上同样要过（否则 monitor 的
        // `path:/proc` 会在根 `cargo test` 里被误判）
        assert!(validate_resource("path:/proc").is_ok());
        assert!(validate_resource("container:web").is_ok());
        assert!(validate_resource("project:app").is_ok());
        assert!(validate_resource("plugin:com.skiff.cron").is_ok());
        assert!(validate_resource("plugin:*").is_ok());
        // 相对路径、保留 `..`、根目录、空对象名、未知类型一律拒
        assert!(validate_resource("path:relative/conf").is_err());
        assert!(validate_resource("path:/etc/../passwd").is_err());
        assert!(validate_resource("path:/").is_err());
        assert!(validate_resource("container:").is_err());
        assert!(validate_resource("container:a b").is_err());
        assert!(validate_resource("volume:data").is_err());
        assert!(validate_resource("container").is_err());
        // 插件 id 必须反向域名风格；通配只能整条写
        assert!(validate_resource("plugin:cron").is_err());
        assert!(validate_resource("plugin:com.*").is_err());
        assert!(validate_resource("plugin:").is_err());
    }

    fn interop_manifest(consumes: &str, call_perm: bool) -> String {
        let perm = if call_perm {
            "[[permissions]]\ncapability = \"interop\"\nverbs = [\"call\"]\n"
        } else {
            ""
        };
        format!(
            r#"
id = "com.example.caller"
name = "Caller"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000001"
api = "1.0"
requires = ["interop", "identity"]
{perm}
[[permissions]]
capability = "identity"
verbs = ["self"]

[interop]
provides = [{{ method = "backup", label = "立即备份" }}]
consumes = [{consumes}]
"#
        )
    }

    #[test]
    fn interop_decl_is_validated_with_requires_and_permissions() {
        let m = Manifest::parse(&interop_manifest("\"com.skiff.cron:jobs.create\"", true)).unwrap();
        assert_eq!(m.interop.provides[0].method, "backup");
        assert!(m.interop.allows("com.skiff.cron", "jobs.create"));
        assert!(!m.interop.allows("com.skiff.cron", "jobs.remove"));
        assert_eq!(
            m.interop_resource_hints(),
            vec!["plugin:com.skiff.cron".to_string()]
        );

        // consumes 不给方法通配：只有精确方法放行
        let m = Manifest::parse(&interop_manifest("\"com.skiff.cron:*\"", true)).unwrap();
        assert!(m.interop.allows("com.skiff.cron", "jobs.remove"));

        // 声明了 consumes 却没申请 interop:call → 装完必然调不通，装前就拒
        let e = Manifest::parse(&interop_manifest("\"com.skiff.cron:jobs.create\"", false))
            .unwrap_err()
            .to_string();
        assert!(e.contains("interop:call"), "{e}");

        // `[interop]` 但 requires 缺 interop：能力面没声明
        let bad = interop_manifest("\"com.skiff.cron:jobs.create\"", true).replace(
            "requires = [\"interop\", \"identity\"]",
            "requires = [\"identity\"]",
        );
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("requires"), "{e}");

        // consumes 形式非法
        for bad in ["\"cron\"", "\"com.skiff.cron\"", "\"com.skiff.cron:\""] {
            let e = Manifest::parse(&interop_manifest(bad, true))
                .unwrap_err()
                .to_string();
            assert!(e.contains("interop.consumes"), "{bad} -> {e}");
        }
    }

    /// 通配白名单派生出 `plugin:*`：安装确认页必须让 admin 看见这条宽授权。
    #[test]
    fn wildcard_consume_derives_plugin_star_hint() {
        let m = Manifest::parse(&interop_manifest("\"*\"", true)).unwrap();
        assert_eq!(m.interop_resource_hints(), vec!["plugin:*".to_string()]);
    }

    fn menu_manifest() -> String {
        OK.replace("api = \"1.0\"", "api = \"1.2\"").replace(
            "path = \"index.html\"",
            "path = \"main.html\"\n\n[[menu]]\nid = \"sites\"\nparent = \"sites\"\nlabel = \"网站\"\norder = 10\nicon = \"globe\"\nroles = [\"viewer\", \"operator\", \"admin\"]\npinnable = true\nview = \"main\"\n\n[[menu.items]]\nid = \"overview\"\nlabel = \"站点概览\"\norder = 10\nview = \"overview\"\n\n[[menu.items]]\nid = \"certificates\"\nlabel = \"证书\"\norder = 20\nview = \"certs\"\n\n[[mount]]\nslot = \"overview.health\"\nui = \"health.html\"\nheight = \"md\"\nrefresh = 30",
        )
    }

    #[test]
    fn parses_menu_and_mount() {
        let m = Manifest::parse(&menu_manifest()).unwrap();
        assert_eq!(m.menu.len(), 1);
        assert_eq!(m.menu[0].id, "sites");
        assert_eq!(m.menu[0].view, "main");
        assert_eq!(m.menu[0].items.len(), 2);
        assert_eq!(m.menu[0].items[1].view.as_deref(), Some("certs"));
        assert_eq!(m.menu[0].effective_views(), vec!["overview", "certs"]);
        assert_eq!(m.mount[0].slot, "overview.health");
        assert_eq!(m.mount[0].refresh, Some(30));
    }

    #[test]
    fn menu_rejects_illegal_declarations() {
        let base = menu_manifest();
        let cases: &[(&str, &str, &str)] = &[
            (
                "parent = \"sites\"",
                "parent = \"overview\"",
                "MENU_PARENT_UNKNOWN",
            ),
            ("id = \"sites\"", "id = \"core\"", "MENU_ID_INVALID"),
            ("id = \"sites\"", "id = \"Sites\"", "MENU_ID_INVALID"),
            ("view = \"main\"", "view = \"Main\"", "MENU_VIEW_INVALID"),
            ("view = \"main\"", "view = \"ui\"", "MENU_VIEW_INVALID"),
            (
                "roles = [\"viewer\", \"operator\", \"admin\"]",
                "roles = [\"guest\"]",
                "MENU_ROLE_INVALID",
            ),
            ("order = 10", "order = 99999", "MENU_ORDER_OUT_OF_RANGE"),
            (
                "[[mount]]\nslot = \"overview.health\"",
                "[[mount]]\nslot = \"overview.footer\"",
                "MOUNT_SLOT_INVALID",
            ),
            ("refresh = 30", "refresh = 5", "MOUNT_REFRESH_INVALID"),
        ];
        for (from, to, code) in cases {
            let bad = base.replace(from, to);
            let e = Manifest::parse(&bad).unwrap_err().to_string();
            assert!(e.contains(code), "{from} -> {to}: {e}");
        }

        // 同一插件内重复 L2 id
        let dup = format!(
            "{}\n\n[[menu]]\nid = \"sites\"\nparent = \"tools\"\nlabel = \"工具\"\nview = \"main\"",
            base
        );
        let e = Manifest::parse(&dup).unwrap_err().to_string();
        assert!(e.contains("MENU_ID_CONFLICT"), "{e}");

        // 视图名冲突：单视图 L2 复用已有 L3 的视图名，路由无法判定归属
        let dup_view = format!(
            "{}\n\n[[menu]]\nid = \"other\"\nparent = \"tools\"\nlabel = \"另一个\"\nview = \"overview\"",
            base
        );
        let e = Manifest::parse(&dup_view).unwrap_err().to_string();
        assert!(e.contains("MENU_VIEW_CONFLICT"), "{e}");

        // L3 超过 12 个
        let mut many = base.clone();
        let extra: String = (0..12)
            .map(|i| format!("\n[[menu.items]]\nid = \"x{i}\"\nlabel = \"X{i}\"\n"))
            .collect();
        many.push_str(&extra);
        let e = Manifest::parse(&many).unwrap_err().to_string();
        assert!(e.contains("MENU_TOO_MANY_ITEMS"), "{e}");

        // 旧 API 声明菜单：必须拒绝（不能静默接受）
        let old_api = base.replace("api = \"1.2\"", "api = \"1.1\"");
        let e = Manifest::parse(&old_api).unwrap_err().to_string();
        assert!(e.contains("MENU_API_VERSION"), "{e}");

        // 有菜单却没有 [[ui]] 入口
        let no_ui =
            menu_manifest().replace("[[ui]]\nlabel = \"Nginx\"\npath = \"main.html\"\n\n", "");
        let e = Manifest::parse(&no_ui).unwrap_err().to_string();
        assert!(e.contains("MENU_NO_UI_ENTRY"), "{e}");
    }

    #[test]
    fn menu_assets_must_exist_in_package() {
        let m = Manifest::parse(&menu_manifest()).unwrap();
        assert!(m
            .validate_menu_assets(|p| {
                matches!(
                    p,
                    "main.html" | "overview.html" | "certs.html" | "health.html"
                )
            })
            .is_ok());
        let e = m
            .validate_menu_assets(|p| matches!(p, "main.html" | "overview.html" | "health.html"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("certs.html"), "{e}");
    }
}
