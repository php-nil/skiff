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
    BoundSiteView, DirEntryView, DirListingView, ExtensionOptionView, PhpConfigView,
    RuntimeDetailPage, RuntimeJobView, RuntimeKindView, RuntimeOptionView, RuntimeRow,
    SiteBackupView, SiteDeletePage, SiteDetailPage, SiteEnvPage, SiteEnvView, SiteLogsPage,
    SiteNewForm, SiteTlsView, SiteView, SitesPage,
};
use crate::util::{encode_uri_component, form_first, human_size, parse_form};

fn env_view(env: &SiteEnvStatus) -> SiteEnvView {
    SiteEnvView {
        docker_available: env.docker_available,
        docker_reason: env.docker_reason.clone(),
        nginx_state: env.nginx_state.clone(),
        installed: env.installed,
        ready: env.ready(),
        port: env.port,
        nginx_image: env.nginx_image.clone(),
        php_image: env.php_image.clone(),
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
        "missing" => "未安装",
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

/// 列表版本列：已知真实版本优先；容器缺失 → 目标 + 「未安装」；
/// 容器在但元数据读不到 → 目标 + 「版本未读取」。停止的容器仍展示真实版本。
pub(crate) fn version_display(
    actual: Option<&str>,
    target: &str,
    image: &str,
    container_exists: bool,
) -> (String, String) {
    match actual {
        Some(actual) => (actual.to_string(), format!("目标 {target} · {image}")),
        None if !container_exists => (target.to_string(), "未安装".into()),
        None => (target.to_string(), "版本未读取".into()),
    }
}

fn runtime_row(state: &AppState, v: &skiff_app::sites::RuntimeView) -> RuntimeRow {
    let prefix = &state.cfg.entry_prefix;
    let kind_label = skiff_app::runtime_catalog::kind_label(&v.runtime.kind);
    let endpoint_full = match v.runtime.kind.as_str() {
        skiff_store::KIND_PHP => {
            if v.socket_host.is_empty() {
                "socket 未登记".to_string()
            } else {
                format!("socket {}", v.socket_host)
            }
        }
        skiff_store::KIND_RUST => match v.runtime.port {
            Some(port) => format!("http://127.0.0.1:{port}"),
            None => "端口未分配".to_string(),
        },
        _ => "端口：P2 按站点独立分配".to_string(),
    };
    let endpoint_short = match v.runtime.kind.as_str() {
        skiff_store::KIND_PHP => "sock".to_string(),
        skiff_store::KIND_RUST => v
            .runtime
            .port
            .map(|p| format!(":{p}"))
            .unwrap_or_else(|| "端口".into()),
        _ => "P2".to_string(),
    };
    // 已知真实版本（镜像元数据）优先；容器缺失/未安装 → 目标 + 未安装；
    // 容器在但元数据读不到 → 目标 + 版本未读取。停止的容器仍展示真实版本。
    let target_version = v.runtime.version.clone();
    let (version_display, version_note) = version_display(
        v.actual_version.as_deref(),
        &target_version,
        &v.runtime.image,
        v.container_exists,
    );
    let image_short = |id: &str| -> String {
        let raw = id.strip_prefix("sha256:").unwrap_or(id);
        raw.chars().take(12).collect()
    };
    let job = v.job.clone();
    let job_active = job.as_ref().is_some_and(|j| j.active());
    let job_failed = job
        .as_ref()
        .is_some_and(|j| j.state == skiff_app::sites::RuntimeJobState::Failed);
    RuntimeRow {
        id: v.runtime.id.clone(),
        kind_key: v.runtime.kind.clone(),
        kind_label,
        name: v.runtime.name.clone(),
        container_name: v.container_name.clone(),
        version: v.runtime.version.clone(),
        version_display,
        version_note,
        actual_version: v.actual_version.clone().unwrap_or_default(),
        container_exists: v.container_exists,
        image: v.runtime.image.clone(),
        image_id: image_short(&v.image_id),
        image_id_full: v.image_id.clone(),
        image_digest: image_short(&v.image_digest),
        image_digest_full: v.image_digest.clone(),
        image_mismatch: v.image_mismatch,
        state_label: if let Some(j) = job.as_ref().filter(|j| j.active()) {
            // 安装中优先显示任务阶段，避免「未安装」与正在拉镜像互相矛盾。
            format!("{}：{}", j.state.label(), j.step)
        } else if v.drift {
            "状态漂移".to_string()
        } else if job_failed && v.container_state != "running" {
            "安装失败".to_string()
        } else {
            state_label(&v.effective_state).to_string()
        },
        state_class: if job_active || v.drift || job_failed {
            "warn".to_string()
        } else {
            state_class(&v.effective_state).to_string()
        },
        drift: v.drift,
        site_count: v.site_count,
        container_state: v.container_state.clone(),
        endpoint_short,
        endpoint_full,
        port: v.runtime.port.map(|p| p.to_string()).unwrap_or_default(),
        implemented: v.implemented,
        is_default: v.runtime.is_default,
        is_web: false,
        detail_href: format!(
            "/{prefix}/sites/env/{}",
            encode_uri_component(&v.runtime.id)
        ),
        docker_href: format!(
            "/{prefix}/containers/{}",
            encode_uri_component(&v.container_name)
        ),
        job_progress: job.as_ref().map(|j| j.progress).unwrap_or(0),
        job_error: job.as_ref().map(|j| j.error.clone()).unwrap_or_default(),
        job_active,
    }
}

/// 独立 Web 服务（OpenResty）合成行：不落库、不打开实例抽屉。
///
/// 与 PHP/Node/Go/Rust 行完全平级：各自安装、各自状态、各自删除。
fn web_runtime_row(state: &AppState, env: &SiteEnvStatus, site_count: usize) -> RuntimeRow {
    let prefix = &state.cfg.entry_prefix;
    let exists = env.nginx_state != "missing";
    let short = |id: &str| -> String {
        let raw = id.strip_prefix("sha256:").unwrap_or(id);
        raw.chars().take(12).collect()
    };
    RuntimeRow {
        id: "web".into(),
        kind_key: "web".into(),
        kind_label: "Web 服务（OpenResty）".into(),
        name: "openresty".into(),
        container_name: skiff_app::sites::NGINX_CONTAINER.to_string(),
        version: "openresty".into(),
        version_display: "OpenResty".into(),
        version_note: format!("镜像 {}", env.nginx_image),
        actual_version: env.nginx_image_id.clone(),
        container_exists: exists,
        image: env.nginx_image.clone(),
        image_id: short(&env.nginx_image_id),
        image_id_full: env.nginx_image_id.clone(),
        image_digest: String::new(),
        image_digest_full: String::new(),
        image_mismatch: false,
        state_label: if exists {
            state_label(&env.nginx_state).to_string()
        } else {
            "未安装".into()
        },
        state_class: if !exists {
            "warn".into()
        } else {
            state_class(&env.nginx_state).to_string()
        },
        drift: false,
        site_count,
        container_state: env.nginx_state.clone(),
        endpoint_short: ":80".into(),
        endpoint_full: "宿主 80 端口（host 网络）".into(),
        port: "80".into(),
        implemented: true,
        is_default: false,
        is_web: true,
        detail_href: String::new(),
        docker_href: format!(
            "/{prefix}/containers/{}",
            encode_uri_component(skiff_app::sites::NGINX_CONTAINER)
        ),
        job_progress: 0,
        job_error: String::new(),
        job_active: false,
    }
}

fn job_view(job: &skiff_app::sites::RuntimeJob) -> RuntimeJobView {
    RuntimeJobView {
        state: job.state.as_str().to_string(),
        state_label: job.state.label().to_string(),
        step: job.step.clone(),
        progress: job.progress,
        error: job.error.clone(),
        log: job.log_text(),
        active: job.active(),
        failed: job.state == skiff_app::sites::RuntimeJobState::Failed,
        kind: job.kind.clone(),
        ext_keys: job.ext_keys.clone(),
    }
}

fn job_json(job: &skiff_app::sites::RuntimeJob) -> serde_json::Value {
    serde_json::json!({
        "id": job.runtime_id,
        "state": job.state.as_str(),
        "state_label": job.state.label(),
        "step": job.step,
        "progress": job.progress,
        "error": job.error,
        "log": job.log_text(),
        "active": job.active(),
        "failed": job.state == skiff_app::sites::RuntimeJobState::Failed,
        "kind": job.kind,
        "ext_keys": job.ext_keys,
        "started_at": job.started_at,
        "updated_at": job.updated_at,
    })
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
                    skiff_store::KIND_RUST => "Rust",
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
    // 列表合并列展示实例名（php-8.5），不拼「PHP · php-8.5」这类冗余；记录缺失时留空由模板兜底。
    let runtime_name = runtime.map(|r| r.name.clone()).unwrap_or_default();
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
        runtime_name,
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
        status_class: match s.status.as_str() {
            "running" => "ok",
            "stopped" => "",
            _ => "danger",
        }
        .into(),
        https_label: match (s.https, cert_ready) {
            (true, true) => "已启用 HTTPS",
            (true, false) => "证书缺失",
            (false, true) => "证书已上传",
            (false, false) => "未配置 HTTPS",
        }
        .into(),
        https_class: match (s.https, cert_ready) {
            (true, true) => "ok",
            (true, false) => "danger",
            (false, true) => "info",
            (false, false) => "",
        }
        .into(),
        created: chrono::DateTime::from_timestamp(s.created_at, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        is_default: s.is_default,
        aliases_text: aliases.join("、"),
        alias_preview: aliases
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join("、"),
        alias_more: aliases.len().saturating_sub(2),
        aliases,
        https: s.https,
        force_https: s.force_https,
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

/// 可回填到弹窗的用户侧错误文案；内部错误返回 `None`，交给错误页。
fn site_error_text(e: &SiteError) -> Option<String> {
    match e {
        SiteError::BadRequest(m) | SiteError::Conflict(m) | SiteError::Unavailable(m) => {
            Some(m.clone())
        }
        SiteError::Internal(_) => None,
    }
}

pub(crate) fn require_admin(user: &crate::state::CurrentUser) -> Result<(), AppError> {
    handlers::require_permission(user, skiff_model::Permission::Admin)
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let open = q.contains_key("new") || q.contains_key("type");
    render_sites(&state, &user, &q, default_new_form(&q), String::new(), open).await
}

/// 列表页与新建弹窗共用一次渲染：校验失败时把表单值与错误一并带回，
/// 弹窗保持展开，避免「错误页 + 重新填写」的往返。
async fn render_sites(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    q: &HashMap<String, String>,
    form: SiteNewForm,
    form_error: String,
    form_open: bool,
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
    let runtime_views = state.runtime_views().await.unwrap_or_default();
    let settings = state.site_settings();
    let form_error_field = error_field(&form_error, &form.site_type).to_string();
    handlers::render(SitesPage {
        ctx: handlers::page_ctx(state, user, "core/sites/list"),
        sites: views,
        env: env_view(&env),
        can_admin: user.can(skiff_model::Permission::Admin),
        error,
        notice,
        keyword,
        form,
        runtimes: runtime_options(&runtime_views),
        form_open,
        form_error: form_error.clone(),
        form_error_field,
        sites_prefix: format!("{}/", settings.container_sites_dir()),
    })
}

/// 弹窗默认值：类型默认运行环境，PHP 版本预选已安装项；目录留空由服务端按主域名补全。
fn default_new_form(q: &HashMap<String, String>) -> SiteNewForm {
    let requested = q.get("type").map(String::as_str).unwrap_or_default();
    let site_type = match requested {
        TYPE_STATIC | TYPE_PROXY => requested.to_string(),
        _ => TYPE_RUNTIME.to_string(),
    };
    SiteNewForm {
        site_type,
        runtime_id: "php-8.5".into(),
        ..Default::default()
    }
}

/// 服务端错误落到具体字段，前端只做高亮，不重复解释前缀规则。
///
/// 两类错误回到顶部提示，避免落到当前类型下不可见的字段：
/// - 环境级错误（nginx/Docker 未就绪）不属于任何输入；
/// - 类型专属字段与当前选中的类型不匹配（如静态站报运行环境错误）。
fn error_field(error: &str, site_type: &str) -> &'static str {
    let field = if (error.contains("未就绪")
        && (error.contains("OpenResty") || error.contains("nginx")))
        || error.contains("Docker 不可用")
    {
        ""
    } else if error.contains("别名") || error.contains("域名重复") || error.contains("域名已被")
    {
        "aliases"
    } else if error.contains("上游") {
        "proxy_target"
    } else if error.contains("运行环境") || error.contains("FPM") || error.contains("socket") {
        "runtime_id"
    } else if error.contains("站点目录") || error.contains("目录") {
        "root_dir"
    } else if error.contains("域名") || error.contains("站点已存在") {
        "domain"
    } else {
        ""
    };
    if (field == "runtime_id" && site_type != TYPE_RUNTIME)
        || (field == "proxy_target" && site_type != TYPE_PROXY)
    {
        return "";
    }
    field
}

fn new_form(form: &HashMap<String, Vec<String>>) -> SiteNewForm {
    SiteNewForm {
        site_type: form_first(form, "site_type").trim().to_string(),
        domain: form_first(form, "domain").trim().to_string(),
        aliases: form
            .get("aliases")
            .map(|rows| {
                rows.iter()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        aliases_bulk: form_first(form, "aliases_bulk").trim().to_string(),
        root_dir: form_first(form, "root_dir").trim().to_string(),
        runtime_id: form_first(form, "runtime_id").trim().to_string(),
        proxy_target: form_first(form, "proxy_target").trim().to_string(),
        proxy_websocket: matches!(
            form_first(form, "proxy_websocket").trim(),
            "1" | "on" | "true" | "yes"
        ),
    }
}

/// 用户只填目录名；这里容错剥掉 `/www/sites/` 或宿主机前缀，避免把整段路径当目录名。
fn root_slug(raw: &str, sites_dir: &FsPath) -> Option<String> {
    let value = raw.trim().replace('\\', "/");
    if value.is_empty() {
        return None;
    }
    let value = value.trim_start_matches('/').to_string();
    for prefix in ["www/sites/", "sites/"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return Some(rest.trim_start_matches('/').to_string());
        }
    }
    let host = sites_dir.to_string_lossy().replace('\\', "/");
    let host = host.trim_start_matches('/');
    if let Some(rest) = value.strip_prefix(&format!("{host}/")) {
        return Some(rest.to_string());
    }
    Some(value)
}

fn collect_aliases(form: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = form.get("aliases").cloned().unwrap_or_default();
    if let Some(bulk) = form.get("aliases_bulk") {
        out.extend(bulk.iter().cloned());
    }
    out
}

/// 运行环境选项：label 与类型目录共用 `runtime_catalog`，避免漏映射新类型（如 Rust 被标成 PHP）。
fn runtime_options(views: &[skiff_app::sites::RuntimeView]) -> Vec<RuntimeOptionView> {
    let mut out: Vec<RuntimeOptionView> = views
        .iter()
        .map(|v| {
            let kind = skiff_app::runtime_catalog::kind_label(&v.runtime.kind);
            let ready = v.implemented && v.effective_state == "running";
            RuntimeOptionView {
                id: v.runtime.id.clone(),
                label: format!("{kind} {}", v.runtime.version),
                disabled: !v.implemented || !ready,
                disabled_reason: if !v.implemented {
                    "P2 未实现：当前仅提供 PHP / Rust 运行环境".into()
                } else if v.effective_state == "missing" {
                    "未安装，可到运行环境页安装".into()
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

/// 新建入口不再单独成页：跳到列表并直接展开弹窗（带类型时预选）。
pub async fn new_page_get(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let prefix = &state.cfg.entry_prefix;
    let mut url = format!("/{prefix}/sites?new=1");
    if let Some(t) = q.get("type") {
        if matches!(t.as_str(), TYPE_RUNTIME | TYPE_STATIC | TYPE_PROXY) {
            url.push_str(&format!("&type={t}"));
        }
    }
    Ok(handlers::redirect(url))
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
    let settings = state.site_settings();
    let prepared = skiff_app::sites::validate_domain(&values.domain).and_then(|domain| {
        let slug = root_slug(&values.root_dir, &settings.sites_dir);
        skiff_app::sites::site_root_for(&settings.sites_dir, &domain, slug.as_deref())
            .map(|root| (domain, root))
    });
    let (domain, root_dir) = match prepared {
        Ok(v) => v,
        Err(e) => match site_error_text(&e) {
            Some(message) => {
                return render_sites(&state, &user, &HashMap::new(), values, message, true).await
            }
            None => return Err(map_err(e)),
        },
    };
    let req = NewSite {
        domain,
        root_dir: Some(root_dir.display().to_string()),
        site_type: values.site_type.clone(),
        runtime_id: Some(values.runtime_id.clone()).filter(|v| !v.is_empty()),
        proxy_target: values.proxy_target.clone(),
        proxy_websocket: values.proxy_websocket,
        aliases: collect_aliases(&form),
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
            let message = match site_error_text(&e) {
                Some(m) => m,
                None => return Err(map_err(e)),
            };
            // 校验失败仍留在列表页，弹窗保持展开并回填，不做二次确认。
            render_sites(&state, &user, &HashMap::new(), values, message, true).await
        }
    }
}

fn runtime_kind_views() -> Vec<RuntimeKindView> {
    skiff_app::runtime_catalog::RUNTIME_KINDS
        .iter()
        .map(|k| RuntimeKindView {
            key: k.kind.to_string(),
            label: k.label.to_string(),
            versions: k.versions.iter().map(|v| v.to_string()).collect(),
            implemented: skiff_app::runtime_catalog::is_implemented(k.kind),
        })
        .collect()
}

fn extension_options(enabled: &[String], version: &str, image: &str) -> Vec<ExtensionOptionView> {
    let base = skiff_app::runtime_catalog::base_image_of(image);
    let mut out: Vec<ExtensionOptionView> =
        skiff_app::runtime_catalog::extensions_for(version, base)
            .into_iter()
            .map(|entry| {
                let e = entry.def;
                // 不可用项已在 `extensions_for` 过滤，展示层不再灰显/占位；
                // 这里只保留依赖提示（Ok → 空）。
                let support_note = match &entry.support {
                    skiff_app::runtime_catalog::ExtSupport::NeedsDeps(deps) => {
                        format!("安装时将临时安装 {} 并在完成后清理", deps.join(" / "))
                    }
                    _ => String::new(),
                };
                ExtensionOptionView {
                    key: e.key.to_string(),
                    label: e.label.to_string(),
                    description: e.description.to_string(),
                    // 镜像内置项始终视为已启用（无需再编译）。
                    enabled: e.builtin || enabled.iter().any(|k| k == e.key),
                    default_on: e.default_on,
                    builtin: e.builtin,
                    category: e.category.to_string(),
                    source: e.source.label().to_string(),
                    hot: e.hot,
                    heavy: skiff_app::runtime_catalog::extension_is_heavy(e.key),
                    estimated_time: skiff_app::runtime_catalog::extension_estimated_time(e.key)
                        .to_string(),
                    support_note,
                }
            })
            .collect();
    // 运行环境里按名字装过的 PECL 扩展也要出现在清单里（否则无法卸载）。
    for token in enabled {
        if skiff_app::runtime_catalog::php_extension(token).is_none()
            && skiff_app::runtime_catalog::parse_pecl_token(token).is_some()
        {
            out.push(ExtensionOptionView {
                key: token.clone(),
                label: skiff_app::runtime_catalog::extension_label(token),
                description: "通过 PECL 按名字安装".to_string(),
                enabled: true,
                default_on: false,
                builtin: false,
                category: skiff_app::runtime_catalog::CATEGORY_OTHER.to_string(),
                source: skiff_app::runtime_catalog::ExtSource::Pecl
                    .label()
                    .to_string(),
                hot: true,
                heavy: skiff_app::runtime_catalog::extension_is_heavy(token),
                estimated_time: skiff_app::runtime_catalog::extension_estimated_time(token)
                    .to_string(),
                support_note: String::new(),
            });
        }
    }
    out
}

pub async fn env_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let views = state.runtime_views().await.unwrap_or_default();
    let site_total = state.db().list_sites().map(|s| s.len()).unwrap_or(0);
    let mut runtimes: Vec<RuntimeRow> = Vec::with_capacity(views.len() + 1);
    runtimes.push(web_runtime_row(&state, &env, site_total));
    runtimes.extend(views.iter().map(|v| runtime_row(&state, v)));
    let prefix = &state.cfg.entry_prefix;
    let kinds = runtime_kind_views();
    // 新建弹窗切换 PHP 版本时，扩展勾选项要按该版本重算可用性；
    // 数据与抽屉共用 `extension_options`（`runtime_catalog` 单一来源）。
    let mut exts_by_version = serde_json::Map::new();
    if let Some(def) = skiff_app::runtime_catalog::RUNTIME_KINDS
        .iter()
        .find(|k| k.kind == "php")
    {
        for ver in def.versions {
            let opts = extension_options(
                &skiff_app::runtime_catalog::default_extensions(),
                ver,
                &env.php_image,
            );
            let arr: Vec<serde_json::Value> = opts
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "key": e.key,
                        "label": e.label,
                        "builtin": e.builtin,
                        "default_on": e.default_on,
                        "heavy": e.heavy,
                        "estimated_time": e.estimated_time,
                        "note": e.support_note,
                    })
                })
                .collect();
            exts_by_version.insert(ver.to_string(), serde_json::Value::Array(arr));
        }
    }
    let exts_json = serde_json::Value::Object(exts_by_version);
    let catalog_json = serde_json::to_string(
        &kinds
            .iter()
            .map(|k| {
                let mut v = serde_json::json!({
                    "key": k.key,
                    "label": k.label,
                    "versions": k.versions,
                    "implemented": k.implemented,
                });
                if k.key == "php" {
                    v["exts_by_version"] = exts_json.clone();
                }
                v
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    handlers::render(SiteEnvPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        env: env_view(&env),
        runtimes,
        can_admin: user.can(skiff_model::Permission::Admin),
        kinds,
        catalog_json,
        extensions: extension_options(
            &skiff_app::runtime_catalog::default_extensions(),
            skiff_app::sites::DEFAULT_PHP_VERSION,
            &env.php_image,
        ),
        jobs_href: format!("/{prefix}/sites/env/jobs"),
        add_href: format!("/{prefix}/sites/env/add"),
        images_href: format!("/{prefix}/sites/env/images"),
        error: q.get("error").cloned().unwrap_or_default(),
        notice: q.get("notice").cloned().unwrap_or_default(),
    })
}

pub async fn env_add(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let kind = form_first(&form, "kind").trim().to_string();
    let kind = if kind.is_empty() {
        skiff_store::KIND_PHP.to_string()
    } else {
        kind
    };
    let name = form_first(&form, "name");
    let version = form_first(&form, "version");
    // 弹窗会带 `ext_form=1`（即使一个都没勾）；没有该标记的旧表单/无 JS 场景
    // 回落到目录默认扩展，避免「没提交 ext」被误判成「全部取消」。
    let selected: Vec<String> = if form.contains_key("ext_form") {
        form.get("ext").cloned().unwrap_or_default()
    } else {
        skiff_app::runtime_catalog::default_extensions()
    };
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.add_runtime(&kind, &name, &version, &selected).await {
        Ok((rt, started)) => {
            let _ = state
                .db()
                .audit(&user.user.username, &ip, "sites.env.add", &rt.id, "ok", "");
            let message = if started {
                format!(
                    "已提交 {} {} 安装任务（实例 {}）；列表会实时显示进度，打开详情抽屉可看安装日志",
                    skiff_app::runtime_catalog::kind_label(&rt.kind),
                    rt.version,
                    rt.name
                )
            } else {
                format!(
                    "已登记 {} {}（实例 {}）；P2 预留类型暂不安装容器",
                    skiff_app::runtime_catalog::kind_label(&rt.kind),
                    rt.version,
                    rt.name
                )
            };
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env?notice={}",
                encode_uri_component(&message)
            )))
        }
        Err(e) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.env.add",
                &name,
                "failed",
                &e.to_string(),
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env?error={}",
                encode_uri_component(&e.to_string())
            )))
        }
    }
}

