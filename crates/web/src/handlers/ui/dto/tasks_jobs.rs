//! 任务中心与作业：任务项、运行记录、作业项、作业日志与审计筛选。

#[allow(unused_imports)]
use super::super::*;

/// 作业日志页查询：`after` 为续读游标（缺省从头），`with_schema` 要页面骨架。
#[derive(Debug, Default, Deserialize)]
pub struct JobLogsQuery {
    pub after: Option<i64>,
    #[serde(default)]
    pub with_schema: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskItem {
    pub id: String,
    pub name: String,
    pub expr: String,
    pub next_text: String,
    pub source: String,
    pub target: String,
    pub method: String,
    pub enabled: bool,
    pub state_label: String,
    pub state_class: String,
    pub last_text: String,
    pub last_class: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRunItem {
    pub trigger: String,
    pub started_text: String,
    pub duration: String,
    pub result: String,
    pub result_class: String,
    pub detail: String,
    pub missed: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobItem {
    pub id: String,
    pub title: String,
    pub kind_label: String,
    pub target_label: String,
    pub status_label: String,
    pub status_class: String,
    pub phase_label: String,
    pub progress: i64,
    pub indeterminate: bool,
    pub duration: String,
    pub created_by: String,
    /// RFC3339 UTC（D9）。
    pub created_at: String,
    pub active: bool,
}

/// 审计列表查询参数：关键字 `q`、分类 `cat`、`limit`（默认 50 / 上限 200）与签名游标。
///
/// `with_schema` 只在页面加载时由前端传 `true`；翻页跳转不带该参数。
#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub q: Option<String>,
    /// 分类键（`skiff_model::AuditCategory::key`）；缺省或未知值表示全部分类。
    pub cat: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

/// 一条审计流水；`ts` 为 RFC3339 UTC（D9），`ts_label` 是页面用的本地时间文案。
#[derive(Debug, Serialize)]
pub struct AuditItem {
    pub id: i64,
    pub ts: String,
    pub ts_label: String,
    /// 分类键与展示文案（派生自 action，见 `skiff_model::AuditCategory`）。
    pub category: String,
    pub category_label: String,
    pub actor: String,
    pub ip: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub detail: String,
}
