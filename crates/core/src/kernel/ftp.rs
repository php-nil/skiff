//! `ftp` 能力面：宿主机 pure-ftpd 的安装、站点账号与 uid/gid 对齐。
//!
//! 这是**内核新增原语**，不是插件能自行实现的能力：
//! - 现有 `fs` 只在授权根内读写，不能装宿主软件包；
//! - 现有 `docker` 白名单不开放 `containers.create` / `exec`，且 FTP 不走容器；
//! - 现有 `scheduler` 只调度任务，不执行宿主命令。
//!
//! 因此这里集中实现四个特权动作，并对插件只暴露固定动词：
//! 安装/卸载服务、站点列表、账号增删改、服务重启。命令一律用
//! `Command::new` 参数数组执行，禁止拼 shell；密码只经 stdin 交给 `pure-pw`，
//! 不入 argv、日志或审计。
//!
//! 边界：站点账号的 chroot 根固定为 `<site_root>/index`，内核只认数据库里的
//! 站点与 `/www/sites` 下的真实目录；FTP 账号看不到 `conf/`、`log/`、`ssl/`，
//! 也到不了别的站点。

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::AppState;
use skiff_model::Form;
use skiff_plugin::capability as cap;
#[cfg(target_os = "linux")]
use skiff_store::{Site, TYPE_PROXY, TYPE_RUNTIME, TYPE_STATIC};

const PASSWD_FILE: &str = "/etc/pure-ftpd/pureftpd.passwd";
const PDB_FILE: &str = "/etc/pure-ftpd/pureftpd.pdb";
const CONF_DIR: &str = "/etc/pure-ftpd/conf";
const DEFAULT_FILE: &str = "/etc/default/pure-ftpd-common";
const TLS_PEM: &str = "/etc/ssl/private/pure-ftpd.pem";
const JAIL_FILE: &str = "/etc/fail2ban/jail.d/skiff-pureftpd.conf";
const SERVICE: &str = "pure-ftpd";
const LOGIN_PREFIX: &str = "skiff_";
const DEFAULT_PORT_START: u16 = 30000;
const DEFAULT_PORT_END: u16 = 30100;

#[derive(Debug)]
struct FtpError {
    status: StatusCode,
    message: String,
}

impl FtpError {
    fn bad(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    fn deny(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    fn unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: msg.into(),
        }
    }
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

pub(super) async fn ftp_call(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let id = req
        .extensions()
        .get::<cap::PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let body = cap::read_json(req).await;
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));

    let grants = match state.db().plugin(&id) {
        Ok(Some(p)) => p.grants,
        _ => Vec::new(),
    };
    let allowed = grants
        .iter()
        .any(|g| g.kind == "perm" && (g.value == "ftp:*" || g.value == format!("ftp:{method}")));
    if !allowed {
        let _ = state.db().audit(
            &format!("plugin:{id}"),
            "uds",
            &format!("ftp.deny.{}", method),
            &id,
            "deny",
            "缺少 ftp 能力授权",
        );
        return (StatusCode::FORBIDDEN, "缺少 ftp 能力授权").into_response();
    }

    match dispatch(&state, &method, &params).await {
        Ok(v) => {
            let _ = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &format!("ftp.{method}"),
                "",
                "ok",
                "",
            );
            Json(v).into_response()
        }
        Err(e) => {
            let _ = state.db().audit(
                &format!("plugin:{id}"),
                "uds",
                &format!("ftp.{method}"),
                "",
                if e.status == StatusCode::FORBIDDEN {
                    "deny"
                } else {
                    "fail"
                },
                &e.message,
            );
            (e.status, e.message).into_response()
        }
    }
}

async fn dispatch(state: &AppState, method: &str, params: &Value) -> Result<Value, FtpError> {
    if state.cfg.form != Form::Native {
        return Err(FtpError::unavailable(
            "受限部署形态不支持宿主机 FTP 服务（需要 apt/systemd/pure-ftpd）",
        ));
    }
    dispatch_host(state, method, params).await
}

#[cfg(not(target_os = "linux"))]
async fn dispatch_host(
    _state: &AppState,
    _method: &str,
    _params: &Value,
) -> Result<Value, FtpError> {
    Err(FtpError::unavailable("ftp 能力面仅在 Linux 宿主机可用"))
}

#[cfg(target_os = "linux")]
async fn dispatch_host(state: &AppState, method: &str, params: &Value) -> Result<Value, FtpError> {
    match method {
        "status" => status().await,
        "install" => install(params).await,
        "uninstall" => uninstall(params).await,
        "sites.list" => sites_list(state).await,
        "accounts.list" => accounts_list(state).await,
        "account.create" => account_create(state, params).await,
        "account.password" => account_password(params).await,
        "account.delete" => account_delete(params).await,
        "service.restart" => service_restart().await,
        other => Err(FtpError::deny(format!("未知 ftp 方法：{other}"))),
    }
}

