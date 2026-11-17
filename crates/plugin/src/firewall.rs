//! `firewall` 能力面：**宿主命令编排已全部迁移到插件侧**。
//!
//! 防火墙（nftables/iptables 规则 + fail2ban）15 条方法的宿主命令编排现已由
//! `plugins/firewall/src/main.rs` 经内核 exec 能力面提交，任务中心自动有运行记录。
//! 内核 firewall 能力面 dispatch 对所有方法统一返回拒绝——这里只保留内核 handler
//! 需要的三条契约：
//!
//! - [`FirewallHost`]：特权执行通道 trait（内核实现 `skiff_host`）；
//! - [`FirewallError`]：错误类型；
//! - [`CommandOutput`]：宿主命令输出结构；
//! - [`dispatch`]：统一返回迁移提示。

use std::future::Future;
use std::pin::Pin;

use axum::http::StatusCode;
use serde_json::Value;

/// 宿主命令输出（非零退出也算成功执行，由调用方判断 `success`）。
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// 特权命令执行通道。内核实现它（systemd-run 瞬态单元），本模块只声明契约。
pub trait FirewallHost: Send + Sync {
    fn run<'a>(
        &'a self,
        cmd: &'a str,
        args: &'a [String],
        stdin: Option<&'a str>,
        timeout_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>>;
}

/// 能力面错误。内核 handler 直接映射成响应码。
#[derive(Debug)]
pub struct FirewallError {
    pub status: StatusCode,
    pub message: String,
}

impl FirewallError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

/// 能力面入口：全部方法已迁移到插件侧 exec 能力面，统一返回拒绝。
pub async fn dispatch<H: FirewallHost>(
    _host: &H,
    method: &str,
    _params: &Value,
) -> Result<Value, FirewallError> {
    Err(FirewallError::bad_request(format!(
        "方法 `firewall.{method}` 已迁移到插件侧 exec 能力面编排，请更新调用方"
    )))
}