/// 运行环境页「镜像参数」保存：nginx / PHP 基础镜像标签。
/// 镜像前缀与 Docker 加速源已迁到 Docker 设置，这里只管运行环境自身的镜像名。
pub async fn env_images_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    for (form_key, setting_key) in [
        ("nginx_image", "sites.nginx_image"),
        ("php_image", "sites.php_image"),
    ] {
        if form.contains_key(form_key) {
            let value = form_first(&form, form_key);
            state.db().set_setting(setting_key, value.trim())?;
        }
    }
    let ip = handlers::client_ip(&state, &parts);
    let _ = state
        .db()
        .audit(&user.user.username, &ip, "sites.env.images", "-", "ok", "");
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!(
        "/{prefix}/sites/env?notice={}",
        encode_uri_component("镜像参数已保存，重建 Web 服务或重装对应实例后生效")
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
    Ok(handlers::redirect(format!(
        "/{prefix}/sites/{}?notice={}",
        encode_uri_component(&domain),
        encode_uri_component(if running {
            "站点已启动"
        } else {
            "站点已停止"
        })
    )))
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
    Ok(handlers::redirect(format!(
        "/{prefix}/sites/{}?notice={}",
        encode_uri_component(&domain),
        encode_uri_component(if enabled {
            "已设为默认站点"
        } else {
            "已取消默认站点"
        })
    )))
}

