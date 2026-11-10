//! 服务端复制与跨盘移动（契约 §3.1 `copy`、§5.1 回收站移动）。
//!
//! 递归复制不跟随符号链接：目录里的链接只复制链接本身；目录复制有条目数与
//! 深度上限（契约要求；>1000 项在 P2 改为异步任务，P1 直接拒绝）。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{io_error, FsError};

use super::io;

/// 临时物名前缀；回收站/复制目录都会在目标同父目录短暂出现，成功后即消失。
const TEMP_PREFIX: &str = ".skiff-tmp-";

#[derive(Debug, Clone, Copy)]
pub struct CopyLimits {
    pub max_entries: usize,
    pub max_depth: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub bytes: u64,
    pub entries: u64,
    /// 普通文件个数（目录/符号链接不计）。
    pub files: u64,
}

/// 复制一个对象（文件/目录/符号链接），`overwrite` 控制已有目标。
///
/// 目录复制到**不存在**的目标时走「同父临时目录 + 原子 rename」：中途失败不会在
/// 目标位置留下半成品目录（`move_object` 的跨盘分支复用同一条路径）。目标已存在
/// 时保持「合并覆盖」语义——整体原子替换会误删目标里的既有内容。
pub(crate) fn copy_object(
    src: &Path,
    dst: &Path,
    limits: CopyLimits,
    overwrite: bool,
) -> Result<CopyStats, FsError> {
    let meta = fs::symlink_metadata(src).map_err(|e| io_error(src, e))?;
    if meta.is_dir() && !path_exists(dst)? {
        let tmp = temp_sibling(dst);
        let mut stats = CopyStats::default();
        if let Err(e) = copy_any(src, &tmp, 0, limits, false, &mut stats) {
            cleanup_temp(&tmp);
            return Err(e);
        }
        if let Err(e) = fs::rename(&tmp, dst) {
            cleanup_temp(&tmp);
            return Err(io_error(dst, e));
        }
        return Ok(stats);
    }
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
        stats.files = stats.files.saturating_add(1);
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
    // 失败时删掉半成品：目标此前不存在（create_new），删除不会误伤既有数据；
    // 否则残留的不完整文件会被后续「目标已存在」挡住，并可能被当成正常副本。
    if let Err(e) = copy_stream(&mut reader, &mut writer, dst) {
        drop(writer);
        let _ = fs::remove_file(dst);
        return Err(e);
    }
    drop(writer);
    if let Err(e) = preserve_mode(dst, meta) {
        let _ = fs::remove_file(dst);
        return Err(e);
    }
    Ok(())
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

/// 移动对象：同文件系统走 `rename`；跨盘（EXDEV）退回「复制到同父临时物 + 原子
/// rename + 删源」。
///
/// 返回 `(统计, 是否跨盘)`。
pub(crate) fn move_object(
    src: &Path,
    dst: &Path,
    limits: CopyLimits,
) -> Result<(CopyStats, bool), FsError> {
    match fs::rename(src, dst) {
        Ok(()) => Ok((CopyStats::default(), false)),
        Err(e) if is_cross_device(&e) => move_cross_device(src, dst, limits),
        Err(e) => Err(io_error(src, e)),
    }
}

/// 跨盘移动：临时物与最终目标同父目录（同一文件系统），复制完成后原子 rename
/// 到位。复制或 rename 失败清理临时物、源保持原样；删源失败时目标已是完整副本，
/// 不删目标回滚（回滚可能误删同名的既存数据）。
fn move_cross_device(
    src: &Path,
    dst: &Path,
    limits: CopyLimits,
) -> Result<(CopyStats, bool), FsError> {
    let tmp = temp_sibling(dst);
    let mut stats = CopyStats::default();
    if let Err(e) = copy_any(src, &tmp, 0, limits, false, &mut stats) {
        cleanup_temp(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, dst) {
        cleanup_temp(&tmp);
        return Err(io_error(dst, e));
    }
    io::remove_object(src)?;
    Ok((stats, true))
}

fn path_exists(p: &Path) -> Result<bool, FsError> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(p, e)),
    }
}

/// 目标同父目录下的临时名：`rename` 只在同一文件系统内原子生效，临时物必须与
/// 最终目标同盘（回收站场景即落在 `<trash>/<yyyy>/<mm>/` 下）。
fn temp_sibling(dst: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let raw = dst.file_name().and_then(|n| n.to_str()).unwrap_or("skiff");
    let short: String = raw.chars().take(40).collect();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    dst.with_file_name(format!(
        "{TEMP_PREFIX}{short}-{}-{nanos}-{seq}",
        std::process::id()
    ))
}

/// 失败清理半成品；临时物可能在首个条目就失败而尚未创建，忽略清理错误
/// （失败原因已经由返回的错误表达）。
fn cleanup_temp(tmp: &Path) {
    let _ = io::remove_object(tmp);
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
    #[cfg(not(any(unix, windows)))]
    {
        let _ = e;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CopyLimits {
        CopyLimits {
            max_entries: 100,
            max_depth: 8,
        }
    }

    fn has_temp(dir: &Path) -> bool {
        fs::read_dir(dir).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(TEMP_PREFIX)
        })
    }

    #[test]
    fn cross_device_move_renames_temp_into_place_and_removes_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"AAA").unwrap();
        fs::write(src.join("sub/b.txt"), b"BBB").unwrap();
        let dst = dir.path().join("moved");

        let (stats, crossed) = move_cross_device(&src, &dst, limits()).unwrap();
        assert!(crossed);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.entries, 3, "子目录 + 两个文件");
        assert!(!src.exists(), "源应已删除");
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"AAA");
        assert_eq!(fs::read(dst.join("sub/b.txt")).unwrap(), b"BBB");
        assert!(!has_temp(dir.path()), "成功后不应残留临时物");
    }

    #[test]
    fn cross_device_move_failure_cleans_temp_and_keeps_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(src.join(name), b"x").unwrap();
        }
        let dst = dir.path().join("moved");

        let err = move_cross_device(
            &src,
            &dst,
            CopyLimits {
                max_entries: 1,
                max_depth: 4,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "fs.too_large");
        assert!(src.exists(), "失败时源必须原样保留");
        assert!(!dst.exists(), "目标不得出现半成品");
        assert!(!has_temp(dir.path()), "失败后必须清理临时物");
    }

    #[test]
    fn copy_dir_to_missing_target_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.txt"), b"AAA").unwrap();
        let dst = dir.path().join("dst");

        let stats = copy_object(&src, &dst, limits(), false).unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"AAA");
        assert!(src.exists());
        assert!(!has_temp(dir.path()));
    }

    #[test]
    fn copy_dir_failure_does_not_leave_partial_target_or_temp() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(src.join(name), b"x").unwrap();
        }
        let dst = dir.path().join("dst");

        let err = copy_object(
            &src,
            &dst,
            CopyLimits {
                max_entries: 1,
                max_depth: 4,
            },
            false,
        )
        .unwrap_err();
        assert_eq!(err.code(), "fs.too_large");
        assert!(!dst.exists(), "失败时目标目录不得出现");
        assert!(!has_temp(dir.path()), "失败后必须清理临时目录");
    }
}
