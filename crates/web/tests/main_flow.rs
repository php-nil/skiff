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

struct Harness {
    app: Router,
    state: Arc<AppState>,
    _data: tempfile::TempDir,
    data_dir: PathBuf,
}

fn harness() -> Harness {
    harness_with(Form::Native)
}

/// 指定部署形态的 harness：受限形态（1Panel Docker 包）能力面收缩用它。
fn harness_with(form: Form) -> Harness {
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
        entry_prefix: "p".into(),
        dev_http: true,
        trusted_proxies: vec![],
        external_url: None,
        tier: Tier::Basic,
        form,
        allow_unsigned: false,
        allow_reduced_sandbox: true,
        policy: Policy::new(Tier::Basic, form, vec![]).unwrap(),
        session: skiff_identity::SessionLimits::default(),
        login_concurrency: 2,
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
            && html.contains(r#"href="/p/m/core/sites/settings""#),
        "网站一级由内核自带网站列表/运行环境/设置: {html}"
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
    assert!(
        !html.contains("文件管理"),
        "未启用插件不得出现任何插件菜单入口: {html}"
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
        !center_html.contains(r#"href="/p/plugins/com.skiff.files""#),
        "未启用插件不得渲染「打开」死入口: {center_html}"
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
            && filtered_html.contains("没有处于异常状态的容器")
            && filtered_html.contains("清除筛选"),
        "异常筛选深链必须可解释且可恢复: {filtered_html}"
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
        state: "running".into(),
        status: "Up 1m".into(),
        labels: Default::default(),
        project: None,
        owner: None,
        protected: false,
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

/// 插件详情页的两条 UI 约定（跨插件跳转 + 顶栏插件菜单）：
/// 1. `?open=` 只作为「定位建议」被清洗后原样透传（控制字符清掉、百分号编码）；
/// 2. `data-plugins` 白名单只含「已启用且有 UI 入口」的插件——iframe 的
///    `skiff.open` 只能往这些目标跳，跳转本身不授予任何能力。
#[tokio::test]
async fn plugin_detail_passes_open_hint_and_whitelists_enabled_plugins() {
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

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/plugins/com.example.ui?open=%2Fvar%2Fwww%2Fsite%20one")
                .header(header::COOKIE, format!("skiff_dev_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_text(resp).await;

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
    // 沙箱属性必须下发：没有它「opaque origin / 不持面板凭据」只是文案
    assert!(
        html.contains(r#"sandbox="allow-scripts allow-modals""#),
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
                .uri("/p/plugins/com.example.ui?open=%2Fetc%2F%0Apasswd")
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

    // 恢复默认：隐藏与布局一起回到 manifest/管理员默认
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
        reset.contains(r#"data-layout="top""#)
            && reset.contains(r#"href="/p/m/core/docker/images""#),
        "恢复默认必须同时复原布局与隐藏项: {reset}"
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

/// P2：L3 标签栏由内核渲染，三个标签共用同一个 iframe（`menu` + `tab` 参数）。
#[tokio::test]
async fn l3_tabs_render_share_iframe_and_deep_link() {
    let h = harness();
    let manifest = r#"id = "com.example.tabs"
name = "Tabs Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.1"
requires_full_sandbox = false
[[ui]]
label = "演示"
path = "index.html"
[[menu]]
id = "sites"
parent = "sites"
label = "站点管理"
order = 10
icon = "globe"
roles = ["operator", "admin"]
target = { kind = "plugin_ui", ui = "index.html" }
[[menu.items]]
id = "list"
label = "站点列表"
order = 10
icon = "list"
ui = "index.html"
tab = "sites"
[[menu.items]]
id = "backup"
label = "备份"
order = 20
icon = "archive"
ui = "index.html"
tab = "backup"
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
    assert!(html.contains("data-workbench"), "应渲染内核 L3 标签栏");
    assert!(html.contains(r#"data-local="list""#) && html.contains(r#"data-local="backup""#));
    assert!(html.contains("tab=sites"), "默认取第一个 L3 的 tab");
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

    // P1 旧深链 `legacy` 升级到声明式后落到首个可见 L2，不破坏旧链接。
    let (status, html) = get_page(&h.app, &format!("{base}/legacy"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-menu="plugin/com.example.tabs/sites""#),
        "{html}"
    );
}

/// P2：`overview.health` 槽位成功渲染；插件停用/角色不可见时回落内核健康块。
#[tokio::test]
async fn overview_health_mount_renders_and_falls_back() {
    let h = harness();
    let manifest = r#"id = "com.example.health"
name = "Health Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000000"
api = "1.1"
requires_full_sandbox = false
[[ui]]
label = "主机监控"
path = "index.html"
[[menu]]
id = "monitor"
parent = "tools"
label = "主机监控"
order = 30
icon = "activity"
target = { kind = "plugin_ui", ui = "index.html" }
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

/// 网站列表：默认站点标记与切换入口由内核渲染，不要求环境已安装。
#[tokio::test]
async fn sites_list_shows_default_badge_and_toggle() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    h.state
        .db()
        .create_site(
            "demo.local",
            &h.data_dir
                .join("www/sites/demo.local")
                .display()
                .to_string(),
            "8.5",
        )
        .unwrap();
    h.state.db().set_site_default("demo.local", true).unwrap();

    let (status, html) = get_page(&h.app, "/p/sites", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("demo.local"), "{html}");
    assert!(html.contains("默认</span>"), "默认标记必须出现");
    assert!(
        html.contains("/p/sites/demo.local/default"),
        "切换入口必须出现"
    );
    assert!(html.contains("取消默认"), "已是默认时按钮文案应为取消默认");
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
        .create_site("demo.local", &root.display().to_string(), "8.5")
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
        .create_site("demo.local", &root.display().to_string(), "8.5")
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
        .create_site("demo.local", &root.display().to_string(), "8.5")
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
            "guard",
            include_str!("../../../plugins/guard/ui/index.html"),
            include_str!("../../../plugins/guard/ui/app.js"),
        ),
        (
            "cron",
            include_str!("../../../plugins/cron/ui/index.html"),
            include_str!("../../../plugins/cron/ui/app.js"),
        ),
        (
            "monitor",
            include_str!("../../../plugins/monitor/ui/index.html"),
            include_str!("../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-health",
            include_str!("../../../plugins/monitor/ui/health.html"),
            include_str!("../../../plugins/monitor/ui/health.js"),
        ),
        (
            "files",
            include_str!("../../../plugins/files/ui/index.html"),
            include_str!("../../../plugins/files/ui/app.js"),
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
    }
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
        css.contains("table.grid.sites-grid th.col-actions { width: 252px; }"),
        "操作列必须给出固定宽度，站点列吃掉剩余空间"
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

/// 网站创建向导：分步结构、未实现项置灰标注、服务端非法域名就地报错。
#[tokio::test]
async fn sites_wizard_validates_and_marks_unimplemented() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

    let (status, html) = get_page(&h.app, "/p/sites/new", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-wizard"), "创建页必须是分步向导: {html}");
    assert!(html.contains(r#"data-wizard-step="3""#), "必须有确认步骤");
    assert!(html.contains("未实现"), "数据库/SSL 必须明确标注未实现");
    assert!(
        !html.contains("type=\"checkbox\""),
        "不得用可点击的假开关冒充数据库/SSL"
    );
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
                    ("domain", "bad_domain"),
                    ("aliases", ""),
                    ("root_dir", "/www/sites/bad"),
                    ("php_version", "8.5"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "校验失败应留在向导页");
    let html = body_text(resp).await;
    assert!(
        html.contains("域名只能包含") && html.contains("data-wizard"),
        "非法域名必须就地报错并保留向导: {html}"
    );
}

/// 批量操作必须要求管理员；viewer 直接 403，不产生任何副作用。
#[tokio::test]
async fn sites_batch_requires_admin() {
    let h = harness();
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    h.state
        .db()
        .create_site("demo.local", "/www/sites/demo.local", "8.5")
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

/// 配置保存失败（本机无 Docker，reload 失败）必须回滚旧内容并给出提示。
#[tokio::test]
async fn site_config_save_failure_rolls_back() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let root = h.data_dir.join("www/sites/demo.local");
    h.state
        .db()
        .create_site("demo.local", &root.display().to_string(), "8.5")
        .unwrap();
    std::fs::create_dir_all(skiff_app::sites::site_conf_dir(&root)).unwrap();
    let conf = skiff_app::sites::site_conf_dir(&root).join("demo.local.conf");
    let original = "server { listen 80; server_name demo.local; }";
    std::fs::write(&conf, original).unwrap();

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
                    ("content", "server { listen 80; server_name broken.local; }"),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "失败应跳回配置页");
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let (status, html) = get_page(&h.app, &loc, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("已回滚"), "必须给回滚提示: {html}");
    assert!(
        html.contains(original),
        "页面必须展示回滚后的旧配置: {html}"
    );
    assert_eq!(
        std::fs::read_to_string(&conf).unwrap(),
        original,
        "磁盘内容必须回滚"
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
        .create_site("demo.local", &root.display().to_string(), "8.5")
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
        .create_site("demo.local", &root.display().to_string(), "8.5")
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
    let html = include_str!("../../../plugins/files/ui/index.html");
    let js = include_str!("../../../plugins/files/ui/app.js");

    // 自包含：不得出现任何 scheme 形式的绝对地址（包内一律相对路径）。
    for (name, src) in [("index.html", html), ("app.js", js)] {
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
}
