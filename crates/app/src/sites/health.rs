//! 站点/运行环境的健康检查：宿主端口占用探测与容器就绪探针。
//!
//! 这里的函数只看「端口与进程」，不碰站点目录与容器编排；调用方负责在探测
//! 失败后决定阻塞还是降级。

use skiff_docker::ContainerRow;

use super::NGINX_CONTAINER;

/// 只有自家 nginx 容器处于 running 才认为 80 由自己持有。
/// 停止/退出/正在重启的容器不构成「端口已占」，必须重新探测。
pub(crate) fn nginx_container_running(rows: &[ContainerRow]) -> bool {
    rows.iter()
        .any(|c| c.name == NGINX_CONTAINER && c.state == "running")
}

/// 探测宿主端口：绑定失败即视为被占用，并尽力给出占用进程。
pub fn check_port_free(port: u16) -> Result<(), String> {
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(e) => Err(format!(
            "宿主端口 {port} 已被占用（{}），请先停掉占用进程再安装/建站：{e}",
            port_owner(port)
        )),
    }
}

/// 批量探测（目前 80；HTTPS 落地后加 443）。
pub fn ensure_ports_free(ports: &[u16]) -> Result<(), String> {
    for p in ports {
        check_port_free(*p)?;
    }
    Ok(())
}

#[cfg(unix)]
fn port_owner(port: u16) -> String {
    let hex = format!("{port:04X}");
    let mut inodes = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(f) {
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                if cols.len() < 10 {
                    continue;
                }
                if cols[1].rsplit(':').next() == Some(hex.as_str()) && cols[3] == "0A" {
                    inodes.push(cols[9].to_string());
                }
            }
        }
    }
    if inodes.is_empty() {
        return "未找到监听进程（可能在其他网络命名空间）".into();
    }
    let targets: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else {
                    continue;
                };
                if targets
                    .iter()
                    .any(|t| t.as_str() == target.to_string_lossy().as_ref())
                {
                    let comm = std::fs::read_to_string(entry.path().join("comm"))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|_| "未知进程".into());
                    return format!("pid {name} {comm}");
                }
            }
        }
    }
    "未能解析占用进程（需要 root 读 /proc）".into()
}

#[cfg(not(unix))]
fn port_owner(_port: u16) -> String {
    "当前平台无法解析占用进程".into()
}

/// 以 HTTP 探测就绪：Node 进程可能已监听端口但仍在初始化路由，因此比 TCP 更贴近真实可用。
pub(crate) async fn wait_http_ready(port: u16, path: &str, timeout: std::time::Duration) -> bool {
    let path = if path.trim().is_empty() {
        "/"
    } else {
        path.trim()
    };
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if http_probe_ok(port, path).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub(crate) async fn http_probe_ok(port: u16, path: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf)).await;
    let Ok(Ok(n)) = read else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut parts = head.split_whitespace();
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return false;
    };
    version.starts_with("HTTP/") && (code.starts_with('2') || code.starts_with('3'))
}

/// 以 TCP 连接探测端口是否已监听（PHP socket / 一般端口可用性）。
pub(crate) async fn wait_tcp_port(port: u16, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}
