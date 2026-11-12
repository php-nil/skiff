//! 发布面 API 的端到端测试：进程内起监听、真实签名请求、真实 `.wbpkg`。
//!
//! 覆盖：签名正例、篡改 body、错误域前缀、过期时间戳、重放 nonce、非审核钥
//! approve、自我审核、同摘要幂等、非法状态迁移、Release job 生成签名产物。

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::SigningKey;
use sha2::Digest;
use skiff_pkgfmt::signature::SigningKey as PkgSigningKey;
use skiff_server::api::{self, ApiServer};
use skiff_server::events;
use skiff_server::http::Request;
use skiff_server::serve;

const AUTHORS_SCHEMA: &str = "skiff.server.authors/v1";
const REVIEWERS_SCHEMA: &str = "skiff.server.reviewers/v1";
const API_DOMAIN: &[u8] = b"SKIFF-SERVER-API-v1\0";
const MANIFEST_DOMAIN: &[u8] = b"SKIFF-SERVER-MANIFEST-v1\0";

struct Fixture {
    dir: tempfile::TempDir,
    state: PathBuf,
    catalog: PathBuf,
    addr: SocketAddr,
    author: SigningKey,
    reviewer: SigningKey,
    _server: std::thread::JoinHandle<()>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_same_key(false)
    }

    /// `same_key = true`：作者钥同时登记为审核钥，用于自我审核测试。
    fn with_same_key(same_key: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        fs::create_dir_all(&state).unwrap();
        set_mode(&state, 0o700);
        let author = SigningKey::from_bytes(&[7u8; 32]);
        let reviewer = if same_key {
            SigningKey::from_bytes(&[7u8; 32])
        } else {
            SigningKey::from_bytes(&[9u8; 32])
        };
        let author_hex = hex::encode(author.verifying_key().to_bytes());
        let reviewer_hex = hex::encode(reviewer.verifying_key().to_bytes());
        let key_id =
            |key: &SigningKey| hex::encode(sha2::Sha256::digest(key.verifying_key().to_bytes()));

        let authors = serde_json::json!({
            "schema": AUTHORS_SCHEMA,
            "authors": [{
                "key_id": key_id(&author),
                "public_key": author_hex,
                "name": "alice",
                "id_prefixes": ["com.example"],
                "disabled": false
            }]
        });
        let reviewers = serde_json::json!({
            "schema": REVIEWERS_SCHEMA,
            "reviewers": [{
                "key_id": key_id(&reviewer),
                "public_key": reviewer_hex,
                "name": "reviewer"
            }]
        });
        let authors_path = dir.path().join("authors.json");
        let reviewers_path = dir.path().join("reviewers.json");
        fs::write(&authors_path, serde_json::to_vec_pretty(&authors).unwrap()).unwrap();
        fs::write(
            &reviewers_path,
            serde_json::to_vec_pretty(&reviewers).unwrap(),
        )
        .unwrap();
        set_mode(&authors_path, 0o600);
        set_mode(&reviewers_path, 0o600);

        let catalog = dir.path().join("catalog");
        prepare_catalog(&catalog);
        let server = ApiServer::prepare(
            state.clone(),
            authors_path,
            reviewers_path,
            catalog.clone(),
            "stable".to_string(),
            false,
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let _ = api::serve(listener, server);
        });
        Self {
            dir,
            state,
            catalog,
            addr,
            author,
            reviewer,
            _server: handle,
        }
    }

    fn package(&self, id: &str, version: &str) -> Vec<u8> {
        let src = self.dir.path().join(format!("src-{id}-{version}"));
        let key = PkgSigningKey::from_seed_hex(&hex::encode(self.author.to_bytes())).unwrap();
        let manifest = format!(
            r#"id = "{id}"
name = "Demo"
version = "{version}"
key_id = "{}"
api = "1.0"
requires = ["docker"]
idle_timeout_sec = 300

[[permissions]]
capability = "docker"
verbs = ["containers.list"]
"#,
            key.key_id()
        );
        write_file(&src.join("manifest.toml"), manifest.as_bytes());
        write_file(&src.join("bin/amd64/plugin"), b"#!/bin/sh\necho demo\n");
        write_file(&src.join("ui/index.html"), b"<html>demo</html>");
        let out = self.dir.path().join(format!("{id}-{version}.wbpkg"));
        skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
        fs::read(out).unwrap()
    }

    fn submission_body(&self, package: &[u8]) -> (String, Vec<u8>) {
        let boundary = "skiff-test-boundary";
        let entry = serde_json::json!({
            "summary": "示例插件",
            "channel": "stable"
        })
        .to_string();
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"entry\"\r\n\r\n");
        body.extend_from_slice(entry.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"package\"; filename=\"x.wbpkg\"\r\n\r\n",
        );
        body.extend_from_slice(package);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }
}

fn write_file(path: &Path, data: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, data).unwrap();
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_default()
    }
}

