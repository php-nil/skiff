//! 站点/运行环境的健康检查：容器状态与容器地址可达性。
//!
//! 运行环境容器接入专用 bridge，宿主上不存在它们的端口；站点 vhost 只允许指向
//! 内核静态分配的容器地址。因此就绪判定只需要三件事——容器在跑、容器实际地址
//! 与登记地址一致、从宿主访问 `<地址>:<端口>` 能得到正常响应。
//!
//! 这里不再读取 `/proc` 判断端口归属：端口归属不再是安全边界，地址本身就是边界。

use std::time::Duration;

use skiff_docker::{ContainerRow, DockerService};

use super::{NGINX_CONTAINER, SITE_NETWORK};

/// 只有自家 nginx 容器处于 running 才认为 80 由自己持有。
/// 停止/退出/正在重启的容器不构成「端口已占」，必须重新探测。
pub(crate) fn nginx_container_running(rows: &[ContainerRow]) -> bool {
    rows.iter()
        .any(|c| c.name == NGINX_CONTAINER && c.state == "running")
}

/// 探测宿主端口：绑定失败即视为被占用，并尽力给出占用进程。
///
/// 只用于 nginx 的 80/443 与安装前的宿主端口检查；运行环境端口已不再落到宿主。
pub fn check_port_free(port: u16) -> Result<(), String> {
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            drop(l);
            Ok(())
        }
        Err(e) => Err(format!(
            "宿主端口 {port} 已被占用（{}），请先停掉占用进程再继续：{e}",
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

fn port_owner(port: u16) -> String {
    let inodes = listening_inodes(port);
    if inodes.is_empty() {
        return "未找到监听进程（可能在其他网络命名空间）".into();
    }
    match pid_for_inodes(&inodes) {
        Some((pid, comm)) => format!("pid {pid} {comm}"),
        None => "未能解析占用进程（需要 root 读 /proc）".into(),
    }
}

/// 解析 `/proc/net/tcp*`，返回监听指定端口的 socket inode 列表。
fn listening_inodes(port: u16) -> Vec<String> {
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
    inodes
}

/// 在 `/proc/<pid>/fd` 里找出持有这些 socket inode 的进程。
fn pid_for_inodes(inodes: &[String]) -> Option<(u32, String)> {
    let targets: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    let entries = std::fs::read_dir("/proc").ok()?;
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
                let pid = name.parse().unwrap_or(0);
                let comm = std::fs::read_to_string(entry.path().join("comm"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "未知进程".into());
                return Some((pid, comm));
            }
        }
    }
    None
}

/// 容器状态快照（inspect 精简，只取就绪判定需要的字段）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContainerFacts {
    pub running: bool,
    pub restarting: bool,
    pub exit_code: i64,
    /// 最近一次启动时间（RFC3339）。等待期间变化即说明容器重启过。
    pub started_at: String,
    /// 容器在站点专用网络上的实际地址。
    pub ip: String,
}

/// 一次就绪判定的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    /// 还没起来（容器刚创建 / 地址未分配 / 端口未响应），值得继续等。
    Pending,
    /// 容器反复重启，不可能自愈。
    CrashLoop {
        restarts: i64,
    },
    /// 容器已退出且没有继续重启。
    Exited {
        exit_code: i64,
    },
    /// 容器实际地址与登记地址不一致：绝不按实际地址改写站点，避免指到别的容器。
    AddressChanged {
        expected: String,
        actual: String,
    },
}

/// 就绪等待失败的完整上下文（供调用方拼错误文案）。
#[derive(Debug, Clone)]
pub(crate) struct ReadinessFailure {
    pub verdict: Readiness,
    /// 最后一次探测的补充说明（超时、连接失败等）。
    pub note: Option<String>,
}

/// 纯判定：把「容器状态 + 地址归属 + 可达性」翻译成结论。
///
/// `baseline_started`：等待开始后第一次观测到的 `StartedAt`；变化即代表容器重启过。
/// `observed_restarts`：等待窗口内已观察到的重启次数（用于错误文案）。
fn classify(
    facts: &ContainerFacts,
    baseline_started: &str,
    observed_restarts: i64,
    expected_ip: &str,
    reachable: bool,
) -> Readiness {
    let restarted = !baseline_started.is_empty() && facts.started_at != baseline_started;
    if facts.restarting || restarted {
        return Readiness::CrashLoop {
            restarts: observed_restarts.max(1),
        };
    }
    if !facts.running {
        // 刚创建、尚未启动的容器没有 StartedAt：给它一个轮询窗口，别当成已退出。
        if facts.started_at.is_empty() {
            return Readiness::Pending;
        }
        return Readiness::Exited {
            exit_code: facts.exit_code,
        };
    }
    if facts.ip.is_empty() {
        return Readiness::Pending;
    }
    if !expected_ip.is_empty() && facts.ip != expected_ip {
        return Readiness::AddressChanged {
            expected: expected_ip.to_string(),
            actual: facts.ip.clone(),
        };
    }
    if reachable {
        Readiness::Ready
    } else {
        Readiness::Pending
    }
}

