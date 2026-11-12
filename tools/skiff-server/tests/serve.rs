//! `serve` 的端到端测试：真实 TCP 上的 HTTP 语义、CLI 启动差异与自签 CA 的 https 链路。

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use skiff_server::serve;

/// 子进程从启动到开始监听的等待上限。
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// 端口被其它进程抢走时重新分配端口的次数。
const SPAWN_ATTEMPTS: usize = 3;

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
        let _ = serve::serve(listener, &root, tls, None);
    });
    port
}

/// 发一个原始请求，返回（响应头，响应体）。连接/读写错误原样返回，由调用方决定
/// 是重试还是带上子进程 stderr 断言失败。
fn request(port: u16, raw: &str) -> io::Result<(String, Vec<u8>)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(raw.as_bytes())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "响应缺少头结束符"))?
        + 4;
    Ok((
        String::from_utf8_lossy(&buf[..split]).to_string(),
        buf[split..].to_vec(),
    ))
}

/// 端口分配 + 子进程启动 + 就绪探测必须串行。并行测试若在「释放临时端口 → 子进程绑定」
/// 的窗口里抢到同一个端口，会连到别人的子进程上，表现为 `ConnectionReset` / `ConnectionRefused`。
fn spawn_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 让系统分配一个空闲端口并读回来；调用方必须持有 [`spawn_lock`]。
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

enum ReadyError {
    /// 子进程已退出（启动失败），带退出状态。
    Exited(String),
    /// 超时仍未监听。
    Timeout,
}

/// 轮询连接直到成功或超时。`ConnectionReset` 等半开状态也视为未就绪继续重试；
/// 子进程提前退出立即返回失败，避免把「还没起来」和「起不来」混在一起。
fn wait_until_listening(
    child: &mut Child,
    port: u16,
    stderr_path: &Path,
) -> Result<(), ReadyError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut delay = Duration::from_millis(10);
    loop {
        if let Some(status) = child.try_wait().unwrap_or(None) {
            return Err(ReadyError::Exited(status.to_string()));
        }
        match TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
            Ok(stream) => {
                drop(stream);
                // 端口可能被其它进程先占用：连接成功但本进程已退出，仍算启动失败。
                // 给 bind 失败留出一个调度窗口再判定。
                std::thread::sleep(Duration::from_millis(30));
                if let Some(status) = child.try_wait().unwrap_or(None) {
                    return Err(ReadyError::Exited(status.to_string()));
                }
                return Ok(());
            }
            Err(error) => {
                // 未就绪的常见形态：还没有监听者（Refused）或半开（Reset/Aborted）。
                if !matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) {
                    return Err(ReadyError::Exited(format!(
                        "连接探测异常: {error}（stderr 见 {}",
                        stderr_path.display()
                    )));
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(ReadyError::Timeout);
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_millis(200));
    }
}

/// 用 CLI 起一个真实的 `serve --http` 子进程，并完成就绪等待。
///
/// stderr 落到临时文件：既用于启动失败时的断言信息，也用于「访问日志写 stderr」类用例。
struct Server {
    child: Child,
    port: u16,
    stderr: tempfile::NamedTempFile,
}

impl Server {
    fn stderr_text(&self) -> String {
        std::fs::read_to_string(self.stderr.path()).unwrap_or_default()
    }

