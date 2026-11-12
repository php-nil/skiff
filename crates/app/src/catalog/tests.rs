//! 目录客户端单测：全部走假传输层，不触网。

use std::sync::Arc;

use sha2::{Digest, Sha256};
use skiff_store::Db;

use super::fetch::testing::FakeTransport;
use super::fetch::{Conditional, HttpReply, Transport};
use super::model::{INDEX_SCHEMA, MANIFEST_SCHEMA};
use super::verify::testing::{sign_hello, test_key, TestKey};
use super::*;

const MIRROR: &str = "https://mirror.example/skiff";
const BLOB_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// 断言最近审计里有该原因的 `catalog.refresh` 失败。
fn assert_refresh_audit(db: &Db, reason: &str) {
    let audit = db.recent_audit(20).unwrap();
    assert!(
        audit.iter().any(|e| e.action == "catalog.refresh"
            && e.result == "fail"
            && e.detail.starts_with(&format!("reason={reason} "))),
        "缺少 reason={reason} 的目录审计: {audit:?}"
    );
}

fn manifest_url() -> String {
    format!("{MIRROR}/channels/stable/manifest.json")
}

fn sig_url() -> String {
    format!("{MIRROR}/channels/stable/manifest.json.sig")
}

fn index_url() -> String {
    format!("{MIRROR}/channels/stable/index.json")
}

fn index_json(seq: u64, plugin: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": INDEX_SCHEMA,
        "channel": "stable",
        "seq": seq,
        "generated_at": "2026-11-10T00:00:00Z",
        "plugins": [{
            "id": plugin,
            "version": "1.0.0",
            "api": "1.0",
            "requires": ["docker"],
            "summary": "示例插件",
            "published_at": "2026-11-09T00:00:00Z",
            "yanked": false,
            "pkg_sha256": BLOB_SHA,
            "pkg_url": format!("blobs/sha256/{BLOB_SHA}"),
            "size": 3
        }],
        "apps": [{
            "id": "com.example.blog",
            "version": "2.1.0",
            "summary": "示例应用",
            "min_kernel": "0.1.0",
            "compose_sha256": BLOB_SHA,
            "compose_url": format!("blobs/sha256/{BLOB_SHA}"),
            "size": 3
        }],
        "runtimes": [{
            "kind": "php",
            "version": "8.3",
            "image": "skiff/php:8.3",
            "image_digest": format!("sha256:{BLOB_SHA}"),
            "eol": null,
            "api_min": "1.0",
            "default": true
        }],
        "upgrades": [{
            "version": "0.3.0",
            "arch": "x86_64",
            "channel": "stable",
            "min_from": "0.1.0",
            "changelog_url": "https://example.invalid/changelog",
            "pkg_sha256": BLOB_SHA,
            "pkg_url": format!("blobs/sha256/{BLOB_SHA}"),
            "size": 3
        }],
        "revoked": []
    }))
    .unwrap()
}

fn manifest_json(channel: &str, seq: u64, key_id: &str, index: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema": MANIFEST_SCHEMA,
        "channel": channel,
        "seq": seq,
        "generated_at": "2026-11-10T00:00:00Z",
        "key_id": key_id,
        "index": {
            "path": "index.json",
            "sha256": hex::encode(Sha256::digest(index)),
            "size": index.len()
        }
    }))
    .unwrap()
}

fn signature_json(key_id: &str, sig: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "alg": "ed25519",
        "key_id": key_id,
        "sig": sig
    }))
    .unwrap()
}

