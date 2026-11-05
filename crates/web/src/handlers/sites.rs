//! 内核「网站」页面：站点列表、运行环境、环境设置、日志与操作。

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_store::Site;

use crate::state::sites::{NewSite, SiteEnvStatus, SiteError};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{
    DirEntryView, DirListingView, InstallStepView, SiteDeletePage, SiteDetailPage, SiteEnvPage,
    SiteEnvView, SiteLogsPage, SiteNewForm, SiteNewPage, SiteSettingsPage, SiteTlsView, SiteView,
    SitesPage,
};
use crate::util::{encode_uri_component, form_first, human_size, parse_form};

fn env_view(env: &SiteEnvStatus) -> SiteEnvView {
    SiteEnvView {
        docker_available: env.docker_available,
        docker_reason: env.docker_reason.clone(),
        nginx_state: env.nginx_state.clone(),
        php_state: env.php_state.clone(),
        installed: env.installed,
        ready: env.ready(),
        port: env.port,
        port_message: env.port_message.clone(),
        nginx_image: env.nginx_image.clone(),
        php_image: env.php_image.clone(),
        waf_mode: env.waf_mode.clone(),
        waf_rules_dir: env.waf_rules_dir.clone(),
    }
}

fn site_view(s: &Site, aliases: Vec<String>, cert_ready: bool) -> SiteView {
    SiteView {
        domain: s.domain.clone(),
        // 站点目录本身（整站备份单元）与文档根都展示出来，避免把 `index/` 说成整站。
        index_dir: skiff_app::sites::site_index_dir(FsPath::new(&s.root_dir))
            .display()
            .to_string(),
        root_dir: s.root_dir.clone(),
        php_version: s.php_version.clone(),
        status: s.status.clone(),
        status_label: match s.status.as_str() {
            "running" => "运行中",
            "stopped" => "已停止",
            _ => "异常",
        }
        .into(),
        created: chrono::DateTime::from_timestamp(s.created_at, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        is_default: s.is_default,
        aliases_text: aliases.join("、"),
        aliases,
        https: s.https,
        force_https: s.force_https,
        cert_ready,
    }
}

fn tls_view(info: skiff_app::sites::SiteTlsInfo) -> SiteTlsView {
    SiteTlsView {
        enabled: info.enabled,
        force_https: info.force_https,
        cert_path: info.cert_path,
        key_path: info.key_path,
        cert_exists: info.cert_exists,
        subject: info.subject.unwrap_or_default(),
        issuer: info.issuer.unwrap_or_default(),
        expires: info.expires.unwrap_or_default(),
    }
}

fn map_err(e: SiteError) -> AppError {
    match e {
        SiteError::BadRequest(m) => AppError::BadRequest(m),
        SiteError::Conflict(m) => AppError::Conflict(m),
        SiteError::Unavailable(m) => AppError::Unavailable(m),
        SiteError::Internal(m) => AppError::Internal(m),
    }
}

fn require_admin(user: &crate::state::CurrentUser) -> Result<(), AppError> {
    handlers::require_permission(user, skiff_model::Permission::Admin)
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let env = state.site_env_status().await;
    let sites = state.db().list_sites()?;
    let keyword = q.get("q").cloned().unwrap_or_default();
    let needle = keyword.trim().to_lowercase();
    // 证书列与详情 SSL 标签同源：直接看站点证书目录下的 cert.pem/key.pem 是否存在。
    let cert_settings = state.site_settings();
    let views: Vec<SiteView> = sites
        .iter()
        .map(|s| {
            let ready = cert_settings.cert_file(FsPath::new(&s.root_dir)).is_file()
                && cert_settings.key_file(FsPath::new(&s.root_dir)).is_file();
            site_view(s, state.site_aliases(&s.domain), ready)
        })
        .filter(|v| {
            needle.is_empty()
                || v.domain.to_lowercase().contains(&needle)
                || v.root_dir.to_lowercase().contains(&needle)
                || v.php_version.to_lowercase().contains(&needle)
                || v.aliases_text.to_lowercase().contains(&needle)
        })
        .collect();
    let notice = q.get("notice").cloned().unwrap_or_default();
    let error = q.get("error").cloned().unwrap_or_default();
    handlers::render(SitesPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        sites: views,
        env: env_view(&env),
        can_admin: user.can(skiff_model::Permission::Admin),
        error,
        notice,
        keyword,
    })
}

fn split_aliases(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == '，' || c.is_whitespace())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn new_form(form: &HashMap<String, Vec<String>>) -> SiteNewForm {
    SiteNewForm {
        domain: form_first(form, "domain").trim().to_string(),
        aliases: form_first(form, "aliases").trim().to_string(),
        root_dir: form_first(form, "root_dir").trim().to_string(),
        php_version: form_first(form, "php_version").trim().to_string(),
    }
}

