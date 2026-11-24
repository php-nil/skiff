use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream;
use skiff_app::admin_fs::FsPathKind;
use skiff_app::commands::file as file_cmd;

#[allow(unused_imports)]
use super::*;

/// 上传/下载单片字节数（与 `skiff-fs` 单次读写上限一致）；前端上传适配器取同值。
const CHUNK_BYTES: usize = 256 * 1024;

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
    let can_admin = user.can(skiff_model::Permission::Admin);
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
        upload_chunk_bytes: CHUNK_BYTES,
        can_admin,
    };
    let with_schema = q.get("with_schema").map(String::as_str) == Some("true");
    let mut body = json!({ "data": data });
    if with_schema {
        body["schema"] = super::files_page::files_schema(&body["data"], can_admin);
    }
    Ok(Json(body))
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
    // 站点文档根下新建的目录要归还站点身份，否则容器内 PHP 进程写不进去。
    state.restore_site_index_owner(&created);
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
    let owner_path = to.clone();
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
    // 改名可能一次移动整棵子树，用递归口径归还站点身份。
    state.restore_site_index_owner_tree(&owner_path);
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
    let with_schema = q.get("with_schema").map(String::as_str) == Some("true");
    let mut body = json!({
        "data": {
            "path": file.path,
            "text": file.text,
            "size": file.size,
            "truncated": file.truncated,
            "max_edit_bytes": skiff_app::admin_fs::MAX_EDIT_BYTES,
        }
    });
    if with_schema {
        body["schema"] = super::files_page::file_view_schema(&body["data"]);
    }
    Ok(Json(body))
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

