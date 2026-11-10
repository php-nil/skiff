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

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream;
use serde_json::{json, Value};

use skiff_app::actions as act;
use skiff_app::admin_fs::{FsErr, FsPathKind, MAX_EDIT_BYTES};
use skiff_app::commands::file as file_cmd;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::{FilesPage, FilesRootView, FsQuotasPage, FsTrashPage};

/// 上传/下载的单片字节数（与 `skiff-fs` 的单次读写上限一致）。
const CHUNK_BYTES: usize = 256 * 1024;

type JsonErr = (StatusCode, Json<Value>);
type ApiResult = Result<Json<Value>, JsonErr>;

/// 旧页面内 JSON 前缀 `/{p}/files/api/*` 的兼容跳转：308 保留方法与请求体。
///
/// 规范路径见 [docs/URL_CONTRACT.md](../../../../docs/URL_CONTRACT.md) §4：
/// 页面内 JSON 接口与页面同构，不再额外插 `api` 段。
pub async fn legacy_api_redirect(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    uri: Uri,
) -> Response {
    let mut target = format!("/{}/files/{}", state.cfg.entry_prefix, rest);
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(q);
    }
    (StatusCode::PERMANENT_REDIRECT, [(LOCATION, target)]).into_response()
}

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

fn admin(user: &crate::state::CurrentUser, action: act::Action) -> Result<(), JsonErr> {
    crate::authz::authorize(user, action)
        .map(|_| ())
        .map_err(app_err)
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
    crate::authz::authorize(&user, act::FILE_BROWSE)?;
    let start_path = q
        .get("path")
        .filter(|p| p.starts_with('/'))
        .cloned()
        .unwrap_or_else(|| state.cfg.web_root.display().to_string());
    let fs = state.admin_fs_arc();
    let roots = handlers::blocking_fs(move || fs.roots()).await?;
    let page = FilesPage {
        ctx: handlers::page_ctx(&state, &user, "core/files"),
        fs_tab: "browse".into(),
        start_path,
        roots: roots
            .into_iter()
            .map(|r| FilesRootView {
                label: r.label,
                path: r.path,
            })
            .collect(),
        max_edit_bytes: MAX_EDIT_BYTES,
        upload_chunk_bytes: CHUNK_BYTES,
    };
    handlers::render(page)
}

/// 目录树懒展开：只返回子目录名，前端拼成绝对路径。
pub async fn api_tree(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin(&user, act::FILE_BROWSE)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    let requested = path.to_string();
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = requested.clone();
    match handlers::blocking_fs(move || {
        file_cmd::list_dirs(&fs, file_cmd::ListDirs { user: owner, path })
    })
    .await
    .map_err(app_err)?
    {
        Ok(dirs) => Ok(Json(json!({ "ok": true, "path": requested, "dirs": dirs }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 分片上传：`offset` 与磁盘当前大小必须一致；首片按原子替换创建。
pub async fn api_upload(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
    parts: Parts,
    body: Bytes,
) -> ApiResult {
    admin(&user, act::FILE_UPLOAD)?;
    csrf(&state, &user, &parts)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    let offset: u64 = q
        .get("offset")
        .ok_or_else(|| bad_request("缺少 offset 参数"))?
        .parse()
        .map_err(|_| bad_request("offset 必须是非负整数"))?;
    if body.len() > CHUNK_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "ok": false,
                "error": {
                    "code": "files.too_large",
                    "message": format!("单次上传分片不能超过 {} KiB", CHUNK_BYTES / 1024)
                }
            })),
        ));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::upload(
            &fs,
            file_cmd::UploadChunk {
                user: owner,
                path,
                offset,
                data: &body,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(out) => Ok(Json(json!({ "ok": true, "result": out }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 单文件下载：流式按 256 KiB 区间读取，不在内存里整文件缓冲。
pub async fn api_download(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, JsonErr> {
    admin(&user, act::FILE_BROWSE)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let target = path.to_string();
    let stat = handlers::blocking_fs(move || {
        file_cmd::stat(
            &fs,
            file_cmd::StatPath {
                user: owner,
                path: target,
            },
        )
    })
    .await
    .map_err(app_err)?
    .map_err(fs_err)?;
    if stat.kind == FsPathKind::Dir {
        return download_dir(state, user.user.username.clone(), path).await;
    }
    if stat.kind != FsPathKind::File {
        return Err(bad_request("只支持下载普通文件"));
    }

    let name = path
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download");
    let content_type = mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    stream_file(
        state,
        user.user.username,
        path.to_string(),
        stat.size,
        name.to_string(),
        content_type,
        None,
    )
}

/// 目录打包下载：复用 `archive` 落一个临时 zip，再用**同一条下载流**输出；
/// `TempDir` 随流生命周期释放（客户端断开也会清理）。
async fn download_dir(state: Arc<AppState>, user: String, path: &str) -> Result<Response, JsonErr> {
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download");
    let name = format!("{base}.zip");
    let dir = tempfile::tempdir().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "ok": false,
                "error": { "code": "files.temp", "message": format!("创建下载临时目录失败: {e}") }
            })),
        )
    })?;
    let dest = dir.path().join("download.zip");
    let dest_str = dest.display().to_string();
    let archive = {
        let st = Arc::clone(&state);
        let user = user.clone();
        let source = path.to_string();
        tokio::task::spawn_blocking(move || {
            file_cmd::archive(
                st.admin_fs(),
                file_cmd::ArchiveEntries {
                    user,
                    sources: vec![source],
                    dest: dest_str,
                    format: "zip".into(),
                    overwrite: false,
                },
            )
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": { "code": "files.temp", "message": format!("打包任务失败: {e}") }
                })),
            )
        })?
        .map_err(fs_err)?
    };
    stream_file(
        state,
        user,
        dest.display().to_string(),
        archive.bytes,
        name,
        "application/zip".to_string(),
        Some(dir),
    )
}

