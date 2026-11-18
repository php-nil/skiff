//! 内核「网站」页面：站点列表、运行环境、环境设置、日志与操作。
//!
//! 按主题拆成子模块，本文件只保留跨模块共享的展示辅助（状态文案、view model
//! 组装、领域错误映射），并以 `pub use` 维持原有 `handlers::sites::*` 路径不变：
//!
//! - `crud`：站点列表/新建/启停/域名/配置/批量/详情/删除；
//! - `runtime`：运行环境页与全部 `runtime_*` 动作；
//! - `tls`：证书与备份；
//! - `logs`：站点日志。

use skiff_app::sites::{site_runtime_status, SiteRuntimeStatus};
use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::error::AppError;
use crate::state::sites::{SiteEnvStatus, SiteError, SiteField};
use crate::state::AppState;
use crate::templates::{RuntimeRow, SiteEnvView, SiteTlsView, SiteView};
use crate::util::encode_uri_component;

mod crud;
mod logs;
mod runtime;

pub(crate) use runtime::extension_options;
mod tls;

pub(crate) use crud::save_config_notice;
pub use crud::{
    action, batch, create, delete, delete_confirm, domains, new_shell, save_config, set_default,
};
pub use logs::logs;
pub use runtime::{
    env_add, env_images_save, runtime_action, runtime_clone, runtime_config_file_save,
    runtime_config_save, runtime_extension_install, runtime_extension_uninstall,
    runtime_extensions_batch, runtime_extensions_save, runtime_health, runtime_job, runtime_jobs,
    runtime_logs, runtime_node_config_save, runtime_pecl_install, runtime_port_save, runtime_probe,
    runtime_publish_save, runtime_unbind_site,
};
pub use tls::{set_backup, ssl_action};

fn env_view(env: &SiteEnvStatus) -> SiteEnvView {
    SiteEnvView {
        docker_available: env.docker_available,
        docker_reason: env.docker_reason.clone(),
        nginx_state: env.nginx_state.clone(),
        installed: env.installed,
        ready: env.ready(),
        port: env.port,
        nginx_image: env.nginx_image.clone(),
        php_image: env.php_image.clone(),
    }
}

pub(crate) fn type_label(t: &str) -> &'static str {
    match t {
        TYPE_STATIC => "静态网站",
        TYPE_PROXY => "反向代理",
        _ => "运行环境",
    }
}

/// 运行环境动作的中文名：共享环境二次确认页据此说明「将要执行什么」。
fn action_label(action: &str) -> &'static str {
    match action {
        "stop" => "停止",
        "restart" => "重启",
        "rebuild" => "重建",
        "install" => "安装",
        "port" => "修改端口",
        _ => "操作",
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
        detail_href: format!("/{prefix}/runtimes/{}", encode_uri_component(&v.runtime.id)),
        docker_href: format!(
            "/{prefix}/containers/{}",
            encode_uri_component(&v.container_name)
        ),
        job_progress: r.job.as_ref().map(|j| j.progress).unwrap_or(0),
        job_error: r.job.as_ref().map(|j| j.error.clone()).unwrap_or_default(),
        job_active,
    }
}

fn job_json(job: &skiff_app::sites::RuntimeJob) -> serde_json::Value {
    serde_json::json!({
        "id": job.runtime_id,
        "state": job.state.as_str(),
        "state_label": crate::labels::runtime_job_state(job.state),
        "step": job.step,
        "progress": job.progress,
        "error": job.error,
        "log": job.log_text(),
        "active": job.active(),
        "failed": job.state == skiff_app::sites::RuntimeJobState::Failed,
        "kind": job.kind,
        "ext_keys": job.ext_keys,
        "started_at": job.started_at,
        "updated_at": job.updated_at,
    })
}

