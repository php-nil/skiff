//! 运行环境与 PHP 扩展清单的**单一来源**。
//!
//! 版本目录、默认实例名、镜像名与 PHP 扩展的安装脚本都只在这里定义：
//! Web 下拉框与勾选项由它渲染，安装/重建也按它执行，避免前后端各维护一份。

use skiff_store::{KIND_GO, KIND_NODE, KIND_PHP, KIND_RUST};

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
    RuntimeKindDef {
        kind: KIND_RUST,
        label: "Rust",
        versions: &["1.83", "1.82", "1.80"],
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

/// 名称默认跟随版本（`php-8.5` / `node-22` / `go-1.24` / `rust-1.83`）；
/// 唯一性仍由库里同类型校验。
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
        KIND_RUST => format!("rust:{version}-slim"),
        _ => format!("{kind}:{version}"),
    }
}

/// P1 实现 PHP 与 Rust；Node.js/Go 可以落库但不会安装容器。
pub fn is_implemented(kind: &str) -> bool {
    matches!(kind, KIND_PHP | KIND_RUST)
}

/// 官方镜像在 `Config.Env` 里声明的真实版本变量。
/// Rust 官方镜像没有稳定变量，返回 `None`（列表回退目标版本并标注）。
pub fn version_env_key(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_PHP => Some("PHP_VERSION"),
        KIND_NODE => Some("NODE_VERSION"),
        KIND_GO => Some("GOLANG_VERSION"),
        _ => None,
    }
}

/// 默认 PHP 镜像名（P1 默认版本）。
pub fn default_php_image() -> &'static str {
    DEFAULT_PHP_IMAGE
}

/// Rust 运行环境固定端口的可分配区间（host 网络，避开 80/443/面板端口）。
pub const RUST_PORT_RANGE: std::ops::RangeInclusive<u16> = 9100..=9199;

/// 为新的 Rust 环境分配端口：区间内最小的未占用端口；区间耗尽返回 None。
pub fn allocate_rust_port(used: &[u16]) -> Option<u16> {
    RUST_PORT_RANGE.into_iter().find(|p| !used.contains(p))
}

/// 扩展来源：镜像内置 / PHP 源码内置（docker-php-ext-install）/ PECL。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSource {
    Builtin,
    Build,
    Pecl,
}

impl ExtSource {
    pub fn label(self) -> &'static str {
        match self {
            ExtSource::Builtin => "镜像内置",
            ExtSource::Build => "源码内置",
            ExtSource::Pecl => "PECL",
        }
    }
}

/// PHP 扩展定义：勾选界面、安装脚本、卸载与分类都复用这一处。
#[derive(Debug, Clone, Copy)]
pub struct PhpExtensionDef {
    pub key: &'static str,
    pub label: &'static str,
    /// 分类：数据库 / 图片 / 加密 / 缓存 / 其他。前端只按这里的值分组。
    pub category: &'static str,
    pub source: ExtSource,
    /// 镜像自带并默认启用（opcache / curl 等），安装流程据此跳过脚本。
    pub builtin: bool,
    /// 新环境默认勾选。
    pub default_on: bool,
    pub description: &'static str,
    /// 编译前需要临时安装的 apk 包（装完 `apk del` 清理）。
    pub deps: &'static [&'static str],
    /// PECL 包名；源码内置扩展为 None。
    pub pecl_package: Option<&'static str>,
    /// `docker-php-ext-configure` 的参数（如 `gd --with-freetype --with-jpeg`）。
    pub configure: Option<&'static str>,
    /// 是否支持在运行中的容器内热装（安装后重启容器生效，无需重建）。
    pub hot: bool,
    /// 从该 PHP 版本起由核心提供（含 8.5 新增扩展）。
    pub since: Option<&'static str>,
    /// 该 PHP 版本起核心不再提供（不含）；配合 `pecl_only_since` 表达迁移。
    pub core_until: Option<&'static str>,
    /// 从该版本起只能通过 PECL 安装（目前 UI 灰显并提示原因）。
    pub pecl_only_since: Option<&'static str>,
}