    fn request(&mut self, raw: &str) -> (String, Vec<u8>) {
        match request(self.port, raw) {
            Ok(response) => response,
            Err(error) => {
                let status = self.child.try_wait().unwrap_or(None);
                panic!(
                    "请求失败: {error}\n子进程状态: {status:?}\nstderr:\n{}",
                    self.stderr_text()
                );
            }
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 起 `serve --http --listen 127.0.0.1:<系统分配>`，`extra` 追加其余 CLI 参数。
///
/// 整个「选端口 → 启动 → 就绪」过程持有 [`spawn_lock`]；端口绑定失败（被外部进程抢占）
/// 时换新端口重试，其它启动失败直接 panic 并带出 stderr。
fn spawn_http(root: &Path, extra: &[&str]) -> Server {
    let _guard = spawn_lock()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut last_stderr = String::new();
    for _ in 0..SPAWN_ATTEMPTS {
        let port = free_port();
        let stderr = tempfile::NamedTempFile::new().unwrap();
        let stderr_path = stderr.path().to_path_buf();
        let mut command = Command::new(env!("CARGO_BIN_EXE_skiff-server"));
        command
            .arg("serve")
            .arg("--dist")
            .arg(root)
            .arg("--http")
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .args(extra)
            .stdout(Stdio::null())
            .stderr(stderr.reopen().unwrap());
        let mut child = command.spawn().unwrap();

        match wait_until_listening(&mut child, port, &stderr_path) {
            Ok(()) => {
                return Server {
                    child,
                    port,
                    stderr,
                };
            }
            Err(ReadyError::Exited(status)) => {
                let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                let _ = child.wait();
                // 只有「监听失败」值得换端口重试；其它启动错误直接暴露。
                if log.contains("监听") && log.contains("失败") {
                    last_stderr = log;
                    continue;
                }
                panic!("serve 子进程启动失败（{status}）:\n{log}");
            }
            Err(ReadyError::Timeout) => {
                let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "serve 未在 {} 秒内开始监听；stderr:\n{log}",
                    READY_TIMEOUT.as_secs()
                );
            }
        }
    }
    panic!("连续 {SPAWN_ATTEMPTS} 次端口绑定失败（端口被其它进程占用）：\n{last_stderr}");
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
    )
    .unwrap();
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
    )
    .unwrap();
    assert!(head.starts_with("HTTP/1.1 304 Not Modified"), "{head}");
    assert!(body.is_empty());
    assert!(header_value(&head, "content-length").is_none());

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\nRange: bytes=0-3\r\n\r\n",
    )
    .unwrap();
    assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
    assert_eq!(body, b"{\"se");

    let (head, body) = request(
        port,
        "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\nRange: bytes=9999-\r\n\r\n",
    )
    .unwrap();
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
    )
    .unwrap();
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
    )
    .unwrap();
    assert!(head.starts_with("HTTP/1.1 405"), "{head}");
    assert_eq!(header_value(&head, "allow"), Some("GET, HEAD"));
    assert_eq!(body, b"method not allowed");

    let (head, body) = request(port, "GET /api/v1/publish HTTP/1.1\r\nHost: test\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    assert_eq!(body, b"not found");

    let (head, _) = request(
        port,
        "GET /channels/stable/../../../etc/passwd HTTP/1.1\r\nHost: test\r\n\r\n",
    )
    .unwrap();
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
    let (_dir, root) = dist();
    let mut server = spawn_http(&root, &[]);
    let (head, body) =
        server.request("GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
    assert_eq!(body, MANIFEST);

    server.stop();
    let log = server.stderr_text();
    assert!(log.contains("仅测试用"), "{log}");
}

#[test]
fn cli_access_log_file_gets_one_line_per_request() {
    let (dir, root) = dist();
    let log_path = dir.path().join("access.log");
    let mut server = spawn_http(&root, &["--access-log", log_path.to_str().unwrap()]);

    let (head, _) =
        server.request("GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let (head, _) =
        server.request("POST /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 405"), "{head}");

    server.stop();

    let log = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(
        lines[0].contains("GET /channels/stable/manifest.json status=200 bytes=9 cache=none"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].contains("POST /channels/stable/manifest.json status=405 bytes=18"),
        "{}",
        lines[1]
    );
}

#[test]
fn cli_bare_access_log_writes_stderr() {
    let (_dir, root) = dist();
    let mut server = spawn_http(&root, &["--access-log"]);

    let (head, _) =
        server.request("GET /channels/stable/manifest.json HTTP/1.1\r\nHost: test\r\n\r\n");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    server.stop();

    // 启动信息在 stdout（此处已丢弃）；访问日志行必须落在 stderr。
    let log = server.stderr_text();
    assert!(
        log.contains("GET /channels/stable/manifest.json status=200 bytes=9"),
        "{log}"
    );
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
