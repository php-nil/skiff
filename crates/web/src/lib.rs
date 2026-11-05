//! Web 管理服务（核心①）：axum 路由 + 中间件 + SSR + SSE/WS。
//!
//! 全局中间件四层（由内到外，§3.1）：`client_info`（可信代理解析）→ `load_session`（会话）
//! → `entry_guard`（安全入口）→ `CatchPanic`（最外层安全网）；非流式路由组另挂
//! `request_timeout`（长连接路由不挂）。
//! CSRF、RBAC（`policy.check`）与审计**在 handler 内**逐请求调用：它们需要资源对象
//! 与表单字段，放不进 layer。登录限流同理，只在 `handlers/auth` 路径生效。

pub mod assets;
pub mod error;
pub mod handlers;
pub mod menu;
pub mod middleware;
mod templates;
pub mod util;

// 上下文住在 `skiff-app`（装配层）。这里保留 `crate::state` 这个路径别名，是为了让
// 处理器继续写 `crate::state::AppState` 而不必逐个改 use——层归属看 `skiff-app`。
pub use skiff_app as state;

pub use error::AppError;
pub use state::{AppState, CurrentUser, WebConfig};

use axum::extract::DefaultBodyLimit;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::catch_panic::CatchPanicLayer;

/// 组装路由。`/healthz` 与静态资源不受安全入口前缀约束（探针与首屏需要），
/// 但页面与 API 一律要求前缀命中，否则 404（§6.3）。
pub fn router(state: Arc<AppState>) -> Router {
    // 错误页在没有 AppState 的路径上也要能给出「返回首页」，前缀在这里注入一次
    templates::set_entry_prefix(&state.cfg.entry_prefix);
    let p = &state.cfg.entry_prefix;
    // 长连接路由单独成组：不挂超时（见下方 `.layer(request_timeout)` 的位置）。
    let streaming = Router::new()
        .route(
            &format!("/{p}/containers/{{id}}/logs"),
            get(handlers::containers::logs_sse),
        )
        .route(
            &format!("/{p}/containers/{{id}}/exec"),
            get(handlers::containers::exec_ws),
        );

    let ui = Router::new()
        .route(&format!("/{p}"), get(handlers::dashboard::index))
        // 尾斜杠是常见手误（浏览器补全、复制粘贴），不该给个 404
        .route(&format!("/{p}/"), get(handlers::dashboard::root_redirect))
        .route(
            &format!("/{p}/login"),
            get(handlers::auth::login_page).post(handlers::auth::login_submit),
        )
        .route(&format!("/{p}/logout"), post(handlers::auth::logout))
        .route(
            &format!("/{p}/password"),
            post(handlers::auth::change_password),
        )
        .route(&format!("/{p}/totp"), post(handlers::auth::setup_totp))
        .route(
            &format!("/{p}/totp/enable"),
            post(handlers::auth::enable_totp),
        )
        .route(&format!("/{p}/containers"), get(handlers::containers::list))
        .route(
            &format!("/{p}/containers/rows"),
            get(handlers::containers::rows),
        )
        .route(
            &format!("/{p}/containers/{{id}}"),
            get(handlers::containers::detail),
        )
        .route(
            &format!("/{p}/containers/{{id}}/{{action}}"),
            post(handlers::containers::action),
        )
        // WS 只认票据（§6.2）：票据先走这条 HTTP 路由换取，成功后 30 秒内一次性使用
        .route(
            &format!("/{p}/containers/{{id}}/exec/ticket"),
            post(handlers::containers::exec_ticket),
        )
        .route(&format!("/{p}/images"), get(handlers::resources::images))
        .route(
            &format!("/{p}/images/pull"),
            post(handlers::resources::pull_image),
        )
        .route(
            &format!("/{p}/images/{{id}}/remove"),
            post(handlers::resources::remove_image),
        )
        .route(
            &format!("/{p}/networks"),
            get(handlers::resources::networks),
        )
        .route(
            &format!("/{p}/networks/create"),
            post(handlers::resources::create_network),
        )
        .route(
            &format!("/{p}/networks/{{id}}/remove"),
            post(handlers::resources::remove_network),
        )
        .route(&format!("/{p}/volumes"), get(handlers::resources::volumes))
        .route(
            &format!("/{p}/volumes/create"),
            post(handlers::resources::create_volume),
        )
        .route(
            &format!("/{p}/volumes/{{name}}/remove"),
            post(handlers::resources::remove_volume),
        )
        .route(
            &format!("/{p}/docker/settings"),
            get(handlers::docker_settings::page).post(handlers::docker_settings::save),
        )
        .route(&format!("/{p}/plugins"), get(handlers::plugins::list))
        .route(
            &format!("/{p}/plugins/{{id}}"),
            get(handlers::plugins::info),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/{{view}}"),
            get(handlers::plugins::view),
        )
        .route(
            &format!("/{p}/plugins/install"),
            // axum 默认 body 上限 2MB，而 .wbpkg 内含插件二进制，必然超过——
            // 之前 3MB 的包会在 Multipart 解析阶段就被掐断，handler 里那条
            // `archive_bytes`（100MB）判断永远走不到。这里把框架上限抬到应用层
            // 上限之上（+64KB 留给 multipart 头），真正的判定仍由 handler 做。
            post(handlers::plugins::install).layer(DefaultBodyLimit::max(
                skiff_pkgfmt::DEFAULT_LIMITS.archive_bytes as usize + 64 * 1024,
            )),
        )
        .route(
            &format!("/{p}/plugins/confirm"),
            post(handlers::plugins::confirm),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/toggle"),
            post(handlers::plugins::toggle),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/remove"),
            post(handlers::plugins::remove),
        )
        .route(
            &format!("/{p}/plugins/{{id}}/rpc"),
            post(handlers::plugins::rpc),
        )
        .route(
            &format!("/{p}/settings/menu"),
            get(handlers::settings::menu_page).post(handlers::settings::menu_update),
        )
        .route(&format!("/{p}/settings"), get(handlers::settings::page))
        // ---- 内核定时任务：viewer 只读，写操作 Admin + CSRF ----
        .route(&format!("/{p}/tasks"), get(handlers::tasks::list))
        .route(&format!("/{p}/tasks/{{id}}"), get(handlers::tasks::detail))
        .route(
            &format!("/{p}/tasks/{{id}}/toggle"),
            post(handlers::tasks::toggle),
        )
        .route(
            &format!("/{p}/tasks/{{id}}/run"),
            post(handlers::tasks::run),
        )
        .route(
            &format!("/{p}/tasks/{{id}}/delete"),
            post(handlers::tasks::delete),
        )
        // ---- 内核「网站」：站点列表 / 运行环境 / 设置 ----
        .route(&format!("/{p}/sites"), get(handlers::sites::list))
        .route(
            &format!("/{p}/sites/new"),
            get(handlers::sites::new_page_get),
        )
        .route(&format!("/{p}/sites/batch"), post(handlers::sites::batch))
        .route(&format!("/{p}/sites/create"), post(handlers::sites::create))
        .route(&format!("/{p}/sites/env"), get(handlers::sites::env_page))
        .route(
            &format!("/{p}/sites/env/install"),
            post(handlers::sites::env_install),
        )
        .route(
            &format!("/{p}/sites/env/add"),
            post(handlers::sites::env_add),
        )
        .route(
            &format!("/{p}/sites/env/images"),
            post(handlers::sites::env_images_save),
        )
        // 轮询端点必须早于 `{id}` 动态段注册，避免把 `jobs` 当成运行环境 id。
        .route(
            &format!("/{p}/sites/env/jobs"),
            get(handlers::sites::runtime_jobs),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/job"),
            get(handlers::sites::runtime_job),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/extensions"),
            post(handlers::sites::runtime_extensions_save),
        )
        // ---- 内核「网站 → Nginx」：状态 / 配置 / 日志 / 模块 ----
        .route(&format!("/{p}/sites/nginx"), get(handlers::nginx::index))
        .route(
            &format!("/{p}/sites/nginx/config/main"),
            post(handlers::nginx::save_main),
        )
        .route(
            &format!("/{p}/sites/nginx/config/params"),
            post(handlers::nginx::save_params),
        )
        // 运行环境详情与操作；同时提供 `/{p}/runtimes/...` 规范入口。
        .route(
            &format!("/{p}/sites/env/{{id}}"),
            get(handlers::sites::runtime_detail),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/config"),
            post(handlers::sites::runtime_config_save),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/action/{{action}}"),
            post(handlers::sites::runtime_action),
        )
        .route(&format!("/{p}/runtimes"), get(handlers::sites::env_page))
        .route(
            &format!("/{p}/runtimes/{{id}}"),
            get(handlers::sites::runtime_detail),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/config"),
            post(handlers::sites::runtime_config_save),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/action/{{action}}"),
            post(handlers::sites::runtime_action),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/job"),
            get(handlers::sites::runtime_job),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/extensions"),
            post(handlers::sites::runtime_extensions_save),
        )
        .route(
            &format!("/{p}/sites/{{domain}}"),
            get(handlers::sites::detail),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/logs"),
            get(handlers::sites::logs),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/config/save"),
            post(handlers::sites::save_config),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/delete"),
            get(handlers::sites::delete_confirm).post(handlers::sites::delete),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/default"),
            post(handlers::sites::set_default),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/domains"),
            post(handlers::sites::domains),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/ssl"),
            post(handlers::sites::ssl_action),
        )
        // 站点备份开关：直接开关内核 `site-backup` 任务（不跳计划任务页）。
        // 必须注册在 `{{action}}` 通配之前，否则会被当成未知站点动作。
        .route(
            &format!("/{p}/sites/{{domain}}/backup"),
            post(handlers::sites::set_backup),
        )
        .route(
            &format!("/{p}/sites/{{domain}}/{{action}}"),
            post(handlers::sites::action),
        )
        .route(
            &format!("/{p}/settings/keys"),
            post(handlers::settings::import_key),
        )
        .route(
            &format!("/{p}/settings/keys/{{id}}/remove"),
            post(handlers::settings::remove_key),
        )
        .route(&format!("/{p}/audit"), get(handlers::settings::audit))
        // 契约里的规范深链：旧路由继续可用，这里只是内部转发到同一 handler。
        .route(&format!("/{p}/m/{{*rest}}"), get(handlers::menu::dispatch))
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
        .merge(ui)
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
        let _ = state
            .db()
            .audit("system", "-", "web.panic", "-", "error", &msg);
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
        let cfg = crate::state::WebConfig {
            data_dir: PathBuf::from("skiff-test-data"),
            web_root: PathBuf::from("skiff-test-web"),
            entry_prefix: "p".into(),
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
