//! 站点写操作的共用文案。
//!
//! 站点页面已全部迁到 budding 壳（`/ui/v1/sites/*`），本模块只剩壳页与 JSON 通道
//! 共用的提示文案，避免两处各写一份。

/// 保存站点的成功文案：只有真正重载成功才能说「已重载」。
pub(crate) fn save_config_notice(reloaded: bool) -> &'static str {
    if reloaded {
        "配置已保存、校验通过并已重载 nginx"
    } else {
        "配置已保存并通过校验（站点未运行，未重载）"
    }
}
