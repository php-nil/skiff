//! 运行环境：环境页与镜像参数、旧入口重定向，以及全部 `runtime_*` 安装/配置/日志动作。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::request::Parts;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use skiff_app::actions as act;
use skiff_app::commands::runtime as runtime_cmd;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser};
use crate::state::AppState;
use crate::templates::{
    BoundSiteView, ExtensionOptionView, NodeConfigView, PhpConfigView, PhpEditorView, PhpFileView,
    PhpIniValueView, RuntimeDetailPage, RuntimeJobView, RuntimeKindView, RuntimeRow, SiteEnvPage,
};
use crate::util::{encode_uri_component, form_first, parse_form};

use super::*;

/// 旧的运行环境页面入口：`/{p}/sites/env` → `/{p}/runtimes`（301，规范化 URL）。
///
/// 运行环境是独立资源，不属于站点；`sites/env` 只作为兼容别名保留，
/// 旧 POST 动作路由仍指向同一 handler，避免已打开页面提交失败。
pub async fn legacy_env_page(State(state): State<Arc<AppState>>, uri: Uri) -> Response {
    let mut target = format!("/{}/runtimes", state.cfg.entry_prefix);
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(q);
    }
    (StatusCode::MOVED_PERMANENTLY, [(LOCATION, target)]).into_response()
}

/// 旧运行环境详情入口：`/{p}/sites/env/{id}` → `/{p}/runtimes/{id}`（301）。
pub async fn legacy_env_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    uri: Uri,
) -> Response {
    let mut target = format!("/{}/runtimes/{}", state.cfg.entry_prefix, id);
    if let Some(q) = uri.query() {
        target.push('?');
        target.push_str(q);
    }
    (StatusCode::MOVED_PERMANENTLY, [(LOCATION, target)]).into_response()
}

fn runtime_kind_views() -> Vec<RuntimeKindView> {
    skiff_app::runtime_catalog::RUNTIME_KINDS
        .iter()
        .map(|k| RuntimeKindView {
            key: k.kind.to_string(),
            label: k.label.to_string(),
            versions: k.versions.iter().map(|v| v.to_string()).collect(),
            implemented: skiff_app::runtime_catalog::is_implemented(k.kind),
        })
        .collect()
}

fn extension_options(enabled: &[String], version: &str, image: &str) -> Vec<ExtensionOptionView> {
    let base = skiff_app::runtime_catalog::base_image_of(image);
    let mut out: Vec<ExtensionOptionView> =
        skiff_app::runtime_catalog::extensions_for(version, base)
            .into_iter()
            .map(|entry| {
                let e = entry.def;
                // 不可用项已在 `extensions_for` 过滤，展示层不再灰显/占位；
                // 这里只保留依赖提示（Ok → 空）。
                let support_note = match &entry.support {
                    skiff_app::runtime_catalog::ExtSupport::NeedsDeps(deps) => {
                        format!("安装时将临时安装 {} 并在完成后清理", deps.join(" / "))
                    }
                    _ => String::new(),
                };
                ExtensionOptionView {
                    key: e.key.to_string(),
                    label: e.label.to_string(),
                    description: e.description.to_string(),
                    // 镜像内置项始终视为已启用（无需再编译）。
                    enabled: e.builtin || enabled.iter().any(|k| k == e.key),
                    default_on: e.default_on,
                    builtin: e.builtin,
                    category: e.category.to_string(),
                    source: e.source.label().to_string(),
                    hot: e.hot,
                    heavy: skiff_app::runtime_catalog::extension_is_heavy(e.key),
                    estimated_time: skiff_app::runtime_catalog::extension_estimated_time(e.key)
                        .to_string(),
                    support_note,
                }
            })
            .collect();
    // 运行环境里按名字装过的 PECL 扩展也要出现在清单里（否则无法卸载）。
    for token in enabled {
        if skiff_app::runtime_catalog::php_extension(token).is_none()
            && skiff_app::runtime_catalog::parse_pecl_token(token).is_some()
        {
            out.push(ExtensionOptionView {
                key: token.clone(),
                label: skiff_app::runtime_catalog::extension_label(token),
                description: "通过 PECL 按名字安装".to_string(),
                enabled: true,
                default_on: false,
                builtin: false,
                category: skiff_app::runtime_catalog::CATEGORY_OTHER.to_string(),
                source: skiff_app::runtime_catalog::ExtSource::Pecl
                    .label()
                    .to_string(),
                hot: true,
                heavy: skiff_app::runtime_catalog::extension_is_heavy(token),
                estimated_time: skiff_app::runtime_catalog::extension_estimated_time(token)
                    .to_string(),
                support_note: String::new(),
            });
        }
    }
    out
}

