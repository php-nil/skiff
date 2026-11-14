//! 任务中心：进行中与历史作业的列表、详情、日志流与取消/重试。
//!
//! 权限口径（与 `docs/UI_CONTRACT.md`、运行环境页一致）：
//! - 列表/详情元数据：登录即可见（viewer 只读，便于判断「为什么某个对象被占用」）；
//! - 日志正文、取消、重试：仅 Admin（日志里会出现宿主路径与安装器输出）。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use skiff_app::actions as act;
use skiff_app::commands::{job as job_cmd, Actor};
use skiff_app::jobs::{CancelOutcome, Job, JobStatus};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::sites::SiteError;
use crate::state::AppState;
use crate::templates::{JobDetailPage, JobDetailView, JobRowView, JobsPage};
use crate::util::{form_first, parse_form};

/// SSE 每次从库里取的增量上限。
const LOG_PAGE: i64 = 500;
/// SSE 尾随间隔：服务端尾随 DB，客户端只负责追加。
const TAIL_INTERVAL: Duration = Duration::from_millis(400);

fn fmt_ts(ts: Option<i64>) -> String {
    use chrono::TimeZone;
    ts.and_then(|t| chrono::Local.timestamp_opt(t, 0).single())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "-".into())
}

pub(crate) fn fmt_log_time(at: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(at, 0)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
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
        can_retry: job.can_retry(),
        log_cursor,
        log_text,
    }
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let jobs = state.jobs().list(200);
    let rows: Vec<JobRowView> = jobs.iter().map(row_of).collect();
    let has_active = rows.iter().any(|r| r.active);
    handlers::render(JobsPage {
        ctx: handlers::page_ctx(&state, &user, "core/jobs"),
        rows,
        has_active,
        error: String::new(),
        notice: String::new(),
    })
}

pub(crate) fn load_job(state: &AppState, id: &str) -> Result<Job, AppError> {
    state
        .jobs()
        .get(id)
        .ok_or_else(|| AppError::NotFound(format!("任务不存在：{id}")))
}

pub async fn detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let job = load_job(&state, &id)?;
    // 首屏先渲染日志尾巴，之后只靠 SSE 增量追加；游标取尾巴最后一条的 seq。
    let tail = state.jobs().tail(&job.id, 200);
    let cursor = tail.last().map(|r| r.seq).unwrap_or(0);
    let log_text = tail
        .iter()
        .map(|r| format!("[{}] {}", fmt_log_time(r.at), r.line))
        .collect::<Vec<_>>()
        .join("\n");
    handlers::render(JobDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/jobs"),
        job: detail_of(&job, cursor, log_text),
        can_admin: user.can(skiff_model::Permission::Admin),
    })
}

fn job_back(state: &AppState, id: &str) -> String {
    format!("/{}/jobs/{id}", state.cfg.entry_prefix)
}

/// 命令调用主体：用户名 + 客户端 IP，只用于审计，不参与权限判定。
pub(crate) fn actor(state: &AppState, user: &crate::state::CurrentUser, parts: &Parts) -> Actor {
    Actor {
        username: user.user.username.clone(),
        ip: handlers::client_ip(state, parts),
    }
}

pub async fn cancel(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::JOB_CANCEL)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let outcome = job_cmd::cancel(
        &state,
        job_cmd::CancelJob {
            id: id.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let msg = match outcome {
        CancelOutcome::CancelledNow => "已取消（任务尚未开始）",
        CancelOutcome::Requested => "已请求取消，将在当前步骤结束后停止",
        CancelOutcome::Terminal => "任务已结束，无需取消",
    };
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        msg,
        job_back(&state, &id),
    ))
}

pub async fn retry(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::JOB_RETRY)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let job = job_cmd::retry(
        &state,
        job_cmd::RetryJob {
            id: id.clone(),
            role: user.user.role.as_str().to_string(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(|e| match e.unfield() {
        SiteError::BadRequest(m) => AppError::BadRequest(m),
        SiteError::Conflict(m) => AppError::Conflict(m),
        SiteError::Unavailable(m) => AppError::Unavailable(m),
        SiteError::Internal(m) => AppError::Internal(m),
        SiteError::Field(_, inner) => AppError::Internal(inner.to_string()),
    })?;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        format!("已新建重试任务 {}", job.id),
        format!("/{}/jobs/{}", state.cfg.entry_prefix, job.id),
    ))
}

/// 增量日志：浏览器地址栏导航交 budding 壳页；脚本 / 集成测试仍拿 JSON。
pub async fn logs_json(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::JOB_LOGS)?;
    // 与站点 / 容器 / 运行环境日志一致：同一个路径按 `Accept` 分流，
    // 导航请求给壳页，页内脚本再以非 HTML 请求取 JSON，不新增第二套路由。
    if crate::handlers::spa::accepts_html(&headers) {
        return Ok(crate::handlers::spa::shell_page(&state));
    }
    let job = load_job(&state, &id)?;
    let after = q
        .get("after")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let batch = state
        .jobs()
        .logs_since(&id, after, LOG_PAGE)
        .ok_or_else(|| AppError::NotFound(format!("任务不存在：{id}")))?;
    let entries: Vec<Value> = batch
        .entries
        .iter()
        .map(|e| {
            json!({
                "seq": e.seq,
                "at": e.at,
                "stream": e.stream,
                "line": e.line,
            })
        })
        .collect();
    let truncated_before = match batch.min_seq {
        Some(min) if after + 1 < min => json!(min),
        _ => Value::Null,
    };
    Ok(Json(json!({
        "job": {
            "id": job.id,
            "status": job.status.as_str(),
            "status_label": crate::labels::job_status(job.status),
            "phase": job.phase,
            "phase_label": job.phase_label,
            "progress": job.progress,
            "active": job.active(),
            "cancellable": job.cancellable,
            "cancel_requested": job.cancel_requested,
            "exit_code": job.exit_code,
            "duration_ms": job.duration_ms,
        },
        "min_seq": batch.min_seq,
        "max_seq": batch.max_seq,
        "truncated_before": truncated_before,
        "entries": entries,
    }))
    .into_response())
}

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
