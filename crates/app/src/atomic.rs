//! 原子落盘用的同目录唯一临时名。
//!
//! 固定临时名（`x.tmp` / `x.conf.tmp`）在并发写同一路径时会互相截断或抢先
//! rename，表现为「替换失败：No such file or directory」或半截内容。这里统一
//! 用 `.<文件名>.<pid>.<seq>.tmp`：同目录内 rename 仍是原子的，进程内序号保证
//! 同名路径并发写也各有各的临时文件。
//!
//! 写盘本身一律走内核文件通道（`AdminFs`）：原子替换、属主/权限位保留（daemon
//! 以 `root:skiff-plugins` 运行，直接换文件会把 `config.toml` 的 group 污染成
//! `skiff-plugins`）都由 `skiff-fs` 的 `preserve_attrs` 负责，内核不再自己实现
//! 一份。这里只剩「临时文件叫什么」这一个纯命名工具，供证书这类需要在通道写入
//! 之前先落临时件的调用方使用。

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
