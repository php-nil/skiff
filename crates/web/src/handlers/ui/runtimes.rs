use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/runtimes`：运行环境列表与安装状态（只读，管理员）。
///
/// 权限与页面一致（`require_admin`）；列表过滤沿用 `RuntimeView::is_listed`，
/// 展示字段来自页面同一个 `runtime_row`，不新增 SQL。
pub async fn runtimes(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<ListQuery>,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let views = state.runtime_views().await.unwrap_or_default();
    let needle = q.q.as_deref().unwrap_or("").trim().to_lowercase();
    let rows: Vec<(String, RuntimeItem)> = views
        .iter()
        .filter(|v| v.is_listed())
        .filter(|v| {
            needle.is_empty()
                || v.runtime.id.to_lowercase().contains(&needle)
                || v.runtime.name.to_lowercase().contains(&needle)
                || skiff_app::runtime_catalog::kind_label(&v.runtime.kind)
                    .to_lowercase()
                    .contains(&needle)
        })
        .map(|v| {
            let row = handlers::sites::runtime_row(&state, v);
            let job = v.job.as_ref().map(|j| RuntimeJobItem {
                state: j.state.as_str().to_string(),
                label: crate::labels::runtime_job_state(j.state).to_string(),
                step: j.step.clone(),
                progress: j.progress,
                error: j.error.clone(),
                active: j.active(),
                failed: j.state == skiff_app::sites::RuntimeJobState::Failed,
                started_at: rfc3339(j.started_at),
                updated_at: rfc3339(j.updated_at),
            });
            (
                v.runtime.id.clone(),
                RuntimeItem {
                    id: row.id,
                    kind: StatusDto {
                        code: row.kind_key,
                        label: row.kind_label,
                    },
                    name: row.name,
                    container_name: row.container_name,
                    version: row.version,
                    version_display: row.version_display,
                    version_note: row.version_note,
                    image: row.image,
                    state: StatusDto {
                        code: v.effective_state.clone(),
                        label: row.state_label,
                    },
                    state_class: row.state_class,
                    drift: row.drift,
                    site_count: row.site_count,
                    container_state: row.container_state,
                    endpoint_short: row.endpoint_short,
                    endpoint_full: row.endpoint_full,
                    implemented: row.implemented,
                    is_default: row.is_default,
                    created_at: rfc3339(v.runtime.created_at),
                    updated_at: rfc3339(v.runtime.updated_at),
                    job,
                },
            )
        })
        .collect();
    let total = rows.len();
    let start = match q.cursor.as_deref() {
        Some(cursor) => rows
            .iter()
            .position(|(id, _)| id == cursor)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let end = (start + page_limit(&q)).min(total);
    let next_cursor = if end < total && end > start {
        rows.get(end - 1).map(|(id, _)| id.clone())
    } else {
        None
    };
    let items: Vec<RuntimeItem> = rows
        .into_iter()
        .skip(start)
        .take(end - start)
        .map(|(_, item)| item)
        .collect();
    let mut resp = Json(json!({
        "data": { "items": items, "total": total, "next_cursor": next_cursor }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /ui/v1/runtimes/{id}/logs?tail=`：运行环境日志尾部快照（Admin）。
///
/// 复用 `AppState::runtime_logs`（与旧详情抽屉同一读取路径），页面按需刷新。
pub async fn runtime_logs(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::permission(&user, skiff_model::Permission::Admin)?;
    let tail = q
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(300)
        .clamp(1, 2000);
    let text = state
        .runtime_logs(&id, tail)
        .await
        .map_err(site_api_error)?;
    Ok(Json(json!({
        "data": {
            "runtime_id": id,
            "tail": tail,
            "text": text,
        }
    })))
}

