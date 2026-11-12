//! 发布面 `/api/**`（§17 R0）：开发者提交、管理员审核。
//!
//! 本模块**不参与分发**，也不持有清单签名私钥：它只写 `--state` 下的提交记录、
//! 暂存包与审计。产物生成由独立的 `release` 子命令完成（私钥只在受控环境）。
//! `serve` 与 `api` 是两个进程，`serve` 对 `/api/**` 仍然一律 404。

pub mod audit;
pub mod auth;
pub mod job;
pub mod multipart;
pub mod package;
pub mod store;
pub mod takedown;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::http::{self, read_head, Body, Request, Response};

/// 请求体上限：单包 64 MiB（设计目标）+ multipart 头部余量。
pub const MAX_BODY_BYTES: usize = 65 * 1024 * 1024;
/// 单包上限（§17.3 设计目标）。
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;

/// API 统一错误：`status` 决定响应码，`code` 给机器看，`message` 给人看。
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, "bad_request", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(401, "unauthorized", "unauthorized")
    }

    /// 权限失败一律同一个响应体：不透露「对象不存在」还是「无权访问」。
    pub fn forbidden() -> Self {
        Self::new(403, "forbidden", "forbidden")
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, "conflict", message)
    }

    pub fn precondition(message: impl Into<String>) -> Self {
        Self::new(412, "precondition_failed", message)
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::new(422, "unprocessable", message)
    }

    pub fn internal() -> Self {
        Self::new(500, "internal", "internal")
    }

    pub fn unavailable() -> Self {
        Self::new(503, "unavailable", "auth configuration unavailable")
    }
}

pub fn json_response(status: u16, value: &serde_json::Value) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    Response {
        status,
        headers: vec![
            (
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "no-store".to_string()),
            ("content-length".to_string(), body.len().to_string()),
        ],
        body: Body::Bytes(body),
    }
}

pub fn error_response(error: &ApiError) -> Response {
    json_response(
        error.status,
        &json!({ "code": error.code, "message": error.message }),
    )
}

/// 运行期配置（启动时校验，之后只读）。
#[derive(Debug, Clone)]
pub struct ApiServer {
    pub state: PathBuf,
    pub authors: PathBuf,
    pub reviewers: PathBuf,
    pub catalog: PathBuf,
    pub channel: String,
    pub allow_remote: bool,
}

impl ApiServer {
    /// 启动校验：state 目录 0700、白名单 0600 且可解析、源数据渠道目录存在，否则拒启。
    pub fn prepare(
        state: PathBuf,
        authors: PathBuf,
        reviewers: PathBuf,
        catalog: PathBuf,
        channel: String,
        allow_remote: bool,
    ) -> Result<Arc<Self>> {
        require_dir_mode(&state, 0o700, "state 目录")?;
        require_file_mode(&authors, 0o600, "authors.json")?;
        require_file_mode(&reviewers, 0o600, "reviewers.json")?;
        auth::load_authors(&authors).context("authors.json 非法")?;
        auth::load_reviewers(&reviewers).context("reviewers.json 非法")?;
        let channel_dir = catalog.join(&channel);
        if !channel_dir.is_dir() {
            bail!("源数据渠道目录不存在: {}", channel_dir.display());
        }
        for sub in ["submissions", "nonces", "takedowns"] {
            let dir = state.join(sub);
            fs::create_dir_all(&dir).with_context(|| format!("创建失败: {}", dir.display()))?;
            set_dir_mode(&dir, 0o700)?;
        }
        Ok(Arc::new(Self {
            state,
            authors,
            reviewers,
            catalog,
            channel,
            allow_remote,
        }))
    }

    /// 强制回环监听：非回环地址必须显式 `--allow-remote`（并在前面放 TLS 反代）。
    pub fn check_listen(&self, addr: &std::net::SocketAddr) -> Result<()> {
        if !addr.ip().is_loopback() && !self.allow_remote {
            bail!(
                "拒绝监听非回环地址 {addr}：API 会在网络上传输包体与提交元数据；\
                 请用 TLS 反代转发到 127.0.0.1，或显式加 --allow-remote（不推荐）"
            );
        }
        Ok(())
    }

