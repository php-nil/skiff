//! `/ui/v1`：budding 前端运行时（SDUI）的 JSON 命名空间。
//!
//! 与 `handlers::api`（`/api/v1` 机器 API）并列，但服务对象不同：这里只给页面渲染运行时用，
//! 会话 Cookie + CSRF 鉴权，**未认证一律 401 JSON**（`UiAuthUser` 提取器），不做页面 303；
//! 失败载荷沿用 `AppError` 的同一套信封（`code/status/message/field/retryable`），
//! 不另造错误形状（URL 契约 §6、SDUI 集成 §3.2/§5.2）。
//!
//! 本批只落地会话引导与登出：壳页启动时据此判定登录态、取 CSRF、入口前缀与能力开关。

#![allow(unused_imports)]

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
use skiff_app::actions as act;
use skiff_app::commands::runtime as runtime_cmd;
use skiff_app::cursor::{AuditScope, CursorCodec, CursorError, FilterSet};

mod audit;
mod auth;
mod compose;
mod containers;
mod dto;
mod files;
mod jobs;
mod menu;
mod overview;
mod plugins;
mod resources;
mod runtimes;
mod settings;
mod sites;
mod store;
mod tasks;
mod web;

pub use dto::*;

/// 接口版本：与路径段 `v1` 一致，前端只做兼容判断（SDUI 集成 §3.7）。
pub const API_VERSION: &str = "v1";

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