/// `GET /ui/v1/runtimes/{id}[?file=key]`：运行环境详情聚合查询（Admin）。
///
/// 与旧详情页同一数据源：`runtime_view`、`php_config_files`、`read_php_config_file`、
/// `extension_options` 与 `list_sites`；不新增 SQL，也不在浏览器重复拼装。
pub async fn runtime_detail(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    Query(q): Query<RuntimeFileQuery>,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let view = state
        .runtime_view(&id)
        .await
        .map_err(handlers::sites::map_err)
        .map_err(ApiError)?;
    let row = handlers::sites::runtime_row(&state, &view);
    let is_php = view.runtime.kind == skiff_store::KIND_PHP;
    let is_node = view.runtime.kind == skiff_store::KIND_NODE;
    let php = skiff_app::sites::PhpRuntimeConfig::from_json(&view.runtime.config_json);
    let node = skiff_app::sites::NodeRuntimeConfig::from_json(&view.runtime.config_json);
    let sites: Vec<serde_json::Value> = state
        .db()
        .list_sites()
        .map_err(|e| ApiError(AppError::from(e)))?
        .into_iter()
        .filter(|s| s.runtime_id.as_deref() == Some(id.as_str()))
        .map(|s| {
            json!({
                "domain": s.domain,
                "site_type": s.site_type,
                "status": s.status,
            })
        })
        .collect();
    let extensions: Vec<serde_json::Value> = if is_php {
        handlers::sites::extension_options(
            &view.extensions,
            &view.runtime.version,
            &view.runtime.image,
        )
        .into_iter()
        .map(|e| {
            json!({
                "key": e.key,
                "label": e.label,
                "description": e.description,
                "enabled": e.enabled,
                "default_on": e.default_on,
                "builtin": e.builtin,
                "category": e.category,
                "source": e.source,
                "hot": e.hot,
                "heavy": e.heavy,
                "estimated_time": e.estimated_time,
                "support_note": e.support_note,
            })
        })
        .collect()
    } else {
        Vec::new()
    };
    let files: Vec<serde_json::Value> = if is_php {
        state
            .php_config_files(&id)
            .map_err(handlers::sites::map_err)
            .map_err(ApiError)?
            .into_iter()
            .map(|f| {
                json!({
                    "key": f.key,
                    "label": f.label,
                    "container_path": f.container_path,
                    "exists": f.exists,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let editor = if is_php && !q.file.is_empty() {
        match state.read_php_config_file(&id, &q.file, &user.user.username) {
            Ok(v) => json!({
                "key": v.file.key,
                "label": v.file.label,
                "content": v.content,
                "container_path": v.file.container_path,
                "note": v.file.note,
            }),
            Err(e) => return Err(ApiError(handlers::sites::map_err(e))),
        }
    } else {
        serde_json::Value::Null
    };
    let port_editable = matches!(
        view.runtime.kind.as_str(),
        skiff_store::KIND_NODE | skiff_store::KIND_RUST
    );
    let port_range = skiff_app::runtime_catalog::port_range(&view.runtime.kind)
        .map(|r| format!("{}–{}", r.start(), r.end()))
        .unwrap_or_default();
    let config_mounts_stale = if is_php && view.container_exists {
        !state
            .php_config_mounts_current(&id)
            .map_err(handlers::sites::map_err)
            .map_err(ApiError)?
    } else {
        false
    };
    write_done(json!({
        "runtime": {
            "id": row.id,
            "kind": { "code": row.kind_key, "label": row.kind_label },
            "name": row.name,
            "version": row.version,
            "version_display": row.version_display,
            "state": { "code": view.effective_state, "label": row.state_label },
            "image": row.image,
            "endpoint_short": row.endpoint_short,
            "endpoint_full": row.endpoint_full,
            "container_state": row.container_state,
            "site_count": row.site_count,
        },
        "is_php": is_php,
        "is_node": is_node,
        "config": {
            "memory_limit": php.memory_limit,
            "upload_max_filesize": php.upload_max_filesize,
            "post_max_size": php.post_max_size,
            "max_execution_time": php.max_execution_time,
            "pm": php.pm,
            "max_children": php.max_children,
            "start_servers": php.start_servers,
            "min_spare_servers": php.min_spare_servers,
            "max_spare_servers": php.max_spare_servers,
            "max_requests": php.max_requests,
        },
        "node": {
            "command": node.command,
            "health_path": node.health_path,
            "node_env": node.node_env,
            "env": node.env_text(),
        },
        "port": {
            "value": view.runtime.port,
            "editable": port_editable,
            "range": port_range,
        },
        "extensions": extensions,
        "categories": [
            skiff_app::runtime_catalog::CATEGORY_DB,
            skiff_app::runtime_catalog::CATEGORY_IMAGE,
            skiff_app::runtime_catalog::CATEGORY_CRYPTO,
            skiff_app::runtime_catalog::CATEGORY_CACHE,
            skiff_app::runtime_catalog::CATEGORY_OTHER,
        ],
        "files": files,
        "editor": editor,
        "sites": sites,
        "config_mounts_stale": config_mounts_stale,
        "can_admin": user.can(skiff_model::Permission::Admin),
    }))
}

/// `GET /ui/v1/runtimes/catalog`：可安装类型与版本（与旧新建弹窗同一目录来源）。
pub async fn runtime_catalog(
    State(_state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    let kinds: Vec<serde_json::Value> = skiff_app::runtime_catalog::RUNTIME_KINDS
        .iter()
        .map(|k| {
            json!({
                "key": k.kind,
                "label": k.label,
                "versions": k.versions,
                "implemented": skiff_app::runtime_catalog::is_implemented(k.kind),
            })
        })
        .collect();
    write_done(json!({
        "kinds": kinds,
        "default_php_version": skiff_app::sites::DEFAULT_PHP_VERSION,
    }))
}

/// `POST /ui/v1/runtimes`：创建/安装运行环境（同 `RUNTIME_CREATE` 与 `runtime_cmd::create`）。
pub async fn runtime_create(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<RuntimeCreateBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_CREATE)?;
    let extensions = body
        .extensions
        .unwrap_or_else(skiff_app::runtime_catalog::default_extensions);
    let created = runtime_cmd::create(
        &state,
        runtime_cmd::CreateRuntime {
            kind: body.kind,
            name: body.name,
            version: body.version,
            extensions,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    let message = if created.started {
        format!(
            "已提交 {} {} 安装任务（实例 {}）",
            skiff_app::runtime_catalog::kind_label(&created.runtime.kind),
            created.runtime.version,
            created.runtime.name
        )
    } else {
        format!(
            "已登记 {} {}（实例 {}）；该类型暂不安装容器",
            skiff_app::runtime_catalog::kind_label(&created.runtime.kind),
            created.runtime.version,
            created.runtime.name
        )
    };
    write_done(json!({
        "id": created.runtime.id,
        "started": created.started,
        "message": message,
    }))
}

/// `POST /ui/v1/runtimes/{id}/action/{action}`：启停/重启/重建/同步/删除/安装。
pub async fn runtime_action(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, action)): Path<(String, String)>,
    parts: Parts,
    Json(body): Json<RuntimeActionBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_ACTION)?;
    let op = match action.as_str() {
        "start" => runtime_cmd::RuntimeOp::Start,
        "stop" => runtime_cmd::RuntimeOp::Stop,
        "restart" => runtime_cmd::RuntimeOp::Restart,
        "rebuild" => runtime_cmd::RuntimeOp::Rebuild,
        "sync" => runtime_cmd::RuntimeOp::Sync,
        "delete" => runtime_cmd::RuntimeOp::Delete,
        "install" => runtime_cmd::RuntimeOp::Install,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知运行环境操作：{action}"
            ))))
        }
    };
    let web = id == "web";
    if web && op == runtime_cmd::RuntimeOp::Sync {
        return Err(ApiError(AppError::BadRequest(format!(
            "Web 服务不支持操作：{action}"
        ))));
    }
    let outcome = runtime_cmd::action(
        &state,
        runtime_cmd::RuntimeAction {
            id: id.clone(),
            op,
            confirmed: body.confirm,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    match outcome {
        runtime_cmd::RuntimeActionOutcome::NeedsConfirm { domains } => {
            Err(ApiError(AppError::Conflict(format!(
                "该运行环境已被 {} 个站点使用（{}），{}会影响全部站点；确认后重试",
                domains.len(),
                domains.join("、"),
                action
            ))))
        }
        runtime_cmd::RuntimeActionOutcome::Web => write_done(json!({
            "message": if action == "delete" {
                "Web 服务容器已删除（语言运行环境各自独立，未受影响）".to_string()
            } else {
                "Web 服务操作已完成".to_string()
            }
        })),
        runtime_cmd::RuntimeActionOutcome::Deleted(report) => write_done(json!({
            "deleted": true,
            "skipped": report.skipped,
            "message": "运行环境已删除",
        })),
        runtime_cmd::RuntimeActionOutcome::Done => write_done(json!({
            "message": if matches!(
                op,
                runtime_cmd::RuntimeOp::Install | runtime_cmd::RuntimeOp::Rebuild
            ) {
                "已提交后台任务：状态列实时显示阶段，详情可查看安装日志"
            } else {
                "操作已完成"
            }
        })),
    }
}

/// `POST /ui/v1/runtimes/{id}/config`：PHP 运行参数（同 `RUNTIME_SAVE_CONFIG`）。
pub async fn runtime_config_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<PhpConfigBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_CONFIG)?;
    let default = skiff_app::sites::PhpRuntimeConfig::default();
    let config = skiff_app::sites::PhpRuntimeConfig {
        memory_limit: if body.memory_limit.trim().is_empty() {
            default.memory_limit.clone()
        } else {
            body.memory_limit.trim().to_string()
        },
        upload_max_filesize: if body.upload_max_filesize.trim().is_empty() {
            default.upload_max_filesize.clone()
        } else {
            body.upload_max_filesize.trim().to_string()
        },
        post_max_size: if body.post_max_size.trim().is_empty() {
            default.post_max_size.clone()
        } else {
            body.post_max_size.trim().to_string()
        },
        max_execution_time: body
            .max_execution_time
            .unwrap_or(default.max_execution_time),
        pm: if body.pm.trim().is_empty() {
            default.pm.clone()
        } else {
            body.pm.trim().to_string()
        },
        max_children: body.max_children.unwrap_or(default.max_children),
        start_servers: body.start_servers.unwrap_or(default.start_servers),
        min_spare_servers: body.min_spare_servers.unwrap_or(default.min_spare_servers),
        max_spare_servers: body.max_spare_servers.unwrap_or(default.max_spare_servers),
        max_requests: body.max_requests.unwrap_or(default.max_requests),
    };
    runtime_cmd::save_config(
        &state,
        runtime_cmd::SavePhpConfig {
            id,
            config,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({ "message": "PHP 配置已保存并重载 FPM" }))
}

