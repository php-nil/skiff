//! Web 管理服务（核心①）：axum 路由 + 中间件 + SSR + SSE/WS。
//!
//! 全局中间件四层（由内到外，§3.1）：`client_info`（可信代理解析）→ `load_session`（会话）
//! → `entry_guard`（安全入口）→ `CatchPanic`（最外层安全网）；非流式路由组另挂
//! `request_timeout`（长连接路由不挂）。
//! CSRF、RBAC（`policy.check`）与审计**在 handler 内**逐请求调用：它们需要资源对象
//! 与表单字段，放不进 layer。登录限流同理，只在 `handlers/auth` 路径生效。

pub mod assets;
pub mod authz;
pub mod error;
pub mod handlers;
pub mod labels;
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
            &format!("/{p}/containers/{{id}}/logs"),
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
    // 没有 fs，页面与 API 直达一律 403（入口由 `menu::declarations` 按形态隐藏）。
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
            get(handlers::files::trash_page),
        )
        .route(
            &format!("/{p}/files/quotas"),
            get(handlers::files::quotas_page),
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
    let ui_json = Router::new()
        .route(
            "/ui/v1/auth/login",
            get(handlers::ui_api::login_config).post(handlers::ui_api::login_submit),
        )
        .route("/ui/v1/session", get(handlers::ui_api::session))
        .route("/ui/v1/menu", get(handlers::ui_api::menu))
        .route("/ui/v1/overview", get(handlers::ui_api::overview))
        .route("/ui/v1/sites", get(handlers::ui_api::sites))
        .route("/ui/v1/sites/{domain}", get(handlers::ui_api::site_detail))
        .route("/ui/v1/runtimes", get(handlers::ui_api::runtimes))
        .route("/ui/v1/images", get(handlers::ui_api::images))
        .route("/ui/v1/networks", get(handlers::ui_api::networks))
        .route("/ui/v1/volumes", get(handlers::ui_api::volumes))
        .route("/ui/v1/compose", get(handlers::ui_api::compose))
        .route("/ui/v1/compose/down", post(handlers::ui_api::compose_down))
        .route(
            "/ui/v1/compose/{project}/{action}",
            post(handlers::ui_api::compose_project_action),
        )
        .route("/ui/v1/tasks", get(handlers::ui_api::tasks))
        .route("/ui/v1/tasks/{id}", get(handlers::ui_api::task_detail))
        .route("/ui/v1/jobs", get(handlers::ui_api::jobs))
        .route("/ui/v1/jobs/{id}", get(handlers::ui_api::job_detail))
        .route("/ui/v1/settings", get(handlers::ui_api::settings))
        .route("/ui/v1/audit", get(handlers::ui_api::audit))
        .route(
            "/ui/v1/settings/panel",
            post(handlers::ui_api::settings_panel_update),
        )
        .route(
            "/ui/v1/settings/password",
            post(handlers::ui_api::settings_password_update),
        )
        .route(
            "/ui/v1/settings/totp/setup",
            post(handlers::ui_api::settings_totp_setup),
        )
        .route(
            "/ui/v1/settings/totp/enable",
            post(handlers::ui_api::settings_totp_enable),
        )
        .route(
            "/ui/v1/settings/keys",
            post(handlers::ui_api::settings_key_import),
        )
        .route(
            "/ui/v1/settings/keys/{id}",
            post(handlers::ui_api::settings_key_remove),
        )
        .route("/ui/v1/files", get(handlers::ui_api::files))
        .route("/ui/v1/files/read", get(handlers::ui_api::files_read))
        .route(
            "/ui/v1/sites/{domain}/logs",
            get(handlers::ui_api::site_logs),
        )
        .route("/ui/v1/files/mkdir", post(handlers::ui_api::files_mkdir))
        .route("/ui/v1/files/rename", post(handlers::ui_api::files_rename))
        .route("/ui/v1/files/remove", post(handlers::ui_api::files_remove))
        .route("/ui/v1/store", get(handlers::ui_api::store))
        .route(
            "/ui/v1/store/plugins/{id}/install",
            post(handlers::ui_api::store_install),
        )
        .route(
            "/ui/v1/plugins/confirm",
            post(handlers::ui_api::plugin_confirm),
        )
        .route("/ui/v1/plugins", get(handlers::ui_api::plugins))
        .route("/ui/v1/plugins/{id}", get(handlers::ui_api::plugin_info))
        .route(
            "/ui/v1/plugins/{id}/toggle",
            post(handlers::ui_api::plugin_toggle),
        )
        .route(
            "/ui/v1/plugins/{id}/delete",
            post(handlers::ui_api::plugin_delete),
        )
        .route("/ui/v1/containers", get(handlers::ui_api::containers))
        .route(
            "/ui/v1/containers/{id}",
            get(handlers::ui_api::container_detail),
        )
        .route(
            "/ui/v1/containers/{id}/{action}",
            post(handlers::ui_api::container_action),
        )
        .route("/ui/v1/session/logout", post(handlers::ui_api::logout));

    let ui = Router::new()
        .route(&format!("/{p}"), get(handlers::spa::root_shell))
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
        // 容器列表与详情已迁 budding（壳页回落，旧 SSR 在 `/{p}/m/core/docker/containers*`）。
        .route(
            &format!("/{p}/containers/rows"),
            get(handlers::containers::rows),
        )
        .route(
            &format!("/{p}/containers/new"),
            get(handlers::containers::new_page),
        )
        .route(
            &format!("/{p}/containers/create"),
            post(handlers::containers::create),
        )
        .route(
            &format!("/{p}/containers/{{id}}/limits"),
            post(handlers::containers::update_limits),
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
        // 镜像/网络/卷列表已迁 budding（旧 SSR 在 `/{p}/m/core/docker/*`）。
        .route(
            &format!("/{p}/images/pull"),
            post(handlers::resources::pull_image),
        )
        .route(
            &format!("/{p}/images/{{id}}/delete"),
            post(handlers::resources::remove_image),
        )
        .route(
            &format!("/{p}/images/{{id}}/remove"),
            post(handlers::resources::remove_image),
        )
        .route(
            &format!("/{p}/networks/create"),
            post(handlers::resources::create_network),
        )
        .route(
            &format!("/{p}/networks/{{id}}/delete"),
            post(handlers::resources::remove_network),
        )
        .route(
            &format!("/{p}/networks/{{id}}/remove"),
            post(handlers::resources::remove_network),
        )
        .route(
            &format!("/{p}/volumes/create"),
            post(handlers::resources::create_volume),
        )
        .route(
            &format!("/{p}/volumes/{{name}}/delete"),
            post(handlers::resources::remove_volume),
        )
        .route(
            &format!("/{p}/volumes/{{name}}/remove"),
            post(handlers::resources::remove_volume),
        )
        // 编排页已迁 budding（旧 SSR 在 `/{p}/m/core/docker/compose`）。
        .route(
            &format!("/{p}/compose/validate"),
            post(handlers::compose::validate),
        )
        .route(&format!("/{p}/compose/up"), post(handlers::compose::up))
        .route(&format!("/{p}/compose/down"), post(handlers::compose::down))
        .route(
            &format!("/{p}/compose/{{project}}/{{action}}"),
            post(handlers::compose::project_action),
        )
        .route(
            &format!("/{p}/settings/docker"),
            get(handlers::docker_settings::page).post(handlers::docker_settings::save),
        )
        // 旧位置兼容：Docker 设置属于「设置」资源，规范路径为 `/{p}/settings/docker`。
        .route(
            &format!("/{p}/docker/settings"),
            axum::routing::any(handlers::docker_settings::legacy_path),
        )
        // 插件中心与信息页已进入 budding 适配：`GET /{p}/plugins`、`/{p}/plugins/{id}`
        // 回落到壳页；旧 SSR 保留在 `/{p}/m/core/plugins/{installed|discover|updates}`
        // 与 `/{p}/m/core/plugins/info/{id}`。功能页 `/{p}/plugins/{id}/{view}` 不变。
        .route(
            &format!("/{p}/store/plugins/{{id}}/install"),
            post(handlers::store::install).get(handlers::store::install_legacy_get),
        )
        .route(
            &format!("/{p}/store/upgrades/{{version}}/download"),
            get(handlers::store::download_upgrade),
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
            &format!("/{p}/plugins/{{id}}/delete"),
            post(handlers::plugins::remove),
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
        // 设置页已进入 budding 适配：规范页回落到壳页，旧 SSR（口令 / TOTP / 公钥 /
        // 完整面板表单）保留在 `/{p}/m/core/settings/security`；写操作仍走旧表单。
        // ---- 内核定时任务：viewer 只读，写操作 Admin + CSRF ----
        // 任务与作业列表/详情已迁 budding（旧 SSR 在 `/{p}/m/core/{tasks|jobs}*`）。
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
        // ---- 任务中心：一次性异步作业（安装/拉镜像/Compose 等）----
        .route(
            &format!("/{p}/jobs/{{id}}/cancel"),
            post(handlers::jobs::cancel),
        )
        .route(
            &format!("/{p}/jobs/{{id}}/retry"),
            post(handlers::jobs::retry),
        )
        .route(
            &format!("/{p}/jobs/{{id}}/logs"),
            get(handlers::jobs::logs_json),
        )
        // ---- 内核「文件管理」路由在 `files` 分组统一登记并挂形态门禁 ----
        // ---- 内核「网站」：站点列表 / 运行环境 / 设置 ----
        // 列表与详情已迁 budding（壳页回落，旧 SSR 留在 `/{p}/m/core/sites/*`）；
        // 写操作路由本批保持原样，等前端表单覆盖后再逐步切换。
        .route(
            &format!("/{p}/sites/new"),
            get(handlers::sites::new_page_get),
        )
        .route(&format!("/{p}/sites/batch"), post(handlers::sites::batch))
        .route(&format!("/{p}/sites/create"), post(handlers::sites::create))
        .route(
            &format!("/{p}/sites/env"),
            get(handlers::sites::legacy_env_page),
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
        .route(
            &format!("/{p}/sites/env/{{id}}/extensions-batch"),
            post(handlers::sites::runtime_extensions_batch),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/extensions/{{key}}/install"),
            post(handlers::sites::runtime_extension_install),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/extensions/{{key}}/uninstall"),
            post(handlers::sites::runtime_extension_uninstall),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/pecl"),
            post(handlers::sites::runtime_pecl_install),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/logs"),
            get(handlers::sites::runtime_logs),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/probe"),
            post(handlers::sites::runtime_probe),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/sites/{{domain}}/unbind"),
            post(handlers::sites::runtime_unbind_site),
        )
        // ---- 内核「网站 → Web 服务」：状态 / 配置 / 日志 / 模块 ----
        .route(&format!("/{p}/sites/web"), get(handlers::nginx::index))
        .route(
            &format!("/{p}/sites/web/config/main"),
            post(handlers::nginx::save_main),
        )
        .route(
            &format!("/{p}/sites/web/config/params"),
            post(handlers::nginx::save_params),
        )
        // 运行环境详情与操作；同时提供 `/{p}/runtimes/...` 规范入口。
        .route(
            &format!("/{p}/sites/env/{{id}}"),
            get(handlers::sites::legacy_env_detail),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/config"),
            post(handlers::sites::runtime_config_save),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/node-config"),
            post(handlers::sites::runtime_node_config_save),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/port"),
            post(handlers::sites::runtime_port_save),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/clone"),
            post(handlers::sites::runtime_clone),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/health"),
            get(handlers::sites::runtime_health),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/config-file"),
            post(handlers::sites::runtime_config_file_save),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/config-file"),
            post(handlers::sites::runtime_config_file_save),
        )
        .route(
            &format!("/{p}/sites/env/{{id}}/action/{{action}}"),
            post(handlers::sites::runtime_action),
        )
        // 静态段必须早于 `{id}` 动态段注册，避免把 `create` / `images` / `jobs` 当成运行环境 id。
        .route(
            &format!("/{p}/runtimes/create"),
            post(handlers::sites::env_add),
        )
        // 旧动词 `add` 保留为别名：已打开的表单仍能提交（URL 契约 §7）。
        .route(
            &format!("/{p}/runtimes/add"),
            post(handlers::sites::env_add),
        )
        .route(
            &format!("/{p}/runtimes/images"),
            post(handlers::sites::env_images_save),
        )
        .route(
            &format!("/{p}/runtimes/jobs"),
            get(handlers::sites::runtime_jobs),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/config"),
            post(handlers::sites::runtime_config_save),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/node-config"),
            post(handlers::sites::runtime_node_config_save),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/port"),
            post(handlers::sites::runtime_port_save),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/clone"),
            post(handlers::sites::runtime_clone),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/health"),
            get(handlers::sites::runtime_health),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/action/{{action}}"),
            post(handlers::sites::runtime_action),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/extensions/{{key}}/install"),
            post(handlers::sites::runtime_extension_install),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/extensions/{{key}}/uninstall"),
            post(handlers::sites::runtime_extension_uninstall),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/pecl"),
            post(handlers::sites::runtime_pecl_install),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/logs"),
            get(handlers::sites::runtime_logs),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/probe"),
            post(handlers::sites::runtime_probe),
        )
        .route(
            &format!("/{p}/runtimes/{{id}}/sites/{{domain}}/unbind"),
            post(handlers::sites::runtime_unbind_site),
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
            &format!("/{p}/settings/keys/create"),
            post(handlers::settings::import_key),
        )
        // 集合 POST 等价别名：保留旧表单与脚本的提交入口。
        .route(
            &format!("/{p}/settings/keys"),
            post(handlers::settings::import_key),
        )
        // 监听端口 / 安全入口前缀 / TLS：写 config.toml，重启生效。
        .route(
            &format!("/{p}/settings/panel"),
            post(handlers::settings::panel_update),
        )
        .route(
            &format!("/{p}/settings/keys/{{id}}/delete"),
            post(handlers::settings::remove_key),
        )
        .route(
            &format!("/{p}/settings/keys/{{id}}/remove"),
            post(handlers::settings::remove_key),
        )
        // 审计页已原生渲染（第十五批）：数据走 `/ui/v1/audit`（关键字 + 签名 keyset 游标），
        // 旧 SSR 仅保留在 `/{p}/m/core/settings/audit`（最近 200 条）。
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
        let cfg = crate::state::WebConfig {
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