fn new_page(
    state: &AppState,
    user: &crate::state::CurrentUser,
    env: &SiteEnvStatus,
    form: SiteNewForm,
    error: String,
) -> Result<Response, AppError> {
    let error_field = if error.contains("别名") {
        "aliases"
    } else if error.contains("根目录") || error.contains("目录") {
        "root_dir"
    } else if error.contains("域名") || error.contains("站点已存在") {
        "domain"
    } else {
        ""
    };
    handlers::render(SiteNewPage {
        ctx: handlers::page_ctx(state, user, "core/sites/list"),
        env: env_view(env),
        form,
        error,
        error_field: error_field.to_string(),
        wizard_start: 1,
    })
}

pub async fn new_page_get(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let form = SiteNewForm {
        php_version: "8.5".to_string(),
        ..Default::default()
    };
    new_page(&state, &user, &env, form, String::new())
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let values = new_form(&form);
    let req = NewSite {
        domain: values.domain.clone(),
        root_dir: Some(values.root_dir.clone()).filter(|v| !v.is_empty()),
        php_version: if values.php_version.is_empty() {
            "8.5".to_string()
        } else {
            values.php_version.clone()
        },
        https: false,
        aliases: split_aliases(&values.aliases),
    };
    match state.create_site(req).await {
        Ok(result) => {
            let ip = handlers::client_ip(&state, &parts);
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.create",
                &result.domain,
                "ok",
                &result.root_dir,
            );
            let prefix = &state.cfg.entry_prefix;
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?notice={}",
                encode_uri_component(&result.domain),
                encode_uri_component("站点已创建")
            )))
        }
        Err(e) => {
            let message = match &e {
                SiteError::BadRequest(m) | SiteError::Conflict(m) => m.clone(),
                _ => return Err(map_err(e)),
            };
            let env = state.site_env_status().await;
            new_page(&state, &user, &env, values, message)
        }
    }
}

pub async fn env_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let env = state.site_env_status().await;
    let progress = state.site_install_progress().await;
    let settings = state.site_settings();
    handlers::render(SiteEnvPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        env: env_view(&env),
        steps: progress
            .steps
            .iter()
            .map(|s| InstallStepView {
                name: s.name.clone(),
                state: s.state.as_str().to_string(),
                detail: s.detail.clone(),
            })
            .collect(),
        running: progress.running,
        done: progress.done,
        install_error: progress.error,
        can_admin: user.can(skiff_model::Permission::Admin),
        registry_mirror: settings.registry_mirror,
        image_prefix: state
            .db()
            .get_setting("sites.image_prefix")?
            .unwrap_or_default(),
        error: q.get("error").cloned().unwrap_or_default(),
        notice: q.get("notice").cloned().unwrap_or_default(),
    })
}

pub async fn env_install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state.start_site_env_install();
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        "sites.env.install",
        "-",
        "accepted",
        "",
    );
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!("/{prefix}/sites/env")))
}

pub async fn env_settings_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let env = state.site_env_status().await;
    let settings = state.site_settings();
    handlers::render(SiteSettingsPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/settings"),
        env: env_view(&env),
        registry_mirror: settings.registry_mirror,
        image_prefix: state
            .db()
            .get_setting("sites.image_prefix")?
            .unwrap_or_default(),
        can_admin: user.can(skiff_model::Permission::Admin),
        error: q.get("error").cloned().unwrap_or_default(),
        notice: q.get("notice").cloned().unwrap_or_default(),
    })
}

