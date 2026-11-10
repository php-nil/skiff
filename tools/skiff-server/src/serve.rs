//! `serve`：把已生成的静态产物目录按 [docs/SKIFF_SERVER.md §16] 的契约原样提供。
//!
//! 只读、无写入路由、无目录列表；默认 https（显式 `--http` 才明文）。
//! 契约是规范来源，本文件只是实现：路径白名单 → 条件请求 → 单区间 Range。

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use ureq::rustls;

/// 请求头上限：只接受一个 GET/HEAD，超出直接断开。
const MAX_HEAD_BYTES: usize = 16 * 1024;
/// JSON 允许读入内存的上限；超过按内部错误处理（产物 JSON 只有几十 KB 量级）。
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

const JSON_FILES: [&str; 3] = ["manifest.json", "manifest.json.sig", "index.json"];

/// 白名单命中的资源种类。只有这两种路径会落到文件系统。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Json,
    Blob { hex: String },
}

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

    fn header(&self, name: &str) -> Option<&str> {
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

/// 阻塞式服务循环：每连接一线程，处理一个请求后关闭。
///
/// `root` 在启动时 canonicalize 一次；后续所有包含性检查都基于它。
pub fn serve(
    listener: TcpListener,
    root: &Path,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<()> {
    let root =
        fs::canonicalize(root).with_context(|| format!("--dist 目录不可用: {}", root.display()))?;
    if !root.join("channels").is_dir() || !root.join("blobs").is_dir() {
        bail!(
            "--dist 不是产物目录（缺少 channels/ 或 blobs/）: {}",
            root.display()
        );
    }
    let root = Arc::new(root);
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        let root = Arc::clone(&root);
        let tls = tls.clone();
        std::thread::spawn(move || handle_connection(stream, &root, tls));
    }
    Ok(())
}

fn handle_connection(
    stream: std::net::TcpStream,
    root: &Path,
    tls: Option<Arc<rustls::ServerConfig>>,
) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    match tls {
        Some(config) => match rustls::ServerConnection::new(config) {
            Ok(conn) => {
                let mut stream = rustls::StreamOwned::new(conn, stream);
                handle_http(&mut stream, root);
            }
            Err(error) => tracing_free_log(&format!("TLS 会话建立失败: {error}")),
        },
        None => {
            let mut stream = stream;
            handle_http(&mut stream, root);
        }
    }
}

fn handle_http<S: Read + Write>(stream: &mut S, root: &Path) {
    let Some(request) = read_request(stream) else {
        return;
    };
    let head_only = request.method.eq_ignore_ascii_case("HEAD");
    let response = respond(root, &request);
    if let Err(error) = write_response(stream, &response, head_only) {
        tracing_free_log(&format!("写响应失败: {error}"));
    }
}

fn tracing_free_log(message: &str) {
    eprintln!("[serve] {message}");
}

fn read_request<S: Read>(stream: &mut S) -> Option<Request> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > MAX_HEAD_BYTES {
            break;
        }
    }
    if buf.len() > MAX_HEAD_BYTES {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
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
    Some(Request {
        method,
        target,
        headers,
    })
}

fn write_response<S: Write>(
    stream: &mut S,
    response: &Response,
    head_only: bool,
) -> io::Result<()> {
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
        return stream.flush();
    }
    match &response.body {
        Body::Empty => {}
        Body::Bytes(bytes) => stream.write_all(bytes)?,
        Body::File { path, offset, len } => {
            let mut file = fs::File::open(path)?;
            file.seek(SeekFrom::Start(*offset))?;
            io::copy(&mut file.take(*len), stream)?;
        }
    }
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        304 => "Not Modified",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "Internal Server Error",
    }
}

