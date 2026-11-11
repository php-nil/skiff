//! `ftp` 能力面：宿主机 pure-ftpd 的安装、站点账号与 uid/gid 对齐。
//!
//! 判定与编排在 `skiff_plugin::ftp`（能力面实现单一来源）；这里只做三件内核侧
//! 的事：查授权、审计、用 `systemd-run` 提供一条不继承主服务沙箱的特权执行通道。
//! 站点快照由内核从 DB 读出后注入（`FtpContext`），插件不查库。
//! 方法清单与动词见 `docs/KERNEL_FTP_CONTRACT.md`。

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
use skiff_plugin::capability as cap;
#[cfg(not(target_os = "linux"))]
use skiff_plugin::ftp::FtpError;
#[cfg(target_os = "linux")]
use skiff_plugin::ftp::{CommandOutput, FtpContext, FtpError, FtpHost, FtpSite};
#[cfg(target_os = "linux")]
use skiff_store::TYPE_RUNTIME;

/// 审计 detail 只放定位信息：配置摘要、登录名、启用状态；
/// 密码与私钥内容一律不进入审计。
fn audit_ok_detail(method: &str, result: &Value) -> String {
    match method {
        "install" | "service.configure" => result
            .get("config")
            .map(|c| {
                format!(
                    "listen_port={} passive={}-{} tls={} login_guard={} applied={}",
                    c.get("listen_port")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    c.get("passive_port_start")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    c.get("passive_port_end")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    c.get("tls_mode").and_then(Value::as_str).unwrap_or(""),
                    c.get("login_guard")
                        .and_then(Value::as_bool)
                        .unwrap_or_default(),
                    result
                        .get("applied")
                        .and_then(Value::as_bool)
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default(),
        "account.set_enabled" => format!(
            "login={} enabled={}",
            result.get("login").and_then(Value::as_str).unwrap_or(""),
            result
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        ),
        "account.create" => format!(
            "login={} site={} uid={} gid={}",
            result.get("login").and_then(Value::as_str).unwrap_or(""),
            result.get("site").and_then(Value::as_str).unwrap_or(""),
            result
                .get("uid")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            result
                .get("gid")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        ),
        "account.delete" | "account.password" => format!(
            "login={}",
            result.get("login").and_then(Value::as_str).unwrap_or("")
        ),
        "uninstall" => format!(
            "removed_accounts={} removed_package={}",
            result
                .get("removed_accounts")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            result
                .get("removed_package")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
        ),
        _ => String::new(),
    }
}

pub(super) async fn ftp_call(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
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

    let grants = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        _ => Vec::new(),
    };
    let allowed = grants
        .iter()
        .any(|g| g.kind == "perm" && (g.value == "ftp:*" || g.value == format!("ftp:{method}")));
    if !allowed {
        if let Err(e) = state.db().audit(
            &format!("plugin:{id}"),
            "uds",
            &format!("ftp.deny.{}", method),
            &id,
            "deny",
            "缺少 ftp 能力授权",
        ) {
            tracing::error!(error = %e, "ftp 拒绝审计写入失败");
        }
        return (StatusCode::FORBIDDEN, "缺少 ftp 能力授权").into_response();
    }

    match dispatch(&state, &method, &params).await {
        Ok(v) => {
            if let Err(e) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &format!("ftp.{method}"),
                "",
                "ok",
                &audit_ok_detail(&method, &v),
            ) {
                tracing::error!(error = %e, "ftp 调用审计写入失败");
            }
            Json(v).into_response()
        }
        Err(e) => {
            if let Err(ae) = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &format!("ftp.{method}"),
                "",
                if e.status == StatusCode::FORBIDDEN {
                    "deny"
                } else {
                    "fail"
                },
                &e.message,
            ) {
                tracing::error!(error = %ae, "ftp 失败审计写入失败");
            }
            (e.status, e.message).into_response()
        }
    }
}

async fn dispatch(state: &AppState, method: &str, params: &Value) -> Result<Value, FtpError> {
    if state.cfg.form != Form::Native {
        return Err(FtpError::unavailable(
            "受限部署形态不支持宿主机 FTP 服务（需要 apt/systemd/pure-ftpd）",
        ));
    }
    dispatch_host(state, method, params).await
}

