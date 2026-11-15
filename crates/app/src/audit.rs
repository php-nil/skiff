//! 审计兜底落盘。
//!
//! 审计主路径是**同步写 SQLite**（`AppState::audit_or_log`）：接口返回前记录已经
//! 在库里，不存在「异步队列满被丢弃」「进程崩溃丢尾部」的窗口。只有数据库写入
//! 本身失败（磁盘满、IO 错误、库被锁死）时才走这里：把同一条记录追加到数据根下
//! 的 `audit-fallback.jsonl` 并 `fsync`，保证记录不因写库失败而消失。
//!
//! 明确放弃的方案：有界队列 + 后台批量落库。它的收益只是把几毫秒的插入挪出请求
//! 路径，代价是队列满、进程被杀、重启丢尾部三种丢记录场景，与「审计不丢」直接
//! 冲突。审计量级是「每个写操作一条」，同步插入的延迟可以接受。

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// 兜底文件的固定文件名（位于数据根，0711 目录内，仅服务进程可写）。
pub(crate) const FALLBACK_FILE: &str = "audit-fallback.jsonl";

#[derive(Debug, Serialize)]
pub(crate) struct FallbackEntry<'a> {
    pub ts: i64,
    pub actor: &'a str,
    pub ip: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub result: &'a str,
    pub detail: &'a str,
}

/// 追加一条兜底记录并刷盘，返回文件路径。
///
/// 追加语义：不重写、不截断，进程崩溃最多丢掉「正在写的那一行」之前的缓冲空档，
/// 而 `sync_data` 保证已经返回成功的行确实在磁盘上。
pub(crate) fn append_fallback(
    data_dir: &Path,
    entry: &FallbackEntry<'_>,
) -> std::io::Result<PathBuf> {
    let path = data_dir.join(FALLBACK_FILE);
    let mut line = serde_json::to_vec(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(action: &'a str) -> FallbackEntry<'a> {
        FallbackEntry {
            ts: 1_700_000_000,
            actor: "admin",
            ip: "127.0.0.1",
            action,
            target: "site:example.com",
            result: "ok",
            detail: "detail",
        }
    }

    #[test]
    fn appends_one_json_line_per_entry_and_keeps_previous_lines() {
        let dir = std::env::temp_dir().join(format!(
            "skiff-audit-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = append_fallback(&dir, &entry("site.create")).expect("第一次追加");
        append_fallback(&dir, &entry("site.delete")).expect("第二次追加");

        let text = std::fs::read_to_string(&path).expect("读取兜底文件");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "必须是追加而不是覆盖");
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("第一行是 JSON");
        assert_eq!(first["action"], "site.create");
        assert_eq!(first["ts"], 1_700_000_000i64);
        assert_eq!(first["actor"], "admin");

        std::fs::remove_dir_all(&dir).ok();
    }
}
