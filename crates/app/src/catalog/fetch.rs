//! 目录拉取的传输层：`Transport` 只描述「带条件头的 GET」，真实实现走 ureq，
//! 单测注入假实现，绝不触网。

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::CertificateDer;
use ureq::rustls;

/// 响应体上限：目录 JSON 只有几十 KB，超过即视为异常源。
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// 插件包 blob 的下载上限：与安装侧的 `DEFAULT_LIMITS.archive_bytes`（100MiB）同量级。
pub const MAX_BLOB_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Conditional<'a> {
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub trait Transport: Send + Sync {
    fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String>;

    /// 二进制 blob 下载（安装链路用）。目录 JSON 的 8MiB 上限装不下插件包，
    /// 因此单独放宽；假传输层无需覆写，默认复用 `get`。
    fn get_blob(&self, url: &str) -> Result<HttpReply, String> {
        self.get(url, &Conditional::default())
    }
}

/// 真实实现：同步阻塞，调用方必须放在 `spawn_blocking`。
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    /// `extra_roots` 为空时与历史行为完全一致：只用 ureq 内置（webpki）根。
    ///
    /// 追加自定义根**不放松校验**：仍是 https、仍做完整证书链与主机名校验，
    /// 只是多信任若干个 CA；没有任何 http 或 insecure 开关。
    pub fn new(timeout: Duration, extra_roots: &[CertificateDer<'static>]) -> Self {
        let mut builder = ureq::AgentBuilder::new().timeout(timeout).redirects(3);
        if !extra_roots.is_empty() {
            // 配置加载阶段已用同一个 `root_store` 校验过，这里不会失败。
            let roots = root_store(extra_roots).expect("CA 根已在配置加载阶段校验");
            let config = rustls::ClientConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .expect("ring 提供者支持默认 TLS 版本")
            .with_root_certificates(roots)
            .with_no_client_auth();
            builder = builder.tls_config(Arc::new(config));
        }
        Self {
            agent: builder.build(),
        }
    }
}

/// 读取自定义 CA 文件（PEM）。空文件、无 PEM 证书、无法作为信任锚都返回可读错误，
/// 由配置层拒绝启动；**不做静默回退**到内置根。
pub fn load_ca_bundle(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("读取 CA 文件 {} 失败: {e}", path.display()))?;
    let certs = rustls_pemfile::certs(&mut &bytes[..])
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("解析 CA 文件 {} 失败: {e}", path.display()))?;
    if certs.is_empty() {
        return Err(format!("CA 文件 {} 不含任何 PEM 证书", path.display()));
    }
    // 提前验证每张证书都能作为信任锚，避免到建连时才报错。
    root_store(&certs).map_err(|e| format!("CA 文件 {} 不是可用信任锚: {e}", path.display()))?;
    Ok(certs)
}

