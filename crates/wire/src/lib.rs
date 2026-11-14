//! 插件 ↔ 内核的 UDS 协议：HTTP/1.1 帧编解码、路由常量、帧上限（§4.2）。
//!
//! **本 crate 不含 I/O、不含运行时**：内核侧用 tokio、插件侧用 std，
//! 两边共享同一份「字节 → 结构」与「结构 → 字节」的实现。
//! 这样做的直接原因：协议原先在 `plugin/ipc.rs` 与 `sdk/lib.rs` 各写一遍，
//! 已经漂移过一次——SDK 调 `/api/v1/capability/config` 而内核没有这条路由，
//! 插件读配置必然 404。路由字符串现在只有这里一份。
//!
//! 帧格式固定为 `Content-Length` + `Connection: close`：单请求单响应，
//! 不做 chunked、不做 keep-alive，解析器因此可以保持在几十行内。

use serde_json::Value;

/// 单帧上限（请求体或响应体），超限快速失败，避免恶意插件撑爆内存。
pub const MAX_FRAME: usize = 1024 * 1024;

/// 请求头上限。头部不可能接近帧上限，单独卡死以免慢速灌头。
pub const MAX_HEAD: usize = 64 * 1024;

/// 协议版本（内核 ↔ 插件必须一致）。
///
/// 插件是**独立发布**的 `.wbpkg`：同一台机器上内核已升级、插件还停在旧版是常态。
/// 没有版本号时，「路由改了 / 字段变了」只会在运行期表现为 404 或字段缺失，
/// 排查成本远高于握手时的一次显式拒绝。
pub const PROTOCOL_VERSION: u32 = 1;

/// 协议版本请求头名（两端共用，取值是十进制版本号）。
pub const PROTOCOL_HEADER: &str = "x-skiff-protocol";

/// 能力面与插件自带端点。**两端只能引用这里的常量**，不得再写字面量。
pub mod routes {
    // ---- 内核提供、插件回调（kernel.sock）----
    pub const CAP_IDENTITY_SELF: &str = "/api/v1/capability/identity/self";
    pub const CAP_CONFIG: &str = "/api/v1/capability/config";
    pub const CAP_FS_READ: &str = "/api/v1/capability/fs/read";
    pub const CAP_FS_WRITE: &str = "/api/v1/capability/fs/write";
    pub const CAP_FS_LIST: &str = "/api/v1/capability/fs/list";
    /// 宿主路径类操作（官方插件 files 用）：路径必须是绝对路径，
    /// 且落在安装时确认过的 `path:` 对象授权根内。
    pub const CAP_FS_MKDIR: &str = "/api/v1/capability/fs/mkdir";
    pub const CAP_FS_REMOVE: &str = "/api/v1/capability/fs/remove";
    pub const CAP_FS_RENAME: &str = "/api/v1/capability/fs/rename";
    pub const CAP_DOCKER_CALL: &str = "/api/v1/capability/docker/call";
    pub const CAP_NET_APPLY: &str = "/api/v1/capability/net/apply";
    /// 跨插件接口调用：`{plugin, method, params}` → 内核转发到目标插件 RPC。
    /// 只做**单层**转发（被调用方不能再发起 interop，防 A→B→A 环路）。
    pub const CAP_INTEROP_CALL: &str = "/api/v1/capability/interop/call";
    /// 已启用插件对外暴露的接口清单（供插件 UI 选择目标，不返回实现细节）。
    pub const CAP_INTEROP_CATALOG: &str = "/api/v1/capability/interop/catalog";
    /// 内核定时任务能力面：`{method, params}` → `skiff-sched` 仓储/调度。
    /// 插件的 `scheduler` 能力声明对应此路由（任务列表/注册/更新/运行/缓冲）。
    pub const CAP_SCHEDULER_CALL: &str = "/api/v1/capability/scheduler/call";
    /// 宿主机 FTP 服务能力面（pure-ftpd 安装/账号/权限对齐）。
    ///
    /// 仅 Native 形态可用：`{method, params}` → 内核 `ftp` handler。该能力面是
    /// 内核新增原语（要装宿主软件包、改 systemd、写 pure-pw 用户库），不是
    /// 插件可自行实现的普通能力。
    pub const CAP_FTP_CALL: &str = "/api/v1/capability/ftp/call";
    /// 宿主机防火墙/fail2ban 能力面（nftables/iptables 规则、jail 管理）。
    ///
    /// 仅 Native 形态可用：`{method, params}` → 内核 `firewall` handler，
    /// 判定与命令编排在 `skiff_plugin::firewall`，这里是唯一的 UDS 协议入口。
    pub const CAP_FIREWALL_CALL: &str = "/api/v1/capability/firewall/call";

