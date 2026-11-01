//! 策略判定引擎：动词白名单 → 角色/插件权限 → 对象授权 → 参数字段白名单。
//!
//! **角色→权限的映射只有一处**：`skiff_model::Permission` + `Role::can()`。
//! 此前本文件内联写过一份 `Action` 三分法（`Read/Operate/Exec/Lifecycle`），
//! 与 `model::rbac` 的 `Permission` 表并行演化；现在改为直接查表。

use std::collections::BTreeMap;
use std::path::PathBuf;

use skiff_model::{
    has_parent_segment, is_posix_absolute, Actor, ContainerSpec, Form, Mount, ObjectRef, RoleExt,
    Tier, Verb, LABEL_OWNER, LABEL_PROJECT, LABEL_PROTECTED,
};

use crate::paths::{PathError, PathGuard};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyError {
    /// 动词不在白名单（或当前分档未放开）
    VerbNotAllowed {
        verb: &'static str,
        tier: &'static str,
    },
    /// 角色不具备该动作权限
    RoleDenied {
        role: String,
        permission: &'static str,
    },
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
            PolicyError::RoleDenied { role, permission } => {
                write!(f, "角色 {role} 无权执行 {permission}")
            }
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
                // 角色矩阵的唯一来源：`skiff_model::Permission`。
                let perm = verb.permission();
                if !role.can(perm) {
                    return Err(PolicyError::RoleDenied {
                        role: role.to_string(),
                        permission: perm.as_str(),
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

    /// 第三层：参数字段白名单的**校验**。
    ///
    /// 方法挂在 `Policy` 而不是 `ContainerSpec` 上：类型住在 `skiff-model`，
    /// 而「impl 必须与类型同 crate」是仓库红线——跨 crate 的 inherent impl 做不到。
    ///
    /// 分档取 `self.tier`（不再单独传一个 tier 参数）：两处取值必须一致，
    /// 传参版本允许调用方把「策略分档」和「校验分档」写成两个不同的值。
    pub fn validate_spec(&self, spec: &ContainerSpec, actor: &Actor) -> Result<(), PolicyError> {
        if self.tier != Tier::Lifecycle {
            return Err(PolicyError::VerbNotAllowed {
                verb: Verb::ContainerCreate.as_str(),
                tier: self.tier.max_verb(),
            });
        }
        Policy::check_reserved_labels(actor, &spec.labels)?;

        if spec.image.trim().is_empty() {
            return Err(PolicyError::BadRequest("image 不能为空".into()));
        }
        if let Some(name) = &spec.name {
            validate_object_name(name)?;
        }
        if let Some(restart) = &spec.restart {
            let ok = matches!(restart.as_str(), "no" | "always" | "unless-stopped")
                || restart.starts_with("on-failure");
            if !ok {
                return Err(PolicyError::BadRequest(format!(
                    "restart 取值非法: {restart}"
                )));
            }
        }
        for p in &spec.ports {
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
        for m in &spec.mounts {
            validate_mount_target(m)?;
            match m {
                Mount::Bind { source, .. } => {
                    if self.form == Form::DockerRestricted {
                        return Err(PolicyError::Restricted(
                            "该形态关闭宿主绑定挂载（容器内路径 ≠ Engine 宿主路径）".into(),
                        ));
                    }
                    self.path_guard
                        .validate_bind_source(std::path::Path::new(source))
                        .map_err(|e| PolicyError::BadRequest(format!("绑定挂载源被拒: {e}")))?;
                }
                Mount::Volume { source, .. } => validate_object_name(source)?,
            }
        }
        for net in &spec.networks {
            validate_object_name(net)?;
        }
        if spec.memory_limit_mb.map(|v| v <= 0).unwrap_or(false) {
            return Err(PolicyError::BadRequest("memory_limit_mb 必须为正数".into()));
        }
        if spec.nano_cpus.map(|v| v <= 0).unwrap_or(false) {
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
    if !is_posix_absolute(target) || has_parent_segment(target) {
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
    use skiff_model::Role;
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
            id: "id".into(),
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn spec() -> ContainerSpec {
        ContainerSpec {
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
            mounts: vec![],
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
        }
    }

    #[test]
    fn tier_gates_lifecycle_verbs() {
        let p = policy(Tier::Basic, Form::Native);
        assert!(p
            .check(&user(Role::Admin), Verb::ContainerCreate, None)
            .is_err());
        let p = policy(Tier::Lifecycle, Form::Native);
        assert!(p
            .check(&user(Role::Admin), Verb::ContainerCreate, None)
            .is_ok());
    }

    /// 角色矩阵的唯一来源是 `Permission` 表：viewer 只读、operator 可启停、
    /// exec 与生命周期 admin 专属。改表即改判定。
    #[test]
    fn role_matrix_is_enforced() {
        let p = policy(Tier::Lifecycle, Form::Native);
        assert!(p.check(&user(Role::Viewer), Verb::Start, None).is_err());
        assert!(p.check(&user(Role::Operator), Verb::Start, None).is_ok());
        assert!(p.check(&user(Role::Operator), Verb::Exec, None).is_err());
        assert!(p.check(&user(Role::Admin), Verb::Exec, None).is_ok());
        assert!(
            p.check(&user(Role::Operator), Verb::ContainerRemove, None)
                .is_err(),
            "生命周期动词 admin 专属"
        );
    }

    #[test]
    fn protected_object_blocks_state_change() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let skiff = obj("skiff", &[]);
        assert!(p
            .check(&user(Role::Admin), Verb::Stop, Some(&skiff))
            .is_err());
        let flagged = obj("web", &[(LABEL_PROTECTED, "true")]);
        assert!(p
            .check(&user(Role::Admin), Verb::Stop, Some(&flagged))
            .is_err());
        let normal = obj("web", &[]);
        assert!(p
            .check(&user(Role::Admin), Verb::Stop, Some(&normal))
            .is_ok());
    }

    #[test]
    fn plugin_needs_explicit_object_grant() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![skiff_model::Grant::perm("docker:containers.logs")],
        };
        let foreign = obj("web", &[]);
        assert!(
            p.check(&plugin, Verb::ContainerLogs, Some(&foreign))
                .is_err(),
            "无对象 grant 不能读别人的容器"
        );
        let granted = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![
                skiff_model::Grant::perm("docker:containers.logs"),
                skiff_model::Grant::object("container:web"),
            ],
        };
        assert!(p
            .check(&granted, Verb::ContainerLogs, Some(&foreign))
            .is_ok());
        assert!(
            p.check(&granted, Verb::Exec, Some(&foreign)).is_err(),
            "exec 不因 read 授权自动获得"
        );
    }

    #[test]
    fn owned_container_is_readable_without_grant() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![skiff_model::Grant::perm("docker:containers.logs")],
        };
        let own = obj("web", &[(LABEL_OWNER, "com.x.demo")]);
        assert!(p.check(&plugin, Verb::ContainerLogs, Some(&own)).is_ok());
    }

    #[test]
    fn read_only_grant_cannot_stop() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let plugin = Actor::Plugin {
            id: "com.x.demo".into(),
            grants: vec![skiff_model::Grant::perm("docker:containers.inspect")],
        };
        let own = obj("web", &[(LABEL_OWNER, "com.x.demo")]);
        assert!(
            p.check(&plugin, Verb::Stop, Some(&own)).is_err(),
            "自有对象也只对非破坏性动词放行"
        );
    }

    #[test]
    fn reserved_labels_rejected() {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_OWNER.to_string(), "com.x.demo".to_string());
        let e = Policy::check_reserved_labels(&user(Role::Admin), &labels).unwrap_err();
        assert!(matches!(e, PolicyError::BadRequest(_)));
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
        p.validate_spec(&spec, &user(Role::Admin)).unwrap();
    }

    #[test]
    fn basic_tier_rejects_spec() {
        let p = policy(Tier::Basic, Form::Native);
        assert!(p.validate_spec(&spec(), &user(Role::Admin)).is_err());
    }

    #[test]
    fn bind_mount_outside_root_rejected_and_restricted_mode_rejects_all() {
        let p = policy(Tier::Lifecycle, Form::Native);
        let mut spec = spec();
        spec.mounts = vec![Mount::Bind {
            source: if cfg!(windows) {
                "C:/Windows".into()
            } else {
                "/etc".into()
            },
            target: "/data".into(),
            read_only: false,
        }];
        assert!(p.validate_spec(&spec, &user(Role::Admin)).is_err());

        let restricted = policy(Tier::Lifecycle, Form::DockerRestricted);
        let e = restricted
            .validate_spec(&spec, &user(Role::Admin))
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
