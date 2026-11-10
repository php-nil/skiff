//! Compose 页面：粘贴/选择文件 → 校验 → 部署 → 起停/删除 → 服务状态与日志。
//!
//! 项目归属完全由容器标签重建（`io.skiff.project` 优先，回落
//! `com.docker.compose.project`），不持久化 compose 源文件；`down` 只清理
//! 带 `io.skiff.project` 的内核项目，外部 compose 项目只读展示。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use serde_json::Value;
use skiff_app::commands::docker::{
    self as docker_cmd, ComposeDown, ComposeUp, ContainerTarget, DockerCommandError, ProjectAction,
};
use skiff_app::commands::Actor;
use skiff_docker::compose::{normalize_project_name, parse, ComposeProject};
use skiff_docker::{project_of, service_of, ObjectRef, Verb};
use skiff_model::Permission;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::{AppState, CurrentUser};
use crate::templates::{ComposePage, ComposeProjectView, ComposeReportRow, ComposeServiceView};
use crate::util::{form_first, parse_form};

const RESERVED_PROJECTS: [&str; 2] = ["sites", "skiff"];

#[derive(Default)]
struct PageInput {
    compose_text: String,
    project_input: String,
    selected_project: String,
    error: String,
    notice: String,
    validation: Vec<String>,
    report_title: String,
    report: Vec<ComposeReportRow>,
}

impl PageInput {
    fn from_form(form: &HashMap<String, Vec<String>>) -> Self {
        Self {
            compose_text: form_first(form, "compose"),
            project_input: form_first(form, "project"),
            ..Default::default()
        }
    }

    fn failed(self, error: String) -> Self {
        Self { error, ..self }
    }
}

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    render(
        &state,
        &user,
        PageInput {
            selected_project: q.get("project").cloned().unwrap_or_default(),
            notice: String::new(),
            error: String::new(),
            ..Default::default()
        },
    )
    .await
}

/// 仅解析校验：返回服务拓扑顺序、网络与卷清单，不触碰 Engine。
pub async fn validate(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    require_compose_admin(&state, &user)?;
    let mut input = PageInput::from_form(&form);

    let project = match planned_project(&input.compose_text, &input.project_input) {
        Ok(p) => p,
        Err(e) => return render(&state, &user, input.failed(e.to_string())).await,
    };
    if let Err(e) = validate_specs(&state, &user, &project) {
        return render(&state, &user, input.failed(e.to_string())).await;
    }

    let names: Vec<&str> = project
        .services
        .iter()
        .map(|s| s.service.as_str())
        .collect();
    input.selected_project = project.name.clone();
    input.validation = vec![
        format!("项目：{}", project.name),
        format!("服务顺序：{}", names.join(" → ")),
        format!(
            "网络：{}",
            if project.networks.is_empty() {
                "（无）".to_string()
            } else {
                project.networks.join(", ")
            }
        ),
        format!(
            "卷：{}",
            if project.volumes.is_empty() {
                "（无）".to_string()
            } else {
                project.volumes.join(", ")
            }
        ),
        "校验通过；未执行任何引擎操作。".to_string(),
    ];
    input.notice = "校验通过，可继续部署。".to_string();
    render(&state, &user, input).await
}

