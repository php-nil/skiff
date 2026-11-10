//! 容器 spec 与镜像安装：镜像/脚本常量、纯函数与 `AppState` 的建容器方法。

use super::*;

/// OpenResty 官方镜像，固定 digest（不复用 1Panel 的 tag）。
pub const DEFAULT_NGINX_IMAGE: &str =
    "openresty/openresty:alpine@sha256:2cb90d64192934c26b3749cf744f47edd59277787fe761bd109eeebdc3e2214a";
pub const DEFAULT_PHP_IMAGE: &str = "php:8.5-fpm-alpine";
/// nginx 用 host 网络直接监听宿主 80；443 预留给 HTTPS。
pub const DEFAULT_PORT: u16 = 80;
pub const DEFAULT_PHP_VERSION: &str = "8.5";
/// 内核内置可安装的 PHP 版本目录由 `runtime_catalog` 单一维护。
/// P1 默认 PHP 运行环境 id（与 `site_runtimes` 种入行一致）。
pub const PHP_RUNTIME_ID: &str = "php-8.5";
/// 容器内 `/www` 的宿主来源由 `SiteSettings::web_root` 决定。
pub const CONTAINER_WEB_ROOT: &str = "/www";
/// 每版本独立 socket 的容器内根目录：`/www/runtime/php-<version>/php-fpm.sock`。
pub const CONTAINER_RUNTIME_ROOT: &str = "/www/runtime";
/// socket 固定属组：openresty 镜像的 worker 与 php 镜像都带 `nobody`（gid 65534）。
/// 0660 只放行该组，本机普通用户连不上。
pub const SOCKET_GID: u32 = 65534;
pub const SOCKET_MODE: u32 = 0o660;
/// PHP-FPM 默认池配置在容器内的位置（用宿主生成的文件覆盖它）。
pub const CONTAINER_PHP_POOL: &str = "/usr/local/etc/php-fpm.d/www.conf";
/// PHP 主配置（宿主 `php/php.ini` 挂到这里）。
pub const CONTAINER_PHP_INI: &str = "/usr/local/etc/php/php.ini";
/// 镜像自带的扩展 ini 目录（官方 PHP 镜像的编译期默认扫描目录）。
pub const CONTAINER_PHP_CONFD: &str = "/usr/local/etc/php/conf.d";
/// 宿主托管的扩展 ini 目录在容器内的位置。
pub const CONTAINER_PHP_SKIFF_CONFD: &str = "/usr/local/etc/php/conf.d-skiff";
/// 数据根中的 PHP 容器身份初始化脚本（宿主侧，只读挂载进容器）。
pub const PHP_INIT_SCRIPT_NAME: &str = "php-account-init.sh";
/// 初始化脚本在容器内的固定挂载点。
pub const CONTAINER_PHP_INIT: &str = "/usr/local/etc/skiff-php-init.sh";
/// PHP 启动时扫描的 ini 目录：镜像自带 conf.d 在前，Skiff 托管目录在后。
///
/// **不能带前导冒号**：PHP 语义里前导冒号表示「把编译期默认扫描目录追加到最前」，
/// 而官方 PHP 镜像的默认目录就是 `conf.d`，会让同一目录被扫两次、扩展 ini 重复加载
/// （`Module already loaded`），`install-php-extensions` 的 `checkModuleWorking`
/// 因此判失败。这里显式列出两个目录，各扫一次。
pub const PHP_INI_SCAN_DIR_VALUE: &str =
    "/usr/local/etc/php/conf.d:/usr/local/etc/php/conf.d-skiff";
/// 宿主侧标记：本实例是新版安装（`php.ini` / `conf.d` 已挂载，编辑才会生效）。
///
/// 旧实例的容器没有这两个挂载，编辑能保存但不生效；详情页据此提示重建。
pub const PHP_CONFIG_SENTINEL: &str = ".config-mounted";
/// 宿主托管 `php.ini` 的初始内容：只写说明，不改镜像默认值。
pub const DEFAULT_PHP_INI: &str =
    "; Skiff 托管的 PHP 主配置（挂载为 /usr/local/etc/php/php.ini）。\n\
     ; 保存前会用容器内 `php -c` 校验；保存后重启 FPM，失败自动回滚。\n\
     ; 注意：页面「PHP 参数」写入 FPM 池的 php_admin_value，优先级高于本文件同名项。\n";

