use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use skiff_app::commands::file as file_cmd;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/files?path=`：平铺目录列表（不提供树形）。
///
/// 授权、路径硬拒、配额与审计全部沿用 `AdminFs`；这里只做 DTO 形状转换。
pub async fn files(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_BROWSE)?;
    let path = q
        .get("path")
        .map(|p| p.trim().to_string())
        .filter(|p| std::path::Path::new(p).is_absolute())
        .unwrap_or_else(|| state.cfg.web_root.display().to_string());
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let (roots, listing) = handlers::blocking_fs(move || {
        let roots = fs.roots();
        let listing = file_cmd::list(&fs, file_cmd::ListDir { user: owner, path })?;
        Ok((roots, listing))
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;

    let entries = listing
        .entries
        .into_iter()
        .map(|e| {
            let child = std::path::Path::new(&listing.path).join(&e.name);
            FileEntryItem {
                name: e.name,
                path: child.display().to_string(),
                dir: e.dir,
                symlink: e.symlink,
                size: e.size,
                mtime: e.mtime.and_then(skiff_app::time_fmt::epoch_to_rfc3339_utc),
                mode: e.mode,
                owner: e.owner,
                nlink: e.nlink,
                link_target: e.link_target,
            }
        })
        .collect();
    let data = FilesData {
        path: listing.path,
        parent: listing.parent,
        roots: roots
            .into_iter()
            .map(|r| FileRootItem {
                label: r.label,
                path: r.path,
            })
            .collect(),
        entries,
        truncated: listing.truncated,
        max_edit_bytes: skiff_app::admin_fs::MAX_EDIT_BYTES,
        upload_chunk_bytes: handlers::files::CHUNK_BYTES,
        can_admin: user.can(skiff_model::Permission::Admin),
    };
    Ok(Json(json!({ "data": data })))
}

/// `POST /ui/v1/files/mkdir`：新建目录（管理员 + CSRF）。
pub async fn files_mkdir(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FilePathBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_WRITE)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("目录路径不能为空".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let created = handlers::blocking_fs(move || {
        file_cmd::mkdir(&fs, file_cmd::MakeDir { user: owner, path })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(
        json!({ "data": { "path": created, "message": "目录已创建" } }),
    ))
}

/// `POST /ui/v1/files/rename`：重命名（同目录或移动，覆盖规则由 `skiff-fs` 决定）。
pub async fn files_rename(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileRenameBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_WRITE)?;
    guard::csrf(&state, &user, &parts)?;
    let from = body.from.trim().to_string();
    let to = body.to.trim().to_string();
    if from.is_empty() || to.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "重命名需要同时提供原路径与新路径".into(),
        )));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    handlers::blocking_fs(move || {
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": { "message": "已重命名" } })))
}

/// `POST /ui/v1/files/remove`：删除（进回收站，`trash_id` 非空即可恢复）。
pub async fn files_remove(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FilePathBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_REMOVE)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("路径不能为空".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let removed = handlers::blocking_fs(move || {
        file_cmd::remove(&fs, file_cmd::RemoveEntry { user: owner, path })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({
        "data": {
            "path": removed.path,
            "trash_id": removed.trash_id,
            "message": "已移入回收站"
        }
    })))
}

/// `GET /ui/v1/files/read?path=`：只读文本（代码查看器）；二进制判定归前端。
pub async fn files_read(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_READ)?;
    let path = q
        .get("path")
        .map(|p| p.trim().to_string())
        .filter(|p| std::path::Path::new(p).is_absolute())
        .ok_or_else(|| ApiError(AppError::BadRequest("缺少 path 参数或不是绝对路径".into())))?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let file = handlers::blocking_fs(move || {
        file_cmd::read(&fs, file_cmd::ReadFile { user: owner, path })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({
        "data": {
            "path": file.path,
            "text": file.text,
            "size": file.size,
            "truncated": file.truncated,
            "max_edit_bytes": skiff_app::admin_fs::MAX_EDIT_BYTES,
        }
    })))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/files", get(files))
        .route("/ui/v1/files/read", get(files_read))
        .route("/ui/v1/files/mkdir", post(files_mkdir))
        .route("/ui/v1/files/rename", post(files_rename))
        .route("/ui/v1/files/remove", post(files_remove))
}
