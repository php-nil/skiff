//! 前端资源 gzip 预压缩（§2.2）：为 `crates/web/static` 下每个文件生成同名 `.gz` 变体，
//! 随静态资源一起 embed；`assets.rs` 按 `Accept-Encoding` 协商选择变体。
//!
//! 生成物（`*.gz`）写回静态目录并被 `.gitignore` 忽略——每次构建重生成，
//! 不入库、不需要手工维护。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let static_dir = manifest_dir.join("static");
    let static_dir = static_dir.canonicalize().unwrap_or(static_dir);
    println!("cargo:rerun-if-changed={}", static_dir.display());

    for entry in walk(&static_dir) {
        // 预压缩变体不再压缩；缺跳过非文件
        if entry.extension().map(|e| e == "gz").unwrap_or(false) {
            continue;
        }
        // 已压缩格式（woff2 等）再 gzip 只会增加字节，还让客户端多做一次解压。
        if entry
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "woff2" | "woff" | "png" | "jpg" | "jpeg" | "webp" | "zip" | "gz"
                )
            })
            .unwrap_or(false)
        {
            continue;
        }
        let input = match fs::read(&entry) {
            Ok(v) => v,
            Err(e) => {
                println!("cargo:warning=读取 {} 失败: {e}", entry.display());
                continue;
            }
        };
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        if enc.write_all(&input).is_err() {
            continue;
        }
        let Ok(compressed) = enc.finish() else {
            continue;
        };
        let out = PathBuf::from(format!("{}.gz", entry.display()));
        if let Err(e) = fs::write(&out, &compressed) {
            println!("cargo:warning=写入 {} 失败: {e}", out.display());
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}
