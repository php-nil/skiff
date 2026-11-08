//! `com.skiff.testkit`：插件架构测试台。
//!
//! 不是业务插件，而是把宿主机上「插件能做什么、不能做什么」摊开验证：
//! - 多视图 / 单视图菜单声明与深链；
//! - RPC 成功与失败分支；
//! - `fs` 授权内读取与越界拒绝、`identity:self`、`interop:call`；
//! - 概览槽位 `[[mount]]`、主题跟随、沙箱 iframe 表单与键盘事件。
//!
//! 所有能力都真实回调内核，不做本地放行——测试台自身也受同一套权限闸限制。

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use skiff_sdk::{Plugin, RpcError};

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
        "ping" => Ok(ping(&p)),
        "fail" => Err(RpcError::invalid_params(
            "预期失败分支：fail 永远返回 -32602（用于验证错误透传）",
        )),
        "state" => Ok(state(&p)),
        "identity" => p.identity().map_err(rpc_err),
        "fs_read" => fs_read(&p, &params),
        "interop_catalog" => p.interop_catalog().map_err(rpc_err),
        "interop_cron" => interop_cron(&p),
        "scheduler_probe" => scheduler_probe(&p),
        "busy_probe" => scheduler_probe(&p),
        "buffer_probe" | "buffer_stat" => buffer_probe(&p, &params),
        "buffer_push" => buffer_push(&p, &params),
        "probe" => Ok(probe(&p)),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn rpc_err(e: skiff_sdk::SdkError) -> RpcError {
    RpcError::internal(format!("{e}"))
}

fn ping(plugin: &Plugin) -> Value {
    json!({ "pong": true, "id": plugin.id, "at": now() })
}

fn state(plugin: &Plugin) -> Value {
    json!({
        "id": plugin.id,
        "api": plugin.api,
        "granted": plugin.granted(),
        "data_dir": plugin.data_dir.display().to_string(),
        "pid": std::process::id(),
        "at": now(),
    })
}

fn probe(plugin: &Plugin) -> Value {
    json!({
        "id": plugin.id,
        "api": plugin.api,
        "granted": plugin.granted(),
        "at": now(),
    })
}

/// 只读：`path` 由调用方传入，越界与否由内核 `path:` 授权根判定。
fn fs_read(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let path = params
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("缺少 path"))?;
    match plugin.fs_host_read_text(path) {
        Ok(text) => Ok(json!({
            "path": path,
            "bytes": text.len(),
            "preview": text.chars().take(120).collect::<String>(),
        })),
        Err(e) => Err(RpcError::invalid_params(format!(
            "读取 {path} 被拒绝或失败：{e}（越界路径应在此分支被拒绝）"
        ))),
    }
}

/// `interop:call` 真实调用计划任务；目标未安装/未授权时走失败分支。
fn interop_cron(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .interop_call("com.skiff.cron", "jobs.list", json!({}))
        .map(|v| json!({ "ok": true, "result": v }))
        .map_err(|e| {
            RpcError::internal(format!(
                "调用 com.skiff.cron:jobs.list 失败：{e}（需安装计划任务并授予 plugin:com.skiff.cron）"
            ))
        })
}

/// 取任务 id：`buffer.*` 只作用于本插件来源的任务，越权由内核 `ensure_owned` 拒绝。
fn param_id(params: &Value) -> Result<&str, RpcError> {
    params
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("缺少 id"))
}

/// `buffer.stat` 只读探针：原样回报内核缓冲统计（容量/条数/丢弃/拒绝/高水位等），
/// 不额外加工字段，避免测试台与内核口径漂移。
fn buffer_probe(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = param_id(params)?;
    plugin
        .scheduler_call("buffer.stat", json!({ "id": id }))
        .map(|v| {
            let stat = v.get("stat").cloned().unwrap_or(Value::Null);
            json!({
                "ok": true,
                "id": id,
                "stat": stat,
                "fields": [
                    "points", "bytes", "capacity_bytes", "seq", "head_seq", "tail_seq",
                    "dropped_points", "dropped_bytes", "rejected_points", "rejected_bytes",
                    "aggregated_groups", "consumed_points", "consumed_bytes", "high_watermark"
                ],
            })
        })
        .map_err(|e| {
            RpcError::internal(format!(
                "buffer.stat 失败：{e}（需 scheduler:read，且任务须属于本插件）"
            ))
        })
}

/// `buffer.push` 写入探针：内核每次只收一个 `point`，这里按 `count` 循环推入，
/// 用于构造缓冲溢出与回执场景。`count` 上限 100，对齐内核每任务 100 点/秒限速。
fn buffer_push(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let id = param_id(params)?;
    let count = params.get("count").and_then(Value::as_u64).unwrap_or(1);
    if !(1..=100).contains(&count) {
        return Err(RpcError::invalid_params(
            "count 需在 1..=100（内核限速每任务 100 点/秒）",
        ));
    }
    let point = params
        .get("point")
        .cloned()
        .unwrap_or_else(|| json!({ "at": now() }));

    let mut accepted = 0u64;
    let mut rejected = 0u64;
    let mut dropped_points = 0u64;
    let mut dropped_bytes = 0u64;
    let mut aggregated = 0u64;
    let mut signals: Vec<String> = Vec::new();
    let mut last = Value::Null;

    for i in 0..count {
        let report = plugin
            .scheduler_call("buffer.push", json!({ "id": id, "point": point }))
            .map_err(|e| {
                RpcError::internal(format!(
                    "buffer.push 失败（已成功推入 {i} 条）：{e}（需 scheduler:manage，且任务须属于本插件）"
                ))
            })?;
        let outcome = report.get("outcome").cloned().unwrap_or(Value::Null);
        if outcome
            .get("accepted")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            accepted += 1;
        } else {
            rejected += 1;
        }
        dropped_points += outcome
            .get("dropped_points")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        dropped_bytes += outcome
            .get("dropped_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        aggregated += outcome
            .get("aggregated")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(list) = report.get("signals").and_then(Value::as_array) {
            for s in list.iter().filter_map(Value::as_str) {
                if !signals.iter().any(|x| x == s) {
                    signals.push(s.to_string());
                }
            }
        }
        last = report;
    }

    Ok(json!({
        "ok": true,
        "id": id,
        "requested": count,
        "accepted": accepted,
        "rejected": rejected,
        "dropped_points": dropped_points,
        "dropped_bytes": dropped_bytes,
        "aggregated": aggregated,
        "signals": signals,
        "last": last,
    }))
}

/// 回调 scheduler：直接调用时正常返回任务列表；作为任务执行体（`busy_probe`）
/// 调用时应被内核 403 拒绝，错误原文透传给任务运行历史。
fn scheduler_probe(plugin: &Plugin) -> Result<Value, RpcError> {
    plugin
        .scheduler_call("task.list", json!({}))
        .map(|v| {
            let count = v
                .get("tasks")
                .and_then(|t| t.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            json!({ "ok": true, "count": count, "result": v })
        })
        .map_err(|e| {
            RpcError::internal(format!(
                "执行期回调 scheduler 被拒绝：{e}（BusyGuard 期望 403）"
            ))
        })
}