impl PhpExtensionDef {
    /// 旧的手写依赖 + `docker-php-ext-install` 路径：仅在 mlocati 脚本下载失败时兜底。
    fn legacy_install_script(&self) -> String {
        let mut out = String::new();
        let mut clean = false;
        if !self.deps.is_empty() {
            out.push_str("apk add --no-cache --virtual .skiff-build ");
            out.push_str(&self.deps.join(" "));
            out.push_str(" && ");
            clean = true;
        }
        if let Some(pkg) = self.pecl_package {
            // pecl 在 PTY 下会逐项提问；不重定向 stdin 会阻塞（实测挂起）。
            out.push_str("yes '' | pecl install ");
            out.push_str(pkg);
            out.push_str(" && docker-php-ext-enable ");
            out.push_str(self.key);
        } else {
            if let Some(cfg) = self.configure {
                out.push_str("docker-php-ext-configure ");
                out.push_str(cfg);
                out.push_str(" && ");
            }
            out.push_str("docker-php-ext-install -j\"$(nproc)\" ");
            out.push_str(self.key);
            out.push_str(" && docker-php-ext-enable ");
            out.push_str(self.key);
        }
        if clean {
            out.push_str(" && apk del .skiff-build");
        }
        out
    }

    /// 安装并启用该扩展的 shell 脚本。
    ///
    /// 优先使用 mlocati/docker-php-extension-installer：它按 PHP 版本与基础镜像
    /// 解析系统依赖（alpine 用 apk、debian 用 apt），安装后自动清理，失败保留真实 stderr。
    /// 下载失败时回退到旧的手写依赖表，保证离线/受限网络仍可编译。
    pub fn install_script(&self) -> String {
        if self.builtin {
            // 官方镜像已编译并默认启用，返回空脚本让调用方直接跳过：
            // 重复 `docker-php-ext-enable` 可能触发重复加载或找不到 .so。
            return String::new();
        }
        let pkg = self.pecl_package.unwrap_or(self.key);
        let legacy = self.legacy_install_script();
        format!(
            "set -e\n\
             if ! command -v install-php-extensions >/dev/null 2>&1; then\n\
             \tcurl -fsSL https://github.com/mlocati/docker-php-extension-installer/releases/latest/download/install-php-extensions -o /usr/local/bin/install-php-extensions && chmod +x /usr/local/bin/install-php-extensions || true\n\
             fi\n\
             if command -v install-php-extensions >/dev/null 2>&1; then\n\
             \tinstall-php-extensions {pkg}\n\
             else\n\
             \t{legacy}\n\
             fi"
        )
    }
}

pub const CATEGORY_DB: &str = "数据库";
pub const CATEGORY_IMAGE: &str = "图片";
pub const CATEGORY_CRYPTO: &str = "加密";
pub const CATEGORY_CACHE: &str = "缓存";
pub const CATEGORY_OTHER: &str = "其他";

const fn builtin(
    key: &'static str,
    label: &'static str,
    category: &'static str,
    default_on: bool,
    description: &'static str,
) -> PhpExtensionDef {
    PhpExtensionDef {
        key,
        label,
        category,
        source: ExtSource::Builtin,
        builtin: true,
        default_on,
        description,
        deps: &[],
        pecl_package: None,
        configure: None,
        hot: false,
        since: None,
        core_until: None,
        pecl_only_since: None,
    }
}

const fn build(
    key: &'static str,
    label: &'static str,
    category: &'static str,
    default_on: bool,
    description: &'static str,
) -> PhpExtensionDef {
    PhpExtensionDef {
        key,
        label,
        category,
        source: ExtSource::Build,
        builtin: false,
        default_on,
        description,
        deps: &[],
        pecl_package: None,
        configure: None,
        hot: true,
        since: None,
        core_until: None,
        pecl_only_since: None,
    }
}