pub async fn up(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    require_compose_admin(&state, &user)?;
    let mut input = PageInput::from_form(&form);

    let project = match planned_project(&input.compose_text, &input.project_input) {
        Ok(p) => p,
        Err(e) => return render(&state, &user, input.failed(e.to_string())).await,
    };
    if let Err(e) = validate_specs(&state, &user, &project) {
        return render(&state, &user, input.failed(e.to_string())).await;
    }

    let value = match docker_cmd::compose_up(
        &state,
        ComposeUp {
            project: project.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return render(&state, &user, input.failed(e.to_string())).await,
    };
    input.selected_project = project.name.clone();
    input.report = report_rows(&value, "services", "service", "container");
    input.report_title = "部署结果".to_string();
    let failed = value.get("failed").and_then(Value::as_u64).unwrap_or(0);
    input.error = if failed == 0 {
        String::new()
    } else {
        format!("项目 {} 有 {} 个服务失败，详见下表", project.name, failed)
    };
    if failed == 0 {
        input.notice = format!(
            "项目 {} 已部署（{} 个服务）",
            project.name,
            project.services.len()
        );
    }
    let resp = render(&state, &user, input).await?;
    Ok(with_partial_status(resp, failed))
}

/// 部分失败时页面照常渲染（带逐服务报告），但 HTTP 状态不能仍是 2xx——
/// 否则调用方与监控会把「部分失败」当整体成功。
fn with_partial_status(mut resp: Response, failed: u64) -> Response {
    if failed > 0 {
        *resp.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
    }
    resp
}

pub async fn down(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    require_compose_admin(&state, &user)?;
    let raw_project = form_first(&form, "project");
    let remove_volumes = matches!(
        form_first(&form, "remove_volumes").as_str(),
        "on" | "true" | "1"
    );

    let Some(project) = checked_project(&raw_project)? else {
        return Err(AppError::BadRequest("请填写项目名".into()));
    };
    let value = match docker_cmd::compose_down(
        &state,
        ComposeDown {
            project: project.clone(),
            remove_volumes,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return render(
                &state,
                &user,
                PageInput {
                    error: e.to_string(),
                    selected_project: project,
                    ..Default::default()
                },
            )
            .await
        }
    };
    let nets = value
        .get("removed_networks")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let vols = value
        .get("removed_volumes")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let failed = value.get("failed").and_then(Value::as_u64).unwrap_or(0);
    let input = if failed == 0 {
        PageInput {
            selected_project: project.clone(),
            notice: format!(
                "项目 {project} 已删除：网络 {nets} 个、卷 {vols} 个{}",
                if remove_volumes {
                    "（含数据）"
                } else {
                    "（卷保留）"
                }
            ),
            report_title: "清理结果".to_string(),
            report: report_rows(&value, "containers", "", "container"),
            ..Default::default()
        }
    } else {
        PageInput {
            selected_project: project.clone(),
            error: format!("项目 {project} 清理时有 {failed} 个容器失败，详见下表"),
            report_title: "清理结果".to_string(),
            report: report_rows(&value, "containers", "", "container"),
            ..Default::default()
        }
    };
    let resp = render(&state, &user, input).await?;
    Ok(with_partial_status(resp, failed))
}

/// 项目级起停/重启：逐容器复用既有动词分发，权限与审计按单容器判定。
pub async fn project_action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((project, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    require_compose_admin(&state, &user)?;
    let verb = match action.as_str() {
        "start" => Verb::Start,
        "stop" => Verb::Stop,
        "restart" => Verb::Restart,
        _ => return Err(AppError::BadRequest("未知的项目动作".into())),
    };
    let Some(project) = checked_project(&project)? else {
        return Err(AppError::BadRequest("项目名无效".into()));
    };

    let rows = state
        .docker()
        .list_containers()
        .await
        .map_err(|e| AppError::Unavailable(e.to_string()))?;
    let mut targets = Vec::new();
    let mut failed = 0;
    for row in rows
        .iter()
        .filter(|r| project_of(&r.labels) == Some(project.as_str()))
    {
        let obj: ObjectRef = handlers::object_ref(row);
        if state
            .cfg
            .policy
            .check(&handlers::actor(&user), verb, Some(&obj))
            .is_err()
        {
            failed += 1;
            continue;
        }
        targets.push(ContainerTarget::new(row.name.clone(), row.short_id(), obj));
    }

    let report = docker_cmd::compose_project_action(
        &state,
        ProjectAction {
            targets,
            verb,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    let done = report.done;
    failed += report.failed;

    let notice = format!("项目 {project}：成功 {done} 个，失败 {failed} 个");
    let kind = if failed == 0 {
        skiff_app::FlashKind::Notice
    } else {
        skiff_app::FlashKind::Error
    };
    Ok(handlers::flash_redirect(
        &state,
        &user,
        kind,
        notice,
        format!("/{}/compose?project={}", state.cfg.entry_prefix, project),
    ))
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    mut input: PageInput,
) -> Result<Response, AppError> {
    let (projects, services, docker_error) = status_of(state, &input.selected_project).await;
    if input.error.is_empty() {
        input.error = docker_error;
    }
    let page = ComposePage {
        ctx: handlers::page_ctx(state, user, "compose"),
        can_admin: user.can(Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
        error: input.error,
        notice: input.notice,
        compose_text: input.compose_text,
        project_input: input.project_input,
        validation: input.validation,
        report_title: input.report_title,
        report: input.report,
        projects,
        selected_project: input.selected_project,
        services,
    };
    handlers::render(page)
}

/// 读取容器列表并按项目归组；选中项目时附带每个服务最近 100 行日志。
async fn status_of(
    state: &AppState,
    selected: &str,
) -> (Vec<ComposeProjectView>, Vec<ComposeServiceView>, String) {
    if !state.docker().available() {
        return (Vec::new(), Vec::new(), String::new());
    }
    let rows = match state.docker().list_containers().await {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Vec::new(), e.to_string()),
    };

    let mut groups: BTreeMap<String, (bool, usize, usize)> = BTreeMap::new();
    for row in &rows {
        let Some(project) = project_of(&row.labels) else {
            continue;
        };
        let managed = row.labels.contains_key("io.skiff.project");
        let entry = groups.entry(project.to_string()).or_insert((managed, 0, 0));
        entry.0 |= managed;
        entry.1 += 1;
        if row.state == "running" {
            entry.2 += 1;
        }
    }
    let projects = groups
        .into_iter()
        .map(|(name, (managed, services, running))| ComposeProjectView {
            name,
            managed,
            services,
            running,
        })
        .collect();

    let mut services = Vec::new();
    if !selected.is_empty() {
        for row in &rows {
            if project_of(&row.labels) != Some(selected) {
                continue;
            }
            let logs = match state.docker().logs(&row.id, 100).await {
                Ok(text) => {
                    // 取末尾 100 行；`Lines` 是单端迭代器，先收集再切片。
                    let all: Vec<&str> = text.lines().collect();
                    let start = all.len().saturating_sub(100);
                    all[start..].join("\n")
                }
                Err(e) => format!("日志读取失败：{e}"),
            };
            services.push(ComposeServiceView {
                service: service_of(&row.labels)
                    .map(str::to_string)
                    .unwrap_or_else(|| row.name.clone()),
                container: row.name.clone(),
                id: row.id.clone(),
                state: row.state.clone(),
                status: row.status.clone(),
                image: row.image.clone(),
                managed: row.labels.contains_key("io.skiff.project"),
                logs,
            });
        }
    }
    (projects, services, String::new())
}

fn require_compose_admin(state: &AppState, user: &CurrentUser) -> Result<(), AppError> {
    state
        .cfg
        .policy
        .check(&handlers::actor(user), Verb::ComposeUp, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))
}

fn validate_specs(
    state: &AppState,
    user: &CurrentUser,
    project: &ComposeProject,
) -> Result<(), AppError> {
    for svc in &project.services {
        state
            .cfg
            .policy
            .validate_spec(&svc.spec, &handlers::actor(user))
            .map_err(|e| AppError::BadRequest(format!("服务 {}：{e}", svc.service)))?;
    }
    Ok(())
}

fn planned_project(text: &str, raw_project: &str) -> Result<ComposeProject, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("请粘贴 compose 内容或选择文件".into()));
    }
    let Some(project) = checked_project(raw_project)? else {
        return Err(AppError::BadRequest("请填写项目名".into()));
    };
    parse(text, &project).map_err(|e| AppError::BadRequest(e.to_string()))
}

