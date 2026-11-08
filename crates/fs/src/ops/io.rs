//! 低层文件原语：不跟随符号链接的打开、原子写、目录 fsync、对象删除与元数据。
//!
//! Unix 上用 `O_NOFOLLOW` 收口最后一段；Windows 没有等价 flag，退回
//! `symlink_metadata` 二次校验（残留竞态窗口见 `docs/KERNEL_FS_IMPL_NOTES.md`）。

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{io_error, FsError};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// 只读打开，不跟随最后一段符号链接。
pub(crate) fn open_read(path: &Path) -> Result<File, FsError> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    opts.custom_flags(libc::O_NOFOLLOW);
    #[cfg(not(unix))]
    ensure_not_symlink(path)?;
    opts.open(path).map_err(|e| io_error(path, e))
}

/// 以覆盖/追加方式打开；不跟随符号链接。
pub(crate) fn open_write(path: &Path, append: bool) -> Result<File, FsError> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if append {
        opts.append(true);
    }
    #[cfg(unix)]
    opts.custom_flags(libc::O_NOFOLLOW);
    #[cfg(not(unix))]
    ensure_not_symlink(path)?;
    opts.open(path).map_err(|e| io_error(path, e))
}

#[cfg(not(unix))]
pub(crate) fn ensure_not_symlink(path: &Path) -> Result<(), FsError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(FsError::Escape(format!(
            "不跟随符号链接: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(path, e)),
    }
}

/// 原子替换：同目录临时文件 → 写入 → fsync(file) → rename → fsync(dir)。
///
/// 失败清理临时文件；`existing` 用于保留原权限（Unix 另尽力保留属主）。
pub(crate) fn atomic_write(
    path: &Path,
    data: &[u8],
    existing: Option<&Metadata>,
) -> Result<(), FsError> {
    let dir = path
        .parent()
        .ok_or_else(|| FsError::Denied("路径无父目录".into()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| FsError::Denied("路径缺少文件名或文件名非 UTF-8".into()))?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}-{}",
        name,
        std::process::id(),
        tmp_seq()
    ));

    let result = (|| -> Result<(), FsError> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_error(&tmp, e))?;
        f.write_all(data).map_err(|e| io_error(&tmp, e))?;
        f.sync_all().map_err(|e| io_error(&tmp, e))?;
        if let Some(meta) = existing {
            preserve_attrs(&f, meta);
        }
        drop(f);
        fs::rename(&tmp, path).map_err(|e| io_error(path, e))?;
        sync_dir(dir)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// 显式非原子追加（契约 §3.3）。
pub(crate) fn append(path: &Path, data: &[u8]) -> Result<(), FsError> {
    let mut f = open_write(path, true)?;
    f.write_all(data).map_err(|e| io_error(path, e))?;
    f.flush().map_err(|e| io_error(path, e))
}

/// 显式非原子定位写（契约未单列；见实现说明）。
pub(crate) fn write_at(path: &Path, offset: u64, data: &[u8]) -> Result<(), FsError> {
    let mut f = open_write(path, false)?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| io_error(path, e))?;
    f.write_all(data).map_err(|e| io_error(path, e))?;
    f.flush().map_err(|e| io_error(path, e))
}

/// 读区间；`size` 是文件大小（procfs 等上报 0 时按实际读到为准）。
pub(crate) fn read_range(
    path: &Path,
    offset: u64,
    max: usize,
) -> Result<(Vec<u8>, u64, bool), FsError> {
    let mut f = open_read(path)?;
    let meta = f.metadata().map_err(|e| io_error(path, e))?;
    let size = meta.len();
    if size > 0 && offset >= size {
        return Ok((Vec::new(), size, true));
    }
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| io_error(path, e))?;
    let want = if size == 0 {
        max
    } else {
        (size - offset).min(max as u64) as usize
    };
    let mut buf = Vec::with_capacity(want);
    (&mut f)
        .take(want as u64)
        .read_to_end(&mut buf)
        .map_err(|e| io_error(path, e))?;
    let eof = if size == 0 {
        buf.len() < max
    } else {
        offset + buf.len() as u64 >= size
    };
    Ok((buf, size, eof))
}

/// 删除对象：目录递归删，文件/符号链接只删自身（硬链接只减引用计数）。
pub(crate) fn remove_object(path: &Path) -> Result<bool, FsError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    if meta.is_dir() {
        fs::remove_dir_all(path).map_err(|e| io_error(path, e))?;
        Ok(true)
    } else {
        fs::remove_file(path).map_err(|e| io_error(path, e))?;
        Ok(false)
    }
}

/// 对象逻辑用量：字节 = 普通文件大小之和，文件数 = 普通文件个数；符号链接
/// 既不计字节也不计文件数，目录只递归不计数。有上限，防无限扫描。
pub(crate) fn object_stats(
    path: &Path,
    cap_entries: usize,
) -> Result<crate::config::QuotaUsage, FsError> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    let mut usage = crate::config::QuotaUsage::default();
    if meta.file_type().is_symlink() {
        return Ok(usage);
    }
    if meta.is_file() {
        return Ok(crate::config::QuotaUsage {
            used_bytes: meta.len(),
            used_files: 1,
        });
    }
    if !meta.is_dir() {
        return Ok(usage);
    }
    let mut seen = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| io_error(&dir, e))?;
        for entry in entries {
            seen += 1;
            if seen > cap_entries {
                return Ok(usage);
            }
            let entry = entry.map_err(|e| io_error(&dir, e))?;
            // 不跟随符号链接：链接本身不计入用量。
            let meta = fs::symlink_metadata(entry.path()).map_err(|e| io_error(&dir, e))?;
            let ft = meta.file_type();
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                usage.used_bytes = usage.used_bytes.saturating_add(meta.len());
                usage.used_files = usage.used_files.saturating_add(1);
            }
        }
    }
    Ok(usage)
}

/// 全量扫描目录树用量（配额「重新统计」使用；成本 O(条目数)）。
pub(crate) fn scan_usage(
    root: &Path,
    cap_entries: usize,
) -> Result<crate::config::QuotaUsage, FsError> {
    object_stats(root, cap_entries)
}

/// 文件类型之外的可审计属性。
#[cfg(unix)]
pub(crate) fn mode_of(meta: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
pub(crate) fn mode_of(_meta: &Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
pub(crate) fn owner_of(meta: &Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", meta.uid(), meta.gid()))
}

#[cfg(not(unix))]
pub(crate) fn owner_of(_meta: &Metadata) -> Option<String> {
    None
}

#[cfg(unix)]
pub(crate) fn nlink_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink()
}

#[cfg(not(unix))]
pub(crate) fn nlink_of(_meta: &Metadata) -> u64 {
    1
}

pub(crate) fn mtime_of(meta: &Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// 创建符号链接（复制/回收站跨盘恢复用）。
pub(crate) fn make_symlink(target: &Path, link: &Path) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| io_error(link, e))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(FsError::Unsupported("当前平台不支持创建符号链接".into()))
    }
}

fn preserve_attrs(_f: &File, existing: &Metadata) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let _ = _f.set_permissions(fs::Permissions::from_mode(existing.mode() & 0o7777));
        // 保留属主：非 root 时 fchown 会 EPERM，忽略失败（权限位仍是我们的策略基线）。
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(_f);
        unsafe {
            libc::fchown(fd, existing.uid(), existing.gid());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = _f;
        let _ = existing;
    }
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), FsError> {
    let f = File::open(dir).map_err(|e| io_error(dir, e))?;
    f.sync_all().map_err(|e| io_error(dir, e))
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<(), FsError> {
    Ok(())
}

fn tmp_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}
