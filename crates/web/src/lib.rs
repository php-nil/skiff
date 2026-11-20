//! Web 管理服务（核心①）：axum 路由 + 中间件 + budding 壳页 + `/ui/v1` JSON。
//!
//! 全局中间件四层（由内到外，§3.1）：`client_info`（可信代理解析）→ `load_session`（会话）
//! → `entry_guard`（安全入口）→ `CatchPanic`（最外层安全网）；非流式路由组另挂
//! `request_timeout`（长连接路由不挂）。
//! CSRF、RBAC（`policy.check`）与审计**在 handler 内**逐请求调用：它们需要资源对象
//! 与表单字段，放不进 layer。登录限流同理，只在 `handlers/auth` 路径生效。

mod asset_rules;
pub mod assets;
pub mod authz;
pub mod error;
pub mod handlers;
pub mod labels;
pub mod middleware;
mod templates;
pub mod util;

// 上下文（`AppState` / `WebConfig`）住在 `skiff-app`（装配层），这里不设路径别名：
// 处理器直接写 `skiff_app::AppState`，层归属在调用点可见。这三项仍对外再导出，
// 集成测试按 `skiff_web::{AppState, WebConfig}` 取用，不必多引一个 crate。
pub use error::AppError;
pub use skiff_app::{AppState, CurrentUser, WebConfig};

use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::catch_panic::CatchPanicLayer;

