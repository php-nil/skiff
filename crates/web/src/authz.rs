//! 授权唯一入口：**动作注册表 → 权限 → 判定**。
//!
//! 写路径只调用 [`authorize`]，不再各自写 `require_admin` / 角色比较；
//! `user.can` 仅保留给模板做按钮显隐（展示），不承担拦截。
//!
//! 判定本体仍是 [`handlers::require_permission`]，它读 `skiff_model::Permission`
//! 这张唯一矩阵；这里只是把「哪个动作需要什么权限」交给注册表。
//!
//! 参数独立成 [`authorize_for`] 供菜单等只有 `Permission` 的场景复用，但动作
//! 型写路径应优先用它，保证注册表与判定同源。

use skiff_app::actions::{self, Action, ActionSpec};
use skiff_model::Permission;

use crate::error::AppError;
use crate::handlers;
use crate::state::CurrentUser;

/// 校验当前主体能否执行动作；成功返回动作声明，便于审计与后续分发。
pub fn authorize(user: &CurrentUser, action: Action) -> Result<&'static ActionSpec, AppError> {
    let spec = actions::spec(action);
    if let Some(permission) = spec.permission {
        authorize_for(user, permission)?;
    }
    Ok(spec)
}

/// 按权限直接判定（无对应动作的读取页/菜单过滤使用）。
pub fn authorize_for(user: &CurrentUser, permission: Permission) -> Result<(), AppError> {
    handlers::require_permission(user, permission)
}

/// 展示用：不产生错误，供页面计算「可见动作」。不能替代 [`authorize`]。
pub fn allowed(user: &CurrentUser, action: Action) -> bool {
    actions::spec(action).allows(user.role())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_app::actions::*;
    use skiff_model::Role;
    use skiff_store::User;

    fn user(role: Role) -> CurrentUser {
        CurrentUser {
            user: User {
                id: 1,
                username: format!("{role:?}"),
                password_hash: String::new(),
                role,
                totp_secret: None,
                totp_enabled: false,
                must_change_password: false,
                created_at: 0,
            },
            session_id: "test-session".into(),
            session_token: "test-token".into(),
            scopes: None,
        }
    }

    fn is_forbidden<T>(result: Result<T, AppError>) -> bool {
        matches!(result, Err(AppError::Forbidden(_)))
    }

    #[test]
    fn viewer_cannot_execute_admin_or_operator_actions() {
        let viewer = user(Role::Viewer);
        assert!(is_forbidden(authorize(&viewer, SITE_DELETE)));
        assert!(is_forbidden(authorize(&viewer, FILE_WRITE)));
        assert!(is_forbidden(authorize(&viewer, CONTAINER_START)));
        assert!(!allowed(&viewer, PLUGIN_INSTALL));
    }

    #[test]
    fn operator_can_operate_but_not_destroy_or_exec() {
        let operator = user(Role::Operator);
        assert!(authorize(&operator, CONTAINER_START).is_ok());
        assert!(authorize(&operator, CONTAINER_STOP).is_ok());
        assert!(is_forbidden(authorize(&operator, CONTAINER_REMOVE)));
        assert!(is_forbidden(authorize(&operator, CONTAINER_EXEC)));
        assert!(is_forbidden(authorize(&operator, SITE_CREATE)));
        assert!(is_forbidden(authorize(&operator, PLUGIN_INSTALL)));
    }

    #[test]
    fn admin_can_execute_registered_privileged_actions() {
        let admin = user(Role::Admin);
        for spec in ACTIONS {
            assert!(
                authorize(&admin, spec.action).is_ok(),
                "Admin 被拒绝了已登记动作 {}",
                spec.id
            );
        }
    }

    #[test]
    fn extractor_protected_actions_match_extractors() {
        // 这些动作的 handler 用 `AdminUser` 提取器守门：注册表必须是 Admin，
        // 否则提取器与动作端点会给出两种结论。
        for action in [
            SETTINGS_PANEL_UPDATE,
            SETTINGS_KEY_IMPORT,
            SETTINGS_KEY_REMOVE,
            SETTINGS_DOCKER_DAEMON_SAVE,
        ] {
            assert_eq!(
                actions::spec(action).permission,
                Some(Permission::Admin),
                "{} 与 AdminUser 提取器口径不一致",
                action
            );
        }
        // 插件写路径用 `PluginManagerUser` 提取器守门。
        for action in [PLUGIN_INSTALL, PLUGIN_CONFIRM, PLUGIN_TOGGLE, PLUGIN_REMOVE] {
            assert_eq!(
                actions::spec(action).permission,
                Some(Permission::PluginManage),
                "{} 与 PluginManagerUser 提取器口径不一致",
                action
            );
        }
    }

    #[test]
    fn converted_handler_groups_match_old_admin_helper() {
        // 站点/运行时/文件/任务/网站配置/作业原先统一走 `require_admin`，
        // 收口后必须仍是 Admin。
        for action in [
            SITE_CREATE,
            RUNTIME_CREATE,
            FILE_BROWSE,
            TASK_TOGGLE,
            WEB_CONFIG_MAIN_SAVE,
            JOB_CANCEL,
        ] {
            assert_eq!(
                actions::spec(action).permission,
                Some(Permission::Admin),
                "{} 与旧 require_admin 口径不一致",
                action
            );
        }
    }
}