fn canonical(
    domain: &[u8],
    method: &str,
    target: &str,
    body: &[u8],
    timestamp: i64,
    nonce: &str,
) -> Vec<u8> {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let body_sha = hex::encode(sha2::Sha256::digest(body));
    let mut out = domain.to_vec();
    out.extend_from_slice(method.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(path.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(query.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(body_sha.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(timestamp.to_string().as_bytes());
    out.push(b'\n');
    out.extend_from_slice(nonce.as_bytes());
    out
}

fn signed_call(
    addr: SocketAddr,
    key: &SigningKey,
    method: &str,
    target: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let nonce = format!(
        "n{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    signed_call_with(
        addr, key, method, target, extra, body, API_DOMAIN, 0, &nonce,
    )
}

#[allow(clippy::too_many_arguments)]
fn signed_call_with(
    addr: SocketAddr,
    key: &SigningKey,
    method: &str,
    target: &str,
    extra: &[(&str, &str)],
    body: &[u8],
    domain: &[u8],
    timestamp_offset: i64,
    nonce: &str,
) -> Reply {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + timestamp_offset;
    let message = canonical(domain, method, target, body, timestamp, nonce);
    let signature = ed25519_dalek::Signer::sign(key, &message);
    let key_id = hex::encode(sha2::Sha256::digest(key.verifying_key().to_bytes()));

    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: localhost\r\n");
    head.push_str(&format!("X-Skiff-Key-Id: {key_id}\r\n"));
    head.push_str(&format!("X-Skiff-Timestamp: {timestamp}\r\n"));
    head.push_str(&format!("X-Skiff-Nonce: {nonce}\r\n"));
    head.push_str(&format!(
        "X-Skiff-Signature: {}\r\n",
        hex::encode(signature.to_bytes())
    ));
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");

    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    parse_reply(&raw)
}

fn call_raw(addr: SocketAddr, request: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    parse_reply(&raw)
}

fn parse_reply(raw: &[u8]) -> Reply {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("响应缺少头部结束标记");
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .expect("响应缺少状态码");
    Reply {
        status,
        body: raw[split + 4..].to_vec(),
    }
}

fn submit(fx: &Fixture, id: &str, version: &str) -> Reply {
    let package = fx.package(id, version);
    let (content_type, body) = fx.submission_body(&package);
    signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
    )
}

const APP_COMPOSE: &str =
    "services:\n  web:\n    image: nginx:1.27-alpine\n    ports:\n      - \"8080:80\"\n";

fn app_body(entry: &serde_json::Value, compose: &[u8]) -> (String, Vec<u8>) {
    let boundary = "skiff-app-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"entry\"\r\n\r\n");
    body.extend_from_slice(entry.to_string().as_bytes());
    body.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"compose\"; filename=\"compose.yaml\"\r\n\r\n",
    );
    body.extend_from_slice(compose);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn app_entry(id: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "version": version,
        "summary": "示例应用",
        "min_kernel": "1.2.0",
        "images": [{ "ref": "nginx:1.27-alpine", "digest": format!("sha256:{}", "a".repeat(64)) }],
        "env": [{ "key": "TZ", "default": "UTC" }],
        "ports": [{ "container": 80, "protocol": "tcp" }],
        "volumes": ["/data"]
    })
}

fn submit_app(fx: &Fixture, id: &str, version: &str, compose: &[u8]) -> Reply {
    let (content_type, body) = app_body(&app_entry(id, version), compose);
    signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/apps",
        &[("Content-Type", &content_type)],
        &body,
    )
}

fn approve(fx: &Fixture, sid: &str) -> Reply {
    signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    )
}

fn runtime_entry(kind: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "version": version,
        "image": format!("php:{version}-fpm-alpine"),
        "image_digest": format!("sha256:{}", "b".repeat(64)),
        "eol": null,
        "api_min": "1.0",
        "default": false
    })
}

fn submit_runtime(fx: &Fixture, entry: &serde_json::Value) -> Reply {
    signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/runtimes",
        &[("Content-Type", "application/json")],
        entry.to_string().as_bytes(),
    )
}

fn revise(fx: &Fixture, sid: &str, body: &serde_json::Value) -> Reply {
    signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/revise"),
        &[("Content-Type", "application/json")],
        body.to_string().as_bytes(),
    )
}

