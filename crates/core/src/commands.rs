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

/// 把公钥文件写入信任库（`skiff trust import`）。
///
/// 这是插件安装报错里给出的一步命令；发布包自带的
/// `official/public-key.txt` 已由首启自动信任，通常不需要手动执行。
pub fn trust_import(cfg: &Config, key_file: &Path, label: &str) -> Result<()> {
    let db = open_db(cfg)?;
    let raw = std::fs::read_to_string(key_file)
        .with_context(|| format!("读取公钥文件失败: {}", key_file.display()))?;
    let hex = raw.trim();
    if hex.is_empty() {
        bail!("公钥文件为空: {}", key_file.display());
    }
    let key =
        skiff_pkgfmt::decode_public_key(hex).map_err(|e| anyhow::anyhow!("公钥格式无效: {e}"))?;
    let key_id = skiff_pkgfmt::key_id_of(&key.to_bytes());
    let existed = db.trusted_key(&key_id)?.is_some();
    db.trust_key(&key_id, hex, label)?;
    if existed {
        println!("公钥已存在（key_id {key_id}），未覆盖");
    } else {
        println!(
            "已导入公钥 key_id={key_id} label={label} 文件={}",
            key_file.display()
        );
    }
    Ok(())
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

/// 把「公钥未信任」翻译成可直接执行的下一步。
///
/// 区分两种情况：
/// - 包目录自带 `public-key.txt`：给出真实路径 + `skiff trust import` 一步命令；
/// - 包里没带公钥：给出预期路径，提示先把公钥放到该处，再执行同一命令。
fn trust_hint(package: &Path, key_id: &str) -> String {
    let key_path = package
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("public-key.txt");
    if key_path.is_file() {
        format!(
            "包自带公钥但尚未信任（fingerprint {key_id}）。执行以下命令后重试：\n  \
             skiff trust import {} --label official",
            key_path.display()
        )
    } else {
        format!(
            "未导入公钥（fingerprint {key_id}），且包目录未自带 public-key.txt（预期路径：{}）。\n\
             请从发布方取得公钥并放到该路径，然后执行：\n  \
             skiff trust import {} --label official\n\
             （也可在面板「设置 → 信任公钥」导入；开发环境可加 --allow-unsigned）",
            key_path.display(),
            key_path.display()
        )
    }
}

/// 非交互安装（`--yes`）的选择口径：未显式指定时采用 manifest 声明值，
/// 保证官方包「装完即可用」；显式 `--grant`/`--resource` 覆盖为最小授权。
fn resolve_yes_selection(
    declared: &[String],
    hints: &[String],
    explicit_grants: &[String],
    explicit_resources: &[String],
) -> Result<(Vec<String>, Vec<String>)> {
    let granted = if explicit_grants.is_empty() {
        declared.to_vec()
    } else {
        for g in explicit_grants {
            if !declared.contains(g) {
                bail!("--grant {g} 未在 manifest 中声明");
            }
        }
        explicit_grants.to_vec()
    };
    let object_grants = if explicit_resources.is_empty() {
        hints.to_vec()
    } else {
        explicit_resources.to_vec()
    };
    Ok((granted, object_grants))
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
        skiff_plugin::TrustError::UnverifiedKey { key_id } => {
            anyhow::anyhow!(trust_hint(package, &key_id))
        }
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
    let hints = meta.manifest.resource_hints.clone();
    let mut granted = Vec::new();
    let mut object_grants = Vec::new();
    // 重装/升级同一插件：既有授权必须保留，不再逐条确认，也不按 manifest
    // 默认值重置。`install_package` 会按插件 id 从库内取回并按新声明集过滤。
    let existing = db.plugin(&meta.manifest.id)?;
    if let Some(prev) = &existing {
        println!(
            "重装/升级: 沿用已授权 {} 项（不重新确认）；新 manifest 新增声明的权限由安装器按声明补齐，\
             新增敏感动词会写审计 plugin.grant_upgrade",
            prev.grants.len()
        );
        if !grants.is_empty() || !resources.is_empty() {
            println!(
                "警告: 重装/升级不改动既有授权，--grant/--resource 已忽略；\
                 如需缩减授权请 remove 后重装"
            );
        }
    } else if yes {
        // --yes：未显式指定时采用 manifest 声明值（官方包「装完即可用」）；
        // 给了 --grant/--resource 就按显式清单来，便于最小授权安装。
        let (g, r) = resolve_yes_selection(&declared, &hints, &grants, &resources)?;
        granted = g;
        object_grants = r;
    } else {
        if !is_tty() {
            bail!("非交互环境请使用 --yes（默认授予 manifest 声明权限与 resource_hints）");
        }
        for g in &declared {
            print!("授予 {g} ? [Y/n] ");
            use std::io::Write;
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !matches!(line.trim(), "n" | "N" | "no") {
                granted.push(g.clone());
            }
        }
        for h in &hints {
            print!("按默认建议授权 {h} ? [Y/n] ");
            use std::io::Write;
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !matches!(line.trim(), "n" | "N" | "no") {
                object_grants.push(h.clone());
            }
        }
    }

    let opts = skiff_plugin::install_options(
        &trust,
        granted,
        object_grants,
        allow_unsigned,
        cfg.form,
        cfg.allow_reduced_sandbox,
        "cli",
    );
    let info = match skiff_plugin::install_package(&db, &cfg.data_dir, package, &opts) {
        Ok(info) => info,
        Err(e) => {
            if let Err(ae) = db.audit(
                "cli",
                "local",
                "plugin.install",
                &meta.manifest.id,
                "fail",
                &e.to_string(),
            ) {
                tracing::error!(error = %ae, "插件安装失败审计写入失败");
            }
            return Err(anyhow::anyhow!(e.to_string()));
        }
    };

    // 与 Web 装包同口径留痕：谁在什么时候装了哪个版本、授了哪些权限。
    if let Err(e) = db.audit(
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
    ) {
        tracing::error!(error = %e, "插件安装审计写入失败");
    }

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
    // 卸载即回收该插件名下的内核任务（manifest 种子任务 + cron 等代建的同 source 任务）；
    // Web 卸载走 `TaskService::drop_plugin_tasks`，CLI 直连库时在这里清掉，避免残留行
    // 让「重装后任务仍在跑」或「升级后 spec 仍是旧的」。
    let source = format!("plugin:{id}");
    let mut dropped = 0usize;
    for task in db.scheduled_tasks()? {
        if task.source == source {
            db.delete_scheduled_task(&task.id)?;
            dropped += 1;
        }
    }
    println!("已卸载 {id}（delete_data={delete_data}，回收任务 {dropped} 条）");
    Ok(())
}

pub fn plugin_set_enabled(cfg: &Config, id: &str, enabled: bool) -> Result<()> {
    let db = open_db(cfg)?;
    if db.plugin(id)?.is_none() {
        bail!("插件不存在: {id}");
    }
    db.set_plugin_enabled_state(id, enabled)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `--yes` 未显式指定时必须采用 manifest 默认值，否则官方包装完即不可用
    /// （files 无 path: 授权 → 所有 RPC 400；cron 无 scheduler:* → 列表 400）。
    #[test]
    fn yes_install_defaults_to_manifest_declared_permissions_and_hints() {
        let declared = vec!["fs:read".to_string(), "scheduler:read".to_string()];
        let hints = vec!["path:/www".to_string()];

        let (g, r) = resolve_yes_selection(&declared, &hints, &[], &[]).unwrap();
        assert_eq!(g, declared, "未指定 --grant 时应全量授予 manifest 声明权限");
        assert_eq!(r, hints, "未指定 --resource 时应采用 resource_hints");

        // 显式清单覆盖默认值（最小授权安装仍可用）
        let (g, r) = resolve_yes_selection(
            &declared,
            &hints,
            &["fs:read".to_string()],
            &["path:/tmp".to_string()],
        )
        .unwrap();
        assert_eq!(g, vec!["fs:read".to_string()]);
        assert_eq!(r, vec!["path:/tmp".to_string()]);

        // 未声明的权限必须拒绝
        let e = resolve_yes_selection(&declared, &hints, &["docker:run".to_string()], &[])
            .unwrap_err()
            .to_string();
        assert!(e.contains("未在 manifest 中声明"), "{e}");
    }

    /// 安装报错必须把「包自带公钥」与「包里没带公钥」分开，并给出可复制命令。
    #[test]
    fn trust_hint_distinguishes_bundled_and_missing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let official = tmp.path().join("official");
        std::fs::create_dir_all(&official).unwrap();
        let pkg = official.join("demo.wbpkg");
        std::fs::write(&pkg, b"pkg").unwrap();

        // 情况一：包目录自带 public-key.txt → 报错给出真实路径与一步命令
        std::fs::write(official.join("public-key.txt"), "aa").unwrap();
        let hint = trust_hint(&pkg, "key-123");
        assert!(hint.contains("包自带公钥但尚未信任"), "{hint}");
        assert!(hint.contains("key-123"));
        assert!(hint.contains("skiff trust import"));
        assert!(hint.contains(&official.join("public-key.txt").display().to_string()));

        // 情况二：包里没带公钥 → 报错说明缺失，并给出预期路径，不能谎称「包自带」
        std::fs::remove_file(official.join("public-key.txt")).unwrap();
        let hint = trust_hint(&pkg, "key-123");
        assert!(hint.contains("包目录未自带 public-key.txt"), "{hint}");
        assert!(hint.contains("skiff trust import"));
        assert!(hint.contains(&official.join("public-key.txt").display().to_string()));
        assert!(!hint.contains("包自带公钥但尚未信任"));
    }
}
