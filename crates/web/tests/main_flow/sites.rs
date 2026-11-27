//! 站点域：站点增删改 / 默认站点 / 目录边界 / 运行环境版本与绑定 / 站点日志与写权限面。

use crate::common::*;

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

// ---- 站点 / 运行环境只读页：JSON 列表 + keyset 游标 + 壳页回落 ----

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
    // 2026-11-24 页头主操作迁移：新建/批量从卡片体内挪到 `page.content.actions`。
    assert_eq!(v["schema"]["body"][0]["type"], "text", "首行是站点计数");
    let actions = v["schema"]["content"]["actions"]
        .as_array()
        .expect("管理员必须有页头主操作区");
    let texts: Vec<&str> = actions
        .iter()
        .filter_map(|a| a["props"]["text"].as_str())
        .collect();
    assert!(texts.contains(&"新建站点"), "页头缺新建站点：{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("批量")),
        "页头缺批量操作：{texts:?}"
    );

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

    // 版本清单默认就有内容：镜像源未配置（mirrors 为空）时用内置副本 / 目录大版本，
    // 弹窗不必等网络。三类各一份，缺一份 Node/Rust 就没有下拉可选。
    for kind in ["php", "node", "rust"] {
        let versions = data["version_indexes"][kind]["versions"]
            .as_array()
            .unwrap_or_else(|| panic!("{kind} 必须带版本清单：{v}"));
        assert!(!versions.is_empty(), "{kind} 的清单不能为空：{v}");
        assert_eq!(data["version_indexes"][kind]["pending"], false);
        assert_eq!(
            data["version_indexes"][kind]["url"], "",
            "没有镜像基址就不该拼出半截清单地址：{v}"
        );
    }
    assert_eq!(
        data["version_indexes"]["php"]["source"], "builtin",
        "PHP 有内置副本：{v}"
    );
    assert_eq!(
        data["version_indexes"]["node"]["source"], "catalog",
        "Node.js / Rust 没有内置副本，回退目录大版本：{v}"
    );

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
