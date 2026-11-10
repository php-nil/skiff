//! Docker 资源命令（容器/镜像/网络/卷/Compose，对应 `actions::Resource` 的 Docker 组）。
//!
//! **权限不在这里**：对象级授权（`policy.check`）与参数策略校验仍在写入口，
//! 命令只接收已授权的目标并执行。动词到 Docker 的分发仍然只经过
//! `Docker::execute`（红线：不另开分发路径）。
//!
//! 审计随命令产生，动作名与 HTTP 路径取自 `Verb`，与旧 handler 逐字节一致。

use std::sync::Arc;

use serde_json::Value;
use skiff_docker::{DockerError, DockerParams, ObjectRef, ResourceLimits, Verb};
use skiff_model::ContainerSpec;

use super::Actor;
use crate::AppState;

/// 命令名（与 `actions` 表里的 `command` 后缀一一对应），供漂移测试使用。
pub const NAMES: &[&str] = &[
    "container_create",
    "container_update",
    "container_action",
    "image_pull",
    "image_remove",
    "network_create",
    "network_remove",
    "volume_create",
    "volume_remove",
    "compose_up",
    "compose_down",
    "compose_project_action",
];

/// 命令错误：与 `SiteError` 同形，由传输层映射到 HTTP 状态与文案。
#[derive(Debug, Clone)]
pub enum DockerCommandError {
    BadRequest(String),
    Conflict(String),
    Unavailable(String),
}

impl std::fmt::Display for DockerCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerCommandError::BadRequest(m)
            | DockerCommandError::Conflict(m)
            | DockerCommandError::Unavailable(m) => f.write_str(m),
        }
    }
}

/// 已解析的容器目标：名称/短 ID 用于审计与提示，`object` 用于 Docker 调用。
#[derive(Debug, Clone)]
pub struct ContainerTarget {
    pub name: String,
    pub short_id: String,
    pub object: ObjectRef,
}

impl ContainerTarget {
    pub fn new(name: String, short_id: String, object: ObjectRef) -> Self {
        Self {
            name,
            short_id,
            object,
        }
    }
}

fn audit_status(
    state: &AppState,
    actor: &Actor,
    verb: Verb,
    target: &str,
    status: &str,
    detail: &str,
) {
    state.audit_or_log(
        &actor.username,
        &actor.ip,
        &verb.audit_action(),
        target,
        status,
        detail,
    );
}

fn audit(
    state: &AppState,
    actor: &Actor,
    verb: Verb,
    target: &str,
    result: Result<(), &DockerError>,
    ok_detail: &str,
) {
    match result {
        Ok(()) => audit_status(state, actor, verb, target, "ok", ok_detail),
        Err(e) => audit_status(state, actor, verb, target, "fail", &e.to_string()),
    }
}

/// 报告里的失败计数（`compose.up` / `compose.down` 的 `failed` 字段）。
fn report_failed(value: &Value) -> u64 {
    value.get("failed").and_then(Value::as_u64).unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct CreateContainer {
    pub spec: ContainerSpec,
    pub actor: Actor,
}

/// 返回新容器 id；Docker 未回 id 属于协议异常，用 `BadRequest` 表达。
pub async fn container_create(
    state: &Arc<AppState>,
    cmd: CreateContainer,
) -> Result<String, DockerCommandError> {
    let target = cmd
        .spec
        .name
        .clone()
        .unwrap_or_else(|| cmd.spec.image.clone());
    let result = state
        .docker()
        .execute(
            Verb::ContainerCreate,
            None,
            DockerParams::ContainerCreate(Box::new(cmd.spec)),
        )
        .await;
    audit(
        state,
        &cmd.actor,
        Verb::ContainerCreate,
        &target,
        result.as_ref().map(|_| ()),
        Verb::ContainerCreate.http_path(),
    );
    let value = result.map_err(|e| DockerCommandError::Unavailable(e.to_string()))?;
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| DockerCommandError::BadRequest("Docker 未返回容器 id".into()))
}