pub async fn env_page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let env = state.site_env_status().await;
    let views = state.runtime_views().await.unwrap_or_default();
    // Web 服务在「Web 服务」页单独管理；种子行只在已安装或被站点引用时出现。
    let runtimes: Vec<RuntimeRow> = views
        .iter()
        .filter(|v| v.is_listed())
        .map(|v| runtime_row(&state, v))
        .collect();
    let prefix = &state.cfg.entry_prefix;
    let kinds = runtime_kind_views();
    // 新建弹窗切换 PHP 版本时，扩展勾选项要按该版本重算可用性；
    // 数据与抽屉共用 `extension_options`（`runtime_catalog` 单一来源）。
    let mut exts_by_version = serde_json::Map::new();
    if let Some(def) = skiff_app::runtime_catalog::RUNTIME_KINDS
        .iter()
        .find(|k| k.kind == "php")
    {
        for ver in def.versions {
            let opts = extension_options(
                &skiff_app::runtime_catalog::default_extensions(),
                ver,
                &env.php_image,
            );
            let arr: Vec<serde_json::Value> = opts
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "key": e.key,
                        "label": e.label,
                        "builtin": e.builtin,
                        "default_on": e.default_on,
                        "heavy": e.heavy,
                        "estimated_time": e.estimated_time,
                        "note": e.support_note,
                    })
                })
                .collect();
            exts_by_version.insert(ver.to_string(), serde_json::Value::Array(arr));
        }
    }
    let exts_json = serde_json::Value::Object(exts_by_version);
    let catalog_json = serde_json::to_string(
        &kinds
            .iter()
            .map(|k| {
                let mut v = serde_json::json!({
                    "key": k.key,
                    "label": k.label,
                    "versions": k.versions,
                    "implemented": k.implemented,
                });
                if k.key == "php" {
                    v["exts_by_version"] = exts_json.clone();
                }
                v
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    handlers::render(SiteEnvPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        env: env_view(&env),
        runtimes,
        can_admin: user.can(skiff_model::Permission::Admin),
        kinds,
        catalog_json,
        extensions: extension_options(
            &skiff_app::runtime_catalog::default_extensions(),
            skiff_app::sites::DEFAULT_PHP_VERSION,
            &env.php_image,
        ),
        jobs_href: format!("/{prefix}/runtimes/jobs"),
        add_href: format!("/{prefix}/runtimes/create"),
        images_href: format!("/{prefix}/runtimes/images"),
        error: String::new(),
        notice: String::new(),
    })
}

pub async fn env_add(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_CREATE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let kind = form_first(&form, "kind");
    let name = form_first(&form, "name");
    let version = form_first(&form, "version");
    // 弹窗会带 `ext_form=1`（即使一个都没勾）；没有该标记的旧表单/无 JS 场景
    // 回落到目录默认扩展，避免「没提交 ext」被误判成「全部取消」。
    let selected: Vec<String> = if form.contains_key("ext_form") {
        form.get("ext").cloned().unwrap_or_default()
    } else {
        skiff_app::runtime_catalog::default_extensions()
    };
    let prefix = &state.cfg.entry_prefix;
    let created = runtime_cmd::create(
        &state,
        runtime_cmd::CreateRuntime {
            kind,
            name,
            version,
            extensions: selected,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await;
    match created {
        Ok(created) => {
            let rt = &created.runtime;
            let message = if created.started {
                format!(
                    "已提交 {} {} 安装任务（实例 {}）；列表会实时显示进度，打开详情抽屉可看安装日志",
                    skiff_app::runtime_catalog::kind_label(&rt.kind),
                    rt.version,
                    rt.name
                )
            } else {
                format!(
                    "已登记 {} {}（实例 {}）；P2 预留类型暂不安装容器",
                    skiff_app::runtime_catalog::kind_label(&rt.kind),
                    rt.version,
                    rt.name
                )
            };
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                message,
                format!("/{prefix}/runtimes"),
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes"),
        )),
    }
}

/// 运行环境页「镜像参数」保存：nginx / PHP 基础镜像标签。
/// 镜像前缀与 Docker 加速源已迁到 Docker 设置，这里只管运行环境自身的镜像名。
pub async fn env_images_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_IMAGES)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let nginx = form
        .contains_key("nginx_image")
        .then(|| form_first(&form, "nginx_image"));
    let php = form
        .contains_key("php_image")
        .then(|| form_first(&form, "php_image"));
    runtime_cmd::save_images(
        &state,
        runtime_cmd::SaveImages {
            nginx_image: nginx,
            php_image: php,
            actor: actor(&state, &user, &parts),
        },
    )
    .map_err(map_err)?;
    let prefix = &state.cfg.entry_prefix;
    Ok(handlers::flash_redirect(
        &state,
        &user,
        skiff_app::FlashKind::Notice,
        "镜像参数已保存，重建 Web 服务或重装对应实例后生效",
        format!("/{prefix}/runtimes"),
    ))
}

