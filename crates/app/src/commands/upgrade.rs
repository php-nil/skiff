//! 内核升级只读段（Skiff-server P3）：升级提示、手动下载与摘要校验。
//!
//! **不执行升级**：不停服、不替换包、不跑迁移。本模块只把已验证渠道索引里声明的
//! 升级包下载到 `<data_dir>/upgrade/incoming/` 并核对 `size` + `sha256`；签名锚点是
//! 渠道 manifest（索引摘要由固定公钥验签），升级包本身不再单独签名
//! （`docs/SKIFF_SERVER.md` §9 决策 4）。
//!
//! 拒绝原因与商店安装共用一套 `reason=<code>` 口径，由 Web 层落 `upgrade.download`
//! 审计。任何失败都清理 `.part` 与目标文件，不留下半包。

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use skiff_plugin::install::version_gt;
use tracing::info;

use crate::catalog::{CatalogService, DownloadError};

/// 当前内核版本：与 `skiff-core` 落 `AppState.version` 的来源一致（workspace 统一版本）。
pub const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 升级包暂存目录（相对 `data_dir`）。
pub const UPGRADE_DIR: &str = "upgrade";

/// 暂存子目录：只放待人工处理的升级包与下载中的 `.part`。
pub const INCOMING_DIR: &str = "incoming";

/// 宿主机架构：清单 `arch` 口径（`x86_64` / `aarch64`），不沿用包内 `amd64`/`arm64`。
pub fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => "unsupported",
    }
}

/// 一次手动下载的输入。
#[derive(Debug, Clone)]
pub struct UpgradeDownloadRequest {
    /// 目标版本；`None` 表示渠道里高于当前版本的最高版本。
    pub version: Option<String>,
    pub arch: String,
    pub current: String,
    pub data_dir: PathBuf,
}

/// 已下载并通过摘要校验的暂存升级包。
#[derive(Debug, Clone)]
pub struct StagedUpgrade {
    pub version: String,
    pub arch: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub channel: String,
    /// 命中的镜像基地址。
    pub source: String,
    /// 磁盘上已有且摘要一致时为 true：本次没有重新下载。
    pub reused: bool,
}

#[derive(Debug)]
pub enum UpgradeDownloadError {
    /// 商店目录未启用。
    Disabled,
    /// 未配置固定公钥：升级包无法确认来自可信索引，不提供下载。
    KeyMissing,
    /// 已验证目录里没有该版本/架构的升级条目（含渠道未发布升级数据）。
    NotFound(String),
    /// 目标版本不高于当前版本：拒绝降级与同版本重下。
    Downgrade(String),
    /// 网络、HTTP 非 200 或传输层错误。
    Download(String),
    /// 条目缺三件套、大小或 sha256 不符。
    Digest(String),
    /// 读/写暂存目录失败。
    Io(String),
}

impl UpgradeDownloadError {
    /// 稳定的原因码：审计与页面提示共用，与 `StoreInstallError::reason` 同口径。
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::KeyMissing => "no_key",
            Self::NotFound(_) => "not_found",
            Self::Downgrade(_) => "downgrade",
            Self::Download(_) => "download",
            Self::Digest(_) => "digest",
            Self::Io(_) => "io",
        }
    }

    pub fn message(&self) -> String {
        let detail = match self {
            Self::Disabled => "商店目录未启用".to_string(),
            Self::KeyMissing => "未配置固定 Ed25519 公钥，不提供升级下载".to_string(),
            Self::NotFound(m)
            | Self::Downgrade(m)
            | Self::Download(m)
            | Self::Digest(m)
            | Self::Io(m) => m.clone(),
        };
        format!("[{}] {detail}", self.reason())
    }
}

impl std::fmt::Display for UpgradeDownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for UpgradeDownloadError {}

impl From<DownloadError> for UpgradeDownloadError {
    fn from(value: DownloadError) -> Self {
        match value {
            DownloadError::Download(m) => Self::Download(m),
            DownloadError::Digest(m) => Self::Digest(m),
            DownloadError::Io(m) => Self::Io(m),
        }
    }
}

