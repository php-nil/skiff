//! Compose 项目的执行层：`compose.up` / `compose.down`。
//!
//! 解析在 `skiff-compose`；本模块只做「按计划创建/清理」，并按 `Docker::execute`
//! 统一分发的约定暴露方法。保留标签（`io.skiff.project` / `io.skiff.owner`）与
//! Docker compose 识别标签都在这里、在校验通过之后注入，解析器不接触它们。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

use crate::client::{DockerError, DockerService};
use crate::compose::ComposeProject;
use crate::policy::{LABEL_OWNER, LABEL_PROJECT, LABEL_PROTECTED};

/// Docker Compose 官方标签：外部 `docker compose` 项目也带这两个，列表据此归组。
pub const LABEL_COMPOSE_PROJECT: &str = "com.docker.compose.project";
pub const LABEL_COMPOSE_SERVICE: &str = "com.docker.compose.service";

fn project_prefix(project: &str) -> String {
    format!("{project}_")
}

impl DockerService {
    /// 按已解析的项目计划创建网络/卷/镜像/容器并启动。
    ///
    /// 失败按服务逐条记录，不把「部分成功」当整体成功；依赖失败的后续服务标记 skipped。
    pub(crate) async fn compose_up(&self, project: &ComposeProject) -> Result<Value, DockerError> {
        self.ensure_networks(&project.networks).await?;
        self.ensure_volumes(&project.volumes).await?;
        let failed_images = self.ensure_images(project).await;

        let mut failed_services: BTreeSet<String> = BTreeSet::new();
        let mut report = Vec::new();
        for svc in &project.services {
            if let Some(dep) = svc
                .depends_on
                .iter()
                .find(|d| failed_services.contains(d.as_str()))
            {
                failed_services.insert(svc.service.clone());
                report.push(json!({
                    "service": svc.service,
                    "container": svc.container_name,
                    "status": "skipped",
                    "error": format!("依赖服务 {dep} 失败，跳过"),
                }));
                continue;
            }
            if failed_images.contains(&svc.spec.image) {
                failed_services.insert(svc.service.clone());
                report.push(json!({
                    "service": svc.service,
                    "container": svc.container_name,
                    "status": "failed",
                    "error": "镜像拉取失败",
                }));
                continue;
            }

            let mut spec = svc.spec.clone();
            spec.name = Some(svc.container_name.clone());
            spec.labels
                .insert(LABEL_PROJECT.into(), project.name.clone());
            spec.labels.insert(LABEL_OWNER.into(), "skiff".into());
            spec.labels
                .insert(LABEL_COMPOSE_PROJECT.into(), project.name.clone());
            spec.labels
                .insert(LABEL_COMPOSE_SERVICE.into(), svc.service.clone());

            match self.create_container(&spec).await {
                Ok(id) => match self.start_container(&id).await {
                    Ok(()) => report.push(json!({
                        "service": svc.service,
                        "container": svc.container_name,
                        "container_id": id,
                        "status": "started",
                    })),
                    Err(e) => {
                        failed_services.insert(svc.service.clone());
                        report.push(json!({
                            "service": svc.service,
                            "container": svc.container_name,
                            "container_id": id,
                            "status": "created-not-started",
                            "error": e.to_string(),
                        }));
                    }
                },
                Err(e) => {
                    failed_services.insert(svc.service.clone());
                    report.push(json!({
                        "service": svc.service,
                        "container": svc.container_name,
                        "status": "failed",
                        "error": e.to_string(),
                    }));
                }
            }
        }

        Ok(json!({
            "project": project.name,
            "failed": count_unstarted(&report),
            // `partial` 让上层（审计 / 响应码）不必再自己数报告行。
            "status": if count_unstarted(&report) == 0 { "ok" } else { "partial" },
            "services": report,
        }))
    }

