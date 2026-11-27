//! `/ui/v1` 契约面：会话引导、菜单投影、壳页深链回落、概览、审计游标、路由表与页面定位、响应 key-set 冻结。

use crate::common::*;

/// 任务审计一级：计划任务 / 任务中心 / 审计日志同组；工具分组不再重复挂任务入口。
/// viewer 无 `AuditRead`，只能看到前两项，但一级分组仍然保留。
#[tokio::test]
async fn task_audit_group_collects_tasks_jobs_and_audit() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("menu 必须是 JSON");
    let groups = v["data"]["groups"].as_array().expect("groups 必须是数组");
    let tasks = groups
        .iter()
        .find(|g| g["key"] == "tasks")
        .expect("任务审计一级必须存在");
    assert_eq!(tasks["label"], "任务审计", "{body}");
    let hrefs: Vec<&str> = tasks["items"]
        .as_array()
        .expect("任务审计必须有子项")
        .iter()
        .filter_map(|i| i["href"].as_str())
        .collect();
    assert_eq!(
        hrefs,
        vec!["/p/tasks/scheduled", "/p/tasks/jobs", "/p/tasks/audit"],
        "{body}"
    );

    let tools = groups
        .iter()
        .find(|g| g["key"] == "tools")
        .expect("工具一级仍应存在");
    let tool_hrefs: Vec<&str> = tools["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["href"].as_str())
        .collect();
    assert!(!tool_hrefs.contains(&"/p/tasks/scheduled"), "{body}");
    assert!(!tool_hrefs.contains(&"/p/tasks/jobs"), "{body}");

    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("menu 必须是 JSON");
    let groups = v["data"]["groups"].as_array().expect("groups 必须是数组");
    let tasks = groups
        .iter()
        .find(|g| g["key"] == "tasks")
        .expect("viewer 仍能看到任务审计一级");
    let hrefs: Vec<&str> = tasks["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|i| i["href"].as_str())
        .collect();
    assert_eq!(
        hrefs,
        vec!["/p/tasks/scheduled", "/p/tasks/jobs"],
        "viewer 不得看到审计日志"
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
        "/p/tasks/jobs/1/logs/deep",
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
/// `/p/login` 只注册了 GET；POST 到同一路径必须 405，而不是被当成壳页回落。
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

    // 写请求不回落：`/p/login` 只注册 GET，POST 必须是 405。
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/p/login")
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

// ---- budding 概览页：入口根壳页 + /ui/v1/overview 只读计数 ----

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

/// 审计规范页交 budding 壳；数据接口对无 `AuditRead` 的角色仍是 403。
#[tokio::test]
async fn audit_canonical_path_serves_native_shell() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;
    let viewer = login(&h.app, "viewer", "pw-test-123").await;
    for token in [&admin, &viewer] {
        let (status, html) = get_html(&h.app, "/p/tasks/audit", token).await;
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
    // 另一分类的记录：用来验证分类过滤与游标指纹。
    h.state
        .db()
        .audit(
            "admin",
            "10.8.8.8",
            "docker.containers.start",
            "docker-needle",
            "ok",
            "seed",
        )
        .unwrap();

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

    // 分类过滤：站点分类下仍是 6 条，Docker 分类下该关键字为 0。
    let (status, body) = get_page(&h.app, "/ui/v1/audit?q=audit-needle&cat=site", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["items"].as_array().unwrap().len(), 6, "{body}");
    assert_eq!(v["data"]["category"], "site", "{body}");
    let (status, body) = get_page(&h.app, "/ui/v1/audit?q=audit-needle&cat=docker", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"]["items"].as_array().unwrap().is_empty(), "{body}");
    let (status, body) = get_page(&h.app, "/ui/v1/audit?q=docker-needle&cat=docker", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let item = &v["data"]["items"][0];
    assert_eq!(item["category_label"], "容器与镜像", "{body}");
    // 未知分类按「全部分类」处理，不报错。
    let (status, body) = get_page(&h.app, "/ui/v1/audit?cat=nope", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");

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

    // 换分类沿用旧游标：同样因筛选指纹不匹配被拒。
    let (status, body) = get_page(
        &h.app,
        &format!("/ui/v1/audit?q=audit-needle&cat=site&limit=1&cursor={cursor}"),
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
        &["items", "keyword", "category", "has_more", "next_cursor"],
        "GET /ui/v1/audit",
    );
    let row = v["data"]["items"]
        .as_array()
        .and_then(|a| a.first())
        .expect("应有一条审计记录");
    assert_key_set(
        row,
        &[
            "id",
            "ts",
            "ts_label",
            "category",
            "category_label",
            "actor",
            "ip",
            "action",
            "target",
            "result",
            "detail",
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
        &[
            "registry_mirror",
            "image_prefix",
            "runtime_index_url",
            "can_admin",
        ],
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
    assert_key_set(
        &v["data"],
        &["routes", "plugins", "subviews"],
        "GET /ui/v1/routes",
    );
    // 三级子视图登记表随路由表下发（2026-11-24 SUBVIEWS 双源合一：JS 侧硬编码已删）。
    let subviews = v["data"]["subviews"]
        .as_array()
        .expect("subviews 必须是数组");
    assert!(!subviews.is_empty(), "内置三级子视图登记表不能为空");
    for sv in subviews {
        assert!(
            sv["pattern"].is_array(),
            "subviews 条目必须有 pattern：{sv}"
        );
        assert!(sv["tabs"].is_array(), "subviews 条目必须有 tabs：{sv}");
    }
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
        "/p/tasks/jobs/:id/logs",
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
