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

/// 内存 keyset 分页：`rows` 已按展示顺序排列，游标为上一页末项的键。
fn paginate<T>(
    rows: Vec<(String, T)>,
    limit: usize,
    cursor: Option<&str>,
) -> (Vec<T>, Option<String>, usize) {
    let total = rows.len();
    let start = match cursor {
        Some(cursor) => rows
            .iter()
            .position(|(key, _)| key == cursor)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let end = (start + limit).min(total);
    let next_cursor = if end < total && end > start {
        rows.get(end - 1).map(|(key, _)| key.clone())
    } else {
        None
    };
    let page = rows
        .into_iter()
        .skip(start)
        .take(end - start)
        .map(|(_, item)| item)
        .collect();
    (page, next_cursor, total)
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

/// 镜像 / 网络 / 卷的通用行。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceItem {
    pub id: String,
    pub name: String,
    pub meta: String,
    pub warn: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkContainerItem {
    pub name: String,
    pub ipv4: String,
    pub ipv6: String,
    pub mac: String,
}

/// 网络行（字段冻结，只增不改；与旧页面 `NetworkRowView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkItem {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub ipam_driver: String,
    pub subnet: String,
    pub gateway: String,
    pub internal: String,
    pub attachable: String,
    pub container_count: String,
    pub containers_known: bool,
    pub containers: Vec<NetworkContainerItem>,
    pub builtin: bool,
    pub can_remove: bool,
}

fn network_item(v: crate::templates::NetworkRowView) -> NetworkItem {
    NetworkItem {
        id: v.id,
        name: v.name,
        driver: v.driver,
        scope: v.scope,
        ipam_driver: v.ipam_driver,
        subnet: v.subnet,
        gateway: v.gateway,
        internal: v.internal,
        attachable: v.attachable,
        container_count: v.container_count,
        containers_known: v.containers_known,
        containers: v
            .containers
            .into_iter()
            .map(|c| NetworkContainerItem {
                name: c.name,
                ipv4: c.ipv4,
                ipv6: c.ipv6,
                mac: c.mac,
            })
            .collect(),
        builtin: v.builtin,
        can_remove: v.can_remove,
    }
}

/// `GET /ui/v1/images`：镜像列表（只读）。
pub async fn images(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_images()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
            .into_iter()
            .map(|i| {
                let name = if i.tags.is_empty() {
                    format!("<none> ({})", i.short_id())
                } else {
                    i.tags.join(", ")
                };
                (
                    i.id.clone(),
                    ResourceItem {
                        id: i.id,
                        name,
                        meta: crate::util::human_size(i.size),
                        warn: String::new(),
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/networks`：网络列表（只读）。
pub async fn networks(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_networks()
            .await
            .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?
            .into_iter()
            .map(|n| {
                let item = network_item(handlers::resources::network_view(n));
                (item.id.clone(), item)
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/volumes`：卷列表（只读）。
pub async fn volumes(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let available = state.docker().available();
    let rows = if available {
        state
            .docker()
            .list_volumes()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|v| {
                let name = v.name.clone();
                (
                    name.clone(),
                    ResourceItem {
                        id: name.clone(),
                        name,
                        meta: format!("{} → {}", v.driver, v.mountpoint),
                        warn: if v.host_binding {
                            "宿主绑定语义".to_string()
                        } else {
                            String::new()
                        },
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": {
            "items": items,
            "total": total,
            "next_cursor": next_cursor,
            "docker_available": available,
            "docker_reason": state.docker_unavailable_reason(),
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Serialize)]
pub struct ComposeProjectItem {
    pub name: String,
    pub managed: bool,
    pub services: usize,
    pub running: usize,
}

#[derive(Debug, Serialize)]
pub struct ComposeServiceItem {
    pub service: String,
    pub container: String,
    pub id: String,
    pub state: StatusDto,
    pub status: String,
    pub image: String,
    pub managed: bool,
    pub logs: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ComposeQuery {
    pub project: Option<String>,
}

/// `GET /ui/v1/compose`：compose 项目列表；选中项目时附带服务与最近日志。
///
/// 与旧页面同一 `status_of`（容器标签归组，不持久化 compose 源文件）。
pub async fn compose(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ComposeQuery>,
) -> Result<Response, ApiError> {
    let selected = q.project.unwrap_or_default();
    let (projects, services, docker_error) = handlers::compose::status_of(&state, &selected).await;
    let projects: Vec<ComposeProjectItem> = projects
        .into_iter()
        .map(|p| ComposeProjectItem {
            name: p.name,
            managed: p.managed,
            services: p.services,
            running: p.running,
        })
        .collect();
    let services: Vec<ComposeServiceItem> = services
        .into_iter()
        .map(|s| ComposeServiceItem {
            service: s.service,
            container: s.container,
            id: s.id,
            state: StatusDto {
                code: s.state.clone(),
                label: container_state_label(&s.state).to_string(),
            },
            status: s.status,
            image: s.image,
            managed: s.managed,
            logs: s.logs,
        })
        .collect();
    let mut resp = Json(json!({
        "data": {
            "projects": projects,
            "services": services,
            "selected_project": selected,
            "docker_available": state.docker().available(),
            "docker_reason": docker_error,
            "can_admin": user.can(skiff_model::Permission::DockerLifecycle),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct ComposeDownBody {
    pub project: String,
    #[serde(default)]
    pub remove_volumes: bool,
}

/// `POST /ui/v1/compose/down`：清理内核托管项目（外部项目只读）。
pub async fn compose_down(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<ComposeDownBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let project = skiff_app::commands::docker::prepare_project(&body.project)
        .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let value = skiff_app::commands::docker::compose_down(
        &state,
        skiff_app::commands::docker::ComposeDown {
            project: project.clone(),
            remove_volumes: body.remove_volumes,
            actor: handlers::compose::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let mut resp = Json(json!({
        "data": {
            "project": project,
            "remove_volumes": body.remove_volumes,
            "report": value,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/compose/{project}/{action}`：项目级启动/停止/重启。
///
/// 逐容器复用既有动词分发与权限判定；策略拒绝计入 `failed`，不中断其余容器。
pub async fn compose_project_action(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((project, action)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::compose::require_compose_admin(&state, &user).map_err(ApiError)?;
    let verb = match action.as_str() {
        "start" => skiff_docker::Verb::Start,
        "stop" => skiff_docker::Verb::Stop,
        "restart" => skiff_docker::Verb::Restart,
        _ => return Err(ApiError(AppError::BadRequest("未知的项目动作".into()))),
    };
    let project = skiff_app::commands::docker::prepare_project(&project)
        .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let rows = state
        .docker()
        .list_containers()
        .await
        .map_err(|e| ApiError(AppError::Unavailable(e.to_string())))?;
    let mut targets = Vec::new();
    let mut failed = 0usize;
    for row in rows
        .iter()
        .filter(|r| skiff_docker::project_of(&r.labels) == Some(project.as_str()))
    {
        let obj = handlers::object_ref(row);
        if state
            .cfg
            .policy
            .check(&handlers::actor(&user), verb, Some(&obj))
            .is_err()
        {
            failed += 1;
            continue;
        }
        targets.push(skiff_app::commands::docker::ContainerTarget::new(
            row.name.clone(),
            row.short_id(),
            obj,
        ));
    }
    let report = skiff_app::commands::docker::compose_project_action(
        &state,
        skiff_app::commands::docker::ProjectAction {
            project: project.clone(),
            targets,
            verb,
            actor: handlers::compose::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| ApiError(handlers::compose::map_err(e)))?;
    let mut resp = Json(json!({
        "data": {
            "project": project,
            "action": action,
            "done": report.done,
            "failed": failed + report.failed,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskItem {
    pub id: String,
    pub name: String,
    pub expr: String,
    pub next_text: String,
    pub source: String,
    pub target: String,
    pub method: String,
    pub enabled: bool,
    pub state_label: String,
    pub state_class: String,
    pub last_text: String,
    pub last_class: String,
    pub builtin: bool,
}

fn task_item(v: &serde_json::Value) -> TaskItem {
    let r = handlers::tasks::row_of(v);
    TaskItem {
        id: r.id,
        name: r.name,
        expr: r.expr,
        next_text: r.next_text,
        source: r.source,
        target: r.target,
        method: r.method,
        enabled: r.enabled,
        state_label: r.state_label,
        state_class: r.state_class,
        last_text: r.last_text,
        last_class: r.last_class,
        builtin: r.builtin,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRunItem {
    pub trigger: String,
    pub started_text: String,
    pub duration: String,
    pub result: String,
    pub result_class: String,
    pub detail: String,
    pub missed: i64,
}

/// `GET /ui/v1/tasks`：定时任务列表（只读）。
pub async fn tasks(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let svc = handlers::tasks::service(&state).map_err(ApiError)?;
    let needle = q.q.as_deref().unwrap_or("").trim().to_lowercase();
    let views = svc.list_views().await;
    let rows: Vec<(String, TaskItem)> = views
        .iter()
        .map(task_item)
        .filter(|t| {
            needle.is_empty()
                || t.name.to_lowercase().contains(&needle)
                || t.id.to_lowercase().contains(&needle)
                || t.target.to_lowercase().contains(&needle)
                || t.source.to_lowercase().contains(&needle)
        })
        .map(|t| (t.id.clone(), t))
        .collect();
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
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

/// `GET /ui/v1/tasks/{id}`：任务详情 + 最近 50 次执行（只读）。
pub async fn task_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let svc = handlers::tasks::service(&state).map_err(ApiError)?;
    let view = svc
        .detail_view(&id, 50)
        .await
        .ok_or_else(|| ApiError(AppError::NotFound(format!("任务不存在：{id}"))))?;
    let task = task_item(&view["task"]);
    let payload = view["task"]["payload"].clone();
    let runs: Vec<TaskRunItem> = view["runs"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| {
                    let r = handlers::tasks::run_row(v);
                    TaskRunItem {
                        trigger: r.trigger,
                        started_text: r.started_text,
                        duration: r.duration,
                        result: r.result,
                        result_class: r.result_class,
                        detail: r.detail,
                        missed: r.missed,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mut resp = Json(json!({
        "data": {
            "task": task,
            "payload": payload,
            "runs": runs,
            "can_admin": user.can(skiff_model::Permission::Admin),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Clone, Serialize)]
pub struct JobItem {
    pub id: String,
    pub title: String,
    pub kind_label: String,
    pub target_label: String,
    pub status_label: String,
    pub status_class: String,
    pub phase_label: String,
    pub progress: i64,
    pub indeterminate: bool,
    pub duration: String,
    pub created_by: String,
    /// RFC3339 UTC（D9）。
    pub created_at: String,
    pub active: bool,
}

fn job_item(job: &skiff_app::jobs::Job) -> JobItem {
    let r = handlers::jobs::row_of(job);
    JobItem {
        id: r.id,
        title: r.title,
        kind_label: r.kind_label,
        target_label: r.target_label,
        status_label: r.status_label,
        status_class: r.status_class,
        phase_label: r.phase_label,
        progress: r.progress,
        indeterminate: r.indeterminate,
        duration: r.duration,
        created_by: r.created_by,
        created_at: rfc3339(job.created_at),
        active: r.active,
    }
}

/// `GET /ui/v1/jobs`：作业列表（只读，最近 200 条）。
pub async fn jobs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let rows: Vec<(String, JobItem)> = state
        .jobs()
        .list(200)
        .iter()
        .map(|job| {
            let item = job_item(job);
            (item.id.clone(), item)
        })
        .collect();
    let (items, next_cursor, total) = paginate(rows, page_limit(&q), q.cursor.as_deref());
    let mut resp = Json(json!({
        "data": { "items": items, "total": total, "next_cursor": next_cursor }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/jobs/{id}`：作业详情 + 首屏日志尾巴（纯文本）。
pub async fn job_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let job = handlers::jobs::load_job(&state, &id).map_err(ApiError)?;
    let tail = state.jobs().tail(&job.id, 200);
    let cursor = tail.last().map(|r| r.seq).unwrap_or(0);
    let log_text = tail
        .iter()
        .map(|r| format!("[{}] {}", handlers::jobs::fmt_log_time(r.at), r.line))
        .collect::<Vec<_>>()
        .join("\n");
    let view = handlers::jobs::detail_of(&job, cursor, log_text);
    let mut resp = Json(json!({
        "data": {
            "id": view.id,
            "title": view.title,
            "kind_label": view.kind_label,
            "target_label": view.target_label,
            "status_label": view.status_label,
            "status_class": view.status_class,
            "phase_label": view.phase_label,
            "progress": view.progress,
            "indeterminate": view.indeterminate,
            "cancellable": view.cancellable,
            "cancel_requested": view.cancel_requested,
            "created_by": view.created_by,
            "created_by_role": view.created_by_role,
            "requester_ip": view.requester_ip,
            "created_at": rfc3339(job.created_at),
            "started_at": job.started_at.map(rfc3339),
            "finished_at": job.finished_at.map(rfc3339),
            "duration": view.duration,
            "exit_code": view.exit_code,
            "trigger": view.trigger,
            "error_class": view.error_class,
            "error_detail": view.error_detail,
            "result": job.result.clone(),
            "params": job.params.clone(),
            "retry_of": view.retry_of,
            "can_cancel": view.can_cancel,
            "can_retry": view.can_retry,
            "lookback": cursor,
            "logs": view.log_text,
            "can_admin": user.can(skiff_model::Permission::Admin),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 商店目录的一行（四类条目共用；字段与 `CatalogItem` 同源）。
#[derive(Debug, Serialize)]
pub struct StoreItem {
    pub name: String,
    pub version: String,
    pub source: String,
    pub updated_at: String,
    pub compatibility: String,
    pub note: String,
    pub warn: String,
    pub installable: bool,
    pub action: String,
    pub install_note: String,
}

#[derive(Debug, Serialize)]
pub struct StoreSectionItem {
    pub title: String,
    pub hint: String,
    pub installable: bool,
    pub items: Vec<StoreItem>,
}

#[derive(Debug, Serialize)]
pub struct StoreUpgradeItem {
    pub current: String,
    pub arch: String,
    pub available: String,
    pub channel: String,
    pub min_from: String,
    pub changelog_url: String,
    pub note: String,
    pub can_download: bool,
}

fn catalog_item(item: skiff_app::catalog::CatalogItem) -> StoreItem {
    StoreItem {
        name: item.name,
        version: item.version,
        source: item.source,
        updated_at: item.updated_at,
        compatibility: item.compatibility,
        note: item.note,
        warn: item.warn,
        installable: item.installable,
        action: item.action,
        install_note: item.install_note,
    }
}

fn catalog_state_code(state: skiff_app::catalog::CatalogState) -> &'static str {
    use skiff_app::catalog::CatalogState;
    match state {
        CatalogState::Disabled => "disabled",
        CatalogState::Fresh => "fresh",
        CatalogState::Cached => "cached",
        CatalogState::Offline => "offline",
        CatalogState::Rejected => "rejected",
    }
}

/// `GET /ui/v1/store?refresh=1`：商店目录只读快照（列表 / 状态 / 升级提示）。
pub async fn store(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let force = q.get("refresh").map(|v| v == "1").unwrap_or(false);
    let view = handlers::store::load_view(&state, force)
        .await
        .map_err(ApiError)?;
    let snap = view.snapshot;
    let total = snap.entries.total();
    let sections = vec![
        StoreSectionItem {
            title: "插件".into(),
            hint: "已验证目录条目可直接安装：下载 → 摘要/大小 → 验签 → 权限确认。".into(),
            installable: view.can_install,
            items: view.plugins.into_iter().map(catalog_item).collect(),
        },
        StoreSectionItem {
            title: "应用模板".into(),
            hint: "Docker 应用模板（Compose）；本页只展示清单。".into(),
            installable: false,
            items: snap.entries.apps.into_iter().map(catalog_item).collect(),
        },
        StoreSectionItem {
            title: "运行环境".into(),
            hint: "网站运行环境版本（PHP/Node/Rust 镜像）；本页只展示清单。".into(),
            installable: false,
            items: snap
                .entries
                .runtimes
                .into_iter()
                .map(catalog_item)
                .collect(),
        },
        StoreSectionItem {
            title: "升级通道".into(),
            hint: "内核升级包通道；升级操作不在本页发起。".into(),
            installable: false,
            items: snap
                .entries
                .upgrades
                .into_iter()
                .map(catalog_item)
                .collect(),
        },
    ];
    let upgrade = StoreUpgradeItem {
        current: view.upgrade.current,
        arch: view.upgrade.arch,
        available: view.upgrade.available,
        channel: view.upgrade.channel,
        min_from: view.upgrade.min_from,
        changelog_url: view.upgrade.changelog_url,
        note: view.upgrade.note,
        can_download: view.upgrade.can_download,
    };
    let mut resp = Json(json!({
        "data": {
            "state": StatusDto {
                code: catalog_state_code(snap.state).into(),
                label: crate::labels::catalog_state(snap.state).into(),
            },
            "message": snap.message,
            "channel": snap.channel,
            "seq": snap.seq.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            "fetched_at": snap.fetched_at,
            "source": snap.source,
            "allow_unsigned": snap.allow_unsigned,
            "key_configured": snap.key_configured,
            "can_install": view.can_install,
            "total": total,
            "sections": sections,
            "upgrade": upgrade,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct StoreInstallBody {
    #[serde(default)]
    pub version: Option<String>,
}

/// `POST /ui/v1/store/plugins/{id}/install`：下载并验签，返回权限确认数据。
///
/// 与旧表单入口共用 `store::prepare`、同一审计动作与同一权限（`PluginManage`）；
/// 真正的安装要再调 `POST /ui/v1/plugins/confirm`（与旧确认页同一条收尾路径）。
pub async fn store_install(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<StoreInstallBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    if !state.cfg.catalog.enabled {
        return Err(ApiError(AppError::NotFound("商店目录未启用".into())));
    }
    let ip = handlers::client_ip(&state, &parts);
    if let Err(e) = handlers::require_permission(&user, skiff_model::Permission::PluginManage) {
        handlers::store::audit(&state, &user, &ip, &id, "deny", "reason=permission");
        return Err(ApiError(e));
    }
    if state.cfg.catalog.public_key_hex().is_none() {
        handlers::store::audit(&state, &user, &ip, &id, "deny", "reason=no_key");
        return Err(ApiError(AppError::BadRequest(
            "未配置固定 Ed25519 公钥，不提供商店安装".into(),
        )));
    }
    let req = skiff_app::commands::store::StoreInstallRequest {
        plugin_id: id.clone(),
        version: body.version.filter(|v| !v.trim().is_empty()),
        form: state.cfg.form,
        allow_unsigned: state.cfg.allow_unsigned,
        allow_reduced_sandbox: state.cfg.allow_reduced_sandbox,
        data_dir: state.cfg.data_dir.clone(),
    };
    let worker = Arc::clone(&state);
    let prepared = match handlers::blocking_fs(move || {
        skiff_app::commands::store::prepare(worker.catalog(), worker.db(), &req)
    })
    .await?
    {
        Ok(prepared) => prepared,
        Err(e) => {
            handlers::store::audit(
                &state,
                &user,
                &ip,
                &id,
                "fail",
                &format!("reason={} {e}", e.reason()),
            );
            return Err(ApiError(AppError::BadRequest(e.message())));
        }
    };
    let meta = &prepared.trust.meta;
    handlers::store::audit(
        &state,
        &user,
        &ip,
        &meta.manifest.id,
        "ok",
        &format!(
            "reason=prepared version={} channel={} source={}",
            meta.manifest.version, prepared.channel, prepared.source
        ),
    );
    let mut resources = meta.manifest.resource_hints.clone();
    resources.extend(meta.manifest.interop_resource_hints());
    resources.sort();
    resources.dedup();
    let (interop_provides, interop_consumes) = handlers::plugins::interop_rows(&meta.manifest);
    let mut resp = Json(json!({
        "data": {
            "token": prepared.token,
            "id": meta.manifest.id,
            "name": meta.manifest.name,
            "version": meta.manifest.version,
            "api": meta.manifest.api,
            "key_id": meta.key_id,
            "signed": meta.signature_verified,
            "permissions": meta.manifest.permission_strings(),
            "resources": resources,
            "interop_provides": interop_provides,
            "interop_consumes": interop_consumes,
            "requires": meta.manifest.requires_str(),
            "egress": meta.manifest.network_egress,
            "channel": prepared.channel,
            "source": prepared.source,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct PluginConfirmBody {
    pub token: String,
    #[serde(default)]
    pub grant: Vec<String>,
    #[serde(default)]
    pub resource: Vec<String>,
    #[serde(default)]
    pub extra_resource: String,
}

/// `POST /ui/v1/plugins/confirm`：权限确认后安装商店 / 上传的暂存包。
///
/// 与旧确认页共用 `verify_for_confirm` + `commit_install`（同一信任判定、审计与清理）。
pub async fn plugin_confirm(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<PluginConfirmBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::require_permission(&user, skiff_model::Permission::PluginManage).map_err(ApiError)?;
    let token = body.token.trim();
    if token.len() != 24 || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(ApiError(AppError::BadRequest("确认令牌非法".into())));
    }
    let incoming = state.cfg.data_dir.join("plugins").join(".incoming");
    let path = incoming.join(format!("{token}.wbpkg"));
    if path.parent() != Some(incoming.as_path()) || !path.exists() {
        return Err(ApiError(AppError::BadRequest(
            "上传文件已过期，请重新上传".into(),
        )));
    }
    let trust = handlers::plugins::verify_for_confirm(&state, &path).map_err(ApiError)?;
    let mut objects = body.resource.clone();
    for part in body
        .extra_resource
        .split(['\n', '\r', ',', ' ', '\t'])
        .map(str::trim)
    {
        if !part.is_empty() {
            objects.push(part.to_string());
        }
    }
    objects.sort();
    objects.dedup();
    for o in &objects {
        skiff_pkgfmt::manifest::validate_resource(o)
            .map_err(|e| ApiError(AppError::BadRequest(format!("对象授权 {o} 非法：{e}"))))?;
    }
    let opts = skiff_plugin::install_options(
        &trust,
        body.grant.clone(),
        objects,
        state.cfg.allow_unsigned,
        state.cfg.form,
        state.cfg.allow_reduced_sandbox,
        &user.user.username,
    );
    let ip = handlers::client_ip(&state, &parts);
    let id = trust.meta.manifest.id.clone();
    handlers::plugins::commit_install(&state, &user, &ip, &path, &opts, &id)
        .await
        .map_err(ApiError)?;
    let mut resp = Json(json!({
        "data": {
            "id": id,
            "redirect_to": format!("/{}/plugins", state.cfg.entry_prefix),
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 设置页「后台端口 / 安全入口 / TLS」区块（字段冻结，只增不改）。
#[derive(Debug, Serialize)]
pub struct SettingsPanelItem {
    pub listen_addr: String,
    pub listen_port: u16,
    pub entry_prefix: String,
    /// `auto` / `on` / `off`，与旧页面三态回显同口径。
    pub tls_mode: String,
    pub tls_cert: String,
    pub tls_key: String,
    pub config_path: String,
    pub plaintext_behind_proxy: bool,
    /// 首启口令文件仍在：提示尽快改密（只提示，不拦截）。
    pub initial_admin_pending: bool,
}

/// 设置页账户安全区块（口令 / TOTP 表单仍在旧版设置页）。
#[derive(Debug, Serialize)]
pub struct SettingsAccountItem {
    pub totp_enabled: bool,
    pub must_change_password: bool,
}

/// 信任公钥行（仅管理员可见，非管理员返回空数组）。
#[derive(Debug, Serialize)]
pub struct SettingsKeyItem {
    pub key_id: String,
    pub fingerprint_short: String,
    pub label: String,
}

/// 设置页一次性提示（本次读取消费，刷新不重放）。
#[derive(Debug, Serialize)]
pub struct SettingsFlashItem {
    pub notice: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsData {
    pub panel: SettingsPanelItem,
    pub account: SettingsAccountItem,
    pub keys: Vec<SettingsKeyItem>,
    pub can_admin: bool,
    pub flash: SettingsFlashItem,
}

/// `GET /ui/v1/settings`：设置页只读快照。
///
/// 写操作按钮显隐只看 `can_admin`；口令与 TOTP 仍走旧版设置页，不在本接口返回秘密材料。
pub async fn settings(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    let can_admin = user.can(skiff_model::Permission::Admin);
    let user_row = state
        .db()
        .user_by_id(user.user.id)
        .map_err(|e| ApiError(AppError::from(e)))?
        .ok_or_else(|| ApiError(AppError::NotFound("用户不存在".into())))?;
    let keys = if can_admin {
        state
            .db()
            .list_trusted_keys()
            .map_err(|e| ApiError(AppError::from(e)))?
            .into_iter()
            .map(|(key_id, _pk, label)| SettingsKeyItem {
                fingerprint_short: key_id.chars().take(16).collect(),
                key_id,
                label,
            })
            .collect()
    } else {
        Vec::new()
    };
    let tls_mode = if state.cfg.tls_enabled {
        "on"
    } else if !state.cfg.dev_http {
        "auto"
    } else if !state.cfg.listen.ip().is_loopback() {
        "off"
    } else {
        "auto"
    };
    let (notice, error) = match state.take_flash(&user.session_id) {
        Some(msg) => match msg.kind {
            skiff_app::FlashKind::Notice => (msg.text, String::new()),
            skiff_app::FlashKind::Error => (String::new(), msg.text),
        },
        None => (String::new(), String::new()),
    };
    let data = SettingsData {
        panel: SettingsPanelItem {
            listen_addr: state.cfg.listen.ip().to_string(),
            listen_port: state.cfg.listen.port(),
            entry_prefix: state.cfg.entry_prefix.clone(),
            tls_mode: tls_mode.into(),
            tls_cert: state
                .cfg
                .tls_cert
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            tls_key: state
                .cfg
                .tls_key
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            config_path: state.cfg.config_path.display().to_string(),
            plaintext_behind_proxy: !state.cfg.dev_http
                && !state.cfg.tls_enabled
                && !state.cfg.trusted_proxies.is_empty(),
            initial_admin_pending: state.cfg.data_dir.join("initial-admin.txt").exists(),
        },
        account: SettingsAccountItem {
            totp_enabled: user_row.totp_enabled,
            must_change_password: user.must_change_password(),
        },
        keys,
        can_admin,
        flash: SettingsFlashItem { notice, error },
    };
    let mut resp = Json(json!({ "data": data })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct SettingsPanelBody {
    #[serde(default)]
    pub listen_addr: String,
    #[serde(default)]
    pub listen_port: String,
    #[serde(default)]
    pub entry_prefix: String,
    #[serde(default)]
    pub tls_mode: String,
    #[serde(default)]
    pub tls_cert: String,
    #[serde(default)]
    pub tls_key: String,
}

/// `POST /ui/v1/settings/panel`：保存监听地址 / 入口前缀 / TLS。
///
/// 复用旧表单的 `apply_panel_update`（同一校验、同一落盘、同一审计），只把传输层
/// 换成 JSON；CSRF 用 `/ui/v1` 统一的 `x-csrf-token` 头。
pub async fn settings_panel_update(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<SettingsPanelBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    handlers::require_permission(&user, skiff_model::Permission::Admin).map_err(ApiError)?;
    let mut form: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (key, value) in [
        ("listen_addr", body.listen_addr),
        ("listen_port", body.listen_port),
        ("entry_prefix", body.entry_prefix),
        ("tls_mode", body.tls_mode),
        ("tls_cert", body.tls_cert),
        ("tls_key", body.tls_key),
    ] {
        form.insert(key.to_string(), vec![value]);
    }
    let message = handlers::settings::apply_panel_update(&state, &user, &form)
        .await
        .map_err(|m| ApiError(AppError::BadRequest(m)))?;
    let mut resp = Json(json!({ "data": { "message": message } })).into_response();
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
