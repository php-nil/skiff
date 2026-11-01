//! 跨 crate 共享的**词汇表**：角色、授权项、容器规格、动词白名单、部署形态。
//!
//! 为什么单独一个 crate：`Role`/`Grant` 只被当作枚举值使用，却因为定义在 `store`
//! 里，导致「策略判定」这种纯计算模块不得不依赖 rusqlite（SQLite bundled）。
//! 拆出后 `skiff-policy` 可以零数据库 / 零 Docker 编译与测试，插件 SDK 侧也不会被迫拖进数据库。
//!
//! 只放**没有行为、没有 I/O** 的东西；领域对象（User/Session/PluginRecord）
//! 留在 `store`，谁存谁定义。
//!
//! 边界：**判定**在 `skiff-policy`（`Policy::check`、`PathGuard`），**词汇**在这里。
//! `impl` 与类型同 crate——所以 `ContainerSpec` 的校验方法是
//! `skiff_policy::Policy::validate_spec`，不是 `ContainerSpec::validate`。

use std::fmt;

mod cidr;
mod container;
mod path;
mod rbac;
mod scope;
mod verb;
pub use cidr::Cidr;
pub use container::{
    short_id, Actor, ContainerSpec, Healthcheck, Mount, ObjectRef, PortMapping, SHORT_ID_LEN,
};
pub use path::{
    any_root_contains, has_parent_segment, hits_a_root_itself, is_absolute, is_posix_absolute,
    is_root_path, normalize_dir, parent_within, within, within_any, within_root,
};
pub use rbac::{Permission, RoleExt};
pub use scope::{Scope, ScopeSet};
pub use verb::{Tier, Verb, LABEL_OWNER, LABEL_PROJECT, LABEL_PROTECTED};

/// RBAC 三种角色（§6.3）。危险操作（插件安装、对象 grant、宿主路径授权、
/// Docker 删除/exec）仅 admin。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 插件授权项。`kind = perm` 时 `value` 形如 `docker:list`；`kind = object` 时
/// `value` 形如 `container:<id>`（§6.4.2 对象 grant）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub kind: String,
    pub value: String,
}

impl Grant {
    pub fn perm(value: impl Into<String>) -> Self {
        Self {
            kind: "perm".into(),
            value: value.into(),
        }
    }
    pub fn object(value: impl Into<String>) -> Self {
        Self {
            kind: "object".into(),
            value: value.into(),
        }
    }
}

/// 部署形态（§5.1）。决定可用能力面与是否允许宿主绑定挂载。
///
/// - `Native`：宿主机 systemd 直装，完整宿主能力（路径白名单 + 绑定挂载）；
/// - `DockerRestricted`：1Panel 商店容器包，能力面收缩为 docker/identity，
///   且**禁止**宿主绑定挂载（否则等于把宿主机 root 交给容器）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Native,
    DockerRestricted,
}

impl Form {
    pub fn as_str(self) -> &'static str {
        match self {
            Form::Native => "native",
            Form::DockerRestricted => "docker-restricted",
        }
    }

    pub fn parse(s: &str) -> Option<Form> {
        match s {
            "native" => Some(Form::Native),
            "docker" | "docker-restricted" => Some(Form::DockerRestricted),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrip_and_order() {
        for r in [Role::Viewer, Role::Operator, Role::Admin] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("root"), None);
        // 权限比较依赖这个顺序（viewer < operator < admin）
        assert!(Role::Viewer < Role::Operator);
        assert!(Role::Operator < Role::Admin);
    }

    #[test]
    fn grant_kinds_are_stable() {
        assert_eq!(Grant::perm("docker:list").kind, "perm");
        assert_eq!(Grant::object("container:abc").value, "container:abc");
    }

    #[test]
    fn form_accepts_both_spellings() {
        assert_eq!(Form::parse("native"), Some(Form::Native));
        assert_eq!(Form::parse("docker"), Some(Form::DockerRestricted));
        assert_eq!(
            Form::parse("docker-restricted"),
            Some(Form::DockerRestricted)
        );
        assert_eq!(Form::parse("k8s"), None);
    }
}