    /// 停止并删除项目容器；`remove_volumes` 显式勾选才删除项目前缀卷。
    ///
    /// 只处理带 `io.skiff.project == project` 的容器（外部 compose 项目不会被误删），
    /// 并跳过受保护容器。
    pub(crate) async fn compose_down(
        &self,
        project: &str,
        remove_volumes: bool,
    ) -> Result<Value, DockerError> {
        let rows = self.list_containers().await?;
        let targets: Vec<_> = rows
            .iter()
            .filter(|r| r.labels.get(LABEL_PROJECT).is_some_and(|p| p == project))
            .collect();

        let mut report = Vec::new();
        let mut volume_names: BTreeSet<String> = BTreeSet::new();
        let mut network_names: BTreeSet<String> = BTreeSet::new();
        let prefix = project_prefix(project);

        for row in &targets {
            if row.protected || row.labels.get(LABEL_PROTECTED).is_some_and(|v| v == "true") {
                report.push(json!({
                    "container": row.name,
                    "status": "skipped",
                    "error": "受保护容器不参与 compose down",
                }));
                continue;
            }
            match self.inspect_container(&row.id).await {
                Ok(info) => {
                    for mount in info.mounts.unwrap_or_default() {
                        if mount.typ.as_deref() == Some("volume") {
                            if let Some(name) = mount.name {
                                if name.starts_with(&prefix) {
                                    volume_names.insert(name);
                                }
                            }
                        }
                    }
                    let attached: Vec<String> = info
                        .network_settings
                        .and_then(|n| n.networks)
                        .map(|n| n.into_keys().collect())
                        .unwrap_or_default();
                    for name in attached {
                        if name.starts_with(&prefix) {
                            network_names.insert(name);
                        }
                    }
                }
                // inspect 失败以前被 `if let Ok(info)` 静默跳过：卷/网络收集不到，
                // 报告里也没有痕迹，用户只看到「已删除」。现在必须显式留一行。
                Err(e) => report.push(inspect_failed_entry(&row.name, &e.to_string())),
            }

            if let Err(e) = self.stop_container(&row.id).await {
                report.push(json!({
                    "container": row.name,
                    "status": "stop-failed",
                    "error": e.to_string(),
                }));
            }
            match self.remove_container(&row.id, false).await {
                Ok(()) => report.push(json!({
                    "container": row.name,
                    "status": "removed",
                })),
                Err(e) => report.push(json!({
                    "container": row.name,
                    "status": "remove-failed",
                    "error": e.to_string(),
                })),
            }
        }

        let networks = self.list_networks().await?;
        let by_name: HashMap<&str, &str> = networks
            .iter()
            .map(|n| (n.name.as_str(), n.id.as_str()))
            .collect();
        let mut removed_networks = Vec::new();
        for name in &network_names {
            if let Some(id) = by_name.get(name.as_str()) {
                match self.remove_network(id).await {
                    Ok(()) => removed_networks.push(name.clone()),
                    Err(e) => report.push(json!({
                        "network": name,
                        "status": "remove-failed",
                        "error": e.to_string(),
                    })),
                }
            }
        }

        let mut removed_volumes = Vec::new();
        if remove_volumes {
            for name in &volume_names {
                match self.remove_volume(name).await {
                    Ok(()) => removed_volumes.push(name.clone()),
                    Err(e) => report.push(json!({
                        "volume": name,
                        "status": "remove-failed",
                        "error": e.to_string(),
                    })),
                }
            }
        }

        Ok(json!({
            "project": project,
            "failed": count_down_failures(&report),
            "containers": report,
            "removed_networks": removed_networks,
            "removed_volumes": removed_volumes,
            "kept_volumes": if remove_volumes { Vec::<String>::new() } else { volume_names.into_iter().collect::<Vec<_>>() },
        }))
    }

    async fn ensure_networks(&self, wanted: &[String]) -> Result<(), DockerError> {
        if wanted.is_empty() {
            return Ok(());
        }
        let existing = self.list_networks().await?;
        let names: BTreeSet<&str> = existing.iter().map(|n| n.name.as_str()).collect();
        for name in wanted {
            if !names.contains(name.as_str()) {
                self.create_network(name).await?;
            }
        }
        Ok(())
    }

    async fn ensure_volumes(&self, wanted: &[String]) -> Result<(), DockerError> {
        if wanted.is_empty() {
            return Ok(());
        }
        let existing = self.list_volumes().await?;
        let names: BTreeSet<&str> = existing.iter().map(|v| v.name.as_str()).collect();
        for name in wanted {
            if !names.contains(name.as_str()) {
                self.create_volume(name).await?;
            }
        }
        Ok(())
    }

    /// 返回拉取失败的镜像集合；已存在的镜像不重复拉取。
    async fn ensure_images(&self, project: &ComposeProject) -> BTreeSet<String> {
        let unique: BTreeSet<String> = project
            .services
            .iter()
            .map(|s| s.spec.image.clone())
            .collect();
        let mut failed = BTreeSet::new();
        for image in unique {
            if self.inspect_image(&image).await.is_ok() {
                continue;
            }
            if self.pull_image(&image).await.is_err() {
                failed.insert(image);
            }
        }
        failed
    }
}

