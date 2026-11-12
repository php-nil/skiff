//! 领域枚举 → 中文展示文案的唯一映射。
//!
//! App 只输出稳定枚举值（`as_str`），中文集中在这里，SDUI/模板/JSON 共用；
//! 新增或调整文案只改本文件。

use skiff_app::catalog::CatalogState;
use skiff_app::jobs::JobStatus;
use skiff_app::runtime_catalog::ExtSource;
use skiff_app::sites::RuntimeJobState;

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