/// 组装路由。`/healthz` 与 `/favicon.ico` 不受安全入口前缀约束（探针与浏览器默认探测），
/// 其余页面、API 与静态资源一律要求前缀命中，否则 404（§6.3）。
pub fn router(state: Arc<AppState>) -> Router {
    // 错误页在没有 AppState 的路径上也要能给出「返回首页」，前缀在这里注入一次
    templates::set_entry_prefix(&state.cfg.entry_prefix);
    let p = &state.cfg.entry_prefix;
    // 改前缀防锁死：重启后的 24 小时内，旧入口 GET 请求 302 到新前缀同名路径。
    // 只注册 GET，不复制认证/写操作，避免旧入口成为长期旁路。
    let legacy = match state.cfg.entry_prefix_previous.as_deref() {
        Some(prev) if !prev.is_empty() => Router::new()
            .route(
                &format!("/{prev}"),
                get(handlers::settings::legacy_prefix_root),
            )
            .route(
                &format!("/{prev}/{{*rest}}"),
                get(handlers::settings::legacy_prefix_redirect),
            ),
        _ => Router::new(),
    };
    // 长连接路由单独成组：不挂超时（见下方 `.layer(request_timeout)` 的位置）。
    let streaming = Router::new()
        .route(
            &format!("/{p}/containers/{{id}}/logs/stream"),
            get(handlers::containers::logs_sse),
        )
        .route(
            &format!("/{p}/containers/{{id}}/exec"),
            get(handlers::containers::exec_ws),
        )
        .route(
            &format!("/{p}/jobs/{{id}}/logs/stream"),
            get(handlers::jobs::logs_sse),
        );

    // ---- 内核「文件管理」：管理员侧 fs 服务 + 页面（第一步浏览/编辑）----
    // 单独成组挂形态门禁：受限形态（`SKIFF_FORM=docker`）能力面只有 docker/identity，
    // 没有 fs，页面与 API 直达一律 403（入口由 `skiff_app::menu::declarations` 按形态隐藏）。
    let files = Router::new()
        .route(&format!("/{p}/files"), get(handlers::files::page))
        // 页面内 JSON 接口与页面路径同构，不额外插 `api` 段（URL 契约 §4）。
        .route(&format!("/{p}/files/list"), get(handlers::files::api_list))
        .route(&format!("/{p}/files/read"), get(handlers::files::api_read))
        .route(
            &format!("/{p}/files/write"),
            post(handlers::files::api_write),
        )
        .route(
            &format!("/{p}/files/mkdir"),
            post(handlers::files::api_mkdir),
        )
        .route(
            &format!("/{p}/files/rename"),
            post(handlers::files::api_rename),
        )
        .route(
            &format!("/{p}/files/remove"),
            post(handlers::files::api_remove),
        )
        .route(&format!("/{p}/files/tree"), get(handlers::files::api_tree))
        .route(
            &format!("/{p}/files/upload"),
            post(handlers::files::api_upload),
        )
        .route(
            &format!("/{p}/files/download"),
            get(handlers::files::api_download),
        )
        .route(&format!("/{p}/files/copy"), post(handlers::files::api_copy))
        .route(
            &format!("/{p}/files/chmod"),
            post(handlers::files::api_chmod),
        )
        .route(
            &format!("/{p}/files/chown"),
            post(handlers::files::api_chown),
        )
        .route(
            &format!("/{p}/files/archive"),
            post(handlers::files::api_archive),
        )
        .route(
            &format!("/{p}/files/extract"),
            post(handlers::files::api_extract),
        )
        // ---- 回收站与配额（内核文件管理第二步）----
        .route(
            &format!("/{p}/files/trash"),
            get(handlers::files::trash_shell),
        )
        .route(
            &format!("/{p}/files/quotas"),
            get(handlers::files::quotas_shell),
        )
        .route(
            &format!("/{p}/files/trash/list"),
            get(handlers::files::api_trash_list),
        )
        .route(
            &format!("/{p}/files/trash/restore"),
            post(handlers::files::api_trash_restore),
        )
        .route(
            &format!("/{p}/files/trash/purge"),
            post(handlers::files::api_trash_purge),
        )
        .route(
            &format!("/{p}/files/trash/clear"),
            post(handlers::files::api_trash_clear),
        )
        .route(
            &format!("/{p}/files/quotas/list"),
            get(handlers::files::api_quota_list),
        )
        .route(
            &format!("/{p}/files/quotas/set"),
            post(handlers::files::api_quota_set),
        )
        .route(
            &format!("/{p}/files/quotas/cancel"),
            post(handlers::files::api_quota_cancel),
        )
        .route(
            &format!("/{p}/files/quotas/scan"),
            post(handlers::files::api_quota_scan),
        )
        // 兼容旧前缀 `/{p}/files/api/*`：308 保留方法与请求体，语义不变（URL 契约 §7）。
        .route(
            &format!("/{p}/files/api/{{*rest}}"),
            axum::routing::any(handlers::files::legacy_api_redirect),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::fs_capability,
        ));

    // ---- budding 前端 JSON 命名空间（/ui/v1）：无入口前缀、未认证 401、只返 JSON ----
    // 鉴权与语义口径见 URL 契约 §2/§6；路由清单按资源逐步登记（本批只有会话引导与登出）。
    let ui_json = handlers::ui::router();

    let ui = Router::new()
        .route(&format!("/{p}"), get(handlers::spa::root_shell))
        // 尾斜杠是常见手误（浏览器补全、复制粘贴），不该给个 404
        .route(&format!("/{p}/"), get(handlers::dashboard::root_redirect))
        // 登录只交付 budding 登录壳；提交走 `POST /ui/v1/auth/login`（无表单回退）。
        .route(&format!("/{p}/login"), get(handlers::auth::login_page))
        // WS 只认票据（§6.2）：票据先走这条 HTTP 路由换取，成功后 30 秒内一次性使用
        .route(
            &format!("/{p}/containers/{{id}}/exec/ticket"),
            post(handlers::containers::exec_ticket),
        )
        .route(
            &format!("/{p}/plugins/store/confirm"),
            get(handlers::spa::root_shell),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/schema/{{view}}"),
            get(handlers::plugins::schema),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/{{view}}"),
            get(handlers::plugins::view),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/rpc"),
            post(handlers::plugins::rpc),
        )
        // 静态资源只认带安全入口前缀的写法，根路径不再兜底，避免入口收紧后残留旁路。
        .route(&format!("/{p}/static/{{*path}}"), get(assets::static_file))
        .route(
            &format!("/{p}/plugins/{{id}}/ui/{{*path}}"),
            get(handlers::plugins::plugin_ui),
        )
        // ---- JSON API v1（与 UI 共用同一套 RBAC 与审计）----
        .route("/api/v1/session", post(handlers::api::login))
        .route("/api/v1/docker/containers", get(handlers::api::containers))
        .route(
            "/api/v1/docker/containers/{id}",
            get(handlers::api::container),
        )
        .route(
            "/api/v1/docker/containers/{id}/restart",
            post(handlers::api::restart_container),
        )
        .route("/api/v1/plugins", get(handlers::api::plugins))
        .route("/api/v1/plugins/ui", get(handlers::api::plugin_ui_index))
        // 文件管理分组（自带形态门禁）也在超时层覆盖范围内。
        .merge(files)
        .merge(ui_json)
        // 命中路径但方法不匹配时同样走壳页判定（GET 导航可能是 POST-only 路由的形态）。
        // 必须在 `.layer(...)` 之前：它会替换各 MethodRouter 的默认 405 兜底，
        // 之后再套层才会把会话/超时中间件包在兜底上。
        .method_not_allowed_fallback(handlers::spa::method_not_allowed)
        // SPA 壳页回落（URL 契约 §2.1）：必须在 `.layer(...)` 之前设置，
        // 这样壳页同样经过会话加载（未登录 303）与安全响应头。
        .fallback(handlers::spa::fallback)
        // 到这里的所有路由都是一次请求一次响应，套超时。
        // 必须在 `.merge(streaming)` **之前**：`Router::layer` 只作用于已加入的路由，
        // SSE（日志）与 WS（终端）因此天然被排除在外。
        .layer(axum::middleware::from_fn(middleware::request_timeout))
        .merge(streaming)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::client_info,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::load_session,
        ));

    // CatchPanic 是最外层安全网（core/src/main.rs 抬头承诺的行为）：panic=unwind 下
    // handler panic 若不拦，会直接吞掉请求——无 500、无审计。这里统一转错误页 + 审计。
    Router::new()
        .route("/healthz", get(handlers::meta::healthz))
        // 浏览器默认会探测根路径 favicon；显式返回图标，避免全站 404。
        .route("/favicon.ico", get(assets::favicon))
        .merge(ui)
        .merge(legacy)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::entry_guard,
        ))
        .layer(CatchPanicLayer::custom(panic_response(state.clone())))
        .with_state(state)
}

