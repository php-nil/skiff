//! `firewall` 能力面：**宿主命令编排已全部迁移到插件侧**。
//!
//! 防火墙（nftables/iptables 规则 + fail2ban）15 条方法的宿主命令编排现已由
//! `plugins/firewall/src/main.rs` 经内核 exec 能力面提交，任务中心自动有运行记录。
//! 内核 firewall 能力面 dispatch 对所有方法统一返回拒绝——这里只保留内核 handler
//! 需要的两条契约：
//!
//! - [`FirewallError`]：错误类型；
//! - [`dispatch`]：统一返回迁移提示。

use axum::http::StatusCode;
use serde_json::Value;

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
pub async fn dispatch(method: &str, _params: &Value) -> Result<Value, FirewallError> {
    Err(FirewallError::bad_request(format!(
        "方法 `firewall.{method}` 已迁移到插件侧 exec 能力面编排，请更新调用方"
    )))
}
