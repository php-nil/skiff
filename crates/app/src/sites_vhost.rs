//! 站点 vhost 与域名/别名判定（无状态纯函数）。
//!
//! 这是 `sites.rs` 拆分的第一步：本模块不持有 `AppState`、不碰数据库与 Docker，
//! 只做「输入 → 文本 / 规范化结果」的纯变换，因此可以脱开装配层直接单测。
//!
//! 对外路径保持不变：`sites.rs` 用 `pub use crate::sites_vhost::{...}` 再导出，
//! 上层仍按 `skiff_app::sites::validate_domain` 之类的旧路径调用，handler 不用改。

use std::path::{Path, PathBuf};

use skiff_store::{TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

use crate::sites::{
    default_php_name, runtime_container_socket, PhpRuntimeConfig, SiteError,
    CONTAINER_FASTCGI_PARAMS,
};

/// 域名白名单：小写字母、数字、`-`、`.`；每段非空且不以 `-` 开头/结尾。
///
/// 拒绝 `..`、`/`、端口、通配符与空白；同时限制总长 253、单段 63。
pub fn validate_domain(raw: &str) -> Result<String, SiteError> {
    let d = raw.trim().to_ascii_lowercase();
    if d.is_empty() {
        return Err(SiteError::BadRequest("域名不能为空".into()));
    }
    if d.len() > 253 {
        return Err(SiteError::BadRequest("域名长度超过 253".into()));
    }
    if d.contains('/') || d.contains('\\') || d.contains(' ') || d.contains('_') {
        return Err(SiteError::BadRequest(
            "域名只能包含字母、数字、点和中划线".into(),
        ));
    }
    let labels: Vec<&str> = d.split('.').collect();
    if labels.len() < 2 {
        return Err(SiteError::BadRequest(
            "域名至少包含两段（如 example.com）".into(),
        ));
    }
    for label in &labels {
        if label.is_empty() {
            return Err(SiteError::BadRequest("域名不能包含空段".into()));
        }
        if label.len() > 63 {
            return Err(SiteError::BadRequest("域名单段长度超过 63".into()));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(SiteError::BadRequest(
                "域名每段不能以中划线开头或结尾".into(),
            ));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(SiteError::BadRequest(
                "域名只能包含字母、数字、点和中划线".into(),
            ));
        }
    }
    // 纯数字最后一段（IPv4）不接受：站点按域名建目录，IP 访问请用默认站点。
    if labels.last().is_some_and(|l| {
        l.chars().all(|c| c.is_ascii_digit()) && labels[0].chars().all(|c| c.is_ascii_digit())
    }) {
        return Err(SiteError::BadRequest(
            "请使用域名而不是 IP 地址创建站点".into(),
        ));
    }
    Ok(d)
}

/// 批量别名解析：主域名与别名一次提交，去重与语法校验都收在这里。
///
/// `raw` 里每个元素可能是一行粘贴内容（换行/逗号/空格混合），也可能是单行输入。
/// 重复别名与「别名等于主域名」一律拒绝——不静默丢弃，避免用户以为已经加上。
pub fn normalize_site_aliases(domain: &str, raw: &[String]) -> Result<Vec<String>, SiteError> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        for token in item
            .split(|c: char| c == ',' || c == '，' || c == ';' || c == '；' || c.is_whitespace())
        {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            let alias = validate_domain(token)?;
            if alias == domain {
                return Err(SiteError::BadRequest("别名不能与主域名相同".into()));
            }
            if out.contains(&alias) {
                return Err(SiteError::Conflict(format!("域名重复：{alias}")));
            }
            out.push(alias);
        }
    }
    Ok(out)
}

