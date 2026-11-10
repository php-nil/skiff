use std::collections::{BTreeMap, HashMap};
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
use skiff_app::commands::docker::{
    self as docker_cmd, ContainerAction, ContainerTarget, CreateContainer, DockerCommandError,
    UpdateContainer,
};
use skiff_app::commands::Actor;
use skiff_docker::{Mount, ObjectRef, PortMapping, ResourceLimits, Verb};
use skiff_model::ContainerSpec;
use tokio::io::AsyncWriteExt;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AdminUser, AuthUser};
use crate::state::AppState;
use crate::templates::{
    ContainerDetailPage, ContainerFormView, ContainerNewPage, ContainerRowView, ContainerRows,
    ContainersPage, MountView, PortView, ResourceLimitsForm,
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

    render_detail(&state, &user, &row, String::new()).await
}

/// 详情页渲染：GET 与「资源限制」提交失败共用，失败时保留错误提示。
async fn render_detail(
    state: &AppState,
    user: &crate::state::CurrentUser,
    row: &skiff_docker::client::ContainerRow,
    error: String,
) -> Result<Response, AppError> {
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

    let hc = inspect.host_config.unwrap_or_default();
    let mb = |bytes: Option<i64>| match bytes {
        Some(v) if v < 0 => "不限制".to_string(),
        Some(0) | None => "默认".to_string(),
        Some(v) => format!("{} MB", v / (1024 * 1024)),
    };
    let effective = vec![
        (
            "CPU 份额".to_string(),
            hc.cpu_shares
                .map(|v| v.to_string())
                .filter(|v| v != "0")
                .unwrap_or_else(|| "默认".into()),
        ),
        (
            "CPU 周期".to_string(),
            hc.cpu_period
                .map(|v| format!("{v} μs"))
                .filter(|v| v != "0 μs")
                .unwrap_or_else(|| "默认".into()),
        ),
        (
            "CPU 配额".to_string(),
            match hc.cpu_quota {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => format!("{v} μs/周期"),
            },
        ),
        ("内存上限".to_string(), mb(hc.memory)),
        ("内存+交换".to_string(), mb(hc.memory_swap)),
        (
            "PIDs 上限".to_string(),
            match hc.pids_limit {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => v.to_string(),
            },
        ),
    ];
    let limits = ResourceLimitsForm {
        cpu_shares: hc.cpu_shares.map(|v| v.to_string()).unwrap_or_default(),
        cpu_period: hc.cpu_period.map(|v| v.to_string()).unwrap_or_default(),
        cpu_quota: hc.cpu_quota.map(|v| v.to_string()).unwrap_or_default(),
        memory_limit_mb: hc
            .memory
            .filter(|v| *v >= 0)
            .map(|v| (v / (1024 * 1024)).to_string())
            .unwrap_or_default(),
        memory_swap_mb: hc
            .memory_swap
            .map(|v| {
                if v < 0 {
                    v.to_string()
                } else {
                    (v / (1024 * 1024)).to_string()
                }
            })
            .unwrap_or_default(),
        pids_limit: hc.pids_limit.map(|v| v.to_string()).unwrap_or_default(),
    };

    let page = ContainerDetailPage {
        ctx: handlers::page_ctx(state, user, "containers"),
        container: handlers::container_view(row),
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
        error,
        limits,
        limits_effective: effective,
    };
    handlers::render(page)
}

/// 新建容器页面（admin / DockerLifecycle）。
pub async fn new_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ContainerCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;
    render_new(&state, &user, ContainerFormView::default(), String::new()).await
}

fn form_view(form: &HashMap<String, Vec<String>>) -> ContainerFormView {
    ContainerFormView {
        image: form_first(form, "image"),
        name: form_first(form, "name"),
        ports: form_first(form, "ports"),
        env: form_first(form, "env"),
        binds: form_first(form, "binds"),
        volumes: form_first(form, "volumes"),
        restart: form_first(form, "restart"),
        cpu_shares: form_first(form, "cpu_shares"),
        cpu_period: form_first(form, "cpu_period"),
        cpu_quota: form_first(form, "cpu_quota"),
        memory_limit_mb: form_first(form, "memory_limit_mb"),
        memory_swap_mb: form_first(form, "memory_swap_mb"),
        pids_limit: form_first(form, "pids_limit"),
        networks: form.get("networks").cloned().unwrap_or_default(),
    }
}

