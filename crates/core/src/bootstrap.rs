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

pub use skiff_app::panel_settings::{
    ENTRY_PREFIX_GRACE_SECS, KEY_ENTRY_PREFIX, KEY_ENTRY_PREFIX_PREVIOUS,
    KEY_ENTRY_PREFIX_PREVIOUS_UNTIL,
};
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
    prepare_dirs(&cfg.data_dir, &cfg.web_root, &cfg.run_dir)?;

    let db_path = cfg.data_dir.join("skiff.db");
    let db = skiff_store::Db::open(&db_path)
        .with_context(|| format!("打开数据库失败: {}", db_path.display()))?;
    restrict_mode(&db_path, 0o600);
    let db = Arc::new(db);

    // 官方发布包自带 `official/public-key.txt`：首次启动（以及每次启动）自动写入
    // 信任库，避免新装机器上 `skiff plugin install` 因「未导入公钥」而失败。
    // 信任库本身是首用即固定（同 key_id 不覆盖），这里重复调用不会改变已有记录。
    if let Some(key_id) = trust_bundled_public_key(&db, &cfg.data_dir)? {
        info!(key_id = %key_id, "已自动信任官方发布包公钥");
    }

    // 升级安装只换二进制时，随包 `official/*.wbpkg` 不会自动生效；启动早期对库内
    // 已安装的官方插件做一次版本/内容比对并补齐（不动 data/、不改既有授权、
    // 不自动安装未装插件）。单包失败只告警，不阻断启动。
    let _ = skiff_plugin::sync_official_plugins(
        &db,
        &cfg.data_dir,
        cfg.allow_unsigned,
        cfg.form,
        cfg.allow_reduced_sandbox,
    );

    let stored_prefix = db.get_setting(KEY_ENTRY_PREFIX)?;
    let entry_prefix = match cfg
        .entry_prefix
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(p) => {
            // 配置文件/环境变量是显式来源：同步进库，并把旧前缀留作兼容别名。
            if let Some(old) = stored_prefix.as_deref().filter(|old| *old != p) {
                db.set_setting(KEY_ENTRY_PREFIX_PREVIOUS, old)?;
                db.set_setting(
                    KEY_ENTRY_PREFIX_PREVIOUS_UNTIL,
                    &(chrono::Utc::now().timestamp() + ENTRY_PREFIX_GRACE_SECS).to_string(),
                )?;
            }
            db.set_setting(KEY_ENTRY_PREFIX, p)?;
            p.to_string()
        }
        None => match stored_prefix {
            Some(p) => p,
            None => db.get_or_init_setting(KEY_ENTRY_PREFIX, || random_prefix(8))?,
        },
    };
    // 旧前缀只在有效窗口内作为 GET 重定向别名；窗口外忽略，避免永久旁路。
    let entry_prefix_previous = db
        .get_setting(KEY_ENTRY_PREFIX_PREVIOUS)?
        .filter(|prev| prev != &entry_prefix)
        .filter(|prev| skiff_app::panel_settings::validate_entry_prefix(prev).is_ok())
        .filter(|_| {
            db.get_setting(KEY_ENTRY_PREFIX_PREVIOUS_UNTIL)
                .ok()
                .flatten()
                .and_then(|s| s.parse::<i64>().ok())
                .map(|until| until > chrono::Utc::now().timestamp())
                .unwrap_or(false)
        });
    let csrf_secret = db.get_or_init_setting(KEY_CSRF_SECRET, skiff_identity::new_token)?;
    let _ = db.get_or_init_setting(KEY_INSTANCE_ID, skiff_identity::new_token);

    // 首启管理员（§5.2）：随机口令写入 initial-admin.txt。默认**不**强制改密，
    // 初始口令可直接登录；只有显式开启 require_initial_password_change 时才置标志。
    // 已有管理员一律不动：仅在空库（无任何用户）时创建。
    let mut first_run = None;
    if db.count_users()? == 0 {
        let password = random_password(16);
        let hash = skiff_identity::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("生成管理员口令失败: {e}"))?;
        db.create_user(
            "admin",
            &hash,
            skiff_store::Role::Admin,
            cfg.require_initial_password_change,
        )?;
        let path = cfg.data_dir.join("initial-admin.txt");
        let change_hint = if cfg.require_initial_password_change {
            "首次登录后必须改密；改密成功后本文件自动删除。"
        } else {
            "初始口令可直接登录；建议首次登录后尽快在「设置 → 口令」修改，改密成功后本文件自动删除。"
        };
        let content = format!(
            "Skiff 初始管理员\n用户名: admin\n口令: {password}\n\n{change_hint}\n生成时间: {}\n",
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
        web_root: cfg.web_root.clone(),
        trash_retention_days: cfg.trash_retention_days,
        config_path: cfg.config_path.clone(),
        listen: cfg.listen,
        entry_prefix,
        entry_prefix_previous,
        tls_enabled: cfg.tls_active,
        tls_cert: cfg.tls_cert.clone(),
        tls_key: cfg.tls_key.clone(),
        allow_insecure_http: cfg.allow_insecure_http,
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
        ext_exec_timeout: cfg.ext_exec_timeout,
        ext_batch_timeout: cfg.ext_batch_timeout,
    };

    let state = AppState::new(
        db,
        docker,
        supervisor,
        web_cfg,
        csrf_secret,
        env!("CARGO_PKG_VERSION"),
    );
    state
        .init_tasks()
        .map_err(|e| anyhow::anyhow!("初始化内核定时任务失败: {e}"))?;

    // 站点目录布局不做旧数据迁移：新建站点时直接创建 `{index,log,ssl,conf}`。
    // 公共 include 由运行环境安装/启动流程写入，这里不触碰站点数据。

    Ok(Bootstrapped { state, first_run })
}