#[test]
fn signed_submit_approve_and_release_produces_pullable_artifacts() {
    let fx = Fixture::new();
    let reply = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(reply.status, 201, "{:?}", reply.json());
    let sid = reply.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(reply.json()["state"], "pending");

    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202, "{:?}", approve.json());

    // 源数据 + 产物：只产出 staging，不碰线上目录。
    let catalog = fx.catalog.clone();
    let schema = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/skiff-server/schema");
    let staging = fx.dir.path().join("staging");
    let signer = SigningKey::from_bytes(&[11u8; 32]);
    std::env::set_var("SKIFF_SIGNING_KEY", hex::encode(signer.to_bytes()));
    std::env::set_var(
        "SKIFF_SERVER_PUBLIC_KEY",
        hex::encode(signer.verifying_key().to_bytes()),
    );
    let job = api::job::JobOptions {
        state: fx.state.clone(),
        catalog: catalog.clone(),
        schema,
        channel: "stable".to_string(),
        staging_out: Some(staging.clone()),
        dist: None,
        swap: false,
        seq: None,
        prev: None,
        submission: None,
        public_key: None,
    };
    let report = api::job::run(&job).unwrap();
    assert_eq!(report.seq, 1);
    assert!(!report.swapped);

    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(staging.join("channels/stable/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["seq"], 1);
    let index_text = fs::read_to_string(staging.join("channels/stable/index.json")).unwrap();
    assert!(index_text.contains("com.example.demo"), "{index_text}");
    let plugin: serde_json::Value = serde_json::from_str(&index_text).unwrap();
    let sha = plugin["plugins"][0]["pkg_sha256"].as_str().unwrap();
    assert!(staging.join("blobs/sha256").join(sha).is_file());

    // 状态回写：published + seq。
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.state.join("submissions").join(format!("{sid}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["state"], "published");
    assert_eq!(record["release"]["seq"], 1);
}

#[test]
fn tampered_body_is_rejected() {
    let fx = Fixture::new();
    let package = fx.package("com.example.demo", "1.0.0");
    let (content_type, body) = fx.submission_body(&package);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let message = canonical(
        API_DOMAIN,
        "POST",
        "/api/v1/submissions",
        &body,
        timestamp,
        "n1",
    );
    let signature = ed25519_dalek::Signer::sign(&fx.author, &message);
    let key_id = hex::encode(sha2::Sha256::digest(fx.author.verifying_key().to_bytes()));
    let mut tampered = body.clone();
    tampered[0] = b'X';
    let head = format!(
        "POST /api/v1/submissions HTTP/1.1\r\nHost: localhost\r\nX-Skiff-Key-Id: {key_id}\r\n\
         X-Skiff-Timestamp: {timestamp}\r\nX-Skiff-Nonce: n1\r\nX-Skiff-Signature: {}\r\n\
         Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        hex::encode(signature.to_bytes()),
        tampered.len()
    );
    let mut stream = TcpStream::connect(fx.addr).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(&tampered).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    assert_eq!(parse_reply(&raw).status, 401);
}

#[test]
fn wrong_signature_domain_is_rejected() {
    let fx = Fixture::new();
    let package = fx.package("com.example.demo", "1.0.0");
    let (content_type, body) = fx.submission_body(&package);
    let reply = signed_call_with(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
        MANIFEST_DOMAIN,
        0,
        "domain-nonce",
    );
    assert_eq!(reply.status, 401);
}

#[test]
fn expired_timestamp_is_rejected() {
    let fx = Fixture::new();
    let package = fx.package("com.example.demo", "1.0.0");
    let (content_type, body) = fx.submission_body(&package);
    let reply = signed_call_with(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
        API_DOMAIN,
        -301,
        "expired-nonce",
    );
    assert_eq!(reply.status, 401);
}

#[test]
fn replayed_nonce_is_rejected() {
    let fx = Fixture::new();
    let package = fx.package("com.example.demo", "1.0.0");
    let (content_type, body) = fx.submission_body(&package);
    let first = signed_call_with(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
        API_DOMAIN,
        0,
        "replay-nonce",
    );
    assert_eq!(first.status, 201, "{:?}", first.json());
    let second = signed_call_with(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
        API_DOMAIN,
        0,
        "replay-nonce",
    );
    assert_eq!(second.status, 401);
}

#[test]
fn author_cannot_approve() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let reply = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(reply.status, 403, "{:?}", reply.json());
}

#[test]
fn self_review_is_forbidden() {
    let fx = Fixture::with_same_key(true);
    let created = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(created.status, 201, "{:?}", created.json());
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let reply = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(reply.status, 403, "{:?}", reply.json());
}

#[test]
fn same_digest_resubmit_returns_same_submission() {
    let fx = Fixture::new();
    let first = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(first.status, 201);
    let package = fx.package("com.example.demo", "1.0.0");
    let (content_type, body) = fx.submission_body(&package);
    let second = signed_call_with(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/submissions",
        &[("Content-Type", &content_type)],
        &body,
        API_DOMAIN,
        0,
        "resubmit-nonce",
    );
    assert_eq!(second.status, 200, "{:?}", second.json());
    assert_eq!(first.json()["sid"], second.json()["sid"]);
}

#[test]
fn plugin_records_omit_kind_and_legacy_records_still_load() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(created.status, 201, "{:?}", created.json());
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert!(
        created.json().get("kind").is_none(),
        "插件提交响应不应出现 kind: {}",
        created.json()
    );

    let path = fx.state.join("submissions").join(format!("{sid}.json"));
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(raw.get("kind").is_none(), "插件记录不应落 kind: {raw}");

    // 旧记录（无 kind）按 plugin 解析：R1 时代落盘的文件不需要迁移。
    let legacy: api::store::Submission = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(legacy.kind, api::store::EntryKind::Plugin);

    // 全链路：approve → release 后源条目与索引都不出现 kind。
    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202, "{:?}", approve.json());
    let (_, staging) = publish(&fx, 1);
    let source = fx
        .catalog
        .join("stable/plugins/com.example.demo-1.0.0.json");
    let source_text = fs::read_to_string(&source).unwrap();
    assert!(!source_text.contains("\"kind\""), "{source_text}");
    let index_text = fs::read_to_string(staging.path().join("channels/stable/index.json")).unwrap();
    assert!(index_text.contains("com.example.demo"), "{index_text}");
    assert!(!index_text.contains("\"kind\""), "{index_text}");
}

#[test]
fn app_submit_approve_release_writes_compose_blob_and_index() {
    let fx = Fixture::new();
    let created = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    assert_eq!(created.status, 201, "{:?}", created.json());
    let body = created.json();
    assert_eq!(body["kind"], "app");
    assert_eq!(body["plugin_id"], "com.example.blog");
    assert_eq!(body["app"]["min_kernel"], "1.2.0");
    assert_eq!(body["app"]["ports"][0]["container"], 80);
    let sid = body["sid"].as_str().unwrap().to_string();

    let approved = approve(&fx, &sid);
    assert_eq!(approved.status, 202, "{:?}", approved.json());

    let (report, staging) = publish(&fx, 1);
    assert_eq!(report.released.len(), 1);
    assert_eq!(report.seq, 1);

    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["seq"], 1);
    assert_eq!(index["plugins"].as_array().unwrap().len(), 0);
    let app = &index["apps"][0];
    let sha = hex::encode(sha2::Sha256::digest(APP_COMPOSE.as_bytes()));
    assert_eq!(app["id"], "com.example.blog");
    assert_eq!(app["version"], "1.0.0");
    assert_eq!(app["compose_sha256"], sha);
    assert_eq!(app["compose_url"], format!("blobs/sha256/{sha}"));
    assert_eq!(
        fs::read(staging.path().join("blobs/sha256").join(&sha)).unwrap(),
        APP_COMPOSE.as_bytes()
    );

    let source: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.catalog.join("stable/apps/com.example.blog-1.0.0.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(source["asset"], format!("assets/{sha}.yaml"));
    assert_eq!(
        fs::read(fx.catalog.join(format!("assets/{sha}.yaml"))).unwrap(),
        APP_COMPOSE.as_bytes()
    );
}

