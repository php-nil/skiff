//! `com.skiff.guard`：容器守护（零内核改动版）。
//!
//! 巡检由**内核调度器**按 manifest `[[task]]` 每分钟调用一次 `reconcile`；插件用
//! 既有 docker 能力列出容器，发现 `dead` / `restarting` / `unhealthy`（可选
//! `exited`）就按冷却时间与每小时预算重启。**未授权的容器只记录、不重启**——
//! 对象授权的判定同时在内核侧复验一遍，插件侧的预筛只是为了让 UI 能提前显示
//! 「未授权」。插件内没有定时器，也没有旧 `tick` 入口；UI 只读展示。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const STATE_FILE: &str = "guard.json";
const MAX_EVENTS: usize = 100;
const TASK_ID: &str = "plugin:com.skiff.guard:reconcile";

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    eprintln!("[{TASK_ID}] 巡检由内核调度器驱动，插件内无定时器");
    if let Err(e) = plugin.serve(move |method, params| match method {
        "reconcile" => reconcile(&p, params),
        "status" => status(&p),
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
    Value::Object(map)
}

/// 取值上限，避免配置被写成一个离谱的数字后在 UI 上溢出。
fn clamp(v: u64, max: u64) -> u64 {
    v.min(max)
}

/// 容器是否处于「需要守护」的状态。
fn is_suspect(state: &str, status: &str, restart_exited: bool) -> bool {
    match state.to_ascii_lowercase().as_str() {
        "dead" | "restarting" => true,
        "exited" => restart_exited,
        _ => status.to_ascii_lowercase().contains("unhealthy"),
    }
}

/// 对象授权是否覆盖该容器（名字/ID/项目三选一，与内核 `Policy::check` 同口径）。
fn covered(objects: &[String], id: &str, name: &str, project: Option<&str>) -> bool {
    objects.iter().any(|o| {
        o == &format!("container:{id}")
            || o == &format!("container:{name}")
            || project
                .map(|p| o == &format!("project:{p}"))
                .unwrap_or(false)
    })
}

fn rows_of(list: &Value) -> Vec<(String, String, String, String, Option<String>)> {
    let mut out = Vec::new();
    let Some(arr) = list.as_array() else {
        return out;
    };
    for r in arr {
        let id = r
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let name = r
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let state = r
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = r
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let project = r
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                r.get("labels")
                    .and_then(|v| v.get("io.skiff.project"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        if !id.is_empty() && !name.is_empty() {
            out.push((id, name, state, status, project));
        }
    }
    out
}

fn push_event(state: &mut Value, item: Value) {
    let events = state
        .get_mut("events")
        .and_then(|v| v.as_array_mut())
        .expect("state.events 恒为数组");
    events.push(item);
    if events.len() > MAX_EVENTS {
        let drop = events.len() - MAX_EVENTS;
        events.drain(0..drop);
    }
}

/// 内核执行出票：一轮全量对账。告警/冷却/预算/未授权只报不动语义保持不变；
/// 返回值走调度器 `ExecOutput`（无缓冲，`verdict` 省略等价 `keep`）。
fn reconcile(plugin: &Plugin, _params: Value) -> Result<Value, RpcError> {
    let cfg = common::load_config(plugin);
    let cooldown = clamp(common::cfg_u64(&cfg, "cooldown_sec", 300), 86400);
    let max_per_hour = clamp(common::cfg_u64(&cfg, "max_per_hour", 6), 1000);
    let dry_run = common::cfg_bool(&cfg, "dry_run", false);
    let restart_exited = common::cfg_bool(&cfg, "restart_exited", false);
    let watch = common::csv(&common::cfg_str(&cfg, "watch", ""));

    let now = common::now_secs();
    let hour = now / 3600;
    let mut st = common::read_state(plugin, STATE_FILE);
    if !st.is_object() {
        st = json!({});
    }
    if st.get("hour").and_then(|v| v.as_u64()) != Some(hour) {
        st["hour"] = json!(hour);
        st["count"] = json!(0);
    }
    if !st.get("events").map(|v| v.is_array()).unwrap_or(false) {
        st["events"] = json!([]);
    }
    if !st.get("containers").map(|v| v.is_object()).unwrap_or(false) {
        st["containers"] = json!({});
    }

    let objects = plugin.objects().unwrap_or_default();
    // 内核的 docker 回调把结果包在 `{"result": …}` 里，这里先脱一层再按行解析。
    let list = common::unwrap_result(
        plugin
            .docker_call("containers.list", Value::Null)
            .map_err(common::rpc_err)?,
    );
    let rows = rows_of(&list);

    let mut restarted: Vec<String> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut unauthorized: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for (id, name, state, status, project) in rows {
        if !watch.is_empty() && !watch.iter().any(|w| w == &name || w == &id) {
            continue;
        }
        if !is_suspect(&state, &status, restart_exited) {
            continue;
        }
        checked += 1;
        if !covered(&objects, &id, &name, project.as_deref()) {
            unauthorized.push(name.clone());
            continue;
        }

        let last = st["containers"]
            .get(&name)
            .and_then(|c| c.get("last"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if cooldown > 0 && now.saturating_sub(last) < cooldown {
            skipped.push(
                json!({ "name": name, "reason": "cooldown", "retry_in": cooldown - (now - last) }),
            );
            continue;
        }
        let count = st.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        if max_per_hour > 0 && count >= max_per_hour {
            skipped.push(json!({ "name": name, "reason": "hour_budget", "limit": max_per_hour }));
            continue;
        }

        if dry_run {
            push_event(
                &mut st,
                json!({ "t": now, "name": name, "result": "dry-run", "detail": format!("{state}/{status}") }),
            );
            skipped.push(json!({ "name": name, "reason": "dry_run" }));
            continue;
        }

        // 真实重启：target 带上 id/name/项目标签，内核按对象 grant 复验一次。
        let mut t = json!({ "id": id, "name": name });
        if let Some(pr) = &project {
            t["labels"] = json!({ "io.skiff.project": pr });
        }
        match plugin.docker_call("containers.restart", t) {
            Ok(_) => {
                let total = st["containers"]
                    .get(&name)
                    .and_then(|c| c.get("total"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    + 1;
                st["containers"][&name] = json!({ "last": now, "total": total });
                st["count"] = json!(count + 1);
                restarted.push(name.clone());
                push_event(
                    &mut st,
                    json!({ "t": now, "name": name, "result": "restarted", "detail": format!("{state}/{status}") }),
                );
            }
            Err(e) => {
                let msg = format!("{e}");
                push_event(
                    &mut st,
                    json!({ "t": now, "name": name, "result": "failed", "detail": msg }),
                );
                skipped.push(json!({ "name": name, "reason": "restart_failed", "detail": msg }));
            }
        }
    }

    common::write_state(plugin, STATE_FILE, &st).map_err(common::rpc_err)?;

    // 状态与事件已经落盘；执行回执照调度器协议返回，`detail` 保留本轮摘要。
    let summary = json!({
        "at": now,
        "checked": checked,
        "restarted": restarted,
        "skipped": skipped,
        "unauthorized": unauthorized,
        "dry_run": dry_run,
        "hour_count": st.get("count").cloned().unwrap_or(json!(0)),
    });
    Ok(json!({
        "detail": summary.to_string(),
        "output_bytes": 0,
    }))
}

fn relative_ago(now: u64, ts: u64) -> String {
    if ts == 0 {
        return "-".into();
    }
    let d = now.saturating_sub(ts);
    if d < 60 {
        "刚刚".into()
    } else if d < 3600 {
        format!("{} 分钟前", d / 60)
    } else if d < 86400 {
        format!("{} 小时前", d / 3600)
    } else {
        format!("{} 天前", d / 86400)
    }
}

fn status(plugin: &Plugin) -> Result<Value, RpcError> {
    let st = common::read_state(plugin, STATE_FILE);
    let objects = plugin.objects().unwrap_or_default();
    let cfg = common::load_config(plugin);
    let now = common::now_secs();
    // 旧 UI 直接读 `state.containers`（对象表）；新壳的静态 schema 只消费数组，
    // 这里补一份派生的行数组，避免页面模板承担对象→数组的转换逻辑。
    let mut containers: Vec<Value> = st
        .get("containers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(name, row)| {
                    let last = row.get("last").and_then(|v| v.as_u64()).unwrap_or(0);
                    json!({
                        "name": name,
                        "last": last,
                        "last_text": relative_ago(now, last),
                        "total": row.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    containers.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    let mut events: Vec<Value> = st
        .get("events")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .rev()
                .map(|e| {
                    let t = e.get("t").and_then(|v| v.as_u64()).unwrap_or(0);
                    json!({
                        "t": t,
                        "t_text": relative_ago(now, t),
                        "name": e.get("name").cloned().unwrap_or(json!("")),
                        "result": e.get("result").cloned().unwrap_or(json!("")),
                        "detail": e.get("detail").cloned().unwrap_or(json!("")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if events.len() > MAX_EVENTS {
        events.truncate(MAX_EVENTS);
    }
    let dry_run = common::cfg_bool(&cfg, "dry_run", false);
    let containers_empty = containers.is_empty();
    let events_empty = events.is_empty();
    let objects_count = objects.len();
    let hour_count = st.get("count").cloned().unwrap_or(json!(0));
    Ok(json!({
        "state": st,
        "containers": containers,
        "containers_empty": containers_empty,
        "events": events,
        "events_empty": events_empty,
        "objects": objects,
        "objects_count": objects_count,
        "hour_count": hour_count,
        "dry_run": dry_run,
        "dry_run_text": if dry_run { "开" } else { "关" },
        "granted": plugin.granted(),
        "config": cfg,
        "scheduler": { "driver": "kernel", "task_id": TASK_ID },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspect_states() {
        assert!(is_suspect("dead", "", false));
        assert!(is_suspect("restarting", "", false));
        assert!(is_suspect("running", "Up 2 hours (unhealthy)", false));
        assert!(!is_suspect("exited", "", false));
        assert!(is_suspect("exited", "", true));
        assert!(!is_suspect("running", "Up 3 hours (healthy)", true));
    }

    #[test]
    fn object_grant_covering() {
        let grants = vec![
            "container:nginx".to_string(),
            "project:web".to_string(),
            "path:/var/log".to_string(),
        ];
        assert!(covered(&grants, "abc", "nginx", None));
        assert!(covered(&grants, "abc", "other", Some("web")));
        assert!(!covered(&grants, "abc", "other", None));
        assert!(!covered(&grants, "abc", "other", Some("db")));
    }

    #[test]
    fn rows_parse_partial_objects() {
        let list = json!([
            {"id": "abc", "name": "web", "state": "running", "status": "Up", "labels": {"io.skiff.project": "proj"}},
            {"id": "", "name": "broken"},
        ]);
        let rows = rows_of(&list);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "web");
        assert_eq!(rows[0].4.as_deref(), Some("proj"));
    }
}
