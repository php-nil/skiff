//! 运行环境读模型：详情所需的核心取数（视图、绑定站点、PHP 配置文件、端口能力）
//! 在这里收口，SSR 详情页与 `/ui/v1` 共用；日志/编辑器等条件扩展仍由各自 handler 处理。

use skiff_store::{Site, KIND_NODE, KIND_PHP, KIND_RUST};

use crate::sites::{PhpConfigFile, RuntimeView, SiteError};
use crate::AppState;

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