    // ---- 插件提供、内核调用（plugin.sock）----
    pub const PLUGIN_HEALTHZ: &str = "/healthz";
    pub const PLUGIN_RPC: &str = "/rpc";
}

/// JSON-RPC 错误的结构化形态：机器码、文案与上游 HTTP 状态。
///
/// 插件 SDK 会把内核非 200 响应折叠成 `协议错误: 内核返回 HTTP <code>: <detail>`
/// 文本；上游状态在这里解析一次，`web` 等调用方只读 `upstream_status`，不再
/// 各自按字符串片段猜测状态码。插件若在 `error.data` 里显式给出 `status` /
/// `upstream_status`，优先采信结构化字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcFailure {
    pub code: i64,
    pub message: String,
    pub upstream_status: Option<u16>,
}

impl RpcFailure {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        let message = message.into();
        let upstream_status = upstream_status_of(&message);
        Self {
            code,
            message,
            upstream_status,
        }
    }

    /// 从 JSON-RPC `error` 对象构造；`data.status` 优先于文案解析。
    pub fn from_json(err: &Value) -> Self {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-32000);
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown")
            .to_string();
        let structured = err
            .get("data")
            .and_then(|d| {
                d.get("status")
                    .or_else(|| d.get("upstream_status"))
                    .and_then(|s| s.as_u64())
            })
            .and_then(|n| u16::try_from(n).ok())
            .filter(|s| (100..=599).contains(s));
        let upstream_status = structured.or_else(|| upstream_status_of(&message));
        Self {
            code,
            message,
            upstream_status,
        }
    }
}

/// 从 SDK 固定文案里提取「内核返回 HTTP <code>」。
///
/// 只认这一固定片段，并校验 100..=599，避免把业务文案里的端口/计数误判成状态码。
pub fn upstream_status_of(message: &str) -> Option<u16> {
    let (_, tail) = message.split_once("内核返回 HTTP ")?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse::<u16>()
        .ok()
        .filter(|s| (100..=599).contains(s))
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("请求头未结束")]
    IncompleteHead,
    #[error("请求头过大（>{MAX_HEAD} 字节）")]
    HeadTooLarge,
    #[error("请求行非法")]
    BadRequestLine,
    #[error("状态行非法")]
    BadStatusLine,
    #[error("帧超过上限（{0} 字节）")]
    FrameTooLarge(usize),
    #[error("响应不是合法 JSON: {0}")]
    Json(String),
    #[error("对端返回错误 {}: {}", .0.code, .0.message)]
    Rpc(RpcFailure),
    #[error("协议版本不匹配（本端 {expected}，对端 {got}）")]
    ProtocolMismatch { expected: u32, got: u32 },
    #[error("对端未声明协议版本")]
    ProtocolMissing,
}

/// 校验对端声明的协议版本。
///
/// `None` = 对端没带版本头（协议头是后加的，老插件没有）：与其放行一个语义
/// 可能已经错位的对端，不如在握手处明确拒绝。
pub fn check_protocol(declared: Option<u32>) -> Result<(), WireError> {
    match declared {
        Some(v) if v == PROTOCOL_VERSION => Ok(()),
        Some(v) => Err(WireError::ProtocolMismatch {
            expected: PROTOCOL_VERSION,
            got: v,
        }),
        None => Err(WireError::ProtocolMissing),
    }
}

/// 解析后的请求头（请求体由调用方按 `content_length` 继续读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub content_length: usize,
    /// `Authorization` 原值（含 `Bearer ` 前缀），由调用方决定怎么校验。
    pub authorization: Option<String>,
    /// 对端声明的协议版本（`None` = 没带版本头）。
    pub protocol: Option<u32>,
}

/// 解析后的响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> Result<Value, WireError> {
        serde_json::from_slice(&self.body).map_err(|e| WireError::Json(e.to_string()))
    }
}