/// PHP 扩展目录（单一来源）。覆盖官方镜像 `docker-php-ext-install` 的常用全集，
/// 并为每个需要本地库的扩展标注编译依赖；PECL 项按包名热装。
pub const PHP_EXTENSIONS: &[PhpExtensionDef] = &[
    // ---- 数据库 ----
    PhpExtensionDef {
        deps: &[],
        ..build(
            "pdo_mysql",
            "PDO MySQL",
            CATEGORY_DB,
            true,
            "PDO 的 MySQL 驱动",
        )
    },
    build("mysqli", "MySQLi", CATEGORY_DB, false, "MySQL 原生扩展"),
    PhpExtensionDef {
        deps: &["postgresql-dev"],
        ..build(
            "pdo_pgsql",
            "PDO PostgreSQL",
            CATEGORY_DB,
            false,
            "PDO 的 PostgreSQL 驱动",
        )
    },
    PhpExtensionDef {
        deps: &["postgresql-dev"],
        ..build(
            "pgsql",
            "PostgreSQL",
            CATEGORY_DB,
            false,
            "PostgreSQL 原生扩展",
        )
    },
    // 官方 php 镜像默认编译并启用 sqlite3 / pdo_sqlite（无需再编译）。
    PhpExtensionDef {
        builtin: true,
        source: ExtSource::Builtin,
        ..build(
            "pdo_sqlite",
            "PDO SQLite",
            CATEGORY_DB,
            false,
            "PDO 的 SQLite 驱动，镜像内置",
        )
    },
    PhpExtensionDef {
        builtin: true,
        source: ExtSource::Builtin,
        ..build(
            "sqlite3",
            "SQLite3",
            CATEGORY_DB,
            false,
            "SQLite3 原生扩展，镜像内置",
        )
    },
    PhpExtensionDef {
        deps: &["unixodbc-dev"],
        ..build(
            "pdo_odbc",
            "PDO ODBC",
            CATEGORY_DB,
            false,
            "PDO 的 ODBC 驱动",
        )
    },
    PhpExtensionDef {
        pecl_package: Some("oci8"),
        ..build(
            "oci8",
            "OCI8",
            CATEGORY_DB,
            false,
            "Oracle 数据库驱动（PECL）",
        )
    },
    PhpExtensionDef {
        pecl_package: Some("pdo_oci"),
        ..build(
            "pdo_oci",
            "PDO OCI",
            CATEGORY_DB,
            false,
            "PDO 的 Oracle 驱动（PECL）",
        )
    },
    // ---- 图片 ----
    PhpExtensionDef {
        deps: &["libpng-dev", "libjpeg-turbo-dev", "freetype-dev"],
        configure: Some("gd --with-freetype --with-jpeg"),
        ..build(
            "gd",
            "GD",
            CATEGORY_IMAGE,
            false,
            "图像处理（libpng / freetype / jpeg）",
        )
    },
    build(
        "exif",
        "EXIF",
        CATEGORY_IMAGE,
        false,
        "读取图片 EXIF 元数据",
    ),
    PhpExtensionDef {
        deps: &[
            "imagemagick-dev",
            "autoconf",
            "g++",
            "make",
            "pkgconf",
            "re2c",
        ],
        pecl_package: Some("imagick"),
        ..build(
            "imagick",
            "ImageMagick",
            CATEGORY_IMAGE,
            false,
            "PECL ImageMagick 图像处理",
        )
    },
    // ---- 加密 ----
    builtin(
        "openssl",
        "OpenSSL",
        CATEGORY_CRYPTO,
        true,
        "TLS/加密，镜像内置",
    ),
    PhpExtensionDef {
        builtin: true,
        source: ExtSource::Builtin,
        ..build(
            "sodium",
            "Sodium",
            CATEGORY_CRYPTO,
            false,
            "现代加密库 libsodium，镜像内置",
        )
    },
    PhpExtensionDef {
        deps: &["gmp-dev"],
        ..build("gmp", "GMP", CATEGORY_CRYPTO, false, "任意精度整数（GMP）")
    },
    build(
        "hash",
        "Hash",
        CATEGORY_CRYPTO,
        false,
        "哈希框架，镜像通常已内置",
    ),
    // ---- 缓存 ----
    builtin(
        "opcache",
        "OPcache",
        CATEGORY_CACHE,
        true,
        "脚本字节码缓存，镜像内置",
    ),
    PhpExtensionDef {
        deps: &["autoconf", "g++", "make", "pkgconf", "re2c"],
        pecl_package: Some("apcu"),
        ..build(
            "apcu",
            "APCu",
            CATEGORY_CACHE,
            false,
            "PECL 用户态共享内存缓存",
        )
    },
    PhpExtensionDef {
        deps: &["autoconf", "g++", "make", "pkgconf", "re2c"],
        pecl_package: Some("redis"),
        ..build("redis", "Redis", CATEGORY_CACHE, false, "PECL Redis 客户端")
    },
    PhpExtensionDef {
        deps: &[
            "libmemcached-dev",
            "zlib-dev",
            "autoconf",
            "g++",
            "make",
            "pkgconf",
            "re2c",
        ],
        pecl_package: Some("memcached"),
        ..build(
            "memcached",
            "Memcached",
            CATEGORY_CACHE,
            false,
            "PECL Memcached 客户端",
        )
    },
    PhpExtensionDef {
        deps: &["autoconf", "g++", "make", "pkgconf", "re2c"],
        pecl_package: Some("xdebug"),
        ..build(
            "xdebug",
            "Xdebug",
            CATEGORY_CACHE,
            false,
            "PECL 调试与性能分析（仅开发环境）",
        )
    },
    // ---- 其他 ----
    build("bcmath", "BCMath", CATEGORY_OTHER, false, "任意精度数学"),
    PhpExtensionDef {
        deps: &["libzip-dev"],
        ..build("zip", "Zip", CATEGORY_OTHER, false, "压缩包读写（libzip）")
    },
    PhpExtensionDef {
        deps: &["icu-dev"],
        ..build("intl", "Intl", CATEGORY_OTHER, false, "国际化（ICU）")
    },
    PhpExtensionDef {
        builtin: true,
        source: ExtSource::Builtin,
        ..build(
            "mbstring",
            "Mbstring",
            CATEGORY_OTHER,
            true,
            "多字节字符串，镜像内置",
        )
    },
    builtin(
        "curl",
        "cURL",
        CATEGORY_OTHER,
        true,
        "HTTP 客户端，镜像内置",
    ),
    build(
        "sockets",
        "Sockets",
        CATEGORY_OTHER,
        false,
        "低层 socket 接口",
    ),
    build("soap", "SOAP", CATEGORY_OTHER, false, "SOAP 客户端/服务端"),
    build("ftp", "FTP", CATEGORY_OTHER, false, "FTP 客户端"),
    build("gettext", "Gettext", CATEGORY_OTHER, false, "本地化翻译"),
    PhpExtensionDef {
        deps: &["bzip2-dev"],
        ..build("bz2", "Bzip2", CATEGORY_OTHER, false, "bzip2 压缩")
    },
    build("calendar", "Calendar", CATEGORY_OTHER, false, "日历换算"),
    build(
        "pcntl",
        "PCNTL",
        CATEGORY_OTHER,
        false,
        "进程控制（CLI 场景）",
    ),
    build("posix", "POSIX", CATEGORY_OTHER, false, "POSIX 接口"),
    build("shmop", "Shmop", CATEGORY_OTHER, false, "共享内存操作"),
    build(
        "sysvmsg",
        "SysV Msg",
        CATEGORY_OTHER,
        false,
        "SysV 消息队列",
    ),
    build("sysvsem", "SysV Sem", CATEGORY_OTHER, false, "SysV 信号量"),
    build(
        "sysvshm",
        "SysV Shm",
        CATEGORY_OTHER,
        false,
        "SysV 共享内存",
    ),
    build(
        "tokenizer",
        "Tokenizer",
        CATEGORY_OTHER,
        false,
        "PHP 词法分析，通常已内置",
    ),
    build("xml", "XML", CATEGORY_OTHER, false, "XML 解析"),
    build("dom", "DOM", CATEGORY_OTHER, false, "DOM 文档模型"),
    build("simplexml", "SimpleXML", CATEGORY_OTHER, false, "SimpleXML"),
    build("xmlreader", "XMLReader", CATEGORY_OTHER, false, "XMLReader"),
    build("xmlwriter", "XMLWriter", CATEGORY_OTHER, false, "XMLWriter"),
    PhpExtensionDef {
        deps: &["libxslt-dev"],
        ..build("xsl", "XSL", CATEGORY_OTHER, false, "XSLT 转换（libxslt）")
    },
    PhpExtensionDef {
        deps: &["openldap-dev"],
        ..build("ldap", "LDAP", CATEGORY_OTHER, false, "LDAP 目录服务")
    },
    PhpExtensionDef {
        deps: &["net-snmp-dev"],
        ..build("snmp", "SNMP", CATEGORY_OTHER, false, "SNMP 监控")
    },
    PhpExtensionDef {
        deps: &["tidyhtml-dev"],
        ..build("tidy", "Tidy", CATEGORY_OTHER, false, "HTML 清理与修复")
    },
    PhpExtensionDef {
        core_until: Some("8.4"),
        pecl_only_since: Some("8.4"),
        ..build("imap", "IMAP", CATEGORY_OTHER, false, "IMAP/POP3 邮件")
    },
    PhpExtensionDef {
        core_until: Some("8.4"),
        pecl_only_since: Some("8.4"),
        pecl_package: Some("pspell"),
        ..build("pspell", "Pspell", CATEGORY_OTHER, false, "拼写检查")
    },
];

