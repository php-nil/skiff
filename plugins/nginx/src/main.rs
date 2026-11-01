//! `com.skiff.nginx`：NGINX 站点管理（零内核改动：写授权目录 + 重启容器）。
//!
//! 一期刻意不做 `nginx -t`——那要 `containers.exec`（属 M3 门禁）。这里走
//! 「写授权目录 → 管理员点重载重启容器」：配置写坏时容器起不来，面板本身不受影响。
//!
//! 站点目录与容器都必须在安装时授权：目录命中 `path:` 根，容器命中
//! `container:NAME`。插件侧先做一次预检（为了给出人话错误），内核侧再复验一次。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

/// 单站点配置上限：整文件读写与 [`skiff_sdk::HOST_IO_MAX`] 同口径（256KiB）。
const MAX_SITE_BYTES: usize = skiff_sdk::HOST_IO_MAX;
const STATE_FILE: &str = "backups.json";
/// 备份保留份数上限：再多也没人看，纯占盘。
const KEEP_MAX: u64 = 90;
/// 计划任务插件：`新增备份任务` 按钮通过 interop 往它写 `jobs.create`。
const CRON_PLUGIN: &str = "com.skiff.cron";
/// 幂等键：同一个来源 + 同一个 key 反复写只会更新这一条任务。
const BACKUP_KEY: &str = "nightly";

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
        "sites" => sites(&p),
        "read" => read(&p, &params),
        "save" => save(&p, &params, false),
        "create" => save(&p, &params, true),
        "remove" => remove(&p, &params),
        "rename" => rename(&p, &params),
        "reload" => reload(&p),
        "backup" | "tick" => backup(&p),
        "scheduleBackup" => schedule_backup(&p, &params),
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
    out["roots"] = json!(plugin.path_roots().unwrap_or_default());
    out
}

/// 让「403」不至于只剩一串内核原文：插件侧补一句「该去勾哪个授权」。
fn fs_err(e: skiff_sdk::SdkError) -> RpcError {
    let msg = format!("{e}");
    if msg.contains("403") {
        RpcError::invalid_params(format!("{msg}（请确认安装时勾选了对应 path: 授权）"))
    } else {
        common::rpc_err(e)
    }
}

fn docker_err(e: skiff_sdk::SdkError) -> RpcError {
    let msg = format!("{e}");
    if msg.contains("403") {
        RpcError::invalid_params(format!(
            "{msg}（请确认已授予 docker 动词，且容器命中 container: 对象授权）"
        ))
    } else {
        common::rpc_err(e)
    }
}

/// 站点名（不含 `.conf`）：只允许 `[A-Za-z0-9._-]`，拒绝隐藏名与 `..`。
///
/// 这里是**命名规范**而不是路径越界判定，所以按子串拒 `..`（连写两个点的
/// 名字一律不接受），不是按路径组件判定。
fn valid_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() || n.len() > 64 || n.starts_with('.') || n.contains("..") {
        return false;
    }
    n.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// 允许用户带 `.conf` 或不带，入库一律按「裸名」。
fn bare_name(raw: &str) -> String {
    let n = raw.trim();
    n.strip_suffix(".conf").unwrap_or(n).trim().to_string()
}

fn site_root(plugin: &Plugin) -> Result<String, RpcError> {
    let cfg = common::load_config(plugin);
    let dir = common::normalize_dir(&common::cfg_str(&cfg, "site_dir", "/etc/nginx/conf.d"));
    if !common::is_posix_absolute(&dir) {
        return Err(RpcError::invalid_params(
            "配置 site_dir 必须是绝对路径（如 /etc/nginx/conf.d）".to_string(),
        ));
    }
    let roots = plugin.path_roots().map_err(common::rpc_err)?;
    if !common::within_any(&dir, &roots) {
        let shown = if roots.is_empty() {
            "无（安装时一个 path: 都没勾）".to_string()
        } else {
            roots.join("、")
        };
        return Err(RpcError::invalid_params(format!(
            "site_dir（{dir}）不在已授权路径内；当前已授权：{shown}"
        )));
    }
    Ok(dir)
}

/// 从 `name` 参数解析出「裸名 + 绝对路径」。
fn site_of(plugin: &Plugin, params: &Value) -> Result<(String, String), RpcError> {
    let name = bare_name(&common::param_str(params, "name"));
    if !valid_name(&name) {
        return Err(RpcError::invalid_params(format!(
            "站点名非法: {:?}（只允许字母/数字/`.`/`-`/`_`，最长 64）",
            common::param_str(params, "name")
        )));
    }
    let root = site_root(plugin)?;
    Ok((name.clone(), common::join(&root, &format!("{name}.conf"))))
}