/// 站点详情「域名」标签：新增/删除别名，落库后重建 vhost 并 reload。
pub async fn domains(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action").trim().to_string();
    let alias = form_first(&form, "alias").trim().to_string();
    let result = match action.as_str() {
        "add" => state.add_site_alias(&domain, &alias).await,
        "remove" => state.remove_site_alias(&domain, &alias).await,
        _ => Err(SiteError::BadRequest(format!("未知域名操作：{action}"))),
    };
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    let target = format!(
        "/{prefix}/sites/{}?tab=domains",
        encode_uri_component(&domain)
    );
    match result {
        Ok(name) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                if action == "add" {
                    "sites.domains.add"
                } else {
                    "sites.domains.remove"
                },
                &domain,
                "ok",
                &name,
            );
            let notice = if action == "add" {
                "别名已添加"
            } else {
                "别名已删除"
            };
            Ok(handlers::redirect(format!(
                "{target}&notice={}",
                encode_uri_component(notice)
            )))
        }
        Err(e) => {
            let message = match &e {
                SiteError::BadRequest(m) | SiteError::Conflict(m) | SiteError::Unavailable(m) => {
                    m.clone()
                }
                SiteError::Internal(_) => return Err(map_err(e)),
            };
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                if action == "add" {
                    "sites.domains.add"
                } else {
                    "sites.domains.remove"
                },
                &domain,
                "fail",
                &message,
            );
            Ok(handlers::redirect(format!(
                "{target}&error={}",
                encode_uri_component(&message)
            )))
        }
    }
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
    render_detail(
        state,
        user,
        domain,
        q.get("tab").cloned().unwrap_or_else(|| "overview".into()),
        q.get("error").cloned().unwrap_or_default(),
        q.get("notice").cloned().unwrap_or_default(),
        q.get("path").cloned(),
        None,
    )
    .await
}

