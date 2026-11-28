//! TLS 证书管理：自签/上传/切换/证书校验。
//!
//! 属于 `sites::crud` 子模块；与 `mod.rs` / `vhost.rs` 共同组成 `impl AppState`。
use std::path::PathBuf;

use skiff_store::Site;

use super::*;
use crate::AppState;

impl AppState {
    pub(crate) fn site_tls(&self, s: &SiteSettings, site: &Site) -> Option<VhostTls> {
        if !site.https {
            return None;
        }
        let site_dir = PathBuf::from(&site.root_dir);
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        if !cert.is_file() || !key.is_file() {
            return None;
        }
        Some(VhostTls {
            cert: s.container_path(&cert),
            key: s.container_path(&key),
            force_https: site.force_https,
        })
    }

    pub async fn site_tls_info(&self, domain: &str) -> Result<SiteTlsInfo, SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        let mut info = SiteTlsInfo {
            enabled: site.https,
            force_https: site.force_https,
            cert_path: cert.display().to_string(),
            key_path: key.display().to_string(),
            cert_exists: s.cert_ready(&site_dir),
            subject: None,
            issuer: None,
            expires: None,
        };
        if info.cert_exists {
            let text = cached_cert_text(&cert, &key).await;
            info.subject = text.subject;
            info.issuer = text.issuer;
            info.expires = text.expires;
        } else {
            // 站点删除或证书被删：清掉旧条目，避免同名目录下次复用时读到陈旧解析。
            tls_info_cache()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&cert);
        }
        Ok(info)
    }

    pub async fn generate_self_signed(&self, domain: &str) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let site_dir = PathBuf::from(&site.root_dir);
        let aliases = self.site_aliases(domain);
        let dir = site_ssl_dir(&site_dir);
        self.admin_fs()
            .ensure_dir_kernel(&dir)
            .map_err(|e| SiteError::Internal(format!("创建证书目录失败：{e}")))?;
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        // 先生成到进程内唯一临时名，再在锁内成对替换：openssl 生成期间不占锁，
        // 也不会让 nginx 读到「新证书 + 旧私钥」的中间态。
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let tmp_key = crate::atomic::sibling_temp(&key);
        let mut san = format!("DNS:{}", site.domain);
        for a in &aliases {
            san.push_str(&format!(",DNS:{a}"));
        }
        // 走统一执行通道（`skiff_host`）：超时、读流、错误文案只有一份。
        // 直接起子进程即可——openssl 不要 root，也不必为它 fork 一个 systemd 单元。
        let args: Vec<String> = vec![
            "req".into(),
            "-x509".into(),
            "-newkey".into(),
            "rsa:2048".into(),
            "-sha256".into(),
            "-days".into(),
            "825".into(),
            "-nodes".into(),
            "-keyout".into(),
            tmp_key.display().to_string(),
            "-out".into(),
            tmp_cert.display().to_string(),
            "-subj".into(),
            format!("/CN={}", site.domain),
            "-addext".into(),
            format!("subjectAltName={san}"),
        ];
        let out = crate::host::run_host_msg(
            skiff_host::HostSpec::direct("openssl", &args)
                .prefix("skiff-cert")
                .description("skiff 自签证书生成")
                .timeout(30),
        )
        .await
        .map_err(|e| SiteError::Internal(format!("生成自签证书失败：{e}")))?;
        if !out.success {
            remove_quietly(self.admin_fs(), &tmp_cert);
            remove_quietly(self.admin_fs(), &tmp_key);
            return Err(SiteError::Internal(format!(
                "生成自签证书失败：{}",
                out.stderr.trim()
            )));
        }
        let _guard = super::nginx_write_lock().lock().await;
        if let Err(failure) = replace_cert_pair(self.admin_fs(), &tmp_cert, &tmp_key, &cert, &key) {
            if failure.was_cleared() {
                self.repair_no_cert_state(domain).await;
            }
            return Err(failure.into_site_error());
        }
        self.apply_tls_locked(domain, true, false).await
    }

    pub async fn upload_cert(
        &self,
        domain: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if !cert_pem.contains("BEGIN CERTIFICATE") || !key_pem.contains("PRIVATE KEY") {
            return Err(SiteError::BadRequest(
                "证书需为 PEM（BEGIN CERTIFICATE），私钥需包含 PRIVATE KEY".into(),
            ));
        }
        let site_dir = PathBuf::from(&site.root_dir);
        let dir = site_ssl_dir(&site_dir);
        self.admin_fs()
            .ensure_dir_kernel(&dir)
            .map_err(|e| SiteError::Internal(format!("创建证书目录失败：{e}")))?;
        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        // 唯一临时名：同名并发上传各写各的，不会互相截断或抢先 rename。
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let tmp_key = crate::atomic::sibling_temp(&key);
        // 写盘与 `openssl` 校验（连续 fork 4 次）不能占 async worker：与
        // `generate_self_signed` 一致投递到阻塞池；失败时在同一闭包内清理临时文件。
        let (cert_text, key_text) = (cert_pem.to_string(), key_pem.to_string());
        let (write_cert, write_key) = (tmp_cert.clone(), tmp_key.clone());
        // 写盘留在阻塞池（纯 IO）；`openssl` 校验走统一执行通道，不再在阻塞池里 fork。
        // 证书与私钥是内核受保护前缀里的 TLS 材料，写入经内核文件通道并各自留审计；
        // 权限位随写入给定（私钥 0600），不再另调 chmod。
        let kfs = self.admin_fs_arc();
        tokio::task::spawn_blocking(move || {
            kfs.write_kernel_mode(&write_cert, cert_text.as_bytes(), 0o644)
                .map_err(|e| SiteError::Internal(format!("写入证书失败：{e}")))?;
            kfs.write_kernel_mode(&write_key, key_text.as_bytes(), 0o600)
                .map_err(|e| SiteError::Internal(format!("写入私钥失败：{e}")))?;
            Ok::<(), SiteError>(())
        })
        .await
        .map_err(|e| SiteError::Internal(format!("证书写入任务失败：{e}")))??;
        if let Err(e) = verify_cert_pair(&tmp_cert, &tmp_key).await {
            remove_quietly(self.admin_fs(), &tmp_cert);
            remove_quietly(self.admin_fs(), &tmp_key);
            return Err(SiteError::BadRequest(e));
        }
        // 成对替换与 reload 必须在一把锁内完成：中间态对 nginx 不可见。
        let _guard = super::nginx_write_lock().lock().await;
        if let Err(failure) = replace_cert_pair(self.admin_fs(), &tmp_cert, &tmp_key, &cert, &key) {
            if failure.was_cleared() {
                self.repair_no_cert_state(domain).await;
            }
            return Err(failure.into_site_error());
        }
        self.apply_tls_locked(domain, true, false).await
    }

    pub async fn disable_tls(&self, domain: &str) -> Result<(), SiteError> {
        self.apply_tls(domain, false, false).await
    }

    pub async fn delete_cert(&self, domain: &str) -> Result<(), SiteError> {
        // 先关 HTTPS 再删文件，两步都在锁内：不会与上传/生成交错。
        let _guard = super::nginx_write_lock().lock().await;
        self.apply_tls_locked(domain, false, false).await?;
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let s = self.site_settings();
        let dir = site_ssl_dir(Path::new(&site.root_dir));
        // 只删证书文件，保留 ssl/ 目录（它是站点固定布局的一部分）。
        for p in [
            s.cert_file(Path::new(&site.root_dir)),
            s.key_file(Path::new(&site.root_dir)),
        ] {
            if p.exists() {
                self.admin_fs()
                    .remove_kernel(&p)
                    .map_err(|e| SiteError::Internal(format!("删除证书文件失败：{e}")))?;
            }
        }
        // 清理上传留下的中间件（唯一名以 `.cert.pem.` / `.key.pem.` 开头），避免半套。
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let is_upload_tmp = (name.starts_with(".cert.pem.")
                    || name.starts_with(".key.pem."))
                    && name.ends_with(".tmp");
                if !is_upload_tmp {
                    continue;
                }
                if let Err(e) = self.admin_fs().remove_kernel(&entry.path()) {
                    tracing::debug!(path = %entry.path().display(), error = %e, "清理残留的临时证书失败");
                }
            }
        }
        Ok(())
    }

    pub async fn set_force_https(&self, domain: &str, force: bool) -> Result<(), SiteError> {
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        if force && !site.https {
            return Err(SiteError::BadRequest(
                "请先启用 HTTPS（上传或生成证书）".into(),
            ));
        }
        self.apply_tls(domain, site.https, force).await
    }

    pub async fn set_site_policy(
        &self,
        domain: &str,
        rewrite: &str,
        limit_rate_kbps: u32,
    ) -> Result<(), SiteError> {
        let rewrite = validate_rewrite(rewrite)?;
        if limit_rate_kbps > MAX_LIMIT_RATE_KBPS {
            return Err(SiteError::BadRequest(format!(
                "限速不能超过 {MAX_LIMIT_RATE_KBPS} KB/s"
            )));
        }
        let _guard = super::nginx_write_lock().lock().await;
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let (old_rewrite, old_limit) = (site.rewrite.clone(), site.limit_rate_kbps);
        if old_rewrite == rewrite && old_limit == limit_rate_kbps {
            return Ok(());
        }
        let site_dir = PathBuf::from(&site.root_dir);
        let site_conf = if site.status == "running" {
            s.vhost_file(&site_dir, domain)
        } else {
            s.vhost_disabled(&site_dir, domain)
        };
        let default_conf = s.conf_dir.join(DEFAULT_CONF);
        let snapshot: Vec<(PathBuf, Option<Vec<u8>>)> = [&site_conf, &default_conf]
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(p).ok()))
            .collect();
        let rollback_db = || {
            if let Err(e) = self.db().set_site_policy(domain, &old_rewrite, old_limit) {
                tracing::error!(domain, error = %e, "回滚站点策略失败：数据库与磁盘可能不一致");
            }
        };
        self.db()
            .set_site_policy(domain, rewrite, limit_rate_kbps)
            .map_err(|e| SiteError::Internal(format!("写入站点策略失败：{e}")))?;
        if let Err(e) = self.write_site_vhost(&s, domain) {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            rollback_db();
            return Err(e);
        }
        if let Err(e) = self.sync_default_conf(&s) {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            rollback_db();
            return Err(SiteError::Internal(format!("写入默认站点配置失败：{e}")));
        }
        if let Err(e) = self.validate_nginx_locked().await {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            rollback_db();
            return Err(e);
        }
        if let Err(e) = self.activate_nginx_locked().await {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            rollback_db();
            if let Err(re) = self.restart_nginx_locked().await {
                tracing::error!(error = %re, "回滚后重启 nginx 失败，现网可能仍加载失败配置");
            }
            return Err(SiteError::Unavailable(e));
        }
        Ok(())
    }

    async fn apply_tls(
        &self,
        domain: &str,
        https: bool,
        force_https: bool,
    ) -> Result<(), SiteError> {
        let _guard = super::nginx_write_lock().lock().await;
        self.apply_tls_locked(domain, https, force_https).await
    }

    async fn apply_tls_locked(
        &self,
        domain: &str,
        https: bool,
        force_https: bool,
    ) -> Result<(), SiteError> {
        let s = self.site_settings();
        let site = self
            .db()
            .site_by_domain(domain)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("站点不存在：{domain}")))?;
        let (old_https, old_force) = (site.https, site.force_https);
        let site_dir = PathBuf::from(&site.root_dir);
        if https && (!s.cert_file(&site_dir).is_file() || !s.key_file(&site_dir).is_file()) {
            return Err(SiteError::BadRequest(
                "缺少证书文件，请先上传或生成自签证书".into(),
            ));
        }
        let site_conf = if site.status == "running" {
            s.vhost_file(&site_dir, domain)
        } else {
            s.vhost_disabled(&site_dir, domain)
        };
        let default_conf = s.conf_dir.join(DEFAULT_CONF);
        let snapshot: Vec<(PathBuf, Option<Vec<u8>>)> = [&site_conf, &default_conf]
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(p).ok()))
            .collect();

        self.db()
            .set_site_tls(domain, https, force_https)
            .map_err(|e| SiteError::Internal(format!("写入 HTTPS 状态失败：{e}")))?;
        if let Err(e) = self.write_site_vhost(&s, domain) {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(e);
        }
        if let Err(e) = self.sync_default_conf(&s) {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(SiteError::Internal(format!("写入默认站点配置失败：{e}")));
        }
        if let Err(e) = self.validate_nginx_locked().await {
            // 校验失败时运行中的 nginx 还没加载新配置，恢复文件即可；
            // 这里**不能再重启**——若失败源于别的坏 conf，重启会让现网 nginx 起不来。
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            return Err(e);
        }
        if let Err(e) = self.activate_nginx_locked().await {
            restore_conf_snapshot(self.admin_fs(), &snapshot);
            if let Err(re) = self.db().set_site_tls(domain, old_https, old_force) {
                tracing::error!(domain, error = %re, "回滚 HTTPS 状态失败：数据库与磁盘可能不一致");
            }
            if let Err(re) = self.restart_nginx_locked().await {
                tracing::error!(error = %re, "回滚后重启 nginx 失败，现网可能仍加载失败配置");
            }
            return Err(SiteError::Unavailable(e));
        }
        Ok(())
    }

    pub(crate) async fn repair_no_cert_state(&self, domain: &str) {
        if let Err(e) = self.db().set_site_tls(domain, false, false) {
            tracing::error!(domain, error = %e, "证书写入失败后回滚 HTTPS 状态失败");
        }
        let s = self.site_settings();
        if let Err(e) = self.write_site_vhost(&s, domain) {
            tracing::error!(domain, error = %e, "证书写入失败后重写站点配置失败");
        }
        if let Err(e) = self.sync_default_conf(&s) {
            tracing::error!(domain, error = %e, "证书写入失败后重写默认站点配置失败");
        }
        if let Err(e) = self.validate_nginx_locked().await {
            tracing::warn!(error = %e, "证书写入失败后 nginx -t 未通过（配置已不含证书引用）");
            return;
        }
        if let Err(e) = self.activate_nginx_locked().await {
            tracing::warn!(error = %e, "证书写入失败后 nginx 重载失败");
        }
    }
}
