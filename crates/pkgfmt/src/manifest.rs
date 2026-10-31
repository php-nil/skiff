//! manifest.toml：包标识、能力、权限与运行配置（§4.1 表 1 / 表 2）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{PkgError, Result};

/// 内核并行兼容的插件 API 主版本（§11 决策 4：N-1）。
/// MVP 只发过 API 1，因此支持区间是 1.x；将来升 2 时这里扩成 `[1, 2]`。
pub const SUPPORTED_API_MAJOR: &[u32] = &[1];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Docker,
    Identity,
    Fs,
    Net,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Docker => "docker",
            Capability::Identity => "identity",
            Capability::Fs => "fs",
            Capability::Net => "net",
        }
    }

    /// 全部能力。
    pub fn all() -> &'static [Capability] {
        &[
            Capability::Docker,
            Capability::Identity,
            Capability::Fs,
            Capability::Net,
        ]
    }

    /// 该部署形态下可授予的能力面（§5.1）。
    ///
    /// 这是**唯一**一处形态→能力的映射。之前 CLI 与 Web 各写一份，
    /// 新增形态或能力时必忘一处，而漏掉的那侧会多给或少给能力。
    /// 受限形态（1Panel 商店 Docker 包）只保留 docker/identity：宿主路径与
    /// 出网收口在容器里无法可靠实施。
    pub fn available_in(form: skiff_model::Form) -> Vec<Capability> {
        match form {
            skiff_model::Form::Native => Capability::all().to_vec(),
            skiff_model::Form::DockerRestricted => vec![Capability::Docker, Capability::Identity],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum RunMode {
    #[default]
    OnDemand,
    Resident,
}

/// 插件清单里声明的一条权限。命名刻意不叫 `Permission`：
/// `skiff_model::Permission` 是「角色→动作」的 RBAC 枚举，两者同名会误读。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPerm {
    pub capability: Capability,
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UiEntry {
    pub label: String,
    /// 相对 `ui/` 的页面路径，内核托管为 `/plugins/{id}/ui/{path}`
    pub path: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigField {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
}

impl ConfigField {
    pub fn validate(&self, key: &str) -> Result<()> {
        const TYPES: &[&str] = &["string", "text", "number", "bool", "password", "url"];
        if !TYPES.contains(&self.kind.as_str()) {
            return Err(PkgError::Manifest(format!(
                "config_schema.{key} 的 type 不支持: {}（可选 {}）",
                self.kind,
                TYPES.join("/")
            )));
        }
        Ok(())
    }

    /// 校验一个**具体取值**是否符合声明的 `type`。
    ///
    /// CLI 写入与将来的 UI 表单共用这一处，避免两边各写一套弱校验
    /// （只校验类型名会导致 `number` 项被塞进任意字符串）。
    pub fn check_value(&self, key: &str, value: &str) -> Result<()> {
        let bad = |expected: &str| {
            Err(PkgError::Manifest(format!(
                "配置项 {key} 需要 {expected}，得到 {value:?}"
            )))
        };
        match self.kind.as_str() {
            "number" => {
                if value.trim().parse::<f64>().is_err() {
                    return bad("数字");
                }
            }
            "bool" => {
                if !matches!(value.trim(), "true" | "false") {
                    return bad("true/false");
                }
            }
            "url" => {
                let v = value.trim();
                if !(v.starts_with("http://") || v.starts_with("https://")) || v.len() > 2048 {
                    return bad("http(s) URL");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// SHA-256(raw public key) 的小写 hex
    pub key_id: String,
    pub api: String,
    #[serde(default)]
    pub run: RunMode,
    #[serde(default)]
    pub resident_reason: Option<String>,
    #[serde(default = "default_true")]
    pub requires_full_sandbox: bool,
    #[serde(default)]
    pub requires: Vec<Capability>,
    #[serde(default)]
    pub permissions: Vec<ManifestPerm>,
    #[serde(default)]
    pub network_egress: Vec<String>,
    #[serde(default)]
    pub ui: Vec<UiEntry>,
    #[serde(default = "default_idle")]
    pub idle_timeout_sec: u64,
    #[serde(default = "default_restarts")]
    pub max_restarts: u32,
    #[serde(default)]
    pub config_schema: BTreeMap<String, ConfigField>,
    /// 周期任务：5 段 cron 表达式；到点拉起插件调 `tick`，跑完回收（§4.2）
    #[serde(default)]
    pub cron: Vec<String>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
    #[serde(default)]
    pub pids_limit: Option<u64>,
}

fn default_true() -> bool {
    true
}
fn default_idle() -> u64 {
    300
}
fn default_restarts() -> u32 {
    3
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest> {
        let m: Manifest = toml::from_str(text).map_err(|e| PkgError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        if self.name.trim().is_empty() || self.name.len() > 64 {
            return Err(PkgError::Manifest("name 长度必须 1–64".into()));
        }
        validate_semver(&self.version)?;
        validate_api(&self.api)?;
        validate_hex64(&self.key_id, "key_id")?;

        if self.run == RunMode::Resident
            && self
                .resident_reason
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(PkgError::Manifest(
                "run = \"resident\" 必须写 resident_reason（并计入资源预算）".into(),
            ));
        }

        let requires: BTreeSet<Capability> = self.requires.iter().copied().collect();
        if requires.len() != self.requires.len() {
            return Err(PkgError::Manifest("requires 存在重复能力".into()));
        }
        for p in &self.permissions {
            if !requires.contains(&p.capability) {
                return Err(PkgError::Manifest(format!(
                    "permissions 声明了未在 requires 中的能力 {}（离了它不能用，就该在 requires 里）",
                    p.capability.as_str()
                )));
            }
            if p.verbs.is_empty() {
                return Err(PkgError::Manifest(format!(
                    "{} 的 verbs 不能为空",
                    p.capability.as_str()
                )));
            }
            for v in &p.verbs {
                validate_verb(v)?;
            }
        }

        for e in &self.network_egress {
            validate_egress(e)?;
        }

        for u in &self.ui {
            if u.path.starts_with('/') || u.path.contains("..") || u.path.is_empty() {
                return Err(PkgError::Manifest(format!(
                    "ui.path 必须是 ui/ 下的相对路径: {}",
                    u.path
                )));
            }
        }

        for c in &self.cron {
            validate_cron(c)?;
        }

        for (k, f) in &self.config_schema {
            if k.is_empty()
                || !k
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                return Err(PkgError::Manifest(format!("config_schema 键名非法: {k}")));
            }
            f.validate(k)?;
        }

        if self.idle_timeout_sec < 30 {
            return Err(PkgError::Manifest("idle_timeout_sec 不得小于 30".into()));
        }
        Ok(())
    }

    /// 已声明的权限串（`cap:verb`），安装时逐条向用户确认。
    pub fn permission_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.permissions {
            for v in &p.verbs {
                out.push(format!("{}:{}", p.capability.as_str(), v));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn requires_str(&self) -> Vec<&'static str> {
        self.requires.iter().map(|c| c.as_str()).collect()
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() < 4 || id.len() > 128 {
        return Err(PkgError::Manifest("id 长度必须 4–128".into()));
    }
    if !id.contains('.') {
        return Err(PkgError::Manifest(
            "id 必须是反向域名风格（如 com.example.nginx）".into(),
        ));
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-');
    if !ok || id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return Err(PkgError::Manifest(format!("id 非法: {id}")));
    }
    Ok(())
}

fn validate_semver(v: &str) -> Result<()> {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(PkgError::Manifest(format!(
            "version 必须是 x.y.z 形式: {v}"
        )));
    }
    Ok(())
}

fn validate_api(api: &str) -> Result<()> {
    let mut it = api.split('.');
    let major: u32 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| PkgError::Manifest(format!("api 版本非法: {api}")))?;
    let minor_ok = it
        .next()
        .map(|s| s.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false);
    if it.next().is_some() || !minor_ok {
        return Err(PkgError::Manifest(format!(
            "api 版本必须是 主.次（如 1.0）: {api}"
        )));
    }
    if !SUPPORTED_API_MAJOR.contains(&major) {
        return Err(PkgError::Manifest(format!(
            "插件 API 主版本 {major} 不被内核支持（支持 {SUPPORTED_API_MAJOR:?}）"
        )));
    }
    Ok(())
}

fn validate_hex64(v: &str, field: &str) -> Result<()> {
    if v.len() != 64
        || !v
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(PkgError::Manifest(format!(
            "{field} 必须是 64 位小写 hex（SHA-256 指纹）: {v}"
        )));
    }
    Ok(())
}

fn validate_verb(v: &str) -> Result<()> {
    let ok = !v.is_empty()
        && v.len() <= 64
        && v.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        });
    if ok {
        Ok(())
    } else {
        Err(PkgError::Manifest(format!("权限动词非法: {v}")))
    }
}

fn validate_egress(e: &str) -> Result<()> {
    if e == "unrestricted" {
        return Ok(());
    }
    let (cidr, port) = match e.rsplit_once(':') {
        Some((c, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| PkgError::Manifest(format!("network_egress 端口非法: {e}")))?;
            if port == 0 {
                return Err(PkgError::Manifest(format!("network_egress 端口非法: {e}")));
            }
            (c, Some(port))
        }
        None => (e, None),
    };
    let _ = port;
    parse_cidr(cidr).ok_or_else(|| {
        PkgError::Manifest(format!(
            "network_egress 只支持 IP/CIDR[:端口] 或 unrestricted（不做域名放行）: {e}"
        ))
    })?;
    Ok(())
}

fn parse_cidr(s: &str) -> Option<()> {
    let (ip, prefix) = match s.split_once('/') {
        Some((i, p)) => {
            let prefix: u8 = p.parse().ok()?;
            (i, Some(prefix))
        }
        None => (s, None),
    };
    let addr: std::net::IpAddr = ip.parse().ok()?;
    match addr {
        std::net::IpAddr::V4(_) => {
            if let Some(p) = prefix {
                if p > 32 {
                    return None;
                }
            }
        }
        std::net::IpAddr::V6(_) => {
            if let Some(p) = prefix {
                if p > 128 {
                    return None;
                }
            }
        }
    }
    Some(())
}

fn validate_cron(expr: &str) -> Result<()> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(PkgError::Manifest(format!(
            "cron 只支持 5 段表达式（分 时 日 月 周）: {expr}"
        )));
    }
    for f in fields {
        if f.is_empty()
            || !f
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | ',' | '-' | '?'))
        {
            return Err(PkgError::Manifest(format!("cron 字段非法: {expr}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
id = "com.example.nginx"
name = "Nginx 管理"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000001"
api = "1.0"
run = "on-demand"
requires = ["docker"]
network_egress = ["10.0.0.0/8:443"]
idle_timeout_sec = 300
max_restarts = 3

[[permissions]]
capability = "docker"
verbs = ["containers.list", "containers.logs", "containers.exec"]

[[ui]]
label = "Nginx"
path = "index.html"

[config_schema.site_name]
type = "string"
label = "站点名"

[config_schema.token]
type = "password"
secret = true
"#;

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(OK).unwrap();
        assert_eq!(m.id, "com.example.nginx");
        assert_eq!(m.requires, vec![Capability::Docker]);
        assert_eq!(m.permission_strings().len(), 3);
        assert!(m.requires_full_sandbox);
        assert_eq!(m.run, RunMode::OnDemand);
    }

    #[test]
    fn rejects_bad_id_version_api() {
        let bad = OK.replace("com.example.nginx", "nginx");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace("0.1.0", "0.1");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace("\"1.0\"", "\"2.0\"");
        assert!(Manifest::parse(&bad).is_err());
        let bad = OK.replace(
            "0000000000000000000000000000000000000000000000000000000000000001",
            "ZZ",
        );
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn resident_requires_reason() {
        let bad = OK.replace("run = \"on-demand\"", "run = \"resident\"");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("resident_reason"), "{e}");
        // 注意：新键必须插在第一个 [table] 之前，否则 TOML 会把它算进那张表里
        let good = OK.replace(
            "run = \"on-demand\"",
            "run = \"resident\"\nresident_reason = \"需要常驻采集指标\"",
        );
        assert!(Manifest::parse(&good).is_ok());
    }

    #[test]
    fn permission_must_be_in_requires() {
        let bad = OK.replace("requires = [\"docker\"]", "requires = []");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("requires"), "{e}");
    }

    #[test]
    fn egress_rejects_domains() {
        let bad = OK.replace("10.0.0.0/8:443", "example.com:443");
        let e = Manifest::parse(&bad).unwrap_err().to_string();
        assert!(e.contains("network_egress"), "{e}");
        let bad = OK.replace("10.0.0.0/8:443", "10.0.0.0/33:443");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn unknown_fields_rejected() {
        let bad = format!("{OK}\nprivileged = true\n");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn cron_and_ui_paths_validated() {
        let bad = OK.replace("path = \"index.html\"", "path = \"/etc/passwd\"");
        assert!(Manifest::parse(&bad).is_err());
        // cron 是顶层键，必须插在第一个 [table] 之前；这里挂在 network_egress 后面。
        let with_cron = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\ncron = [\"*/5 * * * *\"]",
        );
        assert_ne!(with_cron, OK, "样例里找不到插入点，测试本身要修");
        assert!(Manifest::parse(&with_cron).is_ok());
        let bad_cron = OK.replace(
            "network_egress = [\"10.0.0.0/8:443\"]",
            "network_egress = [\"10.0.0.0/8:443\"]\ncron = [\"*/5 * *\"]",
        );
        assert!(Manifest::parse(&bad_cron).is_err());
    }
}
