//! 运行环境读模型：详情所需的核心取数（视图、绑定站点、PHP 配置文件、端口能力）
//! 在这里收口，SSR 详情页与 `/ui/v1` 共用；日志/编辑器等条件扩展仍由各自 handler 处理。

use skiff_store::{Site, KIND_NODE, KIND_PHP, KIND_RUST};

use crate::runtime_catalog::kind_label;
use crate::sites::{PhpConfigFile, RuntimeJob, RuntimeVersionState, RuntimeView, SiteError};
use crate::AppState;

/// 运行环境行读模型：列表页与 `/ui/v1` 共用同一份取值与派生。
///
/// 只装业务数据；状态文案、色彩类与 URL 属于表现层，由 web 的模板映射与 DTO
/// 映射各自补齐——这样删改模板字段不会连带改 API 组装。
#[derive(Debug, Clone)]
pub struct RuntimeRow {
    pub id: String,
    pub kind: String,
    pub kind_label: String,
    pub name: String,
    pub container_name: String,
    pub version: String,
    pub version_display: String,
    pub version_note: String,
    pub actual_version: String,
    pub container_exists: bool,
    pub image: String,
    pub image_id: String,
    pub image_digest: String,
    pub image_mismatch: bool,
    pub drift: bool,
    pub site_count: usize,
    pub container_state: String,
    pub endpoint_short: String,
    pub endpoint_full: String,
    pub container_address: String,
    pub port: Option<u16>,
    pub publish_port: Option<u16>,
    pub implemented: bool,
    pub is_default: bool,
    pub job: Option<RuntimeJob>,
}

impl RuntimeRow {
    pub fn job_active(&self) -> bool {
        self.job.as_ref().is_some_and(RuntimeJob::active)
    }
}

/// 单行装配：容器接入方式、版本展示与预期缺失的派生都在这里收口。
pub fn row(v: &RuntimeView) -> RuntimeRow {
    let endpoint_full = match v.runtime.kind.as_str() {
        KIND_PHP => {
            if v.socket_host.is_empty() {
                "socket 未登记".to_string()
            } else {
                format!("socket {}", v.socket_host)
            }
        }
        KIND_RUST | KIND_NODE => match v.runtime.port {
            Some(port) => match v.runtime.container_ip.as_deref() {
                Some(ip) => {
                    let mut text = format!("容器 http://{ip}:{port}");
                    match v.runtime.publish_port {
                        Some(host) => text.push_str(&format!("；对外已发布 :{host}")),
                        None => text.push_str("；未对外发布"),
                    }
                    text
                }
                None => "容器地址未分配（需重建环境）".to_string(),
            },
            None => "端口未分配".to_string(),
        },
        _ => "尚未接入".to_string(),
    };
    let container_address = match (v.runtime.container_ip.as_deref(), v.runtime.port) {
        (Some(ip), Some(port)) => format!("{ip}:{port}"),
        _ => String::new(),
    };
    let endpoint_short = match v.runtime.kind.as_str() {
        KIND_PHP => "sock".to_string(),
        KIND_RUST | KIND_NODE => v
            .runtime
            .port
            .map(|p| format!(":{p}"))
            .unwrap_or_else(|| "端口".into()),
        _ => "—".to_string(),
    };
    // 已知真实版本（镜像元数据）优先；容器缺失/未安装 → 目标 + 未安装；
    // 容器在但元数据读不到 → 目标 + 版本未读取。停止的容器仍展示真实版本。
    let (version_display, version_note) = version_display(
        v.actual_version.as_deref(),
        &v.runtime.version,
        &v.runtime.image,
        v.container_exists,
    );
    let image_short = |id: &str| -> String {
        let raw = id.strip_prefix("sha256:").unwrap_or(id);
        raw.chars().take(12).collect()
    };
    RuntimeRow {
        id: v.runtime.id.clone(),
        kind: v.runtime.kind.clone(),
        kind_label: kind_label(&v.runtime.kind),
        name: v.runtime.name.clone(),
        container_name: v.container_name.clone(),
        version: v.runtime.version.clone(),
        version_display,
        version_note,
        actual_version: v.actual_version.clone().unwrap_or_default(),
        container_exists: v.container_exists,
        image: v.runtime.image.clone(),
        image_id: image_short(&v.image_id),
        image_digest: image_short(&v.image_digest),
        image_mismatch: v.image_mismatch,
        drift: v.drift,
        site_count: v.site_count,
        container_state: v.container_state.clone(),
        endpoint_short,
        endpoint_full,
        container_address,
        port: v.runtime.port,
        publish_port: v.runtime.publish_port,
        implemented: v.implemented,
        is_default: v.runtime.is_default,
        job: v.job.clone(),
    }
}

/// 列表版本列：版本来源由内核分类，文案与镜像副行在这里格式化。
pub fn version_display(
    actual: Option<&str>,
    target: &str,
    image: &str,
    container_exists: bool,
) -> (String, String) {
    match crate::sites::runtime_version_state(actual, container_exists) {
        RuntimeVersionState::Actual(v) => (v.to_string(), format!("目标 {target} · {image}")),
        RuntimeVersionState::NotInstalled => (target.to_string(), "未安装".into()),
        RuntimeVersionState::Unreadable => (target.to_string(), "版本未读取".into()),
    }
}

/// 运行环境详情读模型。
pub struct RuntimeDetailRead {
    pub view: RuntimeView,
    /// `runtime_id == id` 的站点记录。
    pub bound_sites: Vec<Site>,
    /// 仅 PHP 非空。
    pub php_files: Vec<PhpConfigFile>,
    pub config_mounts_stale: bool,
    pub port_editable: bool,
    pub port_range: String,
}

/// 运行环境详情：语义与两个 handler 现有取数完全一致。
pub async fn detail(state: &AppState, id: &str) -> Result<RuntimeDetailRead, SiteError> {
    let view = state.runtime_view(id).await?;
    let bound_sites = state
        .db()
        .list_sites()
        .map_err(|e| SiteError::Internal(e.to_string()))?
        .into_iter()
        .filter(|s| s.runtime_id.as_deref() == Some(id))
        .collect();
    let is_php = view.runtime.kind == KIND_PHP;
    let php_files = if is_php {
        state.php_config_files(id)?
    } else {
        Vec::new()
    };
    // 安装时未挂载 php/php.ini 与 conf.d 的存量容器：编辑能保存但不生效，
    // 详情页据此给出重建入口。
    let config_mounts_stale = if is_php && view.container_exists {
        !state.php_config_mounts_current(id)?
    } else {
        false
    };
    // 进程型环境（Node.js / Rust）：端口可改、应用健康可探测、同配置可克隆。
    let port_editable = matches!(view.runtime.kind.as_str(), KIND_NODE | KIND_RUST);
    let port_range = crate::runtime_catalog::port_range(&view.runtime.kind)
        .map(|r| format!("{}–{}", r.start(), r.end()))
        .unwrap_or_default();
    Ok(RuntimeDetailRead {
        view,
        bound_sites,
        php_files,
        config_mounts_stale,
        port_editable,
        port_range,
    })
}

/// 运行环境列表：取数失败时按「无数据」降级，与页面/接口原有口径一致。
pub async fn list(state: &AppState) -> Vec<RuntimeView> {
    state.runtime_views().await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
