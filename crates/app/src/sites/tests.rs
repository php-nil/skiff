//! 站点模块的单元测试：只覆盖内核侧的编排与判定，真实容器动作由上机用例兜底。

use super::*;
use skiff_docker::ContainerRow;
use skiff_store::{NewRuntime, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn php_init_script_resolves_nobody_from_gid_not_family_name() {
    // Debian 的 65534 组名是 nogroup；脚本必须建 `nobody` 同 gid 别名，
    // 而不是按镜像家族写死组名（自建镜像同样适用）。
    assert!(PHP_ACCOUNT_INIT_SCRIPT.contains(&format!("SOCKET_GID={SOCKET_GID}")));
    let block = PHP_ACCOUNT_INIT_SCRIPT
        .split("池模板固定写 listen.group = nobody")
        .nth(1)
        .expect("初始化脚本必须包含 nobody 组别名段");
    // Debian（groupadd）与 Alpine（addgroup）各自有兜底，最后还有 /etc/group 直写；
    // 两条路径都只依赖 gid 65534，不依赖某个组名恰好存在。
    assert!(block.contains("groupadd -o -g \"$SOCKET_GID\" nobody"));
    assert!(block.contains("addgroup -g \"$SOCKET_GID\" -S nobody"));
    assert!(block.contains("'nobody:x:%s:\\n'"));
    // 别名段本身不得再按镜像家族分支：同一段逻辑覆盖两种家族与自建镜像。
    assert!(!block.contains("alpine-release"));
    assert!(!block.contains("debian_version"));
    // 模板不出现 nogroup：Debian 的组名细节只由初始化脚本用 gid 兜住。
    assert!(!block.contains("listen.group = nogroup"));
}

#[test]
fn php_ini_scan_dir_lists_each_directory_once() {
    // 前导冒号在 PHP 里表示「沿用编译期默认扫描目录」；官方镜像的默认目录
    // 就是 conf.d，一旦保留就会重复扫描并让扩展 ini 加载两次。
    assert!(
        !PHP_INI_SCAN_DIR_VALUE.starts_with(':'),
        "前导冒号会重新引入默认目录，导致 conf.d 被扫两次：{PHP_INI_SCAN_DIR_VALUE}"
    );
    let dirs: Vec<&str> = PHP_INI_SCAN_DIR_VALUE.split(':').collect();
    assert_eq!(
        dirs,
        vec![CONTAINER_PHP_CONFD, CONTAINER_PHP_SKIFF_CONFD],
        "必须显式列出 conf.d 与 conf.d-skiff，且顺序为镜像目录在前"
    );
    // 空段等价于默认目录，同样会造成重复。
    assert!(
        dirs.iter().all(|d| !d.is_empty()),
        "扫描目录不能有空段：{dirs:?}"
    );
    let mut unique = dirs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), dirs.len(), "扫描目录不得重复：{dirs:?}");

    // 容器环境变量必须与常量一致，避免测试与真实 spec 分叉。
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = settings_at(dir.path());
    let php = state.php_spec_for(&s, "php", "scan1", "php:8.5-fpm-alpine");
    assert_eq!(
        php.env.get("PHP_INI_SCAN_DIR").map(String::as_str),
        Some(PHP_INI_SCAN_DIR_VALUE)
    );
}

#[test]
fn timeout_message_never_renders_zero_minutes() {
    assert_eq!(fmt_timeout(std::time::Duration::from_secs(3)), "3 秒");
    assert_eq!(fmt_timeout(std::time::Duration::from_secs(90)), "90 秒");
    assert_eq!(
        fmt_timeout(std::time::Duration::from_secs(15 * 60)),
        "15 分钟"
    );
}

/// 证书解析缓存只按 `(mtime, size)` 命中：文件改写后必须重新解析，
/// 否则换证后 SSL 标签会一直显示旧主体/旧到期时间。
#[test]
fn tls_cert_text_cache_invalidates_on_file_change() {
    let dir = std::env::temp_dir().join(format!("skiff-tls-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    std::fs::write(&cert, b"first").expect("写证书");
    std::fs::write(&key, b"k1").expect("写私钥");

    let (cert_stamp, key_stamp) = (file_stamp(&cert), file_stamp(&key));
    tls_info_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            cert.clone(),
            TlsInfoCacheEntry {
                cert: cert_stamp,
                key: key_stamp,
                text: TlsCertText {
                    subject: Some("CN=old".into()),
                    issuer: None,
                    expires: None,
                },
            },
        );

    // 文件未变：命中缓存，不跑 openssl。
    assert_eq!(
        cached_cert_text(&cert, &key).subject.as_deref(),
        Some("CN=old")
    );

    // 证书被改写（大小变化）：指纹失效，必须重新解析。
    std::fs::write(&cert, b"second-certificate").expect("覆写证书");
    assert_ne!(
        cached_cert_text(&cert, &key).subject.as_deref(),
        Some("CN=old"),
        "文件变化后不得继续返回旧解析结果"
    );

    // 站点/证书删除：条目不残留（`site_tls_info` 的不存在分支会这样清）。
    std::fs::remove_file(&cert).expect("删除证书");
    tls_info_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&cert);
    assert!(!tls_info_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&cert));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runtime_container_state_distinguishes_unknown_missing_and_stopped() {
    // Docker 不可达/列表失败：状态未知，不能伪造成 missing 或 running。
    assert_eq!(probe_container_state(false, None), "unknown");
    assert_eq!(probe_container_state(false, Some("running")), "unknown");
    // Docker 可查但列表里没有该容器：确实没安装。
    assert_eq!(probe_container_state(true, None), "missing");
    // 容器存在但已退出/终止：归一成 stopped，界面显示「已停止」而不是「运行中」或「未知」。
    for raw in [
        "exited",
        "dead",
        "created",
        "paused",
        "restarting",
        "stopped",
    ] {
        assert_eq!(runtime_effective_state(raw), "stopped", "{raw}");
    }
    assert_eq!(runtime_effective_state("running"), "running");
    assert_eq!(runtime_effective_state(""), "unknown");
    assert_eq!(runtime_effective_state("weird-state"), "unknown");
}

#[test]
fn runtime_site_status_distinguishes_unknown_missing_and_stopped() {
    use SiteRuntimeStatus as S;
    // 记录缺失 / 容器不存在 / 容器停止 / Docker 不可达 / 安装中 / 异常 / 未接入必须分开。
    assert_eq!(site_runtime_status("running", None), S::RecordMissing);
    assert_eq!(site_runtime_status("running", Some("missing")), S::Missing);
    assert_eq!(
        site_runtime_status("running", Some("stopped")),
        S::EnvironmentStopped
    );
    assert_eq!(
        site_runtime_status("running", Some("unknown")),
        S::DockerUnavailable
    );
    assert_eq!(site_runtime_status("running", Some("running")), S::Running);
    assert_eq!(
        site_runtime_status("running", Some("installing")),
        S::Installing
    );
    assert_eq!(site_runtime_status("running", Some("error")), S::Error);
    assert_eq!(
        site_runtime_status("running", Some("unimplemented")),
        S::Unimplemented
    );
    // 站点自身已停止时优先展示站点状态。
    assert_eq!(
        site_runtime_status("stopped", Some("running")),
        S::SiteStopped
    );
}

#[test]
fn runtime_version_state_distinguishes_actual_missing_and_unreadable() {
    assert_eq!(
        runtime_version_state(Some("8.5.12"), true),
        RuntimeVersionState::Actual("8.5.12")
    );
    assert_eq!(
        runtime_version_state(None, false),
        RuntimeVersionState::NotInstalled
    );
    assert_eq!(
        runtime_version_state(None, true),
        RuntimeVersionState::Unreadable
    );
}

#[test]
fn runtime_site_defaults_follow_runtime_kind() {
    // PHP 站点：探针 index.php。
    let php = runtime_site_default_files(skiff_store::KIND_PHP, "8.5", None);
    assert_eq!(php.len(), 1);
    assert_eq!(php[0].0, "index.php");
    // 方案 A：Node.js / Rust 的代码固定在环境 `app/`，站点只是域名入口，不写脚手架。
    assert!(runtime_site_default_files(skiff_store::KIND_RUST, "1.83", Some(9101)).is_empty());
    assert!(runtime_site_default_files(skiff_store::KIND_NODE, "22", Some(9200)).is_empty());
    // 未接入的 Go 同样不给默认文件，避免伪造可用性。
    assert!(runtime_site_default_files("go", "1.24", None).is_empty());
}

#[test]
fn node_entry_script_uses_fixed_app_dir() {
    // 与 PHP 不同：Node 入口脚本固定进入 `app/`，不读绑定站点标记，也不接 FastCGI。
    let script = node_entry_script();
    assert!(script.contains("APP=\"$DIR/app\""));
    assert!(
        !script.contains("$DIR/site"),
        "不得再按绑定站点切换源码目录"
    );
    assert!(script.contains("package.json"));
    assert!(script.contains("exec \"$@\""));
    assert!(!script.contains("php-fpm"));
}