/// `POST /ui/v1/runtimes/{id}/node-config`：Node.js 启动命令/健康检查/环境变量。
pub async fn runtime_node_config_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<NodeConfigBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_NODE_CONFIG)?;
    let outcome = runtime_cmd::save_node_config(
        &state,
        runtime_cmd::SaveNodeConfig {
            id,
            command: body.command,
            health_path: body.health_path,
            node_env: body.node_env,
            env_text: body.env,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    let message = match outcome {
        runtime_cmd::NodeConfigOutcome::Unchanged => "Node.js 配置未变化，无需重建",
        runtime_cmd::NodeConfigOutcome::RebuildSubmitted => {
            "Node.js 配置已保存，正在重建环境以生效"
        }
        runtime_cmd::NodeConfigOutcome::RebuildFailed(_) => "配置已保存，但重建任务未启动",
    };
    write_done(json!({ "message": message }))
}

/// `POST /ui/v1/runtimes/{id}/port`：改固定端口；有绑定站点时未确认返回 409。
pub async fn runtime_port_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<RuntimePortBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_PORT)?;
    let domains = state
        .db()
        .runtime_site_domains(&id)
        .map_err(|e| ApiError(AppError::from(e)))?;
    if !domains.is_empty() && !body.confirm {
        return Err(ApiError(AppError::Conflict(format!(
            "该运行环境已被 {} 个站点使用（{}），改端口会重写全部站点反代并重建容器；确认后重试",
            domains.len(),
            domains.join("、")
        ))));
    }
    runtime_cmd::save_port(
        &state,
        runtime_cmd::SavePort {
            id,
            port: body.port,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({
        "message": format!("端口已改为 {}，正在重建运行环境", body.port)
    }))
}

