//! `/ui/v1`：budding 前端运行时（SDUI）的 JSON 命名空间。
//!
//! 与 `handlers::api`（`/api/v1` 机器 API）并列，但服务对象不同：这里只给页面渲染运行时用，
//! 会话 Cookie + CSRF 鉴权，**未认证一律 401 JSON**（`UiAuthUser` 提取器），不做页面 303；
//! 失败载荷沿用 `AppError` 的同一套信封（`code/status/message/field/retryable`），
//! 不另造错误形状（URL 契约 §6、SDUI 集成 §3.2/§5.2）。
//!
//! 本批只落地会话引导与登出：壳页启动时据此判定登录态、取 CSRF、入口前缀与能力开关。

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{ApiError, AppError};
use crate::handlers::auth::{
    issue_login_csrf, login_redirect_target, verify_login_csrf, LOGIN_CSRF_TTL_SECS,
};
use crate::handlers::{self, LOGIN_CSRF_COOKIE};
use crate::middleware::UiAuthUser;
use crate::state::auth::{LoginError, LoginRequest};
use crate::state::AppState;

/// 接口版本：与路径段 `v1` 一致，前端只做兼容判断（SDUI 集成 §3.7）。
pub const API_VERSION: &str = "v1";

/// `GET /ui/v1/session` 的响应数据。
#[derive(Debug, Serialize)]
pub struct SessionData {
    pub username: String,
    pub role: String,
    /// 与页面写操作同源的 CSRF 令牌（`AppState::csrf_token`）。
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
    /// 壳页启动时需要知道的能力开关（字段只增不改）。
    pub features: SessionFeatures,
}

#[derive(Debug, Serialize)]
pub struct SessionFeatures {
    /// 页面深链由内核回落壳页（URL 契约 §2.1）。
    pub shell_fallback: bool,
    /// 尚未适配的页面用统一占位呈现。
    pub placeholders: bool,
    /// 插件功能页仍走 iframe 外壳路由（PLUGIN_UI_CONTRACT）。
    pub plugin_iframe: bool,
}

/// `GET /ui/v1/auth/login` 的响应数据：登录页渲染前只需要 CSRF 与入口前缀。
#[derive(Debug, Serialize)]
pub struct LoginConfigData {
    pub csrf_token: String,
    pub entry_prefix: String,
    pub api_version: &'static str,
}