/// PHP 容器身份初始化脚本（数据根、只读挂载、root 执行）。
///
/// 不修改镜像：按包管理器分支幂等建立 `skiff-web` 201000:201000，随后
/// `exec php-fpm` 让 master 仍以 root 启动、再按池配置降权。
pub const PHP_ACCOUNT_INIT_SCRIPT: &str = r#"#!/bin/sh
# Skiff 统一站点身份初始化：容器内幂等建立 skiff-web(201000:201000)。
# 由 root 执行；建号失败直接退出，不带着不一致身份启动 php-fpm。
set -eu

SITE_USER=skiff-web
SITE_ID=201000

if id -u "$SITE_USER" >/dev/null 2>&1; then
    if [ "$(id -u "$SITE_USER")" != "$SITE_ID" ]; then
        echo "[skiff] $SITE_USER 已存在但 uid=$(id -u "$SITE_USER")，要求 $SITE_ID" >&2
        exit 1
    fi
elif [ -f /etc/alpine-release ]; then
    addgroup -g "$SITE_ID" -S "$SITE_USER" 2>/dev/null || true
    adduser -u "$SITE_ID" -D -H -S -G "$SITE_USER" "$SITE_USER"
else
    groupadd -g "$SITE_ID" "$SITE_USER" 2>/dev/null || true
    useradd -u "$SITE_ID" -g "$SITE_ID" -M -s /usr/sbin/nologin "$SITE_USER"
fi

# Debian 系默认 session.save_path 归 www-data；换 uid 后统一改到 /tmp。
mkdir -p /tmp/skiff-opcache
chown "$SITE_USER:$SITE_USER" /tmp/skiff-opcache

# 池模板固定写 listen.group = nobody，宿主侧 SOCKET_GID 也是 65534。
# Debian 系 gid 65534 的组名是 nogroup，getgrnam("nobody") 会失败并让 FPM 起不来。
# 这里不按镜像家族猜组名，而是保证「nobody」这个名字解析到 65534：
# 65534 已被 nogroup 占用时用 groupadd -o 建同 gid 别名，自建镜像也适用。
SOCKET_GID=65534
if ! grep -q '^nobody:' /etc/group; then
    groupadd -o -g "$SOCKET_GID" nobody 2>/dev/null \
        || addgroup -g "$SOCKET_GID" -S nobody 2>/dev/null \
        || printf 'nobody:x:%s:\n' "$SOCKET_GID" >> /etc/group
fi
CURRENT_GID=$(awk -F: '$1 == "nobody" { print $3; exit }' /etc/group)
if [ "$CURRENT_GID" != "$SOCKET_GID" ]; then
    echo "[skiff] 组 nobody 已存在但 gid=$CURRENT_GID，要求 $SOCKET_GID" >&2
    exit 1
fi

exec php-fpm
"#;
/// `fastcgi_params` 的容器内绝对路径。
///
/// vhost 里的 `include` 必须写绝对路径：主配置编辑会用
/// `nginx -t -c /www/nginx/nginx.conf.candidate` 校验候选文件，此时配置目录变成
/// `/www/nginx`，相对 `include fastcgi_params` 会解析成 `/www/nginx/fastcgi_params`
/// 并报 No such file，导致任何合法修改都被误判为失败。
pub const CONTAINER_FASTCGI_PARAMS: &str = "/usr/local/openresty/nginx/conf/fastcgi_params";

/// 内置默认 PHP 实例名（`name` 与 id 一致）。
pub fn default_php_name() -> String {
    format!("php-{DEFAULT_PHP_VERSION}")
}

