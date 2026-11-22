//! 主干集成测试（跨 crate 契约守门人）：登录 → 会话 → RBAC → 插件上传/信任/确认安装。
//!
//! **如实标注的缺口**：
//! - 「建容器」主干不在本文件：需要 Docker 引擎（Windows 开发机 / CI 无守护进程），
//!   该路径只能在 Linux 真机验证（见 docs/test/DEPLOY_TEST.md 未验证项）。
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

use serde_json::json;

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

struct Harness {
    app: Router,
    state: Arc<AppState>,
    _data: tempfile::TempDir,
    data_dir: PathBuf,
}

fn harness() -> Harness {
    harness_full(Form::Native, Tier::Basic)
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

/// GET 登录配置，返回（签名 CSRF token，Set-Cookie 下发的 `skiff_login` 值）。
///
/// budding 登录页通过 `GET /ui/v1/auth/login` 取 token，提交走
/// `POST /ui/v1/auth/login`（同一个 `skiff_login` Cookie 仍是双提交兼容信号）。
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
        .uri("/ui/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, format!("skiff_login={c}"));
    }
    if let Some(site) = sec_fetch_site {
        req = req.header("sec-fetch-site", site);
    }
    let payload = serde_json::json!({
        "username": username,
        "password": password,
        "otp": "",
        "csrf": csrf,
        "next": "",
    });
    app.clone()
        .oneshot(req.body(Body::from(payload.to_string())).unwrap())
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
    assert_eq!(resp.status(), StatusCode::OK, "登录应返回 200 JSON 信封");
    set_cookie_value(&resp, skiff_identity::session_cookie_name(true))
        .expect("登录成功应下发会话 Cookie")
}

/// 指定 manifest `api` 的签名包（兼容门测试用）。
fn build_package_api(root: &std::path::Path, api: &str) -> (PathBuf, String, String) {
    build_package_full(root, "docker", "docker", api)
}

fn build_package_full(
    root: &std::path::Path,
    requires: &str,
    cap: &str,
    api: &str,
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
api = "{api}"
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

/// 把插件包放进安装暂存区，返回 24 位确认令牌。
///
/// 上传入口只由 budding 壳页（`PlatformAdapter.upload`）走浏览器通道，测试直接
/// 复用同一条落点：`{data_dir}/plugins/.incoming/{token}.wbpkg`。
fn stage_pkg(data_dir: &std::path::Path, pkg: &std::path::Path) -> String {
    let token: String = skiff_identity::new_token()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(24)
        .collect();
    let incoming = data_dir.join("plugins").join(".incoming");
    std::fs::create_dir_all(&incoming).unwrap();
    std::fs::copy(pkg, incoming.join(format!("{token}.wbpkg"))).unwrap();
    token
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
                .uri("/p")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "带会话访问概览壳页应 200");
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
    assert_eq!(resp.status(), StatusCode::OK);
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
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_text(resp).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("登录响应必须是 JSON");
    assert_eq!(
        value["data"]["redirect_to"], "/p",
        "默认不得跳到 settings?force=1"
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
    assert_eq!(resp.status(), StatusCode::OK);
    let token = set_cookie_value(&resp, skiff_identity::session_cookie_name(true))
        .expect("登录成功应下发会话 Cookie");
    let body = body_text(resp).await;
    let value: serde_json::Value = serde_json::from_str(&body).expect("登录响应必须是 JSON");
    assert_eq!(value["data"]["redirect_to"], "/p/settings/security?force=1");

    // 后续页面仍被改密闸门拦下（303 回到设置页）。
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
        "/p/settings/security?force=1"
    );
}