/// 取条目 → 拒绝降级 → 下载到暂存 → 校验 sha256。做阻塞 I/O，调用方必须放进
/// `spawn_blocking`。
pub fn download(
    catalog: &CatalogService,
    req: &UpgradeDownloadRequest,
) -> Result<StagedUpgrade, UpgradeDownloadError> {
    if !catalog.enabled() {
        return Err(UpgradeDownloadError::Disabled);
    }
    if catalog.settings().public_key_hex().is_none() {
        return Err(UpgradeDownloadError::KeyMissing);
    }

    let package = match req.version.as_deref() {
        Some(version) => catalog.upgrade_package(version, &req.arch),
        None => catalog.latest_upgrade(&req.current, &req.arch),
    }
    .ok_or_else(|| {
        let want = req.version.as_deref().unwrap_or("最新");
        UpgradeDownloadError::NotFound(format!("已验证目录中没有 {want}（{}）升级条目", req.arch))
    })?;

    if !version_gt(&package.entry.version, &req.current) {
        return Err(UpgradeDownloadError::Downgrade(format!(
            "拒绝降级或同版本重下：当前 {}，目标 {}",
            req.current, package.entry.version
        )));
    }

    let sha = package
        .entry
        .pkg_sha256
        .as_deref()
        .ok_or_else(|| UpgradeDownloadError::Digest("目录条目缺少内容摘要".into()))?;
    let size = package
        .entry
        .size
        .ok_or_else(|| UpgradeDownloadError::Digest("目录条目缺少字节数".into()))?;

    let dir = req.data_dir.join(UPGRADE_DIR).join(INCOMING_DIR);
    let dest = dir.join(format!(
        "skiff-{}-{}.tar.gz",
        package.entry.version, package.entry.arch
    ));

    let mut reused = false;
    if dest.is_file() {
        match verify_file(&dest, sha, size) {
            Ok(()) => reused = true,
            Err(e) => {
                // 暂存被截断/替换：丢弃后重下，绝不把可疑文件当已下载。
                tracing::warn!(
                    path = %dest.display(),
                    error = %e,
                    "暂存升级包校验失败，丢弃后重新下载"
                );
                let _ = fs::remove_file(&dest);
            }
        }
    }

    if !reused {
        fs::create_dir_all(&dir).map_err(|e| UpgradeDownloadError::Io(e.to_string()))?;
        catalog.download_upgrade(&package, &dest)?;
    }

    info!(
        version = %package.entry.version,
        arch = %package.entry.arch,
        path = %dest.display(),
        sha256 = %sha,
        reused,
        "内核升级包已暂存并通过摘要校验"
    );

    Ok(StagedUpgrade {
        version: package.entry.version,
        arch: package.entry.arch,
        path: dest,
        sha256: sha.to_string(),
        size,
        channel: package.channel,
        source: package.base,
        reused,
    })
}

