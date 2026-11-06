//! 运行环境与 PHP 扩展清单的**单一来源**。
//!
//! 版本目录、默认实例名、镜像名与 PHP 扩展的安装脚本都只在这里定义：
//! Web 下拉框与勾选项由它渲染，安装/重建也按它执行，避免前后端各维护一份。

use skiff_store::{KIND_GO, KIND_NODE, KIND_PHP};

use crate::sites::{DEFAULT_PHP_IMAGE, DEFAULT_PHP_VERSION};

/// 一个可创建运行环境类型及其可选的次版本号。
#[derive(Debug, Clone, Copy)]
pub struct RuntimeKindDef {
    pub kind: &'static str,
    pub label: &'static str,
    /// 大版本 → 小版本：只放完整版本号，前端不提供手填。
    pub versions: &'static [&'static str],
}

/// 可创建的类型目录。Node.js / Go 为 P2 预留：可登记，但不安装容器。
pub const RUNTIME_KINDS: &[RuntimeKindDef] = &[
    RuntimeKindDef {
        kind: KIND_PHP,
        label: "PHP",
        versions: &["8.5", "8.4", "8.3"],
    },
    RuntimeKindDef {
        kind: KIND_NODE,
        label: "Node.js",
        versions: &["22", "20", "18"],
    },
    RuntimeKindDef {
        kind: KIND_GO,
        label: "Go",
        versions: &["1.24", "1.23", "1.22"],
    },
];

pub fn kind_def(kind: &str) -> Option<&'static RuntimeKindDef> {
    RUNTIME_KINDS.iter().find(|d| d.kind == kind)
}

pub fn kind_label(kind: &str) -> String {
    kind_def(kind)
        .map(|d| d.label.to_string())
        .unwrap_or_else(|| kind.to_string())
}

pub fn version_supported(kind: &str, version: &str) -> bool {
    kind_def(kind).is_some_and(|d| d.versions.contains(&version.trim()))
}

/// 名称默认跟随版本（`php-8.5` / `node-22` / `go-1.24`）；唯一性仍由库里同类型校验。
pub fn default_name(kind: &str, version: &str) -> String {
    format!("{}-{}", kind, version.trim())
}

pub fn image_for(kind: &str, version: &str, configured_php_image: &str) -> String {
    let version = version.trim();
    match kind {
        KIND_PHP if version == DEFAULT_PHP_VERSION => configured_php_image.to_string(),
        KIND_PHP => format!("php:{version}-fpm-alpine"),
        KIND_NODE => format!("node:{version}-alpine"),
        KIND_GO => format!("golang:{version}-alpine"),
        _ => format!("{kind}:{version}"),
    }
}

/// P1 只实现 PHP；Node.js/Go 可以落库但不会安装容器。
pub fn is_implemented(kind: &str) -> bool {
    kind == KIND_PHP
}

/// 默认 PHP 镜像名（P1 默认版本）。
pub fn default_php_image() -> &'static str {
    DEFAULT_PHP_IMAGE
}

/// PHP 扩展定义：勾选界面、安装脚本与开关都复用这一处。
#[derive(Debug, Clone, Copy)]
pub struct PhpExtensionDef {
    pub key: &'static str,
    pub label: &'static str,
    /// 镜像自带并默认启用（opcache / curl）。
    pub builtin: bool,
    /// 新环境默认勾选。
    pub default_on: bool,
    pub description: &'static str,
    /// 启用脚本（在运行中的 PHP 容器内以 `sh -lc` 执行）。
    pub enable_script: &'static str,
}

