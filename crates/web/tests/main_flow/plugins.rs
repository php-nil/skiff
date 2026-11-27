//! 插件域：商店安装 / 内核升级 / 包确认与兼容门 / 信任与卸载 / L3 页签 / 插件页壳与 Schema / 插件中心写操作。

use crate::common::*;

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
            r#"{"v":1,"initialized":true,"hidden":["plugin/com.example.retired/legacy"],"pinned":["plugin/com.example.retired/legacy"],"order":{"sites":["plugin/com.example.retired/legacy"]}}"#,
        )
        .unwrap();
    skiff_app::menu::cleanup_plugin_prefs(h.state.db().as_ref(), "com.example.retired");
    let raw = h
        .state
        .db()
        .get_user_pref(admin.id, "menu.prefs")
        .unwrap()
        .unwrap();
    assert!(
        !raw.contains("com.example.retired"),
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
    let manifest = include_str!("../../../../plugins/testkit/manifest.toml").to_string();
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
/// 又没登记时会直接红。
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
            include_str!("../../../../plugins/guard/manifest.toml"),
        ),
        (
            "monitor",
            include_str!("../../../../plugins/monitor/manifest.toml"),
        ),
        (
            "cron",
            include_str!("../../../../plugins/cron/manifest.toml"),
        ),
        ("ftp", include_str!("../../../../plugins/ftp/manifest.toml")),
        (
            "firewall",
            include_str!("../../../../plugins/firewall/manifest.toml"),
        ),
        (
            "sources",
            include_str!("../../../../plugins/sources/manifest.toml"),
        ),
        (
            "testkit",
            include_str!("../../../../plugins/testkit/manifest.toml"),
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
        if !m.resource_hints.iter().any(usable) {
            assert!(
                NO_DEFAULT_GRANT.iter().any(|(id, _)| *id == m.id),
                "{} 必须声明可用的默认授权建议，或在 NO_DEFAULT_GRANT 里显式列名并说明原因",
                m.id
            );
        }
    }

    let cron =
        skiff_pkgfmt::Manifest::parse(include_str!("../../../../plugins/cron/manifest.toml"))
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

/// 商店安装只认 JSON POST（带 CSRF）：GET 同一路径必须是 405，不能被当成页面。
#[tokio::test]
async fn store_install_get_returns_method_not_allowed() {
    let h = harness();
    let admin = login(&h.app, "admin", "pw-test-123").await;

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