fn reply(status: u16, body: Vec<u8>, etag: Option<&str>) -> HttpReply {
    HttpReply {
        status,
        body,
        etag: etag.map(str::to_string),
        last_modified: None,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    fake: Arc<FakeTransport>,
    key: TestKey,
}

/// 模拟「连接被静默丢包」：TCP 能通但永不回包，请求要等满剩余预算才超时失败。
struct HangingTransport {
    calls: std::sync::atomic::AtomicUsize,
}

impl HangingTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Transport for HangingTransport {
    fn get(&self, _url: &str, _cond: &Conditional<'_>) -> Result<HttpReply, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("连接超时（模拟静默丢包）".into())
    }

    fn get_within(
        &self,
        _url: &str,
        _cond: &Conditional<'_>,
        budget: Duration,
    ) -> Result<HttpReply, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(budget);
        Err("连接超时（模拟静默丢包）".into())
    }
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            fake: FakeTransport::new(),
            key: test_key(7),
        }
    }

    fn settings(&self) -> CatalogSettings {
        CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec![MIRROR.into()],
            public_key: Some(self.key.public_hex.clone()),
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        }
    }

    fn service(&self) -> Arc<CatalogService> {
        self.service_with(self.fake.clone(), self.settings())
    }

    fn service_with(
        &self,
        fake: Arc<FakeTransport>,
        settings: CatalogSettings,
    ) -> Arc<CatalogService> {
        CatalogService::with_transport(settings, self.dir.path().join("catalog"), fake)
    }

    /// 让假传输层返回一份签名正确的快照。
    fn serve_signed(&self, seq: u64, plugin: &str, etag_m: &str, etag_i: &str) {
        let index = index_json(seq, plugin);
        self.serve_index(seq, index, etag_m, etag_i);
    }

    /// 自定义索引的签名快照：升级条目测试要自己拼 `upgrades`。
    fn serve_index(&self, seq: u64, index: Vec<u8>, etag_m: &str, etag_i: &str) {
        let manifest = manifest_json("stable", seq, &self.key.key_id(), &index);
        let sig = signature_json(&self.key.key_id(), &sign_hello(&self.key, &manifest));
        self.fake
            .push(&manifest_url(), reply(200, manifest, Some(etag_m)));
        self.fake.push(&sig_url(), reply(200, sig, None));
        self.fake
            .push(&index_url(), reply(200, index, Some(etag_i)));
    }
}

#[test]
fn disabled_service_never_calls_transport() {
    let fx = Fixture::new();
    let mut settings = fx.settings();
    settings.enabled = false;
    let svc = fx.service_with(fx.fake.clone(), settings);
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Disabled);
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn fetch_verifies_and_caches_snapshot() {
    let fx = Fixture::new();
    fx.serve_signed(3, "com.example.one", "etag-m", "etag-i");
    let snap = fx.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.seq, Some(3));
    assert_eq!(snap.entries.plugins.len(), 1);
    assert_eq!(snap.entries.apps.len(), 1);
    assert_eq!(snap.entries.runtimes.len(), 1);
    assert_eq!(snap.entries.upgrades.len(), 1);
    // 快照确实落盘：新服务实例（无网络）也能读到
    let offline = Fixture {
        dir: fx.dir,
        fake: FakeTransport::new(),
        key: test_key(7),
    };
    let snap = offline.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Offline);
    assert_eq!(snap.entries.plugins.len(), 1);
}

#[test]
fn conditional_304_reuses_cached_bytes() {
    let fx = Fixture::new();
    fx.serve_signed(1, "com.example.one", "etag-m", "etag-i");
    let svc = fx.service();
    assert_eq!(svc.snapshot(true).state, CatalogState::Fresh);

    fx.fake
        .push(&manifest_url(), reply(304, Vec::new(), Some("etag-m")));
    fx.fake
        .push(&index_url(), reply(304, Vec::new(), Some("etag-i")));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Cached, "{}", snap.message);
    assert!(snap.message.contains("304"), "{}", snap.message);
    assert_eq!(snap.entries.plugins.len(), 1);

    let calls = fx.fake.calls();
    let manifest_calls: Vec<_> = calls
        .iter()
        .filter(|(url, _)| url.ends_with("/manifest.json"))
        .collect();
    assert_eq!(manifest_calls.len(), 2);
    assert_eq!(manifest_calls[0].1.etag, None);
    assert_eq!(manifest_calls[1].1.etag.as_deref(), Some("etag-m"));
    let index_calls: Vec<_> = calls
        .iter()
        .filter(|(url, _)| url.ends_with("/index.json"))
        .collect();
    assert_eq!(index_calls[1].1.etag.as_deref(), Some("etag-i"));
}

#[test]
fn tampered_signature_is_rejected_and_old_snapshot_kept() {
    let fx = Fixture::new();
    fx.serve_signed(1, "com.example.one", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    // 同一把公钥、清单内容被换掉但签名仍来自旧内容 → 验签失败。
    let index = index_json(2, "com.example.two");
    let manifest = manifest_json("stable", 2, &fx.key.key_id(), &index);
    let sig = signature_json(&fx.key.key_id(), &sign_hello(&test_key(9), &manifest));
    fx.fake
        .push(&manifest_url(), reply(200, manifest, Some("etag-m2")));
    fx.fake.push(&sig_url(), reply(200, sig, None));
    fx.fake
        .push(&index_url(), reply(200, index, Some("etag-i2")));

    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = fx.service();
    svc.set_audit_db(Arc::clone(&db));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("校验失败"), "{}", snap.message);
    // 旧快照（seq 1）仍可展示
    assert_eq!(snap.seq, Some(1));
    assert_eq!(snap.entries.plugins[0].name, "com.example.one");
    assert_refresh_audit(&db, "signature");
}

