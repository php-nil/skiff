//! Skiff（轻舟）· 单二进制入口。
//!
//! 设计约束：tokio **显式** `worker_threads = 2`（默认值是 CPU 数）；`panic = unwind`
//! 让 handler panic 只断该请求，配合 tower-http 的 CatchPanic 转 500 + 审计。

mod bootstrap;
mod cli;
mod commands;
mod config;
mod kernel;
mod server;

use std::process::ExitCode;

use anyhow::Result;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("错误: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    // ring 作为 rustls 的密码学 provider（R1 例外清单内的 C/asm 组件）
    let _ = rustls::crypto::ring::default_provider().install_default();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse(raw) {
        Ok(c) => c,
        Err(msg) => {
            println!("{msg}");
            return Ok(ExitCode::from(if msg.starts_with("Skiff") { 0 } else { 2 }));
        }
    };

    init_tracing();
    let cli::Cli {
        config: config_path,
        dev,
        command,
    } = parsed;

    match command {
        cli::Command::Version => {
            println!("skiff {}", env!("CARGO_PKG_VERSION"));
            println!(
                "arch={} os={}",
                skiff_pkgfmt::host_arch(),
                std::env::consts::OS
            );
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Health => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::health(&cfg)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Doctor => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::doctor(&cfg);
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginList => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_list(&cfg)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginInstall {
            package,
            yes,
            grants,
            resources,
            allow_unsigned,
        } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_install(&cfg, &package, yes, grants, resources, allow_unsigned)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginRemove { id, delete_data } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_remove(&cfg, &id, delete_data)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginSetEnabled { id, enabled } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_set_enabled(&cfg, &id, enabled)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginConfigList { id } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_config_list(&cfg, &id)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::PluginConfigSet { id, key, value } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::plugin_config_set(&cfg, &id, &key, &value)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::TokenCreate { name, scopes } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::token_create(&cfg, &name, &scopes)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Audit { limit } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::audit(&cfg, limit)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Backup { out } => {
            let cfg = config::Config::load(config_path, dev)?;
            commands::backup(&cfg, out)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Serve => serve(config_path, dev),
    }
}

fn serve(config_path: Option<std::path::PathBuf>, dev: bool) -> Result<ExitCode> {
    let mut cfg = config::Config::load(config_path, dev)?;
    cfg.validate()?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let boot = bootstrap::bootstrap(&cfg)?;
        let state = boot.state.clone();

        println!("Skiff 已启动");
        println!(
            "  监听      : {}{}",
            cfg.listen,
            if cfg.dev_http {
                "（明文开发模式）"
            } else if cfg.plaintext_behind_proxy {
                "（明文，假定可信反代已终止 TLS）"
            } else {
                "（TLS）"
            }
        );
        println!("  数据根    : {}", cfg.data_dir.display());
        println!("  安全入口  : /{}", state.cfg.entry_prefix);
        println!("  配置      : {}", cfg.config_path.display());
        // 只有真 TLS 或反代终止才算 https；dev_http 时明确降级为 http。
        let scheme = if cfg.tls_cert.is_some() || cfg.plaintext_behind_proxy { "https" } else { "http" };
        println!(
            "  登录地址  : {scheme}://{}/{}/login",
            cfg.listen, state.cfg.entry_prefix
        );
        if cfg.plaintext_behind_proxy {
            warn!(
                listen = %cfg.listen,
                proxies = ?cfg.trusted_proxies,
                "非 loopback 明文监听：仅当 trusted_proxies 中的反代确实终止了 TLS 时才安全，请确认前置代理已启用 HTTPS"
            );
        }
        if let Some(fr) = &boot.first_run {
            println!(
                "  初始口令  : {}（用户 {}，首次登录后强制改密）",
                fr.password_path.display(),
                fr.username
            );
        }
        if !state.docker.available() {
            warn!(reason = %state.docker_unavailable_reason(), "Docker 不可用：容器功能降级为只读空态");
        }
        if cfg.allow_unsigned {
            warn!("allow_unsigned=true：未签名包可安装，仅限开发环境");
        }
        if !cfg.allow_reduced_sandbox {
            info!("allow_reduced_sandbox=false：要求完整沙箱的插件会被拒装（landlock/cgroups 未实现）");
        }

        let router = skiff_web::router(state.clone());
        server::spawn_supervisor_reaper(state.clone());
        kernel::spawn(state.clone(), cfg.run_dir.clone()).await;

        let result = server::serve(&cfg, router).await;
        state.supervisor.stop_all().await;
        if let Err(e) = &result {
            error!("服务退出: {e:#}");
        }
        result.map(|_| ExitCode::SUCCESS)
    })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("SKIFF_LOG_LEVEL")
        .unwrap_or_else(|_| EnvFilter::new("info,skiff_web=info,skiff_plugin=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false)
        .try_init();
}
