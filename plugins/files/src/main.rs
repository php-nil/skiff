//! `com.skiff.files`：文件管理（零内核改动：只走已有宿主 fs 能力面）。
//!
//! 授权模型是「两把钥匙」：权限串（`fs:read/write/list/mkdir/remove/rename`）
//! 加上安装时确认的 `path:` 对象授权根。插件侧先按授权根做一次预检（人话错误），
//! 内核侧再复验一次（真正的边界）。
//!
//! 大文件按 [`skiff_sdk::HOST_IO_MAX`]（256KiB）分页——协议是单帧 1MiB 的 UDS，
//! 整文件塞进一帧会被内核直接拒。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const MAX_PAGE: usize = skiff_sdk::HOST_IO_MAX;

/// 本插件只服务站点内容目录：`/www/sites/<域名>/index`（含其子目录）。
///
/// 站点域名在打包时未知，`resource_hints` 无法预置；安装时由管理员手动添加。
/// 这里对授权根做**形态收口**：不是站点内容目录的根一律拒绝，避免插件变成
/// 跨站点/全盘文件管理器（内核「工具 → 文件管理」才是管理员客户端）。
const SITE_CONTENT_PREFIX: &str = "/www/sites/";

/// 从授权根或路径取站点域名；不是站点内容目录返回 `None`。
fn site_domain(path: &str) -> Option<String> {
    let rest = path.strip_prefix(SITE_CONTENT_PREFIX)?;
    let (domain, tail) = rest.split_once('/')?;
    if domain.is_empty() {
        return None;
    }
    let tail = tail.trim_end_matches('/');
    if tail == "index" || tail.starts_with("index/") {
        Some(domain.to_string())
    } else {
        None
    }
}

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
        "roots" | "config" => Ok(config(&p)),
        "list" => list(&p, &params),
        "read" => read(&p, &params),
        "write" => write(&p, &params),
        "mkdir" => mkdir(&p, &params),
        "remove" => remove(&p, &params),
        "rename" => rename(&p, &params),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

fn config(plugin: &Plugin) -> Value {
    let cfg = common::load_config(plugin);
    let roots = plugin.path_roots().unwrap_or_default();
    let sites: Vec<String> = roots
        .iter()
        .filter_map(|r| site_domain(&common::normalize_dir(r)))
        .collect();
    let start = pick_start(&common::cfg_str(&cfg, "start_dir", ""), &roots);
    let map = cfg
        .into_iter()
        .map(|(k, v)| (k, Value::from(v)))
        .collect::<serde_json::Map<String, Value>>();
    json!({
        "config": Value::Object(map),
        "roots": roots,
        "sites": sites,
        "start_dir": start,
    })
}

fn fs_err(e: skiff_sdk::SdkError) -> RpcError {
    let msg = format!("{e}");
    // 只有「越权/不在授权根」才追加授权提示；403 也可能是只读宿主路径
    // （ProtectSystem），那时补授权建议会误导用户去改授权。
    if msg.contains("403") && msg.contains("已授权") {
        RpcError::invalid_params(format!(
            "{msg}（该路径不在已授权范围内；路径授权只能在安装确认页或由 admin 追加）"
        ))
    } else {
        common::rpc_err(e)
    }
}

fn granted_roots(plugin: &Plugin) -> Result<Vec<String>, RpcError> {
    let roots = plugin.path_roots().map_err(common::rpc_err)?;
    if roots.is_empty() {
        return Err(no_grant());
    }
    let bad: Vec<String> = roots
        .iter()
        .filter(|r| site_domain(&common::normalize_dir(r)).is_none())
        .cloned()
        .collect();
    if !bad.is_empty() {
        return Err(RpcError::invalid_params(format!(
            "站点文件只接受 /www/sites/<域名>/index 形态的授权根；以下授权根不在范围内：{}",
            bad.join("、")
        )));
    }
    Ok(roots)
}

fn no_grant() -> RpcError {
    RpcError::invalid_params(
        "未授权站点目录：安装插件时需添加 path:/www/sites/<域名>/index（站点内容目录），\
         回收站/配额/更多操作请用内核「工具 → 文件管理」"
            .to_string(),
    )
}

fn check(roots: &[String], path: &str) -> Result<(), RpcError> {
    if path.trim().is_empty() {
        return Err(RpcError::invalid_params("路径不能为空".to_string()));
    }
    if path.contains('\0') {
        return Err(RpcError::invalid_params("路径含非法字符".to_string()));
    }
    if !common::within_any(path, roots) {
        let shown = if roots.is_empty() {
            "无".to_string()
        } else {
            roots.join("、")
        };
        return Err(RpcError::invalid_params(format!(
            "路径 {path} 不在已授权范围内；当前已授权：{shown}"
        )));
    }
    Ok(())
}

/// 首页目录：配置里指定且仍被授权的优先，否则第一个授权根。
fn pick_start(raw: &str, roots: &[String]) -> Option<String> {
    let wanted = common::normalize_dir(raw);
    if !wanted.is_empty() && common::within_any(&wanted, roots) {
        return Some(wanted);
    }
    roots.first().map(|r| common::normalize_dir(r))
}