// ───────────────────────────── 纯校验（跨平台可单测） ─────────────────────────

/// 账号名：字母数字开头，1–32 位，只允许 `[A-Za-z0-9_-]`。
pub(crate) fn valid_account_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.len() <= 32 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 密码：8–128 字节，禁止控制字符（防 stdin 注入与日志污染）。
pub(crate) fn valid_password(pw: &str) -> bool {
    (8..=128).contains(&pw.len()) && !pw.chars().any(|c| c.is_control())
}

/// 被动端口：高端口、闭区间、跨度 ≤ 500，避免占满宿主端口表。
pub(crate) fn valid_port_range(start: u16, end: u16) -> bool {
    start >= 1024 && end >= 1024 && start < end && (end - start) <= 500
}

/// 稳定且可读的 FTP 登录名：`skiff_<域名slug>_<hash4>_<账号>`。
///
/// 域名 slug 只用于可读性；hash4 防止 `a-b` 与 `a.b` 这类清洗碰撞。
pub(crate) fn ftp_login(domain: &str, account: &str) -> String {
    let mut slug = String::new();
    for c in domain.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').chars().take(16).collect::<String>();
    format!("{LOGIN_PREFIX}{}_{}_{}", slug, fnv4(domain), account)
}

fn fnv4(input: &str) -> String {
    let mut h: u32 = 2_166_136_261;
    for b in input.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    format!("{h:08x}").chars().take(4).collect()
}

// ───────────────────────────── Linux 实现 ─────────────────────────

#[cfg(target_os = "linux")]
async fn status() -> Result<Value, FtpError> {
    let installed = Path::new("/usr/sbin/pure-ftpd").exists();
    let running = systemctl_active(SERVICE).await;
    let enabled = systemctl_enabled(SERVICE).await;
    let version = if installed {
        run("dpkg-query", &["-W", "-f=${Version}", "pure-ftpd"], None)
            .await
            .ok()
            .map(|o| o.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    let (start, end) = read_port_range().await;
    let tls = read_conf("TLS")
        .await
        .unwrap_or_else(|| "0".into())
        .trim()
        .to_string();
    let accounts = read_accounts().map(|v| v.len()).unwrap_or(0);
    let login_guard = systemctl_active("fail2ban").await;
    Ok(json!({
        "installed": installed,
        "running": running,
        "enabled": enabled,
        "version": version,
        "passive_port_start": start,
        "passive_port_end": end,
        "tls_mode": tls,
        "login_guard": login_guard,
        "accounts": accounts,
        "passwd_file": PASSWD_FILE,
        "pdb_file": PDB_FILE,
    }))
}

#[cfg(target_os = "linux")]
async fn install(params: &Value) -> Result<Value, FtpError> {
    let start = params
        .get("passive_port_start")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_PORT_START as u64) as u16;
    let end = params
        .get("passive_port_end")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_PORT_END as u64) as u16;
    if !valid_port_range(start, end) {
        return Err(FtpError::bad(
            "被动端口范围非法（需 1024–65535，start<end，跨度≤500）",
        ));
    }
    let tls_mode = match params
        .get("tls_mode")
        .and_then(Value::as_str)
        .unwrap_or("optional")
    {
        "off" | "0" => "0",
        "required" | "2" => "2",
        "optional" | "1" => "1",
        other => return Err(FtpError::bad(format!("未知 tls_mode：{other}"))),
    };
    let login_guard = params
        .get("login_guard")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let had_f2b = Path::new("/usr/sbin/fail2ban-client").exists()
        || Path::new("/usr/bin/fail2ban-client").exists();

    if !Path::new("/usr/sbin/pure-ftpd").exists() {
        let mut pkgs = vec!["pure-ftpd"];
        if login_guard && !had_f2b {
            pkgs.push("fail2ban");
        }
        let mut args = vec!["install", "-y", "--no-install-recommends"];
        args.extend(pkgs.iter().copied());
        let mut out = run_host("/usr/bin/apt-get", &args, None)
            .await
            .map_err(|e| FtpError::internal(format!("安装 pure-ftpd 失败：{e}")))?;
        if out.contains("Unable to locate package") {
            run_host("/usr/bin/apt-get", &["update"], None)
                .await
                .map_err(|e| FtpError::internal(format!("apt-get update 失败：{e}")))?;
            out = run_host("/usr/bin/apt-get", &args, None)
                .await
                .map_err(|e| FtpError::internal(format!("安装 pure-ftpd 失败：{e}")))?;
        }
        if !Path::new("/usr/sbin/pure-ftpd").exists() {
            return Err(FtpError::internal(format!(
                "pure-ftpd 安装后仍不存在：{out}"
            )));
        }
    }

    // standalone 模式：Debian 默认即 standalone；显式固化，避免被 inetd 抢占 21。
    ensure_default_file().await?;
    // 若安装时仍拉入了 openbsd-inetd，停用它；pure-ftpd 由 systemd 托管。
    let _ = run_host(
        "/usr/bin/systemctl",
        &["disable", "--now", "openbsd-inetd"],
        None,
    )
    .await;
    let _ = run_host("/usr/bin/systemctl", &["disable", "--now", "inetd"], None).await;

    write_conf("ChrootEveryone", "yes").await?;
    write_conf("NoAnonymous", "yes").await?;
    write_conf("PAMAuthentication", "no").await?;
    write_conf("UnixAuthentication", "no").await?;
    write_conf("PureDB", PDB_FILE).await?;
    // MinUID 不写死：pure-ftpd 默认 1000，而 PHP 镜像的 www-data 常见 82。
    // 建号时按容器内探测到的 uid 只降不升（见 ensure_min_uid），换镜像不会全线登录失败。
    write_conf("PassivePortRange", &format!("{start} {end}")).await?;
    write_conf("Umask", "002:002").await?;
    write_conf("MaxClientsNumber", "30").await?;
    write_conf("MaxClientsPerIP", "3").await?;
    write_conf("DontResolve", "yes").await?;
    write_conf("TLS", tls_mode).await?;
    if tls_mode != "0" {
        ensure_tls_pem().await?;
    }

    if login_guard {
        ensure_login_guard(had_f2b).await?;
    }

    run("systemctl", &["daemon-reload"], None)
        .await
        .map_err(|e| FtpError::internal(format!("systemctl daemon-reload 失败：{e}")))?;
    run_host("/usr/bin/systemctl", &["enable", SERVICE], None)
        .await
        .map_err(|e| FtpError::internal(format!("启用 pure-ftpd 失败：{e}")))?;
    run("systemctl", &["restart", SERVICE], None)
        .await
        .map_err(|e| FtpError::internal(format!("启动 pure-ftpd 失败：{e}")))?;
    if !systemctl_active(SERVICE).await {
        return Err(FtpError::internal("pure-ftpd 启动后未进入 active"));
    }
    status().await
}

