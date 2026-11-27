//! 通用查询参数与状态：列表分页 / 页面骨架开关 / `{code,label}` 状态对 / 启停开关。

#[allow(unused_imports)]
use super::super::*;

/// 列表接口公共查询参数（D10）：`limit` 默认 50、上限 200；`cursor` 为上一页末项的
/// 不透明键（站点 `created_at:id`，运行环境 `id`）。
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// 站点列表关键字，与页面同名参数一致（域名 / 目录 / 接入方式 / 别名）。
    pub q: Option<String>,
}

/// 单页请求只要页面骨架时的开关（详情类端点用）。
#[derive(Debug, Default, Deserialize)]
pub struct PageFlagQuery {
    #[serde(default)]
    pub with_schema: bool,
}

/// 需要页面骨架的列表查询（任务 / 作业）：通用分页字段 + `with_schema`。
///
/// 不复用 `ListQuery`：资源页与运行环境页不产页面骨架，加一个它们永远忽略的
/// 开关只会让「这个端点会不会下发布局」变得不可判定。
#[derive(Debug, Default, Deserialize)]
pub struct PageListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

/// 文案与状态统一走 `{code,label}` 双字段（D8）。
#[derive(Debug, Clone, Serialize)]
pub struct StatusDto {
    pub code: String,
    pub label: String,
}

/// `POST /ui/v1/sites/{id}/default`：设为 / 取消默认站点（`SITE_SET_DEFAULT`）。
#[derive(Debug, Default, Deserialize)]
pub struct EnabledBody {
    #[serde(default)]
    pub enabled: bool,
}
