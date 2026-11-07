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

/// 宿主路径单次读写上限。插件↔内核走 `skiff_wire::MAX_FRAME`（1MiB）单帧，
/// 这里取 256KiB：base64 膨胀 4/3 后仍在帧内，且留足 JSON 头部余量。
pub const HOST_IO_MAX: usize = 256 * 1024;

/// `fs_host_read_bytes` / `fs_host_write_bytes` 的整文件上限（常规用例够用，
/// 再大的文件请自己按页读写，别让插件把整文件塞进内存）。
pub const HOST_WHOLE_MAX: usize = 32 * 1024 * 1024;

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

    /// 宿主路径读取的一页（base64），`path` 必须是安装时确认过的 `path:` 根内绝对路径。
    ///
    /// 返回 `(b64, offset, size, eof)`。单页上限 [`HOST_IO_MAX`]——插件↔内核是单帧
    /// 1MiB 的 UDS 协议，大文件必须分页，SDK 不替调用方猜「多大算大」。
    pub fn fs_host_read_b64(
        &self,
        path: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(String, u64, u64, bool)> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_FS_READ,
            json!({
                "path": path,
                "offset": offset,
                "max_bytes": max_bytes.min(HOST_IO_MAX),
                "encoding": "base64",
            }),
        )?;
        let b64 = v
            .get("b64")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let off = v.get("offset").and_then(|x| x.as_u64()).unwrap_or(0);
        let size = v.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
        let eof = v.get("eof").and_then(|x| x.as_bool()).unwrap_or(true);
        Ok((b64, off, size, eof))
    }

    /// 宿主路径写一页（base64）。`offset = None` 表示从 0 覆盖写；`append = true`
    /// 表示追加（分块上传的最后一块之外都用追加）。
    pub fn fs_host_write_b64(
        &self,
        path: &str,
        b64: &str,
        offset: Option<u64>,
        append: bool,
    ) -> Result<Value> {
        let mut body = json!({ "path": path, "b64": b64 });
        if let Some(o) = offset {
            body["offset"] = json!(o);
        }
        if append {
            body["append"] = json!(true);
        }
        self.call_kernel(skiff_wire::routes::CAP_FS_WRITE, body)
    }

    /// 宿主路径读文本（整文件，超过 [`HOST_IO_MAX`] 时报错而不是静默截断）。
    pub fn fs_host_read_text(&self, path: &str) -> Result<String> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_FS_READ,
            json!({ "path": path, "max_bytes": HOST_IO_MAX }),
        )?;
        if !v.get("eof").and_then(|x| x.as_bool()).unwrap_or(true) {
            return Err(SdkError::Protocol(format!(
                "文件超过 {} 字节，请改用 fs_host_read_b64 分页读取",
                HOST_IO_MAX
            )));
        }
        Ok(v.get("data")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// 宿主路径写文本（整文件覆盖）。
    pub fn fs_host_write_text(&self, path: &str, text: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_WRITE,
            json!({ "path": path, "data": text }),
        )
    }

    /// 列宿主目录（`items` 含 name/dir/size/mtime/mode）。
    pub fn fs_host_list(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_LIST, json!({ "path": path }))
    }

    pub fn fs_host_mkdir(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_MKDIR, json!({ "path": path }))
    }

    pub fn fs_host_remove(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_REMOVE, json!({ "path": path }))
    }

    pub fn fs_host_rename(&self, from: &str, to: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_RENAME,
            json!({ "path": from, "to": to }),
        )
    }

    /// 已授予的对象（`container:<id|name>` / `project:<p>` / `path:<abs>`）。
    ///
    /// 来自内核 `identity/self`，不是本地猜测——对象授权只在内核库里，env 里只有权限串。
    pub fn objects(&self) -> Result<Vec<String>> {
        let v = self.identity()?;
        Ok(v.get("objects")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 已授予的 `path:` 授权根（原始字符串，未 canonicalize）。
    pub fn path_roots(&self) -> Result<Vec<String>> {
        Ok(self
            .objects()?
            .into_iter()
            .filter_map(|o| o.strip_prefix("path:").map(str::to_string))
            .collect())
    }

    /// 整文件读字节（内部按 [`HOST_IO_MAX`] 分页，超过 [`HOST_WHOLE_MAX`] 报错）。
    pub fn fs_host_read_bytes(&self, path: &str) -> Result<Vec<u8>> {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let mut out = Vec::new();
        let mut offset = 0u64;
        loop {
            let (b64, _, _, eof) = self.fs_host_read_b64(path, offset, HOST_IO_MAX)?;
            let page = engine
                .decode(b64.as_bytes())
                .map_err(|e| SdkError::Protocol(format!("base64 解码失败: {e}")))?;
            offset += page.len() as u64;
            out.extend_from_slice(&page);
            if out.len() > HOST_WHOLE_MAX {
                return Err(SdkError::Protocol(format!(
                    "文件超过 {HOST_WHOLE_MAX} 字节，请改用分页接口"
                )));
            }
            if eof {
                return Ok(out);
            }
        }
    }

    /// 整文件写字节（内部按 [`HOST_IO_MAX`] 分页，第一页覆盖写、其余追加）。
    pub fn fs_host_write_bytes(&self, path: &str, data: &[u8]) -> Result<Value> {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        if data.len() > HOST_WHOLE_MAX {
            return Err(SdkError::Protocol(format!(
                "写入超过 {HOST_WHOLE_MAX} 字节，请改用分页接口"
            )));
        }
        // 空文件也必须显式写一次，否则「新建空文件」会变成「什么都不做」。
        if data.is_empty() {
            return self.fs_host_write_b64(path, "", None, false);
        }
        let mut last = Value::Null;
        for (i, chunk) in data.chunks(HOST_IO_MAX).enumerate() {
            let b64 = engine.encode(chunk);
            last = self.fs_host_write_b64(path, &b64, None, i > 0)?;
        }
        Ok(last)
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

    /// 调用另一个插件声明过的接口（manifest `[interop].consumes` + `interop:call` + `plugin:` 授权）。
    ///
    /// 内核在 `params` 里注入 `_interop.caller`（调用方插件 id），目标插件据此做来源
    /// 展示/幂等键；调用方传的同名字段会被内核覆盖。
    ///
    /// 返回目标插件 RPC 的 `result`（不是 `{"result": …}` 信封）。只有**单层**转发：
    /// 目标插件在自己的 handler 里再次发起 interop 会被内核以「环」拒绝。
    pub fn interop_call(&self, plugin: &str, method: &str, params: Value) -> Result<Value> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_INTEROP_CALL,
            json!({ "plugin": plugin, "method": method, "params": params }),
        )?;
        Ok(v.get("result").cloned().unwrap_or(v))
    }

    /// 列出已启用插件对外暴露的接口（`{plugins:[{id,name,methods:[…]}]}`）。
    ///
    /// 需要 `interop:call` 权限；不返回目标插件的授权/配置等敏感信息。
    pub fn interop_catalog(&self) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_INTEROP_CATALOG, json!({}))
    }

    /// 内核调度能力面（manifest 需声明 `scheduler` 能力与对应 verbs）。
    ///
    /// 常用方法：`task.list` / `task.get` / `task.history` / `task.register` /
    /// `task.update` / `task.delete` / `task.run`。返回内核原样 JSON（任务视图已带
    /// `next_run`/`state` 等展示字段，插件 UI 不需要自己算表达式）。
    pub fn scheduler_call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_SCHEDULER_CALL,
            json!({ "method": method, "params": params }),
        )
    }

    /// 宿主机 FTP 能力面（manifest 需声明 `ftp` 能力与对应 verbs）。
    ///
    /// 方法：`status` / `install` / `uninstall` / `sites.list` / `accounts.list` /
    /// `account.create` / `account.password` / `account.delete` / `service.restart`。
    /// 返回内核原样 JSON；仅 Native 形态可用，受限形态安装期就不会授予 `ftp`。
    pub fn ftp_call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FTP_CALL,
            json!({ "method": method, "params": params }),
        )
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
    let (method, path, body, protocol) = read_request(stream)?;
    // 协议版本先判：内核与插件不同步时，明确拒绝比让插件按旧语义处理要安全。
    if let Err(e) = skiff_wire::check_protocol(protocol) {
        return write_response(
            stream,
            426,
            &json!({"error": {"code": -32000, "message": e.to_string()}}),
        );
    }
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
fn read_request(
    stream: &mut std::os::unix::net::UnixStream,
) -> Result<(String, String, Vec<u8>, Option<u32>)> {
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
    Ok((head.method, head.path, body, head.protocol))
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
