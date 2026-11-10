//! 面板运行参数（监听地址 / 安全入口前缀 / TLS）的校验与 `config.toml` 落盘。
//!
//! 设置页保存后统一走这里：先校验，再按 key 原位改写配置文件（保留注释与其他字段），
//! 最后用真实配置结构做语义复验。端口/前缀/TLS 都需要重启进程才生效，页面负责提示。

use std::fs;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 配置文件形态（`/etc/skiff/config.toml`）。字段全部可选，缺失走默认。
///
/// 这是文件结构的**唯一来源**：`skiff`（加载端）与写入端共用同一份定义，
/// 写入端落盘前直接反序列化它做语义复验。`deny_unknown_fields` 是最后一道闸——
/// 语法合法但塞进错误表里的键会在这一步被拒绝，不会留到下次启动才 crash-loop。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub listen: Option<String>,
    pub data_dir: Option<PathBuf>,
    /// 站点/nginx/日志/证书/PHP socket 根（默认 `/www`）
    pub web_root: Option<PathBuf>,
    pub run_dir: Option<PathBuf>,
    pub docker_socket: Option<String>,
    /// `native`（默认）或 `docker`（受限形态，§5.4）
    pub form: Option<String>,
    /// `basic`（M2）或 `lifecycle`（M3，默认）
    pub docker_tier: Option<String>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// `true` 启用 TLS；`false` 直接明文监听（Cookie 降级）；缺省走旧行为自动推断。
    pub tls_enabled: Option<bool>,
    pub allow_insecure_http: Option<bool>,
    /// 安全入口前缀；缺省时沿用数据库里首启随机生成的值。
    pub entry_prefix: Option<String>,
    pub allow_unsigned: Option<bool>,
    pub allow_reduced_sandbox: Option<bool>,
    pub trusted_proxies: Option<Vec<String>>,
    /// 绑定挂载授权根（native 形态，§5.5）
    pub authorized_roots: Option<Vec<PathBuf>>,
    pub external_url: Option<String>,
    pub login_concurrency: Option<usize>,
    /// 回收站保留天数（默认 30，范围 1–365）：到期由内置任务 `trash-prune` 彻底删除。
    pub trash_retention_days: Option<u32>,
    /// 首启管理员是否强制首次登录改密。默认 false（初始口令可直接使用）。
    pub require_initial_password_change: Option<bool>,
    /// 单个 PHP 扩展安装/卸载一次 `docker exec` 的硬超时（秒，默认 900）。
    pub ext_exec_timeout_secs: Option<u64>,
    /// 批量扩展脚本的硬超时（秒，默认 1800）。
    pub ext_batch_timeout_secs: Option<u64>,
    /// 商店目录（Skiff-server）只读接入；缺省整段关闭。
    pub catalog: Option<CatalogFile>,
}

/// `[catalog]` 段：默认关闭，不拉取、不显示商店入口。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    pub enabled: Option<bool>,
    /// `stable` 或 `night`。
    pub channel: Option<String>,
    /// 镜像基地址，按序回退；必须是 https。
    pub mirrors: Option<Vec<String>>,
    /// 固定 Ed25519 公钥（64 位十六进制）。
    pub public_key: Option<String>,
    /// 仅开发模式允许接受未签名渠道。
    pub allow_unsigned: Option<bool>,
    pub timeout_secs: Option<u64>,
}

/// 可写键 → (所属表, 段内键名)。`None` 表示顶层。
///
/// 键名必须与 [`FileConfig`] / [`CatalogFile`] 的字段一一对应：写错会在落盘前的
/// 语义复验中被 `deny_unknown_fields` 拒绝，不会产生「语法合法、启动失败」的文件。
const WRITABLE_KEYS: &[(&str, Option<&str>, &str)] = &[
    // ---- 顶层 ----
    ("listen", None, "listen"),
    ("data_dir", None, "data_dir"),
    ("web_root", None, "web_root"),
    ("run_dir", None, "run_dir"),
    ("docker_socket", None, "docker_socket"),
    ("form", None, "form"),
    ("docker_tier", None, "docker_tier"),
    ("tls_cert", None, "tls_cert"),
    ("tls_key", None, "tls_key"),
    ("tls_enabled", None, "tls_enabled"),
    ("allow_insecure_http", None, "allow_insecure_http"),
    ("entry_prefix", None, "entry_prefix"),
    ("allow_unsigned", None, "allow_unsigned"),
    ("allow_reduced_sandbox", None, "allow_reduced_sandbox"),
    ("trusted_proxies", None, "trusted_proxies"),
    ("authorized_roots", None, "authorized_roots"),
    ("external_url", None, "external_url"),
    ("login_concurrency", None, "login_concurrency"),
    ("trash_retention_days", None, "trash_retention_days"),
    (
        "require_initial_password_change",
        None,
        "require_initial_password_change",
    ),
    ("ext_exec_timeout_secs", None, "ext_exec_timeout_secs"),
    ("ext_batch_timeout_secs", None, "ext_batch_timeout_secs"),
    // ---- [catalog] ----
    ("catalog.enabled", Some("catalog"), "enabled"),
    ("catalog.channel", Some("catalog"), "channel"),
    ("catalog.mirrors", Some("catalog"), "mirrors"),
    ("catalog.public_key", Some("catalog"), "public_key"),
    ("catalog.allow_unsigned", Some("catalog"), "allow_unsigned"),
    ("catalog.timeout_secs", Some("catalog"), "timeout_secs"),
];

