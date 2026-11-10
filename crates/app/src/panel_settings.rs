//! 面板运行参数（监听地址 / 安全入口前缀 / TLS）的校验与 `config.toml` 落盘。
//!
//! 设置页保存后统一走这里：先校验，再按 key 原位改写配置文件（保留注释与其他字段），
//! 最后校验生成的 TOML 仍可解析。端口/前缀/TLS 都需要重启进程才生效，页面负责提示。

use std::fs;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;

/// 入口前缀长度下限。默认随机前缀是 8 位；6 位在 36 进制下仍有约 21 亿种组合。
pub const MIN_PREFIX_LEN: usize = 6;
/// 入口前缀长度上限：过长会撑坏导航与日志，也没有额外安全收益。
pub const MAX_PREFIX_LEN: usize = 32;

/// 数据库设置键：当前入口前缀。
pub const KEY_ENTRY_PREFIX: &str = "security.entry_prefix";
/// 数据库设置键：上一次入口前缀（重启后兼容窗口内做 GET 重定向）。
pub const KEY_ENTRY_PREFIX_PREVIOUS: &str = "security.entry_prefix_previous";
pub const KEY_ENTRY_PREFIX_PREVIOUS_UNTIL: &str = "security.entry_prefix_previous_until";
/// 入口前缀兼容窗口（秒）。
pub const ENTRY_PREFIX_GRACE_SECS: i64 = 24 * 3600;

/// 顶层保留段：这些名字属于内核固定路由，不能拿来当入口前缀。
const RESERVED_PREFIXES: &[&str] = &[
    "api",
    "healthz",
    "static",
    "assets",
    "favicon.ico",
    "robots.txt",
];

/// 校验并返回规范化后的入口前缀。
///
/// 规则：只允许 ASCII 字母、数字、`-`、`_`；首字符必须是字母或数字；
/// 长度 6–32；不得为 `/`、不得含 `/` 或 `..`；不得占用内核固定顶层路由。
pub fn validate_entry_prefix(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    if v.is_empty() {
        return Err("入口前缀不能为空".into());
    }
    if v == "/" {
        return Err("入口前缀不能是 `/`".into());
    }
    if v.len() < MIN_PREFIX_LEN || v.len() > MAX_PREFIX_LEN {
        return Err(format!(
            "入口前缀长度必须为 {MIN_PREFIX_LEN}–{MAX_PREFIX_LEN} 个字符（当前 {}）",
            v.len()
        ));
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("入口前缀只允许 ASCII 字母、数字、`-`、`_`".into());
    }
    let first = v.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err("入口前缀必须以字母或数字开头".into());
    }
    let lower = v.to_ascii_lowercase();
    if RESERVED_PREFIXES.contains(&lower.as_str()) {
        return Err(format!(
            "入口前缀 `{v}` 与内核固定路由冲突（保留：{}）",
            RESERVED_PREFIXES.join("、")
        ));
    }
    Ok(v.to_string())
}

/// 校验端口号：`u16` 已保证上限，只剩 0 需要拒绝。
pub fn validate_port(port: u16) -> Result<u16, String> {
    if port == 0 {
        return Err("端口必须在 1–65535 之间".into());
    }
    Ok(port)
}

/// 端口占用检测。当前正在监听的同一地址直接放行（不能因为自己占着就拒绝保存）。
///
/// 只做一次短绑定探针，成功即释放；失败的绑定错误原样带出，便于区分「被占用」与
/// 「地址不可用/权限不足」。
pub fn check_port_free(addr: SocketAddr, current: SocketAddr) -> Result<(), String> {
    if addr == current {
        return Ok(());
    }
    match TcpListener::bind(addr) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AddrInUse => Err(format!("端口已被占用: {addr}")),
        Err(e) => Err(format!("无法监听 {addr}: {e}")),
    }
}

/// 把字符串渲染成 TOML 基本字符串（转义反斜杠与引号）。
pub fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 按 key 原位更新 `config.toml`；不存在的 key 追加到文件末尾。
///
/// - 保留注释与其他字段，只替换**生效态**的 `key = ...` 行；
/// - 同一 key 的重复生效行只保留第一条（避免生成重复键）；
/// - 值为 `None` 表示**删除**该生效行（用于把 `tls_enabled` 恢复成「自动推断」）；
/// - 写盘前用 toml 解析器复验，语法非法则不落盘；
/// - 先写临时文件再 rename，避免半截配置。
pub fn write_config_updates(path: &Path, updates: &[(&str, Option<String>)]) -> Result<(), String> {
    let mut text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读取 {} 失败: {e}", path.display())),
    };
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        text.lines().map(str::to_string).collect()
    };

    for (key, value) in updates {
        let mut replaced = false;
        let mut kept = Vec::with_capacity(lines.len() + 1);
        for line in lines {
            if is_active_assignment(&line, key) {
                if !replaced {
                    if let Some(v) = value {
                        kept.push(format!("{key} = {v}"));
                    }
                    replaced = true;
                }
                // 重复的生效键直接丢弃，TOML 不允许重复定义
            } else {
                kept.push(line);
            }
        }
        if !replaced {
            if let Some(v) = value {
                kept.push(format!("{key} = {v}"));
            }
        }
        lines = kept;
    }

    text = lines.join("\n");
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    toml::from_str::<toml::Table>(&text)
        .map_err(|e| format!("生成的 config.toml 非法，已放弃写入: {e}"))?;

    // 唯一临时名 + rename：并发保存设置时互不踩临时文件。
    crate::atomic::write_atomic(path, text.as_bytes(), 0o644)
        .map_err(|e| format!("替换 {} 失败: {e}", path.display()))?;
    Ok(())
}