/// 回归：登录后带会话访问 settings / 容器页必须是 200，而不是 303 回登录页。
#[tokio::test]
async fn session_survives_login_redirect_and_reaches_pages() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let cookie = format!("{}={token}", skiff_identity::session_cookie_name(true));
    for path in [
        "/p",
        "/p/settings/security?force=1",
        "/p/settings/security",
        "/p/docker/containers",
    ] {
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
                .uri("/p")
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
    assert_eq!(resp.status(), StatusCode::OK, "旧页面 token 应仍然有效");

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
    assert_eq!(resp.status(), StatusCode::OK);
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
    assert_eq!(resp.status(), StatusCode::OK, "curl 兼容路径应仍然可用");
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
    // 菜单只由 `/ui/v1/menu` 下发（壳页不再渲染菜单 HTML）。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("menu 必须是 JSON");
    let groups = v["data"]["groups"].as_array().expect("groups 必须是数组");
    let sites = groups
        .iter()
        .find(|g| g["key"] == "sites")
        .expect("网站一级必须存在");
    let site_items = sites["items"].as_array().expect("网站一级必须有子项");
    let files = site_items
        .iter()
        .find(|i| i["plugin_id"] == "com.skiff.files")
        .expect("站点文件应挂在「网站」下");
    assert_eq!(files["label"], "站点文件", "{body}");
    assert!(
        files["href"]
            .as_str()
            .unwrap_or_default()
            .starts_with("/p/plugins/com.skiff.files/"),
        "站点文件入口应是原生插件页: {body}"
    );
    assert!(
        !site_items.iter().any(|i| i["href"] == "/p/files"),
        "站点文件不得与内核文件管理同组: {body}"
    );
    let tools = groups
        .iter()
        .find(|g| g["key"] == "tools")
        .expect("工具一级必须存在");
    assert!(
        tools["items"]
            .as_array()
            .expect("工具一级必须有子项")
            .iter()
            .any(|i| i["href"] == "/p/files"),
        "内核「工具 → 文件管理」入口必须保留: {body}"
    );
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
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "口令错误必须是 401"
    );
    let text = body_text(resp).await;
    assert!(text.contains("不正确"), "错误响应应提示口令不正确: {text}");
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
                .uri("/p/docker/containers")
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
                .uri("/ui/v1/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
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
                .uri("/ui/v1/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
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

/// 目录不可用（无镜像/无缓存）时拒绝安装：失败原因落审计、按输入错误拒绝、不留暂存文件。
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
                .uri("/ui/v1/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "目录不可用必须按输入错误拒绝"
    );
    let text = body_text(resp).await;
    assert!(!text.is_empty(), "失败必须给出可读原因");
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
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
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
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
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

/// 渠道没有升级数据时安静拒绝：落 `not_found` 审计、按输入错误拒绝、不留暂存目录文件。
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
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/store/upgrades/0.2.0/download")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "无升级数据必须按输入错误拒绝"
    );
    let text = body_text(resp).await;
    assert!(!text.is_empty(), "失败必须给出可读原因");
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
/// 安装确认同样走内核 API 兼容门：不兼容包在落库之前就返回 400，
/// 并给出「要求 / 当前」两端的明确文案（稳定错误码 `shape.plugin_api`）。
#[tokio::test]
async fn plugin_confirm_rejects_incompatible_kernel_api() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let (pkg, pubkey, kid) = build_package_api(&h.data_dir, "1.4");
    h.state.db().trust_key(&kid, &pubkey, "测试作者").unwrap();
    let staged = stage_pkg(&h.data_dir, &pkg);

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/ui/v1/plugins/confirm")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .header("x-csrf-token", csrf.as_str())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "token": staged, "grant": [], "resource": [] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let text = body_text(resp).await;
    assert!(
        text.contains("要求内核 API 1.4") && text.contains("当前内核为 1.3"),
        "不兼容包必须说明两端 API 版本: {text}"
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

/// `/api/v1/docker/containers/{id}/restart`：执行与审计改走
/// `commands::docker::container_action` 后，响应仍保持原契约。
///
/// 无 Docker 时解析容器即失败，因此这里锁定的是「解析失败 → 503
/// dependency.unavailable」；审计 target/detail 的形状由命令层单测
/// `action_audit_shape_is_stable` 覆盖。
#[tokio::test]
async fn api_container_restart_keeps_response_contract() {
    let h = harness();
    let admin_id = h
        .state
        .db()
        .user_by_name("admin")
        .unwrap()
        .expect("harness 应建好 admin")
        .id;
    let token = "tok-docker-write";
    h.state
        .db()
        .create_api_token(
            "t-docker-write",
            admin_id,
            "docker-write",
            &skiff_identity::sha256_hex(token),
            "docker:write",
        )
        .unwrap();

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/docker/containers/abc/restart")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_text(resp).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable", "{body}");
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

/// 浏览器默认探测 `/favicon.ico`：必须 200，且缓存策略可被浏览器复用一天。
/// 页面同时用 `<link rel="icon">` 指向带版本查询的静态 SVG。
#[tokio::test]
async fn favicon_is_served_with_reasonable_cache() {
    let h = harness();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/favicon.ico")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cc = resp
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = body_text(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("image/svg+xml"));
    assert!(
        cc.unwrap_or_default().contains("max-age=86400"),
        "favicon 应允许缓存一天"
    );
    assert!(
        body.contains(r#"viewBox="0 0 32 32""#) && body.contains("<svg"),
        "favicon 必须是可缩放矢量图标"
    );

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/static/favicon.svg?v=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=31536000, immutable"),
        "带版本查询的静态图标应走 immutable"
    );
}

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
/// 插件功能页的两条 UI 约定（跨插件跳转 + 顶栏插件菜单）：
/// 1. `?open=` 只作为「定位建议」被清洗后原样透传（控制字符清掉、百分号编码）；
/// 2. `data-plugins` 白名单只含「已启用且有 UI 入口」的插件——iframe 的
///    `skiff.open` 只能往这些目标跳，跳转本身不授予任何能力。
///
/// 信息页与功能页的拆分见 `docs/contract/PLUGIN_UI_CONTRACT.md`：`/plugins/{id}` 只由
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

    // 信息与管理数据由 `/ui/v1/plugins/{id}` 承载：身份 / 授权 / 管理动作与功能页入口
    // 在同一份快照里，工作台不嵌信息页。
    let (status, _, body) =
        get_ui(&h.app, "/ui/v1/plugins/com.example.ui", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["can_admin"], true);
    assert!(data["grants"].is_array(), "{body}");
    assert_eq!(data["entry_href"], "/p/plugins/com.example.ui/main");
    assert!(
        body.contains("/p/plugins/com.example.ui/legacy"),
        "关联菜单深链必须给出: {body}"
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
    // 壳层导航（顶栏应用菜单）由 budding 壳页负责，工作台宿主页只渲染 L3 标签栏与
    // iframe：不得再出现旧的顶栏菜单或启停表单。
    assert!(!html.contains(r#"class="nav-plugin"#), "{html}");
    assert!(
        !html.contains(r#"action="/p/plugins/com.example.ui/toggle""#),
        "工作台不得承载停用/启用动作: {html}"
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
    skiff_app::menu::cleanup_plugin_prefs(h.state.db().as_ref(), "com.skiff.nginx");
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
    let base = "/p/plugins/com.example.tabs";

    // 默认 L2：选中第一个可见 L3，iframe 同时带 `menu`（完整稳定键）与 `tab`。
    let (status, html) = get_page(&h.app, &format!("{base}/sites?legacy=1"), &admin).await;
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
    // 标签 href 指向原生视图路由（无 `?legacy=1`）：壳页在 schema 404 时自己补回参数。
    assert!(
        html.contains(r#"href="/p/plugins/com.example.tabs/backup""#),
        "{html}"
    );
    let pos = html.find(r#"data-local="list""#).unwrap();
    let around = &html[pos.saturating_sub(200)..(pos + 700).min(html.len())];
    assert!(
        around.contains(r#"aria-current="page""#),
        "默认应选中第一个 L3: {around}"
    );

    // L3 切换：视图名直接寻址，iframe 的 tab 跟着变，仍复用同一个 iframe DOM。
    let (status, html) = get_page(&h.app, &format!("{base}/backup?legacy=1"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("tab=backup"), "{html}");
    assert!(
        html.contains("/p/plugins/com.example.tabs/ui/backup.html?nonce="),
        "切换后 iframe 必须换成 ui/backup.html: {html}"
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

    // 不存在的视图直接 404；角色过滤在菜单层完成（不是渲染后 403）。
    let (status, _) = get_page(&h.app, &format!("{base}/nope?legacy=1"), &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // viewer 被 `PluginManage` 前置挡住 → 403（与 RPC 同权限），不是视图不存在。
    // 这里刻意不把权限不足伪装成 404：功能页权限口径必须与对应 RPC 一致。
    let (status, _) = get_page(&h.app, &format!("{base}/list?legacy=1"), &viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 无 L3 的 L2：隐式主视图，不渲染标签栏。
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

    // 不带 `?legacy=1` 的规范路由一律交 budding 壳页（原生优先，不再默认 SSR）。
    let (status, html) = get_html(&h.app, &format!("{base}/sites"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
    assert!(!html.contains("data-workbench-frame"), "{html}");
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

/// 插件 schema 端点：与 RPC 同权限（`PluginManage`），只返回通过静态校验的 JSON；
/// 未知视图 404，viewer 不得取 schema，也不得打开功能页。
#[tokio::test]
async fn plugin_schema_is_permission_gated_and_serves_json() {
    let h = harness();
    let manifest = r#"id = "com.example.schema"
name = "Schema Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.3"
requires_full_sandbox = false
[[ui]]
label = "Schema 演示"
path = "legacy.html"
[[menu]]
id = "schema-demo"
parent = "tools"
label = "Schema 演示"
view = "list"
  [[menu.items]]
  id = "list"
  label = "列表"
  view = "list"
  schema = "list.json"
"#
    .to_string();
    seed_declared_plugin(h.state.db().as_ref(), manifest, true);
    let schema_dir = h
        .data_dir
        .join("plugins/com.example.schema/versions/0.1.0/ui/schema");
    std::fs::create_dir_all(&schema_dir).unwrap();
    std::fs::write(
        schema_dir.join("list.json"),
        r#"{"version":1,"data":{"status":{"method":"status"}},"page":{"type":"page","props":{"title":"列表"},"body":[{"type":"text","props":{"text":"{{data.status.count}}"}}]}}"#,
    )
    .unwrap();

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.schema/schema/list")
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
        Some("application/json")
    );
    assert!(body_text(resp).await.contains("\"version\":1"));

    // 未知视图：不回落 main。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.schema/schema/nope")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // 运行期校验失败：schema 内容不合法是「输入包不成立」，按 400 返回
    // （`shape.plugin_schema`），不得归因成内核 500。
    std::fs::write(
        schema_dir.join("list.json"),
        r#"{"version":1,"page":{"type":""}}"#,
    )
    .unwrap();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.schema/schema/list")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // viewer：功能页与 schema 都应 403（和 RPC 同权限口径）。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for uri in [
        "/p/plugins/com.example.schema/list",
        "/p/plugins/com.example.schema/schema/list",
    ] {
        let resp = h
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "viewer 访问 {uri} 必须 403"
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

    // 插件 UI 静态资源与 schema / RPC 同权限：viewer 403，未登录 401。
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.title/ui/plain.html")
                .header(header::COOKIE, format!("skiff_dev_session={viewer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer 读取插件 UI 必须 403（与 schema/RPC 同口径）"
    );
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.title/ui/plain.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "未登录读取插件 UI 必须 303 到登录页，不得匿名下发静态资源"
    );
    assert!(
        resp.headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("/p/login?next="))
            .unwrap_or(false),
        "未登录必须带登录回跳地址"
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

    // 信息与管理数据走 `/ui/v1/plugins/{id}`（壳页渲染）：必须给出 main 入口与旧包菜单深链。
    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/plugins/com.example.legacy",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        v["data"]["entry_href"],
        "/p/plugins/com.example.legacy/main"
    );
    assert!(
        body.contains("/p/plugins/com.example.legacy/legacy"),
        "旧包菜单深链必须出现在关联菜单里: {body}"
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
    // 功能页走随包 schema 需要 api 1.3：测试台必须跟新壳一起演进，否则它验证的是
    // 一套面板里已经看不到的旧界面（HTML/iframe 只留给 `?legacy=1`）。
    assert_eq!(m.api, "1.3");
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
    // 四个视图都要有自己的 schema：缺一个就会在壳内回落到旧 SSR 工作台。
    for view in ["main", "check", "group", "single"] {
        let expect = format!("{view}.json");
        assert_eq!(
            m.schema_for_view(view),
            Some(expect.as_str()),
            "{view} 视图必须声明随包 schema"
        );
    }
    assert_eq!(
        m.schema_files().len(),
        4,
        "四个 schema 文件（去重后）都要被 manifest 引用"
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

    // 信息与管理数据走 `/ui/v1/plugins/{id}`（旧 SSR 信息页已删除）。
    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/plugins/com.skiff.testkit",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["plugin"]["name"], "插件架构测试台");
    assert_eq!(v["data"]["entry_href"], "/p/plugins/com.skiff.testkit/main");

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

    // 概览槽位由 budding 壳页按 `/ui/v1/overview` 的 `health_plugin` 拉取：
    // 服务端不再渲染槽位 HTML，这里只确认入口页是壳页。
    let (status, html) = get_html(&h.app, "/p", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
}

/// 官方包「装完即可用」回归。
///
/// 规则：**每个**官方包要么声明至少一条可用的默认授权建议（`bin:` / `path:` /
/// `container:` / `project:` / `plugin:`），要么在下表里**显式列名并写明原因**；新包既没默认授权、
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
        // `bin:<绝对路径>` 是 exec 能力面的默认授权（见 KERNEL_EXEC_CONTRACT §1），
        // 与 path:/container:/project:/plugin: 一样可直接落到安装确认页。
        let usable = |h: &String| {
            h.starts_with("bin:/")
                || h.starts_with("path:/")
                || h.starts_with("container:")
                || h.starts_with("project:")
                || h.starts_with("plugin:")
        };
        for h in &m.resource_hints {
            assert!(
                usable(h),
                "{name} 的默认授权建议不可用（{h}），只接受 bin:/ path:/ container:/ project:/ plugin: 前缀"
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
/// 主题一致性：bfcache 恢复后必须按 localStorage 重新 apply，
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
        !js.contains("htmx"),
        "htmx 已随旧 SSR 局部刷新端点下线，脚本里不得再留 htmx 分支"
    );
}
/// 没有随包 schema 的旧 HTML 插件视图自动回落到 `?legacy=1`，而不是渲染失败页。
#[test]
fn plugin_view_falls_back_to_legacy_when_schema_missing() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        js.contains("legacyUrl.searchParams.set('legacy', '1')")
            && js.contains("location.replace(legacyUrl.pathname + legacyUrl.search)"),
        "schema 404 时必须自动回落旧 SSR 工作台"
    );
}

/// 任务页筛选：关键字必须随页面骨架请求带进服务端，再由表格数据源复用。
#[test]
fn budding_tasks_page_keeps_keyword_filter() {
    // 「关键字 → 骨架请求」的拼装已收进 `routes.rs::page_source`。
    let source = include_str!("../src/handlers/ui/routes.rs");
    assert!(
        source.contains("api(\"/tasks?limit=1&with_schema=true\")"),
        "任务页骨架必须由服务端产出"
    );
    assert!(
        source.contains("format!(\"&q={}\", enc(s.get(\"q\")))"),
        "页面骨架请求必须带上关键字，否则筛选后的骨架与数据不一致"
    );
    let page = include_str!("../src/handlers/ui/tasks_page.rs");
    assert!(
        page.contains(r#"json!({ "q": keyword })"#),
        "表格数据源必须复用同一份关键字"
    );
    assert!(page.contains("清除筛选"), "必须有清除筛选入口");
}
/// Web 服务 `/ui/v1` 读写：读聚合 Admin-only，写缺 CSRF 403、viewer 403；
/// 管理员过门禁后走真实 nginx -t 语义（无 Docker 时失败，不伪装成功）。
#[tokio::test]
async fn ui_web_requires_csrf_and_admin() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["status"].is_object(), "{body}");
    assert!(v["data"]["config"]["main_content"].is_string(), "{body}");
    assert!(
        v["data"]["config"]["params"]["worker_connections"].is_number(),
        "{body}"
    );
    assert!(v["data"]["modules"].is_object(), "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web/logs", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["loaded"], false, "{body}");

    let cases = [
        ("/ui/v1/web/config", json!({ "content": "events {}" })),
        (
            "/ui/v1/web/params",
            json!({
                "worker_processes": "auto",
                "worker_connections": 2048,
                "keepalive_timeout": 65,
                "client_max_body_size": "16m",
                "gzip": true,
                "gzip_types": "text/css"
            }),
        ),
    ];
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 缺 CSRF: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} viewer: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, Some(&csrf), body).await;
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let code = v["error"]["code"].as_str().unwrap_or("");
        assert!(
            code != "403" && code != "auth.forbidden",
            "{uri} 管理员不应被权限拒绝: {text}"
        );
        // 无 Docker：nginx -t 不可执行，保存必须失败而不是返回 200 假成功。
        assert_ne!(status, StatusCode::OK, "{uri} 无 Docker 不能假成功: {text}");
    }
}

/// `/ui/v1/web/install`：轻量轮询接口，与 `/ui/v1/web` 的 install 块同源，
/// 数据来自任务中心 `web.install` 作业（重启后仍可查）。
#[tokio::test]
async fn ui_web_install_reports_persisted_job_and_same_shape() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web/install", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, _, body) = get_ui(&h.app, "/ui/v1/web/install", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web/install", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let idle: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(idle["data"]["status"], "idle", "{body}");
    assert_eq!(idle["data"]["running"], false, "{body}");
    assert_eq!(idle["data"]["steps"], json!([]), "{body}");

    let job = h
        .state
        .jobs()
        .submit(skiff_app::jobs::JobSubmit {
            kind: skiff_app::jobs::KIND_WEB_INSTALL.into(),
            target_type: skiff_app::jobs::TARGET_WEB_ENV.into(),
            target_id: skiff_app::jobs::WEB_ENV_ID.into(),
            title: "安装 Web 服务".into(),
            params: json!({ "action": "install" }),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: false,
        })
        .unwrap();
    h.state
        .jobs()
        .set_phase(&job.id, "pull_image", "拉取 OpenResty 镜像", 33, false);
    h.state.jobs().set_steps(
        &job.id,
        &json!([
            { "key": "check", "name": "检查 Docker 与 80 端口", "state": "ok", "detail": "Docker socket 可用", "started_at": 1, "finished_at": 2 },
            { "key": "pull_image", "name": "拉取 OpenResty 镜像", "state": "running", "detail": "", "started_at": 3, "finished_at": null }
        ]),
    );

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ui/v1/web/install")
                .header(
                    header::COOKIE,
                    format!("{}={admin}", skiff_identity::session_cookie_name(true)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "轮询接口必须禁缓存"
    );
    let body = body_text(resp).await;
    let live: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(live["data"]["job_id"], job.id, "{body}");
    assert_eq!(live["data"]["status"], "running", "{body}");
    assert_eq!(live["data"]["running"], true, "{body}");
    assert_eq!(live["data"]["progress"], 33, "{body}");
    assert_eq!(live["data"]["created_by"], "admin", "{body}");
    assert_eq!(live["data"]["requester_ip"], "127.0.0.1", "{body}");
    assert!(live["data"]["created_at"].is_string(), "{body}");
    assert!(live["data"]["started_at"].is_string(), "{body}");
    assert!(live["data"]["duration"].is_string(), "{body}");
    assert_eq!(live["data"]["steps"][0]["state"], "ok", "{body}");
    assert_eq!(live["data"]["steps"][1]["state"], "running", "{body}");

    // 同一构建函数：`/ui/v1/web` 的 install 块必须与轻量接口完全一致。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/web", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let snapshot: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(snapshot["data"]["install"], live["data"], "{body}");
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
}

/// 回收站是「工具 → 文件管理」的 L3 视图（菜单契约 §1.1），不再是独立二级菜单：
/// 菜单里没有独立入口，三个页面共用同一 L3 标签栏，深链直达壳页。
#[tokio::test]
async fn trash_is_files_l3_tab_not_standalone_menu_item() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("menu 必须是 JSON");
    let groups = v["data"]["groups"].as_array().expect("groups 必须是数组");
    // 菜单项按 key/href 扁平收一遍：回收站若被提升成二级就会有独立 href。
    let hrefs: Vec<&str> = groups
        .iter()
        .filter_map(|g| g["items"].as_array())
        .flatten()
        .filter_map(|i| i["href"].as_str())
        .collect();
    assert!(
        hrefs.contains(&"/p/files"),
        "文件管理二级入口必须保留: {body}"
    );
    assert!(
        !hrefs.contains(&"/p/files/trash"),
        "回收站不得再作为独立二级菜单项下发: {body}"
    );
    assert!(
        hrefs.contains(&"/p/files/quotas"),
        "配额管理仍是独立二级入口: {body}"
    );

    // 浏览 / 回收站 / 配额三个 L3 都是 budding 壳页（旧 SSR 已删除）。
    for uri in ["/p/files", "/p/files/trash", "/p/files/quotas"] {
        let (status, html) = get_page(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
    }
}
/// URL 契约 §7：旧路径必须 308 到规范路径，保留方法与查询串，不留死链。
#[tokio::test]
async fn legacy_paths_redirect_to_canonical_urls() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    for (old, status, new) in [(
        "/p/files/api/list?path=%2Fwww",
        StatusCode::PERMANENT_REDIRECT,
        "/p/files/list?path=%2Fwww",
    )] {
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

    // 商店安装只认 JSON POST（带 CSRF）：GET 同一路径必须是 405，不能被当成页面。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/ui/v1/store/plugins/com.example.demo/install")
                .header(header::COOKIE, format!("skiff_dev_session={admin}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
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
                        .uri("/p")
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
async fn store_api_returns_200_when_mirror_silently_drops_packets() {
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
                .uri("/ui/v1/store")
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
    assert_eq!(groups[0]["href"], "/p", "概览仍是直达链接");
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
        "/p/docker/containers/abc/unknown",
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

/// 未认证的 GET 页面请求：settings（只靠回落）与 files（有注册路由）必须一致 303 登录。
///
/// 覆盖回落、尾部斜杠、注册路由与「路径存在但方法不匹配」四种入口；写请求仍不回落。
#[tokio::test]
async fn shell_unauthenticated_get_redirects_consistently() {
    let h = harness();
    for uri in [
        "/p/settings",
        "/p/settings/",
        "/p/files",
        "/p/files/trash",
        "/p/settings/panel",
    ] {
        for accept in [None, Some("text/html"), Some("application/json")] {
            let mut builder = Request::builder().method("GET").uri(uri);
            if let Some(value) = accept {
                builder = builder.header(header::ACCEPT, value);
            }
            let resp = h
                .app
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::SEE_OTHER,
                "{uri} accept={accept:?} 未认证应 303"
            );
            let location = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                location.starts_with("/p/login?next="),
                "{uri} accept={accept:?} 应跳登录页，实际 {location}"
            );
        }
    }

    // 写请求不因未认证而回落或跳转：`/p/settings` 没有任何写路由，保持 404。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/settings")
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// 命中「路径存在但方法不匹配」时：GET 导航仍回落壳页，写请求仍 405。
///
/// `/p/files` 只注册了 GET；POST 到同一路径必须 405，而不是被当成壳页回落。
#[tokio::test]
async fn shell_fallback_covers_method_not_allowed_paths() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let path = "/p/docker/containers/abc/unknown";

    let (status, ct, body) = get_ui(&h.app, path, Some(&token), Some("text/html")).await;
    assert_eq!(status, StatusCode::OK, "白名单深链应回落壳页");
    assert!(ct.starts_with("text/html; charset=utf-8"));
    assert!(body.contains("该功能正在适配中"));

    // 非 HTML Accept 不回落，保持 404（没有任何路由匹配这条深链）。
    let (status, _, body) = get_ui(&h.app, path, Some(&token), Some("application/json")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.contains("该功能正在适配中"));

    // 写请求不回落：`/p/files` 只注册 GET，POST 必须是 405。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/files")
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
    // 登录页的品牌面板与表单标题由服务端模板承担，budding 只渲染表单本体。
    assert!(html.contains(r#"class="login-form-inner""#), "{html}");
    assert!(html.contains("欢迎回来"), "{html}");
    assert!(html.contains("login-eyebrow"), "{html}");
    assert!(html.contains(r#"data-next="/p/sites/a""#), "{html}");
    let ver = skiff_web::assets::BUDDING_VERSION;
    assert!(
        html.contains(&format!(
            r#"src="/p/static/budding/budding-{ver}.min.js?v={ver}""#
        )),
        "登录页应引用版本化 budding 产物"
    );
    let shell = skiff_web::assets::SHELL_VERSION;
    assert!(
        html.contains(&format!(
            r#"src="/p/static/budding/budding-login.js?v={shell}""#
        )),
        "登录页应加载版本化启动脚本"
    );
    // 登录页不加载 `budding-boot.js`（那是壳页启动脚本），回车补丁必须单独引入，
    // 否则登录框按回车没有任何反应。
    assert!(
        html.contains(&format!(
            r#"src="/p/static/form-enter-submit.js?v={shell}""#
        )),
        "登录页应加载回车提交补丁：{html}"
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
    // 启动脚本与壳层样式一样按 `?v=` 引用（命中 immutable），升级后必然换 URL。
    assert!(
        html.contains(r#"src="/p/static/budding/budding-boot.js?v="#),
        "{html}"
    );
    assert!(
        !html.contains(r#"id="skiff-main-nav""#),
        "壳页不应再输出旧 SSR 导航"
    );
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

[[ui]]
label = "Count"
path = "index.html"

[[mount]]
slot = "overview.health"
ui = "health.html"
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
    // 已启用插件声明 overview.health 槽位时，概览把提供者 id 交给前端走 RPC。
    assert_eq!(data["health_plugin"], "com.example.count");
    assert_eq!(data["health_reason"], "", "有提供者时不需要原因文案");
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

/// 没有可用的 `overview.health` 提供者时，概览必须给出可读原因，
/// 而不是让「系统状态」整块静默消失（停用与未安装分别给不同文案）。
#[tokio::test]
async fn ui_overview_reports_health_reason_without_enabled_provider() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["health_plugin"].is_null());
    let reason = v["data"]["health_reason"].as_str().unwrap();
    assert!(reason.contains("未安装"), "{reason}");

    seed_declared_plugin(
        h.state.db().as_ref(),
        r#"id = "com.example.health"
name = "Health Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.2"
requires_full_sandbox = false

[[ui]]
label = "Health"
path = "index.html"

[[mount]]
slot = "overview.health"
ui = "health.html"
"#
        .to_string(),
        false,
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["health_plugin"].is_null());
    let reason = v["data"]["health_reason"].as_str().unwrap();
    assert!(reason.contains("com.example.health"), "{reason}");
    assert!(reason.contains("已停用"), "{reason}");
}

/// 插件菜单 JSON 的 `href` 指向原生功能页（壳页优先读随包 schema）。
#[tokio::test]
async fn ui_menu_plugin_href_points_to_native_route() {
    let h = harness();
    seed_declared_plugin(
        h.state.db().as_ref(),
        r#"id = "com.example.native"
name = "Native Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.3"
requires_full_sandbox = false

[[ui]]
label = "Native"
path = "main.html"

[[menu]]
id = "main"
parent = "tools"
label = "Native"
order = 10
view = "main"
schema = "main.json"
"#
        .to_string(),
        true,
    );
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let item = v["data"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["items"].as_array())
        .flat_map(|items| items.iter())
        .find(|i| i["key"] == "plugin/com.example.native/main")
        .expect("插件菜单项必须存在");
    assert_eq!(item["href"], "/p/plugins/com.example.native/main");
    assert_eq!(item["default_view"], "main");
}

/// 插件 RPC 失败必须返回统一 JSON 错误信封（壳页读 `error.message`），
/// 不能退回 HTML 错误页导致前端只剩「服务返回状态 4xx」。
#[tokio::test]
async fn ui_plugin_rpc_errors_are_json_with_readable_message() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&token);
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/plugins/com.example.missing/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!("{}={}", skiff_identity::session_cookie_name(true), token),
                )
                .header("x-csrf-token", csrf)
                .body(Body::from(r#"{"method":"snapshot","params":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.starts_with("application/json"), "content-type: {ct}");
    let v: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("插件不存在"),
        "{v}"
    );
}

// ---- 站点 / 运行环境只读页（第一批）：JSON 列表 + keyset 游标 + 壳页回落 ----
/// `/ui/v1/sites`、`/ui/v1/sites/{id}`、`/ui/v1/runtimes` 的鉴权与冻结字段。
/// 列表走 keyset 游标（D10）：`limit` 截断后返回 `next_cursor`，下一页从游标继续。
#[tokio::test]
async fn ui_sites_and_runtimes_lists_use_frozen_fields_and_keyset_cursor() {
    let h = harness();
    for uri in ["/ui/v1/sites", "/ui/v1/sites/1", "/ui/v1/runtimes"] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }

    let mut ids = std::collections::HashMap::new();
    for domain in ["a.local", "b.local", "c.local"] {
        let root = h.data_dir.join(format!("www/sites/{domain}"));
        std::fs::create_dir_all(&root).unwrap();
        let site = h
            .state
            .db()
            .create_site(&php_site(domain, &root.display().to_string()))
            .unwrap();
        ids.insert(domain, site.id);
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
    assert!(first["id"].as_i64().is_some());
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
    // 默认不带页面骨架；只有页面加载显式要 `with_schema=true` 时才生成。
    assert!(v.get("schema").is_none(), "列表接口不应默认生成 Schema");

    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/sites?limit=1&with_schema=true",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["schema"]["type"], "page");
    assert_eq!(v["schema"]["props"]["title"], "网站");
    assert_eq!(v["schema"]["body"][1]["type"], "table");
    assert_eq!(v["schema"]["body"][1]["source"]["url"], "/ui/v1/sites");
    // 管理员能看到新建/批量弹窗与行内启停。
    assert_eq!(v["schema"]["body"][1]["props"]["rowKey"], "domain");
    assert!(v["schema"]["body"][0]["content"]["body"][1]["content"]["body"].is_array());

    let second_uri = format!("/ui/v1/sites?limit=2&cursor={cursor}");
    let (status, _, body) = get_ui(&h.app, &second_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["items"].as_array().unwrap().len(), 1);
    assert!(data["next_cursor"].is_null(), "最后一页不应再给游标");

    // 详情：列表字段 + 证书 + 运行环境状态，均为只读。深链只使用主键。
    let detail_uri = format!("/ui/v1/sites/{}", ids["a.local"]);
    let (status, _, body) = get_ui(&h.app, &detail_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    assert_eq!(data["domain"], "a.local");
    assert_eq!(data["id"], ids["a.local"]);
    assert!(data["ssl"].is_object());
    assert!(data["runtime_alert"].is_string());
    assert!(data["runtime_down"].is_boolean());
    // 详情响应同时带回 SDUI Schema：概览页标题用域名，正文非空。
    assert_eq!(v["schema"]["type"], "page");
    assert_eq!(v["schema"]["props"]["title"], "a.local");
    assert!(!v["schema"]["body"].as_array().unwrap().is_empty());

    // 子视图只换渲染块：`tab=config` 必须切到配置页。
    let (status, _, body) = get_ui(
        &h.app,
        &format!("{detail_uri}?tab=config"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["schema"]["type"], "page");
    assert!(
        v["schema"]["props"]["subtitle"]
            .as_str()
            .unwrap_or_default()
            .contains("伪静态"),
        "{v}"
    );

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

/// 新建运行环境弹窗的目录接口：给出 apk 源候选与**当前生效值**，
/// 弹窗才能预选「已固定的源」，避免用户只新建环境却把源改回自动。
#[tokio::test]
async fn ui_runtime_catalog_exposes_apk_mirrors_and_current_choice() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/runtimes/catalog", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = &v["data"];
    let mirrors = data["apk_mirrors"].as_array().unwrap();
    assert!(mirrors.iter().any(|m| m["key"] == "tuna"), "{v}");
    assert!(
        mirrors
            .iter()
            .all(|m| !m["base"].as_str().unwrap_or_default().is_empty()),
        "候选表不能混入「自动」占位（它由前端单独的 auto 项表达）"
    );
    assert_eq!(data["apk_mirror_current"]["fixed"], false);

    // 用户此前固定过源：目录接口必须原样回传。
    let dir = h.data_dir.join("www/runtime/cache/apk");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".skiff-mirror"),
        "# skiff-fixed\nhttps://mirrors.ustc.edu.cn/alpine\n",
    )
    .unwrap();
    let (status, _, body) = get_ui(&h.app, "/ui/v1/runtimes/catalog", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["apk_mirror_current"]["fixed"], true);
    assert_eq!(
        v["data"]["apk_mirror_current"]["base"],
        "https://mirrors.ustc.edu.cn/alpine"
    );
    assert_eq!(v["data"]["apk_mirror_current"]["label"], "中科大 USTC");

    // 非法源在命令层就被拒，不登记运行环境。
    let csrf = h.state.csrf_token(&admin);
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/runtimes",
        &admin,
        Some(&csrf),
        &json!({ "kind": "php", "version": "8.5", "apk_mirror": "ftp://bad.example/alpine" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
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

/// Docker 写操作 JSON 接口：CSRF 先于一切、viewer 被权限矩阵拒绝、
/// 管理员在无 Docker 环境下得到失败而不是「假成功」；非法名称在触达 Docker 前 400。
#[tokio::test]
async fn ui_docker_writes_require_csrf_and_permission() {
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("/ui/v1/containers", json!({ "image": "nginx:1.27" })),
        (
            "/ui/v1/containers/abc/limits",
            json!({ "cpu_shares": "512" }),
        ),
        ("/ui/v1/images/pull", json!({ "image": "nginx:1.27" })),
        ("/ui/v1/images/sha256:abc/remove", json!({})),
        ("/ui/v1/networks", json!({ "name": "app-net" })),
        ("/ui/v1/networks/app-net/remove", json!({})),
        ("/ui/v1/volumes", json!({ "name": "app-data" })),
        ("/ui/v1/volumes/app-data/remove", json!({})),
    ];

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} 缺 CSRF 必须 403: {text}"
        );
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }

    for (uri, body) in &cases {
        if uri.ends_with("/limits") {
            // 资源限制先解析容器再判权限；无 Docker 时解析失败，同样不能执行写操作。
            let (status, _) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
            assert_ne!(status, StatusCode::OK, "{uri} viewer 不得执行写操作");
            continue;
        }
        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} viewer 必须 403: {text}"
        );
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }

    // 守卫顺序固定为「先权限、后 CSRF」：viewer 即使不带令牌也先得到权限拒绝，
    // 而不是 `auth.csrf_invalid`（后者会让客户端误以为刷新令牌即可）。
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &viewer, None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} viewer 无令牌: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, Some(&csrf), body).await;
        assert_ne!(status, StatusCode::OK, "{uri} 无 Docker 时不得成功: {text}");
        assert_ne!(status, StatusCode::FORBIDDEN, "{uri} 管理员不应被权限拒绝");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(
            v["error"]["code"] == "dependency.unavailable" || v["error"]["code"] == "shape.invalid",
            "{uri} 错误码必须是依赖不可用或参数错误: {text}"
        );
    }

    for uri in ["/ui/v1/networks", "/ui/v1/volumes"] {
        let (status, text) = post_ui_json(
            &h.app,
            uri,
            &admin,
            Some(&csrf),
            &json!({ "name": "bad name" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "shape.invalid", "{uri}");
    }
}

/// 站点写操作 JSON 接口：CSRF 先行、viewer 一律 403、管理员校验字段后再执行；
/// 建站成功并能在详情里看到配置/备份/写权限字段。
#[tokio::test]
async fn ui_site_writes_require_csrf_and_permission() {
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);
    let root = h.data_dir.join("www/sites/x.local");
    std::fs::create_dir_all(&root).unwrap();
    let site = h
        .state
        .db()
        .create_site(&static_site("x.local", &root.display().to_string()))
        .unwrap();
    let site_id = site.id;

    let cases: Vec<(String, serde_json::Value)> = vec![
        (
            "/ui/v1/sites".to_string(),
            json!({ "site_type": "static", "domain": "api.local" }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/config"),
            json!({ "content": "server {}" }),
        ),
        (format!("/ui/v1/sites/{site_id}/start"), json!({})),
        (
            format!("/ui/v1/sites/{site_id}/default"),
            json!({ "enabled": true }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/domains"),
            json!({ "action": "add", "alias": "www.x.local" }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/ssl"),
            json!({ "action": "generate" }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/backup"),
            json!({ "enabled": true, "interval": "daily", "time": "03:00" }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/policy"),
            json!({ "rewrite": "none", "limit_rate_kbps": 0 }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/delete"),
            json!({ "confirm": "x.local" }),
        ),
        (
            "/ui/v1/sites/batch".to_string(),
            json!({ "action": "start", "domains": ["x.local"] }),
        ),
    ];

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} 缺 CSRF 必须 403: {text}"
        );
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");

        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} viewer 必须 403: {text}"
        );
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }

    // 「先权限、后 CSRF」：viewer 不带令牌也先得到权限拒绝。
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &viewer, None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} viewer 无令牌: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }

    // 字段校验先于业务执行：确认串不符、批量为空、未知子动作都必须是 400。
    let (status, _) = post_ui_json(
        &h.app,
        &format!("/ui/v1/sites/{site_id}/delete"),
        &admin,
        Some(&csrf),
        &json!({ "confirm": "wrong.local" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(h.state.db().site_by_domain("x.local").unwrap().is_some());

    let (status, _) = post_ui_json(
        &h.app,
        "/ui/v1/sites/batch",
        &admin,
        Some(&csrf),
        &json!({ "action": "start", "domains": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for (uri, body) in [
        (
            format!("/ui/v1/sites/{site_id}/domains"),
            json!({ "action": "unknown", "alias": "a.local" }),
        ),
        (
            format!("/ui/v1/sites/{site_id}/ssl"),
            json!({ "action": "unknown" }),
        ),
    ] {
        let (status, _) = post_ui_json(&h.app, &uri, &admin, Some(&csrf), &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }

    // 建站：测试机无 OpenResty，命令层必须返回依赖不可用而不是假成功。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/sites",
        &admin,
        Some(&csrf),
        &json!({
            "site_type": "static",
            "domain": "new.local",
            "aliases": ["www.new.local"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "建站失败: {text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable");
}

/// 站点详情：管理员有 can_manage 与配置内容；viewer 只读。
#[tokio::test]
async fn ui_site_detail_exposes_write_surface_by_role() {
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/role.local");
    std::fs::create_dir_all(&root).unwrap();
    let role_site = h
        .state
        .db()
        .create_site(&static_site("role.local", &root.display().to_string()))
        .unwrap();

    let detail_uri = format!("/ui/v1/sites/{}", role_site.id);
    let (status, _, text) =
        get_ui(&h.app, &detail_uri, Some(&admin), Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["can_manage"], true);
    assert!(v["data"]["config"].is_object());
    assert!(
        v["data"]["config"]["error"]
            .as_str()
            .unwrap()
            .contains("role.local"),
        "直插站点没有 vhost 文件时应回传可读错误而不是 500: {text}"
    );
    assert!(v["data"]["backup"].is_object());

    let (status, _, text) =
        get_ui(&h.app, &detail_uri, Some(&viewer), Some("application/json")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["can_manage"], false);
}

/// 规范新建站点入口交付 budding 壳页；写动作由 `POST /ui/v1/sites` 判权（viewer 403）。
#[tokio::test]
async fn site_new_canonical_path_serves_native_shell() {
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;

    let (status, html) = get_html(&h.app, "/p/sites/new", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"id="skiff-root""#),
        "新建站点页应交付 budding 壳页: {html}"
    );

    // 页面本身不做 RBAC（壳页对所有已登录用户一致），权限落在写入口。
    let (status, _) = get_html(&h.app, "/p/sites/new", &viewer).await;
    assert_eq!(status, StatusCode::OK, "壳页不做角色分流");
    let csrf = h.state.csrf_token(&viewer);
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/sites",
        &viewer,
        Some(&csrf),
        &json!({ "site_type": "static", "domain": "blocked.local" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "viewer 建站必须 403: {text}");
}

/// Compose 校验/部署：CSRF 与管理员门禁先行；校验不触达 Docker，部署无引擎时 503。
#[tokio::test]
async fn ui_compose_validate_and_up_require_csrf_and_permission() {
    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);
    let project = "services:\n  web:\n    image: nginx:alpine\n    ports:\n      - \"8080:80\"\n";

    for uri in ["/ui/v1/compose/validate", "/ui/v1/compose/up"] {
        let body = json!({ "project": "demo", "compose": project });
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid");

        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden");
    }

    // down 与项目级动作同样在 Lifecycle 档具备 ComposeUp/Down：缺令牌必须是 CSRF 拒绝。
    for (uri, body) in [
        (
            "/ui/v1/compose/down",
            json!({ "project": "demo", "remove_volumes": false }),
        ),
        (
            "/ui/v1/compose/demo/start",
            json!({ "project": "demo", "remove_volumes": false }),
        ),
    ] {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/compose/validate",
        &admin,
        Some(&csrf),
        &json!({ "project": "demo", "compose": "services: [oops" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/compose/validate",
        &admin,
        Some(&csrf),
        &json!({ "project": "demo", "compose": project }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["project"], "demo");
    assert_eq!(v["data"]["services"], json!(["web"]));
    assert_eq!(v["data"]["valid"], true);

    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/compose/up",
        &admin,
        Some(&csrf),
        &json!({ "project": "demo", "compose": project }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable");
}

/// 任务与作业写操作：CSRF 先行、viewer 403；无调度器时任务写 503；
/// 作业取消返回真实结果，无执行器的重试仍拒绝。
#[tokio::test]
async fn ui_task_and_job_writes_require_csrf_and_permission() {
    use skiff_app::jobs::JobSubmit;

    let h = harness_lifecycle();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    let task_cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "/ui/v1/tasks/builtin:db-vacuum/toggle",
            json!({ "enabled": false }),
        ),
        ("/ui/v1/tasks/builtin:db-vacuum/run", json!({})),
        ("/ui/v1/tasks/builtin:db-vacuum/delete", json!({})),
    ];
    for (uri, body) in &task_cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");

        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");

        let (status, text) = post_ui_json(&h.app, uri, &admin, Some(&csrf), body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "dependency.unavailable", "{uri}");
    }

    for uri in ["/ui/v1/jobs/missing/cancel", "/ui/v1/jobs/missing/retry"] {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, &json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");

        let (status, text) =
            post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), &json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");

        let (status, text) = post_ui_json(&h.app, uri, &admin, Some(&csrf), &json!({})).await;
        assert_ne!(status, StatusCode::OK, "{uri} 不存在的作业不得成功: {text}");
        assert_ne!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    let job = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "site.create".into(),
            target_type: "site".into(),
            target_id: "ui.cancel.example.com".into(),
            title: "创建站点 ui.cancel.example.com".into(),
            params: json!({ "name": "ui.cancel.example.com" }),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();

    let (status, text) = post_ui_json(
        &h.app,
        &format!("/ui/v1/jobs/{}/cancel", job.id),
        &admin,
        Some(&csrf),
        &json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["data"]["outcome"].is_string(),
        "取消结果必须回传 outcome: {text}"
    );

    let (status, text) = post_ui_json(
        &h.app,
        &format!("/ui/v1/jobs/{}/retry", job.id),
        &admin,
        Some(&csrf),
        &json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "无执行器类型不得重试: {text}"
    );
}

/// 审计规范页交 budding 壳；数据接口对无 `AuditRead` 的角色仍是 403。
#[tokio::test]
async fn audit_canonical_path_serves_native_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/settings/audit", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"id="skiff-root""#), "{html}");
    }
    let (status, _, text) = get_ui(&h.app, "/ui/v1/audit", Some(&viewer), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "viewer 无 AuditRead，不得读到审计数据: {text}"
    );
}

/// 审计查询：关键字命中 + 签名 keyset 游标；换关键字沿用旧游标被拒，篡改被拒。
#[tokio::test]
async fn ui_audit_filters_and_walks_signed_cursor() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for _ in 0..3 {
        h.state
            .db()
            .audit(
                "admin",
                "10.8.8.8",
                "site.create",
                "audit-needle.example",
                "ok",
                "seed",
            )
            .unwrap();
        h.state
            .db()
            .audit(
                "admin",
                "10.8.8.8",
                "site.delete",
                "audit-needle.example",
                "ok",
                "seed",
            )
            .unwrap();
    }

    // 权限与旧 SSR 同口径：viewer 无 AuditRead。
    let (status, _) = get_page(&h.app, "/ui/v1/audit", &viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = get_page(&h.app, "/ui/v1/audit?q=audit-needle", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let items = v["data"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 6, "{body}");
    assert!(
        items[0]["ts"].as_str().unwrap_or_default().contains('T'),
        "ts 应为 RFC3339 UTC：{body}"
    );
    assert_eq!(items[0]["action"], "site.delete", "按时间倒序");

    // 首页只取 1 条，用返回的签名游标取下一页。
    let (status, body) = get_page(&h.app, "/ui/v1/audit?q=audit-needle&limit=1", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let first_id = v["data"]["items"][0]["id"].as_i64().expect("id");
    let cursor = v["data"]["next_cursor"]
        .as_str()
        .expect("next_cursor")
        .to_string();
    assert_eq!(v["data"]["has_more"], true);

    let (status, body) = get_page(
        &h.app,
        &format!("/ui/v1/audit?q=audit-needle&limit=2&cursor={cursor}"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_ne!(v["data"]["items"][0]["id"].as_i64(), Some(first_id));

    // 换关键字沿用旧游标：筛选指纹不匹配，400 而不是悄悄从头开始。
    let (status, body) = get_page(
        &h.app,
        &format!("/ui/v1/audit?q=other&limit=1&cursor={cursor}"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("筛选条件"), "{body}");

    // 篡改签名最后一个字符：400，文案区分「校验失败」。
    let mut chars: Vec<char> = cursor.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == '0' { '1' } else { '0' };
    let forged: String = chars.into_iter().collect();
    let (status, body) = get_page(
        &h.app,
        &format!("/ui/v1/audit?q=audit-needle&limit=1&cursor={forged}"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("游标校验失败"), "{body}");
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

/// 设置规范页交付 budding 壳页。
#[tokio::test]
async fn settings_canonical_path_serves_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/settings/security", token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"id="skiff-root""#), "{html}");
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
    let log_site = h
        .state
        .db()
        .create_site(&static_site("logs.local", &root.display().to_string()))
        .unwrap();
    let log_id = log_site.id;

    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(
        &h.app,
        &format!("/ui/v1/sites/{log_id}/logs?tail=2"),
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
        &format!("/ui/v1/sites/{log_id}/logs?q=abc"),
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
    let (status, _, _) = get_ui(
        &h.app,
        &format!("/ui/v1/sites/{log_id}/logs"),
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
/// 容器终端页交 budding 壳；真正的 WS 连接需 Linux 测试机，本机不做断言。
#[tokio::test]
async fn container_terminal_page_serves_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/docker/containers/abc123/terminal", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
}

/// 容器日志页交 budding 壳；JSON 只读、未认证 401、无 Docker 503（不再有 SSE 页面入口）。
#[tokio::test]
async fn container_logs_page_and_json_keep_read_only_semantics() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/docker/containers/abc123/logs", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/containers/abc123/logs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/containers/abc123/logs?tail=500",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "dependency.unavailable");
}

/// 作业日志：浏览器导航（Accept: text/html）交 budding 壳页；日志正文走 `/ui/v1/jobs/{id}`。
#[tokio::test]
async fn job_logs_serve_shell_for_html_and_json_for_scripts() {
    use skiff_app::jobs::JobSubmit;

    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let job = h
        .state
        .jobs()
        .submit(JobSubmit {
            kind: "site.create".into(),
            target_type: "site".into(),
            target_id: "logs.example.com".into(),
            title: "创建站点 logs.example.com".into(),
            params: serde_json::json!({ "name": "logs.example.com" }),
            created_by: "admin".into(),
            created_by_role: "admin".into(),
            requester_ip: "127.0.0.1".into(),
            cancellable: true,
        })
        .unwrap();
    h.state
        .jobs()
        .log_lines(&job.id, "out", &["作业日志第一行".to_string()]);
    let uri = format!("/p/jobs/{}/logs", job.id);

    let (status, html) = get_html(&h.app, &uri, &admin).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(
        html.contains(r#"id="skiff-root""#),
        "导航应返回壳页: {html}"
    );
    assert!(
        html.contains("budding-boot.js"),
        "壳页应加载 budding 运行时（日志用 code 组件）: {html}"
    );

    // 非浏览器导航（无 `Accept: text/html`）不回落壳页：脚本请求拿到 404，
    // 日志正文改走 `/ui/v1/jobs/{id}`。
    let (status, _) = get_page(&h.app, &uri, &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // 脚本取日志正文走 JSON 详情接口（旧 `?format=json` 页面分支已删除）。
    let (status, _, body) = get_ui(
        &h.app,
        &format!("/ui/v1/jobs/{}", job.id),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["id"], job.id, "{body}");
    assert!(
        body.contains("作业日志第一行"),
        "作业详情必须带上增量日志: {body}"
    );
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

/// 文件写能力面补齐：write → copy → archive → extract；权限位解析在触达 fs 前拒绝。
#[tokio::test]
async fn ui_files_write_copy_archive_extract_roundtrip() {
    let h = harness();
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().to_path_buf();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);

    let file = root.join("note.txt");
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/write",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "path": file.display().to_string(),
            "content": "hello p0"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello p0");

    let copy = root.join("copy.txt");
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/copy",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "from": file.display().to_string(),
            "to": copy.display().to_string(),
            "overwrite": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "hello p0");

    let archive = root.join("bundle.tar.gz");
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/archive",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "sources": [copy.display().to_string()],
            "dest": archive.display().to_string(),
            "format": "tar.gz",
            "overwrite": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(archive.is_file(), "压缩包必须落盘");

    let extract_dir = root.join("extracted");
    std::fs::create_dir(&extract_dir).unwrap();
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/extract",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "archive": archive.display().to_string(),
            "dest": extract_dir.display().to_string(),
            "overwrite": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        std::fs::read_to_string(extract_dir.join("copy.txt")).unwrap(),
        "hello p0"
    );

    // 非法八进制在服务端解析阶段就拒绝，不依赖平台是否支持 chmod。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/files/chmod",
        &admin,
        Some(&csrf),
        &serde_json::json!({
            "path": file.display().to_string(),
            "mode": "99x"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("八进制"), "错误文案应指出八进制: {text}");

    // 空源数组与默认 uid/gid 缺失都必须拒绝，不能退化成 0:0。
    let (status, _) = post_ui_json(
        &h.app,
        "/ui/v1/files/archive",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "sources": [], "dest": "", "format": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post_ui_json(
        &h.app,
        "/ui/v1/files/chown",
        &admin,
        Some(&csrf),
        &serde_json::json!({ "path": file.display().to_string() }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// 商店规范页交付 budding 壳页。
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
    // 确认页与商店同属 `/plugins` 分组；它必须命中壳页，而不是通用插件功能页路由。
    for uri in ["/p/plugins/store", "/p/plugins/store/confirm"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
    }
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

/// 升级包下载：未认证 401、缺 CSRF 403、viewer 403；管理员在无固定公钥时不假成功。
#[tokio::test]
async fn ui_store_upgrade_download_requires_csrf_and_admin() {
    // 目录未启用时也先按未认证处理，不泄露目录状态。
    let h = harness();
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/upgrades/0.2.0/download",
        "",
        None,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{text}");

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

    // 缺 CSRF：403，先于权限与下载。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/upgrades/0.2.0/download",
        &admin,
        None,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.csrf_invalid");

    // viewer 无 Admin：403 auth.forbidden，不发起下载。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/upgrades/0.2.0/download",
        &viewer,
        Some(&viewer_csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    // 管理员但目录未配置固定公钥：400 明确拒绝，不发起下载。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/store/upgrades/0.2.0/download",
        &admin,
        Some(&csrf),
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("公钥"), "{text}");
}

/// 列表空态是场景语义，必须原样落到表格 props；不得被共享层丢弃成组件默认的
/// 「暂无数据」。页面骨架已全部由服务端产出，因此契约钉在 `schema_kit::table`
/// 与各页的文案上，不再钉壳脚本（脚本里已没有表格构建器）。
/// 各列表页的文案按 docs/contract/UI_CONTRACT.md §7 的口径（真空列表 / 筛选无结果 /
/// 依赖未就绪）分别给出，这里逐条防回归。
/// 镜像 / 网络 / 卷是 JSON 页面资产，文案断言读资产而不是读脚本。
#[test]
fn budding_boot_status_table_honors_caller_empty_text() {
    // 构件层：调用方给了文案就必须进 props，没给才回落到通用文案。
    let kit = include_str!("../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains(r#"opts.empty_text.unwrap_or_else(|| "暂无数据".to_string())"#),
        "schema_kit::table 必须把调用方空态文案传给表格组件"
    );
    assert!(
        kit.contains(r#""emptyText": empty_text"#),
        "空态文案必须落到表格 props.emptyText"
    );
    // 站点列表已服务端化，文案在 site_page.rs 而非 JS 内联。
    let site_page = include_str!("../src/handlers/ui/site_page.rs");
    assert!(
        site_page.contains("还没有站点，创建后会生成整站目录、nginx vhost 与 PHP 探针"),
        "站点空态文案在服务端 schema"
    );
    // 容器 / 运行环境 / compose / Web 的空态随页面骨架迁到服务端 schema。
    for (source, text) in [
        // 容器：按 Docker 可用性与筛选状态分三种。
        (
            include_str!("../src/handlers/ui/containers_page.rs"),
            "Docker 不可用，容器列表为空",
        ),
        (
            include_str!("../src/handlers/ui/containers_page.rs"),
            "没有异常容器",
        ),
        (
            include_str!("../src/handlers/ui/containers_page.rs"),
            "还没有容器，可在下方新建",
        ),
        // 运行环境：真空列表引导创建；配置 / 扩展页签各自的空态。
        (
            include_str!("../src/handlers/ui/runtimes_page.rs"),
            "还没有运行环境，点击下方「新建运行环境」安装后即可绑定站点",
        ),
        (
            include_str!("../src/handlers/ui/runtimes_page.rs"),
            "没有可编辑的配置文件",
        ),
        (
            include_str!("../src/handlers/ui/runtimes_page.rs"),
            "还没有可用扩展",
        ),
        // compose：真空与 Docker 不可用两种。
        (
            include_str!("../src/handlers/ui/compose_page.rs"),
            "还没有 compose 项目，可在下方编辑并部署",
        ),
        (
            include_str!("../src/handlers/ui/compose_page.rs"),
            "Docker 不可用，compose 项目列表为空",
        ),
        // Web 日志查询无结果。
        (
            include_str!("../src/handlers/ui/web_page.rs"),
            "本次查询没有日志行",
        ),
    ] {
        assert!(source.contains(text), "列表空态文案缺失：{text}");
    }
    // 任务 / 作业 / 审计的空态文案已随页面迁到服务端 schema。
    for (source, text) in [
        (
            include_str!("../src/handlers/ui/tasks_page.rs"),
            "还没有计划任务，站点详情页可开启每日备份",
        ),
        (
            include_str!("../src/handlers/ui/tasks_page.rs"),
            "还没有运行记录",
        ),
        (
            include_str!("../src/handlers/ui/jobs_page.rs"),
            "还没有长耗时任务，安装运行环境等操作会在这里显示进度",
        ),
        (
            include_str!("../src/handlers/ui/audit_page.rs"),
            "暂无审计记录",
        ),
    ] {
        assert!(source.contains(text), "列表空态文案缺失：{text}");
    }
    // 镜像 / 网络 / 卷：真空列表与依赖未就绪两种，逐页两种都要在。
    for (asset, texts) in [
        (
            include_str!("../static/budding/pages/docker-images.json"),
            [
                "还没有拉取镜像，拉取后新建容器或运行环境可直接使用",
                "Docker 不可用，镜像列表为空",
            ],
        ),
        (
            include_str!("../static/budding/pages/docker-networks.json"),
            [
                "还没有自定义网络，需要隔离或固定子网时创建",
                "Docker 不可用，网络列表为空",
            ],
        ),
        (
            include_str!("../static/budding/pages/docker-volumes.json"),
            [
                "还没有命名卷，创建后可在新建容器时挂载",
                "Docker 不可用，卷列表为空",
            ],
        ),
    ] {
        for text in texts {
            assert!(asset.contains(text), "JSON 页面空态文案缺失：{text}");
        }
    }
}

/// 页面 Schema 迁到 JSON 资产后的接线：路由给资产 URL，资产自带取数与动作。
/// 三件事缺一环就是白屏 / 空表 / 403，所以逐项钉住。
#[test]
fn budding_boot_json_pages_are_wired_and_parse() {
    let js = include_str!("../static/budding/budding-boot.js");
    // 路由表已由服务端下发：资产 URL 登记处从壳脚本换到 `handlers/ui/routes.rs`。
    let routes_src = include_str!("../src/handlers/ui/routes.rs");
    for name in [
        "docker-images.json",
        "docker-networks.json",
        "docker-volumes.json",
    ] {
        assert!(
            routes_src.contains(&format!("PageKind::Asset(\"{name}\")")),
            "路由必须指向 JSON 资产：{name}"
        );
    }
    // 已迁出的页面不得再留在脚本里，否则两份实现会各自漂移。
    for kind in ["images", "networks", "volumes"] {
        assert!(
            !js.contains(&format!("kind === '{kind}'")),
            "页面已迁到 JSON，脚本不应再有 kind === '{kind}' 分支"
        );
        assert!(
            !js.contains(&format!("'skiff:{kind}'")),
            "路由不应再引用 skiff:{kind}"
        );
    }
    // JSON 里写 `${prefix}` / `${api}` 依赖全局 context；写请求的 CSRF 靠平台适配器补。
    assert!(
        js.contains("context: { prefix: prefix, api: apiBase }"),
        "必须把入口前缀与 API 基址注入页面根作用域"
    );
    assert!(
        js.contains("if (method !== 'GET' && method !== 'HEAD')"),
        "平台请求适配器必须为写请求补 CSRF 头，否则 JSON 里的 request / form 一律 403"
    );

    for (name, api) in [
        ("docker-images.json", "/ui/v1/images"),
        ("docker-networks.json", "/ui/v1/networks"),
        ("docker-volumes.json", "/ui/v1/volumes"),
    ] {
        let text = match name {
            "docker-images.json" => include_str!("../static/budding/pages/docker-images.json"),
            "docker-networks.json" => include_str!("../static/budding/pages/docker-networks.json"),
            _ => include_str!("../static/budding/pages/docker-volumes.json"),
        };
        let page: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|e| {
            panic!("{name} 不是合法 JSON：{e}");
        });
        assert_eq!(page["type"], "page", "{name} 根节点必须是 page");
        assert_eq!(
            page["source"]["url"], api,
            "{name} 必须由页面 source 自己取数（can_admin / docker_available 都在这个响应里）"
        );
        let table = page["body"]
            .as_array()
            .and_then(|b| b.iter().find(|n| n["type"] == "table"))
            .unwrap_or_else(|| panic!("{name} 缺少表格节点"));
        assert!(
            table["id"].as_str().is_some_and(|s| !s.is_empty()),
            "{name} 表格必须声明稳定 id——写操作成功后要靠 invoke reload 刷新"
        );
        assert_eq!(table["source"]["url"], api, "{name} 表格取数地址");
        assert_eq!(
            table["props"]["itemsPath"], "data.items",
            "{name} 表格行路径必须对齐 /ui/v1 信封"
        );
        let actions = table["props"]["rowActions"][0]["onEvent"]["click"]["actions"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} 行操作缺少动作链"));
        let types: Vec<&str> = actions.iter().filter_map(|a| a["type"].as_str()).collect();
        assert_eq!(
            types,
            ["confirm", "request", "invoke"],
            "{name} 删除必须走 确认 → 请求 → 刷新 三步，且只用 budding 内置动作"
        );
    }
}

/// budding 的 `button` 只渲染 `<button type="button">`，不消费 `to`；
/// 页面切换必须用显式动作。同 path 只换 query 的页内跳转（容器「仅异常」）
/// 还要解决 SDK 页面加载键不含 query 的问题：走 `skiffQueryNav` 导航后显式
/// `refreshPage`，并对浏览器前进/后退的同 path 情况做整页刷新兜底。
#[test]
fn budding_boot_buttons_navigate_explicitly() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        !js.contains("to: prefix +"),
        "button.props.to 不会触发导航，必须改用 click → 导航动作"
    );
    // query 变化不会改变 SDK 的页面加载键，必须注册并下发 skiffQueryNav。
    assert!(
        js.contains("skiffQueryNav: queryNav,"),
        "skiffQueryNav 必须注册到自定义动作表"
    );
    assert!(
        js.contains("runtime.navigateTo({ path: path, query: query });"),
        "skiffQueryNav 必须经宿主路由更新地址栏（含 query）"
    );
    assert!(
        js.contains("runtime.refreshPage();"),
        "skiffQueryNav 必须在导航后显式刷新页面内容"
    );
    assert!(
        js.contains("window.addEventListener('popstate'"),
        "浏览器前进/后退的同 path query 变化必须有 popstate 兜底"
    );
    // 容器过滤按钮的服务端 schema：两个页内跳转都走 skiffQueryNav。
    let containers_page = include_str!("../src/handlers/ui/containers_page.rs");
    for needle in [
        "{ \"type\": \"skiffQueryNav\", \"props\": { \"path\": \"${prefix}/docker/containers\", \"query\": query } }",
        "filter_button(\"全部\", filter != \"error\", json!({}))",
        "filter_button(\"仅异常\", filter == \"error\", json!({ \"filter\": \"error\" }))",
    ] {
        assert!(
            containers_page.contains(needle),
            "容器过滤按钮缺少 skiffQueryNav 动作：{needle}"
        );
    }
    assert!(
        !js.contains("url: prefix + '/docker/containers?filter=error'"),
        "只换 query 的跳转不能再用 navigate（只会 pushState，不会重载内容）"
    );
    // 计数与列表共用同一 filter 口径的拼装已收进 `routes.rs::page_source`。
    let routes_src = include_str!("../src/handlers/ui/routes.rs");
    assert!(
        routes_src.contains("if s.get(\"filter\") == \"error\""),
        "容器计数必须与列表用同一 filter 口径，切换后计数才同步变化"
    );
}

/// 宿主桥与文件写入口的静态守门：平台注入 confirm、确认框走运行时图层、
/// 文件能力面动作与弹窗表单入口都在，且不再依赖原生 prompt。
#[test]
fn budding_boot_host_bridge_and_file_entries_present() {
    let js = include_str!("../static/budding/budding-boot.js");
    for needle in [
        // 确认框走 budding 内建图层，不再注入 platform.confirm 或自建按钮动作。
        "runtime.confirm(payload)",
        "buddingRuntime()",
        "consumeFlashToast();",
        // 文件能力面入口（后端 /ui/v1/files/* 已补齐）。
        "skiffFileWrite: fileWrite,",
        "skiffFileCopy: fileCopy,",
        "skiffFileChmod: fileChmod,",
        "skiffFileChown: fileChown,",
        "skiffFileArchive: fileArchive,",
        "skiffFileExtract: fileExtract,",
        // 站点详情 Schema 由服务端 UI 端点产出，前端只 fetch 并做运行时转义。
        "function toPageSchema(",
        "prepareServerSchema(schema)",
    ] {
        assert!(js.contains(needle), "宿主桥/文件入口缺失：{needle}");
    }
    // 文件目录行模板已迁到 JSON 资产：解压入口按文件类型显隐的声明在这里。
    let file_row = include_str!("../static/budding/pages/_file-row.json");
    assert!(
        file_row.contains("\"visibleOn\": \"${item.can_extract}\""),
        "文件行模板缺少解压入口的显隐条件"
    );
    // 站点删除的确认链已随 Schema 迁到服务端（`site_page.rs`），静态守门跟着挪。
    let site_page = include_str!("../src/handlers/ui/site_page.rs");
    for needle in [
        "\"requireText\": site.domain",
        "\"op\": \"delete\", \"id\": site.id, \"domain\": site.domain",
    ] {
        assert!(
            site_page.contains(needle),
            "服务端站点 Schema 缺少删除确认链：{needle}"
        );
    }
    for banned in [
        "confirm: hostConfirm",
        "skiffConfirmAnswer",
        "skiffSiteDelete",
        "siteDetailSchema",
    ] {
        assert!(!js.contains(banned), "确认框不应再自建图层：{banned}");
    }
    assert!(
        !js.contains("window.prompt("),
        "不再允许 window.prompt；输入型确认必须用内置 confirm.requireText"
    );
    assert!(
        !js.contains("window.alert("),
        "不允许用阻塞 alert 反馈结果；成功/失败统一走 toast"
    );
    assert_eq!(
        js.matches("window.confirm(").count(),
        1,
        "window.confirm 只允许保留在 hostConfirm 的运行时未就绪兜底分支"
    );
    assert!(
        !js.contains("window.confirm('确认删除 ' + display"),
        "Docker 资源删除确认必须走宿主图层"
    );
}

#[test]
fn budding_boot_p1_confirms_and_kill_entry_present() {
    let js = include_str!("../static/budding/budding-boot.js");
    let containers_page = include_str!("../src/handlers/ui/containers_page.rs");
    // 容器强制终止入口（后端 action_verb 白名单已支持 kill）；列表骨架在服务端。
    assert!(
        containers_page.contains("container_op(\"强制终止\", \"kill\", c)"),
        "容器详情缺少强制终止入口"
    );
    for needle in [
        "title: '强制终止容器'",
        // 站点破坏性动作确认。
        "title: '停止站点'",
        "title: '关闭 HTTPS'",
        "title: '移除附加域名'",
        "title: '批量停止站点'",
        // 设置与插件侧确认。
        "title: '撤销公钥'",
        "title: '卸载插件'",
        "title: '恢复默认菜单'",
    ] {
        assert!(js.contains(needle), "P1 确认/入口缺失：{needle}");
    }
    // 表单就地数值校验：字段随页面迁到服务端 schema。
    for (source, needle) in [
        (
            include_str!("../src/handlers/ui/runtimes_page.rs"),
            "\"pattern\": \"^[0-9]{1,5}$\"",
        ),
        (
            include_str!("../src/handlers/ui/web_page.rs"),
            "\"pattern\": \"^[0-9]+$\"",
        ),
    ] {
        assert!(source.contains(needle), "表单数值校验缺失：{needle}");
    }
}

/// 站点目录入口与归档恢复：两处都容易在改版时被「顺手简化」掉。
///
/// - 产品不提供站点目录浏览页签，详情概览的「站点目录 / 文档根」必须带文件管理链接；
/// - 备份页签的恢复必须走 `SITE_RESTORE` 接口，而不是前端自己调 tar。
#[test]
fn site_page_keeps_directory_links_and_restore_entry() {
    let site_page = include_str!("../src/handlers/ui/site_page.rs");
    let js = include_str!("../static/budding/budding-boot.js");
    for needle in [
        "files?path=",
        "\"打开文件管理\"",
        "\"op\": \"restore\"",
        "\"archive\": archive.name",
        "\"归档与恢复\"",
    ] {
        assert!(site_page.contains(needle), "站点详情缺入口：{needle}");
    }
    for needle in ["} else if (op === 'restore') {", "'/backups/restore'"] {
        assert!(js.contains(needle), "skiffSiteWrite 缺恢复分支：{needle}");
    }
}

/// P2 列表行内入口：站点列表不再只有详情入口，容器列表补齐启停/重启/删除。
#[test]
fn budding_boot_p2_row_actions_present() {
    let js = include_str!("../static/budding/budding-boot.js");
    let site_page = include_str!("../src/handlers/ui/site_page.rs");
    let containers_page = include_str!("../src/handlers/ui/containers_page.rs");
    // JS 里注册的自定义动作（内核服务端 schema 化后，props 在 Rust 里拼装）。
    for needle in [
        "skiffSiteToggle: siteToggle",
        "skiffContainerToggle: containerToggle",
    ] {
        assert!(js.contains(needle), "P2 行内入口缺失（JS）：{needle}");
    }
    // 行内启停按钮的 props 由服务端页面模块生成。
    assert!(
        containers_page
            .contains("\"启停\",\n            json!({ \"type\": \"skiffContainerToggle\""),
        "容器列表服务端 schema 缺少启停按钮"
    );
    assert!(
        containers_page
            .contains("\"props\": { \"id\": \"${id}\", \"name\": \"${name}\", \"status\": \"${state.code}\" }"),
        "容器列表服务端 schema 缺失 skiffContainerToggle props"
    );
    assert!(
        site_page.contains("\"props\": { \"id\": \"${id}\", \"domain\": \"${domain}\", \"status\": \"${status.code}\" }"),
        "站点列表服务端 schema 缺失 skiffSiteToggle props"
    );
}

/// 动作注册表键去重：`skiffDockerWrite` 曾与设置保存动作共用同名键，
/// 后注册的实现静默覆盖前一个，导致镜像/卷删除被劫持到 `/settings/docker`。
#[test]
fn budding_boot_action_registry_has_no_duplicate_keys() {
    let js = include_str!("../static/budding/budding-boot.js");
    let marker = "      actions: {";
    let start = js.find(marker).expect("actions 注册表缺失") + marker.len();
    let bytes = js.as_bytes();
    let mut depth = 1usize;
    let mut end = start;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(depth == 0, "actions 注册表未闭合");
    let block = &js[start..end];

    let mut keys = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        let Some((key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        {
            keys.push(key.to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut dup = Vec::new();
    for key in &keys {
        if !seen.insert(key.clone()) {
            dup.push(key.clone());
        }
    }
    assert!(dup.is_empty(), "动作注册表存在重复键：{dup:?}");
    assert_eq!(
        keys.iter()
            .filter(|k| k.as_str() == "skiffDockerWrite")
            .count(),
        1,
        "资源动作 skiffDockerWrite 必须唯一注册"
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.as_str() == "skiffDockerSettingsWrite")
            .count(),
        1,
        "Docker 设置保存动作必须独立命名"
    );
}

/// 资源动作必须映射到资源接口；设置保存只能用于 `/settings/docker` 表单。
/// 镜像 / 网络 / 卷 / 容器新建均已迁到 JSON 页面资产（声明式 `request`），
/// 设置保存随设置页迁到 `settings_page.rs`——全部改读各自产出端，不再钉脚本。
#[test]
fn budding_boot_docker_actions_map_to_resource_endpoints() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        js.contains("path = '/containers';"),
        "Docker 动作映射缺失：path = '/containers';"
    );
    for (asset, needles) in [
        (
            include_str!("../static/budding/pages/docker-images.json"),
            vec!["/ui/v1/images/pull", "/ui/v1/images/${id}/remove"],
        ),
        (
            include_str!("../static/budding/pages/docker-networks.json"),
            vec!["/ui/v1/networks", "/ui/v1/networks/${id}/remove"],
        ),
        (
            include_str!("../static/budding/pages/docker-volumes.json"),
            vec!["/ui/v1/volumes", "/ui/v1/volumes/${id}/remove"],
        ),
        (
            include_str!("../static/budding/pages/docker-container-new.json"),
            vec![
                "/ui/v1/containers",
                "\"type\": \"skiffDockerWrite\", \"props\": { \"op\": \"create\" }",
            ],
        ),
        (
            include_str!("../src/handlers/ui/settings_page.rs"),
            vec!["\"type\": \"skiffDockerSettingsWrite\"", "/settings/docker"],
        ),
    ] {
        for needle in needles {
            assert!(asset.contains(needle), "JSON 页面动作映射缺失：{needle}");
        }
    }
}

/// 服务端下发的弹窗必须能进注册表：`dialogError` 靠它重开同一张表单。
///
/// budding 1.8.3 起 dialog 的正文/底部在动作顶层 `content`，读 `props.body`
/// 会永远拿不到 body、静默不注册（提交失败只能退化成一闪而过的 toast）。
#[test]
fn budding_boot_registers_server_dialogs_from_content_slot() {
    let js = include_str!("../static/budding/budding-boot.js");
    let fn_body = js
        .split("function registerServerDialog(button)")
        .nth(1)
        .and_then(|s| s.split("\n  function ").next())
        .expect("registerServerDialog 必须存在");
    assert!(
        fn_body.contains("button.action.content"),
        "服务端弹窗必须从 action.content 取插槽"
    );
    assert!(
        !fn_body.contains("layer.body"),
        "不得再从 props.body（layer.body）取 body：该位置在 1.8.3+ 是空的"
    );
    // 资产里每个 dialog 的提交动作都要带 dialog 键，否则失败时找不到注册项。
    let row: serde_json::Value =
        serde_json::from_str(include_str!("../static/budding/pages/_file-row.json")).unwrap();
    fn walk_forms(node: &serde_json::Value, seen: &mut usize) {
        match node {
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk_forms(i, seen)),
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|t| t.as_str()) == Some("form") {
                    if let Some(submit) = map
                        .get("onEvent")
                        .and_then(|e| e.get("submit"))
                        .and_then(|s| s.as_array())
                        .and_then(|s| s.first())
                    {
                        assert!(
                            submit
                                .get("props")
                                .and_then(|p| p.get("dialog"))
                                .is_some_and(|d| d.as_str().is_some_and(|s| !s.is_empty())),
                            "弹窗表单的提交动作必须带 props.dialog：{submit}"
                        );
                        *seen += 1;
                    }
                }
                map.values().for_each(|v| walk_forms(v, seen));
            }
            _ => {}
        }
    }
    let mut forms = 0;
    walk_forms(&row, &mut forms);
    assert_eq!(forms, 6, "文件行模板里的弹窗表单数量变了");
}

/// 作业日志页：游标聚合与骨架都在服务端，脚本不再自建日志页。
#[test]
fn budding_boot_job_logs_page_comes_from_server() {
    let js = include_str!("../static/budding/budding-boot.js");
    // 页面取数地址由服务端 `/ui/v1/page` 回答，脚本只报「当前路径」。
    assert!(
        js.contains("function fetchPageSchema("),
        "页面分派必须走服务端取数地址"
    );
    for banned in ["function loadJobLogs(", "function jobLogsSchema("] {
        assert!(!js.contains(banned), "作业日志构建器已迁出：{banned}");
    }
    // 产出端：路由表登记隐藏路由，`page_source` 指到日志端点。
    let routes = include_str!("../src/handlers/ui/routes.rs");
    assert!(
        routes.contains("\"/jobs/:id/logs\", \"作业日志\""),
        "路由表必须登记作业日志隐藏路由"
    );
    assert!(
        routes.contains("\"/jobs/{}/logs?with_schema=true\""),
        "page_source 必须把作业日志指到日志端点"
    );
    let page = include_str!("../src/handlers/ui/jobs_page.rs");
    for needle in [
        "pub fn job_logs_schema(",
        "仅加载前 10 页，可刷新继续",
        "\"type\": \"skiffLogReload\"",
    ] {
        assert!(page.contains(needle), "作业日志页缺失：{needle}");
    }
    let handler = include_str!("../src/handlers/ui/jobs.rs");
    for needle in [
        ".route(\"/ui/v1/jobs/{id}/logs\", get(job_logs))",
        "pages >= 10",
    ] {
        assert!(handler.contains(needle), "作业日志端点缺失：{needle}");
    }
}

/// 文件弹窗表单：数值字段必须发数字，弹窗必须能展示服务端错误摘要。
#[test]
fn budding_boot_file_forms_send_typed_values_and_show_errors() {
    let js = include_str!("../static/budding/budding-boot.js");
    let chown = js
        .split("submitJson('/files/chown'")
        .nth(1)
        .expect("fileChown 必须提交 /files/chown");
    let body = &chown[..chown.find("})").unwrap_or(chown.len())];
    assert!(body.contains("uid: uid"), "chown 的 uid 必须发数字变量");
    assert!(body.contains("gid: gid"), "chown 的 gid 必须发数字变量");
    assert!(
        !body.contains("uid: uidRaw"),
        "不得把原始字符串直接发给 u32 字段"
    );
    assert!(js.contains("var uid = Number(uidRaw);"));
    assert!(js.contains("var gid = Number(gidRaw);"));
    assert!(js.contains("'uid 与 gid 必须是非负整数'"));

    assert!(js.contains("function dialogError("), "弹窗错误摘要函数缺失");
    assert!(
        js.contains("props: { type: 'error', title: '提交未完成', text: text }"),
        "弹窗顶部必须渲染 error alert"
    );
    assert!(js.contains("function formError("), "formError 分发函数缺失");

    for name in [
        "fileWrite",
        "fileRename",
        "fileCopy",
        "fileChmod",
        "fileChown",
        "fileArchive",
        "fileExtract",
    ] {
        let start = js
            .find(&format!("function {name}("))
            .unwrap_or_else(|| panic!("{name} 不存在"));
        let rest = &js[start..];
        let end = rest[1..]
            .find("\n  function ")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        assert!(
            rest[..end].contains("formError("),
            "{name} 的错误分支必须走 formError（弹窗内可见）"
        );
    }
}

/// 插件菜单的「发现 / 可更新」与商店升级包下载必须留在原生壳页，
/// 不得再退回旧 SSR（旧路由与模板按约定保留，只是产品前端不再链接）。
#[test]
fn budding_boot_keeps_plugin_tabs_and_store_upgrade_native() {
    let js = include_str!("../static/budding/budding-boot.js");
    // 路由映射由服务端 `/ui/v1/routes` 下发，产出端是 routes.rs 的内置页登记表。
    let routes = include_str!("../src/handlers/ui/routes.rs");
    for (key, path) in [
        ("core/plugins/discover", "/plugins/discover"),
        ("core/plugins/updates", "/plugins/updates"),
    ] {
        assert!(
            routes.contains(&format!("(\"{key}\", \"{path}\", PageKind::Page)")),
            "{key} 必须映射到原生插件页"
        );
    }
    // 页内标签：随插件页迁到服务端 schema，链接仍指向原生路径。
    let plugins_page = include_str!("../src/handlers/ui/plugins_page.rs");
    assert!(
        plugins_page.contains("kit::link(\"发现\", \"${prefix}/plugins/discover\")"),
        "页内标签应使用原生路径"
    );
    assert!(
        js.contains("/store/upgrades/' + encodeURIComponent(version) + '/download'"),
        "升级包下载应走 /ui/v1"
    );
}
/// 新增的界面设置与 Web 服务运维入口必须落在既有契约上：
/// 主色由服务端写进壳页内联 `<style>`；Web 动作复用 `/ui/v1/runtimes/web/action/{op}`。
#[test]
fn budding_boot_has_ui_theme_and_web_env_entries() {
    let js = include_str!("../static/budding/budding-boot.js");
    let spa = include_str!("../src/handlers/spa.rs");
    // 界面设置：路由登记 + 页面取数地址都在 routes.rs，脚本不再持有映射。
    let routes = include_str!("../src/handlers/ui/routes.rs");
    assert!(
        routes.contains("(\"core/settings/ui\", \"/settings/ui\", PageKind::Page)"),
        "界面设置必须映射到规范路径"
    );
    assert!(
        routes.contains("\"/settings/ui?with_schema=true\""),
        "界面设置的页面取数地址必须指到 /ui/v1/settings/ui"
    );
    // 主题 CSS 由服务端出：颜色计算只有 `skiff_app::theme` 一处，前端不再重算。
    assert!(
        spa.contains("pub fn theme_css(theme: &skiff_app::theme::Theme) -> String"),
        "壳主题必须由服务端生成 CSS（与 skiff_app::theme 同源）"
    );
    assert!(
        spa.contains(r#"<style id="skiff-shell-theme">{theme_css}</style>"#),
        "壳页必须内联主题样式"
    );
    assert!(
        !js.contains("applyShellTheme"),
        "前端不得再自带主题实现（亮度/对比度算法会与服务端漂移）"
    );
    assert!(
        !js.contains("schema.theme"),
        "budding 1.8.0-r7 根节点未注册 theme，禁止写进 schema 树"
    );
    assert!(
        js.contains("markShellChrome();") && js.contains("classList.add('bud-admin--chrome')"),
        "外壳换底色必须补 bud-admin--chrome 类（schema.theme 被编译期拒绝）"
    );
    assert!(
        js.contains("skiffUiThemeWrite: uiThemeWrite,"),
        "界面设置写动作必须注册"
    );
    // Web 服务运维入口随页面迁到服务端 schema（`web_page.rs`）。
    let web_page = include_str!("../src/handlers/ui/web_page.rs");
    for needle in [
        "web_button(\"安装 Web 服务\", \"install\", \"primary\", \"flat\")",
        "web_button(\"重启\", \"restart\", \"\", \"tonal\")",
        "web_button(\"重新安装\", \"rebuild\", \"\", \"tonal\")",
        "web_button(\"删除容器\", \"delete\", \"danger\", \"tonal\")",
        "if running { \"stop\" } else { \"start\" }",
        "{ \"type\": \"skiffWebEnv\", \"props\": { \"op\": op } }",
        "查看任务详情",
        "fn install_node(install: &Value)",
    ] {
        assert!(web_page.contains(needle), "Web 服务操作入口缺失：{needle}");
    }
    assert!(
        web_page.contains(
            "let installed = !container_state.is_empty() && container_state != \"missing\";"
        ),
        "未安装与已安装必须区分，避免出现无意义按钮"
    );
    // 安装进度改由表格 `source.autoRefresh` 轮询：壳脚本不再自己定时改 DOM。
    assert!(
        web_page.contains("table[\"source\"][\"autoRefresh\"] = json!(2000);"),
        "安装步骤表必须自带轮询，否则进度停在打开那一刻"
    );
    // 提交动作仍在壳脚本里（要 POST + 刷新），但目标与刷新时机是契约。
    for needle in [
        "'/runtimes/web/action/' + encodeURIComponent(op)",
        "skiffWebEnv: webEnvAction,",
    ] {
        assert!(js.contains(needle), "Web 服务操作入口缺失：{needle}");
    }
    // 安装/重建点击后不得先弹阻塞提示再刷新，否则旧作业结果会继续可见。
    let action = js
        .split("function webEnvAction(args)")
        .nth(1)
        .expect("webEnvAction 必须存在");
    let compact = action.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        compact
            .contains("if (op === 'install' || op === 'rebuild') { location.reload(); return; }"),
        "安装/重建提交后应立即进入当前作业视图"
    );
}

/// 「设置 → 界面」配色扩展：预设 + 主色 / 菜单底色 / 页面底色。
///
/// budding 1.8.0-r7 没有颜色选择器节点，前端只提交 `#rrggbb` 文本；服务端仍是
/// 唯一裁决（非法 400 不落库），配色经文档级 CSS 变量全站生效。
#[test]
fn budding_boot_ui_theme_presets_and_color_fields() {
    let js = include_str!("../static/budding/budding-boot.js");
    // 预设与配色表单随「设置 → 界面」页迁到服务端 schema。
    let settings_page = include_str!("../src/handlers/ui/settings_page.rs");
    for name in ["\"星蓝\"", "\"翡翠\"", "\"石墨\"", "\"暖岩\"", "\"玫瑰\""] {
        assert!(settings_page.contains(name), "预设方案缺少名称：{name}");
    }
    assert!(
        settings_page.contains("fn preset_card("),
        "预设方案应渲染为色卡而不是纯文字按钮"
    );
    assert!(
        settings_page.contains("\"className\": \"skiff-swatch\""),
        "预设色点应有描边类名（浅色在白底上可见）"
    );
    for needle in [
        "color_field(\"primary\",",
        "color_field(\n                    \"menu\",",
        "color_field(\"page\",",
        "\"name\": \"menu_custom\"",
        "let color_rules = |message: &str|",
        "\"pattern\": \"^#[0-9a-fA-F]{6}$\"",
    ] {
        assert!(settings_page.contains(needle), "配色扩展缺失：{needle}");
    }
    // 动作注册仍在壳脚本：写操作要 POST 并整页刷新；CSS 变量由服务端注入。
    for needle in [
        "skiffUiPreset: uiPresetApply,",
        "skiffUiThemeWrite: uiThemeWrite,",
    ] {
        assert!(js.contains(needle), "配色扩展缺失：{needle}");
    }
    let spa = include_str!("../src/handlers/spa.rs");
    for needle in [
        "--bud-admin-chrome-bg:rgb({menu}) !important",
        "--v-theme-background:{page} !important",
    ] {
        assert!(spa.contains(needle), "壳主题变量缺失：{needle}");
    }
    assert!(
        !js.contains("color-picker") && !js.contains("input-color"),
        "1.8.0-r7 没有颜色选择器节点，禁止引用"
    );
    // 关闭自定义时必须留空，避免旧值在下次开启时复活。
    assert!(
        js.contains("menu: menuCustom ? menu : ''"),
        "自定义关闭时不得提交菜单底色"
    );
    assert!(
        js.contains("formError(args, invalid + '必须是 #rrggbb 格式"),
        "颜色本地校验不通过时必须在弹窗/页面显示原因"
    );
}

/// 站点 / 运行环境列表的新建改为弹窗；独立深链页必须保持可用。
#[test]
fn budding_boot_create_dialogs_keep_deep_links() {
    let js = include_str!("../static/budding/budding-boot.js");
    // 深链页路由与独立表单都保留：路由与「页面结构去哪取」都在 `routes.rs`
    // （`DETAIL_ROUTES` 的 `/sites/new` + `page_source` 映射到 `/ui/v1/pages/sites-new`）。
    let routes_src = include_str!("../src/handlers/ui/routes.rs");
    for needle in [
        "(\"/sites/new\", \"新建站点\", PageKind::Page, \"/sites/list\")",
        "api(\"/pages/sites-new\")",
    ] {
        assert!(
            routes_src.contains(needle),
            "新建站点深链登记缺失：{needle}"
        );
    }
    for needle in [
        // 弹窗提交成功回列表刷新；独立页仍进入详情。
        "op === 'create' && data.id && !args.dialog",
        "if (args.dialog) closeTopLayer();",
        "formError(\n          args,\n          '操作失败：'",
    ] {
        assert!(js.contains(needle), "新建弹窗改造缺失：{needle}");
    }
    // 运行环境新建弹窗随页面迁到服务端：按钮 → dialog → 表单字段。
    let runtimes_page = include_str!("../src/handlers/ui/runtimes_page.rs");
    for needle in [
        "\"props\": { \"text\": \"新建运行环境\", \"variant\": \"flat\", \"color\": \"primary\" }",
        "\"props\": { \"title\": \"新建运行环境\", \"width\": 640 }",
        "\"name\": \"kind\"",
        "\"name\": \"version\"",
        "\"name\": \"apk_mirror\"",
        "let initial = json!({ \"kind\": default_kind, \"apk_mirror\": default_mirror });",
    ] {
        assert!(
            runtimes_page.contains(needle),
            "运行环境新建弹窗缺失：{needle}"
        );
    }
    // budding 1.8.3 契约（upstream compiler action.ts）：dialog 的 body/footer 是动作
    // 顶层 content 插槽，写在 props 里会整页 CompileError。文件行模板逐弹窗校验。
    let row: serde_json::Value =
        serde_json::from_str(include_str!("../static/budding/pages/_file-row.json")).unwrap();
    fn walk_dialogs(node: &serde_json::Value, found: &mut usize) {
        match node {
            serde_json::Value::Array(items) => {
                for item in items {
                    walk_dialogs(item, found);
                }
            }
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|t| t.as_str()) == Some("dialog") {
                    *found += 1;
                    assert!(
                        map.contains_key("content"),
                        "dialog 动作必须在顶层写 content：{node}"
                    );
                    if let Some(props) = map.get("props") {
                        assert!(
                            props.get("body").is_none() && props.get("footer").is_none(),
                            "dialog 的 body/footer 不能写进 props：{node}"
                        );
                    }
                }
                for value in map.values() {
                    walk_dialogs(value, found);
                }
            }
            _ => {}
        }
    }
    let mut dialogs = 0;
    walk_dialogs(&row, &mut dialogs);
    assert_eq!(
        dialogs, 6,
        "文件行模板的弹窗数量变了，请复核 content 插槽是否仍合规"
    );
    // 站点列表的新建/批量弹窗随页面骨架迁到服务端。
    let site_page = include_str!("../src/handlers/ui/site_page.rs");
    for needle in [
        "text: \"批量启停\"",
        "text: \"新建站点\"",
        "initial: Some(json!({ \"site_type\": \"static\" }))",
        "fn site_create_fields(",
        "fn dialog_button(",
        "\"width\": 620\n                        },\n                        \"content\": {",
        "\"width\": spec.width\n            },\n            \"content\": {",
    ] {
        assert!(
            site_page.contains(needle),
            "服务端站点列表弹窗缺失：{needle}"
        );
    }
    assert!(!js.contains("sitesSchema"), "站点列表骨架不得再由前端拼接");
    assert!(
        !js.contains("'安装运行环境'"),
        "运行环境列表不应再保留独立安装卡片"
    );
    assert!(
        !js.contains("type: 'navigate', props: { url: prefix + '/sites/new' }"),
        "列表新建不得再跳独立页面"
    );
}

/// 壳页 `admin` 根节点不得再写 1.8.4 已移除的 `content.breadcrumbs` 插槽。
///
/// 1.8.4 把它从字段与插槽表里删掉，旧写法会让整棵壳页 `CompileError`（面板打不开），
/// 关闭面包屑只能走 `props.breadcrumbs=false`。
#[test]
fn shell_schema_disables_breadcrumbs_via_props() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        js.contains("breadcrumbs: false"),
        "壳页必须用 props.breadcrumbs=false 关闭面包屑"
    );
    assert!(
        !js.contains("breadcrumbs: []"),
        "content.breadcrumbs 插槽在 budding 1.8.4 已移除，写了会整页编译失败"
    );
}

/// 安装进度只允许一个事实源：任务中心 `web.install` 作业。
///
/// 内存版 `site_install_progress` 必须彻底消失，否则页面会出现两套口径。
#[test]
fn web_install_progress_has_single_persisted_source() {
    let sources = [
        ("web handler", include_str!("../src/handlers/ui/web.rs")),
        ("nginx handler", include_str!("../src/handlers/nginx.rs")),
        ("app state", include_str!("../../app/src/lib.rs")),
        ("sites crud", include_str!("../../app/src/sites/crud.rs")),
        (
            "sites runtime",
            include_str!("../../app/src/sites/runtime.rs"),
        ),
    ];
    for (name, src) in sources {
        assert!(
            !src.contains("site_install_progress") && !src.contains("site_env.lock"),
            "{name} 不应再出现内存安装进度"
        );
    }
    let state = include_str!("../../app/src/lib.rs");
    assert!(
        state.contains("pub fn web_install_progress") && state.contains("TARGET_WEB_ENV"),
        "安装进度必须从任务中心最新 web.install 作业构建"
    );
    let crud = include_str!("../../app/src/sites/crud.rs");
    assert!(
        crud.contains("KIND_WEB_INSTALL") && crud.contains("set_steps"),
        "安装步骤必须写入作业快照"
    );
    let store = include_str!("../../store/src/db/jobs.rs");
    assert!(
        store.contains("set_job_steps") && store.contains("fail_job_interrupted"),
        "store 必须提供步骤持久化与重启中断收尾"
    );
}

/// 静态防回归：禁止 boot 向 schema 根写入未注册的顶层属性。
///
/// 白名单来自随包 budding 1.8.0-r7 的编译期属性集合：
/// `type` / `id` / `definitions` / `props` 为特殊键，其余为全局基础属性
/// （该版本未注册任何 `nodeAttributes`，所以 `theme` 之类一律不合法）。
/// 这类错误会让整棵 schema 编译失败、全站白屏，静态断言必须走在装机前面。
#[test]
fn budding_boot_only_writes_registered_schema_root_keys() {
    let js = include_str!("../static/budding/budding-boot.js");
    let allowed = [
        "type",
        "id",
        "definitions",
        "props",
        "data",
        "visibleOn",
        "hiddenOn",
        "permission",
        "permissions",
        "onEvent",
        "action",
        "to",
        "class",
        "className",
        "style",
        "content",
        "source",
    ];
    let mut rest = js;
    while let Some(pos) = rest.find("schema.") {
        let tail = &rest[pos + "schema.".len()..];
        let key_len = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        let key = &tail[..key_len];
        let after = tail[key_len..].trim_start();
        if !key.is_empty()
            && after.starts_with('=')
            && !after.starts_with("==")
            && !after.starts_with("=>")
        {
            assert!(
                allowed.contains(&key),
                "schema.{key} 未在 budding 1.8.1 注册，编译会整棵树失败"
            );
        }
        rest = tail;
    }
}

/// 1.8.0-r3 已闭环 T10 / T16 / code Pack：宿主不得再保留旧能力缺失时的绕过。
#[test]
fn budding_boot_drops_pre_r3_workarounds() {
    let js = include_str!("../static/budding/budding-boot.js");
    // T16：17 个业务语义图标全部有映射，不再只映射 clock/file/dot。
    // 图标映射随路由表迁到服务端 `routes.rs::MENU_ICONS`，断言跟到产出端。
    let routes_src = include_str!("../src/handlers/ui/routes.rs");
    for (name, glyph) in [
        ("overview", "mdi-view-dashboard"),
        ("globe", "mdi-web"),
        ("docker", "mdi-docker"),
        ("plug", "mdi-puzzle"),
        ("tools", "mdi-wrench"),
        ("settings", "mdi-cog"),
        ("container", "mdi-cube-outline"),
        ("shield", "mdi-shield-outline"),
        ("lock", "mdi-lock-outline"),
        ("network", "mdi-lan"),
        ("user", "mdi-account-outline"),
        ("activity", "mdi-pulse"),
        ("list", "mdi-format-list-bulleted"),
        ("archive", "mdi-archive-outline"),
    ] {
        assert!(
            routes_src.contains(&format!("(\"{name}\", \"{glyph}\")")),
            "菜单图标映射缺失：{name}: '{glyph}'"
        );
    }
    // T13：高亮包已改名 @budding/code，旧 highlight 注册与产物不得残留。
    assert!(
        js.contains("registerPack(\n        'code'"),
        "应注册 code Pack"
    );
    assert!(js.contains("budding-code-"), "code Pack 应走版本化文件名");
    assert!(
        !js.contains("budding-highlight-"),
        "不得再引用旧 highlight 产物"
    );
    assert!(!js.contains("@budding/highlight"), "文案不得再指向旧 Pack");
    // T10：列表走 source 游标分页（构件在 schema_kit，资产里声明的页面直接写 JSON）。
    let kit = include_str!("../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains(r#"props.insert("paging".into(), json!("cursor"))"#),
        "列表应使用游标分页"
    );
    assert!(
        include_str!("../static/budding/pages/docker-images.json")
            .contains(r#""source": { "url": "/ui/v1/images", "params": {} }"#),
        "镜像列表应有 source 路径"
    );
    assert!(js.contains("'/sites'"), "站点列表应有 source 路径");
    assert!(
        !js.contains("翻页交互待后续批次接入"),
        "翻页待接入提示应删除"
    );
    assert!(!js.contains("翻页待接入"), "翻页待接入提示应删除");
    assert!(!js.contains("旧版详情页"), "容器日志不得再指向旧详情页");
}

/// 1.8.0-r3 的代码渲染器注册名是 `code`、文本字段是 `modelValue`；
/// 可编辑表单字段已改用 `textarea`（避免代码编辑器提交空串），`code` 只作只读查看器；
/// 列表必须渲染宿主给定的全部行（审计页定高内部滚动），游标才与本页末行对齐。
#[test]
fn budding_boot_uses_registered_code_component_and_full_rows() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        !js.contains("code-viewer"),
        "宿主不得再使用未注册的 code-viewer 类型"
    );
    assert!(
        !js.contains("props: { code:"),
        "code 组件的文本字段是 modelValue，不是 code"
    );
    // `code` 组件只用于只读查看器（日志 / 配置预览），必须显式传 `modelValue` 绑定文本；
    // 可编辑表单字段（配置编辑器 / 文件编辑）统一走 `textarea`，不得再裸挂 `code`。
    // 页面模块里的只读查看器统一由 `schema_kit::code_block` 产出（日志 / 配置预览），
    // 契约钉在这一个构件上，避免各页各写一份后漂移。
    let kit = include_str!("../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains("pub fn code_block(text: &str, height: i64) -> Value {")
            && kit.contains("\"modelValue\": text")
            && kit.contains("\"readonly\": true"),
        "只读代码块必须由 schema_kit 统一产出并绑定 modelValue"
    );
    let mut viewers = 0;
    for (pos, _) in js.match_indices("type: 'code'") {
        let rest = &js[pos + "type: 'code'".len()..];
        let end = rest.find("type: '").unwrap_or(rest.len());
        let seg = &rest[..end];
        assert!(
            seg.contains("modelValue:"),
            "code 查看器必须绑定 modelValue：{seg}"
        );
        assert!(
            !seg.contains("name: '"),
            "code 不得再作可编辑表单字段：{seg}"
        );
        viewers += 1;
    }
    // 日志/配置查看器已随页面迁到服务端（`schema_kit::code_block`），壳脚本只剩
    // 容器终端一处：数量不再随页面迁移而变，只要求「有一处且绑定 modelValue」。
    assert!(viewers >= 1, "壳脚本至少保留终端查看器：{viewers}");
    // 列表行由宿主一次性给出，不得再被组件默认每页 10 条截断（构件层已固定）。
    assert!(kit.contains(r#""perPage": -1"#), "列表应渲染本页全部行");
    // 审计页骨架已迁到服务端：定高与场景空态文案在 audit_page.rs。
    let audit_page = include_str!("../src/handlers/ui/audit_page.rs");
    assert!(
        audit_page.contains("height: Some(420)") && audit_page.contains("暂无审计记录"),
        "审计列表应定高并内部滚动，且带场景空态文案"
    );
    // 四个菜单图标使用 1.8.1 子集的原生字形，不再近似兜底或留空。
    // 图标映射随路由表迁到服务端（`routes.rs`），断言跟着转过去。
    let routes_src = include_str!("../src/handlers/ui/routes.rs");
    assert!(
        routes_src.contains(r#"("folder", "mdi-folder-outline")"#),
        "文件管理应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("disk", "mdi-harddisk")"#),
        "卷应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("image", "mdi-image-outline")"#),
        "镜像应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("hard-drive", "mdi-harddisk")"#),
        "配额管理应使用原生字形"
    );
}

/// 登录态失效走 budding `redirect` 组件（1.8.1 新增）：401 不再只抛一行错误，
/// 宿主整页重挂一个 `redirect` 页面，并把各 JSON 通道的 401 都接到同一出口。
#[test]
fn budding_boot_session_expiry_uses_redirect_component() {
    let js = include_str!("../static/budding/budding-boot.js");
    assert!(
        js.contains("function showSessionExpired()"),
        "401 必须有统一的登录失效出口"
    );
    // 会话过期简化为硬跳转：立即 window.location.replace 到登录页，
    // 不再用 budding redirect 组件（无延迟、无按钮、不重新 embed）。
    assert!(
        js.contains("loginUrl()"),
        "跳转目标应构造带 next 的登录页 URL"
    );
    assert!(
        js.contains("window.location.replace"),
        "401 应硬跳转，不走 budding redirect 组件"
    );
    assert!(js.contains("/login?next="), "跳转目标应回带 next");
    // 每个 fetch 通道一处：会话引导、读数据、读裸 JSON、写通道、上传、插件 RPC、终端。
    // 写通道原先各写一份（submitJson / postPrefixJson），合并后总数为 8；阈值跟着
    // 实现走只会拦「通道被删掉」，不拦「多接一次」。
    let calls = js.matches("showSessionExpired()").count();
    assert!(
        calls >= 8,
        "401 出口覆盖不足：showSessionExpired 出现 {calls} 次"
    );
    assert!(
        !js.contains("会话已过期，请刷新页面后重新登录。"),
        "旧的一行错误文案应被硬跳转替代"
    );
}

/// 1.8.0-r7 起浮层归属图层根（上游 `docs/plan-layer-stacking.md`）：入库主包必须带
/// `data-bud-layer-root` 与浮层 `attach` 选择器，宿主 boot 则不得再注入层叠补丁。
/// 这条断言的作用是「换包时别把层级契约换丢」——升级到没有该契约的旧包会当场失败。
#[test]
fn budding_bundle_carries_layer_root_attach_contract() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/budding");
    let mut bundles: Vec<String> = std::fs::read_dir(&dir)
        .expect("static/budding 必须存在")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with("budding-")
                && name.ends_with(".min.js")
                && !name.starts_with("budding-code-")
                && !name.starts_with("budding-terminal-")
        })
        .collect();
    bundles.sort();
    assert_eq!(
        bundles.len(),
        1,
        "目录里应只有一份主包（升级时删除旧产物，避免 immutable 命中旧缓存）：{bundles:?}"
    );
    let js = std::fs::read_to_string(dir.join(&bundles[0])).expect("主包应可读");
    assert!(
        js.contains("data-bud-layer-root"),
        "主包应声明图层根标识（浮层 attach 目标）：{}",
        bundles[0]
    );
    assert!(
        js.contains("[data-bud-layer-root]"),
        "主包应把浮层 attach 到图层根选择器：{}",
        bundles[0]
    );
    let boot = include_str!("../static/budding/budding-boot.js");
    for banned in ["v-overlay-container", "data-bud-layer-root"] {
        assert!(
            !boot.contains(banned),
            "宿主不得再为弹窗内浮层打补丁（出现 {banned}），层级归上游契约负责"
        );
    }
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

/// 插件中心与信息页规范路径交付壳页。
#[tokio::test]
async fn plugins_canonical_paths_serve_shell() {
    let h = harness();
    seed_plugin(h.state.db(), "com.example.ui", "示例界面", "示例", true);
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/p/plugins/installed", "/p/plugins/com.example.ui"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
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

/// 文件管理规范页交付 budding 壳页（原生平铺目录）；回收站/配额同为壳页。
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
    for uri in ["/p/files/trash", "/p/files/quotas"] {
        let (status, html) = get_page(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
        let (status, _) = get_page(&h.app, uri, &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} viewer 必须 403");
    }
}

/// 文件回收站/配额写接口：CSRF 先行、viewer 被权限矩阵拒绝；管理员过门禁后
/// 走真实 fs 语义（不存在的回收站条目 → 404，不伪装成功）。
#[tokio::test]
async fn files_trash_and_quota_writes_require_csrf_and_permission() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);
    let root = h.data_dir.join("www");
    std::fs::create_dir_all(&root).unwrap();

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("/p/files/trash/restore", json!({ "id": "t-missing" })),
        ("/p/files/trash/purge", json!({ "id": "t-missing" })),
        ("/p/files/trash/clear", json!({})),
        (
            "/p/files/quotas/set",
            json!({ "path": root.display().to_string(), "max_bytes": 1024 }),
        ),
        (
            "/p/files/quotas/cancel",
            json!({ "path": root.display().to_string() }),
        ),
        (
            "/p/files/quotas/scan",
            json!({ "path": root.display().to_string() }),
        ),
    ];

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} 缺 CSRF 必须 403: {text}"
        );
    }

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} viewer 必须 403: {text}"
        );
    }

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, Some(&csrf), body).await;
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let code = v["error"]["code"].as_str().unwrap_or("");
        assert!(
            code != "403" && code != "auth.forbidden",
            "{uri} 管理员不应被权限拒绝: {text}"
        );
        if uri.ends_with("/restore") || uri.ends_with("/purge") {
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {text}");
        }
    }
}

/// 设置里的菜单与 Docker 设置：规范页交壳页、读接口按角色返回、写接口 CSRF 先行、
/// Docker 写仅管理员可用，菜单偏好可写自己的账号。
#[tokio::test]
async fn ui_settings_menu_and_docker_require_csrf_and_permission() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    for uri in [
        "/ui/v1/settings/menu",
        "/ui/v1/settings/docker",
        "/ui/v1/settings/ui",
    ] {
        let (status, _, body) = get_ui(&h.app, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "auth.unauthenticated", "{uri}");
    }

    for token in [&admin, &viewer] {
        for uri in ["/p/settings/menu", "/p/docker/settings", "/p/settings/ui"] {
            let (status, html) = get_html(&h.app, uri, token).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
        }
    }

    // 菜单读对登录用户开放；Docker / 界面读对 viewer 也开放但 can_admin=false。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/menu", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["groups"].is_array(), "{body}");
    assert_eq!(v["data"]["layout"], serde_json::Value::Null, "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/docker", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["can_admin"], false, "{body}");

    // 写接口：缺 CSRF 一律 403，先于权限与业务。
    for (uri, body) in [
        (
            "/ui/v1/settings/menu",
            json!({ "action": "hide", "key": "core/docker/images" }),
        ),
        (
            "/ui/v1/settings/docker",
            json!({ "registry_mirror": "", "image_prefix": "" }),
        ),
        ("/ui/v1/settings/ui", json!({ "primary": "#2f6feb" })),
    ] {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }

    // Docker 写：viewer 即使有 CSRF 也被权限矩阵拒绝。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/docker",
        &viewer,
        Some(&viewer_csrf),
        &json!({ "registry_mirror": "", "image_prefix": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden");

    // 管理员写：镜像源留空不触达宿主 daemon.json，只落镜像前缀。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/docker",
        &admin,
        Some(&csrf),
        &json!({ "registry_mirror": "", "image_prefix": "mirror.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");

    // 菜单偏好：viewer 与管理员各自可写自己的账号；隐藏后读响应立即反映。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/menu",
        &viewer,
        Some(&viewer_csrf),
        &json!({ "action": "hide", "key": "core/docker/images" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/menu", Some(&viewer), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let images = v["data"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap())
        .find(|i| i["key"] == "core/docker/images")
        .expect("菜单快照必须包含 Docker→镜像");
    assert_eq!(images["hidden"], true, "{body}");
}

/// 界面设置（壳主题）：读对所有登录用户开放；写仅管理员；主色校验、规范化、
/// 持久化并经 `/ui/v1/session` 下发，刷新后仍生效。
#[tokio::test]
async fn ui_theme_requires_admin_validates_and_persists() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    // 缺省主色随会话下发，壳页首屏即按全站主题渲染。
    let (_, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&viewer), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["theme"]["primary"], "#1867c0", "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["primary"], "#1867c0", "{body}");
    assert_eq!(v["data"]["can_admin"], false, "{body}");

    // viewer 即使带 CSRF 也不能改全局主题。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &viewer,
        Some(&viewer_csrf),
        &json!({ "primary": "#2f6feb" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["error"]["code"], "auth.forbidden", "{text}");

    // 非法格式 400，且不落库。
    for bad in ["blue", "#12345", "#1234567"] {
        let (status, text) = post_ui_json(
            &h.app,
            "/ui/v1/settings/ui",
            &admin,
            Some(&csrf),
            &json!({ "primary": bad }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {text}");
    }
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&admin), None).await;
    assert!(body.contains("#1867c0"), "{body}");

    // 合法写入规范化为小写并持久化；session 立即下发新主色。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({ "primary": "#2F6FEB" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&admin), None).await;
    assert!(body.contains(r##""primary":"#2f6feb""##), "{body}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&viewer), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["theme"]["primary"], "#2f6feb", "{body}");
    assert!(
        h.state
            .db()
            .recent_audit(10)
            .unwrap()
            .iter()
            .any(|e| e.action == "settings.ui.update"
                && e.result == "ok"
                && e.detail.contains("#2f6feb")),
        "主题写入必须留审计"
    );

    // 空串恢复缺省主色。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({ "primary": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&admin), None).await;
    assert!(body.contains("#1867c0"), "{body}");
}

/// 配色扩展：菜单底色随页面自动推导、自定义菜单、主色对比度校正，
/// 以及任一颜色非法时 400 且不落库。
#[tokio::test]
async fn ui_theme_menu_page_and_contrast() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);

    // 浅色页面 + 自动菜单：白底黑字。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({ "primary": "#2f6feb", "page": "#F5F5F5", "menu": "", "menu_custom": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["menu"], "#ffffff", "{text}");
    assert_eq!(v["data"]["menu_text"], "#000000", "{text}");
    assert_eq!(v["data"]["page_text"], "#000000", "{text}");
    assert_eq!(v["data"]["adjusted"], false, "{text}");

    // 深色页面 + 自动菜单：近黑菜单、白字；主色按对比度校正。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({ "primary": "#2f6feb", "page": "#101418", "menu": "", "menu_custom": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["menu"], "#121212", "{text}");
    assert_eq!(v["data"]["menu_text"], "#ffffff", "{text}");
    assert_eq!(v["data"]["page_text"], "#ffffff", "{text}");
    assert_eq!(v["data"]["adjusted"], true, "{text}");
    let primary = v["data"]["primary"].as_str().unwrap();
    assert!(
        skiff_app::theme::contrast(primary, "#121212") >= skiff_app::theme::MIN_CONTRAST,
        "校正后主色对比度不足：{primary}"
    );

    // 自定义菜单底色：会话与设置页读到的都是自定义值。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({
            "primary": "#1867c0",
            "page": "#fff7ed",
            "menu": "#1f2933",
            "menu_custom": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["menu"], "#1f2933", "{text}");
    assert_eq!(v["data"]["menu_custom"], true, "{text}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&admin), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["theme"]["menu"], "#1f2933", "{body}");
    assert_eq!(v["data"]["theme"]["menu_custom"], true, "{body}");
    assert_eq!(v["data"]["theme"]["page"], "#fff7ed", "{body}");

    // 任一颜色非法都要 400，且不改动已保存的整组配色。
    for payload in [
        json!({ "primary": "#1867c0", "page": "orange" }),
        json!({ "primary": "#1867c0", "page": "#fff7ed", "menu": "#12345" }),
    ] {
        let (status, text) =
            post_ui_json(&h.app, "/ui/v1/settings/ui", &admin, Some(&csrf), &payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}: {text}");
    }
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&admin), None).await;
    assert!(body.contains(r##""menu":"#1f2933""##), "{body}");
    assert!(body.contains(r##""page":"#fff7ed""##), "{body}");
}

/// 运行环境配置/安装/扩展写接口：未认证 401、viewer 403、缺 CSRF 403；
/// 详情聚合查询复用旧页同一数据源（只验证形状与门禁，真实安装需 Docker）。
#[tokio::test]
async fn ui_runtime_writes_require_csrf_and_permission() {
    let h = harness();
    h.state
        .db()
        .create_runtime(&skiff_store::NewRuntime {
            id: "php-8.5-test".into(),
            kind: "php".into(),
            name: "php-8.5-test".into(),
            version: "8.5".into(),
            image: "php:8.5-fpm".into(),
            image_digest: String::new(),
            socket_path: None,
            port: None,
            is_default: false,
        })
        .unwrap();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let csrf = h.state.csrf_token(&admin);
    let viewer_csrf = h.state.csrf_token(&viewer);

    let (status, _, body) = get_ui(&h.app, "/ui/v1/runtimes/php-8.5-test", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], "auth.unauthenticated");

    let (status, _, body) =
        get_ui(&h.app, "/ui/v1/runtimes/php-8.5-test", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, _, body) =
        get_ui(&h.app, "/ui/v1/runtimes/php-8.5-test", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["runtime"]["id"], "php-8.5-test", "{body}");
    assert_eq!(v["data"]["is_php"], true, "{body}");
    assert!(v["data"]["config"]["memory_limit"].is_string(), "{body}");
    assert!(v["data"]["extensions"].is_array(), "{body}");
    assert!(v["data"]["files"].is_array(), "{body}");

    // 规范详情页交 budding 壳页。
    let (status, html) = get_html(&h.app, "/p/sites/runtimes/php-8.5-test", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");

    let cases = [
        (
            "/ui/v1/runtimes",
            json!({ "kind": "php", "version": "8.5", "name": "new-php" }),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/action/restart",
            json!({ "confirm": true }),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/config",
            json!({ "memory_limit": "128M" }),
        ),
        ("/ui/v1/runtimes/php-8.5-test/node-config", json!({})),
        (
            "/ui/v1/runtimes/php-8.5-test/port",
            json!({ "port": 9000, "confirm": true }),
        ),
        ("/ui/v1/runtimes/php-8.5-test/clone", json!({})),
        (
            "/ui/v1/runtimes/php-8.5-test/extensions",
            json!({ "extensions": [] }),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/extensions/batch",
            json!({ "op": "install", "keys": ["redis"] }),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/extensions/redis/install",
            json!({}),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/pecl",
            json!({ "name": "yaml" }),
        ),
        (
            "/ui/v1/runtimes/php-8.5-test/config-file",
            json!({ "key": "php-ini", "content": ";" }),
        ),
        ("/ui/v1/runtimes/php-8.5-test/probe", json!({})),
        (
            "/ui/v1/runtimes/php-8.5-test/sites/example.com/unbind",
            json!({}),
        ),
        (
            "/ui/v1/runtimes/images",
            json!({ "nginx_image": "", "php_image": "" }),
        ),
    ];

    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &admin, None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} 缺 CSRF: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.csrf_invalid", "{uri}");
    }
    for (uri, body) in &cases {
        let (status, text) = post_ui_json(&h.app, uri, &viewer, Some(&viewer_csrf), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} viewer: {text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }

    // 管理员过门禁：镜像参数写库不依赖 Docker，必须 200。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/runtimes/images",
        &admin,
        Some(&csrf),
        &json!({ "nginx_image": "", "php_image": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
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

/// 任务/作业规范页交付壳页（页面不做 RBAC，数据由 `/ui/v1/*` 判权）。
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
        // Basic 档不提供 ComposeUp/Down 动词：这里测的是「无该能力」的档位拒绝，
        // 不是 CSRF 失败。CSRF 覆盖见 `ui_compose_validate_and_up_require_csrf_and_permission`。
        assert_eq!(v["error"]["code"], "auth.forbidden", "{uri}");
    }
}

/// compose 规范页交付壳页。
#[tokio::test]
async fn compose_canonical_path_serves_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, html) = get_html(&h.app, "/p/docker/compose", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"id="skiff-root""#), "{html}");
}

/// 镜像/网络/卷规范页交付壳页。
#[tokio::test]
async fn resource_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in [
        "/p/docker/images",
        "/p/docker/networks",
        "/p/docker/volumes",
    ] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
}

/// 容器规范页交付壳页。
#[tokio::test]
async fn containers_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in ["/p/docker/containers", "/p/docker/containers/abc"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
        );
    }
}

/// 站点/运行环境规范页交付壳页。
#[tokio::test]
async fn sites_and_runtimes_canonical_paths_serve_shell() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    for uri in [
        "/p/sites/list",
        "/p/sites/a.local",
        "/p/sites/runtimes",
        "/p/sites/runtimes/php-8.5",
    ] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(
            html.contains(r#"id="skiff-root""#),
            "{uri} 应为壳页: {html}"
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

// ---- /ui/v1 DTO 冻结：关键响应 key-set 快照（SDUI 集成 §3.7）----
//
// 规则：`/ui/v1` 的响应字段只增不改。新增可选字段不算破坏，但必须同步更新这里的
// 冻结清单；删除字段、改名或改变语义必须升 `api_version`，并写进 SDUI 集成文档。
// 这里断言 key-set 而不是 golden JSON：时间、计数、路径随环境变化，字段集才是契约。

/// 取 JSON 对象的键（非对象直接失败）。
fn object_keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("期望 JSON 对象，实际: {v}"))
        .keys()
        .cloned()
        .collect()
}

/// 断言对象键集与冻结清单完全一致（顺序无关）。
fn assert_key_set(v: &serde_json::Value, expected: &[&str], ctx: &str) {
    let mut actual = object_keys(v);
    actual.sort();
    let mut expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(
        actual, expected,
        "{ctx} 字段集漂移：新增可选字段请更新冻结清单；删除/改名/改语义必须升 api_version 并写进 SDUI 集成文档"
    );
}

/// 错误信封、登录引导与会话端点是壳页启动的硬依赖，字段集先冻结。
#[tokio::test]
async fn ui_v1_envelope_and_bootstrap_key_sets_are_frozen() {
    let h = harness();

    // 未认证：信封与错误码是前端分支依据（`field` 允许为 null，但键必须存在）。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/session", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(&v, &["error"], "错误信封顶层");
    assert_key_set(
        &v["error"],
        &["code", "status", "message", "field", "retryable"],
        "错误信封 error",
    );

    // 登录引导是公开端点：字段与 session 的 `api_version` 同源。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/auth/login", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["csrf_token", "entry_prefix", "api_version"],
        "GET /ui/v1/auth/login",
    );

    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "username",
            "role",
            "csrf_token",
            "entry_prefix",
            "api_version",
            "theme",
            "features",
        ],
        "GET /ui/v1/session",
    );
    assert_key_set(
        &v["data"]["features"],
        &["shell_fallback", "placeholders", "plugin_iframe"],
        "GET /ui/v1/session features",
    );
    assert_key_set(
        &v["data"]["theme"],
        &[
            "primary",
            "menu",
            "page",
            "menu_custom",
            "menu_text",
            "page_text",
            "on_primary",
            "adjusted",
        ],
        "GET /ui/v1/session theme",
    );
}

/// 关键读接口的 `data` 字段集与资源行字段集冻结；新增字段必须显式更新本用例。
#[tokio::test]
async fn ui_v1_key_response_shapes_are_frozen() {
    /// 站点列表项（`SiteItem`）：列表与详情共用，字段只增不改。
    const SITE_ITEM_KEYS: &[&str] = &[
        "id",
        "domain",
        "root_dir",
        "index_dir",
        "site_type",
        "type_label",
        "runtime_id",
        "runtime_label",
        "runtime_name",
        "runtime_shared_hint",
        "proxy_target",
        "proxy_websocket",
        "endpoint",
        "status",
        "maintenance_reason",
        "https",
        "is_default",
        "aliases",
        "rewrite",
        "limit_rate_kbps",
        "created_at",
    ];

    // 商店接口在目录未启用时返回 404，这里用启用目录的 harness 覆盖 200 形状。
    let h = harness_full_with_catalog(
        Form::Native,
        Tier::Basic,
        skiff_app::catalog::CatalogSettings {
            enabled: true,
            ..Default::default()
        },
    );
    h.state.init_tasks().expect("任务服务应能初始化");
    let root = h.data_dir.join("www/sites/a.local");
    std::fs::create_dir_all(&root).unwrap();
    h.state
        .db()
        .create_site(&php_site("a.local", &root.display().to_string()))
        .unwrap();
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
    seed_plugin(
        h.state.db().as_ref(),
        "com.example.freeze",
        "Freeze",
        "Freeze",
        true,
    );
    h.state
        .db()
        .audit(
            "admin",
            "10.0.0.1",
            "site.update",
            "freeze.example",
            "ok",
            "freeze",
        )
        .unwrap();
    // harness 的 data_dir 是受保护路径，文件管理改看独立临时目录。
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(scratch.path().join("sites")).unwrap();
    std::fs::write(scratch.path().join("note.txt"), b"hello").unwrap();
    let files_uri = format!(
        "/ui/v1/files?path={}",
        scratch.path().to_string_lossy().replace('\\', "/")
    );

    let token = login(&h.app, "admin", "pw-test-123").await;

    let (status, _, body) = get_ui(&h.app, "/ui/v1/overview", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "collected_at",
            "docker",
            "containers",
            "sites",
            "plugins",
            "runtimes",
            "health_plugin",
            "health_reason",
        ],
        "GET /ui/v1/overview",
    );
    assert_key_set(
        &v["data"]["docker"],
        &["status", "reason"],
        "overview.docker",
    );
    assert_key_set(
        &v["data"]["docker"]["status"],
        &["code", "label"],
        "overview.docker.status",
    );
    assert_key_set(
        &v["data"]["containers"],
        &["running", "total"],
        "overview.containers",
    );
    assert_key_set(
        &v["data"]["sites"],
        &["total", "abnormal"],
        "overview.sites",
    );
    assert_key_set(
        &v["data"]["plugins"],
        &["total", "enabled", "disabled"],
        "overview.plugins",
    );
    assert_key_set(
        &v["data"]["runtimes"],
        &["total", "abnormal", "uninstalled"],
        "overview.runtimes",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(&v["data"], &["groups"], "GET /ui/v1/menu");
    let groups = v["data"]["groups"].as_array().expect("menu.groups");
    assert!(!groups.is_empty(), "菜单至少应有一个分组: {body}");
    assert_key_set(
        &groups[0],
        &["key", "label", "icon", "href", "items", "empty"],
        "menu.group",
    );
    if let Some(item) = groups
        .iter()
        .flat_map(|g| g["items"].as_array().into_iter().flatten())
        .next()
    {
        assert_key_set(
            item,
            &[
                "key",
                "label",
                "icon",
                "href",
                "plugin",
                "plugin_id",
                "default_view",
                "views",
            ],
            "menu.item",
        );
        if let Some(view) = item["views"].as_array().and_then(|a| a.first()) {
            assert_key_set(view, &["view", "label"], "menu.item.views[]");
        }
    }

    let (status, _, body) = get_ui(&h.app, "/ui/v1/sites", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["items", "total", "next_cursor", "can_admin"],
        "GET /ui/v1/sites",
    );
    let site = v["data"]["items"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一条站点");
    assert_key_set(site, SITE_ITEM_KEYS, "site item");
    assert_key_set(&site["status"], &["code", "label"], "site item status");
    assert_key_set(
        &site["https"],
        &["enabled", "force_https", "label", "class"],
        "site item https",
    );

    let detail_uri = format!(
        "/ui/v1/sites/{}",
        site["id"].as_i64().expect("站点 id 必须是数字")
    );
    let (status, _, body) = get_ui(&h.app, &detail_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let detail = &v["data"];
    let mut detail_keys: Vec<String> = SITE_ITEM_KEYS.iter().map(|s| (*s).to_string()).collect();
    for extra in [
        "ssl",
        "runtime",
        "runtime_down",
        "runtime_alert",
        "config",
        "backup",
        "can_manage",
    ] {
        detail_keys.push(extra.to_string());
    }
    let mut actual: Vec<String> = detail_keys;
    actual.sort();
    let mut got = object_keys(detail);
    got.sort();
    assert_eq!(got, actual, "site detail 字段集漂移");
    // 详情响应顶层多一个 `schema`：只校验形状，不锁定内部节点。
    assert_eq!(v["schema"]["type"], "page", "详情响应必须带回服务端 Schema");
    assert_eq!(v["schema"]["props"]["title"], detail["domain"]);
    assert_key_set(
        &detail["ssl"],
        &[
            "enabled",
            "force_https",
            "cert_exists",
            "cert_path",
            "key_path",
            "subject",
            "issuer",
            "expires",
        ],
        "site detail ssl",
    );
    assert_key_set(
        &detail["config"],
        &["path", "content", "active", "error"],
        "site detail config",
    );
    assert_key_set(
        &detail["backup"],
        &[
            "available",
            "enabled",
            "task_id",
            "interval",
            "time",
            "last_text",
            "next_text",
            "archives",
        ],
        "site detail backup",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/runtimes", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["items", "total", "next_cursor"],
        "GET /ui/v1/runtimes",
    );
    let runtime = v["data"]["items"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一条运行环境");
    assert_key_set(
        runtime,
        &[
            "id",
            "kind",
            "name",
            "container_name",
            "version",
            "version_display",
            "version_note",
            "image",
            "state",
            "state_class",
            "drift",
            "site_count",
            "container_state",
            "endpoint_short",
            "endpoint_full",
            "implemented",
            "is_default",
            "created_at",
            "updated_at",
            "job",
        ],
        "runtime item",
    );
    assert_key_set(&runtime["kind"], &["code", "label"], "runtime item kind");
    assert_key_set(&runtime["state"], &["code", "label"], "runtime item state");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/containers", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "items",
            "total",
            "next_cursor",
            "filter",
            "docker_available",
            "docker_reason",
            "can_operate",
            "can_delete",
            "can_admin",
        ],
        "GET /ui/v1/containers",
    );

    for uri in ["/ui/v1/images", "/ui/v1/networks", "/ui/v1/volumes"] {
        let (status, _, body) = get_ui(&h.app, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_key_set(
            &v["data"],
            &[
                "items",
                "total",
                "next_cursor",
                "docker_available",
                "docker_reason",
                "can_admin",
            ],
            uri,
        );
    }

    let (status, _, body) = get_ui(&h.app, "/ui/v1/tasks", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["items", "total", "next_cursor", "can_admin"],
        "GET /ui/v1/tasks",
    );
    if let Some(task) = v["data"]["items"].as_array().and_then(|a| a.first()) {
        assert_key_set(
            task,
            &[
                "id",
                "name",
                "expr",
                "next_text",
                "source",
                "target",
                "method",
                "enabled",
                "state_label",
                "state_class",
                "last_text",
                "last_class",
                "builtin",
            ],
            "task item",
        );
    }

    let (status, _, body) = get_ui(&h.app, "/ui/v1/jobs", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["items", "total", "next_cursor", "can_admin"],
        "GET /ui/v1/jobs",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/audit", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["items", "keyword", "has_more", "next_cursor"],
        "GET /ui/v1/audit",
    );
    let row = v["data"]["items"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一条审计记录");
    assert_key_set(
        row,
        &[
            "id", "ts", "ts_label", "actor", "ip", "action", "target", "result", "detail",
        ],
        "audit item",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/plugins", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "plugins",
            "enabled_count",
            "disabled_count",
            "can_admin",
            "allow_unsigned",
            "reduced_allowed",
        ],
        "GET /ui/v1/plugins",
    );
    let plugin = v["data"]["plugins"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一条插件");
    assert_key_set(
        plugin,
        &[
            "id",
            "name",
            "version",
            "api",
            "status",
            "enabled",
            "key_id",
            "signed",
            "reduced",
            "running",
            "requires",
            "permissions",
            "grants",
            "ui_path",
            "takedown_note",
        ],
        "plugin item",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/store", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "state",
            "message",
            "channel",
            "seq",
            "fetched_at",
            "source",
            "allow_unsigned",
            "key_configured",
            "can_install",
            "total",
            "sections",
            "upgrade",
        ],
        "GET /ui/v1/store",
    );
    assert_key_set(&v["data"]["state"], &["code", "label"], "store.state");
    assert_key_set(
        &v["data"]["upgrade"],
        &[
            "current",
            "arch",
            "available",
            "channel",
            "min_from",
            "changelog_url",
            "note",
            "can_download",
        ],
        "store.upgrade",
    );
    if let Some(section) = v["data"]["sections"].as_array().and_then(|a| a.first()) {
        assert_key_set(
            section,
            &["title", "hint", "installable", "items"],
            "store.section",
        );
        if let Some(item) = section["items"].as_array().and_then(|a| a.first()) {
            assert_key_set(
                item,
                &[
                    "name",
                    "version",
                    "source",
                    "updated_at",
                    "compatibility",
                    "note",
                    "warn",
                    "installable",
                    "action",
                    "install_note",
                ],
                "store.item",
            );
        }
    }

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["panel", "account", "keys", "can_admin", "flash"],
        "GET /ui/v1/settings",
    );
    assert_key_set(
        &v["data"]["panel"],
        &[
            "listen_addr",
            "listen_port",
            "entry_prefix",
            "tls_mode",
            "tls_cert",
            "tls_key",
            "config_path",
            "plaintext_behind_proxy",
            "initial_admin_pending",
        ],
        "settings.panel",
    );
    assert_key_set(
        &v["data"]["account"],
        &[
            "totp_enabled",
            "must_change_password",
            "totp_pending_secret",
            "totp_pending_uri",
        ],
        "settings.account",
    );
    assert_key_set(&v["data"]["flash"], &["notice", "error"], "settings.flash");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/menu", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(&v["data"], &["groups"], "GET /ui/v1/settings/menu");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "primary",
            "menu",
            "page",
            "menu_custom",
            "menu_text",
            "page_text",
            "on_primary",
            "adjusted",
            "can_admin",
        ],
        "GET /ui/v1/settings/ui",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/docker", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["registry_mirror", "image_prefix", "can_admin"],
        "GET /ui/v1/settings/docker",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/compose", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "projects",
            "services",
            "selected_project",
            "docker_available",
            "docker_reason",
            "can_admin",
        ],
        "GET /ui/v1/compose",
    );

    let (status, _, body) = get_ui(&h.app, "/ui/v1/web", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &["status", "install", "config", "modules", "can_admin"],
        "GET /ui/v1/web",
    );
    assert_key_set(
        &v["data"]["status"],
        &[
            "available",
            "reason",
            "container_state",
            "version",
            "config_test",
            "uptime",
            "last_reload",
            "probe_url",
            "probe_error",
            "conf_path",
            "conf_has_stub",
            "active",
            "accepts",
            "handled",
            "requests",
            "reading",
            "writing",
            "waiting",
            "stub_reason",
        ],
        "web.status",
    );
    assert_key_set(
        &v["data"]["install"],
        &[
            "job_id",
            "status",
            "running",
            "done",
            "interrupted",
            "phase",
            "phase_label",
            "progress",
            "error",
            "created_by",
            "created_by_role",
            "requester_ip",
            "created_at",
            "started_at",
            "finished_at",
            "duration_ms",
            "duration",
            "steps",
        ],
        "web.install",
    );
    assert_key_set(
        &v["data"]["config"],
        &[
            "main_path",
            "main_content",
            "include_path",
            "include_content",
            "sites",
            "params",
        ],
        "web.config",
    );
    assert_key_set(
        &v["data"]["config"]["params"],
        &[
            "worker_processes",
            "worker_connections",
            "keepalive_timeout",
            "client_max_body_size",
            "gzip",
            "gzip_types",
        ],
        "web.config.params",
    );
    assert_key_set(
        &v["data"]["modules"],
        &[
            "available",
            "reason",
            "version",
            "configure_args",
            "modules",
        ],
        "web.modules",
    );

    let (status, _, body) = get_ui(&h.app, &files_uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_key_set(
        &v["data"],
        &[
            "path",
            "parent",
            "roots",
            "entries",
            "truncated",
            "max_edit_bytes",
            "upload_chunk_bytes",
            "can_admin",
        ],
        "GET /ui/v1/files",
    );
    let entry = v["data"]["entries"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一个目录项");
    assert_key_set(
        entry,
        &[
            "name",
            "path",
            "dir",
            "symlink",
            "size",
            "mtime",
            "mode",
            "owner",
            "nlink",
            "link_target",
        ],
        "files.entry",
    );
}

/// 路由表由服务端下发：菜单投影 + 详情/子视图隐藏路由，壳脚本只消费不拼装。
///
/// 这条用例替代原先「脚本里必须有 `buildRoutes` / `BUILTIN_PAGES`」的断言：
/// 契约从「脚本怎么拼」变成「服务端出什么」，改实现不再需要动用例。
#[tokio::test]
async fn ui_routes_project_menu_with_hidden_details() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/routes", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("routes 必须是 JSON");
    assert_key_set(&v["data"], &["routes", "plugins"], "GET /ui/v1/routes");
    let routes = v["data"]["routes"].as_array().expect("routes 必须是数组");
    // 递归收集：L2 子项挂在分组的 `children` 下，隐藏路由在顶层。
    fn collect<'a>(routes: &'a [serde_json::Value], out: &mut Vec<&'a serde_json::Value>) {
        for r in routes {
            out.push(r);
            if let Some(children) = r["children"].as_array() {
                collect(children, out);
            }
        }
    }
    let mut all = Vec::new();
    collect(routes, &mut all);
    let paths: Vec<&str> = all.iter().filter_map(|r| r["path"].as_str()).collect();
    // 菜单投影：一级分组路径带入口前缀，子项是绝对路径。
    assert!(paths.contains(&"/p/__menu/sites"), "{paths:?}");
    assert!(paths.contains(&"/p/sites/list"), "{paths:?}");
    // 详情与子视图隐藏路由：地址栏直达与行内跳转都靠它们。
    for expected in [
        "/p/sites/:id",
        "/p/sites/:id/logs",
        "/p/docker/containers/:id/terminal",
        "/p/jobs/:id/logs",
    ] {
        assert!(
            paths.contains(&expected),
            "缺少隐藏路由 {expected}: {paths:?}"
        );
    }
    for r in all {
        assert!(
            r["title"].as_str().is_some_and(|t| !t.is_empty()),
            "每条路由都要有标题: {r}"
        );
        let path = r["path"].as_str().unwrap_or_default();
        if path.contains(":id") {
            assert_eq!(r["hidden"], true, "参数路由不得进侧栏: {r}");
            assert!(
                r["meta"]["activeMenu"].as_str().is_some(),
                "隐藏路由必须声明侧栏高亮落点: {r}"
            );
        }
    }
}

/// 页面取数地址：前端只报「当前路径 + 查询串」，端点由服务端回答。
#[tokio::test]
async fn ui_page_answers_where_the_schema_lives() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let cases = [
        (
            "/ui/v1/page?path=%2Fsites%2Flist",
            "/ui/v1/sites?limit=1&with_schema=true",
        ),
        ("/ui/v1/page?path=", "/ui/v1/overview?with_schema=true"),
        (
            "/ui/v1/page?path=%2Fsites%2Fweb%2Flogs&search=type%3Derror",
            "/ui/v1/web?view=logs&with_schema=true&load=1&kind=error&lines=200&level=all",
        ),
        (
            "/ui/v1/page?path=%2Fdocker%2Fimages",
            "/p/static/budding/pages/docker-images.json",
        ),
    ];
    for (uri, expected) in cases {
        let (status, _, body) = get_ui(&h.app, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["data"]["schema_url"], expected, "{uri}: {body}");
    }
    // 离不开浏览器的三种形态给标记，不给地址。
    let (status, _, body) = get_ui(
        &h.app,
        "/ui/v1/page?path=%2Fdocker%2Fcontainers%2Fc1%2Fterminal",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["special"], "terminal");
    assert!(v["data"]["schema_url"].is_null(), "{body}");
}