fn prepare_dirs(data_dir: &Path, web_root: &Path, run_dir: &Path) -> Result<()> {
    for dir in [
        data_dir.to_path_buf(),
        data_dir.join("plugins"),
        data_dir.join("logs"),
        data_dir.join("backup"),
        // 站点寄宿布局：/www/{sites,nginx/conf.d,php}。站点级 index/log/ssl/conf
        // 建在各自 `/www/sites/<域名>/` 内，不再有集中日志/证书目录。
        web_root.to_path_buf(),
        web_root.join("sites"),
        web_root.join("nginx").join("conf.d"),
        web_root.join("php"),
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
    // 站点公共目录/php socket 目录允许容器内非 root 进程读取（0755）。
    for dir in [
        web_root.to_path_buf(),
        web_root.join("sites"),
        web_root.join("nginx").join("conf.d"),
        web_root.join("php"),
    ] {
        restrict_mode(&dir, 0o755);
    }
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

/// 自动信任发布包自带的官方公钥（`<data_dir>/official/public-key.txt`）。
///
/// 安装脚本会把 `official/` 整个目录（含公钥）复制到数据目录，这里在首次启动
/// （以及后续每次启动）把公钥写入信任库，使新装机器上的
/// `skiff plugin install <包>` 不再需要先手动导入。
///
/// 信任库是 TOFU（同 key_id 不覆盖），因此重复调用是幂等的；公钥文件缺失或
/// 内容损坏时只告警、不影响面板启动。
pub(crate) fn trust_bundled_public_key(
    db: &skiff_store::Db,
    data_dir: &Path,
) -> Result<Option<String>> {
    let path = data_dir.join("official").join("public-key.txt");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "读取官方公钥失败，跳过自动信任"
            );
            return Ok(None);
        }
    };
    let hex = raw.trim();
    if hex.is_empty() {
        warn!(path = %path.display(), "官方公钥为空，跳过自动信任");
        return Ok(None);
    }
    let key = match skiff_pkgfmt::decode_public_key(hex) {
        Ok(v) => v,
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "官方公钥无法解析，跳过自动信任"
            );
            return Ok(None);
        }
    };
    let key_id = skiff_pkgfmt::key_id_of(&key.to_bytes());
    db.trust_key(&key_id, hex, "official")
        .with_context(|| format!("写入信任库失败: {}", path.display()))?;
    Ok(Some(key_id))
}

