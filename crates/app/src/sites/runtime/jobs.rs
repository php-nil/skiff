//! 运行环境安装执行体、任务中心适配与扩展安装器。
//!
//! 属于 sites::runtime 子模块；与 mod.rs / php.rs 共同组成 impl AppState。
use std::sync::Arc;

use skiff_docker::Verb;
use skiff_store::{NewRuntime, Runtime};

use super::*;
use crate::jobs as app_jobs;
use crate::sites::runtime_catalog;
use crate::AppState;

impl AppState {
    pub async fn runtime_job(&self, id: &str) -> Option<RuntimeJob> {
        let job = self
            .jobs()
            .latest_for_target(app_jobs::TARGET_RUNTIME, id)?;
        Some(self.runtime_job_view(&job, 300))
    }

    pub async fn runtime_jobs_snapshot(&self) -> Vec<RuntimeJob> {
        self.jobs()
            .list_for_target_type(app_jobs::TARGET_RUNTIME, 200)
            .iter()
            .map(|j| self.runtime_job_view(j, 60))
            .collect()
    }

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

    pub(crate) async fn job_advance(&self, job_id: &str, step: &str, progress: u8, line: &str) {
        self.job_advance_with(job_id, step, progress, line, true)
            .await;
    }

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

    async fn job_cancelled(&self, job_id: &str) -> bool {
        self.jobs().cancel_requested(job_id)
    }

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

    async fn run_runtime_install_job(&self, runtime_id: &str, job_id: &str) {
        let result = self.install_runtime_task(runtime_id, job_id).await;
        match result {
            Ok(()) => {
                if self.job_cancelled(job_id).await {
                    self.jobs()
                        .finish_cancelled(job_id, "已取消，运行环境可能处于中间状态，可重试安装");
                } else {
                    self.job_finish_ok(job_id, "安装完成，运行环境已就绪").await;
                    // 刚验证过就绪：清掉上轮巡检留下的不可用结论，站点立即按正常配置渲染。
                    self.set_runtime_guard(runtime_id, RuntimeGuard::ok());
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
        let kfs = self.admin_fs();
        kfs.ensure_dir_kernel(&dir)
            .map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
        kfs.ensure_dir_kernel(&s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 PHP 日志目录失败：{e}"))?;
        // apk 包缓存挂载源必须存在（Docker 不会替我们建宿主机目录）。
        kfs.ensure_dir_kernel(&s.system_pkg_cache_dir())
            .map_err(|e| format!("创建依赖包缓存目录失败：{e}"))?;
        let socket_container = runtime_container_socket(&rt.kind, &rt.name);
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)?;
        self.ensure_php_account_init(&s)
            .map_err(|e| format!("准备 PHP 容器身份初始化脚本失败：{e}"))?;
        // 标记“本实例创建时已带 php.ini / conf.d 挂载”：只有安装流程写这个标记，
        // 保存配置时补目录不写，旧实例的“编辑不生效”提示就不会被误清掉。
        kfs.write_kernel(
            &s.runtime_php_dir(&rt.kind, &rt.name)
                .join(PHP_CONFIG_SENTINEL),
            b"1\n",
        )
        .map_err(|e| format!("写入 PHP 配置挂载标记失败：{e}"))?;
        kfs.write_kernel(
            &s.runtime_pool_conf(&rt.kind, &rt.name),
            render_php_pool_with(&cfg, &socket_container).as_bytes(),
        )
        .map_err(|e| format!("写入 PHP-FPM 配置失败：{e}"))?;

        let image =
            runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image, &s.image_prefix);
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
        self.ensure_site_network()
            .await
            .map_err(|e| format!("准备站点网络失败：{e}"))?;
        let ip = self
            .ensure_runtime_ip(&rt)
            .await
            .map_err(|e| format!("分配容器地址失败：{e}"))?;
        self.ensure_container(&self.php_spec_for(&s, &rt.kind, &rt.name, &pinned, &ip))
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
            self.job_advance(job_id, "重启 PHP 容器", 96, "重启容器以加载扩展")
                .await;
            self.docker_runtime_action(&container, Verb::Restart)
                .await
                .map_err(|e| format!("重启 PHP 容器失败：{e}"))?;
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
        let fs = self.admin_fs();
        fs.ensure_dir_kernel(&app_dir.join("src"))
            .map_err(|e| format!("创建 Rust 工程目录失败：{e}"))?;
        fs.ensure_dir_kernel(&s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Rust 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（无第三方依赖）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的工程，重建不能覆盖。
        let cargo = app_dir.join("Cargo.toml");
        if !cargo.exists() {
            fs.write_kernel(&cargo, rust_cargo_manifest().as_bytes())
                .map_err(|e| format!("写入 Cargo.toml 失败：{e}"))?;
        }
        let main_rs = app_dir.join("src/main.rs");
        if !main_rs.exists() {
            fs.write_kernel(&main_rs, rust_main_source(&rt.version, port).as_bytes())
                .map_err(|e| format!("写入 main.rs 失败：{e}"))?;
        }
        // 入口脚本是内核资产，每次安装/重建都刷新。
        fs.write_kernel(&dir.join("entry.sh"), rust_entry_script().as_bytes())
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image =
            runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image, &s.image_prefix);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.ensure_site_network()
            .await
            .map_err(|e| format!("准备站点网络失败：{e}"))?;
        let ip = self
            .ensure_runtime_ip(rt)
            .await
            .map_err(|e| format!("分配容器地址失败：{e}"))?;
        self.job_advance(
            job_id,
            "创建容器",
            50,
            &format!("创建容器 {container}（专用网络，地址 {ip}，端口 {port}）"),
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
            &ip,
            rt.publish_port,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "构建并启动进程",
            70,
            &format!("cargo run --release（监听容器内 0.0.0.0:{port}），首次编译需等待"),
        )
        .await;
        if let Err(failure) = wait_runtime_ready(
            self.docker(),
            &container,
            &ip,
            port,
            None,
            std::time::Duration::from_secs(240),
        )
        .await
        {
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
                "{}；\n{}",
                readiness_message("Rust", &ip, port, None, &failure),
                tail.join("\n")
            ));
        }
        self.job_note(job_id, &format!("端口 {port} 已就绪，Rust 进程返回内容"))
            .await;
        self.set_runtime_guard(&rt.id, RuntimeGuard::ok());
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
        let fs = self.admin_fs();
        fs.ensure_dir_kernel(&app_dir)
            .map_err(|e| format!("创建 Node 工程目录失败：{e}"))?;
        fs.ensure_dir_kernel(&s.runtime_log_dir(&rt.kind, &rt.name))
            .map_err(|e| format!("创建 Node 日志目录失败：{e}"))?;
        self.job_advance(job_id, "准备工程", 15, "写入最小 HTTP 工程（npm start）")
            .await;
        // 只在缺失时生成脚手架：`app/` 可能已经是用户自己的代码，重建不能覆盖。
        let package_json = app_dir.join("package.json");
        if !package_json.exists() {
            fs.write_kernel(&package_json, node_package_manifest().as_bytes())
                .map_err(|e| format!("写入 package.json 失败：{e}"))?;
        }
        let server_js = app_dir.join("server.js");
        if !server_js.exists() {
            fs.write_kernel(&server_js, node_server_source(&rt.version, port).as_bytes())
                .map_err(|e| format!("写入 server.js 失败：{e}"))?;
        }
        // 入口脚本是内核资产，每次安装/重建都刷新。
        fs.write_kernel(&dir.join("entry.sh"), node_entry_script().as_bytes())
            .map_err(|e| format!("写入入口脚本失败：{e}"))?;

        let image =
            runtime_catalog::image_for(&rt.kind, &rt.version, &s.php_image, &s.image_prefix);
        let image_digest = self
            .ensure_runtime_image(job_id, &image, &rt.image_digest)
            .await?;

        let container = runtime_container_name(&rt.kind, &rt.name);
        self.ensure_site_network()
            .await
            .map_err(|e| format!("准备站点网络失败：{e}"))?;
        let ip = self
            .ensure_runtime_ip(rt)
            .await
            .map_err(|e| format!("分配容器地址失败：{e}"))?;
        self.job_advance(
            job_id,
            "创建容器",
            55,
            &format!("创建容器 {container}（专用网络，地址 {ip}，端口 {port}）"),
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
            &ip,
            rt.publish_port,
            &cfg,
        ))
        .await
        .map_err(|e| format!("创建/启动 {container} 失败：{e}"))?;

