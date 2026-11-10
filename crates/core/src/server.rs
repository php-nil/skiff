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

    if cfg.tls_active {
        let (cert, key) = match (&cfg.tls_cert, &cfg.tls_key) {
            (Some(cert), Some(key)) => (cert, key),
            _ => anyhow::bail!("内部错误：tls_active 为真但证书/私钥缺失"),
        };
        let tls = RustlsConfig::from_pem_file(cert, key)
            .await
            .with_context(|| format!("加载 TLS 证书失败: {}", cert.display()))?;
        info!(%addr, "HTTPS 监听中");
        axum_server::bind_rustls(addr, tls)
            .handle(handle)
            .serve(app)
            .await
            .map_err(|e| serve_error(e, addr))?;
    } else {
        // `validate()` 只放行两种明文：loopback/显式关 TLS（dev_http）与「可信反代终止 TLS」
        // （plaintext_behind_proxy）。这里必须同时认两者，否则后者永远起不来。
        plaintext_allowed(cfg)?;
        if cfg.dev_http {
            warn!(
                %addr,
                "明文 HTTP 监听：Cookie 已降级为 skiff_dev_session（无 __Host-/Secure）；\
                 仅限内网直连或反向代理之后"
            );
        } else {
            warn!(%addr, "明文 HTTP 监听：假定 TLS 已由 trusted_proxies 中的反代终止");
        }
        axum_server::bind(addr)
            .handle(handle)
            .serve(app)
            .await
            .map_err(|e| serve_error(e, addr))?;
    }
    Ok(())
}

/// 无 TLS 时是否允许明文监听。纯判定，不绑端口，便于单测覆盖三条分支。
fn plaintext_allowed(cfg: &Config) -> Result<()> {
    if cfg.dev_http || cfg.plaintext_behind_proxy {
        Ok(())
    } else {
        anyhow::bail!("内部错误：未通过 TLS 校验却进入明文模式")
    }
}

/// 监听失败要给运维能看懂的原因（端口被占是最常见的一种），
/// 不能笼统一句「服务异常退出」就完了。
fn serve_error(e: std::io::Error, addr: SocketAddr) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        anyhow::anyhow!("监听 {addr} 失败：地址已被占用（换 listen 或停掉占用进程）")
    } else {
        anyhow::anyhow!("监听 {addr} 失败: {e}")
    }
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

/// 内核调度器 tick 循环：1s 一轮只做「出票」，执行体在 tick 之外的任务里跑，
/// 因此单个任务耗时不阻塞下一轮（`docs/KERNEL_SCHEDULER_CONTRACT.md` §5）。
pub fn spawn_task_loop(state: Arc<skiff_app::AppState>) {
    let Some(tasks) = state.tasks().cloned() else {
        warn!("任务服务未初始化，调度器 tick 循环未启动");
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // 上一轮晚到不能靠「补拍」追平：错过的触发由调度器自己的 missed 策略处理。
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            tasks.tick_once().await;
        }
    });
}

/// Docker 探活循环：守护进程可能在中途停止/恢复，`available()` 是内存态标志，
/// 由这里每 3 秒刷新一次；页面 gate 文案随可达性出现或消失。
pub fn spawn_docker_health(state: Arc<skiff_app::AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last = state.docker().available();
        loop {
            tick.tick().await;
            let ok = state.docker().probe().await;
            if ok != last {
                if ok {
                    info!("Docker 已恢复，容器功能重新可用");
                } else {
                    warn!(
                        reason = %state.docker_unavailable_reason(),
                        "Docker 不可达：容器相关页面切换为 gate 状态"
                    );
                }
                last = ok;
            }
        }
    });
}

pub fn spawn_supervisor_reaper(state: Arc<skiff_app::AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        // 周期任务全部由内核调度器驱动（`spawn_task_loop`）；这里只做插件回收、
        // 会话清理与站点日志轮转，不再有第二套 cron 执行路径。
        loop {
            tick.tick().await;
            state.supervisor().reap_idle().await;
            if let Err(e) = state.db().purge_expired_sessions() {
                tracing::warn!(error = %e, "清理过期会话失败");
            }
            // 站点日志轮转由内核巡检做（改名 + 让 nginx 重开日志），不依赖宿主 logrotate。
            // 一站一目录：逐个扫描 `/www/sites/<域名>/log/`。
            let sites_dir = state.cfg.web_root.join("sites");
            let rotated =
                skiff_app::sites::rotate_site_logs(&sites_dir, skiff_app::sites::LOG_ROTATE_BYTES)
                    .unwrap_or(0);
            if rotated > 0 {
                // 改名后原 fd 仍指向 `.1`；不重开就会一直写进旧文件、新文件永远为空。
                if let Err(e) = state
                    .docker()
                    .exec_capture(
                        skiff_app::sites::NGINX_CONTAINER,
                        vec!["nginx".into(), "-s".into(), "reopen".into()],
                    )
                    .await
                {
                    warn!(error = %e, "日志轮转后重开 nginx 日志失败，写入仍落在 .1");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    use axum::Router;
    use skiff_docker::{Form, Tier};

    fn test_cfg(listen: SocketAddr) -> Config {
        let tmp = std::env::temp_dir().join(format!("skiff-server-test-{}", std::process::id()));
        Config {
            listen,
            data_dir: tmp.join("data"),
            web_root: tmp.join("www"),
            run_dir: tmp.join("run"),
            config_path: tmp.join("config.toml"),
            docker_socket: "/nonexistent/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Basic,
            tls_cert: None,
            tls_key: None,
            tls_enabled: None,
            allow_insecure_http: true,
            entry_prefix: None,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 1,
            require_initial_password_change: false,
            ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
            ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
            catalog: Default::default(),
            dev_http: true,
            plaintext_behind_proxy: false,
            tls_active: false,
            trash_retention_days: 30,
        }
    }

    /// 明文监听的两条合法路径：loopback 调试（dev_http）与可信反代（plaintext_behind_proxy）。
    /// 未开 TLS 时后者曾被 `serve` 误判，导致「非 loopback 明文」永远起不来。
    #[test]
    fn plaintext_allowed_covers_dev_and_trusted_proxy() {
        let mut cfg = test_cfg("0.0.0.0:18080".parse().unwrap());
        cfg.dev_http = false;
        cfg.plaintext_behind_proxy = false;
        assert!(plaintext_allowed(&cfg).is_err(), "裸奔必须拒绝");

        cfg.dev_http = true;
        assert!(plaintext_allowed(&cfg).is_ok(), "loopback 开发模式应放行");

        cfg.dev_http = false;
        cfg.plaintext_behind_proxy = true;
        assert!(plaintext_allowed(&cfg).is_ok(), "可信反代终止 TLS 应放行");
    }

    /// 端口被占：必须立刻带明确原因返回 Err，不得 panic、不得挂住。
    #[tokio::test]
    async fn occupied_port_reports_address_in_use() {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();

        let res = tokio::time::timeout(
            Duration::from_secs(5),
            serve(&test_cfg(addr), Router::new()),
        )
        .await
        .expect("端口被占应立即返回错误，而不是挂住");

        let err = res.expect_err("端口被占时 serve 必须返回 Err");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("已被占用"),
            "错误信息应指明端口占用，实际: {msg}"
        );
    }
}
