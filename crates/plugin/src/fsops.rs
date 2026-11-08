//! 插件 `fs` 能力面的实现（内核侧）。
//!
//! 两条分支共用同一组权限串：
//! - **相对路径** → 插件自己的 `data/`（原有语义，`fs:read/write/list`）；
//! - **绝对路径** → 宿主文件，额外要求落在安装时确认过的 `path:` 对象授权根内
//!   （判定只在 [`skiff_plugin::fsgrant`]，这里不重复实现）。
//!
//! 为什么必须分块：插件↔内核走 `skiff_wire::MAX_FRAME`（1MiB）单帧，整文件直传
//! 会在帧上限处失败。单次读写上限固定 [`MAX_IO`]，大文件由调用方按 `offset` 分页。

// 调用方是 unix-only 的 `capability::fs_*`；Windows 的普通编译单元里没人调用，
// `-D dead_code` 会当成死代码——那是平台差异，不是真死（单测仍会真跑到这些函数）。
#![cfg_attr(not(unix), allow(dead_code))]

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use skiff_model::{has_parent_segment, is_absolute};
use skiff_store::Grant;

use crate::fsgrant::{self, ResolveError};

/// 单次读/写的字节上限（见模块注释：留在单帧 1MiB 内，base64 膨胀后仍有余量）。
pub const MAX_IO: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Host,
    Data,
}

#[derive(Debug)]
pub enum FsError {
    /// 形式非法 / 越权 / 缺授权（以及宿主权限不足 `EACCES`/`EPERM`）
    Forbidden(String),
    NotFound(String),
    /// 类型冲突（目标是目录、目标已存在等）
    Conflict(String),
    /// 目标文件系统只读（宿主 `ProtectSystem`、只读挂载等策略限制）
    ReadOnly(String),
    /// 宿主磁盘空间不足，或文件系统配额（`ENOSPC` / `EDQUOT`）已用尽
    NoSpace(String),
    /// 未归类的 I/O 故障；只有这一类落 500
    Io(String),
}

impl FsError {
    /// HTTP 状态码。用裸 `u16` 而不是 `axum::StatusCode`——判定层不该依赖
    /// HTTP 框架；映射成 `StatusCode` 由调用方（能力面适配层）负责。
    ///
    /// 与内核侧 `skiff-fs::FsError` 的语义保持一致，对照表见
    /// `docs/KERNEL_FS_CONTRACT.md` §7.3：宿主拒绝类不落 500，只有未归类 I/O 才是 500。
    pub fn status(&self) -> u16 {
        match self {
            FsError::Forbidden(_) | FsError::ReadOnly(_) => 403,
            FsError::NotFound(_) => 404,
            FsError::Conflict(_) => 409,
            FsError::NoSpace(_) => 507,
            FsError::Io(_) => 500,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            FsError::Forbidden(m)
            | FsError::NotFound(m)
            | FsError::Conflict(m)
            | FsError::ReadOnly(m)
            | FsError::NoSpace(m)
            | FsError::Io(m) => m,
        }
    }

    /// 审计结论：越权用 `deny`，其余按故障处理。
    pub fn audit_result(&self) -> &'static str {
        match self {
            FsError::Forbidden(_) => "deny",
            FsError::NotFound(_) | FsError::Conflict(_) => "fail",
            FsError::ReadOnly(_) | FsError::NoSpace(_) | FsError::Io(_) => "fail",
        }
    }
}

/// 与内核侧 `skiff-fs::error::io_error` 同语义：宿主限制类优先按 `raw_os_error()`
/// 归类并给可读中文原因，其余按 `ErrorKind` 回落。
fn io_err(path: &Path, e: std::io::Error) -> FsError {
    io_err_ctx(path, e, "")
}

/// 同 [`io_err`]，但保留调用点的动作描述（如「创建数据目录失败」）。
fn io_err_ctx(path: &Path, e: std::io::Error, context: &str) -> FsError {
    if let Some(err) = classify_os_error(path, e.raw_os_error()) {
        return err;
    }
    let ctx = if context.is_empty() {
        format!("{}: {e}", path.display())
    } else {
        format!("{context}（{}）: {e}", path.display())
    };
    match e.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound(ctx),
        std::io::ErrorKind::PermissionDenied => FsError::Forbidden(format!("权限不足: {ctx}")),
        std::io::ErrorKind::AlreadyExists => FsError::Conflict(ctx),
        _ => FsError::Io(ctx),
    }
}

