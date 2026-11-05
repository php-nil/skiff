//! 内核「网站」页面：站点列表、运行环境、环境设置、日志与操作。

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_store::{Runtime, Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::state::sites::{NewSite, SiteEnvStatus, SiteError};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{
    BoundSiteView, DirEntryView, DirListingView, InstallStepView, PhpConfigView, RuntimeDetailPage,
    RuntimeOptionView, RuntimeRow, SiteDeletePage, SiteDetailPage, SiteEnvPage, SiteEnvView,
    SiteLogsPage, SiteNewForm, SiteNewPage, SiteSettingsPage, SiteTlsView, SiteView, SitesPage,
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

fn type_label(t: &str) -> &'static str {
    match t {
        TYPE_STATIC => "静态网站",
        TYPE_PROXY => "反向代理",
        _ => "运行环境",
    }
}

fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "运行中",
        "stopped" => "已停止",
        "installing" => "安装中",
        "error" => "异常",
        "missing" => "容器缺失",
        "unimplemented" => "P2 未实现",
        _ => "未知",
    }
}

fn state_class(state: &str) -> &'static str {
    match state {
        "running" => "ok",
        "error" => "danger",
        "missing" => "warn",
        "stopped" => "muted",
        _ => "muted",
    }
}

fn runtime_row(state: &AppState, v: &skiff_app::sites::RuntimeView) -> RuntimeRow {
    let prefix = &state.cfg.entry_prefix;
    let kind_label = match v.runtime.kind.as_str() {
        skiff_store::KIND_NODE => "Node.js",
        skiff_store::KIND_GO => "Go",
        _ => "PHP",
    };
    let endpoint = match v.runtime.kind.as_str() {
        skiff_store::KIND_PHP => {
            if v.socket_host.is_empty() {
                "socket 未登记".to_string()
            } else {
                format!("socket {}", v.socket_host)
            }
        }
        _ => "端口：P2 按站点独立分配".to_string(),
    };
    RuntimeRow {
        id: v.runtime.id.clone(),
        kind_label: kind_label.into(),
        version: v.runtime.version.clone(),
        image: v.runtime.image.clone(),
        state_label: if v.drift {
            "状态漂移".to_string()
        } else {
            state_label(&v.effective_state).to_string()
        },
        state_class: if v.drift {
            "warn".to_string()
        } else {
            state_class(&v.effective_state).to_string()
        },
        drift: v.drift,
        site_count: v.site_count,
        container_state: v.container_state.clone(),
        endpoint,
        implemented: v.implemented,
        is_default: v.runtime.is_default,
        detail_href: format!(
            "/{prefix}/sites/env/{}",
            encode_uri_component(&v.runtime.id)
        ),
        docker_href: format!(
            "/{prefix}/containers/{}",
            encode_uri_component(&v.container_name)
        ),
    }
}