async fn container_facts(
    docker: &DockerService,
    container: &str,
) -> Result<ContainerFacts, String> {
    let info = docker
        .inspect_container(container)
        .await
        .map_err(|e| format!("读取容器 {container} 状态失败：{e}"))?;
    let state = info.state.unwrap_or_default();
    let ip = info
        .network_settings
        .and_then(|n| n.networks)
        .and_then(|mut nets| nets.remove(SITE_NETWORK))
        .map(|e| e.ip_address.unwrap_or_default())
        .unwrap_or_default();
    Ok(ContainerFacts {
        running: state.running.unwrap_or(false),
        restarting: state.restarting.unwrap_or(false),
        exit_code: state.exit_code.unwrap_or(0),
        started_at: state.started_at.unwrap_or_default(),
        ip,
    })
}

/// 从宿主向容器地址发起 HTTP 探测：2xx/3xx 视为正常响应。
pub(crate) async fn http_probe_ok(ip: &str, port: u16, path: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let connect = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect((ip, port)),
    )
    .await;
    let Ok(Ok(mut stream)) = connect else {
        return false;
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: {ip}\r\nConnection: close\r\n\r\n");
    if tokio::time::timeout(Duration::from_secs(2), stream.write_all(req.as_bytes()))
        .await
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buf)).await
    else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    matches!(status, Some(200..=399))
}

/// TCP 可达性（Rust 等没有健康路径的环境）。
async fn tcp_probe_ok(ip: &str, port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect((ip, port)),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}

async fn reachable(ip: &str, port: u16, http_path: Option<&str>) -> bool {
    match http_path {
        Some(path) => http_probe_ok(ip, port, path).await,
        None => tcp_probe_ok(ip, port).await,
    }
}

