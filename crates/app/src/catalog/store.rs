//! 目录缓存：`<data_dir>/catalog/` 下的原子读写。
//!
//! 布局与 `docs/SKIFF_SERVER.md` 的服务端产物同构，只是多一层渠道目录与
//! `meta.json`（条件请求头 + 已见 seq）：
//!
//! ```text
//! catalog/
//!   meta.json                 条件请求头、已见 seq、上次成功时间
//!   channels/<name>/manifest.json
//!   channels/<name>/manifest.json.sig
//!   channels/<name>/index.json
//!   blobs/sha256/<hex>        本地已有 blob（P1 不主动下载）
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::model::{Index, Manifest};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheMeta {
    pub channel: String,
    pub seq: u64,
    /// 上次成功拉取时间（UTC RFC3339 秒级）。
    pub fetched_at: String,
    /// 命中的镜像地址（多地址回退时记录实际来源）。
    pub source: String,
    pub etag_manifest: Option<String>,
    pub last_modified_manifest: Option<String>,
    pub etag_index: Option<String>,
    pub last_modified_index: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CachedCatalog {
    pub manifest_bytes: Vec<u8>,
    pub manifest: Manifest,
    /// `manifest.json.sig` 的原始字节；未签名渠道为 `None`。
    /// 304 复用时必须原样带回，否则下次写入会把旧签名清掉。
    pub signature: Option<Vec<u8>>,
    pub index_bytes: Vec<u8>,
    pub index: Index,
    pub meta: CacheMeta,
}

pub fn channel_dir(root: &Path, channel: &str) -> PathBuf {
    root.join("channels").join(channel)
}

pub fn blob_dir(root: &Path) -> PathBuf {
    root.join("blobs").join("sha256")
}

/// 读取缓存。任何文件缺失/损坏/校验不过都返回 `None`（等价于「没有可用快照」），
/// 绝不把半截内容当成功快照使用。
pub fn load(root: &Path, channel: &str) -> Option<CachedCatalog> {
    let dir = channel_dir(root, channel);
    let meta: CacheMeta = serde_json::from_slice(&fs::read(dir.join("meta.json")).ok()?).ok()?;
    if meta.channel != channel {
        return None;
    }
    let manifest_bytes = fs::read(dir.join("manifest.json")).ok()?;
    let index_bytes = fs::read(dir.join("index.json")).ok()?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).ok()?;
    let index: Index = serde_json::from_slice(&index_bytes).ok()?;
    manifest.validate().ok()?;
    index.validate(Some(&blob_dir(root))).ok()?;
    if manifest.channel != channel || index.channel != channel || manifest.seq != index.seq {
        return None;
    }
    if manifest.seq != meta.seq {
        return None;
    }
    Some(CachedCatalog {
        manifest_bytes,
        manifest,
        signature: fs::read(dir.join("manifest.json.sig")).ok(),
        index_bytes,
        index,
        meta,
    })
}

/// 原子写入一份成功快照。先写内容文件，最后写 `meta.json`：
/// 中途失败时旧 `meta.json` 仍指向旧的完整文件集合，不会出现「meta 新、内容旧」。
pub fn save(
    root: &Path,
    channel: &str,
    manifest_bytes: &[u8],
    signature: Option<&[u8]>,
    index_bytes: &[u8],
    meta: &CacheMeta,
) -> io::Result<()> {
    let dir = channel_dir(root, channel);
    fs::create_dir_all(&dir)?;
    fs::create_dir_all(blob_dir(root))?;
    write_atomic(&dir.join("manifest.json"), manifest_bytes)?;
    match signature {
        Some(bytes) => write_atomic(&dir.join("manifest.json.sig"), bytes)?,
        None => {
            // 上次是签名渠道、这次未签名：清掉旧签名，避免缓存里出现不一致的两份证据。
            let _ = fs::remove_file(dir.join("manifest.json.sig"));
        }
    }
    write_atomic(&dir.join("index.json"), index_bytes)?;
    let json = serde_json::to_vec_pretty(meta)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&dir.join("meta.json"), &json)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}