/// 数据目录健康检查（`skiff doctor`）：只读，不改动任何文件。
pub fn doctor(cfg: &Config) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    out.push((
        cfg.data_dir.exists(),
        format!("数据根存在: {}", cfg.data_dir.display()),
    ));
    out.push((
        cfg.web_root.exists(),
        format!("站点寄宿根存在: {}", cfg.web_root.display()),
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

    fn test_config(
        data_dir: PathBuf,
        web_root: PathBuf,
        run_dir: PathBuf,
        config_path: PathBuf,
    ) -> Config {
        Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir,
            web_root,
            run_dir,
            config_path,
            docker_socket: "/nonexistent/docker.sock".into(),
            form: Form::Native,
            tier: Tier::Basic,
            tls_cert: None,
            tls_key: None,
            tls_enabled: None,
            allow_insecure_http: true,
            entry_prefix: None,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            trusted_proxies: vec![],
            authorized_roots: vec![],
            external_url: None,
            login_concurrency: 1,
            require_initial_password_change: false,
            ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
            ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
            dev_http: true,
            plaintext_behind_proxy: false,
            tls_active: false,
            trash_retention_days: 30,
        }
    }

    /// 默认口径：首启管理员**不**强制改密，初始口令可直接登录。
    #[test]
    fn initial_admin_does_not_require_password_change_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_config(
            tmp.path().join("data"),
            tmp.path().join("www"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        let boot = bootstrap(&cfg).expect("bootstrap 应成功");
        let user = boot
            .state
            .db()
            .user_by_name("admin")
            .unwrap()
            .expect("首启应创建 admin");
        assert!(!user.must_change_password, "默认不得强制首登改密");
        let initial = std::fs::read_to_string(cfg.data_dir.join("initial-admin.txt")).unwrap();
        assert!(initial.contains("可直接登录"), "{initial}");
        assert!(!initial.contains("必须改密"), "{initial}");
    }

    /// 显式开启后仍保留原能力：新管理员带 must_change_password=1。
    #[test]
    fn initial_admin_requires_password_change_when_explicitly_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = test_config(
            tmp.path().join("data"),
            tmp.path().join("www"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        cfg.require_initial_password_change = true;
        let boot = bootstrap(&cfg).expect("bootstrap 应成功");
        let user = boot
            .state
            .db()
            .user_by_name("admin")
            .unwrap()
            .expect("首启应创建 admin");
        assert!(user.must_change_password, "显式开启后应强制首登改密");
        let initial = std::fs::read_to_string(cfg.data_dir.join("initial-admin.txt")).unwrap();
        assert!(initial.contains("必须改密"), "{initial}");
    }

    /// 重装/升级口径：已有管理员的口令哈希与标志一律不动，不再生成新口令。
    #[test]
    fn bootstrap_does_not_overwrite_existing_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_config(
            tmp.path().join("data"),
            tmp.path().join("www"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        let boot = bootstrap(&cfg).expect("首启应成功");
        let admin = boot
            .state
            .db()
            .user_by_name("admin")
            .unwrap()
            .expect("首启应创建 admin");
        // 模拟用户已自行改密（升级后再启动不得覆盖）。
        let changed = skiff_identity::hash_password("user-changed-pass").unwrap();
        boot.state.db().set_password(admin.id, &changed).unwrap();
        drop(boot);

        let boot2 = bootstrap(&cfg).expect("再次启动应成功");
        assert!(boot2.first_run.is_none(), "已有管理员不应重新生成口令");
        let admin2 = boot2
            .state
            .db()
            .user_by_name("admin")
            .unwrap()
            .expect("admin 仍应存在");
        assert_eq!(admin2.password_hash, changed, "已有口令哈希不得被覆盖");
        assert!(!admin2.must_change_password);
    }

    /// 权限口径回归（红线）：数据根与 plugins/ 必须 0711（可穿越不可列），
    /// 改回 0700 会让降权到独立 UID 的插件全线 EACCES；backup 0700、
    /// 库文件与初始口令 0600。Windows 上 restrict_mode 是 no-op，只在 unix 断言。
    #[cfg(unix)]
    #[test]
    fn bootstrap_permission_bits_match_red_line() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let web_root = tmp.path().join("www");
        let run_dir = tmp.path().join("run");
        let cfg = test_config(
            data_dir.clone(),
            web_root.clone(),
            run_dir,
            tmp.path().join("config.toml"),
        );
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
        // 站点寄宿目录对容器内非 root 进程可读。
        assert_eq!(mode(&web_root), 0o755);
        assert_eq!(mode(&web_root.join("sites")), 0o755);
        assert_eq!(mode(&web_root.join("php")), 0o755);
        assert!(web_root.join("nginx").join("conf.d").is_dir());
        // 站点已改为「一站一目录」：日志在 `sites/<域名>/log/`，不再有集中日志目录。
        assert!(web_root.join("skiff").join("waf").join("rules").is_dir());
        assert!(
            !web_root.join("logs").exists(),
            "布局已改为站点内 log/，不得再建集中日志目录"
        );
    }

    /// 安装包自带 `official/public-key.txt` 时，首启应自动写入信任库。
    #[test]
    fn bundled_public_key_is_trusted_automatically() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let official = data_dir.join("official");
        std::fs::create_dir_all(&official).unwrap();
        let key = skiff_pkgfmt::SigningKey::generate();
        std::fs::write(official.join("public-key.txt"), key.public_hex()).unwrap();

        let db = skiff_store::Db::open_in_memory().unwrap();
        assert!(db.trusted_key(&key.key_id()).unwrap().is_none());

        let id = trust_bundled_public_key(&db, &data_dir)
            .unwrap()
            .expect("有公钥文件时应写入信任库");
        assert_eq!(id, key.key_id());
        assert_eq!(
            db.trusted_key(&key.key_id()).unwrap(),
            Some(key.public_hex())
        );
        assert_eq!(db.list_trusted_keys().unwrap()[0].2, "official");

        // 再次启动幂等：同 key_id 不重复插入，也不覆盖内容。
        assert!(trust_bundled_public_key(&db, &data_dir).unwrap().is_some());
        assert_eq!(db.list_trusted_keys().unwrap().len(), 1);
    }

    /// 官方目录里有坏包时：只告警，不得阻断启动（升级安装失败的兜底口径）。
    #[test]
    fn corrupt_official_package_does_not_block_bootstrap() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let official = data_dir.join("official");
        std::fs::create_dir_all(&official).unwrap();
        std::fs::write(official.join("broken-0.1.0.wbpkg"), b"not a package").unwrap();

        let cfg = test_config(
            data_dir,
            tmp.path().join("www"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        bootstrap(&cfg).expect("坏官方包不得让面板启动失败");
    }

    /// 没有公钥文件 / 文件为空时不写库、也不报错（未携带公钥是合法状态）。
    #[test]
    fn missing_or_empty_bundled_key_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        let db = skiff_store::Db::open_in_memory().unwrap();
        assert!(trust_bundled_public_key(&db, &data_dir).unwrap().is_none());

        std::fs::create_dir_all(data_dir.join("official")).unwrap();
        std::fs::write(data_dir.join("official").join("public-key.txt"), "  \n").unwrap();
        assert!(trust_bundled_public_key(&db, &data_dir).unwrap().is_none());
        assert!(db.list_trusted_keys().unwrap().is_empty());
    }

    /// 跨平台烟测：bootstrap 全流程（建目录、开库、首启管理员）必须能跑完。
    #[test]
    fn bootstrap_succeeds_and_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = test_config(
            tmp.path().join("data"),
            tmp.path().join("www"),
            tmp.path().join("run"),
            tmp.path().join("config.toml"),
        );
        bootstrap(&cfg).expect("bootstrap 应成功");
    }
}
