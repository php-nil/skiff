//! 运行环境：容器编排、镜像、扩展安装与 PHP/Node 配置。
//!
//! 动词仍经 `DockerService::execute` 单点分发；这里只承载 `AppState` 上运行
//! 环境相关的方法，站点对象生命周期在 `sites::crud`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::{DockerParams, Verb};
use skiff_store::{NewRuntime, Runtime};

use super::*;
use crate::jobs as app_jobs;
use crate::runtime_catalog;
use crate::AppState;

impl AppState {
    /// 运行环境列表：库是事实源，容器状态只作实时探测（漂移徽章的依据）。
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
            let container_name = if implemented {
                runtime_container_name(&rt.kind, &rt.name)
            } else {
                format!("skiff-runtime-{}", rt.id)
            };
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

    /// 独立 Web 服务（OpenResty）操作。与任何 PHP/Node/Go/Rust 行完全解耦：
    /// 安装只拉 nginx 镜像、只建 nginx 容器，不触碰语言运行环境。
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

    /// 内核自身容器操作的唯一入口：先经 `policy.check(Actor::System, …)`，
    /// 再交给 `Docker::execute` 单点分发。
    ///
    /// 站点与运行环境的每一处容器动作都必须走这里：`Actor::System` 只放行内核
    /// 托管的对象（`io.skiff.project=sites`），外部面板对象仍被拒。用户角色鉴权
    /// 由调用方 handler 负责，这里只做对象面收口。
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

    /// 运行环境启停/重建/同步。Go 未接入，明确返回「尚未接入」。
    ///
    /// `install` / `rebuild` 只登记后台任务并立即返回；页面轮询任务状态与日志。
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

    /// Node 进程启动后的健康闸门：PHP / Rust 不需要（PHP 有 socket 探活，Rust 安装期
    /// 已等端口），只有 Node 需要在「手动启动/重启」时确认应用真的能响应。
    async fn node_health_gate(&self, rt: &Runtime) -> Result<(), SiteError> {
        if rt.kind != skiff_store::KIND_NODE {
            return Ok(());
        }
        let port = rt
            .port
            .ok_or_else(|| SiteError::Unavailable("Node.js 环境缺少端口，请重建".into()))?;
        let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
        if wait_http_ready(port, &cfg.health_path, std::time::Duration::from_secs(60)).await {
            Ok(())
        } else {
            Err(SiteError::Unavailable(format!(
                "Node.js 进程未通过健康检查（{}），请查看运行环境日志",
                cfg.health_path
            )))
        }
    }

    /// 最近一次任务快照（列表/抽屉轮询）。
    pub async fn runtime_job(&self, id: &str) -> Option<RuntimeJob> {
        let job = self
            .jobs()
            .latest_for_target(app_jobs::TARGET_RUNTIME, id)?;
        Some(self.runtime_job_view(&job, 300))
    }

    /// 全部运行环境任务（只含有任务记录的环境）。
    pub async fn runtime_jobs_snapshot(&self) -> Vec<RuntimeJob> {
        self.jobs()
            .list_for_target_type(app_jobs::TARGET_RUNTIME, 200)
            .iter()
            .map(|j| self.runtime_job_view(j, 60))
            .collect()
    }

    /// 作业 → 运行环境页投影；日志按时间升序带上 `[HH:MM:SS]` 前缀。
    fn runtime_job_view(&self, job: &app_jobs::Job, log_limit: i64) -> RuntimeJob {
        let logs = self
            .db()
            .job_log_tail(&job.id, log_limit)
            .unwrap_or_default()
            .into_iter()
            .map(|e| format!("[{}] {}", fmt_log_time(e.at), e.line))
            .collect();
        RuntimeJob::from_job(job, logs, chrono::Local::now().timestamp())
    }

    /// 容器是否被进行中的内核任务占用（Docker 页删除前的 BusyGuard）。
    ///
    /// 与 `delete_site_runtime` 同一口径：运行环境有安装/扩展任务、Web 服务
    /// 正在安装或重建时，都不允许从 Docker 页直接删除对应容器。
    pub async fn container_busy_reason(&self, container: &str) -> Option<String> {
        if container == NGINX_CONTAINER && self.web_install_progress().running {
            return Some("Web 服务正在安装或重建中，暂不能删除容器".into());
        }
        let runtimes = self.db().list_runtimes().unwrap_or_default();
        for rt in runtimes {
            if runtime_container_name(&rt.kind, &rt.name) != container {
                continue;
            }
            if self
                .runtime_job(&rt.id)
                .await
                .is_some_and(|job| job.active())
            {
                return Some(format!(
                    "运行环境 {} 有进行中的任务，暂不能删除容器",
                    rt.name
                ));
            }
        }
        None
    }

    /// 阶段推进。参数是**任务中心作业 id**（不是 runtime id），进度只在真实步骤落库。
    pub(crate) async fn job_advance(&self, job_id: &str, step: &str, progress: u8, line: &str) {
        self.job_advance_with(job_id, step, progress, line, true)
            .await;
    }

    /// 不可中断步骤（`docker exec` 安装脚本）推进：期间取消按钮应隐藏。
    async fn job_advance_blocking(&self, job_id: &str, step: &str, progress: u8, line: &str) {
        self.job_advance_with(job_id, step, progress, line, false)
            .await;
    }

    async fn job_advance_with(
        &self,
        job_id: &str,
        step: &str,
        progress: u8,
        line: &str,
        cancellable: bool,
    ) {
        self.jobs()
            .set_phase(job_id, step, step, progress as i64, cancellable);
        self.jobs().note(job_id, line);
    }

    pub(crate) async fn job_note(&self, job_id: &str, line: &str) {
        self.jobs().note(job_id, line);
    }

    async fn job_finish_ok(&self, job_id: &str, line: &str) {
        self.jobs().finish_ok(job_id, line, serde_json::json!({}));
    }

    async fn job_finish_err(&self, job_id: &str, error: &str) {
        match error.strip_prefix(TIMEOUT_MARK) {
            Some(msg) => self.job_finish_err_class(job_id, "timeout", msg).await,
            None => self.job_finish_err_class(job_id, "failed", error).await,
        }
    }

    async fn job_finish_err_class(&self, job_id: &str, class: &str, error: &str) {
        self.jobs().finish_err(job_id, class, error);
    }

    /// 执行体在阶段边界检查取消标记。
    async fn job_cancelled(&self, job_id: &str) -> bool {
        self.jobs().cancel_requested(job_id)
    }