/// 容器名只从 `<kind>/<name>` 推导：`skiff-<kind>-<name>`。
pub fn runtime_container_name(kind: &str, name: &str) -> String {
    format!("skiff-{kind}-{name}")
}

/// 进程型环境端口校验：必须落在该类型的区间内，且未被其它环境占用。
///
/// 端口是**环境属性**（`site_runtimes.port`），站点侧不接受自定义端口；
/// 这里同时服务于建环境（走 `allocate_port`）与改端口两条路径。
pub(crate) fn validate_process_port(
    kind: &str,
    port: u16,
    used_by_others: &[u16],
) -> Result<(), String> {
    let Some(range) = runtime_catalog::port_range(kind) else {
        return Err(format!(
            "{} 运行环境不占用宿主端口",
            runtime_catalog::kind_label(kind)
        ));
    };
    if !range.contains(&port) {
        return Err(format!(
            "端口必须落在 {}–{} 之间",
            range.start(),
            range.end()
        ));
    }
    if used_by_others.contains(&port) {
        return Err(format!("端口 {port} 已被其它运行环境占用"));
    }
    Ok(())
}

/// 克隆时复制 `<runtime>/app`，跳过依赖与构建产物（新环境启动时会自行安装/编译）。
pub(crate) fn copy_runtime_app(from_home: &Path, to_home: &Path) -> Result<(), String> {
    copy_dir_skip(
        &from_home.join("app"),
        &to_home.join("app"),
        &["node_modules", "target"],
    )
}

pub(crate) fn copy_dir_skip(src: &Path, dst: &Path, skip: &[&str]) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败：{e}", dst.display()))?;
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败：{e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败：{e}"))?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if skip.contains(&name_str) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry
            .file_type()
            .map_err(|e| format!("读取 {} 类型失败：{e}", from.display()))?;
        if ft.is_dir() {
            copy_dir_skip(&from, &to, skip)?;
        } else if ft.is_file() && !to.exists() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制 {} 失败：{e}", from.display()))?;
        }
    }
    Ok(())
}

/// 探测单个运行环境的原始容器状态。
///
/// 三种「不在运行」必须分开：Docker 不可达/列表失败是 `unknown`（不知道），
/// 列表能查但没这个容器是 `missing`（没装），容器在但状态非 running 由
/// [`runtime_effective_state`] 归一成 `stopped`。
pub(crate) fn probe_container_state(docker_known: bool, state: Option<&str>) -> String {
    if !docker_known {
        return "unknown".into();
    }
    match state {
        None => "missing".into(),
        Some(s) if s.trim().is_empty() => "unknown".into(),
        Some(s) => s.to_string(),
    }
}

/// Docker 原始状态归一为界面使用的业务状态；未知值单独保留，不与 running 合并。
pub(crate) fn runtime_effective_state(raw: &str) -> &'static str {
    match raw {
        "running" => "running",
        "stopped" | "exited" | "dead" | "created" | "paused" | "restarting" | "removing" => {
            "stopped"
        }
        "missing" => "missing",
        _ => "unknown",
    }
}

/// 从 `repo@sha256:...` 取出摘要部分；已是摘要时原样返回。
pub(crate) fn digest_sha256(digest: &str) -> &str {
    match digest.find("@sha256:") {
        Some(i) => &digest[i + 1..],
        None => digest.trim(),
    }
}

/// 用于拉取与建容器的固定引用：`image@sha256:...`；无 digest 时保持 tag。
pub(crate) fn pinned_image_ref(image: &str, digest: &str) -> String {
    let sha = digest_sha256(digest);
    if sha.is_empty() || image.contains('@') {
        image.to_string()
    } else {
        format!("{image}@{sha}")
    }
}

/// PHP 实例 socket 的容器内路径：`/www/runtime/<kind>/<name>/php-fpm.sock`。
pub fn runtime_container_socket(kind: &str, name: &str) -> String {
    format!("{CONTAINER_RUNTIME_ROOT}/{kind}/{name}/php-fpm.sock")
}

