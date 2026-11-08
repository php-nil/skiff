//! 配置：环境变量 > `/etc/skiff/config.toml` > 内置默认（§5.2）。

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use skiff_docker::{Form, Tier};
use skiff_model::Cidr;

/// 配置文件形态（`/etc/skiff/config.toml`）。字段全部可选，缺失走默认。
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
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub web_root: PathBuf,
    pub run_dir: PathBuf,
    pub config_path: PathBuf,
    pub docker_socket: String,
    pub form: Form,
    pub tier: Tier,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// `Some(true/false)` 为显式开关；`None` 表示旧配置的自动推断。
    pub tls_enabled: Option<bool>,
    pub allow_insecure_http: bool,
    /// 安全入口前缀（配置文件/环境变量显式值；`None` 时用数据库值）。
    pub entry_prefix: Option<String>,
    pub allow_unsigned: bool,
    pub allow_reduced_sandbox: bool,
    pub trusted_proxies: Vec<Cidr>,
    pub authorized_roots: Vec<PathBuf>,
    pub external_url: Option<String>,
    pub login_concurrency: usize,
    /// 回收站保留天数（默认 30，范围 1–365）。
    pub trash_retention_days: u32,
    /// 首启管理员是否强制首次登录改密。默认 false，可用配置/环境变量/CLI 显式开启。
    /// 只影响**新建**管理员；已有账号一律不动。
    pub require_initial_password_change: bool,
    /// 由 main 计算：无 TLS 的 loopback 开发模式
    pub dev_http: bool,
    /// 由 main 计算：非 loopback 明文，但显式确认了「可信反代已终止 TLS」
    pub plaintext_behind_proxy: bool,
    /// 由 main 计算：本次进程是否真正用 TLS 监听（显式关闭时即使配了证书也为 false）。
    pub tls_active: bool,
}