/// 详情页渲染。`submitted_config` 非空时用用户刚提交的配置替代磁盘内容，
/// 供保存校验失败后原地回显，避免跳转重读把编辑内容冲掉。
#[allow(clippy::too_many_arguments)]
async fn render_detail(
    state: Arc<AppState>,
    user: crate::state::CurrentUser,
    domain: String,
    mut tab: String,
    mut error: String,
    notice: String,
    dir_path: Option<String>,
    submitted_config: Option<String>,
) -> Result<Response, AppError> {
    let site = state
        .db()
        .site_by_domain(&domain)?
        .ok_or_else(|| AppError::NotFound(format!("站点不存在：{domain}")))?;
    let env = state.site_env_status().await;
    let (runtime_down, runtime_alert, runtime_env_href) = if site.site_type
        == skiff_store::TYPE_RUNTIME
    {
        let rid = site.runtime_id.clone().unwrap_or_default();
        let rt = state.db().runtime_by_id(&rid)?;
        let down = rt
            .as_ref()
            .map(|r| {
                r.last_status != "running"
                    || r.socket_path
                        .as_deref()
                        .map(|p| !std::path::Path::new(p).exists())
                        .unwrap_or(true)
            })
            .unwrap_or(true);
        let name = rt
            .as_ref()
            .map(|r| r.name.clone())
            .unwrap_or_else(|| rid.clone());
        (
            down,
            format!(
                "绑定的运行环境 {name} 当前未运行，访问本站点会返回 502/503。请启动该运行环境；系统不会静默切换到其它版本。"
            ),
            format!(
                "/{}/sites/env/{}",
                state.cfg.entry_prefix,
                encode_uri_component(&rid)
            ),
        )
    } else {
        (false, String::new(), String::new())
    };
    if !matches!(
        tab.as_str(),
        "overview" | "domains" | "config" | "dir" | "ssl"
    ) {
        tab = "overview".to_string();
    }
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
        if let Some(text) = submitted_config {
            config_content = text;
        }
    }
    if tab == "dir" {
        let rel = dir_path.unwrap_or_default();
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
    let backup = backup_view(&state, &domain).await;
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
        runtime_down,
        runtime_alert,
        runtime_env_href,
        backup,
        error,
        notice,
    })
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

