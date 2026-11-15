//! 站点/运行环境的健康检查：宿主端口占用探测与容器就绪判定。
//!
//! 这里的函数只看「端口与进程」，不碰站点目录与容器编排；调用方负责在探测
//! 失败后决定阻塞还是降级。
//!
//! 运行环境（Node/Rust）容器使用 host 网络，「端口能连上」并不等于「响应方
//! 是本容器」：宿主上任何进程占住同一端口，探针都会拿到陌生进程的响应。因此
//! 就绪判定必须同时看三件事——容器状态（running / restarting / RestartCount）、
//! 监听套接字的归属（按 `/proc/<pid>/cgroup` 判断是否属于本容器）、以及 HTTP
//! 响应。三者缺一都可能把「端口被占、容器 crash-loop」误判成「操作已完成」。

use std::time::Duration;

use skiff_docker::{ContainerRow, DockerService};

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
}

/// 端口监听者的归属。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListenerOwner {
    /// 端口上没有监听者。
    None,
    /// 监听套接字属于目标容器。
    Container { pid: u32, comm: String },
    /// 监听套接字属于宿主上的其它进程，不是本站点进程。
    Foreign { pid: u32, comm: String },
    /// 无法判定（缺 root、/proc 不可读、容器 id 缺失）。
    Unknown(String),
}

/// 一次就绪判定的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    /// 还没起来（端口未监听 / HTTP 未就绪），值得继续等。
    Pending,
    /// 容器在跑，但端口被别的进程占着。
    WrongOwner {
        pid: u32,
        comm: String,
    },
    /// 容器反复重启，不可能自愈。
    CrashLoop {
        restarts: i64,
    },
    /// 容器已退出且没有继续重启。
    Exited {
        exit_code: i64,
    },
}

/// 就绪等待失败的完整上下文（供调用方拼错误文案）。
#[derive(Debug, Clone)]
pub(crate) struct ReadinessFailure {
    pub verdict: Readiness,
    pub owner: ListenerOwner,
    pub note: Option<String>,
}

/// 端口监听者归属：读取 `/proc/net/tcp*` 拿到监听 socket，再按 cgroup 判断
/// 持有者是否属于目标容器。
pub(crate) fn listener_owner(port: u16, container_id: &str) -> ListenerOwner {
    let inodes = listening_inodes(port);
    if inodes.is_empty() {
        return ListenerOwner::None;
    }
    let targets: Vec<String> = inodes.iter().map(|i| format!("socket:[{i}]")).collect();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return ListenerOwner::Unknown("无法读取 /proc，不能确认端口归属".into());
    };
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
            if !targets
                .iter()
                .any(|t| t.as_str() == target.to_string_lossy().as_ref())
            {
                continue;
            }
            let pid = name.parse().unwrap_or(0);
            let comm = std::fs::read_to_string(entry.path().join("comm"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "未知进程".into());
            return match pid_belongs_to_container(pid, container_id) {
                Some(true) => ListenerOwner::Container { pid, comm },
                Some(false) => ListenerOwner::Foreign { pid, comm },
                None => ListenerOwner::Unknown(format!("监听进程 pid {pid} {comm} 的归属无法判定")),
            };
        }
    }
    ListenerOwner::Unknown("未能解析监听进程（需要 root 读 /proc）".into())
}

/// 进程是否属于指定容器：容器的 cgroup 路径里带完整 id（或前 12 位短 id）。
fn pid_belongs_to_container(pid: u32, container_id: &str) -> Option<bool> {
    if container_id.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let short: String = container_id.chars().take(12).collect();
    Some(text.contains(container_id) || (!short.is_empty() && text.contains(&short)))
}

