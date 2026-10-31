//! HTTP(S) 服务：axum-server + rustls(ring)；非 loopback 无 TLS 直接拒绝启动（§6.2）。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use axum_server::Handle;
use tokio::signal;
use tracing::{info, warn};

use crate::config::Config;

pub async fn serve(cfg: &Config, router: axum::Router) -> Result<()> {
    let addr: SocketAddr = cfg.listen;
    let handle = Handle::new();

    {
        let handle = handle.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            info!("收到停机信号，开始优雅退出（10s 截止）");
            handle.graceful_shutdown(Some(Duration::from_secs(10)));
        });
    }

    let app = router.into_make_service_with_connect_info::<SocketAddr>();

    match (&cfg.tls_cert, &cfg.tls_key) {
        (Some(cert), Some(key)) => {
            let tls = RustlsConfig::from_pem_file(cert, key)
                .await
                .with_context(|| format!("加载 TLS 证书失败: {}", cert.display()))?;
            info!(%addr, "HTTPS 监听中");
            axum_server::bind_rustls(addr, tls)
                .handle(handle)
                .serve(app)
                .await
                .context("HTTPS 服务异常退出")?;
        }
        _ => {
            if !cfg.dev_http {
                anyhow::bail!("内部错误：未通过 TLS 校验却进入明文模式");
            }
            warn!(%addr, "明文 HTTP 开发模式（仅 loopback）：Cookie 无 Secure 前缀");
            axum_server::bind(addr)
                .handle(handle)
                .serve(app)
                .await
                .context("HTTP 服务异常退出")?;
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = signal::unix::signal(signal::unix::SignalKind::terminate()).ok();
        if let Some(t) = term.as_mut() {
            tokio::select! {
                _ = signal::ctrl_c() => {},
                _ = t.recv() => {},
            }
            return;
        }
    }
    let _ = signal::ctrl_c().await;
}

/// 供 `skiff health` 使用的最小 HTTP 探测（不引 HTTP 客户端依赖）。
pub fn probe(addr: SocketAddr) -> Result<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .with_context(|| format!("连接 {addr} 失败"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let host = addr.ip().to_string();
    let req = format!("GET /healthz HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    Ok(buf)
}

pub fn spawn_supervisor_reaper(state: Arc<skiff_app::AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            state.supervisor.reap_idle().await;
            let _ = state.db.purge_expired_sessions();
        }
    });
}