pub const PHP_EXTENSIONS: &[PhpExtensionDef] = &[
    PhpExtensionDef {
        key: "opcache",
        label: "OPcache",
        builtin: true,
        default_on: true,
        description: "脚本字节码缓存，镜像内置",
        enable_script: "docker-php-ext-enable opcache",
    },
    PhpExtensionDef {
        key: "curl",
        label: "cURL",
        builtin: true,
        default_on: true,
        description: "HTTP 客户端，镜像内置",
        enable_script: "docker-php-ext-enable curl",
    },
    PhpExtensionDef {
        key: "pdo_mysql",
        label: "PDO MySQL",
        builtin: false,
        default_on: true,
        description: "PDO 的 MySQL 驱动",
        enable_script:
            "docker-php-ext-install -j\"$(nproc)\" pdo_mysql && docker-php-ext-enable pdo_mysql",
    },
    PhpExtensionDef {
        key: "mysqli",
        label: "MySQLi",
        builtin: false,
        default_on: false,
        description: "MySQL 原生扩展",
        enable_script:
            "docker-php-ext-install -j\"$(nproc)\" mysqli && docker-php-ext-enable mysqli",
    },
    PhpExtensionDef {
        key: "gd",
        label: "GD",
        builtin: false,
        default_on: false,
        description: "图像处理（libpng / freetype / jpeg）",
        enable_script: "apk add --no-cache --virtual .skiff-build libpng-dev libjpeg-turbo-dev \
                        freetype-dev && docker-php-ext-configure gd --with-freetype --with-jpeg \
                        && docker-php-ext-install -j\"$(nproc)\" gd \
                        && docker-php-ext-enable gd && apk del .skiff-build",
    },
    PhpExtensionDef {
        key: "zip",
        label: "Zip",
        builtin: false,
        default_on: false,
        description: "压缩包读写（libzip）",
        enable_script: "apk add --no-cache --virtual .skiff-build libzip-dev \
                        && docker-php-ext-install -j\"$(nproc)\" zip \
                        && docker-php-ext-enable zip && apk del .skiff-build",
    },
    PhpExtensionDef {
        key: "bcmath",
        label: "BCMath",
        builtin: false,
        default_on: false,
        description: "任意精度数学",
        enable_script:
            "docker-php-ext-install -j\"$(nproc)\" bcmath && docker-php-ext-enable bcmath",
    },
    PhpExtensionDef {
        key: "intl",
        label: "Intl",
        builtin: false,
        default_on: false,
        description: "国际化（ICU）",
        enable_script: "apk add --no-cache --virtual .skiff-build icu-dev \
                        && docker-php-ext-install -j\"$(nproc)\" intl \
                        && docker-php-ext-enable intl && apk del .skiff-build",
    },
    PhpExtensionDef {
        key: "redis",
        label: "Redis",
        builtin: false,
        default_on: false,
        description: "PECL Redis 客户端",
        // pecl 会就 igbinary / lzf / zstd 等编译选项逐个提问；面板的 docker exec
        // 带 PTY，没人回答就会一直阻塞（实测挂起 20 分钟以上）。用 yes 持续喂空行
        // 取默认值，keep-alive 到 pecl 退出后由 SIGPIPE 结束。
        enable_script: "apk add --no-cache --virtual .skiff-build autoconf g++ make re2c pkgconf \
                        && yes '' | pecl install redis && docker-php-ext-enable redis \
                        && apk del .skiff-build",
    },
];

pub fn php_extension(key: &str) -> Option<&'static PhpExtensionDef> {
    PHP_EXTENSIONS.iter().find(|e| e.key == key)
}

pub fn default_extensions() -> Vec<String> {
    PHP_EXTENSIONS
        .iter()
        .filter(|e| e.default_on)
        .map(|e| e.key.to_string())
        .collect()
}

/// 接受客户端提交的扩展 key 列表：去重、忽略未知项、按目录顺序落库。
pub fn normalize_extensions(raw: &[String]) -> Vec<String> {
    let mut wanted = Vec::new();
    for key in raw {
        let key = key.trim();
        if php_extension(key).is_some() && !wanted.iter().any(|k: &String| k == key) {
            wanted.push(key.to_string());
        }
    }
    PHP_EXTENSIONS
        .iter()
        .filter(|e| wanted.iter().any(|k| k == e.key))
        .map(|e| e.key.to_string())
        .collect()
}

/// 逗号分隔串 → 规范化列表（兼容空串）。
pub fn split_extensions(raw: &str) -> Vec<String> {
    let items: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    normalize_extensions(&items)
}

pub fn join_extensions(list: &[String]) -> String {
    normalize_extensions(list).join(",")
}