#[test]
fn seq_rollback_is_rejected() {
    let fx = Fixture::new();
    fx.serve_signed(5, "com.example.five", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    fx.serve_signed(4, "com.example.four", "etag-m2", "etag-i2");
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = fx.service();
    svc.set_audit_db(Arc::clone(&db));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("防回滚"), "{}", snap.message);
    assert_eq!(snap.seq, Some(5));
    assert_eq!(snap.entries.plugins[0].name, "com.example.five");
    assert_refresh_audit(&db, "seq");
}

#[test]
fn index_digest_mismatch_is_rejected() {
    let fx = Fixture::new();
    let index = index_json(1, "com.example.one");
    let manifest = manifest_json("stable", 1, &fx.key.key_id(), &index);
    let sig = signature_json(&fx.key.key_id(), &sign_hello(&fx.key, &manifest));
    // 同长度、不同内容的索引：大小校验通过，摘要必须失败。
    let mut tampered = index.clone();
    let pos = tampered
        .windows(b"com.example.one".len())
        .position(|w| w == b"com.example.one")
        .unwrap();
    tampered[pos..pos + "com.example.one".len()].copy_from_slice(b"com.example.two");
    assert_eq!(tampered.len(), index.len());

    fx.fake
        .push(&manifest_url(), reply(200, manifest, Some("e")));
    fx.fake.push(&sig_url(), reply(200, sig, None));
    fx.fake.push(&index_url(), reply(200, tampered, Some("e")));
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = fx.service();
    svc.set_audit_db(Arc::clone(&db));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("摘要"), "{}", snap.message);
    assert_refresh_audit(&db, "digest");
}

#[test]
fn unsigned_channel_rejected_unless_explicitly_allowed() {
    let fx = Fixture::new();
    let index = index_json(1, "com.example.one");
    let manifest = manifest_json("stable", 1, &fx.key.key_id(), &index);
    fx.fake
        .push(&manifest_url(), reply(200, manifest, Some("e")));
    fx.fake.push(&sig_url(), reply(404, Vec::new(), None));
    fx.fake.push(&index_url(), reply(200, index, Some("e")));
    let snap = fx.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("未签名"), "{}", snap.message);
}

#[test]
fn unsigned_allowed_only_when_configured() {
    let fx = Fixture::new();
    let index = index_json(1, "com.example.one");
    let manifest = manifest_json("stable", 1, &"0".repeat(64), &index);
    fx.fake
        .push(&manifest_url(), reply(200, manifest, Some("e")));
    fx.fake.push(&sig_url(), reply(404, Vec::new(), None));
    fx.fake.push(&index_url(), reply(200, index, Some("e")));

    let mut settings = fx.settings();
    settings.allow_unsigned = true;
    let snap = fx.service_with(fx.fake.clone(), settings).snapshot(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert!(snap.allow_unsigned);
}

#[test]
fn offline_falls_back_to_last_successful_snapshot() {
    let fx = Fixture::new();
    fx.serve_signed(2, "com.example.two", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = fx.service_with(FakeTransport::new(), fx.settings());
    svc.set_audit_db(Arc::clone(&db));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Offline);
    assert!(snap.message.contains("镜像源不可达"), "{}", snap.message);
    assert!(snap.message.contains("本地缓存"), "{}", snap.message);
    assert_eq!(snap.seq, Some(2));
    assert_eq!(snap.entries.plugins[0].name, "com.example.two");
    assert_refresh_audit(&db, "offline");
}

/// 默认源不可达且没有任何缓存：文案必须给出「去设置里填镜像地址」的下一步，而不是裸错误串。
#[test]
fn unreachable_mirror_without_cache_explains_next_step() {
    let fx = Fixture::new();
    let snap = fx.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Offline);
    assert!(
        snap.message.contains("尚未连上任何镜像源"),
        "{}",
        snap.message
    );
    assert!(snap.message.contains("设置"), "{}", snap.message);
    assert!(snap.entries.plugins.is_empty());
}

