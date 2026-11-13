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
        "roots" | "config" => Ok(decorate_config(config(&p))),
        "list" => list(&p, &params),
        "read" => read(&p, &params),
        // 新壳原生编辑器：整段文本（≤256KiB）+ UTF-8 校验，schema 里不做 base64。
        "read_text" => read_text(&p, &params),
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

/// 新建/浏览页的下拉：标题优先用站点域名，非标准根退回末级目录名。
fn decorate_config(mut cfg: Value) -> Value {
    let roots = cfg
        .get("roots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let options: Vec<Value> = roots
        .iter()
        .filter_map(Value::as_str)
        .map(|root| {
            let title = site_domain(&common::normalize_dir(root))
                .unwrap_or_else(|| root.rsplit('/').next().unwrap_or(root).to_string());
            json!({ "title": title, "value": root })
        })
        .collect();
    let options_empty = options.is_empty();
    let Some(obj) = cfg.as_object_mut() else {
        return cfg;
    };
    obj.insert("root_options".into(), json!(options));
    obj.insert("roots_empty".into(), json!(options_empty));
    cfg
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
    // 授权根缺失/形态不对时不在 RPC 层报错：页面要能展示「去安装确认页加授权」的空态。
    let roots = match granted_roots(plugin) {
        Ok(roots) => roots,
        Err(e) => {
            return Ok(json!({
                "ok": false,
                "path": "",
                "parent": null,
                "roots": [],
                "items": [],
                "entries": [],
                "dirs": [],
                "files": [],
                "error": e.message,
            }))
        }
    };
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
    let items = v.get("items").cloned().unwrap_or(json!([]));
    let (dirs, files) = split_entries(&items, &dir);
    Ok(json!({
        "ok": true,
        "path": dir,
        "parent": common::parent_within(&dir, &roots),
        "roots": roots,
        "items": items,
        "entries": v.get("entries").cloned().unwrap_or(json!([])),
        "dirs": dirs,
        "files": files,
    }))
}

/// 目录表与文件表分开：budding 1.8.0 的 `rowActions` 不能按行隐藏，
/// 拆表后「进入」只出现在目录、「编辑」只出现在文件。
fn split_entries(items: &Value, cwd: &str) -> (Vec<Value>, Vec<Value>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let base = cwd.trim_end_matches('/');
    if let Some(rows) = items.as_array() {
        for item in rows {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let is_dir = item.get("dir").and_then(Value::as_bool).unwrap_or(false);
            let size = item.get("size").and_then(Value::as_u64).unwrap_or(0);
            let mtime = item.get("mtime").and_then(Value::as_u64).unwrap_or(0);
            let mut row = item.clone();
            row["path"] = json!(format!("{base}/{name}"));
            row["kind_text"] = json!(if is_dir { "目录" } else { "文件" });
            row["size_text"] = json!(if is_dir {
                "—".to_string()
            } else {
                size_text(size)
            });
            row["mtime_text"] = json!(ago_text(mtime));
            if is_dir {
                dirs.push(row);
            } else {
                files.push(row);
            }
        }
    }
    (dirs, files)
}

fn size_text(size: u64) -> String {
    const KB: f64 = 1024.0;
    let n = size as f64;
    if n < KB {
        format!("{size} B")
    } else if n < KB * KB {
        format!("{:.1} KiB", n / KB)
    } else if n < KB * KB * KB {
        format!("{:.2} MiB", n / (KB * KB))
    } else {
        format!("{:.2} GiB", n / (KB * KB * KB))
    }
}

fn ago_text(ts: u64) -> String {
    if ts == 0 {
        return "—".into();
    }
    let now = common::now_secs();
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

/// 原生编辑器的只读文本：只取第一页（256KiB），二进制/非 UTF-8 直接拒绝。
///
/// `text_safe` 把每个 `${` 改写成 `${'${'}`：budding 会把 props 字符串里的 `${...}`
/// 当模板求值，而站点里的 shell/JS 文件经常含 `${VAR}`。改写后渲染结果与原文一致，
/// 保存时 textarea 的值就是原文，不需要反向解码。
fn read_text(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let path = common::param_str(params, "path");
    if path.trim().is_empty() {
        return Ok(json!({
            "ok": true, "path": "", "text": "", "text_safe": "",
            "read_only": true, "can_save": false, "error": "", "empty": true, "open": false,
        }));
    }
    let roots = granted_roots(plugin)?;
    check(&roots, &path)?;
    let (b64, _off, size, eof) = plugin
        .fs_host_read_b64(&path, 0, MAX_PAGE)
        .map_err(fs_err)?;
    let bytes = b64_decode(&b64)?;
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(json!({
            "ok": false,
            "path": path,
            "text": "",
            "text_safe": "",
            "size": size,
            "size_text": size_text(size),
            "read_only": true,
            "can_save": false,
            "error": "文件不是 UTF-8 文本，原生编辑器不支持；请用内核「工具 → 文件管理」下载或 SSH。",
            "empty": false,
            "open": true,
        }));
    };
    let truncated = !eof;
    let note = if truncated {
        "文件超过 256 KiB，只显示第一页且不可保存；请用内核「工具 → 文件管理」处理。"
    } else {
        ""
    };
    Ok(json!({
        "ok": true,
        "path": path,
        "text": text,
        "text_safe": escape_template(&text),
        "size": size,
        "size_text": size_text(size),
        "read_only": truncated,
        "can_save": !truncated,
        "error": note,
        "empty": false,
        "open": true,
    }))
}

/// 只改写 `${`，不碰其它字符；渲染后与原文一致。
fn escape_template(text: &str) -> String {
    text.replace("${", "${'${'}")
}

/// 宿主返回标准 base64；这里只做解码，不校验多余填充位。
fn b64_decode(text: &str) -> Result<Vec<u8>, RpcError> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in text.as_bytes() {
        if c == b'=' {
            break;
        }
        if c == b'\n' || c == b'\r' {
            continue;
        }
        let v =
            val(c).ok_or_else(|| RpcError::internal("宿主返回的 base64 数据非法".to_string()))?;
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
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

    #[test]
    fn base64_decodes_host_payloads() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("5L2g5aW9").unwrap(), "你好".as_bytes());
        assert_eq!(b64_decode("").unwrap(), Vec::<u8>::new());
        assert!(b64_decode("!!").is_err());
    }

    #[test]
    fn template_escape_only_touches_dollar_brace() {
        assert_eq!(escape_template("plain"), "plain");
        assert_eq!(escape_template("echo ${HOME}"), "echo ${'${'}HOME}");
        assert_eq!(escape_template("a${b}c${d}"), "a${'${'}b}c${'${'}d}");
        // 已存在的转义串也按同一规则改写，渲染后仍是原文。
        assert_eq!(escape_template("${'${'}"), "${'${'}'${'${'}'}");
    }

    #[test]
    fn sizes_and_entry_split() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(2048), "2.0 KiB");
        assert_eq!(size_text(3 * 1024 * 1024), "3.00 MiB");

        let (dirs, files) = split_entries(
            &json!([
                { "name": "conf", "dir": true, "size": 0, "mtime": 0 },
                { "name": "index.php", "dir": false, "size": 2048, "mtime": 0 },
            ]),
            "/www/sites/a.com/index/",
        );
        assert_eq!(dirs.len(), 1);
        assert_eq!(files.len(), 1);
        assert_eq!(dirs[0]["path"], json!("/www/sites/a.com/index/conf"));
        assert_eq!(dirs[0]["size_text"], json!("—"));
        assert_eq!(files[0]["path"], json!("/www/sites/a.com/index/index.php"));
        assert_eq!(files[0]["size_text"], json!("2.0 KiB"));
        assert_eq!(files[0]["kind_text"], json!("文件"));
    }
}