#[cfg(target_os = "linux")]
async fn uninstall(params: &Value) -> Result<Value, FtpError> {
    let remove_accounts = params
        .get("remove_accounts")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let remove_package = params
        .get("remove_package")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let accounts = read_accounts().unwrap_or_default();
    if remove_accounts {
        for a in &accounts {
            let _ = run_host(
                "/usr/sbin/pure-pw",
                &["userdel", &a.login, "-f", PASSWD_FILE, "-m"],
                None,
            )
            .await;
        }
    }
    let _ = run_host("/usr/bin/systemctl", &["stop", SERVICE], None).await;
    let _ = run_host("/usr/bin/systemctl", &["disable", SERVICE], None).await;
    let _ = run_host("/usr/bin/rm", &["-f", JAIL_FILE], None).await;
    let _ = run("systemctl", &["restart", "fail2ban"], None).await;
    if remove_package {
        let _ = run_host("/usr/bin/apt-get", &["remove", "-y", "pure-ftpd"], None).await;
        let _ = run_host("/usr/bin/rm", &["-f", PASSWD_FILE, PDB_FILE], None).await;
        let _ = run_host("/usr/bin/rm", &["-rf", CONF_DIR], None).await;
    }
    Ok(json!({
        "stopped": true,
        "removed_accounts": if remove_accounts { accounts.len() } else { 0 },
        "removed_package": remove_package,
    }))
}

#[cfg(target_os = "linux")]
async fn service_restart() -> Result<Value, FtpError> {
    if !Path::new("/usr/sbin/pure-ftpd").exists() {
        return Err(FtpError::bad("pure-ftpd 尚未安装"));
    }
    run("systemctl", &["restart", SERVICE], None)
        .await
        .map_err(|e| FtpError::internal(format!("重启 pure-ftpd 失败：{e}")))?;
    let active = systemctl_active(SERVICE).await;
    Ok(json!({ "running": active }))
}

#[cfg(target_os = "linux")]
async fn sites_list(state: &AppState) -> Result<Value, FtpError> {
    let sites = state
        .db()
        .list_sites()
        .map_err(|e| FtpError::internal(format!("读取站点失败：{e}")))?;
    let mut out = Vec::new();
    for s in &sites {
        let (php, runtime_kind, container) = match (&s.site_type[..], s.runtime_id.as_deref()) {
            (TYPE_RUNTIME, Some(id)) => match state.db().runtime_by_id(id) {
                Ok(Some(r)) => (
                    r.kind == "php",
                    r.kind.clone(),
                    Some(skiff_app::sites::runtime_container_name("php", &r.name)),
                ),
                _ => (false, "missing".into(), None),
            },
            _ => (false, String::new(), None),
        };
        let supported = match &s.site_type[..] {
            TYPE_RUNTIME => php,
            TYPE_STATIC => true,
            TYPE_PROXY => false,
            _ => false,
        };
        let reason = if supported {
            ""
        } else if s.site_type == TYPE_PROXY {
            "反向代理站点没有可归属的文档根"
        } else {
            "该运行环境不是 PHP，v1 未验证其文件身份"
        };
        out.push(json!({
            "domain": s.domain,
            "site_type": s.site_type,
            "root_dir": s.root_dir,
            "index_dir": skiff_app::sites::site_index_dir(Path::new(&s.root_dir)).to_string_lossy(),
            "php": php,
            "runtime_kind": runtime_kind,
            "container": container,
            "supported": supported,
            "reason": reason,
        }));
    }
    Ok(json!({ "sites": out }))
}

