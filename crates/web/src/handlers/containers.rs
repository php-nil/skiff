use std::collections::HashMap;
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
use skiff_docker::{ObjectRef, Verb};
use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AdminUser, AuthUser};
use crate::state::AppState;
use crate::templates::{
    ContainerDetailPage, ContainerRowView, ContainerRows, ContainersPage, MountView, PortView,
};
use crate::util::{form_first, parse_form};

/// `?filter=error` 只保留异常容器（概览待办深链），其余值视为不筛选。
fn filter_of(q: &HashMap<String, String>) -> String {
    match q.get("filter").map(String::as_str) {
        Some("error") => "error".into(),
        _ => String::new(),
    }
}

fn rows_for(
    containers: Vec<skiff_docker::client::ContainerRow>,
    filter: &str,
) -> Vec<ContainerRowView> {
    containers
        .iter()
        .filter(|c| filter.is_empty() || handlers::container_abnormal(&c.state))
        .map(handlers::container_view)
        .collect()
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let docker_ok = state.docker().available();
    let containers = if docker_ok {
        state
            .docker()
            .list_containers()
            .await
            .map_err(|e| AppError::Unavailable(e.to_string()))?
    } else {
        Vec::new()
    };
    let filter = filter_of(&q);
    let can_operate = user.can(skiff_model::Permission::DockerOperate);
    let page = ContainersPage {
        ctx: handlers::page_ctx(&state, &user, "containers"),
        can_operate,
        can_delete: user.can(skiff_model::Permission::DockerLifecycle),
        containers: rows_for(containers, &filter),
        docker_reason: state.docker_unavailable_reason().to_string(),
        filter,
        error: String::new(),
    };
    handlers::render(page)
}