#[derive(Debug, Clone)]
pub struct UpdateContainer {
    pub target: ContainerTarget,
    pub limits: ResourceLimits,
    pub actor: Actor,
}

pub async fn container_update(
    state: &Arc<AppState>,
    cmd: UpdateContainer,
) -> Result<(), DockerCommandError> {
    let result = state
        .docker()
        .execute(
            Verb::ContainerUpdate,
            Some(&cmd.target.object),
            DockerParams::ContainerUpdate(Box::new(cmd.limits)),
        )
        .await;
    audit(
        state,
        &cmd.actor,
        Verb::ContainerUpdate,
        &cmd.target.name,
        result.as_ref().map(|_| ()),
        Verb::ContainerUpdate.http_path(),
    );
    result
        .map(|_| ())
        .map_err(|e| DockerCommandError::Unavailable(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct ContainerAction {
    pub target: ContainerTarget,
    pub verb: Verb,
    pub actor: Actor,
}

pub async fn container_action(
    state: &Arc<AppState>,
    cmd: ContainerAction,
) -> Result<(), DockerCommandError> {
    // BusyGuard：容器有进行中的安装/扩展/重建任务时，删除会打断内核自己的任务。
    if cmd.verb == Verb::ContainerRemove {
        if let Some(reason) = state.container_busy_reason(&cmd.target.name).await {
            state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                &cmd.verb.audit_action(),
                &cmd.target.name,
                "deny",
                &reason,
            );
            return Err(DockerCommandError::Conflict(reason));
        }
    }
    let params = match cmd.verb {
        // 删除即销毁；running 容器一并强制移除，不再拆成 stop→remove 两步。
        Verb::ContainerRemove => DockerParams::ContainerRemove { force: true },
        _ => DockerParams::None,
    };
    let result = state
        .docker()
        .execute(cmd.verb, Some(&cmd.target.object), params)
        .await
        .map(|_| ());
    match &result {
        Ok(()) => {
            state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                &cmd.verb.audit_action(),
                &format!("{} ({})", cmd.target.name, cmd.target.short_id),
                "ok",
                cmd.verb.http_path(),
            );
        }
        Err(e) => {
            state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                &cmd.verb.audit_action(),
                &cmd.target.name,
                "fail",
                &e.to_string(),
            );
        }
    }
    result.map_err(|e| DockerCommandError::Unavailable(e.to_string()))
}

/// 镜像/网络/卷这类「按名字直接操作」的资源。
#[derive(Debug, Clone)]
pub struct NamedResource {
    pub name: String,
    pub actor: Actor,
}

