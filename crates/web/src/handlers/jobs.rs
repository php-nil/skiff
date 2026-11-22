//! 任务中心的读模型与日志流：`/ui/v1/jobs` 的列表/详情行与 SSE 尾随。
//!
//! 权限口径（与 `docs/contract/UI_CONTRACT.md` 一致）：
//! - 列表/详情元数据：登录即可见（viewer 只读，便于判断「为什么某个对象被占用」）；
//! - 日志正文、取消、重试：仅 Admin（日志里会出现宿主路径与安装器输出）。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde_json::{json, Value};

use skiff_app::actions as act;
use skiff_app::jobs::{Job, JobStatus};

use crate::error::AppError;
use crate::middleware::AuthUser;
use crate::templates::{JobDetailView, JobRowView};
use skiff_app::AppState;

/// SSE 每次从库里取的增量上限。
pub(crate) const LOG_PAGE: i64 = 500;
/// SSE 尾随间隔：服务端尾随 DB，客户端只负责追加。
const TAIL_INTERVAL: Duration = Duration::from_millis(400);

fn fmt_ts(ts: Option<i64>) -> String {
    crate::util::fmt_local_time(ts, "%Y-%m-%d %H:%M:%S", "-")
}

pub(crate) fn fmt_log_time(at: i64) -> String {
    crate::util::fmt_local_time(Some(at), "%H:%M:%S", "")
}

fn fmt_duration(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{} 时 {} 分", secs / 3600, (secs % 3600) / 60)
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "runtime.install" => "运行环境安装",
        "runtime.ext.install" => "PHP 扩展安装",
        "runtime.ext.uninstall" => "PHP 扩展卸载",
        "runtime.ext.apply" => "PHP 扩展变更",
        "image.pull" => "镜像拉取",
        "compose.up" => "Compose 启动",
        "compose.down" => "Compose 停止",
        "site.create" => "站点创建",
        "web.install" => "Web 服务安装",
        skiff_app::jobs::KIND_HOST_EXEC => "宿主命令执行",
        _ => "异步任务",
    }
}

fn target_label(job: &Job) -> String {
    let name = job
        .params
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&job.target_id)
        .to_string();
    let kind = job
        .params
        .get("kind")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    match (job.target_type.as_str(), kind) {
        ("runtime", Some(k)) => format!("运行环境 · {name}（{k}）"),
        ("runtime", None) => format!("运行环境 · {name}"),
        ("container", _) => format!("容器 · {name}"),
        ("image", _) => format!("镜像 · {name}"),
        ("compose", _) => format!("Compose · {name}"),
        ("site", _) => format!("站点 · {name}"),
        ("web_env", _) => "Web 服务".to_string(),
        // 命令执行的 target_id 由发起方决定：插件提交时是插件 id，内核计划任务是
        // 任务 id（`params.origin=sched`）。两者必须分开显示——否则一次「站点备份」
        // 会被念成「插件 · builtin:site-backup:example.com」，排障时先认错对象。
        ("host", _) => match job.params.get("origin").and_then(Value::as_str) {
            Some("sched") => format!("计划任务 · {name}"),
            _ => format!("插件 · {name}"),
        },
        _ => name,
    }
}

fn status_class(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "muted",
        JobStatus::Running => "warn",
        JobStatus::Succeeded => "ok",
        JobStatus::Failed => "danger",
        JobStatus::Cancelled => "muted",
    }
}

/// 触发来源：任务中心目前只接受手动提交与「重试」两种来源。
/// 不臆造定时/事件触发——定时任务走内核调度器，不建作业。
fn trigger_label(job: &Job) -> String {
    match job.retry_of.as_deref() {
        Some(parent) => format!("重试 · 原任务 {parent}"),
        None => format!("手动提交 · {}", job.created_by),
    }
}

/// 退出码：只有由单条命令决定成败的作业才有值，其余显示 `-`（不用 0 冒充成功）。
fn exit_code_text(job: &Job) -> String {
    job.exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "-".into())
}

pub(crate) fn row_of(job: &Job) -> JobRowView {
    JobRowView {
        id: job.id.clone(),
        title: job.title.clone(),
        kind_label: kind_label(&job.kind).to_string(),
        target_label: target_label(job),
        status_label: crate::labels::job_status(job.status).to_string(),
        status_class: status_class(job.status).to_string(),
        phase_label: job.phase_label.clone(),
        progress: job.progress,
        indeterminate: job.progress < 0,
        duration: fmt_duration(job.duration_ms),
        created_by: job.created_by.clone(),
        created_at: fmt_ts(Some(job.created_at)),
        active: job.active(),
    }
}