/// `POST /ui/v1/runtimes/{id}/clone`：同配置克隆（端口由内核重新分配）。
pub async fn runtime_clone(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<RuntimeCloneBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_CLONE)?;
    let rt = runtime_cmd::clone(
        &state,
        runtime_cmd::CloneRuntime {
            id,
            name: body.name,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({
        "id": rt.id,
        "name": rt.name,
        "port": rt.port,
        "message": format!("已克隆为 {}，正在后台安装", rt.name),
    }))
}

/// `POST /ui/v1/runtimes/{id}/extensions`：整组扩展集合（同 `RUNTIME_SAVE_EXTENSIONS`）。
pub async fn runtime_extensions_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<RuntimeExtensionsBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_EXTENSIONS)?;
    let changed = runtime_cmd::save_extensions(
        &state,
        runtime_cmd::SaveExtensions {
            id,
            extensions: body.extensions,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({
        "changed": changed,
        "message": if changed { "扩展变更任务已提交：进度与日志会实时刷新" } else { "扩展集合未变化" },
    }))
}

/// `POST /ui/v1/runtimes/{id}/extensions/{key}/install|uninstall`：单项扩展。
pub async fn runtime_extension_change(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, key, op)): Path<(String, String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    let action = match op.as_str() {
        "install" => act::RUNTIME_INSTALL_EXTENSION,
        "uninstall" => act::RUNTIME_UNINSTALL_EXTENSION,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知扩展操作：{op}（应为 install / uninstall）"
            ))))
        }
    };
    guard::write(&state, &user, &parts, action)?;
    let change = runtime_cmd::ExtensionChange {
        id: id.clone(),
        key: key.clone(),
        actor: handlers::sites::actor(&state, &user, &parts),
        role: user.user.role,
    };
    match op.as_str() {
        "install" => {
            runtime_cmd::install_extension(&state, change)
                .await
                .map_err(handlers::sites::map_err)
                .map_err(ApiError)?;
            write_done(json!({ "message": format!("扩展 {key} 安装任务已提交") }))
        }
        _ => {
            runtime_cmd::uninstall_extension(&state, change)
                .await
                .map_err(handlers::sites::map_err)
                .map_err(ApiError)?;
            write_done(json!({ "message": format!("扩展 {key} 卸载任务已提交") }))
        }
    }
}