pub async fn runtime_detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let view = state.runtime_view(&id).await.map_err(map_err)?;
    let prefix = &state.cfg.entry_prefix;
    let config = PhpConfigView::from(&skiff_app::sites::PhpRuntimeConfig::from_json(
        &view.runtime.config_json,
    ));
    let bound_site_rows: Vec<Site> = state
        .db()
        .list_sites()?
        .into_iter()
        .filter(|s| s.runtime_id.as_deref() == Some(id.as_str()))
        .collect();
    let bound_sites = bound_site_rows
        .iter()
        .map(|s| BoundSiteView {
            href: format!("/{prefix}/sites/{}", encode_uri_component(&s.domain)),
            domain: s.domain.clone(),
            type_label: type_label(&s.site_type).to_string(),
            status_label: runtime_site_status(&s.status, Some(view.effective_state.as_str()))
                .0
                .to_string(),
        })
        .collect();
    let (logs, logs_error) = if q.get("tab").map(String::as_str) == Some("logs") {
        match state.runtime_logs(&id, 300).await {
            Ok(text) => (text, String::new()),
            Err(e) => (String::new(), format!("{e}")),
        }
    } else {
        (String::new(), String::new())
    };
    let enabled_exts = view.extensions.clone();
    let is_php = view.runtime.kind == skiff_store::KIND_PHP;
    let node = if view.runtime.kind == skiff_store::KIND_NODE {
        let s = state.site_settings();
        // 一个环境 = 一个应用进程 = 一个固定代码目录；绑定站点不改变工作目录。
        let workdir = s.container_path(
            &s.runtime_home(&view.runtime.kind, &view.runtime.name)
                .join("app"),
        );
        let cfg = skiff_app::sites::NodeRuntimeConfig::from_json(&view.runtime.config_json);
        let env_text = cfg.env_text();
        NodeConfigView {
            command: cfg.command,
            health_path: cfg.health_path,
            node_env: cfg.node_env,
            env_text,
            workdir,
            port: view.runtime.port.map(|p| p.to_string()).unwrap_or_default(),
            config_url: format!(
                "/{prefix}/runtimes/{}/node-config",
                encode_uri_component(&id)
            ),
        }
    } else {
        NodeConfigView::default()
    };
    let mut page_error = String::new();
    let mut php_files = Vec::new();
    let mut php_editor = None;
    let mut php_values = Vec::new();
    let mut php_values_loaded = false;
    let mut php_values_error = String::new();
    if is_php {
        php_files = state
            .php_config_files(&id)
            .map_err(map_err)?
            .into_iter()
            .map(|f| {
                let selected = q.get("file").map(String::as_str) == Some(f.key.as_str());
                PhpFileView {
                    label: f.label,
                    host_path: f.host_path,
                    container_path: f.container_path,
                    exists: f.exists,
                    selected,
                    edit_href: format!(
                        "/{prefix}/runtimes/{}?tab=config&file={}",
                        encode_uri_component(&id),
                        encode_uri_component(&f.key)
                    ),
                }
            })
            .collect::<Vec<_>>();
        if let Some(key) = q.get("file").filter(|v| !v.is_empty()) {
            match state.read_php_config_file(&id, key, &user.user.username) {
                Ok(v) => {
                    php_editor = Some(PhpEditorView {
                        key: v.file.key,
                        label: v.file.label,
                        content: v.content,
                        container_path: v.file.container_path,
                        note: v.file.note,
                    });
                }
                Err(e) => page_error = format!("{e}"),
            }
        }
        if q.get("ini").map(String::as_str) == Some("1") {
            php_values_loaded = true;
            match state.php_effective_values(&id).await {
                Ok(values) => {
                    php_values = values
                        .into_iter()
                        .map(|v| PhpIniValueView {
                            key: v.key,
                            file_value: v.file_value,
                            effective: v.effective,
                            source: v.source,
                        })
                        .collect();
                }
                Err(e) => php_values_error = format!("{e}"),
            }
        }
    }
    let job = state.runtime_job(&id).await;
    let (has_job, job_view) = match job.as_ref() {
        Some(j) => (true, job_view(j)),
        None => (false, RuntimeJobView::default()),
    };
    // 共享环境的影响面：停用/重启/重建/改端口前先列出绑定站点，二次确认后才执行。
    // 只接受已知动作，避免 `?confirm=任意值` 渲染出可提交的确认面板。
    let pending_confirm = q
        .get("confirm")
        .map(String::as_str)
        .filter(|a| matches!(*a, "stop" | "restart" | "rebuild" | "install" | "port"))
        .unwrap_or_default()
        .to_string();
    let pending_confirm_label = action_label(&pending_confirm).to_string();
    let pending_port = q.get("port").cloned().unwrap_or_default();
    // 安装时未挂载 php/php.ini 与 conf.d 的存量容器：编辑能保存但不生效，
    // 详情页给出重建入口，别让用户对着界面猜。
    let config_mounts_stale = if is_php && view.container_exists {
        !state.php_config_mounts_current(&id).map_err(map_err)?
    } else {
        false
    };
    // 进程型环境（Node.js / Rust）：端口可改、应用健康可探测、同配置可克隆。
    let port_editable = matches!(
        view.runtime.kind.as_str(),
        skiff_store::KIND_NODE | skiff_store::KIND_RUST
    );
    let port_value = view.runtime.port.map(|p| p.to_string()).unwrap_or_default();
    let port_range_label = skiff_app::runtime_catalog::port_range(&view.runtime.kind)
        .map(|r| format!("{}–{}", r.start(), r.end()))
        .unwrap_or_default();
    handlers::render(RuntimeDetailPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/env"),
        runtime: runtime_row(&state, &view),
        config,
        node,
        bound_sites,
        pending_confirm,
        pending_confirm_label,
        pending_port,
        port_editable,
        port_value,
        port_range_label,
        health_url: format!("/{prefix}/runtimes/{id}/health"),
        clone_url: format!("/{prefix}/runtimes/{id}/clone"),
        socket_host: view.socket_host.clone(),
        socket_container: view.socket_container.clone(),
        socket_exists: view.socket_exists,
        socket_mode: view.socket_mode.clone(),
        extensions: if is_php {
            extension_options(&enabled_exts, &view.runtime.version, &view.runtime.image)
        } else {
            Vec::new()
        },
        ext_categories: [
            skiff_app::runtime_catalog::CATEGORY_DB,
            skiff_app::runtime_catalog::CATEGORY_IMAGE,
            skiff_app::runtime_catalog::CATEGORY_CRYPTO,
            skiff_app::runtime_catalog::CATEGORY_CACHE,
            skiff_app::runtime_catalog::CATEGORY_OTHER,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
        has_job,
        job: job_view,
        job_url: format!("/{prefix}/runtimes/{}/job", encode_uri_component(&id)),
        logs_url: format!(
            "/{prefix}/runtimes/{}/logs?tail=300",
            encode_uri_component(&id)
        ),
        env_href: format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        runtime_log_path: if matches!(
            view.runtime.kind.as_str(),
            skiff_store::KIND_RUST | skiff_store::KIND_NODE
        ) {
            state
                .site_settings()
                .runtime_log_dir(&view.runtime.kind, &view.runtime.name)
                .join("server.log")
                .display()
                .to_string()
        } else {
            String::new()
        },
        logs,
        logs_error,
        php_files,
        php_editor,
        php_values,
        php_values_loaded,
        php_values_error,
        config_mounts_stale,
        can_admin: user.can(skiff_model::Permission::Admin),
        error: page_error,
        notice: String::new(),
    })
}

