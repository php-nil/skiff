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