/// `POST /ui/v1/runtimes/{id}/extensions/batch`：批量安装/卸载，单任务串行。
pub async fn runtime_extensions_batch(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<ExtensionBatchBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_BATCH_EXTENSIONS)?;
    let install = match body.op.as_str() {
        "install" => true,
        "uninstall" => false,
        _ => {
            return Err(ApiError(AppError::BadRequest(format!(
                "未知扩展操作：{}（应为 install / uninstall）",
                body.op
            ))))
        }
    };
    if body.keys.is_empty() {
        return Err(ApiError(AppError::BadRequest("未选择任何扩展".into())));
    }
    let changed = runtime_cmd::batch_extensions(
        &state,
        runtime_cmd::BatchExtensions {
            id,
            install,
            keys: body.keys,
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({
        "changed": changed,
        "message": if changed == 0 {
            "所选扩展均已是目标状态，无需变更".to_string()
        } else {
            format!(
                "{changed} 项扩展{}任务已提交（单任务串行执行，日志合流，只重启一次容器）",
                if install { "安装" } else { "卸载" }
            )
        },
    }))
}

/// `POST /ui/v1/runtimes/{id}/pecl`：按名字安装 PECL 扩展。
pub async fn runtime_pecl_install(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<PeclBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_INSTALL_PECL)?;
    let name = body.name.trim().to_string();
    runtime_cmd::install_pecl(
        &state,
        runtime_cmd::InstallPecl {
            id,
            name: name.clone(),
            version: body.version.trim().to_string(),
            actor: handlers::sites::actor(&state, &user, &parts),
            role: user.user.role,
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({ "message": format!("PECL {name} 安装任务已提交") }))
}