/// 宿主只读 / 容量类 OS 错误译成可读的 4xx/507；语义与内核侧一致。
#[cfg(unix)]
fn classify_os_error(path: &Path, code: Option<i32>) -> Option<FsError> {
    let code = code?;
    let display = path.display();
    match code {
        libc::EROFS => Some(FsError::ReadOnly(format!(
            "该路径所在文件系统为只读，无法写入（宿主策略限制）: {display}"
        ))),
        libc::ENOSPC => Some(FsError::NoSpace(format!(
            "宿主磁盘空间不足，无法写入: {display}"
        ))),
        libc::EDQUOT => Some(FsError::NoSpace(format!(
            "文件系统配额已用尽，无法写入: {display}"
        ))),
        _ => None,
    }
}

#[cfg(not(unix))]
fn classify_os_error(_path: &Path, _code: Option<i32>) -> Option<FsError> {
    None
}

/// 把插件给的路径解析成可操作的真实路径。
///
/// `create = true` 表示目标可以尚不存在（写文件 / 建目录 / 改名目标）。
pub fn resolve(
    data_dir: &Path,
    id: &str,
    grants: &[Grant],
    raw: &str,
    create: bool,
) -> Result<(PathBuf, Scope), FsError> {
    // 绝对路径走宿主分支（要 `path:` 授权），相对路径走插件自己的 `data/`。
    // 口径与内核其它地方一致（`skiff_model::is_absolute`）：生产环境只跑 Linux
    // （`/` 即绝对）；同时接受平台 `Path::is_absolute()`，是为了让这条判定在
    // Windows 开发机上也能被单测覆盖（`C:\...` 同样算绝对）。
    if is_absolute(raw) {
        let roots = fsgrant::canonical_roots(grants);
        if roots.is_empty() {
            return Err(FsError::Forbidden(
                "没有 path: 宿主路径授权（需在安装确认页勾选，或由 admin 追加）".into(),
            ));
        }
        let path = fsgrant::resolve(&roots, raw, create).map_err(|e| match e {
            ResolveError::Illegal(m) => FsError::Forbidden(m.to_string()),
            ResolveError::Outside => FsError::Forbidden(e.to_string()),
            ResolveError::Missing(m) => FsError::NotFound(m),
        })?;
        Ok((path, Scope::Host))
    } else {
        Ok((resolve_data(data_dir, id, raw, create)?, Scope::Data))
    }
}

fn resolve_data(data_dir: &Path, id: &str, raw: &str, create: bool) -> Result<PathBuf, FsError> {
    if has_parent_segment(raw) || raw.contains('\0') || is_absolute(raw) {
        return Err(FsError::Forbidden("路径非法".into()));
    }
    let root = data_dir.join("plugins").join(id).join("data");
    fs::create_dir_all(&root).map_err(|e| io_err_ctx(&root, e, "创建数据目录失败"))?;
    let canon_root = fs::canonicalize(&root).map_err(|e| io_err_ctx(&root, e, "数据目录不可用"))?;
    let target = root.join(raw);
    if create {
        let parent = target
            .parent()
            .ok_or_else(|| FsError::Forbidden("路径无父目录".into()))?;
        fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
        let canon_parent = fs::canonicalize(parent).map_err(|e| io_err(parent, e))?;
        if !canon_parent.starts_with(&canon_root) {
            return Err(FsError::Forbidden("越权路径".into()));
        }
        let mut resolved = canon_parent.join(
            target
                .file_name()
                .ok_or_else(|| FsError::Forbidden("路径缺少文件名".into()))?,
        );
        // 目标已存在（软链）时按真实路径再判一次，避免借既有软链写到 data/ 之外。
        if resolved.exists() {
            resolved = fs::canonicalize(&resolved).map_err(|e| io_err(&resolved, e))?;
            if !resolved.starts_with(&canon_root) {
                return Err(FsError::Forbidden("越权路径".into()));
            }
        }
        Ok(resolved)
    } else {
        let canon = fs::canonicalize(&target).map_err(|e| io_err(&target, e))?;
        if !canon.starts_with(&canon_root) {
            return Err(FsError::Forbidden("越权路径".into()));
        }
        Ok(canon)
    }
}

