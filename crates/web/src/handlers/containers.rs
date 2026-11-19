use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use axum::Json;
use futures_util::StreamExt;
use serde::Deserialize;
use skiff_docker::{Mount, PortMapping, ResourceLimits, Verb};
use skiff_model::ContainerSpec;
use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{AdminUser, AuthUser};
use crate::state::AppState;
fn parse_number(raw: &str, field: &str) -> Result<i64, AppError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("{field} 需要整数：{raw}")))
}

fn parse_ports(text: &str) -> Result<Vec<PortMapping>, AppError> {
    let mut out = Vec::new();
    for raw in text.split(['\n', ',']) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (addr, protocol) = match line.split_once('/') {
            Some((a, p)) if p == "tcp" || p == "udp" => (a, Some(p.to_string())),
            Some(_) => {
                return Err(AppError::BadRequest(format!(
                    "端口协议只支持 tcp/udp：{line}"
                )));
            }
            None => (line, None),
        };
        let (host, container) = addr.split_once(':').ok_or_else(|| {
            AppError::BadRequest(format!("端口映射需要 host:container 格式：{line}"))
        })?;
        if host.contains(':') {
            return Err(AppError::BadRequest(format!(
                "暂不支持绑定 IP，请只写 host:container：{line}"
            )));
        }
        let host = parse_number(host, "宿主端口")?;
        let container = parse_number(container, "容器端口")?;
        if !(1..=65535).contains(&host) || !(1..=65535).contains(&container) {
            return Err(AppError::BadRequest(format!("端口必须在 1–65535：{line}")));
        }
        let (host, container): (u16, u16) = (host as u16, container as u16);
        out.push(PortMapping {
            host,
            container,
            protocol,
        });
    }
    Ok(out)
}

fn parse_env(text: &str) -> Result<BTreeMap<String, String>, AppError> {
    let mut out = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| AppError::BadRequest(format!("环境变量需要 KEY=VALUE 格式：{line}")))?;
        if k.trim().is_empty() {
            return Err(AppError::BadRequest(format!("环境变量名不能为空：{line}")));
        }
        out.insert(k.trim().to_string(), v.to_string());
    }
    Ok(out)
}

fn parse_mounts(text: &str, volume: bool) -> Result<Vec<Mount>, AppError> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            return Err(AppError::BadRequest(format!(
                "挂载需要 source:target[:ro] 格式：{line}"
            )));
        }
        let source = parts[0].trim();
        let target = parts[1].trim();
        if source.is_empty() || target.is_empty() {
            return Err(AppError::BadRequest(format!("挂载源/目标不能为空：{line}")));
        }
        let read_only = match parts.get(2).map(|s| s.trim()) {
            None | Some("rw") => false,
            Some("ro") => true,
            Some(other) => {
                return Err(AppError::BadRequest(format!(
                    "挂载选项只支持 ro/rw：{line} ({other})"
                )))
            }
        };
        out.push(if volume {
            Mount::Volume {
                source: source.to_string(),
                target: target.to_string(),
                read_only,
            }
        } else {
            Mount::Bind {
                source: source.to_string(),
                target: target.to_string(),
                read_only,
            }
        });
    }
    Ok(out)
}
/// 容器创建字段：SSR 表单与 `/ui/v1` JSON 共用同一套解析与校验。
///
/// 两端字段名与语义一致；JSON 路径直接构造本结构，不再先降级成表单 map。
pub(crate) struct SpecFields {
    pub image: String,
    pub name: String,
    pub restart: String,
    pub binds: String,
    pub volumes: String,
    pub env: String,
    pub ports: String,
    pub networks: Vec<String>,
    pub memory_limit_mb: String,
    pub pids_limit: String,
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_swap_mb: String,
}

fn optional_number_value(raw: &str, field: &str) -> Result<Option<i64>, AppError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_number(raw, field).map(Some)
}

pub(crate) fn spec_from_fields(f: SpecFields) -> Result<ContainerSpec, AppError> {
    if f.image.trim().is_empty() {
        return Err(AppError::BadRequest("镜像不能为空".into()));
    }
    let mut mounts = parse_mounts(&f.binds, false)?;
    mounts.extend(parse_mounts(&f.volumes, true)?);
    Ok(ContainerSpec {
        image: f.image.trim().to_string(),
        name: if f.name.trim().is_empty() {
            None
        } else {
            Some(f.name.trim().to_string())
        },
        command: None,
        entrypoint: None,
        env: parse_env(&f.env)?.into_iter().collect(),
        labels: BTreeMap::new(),
        ports: parse_ports(&f.ports)?,
        restart: if f.restart.trim().is_empty() {
            None
        } else {
            Some(f.restart.trim().to_string())
        },
        healthcheck: None,
        networks: f.networks,
        network_mode: None,
        network_ip: None,
        mounts,
        log_driver: None,
        log_max_size: None,
        memory_limit_mb: optional_number_value(&f.memory_limit_mb, "内存上限")?,
        nano_cpus: None,
        pids_limit: optional_number_value(&f.pids_limit, "PIDs 上限")?,
        cpu_shares: optional_number_value(&f.cpu_shares, "CPU 份额")?,
        cpu_period: optional_number_value(&f.cpu_period, "CPU 周期")?,
        cpu_quota: optional_number_value(&f.cpu_quota, "CPU 配额")?,
        memory_swap_mb: optional_number_value(&f.memory_swap_mb, "内存交换")?,
    })
}
/// 资源限制字段：与 SSR 表单同一语义（空串 = 0）。
pub(crate) struct LimitFields {
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
}

