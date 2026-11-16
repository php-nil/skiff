//! `exec` 能力面：宿主机命令执行的唯一入口（粗授权 + 全审计 + 运行记录）。
//!
//! 与 `fs` 同一套模型，不引入命令模板（否决理由见
//! `docs/plugin/HOST_EXEC_DESIGN.md` §1）：
//!
//! | fs | exec |
//! | --- | --- |
//! | 权限串 `fs:write` | 权限串 `exec:run` |
//! | 对象授权 `path:<abs>` | 对象授权 `bin:<abs>` |
//! | `fsgrant` 判授权根 | `execgrant` 判二进制身份 |
//! | 即时动作 → 审计 | 耗时任务 → 作业（运行记录） |
//!
//! **为什么不是「请求-响应」而是「提交作业」**：一条特权命令的实际耗时由命令
//! 自己决定（`apt-get install` 几分钟，`fail2ban-client status` 几十毫秒），
//! 用同步 HTTP 等它会把「慢」变成「超时失败」。内核已有的任务中心
//! （`skiff_app::jobs`）本来就是「提交一次、长耗时、可看过程日志」的机制，
//! 环境安装、镜像拉取、站点创建全在它上面。命令执行接进同一张表后：
//!
//! - **审计**记「谁在什么时候请求对哪个二进制做什么」（保留 90 天，安全追溯）；
//! - **运行记录**记「这一次实际怎么跑的、输出了什么、退出码多少」（过程可复查）；
//! - 取消、重试、进度、日志流全部复用任务中心，不再给命令执行造第二套。
//!
//! 请求进来时先做完全部同步前置判定（权限 / 形态 / 二进制身份），失败立即
//! 4xx——**能跑什么在提交那一刻就定了**，不会出现「作业建好了但授权不足」。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::AppState;
use skiff_model::Form;
use skiff_plugin::capability as cap;
use skiff_plugin::execgrant::{self, ExecError};

/// 能力面权限串：只有一个动词，动作集合由 `bin:` 授权决定。
const PERM: &str = "exec:run";

/// 作业分类与目标：`target_type` 固定 `host`，`target_id` 是发起插件。
///
/// 用插件 id 作目标而不是每次唯一：同一插件同时只跑一条特权命令（与「同目标
/// 唯一活跃作业」的部分索引一致），并发提交直接 409 并告知在跑的那条作业。
/// 插件需要并发时应当自己排队——它比内核更清楚哪两条命令能并行。
const JOB_KIND: &str = skiff_app::jobs::KIND_HOST_EXEC;
const TARGET_TYPE: &str = "host";

/// 默认超时 / 上限（秒）。作业化后不受 HTTP 生命周期约束，可以给足；
/// 超过上限的请求被夹到上限而不是拒绝：插件写 99999 秒说明它想「一直等」，
/// 那正是要拦住的意图，但不值得让它 400 重试。
const TIMEOUT_DEFAULT_SEC: u64 = 600;
const TIMEOUT_MAX_SEC: u64 = 3600;

/// 前台等待上限（秒）。`wait_sec = 0` 表示纯异步提交；给值表示「愿意在本次
/// 请求里等这么久」，等不到就带着 `running` 返回 job_id，插件后续查状态。
const WAIT_MAX_SEC: u64 = 120;

/// argv 形状上限：docker/firewall 级命令远到不了这个量级，超了基本是误用或攻击。
const ARGV_MAX_ITEMS: usize = 64;
const ARGV_ITEM_CAP: usize = 4096;
const ARGV_TOTAL_CAP: usize = 16 * 1024;

/// stdin 上限。stdin 用于投递密码/配置内容（不进 argv、不进审计、**不落库**）。
const STDIN_CAP: usize = 64 * 1024;
/// 审计里 argv 的展示上限：够定位问题，又不让审计行变成一份完整命令行转储。
const AUDIT_ARGV_CAP: usize = 512;