pub(crate) fn site_view(s: &skiff_app::views::sites::SiteSummary) -> SiteView {
    let (status_label, status_class) = site_status(s);
    let (https_label, https_class) = https_status(s.https, s.cert_ready);
    SiteView {
        domain: s.domain.clone(),
        index_dir: s.index_dir.clone(),
        root_dir: s.root_dir.clone(),
        site_type: s.site_type.clone(),
        type_label: type_label(&s.site_type).to_string(),
        runtime_id: s.runtime_id.clone(),
        runtime_label: s.runtime_label.clone(),
        runtime_name: s.runtime_name.clone(),
        runtime_shared_hint: s.runtime_shared_hint.clone(),
        proxy_target: s.proxy_target.clone(),
        proxy_websocket: s.proxy_websocket,
        status: s.status.clone(),
        status_label: status_label.to_string(),
        status_class: status_class.to_string(),
        maintenance_reason: s.maintenance_reason.clone(),
        https_label: https_label.to_string(),
        https_class: https_class.to_string(),
        created: s.created.clone(),
        is_default: s.is_default,
        aliases_text: s.aliases_text.clone(),
        alias_preview: s.alias_preview.clone(),
        alias_more: s.alias_more,
        aliases: s.aliases.clone(),
        https: s.https,
        force_https: s.force_https,
    }
}

fn tls_view(info: skiff_app::sites::SiteTlsInfo) -> SiteTlsView {
    SiteTlsView {
        enabled: info.enabled,
        force_https: info.force_https,
        cert_path: info.cert_path,
        key_path: info.key_path,
        cert_exists: info.cert_exists,
        subject: info.subject.unwrap_or_default(),
        issuer: info.issuer.unwrap_or_default(),
        expires: info.expires.unwrap_or_default(),
    }
}

pub(crate) use tls::backup_view;

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

/// 可回填到弹窗的用户侧错误文案；内部错误返回 `None`，交给错误页。
fn site_error_text(e: &SiteError) -> Option<String> {
    match e {
        SiteError::BadRequest(m) | SiteError::Conflict(m) | SiteError::Unavailable(m) => {
            Some(m.clone())
        }
        SiteError::Internal(_) => None,
        SiteError::Field(_, inner) => site_error_text(inner),
    }
}

#[cfg(test)]
mod tests {
    use super::crud::{error_field, save_config_notice};
    use super::{map_err, runtime_site_status, site_error_text, state_class, state_label};
    use crate::state::sites::SiteError;
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
    fn only_internal_domain_errors_break_out_of_the_page() {
        // 业务错误由页面就地提示（返回 Some），内部错误才升级成错误页（None）。
        for err in [
            SiteError::BadRequest("x".into()),
            SiteError::Conflict("x".into()),
            SiteError::Unavailable("x".into()),
        ] {
            assert_eq!(site_error_text(&err).as_deref(), Some("x"));
        }
        assert_eq!(site_error_text(&SiteError::Internal("x".into())), None);
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

    #[test]
    fn server_error_maps_to_visible_field_only() {
        use crate::state::sites::SiteField as F;
        // 每个码映射到唯一字段。
        assert_eq!(error_field(Some(F::Domain), "static"), "domain");
        assert_eq!(error_field(Some(F::Aliases), "runtime"), "aliases");
        assert_eq!(error_field(Some(F::RootDir), "runtime"), "root_dir");
        // 类型专属错误只在对应类型下落到字段，否则回顶部提示。
        assert_eq!(error_field(Some(F::ProxyTarget), "proxy"), "proxy_target");
        assert_eq!(error_field(Some(F::ProxyTarget), "static"), "");
        assert_eq!(error_field(Some(F::RuntimeId), "runtime"), "runtime_id");
        assert_eq!(error_field(Some(F::RuntimeId), "static"), "");
        // 没有字段码（含环境级错误）一律回顶部，不再按文案猜。
        assert_eq!(error_field(None, "runtime"), "");
        assert_eq!(error_field(None, "static"), "");
    }
}
