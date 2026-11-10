//! 菜单领域逻辑已下移到 `skiff-app`（`skiff_app::menu`），这里只做路径兼容的再导出。
//!
//! 模板仍消费同一套 `PageMenu` / `MountView` / `InfoMenu` 结构，返回结构不变；
//! web 层只负责把 `skiff_app::menu` 的结果装进模板。

pub use skiff_app::menu::*;