/// 轮询等待运行环境就绪。
///
/// - `ip`：内核登记的静态容器地址；实际地址不一致直接终态失败。
/// - `http_path = Some(p)`：Node 等 Web 应用要求返回 2xx/3xx；`None`：只要求 TCP 可达。
/// - 「crash-loop / 已退出 / 地址变化」是终态，连续两次确认后立即返回，避免重启
///   瞬间的瞬时失败造成误判。
pub(crate) async fn wait_runtime_ready(
    docker: &DockerService,
    container: &str,
    ip: &str,
    port: u16,
    http_path: Option<&str>,
    timeout: Duration,
) -> Result<(), ReadinessFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut baseline_started: Option<String> = None;
    let mut prev_started: Option<String> = None;
    let mut observed_restarts = 0i64;
    let mut hits = 0u32;
    let mut last: Option<Readiness> = None;

    loop {
        match container_facts(docker, container).await {
            Ok(facts) => {
                let base = baseline_started
                    .get_or_insert_with(|| facts.started_at.clone())
                    .clone();
                if let Some(prev) = &prev_started {
                    if *prev != facts.started_at {
                        observed_restarts += 1;
                    }
                }
                prev_started = Some(facts.started_at.clone());
                let ok = facts.running
                    && !facts.restarting
                    && facts.ip == ip
                    && reachable(ip, port, http_path).await;
                let verdict = classify(&facts, &base, observed_restarts, ip, ok);
                if verdict == Readiness::Ready {
                    return Ok(());
                }
                if matches!(
                    verdict,
                    Readiness::CrashLoop { .. }
                        | Readiness::Exited { .. }
                        | Readiness::AddressChanged { .. }
                ) {
                    hits += 1;
                    if hits >= 2 {
                        return Err(ReadinessFailure {
                            verdict,
                            note: None,
                        });
                    }
                } else {
                    hits = 0;
                }
                last = Some(verdict);
            }
            Err(note) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(ReadinessFailure {
                        verdict: Readiness::Pending,
                        note: Some(note),
                    });
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ReadinessFailure {
                verdict: last.unwrap_or(Readiness::Pending),
                note: Some("等待超时".into()),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 一次性就绪检查（页面「检测」按钮）。
pub(crate) async fn check_runtime_ready(
    docker: &DockerService,
    container: &str,
    ip: &str,
    port: u16,
    http_path: Option<&str>,
) -> Result<(), ReadinessFailure> {
    let facts = container_facts(docker, container)
        .await
        .map_err(|note| ReadinessFailure {
            verdict: Readiness::Pending,
            note: Some(note),
        })?;
    let ok = facts.running
        && !facts.restarting
        && !facts.ip.is_empty()
        && facts.ip == ip
        && reachable(ip, port, http_path).await;
    match classify(&facts, "", 0, ip, ok) {
        Readiness::Ready => Ok(()),
        verdict => Err(ReadinessFailure {
            verdict,
            note: None,
        }),
    }
}

/// 把判定结论翻译成面向用户的错误文案。
pub(crate) fn readiness_message(
    subject: &str,
    ip: &str,
    port: u16,
    http_path: Option<&str>,
    failure: &ReadinessFailure,
) -> String {
    let target = if http_path.is_some() {
        format!("http://{ip}:{port}{}", http_path.unwrap_or(""))
    } else {
        format!("{ip}:{port}")
    };
    match &failure.verdict {
        Readiness::Ready => format!("{subject} 已就绪"),
        Readiness::CrashLoop { restarts } => format!(
            "{subject} 容器反复重启（已观察到 {restarts} 次），未通过健康检查；请查看容器日志（EADDRINUSE 等启动错误会在这里暴露）"
        ),
        Readiness::Exited { exit_code } => {
            format!("{subject} 容器已退出（exit code {exit_code}），站点不会转发到任何其它进程")
        }
        Readiness::AddressChanged { expected, actual } => format!(
            "{subject} 容器实际地址 {actual} 与登记地址 {expected} 不一致；站点不会改指其它地址，请在运行环境页重建后再启用站点"
        ),
        Readiness::Pending => match &failure.note {
            Some(note) => format!(
                "{subject} 容器在跑，但 {target} 未返回正常响应（{note}）；已自动进入维护态，恢复后自动回到正常服务"
            ),
            None => format!(
                "{subject} 容器在跑，但 {target} 未返回正常响应；已自动进入维护态，恢复后自动回到正常服务"
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(running: bool, restarting: bool, ip: &str) -> ContainerFacts {
        ContainerFacts {
            running,
            restarting,
            exit_code: 0,
            started_at: "2026-01-01T00:00:00Z".into(),
            ip: ip.into(),
        }
    }

    #[test]
    fn classify_crash_loop_wins() {
        let f = facts(true, true, "172.18.16.2");
        assert!(matches!(
            classify(&f, "", 0, "172.18.16.2", true),
            Readiness::CrashLoop { .. }
        ));
    }

    #[test]
    fn classify_exited() {
        let mut f = facts(false, false, "");
        f.exit_code = 1;
        assert!(matches!(
            classify(&f, "", 0, "172.18.16.2", false),
            Readiness::Exited { exit_code: 1 }
        ));
    }

    #[test]
    fn classify_not_started_yet_is_pending() {
        let mut f = facts(false, false, "");
        f.started_at.clear();
        assert_eq!(
            classify(&f, "", 0, "172.18.16.2", false),
            Readiness::Pending
        );
    }

    #[test]
    fn classify_address_change_is_terminal() {
        let f = facts(true, false, "172.18.16.9");
        assert_eq!(
            classify(&f, "", 0, "172.18.16.2", true),
            Readiness::AddressChanged {
                expected: "172.18.16.2".into(),
                actual: "172.18.16.9".into(),
            }
        );
    }

    #[test]
    fn classify_pending_until_probe_ok() {
        let f = facts(true, false, "172.18.16.2");
        assert_eq!(
            classify(&f, "", 0, "172.18.16.2", false),
            Readiness::Pending
        );
        assert_eq!(classify(&f, "", 0, "172.18.16.2", true), Readiness::Ready);
    }

    #[test]
    fn allocate_ip_skips_used_addresses() {
        let used = vec!["172.18.16.1".to_string(), "172.18.16.2".to_string()];
        assert_eq!(
            super::super::allocate_container_ip(&used).as_deref(),
            Some("172.18.16.3")
        );
    }

    #[test]
    fn static_pool_bounds() {
        assert!(super::super::is_static_pool_ip("172.18.16.1"));
        assert!(super::super::is_static_pool_ip("172.18.31.254"));
        assert!(!super::super::is_static_pool_ip("172.18.0.5"));
        assert!(!super::super::is_static_pool_ip("172.18.16.0"));
        assert!(!super::super::is_static_pool_ip("10.0.0.1"));
    }
}