        self.job_advance(
            job_id,
            "启动 Node 进程",
            78,
            &format!(
                "{}（监听容器内 0.0.0.0:{port}，健康检查 {}）",
                cfg.command, cfg.health_path
            ),
        )
        .await;
        if let Err(failure) = wait_runtime_ready(
            self.docker(),
            &container,
            &ip,
            port,
            Some(&cfg.health_path),
            std::time::Duration::from_secs(180),
        )
        .await
        {
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
                "{}；\n{}",
                readiness_message("Node.js", &ip, port, Some(&cfg.health_path), &failure),
                tail.join("\n")
            ));
        }
        self.job_note(
            job_id,
            &format!("端口 {port} 已就绪，Node 进程通过健康检查"),
        )
        .await;
        self.set_runtime_guard(&rt.id, RuntimeGuard::ok());
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
                     Docker 无法中断 exec，容器内脚本可能仍在运行，重建运行环境后重试即可：\
                     已下载的系统包缓存在宿主 `cache/apk`，重试会跳过已完成部分（通常几分钟内装完）。",
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
                 Docker 无法中断 exec，容器内脚本可能仍在运行，重载或重建运行环境后重试即可——\
                 冷缓存要先拉整套构建工具链，慢链路上十几分钟属正常，重试会复用已下载的系统包。\
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
        self.job_advance(job_id, "重启 PHP 容器", 80, "重启容器以加载扩展变更")
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
        self.job_advance(job_id, "重启 PHP 容器", 85, "重启容器以加载扩展变更")
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
}
