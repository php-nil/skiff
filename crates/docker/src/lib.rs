//! Docker 管理（核心②）：bollard 封装、三层策略、路径校验、Compose 子集。
//!
//! 三层控制（§6.4）：
//! 1. 动词白名单 —— `policy::Verb` + `Tier`；
//! 2. 资源对象授权 —— `policy::Policy::check` 对容器标签/对象 grant 判定；
//! 3. 参数字段白名单 —— `policy::ContainerSpec`（`deny_unknown_fields`，未列字段直接拒）。
//!
//! 诚实前提：docker.sock 等价宿主 root。以上策略约束「面板与插件能看见并操作什么」，
//! 不改变 socket 本身的权限本质。

// 让上层（web/kernel）能匹配 exec/日志等 bollard 类型，而不必各自依赖 bollard。
pub use bollard;

pub mod client;
// Compose 子集解析已独立为 `skiff-compose`（纯 serde_yaml，零引擎依赖）；
// 这里转出保持 `skiff_docker::compose::parse` 既有写法成立。
pub use skiff_compose as compose;
// 容器引用解析（完整 id / 名称 / id 前缀）属于 Docker 领域语义，住在这里。
pub mod resolve;

pub use client::{ContainerRow, DockerError, DockerParams, DockerService, ImageInfo};
pub use resolve::{match_container, MatchError};

// 策略与路径校验在 `skiff-policy`（零 Docker / 零 SQLite）；这里转出，
// 让 `skiff_docker::Policy`、`skiff_docker::Form` 这些既有写法继续成立。
// 内部还有 `crate::policy::x` 的写法，保留模块别名
pub use skiff_policy as policy;
pub use skiff_policy::{PathError, PathGuard, Policy, PolicyError};
// 词汇层在 `skiff-model`：动词、容器规格、调用主体等纯值类型。
pub use skiff_model::{
    Actor, ContainerSpec, Form, Healthcheck, Mount, ObjectRef, PortMapping, Tier, Verb,
    LABEL_OWNER, LABEL_PROJECT, LABEL_PROTECTED,
};
