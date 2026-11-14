use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/sites`：站点列表（只读）。
///
/// 复用页面同一批查询与 `site_view` 展示模型，不新增 SQL；`q` 过滤与页面一致，
/// 分页为内存 keyset（排序沿用 `list_sites` 的 `created_at DESC, id DESC`）。
pub async fn sites(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let sites = state
        .db()
        .list_sites()
        .map_err(|e| ApiError(AppError::from(e)))?;
    let runtimes = state
        .db()
        .list_runtimes()
        .map_err(|e| ApiError(AppError::from(e)))?;
    let views = state.runtime_views().await.unwrap_or_default();
    let cert_settings = state.site_settings();
    let needle = q.q.as_deref().unwrap_or("").trim().to_lowercase();
    let mut rows: Vec<(i64, i64, SiteItem)> = sites
        .iter()
        .map(|s| {
            let cert_ready = cert_settings.cert_ready(FsPath::new(&s.root_dir));
            (
                s.created_at,
                s.id,
                site_item(&state, s, &runtimes, &views, cert_ready),
            )
        })
        .filter(|(_, _, item)| {
            needle.is_empty()
                || item.domain.to_lowercase().contains(&needle)
                || item.root_dir.to_lowercase().contains(&needle)
                || item.endpoint.to_lowercase().contains(&needle)
                || item.aliases.join("、").to_lowercase().contains(&needle)
        })
        .collect();
    let total = rows.len();
    let start = match q.cursor.as_deref().and_then(parse_site_cursor) {
        Some(cursor) => rows
            .iter()
            .position(|(created, id, _)| (*created, *id) < cursor)
            .unwrap_or(total),
        None => 0,
    };
    let end = (start + page_limit(&q)).min(total);
    let next_cursor = if end < total && end > start {
        rows.get(end - 1)
            .map(|(created, id, _)| format!("{created}:{id}"))
    } else {
        None
    };
    let items: Vec<SiteItem> = rows.drain(start..end).map(|(_, _, item)| item).collect();
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "can_admin": user.can(skiff_model::Permission::Admin),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/sites/{domain}`：站点详情（只读）。
///
/// 只读分支复用详情页的既有查询（站点记录、运行环境实时视图、证书信息），
/// 配置/目录/域名等标签页的写操作留给后续批次。
pub async fn site_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
) -> Result<Response, ApiError> {
    let site = state
        .db()
        .site_by_domain(&domain)
        .map_err(|e| ApiError(AppError::from(e)))?
        .ok_or_else(|| ApiError(AppError::NotFound(format!("站点不存在：{domain}"))))?;
    let runtime_view = if site.site_type == skiff_store::TYPE_RUNTIME {
        match site.runtime_id.as_deref() {
            Some(rid) if !rid.is_empty() => state.runtime_view(rid).await.ok(),
            _ => None,
        }
    } else {
        None
    };

    let (runtime_down, runtime_alert) = if site.site_type == skiff_store::TYPE_RUNTIME {
        match runtime_view.as_ref() {
            Some(v) => {
                let mut down = v.effective_state != "running";
                let mut reason = String::new();
                if !down
                    && v.runtime.kind == skiff_store::KIND_PHP
                    && v.runtime.socket_path.is_some()
                    && !v.socket_exists
                {
                    down = true;
                    reason = "socket 缺失".to_string();
                }
                let name = v.runtime.name.clone();
                let alert = if down {
                    let state_text = if reason.is_empty() {
                        handlers::sites::state_label(&v.effective_state)
                    } else {
                        reason.as_str()
                    };
                    format!(
                        "绑定的运行环境 {name} 当前{state_text}，访问本站点会返回 502/503。请启动该运行环境；系统不会静默切换到其它版本。"
                    )
                } else {
                    String::new()
                };
                (down, alert)
            }
            None => {
                let rid = site.runtime_id.clone().unwrap_or_default();
                let name = match state
                    .db()
                    .runtime_by_id(&rid)
                    .map_err(|e| ApiError(AppError::from(e)))?
                {
                    Some(r) => r.name,
                    None => rid,
                };
                (
                    true,
                    format!(
                        "绑定的运行环境 {name} 不存在或状态未知，访问本站点会返回 502/503。请到运行环境页确认；系统不会静默切换到其它版本。"
                    ),
                )
            }
        }
    } else {
        (false, String::new())
    };

    let runtime = match runtime_view.as_ref() {
        Some(v) => Some(v.runtime.clone()),
        None => match site.runtime_id.as_deref() {
            Some(id) => state
                .db()
                .runtime_by_id(id)
                .map_err(|e| ApiError(AppError::from(e)))?,
            None => None,
        },
    };

    // `openssl x509` 是同步 fork，与详情页一样丢进阻塞线程池；读取失败只降级证书信息。
    let ssl = {
        let tls_state = state.clone();
        let tls_domain = domain.clone();
        match crate::handlers::blocking_fs(move || tls_state.site_tls_info(&tls_domain)).await {
            Ok(Ok(info)) => SiteSsl {
                enabled: info.enabled,
                force_https: info.force_https,
                cert_exists: info.cert_exists,
                cert_path: info.cert_path,
                key_path: info.key_path,
                subject: info.subject.unwrap_or_default(),
                issuer: info.issuer.unwrap_or_default(),
                expires: info.expires.unwrap_or_default(),
            },
            _ => SiteSsl {
                enabled: site.https,
                force_https: site.force_https,
                ..Default::default()
            },
        }
    };
    let cert_ready = if ssl.cert_exists {
        true
    } else {
        state
            .site_settings()
            .cert_ready(FsPath::new(&site.root_dir))
    };
    let runtime_records: Vec<skiff_store::Runtime> = runtime.clone().into_iter().collect();
    let runtime_views: Vec<skiff_app::sites::RuntimeView> =
        runtime_view.clone().into_iter().collect();
    let item = site_item(&state, &site, &runtime_records, &runtime_views, cert_ready);
    let runtime_ref = runtime_view.as_ref().map(|v| SiteRuntimeRef {
        id: v.runtime.id.clone(),
        name: v.runtime.name.clone(),
        kind: StatusDto {
            code: v.runtime.kind.clone(),
            label: skiff_app::runtime_catalog::kind_label(&v.runtime.kind).to_string(),
        },
        version: v.runtime.version.clone(),
        state: StatusDto {
            code: v.effective_state.clone(),
            label: handlers::sites::state_label(&v.effective_state).to_string(),
        },
        site_count: v.site_count,
    });
    let config = match state.read_site_vhost(&domain) {
        Ok((path, content, active)) => SiteConfigInfo {
            path,
            content,
            active,
            error: String::new(),
        },
        Err(e) => SiteConfigInfo {
            error: e.to_string(),
            ..Default::default()
        },
    };
    let backup_view = handlers::sites::backup_view(&state, &domain).await;
    let backup = SiteBackupInfo {
        available: state.tasks().is_some(),
        enabled: backup_view.enabled,
        task_id: backup_view.task_id,
        last_text: backup_view.last_text,
        next_text: backup_view.next_text,
    };
    let data = SiteDetailData {
        site: item,
        ssl,
        runtime: runtime_ref,
        runtime_down,
        runtime_alert,
        config,
        backup,
        can_manage: user.can(skiff_model::Permission::Admin),
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/sites`：新建站点（`SITE_CREATE`）。
pub async fn site_create(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<SiteCreateBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_CREATE).map_err(ApiError)?;
    let form = site_create_form(&body);
    let values = handlers::sites::new_form(&form);
    let cmd = skiff_app::commands::site::CreateSite {
        domain: values.domain.clone(),
        root_dir: values.root_dir.clone(),
        site_type: values.site_type.clone(),
        runtime_id: Some(values.runtime_id.clone()),
        proxy_target: values.proxy_target.clone(),
        proxy_websocket: values.proxy_websocket,
        aliases: handlers::sites::collect_aliases(&form),
        actor: handlers::sites::actor(&state, &user, &parts),
    };
    let result = skiff_app::commands::site::create(&state, cmd)
        .await
        .map_err(site_write_err)
        .map_err(ApiError)?;
    write_done(json!({ "domain": result.domain, "created": true }))
}

/// `POST /ui/v1/sites/{domain}/{action}`：启动 / 停止（`SITE_ACTION`）。
pub async fn site_power(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((domain, action)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_ACTION).map_err(ApiError)?;
    let running = match action.as_str() {
        "start" => true,
        "stop" => false,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知站点操作：{action}"
            ))))
        }
    };
    skiff_app::commands::site::power(
        &state,
        skiff_app::commands::site::SitePower {
            domain: domain.clone(),
            running,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "action": action, "running": running }))
}

pub async fn site_set_default(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<EnabledBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_SET_DEFAULT).map_err(ApiError)?;
    skiff_app::commands::site::set_default(
        &state,
        skiff_app::commands::site::SetDefaultSite {
            domain: domain.clone(),
            enabled: body.enabled,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "is_default": body.enabled }))
}

pub async fn site_domains(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<SiteDomainBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_UPDATE_DOMAINS).map_err(ApiError)?;
    let op = match body.action.as_str() {
        "add" => skiff_app::commands::site::AliasOp::Add,
        "remove" => skiff_app::commands::site::AliasOp::Remove,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知域名操作：{}",
                body.action
            ))))
        }
    };
    skiff_app::commands::site::update_domains(
        &state,
        skiff_app::commands::site::UpdateDomains {
            domain: domain.clone(),
            op,
            alias: body.alias.clone(),
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "action": body.action, "alias": body.alias }))
}

pub async fn site_save_config(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<SiteConfigBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_SAVE_CONFIG).map_err(ApiError)?;
    let outcome = skiff_app::commands::site::save_config(
        &state,
        skiff_app::commands::site::SaveSiteConfig {
            domain: domain.clone(),
            content: body.content,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({
        "domain": domain,
        "reloaded": outcome.reloaded,
        "notice": handlers::sites::save_config_notice(outcome.reloaded),
    }))
}

pub async fn site_ssl(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<SiteSslBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_SSL).map_err(ApiError)?;
    let op = match body.action.as_str() {
        "generate" => skiff_app::commands::site::SslOp::Generate,
        "upload" => skiff_app::commands::site::SslOp::Upload {
            cert_pem: body.cert_pem,
            key_pem: body.key_pem,
        },
        "force" => skiff_app::commands::site::SslOp::Force {
            enabled: body.enabled,
        },
        "disable" => skiff_app::commands::site::SslOp::Disable,
        "delete_cert" => skiff_app::commands::site::SslOp::DeleteCert,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知 SSL 操作：{}",
                body.action
            ))))
        }
    };
    skiff_app::commands::site::ssl(
        &state,
        skiff_app::commands::site::SiteSsl {
            domain: domain.clone(),
            op,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "action": body.action }))
}

/// `POST /ui/v1/sites/{domain}/backup`：站点备份开关（`SITE_SET_BACKUP`）。
pub async fn site_backup(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<EnabledBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_SET_BACKUP).map_err(ApiError)?;
    let svc = state
        .tasks()
        .ok_or_else(|| ApiError(AppError::Unavailable("内核任务服务未初始化".into())))?;
    skiff_app::commands::site::backup(
        &state,
        svc,
        skiff_app::commands::site::SetSiteBackup {
            domain: domain.clone(),
            enabled: body.enabled,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "enabled": body.enabled }))
}

pub async fn site_delete(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(domain): Path<String>,
    parts: Parts,
    Json(body): Json<SiteDeleteBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_DELETE).map_err(ApiError)?;
    if body.confirm.trim() != domain {
        return Err(ApiError(AppError::BadRequest(format!(
            "请输入站点域名 {domain} 以确认删除"
        ))));
    }
    skiff_app::commands::site::delete(
        &state,
        skiff_app::commands::site::DeleteSite {
            domain: domain.clone(),
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "removed": true }))
}

pub async fn site_batch(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<SiteBatchBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    crate::authz::authorize(&user, act::SITE_BATCH_CREATE).map_err(ApiError)?;
    let domains: Vec<String> = body
        .domains
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect();
    if domains.is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "请先选择至少一个站点".into(),
        )));
    }
    let op = match body.action.as_str() {
        "start" => skiff_app::commands::site::BatchOp::Start,
        "stop" => skiff_app::commands::site::BatchOp::Stop,
        "delete" => skiff_app::commands::site::BatchOp::Delete,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知批量操作：{}",
                body.action
            ))))
        }
    };
    let report = skiff_app::commands::site::batch(
        &state,
        skiff_app::commands::site::BatchSites {
            op,
            domains,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    let failures: Vec<serde_json::Value> = report
        .failures
        .into_iter()
        .map(|f| json!({ "domain": f.domain, "error": f.error.to_string() }))
        .collect();
    write_done(json!({
        "succeeded": report.succeeded,
        "failed": failures.len(),
        "failures": failures,
    }))
}

/// `GET /ui/v1/sites/{domain}/logs?part=access|error&tail=200&q=`：日志查看器数据。
///
/// 与旧 SSR 页面同权限口径（登录即可读）；读取逻辑复用 `AppState::site_logs`：
/// 只看文件尾部 2MB 窗口，时间倒序，关键字只作用于该窗口。
pub async fn site_logs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Path(domain): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let part = match q.get("part").map(|s| s.as_str()) {
        Some("error") => "error",
        _ => "access",
    };
    let tail = q
        .get("tail")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 2000);
    let keyword = q.get("q").cloned().unwrap_or_default();
    let st = state.clone();
    let domain_read = domain.clone();
    let keyword_read = keyword.clone();
    let (access, error) =
        handlers::blocking_fs(move || st.site_logs(&domain_read, tail, &keyword_read))
            .await
            .map_err(ApiError)?
            .map_err(site_api_error)?;
    let text = if part == "error" { error } else { access };
    let lines: Vec<String> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect();
    let log_dir = state
        .db()
        .site_by_domain(&domain)
        .map_err(|e| ApiError(AppError::Internal(e.to_string())))?
        .map(|s| {
            skiff_app::sites::site_log_dir(std::path::Path::new(&s.root_dir))
                .display()
                .to_string()
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "data": {
            "domain": domain,
            "part": part,
            "tail": tail,
            "keyword": keyword,
            "log_dir": log_dir,
            "lines": lines,
        }
    })))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/sites", get(sites).post(site_create))
        .route("/ui/v1/sites/batch", post(site_batch))
        .route("/ui/v1/sites/{domain}", get(site_detail))
        .route("/ui/v1/sites/{domain}/default", post(site_set_default))
        .route("/ui/v1/sites/{domain}/domains", post(site_domains))
        .route("/ui/v1/sites/{domain}/config", post(site_save_config))
        .route("/ui/v1/sites/{domain}/ssl", post(site_ssl))
        .route("/ui/v1/sites/{domain}/backup", post(site_backup))
        .route("/ui/v1/sites/{domain}/delete", post(site_delete))
        .route("/ui/v1/sites/{domain}/logs", get(site_logs))
        .route("/ui/v1/sites/{domain}/{action}", post(site_power))
}
