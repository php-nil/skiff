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
//!
//! **两条通道、一套判定**：作业化 `exec/run`（提交 → 轮询 → 终态，写 `jobs`）
//! 与轻量 `exec/run_light`（一次 HTTP 返回，不写作业）。授权、形态、二进制身份、
//! argv/stdin 上限全部复用；区别只在「要不要作业记录与取消」。短命令 / 只读探测
//! 走轻量（不抢同插件唯一活跃槽位），长任务 / 有副作用走作业化。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::host::OUTPUT_CAP;
use skiff_app::AppState;
use skiff_host::HostSpec;
use skiff_model::Form;
use skiff_plugin::capability as cap;
use skiff_plugin::execgrant::{self, ExecError};
use tokio::sync::Semaphore;

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

/// 轻量通道的超时默认 / 上限（秒）。跑超过 30s 的命令本身就是长任务，
/// 应该走作业化通道（有过程日志与取消）；夹取而不是拒绝，理由同上。
const LIGHT_TIMEOUT_DEFAULT_SEC: u64 = 10;
const LIGHT_TIMEOUT_MAX_SEC: u64 = 30;

/// 每插件轻量通道并发上限。一页探测约 10 条，8 足够覆盖正常并发；
/// 拿不到槽位立即 429（探测可以稍后重试，排队会拖住调用方线程）。
/// 跨插件不设全局上限（`docs/plugin/HOST_EXEC_DESIGN.md` §6.3）。
const LIGHT_CONCURRENCY: usize = 8;

/// 前台等待上限（秒）。`wait_sec = 0` 表示纯异步提交；给值表示「愿意在本次
/// 请求里等这么久」，等不到就带着 `running` 返回 job_id，插件后续查状态。
const WAIT_MAX_SEC: u64 = 120;

/// `exec/status` 长轮询上限（毫秒）与同时挂起数上限。
///
/// 挂起只占一个 tokio 任务（不占线程），但无上限会让插件用海量挂起请求堆内存；
/// 拿不到槽位时立刻返回当前快照——长轮询是省请求的优化，不是正确性依赖。
const STATUS_WAIT_MAX_MS: u64 = 30_000;
const STATUS_WAIT_CONCURRENCY: usize = 64;

fn status_wait_permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| Semaphore::new(STATUS_WAIT_CONCURRENCY))
}

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
    /// 默认上限/默认值的便捷入口——单测友好，生产直接调 `parse_with`。
    #[cfg_attr(not(test), allow(dead_code))]
    fn parse(body: &Value) -> Result<Self, String> {
        Self::parse_with(body, TIMEOUT_DEFAULT_SEC, TIMEOUT_MAX_SEC)
    }

    /// 两条通道共用同一份字段解析：只有 `timeout_sec` 的默认值与上限不同，
    /// 其它限制（argv / stdin 形状）必须完全一致，不能各写一份。
    fn parse_with(body: &Value, timeout_default: u64, timeout_max: u64) -> Result<Self, String> {
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
            .unwrap_or(timeout_default)
            .clamp(1, timeout_max);

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
// 共享前置判定（两条通道共用）
// ---------------------------------------------------------------------------

/// 两条通道共用的前置判定链：请求体解析 → `exec:run` 权限 → Native+Linux 形态闸 →
/// `execgrant::resolve` 三道闸。只有 `timeout_sec` 的默认值与上限两条通道不同。
///
/// 成功时返回 `(ExecRequest, binary_display, risk)`；失败时返回
/// `(StatusCode, error_message)` —— 审计已在内部按需写入，调用方只负责返回。
fn pre_check_exec(
    body: &Value,
    id: &str,
    state: &Arc<AppState>,
    action: &str,
    timeout_default: u64,
    timeout_max: u64,
) -> Result<(ExecRequest, String, skiff_model::BinRisk), (StatusCode, String)> {
    let parsed = ExecRequest::parse_with(body, timeout_default, timeout_max).map_err(|msg| {
        audit(state, id, action, "deny", &format!("请求非法: {msg}"));
        (StatusCode::BAD_REQUEST, msg)
    })?;

    let grants = state.plugin_auth_of(id).unwrap_or_default();
    // 精确命中而不是 `exec:*` 通配：本能力面只有一个动词，留通配只会在将来
    // 加动词时悄悄放宽权限。
    if !grants.iter().any(|g| g.permits(PERM)) {
        audit(state, id, action, "deny", "缺少 exec:run 授权");
        return Err((StatusCode::FORBIDDEN, "缺少 exec:run 授权".into()));
    }

    if state.cfg.form != Form::Native || !cfg!(target_os = "linux") {
        audit(
            state,
            id,
            action,
            "deny",
            "当前部署形态不支持宿主机命令执行",
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "当前部署形态不支持宿主机命令执行（需要 Native + Linux）".into(),
        ));
    }

    let data_dir = state.cfg.data_dir.clone();
    let bin = execgrant::resolve(&data_dir, &grants, &parsed.binary).map_err(|e| {
        audit(
            state,
            id,
            action,
            e.audit_result(),
            &format!("binary={} 拒绝：{e}", parsed.binary),
        );
        (status_of(&e), e.to_string())
    })?;

    let bin_display = bin.display().to_string();
    let risk = skiff_model::bin_risk(&bin_display);
    Ok((parsed, bin_display, risk))
}

