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

/// `POST /ui/v1/files/write`：文本写入（新建或覆盖，权限/审计在 `AdminFs`）。
pub async fn files_write(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileWriteBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_WRITE)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("文件路径不能为空".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let content = body.content;
    let saved = handlers::blocking_fs(move || {
        file_cmd::write(
            &fs,
            file_cmd::WriteFile {
                user: owner,
                path,
                content,
            },
        )
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    state.restore_site_index_owner(&saved.path);
    Ok(Json(json!({
        "data": {
            "path": saved.path,
            "size": saved.size,
            "sha256": saved.sha256,
            "message": "已保存"
        }
    })))
}

/// `POST /ui/v1/files/copy`：复制（`overwrite` 为 false 时目标已存在会失败）。
pub async fn files_copy(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileCopyBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_COPY)?;
    guard::csrf(&state, &user, &parts)?;
    let from = body.from.trim().to_string();
    let to = body.to.trim().to_string();
    if from.is_empty() || to.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "复制需要同时提供源路径与目标路径".into(),
        )));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let overwrite = body.overwrite;
    let copied = handlers::blocking_fs(move || {
        file_cmd::copy(
            &fs,
            file_cmd::CopyEntry {
                user: owner,
                from,
                to,
                overwrite,
            },
        )
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    state.restore_site_index_owner_tree(&copied.to);
    Ok(Json(json!({
        "data": {
            "from": copied.from,
            "to": copied.to,
            "bytes": copied.bytes,
            "entries": copied.entries,
            "message": "已复制"
        }
    })))
}

/// 八进制权限解析：接受 `755` / `0755` / `0o755` 或数字。
fn parse_mode(raw: Option<serde_json::Value>) -> Result<u32, ApiError> {
    let text = match raw {
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(|v| v.to_string())
            .ok_or_else(|| ApiError(AppError::BadRequest("mode 必须是非负整数".into())))?,
        Some(serde_json::Value::String(s)) => s.trim().to_string(),
        _ => {
            return Err(ApiError(AppError::BadRequest(
                "mode 必须是八进制字符串或数字".into(),
            )))
        }
    };
    let digits = text
        .strip_prefix("0o")
        .or_else(|| text.strip_prefix("0O"))
        .unwrap_or(&text);
    let mode = u32::from_str_radix(digits, 8)
        .map_err(|_| ApiError(AppError::BadRequest("mode 不是合法的八进制权限".into())))?;
    if mode > 0o17777 {
        return Err(ApiError(AppError::BadRequest(
            "mode 超出 0000-7777 范围".into(),
        )));
    }
    Ok(mode)
}

/// `POST /ui/v1/files/chmod`：修改权限位。
pub async fn files_chmod(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileChmodBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_CHMOD)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("路径不能为空".into())));
    }
    let mode = parse_mode(body.mode)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let out = handlers::blocking_fs(move || {
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({
        "data": {
            "path": out.path,
            "mode": out.mode,
            "previous": out.previous,
            "message": "权限已修改"
        }
    })))
}

/// `POST /ui/v1/files/chown`：修改属主；uid/gid 缺失直接拒绝，不默认成 0。
pub async fn files_chown(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileChownBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_CHOWN)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("路径不能为空".into())));
    }
    let uid = body
        .uid
        .ok_or_else(|| ApiError(AppError::BadRequest("uid 不能为空".into())))?;
    let gid = body
        .gid
        .ok_or_else(|| ApiError(AppError::BadRequest("gid 不能为空".into())))?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let out = handlers::blocking_fs(move || {
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({
        "data": {
            "path": out.path,
            "owner": out.owner,
            "root_owned": out.root_owned,
            "message": if out.root_owned { "属主已改为 root" } else { "属主已修改" }
        }
    })))
}

/// `POST /ui/v1/files/archive`：压缩；压缩包路径与格式校验在 `skiff-fs`。
pub async fn files_archive(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileArchiveBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_ARCHIVE)?;
    guard::csrf(&state, &user, &parts)?;
    let sources = body
        .sources
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "压缩至少需要一个源路径".into(),
        )));
    }
    let dest = body.dest.trim().to_string();
    let format = body.format.trim().to_string();
    if dest.is_empty() || format.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "压缩需要提供目标路径与格式".into(),
        )));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let overwrite = body.overwrite;
    let out = handlers::blocking_fs(move || {
        file_cmd::archive(
            &fs,
            file_cmd::ArchiveEntries {
                user: owner,
                sources,
                dest,
                format,
                overwrite,
            },
        )
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    state.restore_site_index_owner(&out.path);
    Ok(Json(json!({
        "data": {
            "path": out.path,
            "bytes": out.bytes,
            "entries": out.entries,
            "skipped_symlinks": out.skipped_symlinks,
            "skipped_special": out.skipped_special,
            "message": "已创建压缩包"
        }
    })))
}

/// `POST /ui/v1/files/extract`：解压；路径穿越与目标存在性由 `skiff-fs` 硬拒。
pub async fn files_extract(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FileExtractBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_EXTRACT)?;
    guard::csrf(&state, &user, &parts)?;
    let archive = body.archive.trim().to_string();
    let dest = body.dest.trim().to_string();
    if archive.is_empty() || dest.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "解压需要提供压缩包路径与目标目录".into(),
        )));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let overwrite = body.overwrite;
    let out = handlers::blocking_fs(move || {
        file_cmd::extract(
            &fs,
            file_cmd::ExtractArchive {
                user: owner,
                archive,
                dest,
                overwrite,
            },
        )
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    state.restore_site_index_owner_tree(&out.dest);
    Ok(Json(json!({
        "data": {
            "dest": out.dest,
            "files": out.files,
            "bytes": out.bytes,
            "skipped_symlinks": out.skipped_symlinks,
            "skipped_special": out.skipped_special,
            "message": "已解压"
        }
    })))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/files", get(files))
        .route("/ui/v1/files/read", get(files_read))
        .route("/ui/v1/files/write", post(files_write))
        .route("/ui/v1/files/mkdir", post(files_mkdir))
        .route("/ui/v1/files/rename", post(files_rename))
        .route("/ui/v1/files/remove", post(files_remove))
        .route("/ui/v1/files/copy", post(files_copy))
        .route("/ui/v1/files/chmod", post(files_chmod))
        .route("/ui/v1/files/chown", post(files_chown))
        .route("/ui/v1/files/archive", post(files_archive))
        .route("/ui/v1/files/extract", post(files_extract))
}
