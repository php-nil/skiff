//! vhost 渲染、nginx 重载与站点配置文件读写。
//!
//! 属于 `sites::crud` 子模块；与 `mod.rs` / `tls.rs` 共同组成 `impl AppState`。
use std::path::{Path, PathBuf};

use super::*;
use crate::AppState;

impl AppState {
    pub(crate) fn write_default_vhost(&self, s: &SiteSettings) -> std::io::Result<()> {
        let fs = self.admin_fs();
        fs.ensure_dir_kernel(&s.conf_dir)
            .map_err(std::io::Error::other)?;
        // 兜底文件与站点 include 入口始终存在；默认站点的投影以库里 `is_default` 为准。
        fs.write_kernel(&s.conf_dir.join(FALLBACK_CONF), FALLBACK_VHOST.as_bytes())
            .map_err(std::io::Error::other)?;
        fs.write_kernel(
            &s.conf_dir.join(SITES_INCLUDE_CONF),
            s.sites_include_conf().as_bytes(),
        )
        .map_err(std::io::Error::other)?;
        self.sync_default_conf(s)
    }

    fn bound_runtime(&self, site: &Site) -> Result<Runtime, SiteError> {
        let rid = site
            .runtime_id
            .as_deref()
            .ok_or_else(|| SiteError::BadRequest("运行环境站点未绑定运行环境".into()))?;
        self.db()
            .runtime_by_id(rid)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))
    }

    fn php_socket_paths(
        &self,
        s: &SiteSettings,
        site: &Site,
    ) -> Result<(String, String), SiteError> {
        let rid = site
            .runtime_id
            .as_deref()
            .ok_or_else(|| SiteError::BadRequest("PHP 站点未绑定运行环境".into()))?;
        let rt = self
            .db()
            .runtime_by_id(rid)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::Unavailable(format!("运行环境不存在：{rid}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境不提供 PHP-FPM socket",
                rt.kind
            )));
        }
        let host = rt
            .socket_path
            .clone()
            .unwrap_or_else(|| s.runtime_socket(&rt.kind, &rt.name).display().to_string());
        let container = s.container_path(Path::new(&host));
        Ok((host, container))
    }

    pub(crate) fn render_vhost_for(
        &self,
        s: &SiteSettings,
        site: &Site,
        aliases: &[String],
        tls: Option<&VhostTls>,
    ) -> Result<String, SiteError> {
        let site_dir = PathBuf::from(&site.root_dir);
        let root = s.container_path(&site_index_dir(&site_dir));
        let access = s.container_log(&site_dir, LogKind::Access);
        let error = s.container_log(&site_dir, LogKind::Error);
        let runtime = if site.site_type == TYPE_RUNTIME {
            Some(self.bound_runtime(site)?)
        } else {
            None
        };
        let socket = if runtime
            .as_ref()
            .is_some_and(|r| r.kind == skiff_store::KIND_PHP)
        {
            Some(self.php_socket_paths(s, site)?.1)
        } else {
            None
        };
        // 巡检已判定失联的运行环境：不生成指向容器地址的可用配置，直接维护态。
        let maintenance = runtime
            .as_ref()
            .and_then(|rt| self.runtime_guard(&rt.id))
            .filter(|g| !g.available)
            .map(|g| g.reason);
        // 进程型站点的容器地址先算好：vhost 只持有它的引用，不能指向临时值；
        // 维护态下不计算，避免「已失联」时因地址缺失而渲染失败。
        let process_address = if maintenance.is_none() {
            match runtime.as_ref() {
                Some(rt)
                    if matches!(
                        rt.kind.as_str(),
                        skiff_store::KIND_RUST | skiff_store::KIND_NODE
                    ) =>
                {
                    Some(self.runtime_site_address(s, rt)?)
                }
                _ => None,
            }
        } else {
            None
        };
        let kind = if let Some(reason) = maintenance.as_deref() {
            VhostKind::Maintenance { reason }
        } else {
            match site.site_type.as_str() {
                TYPE_STATIC => VhostKind::Static { root: &root },
                TYPE_PROXY => VhostKind::Proxy {
                    target: site.proxy_target.as_deref().unwrap_or(""),
                    websocket: site.proxy_websocket,
                },
                _ => match runtime.as_ref().map(|r| r.kind.as_str()) {
                    Some(kind @ (skiff_store::KIND_RUST | skiff_store::KIND_NODE)) => {
                        let rt = runtime.as_ref().expect("runtime 已判定存在");
                        let address = process_address.as_deref().ok_or_else(|| {
                            SiteError::Unavailable(format!(
                                "{} 运行环境尚未分配容器地址，请重建运行环境后再启用站点",
                                runtime_catalog::kind_label(kind)
                            ))
                        })?;
                        VhostKind::Process {
                            root: &root,
                            address,
                            port: rt.port.ok_or_else(|| {
                                SiteError::Unavailable(format!(
                                    "{} 运行环境缺少端口，请重新安装",
                                    runtime_catalog::kind_label(kind)
                                ))
                            })?,
                        }
                    }
                    Some(skiff_store::KIND_GO) => {
                        return Err(SiteError::Unavailable(
                            "Go 运行环境尚未接入，当前不能生成 vhost".into(),
                        ))
                    }
                    _ => VhostKind::Php {
                        root: &root,
                        socket: socket.as_deref().unwrap_or(""),
                    },
                },
            }
        };
        Ok(render_site_vhost(
            &site.domain,
            aliases,
            kind,
            &access,
            &error,
            VhostPolicy::new(&site.rewrite, site.limit_rate_kbps),
            tls,
        ))
    }

    pub(crate) fn render_default_vhost_for(
        &self,
        s: &SiteSettings,
        site: &Site,
        tls: Option<&VhostTls>,
    ) -> Result<String, SiteError> {
        let site_dir = PathBuf::from(&site.root_dir);
        let root = s.container_path(&site_index_dir(&site_dir));
        let access = s.container_log(&site_dir, LogKind::Access);
        let error = s.container_log(&site_dir, LogKind::Error);
        let runtime = if site.site_type == TYPE_RUNTIME {
            Some(self.bound_runtime(site)?)
        } else {
            None
        };
        let socket = if runtime
            .as_ref()
            .is_some_and(|r| r.kind == skiff_store::KIND_PHP)
        {
            Some(self.php_socket_paths(s, site)?.1)
        } else {
            None
        };
        // 巡检已判定失联的运行环境：不生成指向容器地址的可用配置，直接维护态。
        let maintenance = runtime
            .as_ref()
            .and_then(|rt| self.runtime_guard(&rt.id))
            .filter(|g| !g.available)
            .map(|g| g.reason);
        // 进程型站点的容器地址先算好：vhost 只持有它的引用，不能指向临时值；
        // 维护态下不计算，避免「已失联」时因地址缺失而渲染失败。
        let process_address = if maintenance.is_none() {
            match runtime.as_ref() {
                Some(rt)
                    if matches!(
                        rt.kind.as_str(),
                        skiff_store::KIND_RUST | skiff_store::KIND_NODE
                    ) =>
                {
                    Some(self.runtime_site_address(s, rt)?)
                }
                _ => None,
            }
        } else {
            None
        };
        let kind = if let Some(reason) = maintenance.as_deref() {
            VhostKind::Maintenance { reason }
        } else {
            match site.site_type.as_str() {
                TYPE_STATIC => VhostKind::Static { root: &root },
                TYPE_PROXY => VhostKind::Proxy {
                    target: site.proxy_target.as_deref().unwrap_or(""),
                    websocket: site.proxy_websocket,
                },
                _ => match runtime.as_ref().map(|r| r.kind.as_str()) {
                    Some(kind @ (skiff_store::KIND_RUST | skiff_store::KIND_NODE)) => {
                        let rt = runtime.as_ref().expect("runtime 已判定存在");
                        let address = process_address.as_deref().ok_or_else(|| {
                            SiteError::Unavailable(format!(
                                "{} 运行环境尚未分配容器地址，请重建运行环境后再启用站点",
                                runtime_catalog::kind_label(kind)
                            ))
                        })?;
                        VhostKind::Process {
                            root: &root,
                            address,
                            port: rt.port.ok_or_else(|| {
                                SiteError::Unavailable(format!(
                                    "{} 运行环境缺少端口，请重新安装",
                                    runtime_catalog::kind_label(kind)
                                ))
                            })?,
                        }
                    }
                    Some(skiff_store::KIND_GO) => {
                        return Err(SiteError::Unavailable(
                            "Go 运行环境尚未接入，当前不能生成 vhost".into(),
                        ))
                    }
                    _ => VhostKind::Php {
                        root: &root,
                        socket: socket.as_deref().unwrap_or(""),
                    },
                },
            }
        };
        Ok(render_default_vhost(
            &site.domain,
            kind,
            &access,
            &error,
            VhostPolicy::new(&site.rewrite, site.limit_rate_kbps),
            tls,
        ))
    }

    pub(crate) fn sync_default_conf(&self, s: &SiteSettings) -> std::io::Result<()> {
        let conf = s.conf_dir.join(DEFAULT_CONF);
        match self.db().default_site() {
            Ok(Some(site)) => {
                let tls = self.site_tls(s, &site);
                let content = self
                    .render_default_vhost_for(s, &site, tls.as_ref())
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
                if std::fs::read(&conf).ok().as_deref() != Some(content.as_bytes()) {
                    self.admin_fs()
                        .write_kernel(&conf, content.as_bytes())
                        .map_err(std::io::Error::other)?;
                }
                Ok(())
            }
            Ok(None) => {
                if conf.exists() {
                    self.admin_fs()
                        .remove_kernel(&conf)
                        .map_err(std::io::Error::other)?;
                }
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e.to_string())),
        }
    }

    pub(crate) async fn activate_nginx_locked(&self) -> Result<NginxActivation, String> {
        activate_with(&DockerNginx { state: self }).await
    }

    pub async fn activate_nginx(&self) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.activate_nginx_locked()
            .await
            .map(|_| ())
            .map_err(SiteError::Unavailable)
    }

    pub async fn restart_nginx(&self) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.restart_nginx_locked().await
    }

    pub(crate) async fn restart_nginx_locked(&self) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == NGINX_CONTAINER)
            .ok_or_else(|| SiteError::Unavailable("nginx 运行环境未安装".into()))?;
        let obj = skiff_docker::ObjectRef {
            id: c.id.clone(),
            name: c.name.clone(),
            labels: c.labels.clone(),
        };
        self.kernel_execute(Verb::Restart, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| SiteError::Unavailable(format!("重载 nginx 失败：{e}")))
    }

    pub async fn validate_nginx(&self) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.validate_nginx_locked().await
    }

    pub(crate) async fn validate_nginx_locked(&self) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        if !nginx_container_running(&rows) {
            return Err(SiteError::Unavailable(
                "nginx 运行环境未启动，无法校验配置".into(),
            ));
        }
        let (code, out, timed_out) = self
            .docker()
            .exec_capture_timeout(
                NGINX_CONTAINER,
                vec!["nginx".into(), "-t".into()],
                crate::nginx::NGINX_TEST_TIMEOUT,
            )
            .await
            .map_err(|e| SiteError::Unavailable(format!("执行 nginx -t 失败：{e}")))?;
        if timed_out {
            return Err(SiteError::Unavailable(format!(
                "nginx -t 超过 {} 秒未返回，配置未生效（容器内校验进程可能仍在运行）",
                crate::nginx::NGINX_TEST_TIMEOUT.as_secs()
            )));
        }
        if code == 0 {
            Ok(())
        } else {
            Err(SiteError::Conflict(format!(
                "nginx -t 未通过：{}",
                out.trim()
            )))
        }
    }

    pub(crate) fn write_site_vhost(&self, s: &SiteSettings, domain: &str) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let fs = self.admin_fs();
        fs.ensure_dir_kernel(&site_conf_dir(&site_dir))
            .map_err(|e| SiteError::Internal(format!("创建站点配置目录失败：{e}")))?;
        let aliases = self.site_aliases(&site.domain);
        let tls = self.site_tls(s, &site);
        let vhost = self.render_vhost_for(s, &site, &aliases, tls.as_ref())?;
        let conf = s.vhost_file(&site_dir, &site.domain);
        let disabled = s.vhost_disabled(&site_dir, &site.domain);
        if site.status == "running" {
            fs.write_kernel(&conf, vhost.as_bytes())
                .map_err(|e| SiteError::Internal(format!("写入站点配置失败：{e}")))?;
            if let Err(e) = fs.remove_kernel(&disabled) {
                tracing::debug!(path = %disabled.display(), error = %e, "清理已禁用的站点配置失败");
            }
        } else {
            fs.write_kernel(&disabled, vhost.as_bytes())
                .map_err(|e| SiteError::Internal(format!("写入站点配置失败：{e}")))?;
            if let Err(e) = fs.remove_kernel(&conf) {
                tracing::debug!(path = %conf.display(), error = %e, "清理生效中的站点配置失败");
            }
        }
        Ok(())
    }

    pub(crate) fn rewrite_site_vhosts(&self, s: &SiteSettings) -> std::io::Result<usize> {
        let sites = self
            .db()
            .list_sites()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let fs = self.admin_fs();
        fs.ensure_dir_kernel(&s.conf_dir)
            .map_err(std::io::Error::other)?;
        fs.write_kernel(
            &s.conf_dir.join(SITES_INCLUDE_CONF),
            s.sites_include_conf().as_bytes(),
        )
        .map_err(std::io::Error::other)?;
        for site in &sites {
            let site_dir = PathBuf::from(&site.root_dir);
            fs.ensure_dir_kernel(&site_conf_dir(&site_dir))
                .map_err(std::io::Error::other)?;
            let aliases = self.site_aliases(&site.domain);
            let tls = self.site_tls(s, site);
            let vhost = self
                .render_vhost_for(s, site, &aliases, tls.as_ref())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let conf = s.vhost_file(&site_dir, &site.domain);
            let disabled = s.vhost_disabled(&site_dir, &site.domain);
            if site.status == "running" {
                fs.write_kernel(&conf, vhost.as_bytes())
                    .map_err(std::io::Error::other)?;
                if let Err(e) = fs.remove_kernel(&disabled) {
                    tracing::debug!(path = %disabled.display(), error = %e, "清理已禁用的站点配置失败");
                }
            } else {
                fs.write_kernel(&disabled, vhost.as_bytes())
                    .map_err(std::io::Error::other)?;
                if let Err(e) = fs.remove_kernel(&conf) {
                    tracing::debug!(path = %conf.display(), error = %e, "清理生效中的站点配置失败");
                }
            }
        }
        Ok(sites.len())
    }

    pub(crate) async fn apply_site_aliases(
        &self,
        site: &Site,
        previous: &[String],
    ) -> Result<(), SiteError> {
        let s = self.site_settings();
        let rollback = |this: &Self| {
            let db = this.db();
            if let Err(e) = db.clear_site_aliases(&site.domain) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点别名失败（清理）");
            }
            if let Err(e) = db.add_site_aliases(&site.domain, previous) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点别名失败（恢复）");
            }
            if let Err(e) = this.write_site_vhost(&s, &site.domain) {
                tracing::error!(site = %site.domain, error = %e, "回滚站点配置失败");
            }
        };
        // 写文件 → 校验 → reload 在同一把锁内：并发增删别名不会互相读到半套配置。
        let _guard = super::nginx_write_lock().lock().await;
        if let Err(e) = self.write_site_vhost(&s, &site.domain) {
            rollback(self);
            return Err(e);
        }
        if let Err(e) = self.validate_nginx_locked().await {
            rollback(self);
            return Err(e);
        }
        if let Err(e) = self.activate_nginx_locked().await {
            rollback(self);
            if let Err(re) = self.restart_nginx_locked().await {
                tracing::error!(error = %re, "回滚后重启 nginx 失败，现网可能仍加载失败配置");
            }
            return Err(SiteError::Unavailable(e));
        }
        Ok(())
    }

    fn site_vhost_path(&self, domain: &str) -> Result<(PathBuf, bool), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let active = s.vhost_file(&site_dir, domain);
        if active.exists() {
            return Ok((active, true));
        }
        let disabled = s.vhost_disabled(&site_dir, domain);
        if disabled.exists() {
            return Ok((disabled, false));
        }
        Err(SiteError::BadRequest(format!(
            "站点 vhost 文件不存在：{}",
            active.display()
        )))
    }

    pub fn read_site_vhost(&self, domain: &str) -> Result<(String, String, bool), SiteError> {
        let (path, active) = self.site_vhost_path(domain)?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| SiteError::Internal(format!("读取 {} 失败：{e}", path.display())))?;
        Ok((path.display().to_string(), content, active))
    }

    pub(crate) async fn validate_site_vhost_content(
        &self,
        s: &SiteSettings,
        domain: &str,
        original: &str,
        content: &str,
    ) -> Result<(), SiteError> {
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        if !nginx_container_running(&rows) {
            return Err(SiteError::Unavailable(
                "Web 服务（OpenResty）未运行，无法校验配置".into(),
            ));
        }
        let dir = s.web_root.join("skiff").join("vhost-check");
        self.admin_fs()
            .ensure_dir_kernel(&dir)
            .map_err(|e| SiteError::Internal(format!("创建校验目录失败：{e}")))?;
        let stamp = chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default();
        let candidate = dir.join(format!("{domain}-{stamp}.conf.candidate"));
        let main = dir.join(format!("{domain}-{stamp}.check-main.candidate"));
        let container_candidate = s.container_path(&candidate);
        let container_main = s.container_path(&main);
        self.admin_fs()
            .write_kernel(&candidate, content.as_bytes())
            .map_err(|e| SiteError::Internal(format!("写入候选配置失败：{e}")))?;
        self.admin_fs()
            .write_kernel(&main, vhost_check_main(&container_candidate).as_bytes())
            .map_err(|e| SiteError::Internal(format!("写入校验主配置失败：{e}")))?;
        let result = self.nginx_test_candidate(&container_main).await;
        remove_quietly(self.admin_fs(), &candidate);
        remove_quietly(self.admin_fs(), &main);
        let (code, out) = result?;
        if code == 0 {
            Ok(())
        } else {
            Err(SiteError::Conflict(format!(
                "站点配置校验未通过：{}；差异：{}",
                crate::nginx::error_line(&out),
                crate::nginx::diff_summary(original, content, 6)
            )))
        }
    }

    pub async fn save_site_vhost(
        &self,
        domain: &str,
        content: &str,
    ) -> Result<VhostSaveOutcome, SiteError> {
        if content.trim().is_empty() {
            return Err(SiteError::BadRequest("配置内容不能为空".into()));
        }
        if !content.contains("server") {
            return Err(SiteError::BadRequest(
                "配置至少需要包含一个 server 块".into(),
            ));
        }
        let s = self.site_settings();
        let (path, active) = self.site_vhost_path(domain)?;
        let original = std::fs::read(&path)
            .map_err(|e| SiteError::Internal(format!("读取原配置失败：{e}")))?;
        let original_text = String::from_utf8_lossy(&original).to_string();
        // 无论启停都先校验：停止态的文件不在 include 内，直接 reload 等于没测。
        self.validate_site_vhost_content(&s, domain, &original_text, content)
            .await?;
        // 写入与生效在同一把锁内：并发保存同一站点不会互相吃半套配置，也不会
        // 与本模块的证书成对写入（`upload_cert` / `generate_self_signed`）交错。
        let _guard = super::nginx_write_lock().lock().await;
        // 先写临时文件再 rename，避免半截文件被 nginx 加载；临时名进程内唯一，
        // 并发保存同一站点时不会出现「后写者删掉先写者临时文件」。
        self.admin_fs()
            .write_kernel_mode(&path, content.as_bytes(), 0o644)
            .map_err(|e| SiteError::Internal(format!("替换配置失败：{e}")))?;
        if !active {
            return Ok(VhostSaveOutcome { reloaded: false });
        }
        match self.activate_nginx_locked().await {
            // reload 已由 worker 换代自证生效：服务不中断，也不必再等启动窗口。
            Ok(NginxActivation::Reloaded) => Ok(VhostSaveOutcome { reloaded: true }),
            Ok(NginxActivation::Restarted) => {
                // 回退重启：返回成功不代表 nginx 配置有效，容器仍可能在启动后立即退出。
                // 留出极短启动窗口再核对状态，失败则回滚并尽量把旧配置重新拉起。
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let env = self.site_env_status().await;
                if env.installed && env.nginx_state != "running" {
                    if let Err(re) = self.admin_fs().write_kernel_mode(&path, &original, 0o644) {
                        tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
                    }
                    if let Err(re) = self.restart_nginx_locked().await {
                        tracing::error!(error = %re, "回滚后重载 nginx 失败");
                    }
                    return Err(SiteError::Unavailable(format!(
                        "配置已回滚到保存前内容：nginx 未能保持运行（当前状态 {}）",
                        env.nginx_state
                    )));
                }
                Ok(VhostSaveOutcome { reloaded: true })
            }
            Err(e) => {
                // 回滚文件内容，保证磁盘与正在运行的 nginx 保持一致。
                if let Err(re) = self.admin_fs().write_kernel_mode(&path, &original, 0o644) {
                    tracing::error!(path = %path.display(), error = %re, "回滚站点配置内容失败");
                }
                Err(SiteError::Unavailable(format!(
                    "配置已回滚到保存前内容：重载 nginx 失败：{e}"
                )))
            }
        }
    }
}
