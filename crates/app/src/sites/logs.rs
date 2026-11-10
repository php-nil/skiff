//! 站点与内核公共日志的读取与轮转。
//!
//! 读取只取文件尾部固定窗口，内存占用与文件大小无关；轮转是改名 + 新建，
//! 不复制数据，调用方负责让写者重新打开文件（nginx 由巡检触发 `-s reopen`）。

use std::path::Path;

use super::site_log_dir;

/// 单次日志读取的最大字节数：只看文件尾部这么多字节，避免大文件整读拖垮页面。
pub(crate) const LOG_READ_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// 读取日志尾部：`keyword` 为空不过滤；输出时间倒序（最新在前）。
///
/// 只读文件最后 [`LOG_READ_MAX_BYTES`] 字节，内存占用与文件大小无关；因此关键字
/// 过滤与 `tail` 都限制在这个窗口内。起点落在文件中间时丢弃首个半行。
pub(crate) fn read_log(path: &Path, tail: usize, keyword: &str) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(LOG_READ_MAX_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let text = if start > 0 {
        match text.find('\n') {
            // 首行是半行：跳过它；整段没有换行说明是单条超长行，直接放弃。
            Some(i) => &text[i + 1..],
            None => "",
        }
    } else {
        &text[..]
    };
    let kw = keyword.trim().to_lowercase();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| kw.is_empty() || l.to_lowercase().contains(&kw))
        .rev()
        .take(tail)
        .collect();
    Ok(lines.join("\n"))
}

/// 内核轮转：扫描日志目录，对超过阈值的 `.log` 改名轮转（不复制数据）。
///
/// 由内核巡检（30s 一次）调用，不依赖宿主 logrotate；改名后调用方负责让
/// nginx 重新打开日志，否则写入会继续落在 `.1` 上。
pub fn rotate_logs(dir: &Path, max_bytes: u64) -> std::io::Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut rotated = 0;
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "log") && rotate_if_large(&path, max_bytes)? {
            rotated += 1;
        }
    }
    Ok(rotated)
}

/// 扫描 `sites/` 下一层站点，各自轮转 `log/` 目录（内核巡检用）。
pub fn rotate_site_logs(sites_dir: &Path, max_bytes: u64) -> std::io::Result<usize> {
    if !sites_dir.is_dir() {
        return Ok(0);
    }
    let mut rotated = 0;
    for entry in std::fs::read_dir(sites_dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rotated += rotate_logs(&site_log_dir(&path), max_bytes)?;
        }
    }
    Ok(rotated)
}

/// 内核轮转：达到阈值时把文件改名为 `.1` 并新建同名空文件。
///
/// 改名不复制数据，写放大与文件大小无关；不改 inode 语义的前提是写者重新打开
/// 文件——nginx 由 30s 巡检在轮转后触发 `nginx -s reopen`，新文件立即接续写入。
pub(crate) fn rotate_if_large(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.len() < max_bytes {
        return Ok(false);
    }
    let rotated = path.with_extension(format!(
        "{}.1",
        path.extension().and_then(|e| e.to_str()).unwrap_or("log")
    ));
    // Windows 的 rename 不能覆盖已存在目标；Unix 上是原子覆盖，先删也无害。
    if let Err(e) = std::fs::remove_file(&rotated) {
        tracing::debug!(path = %rotated.display(), error = %e, "清理上次轮转的日志失败");
    }
    std::fs::rename(path, &rotated)?;
    std::fs::File::create(path)?;
    Ok(true)
}