fn site_view(
    s: &Site,
    aliases: Vec<String>,
    cert_ready: bool,
    runtime: Option<&Runtime>,
) -> SiteView {
    let runtime_label = runtime
        .map(|r| {
            format!(
                "{} {}",
                match r.kind.as_str() {
                    skiff_store::KIND_NODE => "Node.js",
                    skiff_store::KIND_GO => "Go",
                    _ => "PHP",
                },
                r.version
            )
        })
        .unwrap_or_default();
    let endpoint = match s.site_type.as_str() {
        TYPE_PROXY => {
            let mut t = s.proxy_target.clone().unwrap_or_default();
            if s.proxy_websocket {
                t.push_str(" · WebSocket");
            }
            t
        }
        TYPE_STATIC => "静态文档根".to_string(),
        _ => runtime_label.clone(),
    };
    SiteView {
        domain: s.domain.clone(),
        // 站点目录本身（整站备份单元）与文档根都展示出来，避免把 `index/` 说成整站。
        index_dir: skiff_app::sites::site_index_dir(FsPath::new(&s.root_dir))
            .display()
            .to_string(),
        root_dir: s.root_dir.clone(),
        site_type: s.site_type.clone(),
        type_label: type_label(&s.site_type).to_string(),
        runtime_id: s.runtime_id.clone().unwrap_or_default(),
        runtime_label,
        proxy_target: s.proxy_target.clone().unwrap_or_default(),
        proxy_websocket: s.proxy_websocket,
        endpoint,
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
    let runtimes = state.db().list_runtimes().unwrap_or_default();
    let views: Vec<SiteView> = sites
        .iter()
        .map(|s| {
            let ready = cert_settings.cert_file(FsPath::new(&s.root_dir)).is_file()
                && cert_settings.key_file(FsPath::new(&s.root_dir)).is_file();
            let runtime = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtimes.iter().find(|r| r.id == id));
            site_view(s, state.site_aliases(&s.domain), ready, runtime)
        })
        .filter(|v| {
            needle.is_empty()
                || v.domain.to_lowercase().contains(&needle)
                || v.root_dir.to_lowercase().contains(&needle)
                || v.endpoint.to_lowercase().contains(&needle)
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
        site_type: form_first(form, "site_type").trim().to_string(),
        domain: form_first(form, "domain").trim().to_string(),
        aliases: form_first(form, "aliases").trim().to_string(),
        root_dir: form_first(form, "root_dir").trim().to_string(),
        runtime_id: form_first(form, "runtime_id").trim().to_string(),
        proxy_target: form_first(form, "proxy_target").trim().to_string(),
        proxy_websocket: matches!(
            form_first(form, "proxy_websocket").trim(),
            "1" | "on" | "true" | "yes"
        ),
    }
}

/// 运行环境选项：P1 只放行 PHP；Node/Go 以禁用态出现并说明 P2。
fn runtime_options(views: &[skiff_app::sites::RuntimeView]) -> Vec<RuntimeOptionView> {
    let mut out: Vec<RuntimeOptionView> = views
        .iter()
        .map(|v| {
            let kind = match v.runtime.kind.as_str() {
                skiff_store::KIND_NODE => "Node.js",
                skiff_store::KIND_GO => "Go",
                _ => "PHP",
            };
            let ready = v.implemented && v.effective_state == "running";
            RuntimeOptionView {
                id: v.runtime.id.clone(),
                label: format!("{kind} {}", v.runtime.version),
                disabled: !v.implemented || !ready,
                disabled_reason: if !v.implemented {
                    "P2 未实现：P1 只支持 PHP 运行环境".into()
                } else if !ready {
                    format!(
                        "运行环境未运行（当前：{}）",
                        state_label(&v.effective_state)
                    )
                } else {
                    String::new()
                },
            }
        })
        .collect();
    // 固定展示 P2 预留项，避免用户以为界面漏做了 Node/Go。
    for (id, version) in [("node-22", "22"), ("go-1.24", "1.24")] {
        let kind = if id.starts_with("node") { "node" } else { "go" };
        out.push(RuntimeOptionView {
            id: id.into(),
            label: format!(
                "{} {}",
                if kind == "node" { "Node.js" } else { "Go" },
                version
            ),
            disabled: true,
            disabled_reason: "P2 未实现：P1 不提供该运行时的安装与运行".into(),
        });
    }
    out
}

fn new_page(
    state: &AppState,
    user: &crate::state::CurrentUser,
    env: &SiteEnvStatus,
    runtimes: Vec<RuntimeOptionView>,
    form: SiteNewForm,
    error: String,
) -> Result<Response, AppError> {
    let error_field = if error.contains("别名") {
        "aliases"
    } else if error.contains("根目录") {
        "root_dir"
    } else if error.contains("上游") {
        "proxy_target"
    } else if error.contains("运行环境") || error.contains("socket") {
        "runtime_id"
    } else if error.contains("域名") || error.contains("站点已存在") {
        "domain"
    } else {
        ""
    };
    let wizard_start = if form.site_type.is_empty() {
        1
    } else if !error.is_empty() {
        // 所有创建校验都发生在第二步（类型已选定）。
        2
    } else {
        3
    };
    handlers::render(SiteNewPage {
        ctx: handlers::page_ctx(state, user, "core/sites/list"),
        env: env_view(env),
        form,
        runtimes,
        error,
        error_field: error_field.to_string(),
        wizard_start,
    })
}

pub async fn new_page_get(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let views = state.runtime_views().await.unwrap_or_default();
    let requested = q.get("type").cloned().unwrap_or_default();
    let form = SiteNewForm {
        site_type: if matches!(requested.as_str(), TYPE_RUNTIME | TYPE_STATIC | TYPE_PROXY) {
            requested
        } else {
            String::new()
        },
        runtime_id: "php-8.5".to_string(),
        ..Default::default()
    };
    new_page(
        &state,
        &user,
        &env,
        runtime_options(&views),
        form,
        String::new(),
    )
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
        site_type: values.site_type.clone(),
        runtime_id: Some(values.runtime_id.clone()).filter(|v| !v.is_empty()),
        proxy_target: values.proxy_target.clone(),
        proxy_websocket: values.proxy_websocket,
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
                SiteError::BadRequest(m) | SiteError::Conflict(m) | SiteError::Unavailable(m) => {
                    m.clone()
                }
                SiteError::Internal(_) => return Err(map_err(e)),
            };
            let env = state.site_env_status().await;
            let views = state.runtime_views().await.unwrap_or_default();
            new_page(
                &state,
                &user,
                &env,
                runtime_options(&views),
                values,
                message,
            )
        }
    }
}