// ---------------------------------------------------------------------------
// 提交（作业化通道）
// ---------------------------------------------------------------------------

pub(super) async fn exec_run(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;

    let (parsed, bin_display, risk) = match pre_check_exec(
        &body,
        &id,
        &state,
        "exec.run",
        TIMEOUT_DEFAULT_SEC,
        TIMEOUT_MAX_SEC,
    ) {
        Ok(v) => v,
        Err((code, msg)) => return (code, msg).into_response(),
    };

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
// 轻量通道
// ---------------------------------------------------------------------------

/// 插件 id → 轻量通道信号量（进程内，懒建）。
///
/// 判定的东西与作业化通道完全相同，这里只限流：探测是「随时可以重试」的动作，
/// 拿不到槽位立刻 429 比排队更诚实（排队会把调用方线程一起拖住）。
fn light_permits(plugin: &str) -> Arc<Semaphore> {
    static PERMITS: OnceLock<Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();
    let map = PERMITS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        guard
            .entry(plugin.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(LIGHT_CONCURRENCY))),
    )
}

/// 轻量通道：同授权、并行跑、不写作业、一次 HTTP 返回（契约 §2.2）。
///
/// 与 [`exec_run`] 的差别只有「去掉作业管理层」：不 submit、不占同插件唯一活跃槽位、
/// 不写 jobs/job_logs、不可取消。前置判定一步不少——复用共享的 [`pre_check_exec`]。
pub(super) async fn exec_run_light(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;

    let (parsed, bin_display, risk) = match pre_check_exec(
        &body,
        &id,
        &state,
        "exec.run_light",
        LIGHT_TIMEOUT_DEFAULT_SEC,
        LIGHT_TIMEOUT_MAX_SEC,
    ) {
        Ok(v) => v,
        Err((code, msg)) => return (code, msg).into_response(),
    };

    // 槽位在审计之前拿：拿不到就是「没跑」，不能先记一条 ok。
    let permits = light_permits(&id);
    let Ok(_permit) = permits.try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            format!("轻量执行并发已满（每插件上限 {LIGHT_CONCURRENCY}），请稍后重试"),
        )
            .into_response();
    };

    audit(
        &state,
        &id,
        "exec.run_light",
        "ok",
        &light_detail(&bin_display, risk, &parsed.argv),
    );

    let out = skiff_app::host::run_host(
        HostSpec::privileged(&bin_display, &parsed.argv)
            .prefix("skiff-exec-light")
            .description("skiff host exec (light)")
            .stdin(parsed.stdin.as_deref())
            .timeout(parsed.timeout_sec),
    )
    .await;

    match out {
        Ok(out) => {
            let (stdout, out_cut) = skiff_app::host::clip_text(out.stdout, OUTPUT_CAP);
            let (stderr, err_cut) = skiff_app::host::clip_text(out.stderr, OUTPUT_CAP);
            Json(json!({
                "success": out.success,
                "code": out.code,
                "stdout": stdout,
                "stderr": stderr,
                "truncated": { "stdout": out_cut, "stderr": err_cut },
            }))
            .into_response()
        }
        // 超时是「结果未知」：单元可能仍在跑，调用方需要按可重试的探测处理。
        Err(e @ skiff_host::HostRunError::Timeout { .. }) => {
            (StatusCode::GATEWAY_TIMEOUT, e.message(&bin_display)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.message(&bin_display)).into_response(),
    }
}