pub(crate) fn detail_of(job: &Job, log_cursor: i64, log_text: String) -> JobDetailView {
    JobDetailView {
        id: job.id.clone(),
        title: job.title.clone(),
        kind_label: kind_label(&job.kind).to_string(),
        target_label: target_label(job),
        status_label: crate::labels::job_status(job.status).to_string(),
        status_class: status_class(job.status).to_string(),
        phase_label: job.phase_label.clone(),
        progress: job.progress,
        indeterminate: job.progress < 0,
        cancellable: job.cancellable,
        cancel_requested: job.cancel_requested,
        created_by: job.created_by.clone(),
        created_by_role: job.created_by_role.clone(),
        requester_ip: job.requester_ip.clone(),
        created_at: fmt_ts(Some(job.created_at)),
        started_at: fmt_ts(job.started_at),
        finished_at: fmt_ts(job.finished_at),
        duration: fmt_duration(job.duration_ms),
        exit_code: exit_code_text(job),
        trigger: trigger_label(job),
        error_class: job.error_class.clone().unwrap_or_default(),
        error_detail: job.error_detail.clone().unwrap_or_default(),
        result_json: if job.result.as_object().is_some_and(|o| o.is_empty()) {
            String::new()
        } else {
            serde_json::to_string_pretty(&job.result).unwrap_or_default()
        },
        params_json: serde_json::to_string_pretty(&job.params).unwrap_or_default(),
        retry_of: job.retry_of.clone().unwrap_or_default(),
        can_cancel: job.can_cancel(),
        // 命令执行不提供「重试」：它由插件在授权有效期内提交，重放一条特权命令
        // 必须重新走插件的授权判定（插件可能已被卸载、`bin:` 授权可能已收回），
        // 页面不显示这个按钮，`retry_job` 也会以「无执行器」拒绝。
        can_retry: job.can_retry() && job.kind != skiff_app::jobs::KIND_HOST_EXEC,
        log_cursor,
        log_text,
    }
}

pub(crate) fn load_job(state: &AppState, id: &str) -> Result<Job, AppError> {
    state
        .jobs()
        .get(id)
        .ok_or_else(|| AppError::NotFound(format!("任务不存在：{id}")))
}
/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
/// 审计主体：唯一实现在 [`crate::handlers::audit_actor`]。此处再导出，
/// 保住模块内 `actor(...)` 调用点与 `handlers::jobs::actor` 跨模块路径。
pub(crate) use crate::handlers::audit_actor as actor;
fn log_event(seq: i64, at: i64, stream_name: &str, line: &str) -> Event {
    Event::default()
        .event("log")
        .data(json!({"seq": seq, "at": at, "stream": stream_name, "line": line}).to_string())
}

fn status_event(job: &Job) -> Event {
    Event::default().event("status").data(
        json!({
            "status": job.status.as_str(),
            "status_label": crate::labels::job_status(job.status),
            "phase": job.phase,
            "phase_label": job.phase_label,
            "progress": job.progress,
            "active": job.active(),
            "cancellable": job.cancellable,
            "cancel_requested": job.cancel_requested,
            "exit_code": job.exit_code,
            "error_detail": job.error_detail,
        })
        .to_string(),
    )
}

/// SSE 日志流：服务端每 400ms 尾随一次 DB。
///
/// 断线重连用 `Last-Event-ID`（或 `?after=`）续传；起点已被轮转时先发
/// `truncated` 事件，客户端据此提示「更早日志已截断」，不会停在等待。
pub async fn logs_sse(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    crate::authz::authorize(&user, act::JOB_LOGS)?;
    load_job(&state, &id)?;
    let mut cursor = q
        .get("after")
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i64>().ok())
        })
        .unwrap_or(0);

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(256);
    let state2 = Arc::clone(&state);
    let job_id = id.clone();
    tokio::spawn(async move {
        loop {
            let Some(job) = state2.jobs().get(&job_id) else {
                let _ = tx
                    .send(Event::default().event("error").data("任务不存在"))
                    .await;
                break;
            };
            if let Some(batch) = state2.jobs().logs_since(&job_id, cursor, LOG_PAGE) {
                if let Some(min) = batch.min_seq {
                    if cursor + 1 < min {
                        let _ = tx
                            .send(
                                Event::default()
                                    .event("truncated")
                                    .data(json!({"before": min}).to_string()),
                            )
                            .await;
                        cursor = min - 1;
                    }
                }
                for e in &batch.entries {
                    if tx
                        .send(log_event(e.seq, e.at, &e.stream, &e.line))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    cursor = e.seq;
                }
            }
            if tx.send(status_event(&job)).await.is_err() {
                return;
            }
            if !job.active() {
                let _ = tx.send(Event::default().event("end").data("ok")).await;
                break;
            }
            tokio::time::sleep(TAIL_INTERVAL).await;
        }
    });

    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok::<_, Infallible>(ev), rx))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
