//! `com.skiff.cron`：计划任务（只调度插件接口，不执行宿主 shell）。
//!
//! 内核每分钟调一次 `tick`；本插件按任务表达式逐条匹配，再通过内核对目标插件发起
//! `interop:call`。因此：
//! - 任务只能调用**已声明 provides** 的插件方法，不能跑任意命令；
//! - 能不能调通由内核四道闸判定（`interop:call` + `plugin:` 授权 + consumes/provides），
//!   本插件侧的校验只负责给出人话错误；
//! - 同一 `(source, key)` 再写一次是更新而非新增：其他插件重复触发不会堆任务，
//!   `source` 由内核注入的 `_interop.caller` 决定，调用方无法伪造。
//!
//! 任务身本身（表达式、目标、参数、上次结果）都落在插件自己的 `data/jobs.json`，
//! 卸载时随「删除数据」一起清掉。

use chrono::Local;
use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const JOBS_FILE: &str = "jobs.json";
/// 任务条数上限：再多也没人看，且每次 tick 都要逐个匹配表达式。
const MAX_JOBS: usize = 200;
const MAX_NAME: usize = 64;
const MAX_KEY: usize = 128;
const MAX_METHOD: usize = 64;
const MAX_PARAMS_BYTES: usize = 4096;
const MAX_DETAIL: usize = 200;
/// 面板 UI 直接调用的任务来源；插件间调用会被内核 `_interop.caller` 覆盖。
const SOURCE_UI: &str = "ui";
/// 本插件自己的 id：不允许出现自调任务（内核也会拒）。
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
        "tick" => tick(&p, &params),
        "list" | "jobs" => list(&p, &params),
        "jobs.create" | "create" => create(&p, &params),
        "jobs.update" | "update" => create(&p, &params),
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
    out["jobs"] = json!(list_jobs(plugin).len());
    out["now"] = json!(common::now_secs());
    out
}

// ---- 纯校验 ---------------------------------------------------------------

fn valid_expr(expr: &str) -> bool {
    skiff_cron::is_valid(expr)
}

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