/// 内核自建容器的统一标签：`project=sites` 用于识别归属，`protected=true`
/// 保证面板不能停/删自己托管的 Web 服务与运行时容器。
pub(crate) fn skiff_labels(component: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            skiff_model::LABEL_PROJECT.to_string(),
            skiff_model::PROJECT_SITES.to_string(),
        ),
        ("io.skiff.component".to_string(), component.to_string()),
        (skiff_model::LABEL_PROTECTED.to_string(), "true".to_string()),
    ])
}

/// 取镜像的 registry 主机名，失败文案里必须带上它。
pub fn registry_of(image: &str) -> String {
    let mut parts = image.split('/');
    let first = parts.next().unwrap_or("");
    let has_path = parts.next().is_some();
    if has_path && (first.contains('.') || first.contains(':')) {
        first.to_string()
    } else {
        "docker.io".to_string()
    }
}

/// Rust 容器入口脚本：工作目录固定为环境自带 `app/`，与站点绑定无关。
///
/// 容器整目录挂载 `/www`，站点只做域名反代；源码只在 `<runtime>/app` 一处维护，
/// 不再有按绑定站点切换目录的“站点脚手架 + 运行代码”双份副本。
pub(crate) fn rust_entry_script() -> &'static str {
    "#!/bin/sh\n\
     DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
     APP=\"$DIR/app\"\n\
     if [ ! -f \"$APP/Cargo.toml\" ]; then\n\
       echo \"skiff: rust source missing: $APP/Cargo.toml\" >&2\n\
       exit 1\n\
     fi\n\
     exec cargo run --release --manifest-path \"$APP/Cargo.toml\"\n"
}

pub(crate) fn rust_run_command(entry: &str, log: &str) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!("sh {entry} >> {log} 2>&1"),
    ]
}

/// Rust P1 工程：无第三方依赖，`cargo run --release` 离线可编译。
pub(crate) fn rust_cargo_manifest() -> &'static str {
    "[package]\nname = \"skiff-runtime-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
     [[bin]]\nname = \"skiff-runtime-app\"\npath = \"src/main.rs\"\n"
}

/// 运行环境站点的初始文档根文件（相对站点 `index/`）。
///
/// 只有 PHP 站点需要 `index.php` 探针。Node.js / Rust 是「一个环境 = 一个应用进程 =
/// 一个固定代码目录（`<runtime>/app`）」，站点只是域名入口，源码不落在站点目录，
/// 因此**不写任何脚手架**，避免让人误以为改站点目录能改应用。
pub(crate) fn runtime_site_default_files(
    kind: &str,
    _version: &str,
    _port: Option<u16>,
) -> Vec<(&'static str, String)> {
    match kind {
        skiff_store::KIND_PHP => vec![("index.php", default_probe_php().to_string())],
        _ => Vec::new(),
    }
}

/// 最小 HTTP 服务：监听 `$PORT`，任何请求返回一段带版本的环境标识文本。
pub(crate) fn rust_main_source(version: &str, port: u16) -> String {
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::TcpListener;

const VERSION: &str = "{version}";

fn main() {{
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or({port});
    let listener = TcpListener::bind(("0.0.0.0", port)).expect("bind port");
    println!("skiff rust runtime {{}} listening on {{}}", VERSION, port);
    for stream in listener.incoming() {{
        let mut stream = match stream {{
            Ok(s) => s,
            Err(e) => {{
                eprintln!("accept failed: {{e}}");
                continue;
            }}
        }};
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let body = format!(
            "Skiff Rust runtime ready\nversion: {{}}\nport: {{}}\n",
            VERSION, port
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {{}}\r\nConnection: close\r\n\r\n{{}}",
            body.as_bytes().len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }}
}}
"#
    )
}