/// 静默丢包（连得上但无响应）：抓取必须在总预算内失败，走离线缓存路径并进入冷却。
#[test]
fn silent_drop_uses_cache_and_enters_cooldown() {
    let fx = Fixture::new();
    fx.serve_signed(2, "com.example.one", "e-m", "e-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    let hang = HangingTransport::new();
    let svc = CatalogService::with_transport_budget(
        fx.settings(),
        fx.dir.path().join("catalog"),
        hang.clone(),
        Duration::from_millis(120),
    );
    let started = Instant::now();
    let snap = svc.snapshot(true);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "抓取必须受总预算约束"
    );
    assert_eq!(snap.state, CatalogState::Offline, "{}", snap.message);
    assert!(snap.message.contains("镜像源不可达"), "{}", snap.message);
    assert!(snap.message.contains("本地缓存"), "{}", snap.message);
    assert_eq!(snap.entries.plugins.len(), 1, "应继续展示上次快照");
    assert_eq!(hang.calls(), 1);

    // 5 分钟冷却内：不再发请求。
    let snap = svc.snapshot(false);
    assert_eq!(snap.state, CatalogState::Offline);
    assert_eq!(hang.calls(), 1, "失败冷却期内不得重试");
}

/// 多个镜像共享同一个总预算：不能每个源各等一个超时。
#[test]
fn multiple_mirrors_share_one_fetch_budget() {
    let fx = Fixture::new();
    let hang = HangingTransport::new();
    let mut settings = fx.settings();
    settings.mirrors = (0..3)
        .map(|i| format!("https://m{i}.example/skiff"))
        .collect();
    let svc = CatalogService::with_transport_budget(
        settings,
        fx.dir.path().join("catalog"),
        hang.clone(),
        Duration::from_millis(150),
    );

    let started = Instant::now();
    let snap = svc.snapshot(true);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "总耗时应受共享预算约束"
    );
    assert_eq!(snap.state, CatalogState::Offline);
    assert_eq!(hang.calls(), 1, "预算耗尽后不应再逐个尝试剩余镜像");
    assert!(snap.message.contains("总预算"), "{}", snap.message);
}

#[test]
fn key_rotation_invalidates_cache_and_forces_refetch() {
    let fx = Fixture::new();
    fx.serve_signed(3, "com.example.one", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    // 换 `catalog.public_key`：同一缓存目录，新公钥 + 新签名快照。
    let rotated = Fixture {
        dir: fx.dir,
        fake: FakeTransport::new(),
        key: test_key(8),
    };
    rotated.serve_signed(4, "com.example.two", "etag-m2", "etag-i2");
    let snap = rotated.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.seq, Some(4));
    assert_eq!(snap.entries.plugins[0].name, "com.example.two");

    // 旧缓存已作废 → 请求不带条件头，忽略 ETag 强制重拉。
    let calls = rotated.fake.calls();
    let manifest_calls: Vec<_> = calls
        .iter()
        .filter(|(url, _)| url.ends_with("/manifest.json"))
        .collect();
    assert_eq!(manifest_calls.len(), 1);
    assert_eq!(manifest_calls[0].1.etag, None, "换钥后不得复用旧 ETag");
    let index_calls: Vec<_> = calls
        .iter()
        .filter(|(url, _)| url.ends_with("/index.json"))
        .collect();
    assert_eq!(index_calls.len(), 1);
    assert_eq!(index_calls[0].1.etag, None, "换钥后不得复用旧 ETag");
}

#[test]
fn key_rotation_hides_stale_cache_when_offline() {
    let fx = Fixture::new();
    fx.serve_signed(3, "com.example.one", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    // 换钥后离线：旧条目既不得展示，也不得作为安装来源。
    let rotated = Fixture {
        dir: fx.dir,
        fake: FakeTransport::new(),
        key: test_key(8),
    };
    let svc = rotated.service();
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Offline, "{}", snap.message);
    assert_eq!(snap.seq, None);
    assert_eq!(snap.entries.total(), 0);
    assert!(
        svc.plugin_package("com.example.one", None).is_none(),
        "换钥后旧缓存条目不得用于安装"
    );
}

#[test]
fn mirror_fallback_uses_second_address() {
    let fx = Fixture::new();
    let index = index_json(1, "com.example.one");
    let manifest = manifest_json("stable", 1, &fx.key.key_id(), &index);
    let sig = signature_json(&fx.key.key_id(), &sign_hello(&fx.key, &manifest));
    let second = "https://backup.example/skiff";
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json"),
        reply(200, manifest, Some("e")),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json.sig"),
        reply(200, sig, None),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/index.json"),
        reply(200, index, Some("e")),
    );

    let mut settings = fx.settings();
    settings.mirrors = vec![MIRROR.into(), second.into()];
    let snap = fx.service_with(fx.fake.clone(), settings).snapshot(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.source, second);
}

