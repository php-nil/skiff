//! 内核侧 UDS 服务 `/run/skiff/kernel.sock`：插件回调内核的唯一通道（§4.2）。
//!
//! 双重鉴权：`SO_PEERCRED` 的 UID ↔ 插件 id 映射（每个插件独立 UID），
//! 加上进程级 Bearer token（经 stdin 一次性交付）。只凭 socket 可达或 token 单一因素不授权。
//!
//! 能力面现状（诚实标注）：
//! - `identity`：返回插件自身身份与 granted 集；
//! - `config`：返回内核代管的 `config_schema` 取值（§5.2 两轨制的第一轨）；
//! - `fs`：相对路径限插件自身 `data/`；**绝对路径**是宿主文件，额外要求安装时
//!   确认过的 `path:` 授权根命中（判定唯一实现在 `skiff_plugin::fsgrant`），读写按
//!   256KiB 分页（单帧 1MiB），`mkdir`/`remove`/`rename` 不许把授权根本身当目标；
//! - `docker`：白名单动词 + 对象 grant，走与 UI 相同的三层策略；
//! - `interop`：插件间 RPC 转发（`call` / `catalog`）。四道闸（`interop:call`
//!   权限、`plugin:<目标>`/`plugin:*` 对象授权、调用方 `consumes` 白名单、目标
//!   `provides` 声明）由 `skiff_plugin::interop` 判定；这里负责查库、防环、超时与审计。
//!   目标插件仍按自己的授权读写，转发不放宽任何一方。
//! - `net`：**未实现**（nftables 收口属 M4 门禁），一律 501 + 审计。
//!
//! 文件分工（一个职责一个文件，避免单文件既管 socket 又管判定）：
//! - `server`（仅 unix）：socket 建立、属组/权限、accept 循环；
//! - `routes`：能力面路由表与中间件顺序；
//! - `auth`：协议版本闸 + 双重鉴权（peer uid ↔ 插件 id + Bearer token）；
//! - `capability`：identity / config / docker / net 四条能力面；
//! - `interop`：插件间调用与接口目录。
//!
//! 除 `server` 外全部与平台无关：判定与响应组装能在 Windows 开发机上直接编译与单测，
//! 只有真正依赖 unix socket 的那层留在 `#[cfg(unix)]` 里。

use std::sync::Arc;

use skiff_app::AppState;

// 下面四个模块在 unix 上由 `server::run` 挂到 kernel.sock；非 unix 上没有
// socket 可挂，但保留编译——这样模块装配（导入、可见性、路由表）能在开发机上
// 直接被编译器检查，不必等 CI。
#[cfg_attr(not(unix), allow(dead_code))]
mod auth;
#[cfg_attr(not(unix), allow(dead_code))]
mod capability;
#[cfg_attr(not(unix), allow(dead_code))]
mod interop;
#[cfg_attr(not(unix), allow(dead_code))]
mod routes;

#[cfg(unix)]
mod server;

/// 对端进程的 uid（`SO_PEERCRED`）。unix 上 accept 时塞进请求扩展，
/// `auth` 据此把 uid 映射成插件 id。
#[derive(Clone, Copy, Debug)]
pub(super) struct PeerUid(pub u32);

#[cfg(unix)]
pub fn spawn(state: Arc<AppState>, run_dir: std::path::PathBuf) {
    // 必须真后台：server::run 是常驻 accept 循环，被调用方 .await 会永远堵在这，
    // HTTP serve 就排不到（Windows 的 stub 立即返回，掩盖过这个问题）。
    tokio::spawn(async move {
        match server::run(state, run_dir).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "kernel.sock 服务未启动（插件回调能力不可用）"),
        }
    });
}

#[cfg(not(unix))]
pub fn spawn(_state: Arc<AppState>, _run_dir: std::path::PathBuf) {
    tracing::info!("非 unix 平台：跳过 kernel.sock（插件进程外隔离不可用）");
}

#[cfg(test)]
mod tests;