/// 站点写操作的领域错误：字段级错误在 JSON 里按 400 返回，而不是 500。
fn site_write_err(e: crate::state::sites::SiteError) -> AppError {
    use crate::state::sites::SiteError;
    match e.unfield() {
        SiteError::BadRequest(m) => AppError::BadRequest(m),
        SiteError::Conflict(m) => AppError::Conflict(m),
        SiteError::Unavailable(m) => AppError::Unavailable(m),
        SiteError::Internal(m) => AppError::Internal(m),
        SiteError::Field(_, inner) => AppError::BadRequest(inner.to_string()),
    }
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

fn write_done(value: serde_json::Value) -> Result<Response, ApiError> {
    let mut resp = Json(json!({ "data": value })).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

fn planned_compose(
    state: &AppState,
    user: &crate::state::CurrentUser,
    body: &ComposeSourceBody,
) -> Result<skiff_docker::compose::ComposeProject, ApiError> {
    if body.compose.trim().is_empty() {
        return Err(ApiError(AppError::BadRequest(
            "compose 内容不能为空".into(),
        )));
    }
    let project =
        handlers::compose::planned_project(&body.compose, &body.project).map_err(ApiError)?;
    handlers::compose::validate_specs(state, user, &project).map_err(ApiError)?;
    Ok(project)
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

fn task_write_err(e: skiff_app::commands::task::TaskCommandError) -> AppError {
    use skiff_app::commands::task::TaskCommandError;
    match e {
        TaskCommandError::Unavailable(m) => AppError::Unavailable(m),
        TaskCommandError::Failed(m) => AppError::BadRequest(m),
    }
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

/// 解码审计游标：排序键固定为 `(ts, id)`，结构不符按格式非法处理。
fn parse_audit_cursor(
    codec: &CursorCodec<AuditScope>,
    cursor: &str,
    filter: &FilterSet,
) -> Result<(i64, i64), ApiError> {
    let keys = codec
        .decode(cursor, filter)
        .map_err(|e: CursorError| ApiError(AppError::BadRequest(e.message().into())))?;
    let parsed = match keys.as_slice() {
        [ts, id] => ts.parse::<i64>().ok().zip(id.parse::<i64>().ok()),
        _ => None,
    };
    parsed.ok_or_else(|| {
        ApiError(AppError::BadRequest(
            CursorError::Malformed.message().into(),
        ))
    })
}

/// 插件状态标签：库内状态是 `enabled` / `installed` / `disabled`（`installed` 就是已安装），
/// 展示口径与 SSR 插件页一致（「已启用」「已安装 · 未启用」「运行中」），四态固定：
/// 启用且运行中、启用但未运行、未启用、异常（未知状态码）。
fn plugin_status_label(status: &str, enabled: bool, running: bool) -> &'static str {
    match status {
        // `running` 是 supervisor 进程起来后写回库的常态，不是异常。
        "enabled" | "installed" | "disabled" | "running" => match (enabled, running) {
            (true, true) => "已启用 · 运行中",
            (true, false) => "已启用 · 未运行",
            (false, _) => "已安装 · 未启用",
        },
        _ => "异常",
    }
}

fn plugin_item(view: crate::templates::PluginView) -> PluginItem {
    PluginItem {
        status: StatusDto {
            code: view.status.clone(),
            label: plugin_status_label(&view.status, view.enabled, view.running).into(),
        },
        id: view.id,
        name: view.name,
        version: view.version,
        api: view.api,
        enabled: view.enabled,
        key_id: view.key_id_full,
        signed: view.signed,
        reduced: view.reduced,
        running: view.running,
        requires: view.requires,
        permissions: view.permissions,
        grants: view.grants,
        ui_path: view.ui_path,
        takedown_note: view.takedown_note,
    }
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

/// `skiff-fs` 稳定错误码 → `/ui/v1` 信封：状态由 `FsError::http_status` 决定。
fn fs_api_error(e: skiff_app::admin_fs::FsErr) -> ApiError {
    let message = e.message().to_string();
    ApiError(match e.http_status() {
        400 | 413 => AppError::BadRequest(message),
        403 => AppError::Forbidden(message),
        404 => AppError::NotFound(message),
        409 => AppError::Conflict(message),
        507 => AppError::NoSpace(message),
        503 => AppError::Unavailable(message),
        504 => AppError::Timeout(message),
        _ => AppError::Internal(message),
    })
}

/// `skiff_app::sites::SiteError` → `/ui/v1` 信封。
fn site_api_error(e: skiff_app::sites::SiteError) -> ApiError {
    match e {
        skiff_app::sites::SiteError::Field(_, inner) => site_api_error(*inner),
        skiff_app::sites::SiteError::BadRequest(m) => ApiError(AppError::BadRequest(m)),
        skiff_app::sites::SiteError::Conflict(m) => ApiError(AppError::Conflict(m)),
        skiff_app::sites::SiteError::Unavailable(m) => ApiError(AppError::Unavailable(m)),
        skiff_app::sites::SiteError::Internal(m) => ApiError(AppError::Internal(m)),
    }
}

/// 插件菜单键 → (插件 id, 默认视图, 视图列表)。
///
/// 只读 manifest，不另造菜单语义：视图顺序与 `[[menu.items]]` 的排序一致。
fn plugin_menu_views(state: &AppState, key: &str) -> (String, String, Vec<MenuViewData>) {
    let Some(rest) = key.strip_prefix("plugin/") else {
        return (String::new(), String::new(), Vec::new());
    };
    let Some((id, l2)) = rest.split_once('/') else {
        return (String::new(), String::new(), Vec::new());
    };
    let Some(manifest) = state
        .db()
        .plugin(id)
        .ok()
        .flatten()
        .and_then(|r| skiff_pkgfmt::Manifest::parse(&r.manifest).ok())
    else {
        return (id.to_string(), String::new(), Vec::new());
    };
    let Some(decl) = manifest.menu.iter().find(|m| m.id == l2) else {
        return (id.to_string(), String::new(), Vec::new());
    };
    let mut views: Vec<MenuViewData> = if decl.items.is_empty() {
        vec![MenuViewData {
            view: decl.view.clone(),
            label: decl.label.clone(),
        }]
    } else {
        let mut items: Vec<&skiff_pkgfmt::MenuItem> = decl.items.iter().collect();
        items.sort_by(|a, b| (a.order, &a.label, &a.id).cmp(&(b.order, &b.label, &b.id)));
        items
            .into_iter()
            .map(|it| MenuViewData {
                view: it.view.clone().unwrap_or_else(|| it.id.clone()),
                label: it.label.clone(),
            })
            .collect()
    };
    views.dedup_by(|a, b| a.view == b.view);
    let default_view = views
        .first()
        .map(|v| v.view.clone())
        .unwrap_or_else(|| decl.view.clone());
    (id.to_string(), default_view, views)
}

/// /ui/v1 路由：各资源模块各自注册后合并；路径、方法与响应保持拆分前一致。
pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .merge(auth::router())
        .merge(menu::router())
        .merge(overview::router())
        .merge(sites::router())
        .merge(runtimes::router())
        .merge(containers::router())
        .merge(resources::router())
        .merge(compose::router())
        .merge(tasks::router())
        .merge(jobs::router())
        .merge(audit::router())
        .merge(plugins::router())
        .merge(store::router())
        .merge(settings::router())
        .merge(web::router())
        .merge(files::router())
}

#[cfg(test)]
mod tests {
    use super::plugin_status_label;

    #[test]
    fn plugin_status_label_maps_installed_states_without_abnormal() {
        // `installed` 是常态，不是异常；按启用 + 运行两维给出四态中的三态。
        assert_eq!(
            plugin_status_label("installed", true, true),
            "已启用 · 运行中"
        );
        assert_eq!(
            plugin_status_label("installed", true, false),
            "已启用 · 未运行"
        );
        assert_eq!(
            plugin_status_label("installed", false, false),
            "已安装 · 未启用"
        );
        assert_eq!(
            plugin_status_label("disabled", false, false),
            "已安装 · 未启用"
        );
        assert_eq!(
            plugin_status_label("enabled", true, false),
            "已启用 · 未运行"
        );
        // supervisor 正常拉起进程后库内写的是 `running`，必须落正常分支。
        assert_eq!(
            plugin_status_label("running", true, true),
            "已启用 · 运行中"
        );
    }

    #[test]
    fn plugin_status_label_keeps_unknown_status_abnormal() {
        assert_eq!(plugin_status_label("revoked", true, true), "异常");
        assert_eq!(plugin_status_label("", false, false), "异常");
    }
}