#[test]
fn app_compose_outside_supported_subset_is_rejected() {
    let fx = Fixture::new();
    let compose = b"services:\n  web:\n    build: .\n";
    let reply = submit_app(&fx, "com.example.blog", "1.0.0", compose);
    assert_eq!(reply.status, 422, "{:?}", reply.json());
    let text = serde_json::to_string(&reply.json()).unwrap();
    assert!(text.contains("compose"), "{text}");

    let (content_type, body) = app_body(
        &app_entry("com.example.blog", "1.0.0"),
        b"services:\n  web:\n    image: nginx\n\xff\n",
    );
    let reply = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/apps",
        &[("Content-Type", &content_type)],
        &body,
    );
    assert_eq!(reply.status, 422, "{:?}", reply.json());
    assert!(fs::read_dir(fx.state.join("submissions"))
        .map(|d| d.count() == 0)
        .unwrap_or(true));
}

#[test]
fn same_app_version_with_different_compose_conflicts() {
    let fx = Fixture::new();
    let first = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    assert_eq!(first.status, 201, "{:?}", first.json());
    let sid = first.json()["sid"].as_str().unwrap().to_string();

    // 同摘要重复提交：幂等 200，返回同一 sid。
    let again = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    assert_eq!(again.status, 200, "{:?}", again.json());
    assert_eq!(again.json()["sid"], sid);

    // 同版本不同摘要：409。
    let changed = APP_COMPOSE.replace("8080:80", "8081:80");
    let conflict = submit_app(&fx, "com.example.blog", "1.0.0", changed.as_bytes());
    assert_eq!(conflict.status, 409, "{:?}", conflict.json());
}

#[test]
fn plugin_and_app_release_share_one_seq() {
    let fx = Fixture::new();
    let plugin = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(plugin.status, 201, "{:?}", plugin.json());
    let plugin_sid = plugin.json()["sid"].as_str().unwrap().to_string();
    let app = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    assert_eq!(app.status, 201, "{:?}", app.json());
    let app_sid = app.json()["sid"].as_str().unwrap().to_string();

    assert_eq!(approve(&fx, &plugin_sid).status, 202);
    assert_eq!(approve(&fx, &app_sid).status, 202);

    let (report, staging) = publish(&fx, 1);
    assert_eq!(report.seq, 1);
    assert_eq!(report.released.len(), 2);
    assert!(
        report.released.iter().all(|(_, seq)| *seq == 1),
        "{:?}",
        report.released
    );

    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["seq"], 1);
    assert_eq!(index["plugins"].as_array().unwrap().len(), 1);
    assert_eq!(index["apps"].as_array().unwrap().len(), 1);
    assert_eq!(index["plugins"][0]["id"], "com.example.demo");
    assert_eq!(index["apps"][0]["id"], "com.example.blog");
}

#[test]
fn runtime_submit_approve_release_writes_index_entry() {
    let fx = Fixture::new();
    let created = submit_runtime(&fx, &runtime_entry("php", "8.5"));
    assert_eq!(created.status, 201, "{:?}", created.json());
    let body = created.json();
    assert_eq!(body["kind"], "runtime");
    assert_eq!(body["plugin_id"], "php");
    assert_eq!(body["runtime"]["image"], "php:8.5-fpm-alpine");
    assert_eq!(body["runtime"]["default"], false);
    let sid = body["sid"].as_str().unwrap().to_string();

    assert_eq!(approve(&fx, &sid).status, 202);
    let (report, staging) = publish(&fx, 1);
    assert_eq!(report.seq, 1);

    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    let runtime = &index["runtimes"][0];
    assert_eq!(runtime["kind"], "php");
    assert_eq!(runtime["version"], "8.5");
    assert_eq!(runtime["image"], "php:8.5-fpm-alpine");
    assert_eq!(
        runtime["image_digest"],
        format!("sha256:{}", "b".repeat(64))
    );
    assert_eq!(runtime["api_min"], "1.0");
    assert_eq!(runtime["default"], false);
    assert!(runtime["eol"].is_null());
    assert!(fx.catalog.join("stable/runtimes/php-8.5.json").is_file());
}

#[test]
fn runtime_default_must_be_false_at_submission() {
    let fx = Fixture::new();
    let mut entry = runtime_entry("php", "8.5");
    entry["default"] = serde_json::json!(true);
    let reply = submit_runtime(&fx, &entry);
    assert_eq!(reply.status, 422, "{:?}", reply.json());
    let text = serde_json::to_string(&reply.json()).unwrap();
    assert!(text.contains("修订"), "{text}");
}

#[test]
fn runtime_revision_changes_eol_and_default_with_audit() {
    let fx = Fixture::new();
    let created = submit_runtime(&fx, &runtime_entry("php", "8.5"));
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &sid).status, 202);
    publish(&fx, 1);

    // 镜像与 digest 不可改：带 image_digest 的修订直接 422。
    let rejected = revise(
        &fx,
        &sid,
        &serde_json::json!({ "image_digest": format!("sha256:{}", "c".repeat(64)) }),
    );
    assert_eq!(rejected.status, 422, "{:?}", rejected.json());

    let revised = revise(
        &fx,
        &sid,
        &serde_json::json!({ "eol": "2027-01-31", "default": true }),
    );
    assert_eq!(revised.status, 202, "{:?}", revised.json());
    assert!(revised.json()["seq"].is_null());

    let (report, staging) = publish(&fx, 2);
    assert_eq!(report.seq, 2);
    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["runtimes"][0]["eol"], "2027-01-31");
    assert_eq!(index["runtimes"][0]["default"], true);

    let audit = fs::read_to_string(fx.state.join("audit.jsonl")).unwrap();
    assert!(
        audit
            .lines()
            .any(|line| line.contains("\"action\":\"revise\"") && line.contains(&sid)),
        "{audit}"
    );
}

