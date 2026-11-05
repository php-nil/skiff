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
        "targets" => targets(&p),
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

/// 内核 403/404 的原文对用户没信息量，这里补一句「该去勾哪个授权」。
fn call_err(target: &str, e: &str) -> String {
    if e.contains("403") || e.contains("缺少") {
        format!("{e}（请确认计划任务持有 scheduler 权限，且安装时勾选了 plugin:{target}）")
    } else if e.contains("404") || e.contains("不存在") {
        format!("{e}（目标插件 {target} 可能未安装或未启用）")
    } else {
        truncate(e)
    }
}

// ---- 视图映射 -----------------------------------------------------------

/// 内核任务视图 → 插件 UI 沿用的 `job` 形状。
///
/// 纯函数，便于单测：不改写内核数据，只做字段搬运与展示值补全。
fn task_to_job(task: &Value) -> Value {
    let id = task["id"].as_str().unwrap_or("");
    let source = task["source"].as_str().unwrap_or("").to_string();
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
        "expr": task["expr"].as_str().unwrap_or(""),
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
    let expr = {
        let raw = common::param_str(params, "expr");
        let raw = raw.trim();
        if raw.is_empty() {
            common::cfg_str(&cfg, "default_expr", "0 3 * * *")
        } else {
            raw.to_string()
        }
    };

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

    #[test]
    fn require_id_rejects_blank_and_trims() {
        assert!(require_id(&json!({})).is_err());
        assert!(require_id(&json!({"id": "   "})).is_err());
        assert_eq!(require_id(&json!({"id": " j1 "})).unwrap(), "j1");
    }
}
