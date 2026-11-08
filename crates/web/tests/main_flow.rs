//! 主干集成测试（跨 crate 契约守门人）：登录 → 会话 → RBAC → 插件上传/信任/确认安装。
//!
//! **如实标注的缺口**：
//! - 「建容器」主干不在本文件：需要 Docker 引擎（Windows 开发机 / CI 无守护进程），
//!   该路径只能在 Linux 真机验证（见 docs/DEPLOY_TEST.md 未验证项）。
//! - kernel.sock / Supervisor 拉起插件进程是 unix-only，不在 Windows 测试面内。

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use skiff_docker::{DockerService, Form, Policy, Tier};
use skiff_plugin::{Supervisor, SupervisorConfig};
use skiff_store::{Db, PluginRecord, Role};
use skiff_web::{AppState, WebConfig};
use tower::ServiceExt;

/// 测试用 PHP 站点：绑定 v7 迁移种入的 php-8.5 运行环境。
fn php_site(domain: &str, root: &str) -> skiff_store::NewSite {
    skiff_store::NewSite {
        domain: domain.into(),
        root_dir: root.into(),
        site_type: skiff_store::TYPE_RUNTIME.into(),
        runtime_id: Some("php-8.5".into()),
        app_command: None,
        app_port: None,
        proxy_target: None,
        proxy_websocket: false,
    }
}

fn static_site(domain: &str, root: &str) -> skiff_store::NewSite {
    skiff_store::NewSite {
        domain: domain.into(),
        root_dir: root.into(),
        site_type: skiff_store::TYPE_STATIC.into(),
        runtime_id: None,
        app_command: None,
        app_port: None,
        proxy_target: None,
        proxy_websocket: false,
    }
}

fn proxy_site(domain: &str, root: &str, target: &str, websocket: bool) -> skiff_store::NewSite {
    skiff_store::NewSite {
        domain: domain.into(),
        root_dir: root.into(),
        site_type: skiff_store::TYPE_PROXY.into(),
        runtime_id: None,
        app_command: None,
        app_port: None,
        proxy_target: Some(target.into()),
        proxy_websocket: websocket,
    }
}

struct Harness {
    app: Router,
    state: Arc<AppState>,
    _data: tempfile::TempDir,
    data_dir: PathBuf,
}

fn harness() -> Harness {
    harness_full(Form::Native, Tier::Basic)
}

/// 指定部署形态的 harness：受限形态（1Panel Docker 包）能力面收缩用它。
fn harness_with(form: Form) -> Harness {
    harness_full(form, Tier::Basic)
}

/// 生命周期档 harness：创建容器 / Compose 等 `Tier::Lifecycle` 动词只在宿主机直装形态可用。
fn harness_lifecycle() -> Harness {
    harness_full(Form::Native, Tier::Lifecycle)
}

fn harness_full(form: Form, tier: Tier) -> Harness {
    let data = tempfile::tempdir().unwrap();
    let data_dir = data.path().to_path_buf();
    let db = Arc::new(Db::open_in_memory().unwrap());
    let hash = skiff_identity::hash_password("pw-test-123").unwrap();
    db.create_user("admin", &hash, Role::Admin, false).unwrap();
    db.create_user("viewer", &hash, Role::Viewer, false)
        .unwrap();
    let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
    let sup = Arc::new(Supervisor::new(
        SupervisorConfig::new(data_dir.clone(), data_dir.join("run")),
        db.clone(),
    ));
    let cfg = WebConfig {
        data_dir: data_dir.clone(),
        web_root: data_dir.join("www"),
        trash_retention_days: 30,
        config_path: data_dir.join("config.toml"),
        listen: "127.0.0.1:18080".parse().unwrap(),
        entry_prefix: "p".into(),
        entry_prefix_previous: None,
        tls_enabled: false,
        tls_cert: None,
        tls_key: None,
        allow_insecure_http: true,
        dev_http: true,
        trusted_proxies: vec![],
        external_url: None,
        tier,
        form,
        allow_unsigned: false,
        allow_reduced_sandbox: true,
        policy: Policy::new(tier, form, vec![]).unwrap(),
        session: skiff_identity::SessionLimits::default(),
        login_concurrency: 2,
        ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
        ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
    };
    let state = AppState::new(db, docker, sup, cfg, "csrf-test-secret".into(), "test");
    let app = skiff_web::router(state.clone());
    Harness {
        app,
        state,
        _data: data,
        data_dir,
    }
}

/// 直接落库一条插件记录：只为验证导航「已启用才出现」与插件中心卡片状态，
/// 不经过安装/授权后端（本轮边界禁止改那条链路）。
fn seed_plugin(db: &Db, id: &str, name: &str, label: &str, enabled: bool) {
    let manifest = format!(
        r#"id = "{id}"
name = "{name}"
version = "0.1.0"
key_id = "{}"
api = "1.0"
requires_full_sandbox = false
[[ui]]
label = "{label}"
path = "index.html"
"#,
        "0".repeat(64)
    );
    db.upsert_plugin(&PluginRecord {
        id: id.into(),
        name: name.into(),
        version: "0.1.0".into(),
        api: "1.0".into(),
        key_id: "0".repeat(64),
        manifest,
        enabled,
        status: if enabled { "enabled" } else { "disabled" }.into(),
        status_detail: String::new(),
        active_version: "0.1.0".into(),
        installed_at: 0,
        grants: Vec::new(),
    })
    .unwrap();
}

/// 落库一条**声明式菜单**（`api = "1.1"` + `[[menu]]`）的插件记录，
/// 用于验证 P2 的 L3 标签栏与固定页槽位；同样绕过安装/授权后端。
fn seed_declared_plugin(db: &Db, manifest: String, enabled: bool) {
    let parsed = skiff_pkgfmt::Manifest::parse(&manifest).expect("seed manifest 必须可解析");
    let id = parsed.id.clone();
    let name = parsed.name.clone();
    db.upsert_plugin(&PluginRecord {
        id,
        name,
        version: parsed.version.clone(),
        api: parsed.api.clone(),
        key_id: "0".repeat(64),
        manifest,
        enabled,
        status: if enabled { "enabled" } else { "disabled" }.into(),
        status_detail: String::new(),
        active_version: parsed.version,
        installed_at: 0,
        grants: Vec::new(),
    })
    .unwrap();
}

fn set_cookie_value(resp: &axum::response::Response, name: &str) -> Option<String> {
    for v in resp.headers().get_all(header::SET_COOKIE) {
        let v = v.to_str().ok()?;
        if let Some(rest) = v.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return Some(rest.split(';').next().unwrap_or_default().to_string());
        }
    }
    None
}

/// 整条 `Set-Cookie` 头（含属性），用于断言 Cookie 形态。
fn set_cookie_header(resp: &axum::response::Response, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| {
            v.strip_prefix(name)
                .and_then(|r| r.strip_prefix('='))
                .is_some()
        })
        .map(str::to_string)
}

fn csrf_from(html: &str) -> String {
    let marker = "name=\"csrf\" value=\"";
    let start = html.find(marker).expect("页面应有 CSRF 隐藏字段") + marker.len();
    html[start..]
        .split('"')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn form_body(pairs: &[(&str, &str)]) -> Body {
    let s = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    Body::from(s)
}

/// GET 登录页，返回（表单里的签名 CSRF token，Set-Cookie 下发的 `skiff_login` 值）。
///
/// 浏览器等价链路以表单 token 为准；Cookie 只用于验证「兼容路径」仍可用。
async fn login_page_tokens(app: &Router) -> (String, String) {
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let cookie = set_cookie_value(&page, "skiff_login").expect("登录页应下发 CSRF cookie");
    let html = body_text(page).await;
    (csrf_from(&html), cookie)
}

/// 提交登录表单。`sec_fetch_site` 模拟浏览器来源头；`cookie` 为 `None` 表示浏览器
/// 未回传 `skiff_login`（这正是线上失败场景）。
async fn post_login(
    app: &Router,
    csrf: &str,
    cookie: Option<&str>,
    sec_fetch_site: Option<&str>,
    username: &str,
    password: &str,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method("POST")
        .uri("/p/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, format!("skiff_login={c}"));
    }
    if let Some(site) = sec_fetch_site {
        req = req.header("sec-fetch-site", site);
    }
    app.clone()
        .oneshot(
            req.body(form_body(&[
                ("username", username),
                ("password", password),
                ("csrf", csrf),
                ("otp", ""),
                ("next", ""),
            ]))
            .unwrap(),
        )
        .await
        .unwrap()
}

/// 登录并返回会话令牌（Cookie 值）。
async fn login(app: &Router, username: &str, password: &str) -> String {
    let (csrf, cookie) = login_page_tokens(app).await;
    let resp = post_login(
        app,
        &csrf,
        Some(&cookie),
        Some("same-origin"),
        username,
        password,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "登录应 303 跳转");
    set_cookie_value(&resp, skiff_identity::session_cookie_name(true))
        .expect("登录成功应下发会话 Cookie")
}

/// 构造一个签名的 .wbpkg（`bin/<arch>/plugin` + manifest），返回（包路径、公钥 hex、key_id）。
fn build_package(root: &std::path::Path) -> (PathBuf, String, String) {
    build_package_with(root, "docker", "docker")
}

/// 同上，但可指定 manifest 的 `requires` 能力与权限 capability（受限形态测试用）。
fn build_package_with(
    root: &std::path::Path,
    requires: &str,
    cap: &str,
) -> (PathBuf, String, String) {
    use skiff_pkgfmt::SigningKey;
    let key = SigningKey::generate();
    let src = root.join("pkg-src");
    let arch = skiff_pkgfmt::host_arch();
    std::fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
    std::fs::write(src.join("bin").join(&arch).join("plugin"), b"test-binary").unwrap();
    let manifest = format!(
        r#"id = "com.example.demo"
name = "Demo"
version = "0.1.0"
key_id = "{}"
api = "1.0"
requires_full_sandbox = false
requires = ["{}"]
[[permissions]]
capability = "{}"
verbs = ["list"]
"#,
        key.key_id(),
        requires,
        cap
    );
    std::fs::write(src.join("manifest.toml"), manifest).unwrap();
    let out = root.join("demo.wbpkg");
    skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
    (out, key.public_hex(), key.key_id())
}

/// 复用同一签名密钥构建指定版本/声明集/资源提示的包（重装、升级用例）。
fn build_package_version(
    root: &std::path::Path,
    key: &skiff_pkgfmt::SigningKey,
    version: &str,
    perms: &[&str],
    hints: &[&str],
) -> PathBuf {
    let src = root.join(format!("pkg-src-{version}"));
    let arch = skiff_pkgfmt::host_arch();
    std::fs::create_dir_all(src.join("bin").join(&arch)).unwrap();
    std::fs::write(src.join("bin").join(&arch).join("plugin"), b"test-binary").unwrap();
    let hints_list = hints
        .iter()
        .map(|h| format!("\"{h}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut manifest = format!(
        r#"id = "com.example.demo"
name = "Demo"
version = "{version}"
key_id = "{}"
api = "1.0"
requires_full_sandbox = false
requires = ["docker"]
resource_hints = [{hints_list}]
"#,
        key.key_id()
    );
    for p in perms {
        let (cap, verb) = p.split_once(':').expect("权限串必须是 cap:verb");
        manifest.push_str(&format!(
            "[[permissions]]\ncapability = \"{cap}\"\nverbs = [\"{verb}\"]\n"
        ));
    }
    std::fs::write(src.join("manifest.toml"), manifest).unwrap();
    let out = root.join(format!("demo-{version}.wbpkg"));
    skiff_pkgfmt::package::pack_dir(&src, &out, &key.seed_hex()).unwrap();
    out
}

/// 上传插件包（不跟随跳转），供安装/重装/升级用例复用。
async fn upload_pkg(app: &Router, token: &str, pkg: &std::path::Path) -> axum::response::Response {
    let bytes = std::fs::read(pkg).unwrap();
    let (ct, body) = multipart(&bytes);
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

/// 读取 `.incoming` 里唯一暂存包的确认令牌。
fn staged_confirm_token(data_dir: &std::path::Path) -> String {
    let incoming = data_dir.join("plugins").join(".incoming");
    let staged: Vec<_> = std::fs::read_dir(&incoming)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".wbpkg"))
        .collect();
    assert_eq!(staged.len(), 1, "上传包应暂存一份: {staged:?}");
    staged[0].trim_end_matches(".wbpkg").to_string()
}

fn multipart(pkg: &[u8]) -> (String, Body) {
    let boundary = "----skifftestboundary7d93";
    let mut buf = Vec::new();
    buf.extend_from_slice(
        format!(
            "--{boundary}\r\ncontent-disposition: form-data; name=\"package\"; filename=\"demo.wbpkg\"\r\ncontent-type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    buf.extend_from_slice(pkg);
    buf.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        Body::from(buf),
    )
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// 带会话 GET，返回（状态码，整页 HTML）。
async fn get_page(app: &Router, uri: &str, token: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp).await)
}

#[tokio::test]
async fn login_session_and_dashboard() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "带会话访问仪表盘应 200");
}

/// 会话 Cookie 形态：`Path=/; HttpOnly; SameSite=Lax; Max-Age=...; Secure`。
///
/// 回归线上「登录成功后立刻被打回登录页」：`SameSite=Strict` 会让浏览器在跨站顶层
/// 导航（从聊天/邮件里点链接进面板）上丢弃 Cookie，服务端认不出会话；curl 不受
/// SameSite 约束，所以只能在代码层固化这个不变量。
#[tokio::test]
async fn session_cookie_uses_lax_for_top_level_navigation() {
    let h = harness();
    let (csrf, cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &csrf,
        Some(&cookie),
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let name = skiff_identity::session_cookie_name(true);
    let header = set_cookie_header(&resp, name).expect("登录成功应下发会话 Cookie");
    assert!(header.contains("Path=/"), "{header}");
    assert!(header.contains("HttpOnly"), "{header}");
    assert!(header.contains("SameSite=Lax"), "{header}");
    assert!(!header.contains("SameSite=Strict"), "{header}");
    assert!(header.contains("Max-Age="), "{header}");
    assert!(
        !header.contains("Domain="),
        "会话 Cookie 不应带 Domain: {header}"
    );
}

/// 默认口径：管理员首登不再强制改密，登录后直接落到概览。
#[tokio::test]
async fn default_admin_login_lands_on_overview_without_force_change() {
    let h = harness();
    let (csrf, cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &csrf,
        Some(&cookie),
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/p",
        "默认不得跳到 settings?force=1"
    );
}

/// 改密表单校验失败必须回跳设置页，错误经会话 flash 展示一次，
/// URL 不得带 `?err=`（刷新重放 / 链接带脏数据）。
#[tokio::test]
async fn change_password_failure_redirects_back_to_settings() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let body = format!(
        "csrf={csrf}&old_password=wrong&new_password=new-pw-123&confirm_password=new-pw-123"
    );
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/password")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(loc, "/p/settings", "回跳设置页不得携带消息参数");

    let fetch = |cookie: String| {
        h.app.clone().oneshot(
            Request::builder()
                .uri("/p/settings")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
    };
    let first = fetch(format!("skiff_dev_session={token}")).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let html = body_text(first).await;
    assert!(
        html.contains("当前口令不正确"),
        "首次渲染应显示错误: {html}"
    );
    let second = fetch(format!("skiff_dev_session={token}")).await.unwrap();
    let html = body_text(second).await;
    assert!(
        !html.contains("当前口令不正确"),
        "flash 只能消费一次，刷新不应重放: {html}"
    );
}

/// 显式开启（用户标志为 1）时保留原能力：登录与后续页面强制跳改密页。
#[tokio::test]
async fn explicit_must_change_password_still_redirects() {
    let h = harness();
    let hash = skiff_identity::hash_password("pw-test-123").unwrap();
    h.state
        .db()
        .create_user("force-admin", &hash, Role::Admin, true)
        .unwrap();

    let (csrf, cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &csrf,
        Some(&cookie),
        Some("same-origin"),
        "force-admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/p/settings?force=1"
    );

    let token = set_cookie_value(&resp, skiff_identity::session_cookie_name(true))
        .expect("登录成功应下发会话 Cookie");
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/p/settings?force=1"
    );
}

/// 回归：登录后带会话访问 settings / 容器页必须是 200，而不是 303 回登录页。
#[tokio::test]
async fn session_survives_login_redirect_and_reaches_pages() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let cookie = format!("{}={token}", skiff_identity::session_cookie_name(true));
    for path in ["/p", "/p/settings?force=1", "/p/settings", "/p/containers"] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path} 带会话必须 200（不得 303 回登录页）"
        );
    }
}

/// HTTP/2 允许把 `Cookie` 拆成多个头字段（RFC 9113 §8.1.2.5）。会话令牌落在第二个
/// 字段时也必须能识别，否则浏览器会被误判为未登录。
#[tokio::test]
async fn session_cookie_in_second_cookie_header_is_recognized() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/settings")
                .header(header::COOKIE, "skiff_login=stale")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "第二个 Cookie 头字段里的会话令牌也必须生效"
    );
}