    pub fn audit(&self, key_id: &str, action: &str, sid: &str, result: &str) {
        let line = json!({
            "at": crate::api::now_rfc3339(),
            "key_id": key_id,
            "action": action,
            "submission_id": sid,
            "result": result,
        });
        let mut text = line.to_string();
        text.push('\n');
        let path = self.state.join("audit.jsonl");
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(text.as_bytes()));
    }
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn require_dir_mode(path: &Path, want: u32, what: &str) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("{what}不存在: {}", path.display()))?;
    if !meta.is_dir() {
        bail!("{what}不是目录: {}", path.display());
    }
    check_mode(&meta, want, what, path)
}

fn require_file_mode(path: &Path, want: u32, what: &str) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("{what}不存在: {}", path.display()))?;
    if !meta.is_file() {
        bail!("{what}不是普通文件: {}", path.display());
    }
    check_mode(&meta, want, what, path)
}

#[cfg(unix)]
fn check_mode(meta: &fs::Metadata, want: u32, what: &str, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode != want {
        bail!(
            "{what}权限必须是 {want:o}，实为 {mode:o}: {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_mode(_meta: &fs::Metadata, _want: u32, _what: &str, _path: &Path) -> Result<()> {
    // Windows 没有 POSIX 位；权限边界由 ACL 与回环监听保证。
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("设置权限失败: {}", path.display()))
}

#[cfg(not(unix))]
fn set_dir_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// 阻塞式服务循环：每连接一线程，一个请求后关闭（与 `serve` 同风格）。
pub fn serve(listener: TcpListener, server: Arc<ApiServer>) -> Result<()> {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else { continue };
        let server = Arc::clone(&server);
        std::thread::spawn(move || handle_connection(stream, &server));
    }
    Ok(())
}

