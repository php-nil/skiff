//! 三层策略判定（§6.4），**不依赖 Docker、不依赖数据库**。
//!
//! 1. 动词白名单 —— `Verb` + `Tier`（词汇在 `skiff-model`）；
//! 2. 资源对象授权 —— `Policy::check` 按角色 / 插件 grant / 对象标签判定；
//! 3. 参数字段白名单 —— `ContainerSpec`（`deny_unknown_fields`）。
//!
//! 拆成独立 crate 的理由：策略是安全核心，却原本住在 `skiff-docker` 里，
//! 跟着 bollard + tokio + rusqlite 一起编译，导致「判定对不对」这件事必须先有
//! Docker 环境才能验证。现在 `cargo test -p skiff-policy` 是纯粹的演算测试。
//!
//! 注意：这里判定的是「面板与插件能看见/操作什么」，不改变 docker.sock
//! 等价宿主 root 这一事实（§6.4 开头）。
//!
//! 边界：**词汇**（`Verb`/`Tier`/`Actor`/`ObjectRef`/`ContainerSpec`）在 `skiff-model`，
//! **判定**在这里。角色→权限的映射只有一处来源：`skiff_model::Permission`。

mod paths;
mod policy;

pub use paths::{PathError, PathGuard};
pub use policy::{volume_has_host_binding, Policy, PolicyError};

// 词汇从 `skiff-model` 转出：既有调用方写的是 `skiff_policy::Verb` 等，保持路径不变。
pub use skiff_model::{
    Actor, ContainerSpec, Healthcheck, Mount, ObjectRef, PortMapping, Tier, Verb, LABEL_OWNER,
    LABEL_PROJECT, LABEL_PROTECTED,
};
// 部署形态定义在 skiff-model，这里转出（历史调用方写的是 `skiff_docker::Form`）
pub use skiff_model::Form;