/// Node.js 容器入口脚本：工作目录固定为环境自带 `app/`，与站点绑定无关。
///
/// 与 PHP 的关键差异：这里不是 FastCGI 池，而是**单进程常驻**。脚本只负责进入固定
/// 目录、首次安装依赖，然后把启动命令 `exec` 成 PID 1；`PORT` / `NODE_ENV` / `HOST`
/// 由容器环境注入，应用直接读取即可。绑定站点只是一层域名反代，不参与源码定位。
pub(crate) fn node_entry_script() -> &'static str {
    "#!/bin/sh\n\
     set -e\n\
     DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
     APP=\"$DIR/app\"\n\
     if [ ! -f \"$APP/package.json\" ]; then\n\
       echo \"skiff: node app missing: $APP/package.json\" >&2\n\
       exit 1\n\
     fi\n\
     cd \"$APP\"\n\
     if [ ! -d node_modules ]; then\n\
       if [ -f package-lock.json ]; then\n\
         npm ci --omit=dev --no-audit --no-fund\n\
       else\n\
         npm install --omit=dev --no-audit --no-fund\n\
       fi\n\
     fi\n\
     exec \"$@\"\n"
}

/// 启动命令必须已按 `NodeRuntimeConfig::validate` 校验；这里只做拼接，
/// 不经宿主 shell，元字符在落库前已被拒绝。
pub(crate) fn node_run_command(entry: &str, log: &str, argv: &[String]) -> Vec<String> {
    let mut cmd = format!("sh {entry}");
    for arg in argv {
        cmd.push(' ');
        cmd.push_str(arg);
    }
    cmd.push_str(&format!(" >> {log} 2>&1"));
    vec!["sh".into(), "-c".into(), cmd]
}

/// Node 环境的兜底应用（没有站点绑定时运行），监听 `$PORT`。
pub(crate) fn node_package_manifest() -> &'static str {
    "{\n  \"name\": \"skiff-node-runtime\",\n  \"private\": true,\n  \"version\": \"0.1.0\",\n  \
     \"scripts\": { \"start\": \"node server.js\" }\n}\n"
}

/// 运行环境 `app/` 的初始 Node 工程：一个无三方依赖的最小 HTTP 服务。
///
/// 与 Rust 的 `Cargo.toml + src/main.rs` 对齐——一个环境一个固定代码目录，
/// 容器在 `<runtime>/app` 执行启动命令，站点只做域名反代，不生成 `index.php`。
pub(crate) fn node_server_source(version: &str, port: u16) -> String {
    format!(
        r#"const http = require("http");

const VERSION = "{version}";
const port = Number(process.env.PORT || "{port}");
const host = process.env.HOST || "127.0.0.1";

const server = http.createServer((req, res) => {{
  res.writeHead(200, {{ "Content-Type": "text/plain; charset=utf-8" }});
  res.end(`Skiff Node.js runtime ready\nversion: ${{VERSION}}\nport: ${{port}}\n`);
}});

server.listen(port, host, () => {{
  console.log(`skiff node runtime ${{VERSION}} listening on ${{host}}:${{port}}`);
}});
"#
    )
}