pub fn php_extension(key: &str) -> Option<&'static PhpExtensionDef> {
    PHP_EXTENSIONS.iter().find(|e| e.key == key)
}

/// 某个 PHP 版本 + 基础镜像下，一个扩展的可用状态（只做 delta，不做全量清单）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtSupport {
    /// 可直接安装（内置项只做 enable）。
    Ok,
    /// 需要临时安装的系统依赖（UI 提示「将临时安装 X/Y」，mlocati 脚本自动处理并清理）。
    NeedsDeps(Vec<String>),
    /// 当前版本/基座不可用，附原因。
    Unsupported(String),
}

pub struct ExtensionEntry {
    pub def: &'static PhpExtensionDef,
    pub support: ExtSupport,
}

/// 版本归一为 `(major, minor)`；`8.5.12` 与 `8.5` 等价。
fn version_pair(version: &str) -> (u32, u32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (major, minor)
}

fn version_at_least(version: &str, threshold: &str) -> bool {
    version_pair(version) >= version_pair(threshold)
}

/// 从镜像引用判断基础镜像：含 `alpine` 即 musl/alpine，否则按 debian(glibc)。
pub fn base_image_of(image: &str) -> &'static str {
    if image.to_ascii_lowercase().contains("alpine") {
        "alpine"
    } else {
        "debian"
    }
}

