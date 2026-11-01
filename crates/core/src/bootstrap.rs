//! 启动引导：目录、数据库、首启管理员、安全入口前缀、CSRF 密钥。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use rand::Rng;
use tracing::{info, warn};

use skiff_app::{AppState, WebConfig};
use skiff_docker::{DockerService, Form, Policy, Tier};
use skiff_identity::SessionLimits;
use skiff_plugin::{Supervisor, SupervisorConfig};

use crate::config::Config;

pub const KEY_ENTRY_PREFIX: &str = "security.entry_prefix";
pub const KEY_CSRF_SECRET: &str = "security.csrf_secret";
pub const KEY_INSTANCE_ID: &str = "instance.id";

/// 首启写入 `initial-admin.txt` 的内容（0600）。
pub struct FirstRun {
    pub password_path: PathBuf,
    pub username: String,
}

pub struct Bootstrapped {
    pub state: Arc<AppState>,
    pub first_run: Option<FirstRun>,
}

pub fn bootstrap(cfg: &Config) -> Result<Bootstrapped> {
    prepare_dirs(&cfg.data_dir, &cfg.run_dir)?;

    let db_path = cfg.data_dir.join("skiff.db");
    let db = skiff_store::Db::open(&db_path)
        .with_context(|| format!("打开数据库失败: {}", db_path.display()))?;
    restrict_mode(&db_path, 0o600);
    let db = Arc::new(db);

    let entry_prefix = db.get_or_init_setting(KEY_ENTRY_PREFIX, || random_prefix(8))?;
    let csrf_secret = db.get_or_init_setting(KEY_CSRF_SECRET, skiff_identity::new_token)?;
    let _ = db.get_or_init_setting(KEY_INSTANCE_ID, skiff_identity::new_token);

    // 首启管理员（§5.2）：随机口令写入 initial-admin.txt，首登强制改密
    let mut first_run = None;
    if db.count_users()? == 0 {
        let password = random_password(16);
        let hash = skiff_identity::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("生成管理员口令失败: {e}"))?;
        db.create_user("admin", &hash, skiff_store::Role::Admin, true)?;
        let path = cfg.data_dir.join("initial-admin.txt");
        let content = format!(
            "Skiff 初始管理员\n用户名: admin\n口令: {password}\n\n首次登录后必须改密；改密成功后本文件自动删除。\n生成时间: {}\n",
            chrono::Utc::now().to_rfc3339()
        );
        std::fs::write(&path, content)
            .with_context(|| format!("写入初始口令文件失败: {}", path.display()))?;
        restrict_mode(&path, 0o600);
        info!(path = %path.display(), "已生成初始管理员口令");
        first_run = Some(FirstRun {
            password_path: path,
            username: "admin".into(),
        });
    }

    // 插件 run/ 重启即清（§5.2）
    skiff_plugin::supervisor::clean_run_dirs(&cfg.data_dir);

    let docker = Arc::new(DockerService::connect(&cfg.docker_socket));
    if !docker.available() {
        warn!(reason = %docker.unavailable_reason(), "Docker 不可用，面板以降级模式启动");
    }

    let sup_cfg = SupervisorConfig {
        allow_reduced_sandbox: cfg.allow_reduced_sandbox,
        ..SupervisorConfig::new(cfg.data_dir.clone(), cfg.run_dir.clone())
    };
    let supervisor = Arc::new(Supervisor::new(sup_cfg, db.clone()));

    let policy = Policy::new(cfg.tier, cfg.form, cfg.authorized_roots.clone())
        .map_err(|e| anyhow::anyhow!("路径授权根校验失败: {e}"))?;

    // 非法 CIDR 在 Config::load 里已拒绝启动（parse_trusted_proxies），这里直接用
    let trusted_proxies = cfg.trusted_proxies.clone();

    let web_cfg = WebConfig {
        data_dir: cfg.data_dir.clone(),
        entry_prefix,
        dev_http: cfg.dev_http,
        trusted_proxies,
        external_url: cfg.external_url.clone(),
        tier: cfg.tier,
        form: cfg.form,
        allow_unsigned: cfg.allow_unsigned,
        allow_reduced_sandbox: cfg.allow_reduced_sandbox,
        policy,
        session: SessionLimits::default(),
        login_concurrency: cfg.login_concurrency,
    };

    let state = AppState::new(
        db,
        docker,
        supervisor,
        web_cfg,
        csrf_secret,
        env!("CARGO_PKG_VERSION"),
    );

    Ok(Bootstrapped { state, first_run })
}