/// 容器列表里的 compose 归属：优先内核标签，回落 Docker compose 标签。
pub fn project_of(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels
        .get(LABEL_PROJECT)
        .or_else(|| labels.get(LABEL_COMPOSE_PROJECT))
        .map(String::as_str)
}

pub fn service_of(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels.get(LABEL_COMPOSE_SERVICE).map(String::as_str)
}

/// 部署报告里「没有成功启动」的服务数：`started` 之外（failed /
/// created-not-started / skipped）都算。纯函数，便于在无 Docker 的环境里
/// 锁定「部分成功不被当成整体成功」的语义。
pub(crate) fn count_unstarted(services: &[Value]) -> usize {
    services
        .iter()
        .filter(|s| s.get("status").and_then(Value::as_str) != Some("started"))
        .count()
}

/// 清理报告里显式失败的行数（stop / remove / inspect 失败）。
/// 受保护容器的 `skipped` 是预期行为，不计入失败。
pub(crate) fn count_down_failures(rows: &[Value]) -> usize {
    rows.iter()
        .filter(|r| {
            r.get("status")
                .and_then(Value::as_str)
                .is_some_and(|s| s.ends_with("-failed"))
        })
        .count()
}

/// inspect 失败的报告行：没有它，卷/网络收集失败就完全不可见。
fn inspect_failed_entry(container: &str, error: &str) -> Value {
    json!({
        "container": container,
        "status": "inspect-failed",
        "error": error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_lookup_prefers_kernel_label() {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_COMPOSE_PROJECT.to_string(), "external".to_string());
        assert_eq!(project_of(&labels), Some("external"));
        labels.insert(LABEL_PROJECT.to_string(), "mine".to_string());
        assert_eq!(project_of(&labels), Some("mine"));
    }

    #[test]
    fn compose_label_style_containers_are_recognized() {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_COMPOSE_PROJECT.to_string(), "demo".to_string());
        labels.insert(LABEL_COMPOSE_SERVICE.to_string(), "web".to_string());
        assert_eq!(project_of(&labels), Some("demo"));
        assert_eq!(service_of(&labels), Some("web"));

        // 外部 compose 容器没有 io.skiff.project，不会被 compose down 清理。
        let mut kernel = BTreeMap::new();
        kernel.insert(LABEL_PROJECT.to_string(), "demo".to_string());
        assert_eq!(project_of(&kernel), Some("demo"));
    }

    /// 部分成功必须能被数出来：`started` 之外（失败、创建未启动、依赖跳过）
    /// 都算未成功，否则上层会把 2xx 当整体成功。
    #[test]
    fn up_report_counts_partial_failures() {
        let report = vec![
            json!({"service": "a", "status": "started"}),
            json!({"service": "b", "status": "failed", "error": "image pull"}),
            json!({"service": "c", "status": "created-not-started", "error": "port busy"}),
            json!({"service": "d", "status": "skipped", "error": "依赖服务 b 失败"}),
        ];
        assert_eq!(count_unstarted(&report), 3);
        assert_eq!(count_unstarted(&report[..1]), 0);
    }

    /// down 的失败行（stop / remove / inspect）都要计入 `failed`，
    /// 受保护容器的 `skipped` 不算失败。
    #[test]
    fn down_report_counts_stop_remove_and_inspect_failures() {
        let rows = vec![
            json!({"container": "a", "status": "removed"}),
            json!({"container": "b", "status": "stop-failed", "error": "timeout"}),
            json!({"container": "c", "status": "remove-failed", "error": "busy"}),
            json!({"container": "d", "status": "inspect-failed", "error": "engine busy"}),
            json!({"container": "e", "status": "skipped", "error": "受保护容器"}),
        ];
        assert_eq!(count_down_failures(&rows), 3);
    }

    /// inspect 失败必须显式留痕，不能像旧实现那样静默跳过。
    #[test]
    fn inspect_failure_is_recorded_not_silently_skipped() {
        let row = inspect_failed_entry("web", "engine busy");
        assert_eq!(row["status"], "inspect-failed");
        assert_eq!(row["container"], "web");
        assert_eq!(count_down_failures(&[row]), 1);
    }
}