/// `POST /ui/v1/runtimes/{id}/config-file`：保存 php.ini / FPM 池等配置文件。
pub async fn runtime_config_file_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<ConfigFileBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_CONFIG_FILE)?;
    let file = runtime_cmd::save_config_file(
        &state,
        runtime_cmd::SaveConfigFile {
            id,
            key: body.key,
            content: body.content,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    let mut message = format!("{} 已保存，PHP-FPM 已重启", file.label);
    if !file.warnings.is_empty() {
        message.push_str("；校验警告（不影响本次保存）：");
        message.push_str(&file.warnings.join("；"));
    }
    write_done(json!({
        "label": file.label,
        "warnings": file.warnings,
        "message": message,
    }))
}

/// `POST /ui/v1/runtimes/{id}/probe`：实测版本（2 秒超时，唯一主动进容器的写路径）。
pub async fn runtime_probe(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_PROBE)?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match runtime_cmd::probe(
        &state,
        runtime_cmd::ProbeRuntime {
            id,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    {
        Ok(version) => write_done(json!({ "ok": true, "version": version, "at": at })),
        Err(e) => write_done(json!({ "ok": false, "error": e.to_string(), "at": at })),
    }
}

/// `POST /ui/v1/runtimes/{id}/sites/{domain}/unbind`：解绑站点并转为静态。
pub async fn runtime_unbind_site(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path((id, domain)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_UNBIND_SITE)?;
    runtime_cmd::unbind_site(
        &state,
        runtime_cmd::UnbindSite {
            id,
            domain: domain.clone(),
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .await
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({ "message": format!("站点 {domain} 已解绑并停止") }))
}

/// `GET /ui/v1/runtimes/images`：当前 nginx / PHP 基础镜像标签。
pub async fn runtime_images(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
) -> Result<Response, ApiError> {
    handlers::sites::require_admin(&user).map_err(ApiError)?;
    // 读取与旧页面同源：`site_env_status` 已包含落库后的镜像标签。
    let env = state.site_env_status().await;
    write_done(json!({
        "nginx_image": env.nginx_image,
        "php_image": env.php_image,
    }))
}

/// `POST /ui/v1/runtimes/images`：保存 nginx / PHP 基础镜像标签。
pub async fn runtime_images_save(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    parts: Parts,
    Json(body): Json<RuntimeImagesBody>,
) -> Result<Response, ApiError> {
    guard::write(&state, &user, &parts, act::RUNTIME_SAVE_IMAGES)?;
    runtime_cmd::save_images(
        &state,
        runtime_cmd::SaveImages {
            nginx_image: body.nginx_image,
            php_image: body.php_image,
            actor: handlers::sites::actor(&state, &user, &parts),
        },
    )
    .map_err(handlers::sites::map_err)
    .map_err(ApiError)?;
    write_done(json!({ "message": "镜像参数已保存，重建 Web 服务或重装对应实例后生效" }))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/runtimes", get(runtimes).post(runtime_create))
        .route("/ui/v1/runtimes/catalog", get(runtime_catalog))
        .route(
            "/ui/v1/runtimes/images",
            get(runtime_images).post(runtime_images_save),
        )
        .route("/ui/v1/runtimes/{id}", get(runtime_detail))
        .route("/ui/v1/runtimes/{id}/logs", get(runtime_logs))
        .route("/ui/v1/runtimes/{id}/action/{action}", post(runtime_action))
        .route("/ui/v1/runtimes/{id}/config", post(runtime_config_save))
        .route(
            "/ui/v1/runtimes/{id}/node-config",
            post(runtime_node_config_save),
        )
        .route("/ui/v1/runtimes/{id}/port", post(runtime_port_save))
        .route("/ui/v1/runtimes/{id}/clone", post(runtime_clone))
        .route(
            "/ui/v1/runtimes/{id}/extensions",
            post(runtime_extensions_save),
        )
        .route(
            "/ui/v1/runtimes/{id}/extensions/batch",
            post(runtime_extensions_batch),
        )
        .route(
            "/ui/v1/runtimes/{id}/extensions/{key}/{op}",
            post(runtime_extension_change),
        )
        .route("/ui/v1/runtimes/{id}/pecl", post(runtime_pecl_install))
        .route(
            "/ui/v1/runtimes/{id}/config-file",
            post(runtime_config_file_save),
        )
        .route("/ui/v1/runtimes/{id}/probe", post(runtime_probe))
        .route(
            "/ui/v1/runtimes/{id}/sites/{domain}/unbind",
            post(runtime_unbind_site),
        )
}