/// 站点目录里可管理的 `.conf` 文件（`items` 来自内核 `fs.list`）。
fn sites_from(listing: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = listing
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|i| !i.get("dir").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|i| {
                    let file = i.get("name")?.as_str()?;
                    if !file.ends_with(".conf") {
                        return None;
                    }
                    let name = bare_name(file);
                    if !valid_name(&name) {
                        return None;
                    }
                    Some(json!({
                        "name": name,
                        "file": file,
                        "size": i.get("size").cloned().unwrap_or(json!(0)),
                        "mtime": i.get("mtime").cloned().unwrap_or(Value::Null),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    out
}

fn has_site(sites: &[Value], name: &str) -> bool {
    sites.iter().any(|s| s["name"].as_str() == Some(name))
}

struct Hit {
    id: String,
    name: String,
    state: String,
    labels: serde_json::Map<String, Value>,
}

/// 按容器名从 `containers.list` 的结果里取一条（含 id：重启必须带 id）。
fn find_hit(list: &Value, wanted: &str) -> Option<Hit> {
    let arr = list.as_array()?;
    arr.iter().find_map(|r| {
        let name = r.get("name")?.as_str()?;
        if name != wanted {
            return None;
        }
        Some(Hit {
            id: r
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: name.to_string(),
            state: r
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            labels: r
                .get("labels")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default(),
        })
    })
}

/// 配置里的容器 → 已授权的那条记录。内核把 docker 结果包在 `{"result": …}` 里。
fn container_hit(plugin: &Plugin) -> Result<Hit, RpcError> {
    let cfg = common::load_config(plugin);
    let name = common::cfg_str(&cfg, "container", "nginx");
    if !valid_name(&name) {
        return Err(RpcError::invalid_params(format!(
            "配置 container 不是合法容器名: {name}"
        )));
    }
    let list = common::unwrap_result(
        plugin
            .docker_call("containers.list", Value::Null)
            .map_err(docker_err)?,
    );
    let hit = find_hit(&list, &name).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "找不到容器 {name}（或未授权）；请核对配置里的容器名，并在安装确认页勾选 container:{name}"
        ))
    })?;
    if hit.id.is_empty() {
        return Err(RpcError::internal(format!("容器 {name} 缺少 id，无法重启")));
    }
    Ok(hit)
}

fn target_of(hit: &Hit) -> Value {
    let mut t = json!({ "id": hit.id, "name": hit.name });
    if !hit.labels.is_empty() {
        t["labels"] = Value::Object(hit.labels.clone());
    }
    t
}

fn sites(plugin: &Plugin) -> Result<Value, RpcError> {
    let root = site_root(plugin)?;
    let listing = plugin.fs_host_list(&root).map_err(fs_err)?;
    let container = match container_hit(plugin) {
        Ok(h) => json!({ "name": h.name, "state": h.state }),
        // 没授权 docker 或不叫这个名：UI 只显示站点列表，不阻断读写。
        Err(_) => Value::Null,
    };
    Ok(json!({
        "ok": true,
        "dir": root,
        "sites": sites_from(&listing),
        "container": container,
        "roots": plugin.path_roots().unwrap_or_default(),
    }))
}

fn read(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (name, path) = site_of(plugin, params)?;
    let content = plugin.fs_host_read_text(&path).map_err(fs_err)?;
    Ok(json!({
        "ok": true,
        "name": name,
        "path": path,
        "content": content,
        "size": content.len(),
    }))
}

/// `save`（新建或覆盖）与 `create`（仅新建）共用一处写盘逻辑，差别只在存在性检查。
fn save(plugin: &Plugin, params: &Value, create_only: bool) -> Result<Value, RpcError> {
    let (name, path) = site_of(plugin, params)?;
    let content = common::param_str(params, "content");
    if content.len() > MAX_SITE_BYTES {
        return Err(RpcError::invalid_params(format!(
            "配置超过 {MAX_SITE_BYTES} 字节，请拆分文件"
        )));
    }
    if create_only {
        let root = site_root(plugin)?;
        let listing = plugin.fs_host_list(&root).map_err(fs_err)?;
        if has_site(&sites_from(&listing), &name) {
            return Err(RpcError::invalid_params(format!("站点 {name} 已存在")));
        }
    }
    plugin.fs_host_write_text(&path, &content).map_err(fs_err)?;
    Ok(json!({
        "ok": true,
        "name": name,
        "path": path,
        "size": content.len(),
        "created": create_only,
    }))
}

