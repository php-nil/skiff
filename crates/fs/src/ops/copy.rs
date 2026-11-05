//! 服务端复制与跨盘移动（契约 §3.1 `copy`、§5.1 回收站移动）。
//!
//! 递归复制不跟随符号链接：目录里的链接只复制链接本身；目录复制有条目数与
//! 深度上限（契约要求；>1000 项在 P2 改为异步任务，P1 直接拒绝）。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{io_error, FsError};

use super::io;

#[derive(Debug, Clone, Copy)]
pub struct CopyLimits {
    pub max_entries: usize,
    pub max_depth: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub bytes: u64,
    pub entries: u64,
}

/// 复制一个对象（文件/目录/符号链接），`overwrite` 控制已有目标。
pub(crate) fn copy_object(
    src: &Path,
    dst: &Path,
    limits: CopyLimits,
    overwrite: bool,
) -> Result<CopyStats, FsError> {
    let mut stats = CopyStats::default();
    copy_any(src, dst, 0, limits, overwrite, &mut stats)?;
    Ok(stats)
}

fn copy_any(
    src: &Path,
    dst: &Path,
    depth: usize,
    limits: CopyLimits,
    overwrite: bool,
    stats: &mut CopyStats,
) -> Result<(), FsError> {
    if depth > limits.max_depth {
        return Err(FsError::TooLarge(format!(
            "目录复制深度超过上限 {}",
            limits.max_depth
        )));
    }
    let meta = fs::symlink_metadata(src).map_err(|e| io_error(src, e))?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        copy_symlink(src, dst, overwrite)?;
        bump(stats, limits)?;
        return Ok(());
    }
    if ft.is_dir() {
        // 根目录本身不计入条目数；条目数 = 被复制出的文件/子目录数量。
        if depth > 0 {
            bump(stats, limits)?;
        }
        copy_dir(src, dst, depth, limits, overwrite, stats)
    } else if ft.is_file() {
        copy_file(src, dst, &meta, overwrite)?;
        stats.bytes = stats.bytes.saturating_add(meta.len());
        bump(stats, limits)?;
        Ok(())
    } else {
        Err(FsError::Unsupported(format!(
            "不支持复制特殊文件: {}",
            src.display()
        )))
    }
}

fn bump(stats: &mut CopyStats, limits: CopyLimits) -> Result<(), FsError> {
    stats.entries = stats.entries.saturating_add(1);
    if stats.entries > limits.max_entries as u64 {
        return Err(FsError::TooLarge(format!(
            "目录复制条目超过上限 {}（P2 走异步任务）",
            limits.max_entries
        )));
    }
    Ok(())
}

fn copy_dir(
    src: &Path,
    dst: &Path,
    depth: usize,
    limits: CopyLimits,
    overwrite: bool,
    stats: &mut CopyStats,
) -> Result<(), FsError> {
    match fs::symlink_metadata(dst) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(FsError::Conflict("目标已存在且不是目录".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(dst).map_err(|e| io_error(dst, e))?;
        }
        Err(e) => return Err(io_error(dst, e)),
    }
    for entry in fs::read_dir(src).map_err(|e| io_error(src, e))? {
        let entry = entry.map_err(|e| io_error(src, e))?;
        let child_src = entry.path();
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| FsError::Denied("目录含非 UTF-8 名称".into()))?
            .to_string();
        let child_dst = dst.join(name);
        copy_any(&child_src, &child_dst, depth + 1, limits, overwrite, stats)?;
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path, meta: &fs::Metadata, overwrite: bool) -> Result<(), FsError> {
    match fs::symlink_metadata(dst) {
        Ok(dst_meta) if dst_meta.is_dir() => {
            return Err(FsError::Conflict("目标是目录".into()));
        }
        Ok(_) if !overwrite => {
            return Err(FsError::Conflict("目标已存在".into()));
        }
        Ok(_) => {
            // 覆盖时先移除目标（含符号链接本身），避免沿链接写到根外。
            io::remove_object(dst)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(dst, e)),
    }

    let mut reader = io::open_read(src)?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(|e| io_error(dst, e))?;
    copy_stream(&mut reader, &mut writer, dst)?;
    drop(writer);
    preserve_mode(dst, meta)
}

fn copy_stream(reader: &mut File, writer: &mut File, dst: &Path) -> Result<(), FsError> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| io_error(dst, e))?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).map_err(|e| io_error(dst, e))?;
    }
    writer.flush().map_err(|e| io_error(dst, e))
}

fn copy_symlink(src: &Path, dst: &Path, overwrite: bool) -> Result<(), FsError> {
    match fs::symlink_metadata(dst) {
        Ok(_) if !overwrite => return Err(FsError::Conflict("目标已存在".into())),
        Ok(_) => {
            io::remove_object(dst)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(dst, e)),
    }
    let target = fs::read_link(src).map_err(|e| io_error(src, e))?;
    io::make_symlink(&target, dst)
}

fn preserve_mode(dst: &Path, meta: &fs::Metadata) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dst,
            fs::Permissions::from_mode(meta.permissions().mode() & 0o7777),
        )
        .map_err(|e| io_error(dst, e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (dst, meta);
    }
    Ok(())
}

/// 移动对象：同文件系统走 `rename`；跨盘（EXDEV）退回「复制 + 校验 + 删除」。
///
/// 返回 `(统计, 是否跨盘)`。
pub(crate) fn move_object(
    src: &Path,
    dst: &Path,
    limits: CopyLimits,
) -> Result<(CopyStats, bool), FsError> {
    match fs::rename(src, dst) {
        Ok(()) => Ok((CopyStats::default(), false)),
        Err(e) if is_cross_device(&e) => {
            let stats = copy_object(src, dst, limits, false)?;
            io::remove_object(src)?;
            Ok((stats, true))
        }
        Err(e) => Err(io_error(src, e)),
    }
}

fn is_cross_device(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17) // ERROR_NOT_SAME_DEVICE
    }
}
