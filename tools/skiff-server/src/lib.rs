//! Skiff-server P0：商店清单的 schema 校验与静态产物生成。
//!
//! 只做服务端产物这一侧，不依赖内核 crate，也不引入常驻服务。

pub mod build;
pub mod check;
pub mod model;
pub mod release;
pub mod schema;
pub mod sign;
pub mod sync;