pub async fn image_pull(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().pull_image(&cmd.name).await;
    audit(
        state,
        &cmd.actor,
        Verb::ImagePull,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result.map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

pub async fn image_remove(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().remove_image(&cmd.name, false).await;
    audit(
        state,
        &cmd.actor,
        Verb::ImageRemove,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result.map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

pub async fn network_create(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().create_network(&cmd.name).await;
    audit(
        state,
        &cmd.actor,
        Verb::NetworkCreate,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result
        .map(|_| ())
        .map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

pub async fn network_remove(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().remove_network(&cmd.name).await;
    audit(
        state,
        &cmd.actor,
        Verb::NetworkRemove,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result.map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

pub async fn volume_create(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().create_volume(&cmd.name).await;
    audit(
        state,
        &cmd.actor,
        Verb::VolumeCreate,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result.map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

pub async fn volume_remove(
    state: &Arc<AppState>,
    cmd: NamedResource,
) -> Result<(), DockerCommandError> {
    let result = state.docker().remove_volume(&cmd.name).await;
    audit(
        state,
        &cmd.actor,
        Verb::VolumeRemove,
        &cmd.name,
        result.as_ref().map(|_| ()),
        "",
    );
    result.map_err(|e| DockerCommandError::BadRequest(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct ComposeUp {
    pub project: skiff_docker::compose::ComposeProject,
    pub actor: Actor,
}

pub async fn compose_up(
    state: &Arc<AppState>,
    cmd: ComposeUp,
) -> Result<Value, DockerCommandError> {
    let result = state
        .docker()
        .execute(
            Verb::ComposeUp,
            None,
            DockerParams::ComposeUp(Box::new(cmd.project.clone())),
        )
        .await;
    // 部分成功不能记成 ok：`failed > 0` 时审计落 `partial`，保留完整追溯。
    match &result {
        Ok(v) if report_failed(v) > 0 => audit_status(
            state,
            &cmd.actor,
            Verb::ComposeUp,
            &cmd.project.name,
            "partial",
            &format!("{} 个服务未成功启动", report_failed(v)),
        ),
        _ => audit(
            state,
            &cmd.actor,
            Verb::ComposeUp,
            &cmd.project.name,
            result.as_ref().map(|_| ()),
            Verb::ComposeUp.http_path(),
        ),
    }
    result.map_err(|e| DockerCommandError::Unavailable(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct ComposeDown {
    pub project: String,
    pub remove_volumes: bool,
    pub actor: Actor,
}

pub async fn compose_down(
    state: &Arc<AppState>,
    cmd: ComposeDown,
) -> Result<Value, DockerCommandError> {
    let result = state
        .docker()
        .execute(
            Verb::ComposeDown,
            None,
            DockerParams::ComposeDown {
                project: cmd.project.clone(),
                remove_volumes: cmd.remove_volumes,
            },
        )
        .await;
    // 清理里出现 stop/remove/inspect 失败同样记 `partial`。
    match &result {
        Ok(v) if report_failed(v) > 0 => audit_status(
            state,
            &cmd.actor,
            Verb::ComposeDown,
            &cmd.project,
            "partial",
            &format!("{} 个容器清理失败", report_failed(v)),
        ),
        _ => audit(
            state,
            &cmd.actor,
            Verb::ComposeDown,
            &cmd.project,
            result.as_ref().map(|_| ()),
            Verb::ComposeDown.http_path(),
        ),
    }
    result.map_err(|e| DockerCommandError::Unavailable(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct ProjectAction {
    /// 已通过对象级授权的容器；命令不再重复判定权限。
    pub targets: Vec<ContainerTarget>,
    pub verb: Verb,
    pub actor: Actor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectActionReport {
    pub done: usize,
    pub failed: usize,
}

pub async fn compose_project_action(
    state: &Arc<AppState>,
    cmd: ProjectAction,
) -> Result<ProjectActionReport, DockerCommandError> {
    let mut report = ProjectActionReport::default();
    for target in &cmd.targets {
        let result = state
            .docker()
            .execute(cmd.verb, Some(&target.object), DockerParams::None)
            .await;
        audit(
            state,
            &cmd.actor,
            cmd.verb,
            &target.name,
            result.as_ref().map(|_| ()),
            cmd.verb.http_path(),
        );
        if result.is_ok() {
            report.done += 1;
        } else {
            report.failed += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use skiff_docker::ObjectRef;

    use super::*;
    use crate::commands::test_support::state_at;
    use crate::jobs::{JobSubmit, TARGET_RUNTIME};

    fn actor() -> Actor {
        Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        }
    }

    fn spec(name: &str, image: &str) -> ContainerSpec {
        serde_json::from_value(serde_json::json!({ "image": image, "name": name })).unwrap()
    }

    fn target(name: &str) -> ContainerTarget {
        let id = "0123456789abcdef".to_string();
        ContainerTarget::new(
            name.into(),
            "0123456789ab".into(),
            ObjectRef {
                id,
                name: name.into(),
                labels: BTreeMap::new(),
            },
        )
    }

    #[tokio::test]
    async fn create_maps_unreachable_engine_and_audits_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = container_create(
            &state,
            CreateContainer {
                spec: spec("demo", "alpine"),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DockerCommandError::Unavailable(ref m) if !m.is_empty()),
            "{err}"
        );

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "docker.containers.create");
        assert_eq!(rows[0].target, "demo");
        assert_eq!(rows[0].result, "fail");
    }

    #[tokio::test]
    async fn action_maps_unreachable_engine_and_audits_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = container_action(
            &state,
            ContainerAction {
                target: target("demo"),
                verb: Verb::Start,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DockerCommandError::Unavailable(ref m) if !m.is_empty()),
            "{err}"
        );

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "docker.containers.start");
        assert_eq!(rows[0].target, "demo", "失败审计用容器名，不带短 id");
        assert_eq!(rows[0].result, "fail");
    }

    #[tokio::test]
    async fn remove_is_denied_when_kernel_job_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        state
            .db()
            .create_runtime(&skiff_store::NewRuntime {
                id: "php-busy".into(),
                kind: "php".into(),
                name: "busy".into(),
                version: "8.3".into(),
                image: "php:8.3-fpm-alpine".into(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
        state
            .jobs()
            .submit(JobSubmit {
                kind: "runtime.install".into(),
                target_type: TARGET_RUNTIME.into(),
                target_id: "php-busy".into(),
                title: "安装运行环境 busy".into(),
                params: serde_json::json!({}),
                created_by: "admin".into(),
                created_by_role: "admin".into(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .unwrap();

        let name = crate::sites::runtime_container_name("php", "busy");
        let err = container_action(
            &state,
            ContainerAction {
                target: target(&name),
                verb: Verb::ContainerRemove,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DockerCommandError::Conflict(ref m) if m.contains("进行中的任务")),
            "{err}"
        );

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "docker.containers.remove");
        assert_eq!(rows[0].result, "deny");
        assert!(!rows[0].detail.is_empty());
    }

    #[tokio::test]
    async fn named_resources_map_engine_errors_to_bad_request() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let cmd = || NamedResource {
            name: "res".into(),
            actor: actor(),
        };
        for result in [
            image_pull(&state, cmd()).await.err(),
            image_remove(&state, cmd()).await.err(),
            network_create(&state, cmd()).await.err(),
            network_remove(&state, cmd()).await.err(),
            volume_create(&state, cmd()).await.err(),
            volume_remove(&state, cmd()).await.err(),
        ] {
            assert!(
                matches!(result, Some(DockerCommandError::BadRequest(_))),
                "Docker 不可达应映射为 BadRequest：{result:?}"
            );
        }
        let rows = state.db().recent_audit(6).unwrap();
        assert_eq!(rows.len(), 6, "六个命令都应留失败审计：{rows:?}");
        assert!(rows.iter().all(|r| r.result == "fail"));
    }

    #[tokio::test]
    async fn compose_maps_unreachable_engine_and_audits() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let project =
            skiff_docker::compose::parse("services:\n  web:\n    image: nginx:alpine\n", "demo")
                .unwrap();

        let up = compose_up(
            &state,
            ComposeUp {
                project,
                actor: actor(),
            },
        )
        .await;
        assert!(
            matches!(up, Err(DockerCommandError::Unavailable(_))),
            "{up:?}"
        );
        let down = compose_down(
            &state,
            ComposeDown {
                project: "demo".into(),
                remove_volumes: false,
                actor: actor(),
            },
        )
        .await;
        assert!(
            matches!(down, Err(DockerCommandError::Unavailable(_))),
            "{down:?}"
        );

        let rows = state.db().recent_audit(2).unwrap();
        assert_eq!(rows.len(), 2);
        let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
        assert!(actions.contains(&"docker.compose.up"), "{actions:?}");
        assert!(actions.contains(&"docker.compose.down"), "{actions:?}");
    }

    #[tokio::test]
    async fn project_action_without_targets_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let report = compose_project_action(
            &state,
            ProjectAction {
                targets: Vec::new(),
                verb: Verb::Restart,
                actor: actor(),
            },
        )
        .await
        .unwrap();
        assert_eq!(report, ProjectActionReport { done: 0, failed: 0 });
        assert!(state.db().recent_audit(10).unwrap().is_empty());
    }
}