fn parse_u32_field(
    form: &HashMap<String, Vec<String>>,
    key: &str,
    default: u32,
) -> Result<u32, AppError> {
    let raw = form_first(form, key);
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(default);
    }
    raw.parse::<u32>()
        .map_err(|_| AppError::BadRequest(format!("{key} 必须是整数")))
}

/// 保存 PHP 配置文件：校验在容器内完成，失败回滚并提示原因。
pub async fn runtime_config_file_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_CONFIG_FILE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let key = form_first(&form, "key");
    let content = form_first(&form, "content");
    let prefix = &state.cfg.entry_prefix;
    let back = format!(
        "/{prefix}/runtimes/{}?tab=config&file={}",
        encode_uri_component(&id),
        encode_uri_component(&key)
    );
    match runtime_cmd::save_config_file(
        &state,
        runtime_cmd::SaveConfigFile {
            id: id.clone(),
            key,
            content,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(file) => {
            // PHP Warning 不阻止保存，但原文回显，避免“保存成功”掩盖潜在问题。
            let mut notice = format!("{} 已保存，PHP-FPM 已重启", file.label);
            if !file.warnings.is_empty() {
                notice.push_str("；校验警告（不影响本次保存）：");
                notice.push_str(&file.warnings.join("；"));
            }
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                back,
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            back,
        )),
    }
}

