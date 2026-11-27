//! 共享夹具与请求辅助。

//!

//! 全部项对子模块公开（`pub`）并统一经 `use crate::common::*;` 取用；

//! `include_str!` 的相对路径比原单文件深一层（`tests/main_flow/`）。

pub use std::path::PathBuf;

pub use std::sync::atomic::{AtomicUsize, Ordering};

pub use std::sync::Arc;

pub use axum::body::Body;

pub use axum::http::{header, Request, StatusCode};

pub use axum::Router;

pub use skiff_docker::{DockerService, Form, Policy, Tier};

pub use skiff_plugin::{Supervisor, SupervisorConfig};

pub use skiff_store::{Db, PluginRecord, Role};

pub use skiff_web::{AppState, WebConfig};

pub use tower::ServiceExt;

pub use serde_json::json;

/// 测试用 PHP 站点：绑定 v7 迁移种入的 php-8.5 运行环境。
pub fn php_site(domain: &str, root: &str) -> skiff_store::NewSite {
    skiff_store::NewSite {
        domain: domain.into(),
        root_dir: root.into(),
        site_type: skiff_store::TYPE_RUNTIME.into(),
        runtime_id: Some("php-8.5".into()),
        proxy_target: None,
        proxy_websocket: false,
    }
}

pub fn static_site(domain: &str, root: &str) -> skiff_store::NewSite {
    skiff_store::NewSite {
        domain: domain.into(),
        root_dir: root.into(),
        site_type: skiff_store::TYPE_STATIC.into(),
        runtime_id: None,
        proxy_target: None,
        proxy_websocket: false,
    }
}

pub struct Harness {
    pub app: Router,
    pub state: Arc<AppState>,
    pub _data: tempfile::TempDir,
    pub data_dir: PathBuf,
}

pub fn harness() -> Harness {
    harness_full(Form::Native, Tier::Basic)
}

/// 生命周期档 harness：创建容器 / Compose 等 `Tier::Lifecycle` 动词只在宿主机直装形态可用。
pub fn harness_lifecycle() -> Harness {
    harness_full(Form::Native, Tier::Lifecycle)
}

pub fn harness_full(form: Form, tier: Tier) -> Harness {
    harness_full_with_catalog(form, tier, Default::default())
}

/// 指定商店目录配置的 harness：商店安装入口的路由与审计测试用。
pub fn harness_full_with_catalog(
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
pub fn seed_plugin(db: &Db, id: &str, name: &str, label: &str, enabled: bool) {
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
pub fn seed_declared_plugin(db: &Db, manifest: String, enabled: bool) {
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

pub fn set_cookie_value(resp: &axum::response::Response, name: &str) -> Option<String> {
    for v in resp.headers().get_all(header::SET_COOKIE) {
        let v = v.to_str().ok()?;
        if let Some(rest) = v.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return Some(rest.split(';').next().unwrap_or_default().to_string());
        }
    }
    None
}

/// 整条 `Set-Cookie` 头（含属性），用于断言 Cookie 形态。
pub fn set_cookie_header(resp: &axum::response::Response, name: &str) -> Option<String> {
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
pub async fn login_page_tokens(app: &Router) -> (String, String) {
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
pub async fn post_login(
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
pub async fn login(app: &Router, username: &str, password: &str) -> String {
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
pub fn build_package_api(root: &std::path::Path, api: &str) -> (PathBuf, String, String) {
    build_package_full(root, "docker", "docker", api)
}

pub fn build_package_full(
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
pub fn stage_pkg(data_dir: &std::path::Path, pkg: &std::path::Path) -> String {
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

pub async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

/// 带会话 GET，返回（状态码，整页 HTML）。
pub async fn get_page(app: &Router, uri: &str, token: &str) -> (StatusCode, String) {
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
pub async fn get_html(app: &Router, uri: &str, token: &str) -> (StatusCode, String) {
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

/// 带可选会话与 `Accept` 的 GET，返回（状态码，Content-Type，响应体）。
pub async fn get_ui(
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

/// 提交 budding 登录 JSON；`site` 模拟浏览器的 `Sec-Fetch-Site`。
pub async fn post_ui_login(
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

/// 带会话与可选 CSRF 的 JSON POST，返回（状态码，响应体）。
pub async fn post_ui_json(
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

/// 原始 GET（需自读响应头时用；`get_ui` 只返回状态码/类型/正文）。
pub async fn get_raw(
    app: &Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut req = Request::builder().uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

pub fn header_str(resp: &axum::response::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// HTML 属性里以 `attr="..."` 形式出现的值（壳页引用解析用）。
pub fn attr_values(html: &str, attr: &str) -> Vec<String> {
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

/// 取 JSON 对象的键（非对象直接失败）。
pub fn object_keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("期望 JSON 对象，实际: {v}"))
        .keys()
        .cloned()
        .collect()
}

/// 断言对象键集与冻结清单完全一致（顺序无关）。
pub fn assert_key_set(v: &serde_json::Value, expected: &[&str], ctx: &str) {
    let mut actual = object_keys(v);
    actual.sort();
    let mut expected: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
    expected.sort();
    assert_eq!(
        actual, expected,
        "{ctx} 字段集漂移：新增可选字段请更新冻结清单；删除/改名/改语义必须升 api_version 并写进 SDUI 集成文档"
    );
}