pub(crate) fn limits_from_fields(f: LimitFields) -> Result<ResourceLimits, AppError> {
    let field = |raw: &str, label: &str| -> Result<Option<i64>, AppError> {
        if raw.trim().is_empty() {
            return Ok(Some(0));
        }
        parse_number(raw, label).map(Some)
    };
    Ok(ResourceLimits {
        cpu_shares: field(&f.cpu_shares, "CPU 份额")?,
        cpu_period: field(&f.cpu_period, "CPU 周期")?,
        cpu_quota: field(&f.cpu_quota, "CPU 配额")?,
        memory_limit_mb: field(&f.memory_limit_mb, "内存上限")?,
        memory_swap_mb: field(&f.memory_swap_mb, "内存交换")?,
        pids_limit: field(&f.pids_limit, "PIDs 上限")?,
    })
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::containers::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;

/// Docker 命令错误映射：唯一实现在 [`crate::error::map_docker_cmd`]。
pub(crate) use crate::error::map_docker_cmd as map_err;

/// 页面动作 → 动词白名单。未知动作仍是 400，不能落到别的动词上。
pub(crate) fn action_verb(action: &str) -> Result<Verb, AppError> {
    match action {
        "start" => Ok(Verb::Start),
        "stop" => Ok(Verb::Stop),
        "restart" => Ok(Verb::Restart),
        "kill" => Ok(Verb::Kill),
        // `delete` 是规范动词；`remove` 为兼容旧链接保留。
        "delete" | "remove" => Ok(Verb::ContainerRemove),
        other => Err(AppError::BadRequest(format!("未知动作: {other}"))),
    }
}
/// 日志流（SSE）。断连即 drop 流，Docker 侧连接随之取消（§6.2）。
pub async fn logs_sse(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let row = handlers::resolve_container(&state, &id).await?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            Verb::ContainerLogs,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    let stream = state.docker().stream_logs(row.id.clone(), 200).map(|item| {
        let ev = match item {
            Ok(line) => Event::default().data(line.trim_end()),
            Err(e) => Event::default().event("error").data(e.to_string()),
        };
        Ok::<_, Infallible>(ev)
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, Deserialize)]
pub struct ExecTicketQuery {
    pub ticket: Option<String>,
}

/// 签发 exec 单连票据（§6.2）：admin + CSRF + 动词策略三重前置。
/// 票据绑定当前会话与该容器，30 秒有效、用一次即废；明文只在响应体里出现一次。
pub async fn exec_ticket(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<serde_json::Value>, AppError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let row = handlers::resolve_container(&state, &id).await?;
    check_exec_allowed(&state, &user, &row)?;
    let (ticket, expires_in) = state.issue_exec_ticket(&user.session_id, &row.id).await;
    Ok(Json(serde_json::json!({
        "ticket": ticket,
        "expires_in": expires_in,
        "container": row.id,
    })))
}

/// exec 的动词策略判定：签发票据与 WS 握手共用，避免两处各写一遍 `map_err`。
fn check_exec_allowed(
    state: &AppState,
    user: &crate::state::CurrentUser,
    row: &skiff_docker::client::ContainerRow,
) -> Result<(), AppError> {
    state
        .cfg
        .policy
        .check(
            &handlers::actor(user),
            Verb::Exec,
            Some(&handlers::object_ref(row)),
        )
        .map_err(|e| AppError::Forbidden(e.to_string()))
}

/// exec 的授权闸门：动词策略 + 单连票据。
///
/// 抽成公开函数是为了能在**没有 Docker 的环境**里直接验证「无票据 / 重放 / 跨容器」
/// 都被拒——WS 升级本身要在真连接上测，闸门却不必。
pub async fn authorize_exec(
    state: &AppState,
    user: &crate::state::CurrentUser,
    row: &skiff_docker::client::ContainerRow,
    ticket: Option<&str>,
) -> Result<(), AppError> {
    check_exec_allowed(state, user, row)?;
    let Some(ticket) = ticket.filter(|t| !t.is_empty()) else {
        return Err(AppError::Forbidden("缺少 exec 票据，请重新打开终端".into()));
    };
    state
        .consume_exec_ticket(ticket, &user.session_id, &row.id)
        .await
        .map_err(|e| AppError::Forbidden(format!("exec 票据校验失败：{e}")))
}

/// 容器终端（WS → exec hijack 流）。
///
/// 两道门：`Origin` 必须与面板预期 origin 完全一致；`?ticket=` 必须是一张未被用过、
/// 未过期、且绑定当前会话与该容器的单连票据（§6.2）。票据由
/// `POST /{prefix}/containers/{id}/exec/ticket` 在 HTTP 侧签发，WS 本身不再接受
/// 「只有 Cookie 就能连」的请求。
pub async fn exec_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(query): Query<ExecTicketQuery>,
    parts: Parts,
) -> Result<Response, AppError> {
    let host = handlers::host_header(&parts);
    let origin = parts
        .headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if !handlers::ws_origin_allowed(
        origin.as_deref(),
        &host,
        state.cfg.external_url.as_deref(),
        state.cfg.dev_http,
    ) {
        return Err(AppError::Forbidden("WebSocket Origin 校验失败".into()));
    }

    let row = handlers::resolve_container(&state, &id).await?;
    authorize_exec(&state, &user, &row, query.ticket.as_deref()).await?;

    let username = user.user.username.clone();
    let db = state.db().clone();
    let docker = state.docker().clone();
    let cid = row.id.clone();
    let cname = row.name.clone();

    Ok(ws.on_upgrade(move |socket| async move {
        handlers::audit_db(
            &db,
            &username,
            "ws",
            &Verb::Exec.audit_action(),
            &cname,
            "start",
            "",
        );
        if let Err(e) = run_exec(docker, cid, socket).await {
            handlers::audit_db(
                &db,
                &username,
                "ws",
                &Verb::Exec.audit_action(),
                &cname,
                "fail",
                &e,
            );
        } else {
            handlers::audit_db(
                &db,
                &username,
                "ws",
                &Verb::Exec.audit_action(),
                &cname,
                "end",
                "",
            );
        }
    }))
}

async fn run_exec(
    docker: Arc<skiff_docker::DockerService>,
    container: String,
    mut socket: WebSocket,
) -> Result<(), String> {
    let exec_id = docker
        .create_exec(&container, vec!["/bin/sh".to_string()])
        .await
        .map_err(|e| e.to_string())?;
    let started = docker
        .start_exec(&exec_id)
        .await
        .map_err(|e| e.to_string())?;
    match started {
        skiff_docker::bollard::exec::StartExecResults::Attached {
            mut output,
            mut input,
        } => {
            loop {
                tokio::select! {
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(Message::Text(t))) => {
                                input.write_all(t.as_bytes()).await.map_err(|e| e.to_string())?;
                            }
                            Some(Ok(Message::Binary(b))) => {
                                input.write_all(&b).await.map_err(|e| e.to_string())?;
                            }
                            Some(Ok(Message::Close(_))) | None => break,
                            _ => {}
                        }
                    }
                    out = output.next() => {
                        match out {
                            Some(Ok(chunk)) => {
                                let text = chunk.to_string();
                                if socket.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                            Some(Err(e)) => return Err(e.to_string()),
                            None => break,
                        }
                    }
                }
            }
            Ok(())
        }
        skiff_docker::bollard::exec::StartExecResults::Detached => {
            Err("exec 未进入 attach 模式".into())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_is_a_known_container_action() {
        assert_eq!(action_verb("remove").unwrap(), Verb::ContainerRemove);
        for action in ["start", "stop", "restart", "kill"] {
            assert!(action_verb(action).is_ok(), "{action} 应保持可用");
        }
        assert!(action_verb("nope").is_err(), "未知动作仍要 400");
    }

    #[test]
    fn ports_accept_host_container_and_protocol() {
        let got = parse_ports("8080:80\n53:53/udp").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].host, got[0].container), (8080, 80));
        assert_eq!(got[1].protocol.as_deref(), Some("udp"));

        for bad in [
            "8080",
            "127.0.0.1:8080:80",
            "8080:80/sctp",
            "0:80",
            "70000:80",
        ] {
            assert!(parse_ports(bad).is_err(), "{bad} 应被拒绝");
        }
    }

    #[test]
    fn env_rejects_lines_without_equals() {
        let got = parse_env("A=1\n# 注释\n\nB=x=y").unwrap();
        assert_eq!(got.get("A").map(String::as_str), Some("1"));
        assert_eq!(got.get("B").map(String::as_str), Some("x=y"));
        assert!(parse_env("NO_EQUALS").is_err());
        assert!(parse_env("=v").is_err());
    }

    #[test]
    fn mounts_keep_bind_and_volume_apart() {
        let binds = parse_mounts("/data/logs:/var/log:ro", false).unwrap();
        assert!(matches!(
            &binds[0],
            Mount::Bind {
                read_only: true,
                ..
            }
        ));
        let volumes = parse_mounts("html:/usr/share/nginx/html", true).unwrap();
        assert!(matches!(
            &volumes[0],
            Mount::Volume {
                read_only: false,
                ..
            }
        ));
        assert!(parse_mounts("only-source", true).is_err());
        assert!(
            parse_mounts("a:b:rw:z", true).is_err(),
            "多余的挂载选项要拒绝"
        );
    }
}
