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

mod audit;
mod cidr;
mod container;
mod error;
mod exec;
mod path;
mod rbac;
mod scope;
mod site;
pub mod url;
mod verb;
pub use audit::AuditCategory;
pub use cidr::Cidr;
pub use container::{
    short_id, Actor, ContainerSpec, Healthcheck, Mount, ObjectRef, PortMapping, ResourceLimits,
    SHORT_ID_LEN,
};
pub use error::{ErrorCategory, ErrorCode};
pub use exec::{bin_name, bin_risk, is_root_equivalent, is_wrapper_bin, BinRisk, BIN_PREFIX};
pub use path::{
    any_root_contains, has_parent_component, has_parent_segment, hits_a_root_itself, is_absolute,
    is_posix_absolute, is_root_path, normalize_dir, parent_within, within, within_any, within_root,
};
pub use rbac::{Permission, RoleExt};
pub use scope::{Scope, ScopeSet};
pub use site::{is_site_index_path, SITE_GID, SITE_INDEX_DIR, SITE_UID, SITE_USER};
pub use verb::{Tier, Verb, LABEL_OWNER, LABEL_PROJECT, LABEL_PROTECTED, PROJECT_SITES};

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

    /// 精确命中授权项：`kind = "perm"` 且 `value` 与请求完全一致。
    pub fn permits(&self, perm: &str) -> bool {
        self.kind == "perm" && self.value == perm
    }

    /// 精确命中，或命中能力面级通配授权 `<cap>:*`。
    ///
    /// 通配目前**只**供宿主变更类能力面（ftp / firewall）使用：它们的动作集合是
    /// 固定的小集合，安装确认时允许一次授予整面。其余能力面（fs / docker /
    /// scheduler 等）必须逐条授权——这是现行规范，不是匹配漏项；扩面须先改契约。
    pub fn permits_wildcard(&self, perm: &str) -> bool {
        if self.permits(perm) {
            return true;
        }
        let Some((cap, _)) = perm.split_once(':') else {
            return false;
        };
        self.kind == "perm" && self.value == format!("{cap}:*")
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
    fn grant_permits_matches_exact_and_wildcard_only_for_same_capability() {
        assert!(Grant::perm("ftp:account.list").permits("ftp:account.list"));
        assert!(!Grant::perm("ftp:account.list").permits("ftp:account.add"));
        assert!(!Grant::perm("ftp:*").permits("firewall:list"));

        assert!(Grant::perm("ftp:*").permits_wildcard("ftp:account.add"));
        assert!(Grant::perm("ftp:account.add").permits_wildcard("ftp:account.add"));
        assert!(!Grant::perm("ftp:*").permits_wildcard("firewall:list"));
        assert!(!Grant::perm("firewall:*").permits_wildcard("ftp:account.add"));
        // 非 perm 授权项（对象授权）不参与权限判定。
        assert!(!Grant::object("ftp:*").permits_wildcard("ftp:account.add"));
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