#[cfg(target_os = "linux")]
async fn accounts_list(state: &AppState) -> Result<Value, FtpError> {
    let accounts = read_accounts().map_err(FtpError::internal)?;
    let sites = state
        .db()
        .list_sites()
        .map_err(|e| FtpError::internal(format!("读取站点失败：{e}")))?;
    let mut out = Vec::new();
    for a in accounts {
        let home = normalize_home(&a.home);
        let site = sites
            .iter()
            .find(|s| normalize_home(&site_index_str(s)) == home)
            .map(|s| s.domain.clone())
            .unwrap_or_default();
        out.push(json!({
            "login": a.login,
            "site": site,
            "uid": a.uid,
            "gid": a.gid,
            "home": home,
        }));
    }
    Ok(json!({ "accounts": out }))
}

#[cfg(target_os = "linux")]
async fn account_create(state: &AppState, params: &Value) -> Result<Value, FtpError> {
    let domain = params
        .get("site")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let account = params
        .get("account")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if domain.is_empty() {
        return Err(FtpError::bad("缺少站点域名"));
    }
    if !valid_account_name(account) {
        return Err(FtpError::bad(
            "账号名需以字母/数字开头，1–32 位，只允许字母、数字、下划线与连字符",
        ));
    }
    if !valid_password(password) {
        return Err(FtpError::bad("密码需 8–128 字节且不能包含控制字符"));
    }
    if !Path::new("/usr/sbin/pure-ftpd").exists() {
        return Err(FtpError::bad("FTP 服务尚未安装，请先调用 install"));
    }

    let site = state
        .db()
        .site_by_domain(domain)
        .map_err(|e| FtpError::internal(format!("读取站点失败：{e}")))?
        .ok_or_else(|| FtpError::not_found(format!("站点不存在：{domain}")))?;
    let index = validate_index(state, &site)?;
    let (uid, gid) = resolve_identity(state, &site).await?;
    ensure_min_uid(uid).await?;
    let login = ftp_login(domain, account);
    let existing = read_accounts().map_err(FtpError::internal)?;
    if existing.iter().any(|a| a.login == login) {
        return Err(FtpError {
            status: StatusCode::CONFLICT,
            message: format!("FTP 账号已存在：{login}"),
        });
    }

    // 只对齐本站点文档根；父目录、log/ssl/conf 与其它站点一律不碰。
    let index_for_task = index.clone();
    tokio::task::spawn_blocking(move || align_tree(&index_for_task, uid, gid))
        .await
        .map_err(|e| FtpError::internal(format!("对齐目录任务失败：{e}")))?
        .map_err(FtpError::internal)?;

    let home = index.to_string_lossy().to_string();
    run_host(
        "/usr/sbin/pure-pw",
        &[
            "useradd",
            &login,
            "-u",
            &uid.to_string(),
            "-g",
            &gid.to_string(),
            "-d",
            &home,
            "-f",
            PASSWD_FILE,
            "-m",
        ],
        Some(&format!("{password}\n{password}\n")),
    )
    .await
    .map_err(|e| FtpError::internal(format!("创建 pure-pw 账号失败：{e}")))?;
    restart_service().await?;

    Ok(json!({
        "login": login,
        "site": domain,
        "uid": uid,
        "gid": gid,
        "home": home,
    }))
}

#[cfg(target_os = "linux")]
async fn account_password(params: &Value) -> Result<Value, FtpError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let password = params.get("password").and_then(Value::as_str).unwrap_or("");
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(FtpError::deny("只能修改本插件创建的 FTP 账号"));
    }
    if !valid_password(password) {
        return Err(FtpError::bad("密码需 8–128 字节且不能包含控制字符"));
    }
    if !read_accounts()
        .map_err(FtpError::internal)?
        .iter()
        .any(|a| a.login == login)
    {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    }
    run_host(
        "/usr/sbin/pure-pw",
        &["passwd", login, "-f", PASSWD_FILE, "-m"],
        Some(&format!("{password}\n{password}\n")),
    )
    .await
    .map_err(|e| FtpError::internal(format!("修改 FTP 密码失败：{e}")))?;
    restart_service().await?;
    Ok(json!({ "login": login }))
}

