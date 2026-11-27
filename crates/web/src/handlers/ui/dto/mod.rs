//! `/ui/v1` 的 wire 形状（DTO）：只描述请求/响应字段，不含业务逻辑。
//!
//! 按域分文件，请求体与响应 DTO 同域同居；本文件只做聚合，
//! 经 `ui/mod.rs` 的 `pub use dto::*` 保持 `handlers::ui::Xxx` 既有路径不变。

mod common;
mod compose;
mod containers;
mod files;
mod menu;
mod overview;
mod plugins;
mod runtimes;
mod session;
mod settings;
mod sites;
mod tasks_jobs;
mod web;

pub use common::*;
pub use compose::*;
pub use containers::*;
pub use files::*;
pub use menu::*;
pub use overview::*;
pub use plugins::*;
pub use runtimes::*;
pub use session::*;
pub use settings::*;
pub use sites::*;
pub use tasks_jobs::*;
pub use web::*;
