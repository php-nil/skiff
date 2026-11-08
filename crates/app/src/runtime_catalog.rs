//! 运行环境与 PHP 扩展清单的**单一来源**。
//!
//! 版本目录、默认实例名、镜像名与 PHP 扩展的安装脚本都只在这里定义：
//! Web 下拉框与勾选项由它渲染，安装/重建也按它执行，避免前后端各维护一份。

use skiff_store::{KIND_PHP, KIND_RUST};

use crate::sites::{DEFAULT_PHP_IMAGE, DEFAULT_PHP_VERSION};

/// 一个可创建运行环境类型及其可选的次版本号。
#[derive(Debug, Clone, Copy)]
pub struct RuntimeKindDef {
    pub kind: &'static str,
    pub label: &'static str,
    /// 大版本 → 小版本：只放完整版本号，前端不提供手填。
    pub versions: &'static [&'static str],
}

/// 可创建的类型目录。只列已实现机型（PHP / Rust）；Node.js / Go 未实现，
/// 不进入目录，新建弹窗与 API 校验都不会接受。
pub const RUNTIME_KINDS: &[RuntimeKindDef] = &[
    RuntimeKindDef {
        kind: KIND_PHP,
        label: "PHP",
        versions: &["8.5", "8.4", "8.3"],
    },
    RuntimeKindDef {
        kind: KIND_RUST,
        label: "Rust",
        // 2026-11：最新稳定 1.99（1.99.0 于 2026-10-01 发布，1.99.1 于 2026-10-15）；
        // 保留 1.98（次新）与仍在使用中的 1.83 作为可选，不强制所有人升级。
        versions: &["1.99", "1.98", "1.97", "1.83"],
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

/// 名称默认跟随版本（`php-8.5` / `rust-1.99`）；唯一性仍由库里同类型校验。
pub fn default_name(kind: &str, version: &str) -> String {
    format!("{}-{}", kind, version.trim())
}

pub fn image_for(kind: &str, version: &str, configured_php_image: &str) -> String {
    let version = version.trim();
    match kind {
        KIND_PHP if version == DEFAULT_PHP_VERSION => configured_php_image.to_string(),
        KIND_PHP => format!("php:{version}-fpm-alpine"),
        KIND_RUST => format!("rust:{version}-slim"),
        _ => format!("{kind}:{version}"),
    }
}

/// 当前实现 PHP 与 Rust；Node.js/Go 未实现，也不在 `RUNTIME_KINDS` 目录中。
pub fn is_implemented(kind: &str) -> bool {
    matches!(kind, KIND_PHP | KIND_RUST)
}

/// 官方镜像在 `Config.Env` 里声明的真实版本变量。
/// Rust 官方镜像没有稳定变量，返回 `None`（列表回退目标版本并标注）。
pub fn version_env_key(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_PHP => Some("PHP_VERSION"),
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
    /// 编译依赖被清理后仍需保留的运行库：显式安装，避免 `.so` 加载时缺库。
    pub runtime_deps: &'static [&'static str],
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
    /// 只编译并启用本扩展（不含 apk 依赖安装/清理），供单项与批量 legacy 路径复用。
    fn legacy_compile_command(&self) -> String {
        if let Some(pkg) = self.pecl_package {
            // pecl 在 PTY 下会逐项提问；不重定向 stdin 会阻塞（实测挂起）。
            // MAKE 交给并行 make，缩短 PECL 源码编译时间。
            format!(
                "export MAKE=\"make -j$(nproc)\"; yes '' | pecl install {pkg} && docker-php-ext-enable {}",
                self.key
            )
        } else {
            let mut out = String::new();
            if let Some(cfg) = self.configure {
                out.push_str("docker-php-ext-configure ");
                out.push_str(cfg);
                out.push_str(" && ");
            }
            out.push_str("docker-php-ext-install -j\"$(nproc)\" ");
            out.push_str(self.key);
            out.push_str(" && docker-php-ext-enable ");
            out.push_str(self.key);
            out
        }
    }

    /// 旧的手写依赖 + `docker-php-ext-install` 路径：仅在 mlocati 主路径失败时兜底。
    fn legacy_install_script(&self) -> String {
        let mut out = String::new();
        let mut clean = false;
        // 运行库必须显式安装：`apk del .skiff-build` 会把作为虚拟包依赖装进来的
        // 运行库一起带走，导致 .so 编译成功但启动时缺库（gd 实测 libpng16.so.16 丢失）。
        if !self.runtime_deps.is_empty() {
            out.push_str("apk add --no-cache ");
            out.push_str(&self.runtime_deps.join(" "));
            out.push_str(" && ");
        }
        if !self.deps.is_empty() {
            out.push_str("apk add --no-cache --virtual .skiff-build ");
            out.push_str(&self.deps.join(" "));
            out.push_str(" && ");
            clean = true;
        }
        out.push_str(&self.legacy_compile_command());
        if clean {
            out.push_str(" && apk del .skiff-build");
        }
        out
    }

    /// 安装并启用该扩展的 shell 脚本。
    ///
    /// 优先使用随包分发的 mlocati/docker-php-extension-installer（正式 release、
    /// 容器内本地副本 `/www/skiff/tools/install-php-extensions`）：它按 PHP 版本
    /// 与基础镜像解析系统依赖（alpine 用 apk、debian 用 apt），安装后自动清理，
    /// 失败保留真实 stderr。本地副本缺失/版本不符时打印明确错误并回退固定版本
    /// 下载；主路径**执行失败**也会回退到手写依赖表，两段输出都留在任务日志里。
    pub fn install_script(&self) -> String {
        if self.builtin {
            // 官方镜像已编译并默认启用，返回空脚本让调用方直接跳过：
            // 重复 `docker-php-ext-enable` 可能触发重复加载或找不到 .so。
            return String::new();
        }
        let pkg = self.pecl_package.unwrap_or(self.key);
        fallback_script(pkg, &self.legacy_install_script())
    }
}

pub const CATEGORY_DB: &str = "数据库";
pub const CATEGORY_IMAGE: &str = "图片";
pub const CATEGORY_CRYPTO: &str = "加密";
pub const CATEGORY_CACHE: &str = "缓存";
pub const CATEGORY_OTHER: &str = "其他";

// ---------------------------------------------------------------------------
// mlocati/docker-php-extension-installer：固定版本、随包分发
// ---------------------------------------------------------------------------
//
// 运行时不再从 GitHub 拉脚本：安装包携带 `deploy/vendor/install-php-extensions`，
// 内核启动/安装环境时写到 `<web_root>/skiff/tools/`，PHP 容器内固定可见
// `/www/skiff/tools/install-php-extensions`。下面的常量也编进二进制，保证
// Docker 形态（无 install.sh）同样能落盘。

/// 随包脚本的版本号（GitHub release tag）。
pub const PHP_EXTENSION_INSTALLER_VERSION: &str = "2.13.2";
/// 官方来源（记录用；默认路径不访问网络）。
pub const PHP_EXTENSION_INSTALLER_URL: &str =
    "https://github.com/mlocati/docker-php-extension-installer/releases/download/2.13.2/install-php-extensions";
/// 随包脚本的 SHA-256（`deploy/vendor/install-php-extensions`，官方 2.13.2 release 资产）。
pub const PHP_EXTENSION_INSTALLER_SHA256: &str =
    "27fa8d80bd4e22feb9ddcadff1ca40c8f3e8ea942eb2dd8f446ed1afd2f2f0ab";
/// 容器内固定路径（`web_root` 挂载到 `/www`）。
pub const PHP_EXTENSION_INSTALLER_CONTAINER: &str = "/www/skiff/tools/install-php-extensions";
/// 随二进制分发的脚本内容（与 `deploy/vendor/` 同一份资产）。
pub const PHP_EXTENSION_INSTALLER: &str =
    include_str!("../../../deploy/vendor/install-php-extensions");

/// 生成安装脚本前缀：优先本地副本并校验版本；缺失/版本不符时打印明确错误并回退
/// 固定版本下载。正式 release 资产的 `IPE_VERSION` 是具体版本号，不会进入
/// `IPE_VERSION=master` 的 `sleep 10` 分支；执行安装器时仍显式 `CI=true` 兜底。
///
/// `IPE_KEEP_SYSPKG_CACHE=1`：安装器默认在收尾时删掉 `/var/cache/apk/*`。
/// 容器把该目录挂到宿主 `<runtime>/cache/apk`，保留缓存后，重建与同机多环境
/// 可复用已下载的依赖包，不再每次重下整套构建依赖。
///
/// apk-tools 3 只在 `/etc/apk/cache` 存在时才自动启用 `cache-packages`；容器里
/// 该目录不存在，apk 会静默回退到 `/var/cache/apk` 静态缓存目录，但**不把 `.apk`
/// 落盘**（只留索引）。安装器的 `IPE_APK_FLAGS` 在脚本内部被重置为 `''`，无法从
/// 环境注入，因此在 prelude 里写 `/etc/apk/config` 显式打开 `cache-packages`，
/// 让主路径的 `apk add` 把包副本落进挂载的 `/var/cache/apk`。
fn installer_prelude() -> String {
    format!(
        "set -e\n\
         IPE_KEEP_SYSPKG_CACHE=1\n\
         export IPE_KEEP_SYSPKG_CACHE\n\
         if [ ! -f /etc/apk/config ] || ! grep -q '^cache-packages' /etc/apk/config; then\n\
         \tprintf 'cache-packages yes\\n' >> /etc/apk/config\n\
         fi\n\
         SKIFF_LOCAL_IPE='{local}'\n\
         if [ -f \"$SKIFF_LOCAL_IPE\" ]; then\n\
         \tSKIFF_IPE_VER=$(sed -n 's/^IPE_VERSION=//p' \"$SKIFF_LOCAL_IPE\" | head -n 1)\n\
         \tif [ \"$SKIFF_IPE_VER\" = '{ver}' ]; then\n\
         \t\tcp \"$SKIFF_LOCAL_IPE\" /usr/local/bin/install-php-extensions\n\
         \t\tchmod 0755 /usr/local/bin/install-php-extensions\n\
         \t\techo \"[skiff] 扩展安装器：使用本地副本 $SKIFF_LOCAL_IPE（mlocati {ver}，sha256 {sha}；apk 包缓存 /var/cache/apk 已启用）\"\n\
         \telse\n\
         \t\techo \"[skiff] 本地扩展安装器版本不符：$SKIFF_IPE_VER（期望 {ver}）；回退下载固定版本\" >&2\n\
         \t\tcurl -fsSL {url} -o /usr/local/bin/install-php-extensions && chmod 0755 /usr/local/bin/install-php-extensions || true\n\
         \tfi\n\
         else\n\
         \techo \"[skiff] 本地扩展安装器缺失：$SKIFF_LOCAL_IPE；回退下载固定版本 {ver}\" >&2\n\
         \tcurl -fsSL {url} -o /usr/local/bin/install-php-extensions && chmod 0755 /usr/local/bin/install-php-extensions || true\n\
         fi\n",
        local = PHP_EXTENSION_INSTALLER_CONTAINER,
        ver = PHP_EXTENSION_INSTALLER_VERSION,
        sha = PHP_EXTENSION_INSTALLER_SHA256,
        url = PHP_EXTENSION_INSTALLER_URL,
    )
}

/// 主路径（本地/下载的 mlocati 安装器）与 legacy 手写依赖表的组合。
///
/// 关键点：判据不再只是 `command -v`。主路径**执行失败**也会进入 legacy，
/// 失败原因与主路径 stderr 都留在输出里；两者都失败时退出码取 legacy
/// （编译阶段的具体失败码），并打印两段均已保留的说明。
fn fallback_script(pkg: &str, legacy: &str) -> String {
    format!(
        "{pre}\
         if command -v install-php-extensions >/dev/null 2>&1; then\n\
         \tif CI=true install-php-extensions {pkg}; then\n\
         \t\texit 0\n\
         \telse\n\
         \t\tskiff_main_rc=$?\n\
         \t\techo \"[skiff] 主路径失败（exit $skiff_main_rc），已回退手写依赖表：{pkg}\" >&2\n\
         \tfi\n\
         fi\n\
         if {legacy}; then\n\
         \texit 0\n\
         else\n\
         \tskiff_legacy_rc=$?\n\
         \techo \"[skiff] 回退路径也失败（exit $skiff_legacy_rc）；主路径与回退两段输出均已保留\" >&2\n\
         \texit $skiff_legacy_rc\n\
         fi\n",
        pre = installer_prelude(),
        pkg = pkg,
        legacy = legacy,
    )
}

/// 批量安装脚本：**一次** `docker exec`、主路径一次传多个包名；
/// legacy 回退按「union 运行库 + union 编译依赖 → 逐项编译 → 末尾一次清理」，
/// 每项前打印分隔标记，失败时能定位到具体扩展。
pub fn batch_install_script(keys: &[String]) -> Option<String> {
    let mut items: Vec<(String, String, String)> = Vec::new();
    let mut runtime_deps: Vec<String> = Vec::new();
    let mut build_deps: Vec<String> = Vec::new();
    for key in keys {
        if let Some(def) = php_extension(key) {
            if def.builtin {
                continue;
            }
            let pkg = def.pecl_package.unwrap_or(def.key).to_string();
            for d in def.runtime_deps {
                if !runtime_deps.iter().any(|x| x == d) {
                    runtime_deps.push(d.to_string());
                }
            }
            for d in def.deps {
                if !build_deps.iter().any(|x| x == d) {
                    build_deps.push(d.to_string());
                }
            }
            items.push((pkg, def.legacy_compile_command(), extension_label(def.key)));
        } else if let Some((name, version)) = parse_pecl_token(key) {
            let pkg = if version.is_empty() {
                name.clone()
            } else {
                format!("{name}-{version}")
            };
            // 自定义 PECL 扩展没有登记依赖：沿用旧脚本的通用编译工具集。
            for d in ["autoconf", "g++", "make", "pkgconf", "re2c"] {
                if !build_deps.iter().any(|x| x == d) {
                    build_deps.push(d.to_string());
                }
            }
            items.push((
                pkg.clone(),
                format!(
                    "export MAKE=\"make -j$(nproc)\"; yes '' | pecl install {pkg} && docker-php-ext-enable {name}"
                ),
                extension_label(key),
            ));
        }
    }
    if items.is_empty() {
        return None;
    }
    let pkgs: Vec<String> = items.iter().map(|(p, _, _)| p.clone()).collect();
    let mut legacy = String::from("export MAKE=\"make -j$(nproc)\"");
    if !runtime_deps.is_empty() {
        legacy.push_str(" && apk add --no-cache ");
        legacy.push_str(&runtime_deps.join(" "));
    }
    if !build_deps.is_empty() {
        legacy.push_str(" && apk add --no-cache --virtual .skiff-build ");
        legacy.push_str(&build_deps.join(" "));
    }
    for (idx, (_, cmd, label)) in items.iter().enumerate() {
        legacy.push_str(&format!(
            "\necho \"[skiff] --- 回退安装 {label}（{}/{total}）---\"\n\
             if {cmd}; then\n\
             \techo \"[skiff] --- {label} 完成 ---\"\n\
             else\n\
             \tskiff_ext_rc=$?\n\
             \techo \"[skiff] --- {label} 失败（exit $skiff_ext_rc）---\" >&2\n\
             \texit $skiff_ext_rc\n\
             fi",
            idx + 1,
            total = items.len()
        ));
    }
    if !build_deps.is_empty() {
        legacy.push_str(" && apk del .skiff-build");
    }
    Some(fallback_script(&pkgs.join(" "), &legacy))
}

/// 批量停用脚本：一次 `docker exec` 内逐项删除 ini，并打印分段标记。
pub fn batch_disable_script(keys: &[String]) -> Option<String> {
    let scripts: Vec<String> = keys
        .iter()
        .filter_map(|key| {
            let script = disable_script(key)?;
            Some(format!(
                "echo \"[skiff] --- 停用 {} ---\"\n{script}",
                extension_label(key)
            ))
        })
        .collect();
    if scripts.is_empty() {
        return None;
    }
    Some(format!("set -e\n{}", scripts.join("\n")))
}

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
        runtime_deps: &[],
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
        runtime_deps: &[],
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
        runtime_deps: &["libpq"],
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
        runtime_deps: &["libpq"],
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
        runtime_deps: &["unixodbc"],
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
        runtime_deps: &["libpng", "libjpeg-turbo", "freetype"],
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
        runtime_deps: &["imagemagick-libs"],
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
        runtime_deps: &["gmp"],
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
        runtime_deps: &["libzip"],
        ..build("zip", "Zip", CATEGORY_OTHER, false, "压缩包读写（libzip）")
    },
    PhpExtensionDef {
        deps: &["icu-dev"],
        runtime_deps: &["icu-libs"],
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
        runtime_deps: &["libbz2"],
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
        runtime_deps: &["libxslt"],
        ..build("xsl", "XSL", CATEGORY_OTHER, false, "XSLT 转换（libxslt）")
    },
    PhpExtensionDef {
        deps: &["openldap-dev"],
        runtime_deps: &["libldap"],
        ..build("ldap", "LDAP", CATEGORY_OTHER, false, "LDAP 目录服务")
    },
    PhpExtensionDef {
        deps: &["net-snmp-dev"],
        runtime_deps: &["net-snmp-libs"],
        ..build("snmp", "SNMP", CATEGORY_OTHER, false, "SNMP 监控")
    },
    PhpExtensionDef {
        deps: &["tidyhtml-dev"],
        runtime_deps: &["tidyhtml-libs"],
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

/// 建环境时预选扩展的二次校验：与抽屉安装共用同一份版本/基座矩阵。
/// 返回 `Err((code, reason))` 时直接拒绝创建，避免绕过 UI 灰显塞入不可用扩展。
pub fn validate_selection(
    version: &str,
    image: &str,
    exts: &[String],
) -> Result<(), (&'static str, String)> {
    let base = base_image_of(image);
    for token in exts {
        if let Some(def) = php_extension(token) {
            if let Some((code, reason)) = extension_block_reason(def, version, base) {
                return Err((code, reason));
            }
        }
    }
    Ok(())
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

/// 某个版本 + 基座**可用**的扩展清单。
///
/// 不可用项不再进入展示层（不灰显、不占位）；原因只通过
/// [`extension_block_reason`] / [`extension_support`] 在后端报错时给出。
pub fn extensions_for(version: &str, base_image: &str) -> Vec<ExtensionEntry> {
    PHP_EXTENSIONS
        .iter()
        .filter_map(|def| {
            let support = extension_support(def, version, base_image);
            match support {
                ExtSupport::Unsupported(_) => None,
                _ => Some(ExtensionEntry { def, support }),
            }
        })
        .collect()
}

/// 编译较重、耗时明显的扩展（确认弹窗据此提示预计耗时）。
pub fn extension_is_heavy(key: &str) -> bool {
    matches!(
        key,
        "memcached"
            | "imagick"
            | "intl"
            | "ldap"
            | "snmp"
            | "tidy"
            | "rdkafka"
            | "grpc"
            | "swoole"
            | "mongodb"
            | "amqp"
            | "oci8"
            | "pdo_oci"
    ) || key.starts_with("pecl:")
}

/// 重项预计耗时文案；轻量项返回空串。
pub fn extension_estimated_time(key: &str) -> &'static str {
    if extension_is_heavy(key) {
        "预计 2–5 分钟（需编译）"
    } else {
        ""
    }
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
    // PECL 自定义扩展：同样优先本地 mlocati（依赖解析 + 清理），失败保留真实 stderr；
    // 本地副本缺失/不可用或主路径执行失败时回退 `pecl install`
    // （PTY 下用 `yes ''` 防提问挂起；MAKE 并行缩短编译）。
    Some(fallback_script(
        &pkg,
        &format!(
            "export MAKE=\"make -j$(nproc)\" && apk add --no-cache --virtual .skiff-build autoconf g++ make pkgconf re2c && yes '' | pecl install {pkg} && docker-php-ext-enable {name} && apk del .skiff-build"
        ),
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

/// 卸载脚本：官方镜像只有 `docker-php-ext-enable`，**没有** `docker-php-ext-disable`
/// （实测 exit 127 `not found`）。启用时写入的 ini 在 `/usr/local/etc/php/conf.d/`，
/// 删掉 `docker-php-ext-<name>.ini` 即可让容器重启后不再加载该扩展。
///
/// 但只删 ini 会把 PECL 注册表项与 `.so` 留下：再次安装时 `install-php-extensions`
/// 会因 `pecl/<name> is already installed and is the same as the released version`
/// 直接失败（实测 redis 卸载后重装即如此）。因此还要：已被 pecl 登记则
/// `pecl uninstall`（它同时清注册表与文件），再兜底删扩展目录里的 `.so`
/// （`docker-php-ext-install` 编译的扩展不经 pecl）。
pub fn disable_script(key: &str) -> Option<String> {
    let name = if let Some(def) = php_extension(key) {
        def.key.to_string()
    } else {
        parse_pecl_token(key)?.0
    };
    Some(format!(
        "rm -f /usr/local/etc/php/conf.d/docker-php-ext-{name}.ini\n\
         if pecl list 2>/dev/null | awk 'NR>3 {{print $1}}' | grep -qx '{name}'; then\n\
         \tpecl uninstall {name} >/dev/null 2>&1 || true\n\
         fi\n\
         rm -f \"$(php-config --extension-dir)/{name}.so\"\n\
         echo '[skiff] 已停用扩展 {name}（ini/注册表/.so 均已清理）'"
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
        assert!(kind_def("node").is_none(), "Node.js 未实现，不得进入目录");
        assert!(kind_def("go").is_none(), "Go 未实现，不得进入目录");
        assert!(version_supported(KIND_RUST, "1.99"));
        assert!(version_supported(KIND_RUST, "1.98"));
        assert!(version_supported(KIND_RUST, "1.83"), "保留旧版本可选");
        assert!(!version_supported(KIND_RUST, "1.100"));
        assert!(kind_def("unknown").is_none());
    }

    #[test]
    fn rust_catalog_starts_with_latest_stable() {
        let rust = kind_def(KIND_RUST).expect("rust 目录必须存在");
        assert_eq!(*rust.versions.first().unwrap(), "1.99");
        assert!(rust.versions.len() >= 3, "至少保留一个旧版本可选");
    }

    #[test]
    fn default_name_follows_version() {
        assert_eq!(default_name(KIND_PHP, "8.5"), "php-8.5");
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
            image_for(KIND_RUST, "1.99", DEFAULT_PHP_IMAGE),
            "rust:1.99-slim"
        );
        assert_eq!(
            image_for(KIND_RUST, "1.98", DEFAULT_PHP_IMAGE),
            "rust:1.98-slim"
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
    fn runtime_libs_are_installed_explicitly_before_cleanup() {
        // 实测：gd 编译成功但 `.so` 启动时报 libpng16.so.16 缺失——`apk del .skiff-build`
        // 把作为虚拟包依赖装进来的运行库一起删了。运行库必须显式安装且排在编译依赖之前。
        for e in PHP_EXTENSIONS.iter().filter(|e| !e.runtime_deps.is_empty()) {
            let script = e.install_script();
            let head = format!("apk add --no-cache {} &&", e.runtime_deps.join(" "));
            assert!(script.contains(&head), "{}：运行库未显式安装", e.key);
            let head_at = script.find(&head).expect("head");
            let build_at = script
                .find("--virtual .skiff-build")
                .unwrap_or_else(|| panic!("{}：缺编译依赖段", e.key));
            assert!(
                head_at < build_at,
                "{}：运行库必须先于编译依赖显式安装，否则会被一并清理",
                e.key
            );
        }
        let gd = extension_install_script("gd").expect("gd");
        assert!(gd.contains("apk add --no-cache libpng libjpeg-turbo freetype &&"));
        assert!(gd.contains("apk del .skiff-build"));
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
            // 实测：只删 ini 会留下 pecl 注册表与 .so，重装报 already installed。
            assert!(
                script.contains(&format!("pecl uninstall {key}")),
                "{key}：停用脚本应清理 pecl 注册表"
            );
            assert!(
                script.contains(&format!("/{key}.so")),
                "{key}：停用脚本应清理扩展目录里的 .so"
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
        let support = |version: &str, base: &str, key: &str| {
            let def = php_extension(key).expect(key);
            extension_support(def, version, base)
        };
        // 8.3 核心仍提供 imap；8.4 起移入 PECL：展示层直接不出现。
        assert_eq!(find("8.3", "alpine", "imap"), ExtSupport::Ok);
        for v in ["8.4", "8.5", "8.5.12"] {
            assert!(
                matches!(support(v, "alpine", "imap"), ExtSupport::Unsupported(_)),
                "PHP {v} 的 imap 应不可用"
            );
            assert!(
                extensions_for(v, "alpine")
                    .iter()
                    .all(|e| e.def.key != "imap"),
                "PHP {v} 的不可用 imap 不应出现在展示清单"
            );
        }
        // Oracle Instant Client 只有 glibc 版：alpine 不可用、展示层不出现；debian 可用。
        for key in ["oci8", "pdo_oci"] {
            assert!(matches!(
                support("8.5", "alpine", key),
                ExtSupport::Unsupported(_)
            ));
            assert!(
                extensions_for("8.5", "alpine")
                    .iter()
                    .all(|e| e.def.key != key),
                "alpine 的 {key} 不应出现在展示清单"
            );
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
        // 三版本 × 两基座：展示清单只含可用项（不含任何 Unsupported）。
        // 8.3/debian 恰好全部可用，因此不强制「少于完整目录」，只校验过滤正确性。
        for v in ["8.3", "8.4", "8.5"] {
            for b in ["alpine", "debian"] {
                let visible = extensions_for(v, b);
                println!(
                    "visible {v}/{b}: {} / {}",
                    visible.len(),
                    PHP_EXTENSIONS.len()
                );
                assert!(!visible.is_empty(), "{v}/{b} 不应为空");
                assert!(
                    visible
                        .iter()
                        .all(|e| !matches!(e.support, ExtSupport::Unsupported(_))),
                    "{v}/{b} 展示清单不应包含不可用项"
                );
            }
        }
        // 8.4/8.5 有核心移除项（imap/pspell），两基座都应出现过滤。
        for v in ["8.4", "8.5"] {
            for b in ["alpine", "debian"] {
                assert!(
                    extensions_for(v, b).len() < PHP_EXTENSIONS.len(),
                    "{v}/{b} 应过滤掉不可用项"
                );
            }
        }
    }

    #[test]
    fn extension_installer_is_vendored_and_local_first() {
        // 随包资产本体：固定版本脚本，来源/版本/sha256 都记录在常量里。
        assert!(
            PHP_EXTENSION_INSTALLER.starts_with("#!/bin/sh"),
            "随包脚本应以 shebang 开头"
        );
        assert!(PHP_EXTENSION_INSTALLER.contains("IPE_VERSION"));
        // 必须是正式 release：脚本头写死具体版本，而非 master（master 会 sleep 10）。
        assert!(
            PHP_EXTENSION_INSTALLER
                .contains(&format!("IPE_VERSION={PHP_EXTENSION_INSTALLER_VERSION}")),
            "随包脚本应为正式 release 的 IPE_VERSION"
        );
        assert!(!PHP_EXTENSION_INSTALLER.contains("IPE_VERSION=master"));
        assert_eq!(PHP_EXTENSION_INSTALLER_SHA256.len(), 64);
        assert!(PHP_EXTENSION_INSTALLER_URL.contains(PHP_EXTENSION_INSTALLER_VERSION));
        assert!(PHP_EXTENSION_INSTALLER_CONTAINER.starts_with("/www/skiff/tools/"));
        // 安装脚本默认走本地副本，本地缺失/版本不符才回退固定版本下载。
        let script = php_extension("redis").expect("redis").install_script();
        assert!(script.contains("SKIFF_LOCAL_IPE"), "{script}");
        assert!(script.contains("使用本地副本 $SKIFF_LOCAL_IPE"), "{script}");
        assert!(script.contains("本地扩展安装器缺失"), "{script}");
        assert!(script.contains("本地扩展安装器版本不符"), "{script}");
        assert!(script.contains(PHP_EXTENSION_INSTALLER_URL), "{script}");
        // 执行安装器时 CI=true，避免任何 master 脚本进入 sleep 分支。
        assert!(
            script.contains("CI=true install-php-extensions"),
            "{script}"
        );
        // 主路径执行失败也要回退（判据不是只有 command -v）。
        assert!(script.contains("主路径失败"), "{script}");
        assert!(script.contains("已回退手写依赖表"), "{script}");
        assert!(script.contains("回退路径也失败"), "{script}");
        // 不再出现「latest」随缘下载。
        assert!(!script.contains("/latest/"), "{script}");
        let pecl = extension_install_script("pecl:swoole").expect("pecl script");
        assert!(pecl.contains("使用本地副本"), "{pecl}");
        assert!(pecl.contains("MAKE=\"make -j$(nproc)\""), "{pecl}");
        assert!(!pecl.contains("/latest/"), "{pecl}");
    }

    #[test]
    fn batch_scripts_single_exec_and_union_deps() {
        let keys = vec![
            "pdo_pgsql".to_string(),
            "gd".to_string(),
            "redis".to_string(),
        ];
        let batch = batch_install_script(&keys).expect("batch");
        // 一次把多个包名交给安装器。
        assert!(
            batch.contains("install-php-extensions pdo_pgsql gd redis"),
            "{batch}"
        );
        // 回退 union 依赖只装一次、末尾只清理一次。
        assert_eq!(batch.matches(".skiff-build").count(), 2, "{batch}");
        assert!(batch.contains("libpq"), "{batch}");
        assert!(batch.contains("回退安装"), "{batch}");
        assert!(batch.contains("MAKE=\"make -j$(nproc)\""), "{batch}");
        assert_eq!(batch.matches("主路径失败").count(), 1, "{batch}");
        // 内置项不进批量脚本。
        let builtin_keys = vec!["opcache".to_string()];
        assert!(batch_install_script(&builtin_keys).is_none());
        // 卸载合并为一次脚本，带分段标记。
        let rm = batch_disable_script(&["redis".to_string(), "gd".to_string()]).expect("rm");
        assert!(
            rm.contains("停用 Redis") || rm.contains("停用 redis"),
            "{rm}"
        );
        assert_eq!(
            rm.matches("rm -f /usr/local/etc/php/conf.d/").count(),
            2,
            "{rm}"
        );
    }

    #[test]
    fn heavy_extensions_are_flagged_for_confirmation() {
        for key in ["intl", "imagick", "memcached", "ldap"] {
            assert!(extension_is_heavy(key), "{key} 应标记为重项");
            assert!(!extension_estimated_time(key).is_empty());
        }
        assert!(!extension_is_heavy("redis"));
        assert!(extension_estimated_time("redis").is_empty());
        assert!(extension_is_heavy("pecl:swoole"));
    }

    #[test]
    fn selection_validation_rejects_unsupported_extensions() {
        // 建环境表单可被直接构造 POST 绕过 UI 灰显，后端必须按同一矩阵拒绝。
        let sel = |v: &str, keys: &[&str]| {
            let exts: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
            validate_selection(v, &format!("php:{v}-fpm-alpine"), &exts)
                .err()
                .map(|(code, _)| code)
        };
        assert_eq!(sel("8.4", &["imap"]), Some("EXT_NOT_IN_VERSION"));
        assert_eq!(sel("8.4", &["pspell"]), Some("EXT_NOT_IN_VERSION"));
        assert_eq!(sel("8.5", &["imap"]), Some("EXT_NOT_IN_VERSION"));
        assert!(sel("8.3", &["imap", "pspell"]).is_none());
        assert!(sel("8.5", &["gd", "redis"]).is_none());
    }

    #[test]
    fn legacy_dependency_names_are_package_names() {
        // 旧表里的 `ldap-dev` / `tidy` / `snmp` 都不是 apk 包名，会导致兜底路径失败。
        assert_eq!(php_extension("ldap").expect("ldap").deps, &["openldap-dev"]);
        assert_eq!(php_extension("tidy").expect("tidy").deps, &["tidyhtml-dev"]);
        assert_eq!(php_extension("snmp").expect("snmp").deps, &["net-snmp-dev"]);
    }

    #[test]
    fn installer_prelude_enables_apk_package_cache() {
        // apk-tools 3 默认只缓存索引、不缓存 `.apk`；只挂载 `/var/cache/apk`
        // 会出现「目录有文件但二装不命中包」的假优化。必须显式打开 cache-packages。
        let script = php_extension("redis").expect("redis").install_script();
        assert!(script.contains("cache-packages yes"), "{script}");
        assert!(script.contains("/etc/apk/config"), "{script}");
        // 安装器收尾默认 `rm -rf /var/cache/apk/*`，保留缓存才能跨安装复用。
        assert!(script.contains("IPE_KEEP_SYSPKG_CACHE=1"), "{script}");
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