/// 纯判定：把「容器状态 + 端口归属 + HTTP 结果」翻译成结论。
///
/// `baseline_started`：等待开始后第一次观测到的 `StartedAt`；变化即代表容器重启过。
/// `observed_restarts`：等待窗口内已观察到的重启次数（用于错误文案）。
/// `http_ok`：`None` 表示只做 TCP/端口判定（Rust 等无健康路径的环境）。
fn classify(
    facts: &ContainerFacts,
    baseline_started: &str,
    observed_restarts: i64,
    owner: &ListenerOwner,
    http_ok: Option<bool>,
) -> Readiness {
    let restarted = !baseline_started.is_empty() && facts.started_at != baseline_started;
    if facts.restarting || restarted {
        return Readiness::CrashLoop {
            restarts: observed_restarts.max(1),
        };
    }
    if !facts.running {
        return Readiness::Exited {
            exit_code: facts.exit_code,
        };
    }
    match owner {
        ListenerOwner::None => Readiness::Pending,
        ListenerOwner::Foreign { pid, comm } => Readiness::WrongOwner {
            pid: *pid,
            comm: comm.clone(),
        },
        // 归属无法确认时退化为「容器在跑 + HTTP 可响应」，并由调用方保留 note。
        ListenerOwner::Unknown(_) => match http_ok {
            Some(true) => Readiness::Ready,
            _ => Readiness::Pending,
        },
        ListenerOwner::Container { .. } => match http_ok {
            Some(false) => Readiness::Pending,
            _ => Readiness::Ready,
        },
    }
}

async fn container_facts(
    docker: &DockerService,
    container: &str,
) -> Result<(ContainerFacts, String), String> {
    let info = docker
        .inspect_container(container)
        .await
        .map_err(|e| format!("读取容器 {container} 状态失败：{e}"))?;
    let id = info.id.unwrap_or_default();
    let facts = match info.state {
        Some(state) => ContainerFacts {
            running: state.running.unwrap_or(false),
            restarting: state.restarting.unwrap_or(false),
            exit_code: state.exit_code.unwrap_or(0),
            started_at: state.started_at.unwrap_or_default(),
        },
        None => ContainerFacts::default(),
    };
    Ok((facts, id))
}