/// 一次执行请求（解析后）。`stdin` 只在内存里传给执行体，**不进作业参数**。
struct ExecRequest {
    binary: String,
    argv: Vec<String>,
    stdin: Option<String>,
    timeout_sec: u64,
    wait_sec: u64,
    title: Option<String>,
}

impl ExecRequest {
    fn parse(body: &Value) -> Result<Self, String> {
        let binary = body
            .get("binary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if binary.is_empty() {
            return Err("缺少 binary".into());
        }
        if binary.contains('\0') {
            return Err("binary 含 NUL".into());
        }

        let raw = match body.get("argv") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                if items.len() > ARGV_MAX_ITEMS {
                    return Err(format!("argv 参数过多（上限 {ARGV_MAX_ITEMS}）"));
                }
                let mut out = Vec::with_capacity(items.len());
                let mut total = 0usize;
                for item in items {
                    let s = item
                        .as_str()
                        .ok_or_else(|| "argv 元素必须是字符串".to_string())?;
                    if s.contains('\0') {
                        return Err("argv 元素含 NUL".into());
                    }
                    if s.len() > ARGV_ITEM_CAP {
                        return Err(format!("argv 元素过长（上限 {ARGV_ITEM_CAP} 字节）"));
                    }
                    total += s.len();
                    out.push(s.to_string());
                }
                if total > ARGV_TOTAL_CAP {
                    return Err(format!("argv 总长超限（上限 {ARGV_TOTAL_CAP} 字节）"));
                }
                out
            }
            Some(_) => return Err("argv 必须是字符串数组".into()),
        };

        let stdin = match body.get("stdin") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => {
                if s.len() > STDIN_CAP {
                    return Err(format!("stdin 过长（上限 {STDIN_CAP} 字节）"));
                }
                Some(s.clone())
            }
            Some(_) => return Err("stdin 必须是字符串".into()),
        };

        let timeout_sec = body
            .get("timeout_sec")
            .and_then(Value::as_u64)
            .unwrap_or(TIMEOUT_DEFAULT_SEC)
            .clamp(1, TIMEOUT_MAX_SEC);

        let wait_sec = body
            .get("wait_sec")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(WAIT_MAX_SEC);

        let title = body
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(80).collect::<String>());

        Ok(Self {
            binary,
            argv: raw,
            stdin,
            timeout_sec,
            wait_sec,
            title,
        })
    }
}

// ---------------------------------------------------------------------------
// 待执行 stdin（进程内，不落库）
// ---------------------------------------------------------------------------

/// 作业 id → stdin 内容。提交时放入，执行体启动时取走。
///
/// 为什么不进 `jobs.params_json`：那里会落库并进任务详情的「参数」面板——密码、
/// 私钥、`.pgpass` 这类内容不该因为「有作业记录」就长期留在磁盘上。代价是进程
/// 重启时在途作业会拿不到 stdin，但重启本来就会把在途作业结算为「中断」。
fn stdin_slot() -> &'static Mutex<HashMap<String, String>> {
    static SLOT: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn put_stdin(job_id: &str, stdin: &str) {
    stdin_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(job_id.to_string(), stdin.to_string());
}

fn take_stdin(job_id: &str) -> Option<String> {
    stdin_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(job_id)
}

// ---------------------------------------------------------------------------
// 提交
// ---------------------------------------------------------------------------

