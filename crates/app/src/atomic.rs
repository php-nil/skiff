//! 原子落盘：同目录唯一临时名 + rename。
//!
//! 固定临时名（`x.tmp` / `x.conf.tmp`）在并发写同一路径时会互相截断或抢先
//! rename，表现为「替换失败：No such file or directory」或半截内容。这里统一
//! 用 `.<文件名>.<pid>.<seq>.tmp`：同目录内 rename 仍是原子的，进程内序号保证
//! 同名路径并发写也各有各的临时文件。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 与 `path` 同目录的唯一临时路径。
pub(crate) fn sibling_temp(path: &Path) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("skiff"),
        std::process::id(),
        seq
    ))
}

/// 写临时文件后 rename 覆盖目标；`mode` 在 Unix 上设置到临时文件。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = sibling_temp(path);
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}