/// 归一化并拒绝内核保留项目名（`sites` / `skiff` 由内核或站点模块持有）。
fn checked_project(raw: &str) -> Result<Option<String>, AppError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let name = normalize_project_name(raw);
    if RESERVED_PROJECTS.contains(&name.as_str()) {
        return Err(AppError::BadRequest(format!(
            "项目名 {name} 为内核保留，请换一个"
        )));
    }
    Ok(Some(name))
}

fn report_rows(
    value: &Value,
    key: &str,
    service_key: &str,
    name_key: &str,
) -> Vec<ComposeReportRow> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| ComposeReportRow {
                    kind: if service_key.is_empty() {
                        "容器".into()
                    } else {
                        row.get(service_key)
                            .and_then(Value::as_str)
                            .unwrap_or("服务")
                            .to_string()
                    },
                    name: row
                        .get(name_key)
                        .or_else(|| row.get("network"))
                        .or_else(|| row.get("volume"))
                        .and_then(Value::as_str)
                        .unwrap_or("-")
                        .to_string(),
                    status: row
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("-")
                        .to_string(),
                    error: row
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
fn actor(state: &AppState, user: &CurrentUser, parts: &Parts) -> Actor {
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

#[cfg(test)]
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_project_names_are_rejected() {
        assert!(checked_project("sites").is_err());
        assert!(checked_project("Skiff").is_err());
        assert_eq!(checked_project("").unwrap(), None);
        assert_eq!(
            checked_project("My App").unwrap().as_deref(),
            Some("my-app")
        );
    }

    #[test]
    fn report_rows_reads_service_results() {
        let v = serde_json::json!({
            "services": [
                {"service":"web","container":"demo-web","status":"started"}
            ]
        });
        let rows = report_rows(&v, "services", "service", "container");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "web");
        assert_eq!(rows[0].name, "demo-web");
        assert_eq!(rows[0].status, "started");
    }

    #[test]
    fn urlencode_keeps_safe_chars() {
        assert_eq!(
            urlencode("项目 已删除"),
            "%E9%A1%B9%E7%9B%AE+%E5%B7%B2%E5%88%A0%E9%99%A4"
        );
    }
}
