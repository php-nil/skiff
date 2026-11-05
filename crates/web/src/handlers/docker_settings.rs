//! 内核「Docker 设置」：镜像加速源与镜像前缀。
//!
//! 原挂在「网站设置」下，但两项都只影响镜像拉取，与站点业务无关，因此迁到 Docker 一级。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::DockerSettingsPage;
use crate::util::{encode_uri_component, form_first, parse_form};

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
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
        error: q.get("error").cloned().unwrap_or_default(),
        notice: q.get("notice").cloned().unwrap_or_default(),
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
        let value = raw.trim().trim_end_matches('/').to_string();
        state.db().set_setting("sites.image_prefix", &value)?;
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
                    state.db().set_setting("sites.registry_mirror", raw)?;
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
                    let _ = state.db().audit(
                        &user.user.username,
                        &ip,
                        "docker.settings.save",
                        "registry_mirror",
                        "failed",
                        &e,
                    );
                    return Ok(handlers::redirect(format!(
                        "/{prefix}/docker/settings?error={}",
                        encode_uri_component(&e)
                    )));
                }
            }
        }
    }

    let ip = handlers::client_ip(&state, &parts);
    let _ = state.db().audit(
        &user.user.username,
        &ip,
        "docker.settings.save",
        "-",
        "ok",
        "",
    );
    Ok(handlers::redirect(format!(
        "/{prefix}/docker/settings?notice={}",
        encode_uri_component(&notice)
    )))
}
