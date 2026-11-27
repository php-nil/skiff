//! 主干集成测试（跨 crate 契约守门人）：登录 → 会话 → RBAC → 插件上传/信任/确认安装。
//!
//! 按主题拆分为多个子模块，共享夹具与请求辅助在 [`common`]：
//!
//! - [`auth`]：认证与会话
//! - [`ui_api`]：/ui/v1 契约面
//! - [`plugins`]：插件域
//! - [`sites`]：站点域
//! - [`ops`]：运维域
//! - [`budding`]：budding 壳与静态产物
//!
//! **如实标注的缺口**：
//! - 「建容器」主干不在本套用例：需要 Docker 引擎（Windows 开发机 / CI 无守护进程），
//!   该路径只能在 Linux 真机验证（见 docs/test/DEPLOY_TEST.md 未验证项）。
//! - kernel.sock / Supervisor 拉起插件进程是 unix-only，不在 Windows 测试面内。
//!
//! 运行整体：`cargo test -p skiff-web --test main_flow`；单模块用 `::<模块>::` 过滤。

mod auth;
mod budding;
#[allow(dead_code, unused_imports)]
mod common;
mod ops;
mod plugins;
mod sites;
mod ui_api;