/// 轻量通道审计 detail：二进制 + 风险档 + argv 截断。
///
/// **不写 job_id**（没有作业）、**不写 exit_code**（属运行日志范畴）、**不写 stdin**
/// （只在内存里交给通道）。action 与作业化通道分开，90 天追溯里可分别 filter。
fn light_detail(bin: &str, risk: skiff_model::BinRisk, argv: &[String]) -> String {
    let mut detail = format!("bin={bin} risk={}", risk.as_str());
    if !argv.is_empty() {
        let (shown, cut) = skiff_app::host::clip_text(argv.join(" "), AUDIT_ARGV_CAP);
        detail.push_str(&format!(" argv={shown}"));
        if cut {
            detail.push_str("…(截断)");
        }
    }
    detail
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
    // 长轮询：作业未结束时最多挂起这么久，状态变化或超时后返回（仍是单请求单响应）。
    // 与 `after` / `limit` 正交，不影响日志游标语义。
    let wait_ms = parse_wait_ms(&body);

    // 先判「有没有这个作业」再判归属：不存在的作业回 404，别人的作业回 403，
    // 两种情形对调用方要做的事不同（改 job_id / 别乱查）。
    let Some(job) = state.jobs().get(&job_id) else {
        return (StatusCode::NOT_FOUND, format!("作业不存在：{job_id}")).into_response();
    };
    if !owned_by(&job, &id) {
        return (StatusCode::FORBIDDEN, "只能查询本插件提交的作业").into_response();
    }
    let snapshot = if wait_ms > 0 {
        wait_for_status(&state, &job_id, wait_ms, after, limit).await
    } else {
        status_snapshot(&state, &job_id, after, limit)
    };
    match snapshot {
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

/// `wait_ms` 夹到上限而不是拒绝：插件想「一直等」正是要拦住的意图，但不值得 400 重试。
fn parse_wait_ms(body: &Value) -> u64 {
    body.get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(STATUS_WAIT_MAX_MS)
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

/// `exec/status` 长轮询：作业进入终态或到点后返回快照。
///
/// 只改变「什么时候回」，不改变响应形状；日志游标 `after` / `limit` 原样透传。
async fn wait_for_status(
    state: &AppState,
    job_id: &str,
    wait_ms: u64,
    after: i64,
    limit: i64,
) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    let Ok(_permit) = status_wait_permits().try_acquire() else {
        // 槽位不足：立刻返回当前快照，插件下一轮继续问即可。
        return status_snapshot(state, job_id, after, limit);
    };
    loop {
        let job = state.jobs().get(job_id)?;
        if !job.active() || Instant::now() >= deadline {
            return status_snapshot(state, job_id, after, limit);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    // 统一出口：写库失败落盘兜底，不静默丢弃（与作业化通道同一份）。
    state.audit_or_log(&format!("plugin:{id}"), "uds", action, "", verdict, detail);
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

    /// `wait_ms` 缺省为 0（纯快照），超上限夹到 30s 而不是报错。
    #[test]
    fn status_wait_ms_defaults_and_clamps() {
        assert_eq!(parse_wait_ms(&json!({})), 0);
        assert_eq!(parse_wait_ms(&json!({ "wait_ms": 1500 })), 1500);
        assert_eq!(
            parse_wait_ms(&json!({ "wait_ms": 999_999 })),
            STATUS_WAIT_MAX_MS
        );
        // 非数字：按缺省处理，不影响状态查询本身。
        assert_eq!(parse_wait_ms(&json!({ "wait_ms": "5s" })), 0);
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

    /// 轻量通道的 timeout 默认 10s、上限 30s：超过 30s 的命令本身就是长任务，
    /// 应当走作业化通道；夹取而不是拒绝，与作业化通道同一取舍。
    #[test]
    fn light_timeout_defaults_to_ten_and_caps_at_thirty() {
        let parse = |body: Value| {
            ExecRequest::parse_with(&body, LIGHT_TIMEOUT_DEFAULT_SEC, LIGHT_TIMEOUT_MAX_SEC)
                .unwrap()
                .timeout_sec
        };
        assert_eq!(parse(json!({ "binary": "/usr/sbin/nft" })), 10);
        assert_eq!(
            parse(json!({ "binary": "/usr/sbin/nft", "timeout_sec": 0 })),
            1
        );
        assert_eq!(
            parse(json!({ "binary": "/usr/sbin/nft", "timeout_sec": 999 })),
            30
        );
    }

    /// 轻量通道审计只有「对哪个二进制做什么」：没有作业 id、没有 exit_code、
    /// 不出现 stdin 内容（敏感内容只在内存里交给通道）。
    #[test]
    fn light_audit_detail_has_no_job_id_or_stdin() {
        let req = ExecRequest {
            binary: "/usr/sbin/nft".into(),
            argv: vec!["--version".into()],
            stdin: Some("secret\n".into()),
            timeout_sec: 10,
            wait_sec: 0,
            title: None,
        };
        let d = light_detail(&req.binary, skiff_model::BinRisk::Scoped, &req.argv);
        assert_eq!(d, "bin=/usr/sbin/nft risk=scoped argv=--version");
        assert!(!d.contains("job="));
        assert!(!d.contains("secret"));
    }

    /// 信号量按插件隔离且上限固定：同一插件拿到同一个实例，跨插件互不影响，
    /// 槽位耗尽时立即拒绝（429 的依据）。
    #[test]
    fn light_permits_are_per_plugin_and_bounded() {
        let a = light_permits("com.example.perm.a");
        let same = light_permits("com.example.perm.a");
        let other = light_permits("com.example.perm.b");
        assert!(Arc::ptr_eq(&a, &same));
        assert!(!Arc::ptr_eq(&a, &other));

        let held: Vec<_> = (0..LIGHT_CONCURRENCY)
            .map(|_| a.clone().try_acquire_owned().expect("槽位应可获取"))
            .collect();
        assert!(a.clone().try_acquire_owned().is_err(), "超出上限必须被拒");
        drop(held);
        assert!(a.clone().try_acquire_owned().is_ok(), "释放后应恢复");
    }
}