/// 站点备份开关的视图。任务服务未初始化（旧库/未接线）时渲染为「不可用」，
/// 不把整页拖成 503——站点页的主要信息仍然可用。
async fn backup_view(state: &AppState, domain: &str) -> SiteBackupView {
    let Some(tasks) = state.tasks() else {
        return SiteBackupView::default();
    };
    let status = tasks.site_backup_status(domain).await;
    SiteBackupView {
        enabled: status.enabled,
        task_id: status.task_id,
        last_text: status
            .last_at
            .map(|at| {
                format!(
                    "{} · {}",
                    fmt_ts(at),
                    status.last_result.as_deref().unwrap_or("-")
                )
            })
            .unwrap_or_else(|| "从未执行".to_string()),
        next_text: status
            .next_at
            .map(fmt_ts)
            .unwrap_or_else(|| "-".to_string()),
    }
}

/// 站点备份开关：直接开关内核 `site-backup` 任务，不跳计划任务页。
pub async fn set_backup(
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
    let svc = state
        .tasks()
        .ok_or_else(|| AppError::Unavailable("内核任务服务未初始化".into()))?;
    let ip = handlers::client_ip(&state, &parts);
    match svc
        .set_site_backup(&domain, enabled, &user.user.username)
        .await
    {
        Ok(_) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.backup",
                &domain,
                "ok",
                if enabled {
                    "开启站点备份"
                } else {
                    "关闭站点备份"
                },
            );
            Ok(handlers::redirect(format!(
                "/{}/sites/{}?notice={}",
                state.cfg.entry_prefix,
                encode_uri_component(&domain),
                encode_uri_component(if enabled {
                    "站点备份已开启（每日 03:00）"
                } else {
                    "站点备份已关闭"
                })
            )))
        }
        Err(e) => Ok(handlers::redirect(format!(
            "/{}/sites/{}?error={}",
            state.cfg.entry_prefix,
            encode_uri_component(&domain),
            encode_uri_component(&e)
        ))),
    }
}