impl AppState {
    /// 拉镜像：先 `inspect`，本地已有就直接回填 digest 并跳过 `ImagePull`。
    ///
    /// - 登记 digest 为空（新环境）：采用本地 tag 的真实 digest 落库，不再冷拉。
    /// - 登记 digest 与本地一致：跳过拉取。
    /// - 登记 digest 与本地不一致（tag 漂移）：按登记 digest 拉取固定版本并提示。
    ///
    /// 注意：`primary_digest_for` 按镜像引用（仓库名）匹配 `repo_digests`，同一镜像存在
    /// 多个仓库条目时不会取错；Docker 的 `RepoDigests` 不含 tag，故无法按 tag 精确匹配。
    pub(crate) async fn ensure_runtime_image(
        &self,
        job_id: &str,
        image: &str,
        registered_digest: &str,
    ) -> Result<String, String> {
        let registered = digest_sha256(registered_digest);
        let local = self
            .docker()
            .inspect_image(image)
            .await
            .ok()
            .and_then(|i| i.primary_digest_for(image));
        if let Some(local_digest) = &local {
            if registered.is_empty() {
                self.job_note(
                    job_id,
                    &format!(
                        "镜像 {image} 本地已存在（{local_digest}），回填 image_digest 并跳过拉取"
                    ),
                )
                .await;
                return Ok(local_digest.clone());
            }
            if digest_sha256(local_digest) == registered {
                self.job_note(
                    job_id,
                    &format!("镜像 {image} 已存在且与登记 digest 一致（{local_digest}），跳过拉取"),
                )
                .await;
                return Ok(local_digest.clone());
            }
            self.job_note(
                job_id,
                &format!(
                    "镜像 {image} 与登记值漂移：本地 {local_digest}，登记 {registered_digest}；按登记 digest 拉取"
                ),
            )
            .await;
        } else if !registered.is_empty() {
            self.job_note(
                job_id,
                &format!("本地无镜像 {image}，按登记 digest 拉取固定版本"),
            )
            .await;
        }
        let pull_ref = if registered.is_empty() {
            image.to_string()
        } else {
            pinned_image_ref(image, registered_digest)
        };
        self.job_advance(
            job_id,
            "拉取镜像",
            30,
            &format!("拉取镜像 {pull_ref}（首次可能较慢）"),
        )
        .await;
        self.kernel_execute(
            Verb::ImagePull,
            None,
            DockerParams::ImagePull(pull_ref.clone()),
        )
        .await
        .map_err(|e| format!("拉取 {pull_ref} 失败：{e}"))?;
        let digest = self
            .docker()
            .inspect_image(&pull_ref)
            .await
            .ok()
            .and_then(|i| i.primary_digest_for(&pull_ref));
        let digest = match digest {
            Some(d) if !registered.is_empty() && digest_sha256(&d) != registered => {
                return Err(format!(
                    "拉取 {pull_ref} 后 digest 与登记值不一致：期望 {registered_digest}，实际 {d}"
                ));
            }
            Some(d) => d,
            None if !registered.is_empty() => registered_digest.to_string(),
            None => {
                self.job_note(
                    job_id,
                    &format!("警告：{pull_ref} 拉取成功但读不到 repo digest，image_digest 暂为空"),
                )
                .await;
                String::new()
            }
        };
        self.job_note(
            job_id,
            &format!("镜像 {pull_ref} 已就绪（digest {digest}）"),
        )
        .await;
        Ok(digest)
    }

    /// 把随包分发的 mlocati 扩展安装器写到 `<web_root>/skiff/tools/`。
    ///
    /// PHP 容器整目录挂载 `web_root`，因此容器内固定路径是
    /// `/www/skiff/tools/install-php-extensions`。内容一致时不重复写，避免每次
    /// 扩展任务都改文件 mtime。
    pub(crate) fn ensure_php_extension_installer(
        &self,
        s: &SiteSettings,
    ) -> Result<PathBuf, String> {
        let path = s.php_extension_installer();
        write_if_changed(
            &path,
            runtime_catalog::PHP_EXTENSION_INSTALLER.as_bytes(),
            0o755,
        )?;
        Ok(path)
    }

    /// 准备 PHP 宿主托管配置：`php/php.ini` 与 `php/conf.d/`。
    ///
    /// 两者都要在容器创建前存在，否则 Docker 会把缺失的宿主路径建成目录或拒绝挂载。
    pub(crate) fn ensure_php_config_files(
        &self,
        s: &SiteSettings,
        kind: &str,
        name: &str,
    ) -> Result<(), String> {
        std::fs::create_dir_all(s.runtime_php_confd(kind, name))
            .map_err(|e| format!("创建扩展 ini 目录失败：{e}"))?;
        let ini = s.runtime_php_ini(kind, name);
        if !ini.exists() {
            write_if_changed(&ini, DEFAULT_PHP_INI.as_bytes(), 0o644)?;
        }
        Ok(())
    }