/// 修改进程型环境的固定端口。端口属于环境，站点侧不可自定义；有绑定站点时
/// 先进入与启停一致的二次确认（改端口会重写全部站点反代并重建容器）。
pub async fn runtime_port_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_PORT)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    let back = format!("/{prefix}/runtimes/{id}");
    let raw = form_first(&form, "port");
    let port: u16 = raw
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest(format!("端口不合法：{raw}")))?;
    let domains = state
        .db()
        .runtime_site_domains(&id)
        .map_err(|e| AppError::Internal(format!("{e}")))?;
    if !domains.is_empty() && form_first(&form, "confirm") != "1" {
        return Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!(
                "该运行环境已被 {} 个站点使用（{}），改端口会重写全部站点反代并重建容器，请确认后执行",
                domains.len(),
                domains.join("、")
            ),
            format!("/{prefix}/runtimes/{id}?confirm=port&port={port}"),
        ));
    }
    match runtime_cmd::save_port(
        &state,
        runtime_cmd::SavePort {
            id: id.clone(),
            port,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("端口已改为 {port}，正在重建运行环境"),
            back,
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            back,
        )),
    }
}

/// 克隆进程型环境：新名称可留空（默认 `<原名>-copy`），端口由内核重新分配。
pub async fn runtime_clone(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_CLONE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    let name = form_first(&form, "name");
    match runtime_cmd::clone(
        &state,
        runtime_cmd::CloneRuntime {
            id: id.clone(),
            name,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(rt) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!(
                "已克隆为 {}，端口 {}；正在后台安装",
                rt.name,
                rt.port.unwrap_or_default()
            ),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&rt.id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            format!("/{prefix}/runtimes/{id}"),
        )),
    }
}

