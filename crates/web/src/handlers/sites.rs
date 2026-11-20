//! 内核「网站」的跨模块共享辅助：状态文案、view model 组装、领域错误映射。
//!
//! 站点页面与服务端表单已全部迁到 budding 壳（`/ui/v1/sites/*`、`/ui/v1/runtimes/*`），
//! 这里只留 JSON 通道仍在复用的纯函数：
//!
//! - `crud`：配置保存文案；
//! - `runtime`：运行环境扩展清单；
//! - `tls`：站点备份视图。

use skiff_app::sites::{site_runtime_status, SiteRuntimeStatus};
use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::error::AppError;
use crate::templates::RuntimeRow;
use crate::util::encode_uri_component;
use skiff_app::sites::SiteError;
use skiff_app::AppState;

mod crud;
mod runtime;
mod tls;

pub(crate) use crud::save_config_notice;
pub(crate) use runtime::extension_options;
pub(crate) use tls::backup_view;

pub(crate) fn type_label(t: &str) -> &'static str {
    match t {
        TYPE_STATIC => "静态网站",
        TYPE_PROXY => "反向代理",
        _ => "运行环境",
    }
}
pub(crate) fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "运行中",
        // Docker 的终止态统一说「已停止」，别再落到兜底的「未知」。
        "stopped" | "exited" | "dead" | "created" => "已停止",
        "paused" => "已暂停",
        "restarting" => "重启中",
        "installing" => "安装中",
        "error" => "异常",
        "missing" => "未安装",
        "unimplemented" => "P2 未实现",
        _ => "状态未知",
    }
}

fn state_class(state: &str) -> &'static str {
    match state {
        "running" => "ok",
        "error" => "danger",
        "missing" => "warn",
        // 未知不是「运行中」，用 muted 表示无法确认。
        "unknown" | "" => "muted",
        "stopped" | "exited" | "dead" | "created" | "paused" | "restarting" | "installing" => {
            "warn"
        }
        _ => "",
    }
}

/// 运行环境站点的对外状态：领域分类在 `skiff_app::sites`，文案与配色只在这里映射
/// （SSR 与 `/ui/v1` 共用），不再下沉到 app 读模型。
fn runtime_site_status(
    site_status: &str,
    runtime_state: Option<&str>,
) -> (&'static str, &'static str) {
    use SiteRuntimeStatus as Status;
    match site_runtime_status(site_status, runtime_state) {
        Status::SiteStopped => ("已停止", ""),
        Status::Running => ("运行中", "ok"),
        Status::Installing => ("环境安装中", "warn"),
        Status::Error => ("环境异常", "danger"),
        Status::Missing => ("未运行（环境未安装）", "warn"),
        Status::DockerUnavailable => ("状态未知（Docker 不可用）", "muted"),
        Status::Unimplemented => ("运行环境尚未接入", "warn"),
        Status::EnvironmentStopped => ("未运行（环境已停止）", "warn"),
        Status::RecordMissing => ("未运行（环境记录缺失）", "warn"),
    }
}

/// 站点状态列：运行环境站点看有效运行态，静态/反代站点看自身启停状态。
pub(crate) fn site_status(
    summary: &skiff_app::views::sites::SiteSummary,
) -> (&'static str, &'static str) {
    // 已启用且巡检已把 vhost 切到维护页：状态优先于容器运行态，避免「运行中」与实际 503 矛盾。
    if summary.status == "running" && !summary.maintenance_reason.is_empty() {
        return ("维护中", "warn");
    }
    if summary.site_type == TYPE_RUNTIME {
        runtime_site_status(&summary.status, summary.runtime_state.as_deref())
    } else {
        match summary.status.as_str() {
            "running" => ("运行中", "ok"),
            "stopped" => ("已停止", ""),
            _ => ("异常", "danger"),
        }
    }
}

/// HTTPS 列：是否启用 + 证书是否就绪。
pub(crate) fn https_status(enabled: bool, cert_ready: bool) -> (&'static str, &'static str) {
    match (enabled, cert_ready) {
        (true, true) => ("已启用 HTTPS", "ok"),
        (true, false) => ("证书缺失", "danger"),
        (false, true) => ("证书已上传", "info"),
        (false, false) => ("未配置 HTTPS", ""),
    }
}

/// 运行环境状态文案（SSR 与 JSON 共用）：安装中优先显示任务阶段，
/// 避免「未安装」与正在拉镜像互相矛盾。
pub(crate) fn runtime_state_text(v: &skiff_app::sites::RuntimeView) -> String {
    let job = v.job.as_ref();
    let job_failed = job.is_some_and(|j| j.state == skiff_app::sites::RuntimeJobState::Failed);
    if let Some(j) = job.filter(|j| j.active()) {
        format!("{}：{}", crate::labels::runtime_job_state(j.state), j.step)
    } else if v.drift {
        "状态漂移".to_string()
    } else if job_failed && v.container_state != "running" {
        "安装失败".to_string()
    } else {
        state_label(&v.effective_state).to_string()
    }
}

/// 运行环境状态色彩（模板用）。
pub(crate) fn runtime_state_class(v: &skiff_app::sites::RuntimeView) -> String {
    let job_failed = v
        .job
        .as_ref()
        .is_some_and(|j| j.state == skiff_app::sites::RuntimeJobState::Failed);
    if v.job.as_ref().is_some_and(|j| j.active()) || v.drift || job_failed {
        "warn".to_string()
    } else {
        state_class(&v.effective_state).to_string()
    }
}