#[cfg(not(target_os = "linux"))]
async fn dispatch_host(
    _state: &AppState,
    _method: &str,
    _params: &Value,
) -> Result<Value, FtpError> {
    Err(FtpError::unavailable("ftp 能力面仅在 Linux 宿主机可用"))
}

#[cfg(target_os = "linux")]
async fn dispatch_host(state: &AppState, method: &str, params: &Value) -> Result<Value, FtpError> {
    let ctx = context(state).await?;
    skiff_plugin::ftp::dispatch(&KernelHost, &ctx, method, params).await
}

/// 从 DB 读站点快照并算好运行环境信息，插件侧不再查库。
///
/// `list_sites` + 每站点 `runtime_by_id` 是 N+1：整段投递到一次阻塞任务里，
/// 既不在 worker 上跑，也不为每个站点各排一次队。
#[cfg(target_os = "linux")]
async fn context(state: &AppState) -> Result<FtpContext, FtpError> {
    let db = Arc::clone(state.db());
    let sites_root = state.cfg.web_root.join("sites");
    let sites = skiff_app::db_async::run(move || -> Result<Vec<FtpSite>, FtpError> {
        let sites = db
            .list_sites()
            .map_err(|e| FtpError::internal(format!("读取站点失败：{e}")))?;
        let mut out = Vec::with_capacity(sites.len());
        for s in sites {
            let (runtime_kind, runtime_name, container) =
                match (&s.site_type[..], s.runtime_id.as_deref()) {
                    (TYPE_RUNTIME, Some(id)) => match db.runtime_by_id(id) {
                        Ok(Some(r)) => (
                            Some(r.kind.clone()),
                            Some(r.name.clone()),
                            Some(skiff_app::sites::runtime_container_name("php", &r.name)),
                        ),
                        _ => (Some("missing".into()), None, None),
                    },
                    _ => (None, None, None),
                };
            out.push(FtpSite {
                domain: s.domain,
                site_type: s.site_type,
                root_dir: s.root_dir,
                runtime_kind,
                runtime_name,
                container,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| FtpError::internal(format!("投递站点快照失败：{e}")))??;
    Ok(FtpContext { sites, sites_root })
}

#[cfg(target_os = "linux")]
struct KernelHost;

#[cfg(target_os = "linux")]
impl FtpHost for KernelHost {
    fn run<'a>(
        &'a self,
        cmd: &'a str,
        args: &'a [String],
        stdin: Option<&'a str>,
        timeout_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CommandOutput, String>> + Send + 'a>> {
        Box::pin(async move { run_host(cmd, args, stdin, timeout_secs).await })
    }
}

/// 用 `systemd-run` 起一个不继承主服务沙箱的瞬态单元执行；命令仍是固定
/// 二进制 + 参数数组，不经 shell，密码等敏感内容只走 stdin。
/// 所有 ftp 命令统一走这条通道（含只读探测），主服务的 `ProtectSystem=full`
/// 不影响宿主机动作。
#[cfg(target_os = "linux")]
async fn run_host(
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    timeout_secs: u64,
) -> Result<CommandOutput, String> {
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::AsyncWriteExt;

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let unit = format!("skiff-ftp-{}-{n}", std::process::id());
    let mut full: Vec<String> = [
        "--unit",
        unit.as_str(),
        "--wait",
        "--pipe",
        "--collect",
        "--quiet",
        "--description",
        "skiff ftp host mutation",
        "--",
        cmd,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    full.extend(args.iter().cloned());
    let argv: Vec<&str> = full.iter().map(String::as_str).collect();

    let mut command = tokio::process::Command::new("/usr/bin/systemd-run");
    command
        .args(&argv)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .kill_on_drop(true)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("执行 {cmd} 失败：{e}"))?;
    if let Some(input) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(input.as_bytes())
                .await
                .map_err(|e| format!("写入 {cmd} stdin 失败：{e}"))?;
            // 必须显式关闭写端，`tee` 等命令要读到 EOF 才会退出。
            drop(pipe);
        }
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.max(1)),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("{cmd} 执行超时（{timeout_secs}s）"))?
    .map_err(|e| format!("等待 {cmd} 失败：{e}"))?;
    Ok(CommandOutput {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}
