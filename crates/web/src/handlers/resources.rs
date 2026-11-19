//! 镜像 / 网络 / 卷的共用校验：`/ui/v1/resources` 与 CLI 用同一判定。

use crate::error::AppError;

/// 网络/卷名：只允许字母数字与 `_-.`。
pub(crate) fn validate_name(kind: &str, name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
    {
        return Err(AppError::BadRequest(format!(
            "{kind}名只允许字母数字与 _-. "
        )));
    }
    Ok(())
}
/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::resources::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;

/// Docker 命令错误映射：唯一实现在 [`crate::error::map_docker_cmd`]。
pub(crate) use crate::error::map_docker_cmd as map_err;

#[cfg(test)]
mod tests {
    fn row(name: &str, id: &str) -> skiff_docker::NetworkRow {
        skiff_docker::NetworkRow {
            id: id.into(),
            name: name.into(),
            driver: "bridge".into(),
            scope: "local".into(),
            ipam_driver: None,
            subnets: None,
            gateways: None,
            internal: None,
            attachable: None,
            containers: None,
        }
    }

    #[test]
    fn builtin_networks_cannot_be_removed() {
        // 判定在 `NetworkRow`；视图层只消费 can_remove。
        assert!(!skiff_app::views::resources::network_row(row("bridge", "abc")).can_remove);
        assert!(skiff_app::views::resources::network_row(row("app-net", "def")).can_remove);
    }
}