/// 保存站点的成功文案：只有真正重载成功才能说「已重载」。
fn save_config_notice(reloaded: bool) -> &'static str {
    if reloaded {
        "配置已保存、校验通过并已重载 nginx"
    } else {
        "配置已保存并通过校验（站点未运行，未重载）"
    }
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
        Ok(outcome) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "sites.config.save",
                &domain,
                "ok",
                if outcome.reloaded {
                    "已保存并重载 nginx"
                } else {
                    "已保存并通过校验（站点未运行）"
                },
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/{}?tab=config&notice={}",
                encode_uri_component(&domain),
                encode_uri_component(save_config_notice(outcome.reloaded))
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
            // 校验失败原地回显：把本次提交内容带回配置标签页；跳转会重读磁盘，冲掉编辑。
            render_detail(
                state,
                user,
                domain,
                "config".to_string(),
                msg,
                String::new(),
                None,
                Some(content),
            )
            .await
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
    let enabled_exts = view.extensions.clone();
    let job = state.runtime_job(&id).await;
    let (has_job, job_view) = match job.as_ref() {
        Some(j) => (true, job_view(j)),
        None => (false, RuntimeJobView::default()),
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
        extensions: extension_options(&enabled_exts, &view.runtime.version, &view.runtime.image),
        ext_categories: [
            skiff_app::runtime_catalog::CATEGORY_DB,
            skiff_app::runtime_catalog::CATEGORY_IMAGE,
            skiff_app::runtime_catalog::CATEGORY_CRYPTO,
            skiff_app::runtime_catalog::CATEGORY_CACHE,
            skiff_app::runtime_catalog::CATEGORY_OTHER,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        has_job,
        job: job_view,
        job_url: format!("/{prefix}/sites/env/{}/job", encode_uri_component(&id)),
        logs_url: format!(
            "/{prefix}/sites/env/{}/logs?tail=300",
            encode_uri_component(&id)
        ),
        env_href: format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
        runtime_log_path: if view.runtime.kind == skiff_store::KIND_RUST {
            state
                .site_settings()
                .runtime_log_dir(&view.runtime.kind, &view.runtime.name)
                .join("server.log")
                .display()
                .to_string()
        } else {
            String::new()
        },
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
        "start" | "stop" | "restart" | "rebuild" | "sync" | "delete" | "install"
    ) {
        return Err(AppError::BadRequest(format!("未知运行环境操作：{action}")));
    }
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    // Web 服务是独立合成行（id=web，不落库），动作直接落到 OpenResty 容器。
    if id == "web" {
        if !matches!(
            action.as_str(),
            "install" | "rebuild" | "start" | "stop" | "restart" | "delete"
        ) {
            return Err(AppError::BadRequest(format!(
                "Web 服务不支持操作：{action}"
            )));
        }
        return match state.web_env_action(&action).await {
            Ok(()) => {
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    &format!("web_env.{action}"),
                    "-",
                    "ok",
                    "",
                );
                let notice = match action.as_str() {
                    "install" | "rebuild" => {
                        "已提交 Web 服务（OpenResty）安装任务；PHP/Node/Go/Rust 不受影响"
                    }
                    "delete" => "Web 服务容器已删除（语言运行环境各自独立，未受影响）",
                    _ => "Web 服务操作已完成",
                };
                Ok(handlers::redirect(format!(
                    "/{prefix}/sites/env?notice={}",
                    encode_uri_component(notice)
                )))
            }
            Err(e) => {
                let msg = format!("{e}");
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    &format!("web_env.{action}"),
                    "-",
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
    if action == "delete" {
        return match state.delete_site_runtime(&id).await {
            Ok(report) => {
                let mut notice = "运行环境已删除".to_string();
                if !report.skipped.is_empty() {
                    notice.push_str("；注意：");
                    notice.push_str(&report.skipped.join("；"));
                }
                let _ = state.db().audit(
                    &user.user.username,
                    &ip,
                    "runtimes.delete",
                    &id,
                    "ok",
                    &report.completed.join("；"),
                );
                Ok(handlers::redirect(format!(
                    "/{prefix}/sites/env?notice={}",
                    encode_uri_component(&notice)
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
            let notice = if matches!(action.as_str(), "install" | "rebuild") {
                "已提交后台任务：状态列实时显示阶段，详情抽屉可查看安装日志"
            } else {
                "操作已完成"
            };
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(notice)
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

/// 抽屉轮询：单个运行环境的任务快照（状态 / 阶段 / 进度 / 实时日志）。
pub async fn runtime_job(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    match state.runtime_job(&id).await {
        Some(job) => Ok(axum::Json(job_json(&job))),
        None => Ok(axum::Json(serde_json::json!({
            "id": id,
            "state": "none",
            "state_label": "",
            "step": "",
            "progress": 0,
            "error": "",
            "log": "",
            "active": false,
            "failed": false,
            "kind": "",
        }))),
    }
}

/// 列表轮询：全部运行环境任务（只返回有任务记录的环境）。
pub async fn runtime_jobs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let jobs = state.runtime_jobs_snapshot().await;
    Ok(axum::Json(serde_json::json!({
        "jobs": jobs.iter().map(job_json).collect::<Vec<_>>(),
    })))
}

/// 扩展增删：写库 + 后台执行（启用脚本 / 停用脚本 + 重载 FPM）。
pub async fn runtime_extensions_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let selected = form.get("ext").cloned().unwrap_or_default();
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.update_runtime_extensions(&id, &selected).await {
        Ok(changed) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "ok",
                "",
            );
            let notice = if changed {
                "扩展变更任务已提交：进度与日志会实时刷新"
            } else {
                "扩展集合未变化"
            };
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(notice)
            )))
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
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

/// 安装单个扩展（目录项或 `pecl:` token）：异步任务，立即返回。
pub async fn runtime_extension_install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, key)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.install_extension(&id, &key).await {
        Ok(_) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "ok",
                &key,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(&format!("扩展 {key} 安装任务已提交，进度与日志会实时刷新"))
            )))
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &format!("{key}: {msg}"),
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?error={}",
                encode_uri_component(&id),
                encode_uri_component(&msg)
            )))
        }
    }
}