/// 轮询等待运行环境就绪。
///
/// - `http_path = Some(p)`：Node 等 Web 应用，要求容器自身的监听端口对 `p`
///   返回 2xx/3xx；`None`：只要求端口由容器自己监听（Rust 等）。
/// - `restart_count` 以**本函数第一次观测到的值**为基准：手动 start/restart
///   可能重置计数，用调用前的旧值当基准会漏判。
/// - 「端口属于别的进程 / crash-loop / 已退出」是终态，连续两次确认后立即返回，
///   避免重启瞬间的瞬时 EADDRINUSE 造成误判。
pub(crate) async fn wait_runtime_ready(
    docker: &DockerService,
    container: &str,
    port: u16,
    http_path: Option<&str>,
    timeout: Duration,
) -> Result<(), ReadinessFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut baseline_started: Option<String> = None;
    let mut prev_started: Option<String> = None;
    let mut observed_restarts = 0i64;
    let mut hits = 0u32;
    let mut last = Readiness::Pending;
    let mut last_owner = ListenerOwner::None;
    let mut note: Option<String> = None;

    loop {
        match container_facts(docker, container).await {
            Ok((facts, id)) => {
                let base = baseline_started
                    .get_or_insert_with(|| facts.started_at.clone())
                    .clone();
                if let Some(prev) = &prev_started {
                    if *prev != facts.started_at {
                        observed_restarts += 1;
                    }
                }
                prev_started = Some(facts.started_at.clone());
                let owner = listener_owner(port, &id);
                let http_ok = match http_path {
                    Some(path)
                        if matches!(
                            owner,
                            ListenerOwner::Container { .. } | ListenerOwner::Unknown(_)
                        ) =>
                    {
                        Some(http_probe_ok(port, path).await)
                    }
                    Some(_) => Some(false),
                    None => None,
                };
                let verdict = classify(&facts, &base, observed_restarts, &owner, http_ok);
                if verdict == Readiness::Ready {
                    return Ok(());
                }
                let terminal = matches!(
                    verdict,
                    Readiness::CrashLoop { .. }
                        | Readiness::WrongOwner { .. }
                        | Readiness::Exited { .. }
                );
                hits = if terminal { hits + 1 } else { 0 };
                if let ListenerOwner::Unknown(msg) = &owner {
                    note = Some(msg.clone());
                }
                last = verdict;
                last_owner = owner;
                if hits >= 2 {
                    return Err(ReadinessFailure {
                        verdict: last,
                        owner: last_owner,
                        note,
                    });
                }
            }
            Err(e) => {
                hits = 0;
                note = Some(e);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ReadinessFailure {
                verdict: last,
                owner: last_owner,
                note,
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 一次性就绪检查（页面「检测」按钮）：不等待，直接给出当前结论。
pub(crate) async fn check_runtime_ready(
    docker: &DockerService,
    container: &str,
    port: u16,
    http_path: Option<&str>,
) -> Result<(), ReadinessFailure> {
    let (facts, id) = container_facts(docker, container)
        .await
        .map_err(|e| ReadinessFailure {
            verdict: Readiness::Pending,
            owner: ListenerOwner::Unknown(e.clone()),
            note: Some(e),
        })?;
    let owner = listener_owner(port, &id);
    let http_ok = match http_path {
        Some(path)
            if matches!(
                owner,
                ListenerOwner::Container { .. } | ListenerOwner::Unknown(_)
            ) =>
        {
            Some(http_probe_ok(port, path).await)
        }
        Some(_) => Some(false),
        None => None,
    };
    let verdict = classify(&facts, &facts.started_at, 0, &owner, http_ok);
    if verdict == Readiness::Ready {
        Ok(())
    } else {
        Err(ReadinessFailure {
            verdict,
            owner,
            note: None,
        })
    }
}

/// 把就绪失败翻译成给管理员看的一句话：谁占了端口、容器处于什么状态。
pub(crate) fn readiness_message(
    subject: &str,
    port: u16,
    http_path: Option<&str>,
    failure: &ReadinessFailure,
) -> String {
    let what = match http_path {
        Some(p) => format!("HTTP {p} 无 2xx/3xx 响应"),
        None => format!("端口 {port} 未监听"),
    };
    match &failure.verdict {
        Readiness::WrongOwner { pid, comm } => format!(
            "宿主端口 {port} 已被占用，占用方不是本站点进程（pid {pid} {comm}）；{subject} 无法监听该端口，请先停止该进程或更换端口"
        ),
        Readiness::CrashLoop { restarts } => {
            let state_text = if *restarts > 0 {
                format!("容器反复重启（本轮观察到 {restarts} 次重启）")
            } else {
                "容器处于重启状态（crash-loop）".to_string()
            };
            match &failure.owner {
                ListenerOwner::Foreign { pid, comm } => format!(
                    "{subject} {state_text}：宿主端口 {port} 已被非本站点进程占用（pid {pid} {comm}），容器无法监听；请先停止该进程后重试"
                ),
                _ => format!("{subject} {state_text}，启动失败；请查看容器日志（docker logs）"),
            }
        }
        Readiness::Exited { exit_code } => match &failure.owner {
            ListenerOwner::Foreign { pid, comm } => format!(
                "{subject} 容器已退出（exit {exit_code}）：宿主端口 {port} 已被非本站点进程占用（pid {pid} {comm}）；请先停止该进程后重试"
            ),
            _ => format!("{subject} 容器已退出（exit {exit_code}），未通过就绪检查；请查看容器日志"),
        },
        Readiness::Pending => match &failure.note {
            Some(note) => format!("{subject} 未通过就绪检查（{what}）：{note}"),
            None => format!("{subject} 未通过就绪检查（{what}），请查看运行环境日志"),
        },
        Readiness::Ready => format!("{subject} 应用就绪"),
    }
}

/// 以 HTTP 探测就绪：Node 进程可能已监听端口但仍在初始化路由，因此比 TCP 更贴近真实可用。
///
/// 只在确认监听套接字属于目标容器之后调用，不能单独当作健康结论。
pub(crate) async fn http_probe_ok(port: u16, path: &str) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let path = if path.trim().is_empty() {
        "/"
    } else {
        path.trim()
    };
    let Ok(mut stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(running: bool, restarting: bool, started_at: &str, exit_code: i64) -> ContainerFacts {
        ContainerFacts {
            running,
            restarting,
            exit_code,
            started_at: started_at.to_string(),
        }
    }

    /// 端口被宿主其它进程占用：即使端口能返回 200，也必须判失败并指出占用者。
    #[test]
    fn foreign_listener_is_not_ready_even_if_http_answers() {
        let owner = ListenerOwner::Foreign {
            pid: 4242,
            comm: "python3".into(),
        };
        let verdict = classify(&facts(true, false, "t0", 0), "t0", 0, &owner, Some(true));
        assert_eq!(
            verdict,
            Readiness::WrongOwner {
                pid: 4242,
                comm: "python3".into()
            }
        );
        let msg = readiness_message(
            "Node.js",
            9200,
            Some("/"),
            &ReadinessFailure {
                verdict,
                owner,
                note: None,
            },
        );
        assert!(msg.contains("不是本站点进程"), "文案未指出占用方：{msg}");
        assert!(msg.contains("pid 4242"), "文案缺少占用进程：{msg}");
    }

    /// crash-loop：容器在重启（或等待期间 StartedAt 变化），不能报「操作已完成」。
    #[test]
    fn crash_loop_is_not_ready() {
        let owner = ListenerOwner::Foreign {
            pid: 4242,
            comm: "python3".into(),
        };
        let verdict = classify(&facts(false, true, "t0", 1), "t0", 3, &owner, Some(true));
        assert_eq!(verdict, Readiness::CrashLoop { restarts: 3 });

        // 等待窗口内 StartedAt 变化 = 容器重启过，同样判 crash-loop。
        let verdict = classify(
            &facts(true, false, "t1", 0),
            "t0",
            1,
            &ListenerOwner::None,
            None,
        );
        assert_eq!(verdict, Readiness::CrashLoop { restarts: 1 });
    }

    /// 正常：容器在跑且监听套接字属于它自己，HTTP 2xx/3xx 才算就绪。
    #[test]
    fn container_owned_listener_is_ready_only_when_http_ok() {
        let owner = ListenerOwner::Container {
            pid: 777,
            comm: "node".into(),
        };
        assert_eq!(
            classify(&facts(true, false, "t0", 0), "t0", 0, &owner, Some(true)),
            Readiness::Ready
        );
        assert_eq!(
            classify(&facts(true, false, "t0", 0), "t0", 0, &owner, Some(false)),
            Readiness::Pending
        );
        // Rust 等只做端口判定的环境。
        assert_eq!(
            classify(&facts(true, false, "t0", 0), "t0", 0, &owner, None),
            Readiness::Ready
        );
    }

    /// 端口尚无监听者时继续等待，不误判成失败。
    #[test]
    fn no_listener_is_pending() {
        assert_eq!(
            classify(
                &facts(true, false, "t0", 0),
                "t0",
                0,
                &ListenerOwner::None,
                Some(false)
            ),
            Readiness::Pending
        );
        assert_eq!(
            classify(
                &facts(false, false, "t0", 0),
                "t0",
                0,
                &ListenerOwner::None,
                None
            ),
            Readiness::Exited { exit_code: 0 }
        );
    }

    /// 归属无法解析时退回「容器在跑 + HTTP 可响应」，但不把 TCP 可达当成就绪。
    #[test]
    fn unknown_owner_needs_http_signal() {
        let owner = ListenerOwner::Unknown("无法读取 /proc".into());
        assert_eq!(
            classify(&facts(true, false, "t0", 0), "t0", 0, &owner, Some(true)),
            Readiness::Ready
        );
        assert_eq!(
            classify(&facts(true, false, "t0", 0), "t0", 0, &owner, None),
            Readiness::Pending
        );
    }

    /// 实际监听套接字的归属解析：测试进程自己的监听端口不能被认作容器端口。
    #[cfg(target_os = "linux")]
    #[test]
    fn listener_owner_does_not_claim_test_process_as_container() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("绑定临时端口");
        let port = listener.local_addr().expect("读取端口").port();
        let owner = listener_owner(
            port,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        assert!(
            !matches!(owner, ListenerOwner::Container { .. }),
            "测试进程不应被判定为容器内进程，实际 {owner:?}"
        );
        drop(listener);
    }
}
