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
    BoundSiteView, DirEntryView, DirListingView, ExtensionOptionView, PhpConfigView, PhpEditorView,
    PhpFileView, PhpIniValueView, RuntimeDetailPage, RuntimeJobView, RuntimeKindView,
    RuntimeOptionView, RuntimeRow, SiteBackupView, SiteDeletePage, SiteDetailPage, SiteEnvPage,
    SiteEnvView, SiteLogsPage, SiteNewForm, SiteTlsView, SiteView, SitesPage,
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
        // Docker 的终止态统一说「已停止」，别再落到兜底的「未知」。
        "stopped" | "exited" | "dead" | "created" => "已停止",
        "paused" => "已暂停",
        "restarting" => "重启中",
        "installing" => "安装中",
        "error" => "异常",
        "missing" => "未安装",
        "unimplemented" => "P2 未实现",
        _ => "状态未知",
    }
}

fn state_class(state: &str) -> &'static str {
    match state {
        "running" => "ok",
        "error" => "danger",
        "missing" => "warn",
        // 未知不是「运行中」，用 muted 表示无法确认。
        "unknown" | "" => "muted",
        "stopped" | "exited" | "dead" | "created" | "paused" | "restarting" | "installing" => {
            "warn"
        }
        _ => "",
    }
}

/// 运行环境站点的对外状态：站点自身停止优先，否则以 Docker 实时容器状态为准。
///
/// 容器不存在 / 已停止 / Docker 不可达分别给出不同文案，禁止把「未知」当成运行中。
fn runtime_site_status(
    site_status: &str,
    runtime_state: Option<&str>,
) -> (&'static str, &'static str) {
    if site_status != "running" {
        return ("已停止", "");
    }
    match runtime_state {
        Some("running") => ("运行中", "ok"),
        Some("installing") => ("环境安装中", "warn"),
        Some("error") => ("环境异常", "danger"),
        Some("missing") => ("未运行（环境未安装）", "warn"),
        Some("unknown") => ("状态未知（Docker 不可用）", "muted"),
        Some("unimplemented") => ("运行环境 P2 未实现", "warn"),
        Some(_) => ("未运行（环境已停止）", "warn"),
        None => ("未运行（环境记录缺失）", "warn"),
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
    runtime_state: Option<&str>,
) -> SiteView {
    let runtime_label = runtime
        .map(|r| {
            format!(
                "{} {}",
                skiff_app::runtime_catalog::kind_label(&r.kind),
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
        status_label: if s.site_type == TYPE_RUNTIME {
            runtime_site_status(&s.status, runtime_state).0
        } else {
            match s.status.as_str() {
                "running" => "运行中",
                "stopped" => "已停止",
                _ => "异常",
            }
        }
        .into(),
        status_class: if s.site_type == TYPE_RUNTIME {
            runtime_site_status(&s.status, runtime_state).1
        } else {
            match s.status.as_str() {
                "running" => "ok",
                "stopped" => "",
                _ => "danger",
            }
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
    let mut form = form;
    let env = state.site_env_status().await;
    let sites = state.db().list_sites()?;
    let keyword = q.get("q").cloned().unwrap_or_default();
    let needle = keyword.trim().to_lowercase();
    // 证书列与详情 SSL 标签同源：直接看站点证书目录下的 cert.pem/key.pem 是否存在。
    let cert_settings = state.site_settings();
    let runtimes = state.db().list_runtimes().unwrap_or_default();
    // 站点列表的运行态以 Docker 实时容器状态为准；DB last_status 只是缓存，不能直接展示。
    let runtime_views = state.runtime_views().await.unwrap_or_default();
    let runtime_opts = runtime_options(&runtime_views);
    // 默认选中第一个已就绪实例；没有就绪实例时留空，由模板给出引导。
    if runtime_opts.is_empty() || !runtime_opts.iter().any(|o| o.id == form.runtime_id) {
        form.runtime_id = runtime_opts
            .first()
            .map(|o| o.id.clone())
            .unwrap_or_default();
    }
    let views: Vec<SiteView> = sites
        .iter()
        .map(|s| {
            let ready = cert_settings.cert_file(FsPath::new(&s.root_dir)).is_file()
                && cert_settings.key_file(FsPath::new(&s.root_dir)).is_file();
            let runtime = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtimes.iter().find(|r| r.id == id));
            let runtime_state = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtime_views.iter().find(|v| v.runtime.id == id))
                .map(|v| v.effective_state.as_str());
            site_view(
                s,
                state.site_aliases(&s.domain),
                ready,
                runtime,
                runtime_state,
            )
        })
        .filter(|v| {
            needle.is_empty()
                || v.domain.to_lowercase().contains(&needle)
                || v.root_dir.to_lowercase().contains(&needle)
                || v.endpoint.to_lowercase().contains(&needle)
                || v.aliases_text.to_lowercase().contains(&needle)
        })
        .collect();
    let settings = state.site_settings();
    let form_error_field = error_field(&form_error, &form.site_type).to_string();
    handlers::render(SitesPage {
        ctx: handlers::page_ctx(state, user, "core/sites/list"),
        sites: views,
        env: env_view(&env),
        can_admin: user.can(skiff_model::Permission::Admin),
        error: String::new(),
        notice: String::new(),
        keyword,
        form,
        runtimes: runtime_opts,
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
        runtime_id: String::new(),
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

/// 运行环境选项：只列已就绪（已安装且运行中）的实例。
///
/// 未安装、未运行与 P2 未实现的类型都不出现在下拉里；模板在选项为空时
/// 给出「先安装运行环境」的引导，而不是渲染一个空下拉。
fn runtime_options(views: &[skiff_app::sites::RuntimeView]) -> Vec<RuntimeOptionView> {
    views
        .iter()
        .filter(|v| v.implemented && v.effective_state == "running")
        .map(|v| {
            let kind = skiff_app::runtime_catalog::kind_label(&v.runtime.kind);
            RuntimeOptionView {
                id: v.runtime.id.clone(),
                label: format!("{kind} {}", v.runtime.version),
            }
        })
        .collect()
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
            state.audit_or_log(
                &user.user.username,
                &ip,
                "sites.create",
                &result.domain,
                "ok",
                &result.root_dir,
            );
            let prefix = &state.cfg.entry_prefix;
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                "站点已创建",
                format!("/{prefix}/sites/{}", encode_uri_component(&result.domain)),
            ))
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
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let views = state.runtime_views().await.unwrap_or_default();
    // Web 服务在「Web 服务」页单独管理；种子行只在已安装或被站点引用时出现。
    let runtimes: Vec<RuntimeRow> = views
        .iter()
        .filter(|v| v.is_listed())
        .map(|v| runtime_row(&state, v))
        .collect();
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
        error: String::new(),
        notice: String::new(),
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
    match state
        .add_runtime(
            &kind,
            &name,
            &version,
            &selected,
            &user.user.username,
            user.user.role.as_str(),
        )
        .await
    {
        Ok((rt, started)) => {
            state.audit_or_log(&user.user.username, &ip, "sites.env.add", &rt.id, "ok", "");
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                message,
                format!("/{prefix}/sites/env"),
            ))
        }
        Err(e) => {
            state.audit_or_log(
                &user.user.username,
                &ip,
                "sites.env.add",
                &name,
                "failed",
                &e.to_string(),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                e.to_string(),
                format!("/{prefix}/sites/env"),
            ))
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
    let nginx = form
        .contains_key("nginx_image")
        .then(|| form_first(&form, "nginx_image"));
    let php = form
        .contains_key("php_image")
        .then(|| form_first(&form, "php_image"));
    state
        .save_runtime_images(nginx.as_deref(), php.as_deref())
        .map_err(AppError::Internal)?;
    let ip = handlers::client_ip(&state, &parts);
    if let Err(e) = state
        .db()
        .audit(&user.user.username, &ip, "sites.env.images", "-", "ok", "")
    {
        tracing::error!(error = %e, "运行环境镜像参数审计写入失败");
    }
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        "镜像参数已保存，重建 Web 服务或重装对应实例后生效",
        format!("/{prefix}/sites/env"),
    ))
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
    state.audit_or_log(
        &user.user.username,
        &ip,
        if running { "sites.start" } else { "sites.stop" },
        &domain,
        "ok",
        "",
    );
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        if running {
            "站点已启动"
        } else {
            "站点已停止"
        },
        format!(
            "/{prefix}/sites/{}?tab=manage",
            encode_uri_component(&domain)
        ),
    ))
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
    state.audit_or_log(
        &user.user.username,
        &ip,
        "sites.default",
        &domain,
        "ok",
        if enabled { "set" } else { "unset" },
    );
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        if enabled {
            "已设为默认站点"
        } else {
            "已取消默认站点"
        },
        format!("/{prefix}/sites/{}", encode_uri_component(&domain)),
    ))
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
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                target,
            ))
        }
        Err(e) => {
            let message = match &e {
                SiteError::BadRequest(m) | SiteError::Conflict(m) | SiteError::Unavailable(m) => {
                    m.clone()
                }
                SiteError::Internal(_) => return Err(map_err(e)),
            };
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                message,
                target,
            ))
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
            state.audit_or_log(
                &user.user.username,
                &ip,
                &format!("sites.ssl.{action}"),
                &domain,
                "ok",
                "",
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                "SSL 设置已更新并重载 nginx",
                format!("/{prefix}/sites/{}?tab=ssl", encode_uri_component(&domain)),
            ))
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                &ip,
                &format!("sites.ssl.{action}"),
                &domain,
                "failed",
                &msg,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/{}?tab=ssl", encode_uri_component(&domain)),
            ))
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
        String::new(),
        String::new(),
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
    // 运行态只信 Docker 实时探测：DB 的 last_status 是缓存，可能滞后于手工停容器。
    let runtime_view = if site.site_type == skiff_store::TYPE_RUNTIME {
        match site.runtime_id.as_deref() {
            Some(rid) if !rid.is_empty() => state.runtime_view(rid).await.ok(),
            _ => None,
        }
    } else {
        None
    };
    let (runtime_down, runtime_alert, runtime_env_href) = if site.site_type
        == skiff_store::TYPE_RUNTIME
    {
        let rid = site.runtime_id.clone().unwrap_or_default();
        let name = runtime_view
            .as_ref()
            .map(|v| v.runtime.name.clone())
            .or_else(|| {
                state
                    .db()
                    .runtime_by_id(&rid)
                    .ok()
                    .flatten()
                    .map(|r| r.name)
            })
            .unwrap_or_else(|| rid.clone());
        let href = format!(
            "/{}/sites/env/{}",
            state.cfg.entry_prefix,
            encode_uri_component(&rid)
        );
        match runtime_view.as_ref() {
            Some(v) => {
                let mut down = v.effective_state != "running";
                let mut reason = String::new();
                if !down
                    && v.runtime.kind == skiff_store::KIND_PHP
                    && v.runtime.socket_path.is_some()
                    && !v.socket_exists
                {
                    // 容器在跑但 PHP-FPM socket 没挂上，同样不可用。
                    down = true;
                    reason = "socket 缺失".into();
                }
                let alert = if down {
                    let state_text = if reason.is_empty() {
                        state_label(&v.effective_state)
                    } else {
                        reason.as_str()
                    };
                    format!(
                        "绑定的运行环境 {name} 当前{state_text}，访问本站点会返回 502/503。请启动该运行环境；系统不会静默切换到其它版本。"
                    )
                } else {
                    String::new()
                };
                (down, alert, href)
            }
            None => (
                true,
                format!(
                    "绑定的运行环境 {name} 不存在或状态未知，访问本站点会返回 502/503。请到运行环境页确认；系统不会静默切换到其它版本。"
                ),
                href,
            ),
        }
    } else {
        (false, String::new(), String::new())
    };
    if !matches!(
        tab.as_str(),
        "overview" | "domains" | "config" | "dir" | "ssl" | "manage"
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
    let runtime = runtime_view
        .as_ref()
        .map(|v| v.runtime.clone())
        .or_else(|| {
            site.runtime_id
                .as_deref()
                .and_then(|id| state.db().runtime_by_id(id).ok().flatten())
        });
    let runtime_state = runtime_view.as_ref().map(|v| v.effective_state.as_str());
    let backup = backup_view(&state, &domain).await;
    handlers::render(SiteDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        site: site_view(
            &site,
            state.site_aliases(&domain),
            cert_ready,
            runtime.as_ref(),
            runtime_state,
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
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                if enabled {
                    "站点备份已开启（每日 03:00）"
                } else {
                    "站点备份已关闭"
                },
                format!(
                    "/{}/sites/{}",
                    state.cfg.entry_prefix,
                    encode_uri_component(&domain)
                ),
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!(
                "/{}/sites/{}",
                state.cfg.entry_prefix,
                encode_uri_component(&domain)
            ),
        )),
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
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                save_config_notice(outcome.reloaded),
                format!(
                    "/{prefix}/sites/{}?tab=config",
                    encode_uri_component(&domain)
                ),
            ))
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
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
        return Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            "请先选择至少一个站点",
            format!("/{prefix}/sites"),
        ));
    }
    // 批量删除没有「输入域名确认」这一关，留着就是绕过删除确认的后门：
    // 一律拒绝并引导到单站确认页（那里会展示将被删除的量并要求输入域名）。
    if action == "delete" {
        return Err(AppError::BadRequest(
            "删除站点请走「站点详情 → 删除确认页」（需输入域名确认）".into(),
        ));
    }
    if !matches!(action.as_str(), "start" | "stop") {
        return Err(AppError::BadRequest(format!("未知批量操作：{action}")));
    }
    let ip = handlers::client_ip(&state, &parts);
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for domain in &domains {
        let result = match action.as_str() {
            "start" => state.set_site_running(domain, true).await.map_err(map_err),
            // 上面已限制只有 start/stop 两种动作。
            _ => state.set_site_running(domain, false).await.map_err(map_err),
        };
        match result {
            Ok(()) => {
                ok += 1;
                state.audit_or_log(
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
                state.audit_or_log(
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
        Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("批量操作完成：{ok} 个站点"),
            format!("/{prefix}/sites"),
        ))
    } else {
        Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!(
                "成功 {ok} 个，失败 {} 个：{}",
                failed.len(),
                failed.join("；")
            ),
            format!("/{prefix}/sites"),
        ))
    }
}