pub(super) async fn exec_run(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;

    let parsed = match ExecRequest::parse(&body) {
        Ok(p) => p,
        Err(msg) => {
            audit(&state, &id, "exec.run", "deny", &format!("请求非法: {msg}"));
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
    };

    let grants = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        _ => Vec::new(),
    };
    // 精确命中而不是 `exec:*` 通配：本能力面只有一个动词，留通配只会在将来
    // 加动词时悄悄放宽权限。
    if !grants.iter().any(|g| g.permits(PERM)) {
        audit(&state, &id, "exec.run", "deny", "缺少 exec:run 授权");
        return (StatusCode::FORBIDDEN, "缺少 exec:run 授权").into_response();
    }

    if state.cfg.form != Form::Native || !cfg!(target_os = "linux") {
        audit(
            &state,
            &id,
            "exec.run",
            "deny",
            "当前部署形态不支持宿主机命令执行",
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "当前部署形态不支持宿主机命令执行（需要 Native + Linux）",
        )
            .into_response();
    }

    let data_dir = state.cfg.data_dir.clone();
    let bin = match execgrant::resolve(&data_dir, &grants, &parsed.binary) {
        Ok(p) => p,
        Err(e) => {
            audit(
                &state,
                &id,
                "exec.run",
                e.audit_result(),
                &format!("binary={} 拒绝：{e}", parsed.binary),
            );
            return (status_of(&e), e.to_string()).into_response();
        }
    };
    let bin_display = bin.display().to_string();
    let risk = skiff_model::bin_risk(&bin_display);

    // 作业参数只放「非敏感、可复查」的内容：argv 是要复查的，stdin 不是。
    let job = match state.jobs().submit(skiff_app::jobs::JobSubmit {
        kind: JOB_KIND.to_string(),
        target_type: TARGET_TYPE.to_string(),
        target_id: id.clone(),
        title: parsed.title.clone().unwrap_or_else(|| {
            let shown = parsed.argv.first().map(String::as_str).unwrap_or("");
            if shown.is_empty() {
                format!("执行 {bin_display}")
            } else {
                format!("执行 {bin_display} {shown}")
            }
        }),
        params: json!({
            "plugin": id,
            "binary": bin_display,
            "argv": parsed.argv,
            "timeout_sec": parsed.timeout_sec,
            "risk": risk.as_str(),
        }),
        created_by: format!("plugin:{id}"),
        created_by_role: "plugin".into(),
        requester_ip: String::new(),
        cancellable: true,
    }) {
        Ok(j) => j,
        Err(e) => {
            let (status, msg) = match &e {
                skiff_app::jobs::JobError::Busy(m) => (StatusCode::CONFLICT, m.clone()),
                other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
            };
            audit(
                &state,
                &id,
                "exec.run",
                "deny",
                &format!("bin={bin_display} 提交作业失败：{msg}"),
            );
            return (status, msg).into_response();
        }
    };

    let job_id = job.id.clone();
    if let Some(stdin) = &parsed.stdin {
        put_stdin(&job_id, stdin);
    }

    audit(
        &state,
        &id,
        "exec.run",
        "ok",
        &submitted_detail(&bin_display, risk, &parsed, &job_id),
    );

    let argv = parsed.argv.clone();
    let timeout_sec = parsed.timeout_sec;
    let stdin = take_stdin(&job_id);
    let runner = Arc::clone(&state);
    let runner_job = job_id.clone();
    tokio::spawn(async move {
        // 执行体与内核计划任务共用同一份（`skiff_app::host`）：起进程、读流、写作业日志、
        // 超时/取消结算只有一处实现，两条路只在「谁授权、谁提交」上不同。
        skiff_app::host::drive_exec_job(
            runner.jobs(),
            &runner_job,
            &bin_display,
            &argv,
            timeout_sec,
            stdin,
        )
        .await;
    });

    let snapshot = if parsed.wait_sec > 0 {
        wait_for(&state, &job_id, parsed.wait_sec).await
    } else {
        status_snapshot(&state, &job_id, 0, 0)
    };
    match snapshot {
        Some(v) => Json(v).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "作业已提交但读回失败，请用 exec/status 查询",
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// 状态 / 取消
// ---------------------------------------------------------------------------

/// `exec/status`：状态 + 结果 + 增量日志。
///
/// 只允许查询自己提交的作业：`params.plugin` 在提交时由内核写入，不接受调用方
/// 传参，否则一个插件能读到别人命令的输出（里面可能有密码）。
pub(super) async fn exec_status(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;
    let job_id = match body.get("job_id").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return (StatusCode::BAD_REQUEST, "缺少 job_id").into_response(),
    };
    let after = body.get("after").and_then(Value::as_i64).unwrap_or(0);
    // `limit = 0` 表示只要状态不要日志（SDK 的轮询就走这条，省一次查询）。
    let limit = match body.get("limit").and_then(Value::as_i64).unwrap_or(200) {
        l if l <= 0 => 0,
        l => l.min(500),
    };

    // 先判「有没有这个作业」再判归属：不存在的作业回 404，别人的作业回 403，
    // 两种情形对调用方要做的事不同（改 job_id / 别乱查）。
    let Some(job) = state.jobs().get(&job_id) else {
        return (StatusCode::NOT_FOUND, format!("作业不存在：{job_id}")).into_response();
    };
    if !owned_by(&job, &id) {
        return (StatusCode::FORBIDDEN, "只能查询本插件提交的作业").into_response();
    }
    match status_snapshot(&state, &job_id, after, limit) {
        Some(v) => Json(v).into_response(),
        None => (StatusCode::NOT_FOUND, format!("作业不存在：{job_id}")).into_response(),
    }
}

/// `exec/cancel`：协作式取消。执行体在 250ms 的通道 tick 上轮询取消标记，
/// 命中即 `systemctl stop` 瞬态单元并结算为 `cancelled`。
pub(super) async fn exec_cancel(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;
    let job_id = match body.get("job_id").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return (StatusCode::BAD_REQUEST, "缺少 job_id").into_response(),
    };
    let Some(job) = state.jobs().get(&job_id) else {
        return (StatusCode::NOT_FOUND, format!("作业不存在：{job_id}")).into_response();
    };
    if !owned_by(&job, &id) {
        return (StatusCode::FORBIDDEN, "只能取消本插件提交的作业").into_response();
    }
    match state.jobs().cancel(&job_id) {
        Ok(outcome) => Json(json!({
            "outcome": match outcome {
                skiff_app::jobs::CancelOutcome::CancelledNow => "cancelled",
                skiff_app::jobs::CancelOutcome::Requested => "requested",
                skiff_app::jobs::CancelOutcome::Terminal => "terminal",
            }
        }))
        .into_response(),
        Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
    }
}