#[cfg(target_os = "linux")]
async fn account_delete(params: &Value) -> Result<Value, FtpError> {
    let login = params
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !login.starts_with(LOGIN_PREFIX) {
        return Err(FtpError::deny("只能删除本插件创建的 FTP 账号"));
    }
    if !read_accounts()
        .map_err(FtpError::internal)?
        .iter()
        .any(|a| a.login == login)
    {
        return Err(FtpError::not_found(format!("FTP 账号不存在：{login}")));
    }
    run_host(
        "/usr/sbin/pure-pw",
        &["userdel", login, "-f", PASSWD_FILE, "-m"],
        None,
    )
    .await
    .map_err(|e| FtpError::internal(format!("删除 FTP 账号失败：{e}")))?;
    restart_service().await?;
    Ok(json!({ "login": login, "files_kept": true }))
}

#[cfg(target_os = "linux")]
async fn restart_service() -> Result<(), FtpError> {
    run("systemctl", &["restart", SERVICE], None)
        .await
        .map_err(|e| FtpError::internal(format!("重启 pure-ftpd 失败：{e}")))?;
    Ok(())
}

// ───────────────────────────── 站点与身份 ─────────────────────────

#[cfg(target_os = "linux")]
fn validate_index(state: &AppState, site: &Site) -> Result<PathBuf, FtpError> {
    let root = Path::new(&site.root_dir);
    let index = skiff_app::sites::site_index_dir(root);
    let meta = std::fs::symlink_metadata(&index)
        .map_err(|e| FtpError::bad(format!("站点文档根不存在：{}（{e}）", index.display())))?;
    if meta.file_type().is_symlink() {
        return Err(FtpError::bad("文档根不能是符号链接"));
    }
    if !meta.is_dir() {
        return Err(FtpError::bad("文档根不是目录"));
    }
    let sites_root = std::fs::canonicalize(state.cfg.web_root.join("sites"))
        .map_err(|e| FtpError::internal(format!("站点根目录不可用：{e}")))?;
    let real_index =
        std::fs::canonicalize(&index).map_err(|e| FtpError::bad(format!("文档根解析失败：{e}")))?;
    if !real_index.starts_with(&sites_root) {
        return Err(FtpError::deny("文档根不在 /www/sites 下"));
    }
    let real_root =
        std::fs::canonicalize(root).map_err(|e| FtpError::bad(format!("站点根解析失败：{e}")))?;
    if !real_index.starts_with(&real_root) {
        return Err(FtpError::deny("文档根越出站点目录"));
    }
    Ok(real_index)
}

#[cfg(target_os = "linux")]
async fn resolve_identity(state: &AppState, site: &Site) -> Result<(u32, u32), FtpError> {
    match &site.site_type[..] {
        TYPE_RUNTIME => {
            let id = site
                .runtime_id
                .as_deref()
                .ok_or_else(|| FtpError::bad("站点未绑定运行环境"))?;
            let rt = state
                .db()
                .runtime_by_id(id)
                .map_err(|e| FtpError::internal(format!("读取运行环境失败：{e}")))?
                .ok_or_else(|| FtpError::bad("运行环境不存在"))?;
            if rt.kind != "php" {
                return Err(FtpError::bad(format!(
                    "运行环境类型 {} 的文件身份尚未验证，v1 只支持 PHP",
                    rt.kind
                )));
            }
            let container = skiff_app::sites::runtime_container_name("php", &rt.name);
            let uid = probe_id(state, &container, "-u").await?;
            let gid = probe_id(state, &container, "-g").await?;
            if uid == 0 || gid == 0 {
                return Err(FtpError::bad("PHP 容器返回了 root 身份，拒绝对齐"));
            }
            Ok((uid, gid))
        }
        TYPE_STATIC => static_identity(site).await,
        TYPE_PROXY => Err(FtpError::bad("反向代理站点不能作为 FTP 文档根")),
        other => Err(FtpError::bad(format!("未知站点类型：{other}"))),
    }
}

#[cfg(target_os = "linux")]
async fn probe_id(state: &AppState, container: &str, flag: &str) -> Result<u32, FtpError> {
    let (code, out) = state
        .docker()
        .exec_capture(container, vec!["id".into(), flag.into(), "www-data".into()])
        .await
        .map_err(|e| FtpError::bad(format!("无法探测 PHP 容器身份：{e}")))?;
    if code != 0 {
        return Err(FtpError::bad(format!(
            "PHP 容器未运行或缺少 www-data（退出码 {code}）：{}",
            out.trim()
        )));
    }
    out.trim()
        .parse::<u32>()
        .map_err(|_| FtpError::bad(format!("容器身份输出无法解析：{}", out.trim())))
}