pub async fn env_settings_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    for (form_key, setting_key) in [
        ("image_prefix", "sites.image_prefix"),
        ("nginx_image", "sites.nginx_image"),
        ("php_image", "sites.php_image"),
    ] {
        if form.contains_key(form_key) {
            let value = form_first(&form, form_key);
            state.db().set_setting(setting_key, value.trim())?;
        }
    }
    let prefix = &state.cfg.entry_prefix;
    let mut notice = "已保存".to_string();
    if form.contains_key("registry_mirror") {
        let raw = form_first(&form, "registry_mirror");
        let raw = raw.trim();
        let old = state
            .db()
            .get_setting("sites.registry_mirror")?
            .unwrap_or_default();
        if raw != old {
            match state.apply_registry_mirror(raw).await {
                Ok(r) => {
                    state.db().set_setting("sites.registry_mirror", raw)?;
                    if r.restarted {
                        notice =
                            "镜像源已写入 /etc/docker/daemon.json，Docker 已重启并通过可用性检查"
                                .into();
                    } else if r.written {
                        notice = format!(
                            "镜像源已写入 /etc/docker/daemon.json；检测到 {} 个运行中容器，未自动重启 Docker，请在维护窗口执行 systemctl restart docker",
                            r.running_containers
                        );
                    } else {
                        notice = "镜像源未变化，无需写入".into();
                    }
                }
                Err(e) => {
                    let ip = handlers::client_ip(&state, &parts);
                    let _ = state.db().audit(
                        &user.user.username,
                        &ip,
                        "sites.env.config",
                        "registry_mirror",
                        "failed",
                        &e,
                    );
                    return Ok(handlers::redirect(format!(
                        "/{prefix}/sites/settings?error={}",
                        encode_uri_component(&e)
                    )));
                }
            }
        }
    }
    // 端口固定为宿主 80（host 网络 + unix socket），不再允许从界面改。
    let ip = handlers::client_ip(&state, &parts);
    let _ = state
        .db()
        .audit(&user.user.username, &ip, "sites.env.config", "-", "ok", "");
    Ok(handlers::redirect(format!(
        "/{prefix}/sites/settings?notice={}",
        encode_uri_component(&notice)
    )))
}

pub async fn action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((domain, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let running = match action.as_str() {
        "start" => true,
        "stop" => false,
        _ => return Err(AppError::BadRequest(format!("未知站点操作：{action}"))),
    };
    state
        .set_site_running(&domain, running)
        .await
        .map_err(map_err)?;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        if running { "sites.start" } else { "sites.stop" },
        &domain,
        "ok",
        "",
    );
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!("/{prefix}/sites")))
}

pub async fn set_default(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let enabled = matches!(
        form_first(&form, "enabled").trim(),
        "1" | "on" | "true" | "yes"
    );
    state
        .set_default_site(&domain, enabled)
        .await
        .map_err(map_err)?;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        "sites.default",
        &domain,
        "ok",
        if enabled { "set" } else { "unset" },
    );
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!("/{prefix}/sites")))
}

/// SSL 标签：生成自签 / 上传 PEM / 强制跳转 / 关闭 / 删除证书。
pub async fn ssl_action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action");
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    let result = match action.as_str() {
        "generate" => state.generate_self_signed(&domain).await,
        "upload" => {
            state
                .upload_cert(
                    &domain,
                    &form_first(&form, "cert_pem"),
                    &form_first(&form, "key_pem"),
                )
                .await
        }
        "force" => {
            let enabled = matches!(
                form_first(&form, "enabled").trim(),
                "1" | "on" | "true" | "yes"
            );
            state.set_force_https(&domain, enabled).await
        }
        "disable" => state.disable_tls(&domain).await,
        "delete_cert" => state.delete_cert(&domain).await,
        _ => Err(SiteError::BadRequest(format!("未知 SSL 操作：{action}"))),
    };
    match result {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &format!("sites.ssl.{action}"),
                &domain,
                "ok",
                "",
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?tab=ssl&notice={}",
                encode_uri_component(&domain),
                encode_uri_component("SSL 设置已更新并重载 nginx")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &format!("sites.ssl.{action}"),
                &domain,
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?tab=ssl&error={}",
                encode_uri_component(&domain),
                encode_uri_component(&msg)
            )))
        }
    }
}

fn format_ts(secs: i64) -> String {
    if secs <= 0 {
        return "—".to_string();
    }
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn dir_view(listing: skiff_app::sites::SiteDirListing) -> DirListingView {
    DirListingView {
        rel: listing.rel,
        parent_url: encode_uri_component(&listing.parent),
        at_root: listing.at_root,
        entries: listing
            .entries
            .into_iter()
            .map(|e| DirEntryView {
                name: e.name,
                url: encode_uri_component(&e.rel),
                is_dir: e.is_dir,
                size: if e.is_dir {
                    "—".to_string()
                } else {
                    human_size(e.size as i64)
                },
                modified: format_ts(e.modified),
            })
            .collect(),
    }
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let site = state
        .db()
        .site_by_domain(&domain)?
        .ok_or_else(|| AppError::NotFound(format!("站点不存在：{domain}")))?;
    let env = state.site_env_status().await;
    let mut tab = q.get("tab").cloned().unwrap_or_else(|| "overview".into());
    if !matches!(
        tab.as_str(),
        "overview" | "domains" | "config" | "dir" | "ssl"
    ) {
        tab = "overview".to_string();
    }
    let mut error = q.get("error").cloned().unwrap_or_default();
    let notice = q.get("notice").cloned().unwrap_or_default();
    let mut config_path = String::new();
    let mut config_content = String::new();
    let mut config_active = false;
    let mut dir = DirListingView::default();
    if tab == "config" {
        match state.read_site_vhost(&domain) {
            Ok((path, content, active)) => {
                config_path = path;
                config_content = content;
                config_active = active;
            }
            Err(e) => error = format!("{e}"),
        }
    }
    if tab == "dir" {
        let rel = q.get("path").cloned().unwrap_or_default();
        match state.site_dir_listing(&domain, &rel) {
            Ok(listing) => dir = dir_view(listing),
            // 越界（../log、../ssl 等）按请求错误直接 400，不降级成页面内提示。
            Err(SiteError::BadRequest(m)) => return Err(AppError::BadRequest(m)),
            Err(e) => error = format!("{e}"),
        }
    }
    let ssl_info = state.site_tls_info(&domain).ok();
    let cert_ready = ssl_info.as_ref().map(|i| i.cert_exists).unwrap_or(false);
    let ssl = ssl_info.map(tls_view).unwrap_or_default();
    handlers::render(SiteDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        site: site_view(&site, state.site_aliases(&domain), cert_ready),
        env: env_view(&env),
        can_admin: user.can(skiff_model::Permission::Admin),
        tab,
        config_path,
        config_content,
        config_active,
        dir,
        ssl,
        error,
        notice,
    })
}