/// 复核磁盘上已有暂存包的大小与 sha256；任一不符都视为不可用。
fn verify_file(path: &Path, sha256: &str, size: u64) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| format!("读取暂存包失败: {e}"))?;
    if bytes.len() as u64 != size {
        return Err(format!(
            "大小不符：清单 {size} 字节，本地 {} 字节",
            bytes.len()
        ));
    }
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != sha256 {
        return Err(format!("sha256 不符：清单 {sha256}，本地 {actual}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use crate::catalog::fetch::testing::FakeTransport;
    use crate::catalog::fetch::HttpReply;
    use crate::catalog::model::{INDEX_SCHEMA, MANIFEST_SCHEMA};
    use crate::catalog::verify::testing::{sign_hello, test_key, TestKey};
    use crate::catalog::CatalogSettings;

    use super::*;

    const MIRROR: &str = "https://mirror.example/skiff";

    struct Fixture {
        dir: tempfile::TempDir,
        fake: Arc<FakeTransport>,
        key: TestKey,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                fake: FakeTransport::new(),
                key: test_key(7),
            }
        }

        fn service(&self) -> Arc<CatalogService> {
            CatalogService::with_transport(
                CatalogSettings {
                    enabled: true,
                    channel: "stable".into(),
                    mirrors: vec![MIRROR.into()],
                    public_key: Some(self.key.public_hex.clone()),
                    allow_unsigned: false,
                    timeout: std::time::Duration::from_secs(1),
                },
                self.dir.path().join("catalog"),
                self.fake.clone(),
            )
        }

        fn service_without_key(&self) -> Arc<CatalogService> {
            CatalogService::with_transport(
                CatalogSettings {
                    enabled: true,
                    channel: "stable".into(),
                    mirrors: vec![MIRROR.into()],
                    public_key: None,
                    allow_unsigned: false,
                    timeout: std::time::Duration::from_secs(1),
                },
                self.dir.path().join("catalog"),
                self.fake.clone(),
            )
        }

        fn request(&self, version: Option<&str>) -> UpgradeDownloadRequest {
            UpgradeDownloadRequest {
                version: version.map(str::to_string),
                arch: "x86_64".into(),
                current: "0.1.0".into(),
                data_dir: self.dir.path().join("data"),
            }
        }

        fn incoming(&self) -> PathBuf {
            self.dir
                .path()
                .join("data")
                .join(UPGRADE_DIR)
                .join(INCOMING_DIR)
        }

        fn files(&self) -> Vec<String> {
            fs::read_dir(self.incoming())
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default()
        }

        fn blob_calls(&self) -> usize {
            self.fake
                .calls()
                .iter()
                .filter(|(url, _)| url.contains("/blobs/sha256/"))
                .count()
        }

        /// 发布一份升级条目（可空），并让 blob 地址返回 `payload`；返回声明的 sha256。
        fn serve(&self, upgrades: &[(&str, &str)], payload: &[u8]) -> String {
            self.serve_inner(upgrades, payload, payload)
        }

        /// 条目声明 `declared` 的摘要/大小，但 blob 实际返回 `served`（模拟篡改）。
        fn serve_tampered(
            &self,
            upgrades: &[(&str, &str)],
            declared: &[u8],
            served: &[u8],
        ) -> String {
            self.serve_inner(upgrades, declared, served)
        }

        fn serve_inner(&self, upgrades: &[(&str, &str)], declared: &[u8], served: &[u8]) -> String {
            let sha = hex::encode(Sha256::digest(declared));
            let entries: Vec<_> = upgrades
                .iter()
                .map(|(version, arch)| {
                    serde_json::json!({
                        "version": version,
                        "arch": arch,
                        "channel": "stable",
                        "min_from": "0.1.0",
                        "changelog_url": "https://example.invalid/changelog",
                        "pkg_sha256": sha,
                        "pkg_url": format!("blobs/sha256/{sha}"),
                        "size": declared.len()
                    })
                })
                .collect();
            let index = serde_json::to_vec(&serde_json::json!({
                "schema": INDEX_SCHEMA,
                "channel": "stable",
                "seq": 1,
                "generated_at": "2026-11-10T00:00:00Z",
                "upgrades": entries
            }))
            .unwrap();
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schema": MANIFEST_SCHEMA,
                "channel": "stable",
                "seq": 1,
                "generated_at": "2026-11-10T00:00:00Z",
                "key_id": self.key.key_id(),
                "index": {
                    "path": "index.json",
                    "sha256": hex::encode(Sha256::digest(&index)),
                    "size": index.len()
                }
            }))
            .unwrap();
            let sig = serde_json::to_vec(&serde_json::json!({
                "alg": "ed25519",
                "key_id": self.key.key_id(),
                "sig": sign_hello(&self.key, &manifest)
            }))
            .unwrap();
            self.fake.push(
                &format!("{MIRROR}/channels/stable/manifest.json"),
                HttpReply {
                    status: 200,
                    body: manifest,
                    etag: None,
                    last_modified: None,
                },
            );
            self.fake.push(
                &format!("{MIRROR}/channels/stable/manifest.json.sig"),
                HttpReply {
                    status: 200,
                    body: sig,
                    etag: None,
                    last_modified: None,
                },
            );
            self.fake.push(
                &format!("{MIRROR}/channels/stable/index.json"),
                HttpReply {
                    status: 200,
                    body: index,
                    etag: None,
                    last_modified: None,
                },
            );
            if !upgrades.is_empty() {
                self.fake.push(
                    &format!("{MIRROR}/blobs/sha256/{sha}"),
                    HttpReply {
                        status: 200,
                        body: served.to_vec(),
                        etag: None,
                        last_modified: None,
                    },
                );
            }
            sha
        }
    }

    #[test]
    fn missing_upgrade_data_is_not_found_and_touches_no_blob() {
        let fx = Fixture::new();
        fx.serve(&[], b"unused");
        let err = download(&fx.service(), &fx.request(None)).unwrap_err();
        assert_eq!(err.reason(), "not_found");
        assert!(fx.files().is_empty());
        assert_eq!(fx.blob_calls(), 0);
    }

    #[test]
    fn download_requires_configured_key() {
        let fx = Fixture::new();
        let err = download(&fx.service_without_key(), &fx.request(None)).unwrap_err();
        assert_eq!(err.reason(), "no_key");
        assert!(fx.fake.calls().is_empty(), "缺公钥不得触网");
    }

    #[test]
    fn download_rejects_downgrade_and_same_version() {
        let fx = Fixture::new();
        fx.serve(&[("0.1.0", "x86_64")], b"pkg");
        let svc = fx.service();
        assert_eq!(
            download(&svc, &fx.request(Some("0.1.0")))
                .unwrap_err()
                .reason(),
            "downgrade"
        );
        assert_eq!(fx.blob_calls(), 0);
        assert!(fx.files().is_empty());
    }

    #[test]
    fn download_stages_and_verifies_then_reuses_existing_file() {
        let fx = Fixture::new();
        let payload = b"skiff-upgrade-payload";
        let sha = fx.serve(&[("0.2.0", "x86_64")], payload);
        let svc = fx.service();

        let staged = download(&svc, &fx.request(None)).expect("下载应成功");
        assert_eq!(staged.version, "0.2.0");
        assert_eq!(staged.sha256, sha);
        assert_eq!(staged.size, payload.len() as u64);
        assert!(!staged.reused);
        assert_eq!(fs::read(&staged.path).unwrap(), payload);
        assert_eq!(staged.path, fx.incoming().join("skiff-0.2.0-x86_64.tar.gz"));
        assert_eq!(fx.files(), vec!["skiff-0.2.0-x86_64.tar.gz".to_string()]);
        let calls = fx.blob_calls();
        assert_eq!(calls, 1);

        // 第二次：磁盘文件摘要一致 → 直接复用，不再发 blob 请求。
        let again = download(&svc, &fx.request(None)).expect("复用应成功");
        assert!(again.reused);
        assert_eq!(fx.blob_calls(), calls);
    }

    #[test]
    fn digest_mismatch_discards_partial_and_leaves_nothing() {
        let fx = Fixture::new();
        // 同长度、不同内容：大小校验通过，sha256 必须失败。
        fx.serve_tampered(&[("0.2.0", "x86_64")], b"expected", b"tampered");
        let err = download(&fx.service(), &fx.request(None)).unwrap_err();
        assert_eq!(err.reason(), "digest");
        assert!(
            fx.files().is_empty(),
            "摘要不符不得留下半包: {:?}",
            fx.files()
        );
    }

    #[test]
    fn tampered_staged_file_is_discarded_and_downloaded_again() {
        let fx = Fixture::new();
        let payload = b"skiff-upgrade-payload";
        fx.serve(&[("0.2.0", "x86_64")], payload);
        let svc = fx.service();
        let dest = fx.incoming().join("skiff-0.2.0-x86_64.tar.gz");
        fs::create_dir_all(fx.incoming()).unwrap();
        fs::write(&dest, b"corrupted").unwrap();

        let staged = download(&svc, &fx.request(None)).expect("坏文件应重下");
        assert!(!staged.reused);
        assert_eq!(fs::read(&staged.path).unwrap(), payload);
    }
}