/// 非 PHP 站点：用专门的系统 uid，gid 沿用站点目录属组（通常是 skiff-plugins）。
#[cfg(target_os = "linux")]
async fn static_identity(site: &Site) -> Result<(u32, u32), FtpError> {
    let uid = ensure_static_user().await?;
    let root = Path::new(&site.root_dir);
    let gid = std::fs::symlink_metadata(root)
        .map(|m| std::os::unix::fs::MetadataExt::gid(&m))
        .unwrap_or(uid);
    let gid = if gid == 0 { uid } else { gid };
    if uid == 0 || gid == 0 {
        return Err(FtpError::bad("静态站点身份解析为 root，拒绝创建"));
    }
    Ok((uid, gid))
}

#[cfg(target_os = "linux")]
async fn ensure_static_user() -> Result<u32, FtpError> {
    if let Ok(out) = run("getent", &["passwd", "skiff-ftp"], None).await {
        if let Some(uid) = out.trim().split(':').nth(2).and_then(|s| s.parse().ok()) {
            return Ok(uid);
        }
    }
    run_host(
        "/usr/sbin/useradd",
        &[
            "--system",
            "--no-create-home",
            "--shell",
            "/usr/sbin/nologin",
            "skiff-ftp",
        ],
        None,
    )
    .await
    .map_err(|e| FtpError::internal(format!("创建 skiff-ftp 系统用户失败：{e}")))?;
    let out = run("getent", &["passwd", "skiff-ftp"], None)
        .await
        .map_err(|e| FtpError::internal(format!("读取 skiff-ftp 失败：{e}")))?;
    let uid: u32 = out
        .trim()
        .split(':')
        .nth(2)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FtpError::internal("无法解析 skiff-ftp uid"))?;
    if uid == 0 {
        return Err(FtpError::internal("skiff-ftp 解析为 root，拒绝"));
    }
    Ok(uid)
}

/// 递归对齐文档根：目录 2775、文件 0664（可执行文件 0775），属主/属组为目标身份。
/// 不跟随符号链接；`log/`、`ssl/`、`conf/` 不在路径内，天然不受影响。
#[cfg(target_os = "linux")]
fn align_tree(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let is_dir = meta.is_dir();
    std::os::unix::fs::chown(path, Some(uid), Some(gid))
        .map_err(|e| format!("chown {} 失败：{e}", path.display()))?;
    let mut perms = meta.permissions();
    let old = perms.mode();
    let mode = if is_dir {
        0o2775
    } else if old & 0o111 != 0 {
        0o775
    } else {
        0o664
    };
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms)
        .map_err(|e| format!("chmod {} 失败：{e}", path.display()))?;
    if is_dir {
        let entries =
            std::fs::read_dir(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("遍历 {} 失败：{e}", path.display()))?;
            let child = entry.path();
            let child_meta = std::fs::symlink_metadata(&child)
                .map_err(|e| format!("读取 {} 失败：{e}", child.display()))?;
            if child_meta.file_type().is_symlink() {
                continue;
            }
            align_tree(&child, uid, gid)?;
        }
    }
    Ok(())
}

// ───────────────────────────── 宿主文件与命令 ─────────────────────────

#[cfg(target_os = "linux")]
async fn run(cmd: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    run_timeout(cmd, args, stdin, 300).await
}

/// 宿主机落盘动作（apt/pure-pw/写 `/etc`、`/usr`）必须绕开主服务的
/// `ProtectSystem=full`：这些动作在 skiff 服务进程里会被内核拒绝写（只读文件系统）。
/// 这里用 `systemd-run` 起一个**不继承主服务沙箱**的瞬态单元执行；命令仍是
/// 固定二进制 + 固定参数，不经 shell，密码仍只走 stdin。
#[cfg(target_os = "linux")]
async fn run_host(cmd: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let unit = format!("skiff-ftp-{}-{n}", std::process::id());
    let mut full = vec![
        "--unit",
        unit.as_str(),
        "--wait",
        "--pipe",
        "--collect",
        "--quiet",
        "--description",
        "skiff ftp host mutation",
        "--",
        cmd,
    ];
    full.extend_from_slice(args);
    run_timeout("/usr/bin/systemd-run", &full, stdin, 900).await
}

#[cfg(target_os = "linux")]
async fn run_timeout(
    cmd: &str,
    args: &[&str],
    stdin: Option<&str>,
    secs: u64,
) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut command = tokio::process::Command::new(cmd);
    command
        .args(args)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("执行 {cmd} 失败：{e}"))?;
    if let Some(input) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(input.as_bytes())
                .await
                .map_err(|e| format!("写入 {cmd} stdin 失败：{e}"))?;
        }
    }
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("{cmd} 执行超时（{secs}s）"))?
    .map_err(|e| format!("等待 {cmd} 失败：{e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!(
            "{} 退出码 {:?}：{}",
            cmd,
            out.status.code(),
            detail
        ));
    }
    Ok(if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    })
}