#[test]
fn runtime_revision_requires_reviewer_and_published_state() {
    let fx = Fixture::new();
    let created = submit_runtime(&fx, &runtime_entry("php", "8.5"));
    let sid = created.json()["sid"].as_str().unwrap().to_string();

    // 未发布：412。
    let pending = revise(&fx, &sid, &serde_json::json!({ "eol": "2027-01-31" }));
    assert_eq!(pending.status, 412, "{:?}", pending.json());

    // 作者不能修订。
    let author = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        &format!("/api/v1/submissions/{sid}/revise"),
        &[("Content-Type", "application/json")],
        b"{\"default\":true}",
    );
    assert_eq!(author.status, 403, "{:?}", author.json());
}

#[test]
fn release_rejects_multiple_defaults_per_kind() {
    let fx = Fixture::new();
    let first = submit_runtime(&fx, &runtime_entry("php", "8.5"));
    let first_sid = first.json()["sid"].as_str().unwrap().to_string();
    let second = submit_runtime(&fx, &runtime_entry("php", "8.4"));
    let second_sid = second.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &first_sid).status, 202);
    assert_eq!(approve(&fx, &second_sid).status, 202);
    publish(&fx, 1);

    assert_eq!(
        revise(&fx, &first_sid, &serde_json::json!({ "default": true })).status,
        202
    );
    assert_eq!(
        revise(&fx, &second_sid, &serde_json::json!({ "default": true })).status,
        202
    );
    let (result, _staging) = publish_raw(&fx, 2);
    let error = result.expect_err("同 kind 两个 default 必须让 release 失败");
    let message = format!("{error:#}");
    assert!(message.contains("default"), "{message}");
}

fn yank(fx: &Fixture, path: &str, reason: &str) -> Reply {
    let body = serde_json::json!({ "reason": reason }).to_string();
    signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

#[test]
fn yank_app_marks_source_and_revoked_apps_only() {
    let fx = Fixture::new();
    let created = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &sid).status, 202);
    publish(&fx, 1);

    let reply = yank(
        &fx,
        "/api/v1/apps/com.example.blog/versions/1.0.0/yank",
        "compose 引用不可信镜像",
    );
    assert_eq!(reply.status, 202, "{:?}", reply.json());
    assert_eq!(reply.json()["entry_kind"], "app");

    let (report, staging) = publish(&fx, 2);
    assert_eq!(report.seq, 2);
    let text = fs::read_to_string(staging.path().join("channels/stable/index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(index["apps"][0]["yanked"], true);
    assert_eq!(index["apps"][0]["yank_reason"], "compose 引用不可信镜像");
    let sha = hex::encode(sha2::Sha256::digest(APP_COMPOSE.as_bytes()));
    let revoked = &index["revoked_apps"][0];
    assert_eq!(revoked["id"], "com.example.blog");
    assert_eq!(revoked["version"], "1.0.0");
    assert_eq!(revoked["sha256"], sha);
    assert_eq!(revoked["reason"], "compose 引用不可信镜像");
    assert!(revoked["at"].as_str().unwrap().len() == 20);
    assert_eq!(index["revoked"].as_array().unwrap().len(), 0);
    assert!(!text.contains("revoked_runtimes"), "{text}");

    let source: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.catalog.join("stable/apps/com.example.blog-1.0.0.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(source["yanked"], true);

    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.state.join("submissions").join(format!("{sid}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["takedown"]["kind"], "yank");
    assert_eq!(record["takedown"]["reason"], "compose 引用不可信镜像");
}

#[test]
fn yank_runtime_marks_revoked_runtimes_only() {
    let fx = Fixture::new();
    let created = submit_runtime(&fx, &runtime_entry("php", "8.5"));
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &sid).status, 202);
    publish(&fx, 1);

    let reply = yank(
        &fx,
        "/api/v1/runtimes/php/versions/8.5/yank",
        "基础镜像 CVE",
    );
    assert_eq!(reply.status, 202, "{:?}", reply.json());
    assert_eq!(reply.json()["entry_kind"], "runtime");

    let (_, staging) = publish(&fx, 2);
    let text = fs::read_to_string(staging.path().join("channels/stable/index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(index["runtimes"][0]["yanked"], true);
    let revoked = &index["revoked_runtimes"][0];
    assert_eq!(revoked["id"], "php");
    assert_eq!(revoked["version"], "8.5");
    assert_eq!(revoked["sha256"], "b".repeat(64));
    assert_eq!(index["revoked"].as_array().unwrap().len(), 0);
    assert!(!text.contains("revoked_apps"), "{text}");
}

#[test]
fn yank_kind_endpoints_are_reviewer_only_and_idempotent() {
    let fx = Fixture::new();
    let created = submit_app(&fx, "com.example.blog", "1.0.0", APP_COMPOSE.as_bytes());
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &sid).status, 202);
    publish(&fx, 1);

    // 作者不能撤包。
    let author = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/apps/com.example.blog/versions/1.0.0/yank",
        &[],
        b"",
    );
    assert_eq!(author.status, 403, "{:?}", author.json());

    // 不存在的目标 404。
    let missing = yank(
        &fx,
        "/api/v1/apps/com.example.none/versions/9.9.9/yank",
        "no such app",
    );
    assert_eq!(missing.status, 404, "{:?}", missing.json());

    let first = yank(
        &fx,
        "/api/v1/apps/com.example.blog/versions/1.0.0/yank",
        "下架",
    );
    assert_eq!(first.status, 202, "{:?}", first.json());
    let again = yank(
        &fx,
        "/api/v1/apps/com.example.blog/versions/1.0.0/yank",
        "下架",
    );
    assert_eq!(again.status, 200, "{:?}", again.json());
    assert_eq!(again.json()["id"], first.json()["id"]);
}

#[test]
fn plugin_yank_keeps_revoked_namespaces_untouched() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    assert_eq!(approve(&fx, &sid).status, 202);
    publish(&fx, 1);

    let reply = yank(
        &fx,
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        "插件下架",
    );
    assert_eq!(reply.status, 202, "{:?}", reply.json());
    assert!(
        reply.json().get("entry_kind").is_none(),
        "插件撤包记录不应出现 entry_kind: {}",
        reply.json()
    );

    let (_, staging) = publish(&fx, 2);
    let text = fs::read_to_string(staging.path().join("channels/stable/index.json")).unwrap();
    let index: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(index["plugins"][0]["yanked"], true);
    assert_eq!(index["revoked"].as_array().unwrap().len(), 0);
    assert!(!text.contains("revoked_apps"), "{text}");
    assert!(!text.contains("revoked_runtimes"), "{text}");
}

