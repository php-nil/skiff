//! 内核「文件管理」页与它的 JSON API（第一步：浏览 + 编辑 + 基础变更）。
//!
//! IO 全部走 [`skiff_app::admin_fs::AdminFs`] → `skiff-fs`：授权、原子写、路径锁、
//! 审计都在下层完成。这里只做 HTTP 形状：页面要求管理员，写操作要求 CSRF 头；
//! 文件错误按 `FsError` 的稳定 `code()` 透出，前端据此给明确提示。
//!
//! 回收站与配额：第二步接 SQLite 后，删除语义从 `permanent = true` 换成回收站；
//! 本页不需要改路由，只换 `AdminFs::remove` 的实现。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use serde_json::{json, Value};

use skiff_app::admin_fs::FsErr;
use skiff_model::Permission;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::FilesPage;

type JsonErr = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, JsonErr>;

fn app_err(e: AppError) -> JsonErr {
    (
        e.status(),
        Json(json!({
            "ok": false,
            "error": { "code": e.status().as_u16().to_string(), "message": e.message() }
        })),
    )
}

/// `FsError` → 前端可读的 JSON：HTTP 状态来自契约映射，`code` 是稳定字符串。
fn fs_err(e: FsErr) -> JsonErr {
    (
        StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({
            "ok": false,
            "error": { "code": e.code(), "message": e.message() }
        })),
    )
}

fn bad_request(message: impl Into<String>) -> JsonErr {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "ok": false,
            "error": { "code": "files.bad_request", "message": message.into() }
        })),
    )
}

fn admin(user: &crate::state::CurrentUser) -> Result<(), JsonErr> {
    handlers::require_permission(user, Permission::Admin).map_err(app_err)
}

fn csrf(state: &AppState, user: &crate::state::CurrentUser, parts: &Parts) -> Result<(), JsonErr> {
    handlers::check_csrf_header(state, user, parts).map_err(app_err)
}

fn body_json(raw: &str) -> Result<Value, JsonErr> {
    serde_json::from_str(raw).map_err(|e| bad_request(format!("请求体不是合法 JSON: {e}")))
}

fn field<'a>(body: &'a Value, key: &str) -> Result<&'a str, JsonErr> {
    body.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request(format!("缺少字段 {key}")))
}

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    handlers::require_permission(&user, Permission::Admin)?;
    let start_path = q
        .get("path")
        .filter(|p| p.starts_with('/'))
        .cloned()
        .unwrap_or_else(|| state.cfg.web_root.display().to_string());
    let page = FilesPage {
        ctx: handlers::page_ctx(&state, &user, "core/files"),
        start_path,
    };
    handlers::render(page)
}

pub async fn api_list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin(&user)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    match state.admin_fs().list(&user.user.username, path) {
        Ok(listing) => Ok(Json(json!({ "ok": true, "listing": listing }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_read(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin(&user)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    match state.admin_fs().read(&user.user.username, path) {
        Ok(file) => Ok(Json(json!({ "ok": true, "file": file }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_write(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let content = field(&body, "content")?;
    match state.admin_fs().write(&user.user.username, path, content) {
        Ok(out) => Ok(Json(json!({ "ok": true, "result": out }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_mkdir(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    match state.admin_fs().mkdir(&user.user.username, path) {
        Ok(path) => Ok(Json(json!({ "ok": true, "path": path }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_rename(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let from = field(&body, "from")?;
    let to = field(&body, "to")?;
    match state.admin_fs().rename(&user.user.username, from, to) {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_remove(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    match state.admin_fs().remove(&user.user.username, path) {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(fs_err(e)),
    }
}
