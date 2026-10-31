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
    pub run_dir: Option<PathBuf>,
    pub docker_socket: Option<String>,
    /// `native`（默认）或 `docker`（受限形态，§5.4）
    pub form: Option<String>,
    /// `basic`（M2）或 `lifecycle`（M3，默认）
    pub docker_tier: Option<String>,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub allow_insecure_http: Option<bool>,
    pub allow_unsigned: Option<bool>,
    pub allow_reduced_sandbox: Option<bool>,
    pub trusted_proxies: Option<Vec<String>>,
    /// 绑定挂载授权根（native 形态，§5.5）
    pub authorized_roots: Option<Vec<PathBuf>>,
    pub external_url: Option<String>,
    pub login_concurrency: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub run_dir: PathBuf,
    pub config_path: PathBuf,
    pub docker_socket: String,
    pub form: Form,
    pub tier: Tier,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub allow_insecure_http: bool,
    pub allow_unsigned: bool,
    pub allow_reduced_sandbox: bool,
    pub trusted_proxies: Vec<Cidr>,
    pub authorized_roots: Vec<PathBuf>,
    pub external_url: Option<String>,
    pub login_concurrency: usize,
    /// 由 main 计算：无 TLS 的 loopback 开发模式
    pub dev_http: bool,
    /// 由 main 计算：非 loopback 明文，但显式确认了「可信反代已终止 TLS」
    pub plaintext_behind_proxy: bool,
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

        let external_url = env_str("SKIFF_EXTERNAL_URL").or(file.external_url.clone());
        let login_concurrency = file.login_concurrency.unwrap_or(2).max(1);

        Ok(Config {
            listen,
            data_dir,
            run_dir,
            config_path,
            docker_socket,
            form,
            tier,
            tls_cert,
            tls_key,
            allow_insecure_http,
            allow_unsigned,
            allow_reduced_sandbox,
            trusted_proxies,
            authorized_roots,
            external_url,
            login_concurrency,
            dev_http: false,
            plaintext_behind_proxy: false,
        })
    }

    /// 启动前校验（§6.2）：非 loopback 必须 HTTPS 或可信反代终止。
    ///
    /// 明文路径只有两种，且都必须显式打开：
    /// 1. loopback + `allow_insecure_http`（本地调试，Cookie 降级为 `skiff_dev_session`）
    /// 2. 非 loopback + `allow_insecure_http` + **非空 `trusted_proxies`**
    ///    —— 用于「反代终止 TLS」（含 1Panel 商店形态）。没有可信反代白名单就只是裸奔，
    ///    一律拒绝。
    pub fn validate(&mut self) -> Result<()> {
        let has_tls = self.tls_cert.is_some() && self.tls_key.is_some();
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
            return Ok(());
        }
        if loopback {
            if !self.allow_insecure_http {
                bail!(
                    "未配置 TLS：loopback 调试请显式加 --dev 或 allow_insecure_http = true（§6.2）。\
                     远程部署请配置 tls_cert/tls_key，或在可信反代终止 TLS。"
                );
            }
            self.dev_http = true;
            return Ok(());
        }
        // 非 loopback
        if self.allow_insecure_http && !self.trusted_proxies.is_empty() {
            self.dev_http = false;
            self.plaintext_behind_proxy = true;
            return Ok(());
        }
        if self.allow_insecure_http {
            bail!(
                "监听 {} 为非 loopback 且未配置 TLS，且 trusted_proxies 为空：\
                 这等于把管理面明文暴露在网络上。请配置 tls_cert/tls_key；\
                 若 TLS 已由可信反代终止，请同时配置 trusted_proxies（CIDR 白名单）。",
                self.listen
            );
        }
        bail!(
            "监听 {} 为非 loopback 且未配置 TLS：远程管理默认只接受 HTTPS。\
             请配置 tls_cert/tls_key（可信反代终止时也请显式加 allow_insecure_http = true 并配 trusted_proxies）",
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
        PathBuf::from("/var/lib/skiff")
    } else {
        PathBuf::from("skiff-data")
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
            run_dir: PathBuf::from("r"),
            config_path: PathBuf::from("c.toml"),
            docker_socket: "/var/run/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Lifecycle,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: true,
            allow_unsigned: false,
            allow_reduced_sandbox: false,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 2,
            dev_http: false,
            plaintext_behind_proxy: false,
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
            run_dir: PathBuf::from("r"),
            config_path: PathBuf::from("c.toml"),
            docker_socket: "/var/run/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Lifecycle,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: false,
            allow_unsigned: false,
            allow_reduced_sandbox: false,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 2,
            dev_http: false,
            plaintext_behind_proxy: false,
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
    fn invalid_trusted_proxy_cidr_fails_startup() {
        let e = parse_trusted_proxies(&["10.0.0.0/8".into(), "not-a-cidr".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("not-a-cidr"), "{e}");
        assert!(parse_trusted_proxies(&["172.18.0.0/16".into()]).is_ok());
    }

    #[test]
    fn probe_addr_substitutes_unspecified() {
        let mut c = Config::load(None, true).unwrap();
        c.listen = "0.0.0.0:18080".parse().unwrap();
        assert_eq!(c.probe_addr().ip().to_string(), "127.0.0.1");
    }
}