#[test]
fn illegal_state_transition_is_rejected() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202);
    // approved 之后作者不能再撤回。
    let withdraw = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        &format!("/api/v1/submissions/{sid}/withdraw"),
        &[],
        b"",
    );
    assert_eq!(withdraw.status, 412, "{:?}", withdraw.json());
}

#[test]
fn missing_content_length_and_transfer_encoding_are_rejected() {
    let fx = Fixture::new();
    let reply = call_raw(
        fx.addr,
        "POST /api/v1/submissions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(reply.status, 411, "{:?}", reply.json());

    let chunked = call_raw(
        fx.addr,
        "POST /api/v1/submissions HTTP/1.1\r\nHost: localhost\r\n\
         Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(chunked.status, 400, "{:?}", chunked.json());
}

#[test]
fn oversized_content_length_is_rejected_without_reading_body() {
    let fx = Fixture::new();
    let reply = call_raw(
        fx.addr,
        "POST /api/v1/submissions HTTP/1.1\r\nHost: localhost\r\n\
         Content-Length: 999999999\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(reply.status, 413, "{:?}", reply.json());
}

#[test]
fn failed_release_does_not_touch_dist() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202);

    let catalog = fx.catalog.clone();
    // 空的 schema 目录 → publish 在写盘前失败。
    let schema = fx.dir.path().join("empty-schema");
    fs::create_dir_all(&schema).unwrap();
    let dist = fx.dir.path().join("dist");
    fs::create_dir_all(&dist).unwrap();
    fs::write(dist.join("marker.txt"), b"live").unwrap();

    let signer = SigningKey::from_bytes(&[11u8; 32]);
    std::env::set_var("SKIFF_SIGNING_KEY", hex::encode(signer.to_bytes()));
    std::env::set_var(
        "SKIFF_SERVER_PUBLIC_KEY",
        hex::encode(signer.verifying_key().to_bytes()),
    );
    let job = api::job::JobOptions {
        state: fx.state.clone(),
        catalog,
        schema,
        channel: "stable".to_string(),
        staging_out: None,
        dist: Some(dist.clone()),
        swap: true,
        seq: Some(2),
        prev: None,
        submission: None,
        public_key: None,
    };
    assert!(api::job::run(&job).is_err(), "schema 为空时发布必须失败");
    assert!(dist.join("marker.txt").is_file(), "失败不得改动线上目录");
    assert!(!dist.join("channels").exists());

    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.state.join("submissions").join(format!("{sid}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["state"], "approved", "失败后不得标记 published");
}

fn pending_events(state: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = fs::read_dir(events::pending_dir(state)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect()
}

fn event_types(state: &Path) -> Vec<String> {
    pending_events(state)
        .iter()
        .filter_map(|event| event["type"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn submit_succeeds_when_outbox_cannot_be_written() {
    let fx = Fixture::new();
    // 让 outbox 路径变成普通文件：事件写不进去，但业务必须照常成功。
    fs::write(fx.state.join("outbox"), b"not a directory").unwrap();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    assert_eq!(created.status, 201, "{:?}", created.json());
}

#[test]
fn release_failure_emits_event() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202);

    let staging = tempfile::tempdir_in(fx.dir.path()).unwrap();
    let job = api::job::JobOptions {
        state: fx.state.clone(),
        catalog: fx.catalog.clone(),
        // 不存在的 schema 目录 → 发布在写盘前失败。
        schema: fx.dir.path().join("missing-schema"),
        channel: "stable".to_string(),
        staging_out: Some(staging.path().to_path_buf()),
        dist: None,
        swap: false,
        seq: Some(1),
        prev: None,
        submission: None,
        public_key: None,
    };
    assert!(api::job::run(&job).is_err());
    assert!(
        event_types(&fx.state).iter().any(|t| t == "release.failed"),
        "发布失败必须留下 release.failed 事件"
    );
}

#[test]
fn api_actions_emit_outbox_events() {
    let fx = Fixture::new();
    published_submission(&fx);
    let yank = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        &[("Content-Type", "application/json")],
        br#"{"reason":"policy violation"}"#,
    );
    assert_eq!(yank.status, 202);

    let types = event_types(&fx.state);
    for expected in [
        "submission.created",
        "submission.approved",
        "submission.published",
        "takedown.yanked",
    ] {
        assert!(
            types.iter().any(|t| t == expected),
            "缺少事件 {expected}: {types:?}"
        );
    }
    for event in pending_events(&fx.state) {
        let text = serde_json::to_string(&event).unwrap();
        for forbidden in ["package", "signature", "wbpkg"] {
            assert!(!text.contains(forbidden), "{text}");
        }
    }
}

#[test]
fn serve_still_returns_404_for_api_paths() {
    let dir = tempfile::tempdir().unwrap();
    let response = serve::respond(dir.path(), &Request::new("GET", "/api/v1/submissions"));
    assert_eq!(response.status, 404);
}

/// 只产出 staging 的发布 helper：seq 显式给定，避免并行测试共享 `--prev`。
fn publish(fx: &Fixture, seq: u64) -> (api::job::JobReport, tempfile::TempDir) {
    let (report, staging) = publish_raw(fx, seq);
    let report = report.unwrap_or_else(|error| panic!("{error:#}"));
    (report, staging)
}

/// 同 `publish`，但把失败原样交给调用方（用于断言 release 拒绝）。
fn publish_raw(fx: &Fixture, seq: u64) -> (anyhow::Result<api::job::JobReport>, tempfile::TempDir) {
    let schema = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/skiff-server/schema");
    let staging = tempfile::tempdir_in(fx.dir.path()).unwrap();
    let signer = SigningKey::from_bytes(&[11u8; 32]);
    std::env::set_var("SKIFF_SIGNING_KEY", hex::encode(signer.to_bytes()));
    std::env::set_var(
        "SKIFF_SERVER_PUBLIC_KEY",
        hex::encode(signer.verifying_key().to_bytes()),
    );
    let job = api::job::JobOptions {
        state: fx.state.clone(),
        catalog: fx.catalog.clone(),
        schema,
        channel: "stable".to_string(),
        staging_out: Some(staging.path().to_path_buf()),
        dist: None,
        swap: false,
        seq: Some(seq),
        prev: None,
        submission: None,
        public_key: None,
    };
    (api::job::run(&job), staging)
}

fn key_id(key: &SigningKey) -> String {
    hex::encode(sha2::Sha256::digest(key.verifying_key().to_bytes()))
}

/// 提交 → 审核 → 发布到 seq 1，返回 `sid`。
fn published_submission(fx: &Fixture) -> String {
    let created = submit(fx, "com.example.demo", "1.0.0");
    assert_eq!(created.status, 201, "{:?}", created.json());
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 202, "{:?}", approve.json());
    publish(fx, 1);
    sid
}

#[test]
fn yank_requires_reviewer_is_idempotent_and_404s_unknown_versions() {
    let fx = Fixture::new();
    published_submission(&fx);

    let denied = signed_call(
        fx.addr,
        &fx.author,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        &[("Content-Type", "application/json")],
        br#"{"reason":"author cannot"}"#,
    );
    assert_eq!(denied.status, 403, "{:?}", denied.json());

    let unknown = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/9.9.9/yank",
        &[("Content-Type", "application/json")],
        br#"{"reason":"nope"}"#,
    );
    assert_eq!(unknown.status, 404, "{:?}", unknown.json());

    let body = br#"{"reason":"malicious behavior"}"#;
    let first = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        &[("Content-Type", "application/json")],
        body,
    );
    assert_eq!(first.status, 202, "{:?}", first.json());
    let id = first.json()["id"].as_str().unwrap().to_string();
    assert_eq!(first.json()["kind"], "yank");
    assert_eq!(first.json()["state"], "pending");

    let second = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        &[("Content-Type", "application/json")],
        body,
    );
    assert_eq!(second.status, 200, "{:?}", second.json());
    assert_eq!(second.json()["id"], id, "重复 yank 必须返回同一记录");
}