    /// 把命令输出按行写入任务日志（只保留尾部，防止一次 pull 撑爆轮询响应）。
    async fn job_log_output(&self, job_id: &str, output: &str) {
        let mut lines: Vec<String> = output
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| format!("  {l}"))
            .collect();
        let keep = 30;
        if lines.len() > keep {
            lines.drain(..lines.len() - keep);
        }
        self.jobs().log_lines(job_id, "out", &lines);
    }

    /// 启动一次运行环境安装（异步）。只登记作业，立即返回。
    pub async fn start_runtime_install(
        self: &Arc<Self>,
        id: &str,
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
                "{} 运行环境尚未接入，不提供安装",
                runtime_catalog::kind_label(&rt.kind)
            )));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!("安装运行环境 {}", rt.name),
                params: serde_json::json!({
                    "runtime_id": id,
                    "kind": rt.kind,
                    "name": rt.name,
                    "version": rt.version,
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        if let Err(e) = self.db().set_runtime_status(id, "running", "installing") {
            tracing::warn!(runtime = id, error = %e, "写入运行环境安装中状态失败");
        }
        let state = self.clone();
        let rid = id.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state.run_runtime_install_job(&rid, &job_id).await;
        });
        Ok(())
    }

    /// 修改进程型环境的固定端口：校验区间/占用 → 落库 → 重写绑定站点 vhost →
    /// 重建容器。端口只属于环境，站点侧不持有，避免出现第二处端口来源。
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

    /// 克隆进程型环境：同版本/镜像、独立代码目录、自动分配新端口，用于并跑第二个应用。
    ///
    /// PHP 是共享 FPM 池，克隆没有「第二个进程」语义，暂不支持。
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
        let image = runtime_catalog::image_for(&src.kind, &src.version, &s.php_image);
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
            &s.runtime_home(&src.kind, &src.name),
            &s.runtime_home(&cloned.kind, &cloned.name),
        ) {
            tracing::warn!(runtime = %cloned.id, error = %e, "克隆代码目录失败，将使用自带脚手架");
        }
        self.start_runtime_install(&cloned.id, actor, role).await?;
        Ok(cloned)
    }

    /// 按需探测「应用就绪」：容器 running 只说明进程活着，不代表端口/HTTP 已能服务。
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
        match rows.iter().find(|c| c.name == container) {
            None => {
                return Err(SiteError::Unavailable(
                    "容器不存在，请先安装运行环境".into(),
                ))
            }
            Some(c) if c.state != "running" => {
                return Err(SiteError::Unavailable(format!(
                    "容器未运行（{}），应用不可用",
                    c.state
                )))
            }
            Some(_) => {}
        }
        if rt.kind == skiff_store::KIND_NODE {
            let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
            if http_probe_ok(port, &cfg.health_path).await {
                Ok(format!("应用就绪：HTTP {} 返回 2xx/3xx", cfg.health_path))
            } else {
                Err(SiteError::Unavailable(format!(
                    "容器在运行，但应用未就绪：HTTP {} 无 2xx/3xx 响应（端口 {port}）",
                    cfg.health_path
                )))
            }
        } else if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            Ok(format!("应用就绪：端口 {port} 已监听"))
        } else {
            Err(SiteError::Unavailable(format!(
                "容器在运行，但端口 {port} 未监听"
            )))
        }
    }

    async fn run_runtime_install_job(&self, runtime_id: &str, job_id: &str) {
        let result = self.install_runtime_task(runtime_id, job_id).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，运行环境可能处于中间状态，可重试安装");
                } else {
                    self.job_finish_ok(job_id, "安装完成，运行环境已就绪").await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "写入运行环境就绪状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，运行环境可能处于中间状态，可重试安装");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                // 记录失败但保留「期望运行」，这样重试入口与漂移提示仍然成立。
                if let Err(e) = self.db().set_runtime_status(runtime_id, "running", "error") {
                    tracing::warn!(runtime = runtime_id, error = %e, "写入运行环境失败状态失败");
                }
            }
        }
    }

    /// 安装主体：目录 → FPM 池 → 拉镜像 → 建/启容器 → socket → 扩展。
    async fn install_runtime_task(&self, id: &str, job_id: &str) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{id}"))?;
        let key = format!("{}:{}", rt.kind, rt.name);
        let _guard = self
            .runtime_installs
            .try_begin(&key)
            .ok_or_else(|| "该环境正在安装，请稍后再试".to_string())?;

        self.job_advance(job_id, "检查 Docker", 8, "检查 Docker 可用性")
            .await;
        if !self.docker().available() {
            return Err(format!(
                "Docker 不可用：{}",
                self.docker_unavailable_reason()
            ));
        }
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        let s = self.site_settings();
        if rt.kind == skiff_store::KIND_RUST {
            return self.install_rust_runtime_task(&rt, job_id, &s).await;
        }
        if rt.kind == skiff_store::KIND_NODE {
            return self.install_node_runtime_task(&rt, job_id, &s).await;
        }
        // PHP 扩展安装器随包分发：先落到共享 web_root，容器内 `/www/skiff/tools`
        // 直接可见；运行时不再依赖 GitHub。
        self.ensure_php_extension_installer(&s)
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let cfg = PhpRuntimeConfig::from_json(&rt.config_json);
        cfg.validate().map_err(|e| e.to_string())?;

        self.job_advance(
            job_id,
            "准备目录与配置",
            15,
            "创建实例目录并写入 PHP-FPM 池配置",
        )
        .await;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 PHP 日志目录失败：{e}"))?;
        // apk 包缓存挂载源必须存在（Docker 不会替我们建宿主机目录）。
        std::fs::create_dir_all(s.system_pkg_cache_dir())
            .map_err(|e| format!("创建依赖包缓存目录失败：{e}"))?;
        let socket_container = runtime_container_socket(&rt.kind, &rt.name);
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)?;
        self.ensure_php_account_init(&s)
            .map_err(|e| format!("准备 PHP 容器身份初始化脚本失败：{e}"))?;
        // 标记“本实例创建时已带 php.ini / conf.d 挂载”：只有安装流程写这个标记，
        // 保存配置时补目录不写，旧实例的“编辑不生效”提示就不会被误清掉。
        std::fs::write(
            s.runtime_php_dir(&rt.kind, &rt.name)
                .join(PHP_CONFIG_SENTINEL),
            "1\n",
        )
        .map_err(|e| format!("写入 PHP 配置挂载标记失败：{e}"))?;
        std::fs::write(
            s.runtime_pool_conf(&rt.kind, &rt.name),
            render_php_pool_with(&cfg, &socket_container),
        )
        .map_err(|e| format!("写入 PHP-FPM 配置失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;
        let pinned = pinned_image_ref(&image, &image_digest);
        // 统一身份依赖镜像「root 启动 + 官方入口」；不满足直接失败，不猜。
        self.verify_php_image_identity(&pinned).await?;
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            50,
            &format!("创建容器 {container}（已有同名容器会先移除）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.php_spec_for(&s, &rt.kind, &rt.name, &pinned))
            .await
            .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        let enabled = runtime_catalog::split_extensions(&rt.extensions);
        if enabled.is_empty() {
            self.job_advance(
                job_id,
                "启动 PHP-FPM",
                62,
                "收紧 socket 权限（0660 root:nobody）",
            )
            .await;
            self.secure_php_socket(&s, &rt.kind, &rt.name)
                .await
                .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        } else {
            // 有扩展时跳过安装期的第一次 socket 收紧：反正随后要重启容器，
            // 重启后统一 chmod，少等一轮 socket。
            self.job_advance(
                job_id,
                "安装扩展",
                65,
                &format!("安装 {} 个扩展：{}", enabled.len(), enabled.join("、")),
            )
            .await;
            self.run_extension_scripts(job_id, &container, &enabled, &[], 65, 92)
                .await?;
            self.job_advance(job_id, "重载 PHP-FPM", 96, "重启容器以加载扩展")
                .await;
            self.docker_runtime_action(&container, Verb::Restart)
                .await
                .map_err(|e| format!("重载 PHP-FPM 失败：{e}"))?;
            self.secure_php_socket(&s, &rt.kind, &rt.name)
                .await
                .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        }

        // 以本次安装的镜像与 socket 为准写回库；name、扩展与默认标记保持不变。
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: Some(s.runtime_socket(&rt.kind, &rt.name).display().to_string()),
                port: None,
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// Rust 环境安装：写最小 std HTTP 工程 → 拉镜像 → 容器内 `cargo run --release`
    /// → 等端口监听。日志重定向到 `/www/runtime/rust/<name>/log/server.log`。
    async fn install_rust_runtime_task(
        &self,
        rt: &Runtime,
        job_id: &str,
        s: &SiteSettings,
    ) -> Result<(), String> {
        let port = rt
            .port
            .ok_or_else(|| "Rust 环境缺少固定端口，请删除后重新创建".to_string())?;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        let app_dir = dir.join("app");
        std::fs::create_dir_all(app_dir.join("src"))
            .map_err(|e| format!("创建 Rust 工程目录失败：{e}"))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Rust 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（无第三方依赖）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的工程，重建不能覆盖。
        seed_file(
            &app_dir.join("Cargo.toml"),
            rust_cargo_manifest().as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 Cargo.toml 失败：{e}"))?;
        seed_file(
            &app_dir.join("src/main.rs"),
            rust_main_source(&rt.version, port).as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 main.rs 失败：{e}"))?;
        // 入口脚本是内核资产，每次安装/重建都刷新。
        std::fs::write(dir.join("entry.sh"), rust_entry_script())
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            50,
            &format!("创建容器 {container}（host 网络，端口 {port}）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.rust_spec_for(
            s,
            &rt.name,
            &pinned_image_ref(&image, &image_digest),
            port,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "构建并启动进程",
            70,
            &format!("cargo run --release（监听 127.0.0.1:{port}），首次编译需等待"),
        )
        .await;
        if !wait_tcp_port(port, std::time::Duration::from_secs(240)).await {
            let log =
                std::fs::read_to_string(s.runtime_log_dir(&rt.kind, &rt.name).join("server.log"))
                    .unwrap_or_default();
            let tail: Vec<&str> = log
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!(
                "端口 {port} 在 240 秒内未监听，构建或启动失败：\n{}",
                tail.join("\n")
            ));
        }
        self.job_note(job_id, &format!("端口 {port} 已就绪，Rust 进程返回内容"))
            .await;
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: None,
                port: Some(port),
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// Node.js 环境安装：写最小 HTTP 工程 → 拉镜像 → host 网络起单进程 →
    /// HTTP 健康检查。日志重定向到 `/www/runtime/node/<name>/log/server.log`。
    ///
    /// 与 PHP 的差异：没有 FPM 池/socket，也没有「多站点共享一个池」的模型；
    /// 一个环境固定一个端口、一个常驻进程、一个代码目录，绑定站点只是域名反代。
    async fn install_node_runtime_task(
        &self,
        rt: &Runtime,
        job_id: &str,
        s: &SiteSettings,
    ) -> Result<(), String> {
        let port = rt
            .port
            .ok_or_else(|| "Node.js 环境缺少固定端口，请删除后重新创建".to_string())?;
        let cfg = NodeRuntimeConfig::from_json(&rt.config_json);
        cfg.validate()
            .map_err(|e| format!("Node.js 配置不合法：{e}"))?;
        let dir = s.runtime_home(&rt.kind, &rt.name);
        let app_dir = dir.join("app");
        std::fs::create_dir_all(&app_dir).map_err(|e| format!("创建 Node 工程目录失败：{e}"))?;
        std::fs::create_dir_all(s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Node 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（npm start）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的代码，重建不能覆盖。
        seed_file(
            &app_dir.join("package.json"),
            node_package_manifest().as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 package.json 失败：{e}"))?;
        seed_file(
            &app_dir.join("server.js"),
            node_server_source(&rt.version, port).as_bytes(),
            0o644,
        )
        .map_err(|e| format!("写入 server.js 失败：{e}"))?;
        // 入口脚本是内核资产，每次安装/重建都刷新。
        write_if_changed(&dir.join("entry.sh"), node_entry_script().as_bytes(), 0o755)
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image = runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.job_advance(
            job_id,
            "创建容器",
            55,
            &format!("创建容器 {container}（host 网络，端口 {port}）"),
        )
        .await;
        if let Ok(rows) = self.docker().list_containers().await {
            if let Some(c) = rows.iter().find(|c| c.name == container) {
                self.remove_existing_container(c)
                    .await
                    .map_err(|e| format!("移除旧容器失败：{e}"))?;
            }
        }
        self.ensure_container(&self.node_spec_for(
            s,
            &rt.name,
            &pinned_image_ref(&image, &image_digest),
            port,
            &cfg,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "启动 Node 进程",
            78,
            &format!(
                "{}（监听 127.0.0.1:{port}，健康检查 {}）",
                cfg.command, cfg.health_path
            ),
        )
        .await;
        if !wait_http_ready(port, &cfg.health_path, std::time::Duration::from_secs(180)).await {
            let log =
                std::fs::read_to_string(s.runtime_log_dir(&rt.kind, &rt.name).join("server.log"))
                    .unwrap_or_default();
            let tail: Vec<&str> = log
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(format!(
                "端口 {port} 在 180 秒内未通过健康检查（{}），启动失败：\n{}",
                cfg.health_path,
                tail.join("\n")
            ));
        }
        self.job_note(
            job_id,
            &format!("端口 {port} 已就绪，Node 进程通过健康检查"),
        )
        .await;
        self.db()
            .ensure_runtime(&NewRuntime {
                id: rt.id.clone(),
                kind: rt.kind.clone(),
                name: rt.name.clone(),
                version: rt.version.clone(),
                image,
                image_digest,
                socket_path: None,
                port: Some(port),
                is_default: rt.is_default,
            })
            .map_err(|e| format!("写入运行环境失败：{e}"))?;
        Ok(())
    }

    /// 批量启用/停用扩展：安装与卸载各只开**一次** `docker exec`。
    ///
    /// 安装主路径把多个包名一次交给 mlocati 安装器；回退路径按「union 依赖 →
    /// 逐项编译 → 末尾一次清理」，并保留按扩展的分段标记。卸载把多个 ini
    /// 删除合并到一次 exec。
    async fn run_extension_scripts(
        &self,
        job_id: &str,
        container: &str,
        enabled: &[String],
        previous: &[String],
        base: u8,
        span: u8,
    ) -> Result<(), String> {
        let mut to_install: Vec<String> = Vec::new();
        for key in enabled {
            if previous.iter().any(|p| p == key) {
                continue;
            }
            // 镜像内置扩展（opcache / curl）已随 PHP 编入并默认启用，
            // 再执行 docker-php-ext-enable 会因找不到 .so 而失败（实测 exit 1）。
            if runtime_catalog::php_extension(key).is_some_and(|d| d.builtin) {
                let label = runtime_catalog::extension_label(key);
                self.job_note(job_id, &format!("{label}：镜像内置已启用，跳过"))
                    .await;
                continue;
            }
            if runtime_catalog::extension_install_script(key).is_none() {
                return Err(format!("扩展 {key} 没有可执行的安装脚本"));
            }
            to_install.push(key.clone());
        }
        if !to_install.is_empty() {
            let labels: Vec<String> = to_install
                .iter()
                .map(|k| runtime_catalog::extension_label(k))
                .collect();
            let script = runtime_catalog::batch_install_script(&to_install)
                .ok_or_else(|| "批量安装脚本生成失败".to_string())?;
            self.job_note(
                job_id,
                &format!(
                    "批量启用 {} 项：{}（单次 exec，安装器一次传多个包名）",
                    to_install.len(),
                    labels.join("、")
                ),
            )
            .await;
            self.jobs().set_cancellable(job_id, false);
            let (code, out, timed_out) = self
                .docker()
                .exec_capture_timeout(
                    container,
                    vec!["sh".into(), "-lc".into(), script],
                    self.cfg.ext_batch_timeout,
                )
                .await
                .map_err(|e| format!("批量安装扩展执行失败：{e}"))?;
            self.jobs().set_exit_code(job_id, code);
            self.job_log_output(job_id, &out).await;
            self.jobs().set_cancellable(job_id, true);
            if timed_out {
                return Err(format!(
                    "{TIMEOUT_MARK}批量安装扩展超过 {} 未结束，已按超时失败结算；\
                     Docker 无法中断 exec，容器内脚本可能仍在运行，建议重建运行环境。",
                    fmt_timeout(self.cfg.ext_batch_timeout)
                ));
            }
            if self.job_cancelled(job_id).await {
                return Err("已取消".into());
            }
            if code != 0 {
                return Err(format!(
                    "批量安装扩展失败（exit {code}）：{}（主路径与回退路径输出见任务日志）",
                    labels.join("、")
                ));
            }
            let progress = base.saturating_add(span / 2);
            self.job_advance(
                job_id,
                "安装扩展",
                progress,
                &format!("已启用 {} 项：{}", to_install.len(), labels.join("、")),
            )
            .await;
        }

        let mut to_remove: Vec<String> = Vec::new();
        for key in previous {
            if enabled.iter().any(|e| e == key) {
                continue;
            }
            // 内置扩展没有独立 .so，docker-php-ext-disable 同样会失败，直接跳过。
            if runtime_catalog::php_extension(key).is_some_and(|d| d.builtin) {
                continue;
            }
            if runtime_catalog::disable_script(key).is_none() {
                return Err(format!("扩展 {key} 不支持停用"));
            }
            to_remove.push(key.clone());
        }
        if !to_remove.is_empty() {
            let labels: Vec<String> = to_remove
                .iter()
                .map(|k| runtime_catalog::extension_label(k))
                .collect();
            let script = runtime_catalog::batch_disable_script(&to_remove)
                .ok_or_else(|| "批量停用脚本生成失败".to_string())?;
            self.job_note(
                job_id,
                &format!(
                    "批量停用 {} 项：{}（单次 exec）",
                    to_remove.len(),
                    labels.join("、")
                ),
            )
            .await;
            self.jobs().set_cancellable(job_id, false);
            let (code, out, timed_out) = self
                .docker()
                .exec_capture_timeout(
                    container,
                    vec!["sh".into(), "-lc".into(), script],
                    self.cfg.ext_batch_timeout,
                )
                .await
                .map_err(|e| format!("批量停用扩展执行失败：{e}"))?;
            self.jobs().set_exit_code(job_id, code);
            self.job_log_output(job_id, &out).await;
            self.jobs().set_cancellable(job_id, true);
            if timed_out {
                return Err(format!(
                    "{TIMEOUT_MARK}批量停用扩展超过 {} 未结束，已按超时失败结算；\
                     Docker 无法中断 exec，容器内脚本可能仍在运行，建议重建运行环境。",
                    fmt_timeout(self.cfg.ext_batch_timeout)
                ));
            }
            if self.job_cancelled(job_id).await {
                return Err("已取消".into());
            }
            if code != 0 {
                return Err(format!(
                    "批量停用扩展失败（exit {code}）：{}",
                    labels.join("、")
                ));
            }
            self.job_note(job_id, &format!("已停用 {} 项", to_remove.len()))
                .await;
        }
        Ok(())
    }

    /// 提交扩展变更（异步）。返回 `true` 表示确实启动了任务；集合未变化时返回 `false`。
    pub async fn update_runtime_extensions(
        self: &Arc<Self>,
        id: &str,
        selected: &[String],
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(
                "只有 PHP 运行环境支持扩展管理".into(),
            ));
        }
        let desired = runtime_catalog::normalize_extensions(selected);
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if desired == current {
            return Ok(false);
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.apply".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!("更新 {} 的 PHP 扩展", rt.name),
                params: serde_json::json!({"runtime_id": id, "ext_keys": desired}),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_runtime_extensions_job(&rid, &job_id, desired)
                .await;
        });
        Ok(true)
    }

    async fn run_runtime_extensions_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: Vec<String>,
    ) {
        let result = self.extensions_task(runtime_id, job_id, &desired).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_ok(job_id, "扩展已更新，PHP-FPM 已重载")
                        .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                // 扩展失败不改变容器可用性：状态回 running，错误留在任务里可重试。
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    /// 安装单个扩展（目录项或 `pecl:...`）。异步任务，立即返回；已启用返回 false。
    pub async fn install_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let token = token.trim();
        if runtime_catalog::extension_install_script(token).is_none() {
            return Err(SiteError::BadRequest(format!("不支持安装扩展：{token}")));
        }
        if let Some(def) = runtime_catalog::php_extension(token) {
            if def.builtin {
                return Err(SiteError::BadRequest(format!(
                    "{} 为镜像内置扩展，无需安装",
                    runtime_catalog::extension_label(token)
                )));
            }
            let base = runtime_catalog::base_image_of(&rt.image);
            if let Some((code, reason)) =
                runtime_catalog::extension_block_reason(def, &rt.version, base)
            {
                return Err(SiteError::BadRequest(format!("{code}: {reason}")));
            }
        }
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if current.iter().any(|k| k == token) {
            return Ok(false);
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.install".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "安装 PHP 扩展 {}（{}）",
                    runtime_catalog::extension_label(token),
                    rt.name
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": [token],
                    "action": "install",
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_single_extension_job(&rid, &job_id, &target, true)
                .await;
        });
        Ok(true)
    }

    /// 卸载单个扩展：删 conf.d 里的 ini → 重启容器 → 从库里的启用集合移除。
    pub async fn uninstall_extension(
        self: &Arc<Self>,
        id: &str,
        token: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let token = token.trim();
        if runtime_catalog::php_extension(token).is_some_and(|e| e.builtin) {
            return Err(SiteError::BadRequest(format!(
                "{} 编译进镜像，不能卸载；如需关闭请修改 PHP 配置",
                runtime_catalog::extension_label(token)
            )));
        }
        let current = runtime_catalog::split_extensions(&rt.extensions);
        if !current.iter().any(|k| k == token) {
            return Ok(false);
        }
        if runtime_catalog::disable_script(token).is_none() {
            return Err(SiteError::BadRequest(format!("不支持卸载扩展：{token}")));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: "runtime.ext.uninstall".into(),
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "卸载 PHP 扩展 {}（{}）",
                    runtime_catalog::extension_label(token),
                    rt.name
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": [token],
                    "action": "uninstall",
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let target = token.to_string();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_single_extension_job(&rid, &job_id, &target, false)
                .await;
        });
        Ok(true)
    }

    /// 按名字安装 PECL 扩展（版本可选），例如 `swoole` / `swoole 5.1.2`。
    pub async fn install_pecl_extension(
        self: &Arc<Self>,
        id: &str,
        name: &str,
        version: &str,
        actor: &str,
        role: &str,
    ) -> Result<bool, SiteError> {
        let token = runtime_catalog::pecl_token(name, version);
        if runtime_catalog::parse_pecl_token(&token).is_none() {
            return Err(SiteError::BadRequest(
                "PECL 扩展名只能包含小写字母、数字与下划线".into(),
            ));
        }
        self.install_extension(id, &token, actor, role).await
    }

    /// 批量勾选安装/卸载：一次任务内按目录顺序串行执行，日志合并，只重启一次容器。
    ///
    /// 返回真正发生变化的扩展数（全部已处于目标状态时为 0，不创建任务）。
    pub async fn apply_extension_selection(
        self: &Arc<Self>,
        id: &str,
        add: bool,
        keys: &[String],
        actor: &str,
        role: &str,
    ) -> Result<usize, SiteError> {
        let rt = self.php_runtime_for_extension(id)?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let mut changed: Vec<String> = Vec::new();
        for raw in keys {
            let token = raw.trim();
            if token.is_empty() {
                continue;
            }
            if add {
                if runtime_catalog::extension_install_script(token).is_none() {
                    return Err(SiteError::BadRequest(format!("不支持安装扩展：{token}")));
                }
                if let Some(def) = runtime_catalog::php_extension(token) {
                    if def.builtin {
                        return Err(SiteError::BadRequest(format!(
                            "{} 为镜像内置扩展，无需安装",
                            runtime_catalog::extension_label(token)
                        )));
                    }
                    let base = runtime_catalog::base_image_of(&rt.image);
                    if let Some((code, reason)) =
                        runtime_catalog::extension_block_reason(def, &rt.version, base)
                    {
                        return Err(SiteError::BadRequest(format!("{code}: {reason}")));
                    }
                }
                if !current.iter().any(|k| k == token) && !changed.iter().any(|k| k == token) {
                    changed.push(token.to_string());
                }
            } else {
                if runtime_catalog::php_extension(token).is_some_and(|e| e.builtin) {
                    return Err(SiteError::BadRequest(format!(
                        "{} 编译进镜像，不能卸载；如需关闭请修改 PHP 配置",
                        runtime_catalog::extension_label(token)
                    )));
                }
                if runtime_catalog::disable_script(token).is_none() {
                    return Err(SiteError::BadRequest(format!("不支持卸载扩展：{token}")));
                }
                if current.iter().any(|k| k == token) && !changed.iter().any(|k| k == token) {
                    changed.push(token.to_string());
                }
            }
        }
        if changed.is_empty() {
            return Ok(0);
        }
        // 目标集合：安装=当前∪选择；卸载=当前−选择。交给同一个串行任务执行。
        let mut desired = current;
        if add {
            for key in &changed {
                if !desired.iter().any(|k| k == key) {
                    desired.push(key.clone());
                }
            }
            desired = runtime_catalog::normalize_extensions(&desired);
        } else {
            desired.retain(|k| !changed.iter().any(|c| c == k));
        }
        let job = self
            .jobs()
            .submit(app_jobs::JobSubmit {
                kind: if add {
                    "runtime.ext.install".into()
                } else {
                    "runtime.ext.uninstall".into()
                },
                target_type: app_jobs::TARGET_RUNTIME.into(),
                target_id: id.to_string(),
                title: format!(
                    "{} {} 的 {} 项 PHP 扩展",
                    if add { "批量安装" } else { "批量卸载" },
                    rt.name,
                    changed.len()
                ),
                params: serde_json::json!({
                    "runtime_id": id,
                    "ext_keys": changed,
                    "action": if add { "install" } else { "uninstall" },
                }),
                created_by: actor.to_string(),
                created_by_role: role.to_string(),
                requester_ip: String::new(),
                cancellable: true,
            })
            .map_err(site_error_from_job)?;
        let state = self.clone();
        let rid = id.to_string();
        let targets = changed.clone();
        let job_id = job.id;
        tokio::spawn(async move {
            state
                .run_batch_extensions_job(&rid, &job_id, &desired, &targets)
                .await;
        });
        Ok(changed.len())
    }

    /// 重试历史任务：与正常提交共用同一组执行器，避免「只插行不执行」留下 queued 僵尸。
    ///
    /// - 只有当前代码仍有执行器的类型可重试；其余直接拒绝，**不插入新行**。
    /// - 同目标已有活跃任务时，`JobService::retry` 经唯一索引返回 Busy（明确拒绝并给原因），
    ///   不做排队；取消旧任务后即可再次提交。
    pub async fn retry_job(
        self: &Arc<Self>,
        id: &str,
        actor: &str,
        role: &str,
        ip: &str,
    ) -> Result<app_jobs::Job, SiteError> {
        let old = self
            .jobs()
            .get(id)
            .ok_or_else(|| SiteError::BadRequest(format!("任务不存在：{id}")))?;
        let runtime_id = old
            .params
            .get("runtime_id")
            .and_then(|v| v.as_str())
            .unwrap_or(old.target_id.as_str())
            .to_string();
        let ext_keys: Vec<String> = old
            .params
            .get("ext_keys")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        // 先算好执行计划：未知类型或运行环境已不存在时不插入重试行。
        let plan = match old.kind.as_str() {
            "runtime.install" => RetryPlan::Install,
            "web.install" => RetryPlan::WebInstall,
            "runtime.ext.apply" => RetryPlan::Apply(ext_keys),
            "runtime.ext.install" | "runtime.ext.uninstall" => {
                if self
                    .db()
                    .runtime_by_id(&runtime_id)
                    .map_err(|e| SiteError::Internal(e.to_string()))?
                    .is_none()
                {
                    return Err(SiteError::BadRequest(format!(
                        "运行环境不存在：{runtime_id}"
                    )));
                }
                let add = old
                    .params
                    .get("action")
                    .and_then(|v| v.as_str())
                    .map(|a| a == "install")
                    .unwrap_or(old.kind.ends_with(".install"));
                if ext_keys.is_empty() {
                    return Err(SiteError::BadRequest(
                        "原任务没有记录扩展集合，无法重试".into(),
                    ));
                }
                if ext_keys.len() == 1 {
                    // 单个扩展与正常提交一致，走单扩展执行器。
                    RetryPlan::Single {
                        token: ext_keys[0].clone(),
                        add,
                    }
                } else {
                    let rt = self
                        .db()
                        .runtime_by_id(&runtime_id)
                        .map_err(|e| SiteError::Internal(e.to_string()))?
                        .ok_or_else(|| {
                            SiteError::BadRequest(format!("运行环境不存在：{runtime_id}"))
                        })?;
                    // 目标集合与正常提交一致：安装=当前∪选择；卸载=当前−选择。
                    let mut desired = runtime_catalog::split_extensions(&rt.extensions);
                    if add {
                        for key in &ext_keys {
                            if !desired.iter().any(|k| k == key) {
                                desired.push(key.clone());
                            }
                        }
                        desired = runtime_catalog::normalize_extensions(&desired);
                    } else {
                        desired.retain(|k| !ext_keys.iter().any(|t| t == k));
                    }
                    RetryPlan::Batch {
                        desired,
                        targets: ext_keys,
                    }
                }
            }
            other => {
                return Err(SiteError::BadRequest(format!(
                    "任务类型 {other} 不支持重试（当前没有对应执行器）"
                )))
            }
        };

        let job = self
            .jobs()
            .retry(id, actor, role, ip)
            .map_err(site_error_from_job)?;
        let job_id = job.id.clone();
        let state = self.clone();
        match plan {
            RetryPlan::Install => {
                if let Err(e) = self
                    .db()
                    .set_runtime_status(&runtime_id, "running", "installing")
                {
                    tracing::warn!(runtime = %runtime_id, error = %e, "重试时写入运行环境状态失败");
                }
                tokio::spawn(async move {
                    state.run_runtime_install_job(&runtime_id, &job_id).await;
                });
            }
            RetryPlan::WebInstall => {
                tokio::spawn(async move {
                    state.run_web_env_install_job(&job_id).await;
                });
            }
            RetryPlan::Single { token, add } => {
                tokio::spawn(async move {
                    state
                        .run_single_extension_job(&runtime_id, &job_id, &token, add)
                        .await;
                });
            }
            RetryPlan::Apply(desired) => {
                tokio::spawn(async move {
                    state
                        .run_runtime_extensions_job(&runtime_id, &job_id, desired)
                        .await;
                });
            }
            RetryPlan::Batch { desired, targets } => {
                tokio::spawn(async move {
                    state
                        .run_batch_extensions_job(&runtime_id, &job_id, &desired, &targets)
                        .await;
                });
            }
        }
        Ok(job)
    }

    async fn run_batch_extensions_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: &[String],
        targets: &[String],
    ) {
        let result = self.extensions_task(runtime_id, job_id, desired).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_ok(
                        job_id,
                        &format!(
                            "已应用 {} 项扩展变更（{}）；PHP-FPM 已重载",
                            targets.len(),
                            targets.join("、")
                        ),
                    )
                    .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展集合可能部分生效，可重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    fn php_runtime_for_extension(&self, id: &str) -> Result<Runtime, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(
                "只有 PHP 运行环境支持扩展管理".into(),
            ));
        }
        Ok(rt)
    }

    async fn run_single_extension_job(
        &self,
        runtime_id: &str,
        job_id: &str,
        token: &str,
        add: bool,
    ) {
        let result = self
            .single_extension_task(runtime_id, job_id, token, add)
            .await;
        let label = runtime_catalog::extension_label(token);
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展状态可能未生效，请核对后重试");
                } else {
                    self.job_finish_ok(
                        job_id,
                        &format!(
                            "扩展 {label} {}，PHP-FPM 已重载",
                            if add { "已启用" } else { "已停用" }
                        ),
                    )
                    .await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
            Err(err) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，扩展状态可能未生效，请核对后重试");
                } else {
                    self.job_finish_err(job_id, &err).await;
                }
                if let Err(e) = self
                    .db()
                    .set_runtime_status(runtime_id, "running", "running")
                {
                    tracing::warn!(runtime = runtime_id, error = %e, "回写运行环境状态失败");
                }
            }
        }
    }

    async fn single_extension_task(
        &self,
        runtime_id: &str,
        job_id: &str,
        token: &str,
        add: bool,
    ) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(runtime_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{runtime_id}"))?;
        let label = runtime_catalog::extension_label(token);
        self.ensure_php_extension_installer(&self.site_settings())
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let container = runtime_container_name(&rt.kind, &rt.name);
        let exists = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| format!("读取容器列表失败：{e}"))?
            .iter()
            .any(|c| c.name == container && c.state == "running");
        if !exists {
            return Err(format!(
                "PHP 容器未运行，无法热装扩展；请先安装/启动运行环境（{container}）"
            ));
        }
        let script = if add {
            runtime_catalog::extension_install_script(token)
                .ok_or_else(|| format!("扩展 {label} 没有可执行的安装脚本"))?
        } else {
            runtime_catalog::disable_script(token)
                .ok_or_else(|| format!("扩展 {label} 不支持卸载"))?
        };
        self.job_advance_blocking(
            job_id,
            if add { "安装扩展" } else { "卸载扩展" },
            15,
            &format!("{label}：{script}"),
        )
        .await;
        let (code, out, timed_out) = self
            .docker()
            .exec_capture_timeout(
                &container,
                vec!["sh".into(), "-lc".into(), script],
                self.cfg.ext_exec_timeout,
            )
            .await
            .map_err(|e| format!("{label} 执行失败：{e}"))?;
        self.jobs().set_exit_code(job_id, code);
        self.job_log_output(job_id, &out).await;
        // 脚本结束（无论成败）后重新允许取消。
        self.jobs().set_cancellable(job_id, true);
        if timed_out {
            return Err(format!(
                "{TIMEOUT_MARK}扩展 {label} 执行超过 {} 未结束，已按超时失败结算；\
                 Docker 无法中断 exec，容器内脚本可能仍在运行，建议重载或重建运行环境。\
                 已采集输出尾部见任务日志。",
                fmt_timeout(self.cfg.ext_exec_timeout)
            ));
        }
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        if code != 0 {
            // 必须回传真实报错（pecl 常见缺依赖/版本不存在），不能只报「安装失败」。
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            let start = lines.len().saturating_sub(20);
            let err_code = if add {
                runtime_catalog::pecl_failure_code(&out)
            } else {
                None
            };
            let prefix = match err_code {
                Some(c) => format!(
                    "{c}：PHP {} 下扩展 {label} 可能尚未就绪（PECL 滞后于新版 PHP）；可稍后重试或指定兼容版本。\n",
                    rt.version
                ),
                None => String::new(),
            };
            return Err(format!(
                "{prefix}扩展 {label} {}失败（exit {code}）：\n{}",
                if add { "安装" } else { "卸载" },
                lines[start..].join("\n")
            ));
        }
        self.job_advance(job_id, "重载 PHP-FPM", 80, "重启容器以加载扩展变更")
            .await;
        self.docker_runtime_action(&container, Verb::Restart)
            .await
            .map_err(|e| format!("重启 PHP-FPM 失败：{e}"))?;
        let s = self.site_settings();
        self.secure_php_socket(&s, &rt.kind, &rt.name)
            .await
            .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        let mut next = current;
        if add {
            next.push(token.to_string());
        } else {
            next.retain(|k| k != token);
        }
        let joined = runtime_catalog::join_extensions(&next);
        self.db()
            .set_runtime_extensions(runtime_id, &joined)
            .map_err(|e| format!("写入扩展集合失败：{e}"))?;
        Ok(())
    }

    async fn extensions_task(
        &self,
        runtime_id: &str,
        job_id: &str,
        desired: &[String],
    ) -> Result<(), String> {
        let rt = self
            .db()
            .runtime_by_id(runtime_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("运行环境不存在：{runtime_id}"))?;
        self.ensure_php_extension_installer(&self.site_settings())
            .map_err(|e| format!("准备本地扩展安装器失败：{e}"))?;
        let current = runtime_catalog::split_extensions(&rt.extensions);
        let container = runtime_container_name(&rt.kind, &rt.name);
        let exists = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| format!("读取容器列表失败：{e}"))?
            .iter()
            .any(|c| c.name == container);

        // 容器不存在时，先把期望集合落库，再走完整安装（安装阶段会重放扩展）。
        if !exists {
            self.job_advance(job_id, "准备重建", 10, "容器不存在，改为完整安装并重放扩展")
                .await;
            let joined = runtime_catalog::join_extensions(desired);
            self.db()
                .set_runtime_extensions(runtime_id, &joined)
                .map_err(|e| format!("写入扩展集合失败：{e}"))?;
            // 这里不预先抢安装锁：`install_runtime_task` 自己会 `try_begin`，
            // 先抢再立刻丢弃的守卫既没有互斥效果，还会让嵌套获取误判「正在安装」。
            return self.install_runtime_task(runtime_id, job_id).await;
        }

        self.job_advance(job_id, "应用扩展变更", 20, "对比扩展集合，执行启用/停用")
            .await;
        self.run_extension_scripts(job_id, &container, desired, &current, 20, 60)
            .await?;
        self.job_advance(job_id, "重载 PHP-FPM", 85, "重启容器以加载扩展变更")
            .await;
        if self.job_cancelled(job_id).await {
            return Err("已取消".into());
        }
        self.docker_runtime_action(&container, Verb::Restart)
            .await
            .map_err(|e| format!("重启 PHP-FPM 失败：{e}"))?;
        let s = self.site_settings();
        self.secure_php_socket(&s, &rt.kind, &rt.name)
            .await
            .map_err(|e| format!("收紧 PHP socket 失败：{e}"))?;
        let joined = runtime_catalog::join_extensions(desired);
        self.db()
            .set_runtime_extensions(runtime_id, &joined)
            .map_err(|e| format!("写入扩展集合失败：{e}"))?;
        Ok(())
    }

    /// 新建运行环境（类型 + 版本目录 + 可选扩展）并异步安装。
    ///
    /// 同步部分只做校验与落库（名称唯一、版本在目录内、目录不覆盖）；安装本身
    /// 交给后台任务。Node.js/Go 在 P1 只登记不安装，返回 `false`。
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
        if !runtime_catalog::version_supported(kind, version) {
            return Err(SiteError::BadRequest(format!(
                "{} 版本无效：请从版本下拉中选择（可选：{}）",
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
        let image = runtime_catalog::image_for(kind, version, &s.php_image);
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

    /// 保存 PHP 专属配置：校验 → 写池配置 → reload FPM → 落库；任一步失败回滚文件。
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
        std::fs::create_dir_all(s.runtime_home(&rt.kind, &rt.name))
            .map_err(|e| SiteError::Internal(format!("创建运行目录失败：{e}")))?;
        let pool = s.runtime_pool_conf(&rt.kind, &rt.name);
        let original = std::fs::read(&pool).ok();
        // 用户可见写操作统一走 skiff-fs：原子替换 + 审计，不在 app 层直接 std::fs::write。
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
                "配置已回滚：重载 PHP-FPM 失败：{e}"
            )));
        }
        self.db()
            .set_runtime_config(id, &cfg.to_json())
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        self.runtime_view(id).await
    }

    /// 保存 Node.js 专属配置。启动命令/环境变量在容器的 `command` 与 `Env` 里，
    /// 只能重建容器才生效，所以这里只落库并返回「是否有变化」，由调用方决定是否
    /// 触发重建任务（与 PHP 写池配置后热重启不同）。
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

    /// 实例创建时是否已带 `php.ini` / `conf.d` 挂载（决定配置编辑是否真生效）。
    ///
    /// 旧实例（早于配置文件编辑功能）容器里没有这两个挂载，编辑能保存但不生效；
    /// 详情页据此提示“重建后生效”，避免用户对着能编辑的界面猜。
    pub fn php_config_mounts_current(&self, id: &str) -> Result<bool, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        Ok(s.runtime_php_dir(&rt.kind, &rt.name)
            .join(PHP_CONFIG_SENTINEL)
            .exists())
    }

    /// 列出 PHP 实例可直接查看/编辑的配置文件。
    ///
    /// 当前实际存在三类：`php.ini`（宿主新建，挂到容器主配置）、FPM 池 `www.conf`
    /// （宿主生成，覆盖镜像默认池）、`conf.d/*.ini`（宿主托管扩展 ini，扫在镜像
    /// `conf.d` 之后）。镜像里由扩展安装器写入的 ini 不在宿主侧，不在此列。
    pub fn php_config_files(&self, id: &str) -> Result<Vec<PhpConfigFile>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let mut files = vec![
            PhpConfigFile {
                key: "php.ini".into(),
                label: "php.ini".into(),
                host_path: s.runtime_php_ini(&rt.kind, &rt.name).display().to_string(),
                container_path: CONTAINER_PHP_INI.into(),
                exists: s.runtime_php_ini(&rt.kind, &rt.name).exists(),
                editable: true,
                note:
                    "PHP 主配置；页面「PHP 参数」中的 memory_limit 等四项写在 FPM 池，优先级更高。"
                        .into(),
                warnings: Vec::new(),
            },
            PhpConfigFile {
                key: "www.conf".into(),
                label: "www.conf（FPM 池）".into(),
                host_path: s
                    .runtime_pool_conf(&rt.kind, &rt.name)
                    .display()
                    .to_string(),
                container_path: CONTAINER_PHP_POOL.into(),
                exists: s.runtime_pool_conf(&rt.kind, &rt.name).exists(),
                editable: true,
                note: "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。"
                    .into(),
                warnings: Vec::new(),
            },
        ];
        let confd = s.runtime_php_confd(&rt.kind, &rt.name);
        if let Ok(entries) = std::fs::read_dir(&confd) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".ini") {
                    continue;
                }
                files.push(PhpConfigFile {
                    key: format!("conf.d/{name}"),
                    label: format!("扩展 {name}"),
                    host_path: entry.path().display().to_string(),
                    container_path: format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    exists: true,
                    editable: true,
                    note: "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。".into(),
                    warnings: Vec::new(),
                });
            }
        }
        Ok(files)
    }

    /// 读一个可用编辑器打开的配置文件。
    pub fn read_php_config_file(
        &self,
        id: &str,
        key: &str,
        actor: &str,
    ) -> Result<PhpConfigFileView, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        let content = if path.exists() {
            self.admin_fs()
                .read(actor, &path.display().to_string())
                .map_err(|e| SiteError::Internal(format!("读取 {key} 失败：{e}")))?
                .text
        } else {
            String::new()
        };
        Ok(PhpConfigFileView {
            file: meta,
            content,
        })
    }

    /// 保存配置文件：先写候选 → 容器内语法校验 → 原子落盘 → 重启 FPM；失败回滚。
    pub async fn save_php_config_file(
        &self,
        id: &str,
        key: &str,
        content: &str,
        actor: &str,
    ) -> Result<PhpConfigFile, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        // 旧实例可能还没有宿主托管的 php/ 目录：先补齐，否则候选文件无处落盘。
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)
            .map_err(|e| SiteError::Internal(format!("准备 PHP 配置目录失败：{e}")))?;
        if content.len() > 256 * 1024 {
            return Err(SiteError::BadRequest(
                "配置文件超过 256 KiB 编辑上限".into(),
            ));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        let running = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器状态失败：{e}")))?
            .iter()
            .any(|c| c.name == container && c.state == "running");
        if !running {
            return Err(SiteError::Unavailable(
                "运行环境未在运行，无法在容器内校验配置；请先启动实例".into(),
            ));
        }
        // 候选文件放在 web_root 内，容器内可直接读到同一个文件。
        let candidate = s
            .runtime_php_dir(&rt.kind, &rt.name)
            .join(format!("{key}.candidate"));
        self.admin_fs()
            .write(actor, &candidate.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入候选文件失败：{e}")))?;
        let candidate_container = s.container_path(&candidate);
        let (code, out) = match key {
            "www.conf" => {
                // php-fpm -t 只接受主配置；用临时主配置 include 候选池文件。
                let main = s
                    .runtime_php_dir(&rt.kind, &rt.name)
                    .join("php-fpm-test.conf");
                let main_text = format!(
                    "[global]\n\
                     error_log = /dev/stderr\n\
                     include = {candidate_container}\n"
                );
                self.admin_fs()
                    .write(actor, &main.display().to_string(), &main_text)
                    .map_err(|e| SiteError::Internal(format!("写入校验主配置失败：{e}")))?;
                let main_container = s.container_path(&main);
                self.docker()
                    .exec_capture(
                        &container,
                        vec![
                            "/usr/local/sbin/php-fpm".into(),
                            "-t".into(),
                            "-y".into(),
                            main_container,
                        ],
                    )
                    .await
                    .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?
            }
            _ => self
                .docker()
                .exec_capture(
                    &container,
                    vec![
                        "php".into(),
                        "-c".into(),
                        candidate_container,
                        "-r".into(),
                        "echo \"skiff-ok\";".into(),
                    ],
                )
                .await
                .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?,
        };
        // 只拦语法错误与启动失败：退出码非 0，或（非池配置）没跑进 `-r` 就退出，
        // 或输出里出现 `syntax error`。最后一条不能省：PHP 的 ini 语法错误只打印到
        // stdout（`PHP:  syntax error, unexpected ...`），进程仍会执行 `-r` 并返回 0，
        // 只看退出码/`skiff-ok` 会把坏配置放过去（上机 FS 验收复现）。
        // `PHP Warning` 不阻止保存，但要收进 warnings 原文回显给用户。
        let invalid = code != 0
            || php_output_has_syntax_error(&out)
            || (key != "www.conf" && !out.contains("skiff-ok"));
        if invalid {
            let reason = out.trim();
            return Err(SiteError::BadRequest(if reason.is_empty() {
                "配置语法校验失败，已拒绝保存".into()
            } else {
                format!("配置语法校验失败，已拒绝保存：{reason}")
            }));
        }
        let warnings = php_validation_warnings(&out);
        let original = std::fs::read(&path).ok();
        self.admin_fs()
            .write(actor, &path.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入 {key} 失败：{e}")))?;
        // 重启后必须复核容器仍在 running：Docker restart 返回成功不代表 FPM
        // 没被坏配置带崩（与 nginx 主配置保存同一条静默路径）。
        let problem = match self.docker_runtime_action(&container, Verb::Restart).await {
            Err(e) => Some(format!("{e}")),
            Ok(()) => {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                match self.docker().list_containers().await {
                    Ok(rows)
                        if rows
                            .iter()
                            .any(|c| c.name == container && c.state == "running") =>
                    {
                        None
                    }
                    Ok(_) => {
                        Some("容器重启后未保持 running（配置可能导致 FPM 启动失败）".to_string())
                    }
                    Err(e) => Some(format!("重启后核对容器状态失败：{e}")),
                }
            }
        };
        if let Some(reason) = problem {
            match &original {
                Some(bytes) if !bytes.is_empty() => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        if let Err(we) =
                            self.admin_fs()
                                .write(actor, &path.display().to_string(), text)
                        {
                            tracing::error!(path = %path.display(), error = %we, "回滚站点配置写入失败");
                        }
                    }
                }
                _ => {
                    if let Err(e) = self.admin_fs().remove(actor, &path.display().to_string()) {
                        tracing::debug!(path = %path.display(), error = %e, "回滚新增的 PHP 配置文件失败");
                    }
                }
            }
            if let Err(e) = self.docker_runtime_action(&container, Verb::Restart).await {
                tracing::error!(error = %e, runtime = %container, "回滚 PHP 配置后重启运行环境失败");
            }
            return Err(SiteError::Unavailable(format!(
                "保存后重启 PHP-FPM 失败，已回滚 {key}：{reason}"
            )));
        }
        Ok(PhpConfigFile {
            exists: true,
            warnings,
            ..meta
        })
    }

    /// 读取容器内 `php -i` 的实际生效值，并与宿主 `php.ini` 文件值并排返回。
    ///
    /// 只在用户点「刷新生效值」时调用，不在页面渲染路径执行容器命令。
    pub async fn php_effective_values(&self, id: &str) -> Result<Vec<PhpIniValue>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let container = runtime_container_name(&rt.kind, &rt.name);
        let (code, out) = self
            .docker()
            .exec_capture(&container, vec!["php".into(), "-i".into()])
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取 php -i 失败：{e}")))?;
        if code != 0 {
            return Err(SiteError::Unavailable(format!(
                "php -i 退出码 {code}，无法读取生效值"
            )));
        }
        let effective = parse_php_i(&out);
        let file_text =
            std::fs::read_to_string(s.runtime_php_ini(&rt.kind, &rt.name)).unwrap_or_default();
        let file_values = parse_ini_assignments(&file_text);
        let managed = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
        ];
        let keys = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
            "display_errors",
            "error_reporting",
            "date.timezone",
            "opcache.enable",
        ];
        Ok(keys
            .iter()
            .map(|key| {
                let eff = effective
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（未读取）".into());
                let file = file_values
                    .iter()
                    .rev()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（文件未设置）".into());
                let source = if managed.contains(key) && file != "（文件未设置）" {
                    "FPM 池 php_admin_value 优先；文件值仅在其他 SAPI 生效".to_string()
                } else if file != "（文件未设置）" && file == eff {
                    "php.ini".to_string()
                } else if file != "（文件未设置）" {
                    "其他 conf.d / 镜像默认覆盖".to_string()
                } else {
                    "镜像默认或其他 conf.d".to_string()
                };
                PhpIniValue {
                    key: key.to_string(),
                    file_value: file,
                    effective: eff,
                    source,
                }
            })
            .collect())
    }

    fn php_runtime(&self, id: &str) -> Result<Runtime, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境没有 PHP 专属配置",
                rt.kind
            )));
        }
        Ok(rt)
    }

    /// 把配置 key 解析成宿主路径；只允许固定两项与 `conf.d/<name>.ini` 单层文件名。
    fn php_config_target(
        &self,
        rt: &Runtime,
        s: &SiteSettings,
        key: &str,
    ) -> Result<(PathBuf, PhpConfigFile), SiteError> {
        let (path, container_path, label, note) = match key {
            "php.ini" => (
                s.runtime_php_ini(&rt.kind, &rt.name),
                CONTAINER_PHP_INI.to_string(),
                "php.ini".to_string(),
                "PHP 主配置；页面「PHP 参数」中的四项写在 FPM 池，优先级更高。",
            ),
            "www.conf" => (
                s.runtime_pool_conf(&rt.kind, &rt.name),
                CONTAINER_PHP_POOL.to_string(),
                "www.conf（FPM 池）".to_string(),
                "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。",
            ),
            other => {
                let name = other
                    .strip_prefix("conf.d/")
                    .ok_or_else(|| SiteError::BadRequest(format!("未知配置文件：{other}")))?;
                let p = Path::new(name);
                if name.is_empty()
                    || p.components().count() != 1
                    || !name.ends_with(".ini")
                    || name.contains('\\')
                {
                    return Err(SiteError::BadRequest("扩展 ini 文件名不合法".into()));
                }
                (
                    s.runtime_php_confd(&rt.kind, &rt.name).join(name),
                    format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    format!("扩展 {name}"),
                    "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。",
                )
            }
        };
        Ok((
            path.clone(),
            PhpConfigFile {
                key: key.to_string(),
                label,
                host_path: path.display().to_string(),
                container_path,
                exists: path.exists(),
                editable: true,
                note: note.to_string(),
                warnings: Vec::new(),
            },
        ))
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
            std::fs::rename(&vhost, &disabled)
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

    /// 删除运行环境：有站点绑定时拒绝并列出域名，绝不隐式解绑/级联删除。
    ///
    /// 真正清理的顺序：容器 stop → remove → 运行目录（含 FPM 池配置、socket、
    /// 日志与 Rust/Node 工程）→ DB 行。任一步失败就停下并保留 DB 行，错误里列出
    /// 已完成项，避免「记录删了、容器和目录还在」的静默残留。
    /// 删除不区分 kind：Go 只是未实现安装，已登记的运行环境仍可删除。
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
            remove_dir_all_retry(&dir).map_err(|e| {
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

    /// 运行环境日志：PHP-FPM 走容器日志（stdout/stderr）；Rust 读宿主日志文件。
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

    /// 用户主动点击「实测版本」时，才进容器执行一次；2 秒超时。
    /// 结果只代表当前运行中的容器，不写缓存、不参与列表渲染。
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