/// 基础镜像不支持的扩展：Oracle Instant Client 只有 glibc 版，
/// 商业 loader（ionCube/sg16）同理；rdkafka 在 alpine 可用，不在此列。
fn base_unsupported(key: &str, base: &str) -> Option<&'static str> {
    if base != "alpine" {
        return None;
    }
    match key {
        "oci8" | "pdo_oci" => Some(
            "Oracle Instant Client 仅提供 glibc 版本，alpine(musl) 不可用；请改用 debian 基础镜像",
        ),
        "ioncube_loader" | "sg16" => Some("商业 loader 仅提供 glibc 版本，alpine(musl) 不可用"),
        _ => None,
    }
}

/// 按 `(version, base_image)` 计算扩展可用状态。UI 与后端校验共用这一处。
pub fn extension_support(
    def: &'static PhpExtensionDef,
    version: &str,
    base_image: &str,
) -> ExtSupport {
    if let Some(reason) = base_unsupported(def.key, base_image) {
        return ExtSupport::Unsupported(reason.to_string());
    }
    if let Some(since) = def.since {
        if !version_at_least(version, since) {
            return ExtSupport::Unsupported(format!(
                "PHP {version} 未提供 {}；自 PHP {since} 起可用",
                def.key
            ));
        }
    }
    if let Some(until) = def.core_until {
        if version_at_least(version, until) {
            let since = def.pecl_only_since.unwrap_or(until);
            return ExtSupport::Unsupported(format!(
                "PHP {since} 起 {} 已从核心移除并转入 PECL，暂未开放一键安装；如需使用请用 PECL 按名字安装",
                def.key
            ));
        }
    }
    if def.builtin {
        return ExtSupport::Ok;
    }
    if def.deps.is_empty() {
        return ExtSupport::Ok;
    }
    // alpine 需要 apk 依赖；debian 交给 mlocati 用 apt 处理，不在这里列包名。
    if base_image == "alpine" {
        return ExtSupport::NeedsDeps(def.deps.iter().map(|s| s.to_string()).collect());
    }
    ExtSupport::Ok
}

/// 后端二次校验用的错误分类：`EXT_NOT_IN_VERSION` / `EXT_BASE_IMAGE_UNSUPPORTED`。
pub fn extension_block_reason(
    def: &'static PhpExtensionDef,
    version: &str,
    base_image: &str,
) -> Option<(&'static str, String)> {
    if let Some(reason) = base_unsupported(def.key, base_image) {
        return Some(("EXT_BASE_IMAGE_UNSUPPORTED", reason.to_string()));
    }
    match extension_support(def, version, base_image) {
        ExtSupport::Unsupported(reason) => Some(("EXT_NOT_IN_VERSION", reason)),
        _ => None,
    }
}

/// PECL/编译失败输出 → 错误码。只识别「包/版本不存在、下载失败、与 PHP 不兼容」，
/// 其余失败保留原始报错，不做过度归类。
pub fn pecl_failure_code(output: &str) -> Option<&'static str> {
    let o = output.to_ascii_lowercase();
    let not_ready = o.contains("no releases available")
        || o.contains("failed to download")
        || o.contains("could not find package")
        || o.contains("not compatible with php")
        || (o.contains("package") && o.contains("not found"));
    not_ready.then_some("EXT_PECL_NOT_READY")
}

/// 某个版本 + 基座的完整扩展清单（含不可用项，供 UI 灰显与后端二次校验）。
pub fn extensions_for(version: &str, base_image: &str) -> Vec<ExtensionEntry> {
    PHP_EXTENSIONS
        .iter()
        .map(|def| ExtensionEntry {
            def,
            support: extension_support(def, version, base_image),
        })
        .collect()
}

pub fn default_extensions() -> Vec<String> {
    PHP_EXTENSIONS
        .iter()
        .filter(|e| e.default_on)
        .map(|e| e.key.to_string())
        .collect()
}

/// PECL 自定义扩展在库里存成 `pecl:<name>[@version]`，与目录 key 共存。
pub fn pecl_token(name: &str, version: &str) -> String {
    let name = name.trim();
    let version = version.trim();
    if version.is_empty() {
        format!("pecl:{name}")
    } else {
        format!("pecl:{name}@{version}")
    }
}

