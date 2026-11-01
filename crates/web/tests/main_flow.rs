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
use skiff_store::{Db, Role};
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

fn set_cookie_value(resp: &axum::response::Response, name: &str) -> Option<String> {
    for v in resp.headers().get_all(header::SET_COOKIE) {
        let v = v.to_str().ok()?;
        if let Some(rest) = v.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
            return Some(rest.split(';').next().unwrap_or_default().to_string());
        }
    }
    None
}

fn form_body(pairs: &[(&str, &str)]) -> Body {
    let s = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    Body::from(s)
}

/// 登录并返回会话令牌（Cookie 值）。
async fn login(app: &Router, username: &str, password: &str) -> String {
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
    let login_csrf = set_cookie_value(&page, "skiff_login").expect("登录页应下发 CSRF cookie");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_login={login_csrf}"))
                .body(form_body(&[
                    ("username", username),
                    ("password", password),
                    ("csrf", &login_csrf),
                    ("otp", ""),
                    ("next", ""),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
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

#[tokio::test]
async fn wrong_password_renders_error_and_audits() {
    let h = harness();
    let page = h
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
    let login_csrf = set_cookie_value(&page, "skiff_login").unwrap();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("skiff_login={login_csrf}"))
                .body(form_body(&[
                    ("username", "admin"),
                    ("password", "wrong-pw"),
                    ("csrf", &login_csrf),
                    ("otp", ""),
                    ("next", ""),
                ]))
                .unwrap(),
        )
        .await
        .unwrap();
    let text = body_text(resp).await;
    assert!(text.contains("不正确"), "错误页应提示口令不正确: {text}");
    let audit = h.state.db.recent_audit(10).unwrap();
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
    h.state.db.trust_key(&kid, &pubkey, "测试作者").unwrap();

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
        .db
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
    let audit = h.state.db.recent_audit(20).unwrap();
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
        .db
        .user_by_name("admin")
        .unwrap()
        .expect("harness 应建好 admin")
        .id;

    // 建两个 token：只有 plugins:read 的、以及 scope 写错的
    let read_only = "tok-plugins-only";
    h.state
        .db
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
        .db
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
        .db
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
    h.state.db.trust_key(&kid, &pubkey, "测试作者").unwrap();

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
        h.state.db.plugin("com.example.demo").unwrap().is_none(),
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
    h.state.db.trust_key(&kid2, &pubkey2, "测试作者").unwrap();
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