/// 卸载单个扩展：删除 ini → 重启容器 → 从启用集合移除。
pub async fn runtime_extension_uninstall(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, key)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.uninstall_extension(&id, &key).await {
        Ok(_) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "ok",
                &key,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(&format!("扩展 {key} 卸载任务已提交"))
            )))
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &format!("{key}: {msg}"),
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?error={}",
                encode_uri_component(&id),
                encode_uri_component(&msg)
            )))
        }
    }
}

/// 批量勾选安装/卸载：一次请求、一个任务、日志合流、只重启一次容器。
pub async fn runtime_extensions_batch(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let op = form_first(&form, "op").trim().to_string();
    let add = match op.as_str() {
        "install" => true,
        "uninstall" => false,
        _ => {
            return Err(AppError::BadRequest(format!(
                "未知扩展操作：{op}（应为 install / uninstall）"
            )))
        }
    };
    let keys = form.get("ext").cloned().unwrap_or_default();
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    if keys.is_empty() {
        return Ok(handlers::redirect(format!(
            "/{prefix}/sites/env/{}?error={}",
            encode_uri_component(&id),
            encode_uri_component("未选择任何扩展")
        )));
    }
    match state.apply_extension_selection(&id, add, &keys).await {
        Ok(0) => Ok(handlers::redirect(format!(
            "/{prefix}/sites/env/{}?notice={}",
            encode_uri_component(&id),
            encode_uri_component("所选扩展均已是目标状态，无需变更")
        ))),
        Ok(n) => {
            let action = if add { "安装" } else { "卸载" };
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions.batch",
                &id,
                "ok",
                &format!("{action} {n} 项：{}", keys.join(", ")),
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(&format!(
                    "{n} 项扩展{action}任务已提交（单任务串行执行，日志合流，只重启一次容器）"
                ))
            )))
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions.batch",
                &id,
                "failed",
                &format!("{}: {msg}", keys.join(", ")),
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?error={}",
                encode_uri_component(&id),
                encode_uri_component(&msg)
            )))
        }
    }
}

