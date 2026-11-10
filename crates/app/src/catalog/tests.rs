//! 目录客户端单测：全部走假传输层，不触网。

use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::fetch::testing::FakeTransport;
use super::fetch::HttpReply;
use super::model::{INDEX_SCHEMA, MANIFEST_SCHEMA};
use super::verify::testing::{sign_hello, test_key, TestKey};
use super::*;

const MIRROR: &str = "https://mirror.example/skiff";
const BLOB_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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

    // 同一个渠道换成另一把公钥：key_id 与清单不符 → 拒绝。
    let other = test_key(9);
    let mut settings = fx.settings();
    settings.public_key = Some(other.public_hex.clone());
    fx.serve_signed(2, "com.example.two", "etag-m2", "etag-i2");
    let snap = fx.service_with(fx.fake.clone(), settings).snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("校验失败"), "{}", snap.message);
    // 旧快照（seq 1）仍可展示
    assert_eq!(snap.seq, Some(1));
    assert_eq!(snap.entries.plugins[0].name, "com.example.one");
}

#[test]
fn seq_rollback_is_rejected() {
    let fx = Fixture::new();
    fx.serve_signed(5, "com.example.five", "etag-m", "etag-i");
    assert_eq!(fx.service().snapshot(true).state, CatalogState::Fresh);

    fx.serve_signed(4, "com.example.four", "etag-m2", "etag-i2");
    let snap = fx.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("防回滚"), "{}", snap.message);
    assert_eq!(snap.seq, Some(5));
    assert_eq!(snap.entries.plugins[0].name, "com.example.five");
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
    let snap = fx.service().snapshot(true);
    assert_eq!(snap.state, CatalogState::Rejected);
    assert!(snap.message.contains("摘要"), "{}", snap.message);
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

    let snap = fx
        .service_with(FakeTransport::new(), fx.settings())
        .snapshot(true);
    assert_eq!(snap.state, CatalogState::Offline);
    assert!(snap.message.contains("离线"), "{}", snap.message);
    assert_eq!(snap.seq, Some(2));
    assert_eq!(snap.entries.plugins[0].name, "com.example.two");
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