/// 应用健康检查（只读）：容器 running 不等于应用就绪，这里按需探测一次。
pub async fn runtime_health(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    require_admin(&user)?;
    let prefix = &state.cfg.entry_prefix;
    let back = format!("/{prefix}/runtimes/{id}");
    match state.probe_runtime_health(&id).await {
        Ok(msg) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            msg,
            back,
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            back,
        )),
    }
}

pub async fn runtime_action(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_ACTION)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let op = match action.as_str() {
        "start" => runtime_cmd::RuntimeOp::Start,
        "stop" => runtime_cmd::RuntimeOp::Stop,
        "restart" => runtime_cmd::RuntimeOp::Restart,
        "rebuild" => runtime_cmd::RuntimeOp::Rebuild,
        "sync" => runtime_cmd::RuntimeOp::Sync,
        "delete" => runtime_cmd::RuntimeOp::Delete,
        "install" => runtime_cmd::RuntimeOp::Install,
        _ => return Err(AppError::BadRequest(format!("未知运行环境操作：{action}"))),
    };
    let prefix = &state.cfg.entry_prefix;
    let web = id == "web";
    // Web 服务不落库，只支持安装/重建/启停/删除；其余动作按旧行为返回 400。
    if web && op == runtime_cmd::RuntimeOp::Sync {
        return Err(AppError::BadRequest(format!(
            "Web 服务不支持操作：{action}"
        )));
    }
    let back = if web {
        format!("/{prefix}/sites/web?tab=status")
    } else {
        format!("/{prefix}/runtimes/{}", encode_uri_component(&id))
    };
    let outcome = runtime_cmd::action(
        &state,
        runtime_cmd::RuntimeAction {
            id: id.clone(),
            op,
            confirmed: form_first(&form, "confirm") == "1",
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await;
    match outcome {
        Ok(runtime_cmd::RuntimeActionOutcome::Web) => {
            let notice = match op {
                runtime_cmd::RuntimeOp::Install | runtime_cmd::RuntimeOp::Rebuild => {
                    "已提交 Web 服务（OpenResty）安装任务；PHP/Node/Go/Rust 不受影响"
                }
                runtime_cmd::RuntimeOp::Delete => {
                    "Web 服务容器已删除（语言运行环境各自独立，未受影响）"
                }
                _ => "Web 服务操作已完成",
            };
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                back,
            ))
        }
        Ok(runtime_cmd::RuntimeActionOutcome::NeedsConfirm { domains }) => {
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!(
                    "该运行环境已被 {} 个站点使用（{}），{}会影响全部站点，请确认后执行",
                    domains.len(),
                    domains.join("、"),
                    action_label(&action)
                ),
                format!("/{prefix}/runtimes/{id}?confirm={action}"),
            ))
        }
        Ok(runtime_cmd::RuntimeActionOutcome::Deleted(report)) => {
            let mut notice = "运行环境已删除".to_string();
            if !report.skipped.is_empty() {
                notice.push_str("；注意：");
                notice.push_str(&report.skipped.join("；"));
            }
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                format!("/{prefix}/runtimes"),
            ))
        }
        Ok(runtime_cmd::RuntimeActionOutcome::Done) => {
            let notice = if matches!(
                op,
                runtime_cmd::RuntimeOp::Install | runtime_cmd::RuntimeOp::Rebuild
            ) {
                "已提交后台任务：状态列实时显示阶段，详情抽屉可查看安装日志"
            } else {
                "操作已完成"
            };
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                back,
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            back,
        )),
    }
}

/// 抽屉轮询：单个运行环境的任务快照（状态 / 阶段 / 进度 / 实时日志）。
pub async fn runtime_job(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    match state.runtime_job(&id).await {
        Some(job) => Ok(axum::Json(job_json(&job))),
        None => Ok(axum::Json(serde_json::json!({
            "id": id,
            "state": "none",
            "state_label": "",
            "step": "",
            "progress": 0,
            "error": "",
            "log": "",
            "active": false,
            "failed": false,
            "kind": "",
        }))),
    }
}