/// 找 `\r\n\r\n` 之后的位置（即头部长度）。
pub fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// 取一个头字段的值（大小写不敏感，返回首个匹配）。
pub fn header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    for line in text.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn content_length_of(head: &[u8]) -> usize {
    header_value(head, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 构造请求头（不含请求体）。`content-length` 取自 `body_len`。
///
/// 总是带上协议版本头：内核据此在握手处拒绝版本不符的插件。
pub fn build_request(method: &str, path: &str, body_len: usize, token: Option<&str>) -> String {
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: skiff\r\nConnection: close\r\n\
         Content-Type: application/json\r\n{PROTOCOL_HEADER}: {PROTOCOL_VERSION}\r\n\
         Content-Length: {body_len}\r\n"
    );
    if let Some(t) = token {
        head.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    head.push_str("\r\n");
    head
}

/// 构造响应头（不含响应体）。
pub fn build_response_head(status: u16, body_len: usize) -> String {
    format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\n\
         Content-Length: {body_len}\r\nConnection: close\r\n\r\n",
        if status == 200 { "OK" } else { "ERR" }
    )
}

/// 增量解析请求头。返回 `None` 表示头部还没收全，调用方继续读。
///
/// 返回的 `usize` 是头部字节数，调用方据此切出已到达的请求体前缀。
pub fn parse_request_head(buf: &[u8]) -> Result<Option<(RequestHead, usize)>, WireError> {
    let Some(end) = find_headers_end(buf) else {
        if buf.len() > MAX_HEAD {
            return Err(WireError::HeadTooLarge);
        }
        return Ok(None);
    };
    if end > MAX_HEAD {
        return Err(WireError::HeadTooLarge);
    }
    let head = &buf[..end];
    let text = String::from_utf8_lossy(head);
    let request_line = text.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    if method.is_empty() || path.is_empty() {
        return Err(WireError::BadRequestLine);
    }
    let content_length = content_length_of(head);
    if content_length > MAX_FRAME {
        return Err(WireError::FrameTooLarge(content_length));
    }
    Ok(Some((
        RequestHead {
            method: method.to_string(),
            path: path.to_string(),
            content_length,
            authorization: header_value(head, "authorization"),
            protocol: header_value(head, PROTOCOL_HEADER).and_then(|v| v.parse().ok()),
        },
        end,
    )))
}

/// 增量解析响应。`None` 表示还没收全（头部未结束或体未收满）。
///
/// 无 `Content-Length` 时按 `Connection: close` 语义处理：缓冲区里现有的就是全部。
/// 这一点必须与内核/插件两侧的实际写法一致，否则会出现「偶发截断」。
pub fn parse_response(buf: &[u8]) -> Result<Option<HttpResponse>, WireError> {
    let Some(end) = find_headers_end(buf) else {
        if buf.len() > MAX_HEAD {
            return Err(WireError::HeadTooLarge);
        }
        return Ok(None);
    };
    let head = &buf[..end];
    let status = String::from_utf8_lossy(head)
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(WireError::BadStatusLine)?;
    let body = &buf[end..];
    let declared = header_value(head, "content-length").and_then(|v| v.parse::<usize>().ok());
    match declared {
        Some(len) => {
            if len > MAX_FRAME {
                return Err(WireError::FrameTooLarge(len));
            }
            if body.len() < len {
                return Ok(None);
            }
            Ok(Some(HttpResponse {
                status,
                body: body[..len].to_vec(),
            }))
        }
        None => Ok(Some(HttpResponse {
            status,
            body: body.to_vec(),
        })),
    }
}

/// 构造 JSON-RPC 2.0 请求体。
pub fn rpc_request_body(method: &str, params: &Value) -> Result<Vec<u8>, WireError> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let body = serde_json::to_vec(&payload).map_err(|e| WireError::Json(e.to_string()))?;
    if body.len() > MAX_FRAME {
        return Err(WireError::FrameTooLarge(body.len()));
    }
    Ok(body)
}