fn prepare_dirs(data_dir: &Path, run_dir: &Path) -> Result<()> {
    for dir in [
        data_dir.to_path_buf(),
        data_dir.join("plugins"),
        data_dir.join("logs"),
        data_dir.join("backup"),
        run_dir.to_path_buf(),
        run_dir.join("plugins"),
    ] {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建目录失败: {}", dir.display()))?;
    }
    // 数据根 0711：**必须**有 other-x，否则降权到独立 UID 的插件进不去自己的
    // data/（ancestor 无 x 权限 = 路径不可达）。0711 不可列目录，库文件 0600，
    // 所以「看不见、打不开」两个目标都还在；若设成 0700，插件运行时会全线 EACCES。
    restrict_mode(data_dir, 0o711);
    restrict_mode(&data_dir.join("plugins"), 0o711);
    restrict_mode(&data_dir.join("backup"), 0o700);
    Ok(())
}

#[cfg(unix)]
fn restrict_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict_mode(_path: &Path, _mode: u32) {}

fn random_prefix(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// 口令字符集去掉易混淆字符（0/O、1/l/I）。
fn random_password(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// 数据目录健康检查（`skiff doctor`）：只读，不改动任何文件。
pub fn doctor(cfg: &Config) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    out.push((
        cfg.data_dir.exists(),
        format!("数据根存在: {}", cfg.data_dir.display()),
    ));
    out.push((
        cfg.data_dir.join("skiff.db").exists(),
        "数据库文件存在".into(),
    ));
    let cfg_ok = !cfg.config_path.exists() || cfg.config_path.is_file();
    out.push((cfg_ok, format!("配置文件: {}", cfg.config_path.display())));
    let tls_ok = cfg.tls_cert.is_some() == cfg.tls_key.is_some();
    out.push((tls_ok, "TLS 证书/私钥成对配置".into()));
    let docker = std::path::Path::new(&cfg.docker_socket).exists();
    out.push((docker, format!("Docker socket: {}", cfg.docker_socket)));
    if cfg.form == Form::Native && cfg.authorized_roots.is_empty() {
        out.push((
            true,
            "未配置宿主挂载授权根：所有 bind 挂载将被拒绝（默认安全）".into(),
        ));
    }
    if cfg.tier == Tier::Basic {
        out.push((
            true,
            "docker_tier=basic：生命周期动词（创建/删除）未放开".into(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_secrets_have_expected_shape() {
        let p = random_prefix(8);
        assert_eq!(p.len(), 8);
        assert!(p
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        let pw = random_password(16);
        assert_eq!(pw.len(), 16);
        assert!(!pw.contains('0') && !pw.contains('O'));
    }

    fn test_config(data_dir: PathBuf, run_dir: PathBuf, config_path: PathBuf) -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir,
            run_dir,
            config_path,
            docker_socket: "/nonexistent/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Basic,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: true,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 1,
            dev_http: true,
            plaintext_behind_proxy: false,
        }
    }

    /// 权限口径回归（红线）：数据根与 plugins/ 必须 0711（可穿越不可列），
    /// 改回 0700 会让降权到独立 UID 的插件全线 EACCES；backup 0700、
    /// 库文件与初始口令 0600。Windows 上 restrict_mode 是 no-op，只在 unix 断言。
    #[cfg(unix)]
    #[test]
    fn bootstrap_permission_bits_match_red_line() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let run_dir = tmp.path().join("run");
        let cfg = test_config(data_dir.clone(), run_dir, tmp.path().join("config.toml"));
        let boot = bootstrap(&cfg).expect("bootstrap 应成功");
        assert!(boot.first_run.is_some(), "空库首启应生成初始管理员");
        drop(boot);

        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o7777;
        assert_eq!(
            mode(&data_dir),
            0o711,
            "数据根必须 0711：改回 0700 会让降权插件全线 EACCES"
        );
        assert_eq!(mode(&data_dir.join("plugins")), 0o711);
        assert_eq!(mode(&data_dir.join("backup")), 0o700);
        assert_eq!(mode(&data_dir.join("skiff.db")), 0o600);
        assert_eq!(mode(&data_dir.join("initial-admin.txt")), 0o600);
    }

    /// 跨平台烟测：bootstrap 全流程（建目录、开库、首启管理员）必须能跑完。
    #[test]
    fn bootstrap_succeeds_and_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_config(
            tmp.path().join("data"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        bootstrap(&cfg).expect("bootstrap 应成功");
    }
}