/// 作业归属：提交时的 `params.plugin` 是唯一判据（内核写入，不接受调用方传参）。
fn owned_by(job: &skiff_app::jobs::Job, plugin: &str) -> bool {
    job.params.get("plugin").and_then(Value::as_str) == Some(plugin)
}

/// 作业视图：状态 + 结果 + 日志增量。同步等待与后续查询共用同一份结构。
fn status_snapshot(state: &AppState, job_id: &str, after: i64, limit: i64) -> Option<Value> {
    let job = state.jobs().get(job_id)?;
    let (entries, min_seq, max_seq) = if limit <= 0 {
        (Vec::new(), None, job.log_seq)
    } else {
        match state.jobs().logs_since(job_id, after, limit) {
            Some(b) => (
                b.entries
                    .iter()
                    .map(|e| json!({"seq": e.seq, "at": e.at, "stream": e.stream, "line": e.line}))
                    .collect::<Vec<_>>(),
                b.min_seq,
                b.max_seq,
            ),
            None => (Vec::new(), None, job.log_seq),
        }
    };
    let mut view = json!({
        "job_id": job.id,
        "status": job.status.as_str(),
        "active": job.active(),
        "phase": job.phase,
        "phase_label": job.phase_label,
        "progress": job.progress,
        "cancellable": job.cancellable,
        "cancel_requested": job.cancel_requested,
        "created_at": job.created_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
        "duration_ms": job.duration_ms,
        "exit_code": job.exit_code,
        "error_class": job.error_class,
        "error_detail": job.error_detail,
        "logs": { "entries": entries, "min_seq": min_seq, "max_seq": max_seq },
    });
    if !job.active() {
        view["result"] = job.result.clone();
        // 顶层 `success` / `code` 与旧版同步响应同形：插件只关心「成没成、退出码」
        // 时不必再钻进 `result`。
        view["success"] = job.result.get("success").cloned().unwrap_or(Value::Null);
        view["code"] = job.result.get("code").cloned().unwrap_or(Value::Null);
    }
    Some(view)
}

