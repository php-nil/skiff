//! Compose 的跨模块共享逻辑：项目归组、权限判定与规格校验。
//!
//! 项目归属完全由容器标签重建（`io.skiff.project` 优先，回落
//! `com.docker.compose.project`），不持久化 compose 源文件；`down` 只清理
//! 带 `io.skiff.project` 的内核项目，外部 compose 项目只读展示。
//!
//! 页面已迁到 budding 壳（`/ui/v1/compose/*`），本模块只留 JSON 通道复用的纯函数。

use std::collections::BTreeMap;

use skiff_app::commands::docker as docker_cmd;
use skiff_docker::compose::{parse, ComposeProject};
use skiff_docker::{project_of, service_of, Verb};

use crate::error::AppError;
use crate::handlers;
use crate::state::{AppState, CurrentUser};
use crate::templates::{ComposeProjectView, ComposeServiceView};

/// 读取容器列表并按项目归组；选中项目时附带每个服务最近 100 行日志。
pub(crate) async fn status_of(
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

pub(crate) fn require_compose_admin(state: &AppState, user: &CurrentUser) -> Result<(), AppError> {
    state
        .cfg
        .policy
        .check(&handlers::actor(user), Verb::ComposeUp, None)
        .map_err(|e| AppError::Forbidden(e.to_string()))
}

pub(crate) fn validate_specs(
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

pub(crate) fn planned_project(text: &str, raw_project: &str) -> Result<ComposeProject, AppError> {
    if text.trim().is_empty() {
        return Err(AppError::BadRequest("请粘贴 compose 内容或选择文件".into()));
    }
    let project = docker_cmd::prepare_project(raw_project).map_err(map_err)?;
    parse(text, &project).map_err(|e| AppError::BadRequest(e.to_string()))
}
/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::compose::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;

/// Docker 命令错误映射：唯一实现在 [`crate::error::map_docker_cmd`]。
pub(crate) use crate::error::map_docker_cmd as map_err;