#[cfg(target_os = "linux")]
async fn systemctl_active(service: &str) -> bool {
    run("systemctl", &["is-active", service], None)
        .await
        .map(|o| o.trim() == "active")
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
async fn systemctl_enabled(service: &str) -> bool {
    run("systemctl", &["is-enabled", service], None)
        .await
        .map(|o| o.trim() == "enabled")
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
async fn read_conf(name: &str) -> Option<String> {
    tokio::fs::read_to_string(format!("{CONF_DIR}/{name}"))
        .await
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
async fn read_port_range() -> (u16, u16) {
    if let Some(raw) = read_conf("PassivePortRange").await {
        let mut parts = raw.split_whitespace();
        if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
            if let (Ok(a), Ok(b)) = (a.parse(), b.parse()) {
                return (a, b);
            }
        }
    }
    (DEFAULT_PORT_START, DEFAULT_PORT_END)
}

#[cfg(target_os = "linux")]
async fn write_conf(name: &str, value: &str) -> Result<(), FtpError> {
    let path = format!("{CONF_DIR}/{name}");
    run_host("/usr/bin/mkdir", &["-p", CONF_DIR], None)
        .await
        .map_err(|e| FtpError::internal(format!("创建 {CONF_DIR} 失败：{e}")))?;
    run_host("/usr/bin/tee", &[&path], Some(&format!("{value}\n")))
        .await
        .map_err(|e| FtpError::internal(format!("写入配置 {name} 失败：{e}")))?;
    Ok(())
}

/// pure-ftpd 的 `MinUID` 是全局配置：只允许它随实际站点身份**下调**。
///
/// 上调会让已有账号（例如容器 uid 82）在下次登录时被拒，因此这里取
/// 「当前值」与「本次探测 uid」的较小者；探测值来自容器内的 `www-data`，
/// 不是宿主 id，也不是写死的 82。
#[cfg(target_os = "linux")]
async fn ensure_min_uid(uid: u32) -> Result<(), FtpError> {
    let current = read_conf("MinUID")
        .await
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(1000);
    if uid < current {
        write_conf("MinUID", &uid.to_string()).await?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn ensure_default_file() -> Result<(), FtpError> {
    let old = tokio::fs::read_to_string(DEFAULT_FILE)
        .await
        .unwrap_or_default();
    let mut kept = Vec::new();
    let mut seen = false;
    for line in old.lines() {
        if line.starts_with("STANDALONE_OR_INETD=") {
            kept.push("STANDALONE_OR_INETD=standalone".to_string());
            seen = true;
        } else {
            kept.push(line.to_string());
        }
    }
    if !seen {
        kept.push("STANDALONE_OR_INETD=standalone".to_string());
    }
    let body = kept.join("\n") + "\n";
    if let Some(parent) = Path::new(DEFAULT_FILE).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_host("/usr/bin/mkdir", &["-p", parent.as_str()], None)
            .await
            .map_err(|e| FtpError::internal(format!("创建 {parent} 失败：{e}")))?;
    }
    run_host("/usr/bin/tee", &[DEFAULT_FILE], Some(&body))
        .await
        .map_err(|e| FtpError::internal(format!("写入 {DEFAULT_FILE} 失败：{e}")))?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn ensure_tls_pem() -> Result<(), FtpError> {
    if Path::new(TLS_PEM).exists() {
        return Ok(());
    }
    let host = tokio::fs::read_to_string("/etc/hostname")
        .await
        .unwrap_or_else(|_| "skiff.local".into())
        .trim()
        .to_string();
    let key = "/tmp/skiff-ftp-key.pem";
    let cert = "/tmp/skiff-ftp-cert.pem";
    run_host(
        "/usr/bin/openssl",
        &[
            "req",
            "-x509",
            "-nodes",
            "-newkey",
            "rsa:2048",
            "-days",
            "3650",
            "-keyout",
            key,
            "-out",
            cert,
            "-subj",
            &format!("/CN={host}"),
        ],
        None,
    )
    .await
    .map_err(|e| FtpError::internal(format!("生成 TLS 自签证书失败：{e}")))?;
    let key_body = tokio::fs::read_to_string(key)
        .await
        .map_err(|e| FtpError::internal(format!("读取 TLS 私钥失败：{e}")))?;
    let cert_body = tokio::fs::read_to_string(cert)
        .await
        .map_err(|e| FtpError::internal(format!("读取 TLS 证书失败：{e}")))?;
    if let Some(parent) = Path::new(TLS_PEM).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_host("/usr/bin/mkdir", &["-p", parent.as_str()], None)
            .await
            .map_err(|e| FtpError::internal(format!("创建证书目录失败：{e}")))?;
    }
    run_host(
        "/usr/bin/tee",
        &[TLS_PEM],
        Some(&format!("{key_body}{cert_body}")),
    )
    .await
    .map_err(|e| FtpError::internal(format!("写入 {TLS_PEM} 失败：{e}")))?;
    let _ = run_host("/usr/bin/chmod", &["600", TLS_PEM], None).await;
    let _ = tokio::fs::remove_file(key).await;
    let _ = tokio::fs::remove_file(cert).await;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn ensure_login_guard(had_f2b: bool) -> Result<(), FtpError> {
    if !Path::new("/usr/sbin/fail2ban-client").exists()
        && !Path::new("/usr/bin/fail2ban-client").exists()
    {
        return Err(FtpError::internal("fail2ban 未安装，无法启用登录尝试限制"));
    }
    if let Some(parent) = Path::new(JAIL_FILE).parent() {
        let parent = parent.to_string_lossy().to_string();
        run_host("/usr/bin/mkdir", &["-p", parent.as_str()], None)
            .await
            .map_err(|e| FtpError::internal(format!("创建 fail2ban jail.d 失败：{e}")))?;
    }
    // 仅在本次新装 fail2ban 时关闭默认 sshd jail，避免影响宿主既有登录；
    // 已装 fail2ban 的机器不动既有 jail，只追加 pure-ftpd。
    let mut body = String::new();
    if !had_f2b {
        body.push_str("[sshd]\nenabled = false\n\n");
    }
    body.push_str(
        "[pure-ftpd]\nenabled = true\nbackend = systemd\nmaxretry = 3\nfindtime = 10m\nbantime = 10m\n",
    );
    run_host("/usr/bin/tee", &[JAIL_FILE], Some(&body))
        .await
        .map_err(|e| FtpError::internal(format!("写入 fail2ban jail 失败：{e}")))?;
    run_host("/usr/bin/systemctl", &["enable", "fail2ban"], None)
        .await
        .map_err(|e| FtpError::internal(format!("启用 fail2ban 失败：{e}")))?;
    run("systemctl", &["restart", "fail2ban"], None)
        .await
        .map_err(|e| FtpError::internal(format!("启动 fail2ban 失败：{e}")))?;
    if !systemctl_active("fail2ban").await {
        return Err(FtpError::internal(
            "fail2ban 未进入 active，登录尝试限制未生效",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct FtpAccount {
    login: String,
    uid: u32,
    gid: u32,
    home: String,
}

#[cfg(target_os = "linux")]
fn read_accounts() -> Result<Vec<FtpAccount>, String> {
    let body = std::fs::read_to_string(PASSWD_FILE).unwrap_or_default();
    let mut out = Vec::new();
    for line in body.lines() {
        if !line.starts_with(LOGIN_PREFIX) {
            continue;
        }
        let fields: Vec<&str> = line.splitn(7, ':').collect();
        if fields.len() < 6 {
            continue;
        }
        let Ok(uid) = fields[2].parse::<u32>() else {
            continue;
        };
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        out.push(FtpAccount {
            login: fields[0].to_string(),
            uid,
            gid,
            home: fields[5].to_string(),
        });
    }
    out.sort_by(|a, b| a.login.cmp(&b.login));
    Ok(out)
}

#[cfg(target_os = "linux")]
fn normalize_home(home: &str) -> String {
    let mut h = home.trim().to_string();
    while h.ends_with("/./") {
        let keep = h.len() - 3;
        h.truncate(keep);
    }
    while h.ends_with('/') {
        h.pop();
    }
    h
}

#[cfg(target_os = "linux")]
fn site_index_str(site: &Site) -> String {
    skiff_app::sites::site_index_dir(Path::new(&site.root_dir))
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_strict() {
        assert!(valid_account_name("deploy"));
        assert!(valid_account_name("ci-1_x"));
        assert!(!valid_account_name(""));
        assert!(!valid_account_name("-bad"));
        assert!(!valid_account_name("has space"));
        assert!(!valid_account_name("a/b"));
        assert!(!valid_account_name(&"x".repeat(33)));
    }

    #[test]
    fn passwords_reject_control_chars() {
        assert!(valid_password("12345678"));
        assert!(valid_password("correct horse battery staple"));
        assert!(!valid_password("short"));
        assert!(!valid_password("bad\npass"));
        assert!(!valid_password("bad\0pass"));
        assert!(!valid_password(&"x".repeat(129)));
    }

    #[test]
    fn port_ranges_are_bounded() {
        assert!(valid_port_range(30000, 30100));
        assert!(!valid_port_range(30000, 30000));
        assert!(!valid_port_range(30000, 30600));
        assert!(!valid_port_range(21, 100));
        assert!(valid_port_range(65530, 65535));
    }

    #[test]
    fn login_names_are_stable_and_collision_aware() {
        let l1 = ftp_login("a.b", "deploy");
        assert_eq!(l1, ftp_login("a.b", "deploy"));
        assert!(l1.starts_with("skiff_a-b_"));
        assert!(l1.ends_with("_deploy"));
        assert_ne!(l1, ftp_login("a-b", "deploy"));
        assert!(ftp_login("very-long-domain-name.example.com", "deploy").len() <= 64);
    }
}