/// 调用来源：内核注入的 `_interop.caller` 优先（覆盖调用方自带的同名键）。
fn source_of(params: &Value) -> String {
    params
        .get("_interop")
        .and_then(|v| v.get("caller"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(SOURCE_UI)
        .to_string()
}

fn jobs_of(st: &Value) -> Vec<Value> {
    st.get("jobs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn list_jobs(plugin: &Plugin) -> Vec<Value> {
    jobs_of(&common::read_state(plugin, JOBS_FILE))
}

/// 优先按 `id`，其次按 `(source, key)` 命中；两者都没有就是新增。
fn find_index(jobs: &[Value], source: &str, key: &str, id: &str) -> Option<usize> {
    if !id.is_empty() {
        if let Some(i) = jobs.iter().position(|j| j["id"].as_str() == Some(id)) {
            return Some(i);
        }
    }
    if key.is_empty() {
        return None;
    }
    jobs.iter().position(|j| {
        j["source"].as_str().unwrap_or("") == source && j["key"].as_str().unwrap_or("") == key
    })
}

/// 任务 id：秒级时间戳 + 序号；同一个秒内创建多条也能保证唯一。
fn make_id(now: u64, jobs: &[Value]) -> String {
    let mut n = 0u32;
    loop {
        let id = format!("j{now}{n:02}");
        if !jobs.iter().any(|j| j["id"].as_str() == Some(id.as_str())) {
            return id;
        }
        n += 1;
    }
}

/// 下一次触发时间（本地时区）。表达式非法时返回 `null`。
fn next_run(expr: &str, from: chrono::NaiveDateTime) -> Option<chrono::NaiveDateTime> {
    skiff_cron::CronExpr::parse(expr)?.next_after(from)
}

fn next_run_json(expr: &str) -> Value {
    match next_run(expr, Local::now().naive_local()) {
        Some(t) => json!({
            "at": t.and_utc().timestamp(),
            "text": t.format("%Y-%m-%d %H:%M").to_string(),
        }),
        None => Value::Null,
    }
}

/// 任务对外视图：补上 `next_run`（UI 直接显示，不用自己算表达式）。
fn job_view(job: &Value) -> Value {
    let mut v = job.clone();
    v["next_run"] = next_run_json(job["expr"].as_str().unwrap_or(""));
    v
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_DETAIL).collect()
}

/// 内核 403/404 的原文对用户没信息量，这里补一句「该去勾哪个授权」。
fn call_err(target: &str, e: &str) -> String {
    if e.contains("403") {
        format!("{e}（请在安装计划任务时勾选 plugin:{target}，并保留 interop:call）")
    } else if e.contains("404") {
        format!("{e}（目标插件 {target} 可能未安装或未启用）")
    } else {
        truncate(e)
    }
}

fn execute(plugin: &Plugin, job: &Value) -> Result<Value, String> {
    let target = job["target"].as_str().unwrap_or("");
    let method = job["method"].as_str().unwrap_or("");
    if target.is_empty() || method.is_empty() {
        return Err("任务缺少 target/method".into());
    }
    let params = job.get("params").cloned().unwrap_or_else(|| json!({}));
    plugin
        .interop_call(target, method, params)
        .map_err(|e| call_err(target, &e.to_string()))
}

// ---- RPC -----------------------------------------------------------------

/// 内核每分钟调一次；这里按分钟去重，单次最多执行 `tick_batch` 条。
fn tick(plugin: &Plugin, _params: &Value) -> Result<Value, RpcError> {
    let cfg = common::load_config(plugin);
    let batch = common::cfg_u64(&cfg, "tick_batch", 5).clamp(1, 50) as usize;
    let now = Local::now().naive_local();
    let minute = now.and_utc().timestamp() / 60;
    let stamp = common::now_secs();

    let mut st = common::read_state(plugin, JOBS_FILE);
    if !st.is_object() {
        st = json!({});
    }
    if !st.get("jobs").map(Value::is_array).unwrap_or(false) {
        st["jobs"] = json!([]);
    }
    let jobs = jobs_of(&st);

    let mut due: Vec<usize> = jobs
        .iter()
        .enumerate()
        .filter(|(_, j)| j["enabled"].as_bool().unwrap_or(false))
        // 同一分钟内已经跑过的跳过（内核通常已去重，这里再兜一层）。
        .filter(|(_, j)| j["last"]["minute"].as_i64() != Some(minute))
        .filter(|(_, j)| valid_expr(j["expr"].as_str().unwrap_or("")))
        .filter(|(_, j)| skiff_cron::expr_matches(j["expr"].as_str().unwrap_or(""), &now))
        .map(|(i, _)| i)
        .collect();
    let matched = due.len();
    due.truncate(batch);

    let mut ran = Vec::new();
    for idx in due {
        let job = st["jobs"][idx].clone();
        let id = job["id"].as_str().unwrap_or("").to_string();
        let target = job["target"].as_str().unwrap_or("").to_string();
        let method = job["method"].as_str().unwrap_or("").to_string();
        let outcome = execute(plugin, &job);
        let (result, detail) = match &outcome {
            Ok(_) => ("ok", String::new()),
            Err(e) => ("fail", truncate(e)),
        };
        st["jobs"][idx]["last"] = json!({
            "at": stamp,
            "minute": minute,
            "result": result,
            "detail": detail,
        });
        ran.push(json!({
            "id": id,
            "target": target,
            "method": method,
            "result": result,
            "detail": detail,
        }));
    }

    common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
    Ok(json!({
        "ok": true,
        "minute": minute,
        "matched": matched,
        "batch": batch,
        "ran": ran,
    }))
}

/// 列表：`source` / `target` 可选过滤，返回带 `next_run` 的任务视图。
fn list(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let source = common::param_str(params, "source");
    let target = common::param_str(params, "target");
    let jobs: Vec<Value> = list_jobs(plugin)
        .iter()
        .filter(|j| source.is_empty() || j["source"].as_str() == Some(source.as_str()))
        .filter(|j| target.is_empty() || j["target"].as_str() == Some(target.as_str()))
        .map(job_view)
        .collect();
    Ok(json!({ "ok": true, "jobs": jobs, "now": common::now_secs() }))
}

fn create(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let source = source_of(params);
    let id = common::param_str(params, "id");
    let key = common::param_str(params, "key");
    if !key.is_empty() && !valid_token(&key, MAX_KEY) {
        return Err(RpcError::invalid_params(format!(
            "key 非法: {key:?}（只允许字母/数字/`.`/`-`/`_`，最长 {MAX_KEY}）"
        )));
    }

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
    if !valid_expr(&expr) {
        return Err(RpcError::invalid_params(format!(
            "cron 表达式非法: {expr:?}（需要 5 段：分 时 日 月 周）"
        )));
    }

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

    let mut st = common::read_state(plugin, JOBS_FILE);
    if !st.is_object() {
        st = json!({});
    }
    if !st.get("jobs").map(Value::is_array).unwrap_or(false) {
        st["jobs"] = json!([]);
    }
    let jobs = jobs_of(&st);
    let now = common::now_secs();
    let found = find_index(&jobs, &source, &key, &id);

    // 已有任务：只有自己的来源能改（插件不能借 id 覆盖别人的任务）。
    if let Some(idx) = found {
        if jobs[idx]["source"].as_str().unwrap_or("") != source {
            return Err(RpcError::invalid_params(format!(
                "任务 {} 属于来源 {}，当前调用方 {source} 无权修改",
                jobs[idx]["id"].as_str().unwrap_or(""),
                jobs[idx]["source"].as_str().unwrap_or(""),
            )));
        }
        let job_id = jobs[idx]["id"].as_str().unwrap_or("").to_string();
        st["jobs"][idx]["name"] = json!(name);
        st["jobs"][idx]["expr"] = json!(expr);
        st["jobs"][idx]["target"] = json!(target);
        st["jobs"][idx]["method"] = json!(method);
        st["jobs"][idx]["params"] = inner;
        st["jobs"][idx]["enabled"] = json!(enabled);
        st["jobs"][idx]["updated_at"] = json!(now);
        common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
        let job = job_view(&st["jobs"][idx]);
        return Ok(json!({
            "ok": true,
            "created": false,
            "id": job_id,
            "next_run": job["next_run"].clone(),
            "job": job,
        }));
    }

    if jobs.len() >= MAX_JOBS {
        return Err(RpcError::internal(format!(
            "任务数已达上限 {MAX_JOBS}，请先删除不用的任务"
        )));
    }
    let new_id = make_id(now, &jobs);
    let job = json!({
        "id": new_id,
        "name": name,
        "expr": expr,
        "target": target,
        "method": method,
        "params": inner,
        "enabled": enabled,
        "key": key,
        "source": source,
        "created_at": now,
        "updated_at": now,
        "last": Value::Null,
    });
    st["jobs"].as_array_mut().expect("jobs 恒为数组").push(job);
    common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
    let created = st["jobs"]
        .as_array()
        .and_then(|a| a.last())
        .map(job_view)
        .unwrap_or(Value::Null);
    Ok(json!({
        "ok": true,
        "created": true,
        "id": new_id,
        "next_run": created["next_run"].clone(),
        "job": created,
    }))
}

/// 按 `id`（或 `source + key`）定位调用方自己的任务。
fn own_job(plugin: &Plugin, params: &Value) -> Result<(Value, usize, String), RpcError> {
    let source = source_of(params);
    let id = common::param_str(params, "id");
    let key = common::param_str(params, "key");
    if id.is_empty() && key.is_empty() {
        return Err(RpcError::invalid_params(
            "需要 id（或 key + source）定位任务".to_string(),
        ));
    }
    let st = common::read_state(plugin, JOBS_FILE);
    let jobs = jobs_of(&st);
    let idx = find_index(&jobs, &source, &key, &id).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "找不到任务（id={id:?} key={key:?} source={source}）"
        ))
    })?;
    if jobs[idx]["source"].as_str().unwrap_or("") != source {
        return Err(RpcError::invalid_params(format!(
            "任务 {} 属于来源 {}，当前调用方 {source} 无权操作",
            jobs[idx]["id"].as_str().unwrap_or(""),
            jobs[idx]["source"].as_str().unwrap_or(""),
        )));
    }
    let id = jobs[idx]["id"].as_str().unwrap_or("").to_string();
    Ok((st, idx, id))
}

