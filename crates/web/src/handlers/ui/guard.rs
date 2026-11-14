//! `/ui/v1` 写入口的统一门禁：动作权限与 CSRF。
//!
//! 顺序固定为「先权限、后 CSRF」：无权限的主体立即得到 `auth.forbidden`，
//! 不会因为令牌缺失/失效先看到 `auth.csrf_invalid`。需要读取对象才能判定的
//! 对象级授权（Docker protected 对象）仍放在 CSRF 之后，先用
//! `policy.check(actor, verb, None)` 做角色门，再解析对象做完整判定。

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

/// 常见写入口：动作权限 → CSRF。
pub(super) fn write(
    state: &AppState,
    user: &CurrentUser,
    parts: &Parts,
    action: Action,
) -> Result<(), ApiError> {
    self::action(user, action)?;
    csrf(state, user, parts)
}
