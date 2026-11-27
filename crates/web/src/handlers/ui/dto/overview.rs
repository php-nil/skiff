//! 概览页只读计数与依赖状态。

#[allow(unused_imports)]
use super::super::*;

/// 概览卡片的计数（只读；数据来自 `dashboard::load_overview` 的既有查询）。
#[derive(Debug, Serialize)]
pub struct OverviewCounts {
    pub collected_at: String,
    pub docker: OverviewDocker,
    pub containers: OverviewContainers,
    pub sites: OverviewSites,
    pub plugins: OverviewPlugins,
    pub runtimes: OverviewRuntimes,
    /// `overview.health` 槽位的提供者（已启用插件）。
    ///
    /// 新壳不渲染 iframe 槽位：前端拿到插件 id 后走统一 RPC 取数，再用 schema 渲染卡片。
    /// 没有可用提供者时为 `null`。
    pub health_plugin: Option<String>,
    /// 没有可用提供者时的可读原因（已停用 / 未安装声明槽位的插件）。
    ///
    /// 前端在 `health_plugin` 为空时渲染原因卡片，不静默消失；取数失败的原因
    /// 由 RPC 错误信封带回，不复用本字段。
    pub health_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OverviewDocker {
    pub status: OverviewStatus,
    pub reason: String,
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Serialize)]
pub struct OverviewStatus {
    pub code: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Serialize)]
pub struct OverviewContainers {
    pub running: usize,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewSites {
    pub total: usize,
    pub abnormal: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewPlugins {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
}

#[derive(Debug, Serialize)]
pub struct OverviewRuntimes {
    pub total: usize,
    pub abnormal: usize,
    pub uninstalled: usize,
}