fn toggle(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (mut st, idx, id) = own_job(plugin, params)?;
    let enabled = params
        .get("enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params("需要 enabled（true/false）".to_string()))?;
    st["jobs"][idx]["enabled"] = json!(enabled);
    st["jobs"][idx]["updated_at"] = json!(common::now_secs());
    common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
    Ok(json!({ "ok": true, "id": id, "enabled": enabled }))
}

fn remove(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (mut st, idx, id) = own_job(plugin, params)?;
    if let Some(arr) = st["jobs"].as_array_mut() {
        arr.remove(idx);
    }
    common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
    Ok(json!({ "ok": true, "id": id, "removed": true }))
}

fn run_now(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (mut st, idx, id) = own_job(plugin, params)?;
    let job = st["jobs"][idx].clone();
    let stamp = common::now_secs();
    let outcome = execute(plugin, &job);
    let (result, detail, payload) = match outcome {
        Ok(v) => ("ok", String::new(), v),
        Err(e) => ("fail", truncate(&e), Value::Null),
    };
    st["jobs"][idx]["last"] = json!({
        "at": stamp,
        "minute": Local::now().naive_local().and_utc().timestamp() / 60,
        "result": result,
        "detail": detail,
        "manual": true,
    });
    common::write_state(plugin, JOBS_FILE, &st).map_err(common::rpc_err)?;
    if result == "fail" {
        return Err(RpcError::internal(detail));
    }
    Ok(json!({ "ok": true, "id": id, "result": payload }))
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
    fn expressions_are_validated_before_saving() {
        assert!(valid_expr("0 3 * * *"));
        assert!(valid_expr("*/5 * * * *"));
        assert!(!valid_expr(""));
        assert!(!valid_expr("0 3 * *"));
        assert!(!valid_expr("61 * * * *"));
    }

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
    fn source_prefers_injected_caller() {
        assert_eq!(source_of(&json!({})), SOURCE_UI);
        assert_eq!(
            source_of(&json!({"source": "spoofed", "_interop": {"caller": "com.skiff.nginx"}})),
            "com.skiff.nginx"
        );
        // 空的 caller 不能盖掉默认值
        assert_eq!(source_of(&json!({"_interop": {"caller": "  "}})), SOURCE_UI);
    }

    #[test]
    fn upsert_matches_id_then_source_and_key() {
        let jobs = vec![
            json!({"id": "j1", "source": "com.skiff.nginx", "key": "nightly"}),
            json!({"id": "j2", "source": "ui", "key": "manual"}),
        ];
        assert_eq!(find_index(&jobs, "com.skiff.nginx", "nightly", ""), Some(0));
        assert_eq!(find_index(&jobs, "ui", "", "j2"), Some(1));
        // 同 key 不同来源不是同一个任务
        assert_eq!(find_index(&jobs, "com.skiff.files", "nightly", ""), None);
        // 没有 id 也没有 key 一律视为新增
        assert_eq!(find_index(&jobs, "ui", "", ""), None);
    }

    #[test]
    fn ids_are_unique_within_the_same_second() {
        let now = 1_700_000_000;
        let mut jobs: Vec<Value> = Vec::new();
        for _ in 0..5 {
            let id = make_id(now, &jobs);
            assert!(!jobs.iter().any(|j| j["id"].as_str() == Some(id.as_str())));
            jobs.push(json!({ "id": id }));
        }
        assert_eq!(jobs[0]["id"], "j170000000000");
        assert_eq!(jobs[4]["id"], "j170000000004");
    }

    #[test]
    fn next_run_is_computed_for_ui() {
        let from = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        let t = next_run("0 3 * * *", from).unwrap();
        assert_eq!(t.format("%Y-%m-%d %H:%M").to_string(), "2026-11-02 03:00");
        assert!(next_run("bad", from).is_none());
        let v = next_run_json("0 3 * * *");
        assert!(v["at"].as_i64().unwrap() > 0);
        assert!(v["text"].as_str().unwrap().contains(':'));
    }

    #[test]
    fn forbidden_errors_carry_the_grant_hint() {
        let e = call_err("com.skiff.nginx", "内核返回 403");
        assert!(e.contains("plugin:com.skiff.nginx"), "{e}");
        let e = call_err("com.skiff.nginx", "504 超时");
        assert!(!e.contains("勾选"));
    }

    #[test]
    fn job_view_attaches_next_run_without_mutating_stored_job() {
        let job = json!({"id": "j1", "expr": "0 3 * * *", "enabled": true});
        let v = job_view(&job);
        assert!(v["next_run"]["text"].is_string());
        assert!(job.get("next_run").is_none());
    }
}