async fn render_new(
    state: &AppState,
    user: &crate::state::CurrentUser,
    form: ContainerFormView,
    error: String,
) -> Result<Response, AppError> {
    let mut network_options = if state.docker().available() {
        state
            .docker()
            .list_networks()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.name)
            .filter(|n| n != "host" && n != "none")
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    network_options.sort();
    let selected = form.networks.clone();
    let network_options = network_options
        .into_iter()
        .map(|n| {
            let checked = selected.contains(&n);
            (n, checked)
        })
        .collect();
    let page = ContainerNewPage {
        ctx: handlers::page_ctx(state, user, "containers"),
        form,
        network_options,
        docker_reason: state.docker_unavailable_reason().to_string(),
        error,
    };
    handlers::render(page)
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ContainerCreate, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    let spec = match spec_from_form(&form) {
        Ok(spec) => spec,
        Err(e) => {
            return render_new(&state, &user, form_view(&form), e.to_string()).await;
        }
    };
    if let Err(e) = state
        .cfg
        .policy
        .validate_spec(&spec, &handlers::actor(&user))
    {
        return render_new(&state, &user, form_view(&form), e.to_string()).await;
    }

    let cmd = CreateContainer {
        spec,
        actor: actor(&state, &user, &parts),
    };
    match docker_cmd::container_create(&state, cmd).await {
        Ok(id) => Ok(handlers::redirect(format!(
            "/{}/containers/{}",
            state.cfg.entry_prefix, id
        ))),
        // 协议异常（Docker 回包缺 id）仍然是传输层 400；引擎失败回填表单页。
        Err(DockerCommandError::BadRequest(m)) => Err(AppError::BadRequest(m)),
        Err(e) => render_new(&state, &user, form_view(&form), e.to_string()).await,
    }
}

/// 资源限制热更新（`containers.update`）。空输入按 0 处理（Engine 默认/清除）。
pub async fn update_limits(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;

    let row = handlers::resolve_container(&state, &id).await?;
    let obj = handlers::object_ref(&row);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), Verb::ContainerUpdate, Some(&obj))
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    let limits = match limits_from_form(&form) {
        Ok(v) => v,
        Err(e) => return render_detail(&state, &user, &row, e.to_string()).await,
    };
    if let Err(e) = state.cfg.policy.validate_limits(&limits) {
        return render_detail(&state, &user, &row, e.to_string()).await;
    }

    let cmd = UpdateContainer {
        target: ContainerTarget::new(row.name.clone(), row.short_id(), obj),
        limits,
        actor: actor(&state, &user, &parts),
    };
    match docker_cmd::container_update(&state, cmd).await {
        Ok(()) => Ok(handlers::redirect(format!(
            "/{}/containers/{}",
            state.cfg.entry_prefix, row.id
        ))),
        Err(e) => render_detail(&state, &user, &row, e.to_string()).await,
    }
}

fn parse_number(raw: &str, field: &str) -> Result<i64, AppError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| AppError::BadRequest(format!("{field} 需要整数：{raw}")))
}