fn list(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    if roots.is_empty() {
        return Err(no_grant());
    }
    let cfg = common::load_config(plugin);
    let raw = common::param_str(params, "path");
    let dir = if raw.trim().is_empty() {
        pick_start(&common::cfg_str(&cfg, "start_dir", ""), &roots).ok_or_else(no_grant)?
    } else {
        common::normalize_dir(&raw)
    };
    check(&roots, &dir)?;

    let v = plugin.fs_host_list(&dir).map_err(fs_err)?;
    Ok(json!({
        "ok": true,
        "path": dir,
        "parent": common::parent_within(&dir, &roots),
        "roots": roots,
        "items": v.get("items").cloned().unwrap_or(json!([])),
        "entries": v.get("entries").cloned().unwrap_or(json!([])),
    }))
}

fn read(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    let path = common::param_str(params, "path");
    check(&roots, &path)?;

    let offset = common::param_u64(params, "offset").unwrap_or(0);
    let max = common::param_u64(params, "max_bytes")
        .unwrap_or(MAX_PAGE as u64)
        .clamp(1, MAX_PAGE as u64) as usize;
    let (b64, off, size, eof) = plugin
        .fs_host_read_b64(&path, offset, max)
        .map_err(fs_err)?;
    Ok(json!({
        "ok": true,
        "path": path,
        "b64": b64,
        "offset": off,
        "size": size,
        "eof": eof,
        "page_max": MAX_PAGE,
    }))
}

/// 写一页：`b64`（可带 offset/append）或 `text`（整文件覆盖）。
fn write(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    let path = common::param_str(params, "path");
    check(&roots, &path)?;

    let append = params
        .get("append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let offset = common::param_u64(params, "offset");
    let saved = match common::param(params, "b64") {
        Some(b64) => plugin.fs_host_write_b64(&path, b64, offset, append),
        None => {
            let text = common::param(params, "text").unwrap_or("");
            if text.is_empty() && !append {
                // 允许「新建空文件」：SDK 显式写一次，否则变成什么都不做。
                return plugin
                    .fs_host_write_text(&path, "")
                    .map(|v| json!({ "ok": true, "path": path, "size": v.get("size") }))
                    .map_err(fs_err);
            }
            plugin.fs_host_write_text(&path, text)
        }
    }
    .map_err(fs_err)?;

    Ok(json!({
        "ok": true,
        "path": path,
        "size": saved.get("size").cloned().unwrap_or(Value::Null),
        "wrote": saved.get("wrote").cloned().unwrap_or(Value::Null),
    }))
}

fn mkdir(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    let path = common::param_str(params, "path");
    check(&roots, &path)?;
    plugin.fs_host_mkdir(&path).map_err(fs_err)?;
    Ok(json!({ "ok": true, "path": path }))
}

fn remove(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    let path = common::param_str(params, "path");
    check(&roots, &path)?;
    let v = plugin.fs_host_remove(&path).map_err(fs_err)?;
    Ok(json!({ "ok": true, "path": path, "dir": v.get("dir").cloned().unwrap_or(json!(false)) }))
}

fn rename(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let roots = granted_roots(plugin)?;
    let from = common::param_str(params, "path");
    let to = common::param_str(params, "to");
    check(&roots, &from)?;
    check(&roots, &to)?;
    if from == to {
        return Err(RpcError::invalid_params("新旧路径相同".to_string()));
    }
    plugin.fs_host_rename(&from, &to).map_err(fs_err)?;
    Ok(json!({ "ok": true, "from": from, "to": to }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_domain_accepts_only_index_roots() {
        assert_eq!(
            site_domain("/www/sites/a.com/index").as_deref(),
            Some("a.com")
        );
        assert_eq!(
            site_domain("/www/sites/a.com/index/sub").as_deref(),
            Some("a.com")
        );
        assert_eq!(site_domain("/www/sites/a.com").as_deref(), None);
        assert_eq!(site_domain("/www/sites/a.com/ssl").as_deref(), None);
        assert_eq!(site_domain("/www").as_deref(), None);
        assert_eq!(site_domain("/www/sites/").as_deref(), None);
        assert_eq!(site_domain("/etc/index").as_deref(), None);
    }

    #[test]
    fn start_dir_prefers_authorized_config() {
        let roots = vec!["/var/www".to_string(), "/etc".to_string()];
        assert_eq!(pick_start("", &roots).as_deref(), Some("/var/www"));
        assert_eq!(pick_start("/etc/", &roots).as_deref(), Some("/etc"));
        // 配了个没授权的目录 → 退回第一个授权根，而不是报错打不开
        assert_eq!(pick_start("/root", &roots).as_deref(), Some("/var/www"));
        assert_eq!(pick_start("", &[]), None);
    }

    #[test]
    fn path_check_uses_granted_roots() {
        let roots = vec!["/var/www".to_string()];
        assert!(check(&roots, "/var/www/html").is_ok());
        assert!(check(&roots, "/var/www").is_ok());
        assert!(check(&roots, "/etc/passwd").is_err());
        assert!(check(&roots, "").is_err());
        assert!(check(&[], "/var/www").is_err());
    }
}
