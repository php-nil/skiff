//! Skiff 插件 SDK（第三方侧，核心③）：读取运行环境、提供 `/healthz` + `/rpc`、
//! 回调内核能力面。窄接口，只承诺 manifest `api` 主版本内的兼容（§4.4）。
//!
//! 插件作者只需要：
//! ```ignore
//! fn main() {
//!     let plugin = skiff_sdk::Plugin::from_env().expect("环境变量缺失");
//!     plugin.serve(|method, params| match method {
//!         "tick" => Ok(serde_json::json!({ "ok": true })),
//!         "status" => Ok(serde_json::json!({ "granted": plugin.granted() })),
//!         _ => Err(skiff_sdk::RpcError::method_not_found(method)),
//!     }).expect("服务启动失败");
//! }
//! ```

use std::collections::BTreeMap;
// 下面 `uds_http` / `read_request` / `write_response` 只在 unix 编译，
// 它们是标准库同步 I/O 的 trait 方法——漏了这行在 Windows 上完全看不出来。
#[cfg(unix)]
use std::io::{Read, Write};

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("缺少环境变量 {0}（必须由 Supervisor 拉起，不能手工启动）")]
    MissingEnv(&'static str),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("平台不支持: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, SdkError>;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: String,
    pub api: String,
    pub socket: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub run_dir: PathBuf,
    pub kernel_socket: PathBuf,
    pub granted: Vec<String>,
    /// 一次性身份令牌（经 stdin 继承交付，不落环境变量）
    pub token: String,
}

impl Plugin {
    /// 从 Supervisor 注入的环境变量构造。token 从 stdin 读一行。
    pub fn from_env() -> Result<Plugin> {
        let env = |k: &'static str| -> Result<String> {
            std::env::var(k).map_err(|_| SdkError::MissingEnv(k))
        };
        let id = env("SKIFF_PLUGIN_ID")?;
        let socket = PathBuf::from(env("SKIFF_PLUGIN_SOCKET")?);
        let data_dir = PathBuf::from(env("SKIFF_PLUGIN_DATA_DIR")?);
        let cache_dir = PathBuf::from(env("SKIFF_PLUGIN_CACHE_DIR")?);
        let run_dir = PathBuf::from(env("SKIFF_PLUGIN_RUN_DIR")?);
        let kernel_socket = PathBuf::from(env("SKIFF_KERNEL_SOCKET")?);
        let api = std::env::var("SKIFF_PLUGIN_API").unwrap_or_else(|_| "1.0".into());
        let granted = std::env::var("SKIFF_GRANTED_PERMS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut token = String::new();
        if std::io::stdin().read_line(&mut token).is_ok() {
            token = token.trim().to_string();
        }

        Ok(Plugin {
            id,
            api,
            socket,
            data_dir,
            cache_dir,
            run_dir,
            kernel_socket,
            granted,
            token,
        })
    }

    pub fn has_perm(&self, perm: &str) -> bool {
        self.granted.iter().any(|g| g == perm)
    }

    pub fn granted(&self) -> &[String] {
        &self.granted
    }

    /// 自身身份与已授予权限（内核 `capability/identity/self`）。
    pub fn identity(&self) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_IDENTITY_SELF, json!({}))
    }

    /// 读取内核代管的配置项（`config_schema`），`secret` 项同样经此返回（不落插件目录）。
    pub fn config(&self) -> Result<BTreeMap<String, String>> {
        let v = self.call_kernel(skiff_wire::routes::CAP_CONFIG, json!({}))?;
        let mut out = BTreeMap::new();
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    out.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(out)
    }

    /// 读自身 `data/` 下的文件（相对路径）。
    pub fn fs_read(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_READ, json!({ "path": path }))
    }

    /// 写自身 `data/` 下的文件（相对路径）。
    pub fn fs_write(&self, path: &str, data: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_WRITE,
            json!({ "path": path, "data": data }),
        )
    }

    /// 列自身 `data/` 目录（相对路径）。
    pub fn fs_list(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_LIST, json!({ "path": path }))
    }

    /// Docker 白名单动词（三层策略在内核侧判定，SDK 不做本地放行）。
    pub fn docker_call(&self, verb: &str, target: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_DOCKER_CALL,
            json!({ "verb": verb, "target": target }),
        )
    }

    /// 出网收口申请。**内核当前一律 501**（nftables 收口属 M4 门禁）。
    pub fn net_apply(&self, spec: Value) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_NET_APPLY, spec)
    }

    /// 回调内核能力面（`/api/v1/capability/*`），带进程级 Bearer token。
    pub fn call_kernel(&self, path: &str, body: Value) -> Result<Value> {
        let payload = serde_json::to_vec(&body).map_err(|e| SdkError::Protocol(e.to_string()))?;
        let resp = uds_http(
            &self.kernel_socket,
            "POST",
            path,
            Some(&payload),
            Some(&self.token),
        )?;
        if resp.status != 200 {
            return Err(SdkError::Protocol(format!(
                "内核返回 HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )));
        }
        resp.json().map_err(|e| SdkError::Protocol(e.to_string()))
    }

    /// 启动插件 HTTP 服务（阻塞）。只暴露 `/healthz` 与 `/rpc`。
    pub fn serve<F>(&self, handler: F) -> Result<()>
    where
        F: Fn(&str, Value) -> std::result::Result<Value, RpcError> + Send + Sync + 'static,
    {
        serve_inner(self, std::sync::Arc::new(handler))
    }
}

fn serve_inner<F>(plugin: &Plugin, handler: std::sync::Arc<F>) -> Result<()>
where
    F: Fn(&str, Value) -> std::result::Result<Value, RpcError> + Send + Sync + 'static,
{
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;

        // 运行目录由 Supervisor 预建并归属插件 UID；socket 0600 避免同机其它插件探活
        if let Some(parent) = plugin.socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&plugin.socket);
        let listener = UnixListener::bind(&plugin.socket)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&plugin.socket, std::fs::Permissions::from_mode(0o600));
        }

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let plugin = plugin.clone();
            let handler = handler.clone();
            std::thread::spawn(move || {
                let _ = handle_conn(&mut stream, &plugin, handler.as_ref());
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (plugin, handler);
        Err(SdkError::Unsupported(
            "插件只支持在 Linux/unix 宿主运行".into(),
        ))
    }
}