/// 列表行：从 app 读模型映射到 SSR 模板（文案色彩与 URL 只在这一侧补齐）。
pub(crate) fn runtime_row(state: &AppState, v: &skiff_app::sites::RuntimeView) -> RuntimeRow {
    let prefix = &state.cfg.entry_prefix;
    let r = skiff_app::views::runtimes::row(v);
    let job_active = r.job_active();
    RuntimeRow {
        id: r.id.clone(),
        kind_key: r.kind,
        kind_label: r.kind_label,
        name: r.name,
        container_name: r.container_name,
        version: r.version,
        version_display: r.version_display,
        version_note: r.version_note,
        actual_version: r.actual_version,
        container_exists: r.container_exists,
        image: r.image,
        image_id: r.image_id,
        image_id_full: v.image_id.clone(),
        image_digest: r.image_digest,
        image_digest_full: v.image_digest.clone(),
        image_mismatch: r.image_mismatch,
        state_label: runtime_state_text(v),
        state_class: runtime_state_class(v),
        drift: r.drift,
        site_count: r.site_count,
        container_state: r.container_state,
        endpoint_short: r.endpoint_short,
        endpoint_full: r.endpoint_full,
        container_address: r.container_address,
        publish_port: r.publish_port.map(|p| p.to_string()).unwrap_or_default(),
        port: r.port.map(|p| p.to_string()).unwrap_or_default(),
        implemented: r.implemented,
        is_default: r.is_default,
        detail_href: format!(
            "/{prefix}/sites/runtimes/{}",
            encode_uri_component(&v.runtime.id)
        ),
        docker_href: format!(
            "/{prefix}/docker/containers/{}",
            encode_uri_component(&v.container_name)
        ),
        job_progress: r.job.as_ref().map(|j| j.progress).unwrap_or(0),
        job_error: r.job.as_ref().map(|j| j.error.clone()).unwrap_or_default(),
        job_active,
    }
}
pub(crate) fn map_err(e: SiteError) -> AppError {
    match e.unfield() {
        SiteError::BadRequest(m) => AppError::BadRequest(m),
        SiteError::Conflict(m) => AppError::Conflict(m),
        SiteError::Unavailable(m) => AppError::Unavailable(m),
        SiteError::Internal(m) => AppError::Internal(m),
        SiteError::Field(_, inner) => AppError::Internal(inner.to_string()),
    }
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住 `sites::crud`/`sites::runtime` 经 `use super::*` 得到的 `actor(...)`。
pub(crate) use crate::handlers::audit_actor as actor;

#[cfg(test)]
mod tests {
    use super::crud::save_config_notice;
    use super::{map_err, runtime_site_status, state_class, state_label};
    use skiff_app::sites::SiteError;
    use skiff_model::ErrorCode;

    #[test]
    fn domain_errors_map_to_transport_status_and_code() {
        // 命令层只表达领域结论，HTTP 状态与稳定错误码在这一层一一对应；
        // 这里锁住映射，防止某天把冲突改成 400 或把依赖故障吞成 500。
        let cases = [
            (
                SiteError::BadRequest("x".into()),
                400,
                ErrorCode::InvalidShape,
            ),
            (SiteError::Conflict("x".into()), 409, ErrorCode::Conflict),
            (
                SiteError::Unavailable("x".into()),
                503,
                ErrorCode::Unavailable,
            ),
            (SiteError::Internal("x".into()), 500, ErrorCode::Internal),
        ];
        for (err, status, code) in cases {
            let mapped = map_err(err);
            assert_eq!(mapped.status().as_u16(), status);
            assert_eq!(mapped.error_code(), code);
            assert_eq!(mapped.message(), "x");
        }
    }

    #[test]
    fn runtime_status_labels_are_stable() {
        // 领域分类在 app（见 `site_runtime_status` 单测）；这里锁定对外文案与配色。
        let cases = [
            ("stopped", Some("running"), "已停止", ""),
            ("running", Some("running"), "运行中", "ok"),
            ("running", Some("installing"), "环境安装中", "warn"),
            ("running", Some("error"), "环境异常", "danger"),
            ("running", Some("missing"), "未运行（环境未安装）", "warn"),
            (
                "running",
                Some("unknown"),
                "状态未知（Docker 不可用）",
                "muted",
            ),
            ("running", Some("unimplemented"), "运行环境尚未接入", "warn"),
            ("running", Some("stopped"), "未运行（环境已停止）", "warn"),
            ("running", None, "未运行（环境记录缺失）", "warn"),
        ];
        for (site, runtime, label, class) in cases {
            assert_eq!(runtime_site_status(site, runtime), (label, class));
        }
    }

    #[test]
    fn env_row_labels_keep_unknown_missing_and_stopped_apart() {
        // 运行环境列表的徽章文案与配色：Docker 原始状态（exited/dead 等）已由
        // `runtime_effective_state` 归一成 stopped，这里只校验展示层不把未知当运行中。
        assert_eq!(state_label("running"), "运行中");
        assert_eq!(state_class("running"), "ok");
        for stopped in ["stopped", "exited", "dead", "created"] {
            assert_eq!(state_label(stopped), "已停止", "{stopped}");
            assert_eq!(state_class(stopped), "warn", "{stopped}");
        }
        assert_eq!(state_label("missing"), "未安装");
        assert_eq!(state_class("missing"), "warn");
        assert_eq!(state_label("unknown"), "状态未知");
        assert_eq!(state_class("unknown"), "muted");
        assert_eq!(state_label(""), "状态未知");
        assert_eq!(state_class(""), "muted");
    }

    #[test]
    fn save_config_notice_never_claims_reload_when_site_stopped() {
        assert!(save_config_notice(true).contains("已重载"));
        assert!(save_config_notice(true).contains("校验通过"));
        assert!(!save_config_notice(false).contains("已重载"));
        assert!(save_config_notice(false).contains("站点未运行，未重载"));
    }
}