/// `GET /ui/v1/auth/login`：签发登录 CSRF（未认证可读）。
///
/// 与页面登录共用同一个签名 token 与 `skiff_login` 兼容 Cookie，不新造第二种
/// CSRF 生成方式；响应禁止缓存，避免代理把旧 token 回放给下一个用户。
pub async fn login_config(State(state): State<Arc<AppState>>, parts: Parts) -> Response {
    let csrf = issue_login_csrf(&state);
    let secure = handlers::is_secure_request(&state, &parts);
    let mut resp = Json(json!({
        "data": LoginConfigData {
            csrf_token: csrf.clone(),
            entry_prefix: state.cfg.entry_prefix.clone(),
            api_version: API_VERSION,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut().append(
        SET_COOKIE,
        handlers::simple_cookie(LOGIN_CSRF_COOKIE, &csrf, LOGIN_CSRF_TTL_SECS, secure)
            .parse()
            .unwrap(),
    );
    resp
}

/// `POST /ui/v1/auth/login` 的请求体（wire 形状）。
#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub otp: String,
    /// 登录页签发的签名 CSRF token（与表单路径同一个）。
    #[serde(default)]
    pub csrf: String,
    /// 登录成功后的回跳深链（只接受站内绝对路径）。
    #[serde(default)]
    pub next: String,
}

/// `POST /ui/v1/auth/login`：budding 登录页的提交入口。
///
/// 认证判定、会话签发与 Cookie 形态完全复用现有表单链路（`AppState::login` +
/// `skiff_identity::session_cookie`），这里只把结果换成 JSON 与 `redirect_to`。
pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    parts: Parts,
    Json(body): Json<LoginBody>,
) -> Result<Response, ApiError> {
    verify_login_csrf(&state, &parts, &body.csrf)?;
    let ip = handlers::client_ip(&state, &parts);
    let req = LoginRequest {
        username: body.username.clone(),
        password: body.password,
        otp: body.otp,
    };
    let success = state
        .login(&req, &ip, "session", &handlers::host_header(&parts))
        .await
        .map_err(|e| {
            ApiError(match e {
                LoginError::RateLimited(wait) => {
                    AppError::Unavailable(format!("登录失败次数过多，请 {wait} 秒后重试"))
                }
                LoginError::Invalid(message) => AppError::Unauthorized(message),
                LoginError::Internal(message) => AppError::Internal(message),
            })
        })?;

    let target = login_redirect_target(&state, &body.next, success.must_change_password);
    let mut resp = Json(json!({
        "data": {
            "redirect_to": target,
            "username": success.username,
            "role": success.role.to_string(),
        }
    }))
    .into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::session_cookie(
            state.cookie_name(),
            &success.session_token,
            state.cfg.session.abs_secs,
            handlers::is_secure_request(&state, &parts),
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}

/// 概览卡片的计数（只读；数据来自 `dashboard::load_overview` 的既有查询）。
#[derive(Debug, Serialize)]
pub struct OverviewCounts {
    pub collected_at: String,
    pub docker: OverviewDocker,
    pub containers: OverviewContainers,
    pub sites: OverviewSites,
    pub plugins: OverviewPlugins,
    pub runtimes: OverviewRuntimes,
}

#[derive(Debug, Serialize)]
pub struct OverviewDocker {
    pub status: OverviewStatus,
    pub reason: String,
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Serialize)]
pub struct OverviewStatus {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct OverviewContainers {
    pub running: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewSites {
    pub total: usize,
    pub abnormal: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewPlugins {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewRuntimes {
    pub total: usize,
    pub abnormal: usize,
    pub uninstalled: usize,
}

/// `GET /ui/v1/overview`：只读概览计数。
///
/// 复用 `GET /{p}` 的同一批查询（容器列表 + 插件/站点/运行环境列表），不新增聚合 SQL；
/// 本批不含列表、写操作、图表或实时刷新，图表位由前端 schema 明确占位。
pub async fn overview(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let data = crate::handlers::dashboard::load_overview(&state, user.user.id).await?;
    let containers = data.containers;
    let running = containers.iter().filter(|c| c.state == "running").count();
    let plugin_enabled = data.plugins.iter().filter(|p| p.enabled).count();
    let site_abnormal = data.sites.iter().filter(|s| s.status != "running").count();
    let mut runtime_abnormal = 0usize;
    let mut runtime_uninstalled = 0usize;
    for r in &data.runtimes {
        let name = if r.kind == skiff_store::KIND_PHP {
            skiff_app::sites::runtime_container_name(&r.kind, &r.name)
        } else {
            format!("skiff-runtime-{}", r.id)
        };
        match containers.iter().find(|c| c.name == name) {
            Some(c) if c.state != "running" => runtime_abnormal += 1,
            None => runtime_uninstalled += 1,
            Some(_) => {}
        }
    }
    let docker = if data.docker_ok {
        OverviewDocker {
            status: OverviewStatus {
                code: "available",
                label: "可用",
            },
            reason: String::new(),
        }
    } else {
        OverviewDocker {
            status: OverviewStatus {
                code: "unavailable",
                label: "不可用",
            },
            reason: data.docker_reason,
        }
    };
    let collected_at = skiff_app::time_fmt::epoch_to_rfc3339_utc(chrono::Utc::now().timestamp())
        .unwrap_or_default();
    let counts = OverviewCounts {
        collected_at,
        docker,
        containers: OverviewContainers {
            running,
            total: containers.len(),
        },
        sites: OverviewSites {
            total: data.sites.len(),
            abnormal: site_abnormal,
        },
        plugins: OverviewPlugins {
            total: data.plugins.len(),
            enabled: plugin_enabled,
            disabled: data.plugins.len().saturating_sub(plugin_enabled),
        },
        runtimes: OverviewRuntimes {
            total: data.runtimes.len(),
            abnormal: runtime_abnormal,
            uninstalled: runtime_uninstalled,
        },
    };
    let mut resp = Json(json!({ "data": counts })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 列表接口公共查询参数（D10）：`limit` 默认 50、上限 200；`cursor` 为上一页末项的
/// 不透明键（站点 `created_at:id`，运行环境 `id`）。
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// 站点列表关键字，与页面同名参数一致（域名 / 目录 / 接入方式 / 别名）。
    pub q: Option<String>,
}

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

fn page_limit(q: &ListQuery) -> usize {
    q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn rfc3339(epoch: i64) -> String {
    skiff_app::time_fmt::epoch_to_rfc3339_utc(epoch).unwrap_or_default()
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub code: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteHttps {
    pub enabled: bool,
    pub force_https: bool,
    pub label: String,
    pub class: String,
}

/// 站点列表项（字段冻结，只增不改；与页面 `SiteView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteItem {
    pub domain: String,
    pub root_dir: String,
    pub index_dir: String,
    pub site_type: String,
    pub type_label: String,
    pub runtime_id: String,
    pub runtime_label: String,
    pub runtime_name: String,
    pub runtime_shared_hint: String,
    pub proxy_target: String,
    pub proxy_websocket: bool,
    pub endpoint: String,
    pub status: StatusDto,
    pub https: SiteHttps,
    pub is_default: bool,
    pub aliases: Vec<String>,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
}

fn site_item(
    state: &AppState,
    s: &skiff_store::Site,
    runtimes: &[skiff_store::Runtime],
    views: &[skiff_app::sites::RuntimeView],
    cert_ready: bool,
) -> SiteItem {
    let runtime = s
        .runtime_id
        .as_deref()
        .and_then(|id| runtimes.iter().find(|r| r.id == id));
    let view = s
        .runtime_id
        .as_deref()
        .and_then(|id| views.iter().find(|v| v.runtime.id == id));
    let v = handlers::sites::site_view(
        s,
        state.site_aliases(&s.domain),
        cert_ready,
        runtime,
        view.map(|v| v.effective_state.as_str()),
        view.map(|v| v.site_count).unwrap_or(0),
    );
    SiteItem {
        domain: v.domain,
        root_dir: v.root_dir,
        index_dir: v.index_dir,
        site_type: v.site_type,
        type_label: v.type_label,
        runtime_id: v.runtime_id,
        runtime_label: v.runtime_label,
        runtime_name: v.runtime_name,
        runtime_shared_hint: v.runtime_shared_hint,
        proxy_target: v.proxy_target,
        proxy_websocket: v.proxy_websocket,
        endpoint: v.endpoint,
        status: StatusDto {
            code: v.status,
            label: v.status_label,
        },
        https: SiteHttps {
            enabled: v.https,
            force_https: v.force_https,
            label: v.https_label,
            class: v.https_class,
        },
        is_default: v.is_default,
        aliases: v.aliases,
        created_at: rfc3339(s.created_at),
    }
}

fn parse_site_cursor(cursor: &str) -> Option<(i64, i64)> {
    let (created, id) = cursor.split_once(':')?;
    Some((created.parse().ok()?, id.parse().ok()?))
}

/// `GET /ui/v1/sites`：站点列表（只读）。
///
/// 复用页面同一批查询与 `site_view` 展示模型，不新增 SQL；`q` 过滤与页面一致，
/// 分页为内存 keyset（排序沿用 `list_sites` 的 `created_at DESC, id DESC`）。
pub async fn sites(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
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
        "data": { "items": items, "total": total, "next_cursor": next_cursor }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 站点详情里的证书信息（只读）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteSsl {
    pub enabled: bool,
    pub force_https: bool,
    pub cert_exists: bool,
    pub cert_path: String,
    pub key_path: String,
    pub subject: String,
    pub issuer: String,
    pub expires: String,
}

/// 站点详情里绑定的运行环境（只读快照）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteRuntimeRef {
    pub id: String,
    pub name: String,
    pub kind: StatusDto,
    pub version: String,
    pub state: StatusDto,
    pub site_count: usize,
}

/// 站点详情响应数据（列表项字段 + 证书 + 运行环境状态）。
#[derive(Debug, Serialize)]
pub struct SiteDetailData {
    #[serde(flatten)]
    pub site: SiteItem,
    pub ssl: SiteSsl,
    pub runtime: Option<SiteRuntimeRef>,
    pub runtime_down: bool,
    pub runtime_alert: String,
}

/// `GET /ui/v1/sites/{domain}`：站点详情（只读）。
///
/// 只读分支复用详情页的既有查询（站点记录、运行环境实时视图、证书信息），
/// 配置/目录/域名等标签页的写操作留给后续批次。
pub async fn site_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
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
    let data = SiteDetailData {
        site: item,
        ssl,
        runtime: runtime_ref,
        runtime_down,
        runtime_alert,
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 运行环境最近一次异步任务（安装/扩展）；字段与页面抽屉同源。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeJobItem {
    pub state: String,
    pub label: String,
    pub step: String,
    pub progress: u8,
    pub error: String,
    pub active: bool,
    pub failed: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub started_at: String,
    pub updated_at: String,
}

/// 运行环境列表项（字段冻结，只增不改；与页面 `RuntimeRow` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeItem {
    pub id: String,
    pub kind: StatusDto,
    pub name: String,
    pub container_name: String,
    pub version: String,
    pub version_display: String,
    pub version_note: String,
    pub image: String,
    pub state: StatusDto,
    pub state_class: String,
    pub drift: bool,
    pub site_count: usize,
    pub container_state: String,
    pub endpoint_short: String,
    pub endpoint_full: String,
    pub implemented: bool,
    pub is_default: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
    pub updated_at: String,
    pub job: Option<RuntimeJobItem>,
}

/// `GET /ui/v1/runtimes`：运行环境列表与安装状态（只读，管理员）。
///
/// 权限与页面一致（`require_admin`）；列表过滤沿用 `RuntimeView::is_listed`，
/// 展示字段来自页面同一个 `runtime_row`，不新增 SQL。
pub async fn runtimes(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let views = state.runtime_views().await.unwrap_or_default();
    let needle = q.q.as_deref().unwrap_or("").trim().to_lowercase();
    let rows: Vec<(String, RuntimeItem)> = views
        .iter()
        .filter(|v| v.is_listed())
        .filter(|v| {
            needle.is_empty()
                || v.runtime.id.to_lowercase().contains(&needle)
                || v.runtime.name.to_lowercase().contains(&needle)
                || skiff_app::runtime_catalog::kind_label(&v.runtime.kind)
                    .to_lowercase()
                    .contains(&needle)
        })
        .map(|v| {
            let row = handlers::sites::runtime_row(&state, v);
            let job = v.job.as_ref().map(|j| RuntimeJobItem {
                state: j.state.as_str().to_string(),
                label: crate::labels::runtime_job_state(j.state).to_string(),
                step: j.step.clone(),
                progress: j.progress,
                error: j.error.clone(),
                active: j.active(),
                failed: j.state == skiff_app::sites::RuntimeJobState::Failed,
                started_at: rfc3339(j.started_at),
                updated_at: rfc3339(j.updated_at),
            });
            (
                v.runtime.id.clone(),
                RuntimeItem {
                    id: row.id,
                    kind: StatusDto {
                        code: row.kind_key,
                        label: row.kind_label,
                    },
                    name: row.name,
                    container_name: row.container_name,
                    version: row.version,
                    version_display: row.version_display,
                    version_note: row.version_note,
                    image: row.image,
                    state: StatusDto {
                        code: v.effective_state.clone(),
                        label: row.state_label,
                    },
                    state_class: row.state_class,
                    drift: row.drift,
                    site_count: row.site_count,
                    container_state: row.container_state,
                    endpoint_short: row.endpoint_short,
                    endpoint_full: row.endpoint_full,
                    implemented: row.implemented,
                    is_default: row.is_default,
                    created_at: rfc3339(v.runtime.created_at),
                    updated_at: rfc3339(v.runtime.updated_at),
                    job,
                },
            )
        })
        .collect();
    let total = rows.len();
    let start = match q.cursor.as_deref() {
        Some(cursor) => rows
            .iter()
            .position(|(id, _)| id == cursor)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let end = (start + page_limit(&q)).min(total);
    let next_cursor = if end < total && end > start {
        rows.get(end - 1).map(|(id, _)| id.clone())
    } else {
        None
    };
    let items: Vec<RuntimeItem> = rows
        .into_iter()
        .skip(start)
        .take(end - start)
        .map(|(_, item)| item)
        .collect();
    let mut resp = Json(json!({
        "data": { "items": items, "total": total, "next_cursor": next_cursor }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 容器状态中文文案；与旧页面 CSS 的 `.state.<code>` 语义一致。
fn container_state_label(state: &str) -> &'static str {
    match state {
        "running" => "运行中",
        "created" => "已创建",
        "exited" => "已退出",
        "restarting" => "重启中",
        "paused" => "已暂停",
        "dead" => "已终止",
        "removing" => "删除中",
        _ => "状态未知",
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ContainerListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// `error` 只保留异常容器（概览待办深链），其余值视为不筛选。
    pub filter: Option<String>,
}

/// 容器列表项（字段冻结，只增不改；与旧表格同一 `container_view`）。
#[derive(Debug, Clone, Serialize)]
pub struct ContainerItem {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub state: StatusDto,
    pub status: String,
    pub project: String,
    pub protected: bool,
    pub unprotected_kernel: bool,
}

fn container_item(c: &skiff_docker::client::ContainerRow) -> ContainerItem {
    let v = handlers::container_view(c);
    ContainerItem {
        id: v.id,
        short_id: v.short_id,
        name: v.name,
        image: v.image,
        state: StatusDto {
            code: v.state.clone(),
            label: container_state_label(&v.state).to_string(),
        },
        status: v.status,
        project: v.project,
        protected: v.protected,
        unprotected_kernel: v.unprotected_kernel,
    }
}

/// `GET /ui/v1/containers`：容器列表（只读）。
///
/// 与旧页面同一份 `list_containers` + `container_view`，支持 `filter=error`；
/// Docker 不可用时返回空列表与原因（页面同口径），不臆造状态。
pub async fn containers(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ContainerListQuery>,
) -> Result<Response, ApiError> {
    let docker_ok = state.docker().available();
    let rows = if docker_ok {
        state
            .docker()
            .list_containers()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
    } else {
        Vec::new()
    };
    let filter = match q.filter.as_deref() {
        Some("error") => "error",
        _ => "",
    };
    let items: Vec<ContainerItem> = rows
        .iter()
        .filter(|c| filter.is_empty() || handlers::container_abnormal(&c.state))
        .map(container_item)
        .collect();
    let total = items.len();
    let start = match q.cursor.as_deref() {
        Some(cursor) => items
            .iter()
            .position(|c| c.id == cursor)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let end = (start + q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)).min(total);
    let next_cursor = if end < total && end > start {
        items.get(end - 1).map(|c| c.id.clone())
    } else {
        None
    };
    let page: Vec<ContainerItem> = items.into_iter().skip(start).take(end - start).collect();
    let mut resp = Json(json!({
        "data": {
            "items": page,
            "total": total,
            "next_cursor": next_cursor,
            "filter": filter,
            "docker_available": docker_ok,
            "docker_reason": state.docker_unavailable_reason(),
            "can_operate": user.can(skiff_model::Permission::DockerOperate),
            "can_delete": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerPort {
    pub host: String,
    pub container: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerMount {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLabel {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLimitLine {
    pub label: String,
    pub value: String,
}

/// 资源限制的原始表单值（字符串保留「默认/不限制」语义）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ContainerLimits {
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
}

/// 容器详情（只读）：基本信息按块拆分，字段与旧详情页同源。
#[derive(Debug, Serialize)]
pub struct ContainerDetailData {
    pub container: ContainerItem,
    pub image: String,
    pub cmd: String,
    /// RFC3339 UTC（D9）；Docker 返回原值。
    pub created_at: String,
    pub health: String,
    pub ports: Vec<ContainerPort>,
    pub mounts: Vec<ContainerMount>,
    pub networks: Vec<String>,
    pub labels: Vec<ContainerLabel>,
    pub limits: ContainerLimits,
    pub limits_effective: Vec<ContainerLimitLine>,
    pub can_exec: bool,
    pub can_operate: bool,
    pub can_delete: bool,
    pub docker_reason: String,
}

/// `GET /ui/v1/containers/{id}`：容器详情（只读）。
pub async fn container_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    state
        .cfg
        .policy
        .check(
            &handlers::actor(&user),
            skiff_docker::Verb::ContainerInspect,
            Some(&handlers::object_ref(&row)),
        )
        .map_err(|e| ApiError(AppError::Forbidden(e.to_string())))?;
    let inspect = state
        .docker()
        .inspect_container(&row.id)
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;

    let ports = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.ports.clone())
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| {
                    let (port, proto) = k.split_once('/').unwrap_or((k.as_str(), "tcp"));
                    let host = v
                        .as_ref()
                        .and_then(|b| b.first())
                        .and_then(|b| b.host_port.clone())
                        .unwrap_or_default();
                    ContainerPort {
                        host,
                        container: port.to_string(),
                        protocol: proto.to_string(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mounts = inspect
        .mounts
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| ContainerMount {
                    kind: m.typ.clone().map(|t| t.to_string()).unwrap_or_default(),
                    source: m.source.clone().unwrap_or_default(),
                    target: m.destination.clone().unwrap_or_default(),
                    read_only: m.rw == Some(false),
                })
                .collect()
        })
        .unwrap_or_default();
    let networks = inspect
        .network_settings
        .as_ref()
        .and_then(|n| n.networks.clone())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let labels = inspect
        .config
        .as_ref()
        .and_then(|c| c.labels.clone())
        .map(|m| {
            let mut pairs: Vec<ContainerLabel> = m
                .into_iter()
                .map(|(key, value)| ContainerLabel { key, value })
                .collect();
            pairs.sort_by(|a, b| a.key.cmp(&b.key));
            pairs
        })
        .unwrap_or_default();
    let cmd = inspect
        .config
        .as_ref()
        .and_then(|c| c.cmd.clone())
        .map(|v| v.join(" "))
        .unwrap_or_default();
    let health = inspect
        .state
        .as_ref()
        .and_then(|s| s.health.as_ref())
        .and_then(|h| h.status)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());

    let hc = inspect.host_config.unwrap_or_default();
    let mb = |bytes: Option<i64>| match bytes {
        Some(v) if v < 0 => "不限制".to_string(),
        Some(0) | None => "默认".to_string(),
        Some(v) => format!("{} MB", v / (1024 * 1024)),
    };
    let limits_effective = vec![
        ContainerLimitLine {
            label: "CPU 份额".into(),
            value: hc
                .cpu_shares
                .map(|v| v.to_string())
                .filter(|v| v != "0")
                .unwrap_or_else(|| "默认".into()),
        },
        ContainerLimitLine {
            label: "CPU 周期".into(),
            value: hc
                .cpu_period
                .map(|v| format!("{v} μs"))
                .filter(|v| v != "0 μs")
                .unwrap_or_else(|| "默认".into()),
        },
        ContainerLimitLine {
            label: "CPU 配额".into(),
            value: match hc.cpu_quota {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => format!("{v} μs/周期"),
            },
        },
        ContainerLimitLine {
            label: "内存上限".into(),
            value: mb(hc.memory),
        },
        ContainerLimitLine {
            label: "内存+交换".into(),
            value: mb(hc.memory_swap),
        },
        ContainerLimitLine {
            label: "PIDs 上限".into(),
            value: match hc.pids_limit {
                Some(-1) => "不限制".to_string(),
                Some(0) | None => "默认".to_string(),
                Some(v) => v.to_string(),
            },
        },
    ];
    let limits = ContainerLimits {
        cpu_shares: hc.cpu_shares.map(|v| v.to_string()).unwrap_or_default(),
        cpu_period: hc.cpu_period.map(|v| v.to_string()).unwrap_or_default(),
        cpu_quota: hc.cpu_quota.map(|v| v.to_string()).unwrap_or_default(),
        memory_limit_mb: hc
            .memory
            .filter(|v| *v >= 0)
            .map(|v| (v / (1024 * 1024)).to_string())
            .unwrap_or_default(),
        memory_swap_mb: hc
            .memory_swap
            .map(|v| {
                if v < 0 {
                    v.to_string()
                } else {
                    (v / (1024 * 1024)).to_string()
                }
            })
            .unwrap_or_default(),
        pids_limit: hc.pids_limit.map(|v| v.to_string()).unwrap_or_default(),
    };

    let data = ContainerDetailData {
        container: container_item(&row),
        image: row.image.clone(),
        cmd,
        created_at: inspect.created.clone().unwrap_or_default(),
        health,
        ports,
        mounts,
        networks,
        labels,
        limits,
        limits_effective,
        can_exec: user.can(skiff_model::Permission::DockerExec),
        can_operate: user.can(skiff_model::Permission::DockerOperate),
        can_delete: user.can(skiff_model::Permission::DockerLifecycle),
        docker_reason: state.docker_unavailable_reason().to_string(),
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/containers/{id}/{action}`：启动/停止/重启/删除。
///
/// 与旧 htmx 路径同一策略判定、同一审计与同一命令；CSRF 用 `/ui/v1` 统一的
/// `x-csrf-token` 头（与登出同源），仅传输层改为 JSON。
pub async fn container_action(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let verb = handlers::containers::action_verb(&action).map_err(ApiError)?;
    let row = handlers::resolve_container(&state, &id)
        .await
        .map_err(ApiError)?;
    let obj = handlers::object_ref(&row);
    state
        .cfg
        .policy
        .check(&handlers::actor(&user), verb, Some(&obj))
        .map_err(|e| {
            state.audit_or_log(
                &user.user.username,
                &handlers::client_ip(&state, &parts),
                &verb.audit_action(),
                &row.name,
                "deny",
                &e.to_string(),
            );
            ApiError(AppError::Forbidden(e.to_string()))
        })?;
    let cmd = skiff_app::commands::docker::ContainerAction {
        target: skiff_app::commands::docker::ContainerTarget::new(
            row.name.clone(),
            row.short_id(),
            obj,
        ),
        verb,
        actor: handlers::containers::actor(&state, &user, &parts),
    };
    skiff_app::commands::docker::container_action(&state, cmd)
        .await
        .map_err(|e| ApiError(handlers::containers::map_err(e)))?;
    let removed = verb == skiff_docker::Verb::ContainerRemove;
    let mut resp = Json(json!({
        "data": {
            "id": row.id,
            "name": row.name,
            "action": action,
            "removed": removed,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/session`：当前用户、角色、CSRF、入口前缀与壳页能力开关。
///
/// 未登录由 `UiAuthUser` 返回 401 JSON（`auth.unauthenticated`）。
pub async fn session(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Json<serde_json::Value> {
    let data = SessionData {
        username: user.user.username.clone(),
        role: user.role().to_string(),
        csrf_token: state.csrf_token(&user.session_token),
        entry_prefix: state.cfg.entry_prefix.clone(),
        api_version: API_VERSION,
        features: SessionFeatures {
            shell_fallback: true,
            placeholders: true,
            plugin_iframe: true,
        },
    };
    Json(json!({ "data": data }))
}

/// `POST /ui/v1/session/logout`：撤销当前会话并清 Cookie。
///
/// 写路径 CSRF 与页面写操作同源：令牌放 `x-csrf-token` 头，判定复用
/// `middleware::check_csrf`（不新造第二套）。失败返回 403 JSON `auth.csrf_invalid`。
pub async fn logout(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let ip = handlers::client_ip(&state, &parts);
    state
        .logout(&user.user.username, &user.session_id, &ip)
        .map_err(|e| ApiError(AppError::Internal(e)))?;

    let mut resp = Json(json!({ "data": { "ok": true } })).into_response();
    resp.headers_mut().append(
        SET_COOKIE,
        skiff_identity::clear_cookie(
            state.cookie_name(),
            handlers::is_secure_request(&state, &parts),
        )
        .parse()
        .unwrap(),
    );
    Ok(resp)
}