/// 按名字安装 PECL 扩展（可选版本）。
pub async fn runtime_pecl_install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let name = form_first(&form, "name").trim().to_string();
    let version = form_first(&form, "version").trim().to_string();
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.install_pecl_extension(&id, &name, &version).await {
        Ok(_) => Ok(handlers::redirect(format!(
            "/{prefix}/sites/env/{}?notice={}",
            encode_uri_component(&id),
            encode_uri_component(&format!("PECL {name} 安装任务已提交"))
        ))),
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.extensions",
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

/// 抽屉日志页签：安装日志在页面快照里，运行日志由这里按需拉取。
pub async fn runtime_logs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let tail = q
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(300)
        .min(2000);
    match state.runtime_logs(&id, tail).await {
        Ok(text) => Ok(axum::Json(serde_json::json!({
            "ok": true,
            "logs": text,
        }))),
        Err(e) => Ok(axum::Json(serde_json::json!({
            "ok": false,
            "logs": e.to_string(),
        }))),
    }
}

/// 「实测版本」按钮：唯一主动进容器执行的位置，2 秒超时。
pub async fn runtime_probe(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match state.probe_runtime_version(&id).await {
        Ok(version) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.probe",
                &id,
                "ok",
                &version,
            );
            Ok(axum::Json(serde_json::json!({
                "ok": true,
                "version": version,
                "at": at,
            })))
        }
        Err(e) => Ok(axum::Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
            "at": at,
        }))),
    }
}

/// 站点页签的解绑入口：停用 vhost、站点转静态并停止。
pub async fn runtime_unbind_site(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, domain)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    match state.unbind_runtime_site(&id, &domain).await {
        Ok(()) => {
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.unbind",
                &id,
                "ok",
                &domain,
            );
            Ok(handlers::redirect(format!(
                "/{prefix}/sites/env/{}?notice={}",
                encode_uri_component(&id),
                encode_uri_component(&format!("站点 {domain} 已解绑并停止"))
            )))
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "runtimes.unbind",
                &id,
                "failed",
                &format!("{domain}: {msg}"),
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

#[cfg(test)]
mod tests {
    use super::{error_field, save_config_notice, version_display};

    #[test]
    fn save_config_notice_never_claims_reload_when_site_stopped() {
        assert!(save_config_notice(true).contains("已重载"));
        assert!(save_config_notice(true).contains("校验通过"));
        assert!(!save_config_notice(false).contains("已重载"));
        assert!(save_config_notice(false).contains("站点未运行，未重载"));
    }

    #[test]
    fn server_error_maps_to_visible_field_only() {
        // 重复站点落到主域名字段旁。
        assert_eq!(error_field("站点已存在：shop.local", "static"), "domain");
        // 别名冲突落到别名区。
        assert_eq!(
            error_field("域名已被站点使用：www.shop.local", "runtime"),
            "aliases"
        );
        assert_eq!(error_field("域名重复：www.shop.local", "proxy"), "aliases");
        // 类型专属错误只在对应类型下落到字段，否则回顶部提示。
        assert_eq!(error_field("上游地址不能为空", "proxy"), "proxy_target");
        assert_eq!(error_field("上游地址不能为空", "static"), "");
        assert_eq!(error_field("请选择运行环境", "runtime"), "runtime_id");
        assert_eq!(error_field("请选择运行环境", "static"), "");
        // 环境级错误不属于任何输入字段。
        assert_eq!(
            error_field(
                "Web 服务（OpenResty）未就绪：请先在「网站 → 运行环境」安装/启动 Web 服务",
                "static"
            ),
            ""
        );
        assert_eq!(
            error_field(
                "Web 服务（OpenResty）未就绪：请先在「网站 → 运行环境」安装/启动 Web 服务",
                "runtime"
            ),
            ""
        );
        assert_eq!(error_field("Docker 不可用：socket 不存在", "runtime"), "");
    }

    #[test]
    fn version_column_shows_actual_then_falls_back_with_marker() {
        // 已知真实版本：主显补丁号，副行给出目标与镜像。
        let (display, note) = version_display(Some("8.5.12"), "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5.12");
        assert_eq!(note, "目标 8.5 · php:8.5-fpm-alpine");

        // 容器缺失（未安装）：目标 + 未安装。
        let (display, note) = version_display(None, "8.5", "php:8.5-fpm-alpine", false);
        assert_eq!(display, "8.5");
        assert_eq!(note, "未安装");

        // 容器存在但镜像元数据读不到：目标 + 版本未读取，不伪造真实版本。
        let (display, note) = version_display(None, "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5");
        assert_eq!(note, "版本未读取");

        // 停止但容器仍存在 → 上层仍传入镜像元数据版本，照常显示真实版本。
        let (display, note) = version_display(Some("8.5.12"), "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5.12");
        assert_eq!(note, "目标 8.5 · php:8.5-fpm-alpine");
    }
}
