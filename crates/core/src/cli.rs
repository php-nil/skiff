//! 命令行入口：`skiff serve|health|doctor|version|plugin|trust|token|audit|backup`。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Cli {
    pub config: Option<PathBuf>,
    pub dev: bool,
    /// 首启新管理员强制首次登录改密（默认关闭，仅对新建管理员生效）。
    pub require_initial_password_change: bool,
    pub command: Command,
}

#[derive(Debug, Clone)]
pub enum Command {
    Serve,
    Health,
    Doctor,
    Version,
    PluginInstall {
        package: PathBuf,
        yes: bool,
        grants: Vec<String>,
        resources: Vec<String>,
        allow_unsigned: bool,
    },
    PluginList,
    /// 把公钥文件写入信任库（安装第三方/官方插件前的显式一步）
    TrustImport {
        key_file: PathBuf,
        label: String,
    },
    PluginRemove {
        id: String,
        delete_data: bool,
    },
    PluginSetEnabled {
        id: String,
        enabled: bool,
    },
    /// 列出插件 `config_schema` 声明项与当前值（secret 打码）
    PluginConfigList {
        id: String,
    },
    /// 写入一项配置（是否 secret 由 manifest 的 `config_schema` 决定）
    PluginConfigSet {
        id: String,
        key: String,
        value: String,
    },
    TokenCreate {
        name: String,
        /// 逗号分隔的 scope；空 = 默认 `docker:read`（最小可用权限）
        scopes: Vec<String>,
    },
    Audit {
        limit: usize,
    },
    Backup {
        out: Option<PathBuf>,
    },
    /// 一次性把站点文档根 `index/` 的属主对齐到统一身份（幂等）
    SitesChownIndex {
        dry_run: bool,
    },
}

pub const HELP: &str = "\
Skiff（轻舟）· 极简 Linux 服务器管理面板

用法: skiff [--config <path>] [--dev] <command>

命令:
  serve                     启动 Web 管理服务（默认）
  health                    探测本机 /healthz（退出码 0 = 健康）
  doctor                    检查数据目录/配置/TLS/Docker socket（只读）
  version                   打印版本
  plugin install <pkg>      安装 .wbpkg（--yes 非交互，默认授予 manifest 声明权限与
                            resource_hints；--grant/--resource 可覆盖为最小授权）
  plugin list               列出已装插件
  plugin remove <id>        卸载（--delete-data 同时删数据）
  plugin enable|disable <id>
  plugin config <id>        列出 config_schema 声明项与当前值（secret 打码）
  plugin config <id> <key> <value>
                            写入一项配置（secret 与否由 manifest 决定）
  trust import <公钥文件> [--label <标签>]
                            把公钥写入信任库（发布包自带的
                            official/public-key.txt 已由首启自动信任）
  token create <name> [--scope <s>]
                            创建 API Token（仅在创建时打印一次）。scope 取值
                            docker:read|docker:write|plugins:read|*，可重复或用逗号分隔；
                            缺省 docker:read（最小权限）。docker:write 蕴含 docker:read
  audit [n]                 输出最近 n 条审计（默认 20）
  backup [--out <path>]     备份数据库与插件数据（VACUUM INTO）
  sites chown-index [--dry-run]
                            把 /www/sites/<域名>/index 的属主对齐到统一站点身份
                            （默认 201000:201000；幂等，跳过符号链接，逐站审计）

全局选项:
  --config <path>           配置文件路径（默认 /etc/skiff/config.toml）
  --dev                     loopback 明文开发模式（等价 allow_insecure_http = true）
  --require-initial-password-change
                            新管理员首登强制改密（默认关闭；已有管理员不受影响）
  -h, --help                显示帮助
";