impl Config {
    pub fn load(config_path: Option<PathBuf>, dev_flag: bool) -> Result<Config> {
        let config_path = config_path
            .or_else(|| std::env::var_os("SKIFF_CONFIG").map(PathBuf::from))
            .unwrap_or_else(default_config_path);

        let file = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)
                .with_context(|| format!("读取配置文件失败: {}", config_path.display()))?;
            toml::from_str::<FileConfig>(&text)
                .with_context(|| format!("解析配置文件失败: {}", config_path.display()))?
        } else {
            FileConfig::default()
        };

        let listen_str = env_str("SKIFF_LISTEN")
            .or(file.listen.clone())
            .unwrap_or_else(|| "127.0.0.1:18080".to_string());
        let listen: SocketAddr = listen_str
            .parse()
            .with_context(|| format!("监听地址非法: {listen_str}"))?;

        let data_dir = env_str("SKIFF_DATA_DIR")
            .map(PathBuf::from)
            .or(file.data_dir.clone())
            .unwrap_or_else(default_data_dir);
        let web_root = env_str("SKIFF_WEB_ROOT")
            .map(PathBuf::from)
            .or(file.web_root.clone())
            .unwrap_or_else(default_web_root);
        let run_dir = env_str("SKIFF_RUN_DIR")
            .map(PathBuf::from)
            .or(file.run_dir.clone())
            .unwrap_or_else(default_run_dir);

        let docker_socket = env_str("SKIFF_DOCKER_SOCKET")
            .or(file.docker_socket.clone())
            .unwrap_or_else(|| "/var/run/docker.sock".to_string());

        // 形态字符串只认 model 里的那张表，避免配置文件、CLI、UI 各写一套拼法
        let form_raw = env_str("SKIFF_FORM")
            .or(file.form.clone())
            .unwrap_or_else(|| "native".into());
        let form = Form::parse(&form_raw)
            .ok_or_else(|| anyhow::anyhow!("form 只支持 native 或 docker: {form_raw}"))?;
        let tier = match env_str("SKIFF_DOCKER_TIER")
            .or(file.docker_tier.clone())
            .unwrap_or_else(|| "lifecycle".into())
            .as_str()
        {
            "basic" => Tier::Basic,
            "lifecycle" => Tier::Lifecycle,
            other => bail!("docker_tier 只支持 basic 或 lifecycle: {other}"),
        };

        let bool_env = |key: &str, fallback: bool| -> bool {
            match std::env::var(key) {
                Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"),
                Err(_) => fallback,
            }
        };

        let allow_insecure_http = bool_env(
            "SKIFF_ALLOW_INSECURE_HTTP",
            file.allow_insecure_http.unwrap_or(false) || dev_flag,
        );
        let allow_unsigned = bool_env("SKIFF_ALLOW_UNSIGNED", file.allow_unsigned.unwrap_or(false));
        let allow_reduced_sandbox = bool_env(
            "SKIFF_ALLOW_REDUCED_SANDBOX",
            file.allow_reduced_sandbox.unwrap_or(false),
        );
        // 默认关闭：初始口令可直接登录。需要「首登强制改密」时显式开启。
        let require_initial_password_change = bool_env(
            "SKIFF_REQUIRE_INITIAL_PASSWORD_CHANGE",
            file.require_initial_password_change.unwrap_or(false),
        );

        let trusted_proxies_raw: Vec<String> = env_str("SKIFF_TRUSTED_PROXIES")
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .or(file.trusted_proxies.clone())
            .unwrap_or_default();
        // 非法 CIDR 直接拒绝启动（不再 warn 后丢弃：悄悄丢弃会把白名单变窄，
        // 反代真实 IP 判定在运行期出错且极难定位）
        let trusted_proxies = parse_trusted_proxies(&trusted_proxies_raw)?;

        let authorized_roots = env_str("SKIFF_AUTHORIZED_ROOTS")
            .map(|s| {
                s.split(',')
                    .map(|x| PathBuf::from(x.trim()))
                    .collect::<Vec<_>>()
            })
            .or(file.authorized_roots.clone())
            .unwrap_or_default();

        let tls_cert = env_str("SKIFF_TLS_CERT")
            .map(PathBuf::from)
            .or(file.tls_cert.clone());
        let tls_key = env_str("SKIFF_TLS_KEY")
            .map(PathBuf::from)
            .or(file.tls_key.clone());
        let tls_enabled = match env_str("SKIFF_TLS_ENABLED") {
            Some(v) => Some(parse_bool_word("SKIFF_TLS_ENABLED", &v)?),
            None => file.tls_enabled,
        };
        let entry_prefix = env_str("SKIFF_ENTRY_PREFIX").or(file.entry_prefix.clone());

        let external_url = env_str("SKIFF_EXTERNAL_URL").or(file.external_url.clone());
        let login_concurrency = file.login_concurrency.unwrap_or(2).max(1);
        let trash_retention_days = match env_str("SKIFF_TRASH_RETENTION_DAYS") {
            Some(v) => v
                .parse::<u32>()
                .with_context(|| format!("SKIFF_TRASH_RETENTION_DAYS 非法: {v}"))?
                .clamp(1, 365),
            None => file.trash_retention_days.unwrap_or(30).clamp(1, 365),
        };

        Ok(Config {
            listen,
            data_dir,
            web_root,
            run_dir,
            config_path,
            docker_socket,
            form,
            tier,
            tls_cert,
            tls_key,
            tls_enabled,
            allow_insecure_http,
            entry_prefix,
            allow_unsigned,
            allow_reduced_sandbox,
            trusted_proxies,
            authorized_roots,
            external_url,
            login_concurrency,
            trash_retention_days,
            require_initial_password_change,
            dev_http: false,
            plaintext_behind_proxy: false,
            tls_active: false,
        })
    }

    /// 启动前校验（§6.2）：三态 TLS 与入口前缀。
    ///
    /// - `tls_enabled = true`：必须同时配置证书与私钥，缺失时报可读错误；
    /// - `tls_enabled = false`：显式直接明文监听（局域网直连），Cookie 降级为
    ///   `skiff_dev_session`（无 `__Host-`/`Secure`），启动日志另打警告；
    /// - 未写 `tls_enabled`：保留旧行为——有证书走 TLS；否则只允许 loopback 调试
    ///   或「非 loopback + allow_insecure_http + 非空 trusted_proxies」的可信反代形态。
    pub fn validate(&mut self) -> Result<()> {
        if let Some(prefix) = self.entry_prefix.as_deref() {
            skiff_app::panel_settings::validate_entry_prefix(prefix)
                .map_err(|e| anyhow::anyhow!("entry_prefix 非法: {e}"))?;
        }

        // 显式关闭 TLS：不看证书/反代白名单，直接监听明文；Cookie 必须同步降级，
        // 否则浏览器在 HTTP 下会丢弃 `__Host-` + Secure 的会话 Cookie。
        if self.tls_enabled == Some(false) {
            self.dev_http = true;
            self.plaintext_behind_proxy = false;
            self.tls_active = false;
            return Ok(());
        }

        let has_tls = self.tls_cert.is_some() && self.tls_key.is_some();
        if self.tls_enabled == Some(true) && !has_tls {
            bail!(
                "已启用 TLS（tls_enabled = true），但 tls_cert / tls_key 未同时配置：\
                 请补齐证书与私钥路径；若要直接监听明文 HTTP，请显式设 tls_enabled = false"
            );
        }
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            bail!("tls_cert 与 tls_key 必须同时配置");
        }
        let loopback = self.listen.ip().is_loopback();
        if has_tls {
            for p in [
                self.tls_cert.as_ref().unwrap(),
                self.tls_key.as_ref().unwrap(),
            ] {
                if !p.exists() {
                    bail!("TLS 文件不存在: {}", p.display());
                }
            }
            self.dev_http = false;
            self.tls_active = true;
            return Ok(());
        }
        if loopback {
            if !self.allow_insecure_http {
                bail!(
                    "未配置 TLS：loopback 调试请显式加 --dev 或 allow_insecure_http = true（§6.2）；\
                     局域网/远程直连请设 tls_enabled = false，或配置 tls_cert/tls_key"
                );
            }
            self.dev_http = true;
            return Ok(());
        }
        // 非 loopback 的兼容形态：可信反代终止 TLS
        if self.allow_insecure_http && !self.trusted_proxies.is_empty() {
            self.dev_http = false;
            self.plaintext_behind_proxy = true;
            return Ok(());
        }
        if self.allow_insecure_http {
            bail!(
                "监听 {} 为非 loopback 且未配置 TLS，且 trusted_proxies 为空。\
                 直接明文监听请设 tls_enabled = false（Cookie 会降级、仅限内网）；\
                 若 TLS 已由可信反代终止，请同时配置 trusted_proxies（CIDR 白名单）；\
                 否则请配置 tls_cert/tls_key。",
                self.listen
            );
        }
        bail!(
            "监听 {} 为非 loopback 且未配置 TLS：远程管理默认只接受 HTTPS。\
             直接明文监听请设 tls_enabled = false（仅限内网）；\
             请配置 tls_cert/tls_key，或在可信反代终止 TLS（allow_insecure_http = true + trusted_proxies）",
            self.listen
        )
    }

    /// `skiff health` 用的探测地址（0.0.0.0 → 127.0.0.1）。
    pub fn probe_addr(&self) -> SocketAddr {
        let ip = if self.listen.ip().is_unspecified() {
            "127.0.0.1".parse().unwrap()
        } else {
            self.listen.ip()
        };
        SocketAddr::new(ip, self.listen.port())
    }
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// 三态/布尔环境变量的可读解析；非法值直接拒绝启动，避免静默按 false 处理。
fn parse_bool_word(key: &str, raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => bail!("{key} 只支持 true/false/on/off/1/0，实际: {other}"),
    }
}

