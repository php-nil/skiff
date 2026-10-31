//! 动词白名单 + 资源对象授权 + 参数字段白名单（§6.4.1–6.4.3）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use skiff_model::{Form, Grant, Role, RoleExt};

use crate::paths::{PathError, PathGuard};

/// 里程碑分档：M2 基础档 / M3 生命周期档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Basic,
    Lifecycle,
}

impl Tier {
    pub fn max_verb(self) -> &'static str {
        match self {
            Tier::Basic => "M2（基础档）",
            Tier::Lifecycle => "M3（生命周期档）",
        }
    }
}

/// §6.4.1 动词白名单。任何未列动词一律 403 + 审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    // ---- 基础档 ----
    ContainersList,
    ContainerInspect,
    ContainerLogs,
    ContainerStats,
    ImagesList,
    NetworksList,
    VolumesList,
    Info,
    Version,
    Start,
    Stop,
    Restart,
    Kill,
    Exec,
    ExecStart,
    ExecInspect,
    // ---- 生命周期档 ----
    ContainerCreate,
    ContainerRemove,
    ImagePull,
    ImageRemove,
    NetworkCreate,
    NetworkRemove,
    VolumeCreate,
    VolumeRemove,
    ComposeUp,
    ComposeDown,
}

impl Verb {
    /// 从稳定字符串还原动词——`as_str()` 的唯一逆函数，由 `all()` 派生。
    ///
    /// 内核回调路径收到的是插件传的字符串。此前这里手写了第二份 26 行 match，加动词时
    /// 两边不同步就会出现「白名单里有、解析不了」；现在解析表只有 `all()` 一份，
    /// 配合 `verb_strings_roundtrip` / `all_covers_every_verb` 成对校验。
    /// 仅在内核回调解析路径使用，不在请求热路径上。
    pub fn parse(s: &str) -> Option<Verb> {
        Self::all().iter().copied().find(|v| v.as_str() == s)
    }