/// CatchPanic 的响应构造：错误页 500 + `web.panic` 审计 + tracing。
/// 独立成函数是为了能直接单测（见 `tests::catch_panic_*`）。
fn panic_response(
    state: Arc<AppState>,
) -> impl FnMut(Box<dyn std::any::Any + Send>) -> axum::response::Response + Clone {
    move |payload| {
        let msg = payload
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload")
            .to_string();
        tracing::error!(error = %msg, "handler panic，已由 CatchPanic 捕获");
        // 审计写失败只能记日志：panic 已经发生，不能再二次 panic。
        if let Err(e) = state
            .db()
            .audit("system", "-", "web.panic", "-", "error", &msg)
        {
            tracing::error!(error = %e, "panic 审计写入失败");
        }
        crate::error::AppError::Internal("服务器内部错误".into()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use skiff_docker::{DockerService, Form, Policy, Tier};
    use skiff_plugin::{Supervisor, SupervisorConfig};
    use std::path::PathBuf;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        let db = Arc::new(skiff_store::Db::open_in_memory().unwrap());
        let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
        let sup = Arc::new(Supervisor::new(
            SupervisorConfig::new(
                PathBuf::from("skiff-test-data"),
                PathBuf::from("skiff-test-run"),
            ),
            db.clone(),
        ));
        let cfg = skiff_app::WebConfig {
            data_dir: PathBuf::from("skiff-test-data"),
            web_root: PathBuf::from("skiff-test-web"),
            trash_retention_days: 30,
            config_path: PathBuf::from("skiff-test-config.toml"),
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
            tier: Tier::Basic,
            form: Form::Native,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            policy: Policy::new(Tier::Basic, Form::Native, vec![]).unwrap(),
            session: skiff_identity::SessionLimits::default(),
            login_concurrency: 1,
            ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
            ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
            catalog: Default::default(),
        };
        AppState::new(db, docker, sup, cfg, "csrf-test".into(), "test")
    }

    /// 安全响应头由 `entry_guard` 统一注入，必须出现在每一个响应上（含探针）。
    #[tokio::test]
    async fn security_headers_on_every_response() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let h = resp.headers();
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(h.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(h.get("referrer-policy").unwrap(), "no-referrer");
        let csp = h
            .get("content-security-policy")
            .expect("应注入 CSP")
            .to_str()
            .unwrap();
        assert!(csp.starts_with("default-src 'self'; script-src 'self'"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("img-src 'self' data:"));
    }

    /// 插件 UI 页面由 handler 显式设 `SAMEORIGIN` 以便被面板 iframe 嵌入，
    /// `entry_guard` 不能把它覆盖回 `DENY`，否则插件功能页在浏览器里整页被拦。
    #[tokio::test]
    async fn entry_guard_keeps_handler_x_frame_options() {
        async fn framed() -> axum::response::Response {
            let mut resp = "ok".into_response();
            resp.headers_mut().insert(
                axum::http::header::X_FRAME_OPTIONS,
                axum::http::HeaderValue::from_static("SAMEORIGIN"),
            );
            resp
        }
        let app =
            Router::new()
                .route("/p/x", get(framed))
                .layer(axum::middleware::from_fn_with_state(
                    test_state(),
                    middleware::entry_guard,
                ));
        let resp = app
            .oneshot(Request::builder().uri("/p/x").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.headers().get("x-frame-options").unwrap(), "SAMEORIGIN");
    }

    /// 非流式路由挂超时：慢 handler 必须变成 504 错误页，而不是一直挂着连接。
    #[tokio::test]
    async fn slow_handler_times_out_with_504() {
        async fn slow() -> axum::response::Response {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            "late".into_response()
        }
        async fn fast() -> &'static str {
            "ok"
        }
        let app = Router::new()
            .route("/slow", get(slow))
            .route("/fast", get(fast))
            .layer(axum::middleware::from_fn(|req, next| async move {
                middleware::timeout_after(std::time::Duration::from_millis(30), req, next).await
            }));

        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("请求超时"),
            "应渲染错误页而不是空白 408，实际: {text}"
        );

        let resp = app
            .oneshot(Request::builder().uri("/fast").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "快请求不应被超时影响");
    }

    #[tokio::test]
    async fn catch_panic_returns_500_and_writes_audit() {
        // 显式返回类型避开 never-type fallback（`async { panic!() }` 在新编译器上是硬错误）
        async fn boom() -> axum::response::Response {
            panic!("boom-test")
        }
        let state = test_state();
        let app = Router::new()
            .route("/boom", get(boom))
            .layer(CatchPanicLayer::custom(panic_response(state.clone())));
        let resp = app
            .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let entries = state.db().recent_audit(10).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.action == "web.panic" && e.detail.contains("boom-test")),
            "审计应包含 web.panic 且带 panic 信息，实际: {:?}",
            entries
        );
    }
}