fn writable_key(key: &str) -> Option<(Option<&'static str>, &'static str)> {
    WRITABLE_KEYS
        .iter()
        .find(|(name, _, _)| *name == key)
        .map(|(_, section, leaf)| (*section, *leaf))
}

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

/// 按 key 原位更新 `config.toml`；不存在的 key 写入**其所属表**，不是一律追加到文件末尾。
///
/// - 键必须落在正确的表里：顶层键写顶层，`catalog.*` 写进 `[catalog]`。追加到末尾
///   会让新键被并进文件末尾的段（典型是 `[catalog]`），语法合法但结构体
///   `deny_unknown_fields` 解析失败，下次启动直接 crash-loop；
/// - 保留注释与其他字段，只替换**目标表内**的生效态 `key = ...` 行；
/// - 同一 key 的重复生效行只保留第一条（避免生成重复键）；
/// - 值为 `None` 表示**删除**该生效行（用于把 `tls_enabled` 恢复成「自动推断」）；
/// - 写盘前先用 TOML 解析器查语法，再用真实的 [`FileConfig`] 反序列化查语义；
///   任何一步失败都不落盘，原文件字节保持不变；
/// - 先写临时文件再 rename，避免半截配置。
pub fn write_config_updates(path: &Path, updates: &[(&str, Option<String>)]) -> Result<(), String> {
    // 先确认每个键都能映射到真实字段：未知键直接拒绝，不猜它该去哪个段。
    let mut plan = Vec::with_capacity(updates.len());
    for (key, value) in updates {
        let (section, leaf) = writable_key(key)
            .ok_or_else(|| format!("不支持的配置键 `{key}`：不在面板可写白名单内"))?;
        plan.push((section, leaf, value.as_deref()));
    }

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

    for (section, leaf, value) in plan {
        apply_update(&mut lines, section, leaf, value);
    }

    text = lines.join("\n");
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    toml::from_str::<toml::Table>(&text)
        .map_err(|e| format!("生成的 config.toml 语法非法，已放弃写入: {e}"))?;
    // 语法合法不代表语义合法：用真实结构体复验，段错位/类型不匹配会在这里失败。
    toml::from_str::<FileConfig>(&text).map_err(|e| {
        format!("生成的 config.toml 语义非法（按真实配置结构复验失败），已放弃写入: {e}")
    })?;

    // 唯一临时名 + rename：并发保存设置时互不踩临时文件。
    crate::atomic::write_atomic(path, text.as_bytes(), 0o644)
        .map_err(|e| format!("替换 {} 失败: {e}", path.display()))?;
    Ok(())
}

