//! 共享 HTTP 传输件：请求头解析、响应写出、TLS 配置。
//!
//! `serve`（静态只读，§16）与 `api`（发布面，§17）共用；这里只负责「读一个请求 /
//! 写一个响应」，不含路径白名单、条件请求或任何业务语义，避免两套协议实现分叉。

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ureq::rustls;

/// 请求头上限：超过直接断开（请求体另由调用方按 Content-Length 读取）。
pub const MAX_HEAD_BYTES: usize = 16 * 1024;
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    pub fn new(method: &str, target: &str) -> Self {
        Self {
            method: method.to_string(),
            target: target.to_string(),
            headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .push((name.to_ascii_lowercase(), value.to_string()));
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// 响应体：小 JSON 直接给字节，blob 用文件区间流式输出。
#[derive(Debug)]
pub enum Body {
    Empty,
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        offset: u64,
        len: u64,
    },
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 读一个请求头，返回请求与「分隔符之后已读到的剩余字节」。
///
/// 返回的剩余字节可能为空，也可能已包含请求体开头（`serve` 忽略它，`api` 接着读）。
pub fn read_head<S: Read>(stream: &mut S) -> Option<(Request, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(pos) = head_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return None;
        }
    };
    if end > MAX_HEAD_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..end]);
    let mut lines = text.split("\r\n");
    let mut parts = lines.next()?.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let _version = parts.next()?;
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Some((
        Request {
            method,
            target,
            headers,
        },
        buf[end + 4..].to_vec(),
    ))
}

/// 写完整响应，返回实际写出的响应体字节数（HEAD 恒为 0），供访问日志使用。
pub fn write_response<S: Write>(
    stream: &mut S,
    response: &Response,
    head_only: bool,
) -> io::Result<u64> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    if head_only {
        stream.flush()?;
        return Ok(0);
    }
    let written = match &response.body {
        Body::Empty => 0,
        Body::Bytes(bytes) => {
            stream.write_all(bytes)?;
            bytes.len() as u64
        }
        Body::File { path, offset, len } => {
            let mut file = fs::File::open(path)?;
            file.seek(SeekFrom::Start(*offset))?;
            io::copy(&mut file.take(*len), stream)?
        }
    };
    stream.flush()?;
    Ok(written)
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        416 => "Range Not Satisfiable",
        422 => "Unprocessable Entity",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

/// 加载 PEM 证书与私钥，构造 rustls 服务端配置。格式非法/文件缺失返回可读错误。
pub fn tls_config(cert: &Path, key: &Path) -> Result<Arc<rustls::ServerConfig>> {
    let certs = read_pem_certs(cert)?;
    let private_key = read_pem_key(key)?;
    let config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .context("构造 TLS 配置失败")?
    .with_no_client_auth()
    .with_single_cert(certs, private_key)
    .with_context(|| format!("证书与私钥不匹配：{} / {}", cert.display(), key.display()))?;
    Ok(Arc::new(config))
}

fn read_pem_certs(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let bytes = fs::read(path).with_context(|| format!("读取证书失败: {}", path.display()))?;
    let certs = rustls_pemfile::certs(&mut &bytes[..])
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("解析证书 PEM 失败: {}", path.display()))?;
    if certs.is_empty() {
        bail!("证书文件不含任何 PEM 证书: {}", path.display());
    }
    Ok(certs)
}

fn read_pem_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let bytes = fs::read(path).with_context(|| format!("读取私钥失败: {}", path.display()))?;
    let key = rustls_pemfile::private_key(&mut &bytes[..])
        .with_context(|| format!("解析私钥 PEM 失败: {}", path.display()))?
        .ok_or_else(|| anyhow::anyhow!("私钥文件不含任何支持的 PEM 私钥: {}", path.display()))?;
    Ok(key)
}