#[test]
fn auto_refresh_skips_mirror_in_backoff() {
    let fx = Fixture::new();
    let second = "https://backup.example/skiff";
    let index = index_json(1, "com.example.one");
    let manifest = manifest_json("stable", 1, &fx.key.key_id(), &index);
    let sig = signature_json(&fx.key.key_id(), &sign_hello(&fx.key, &manifest));
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json"),
        reply(200, manifest.clone(), Some("e-m")),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json.sig"),
        reply(200, sig.clone(), None),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/index.json"),
        reply(200, index.clone(), Some("e-i")),
    );

    let dead = "https://dead.example/skiff";
    let mut settings = fx.settings();
    settings.mirrors = vec![dead.into(), second.into()];
    let svc = fx.service_with(fx.fake.clone(), settings);

    // 手动刷新：第一个镜像不可用 → 记退避，落到第二个。
    let (snap, _, _) = svc.load(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.source, second);
    let after_first = fx.fake.calls().len();

    // 自动刷新：退避中的第一个镜像应被跳过，只请求第二个。
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json"),
        reply(200, manifest, Some("e-m")),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/manifest.json.sig"),
        reply(200, sig, None),
    );
    fx.fake.push(
        &format!("{second}/channels/stable/index.json"),
        reply(200, index, Some("e-i")),
    );
    let (snap, _, _) = svc.load(false);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.source, second);
    let calls = fx.fake.calls();
    assert_eq!(
        calls.len(),
        after_first + 3,
        "自动刷新应只请求健康的第二个镜像"
    );
    assert!(
        calls[after_first..]
            .iter()
            .all(|(url, _)| url.starts_with(second)),
        "退避期内不应再请求第一个镜像"
    );
}

#[test]
fn all_mirrors_cooling_uses_cache_until_force_retry() {
    let fx = Fixture::new();
    fx.serve_signed(2, "com.example.one", "e1", "e2");
    assert_eq!(fx.service().load(true).0.state, CatalogState::Fresh);

    // 换成一个假传输层没有响应的镜像：手动刷新失败 → 进入退避，展示缓存。
    let dead_base = "https://dead.example/skiff";
    let mut settings = fx.settings();
    settings.mirrors = vec![dead_base.into()];
    let svc = fx.service_with(fx.fake.clone(), settings);
    let (snap, _, _) = svc.load(true);
    assert_eq!(snap.state, CatalogState::Offline, "{}", snap.message);
    assert_eq!(snap.entries.plugins.len(), 1);
    let attempted = fx.fake.calls().len();

    // 自动刷新：唯一镜像仍在退避期 → 直接吃缓存，不再发请求。
    let (snap, _, _) = svc.load(false);
    assert_eq!(snap.state, CatalogState::Offline);
    assert!(snap.message.contains("退避"), "{}", snap.message);
    assert_eq!(fx.fake.calls().len(), attempted, "退避期内不应再请求镜像");

    // 手动刷新绕过退避；镜像恢复后成功并清空退避。
    let index = index_json(3, "com.example.two");
    let manifest = manifest_json("stable", 3, &fx.key.key_id(), &index);
    let sig = signature_json(&fx.key.key_id(), &sign_hello(&fx.key, &manifest));
    fx.fake.push(
        &format!("{dead_base}/channels/stable/manifest.json"),
        reply(200, manifest, Some("m3")),
    );
    fx.fake.push(
        &format!("{dead_base}/channels/stable/manifest.json.sig"),
        reply(200, sig, None),
    );
    fx.fake.push(
        &format!("{dead_base}/channels/stable/index.json"),
        reply(200, index, Some("i3")),
    );
    let (snap, _, _) = svc.load(true);
    assert_eq!(snap.state, CatalogState::Fresh, "{}", snap.message);
    assert_eq!(snap.seq, Some(3));
    assert_eq!(snap.source, dead_base);
}

#[test]
fn ttl_reuses_snapshot_and_force_bypasses() {
    let fx = Fixture::new();
    fx.serve_signed(1, "com.example.one", "e-m", "e-i");
    let svc = fx.service();
    assert_eq!(svc.snapshot(false).state, CatalogState::Fresh);
    let calls = fx.fake.calls().len();

    // TTL 内：直接复用内存快照，不联网。
    let snap = svc.snapshot(false);
    assert_eq!(snap.state, CatalogState::Fresh);
    assert_eq!(snap.seq, Some(1));
    assert_eq!(fx.fake.calls().len(), calls, "TTL 内不应重拉");

    // 强制刷新绕过 TTL：304 复用缓存，但请求确实发出去了。
    fx.fake.push(&manifest_url(), reply(304, Vec::new(), None));
    fx.fake.push(&index_url(), reply(304, Vec::new(), None));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Cached, "{}", snap.message);
    assert_eq!(fx.fake.calls().len(), calls + 2, "强制刷新必须真的发请求");
}