fn remove(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (name, path) = site_of(plugin, params)?;
    plugin.fs_host_remove(&path).map_err(fs_err)?;
    Ok(json!({ "ok": true, "name": name, "removed": path }))
}

fn rename(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let (from_name, from) = site_of(plugin, params)?;
    let to_raw = common::param_str(params, "to");
    let to_name = bare_name(&to_raw);
    if !valid_name(&to_name) {
        return Err(RpcError::invalid_params(format!(
            "新站点名非法: {to_raw:?}"
        )));
    }
    if to_name == from_name {
        return Err(RpcError::invalid_params("新旧站点名相同".to_string()));
    }
    let root = site_root(plugin)?;
    let to = common::join(&root, &format!("{to_name}.conf"));
    plugin.fs_host_rename(&from, &to).map_err(fs_err)?;
    Ok(json!({ "ok": true, "from": from_name, "to": to_name }))
}

fn reload(plugin: &Plugin) -> Result<Value, RpcError> {
    let hit = container_hit(plugin)?;
    plugin
        .docker_call("containers.restart", target_of(&hit))
        .map_err(docker_err)?;
    Ok(json!({
        "ok": true,
        "container": hit.name,
        "state": hit.state,
        "note": "容器已重启；起不来说明配置有误，可在备份里回滚",
    }))
}

/// 快照目录名用 12 位补零的 Unix 秒：字典序即时间序，剪枝不用解析时间。
fn snapshot_name(now: u64) -> String {
    format!("{now:012}")
}

fn is_snapshot(name: &str) -> bool {
    name.len() == 12 && name.chars().all(|c| c.is_ascii_digit())
}

/// 只留最近 `keep` 份，返回被删掉的快照目录名。
fn prune(plugin: &Plugin, keep: u64) -> Result<Vec<String>, RpcError> {
    let Ok(listing) = plugin.fs_list("backups") else {
        return Ok(Vec::new()); // 还没有任何备份
    };
    let mut dirs: Vec<String> = listing
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|i| i.get("dir").and_then(|v| v.as_bool()).unwrap_or(false))
                .filter_map(|i| {
                    i.get("name")?
                        .as_str()
                        .filter(|n| is_snapshot(n))
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    let excess = dirs.len().saturating_sub(keep as usize);
    let mut removed = Vec::new();
    for dir in dirs.into_iter().take(excess) {
        if plugin.fs_host_remove(&format!("backups/{dir}")).is_ok() {
            removed.push(dir);
        }
    }
    Ok(removed)
}

/// 把当前站点配置拷进 `data/backups/<快照>/`（由 cron 每天 03:00 调一次，UI 也能手动）。
fn backup(plugin: &Plugin) -> Result<Value, RpcError> {
    let root = site_root(plugin)?;
    let now = common::now_secs();
    let snap = snapshot_name(now);
    let listing = plugin.fs_host_list(&root).map_err(fs_err)?;
    let files: Vec<String> = sites_from(&listing)
        .iter()
        .filter_map(|s| s["file"].as_str().map(str::to_string))
        .collect();
    if files.is_empty() {
        return Ok(json!({ "ok": true, "at": now, "saved": [], "skipped": [], "pruned": [] }));
    }

    let mut saved = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        match plugin.fs_host_read_text(&common::join(&root, &file)) {
            // 写进自身 data/ 时内核会自动建父目录（resolve_data 的 create 分支）。
            Ok(text) => match plugin.fs_write(&format!("backups/{snap}/{file}"), &text) {
                Ok(_) => saved.push(file),
                Err(e) => skipped.push(json!({ "file": file, "reason": e.to_string() })),
            },
            Err(e) => skipped.push(json!({ "file": file, "reason": e.to_string() })),
        }
    }

    let cfg = common::load_config(plugin);
    let keep = common::cfg_u64(&cfg, "backup_keep", 7).clamp(1, KEEP_MAX);
    let pruned = prune(plugin, keep)?;

    common::write_state(
        plugin,
        STATE_FILE,
        &json!({
            "at": now,
            "snapshot": snap,
            "files": saved,
            "skipped": skipped,
            "keep": keep,
            "pruned": pruned,
        }),
    )
    .map_err(common::rpc_err)?;

    Ok(json!({
        "ok": true,
        "at": now,
        "dir": format!("backups/{snap}"),
        "saved": saved,
        "skipped": skipped,
        "pruned": pruned,
        "keep": keep,
    }))
}