/// 把一条更新写进目标表：替换/删除表内的生效行，缺失时按归属插入。
///
/// 顶层键必须插在第一个表头**之前**；直接 `push` 到末尾会被 TOML 归入最后一张表。
fn apply_update(
    lines: &mut Vec<String>,
    section: Option<&str>,
    leaf: &str,
    rendered: Option<&str>,
) {
    let first_header = lines.iter().position(|line| table_header(line).is_some());
    let header_idx = section.and_then(|t| {
        lines
            .iter()
            .position(|line| table_header(line).as_deref() == Some(t))
    });
    // 目标表内可写区间；表不存在时区间为空，稍后在末尾新建。
    let (start, end) = match section {
        None => (0, first_header.unwrap_or(lines.len())),
        Some(_) => match header_idx {
            Some(h) => {
                let next = lines[h + 1..]
                    .iter()
                    .position(|line| table_header(line).is_some())
                    .map(|off| h + 1 + off)
                    .unwrap_or(lines.len());
                (h + 1, next)
            }
            None => (lines.len(), lines.len()),
        },
    };

    let mut replaced = false;
    let mut kept = Vec::with_capacity(lines.len() + 1);
    for (i, line) in lines.iter().enumerate() {
        if (start..end).contains(&i) && is_active_assignment(line, leaf) {
            if !replaced {
                if let Some(v) = rendered {
                    kept.push(format!("{leaf} = {v}"));
                }
                replaced = true;
            }
            // 重复的生效键直接丢弃，TOML 不允许重复定义
            continue;
        }
        kept.push(line.clone());
    }

    if !replaced {
        if let Some(v) = rendered {
            let line = format!("{leaf} = {v}");
            match section {
                // 顶层：插在第一个表头之前（没有表头时就是文件末尾）。
                None => kept.insert(start, line),
                Some(_) if header_idx.is_some() => kept.insert(end, line),
                // 目标段不存在：新建表头，避免把键写到别人的段里。
                Some(t) => {
                    if kept.last().is_some_and(|l| !l.trim().is_empty()) {
                        kept.push(String::new());
                    }
                    kept.push(format!("[{t}]"));
                    kept.push(line);
                }
            }
        }
    }
    *lines = kept;
}