/// 列表轮询：全部运行环境任务（只返回有任务记录的环境）。
pub async fn runtime_jobs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let jobs = state.runtime_jobs_snapshot().await;
    Ok(axum::Json(serde_json::json!({
        "jobs": jobs.iter().map(job_json).collect::<Vec<_>>(),
    })))
}

/// 扩展增删：写库 + 后台执行（启用脚本 / 停用脚本 + 重载 FPM）。
pub async fn runtime_extensions_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_EXTENSIONS)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let selected = form.get("ext").cloned().unwrap_or_default();
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::save_extensions(
        &state,
        runtime_cmd::SaveExtensions {
            id: id.clone(),
            extensions: selected,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(changed) => {
            let notice = if changed {
                "扩展变更任务已提交：进度与日志会实时刷新"
            } else {
                "扩展集合未变化"
            };
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                notice,
                format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// 安装单个扩展（目录项或 `pecl:` token）：异步任务，立即返回。
pub async fn runtime_extension_install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, key)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_INSTALL_EXTENSION)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::install_extension(
        &state,
        runtime_cmd::ExtensionChange {
            id: id.clone(),
            key: key.clone(),
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("扩展 {key} 安装任务已提交，进度与日志会实时刷新"),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// 卸载单个扩展：删除 ini → 重启容器 → 从启用集合移除。
pub async fn runtime_extension_uninstall(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, key)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_UNINSTALL_EXTENSION)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::uninstall_extension(
        &state,
        runtime_cmd::ExtensionChange {
            id: id.clone(),
            key: key.clone(),
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("扩展 {key} 卸载任务已提交"),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// 批量勾选安装/卸载：一次请求、一个任务、日志合流、只重启一次容器。
pub async fn runtime_extensions_batch(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_BATCH_EXTENSIONS)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let op = form_first(&form, "op").trim().to_string();
    let add = match op.as_str() {
        "install" => true,
        "uninstall" => false,
        _ => {
            return Err(AppError::BadRequest(format!(
                "未知扩展操作：{op}（应为 install / uninstall）"
            )))
        }
    };
    let keys = form.get("ext").cloned().unwrap_or_default();
    let prefix = &state.cfg.entry_prefix;
    if keys.is_empty() {
        return Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            "未选择任何扩展",
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        ));
    }
    match runtime_cmd::batch_extensions(
        &state,
        runtime_cmd::BatchExtensions {
            id: id.clone(),
            install: add,
            keys,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(0) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "所选扩展均已是目标状态，无需变更",
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Ok(n) => {
            let action = if add { "安装" } else { "卸载" };
            Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Notice,
                format!("{n} 项扩展{action}任务已提交（单任务串行执行，日志合流，只重启一次容器）"),
                format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
            ))
        }
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// 按名字安装 PECL 扩展（可选版本）。
pub async fn runtime_pecl_install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_INSTALL_PECL)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let name = form_first(&form, "name").trim().to_string();
    let version = form_first(&form, "version").trim().to_string();
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::install_pecl(
        &state,
        runtime_cmd::InstallPecl {
            id: id.clone(),
            name: name.clone(),
            version,
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("PECL {name} 安装任务已提交"),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// 抽屉日志页签：安装日志在页面快照里，运行日志由这里按需拉取。
pub async fn runtime_logs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    require_admin(&user)?;
    let tail = q
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(300)
        .min(2000);
    match state.runtime_logs(&id, tail).await {
        Ok(text) => Ok(axum::Json(serde_json::json!({
            "ok": true,
            "logs": text,
        }))),
        Err(e) => Ok(axum::Json(serde_json::json!({
            "ok": false,
            "logs": e.to_string(),
        }))),
    }
}

/// 「实测版本」按钮：唯一主动进容器执行的位置，2 秒超时。
pub async fn runtime_probe(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<axum::Json<serde_json::Value>, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_PROBE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match runtime_cmd::probe(
        &state,
        runtime_cmd::ProbeRuntime {
            id: id.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(version) => Ok(axum::Json(serde_json::json!({
            "ok": true,
            "version": version,
            "at": at,
        }))),
        Err(e) => Ok(axum::Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
            "at": at,
        }))),
    }
}

