//! 非服务型命令：健康探测、插件管理、令牌、审计、备份。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use skiff_store::Db;

use crate::config::Config;

pub fn health(cfg: &Config) -> Result<()> {
    let addr = cfg.probe_addr();
    let raw = crate::server::probe(addr)?;
    let status_line = raw.lines().next().unwrap_or_default();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    if status_line.contains("200") {
        println!("{status_line}");
        println!("{body}");
        Ok(())
    } else {
        bail!("健康检查失败: {status_line} {body}");
    }
}

pub fn doctor(cfg: &Config) {
    for (ok, line) in crate::bootstrap::doctor(cfg) {
        println!("{} {}", if ok { "[ok]" } else { "[!!]" }, line);
    }
}

pub fn open_db(cfg: &Config) -> Result<Arc<Db>> {
    let path = cfg.data_dir.join("skiff.db");
    if !path.exists() {
        bail!(
            "数据库不存在: {}（请先执行 skiff serve 完成首启初始化）",
            path.display()
        );
    }
    Ok(Arc::new(Db::open(&path).with_context(|| {
        format!("打开数据库失败: {}", path.display())
    })?))
}

pub fn plugin_list(cfg: &Config) -> Result<()> {
    let db = open_db(cfg)?;
    let plugins = db.list_plugins()?;
    if plugins.is_empty() {
        println!("（没有已安装插件）");
        return Ok(());
    }
    println!(
        "{:<28} {:<10} {:<8} {:<10} {:<10} GRANTS",
        "ID", "VERSION", "API", "ENABLED", "STATUS"
    );
    for p in plugins {
        println!(
            "{:<28} {:<10} {:<8} {:<10} {:<10} {}",
            p.id,
            p.version,
            p.api,
            p.enabled,
            p.status,
            p.grants
                .iter()
                .map(|g| format!("{}:{}", g.kind, g.value))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    Ok(())
}

pub fn plugin_install(
    cfg: &Config,
    package: &Path,
    yes: bool,
    grants: Vec<String>,
    resources: Vec<String>,
    allow_unsigned_flag: bool,
) -> Result<()> {
    let db = open_db(cfg)?;
    if !package.exists() {
        bail!("包不存在: {}", package.display());
    }

    // 信任判定只有一份：`skiff_plugin::trust::resolve`（TOFU 三段式）。
    // 这里只负责把「未导入公钥」翻成 CLI 语境下的提示。
    let allow_unsigned = allow_unsigned_flag || cfg.allow_unsigned;
    let trust = skiff_plugin::resolve_trust(package, &db, allow_unsigned).map_err(|e| match e {
        skiff_plugin::TrustError::UnverifiedKey { key_id } => anyhow::anyhow!(
            "未导入公钥（fingerprint {key_id}）。请先在面板「设置 → 信任公钥」导入，\
                 或开发环境加 --allow-unsigned"
        ),
        other => anyhow::anyhow!(other.to_string()),
    })?;
    // 借用而非移出：`trust` 后面还要交给 `install_options` 组装安装选项。
    let meta = &trust.meta;

    println!("插件: {} ({})", meta.manifest.name, meta.manifest.id);
    println!(
        "版本: {}  API: {}",
        meta.manifest.version, meta.manifest.api
    );
    println!(
        "指纹: {}  签名: {}",
        meta.key_id,
        if meta.signature_verified {
            "已验签"
        } else {
            "未验签"
        }
    );
    println!("能力: {:?}", meta.manifest.requires_str());

    let declared = meta.manifest.permission_strings();
    let mut granted = Vec::new();
    if yes {
        // --yes：权限默认全拒，只授予显式 --grant（§4.5）
        for g in &grants {
            if !declared.contains(g) {
                bail!("--grant {g} 未在 manifest 中声明");
            }
            granted.push(g.clone());
        }
        if !declared.is_empty() && granted.is_empty() {
            println!("提示：--yes 模式下权限默认全拒；如需授权请显式 --grant cap:verb");
        }
    } else {
        if !is_tty() {
            bail!("非交互环境请使用 --yes（权限默认全拒，需显式 --grant）");
        }
        for g in &declared {
            print!("授予 {g} ? [y/N] ");
            use std::io::Write;
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if matches!(line.trim(), "y" | "Y" | "yes") {
                granted.push(g.clone());
            }
        }
    }

    let opts = skiff_plugin::install_options(
        &trust,
        granted.clone(),
        resources,
        allow_unsigned,
        cfg.form,
        cfg.allow_reduced_sandbox,
    );
    let info = match skiff_plugin::install_package(&db, &cfg.data_dir, package, &opts) {
        Ok(info) => info,
        Err(e) => {
            let _ = db.audit(
                "cli",
                "local",
                "plugin.install",
                &meta.manifest.id,
                "fail",
                &e.to_string(),
            );
            return Err(anyhow::anyhow!(e.to_string()));
        }
    };

    // 与 Web 装包同口径留痕：谁在什么时候装了哪个版本、授了哪些权限。
    let _ = db.audit(
        "cli",
        "local",
        "plugin.install",
        &info.id,
        "ok",
        &format!(
            "version={} key_id={} grants={}",
            info.version,
            info.key_id,
            info.grants.len()
        ),
    );

    println!(
        "已安装 {} {}（grants={}）",
        info.id,
        info.version,
        info.grants.len()
    );
    for w in info.warnings {
        println!("警告: {w}");
    }
    Ok(())
}

pub fn plugin_remove(cfg: &Config, id: &str, delete_data: bool) -> Result<()> {
    let db = open_db(cfg)?;
    skiff_plugin::uninstall_package(&db, &cfg.data_dir, id, delete_data)?;
    println!("已卸载 {id}（delete_data={delete_data}）");
    Ok(())
}

pub fn plugin_set_enabled(cfg: &Config, id: &str, enabled: bool) -> Result<()> {
    let db = open_db(cfg)?;
    if db.plugin(id)?.is_none() {
        bail!("插件不存在: {id}");
    }
    db.set_plugin_enabled(id, enabled)?;
    db.set_plugin_status(id, if enabled { "installed" } else { "disabled" }, "")?;
    println!("{id} enabled={enabled}");
    Ok(())
}

/// 列出插件 `config_schema` 声明项与当前值。secret 项打码，避免管理台日志/录屏泄露。
pub fn plugin_config_list(cfg: &Config, id: &str) -> Result<()> {
    let (_, schema, stored) = load_config_view(cfg, id)?;
    if schema.is_empty() {
        println!("（{id} 未声明 config_schema）");
        return Ok(());
    }
    println!("{:<24} {:<9} {:<7} VALUE", "KEY", "TYPE", "SECRET");
    for (key, field) in &schema {
        let (value, stored_secret) = match stored.iter().find(|(k, _, _)| k == key) {
            Some((_, v, s)) => (v.clone(), *s),
            None => (field.default.clone().unwrap_or_default(), false),
        };
        let shown = if (field.secret || stored_secret) && !value.is_empty() {
            "******".to_string()
        } else {
            value
        };
        println!(
            "{:<24} {:<9} {:<7} {}",
            key,
            field.kind,
            if field.secret { "yes" } else { "no" },
            shown
        );
    }
    Ok(())
}

/// 写入一项配置。键必须在 manifest 的 `config_schema` 里——否则插件永远读不到，
/// 写进去只会变成一行谁也不知道用途的死数据；`secret` 标记同样以 manifest 为准，
/// 不允许调用方自行指定（漏传就等于明文落库）。
pub fn plugin_config_set(cfg: &Config, id: &str, key: &str, value: &str) -> Result<()> {
    let (db, schema, _) = load_config_view(cfg, id)?;
    let Some(field) = schema.get(key) else {
        bail!("插件 {id} 未声明配置项 {key}（manifest 的 config_schema 里没有）");
    };
    field
        .check_value(key, value)
        .with_context(|| format!("配置项 {key} 取值非法"))?;
    db.set_plugin_config(id, key, value, field.secret)?;
    db.audit(
        "cli",
        "local",
        "plugin.config.set",
        id,
        "ok",
        &format!("{key}{}", if field.secret { "（secret）" } else { "" }),
    )?;
    println!(
        "已写入 {id}/{key}{}",
        if field.secret {
            "（secret，不回显）"
        } else {
            ""
        }
    );
    Ok(())
}

/// `load_config_view` 的返回类型：连接句柄 + 声明 schema + 已存值。
type ConfigView = (
    Arc<Db>,
    std::collections::BTreeMap<String, skiff_pkgfmt::manifest::ConfigField>,
    Vec<(String, String, bool)>,
);

/// 取出「插件记录 + 声明的 schema + 已存值」三件套，列表与写入共用。
fn load_config_view(cfg: &Config, id: &str) -> Result<ConfigView> {
    let db = open_db(cfg)?;
    let rec = db
        .plugin(id)?
        .with_context(|| format!("插件不存在: {id}"))?;
    let schema = skiff_pkgfmt::Manifest::parse(&rec.manifest)
        .with_context(|| format!("插件 {id} 的 manifest 解析失败"))?
        .config_schema;
    let stored = db.plugin_config(id)?;
    Ok((db, schema, stored))
}

/// `token create`：scope 默认 `docker:read`（最小可用权限）。
///
/// 历史值 `api` 不是合法 scope，曾让 `scopes` 这一列变成只存不判的摆设；现在解析
/// 严格化，拼错会直接报错而不是静默生成一个什么都干不了的 token。
pub fn token_create(cfg: &Config, name: &str, scopes: &[String]) -> Result<()> {
    let db = open_db(cfg)?;
    let admin = db
        .user_by_name("admin")?
        .ok_or_else(|| anyhow::anyhow!("没有 admin 用户，请先启动一次面板"))?;
    let requested = if scopes.is_empty() {
        "docker:read".to_string()
    } else {
        scopes.join(",")
    };
    let set = skiff_model::ScopeSet::parse(&requested)
        .map_err(|e| anyhow::anyhow!("{e}（可用: docker:read, docker:write, plugins:read, *）"))?;
    let wire = set.to_wire();
    let token = skiff_identity::new_token();
    let hash = skiff_identity::sha256_hex(&token);
    let id = skiff_identity::new_token()[..16].to_string();
    db.create_api_token(&id, admin.id, name, &hash, &wire)?;
    db.audit("cli", "local", "token.create", name, "ok", &wire)?;
    println!("API Token（只显示这一次）: {token}");
    println!("scope: {wire}（仍受该用户角色约束）");
    println!("用法: curl -H 'Authorization: Bearer <token>' http://127.0.0.1:18080/api/v1/docker/containers");
    Ok(())
}

pub fn audit(cfg: &Config, limit: usize) -> Result<()> {
    let db = open_db(cfg)?;
    for a in db.recent_audit(limit)? {
        let ts = chrono::DateTime::from_timestamp(a.ts, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        println!(
            "{ts}  {:<10} {:<16} {:<28} {:<20} {:<6} {}",
            a.actor, a.ip, a.action, a.target, a.result, a.detail
        );
    }
    Ok(())
}

pub fn backup(cfg: &Config, out: Option<PathBuf>) -> Result<()> {
    let db = open_db(cfg)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dir = cfg.data_dir.join("backup");
    std::fs::create_dir_all(&dir)?;
    let dest = out.unwrap_or_else(|| dir.join(format!("skiff-{stamp}.db")));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    db.backup_to(&dest)?;
    db.audit(
        "cli",
        "local",
        "backup",
        &dest.display().to_string(),
        "ok",
        "",
    )?;
    println!("数据库已备份: {}", dest.display());

    // 插件 data/（cache/run 不卷入）；活动库禁止 tar，这里只复制已落盘数据目录
    let plugins_root = cfg.data_dir.join("plugins");
    if plugins_root.is_dir() {
        let target = dest.with_extension("plugins");
        copy_plugin_data(&plugins_root, &target)?;
        println!("插件数据已备份: {}", target.display());
    }

    // 保留最近 3 份（§5.2）
    prune_backups(&dir, 3)?;
    Ok(())
}

fn copy_plugin_data(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let id = entry.file_name();
        if id.to_string_lossy().starts_with('.') {
            continue;
        }
        let data = entry.path().join("data");
        if data.is_dir() {
            copy_dir(&data, &dest.join(&id))?;
        }
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn prune_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut dbs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "db").unwrap_or(false))
        .collect();
    dbs.sort();
    while dbs.len() > keep {
        let old = dbs.remove(0);
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_dir_all(old.with_extension("plugins"));
    }
    Ok(())
}

#[cfg(unix)]
fn is_tty() -> bool {
    // SAFETY: isatty 只读 fd 状态。
    unsafe { libc::isatty(0) == 1 }
}

#[cfg(not(unix))]
fn is_tty() -> bool {
    false
}