/// 线上缺陷回归：浏览器没有回传 `skiff_login` Cookie 时，只要表单里是登录页签发的
/// 签名 token 且请求同源，登录必须成功。
#[tokio::test]
async fn login_succeeds_when_browser_drops_csrf_cookie() {
    let h = harness();
    let (csrf, _cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &csrf,
        None,
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "缺少 skiff_login Cookie 不应阻断登录"
    );
    assert!(
        set_cookie_value(&resp, skiff_identity::session_cookie_name(true)).is_some(),
        "登录成功必须下发会话 Cookie"
    );

    // 老浏览器没有 Sec-Fetch-Site，但有 Origin：dev_http 下 http:// 与 Host 一致则放行。
    let (csrf2, _) = login_page_tokens(&h.app).await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::HOST, "localhost")
                .header("origin", "http://localhost")
                .body(form_body(&[
                    ("username", "admin"),
                    ("password", "pw-test-123"),
                    ("csrf", &csrf2),
                    ("otp", ""),
                    ("next", ""),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "仅有 Origin 且与 Host 同源时也必须放行"
    );
}

/// 带合法 token 的跨站 POST 必须拒绝（同源判定优先于 token 本身）。
#[tokio::test]
async fn login_rejects_cross_site_post_with_valid_token() {
    let h = harness();
    let (csrf, cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &csrf,
        Some(&cookie),
        Some("cross-site"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let text = body_text(resp).await;
    assert!(text.contains("跨站"), "应明确说明跨站被拒: {text}");
}

/// 超过 10 分钟的登录页给出「已过期」提示，而不是笼统的 CSRF 失败。
#[tokio::test]
async fn login_rejects_expired_page_token_with_clear_message() {
    let h = harness();
    let now = chrono::Utc::now().timestamp();
    let csrf = skiff_identity::signed_token(&h.state.csrf_secret, "login", now - 601);
    let resp = post_login(
        &h.app,
        &csrf,
        None,
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let text = body_text(resp).await;
    assert!(
        text.contains("登录页已过期"),
        "过期 token 必须给出可行动的提示: {text}"
    );
}

/// 双标签页 / bfcache 回退：Cookie 被另一个页面覆盖后，表单 token 仍是唯一依据。
#[tokio::test]
async fn login_accepts_stale_cookie_when_form_token_is_fresh() {
    let h = harness();
    let (csrf_a, cookie_a) = login_page_tokens(&h.app).await;
    let (csrf_b, cookie_b) = login_page_tokens(&h.app).await;
    assert_ne!(csrf_a, csrf_b, "每次打开登录页应签发新 token");

    // 标签页 B 覆盖了 Cookie，标签页 A 的表单仍用 A 的 token
    let resp = post_login(
        &h.app,
        &csrf_a,
        Some(&cookie_b),
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "旧页面 token 应仍然有效"
    );

    // bfcache 回退复提交：Cookie 与表单都还是 A 的旧值，也应成功
    let resp = post_login(
        &h.app,
        &csrf_a,
        Some(&cookie_a),
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

/// 兼容路径：没有任何来源头的老客户端仍走双提交，Cookie 与表单一致才放行。
#[tokio::test]
async fn login_without_origin_headers_falls_back_to_double_submit() {
    let h = harness();
    let (csrf, cookie) = login_page_tokens(&h.app).await;

    let resp = post_login(&h.app, &csrf, None, None, "admin", "pw-test-123").await;
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "无来源头且无 Cookie 的老客户端必须拒绝"
    );

    let resp = post_login(&h.app, &csrf, Some(&cookie), None, "admin", "pw-test-123").await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "curl 兼容路径应仍然可用"
    );
}

/// 伪造/篡改的表单 token 一律拒绝，即使同源且带着 Cookie。
#[tokio::test]
async fn login_rejects_tampered_token() {
    let h = harness();
    let (csrf, cookie) = login_page_tokens(&h.app).await;
    let tampered = format!("{}x", &csrf[..csrf.len() - 1]);
    let resp = post_login(
        &h.app,
        &tampered,
        Some(&cookie),
        Some("same-origin"),
        "admin",
        "pw-test-123",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn final_ia_hides_app_and_keeps_plugin_center_direct() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/containers")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "容器页应 200（无 Docker 时为空态）"
    );
    let html = body_text(resp).await;

    // 契约 P1：固定六一级，0 个插件时「网站」「工具」保留并给可行动空态
    assert!(
        !html.contains(r#"aria-controls="nav-menu-app""#),
        "不应再有旧的「应用」一级: {html}"
    );
    assert!(
        html.contains(r#"aria-controls="nav-menu-sites""#)
            && html.contains(r#"aria-controls="nav-menu-tools""#),
        "网站/工具为空时仍保留一级并给空态: {html}"
    );
    assert!(
        html.contains(r#"href="/p/m/core/sites/list""#)
            && html.contains(r#"href="/p/m/core/sites/env""#)
            && !html.contains(r#"href="/p/m/core/sites/settings""#),
        "网站一级保留列表/运行环境；站点级设置入口已移除: {html}"
    );
    assert!(
        html.contains(r#"href="/p/m/core/docker/settings""#)
            && html.contains(r#"href="/p/m/core/docker/compose""#),
        "Docker 一级必须提供设置与 Compose 入口（镜像加速源/前缀）: {html}"
    );
    // 插件一级拆成已安装 / 发现 / 可更新三个真实页面
    assert!(
        html.contains(r#"aria-controls="nav-menu-plugins""#)
            && html.contains(r#"href="/p/m/core/plugins/installed""#)
            && html.contains(r#"href="/p/m/core/plugins/discover""#)
            && html.contains(r#"href="/p/m/core/plugins/updates""#),
        "插件一级必须是已安装/发现/可更新: {html}"
    );
    // 设置：安全设置 / 菜单设置 / 审计日志
    assert!(
        html.contains(r#"aria-controls="nav-menu-settings""#)
            && html.contains(r#"href="/p/m/core/settings/security""#)
            && html.contains(r#"href="/p/m/core/settings/menu""#)
            && html.contains(r#"href="/p/m/core/settings/audit""#),
        "设置二级必须是安全设置 / 菜单设置 / 审计日志: {html}"
    );
    assert!(
        !html.contains("官方源") && !html.contains("/p/compose"),
        "导航不得挂不存在的页面: {html}"
    );

    // 当前页在 Docker 的「容器」二级项上高亮
    assert!(
        html.contains(r#"class="nav-group current" data-nav-group"#)
            && html.contains(r#"class="nav-sub on""#)
            && html.contains(r#"href="/p/m/core/docker/containers""#),
        "当前页所属一级与二级都必须高亮: {html}"
    );
    assert!(
        html.contains(r#"<button type="button" class="nav-link nav-label" data-nav-label>"#)
            && html.contains(r#"class="nav-caret" data-nav-toggle"#)
            && html.contains(r#"aria-expanded="false""#),
        "一级分组应由「标签 + 独立箭头」驱动并默认收起: {html}"
    );
    // 二级默认带 hidden，折叠时不进入 Tab 顺序。
    assert!(
        html.contains(r#"data-nav-menu hidden"#),
        "折叠的分组二级必须带 hidden: {html}"
    );
}

#[tokio::test]
async fn app_menu_lists_only_enabled_plugins_with_manifest_label() {
    let h = harness();
    seed_plugin(
        h.state.db().as_ref(),
        "com.skiff.nginx",
        "NGINX 站点",
        "NGINX 站点",
        true,
    );
    seed_plugin(
        h.state.db().as_ref(),
        "com.skiff.files",
        "文件管理",
        "文件管理",
        false,
    );
    let token = login(&h.app, "admin", "pw-test-123").await;

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;

    assert!(
        html.contains(r#"aria-controls="nav-menu-sites""#),
        "nginx 老包必须降级挂到「网站」一级: {html}"
    );
    assert!(
        html.contains("NGINX 站点")
            && html.contains(r#"href="/p/m/plugin/com.skiff.nginx/legacy""#),
        "插件二级必须用 [[ui]].label，并指向规范深链: {html}"
    );
    // 文件管理已由内核接管：菜单里只能出现内核入口，不能再有插件入口。
    assert!(
        html.contains(r#"href="/p/m/core/files""#),
        "内核「工具 → 文件管理」入口必须存在: {html}"
    );
    assert!(
        !html.contains(r#"href="/p/m/plugin/com.skiff.files"#),
        "文件管理插件入口必须撤销: {html}"
    );

    // 插件中心卡片仍能看见未启用插件，并给出启用动作而不是打开入口
    let center = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(center.status(), StatusCode::OK);
    let center_html = body_text(center).await;
    assert!(
        center_html.contains("已安装 · 未启用") && center_html.contains(">启用<"),
        "插件中心必须显示未启用状态与启用动作: {center_html}"
    );
    assert!(
        center_html.contains(r#"href="/p/plugins/com.skiff.files">详情"#),
        "未启用插件仍应提供信息与管理入口: {center_html}"
    );
    assert!(
        !center_html.contains(r#"href="/p/plugins/com.skiff.files/main""#),
        "未启用插件不得渲染功能页面死入口: {center_html}"
    );
}

#[tokio::test]
async fn plugin_center_tabs_use_honest_empty_states() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for (uri, expected) in [
        ("/p/plugins?tab=discover", "没有接入在线官方源"),
        ("/p/plugins?tab=updates", "尚未接入更新检查源"),
    ] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri} 应 200");
        let html = body_text(resp).await;
        assert!(html.contains(expected), "{uri} 缺少空态文案「{expected}」");
        assert!(
            !html.contains("可更新版本："),
            "{uri} 不得写死假的更新数字: {html}"
        );
    }
}

#[tokio::test]
async fn current_state_highlights_plugin_center_and_settings_items() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let center = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let center_html = body_text(center).await;
    assert!(
        center_html.contains(r#"class="nav-sub on""#)
            && center_html.contains(r#"href="/p/m/core/plugins/installed""#),
        "插件当前页必须高亮「已安装」: {center_html}"
    );

    let settings = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/settings")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let settings_html = body_text(settings).await;
    assert!(
        settings_html.contains(r#"class="nav-group current" data-nav-group"#)
            && settings_html.contains(r#"class="nav-sub on""#)
            && settings_html.contains(r#"href="/p/m/core/settings/security""#),
        "设置页必须高亮「设置 → 安全设置」: {settings_html}"
    );

    let audit = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/audit")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let audit_html = body_text(audit).await;
    assert!(
        audit_html.contains(r#"class="nav-sub on""#)
            && audit_html.contains(r#"href="/p/m/core/settings/audit""#),
        "审计页必须高亮「设置 → 审计日志」: {audit_html}"
    );
}

#[tokio::test]
async fn overview_and_container_filter_render_explicit_empty_states() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let dash = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dash.status(), StatusCode::OK);
    let dash_html = body_text(dash).await;
    assert!(
        dash_html.contains("需要处理") && dash_html.contains("两步验证未开启"),
        "概览必须给出真实待办（TOTP 未开启）: {dash_html}"
    );
    assert!(
        dash_html.contains("最近审计")
            && dash_html.contains("auth.login")
            && dash_html.contains("Docker 不可用，暂时读不到容器状态"),
        "概览必须接真实审计，并在 Docker 不可用时给出容器空态: {dash_html}"
    );
    assert!(
        dash_html.contains("个未安装"),
        "运行环境只有登记、没有容器时，概览不能笼统宣称「全部运行中」: {dash_html}"
    );

    let filtered = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/containers?filter=error")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(filtered.status(), StatusCode::OK);
    let filtered_html = body_text(filtered).await;
    assert!(
        filtered_html.contains("仅显示异常")
            && filtered_html.contains("清除筛选")
            && filtered_html.contains("Docker 不可用")
            && filtered_html.contains("前往 Docker 设置")
            && filtered_html.contains("docker/settings"),
        "异常筛选深链必须可解释且可恢复: {filtered_html}"
    );
    // Docker 不可达时，镜像/网络/卷入口都要给出同一个恢复出口，而不是裸错误。
    for path in ["/p/images", "/p/networks", "/p/volumes"] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{path} 必须渲染 gate 而非 503"
        );
        let html = body_text(resp).await;
        assert!(
            html.contains("前往 Docker 设置") && html.contains("docker/settings"),
            "{path} 的 gate 必须带「前往 Docker 设置」出口: {html}"
        );
        assert!(
            !html.contains("hyper client error"),
            "{path} 不得把裸连接错误直接甩给用户: {html}"
        );
    }
}

#[tokio::test]
async fn wrong_password_renders_error_and_audits() {
    let h = harness();
    let (login_csrf, _cookie) = login_page_tokens(&h.app).await;
    let resp = post_login(
        &h.app,
        &login_csrf,
        None,
        Some("same-origin"),
        "admin",
        "wrong-pw",
    )
    .await;
    let text = body_text(resp).await;
    assert!(text.contains("不正确"), "错误页应提示口令不正确: {text}");
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "auth.login" && e.result == "fail"),
        "失败登录必须留审计，实际: {:?}",
        audit
    );
}

#[tokio::test]
async fn unauthenticated_page_redirects_to_login() {
    let h = harness();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/containers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(loc.contains("/p/login"), "未登录应跳登录页: {loc}");
}

#[tokio::test]
async fn viewer_cannot_install_plugins() {
    let h = harness();
    let token = login(&h.app, "viewer", "pw-test-123").await;
    let (ct, body) = multipart(b"irrelevant");
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer 装插件必须 403"
    );
}

/// 上传上限必须由应用层（`DEFAULT_LIMITS.archive_bytes` = 100MB）决定。
/// axum 默认 body 上限是 2MB，而 `.wbpkg` 内含插件二进制，2MB 必然不够——
/// 一旦框架默认值先于应用层校验生效，`install` 里的 100MB 判断就永远走不到。
#[tokio::test]
async fn plugin_upload_body_limit_is_not_the_framework_default() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    // 3MB 非法内容：不该在解析前被 413 拦下，应走到应用层校验并给出结构错误
    let (ct, body) = multipart(&vec![0u8; 3 * 1024 * 1024]);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let text = body_text(resp).await;
    assert!(
        !text.contains("上传解析失败"),
        "3MB 上传在 Multipart 解析阶段就被框架默认 body 上限掐断，插件包将无法安装: {text}"
    );
    assert!(
        text.contains("解析失败") || text.contains("包"),
        "应走到应用层的包校验，实际响应: {text}"
    );
}

#[tokio::test]
async fn plugin_install_rejects_unknown_key() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (pkg, _pubkey, _kid) = build_package(&h.data_dir);
    let bytes = std::fs::read(&pkg).unwrap();
    let (ct, body) = multipart(&bytes);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let text = body_text(resp).await;
    assert!(
        text.contains("未导入"),
        "未导入公钥的签名包必须拒装并提示导入: {text}"
    );
}

#[tokio::test]
async fn plugin_install_confirm_main_flow() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let (pkg, pubkey, kid) = build_package(&h.data_dir);
    // 预导入作者公钥（对应 UI 的「设置 → 信任公钥」）
    h.state.db().trust_key(&kid, &pubkey, "测试作者").unwrap();

    let bytes = std::fs::read(&pkg).unwrap();
    let (ct, body) = multipart(&bytes);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "校验通过应渲染确认页");

    // 确认令牌 = .incoming 里暂存文件名（24 位）
    let incoming = h.data_dir.join("plugins").join(".incoming");
    let staged: Vec<_> = std::fs::read_dir(&incoming)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".wbpkg"))
        .collect();
    assert_eq!(staged.len(), 1, "上传包应暂存一份: {staged:?}");
    let confirm_token = staged[0].trim_end_matches(".wbpkg").to_string();

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/confirm")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("token", &confirm_token),
                    ("csrf", &csrf),
                    ("grant", "docker:list"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "确认安装应 303 回列表"
    );

    let rec = h
        .state
        .db()
        .plugin("com.example.demo")
        .unwrap()
        .expect("安装后插件记录必须落库");
    assert_eq!(rec.version, "0.1.0");
    assert_eq!(rec.key_id, kid);
    assert!(
        rec.grants
            .iter()
            .any(|g| g.kind == "perm" && g.value == "docker:list"),
        "确认页勾选的 grant 必须落库: {:?}",
        rec.grants
    );
    let audit = h.state.db().recent_audit(20).unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "plugin.install" && e.result == "ok"),
        "安装成功必须留审计，实际: {:?}",
        audit
    );
}

/// 装 → 授权 → 重装/升级 → 授权仍在；manifest 新增声明不被自动授予；
/// 只有显式卸载才清空授权。
#[tokio::test]
async fn plugin_reinstall_and_upgrade_preserve_existing_grants() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let key = skiff_pkgfmt::SigningKey::generate();
    h.state
        .db()
        .trust_key(&key.key_id(), &key.public_hex(), "测试作者")
        .unwrap();
    let id = "com.example.demo";

    // 首次安装：manifest 声明 list + inspect，管理员只确认 list，并手授对象 container:abc。
    let pkg = build_package_version(
        &h.data_dir,
        &key,
        "0.1.0",
        &["docker:list", "docker:inspect"],
        &["container:abc"],
    );
    let resp = upload_pkg(&h.app, &token, &pkg).await;
    assert_eq!(resp.status(), StatusCode::OK, "首次安装应渲染确认页");
    let confirm_token = staged_confirm_token(&h.data_dir);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/confirm")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("token", confirm_token.as_str()),
                    ("csrf", csrf.as_str()),
                    ("grant", "docker:list"),
                    ("resource", "container:abc"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "确认安装应 303 回列表"
    );

    let assert_grants = |label: &str| {
        let mut got: Vec<(String, String)> = h
            .state
            .db()
            .grants(id)
            .unwrap()
            .into_iter()
            .map(|g| (g.kind, g.value))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("object".to_string(), "container:abc".to_string()),
                ("perm".to_string(), "docker:list".to_string()),
            ],
            "{label} 授权集合不符: {got:?}"
        );
    };
    assert_grants("首次安装后");

    // 同版本重装：跳过确认页直接安装，授权保持既有最小子集，不重置为 manifest 默认值。
    let pkg = build_package_version(
        &h.data_dir,
        &key,
        "0.1.0",
        &["docker:list", "docker:inspect"],
        &["container:abc", "container:def"],
    );
    let resp = upload_pkg(&h.app, &token, &pkg).await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "重装应跳过确认页直接安装"
    );
    assert_grants("重装后");
    assert_eq!(
        h.state.db().plugin(id).unwrap().unwrap().version,
        "0.1.0",
        "重装应保持同版本"
    );
    let staged = h.data_dir.join("plugins").join(".incoming");
    assert_eq!(
        std::fs::read_dir(&staged).unwrap().count(),
        0,
        "安装成功必须清理 .incoming 暂存"
    );

    // 升级 0.2.0：同样沿用既有授权；新增声明项与新增 hint 都不自动授予。
    let pkg = build_package_version(
        &h.data_dir,
        &key,
        "0.2.0",
        &["docker:list", "docker:inspect", "docker:exec"],
        &["container:abc", "container:def"],
    );
    let resp = upload_pkg(&h.app, &token, &pkg).await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "升级应跳过确认页直接安装"
    );
    assert_grants("升级后");
    assert_eq!(
        h.state.db().plugin(id).unwrap().unwrap().version,
        "0.2.0",
        "升级应写入新版本"
    );

    // 只有管理员显式卸载才清空授权。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/com.example.demo/remove")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[("csrf", csrf.as_str())]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "卸载应 303 回列表");
    assert!(h.state.db().plugin(id).unwrap().is_none(), "卸载应删记录");
    assert!(
        h.state.db().grants(id).unwrap().is_empty(),
        "显式卸载必须清空授权"
    );
}

/// exec 闸门（§6.2）：无票据、重放、换会话、换容器都必须拒绝。
///
/// 直接测 `authorize_exec` 而不是打 WS：WS 升级要在真连接上完成，但「谁能升级」
/// 的判定是纯逻辑，不需要 Docker 也不需要网络。
#[tokio::test]
async fn exec_requires_single_use_ticket_bound_to_session_and_container() {
    use skiff_docker::client::ContainerRow;
    use skiff_store::User;
    use skiff_web::handlers::containers::authorize_exec;
    use skiff_web::{AppError, CurrentUser};

    let h = harness();
    let user = CurrentUser {
        user: User {
            id: 1,
            username: "admin".into(),
            password_hash: String::new(),
            role: Role::Admin,
            totp_secret: None,
            totp_enabled: false,
            must_change_password: false,
            created_at: 0,
        },
        session_id: "sess-a".into(),
        session_token: "tok-a".into(),
        scopes: None,
    };
    let row = ContainerRow {
        id: "cid-1".into(),
        name: "web".into(),
        image: "nginx".into(),
        image_id: "sha256:feedface".into(),
        state: "running".into(),
        status: "Up 1m".into(),
        labels: Default::default(),
        project: None,
        owner: None,
        protected: false,
        unprotected_kernel: false,
    };

    // 没票据 → 403
    assert!(matches!(
        authorize_exec(&h.state, &user, &row, None).await,
        Err(AppError::Forbidden(_))
    ));

    // 正常票据 → 通过，且只能通过一次
    let (ticket, ttl) = h.state.issue_exec_ticket(&user.session_id, &row.id).await;
    assert_eq!(ttl, 30, "票据 TTL 应为一页文档承诺的 30 秒");
    assert!(authorize_exec(&h.state, &user, &row, Some(&ticket))
        .await
        .is_ok());
    assert!(
        authorize_exec(&h.state, &user, &row, Some(&ticket))
            .await
            .is_err(),
        "同一张票据重放必须被拒"
    );

    // 换会话
    let (ticket2, _) = h.state.issue_exec_ticket(&user.session_id, &row.id).await;
    let other = CurrentUser {
        session_id: "sess-b".into(),
        ..user.clone()
    };
    assert!(
        authorize_exec(&h.state, &other, &row, Some(&ticket2))
            .await
            .is_err(),
        "票据必须绑定签发它的会话"
    );

    // 换容器
    let (ticket3, _) = h.state.issue_exec_ticket(&user.session_id, &row.id).await;
    let other_row = ContainerRow {
        id: "cid-2".into(),
        name: "db".into(),
        ..row.clone()
    };
    assert!(
        authorize_exec(&h.state, &user, &other_row, Some(&ticket3))
            .await
            .is_err(),
        "票据必须绑定容器"
    );

    // viewer 即便拿到票据也不行（动词策略在前）
    let viewer = CurrentUser {
        user: User {
            role: Role::Viewer,
            ..user.user.clone()
        },
        ..user.clone()
    };
    let (ticket4, _) = h.state.issue_exec_ticket(&viewer.session_id, &row.id).await;
    assert!(
        authorize_exec(&h.state, &viewer, &row, Some(&ticket4))
            .await
            .is_err(),
        "viewer 不应拿到 exec 权限"
    );
}

/// API Token scope（§6.1）：token 权限 = 角色权限 ∩ scope。
#[tokio::test]
async fn api_token_scopes_are_enforced() {
    let h = harness();
    let admin_id = h
        .state
        .db()
        .user_by_name("admin")
        .unwrap()
        .expect("harness 应建好 admin")
        .id;

    // 建两个 token：只有 plugins:read 的、以及 scope 写错的
    let read_only = "tok-plugins-only";
    h.state
        .db()
        .create_api_token(
            "t-plugins",
            admin_id,
            "plugins-only",
            &skiff_identity::sha256_hex(read_only),
            "plugins:read",
        )
        .unwrap();
    let broken = "tok-broken-scope";
    h.state
        .db()
        .create_api_token(
            "t-broken",
            admin_id,
            "broken",
            &skiff_identity::sha256_hex(broken),
            "api",
        )
        .unwrap();

    // plugins:read → /api/v1/plugins 200
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins")
                .header(header::AUTHORIZATION, format!("Bearer {read_only}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 同一个 token 打 docker 只读端点 → 403（scope 不够，而不是 401/503）
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/docker/containers")
                .header(header::AUTHORIZATION, format!("Bearer {read_only}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "缺少 docker:read 应 403，实际 {}",
        resp.status()
    );

    // scope 值非法 → 401 + 审计（不是静默当成空权限）
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins")
                .header(header::AUTHORIZATION, format!("Bearer {broken}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(h
        .state
        .db()
        .recent_audit(20)
        .unwrap()
        .iter()
        .any(|e| e.action == "auth.token_scope" && e.result == "deny"));

    // 会话（Cookie）不受 scope 收窄：同一端点照样 200
    let session = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/plugins")
                .header(
                    header::COOKIE,
                    format!("{}={session}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "会话认证不应被 scope 收窄");
}

/// 契约（§6.3）：裸 `/` 必须 404 而不是跳登录——302 会把随机安全入口前缀
/// 直接递给扫描器，隐蔽入口当场作废。前缀路径未登录才跳登录页。
///
/// 曾经只有冒烟脚本盯着这件事，而脚本期望写反过（写成 / 应跳转）；
/// 把契约固化在这里，脚本与测试互为对拍。
#[tokio::test]
async fn bare_root_is_hidden_404_and_prefix_redirects_to_login() {
    let h = harness();

    let resp = h
        .app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "裸 / 必须 404（隐蔽入口），跳转会泄露安全入口前缀"
    );

    let resp = h
        .app
        .clone()
        .oneshot(Request::builder().uri("/p").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "前缀路径未登录应跳登录"
    );
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(loc.contains("/p/login"), "跳转目标应是登录页: {loc}");
}

/// 静态资源只认带安全入口前缀的 `/{p}/static/…`；根路径不再兜底。
/// 这样收紧入口路由后，资源不会因为旧地址留下一条旁路。
#[tokio::test]
async fn prefixed_static_assets_are_served() {
    let h = harness();
    let root = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/static/skiff.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        root.status(),
        StatusCode::NOT_FOUND,
        "根路径不得再兜底静态资源"
    );
    for uri in ["/p/static/skiff.js"] {
        let resp = h
            .app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri} 应可匿名获取");
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("javascript"),
            "{uri} 的 Content-Type 不对: {ct}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("skiff.theme.request"), "{uri} 内容不应为空壳");
    }
}

/// 渲染出的 HTML 里不得再出现不带安全入口前缀的 `/static/` 或 `/plugins/` 引用。
#[tokio::test]
async fn rendered_pages_use_only_prefixed_asset_urls() {
    let h = harness();
    let login_resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let login_html = String::from_utf8(
        axum::body::to_bytes(login_resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (_, dash_html) = get_page(&h.app, "/p", &admin).await;

    for (name, html) in [
        ("login", login_html.as_str()),
        ("dashboard", dash_html.as_str()),
    ] {
        for bad in [
            r#"src="/static/"#,
            r#"href="/static/"#,
            r#"src="/plugins/"#,
            r#"href="/plugins/"#,
        ] {
            assert!(
                !html.contains(bad),
                "{name} 不得出现无前缀引用 {bad}: {html}"
            );
        }
    }
}

/// 并发票据复用：同一张 exec 票据被并发出示两次，必须只核销成功一次。
/// 顺序版已有（app 层 ticket_is_single_use），这条守「查重与作废不在同一临界区」
/// 这类并发回归。
#[tokio::test]
async fn exec_ticket_concurrent_reuse_wins_exactly_once() {
    let h = harness();
    let (ticket, _) = h.state.issue_exec_ticket("sess-a", "cid-1").await;

    let a = {
        let st = h.state.clone();
        let t = ticket.clone();
        tokio::spawn(async move { st.consume_exec_ticket(&t, "sess-a", "cid-1").await })
    };
    let b = {
        let st = h.state.clone();
        let t = ticket.clone();
        tokio::spawn(async move { st.consume_exec_ticket(&t, "sess-a", "cid-1").await })
    };
    let (ra, rb) = tokio::join!(a, b);
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert!(
        ra.is_ok() ^ rb.is_ok(),
        "并发下同一票据必须恰好核销一次: ({ra:?}, {rb:?})"
    );
}

/// 受限形态（1Panel Docker 包）能力面收缩（§5.4）：只提供 docker/identity，
/// 需要 fs/net 能力的插件必须在确认安装时被拒，不能装上再坏。
#[tokio::test]
async fn restricted_form_rejects_plugins_beyond_capability_surface() {
    let h = harness_with(Form::DockerRestricted);
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);

    // 需要 fs 能力的包：受限形态没有 fs
    let (pkg, pubkey, kid) = build_package_with(&h.data_dir, "fs", "fs");
    h.state.db().trust_key(&kid, &pubkey, "测试作者").unwrap();

    let bytes = std::fs::read(&pkg).unwrap();
    let (ct, body) = multipart(&bytes);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "上传校验应通过（问题在能力面）"
    );

    let incoming = h.data_dir.join("plugins").join(".incoming");
    let staged: Vec<_> = std::fs::read_dir(&incoming)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".wbpkg"))
        .collect();
    let confirm_token = staged[0].trim_end_matches(".wbpkg").to_string();

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/confirm")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("token", &confirm_token),
                    ("csrf", &csrf),
                    ("grant", "fs:list"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "受限形态装 fs 插件必须被拒"
    );
    let text = body_text(resp).await;
    assert!(
        text.contains("能力"),
        "拒绝原因应指明能力面收缩，实际: {text}"
    );
    assert!(
        h.state.db().plugin("com.example.demo").unwrap().is_none(),
        "被拒的插件不得落库"
    );
    let leftover: Vec<_> = std::fs::read_dir(&incoming)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".wbpkg"))
        .collect();
    assert!(leftover.is_empty(), "确认后暂存包应清理: {leftover:?}");

    // 对照组：docker 能力的包在受限形态下照常可装
    let (pkg2, pubkey2, kid2) = build_package_with(&h.data_dir, "docker", "docker");
    h.state.db().trust_key(&kid2, &pubkey2, "测试作者").unwrap();
    let bytes = std::fs::read(&pkg2).unwrap();
    let (ct, body) = multipart(&bytes);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/install")
                .header(header::CONTENT_TYPE, ct)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let staged: Vec<_> = std::fs::read_dir(&incoming)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".wbpkg"))
        .collect();
    let confirm_token = staged[0].trim_end_matches(".wbpkg").to_string();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/confirm")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("token", &confirm_token),
                    ("csrf", &csrf),
                    ("grant", "docker:list"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "docker 能力的插件在受限形态下应可装"
    );
}

/// 插件功能页的两条 UI 约定（跨插件跳转 + 顶栏插件菜单）：
/// 1. `?open=` 只作为「定位建议」被清洗后原样透传（控制字符清掉、百分号编码）；
/// 2. `data-plugins` 白名单只含「已启用且有 UI 入口」的插件——iframe 的
///    `skiff.open` 只能往这些目标跳，跳转本身不授予任何能力。
///
/// 信息页与功能页的拆分见 `docs/PLUGIN_UI_CONTRACT.md`：`/plugins/{id}` 只由
/// 内核渲染信息与管理，功能界面固定落在 `/plugins/{id}/<view>`。
#[tokio::test]
async fn plugin_info_and_view_split_passes_open_hint_and_whitelists_enabled_plugins() {
    use skiff_store::PluginRecord;

    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let manifest = |id: &str, label: &str| {
        format!(
            r#"id = "{id}"
name = "{label}"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.0"
requires = ["docker"]
[[permissions]]
capability = "docker"
verbs = ["list"]
[[ui]]
label = "{label}"
path = "index.html"
"#
        )
    };
    let seed = |id: &str, label: &str, enabled: bool| PluginRecord {
        id: id.into(),
        name: label.into(),
        version: "0.1.0".into(),
        api: "1.0".into(),
        key_id: "0".repeat(64),
        manifest: manifest(id, label),
        enabled,
        status: if enabled { "enabled" } else { "disabled" }.into(),
        status_detail: String::new(),
        active_version: "0.1.0".into(),
        installed_at: 0,
        grants: vec![],
    };
    // 一个启用、一个停用：后者不得出现在白名单里。
    h.state
        .db()
        .upsert_plugin(&seed("com.example.ui", "示例界面", true))
        .unwrap();
    h.state
        .db()
        .upsert_plugin(&seed("com.example.off", "已停用界面", false))
        .unwrap();

    // 信息页：由内核渲染身份、授权与管理动作，不嵌 iframe。
    let (status, info_html) = get_page(&h.app, "/p/plugins/com.example.ui", &token).await;
    assert_eq!(status, StatusCode::OK);
    for marker in [
        "标识",
        "发布者 / 密钥指纹",
        "签名状态",
        "沙箱模式",
        "声明能力面",
        "关联菜单",
        "日志与事件",
        "管理操作",
    ] {
        assert!(
            info_html.contains(marker),
            "信息页缺少 {marker}: {info_html}"
        );
    }
    assert!(
        info_html.contains(r#"action="/p/plugins/com.example.ui/toggle""#)
            && info_html.contains(r#"action="/p/plugins/com.example.ui/remove""#),
        "信息页必须承载启用/停用与卸载动作: {info_html}"
    );
    assert!(
        info_html.contains(r#"href="/p/m/plugin/com.example.ui/legacy""#),
        "信息页必须列出关联菜单深链: {info_html}"
    );
    assert!(
        info_html.contains(r#"href="/p/plugins/com.example.ui/main""#),
        "已启用插件的信息页必须给出功能页入口: {info_html}"
    );
    assert!(
        !info_html.contains("data-workbench-frame"),
        "信息页不得嵌 iframe 工作台: {info_html}"
    );

    let (status, html) = get_page(
        &h.app,
        "/p/plugins/com.example.ui/main?open=%2Fvar%2Fwww%2Fsite%20one",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // open 建议按 RFC 3986 百分号编码后进 iframe src
    assert!(
        html.contains("open=%2Fvar%2Fwww%2Fsite%20one"),
        "open 建议应编码后透传: {html}"
    );
    // 白名单只含启用的那个插件
    assert!(
        html.contains(r#"data-plugins="com.example.ui""#),
        "白名单应只含已启用且有 UI 的插件: {html}"
    );
    // 沙箱属性必须下发：没有它「opaque origin / 不持面板凭据」只是文案。
    // `allow-forms` 是给插件 UI 的原生表单提交事件（新建目录/跳转对话框），
    // CSP `form-action 'none'` 仍阻断任何真实导航。
    assert!(
        html.contains(r#"sandbox="allow-scripts allow-modals allow-forms""#),
        "插件 iframe 必须带 sandbox: {html}"
    );
    // 顶栏「应用」菜单：启用项的 label 进导航并高亮当前项
    assert!(
        html.contains(r#"class="nav-plugin on""#) && html.contains("示例界面"),
        "当前插件的菜单项应在顶栏高亮: {html}"
    );
    // 启停只允许在插件中心卡片操作，工作台不再出现 toggle 表单
    assert!(
        !html.contains(r#"action="/p/plugins/com.example.ui/toggle""#),
        "工作台不得承载停用/启用动作: {html}"
    );
    assert!(
        html.contains("/p/containers?filter=error"),
        "工作台失败路径必须给异常容器/日志入口: {html}"
    );

    // 控制字符（%0A）必须被清掉，否则会被拼进 URL / 日志
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.ui/main?open=%2Fetc%2F%0Apasswd")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;
    assert!(
        html.contains("open=%2Fetc%2Fpasswd") && !html.contains("%0A"),
        "定位串里的控制字符应被清掉: {html}"
    );
}

#[tokio::test]
async fn menu_settings_hide_layout_order_reset_and_user_scope() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let page = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/settings/menu")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK, "菜单设置页应 200");
    let page_html = body_text(page).await;
    let csrf = csrf_from(&page_html);
    assert!(
        page_html.contains("data-menu-key") && page_html.contains("恢复默认"),
        "菜单设置页必须列出可操作项: {page_html}"
    );
    assert!(
        page_html.contains("内容宽度")
            && page_html.contains(r#"value="auto""#)
            && page_html.contains(r#"name="max""#),
        "菜单设置页必须提供内容宽度选项（自适应/固定 + 最大宽度）: {page_html}"
    );
    assert!(
        page_html.contains(r#"data-layout="left""#)
            && page_html.contains(r#"data-content-width="auto""#),
        "缺省布局应为左右、内容宽度应为自适应（铺满）: {page_html}"
    );

    // 隐藏 Docker→镜像
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings/menu")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("action", "hide"),
                    ("key", "core/docker/images"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let dash = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p")
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !dash.contains(r#"href="/p/m/core/docker/images""#),
        "隐藏后导航不得再出现镜像: {dash}"
    );

    // 另一个账号不受影响（偏好按用户）
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let viewer_html = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p")
                    .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        viewer_html.contains(r#"href="/p/m/core/docker/images""#),
        "viewer 的默认菜单不应受 admin 的隐藏影响: {viewer_html}"
    );

    // 切换布局：只变 data-layout，结构不变
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings/menu")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("action", "layout"),
                    ("layout", "left"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let left = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p")
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        left.contains(r#"data-layout="left""#) && left.contains(r#"id="skiff-main-nav""#),
        "布局切换后同一菜单 DOM 必须保留: {left}"
    );

    // 内容宽度：固定上限写进 body，刷新后保持
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings/menu")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("action", "width"),
                    ("width", "fixed"),
                    ("max", "1600"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let fixed = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p")
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        fixed.contains(r#"data-content-width="fixed""#) && fixed.contains("--content-max: 1600px"),
        "固定宽度必须落库并写进页面: {fixed}"
    );

    // 恢复默认：隐藏、布局与内容宽度一起回到缺省值（左右 + 自适应）
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings/menu")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[("csrf", &csrf), ("action", "reset")]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let reset = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p")
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        reset.contains(r#"data-layout="left""#)
            && reset.contains(r#"data-content-width="auto""#)
            && reset.contains(r#"href="/p/m/core/docker/images""#),
        "恢复默认必须同时复原布局、内容宽度与隐藏项: {reset}"
    );
}

#[tokio::test]
async fn deep_links_legacy_routes_and_role_filtered_menu() {
    let h = harness();
    seed_plugin(
        h.state.db().as_ref(),
        "com.skiff.nginx",
        "NGINX 站点",
        "NGINX 站点",
        true,
    );
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    for uri in [
        "/p/m/overview",
        "/p/m/core/docker/containers",
        "/p/m/core/plugins/discover",
        "/p/m/core/settings/security",
        "/p/m/core/settings/menu",
        "/p/m/plugin/com.skiff.nginx",
        "/p/containers",
        "/p/plugins",
    ] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "{uri} 应 200（旧路由也必须可用）"
        );
    }

    let detail = body_text(
        h.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/p/m/plugin/com.skiff.nginx/legacy")
                    .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        detail.contains("/p/plugins/com.skiff.nginx/ui/index.html")
            && detail.contains("NGINX 站点"),
        "旧包菜单深链必须落到 [[ui]].path: {detail}"
    );

    let missing = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/m/nope")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // viewer 没有 AuditRead：审计二级不渲染（不是渲染后 403）。
    for token in [&admin, &viewer] {
        let html = body_text(
            h.app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/p")
                        .header(header::COOKIE, format!("skiff_dev_session={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let has_audit = html.contains(r#"href="/p/m/core/settings/audit""#);
        assert_eq!(
            has_audit,
            token == &admin,
            "审计菜单只对 admin 渲染（viewer={})",
            token == &viewer
        );
    }
}

#[tokio::test]
async fn uninstall_cleanup_removes_plugin_keys_from_user_prefs() {
    let h = harness();
    let admin = h
        .state
        .db()
        .list_users()
        .unwrap()
        .into_iter()
        .find(|u| u.username == "admin")
        .unwrap();
    h.state
        .db()
        .set_user_pref(
            admin.id,
            "menu.prefs",
            r#"{"v":1,"initialized":true,"hidden":["plugin/com.skiff.nginx/legacy"],"pinned":["plugin/com.skiff.nginx/legacy"],"order":{"sites":["plugin/com.skiff.nginx/legacy"]}}"#,
        )
        .unwrap();
    skiff_web::menu::cleanup_plugin_prefs(h.state.db().as_ref(), "com.skiff.nginx");
    let raw = h
        .state
        .db()
        .get_user_pref(admin.id, "menu.prefs")
        .unwrap()
        .unwrap();
    assert!(
        !raw.contains("com.skiff.nginx"),
        "卸载后偏好里不得残留该插件键: {raw}"
    );
}

/// L3 标签栏由内核渲染，多个标签共用同一个 iframe（`menu` + `tab` 参数）。
///
/// 契约：`view` 决定 iframe 加载的包内文件（`ui/<view>.html`），视图名在插件内唯一；
/// 没有 L3 的 L2 只有隐式主视图，**不渲染**标签栏。
#[tokio::test]
async fn l3_tabs_render_share_iframe_and_deep_link() {
    let h = harness();
    let manifest = r#"id = "com.example.tabs"
name = "Tabs Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.2"
requires_full_sandbox = false
[[ui]]
label = "演示"
path = "main.html"
[[menu]]
id = "sites"
parent = "sites"
label = "站点管理"
order = 10
icon = "globe"
roles = ["operator", "admin"]
view = "main"
[[menu.items]]
id = "list"
label = "站点列表"
order = 10
icon = "list"
view = "list"
[[menu.items]]
id = "backup"
label = "备份"
order = 20
icon = "archive"
view = "backup"
[[menu]]
id = "info"
parent = "sites"
label = "单视图信息"
order = 20
icon = "globe"
view = "info"
"#
    .to_string();
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let base = "/p/m/plugin/com.example.tabs";

    // 默认 L2：选中第一个可见 L3，iframe 同时带 `menu`（完整稳定键）与 `tab`。
    let (status, html) = get_page(&h.app, &format!("{base}/sites"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-menu="plugin/com.example.tabs/sites""#),
        "{html}"
    );
    assert!(
        html.contains(r#"class="workbench-tabs""#),
        "应渲染内核 L3 标签栏"
    );
    let bar_pos = html.find(r#"class="workbench-tabs""#).unwrap();
    let panel_pos = html
        .find(r#"class="panel workbench-panel""#)
        .expect("工作台内容面板");
    assert!(
        bar_pos < panel_pos,
        "L3 标签栏必须在页面标题区下方、内容面板之外: {html}"
    );
    assert!(html.contains(r#"data-local="list""#) && html.contains(r#"data-local="backup""#));
    assert!(html.contains("tab=list"), "默认取第一个 L3 的 view");
    assert!(
        html.contains("/p/plugins/com.example.tabs/ui/list.html?nonce="),
        "默认 iframe 必须加载 ui/list.html: {html}"
    );
    assert!(html.contains(r#"href="/p/m/plugin/com.example.tabs/sites/backup""#));
    let pos = html.find(r#"data-local="list""#).unwrap();
    let around = &html[pos.saturating_sub(200)..(pos + 700).min(html.len())];
    assert!(
        around.contains(r#"aria-current="page""#),
        "默认应选中第一个 L3: {around}"
    );

    // L3 深链：切到备份，iframe 的 tab 跟着变，仍复用同一个 iframe DOM。
    let (status, html) = get_page(&h.app, &format!("{base}/sites/backup"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("tab=backup"), "{html}");
    assert!(
        html.contains("/p/plugins/com.example.tabs/ui/backup.html?nonce="),
        "深链切换后 iframe 必须换成 ui/backup.html: {html}"
    );
    let pos = html.find(r#"data-local="backup""#).unwrap();
    let around = &html[pos.saturating_sub(220)..(pos + 700).min(html.len())];
    assert!(
        around.contains(r#"aria-current="page""#),
        "备份标签应标记当前态: {around}"
    );
    assert_eq!(
        html.matches("data-workbench-frame").count(),
        1,
        "只有一份 iframe"
    );

    // 不存在的 L3 直接 404；角色过滤在菜单层完成（不是渲染后 403）。
    let (status, _) = get_page(&h.app, &format!("{base}/sites/nope"), &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_page(&h.app, &format!("{base}/sites/list"), &viewer).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 旧深链 `legacy` 升级到声明式后落到首个可见 L2，不破坏旧链接。
    let (status, html) = get_page(&h.app, &format!("{base}/legacy"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-menu="plugin/com.example.tabs/sites""#),
        "{html}"
    );

    // 规范功能路由：视图名直接寻址，未知视图 404。
    let (status, html) = get_page(&h.app, "/p/plugins/com.example.tabs/list", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("tab=list") && html.contains(r#"class="workbench-tabs""#),
        "{html}"
    );
    let (status, html) = get_page(&h.app, "/p/plugins/com.example.tabs/backup", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("tab=backup"), "{html}");
    let (status, _) = get_page(&h.app, "/p/plugins/com.example.tabs/nope", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知视图必须 404");

    // 无 L3 的 L2：隐式主视图，不渲染标签栏，但深链仍可直达。
    let (status, html) = get_page(&h.app, "/p/plugins/com.example.tabs/info", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/p/plugins/com.example.tabs/ui/info.html?nonce="),
        "单视图 L2 必须加载 ui/info.html: {html}"
    );
    assert!(
        !html.contains(r#"class="workbench-tabs""#),
        "无 L3 时内核不得渲染标签栏: {html}"
    );
    let (status, html) = get_page(&h.app, &format!("{base}/info/info"), &admin).await;
    assert_eq!(status, StatusCode::OK, "单视图深链应接受隐式主视图名");
    assert!(html.contains("tab=info"), "{html}");
    let (status, _) = get_page(&h.app, &format!("{base}/info/nope"), &viewer).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// P2：`overview.health` 槽位成功渲染；插件停用/角色不可见时回落内核健康块。
#[tokio::test]
async fn overview_health_mount_renders_and_falls_back() {
    let h = harness();
    let manifest = r#"id = "com.example.health"
name = "Health Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.2"
requires_full_sandbox = false
[[ui]]
label = "主机监控"
path = "main.html"
[[menu]]
id = "monitor"
parent = "tools"
label = "主机监控"
order = 30
icon = "activity"
view = "main"
[[mount]]
slot = "overview.health"
ui = "health.html"
height = "sm"
refresh = 30
roles = ["admin"]
"#
    .to_string();
    seed_declared_plugin(h.state.db().as_ref(), manifest.clone(), true);
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    // 已启用 + 角色可见：内核卡片外框 + 插件 iframe，且替换默认健康块。
    let (status, html) = get_page(&h.app, "/p", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"data-slot="overview.health""#), "{html}");
    assert!(html.contains("/p/plugins/com.example.health/ui/health.html?nonce="));
    assert!(html.contains(r#"data-refresh="30""#));
    assert!(html.contains(r#"class="mount-frame""#));
    assert!(
        html.contains(r#"data-mount-fail"#),
        "失败降级块必须随卡片一起渲染"
    );
    assert!(
        html.contains(r#"href="/p/m/core/sites/list""#),
        "网站健康块由内核自带，不随插件槽位消失"
    );

    // 角色过滤：viewer 看不到 admin-only 槽位，回落内核默认健康块。
    let (status, html) = get_page(&h.app, "/p", &viewer).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"data-slot="overview.health""#));
    assert!(html.contains(r#"class="health-grid""#));

    // 插件停用：槽位消失，同样回落默认健康块。
    seed_declared_plugin(h.state.db().as_ref(), manifest, false);
    let (status, html) = get_page(&h.app, "/p", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"data-slot="overview.health""#));
    assert!(html.contains(r#"class="health-grid""#));
}

/// 内核信息页：身份字段、已授权资源、插件间接口、关联菜单与管理动作
/// 全部由内核渲染（`docs/PLUGIN_UI_CONTRACT.md` §2）。
#[tokio::test]
async fn plugin_info_page_renders_grants_interop_and_management() {
    let h = harness();
    let manifest = r#"id = "com.example.info"
name = "Info Demo"
version = "0.1.0"
key_id = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd"
api = "1.2"
requires = ["fs", "interop"]
[[permissions]]
capability = "fs"
verbs = ["read"]
[[permissions]]
capability = "interop"
verbs = ["call"]
[[ui]]
label = "信息演示"
path = "main.html"
[[menu]]
id = "panel"
parent = "tools"
label = "信息面板"
order = 10
icon = "tools"
view = "main"
[interop]
consumes = ["com.skiff.cron:jobs.list"]
provides = [{ method = "probe", label = "探针" }]
"#
    .to_string();
    let parsed = skiff_pkgfmt::Manifest::parse(&manifest).unwrap();
    h.state
        .db()
        .upsert_plugin(&PluginRecord {
            id: parsed.id.clone(),
            name: parsed.name.clone(),
            version: parsed.version.clone(),
            api: parsed.api.clone(),
            key_id: "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd".into(),
            manifest,
            enabled: true,
            status: "enabled".into(),
            status_detail: String::new(),
            active_version: parsed.version.clone(),
            installed_at: 0,
            grants: vec![
                skiff_store::Grant {
                    kind: "path".into(),
                    value: "/proc".into(),
                },
                skiff_store::Grant {
                    kind: "plugin".into(),
                    value: "com.skiff.cron".into(),
                },
            ],
        })
        .unwrap();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/plugins/com.example.info", &admin).await;
    assert_eq!(status, StatusCode::OK);
    for marker in [
        "com.example.info",
        "abcdefabcdef",
        "签名状态",
        "未验签",
        "完整沙箱",
        "fs:read",
        "path:/proc",
        "plugin:com.skiff.cron",
        "com.skiff.cron:jobs.list",
        "probe · 探针",
        r#"href="/p/m/plugin/com.example.info/panel""#,
        r#"href="/p/plugins/com.example.info/main""#,
    ] {
        assert!(html.contains(marker), "信息页缺少 {marker}: {html}");
    }
    // 信息页内的启停动作必须回信息页（`next` 只允许本站插件前缀）。
    assert!(
        html.contains(r#"name="next" value="/p/plugins/com.example.info""#),
        "信息页启停表单必须带回跳地址: {html}"
    );

    // 单视图 L2：规范视图路由可直达，且不渲染标签栏。
    let (status, view_html) = get_page(&h.app, "/p/plugins/com.example.info/main", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        view_html.contains("/p/plugins/com.example.info/ui/main.html?nonce="),
        "{view_html}"
    );
    assert!(
        !view_html.contains(r#"class="workbench-tabs""#),
        "{view_html}"
    );
    let (status, _) = get_page(&h.app, "/p/plugins/com.example.info/nope", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 停用后信息页仍可见（用于恢复/卸载），但不再提供功能页入口。
    h.state
        .db()
        .set_plugin_enabled("com.example.info", false)
        .unwrap();
    let (status, html) = get_page(&h.app, "/p/plugins/com.example.info", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("已停用") && html.contains("启用插件"),
        "{html}"
    );
    assert!(
        !html.contains(r#"href="/p/plugins/com.example.info/main""#),
        "停用插件不得给出打开入口: {html}"
    );
}

/// 内核兜底：第三方插件托管 HTML 会被注入标题抑制规则，残留的
/// `data-plugin-title` 页面标题在 iframe 内不显示（标题统一由内核提供）。
#[tokio::test]
async fn plugin_ui_html_gets_kernel_title_guard() {
    let h = harness();
    let manifest = r#"id = "com.example.title"
name = "Title Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.2"
requires_full_sandbox = false
[[ui]]
label = "标题演示"
path = "plain.html"
[[menu]]
id = "plain"
parent = "tools"
label = "标题演示"
view = "plain"
"#
    .to_string();
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let ui_dir = h
        .data_dir
        .join("plugins/com.example.title/versions/0.1.0/ui");
    std::fs::create_dir_all(&ui_dir).unwrap();
    std::fs::write(
        ui_dir.join("plain.html"),
        "<!doctype html><html><head><title>第三方标题</title></head>\
         <body><h1 data-plugin-title>第三方标题</h1><h2>区块小标题</h2></body></html>",
    )
    .unwrap();

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.title/ui/plain.html")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/html"),
        "HTML 必须按 text/html 托管"
    );
    let html = body_text(resp).await;
    assert!(
        html.contains("data-kernel-title-guard")
            && html.contains("[data-plugin-title]{display:none!important}"),
        "托管 HTML 必须注入内核标题抑制规则: {html}"
    );
    assert!(
        html.contains("<h2>区块小标题</h2>"),
        "注入不得误伤区块小标题: {html}"
    );
}

/// 旧包兼容：只有 `[[ui]]` 的老包，默认视图 `main` 指向原有 `ui/index.html`；
/// 未知视图 404，且不渲染标签栏。
#[tokio::test]
async fn legacy_ui_only_package_maps_main_to_index_html() {
    let h = harness();
    seed_plugin(
        h.state.db().as_ref(),
        "com.example.legacy",
        "旧包界面",
        "旧包界面",
        true,
    );
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/plugins/com.example.legacy/main", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/p/plugins/com.example.legacy/ui/index.html?nonce="),
        "旧包 main 必须回落 ui/index.html: {html}"
    );
    assert!(
        !html.contains(r#"class="workbench-tabs""#),
        "旧包单入口不得渲染标签栏: {html}"
    );
    let (status, _) = get_page(&h.app, "/p/plugins/com.example.legacy/other", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 信息页也必须存在，并给出 main 入口与旧包菜单深链。
    let (status, html) = get_page(&h.app, "/p/plugins/com.example.legacy", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"href="/p/plugins/com.example.legacy/main""#),
        "{html}"
    );
    assert!(
        html.contains(r#"href="/p/m/plugin/com.example.legacy/legacy""#),
        "{html}"
    );
}

/// 官方测试插件 `com.skiff.testkit`：多视图 L2 + 单视图对照 + 概览槽位。
/// manifest 随包断言防止测试台与契约漂移。
#[tokio::test]
async fn testkit_plugin_covers_multi_view_single_view_and_slot() {
    let manifest = include_str!("../../../plugins/testkit/manifest.toml").to_string();
    let m = skiff_pkgfmt::Manifest::parse(&manifest).expect("testkit manifest 必须可解析");
    assert_eq!(m.id, "com.skiff.testkit");
    assert_eq!(m.version, "0.1.0");
    assert_eq!(m.api, "1.2");
    assert_eq!(m.menu.len(), 2, "必须同时覆盖多视图与单视图两种 L2");
    assert_eq!(
        m.menu[0].effective_views(),
        vec!["main", "check", "group"],
        "多视图 L2 必须声明 main/check/group"
    );
    assert!(
        m.menu[1].items.is_empty() && m.menu[1].view == "single",
        "对照 L2 必须是无 L3 的单视图"
    );
    assert!(
        m.mount.iter().any(|x| x.slot == "overview.actions"),
        "testkit 必须声明概览槽位（与 monitor 的 health 槽位不冲突）"
    );
    assert_eq!(m.requires_str(), vec!["fs", "identity", "interop"]);
    assert!(m.resource_hints.iter().any(|h| h == "path:/proc"));
    assert_eq!(m.interop.consumes, vec!["com.skiff.cron:jobs.list"]);
    assert!(m.interop.provides.iter().any(|p| p.method == "probe"));

    let h = harness();
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/plugins/com.skiff.testkit", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("插件架构测试台"), "{html}");
    assert!(
        !html.contains("插件架构测试台 插件架构测试台"),
        "L2 label 与插件名同名时标题不得重复渲染: {html}"
    );
    assert!(
        html.contains(r#"href="/p/plugins/com.skiff.testkit/main""#),
        "{html}"
    );

    let (status, html) = get_page(&h.app, "/p/plugins/com.skiff.testkit/check", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"class="workbench-tabs""#),
        "多视图 L2 必须渲染标签栏: {html}"
    );
    for local in ["main", "check", "group"] {
        assert!(html.contains(&format!(r#"data-local="{local}""#)), "{html}");
    }
    assert!(html.contains("tab=check"), "{html}");
    assert!(
        html.contains("/p/plugins/com.skiff.testkit/ui/check.html?nonce="),
        "{html}"
    );

    let (status, html) = get_page(&h.app, "/p/plugins/com.skiff.testkit/single", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/p/plugins/com.skiff.testkit/ui/single.html?nonce="),
        "{html}"
    );
    assert!(
        !html.contains(r#"class="workbench-tabs""#),
        "单视图 L2 不得渲染标签栏: {html}"
    );

    let (status, html) = get_page(&h.app, "/p", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-slot="overview.actions""#),
        "testkit 的概览槽位必须渲染: {html}"
    );
    assert!(
        html.contains("/p/plugins/com.skiff.testkit/ui/mount.html?nonce="),
        "{html}"
    );
}

/// 官方包「装完即可用」回归：files 必须有默认 path: 授权建议，
/// cron 必须声明 scheduler 能力（否则全新安装后 files RPC / cron 列表 400）。
#[tokio::test]
async fn official_packages_declare_usable_install_defaults() {
    let files = skiff_pkgfmt::Manifest::parse(include_str!("../../../plugins/files/manifest.toml"))
        .expect("files manifest 必须可解析");
    let perms = files.permission_strings();
    for p in ["fs:read", "fs:write", "fs:list", "fs:mkdir", "fs:rename"] {
        assert!(perms.iter().any(|x| x == p), "files 缺少 {p}: {perms:?}");
    }
    assert!(
        files.resource_hints.iter().any(|h| h.starts_with("path:/")),
        "files 必须给出默认 path: 授权建议，否则安装后无法浏览: {:?}",
        files.resource_hints
    );

    let cron = skiff_pkgfmt::Manifest::parse(include_str!("../../../plugins/cron/manifest.toml"))
        .expect("cron manifest 必须可解析");
    let cron_perms = cron.permission_strings();
    for p in [
        "scheduler:read",
        "scheduler:manage",
        "scheduler:run",
        "scheduler:admin",
    ] {
        assert!(
            cron_perms.iter().any(|x| x == p),
            "cron 缺少 {p}，全新安装后列表会 400: {cron_perms:?}"
        );
    }
    assert!(cron.cron.is_empty(), "cron 插件不得再声明自建 cron 计时器");
}

/// 计划任务：新建任务是列表页右上角的按钮 + 页内弹窗，不是 L3 独立视图。
/// 守住真实约束：有效视图集合只有 list；旧的 new 视图不得复活。
#[test]
fn cron_new_task_is_a_list_button_not_a_separate_view() {
    let m = skiff_pkgfmt::Manifest::parse(include_str!("../../../plugins/cron/manifest.toml"))
        .expect("cron manifest 必须可解析");
    assert_eq!(m.menu.len(), 1, "cron 只应声明一个 L2 菜单");
    let menu = &m.menu[0];
    assert_eq!(menu.id, "jobs");
    let views = menu.effective_views();
    assert_eq!(
        views,
        vec!["list"],
        "cron 的有效视图集合只能是 [list]，实际 {views:?}"
    );
    assert!(
        !views.contains(&"new"),
        "「新建任务」是按钮，不得再作为独立视图存在"
    );

    let list = include_str!("../../../plugins/cron/ui/list.html");
    assert!(
        list.contains(r#"id="btn-new""#),
        "列表页右上角必须有「新建任务」按钮"
    );
    assert!(
        !list.contains("<h1"),
        "插件页面不得渲染页面级标题（标题由内核提供）"
    );

    let js = include_str!("../../../plugins/cron/ui/app.js");
    assert!(
        js.contains(r#"id="job-dialog""#),
        "新建与编辑必须复用同一个页内弹窗"
    );
    assert!(
        !js.contains("new.html"),
        "列表页脚本不得再跳转到独立的「新建任务」页面"
    );
}

/// 网站列表：默认站点只在列表做标记，切换入口下沉到详情页。
#[tokio::test]
async fn sites_list_shows_default_badge_and_toggle() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    h.state
        .db()
        .create_site(&php_site(
            "demo.local",
            &h.data_dir
                .join("www/sites/demo.local")
                .display()
                .to_string(),
        ))
        .unwrap();
    h.state.db().set_site_default("demo.local", true).unwrap();

    let (status, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("demo.local"), "{html}");
    assert!(html.contains("默认</span>"), "默认标记必须出现");
    assert!(
        !html.contains("/p/sites/demo.local/default"),
        "默认站点操作必须从列表移出: {html}"
    );
    assert!(
        !html.contains("设为默认") && !html.contains("取消默认"),
        "列表不得再有默认切换按钮: {html}"
    );

    let (status, detail) = get_page(&h.app, "/p/sites/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        detail.contains("/p/sites/demo.local/default"),
        "详情页必须提供默认站点切换入口: {detail}"
    );
    assert!(
        detail.contains("取消默认站点"),
        "已是默认时详情页按钮文案应为取消默认站点: {detail}"
    );
}

/// 列表收敛：类型与运行环境合成一列（runtime 显示实例名），别名只在有值时出现，
/// 行操作只保留「详情」，不再有「⋯ 更多」菜单。
#[tokio::test]
async fn sites_list_merges_type_column_and_hides_empty_alias() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let php_root = h.data_dir.join("www/sites/php.local");
    h.state
        .db()
        .create_site(&php_site("php.local", &php_root.display().to_string()))
        .unwrap();
    h.state
        .db()
        .add_site_aliases("php.local", &["www.php.local".to_string()])
        .unwrap();
    h.state
        .db()
        .create_site(&static_site(
            "plain.local",
            &h.data_dir
                .join("www/sites/plain.local")
                .display()
                .to_string(),
        ))
        .unwrap();
    h.state
        .db()
        .create_site(&proxy_site(
            "proxy.local",
            &h.data_dir
                .join("www/sites/proxy.local")
                .display()
                .to_string(),
            "http://127.0.0.1:3000",
            false,
        ))
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("类型 / 运行环境"),
        "类型与运行环境必须合成一列: {html}"
    );
    assert!(
        html.contains(r#"<span class="mono">php-8.5</span>"#),
        "运行环境列必须显示实例名 php-8.5: {html}"
    );
    assert!(
        html.contains(r#"<span class="mono">http://127.0.0.1:3000</span>"#),
        "反向代理列必须显示上游地址: {html}"
    );
    assert!(
        html.contains(r#"<span class="muted">静态</span>"#),
        "静态站点必须显示「静态」: {html}"
    );
    assert!(
        html.contains(r#"class="alias""#),
        "有别名时必须展示别名预览: {html}"
    );
    assert!(
        !html.contains("无别名") && !html.contains("别名 0 个"),
        "没有别名时不得渲染空别名行: {html}"
    );
    assert!(
        !html.contains("row-menu") && !html.contains("更多操作"),
        "行内「⋯ 更多」菜单必须移除: {html}"
    );
    let (_, detail) = get_page(&h.app, "/p/sites/plain.local", &admin).await;
    assert!(
        detail.contains("设为默认站点") || detail.contains("取消默认站点"),
        "默认站点切换必须落到详情页: {detail}"
    );
}

/// 默认站点只能有一个：切换后旧默认必须取消，详情页入口与库状态同源。
#[tokio::test]
async fn sites_default_switch_keeps_single_default() {
    let h = harness();
    for domain in ["a.local", "b.local"] {
        h.state
            .db()
            .create_site(&static_site(
                domain,
                &h.data_dir
                    .join(format!("www/sites/{domain}"))
                    .display()
                    .to_string(),
            ))
            .unwrap();
    }
    h.state.db().set_site_default("a.local", true).unwrap();
    h.state.db().set_site_default("b.local", true).unwrap();

    assert!(
        !h.state
            .db()
            .site_by_domain("a.local")
            .unwrap()
            .unwrap()
            .is_default,
        "切换默认后旧站点必须取消默认"
    );
    assert!(
        h.state
            .db()
            .site_by_domain("b.local")
            .unwrap()
            .unwrap()
            .is_default,
        "新站点必须成为默认"
    );
    assert_eq!(
        h.state.db().default_site().unwrap().unwrap().domain,
        "b.local",
        "任何时刻只能存在一个默认站点"
    );
}

/// 网站列表的证书列必须与详情 SSL 标签同源：证书文件存在即显示已配置，
/// 不能只读 `https` 开关或写死「未配置」。
#[tokio::test]
async fn sites_list_cert_column_matches_ssl_files() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    let s = h.state.site_settings();
    std::fs::create_dir_all(root.join("ssl")).unwrap();
    std::fs::write(s.cert_file(&root), "-----BEGIN CERTIFICATE-----\n").unwrap();
    std::fs::write(s.key_file(&root), "-----BEGIN PRIVATE KEY-----\n").unwrap();
    h.state
        .db()
        .set_site_tls("demo.local", true, false)
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("已启用 HTTPS"),
        "已签发证书且启用 HTTPS 的站点必须显示证书已配置: {html}"
    );

    // 只上传证书、未启用 HTTPS 时，列表也不得回落成「未配置」。
    h.state
        .db()
        .set_site_tls("demo.local", false, false)
        .unwrap();
    let (_, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert!(
        html.contains("证书已上传"),
        "证书文件存在但未启用时应显示已上传: {html}"
    );
}

/// 删除站点必须整目录删除 index/log/ssl/conf；旧路径残留不做兼容也不主动清理。
#[tokio::test]
async fn site_delete_removes_whole_site_directory_only() {
    let h = harness();
    let root = h.data_dir.join("www/sites/demo.local");
    let s = h.state.site_settings();
    for dir in [
        skiff_app::sites::site_index_dir(&root),
        skiff_app::sites::site_log_dir(&root),
        skiff_app::sites::site_ssl_dir(&root),
        skiff_app::sites::site_conf_dir(&root),
    ] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    std::fs::write(
        skiff_app::sites::site_index_dir(&root).join("index.php"),
        "x",
    )
    .unwrap();
    std::fs::write(s.log_file(&root, skiff_app::sites::LogKind::Access), "x").unwrap();
    std::fs::write(s.cert_file(&root), "CERT").unwrap();
    std::fs::write(s.key_file(&root), "KEY").unwrap();
    std::fs::write(s.vhost_file(&root, "demo.local"), "server {}").unwrap();
    // 旧布局残留：新代码不认识也不清理，删除站点时保持原样。
    let legacy_logs = s.web_root.join("logs").join("nginx");
    std::fs::create_dir_all(&legacy_logs).unwrap();
    std::fs::write(legacy_logs.join("demo.local.access.log"), "old").unwrap();
    std::fs::create_dir_all(&s.conf_dir).unwrap();
    std::fs::write(s.conf_dir.join("demo.local.conf"), "server {}").unwrap();

    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    h.state.delete_site("demo.local").await.unwrap();

    assert!(!root.exists(), "整站目录必须一次性删除");
    assert!(
        legacy_logs.join("demo.local.access.log").exists(),
        "旧路径残留不做兼容也不主动删除"
    );
    assert!(s.conf_dir.join("demo.local.conf").exists());
    assert!(
        h.state.db().site_by_domain("demo.local").unwrap().is_none(),
        "站点记录必须删除"
    );
}

/// 删除确认页必须给出具体量，并要求输入域名确认；不带 confirm 的 POST 一律拒绝。
#[tokio::test]
async fn site_delete_confirm_shows_stats_and_requires_typed_domain() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let root = h.data_dir.join("www/sites/demo.local");
    std::fs::create_dir_all(root.join("index/src")).unwrap();
    std::fs::write(root.join("index/index.php"), "0123456789").unwrap();
    std::fs::write(root.join("index/src/main.rs"), "fn main() {}").unwrap();
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites/demo.local/delete", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("data-type-confirm=\"demo.local\""),
        "确认区必须要求输入域名: {html}"
    );
    assert!(
        html.contains("data-type-confirm-submit"),
        "提交按钮必须受域名输入控制: {html}"
    );
    assert!(html.contains("2 个文件"), "确认页必须给出文件数: {html}");
    assert!(
        html.contains("绑定的定时任务"),
        "确认页必须给出定时任务影响: {html}"
    );

    let post = |body: Body| {
        let app = h.app.clone();
        let token = token.clone();
        let uri = "/p/sites/demo.local/delete";
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, format!("skiff_dev_session={token}"))
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    let resp = post(form_body(&[("csrf", csrf.as_str())])).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "未输入域名必须拒绝");
    assert!(
        h.state.db().site_by_domain("demo.local").unwrap().is_some(),
        "拒绝后站点必须仍在"
    );

    let resp = post(form_body(&[
        ("csrf", csrf.as_str()),
        ("confirm", "demo.local"),
    ]))
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(
        h.state.db().site_by_domain("demo.local").unwrap().is_none(),
        "输入匹配后站点删除"
    );
}

/// 批量删除入口已关闭：不能绕过「输入域名确认」，返回 4xx 且站点仍在。
#[tokio::test]
async fn sites_batch_delete_is_rejected_and_keeps_site() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let root = h.data_dir.join("www/sites/batch-guard.local");
    std::fs::create_dir_all(root.join("index")).unwrap();
    h.state
        .db()
        .create_site(&php_site("batch-guard.local", &root.display().to_string()))
        .unwrap();

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/batch")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(form_body(&[
                    ("csrf", csrf.as_str()),
                    ("action", "delete"),
                    ("domains", "batch-guard.local"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "批量删除必须被拒绝，实际 {}",
        resp.status()
    );
    assert!(
        h.state
            .db()
            .site_by_domain("batch-guard.local")
            .unwrap()
            .is_some(),
        "拒绝后站点必须仍在"
    );
    assert!(root.exists(), "拒绝后站点目录必须仍在");
}

/// 站点「目录」页只能浏览文档根 index/：log/ 与 ssl/ 不得通过相对路径越界。
#[tokio::test]
async fn site_dir_listing_is_limited_to_index_dir() {
    let h = harness();
    let root = h.data_dir.join("www/sites/demo.local");
    let s = h.state.site_settings();
    std::fs::create_dir_all(skiff_app::sites::site_index_dir(&root)).unwrap();
    std::fs::create_dir_all(skiff_app::sites::site_log_dir(&root)).unwrap();
    std::fs::create_dir_all(skiff_app::sites::site_ssl_dir(&root)).unwrap();
    std::fs::write(
        skiff_app::sites::site_index_dir(&root).join("hello.txt"),
        "hi",
    )
    .unwrap();
    std::fs::write(s.log_file(&root, skiff_app::sites::LogKind::Access), "log").unwrap();
    std::fs::write(s.key_file(&root), "key").unwrap();
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();

    let listing = h.state.site_dir_listing("demo.local", "").unwrap();
    assert_eq!(
        listing.entries.len(),
        1,
        "根目录只应看到 index/ 内的 hello.txt"
    );
    assert_eq!(listing.entries[0].name, "hello.txt");
    for rel in ["../log", "log/access.log", "../ssl/key.pem"] {
        let err = h.state.site_dir_listing("demo.local", rel).unwrap_err();
        assert!(
            matches!(err, skiff_app::sites::SiteError::BadRequest(_)),
            "{rel} 必须被拒绝：{err:?}"
        );
    }
}

/// 导航交互：静态脚本里不得再有悬停展开；单开手风琴、点空白不收起、Escape 回焦箭头。
#[test]
fn nav_js_has_no_hover_and_is_accordion() {
    let js = include_str!("../static/skiff.js");
    assert!(!js.contains("mouseenter"), "导航不得再监听 hover 展开");
    assert!(!js.contains("mouseleave"), "导航不得再监听 hover 收起");
    assert!(!js.contains("hoverTimer"), "不得保留 320ms 悬停辅助计时器");
    assert!(
        !js.contains("(hover: hover)"),
        "不得保留触摸设备/指针设备的悬停判断"
    );
    assert!(js.contains("function openKey"), "手风琴需要统一的单开函数");
    assert!(
        js.contains("g.dataset.menuKey === key"),
        "展开一个分组前必须先收起其他分组（同一时间只开一个）"
    );
    assert!(
        js.contains("sessionStorage.setItem"),
        "展开状态应在会话内记忆"
    );
    assert!(
        js.contains(r#"menu.setAttribute("hidden", "")"#),
        "关闭面板必须写回 hidden，不能进入 Tab 顺序"
    );
    assert!(
        js.contains("caret.focus()"),
        "Escape 收起后焦点应回到箭头按钮"
    );
    // 点空白不收起：全局 click 只处理窄屏抽屉，不再按 `.nav-group` 关闭导航。
    assert!(
        !js.contains(r#"target.closest("[data-nav-group]")"#),
        "点内容区/空白不得收起导航"
    );
    assert!(
        js.contains(r#"if (e.key !== "Escape") return;"#),
        "只有 Escape 才收起当前分组"
    );
}

/// 预览定稿的行结构、侧栏外观与二级通栏必须原样落到真实模板/样式。
#[test]
fn nav_markup_and_css_match_preview_contract() {
    let tpl = include_str!("../templates/base.html");
    assert!(
        tpl.contains(r#"<div class="nav-row">"#),
        "一级行 = 标签 + 箭头"
    );
    assert!(
        tpl.contains(r#"class="nav-link nav-label" data-nav-label"#),
        "标签承担跳转或展开，不吞掉箭头职责"
    );
    assert!(
        tpl.contains(r#"class="nav-caret" data-nav-toggle"#),
        "箭头必须是独立按钮"
    );
    assert!(
        tpl.contains(r#"aria-controls="nav-menu-{{ g.key }}""#),
        "箭头必须用 aria-controls 指向二级面板"
    );
    assert!(
        tpl.contains(r#"data-nav-menu hidden"#),
        "折叠的二级默认 hidden，不进入 Tab 顺序"
    );
    assert!(tpl.contains("data-subnav"), "上下布局需要独立二级通栏");
    // 侧栏三段：品牌 / 菜单滚动区 / 用户区都在同一顶栏内，顺序固定。
    let brand = tpl.find(r#"<div class="brand">"#).unwrap();
    let nav = tpl.find(r#"id="skiff-main-nav""#).unwrap();
    let who = tpl.find(r#"<div class="who">"#).unwrap();
    assert!(
        brand < nav && nav < who,
        "品牌区 / 菜单 / 用户区顺序必须稳定"
    );

    let css = include_str!("../static/skiff.css");
    assert!(
        css.contains(".subnav.open { height: 48px; }"),
        "二级通栏高度固定 48px"
    );
    assert!(
        css.contains(".subnav-inner .nav-menu {\n  flex-direction: row;"),
        "二级通栏必须横向排列，不能竖向堆叠"
    );
    // 标准属性只能待在 @supports 分支里；否则 Chrome 会忽略 6px 自绘规则（D6）。
    let supports = css
        .find("@supports not selector(::-webkit-scrollbar)")
        .expect("必须用 @supports 隔离 Firefox 的标准滚动条属性");
    let sidebar_nav = css
        .find("body[data-layout=\"left\"] .topbar .nav {")
        .expect("侧栏菜单区必须有显式规则");
    let sidebar_nav_end = sidebar_nav
        + css[sidebar_nav..]
            .find('}')
            .expect("侧栏菜单区规则必须闭合");
    assert!(
        !css[sidebar_nav..sidebar_nav_end].contains("scrollbar-width"),
        "侧栏基础规则不得设置 scrollbar-width，否则 Chrome 会退回 10px"
    );
    assert!(
        css[supports..].contains("scrollbar-width: thin")
            && css[supports..].contains("scrollbar-color:"),
        "Firefox 的 thin + scrollbar-color 必须放在 @supports 分支"
    );
    assert!(
        css.contains("::-webkit-scrollbar { width: 6px; }"),
        "侧栏必须自绘 6px 滚动条"
    );
    assert!(
        css.contains("html[data-theme=\"dark\"] body[data-layout=\"left\"] .topbar .nav"),
        "深色主题需单独配置滚动条配色"
    );
    assert!(
        css.contains("body[data-layout=\"left\"] .brand") && css.contains("flex: none"),
        "侧栏品牌区固定，不参与滚动"
    );
    assert!(
        css.contains("body[data-layout=\"left\"] .topbar .who"),
        "侧栏用户区固定"
    );
    assert!(
        css.contains("body[data-layout=\"left\"] .topbar .nav {\n  flex: 1 1 auto;"),
        "只有中间菜单区可滚动"
    );
    // 方案 A：连续导航带 + 单一强调标记；图标必须是内联 SVG，不再用字符拼符号。
    assert!(
        tpl.contains(r#"<svg class="icon-sprite""#)
            && tpl.contains(r#"<symbol id="i-overview""#)
            && tpl.contains(r##"<use href="#i-{{ g.icon }}"></use>"##),
        "导航图标必须来自内联 SVG 精灵"
    );
    assert!(
        !tpl.contains("menu-icon") && !css.contains(".menu-icon"),
        "不得保留字符图标方案"
    );
    assert!(
        css.contains("--nav-row-h: 44px;")
            && css.contains("--nav-icon: 18px;")
            && css.contains(".subnav.open { height: 48px; }"),
        "导航行高与图标尺寸必须按方案 A 落地"
    );
}

/// D1：上下布局把菜单搬进通栏后，展开状态必须按 `data-nav-key` 全局查找，不能再依赖分组父节点。
#[test]
fn d1_bar_layout_finds_menu_by_global_key() {
    let tpl = include_str!("../templates/base.html");
    let js = include_str!("../static/skiff.js");
    assert!(
        tpl.contains(r#"data-nav-key="{{ g.key }}""#),
        "每个二级菜单都要带 data-nav-key，脚本才能在搬家后找回它"
    );
    assert!(
        js.contains(r#"document.querySelectorAll("[data-nav-key]")"#),
        "菜单查找必须走全文档，而不是分组内部"
    );
    assert!(
        js.contains(r#"var menu = menuOf(group.dataset.menuKey || "");"#),
        "setOpen 必须用全局查找结果"
    );
    assert!(
        !js.contains(r#"group.querySelector("[data-nav-menu]")"#),
        "不得再按分组子树查找菜单（上下布局下菜单已被搬到 .subnav-inner）"
    );
    assert!(
        js.contains(r#"subnav.setAttribute("aria-hidden", show ? "false" : "true")"#),
        "通栏展开/收起必须同步 aria-hidden"
    );
    assert!(
        js.contains(r#"toggle.setAttribute("aria-expanded", open ? "true" : "false")"#),
        "箭头 aria-expanded 必须跟随展开状态"
    );
}

/// D2：侧栏 `.nav-menu{display:flex}` 权重高于通用 `[hidden]`，必须有显式收起规则。
#[test]
fn d2_sidebar_collapsed_menu_is_hidden() {
    let css = include_str!("../static/skiff.css");
    assert!(
        css.contains("body[data-layout=\"left\"] .nav-menu[hidden] { display: none; }"),
        "侧栏布局必须显式覆盖 display:flex，收起后二级菜单才真的不可见"
    );
}

/// D3：插件 iframe 要跟随面板主题；插件缺省深色，旧面板不下发主题时观感不变。
#[test]
fn d3_plugin_ui_adapts_to_theme_message() {
    let css = include_str!("../static/skiff.css");
    let js = include_str!("../static/skiff.js");
    let mount = include_str!("../templates/mount_card.html");
    assert!(
        css.contains("--plugin-frame: #f6f8fa;") && css.contains("--plugin-frame: #0b0d11;"),
        "父页面 iframe 底衬必须按浅/深主题切换"
    );
    assert!(
        js.contains(r#"postMessage({ type: "skiff.theme", theme: theme }, "*")"#),
        "父页面必须向插件 iframe 广播主题"
    );
    assert!(
        js.contains(r#"iframe.mount-frame, iframe.plugin-frame"#),
        "槽位 iframe 与插件详情 iframe 都要收到主题"
    );
    assert!(
        mount.contains(r#"class="mount-frame""#),
        "槽位卡片必须使用 mount-frame 类名，才能被主题广播选中"
    );
    assert!(
        js.contains(r#"msg.type !== "skiff.theme.request""#),
        "父页面必须响应插件主动索取主题，覆盖 iframe 先加载的时序"
    );
    assert!(
        js.contains(
            r#"window.addEventListener("load", function () { broadcastTheme(current()); });"#
        ),
        "父页面 window.load 后必须再次广播，避免 iframe 先加载时漏掉消息"
    );

    // 插件侧：页面 chrome 用变量，默认深色，收到 skiff.theme 才切浅色。
    for (name, html, js) in [
        (
            "guard-containers",
            include_str!("../../../plugins/guard/ui/containers.html"),
            include_str!("../../../plugins/guard/ui/app.js"),
        ),
        (
            "guard-events",
            include_str!("../../../plugins/guard/ui/events.html"),
            include_str!("../../../plugins/guard/ui/app.js"),
        ),
        (
            "cron-list",
            include_str!("../../../plugins/cron/ui/list.html"),
            include_str!("../../../plugins/cron/ui/app.js"),
        ),
        (
            "monitor-overview",
            include_str!("../../../plugins/monitor/ui/overview.html"),
            include_str!("../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-history",
            include_str!("../../../plugins/monitor/ui/history.html"),
            include_str!("../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-health",
            include_str!("../../../plugins/monitor/ui/health.html"),
            include_str!("../../../plugins/monitor/ui/health.js"),
        ),
        (
            "files",
            include_str!("../../../plugins/files/ui/main.html"),
            include_str!("../../../plugins/files/ui/app.js"),
        ),
        (
            "testkit-main",
            include_str!("../../../plugins/testkit/ui/main.html"),
            include_str!("../../../plugins/testkit/ui/app.js"),
        ),
        (
            "testkit-mount",
            include_str!("../../../plugins/testkit/ui/mount.html"),
            include_str!("../../../plugins/testkit/ui/mount.js"),
        ),
    ] {
        assert!(
            html.contains(r#"html[data-theme="light"]"#),
            "{name} 必须提供浅色主题变量覆盖"
        );
        assert!(
            html.contains("background: var(--ui-bg)"),
            "{name} 的页面底色必须走变量"
        );
        assert!(
            html.contains("color-scheme: dark;"),
            "{name} 默认必须保持深色，旧面板不下发主题时不破裂"
        );
        assert!(
            js.contains(r#"msg.type === "skiff.theme""#),
            "{name} 必须监听父页面的主题消息"
        );
        assert!(
            js.contains(r#"type: "skiff.theme.request""#),
            "{name} 必须在脚本执行与 load 时向父页面索取主题"
        );
        assert!(
            !html.contains("<h1") && !html.contains("data-plugin-title"),
            "{name} 不得渲染页面级标题（标题统一由内核提供）"
        );
    }

    // 工作台插件侧：上报内容高度，父页面才能消除内外双滚动。
    for (name, js) in [
        ("cron", include_str!("../../../plugins/cron/ui/app.js")),
        ("guard", include_str!("../../../plugins/guard/ui/app.js")),
        (
            "monitor",
            include_str!("../../../plugins/monitor/ui/app.js"),
        ),
        ("files", include_str!("../../../plugins/files/ui/app.js")),
        (
            "testkit",
            include_str!("../../../plugins/testkit/ui/app.js"),
        ),
    ] {
        assert!(
            js.contains(r#"type: "skiff.height""#) && js.contains("reportHeight"),
            "{name} 必须向父页面上报内容高度"
        );
    }
}

/// D3b：L3 横栏位于页面标题区下方并吸顶；内容宽度默认铺满；插件上报高度后 iframe 跟随。
#[test]
fn d3b_workbench_topbar_width_and_iframe_height() {
    let tpl = include_str!("../templates/plugin_view.html");
    let base = include_str!("../templates/base.html");
    let css = include_str!("../static/skiff.css");
    let js = include_str!("../static/skiff.js");

    // 横栏在页面标题区之后、内容面板之外；无 L3 声明时整条不渲染。
    let head = tpl.find("page-head").expect("页面标题区");
    let bar = tpl.find(r#"class="workbench-tabs""#).expect("L3 横栏");
    let panel = tpl
        .find(r#"class="panel workbench-panel""#)
        .expect("工作台内容面板");
    assert!(
        head < bar && bar < panel,
        "L3 横栏必须位于标题区下方、内容面板之外"
    );
    assert!(
        tpl.contains("{% if !tabs.is_empty() %}"),
        "无 L3 声明时整条横栏不得渲染"
    );
    assert!(
        tpl.contains("data-workbench-frame"),
        "工作台 iframe 必须带标记，高度消息只对它生效"
    );
    assert!(
        !tpl.contains("插件 UI 运行在无同源权限"),
        "工作台只保留标题 + L3 横栏，iframe 是纯内容区"
    );

    // 吸顶：两种布局都停在顶栏之下；上下布局用实测 --shell-h（含二级通栏）。
    assert!(
        css.contains(".workbench-tabs {\n  position: sticky;"),
        "L3 横栏必须 sticky，不随内容滚动"
    );
    assert!(
        css.contains("top: var(--shell-h, var(--topbar-h));"),
        "上下布局的吸顶偏移要落在顶栏/二级通栏之下"
    );
    assert!(
        js.contains(r#"root.style.setProperty("#) && js.contains("--shell-h"),
        "横栏吸顶偏移必须由脚本按顶栏实测高度写入"
    );
    assert!(
        js.contains("new ResizeObserver(syncShell).observe(shell)"),
        "顶栏高度变化（二级通栏展开、窄屏换行）必须自动重算"
    );

    // 内容宽度：默认铺满；固定时才限宽，侧栏布局在剩余空间居中。
    assert!(
        base.contains(r#"data-content-width="{{ ctx.menu.width }}""#)
            && base.contains("--content-max: {{ ctx.menu.max_width }}px"),
        "内容宽度偏好必须落到 body 属性与 CSS 变量"
    );
    assert!(
        css.contains(r#"body[data-content-width="fixed"] main"#),
        "只有「固定」模式才给内容区设上限"
    );
    assert!(
        !css.contains("max-width: 1240px"),
        "默认不得再给内容区固定 1240px 上限"
    );
    assert!(
        css.contains(r#"body[data-layout="left"][data-content-width="fixed"] main"#),
        "侧栏布局的固定宽度必须在侧栏之外的剩余空间居中"
    );

    // iframe 高度桥：只接受工作台 iframe，切 UI 时回落到默认高度。
    assert!(
        js.contains(r#"msg.type === "skiff.height""#),
        "父页面必须处理插件上报的内容高度"
    );
    assert!(
        js.contains(r#"frame.style.height = visual + "px""#),
        "父页面必须把 iframe 调到收敛后的高度（短内容贴合）"
    );
    assert!(
        js.contains("window.innerHeight - top - gap"),
        "长内容必须按一屏可用高度封顶（iframe 内部滚动，不撑出页面第二条滚动条）"
    );
    assert!(
        js.contains("FRAME_MIN_H"),
        "内容高度收敛必须有下限，避免极小空块"
    );
    assert!(
        js.contains(r#"frame.hasAttribute("data-workbench-frame")"#),
        "固定槽位 iframe 不允许被内容撑开"
    );
    assert!(
        js.contains(r#"frame.style.height = "";"#),
        "切换 UI 文件时高度必须回落到默认值"
    );
    assert!(
        css.contains("background: var(--plugin-frame);"),
        "iframe 加载占位底色必须跟随主题，避免白屏闪烁"
    );
}

/// D4：主题切换写回 localStorage，首屏脚本在样式表之前按同一键名落地，避免刷新闪白。
#[test]
fn d4_theme_is_persisted_before_first_paint() {
    let base = include_str!("../templates/base.html");
    let login = include_str!("../templates/login.html");
    let init = include_str!("../static/theme-init.js");
    let js = include_str!("../static/skiff.js");

    assert!(
        init.contains(r#"var KEY = "skiff.theme";"#),
        "首屏脚本必须使用与 skiff.js 相同的存储键名"
    );
    assert!(
        init.contains(r#"localStorage.getItem(KEY)"#),
        "首屏必须读取已保存的主题"
    );
    assert!(
        js.contains(r#"var THEME_KEY = "skiff.theme";"#),
        "切换脚本的键名必须与首屏脚本一致"
    );
    assert!(
        js.contains(r#"localStorage.setItem(THEME_KEY, dark ? "dark" : "light")"#)
            && js.contains(r#"apply(current() === "dark" ? "light" : "dark", true)"#),
        "点击切换必须写回 localStorage，刷新后才能保持"
    );

    for (name, tpl) in [("base", base), ("login", login)] {
        let script = tpl
            .find("static/theme-init.js")
            .unwrap_or_else(|| panic!("{name} 首屏必须加载 theme-init.js"));
        let sheet = tpl
            .find("static/skiff.css")
            .unwrap_or_else(|| panic!("{name} 必须加载主样式表"));
        assert!(
            script < sheet,
            "{name} 主题脚本必须排在样式表之前，才能避免暗色用户刷新时先闪白"
        );
        assert!(
            !tpl.contains(r#"src="/static/"#) && !tpl.contains(r#"href="/static/"#),
            "{name} 的静态资源必须带安全入口前缀"
        );
    }
}

/// 主题一致性：htmx 局部替换、bfcache 恢复后必须按 localStorage 重新 apply，
/// 避免出现「localStorage=light 但 html data-theme=dark」或按钮 aria 未同步。
#[test]
fn d4b_theme_resyncs_after_swap_and_bfcache() {
    let js = include_str!("../static/skiff.js");
    assert!(
        js.contains("function syncFromStorage"),
        "必须有统一的按存储值重新落地函数"
    );
    assert!(
        js.contains(r#"var theme = storedTheme() || current();"#),
        "重新落地时以 localStorage 为准"
    );
    assert!(
        js.contains(r#"window.addEventListener("pageshow", syncFromStorage)"#),
        "bfcache 前进/后退恢复时必须重新 apply"
    );
    assert!(
        js.contains(r#"document.body.addEventListener("htmx:afterSwap", boot)"#),
        "htmx 局部替换后必须走 boot 重新同步"
    );
    assert!(
        js.contains("// htmx 局部替换：重新同步按钮语义，并给新插入的 iframe 补发主题。"),
        "htmx 替换后的 else 分支也必须 syncFromStorage，而不是只广播"
    );
}

/// D5：网站列表操作列固定可见、内部不换行；窄屏取消吸附，改为整表横向滚动。
#[test]
fn d5_sites_actions_column_stays_visible() {
    let tpl = include_str!("../templates/sites.html");
    let css = include_str!("../static/skiff.css");
    assert!(
        tpl.contains(r#"<td class="actions-cell">"#),
        "操作单元格必须带 actions-cell 类"
    );
    assert!(
        tpl.contains(r#"<div class="actions row-actions">"#),
        "操作容器必须放在 td 内部，td 本身不能变成 flex"
    );
    assert!(
        !tpl.contains(r#"<td class="actions "#),
        "不得把 .actions 类直接套在 td 上（会破坏表格列宽）"
    );
    assert!(
        css.contains("table.grid td.actions-cell {")
            && css.contains("position: sticky;")
            && css.contains("right: 0;"),
        "操作列必须右侧吸附，横向滚动时仍可见"
    );
    assert!(
        css.contains("table.grid td.actions-cell > .actions {")
            && css.contains("flex-wrap: nowrap;"),
        "操作按钮不得竖排换行"
    );
    assert!(
        tpl.contains(r#"<table class="grid compact sites-grid">"#),
        "站点表格必须使用 compact + sites-grid（降低 padding 并固定列宽）"
    );
    assert!(
        !tpl.contains("site-domains-cell"),
        "域名列必须合并进站点列，否则 960 下仍然放不下"
    );
    assert!(
        tpl.contains(r#"<th>站点</th>"#) && tpl.contains(r#"class="sub mono site-root""#),
        "站点列必须同时承载域名与根目录"
    );
    assert!(
        css.contains("table.grid.sites-grid { table-layout: fixed; }"),
        "固定表格布局才能保证 960 内容区不被内容撑出横向滚动"
    );
    assert!(
        css.contains(".page-sites table.grid.sites-grid th.col-actions { width: 88px; }"),
        "操作列只剩「详情」，必须收窄固定宽度"
    );
    assert!(
        tpl.contains("类型 / 运行环境") && tpl.contains("s.runtime_name"),
        "类型与运行环境必须合并成一列，并显示运行环境实例名"
    );
    assert!(
        tpl.contains("alias_preview") && !tpl.contains("无别名"),
        "别名只在非空时渲染，不能保留「无别名」占位"
    );
    assert!(
        !tpl.contains("row-menu") && tpl.contains(">详情</a>"),
        "行内更多菜单必须移除，只留详情入口"
    );
    assert!(
        !tpl.contains("runtime_label"),
        "列表不得再拼「PHP 8.5」展示名，实例名已足够"
    );
    assert!(
        css.contains("@media (max-width: 1100px)")
            && css.contains("table.grid.sites-grid .site-root"),
        "≤1100px 必须隐藏根目录/别名行（详情页仍展示）"
    );
    assert!(
        css.contains("text-overflow: ellipsis;"),
        "次要列必须允许省略号收窄"
    );
    assert!(
        css.contains("@media (max-width: 640px)") && css.contains("position: static;"),
        "窄屏必须取消吸附，避免操作列吃掉过多宽度"
    );
}

/// D7：按钮文案显示目标态（深色时显示「浅色」），并由 CSS 跟随首屏主题。
#[test]
fn d7_theme_toggle_shows_target_theme() {
    let tpl = include_str!("../templates/base.html");
    let css = include_str!("../static/skiff.css");
    let js = include_str!("../static/skiff.js");
    assert!(
        tpl.contains(r#"<span class="theme-when-light">深色</span>"#)
            && tpl.contains(r#"<span class="theme-when-dark">浅色</span>"#),
        "按钮需要浅/深两份文案，首屏由 CSS 选一份"
    );
    assert!(
        css.contains(
            r#"html[data-theme="dark"] .theme-toggle .theme-when-light { display: none; }"#
        ) && css.contains(
            r#"html[data-theme="dark"] .theme-toggle .theme-when-dark { display: inline; }"#
        ),
        "深色主题下必须隐藏「深色」、显示「浅色」"
    );
    assert!(
        js.contains(
            r#"button.setAttribute("aria-label", dark ? "切换到浅色主题" : "切换到深色主题")"#
        ),
        "aria-label 必须与按钮当前目标态同步"
    );
    assert!(
        js.contains(r#"button.setAttribute("aria-pressed", dark ? "true" : "false")"#),
        "aria-pressed 必须表示当前是否深色（true=深色已启用）"
    );
    assert!(
        js.contains("function syncButton(theme)")
            && js.contains("syncButton(dark ? \"dark\" : \"light\");"),
        "每次 apply/重新同步都必须刷新按钮 aria，而不是只改一次"
    );
}

/// 新建网站改为单页弹窗：无三步向导、无二次确认；字段默认值、别名列表与
/// 目录前缀自动带上都在这里固化。
#[tokio::test]
async fn sites_new_dialog_defaults_and_validation() {
    let tpl = include_str!("../templates/site_new_dialog.html");
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    // 单屏结构：主域名与目录双列、类型面板占同一槽位、每个字段都有错误位。
    assert!(
        tpl.contains(r#"class="field-grid""#) && tpl.contains(r#"class="field-block type-panel""#),
        "主域名/目录应双列排布，类型专属字段共用面板槽位"
    );
    for name in [
        "domain",
        "aliases",
        "root_dir",
        "runtime_id",
        "proxy_target",
    ] {
        assert!(
            tpl.contains(&format!(r#"data-error-for="{name}""#)),
            "{name} 必须有字段级错误位"
        );
    }
    assert!(tpl.contains("data-row-error"), "别名行必须有逐行错误位");
    assert!(
        tpl.contains(r#"class="req""#),
        "必填字段必须有可见的必填标识"
    );

    let (status, html) = get_page(&h.app, "/p/sites?new=1", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("data-site-dialog"),
        "必须用弹窗承载新建: {html}"
    );
    assert!(
        html.contains(r#"data-site-dialog open"#),
        "?new=1 应直接展开弹窗: {html}"
    );
    assert!(!html.contains("data-wizard"), "不得再三步向导");
    assert!(!html.contains("确认创建"), "不得有二次确认步骤");
    assert!(
        html.contains(r#"name="site_type""#)
            && html.contains("value=\"runtime\"")
            && html.contains("value=\"static\"")
            && html.contains("value=\"proxy\""),
        "三类选择必须在弹窗内: {html}"
    );
    assert!(
        html.contains("data-domain-list")
            && html.contains("data-alias-add")
            && html.contains("data-alias-bulk")
            && html.contains(r#"name="domain""#)
            && html.contains(r#"name="aliases_bulk""#),
        "主域名/别名必须是同一个列表，且支持新增与批量粘贴: {html}"
    );
    // 默认只有主域名一行；别名行由「新增/批量粘贴」产生。
    assert!(
        !html.contains("data-alias-row"),
        "别名列表默认不应带空行: {html}"
    );
    // 弹窗默认值：运行环境类型预选；无就绪实例时给引导，不渲染空下拉。
    let runtime_pos = html.find(r#"value="runtime""#).unwrap();
    let runtime_tag: String = html[runtime_pos..].chars().take(80).collect();
    assert!(runtime_tag.contains("checked"), "默认应选中运行环境类型");
    assert!(
        html.contains("没有已就绪的运行环境") && html.contains("前往运行环境"),
        "无就绪运行环境时必须给出引导而不是空下拉: {html}"
    );
    assert!(
        html.contains("data-domain-main") && html.contains("data-root-slug"),
        "主域名与目录输入必须带前端钩子: {html}"
    );
    assert!(
        html.contains("/www/sites/") && html.contains("默认与主域名同名"),
        "目录前缀必须自动带上，默认跟随主域名: {html}"
    );
    assert!(
        !html.contains("必须以 /www/sites/"),
        "前端不得暴露前缀校验文案: {html}"
    );
    assert!(
        !html.contains("data-disabled"),
        "未就绪的运行环境必须整项不出现，而不是禁用占位: {html}"
    );
    assert!(
        !html.contains("name=\"database\"") && !html.contains("name=\"ssl\""),
        "不得用可点击的假开关冒充数据库/SSL"
    );

    let js = include_str!("../static/skiff.js");
    assert!(
        js.contains("function attachSiteDialog")
            && js.contains("function syncRoot")
            && js.contains("slugTouched"),
        "前端必须有目录自动填主域名的逻辑"
    );
    assert!(
        js.contains("data-alias-import") && js.contains("splitNames"),
        "批量粘贴必须在前端解析成别名行"
    );
    let csrf = csrf_from(&html);

    // 非法域名：服务端就地报错，弹窗保持展开并回填。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/create")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("site_type", "runtime"),
                    ("runtime_id", "php-8.5"),
                    ("domain", "bad_domain"),
                    ("root_dir", "bad"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "校验失败应留在列表页");
    let html = body_text(resp).await;
    assert!(
        html.contains("域名只能包含") && html.contains("data-site-dialog"),
        "非法域名必须就地报错并保留弹窗: {html}"
    );
    // 服务端错误要落到字段旁：错误位可见 + 输入框高亮，而不是只在顶部飘一条。
    assert!(
        html.contains(r#"data-error-for="domain" data-has-error"#),
        "域名错误必须显示在字段旁: {html}"
    );
    assert!(
        html.contains(r#"data-has-error aria-invalid="true""#),
        "出错字段必须高亮并标记 aria-invalid: {html}"
    );
    assert!(!html.contains("确认创建"), "错误回填也不得引入二次确认");

    let js = include_str!("../static/skiff.js");
    assert!(
        js.contains("function rowError") && js.contains("data-row-error"),
        "别名必须支持逐行错误，批量重复要逐条指出"
    );
    assert!(
        js.contains("已存在，未导入") && js.contains("域名重复："),
        "批量粘贴必须逐条列出被跳过的域名"
    );
    assert!(
        js.contains("创建中…") && js.contains("submitBtn.disabled = true"),
        "提交必须进入禁用态，避免连点重复创建"
    );
}

/// 别名批量解析：逗号/空格/换行混合提交能被拆分，重复域名明确拒绝。
#[tokio::test]
async fn sites_create_rejects_duplicate_aliases() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (_, html) = get_page(&h.app, "/p/sites?new=1", &admin).await;
    let csrf = csrf_from(&html);

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/create")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("site_type", "static"),
                    ("domain", "dup.local"),
                    ("aliases", "www.dup.local, WWW.dup.local"),
                    ("root_dir", ""),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;
    assert!(
        html.contains("域名重复"),
        "重复别名必须被拒绝而不是静默去重: {html}"
    );

    // 别名等于主域名同样拒绝。
    let (_, html) = get_page(&h.app, "/p/sites?new=1", &admin).await;
    let csrf = csrf_from(&html);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/create")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("site_type", "static"),
                    ("domain", "same.local"),
                    ("aliases", "same.local"),
                    ("root_dir", ""),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    let html = body_text(resp).await;
    assert!(
        html.contains("别名不能与主域名相同"),
        "别名与主域名相同时必须拒绝: {html}"
    );
}

/// 站点详情「域名」标签：主域名单独标识，别名可删除，新增入口落库并重建 vhost。
#[tokio::test]
async fn site_domains_tab_manages_aliases() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/dom.local").display().to_string();
    h.state
        .db()
        .create_site(&static_site("dom.local", &root))
        .unwrap();
    h.state
        .db()
        .add_site_aliases("dom.local", &["www.dom.local".to_string()])
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites/dom.local?tab=domains", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("主域名"), "{html}");
    assert!(html.contains("www.dom.local"), "{html}");
    assert!(
        html.contains("/p/sites/dom.local/domains")
            && html.contains(r#"name="action" value="remove""#)
            && html.contains(r#"name="action" value="add""#),
        "域名标签必须能增删别名: {html}"
    );
    let csrf = csrf_from(&html);

    // 主域名不能作为别名删除。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/dom.local/domains")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("action", "remove"),
                    ("alias", "dom.local"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        !loc.contains("error=") && !loc.contains("notice="),
        "结果不得走 query: {loc}"
    );
    let (_, page) = get_page(&h.app, &loc, &admin).await;
    assert!(page.contains("主域名不能作为别名删除"), "{page}");
}

/// 运行环境页是业务视角：列表/详情/深链可访问，viewer 一律 403。
#[tokio::test]
async fn runtime_pages_are_admin_only_and_render_business_view() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    for uri in ["/p/runtimes", "/p/sites/env", "/p/sites/env/php-8.5"] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains("PHP"), "{uri} 必须展示运行环境类型: {html}");
    }
    let (status, html) = get_page(&h.app, "/p/sites/env/php-8.5", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("PHP 配置"), "{html}");
    assert!(html.contains("socket"), "{html}");
    assert!(html.contains("关联站点"), "{html}");
    assert!(
        html.contains("/p/containers/"),
        "运行环境详情必须提供 Docker 页深链: {html}"
    );

    // 菜单契约深链同源。
    let (status, html) = get_page(&h.app, "/p/m/core/sites/env/php-8.5", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("PHP 配置"), "{html}");

    for uri in ["/p/runtimes", "/p/sites/env", "/p/sites/env/php-8.5"] {
        let (status, _) = get_page(&h.app, uri, &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 必须拒绝 viewer");
    }

    // viewer 的写操作同样 403，且不产生副作用。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/env/php-8.5/action/start")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(form_body(&[("csrf", "x")]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// 非 PHP 实例（Rust/Node/Go）的详情页不得出现任何 PHP 专属区块：
/// 扩展页签、扩展面板、PHP 配置文件编辑；直达 env 页与规范入口都要判 kind。
#[tokio::test]
async fn runtime_detail_hides_php_blocks_for_non_php_instances() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    h.state
        .db()
        .create_runtime(&skiff_store::NewRuntime {
            id: "rust-1.83".into(),
            kind: "rust".into(),
            name: "rust-1.83".into(),
            version: "1.83".into(),
            image: "rust:1.83-slim".into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();

    for uri in ["/p/sites/env/rust-1.83", "/p/runtimes/rust-1.83"] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            !html.contains(r#"data-runtime-tab="extensions""#)
                && !html.contains(r#"data-runtime-tab-panel="extensions""#),
            "{uri} 不得渲染扩展页签/面板: {html}"
        );
        assert!(
            !html.contains("扩展管理") && !html.contains("php.ini") && !html.contains("PECL"),
            "{uri} 不得出现 PHP 专属配置: {html}"
        );
        assert!(
            html.contains("Rust") && html.contains("rust-1.83"),
            "{uri} 仍应展示实例自身信息: {html}"
        );
    }

    // 同一页面在 PHP 实例上仍完整保留扩展与 PHP 配置。
    let (_, php_html) = get_page(&h.app, "/p/sites/env/php-8.5", &admin).await;
    assert!(
        php_html.contains(r#"data-runtime-tab="extensions""#) && php_html.contains("php.ini"),
        "PHP 实例必须保留扩展页签与 php.ini 查看入口: {php_html}"
    );
}

/// 运行环境页：去安装进度/镜像源，列表 + 新建实例 + 详情抽屉（Esc/遮罩关闭）。
#[tokio::test]
async fn site_env_page_drops_install_progress_and_uses_drawer() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    // 一个真实实例（未安装也算已存在的实例，应进列表）。
    h.state
        .db()
        .create_runtime(&skiff_store::NewRuntime {
            id: "php-8.5-a".into(),
            kind: "php".into(),
            name: "php-8.5-a".into(),
            version: "8.5".into(),
            image: "php:8.5-fpm-alpine".into(),
            image_digest: String::new(),
            socket_path: Some("/www/runtime/php/php-8.5-a/php-fpm.sock".into()),
            port: None,
            is_default: false,
        })
        .unwrap();
    let (status, html) = get_page(&h.app, "/p/sites/env", &admin).await;
    assert_eq!(status, StatusCode::OK);

    for gone in [
        "安装进度",
        "镜像源",
        "下载速度",
        "registry_mirror",
        "image_prefix",
    ] {
        assert!(!html.contains(gone), "运行环境页不得常驻 {gone}: {html}");
    }
    for keep in [
        "类型",
        "版本",
        "占用站点",
        "接入方式",
        "详情",
        "新建运行环境",
        "创建并后台安装",
    ] {
        assert!(html.contains(keep), "列表/弹窗必须展示 {keep}: {html}");
    }
    // 「详情」和「更多」合并成一个入口：每行只留一个按钮，整行也可点开抽屉。
    assert!(
        !html.contains(">更多<"),
        "详情与更多必须合并成单一入口: {html}"
    );
    // 接入方式只显示短标识，完整路径在 title 悬浮提示里（宿主路径在不同平台上不同）。
    assert!(
        html.contains("runtime-endpoint")
            && html.contains(r#"title="socket "#)
            && html.contains(">sock<"),
        "接入方式列必须显示 sock 且完整路径进悬浮提示: {html}"
    );
    assert!(html.contains("共 1 个实例"), "列表计数只算真实实例: {html}");
    assert!(
        !html.contains(r#"data-runtime-open="/p/sites/env/php-8.3""#)
            && !html.contains(r#"data-runtime-open="/p/sites/env/php-8.4""#)
            && !html.contains(r#"data-runtime-open="/p/sites/env/php-8.5""#),
        "迁移种子行不得在列表占位（模板只在新建流程出现）: {html}"
    );
    assert!(
        html.contains(r#"data-runtime-drawer"#)
            && html.contains(r#"data-runtime-open="/p/sites/env/php-8.5-a""#)
            && html.contains("data-drawer-content"),
        "详情必须是右侧抽屉而不是跳页: {html}"
    );
    assert!(
        html.contains(r#"name="kind""#)
            && html.contains(r#"name="name""#)
            && html.contains(r#"name="version""#)
            && html.contains("data-runtime-version")
            && html.contains("目录已存在会被拒绝"),
        "新建弹窗必须暴露 kind/name/version 并提示目录拒绝规则: {html}"
    );
    // 版本下拉来自 catalog（PHP 8.5/8.4/8.3），不允许手填。
    assert!(
        html.contains(r#"<option value="8.5""#)
            && html.contains(r#"<option value="8.4""#)
            && html.contains(r#"<option value="8.3""#)
            && html.contains("data-runtime-catalog"),
        "版本必须是目录驱动的下拉: {html}"
    );
    // 扩展清单单一来源渲染：安装弹窗与抽屉共用同一批 key。
    for key in ["opcache", "redis", "pdo_mysql", "gd", "intl"] {
        assert!(
            html.contains(&format!(r#"value="{key}""#)),
            "安装弹窗必须列出扩展 {key}: {html}"
        );
    }
    // 版本切换必须按该版本重算扩展可用性：8.4 起 imap/pspell 不再出现（不灰显）。
    let raw = html
        .split(r#"data-runtime-catalog>"#)
        .nth(1)
        .and_then(|s| s.split("</script>").next())
        .expect("运行环境目录 JSON");
    let catalog: serde_json::Value = serde_json::from_str(raw.trim()).expect("目录 JSON 可解析");
    let php = catalog
        .as_array()
        .and_then(|a| a.iter().find(|k| k["key"] == "php"))
        .expect("目录必须含 php");
    let by_ver = php["exts_by_version"].as_object().expect("php 扩展矩阵");
    for (ver, expect_present) in [("8.5", false), ("8.4", false), ("8.3", true)] {
        let list = by_ver[ver].as_array().expect("版本扩展列表");
        for key in ["imap", "pspell"] {
            let found = list.iter().any(|e| e["key"] == key);
            assert_eq!(
                found, expect_present,
                "{ver} 的 {key} 是否出现在可用清单必须为 {expect_present}"
            );
        }
    }
    assert!(
        include_str!("../static/skiff.js").contains("function rebuildExts"),
        "新建弹窗切换版本时必须重建扩展勾选项"
    );
    assert!(
        html.contains(r#"class="runtime-extensions exts-box""#),
        "弹窗必须有扩展勾选块: {html}"
    );
    // 「基础环境」概念已取消：入口、文案、次要表单全部不出现。
    assert!(
        !html.contains("基础环境") && !html.contains("runtime-base-install"),
        "运行环境页不得再出现基础环境概念: {html}"
    );
    let head_block = html
        .split(r#"<div class="page-head">"#)
        .nth(1)
        .and_then(|s| s.split("</div>").next())
        .unwrap_or("");
    assert!(
        !head_block.contains("/sites/env/install"),
        "标题栏不得再放基础环境按钮: {head_block}"
    );
    // Web 服务不再是运行环境行：运维入口移到 Web 服务页。
    assert!(
        !html.contains("data-web-action")
            && !html.contains("/sites/env/web/action")
            && !include_str!("../static/skiff.js").contains("data-web-action"),
        "Web 服务不得再出现在运行环境页: {html}"
    );
    let (status, web) = get_page(&h.app, "/p/sites/web", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        web.contains("Web 服务（OpenResty）")
            && web.contains("data-web-env-ops")
            && web.contains("/p/sites/env/web/action/"),
        "Web 服务运维入口必须落在 Web 服务页: {web}"
    );

    // 抽屉交互由原生 <dialog> 承担：showModal 自动焦点锁 + Esc，遮罩点击关闭。
    let js = include_str!("../static/skiff.js");
    assert!(js.contains("dlg.showModal()"), "抽屉必须用 showModal: {js}");
    assert!(
        js.contains("e.target === dlg") && js.contains("dlg.close()"),
        "点遮罩必须关闭抽屉: {js}"
    );
    let css = include_str!("../static/skiff.css");
    assert!(css.contains("dialog.drawer") && css.contains("dialog.drawer::backdrop"));
    assert!(
        css.contains("@media (max-width: 720px)") && css.contains("dialog.drawer"),
        "窄屏必须退化为全屏抽屉"
    );
}

/// 站点级设置页已移除：镜像加速源/前缀归 Docker 设置，镜像标签归运行环境页。
#[tokio::test]
async fn site_settings_removed_and_docker_settings_hosts_mirror() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    // 旧入口与页面都不再存在。
    let (status, _) = get_page(&h.app, "/p/sites/settings", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "站点设置页必须已移除");

    // 新 Docker 设置页：镜像加速源 + 镜像前缀；菜单深链同源。
    for uri in ["/p/docker/settings", "/p/m/core/docker/settings"] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            html.contains("镜像加速源")
                && html.contains(r#"name="registry_mirror""#)
                && html.contains(r#"name="image_prefix""#),
            "{uri} 必须承载镜像加速源与镜像前缀: {html}"
        );
    }

    // 运行环境页接管镜像标签，不再出现加速源/前缀字段。
    let (status, env_html) = get_page(&h.app, "/p/sites/env", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        env_html.contains(r#"name="nginx_image""#)
            && env_html.contains(r#"name="php_image""#)
            && env_html.contains("/p/sites/env/images"),
        "运行环境页必须提供镜像标签保存入口: {env_html}"
    );

    // viewer 打开页面只读，保存仍被拒绝。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, html) = get_page(&h.app, "/p/docker/settings", &viewer).await;
    assert_eq!(status, StatusCode::OK);
    let csrf = csrf_from(&html);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/docker/settings")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("registry_mirror", "https://mirror.example.com"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Docker 三个入口的页面骨架：创建容器（admin-only）、Compose 页（viewer 只读）。
#[tokio::test]
async fn docker_create_and_compose_entries_follow_the_ui_contract() {
    // 创建/更新容器属于生命周期档，只有宿主机直装形态开放。
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    // 受限形态（Tier::Basic）连 admin 也不能创建，能力面收缩由策略层统一拒绝。
    let basic = harness();
    let basic_admin = login(&basic.app, "admin", "pw-test-123").await;
    let (status, _) = get_page(&basic.app, "/p/containers/new", &basic_admin).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "Basic 档必须拒绝创建容器");

    let (status, html) = get_page(&h.app, "/p/containers/new", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"name="image""#)
            && html.contains(r#"name="ports""#)
            && html.contains(r#"name="env""#)
            && html.contains(r#"name="volumes""#)
            && html.contains(r#"name="restart""#),
        "创建容器页必须覆盖镜像/端口/环境变量/卷/重启策略: {html}"
    );
    // 页头右上主操作通过 form= 关联表单，满足「一页一个主操作」。
    assert!(
        html.contains(r#"form="container-form""#),
        "创建容器主操作必须在页头并关联表单: {html}"
    );

    let (status, _) = get_page(&h.app, "/p/containers/new", &viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "viewer 不得进入创建页");

    let (status, html) = get_page(&h.app, "/p/compose", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"name="compose""#)
            && html.contains(r#"name="project""#)
            && html.contains("/p/compose/validate")
            && html.contains("/p/compose/up"),
        "Compose 页必须提供项目名、文件内容、校验与部署动作: {html}"
    );
    assert!(
        html.contains("com.docker.compose") || html.contains("容器标签"),
        "Compose 页必须说明项目归属来自容器标签: {html}"
    );

    // viewer 只读：页面可看，但不得出现部署/删除按钮。
    let (status, viewer_html) = get_page(&h.app, "/p/compose", &viewer).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !viewer_html.contains("/p/compose/up") && !viewer_html.contains("/p/compose/down"),
        "viewer 不得看到 Compose 写操作: {viewer_html}"
    );
}

/// 新建 PHP 实例：名字非法 / 同名 / 目录已存在都在落库前拒绝。
#[tokio::test]
async fn site_env_add_rejects_invalid_duplicate_and_existing_dir() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (_, html) = get_page(&h.app, "/p/sites/env", &admin).await;
    let csrf = csrf_from(&html);

    for (name, version) in [("PHP-8.5", "8.5"), ("-php", "8.5"), ("php-8.5-a", "")] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/p/sites/env/add")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                    .body(form_body(&[
                        ("csrf", &csrf),
                        ("name", name),
                        ("version", version),
                    ]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{name}/{version}");
        let location = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            location, "/p/sites/env",
            "{name}/{version} 回跳不得携带消息参数"
        );
    }

    // 迁移种子行可被同名新建接管：不再以「重名」拒绝，创建后即真实实例。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/env/add")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("name", "php-8.5"),
                    ("version", "8.5"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(location, "/p/sites/env", "回跳不得携带消息参数");
    // 接管后已是真实实例，再次同名必须拒绝。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/env/add")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("name", "php-8.5"),
                    ("version", "8.5"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(location, "/p/sites/env", "回跳不得携带消息参数");
    let (_, page) = get_page(&h.app, "/p/sites/env", &admin).await;
    assert!(page.contains("已存在"), "重名原因应经 flash 回显: {page}");

    // 目标目录已存在：直接拒绝，不覆盖、不自动加后缀。
    std::fs::create_dir_all(h.data_dir.join("www/runtime/php/php-8.4-a")).unwrap();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/env/add")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    ("name", "php-8.4-a"),
                    ("version", "8.4"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(location, "/p/sites/env", "回跳不得携带消息参数");
    let (_, page) = get_page(&h.app, "/p/sites/env", &admin).await;
    assert!(
        page.contains("已存在"),
        "目录冲突原因应经 flash 回显: {page}"
    );
    assert!(
        h.state
            .db()
            .runtime_by_name("php", "php-8.4-a")
            .unwrap()
            .is_none(),
        "目录冲突时不得落库"
    );
}

/// 抽屉扩展管理 + 列表轮询端点：两处扩展清单同源，JSON 端点不被 `{id}` 路由吞掉。
#[tokio::test]
async fn runtime_drawer_shares_extension_catalog_and_exposes_jobs_json() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_page(&h.app, "/p/sites/env/php-8.5", &admin).await;
    assert_eq!(status, StatusCode::OK, "抽屉内容页应 200");
    assert!(
        html.contains("data-runtime-tabs") && html.contains(r#"data-runtime-tab="extensions""#),
        "抽屉必须有页签并包含扩展页签: {html}"
    );
    assert!(
        html.contains(r#"data-ext-search"#) && html.contains(r#"data-ext-cat"#),
        "扩展页签必须有搜索与分类过滤: {html}"
    );
    // 无任务时抽屉不渲染任务面板（有任务时才有 data-runtime-job / data-job-log）。
    assert!(
        html.contains("data-runtime-panel") && !html.contains("data-job-url"),
        "空闲运行环境不应显示安装任务面板: {html}"
    );
    for key in [
        "redis",
        "pdo_mysql",
        "mysqli",
        "gd",
        "zip",
        "bcmath",
        "intl",
    ] {
        assert!(
            html.contains(&format!(r#"data-search="{key} "#))
                && html.contains(&format!(r#"value="{key}""#)),
            "抽屉扩展清单缺少 {key} 的勾选项（必须与安装弹窗同源）: {html}"
        );
    }
    // 多选安装/卸载：一份表单 + 二次确认，后端接收批量 `op` + 多个 `ext`。
    assert!(
        html.contains(r#"data-ext-batch-form"#)
            && html.contains(r#"action="/p/sites/env/php-8.5/extensions-batch""#)
            && html.contains(r#"name="ext""#)
            && html.contains(r#"data-ext-submit="install""#)
            && html.contains(r#"data-ext-submit="uninstall""#)
            && html.contains(r#"data-ext-confirm"#)
            && html.contains("确认提交"),
        "扩展页签必须是多选 + 二次确认: {html}"
    );
    // 镜像内置项（opcache / curl / mbstring / sodium / sqlite3 / pdo_sqlite）直接显示已安装，
    // 不提供安装/编译入口，避免重复编译与重复启用。
    for key in [
        "opcache",
        "curl",
        "mbstring",
        "sodium",
        "sqlite3",
        "pdo_sqlite",
    ] {
        assert!(
            html.contains(&format!(r#"data-search="{key} "#))
                && html.contains(&format!(r#"value="{key}""#))
                && !html.contains(&format!(r#"/extensions/{key}/install"#)),
            "{key} 为镜像内置，不应出现单独安装入口: {html}"
        );
    }
    assert!(
        html.contains(r#"data-ext-builtin="true""#),
        "内置扩展必须带 data-ext-builtin=true 供确认弹窗过滤: {html}"
    );
    assert!(
        html.contains(r#"/pecl""#) && html.contains(r#"name="name""#),
        "扩展页签必须提供 PECL 按名字安装入口: {html}"
    );
    // 8.4 起 imap 移出核心：8.5 环境里直接不显示（不灰显、不占位、无原因文案）。
    assert!(
        !html.contains(r#"data-search="imap "#)
            && !html.contains(r#"value="imap""#)
            && !html.contains("当前不可用")
            && !html.contains("已从核心移除"),
        "imap 在 PHP 8.5 应完全不显示且不出现灰显/原因文案: {html}"
    );

    // 列表轮询端点：空任务也必须返回合法 JSON，且路由优先于 `/sites/env/{id}`。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/sites/env/jobs")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .header("x-requested-with", "fetch")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "任务端点应 200");
    let body = body_text(resp).await;
    assert!(
        body.contains("\"jobs\""),
        "任务端点必须返回结构化 JSON: {body}"
    );
}

/// 绑定运行环境停止时：站点详情明确 502/503 并给出回环境页的入口，不静默换版本。
#[tokio::test]
async fn site_detail_warns_when_bound_runtime_stopped() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    h.state
        .db()
        .set_runtime_status("php-8.5", "running", "stopped")
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-runtime-down"), "{html}");
    assert!(
        html.contains("502/503") && html.contains("不会静默切换"),
        "必须说明停止版本的后果: {html}"
    );
    assert!(
        html.contains(r#"href="/p/sites/env/php-8.5""#),
        "必须提供回运行环境页的入口: {html}"
    );
}

/// Docker 不可达时，运行环境站点不能谎称「运行中」：列表、详情、环境列表都要显示未知。
#[tokio::test]
async fn runtime_sites_show_unknown_instead_of_running_when_docker_unavailable() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();

    let (status, list_html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list_html.contains("状态未知（Docker 不可用）"),
        "站点列表必须把 Docker 不可达标成未知，而不是运行中: {list_html}"
    );

    let (status, detail_html) = get_page(&h.app, "/p/sites/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        detail_html.contains("状态未知（Docker 不可用）"),
        "站点详情必须与列表一致: {detail_html}"
    );
    assert!(
        detail_html.contains("data-runtime-down"),
        "Docker 未知时也要给出不可用告警: {detail_html}"
    );

    let (status, env_html) = get_page(&h.app, "/p/sites/env", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        env_html.contains("状态未知"),
        "运行环境列表必须区分「未知」与「未安装」: {env_html}"
    );
}

/// 「网站 → Web 服务」：四区块常驻渲染；管理员 + CSRF，viewer 403。
#[tokio::test]
async fn nginx_page_renders_four_sections_and_is_admin_only() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    for uri in [
        "/p/sites/web",
        "/p/sites/web?tab=config",
        "/p/sites/web?tab=logs",
        "/p/sites/web?tab=modules",
        "/p/m/core/sites/web",
    ] {
        let (status, _) = get_page(&h.app, uri, &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 必须拒绝 viewer");
    }

    let (status, html) = get_page(&h.app, "/p/sites/web", &admin).await;
    assert_eq!(status, StatusCode::OK);
    for panel in ["status", "config", "logs", "modules"] {
        assert!(
            html.contains(&format!(r#"data-nginx-panel="{panel}""#)),
            "缺少 Web 服务 {panel} 区块: {html}"
        );
    }
    // 复用插件工作台横栏标记；四个标签走页内切换（hash 定位）。
    assert!(
        html.contains(r#"class="workbench-tabs""#)
            && html.contains("data-nginx-tabs")
            && html.contains(r#"data-nginx-tab="status""#)
            && html.contains(r#"data-nginx-tab="config""#)
            && html.contains(r#"data-nginx-tab="log""#)
            && html.contains(r#"data-nginx-tab="modules""#),
        "必须复用 workbench-tabs 标记并给出四个标签: {html}"
    );
    for hash in ["#status", "#config", "#log", "#modules"] {
        assert!(
            html.contains(&format!(r#"href="{hash}""#)),
            "标签必须同步到 URL hash {hash}: {html}"
        );
    }
    // 默认只显示 status，其余区块 hidden；切标签由 JS + hash 控制。
    assert!(
        html.contains(r#"data-nginx-panel="config" hidden"#)
            && html.contains(r#"data-nginx-panel="logs" hidden"#)
            && html.contains(r#"data-nginx-panel="modules" hidden"#),
        "非当前标签必须 hidden，避免四块堆一页: {html}"
    );

    let (status, config_tab) = get_page(&h.app, "/p/sites/web?tab=config", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        config_tab.contains(r#"data-nginx-panel="config""#)
            && !config_tab.contains(r#"data-nginx-panel="config" hidden"#),
        "?tab=config 必须直接停在配置区块（无 hash 时的服务端兜底）"
    );

    assert!(
        html.contains("active connections")
            && html.contains("accepts")
            && html.contains("waiting")
            && html.contains("运行时长")
            && html.contains("最近 reload"),
        "状态必须展示 stub_status 四项 + 版本/时长/reload: {html}"
    );
    assert!(
        html.contains("探测地址")
            && html.contains("http://127.0.0.1:8081/nginx_status")
            && html.contains("主配置含 stub_status"),
        "状态不可得时必须给出探测地址与主配置诊断: {html}"
    );
    assert!(
        html.contains("性能参数")
            && html.contains("worker_connections")
            && html.contains("client_max_body_size"),
        "配置区块必须提供性能参数: {html}"
    );
    // 日志默认不加载：先给「选择日志 → 选择范围」空态，筛选条件齐全但不读文件。
    assert!(
        html.contains("data-log-empty")
            && html.contains("选择日志 → 选择范围")
            && html.contains(r#"name="type""#)
            && html.contains(r#"name="lines""#)
            && html.contains(r#"name="status""#)
            && html.contains(r#"name="level""#)
            && html.contains(r#"name="load""#)
            && !html.contains("data-log-result"),
        "日志区块必须先给空态、不自动读取，并保留类型/范围/关键字/级别筛选: {html}"
    );
    let (_, loaded) = get_page(
        &h.app,
        "/p/sites/web?tab=logs&load=1&type=access&lines=200",
        &admin,
    )
    .await;
    assert!(
        loaded.contains("data-log-result")
            && loaded.contains("本次加载")
            && loaded.contains("已扫描尾部")
            && !loaded.contains("data-log-empty"),
        "点「加载」后只按条件取尾部并标注读取范围: {loaded}"
    );
    assert!(
        html.contains("编译参数") && html.contains("模块列表"),
        "模块区块必须展示 nginx -V 解析结果: {html}"
    );
    assert!(
        html.contains("校验并重载"),
        "主配置保存必须走 nginx -t + reload: {html}"
    );

    // JS：标签页内切换 + hash 同步 + 刷新后停在原标签；CSS 只加页面私有命名空间。
    let js = include_str!("../static/skiff.js");
    assert!(
        js.contains("attachNginxTabs")
            && js.contains("location.hash")
            && js.contains("history.replaceState")
            && js.contains("data-nginx-panel"),
        "缺少 Web 服务标签切换/hash 同步逻辑: {js}"
    );
    let css = include_str!("../static/skiff.css");
    assert!(
        css.contains(".page-nginx .log-empty") && css.contains(".page-nginx .log-card"),
        "日志空态与结果卡必须落在页面私有命名空间"
    );

    // 视图级权限：viewer 不能打开，管理员缺 CSRF 也不能保存。
    let csrf = csrf_from(&html);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/web/config/main")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", "bad-token"),
                    ("content", "events {}"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // 合法 CSRF 时进入回滚路径（无 Docker → 保存失败），必须带回错误并保留本次编辑内容。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/web/config/main")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[("csrf", &csrf), ("content", "events {}")]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "失败应原地回显配置页，不再跳转"
    );
    let html = body_text(resp).await;
    assert!(
        html.contains("events {}"),
        "校验失败必须保留用户正在编辑的内容: {html}"
    );
    assert!(
        h.state
            .db()
            .recent_audit(10)
            .unwrap()
            .iter()
            .any(|a| a.action == "nginx.config.save"),
        "保存动作必须写审计"
    );
}

/// 三类站点在列表/详情中的类型与端点展示不混淆。
#[tokio::test]
async fn three_site_types_are_rendered_in_list() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let php_root = h.data_dir.join("www/sites/php.local");
    let static_root = h.data_dir.join("www/sites/static.local");
    let proxy_root = h.data_dir.join("www/sites/proxy.local");
    h.state
        .db()
        .create_site(&php_site("php.local", &php_root.display().to_string()))
        .unwrap();
    h.state
        .db()
        .create_site(&static_site(
            "static.local",
            &static_root.display().to_string(),
        ))
        .unwrap();
    h.state
        .db()
        .create_site(&proxy_site(
            "proxy.local",
            &proxy_root.display().to_string(),
            "http://127.0.0.1:8080",
            true,
        ))
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("类型 / 运行环境"),
        "类型与运行环境合并成一列: {html}"
    );
    assert!(
        html.contains(r#"<span class="mono">php-8.5</span>"#),
        "运行环境站必须显示实例名 php-8.5: {html}"
    );
    assert!(
        html.contains(r#"<span class="muted">静态</span>"#),
        "静态站必须显示「静态」: {html}"
    );
    assert!(
        html.contains(r#"<span class="mono">http://127.0.0.1:8080</span>"#)
            && html.contains(">WS</span>"),
        "反代必须展示上游与 WS 标记: {html}"
    );

    let (status, html) = get_page(&h.app, "/p/sites/proxy.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("上游地址"), "{html}");
    assert!(!html.contains("?tab=dir"), "反代详情不应有目录标签: {html}");
}

/// 删除仍有站点绑定的运行环境：必须拒绝并列出域名，不做隐式解绑。
#[tokio::test]
async fn deleting_bound_runtime_lists_domains_and_is_rejected() {
    let h = harness();
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    let err = h.state.delete_site_runtime("php-8.5").await.unwrap_err();
    let text = format!("{err}");
    assert!(
        matches!(err, skiff_app::sites::SiteError::Conflict(_)),
        "有绑定必须冲突：{text}"
    );
    assert!(text.contains("demo.local"), "错误必须列出域名：{text}");
    assert!(
        h.state.db().runtime_by_id("php-8.5").unwrap().is_some(),
        "运行环境不能被删掉"
    );
    assert!(
        h.state.db().site_by_domain("demo.local").unwrap().is_some(),
        "站点不能被隐式删除"
    );
}

/// 批量操作必须要求管理员；viewer 直接 403，不产生任何副作用。
#[tokio::test]
async fn sites_batch_requires_admin() {
    let h = harness();
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    h.state
        .db()
        .create_site(&php_site("demo.local", "/www/sites/demo.local"))
        .unwrap();

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/batch")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(form_body(&[
                    ("action", "delete"),
                    ("domains", "demo.local"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        h.state.db().site_by_domain("demo.local").unwrap().is_some(),
        "viewer 的批量删除不得生效"
    );
}

/// 停止态保存非法配置：先校验、失败不落盘，并给出错误而不是「已重载」。
#[tokio::test]
async fn stopped_site_config_save_validation_failure_does_not_persist() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    h.state
        .db()
        .set_site_status("demo.local", "stopped")
        .unwrap();
    std::fs::create_dir_all(skiff_app::sites::site_conf_dir(&root)).unwrap();
    let disabled = skiff_app::sites::site_conf_dir(&root).join("demo.local.conf.disabled");
    let original = "server { listen 80; server_name demo.local; }";
    std::fs::write(&disabled, original).unwrap();

    let (_, sites_html) = get_page(&h.app, "/p/sites", &admin).await;
    let csrf = csrf_from(&sites_html);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/demo.local/config/save")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[
                    ("csrf", &csrf),
                    (
                        "content",
                        "server { listen 80; server_name demo.local; this_is_invalid_directive_xyz; }",
                    ),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "失败应原地回显配置页，不再跳转"
    );
    let html = body_text(resp).await;
    assert!(
        html.contains("this_is_invalid_directive_xyz"),
        "校验失败必须保留用户正在编辑的内容: {html}"
    );
    assert!(
        html.contains("校验") || html.contains("容器列表失败"),
        "必须展示校验失败原因: {html}"
    );
    assert!(
        !html.contains("配置已保存、校验通过并已重载"),
        "失败不得复用重载成功文案: {html}"
    );
    assert_eq!(
        std::fs::read_to_string(&disabled).unwrap(),
        original,
        "校验失败不得落盘"
    );
    assert!(
        !skiff_app::sites::site_conf_dir(&root)
            .join("demo.local.conf")
            .exists(),
        "失败不得把停用配置推进线上 include"
    );
}

/// 新建站点弹窗的运行环境下拉只列就绪实例：未安装/未运行/未实现的类型不出现。
#[tokio::test]
async fn site_new_dialog_excludes_unready_runtimes() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    h.state
        .db()
        .create_runtime(&skiff_store::NewRuntime {
            id: "rust-1.83".into(),
            kind: "rust".into(),
            name: "rust-1.83".into(),
            version: "1.83".into(),
            image: "rust:1.83-slim".into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/sites?new=1", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("Rust 1.83") && !html.contains("PHP 8.5"),
        "未安装或未运行的运行环境不得出现在下拉: {html}"
    );
    assert!(
        html.contains("没有已就绪的运行环境"),
        "完全没有就绪实例时必须给明确引导: {html}"
    );
    assert!(
        !html.contains(r#"name="runtime_id"#),
        "无就绪实例时不得渲染空下拉: {html}"
    );
}

/// 插件 UI 小回归：内置任务不渲染删除按钮；累计流量按字节自适应进位。
#[test]
fn plugin_ui_hides_builtin_delete_and_formats_traffic_bytes() {
    let cron = include_str!("../../../plugins/cron/ui/app.js");
    assert!(
        cron.contains("function isBuiltinTask") && cron.contains("isBuiltinTask(j.id)"),
        "cron 列表必须按 builtin: 前缀隐藏删除按钮"
    );
    assert!(
        !cron.contains("toggle + ' <button class=\"mini danger\" data-remove="),
        "删除按钮不得再无条件渲染"
    );

    let monitor = include_str!("../../../plugins/monitor/ui/app.js");
    assert!(
        monitor.contains("function bytes(") && monitor.contains("bytes(s.net_tx)"),
        "历史出站必须按原始字节格式化"
    );
    assert!(
        !monitor.contains("gb(s.net_tx * 1024)"),
        "不得再把字节当 KiB 换算后拼 GiB"
    );
}

/// 站点详情六个标签可访问，配置文件/目录/SSL 页有明确内容。
#[tokio::test]
async fn site_detail_tabs_are_reachable() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    std::fs::create_dir_all(skiff_app::sites::site_index_dir(&root)).unwrap();
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();
    h.state
        .db()
        .add_site_aliases("demo.local", &["www.demo.local".into()])
        .unwrap();
    std::fs::create_dir_all(skiff_app::sites::site_conf_dir(&root)).unwrap();
    let conf = skiff_app::sites::site_conf_dir(&root).join("demo.local.conf");
    std::fs::write(
        &conf,
        "server { listen 80; server_name demo.local www.demo.local; }",
    )
    .unwrap();

    for (uri, marker) in [
        ("/p/sites/demo.local", "站点概览"),
        ("/p/sites/demo.local?tab=domains", "别名"),
        ("/p/sites/demo.local?tab=config", "保存并重载 nginx"),
        ("/p/sites/demo.local?tab=dir", "文档根"),
        ("/p/sites/demo.local?tab=ssl", "生成自签证书并启用"),
    ] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(marker), "{uri} 缺少 {marker}: {html}");
    }
}

/// 站点详情的停止/删除属于高风险动作：顶部操作区不直出，只在「管理」页签内提供。
#[tokio::test]
async fn site_detail_risk_actions_only_in_manage_tab() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    std::fs::create_dir_all(skiff_app::sites::site_index_dir(&root)).unwrap();
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();

    let (status, overview) = get_page(&h.app, "/p/sites/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    // 顶部操作区 = page-head 内 actions 容器内的内容。
    let head = overview
        .split("class=\"page-head\"")
        .nth(1)
        .expect("详情页必须有顶部操作区");
    let head = head
        .split("class=\"actions\"")
        .nth(1)
        .expect("详情页必须有顶部操作区");
    let head = head.split("</div>").next().unwrap();
    assert!(!head.contains("停止"), "顶部操作区不得直出停止：{head}");
    assert!(
        !head.contains("删除站点"),
        "顶部操作区不得直出删除站点：{head}"
    );
    assert!(
        head.contains("日志") && head.contains("访问"),
        "日志/访问仍应留在顶部：{head}"
    );

    let (status, manage) = get_page(&h.app, "/p/sites/demo.local?tab=manage", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        manage.contains("/sites/demo.local/stop"),
        "管理页签内应有停止表单：{manage}"
    );
    assert!(
        manage.contains("删除站点"),
        "管理页签内应有删除入口：{manage}"
    );
}

/// 目录浏览只开放 `index/`：越界（../log、../ssl）必须是 400，而不是页面内提示。
#[tokio::test]
async fn site_dir_traversal_returns_bad_request() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    for dir in [
        skiff_app::sites::site_index_dir(&root),
        skiff_app::sites::site_log_dir(&root),
        skiff_app::sites::site_ssl_dir(&root),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    h.state
        .db()
        .create_site(&php_site("demo.local", &root.display().to_string()))
        .unwrap();

    for path in ["../log", "../ssl", "../log/access.log"] {
        let uri = format!("/p/sites/demo.local?tab=dir&path={path}");
        let (status, _) = get_page(&h.app, &uri, &admin).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} 必须 400");
    }
}

/// 文件管理插件 UI：CSP 只能自同源静态资源，页面必须提供现代文件管理器
/// 的骨架（目录树/快捷入口/面包屑/多选列表/右键菜单/编辑器）与键盘操作。
///
/// 这里断言的是**随包内容**，不依赖浏览器：任何外链、内联脚本或缺失的
/// 交互元素都会在 `cargo test` 阶段被挡住。
#[test]
fn files_ui_is_self_contained_and_exposes_modern_interactions() {
    let html = include_str!("../../../plugins/files/ui/main.html");
    let js = include_str!("../../../plugins/files/ui/app.js");

    // 自包含：不得出现任何 scheme 形式的绝对地址（包内一律相对路径）。
    for (name, src) in [("main.html", html), ("app.js", js)] {
        assert!(
            !src.contains("http://") && !src.contains("https://"),
            "{name} 不得引用外链资源"
        );
        assert!(!src.contains("//cdn."), "{name} 不得引用 CDN");
        assert!(
            !src.contains("fonts.googleapis") && !src.contains("fonts.gstatic"),
            "{name} 不得引用外链字体"
        );
    }

    // 脚本必须全部外链自包内文件，不允许内联 <script>。
    assert!(
        !html.contains("<script>"),
        "插件 UI 不得包含内联脚本（CSP script-src 'self' 会直接拒绝）"
    );
    for src in [
        "vendor/highlight.min.js",
        "vendor/languages/nginx.min.js",
        "app.js",
    ] {
        assert!(
            html.contains(&format!("<script src=\"{src}\"")),
            "缺少随包脚本引用: {src}"
        );
    }

    // 现代文件管理器骨架。
    for id in [
        "quick",
        "tree",
        "crumbs",
        "list",
        "ctx",
        "dlg-new",
        "dlg-jump",
        "dlg-confirm",
        "dlg-editor",
        "gutter",
        "ed-dirty",
    ] {
        assert!(
            html.contains(&format!("id=\"{id}\"")),
            "缺少必需节点 id=\"{id}\""
        );
    }
    assert!(
        html.contains(r#"role="listbox""#) && html.contains(r#"aria-multiselectable="true""#),
        "文件列表必须是可多选的 listbox，键盘与读屏才能工作"
    );
    assert!(html.contains("未保存"), "编辑器必须有未保存（dirty）提示");
    assert!(
        html.contains("单次读写上限 <b>256 KiB</b>"),
        "侧栏必须写明 256 KiB 读写上限"
    );
    assert!(
        !html.contains("上传") && !html.contains("下载"),
        "不得虚构后端不存在的能力（上传/下载）"
    );

    // 主题契约：缺省深色；父页面下发 skiff.theme 时写 html[data-theme]；
    // iframe 比父脚本先就绪时主动 request 一次，旧插件不监听消息则保持深色。
    assert!(
        html.contains(r#"data-theme="dark""#),
        "插件页缺省必须是深色（父面板未下发主题时不能闪白）"
    );
    assert!(
        js.contains(r#"msg.type === "skiff.theme""#),
        "app.js 必须监听父页面的 skiff.theme 消息"
    );
    assert!(
        js.contains("skiff.theme.request"),
        "app.js 必须主动向父页面索取一次主题，避免 iframe 先加载漏广播"
    );
    assert!(
        js.contains(r#"setAttribute("data-theme""#),
        "收到主题后必须写到 html[data-theme]（浅色覆盖由 CSS 令牌生效）"
    );

    // 真实交互：键盘、多选、重命名、删除确认、越权/超限提示。
    for needle in [
        r#"key === "ArrowDown""#,
        r#"key === "ArrowUp""#,
        r#"key === "Enter""#,
        r#"key === "F2""#,
        r#"key === "Delete""#,
        r#"key === "Backspace""#,
        "ev.ctrlKey || ev.metaKey",
        "ev.shiftKey",
    ] {
        assert!(js.contains(needle), "app.js 缺少交互片段: {needle}");
    }
    assert!(
        js.contains("var readOnly = !r.eof;"),
        "超过 256 KiB（eof=false）必须进入只读模式"
    );
    assert!(
        js.contains("超过 256 KiB") || html.contains("超过 256 KiB"),
        "只读原因必须对用户可见"
    );
    assert!(
        js.contains("无权访问") || html.contains("无权访问"),
        "越权路径必须给出明确提示"
    );

    // 关闭回归：作者样式里的 display 会盖掉 UA 的 dialog:not([open]) { display:none }，
    // 编辑器曾因此 close() 后整块留在页面上（三条关闭路径全部失效）。
    assert!(
        html.contains("dialog:not([open])"),
        "必须显式让位给 UA 的隐藏规则，否则 close() 后对话框仍可见"
    );
    assert!(
        html.contains(".editor-dialog[open] { display: flex"),
        "编辑器的 flex 布局只能作用在打开态，关闭态交给 UA display:none"
    );
    assert!(
        js.contains("ev.target === this"),
        "点遮罩必须走同一关闭/确认路径，不能静默丢未保存内容"
    );
    assert!(
        js.contains(r#"$("dlg-editor").addEventListener("cancel""#),
        "Esc 必须接在编辑器对话框的 cancel 上"
    );
}

#[tokio::test]
async fn tasks_page_is_read_only_for_viewer_and_requires_csrf_for_admin() {
    let h = harness();
    h.state.init_tasks().expect("任务服务应能初始化");

    // viewer：能看到列表与历史入口，但没有任何写操作表单。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, html) = get_page(&h.app, "/p/tasks", &viewer).await;
    assert_eq!(status, StatusCode::OK, "viewer 应能读任务页");
    assert!(html.contains("计划任务"), "任务页标题缺失: {html}");
    assert!(
        html.contains("builtin:db-vacuum"),
        "内置 db-vacuum 应出现在列表: {html}"
    );
    assert!(
        !html.contains("/tasks/builtin:db-vacuum/run"),
        "viewer 页面不得渲染写操作: {html}"
    );
    assert!(html.contains("只读"), "viewer 应看到只读提示: {html}");

    // viewer 直接 POST：403（角色判定先于 CSRF）。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/tasks/builtin:db-vacuum/toggle")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(form_body(&[("enabled", "0"), ("csrf", "bogus")]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer 不得启停任务");

    // admin：没有 CSRF 的 POST 必须被拒。
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/tasks/builtin:db-vacuum/toggle")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[("enabled", "0")]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "缺少 CSRF 的写操作必须失败: {}",
        resp.status()
    );

    // admin + 正确 CSRF：启停生效并回到详情页。
    let csrf = h.state.csrf_token(&admin);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/tasks/builtin:db-vacuum/toggle")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(form_body(&[("enabled", "0"), ("csrf", &csrf)]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "启停应 303 回详情");
    let view = h
        .state
        .tasks()
        .unwrap()
        .detail_view("builtin:db-vacuum", 5)
        .await
        .expect("任务应仍在");
    assert_eq!(view["task"]["enabled"], false, "停用应落库");
}

/// UI 语义兜底回归：`hidden` 与未打开的 dialog 必须不可见，
/// 避免页面里的 display 规则把空进度条、空告警、幽灵弹窗顶出来。
#[test]
fn ui_hidden_and_dialog_guards_hold() {
    let css = include_str!("../static/skiff.css");
    assert!(
        css.contains("[hidden] { display: none !important; }"),
        "缺少 [hidden] 全局兜底，组件 display 会覆盖 hidden"
    );
    assert!(
        css.contains("dialog:not([open]) { display: none !important; }"),
        "缺少 dialog:not([open]) 兜底，关闭的弹窗会被 display:flex 顶出来"
    );
    let tpl = include_str!("../templates/site_env.html");
    assert!(
        !tpl.contains("data-web-action"),
        "Web 服务不再出现在运行环境页，运维入口归 Web 服务页: {tpl}"
    );
}

fn attr_value(html: &str, name: &str) -> Option<String> {
    let needle = format!(r#"{name}=""#);
    let start = html.find(&needle)? + needle.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 极简 query 转义：把 Windows 反斜杠、空格与非 ASCII 都编码掉，
/// 避免 `http::Uri` 解析直接拒绝。
fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 第一步验收：页面可打开、目录可浏览、文本可读可写、旧深链 301。
#[tokio::test]
async fn files_page_browses_edits_and_redirects_legacy_link() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let cookie = format!("skiff_dev_session={token}");

    let page = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/files")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(
        html.contains(r#"class="files-page""#),
        "缺少文件管理页: {html}"
    );
    assert!(
        html.contains("/p/static/files.css") && html.contains("/p/static/files.js"),
        "页面必须加载独立的 files.css / files.js: {html}"
    );
    let csrf = attr_value(&html, "data-csrf").expect("页面必须带 CSRF 令牌");

    // 用独立临时目录作为浏览目标：harness 的 data_dir 本身是受保护根。
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("hello.txt");
    std::fs::write(&file, "旧内容").unwrap();
    let dir = work.path().to_str().unwrap();
    let file_path = file.to_str().unwrap();

    let list = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/p/files/api/list?path={}", encode_query(dir)))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = body_text(list).await;
    assert!(
        list_body.contains("hello.txt"),
        "列表必须包含文件: {list_body}"
    );

    let read = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/p/files/api/read?path={}",
                    encode_query(file_path)
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::OK);
    assert!(body_text(read).await.contains("旧内容"));

    let write = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/files/api/write")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "path": file_path, "content": "新内容" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(write.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "新内容");

    // 数据根硬拒：管理员也不能从页面读写 skiff.db / plugins。
    let denied = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/p/files/api/list?path={}",
                    encode_query(&h.data_dir.join("plugins").display().to_string())
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    let legacy = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.skiff.files/main")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(legacy.headers().get(header::LOCATION).unwrap(), "/p/files");
}