pub fn parse(args: Vec<String>) -> Result<Cli, String> {
    let mut config = None;
    let mut dev = false;
    let mut require_initial_password_change = false;
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    let mut yes = false;
    let mut delete_data = false;
    let mut grants = Vec::new();
    let mut resources = Vec::new();
    let mut scopes: Vec<String> = Vec::new();
    let mut allow_unsigned = false;
    let mut out: Option<PathBuf> = None;
    let mut label = "manual".to_string();
    let mut dry_run = false;

    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--config" => {
                i += 1;
                config = Some(PathBuf::from(args.get(i).ok_or("--config 缺少参数")?));
            }
            "--dev" => dev = true,
            "--require-initial-password-change" => require_initial_password_change = true,
            "--yes" | "-y" => yes = true,
            "--delete-data" => delete_data = true,
            "--allow-unsigned" => allow_unsigned = true,
            "--dry-run" => dry_run = true,
            "--grant" => {
                i += 1;
                grants.push(args.get(i).ok_or("--grant 缺少参数")?.clone());
            }
            "--resource" => {
                i += 1;
                resources.push(args.get(i).ok_or("--resource 缺少参数")?.clone());
            }
            "--scope" => {
                i += 1;
                scopes.push(args.get(i).ok_or("--scope 缺少参数")?.clone());
            }
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(args.get(i).ok_or("--out 缺少参数")?));
            }
            "--label" => {
                i += 1;
                label = args.get(i).ok_or("--label 缺少参数")?.clone();
            }
            "-h" | "--help" => return Err(HELP.into()),
            other if other.starts_with("--") => {
                return Err(format!("未知选项: {other}\n\n{HELP}"));
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    if positional.is_empty() {
        return Ok(Cli {
            config,
            dev,
            require_initial_password_change,
            command: Command::Serve,
        });
    }
    let command = match positional[0].as_str() {
        "serve" => Command::Serve,
        "health" => Command::Health,
        "doctor" => Command::Doctor,
        "version" | "-V" | "--version" => Command::Version,
        "plugin" => match positional.get(1).map(|s| s.as_str()) {
            Some("install") => Command::PluginInstall {
                package: PathBuf::from(positional.get(2).ok_or("plugin install 需要 .wbpkg 路径")?),
                yes,
                grants,
                resources,
                allow_unsigned,
            },
            Some("list") => Command::PluginList,
            Some("remove") => Command::PluginRemove {
                id: positional
                    .get(2)
                    .ok_or("plugin remove 需要插件 id")?
                    .clone(),
                delete_data,
            },
            Some("enable") => Command::PluginSetEnabled {
                id: positional
                    .get(2)
                    .ok_or("plugin enable 需要插件 id")?
                    .clone(),
                enabled: true,
            },
            Some("disable") => Command::PluginSetEnabled {
                id: positional
                    .get(2)
                    .ok_or("plugin disable 需要插件 id")?
                    .clone(),
                enabled: false,
            },
            // `config <id>` 列值；`config <id> <key> <value>` 写值。三项都有才是写。
            Some("config") => {
                let id = positional
                    .get(2)
                    .ok_or("plugin config 需要插件 id")?
                    .clone();
                match (positional.get(3), positional.get(4)) {
                    (None, _) => Command::PluginConfigList { id },
                    (Some(key), Some(value)) => Command::PluginConfigSet {
                        id,
                        key: key.clone(),
                        value: value.clone(),
                    },
                    (Some(_), None) => {
                        return Err(format!(
                            "plugin config 写入需要 <key> <value> 两个参数\n\n{HELP}"
                        ))
                    }
                }
            }
            other => return Err(format!("未知 plugin 子命令: {other:?}\n\n{HELP}")),
        },
        "trust" => match positional.get(1).map(|s| s.as_str()) {
            Some("import") => Command::TrustImport {
                key_file: PathBuf::from(positional.get(2).ok_or("trust import 需要公钥文件路径")?),
                label,
            },
            other => return Err(format!("未知 trust 子命令: {other:?}\n\n{HELP}")),
        },
        "token" => match positional.get(1).map(|s| s.as_str()) {
            Some("create") => Command::TokenCreate {
                name: positional.get(2).cloned().unwrap_or_else(|| "cli".into()),
                scopes,
            },
            other => return Err(format!("未知 token 子命令: {other:?}\n\n{HELP}")),
        },
        "audit" => Command::Audit {
            limit: positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(20),
        },
        "backup" => Command::Backup { out },
        "sites" => match positional.get(1).map(|s| s.as_str()) {
            Some("chown-index") => Command::SitesChownIndex { dry_run },
            other => return Err(format!("未知 sites 子命令: {other:?}\n\n{HELP}")),
        },
        other => return Err(format!("未知命令: {other}\n\n{HELP}")),
    };
    Ok(Cli {
        config,
        dev,
        require_initial_password_change,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_is_serve() {
        let c = parse(args(&[])).unwrap();
        assert!(matches!(c.command, Command::Serve));
        assert!(!c.require_initial_password_change, "默认不强制首登改密");
    }

    #[test]
    fn require_initial_password_change_flag_is_parsed() {
        let c = parse(args(&["--require-initial-password-change", "serve"])).unwrap();
        assert!(c.require_initial_password_change);
    }

    #[test]
    fn parses_plugin_install_flags() {
        let c = parse(args(&[
            "--config",
            "x.toml",
            "plugin",
            "install",
            "a.wbpkg",
            "--yes",
            "--grant",
            "docker:containers.list",
            "--resource",
            "container:abc",
        ]))
        .unwrap();
        match c.command {
            Command::PluginInstall {
                package,
                yes,
                grants,
                resources,
                ..
            } => {
                assert_eq!(package, PathBuf::from("a.wbpkg"));
                assert!(yes);
                assert_eq!(grants.len(), 1);
                assert_eq!(resources.len(), 1);
            }
            _ => panic!("wrong command"),
        }
        assert_eq!(c.config, Some(PathBuf::from("x.toml")));
    }

    /// `trust import` 是插件安装报错里提示的一步命令，参数必须能原样解析。
    #[test]
    fn parses_trust_import_with_label() {
        let c = parse(args(&[
            "trust",
            "import",
            "/www/skiff/official/public-key.txt",
            "--label",
            "official",
        ]))
        .unwrap();
        match c.command {
            Command::TrustImport { key_file, label } => {
                assert_eq!(
                    key_file,
                    PathBuf::from("/www/skiff/official/public-key.txt")
                );
                assert_eq!(label, "official");
            }
            _ => panic!("wrong command"),
        }
        // 缺公钥文件路径必须报错，不能静默落到 serve。
        assert!(parse(args(&["trust", "import"])).is_err());
        assert!(parse(args(&["trust", "nope"])).is_err());
    }

    #[test]
    fn parses_sites_chown_index_with_dry_run() {
        let c = parse(args(&["sites", "chown-index"])).unwrap();
        assert!(matches!(
            c.command,
            Command::SitesChownIndex { dry_run: false }
        ));
        let c = parse(args(&["sites", "chown-index", "--dry-run"])).unwrap();
        assert!(matches!(
            c.command,
            Command::SitesChownIndex { dry_run: true }
        ));
        assert!(parse(args(&["sites", "nope"])).is_err());
    }

    #[test]
    fn missing_arg_is_error() {
        assert!(parse(args(&["plugin", "install"])).is_err());
        assert!(parse(args(&["--config"])).is_err());
        assert!(parse(args(&["nope"])).is_err());
    }

    #[test]
    fn plugin_config_splits_list_and_set() {
        let c = parse(args(&["plugin", "config", "demo"])).unwrap();
        assert!(matches!(c.command, Command::PluginConfigList { .. }));
        let c = parse(args(&[
            "plugin",
            "config",
            "demo",
            "api_base",
            "https://x/y",
        ]))
        .unwrap();
        match c.command {
            Command::PluginConfigSet { id, key, value } => {
                assert_eq!(id, "demo");
                assert_eq!(key, "api_base");
                assert_eq!(value, "https://x/y");
            }
            _ => panic!("wrong command"),
        }
        // 只给 key 不给 value 是写错，不能当成列值静默通过
        assert!(parse(args(&["plugin", "config", "demo", "api_base"])).is_err());
    }
}
