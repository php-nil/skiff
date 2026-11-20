//! Web 服务（nginx）写操作的共用业务入口：主配置与性能参数的校验、落盘与审计。

use skiff_app::nginx::NginxParams;

use skiff_app::AppState;

/// 主配置保存：`/ui/v1/web/config` 的唯一业务入口。
///
/// 校验（`nginx -t`）、原子写与 `nginx.config.save` 审计都在这里。
pub(crate) async fn apply_main(
    state: &AppState,
    user: &skiff_app::CurrentUser,
    ip: &str,
    content: &str,
) -> Result<String, String> {
    match state.save_nginx_main(content).await {
        Ok(warnings) => {
            let detail = warnings.join("；");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.config.save",
                "nginx.conf",
                "ok",
                &detail,
            );
            Ok(if warnings.is_empty() {
                "主配置已通过 nginx -t 并重载".to_string()
            } else {
                format!(
                    "主配置已保存并重载；nginx -t 有 {} 条告警：{}",
                    warnings.len(),
                    detail
                )
            })
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.config.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Err(msg)
        }
    }
}

/// 性能参数保存：`/ui/v1/web/params` 的唯一业务入口。
pub(crate) async fn apply_params(
    state: &AppState,
    user: &skiff_app::CurrentUser,
    ip: &str,
    params: &NginxParams,
) -> Result<String, String> {
    match state.save_nginx_params(params).await {
        Ok(warnings) => {
            let detail = warnings.join("；");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.params.save",
                "nginx.conf",
                "ok",
                &detail,
            );
            Ok(if warnings.is_empty() {
                "性能参数已保存并通过 nginx -t，随后重载".to_string()
            } else {
                format!(
                    "性能参数已保存并重载；nginx -t 有 {} 条告警：{}",
                    warnings.len(),
                    detail
                )
            })
        }
        Err(e) => {
            let msg = format!("{e}");
            state.audit_or_log(
                &user.user.username,
                ip,
                "nginx.params.save",
                "nginx.conf",
                "failed",
                &msg,
            );
            Err(msg)
        }
    }
}
