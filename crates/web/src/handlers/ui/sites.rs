use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/sites`：站点列表（只读）。
///
/// 复用页面同一批查询与 `site_view` 展示模型，不新增 SQL；`q` 过滤与页面一致，
/// 分页为内存 keyset（排序沿用 `list_sites` 的 `created_at DESC, id DESC`）。
/// `with_schema=true` 时额外返回服务端 Schema（页面加载用），表格翻页不带该参数。
pub async fn sites(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<SitesQuery>,
) -> Result<Response, ApiError> {
    let list = ListQuery {
        limit: q.limit,
        cursor: q.cursor.clone(),
        q: q.q.clone(),
    };
    let read = skiff_app::views::sites::list(&state, list.q.as_deref().unwrap_or(""))
        .await
        .map_err(|e| ApiError(AppError::from(e)))?;
    let mut rows: Vec<(i64, i64, SiteItem)> = read
        .sites
        .iter()
        .map(|s| (s.created_at_epoch, s.db_id, site_item(s)))
        .collect();
    let total = rows.len();
    // 站点按 `(created_at, id)` 倒序，游标是上一页末项的复合键：取第一条严格小于游标
    // 的行作为起点（找不到即已翻完），与 `paginate` 的「等值 +1」升序语义不同，故不走它。
    let start = match list.cursor.as_deref().and_then(parse_site_cursor) {
        Some(cursor) => rows
            .iter()
            .position(|(created, id, _)| (*created, *id) < cursor)
            .unwrap_or(total),
        None => 0,
    };
    let end = (start + page_limit(&list)).min(total);
    let next_cursor = if end < total && end > start {
        rows.get(end - 1)
            .map(|(created, id, _)| format!("{created}:{id}"))
    } else {
        None
    };
    let items: Vec<SiteItem> = rows.drain(start..end).map(|(_, _, item)| item).collect();
    let can_admin = user.can(skiff_model::Permission::Admin);
    let mut body = json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "can_admin": can_admin,
        }
    });
    if q.with_schema {
        let runtimes = if can_admin {
            skiff_app::views::runtimes::list(&state).await
        } else {
            Vec::new()
        };
        let prefix = format!("/{}", state.cfg.entry_prefix);
        body["schema"] = super::site_page::sites_page_schema(total, can_admin, &runtimes, &prefix);
    }
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 站点写操作统一按主键解析域名：页面 URL 不再把主域名当路径参数。
fn site_domain(state: &AppState, id: i64) -> Result<String, ApiError> {
    state
        .db()
        .site_by_id(id)
        .map_err(|e| ApiError(AppError::from(e)))?
        .map(|s| s.domain)
        .ok_or_else(|| ApiError(AppError::NotFound(format!("站点不存在：{id}"))))
}

/// `GET /ui/v1/sites/{id}`：站点详情（只读）。
///
/// 只读分支复用详情页的既有查询（站点记录、运行环境实时视图、证书信息），
/// 响应同时带回 `tab` 对应的 SDUI Schema（服务端生成试点），前端只 fetch + embed；
/// 配置/目录/域名等标签页的写操作仍是既有的 `skiffSiteWrite` 动作。
pub async fn site_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<i64>,
    Query(q): Query<SiteDetailQuery>,
) -> Result<Response, ApiError> {
    let site = state
        .db()
        .site_by_id(id)
        .map_err(|e| ApiError(AppError::from(e)))?
        .ok_or_else(|| ApiError(AppError::NotFound(format!("站点不存在：{id}"))))?;
    let domain = site.domain.clone();
    let runtime_view = if site.site_type == skiff_store::TYPE_RUNTIME {
        match site.runtime_id.as_deref() {
            Some(rid) if !rid.is_empty() => state.runtime_view(rid).await.ok(),
            _ => None,
        }
    } else {
        None
    };

    let (mut runtime_down, mut runtime_alert) = if site.site_type == skiff_store::TYPE_RUNTIME {
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

    // 证书解析走内核执行通道（`skiff_host`），异步等待；读取失败只降级证书信息。
    let ssl = {
        match state.site_tls_info(&domain).await {
            Ok(info) => SiteSsl {
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
    let summary = skiff_app::views::sites::summary(
        &state,
        &site,
        cert_ready,
        runtime.as_ref(),
        runtime_view.as_ref().map(|v| v.effective_state.as_str()),
        runtime_view.as_ref().map(|v| v.site_count).unwrap_or(0),
    );
    let item = site_item(&summary);
    if site.status == "running" && !summary.maintenance_reason.is_empty() {
        runtime_down = true;
        runtime_alert = format!(
            "站点已进入维护状态（访问返回 503）：{}。系统会在运行环境恢复后自动解除维护，无需手工改配置。",
            summary.maintenance_reason
        );
    }
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
        interval: backup_view.interval,
        time: backup_view.time,
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
    let tab = q.tab.unwrap_or_default().to_lowercase();
    let prefix = format!("/{}", state.cfg.entry_prefix);
    let schema = super::site_page::site_page_schema(&data, &tab, &prefix);
    let mut resp = Json(json!({ "data": data, "schema": schema })).into_response();
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
    guard::write(&state, &user, &parts, act::SITE_CREATE)?;
    // 请求体字段与旧新建表单同名，但这里直接构造命令输入，不再经表单 map 解析。
    let mut aliases = body.aliases.clone();
    aliases.push(body.aliases_bulk.clone());
    let cmd = skiff_app::commands::site::CreateSite {
        domain: body.domain.trim().to_string(),
        root_dir: body.root_dir.trim().to_string(),
        site_type: body.site_type.trim().to_string(),
        runtime_id: Some(body.runtime_id.trim().to_string()),
        proxy_target: body.proxy_target.trim().to_string(),
        proxy_websocket: body.proxy_websocket,
        aliases,
        actor: handlers::sites::actor(&state, &user, &parts),
    };
    let result = skiff_app::commands::site::create(&state, cmd)
        .await
        .map_err(site_write_err)
        .map_err(ApiError)?;
    write_done(json!({ "id": result.id, "domain": result.domain, "created": true }))
}

/// `POST /ui/v1/sites/{id}/{action}`：启动 / 停止（`SITE_ACTION`）。
pub async fn site_power(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, action)): Path<(i64, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_ACTION)?;
    let domain = site_domain(&state, id)?;
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
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<EnabledBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_SET_DEFAULT)?;
    let domain = site_domain(&state, id)?;
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
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SiteDomainBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_UPDATE_DOMAINS)?;
    let domain = site_domain(&state, id)?;
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
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SiteConfigBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_SAVE_CONFIG)?;
    let domain = site_domain(&state, id)?;
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
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SiteSslBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_SSL)?;
    let domain = site_domain(&state, id)?;
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

/// `POST /ui/v1/sites/{id}/backup`：站点备份开关与计划（`SITE_SET_BACKUP`）。
pub async fn site_backup(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SiteBackupBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_SET_BACKUP)?;
    let domain = site_domain(&state, id)?;
    let svc = state
        .tasks()
        .ok_or_else(|| ApiError(AppError::Unavailable("内核任务服务未初始化".into())))?;
    skiff_app::commands::site::backup(
        &state,
        svc,
        skiff_app::commands::site::SetSiteBackup {
            domain: domain.clone(),
            enabled: body.enabled,
            interval: body.interval,
            time: body.time,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "enabled": body.enabled }))
}

/// `POST /ui/v1/sites/{id}/policy`：伪静态预设 + 出口限速（`SITE_SET_POLICY`）。
pub async fn site_policy(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SitePolicyBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_SET_POLICY)?;
    let domain = site_domain(&state, id)?;
    skiff_app::commands::site::set_policy(
        &state,
        skiff_app::commands::site::SetSitePolicy {
            domain: domain.clone(),
            rewrite: body.rewrite,
            limit_rate_kbps: body.limit_rate_kbps,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(site_write_err)
    .map_err(ApiError)?;
    write_done(json!({ "domain": domain, "saved": true }))
}

pub async fn site_delete(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<i64>,
    parts: Parts,
    Json(body): Json<SiteDeleteBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::SITE_DELETE)?;
    let domain = site_domain(&state, id)?;
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
    guard::write(&state, &user, &parts, act::SITE_BATCH_CREATE)?;
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

/// `GET /ui/v1/sites/{id}/logs?part=access|error&tail=200&q=`：日志查看器数据。
///
/// 与旧 SSR 页面同权限口径（登录即可读）；读取逻辑复用 `AppState::site_logs`：
/// 只看文件尾部 2MB 窗口，时间倒序，关键字只作用于该窗口。
pub async fn site_logs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Path(id): Path<i64>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let domain = site_domain(&state, id)?;
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
            "id": id,
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
        .route("/ui/v1/sites/{id}", get(site_detail))
        .route("/ui/v1/sites/{id}/default", post(site_set_default))
        .route("/ui/v1/sites/{id}/domains", post(site_domains))
        .route("/ui/v1/sites/{id}/config", post(site_save_config))
        .route("/ui/v1/sites/{id}/policy", post(site_policy))
        .route("/ui/v1/sites/{id}/ssl", post(site_ssl))
        .route("/ui/v1/sites/{id}/backup", post(site_backup))
        .route("/ui/v1/sites/{id}/delete", post(site_delete))
        .route("/ui/v1/sites/{id}/logs", get(site_logs))
        .route("/ui/v1/sites/{id}/{action}", post(site_power))
}