#[test]
fn yank_release_marks_index_and_submission_and_is_not_repeatable() {
    let fx = Fixture::new();
    let sid = published_submission(&fx);

    let yank = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        "/api/v1/plugins/com.example.demo/versions/1.0.0/yank",
        &[("Content-Type", "application/json")],
        br#"{"reason":"policy violation"}"#,
    );
    assert_eq!(yank.status, 202, "{:?}", yank.json());

    let (report, staging) = publish(&fx, 2);
    assert_eq!(report.seq, 2);
    assert_eq!(report.takedowns.len(), 1);
    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["seq"], 2);
    assert_eq!(index["plugins"][0]["yanked"], true, "{index}");
    assert_eq!(
        index["plugins"][0]["yank_reason"], "policy violation",
        "撤包原因必须随条目进入索引: {index}"
    );
    assert_eq!(index["revoked"].as_array().unwrap().len(), 0);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["seq"], 2);

    // 作者视角的站内状态：提交记录带撤包原因，状态仍是 published。
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.state.join("submissions").join(format!("{sid}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["state"], "published");
    assert_eq!(record["takedown"]["kind"], "yank");
    assert_eq!(record["takedown"]["reason"], "policy violation");

    // 没有新的撤包/提交：再跑一次必须失败且不产生新 seq 的产物。
    let schema = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/skiff-server/schema");
    let staging2 = tempfile::tempdir_in(fx.dir.path()).unwrap();
    let job = api::job::JobOptions {
        state: fx.state.clone(),
        catalog: fx.catalog.clone(),
        schema,
        channel: "stable".to_string(),
        staging_out: Some(staging2.path().to_path_buf()),
        dist: None,
        swap: false,
        seq: Some(3),
        prev: None,
        submission: None,
        public_key: None,
    };
    assert!(api::job::run(&job).is_err(), "无待发布项时不得空转递增 seq");
}

#[test]
fn revoked_key_blocks_submit_and_review_and_revokes_history() {
    let fx = Fixture::new();
    published_submission(&fx);
    let author_key = key_id(&fx.author);

    let unknown = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/keys/{}/revoke", "f".repeat(64)),
        &[("Content-Type", "application/json")],
        br#"{"reason":"unknown"}"#,
    );
    assert_eq!(unknown.status, 404, "{:?}", unknown.json());

    let body = br#"{"reason":"private key leaked"}"#;
    let first = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/keys/{author_key}/revoke"),
        &[("Content-Type", "application/json")],
        body,
    );
    assert_eq!(first.status, 202, "{:?}", first.json());
    let second = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/keys/{author_key}/revoke"),
        &[("Content-Type", "application/json")],
        body,
    );
    assert_eq!(second.status, 200, "{:?}", second.json());

    // 该 key 立即失效：提交与查询都不再放行。
    let submit_again = submit(&fx, "com.example.demo", "1.1.0");
    assert_eq!(submit_again.status, 403, "{:?}", submit_again.json());

    // 历史版本进入待发布：一次 release 同时写 yanked 与 revoked[]。
    let (report, staging) = publish(&fx, 2);
    assert_eq!(report.seq, 2);
    assert_eq!(report.takedowns.len(), 1);
    let index: serde_json::Value = serde_json::from_slice(
        &fs::read(staging.path().join("channels/stable/index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["plugins"][0]["yanked"], true, "{index}");
    assert_eq!(index["plugins"][0]["yank_reason"], "private key leaked");
    assert_eq!(index["revoked"][0]["id"], "com.example.demo");
    assert_eq!(index["revoked"][0]["version"], "1.0.0");
    assert_eq!(index["revoked"][0]["reason"], "private key leaked");
    let revoked: serde_json::Value =
        serde_json::from_slice(&fs::read(fx.catalog.join("stable/revoked.json")).unwrap()).unwrap();
    assert_eq!(revoked[0]["id"], "com.example.demo");

    // 审计可按 key_id 追到该作者的提交链路，按 sid 追到以该 key 为对象的吊销动作。
    let by_key = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        &format!("/api/v1/audit?key_id={author_key}"),
        &[],
        b"",
    );
    assert_eq!(by_key.status, 200, "{:?}", by_key.json());
    let actions: Vec<String> = by_key.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(actions.iter().any(|a| a == "submit"), "{actions:?}");

    let by_sid = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        &format!("/api/v1/audit?sid={author_key}"),
        &[],
        b"",
    );
    assert_eq!(by_sid.status, 200, "{:?}", by_sid.json());
    assert_eq!(by_sid.json()["entries"][0]["action"], "revoke");
    assert!(by_sid.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["action"] == "revoke" && e["result"] == "ok"));
}