pub async fn save_config(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let content = form_first(&form, "content");
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.save_site_vhost(&domain, &content).await {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.config.save",
                &domain,
                "ok",
                "",
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?tab=config&notice={}",
                encode_uri_component(&domain),
                encode_uri_component("配置已保存并重载 nginx")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.config.save",
                &domain,
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?tab=config&error={}",
                encode_uri_component(&domain),
                encode_uri_component(&msg)
            )))
        }
    }
}

pub async fn batch(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action");
    let domains: Vec<String> = form
        .get("domains")
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();
    let prefix = &state.cfg.entry_prefix;
    if domains.is_empty() {
        return Ok(handlers::redirect(format!(
            "/{prefix}/sites?error={}",
            encode_uri_component("请先选择至少一个站点")
        )));
    }
    if !matches!(action.as_str(), "start" | "stop" | "delete") {
        return Err(AppError::BadRequest(format!("未知批量操作：{action}")));
    }
    let ip = handlers::client_ip(&state, &parts);
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for domain in &domains {
        let result = match action.as_str() {
            "start" => state.set_site_running(domain, true).await.map_err(map_err),
            "stop" => state.set_site_running(domain, false).await.map_err(map_err),
            _ => state.delete_site(domain).await.map_err(map_err),
        };
        match result {
            Ok(()) => {
                ok += 1;
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    &format!("sites.batch.{action}"),
                    domain,
                    "ok",
                    "",
                );
            }
            Err(e) => {
                failed.push(format!("{domain}: {e}"));
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    &format!("sites.batch.{action}"),
                    domain,
                    "failed",
                    &format!("{e}"),
                );
            }
        }
    }
    if failed.is_empty() {
        Ok(handlers::redirect(format!(
            "/{prefix}/sites?notice={}",
            encode_uri_component(&format!("批量操作完成：{ok} 个站点"))
        )))
    } else {
        Ok(handlers::redirect(format!(
            "/{prefix}/sites?error={}",
            encode_uri_component(&format!(
                "成功 {ok} 个，失败 {} 个：{}",
                failed.len(),
                failed.join("；")
            ))
        )))
    }
}

pub async fn logs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let keyword = q.get("q").cloned().unwrap_or_default();
    let (access, error_log) = state.site_logs(&domain, 200, &keyword).map_err(map_err)?;
    let log_dir = state
        .db()
        .site_by_domain(&domain)?
        .map(|s| {
            skiff_app::sites::site_log_dir(FsPath::new(&s.root_dir))
                .display()
                .to_string()
        })
        .unwrap_or_default();
    let notice = if q.contains_key("created") {
        "站点已创建；下面是站点探针首次访问日志（若为空，请用访问入口打开一次）。"
    } else {
        ""
    };
    handlers::render(SiteLogsPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        domain,
        log_dir,
        access_log: access,
        error_log,
        keyword,
        notice: notice.to_string(),
    })
}

pub async fn delete_confirm(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let site = state
        .db()
        .site_by_domain(&domain)?
        .ok_or_else(|| AppError::NotFound(format!("站点不存在：{domain}")))?;
    handlers::render(SiteDeletePage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        domain: site.domain,
        root_dir: site.root_dir,
    })
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    state.delete_site(&domain).await.map_err(map_err)?;
    let ip = handlers::client_ip(&state, &parts);
    let _ = state
        .db()
        .audit(&user.user.username, &ip, "sites.delete", &domain, "ok", "");
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!("/{prefix}/sites")))
}