fn interop_err(e: skiff_sdk::SdkError) -> RpcError {
    let msg = format!("{e}");
    if msg.contains("404") {
        RpcError::invalid_params(format!("{msg}（请先安装并启用「计划任务」插件）"))
    } else if msg.contains("403") {
        RpcError::invalid_params(format!(
            "{msg}（请在安装确认页勾选 plugin:{CRON_PLUGIN}，并保留 interop:call 权限）"
        ))
    } else if msg.contains("409") {
        RpcError::invalid_params(format!("{msg}（计划任务插件可能已停用，或正忙于其他调用）"))
    } else {
        common::rpc_err(e)
    }
}

/// 把「每日备份」写成计划任务插件里的一条任务（按 `source + key` 幂等）。
///
/// `source` 由内核在转发时注入为本插件 id，因此重复点按钮、或换一个管理员再点，
/// 都只会更新同一条任务，不会堆出多份快照。
fn schedule_backup(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let cfg = common::load_config(plugin);
    let expr = {
        let raw = common::param_str(params, "expr");
        let raw = raw.trim();
        if raw.is_empty() {
            common::cfg_str(&cfg, "backup_expr", "0 3 * * *")
        } else {
            raw.to_string()
        }
    };
    let r = plugin
        .interop_call(
            CRON_PLUGIN,
            "jobs.create",
            json!({
                "name": "NGINX 站点配置备份",
                "expr": expr.clone(),
                "target": plugin.id,
                "method": "backup",
                "params": {},
                "enabled": true,
                "key": BACKUP_KEY,
            }),
        )
        .map_err(interop_err)?;
    Ok(json!({
        "ok": true,
        "plugin": CRON_PLUGIN,
        "created": r.get("created").cloned().unwrap_or(json!(false)),
        "job_id": r.get("id").cloned().unwrap_or(Value::Null),
        "next_run": r.get("next_run").cloned().unwrap_or(Value::Null),
        "expr": expr,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_names_are_strict() {
        assert!(valid_name("example.com"));
        assert!(valid_name("api-v2_2024"));
        assert!(!valid_name(""));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("a..b"));
        assert!(!valid_name(&"x".repeat(65)));
        assert_eq!(bare_name("example.com.conf"), "example.com");
        assert_eq!(bare_name(" example.com "), "example.com");
    }

    #[test]
    fn sites_filter_partial_listings() {
        let listing = json!({ "items": [
            {"name": "a.conf", "dir": false, "size": 10, "mtime": 5},
            {"name": "b.txt", "dir": false, "size": 1},
            {"name": "sub", "dir": true},
            {"name": ".secret.conf", "dir": false}
        ]});
        let sites = sites_from(&listing);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0]["name"], "a");
        assert!(has_site(&sites, "a"));
        assert!(!has_site(&sites, "b"));
        assert!(sites_from(&json!({})).is_empty());
    }

    #[test]
    fn finds_container_by_name_and_keeps_labels() {
        let list = common::unwrap_result(json!({ "result": [
            {"id": "abc", "name": "nginx", "state": "running", "labels": {"io.skiff.project": "web"}},
            {"id": "def", "name": "other", "state": "exited"}
        ]}));
        let hit = find_hit(&list, "nginx").unwrap();
        assert_eq!(hit.id, "abc");
        assert_eq!(hit.state, "running");
        let target = target_of(&hit);
        assert_eq!(target["id"], "abc");
        assert_eq!(target["labels"]["io.skiff.project"], "web");
        assert!(find_hit(&list, "missing").is_none());
    }

    #[test]
    fn snapshots_sort_by_time_and_only_digits() {
        assert_eq!(snapshot_name(1_700_000_000), "001700000000");
        assert!(is_snapshot("001700000000"));
        assert!(!is_snapshot("backup-1"));
        let mut dirs = vec!["001700000100", "001700000000", "001700000200"];
        dirs.sort();
        assert_eq!(dirs, vec!["001700000000", "001700000100", "001700000200"]);
    }
}