#[test]
fn audit_sid_chains_submit_approve_and_release() {
    let fx = Fixture::new();
    let sid = published_submission(&fx);

    let response = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        &format!("/api/v1/audit?sid={sid}"),
        &[],
        b"",
    );
    assert_eq!(response.status, 200, "{:?}", response.json());
    let entries = response.json()["entries"].as_array().unwrap().clone();
    let actions: Vec<String> = entries
        .iter()
        .map(|e| e["action"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["submit", "approve", "release"] {
        assert!(
            actions.iter().any(|a| a == expected),
            "按 sid 查询必须能串起 {expected}: {actions:?}"
        );
    }
    let release = entries
        .iter()
        .find(|e| e["action"] == "release")
        .expect("release 行必须可查");
    assert!(
        release["submission_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == &serde_json::Value::String(sid.clone())),
        "release 行必须暴露 submission_ids: {release}"
    );
    for forbidden in ["signature", "package", "body", "sha256"] {
        assert!(release.get(forbidden).is_none(), "{release}");
    }
}

#[test]
fn revoked_reviewer_key_cannot_approve() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    let reviewer_key = key_id(&fx.reviewer);

    let revoke = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/keys/{reviewer_key}/revoke"),
        &[("Content-Type", "application/json")],
        br#"{"reason":"laptop lost"}"#,
    );
    assert_eq!(revoke.status, 202, "{:?}", revoke.json());

    let approve = signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );
    assert_eq!(approve.status, 403, "{:?}", approve.json());
}

#[test]
fn audit_query_is_scoped_and_paginated_without_sensitive_fields() {
    let fx = Fixture::new();
    let created = submit(&fx, "com.example.demo", "1.0.0");
    let sid = created.json()["sid"].as_str().unwrap().to_string();
    signed_call(
        fx.addr,
        &fx.reviewer,
        "POST",
        &format!("/api/v1/submissions/{sid}/approve"),
        &[],
        b"",
    );

    let by_sid = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        &format!("/api/v1/audit?sid={sid}"),
        &[],
        b"",
    );
    assert_eq!(by_sid.status, 200, "{:?}", by_sid.json());
    let entries = by_sid.json()["entries"].as_array().unwrap().clone();
    assert!(entries.len() >= 2, "{entries:?}");
    for entry in &entries {
        assert_eq!(entry["submission_id"], sid);
        for forbidden in ["signature", "package", "body", "sha256"] {
            assert!(
                entry.get(forbidden).is_none(),
                "审计不得返回 {forbidden}: {entry}"
            );
        }
    }
    assert!(entries.iter().any(|e| e["action"] == "submit"));
    assert!(entries.iter().any(|e| e["action"] == "approve"));

    // 分页：limit=1 时 next_offset 指向第二条。
    let page = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        &format!("/api/v1/audit?sid={sid}&limit=1"),
        &[],
        b"",
    );
    assert_eq!(page.json()["entries"].as_array().unwrap().len(), 1);
    assert!(page.json()["total"].as_u64().unwrap() >= 2);
    assert_eq!(page.json()["next_offset"], 1);

    // 作者默认只看自己；显式查他人一律 403。
    let own = signed_call(fx.addr, &fx.author, "GET", "/api/v1/audit", &[], b"");
    assert_eq!(own.status, 200);
    let own_key = key_id(&fx.author);
    assert!(own.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["key_id"] == own_key));

    let other = signed_call(
        fx.addr,
        &fx.author,
        "GET",
        &format!("/api/v1/audit?key_id={}", key_id(&fx.reviewer)),
        &[],
        b"",
    );
    assert_eq!(other.status, 403, "{:?}", other.json());

    let bad = signed_call(
        fx.addr,
        &fx.reviewer,
        "GET",
        "/api/v1/audit?limit=abc",
        &[],
        b"",
    );
    assert_eq!(bad.status, 400, "{:?}", bad.json());
}

fn prepare_catalog(catalog: &Path) {
    let channel = catalog.join("stable");
    for sub in ["plugins", "apps", "runtimes", "upgrades"] {
        fs::create_dir_all(channel.join(sub)).unwrap();
    }
    fs::create_dir_all(catalog.join("assets")).unwrap();
    fs::write(
        channel.join("channel.json"),
        br#"{
  "schema": "skiff.server.channel/v1",
  "channel": "stable",
  "seq": 1,
  "generated_at": "2026-11-09T12:00:00Z"
}
"#,
    )
    .unwrap();
}