/// 契约核心：方法 → 路径白名单 → 条件请求 → Range。
pub fn respond(root: &Path, request: &Request) -> Response {
    if !(request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD"))
    {
        return text_response(405, "method not allowed", vec![("Allow", "GET, HEAD")]);
    }
    let head_only = request.method.eq_ignore_ascii_case("HEAD");
    let Some((path, kind)) = resolve(root, &request.target) else {
        return not_found();
    };
    let Ok(canonical) = fs::canonicalize(&path) else {
        return not_found();
    };
    if !canonical.starts_with(root) {
        return not_found();
    }
    let Ok(meta) = fs::metadata(&canonical) else {
        return not_found();
    };
    if !meta.is_file() {
        return not_found();
    }

    let len = meta.len();
    let modified = meta.modified().unwrap_or(UNIX_EPOCH);
    let last_modified = http_date(modified);

    let (cache_control, content_type) = if matches!(&kind, Kind::Blob { .. }) {
        (
            "public, max-age=31536000, immutable",
            "application/octet-stream",
        )
    } else {
        ("no-cache", "application/json; charset=utf-8")
    };

    let (etag, bytes) = match kind {
        Kind::Blob { hex } => (format!("\"sha256:{hex}\""), None),
        Kind::Json => {
            if len > MAX_JSON_BYTES {
                return text_response(500, "internal error", Vec::new());
            }
            let Ok(body) = fs::read(&canonical) else {
                return text_response(500, "internal error", Vec::new());
            };
            (etag_for(&body), Some(body))
        }
    };

    let mut headers = vec![
        ("ETag".to_string(), etag.clone()),
        ("Last-Modified".to_string(), last_modified),
        ("Cache-Control".to_string(), cache_control.to_string()),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
    ];

    if not_modified(request, &etag, modified) {
        return Response {
            status: 304,
            headers,
            body: Body::Empty,
        };
    }

    let range = if head_only {
        RangeOutcome::Ignored
    } else {
        request
            .header("range")
            .map(|value| parse_range(value, len))
            .unwrap_or(RangeOutcome::Ignored)
    };

    match range {
        RangeOutcome::Unsatisfiable => {
            let text = "range not satisfiable";
            headers.push(("Content-Range".to_string(), format!("bytes */{len}")));
            headers.push((
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            ));
            headers.push(("Content-Length".to_string(), text.len().to_string()));
            Response {
                status: 416,
                headers,
                body: Body::Bytes(text.as_bytes().to_vec()),
            }
        }
        RangeOutcome::Satisfiable(start, end) => {
            let count = end - start + 1;
            headers.push((
                "Content-Range".to_string(),
                format!("bytes {start}-{end}/{len}"),
            ));
            headers.push(("Content-Type".to_string(), content_type.to_string()));
            headers.push(("Content-Length".to_string(), count.to_string()));
            let body = match bytes {
                Some(bytes) => Body::Bytes(bytes[start as usize..=end as usize].to_vec()),
                None => Body::File {
                    path: canonical,
                    offset: start,
                    len: count,
                },
            };
            Response {
                status: 206,
                headers,
                body: if head_only { Body::Empty } else { body },
            }
        }
        RangeOutcome::Ignored => {
            headers.push(("Content-Type".to_string(), content_type.to_string()));
            headers.push(("Content-Length".to_string(), len.to_string()));
            let body = match bytes {
                Some(bytes) => Body::Bytes(bytes),
                None => Body::File {
                    path: canonical,
                    offset: 0,
                    len,
                },
            };
            Response {
                status: 200,
                headers,
                body: if head_only { Body::Empty } else { body },
            }
        }
    }
}

fn text_response(status: u16, text: &str, extra: Vec<(&str, &str)>) -> Response {
    let mut headers = vec![
        (
            "Content-Type".to_string(),
            "text/plain; charset=utf-8".to_string(),
        ),
        ("Content-Length".to_string(), text.len().to_string()),
        ("Cache-Control".to_string(), "no-store".to_string()),
    ];
    headers.extend(
        extra
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    Response {
        status,
        headers,
        body: Body::Bytes(text.as_bytes().to_vec()),
    }
}

fn not_found() -> Response {
    text_response(404, "not found", Vec::new())
}

/// 强 ETag：`"sha256:<hex>"`，hex 为实体字节的 SHA-256。
pub fn etag_for(bytes: &[u8]) -> String {
    format!("\"sha256:{}\"", hex::encode(Sha256::digest(bytes)))
}

fn not_modified(request: &Request, etag: &str, modified: SystemTime) -> bool {
    if let Some(inm) = request.header("if-none-match") {
        return inm.split(',').any(|tag| {
            let tag = tag.trim();
            tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag
        });
    }
    if let Some(ims) = request
        .header("if-modified-since")
        .and_then(parse_http_date)
    {
        let mtime = modified
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return mtime <= ims;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeOutcome {
    Ignored,
    Satisfiable(u64, u64),
    Unsatisfiable,
}

/// 单区间 byte range；`Ignored` 表示按 200 返回整个实体（多区间/非 bytes 单位）。
fn parse_range(value: &str, len: u64) -> RangeOutcome {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeOutcome::Ignored;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        return RangeOutcome::Ignored;
    }
    if len == 0 {
        return RangeOutcome::Unsatisfiable;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeOutcome::Unsatisfiable;
    };
    let (start, end) = (start.trim(), end.trim());
    if start.is_empty() {
        // `-n`：最后 n 字节。
        let Ok(suffix) = end.parse::<u64>() else {
            return RangeOutcome::Unsatisfiable;
        };
        if suffix == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        let n = suffix.min(len);
        return RangeOutcome::Satisfiable(len - n, len - 1);
    }
    let Ok(start) = start.parse::<u64>() else {
        return RangeOutcome::Unsatisfiable;
    };
    if start >= len {
        return RangeOutcome::Unsatisfiable;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        match end.parse::<u64>() {
            Ok(end) => end.min(len - 1),
            Err(_) => return RangeOutcome::Unsatisfiable,
        }
    };
    if start > end {
        return RangeOutcome::Unsatisfiable;
    }
    RangeOutcome::Satisfiable(start, end)
}

/// 路径白名单：`channels/<channel>/<三件>`、`<channel>/<三件>`、`blobs/sha256/<hex64>`。
/// 其他一律 `None`（404），包括 `/api/**`、目录与未知文件。
fn resolve(root: &Path, target: &str) -> Option<(PathBuf, Kind)> {
    let target = target.split(['?', '#']).next()?;
    if !target.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(target)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return None;
    }
    let segments: Vec<&str> = decoded.split('/').skip(1).collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return None;
    }
    if segments
        .iter()
        .any(|s| *s == "." || *s == ".." || s.contains(':'))
    {
        return None;
    }
    match segments.as_slice() {
        ["api", ..] => None,
        ["blobs", "sha256", hex] => {
            if !is_blob_hex(hex) {
                return None;
            }
            Some((
                root.join("blobs").join("sha256").join(hex),
                Kind::Blob {
                    hex: hex.to_string(),
                },
            ))
        }
        ["channels", channel, file] => json_path(root, channel, file),
        [channel, file] => {
            if *channel == "channels" || *channel == "blobs" || *channel == "api" {
                return None;
            }
            json_path(root, channel, file)
        }
        _ => None,
    }
}

fn json_path(root: &Path, channel: &str, file: &str) -> Option<(PathBuf, Kind)> {
    if !is_channel_name(channel) || !JSON_FILES.contains(&file) {
        return None;
    }
    let channel_dir = root.join("channels").join(channel);
    if !channel_dir.join("manifest.json").is_file() {
        return None;
    }
    Some((channel_dir.join(file), Kind::Json))
}

fn is_channel_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_blob_hex(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_value(*bytes.get(index + 1)?)?;
            let low = hex_value(*bytes.get(index + 2)?)?;
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// IMF-fixdate：`Sun, 06 Nov 1994 08:49:37 GMT`。
fn http_date(time: SystemTime) -> String {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    // 1970-01-01 是周四，WEEKDAYS 以 Thu 起头。
    let weekday = WEEKDAYS[(days % 7) as usize];
    format!(
        "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        MONTHS[(month - 1) as usize],
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let mp = if month > 2 {
        month as i64 - 3
    } else {
        month as i64 + 9
    };
    let day_of_year = (153 * mp + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_http_date(value: &str) -> Option<u64> {
    let (_weekday, rest) = value.split_once(',')?;
    let mut parts = rest.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month_text = parts.next()?;
    let month = MONTHS.iter().position(|m| *m == month_text)? as u32 + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    if parts.next()? != "GMT" || hour > 23 || minute > 59 || second > 59 || !(1..=31).contains(&day)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOB_HEX: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const MANIFEST: &[u8] = br#"{"seq":7}"#;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let channel = root.join("channels").join("stable");
        fs::create_dir_all(&channel).unwrap();
        fs::write(channel.join("manifest.json"), MANIFEST).unwrap();
        fs::write(channel.join("manifest.json.sig"), br#"{"sig":"x"}"#).unwrap();
        fs::write(channel.join("index.json"), br#"{"seq":7,"plugins":[]}"#).unwrap();
        let blobs = root.join("blobs").join("sha256");
        fs::create_dir_all(&blobs).unwrap();
        fs::write(blobs.join(BLOB_HEX), b"hello blob").unwrap();
        (dir, root)
    }

    fn body_bytes(response: &Response) -> Vec<u8> {
        match &response.body {
            Body::Empty => Vec::new(),
            Body::Bytes(bytes) => bytes.clone(),
            Body::File { path, offset, len } => {
                let mut file = fs::File::open(path).unwrap();
                file.seek(SeekFrom::Start(*offset)).unwrap();
                let mut out = Vec::new();
                file.take(*len).read_to_end(&mut out).unwrap();
                out
            }
        }
    }

    #[test]
    fn json_ok_has_strong_etag_and_json_content_type() {
        let (_dir, root) = fixture();
        let response = respond(
            &root,
            &Request::new("GET", "/channels/stable/manifest.json"),
        );
        assert_eq!(response.status, 200);
        assert_eq!(body_bytes(&response), MANIFEST);
        assert_eq!(response.header("etag"), Some(etag_for(MANIFEST).as_str()));
        assert_eq!(
            response.header("content-type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(response.header("cache-control"), Some("no-cache"));
        let expected_len = MANIFEST.len().to_string();
        assert_eq!(
            response.header("content-length"),
            Some(expected_len.as_str())
        );
    }

    #[test]
    fn short_channel_alias_matches_canonical_path() {
        let (_dir, root) = fixture();
        let canonical = respond(&root, &Request::new("GET", "/channels/stable/index.json"));
        let alias = respond(&root, &Request::new("GET", "/stable/index.json"));
        assert_eq!(alias.status, 200);
        assert_eq!(body_bytes(&alias), body_bytes(&canonical));
    }

    #[test]
    fn matching_etag_returns_304_without_length() {
        let (_dir, root) = fixture();
        let etag = etag_for(MANIFEST);
        let request = Request::new("GET", "/channels/stable/manifest.json")
            .with_header("If-None-Match", &format!("{etag}, \"other\""));
        let response = respond(&root, &request);
        assert_eq!(response.status, 304);
        assert!(matches!(response.body, Body::Empty));
        assert_eq!(response.header("content-length"), None);
        assert!(response.header("etag").is_some());
    }

    #[test]
    fn wildcard_etag_also_returns_304() {
        let (_dir, root) = fixture();
        let request =
            Request::new("GET", "/channels/stable/manifest.json").with_header("If-None-Match", "*");
        assert_eq!(respond(&root, &request).status, 304);
    }

    #[test]
    fn if_modified_since_returns_304_only_without_etag() {
        let (_dir, root) = fixture();
        let modified = fs::metadata(root.join("channels/stable/manifest.json"))
            .unwrap()
            .modified()
            .unwrap();
        let since = http_date(modified);
        let request = Request::new("GET", "/channels/stable/manifest.json")
            .with_header("If-Modified-Since", &since);
        assert_eq!(respond(&root, &request).status, 304);

        // If-None-Match 优先：不匹配时不看 If-Modified-Since。
        let request = request.with_header("If-None-Match", "\"sha256:deadbeef\"");
        assert_eq!(respond(&root, &request).status, 200);
    }

    #[test]
    fn single_range_returns_206_with_content_range() {
        let (_dir, root) = fixture();
        let request =
            Request::new("GET", "/channels/stable/manifest.json").with_header("Range", "bytes=0-3");
        let response = respond(&root, &request);
        assert_eq!(response.status, 206);
        assert_eq!(body_bytes(&response), b"{\"se");
        let expected_range = format!("bytes 0-3/{}", MANIFEST.len());
        assert_eq!(
            response.header("content-range"),
            Some(expected_range.as_str())
        );
        assert_eq!(response.header("content-length"), Some("4"));
    }

    #[test]
    fn suffix_and_open_ended_ranges_are_supported() {
        let (_dir, root) = fixture();
        let suffix = respond(
            &root,
            &Request::new("GET", "/channels/stable/manifest.json").with_header("Range", "bytes=-2"),
        );
        assert_eq!(suffix.status, 206);
        assert_eq!(body_bytes(&suffix), &MANIFEST[MANIFEST.len() - 2..]);

        let open = respond(
            &root,
            &Request::new("GET", "/channels/stable/manifest.json").with_header("Range", "bytes=6-"),
        );
        assert_eq!(open.status, 206);
        assert_eq!(body_bytes(&open), &MANIFEST[6..]);
    }

    #[test]
    fn unsatisfiable_range_returns_416() {
        let (_dir, root) = fixture();
        for value in ["bytes=999-", "bytes=5-1", "bytes=-0", "bytes=abc"] {
            let request =
                Request::new("GET", "/channels/stable/manifest.json").with_header("Range", value);
            let response = respond(&root, &request);
            assert_eq!(response.status, 416, "Range: {value}");
            let expected = format!("bytes */{}", MANIFEST.len());
            assert_eq!(response.header("content-range"), Some(expected.as_str()));
        }
    }

    #[test]
    fn multi_range_is_ignored_and_returns_200() {
        let (_dir, root) = fixture();
        let request = Request::new("GET", "/channels/stable/manifest.json")
            .with_header("Range", "bytes=0-1,3-4");
        let response = respond(&root, &request);
        assert_eq!(response.status, 200);
        assert_eq!(body_bytes(&response), MANIFEST);
    }

    #[test]
    fn head_has_full_length_but_no_body() {
        let (_dir, root) = fixture();
        let response = respond(
            &root,
            &Request::new("HEAD", "/channels/stable/manifest.json"),
        );
        assert_eq!(response.status, 200);
        assert!(matches!(response.body, Body::Empty));
        let expected_len = MANIFEST.len().to_string();
        assert_eq!(
            response.header("content-length"),
            Some(expected_len.as_str())
        );
    }

    #[test]
    fn blob_serves_with_content_addressed_etag() {
        let (_dir, root) = fixture();
        let path = format!("/blobs/sha256/{BLOB_HEX}");
        let response = respond(&root, &Request::new("GET", &path));
        assert_eq!(response.status, 200);
        assert_eq!(body_bytes(&response), b"hello blob");
        assert_eq!(
            response.header("content-type"),
            Some("application/octet-stream")
        );
        let expected_etag = format!("\"sha256:{BLOB_HEX}\"");
        assert_eq!(response.header("etag"), Some(expected_etag.as_str()));
        assert_eq!(
            response.header("cache-control"),
            Some("public, max-age=31536000, immutable")
        );
    }

    #[test]
    fn unknown_and_reserved_paths_are_404() {
        let (_dir, root) = fixture();
        for path in [
            "/",
            "/channels",
            "/channels/",
            "/channels/stable",
            "/channels/stable/",
            "/channels/stable/other.json",
            "/channels/night/manifest.json",
            "/schema/plugin.schema.json",
            "/catalog/manifest.json",
            "/api/v1/publish",
            "/api/",
        ] {
            assert_eq!(
                respond(&root, &Request::new("GET", path)).status,
                404,
                "{path}"
            );
        }
    }

    #[test]
    fn traversal_attempts_are_rejected() {
        let (_dir, root) = fixture();
        for path in [
            "/../catalog/manifest.json",
            "/channels/stable/../../catalog/manifest.json",
            "/channels/stable/%2e%2e/manifest.json",
            "/%2e%2e/%2e%2e/etc/passwd",
            "/channels%2f..%2f..%2fetc%2fpasswd",
            "/channels\\stable\\manifest.json",
            "/channels/stable/manifest.json%00.txt",
        ] {
            let status = respond(&root, &Request::new("GET", path)).status;
            assert_eq!(status, 404, "{path}");
        }
    }

    #[test]
    fn blob_name_must_be_64_lowercase_hex() {
        let (_dir, root) = fixture();
        let upper = BLOB_HEX.to_ascii_uppercase();
        for name in [
            "abc".to_string(),
            BLOB_HEX[..63].to_string(),
            format!("{BLOB_HEX}0"),
            upper,
            "z".repeat(64),
        ] {
            let path = format!("/blobs/sha256/{name}");
            assert_eq!(
                respond(&root, &Request::new("GET", &path)).status,
                404,
                "{name}"
            );
        }
    }

    #[test]
    fn non_get_methods_are_405_with_allow() {
        let (_dir, root) = fixture();
        for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
            let response = respond(
                &root,
                &Request::new(method, "/channels/stable/manifest.json"),
            );
            assert_eq!(response.status, 405, "{method}");
            assert_eq!(response.header("allow"), Some("GET, HEAD"));
        }
    }

    #[test]
    fn http_date_round_trips() {
        // 784111777 = Sun, 06 Nov 1994 08:49:37 GMT
        assert_eq!(
            http_date(UNIX_EPOCH + Duration::from_secs(784_111_777)),
            "Sun, 06 Nov 1994 08:49:37 GMT"
        );
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 PST"), None);
    }
}