#[cfg(unix)]
fn handle_conn<F>(
    stream: &mut std::os::unix::net::UnixStream,
    plugin: &Plugin,
    handler: &F,
) -> Result<()>
where
    F: Fn(&str, Value) -> std::result::Result<Value, RpcError>,
{
    let (method, path, body) = read_request(stream)?;
    let (status, payload) = match (method.as_str(), path.as_str()) {
        ("GET", skiff_wire::routes::PLUGIN_HEALTHZ) => {
            (200, json!({ "ok": true, "plugin": plugin.id }))
        }
        ("POST", skiff_wire::routes::PLUGIN_RPC) => {
            let req: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(_) => {
                    return write_response(
                        stream,
                        400,
                        &json!({"error": {"code": -32700, "message": "parse error"}}),
                    );
                }
            };
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let m = req
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            match handler(&m, params) {
                Ok(result) => (200, json!({ "jsonrpc": "2.0", "id": id, "result": result })),
                Err(e) => (
                    200,
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
                ),
            }
        }
        _ => (404, json!({ "error": "not found" })),
    };
    write_response(stream, status, &payload)
}

#[cfg(unix)]
fn read_request(stream: &mut std::os::unix::net::UnixStream) -> Result<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // 头部解析与上限交给 skiff-wire，和内核侧同一份实现
    let (head, header_end) = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(SdkError::Protocol("连接提前关闭".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
        match skiff_wire::parse_request_head(&buf) {
            Ok(Some(v)) => break v,
            Ok(None) => continue,
            Err(e) => return Err(SdkError::Protocol(e.to_string())),
        }
    };
    let mut body = buf[header_end..].to_vec();
    while body.len() < head.content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(head.content_length);
    Ok((head.method, head.path, body))
}

#[cfg(unix)]
fn write_response(
    stream: &mut std::os::unix::net::UnixStream,
    status: u16,
    body: &Value,
) -> Result<()> {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let head = skiff_wire::build_response_head(status, payload.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

/// 最小 UDS HTTP 客户端（回调内核用）。帧编解码来自 `skiff-wire`。
#[cfg(unix)]
pub fn uds_http(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    token: Option<&str>,
) -> Result<skiff_wire::HttpResponse> {
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket)?;
    let payload = body.unwrap_or(&[]);
    if payload.len() > skiff_wire::MAX_FRAME {
        return Err(SdkError::Protocol("请求体超过单帧上限".into()));
    }
    let head = skiff_wire::build_request(method, path, payload.len(), token);
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    skiff_wire::parse_response(&buf)
        .map_err(|e| SdkError::Protocol(e.to_string()))?
        .ok_or_else(|| SdkError::Protocol("响应不完整".into()))
}

#[cfg(not(unix))]
pub fn uds_http(
    _socket: &std::path::Path,
    _method: &str,
    _path: &str,
    _body: Option<&[u8]>,
    _token: Option<&str>,
) -> Result<skiff_wire::HttpResponse> {
    Err(SdkError::Unsupported(
        "插件只支持在 Linux/unix 宿主运行".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_error_codes_follow_jsonrpc() {
        assert_eq!(RpcError::method_not_found("x").code, -32601);
        assert_eq!(RpcError::invalid_params("x").code, -32602);
        assert_eq!(RpcError::internal("x").code, -32603);
    }

    #[test]
    fn granted_permission_lookup() {
        let p = Plugin {
            id: "com.example.demo".into(),
            api: "1.0".into(),
            socket: PathBuf::from("/tmp/x"),
            data_dir: PathBuf::from("/tmp/d"),
            cache_dir: PathBuf::from("/tmp/c"),
            run_dir: PathBuf::from("/tmp/r"),
            kernel_socket: PathBuf::from("/tmp/k"),
            granted: vec!["docker:containers.list".into()],
            token: "t".into(),
        };
        assert!(p.has_perm("docker:containers.list"));
        assert!(!p.has_perm("docker:containers.exec"));
    }
}