/// 内核代写 `data/` 文件后把属主还给插件 UID（宿主路径不 chown，保持宿主原有属主）。
#[cfg(unix)]
pub fn own_by_plugin(path: &Path, id: &str) {
    use std::os::unix::fs::PermissionsExt;
    let uid = crate::install::plugin_uid(id);
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    if let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
        // SAFETY: 合法 C 字符串；非 root 运行时失败，仅影响插件自身可读性。
        unsafe { libc::chown(c.as_ptr(), uid, uid) };
    }
}

#[cfg(not(unix))]
pub fn own_by_plugin(_path: &Path, _id: &str) {}

#[derive(Debug)]
pub struct Chunk {
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub size: u64,
    pub eof: bool,
}

/// 读一页。`offset` 越界（等于或超过文件尾）时返回空页 + `eof = true`。
pub fn read_chunk(path: &Path, offset: u64, max: usize) -> Result<Chunk, FsError> {
    let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
    if meta.is_dir() {
        return Err(FsError::Conflict("目标是目录".into()));
    }
    let size = meta.len();
    let max = max.clamp(1, MAX_IO);
    if size > 0 && offset >= size {
        return Ok(Chunk {
            bytes: Vec::new(),
            offset,
            size,
            eof: true,
        });
    }
    let mut f = File::open(path).map_err(|e| io_err(path, e))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| io_err(path, e))?;
    // procfs（如 `/proc/stat`）的 `metadata.len()` 恒为 0，但内容非空：
    // 不能按 size 提前返回空页，否则 monitor 等插件永远读不到 /proc。
    let want = if size == 0 {
        max
    } else {
        (size - offset).min(max as u64) as usize
    };
    let mut bytes = Vec::with_capacity(want);
    (&mut f)
        .take(want as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, e))?;
    let eof = if size == 0 {
        bytes.len() < max
    } else {
        offset + bytes.len() as u64 >= size
    };
    Ok(Chunk {
        bytes,
        offset,
        size,
        eof,
    })
}

/// 写一页，返回 `(本次写入字节数, 写后文件大小)`。
///
/// 语义：`append = true` 追加；否则 `offset = Some(n)` 定位写、`None` 从 0 覆盖写
/// （先截断）。分块上传的第一页用覆盖写，其余用追加，避免留下的旧尾巴。
pub fn write_chunk(
    path: &Path,
    data: &[u8],
    offset: Option<u64>,
    append: bool,
) -> Result<(usize, u64), FsError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    }
    let mut f = opts.open(path).map_err(|e| io_err(path, e))?;
    if !append {
        match offset {
            Some(o) => {
                f.seek(SeekFrom::Start(o)).map_err(|e| io_err(path, e))?;
            }
            None => f.set_len(0).map_err(|e| io_err(path, e))?,
        }
    }
    f.write_all(data).map_err(|e| io_err(path, e))?;
    f.flush().map_err(|e| io_err(path, e))?;
    let size = f.metadata().map_err(|e| io_err(path, e))?.len();
    Ok((data.len(), size))
}

fn mode_of(meta: Option<&fs::Metadata>) -> Value {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.map(|m| Value::from(format!("{:o}", m.permissions().mode() & 0o7777)))
            .unwrap_or(Value::Null)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Value::Null
    }
}

/// 列目录：`entries` 保持旧形态（名字数组），`items` 带类型/大小/时间/权限。
pub fn list(path: &Path) -> Result<Value, FsError> {
    let meta = fs::metadata(path).map_err(|e| io_err(path, e))?;
    if !meta.is_dir() {
        return Err(FsError::Conflict("目标不是目录".into()));
    }
    let mut items: Vec<Value> = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| io_err(path, e))?.flatten() {
        let md = entry.metadata().ok();
        let name = entry.file_name().to_string_lossy().to_string();
        items.push(json!({
            "name": name,
            "dir": md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            "size": md.as_ref().map(|m| m.len()).unwrap_or(0),
            "mtime": md
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            "mode": mode_of(md.as_ref()),
        }));
    }
    items.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    let entries: Vec<String> = items
        .iter()
        .filter_map(|i| i.get("name").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    Ok(json!({ "entries": entries, "items": items, "dir": true }))
}

pub fn make_dir(path: &Path) -> Result<(), FsError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(FsError::Conflict("目标已存在".into()))
        }
        Err(e) => Err(io_err(path, e)),
    }
}

