//! `.wbpkg` 包格式、manifest 与签名编解码（§4.1）。
//!
//! 这是内核与 SDK 共用的**唯一**格式实现：签名输入的字节编码、`files.sha256` 的
//! 覆盖范围与排序规则在此收口，避免「打包器与校验器各写一遍、字节构造不一致」。

pub mod manifest;
pub mod package;
pub mod signature;

pub use manifest::{
    Manifest, ManifestMenu, ManifestMount, ManifestPerm, MenuItem, RunMode, UiEntry,
};
pub use package::{
    host_arch, pack_dir, verify_file, PackageLimits, PackageMeta, UnpackedPackage, DEFAULT_LIMITS,
};
pub use signature::{decode_public_key, key_id_of, verify_bytes, SigningKey};

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("包格式错误: {0}")]
    Format(String),
    #[error("不支持的包内容: {0}")]
    Unsupported(String),
    #[error("签名校验失败: {0}")]
    Signature(String),
    #[error("哈希清单不一致: {0}")]
    Hash(String),
    #[error("manifest 非法: {0}")]
    Manifest(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("压缩流错误: {0}")]
    Zstd(String),
    #[error("tar 解析失败: {0}")]
    Tar(String),
}

pub type Result<T> = std::result::Result<T, PkgError>;
