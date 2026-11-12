//! 内核「网站」页面：站点列表、运行环境、环境设置、日志与操作。
//!
//! 按主题拆成子模块，本文件只保留跨模块共享的展示辅助（状态文案、view model
//! 组装、领域错误映射），并以 `pub use` 维持原有 `handlers::sites::*` 路径不变：
//!
//! - `crud`：站点列表/新建/启停/域名/配置/批量/详情/删除；
//! - `runtime`：运行环境页与全部 `runtime_*` 动作；
//! - `tls`：证书与备份；
//! - `logs`：站点日志。

use std::path::Path as FsPath;

use axum::http::request::Parts;
use skiff_app::commands::Actor;
use skiff_store::{Runtime, Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::error::AppError;
use crate::handlers;
use crate::state::sites::{SiteEnvStatus, SiteError, SiteField};
use crate::state::AppState;
use crate::templates::{RuntimeJobView, RuntimeRow, SiteEnvView, SiteTlsView, SiteView};
use crate::util::encode_uri_component;

mod crud;
mod logs;
mod runtime;
mod tls;

pub use crud::{
    action, batch, create, delete, delete_confirm, detail, domains, list, new_page_get,
    save_config, set_default,
};
pub use logs::logs;
pub use runtime::{
    env_add, env_images_save, env_page, legacy_env_detail, legacy_env_page, runtime_action,
    runtime_clone, runtime_config_file_save, runtime_config_save, runtime_detail,
    runtime_extension_install, runtime_extension_uninstall, runtime_extensions_batch,
    runtime_extensions_save, runtime_health, runtime_job, runtime_jobs, runtime_logs,
    runtime_node_config_save, runtime_pecl_install, runtime_port_save, runtime_probe,
    runtime_unbind_site,
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

fn type_label(t: &str) -> &'static str {
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

fn state_label(state: &str) -> &'static str {
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

/// 运行环境站点的对外状态：领域分类在 `skiff_app::sites`，这里只映射文案与配色。
fn runtime_site_status(
    site_status: &str,
    runtime_state: Option<&str>,
) -> (&'static str, &'static str) {
    use skiff_app::sites::SiteRuntimeStatus as Status;
    match skiff_app::sites::site_runtime_status(site_status, runtime_state) {
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

/// 列表版本列：版本来源由内核分类，文案与镜像副行在这里格式化。
pub(crate) fn version_display(
    actual: Option<&str>,
    target: &str,
    image: &str,
    container_exists: bool,
) -> (String, String) {
    match skiff_app::sites::runtime_version_state(actual, container_exists) {
        skiff_app::sites::RuntimeVersionState::Actual(v) => {
            (v.to_string(), format!("目标 {target} · {image}"))
        }
        skiff_app::sites::RuntimeVersionState::NotInstalled => {
            (target.to_string(), "未安装".into())
        }
        skiff_app::sites::RuntimeVersionState::Unreadable => {
            (target.to_string(), "版本未读取".into())
        }
    }
}

fn runtime_row(state: &AppState, v: &skiff_app::sites::RuntimeView) -> RuntimeRow {
    let prefix = &state.cfg.entry_prefix;
    let kind_label = skiff_app::runtime_catalog::kind_label(&v.runtime.kind);
    let endpoint_full = match v.runtime.kind.as_str() {
        skiff_store::KIND_PHP => {
            if v.socket_host.is_empty() {
                "socket 未登记".to_string()
            } else {
                format!("socket {}", v.socket_host)
            }
        }
        skiff_store::KIND_RUST | skiff_store::KIND_NODE => match v.runtime.port {
            Some(port) => format!("http://127.0.0.1:{port}"),
            None => "端口未分配".to_string(),
        },
        _ => "尚未接入".to_string(),
    };
    let endpoint_short = match v.runtime.kind.as_str() {
        skiff_store::KIND_PHP => "sock".to_string(),
        skiff_store::KIND_RUST | skiff_store::KIND_NODE => v
            .runtime
            .port
            .map(|p| format!(":{p}"))
            .unwrap_or_else(|| "端口".into()),
        _ => "—".to_string(),
    };
    // 已知真实版本（镜像元数据）优先；容器缺失/未安装 → 目标 + 未安装；
    // 容器在但元数据读不到 → 目标 + 版本未读取。停止的容器仍展示真实版本。
    let target_version = v.runtime.version.clone();
    let (version_display, version_note) = version_display(
        v.actual_version.as_deref(),
        &target_version,
        &v.runtime.image,
        v.container_exists,
    );
    let image_short = |id: &str| -> String {
        let raw = id.strip_prefix("sha256:").unwrap_or(id);
        raw.chars().take(12).collect()
    };
    let job = v.job.clone();
    let job_active = job.as_ref().is_some_and(|j| j.active());
    let job_failed = job
        .as_ref()
        .is_some_and(|j| j.state == skiff_app::sites::RuntimeJobState::Failed);
    RuntimeRow {
        id: v.runtime.id.clone(),
        kind_key: v.runtime.kind.clone(),
        kind_label,
        name: v.runtime.name.clone(),
        container_name: v.container_name.clone(),
        version: v.runtime.version.clone(),
        version_display,
        version_note,
        actual_version: v.actual_version.clone().unwrap_or_default(),
        container_exists: v.container_exists,
        image: v.runtime.image.clone(),
        image_id: image_short(&v.image_id),
        image_id_full: v.image_id.clone(),
        image_digest: image_short(&v.image_digest),
        image_digest_full: v.image_digest.clone(),
        image_mismatch: v.image_mismatch,
        state_label: if let Some(j) = job.as_ref().filter(|j| j.active()) {
            // 安装中优先显示任务阶段，避免「未安装」与正在拉镜像互相矛盾。
            format!("{}：{}", j.state.label(), j.step)
        } else if v.drift {
            "状态漂移".to_string()
        } else if job_failed && v.container_state != "running" {
            "安装失败".to_string()
        } else {
            state_label(&v.effective_state).to_string()
        },
        state_class: if job_active || v.drift || job_failed {
            "warn".to_string()
        } else {
            state_class(&v.effective_state).to_string()
        },
        drift: v.drift,
        site_count: v.site_count,
        container_state: v.container_state.clone(),
        endpoint_short,
        endpoint_full,
        port: v.runtime.port.map(|p| p.to_string()).unwrap_or_default(),
        implemented: v.implemented,
        is_default: v.runtime.is_default,
        detail_href: format!("/{prefix}/runtimes/{}", encode_uri_component(&v.runtime.id)),
        docker_href: format!(
            "/{prefix}/containers/{}",
            encode_uri_component(&v.container_name)
        ),
        job_progress: job.as_ref().map(|j| j.progress).unwrap_or(0),
        job_error: job.as_ref().map(|j| j.error.clone()).unwrap_or_default(),
        job_active,
    }
}

fn job_view(job: &skiff_app::sites::RuntimeJob) -> RuntimeJobView {
    RuntimeJobView {
        state: job.state.as_str().to_string(),
        state_label: job.state.label().to_string(),
        step: job.step.clone(),
        progress: job.progress,
        error: job.error.clone(),
        log: job.log_text(),
        active: job.active(),
        failed: job.state == skiff_app::sites::RuntimeJobState::Failed,
        kind: job.kind.clone(),
        ext_keys: job.ext_keys.clone(),
    }
}

fn job_json(job: &skiff_app::sites::RuntimeJob) -> serde_json::Value {
    serde_json::json!({
        "id": job.runtime_id,
        "state": job.state.as_str(),
        "state_label": job.state.label(),
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

fn site_view(
    s: &Site,
    aliases: Vec<String>,
    cert_ready: bool,
    runtime: Option<&Runtime>,
    runtime_state: Option<&str>,
    runtime_site_count: usize,
) -> SiteView {
    let runtime_label = runtime
        .map(|r| {
            format!(
                "{} {}",
                skiff_app::runtime_catalog::kind_label(&r.kind),
                r.version
            )
        })
        .unwrap_or_default();
    let endpoint = match s.site_type.as_str() {
        TYPE_PROXY => {
            let mut t = s.proxy_target.clone().unwrap_or_default();
            if s.proxy_websocket {
                t.push_str(" · WebSocket");
            }
            t
        }
        TYPE_STATIC => "静态文档根".to_string(),
        _ => runtime_label.clone(),
    };
    // 列表合并列展示实例名（php-8.5），不拼「PHP · php-8.5」这类冗余；记录缺失时留空由模板兜底。
    let runtime_name = runtime.map(|r| r.name.clone()).unwrap_or_default();
    // 共享提示：一个环境只有一个进程与端口，重启/升级/改端口会同时影响这些站点。
    let runtime_shared_hint = if s.site_type == TYPE_RUNTIME && runtime_site_count > 0 {
        format!("该环境已被 {runtime_site_count} 个站点使用，重启/升级/改端口会影响全部")
    } else {
        String::new()
    };
    SiteView {
        domain: s.domain.clone(),
        // 站点目录本身（整站备份单元）与文档根都展示出来，避免把 `index/` 说成整站。
        index_dir: skiff_app::sites::site_index_dir(FsPath::new(&s.root_dir))
            .display()
            .to_string(),
        root_dir: s.root_dir.clone(),
        site_type: s.site_type.clone(),
        type_label: type_label(&s.site_type).to_string(),
        runtime_id: s.runtime_id.clone().unwrap_or_default(),
        runtime_label,
        runtime_name,
        runtime_shared_hint,
        proxy_target: s.proxy_target.clone().unwrap_or_default(),
        proxy_websocket: s.proxy_websocket,
        endpoint,
        status: s.status.clone(),
        status_label: if s.site_type == TYPE_RUNTIME {
            runtime_site_status(&s.status, runtime_state).0
        } else {
            match s.status.as_str() {
                "running" => "运行中",
                "stopped" => "已停止",
                _ => "异常",
            }
        }
        .into(),
        status_class: if s.site_type == TYPE_RUNTIME {
            runtime_site_status(&s.status, runtime_state).1
        } else {
            match s.status.as_str() {
                "running" => "ok",
                "stopped" => "",
                _ => "danger",
            }
        }
        .into(),
        https_label: match (s.https, cert_ready) {
            (true, true) => "已启用 HTTPS",
            (true, false) => "证书缺失",
            (false, true) => "证书已上传",
            (false, false) => "未配置 HTTPS",
        }
        .into(),
        https_class: match (s.https, cert_ready) {
            (true, true) => "ok",
            (true, false) => "danger",
            (false, true) => "info",
            (false, false) => "",
        }
        .into(),
        created: chrono::DateTime::from_timestamp(s.created_at, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        is_default: s.is_default,
        aliases_text: aliases.join("、"),
        alias_preview: aliases
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join("、"),
        alias_more: aliases.len().saturating_sub(2),
        aliases,
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

fn map_err(e: SiteError) -> AppError {
    match e.unfield() {
        SiteError::BadRequest(m) => AppError::BadRequest(m),
        SiteError::Conflict(m) => AppError::Conflict(m),
        SiteError::Unavailable(m) => AppError::Unavailable(m),
        SiteError::Internal(m) => AppError::Internal(m),
        SiteError::Field(_, inner) => AppError::Internal(inner.to_string()),
    }
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
fn actor(state: &AppState, user: &crate::state::CurrentUser, parts: &Parts) -> Actor {
    Actor {
        username: user.user.username.clone(),
        ip: handlers::client_ip(state, parts),
    }
}

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

pub(crate) fn require_admin(user: &crate::state::CurrentUser) -> Result<(), AppError> {
    handlers::require_permission(user, skiff_model::Permission::Admin)
}

#[cfg(test)]
mod tests {
    use super::crud::{error_field, save_config_notice};
    use super::{
        map_err, runtime_site_status, site_error_text, state_class, state_label, version_display,
    };
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

    #[test]
    fn version_column_shows_actual_then_falls_back_with_marker() {
        // 已知真实版本：主显补丁号，副行给出目标与镜像。
        let (display, note) = version_display(Some("8.5.12"), "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5.12");
        assert_eq!(note, "目标 8.5 · php:8.5-fpm-alpine");

        // 容器缺失（未安装）：目标 + 未安装。
        let (display, note) = version_display(None, "8.5", "php:8.5-fpm-alpine", false);
        assert_eq!(display, "8.5");
        assert_eq!(note, "未安装");

        // 容器存在但镜像元数据读不到：目标 + 版本未读取，不伪造真实版本。
        let (display, note) = version_display(None, "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5");
        assert_eq!(note, "版本未读取");

        // 停止但容器仍存在 → 上层仍传入镜像元数据版本，照常显示真实版本。
        let (display, note) = version_display(Some("8.5.12"), "8.5", "php:8.5-fpm-alpine", true);
        assert_eq!(display, "8.5.12");
        assert_eq!(note, "目标 8.5 · php:8.5-fpm-alpine");
    }
}