/// 站点目录：用户只填目录名（如 `example.com`），前缀由内核自动补全。
///
/// 空值表示与主域名同名；只允许单段目录名，拒绝 `/`、`..` 与空白。
pub fn site_root_for(
    sites_dir: &Path,
    domain: &str,
    slug: Option<&str>,
) -> Result<PathBuf, SiteError> {
    let Some(slug) = slug.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(sites_dir.join(domain));
    };
    if slug == "." || slug == ".." || slug.contains(['/', '\\']) || slug.contains("..") {
        return Err(SiteError::BadRequest(
            "站点目录只能填写目录名，不能包含 / 或 ..".into(),
        ));
    }
    if slug.chars().any(char::is_whitespace)
        || !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(SiteError::BadRequest(
            "站点目录只能包含字母、数字、点、中划线与下划线".into(),
        ));
    }
    Ok(sites_dir.join(slug))
}

/// 站点类型白名单：只接受三类，大小写归一。
pub fn validate_site_type(raw: &str) -> Result<&'static str, SiteError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        TYPE_RUNTIME => Ok(TYPE_RUNTIME),
        TYPE_STATIC => Ok(TYPE_STATIC),
        TYPE_PROXY => Ok(TYPE_PROXY),
        other => Err(SiteError::BadRequest(format!(
            "未知站点类型：{other}（应为 runtime / static / proxy）"
        ))),
    }
}

/// 反向代理上游：必须是 `http(s)://host[:port][/path]`，禁止 userinfo、空白与裸路径。
pub fn validate_proxy_target(raw: &str) -> Result<String, SiteError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(SiteError::BadRequest("上游地址不能为空".into()));
    }
    if t.chars().any(char::is_whitespace) {
        return Err(SiteError::BadRequest("上游地址不能包含空白".into()));
    }
    let (scheme, rest) = t
        .split_once("://")
        .ok_or_else(|| SiteError::BadRequest("上游地址必须以 http:// 或 https:// 开头".into()))?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(SiteError::BadRequest(
            "上游地址只支持 http:// 或 https://".into(),
        ));
    }
    if rest.is_empty() {
        return Err(SiteError::BadRequest("上游地址缺少主机".into()));
    }
    if rest.contains('@') {
        return Err(SiteError::BadRequest("上游地址不能包含用户名/密码".into()));
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() {
        return Err(SiteError::BadRequest("上游地址缺少主机".into()));
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(SiteError::BadRequest("上游主机名包含非法字符".into()));
    }
    if let Some(port) = authority.split_once(':').map(|(_, p)| p) {
        let port: u16 = port
            .parse()
            .map_err(|_| SiteError::BadRequest("上游端口必须是 1-65535".into()))?;
        if port == 0 {
            return Err(SiteError::BadRequest("上游端口必须是 1-65535".into()));
        }
    }
    // 统一小写 scheme，其余保留用户写法（路径大小写敏感）。
    let normalized = format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        rest.trim_end_matches('/')
    );
    Ok(normalized)
}

/// 站点级 `app_port` 校验（保留给「每站点独立进程」模式；当前 Node.js / Rust 使用
/// 运行环境固定端口，不写该字段）。
pub fn validate_app_port(raw: i64) -> Result<u16, SiteError> {
    if !(1024..=65535).contains(&raw) {
        return Err(SiteError::BadRequest(
            "应用端口必须在 1024-65535 之间".into(),
        ));
    }
    Ok(raw as u16)
}

/// TLS 参数：证书/私钥使用**容器内** `/www/...` 路径。
#[derive(Debug, Clone)]
pub struct VhostTls {
    pub cert: String,
    pub key: String,
    /// 80 端口是否 301 到 443。
    pub force_https: bool,
}