fn is_active_assignment(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    match trimmed.strip_prefix(key) {
        Some(rest) => rest.trim_start().starts_with('='),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_accepts_safe_values() {
        assert_eq!(validate_entry_prefix("abc12345").unwrap(), "abc12345");
        assert_eq!(validate_entry_prefix("A-b_c09").unwrap(), "A-b_c09");
        assert_eq!(validate_entry_prefix("  pad12345  ").unwrap(), "pad12345");
    }

    #[test]
    fn prefix_rejects_unsafe_values() {
        for bad in [
            "",
            "/",
            "abc",
            "this-is-a-very-long-prefix-over-32-chars",
            "../etc",
            "a/b",
            "abc 123",
            "abc.123",
            "前缀",
            "-abc123",
            "_abc123",
        ] {
            assert!(
                validate_entry_prefix(bad).is_err(),
                "应拒绝非法前缀: {bad:?}"
            );
        }
        for reserved in ["healthz", "static", "assets"] {
            assert!(
                validate_entry_prefix(reserved).is_err(),
                "保留段应拒绝: {reserved}"
            );
        }
        // 只是以保留段开头不算冲突
        assert!(validate_entry_prefix("api123").is_ok());
    }

    #[test]
    fn port_rejects_zero() {
        assert!(validate_port(0).is_err());
        assert_eq!(validate_port(18080).unwrap(), 18080);
    }

    #[test]
    fn occupied_port_is_detected() {
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();
        let current: SocketAddr = "127.0.0.1:1".parse().unwrap();
        assert!(check_port_free(addr, current).is_err());
        // 当前监听地址本身应放行（保存时不改端口的情况）
        assert!(check_port_free(addr, addr).is_ok());
        drop(held);
        assert!(check_port_free(addr, current).is_ok());
    }

    #[test]
    fn config_update_preserves_other_fields_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "# 注释应保留\nlisten = \"127.0.0.1:18080\"\n# tls_enabled = true\ndata_dir = \"/www/skiff\"\n",
        )
        .unwrap();

        write_config_updates(
            &path,
            &[
                ("listen", Some(toml_string("0.0.0.0:18443"))),
                ("entry_prefix", Some(toml_string("abc12345"))),
                ("tls_enabled", Some("false".into())),
            ],
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 注释应保留"));
        assert!(text.contains("listen = \"0.0.0.0:18443\""));
        assert!(text.contains("# tls_enabled = true"));
        assert!(text.contains("entry_prefix = \"abc12345\""));
        assert!(text.contains("tls_enabled = false"));
        assert!(text.contains("data_dir = \"/www/skiff\""));
        // 生效态 listen 只应出现一次
        assert_eq!(text.matches("listen = ").count(), 1);
        text.parse::<toml::Table>().expect("结果必须是合法 TOML");
    }

    #[test]
    fn config_update_collapses_duplicate_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "listen = \"a:1\"\nlisten = \"b:2\"\n").unwrap();
        write_config_updates(&path, &[("listen", Some(toml_string("c:3")))]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("listen = ").count(), 1);
        assert!(text.contains("listen = \"c:3\""));
    }

    #[test]
    fn config_update_removes_key_when_value_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "listen = \"127.0.0.1:18080\"\ntls_enabled = false\n").unwrap();
        write_config_updates(&path, &[("tls_enabled", None)]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("tls_enabled"));
        assert!(text.contains("listen = \"127.0.0.1:18080\""));
    }

    #[test]
    fn creates_missing_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        assert!(
            write_config_updates(&path, &[("listen", Some(toml_string("127.0.0.1:18080")))])
                .is_err(),
            "父目录不存在时应返回可读错误"
        );
        // 父目录存在时正常创建
        let path = dir.path().join("config.toml");
        write_config_updates(&path, &[("listen", Some(toml_string("127.0.0.1:18080")))]).unwrap();
        assert!(path.exists());
    }
}