#[test]
fn failure_is_retried_after_failure_ttl_not_a_day() {
    let fx = Fixture::new();
    let dead = "https://dead.example/skiff";
    let mut settings = fx.settings();
    settings.mirrors = vec![dead.into()];
    let svc = fx.service_with(fx.fake.clone(), settings);

    assert_eq!(svc.snapshot(false).state, CatalogState::Offline);
    let calls = fx.fake.calls().len();
    assert_eq!(svc.snapshot(false).state, CatalogState::Offline);
    assert_eq!(fx.fake.calls().len(), calls, "失败冷却期内不应重试");

    // 把失败时间往前拨过 5 分钟：应重新尝试，而不是等满一天。
    if let Ok(mut memo) = svc.memo.lock() {
        let m = memo.as_mut().expect("失败也应写入 memo");
        m.at = Instant::now() - FAILURE_TTL - Duration::from_secs(1);
    }
    assert_eq!(svc.snapshot(false).state, CatalogState::Offline);
    assert!(
        fx.fake.calls().len() > calls,
        "失败冷却到期后应重新请求镜像"
    );
}

#[test]
fn restarted_process_reuses_fresh_disk_cache_without_network() {
    let fx = Fixture::new();
    fx.serve_signed(1, "com.example.one", "e-m", "e-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);
    let calls = fx.fake.calls().len();

    // 新实例 = 进程重启，内存 memo 为空；磁盘 fetched_at 未过一天。
    let svc = fx.service();
    let snap = svc.snapshot(false);
    assert_eq!(snap.state, CatalogState::Cached, "{}", snap.message);
    assert_eq!(snap.seq, Some(1));
    assert_eq!(fx.fake.calls().len(), calls, "未过一天的磁盘缓存不应联网");

    // 强制刷新同样绕过磁盘 TTL。
    fx.fake.push(&manifest_url(), reply(304, Vec::new(), None));
    fx.fake.push(&index_url(), reply(304, Vec::new(), None));
    let snap = svc.snapshot(true);
    assert_eq!(snap.state, CatalogState::Cached, "{}", snap.message);
    assert!(fx.fake.calls().len() > calls);
}

/// 升级条目索引：只关心 `(version, arch, channel, min_from)`，摘要三件套共用占位。
fn upgrades_index(entries: &[(&str, &str, &str, &str)]) -> Vec<u8> {
    let upgrades: Vec<_> = entries
        .iter()
        .map(|(version, arch, channel, min_from)| {
            serde_json::json!({
                "version": version,
                "arch": arch,
                "channel": channel,
                "min_from": min_from,
                "changelog_url": "https://example.invalid/changelog",
                "pkg_sha256": BLOB_SHA,
                "pkg_url": format!("blobs/sha256/{BLOB_SHA}"),
                "size": 3
            })
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "schema": INDEX_SCHEMA,
        "channel": "stable",
        "seq": 1,
        "generated_at": "2026-11-10T00:00:00Z",
        "upgrades": upgrades
    }))
    .unwrap()
}

#[test]
fn latest_upgrade_is_quiet_when_channel_has_no_upgrade_data() {
    let fx = Fixture::new();
    fx.serve_index(1, upgrades_index(&[]), "e-m", "e-i");
    let svc = fx.service();
    assert!(svc.latest_upgrade("0.1.0", "x86_64").is_none());
    assert!(svc.upgrade_package("0.2.0", "x86_64").is_none());
}

#[test]
fn latest_upgrade_picks_highest_matching_channel_and_arch() {
    let fx = Fixture::new();
    fx.serve_index(
        1,
        upgrades_index(&[
            ("0.2.0", "x86_64", "stable", "0.1.0"),
            ("0.4.0", "aarch64", "stable", "0.1.0"),
            ("0.3.0", "x86_64", "night", "0.1.0"),
            ("0.1.5", "x86_64", "stable", "0.1.0"),
        ]),
        "e-m",
        "e-i",
    );
    let svc = fx.service();
    let latest = svc.latest_upgrade("0.1.0", "x86_64").expect("应有可用升级");
    assert_eq!(latest.entry.version, "0.2.0");
    assert_eq!(latest.channel, "stable");
    assert_eq!(latest.base, MIRROR);

    // 指定版本查找同样受渠道与架构约束。
    assert!(svc.upgrade_package("0.2.0", "x86_64").is_some());
    assert!(svc.upgrade_package("0.2.0", "aarch64").is_none());
    assert!(svc.upgrade_package("0.3.0", "x86_64").is_none());
}