/// `POST /ui/v1/files/upload`：分片上传（`offset` 必须与磁盘当前大小一致；首片按原子替换创建）。
///
/// 前端按 `FilesData::upload_chunk_bytes` 切片，单片超限时这里直接拒绝；超限用 400
/// 而非 413——前端只看 `body.error.message`，`AppError` 没有独立的 413 变体。
pub async fn files_upload(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
    parts: Parts,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_UPLOAD)?;
    guard::csrf(&state, &user, &parts)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError(AppError::BadRequest("缺少 path 参数".into())))?;
    let offset: u64 = q
        .get("offset")
        .ok_or_else(|| ApiError(AppError::BadRequest("缺少 offset 参数".into())))?
        .parse()
        .map_err(|_| ApiError(AppError::BadRequest("offset 必须是非负整数".into())))?;
    if body.len() > CHUNK_BYTES {
        return Err(ApiError(AppError::BadRequest(format!(
            "单次上传分片不能超过 {} KiB",
            CHUNK_BYTES / 1024
        ))));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let path = path.to_string();
    let owner_path = path.clone();
    let out = handlers::blocking_fs(move || {
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    state.restore_site_index_owner(&owner_path);
    Ok(Json(json!({ "data": out })))
}

/// `GET /ui/v1/files/download?path=`：单文件流式下载；目录先打包成临时 zip 再流式输出。
pub async fn files_download(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    guard::action(&user, act::FILE_BROWSE)?;
    let path = q
        .get("path")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError(AppError::BadRequest("缺少 path 参数".into())))?;
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    if stat.kind == FsPathKind::Dir {
        return download_dir(state, user.user.username.clone(), path).await;
    }
    if stat.kind != FsPathKind::File {
        return Err(ApiError(AppError::BadRequest("只支持下载普通文件".into())));
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
async fn download_dir(
    state: Arc<AppState>,
    user: String,
    path: &str,
) -> Result<Response, ApiError> {
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download");
    let name = format!("{base}.zip");
    let dir = tempfile::tempdir()
        .map_err(|e| ApiError(AppError::Internal(format!("创建下载临时目录失败: {e}"))))?;
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
        .map_err(|e| ApiError(AppError::Internal(format!("打包任务失败: {e}"))))?
        .map_err(fs_api_error)?
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
) -> Result<Response, ApiError> {
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
        .map_err(|_| ApiError(AppError::BadRequest("文件名包含无法编码的字符".into())))?;
    let mut resp = (
        axum::http::StatusCode::OK,
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

/// `GET /ui/v1/files/trash/list`：回收站列表（`with_schema` 附带页面骨架）。
pub async fn files_trash_list(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_TRASH_READ)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let items = handlers::blocking_fs(move || {
        file_cmd::trash_list(&fs, file_cmd::TrashList { user: owner })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    let with_schema = q.get("with_schema").map(String::as_str) == Some("true");
    let mut body = json!({
        "data": {
            "items": items,
            "retention_days": state.cfg.trash_retention_days,
        }
    });
    if with_schema {
        body["schema"] = super::files_page::trash_schema(&body["data"]);
    }
    Ok(Json(body))
}

/// `POST /ui/v1/files/trash/restore`：按回收站编号还原，可选覆盖同名目标。
pub async fn files_trash_restore(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<TrashRestoreBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_TRASH_RESTORE)?;
    guard::csrf(&state, &user, &parts)?;
    let id = body.id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError(AppError::BadRequest("缺少 id 参数".into())));
    }
    let overwrite = body.overwrite;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let out = handlers::blocking_fs(move || {
        file_cmd::trash_restore(
            &fs,
            file_cmd::TrashRestore {
                user: owner,
                id,
                overwrite,
            },
        )
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": out })))
}

/// `POST /ui/v1/files/trash/purge`：按回收站编号彻底删除（不可恢复）。
pub async fn files_trash_purge(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<TrashIdBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_TRASH_PURGE)?;
    guard::csrf(&state, &user, &parts)?;
    let id = body.id.trim().to_string();
    if id.is_empty() {
        return Err(ApiError(AppError::BadRequest("缺少 id 参数".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    handlers::blocking_fs(move || {
        file_cmd::trash_purge(&fs, file_cmd::TrashPurge { user: owner, id })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": { "message": "已彻底删除" } })))
}

/// `POST /ui/v1/files/trash/clear`：清空回收站。
pub async fn files_trash_clear(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_TRASH_CLEAR)?;
    guard::csrf(&state, &user, &parts)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let out = handlers::blocking_fs(move || {
        file_cmd::trash_clear(&fs, file_cmd::TrashClear { user: owner })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({
        "data": {
            "removed": out.removed,
            "bytes": out.bytes,
            "failed": out.failed,
        }
    })))
}

/// `GET /ui/v1/files/quotas/list`：配额列表（`with_schema` 附带页面骨架）。
pub async fn files_quota_list(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_QUOTA_READ)?;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let quotas = handlers::blocking_fs(move || {
        file_cmd::quota_list(&fs, file_cmd::QuotaList { user: owner })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    let with_schema = q.get("with_schema").map(String::as_str) == Some("true");
    let mut body = json!({ "data": { "quotas": quotas } });
    if with_schema {
        body["schema"] = super::files_page::quotas_schema(&body["data"]);
    }
    Ok(Json(body))
}

/// `POST /ui/v1/files/quotas/set`：设置目录配额（字节数 / 文件数均可单独为 null）。
pub async fn files_quota_set(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<QuotaSetBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_QUOTA_SET)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("缺少 path 参数".into())));
    }
    let max_bytes = body.max_bytes;
    let max_files = body.max_files;
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let quota = handlers::blocking_fs(move || {
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
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": quota })))
}

/// `POST /ui/v1/files/quotas/cancel`：取消目录配额。
pub async fn files_quota_cancel(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FilePathBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_QUOTA_CANCEL)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("缺少 path 参数".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let quota = handlers::blocking_fs(move || {
        file_cmd::quota_cancel(&fs, file_cmd::QuotaCancel { user: owner, path })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": quota })))
}

/// `POST /ui/v1/files/quotas/scan`：递归重扫目录树，修正配额已用量。
pub async fn files_quota_scan(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<FilePathBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::action(&user, act::FILE_QUOTA_SCAN)?;
    guard::csrf(&state, &user, &parts)?;
    let path = body.path.trim().to_string();
    if path.is_empty() {
        return Err(ApiError(AppError::BadRequest("缺少 path 参数".into())));
    }
    let fs = state.admin_fs_arc();
    let owner = user.user.username.clone();
    let quota = handlers::blocking_fs(move || {
        file_cmd::quota_scan(&fs, file_cmd::QuotaScan { user: owner, path })
    })
    .await
    .map_err(ApiError)?
    .map_err(fs_api_error)?;
    Ok(Json(json!({ "data": quota })))
}

pub(super) fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
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
        .route("/ui/v1/files/upload", post(files_upload))
        .route("/ui/v1/files/download", get(files_download))
        .route("/ui/v1/files/trash/list", get(files_trash_list))
        .route("/ui/v1/files/trash/restore", post(files_trash_restore))
        .route("/ui/v1/files/trash/purge", post(files_trash_purge))
        .route("/ui/v1/files/trash/clear", post(files_trash_clear))
        .route("/ui/v1/files/quotas/list", get(files_quota_list))
        .route("/ui/v1/files/quotas/set", post(files_quota_set))
        .route("/ui/v1/files/quotas/cancel", post(files_quota_cancel))
        .route("/ui/v1/files/quotas/scan", post(files_quota_scan))
        // 形态门禁：`SKIFF_FORM=docker` 能力面收缩，`/ui/v1/files*` JSON 接口一律 403
        // （原挂在遗留 `/{p}/files*` 路由组，收口到 `/ui/v1` 后随子路由迁移；
        // 页面 `/p/files*` 走壳页回落，不在此层判定，权限统一在数据端点）。
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            crate::middleware::fs_capability,
        ))
}
