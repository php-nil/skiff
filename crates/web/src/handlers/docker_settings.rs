//! 内核「Docker 设置」：镜像加速源与镜像前缀。
//!
//! 原挂在「网站设置」下，但两项都只影响镜像拉取，与站点业务无关，因此迁到 Docker 一级。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::header::LOCATION;
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::{AppState, CurrentUser};
use crate::util::{form_first, parse_form};

/// 旧路径 `/{p}/docker/settings` → `/{p}/settings/docker`（308 保留方法与请求体）。
///
/// 兼容保留，不设删除时间；规范路径为 `/{p}/settings/docker`。
pub async fn legacy_path(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let prefix = &state.cfg.entry_prefix;
    let mut target = format!("/{prefix}/settings/docker");
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(q);
    }
    (StatusCode::PERMANENT_REDIRECT, [(LOCATION, target)]).into_response()
}

/// `GET /{p}/settings/docker` 规范入口：交付 budding 壳页（唯一入口）。
pub async fn shell(
    State(state): State<Arc<AppState>>,
    AuthUser(_user): AuthUser,
) -> Result<Response, AppError> {
    Ok(crate::handlers::spa::shell_page(&state))
}

/// Docker 设置写操作：SSR 表单与 `/ui/v1/settings/docker` 共用的唯一业务入口。
///
/// 镜像前缀落库、镜像源写入宿主 `daemon.json`、失败审计与成功审计都在这里，
/// 调用方只决定「表单跳转」还是「JSON 信封」。
pub(crate) async fn apply_settings(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    form: &HashMap<String, Vec<String>>,
) -> Result<String, AppError> {
    if form.contains_key("image_prefix") {
        let raw = form_first(form, "image_prefix");
        state.save_image_prefix(&raw).map_err(AppError::Internal)?;
    }

    let mut notice = "已保存".to_string();
    if form.contains_key("registry_mirror") {
        let raw = form_first(form, "registry_mirror");
        let raw = raw.trim();
        let old = state
            .db()
            .get_setting("sites.registry_mirror")?
            .unwrap_or_default();
        if raw != old {
            match state.apply_registry_mirror(raw).await {
                Ok(r) => {
                    state
                        .save_registry_mirror(raw)
                        .map_err(AppError::Internal)?;
                    notice = match r.mode {
                        skiff_app::docker_daemon::MirrorApplyMode::Reloaded => {
                            "镜像源已生效（Docker 热重载，运行中的容器未受影响）".to_string()
                        }
                        skiff_app::docker_daemon::MirrorApplyMode::NeedsRestart => {
                            match r.running_containers {
                                Some(n) if n > 0 => format!(
                                    "镜像源已写入，但本机 Docker 单元不支持热重载；{} 个运行中的容器未动，重启 Docker 后生效",
                                    n
                                ),
                                _ => "镜像源已写入，重启 Docker 后生效".to_string(),
                            }
                        }
                        skiff_app::docker_daemon::MirrorApplyMode::Unchanged => {
                            "镜像源未变化，无需写入".to_string()
                        }
                    };
                }
                Err(e) => {
                    state.audit_or_log(
                        &user.user.username,
                        ip,
                        "docker.settings.save",
                        "registry_mirror",
                        "failed",
                        &e,
                    );
                    return Err(AppError::BadRequest(e));
                }
            }
        }
    }

    state.audit_or_log(
        &user.user.username,
        ip,
        "docker.settings.save",
        "-",
        "ok",
        "",
    );
    Ok(notice)
}

pub async fn save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    handlers::require_permission(&user, skiff_model::Permission::Admin)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    let ip = handlers::client_ip(&state, &parts);

    match apply_settings(&state, &user, &ip, &form).await {
        Ok(notice) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            notice,
            format!("/{prefix}/settings/docker"),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.message(),
            format!("/{prefix}/settings/docker"),
        )),
    }
}