/// 解析 `pecl:<name>[@version]`；非法名称返回 None（只允许小写字母、数字、下划线）。
pub fn parse_pecl_token(token: &str) -> Option<(String, String)> {
    let rest = token.strip_prefix("pecl:")?;
    let (name, version) = match rest.split_once('@') {
        Some((n, v)) => (n, v),
        None => (rest, ""),
    };
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    if !version.is_empty()
        && !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some((name.to_string(), version.to_string()))
}

/// 任意扩展 token（目录 key 或 pecl token）→ 安装脚本。
pub fn extension_install_script(token: &str) -> Option<String> {
    if let Some(def) = php_extension(token) {
        return Some(def.install_script()).filter(|s| !s.is_empty());
    }
    let (name, version) = parse_pecl_token(token)?;
    let pkg = if version.is_empty() {
        name.clone()
    } else {
        format!("{name}-{version}")
    };
    // PECL 自定义扩展：同样优先 mlocati（依赖解析 + 清理），失败保留真实 stderr；
    // 下载失败回退 `pecl install`（PTY 下用 `yes ''` 防提问挂起）。
    Some(format!(
        "set -e\n\
         if ! command -v install-php-extensions >/dev/null 2>&1; then\n\
         \tcurl -fsSL https://github.com/mlocati/docker-php-extension-installer/releases/latest/download/install-php-extensions -o /usr/local/bin/install-php-extensions && chmod +x /usr/local/bin/install-php-extensions || true\n\
         fi\n\
         if command -v install-php-extensions >/dev/null 2>&1; then\n\
         \tinstall-php-extensions {pkg}\n\
         else\n\
         \tapk add --no-cache --virtual .skiff-build autoconf g++ make pkgconf re2c && yes '' | pecl install {pkg} && docker-php-ext-enable {name} && apk del .skiff-build\n\
         fi"
    ))
}

/// 展示名：目录 key 取 label，PECL token 取 `PECL <name>`。
pub fn extension_label(token: &str) -> String {
    if let Some(def) = php_extension(token) {
        return def.label.to_string();
    }
    match parse_pecl_token(token) {
        Some((name, version)) if !version.is_empty() => format!("PECL {name} {version}"),
        Some((name, _)) => format!("PECL {name}"),
        None => token.to_string(),
    }
}

pub fn extension_category(token: &str) -> String {
    php_extension(token)
        .map(|e| e.category.to_string())
        .unwrap_or_else(|| CATEGORY_OTHER.to_string())
}

