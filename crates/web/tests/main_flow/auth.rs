//! 认证与会话：登录页 / 登录提交 / Cookie 形态 / CSRF / 登出 / 会话轮换 / API Token scope / exec 票据闸门。

use crate::common::*;

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

/// 未认证的 GET 页面请求：settings 与 files 均靠壳页回落，必须一致 303 登录。
///
/// 覆盖回落与尾部斜杠入口；写请求仍不回落。
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
