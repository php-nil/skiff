//! `firewall` 能力面：宿主机 nftables/iptables 与 fail2ban 管理。
//!
//! 判定与命令编排在 `skiff_plugin::firewall`（能力面实现单一来源）；这里只做
//! 三件内核侧的事：查授权、审计、用 `systemd-run` 提供一条不继承主服务沙箱的
//! 特权执行通道。方法清单与动词见 `docs/KERNEL_FIREWALL_CONTRACT.md`。

#[cfg(target_os = "linux")]
use std::future::Future;
#[cfg(target_os = "linux")]
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::AppState;
use skiff_model::Form;
#[cfg(target_os = "linux")]
use skiff_plugin::firewall::{CommandOutput, FirewallHost};

use skiff_plugin::capability as cap;

pub(super) async fn firewall_call(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));

    if method.is_empty() {
        return (StatusCode::BAD_REQUEST, "缺少 method").into_response();
    }
    let grants = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        _ => Vec::new(),
    };
    let allowed = grants
        .iter()
        .any(|g| g.permits_wildcard(&format!("firewall:{method}")));
    if !allowed {
        audit(
            &state,
            &id,
            &format!("firewall.deny.{method}"),
            "deny",
            "缺少 firewall 能力授权",
        );
        return (StatusCode::FORBIDDEN, "缺少 firewall 能力授权").into_response();
    }

    if state.cfg.form != Form::Native || !cfg!(target_os = "linux") {
        audit(
            &state,
            &id,
            &format!("firewall.{method}"),
            "deny",
            "当前部署形态不支持宿主机防火墙能力",
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "当前部署形态不支持宿主机防火墙能力（需要 Native + Linux）",
        )
            .into_response();
    }

    let result = dispatch(&method, &params).await;
    match result {
        Ok(v) => {
            audit(
                &state,
                &id,
                &format!("firewall.{method}"),
                "ok",
                &audit_detail(&method, &params, true),
            );
            Json(v).into_response()
        }
        Err(e) => {
            audit(
                &state,
                &id,
                &format!("firewall.{method}"),
                if e.status == StatusCode::FORBIDDEN {
                    "deny"
                } else {
                    "fail"
                },
                &format!("{}：{}", audit_detail(&method, &params, false), e.message),
            );
            (e.status, e.message).into_response()
        }
    }
}

fn audit(state: &AppState, id: &str, action: &str, verdict: &str, detail: &str) {
    if let Err(e) = state
        .db()
        .audit(&format!("plugin:{id}"), "uds", action, "", verdict, detail)
    {
        tracing::error!(error = %e, "firewall 审计写入失败");
    }
}

/// 审计只记「做了什么对象」，不记完整命令与文件内容。
fn audit_detail(method: &str, params: &Value, ok: bool) -> String {
    let mut parts = vec![method.to_string()];
    for key in ["jail", "port", "protocol", "source", "action", "ip", "id"] {
        if let Some(v) = params.get(key) {
            let text = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if !text.is_empty() {
                parts.push(format!("{key}={text}"));
            }
        }
    }
    if !ok {
        parts.push("失败".into());
    }
    parts.join(" ")
}

#[cfg(target_os = "linux")]
async fn dispatch(
    method: &str,
    params: &Value,
) -> Result<Value, skiff_plugin::firewall::FirewallError> {
    skiff_plugin::firewall::dispatch(&KernelHost, method, params).await
}

#[cfg(not(target_os = "linux"))]
async fn dispatch(
    _method: &str,
    _params: &Value,
) -> Result<Value, skiff_plugin::firewall::FirewallError> {
    Err(skiff_plugin::firewall::FirewallError::unavailable(
        "firewall 能力面仅在 Linux 宿主机可用",
    ))
}

#[cfg(target_os = "linux")]
struct KernelHost;

#[cfg(target_os = "linux")]
impl FirewallHost for KernelHost {
    fn run<'a>(
        &'a self,
        cmd: &'a str,
        args: &'a [String],
        stdin: Option<&'a str>,
        timeout_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>> {
        Box::pin(async move {
            // 特权执行通道唯一实现在 `kernel::host_run`（ftp 共用）。
            let out = super::host_run::run_host(
                "skiff-firewall",
                "skiff firewall host mutation",
                cmd,
                args,
                stdin,
                timeout_secs,
            )
            .await?;
            Ok(CommandOutput {
                success: out.success,
                stdout: out.stdout,
                stderr: out.stderr,
            })
        })
    }
}