    /// 全部动词。新增动词必须同时进这张表——`verb_strings_roundtrip` 用它
    /// 保证 `as_str`/`parse` 与审计命名三者不会各写一套。
    pub fn all() -> &'static [Verb] {
        use Verb::*;
        &[
            ContainersList,
            ContainerInspect,
            ContainerLogs,
            ContainerStats,
            ImagesList,
            NetworksList,
            VolumesList,
            Info,
            Version,
            Start,
            Stop,
            Restart,
            Kill,
            Exec,
            ExecStart,
            ExecInspect,
            ContainerCreate,
            ContainerRemove,
            ImagePull,
            ImageRemove,
            NetworkCreate,
            NetworkRemove,
            VolumeCreate,
            VolumeRemove,
            ComposeUp,
            ComposeDown,
        ]
    }

    /// 内核审计与授权用的稳定字符串（同时是插件权限里的动词名）。
    pub fn as_str(self) -> &'static str {
        use Verb::*;
        match self {
            ContainersList => "containers.list",
            ContainerInspect => "containers.inspect",
            ContainerLogs => "containers.logs",
            ContainerStats => "containers.stats",
            ImagesList => "images.list",
            NetworksList => "networks.list",
            VolumesList => "volumes.list",
            Info => "info",
            Version => "version",
            Start => "containers.start",
            Stop => "containers.stop",
            Restart => "containers.restart",
            Kill => "containers.kill",
            Exec => "containers.exec",
            ExecStart => "exec.start",
            ExecInspect => "exec.inspect",
            ContainerCreate => "containers.create",
            ContainerRemove => "containers.remove",
            ImagePull => "images.pull",
            ImageRemove => "images.remove",
            NetworkCreate => "networks.create",
            NetworkRemove => "networks.remove",
            VolumeCreate => "volumes.create",
            VolumeRemove => "volumes.remove",
            ComposeUp => "compose.up",
            ComposeDown => "compose.down",
        }
    }

    /// 审计动作名：`docker.<verb>`，全站唯一来源。
    ///
    /// web 的 UI 路径、JSON API、插件回调（kernel.sock）都必须调这里取名字。
    /// 之前 `resources.rs` 手写了 `"docker.images.pull"` 之类的字面量，一旦
    /// `as_str()` 改名（或新增动词）审计表里就会出现两种拼法的同一操作。
    pub fn audit_action(self) -> String {
        format!("docker.{}", self.as_str())
    }

    /// 对应 §6.4.1 表里的 HTTP 动作，写审计用。
    pub fn http_path(self) -> &'static str {
        use Verb::*;
        match self {
            ContainersList => "GET /containers/json",
            ContainerInspect => "GET /containers/{id}/json",
            ContainerLogs => "GET /containers/{id}/logs",
            ContainerStats => "GET /containers/{id}/stats",
            ImagesList => "GET /images/json",
            NetworksList => "GET /networks",
            VolumesList => "GET /volumes",
            Info => "GET /info",
            Version => "GET /version",
            Start => "POST /containers/{id}/start",
            Stop => "POST /containers/{id}/stop",
            Restart => "POST /containers/{id}/restart",
            Kill => "POST /containers/{id}/kill",
            Exec => "POST /containers/{id}/exec",
            ExecStart => "POST /exec/{id}/start",
            ExecInspect => "GET /exec/{id}/json",
            ContainerCreate => "POST /containers/create",
            ContainerRemove => "DELETE /containers/{id}",
            ImagePull => "POST /images/create（仅 fromImage）",
            ImageRemove => "DELETE /images/{id}",
            NetworkCreate => "POST /networks/create",
            NetworkRemove => "DELETE /networks/{id}",
            VolumeCreate => "POST /volumes/create",
            VolumeRemove => "DELETE /volumes/{id}",
            ComposeUp => "compose up（多对象）",
            ComposeDown => "compose down（多对象）",
        }
    }

    pub fn in_tier(self, tier: Tier) -> bool {
        use Verb::*;
        match self {
            ContainersList | ContainerInspect | ContainerLogs | ContainerStats | ImagesList
            | NetworksList | VolumesList | Info | Version | Start | Stop | Restart | Kill
            | Exec | ExecStart | ExecInspect => true,
            ContainerCreate | ContainerRemove | ImagePull | ImageRemove | NetworkCreate
            | NetworkRemove | VolumeCreate | VolumeRemove | ComposeUp | ComposeDown => {
                tier == Tier::Lifecycle
            }
        }
    }

    /// 只读动词（viewer 可用，插件只需 `<verb>` 权限即可只读）。
    pub fn is_read(self) -> bool {
        use Verb::*;
        matches!(
            self,
            ContainersList
                | ContainerInspect
                | ContainerLogs
                | ContainerStats
                | ImagesList
                | NetworksList
                | VolumesList
                | Info
                | Version
                | ExecInspect
        )
    }

    /// 生命周期/删除类：admin 专属，且需要对象 grant。
    pub fn is_destructive(self) -> bool {
        use Verb::*;
        matches!(
            self,
            Kill | Exec
                | ExecStart
                | ContainerCreate
                | ContainerRemove
                | ImageRemove
                | NetworkRemove
                | VolumeRemove
                | ComposeUp
                | ComposeDown
                | ImagePull
                | NetworkCreate
                | VolumeCreate
        )
    }

    /// 是否改变运行时状态（读之外的动词）。
    ///
    /// 与 `is_destructive` 的区别：`Stop`/`Restart`/`Start` 不销毁数据，所以对「外部面板
    /// 资源」不算 destruct；但对 `io.skiff.protected=true` 的对象（含 Skiff 自身容器）
    /// **停一下就等于自断管理面**，必须一起拦下。
    pub fn changes_state(self) -> bool {
        self.is_destructive() || matches!(self, Verb::Start | Verb::Stop | Verb::Restart)
    }
}

/// 三层策略的调用主体。
#[derive(Debug, Clone)]
pub enum Actor {
    User { username: String, role: Role },
    Plugin { id: String, grants: Vec<Grant> },
}

impl Actor {
    pub fn label(&self) -> String {
        match self {
            Actor::User { username, .. } => format!("user:{username}"),
            Actor::Plugin { id, .. } => format!("plugin:{id}"),
        }
    }
}

