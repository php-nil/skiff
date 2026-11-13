//! 主干集成测试（跨 crate 契约守门人）：登录 → 会话 → RBAC → 插件上传/信任/确认安装。
//!
//! **如实标注的缺口**：
//! - 「建容器」主干不在本文件：需要 Docker 引擎（Windows 开发机 / CI 无守护进程），
//!   该路径只能在 Linux 真机验证（见 docs/DEPLOY_TEST.md 未验证项）。
//! - kernel.sock / Supervisor 拉起插件进程是 unix-only，不在 Windows 测试面内。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    harness_full_with_catalog(form, tier, Default::default())
}

/// 指定商店目录配置的 harness：商店安装入口的路由与审计测试用。
fn harness_full_with_catalog(
    form: Form,
    tier: Tier,
    catalog: skiff_app::catalog::CatalogSettings,
) -> Harness {
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
        catalog,
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

/// GET 登录配置，返回（签名 CSRF token，Set-Cookie 下发的 `skiff_login` 值）。
///
/// budding 登录页通过 `GET /ui/v1/auth/login` 取 token；表单兼容路径仍带同一个
/// `skiff_login` Cookie，因此这里的 token 也能直接用于 `POST /p/login`。
async fn login_page_tokens(app: &Router) -> (String, String) {
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ui/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let cookie = set_cookie_value(&page, "skiff_login").expect("登录配置应下发 CSRF cookie");
    let body = body_text(page).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("登录配置必须是 JSON 信封");
    let csrf = value["data"]["csrf_token"]
        .as_str()
        .expect("登录配置必须带 csrf_token")
        .to_string();
    (csrf, cookie)
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

/// 带 `Accept: text/html` 的页面导航请求：壳页回落只对浏览器导航生效。
async fn get_html(app: &Router, uri: &str, token: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header(header::ACCEPT, "text/html")
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
                .uri("/p/m/overview")
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
    assert_eq!(
        loc, "/p/m/core/settings/security",
        "回跳旧版设置页不得携带消息参数"
    );

    let fetch = |cookie: String| {
        h.app.clone().oneshot(
            Request::builder()
                .uri("/p/m/core/settings/security")
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
                .uri("/p/m/overview")
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
                    .header(header::ACCEPT, "text/html")
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
                .uri("/p/m/core/settings/security")
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
                .uri("/p/m/core/docker/containers")
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
    // 站点文件插件：新定位是「网站 → 站点文件」，与内核「工具 → 文件管理」并存。
    let files_manifest = format!(
        r#"id = "com.skiff.files"
name = "站点文件"
version = "0.1.3"
key_id = "{}"
api = "1.2"
requires = ["fs"]
[[ui]]
label = "站点文件"
path = "main.html"
[[menu]]
id = "files"
parent = "sites"
label = "站点文件"
order = 60
icon = "file"
view = "main"
"#,
        "0".repeat(64)
    );
    seed_declared_plugin(h.state.db().as_ref(), files_manifest, false);
    let token = login(&h.app, "admin", "pw-test-123").await;

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/m/overview")
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
    // 内核「工具 → 文件管理」是独立入口；未启用的站点文件插件不得出现在菜单里。
    assert!(
        html.contains(r#"href="/p/m/core/files""#),
        "内核「工具 → 文件管理」入口必须存在: {html}"
    );
    assert!(
        !html.contains(r#"href="/p/m/plugin/com.skiff.files/files""#),
        "未启用的站点文件插件不得出现在菜单: {html}"
    );

    // 插件中心卡片仍能看见未启用插件，并给出启用动作而不是打开入口
    let center = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/m/core/plugins/installed")
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

/// 站点文件插件启用后挂「网站」一级，与「工具 → 文件管理」并存、不抢位。
#[tokio::test]
async fn site_files_plugin_menu_hangs_under_sites() {
    let h = harness();
    let manifest = format!(
        r#"id = "com.skiff.files"
name = "站点文件"
version = "0.1.3"
key_id = "{}"
api = "1.2"
requires = ["fs"]
[[ui]]
label = "站点文件"
path = "main.html"
[[menu]]
id = "files"
parent = "sites"
label = "站点文件"
order = 60
icon = "file"
view = "main"
"#,
        "0".repeat(64)
    );
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let token = login(&h.app, "admin", "pw-test-123").await;
    let page = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/m/overview")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    let sites_start = html
        .find(r#"data-menu-key="sites""#)
        .expect("网站一级必须存在");
    let docker_start = html
        .find(r#"data-menu-key="docker""#)
        .expect("Docker 一级必须存在");
    let sites = &html[sites_start..docker_start];
    assert!(
        sites.contains(r#"href="/p/m/plugin/com.skiff.files/files""#),
        "站点文件应挂在「网站」下: {sites}"
    );
    assert!(sites.contains("站点文件"), "菜单文案应为站点文件: {sites}");
    assert!(
        !sites.contains(r#"href="/p/m/core/files""#),
        "站点文件不得与内核文件管理同组: {sites}"
    );
    assert!(
        html.contains(r#"href="/p/m/core/files""#),
        "内核「工具 → 文件管理」入口必须保留: {html}"
    );
}

#[tokio::test]
async fn plugin_center_tabs_use_honest_empty_states() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for (uri, expected) in [
        ("/p/m/core/plugins/discover", "没有接入在线官方源"),
        ("/p/m/core/plugins/updates", "尚未接入更新检查源"),
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
                .uri("/p/m/core/plugins/installed")
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
                .uri("/p/m/core/settings/security")
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
                .uri("/p/m/core/settings/audit")
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
                .uri("/p/m/overview")
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
                .uri("/p/m/core/docker/containers?filter=error")
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
    for path in [
        "/p/m/core/docker/images",
        "/p/m/core/docker/networks",
        "/p/m/core/docker/volumes",
    ] {
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
                // 壳页白名单只对浏览器导航回落；未登录也应按页面惯例 303 到登录页。
                .uri("/p/containers")
                .header(header::ACCEPT, "text/html")
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

/// 商店安装入口：viewer 无权安装，必须 403 且落 `store.install` deny 审计。
#[tokio::test]
async fn store_install_requires_plugin_manage_permission() {
    let key = skiff_pkgfmt::SigningKey::generate();
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec!["https://mirror.example/skiff".into()],
            public_key: Some(key.public_hex()),
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer 从商店安装必须 403"
    );
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit.iter().any(|e| e.action == "store.install"
            && e.target == "com.example.demo"
            && e.result == "deny"
            && e.detail.contains("reason=permission")),
        "权限拒绝必须落审计: {audit:?}"
    );
}

/// 未配置固定公钥时不得提供商店安装：拒绝并落 `no_key` 审计，且不触网。
#[tokio::test]
async fn store_install_requires_configured_key() {
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec![],
            public_key: None,
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "store.install" && e.detail.contains("reason=no_key")),
        "缺公钥拒绝必须落审计: {audit:?}"
    );
}

/// 目录不可用（无镜像/无缓存）时拒绝安装：失败原因落审计、回商店页提示、不留暂存文件。
#[tokio::test]
async fn store_install_rejects_without_verified_catalog() {
    let key = skiff_pkgfmt::SigningKey::generate();
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            // 无镜像：快照为「未配置镜像地址」，不会发起任何网络请求。
            mirrors: vec![],
            public_key: Some(key.public_hex()),
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("csrf={csrf}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "失败应回商店页");
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(loc, "/p/store");
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit.iter().any(|e| e.action == "store.install"
            && e.result == "fail"
            && e.detail.contains("reason=not_found")),
        "目录不可用必须落 not_found 审计: {audit:?}"
    );
    let incoming = h.data_dir.join("plugins").join(".incoming");
    let leftover = std::fs::read_dir(&incoming)
        .map(|it| it.count())
        .unwrap_or(0);
    assert_eq!(leftover, 0, "失败不得留下暂存文件");
}

/// 内核升级下载入口：viewer 无权触发，必须 403 且落 `upgrade.download` deny 审计。
#[tokio::test]
async fn upgrade_download_requires_admin_permission() {
    let key = skiff_pkgfmt::SigningKey::generate();
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec!["https://mirror.example/skiff".into()],
            public_key: Some(key.public_hex()),
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "viewer", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer 下载升级包必须 403"
    );
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit.iter().any(|e| e.action == "upgrade.download"
            && e.target == "0.2.0"
            && e.result == "deny"
            && e.detail.contains("reason=permission")),
        "权限拒绝必须落审计: {audit:?}"
    );
}

/// 未配置固定公钥时不得下载升级包：拒绝并落 `no_key` 审计，且不触网。
#[tokio::test]
async fn upgrade_download_requires_configured_key() {
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec![],
            public_key: None,
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.action == "upgrade.download" && e.detail.contains("reason=no_key")),
        "缺公钥拒绝必须落审计: {audit:?}"
    );
}

/// 渠道没有升级数据时安静拒绝：回商店页提示、落 `not_found`、不留暂存目录文件。
#[tokio::test]
async fn upgrade_download_without_catalog_data_is_not_found() {
    let key = skiff_pkgfmt::SigningKey::generate();
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            // 无镜像：快照为「未配置镜像地址」，不会发起任何网络请求。
            mirrors: vec![],
            public_key: Some(key.public_hex()),
            allow_unsigned: false,
            timeout: std::time::Duration::from_secs(1),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "失败应回商店页");
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(loc, "/p/store");
    let audit = h.state.db().recent_audit(10).unwrap();
    assert!(
        audit.iter().any(|e| e.action == "upgrade.download"
            && e.result == "fail"
            && e.detail.contains("reason=not_found")),
        "无升级数据必须落 not_found 审计: {audit:?}"
    );
    let incoming = h.data_dir.join("upgrade").join("incoming");
    let leftover = std::fs::read_dir(&incoming)
        .map(|it| it.count())
        .unwrap_or(0);
    assert_eq!(leftover, 0, "失败不得留下半包");
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

/// 装 → 授权 → 重装/升级 → 既有授权仍在；升级只补「相对旧 manifest 新声明」的权限，
/// 用户当初拒绝过的声明项不会被翻案补授；对象 hint 按「只增不减」补齐；只有显式卸载才清空授权。
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

    let assert_grants = |label: &str, want: &[(&str, &str)]| {
        let mut got: Vec<(String, String)> = h
            .state
            .db()
            .grants(id)
            .unwrap()
            .into_iter()
            .map(|g| (g.kind, g.value))
            .collect();
        got.sort();
        let mut want: Vec<(String, String)> = want
            .iter()
            .map(|(kind, value)| (kind.to_string(), value.to_string()))
            .collect();
        want.sort();
        assert_eq!(got, want, "{label} 授权集合不符: {got:?}");
    };
    assert_grants(
        "首次安装后",
        &[("object", "container:abc"), ("perm", "docker:list")],
    );

    // 同版本重装：跳过确认页直接安装，权限仍保持既有最小子集，不重置为 manifest 默认值；
    // 但 manifest 新增的对象 hint（container:def）按「只增不减」补授。
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
    assert_grants(
        "重装后",
        &[
            ("object", "container:abc"),
            ("object", "container:def"),
            ("perm", "docker:list"),
        ],
    );
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

    // 升级 0.2.0：既有授权与已补的对象 hint 保留；新增声明的 docker:exec 按声明补授
    // （敏感档写审计），但旧 manifest 已声明、用户当初未勾选的 docker:inspect 不被翻案补授。
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
    assert_grants(
        "升级后",
        &[
            ("object", "container:abc"),
            ("object", "container:def"),
            ("perm", "docker:exec"),
            ("perm", "docker:list"),
        ],
    );
    let audit = h.state.db().recent_audit(20).unwrap();
    assert!(
        audit.iter().any(|e| {
            e.action == "plugin.grant_upgrade"
                && e.target == id
                && e.detail.contains("docker:exec")
                && e.detail.contains("0.1.0")
                && e.detail.contains("0.2.0")
        }),
        "升级补授敏感权限必须写审计，实际: {:?}",
        audit
    );
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
                .uri("/p/plugins/com.example.demo/delete")
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
        .oneshot(
            Request::builder()
                .uri("/p/m/overview")
                .body(Body::empty())
                .unwrap(),
        )
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

/// 受限形态（`SKIFF_FORM=docker`，能力面只有 docker/identity）没有 fs：
/// 文件管理入口必须消失，页面/深链/API 直达必须 403 且文案可读（不能借此列出
/// 容器内 `/`、`/www`）；工具分组里不依赖 fs 的入口与容器管理保持可用。
#[tokio::test]
async fn restricted_form_hides_and_rejects_files_management() {
    let h = harness_with(Form::DockerRestricted);
    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/m/overview", &token).await;
    assert_eq!(status, StatusCode::OK);
    for key in ["core/files", "core/files/trash", "core/files/quotas"] {
        assert!(
            !html.contains(&format!(r#"href="/p/m/{key}""#)),
            "受限形态不得渲染 `{key}` 入口: {html}"
        );
    }
    assert!(
        html.contains(r#"href="/p/m/core/tasks""#) && html.contains(r#"href="/p/m/core/jobs""#),
        "工具分组里不依赖 fs 的入口应保留: {html}"
    );
    // 菜单 href 必须真能落地（历史缺陷：`core/jobs` 有入口但 dispatch 漏了分支 → 404）。
    // 计划任务页在无调度器快照的测试环境可能 503，这里只验证不是 404。
    let (status, _) = get_page(&h.app, "/p/m/core/jobs", &token).await;
    assert_eq!(status, StatusCode::OK, "任务中心深链必须可解析");
    let (status, _) = get_page(&h.app, "/p/m/core/tasks", &token).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "计划任务深链必须可解析");

    for uri in [
        "/p/m/core/files",
        "/p/m/core/files/trash",
        "/p/m/core/files/quotas",
        "/p/files",
        "/p/files/trash",
        "/p/files/quotas",
        "/p/files/list",
        "/p/files/trash/list",
        "/p/files/quotas/list",
    ] {
        let (status, body) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "`{uri}` 必须 403");
        assert!(
            body.contains("当前部署形态不提供文件管理能力"),
            "`{uri}` 拒绝文案不可读: {body}"
        );
    }

    // 受限形态保留 docker 能力面：容器管理页照常可用。
    let (status, _) = get_page(&h.app, "/p/m/core/docker/containers", &token).await;
    assert_eq!(status, StatusCode::OK, "受限形态的容器管理必须保持可用");
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
    let (status, info_html) =
        get_page(&h.app, "/p/m/core/plugins/info/com.example.ui", &token).await;
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
            && info_html.contains(r#"action="/p/plugins/com.example.ui/delete""#),
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
        "/p/plugins/com.example.ui/main?legacy=1&open=%2Fvar%2Fwww%2Fsite%20one",
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
                .uri("/p/plugins/com.example.ui/main?legacy=1&open=%2Fetc%2F%0Apasswd")
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
                    .uri("/p/m/overview")
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
                    .uri("/p/m/overview")
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
                    .uri("/p/m/overview")
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
                    .uri("/p/m/overview")
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
                    .uri("/p/m/overview")
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
                    // 规范页已迁 budding 壳页：浏览器导航带 Accept，旧 SSR 走 /m 深链。
                    .header(header::ACCEPT, "text/html")
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
                        .uri("/p/m/overview")
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
    let (status, html) =
        get_page(&h.app, "/p/plugins/com.example.tabs/list?legacy=1", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("tab=list") && html.contains(r#"class="workbench-tabs""#),
        "{html}"
    );
    let (status, html) = get_page(
        &h.app,
        "/p/plugins/com.example.tabs/backup?legacy=1",
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("tab=backup"), "{html}");
    let (status, _) = get_page(&h.app, "/p/plugins/com.example.tabs/nope?legacy=1", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知视图必须 404");

    // 无 L3 的 L2：隐式主视图，不渲染标签栏，但深链仍可直达。
    let (status, html) =
        get_page(&h.app, "/p/plugins/com.example.tabs/info?legacy=1", &admin).await;
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
    let (status, html) = get_page(&h.app, "/p/m/overview", &admin).await;
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
    let (status, html) = get_page(&h.app, "/p/m/overview", &viewer).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!html.contains(r#"data-slot="overview.health""#));
    assert!(html.contains(r#"class="health-grid""#));

    // 插件停用：槽位消失，同样回落默认健康块。
    seed_declared_plugin(h.state.db().as_ref(), manifest, false);
    let (status, html) = get_page(&h.app, "/p/m/overview", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/plugins/info/com.example.info", &admin).await;
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
    let (status, view_html) =
        get_page(&h.app, "/p/plugins/com.example.info/main?legacy=1", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        view_html.contains("/p/plugins/com.example.info/ui/main.html?nonce="),
        "{view_html}"
    );
    assert!(
        !view_html.contains(r#"class="workbench-tabs""#),
        "{view_html}"
    );
    let (status, _) = get_page(&h.app, "/p/plugins/com.example.info/nope?legacy=1", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 停用后信息页仍可见（用于恢复/卸载），但不再提供功能页入口。
    h.state
        .db()
        .set_plugin_enabled_state("com.example.info", false)
        .unwrap();
    let (status, html) = get_page(&h.app, "/p/m/core/plugins/info/com.example.info", &admin).await;
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

/// 任何直接导航到插件功能页都只能拿到原生占位页，DOM 里不得出现 iframe；
/// 旧 SSR 工作台只接受显式 `?legacy=1`（兼容嵌入），不再作为产品入口。
#[tokio::test]
async fn plugin_view_document_navigation_serves_shell_placeholder() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    // 不带任何 Sec-Fetch-* 头（反向代理剥离后的形态）也必须落到占位页。
    for headers in [None, Some(("sec-fetch-dest", "document"))] {
        let mut builder = Request::builder()
            .uri("/p/plugins/com.example.info/main")
            .header(header::COOKIE, format!("skiff_dev_session={admin}"))
            .header(header::ACCEPT, "text/html");
        if let Some((name, value)) = headers {
            builder = builder.header(name, value);
        }
        let resp = h
            .app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let html = body_text(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains("该功能正在适配中"), "{html}");
        assert!(
            !html.contains("<iframe") && !html.contains("data-workbench-frame"),
            "壳页占位不得内嵌 iframe 工作台: {html}"
        );
    }
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

    let (status, html) = get_page(
        &h.app,
        "/p/plugins/com.example.legacy/main?legacy=1",
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/p/plugins/com.example.legacy/ui/index.html?nonce="),
        "旧包 main 必须回落 ui/index.html: {html}"
    );
    assert!(
        !html.contains(r#"class="workbench-tabs""#),
        "旧包单入口不得渲染标签栏: {html}"
    );
    let (status, _) = get_page(
        &h.app,
        "/p/plugins/com.example.legacy/other?legacy=1",
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 信息页也必须存在，并给出 main 入口与旧包菜单深链。
    let (status, html) =
        get_page(&h.app, "/p/m/core/plugins/info/com.example.legacy", &admin).await;
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
    assert_eq!(
        m.requires_str(),
        vec!["fs", "identity", "interop", "scheduler"]
    );
    assert!(m.resource_hints.iter().any(|h| h == "path:/proc"));
    assert_eq!(m.interop.consumes, vec!["com.skiff.cron:jobs.list"]);
    assert!(m.interop.provides.iter().any(|p| p.method == "probe"));
    assert!(
        m.task.iter().any(|t| t.key == "busy-probe"
            && t.method == "busy_probe"
            && !t.enabled
            && t.sample_method.as_deref() == Some("buffer_sample")
            && t.capacity_bytes == Some(4096)),
        "testkit 的 BusyGuard 探针任务必须同时声明缓冲（TASK-06 的 buffer.push 落点）"
    );

    let h = harness();
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/m/core/plugins/info/com.skiff.testkit", &admin).await;
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

    let (status, html) = get_page(
        &h.app,
        "/p/plugins/com.skiff.testkit/check?legacy=1",
        &admin,
    )
    .await;
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

    let (status, html) = get_page(
        &h.app,
        "/p/plugins/com.skiff.testkit/single?legacy=1",
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("/p/plugins/com.skiff.testkit/ui/single.html?nonce="),
        "{html}"
    );
    assert!(
        !html.contains(r#"class="workbench-tabs""#),
        "单视图 L2 不得渲染标签栏: {html}"
    );

    let (status, html) = get_page(&h.app, "/p/m/overview", &admin).await;
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

/// 官方包「装完即可用」回归。
///
/// 规则：**每个**官方包要么声明至少一条可用的默认授权建议（`path:` / `container:`
/// / `project:` / `plugin:`），要么在下表里**显式列名并写明原因**；新包既没默认授权、
/// 又没登记时会直接红。站点文件是唯一「预置 `path:` 反而有害」的例外：站点域名打包时
/// 未知，预置只能写成 `/www/sites` 这类跨站点根，因此只允许它为空且必须有可读提示。
/// cron 还必须声明 scheduler 能力（否则全新安装后列表 400）。
#[tokio::test]
async fn official_packages_declare_usable_install_defaults() {
    const NO_DEFAULT_GRANT: &[(&str, &str)] = &[
        (
            "com.skiff.guard",
            "守护对象由管理员逐条勾选 container:/project:",
        ),
        ("com.skiff.cron", "只调度插件接口，无宿主资源"),
        (
            "com.skiff.firewall",
            "能力面自带受控作用域（inet skiff / SKIFF 链），无需 path: 授权",
        ),
    ];
    const OFFICIAL: &[(&str, &str)] = &[
        (
            "guard",
            include_str!("../../../plugins/guard/manifest.toml"),
        ),
        (
            "monitor",
            include_str!("../../../plugins/monitor/manifest.toml"),
        ),
        (
            "files",
            include_str!("../../../plugins/files/manifest.toml"),
        ),
        ("cron", include_str!("../../../plugins/cron/manifest.toml")),
        ("ftp", include_str!("../../../plugins/ftp/manifest.toml")),
        (
            "firewall",
            include_str!("../../../plugins/firewall/manifest.toml"),
        ),
        (
            "testkit",
            include_str!("../../../plugins/testkit/manifest.toml"),
        ),
    ];

    for (name, src) in OFFICIAL {
        let m = skiff_pkgfmt::Manifest::parse(src)
            .unwrap_or_else(|e| panic!("{name} manifest 必须可解析: {e}"));
        let usable = |h: &String| {
            h.starts_with("path:/")
                || h.starts_with("container:")
                || h.starts_with("project:")
                || h.starts_with("plugin:")
        };
        for h in &m.resource_hints {
            assert!(
                usable(h),
                "{name} 的默认授权建议不可用（{h}），只接受 path:/ container:/ project:/ plugin: 前缀"
            );
        }
        if m.id == "com.skiff.files" {
            assert!(
                m.resource_hints.is_empty(),
                "站点文件不得预置 path:（域名打包时未知，会放大成跨站点授权）: {:?}",
                m.resource_hints
            );
            continue;
        }
        if !m.resource_hints.iter().any(usable) {
            assert!(
                NO_DEFAULT_GRANT.iter().any(|(id, _)| *id == m.id),
                "{} 必须声明可用的默认授权建议，或在 NO_DEFAULT_GRANT 里显式列名并说明原因",
                m.id
            );
        }
    }

    let files = skiff_pkgfmt::Manifest::parse(include_str!("../../../plugins/files/manifest.toml"))
        .expect("files manifest 必须可解析");
    let perms = files.permission_strings();
    for p in ["fs:read", "fs:write", "fs:list", "fs:mkdir", "fs:rename"] {
        assert!(perms.iter().any(|x| x == p), "files 缺少 {p}: {perms:?}");
    }

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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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

    let (status, detail) = get_page(&h.app, "/p/m/core/sites/list/demo.local", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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
    let (_, detail) = get_page(&h.app, "/p/m/core/sites/list/plain.local", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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
    let (_, html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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
    let cancel_at = html.find(">取消<").expect("确认页必须有取消入口");
    let submit_at = html
        .find("data-type-confirm-submit>删除站点<")
        .expect("确认页必须有删除站点按钮");
    assert!(
        cancel_at < submit_at,
        "按钮顺序必须是取消在左、危险操作在右（UI_CONTRACT §9）: {html}"
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
        js.contains("window.innerHeight - top - chromeH - gap"),
        "长内容必须按一屏可用高度封顶（iframe 内部滚动，不撑出页面第二条滚动条）"
    );
    assert!(
        js.contains("borderTopWidth") && js.contains("marginBottom"),
        "可用高度要扣掉宿主面板自身的边框/外边距，否则窄屏封顶仍会挤出一条页级滚动条"
    );
    assert!(
        js.contains("document.documentElement.scrollHeight - window.innerHeight"),
        "封顶模式必须用真实滚动高度做一次溢出校正（亚像素累计会多出 1px）"
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list?new=1", &admin).await;
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
    let (_, html) = get_page(&h.app, "/p/m/core/sites/list?new=1", &admin).await;
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
    let (_, html) = get_page(&h.app, "/p/m/core/sites/list?new=1", &admin).await;
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

    let (status, html) =
        get_page(&h.app, "/p/m/core/sites/list/dom.local?tab=domains", &admin).await;
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
    assert_eq!(
        loc, "/p/sites/dom.local?tab=domains",
        "写操作仍应重定向到规范页"
    );
    // 跟随重定向必须落在规范页（budding 壳页），而不是旧 SSR 或 404。
    let (status, shell) = get_html(&h.app, &loc, &admin).await;
    assert_eq!(status, StatusCode::OK, "跟随重定向应落在规范页");
    assert!(shell.contains(r#"id="skiff-root""#), "{shell}");
    // 写操作迁移前，提示文案仍由旧 SSR 回退落点呈现，确认没有丢失。
    let m_loc = loc.replace("/p/sites/", "/p/m/core/sites/list/");
    let (_, page) = get_page(&h.app, &m_loc, &admin).await;
    assert!(page.contains("主域名不能作为别名删除"), "{page}");
}

/// 运行环境页是业务视角：列表/详情/深链可访问，viewer 一律 403。
#[tokio::test]
async fn runtime_pages_are_admin_only_and_render_business_view() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    for uri in ["/p/m/core/sites/env", "/p/m/core/sites/env/php-8.5"] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains("PHP"), "{uri} 必须展示运行环境类型: {html}");
    }
    let (status, html) = get_page(&h.app, "/p/m/core/sites/env/php-8.5", &admin).await;
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

    // 规范页已迁 budding 壳页：viewer 能打开壳页，但 JSON 列表接口与写操作必须 403。
    for uri in ["/p/runtimes", "/p/runtimes/php-8.5"] {
        let (status, _) = get_html(&h.app, uri, &viewer).await;
        assert_eq!(status, StatusCode::OK, "{uri} 壳页对登录用户可见");
    }
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/ui/v1/runtimes")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer 不得读取运行环境列表"
    );

    // viewer 的写操作同样 403，且不产生副作用。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/runtimes/php-8.5/action/start")
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

    for uri in ["/p/m/core/sites/env/rust-1.83"] {
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
    let (_, php_html) = get_page(&h.app, "/p/m/core/sites/env/php-8.5", &admin).await;
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
    let (status, html) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
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
        !html.contains(r#"data-runtime-open="/p/runtimes/php-8.3""#)
            && !html.contains(r#"data-runtime-open="/p/runtimes/php-8.4""#)
            && !html.contains(r#"data-runtime-open="/p/runtimes/php-8.5""#),
        "迁移种子行不得在列表占位（模板只在新建流程出现）: {html}"
    );
    assert!(
        html.contains(r#"data-runtime-drawer"#)
            && html.contains(r#"data-runtime-open="/p/runtimes/php-8.5-a""#)
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
            && web.contains("/p/runtimes/web/action/"),
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

    // 旧入口不再渲染站点设置页；`/sites` 前缀命中壳页白名单，回落为 budding 占位。
    let (status, html) = get_html(&h.app, "/p/sites/settings", &admin).await;
    assert_eq!(status, StatusCode::OK, "白名单深链应回落壳页");
    assert!(
        !html.contains("镜像加速源") && !html.contains(r#"name="registry_mirror""#),
        "站点设置页必须已移除，不得借壳页回退: {html}"
    );

    // 新 Docker 设置页：镜像加速源 + 镜像前缀；菜单深链同源。
    for uri in ["/p/settings/docker", "/p/m/core/docker/settings"] {
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
    let (status, env_html) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        env_html.contains(r#"name="nginx_image""#)
            && env_html.contains(r#"name="php_image""#)
            && env_html.contains("/p/runtimes/images"),
        "运行环境页必须提供镜像标签保存入口: {env_html}"
    );

    // viewer 打开页面只读，保存仍被拒绝。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, html) = get_page(&h.app, "/p/settings/docker", &viewer).await;
    assert_eq!(status, StatusCode::OK);
    let csrf = csrf_from(&html);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings/docker")
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

    let (status, html) = get_page(&h.app, "/p/m/core/docker/compose", &admin).await;
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
    let (status, viewer_html) = get_page(&h.app, "/p/m/core/docker/compose", &viewer).await;
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
    let (_, html) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
    let csrf = csrf_from(&html);

    for (name, version) in [("PHP-8.5", "8.5"), ("-php", "8.5"), ("php-8.5-a", "")] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/p/runtimes/create")
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
            location, "/p/runtimes",
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
                .uri("/p/runtimes/create")
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
    assert_eq!(location, "/p/runtimes", "回跳不得携带消息参数");
    // 接管后已是真实实例，再次同名必须拒绝。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/runtimes/create")
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
    assert_eq!(location, "/p/runtimes", "回跳不得携带消息参数");
    let (_, page) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
    assert!(page.contains("已存在"), "重名原因应经 flash 回显: {page}");

    // 目标目录已存在：直接拒绝，不覆盖、不自动加后缀。
    std::fs::create_dir_all(h.data_dir.join("www/runtime/php/php-8.4-a")).unwrap();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/runtimes/create")
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
    assert_eq!(location, "/p/runtimes", "回跳不得携带消息参数");
    let (_, page) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
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
    let (status, html) = get_page(&h.app, "/p/m/core/sites/env/php-8.5", &admin).await;
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
            && html.contains(r#"action="/p/runtimes/php-8.5/extensions-batch""#)
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
                .uri("/p/runtimes/jobs")
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-runtime-down"), "{html}");
    assert!(
        html.contains("502/503") && html.contains("不会静默切换"),
        "必须说明停止版本的后果: {html}"
    );
    assert!(
        html.contains(r#"href="/p/runtimes/php-8.5""#),
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

    let (status, list_html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        list_html.contains("状态未知（Docker 不可用）"),
        "站点列表必须把 Docker 不可达标成未知，而不是运行中: {list_html}"
    );

    let (status, detail_html) = get_page(&h.app, "/p/m/core/sites/list/demo.local", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        detail_html.contains("状态未知（Docker 不可用）"),
        "站点详情必须与列表一致: {detail_html}"
    );
    assert!(
        detail_html.contains("data-runtime-down"),
        "Docker 未知时也要给出不可用告警: {detail_html}"
    );

    let (status, env_html) = get_page(&h.app, "/p/m/core/sites/env", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list/proxy.local", &admin).await;
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

    let (_, sites_html) = get_page(&h.app, "/p/m/core/sites/list", &admin).await;
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

    let (status, html) = get_page(&h.app, "/p/m/core/sites/list?new=1", &admin).await;
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
        monitor.contains("rate(s.net_rx_rate)") && monitor.contains("rate(s.net_tx_rate)"),
        "历史表入站/出站都必须按速率列显示"
    );
    assert!(
        monitor.contains("card(\"c-net\", rate(s.net_rx_rate))")
            && monitor.contains("card(\"c-net-tx\", rate(s.net_tx_rate))"),
        "概览必须拆入站/出站两张速率卡"
    );
    assert!(
        !monitor.contains("bytes(s.net_tx)"),
        "不得再把累计字节当速率展示"
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
        ("/p/m/core/sites/list/demo.local", "站点概览"),
        ("/p/m/core/sites/list/demo.local?tab=domains", "别名"),
        (
            "/p/m/core/sites/list/demo.local?tab=config",
            "保存并重载 nginx",
        ),
        ("/p/m/core/sites/list/demo.local?tab=dir", "文档根"),
        (
            "/p/m/core/sites/list/demo.local?tab=ssl",
            "生成自签证书并启用",
        ),
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

    let (status, overview) = get_page(&h.app, "/p/m/core/sites/list/demo.local", &admin).await;
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

    let (status, manage) =
        get_page(&h.app, "/p/m/core/sites/list/demo.local?tab=manage", &admin).await;
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
        let uri = format!("/p/m/core/sites/list/demo.local?tab=dir&path={path}");
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
    let (status, html) = get_page(&h.app, "/p/m/core/tasks", &viewer).await;
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
async fn files_page_browses_edits_and_falls_back_plugin_link() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let cookie = format!("skiff_dev_session={token}");

    let page = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/m/core/files")
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
                .uri(format!("/p/files/list?path={}", encode_query(dir)))
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
                .uri(format!("/p/files/read?path={}", encode_query(file_path)))
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
                .uri("/p/files/write")
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
                    "/p/files/list?path={}",
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
    assert_eq!(legacy.status(), StatusCode::FOUND);
    assert_eq!(legacy.headers().get(header::LOCATION).unwrap(), "/p/files");
}

/// 回收站是「工具 → 文件管理」的 L3 视图（菜单契约 §1.1），不再是独立二级菜单：
/// 导航里没有独立入口，两个页面共用同一标签栏，旧页面路由与深链保持可用。
#[tokio::test]
async fn trash_is_files_l3_tab_not_standalone_menu_item() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, nav) = get_page(&h.app, "/p/m/overview", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        nav.contains(r#"href="/p/m/core/files""#),
        "文件管理二级入口必须保留: {nav}"
    );
    assert!(
        !nav.contains(r#"href="/p/m/core/files/trash""#),
        "回收站不得再作为独立二级菜单项渲染: {nav}"
    );
    assert!(
        nav.contains(r#"href="/p/m/core/files/quotas""#),
        "配额管理仍是独立二级入口: {nav}"
    );

    let (status, files) = get_page(&h.app, "/p/m/core/files", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        files.contains(r#"class="workbench-tabs""#),
        "文件管理页必须渲染共用标签栏: {files}"
    );
    assert!(
        files.contains(r#"data-menu-key="core/files/trash""#)
            && files.contains(r#"aria-current="page">浏览"#),
        "浏览标签应为当前态且回收站作为同级标签存在: {files}"
    );

    let (status, trash) = get_page(&h.app, "/p/files/trash", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        trash.contains(r#"aria-current="page">回收站"#),
        "回收站标签应为当前态: {trash}"
    );

    // 旧深链（现在是 L3 深链）仍可解析，不因菜单结构变化 404。
    let (status, _) = get_page(&h.app, "/p/m/core/files/trash", &token).await;
    assert_eq!(status, StatusCode::OK);
}

/// 任务中心详情的数据链路：`jobs` / `job_logs` 表里的日志必须真的渲染到页面，
/// 空日志显示「暂无日志」而不是空白块；退出码、耗时、触发来源都要有实际取值落点。
#[tokio::test]
async fn job_detail_renders_persisted_logs_and_metadata() {
    use skiff_app::jobs::JobSubmit;

    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let job = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "site.create".into(),
            target_type: "site".into(),
            target_id: "detail.example.com".into(),
            title: "创建站点 detail.example.com".into(),
            params: serde_json::json!({ "name": "detail.example.com" }),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();
    h.state
        .jobs()
        .log_lines(&job.id, "out", &["安装脚本输出第一行".to_string()]);
    h.state.jobs().set_exit_code(&job.id, 127);

    let (status, html) = get_page(&h.app, &format!("/p/m/core/jobs/{}", job.id), &token).await;
    assert_eq!(status, StatusCode::OK, "详情页必须 200: {html}");
    assert!(
        html.contains("任务已创建，等待执行") && html.contains("安装脚本输出第一行"),
        "详情页必须渲染已落库的执行日志: {html}"
    );
    assert!(
        html.contains("退出码") && html.contains(">127<"),
        "详情必须渲染退出码的真实取值: {html}"
    );
    assert!(
        html.contains("触发来源") && html.contains("手动提交 · admin"),
        "详情必须渲染触发来源: {html}"
    );
    assert!(
        html.contains(r#"data-job-log-empty hidden"#),
        "有日志时空态占位必须隐藏而非移除（供 SSE 追加时切换）: {html}"
    );
    // 弹窗的数据源就是同一张详情页：列表侧必须是按钮 + 对话框，且日志容器可被 JS 接管。
    assert!(
        html.contains(r#"data-job-log"#),
        "日志容器缺少 JS 接管标记: {html}"
    );

    let (_, list) = get_page(&h.app, "/p/m/core/jobs", &token).await;
    assert!(
        list.contains(&format!(r#"data-job-open="{}""#, job.id)),
        "任务中心「查看详情」必须打开弹窗: {list}"
    );
    assert!(
        list.contains("data-job-dialog") && list.contains("data-job-dialog-body"),
        "任务中心必须渲染详情弹窗容器: {list}"
    );
}

/// 重试必须复用正常提交的执行器：历史任务重试后会离开 queued 并被结算；
/// 不支持的类型直接拒绝且不插行，同目标已有活跃任务时明确 409，
/// 避免僵尸行长期占用 `(target_type,target_id)` 唯一索引。
#[tokio::test]
async fn job_retry_reuses_executor_and_rejects_unsupported_kind() {
    use skiff_app::jobs::JobSubmit;

    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let retry = |id: String| {
        let app = h.app.clone();
        let cookie = format!("skiff_dev_session={token}");
        let csrf = csrf.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/p/jobs/{id}/retry"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, cookie)
                    .body(format!("csrf={csrf}"))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    // 1) 当前没有执行器的类型：明确拒绝，且不能留下一条 queued 行占索引。
    let legacy = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "site.create".into(),
            target_type: "site".into(),
            target_id: "retry.example.com".into(),
            title: "创建站点 retry.example.com".into(),
            params: serde_json::json!({ "name": "retry.example.com" }),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();
    h.state.jobs().cancel(&legacy.id).unwrap();
    let before = h.state.jobs().list(200).len();
    let resp = retry(legacy.id.clone()).await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "没有执行器的类型必须拒绝重试"
    );
    assert_eq!(
        h.state.jobs().list(200).len(),
        before,
        "拒绝重试时不得插入新行"
    );

    // 2) 有执行器的类型：新行 retry_of 指向原行，且会被执行器结算而不是永远 queued。
    let orig = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "runtime.ext.apply".into(),
            target_type: "runtime".into(),
            target_id: "php-8.5".into(),
            title: "更新 PHP 8.5 的扩展".into(),
            params: serde_json::json!({"runtime_id": "php-8.5", "ext_keys": ["redis"]}),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();
    h.state.jobs().cancel(&orig.id).unwrap();
    let resp = retry(orig.id.clone()).await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "重试应创建新任务并回跳"
    );

    let new_id = h
        .state
        .jobs()
        .list(200)
        .into_iter()
        .find(|j| j.retry_of.as_deref() == Some(orig.id.as_str()))
        .map(|j| j.id)
        .expect("必须存在 retry_of 指向原任务的新行");
    let mut settled = None;
    for _ in 0..150 {
        let cur = h.state.jobs().get(&new_id).unwrap();
        if !cur.active() {
            settled = Some(cur);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let settled = settled.expect("重试任务必须被执行器结算，不能永远停在 queued");

    // 3) 同目标仍有活跃任务时再次重试：唯一索引明确拒绝并说明原因，不排队。
    let blocker = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "runtime.ext.apply".into(),
            target_type: "runtime".into(),
            target_id: "php-8.5".into(),
            title: "占用同目标".into(),
            params: serde_json::json!({"runtime_id": "php-8.5", "ext_keys": ["redis"]}),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();
    let resp = retry(orig.id.clone()).await;
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "同目标已有活跃任务必须明确拒绝（{}）",
        skiff_web::labels::job_status(settled.status)
    );
    h.state.jobs().cancel(&blocker.id).unwrap();
}

/// 没有任何日志的作业：详情页与弹窗接口都要明确显示「暂无日志」。
#[tokio::test]
async fn job_detail_empty_logs_show_placeholder() {
    use skiff_store::jobs::{JobRow, STATUS_SUCCEEDED};

    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    h.state
        .db()
        .insert_job(&JobRow {
            id: "job-empty-log".into(),
            kind: "compose.up".into(),
            target_type: "compose".into(),
            target_id: "empty-log".into(),
            title: "Compose 启动 empty-log".into(),
            params_json: "{}".into(),
            phase: "done".into(),
            phase_label: "完成".into(),
            progress: 100,
            status: STATUS_SUCCEEDED.into(),
            cancellable: false,
            cancel_requested: false,
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            created_at: 1_700_000_000,
            started_at: Some(1_700_000_001),
            finished_at: Some(1_700_000_004),
            duration_ms: 3_000,
            error_class: None,
            error_detail: None,
            result_json: "{}".into(),
            log_bytes: 0,
            log_seq: 0,
            retry_of: None,
            exit_code: None,
        })
        .unwrap();

    let (status, html) = get_page(&h.app, "/p/m/core/jobs/job-empty-log", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("暂无日志"),
        "空日志必须显示「暂无日志」而不是空白区块: {html}"
    );
    assert!(html.contains("3 秒"), "耗时必须真实渲染: {html}");
    assert!(
        html.contains(r#"data-job-exit>-<"#),
        "无退出码语义时必须显示 `-`，不能用 0 冒充: {html}"
    );
}

/// URL 契约 §7：旧路径必须 301/308 到规范路径，保留方法与查询串，不留死链。
#[tokio::test]
async fn legacy_paths_redirect_to_canonical_urls() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    for (old, status, new) in [
        (
            "/p/files/api/list?path=%2Fwww",
            StatusCode::PERMANENT_REDIRECT,
            "/p/files/list?path=%2Fwww",
        ),
        (
            "/p/docker/settings",
            StatusCode::PERMANENT_REDIRECT,
            "/p/settings/docker",
        ),
        ("/p/sites/env", StatusCode::MOVED_PERMANENTLY, "/p/runtimes"),
        (
            "/p/sites/env/php-8.5",
            StatusCode::MOVED_PERMANENTLY,
            "/p/runtimes/php-8.5",
        ),
    ] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(old)
                    .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), status, "{old} 必须重定向");
        assert_eq!(
            resp.headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some(new),
            "{old} 的目标路径"
        );
    }

    // 旧的 GET 安装入口不再执行安装，只回商店页（安装必须是带 CSRF 的 POST）。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/p/store")
    );
}

/// 并发页面请求：会话中间件与概览页的 DB 读走有界阻塞投递后，
/// 多请求并发仍应全部成功（不在 worker 上排队互拖）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_page_requests_all_succeed() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let app = h.app.clone();
        let token = token.clone();
        tasks.push(tokio::spawn(async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri("/p/m/overview")
                        .header(header::COOKIE, format!("skiff_dev_session={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            resp.status()
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }
}

/// 镜像源被静默丢包（TCP 能连上、但永不回包）时：
/// 页面必须正常 200 并展示降级提示，且 5 分钟冷却内不再发起第二次抓取。
#[tokio::test]
async fn store_page_returns_200_when_mirror_silently_drops_packets() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counter.fetch_add(1, Ordering::SeqCst);
            // 持有连接但一个字节都不回：模拟「连得上、无响应」。
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(30));
                drop(stream);
            });
        }
    });

    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            channel: "stable".into(),
            mirrors: vec![format!("http://127.0.0.1:{port}")],
            public_key: None,
            allow_unsigned: false,
            timeout: std::time::Duration::from_millis(800),
            extra_roots: Vec::new(),
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let get_store = |app: Router, token: String| async move {
        app.oneshot(
            Request::builder()
                .uri("/p/m/core/plugins/store")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    };

    let resp = get_store(h.app.clone(), token.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK, "抓取超时不得变成框架 504");
    let body = body_text(resp).await;
    assert!(
        body.contains("镜像源不可达") || body.contains("尚未连上任何镜像源"),
        "应展示降级提示: {body}"
    );
    assert!(accepted.load(Ordering::SeqCst) >= 1, "首次访问应尝试建连");

    let before = accepted.load(Ordering::SeqCst);
    let resp = get_store(h.app.clone(), token).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        accepted.load(Ordering::SeqCst),
        before,
        "5 分钟冷却内不得再次发起抓取"
    );
}

// ---- budding 后端地基：/ui/v1 会话引导与 SPA 壳页回落（URL 契约 §2/§2.1/§6）----

/// 带可选会话与 `Accept` 的 GET，返回（状态码，Content-Type，响应体）。
async fn get_ui(
    app: &Router,
    uri: &str,
    token: Option<&str>,
    accept: Option<&str>,
) -> (StatusCode, String, String) {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = token {
        req = req.header(
            header::COOKIE,
            format!("{}={t}", skiff_identity::session_cookie_name(true)),
        );
    }
    if let Some(a) = accept {
        req = req.header(header::ACCEPT, a);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (status, ct, body_text(resp).await)
}

/// 未认证时必须 401 JSON，不能退回页面 303（SDUI 集成 §5.2）。
#[tokio::test]
async fn ui_session_requires_login_with_json_401() {
    let h = harness();
    let (status, ct, body) = get_ui(&h.app, "/ui/v1/session", None, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "未认证必须是 401，不能 303"
    );
    assert!(
        ct.starts_with("application/json"),
        "401 必须是 JSON 信封，实际 Content-Type: {ct}"
    );
    let v: serde_json::Value = serde_json::from_str(&body).expect("401 响应必须是 JSON");
    assert_eq!(v["error"]["code"], "auth.unauthenticated");
    assert_eq!(v["error"]["status"], 401);
    assert_eq!(v["error"]["retryable"], false);
}

/// 会话引导返回用户、角色、CSRF、入口前缀与壳页能力开关。
#[tokio::test]
async fn ui_session_returns_identity_csrf_and_prefix() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).expect("session 响应必须是 JSON");
    assert_eq!(v["data"]["username"], "admin");
    assert_eq!(v["data"]["role"], "admin");
    assert_eq!(v["data"]["entry_prefix"], "p");
    assert_eq!(v["data"]["api_version"], "v1");
    assert_eq!(v["data"]["features"]["shell_fallback"], true);
    assert_eq!(v["data"]["features"]["placeholders"], true);
    assert_eq!(v["data"]["features"]["plugin_iframe"], true);
    // CSRF 与页面写路径同源：sha256(secret:session token)，不新造第二种生成方式。
    assert_eq!(
        v["data"]["csrf_token"],
        skiff_identity::sha256_hex(format!("csrf-test-secret:{token}"))
    );
}

/// `/ui/menu` 是 budding 壳的菜单数据：直接投影内核菜单树（L1/L2 归属、顺序、
/// 可见性同源），且不把插件自带 HTML/iframe 的功能页入口下发到新一代前端。
#[tokio::test]
async fn ui_menu_projects_kernel_menu_without_plugin_pages() {
    let h = harness();
    let (status, _, _) = get_ui(&h.app, "/ui/v1/menu", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "未认证必须是 401");

    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).expect("menu 响应必须是 JSON");
    let groups = v["data"]["groups"].as_array().expect("groups 必须是数组");
    assert_eq!(groups[0]["key"], "overview", "固定 L1 顺序从概览开始");
    assert_eq!(groups[0]["href"], "/p/m/overview", "概览仍是直达链接");
    assert!(
        groups[0]["items"]
            .as_array()
            .expect("items 必须是数组")
            .is_empty(),
        "直达链接型 L1 没有子项"
    );

    let sites = groups
        .iter()
        .find(|g| g["key"] == "sites")
        .expect("网站分组必须存在");
    let site_items = sites["items"].as_array().expect("网站分组必须有子项");
    assert!(site_items.iter().any(|i| i["key"] == "core/sites/list"));
    assert!(site_items.iter().any(|i| i["key"] == "core/sites/env"));

    // 插件功能页（旧 SSR iframe）不再作为新产品入口：菜单里不得出现 plugin/* 键。
    for group in groups {
        if let Some(items) = group["items"].as_array() {
            for item in items {
                let key = item["key"].as_str().unwrap_or_default();
                assert!(
                    !key.starts_with("plugin/"),
                    "插件功能页入口不得下发到壳页菜单：{key}"
                );
            }
        }
    }
}

/// `/ui/` 下除 `/ui/v1/*` 之外的形态一律 404，不能当静态资源或页面旁路。
#[tokio::test]
async fn ui_namespace_other_shapes_are_404() {
    let h = harness();
    for uri in [
        "/ui",
        "/ui/",
        "/ui/v1",
        "/ui/v2/session",
        "/ui/v1/session/unknown",
    ] {
        let (status, _, _) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} 应 404");
    }
}

/// 白名单深链回落壳页：只给挂载点、占位提示与产物引用。
#[tokio::test]
async fn shell_fallback_serves_whitelisted_deep_links() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in [
        "/p/sites/example.com/open/detail",
        "/p/containers/abc/unknown",
        "/p/files/a/b",
        "/p/plugins/demo.plugin/view/deep",
        "/p/settings/deep",
        "/p/jobs/1/logs/deep",
    ] {
        let (status, ct, body) = get_ui(&h.app, uri, Some(&token), Some("text/html")).await;
        assert_eq!(status, StatusCode::OK, "{uri} 应回落壳页");
        assert!(ct.starts_with("text/html; charset=utf-8"), "{uri}");
        assert!(body.contains("该功能正在适配中"), "{uri} 应有占位提示");
        let ver = skiff_web::assets::BUDDING_VERSION;
        assert!(
            body.contains(&format!(
                r#"src="/p/static/budding/budding-{ver}.min.js?v={ver}""#
            )),
            "{uri} 应引用 budding 版本化产物"
        );
        assert!(
            body.contains(&format!(
                r#"data-budding-css="/p/static/budding/budding-{ver}.min.css?v={ver}""#
            )),
            "{uri} 应显式下发版本化 CSS 路径"
        );
    }
}

/// 白名单外、写请求、非 HTML `Accept` 都不回落（URL 契约 §2.1）。
#[tokio::test]
async fn shell_fallback_never_catches_unlisted_or_non_get() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let deep = "/p/sites/example.com/open/detail";

    // 路径段边界：`/p/sitesfoo` 不算 `/{p}/sites`。
    for uri in ["/p/sitesfoo", "/p/unknown"] {
        let (status, _, body) = get_ui(&h.app, uri, Some(&token), Some("text/html")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} 不应命中白名单");
        assert!(!body.contains("该功能正在适配中"));
    }

    // 写请求永不回落。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(deep)
                .header(header::ACCEPT, "text/html")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(!body_text(resp).await.contains("该功能正在适配中"));

    // 非 HTML 的 Accept 不回落（API/脚本取数据不应拿到壳页）。
    let (status, _, body) = get_ui(&h.app, deep, Some(&token), Some("application/json")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("该功能正在适配中"));
}

/// 命中「路径存在但方法不匹配」时：GET 导航仍回落壳页，写请求仍 405。
///
/// `/p/containers/{id}/{action}` 只注册了 POST，GET 旧实现直接 405，`Router::fallback`
/// 不会触发；`method_not_allowed_fallback` 必须补上这条路径。
#[tokio::test]
async fn shell_fallback_covers_method_not_allowed_paths() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let path = "/p/containers/abc/unknown";

    let (status, ct, body) = get_ui(&h.app, path, Some(&token), Some("text/html")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "POST-only 模式上的 GET 深链应回落壳页，而不是 405"
    );
    assert!(ct.starts_with("text/html; charset=utf-8"));
    assert!(body.contains("该功能正在适配中"));

    // 非 HTML Accept 不回落，保持 405。
    let (status, _, body) = get_ui(&h.app, path, Some(&token), Some("application/json")).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(!body.contains("该功能正在适配中"));

    // 写请求不回落：`/p/sites/new` 只注册 GET，POST 必须是 405。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/sites/new")
                .header(header::ACCEPT, "text/html")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(!body_text(resp).await.contains("该功能正在适配中"));
}

/// 壳页受会话保护：未登录的页面请求仍按页面惯例 303 到登录页。
#[tokio::test]
async fn shell_fallback_redirects_anonymous_to_login() {
    let h = harness();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/sites/example.com/open/detail")
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        location.starts_with("/p/login?next="),
        "应 303 到登录页，实际: {location}"
    );
}

/// `/ui/v1` 写路径：CSRF 缺失/错误必须 403 JSON，正确令牌才生效。
#[tokio::test]
async fn ui_logout_requires_csrf_header() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/session/logout")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "缺 CSRF 必须 403");
    assert!(resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("application/json"));
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");

    // 用 session 下发的令牌重试：成功并清 Cookie。
    let (_, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&token), None).await;
    let csrf = serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/session/logout")
                .header(
                    header::COOKIE,
                    format!("{}={token}", skiff_identity::session_cookie_name(true)),
                )
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(v["data"]["ok"], true);

    // 登出后同一会话失效：回到 401 JSON。
    let (status, _, _) = get_ui(&h.app, "/ui/v1/session", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---- budding 登录页（第三批）：未认证壳页 + /ui/v1/auth/login 读写接口 ----

/// 提交 budding 登录 JSON；`site` 模拟浏览器的 `Sec-Fetch-Site`。
async fn post_ui_login(
    app: &Router,
    body: &serde_json::Value,
    site: Option<&str>,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method("POST")
        .uri("/ui/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(site) = site {
        req = req.header("sec-fetch-site", site);
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// 未认证时登录页必须直接 200 渲染 budding 壳，不能落入「匿名 303 跳登录」的回落
/// 分支——否则会自己跳自己，形成死循环。
#[tokio::test]
async fn login_page_renders_budding_shell_without_session() {
    let h = harness();
    let resp = get_raw(&h.app, "/p/login?next=%2Fp%2Fsites%2Fa", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get(header::LOCATION).is_none(),
        "未认证登录页不得 303"
    );
    assert_eq!(header_str(&resp, "cache-control"), "no-store");
    let html = body_text(resp).await;
    assert!(html.contains(r#"id="skiff-login""#), "{html}");
    assert!(html.contains(r#"data-next="/p/sites/a""#), "{html}");
    let ver = skiff_web::assets::BUDDING_VERSION;
    assert!(
        html.contains(&format!(
            r#"src="/p/static/budding/budding-{ver}.min.js?v={ver}""#
        )),
        "登录页应引用版本化 budding 产物"
    );
    assert!(
        html.contains(r#"src="/p/static/budding/budding-login.js""#),
        "登录页应加载启动脚本"
    );
    assert!(!html.contains("<script>"), "CSP 禁止内联脚本");
}

/// `GET /ui/v1/auth/login` 未认证可读，签发与表单路径同一个签名 CSRF。
#[tokio::test]
async fn ui_login_config_is_public_and_issues_csrf() {
    let h = harness();
    let resp = get_raw(&h.app, "/ui/v1/auth/login", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(header_str(&resp, "content-type").starts_with("application/json"));
    assert_eq!(header_str(&resp, "cache-control"), "no-store");
    assert!(
        set_cookie_value(&resp, "skiff_login").is_some(),
        "应下发兼容 Cookie"
    );
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    let csrf = v["data"]["csrf_token"].as_str().unwrap();
    assert!(!csrf.is_empty());
    assert_eq!(v["data"]["entry_prefix"], "p");
    assert_eq!(v["data"]["api_version"], "v1");
    assert!(skiff_identity::verify_signed_token(
        &h.state.csrf_secret,
        "login",
        csrf,
        600,
        chrono::Utc::now().timestamp()
    )
    .is_ok());
}

/// 登录成功后返回 `redirect_to` 并下发与页面登录同形态的会话 Cookie。
#[tokio::test]
async fn ui_login_submit_authenticates_and_returns_redirect() {
    let h = harness();
    let (csrf, _) = login_page_tokens(&h.app).await;
    let resp = post_ui_login(
        &h.app,
        &serde_json::json!({
            "username": "admin",
            "password": "pw-test-123",
            "otp": "",
            "csrf": csrf,
            "next": "/p/sites/example.com/open"
        }),
        Some("same-origin"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let session = set_cookie_value(&resp, skiff_identity::session_cookie_name(true))
        .expect("登录成功应下发会话 Cookie");
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(v["data"]["redirect_to"], "/p/sites/example.com/open");
    assert_eq!(v["data"]["username"], "admin");
    assert_eq!(v["data"]["role"], "admin");

    // 回跳深链用同一条会话直接可用。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&session), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["username"], "admin");
}

/// 用户名/口令错误与空提交都必须 401，并沿用现有中文提示。
#[tokio::test]
async fn ui_login_submit_rejects_bad_and_empty_credentials() {
    let h = harness();
    for (username, password) in [("admin", "wrong-password"), ("", "")] {
        let (csrf, _) = login_page_tokens(&h.app).await;
        let resp = post_ui_login(
            &h.app,
            &serde_json::json!({
                "username": username,
                "password": password,
                "csrf": csrf,
            }),
            Some("same-origin"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{username:?}");
        let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated");
        assert_eq!(v["error"]["message"], "用户名或口令不正确");
    }
}

/// CSRF 缺失/伪造/过期、跨站来源都必须拒绝，错误码走既有信封。
#[tokio::test]
async fn ui_login_submit_requires_csrf_and_same_origin() {
    let h = harness();
    let body = |csrf: &str| {
        serde_json::json!({
            "username": "admin",
            "password": "pw-test-123",
            "csrf": csrf,
        })
    };

    let resp = post_ui_login(&h.app, &body("forged"), Some("same-origin")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");
    assert_eq!(v["error"]["message"], "CSRF 校验失败，请刷新页面后重试");

    let expired = skiff_identity::signed_token(
        &h.state.csrf_secret,
        "login",
        chrono::Utc::now().timestamp() - 601,
    );
    let resp = post_ui_login(&h.app, &body(&expired), Some("same-origin")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("登录页已过期"));

    let (csrf, _) = login_page_tokens(&h.app).await;
    let resp = post_ui_login(&h.app, &body(&csrf), Some("cross-site")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(v["error"]["message"], "跨站登录请求被拒绝");
}

/// TOTP 分支保留：启用 TOTP 后缺动态码失败且提示区分，正确动态码可登录。
#[tokio::test]
async fn ui_login_submit_preserves_totp_branch() {
    let h = harness();
    let secret = skiff_identity::generate_totp_secret();
    let user = h
        .state
        .db()
        .user_by_name("admin")
        .unwrap()
        .expect("admin 必须存在");
    h.state.save_totp_secret(user.id, &secret, true).unwrap();

    let (csrf, _) = login_page_tokens(&h.app).await;
    let resp = post_ui_login(
        &h.app,
        &serde_json::json!({
            "username": "admin",
            "password": "pw-test-123",
            "csrf": csrf,
            "otp": "",
        }),
        Some("same-origin"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(
        v["error"]["message"].as_str().unwrap().contains("动态码"),
        "TOTP 失败提示必须区分：{v}"
    );

    let code =
        skiff_identity::totp::totp_now(&secret, chrono::Utc::now().timestamp().max(0) as u64)
            .expect("应能生成动态码");
    let (csrf, _) = login_page_tokens(&h.app).await;
    let resp = post_ui_login(
        &h.app,
        &serde_json::json!({
            "username": "admin",
            "password": "pw-test-123",
            "csrf": csrf,
            "otp": code,
        }),
        Some("same-origin"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "正确动态码必须通过");
}

// ---- budding 概览页（第四批）：入口根壳页 + /ui/v1/overview 只读计数 ----

/// 入口根对已登录用户交付 budding 壳（概览由启动脚本渲染），不再返回 SSR 页面。
#[tokio::test]
async fn overview_root_serves_budding_shell_for_authenticated() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_page(&h.app, "/p", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
    assert!(
        html.contains(r#"src="/p/static/budding/budding-boot.js""#),
        "{html}"
    );
    assert!(
        !html.contains(r#"id="skiff-main-nav""#),
        "壳页不应再输出旧 SSR 导航"
    );
    // SSR 概览保留在菜单深链，插件概览槽位仍有落点。
    let (status, legacy) = get_page(&h.app, "/p/m/overview", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(legacy.contains(r#"id="skiff-main-nav""#), "{legacy}");
}

/// `GET /ui/v1/overview`：未认证 401；认证后返回冻结字段的真实计数。
#[tokio::test]
async fn ui_overview_requires_auth_and_returns_frozen_counts() {
    let h = harness();
    let (status, _, body) = get_ui(&h.app, "/ui/v1/overview", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    seed_declared_plugin(
        h.state.db().as_ref(),
        r#"id = "com.example.count"
name = "Count Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.2"
requires_full_sandbox = false
"#
        .to_string(),
        true,
    );

    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, content_type, body) = get_ui(&h.app, "/ui/v1/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("application/json"));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["plugins"]["total"], 1);
    assert_eq!(data["plugins"]["enabled"], 1);
    assert_eq!(data["plugins"]["disabled"], 0);
    assert_eq!(data["containers"]["total"], 0);
    assert_eq!(data["containers"]["running"], 0);
    assert_eq!(data["sites"]["total"], 0);
    assert_eq!(data["runtimes"]["abnormal"], 0);
    // 测试环境可能预置运行环境（无对应容器），未安装数应等于总数。
    assert_eq!(data["runtimes"]["uninstalled"], data["runtimes"]["total"]);
    // 测试环境非 Linux，Docker 不可用：状态走 `{code,label}` 双字段。
    assert_eq!(data["docker"]["status"]["code"], "unavailable");
    assert_eq!(data["docker"]["status"]["label"], "不可用");
    // D9：时间单字段 RFC3339 UTC。
    let collected = data["collected_at"].as_str().unwrap();
    assert!(collected.ends_with('Z'), "{collected}");
    assert!(chrono::DateTime::parse_from_rfc3339(collected).is_ok());
}

// ---- 站点 / 运行环境只读页（第一批）：JSON 列表 + keyset 游标 + 壳页回落 ----

/// `/ui/v1/sites`、`/ui/v1/sites/{domain}`、`/ui/v1/runtimes` 的鉴权与冻结字段。
/// 列表走 keyset 游标（D10）：`limit` 截断后返回 `next_cursor`，下一页从游标继续。
#[tokio::test]
async fn ui_sites_and_runtimes_lists_use_frozen_fields_and_keyset_cursor() {
    let h = harness();
    for uri in ["/ui/v1/sites", "/ui/v1/sites/demo.local", "/ui/v1/runtimes"] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }

    for domain in ["a.local", "b.local", "c.local"] {
        let root = h.data_dir.join(format!("www/sites/{domain}"));
        std::fs::create_dir_all(&root).unwrap();
        h.state
            .db()
            .create_site(&php_site(domain, &root.display().to_string()))
            .unwrap();
    }
    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, content_type, body) =
        get_ui(&h.app, "/ui/v1/sites?limit=2", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("application/json"));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["total"], 3);
    assert_eq!(data["items"].as_array().unwrap().len(), 2);
    let first = &data["items"][0];
    assert_eq!(first["site_type"], "runtime");
    assert_eq!(first["type_label"], "运行环境");
    assert!(first["status"]["code"].is_string());
    assert!(first["status"]["label"].is_string());
    assert!(first["https"]["label"].is_string());
    // D9：创建时间单字段 RFC3339 UTC。
    let created = first["created_at"].as_str().unwrap();
    assert!(created.ends_with('Z'), "{created}");
    assert!(chrono::DateTime::parse_from_rfc3339(created).is_ok());
    let cursor = data["next_cursor"].as_str().unwrap().to_string();

    let second_uri = format!("/ui/v1/sites?limit=2&cursor={cursor}");
    let (status, _, body) = get_ui(&h.app, &second_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["items"].as_array().unwrap().len(), 1);
    assert!(data["next_cursor"].is_null(), "最后一页不应再给游标");

    // 详情：列表字段 + 证书 + 运行环境状态，均为只读。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/sites/a.local", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["domain"], "a.local");
    assert!(data["ssl"].is_object());
    assert!(data["runtime_alert"].is_string());
    assert!(data["runtime_down"].is_boolean());

    // 运行环境列表：管理员可读，字段含安装状态与任务快照。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/runtimes", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert!(data["items"].is_array());
    for item in data["items"].as_array().unwrap() {
        assert!(item["kind"]["code"].is_string());
        assert!(item["kind"]["label"].is_string());
        assert!(item["state"]["code"].is_string());
        assert!(item["state"]["label"].is_string());
        let created = item["created_at"].as_str().unwrap();
        assert!(created.ends_with('Z'), "{created}");
    }
}

/// 容器只读接口在无 Docker 环境下的口径：列表 200 + 空态原因，详情 503，
/// 写操作严格校验 `x-csrf-token`（未带令牌 403 `auth.csrf_invalid`）。
#[tokio::test]
async fn ui_containers_keep_auth_csrf_and_docker_unavailable_semantics() {
    let h = harness();
    for uri in ["/ui/v1/containers", "/ui/v1/containers/abc"] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/containers", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert!(data["items"].as_array().unwrap().is_empty());
    assert_eq!(data["total"], 0);
    assert_eq!(data["docker_available"], false);
    assert!(data["can_operate"].is_boolean());
    assert!(data["can_delete"].is_boolean());

    // 无 Docker 时解析容器失败 → 503，与旧页面同口径。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/containers/abc", Some(&admin), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable");

    // 未带 CSRF 头的写请求必须 403，且不能触达 Docker 命令。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/containers/abc/start")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_text(resp).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");
}

/// 镜像/网络/卷只读接口：未认证 401；无 Docker 时 200 空态并给出原因与能力位。
#[tokio::test]
async fn ui_resource_lists_keep_auth_and_docker_unavailable_semantics() {
    let h = harness();
    for uri in ["/ui/v1/images", "/ui/v1/networks", "/ui/v1/volumes"] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }
    let admin = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/ui/v1/images", "/ui/v1/networks", "/ui/v1/volumes"] {
        let (status, _, body) = get_ui(&h.app, uri, Some(&admin), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let data = &v["data"];
        assert!(data["items"].as_array().unwrap().is_empty(), "{uri}");
        assert_eq!(data["total"], 0, "{uri}");
        assert_eq!(data["docker_available"], false, "{uri}");
        assert!(data["docker_reason"].is_string(), "{uri}");
        assert!(data["can_admin"].is_boolean(), "{uri}");
    }
}

/// 审计规范页：后端尚无关键字游标查询，交付占位壳页；旧 SSR 仍按权限拒绝 viewer。
#[tokio::test]
async fn audit_canonical_path_serves_placeholder_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/audit", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"id="skiff-root""#), "{html}");
    }
    let (status, _) = get_page(&h.app, "/p/m/core/settings/audit", &viewer).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "旧审计页对 viewer 应维持 AuditRead 拒绝（403），不得泄露数据"
    );
    let (status, ssr) = get_page(&h.app, "/p/m/core/settings/audit", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ssr.contains(r#"id="skiff-main-nav""#), "{ssr}");
}

/// 带会话与可选 CSRF 的 JSON POST，返回（状态码，响应体）。
async fn post_ui_json(
    app: &Router,
    uri: &str,
    token: &str,
    csrf: Option<&str>,
    body: &serde_json::Value,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("skiff_dev_session={token}"));
    if let Some(c) = csrf {
        req = req.header("x-csrf-token", c);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp).await)
}

/// 设置规范页交付 budding 壳页；旧 SSR 仍在 `/m/core/settings/security`。
#[tokio::test]
async fn settings_canonical_path_serves_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/settings", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"id="skiff-root""#), "{html}");
    }
    for token in [&admin, &viewer] {
        let (status, ssr) = get_page(&h.app, "/p/m/core/settings/security", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(ssr.contains(r#"id="skiff-main-nav""#), "{ssr}");
    }
}

/// 设置只读快照：未认证 401；字段冻结；信任公钥只对管理员返回。
#[tokio::test]
async fn ui_settings_returns_frozen_fields_and_scopes_keys() {
    let h = harness();
    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["panel"]["listen_addr"], "127.0.0.1");
    assert_eq!(data["panel"]["listen_port"], 18080);
    assert_eq!(data["panel"]["entry_prefix"], "p");
    assert!(data["panel"]["tls_mode"].is_string());
    assert!(data["panel"]["config_path"].is_string());
    assert!(data["panel"]["plaintext_behind_proxy"].is_boolean());
    assert!(data["panel"]["initial_admin_pending"].is_boolean());
    assert_eq!(data["account"]["totp_enabled"], false);
    assert_eq!(data["account"]["must_change_password"], false);
    assert_eq!(data["can_admin"], true);
    assert!(data["keys"].is_array());
    assert!(data["flash"]["notice"].is_string());
    assert!(data["flash"]["error"].is_string());

    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["can_admin"], false);
    assert!(
        data["keys"].as_array().unwrap().is_empty(),
        "viewer 不得看到信任公钥"
    );
}

/// 面板保存：CSRF 头、Admin 权限与字段校验都复用旧表单口径。
#[tokio::test]
async fn ui_settings_panel_update_keeps_csrf_and_admin_semantics() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);
    let body = serde_json::json!({
        "listen_addr": "127.0.0.1",
        "listen_port": "18080",
        "entry_prefix": "skiffx",
        "tls_mode": "auto",
        "tls_cert": "",
        "tls_key": ""
    });

    // 未带 CSRF 头：403 auth.csrf_invalid。
    let (status, text) = post_ui_json(&h.app, "/ui/v1/settings/panel", &admin, None, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");

    // viewer 即便带 CSRF 也无权写（Admin 权限）。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/panel",
        &viewer,
        Some(&viewer_csrf),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    // 字段非法：400 shape.invalid，不落盘。
    let invalid = serde_json::json!({
        "listen_addr": "127.0.0.1",
        "listen_port": "abc",
        "entry_prefix": "p",
        "tls_mode": "auto"
    });
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/panel",
        &admin,
        Some(&csrf),
        &invalid,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "shape.invalid");

    // 合法保存（地址/端口不变，仅验证复用旧 handler 的成功路径与提示）。
    let (status, text) =
        post_ui_json(&h.app, "/ui/v1/settings/panel", &admin, Some(&csrf), &body).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["data"]["message"].as_str().unwrap().contains("已写入"),
        "{text}"
    );
}

/// 改密：旧口令校验失败即 400；成功后会话轮换，旧 Cookie 立即失效。
#[tokio::test]
async fn ui_password_change_rotates_session() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);

    let wrong = serde_json::json!({
        "old_password": "not-the-password",
        "new_password": "new-pw-456",
        "confirm_password": "new-pw-456"
    });
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/password",
        &admin,
        Some(&csrf),
        &wrong,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("当前口令不正确"), "{text}");

    let mismatch = serde_json::json!({
        "old_password": "pw-test-123",
        "new_password": "new-pw-456",
        "confirm_password": "new-pw-789"
    });
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/password",
        &admin,
        Some(&csrf),
        &mismatch,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("两次输入的新口令不一致"), "{text}");

    let ok = serde_json::json!({
        "old_password": "pw-test-123",
        "new_password": "new-pw-456",
        "confirm_password": "new-pw-456"
    });
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/settings/password")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", &csrf)
                .body(Body::from(ok.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let cookie = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("skiff_dev_session="))
        .expect("改密成功必须回写会话 Cookie")
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("skiff_dev_session=")
        .to_string();
    let text = body_text(resp).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["data"]["csrf_token"].as_str().unwrap().len() > 10);

    let (status, _, _) = get_ui(&h.app, "/ui/v1/session", Some(&admin), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "旧会话必须失效");
    let (status, _, _) = get_ui(&h.app, "/ui/v1/session", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "新会话必须可用");
}

/// TOTP：setup 生成密钥并在 GET /settings 中可再次取回；错误动态码不得启用。
#[tokio::test]
async fn ui_totp_setup_exposes_pending_secret_and_rejects_bad_code() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/totp/setup",
        &admin,
        Some(&csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let secret = v["data"]["secret"].as_str().unwrap().to_string();
    assert!(secret.len() >= 16, "{text}");
    assert!(v["data"]["otpauth_uri"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://"));

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["account"]["totp_pending_secret"], secret);
    assert_eq!(
        v["data"]["account"]["totp_enabled"],
        serde_json::Value::Bool(false)
    );

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/totp/enable",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "code": "000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("动态码不正确"), "{text}");
}

/// 信任公钥：非管理员 403，管理员非法公钥 400（合法导入走既有 TOFU 用例）。
#[tokio::test]
async fn ui_settings_keys_enforce_admin_and_key_shape() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);
    let body = serde_json::json!({ "public_key": "zz", "label": "x" });

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/keys",
        &viewer,
        Some(&viewer_csrf),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    let (status, text) =
        post_ui_json(&h.app, "/ui/v1/settings/keys", &admin, Some(&csrf), &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("公钥非法"), "{text}");
}

/// 文件管理只读列表：管理员可平铺浏览（父路径、根入口、绝对路径、RFC3339 时间），
/// viewer 403；不提供树形字段。
#[tokio::test]
async fn ui_files_list_is_flat_and_admin_only() {
    let h = harness();
    // harness 的 data_dir 是受保护路径，文件管理不能访问；用独立临时目录当浏览目标。
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    std::fs::create_dir_all(root.join("sites/sub")).unwrap();
    std::fs::write(root.join("note.txt"), b"hello").unwrap();
    let uri = format!(
        "/ui/v1/files?path={}",
        root.to_string_lossy().replace('\\', "/")
    );

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("files 响应必须是 JSON");
    let data = &v["data"];
    assert!(
        data["path"]
            .as_str()
            .unwrap_or_default()
            .replace('\\', "/")
            .ends_with(root.file_name().unwrap().to_string_lossy().as_ref()),
        "必须回显浏览目录: {body}"
    );
    assert!(data["parent"].is_string(), "面包屑需要父路径: {body}");
    assert!(
        data["roots"].is_array(),
        "根入口必须下发（数组；测试环境 Web 根未落盘时可为空）: {body}"
    );
    assert!(
        data["upload_chunk_bytes"].as_u64().unwrap_or(0) > 0,
        "前端上传适配器需要分片大小: {body}"
    );
    let entries = data["entries"].as_array().expect("entries 必须是数组");
    let sites = entries
        .iter()
        .find(|e| e["name"] == "sites")
        .expect("目录应平铺出现");
    assert_eq!(sites["dir"], true);
    assert!(
        sites["path"]
            .as_str()
            .unwrap_or_default()
            .replace('\\', "/")
            .ends_with("sites"),
        "条目必须带绝对路径: {sites}"
    );
    let note = entries
        .iter()
        .find(|e| e["name"] == "note.txt")
        .expect("文件应平铺出现");
    assert_eq!(note["dir"], false);
    assert_eq!(note["size"], 5);
    assert!(
        note["mtime"].is_string() || note["mtime"].is_null(),
        "时间只出 RFC3339 或 null: {note}"
    );
    assert!(note.get("children").is_none(), "不得下发树形子项");

    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, _) = get_ui(&h.app, "/ui/v1/files", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "文件管理仅管理员");
}

/// 代码查看器：只读文本接口，管理员可取内容，viewer 403；截断标记与上限随响应下发。
#[tokio::test]
async fn ui_files_read_returns_text_and_requires_admin() {
    let h = harness();
    let scratch = tempfile::tempdir().unwrap();
    let file = scratch.path().join("hello.rs");
    std::fs::write(&file, "fn main() {}\n").unwrap();
    let uri = format!(
        "/ui/v1/files/read?path={}",
        file.display().to_string().replace('\\', "/")
    );

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["text"], "fn main() {}\n");
    assert_eq!(v["data"]["size"], 13);
    assert_eq!(v["data"]["truncated"], false);
    assert!(v["data"]["max_edit_bytes"].as_u64().unwrap_or(0) > 0);

    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, _) = get_ui(&h.app, &uri, Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "代码查看器仅管理员");
}

/// 站点日志查看器数据：只出尾部窗口、最新在前，关键字只过滤该窗口。
#[tokio::test]
async fn ui_site_logs_json_lists_tail_and_filters() {
    let h = harness();
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    std::fs::create_dir_all(root.join("log")).unwrap();
    std::fs::write(
        root.join("log/access.log"),
        "old-line\nkeep-abc-line\nnew-line\n",
    )
    .unwrap();
    h.state
        .db()
        .create_site(&static_site("logs.local", &root.display().to_string()))
        .unwrap();

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/sites/logs.local/logs?tail=2",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let lines = v["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "tail=2 只取最后两行: {body}");
    assert_eq!(lines[0], "new-line", "最新在前: {body}");
    assert!(v["data"]["log_dir"].as_str().unwrap_or("").ends_with("log"));

    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/sites/logs.local/logs?q=abc",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let lines = v["data"]["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 1, "关键字只命中一行: {body}");
    assert_eq!(lines[0], "keep-abc-line");

    // 与旧 SSR 页面同口径：登录即可读，viewer 也能查看。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, _) = get_ui(&h.app, "/ui/v1/sites/logs.local/logs", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK);
}

/// 站点日志页：默认交 budding 壳（查看器），`?legacy=1` 仍可拿到旧 SSR。
#[tokio::test]
async fn site_logs_page_serves_shell_unless_legacy() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/sites/logs.local/logs", &admin).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
    let (status, legacy) = get_html(&h.app, "/p/sites/logs.local/logs?legacy=1", &admin).await;
    // 站点不存在时旧 SSR 返回 400；只要不返回壳页即可。
    if status == StatusCode::OK {
        assert!(
            !legacy.contains(r#"id="skiff-root""#),
            "legacy 请求不得返回壳页"
        );
    } else {
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{status}"
        );
    }
}

/// 容器终端页交 budding 壳；真正的 WS 连接需 Linux 测试机，本机不做断言。
#[tokio::test]
async fn container_terminal_page_serves_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/containers/abc123/terminal", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
}

/// 文件写动作：mkdir → rename → remove（进回收站）闭环；无 CSRF 头 403 且无副作用。
#[tokio::test]
async fn ui_files_mkdir_rename_remove_roundtrip() {
    let h = harness();
    // harness 的 data_dir 是受保护路径；写动作在独立临时目录里做闭环。
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let dir = root.join("newdir");
    let dir_json = dir.display().to_string();

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/mkdir",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "path": dir_json }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(dir.is_dir(), "mkdir 必须真正建目录");

    let renamed = root.join("renamed");
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/rename",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "from": dir.display().to_string(),
            "to": renamed.display().to_string()
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(renamed.is_dir() && !dir.exists());

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/remove",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "path": renamed.display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["data"]["trash_id"].is_string(),
        "删除必须进回收站: {text}"
    );
    assert!(!renamed.exists());

    let other = root.join("no-csrf");
    let (status, _) = post_ui_json(
        &h.app,
        "/ui/v1/files/mkdir",
        &admin,
        None,
        &serde_json::json!({ "path": other.display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "写动作必须校验 CSRF");
    assert!(!other.exists(), "CSRF 失败不得产生副作用");
}

/// 商店规范页交付 budding 壳页；旧 SSR 仍在 `/m/core/plugins/store`（目录启用时 200）。
#[tokio::test]
async fn store_canonical_path_serves_shell() {
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            ..Default::default()
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/store", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
    let (status, ssr) = get_page(&h.app, "/p/m/core/plugins/store", &token).await;
    assert_eq!(status, StatusCode::OK, "{ssr}");
}

/// 商店只读接口：未认证 401；目录未启用 404；启用后返回冻结字段与四个分区。
#[tokio::test]
async fn ui_store_keeps_auth_and_frozen_fields() {
    let h = harness();
    let (status, _, body) = get_ui(&h.app, "/ui/v1/store", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/store", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "lookup.not_found");

    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            ..Default::default()
        },
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/store", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert!(data["state"]["code"].is_string());
    assert!(data["state"]["label"].is_string());
    assert!(data["message"].is_string());
    assert_eq!(data["channel"], "stable");
    assert!(data["key_configured"].is_boolean());
    assert_eq!(data["can_install"], data["key_configured"]);
    assert_eq!(data["total"], 0);
    assert_eq!(data["sections"].as_array().unwrap().len(), 4);
    for (i, title) in ["插件", "应用模板", "运行环境", "升级通道"]
        .iter()
        .enumerate()
    {
        assert_eq!(data["sections"][i]["title"], *title);
        assert!(data["sections"][i]["items"].is_array());
    }
    assert!(data["upgrade"]["current"].is_string());
    assert!(data["upgrade"]["can_download"].is_boolean());
}

/// 商店安装与确认：CSRF、PluginManage 权限和确认令牌校验都复用旧链路。
#[tokio::test]
async fn ui_store_install_and_confirm_keep_csrf_and_permission_semantics() {
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            ..Default::default()
        },
    );
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    // 未带 CSRF：403 auth.csrf_invalid。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/plugins/com.example.demo/install",
        &admin,
        None,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");

    // viewer 无 PluginManage：403 auth.forbidden。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/plugins/com.example.demo/install",
        &viewer,
        Some(&viewer_csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    // 确认接口：令牌非法时 400，且绝不触碰暂存目录。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/plugins/confirm",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "token": "bad-token" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "shape.invalid");
}

/// 插件中心与信息页规范路径交付壳页；旧 SSR 仍在 `/m/core/plugins/*`。
#[tokio::test]
async fn plugins_canonical_paths_serve_shell() {
    let h = harness();
    seed_plugin(h.state.db(), "com.example.ui", "示例界面", "示例", true);
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/p/plugins", "/p/plugins/com.example.ui"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
    }
    for uri in [
        "/p/m/core/plugins/installed",
        "/p/m/core/plugins/info/com.example.ui",
    ] {
        let (status, ssr) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(ssr.contains(r#"id="skiff-main-nav""#), "{uri}: {ssr}");
    }
}

/// 插件列表与信息页只读接口：未认证 401；冻结字段；未知插件 404。
#[tokio::test]
async fn ui_plugins_list_and_info_freeze_fields() {
    let h = harness();
    seed_plugin(h.state.db(), "com.example.ui", "示例界面", "示例", true);
    let (status, _, body) = get_ui(&h.app, "/ui/v1/plugins", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/plugins", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["enabled_count"], 1);
    assert_eq!(data["disabled_count"], 0);
    assert_eq!(data["can_admin"], true);
    assert_eq!(data["plugins"][0]["id"], "com.example.ui");
    assert_eq!(data["plugins"][0]["status"]["code"], "enabled");
    assert_eq!(data["plugins"][0]["status"]["label"], "已启用 · 未运行");
    assert!(data["plugins"][0]["key_id"].is_string());
    assert!(data["plugins"][0]["ui_path"].is_string());

    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/plugins", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["can_admin"], false);

    let (status, _, body) =
        get_ui(&h.app, "/ui/v1/plugins/com.example.ui", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["plugin"]["id"], "com.example.ui");
    assert!(data["entry_href"]
        .as_str()
        .unwrap()
        .starts_with("/p/plugins/"));
    assert!(data["menus"].is_array());
    assert!(data["grants"].is_array());
    assert_eq!(data["can_admin"], true);

    let (status, _, body) = get_ui(&h.app, "/ui/v1/plugins/nope", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "lookup.not_found");
}

/// 插件启停/卸载：CSRF 与 PluginManage 权限复用旧 handler 口径。
#[tokio::test]
async fn ui_plugin_toggle_and_delete_keep_csrf_and_permission_semantics() {
    let h = harness();
    seed_plugin(h.state.db(), "com.example.off", "停用示例", "示例", false);
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/plugins/com.example.off/toggle",
        &admin,
        None,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/plugins/com.example.off/toggle",
        &viewer,
        Some(&viewer_csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/plugins/com.example.off/toggle",
        &admin,
        Some(&csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["enabled"], true);

    // 卸载：无权限时在触碰文件系统之前就被拒绝。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/plugins/com.example.off/delete",
        &viewer,
        Some(&viewer_csrf),
        &serde_json::json!({ "delete_data": false }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");
}

/// 文件管理规范页交付 budding 壳页（原生平铺目录）；旧 SSR（浏览/回收站/配额）仍在 `/m/core/files`。
#[tokio::test]
async fn files_canonical_path_serves_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/files", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"id="skiff-root""#), "{html}");
    }
    // 旧 SSR 仍按 FILE_BROWSE 授权：admin 可进，viewer 403。
    let (status, ssr) = get_page(&h.app, "/p/m/core/files", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ssr.contains(r#"class="workbench-tabs""#), "{ssr}");
    let (status, _) = get_page(&h.app, "/p/m/core/files", &viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// 任务/作业只读接口：未认证 401；列表返回冻结字段；不存在的详情 404。
#[tokio::test]
async fn ui_tasks_and_jobs_keep_auth_and_read_only_shapes() {
    let h = harness();
    for uri in [
        "/ui/v1/tasks",
        "/ui/v1/tasks/missing",
        "/ui/v1/jobs",
        "/ui/v1/jobs/missing",
    ] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }
    let admin = login(&h.app, "admin", "pw-test-123").await;

    // 测试装配未启动调度器：任务接口与旧 SSR 一致返回 503，不得伪造空列表。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/tasks", Some(&admin), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable");

    let (status, _, _) = get_ui(&h.app, "/ui/v1/tasks/missing", Some(&admin), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _, body) = get_ui(&h.app, "/ui/v1/jobs", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["items"].is_array());

    let (status, _, _) = get_ui(&h.app, "/ui/v1/jobs/missing", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 任务/作业规范页交付壳页；旧 SSR 仍在 `/m/core/tasks|jobs`。
#[tokio::test]
async fn tasks_and_jobs_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in [
        "/p/tasks",
        "/p/tasks/builtin:db-vacuum",
        "/p/jobs",
        "/p/jobs/abc",
    ] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
    for uri in ["/p/m/core/jobs"] {
        let (status, html) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-main-nav""#),
            "{uri} 旧 SSR 必须保留: {html}"
        );
    }
    // 测试装配未启动调度器，任务旧 SSR 与 JSON 接口同为 503。
    let (status, _) = get_page(&h.app, "/p/m/core/tasks", &token).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

/// compose 只读接口与写操作：未认证 401；无 Docker 时项目为空；
/// 项目动作与 down 必须先过 `x-csrf-token`。
#[tokio::test]
async fn ui_compose_keeps_auth_csrf_and_docker_unavailable_semantics() {
    let h = harness();
    let (status, _, body) = get_ui(&h.app, "/ui/v1/compose", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/compose", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert!(data["projects"].as_array().unwrap().is_empty());
    assert!(data["services"].as_array().unwrap().is_empty());
    assert_eq!(data["docker_available"], false);
    assert!(data["can_admin"].is_boolean());

    for (method, uri) in [
        ("POST", "/ui/v1/compose/demo/start"),
        ("POST", "/ui/v1/compose/down"),
    ] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"project":"demo","remove_volumes":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{uri}");
        let body = body_text(resp).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }
}

/// compose 规范页交付壳页；旧 SSR 仍在 `/m/core/docker/compose`。
#[tokio::test]
async fn compose_canonical_path_serves_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/compose", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
    let (status, ssr) = get_page(&h.app, "/p/m/core/docker/compose", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(ssr.contains(r#"id="skiff-main-nav""#), "{ssr}");
}

/// 镜像/网络/卷规范页交付壳页；旧 SSR 仍在 `/m/core/docker/*`。
#[tokio::test]
async fn resource_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/p/images", "/p/networks", "/p/volumes"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
    for uri in [
        "/p/m/core/docker/images",
        "/p/m/core/docker/networks",
        "/p/m/core/docker/volumes",
    ] {
        let (status, html) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-main-nav""#),
            "{uri} 旧 SSR 必须保留: {html}"
        );
    }
}

/// 容器规范页交付壳页；旧 SSR 仍在 `/m/core/docker/containers`。
#[tokio::test]
async fn containers_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/p/containers", "/p/containers/abc"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
    let (status, html) = get_page(&h.app, "/p/m/core/docker/containers", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"id="skiff-main-nav""#),
        "旧容器列表必须保留在 /m 深链: {html}"
    );
}

/// 站点/运行环境规范页交付壳页；旧 SSR 仍在 `/{p}/m/core/sites/*` 菜单深链。
#[tokio::test]
async fn sites_and_runtimes_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in [
        "/p/sites",
        "/p/sites/a.local",
        "/p/runtimes",
        "/p/runtimes/php-8.5",
    ] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
    for uri in ["/p/m/core/sites/list", "/p/m/core/sites/env"] {
        let (status, html) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-main-nav""#),
            "{uri} 应保留 SSR: {html}"
        );
    }
}

// ---- budding 产物入库：D11 缓存契约与壳页引用可解析 ----

/// 原始 GET（需自读响应头时用；`get_ui` 只返回状态码/类型/正文）。
async fn get_raw(app: &Router, uri: &str, headers: &[(&str, &str)]) -> axum::response::Response {
    let mut req = Request::builder().uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn header_str(resp: &axum::response::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// HTML 属性里以 `attr="..."` 形式出现的值（壳页引用解析用）。
fn attr_values(html: &str, attr: &str) -> Vec<String> {
    let marker = format!("{attr}=\"");
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&marker) {
        rest = &rest[pos + marker.len()..];
        match rest.find('"') {
            Some(end) => out.push(rest[..end].to_string()),
            None => break,
        }
    }
    out
}

/// 版本化产物 immutable、稳定别名 no-cache，两者同一份字节 → 同一强 ETag；
/// gzip 变体与 identity 是两个表示（ETag 不同）。
#[tokio::test]
async fn budding_static_assets_follow_d11_cache_contract() {
    let h = harness();
    let ver = skiff_web::assets::BUDDING_VERSION;
    let js_v = format!("/p/static/budding/budding-{ver}.min.js");
    let js_a = "/p/static/budding/budding.min.js".to_string();
    let css_v = format!("/p/static/budding/budding-{ver}.min.css");
    let css_a = "/p/static/budding/budding.css".to_string();

    for (uri, immutable) in [
        (js_v.as_str(), true),
        (css_v.as_str(), true),
        (js_a.as_str(), false),
        (css_a.as_str(), false),
    ] {
        let resp = get_raw(&h.app, uri, &[]).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri} 应 200");
        let cc = header_str(&resp, "cache-control");
        if immutable {
            assert_eq!(cc, "public, max-age=31536000, immutable", "{uri}");
        } else {
            assert_eq!(cc, "no-cache", "{uri}");
        }
        let etag = header_str(&resp, "etag");
        assert!(
            etag.starts_with('"') && etag.ends_with('"') && etag.len() == 66,
            "{uri} 必须是强 ETag，实际: {etag}"
        );
        let ct = header_str(&resp, "content-type");
        assert!(
            ct.starts_with(if uri.ends_with(".css") {
                "text/css"
            } else {
                "text/javascript"
            }),
            "{uri} Content-Type 不对: {ct}"
        );
    }

    // 别名与版本化指向同一份内嵌字节：ETag 必须相同（不复制第二份文件）。
    let versioned = get_raw(&h.app, js_v.as_str(), &[]).await;
    let alias = get_raw(&h.app, js_a.as_str(), &[]).await;
    let etag = header_str(&versioned, "etag");
    assert_eq!(etag, header_str(&alias, "etag"));

    // If-None-Match 命中 304，ETag/Cache-Control 原样保留。
    let resp = get_raw(&h.app, js_v.as_str(), &[("if-none-match", &etag)]).await;
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_str(&resp, "etag"), etag);
    assert_eq!(
        header_str(&resp, "cache-control"),
        "public, max-age=31536000, immutable"
    );

    // gzip 变体：有 Content-Encoding，ETag 与 identity 不同，字节确实是 gzip。
    let gz = get_raw(&h.app, js_v.as_str(), &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz.status(), StatusCode::OK);
    assert_eq!(header_str(&gz, "content-encoding"), "gzip");
    let gz_etag = header_str(&gz, "etag");
    assert_ne!(gz_etag, etag, "gzip 与 identity 的 ETag 必须不同");
    let bytes = axum::body::to_bytes(gz.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.len() > 2 && bytes[0] == 0x1f && bytes[1] == 0x8b);
    let gz_alias = get_raw(&h.app, js_a.as_str(), &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz_etag, header_str(&gz_alias, "etag"));
}

/// 壳页里引用的 JS/CSS（含 data 属性里的 CSS 与启动脚本）都必须真实可 200：
/// 防「引用写错、上线才 404」。
#[tokio::test]
async fn shell_html_asset_references_resolve() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, html) = get_ui(
        &h.app,
        "/p/files/deep/link",
        Some(&token),
        Some("text/html"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut uris: Vec<String> = attr_values(&html, "src");
    uris.extend(attr_values(&html, "data-budding-css"));
    uris.retain(|u| u.starts_with("/p/static/"));
    assert!(
        uris.len() >= 3,
        "壳页应引用产物 JS、启动脚本与 CSS: {uris:?}"
    );
    for uri in uris {
        let resp = get_raw(&h.app, &uri, &[]).await;
        assert_eq!(resp.status(), StatusCode::OK, "壳页引用必须可取: {uri}");
    }
}