/// vhost 的三种业务形态；容器内路径由调用方换算，生成函数不再硬编码 socket。
#[derive(Debug, Clone)]
pub enum VhostKind<'a> {
    /// 静态站：只读文档根，不接 PHP。
    Static { root: &'a str },
    /// 运行环境站（PHP）：fastcgi 指向运行环境登记的 socket。
    Php { root: &'a str, socket: &'a str },
    /// 运行环境站（Node.js / Rust）：独立进程监听固定端口，vhost 反代过去。
    Process { root: &'a str, port: u16 },
    /// 反向代理：不落文档根，只转发到上游。
    Proxy { target: &'a str, websocket: bool },
}

/// 生成站点 vhost。路径里的站点目录已换算为容器内 `/www/...`；
/// PHP 的 socket 来自 `site_runtimes.socket_path`，不在此硬编码。
pub fn render_site_vhost(
    domain: &str,
    aliases: &[String],
    kind: VhostKind<'_>,
    access_log: &str,
    error_log: &str,
    tls: Option<&VhostTls>,
) -> String {
    let names = if aliases.is_empty() {
        domain.to_string()
    } else {
        format!("{} {}", domain, aliases.join(" "))
    };
    let mut out = String::new();
    if tls.is_some_and(|t| t.force_https) {
        // 强制 HTTPS：80 只做跳转，不再渲染站点内容。
        out.push_str("server {\n    listen 80;\n");
        out.push_str(&format!("    server_name {names};\n"));
        out.push_str("    return 301 https://$host$request_uri;\n}\n");
    } else {
        out.push_str("server {\n    listen 80;\n");
        out.push_str(&format!("    server_name {names};\n"));
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl;\n");
        out.push_str(&format!("    server_name {names};\n"));
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    out
}

fn push_site_body(out: &mut String, kind: &VhostKind<'_>, access_log: &str, error_log: &str) {
    out.push_str(&format!("    access_log {access_log};\n"));
    out.push_str(&format!("    error_log {error_log};\n"));
    match kind {
        VhostKind::Static { root } => {
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    index index.html;\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
        }
        VhostKind::Php { root, socket } => {
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    index index.php index.html;\n");
            out.push_str("    location / {\n        try_files $uri $uri/ =404;\n    }\n");
            out.push_str(&format!(
                "    location ~ \\.php$ {{\n\
                 \x20       include {CONTAINER_FASTCGI_PARAMS};\n\
                 \x20       fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n"
            ));
            out.push_str(&format!("        fastcgi_pass unix:{socket};\n"));
            out.push_str("        fastcgi_index index.php;\n    }\n");
        }
        VhostKind::Process { root, port } => {
            // Rust 进程自带内容与路由：文档根只作兜底静态资源，主路径反代到端口。
            out.push_str(&format!("    root {root};\n"));
            out.push_str("    location / {\n");
            out.push_str(&format!("        proxy_pass http://127.0.0.1:{port};\n"));
            out.push_str("        proxy_http_version 1.1;\n");
            out.push_str("        proxy_set_header Host $host;\n");
            out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
            out.push_str("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n");
            out.push_str("        proxy_set_header X-Forwarded-Proto $scheme;\n");
            out.push_str("        proxy_set_header Upgrade $http_upgrade;\n");
            out.push_str("        proxy_set_header Connection \"upgrade\";\n");
            out.push_str("        proxy_read_timeout 3600s;\n    }\n");
        }
        VhostKind::Proxy { target, websocket } => {
            out.push_str("    location / {\n");
            out.push_str(&format!("        proxy_pass {target};\n"));
            out.push_str("        proxy_http_version 1.1;\n");
            out.push_str("        proxy_set_header Host $host;\n");
            out.push_str("        proxy_set_header X-Real-IP $remote_addr;\n");
            out.push_str("        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n");
            out.push_str("        proxy_set_header X-Forwarded-Proto $scheme;\n");
            if *websocket {
                out.push_str("        proxy_set_header Upgrade $http_upgrade;\n");
                out.push_str("        proxy_set_header Connection \"upgrade\";\n");
            }
            out.push_str("        proxy_read_timeout 3600s;\n");
            out.push_str("    }\n");
        }
    }
    out.push_str("}\n");
}

pub(crate) fn default_probe_php() -> &'static str {
    "<?php\n// Skiff 站点探针：确认 PHP-FPM 正常渲染。\nphpinfo();\n"
}

pub(crate) fn default_probe_html() -> &'static str {
    "<!doctype html>\n<html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
     <title>Skiff 静态站点</title></head>\n<body><h1>站点已就绪</h1>\
     <p>把您的静态文件放入 <code>index/</code> 目录即可。</p></body></html>\n"
}

/// 默认站点 vhost：直接访问 IP:80 时落到该站点的 `public`。
///
/// `default_server` 只能有一个；本文件存在时由它接管裸访问，不存在时由
/// `00-skiff-fallback.conf`（按文件名先于站点 vhost 加载）返回 404。
pub fn render_default_vhost(
    domain: &str,
    kind: VhostKind<'_>,
    access_log: &str,
    error_log: &str,
    tls: Option<&VhostTls>,
) -> String {
    let mut out = format!("# Skiff 默认站点：直接访问 IP:80 时展示 {domain}。\n");
    if tls.is_some_and(|t| t.force_https) {
        out.push_str("server {\n    listen 80 default_server;\n    server_name _;\n");
        out.push_str("    return 301 https://$host$request_uri;\n}\n");
    } else {
        out.push_str("server {\n    listen 80 default_server;\n    server_name _;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    if let Some(t) = tls {
        out.push_str("server {\n    listen 443 ssl default_server;\n    server_name _;\n");
        out.push_str(&format!("    ssl_certificate {};\n", t.cert));
        out.push_str(&format!("    ssl_certificate_key {};\n", t.key));
        out.push_str("    ssl_protocols TLSv1.2 TLSv1.3;\n");
        push_site_body(&mut out, &kind, access_log, error_log);
    }
    out
}

/// PHP-FPM 池配置：只监听 unix socket（不暴露 9000），socket 目录在共享的 `/www`。
pub fn render_php_pool_with(cfg: &PhpRuntimeConfig, socket: &str) -> String {
    format!(
        "[www]\n\
         user = www-data\n\
         group = www-data\n\
         listen = {socket}\n\
         listen.owner = root\n\
         ; nginx worker 以 nobody(65534) 运行；只给同组读写，others 无权限。\n\
         listen.group = nobody\n\
         listen.mode = 0660\n\
         pm = {pm}\n\
         pm.max_children = {max_children}\n\
         pm.start_servers = {start_servers}\n\
         pm.min_spare_servers = {min_spare_servers}\n\
         pm.max_spare_servers = {max_spare_servers}\n\
         pm.max_requests = {max_requests}\n\
         php_admin_value[memory_limit] = {memory_limit}\n\
         php_admin_value[upload_max_filesize] = {upload_max_filesize}\n\
         php_admin_value[post_max_size] = {post_max_size}\n\
         php_admin_value[max_execution_time] = {max_execution_time}\n\
         ; 镜像 docker.conf 默认 access.log = /proc/self/fd/2，访问日志经容器 stdout\n\
         ; 落到 dockerd/shims；nginx 侧已记录同一请求，这里显式关闭以免每请求重复写。\n\
         access.log = /dev/null\n\
         catch_workers_output = yes\n",
        pm = cfg.pm,
        max_children = cfg.max_children,
        start_servers = cfg.start_servers,
        min_spare_servers = cfg.min_spare_servers,
        max_spare_servers = cfg.max_spare_servers,
        max_requests = cfg.max_requests,
        memory_limit = cfg.memory_limit,
        upload_max_filesize = cfg.upload_max_filesize,
        post_max_size = cfg.post_max_size,
        max_execution_time = cfg.max_execution_time,
    )
}

/// 安装/默认池：使用默认配置与共享安装 socket。
pub fn render_php_pool() -> String {
    render_php_pool_with(
        &PhpRuntimeConfig::default(),
        &runtime_container_socket(skiff_store::KIND_PHP, &default_php_name()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// golden：静态站 vhost 全文。拼 expected 时用显式 `\n`，避免宿主换行符差异。
    #[test]
    fn golden_static_vhost() {
        let v = render_site_vhost(
            "example.com",
            &["www.example.com".to_string()],
            VhostKind::Static {
                root: "/www/sites/example.com/index",
            },
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
            None,
        );
        let expected = concat!(
            "server {\n",
            "    listen 80;\n",
            "    server_name example.com www.example.com;\n",
            "    access_log /www/sites/example.com/log/access.log;\n",
            "    error_log /www/sites/example.com/log/error.log;\n",
            "    root /www/sites/example.com/index;\n",
            "    index index.html;\n",
            "    location / {\n",
            "        try_files $uri $uri/ =404;\n",
            "    }\n",
            "}\n",
        );
        assert_eq!(v, expected);
    }

    /// golden：PHP 站 + 强制 HTTPS。80 只跳转，443 里是完整 PHP 块，
    /// `include` 必须是绝对路径（候选配置校验时相对路径会被解析错）。
    #[test]
    fn golden_php_vhost_with_force_https() {
        let tls = VhostTls {
            cert: "/www/sites/example.com/ssl/cert.pem".to_string(),
            key: "/www/sites/example.com/ssl/key.pem".to_string(),
            force_https: true,
        };
        let v = render_site_vhost(
            "example.com",
            &[],
            VhostKind::Php {
                root: "/www/sites/example.com/index",
                socket: "/www/runtime/php/php-8.5/php-fpm.sock",
            },
            "/www/sites/example.com/log/access.log",
            "/www/sites/example.com/log/error.log",
            Some(&tls),
        );
        let expected = concat!(
            "server {\n",
            "    listen 80;\n",
            "    server_name example.com;\n",
            "    return 301 https://$host$request_uri;\n",
            "}\n",
            "server {\n",
            "    listen 443 ssl;\n",
            "    server_name example.com;\n",
            "    ssl_certificate /www/sites/example.com/ssl/cert.pem;\n",
            "    ssl_certificate_key /www/sites/example.com/ssl/key.pem;\n",
            "    ssl_protocols TLSv1.2 TLSv1.3;\n",
            "    access_log /www/sites/example.com/log/access.log;\n",
            "    error_log /www/sites/example.com/log/error.log;\n",
            "    root /www/sites/example.com/index;\n",
            "    index index.php index.html;\n",
            "    location / {\n",
            "        try_files $uri $uri/ =404;\n",
            "    }\n",
            "    location ~ \\.php$ {\n",
            "        include /usr/local/openresty/nginx/conf/fastcgi_params;\n",
            "        fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n",
            "        fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;\n",
            "        fastcgi_index index.php;\n",
            "    }\n",
            "}\n",
        );
        assert_eq!(v, expected);
    }

    /// golden：FPM 池配置全文（默认值 + 指定 socket）。
    #[test]
    fn golden_php_pool() {
        let cfg = PhpRuntimeConfig::default();
        let pool = render_php_pool_with(&cfg, "/www/runtime/php/php-8.5/php-fpm.sock");
        let expected = concat!(
            "[www]\n",
            "user = www-data\n",
            "group = www-data\n",
            "listen = /www/runtime/php/php-8.5/php-fpm.sock\n",
            "listen.owner = root\n",
            "; nginx worker 以 nobody(65534) 运行；只给同组读写，others 无权限。\n",
            "listen.group = nobody\n",
            "listen.mode = 0660\n",
            "pm = dynamic\n",
            "pm.max_children = 20\n",
            "pm.start_servers = 3\n",
            "pm.min_spare_servers = 2\n",
            "pm.max_spare_servers = 8\n",
            "pm.max_requests = 500\n",
            "php_admin_value[memory_limit] = 256M\n",
            "php_admin_value[upload_max_filesize] = 64M\n",
            "php_admin_value[post_max_size] = 64M\n",
            "php_admin_value[max_execution_time] = 30\n",
            "; 镜像 docker.conf 默认 access.log = /proc/self/fd/2，访问日志经容器 stdout\n",
            "; 落到 dockerd/shims；nginx 侧已记录同一请求，这里显式关闭以免每请求重复写。\n",
            "access.log = /dev/null\n",
            "catch_workers_output = yes\n",
        );
        assert_eq!(pool, expected);
    }
}
