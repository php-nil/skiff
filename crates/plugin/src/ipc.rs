//! 插件与内核之间的 IPC：UDS + HTTP/1.1 + JSON-RPC 2.0（§4.2）。
//!
//! 插件只暴露 `GET /healthz` 与 `POST /rpc`；内核回调插件、插件回调内核都走
//! 同一套 JSON-RPC（回调内核的 socket 是 `/run/skiff/kernel.sock`，不开 TCP 端口）。
//!
//! **帧编解码不在本文件**：头部构造、状态行解析、`Content-Length` 等待逻辑、
//! `MAX_FRAME` 全部来自 `skiff-wire`，与插件侧 SDK 共用同一份实现。
//! 这里只留 tokio 的 I/O 循环与错误翻译。

use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use skiff_wire::{routes, WireError};

/// 帧上限（实现唯一来源是 `skiff-wire`；此处转出以保留 `skiff_plugin::ipc::MAX_FRAME` 旧路径）。
pub const MAX_FRAME: usize = skiff_wire::MAX_FRAME;
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("平台不支持 UDS: {0}")]
    Unsupported(String),
    #[error("连接失败: {0}")]
    Connect(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("超时")]
    Timeout,
    #[error("插件返回错误 {code}: {message}")]
    Rpc { code: i64, message: String },
}

impl From<WireError> for IpcError {
    fn from(e: WireError) -> Self {
        match e {
            WireError::Rpc { code, message } => IpcError::Rpc { code, message },
            other => IpcError::Protocol(other.to_string()),
        }
    }
}

pub use skiff_wire::HttpResponse;

#[cfg(unix)]
pub async fn request(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<HttpResponse, IpcError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let fut = async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|e| IpcError::Connect(format!("{}: {e}", socket.display())))?;
        let body = body.unwrap_or(&[]);
        if body.len() > MAX_FRAME {
            return Err(IpcError::Protocol("请求超过单帧上限".into()));
        }
        let head = skiff_wire::build_request(method, path, body.len(), None);
        stream
            .write_all(head.as_bytes())
            .await
            .map_err(|e| IpcError::Protocol(e.to_string()))?;
        if !body.is_empty() {
            stream
                .write_all(body)
                .await
                .map_err(|e| IpcError::Protocol(e.to_string()))?;
        }
        stream
            .flush()
            .await
            .map_err(|e| IpcError::Protocol(e.to_string()))?;

        let mut buf = Vec::with_capacity(4096);
        let mut chunk = [0u8; 8192];
        loop {
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|e| IpcError::Protocol(e.to_string()))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > MAX_FRAME * 2 {
                return Err(IpcError::Protocol("响应超过帧上限".into()));
            }
            if let Some(resp) = skiff_wire::parse_response(&buf)? {
                return Ok(resp);
            }
        }
        // 连接关闭时按已有字节收尾（对端可能未给 Content-Length 就断开）
        skiff_wire::parse_response(&buf)?.ok_or_else(|| IpcError::Protocol("响应不完整".into()))
    };

    match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r,
        Err(_) => Err(IpcError::Timeout),
    }
}

#[cfg(not(unix))]
pub async fn request(
    _socket: &Path,
    _method: &str,
    _path: &str,
    _body: Option<&[u8]>,
    _timeout: Duration,
) -> Result<HttpResponse, IpcError> {
    Err(IpcError::Unsupported(
        "插件进程外隔离依赖 Unix socket".into(),
    ))
}

pub async fn healthz(socket: &Path) -> bool {
    matches!(
        request(socket, "GET", routes::PLUGIN_HEALTHZ, None, Duration::from_secs(2)).await,
        Ok(r) if r.status == 200
    )
}

pub async fn rpc_call(socket: &Path, method: &str, params: Value) -> Result<Value, IpcError> {
    let body = skiff_wire::rpc_request_body(method, &params)?;
    let resp = request(socket, "POST", routes::PLUGIN_RPC, Some(&body), RPC_TIMEOUT).await?;
    if resp.status != 200 {
        return Err(IpcError::Protocol(format!("插件返回 HTTP {}", resp.status)));
    }
    Ok(skiff_wire::rpc_result(&resp.body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthz_false_when_socket_missing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ok = rt.block_on(healthz(Path::new("/nonexistent/plugin.sock")));
        assert!(!ok);
    }

    #[test]
    fn wire_errors_map_to_ipc_errors() {
        let e: IpcError = WireError::Rpc {
            code: -32601,
            message: "nope".into(),
        }
        .into();
        assert!(matches!(e, IpcError::Rpc { code: -32601, .. }));
        let e: IpcError = WireError::BadStatusLine.into();
        assert!(matches!(e, IpcError::Protocol(_)));
    }
}