/// 逐项解析可信代理 CIDR；非法项拒绝启动。
fn parse_trusted_proxies(items: &[String]) -> Result<Vec<Cidr>> {
    items
        .iter()
        .map(|s| {
            Cidr::parse(s).ok_or_else(|| {
                anyhow::anyhow!(
                    "trusted_proxies 含非法 CIDR 项: {s}（示例：172.18.0.0/16、10.0.0.1）"
                )
            })
        })
        .collect()
}

fn default_config_path() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/etc/skiff/config.toml")
    } else {
        PathBuf::from("skiff-config.toml")
    }
}

fn default_data_dir() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/www/skiff")
    } else {
        PathBuf::from("skiff-data")
    }
}

fn default_web_root() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/www")
    } else {
        PathBuf::from("skiff-web")
    }
}

fn default_run_dir() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/run/skiff")
    } else {
        PathBuf::from("skiff-run")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_plaintext() {
        let mut c = Config {
            listen: "0.0.0.0:18080".parse().unwrap(),
            data_dir: PathBuf::from("d"),
            web_root: PathBuf::from("w"),
            run_dir: PathBuf::from("r"),
            config_path: PathBuf::from("c.toml"),
            docker_socket: "/var/run/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Lifecycle,
            tls_cert: None,
            tls_key: None,
            tls_enabled: None,
            allow_insecure_http: true,
            entry_prefix: None,
            allow_unsigned: false,
            allow_reduced_sandbox: false,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 2,
            trash_retention_days: 30,
            require_initial_password_change: false,
            dev_http: false,
            plaintext_behind_proxy: false,
            tls_active: false,
        };
        let e = c.validate().unwrap_err().to_string();
        // 只有 allow_insecure_http、没有可信反代白名单：必须拒绝，并说清差什么
        assert!(e.contains("trusted_proxies"), "{e}");
    }

    #[test]
    fn loopback_requires_explicit_dev_flag() {
        let mut c = Config {
            listen: "127.0.0.1:18080".parse().unwrap(),
            data_dir: PathBuf::from("d"),
            web_root: PathBuf::from("w"),
            run_dir: PathBuf::from("r"),
            config_path: PathBuf::from("c.toml"),
            docker_socket: "/var/run/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Lifecycle,
            tls_cert: None,
            tls_key: None,
            tls_enabled: None,
            allow_insecure_http: false,
            entry_prefix: None,
            allow_unsigned: false,
            allow_reduced_sandbox: false,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 2,
            trash_retention_days: 30,
            require_initial_password_change: false,
            dev_http: false,
            plaintext_behind_proxy: false,
            tls_active: false,
        };
        assert!(c.validate().is_err());
        c.allow_insecure_http = true;
        c.validate().unwrap();
        assert!(c.dev_http);
    }

    #[test]
    fn plaintext_requires_trusted_proxy_when_not_loopback() {
        let mut c = Config::load(None, false).unwrap();
        c.listen = "0.0.0.0:18080".parse().unwrap();
        c.tls_cert = None;
        c.tls_key = None;
        c.allow_insecure_http = true;
        c.trusted_proxies = vec![];
        // 只有 allow_insecure_http、没有可信反代 = 裸奔，拒绝
        assert!(c.validate().is_err());
        c.trusted_proxies = vec![Cidr::parse("172.18.0.0/16").unwrap()];
        c.validate().unwrap();
        assert!(c.plaintext_behind_proxy);
        assert!(!c.dev_http);
    }

    #[test]
    fn tls_enabled_without_cert_fails_readably() {
        let mut c = Config::load(None, false).unwrap();
        c.listen = "0.0.0.0:18080".parse().unwrap();
        c.tls_enabled = Some(true);
        c.tls_cert = None;
        c.tls_key = None;
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("tls_enabled"), "{e}");
        assert!(e.contains("tls_cert"), "{e}");
    }

    #[test]
    fn tls_off_allows_lan_plaintext_and_downgrades_cookie() {
        // 关闭 TLS 直接监听局域网地址：必须允许启动，且 Cookie 走非 __Host- 形态。
        let mut c = Config::load(None, false).unwrap();
        c.listen = "0.0.0.0:18080".parse().unwrap();
        c.tls_enabled = Some(false);
        c.tls_cert = None;
        c.tls_key = None;
        c.allow_insecure_http = false;
        c.trusted_proxies = vec![];
        c.validate().unwrap();
        assert!(c.dev_http, "明文直连必须降级 Cookie");
        assert!(!c.plaintext_behind_proxy);
        assert_eq!(
            skiff_identity::session_cookie_name(c.dev_http),
            "skiff_dev_session"
        );
        // 反代形态仍使用 __Host- + Secure
        c.tls_enabled = None;
        c.allow_insecure_http = true;
        c.trusted_proxies = vec![Cidr::parse("172.18.0.0/16").unwrap()];
        c.dev_http = false;
        c.plaintext_behind_proxy = false;
        c.validate().unwrap();
        assert!(!c.dev_http);
        assert_eq!(
            skiff_identity::session_cookie_name(c.dev_http),
            "__Host-skiff_session"
        );
    }

    #[test]
    fn invalid_entry_prefix_fails_startup() {
        let mut c = Config::load(None, true).unwrap();
        c.entry_prefix = Some("api".into()); // 保留段 + 长度不足
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("entry_prefix"), "{e}");
        c.entry_prefix = Some("abc12345".into());
        c.validate().unwrap();
    }

    #[test]
    fn invalid_trusted_proxy_cidr_fails_startup() {
        let e = parse_trusted_proxies(&["10.0.0.0/8".into(), "not-a-cidr".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("not-a-cidr"), "{e}");
        assert!(parse_trusted_proxies(&["172.18.0.0/16".into()]).is_ok());
    }

    #[test]
    fn require_initial_password_change_defaults_off_and_can_be_enabled() {
        // 默认关闭：初始口令可直接登录，不强制首登改密。
        let c = Config::load(None, true).unwrap();
        assert!(!c.require_initial_password_change);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "require_initial_password_change = true\n").unwrap();
        let c = Config::load(Some(path), true).unwrap();
        assert!(c.require_initial_password_change, "配置显式开启后应生效");
    }

    #[test]
    fn default_paths_follow_www_layout() {
        let c = Config::load(None, true).unwrap();
        if cfg!(unix) {
            assert_eq!(c.data_dir, PathBuf::from("/www/skiff"));
            assert_eq!(c.web_root, PathBuf::from("/www"));
        }
        assert!(c.data_dir.is_absolute() || !cfg!(unix));
    }

    #[test]
    fn probe_addr_substitutes_unspecified() {
        let mut c = Config::load(None, true).unwrap();
        c.listen = "0.0.0.0:18080".parse().unwrap();
        assert_eq!(c.probe_addr().ip().to_string(), "127.0.0.1");
    }
}
