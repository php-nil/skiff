//! `.wbpkg` 校验：直接复用 `skiff-pkgfmt`，不另写一套验签。
//!
//! 信任锚是请求者自己的公钥（白名单里登记的），而不是 TOFU 信任库：
//! `VerifyOptions.trusted_public_key = Some(请求者公钥)` 会强制包内 `key_id`
//! 与该指纹一致并验签，`allow_unsigned = false`。

use std::path::Path;

use skiff_pkgfmt::package::{host_arch, verify_file, PackageMeta, VerifyOptions};
use skiff_pkgfmt::PkgError;

use super::ApiError;

/// 校验失败一律 422；签名专项失败给独立 `code`，便于调用方区分。
pub fn verify(path: &Path, requester_public_key: &str) -> Result<PackageMeta, ApiError> {
    let options = VerifyOptions {
        trusted_public_key: Some(requester_public_key.to_string()),
        allow_unsigned: false,
        limits: skiff_pkgfmt::package::DEFAULT_LIMITS,
        arch: host_arch(),
    };
    verify_file(path, &options).map_err(|error| match error {
        PkgError::Signature(message) => {
            ApiError::new(422, "invalid_signature", format!("包验签失败: {message}"))
        }
        other => ApiError::new(422, "invalid_package", format!("包校验失败: {other}")),
    })
}
