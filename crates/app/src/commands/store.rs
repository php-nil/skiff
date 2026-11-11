//! 商店安装命令（Skiff-server P2）：目录条目 → 下载 → 摘要/大小 → 验签 → 权限确认。
//!
//! 只做「把已验证目录里的插件包安全地落到 `.incoming`，并给出确认页所需事实」；
//! 最终安装仍复用 `web` 确认页与 `skiff_plugin::install_package`（同一套安装、
//! 授权与审计口径）。本模块不写库、不做权限判定，也不改变现有信任判定。
//!
//! 拒绝语义按原因分开（下载 / 摘要 / 验签 / 兼容性 / 吊销 / 未启用 / 缺公钥），
//! 由调用方落审计并给出页面提示；任何一步失败都清理暂存文件，不留半装状态。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use skiff_plugin::TrustDecision;
use skiff_store::Db;
use tracing::{info, warn};

use crate::catalog::{CatalogService, DownloadError, PluginPackage};
use crate::tasks::KERNEL_API;

/// 暂存包最长保留时长：放弃权限确认页后由启动/准备安装前的清扫回收。
pub const INCOMING_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 清理 `.incoming` 下超过 `ttl` 的暂存物（进程启动、每次准备安装前各调用一次）。
///
/// 只删单次安装的中间产物（`<token>.wbpkg` 与下载中的 `.part`），目录里其它文件
/// 一律不碰；单个文件删除失败只留痕，不影响调用方流程。
pub fn cleanup_stale_incoming(data_dir: &Path, ttl: Duration) -> usize {
    let removed = cleanup_stale_incoming_at(
        &data_dir.join("plugins").join(".incoming"),
        SystemTime::now(),
        ttl,
    );
    if removed > 0 {
        info!(removed, ttl_secs = ttl.as_secs(), "已清理过期插件暂存包");
    }
    removed
}

/// `now` 显式传入以便单测精确控制年龄，不依赖修改 mtime。
fn cleanup_stale_incoming_at(dir: &Path, now: SystemTime, ttl: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let staged = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("wbpkg" | "part")
        );
        if !staged {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        // 读不到 mtime 或 mtime 在当前时间之后（时钟回拨）时保守跳过，避免误删新文件。
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < ttl {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => warn!(path = %path.display(), error = %e, "清理过期插件暂存包失败"),
        }
    }
    removed
}

/// 一次商店安装的输入（不含展示字段）。
#[derive(Debug, Clone)]
pub struct StoreInstallRequest {
    pub plugin_id: String,
    /// 当前部署形态（能力面收缩口径随之变化）。
    pub form: skiff_model::Form,
    /// 有效未签名开关（与上传安装一致：只看配置，不看命令行标志）。
    pub allow_unsigned: bool,
    pub allow_reduced_sandbox: bool,
    pub data_dir: PathBuf,
}

/// 已暂存、等待权限确认的安装。
#[derive(Debug)]
pub struct PreparedInstall {
    /// 确认页隐藏字段；对应 `.incoming/<token>.wbpkg`。
    pub token: String,
    pub path: PathBuf,
    pub trust: TrustDecision,
    pub channel: String,
    /// 命中的镜像基地址。
    pub source: String,
}

#[derive(Debug)]
pub enum StoreInstallError {
    /// 商店目录未启用。
    Disabled,
    /// 未配置固定公钥：不提供安装入口。
    KeyMissing,
    /// 已验证目录里没有该插件（目录未更新/被拒绝/版本不存在）。
    NotFound(String),
    /// 条目已被服务端撤回。
    Revoked(String),
    /// 内核 API、能力面或版本不满足。
    Compatibility(String),
    /// 网络/HTTP 失败。
    Download(String),
    /// 摘要或大小与目录不符（含条目缺三件套）。
    Digest(String),
    /// 插件包验签/结构校验失败。
    Signature(String),
    /// 暂存写盘失败。
    Io(String),
    /// 读库失败。
    Store(String),
}