/// 轮询等待终态；超时返回当前 `running` 快照。
async fn wait_for(state: &AppState, job_id: &str, wait_sec: u64) -> Option<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_sec);
    loop {
        let job = state.jobs().get(job_id)?;
        if !job.active() {
            return status_snapshot(state, job_id, 0, 200);
        }
        if std::time::Instant::now() >= deadline {
            return status_snapshot(state, job_id, 0, 200);
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

/// 授权类错误回 4xx，环境类回 409：插件据此能区分「要不要管理员改授权」
/// 与「这台机器现在跑不了」。
fn status_of(e: &ExecError) -> StatusCode {
    match e {
        ExecError::Illegal(_) => StatusCode::BAD_REQUEST,
        ExecError::NotGranted | ExecError::Wrapper | ExecError::Protected => StatusCode::FORBIDDEN,
        _ => StatusCode::CONFLICT,
    }
}

fn audit(state: &AppState, id: &str, action: &str, verdict: &str, detail: &str) {
    if let Err(e) = state
        .db()
        .audit(&format!("plugin:{id}"), "uds", action, "", verdict, detail)
    {
        tracing::error!(error = %e, "exec 审计写入失败");
    }
}

/// 审计 detail：二进制 + 风险档 + argv 全文（截断）+ stdin 字节数 + 作业 id。
///
/// argv 全文是**刻意**记的——本能力面放弃「逐条声明命令」，可复查性就落在审计与
/// 运行日志上。因此纪律是反向的：密码/私钥走 `stdin`（只记字节数，不记内容），
/// 不要放进 argv。
fn submitted_detail(
    bin: &str,
    risk: skiff_model::BinRisk,
    req: &ExecRequest,
    job_id: &str,
) -> String {
    let mut detail = format!("bin={bin} risk={} job={job_id}", risk.as_str());
    if !req.argv.is_empty() {
        let joined = req.argv.join(" ");
        let (shown, cut) = skiff_app::host::clip_text(joined, AUDIT_ARGV_CAP);
        detail.push_str(&format!(" argv={shown}"));
        if cut {
            detail.push_str("…(截断)");
        }
    }
    if let Some(s) = &req.stdin {
        detail.push_str(&format!(" stdin={}B", s.len()));
    }
    detail.push_str(&format!(" timeout={}s", req.timeout_sec));
    detail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_requires_binary_and_accepts_argv() {
        let ok = ExecRequest::parse(&json!({
            "binary": "/usr/bin/fail2ban-client",
            "argv": ["status"]
        }))
        .unwrap();
        assert_eq!(ok.binary, "/usr/bin/fail2ban-client");
        assert_eq!(ok.argv, vec!["status"]);
        assert!(ok.stdin.is_none());
        assert_eq!(ok.timeout_sec, TIMEOUT_DEFAULT_SEC);
        // 缺省是纯异步提交：不占住 HTTP 请求。
        assert_eq!(ok.wait_sec, 0);

        assert!(ExecRequest::parse(&json!({ "argv": [] })).is_err());
        assert!(ExecRequest::parse(&json!({ "binary": "  " })).is_err());
    }

    /// 上限是防护栏：超了直接拒（而不是截断后执行一个和请求不一样的命令）。
    #[test]
    fn parse_rejects_oversized_values() {
        let many: Vec<String> = (0..ARGV_MAX_ITEMS + 1).map(|i| i.to_string()).collect();
        assert!(ExecRequest::parse(&json!({
            "binary": "/bin/x",
            "argv": many
        }))
        .is_err());

        let long = "a".repeat(ARGV_ITEM_CAP + 1);
        assert!(ExecRequest::parse(&json!({
            "binary": "/bin/x",
            "argv": [long]
        }))
        .is_err());

        let stdin = "a".repeat(STDIN_CAP + 1);
        assert!(ExecRequest::parse(&json!({
            "binary": "/bin/x",
            "stdin": stdin
        }))
        .is_err());

        // 非字符串元素与错类型
        assert!(ExecRequest::parse(&json!({ "binary": "/bin/x", "argv": [1] })).is_err());
        assert!(ExecRequest::parse(&json!({ "binary": "/bin/x", "argv": "x" })).is_err());
        assert!(ExecRequest::parse(&json!({ "binary": "/bin/x", "stdin": 1 })).is_err());
    }

    /// 两个时间参数都被夹到上限而不是拒绝：写了 99999 秒说明想「一直等」，
    /// 那正是要拦住的意图，但不值得让它 400 重试。
    #[test]
    fn timeout_and_wait_are_clamped_not_rejected() {
        let parsed = ExecRequest::parse(&json!({
            "binary": "/bin/x",
            "timeout_sec": 99999,
            "wait_sec": 99999
        }))
        .unwrap();
        assert_eq!(parsed.timeout_sec, TIMEOUT_MAX_SEC);
        assert_eq!(parsed.wait_sec, WAIT_MAX_SEC);

        let zero = ExecRequest::parse(&json!({ "binary": "/bin/x", "timeout_sec": 0 })).unwrap();
        assert_eq!(zero.timeout_sec, 1);
    }

    #[test]
    fn title_is_trimmed_and_capped() {
        let t = ExecRequest::parse(&json!({ "binary": "/bin/x", "title": "   " })).unwrap();
        assert!(t.title.is_none());

        let long = "命".repeat(200);
        let t = ExecRequest::parse(&json!({ "binary": "/bin/x", "title": long })).unwrap();
        assert_eq!(t.title.unwrap().chars().count(), 80);
    }

    /// 审计 detail 默认不含 stdout/stderr，但含 argv 全文、stdin 字节数与作业 id。
    #[test]
    fn audit_detail_carries_argv_and_job_id_but_not_stdin_content() {
        let req = ExecRequest {
            binary: "/usr/sbin/pure-pw".into(),
            argv: vec!["useradd".into(), "site1".into()],
            stdin: Some("secret\n".into()),
            timeout_sec: 30,
            wait_sec: 0,
            title: None,
        };
        let d = submitted_detail(&req.binary, skiff_model::BinRisk::Scoped, &req, "job-1");
        assert!(d.contains("bin=/usr/sbin/pure-pw"));
        assert!(d.contains("argv=useradd site1"));
        assert!(d.contains("stdin=7B"));
        assert!(d.contains("job=job-1"));
        assert!(d.contains("timeout=30s"));
        assert!(!d.contains("secret"));
    }

    /// 风险档进审计：授权 root 等价的二进制，事后复查一眼能看出来。
    #[test]
    fn audit_detail_marks_root_equivalence() {
        let req = ExecRequest {
            binary: "/usr/bin/apt-get".into(),
            argv: vec!["install".into(), "-y".into(), "fail2ban".into()],
            stdin: None,
            timeout_sec: 300,
            wait_sec: 0,
            title: None,
        };
        let d = submitted_detail(
            &req.binary,
            skiff_model::bin_risk(&req.binary),
            &req,
            "job-2",
        );
        assert!(d.contains("risk=root-equiv"));
    }

    /// stdin 只在内存里交给执行体一次；取走即删，不会留在任何可查询的地方。
    #[test]
    fn stdin_slot_hands_out_once() {
        put_stdin("job-x", "hunter2\n");
        assert_eq!(take_stdin("job-x").as_deref(), Some("hunter2\n"));
        assert!(take_stdin("job-x").is_none());
    }
}