/// 内置根 + 额外根。两者并存：导入自签 CA 不会丢掉公网根。
fn root_store(extra: &[CertificateDer<'static>]) -> Result<rustls::RootCertStore, rustls::Error> {
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    for cert in extra {
        roots.add(cert.clone())?;
    }
    Ok(roots)
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String> {
        let mut req = self.agent.get(url);
        if let Some(etag) = cond.etag {
            req = req.set("If-None-Match", etag);
        }
        if let Some(last) = cond.last_modified {
            req = req.set("If-Modified-Since", last);
        }
        match req.call() {
            Ok(resp) => Ok(read_reply(resp, MAX_BODY_BYTES)),
            // 条件请求的 304 与 4xx/5xx 都从 Status 分支返回，交给上层判定。
            Err(ureq::Error::Status(code, resp)) => {
                let mut reply = read_reply(resp, MAX_BODY_BYTES);
                reply.status = code;
                Ok(reply)
            }
            Err(ureq::Error::Transport(e)) => Err(e.to_string()),
        }
    }

    fn get_blob(&self, url: &str) -> Result<HttpReply, String> {
        match self.agent.get(url).call() {
            Ok(resp) => Ok(read_reply(resp, MAX_BLOB_BYTES)),
            Err(ureq::Error::Status(code, resp)) => {
                let mut reply = read_reply(resp, MAX_BLOB_BYTES);
                reply.status = code;
                Ok(reply)
            }
            Err(ureq::Error::Transport(e)) => Err(e.to_string()),
        }
    }
}

fn read_reply(resp: ureq::Response, limit: u64) -> HttpReply {
    let status = resp.status();
    let header = |name: &str| resp.header(name).map(str::to_string);
    let (etag, last_modified) = (header("etag"), header("last-modified"));
    let mut body = Vec::new();
    // 读失败按空体处理：上层会用摘要/大小校验兜底，不会把残缺内容当有效清单。
    let _ = resp.into_reader().take(limit).read_to_end(&mut body);
    HttpReply {
        status,
        body,
        etag,
        last_modified,
    }
}

/// 供 `AppState` 持有的可替换句柄。
pub type SharedTransport = Arc<dyn Transport>;

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeTransport {
        replies: Mutex<HashMap<String, Vec<HttpReply>>>,
        pub calls: Mutex<Vec<(String, ConditionalOwned)>>,
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ConditionalOwned {
        pub etag: Option<String>,
        pub last_modified: Option<String>,
    }

    impl FakeTransport {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// 为同一 URL 排队多个响应；用完后重复最后一个。
        pub fn push(&self, url: &str, reply: HttpReply) {
            self.replies
                .lock()
                .unwrap()
                .entry(url.to_string())
                .or_default()
                .push(reply);
        }

        pub fn replies_for(&self, url: &str) -> usize {
            self.replies
                .lock()
                .unwrap()
                .get(url)
                .map(Vec::len)
                .unwrap_or(0)
        }

        pub fn calls(&self) -> Vec<(String, ConditionalOwned)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String> {
            self.calls.lock().unwrap().push((
                url.to_string(),
                ConditionalOwned {
                    etag: cond.etag.map(str::to_string),
                    last_modified: cond.last_modified.map(str::to_string),
                },
            ));
            let mut map = self.replies.lock().unwrap();
            let queue = map
                .get_mut(url)
                .ok_or_else(|| format!("假传输层没有该地址: {url}"))?;
            if queue.is_empty() {
                return Err(format!("假传输层响应队列为空: {url}"));
            }
            // 按入队顺序消费；测试需要几次响应就按顺序 push 几次。
            Ok(queue.remove(0))
        }
    }
}

/// 自定义 CA 的端到端校验：进程内生成「自签 CA → 服务器证书」，起一个最小 TLS
/// 服务端，用真实 `UreqTransport` 建连；不触网、不依赖系统证书。
#[cfg(test)]
mod tls_tests {
    use super::*;
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr, TcpListener};
    use std::time::Duration;
    use ureq::rustls;

    fn test_chain() -> (
        String,
        CertificateDer<'static>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ) {
        let mut ca_params = rcgen::CertificateParams::default();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "skiff-test-ca");
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
        // rustls 的服务端校验要求 serverAuth EKU，缺了会在「已信任 CA」后仍被拒。
        server_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        let server = rcgen::Certificate::from_params(server_params).unwrap();

        let ca_pem = ca.serialize_pem().unwrap();
        let cert = CertificateDer::from(server.serialize_der_with_signer(&ca).unwrap());
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(server.serialize_private_key_der()),
        );
        (ca_pem, cert, key)
    }

    /// 最小 TLS 服务端：接受一次连接、读一个请求、回 200 `ok` 后关闭。
    fn serve_once(
        cert: CertificateDer<'static>,
        key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let config = rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let Ok((tcp, _)) = listener.accept() else {
                return;
            };
            let Ok(conn) = rustls::ServerConnection::new(Arc::new(config)) else {
                return;
            };
            let mut stream = rustls::StreamOwned::new(conn, tcp);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = b"ok";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
            let _ = stream.flush();
        });
        (port, handle)
    }

    /// 未导入 CA：内置根不认识自签 CA，必须被拒（不放松校验）。
    #[test]
    fn self_signed_ca_is_rejected_without_bundle() {
        let (_ca_pem, cert, key) = test_chain();
        let (port, handle) = serve_once(cert, key);
        let err = UreqTransport::new(Duration::from_secs(5), &[])
            .get(
                &format!("https://127.0.0.1:{port}/manifest.json"),
                &Conditional::default(),
            )
            .expect_err("未导入自签 CA 时必须拒绝");
        let _ = handle.join();
        let lower = err.to_lowercase();
        assert!(
            lower.contains("certificate")
                || lower.contains("signature")
                || lower.contains("tls")
                || lower.contains("handshake"),
            "拒绝原因应可辨认: {err}"
        );
    }

    /// 导入 CA 后同一服务端成功；仍走 https（这里是 https URL）。
    #[test]
    fn custom_ca_bundle_is_accepted() {
        let (ca_pem, cert, key) = test_chain();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, ca_pem).unwrap();
        let roots = load_ca_bundle(&path).unwrap();
        assert_eq!(roots.len(), 1);

        let (port, handle) = serve_once(cert, key);
        let reply = UreqTransport::new(Duration::from_secs(5), &roots)
            .get(
                &format!("https://127.0.0.1:{port}/manifest.json"),
                &Conditional::default(),
            )
            .expect("导入 CA 后必须能建连");
        let _ = handle.join();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"ok");
    }

    /// CA 文件不存在 / 不含 PEM / 内容非法：都返回可读错误，不静默回退。
    #[test]
    fn broken_ca_bundle_is_rejected() {
        let dir = tempfile::tempdir().unwrap();

        let missing = dir.path().join("nope.pem");
        let err = load_ca_bundle(&missing).unwrap_err();
        assert!(err.contains("读取"), "{err}");

        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, b"# no certificate here\n").unwrap();
        let err = load_ca_bundle(&empty).unwrap_err();
        assert!(err.contains("不含任何 PEM 证书"), "{err}");

        let garbage = dir.path().join("garbage.pem");
        std::fs::write(
            &garbage,
            "-----BEGIN CERTIFICATE-----\nnot-base64\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        assert!(load_ca_bundle(&garbage).is_err());
    }
}
