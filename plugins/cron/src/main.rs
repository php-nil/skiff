//! `com.skiff.cron`：计划任务的**纯管理 UI**。
//!
//! 调度已经全部收归内核（`skiff-sched` + `crates/app/src/tasks.rs`）。本插件不再
//! 自带计时器、不再按表达式匹配任务，只把面板上的查看/新建/启停/删除/立即执行
//! 请求翻译成内核 `scheduler` 能力面（`task.*`）调用：
//!
//! - 任务定义、运行历史、下次触发时间都由内核提供，插件重启不丢任务；
//! - 任务执行由内核调度器发起（内置任务走 Rust 执行体，插件任务走 Supervisor）；
//! - 插件只持有 `scheduler:read/manage/run/admin`，不落任何任务状态文件；
//! - 内核在插件作为执行目标期间拒绝其回调 `scheduler`，因此「任务 → 注册新任务」
//!   的自我繁殖链被切断。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

/// 任务条数上限（与内核每插件 ≤20 的硬上限一致；UI 侧提前给出人话错误）。
const MAX_JOBS: usize = 20;
const MAX_NAME: usize = 64;
const MAX_KEY: usize = 64;
const MAX_METHOD: usize = 64;
const MAX_PARAMS_BYTES: usize = 4096;
/// 本插件自己的 id：任务来源 / 表单默认值都记在这里。
const SELF_ID: &str = "com.skiff.cron";

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    if let Err(e) = plugin.serve(move |method, params| match method {
        // 旧插件 RPC 名称保留为薄别名：面板 UI（app.js）与旧调用方都能平滑切换。
        "jobs.list" | "list" | "jobs" => list(&p, &params),
        "jobs.create" | "create" | "jobs.update" | "update" => create(&p, &params),
        "jobs.toggle" | "toggle" => toggle(&p, &params),
        "jobs.remove" | "remove" => remove(&p, &params),
        "jobs.run" | "run" => run_now(&p, &params),
        "jobs.reschedule" | "reschedule" => reschedule(&p, &params),
        "targets" => targets(&p),
        "schedule.describe" | "schedule" => schedule_describe(&params),
        "schedule.build" => schedule_build(&params),
        "config" => Ok(config(&p)),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

fn config(plugin: &Plugin) -> Value {
    let map = common::load_config(plugin)
        .into_iter()
        .map(|(k, v)| (k, Value::from(v)))
        .collect::<serde_json::Map<String, Value>>();
    let mut out = Value::Object(map);
    out["jobs"] = json!(count_tasks(plugin).unwrap_or(0));
    out["now"] = json!(common::now_secs());
    out
}

fn count_tasks(plugin: &Plugin) -> Result<usize, String> {
    let body = plugin
        .scheduler_call("task.list", json!({}))
        .map_err(|e| e.to_string())?;
    Ok(body
        .get("tasks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0))
}

// ---- 校验 ---------------------------------------------------------------

/// 插件 id / 任务 key：只允许 `[A-Za-z0-9._-]`，禁止空串与超长。
fn valid_token(s: &str, max: usize) -> bool {
    let n = s.trim();
    !n.is_empty()
        && n.len() <= max
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_method(s: &str) -> bool {
    valid_token(s, MAX_METHOD)
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

/// 只有「缺少授权」这类权限不足才补「该去勾哪个授权」；其余 403（内置任务保护、
/// 执行中互斥等）保留内核原文，避免把非权限问题误报成授权缺失。
fn call_err(target: &str, e: &str) -> String {
    if e.contains("缺少") {
        format!("{e}（请确认计划任务持有 scheduler 权限，且安装时勾选了 plugin:{target}）")
    } else if e.contains("404") || e.contains("不存在") {
        format!("{e}（目标插件 {target} 可能未安装或未启用）")
    } else {
        truncate(e)
    }
}

// ---- 调度预设 ---------------------------------------------------------

/// 预设 → 前端 JSON。内核仍只存表达式，这些字段只用于表单回显。
fn preset_to_json(preset: &skiff_cron::Preset) -> Value {
    use skiff_cron::Preset;
    match preset {
        Preset::Minutes { every } => json!({ "kind": "minutes", "every": every }),
        Preset::Hours { every } => json!({ "kind": "hours", "every": every }),
        Preset::Daily { hour, minute } => {
            json!({ "kind": "daily", "hour": hour, "minute": minute })
        }
        Preset::Weekly { days, hour, minute } => {
            json!({ "kind": "weekly", "days": days, "hour": hour, "minute": minute })
        }
        Preset::Monthly { day, hour, minute } => {
            json!({ "kind": "monthly", "day": day, "hour": hour, "minute": minute })
        }
        Preset::Weekdays { hour, minute } => {
            json!({ "kind": "weekdays", "hour": hour, "minute": minute })
        }
    }
}

fn preset_from_json(value: &Value) -> Result<skiff_cron::Preset, String> {
    use skiff_cron::Preset;
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "preset 缺少 kind".to_string())?;
    let num = |name: &str| -> Result<u32, String> {
        value
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("preset.{name} 必须是非负整数"))
    };
    match kind {
        "minutes" => Ok(Preset::Minutes {
            every: num("every")?,
        }),
        "hours" => Ok(Preset::Hours {
            every: num("every")?,
        }),
        "daily" => Ok(Preset::Daily {
            hour: num("hour")?,
            minute: num("minute")?,
        }),
        "weekly" => {
            let days = value
                .get("days")
                .and_then(Value::as_array)
                .ok_or_else(|| "preset.days 必须是数组".to_string())?;
            let mut out = Vec::with_capacity(days.len());
            for day in days {
                let n = day
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| "preset.days 只能包含 0–6".to_string())?;
                out.push(n);
            }
            Ok(Preset::Weekly {
                days: out,
                hour: num("hour")?,
                minute: num("minute")?,
            })
        }
        "monthly" => Ok(Preset::Monthly {
            day: num("day")?,
            hour: num("hour")?,
            minute: num("minute")?,
        }),
        "weekdays" => Ok(Preset::Weekdays {
            hour: num("hour")?,
            minute: num("minute")?,
        }),
        other => Err(format!("未知的调度预设：{other}")),
    }
}

/// 表单来源二选一：给了 `preset` 就由本插件翻译成表达式；否则用 `expr`。
///
/// 两条路径都返回可直接落库的标准 5 段表达式；非法时返回人话错误，
/// 内核侧仍会再做一次校验（这里只是把错误提前到界面上）。
fn expr_from_params(params: &Value, default: Option<&str>) -> Result<String, String> {
    if let Some(value) = params.get("preset").filter(|v| !v.is_null()) {
        return preset_from_json(value)?.to_expr();
    }
    let raw = common::param_str(params, "expr");
    let raw = raw.trim();
    let expr = if raw.is_empty() {
        match default {
            Some(value) => value.trim().to_string(),
            None => return Err("缺少 preset 或 expr".to_string()),
        }
    } else {
        raw.to_string()
    };
    if !skiff_cron::is_valid(&expr) {
        return Err(format!(
            "cron 表达式无法解析：{expr}（需要 5 段：分 时 日 月 周；不支持秒级与 @reboot）"
        ));
    }
    Ok(expr)
}

/// 反解：表达式 → 预设 + 人话预览。无法匹配预设时 `preset` 为 `null`，
/// `expr` 原样返回，供界面落到「自定义」而不是丢掉用户数据。
fn schedule_describe(params: &Value) -> Result<Value, RpcError> {
    let expr = common::param_str(params, "expr");
    let expr = expr.trim();
    if !skiff_cron::is_valid(expr) {
        return Ok(json!({
            "ok": true,
            "valid": false,
            "expr": expr,
            "preset": Value::Null,
            "human": "",
            "error": format!(
                "cron 表达式无法解析：{expr}（需要 5 段：分 时 日 月 周；不支持秒级与 @reboot）"
            ),
        }));
    }
    let preset = skiff_cron::detect_preset(expr);
    let human = preset.as_ref().map(|p| p.describe()).unwrap_or_default();
    Ok(json!({
        "ok": true,
        "valid": true,
        "expr": expr,
        "preset": preset.as_ref().map(preset_to_json).unwrap_or(Value::Null),
        "human": human,
    }))
}

/// 正向：预设 → 表达式 + 人话预览。范围错误以 `{ok:false,error}` 返回，界面就地提示。
fn schedule_build(params: &Value) -> Result<Value, RpcError> {
    let value = params
        .get("preset")
        .ok_or_else(|| RpcError::invalid_params("缺少 preset".to_string()))?;
    match preset_from_json(value).and_then(|preset| preset.to_expr().map(|expr| (preset, expr))) {
        Ok((preset, expr)) => Ok(json!({
            "ok": true,
            "preset": preset_to_json(&preset),
            "expr": expr,
            "human": preset.describe(),
        })),
        Err(error) => Ok(json!({ "ok": false, "error": error })),
    }
}

// ---- 视图映射 -----------------------------------------------------------

/// 内核任务视图 → 插件 UI 沿用的 `job` 形状。
///
/// 纯函数，便于单测：不改写内核数据，只做字段搬运与展示值补全。
fn task_to_job(task: &Value) -> Value {
    let id = task["id"].as_str().unwrap_or("");
    let source = task["source"].as_str().unwrap_or("").to_string();
    let expr = task["expr"].as_str().unwrap_or("");
    // 展示层翻译：能对应到预设就给人话，匹配不到就留空由界面显示「自定义」。
    let expr_human = skiff_cron::describe_expr(expr).unwrap_or_default();
    // 本插件创建的任务在面板语境下就是「本面板」。
    let source_label = if source == SELF_ID {
        "ui".to_string()
    } else if source.is_empty() {
        "system".to_string()
    } else {
        source
    };
    json!({
        "id": id,
        "name": task["name"].as_str().unwrap_or(id),
        "expr": expr,
        "expr_human": expr_human,
        "enabled": task["enabled"].as_bool().unwrap_or(false),
        "state": task["state"].as_str().unwrap_or("idle"),
        "lifecycle": task["lifecycle"].as_str().unwrap_or("enabled"),
        "blocked_reason": task["blocked_reason"].clone(),
        "target": task["target"].as_str().unwrap_or(""),
        "method": task["method"].as_str().unwrap_or(""),
        "key": task["key"].as_str().unwrap_or(""),
        "source": source_label,
        "payload": task["payload"].clone(),
        "last": task["last"].clone(),
        "next_run": task["next_run"].clone(),
    })
}

fn jobs_of(body: &Value) -> Vec<Value> {
    body.get("tasks")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(task_to_job).collect())
        .unwrap_or_default()
}

// ---- RPC -------------------------------------------------------------

/// 列表：`source` / `target` 可选过滤，返回带 `next_run` 的任务视图。
fn list(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let source = common::param_str(params, "source");
    let target = common::param_str(params, "target");
    let body = plugin
        .scheduler_call("task.list", json!({}))
        .map_err(|e| RpcError::internal(call_err("调度器", &e.to_string())))?;
    let jobs: Vec<Value> = jobs_of(&body)
        .into_iter()
        .filter(|j| {
            let src = j["source"].as_str().unwrap_or("");
            // 面板 UI 的「本面板」对应内核里 cron 自己创建的任务，也接受内核的 ui 标记。
            source.is_empty()
                || src == source
                || (source == "ui" && (src == "ui" || src == "system"))
        })
        .filter(|j| target.is_empty() || j["target"].as_str() == Some(target.as_str()))
        .collect();
    Ok(json!({ "ok": true, "jobs": jobs, "now": common::now_secs() }))
}

fn create(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let key_in = common::param_str(params, "key");
    if !key_in.is_empty() && !valid_token(&key_in, MAX_KEY) {
        return Err(RpcError::invalid_params(format!(
            "key 非法: {key_in:?}（只允许字母/数字/`.`/`-`/`_`，最长 {MAX_KEY}）"
        )));
    }
    // 没填 key 时按秒生成：面板「新建任务」是显式动作，不需要幂等。
    let key = if key_in.is_empty() {
        format!("job{}", common::now_secs())
    } else {
        key_in
    };

    let target = common::param_str(params, "target").trim().to_string();
    let method = common::param_str(params, "method").trim().to_string();
    if !valid_token(&target, 128) {
        return Err(RpcError::invalid_params(format!(
            "target 插件 id 非法: {target:?}"
        )));
    }
    if target == SELF_ID {
        return Err(RpcError::invalid_params(
            "计划任务不能调度自己（会把调度器拖进递归）".to_string(),
        ));
    }
    if !valid_method(&method) {
        return Err(RpcError::invalid_params(format!(
            "method 非法: {method:?}（只允许字母/数字/`.`/`_`/`-`，最长 {MAX_METHOD}）"
        )));
    }

    let cfg = common::load_config(plugin);
    let default_expr = common::cfg_str(&cfg, "default_expr", "0 3 * * *");
    let expr =
        expr_from_params(params, Some(default_expr.as_str())).map_err(RpcError::invalid_params)?;

    let name = {
        let raw = common::param_str(params, "name");
        let raw = raw.trim();
        if raw.is_empty() {
            format!("{target} {method}")
        } else {
            raw.to_string()
        }
    };
    if name.chars().count() > MAX_NAME {
        return Err(RpcError::invalid_params(format!(
            "任务名过长（上限 {MAX_NAME} 字符）"
        )));
    }

    let inner = params.get("params").cloned().unwrap_or_else(|| json!({}));
    if !inner.is_object() {
        return Err(RpcError::invalid_params(
            "params 必须是 JSON 对象".to_string(),
        ));
    }
    if serde_json::to_vec(&inner)
        .map(|b| b.len())
        .unwrap_or(usize::MAX)
        > MAX_PARAMS_BYTES
    {
        return Err(RpcError::invalid_params(format!(
            "params 超过 {MAX_PARAMS_BYTES} 字节"
        )));
    }

    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let total = count_tasks(plugin).unwrap_or(0);
    if total >= MAX_JOBS && common::param_str(params, "id").is_empty() {
        return Err(RpcError::internal(format!(
            "任务数已达上限 {MAX_JOBS}，请先删除不用的任务"
        )));
    }

    let payload = json!({ "name": name, "params": inner });
    let body = plugin
        .scheduler_call(
            "task.register",
            json!({
                "local_key": key,
                "target": target,
                "method": method,
                "expr": expr,
                "payload": payload,
                "enabled": enabled,
            }),
        )
        .map_err(|e| RpcError::internal(call_err(&target, &e.to_string())))?;
    let created = body["created"].as_bool().unwrap_or(false);
    let job = body.get("task").map(task_to_job).unwrap_or(Value::Null);
    Ok(json!({
        "ok": true,
        "created": created,
        "id": body["id"].clone(),
        "next_run": job["next_run"].clone(),
        "job": job,
    }))
}

/// 定位任务并返回内核任务 id（面板 UI 只按 id 操作，不做来源推断）。
fn require_id(params: &Value) -> Result<String, RpcError> {
    let id = common::param_str(params, "id");
    let id = id.trim();
    if id.is_empty() {
        Err(RpcError::invalid_params("需要 id 定位任务".to_string()))
    } else {
        Ok(id.to_string())
    }
}

fn toggle(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = require_id(params)?;
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params("需要 enabled（true/false）".to_string()))?;
    let body = plugin
        .scheduler_call(
            "task.update",
            json!({ "id": id, "patch": { "enabled": enabled } }),
        )
        .map_err(|e| RpcError::internal(call_err(&id, &e.to_string())))?;
    Ok(json!({ "ok": true, "id": id, "enabled": enabled, "task": body.get("task").cloned() }))
}

fn remove(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = require_id(params)?;
    plugin
        .scheduler_call("task.delete", json!({ "id": id }))
        .map_err(|e| RpcError::internal(call_err(&id, &e.to_string())))?;
    Ok(json!({ "ok": true, "id": id, "removed": true }))
}

/// 只改调度（表达式），不动任务身份 / 目标 / 参数。
///
/// 内核存的标准表达式是唯一事实：这里把预设翻译好后走 `task.update`，
/// 与新建共用同一套 `expr_from_params`，不产生第二份调度语义。
fn reschedule(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = require_id(params)?;
    let expr = expr_from_params(params, None).map_err(RpcError::invalid_params)?;
    let body = plugin
        .scheduler_call(
            "task.update",
            json!({
                "id": id,
                "patch": { "schedule": { "type": "cron", "expr": expr } },
            }),
        )
        .map_err(|e| RpcError::internal(call_err(&id, &e.to_string())))?;
    Ok(json!({
        "ok": true,
        "id": id,
        "expr": expr,
        "task": body.get("task").cloned().unwrap_or(Value::Null),
    }))
}

fn run_now(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = require_id(params)?;
    let body = plugin
        .scheduler_call("task.run", json!({ "id": id }))
        .map_err(|e| RpcError::internal(call_err(&id, &e.to_string())))?;
    let run = body.get("run").cloned().unwrap_or(Value::Null);
    let result = run["result"].as_str().unwrap_or("");
    if result != "ok" {
        return Err(RpcError::internal(format!(
            "执行失败：{}",
            run["error_detail"].as_str().unwrap_or(result)
        )));
    }
    Ok(json!({ "ok": true, "id": id, "run": run }))
}

/// 可调度目标：直接读内核的 interop 目录，保证 UI 里选得到的都真调得通。
fn targets(plugin: &Plugin) -> Result<Value, RpcError> {
    let body = plugin
        .interop_catalog()
        .map_err(|e| RpcError::internal(call_err("目标插件", &e.to_string())))?;
    Ok(json!({
        "ok": true,
        "interfaces": body.get("interfaces").cloned().unwrap_or(json!([])),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_reject_paths_and_empty_values() {
        assert!(valid_token("com.skiff.nginx", 128));
        assert!(valid_token("site-a.v1", MAX_KEY));
        assert!(!valid_token("", MAX_KEY));
        assert!(!valid_token("../evil", MAX_KEY));
        assert!(!valid_token("a b", MAX_KEY));
        assert!(!valid_token(&"x".repeat(MAX_KEY + 1), MAX_KEY));
        assert!(valid_method("backup"));
        assert!(valid_method("jobs.create"));
        assert!(!valid_method(""));
    }

    #[test]
    fn task_view_maps_to_legacy_job_shape_without_mutating() {
        let task = json!({
            "id": "plugin:com.skiff.cron:nightly",
            "name": "夜间备份",
            "expr": "0 3 * * *",
            "enabled": true,
            "state": "idle",
            "lifecycle": "enabled",
            "source": SELF_ID,
            "target": "com.skiff.nginx",
            "method": "backup",
            "key": "nightly",
            "payload": {"name": "夜间备份"},
            "last": {"at": 1700000000, "result": "ok", "detail": ""},
            "next_run": {"at": 1700003600, "text": "2023-11-15 03:00:00"},
        });
        let job = task_to_job(&task);
        assert_eq!(job["id"], "plugin:com.skiff.cron:nightly");
        assert_eq!(job["source"], "ui");
        assert_eq!(job["expr"], "0 3 * * *");
        assert_eq!(job["expr_human"], "每天 03:00 执行");
        assert_eq!(job["next_run"]["text"], "2023-11-15 03:00:00");
        assert_eq!(job["last"]["result"], "ok");
        // 纯映射：输入不被改写
        assert!(task.get("next_run").is_some());
        assert!(job.get("tick").is_none());
    }

    #[test]
    fn list_filters_keep_ui_and_system_tasks() {
        let body = json!({"tasks": [
            {"id": "a", "source": "ui", "target": "x", "expr": "0 3 * * *"},
            {"id": "b", "source": "system", "target": "x", "expr": "0 4 * * 0"},
            {"id": "c", "source": "com.skiff.guard", "target": "y", "expr": "* * * * *"},
        ]});
        assert_eq!(jobs_of(&body).len(), 3);
        // source=ui → ui + system；source=system 只留 system
        let only = |s: &str| {
            jobs_of(&body)
                .into_iter()
                .filter(|j| {
                    let src = j["source"].as_str().unwrap_or("");
                    src == s
                })
                .count()
        };
        assert_eq!(only("system"), 1);
        assert_eq!(only("com.skiff.guard"), 1);
    }

    /// 403 不都等于权限不足：内置任务保护保留内核原文，只有「缺少」才补授权指引。
    #[test]
    fn call_err_only_hints_when_permission_is_missing() {
        let builtin = "协议错误: 内核返回 HTTP 403: 内置任务不可删除（只能停用）";
        assert_eq!(
            call_err("builtin:db-vacuum", builtin),
            builtin,
            "内置任务保护不应套授权提示"
        );
        let busy = "协议错误: 内核返回 HTTP 403: 插件正在执行任务，不能再调用 scheduler";
        assert_eq!(call_err("调度器", busy), busy, "执行中互斥不应套授权提示");
        let denied = "协议错误: 内核返回 HTTP 403: 缺少 scheduler:manage";
        assert!(
            call_err("调度器", denied).contains("请确认计划任务持有 scheduler 权限"),
            "真正的权限不足应补授权指引"
        );
    }

    #[test]
    fn require_id_rejects_blank_and_trims() {
        assert!(require_id(&json!({})).is_err());
        assert!(require_id(&json!({"id": "   "})).is_err());
        assert_eq!(require_id(&json!({"id": " j1 "})).unwrap(), "j1");
    }

    #[test]
    fn preset_json_round_trips_every_kind() {
        use skiff_cron::Preset;
        let presets = [
            Preset::Minutes { every: 5 },
            Preset::Hours { every: 2 },
            Preset::Daily { hour: 3, minute: 0 },
            Preset::Weekly {
                days: vec![1, 3, 5],
                hour: 9,
                minute: 30,
            },
            Preset::Monthly {
                day: 1,
                hour: 4,
                minute: 0,
            },
            Preset::Weekdays { hour: 9, minute: 0 },
        ];
        for preset in presets {
            let value = preset_to_json(&preset);
            assert_eq!(preset_from_json(&value).unwrap(), preset, "{value}");
        }
    }

    #[test]
    fn build_and_describe_share_one_translation() {
        let built = schedule_build(&json!({
            "preset": {"kind": "daily", "hour": 3, "minute": 0}
        }))
        .unwrap();
        assert_eq!(built["ok"], true);
        assert_eq!(built["expr"], "0 3 * * *");
        assert_eq!(built["human"], "每天 03:00 执行");

        let described = schedule_describe(&json!({"expr": "0 3 * * *"})).unwrap();
        assert_eq!(described["valid"], true);
        assert_eq!(described["expr"], "0 3 * * *");
        assert_eq!(described["preset"]["kind"], "daily");
        assert_eq!(described["human"], "每天 03:00 执行");
    }

    #[test]
    fn unmatched_expression_falls_back_to_custom_without_losing_text() {
        let described = schedule_describe(&json!({"expr": "0,30 9 * * *"})).unwrap();
        assert_eq!(described["valid"], true);
        assert!(described["preset"].is_null(), "不该硬套预设");
        assert_eq!(described["expr"], "0,30 9 * * *");
        assert_eq!(described["human"], "");
    }

    #[test]
    fn invalid_expression_is_reported_clearly() {
        for bad in ["* * * *", "60 * * * *", "@reboot", "0 0 0 * *"] {
            let described = schedule_describe(&json!({ "expr": bad })).unwrap();
            assert_eq!(described["valid"], false, "{bad:?}");
            assert_eq!(described["expr"], bad, "原文不能丢");
            assert!(
                described["error"].as_str().unwrap_or("").contains("5 段"),
                "{bad:?} 应给出明确报错"
            );
        }
    }

    #[test]
    fn expr_from_params_prefers_preset_and_validates_custom() {
        let from_preset = expr_from_params(
            &json!({"preset": {"kind": "weekdays", "hour": 9, "minute": 0}}),
            Some("0 3 * * *"),
        )
        .unwrap();
        assert_eq!(from_preset, "0 9 * * 1-5");

        let from_custom = expr_from_params(&json!({"expr": "15 2 * * 6"}), None).unwrap();
        assert_eq!(from_custom, "15 2 * * 6");

        let defaulted = expr_from_params(&json!({}), Some("0 3 * * *")).unwrap();
        assert_eq!(defaulted, "0 3 * * *");

        let bad = expr_from_params(&json!({"expr": "0 25 * * *"}), None).unwrap_err();
        assert!(bad.contains("5 段"), "非法表达式要给人话提示：{bad}");

        let missing = expr_from_params(&json!({}), None).unwrap_err();
        assert!(missing.contains("缺少 preset 或 expr"));
    }

    #[test]
    fn out_of_range_preset_returns_inline_error() {
        let built = schedule_build(&json!({
            "preset": {"kind": "minutes", "every": 60}
        }))
        .unwrap();
        assert_eq!(built["ok"], false);
        assert!(
            built["error"].as_str().unwrap_or("").contains("1–59"),
            "越界要给出范围：{built}"
        );
    }
}
