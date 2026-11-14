//! `/ui/v1` 写入口的统一门禁：CSRF 与动作权限。
//!
//! 绝大多数写路径是「先 CSRF、后权限」；这里把两段固定下来，避免各 handler
//! 重复拼 `check_csrf_header` + `authorize`。极少数需要「权限先行」的端点
//! （如公钥导入/撤销）继续分别调用 [`permission`] / [`action`]，不在这里强行
//! 重排，保证错误返回顺序与拆分前一致。

use axum::http::request::Parts;
use skiff_app::actions::Action;

use crate::error::ApiError;
use crate::state::{AppState, CurrentUser};

/// 写请求先过 `x-csrf-token`。
pub(super) fn csrf(state: &AppState, user: &CurrentUser, parts: &Parts) -> Result<(), ApiError> {
    crate::handlers::check_csrf_header(state, user, parts).map_err(ApiError)
}

/// 按动作注册表判定权限（错误映射为 JSON 信封）。
pub(super) fn action(user: &CurrentUser, action: Action) -> Result<(), ApiError> {
    crate::authz::authorize(user, action)
        .map(|_| ())
        .map_err(ApiError)
}

/// 直接按权限矩阵判定（插件管理、管理员等无独立动作的入口）。
pub(super) fn permission(
    user: &CurrentUser,
    permission: skiff_model::Permission,
) -> Result<(), ApiError> {
    crate::handlers::require_permission(user, permission).map_err(ApiError)
}

/// 常见写入口：CSRF → 动作权限。
pub(super) fn write(
    state: &AppState,
    user: &CurrentUser,
    parts: &Parts,
    action: Action,
) -> Result<(), ApiError> {
    csrf(state, user, parts)?;
    self::action(user, action)
}