#[test]
fn node_config_defaults_are_single_process_contract() {
    let cfg = NodeRuntimeConfig::default();
    assert_eq!(cfg.command, "npm start");
    assert_eq!(cfg.health_path, "/");
    assert_eq!(cfg.node_env, "production");
    assert_eq!(cfg.argv().unwrap(), vec!["npm", "start"]);
    cfg.validate().expect("默认配置必须合法");
}

#[test]
fn node_config_rejects_shell_metacharacters_and_reserved_env() {
    let cfg = NodeRuntimeConfig {
        command: "node server.js; rm -rf /www".into(),
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "分号必须被拒绝，不能进容器 shell");

    let cfg = NodeRuntimeConfig {
        command: "node server.js".into(),
        ..Default::default()
    };
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.argv().unwrap(), vec!["node", "server.js"]);

    let cfg = NodeRuntimeConfig {
        env: vec![("PORT".into(), "3000".into())],
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "PORT 由内核注入，不能覆盖");

    let cfg = NodeRuntimeConfig {
        env: vec![("lower".into(), "1".into())],
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "环境变量名必须大写");

    let cfg = NodeRuntimeConfig {
        health_path: "health".into(),
        ..Default::default()
    };
    assert!(cfg.validate().is_err(), "健康检查路径必须以 / 开头");
}

#[test]
fn node_config_env_text_roundtrip() {
    let mut cfg = NodeRuntimeConfig::default();
    cfg.set_env_text("# 注释\nAPI_BASE=https://api.example.com\nFEATURE_X=1\n\n")
        .unwrap();
    assert_eq!(cfg.env.len(), 2);
    assert_eq!(
        cfg.env_text(),
        "API_BASE=https://api.example.com\nFEATURE_X=1"
    );
    let json = cfg.to_json();
    let back = NodeRuntimeConfig::from_json(&json);
    assert_eq!(back, cfg);
    assert!(cfg.set_env_text("NOPE").is_err(), "缺 = 必须报错");
    assert!(cfg.set_env_text("A=1\nA=2").is_err(), "重复键必须报错");
}

#[test]
fn rust_entry_script_uses_fixed_app_dir() {
    // 源码目录固定为环境 `app/`：不再有“绑定站点即源码目录”的切换。
    let script = rust_entry_script();
    assert!(script.contains("APP=\"$DIR/app\""));
    assert!(!script.contains("$DIR/site"));
    assert!(script.contains("--manifest-path"));
}

#[test]
fn process_port_must_stay_in_kind_range_and_be_free() {
    // 端口属于环境：必须落在该 kind 的区间内，且不能被其它环境占用。
    assert!(validate_process_port(skiff_store::KIND_NODE, 9200, &[]).is_ok());
    assert!(validate_process_port(skiff_store::KIND_NODE, 9199, &[]).is_err());
    assert!(validate_process_port(skiff_store::KIND_NODE, 9200, &[9200]).is_err());
    assert!(validate_process_port(skiff_store::KIND_RUST, 9100, &[9100]).is_err());
    // PHP 走 unix socket，不占宿主端口。
    assert!(validate_process_port(skiff_store::KIND_PHP, 9200, &[]).is_err());
}

#[test]
fn clone_copy_skips_dependencies_and_keeps_sources() {
    // 克隆复制 `app/` 时必须跳过 node_modules / target，避免把依赖树复制一份。
    let base = std::env::temp_dir().join(format!("skiff-clone-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let from = base.join("from/app");
    let to = base.join("to/app");
    std::fs::create_dir_all(from.join("src")).expect("mkdir src");
    std::fs::create_dir_all(from.join("node_modules/dep")).expect("mkdir modules");
    std::fs::write(from.join("package.json"), "{}").expect("write manifest");
    std::fs::write(from.join("src/main.js"), "x").expect("write source");
    std::fs::write(from.join("node_modules/dep/index.js"), "x").expect("write dep");
    copy_dir_skip(&from, &to, &["node_modules", "target"]).expect("copy");
    assert!(to.join("package.json").is_file());
    assert!(to.join("src/main.js").is_file());
    assert!(!to.join("node_modules").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn php_validation_warnings_only_collects_warning_lines() {
    // `PHP Warning` 不阻止保存，但必须原文回显；普通输出与错误行不混进 warnings。
    let out = "PHP Warning:  Module \"foo\" is already loaded in Unknown on line 0\n\
                   skiff-ok\n\
                   PHP Warning:  Module \"foo\" is already loaded in Unknown on line 0\n\
                   PHP Fatal error: Uncaught Exception\n";
    let warnings = php_validation_warnings(out);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].starts_with("PHP Warning:"));
    assert!(!warnings[0].contains("skiff-ok"));
}

#[test]
fn php_validation_rejects_invalid_output_even_when_exit_zero() {
    // PHP ini 语法错误只写到 stdout，且 `-r` 仍会执行打印标记得以返回 0；
    // 这种情况下必须判无效，否则坏配置会覆盖原文件。
    let out = "PHP:  syntax error, unexpected end of file in /probe/b.ini on line 2\nskiff-ok";
    assert!(php_output_has_syntax_error(out));
    let ok = "PHP Version => 8.5.0\nskiff-ok";
    assert!(!php_output_has_syntax_error(ok));
}

#[test]
fn dir_usage_counts_nested_files_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("index/src")).unwrap();
    std::fs::write(dir.path().join("index/a.txt"), "12345").unwrap();
    std::fs::write(dir.path().join("index/src/main.rs"), "ok").unwrap();
    let (bytes, files) = dir_usage(dir.path());
    assert_eq!(files, 2);
    assert_eq!(bytes, 7);
}

#[test]
fn domain_validation_rejects_unsafe_input() {
    assert_eq!(validate_domain("Example.COM").unwrap(), "example.com");
    assert_eq!(
        validate_domain("a-b.example.com").unwrap(),
        "a-b.example.com"
    );
    for bad in [
        "",
        "../etc",
        "a/b",
        "exa mple.com",
        "example",
        ".example.com",
        "example..com",
        "-example.com",
        "example-.com",
        "1.2.3.4",
        "under_score.com",
    ] {
        assert!(validate_domain(bad).is_err(), "{bad:?} 应被拒绝");
    }
}