/// 接受客户端提交的扩展 token：目录项按目录顺序，PECL token 去重后追加。
pub fn normalize_extensions(raw: &[String]) -> Vec<String> {
    let mut catalog = Vec::new();
    let mut pecl: Vec<String> = Vec::new();
    for token in raw {
        let token = token.trim();
        if let Some(def) = php_extension(token) {
            if !catalog.iter().any(|k: &String| k == def.key) {
                catalog.push(def.key.to_string());
            }
            continue;
        }
        if let Some((name, version)) = parse_pecl_token(token) {
            let token = pecl_token(&name, &version);
            if !pecl.iter().any(|k| k == &token) {
                pecl.push(token);
            }
        }
    }
    let mut out: Vec<String> = PHP_EXTENSIONS
        .iter()
        .filter(|e| catalog.iter().any(|k| k == e.key))
        .map(|e| e.key.to_string())
        .collect();
    out.extend(pecl);
    out
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
/// 删掉 `docker-php-ext-<name>.ini` 即可让容器重启后不再加载该扩展。
pub fn disable_script(key: &str) -> Option<String> {
    let name = if let Some(def) = php_extension(key) {
        def.key.to_string()
    } else {
        parse_pecl_token(key)?.0
    };
    Some(format!(
        "rm -f /usr/local/etc/php/conf.d/docker-php-ext-{name}.ini"
    ))
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
        assert!(version_supported(KIND_RUST, "1.83"));
        assert!(!version_supported(KIND_RUST, "1.84"));
        assert!(kind_def("unknown").is_none());
    }

    #[test]
    fn default_name_follows_version() {
        assert_eq!(default_name(KIND_PHP, "8.5"), "php-8.5");
        assert_eq!(default_name(KIND_NODE, "22"), "node-22");
        assert_eq!(default_name(KIND_GO, "1.24"), "go-1.24");
        assert_eq!(default_name(KIND_RUST, "1.83"), "rust-1.83");
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
        assert_eq!(
            image_for(KIND_RUST, "1.83", DEFAULT_PHP_IMAGE),
            "rust:1.83-slim"
        );
    }

    #[test]
    fn rust_ports_are_allocated_in_range() {
        assert_eq!(allocate_rust_port(&[]), Some(9100));
        assert_eq!(allocate_rust_port(&[9100, 9102]), Some(9101));
        let all: Vec<u16> = RUST_PORT_RANGE.collect();
        assert_eq!(allocate_rust_port(&all), None);
    }

    #[test]
    fn image_version_env_keys_cover_official_images() {
        assert_eq!(version_env_key(KIND_PHP), Some("PHP_VERSION"));
        assert_eq!(version_env_key(KIND_NODE), Some("NODE_VERSION"));
        assert_eq!(version_env_key(KIND_GO), Some("GOLANG_VERSION"));
        // Rust 官方镜像没有稳定的版本 ENV：回退目标版本并标注「版本未读取」。
        assert_eq!(version_env_key(KIND_RUST), None);
    }

    #[test]
    fn builtin_extensions_are_flagged() {
        // 内置扩展随 PHP 编入并默认启用，docker-php-ext-enable/disable 都会因找不到
        // .so 而失败（PHP 8.5 实测 exit 1），安装流程据此跳过脚本。
        for key in ["opcache", "curl", "openssl"] {
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
    fn catalog_covers_categories_and_is_grouped() {
        for c in [
            CATEGORY_DB,
            CATEGORY_IMAGE,
            CATEGORY_CRYPTO,
            CATEGORY_CACHE,
            CATEGORY_OTHER,
        ] {
            assert!(
                PHP_EXTENSIONS.iter().any(|e| e.category == c),
                "分类 {c} 没有条目"
            );
        }
        // 每个条目的安装脚本必须非空（内置项除外），且启用/停用链路完整。
        for e in PHP_EXTENSIONS {
            if e.builtin {
                assert!(e.install_script().is_empty());
            } else {
                assert!(
                    !e.install_script().trim().is_empty(),
                    "{} 缺安装脚本",
                    e.key
                );
            }
            assert!(disable_script(e.key).is_some());
        }
        // 目录条目数（接近官方镜像可编译全集，且不写死在前端）。
        assert!(PHP_EXTENSIONS.len() >= 35, "扩展目录条目过少");
    }

    #[test]
    fn pecl_scripts_feed_stdin_and_clean_deps() {
        // pecl 在 PTY 下会就编译选项逐个提问；不重定向 stdin 会一直阻塞（实测挂起）。
        for e in PHP_EXTENSIONS {
            let script = e.install_script();
            if script.contains("pecl install") {
                assert!(
                    script.contains("yes '' | pecl install"),
                    "{}：pecl 必须重定向 stdin，否则 PTY 下无人应答会挂起",
                    e.key
                );
                if !e.deps.is_empty() {
                    assert!(
                        script.contains("apk del .skiff-build"),
                        "{}：装完应清理依赖",
                        e.key
                    );
                }
            }
        }
    }

    #[test]
    fn pecl_tokens_roundtrip() {
        assert_eq!(pecl_token("swoole", ""), "pecl:swoole");
        assert_eq!(pecl_token("swoole", "5.1.2"), "pecl:swoole@5.1.2");
        assert_eq!(
            parse_pecl_token("pecl:swoole@5.1.2"),
            Some(("swoole".into(), "5.1.2".into()))
        );
        assert!(parse_pecl_token("pecl:../evil").is_none());
        assert!(parse_pecl_token("pecl:Swoole").is_none());
        let script = extension_install_script("pecl:swoole@5.1.2").expect("script");
        assert!(script.contains("pecl install swoole-5.1.2"));
        assert!(script.contains("docker-php-ext-enable swoole"));
        assert_eq!(extension_label("pecl:swoole@5.1.2"), "PECL swoole 5.1.2");
        assert!(disable_script("pecl:swoole@5.1.2")
            .expect("disable")
            .contains("docker-php-ext-swoole.ini"));
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
    fn catalog_keys_are_required() {
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
    }

    #[test]
    fn normalize_dedupes_and_orders_by_catalog() {
        let raw = vec![
            "gd".to_string(),
            "opcache".to_string(),
            "gd".to_string(),
            "bogus".to_string(),
            "pecl:swoole".to_string(),
            "pecl:swoole".to_string(),
        ];
        assert_eq!(
            normalize_extensions(&raw),
            vec!["gd", "opcache", "pecl:swoole"]
        );
        assert_eq!(split_extensions("gd, opcache,,"), vec!["gd", "opcache"]);
        assert_eq!(
            join_extensions(&["gd".into(), "opcache".into()]),
            "gd,opcache"
        );
        // 非法 PECL token 丢弃。
        assert_eq!(split_extensions("gd, pecl:../x"), vec!["gd"]);
    }

    #[test]
    fn builtin_covers_image_compiled_set_without_extra_deps() {
        // 官方 php:*-fpm-alpine 已编译并启用这些扩展；重复编译/启用既浪费也可能报错。
        for key in [
            "opcache",
            "curl",
            "openssl",
            "mbstring",
            "sodium",
            "sqlite3",
            "pdo_sqlite",
        ] {
            let def = php_extension(key).expect(key);
            assert!(def.builtin, "{key} 应标记为镜像内置");
            assert!(def.deps.is_empty(), "{key} 内置项不应再列编译依赖");
            assert!(
                def.install_script().is_empty(),
                "{key} 内置项不应产生安装步骤"
            );
        }
        // mbstring 曾错误依赖 oniguruma-dev，会触发多余 apk 安装与编译。
        assert!(!php_extension("mbstring")
            .expect("mbstring")
            .deps
            .contains(&"oniguruma-dev"));
    }

    #[test]
    fn extension_support_matrix_filters_by_version_and_base() {
        let find = |version: &str, base: &str, key: &str| {
            extensions_for(version, base)
                .into_iter()
                .find(|e| e.def.key == key)
                .expect(key)
                .support
        };
        // 8.3 核心仍提供 imap；8.4 起移入 PECL，UI 灰显。
        assert_eq!(find("8.3", "alpine", "imap"), ExtSupport::Ok);
        for v in ["8.4", "8.5", "8.5.12"] {
            assert!(
                matches!(find(v, "alpine", "imap"), ExtSupport::Unsupported(_)),
                "PHP {v} 的 imap 应不可用"
            );
        }
        // Oracle Instant Client 只有 glibc 版：alpine 不可用，debian 可用。
        for key in ["oci8", "pdo_oci"] {
            assert!(matches!(
                find("8.5", "alpine", key),
                ExtSupport::Unsupported(_)
            ));
            assert_eq!(find("8.5", "debian", key), ExtSupport::Ok);
        }
        // 依赖差异：alpine 提示临时依赖，debian 交给 mlocati 用 apt 解析。
        assert!(matches!(
            find("8.5", "alpine", "gd"),
            ExtSupport::NeedsDeps(_)
        ));
        assert_eq!(find("8.5", "debian", "gd"), ExtSupport::Ok);
        // 后端校验错误码：版本移除 → EXT_NOT_IN_VERSION；alpine OCI → 基座不支持。
        let imap = php_extension("imap").expect("imap");
        assert_eq!(
            extension_block_reason(imap, "8.5", "alpine")
                .expect("blocked")
                .0,
            "EXT_NOT_IN_VERSION"
        );
        let oci = php_extension("oci8").expect("oci8");
        assert_eq!(
            extension_block_reason(oci, "8.5", "alpine")
                .expect("blocked")
                .0,
            "EXT_BASE_IMAGE_UNSUPPORTED"
        );
        assert!(extension_block_reason(oci, "8.5", "debian").is_none());
        // 三版本 × 两基座都返回完整清单（只做 delta，不做裁剪）。
        for v in ["8.3", "8.4", "8.5"] {
            for b in ["alpine", "debian"] {
                assert_eq!(extensions_for(v, b).len(), PHP_EXTENSIONS.len());
            }
        }
    }

    #[test]
    fn legacy_dependency_names_are_package_names() {
        // 旧表里的 `ldap-dev` / `tidy` / `snmp` 都不是 apk 包名，会导致兜底路径失败。
        assert_eq!(php_extension("ldap").expect("ldap").deps, &["openldap-dev"]);
        assert_eq!(php_extension("tidy").expect("tidy").deps, &["tidyhtml-dev"]);
        assert_eq!(php_extension("snmp").expect("snmp").deps, &["net-snmp-dev"]);
    }

    #[test]
    fn install_scripts_prefer_mlocati_and_keep_real_stderr() {
        let script = php_extension("redis").expect("redis").install_script();
        assert!(script.contains("install-php-extensions"), "{script}");
        assert!(script.contains("install-php-extensions redis"), "{script}");
        // 失败要能回传真实报错：不能把 stderr 丢掉。
        assert!(!script.contains("2>/dev/null"), "{script}");
    }

    #[test]
    fn pecl_failures_are_classified_without_hiding_output() {
        assert_eq!(
            pecl_failure_code("ERROR: No releases available for package \"pecl/foo\""),
            Some("EXT_PECL_NOT_READY")
        );
        assert_eq!(
            pecl_failure_code("failed to download pecl/bar within preferred state"),
            Some("EXT_PECL_NOT_READY")
        );
        assert_eq!(pecl_failure_code("make: *** No targets specified"), None);
    }
}