/// htmx 局部刷新：只回表格行；保留当前筛选。
pub async fn rows(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let containers = if state.docker().available() {
        state.docker().list_containers().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let filter = filter_of(&q);
    let page = ContainerRows {
        ctx: handlers::page_ctx(&state, &user, "containers"),
        can_operate: user.can(skiff_model::Permission::DockerOperate),
        can_delete: user.can(skiff_model::Permission::DockerLifecycle),
        containers: rows_for(containers, &filter),
        filter,
    };
    handlers::render(page)
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let row = handlers::resolve_container(&state, &id).await?;

    // 策略：读详情同样过三层策略（viewer 可读，插件需 grant）
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            Verb::ContainerInspect,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    let inspect = state
        .docker()
        .inspect_container(&row.id)
        .await
        .map_err(|e| AppError::Unavailable(e.to_string()))?;

    let created = inspect
        .created
        .clone()
        .unwrap_or_default()
        .chars()
        .take(19)
        .collect::<String>();
    let cmd = inspect
        .config
        .as_ref()
        .and_then(|c| c.cmd.clone())
        .map(|v| v.join(" "))
        .unwrap_or_default();
    let labels = inspect
        .config
        .as_ref()
        .and_then(|c| c.labels.clone())
        .map(|m| m.into_iter().collect::<Vec<_>>())
        .unwrap_or_default();
    let ports = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.ports.clone())
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let (port, proto) = k.split_once('/').unwrap_or((k.as_str(), "tcp"));
                    let host = v
                        .as_ref()
                        .and_then(|b| b.first())
                        .and_then(|b| b.host_port.clone())
                        .unwrap_or_default();
                    PortView {
                        host,
                        container: port.to_string(),
                        protocol: proto.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mounts = inspect
        .mounts
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| MountView {
                    kind: m.typ.clone().map(|t| t.to_string()).unwrap_or_default(),
                    source: m.source.clone().unwrap_or_default(),
                    target: m.destination.clone().unwrap_or_default(),
                    read_only: m.rw == Some(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let networks = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.networks.clone())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let health = inspect
        .state
        .as_ref()
        .and_then(|s| s.health.as_ref())
        .and_then(|h| h.status)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());

    let page = ContainerDetailPage {
        ctx: handlers::page_ctx(&state, &user, "containers"),
        container: handlers::container_view(&row),
        image: row.image.clone(),
        cmd,
        created,
        ports,
        mounts,
        networks,
        labels,
        health,
        can_exec: user.can(skiff_model::Permission::DockerExec),
        can_operate: user.can(skiff_model::Permission::DockerOperate),
        can_delete: user.can(skiff_model::Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
        error: String::new(),
    };
    handlers::render(page)
}

/// 页面动作 → 动词白名单。未知动作仍是 400，不能落到别的动词上。
fn action_verb(action: &str) -> Result<Verb, AppError> {
    match action {
        "start" => Ok(Verb::Start),
        "stop" => Ok(Verb::Stop),
        "restart" => Ok(Verb::Restart),
        "kill" => Ok(Verb::Kill),
        "remove" => Ok(Verb::ContainerRemove),
        other => Err(AppError::BadRequest(format!("未知动作: {other}"))),
    }
}

pub async fn action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;

    let verb = action_verb(&action)?;

    let row = handlers::resolve_container(&state, &id).await?;
    let obj = handlers::object_ref(&row);

    state
        .cfg
        .policy
        .check(&handlers::actor(&user), verb, Some(&obj))
        .map_err(|e| {
            let _ = state.db().audit(
                &user.user.username,
                &handlers::client_ip(&state, &parts),
                &verb.audit_action(),
                &row.name,
                "deny",
                &e.to_string(),
            );
            AppError::Forbidden(e.to_string())
        })?;

    // BusyGuard：容器有进行中的安装/扩展/重建任务时，删除会打断内核自己的任务。
    if verb == Verb::ContainerRemove {
        if let Some(reason) = state.container_busy_reason(&row.name).await {
            let _ = state.db().audit(
                &user.user.username,
                &handlers::client_ip(&state, &parts),
                &verb.audit_action(),
                &row.name,
                "deny",
                &reason,
            );
            return Err(AppError::Conflict(reason));
        }
    }

    let params = match verb {
        // 删除即销毁；running 容器一并强制移除，不再拆成 stop→remove 两步。
        Verb::ContainerRemove => skiff_docker::DockerParams::ContainerRemove { force: true },
        _ => skiff_docker::DockerParams::None,
    };

    // 分发只在 Docker::execute 一处；这里拿到的 Ok 值（动作类动词为 null）用不上
    let result = state
        .docker()
        .execute(verb, Some(&obj), params)
        .await
        .map(|_| ());

    let ip = handlers::client_ip(&state, &parts);
    match &result {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &verb.audit_action(),
                &format!("{} ({})", row.name, row.short_id()),
                "ok",
                verb.http_path(),
            );
        }
        Err(e) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &verb.audit_action(),
                &row.name,
                "fail",
                &e.to_string(),
            );
        }
    }
    result.map_err(|e| AppError::Unavailable(e.to_string()))?;

    if handlers::htmx_request(&parts) {
        return rows(State(state), AuthUser(user), Query(HashMap::new())).await;
    }
    // 删除后详情页已不存在，回列表而不是落到 404。
    if verb == Verb::ContainerRemove {
        return Ok(handlers::redirect(format!(
            "/{}/containers",
            state.cfg.entry_prefix
        )));
    }
    Ok(handlers::redirect(format!(
        "/{}/containers/{}",
        state.cfg.entry_prefix, row.id
    )))
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
        let _ = db.audit(
            &username,
            "ws",
            &Verb::Exec.audit_action(),
            &cname,
            "start",
            "",
        );
        if let Err(e) = run_exec(docker, cid, socket).await {
            let _ = db.audit(
                &username,
                "ws",
                &Verb::Exec.audit_action(),
                &cname,
                "fail",
                &e,
            );
        } else {
            let _ = db.audit(
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

/// 供 UI 列表用：把 ObjectRef 转回展示名。
pub fn describe(obj: &ObjectRef) -> String {
    format!("{} ({})", obj.name, obj.short_id())
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
}