impl StoreInstallError {
    /// 稳定的原因码：审计与页面提示共用，便于事后按原因筛选。
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::KeyMissing => "no_key",
            Self::NotFound(_) => "not_found",
            Self::Revoked(_) => "revoked",
            Self::Compatibility(_) => "compatibility",
            Self::Download(_) => "download",
            Self::Digest(_) => "digest",
            Self::Signature(_) => "signature",
            Self::Io(_) => "io",
            Self::Store(_) => "store",
        }
    }

    /// 页面提示：原因码 + 具体说明。
    pub fn message(&self) -> String {
        let detail = match self {
            Self::Disabled => "商店目录未启用".to_string(),
            Self::KeyMissing => "未配置固定 Ed25519 公钥，不提供商店安装".to_string(),
            Self::NotFound(m)
            | Self::Revoked(m)
            | Self::Compatibility(m)
            | Self::Download(m)
            | Self::Digest(m)
            | Self::Signature(m)
            | Self::Io(m)
            | Self::Store(m) => m.clone(),
        };
        format!("[{}] {detail}", self.reason())
    }
}

impl std::fmt::Display for StoreInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for StoreInstallError {}

/// 目录条目 → 暂存包 → 验签，返回确认页所需事实。
///
/// 任一步失败都保证 `.incoming` 下没有残留：下载失败由 `download_blob` 清理，
/// 验签失败在这里清理。做阻塞 I/O，调用方必须放进 `spawn_blocking`。
pub fn prepare(
    catalog: &CatalogService,
    db: &Db,
    req: &StoreInstallRequest,
) -> Result<PreparedInstall, StoreInstallError> {
    // 放弃确认页会遗留暂存包：每次准备安装前先扫一次过期文件。
    cleanup_stale_incoming(&req.data_dir, INCOMING_TTL);

    if !catalog.enabled() {
        return Err(StoreInstallError::Disabled);
    }
    if catalog.settings().public_key_hex().is_none() {
        return Err(StoreInstallError::KeyMissing);
    }

    let package = catalog
        .plugin_package(&req.plugin_id, None)
        .ok_or_else(|| {
            StoreInstallError::NotFound(format!(
                "已验证目录中没有插件 {}（目录未更新、校验失败或条目不存在）",
                req.plugin_id
            ))
        })?;
    check_compatibility(db, req, &package)?;

    let token = new_token();
    let path = req
        .data_dir
        .join("plugins")
        .join(".incoming")
        .join(format!("{token}.wbpkg"));

    catalog
        .download_blob(&package, &path)
        .map_err(|e| match e {
            DownloadError::Download(m) => StoreInstallError::Download(m),
            DownloadError::Digest(m) => StoreInstallError::Digest(m),
            DownloadError::Io(m) => StoreInstallError::Io(m),
        })?;

    let trust = match skiff_plugin::resolve_trust(&path, db, req.allow_unsigned) {
        Ok(trust) => trust,
        Err(e) => {
            // 验签失败即删暂存：确认页不会再出现，文件也不能留在磁盘上。
            let _ = fs::remove_file(&path);
            return Err(match e {
                skiff_plugin::TrustError::UnverifiedKey { key_id } => StoreInstallError::Signature(
                    format!("未导入该包的公钥（指纹 {key_id}）；未签名包默认拒装"),
                ),
                other => StoreInstallError::Signature(other.to_string()),
            });
        }
    };

    Ok(PreparedInstall {
        token,
        path,
        trust,
        channel: package.channel,
        source: package.base,
    })
}

/// 安装前兼容性：内核 API、能力面、版本（降级拒绝）。
fn check_compatibility(
    db: &Db,
    req: &StoreInstallRequest,
    package: &PluginPackage,
) -> Result<(), StoreInstallError> {
    let entry = &package.entry;
    if let Some(reason) = &package.revoked {
        return Err(StoreInstallError::Revoked(format!(
            "{} {} 已被服务端撤回：{reason}",
            entry.id, entry.version
        )));
    }
    if entry.yanked {
        let detail = entry
            .yank_reason
            .as_deref()
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| format!("{} {} 已被服务端撤回：{reason}", entry.id, entry.version))
            .unwrap_or_else(|| format!("{} {} 已被服务端撤回", entry.id, entry.version));
        return Err(StoreInstallError::Revoked(detail));
    }
    if !api_compatible(&entry.api, KERNEL_API) {
        return Err(StoreInstallError::Compatibility(format!(
            "插件要求内核 API {}，当前内核为 {KERNEL_API}",
            entry.api
        )));
    }
    let available = skiff_pkgfmt::manifest::Capability::available_in(req.form);
    let missing: Vec<&str> = entry
        .requires
        .iter()
        .map(String::as_str)
        .filter(|r| !available.iter().any(|c| c.as_str() == *r))
        .collect();
    if !missing.is_empty() {
        return Err(StoreInstallError::Compatibility(format!(
            "当前部署形态不提供能力面：{}",
            missing.join(", ")
        )));
    }
    let prev = db
        .plugin(&entry.id)
        .map_err(|e| StoreInstallError::Store(e.to_string()))?;
    if let Some(prev) = prev {
        if !skiff_plugin::install::version_gt(&entry.version, &prev.version)
            && entry.version != prev.version
        {
            return Err(StoreInstallError::Compatibility(format!(
                "已安装版本 {} 高于目录版本 {}；不接受降级（同版本可直接重装）",
                prev.version, entry.version
            )));
        }
    }
    Ok(())
}