fn handle_connection(stream: std::net::TcpStream, server: &ApiServer) {
    let _ = stream.set_read_timeout(Some(http::READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(http::WRITE_TIMEOUT));
    let mut stream = stream;
    handle_stream(&mut stream, server);
}

fn handle_stream<S: Read + Write>(stream: &mut S, server: &ApiServer) {
    let Some((request, leftover)) = read_head(stream) else {
        return;
    };
    let response = match read_body(&request, leftover, stream) {
        Ok(body) => match route(server, &request, &body) {
            Ok(response) => response,
            Err(error) => {
                // 鉴权前失败不写审计；避免把探测行为放大成日志噪声。
                let sid = path_sid(&request.target).unwrap_or_default();
                let key_id = request.header("x-skiff-key-id").unwrap_or("-");
                if error.status != 401 {
                    server.audit(key_id, &action_name(&request), &sid, "fail");
                }
                error_response(&error)
            }
        },
        Err((status, code, message)) => {
            json_response(status, &json!({ "code": code, "message": message }))
        }
    };
    let _ = http::write_response(stream, &response, false);
}

fn action_name(request: &Request) -> String {
    let path = path_only(&request.target);
    let action = if path.ends_with("/approve") {
        "approve"
    } else if path.ends_with("/reject") {
        "reject"
    } else if path.ends_with("/withdraw") {
        "withdraw"
    } else if request.method == "POST" && (path == "/api/v1/submissions" || path == "/api/v1/apps")
    {
        "submit"
    } else if request.method == "GET" {
        "read"
    } else {
        "request"
    };
    action.to_string()
}

fn path_sid(target: &str) -> Option<String> {
    let parts: Vec<&str> = path_only(target)
        .trim_start_matches('/')
        .split('/')
        .collect();
    // /api/v1/submissions/{sid}[/action]
    parts.get(3).map(|v| (*v).to_string())
}

pub fn path_only(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn query_only(target: &str) -> &str {
    target.split_once('?').map(|(_, q)| q).unwrap_or("")
}

/// 读请求体：必须有 `Content-Length`（POST），拒绝任何 `Transfer-Encoding`，
/// 超限直接 413 且不读 body。
fn read_body<S: Read>(
    request: &Request,
    leftover: Vec<u8>,
    stream: &mut S,
) -> Result<Vec<u8>, (u16, &'static str, String)> {
    if request.header("transfer-encoding").is_some() {
        return Err((
            400,
            "transfer_encoding",
            "不支持 Transfer-Encoding（含 chunked），请直接给 Content-Length".to_string(),
        ));
    }
    let has_body_method = !matches!(request.method.as_str(), "GET" | "HEAD" | "DELETE");
    let declared = match request.header("content-length") {
        Some(value) => Some(value.parse::<usize>().map_err(|_| {
            (
                400u16,
                "bad_request",
                "Content-Length 不是合法数字".to_string(),
            )
        })?),
        None => None,
    };
    let Some(len) = declared else {
        if has_body_method {
            return Err((411, "length_required", "缺少 Content-Length".to_string()));
        }
        return Ok(Vec::new());
    };
    if len > MAX_BODY_BYTES {
        return Err((
            413,
            "payload_too_large",
            "请求体超过 64 MiB 上限".to_string(),
        ));
    }
    let mut body = leftover;
    if body.len() > len {
        body.truncate(len);
    }
    while body.len() < len {
        let mut chunk = vec![0u8; (len - body.len()).min(64 * 1024)];
        let read = stream
            .read(&mut chunk)
            .map_err(|_| (400u16, "bad_request", "读取请求体失败".to_string()))?;
        if read == 0 {
            return Err((400, "bad_request", "请求体不完整".to_string()));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Ok(body)
}

/// 路由：先鉴权，再分发。所有失败都收敛成 `ApiError`。
fn route(server: &ApiServer, request: &Request, body: &[u8]) -> Result<Response, ApiError> {
    let path = path_only(&request.target);
    if !path.starts_with("/api/v1/") && path != "/api/v1" {
        return Err(ApiError::new(404, "not_found", "not found"));
    }
    let auth = auth::authenticate(server, request, body)?;
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if parts.len() < 3 {
        return Err(ApiError::new(404, "not_found", "not found"));
    }
    match parts[2] {
        "submissions" => route_submissions(server, &auth, request, body, &parts),
        "apps" if parts.len() == 3 => match request.method.as_str() {
            "POST" => store::create_app(server, &auth, request, body),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        },
        "plugins" if parts.len() == 7 && parts[4] == "versions" && parts[6] == "yank" => {
            match request.method.as_str() {
                "POST" => takedown::yank(server, &auth, parts[3], parts[5], body),
                _ => Err(ApiError::new(
                    405,
                    "method_not_allowed",
                    "method not allowed",
                )),
            }
        }
        "keys" if parts.len() == 5 && parts[4] == "revoke" => match request.method.as_str() {
            "POST" => takedown::revoke_key(server, &auth, parts[3], body),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        },
        "audit" if parts.len() == 3 => match request.method.as_str() {
            "GET" => audit::query(server, &auth, query_only(&request.target)),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        },
        _ => Err(ApiError::new(404, "not_found", "not found")),
    }
}

fn route_submissions(
    server: &ApiServer,
    auth: &auth::Authenticated,
    request: &Request,
    body: &[u8],
    parts: &[&str],
) -> Result<Response, ApiError> {
    if parts.len() == 3 {
        return match request.method.as_str() {
            "POST" => store::create(server, auth, request, body),
            "GET" => store::list(server, auth, query_only(&request.target)),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        };
    }
    let sid = parts[3];
    if parts.len() == 4 {
        return match request.method.as_str() {
            "GET" => store::get(server, auth, sid),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        };
    }
    if parts.len() == 5 {
        return match (request.method.as_str(), parts[4]) {
            ("POST", "approve") => store::approve(server, auth, sid),
            ("POST", "reject") => store::reject(server, auth, sid, body),
            ("POST", "withdraw") => store::withdraw(server, auth, sid),
            _ => Err(ApiError::new(
                405,
                "method_not_allowed",
                "method not allowed",
            )),
        };
    }
    Err(ApiError::new(404, "not_found", "not found"))
}