/// 容器/网络/卷的可见对象视图（来自 bollard，策略只看这几个字段）。
#[derive(Debug, Clone, Default)]
pub struct ObjectRef {
    pub id: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

pub const LABEL_PROJECT: &str = "io.skiff.project";
pub const LABEL_OWNER: &str = "io.skiff.owner";
pub const LABEL_PROTECTED: &str = "io.skiff.protected";

impl ObjectRef {
    pub fn is_protected(&self) -> bool {
        self.labels.get(LABEL_PROTECTED).map(|v| v == "true").unwrap_or(false)
            // Skiff 自身容器（compose 服务名 skiff）与内核数据卷恒拒绝
            || self.name == "skiff"
            || self.name.starts_with("skiff_")
    }

    pub fn owner(&self) -> Option<&str> {
        self.labels.get(LABEL_OWNER).map(|s| s.as_str())
    }

    pub fn project(&self) -> Option<&str> {
        self.labels.get(LABEL_PROJECT).map(|s| s.as_str())
    }

    /// 1Panel / 宝塔托管资源：插件默认不可写（§6.4.2）。
    pub fn is_foreign_panel(&self) -> bool {
        self.labels.contains_key("createdBy")
            || self.name.starts_with("1panel")
            || self.name.starts_with("baota")
            || self.name.starts_with("bt_")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 读取列表/详情/日志
    Read,
    /// 启停类
    Operate,
    /// exec 终端
    Exec,
    /// 创建/删除
    Lifecycle,
}

impl Verb {
    pub fn action(self) -> Action {
        use Verb::*;
        match self {
            ContainersList | ContainerInspect | ContainerLogs | ContainerStats | ImagesList
            | NetworksList | VolumesList | Info | Version | ExecInspect => Action::Read,
            Start | Stop | Restart | Kill => Action::Operate,
            Exec | ExecStart => Action::Exec,
            ContainerCreate | ContainerRemove | ImagePull | ImageRemove | NetworkCreate
            | NetworkRemove | VolumeCreate | VolumeRemove | ComposeUp | ComposeDown => {
                Action::Lifecycle
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// 动词不在白名单（或当前分档未放开）
    VerbNotAllowed {
        verb: &'static str,
        tier: &'static str,
    },
    /// 角色不具备该动作权限
    RoleDenied { role: String, action: &'static str },
    /// 插件缺少权限或对象 grant
    PluginDenied { plugin: String, need: String },
    /// 受保护对象
    Protected { object: String },
    /// 1Panel/宝塔资源
    ForeignPanel { object: String },
    /// 参数/路径违规
    BadRequest(String),
    /// 形态不支持（受限 Docker 形态下的宿主绑定挂载）
    Restricted(String),
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::VerbNotAllowed { verb, tier } => {
                write!(f, "动词 {verb} 不在 {tier} 白名单内")
            }
            PolicyError::RoleDenied { role, action } => write!(f, "角色 {role} 无权执行 {action}"),
            PolicyError::PluginDenied { plugin, need } => {
                write!(f, "插件 {plugin} 缺少授权 {need}")
            }
            PolicyError::Protected { object } => write!(f, "受保护对象 {object} 拒绝操作"),
            PolicyError::ForeignPanel { object } => {
                write!(f, "{object} 由其它面板管理，插件不可写")
            }
            PolicyError::BadRequest(m) => write!(f, "{m}"),
            PolicyError::Restricted(m) => write!(f, "受限形态：{m}"),
        }
    }
}

/// 面板运行形态（§5.5）：native 允许授权根内的宿主绑定挂载；Docker 形态一律拒绝。
#[derive(Debug, Clone)]
pub struct Policy {
    pub tier: Tier,
    pub form: Form,
    pub path_guard: PathGuard,
}

impl Policy {
    pub fn new(tier: Tier, form: Form, authorized_roots: Vec<PathBuf>) -> Result<Self, PathError> {
        Ok(Self {
            tier,
            form,
            path_guard: PathGuard::new(authorized_roots)?,
        })
    }

    /// 第一层 + 第二层：动词白名单 → 角色/插件权限 → 对象授权。
    /// `target` 为 None 表示列表类/无对象动词。
    pub fn check(
        &self,
        actor: &Actor,
        verb: Verb,
        target: Option<&ObjectRef>,
    ) -> Result<(), PolicyError> {
        if !verb.in_tier(self.tier) {
            return Err(PolicyError::VerbNotAllowed {
                verb: verb.as_str(),
                tier: self.tier.max_verb(),
            });
        }

        match actor {
            Actor::User { role, .. } => {
                let allowed = match verb.action() {
                    Action::Read => true,
                    Action::Operate => matches!(role, Role::Admin | Role::Operator),
                    Action::Exec | Action::Lifecycle => role.is_admin(),
                };
                if !allowed {
                    return Err(PolicyError::RoleDenied {
                        role: role.to_string(),
                        action: verb.as_str(),
                    });
                }
                // admin 管理外部资源须显式选择；此处只拦「受保护对象」。
                if let Some(t) = target {
                    if t.is_protected() && verb.changes_state() {
                        return Err(PolicyError::Protected {
                            object: t.name.clone(),
                        });
                    }
                }
                Ok(())
            }
            Actor::Plugin { id, grants } => {
                let perm = format!("docker:{}", verb.as_str());
                if !grants.iter().any(|g| g.kind == "perm" && g.value == perm) {
                    return Err(PolicyError::PluginDenied {
                        plugin: id.clone(),
                        need: perm,
                    });
                }
                let Some(t) = target else {
                    return Ok(());
                };
                if t.is_protected() {
                    return Err(PolicyError::Protected {
                        object: t.name.clone(),
                    });
                }
                if t.is_foreign_panel() && verb.is_destructive() {
                    return Err(PolicyError::ForeignPanel {
                        object: t.name.clone(),
                    });
                }
                let owned = t.owner() == Some(id.as_str());
                if owned && !verb.is_destructive() {
                    return Ok(());
                }
                // 非自有对象：需要显式对象 grant（exec/删除/kill 不因 read 授权自动获得）
                let object_grant = grants.iter().any(|g| {
                    g.kind == "object"
                        && (g.value == format!("container:{}", t.id)
                            || g.value == format!("container:{}", t.name)
                            || t.project()
                                .map(|p| g.value == format!("project:{p}"))
                                .unwrap_or(false))
                });
                if object_grant {
                    Ok(())
                } else {
                    Err(PolicyError::PluginDenied {
                        plugin: id.clone(),
                        need: format!("object:container:{}", t.id),
                    })
                }
            }
        }
    }

    /// 第二层补充：插件保留标签不可伪造（§6.4.2）。
    pub fn check_reserved_labels(
        actor: &Actor,
        labels: &BTreeMap<String, String>,
    ) -> Result<(), PolicyError> {
        let reserved = [LABEL_PROJECT, LABEL_OWNER, LABEL_PROTECTED];
        for k in labels.keys() {
            if reserved.contains(&k.as_str()) {
                return Err(PolicyError::BadRequest(format!(
                    "标签 {k} 由内核保留，禁止用户/插件写入（当前主体 {}）",
                    actor.label()
                )));
            }
        }
        Ok(())
    }
}

// ---------------- 第三层：参数字段白名单 ----------------

/// MVP 容器创建的参数面（§6.4.3）。`deny_unknown_fields` 让未列字段**显式报错**
/// 而不是被忽略；`Privileged`/`CapAdd`/`Devices`/`SecurityOpt`/`VolumesFrom`/
/// 自定义 `Runtime`/host 命名空间等字段在类型层面根本不存在。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerSpec {
    pub image: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    #[serde(default)]
    pub restart: Option<String>,
    #[serde(default)]
    pub healthcheck: Option<Healthcheck>,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    #[serde(default)]
    pub memory_limit_mb: Option<i64>,
    #[serde(default)]
    pub nano_cpus: Option<i64>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortMapping {
    pub host: u16,
    pub container: u16,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Healthcheck {
    pub test: Vec<String>,
    #[serde(default)]
    pub interval_secs: Option<i64>,
    #[serde(default)]
    pub timeout_secs: Option<i64>,
    #[serde(default)]
    pub retries: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mount {
    /// named volume（只允许默认 local driver，且 driver_opts 为空）
    Volume {
        source: String,
        target: String,
        #[serde(default)]
        read_only: bool,
    },
    /// 宿主绑定挂载（native 形态 + 授权根内）
    Bind {
        source: String,
        target: String,
        #[serde(default)]
        read_only: bool,
    },
}

impl Mount {
    pub fn read_only(&self) -> bool {
        match self {
            Mount::Bind { read_only, .. } | Mount::Volume { read_only, .. } => *read_only,
        }
    }
}

impl ContainerSpec {
    pub fn validate(&self, policy: &Policy, actor: &Actor, tier: Tier) -> Result<(), PolicyError> {
        if tier != Tier::Lifecycle {
            return Err(PolicyError::VerbNotAllowed {
                verb: Verb::ContainerCreate.as_str(),
                tier: tier.max_verb(),
            });
        }
        Policy::check_reserved_labels(actor, &self.labels)?;

        if self.image.trim().is_empty() {
            return Err(PolicyError::BadRequest("image 不能为空".into()));
        }
        if let Some(name) = &self.name {
            validate_object_name(name)?;
        }
        if let Some(restart) = &self.restart {
            let ok = matches!(restart.as_str(), "no" | "always" | "unless-stopped")
                || restart.starts_with("on-failure");
            if !ok {
                return Err(PolicyError::BadRequest(format!(
                    "restart 取值非法: {restart}"
                )));
            }
        }
        for p in &self.ports {
            if p.host == 0 || p.container == 0 {
                return Err(PolicyError::BadRequest("端口必须在 1–65535".into()));
            }
            match p.protocol.as_deref() {
                None | Some("tcp") | Some("udp") => {}
                Some(other) => {
                    return Err(PolicyError::BadRequest(format!(
                        "不支持的端口协议: {other}"
                    )))
                }
            }
        }
        for m in &self.mounts {
            validate_mount_target(m)?;
            match m {
                Mount::Bind { source, .. } => {
                    if policy.form == Form::DockerRestricted {
                        return Err(PolicyError::Restricted(
                            "该形态关闭宿主绑定挂载（容器内路径 ≠ Engine 宿主路径）".into(),
                        ));
                    }
                    policy
                        .path_guard
                        .validate_bind_source(std::path::Path::new(source))
                        .map_err(|e| PolicyError::BadRequest(format!("绑定挂载源被拒: {e}")))?;
                }
                Mount::Volume { source, .. } => validate_object_name(source)?,
            }
        }
        for net in &self.networks {
            validate_object_name(net)?;
        }
        if self.memory_limit_mb.map(|v| v <= 0).unwrap_or(false) {
            return Err(PolicyError::BadRequest("memory_limit_mb 必须为正数".into()));
        }
        if self.nano_cpus.map(|v| v <= 0).unwrap_or(false) {
            return Err(PolicyError::BadRequest("nano_cpus 必须为正数".into()));
        }
        Ok(())
    }
}

fn validate_object_name(name: &str) -> Result<(), PolicyError> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
    if ok {
        Ok(())
    } else {
        Err(PolicyError::BadRequest(format!("对象名非法: {name}")))
    }
}

fn validate_mount_target(m: &Mount) -> Result<(), PolicyError> {
    let target = match m {
        Mount::Bind { target, .. } | Mount::Volume { target, .. } => target,
    };
    if !target.starts_with('/') || target.contains("..") {
        return Err(PolicyError::BadRequest(format!(
            "容器内挂载点必须是绝对路径且不含 ..: {target}"
        )));
    }
    Ok(())
}

/// 校验从 Docker 读到的 named volume 是否带宿主绑定语义（§6.4.3）。
/// `o=bind` / `type=none` / `device=<host-path>` 一律按绑定挂载处理。
pub fn volume_has_host_binding<S: std::hash::BuildHasher>(
    driver: &str,
    opts: &std::collections::HashMap<String, String, S>,
) -> bool {
    if driver != "local" {
        return true;
    }
    let joined = opts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",");
    let lower = joined.to_ascii_lowercase();
    lower.contains("o=bind")
        || lower.contains("type=none")
        || lower.contains("device=")
        || (!opts.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn policy(tier: Tier, form: Form) -> Policy {
        let root = std::env::temp_dir().join("skiff-policy-root");
        std::fs::create_dir_all(&root).unwrap();
        Policy::new(tier, form, vec![root]).unwrap()
    }

    fn user(role: Role) -> Actor {
        Actor::User {
            username: "u".into(),
            role,
        }
    }

    fn obj(name: &str, labels: &[(&str, &str)]) -> ObjectRef {
        ObjectRef {
            id: format!("id-{name}"),
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// 审计动作名必须由 `as_str()` 派生，且覆盖原先手写死的那批（resources.rs 的历史字面量）。
    #[test]
    fn audit_action_is_derived_from_as_str() {
        assert_eq!(Verb::Stop.audit_action(), "docker.containers.stop");
        assert_eq!(Verb::ImagePull.audit_action(), "docker.images.pull");
        assert_eq!(Verb::ImageRemove.audit_action(), "docker.images.remove");
        assert_eq!(Verb::NetworkCreate.audit_action(), "docker.networks.create");
        assert_eq!(Verb::VolumeRemove.audit_action(), "docker.volumes.remove");
        assert_eq!(Verb::Exec.audit_action(), "docker.containers.exec");
        for v in [Verb::ContainersList, Verb::ComposeUp, Verb::Kill] {
            assert_eq!(v.audit_action(), format!("docker.{}", v.as_str()));
        }
    }

    /// `as_str` / `parse` / `audit_action` 三者成对：任何一侧漏改都会被这条抓住。
    #[test]
    fn verb_strings_roundtrip() {
        for &v in Verb::all() {
            let s = v.as_str();
            assert_eq!(Verb::parse(s), Some(v), "as_str({v:?}) = {s} 无法回解");
            assert_eq!(v.audit_action(), format!("docker.{s}"));
            assert!(!s.is_empty() && !s.contains(':'), "动词名必须点分: {s}");
        }
        assert_eq!(Verb::parse("containers.nope"), None);
        assert_eq!(Verb::parse(""), None);
    }

    /// `all()` 是解析表的唯一来源，漏一个动词就等于「白名单里有、解析不了」。
    /// 数量与枚举变体数对齐；新增动词忘了进表时这条会失败。
    #[test]
    fn all_covers_every_verb() {
        assert_eq!(Verb::all().len(), 26, "新增动词必须同时加入 Verb::all()");
        let mut seen: Vec<&str> = Verb::all().iter().map(|v| v.as_str()).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total, "as_str 存在重复命名: {seen:?}");
    }

    #[test]
    fn lifecycle_verbs_require_lifecycle_tier() {
        let p = policy(Tier::Basic, Form::Native);
        let e = p
            .check(&user(Role::Admin), Verb::ContainerCreate, None)
            .unwrap_err();
        assert!(matches!(e, PolicyError::VerbNotAllowed { .. }));
    }

    #[test]
    fn viewer_cannot_operate_operator_cannot_exec() {
        let p = policy(Tier::Lifecycle, Form::Native);
        assert!(p.check(&user(Role::Viewer), Verb::Start, None).is_err());
        assert!(p.check(&user(Role::Operator), Verb::Start, None).is_ok());
        assert!(p.check(&user(Role::Operator), Verb::Exec, None).is_err());
        assert!(p.check(&user(Role::Admin), Verb::Exec, None).is_ok());
    }

    #[test]
    fn skiff_objects_are_protected() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let skiff = obj("skiff", &[]);
        let e = p
            .check(&user(Role::Admin), Verb::Stop, Some(&skiff))
            .unwrap_err();
        assert!(matches!(e, PolicyError::Protected { .. }));
        let flagged = obj("db", &[(LABEL_PROTECTED, "true")]);
        assert!(p
            .check(&user(Role::Admin), Verb::Stop, Some(&flagged))
            .is_err());
    }

    #[test]
    fn plugin_needs_permission_and_object_grant() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![],
        };
        let foreign = obj("redis", &[(LABEL_OWNER, "someone-else")]);
        assert!(p
            .check(&plugin, Verb::ContainerLogs, Some(&foreign))
            .is_err());

        let granted = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![
                Grant::perm("docker:containers.logs"),
                Grant::object("container:id-redis"),
            ],
        };
        assert!(p
            .check(&granted, Verb::ContainerLogs, Some(&foreign))
            .is_ok());
        // read 授权不会自动带来 exec
        assert!(p.check(&granted, Verb::Exec, Some(&foreign)).is_err());

        // 自有资源只读不需要 grant
        let own = obj("mine", &[(LABEL_OWNER, "com.x.demo")]);
        let read_only = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![Grant::perm("docker:containers.inspect")],
        };
        assert!(p
            .check(&read_only, Verb::ContainerInspect, Some(&own))
            .is_ok());
    }