/// 解析表头行，返回表名；`[[array]]` 也按 `array` 处理（当前配置没有数组表，
/// 但绝不能把它误当成顶层内容）。行尾注释不影响解析。
fn table_header(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = if let Some(r) = trimmed.strip_prefix("[[") {
        r.split_once("]]").map(|(name, _)| name)
    } else if let Some(r) = trimmed.strip_prefix('[') {
        r.split_once(']').map(|(name, _)| name)
    } else {
        None
    }?;
    let name = rest.trim();
    (!name.is_empty()).then(|| name.to_string())
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

    /// 每个可写键的合法取值，用于覆盖全量白名单。
    fn sample_value(key: &str) -> String {
        match key {
            "listen" => toml_string("127.0.0.1:18080"),
            "data_dir" => toml_string("/www/skiff"),
            "web_root" => toml_string("/www"),
            "run_dir" => toml_string("/run/skiff"),
            "docker_socket" => toml_string("/var/run/docker.sock"),
            "form" => toml_string("native"),
            "docker_tier" => toml_string("lifecycle"),
            "tls_cert" => toml_string("/www/skiff/cert.pem"),
            "tls_key" => toml_string("/www/skiff/key.pem"),
            "tls_enabled" => "true".into(),
            "allow_insecure_http" => "false".into(),
            "entry_prefix" => toml_string("abc12345"),
            "allow_unsigned" => "false".into(),
            "allow_reduced_sandbox" => "false".into(),
            "trusted_proxies" => "[\"10.0.0.0/8\"]".into(),
            "authorized_roots" => "[\"/www\"]".into(),
            "external_url" => toml_string("https://panel.example.com"),
            "login_concurrency" => "2".into(),
            "trash_retention_days" => "30".into(),
            "require_initial_password_change" => "false".into(),
            "ext_exec_timeout_secs" => "900".into(),
            "ext_batch_timeout_secs" => "1800".into(),
            "catalog.enabled" => "false".into(),
            "catalog.channel" => toml_string("stable"),
            "catalog.mirrors" => "[\"https://cdn.example/skiff\"]".into(),
            "catalog.public_key" => toml_string(&"ab".repeat(32)),
            "catalog.allow_unsigned" => "false".into(),
            "catalog.timeout_secs" => "30".into(),
            other => panic!("可写键 `{other}` 缺少测试样例值"),
        }
    }

    /// 文件末尾是 `[catalog]` 时写顶层键：新键必须插在表头之前，不能被吞进 `[catalog]`。
    #[test]
    fn top_level_key_is_not_swallowed_by_trailing_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "listen = \"127.0.0.1:18080\"\n\n[catalog]\nchannel = \"stable\"\n",
        )
        .unwrap();

        write_config_updates(&path, &[("entry_prefix", Some(toml_string("abc12345")))]).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let table: toml::Table = text.parse().unwrap();
        let catalog = table
            .get("catalog")
            .and_then(|v| v.as_table())
            .expect("[catalog] 段必须保留");
        assert_eq!(catalog.get("channel").unwrap().as_str(), Some("stable"));
        assert!(
            catalog.get("entry_prefix").is_none(),
            "entry_prefix 不能被并入 [catalog]:\n{text}"
        );
        assert_eq!(
            table.get("entry_prefix").unwrap().as_str(),
            Some("abc12345")
        );
        // 顶层键必须出现在第一个表头之前
        let prefix_at = text.find("entry_prefix").unwrap();
        let header_at = text.find("[catalog]").unwrap();
        assert!(prefix_at < header_at, "顶层键写到了段内:\n{text}");
        // 语义复验等价：真实文件结构必须能解析
        toml::from_str::<FileConfig>(&text).expect("写入结果必须能按真实结构解析");
    }

    /// 全量可写键逐个写入：必须落在正确的表里，且真实结构反序列化成功。
    #[test]
    fn every_writable_key_lands_in_its_table_and_parses() {
        for (key, section, leaf) in WRITABLE_KEYS {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            // 末尾放一个段：如果实现退化成「追加到末尾」，这里一定会暴露。
            fs::write(&path, "[catalog]\nchannel = \"stable\"\n").unwrap();

            write_config_updates(&path, &[(key, Some(sample_value(key)))])
                .unwrap_or_else(|e| panic!("写入 `{key}` 失败: {e}"));

            let text = fs::read_to_string(&path).unwrap();
            let table: toml::Table = text.parse().unwrap();
            match section {
                None => {
                    let catalog = table
                        .get("catalog")
                        .and_then(|v| v.as_table())
                        .expect("[catalog] 段必须保留");
                    assert!(
                        catalog.get(*leaf).is_none(),
                        "顶层键 `{key}` 被写进了 [catalog]:\n{text}"
                    );
                    assert!(table.get(*leaf).is_some(), "顶层键 `{key}` 未写入:\n{text}");
                }
                Some(t) => {
                    let section_table = table
                        .get(*t)
                        .and_then(|v| v.as_table())
                        .unwrap_or_else(|| panic!("段 [{t}] 缺失:\n{text}"));
                    assert!(
                        section_table.get(*leaf).is_some(),
                        "`{key}` 未落在 [{t}]:\n{text}"
                    );
                    assert!(
                        table.get(*leaf).is_none(),
                        "`{key}` 不应出现在顶层:\n{text}"
                    );
                }
            }
            toml::from_str::<FileConfig>(&text)
                .unwrap_or_else(|e| panic!("写入 `{key}` 后真实结构解析失败: {e}\n{text}"));
        }
    }

    /// 修改已有段内键：其他段内容与顺序不变，注释保留。
    #[test]
    fn in_place_update_keeps_other_sections_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "# 顶部注释\nlisten = \"127.0.0.1:18080\"\n\n[catalog]\n# 商店配置\nenabled = true\nchannel = \"stable\"\n",
        )
        .unwrap();

        write_config_updates(
            &path,
            &[
                ("listen", Some(toml_string("0.0.0.0:18443"))),
                ("catalog.channel", Some(toml_string("night"))),
            ],
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 顶部注释"));
        assert!(text.contains("# 商店配置"));
        assert!(text.contains("enabled = true"), "同段其他键不能动:\n{text}");
        assert!(text.contains("channel = \"night\""));
        assert_eq!(
            text.matches("channel = ").count(),
            1,
            "不能产生重复键:\n{text}"
        );
        // 段与其内部键的顺序必须保持：底层 rewrite 不能把段挪走
        let listen_at = text.find("listen = ").unwrap();
        let header_at = text.find("[catalog]").unwrap();
        let enabled_at = text.find("enabled = ").unwrap();
        let channel_at = text.find("channel = ").unwrap();
        assert!(listen_at < header_at, "顶层键被挪到段后:\n{text}");
        assert!(
            header_at < enabled_at && enabled_at < channel_at,
            "段内顺序被打乱:\n{text}"
        );
        toml::from_str::<FileConfig>(&text).unwrap();
    }

    /// 写入失败（未知键 / 语法非法 / 语义非法）时原文件字节不变。
    #[test]
    fn failed_write_leaves_original_bytes_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original =
            "# 原文件\nlisten = \"127.0.0.1:18080\"\n\n[catalog]\nchannel = \"stable\"\n";
        fs::write(&path, original).unwrap();

        let cases: Vec<Vec<(&str, Option<String>)>> = vec![
            // 未知键：不在白名单，直接拒绝
            vec![("not_a_key", Some(toml_string("x")))],
            // 语法非法：字符串未闭合
            vec![("listen", Some("\"0.0.0.0:18443".to_string()))],
            // 语法合法但语义非法：listen 必须是字符串
            vec![("listen", Some("18443".to_string()))],
        ];
        for updates in cases {
            let err = write_config_updates(&path, &updates).unwrap_err();
            assert!(!err.is_empty());
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                original,
                "失败时不得改动原文件: {updates:?}"
            );
        }
    }
}
