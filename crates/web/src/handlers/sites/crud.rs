//! 站点 CRUD：列表与新建、启停与默认站、域名、配置保存、批量操作、详情与删除。

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;
use skiff_app::actions as act;
use skiff_app::commands::site as site_cmd;
use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{
    DirEntryView, DirListingView, RuntimeOptionView, SiteDeletePage, SiteDetailPage, SiteNewForm,
    SiteView, SitesPage,
};
use crate::util::{encode_uri_component, form_first, human_size, parse_form};

use super::tls::backup_view;
use super::*;

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let open = q.contains_key("new") || q.contains_key("type");
    render_sites(
        &state,
        &user,
        &q,
        default_new_form(&q),
        String::new(),
        None,
        open,
    )
    .await
}

/// 列表页与新建弹窗共用一次渲染：校验失败时把表单值与错误一并带回，
/// 弹窗保持展开，避免「错误页 + 重新填写」的往返。
async fn render_sites(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    q: &HashMap<String, String>,
    form: SiteNewForm,
    form_error: String,
    form_error_code: Option<SiteField>,
    form_open: bool,
) -> Result<Response, AppError> {
    let mut form = form;
    let env = state.site_env_status().await;
    let sites = state.db().list_sites()?;
    let keyword = q.get("q").cloned().unwrap_or_default();
    let needle = keyword.trim().to_lowercase();
    // 证书列与详情 SSL 标签同源：直接看站点证书目录下的 cert.pem/key.pem 是否存在。
    let cert_settings = state.site_settings();
    // DB 读失败不能当成「没有运行环境」：列表会伪装成空态，用户以为实例被删了。
    let runtimes = state.db().list_runtimes()?;
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
            let ready = cert_settings.cert_ready(FsPath::new(&s.root_dir));
            let runtime = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtimes.iter().find(|r| r.id == id));
            let runtime_state = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtime_views.iter().find(|v| v.runtime.id == id))
                .map(|v| v.effective_state.as_str());
            let runtime_site_count = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtime_views.iter().find(|v| v.runtime.id == id))
                .map(|v| v.site_count)
                .unwrap_or(0);
            site_view(
                s,
                state.site_aliases(&s.domain),
                ready,
                runtime,
                runtime_state,
                runtime_site_count,
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
    let form_error_field = error_field(form_error_code, &form.site_type).to_string();
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

/// 服务端错误落到具体字段：只查领域错误携带的 `SiteField`，不做中文文案匹配。
///
/// 两类错误回到顶部提示，避免落到当前类型下不可见的字段：
/// - 环境级错误没有字段码；
/// - 类型专属字段与当前选中的类型不匹配（如静态站报运行环境错误）。
pub(super) fn error_field(field: Option<SiteField>, site_type: &str) -> &'static str {
    match field {
        None => "",
        Some(SiteField::RuntimeId) if site_type != TYPE_RUNTIME => "",
        Some(SiteField::ProxyTarget) if site_type != TYPE_PROXY => "",
        Some(field) => field.as_str(),
    }
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

/// 用户只填目录名；容错剥掉 `/www/sites/` 或宿主机前缀的逻辑在命令层
/// （`skiff_app::commands::site`），handler 只负责原样提交表单值。
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
            // 同名版本可能被克隆出多个环境，选项里带上实例名与共享影响面。
            let label = if v.site_count > 0 {
                format!(
                    "{kind} {} · {}（已被 {} 个站点使用，重启/升级影响全部）",
                    v.runtime.version, v.runtime.name, v.site_count
                )
            } else {
                format!("{kind} {} · {}", v.runtime.version, v.runtime.name)
            };
            RuntimeOptionView {
                id: v.runtime.id.clone(),
                label,
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
    crate::authz::authorize(&user, act::SITE_CREATE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let values = new_form(&form);
    let cmd = site_cmd::CreateSite {
        domain: values.domain.clone(),
        root_dir: values.root_dir.clone(),
        site_type: values.site_type.clone(),
        runtime_id: Some(values.runtime_id.clone()),
        proxy_target: values.proxy_target.clone(),
        proxy_websocket: values.proxy_websocket,
        aliases: collect_aliases(&form),
        actor: actor(&state, &user, &parts),
    };
    match site_cmd::create(&state, cmd).await {
        Ok(result) => {
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
            let field = e.field();
            let message = match site_error_text(&e) {
                Some(m) => m,
                None => return Err(map_err(e)),
            };
            // 校验失败仍留在列表页，弹窗保持展开并回填，不做二次确认。
            render_sites(&state, &user, &HashMap::new(), values, message, field, true).await
        }
    }
}

pub async fn action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((domain, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::SITE_ACTION)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let running = match action.as_str() {
        "start" => true,
        "stop" => false,
        _ => return Err(AppError::BadRequest(format!("未知站点操作：{action}"))),
    };
    site_cmd::power(
        &state,
        site_cmd::SitePower {
            domain: domain.clone(),
            running,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
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
    crate::authz::authorize(&user, act::SITE_SET_DEFAULT)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let enabled = matches!(
        form_first(&form, "enabled").trim(),
        "1" | "on" | "true" | "yes"
    );
    site_cmd::set_default(
        &state,
        site_cmd::SetDefaultSite {
            domain: domain.clone(),
            enabled,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
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
    crate::authz::authorize(&user, act::SITE_UPDATE_DOMAINS)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let action = form_first(&form, "action").trim().to_string();
    let alias = form_first(&form, "alias").trim().to_string();
    let prefix = &state.cfg.entry_prefix;
    let target = format!(
        "/{prefix}/sites/{}?tab=domains",
        encode_uri_component(&domain)
    );
    let op = match action.as_str() {
        "add" => site_cmd::AliasOp::Add,
        "remove" => site_cmd::AliasOp::Remove,
        _ => {
            return Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                format!("未知域名操作：{action}"),
                target,
            ))
        }
    };
    let result = site_cmd::update_domains(
        &state,
        site_cmd::UpdateDomains {
            domain: domain.clone(),
            op,
            alias,
            actor: actor(&state, &user, &parts),
        },
    )
    .await;
    match result {
        Ok(_) => {
            let notice = if op == site_cmd::AliasOp::Add {
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
                SiteError::Field(_, inner) => match **inner {
                    SiteError::Internal(_) => return Err(map_err(e)),
                    _ => inner.to_string(),
                },
            };
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
        // 实时视图拿不到时才回查 DB；DB 故障要显式失败，不能降级成「用 id 当名字」。
        let name = match runtime_view.as_ref() {
            Some(v) => v.runtime.name.clone(),
            None => match state.db().runtime_by_id(&rid)? {
                Some(r) => r.name,
                None => rid.clone(),
            },
        };
        let href = format!(
            "/{}/runtimes/{}",
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
    // `openssl x509` 是同步 fork：必须丢进阻塞线程池，否则详情页会在 async worker
    // 上等子进程。读取失败只降级 SSL 标签，不让整张详情页报错。
    let tls_state = state.clone();
    let tls_domain = domain.clone();
    let ssl_info =
        match crate::handlers::blocking_fs(move || tls_state.site_tls_info(&tls_domain)).await {
            Ok(Ok(info)) => Some(info),
            Ok(Err(e)) => {
                tracing::warn!(domain = %domain, error = %e, "读取站点证书信息失败");
                None
            }
            Err(e) => {
                tracing::warn!(domain = %domain, error = %e, "证书信息后台任务执行失败");
                None
            }
        };
    let cert_ready = ssl_info.as_ref().map(|i| i.cert_exists).unwrap_or(false);
    let ssl = ssl_info.map(tls_view).unwrap_or_default();
    // 实时视图缺失时回查 DB 的运行环境记录；DB 故障直接报错，不伪装成「无运行环境」。
    let runtime = match runtime_view.as_ref() {
        Some(v) => Some(v.runtime.clone()),
        None => match site.runtime_id.as_deref() {
            Some(id) => state.db().runtime_by_id(id)?,
            None => None,
        },
    };
    let runtime_state = runtime_view.as_ref().map(|v| v.effective_state.as_str());
    let runtime_site_count = runtime_view.as_ref().map(|v| v.site_count).unwrap_or(0);
    let backup = backup_view(&state, &domain).await;
    handlers::render(SiteDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        site: site_view(
            &site,
            state.site_aliases(&domain),
            cert_ready,
            runtime.as_ref(),
            runtime_state,
            runtime_site_count,
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

/// 保存站点的成功文案：只有真正重载成功才能说「已重载」。
pub(super) fn save_config_notice(reloaded: bool) -> &'static str {
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
    crate::authz::authorize(&user, act::SITE_SAVE_CONFIG)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let content = form_first(&form, "content");
    let prefix = &state.cfg.entry_prefix;
    match site_cmd::save_config(
        &state,
        site_cmd::SaveSiteConfig {
            domain: domain.clone(),
            content: content.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(outcome) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            save_config_notice(outcome.reloaded),
            format!(
                "/{prefix}/sites/{}?tab=config",
                encode_uri_component(&domain)
            ),
        )),
        Err(e) => {
            // 校验失败原地回显：把本次提交内容带回配置标签页；跳转会重读磁盘，冲掉编辑。
            render_detail(
                state,
                user,
                domain,
                "config".to_string(),
                format!("{e}"),
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
    crate::authz::authorize(&user, act::SITE_BATCH_CREATE)?;
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
    let op = match action.as_str() {
        "start" => site_cmd::BatchOp::Start,
        "stop" => site_cmd::BatchOp::Stop,
        // 批量删除必须走单站确认页，由命令层拒绝，这里保留可识别分支。
        "delete" => site_cmd::BatchOp::Delete,
        _ => return Err(AppError::BadRequest(format!("未知批量操作：{action}"))),
    };
    let report = site_cmd::batch(
        &state,
        site_cmd::BatchSites {
            op,
            domains,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    if report.failures.is_empty() {
        Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("批量操作完成：{} 个站点", report.succeeded),
            format!("/{prefix}/sites"),
        ))
    } else {
        let failed: Vec<String> = report
            .failures
            .into_iter()
            .map(|f| format!("{}: {}", f.domain, map_err(f.error)))
            .collect();
        Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!(
                "成功 {} 个，失败 {} 个：{}",
                report.succeeded,
                failed.len(),
                failed.join("；")
            ),
            format!("/{prefix}/sites"),
        ))
    }
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
    })
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::SITE_DELETE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    // 输入域名确认：前端按钮禁用只是防手滑，服务端也必须校验，
    // 防止绕过确认页直接 POST。
    if form_first(&form, "confirm").trim() != domain {
        return Err(AppError::BadRequest(format!(
            "请输入站点域名 {domain} 以确认删除"
        )));
    }
    site_cmd::delete(
        &state,
        site_cmd::DeleteSite {
            domain: domain.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(map_err)?;
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::redirect(format!("/{prefix}/sites")))
}

// ---------------------------------------------------------------------------
// 运行环境详情 / 操作 / PHP 专属配置
// ---------------------------------------------------------------------------