    #[test]
    fn plugin_cannot_touch_protected_even_with_grant() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![
                Grant::perm("docker:containers.stop"),
                Grant::object("container:id-skiff"),
            ],
        };
        let e = p
            .check(&plugin, Verb::Stop, Some(&obj("skiff", &[])))
            .unwrap_err();
        assert!(matches!(e, PolicyError::Protected { .. }));
    }

    #[test]
    fn plugin_cannot_write_foreign_panel_resources() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![
                Grant::perm("docker:containers.remove"),
                Grant::object("container:id-1panel-nginx"),
            ],
        };
        let e = p
            .check(
                &plugin,
                Verb::ContainerRemove,
                Some(&obj("1panel-nginx", &[("createdBy", "Apps")])),
            )
            .unwrap_err();
        assert!(matches!(e, PolicyError::ForeignPanel { .. }));
    }

    #[test]
    fn reserved_labels_rejected() {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_OWNER.to_string(), "com.x.demo".to_string());
        let e = Policy::check_reserved_labels(&user(Role::Admin), &labels).unwrap_err();
        assert!(matches!(e, PolicyError::BadRequest(_)));
    }

    #[test]
    fn spec_rejects_unknown_fields() {
        let json = r#"{"image":"nginx","privileged":true}"#;
        assert!(serde_json::from_str::<ContainerSpec>(json).is_err());
        let json = r#"{"image":"nginx","cap_add":["SYS_ADMIN"]}"#;
        assert!(serde_json::from_str::<ContainerSpec>(json).is_err());
    }

    #[test]
    fn spec_accepts_whitelisted_fields() {
        let json = r#"{
            "image":"nginx:1.27-alpine",
            "name":"web",
            "env":{"TZ":"Asia/Shanghai"},
            "ports":[{"host":18081,"container":80}],
            "restart":"unless-stopped",
            "memory_limit_mb":128
        }"#;
        let spec: ContainerSpec = serde_json::from_str(json).unwrap();
        let p = policy(Tier::Lifecycle, Form::Native);
        spec.validate(&p, &user(Role::Admin), Tier::Lifecycle)
            .unwrap();
    }

    #[test]
    fn bind_mount_outside_root_rejected_and_restricted_mode_rejects_all() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let spec = ContainerSpec {
            image: "nginx".into(),
            name: None,
            command: None,
            entrypoint: None,
            env: BTreeMap::new(),
            labels: BTreeMap::new(),
            ports: vec![],
            restart: None,
            healthcheck: None,
            networks: vec![],
            mounts: vec![Mount::Bind {
                source: if cfg!(windows) {
                    "C:/Windows".into()
                } else {
                    "/etc".into()
                },
                target: "/data".into(),
                read_only: false,
            }],
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        };
        assert!(spec
            .validate(&p, &user(Role::Admin), Tier::Lifecycle)
            .is_err());

        let restricted = policy(Tier::Lifecycle, Form::DockerRestricted);
        let e = spec
            .validate(&restricted, &user(Role::Admin), Tier::Lifecycle)
            .unwrap_err();
        assert!(matches!(e, PolicyError::Restricted(_)));
    }

    #[test]
    fn volume_host_binding_detection() {
        let mut opts = HashMap::new();
        assert!(!volume_has_host_binding("local", &opts));
        opts.insert("o".into(), "bind".into());
        assert!(volume_has_host_binding("local", &opts));
        let mut opts2 = HashMap::new();
        opts2.insert("type".into(), "none".into());
        assert!(volume_has_host_binding("local", &opts2));
        let mut opts3 = HashMap::new();
        opts3.insert("device".into(), "/etc".into());
        assert!(volume_has_host_binding("local", &opts3));
        assert!(volume_has_host_binding("nfs", &HashMap::new()));
    }
}