/// 删除文件或目录（目录递归删），返回 `true` 表示删的是目录。
pub fn remove(path: &Path) -> Result<bool, FsError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_err(path, e))?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io_err(path, e))?;
        Ok(true)
    } else {
        fs::remove_file(path).map_err(|e| io_err(path, e))?;
        Ok(false)
    }
}

/// 改名/移动。目标已存在一律拒：覆盖式改名在「管理站点目录」场景里是误操作放大器。
pub fn rename(from: &Path, to: &Path) -> Result<(), FsError> {
    if to.exists() {
        return Err(FsError::Conflict("目标已存在".into()));
    }
    fs::rename(from, to).map_err(|e| io_err_ctx(from, e, "改名失败"))
}

/// 删除/改名的源路径是不是「根本身」。
///
/// 授权一个目录不等于授权毁掉这个目录：`path:/etc/nginx/conf.d` 可以被逐文件
/// 改写，但不允许把 `conf.d` 整个删掉或改名——那是「授权」与「授权拆家」的区别。
///
/// 宿主分支复用 `fsgrant::is_root_itself`（与授权根判定同源）；data 分支
/// 对应插件自己的数据根。
pub fn is_scope_root(
    data_dir: &Path,
    id: &str,
    grants: &[Grant],
    path: &Path,
    scope: Scope,
) -> bool {
    match scope {
        Scope::Host => fsgrant::is_root_itself(&fsgrant::canonical_roots(grants), path),
        Scope::Data => {
            let root = data_dir.join("plugins").join(id).join("data");
            fs::canonicalize(&root)
                .map(|r| path == r.as_path())
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(value: &str) -> Grant {
        Grant::object(value.to_string())
    }

    #[test]
    fn relative_paths_resolve_into_plugin_data() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let id = "com.example.demo";
        let (path, scope) = resolve(data_dir, id, &[], "notes/x.txt", true).unwrap();
        assert_eq!(scope, Scope::Data);
        // resolve 返回的是 canonical 路径，比较基准也要 canonicalize（Windows 上
        // canonicalize 可能带 `\\?\` 前缀，直接和未规范化字符串比会假失败）。
        let root = fs::canonicalize(data_dir.join("plugins").join(id).join("data")).unwrap();
        assert!(path.starts_with(&root));

        // 越权与非法形式
        assert!(matches!(
            resolve(data_dir, id, &[], "../escape", true),
            Err(FsError::Forbidden(_))
        ));
        assert!(matches!(
            resolve(data_dir, id, &[], "/etc/passwd", false),
            Err(FsError::Forbidden(_))
        ));
    }

    #[test]
    fn host_paths_require_a_path_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("conf.d");
        fs::create_dir_all(&root).unwrap();
        let inside = root.join("site.conf");

        // 没有 path: 授权 → 一律拒（哪怕给了 fs:write）；错误文案必须指出补救入口，
        // 否则用户只看到「未授权」，不知道要去安装确认页勾选。
        match resolve(
            tmp.path(),
            "com.example.demo",
            &[],
            &inside.display().to_string(),
            true,
        ) {
            Err(FsError::Forbidden(m)) => {
                assert!(m.contains("安装确认页勾选"), "错误文案不可读: {m}")
            }
            other => panic!("应当拒绝，实际: {other:?}"),
        }

        let grants = vec![grant(&format!("path:{}", root.display()))];
        let (path, scope) = resolve(
            tmp.path(),
            "com.example.demo",
            &grants,
            &inside.display().to_string(),
            true,
        )
        .unwrap();
        assert_eq!(scope, Scope::Host);
        assert!(path.starts_with(fs::canonicalize(&root).unwrap()));

        // 根外路径仍拒（授权不是「任意绝对路径通行证」）
        let outside = tmp.path().join("other.conf");
        assert!(matches!(
            resolve(
                tmp.path(),
                "com.example.demo",
                &grants,
                &outside.display().to_string(),
                true
            ),
            Err(FsError::Forbidden(_))
        ));
    }

    #[test]
    fn chunked_read_write_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("big.bin");
        let data: Vec<u8> = (0..MAX_IO * 2 + 17).map(|i| (i % 251) as u8).collect();

        // 第一页覆盖写
        write_chunk(&file, &data[..MAX_IO], None, false).unwrap();
        // 后续页追加
        write_chunk(&file, &data[MAX_IO..], None, true).unwrap();

        let mut out = Vec::new();
        let mut offset = 0u64;
        loop {
            let c = read_chunk(&file, offset, MAX_IO).unwrap();
            assert_eq!(c.offset, offset);
            assert_eq!(c.size, data.len() as u64);
            offset += c.bytes.len() as u64;
            out.extend_from_slice(&c.bytes);
            if c.eof {
                break;
            }
        }
        assert_eq!(out, data);

        // 定位写不回卷尾部
        write_chunk(&file, b"XY", Some(0), false).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().len(), data.len() as u64);
        let head = read_chunk(&file, 0, 2).unwrap();
        assert_eq!(head.bytes, b"XY");
    }

    /// procfs 文件的 `metadata.len()` 为 0，但内容非空；读一页必须能拿到正文。
    #[cfg(target_os = "linux")]
    #[test]
    fn reads_procfs_file_with_zero_reported_size() {
        let path = Path::new("/proc/stat");
        assert_eq!(fs::metadata(path).unwrap().len(), 0);
        let c = read_chunk(path, 0, 64 * 1024).unwrap();
        assert!(!c.bytes.is_empty(), "/proc/stat 应读到内容");
        assert!(c.eof, "单页应能读完");
    }

    #[test]
    fn list_reports_dirs_and_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("a.conf"), b"abc").unwrap();
        let v = list(tmp.path()).unwrap();
        let items = v["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let sub = items.iter().find(|i| i["name"] == "sub").unwrap();
        assert_eq!(sub["dir"], true);
        let file = items.iter().find(|i| i["name"] == "a.conf").unwrap();
        assert_eq!(file["dir"], false);
        assert_eq!(file["size"], 3);
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);

        assert!(matches!(
            list(&tmp.path().join("a.conf")),
            Err(FsError::Conflict(_))
        ));
    }

    #[test]
    fn mkdir_remove_rename_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("site");
        make_dir(&dir).unwrap();
        assert!(matches!(make_dir(&dir), Err(FsError::Conflict(_))));

        let from = dir.join("a.conf");
        fs::write(&from, b"x").unwrap();
        let to = dir.join("b.conf");
        rename(&from, &to).unwrap();
        assert!(to.exists());
        assert!(!from.exists());
        // 覆盖式改名必须拒
        fs::write(&from, b"y").unwrap();
        assert!(matches!(rename(&from, &to), Err(FsError::Conflict(_))));

        assert!(!remove(&to).unwrap(), "删文件返回 false");
        assert!(remove(&dir).unwrap(), "删目录返回 true");
        assert!(matches!(remove(&dir), Err(FsError::NotFound(_))));
    }

    /// 宿主拒绝类错误的映射必须与内核侧一致（对照表见 `KERNEL_FS_CONTRACT.md` §7.3）。
    #[cfg(unix)]
    #[test]
    fn host_reject_errors_keep_kernel_semantics() {
        let p = Path::new("/etc/skiff-iter-probe.txt");
        let erofs = io_err(p, std::io::Error::from_raw_os_error(libc::EROFS));
        assert_eq!(erofs.status(), 403);
        assert!(erofs.message().contains("只读"), "{}", erofs.message());

        let eacces = io_err(p, std::io::Error::from_raw_os_error(libc::EACCES));
        assert_eq!(eacces.status(), 403);
        assert!(
            eacces.message().contains("权限不足"),
            "{}",
            eacces.message()
        );

        let enospc = io_err(p, std::io::Error::from_raw_os_error(libc::ENOSPC));
        assert_eq!(enospc.status(), 507);

        let edquot = io_err(p, std::io::Error::from_raw_os_error(libc::EDQUOT));
        assert_eq!(edquot.status(), 507);
        assert!(edquot.message().contains("配额"), "{}", edquot.message());
    }

    #[test]
    fn permission_denied_maps_to_403() {
        let err = io_err(
            Path::new("/etc/hosts"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(err.status(), 403);
        assert!(err.message().contains("权限不足"), "{}", err.message());
    }

    /// 真正的内部错误不能被降级成 4xx。
    #[test]
    fn unknown_io_error_stays_500() {
        let err = io_err(
            Path::new("/www/probe.bin"),
            std::io::Error::other("device exploded"),
        );
        assert_eq!(err.status(), 500);
        assert_eq!(err.audit_result(), "fail");
    }
}