#[test]
fn latest_upgrade_is_none_when_current_is_not_older() {
    let fx = Fixture::new();
    fx.serve_index(
        1,
        upgrades_index(&[("0.2.0", "x86_64", "stable", "0.1.0")]),
        "e-m",
        "e-i",
    );
    let svc = fx.service();
    assert!(svc.latest_upgrade("0.2.0", "x86_64").is_none());
    assert!(svc.latest_upgrade("0.9.0", "x86_64").is_none());
}

/// R1 之前生成的旧索引（seq=2 的形状：`yanked` 为 false、`revoked` 为空）。
/// 这是客户端在 R1 之前见到的真实产物形态；解析器必须原样接受。
const LEGACY_INDEX: &str = r#"{
  "schema": "skiff.server.index/v1",
  "channel": "stable",
  "seq": 2,
  "generated_at": "2026-11-10T08:00:00Z",
  "plugins": [
    {
      "id": "com.example.demo",
      "version": "1.0.0",
      "api": "1.0",
      "requires": ["docker"],
      "summary": "示例插件",
      "published_at": "2026-11-09T00:00:00Z",
      "yanked": false,
      "pkg_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "pkg_url": "blobs/sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "size": 3
    }
  ],
  "apps": [],
  "runtimes": [],
  "upgrades": [],
  "revoked": []
}"#;

#[test]
fn legacy_index_without_takedown_marks_parses_unchanged() {
    let index: Index = serde_json::from_str(LEGACY_INDEX).expect("旧索引必须能解析");
    index.validate(None).expect("旧索引必须通过校验");
    assert_eq!(index.seq, 2);
    assert!(!index.plugins[0].yanked);
    assert!(index.revoked.is_empty());
    assert!(index.revoked("com.example.demo", "1.0.0").is_none());
    assert!(index.plugins[0].yank_reason.is_none());
    assert!(takedown_note(&index, "com.example.demo", "1.0.0").is_none());
    assert!(entries_of(&index).plugins[0].warn.is_empty());
}

#[test]
fn new_index_with_takedown_fields_parses_and_reports_reason() {
    let mut value: serde_json::Value = serde_json::from_str(LEGACY_INDEX).unwrap();
    value["plugins"][0]["yanked"] = serde_json::json!(true);
    value["revoked"] = serde_json::json!([{
        "id": "com.example.demo",
        "version": "1.0.0",
        "reason": "private key leaked",
        "at": "2026-11-11T00:00:00Z"
    }]);
    let index: Index = serde_json::from_value(value).expect("新索引必须能解析而不是解析失败");
    index.validate(None).expect("新索引仍符合 v1 校验");
    assert!(index.plugins[0].yanked);
    assert_eq!(
        index.revoked("com.example.demo", "1.0.0").unwrap().reason,
        "private key leaked"
    );
    assert!(takedown_note(&index, "com.example.demo", "1.0.0")
        .unwrap()
        .contains("private key leaked"));
    assert!(entries_of(&index).plugins[0].warn.contains("已撤回"));
}

#[test]
fn app_and_runtime_takedown_use_their_own_namespaces() {
    let mut value: serde_json::Value = serde_json::from_str(LEGACY_INDEX).unwrap();
    value["apps"] = serde_json::json!([{
        "id": "com.example.blog",
        "version": "1.0.0",
        "summary": "示例应用",
        "min_kernel": "1.2.0",
        "compose_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "compose_url": "blobs/sha256/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "size": 10,
        "yanked": true,
        "yank_reason": "模板损坏"
    }]);
    value["runtimes"] = serde_json::json!([{
        "kind": "php",
        "version": "8.5",
        "image": "php:8.5-fpm-alpine",
        "image_digest": format!("sha256:{}", "c".repeat(64)),
        "eol": null,
        "api_min": "1.0",
        "default": false,
        "yanked": true,
        "yank_reason": "基础镜像 CVE"
    }]);
    value["revoked_apps"] = serde_json::json!([{
        "id": "com.example.blog",
        "version": "1.0.0",
        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "reason": "compose 篡改",
        "at": "2026-11-12T00:00:00Z"
    }]);
    value["revoked_runtimes"] = serde_json::json!([{
        "id": "php",
        "version": "8.5",
        "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "reason": "镜像 CVE-2026-0001",
        "at": "2026-11-12T00:00:00Z"
    }]);

    let index: Index = serde_json::from_value(value).expect("R2e 索引必须能解析");
    index.validate(None).expect("R2e 索引仍符合 v1 校验");
    assert!(index.revoked.is_empty(), "revoked[] 只表示插件");

    let entries = entries_of(&index);
    assert!(
        entries.apps[0].warn.contains("compose 篡改"),
        "{:?}",
        entries.apps[0]
    );
    assert!(
        entries.runtimes[0].warn.contains("CVE-2026-0001"),
        "{:?}",
        entries.runtimes[0]
    );

    // 只有 yanked 没有吊销命中时，退回 yank_reason。
    let mut only_yanked: serde_json::Value = serde_json::from_str(LEGACY_INDEX).unwrap();
    only_yanked["apps"] = serde_json::json!([{
        "id": "com.example.blog",
        "version": "1.0.0",
        "summary": "示例应用",
        "min_kernel": "1.2.0",
        "compose_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "compose_url": "blobs/sha256/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "size": 10,
        "yanked": true,
        "yank_reason": "模板损坏"
    }]);
    let index: Index = serde_json::from_value(only_yanked).unwrap();
    assert!(entries_of(&index).apps[0].warn.contains("模板损坏"));
}