/// 站点页签的解绑入口：停用 vhost、站点转静态并停止。
pub async fn runtime_unbind_site(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path((id, domain)): Path<(String, String)>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_UNBIND_SITE)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::unbind_site(
        &state,
        runtime_cmd::UnbindSite {
            id: id.clone(),
            domain: domain.clone(),
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            format!("站点 {domain} 已解绑并停止"),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            e.to_string(),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

pub async fn runtime_config_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_CONFIG)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let default = skiff_app::sites::PhpRuntimeConfig::default();
    let cfg = skiff_app::sites::PhpRuntimeConfig {
        memory_limit: form_first(&form, "memory_limit").trim().to_string(),
        upload_max_filesize: form_first(&form, "upload_max_filesize").trim().to_string(),
        post_max_size: form_first(&form, "post_max_size").trim().to_string(),
        max_execution_time: parse_u32_field(&form, "max_execution_time", 30)? as u64,
        pm: form_first(&form, "pm").trim().to_string(),
        max_children: parse_u32_field(&form, "max_children", 20)?,
        start_servers: parse_u32_field(&form, "start_servers", 3)?,
        min_spare_servers: parse_u32_field(&form, "min_spare_servers", 2)?,
        max_spare_servers: parse_u32_field(&form, "max_spare_servers", 8)?,
        max_requests: parse_u32_field(&form, "max_requests", 500)?,
    };
    // 缺省字段回落到默认值，避免空表单把合法配置清成非法值。
    let cfg = skiff_app::sites::PhpRuntimeConfig {
        memory_limit: if cfg.memory_limit.is_empty() {
            default.memory_limit.clone()
        } else {
            cfg.memory_limit
        },
        upload_max_filesize: if cfg.upload_max_filesize.is_empty() {
            default.upload_max_filesize.clone()
        } else {
            cfg.upload_max_filesize
        },
        post_max_size: if cfg.post_max_size.is_empty() {
            default.post_max_size.clone()
        } else {
            cfg.post_max_size
        },
        pm: if cfg.pm.is_empty() {
            default.pm.clone()
        } else {
            cfg.pm
        },
        ..cfg
    };
    let prefix = &state.cfg.entry_prefix;
    match runtime_cmd::save_config(
        &state,
        runtime_cmd::SavePhpConfig {
            id: id.clone(),
            config: cfg,
            actor: actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(()) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "PHP 配置已保存并重载 FPM",
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            format!("/{prefix}/runtimes/{}", encode_uri_component(&id)),
        )),
    }
}

/// Node.js 专属配置：命令 / 健康检查 / NODE_ENV / 额外环境变量。
///
/// 这些字段进入容器 `command` 与 `Env`，保存后必须重建才能生效；Handler 在
/// 「配置确实变化」时提交一次重建任务，避免每次点保存都重启应用。
pub async fn runtime_node_config_save(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    crate::authz::authorize(&user, act::RUNTIME_SAVE_NODE_CONFIG)?;
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let prefix = &state.cfg.entry_prefix;
    let back = format!("/{prefix}/runtimes/{}", encode_uri_component(&id));
    let outcome = runtime_cmd::save_node_config(
        &state,
        runtime_cmd::SaveNodeConfig {
            id: id.clone(),
            command: form_first(&form, "command"),
            health_path: form_first(&form, "health_path"),
            node_env: form_first(&form, "node_env"),
            env_text: form_first(&form, "env"),
            actor: actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await;
    match outcome {
        Ok(runtime_cmd::NodeConfigOutcome::Unchanged) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "Node.js 配置未变化，无需重建",
            back,
        )),
        Ok(runtime_cmd::NodeConfigOutcome::RebuildSubmitted) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Notice,
            "Node.js 配置已保存，正在重建环境以生效",
            back,
        )),
        Ok(runtime_cmd::NodeConfigOutcome::RebuildFailed(message)) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("配置已保存，但重建任务未启动：{message}"),
            back,
        )),
        Err(e) => Ok(handlers::flash_redirect(
            &state,
            &user,
            skiff_app::FlashKind::Error,
            format!("{e}"),
            back,
        )),
    }
}