pub async fn logs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let keyword = q.get("q").cloned().unwrap_or_default();
    // 日志读取是同步文件 IO（尾部最多 2MB），放到阻塞线程池，避免占住 async worker。
    let st = state.clone();
    let domain_for_read = domain.clone();
    let keyword_for_read = keyword.clone();
    let (access, error_log) = crate::handlers::blocking_fs(move || {
        st.site_logs(&domain_for_read, 200, &keyword_for_read)
    })
    .await?
    .map_err(map_err)?;
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
    let stats = state.site_delete_stats(&domain).map_err(map_err)?;
    let runtime_label = if stats.runtime_name.is_empty() {
        String::new()
    } else {
        format!(
            "{} {}",
            skiff_app::runtime_catalog::kind_label(&stats.runtime_kind),
            stats.runtime_name
        )
    };
    handlers::render(SiteDeletePage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        domain: site.domain,
        root_dir: site.root_dir,
        dir_size: human_size(i64::try_from(stats.dir_bytes).unwrap_or(i64::MAX)),
        file_count: stats.file_count,
        task_count: stats.task_count,
        runtime_label,
        rust_source: stats.rust_source,
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
    // 输入域名确认：前端按钮禁用只是防手滑，服务端也必须校验，
    // 防止绕过确认页直接 POST。
    if form_first(&form, "confirm").trim() != domain {
        return Err(AppError::BadRequest(format!(
            "请输入站点域名 {domain} 以确认删除"
        )));
    }
    state.delete_site(&domain).await.map_err(map_err)?;
    let ip = handlers::client_ip(&state, &parts);
    state.audit_or_log(&user.user.username, &ip, "sites.delete", &domain, "ok", "");
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
            status_label: runtime_site_status(&s.status, Some(view.effective_state.as_str()))
                .0
                .to_string(),
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
    let is_php = view.runtime.kind == skiff_store::KIND_PHP;
    let mut page_error = String::new();
    let mut php_files = Vec::new();
    let mut php_editor = None;
    let mut php_values = Vec::new();
    let mut php_values_loaded = false;
    let mut php_values_error = String::new();
    if is_php {
        php_files = state
            .php_config_files(&id)
            .map_err(map_err)?
            .into_iter()
            .map(|f| {
                let selected = q.get("file").map(String::as_str) == Some(f.key.as_str());
                PhpFileView {
                    label: f.label,
                    host_path: f.host_path,
                    container_path: f.container_path,
                    exists: f.exists,
                    selected,
                    edit_href: format!(
                        "/{prefix}/sites/env/{}?tab=config&file={}",
                        encode_uri_component(&id),
                        encode_uri_component(&f.key)
                    ),
                }
            })
            .collect::<Vec<_>>();
        if let Some(key) = q.get("file").filter(|v| !v.is_empty()) {
            match state.read_php_config_file(&id, key, &user.user.username) {
                Ok(v) => {
                    php_editor = Some(PhpEditorView {
                        key: v.file.key,
                        label: v.file.label,
                        content: v.content,
                        container_path: v.file.container_path,
                        note: v.file.note,
                    });
                }
                Err(e) => page_error = format!("{e}"),
            }
        }
        if q.get("ini").map(String::as_str) == Some("1") {
            php_values_loaded = true;
            match state.php_effective_values(&id).await {
                Ok(values) => {
                    php_values = values
                        .into_iter()
                        .map(|v| PhpIniValueView {
                            key: v.key,
                            file_value: v.file_value,
                            effective: v.effective,
                            source: v.source,
                        })
                        .collect();
                }
                Err(e) => php_values_error = format!("{e}"),
            }
        }
    }
    let job = state.runtime_job(&id).await;
    let (has_job, job_view) = match job.as_ref() {
        Some(j) => (true, job_view(j)),
        None => (false, RuntimeJobView::default()),
    };
    // 安装时未挂载 php/php.ini 与 conf.d 的存量容器：编辑能保存但不生效，
    // 详情页给出重建入口，别让用户对着界面猜。
    let config_mounts_stale = if is_php && view.container_exists {
        !state.php_config_mounts_current(&id).map_err(map_err)?
    } else {
        false
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
        extensions: if is_php {
            extension_options(&enabled_exts, &view.runtime.version, &view.runtime.image)
        } else {
            Vec::new()
        },
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
        php_files,
        php_editor,
        php_values,
        php_values_loaded,
        php_values_error,
        config_mounts_stale,
        can_admin: user.can(skiff_model::Permission::Admin),
        error: page_error,
        notice: String::new(),
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

/// 保存 PHP 配置文件：校验在容器内完成，失败回滚并提示原因。
pub async fn runtime_config_file_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let key = form_first(&form, "key");
    let content = form_first(&form, "content");
    let ip = handlers::client_ip(&state, &parts);
    let prefix = &state.cfg.entry_prefix;
    let back = format!(
        "/{prefix}/sites/env/{}?tab=config&file={}",
        encode_uri_component(&id),
        encode_uri_component(&key)
    );
    match state
        .save_php_config_file(&id, &key, &content, &user.user.username)
        .await
    {
        Ok(file) => {
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.config_file",
                &id,
                "ok",
                &key,
            );
            // PHP Warning 不阻止保存，但原文回显，避免“保存成功”掩盖潜在问题。
            let mut notice = format!("{} 已保存，PHP-FPM 已重启", file.label);
            if !file.warnings.is_empty() {
                notice.push_str("；校验警告（不影响本次保存）：");
                notice.push_str(&file.warnings.join("；"));
            }
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                back,
            ))
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.config_file",
                &id,
                "failed",
                &format!("{key}: {msg}"),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                back,
            ))
        }
    }
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
    // Web 服务不落库，运维入口在「Web 服务」页；动作直接落到 OpenResty 容器。
    if id == "web" {
        if !matches!(
            action.as_str(),
            "install" | "rebuild" | "start" | "stop" | "restart" | "delete"
        ) {
            return Err(AppError::BadRequest(format!(
                "Web 服务不支持操作：{action}"
            )));
        }
        let back = format!("/{prefix}/sites/web?tab=status");
        return match state.web_env_action(&action).await {
            Ok(()) => {
                state.audit_or_log(
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
                Ok(handlers::flash_redirect(
                    &state,
                    &user,
                    skiff_app::FlashKind::Notice,
                    notice,
                    back,
                ))
            }
            Err(e) => {
                let msg = format!("{e}");
                state.audit_or_log(
                    &user.user.username,
                    &ip,
                    &format!("web_env.{action}"),
                    "-",
                    "failed",
                    &msg,
                );
                Ok(handlers::flash_redirect(
                    &state,
                    &user,
                    skiff_app::FlashKind::Error,
                    msg,
                    back,
                ))
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
                state.audit_or_log(
                    &user.user.username,
                    &ip,
                    "runtimes.delete",
                    &id,
                    "ok",
                    &report.completed.join("；"),
                );
                Ok(handlers::flash_redirect(
                    &state,
                    &user,
                    skiff_app::FlashKind::Notice,
                    notice,
                    format!("/{prefix}/sites/env"),
                ))
            }
            Err(e) => {
                let msg = format!("{e}");
                state.audit_or_log(
                    &user.user.username,
                    &ip,
                    "runtimes.delete",
                    &id,
                    "failed",
                    &msg,
                );
                Ok(handlers::flash_redirect(
                    &state,
                    &user,
                    skiff_app::FlashKind::Error,
                    msg,
                    format!("/{prefix}/sites/env"),
                ))
            }
        };
    }
    match state
        .runtime_action(&id, &action, &user.user.username, user.user.role.as_str())
        .await
    {
        Ok(_) => {
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                &ip,
                &format!("runtimes.{action}"),
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
    match state
        .update_runtime_extensions(&id, &selected, &user.user.username, user.user.role.as_str())
        .await
    {
        Ok(changed) => {
            state.audit_or_log(
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
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
    match state
        .install_extension(&id, &key, &user.user.username, user.user.role.as_str())
        .await
    {
        Ok(_) => {
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "ok",
                &key,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!("扩展 {key} 安装任务已提交，进度与日志会实时刷新"),
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &format!("{key}: {msg}"),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
    match state
        .uninstall_extension(&id, &key, &user.user.username, user.user.role.as_str())
        .await
    {
        Ok(_) => {
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "ok",
                &key,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!("扩展 {key} 卸载任务已提交"),
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &format!("{key}: {msg}"),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
        return Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            "未选择任何扩展",
            format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
        ));
    }
    match state
        .apply_extension_selection(
            &id,
            add,
            &keys,
            &user.user.username,
            user.user.role.as_str(),
        )
        .await
    {
        Ok(0) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "所选扩展均已是目标状态，无需变更",
            format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
        )),
        Ok(n) => {
            let action = if add { "安装" } else { "卸载" };
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions.batch",
                &id,
                "ok",
                &format!("{action} {n} 项：{}", keys.join(", ")),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!("{n} 项扩展{action}任务已提交（单任务串行执行，日志合流，只重启一次容器）"),
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions.batch",
                &id,
                "failed",
                &format!("{}: {msg}", keys.join(", ")),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
    match state
        .install_pecl_extension(
            &id,
            &name,
            &version,
            &user.user.username,
            user.user.role.as_str(),
        )
        .await
    {
        Ok(_) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("PECL {name} 安装任务已提交"),
            format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
        )),
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.extensions",
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
            state.audit_or_log(
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
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.unbind",
                &id,
                "ok",
                &domain,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!("站点 {domain} 已解绑并停止"),
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.unbind",
                &id,
                "failed",
                &format!("{domain}: {msg}"),
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
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
    match state
        .save_php_runtime_config(&id, &cfg, &user.user.username)
        .await
    {
        Ok(_) => {
            state.audit_or_log(&user.user.username, &ip, "runtimes.config", &id, "ok", "");
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                "PHP 配置已保存并重载 FPM",
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                &ip,
                "runtimes.config",
                &id,
                "failed",
                &msg,
            );
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                msg,
                format!("/{prefix}/sites/env/{}", encode_uri_component(&id)),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        error_field, runtime_site_status, save_config_notice, state_class, state_label,
        version_display,
    };

    #[test]
    fn runtime_site_status_distinguishes_unknown_missing_and_stopped() {
        // 记录缺失 / 容器不存在 / 容器停止 / Docker 不可达必须是四种不同文案。
        assert_eq!(
            runtime_site_status("running", None).0,
            "未运行（环境记录缺失）"
        );
        assert_eq!(
            runtime_site_status("running", Some("missing")).0,
            "未运行（环境未安装）"
        );
        assert_eq!(
            runtime_site_status("running", Some("stopped")).0,
            "未运行（环境已停止）"
        );
        assert_eq!(
            runtime_site_status("running", Some("unknown")).0,
            "状态未知（Docker 不可用）"
        );
        assert_eq!(runtime_site_status("running", Some("running")).0, "运行中");
        // 配色同样区分：未知是 muted（无法确认），停止/未安装是 warn（确认不可用）。
        assert_eq!(runtime_site_status("running", Some("unknown")).1, "muted");
        assert_eq!(runtime_site_status("running", Some("stopped")).1, "warn");
        assert_eq!(runtime_site_status("running", Some("running")).1, "ok");
        // 站点自身已停止时优先展示站点状态。
        assert_eq!(runtime_site_status("stopped", Some("running")).0, "已停止");
    }

    #[test]
    fn env_row_labels_keep_unknown_missing_and_stopped_apart() {
        // 运行环境列表的徽章文案与配色：Docker 原始状态（exited/dead 等）已由
        // `runtime_effective_state` 归一成 stopped，这里只校验展示层不把未知当运行中。
        assert_eq!(state_label("running"), "运行中");
        assert_eq!(state_class("running"), "ok");
        for stopped in ["stopped", "exited", "dead", "created"] {
            assert_eq!(state_label(stopped), "已停止", "{stopped}");
            assert_eq!(state_class(stopped), "warn", "{stopped}");
        }
        assert_eq!(state_label("missing"), "未安装");
        assert_eq!(state_class("missing"), "warn");
        assert_eq!(state_label("unknown"), "状态未知");
        assert_eq!(state_class("unknown"), "muted");
        assert_eq!(state_label(""), "状态未知");
        assert_eq!(state_class(""), "muted");
    }

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