#[test]
fn yanked_reason_is_reported_by_the_client_view() {
    let mut value: serde_json::Value = serde_json::from_str(LEGACY_INDEX).unwrap();
    value["plugins"][0]["yanked"] = serde_json::json!(true);
    value["plugins"][0]["yank_reason"] = serde_json::json!("policy violation");
    let index: Index = serde_json::from_value(value).expect("带 yank_reason 的索引必须能解析");
    index.validate(None).expect("新增可选字段不影响 v1 校验");
    assert_eq!(
        index.plugins[0].yank_reason.as_deref(),
        Some("policy violation")
    );
    let note = takedown_note(&index, "com.example.demo", "1.0.0").unwrap();
    assert!(note.contains("policy violation"), "{note}");
    assert!(entries_of(&index).plugins[0]
        .warn
        .contains("policy violation"));
}

/// 选择器口径：未 yank、未命中 `revoked[]` 的最大版本；全部不可装时为空。
#[test]
fn selector_skips_yanked_and_revoked_versions() {
    fn variant(
        base: &serde_json::Value,
        ver: &str,
        yanked: bool,
        reason: Option<&str>,
    ) -> serde_json::Value {
        let mut p = base.clone();
        p["version"] = serde_json::json!(ver);
        p["yanked"] = serde_json::json!(yanked);
        if let Some(reason) = reason {
            p["yank_reason"] = serde_json::json!(reason);
        }
        p
    }

    let base_value: serde_json::Value = serde_json::from_str(LEGACY_INDEX).unwrap();
    let base = base_value["plugins"][0].clone();

    // 最新版 yanked → 回退到中间版本；更高但被吊销的版本同样跳过。
    let mut value = base_value.clone();
    value["plugins"] = serde_json::json!([
        variant(&base, "1.0.0", false, None),
        variant(&base, "1.1.0", false, None),
        variant(&base, "1.2.0", true, Some("policy violation")),
        variant(&base, "1.3.0", false, None),
    ]);
    value["revoked"] = serde_json::json!([{
        "id": "com.example.demo",
        "version": "1.3.0",
        "reason": "private key leaked",
        "at": "2026-11-11T00:00:00Z"
    }]);
    let index: Index = serde_json::from_value(value).unwrap();
    index.validate(None).unwrap();
    let items = entries_of(&index).plugins;
    assert_eq!(
        installable_versions(&items)
            .get("com.example.demo")
            .map(String::as_str),
        Some("1.1.0"),
        "必须跳过 yanked 与 revoked 的更高版本"
    );
    assert_eq!(
        newest_versions(&items)
            .get("com.example.demo")
            .map(String::as_str),
        Some("1.3.0"),
        "最高版本只用于展示原因，不代表可安装"
    );

    // 全部 yank → 没有可安装版本，最高版本行带可读原因。
    let mut value = base_value.clone();
    value["plugins"] = serde_json::json!([
        variant(&base, "1.0.0", true, None),
        variant(&base, "1.1.0", true, Some("policy violation")),
    ]);
    let index: Index = serde_json::from_value(value).unwrap();
    let items = entries_of(&index).plugins;
    assert!(!installable_versions(&items).contains_key("com.example.demo"));
    let newest = newest_versions(&items)["com.example.demo"].clone();
    let row = items
        .iter()
        .find(|r| r.version == newest)
        .expect("最高版本行必须存在");
    assert!(
        row.warn.contains("policy violation"),
        "禁用按钮所在行必须带 yank 原因: {row:?}"
    );

    // 未撤包的单版本：行为不变。
    let index: Index = serde_json::from_str(LEGACY_INDEX).unwrap();
    let items = entries_of(&index).plugins;
    assert_eq!(
        installable_versions(&items)
            .get("com.example.demo")
            .map(String::as_str),
        Some("1.0.0")
    );
}