#[test]
fn alias_batch_parsing_normalizes_and_rejects_duplicates() {
    // 一次粘贴多行/逗号/空格混合：统一拆分并小写归一。
    let ok = normalize_site_aliases(
        "example.com",
        &[
            "www.example.com, api.example.com".to_string(),
            "static.example.com\nAlt.Example.COM".to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        ok,
        vec![
            "www.example.com",
            "api.example.com",
            "static.example.com",
            "alt.example.com"
        ]
    );
    // 重复别名明确拒绝，而不是静默去重。
    let dup = normalize_site_aliases(
        "example.com",
        &["www.example.com".to_string(), "WWW.example.com".to_string()],
    )
    .unwrap_err();
    assert!(
        matches!(&dup, SiteError::Conflict(m) if m.contains("域名重复")),
        "{dup:?}"
    );
    // 与主域名相同同样拒绝。
    assert!(normalize_site_aliases("example.com", &["example.com".to_string()]).is_err());
    // 空行/空值直接跳过。
    assert!(normalize_site_aliases("example.com", &[" , ".to_string()])
        .unwrap()
        .is_empty());
}

#[test]
fn local_extension_installer_is_materialized_and_kept_in_sync() {
    let dir = std::env::temp_dir().join(format!(
        "skiff-ipe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = SiteSettings {
        web_root: dir.clone(),
        sites_dir: dir.join("sites"),
        conf_dir: dir.join("nginx").join("conf.d"),
        runtime_dir: dir.join("runtime"),
        data_dir: dir.join("data"),
        nginx_image: String::new(),
        php_image: String::new(),
        port: 80,
        registry_mirror: String::new(),
    }
    .php_extension_installer();
    write_if_changed(&path, b"# local v1\n", 0o755).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"# local v1\n");
    // 内容一致不重复写（mtime 不变）。
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    write_if_changed(&path, b"# local v1\n", 0o755).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    // 内容不同则原子替换。
    write_if_changed(&path, b"# local v2\n", 0o755).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"# local v2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn site_root_defaults_to_domain_and_rejects_escape() {
    let dir = Path::new("/www/sites");
    // 空值 = 与主域名同名（前端自动填的兜底）。
    assert_eq!(
        site_root_for(dir, "example.com", None).unwrap(),
        dir.join("example.com")
    );
    assert_eq!(
        site_root_for(dir, "example.com", Some("  ")).unwrap(),
        dir.join("example.com")
    );
    assert_eq!(
        site_root_for(dir, "example.com", Some("blog")).unwrap(),
        dir.join("blog")
    );
    for bad in ["../etc", "a/b", "a b", "/abs", "..", "a..b"] {
        assert!(
            site_root_for(dir, "example.com", Some(bad)).is_err(),
            "{bad:?} 应被拒绝"
        );
    }
}

#[test]
fn site_type_and_proxy_target_validation() {
    assert_eq!(validate_site_type("runtime").unwrap(), TYPE_RUNTIME);
    assert_eq!(validate_site_type("STATIC").unwrap(), TYPE_STATIC);
    assert_eq!(validate_site_type("proxy").unwrap(), TYPE_PROXY);
    assert!(validate_site_type("node").is_err());

    assert_eq!(
        validate_proxy_target("http://127.0.0.1:8080/").unwrap(),
        "http://127.0.0.1:8080"
    );
    for bad in [
        "",
        "127.0.0.1:8080",
        "ftp://x.example",
        "http://",
        "http://user:pw@host",
        "http://bad host",
        "http://host:0",
        "http://host:99999",
    ] {
        assert!(validate_proxy_target(bad).is_err(), "{bad:?} 应被拒绝");
    }
}

#[test]
fn php_vhost_points_fastcgi_at_runtime_socket() {
    let v = render_site_vhost(
        "example.com",
        &[],
        VhostKind::Php {
            root: "/www/sites/example.com/index",
            socket: "/www/runtime/php/php-8.5/php-fpm.sock",
        },
        "/www/sites/example.com/log/access.log",
        "/www/sites/example.com/log/error.log",
        None,
    );
    assert!(v.contains("server_name example.com;"));
    assert!(v.contains("root /www/sites/example.com/index;"));
    assert!(v.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"));
    assert!(!v.contains("9000"));
    assert!(v.contains("SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
    // 主配置候选校验时配置目录会变成 /www/nginx，include 必须写绝对路径。
    assert!(v.contains(&format!("include {CONTAINER_FASTCGI_PARAMS};")));
    assert!(!v.contains("include fastcgi_params;"));
    assert!(v.contains("access_log /www/sites/example.com/log/access.log;"));
    assert!(v.contains("error_log /www/sites/example.com/log/error.log;"));
    assert!(!v.contains("/www/logs/"), "不得再引用旧的集中日志目录");
    assert!(!v.contains("/public"), "不得再引用旧的 public 根目录");
    assert!(!v.contains("listen 443"));
    assert!(v.contains("listen 80;"));
}

#[test]
fn static_vhost_has_no_php_but_reads_index() {
    let v = render_site_vhost(
        "static.local",
        &[],
        VhostKind::Static {
            root: "/www/sites/static.local/index",
        },
        "/www/sites/static.local/log/access.log",
        "/www/sites/static.local/log/error.log",
        None,
    );
    assert!(v.contains("root /www/sites/static.local/index;"));
    assert!(v.contains("index index.html;"));
    assert!(v.contains("try_files $uri $uri/ =404;"));
    assert!(!v.contains("fastcgi_pass"), "静态站不得接 PHP");
    assert!(!v.contains("index.php"));
}

#[test]
fn proxy_vhost_forwards_and_optionally_upgrades_websocket() {
    let ws = render_site_vhost(
        "proxy.local",
        &[],
        VhostKind::Proxy {
            target: "http://127.0.0.1:8080",
            websocket: true,
        },
        "/www/sites/proxy.local/log/access.log",
        "/www/sites/proxy.local/log/error.log",
        None,
    );
    assert!(ws.contains("proxy_pass http://127.0.0.1:8080;"));
    assert!(ws.contains("proxy_http_version 1.1;"));
    assert!(ws.contains("proxy_set_header Host $host;"));
    assert!(ws.contains("proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;"));
    assert!(ws.contains("proxy_set_header Upgrade $http_upgrade;"));
    assert!(ws.contains("proxy_set_header Connection \"upgrade\";"));
    assert!(!ws.contains("root "), "反代不落文档根");
    assert!(!ws.contains("fastcgi_pass"));

    let plain = render_site_vhost(
        "proxy.local",
        &[],
        VhostKind::Proxy {
            target: "https://api.example.com:8443/base",
            websocket: false,
        },
        "/www/sites/proxy.local/log/access.log",
        "/www/sites/proxy.local/log/error.log",
        None,
    );
    assert!(plain.contains("proxy_pass https://api.example.com:8443/base;"));
    assert!(!plain.contains("Upgrade $http_upgrade"));
}

#[test]
fn php_config_validation_rejects_out_of_range_and_unsafe_values() {
    PhpRuntimeConfig::default().validate().unwrap();

    let cfg = PhpRuntimeConfig {
        memory_limit: "8M".into(),
        ..PhpRuntimeConfig::default()
    };
    assert!(cfg.validate().is_err(), "memory_limit 下限 32M");

    let cfg = PhpRuntimeConfig {
        memory_limit: "9999M".into(),
        ..PhpRuntimeConfig::default()
    };
    assert!(cfg.validate().is_err());

    let cfg = PhpRuntimeConfig {
        upload_max_filesize: "256M".into(),
        post_max_size: "64M".into(),
        ..PhpRuntimeConfig::default()
    };
    assert!(cfg.validate().is_err(), "post 不能小于 upload");

    let cfg = PhpRuntimeConfig {
        max_children: 100,
        start_servers: 120,
        ..PhpRuntimeConfig::default()
    };
    assert!(cfg.validate().is_err());

    let cfg = PhpRuntimeConfig {
        pm: "static".into(),
        ..PhpRuntimeConfig::default()
    };
    assert!(cfg.validate().is_err());

    // 合法配置可往返序列化，且不丢字段。
    let cfg = PhpRuntimeConfig {
        memory_limit: "512M".into(),
        upload_max_filesize: "128M".into(),
        post_max_size: "128M".into(),
        max_execution_time: 120,
        max_children: 40,
        start_servers: 5,
        min_spare_servers: 3,
        max_spare_servers: 12,
        max_requests: 800,
        ..PhpRuntimeConfig::default()
    };
    cfg.validate().unwrap();
    let back = PhpRuntimeConfig::from_json(&cfg.to_json());
    assert_eq!(back, cfg);
}

#[test]
fn php_pool_uses_config_and_runtime_socket() {
    let cfg = PhpRuntimeConfig::default();
    let pool = render_php_pool_with(&cfg, "/www/runtime/php/php-8.5/php-fpm.sock");
    assert!(pool.contains("listen = /www/runtime/php/php-8.5/php-fpm.sock"));
    assert!(pool.contains("listen.mode = 0660"));
    assert!(pool.contains("pm = dynamic"));
    assert!(pool.contains("pm.max_children = 20"));
    assert!(pool.contains("php_admin_value[memory_limit] = 256M"));
    assert!(pool.contains("php_admin_value[upload_max_filesize] = 64M"));
    assert!(pool.contains("php_admin_value[max_execution_time] = 30"));
    assert!(
        pool.contains("access.log = /dev/null"),
        "必须覆盖镜像的 /proc/self/fd/2，避免每请求重复写访问日志"
    );
}

fn settings_at(root: &Path) -> SiteSettings {
    SiteSettings {
        web_root: root.to_path_buf(),
        sites_dir: root.join("sites"),
        conf_dir: root.join("nginx").join("conf.d"),
        runtime_dir: root.join("runtime"),
        data_dir: root.join("data"),
        nginx_image: DEFAULT_NGINX_IMAGE.into(),
        php_image: DEFAULT_PHP_IMAGE.into(),
        port: DEFAULT_PORT,
        registry_mirror: String::new(),
    }
}

/// 第（无 Docker）测试用应用上下文：数据目录落在临时目录，避免污染源码树。
fn app_state_at(root: &Path) -> Arc<AppState> {
    crate::commands::test_support::state_at(root)
}

/// 内核自建容器（Web 服务 + 各运行环境）必须统一带保护标签，
/// 否则面板能把自己的 nginx / PHP / Rust 容器停掉。
#[test]
fn kernel_container_specs_carry_protected_label() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = settings_at(dir.path());
    let nginx = state.nginx_spec(&s);
    let php = state.php_spec_for(&s, "php", "batch1", "php:8.5-fpm-alpine");
    let rust = state.rust_spec_for(&s, "rust-1.83", "rust:1.83", 18090);
    assert_eq!(nginx.name.as_deref(), Some(NGINX_CONTAINER));
    assert_eq!(php.name.as_deref(), Some("skiff-php-batch1"));
    assert_eq!(rust.name.as_deref(), Some("skiff-rust-rust-1.83"));
    for spec in [&nginx, &php, &rust] {
        assert_eq!(
            spec.labels
                .get(skiff_model::LABEL_PROTECTED)
                .map(String::as_str),
            Some("true"),
            "{} 缺 io.skiff.protected 标签",
            spec.name.as_deref().unwrap_or_default()
        );
        assert_eq!(
            spec.labels
                .get(skiff_model::LABEL_PROJECT)
                .map(String::as_str),
            Some("sites")
        );
    }
}

/// 删除容器前的 BusyGuard：运行环境有任务、Web 服务安装/重建中都应拒绝，
/// 空闲时放行（正常删除路径）。
#[tokio::test]
async fn container_busy_reason_matches_kernel_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    state
        .db()
        .create_runtime(&skiff_store::NewRuntime {
            id: "php-busy".into(),
            kind: "php".into(),
            name: "busy".into(),
            version: "8.5".into(),
            image: "php:8.5-fpm-alpine".into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();
    let container = runtime_container_name("php", "busy");
    assert!(
        state.container_busy_reason(&container).await.is_none(),
        "空闲运行环境应允许删除"
    );
    state
        .jobs()
        .submit(app_jobs::JobSubmit {
            kind: "runtime.install".into(),
            target_type: app_jobs::TARGET_RUNTIME.into(),
            target_id: "php-busy".into(),
            title: "安装运行环境 busy".into(),
            params: serde_json::json!({}),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: String::new(),
            cancellable: true,
        })
        .unwrap();
    assert!(state
        .container_busy_reason(&container)
        .await
        .unwrap()
        .contains("进行中的任务"));
    assert!(
        state.container_busy_reason("someone-else").await.is_none(),
        "无关容器不受运行环境任务影响"
    );
    assert!(state.container_busy_reason(NGINX_CONTAINER).await.is_none());
    state.site_env.lock().await.running = true;
    assert!(state
        .container_busy_reason(NGINX_CONTAINER)
        .await
        .unwrap()
        .contains("Web 服务"));
}

#[test]
fn vhost_check_main_isolates_candidate_from_live_include() {
    let text = vhost_check_main("/www/skiff/vhost-check/x.conf.candidate");
    assert!(text.contains("include /www/skiff/vhost-check/x.conf.candidate;"));
    // 绝不能把线上站点 include 拉进来，否则等于先写进线上再测。
    assert!(!text.contains("10-skiff-sites"));
    assert!(!text.contains("/www/sites/*/conf"));
}

#[test]
fn vhost_check_failure_reports_line_and_diff() {
    let original = "server {\n    listen 80;\n    server_name a.local;\n}\n";
    let bad = "server {\n    listen 80;\n    server_name a.local;\n    this_is_invalid_directive_xyz;\n}\n";
    let out = "nginx: [emerg] unknown directive \"this_is_invalid_directive_xyz\" in /www/skiff/vhost-check/a.local-1.conf.candidate:5";
    let msg = format!(
        "站点配置校验未通过：{}；差异：{}",
        crate::nginx::error_line(out),
        crate::nginx::diff_summary(original, bad, 6)
    );
    assert!(msg.contains("unknown directive"), "{msg}");
    assert!(msg.contains("第 5 行"), "{msg}");
}

#[tokio::test]
async fn stopped_site_save_does_not_persist_when_validation_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    let site_dir = s.sites_dir.join("audit-env.local");
    std::fs::create_dir_all(site_conf_dir(&site_dir)).unwrap();
    let disabled = s.vhost_disabled(&site_dir, "audit-env.local");
    let original = "server {\n    listen 80;\n    server_name audit-env.local;\n}\n";
    std::fs::write(&disabled, original).unwrap();
    state
        .db()
        .create_site(&StoreNewSite {
            domain: "audit-env.local".into(),
            root_dir: site_dir.display().to_string(),
            site_type: TYPE_STATIC.into(),
            runtime_id: None,
            proxy_target: None,
            proxy_websocket: false,
        })
        .unwrap();
    let bad = "server {\n    listen 80;\n    this_is_invalid_directive_xyz;\n}\n";
    let res = state.save_site_vhost("audit-env.local", bad).await;
    assert!(res.is_err(), "校验不可用/失败时必须返回错误");
    assert_eq!(
        std::fs::read_to_string(&disabled).unwrap(),
        original,
        "校验失败不得落盘"
    );
    assert!(!s.vhost_file(&site_dir, "audit-env.local").exists());
}

#[tokio::test]
async fn start_site_does_not_activate_when_validation_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    let site_dir = s.sites_dir.join("audit-env.local");
    std::fs::create_dir_all(site_conf_dir(&site_dir)).unwrap();
    let disabled = s.vhost_disabled(&site_dir, "audit-env.local");
    std::fs::write(
        &disabled,
        "server {\n    listen 80;\n    this_is_invalid_directive_xyz;\n}\n",
    )
    .unwrap();
    state
        .db()
        .create_site(&StoreNewSite {
            domain: "audit-env.local".into(),
            root_dir: site_dir.display().to_string(),
            site_type: TYPE_STATIC.into(),
            runtime_id: None,
            proxy_target: None,
            proxy_websocket: false,
        })
        .unwrap();
    state
        .db()
        .set_site_status("audit-env.local", "stopped")
        .unwrap();
    let res = state.set_site_running("audit-env.local", true).await;
    assert!(res.is_err(), "非法配置必须拒绝启动");
    assert!(disabled.exists(), "拒绝启动后仍应保留停用态文件");
    assert!(
        !s.vhost_file(&site_dir, "audit-env.local").exists(),
        "拒绝启动不得把配置推进线上 include"
    );
    let site = state
        .db()
        .site_by_domain("audit-env.local")
        .unwrap()
        .unwrap();
    assert_eq!(site.status, "stopped", "拒绝启动不得改写运行状态");
}

#[test]
fn runtime_site_start_guard_only_treats_php_as_socket_based() {
    let rt = |kind: &str, last_status: &str, socket: Option<&str>| Runtime {
        id: "probe".into(),
        kind: kind.into(),
        name: "probe".into(),
        version: "1.99".into(),
        image: String::new(),
        image_digest: String::new(),
        desired_state: "running".into(),
        last_status: last_status.into(),
        socket_path: socket.map(ToString::to_string),
        port: None,
        is_default: false,
        config_json: "{}".into(),
        extensions: String::new(),
        origin: "instance".into(),
        created_at: 0,
        updated_at: 0,
    };

    // Rust 运行环境没有 FPM socket，只要状态 running 就允许启动站点。
    ensure_runtime_ready_for_start(&rt(skiff_store::KIND_RUST, "running", None))
        .expect("运行中的 Rust 环境不得被 PHP socket 条件误拦");
    let stopped = ensure_runtime_ready_for_start(&rt(skiff_store::KIND_RUST, "stopped", None))
        .expect_err("停用的 Rust 环境必须拒绝启动");
    assert!(stopped.to_string().contains("未运行"), "{stopped}");

    // PHP 仍要求 socket 文件真实存在。
    let missing = ensure_runtime_ready_for_start(&rt(
        skiff_store::KIND_PHP,
        "running",
        Some("/nonexistent/php-fpm.sock"),
    ))
    .expect_err("socket 缺失必须拒绝启动");
    assert!(
        missing.to_string().contains("PHP socket 未就绪"),
        "{missing}"
    );

    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("php-fpm.sock");
    std::fs::write(&sock, b"").unwrap();
    ensure_runtime_ready_for_start(&rt(
        skiff_store::KIND_PHP,
        "running",
        Some(&sock.display().to_string()),
    ))
    .expect("socket 就绪的 PHP 环境应允许启动");
}

#[test]
fn site_vhost_routes_by_runtime_id_to_its_socket() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    // 同版本第二实例：socket 只从 name 推导，与内置实例分开。
    state
        .db()
        .create_runtime(&NewRuntime {
            id: "php-8.5-a".into(),
            kind: skiff_store::KIND_PHP.into(),
            name: "php-8.5-a".into(),
            version: "8.5".into(),
            image: "php:8.5-fpm-alpine".into(),
            image_digest: String::new(),
            socket_path: Some(s.runtime_socket("php", "php-8.5-a").display().to_string()),
            port: None,
            is_default: false,
        })
        .unwrap();

    let site = |domain: &str, runtime_id: &str| {
        state
            .db()
            .create_site(&StoreNewSite {
                domain: domain.into(),
                root_dir: s.sites_dir.join(domain).display().to_string(),
                site_type: TYPE_RUNTIME.into(),
                runtime_id: Some(runtime_id.into()),
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap()
    };

    let custom = site("a.local", "php-8.5-a");
    let vhost = state.render_vhost_for(&s, &custom, &[], None).unwrap();
    assert!(
        vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5-a/php-fpm.sock;"),
        "站点必须按 runtime_id 指向所属实例 socket: {vhost}"
    );
    assert!(
        !vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"),
        "不得回落到默认实例: {vhost}"
    );

    let default_site = site("b.local", "php-8.5");
    let vhost = state
        .render_vhost_for(&s, &default_site, &[], None)
        .unwrap();
    assert!(
        vhost.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"),
        "默认实例站点仍指向自己的 socket: {vhost}"
    );
}

#[test]
fn container_path_is_under_shared_web_root() {
    let s = settings_at(Path::new("/www"));
    assert_eq!(
        s.container_path(Path::new("/www/sites/example.com")),
        "/www/sites/example.com"
    );
    assert_eq!(s.container_sites_dir(), "/www/sites");
    assert_eq!(
        s.runtime_pool_conf("php", "php-8.5"),
        PathBuf::from("/www/runtime/php/php-8.5/www.conf")
    );
    assert_eq!(
        s.runtime_socket("php", "php-8.5"),
        PathBuf::from("/www/runtime/php/php-8.5/php-fpm.sock")
    );
    let site_dir = Path::new("/www/sites/example.com");
    assert_eq!(
        s.container_log(site_dir, LogKind::Error),
        "/www/sites/example.com/log/error.log"
    );
    assert_eq!(
        s.container_path(&site_index_dir(site_dir)),
        "/www/sites/example.com/index"
    );
    assert_eq!(
        s.cert_file(site_dir),
        PathBuf::from("/www/sites/example.com/ssl/cert.pem")
    );
}

#[test]
fn php_pool_only_uses_unix_socket() {
    let pool = render_php_pool();
    assert!(pool.contains("listen = /www/runtime/php/php-8.5/php-fpm.sock"));
    assert!(pool.contains("listen.group = nobody"));
    assert!(pool.contains("listen.mode = 0660"));
    assert!(!pool.contains("0666"), "socket 不得 world 可写");
    assert!(!pool.contains("9000"));
}

#[test]
fn runtime_paths_and_container_names_derive_from_name_only() {
    let s = settings_at(Path::new("/www"));
    assert_eq!(
        runtime_container_name("php", "php-8.5"),
        "skiff-php-php-8.5"
    );
    assert_eq!(
        runtime_container_name("php", "php-8.5-a"),
        "skiff-php-php-8.5-a"
    );
    assert_eq!(
        runtime_container_name("node", "php-8.5"),
        "skiff-node-php-8.5",
        "跨 kind 同名容器天然分开"
    );
    assert_eq!(
        runtime_container_socket("php", "php-8.5-a"),
        "/www/runtime/php/php-8.5-a/php-fpm.sock"
    );
    assert_eq!(
        s.runtime_socket("php", "php-8.5-a"),
        PathBuf::from("/www/runtime/php/php-8.5-a/php-fpm.sock")
    );
    assert_eq!(
        s.runtime_home("php", "php-8.5-a"),
        PathBuf::from("/www/runtime/php/php-8.5-a")
    );
    assert_eq!(
        s.runtime_log_dir("php", "php-8.5-a"),
        PathBuf::from("/www/runtime/php/php-8.5-a/log")
    );
    // 同版本不同实例的 socket / 容器名必须不同，不能按版本号共用。
    assert_ne!(
        s.runtime_socket("php", "php-8.5"),
        s.runtime_socket("php", "php-8.5-a")
    );
    assert_ne!(
        runtime_container_name("php", "php-8.5"),
        runtime_container_name("php", "php-8.5-a")
    );
}

#[test]
fn install_locks_are_mutually_exclusive_per_kind_name() {
    let locks = InstallLocks::default();
    let a = locks.try_begin("php:php-8.5-a").expect("首个安装应拿到锁");
    assert!(
        locks.try_begin("php:php-8.5-a").is_none(),
        "同一实例并发安装必须被拒绝"
    );
    assert!(
        locks.try_begin("php:php-8.5-b").is_some(),
        "不同实例可并行安装"
    );
    assert!(
        locks.try_begin("node:php-8.5-a").is_some(),
        "跨 kind 同名是不同实例"
    );
    drop(a);
    assert!(
        locks.try_begin("php:php-8.5-a").is_some(),
        "安装结束后锁应释放"
    );
}

#[test]
fn runtime_job_projection_follows_job_center_status() {
    let svc = app_jobs::JobService::new(std::sync::Arc::new(
        skiff_store::Db::open_in_memory().unwrap(),
    ));
    let job = svc
        .submit(app_jobs::JobSubmit {
            kind: "runtime.install".into(),
            target_type: app_jobs::TARGET_RUNTIME.into(),
            target_id: "php-8.5".into(),
            title: "安装运行环境 php-8.5".into(),
            params: serde_json::json!({"runtime_id": "php-8.5"}),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: String::new(),
            cancellable: true,
        })
        .unwrap();
    let queued = RuntimeJob::from_job(&svc.get(&job.id).unwrap(), Vec::new(), 0);
    assert_eq!(queued.state, RuntimeJobState::Queued);
    assert!(queued.active());
    assert_eq!(queued.kind, "install");

    svc.set_phase(&job.id, "拉取镜像", "拉取镜像", 15, true);
    svc.note(&job.id, "pull php:8.5-fpm");
    let running = RuntimeJob::from_job(&svc.get(&job.id).unwrap(), vec!["pull".into()], 0);
    assert_eq!(running.state, RuntimeJobState::Running);
    assert_eq!(running.step, "拉取镜像");
    assert_eq!(running.progress, 15);

    svc.finish_err(&job.id, "failed", "Docker 不可用");
    let failed = RuntimeJob::from_job(
        &svc.get(&job.id).unwrap(),
        vec!["[00:00:00] 失败：Docker 不可用".into()],
        0,
    );
    assert_eq!(failed.state, RuntimeJobState::Failed);
    assert!(!failed.active());
    assert_eq!(failed.error, "Docker 不可用");
    assert!(failed.log_text().contains("Docker 不可用"));
}

#[test]
fn socket_mode_is_group_only() {
    assert_eq!(SOCKET_MODE, 0o660, "socket 必须是 0660");
    assert_eq!(SOCKET_MODE & 0o007, 0, "others 不得有任何权限");
    assert_eq!(SOCKET_GID, 65534, "固定属组 = 两镜像都有的 nobody");
}

#[test]
fn image_is_pinned_to_digest() {
    assert!(DEFAULT_NGINX_IMAGE.starts_with("openresty/openresty:alpine@sha256:"));
    assert!(!DEFAULT_NGINX_IMAGE.contains("1panel"));
}

#[test]
fn vhost_with_tls_adds_443_ssl_server() {
    let tls = VhostTls {
        cert: "/www/sites/example.com/ssl/cert.pem".into(),
        key: "/www/sites/example.com/ssl/key.pem".into(),
        force_https: false,
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
    assert!(v.contains("listen 80;"));
    assert!(v.contains("listen 443 ssl;"));
    assert!(v.contains("ssl_certificate /www/sites/example.com/ssl/cert.pem;"));
    assert!(v.contains("ssl_certificate_key /www/sites/example.com/ssl/key.pem;"));
    assert!(!v.contains("return 301"));
}

#[test]
fn force_https_redirects_80_to_443() {
    let tls = VhostTls {
        cert: "/www/sites/example.com/ssl/cert.pem".into(),
        key: "/www/sites/example.com/ssl/key.pem".into(),
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
    assert!(v.contains("return 301 https://$host$request_uri;"));
    assert!(v.contains("listen 443 ssl;"));
}

#[test]
fn default_vhost_with_tls_serves_443_default_server() {
    let tls = VhostTls {
        cert: "/www/sites/demo.local/ssl/cert.pem".into(),
        key: "/www/sites/demo.local/ssl/key.pem".into(),
        force_https: false,
    };
    let v = render_default_vhost(
        "demo.local",
        VhostKind::Php {
            root: "/www/sites/demo.local/index",
            socket: "/www/runtime/php/php-8.5/php-fpm.sock",
        },
        "/www/sites/demo.local/log/access.log",
        "/www/sites/demo.local/log/error.log",
        Some(&tls),
    );
    assert!(v.contains("listen 443 ssl default_server;"));
    assert!(v.contains("ssl_certificate /www/sites/demo.local/ssl/cert.pem;"));
}

#[test]
fn default_vhost_serves_index_and_fallback_is_404() {
    let v = render_default_vhost(
        "demo.local",
        VhostKind::Php {
            root: "/www/sites/demo.local/index",
            socket: "/www/runtime/php/php-8.5/php-fpm.sock",
        },
        "/www/sites/demo.local/log/access.log",
        "/www/sites/demo.local/log/error.log",
        None,
    );
    assert!(v.contains("listen 80 default_server;"));
    assert!(v.contains("server_name _;"));
    assert!(v.contains("root /www/sites/demo.local/index;"));
    assert!(v.contains("fastcgi_pass unix:/www/runtime/php/php-8.5/php-fpm.sock;"));

    // 兜底文件不能声明 default_server，否则与默认站点冲突；靠文件名排在站点
    // vhost 之前接管裸访问并返回 404。
    assert!(FALLBACK_VHOST.contains("return 404;"));
    assert!(!FALLBACK_VHOST.contains("default_server"));
    // 不能再用 `_`：默认站也用 `_`，nginx 会报重复 server_name 警告。
    assert!(FALLBACK_VHOST.contains("server_name skiff-fallback;"));
    assert!(!FALLBACK_VHOST.contains("server_name _;"));
}

#[test]
fn log_reader_is_reverse_and_filtered() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("demo.access.log");
    std::fs::write(&p, "1 a\n2 b\n3 target\n4 b\n").unwrap();
    assert_eq!(read_log(&p, 2, "").unwrap(), "4 b\n3 target");
    assert_eq!(read_log(&p, 10, "target").unwrap(), "3 target");
    assert!(read_log(&p, 10, "missing").unwrap().is_empty());
}

#[test]
fn log_reader_only_reads_tail_window() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.access.log");
    let line = format!("{}\n", "x".repeat(99));
    let mut text = String::from("EARLY-MARKER\n");
    while text.len() < (LOG_READ_MAX_BYTES as usize) + 4096 {
        text.push_str(&line);
    }
    text.push_str("LATE-MARKER\n");
    std::fs::write(&p, &text).unwrap();
    let out = read_log(&p, 5, "").unwrap();
    assert!(out.contains("LATE-MARKER"), "尾部窗口应包含最新行");
    assert!(!out.contains("EARLY-MARKER"), "窗口之外的内容不应被读入");
    assert!(out.lines().count() <= 5);
    // 关键字过滤同样只作用于尾部窗口，不回头扫描整个文件。
    assert!(read_log(&p, 5, "early-marker").unwrap().is_empty());
}

#[test]
fn log_rotation_renames_and_recreates() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("demo.access.log");
    std::fs::write(&p, "x".repeat(64)).unwrap();
    assert!(rotate_if_large(&p, 32).unwrap());
    assert_eq!(std::fs::read_to_string(&p).unwrap().len(), 0);
    let rotated = dir.path().join("demo.access.log.1");
    assert_eq!(std::fs::read_to_string(rotated).unwrap().len(), 64);
    assert!(!rotate_if_large(&p, 32).unwrap(), "空文件不再轮转");
}

#[test]
fn rotate_logs_sweeps_only_large_log_files() {
    let dir = tempfile::tempdir().unwrap();
    let big = dir.path().join("a.access.log");
    let small = dir.path().join("b.error.log");
    let other = dir.path().join("c.txt");
    std::fs::write(&big, "x".repeat(64)).unwrap();
    std::fs::write(&small, "y".repeat(8)).unwrap();
    std::fs::write(&other, "z".repeat(64)).unwrap();
    assert_eq!(rotate_logs(dir.path(), 32).unwrap(), 1);
    assert_eq!(std::fs::read_to_string(&big).unwrap().len(), 0);
    assert!(big.with_extension("log.1").exists());
    assert_eq!(std::fs::read_to_string(&small).unwrap().len(), 8);
    assert_eq!(std::fs::read_to_string(&other).unwrap().len(), 64);
}

#[test]
fn occupied_port_is_reported_with_process_hint() {
    let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let err = check_port_free(port).expect_err("已监听端口应报占用");
    assert!(err.contains(&port.to_string()), "错误信息应包含端口: {err}");
    drop(listener);
    check_port_free(port).expect("释放后端口应可用");
    assert!(ensure_ports_free(&[port]).is_ok());
}

fn container_row(name: &str, state: &str) -> ContainerRow {
    ContainerRow {
        id: "id".into(),
        name: name.into(),
        image: "img".into(),
        image_id: "sha256:deadbeef".into(),
        state: state.into(),
        status: String::new(),
        labels: BTreeMap::new(),
        project: None,
        owner: None,
        protected: false,
        unprotected_kernel: false,
    }
}

#[test]
fn stopped_nginx_does_not_suppress_port_probe() {
    assert!(nginx_container_running(&[container_row(
        NGINX_CONTAINER,
        "running"
    )]));
    assert!(!nginx_container_running(&[container_row(
        NGINX_CONTAINER,
        "exited"
    )]));
    assert!(!nginx_container_running(&[container_row(
        NGINX_CONTAINER,
        "restarting"
    )]));
    assert!(!nginx_container_running(&[container_row(
        "other", "running"
    )]));
}

#[test]
fn rust_runtime_uses_non_login_shell() {
    // 回归：`sh -lc` 会重置 PATH，cargo 不在默认 PATH 里，容器退出码 127。
    // 源码目录固定为环境 `app/`，由 entry.sh 直接进入。
    let cmd = rust_run_command(
        "/www/runtime/rust/x/entry.sh",
        "/www/runtime/rust/x/log/server.log",
    );
    assert_eq!(cmd[0], "sh");
    assert_eq!(cmd[1], "-c");
    assert!(cmd[2].starts_with("sh /www/runtime/rust/x/entry.sh"));
    assert!(cmd[2].contains(">> /www/runtime/rust/x/log/server.log 2>&1"));
}

#[test]
fn registry_is_reported_for_pull_failures() {
    assert_eq!(registry_of("nginx:alpine"), "docker.io");
    assert_eq!(registry_of("openresty/openresty:alpine"), "docker.io");
    assert_eq!(registry_of("php:8.5-fpm-alpine"), "docker.io");
    assert_eq!(
        registry_of("registry.example.com/nginx:alpine"),
        "registry.example.com"
    );
    assert_eq!(registry_of("localhost:5000/nginx"), "localhost:5000");
}

#[test]
fn site_layout_has_one_directory_per_site() {
    let s = settings_at(Path::new("/www"));
    let site_dir = Path::new("/www/sites/example.com");
    assert_eq!(
        site_index_dir(site_dir),
        PathBuf::from("/www/sites/example.com/index")
    );
    assert_eq!(
        site_log_dir(site_dir),
        PathBuf::from("/www/sites/example.com/log")
    );
    assert_eq!(
        site_ssl_dir(site_dir),
        PathBuf::from("/www/sites/example.com/ssl")
    );
    assert_eq!(
        site_conf_dir(site_dir),
        PathBuf::from("/www/sites/example.com/conf")
    );
    assert_eq!(
        s.log_file(site_dir, LogKind::Access),
        PathBuf::from("/www/sites/example.com/log/access.log")
    );
    assert_eq!(
        s.log_file(site_dir, LogKind::Error),
        PathBuf::from("/www/sites/example.com/log/error.log")
    );
    assert_eq!(
        s.vhost_file(site_dir, "example.com"),
        PathBuf::from("/www/sites/example.com/conf/example.com.conf")
    );
    assert_eq!(
        s.vhost_disabled(site_dir, "example.com"),
        PathBuf::from("/www/sites/example.com/conf/example.com.conf.disabled")
    );
}

#[test]
fn sites_include_entry_globs_per_site_conf() {
    let s = settings_at(Path::new("/www"));
    let text = s.sites_include_conf();
    assert!(text.contains("include /www/sites/*/conf/*.conf;"));
    assert!(text.contains("备份"));
    // 默认站/兜底站仍在内核公共目录，不参与该通配。
    assert!(!text.contains(DEFAULT_CONF));
    assert!(!text.contains(FALLBACK_CONF));
    assert_eq!(SITES_INCLUDE_CONF, "10-skiff-sites.conf");
}

#[test]
fn rotate_site_logs_scans_each_site_log_dir() {
    let dir = tempfile::tempdir().unwrap();
    let sites = dir.path().join("sites");
    let a_log = sites.join("a.example").join("log");
    let b_log = sites.join("b.example").join("log");
    std::fs::create_dir_all(&a_log).unwrap();
    std::fs::create_dir_all(&b_log).unwrap();
    std::fs::write(a_log.join("access.log"), "x".repeat(64)).unwrap();
    std::fs::write(b_log.join("error.log"), "y".repeat(64)).unwrap();
    std::fs::write(sites.join("readme.txt"), "z".repeat(64)).unwrap();
    assert_eq!(rotate_site_logs(&sites, 32).unwrap(), 2);
}

#[test]
fn image_version_cache_positive_has_no_ttl_and_negative_expires() {
    let mut cache: HashMap<String, (Option<String>, std::time::Instant)> = HashMap::new();
    let t0 = std::time::Instant::now();
    // 正缓存：image_id 不可变，过了很久仍然命中。
    cache.insert("sha256:abc".into(), (Some("8.5.12".into()), t0));
    match image_version_lookup(
        &cache,
        "sha256:abc",
        t0 + std::time::Duration::from_secs(3600),
    ) {
        ImageVersionHit::Known(v) => assert_eq!(v, "8.5.12"),
        _ => panic!("正缓存不应过期"),
    }
    // 负缓存：60 秒内抑制重复 inspect。
    cache.insert("sha256:def".into(), (None, t0));
    assert!(matches!(
        image_version_lookup(
            &cache,
            "sha256:def",
            t0 + std::time::Duration::from_secs(59)
        ),
        ImageVersionHit::Suppressed
    ));
    assert!(matches!(
        image_version_lookup(
            &cache,
            "sha256:def",
            t0 + std::time::Duration::from_secs(61)
        ),
        ImageVersionHit::Miss
    ));
    // 未登记 image_id：未命中。
    assert!(matches!(
        image_version_lookup(&cache, "sha256:ghi", t0),
        ImageVersionHit::Miss
    ));
}

#[test]
fn image_id_change_does_not_reuse_previous_version() {
    let mut cache: HashMap<String, (Option<String>, std::time::Instant)> = HashMap::new();
    let t0 = std::time::Instant::now();
    cache.insert("sha256:old".into(), (Some("8.5.11".into()), t0));
    // 重建后 image_id 变化：新 key 未命中，必须重新 inspect，不会沿用旧版本。
    assert!(matches!(
        image_version_lookup(&cache, "sha256:new", t0),
        ImageVersionHit::Miss
    ));
}

#[test]
fn digest_normalizes_repo_digest_and_builds_pinned_ref() {
    let repo = "php@sha256:abc123";
    assert_eq!(digest_sha256(repo), "sha256:abc123");
    assert_eq!(digest_sha256("sha256:abc123"), "sha256:abc123");
    assert_eq!(digest_sha256(""), "");
    assert_eq!(
        pinned_image_ref("php:8.5-fpm-alpine", repo),
        "php:8.5-fpm-alpine@sha256:abc123"
    );
    // 已是固定引用或没有 digest 时保持原样。
    assert_eq!(
        pinned_image_ref("php:8.5-fpm-alpine@sha256:abc123", repo),
        "php:8.5-fpm-alpine@sha256:abc123"
    );
    assert_eq!(
        pinned_image_ref("php:8.5-fpm-alpine", ""),
        "php:8.5-fpm-alpine"
    );
}

#[tokio::test]
async fn deleting_runtime_removes_directory_and_row() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    state
        .db()
        .ensure_runtime(&NewRuntime {
            id: "php-custom".into(),
            kind: skiff_store::KIND_PHP.into(),
            name: "php-custom".into(),
            version: "8.5".into(),
            image: DEFAULT_PHP_IMAGE.into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();
    let home = s.runtime_home(skiff_store::KIND_PHP, "php-custom");
    std::fs::create_dir_all(s.runtime_log_dir(skiff_store::KIND_PHP, "php-custom")).unwrap();
    std::fs::write(
        s.runtime_pool_conf(skiff_store::KIND_PHP, "php-custom"),
        "listen = /www/runtime/php/php-custom/php-fpm.sock\n",
    )
    .unwrap();

    let report = state.delete_site_runtime("php-custom").await.unwrap();
    assert!(!home.exists(), "运行环境目录（含 FPM 池配置）必须删除");
    assert!(
        state.db().runtime_by_id("php-custom").unwrap().is_none(),
        "DB 行必须删除"
    );
    assert!(
        report.skipped.iter().any(|s| s.contains("Docker 不可用")),
        "无 Docker 时必须说明容器未清理：{:?}",
        report.skipped
    );
    assert!(
        report.completed.iter().any(|s| s.contains("目录")),
        "报告必须记录已删除目录：{:?}",
        report.completed
    );
}

#[tokio::test]
async fn deleting_runtime_with_bound_site_is_rejected_and_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    state
        .db()
        .ensure_runtime(&NewRuntime {
            id: "php-bound".into(),
            kind: skiff_store::KIND_PHP.into(),
            name: "php-bound".into(),
            version: "8.5".into(),
            image: DEFAULT_PHP_IMAGE.into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();
    let home = s.runtime_home(skiff_store::KIND_PHP, "php-bound");
    std::fs::create_dir_all(&home).unwrap();
    let site_dir = s.sites_dir.join("bound.local");
    state
        .db()
        .create_site(&StoreNewSite {
            domain: "bound.local".into(),
            root_dir: site_dir.display().to_string(),
            site_type: TYPE_RUNTIME.into(),
            runtime_id: Some("php-bound".into()),
            proxy_target: None,
            proxy_websocket: false,
        })
        .unwrap();

    let err = state.delete_site_runtime("php-bound").await.unwrap_err();
    let text = format!("{err}");
    assert!(matches!(err, SiteError::Conflict(_)), "{text}");
    assert!(text.contains("bound.local"), "{text}");
    assert!(home.exists(), "有绑定时不得删除目录");
    assert!(
        state.db().runtime_by_id("php-bound").unwrap().is_some(),
        "有绑定时不得删除记录"
    );
}

/// 默认站点切换的失败路径：reload 失败（本机无 Docker）时必须把库和 conf
/// 一起回滚到切换前，不能留下「conf 已切、库没变」。
#[tokio::test]
async fn set_default_site_rolls_back_db_and_conf_when_reload_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state_at(dir.path());
    let s = state.site_settings();
    for domain in ["a.local", "b.local"] {
        let site_dir = s.sites_dir.join(domain);
        std::fs::create_dir_all(site_index_dir(&site_dir)).unwrap();
        state
            .db()
            .create_site(&StoreNewSite {
                domain: domain.into(),
                root_dir: site_dir.display().to_string(),
                site_type: TYPE_STATIC.into(),
                runtime_id: None,
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
    }
    state.db().set_site_default("a.local", true).unwrap();
    std::fs::create_dir_all(&s.conf_dir).unwrap();
    let conf = s.conf_dir.join("00-skiff-default.conf");
    std::fs::write(&conf, "# 切换前的旧投影\n").unwrap();

    let err = state.set_default_site("b.local", true).await.unwrap_err();
    assert!(matches!(err, SiteError::Unavailable(_)), "{err}");

    assert_eq!(
        state.db().default_site().unwrap().unwrap().domain,
        "a.local",
        "reload 失败后库必须回到切换前的默认站点"
    );
    let text = std::fs::read_to_string(&conf).unwrap();
    assert!(
        text.contains("a.local"),
        "conf 必须按回滚后的默认站点重写：{text}"
    );
    assert!(!text.contains("b.local"), "不得残留失败方向的投影：{text}");
}

/// nginx 激活策略的内存替身：复现「reload 不改 interrupt 服务、restart 有窗口」。
mod nginx_activation {
    use super::*;
    use crate::sites::crud::{
        activate_with, is_nginx_comm, parse_worker_probe, replace_cert_pair, CertPairError,
        NginxActivation, NginxControl,
    };
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    struct FakeNginx {
        running: AtomicBool,
        generation: AtomicU64,
        reloads: AtomicUsize,
        restarts: AtomicUsize,
        reload_fails: AtomicBool,
        reload_no_effect: AtomicBool,
        restart_fails: AtomicBool,
    }

    impl FakeNginx {
        fn online() -> Self {
            Self {
                running: AtomicBool::new(true),
                generation: AtomicU64::new(1),
                reloads: AtomicUsize::new(0),
                restarts: AtomicUsize::new(0),
                reload_fails: AtomicBool::new(false),
                reload_no_effect: AtomicBool::new(false),
                restart_fails: AtomicBool::new(false),
            }
        }
    }

    impl NginxControl for FakeNginx {
        async fn container_running(&self) -> Result<bool, String> {
            Ok(self.running.load(Ordering::SeqCst))
        }

        async fn worker_generation(&self) -> Result<String, String> {
            Ok(format!("w{}", self.generation.load(Ordering::SeqCst)))
        }

        async fn reload(&self) -> Result<(), String> {
            self.reloads.fetch_add(1, Ordering::SeqCst);
            if self.reload_fails.load(Ordering::SeqCst) {
                return Err("bad config".into());
            }
            if !self.reload_no_effect.load(Ordering::SeqCst) {
                self.generation.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        async fn restart(&self) -> Result<(), String> {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            if self.restart_fails.load(Ordering::SeqCst) {
                return Err("restart failed".into());
            }
            self.generation.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// 在线容器：reload 优先，worker 换代即确认，不重启。
    #[tokio::test]
    async fn reload_is_preferred_when_workers_turn_over() {
        let n = FakeNginx::online();
        let got = activate_with(&n).await.expect("reload 应成功");
        assert_eq!(got, NginxActivation::Reloaded);
        assert_eq!(n.reloads.load(Ordering::SeqCst), 1);
        assert_eq!(n.restarts.load(Ordering::SeqCst), 0, "健康配置不应重启容器");
    }

    /// reload 返回成功但 worker 没换代：master 没采纳新配置，必须回退重启。
    #[tokio::test]
    async fn reload_without_worker_turnover_falls_back_to_restart() {
        let n = FakeNginx::online();
        n.reload_no_effect.store(true, Ordering::SeqCst);
        let got = activate_with(&n).await.expect("应回退重启成功");
        assert_eq!(got, NginxActivation::Restarted);
        assert_eq!(n.restarts.load(Ordering::SeqCst), 1);
    }

    /// 容器未运行：不尝试 reload，直接重启。
    #[tokio::test]
    async fn stopped_container_goes_straight_to_restart() {
        let n = FakeNginx::online();
        n.running.store(false, Ordering::SeqCst);
        let got = activate_with(&n).await.expect("重启应成功");
        assert_eq!(got, NginxActivation::Restarted);
        assert_eq!(n.reloads.load(Ordering::SeqCst), 0);
    }

    /// 并发保存同一站点：reload 路径没有重启窗口，回滚次数应从「每次重启都可能
    /// 撞窗口」降到 0；真正加载不了的坏配置仍会失败并触发回滚。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_saves_do_not_collide_in_restart_windows() {
        let n = Arc::new(FakeNginx::online());
        let mut tasks = Vec::new();
        for _ in 0..48 {
            let n = n.clone();
            tasks.push(tokio::spawn(async move { activate_with(&*n).await }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap().unwrap(), NginxActivation::Reloaded);
        }
        assert_eq!(
            n.restarts.load(Ordering::SeqCst),
            0,
            "并发保存健康配置时不应触发任何容器重启（旧实现 254 次里 29 次撞上重启窗口）"
        );

        let bad = FakeNginx::online();
        bad.reload_fails.store(true, Ordering::SeqCst);
        bad.restart_fails.store(true, Ordering::SeqCst);
        assert!(activate_with(&bad).await.is_err(), "坏配置必须失败");
        assert_eq!(
            bad.restarts.load(Ordering::SeqCst),
            1,
            "reload 失败后必须尝试重启兜底，仍失败才回滚"
        );
    }

    /// 替换失败但原本有成对证书：两个文件都回到替换前内容。
    /// 旧文件与未改动的 vhost 一起构成原来那套可用状态（`nginx -t` 的加载前提不变）。
    #[test]
    fn cert_pair_failure_restores_previous_pair() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        std::fs::write(&cert, "old-cert").unwrap();
        std::fs::write(&key, "old-key").unwrap();
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let missing_key = crate::atomic::sibling_temp(&key);
        std::fs::write(&tmp_cert, "new-cert").unwrap();

        let failure = replace_cert_pair(&tmp_cert, &missing_key, &cert, &key).unwrap_err();
        assert!(
            matches!(failure, CertPairError::Restored(_)),
            "原本有成对文件时必须回滚成旧内容"
        );
        assert_eq!(std::fs::read_to_string(&cert).unwrap(), "old-cert");
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "old-key");
        assert!(!tmp_cert.exists(), "临时证书必须清理");
        assert!(!missing_key.exists());
    }

    /// 替换前没有证书（首次上传）失败：清理成「无证书」，不留半套文件。
    #[test]
    fn cert_pair_failure_without_previous_pair_clears_both() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let missing_key = crate::atomic::sibling_temp(&key);
        std::fs::write(&tmp_cert, "new-cert").unwrap();

        let failure = replace_cert_pair(&tmp_cert, &missing_key, &cert, &key).unwrap_err();
        assert!(
            failure.was_cleared(),
            "首次上传失败必须回到「无证书」，调用方据此同步 vhost/库"
        );
        assert!(!cert.exists(), "已换上的新证书必须删掉，不能留下悬空引用");
        assert!(!key.exists());
        assert!(!tmp_cert.exists());
    }

    /// 成功替换后不留备份或临时文件（备份在内存里，随调用结束丢弃）。
    #[test]
    fn cert_pair_success_leaves_no_backup_or_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("cert.pem");
        let key = dir.path().join("key.pem");
        std::fs::write(&cert, "old-cert").unwrap();
        std::fs::write(&key, "old-key").unwrap();
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        let tmp_key = crate::atomic::sibling_temp(&key);
        std::fs::write(&tmp_cert, "new-cert").unwrap();
        std::fs::write(&tmp_key, "new-key").unwrap();

        replace_cert_pair(&tmp_cert, &tmp_key, &cert, &key).unwrap();

        assert_eq!(std::fs::read_to_string(&cert).unwrap(), "new-cert");
        assert_eq!(std::fs::read_to_string(&key).unwrap(), "new-key");
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["cert.pem", "key.pem"], "不能残留备份/临时文件");
    }

    /// 首次上传失败后，vhost 与库必须同步回到「无证书」，
    /// 不能出现「库 https=1、vhost 声明证书、文件却不存在」的悬空引用。
    #[tokio::test]
    async fn failed_first_upload_repairs_dangling_ssl_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = app_state_at(dir.path());
        let s = state.site_settings();
        let domain = "t.local";
        let site_dir = s.sites_dir.join(domain);
        std::fs::create_dir_all(site_index_dir(&site_dir)).unwrap();
        state
            .db()
            .create_site(&StoreNewSite {
                domain: domain.into(),
                root_dir: site_dir.display().to_string(),
                site_type: TYPE_STATIC.into(),
                runtime_id: None,
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
        // 模拟替换前已有的悬空状态：库里 https=1、vhost 引用证书，但磁盘没有证书文件。
        state.db().set_site_tls(domain, true, false).unwrap();
        let site = state.db().site_by_domain(domain).unwrap().unwrap();
        let vhost = if site.status == "running" {
            s.vhost_file(&site_dir, domain)
        } else {
            s.vhost_disabled(&site_dir, domain)
        };
        std::fs::create_dir_all(vhost.parent().unwrap()).unwrap();
        std::fs::write(
            &vhost,
            "server {\n    ssl_certificate /www/t.local/ssl/cert.pem;\n}\n",
        )
        .unwrap();

        let cert = s.cert_file(&site_dir);
        let key = s.key_file(&site_dir);
        std::fs::create_dir_all(cert.parent().unwrap()).unwrap();
        let tmp_cert = crate::atomic::sibling_temp(&cert);
        std::fs::write(&tmp_cert, "new-cert").unwrap();
        let missing_key = crate::atomic::sibling_temp(&key);
        let failure = replace_cert_pair(&tmp_cert, &missing_key, &cert, &key).unwrap_err();
        assert!(failure.was_cleared());

        state.repair_no_cert_state(domain).await;

        assert!(
            !state.db().site_by_domain(domain).unwrap().unwrap().https,
            "库里的 HTTPS 标记必须回到关闭"
        );
        let text = std::fs::read_to_string(&vhost).unwrap();
        assert!(
            !text.contains("ssl_certificate"),
            "vhost 不能继续引用不存在的证书:\n{text}"
        );
        assert!(!cert.exists() && !key.exists());
    }

    /// `children` 直接给出 worker：master（PID 1）排除，重复项折叠。
    #[test]
    fn worker_probe_reads_children_and_excludes_master() {
        assert_eq!(parse_worker_probe("C 12\nC 1\nC 13\nC 12\n"), vec![12, 13]);
    }

    /// `children` 不可读/为空：退化扫描必须同时认 `nginx` 与 OpenResty 的 `openresty`。
    #[test]
    fn worker_probe_fallback_accepts_nginx_and_openresty() {
        let out = "S 1 openresty\nS 20 nginx\nS 21 openresty\nS 22 sh\nS 23 cat\n";
        assert_eq!(parse_worker_probe(out), vec![20, 21]);
    }

    /// 混合输出：两种来源都解析，结果排序去重。
    #[test]
    fn worker_probe_mixed_output_keeps_all_workers() {
        assert_eq!(parse_worker_probe("C 8\nC 7\nS 5 nginx\n"), vec![5, 7, 8]);
    }

    /// 空输出与无法解析的行不产生 worker（调用方据此回退重启）。
    #[test]
    fn worker_probe_empty_or_unparsable_is_empty() {
        assert!(parse_worker_probe("").is_empty());
        assert!(parse_worker_probe("garbage\nC not-a-pid\nS 9\n").is_empty());
        assert!(
            parse_worker_probe("S 1 nginx\n").is_empty(),
            "master 不是 worker"
        );
    }

    /// 进程名判定只认 nginx 家族，不能把前缀相同的进程（如 `openresty-worker`）算进来。
    #[test]
    fn comm_matches_only_nginx_family() {
        assert!(is_nginx_comm("nginx"));
        assert!(is_nginx_comm("openresty\n"));
        assert!(!is_nginx_comm(""));
        assert!(!is_nginx_comm("sh"));
        assert!(!is_nginx_comm("openresty-worker"));
    }

    /// 用真实解析器 + OpenResty 的 `comm` 驱动激活流程：修复前探测恒空，
    /// 逻辑上永远进不了 reload 分支；现在必须断言返回 [`NginxActivation::Reloaded`]。
    struct ParsedNginx {
        reloaded: AtomicBool,
        reloads: AtomicUsize,
        restarts: AtomicUsize,
    }

    impl NginxControl for ParsedNginx {
        async fn container_running(&self) -> Result<bool, String> {
            Ok(true)
        }

        async fn worker_generation(&self) -> Result<String, String> {
            let raw = if self.reloaded.load(Ordering::SeqCst) {
                "S 200 openresty\nS 201 openresty\n"
            } else {
                "S 100 openresty\nS 101 openresty\n"
            };
            let pids = parse_worker_probe(raw);
            assert!(!pids.is_empty(), "OpenResty 的 comm 必须被识别为 worker");
            Ok(pids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" "))
        }

        async fn reload(&self) -> Result<(), String> {
            self.reloads.fetch_add(1, Ordering::SeqCst);
            self.reloaded.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn restart(&self) -> Result<(), String> {
            self.restarts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn openresty_comm_reaches_reload_branch() {
        let n = ParsedNginx {
            reloaded: AtomicBool::new(false),
            reloads: AtomicUsize::new(0),
            restarts: AtomicUsize::new(0),
        };
        let got = activate_with(&n).await.expect("reload 应成功");
        assert_eq!(
            got,
            NginxActivation::Reloaded,
            "OpenResty worker 必须走 reload 分支，不能落到 restart"
        );
        assert_eq!(n.reloads.load(Ordering::SeqCst), 1);
        assert_eq!(n.restarts.load(Ordering::SeqCst), 0);
    }
}
