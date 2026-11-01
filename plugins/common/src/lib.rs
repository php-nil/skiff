//! 官方插件共用的小工具：配置默认值、参数取值、状态文件、错误转换。
//!
//! 只放**与业务无关**的胶水代码；每个插件的能力调用、策略判定仍各写各的，
//! 免得抽象层把「谁调了什么能力」藏起来（审计要看得见）。

use std::collections::BTreeMap;

use serde_json::{json, Value};
use skiff_sdk::{Plugin, RpcError, SdkError};

/// SDK 错误 → JSON-RPC 错误（插件侧没有更细的错误码可用）。
pub fn rpc_err(e: SdkError) -> RpcError {
    RpcError::internal(e.to_string())
}

pub fn param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

pub fn param_str(params: &Value, key: &str) -> String {
    param(params, key).unwrap_or("").to_string()
}

pub fn param_u64(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| v.as_u64())
}

/// 内核代管的配置（`config_schema`）一次性取回；读不到就用默认值。
pub type Config = BTreeMap<String, String>;

pub fn load_config(plugin: &Plugin) -> Config {
    plugin.config().unwrap_or_default()
}

pub fn cfg_str(cfg: &Config, key: &str, default: &str) -> String {
    cfg.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

pub fn cfg_u64(cfg: &Config, key: &str, default: u64) -> u64 {
    cfg.get(key)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

pub fn cfg_bool(cfg: &Config, key: &str, default: bool) -> bool {
    match cfg.get(key).map(|s| s.trim().to_ascii_lowercase()) {
        Some(v) => matches!(v.as_str(), "1" | "true" | "on" | "yes"),
        None => default,
    }
}

/// 逗号/空白分隔的清单（容器名、路径等）。空串得到空数组。
pub fn csv(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// 内核 docker 回调把结果包在 `{"result": …}` 里（见 `kernel::docker_call`），
/// 插件读列表时要先脱一层；没有这层包装时原样返回。
pub fn unwrap_result(v: Value) -> Value {
    v.get("result").cloned().unwrap_or(v)
}

/// 路径是否落在某个授权根内（与内核 `skiff_model::path` 同源）。
///
/// **这是字符串级快筛，不是授权判定**：内核侧会重新 canonicalize 后再比对
/// （`skiff_plugin::fsgrant`），软链逃逸由内核挡住。插件侧只做「明显越界就早点
/// 报错」，省一次 IPC。
pub use skiff_model::within;
pub use skiff_model::within_any;

/// 目录根统一去掉尾斜杠（`/` 自身除外），避免拼出 `//`。
pub use skiff_model::normalize_dir;

/// 上一级目录；已在授权根上（或父级越过根）时返回 `None`。
pub use skiff_model::parent_within;

/// 路径是否含 `..` **段**（组件级，不是子串）。
pub use skiff_model::has_parent_segment;

/// 是否是 POSIX 绝对路径（`/` 开头）。容器内路径、宿主 Linux 路径用它。
pub use skiff_model::is_posix_absolute;

pub fn join(root: &str, file: &str) -> String {
    if root.ends_with('/') {
        format!("{root}{file}")
    } else {
        format!("{root}/{file}")
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 读自身 `data/` 下的 JSON 状态文件；不存在或解析失败返回空对象。
pub fn read_state(plugin: &Plugin, name: &str) -> Value {
    std::fs::read_to_string(plugin.data_dir.join(name))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// 写自身 `data/` 下的 JSON 状态文件（先写临时文件再替换，避免写一半的状态）。
pub fn write_state(plugin: &Plugin, name: &str, value: &Value) -> Result<(), SdkError> {
    std::fs::create_dir_all(&plugin.data_dir)?;
    let target = plugin.data_dir.join(name);
    let tmp = plugin.data_dir.join(format!(".{name}.tmp"));
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| SdkError::Protocol(format!("状态序列化失败: {e}")))?;
    std::fs::write(&tmp, text)?;
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fake_plugin(data_dir: PathBuf) -> Plugin {
        Plugin {
            id: "com.skiff.test".into(),
            api: "1.0".into(),
            socket: PathBuf::from("/tmp/x.sock"),
            data_dir,
            cache_dir: PathBuf::from("/tmp/c"),
            run_dir: PathBuf::from("/tmp/r"),
            kernel_socket: PathBuf::from("/tmp/k.sock"),
            granted: vec![],
            token: "t".into(),
        }
    }

    #[test]
    fn unwrap_docker_result_envelope() {
        assert_eq!(
            unwrap_result(json!({"result": [{"id": "a"}]})),
            json!([{"id": "a"}])
        );
        // 没有包装时原样返回（例如内核后续版本改成直接返回）
        assert_eq!(unwrap_result(json!([1, 2])), json!([1, 2]));
    }

    #[test]
    fn granted_roots_bound_paths() {
        let roots = vec!["/etc/nginx/conf.d".to_string(), "/var/www".to_string()];
        assert!(within_any("/etc/nginx/conf.d", &roots));
        assert!(within_any("/var/www/html", &roots));
        assert!(!within_any("/etc/nginx/nginx.conf", &roots));
        assert!(!within_any("/etc/nginx/conf.d-evil", &roots));
        assert!(within("/x", "/"));
        assert!(!within_any("/etc", &[]));
    }

    /// 路径代数本体在 `skiff_model::path`（内核与插件同源）；这里只验证转出可用。
    #[test]
    fn path_helpers_are_shared_with_the_kernel() {
        assert_eq!(normalize_dir("/etc/nginx/conf.d/"), "/etc/nginx/conf.d");
        assert_eq!(normalize_dir("/"), "/");
        assert_eq!(
            join("/etc/nginx/conf.d", "a.conf"),
            "/etc/nginx/conf.d/a.conf"
        );
        assert_eq!(join("/", "a.conf"), "/a.conf");
        assert!(!join("/", "a.conf").starts_with("//"));
    }

    #[test]
    fn parent_stops_at_the_granted_root() {
        let roots = vec!["/var/www".to_string()];
        assert_eq!(
            parent_within("/var/www/html", &roots).as_deref(),
            Some("/var/www")
        );
        assert_eq!(parent_within("/var/www", &roots), None);
        assert_eq!(
            parent_within("/var/www/html/css", &roots).as_deref(),
            Some("/var/www/html")
        );
        assert_eq!(parent_within("/etc", &roots), None);
    }

    #[test]
    fn config_helpers_fall_back_to_defaults() {
        let mut cfg = Config::new();
        cfg.insert("n".into(), " 42 ".into());
        cfg.insert("b".into(), "true".into());
        cfg.insert("empty".into(), "   ".into());
        assert_eq!(cfg_u64(&cfg, "n", 7), 42);
        assert_eq!(cfg_u64(&cfg, "missing", 7), 7);
        assert!(cfg_bool(&cfg, "b", false));
        assert!(cfg_bool(&cfg, "missing", true));
        assert_eq!(
            cfg_str(&cfg, "empty", "/etc/nginx/conf.d"),
            "/etc/nginx/conf.d"
        );
    }

    #[test]
    fn csv_splits_on_commas_and_whitespace() {
        assert_eq!(csv("a, b ,c"), vec!["a", "b", "c"]);
        assert_eq!(csv("  "), Vec::<String>::new());
        assert_eq!(csv("one\n two"), vec!["one", "two"]);
    }

    #[test]
    fn state_roundtrip_and_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = fake_plugin(tmp.path().to_path_buf());
        assert_eq!(read_state(&p, "state.json"), json!({}));
        let v = json!({"restarts": 3, "events": [{"t": 1}]});
        write_state(&p, "state.json", &v).unwrap();
        assert_eq!(read_state(&p, "state.json"), v);
        // 覆盖写不会残留旧内容
        write_state(&p, "state.json", &json!({"restarts": 4})).unwrap();
        assert_eq!(read_state(&p, "state.json")["restarts"], 4);
    }
}
