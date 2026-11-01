//! RBAC：admin / operator / viewer（§6.3）。
//!
//! 危险操作（插件安装、对象 grant、宿主路径授权、Docker 删除/exec/生命周期创建）
//! 一律 admin，并需二次确认或重新认证；本模块只回答「角色能否发起该动作」。
//!
//! 放在 model 而不是 identity：策略判定（`skiff-policy`）需要角色→动作的映射，
//! 但不需要口令哈希、TOTP、数据库。identity 仍以 `pub use` 转出，调用方无需改。
//!
//! **这张表是全仓唯一的角色→权限矩阵**：判定（`skiff-policy`）、UI 按钮显隐
//! （`skiff-web`）都查它。此前 `policy` 内联写了一份 `Action` 三分法、web 侧
//! 又手写了 `is_admin() || role == Operator`，三份规则各改各的。

use crate::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// 读容器/镜像/网络/卷/日志/统计
    DockerRead,
    /// 启动/停止/重启/终止容器
    DockerOperate,
    /// 创建/删除容器、镜像、网络、卷（生命周期档，M3）
    DockerLifecycle,
    /// 容器 exec 终端
    DockerExec,
    /// 读取审计
    AuditRead,
    /// 插件安装/卸载/授权
    PluginManage,
    /// 宿主绑定挂载授权根管理
    HostPathGrant,
    /// 用户与设置管理
    Admin,
}

impl Permission {
    /// 稳定字符串：审计与错误信息用。
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::DockerRead => "docker.read",
            Permission::DockerOperate => "docker.operate",
            Permission::DockerLifecycle => "docker.lifecycle",
            Permission::DockerExec => "docker.exec",
            Permission::AuditRead => "audit.read",
            Permission::PluginManage => "plugin.manage",
            Permission::HostPathGrant => "hostpath.grant",
            Permission::Admin => "admin",
        }
    }
}

pub trait RoleExt {
    fn can(self, perm: Permission) -> bool;
    fn is_admin(&self) -> bool;
}

impl RoleExt for Role {
    fn can(self, perm: Permission) -> bool {
        use Permission::*;
        matches!(
            (self, perm),
            (Role::Admin, _)
                | (Role::Operator, DockerRead | DockerOperate | AuditRead)
                | (Role::Viewer, DockerRead)
        )
    }

    fn is_admin(&self) -> bool {
        *self == Role::Admin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_is_read_only() {
        assert!(Role::Viewer.can(Permission::DockerRead));
        assert!(!Role::Viewer.can(Permission::DockerOperate));
        assert!(!Role::Viewer.can(Permission::DockerExec));
    }

    #[test]
    fn operator_can_operate_but_not_exec_or_install() {
        assert!(Role::Operator.can(Permission::DockerOperate));
        assert!(!Role::Operator.can(Permission::DockerExec));
        assert!(!Role::Operator.can(Permission::PluginManage));
        assert!(!Role::Operator.can(Permission::DockerLifecycle));
    }

    #[test]
    fn admin_can_everything() {
        for p in [
            Permission::DockerRead,
            Permission::DockerOperate,
            Permission::DockerLifecycle,
            Permission::DockerExec,
            Permission::PluginManage,
            Permission::HostPathGrant,
            Permission::Admin,
        ] {
            assert!(Role::Admin.can(p));
        }
    }
}