/// 从 JSON-RPC 响应体取 `result`，`error` 转成 `WireError::Rpc`。
pub fn rpc_result(payload: &[u8]) -> Result<Value, WireError> {
    let v: Value = serde_json::from_slice(payload).map_err(|e| WireError::Json(e.to_string()))?;
    if let Some(err) = v.get("error") {
        return Err(WireError::Rpc(RpcFailure::from_json(err)));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_with_token() {
        let head = build_request("POST", routes::CAP_CONFIG, 2, Some("t0ken"));
        let parsed = parse_request_head(head.as_bytes()).unwrap().unwrap();
        assert_eq!(parsed.0.method, "POST");
        assert_eq!(parsed.0.path, routes::CAP_CONFIG);
        assert_eq!(parsed.0.content_length, 2);
        assert_eq!(parsed.0.authorization.as_deref(), Some("Bearer t0ken"));
        assert_eq!(parsed.1, head.len());
    }

    /// 构造出来的请求必须自带协议版本：内核靠这个头在握手处拒绝旧插件。
    #[test]
    fn request_declares_protocol_version() {
        let head = build_request("GET", routes::PLUGIN_HEALTHZ, 0, None);
        let parsed = parse_request_head(head.as_bytes()).unwrap().unwrap();
        assert_eq!(parsed.0.protocol, Some(PROTOCOL_VERSION));
        assert!(check_protocol(parsed.0.protocol).is_ok());
    }

    /// 版本不符与版本缺失都要在握手处被拒。
    #[test]
    fn protocol_mismatch_and_missing_are_rejected() {
        assert!(matches!(
            check_protocol(Some(PROTOCOL_VERSION + 1)),
            Err(WireError::ProtocolMismatch { .. })
        ));
        assert!(matches!(
            check_protocol(None),
            Err(WireError::ProtocolMissing)
        ));
    }

    /// 老插件（没有版本头）不应被当成「版本 0」而蒙混过关。
    #[test]
    fn request_without_version_header_is_detected() {
        let raw = b"GET /healthz HTTP/1.1\r\nHost: skiff\r\nContent-Length: 0\r\n\r\n";
        let parsed = parse_request_head(raw).unwrap().unwrap();
        assert_eq!(parsed.0.protocol, None);
        assert!(check_protocol(parsed.0.protocol).is_err());
    }

    #[test]
    fn response_waits_for_full_body() {
        let head = build_response_head(200, 7);
        // 头到、体没到
        assert!(parse_response(head.as_bytes()).unwrap().is_none());
        let mut buf = head.into_bytes();
        buf.extend_from_slice(b"{}");
        assert!(parse_response(&buf).unwrap().is_none());
        buf.extend_from_slice(b"abcde");
        let resp = parse_response(&buf).unwrap().unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"{}abcde");
    }

    #[test]
    fn response_without_content_length_takes_rest() {
        let raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"a\":1}";
        let resp = parse_response(raw).unwrap().unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.json().unwrap()["a"], 1);
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let head = format!(
            "POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_FRAME + 1
        );
        assert!(matches!(
            parse_request_head(head.as_bytes()),
            Err(WireError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn rpc_error_surfaces_code_and_message() {
        let e = rpc_result(br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#)
            .unwrap_err();
        match e {
            WireError::Rpc(failure) => {
                assert_eq!(failure.code, -32601);
                assert_eq!(failure.message, "nope");
                assert_eq!(failure.upstream_status, None);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rpc_result(br#"{"id":1,"result":42}"#).unwrap(), 42);
    }

    /// 上游状态在协议层解析一次：固定文案与结构化 `data` 都覆盖。
    #[test]
    fn rpc_error_extracts_upstream_status() {
        let text = rpc_result(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"协议错误: 内核返回 HTTP 403: 无权限"}}"#
                .as_bytes(),
        )
        .unwrap_err();
        assert!(matches!(
            text,
            WireError::Rpc(RpcFailure {
                upstream_status: Some(403),
                ..
            })
        ));

        let data = rpc_result(
            br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"denied","data":{"status":503}}}"#,
        )
        .unwrap_err();
        assert!(matches!(
            data,
            WireError::Rpc(RpcFailure {
                upstream_status: Some(503),
                ..
            })
        ));

        assert_eq!(upstream_status_of("内核返回 HTTP 403: x"), Some(403));
        assert_eq!(upstream_status_of("HTTP 403 与业务错误"), None);
        assert_eq!(upstream_status_of("内核返回 HTTP abc: x"), None);
        assert_eq!(upstream_status_of("端口 8080 被占用"), None);
        assert_eq!(upstream_status_of("内核返回 HTTP 999: x"), None);
    }

    /// 路由表是两端唯一的真相来源：任何端点被改名，这个测试都是最后一道提醒。
    #[test]
    fn route_table_is_stable() {
        assert_eq!(routes::CAP_CONFIG, "/api/v1/capability/config");
        assert_eq!(
            routes::CAP_IDENTITY_SELF,
            "/api/v1/capability/identity/self"
        );
        assert_eq!(routes::CAP_DOCKER_CALL, "/api/v1/capability/docker/call");
        assert_eq!(routes::CAP_INTEROP_CALL, "/api/v1/capability/interop/call");
        assert_eq!(
            routes::CAP_INTEROP_CATALOG,
            "/api/v1/capability/interop/catalog"
        );
        assert_eq!(routes::PLUGIN_RPC, "/rpc");
        assert!(routes::CAP_FS_READ.starts_with("/api/v1/capability/"));
        // 宿主路径类操作与 data/ 类操作必须在不同路由上：两者权限名与解析规则不同
        assert_eq!(routes::CAP_FS_MKDIR, "/api/v1/capability/fs/mkdir");
        assert_eq!(routes::CAP_FS_REMOVE, "/api/v1/capability/fs/remove");
        assert_eq!(routes::CAP_FS_RENAME, "/api/v1/capability/fs/rename");
    }
}
