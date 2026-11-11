//! `serve`：把已生成的静态产物目录按 [docs/SKIFF_SERVER.md §16] 的契约原样提供。
//!
//! 只读、无写入路由、无目录列表；默认 https（显式 `--http` 才明文）。
//! 契约是规范来源，本文件只是实现：路径白名单 → 条件请求 → 单区间 Range。
//! HTTP 传输件（读请求头/写响应/TLS）在 `crate::http`，与发布面 `api` 共用。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use ureq::rustls;

use crate::http::{read_head, write_response, READ_TIMEOUT, WRITE_TIMEOUT};
pub use crate::http::{tls_config, Body, Request, Response};

/// JSON 允许读入内存的上限；超过按内部错误处理（产物 JSON 只有几十 KB 量级）。
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

const JSON_FILES: [&str; 3] = ["manifest.json", "manifest.json.sig", "index.json"];

/// 白名单命中的资源种类。只有这两种路径会落到文件系统。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Json,
    Blob { hex: String },
}

/// 可选访问日志（默认关闭）。开启后每请求一行，用于服务端侧取证
/// （304 的命中来源、206 的区间）。**不改变任何协议语义**；
/// 写失败一律忽略：日志不是响应路径的一部分，也不允许 panic。
#[derive(Debug, Clone)]
pub struct AccessLog {
    sink: AccessSink,
}

#[derive(Debug, Clone)]
enum AccessSink {
    /// stderr：systemd 下直接进 journal（`journalctl -u <unit> -f`）。
    Stderr,
    File(PathBuf),
}

impl AccessLog {
    pub fn stderr() -> Self {
        Self {
            sink: AccessSink::Stderr,
        }
    }

    /// 追加写文件：启动时验证可写，之后每行重新打开，兼容 logrotate（不持有常驻 fd）。
    pub fn file(path: &Path) -> Result<Self> {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("--access-log 无法写入: {}", path.display()))?;
        Ok(Self {
            sink: AccessSink::File(path.to_path_buf()),
        })
    }

    fn write_line(&self, line: &str) {
        let _ = match &self.sink {
            AccessSink::Stderr => {
                eprintln!("{line}");
                Ok(())
            }
            AccessSink::File(path) => fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes())),
        };
    }
}

/// 304 的命中来源，仅用于访问日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheHit {
    None,
    IfNoneMatch,
    IfModifiedSince,
}

impl CacheHit {
    fn label(self) -> &'static str {
        match self {
            CacheHit::None => "none",
            CacheHit::IfNoneMatch => "inm",
            CacheHit::IfModifiedSince => "ims",
        }
    }
}

/// 实际生效的单区间，仅用于访问日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeLog {
    None,
    Satisfied(u64, u64),
    Unsatisfiable,
}

impl RangeLog {
    fn label(self) -> String {
        match self {
            RangeLog::None => "none".to_string(),
            RangeLog::Satisfied(start, end) => format!("{start}-{end}"),
            RangeLog::Unsatisfiable => "unsatisfiable".to_string(),
        }
    }
}

/// 响应 + 日志所需的判定结果（两者同源，避免日志与响应各算一遍）。
#[derive(Debug)]
struct Outcome {
    response: Response,
    cache: CacheHit,
    range: RangeLog,
}

impl Outcome {
    fn plain(response: Response) -> Self {
        Self {
            response,
            cache: CacheHit::None,
            range: RangeLog::None,
        }
    }
}

/// 阻塞式服务循环：每连接一线程，处理一个请求后关闭。
///
/// `root` 在启动时 canonicalize 一次；后续所有包含性检查都基于它。
/// `access_log` 为 `None` 时整条日志路径不存在：不打开文件、不写 stderr。
pub fn serve(
    listener: TcpListener,
    root: &Path,
    tls: Option<Arc<rustls::ServerConfig>>,
    access_log: Option<AccessLog>,
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
        let access_log = access_log.clone();
        std::thread::spawn(move || handle_connection(stream, &root, tls, access_log.as_ref()));
    }
    Ok(())
}

fn handle_connection(
    stream: std::net::TcpStream,
    root: &Path,
    tls: Option<Arc<rustls::ServerConfig>>,
    access_log: Option<&AccessLog>,
) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    match tls {
        Some(config) => match rustls::ServerConnection::new(config) {
            Ok(conn) => {
                let mut stream = rustls::StreamOwned::new(conn, stream);
                handle_http(&mut stream, root, access_log);
            }
            Err(error) => tracing_free_log(&format!("TLS 会话建立失败: {error}")),
        },
        None => {
            let mut stream = stream;
            handle_http(&mut stream, root, access_log);
        }
    }
}

