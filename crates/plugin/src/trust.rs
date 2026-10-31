//! 安装期信任判定（TOFU 三段式，§4.5）。
//!
//! **判定只有这一份**：CLI（`skiff plugin install`）与 Web（上传 → 确认安装）
//! 都调 `resolve()`，各自只负责把错误翻成自己语境下的话术。
//!
//! 之前两边各写了一遍，分支顺序与 `allow_unsigned` 的短路时机已经不同：
//! CLI 用 `--allow-unsigned || cfg.allow_unsigned`，Web 只看 `cfg.allow_unsigned`；
//! Web 在「未导入公钥 + 允许未签名」时直接返回探针结果（不再走一次验签），
//! CLI 则重新验一遍。这类分叉不会立刻报错，只会让「同一条命令与同一张表单
//! 给出不同结论」，事后极难复现。
//!
//! 三段式语义（按顺序，先命中者生效）：
//! 1. 包内 `key_id` 命中信任库 → 用该公钥验签，失败即拒；
//! 2. 未命中但 `allow_unsigned` → 允许安装，**标记为未验签**（UI 必须持续标红）；
//! 3. 未命中且不允许未签名 → 拒，并把指纹回给调用方提示「先导入公钥」。

use std::path::Path;

use skiff_pkgfmt::package::{verify_file, PackageMeta, VerifyOptions, DEFAULT_LIMITS};
use skiff_store::Db;

use crate::PluginError;

/// 判定结果：调用方拿 `meta` 展示、拿 `trusted_key` 交给 `InstallOptions`。
#[derive(Debug)]
pub struct TrustDecision {
    pub meta: PackageMeta,
    /// 命中信任库的公钥（`None` 表示这次安装依赖 `allow_unsigned` 放行）
    pub trusted_key: Option<String>,
    /// 是否已验签（= `meta.signature_verified`，单独列出避免调用方理解错）
    pub signature_verified: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// 未导入公钥且未允许未签名：带上指纹让调用方提示用户
    #[error("未导入该包的公钥（指纹 {key_id}）")]
    UnverifiedKey { key_id: String },
    /// 包本身有问题（格式、哈希、签名不匹配、架构不符……）
    #[error("{0}")]
    Package(String),
    /// 读信任库失败
    #[error("读取信任库失败: {0}")]
    Store(String),
}

impl From<TrustError> for PluginError {
    fn from(e: TrustError) -> Self {
        match e {
            TrustError::Package(m) => PluginError::Rejected(m),
            other => PluginError::Rejected(other.to_string()),
        }
    }
}

/// 三段式判定。`allow_unsigned` 由调用方合成（CLI 是 `--allow-unsigned || cfg`，
/// Web 是 `cfg.allow_unsigned`；合成规则留在调用方，判定顺序留在这里）。
pub fn resolve(path: &Path, db: &Db, allow_unsigned: bool) -> Result<TrustDecision, TrustError> {
    // 第一段：只为读出 `key_id` 的探针。`allow_unsigned` 不改变后续判定，
    // 否则「未签名包」连指纹都拿不到，用户没法知道该导入哪把公钥。
    let probe = verify_file(
        path,
        &VerifyOptions {
            trusted_public_key: None,
            allow_unsigned: true,
            limits: DEFAULT_LIMITS,
            arch: skiff_pkgfmt::host_arch(),
        },
    )
    .map_err(|e| TrustError::Package(e.to_string()))?;

    let trusted = db
        .trusted_key(&probe.manifest.key_id)
        .map_err(|e| TrustError::Store(e.to_string()))?;

    decide(&trusted, allow_unsigned, probe.key_id.as_str())?;

    match (&trusted, allow_unsigned) {
        // 有公钥：必须真验签，失败即拒
        (Some(_), _) => {
            let meta = verify_file(
                path,
                &VerifyOptions {
                    trusted_public_key: trusted.clone(),
                    allow_unsigned: false,
                    limits: DEFAULT_LIMITS,
                    arch: skiff_pkgfmt::host_arch(),
                },
            )
            .map_err(|e| TrustError::Package(e.to_string()))?;
            Ok(TrustDecision {
                signature_verified: meta.signature_verified,
                meta,
                trusted_key: trusted,
            })
        }
        // 无公钥 + 允许未签名：接受探针结果，但如实标记未验签
        (None, true) => Ok(TrustDecision {
            signature_verified: probe.signature_verified,
            meta: probe,
            trusted_key: None,
        }),
        // 已由 decide() 拦下，这里不可达
        (None, false) => Err(TrustError::UnverifiedKey {
            key_id: String::new(),
        }),
    }
}

/// 纯判定（不含 I/O），便于穷举三种输入组合。
fn decide(trusted: &Option<String>, allow_unsigned: bool, key_id: &str) -> Result<(), TrustError> {
    if trusted.is_some() || allow_unsigned {
        return Ok(());
    }
    Err(TrustError::UnverifiedKey {
        key_id: key_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_is_unchanged_by_caller() {
        // 有公钥：无论是否允许未签名，都必须走验签
        assert!(decide(&Some("pk".into()), false, "k").is_ok());
        assert!(decide(&Some("pk".into()), true, "k").is_ok());
        // 无公钥 + 允许未签名：放行（调用方负责标红）
        assert!(decide(&None, true, "k").is_ok());
        // 无公钥 + 不允许：拒，且必须带上指纹
        match decide(&None, false, "abc123").unwrap_err() {
            TrustError::UnverifiedKey { key_id } => assert_eq!(key_id, "abc123"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_package_is_a_package_error() {
        let dir = std::env::temp_dir().join("skiff-trust-test");
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("trust.db")).unwrap();
        let err = resolve(&dir.join("nope.wbpkg"), &db, false).unwrap_err();
        assert!(matches!(err, TrustError::Package(_)), "{err:?}");
    }
}
