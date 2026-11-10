//! `serve` 的端到端测试：真实 TCP 上的 HTTP 语义、CLI 启动差异与自签 CA 的 https 链路。

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use skiff_server::serve;

const BLOB_HEX: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const MANIFEST: &[u8] = br#"{"seq":7}"#;

fn dist() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let channel = root.join("channels").join("stable");
    std::fs::create_dir_all(&channel).unwrap();
    std::fs::write(channel.join("manifest.json"), MANIFEST).unwrap();
    std::fs::write(channel.join("manifest.json.sig"), br#"{"sig":"x"}"#).unwrap();
    std::fs::write(channel.join("index.json"), br#"{"seq":7}"#).unwrap();
    let blobs = root.join("blobs").join("sha256");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(BLOB_HEX), b"hello blob").unwrap();
    (dir, root)
}

fn start(root: PathBuf, tls: Option<Arc<ureq::rustls::ServerConfig>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let _ = serve::serve(listener, &root, tls);
    });
    port
}

/// 发一个原始请求，返回（响应头，响应体）。
fn request(port: u16, raw: &str) -> (String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("响应缺少头结束符")
        + 4;
    (
        String::from_utf8_lossy(&buf[..split]).to_string(),
        buf[split..].to_vec(),
    )
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        })
        .map(|(_, v)| v.trim())
}

#[test]
fn http_serves_conditional_and_range_requests() {
    let (_dir, root) = dist();
    let port = start(root, None);

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(body, MANIFEST);
    assert_eq!(
        header_value(&head, "content-type"),
        Some("application/json; charset=utf-8")
    );
    let etag = header_value(&head, "etag").unwrap().to_string();

    let (head, body) = request(
        port,
        &format!(
            "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\nIf-None-Match: {etag}\r\n\r\n"
        ),
    );
    assert!(head.starts_with("HTTP/1.1 304 Not Modified"), "{head}");
    assert!(body.is_empty());
    assert!(header_value(&head, "content-length").is_none());

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\nRange: bytes=0-3\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
    assert_eq!(body, b"{\"se");

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\nRange: bytes=9999-\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 416"), "{head}");
    let expected = format!("bytes */{}", MANIFEST.len());
    assert_eq!(
        header_value(&head, "content-range"),
        Some(expected.as_str())
    );
    assert!(!body.is_empty());
}

#[test]
fn http_head_is_bodyless_and_errors_are_plain() {
    let (_dir, root) = dist();
    let port = start(root, None);

    let (head, body) = request(
        port,
        "HEAD /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    let expected_len = MANIFEST.len().to_string();
    assert_eq!(
        header_value(&head, "content-length"),
        Some(expected_len.as_str())
    );
    assert!(body.is_empty());

    let (head, body) = request(
        port,
        "POST /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 405"), "{head}");
    assert_eq!(header_value(&head, "allow"), Some("GET, HEAD"));
    assert_eq!(body, b"method not allowed");

    let (head, body) = request(port, "GET /api/v1/publish HTTP/1.1\r\nHost: test\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    assert_eq!(body, b"not found");

    let (head, _) = request(
        port,
        "GET /channels/stable/../../../etc/passwd HTTP/1.1\r\nHost: test\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
}

#[test]
fn cli_defaults_to_https_and_refuses_to_start_without_certs() {
    let (_dir, root) = dist();
    let output = Command::new(env!("CARGO_BIN_EXE_skiff-server"))
        .arg("serve")
        .arg("--dist")
        .arg(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("https"), "{stderr}");
    assert!(stderr.contains("--tls-cert"), "{stderr}");

    let output = Command::new(env!("CARGO_BIN_EXE_skiff-server"))
        .arg("serve")
        .arg("--dist")
        .arg(&root)
        .arg("--http")
        .arg("--tls-cert")
        .arg("server.pem")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("互斥"));
}

#[test]
fn cli_http_mode_warns_and_serves() {
    let (dir, root) = dist();
    // 先占一个空闲端口再释放，交给子进程绑定。
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stderr_path = dir.path().join("stderr.log");
    let stderr = std::fs::File::create(&stderr_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_skiff-server"))
        .arg("serve")
        .arg("--dist")
        .arg(&root)
        .arg("--http")
        .arg("--listen")
        .arg(format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();

    let mut ready = false;
    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready, "serve --http 未在预期时间内开始监听");

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n",
    );
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(body, MANIFEST);

    let _ = child.kill();
    let _ = child.wait();
    let log = std::fs::read_to_string(&stderr_path).unwrap();
    assert!(log.contains("仅测试用"), "{log}");
}

fn pem_pair(dir: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "skiff-serve-test-ca");
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let ca = rcgen::Certificate::from_params(ca_params).unwrap();

    let mut server_params = rcgen::CertificateParams::default();
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "127.0.0.1");
    server_params.subject_alt_names =
        vec![rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))];
    server_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let server = rcgen::Certificate::from_params(server_params).unwrap();

    let cert = dir.join("server.pem");
    let key = dir.join("server.key");
    std::fs::write(&cert, server.serialize_pem_with_signer(&ca).unwrap()).unwrap();
    std::fs::write(&key, server.serialize_private_key_pem()).unwrap();
    (cert, key, ca.serialize_der().unwrap())
}

#[test]
fn https_works_only_after_importing_the_self_signed_ca() {
    let (dir, root) = dist();
    let (cert, key, ca_der) = pem_pair(dir.path());
    let tls = serve::tls_config(&cert, &key).unwrap();
    let port = start(root, Some(tls));
    let url = format!("https://127.0.0.1:{port}/channels/stable/manifest.json");

    // 默认根不认识自签 CA：必须失败。
    let untrusted = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .get(&url)
        .call();
    assert!(untrusted.is_err(), "未导入 CA 时不应建连成功");

    let mut roots = ureq::rustls::RootCertStore::empty();
    roots
        .add(ureq::rustls::pki_types::CertificateDer::from(ca_der))
        .unwrap();
    let config = ureq::rustls::ClientConfig::builder_with_provider(
        ureq::rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .tls_config(Arc::new(config))
        .build();
    let response = agent.get(&url).call().unwrap();
    assert_eq!(response.status(), 200);
}
