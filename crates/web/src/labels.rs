//! 跨页面复用的中文文案映射：任务/作业状态、运行环境扩展来源、商店目录状态、审计分类。
//!
//! App 只输出稳定枚举值（`as_str`），这五类的文案集中在这里，SDUI 与模板共用；
//! 只在一处出现、且与配色 class 或视图语义绑定的文案（站点/容器/插件状态）
//! 留在各自的 handler，避免为「统一」而制造跨模块耦合。

use skiff_app::catalog::CatalogState;
use skiff_app::jobs::JobStatus;
use skiff_app::sites::runtime_catalog::ExtSource;
use skiff_app::sites::RuntimeJobState;
use skiff_model::AuditCategory;

/// 审计分类的展示文案（`AuditCategory` 定义在 `skiff-model`，文案按 AGENTS.md §4.1 留在 web 层）。
pub fn audit_category(category: AuditCategory) -> &'static str {
    match category {
        AuditCategory::Auth => "认证与账号",
        AuditCategory::Site => "站点",
        AuditCategory::Docker => "容器与镜像",
        AuditCategory::Files => "文件",
        AuditCategory::Plugin => "插件",
        AuditCategory::Task => "任务",
        AuditCategory::System => "系统",
        AuditCategory::Other => "其他",
    }
}

pub fn job_status(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "排队中",
        JobStatus::Running => "执行中",
        JobStatus::Succeeded => "已完成",
        JobStatus::Failed => "失败",
        JobStatus::Cancelled => "已取消",
    }
}

pub fn runtime_job_state(state: RuntimeJobState) -> &'static str {
    match state {
        RuntimeJobState::Queued => "排队中",
        RuntimeJobState::Running => "执行中",
        RuntimeJobState::Done => "已完成",
        RuntimeJobState::Failed => "失败",
    }
}

pub fn ext_source(source: ExtSource) -> &'static str {
    match source {
        ExtSource::Builtin => "镜像内置",
        ExtSource::Build => "源码内置",
        ExtSource::Pecl => "PECL",
    }
}

pub fn catalog_state(state: CatalogState) -> &'static str {
    match state {
        CatalogState::Disabled => "未启用",
        CatalogState::Fresh => "已更新",
        CatalogState::Cached => "目录未更新",
        CatalogState::Offline => "离线",
        CatalogState::Rejected => "校验失败",
    }
}
