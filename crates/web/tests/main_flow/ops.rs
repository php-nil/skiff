//! 运维域：容器与资源 / Compose / 文件管理（含回收站、配额）/ 任务与作业 / 设置与主题 / Web 服务。

use crate::common::*;

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
    // C12 删掉 `handlers::files::page` 后这些路径只剩 SPA `fallback` 承接，
    // 而 fallback 只对 `Accept: text/html` 回壳页（浏览器一定带，裸 GET 走 404）。
    for uri in ["/p/files", "/p/files/trash", "/p/files/quotas"] {
        let (status, html) = get_html(&h.app, uri, &token).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
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
    let uri = format!("/p/tasks/jobs/{}/logs", job.id);

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

/// 安装进度只允许一个事实源：任务中心 `web.install` 作业。
///
/// 内存版 `site_install_progress` 必须彻底消失，否则页面会出现两套口径。
#[test]
fn web_install_progress_has_single_persisted_source() {
    let sources = [
        ("web handler", include_str!("../../src/handlers/ui/web.rs")),
        ("nginx handler", include_str!("../../src/handlers/nginx.rs")),
        ("app state", include_str!("../../../app/src/lib.rs")),
        (
            "sites crud",
            concat!(
                include_str!("../../../app/src/sites/crud/mod.rs"),
                include_str!("../../../app/src/sites/crud/vhost.rs"),
                include_str!("../../../app/src/sites/crud/tls.rs"),
            ),
        ),
        (
            "sites runtime",
            concat!(
                include_str!("../../../app/src/sites/runtime/mod.rs"),
                include_str!("../../../app/src/sites/runtime/jobs.rs"),
                include_str!("../../../app/src/sites/runtime/php.rs"),
            ),
        ),
    ];
    for (name, src) in sources {
        assert!(
            !src.contains("site_install_progress") && !src.contains("site_env.lock"),
            "{name} 不应再出现内存安装进度"
        );
    }
    let state = include_str!("../../../app/src/lib.rs");
    assert!(
        state.contains("pub fn web_install_progress") && state.contains("TARGET_WEB_ENV"),
        "安装进度必须从任务中心最新 web.install 作业构建"
    );
    let crud = concat!(
        include_str!("../../../app/src/sites/crud/mod.rs"),
        include_str!("../../../app/src/sites/crud/vhost.rs"),
        include_str!("../../../app/src/sites/crud/tls.rs"),
    );
    assert!(
        crud.contains("KIND_WEB_INSTALL") && crud.contains("set_steps"),
        "安装步骤必须写入作业快照"
    );
    let store = include_str!("../../../store/src/db/jobs.rs");
    assert!(
        store.contains("set_job_steps") && store.contains("fail_job_interrupted"),
        "store 必须提供步骤持久化与重启中断收尾"
    );
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
    // 回收站 / 配额页面不再有独立 SSR handler：与主文件页一样走壳页回落，
    // 已认证用户统一拿到壳页；权限改在 `/ui/v1` 数据端点判定。
    // 回落只对 `Accept: text/html` 生效（浏览器语义），所以这里用 `get_html`。
    for uri in ["/p/files/trash", "/p/files/quotas"] {
        let (status, html) = get_html(&h.app, uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(html.contains(r#"id="skiff-root""#), "{uri}: {html}");
        let (status, _) = get_html(&h.app, uri, &viewer).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{uri} 壳页对已认证用户统一交付，权限在数据层判定"
        );
    }
    // 数据端点对 viewer 一律 403：回收站读 / 配额读都只对管理员开放。
    for uri in ["/ui/v1/files/trash/list", "/ui/v1/files/quotas/list"] {
        let (status, _, body) = get_ui(&h.app, uri, Some(&viewer), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} viewer 必须 403: {body}"
        );
    }
}

/// 形态门禁：`SKIFF_FORM=docker`（1Panel 受限形态）能力面收缩为 docker/identity，
/// `/ui/v1/files/*` 一律 403（管理员也不例外），但其它 `/ui/v1` 资源不受影响。
#[tokio::test]
async fn files_routes_are_forbidden_in_docker_restricted_form() {
    let h = harness_full(Form::DockerRestricted, Tier::Basic);
    let admin = login(&h.app, "admin", "pw-test-123").await;
    // 文件管理读端点：管理员也被形态门禁拒绝（先于 RBAC 与业务）。
    for uri in [
        "/ui/v1/files",
        "/ui/v1/files/trash/list",
        "/ui/v1/files/quotas/list",
        "/ui/v1/files/download",
    ] {
        let (status, _, body) = get_ui(&h.app, uri, Some(&admin), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} 受限形态必须 403: {body}"
        );
    }
    // 其它能力面（docker/identity）端点不受文件门禁影响。
    let (status, _, body) = get_ui(&h.app, "/ui/v1/menu", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "菜单不应被文件门禁拒绝: {body}");
    // 菜单入口隐藏：受限形态下「工具 → 文件管理 / 回收站 / 配额」不下发，
    // 与数据端点 403 同源（menu.rs 按 `form != DockerRestricted` 过滤）。
    let menu: serde_json::Value = serde_json::from_str(&body).expect("menu 必须是 JSON");
    let hrefs: Vec<&str> = menu["data"]["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| g["items"].as_array())
        .flatten()
        .filter_map(|i| i["href"].as_str())
        .collect();
    assert!(
        !hrefs.iter().any(|h| h.starts_with("/p/files")),
        "受限形态菜单不应含文件管理入口: {body}"
    );
    // 页面层不挂形态门禁（与全站壳页一致）：受限形态下直达 `/p/files` 壳页仍 200，
    // 数据由 `/ui/v1/files*` 拒绝——C12 收口后的统一行为（壳页不鉴权，数据层判定）。
    let (status, _, body) = get_ui(&h.app, "/p/files", Some(&admin), Some("text/html")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "受限形态下壳页仍应渲染，权限在数据层判定: {body}"
    );
    assert!(body.contains(r#"id="skiff-root""#));
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
    // 内核运行目录（`data_dir` 及其子树，含 `web_root`）对文件管理是保护路径，
    // 配额写入必须挑一个普通目录：用独立临时目录，别落在 `data_dir` 下。
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("www");
    std::fs::create_dir_all(&root).unwrap();

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("/ui/v1/files/trash/restore", json!({ "id": "t-missing" })),
        ("/ui/v1/files/trash/purge", json!({ "id": "t-missing" })),
        ("/ui/v1/files/trash/clear", json!({})),
        (
            "/ui/v1/files/quotas/set",
            json!({ "path": root.display().to_string(), "max_bytes": 1024 }),
        ),
        (
            "/ui/v1/files/quotas/cancel",
            json!({ "path": root.display().to_string() }),
        ),
        (
            "/ui/v1/files/quotas/scan",
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

    // 缺省主色随会话下发，壳页首屏即按全站主题渲染（缺省 = 预设「深潜」）。
    let (_, _, body) = get_ui(&h.app, "/ui/v1/session", Some(&viewer), None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["theme"]["primary"], "#17a98e", "{body}");
    assert_eq!(v["data"]["theme"]["menu"], "#0c2422", "{body}");
    assert_eq!(v["data"]["theme"]["menu_custom"], true, "{body}");

    let (status, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&viewer), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["data"]["primary"], "#17a98e", "{body}");
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
    assert!(body.contains("#17a98e"), "{body}");

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

    // 空串恢复缺省主色。菜单开关一并钉回缺省（「深潜」的深墨绿导航），否则会按
    // 自动推导出的白色菜单做对比度校正，「恢复缺省」测出来的就不是缺省值了。
    let (status, text) = post_ui_json(
        &h.app,
        "/ui/v1/settings/ui",
        &admin,
        Some(&csrf),
        &json!({ "primary": "", "menu_custom": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (_, _, body) = get_ui(&h.app, "/ui/v1/settings/ui", Some(&admin), None).await;
    assert!(body.contains(r##""primary":"#17a98e""##), "{body}");
    assert!(body.contains(r##""menu":"#0c2422""##), "{body}");
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
        "/p/tasks/scheduled",
        "/p/tasks/scheduled/builtin:db-vacuum",
        "/p/tasks/jobs",
        "/p/tasks/jobs/abc",
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