pub async fn env_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let progress = state.site_install_progress().await;
    let settings = state.site_settings();
    let views = state.runtime_views().await.unwrap_or_default();
    let runtimes = views.iter().map(|v| runtime_row(&state, v)).collect();
    handlers::render(SiteEnvPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        env: env_view(&env),
        runtimes,
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
    let runtime = site
        .runtime_id
        .as_deref()
        .and_then(|id| state.db().runtime_by_id(id).ok().flatten());
    handlers::render(SiteDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        site: site_view(
            &site,
            state.site_aliases(&domain),
            cert_ready,
            runtime.as_ref(),
        ),
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

// ---------------------------------------------------------------------------
// 运行环境详情 / 操作 / PHP 专属配置
// ---------------------------------------------------------------------------

pub async fn runtime_detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let view = state.runtime_view(&id).await.map_err(map_err)?;
    let prefix = &state.cfg.entry_prefix;
    let config = PhpConfigView::from(&skiff_app::sites::PhpRuntimeConfig::from_json(
        &view.runtime.config_json,
    ));
    let bound_sites = state
        .db()
        .list_sites()?
        .into_iter()
        .filter(|s| s.runtime_id.as_deref() == Some(id.as_str()))
        .map(|s| BoundSiteView {
            href: format!("/{prefix}/sites/{}", encode_uri_component(&s.domain)),
            domain: s.domain,
            type_label: type_label(&s.site_type).to_string(),
            status_label: state_label(&s.status).to_string(),
        })
        .collect();
    let (logs, logs_error) = if q.get("tab").map(String::as_str) == Some("logs") {
        match state.runtime_logs(&id, 300).await {
            Ok(text) => (text, String::new()),
            Err(e) => (String::new(), format!("{e}")),
        }
    } else {
        (String::new(), String::new())
    };
    handlers::render(RuntimeDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        runtime: runtime_row(&state, &view),
        config,
        bound_sites,
        socket_host: view.socket_host.clone(),
        socket_container: view.socket_container.clone(),
        socket_exists: view.socket_exists,
        socket_mode: view.socket_mode.clone(),
        extensions: ["opcache", "mbstring", "pdo_mysql", "curl", "gd", "redis"]
            .into_iter()
            .map(|name| crate::templates::ExtensionView {
                name: name.into(),
                enabled: matches!(name, "opcache" | "mbstring" | "pdo_mysql" | "curl"),
            })
            .collect(),
        logs,
        logs_error,
        can_admin: user.can(skiff_model::Permission::Admin),
        error: q.get("error").cloned().unwrap_or_default(),
        notice: q.get("notice").cloned().unwrap_or_default(),
    })
}

