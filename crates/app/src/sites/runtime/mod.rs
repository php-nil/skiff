//! 运行环境：容器编排、镜像、扩展安装与 PHP/Node 配置。
//!
//! 动词仍经 `DockerService::execute` 单点分发；这里只承载 `AppState` 上运行
//! 环境相关的方法，站点对象生命周期在 `sites::crud`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::{DockerParams, Verb};
use skiff_store::{NewRuntime, Runtime};

use super::*;
use crate::sites::runtime_catalog;
use crate::AppState;

/// Node.js 启停后等待应用可响应的上限。
///
/// 「启动/重启」在 UI 请求路径上同步执行，等待必须短于 web 层 30 秒请求超时，
/// 否则用户只会看到 504，而拿不到「未通过健康检查」这个真实结论。web 侧有单测
/// 锁定这个大小关系。
pub const NODE_HEALTH_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

pub mod jobs;
pub mod php;

impl AppState {
    pub async fn runtime_views(&self) -> Result<Vec<RuntimeView>, SiteError> {
        let runtimes = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        let docker_available = self.docker().available();
        let mut docker_known = docker_available;
        let rows = if docker_available {
            match self.docker().list_containers().await {
                Ok(rows) => rows,
                Err(_) => {
                    // Docker 可以连上但列表失败：状态未知，不能当成「容器不存在」。
                    docker_known = false;
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let jobs = self.runtime_jobs_snapshot().await;
        let s = self.site_settings();
        let mut out = Vec::new();
        // image_id → 待 inspect 的视图下标（同一镜像多实例只 inspect 一次）。
        let mut pending_images: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for rt in runtimes {
            let implemented = runtime_catalog::is_implemented(&rt.kind);
            let container_name = runtime_container_name_for(&rt.kind, &rt.name, &rt.id);
            let container_row = rows.iter().find(|c| c.name == container_name);
            let container_exists = container_row.is_some();
            let image_id = container_row
                .map(|c| c.image_id.clone())
                .unwrap_or_default();
            let container_image = container_row.map(|c| c.image.clone()).unwrap_or_default();
            // 容器可能登记为 `image@digest`；比较时只看仓库:tag 部分，避免误报漂移。
            let container_ref_base = container_image.split('@').next().unwrap_or("");
            let image_mismatch =
                container_exists && !container_image.is_empty() && container_ref_base != rt.image;
            let container_state =
                probe_container_state(docker_known, container_row.map(|c| c.state.as_str()));
            let job = jobs.iter().find(|j| j.runtime_id == rt.id).cloned();
            let job_active = job.as_ref().is_some_and(RuntimeJob::active);
            let job_failed = job
                .as_ref()
                .is_some_and(|j| j.state == RuntimeJobState::Failed);
            let effective_state = if !implemented {
                "unimplemented".to_string()
            } else if job_active {
                "installing".to_string()
            } else if job_failed && container_state != "running" {
                // 安装失败且容器没起来：列表状态要能看出失败，而不是笼统的「未安装」。
                "error".to_string()
            } else {
                runtime_effective_state(&container_state).to_string()
            };
            let socket_host = rt.socket_path.clone().unwrap_or_default();
            let socket_container = if socket_host.is_empty() {
                String::new()
            } else {
                s.container_path(Path::new(&socket_host))
            };
            let socket_exists = !socket_host.is_empty() && Path::new(&socket_host).exists();
            let socket_mode = socket_mode(&socket_host);
            let site_count = self
                .db()
                .runtime_site_count(&rt.id)
                .map_err(|e| SiteError::Internal(e.to_string()))?;
            let drift = implemented
                && docker_known
                && !job_active
                && rt.desired_state == "running"
                && effective_state != "running"
                && effective_state != "missing"
                && effective_state != "unknown";
            // 写回缓存状态；'unimplemented' 不是合法枚举值，跳过；任务执行中由任务收尾写入。
            // Docker 不可达时状态未知，不能把 unknown 写回成 stopped，避免污染下次判断。
            if implemented && !job_active && docker_known {
                let cached = if matches!(
                    effective_state.as_str(),
                    "running" | "stopped" | "error" | "installing"
                ) {
                    effective_state.clone()
                } else {
                    "stopped".into()
                };
                if let Err(e) = self
                    .db()
                    .set_runtime_status(&rt.id, &rt.desired_state, &cached)
                {
                    tracing::warn!(runtime = %rt.id, error = %e, "回写运行环境状态失败");
                }
            }
            // 真实版本来自镜像元数据（image_id 不可变），与容器是否运行无关：
            // 停止但容器仍在时照样显示真实版本。全程不进容器执行进程。
            let actual_version =
                if !image_id.is_empty() && runtime_catalog::version_env_key(&rt.kind).is_some() {
                    let cache = self.image_versions.lock().await;
                    match image_version_lookup(&cache, &image_id, std::time::Instant::now()) {
                        ImageVersionHit::Known(v) => Some(v),
                        ImageVersionHit::Suppressed => None,
                        ImageVersionHit::Miss => {
                            pending_images
                                .entry(image_id.clone())
                                .or_default()
                                .push(out.len());
                            None
                        }
                    }
                } else {
                    None
                };
            let extensions = runtime_catalog::split_extensions(&rt.extensions);
            let image_digest = rt.image_digest.clone();
            out.push(RuntimeView {
                runtime: rt,
                implemented,
                effective_state,
                container_name,
                container_state,
                drift,
                site_count,
                socket_host,
                socket_container,
                socket_exists,
                socket_mode,
                actual_version,
                image_id,
                image_mismatch,
                image_digest,
                container_exists,
                extensions,
                job,
            });
        }

        // 未命中时每个 image_id 只 inspect 一次；失败写入 60 秒负缓存，不阻塞后续渲染。
        for (image_id, indices) in pending_images {
            let version = self
                .docker()
                .inspect_image(&image_id)
                .await
                .ok()
                .and_then(|info| {
                    let kind = indices
                        .first()
                        .and_then(|i| out.get(*i))
                        .map(|v| v.runtime.kind.as_str())
                        .unwrap_or_default();
                    let key = runtime_catalog::version_env_key(kind)?;
                    info.env_value(key)
                });
            self.image_versions.lock().await.insert(
                image_id.clone(),
                (version.clone(), std::time::Instant::now()),
            );
            for idx in indices {
                if let Some(view) = out.get_mut(idx) {
                    view.actual_version = version.clone();
                }
            }
        }
        Ok(out)
    }

    pub async fn runtime_view(&self, id: &str) -> Result<RuntimeView, SiteError> {
        self.runtime_views()
            .await?
            .into_iter()
            .find(|v| v.runtime.id == id)
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))
    }

    pub async fn web_env_action(
        self: &Arc<Self>,
        action: &str,
        actor: &str,
        role: &str,
        ip: &str,
    ) -> Result<(), SiteError> {
        match action {
            "install" | "rebuild" => {
                self.start_web_env_install(actor, role, ip, action)?;
                Ok(())
            }
            "start" | "stop" | "restart" => {
                let verb = match action {
                    "start" => Verb::Start,
                    "stop" => Verb::Stop,
                    _ => Verb::Restart,
                };
                self.docker_runtime_action(NGINX_CONTAINER, verb).await
            }
            "delete" => {
                let rows = self
                    .docker()
                    .list_containers()
                    .await
                    .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
                let Some(c) = rows.iter().find(|c| c.name == NGINX_CONTAINER) else {
                    return Ok(());
                };
                self.remove_existing_container(c)
                    .await
                    .map_err(SiteError::Internal)
            }
            _ => Err(SiteError::BadRequest(format!(
                "不支持的 Web 服务操作：{action}"
            ))),
        }
    }

    pub(crate) async fn kernel_execute(
        &self,
        verb: Verb,
        target: Option<&skiff_docker::ObjectRef>,
        params: DockerParams,
    ) -> Result<serde_json::Value, String> {
        self.cfg
            .policy
            .check(&skiff_model::Actor::System, verb, target)
            .map_err(|e| e.to_string())?;
        self.docker()
            .execute(verb, target, params)
            .await
            .map_err(|e| e.to_string())
    }

    async fn docker_runtime_action(&self, name: &str, verb: Verb) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows.iter().find(|c| c.name == name).ok_or_else(|| {
            SiteError::Unavailable(format!("容器 {name} 未安装，请先安装运行环境"))
        })?;
        let obj = skiff_docker::ObjectRef {
            id: c.id.clone(),
            name: c.name.clone(),
            labels: c.labels.clone(),
        };
        self.kernel_execute(verb, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| SiteError::Unavailable(format!("容器 {name} 操作失败：{e}")))
    }