/// 同一条流式下载：按 256 KiB 区间读，不在内存整文件缓冲；`keep` 为临时目录守卫。
#[allow(clippy::too_many_arguments)]
fn stream_file(
    state: Arc<AppState>,
    user: String,
    path: String,
    size: u64,
    name: String,
    content_type: String,
    keep: Option<tempfile::TempDir>,
) -> Result<Response, JsonErr> {
    let owner = user;
    let source = path;
    let chunks = stream::try_unfold(
        (Arc::clone(&state), owner, source, 0u64, keep),
        |(st, user, path, offset, keep)| async move {
            let st2 = Arc::clone(&st);
            let user2 = user.clone();
            let path2 = path.clone();
            let read = tokio::task::spawn_blocking(move || {
                st2.admin_fs()
                    .read_chunk(&user2, &path2, offset, CHUNK_BYTES)
            })
            .await
            .map_err(|e| std::io::Error::other(format!("下载读取任务失败: {e}")))?;
            let chunk = read.map_err(|e| std::io::Error::other(e.message().to_string()))?;
            if chunk.bytes.is_empty() {
                return Ok::<_, std::io::Error>(None);
            }
            let next = offset + chunk.bytes.len() as u64;
            Ok(Some((
                Bytes::from(chunk.bytes),
                (st, user, path, next, keep),
            )))
        },
    );

    let body = Body::from_stream(chunks);
    let disposition = axum::http::HeaderValue::from_str(&content_disposition(&name))
        .map_err(|_| bad_request("文件名包含无法编码的字符"))?;
    let mut resp = (
        StatusCode::OK,
        [
            (CONTENT_TYPE, content_type),
            (CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response();
    resp.headers_mut().insert(CONTENT_DISPOSITION, disposition);
    Ok(resp)
}

/// `attachment; filename="x"`（ASCII）或 RFC 5987 `filename*=UTF-8''…`（非 ASCII）。
fn content_disposition(name: &str) -> String {
    let plain = name
        .chars()
        .all(|c| c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\');
    if plain {
        return format!("attachment; filename=\"{name}\"");
    }
    let mut enc = String::new();
    for b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(*b, b'-' | b'_' | b'.' | b'~') {
            enc.push(*b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename*=UTF-8''{enc}")
}

pub async fn api_list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin(&user, act::FILE_BROWSE)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::list(&fs, file_cmd::ListDir { user: owner, path })
    })
    .await
    .map_err(app_err)?
    {
        Ok(listing) => Ok(Json(json!({ "ok": true, "listing": listing }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_read(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult {
    admin(&user, act::FILE_READ)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| bad_request("缺少 path 参数"))?;
    let path = path.to_string();
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || file_cmd::read(&fs, file_cmd::ReadFile { user, path }))
        .await
        .map_err(app_err)?
    {
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
    admin(&user, act::FILE_WRITE)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?.to_string();
    let content = field(&body, "content")?.to_string();
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::write(
            &fs,
            file_cmd::WriteFile {
                user,
                path,
                content,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
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
    admin(&user, act::FILE_MKDIR)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::mkdir(&fs, file_cmd::MakeDir { user: owner, path })
    })
    .await
    .map_err(app_err)?
    {
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
    admin(&user, act::FILE_RENAME)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let from = field(&body, "from")?;
    let to = field(&body, "to")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let from = from.to_string();
    let to = to.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::rename(
            &fs,
            file_cmd::RenameEntry {
                user: owner,
                from,
                to,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
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
    admin(&user, act::FILE_REMOVE)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?.to_string();
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || file_cmd::remove(&fs, file_cmd::RemoveEntry { user, path }))
        .await
        .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 复制/移动：`move` 即同盘 `rename`，两者都只走 `skiff-fs`。
pub async fn api_copy(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_COPY)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let from = field(&body, "from")?.to_string();
    let to = field(&body, "to")?.to_string();
    let overwrite = body
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::copy(
            &fs,
            file_cmd::CopyEntry {
                user,
                from,
                to,
                overwrite,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 权限修改：`mode` 接受数字或八进制字符串（如 `"0755"` / `"0o2775"`）。
pub async fn api_chmod(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_CHMOD)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let mode = mode_field(&body, "mode")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::chmod(
            &fs,
            file_cmd::ChmodEntry {
                user: owner,
                path,
                mode,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 属主修改：任意 `uid:gid`；uid 0 由下层审计单独标注，前端负责提示。
pub async fn api_chown(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_CHOWN)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let uid = u32_field(&body, "uid")?;
    let gid = u32_field(&body, "gid")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::chown(
            &fs,
            file_cmd::ChownEntry {
                user: owner,
                path,
                uid,
                gid,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 压缩：源为绝对路径数组，目标为压缩包绝对路径。
pub async fn api_archive(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_ARCHIVE)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let sources = string_array(&body, "sources")?;
    if sources.is_empty() {
        return Err(bad_request("sources 不能为空"));
    }
    let dest = field(&body, "dest")?.to_string();
    let format = field(&body, "format")?.to_string();
    let overwrite = body
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::archive(
            &fs,
            file_cmd::ArchiveEntries {
                user,
                sources,
                dest,
                format,
                overwrite,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 解压：格式由压缩包扩展名决定，目标目录必须已存在。
pub async fn api_extract(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_EXTRACT)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let archive = field(&body, "archive")?.to_string();
    let dest = field(&body, "dest")?.to_string();
    let overwrite = body
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::extract(
            &fs,
            file_cmd::ExtractArchive {
                user,
                archive,
                dest,
                overwrite,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(result) => Ok(Json(json!({ "ok": true, "result": result }))),
        Err(e) => Err(fs_err(e)),
    }
}

// ---- 回收站 -------------------------------------------------------------

pub async fn trash_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::FILE_TRASH_READ)?;
    let page = FsTrashPage {
        ctx: handlers::page_ctx(&state, &user, "core/files/trash"),
        fs_tab: "trash".into(),
        retention_days: state.cfg.trash_retention_days,
    };
    handlers::render(page)
}

pub async fn api_trash_list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> ApiResult {
    admin(&user, act::FILE_TRASH_READ)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::trash_list(&fs, file_cmd::TrashList { user: owner })
    })
    .await
    .map_err(app_err)?
    {
        Ok(items) => Ok(Json(json!({
            "ok": true,
            "items": items,
            "retention_days": state.cfg.trash_retention_days,
        }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_trash_restore(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_TRASH_RESTORE)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let id = field(&body, "id")?.to_string();
    let overwrite = body
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::trash_restore(
            &fs,
            file_cmd::TrashRestore {
                user,
                id,
                overwrite,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(out) => Ok(Json(json!({ "ok": true, "result": out }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_trash_purge(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_TRASH_PURGE)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let id = field(&body, "id")?.to_string();
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::trash_purge(&fs, file_cmd::TrashPurge { user, id })
    })
    .await
    .map_err(app_err)?
    {
        Ok(()) => Ok(Json(json!({ "ok": true }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_trash_clear(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
) -> ApiResult {
    admin(&user, act::FILE_TRASH_CLEAR)?;
    csrf(&state, &user, &parts)?;
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || file_cmd::trash_clear(&fs, file_cmd::TrashClear { user }))
        .await
        .map_err(app_err)?
    {
        Ok(out) => Ok(Json(json!({
            "ok": out.failed.is_empty(),
            "result": {
                "removed": out.removed,
                "bytes": out.bytes,
                "failed": out.failed,
            },
        }))),
        Err(e) => Err(fs_err(e)),
    }
}

// ---- 配额 ---------------------------------------------------------------

pub async fn quotas_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::FILE_QUOTA_READ)?;
    let page = FsQuotasPage {
        ctx: handlers::page_ctx(&state, &user, "core/files/quotas"),
    };
    handlers::render(page)
}

pub async fn api_quota_list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> ApiResult {
    admin(&user, act::FILE_QUOTA_READ)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::quota_list(&fs, file_cmd::QuotaList { user: owner })
    })
    .await
    .map_err(app_err)?
    {
        Ok(quotas) => Ok(Json(json!({ "ok": true, "quotas": quotas }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_quota_set(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_QUOTA_SET)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let max_bytes = opt_u64(&body, "max_bytes")?;
    let max_files = opt_u64(&body, "max_files")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::quota_set(
            &fs,
            file_cmd::QuotaSet {
                user: owner,
                path,
                max_bytes,
                max_files,
            },
        )
    })
    .await
    .map_err(app_err)?
    {
        Ok(quota) => Ok(Json(json!({ "ok": true, "quota": quota }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_quota_cancel(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_QUOTA_CANCEL)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    match handlers::blocking_fs(move || {
        file_cmd::quota_cancel(&fs, file_cmd::QuotaCancel { user: owner, path })
    })
    .await
    .map_err(app_err)?
    {
        Ok(quota) => Ok(Json(json!({ "ok": true, "quota": quota }))),
        Err(e) => Err(fs_err(e)),
    }
}

pub async fn api_quota_scan(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> ApiResult {
    admin(&user, act::FILE_QUOTA_SCAN)?;
    csrf(&state, &user, &parts)?;
    let body = body_json(&body)?;
    let path = field(&body, "path")?.to_string();
    let fs = state.admin_fs_arc();
    let user = user.user.username.clone();
    match handlers::blocking_fs(move || {
        file_cmd::quota_scan(&fs, file_cmd::QuotaScan { user, path })
    })
    .await
    .map_err(app_err)?
    {
        Ok(quota) => Ok(Json(json!({ "ok": true, "quota": quota }))),
        Err(e) => Err(fs_err(e)),
    }
}

/// 权限位：数字按十进制数值（前端 `parseInt(x, 8)` 得到），字符串按八进制解析。
fn mode_field(body: &Value, key: &str) -> Result<u32, JsonErr> {
    let value = body
        .get(key)
        .ok_or_else(|| bad_request(format!("缺少字段 {key}")))?;
    let raw = match value {
        Value::Number(n) => {
            let n = n
                .as_u64()
                .ok_or_else(|| bad_request(format!("{key} 必须是非负整数")))?;
            if !(0..=0o17777).contains(&n) {
                return Err(bad_request(format!("{key} 超出 0000-7777 范围")));
            }
            return Ok(n as u32);
        }
        Value::String(s) => s.trim().to_string(),
        _ => return Err(bad_request(format!("{key} 必须是数字或八进制字符串"))),
    };
    let digits = raw
        .strip_prefix("0o")
        .or_else(|| raw.strip_prefix("0O"))
        .unwrap_or(&raw);
    if digits.is_empty() || !digits.chars().all(|c| ('0'..='7').contains(&c)) {
        return Err(bad_request(format!(
            "{key} 必须是八进制权限位（例如 0755 / 2775）"
        )));
    }
    let n = u32::from_str_radix(digits, 8)
        .map_err(|_| bad_request(format!("{key} 超出 0000-7777 范围")))?;
    if n > 0o7777 {
        return Err(bad_request(format!("{key} 超出 0000-7777 范围")));
    }
    Ok(n)
}

fn u32_field(body: &Value, key: &str) -> Result<u32, JsonErr> {
    match body.get(key) {
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| bad_request(format!("{key} 必须是非负整数"))),
        Some(Value::String(s)) => s
            .trim()
            .parse::<u32>()
            .map_err(|_| bad_request(format!("{key} 必须是非负整数"))),
        _ => Err(bad_request(format!("缺少字段 {key}"))),
    }
}

fn string_array(body: &Value, key: &str) -> Result<Vec<String>, JsonErr> {
    let arr = body
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request(format!("缺少数组字段 {key}")))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let s = item
            .as_str()
            .ok_or_else(|| bad_request(format!("{key} 的元素必须是非空字符串")))?;
        if s.is_empty() {
            return Err(bad_request(format!("{key} 的元素必须是非空字符串")));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

/// 上限字段允许：数字、数字字符串、空串/null（表示不限制）。
fn opt_u64(body: &Value, key: &str) -> Result<Option<u64>, JsonErr> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| bad_request(format!("{key} 必须是非负整数"))),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| bad_request(format!("{key} 必须是非负整数"))),
        _ => Err(bad_request(format!("{key} 必须是非负整数"))),
    }
}