fn parse_u32_field(
    form: &HashMap<String, Vec<String>>,
    key: &str,
    default: u32,
) -> Result<u32, AppError> {
    let raw = form_first(form, key);
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(default);
    }
    raw.parse::<u32>()
        .map_err(|_| AppError::BadRequest(format!("{key} 必须是整数")))
}

pub async fn runtime_action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    if !matches!(
        action.as_str(),
        "start" | "stop" | "restart" | "rebuild" | "sync" | "delete"
    ) {
        return Err(AppError::BadRequest(format!("未知运行环境操作：{action}")));
    }
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    if action == "delete" {
        return match state.delete_site_runtime(&id) {
            Ok(()) => {
                let _ =
                    state
                        .db()
                        .audit(&user.user.username, &ip, "runtimes.delete", &id, "ok", "");
                Ok(handlers::redirect(format!(
                    "/{prefix}/sites/env?notice={}",
                    encode_uri_component("运行环境已删除")
                )))
            }
            Err(e) => {
                let msg = format!("{e}");
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    "runtimes.delete",
                    &id,
                    "failed",
                    &msg,
                );
                Ok(handlers::redirect(format!(
                    "/{prefix}/sites/env?error={}",
                    encode_uri_component(&msg)
                )))
            }
        };
    }
    match state.runtime_action(&id, &action).await {
        Ok(_) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &format!("runtimes.{action}"),
                &id,
                "ok",
                "",
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component("操作已完成")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                &format!("runtimes.{action}"),
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?error={}",
                encode_uri_component(&id),
                encode_uri_component(&msg)
            )))
        }
    }
}

pub async fn runtime_config_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let default = skiff_app::sites::PhpRuntimeConfig::default();
    let cfg = skiff_app::sites::PhpRuntimeConfig {
        memory_limit: form_first(&form, "memory_limit").trim().to_string(),
        upload_max_filesize: form_first(&form, "upload_max_filesize").trim().to_string(),
        post_max_size: form_first(&form, "post_max_size").trim().to_string(),
        max_execution_time: parse_u32_field(&form, "max_execution_time", 30)? as u64,
        pm: form_first(&form, "pm").trim().to_string(),
        max_children: parse_u32_field(&form, "max_children", 20)?,
        start_servers: parse_u32_field(&form, "start_servers", 3)?,
        min_spare_servers: parse_u32_field(&form, "min_spare_servers", 2)?,
        max_spare_servers: parse_u32_field(&form, "max_spare_servers", 8)?,
        max_requests: parse_u32_field(&form, "max_requests", 500)?,
    };
    // 缺省字段回落到默认值，避免空表单把合法配置清成非法值。
    let cfg = skiff_app::sites::PhpRuntimeConfig {
        memory_limit: if cfg.memory_limit.is_empty() {
            default.memory_limit.clone()
        } else {
            cfg.memory_limit
        },
        upload_max_filesize: if cfg.upload_max_filesize.is_empty() {
            default.upload_max_filesize.clone()
        } else {
            cfg.upload_max_filesize
        },
        post_max_size: if cfg.post_max_size.is_empty() {
            default.post_max_size.clone()
        } else {
            cfg.post_max_size
        },
        pm: if cfg.pm.is_empty() {
            default.pm.clone()
        } else {
            cfg.pm
        },
        ..cfg
    };
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.save_php_runtime_config(&id, &cfg).await {
        Ok(_) => {
            let _ = state
                .db()
                .audit(&user.user.username, &ip, "runtimes.config", &id, "ok", "");
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component("PHP 配置已保存并重载 FPM")
            )))
        }
        Err(e) => {
            let msg = format!("{e}");
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.config",
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?error={}",
                encode_uri_component(&id),
                encode_uri_component(&msg)
            )))
        }
    }
}
