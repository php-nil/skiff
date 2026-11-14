//! 读模型（read model）：SSR 页面与 `/ui/v1` 共用的只读查询编排。
//!
//! 这里只做「取数 + 领域派生」，不含路由、模板与 JSON 形状；web 层负责把结果
//! 映射成 `SiteView` / DTO，避免同一页面与 UI 接口各写一套过滤与派生逻辑。

pub mod runtimes;
pub mod settings;
pub mod sites;