fn optional_number(
    form: &HashMap<String, Vec<String>>,
    key: &str,
    field: &str,
) -> Result<Option<i64>, AppError> {
    let raw = form_first(form, key);
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_number(&raw, field).map(Some)
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

fn spec_from_form(form: &HashMap<String, Vec<String>>) -> Result<ContainerSpec, AppError> {
    let image = form_first(form, "image");
    if image.trim().is_empty() {
        return Err(AppError::BadRequest("镜像不能为空".into()));
    }
    let name = form_first(form, "name");
    let restart = form_first(form, "restart");
    let mut mounts = parse_mounts(&form_first(form, "binds"), false)?;
    mounts.extend(parse_mounts(&form_first(form, "volumes"), true)?);

    Ok(ContainerSpec {
        image: image.trim().to_string(),
        name: if name.trim().is_empty() {
            None
        } else {
            Some(name.trim().to_string())
        },
        command: None,
        entrypoint: None,
        env: parse_env(&form_first(form, "env"))?.into_iter().collect(),
        labels: BTreeMap::new(),
        ports: parse_ports(&form_first(form, "ports"))?,
        restart: if restart.trim().is_empty() {
            None
        } else {
            Some(restart.trim().to_string())
        },
        healthcheck: None,
        networks: form.get("networks").cloned().unwrap_or_default(),
        network_mode: None,
        mounts,
        log_driver: None,
        log_max_size: None,
        memory_limit_mb: optional_number(form, "memory_limit_mb", "内存上限")?,
        nano_cpus: None,
        pids_limit: optional_number(form, "pids_limit", "PIDs 上限")?,
        cpu_shares: optional_number(form, "cpu_shares", "CPU 份额")?,
        cpu_period: optional_number(form, "cpu_period", "CPU 周期")?,
        cpu_quota: optional_number(form, "cpu_quota", "CPU 配额")?,
        memory_swap_mb: optional_number(form, "memory_swap_mb", "内存交换")?,
    })
}

/// 更新表单：空值 = 0（清除/Engine 默认），避免「空字段 = 不修改」的二义性。
fn limits_from_form(form: &HashMap<String, Vec<String>>) -> Result<ResourceLimits, AppError> {
    let field = |key: &str, label: &str| -> Result<Option<i64>, AppError> {
        let raw = form_first(form, key);
        if raw.trim().is_empty() {
            return Ok(Some(0));
        }
        parse_number(&raw, label).map(Some)
    };
    Ok(ResourceLimits {
        cpu_shares: field("cpu_shares", "CPU 份额")?,
        cpu_period: field("cpu_period", "CPU 周期")?,
        cpu_quota: field("cpu_quota", "CPU 配额")?,
        memory_limit_mb: field("memory_limit_mb", "内存上限")?,
        memory_swap_mb: field("memory_swap_mb", "内存交换")?,
        pids_limit: field("pids_limit", "PIDs 上限")?,
    })
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
fn actor(state: &AppState, user: &crate::state::CurrentUser, parts: &Parts) -> Actor {
    Actor {
        username: user.user.username.clone(),
        ip: handlers::client_ip(state, parts),
    }
}

fn map_err(e: DockerCommandError) -> AppError {
    match e {
        DockerCommandError::BadRequest(m) => AppError::BadRequest(m),
        DockerCommandError::Conflict(m) => AppError::Conflict(m),
        DockerCommandError::Unavailable(m) => AppError::Unavailable(m),
    }
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

    // BusyGuard 与引擎审计都在命令里；传输层只负责映射错误与跳转。
    let cmd = ContainerAction {
        target: ContainerTarget::new(row.name.clone(), row.short_id(), obj),
        verb,
        actor: actor(&state, &user, &parts),
    };
    docker_cmd::container_action(&state, cmd)
        .await
        .map_err(map_err)?;

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

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
            .collect()
    }

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

    #[test]
    fn blank_limits_mean_clear_not_untouched() {
        let limits = limits_from_form(&form(&[("cpu_shares", ""), ("pids_limit", "64")])).unwrap();
        assert_eq!(limits.cpu_shares, Some(0), "空字段按 0 清除，避免二义性");
        assert_eq!(limits.pids_limit, Some(64));
        assert_eq!(limits.memory_limit_mb, Some(0));
        assert!(limits_from_form(&form(&[("cpu_shares", "abc")])).is_err());
    }

    #[test]
    fn spec_from_form_requires_image_and_keeps_fields() {
        assert!(spec_from_form(&form(&[])).is_err(), "镜像为空必须报错");
        let spec = spec_from_form(&form(&[
            ("image", "nginx:1.27"),
            ("name", " web "),
            ("ports", "8080:80"),
            ("env", "A=1"),
            ("restart", "unless-stopped"),
            ("cpu_shares", "512"),
            ("memory_limit_mb", "256"),
        ]))
        .unwrap();
        assert_eq!(spec.image, "nginx:1.27");
        assert_eq!(spec.name.as_deref(), Some("web"));
        assert_eq!(spec.restart.as_deref(), Some("unless-stopped"));
        assert_eq!(spec.cpu_shares, Some(512));
        assert_eq!(spec.memory_limit_mb, Some(256));
        assert_eq!(spec.ports.len(), 1);
    }
}