/// 停用脚本：官方镜像只有 `docker-php-ext-enable`，**没有** `docker-php-ext-disable`
/// （实测 exit 127 `not found`）。启用时写入的 ini 在 `/usr/local/etc/php/conf.d/`，
/// 删掉 `docker-php-ext-<key>.ini` 即可让容器重启后不再加载该扩展。
pub fn disable_script(key: &str) -> Option<String> {
    php_extension(key).map(|e| {
        format!(
            "rm -f /usr/local/etc/php/conf.d/docker-php-ext-{}.ini",
            e.key
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_catalog_is_dropdown_only() {
        assert!(version_supported(KIND_PHP, "8.5"));
        assert!(version_supported(KIND_PHP, "8.3"));
        assert!(!version_supported(KIND_PHP, "8.2"));
        assert!(!version_supported(KIND_PHP, "8"));
        assert!(version_supported(KIND_NODE, "22"));
        assert!(version_supported(KIND_GO, "1.24"));
        assert!(kind_def("unknown").is_none());
    }

    #[test]
    fn default_name_follows_version() {
        assert_eq!(default_name(KIND_PHP, "8.5"), "php-8.5");
        assert_eq!(default_name(KIND_NODE, "22"), "node-22");
        assert_eq!(default_name(KIND_GO, "1.24"), "go-1.24");
    }

    #[test]
    fn image_names_are_derived() {
        assert_eq!(
            image_for(KIND_PHP, "8.5", DEFAULT_PHP_IMAGE),
            DEFAULT_PHP_IMAGE
        );
        assert_eq!(
            image_for(KIND_PHP, "8.4", DEFAULT_PHP_IMAGE),
            "php:8.4-fpm-alpine"
        );
        assert_eq!(
            image_for(KIND_NODE, "22", DEFAULT_PHP_IMAGE),
            "node:22-alpine"
        );
    }

    #[test]
    fn builtin_extensions_are_flagged() {
        // 内置扩展随 PHP 编入并默认启用，docker-php-ext-enable/disable 都会因找不到
        // .so 而失败（PHP 8.5 实测 exit 1），安装流程据此跳过脚本。
        for key in ["opcache", "curl"] {
            assert!(
                php_extension(key).expect(key).builtin,
                "{key} 应标记为镜像内置"
            );
        }
        for key in ["pdo_mysql", "redis", "gd", "zip", "intl"] {
            assert!(
                !php_extension(key).expect(key).builtin,
                "{key} 需要单独编译，不应标记内置"
            );
        }
    }

    #[test]
    fn pecl_extensions_feed_stdin() {
        // pecl 在 PTY 下会就编译选项逐个提问；不重定向 stdin 会一直阻塞（实测挂起）。
        for e in PHP_EXTENSIONS {
            if e.enable_script.contains("pecl install") {
                assert!(
                    e.enable_script.contains("| pecl install"),
                    "{}：pecl 必须重定向 stdin，否则 PTY 下无人应答会挂起",
                    e.key
                );
            }
        }
        assert!(php_extension("redis")
            .expect("redis")
            .enable_script
            .contains("yes ''"));
    }

    #[test]
    fn disable_scripts_remove_ini_files() {
        // 官方镜像没有 docker-php-ext-disable（实测 exit 127）；停用只能删 ini。
        for key in ["pdo_mysql", "redis", "gd", "zip", "intl"] {
            let script = disable_script(key).expect(key);
            assert!(
                !script.contains("docker-php-ext-disable"),
                "{key}：官方镜像没有 docker-php-ext-disable"
            );
            assert!(
                script.contains(&format!("/conf.d/docker-php-ext-{key}.ini")),
                "{key}：停用脚本应删除对应 ini"
            );
        }
    }

    #[test]
    fn extension_catalog_is_single_source() {
        let keys: Vec<&str> = PHP_EXTENSIONS.iter().map(|e| e.key).collect();
        for expected in [
            "opcache",
            "redis",
            "mysqli",
            "pdo_mysql",
            "gd",
            "zip",
            "bcmath",
            "curl",
            "intl",
        ] {
            assert!(keys.contains(&expected), "缺少扩展 {expected}");
        }
        for e in PHP_EXTENSIONS {
            assert!(!e.enable_script.trim().is_empty(), "{} 缺安装脚本", e.key);
            assert!(disable_script(e.key).is_some());
        }
    }

    #[test]
    fn normalize_dedupes_and_orders_by_catalog() {
        let raw = vec![
            "gd".to_string(),
            "opcache".to_string(),
            "gd".to_string(),
            "bogus".to_string(),
        ];
        assert_eq!(normalize_extensions(&raw), vec!["opcache", "gd"]);
        assert_eq!(split_extensions("gd, opcache,,"), vec!["opcache", "gd"]);
        assert_eq!(
            join_extensions(&["gd".into(), "opcache".into()]),
            "opcache,gd"
        );
    }
}
