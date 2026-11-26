//! `firewall` 能力面：**宿主命令编排已全部迁移到插件侧，本面保留为迁移提示**。
//!
//! 防火墙（nftables/iptables 规则 + fail2ban）15 条方法的宿主命令编排现已由
//! `plugins/firewall/src/main.rs` 经内核 exec 能力面提交，任务中心自动有运行记录。
//!
//! 本面**不删**是冻结决定（`docs/plugin/HOST_EXEC_DESIGN.md` §五）：`CAP_FIREWALL_CALL`
//! 仍在路由表与 SDK 的 `firewall_call` 导出面上，旧调用方打到这个入口应得到明确的
//! 「已迁移」400，而不是路由缺失的 404。所以 [`dispatch`] 对所有方法统一返回拒绝——
//! 这是冻结的兑现，不是遗留未清理的死代码，别再当死代码删。
//!
//! 这里只保留内核 handler 需要的两条契约：
//!
//! - [`FirewallError`]：错误类型；
//! - [`dispatch`]：统一返回迁移提示。

use serde_json::Value;

/// 能力面错误。`status` 只带状态码数字：与 `fsops::FsError` 同口径，映射成响应
/// 由 `crates/core/src/kernel/firewall.rs` 负责。
#[derive(Debug)]
pub struct FirewallError {
    pub status: u16,
    pub message: String,
}

impl FirewallError {
    /// 越权类（403）在审计里记 `deny` 而不是 `fail`。
    pub fn is_denied(&self) -> bool {
        self.status == 403
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: msg.into(),
        }
    }

    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: 500,
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