/// 条目声明的 `api` 与内核 `KERNEL_API` 是否兼容：主版本必须一致，次版本不高于内核。
fn api_compatible(required: &str, kernel: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64)> {
        let (major, minor) = v.split_once('.')?;
        if major.is_empty()
            || minor.is_empty()
            || !major.bytes().all(|b| b.is_ascii_digit())
            || !minor.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        Some((major.parse().ok()?, minor.parse().ok()?))
    }
    match (parse(required), parse(kernel)) {
        (Some((rm, rn)), Some((km, kn))) => rm == km && rn <= kn,
        _ => false,
    }
}

/// 确认令牌：24 位字母数字，与上传安装的 `.incoming/<token>.wbpkg` 同一格式，
/// 确认页 POST 到 `/plugins/confirm` 时由同一段校验接受。
fn new_token() -> String {
    skiff_identity::new_token()
        .replace(['-', '_'], "")
        .chars()
        .take(24)
        .collect()
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
    const PLUGIN_ID: &str = "com.example.demo";

    struct Fixture {
        dir: tempfile::TempDir,
        fake: Arc<FakeTransport>,
        catalog_key: TestKey,
        plugin_key: skiff_pkgfmt::SigningKey,
        db: Db,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                fake: FakeTransport::new(),
                catalog_key: test_key(7),
                plugin_key: skiff_pkgfmt::SigningKey::generate(),
                db: Db::open_in_memory().unwrap(),
            }
        }

        fn settings(&self) -> CatalogSettings {
            CatalogSettings {
                enabled: true,
                channel: "stable".into(),
                mirrors: vec![MIRROR.into()],
                public_key: Some(self.catalog_key.public_hex.clone()),
                allow_unsigned: false,
                timeout: std::time::Duration::from_secs(1),
                extra_roots: Vec::new(),
            }
        }

        fn service(&self) -> Arc<CatalogService> {
            CatalogService::with_transport(
                self.settings(),
                self.dir.path().join("catalog"),
                self.fake.clone(),
            )
        }

        fn request(&self) -> StoreInstallRequest {
            StoreInstallRequest {
                plugin_id: PLUGIN_ID.into(),
                form: skiff_model::Form::Native,
                allow_unsigned: false,
                allow_reduced_sandbox: true,
                data_dir: self.dir.path().join("data"),
            }
        }

        fn incoming(&self) -> PathBuf {
            self.dir
                .path()
                .join("data")
                .join("plugins")
                .join(".incoming")
        }

        fn incoming_files(&self) -> Vec<String> {
            std::fs::read_dir(self.incoming())
                .map(|it| {
                    it.flatten()
                        .map(|e| e.file_name().to_string_lossy().to_string())
                        .collect()
                })
                .unwrap_or_default()
        }

        /// 打包一个真实 `.wbpkg`，返回（字节、sha256）。
        fn package(&self, requires: &[&str], api: &str) -> (Vec<u8>, String) {
            let src = self.dir.path().join("pkg-src");
            let _ = std::fs::remove_dir_all(&src);
            let arch = skiff_pkgfmt::host_arch();
            std::fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
            std::fs::write(src.join("bin").join(&arch).join("plugin"), b"test-binary").unwrap();
            let capability = requires.first().copied().unwrap_or("docker");
            let requires = requires
                .iter()
                .map(|r| format!("\"{r}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let manifest = format!(
                r#"id = "{PLUGIN_ID}"
name = "Demo"
version = "0.1.0"
key_id = "{}"
api = "{api}"
requires_full_sandbox = false
requires = [{requires}]

[[permissions]]
capability = "{capability}"
verbs = ["list"]
"#,
                self.plugin_key.key_id()
            );
            std::fs::write(src.join("manifest.toml"), manifest).unwrap();
            let out = self.dir.path().join("demo.wbpkg");
            skiff_pkgfmt::package::pack_dir(&src, &out, &self.plugin_key.seed_hex()).unwrap();
            let bytes = std::fs::read(&out).unwrap();
            let sha = hex::encode(Sha256::digest(&bytes));
            (bytes, sha)
        }

        /// 让假传输层返回一份签名正确、条目指向 `blob` 的快照。
        fn serve(&self, plugin: &str, api: &str, requires: &[&str], sha: &str, size: u64) {
            self.serve_index(serde_json::json!({
                "schema": INDEX_SCHEMA,
                "channel": "stable",
                "seq": 1,
                "generated_at": "2026-11-10T00:00:00Z",
                "plugins": [{
                    "id": plugin,
                    "version": "0.1.0",
                    "api": api,
                    "requires": requires,
                    "summary": "示例插件",
                    "published_at": "2026-11-09T00:00:00Z",
                    "yanked": false,
                    "pkg_sha256": sha,
                    "pkg_url": format!("blobs/sha256/{sha}"),
                    "size": size
                }]
            }));
        }

        /// 自定义索引内容：撤包/吊销测试要改 `yanked` 或追加 `revoked[]`。
        fn serve_index(&self, index: serde_json::Value) {
            let seq = index.get("seq").and_then(|v| v.as_u64()).unwrap_or(1);
            let index = serde_json::to_vec(&index).unwrap();
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schema": MANIFEST_SCHEMA,
                "channel": "stable",
                "seq": seq,
                "generated_at": "2026-11-10T00:00:00Z",
                "key_id": self.catalog_key.key_id(),
                "index": {
                    "path": "index.json",
                    "sha256": hex::encode(Sha256::digest(&index)),
                    "size": index.len()
                }
            }))
            .unwrap();
            let sig = serde_json::to_vec(&serde_json::json!({
                "alg": "ed25519",
                "key_id": self.catalog_key.key_id(),
                "sig": sign_hello(&self.catalog_key, &manifest)
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
        }

        fn serve_blob(&self, sha: &str, body: Vec<u8>) {
            self.fake.push(
                &format!("{MIRROR}/blobs/sha256/{sha}"),
                HttpReply {
                    status: 200,
                    body,
                    etag: None,
                    last_modified: None,
                },
            );
        }

        fn trust_plugin_key(&self) {
            self.db
                .trust_key(
                    &self.plugin_key.key_id(),
                    &self.plugin_key.public_hex(),
                    "测试作者",
                )
                .unwrap();
        }
    }

    #[test]
    fn full_chain_downloads_verifies_and_installs() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        fx.serve_blob(&sha, bytes.clone());
        fx.trust_plugin_key();
        let svc = fx.service();

        let prepared = prepare(&svc, &fx.db, &fx.request()).expect("整链路应成功");
        assert_eq!(prepared.token.len(), 24);
        assert!(prepared.path.is_file());
        assert_eq!(std::fs::read(&prepared.path).unwrap(), bytes);
        assert!(prepared.trust.signature_verified);
        assert_eq!(prepared.channel, "stable");
        assert_eq!(prepared.source, MIRROR);

        // 复用现有安装收尾：能力面/沙箱/版本/授权/落库都在 install_package 内。
        let opts = skiff_plugin::install_options(
            &prepared.trust,
            prepared.trust.meta.manifest.permission_strings(),
            Vec::new(),
            false,
            skiff_model::Form::Native,
            false,
            "tester",
        );
        let info =
            skiff_plugin::install_package(&fx.db, &fx.request().data_dir, &prepared.path, &opts)
                .expect("安装应成功");
        assert_eq!(info.id, PLUGIN_ID);
        assert!(fx.db.plugin(PLUGIN_ID).unwrap().is_some());
        let _ = fs::remove_file(&prepared.path);
    }

    #[test]
    fn download_failure_leaves_no_staged_file() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        // 不排 blob 响应：假传输层直接报错。
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Download(_)), "{err}");
        assert_eq!(err.reason(), "download");
        assert!(fx.incoming_files().is_empty(), "失败不得留下暂存文件");
    }

    #[test]
    fn digest_mismatch_leaves_no_staged_file() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        // 大小相同、内容不同：必须按 sha256 拒绝。
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        fx.serve_blob(&sha, tampered);
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Digest(_)), "{err}");
        assert_eq!(err.reason(), "digest");
        assert!(fx.incoming_files().is_empty(), "失败不得留下暂存文件");
    }

    #[test]
    fn signature_failure_leaves_no_staged_file() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        fx.serve_blob(&sha, bytes.clone());
        // 不导入公钥且不允许未签名：验签阶段必须拒。
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Signature(_)), "{err}");
        assert_eq!(err.reason(), "signature");
        assert!(fx.incoming_files().is_empty(), "失败不得留下暂存文件");
    }

    #[test]
    fn capability_missing_rejects_before_download() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["ftp"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["ftp"], &sha, bytes.len() as u64);
        fx.serve_blob(&sha, bytes.clone());
        fx.trust_plugin_key();
        let svc = fx.service();

        let mut req = fx.request();
        req.form = skiff_model::Form::DockerRestricted;
        let err = prepare(&svc, &fx.db, &req).unwrap_err();
        assert!(matches!(err, StoreInstallError::Compatibility(_)), "{err}");
        assert!(err.message().contains("ftp"), "{err}");
        assert!(fx.incoming_files().is_empty(), "兼容性拒绝不得下载任何东西");
    }

    #[test]
    fn api_mismatch_is_rejected() {
        let fx = Fixture::new();
        // 包本身合法（api 1.0）；目录条目声明更高的内核 API，安装前必须拒绝。
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "2.0", &["docker"], &sha, bytes.len() as u64);
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Compatibility(_)), "{err}");
        assert!(err.message().contains("API"), "{err}");
        assert!(fx.incoming_files().is_empty());
    }

    #[test]
    fn missing_key_never_reaches_the_network() {
        let fx = Fixture::new();
        let mut settings = fx.settings();
        settings.public_key = None;
        let svc = CatalogService::with_transport(
            settings,
            fx.dir.path().join("catalog"),
            fx.fake.clone(),
        );
        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::KeyMissing), "{err}");
        assert!(fx.fake.calls().is_empty(), "未配置公钥时不得发起任何请求");
    }

    #[test]
    fn unknown_plugin_is_not_found() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(
            "com.example.other",
            "1.0",
            &["docker"],
            &sha,
            bytes.len() as u64,
        );
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::NotFound(_)), "{err}");
        assert!(fx.incoming_files().is_empty());
    }

    #[test]
    fn api_compatibility_matrix() {
        assert!(api_compatible("1.0", "1.2"));
        assert!(api_compatible("1.2", "1.2"));
        assert!(!api_compatible("1.3", "1.2"));
        assert!(!api_compatible("2.0", "1.2"));
        assert!(!api_compatible("abc", "1.2"));
    }

    #[test]
    fn incoming_ttl_sweeps_only_expired_staged_files() {
        let fx = Fixture::new();
        let dir = fx.incoming();
        std::fs::create_dir_all(&dir).unwrap();
        let staged = dir.join("abandoned.wbpkg");
        let part = dir.join("downloading.part");
        let other = dir.join("notes.txt");
        for f in [&staged, &part, &other] {
            std::fs::write(f, b"x").unwrap();
        }

        // 23 小时：未到 TTL，一个都不删。
        let now = SystemTime::now();
        assert_eq!(
            cleanup_stale_incoming_at(&dir, now + Duration::from_secs(23 * 3600), INCOMING_TTL),
            0
        );
        assert!(staged.is_file());

        // 25 小时：只删暂存物，目录里的其它文件不动。
        assert_eq!(
            cleanup_stale_incoming_at(&dir, now + Duration::from_secs(25 * 3600), INCOMING_TTL),
            2
        );
        assert!(!staged.exists());
        assert!(!part.exists());
        assert!(other.is_file(), "清理不得动目录里的非暂存文件");
    }

    #[test]
    fn prepare_sweeps_expired_staged_packages() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        fx.serve_blob(&sha, bytes.clone());
        fx.trust_plugin_key();

        // 模拟「放弃权限确认页」遗留的暂存包：mtime 拨回 25 小时前。
        std::fs::create_dir_all(fx.incoming()).unwrap();
        let stale = fx.incoming().join("abandoned.wbpkg");
        std::fs::write(&stale, b"old").unwrap();
        let old = SystemTime::now() - Duration::from_secs(25 * 3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();

        let svc = fx.service();
        let prepared = prepare(&svc, &fx.db, &fx.request()).expect("准备安装应成功");
        assert!(!stale.exists(), "超过 24h 的暂存包必须在准备安装前被清理");
        assert!(prepared.path.is_file(), "本次刚下载的暂存包不得被误删");
        let _ = fs::remove_file(&prepared.path);
    }

    /// 只改撤包字段的索引：用于验证 R1 新字段不改变旧安装行为。
    fn takedown_index(
        sha: &str,
        size: u64,
        yanked: bool,
        yank_reason: Option<&str>,
        revoked_reason: Option<&str>,
    ) -> serde_json::Value {
        let mut plugin = serde_json::json!({
            "id": PLUGIN_ID,
            "version": "0.1.0",
            "api": "1.0",
            "requires": ["docker"],
            "summary": "示例插件",
            "published_at": "2026-11-09T00:00:00Z",
            "yanked": yanked,
            "pkg_sha256": sha,
            "pkg_url": format!("blobs/sha256/{sha}"),
            "size": size
        });
        if let Some(reason) = yank_reason {
            plugin["yank_reason"] = serde_json::json!(reason);
        }
        let mut value = serde_json::json!({
            "schema": INDEX_SCHEMA,
            "channel": "stable",
            "seq": 2,
            "generated_at": "2026-11-10T00:00:00Z",
            "plugins": [plugin]
        });
        if let Some(reason) = revoked_reason {
            value["revoked"] = serde_json::json!([{
                "id": PLUGIN_ID,
                "version": "0.1.0",
                "reason": reason,
                "at": "2026-11-11T00:00:00Z"
            }]);
        }
        value
    }

    #[test]
    fn pre_r1_index_without_revoked_field_installs_unchanged() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        // `serve` 产出的索引没有 `revoked` 字段（R1 之前的形状），yanked=false。
        fx.serve(PLUGIN_ID, "1.0", &["docker"], &sha, bytes.len() as u64);
        fx.serve_blob(&sha, bytes.clone());
        fx.trust_plugin_key();
        let svc = fx.service();

        let prepared = prepare(&svc, &fx.db, &fx.request()).expect("旧索引必须与现在一样可安装");
        assert!(prepared.path.is_file());
        assert!(svc.cached_takedown(PLUGIN_ID, "0.1.0").is_none());
        let _ = fs::remove_file(&prepared.path);
    }

    #[test]
    fn yanked_entry_is_rejected_with_reason_not_parse_failure() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve_index(takedown_index(
            &sha,
            bytes.len() as u64,
            true,
            Some("policy violation"),
            None,
        ));
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Revoked(_)), "{err}");
        assert!(err.message().contains("已被服务端撤回"), "{err}");
        assert!(
            err.message().contains("policy violation"),
            "安装被拒文案必须带撤包原因: {err}"
        );
        assert!(svc
            .cached_takedown(PLUGIN_ID, "0.1.0")
            .unwrap()
            .contains("policy violation"));
        assert!(fx.incoming_files().is_empty(), "撤包拒绝不得留下暂存文件");
    }

    #[test]
    fn revoked_entry_is_rejected_with_catalog_reason() {
        let fx = Fixture::new();
        let (bytes, sha) = fx.package(&["docker"], "1.0");
        fx.serve_index(takedown_index(
            &sha,
            bytes.len() as u64,
            false,
            None,
            Some("private key leaked"),
        ));
        fx.trust_plugin_key();
        let svc = fx.service();

        let err = prepare(&svc, &fx.db, &fx.request()).unwrap_err();
        assert!(matches!(err, StoreInstallError::Revoked(_)), "{err}");
        assert!(err.message().contains("private key leaked"), "{err}");
        assert!(svc
            .cached_takedown(PLUGIN_ID, "0.1.0")
            .unwrap()
            .contains("private key leaked"));
        assert!(fx.incoming_files().is_empty());
    }
}