fn handle_http<S: Read + Write>(stream: &mut S, root: &Path, access_log: Option<&AccessLog>) {
    let Some((request, _body)) = read_head(stream) else {
        return;
    };
    let head_only = request.method.eq_ignore_ascii_case("HEAD");
    let outcome = respond_full(root, &request);
    match write_response(stream, &outcome.response, head_only) {
        Ok(written) => {
            // 先回响应再写日志：日志永远不参与响应路径。
            if let Some(access_log) = access_log {
                access_log.write_line(&access_line(&request, &outcome, written));
            }
        }
        Err(error) => tracing_free_log(&format!("写响应失败: {error}")),
    }
}

fn tracing_free_log(message: &str) {
    eprintln!("[serve] {message}");
}

/// 契约核心：方法 → 路径白名单 → 条件请求 → Range。
pub fn respond(root: &Path, request: &Request) -> Response {
    respond_full(root, request).response
}

/// 契约核心：方法 → 路径白名单 → 条件请求 → Range；同时带回访问日志所需判定。
fn respond_full(root: &Path, request: &Request) -> Outcome {
    if !(request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD"))
    {
        return Outcome::plain(text_response(
            405,
            "method not allowed",
            vec![("Allow", "GET, HEAD")],
        ));
    }
    let head_only = request.method.eq_ignore_ascii_case("HEAD");
    let Some((path, kind)) = resolve(root, &request.target) else {
        return Outcome::plain(not_found());
    };
    let Ok(canonical) = fs::canonicalize(&path) else {
        return Outcome::plain(not_found());
    };
    if !canonical.starts_with(root) {
        return Outcome::plain(not_found());
    }
    let Ok(meta) = fs::metadata(&canonical) else {
        return Outcome::plain(not_found());
    };
    if !meta.is_file() {
        return Outcome::plain(not_found());
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
                return Outcome::plain(text_response(500, "internal error", Vec::new()));
            }
            let Ok(body) = fs::read(&canonical) else {
                return Outcome::plain(text_response(500, "internal error", Vec::new()));
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

    let cache = cache_hit(request, &etag, modified);
    if cache != CacheHit::None {
        return Outcome {
            response: Response {
                status: 304,
                headers,
                body: Body::Empty,
            },
            cache,
            range: RangeLog::None,
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
            Outcome {
                response: Response {
                    status: 416,
                    headers,
                    body: Body::Bytes(text.as_bytes().to_vec()),
                },
                cache: CacheHit::None,
                range: RangeLog::Unsatisfiable,
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
            Outcome {
                response: Response {
                    status: 206,
                    headers,
                    body: if head_only { Body::Empty } else { body },
                },
                cache: CacheHit::None,
                range: RangeLog::Satisfied(start, end),
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
            Outcome {
                response: Response {
                    status: 200,
                    headers,
                    body: if head_only { Body::Empty } else { body },
                },
                cache: CacheHit::None,
                range: RangeLog::None,
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

/// 条件请求判定：`If-None-Match` 优先（强比较），无 INM 时才看 `If-Modified-Since`。
/// 返回值同时作为访问日志里 `cache=` 字段的来源。
fn cache_hit(request: &Request, etag: &str, modified: SystemTime) -> CacheHit {
    if let Some(inm) = request.header("if-none-match") {
        let hit = inm.split(',').any(|tag| {
            let tag = tag.trim();
            tag == "*" || tag.strip_prefix("W/").unwrap_or(tag) == etag
        });
        return if hit {
            CacheHit::IfNoneMatch
        } else {
            CacheHit::None
        };
    }
    if let Some(ims) = request
        .header("if-modified-since")
        .and_then(parse_http_date)
    {
        let mtime = modified
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return if mtime <= ims {
            CacheHit::IfModifiedSince
        } else {
            CacheHit::None
        };
    }
    CacheHit::None
}

/// 每请求一行访问日志。只记方法、原始 target（含查询串）、状态码、写出的字节数、
/// 304 命中来源与生效区间；不记请求头全文，避免把凭据带进日志。
fn access_line(request: &Request, outcome: &Outcome, written: u64) -> String {
    format!(
        "{} {} {} status={} bytes={} cache={} range={}",
        http_date(SystemTime::now()),
        sanitize(&request.method),
        sanitize(&request.target),
        outcome.response.status,
        written,
        outcome.cache.label(),
        outcome.range.label()
    )
}

/// 日志字段清洗：控制字符（含换行）替换为 `?`，杜绝日志注入。
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
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
    use std::io;
    use std::io::{Seek, SeekFrom};
    use std::time::Duration;

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

    /// 内存双端流：喂原始请求、收原始响应，用来在单测里跑完整的 `handle_http`。
    struct MockIo {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl MockIo {
        fn new(raw: &str) -> Self {
            Self {
                input: std::io::Cursor::new(raw.as_bytes().to_vec()),
                output: Vec::new(),
            }
        }
    }

    impl Read for MockIo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for MockIo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn run(root: &Path, raw: &str, access_log: Option<&AccessLog>) -> String {
        let mut io = MockIo::new(raw);
        handle_http(&mut io, root, access_log);
        String::from_utf8_lossy(&io.output).to_string()
    }

    #[test]
    fn access_log_records_inm_hit_as_304() {
        let (dir, root) = fixture();
        let etag = etag_for(MANIFEST);
        let raw = format!(
            "GET /channels/stable/manifest.json?x=1 HTTP/1.1\r\nHost: t\r\n\
             If-None-Match: {etag}\r\n\r\n"
        );
        let path = dir.path().join("access.log");
        let access = AccessLog::file(&path).unwrap();

        let response = run(&root, &raw, Some(&access));
        assert!(response.starts_with("HTTP/1.1 304"), "{response}");

        let log = fs::read_to_string(&path).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(
            log.contains("GET /channels/stable/manifest.json?x=1"),
            "{log}"
        );
        assert!(log.contains("status=304"), "{log}");
        assert!(log.contains("bytes=0"), "{log}");
        assert!(log.contains("cache=inm"), "{log}");
        assert!(log.contains("range=none"), "{log}");
    }

    #[test]
    fn access_log_records_range_206() {
        let (dir, root) = fixture();
        let raw = "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: t\r\n\
                   Range: bytes=0-3\r\n\r\n";
        let path = dir.path().join("access.log");
        let access = AccessLog::file(&path).unwrap();

        let response = run(&root, raw, Some(&access));
        assert!(response.starts_with("HTTP/1.1 206"), "{response}");

        let log = fs::read_to_string(&path).unwrap();
        assert!(log.contains("status=206"), "{log}");
        assert!(log.contains("bytes=4"), "{log}");
        assert!(log.contains("range=0-3"), "{log}");
        assert!(log.contains("cache=none"), "{log}");
    }

    #[test]
    fn access_log_records_405() {
        let (dir, root) = fixture();
        let raw = "POST /channels/stable/manifest.json HTTP/1.1\r\nHost: t\r\n\r\n";
        let path = dir.path().join("access.log");
        let access = AccessLog::file(&path).unwrap();

        let response = run(&root, raw, Some(&access));
        assert!(response.starts_with("HTTP/1.1 405"), "{response}");

        let log = fs::read_to_string(&path).unwrap();
        assert!(log.contains("POST /channels/stable/manifest.json"), "{log}");
        assert!(log.contains("status=405"), "{log}");
        assert!(log.contains("bytes=18"), "{log}");
        assert!(log.contains("cache=none"), "{log}");
        assert!(log.contains("range=none"), "{log}");
    }

    #[test]
    fn access_log_disabled_creates_nothing() {
        let (dir, root) = fixture();
        let path = dir.path().join("access.log");
        let raw = "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: t\r\n\r\n";

        let response = run(&root, raw, None);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(!path.exists(), "未开启访问日志时不得创建或写入任何文件");
    }

    #[test]
    fn access_log_write_failure_keeps_response_intact() {
        let (dir, root) = fixture();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        let path = dir.path().join("logs/access.log");
        let access = AccessLog::file(&path).unwrap();
        // 启动后目录被移除：逐行打开会失败，但不能影响响应，也不能 panic。
        fs::remove_dir_all(dir.path().join("logs")).unwrap();

        let raw = "GET /channels/stable/manifest.json HTTP/1.1\r\nHost: t\r\n\r\n";
        let response = run(&root, raw, Some(&access));
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let body = String::from_utf8_lossy(MANIFEST);
        assert!(response.ends_with(body.as_ref()), "{response}");
    }
}