    /// 写数据根的 PHP 容器身份初始化脚本（幂等；内容一致不重写）。
    ///
    /// 脚本只读挂载进容器，由 root 执行：按包管理器幂等建立 `skiff-web`
    /// 201000:201000，再 `exec php-fpm`；镜像自带的 `www-data` 保留不动。
    pub(crate) fn ensure_php_account_init(&self, s: &SiteSettings) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&s.data_dir)
            .map_err(|e| format!("创建数据根 {} 失败：{e}", s.data_dir.display()))?;
        let path = s.data_dir.join(PHP_INIT_SCRIPT_NAME);
        write_if_changed(&path, PHP_ACCOUNT_INIT_SCRIPT.as_bytes(), 0o644)?;
        Ok(path)
    }

    pub(crate) async fn ensure_container(&self, spec: &ContainerSpec) -> Result<(), String> {
        let created = self
            .kernel_execute(
                Verb::ContainerCreate,
                None,
                DockerParams::ContainerCreate(Box::new(spec.clone())),
            )
            .await
            .map_err(|e| e.to_string())?;
        let id = created
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return Err("create 未返回容器 id".into());
        }
        let obj = skiff_docker::ObjectRef {
            id,
            name: spec.name.clone().unwrap_or_default(),
            labels: spec.labels.clone(),
        };
        self.kernel_execute(Verb::Start, Some(&obj), DockerParams::None)
            .await
            .map(|_| ())
            .map_err(|e| format!("启动容器失败：{e}"))
    }

    pub(crate) fn nginx_spec(&self, s: &SiteSettings) -> ContainerSpec {
        ContainerSpec {
            image: s.nginx_image.clone(),
            name: Some(NGINX_CONTAINER.to_string()),
            command: None,
            entrypoint: None,
            env: BTreeMap::new(),
            labels: skiff_labels("nginx"),
            // host 网络：直接监听宿主 80，不再做 8080→80 端口映射。
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![
                Mount::Bind {
                    source: s.web_root.display().to_string(),
                    target: CONTAINER_WEB_ROOT.to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.conf_dir.display().to_string(),
                    target: s.container_conf_dir().to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.nginx_main_conf().display().to_string(),
                    target: s.container_nginx_main_conf().to_string(),
                    read_only: false,
                },
            ],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
            cpu_shares: None,
            cpu_period: None,
            cpu_quota: None,
            memory_swap_mb: None,
        }
    }

    pub(crate) fn php_spec_for(
        &self,
        s: &SiteSettings,
        kind: &str,
        name: &str,
        image: &str,
    ) -> ContainerSpec {
        let mut env = BTreeMap::new();
        // 让 PHP 同时扫描镜像自带 conf.d 与 Skiff 托管目录：后者放用户可编辑的扩展 ini。
        env.insert(
            "PHP_INI_SCAN_DIR".to_string(),
            PHP_INI_SCAN_DIR_VALUE.to_string(),
        );
        ContainerSpec {
            image: image.to_string(),
            name: Some(runtime_container_name(kind, name)),
            // 以 root 起、脚本内建号后再 exec php-fpm；不改镜像本身。
            command: Some(vec!["/bin/sh".to_string(), CONTAINER_PHP_INIT.to_string()]),
            entrypoint: None,
            env,
            labels: skiff_labels("php"),
            // 只走 unix socket，不暴露 9000；host 网络避免额外 bridge。
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![
                Mount::Bind {
                    source: s.web_root.display().to_string(),
                    target: CONTAINER_WEB_ROOT.to_string(),
                    read_only: false,
                },
                Mount::Bind {
                    source: s.runtime_pool_conf(kind, name).display().to_string(),
                    target: CONTAINER_PHP_POOL.to_string(),
                    read_only: true,
                },
                Mount::Bind {
                    source: s.runtime_php_ini(kind, name).display().to_string(),
                    target: CONTAINER_PHP_INI.to_string(),
                    read_only: true,
                },
                Mount::Bind {
                    source: s.runtime_php_confd(kind, name).display().to_string(),
                    target: CONTAINER_PHP_SKIFF_CONFD.to_string(),
                    read_only: true,
                },
                Mount::Bind {
                    source: s.data_dir.join(PHP_INIT_SCRIPT_NAME).display().to_string(),
                    target: CONTAINER_PHP_INIT.to_string(),
                    read_only: true,
                },
                Mount::Bind {
                    source: s.system_pkg_cache_dir().display().to_string(),
                    target: "/var/cache/apk".to_string(),
                    read_only: false,
                },
            ],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
            cpu_shares: None,
            cpu_period: None,
            cpu_quota: None,
            memory_swap_mb: None,
        }
    }

    pub(crate) fn rust_spec_for(
        &self,
        s: &SiteSettings,
        name: &str,
        image: &str,
        port: u16,
    ) -> ContainerSpec {
        let entry = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/entry.sh",
            skiff_store::KIND_RUST
        );
        let log = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/log/server.log",
            skiff_store::KIND_RUST
        );
        let target = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/target",
            skiff_store::KIND_RUST
        );
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), port.to_string());
        // 构建产物留在运行环境目录，站点目录只放源码（用户可整目录备份/替换源码）。
        env.insert("CARGO_TARGET_DIR".to_string(), target);
        ContainerSpec {
            image: image.to_string(),
            name: Some(runtime_container_name(skiff_store::KIND_RUST, name)),
            // 独立进程 + host 网络端口；容器重启后由 restart=unless-stopped 拉起。
            command: Some(rust_run_command(&entry, &log)),
            entrypoint: None,
            env,
            labels: skiff_labels("runtime"),
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![Mount::Bind {
                source: s.web_root.display().to_string(),
                target: CONTAINER_WEB_ROOT.to_string(),
                read_only: false,
            }],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
            cpu_shares: None,
            cpu_period: None,
            cpu_quota: None,
            memory_swap_mb: None,
        }
    }

    /// Node.js 容器：host 网络 + 固定端口，单进程常驻，启动命令来自专属配置。
    ///
    /// 不发布端口、不建 bridge：与 Rust 环境保持同一套网络模型，nginx 以
    /// `127.0.0.1:<port>` 反代；`HOST` 默认 `127.0.0.1`，应用读取它即可避免
    /// 意外监听全部网卡（应用若硬编码 0.0.0.0，需在宿主防火墙侧限制）。
    pub(crate) fn node_spec_for(
        &self,
        s: &SiteSettings,
        name: &str,
        image: &str,
        port: u16,
        cfg: &NodeRuntimeConfig,
    ) -> ContainerSpec {
        let entry = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/entry.sh",
            skiff_store::KIND_NODE
        );
        let log = format!(
            "{CONTAINER_RUNTIME_ROOT}/{}/{name}/log/server.log",
            skiff_store::KIND_NODE
        );
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), port.to_string());
        env.insert("HOST".to_string(), "127.0.0.1".to_string());
        env.insert("NODE_ENV".to_string(), cfg.node_env.clone());
        for (key, value) in &cfg.env {
            env.insert(key.clone(), value.clone());
        }
        ContainerSpec {
            image: image.to_string(),
            name: Some(runtime_container_name(skiff_store::KIND_NODE, name)),
            command: Some(node_run_command(
                &entry,
                &log,
                &cfg.argv()
                    .unwrap_or_else(|_| vec!["npm".into(), "start".into()]),
            )),
            entrypoint: None,
            env,
            labels: skiff_labels("runtime"),
            ports: vec![],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            network_mode: Some("host".into()),
            mounts: vec![Mount::Bind {
                source: s.web_root.display().to_string(),
                target: CONTAINER_WEB_ROOT.to_string(),
                read_only: false,
            }],
            log_driver: Some("local".into()),
            log_max_size: Some("10m".into()),
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
            cpu_shares: None,
            cpu_period: None,
            cpu_quota: None,
            memory_swap_mb: None,
        }
    }
}
