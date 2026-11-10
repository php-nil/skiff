//! 内核「Docker 设置」：镜像加速源与镜像前缀。
//!
//! 原挂在「网站设置」下，但两项都只影响镜像拉取，与站点业务无关，因此迁到 Docker 一级。

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::LOCATION;
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::DockerSettingsPage;
use crate::util::{form_first, parse_form};

/// 旧路径 `/{p}/docker/settings` → `/{p}/settings/docker`（308 保留方法与请求体）。
pub async fn legacy_path(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let prefix = &state.cfg.entry_prefix;
    let mut target = format!("/{prefix}/settings/docker");
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(q);
    }
    (StatusCode::PERMANENT_REDIRECT, [(LOCATION, target)]).into_response()
}

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    handlers::render(DockerSettingsPage {
        ctx: handlers::page_ctx(&state, &user, "core/docker/settings"),
        registry_mirror: state
            .db()
            .get_setting("sites.registry_mirror")?
            .unwrap_or_default(),
        image_prefix: state
            .db()
            .get_setting("sites.image_prefix")?
            .unwrap_or_default(),
        can_admin: user.can(skiff_model::Permission::Admin),
        error: String::new(),
        notice: String::new(),
    })
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

    if form.contains_key("image_prefix") {
        let raw = form_first(&form, "image_prefix");
        state.save_image_prefix(&raw).map_err(AppError::Internal)?;
    }

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
                    state
                        .save_registry_mirror(raw)
                        .map_err(AppError::Internal)?;
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
                    state.audit_or_log(
                        &user.user.username,
                        &ip,
                        "docker.settings.save",
                        "registry_mirror",
                        "failed",
                        &e,
                    );
                    return Ok(handlers::flash_redirect(
                        &state,
                        &user,
                        skiff_app::FlashKind::Error,
                        e,
                        format!("/{prefix}/settings/docker"),
                    ));
                }
            }
        }
    }

    let ip = handlers::client_ip(&state, &parts);
    state.audit_or_log(
        &user.user.username,
        &ip,
        "docker.settings.save",
        "-",
        "ok",
        "",
    );
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        notice,
        format!("/{prefix}/settings/docker"),
    ))
}