    pub async fn runtime_action(
        self: &Arc<Self>,
        id: &str,
        action: &str,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境尚未接入，不提供安装/启停/重建",
                rt.kind
            )));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        match action {
            "install" | "rebuild" => {
                // 重建与安装同一条异步链路：默认读取库里的扩展集并重放。
                self.start_runtime_install(id, actor, role).await?;
                return Ok(());
            }
            "start" => {
                self.docker_runtime_action(&container, Verb::Start).await?;
                if let Err(e) = self.node_health_gate(&rt).await {
                    let _ = self.db().set_runtime_status(id, "running", "error");
                    return Err(e);
                }
                self.db()
                    .set_runtime_status(id, "running", "running")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "stop" => {
                // 停止不换版本、不改绑定：绑定站点继续指向本版本 socket，访问返回
                // 502/503；站点页会给出「前往运行环境页」的入口。
                self.docker_runtime_action(&container, Verb::Stop).await?;
                self.db()
                    .set_runtime_status(id, "stopped", "stopped")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "restart" => {
                self.docker_runtime_action(&container, Verb::Restart)
                    .await?;
                if let Err(e) = self.node_health_gate(&rt).await {
                    let _ = self.db().set_runtime_status(id, "running", "error");
                    return Err(e);
                }
                self.db()
                    .set_runtime_status(id, "running", "running")
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            }
            "sync" => {
                let current = self.runtime_view(id).await?;
                // 以容器实测为准，纠正「期望运行但实际停了」的漂移缓存。
                if current.effective_state == "running" {
                    if let Err(e) = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "running")
                    {
                        tracing::warn!(runtime = id, error = %e, "回写运行环境状态失败");
                    }
                } else if current.effective_state == "stopped" {
                    if let Err(e) = self
                        .db()
                        .set_runtime_status(id, &rt.desired_state, "stopped")
                    {
                        tracing::warn!(runtime = id, error = %e, "回写运行环境状态失败");
                    }
                }
            }
            other => return Err(SiteError::BadRequest(format!("未知运行环境操作：{other}"))),
        }
        Ok(())
    }

    async fn node_health_gate(&self, rt: &Runtime) -> Result<(), SiteError> {
        if rt.kind != skiff_store::KIND_NODE {
            return Ok(());
        }
        let port = rt
            .port
            .ok_or_else(|| SiteError::Unavailable("Node.js 环境缺少端口，请重建".into()))?;
        let ip = self.ensure_runtime_ip(rt).await?;
        let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
        let container = runtime_container_name(&rt.kind, &rt.name);
        match wait_runtime_ready(
            self.docker(),
            &container,
            &ip,
            port,
            Some(&cfg.health_path),
            NODE_HEALTH_BUDGET,
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(failure) => Err(SiteError::Unavailable(readiness_message(
                "Node.js",
                &ip,
                port,
                Some(&cfg.health_path),
                &failure,
            ))),
        }
    }

    pub async fn change_runtime_port(
        self: &Arc<Self>,
        id: &str,
        port: u16,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let used: Vec<u16> = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .into_iter()
            .filter(|r| r.id != rt.id)
            .filter_map(|r| r.port)
            .collect();
        validate_process_port(&rt.kind, port, &used).map_err(SiteError::BadRequest)?;
        if rt.port == Some(port) {
            return Ok(());
        }
        // 已有任务时不能改端口：库与 vhost 先变、容器却没重建会留下半成品状态。
        if self.runtime_job(id).await.is_some_and(|j| j.active()) {
            return Err(SiteError::Conflict(
                "该运行环境有进行中的任务，请等待任务结束后再改端口".into(),
            ));
        }
        self.db()
            .set_runtime_port(id, port)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // vhost 里写死了旧端口，必须一起重写，否则站点反代到一个不再监听的端点。
        // 端口已在库中更新，这里只告警不中断：nginx 未启动或重载失败时仍要让容器按新端口重建，
        // 否则会出现「库/容器新端口、nginx 旧端口」的半成品状态。
        let s = self.site_settings();
        let domains = self
            .db()
            .runtime_site_domains(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        for domain in &domains {
            if let Err(e) = self.write_site_vhost(&s, domain) {
                tracing::warn!(runtime = id, site = %domain, error = %e, "改端口后重写站点 vhost 失败");
            }
        }
        if !domains.is_empty() {
            if let Err(e) = self.validate_nginx().await {
                tracing::warn!(runtime = id, error = %e, "改端口后 nginx 配置校验失败");
            }
            if let Err(e) = self.restart_nginx().await {
                tracing::warn!(runtime = id, error = %e, "改端口后重载 nginx 失败");
            }
        }
        // 容器 Env 里的 PORT 来自环境行，必须重建才能换端口。
        self.start_runtime_install(id, actor, role).await?;
        Ok(())
    }

    pub async fn change_runtime_publish(
        self: &Arc<Self>,
        id: &str,
        publish_port: Option<u16>,
        actor: &str,
        role: &str,
    ) -> Result<(), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !matches!(
            rt.kind.as_str(),
            skiff_store::KIND_NODE | skiff_store::KIND_RUST
        ) {
            return Err(SiteError::BadRequest(format!(
                "{} 运行环境不占用宿主端口，无需对外发布",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        if rt.publish_port == publish_port {
            return Ok(());
        }
        if let Some(port) = publish_port {
            if port < 1024 {
                return Err(SiteError::BadRequest(format!(
                    "对外发布端口必须不小于 1024，{port} 属于特权端口"
                )));
            }
            let used: Vec<u16> = self
                .db()
                .list_runtimes()
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .into_iter()
                .filter(|r| r.id != rt.id)
                .filter_map(|r| r.publish_port)
                .collect();
            if used.contains(&port) {
                return Err(SiteError::BadRequest(format!(
                    "宿主端口 {port} 已被其它运行环境对外发布"
                )));
            }
            check_port_free(port).map_err(SiteError::BadRequest)?;
        }
        if self.runtime_job(id).await.is_some_and(|j| j.active()) {
            return Err(SiteError::Conflict(
                "该运行环境有进行中的任务，请等待任务结束后再修改对外发布".into(),
            ));
        }
        self.db()
            .set_runtime_publish_port(id, publish_port)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // 端口映射在建容器时固定，必须重建容器；站点 vhost 不含宿主端口，无需改动。
        self.start_runtime_install(id, actor, role).await?;
        Ok(())
    }

    pub async fn clone_runtime(
        self: &Arc<Self>,
        id: &str,
        raw_name: &str,
        actor: &str,
        role: &str,
    ) -> Result<Runtime, SiteError> {
        let src = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !matches!(
            src.kind.as_str(),
            skiff_store::KIND_NODE | skiff_store::KIND_RUST
        ) {
            return Err(SiteError::BadRequest(format!(
                "{} 运行环境暂不支持克隆",
                runtime_catalog::kind_label(&src.kind)
            )));
        }
        let s = self.site_settings();
        let name = {
            let raw = raw_name.trim();
            if raw.is_empty() {
                format!("{}-copy", src.name)
            } else {
                raw.to_string()
            }
        };
        skiff_store::validate_runtime_name(&name)
            .map_err(|e| SiteError::BadRequest(format!("环境名不合法：{e}")))?;
        let dir = s.runtime_home(&src.kind, &name);
        if dir.exists() {
            return Err(SiteError::Conflict(format!(
                "目标目录已存在：{}，请换一个名字",
                dir.display()
            )));
        }
        if self
            .db()
            .runtime_by_name(&src.kind, &name)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .is_some()
        {
            return Err(SiteError::Conflict(format!(
                "{} 环境已存在：{name}",
                runtime_catalog::kind_label(&src.kind)
            )));
        }
        let used: Vec<u16> = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .into_iter()
            .filter(|r| r.kind == src.kind)
            .filter_map(|r| r.port)
            .collect();
        let port = runtime_catalog::allocate_port(&src.kind, &used).ok_or_else(|| {
            SiteError::Conflict(format!(
                "{} 端口区间已用满，请先删除不再使用的环境",
                runtime_catalog::kind_label(&src.kind)
            ))
        })?;
        let image =
            runtime_catalog::image_for(&src.kind, &src.version, &s.php_image, &s.image_prefix);
        let cloned = self
            .db()
            .create_runtime(&NewRuntime {
                id: format!("{}-{}", src.kind, name),
                kind: src.kind.clone(),
                name,
                version: src.version.clone(),
                image,
                image_digest: String::new(),
                socket_path: None,
                port: Some(port),
                is_default: false,
            })
            .map_err(|e| SiteError::Conflict(format!("创建运行环境失败：{e}")))?;
        if let Err(e) = self.db().set_runtime_config(&cloned.id, &src.config_json) {
            tracing::warn!(runtime = %cloned.id, error = %e, "克隆运行环境配置失败");
        }
        if let Err(e) = copy_runtime_app(
            self.admin_fs(),
            &s.runtime_home(&src.kind, &src.name),
            &s.runtime_home(&cloned.kind, &cloned.name),
        ) {
            tracing::warn!(runtime = %cloned.id, error = %e, "克隆代码目录失败，将使用自带脚手架");
        }
        self.start_runtime_install(&cloned.id, actor, role).await?;
        Ok(cloned)
    }

    pub async fn probe_runtime_health(&self, id: &str) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if !matches!(
            rt.kind.as_str(),
            skiff_store::KIND_NODE | skiff_store::KIND_RUST
        ) {
            return Err(SiteError::BadRequest(format!(
                "{} 运行环境没有独立应用进程",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        let port = rt.port.ok_or_else(|| {
            SiteError::Unavailable("运行环境缺少固定端口，请重新安装".to_string())
        })?;
        let container = runtime_container_name(&rt.kind, &rt.name);
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("Docker 不可达：{e}")))?;
        if rows.iter().all(|c| c.name != container) {
            return Err(SiteError::Unavailable(
                "容器不存在，请先安装运行环境".into(),
            ));
        }
        // 容器状态与地址可达性交给同一条就绪判定，避免这里与启动闸门口径不一致。
        let ip = self.ensure_runtime_ip(&rt).await?;
        let (http_path, subject) = if rt.kind == skiff_store::KIND_NODE {
            (
                Some(NodeRuntimeConfig::from_json(&rt.config_json).health_path),
                "Node.js",
            )
        } else {
            (None, "Rust")
        };
        match check_runtime_ready(self.docker(), &container, &ip, port, http_path.as_deref()).await
        {
            Ok(()) => Ok(match http_path.as_deref() {
                Some(p) => format!("应用就绪：容器地址 {ip}:{port}，HTTP {p} 返回 2xx/3xx"),
                None => format!("应用就绪：容器地址 {ip}:{port} 可连接"),
            }),
            Err(failure) => Err(SiteError::Unavailable(readiness_message(
                subject,
                &ip,
                port,
                http_path.as_deref(),
                &failure,
            ))),
        }
    }

    pub async fn add_runtime(
        self: &Arc<Self>,
        kind: &str,
        raw_name: &str,
        version: &str,
        extensions: &[String],
        actor: &str,
        role: &str,
    ) -> Result<(Runtime, bool), SiteError> {
        let kind = kind.trim();
        let def = runtime_catalog::kind_def(kind)
            .ok_or_else(|| SiteError::BadRequest(format!("未知运行环境类型：{kind}")))?;
        let version = version.trim();
        // 页面提示「留空使用该类型默认版本」：大版本清单每类只有一个默认（首项），
        // 补丁号版本必须显式给出，不做猜测。
        let version = if version.is_empty() {
            def.versions.first().copied().unwrap_or("")
        } else {
            version
        };
        if !runtime_catalog::version_supported(kind, version) {
            return Err(SiteError::BadRequest(format!(
                "{} 版本无效：可选大版本 {}；带补丁号的版本须落在大版本内（如 8.5.13 / 1.99.1）",
                def.label,
                def.versions.join(" / ")
            )));
        }
        let raw_name = raw_name.trim();
        let name = if raw_name.is_empty() {
            runtime_catalog::default_name(kind, version)
        } else {
            skiff_store::validate_runtime_name(raw_name)
                .map_err(|e| SiteError::BadRequest(e.to_string()))?
                .to_string()
        };
        let mut inherit_default = false;
        if let Some(existing) = self
            .db()
            .runtime_by_name(kind, &name)
            .map_err(|e| SiteError::Internal(e.to_string()))?
        {
            // 迁移种入、从未安装且无站点引用的模板行可被同名新建接管；
            // 其它已存在的实例仍按重名拒绝。
            let seed_free = existing.origin == skiff_store::ORIGIN_SEED
                && self
                    .db()
                    .runtime_site_count(&existing.id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?
                    == 0;
            if seed_free {
                inherit_default = existing.is_default;
                self.db()
                    .delete_runtime(&existing.id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?;
            } else {
                return Err(SiteError::Conflict(format!(
                    "同类型下已存在名为 {name} 的运行环境，请换一个名字"
                )));
            }
        }
        let s = self.site_settings();
        if runtime_catalog::is_implemented(kind) {
            let dir = s.runtime_home(kind, &name);
            if dir.exists() {
                return Err(SiteError::Conflict(format!(
                    "目标目录已存在：{}，请换一个名字（不会覆盖或自动加后缀）",
                    dir.display()
                )));
            }
        }
        let image = runtime_catalog::image_for(kind, version, &s.php_image, &s.image_prefix);
        let enabled = if kind == skiff_store::KIND_PHP {
            runtime_catalog::normalize_extensions(extensions)
        } else {
            Vec::new()
        };
        if kind == skiff_store::KIND_PHP {
            if let Err((code, reason)) =
                runtime_catalog::validate_selection(version, &image, &enabled)
            {
                return Err(SiteError::BadRequest(format!("{code}: {reason}")));
            }
        }
        // Node.js / Rust 是「独立进程 + 固定端口」：建环境时一次性分配，冲突直接拒绝。
        // PHP 走 socket，不占宿主端口。
        let port = if let Some(range) = runtime_catalog::port_range(kind) {
            let used: Vec<u16> = self
                .db()
                .list_runtimes()
                .map_err(|e| SiteError::Internal(e.to_string()))?
                .iter()
                .filter_map(|r| r.port)
                .collect();
            Some(runtime_catalog::allocate_port(kind, &used).ok_or_else(|| {
                SiteError::Conflict(format!(
                    "{} 端口区间 {}–{} 已用满，请先删除不再使用的环境",
                    runtime_catalog::kind_label(kind),
                    *range.start(),
                    *range.end()
                ))
            })?)
        } else {
            None
        };
        // PHP 沿用「实例名即 id」的既有约定；其它类型加前缀避免与 PHP 实例名撞车。
        let id = if kind == skiff_store::KIND_PHP {
            name.clone()
        } else {
            format!("{kind}-{name}")
        };
        let rt = self
            .db()
            .create_runtime(&NewRuntime {
                id: id.clone(),
                kind: kind.to_string(),
                name: name.clone(),
                version: version.to_string(),
                image,
                image_digest: String::new(),
                socket_path: if kind == skiff_store::KIND_PHP {
                    Some(s.runtime_socket(kind, &name).display().to_string())
                } else {
                    None
                },
                port,
                is_default: inherit_default,
            })
            .map_err(|e| SiteError::Conflict(e.to_string()))?;
        let joined = runtime_catalog::join_extensions(&enabled);
        self.db()
            .set_runtime_extensions(&rt.id, &joined)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        let started = runtime_catalog::is_implemented(kind);
        if started {
            self.start_runtime_install(&rt.id, actor, role).await?;
        }
        Ok((rt, started))
    }

    pub async fn save_php_runtime_config(
        &self,
        id: &str,
        cfg: &PhpRuntimeConfig,
        actor: &str,
    ) -> Result<RuntimeView, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境不使用 php.ini / FPM 专属配置",
                rt.kind
            )));
        }
        cfg.validate()?;
        let s = self.site_settings();
        let socket_host = rt
            .socket_path
            .clone()
            .unwrap_or_else(|| s.runtime_socket(&rt.kind, &rt.name).display().to_string());
        let socket_container = s.container_path(Path::new(&socket_host));
        self.admin_fs()
            .ensure_dir_kernel(&s.runtime_home(&rt.kind, &rt.name))
            .map_err(|e| SiteError::Internal(format!("创建运行目录失败：{e}")))?;
        let pool = s.runtime_pool_conf(&rt.kind, &rt.name);
        let original = std::fs::read(&pool).ok();
        // 用户可见写操作统一走 skiff-fs：原子替换 + 审计，不在 app 层直接落盘。
        self.admin_fs()
            .write(
                actor,
                &pool.display().to_string(),
                &render_php_pool_with(cfg, &socket_container),
            )
            .map_err(|e| SiteError::Internal(format!("写入 PHP-FPM 配置失败：{e}")))?;
        if let Err(e) = self
            .docker_runtime_action(&runtime_container_name(&rt.kind, &rt.name), Verb::Restart)
            .await
        {
            if let Some(bytes) = &original {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    if let Err(we) = self
                        .admin_fs()
                        .write(actor, &pool.display().to_string(), text)
                    {
                        tracing::error!(path = %pool.display(), error = %we, "回滚 PHP 池配置写入失败");
                    }
                }
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚：重启 PHP 容器失败：{e}"
            )));
        }
        self.db()
            .set_runtime_config(id, &cfg.to_json())
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        self.runtime_view(id).await
    }

    pub async fn save_node_runtime_config(
        &self,
        id: &str,
        cfg: &NodeRuntimeConfig,
        actor: &str,
    ) -> Result<(RuntimeView, bool), SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_NODE {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境没有 Node.js 专属配置",
                rt.kind
            )));
        }
        cfg.validate()?;
        let previous = NodeRuntimeConfig::from_json(&rt.config_json);
        let changed = &previous != cfg;
        if changed {
            self.db()
                .set_runtime_config(id, &cfg.to_json())
                .map_err(|e| SiteError::Internal(e.to_string()))?;
            tracing::info!(actor = %actor, runtime = %id, "Node.js 运行环境配置已更新");
        }
        Ok((self.runtime_view(id).await?, changed))
    }

    pub async fn unbind_runtime_site(
        &self,
        runtime_id: &str,
        domain: &str,
    ) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if site.runtime_id.as_deref() != Some(runtime_id) {
            return Err(SiteError::BadRequest(format!(
                "站点 {domain} 未绑定到该运行环境"
            )));
        }
        let s = self.site_settings();
        let site_dir = PathBuf::from(&site.root_dir);
        let vhost = s.vhost_file(&site_dir, domain);
        let disabled = s.vhost_disabled(&site_dir, domain);
        if vhost.exists() {
            self.admin_fs()
                .rename_kernel(&vhost, &disabled)
                .map_err(|e| SiteError::Internal(format!("停用 vhost 失败：{e}")))?;
        }
        self.db()
            .set_site_static_unbound(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        // 停用 vhost 后重载 nginx；容器不在运行则留待下次启动加载。
        // 与证书成对写入共用一把锁：reload 不得在 cert/key 两次 rename 之间插入。
        let _guard = super::nginx_write_lock().lock().await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows
                .iter()
                .find(|c| c.name == NGINX_CONTAINER && c.state == "running")
            {
                if let Err(e) = self
                    .docker()
                    .exec_capture(&c.id, vec!["nginx".into(), "-s".into(), "reload".into()])
                    .await
                {
                    tracing::warn!(container = %c.name, error = %e, "nginx -s reload 失败");
                }
            }
        }
        Ok(())
    }

    pub async fn delete_site_runtime(&self, id: &str) -> Result<RuntimeDeleteReport, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let domains = self
            .db()
            .runtime_site_domains(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        if !domains.is_empty() {
            return Err(SiteError::Conflict(format!(
                "运行环境仍被以下站点绑定：{}；请先删除这些站点后再删除运行环境",
                domains.join("、")
            )));
        }
        if self.runtime_job(id).await.is_some_and(|j| j.active()) {
            return Err(SiteError::Conflict(
                "该运行环境有进行中的任务，请等待任务结束后再删除".into(),
            ));
        }

        let s = self.site_settings();
        let container = runtime_container_name(&rt.kind, &rt.name);
        let mut report = RuntimeDeleteReport::default();

        if self.docker().available() {
            let rows = self.docker().list_containers().await.map_err(|e| {
                partial_delete_error(
                    &report.completed,
                    format!("读取容器列表失败，未清理任何容器：{e}"),
                )
            })?;
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                let obj = skiff_docker::ObjectRef {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    labels: c.labels.clone(),
                };
                if matches!(c.state.as_str(), "running" | "restarting" | "paused") {
                    self.kernel_execute(Verb::Stop, Some(&obj), DockerParams::None)
                        .await
                        .map_err(|e| {
                            partial_delete_error(
                                &report.completed,
                                format!("停止容器 {container} 失败：{e}"),
                            )
                        })?;
                    report.completed.push(format!("容器 {container} 已停止"));
                }
                self.kernel_execute(
                    Verb::ContainerRemove,
                    Some(&obj),
                    DockerParams::ContainerRemove { force: true },
                )
                .await
                .map_err(|e| {
                    partial_delete_error(
                        &report.completed,
                        format!("删除容器 {container} 失败：{e}"),
                    )
                })?;
                report.completed.push(format!("容器 {container} 已删除"));
            } else {
                report
                    .completed
                    .push(format!("容器 {container} 不存在（无需删除）"));
            }
        } else {
            // Docker 不可用无法确认容器是否存在；不阻塞目录与记录清理，但明确告知。
            report.skipped.push(format!(
                "Docker 不可用（{}），未确认/清理容器 {container}",
                self.docker_unavailable_reason()
            ));
        }

        let dir = s.runtime_home(&rt.kind, &rt.name);
        if dir.exists() {
            self.admin_fs().remove_tree_kernel(&dir).map_err(|e| {
                partial_delete_error(
                    &report.completed,
                    format!("删除运行环境目录 {} 失败：{e}", dir.display()),
                )
            })?;
            report.completed.push(format!(
                "运行环境目录已删除（含 FPM 池配置与 socket）：{}",
                dir.display()
            ));
        } else {
            report
                .completed
                .push("运行环境目录不存在（无需删除）".into());
        }

        self.db().delete_runtime(id).map_err(|e| {
            partial_delete_error(&report.completed, format!("删除运行环境记录失败：{e}"))
        })?;
        report.completed.push("数据库记录已删除".into());
        Ok(report)
    }

    pub async fn runtime_logs(&self, id: &str, tail: usize) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if matches!(
            rt.kind.as_str(),
            skiff_store::KIND_RUST | skiff_store::KIND_NODE
        ) {
            let s = self.site_settings();
            let path = s.runtime_log_dir(&rt.kind, &rt.name).join("server.log");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(tail);
            return Ok(lines[start..].join("\n"));
        }
        if !runtime_catalog::is_implemented(&rt.kind) {
            return Ok("（Go 运行环境尚未接入）".into());
        }
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == runtime_container_name(&rt.kind, &rt.name))
            .ok_or_else(|| {
                SiteError::Unavailable("PHP-FPM 容器未安装，请先在运行环境页安装".into())
            })?;
        self.docker()
            .logs(&c.id, tail)
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取 PHP-FPM 日志失败：{e}")))
    }

    pub async fn probe_runtime_version(&self, id: &str) -> Result<String, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == runtime_container_name(&rt.kind, &rt.name))
            .filter(|c| c.state == "running")
            .ok_or_else(|| {
                SiteError::Unavailable("容器未运行，无法实测版本（镜像元数据仍可显示）".into())
            })?;
        let (cmd, strip): (Vec<String>, &str) = match rt.kind.as_str() {
            skiff_store::KIND_PHP => (
                vec!["php".into(), "-r".into(), "echo PHP_VERSION;".into()],
                "",
            ),
            skiff_store::KIND_RUST => (vec!["rustc".into(), "--version".into()], "rustc "),
            skiff_store::KIND_NODE => (vec!["node".into(), "-v".into()], ""),
            _ => (vec!["go".into(), "version".into()], ""),
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.docker().exec_capture(&c.id, cmd),
        )
        .await
        .map_err(|_| SiteError::Unavailable("2 秒内未取到实测版本（容器可能繁忙）".into()))?
        .map_err(|e| SiteError::Unavailable(format!("实测版本失败：{e}")))?;
        let (code, out) = result;
        if code != 0 {
            return Err(SiteError::Unavailable(format!(
                "实测版本失败（exit {code}）：{}",
                out.lines().next().unwrap_or("")
            )));
        }
        let value = out
            .trim()
            .strip_prefix(strip)
            .unwrap_or(out.trim())
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        if value.is_empty() {
            return Err(SiteError::Unavailable("实测版本输出为空".into()));
        }
        Ok(value)
    }
}
