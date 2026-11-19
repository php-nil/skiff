//! 内核「Docker 设置」：镜像加速源与镜像前缀。
//!
//! 原挂在「网站设置」下，但两项都只影响镜像拉取，与站点业务无关，因此迁到 Docker 一级。

use std::collections::HashMap;

use crate::error::AppError;
use crate::state::{AppState, CurrentUser};
use crate::util::form_first;
/// `/ui/v1/settings/docker` 的唯一写入入口。
///
/// 镜像前缀落库、镜像源写入宿主 `daemon.json`、失败审计与成功审计都在这里；
/// 调用方只负责把结果包成 JSON 信封。
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
