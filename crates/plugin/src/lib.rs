//! 插件体系（内核侧，核心③）：安装/升级/卸载、Supervisor、UDS JSON-RPC。
//!
//! 沙箱现状（**必须如实标注**）：
//! - 已实现：独立 UID/GID（unix，按插件 id 稳定分配）、`PR_SET_NO_NEW_PRIVS`、
//!   `setrlimit`(AS/NOFILE/NPROC/FSIZE)、`PR_SET_PDEATHSIG`、env 白名单、一次性
//!   身份令牌经 stdin 交付（不落环境变量）、空闲回收、崩溃退避与熔断。
//! - **未实现**：landlock 文件白名单、cgroups v2 限额、nftables 出网收口。
//!   因此 `requires_full_sandbox = true` 的插件默认拒装；admin 显式开启
//!   `allow_reduced_sandbox` 后可降级安装并收到警告（§4.2 降级路径）。
//!   这三项是 M4 的硬门槛，见 §9.1 与 docs/DEPLOY_TEST.md「未验证项」。

pub mod capability;
/// 宿主机防火墙/fail2ban 能力面（内核 `firewall` handler 调用的判定与命令编排）。
pub mod firewall;
pub mod fsgrant;
pub mod fsops;
/// 宿主机 pure-ftpd 能力面（内核 `ftp` handler 调用的判定与命令编排）。
pub mod ftp;
pub mod install;
pub mod interop;
pub mod ipc;
pub mod official;
pub mod supervisor;
pub mod trust;

pub use capability::CapabilityHost;
pub use install::{
    install_options, install_package, uninstall_package, InstallOptions, InstalledInfo, PluginError,
};
pub use official::{sync_official_plugins, OfficialSyncReport};
// 清单词汇的 canonical 定义在 skiff-pkgfmt；此处转出仅为兼容，新代码请直接 `use skiff_pkgfmt::…`。
pub use skiff_pkgfmt::manifest::{Capability, Manifest, RunMode};
pub use supervisor::{Supervisor, SupervisorConfig};
pub use trust::{resolve as resolve_trust, TrustDecision, TrustError};
