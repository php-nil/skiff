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
                .map_err(|e| serve_error(e, addr))?;
        }
        _ => {
            // `validate()` 只放行两种明文：loopback（dev_http）与「可信反代终止 TLS」
            // （plaintext_behind_proxy）。这里必须同时认两者，否则后者永远起不来。
            plaintext_allowed(cfg)?;
            if cfg.dev_http {
                warn!(%addr, "明文 HTTP 开发模式（仅 loopback）：Cookie 无 Secure 前缀");
            } else {
                warn!(%addr, "明文 HTTP 监听：假定 TLS 已由 trusted_proxies 中的反代终止");
            }
            axum_server::bind(addr)
                .handle(handle)
                .serve(app)
                .await
                .map_err(|e| serve_error(e, addr))?;
        }
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

pub fn spawn_supervisor_reaper(state: Arc<skiff_app::AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        // 插件 cron 的去重表：`{id}|{expr}` → 已触发的分钟序号。
        // 30s 一次的巡检如果不去重，`* * * * *` 会变成每分钟两次。
        let mut last_fired: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        // 旧 manifest `cron` 已弃用（契约 §12）：只回一条 deprecation 审计，
        // 不按轮次刷屏。guard/monitor 尚未迁移到任务模型，读取兼容暂时保留。
        let mut cron_deprecated: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        loop {
            tick.tick().await;
            state.supervisor().reap_idle().await;
            let _ = state.db().purge_expired_sessions();
            // 站点日志轮转由内核巡检做（copytruncate），不依赖宿主 logrotate。
            // 一站一目录：逐个扫描 `/www/sites/<域名>/log/`。
            let sites_dir = state.cfg.web_root.join("sites");
            let _ =
                skiff_app::sites::rotate_site_logs(&sites_dir, skiff_app::sites::LOG_ROTATE_BYTES);
            run_cron(
                &state,
                &mut last_fired,
                &mut cron_deprecated,
                chrono::Local::now().naive_local(),
            )
            .await;
        }
    });
}

/// 分钟序号（去重键的一部分）：同一分钟内重复巡检只触发一次。
fn minute_index(t: &chrono::NaiveDateTime) -> i64 {
    t.and_utc().timestamp() / 60
}

/// 单轮 cron 调度：按 manifest 的 `cron` 表达式调用插件 `tick`。
///
/// **过渡态**：契约 §12 要求由内核任务模型接管并移除 `run_cron`；本轮只把 `cron`
/// 插件从 manifest 里摘掉，`guard`/`monitor` 尚未迁移，故保留读取兼容并对旧字段
/// 记录一次性 deprecation 审计（`cron_deprecated` 去重）。
pub async fn run_cron(
    state: &skiff_app::AppState,
    last_fired: &mut std::collections::HashMap<String, i64>,
    cron_deprecated: &mut std::collections::HashSet<String>,
    now: chrono::NaiveDateTime,
) {
    let minute = minute_index(&now);
    let Ok(recs) = state.db().list_plugins() else {
        return;
    };
    for rec in recs {
        if !rec.enabled {
            continue;
        }
        let Ok(manifest) = skiff_plugin::install::manifest_of(&rec) else {
            continue;
        };
        if !manifest.cron.is_empty() && cron_deprecated.insert(rec.id.clone()) {
            let _ = state.db().audit(
                "system",
                "cron",
                "plugin.cron_deprecated",
                &rec.id,
                "ok",
                "manifest `cron` 已弃用，等待迁移到内核任务模型",
            );
        }
        for expr in manifest.cron.iter() {
            if !skiff_cron::expr_matches(expr, &now) {
                continue;
            }
            let key = format!("{}|{expr}", rec.id);
            if last_fired.get(&key).copied() == Some(minute) {
                continue;
            }
            // 先记账再调用：单次 tick 若超过一分钟，下一轮也不会补发一次。
            last_fired.insert(key, minute);

            let params = serde_json::json!({ "cron": expr, "minute": minute });
            let call = state.supervisor().call(&rec, "tick", params);
            // 拉起 + 健康检查本身要 5s，再给一次调用留出余量。
            match tokio::time::timeout(Duration::from_secs(30), call).await {
                Ok(Ok(_)) => {
                    let _ = state
                        .db()
                        .audit("system", "cron", "plugin.cron", &rec.id, "ok", expr);
                }
                Ok(Err(e)) => {
                    let _ = state.db().audit(
                        "system",
                        "cron",
                        "plugin.cron",
                        &rec.id,
                        "fail",
                        &format!("{expr}: {e}"),
                    );
                }
                Err(_) => {
                    let _ = state.db().audit(
                        "system",
                        "cron",
                        "plugin.cron",
                        &rec.id,
                        "fail",
                        &format!("{expr}: 30s 超时"),
                    );
                }
            }
        }
    }
    // 避免长期运行后表里堆积已卸载插件的键。
    last_fired.retain(|_, m| minute - *m <= 1440);
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
            allow_insecure_http: true,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 1,
            require_initial_password_change: false,
            dev_http: true,
            plaintext_behind_proxy: false,
        }
    }

    /// 30s 巡检同一分钟内只该产生一个去重键，否则每分钟会触发两次。
    #[test]
    fn minute_index_groups_a_whole_minute() {
        let base = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        let a = base;
        let b = base + chrono::Duration::seconds(29);
        let c = base + chrono::Duration::seconds(60);
        assert_eq!(minute_index(&a), minute_index(&b));
        assert_ne!(minute_index(&a), minute_index(&c));
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
