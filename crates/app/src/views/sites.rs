//! 站点列表读模型：取数、运行态派生与关键字过滤都在这里，SSR 与 `/ui/v1` 共用。
//!
//! 读模型只携带领域事实（状态码、是否启用 HTTPS、证书是否就绪等）；中文文案与
//! CSS class 属于展示映射，统一在 `skiff-web` 侧生成，避免 JSON 字段依赖样式词表。

use std::path::Path as FsPath;

use skiff_store::{Runtime, Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::sites::{site_index_dir, RuntimeView};
use crate::AppState;

/// 站点列表项：一对一映射到 web 的 `SiteView` 与 `/ui/v1` 的 `SiteItem`。
#[derive(Debug, Clone)]
pub struct SiteSummary {
    pub domain: String,
    pub root_dir: String,
    pub index_dir: String,
    pub site_type: String,
    pub type_label: String,
    pub runtime_id: String,
    pub runtime_label: String,
    pub runtime_name: String,
    pub runtime_shared_hint: String,
    pub proxy_target: String,
    pub proxy_websocket: bool,
    pub endpoint: String,
    pub status: String,
    /// 运行环境站点的有效状态（`running` / `stopped` / `unknown` …）；非运行环境站点为空。
    pub runtime_state: Option<String>,
    pub https: bool,
    pub force_https: bool,
    /// 证书文件是否就绪：决定 HTTPS 展示文案（由 web 侧映射）。
    pub cert_ready: bool,
    /// SSR 展示用时间（`%Y-%m-%d %H:%M`）。
    pub created: String,
    /// UI 转 RFC3339 与游标用。
    pub created_at_epoch: i64,
    /// UI 游标用。
    pub db_id: i64,
    pub is_default: bool,
    pub aliases: Vec<String>,
    pub aliases_text: String,
    pub alias_preview: String,
    pub alias_more: usize,
}

/// 站点列表读模型：已按关键字过滤的站点 + 构造下拉框所需的运行环境数据。
pub struct SiteListRead {
    pub sites: Vec<SiteSummary>,
    pub runtimes: Vec<Runtime>,
    pub runtime_views: Vec<RuntimeView>,
}

/// 站点列表：关键字命中域名/目录/类型详情/别名（大小写不敏感）。
pub async fn list(
    state: &AppState,
    keyword: &str,
) -> Result<SiteListRead, skiff_store::StoreError> {
    let sites = state.db().list_sites()?;
    let runtimes = state.db().list_runtimes()?;
    let runtime_views = state.runtime_views().await.unwrap_or_default();
    let cert_settings = state.site_settings();
    let needle = keyword.trim().to_lowercase();
    let sites = sites
        .iter()
        .map(|s| {
            let runtime = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtimes.iter().find(|r| r.id == id));
            let runtime_view = s
                .runtime_id
                .as_deref()
                .and_then(|id| runtime_views.iter().find(|v| v.runtime.id == id));
            summary(
                state,
                s,
                cert_settings.cert_ready(FsPath::new(&s.root_dir)),
                runtime,
                runtime_view.map(|v| v.effective_state.as_str()),
                runtime_view.map(|v| v.site_count).unwrap_or(0),
            )
        })
        .filter(|s| {
            needle.is_empty()
                || s.domain.to_lowercase().contains(&needle)
                || s.root_dir.to_lowercase().contains(&needle)
                || s.endpoint.to_lowercase().contains(&needle)
                || s.aliases_text.to_lowercase().contains(&needle)
        })
        .collect();
    Ok(SiteListRead {
        sites,
        runtimes,
        runtime_views,
    })
}

/// 单个站点的展示派生：站点详情等只读单个站点的场景直接复用。
pub fn summary(
    state: &AppState,
    s: &Site,
    cert_ready: bool,
    runtime: Option<&Runtime>,
    runtime_state: Option<&str>,
    runtime_site_count: usize,
) -> SiteSummary {
    let runtime_label = runtime
        .map(|r| {
            format!(
                "{} {}",
                crate::runtime_catalog::kind_label(&r.kind),
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
    let aliases = state.site_aliases(&s.domain);
    SiteSummary {
        domain: s.domain.clone(),
        // 站点目录本身（整站备份单元）与文档根都展示出来，避免把 `index/` 说成整站。
        index_dir: site_index_dir(FsPath::new(&s.root_dir))
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
        runtime_state: if s.site_type == TYPE_RUNTIME {
            runtime_state.map(str::to_string)
        } else {
            None
        },
        https: s.https,
        force_https: s.force_https,
        cert_ready,
        created: chrono::DateTime::from_timestamp(s.created_at, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        created_at_epoch: s.created_at,
        db_id: s.id,
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
    }
}

fn type_label(t: &str) -> &'static str {
    match t {
        TYPE_STATIC => "静态网站",
        TYPE_PROXY => "反向代理",
        _ => "运行环境",
    }
}
