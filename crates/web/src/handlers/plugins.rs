use skiff_pkgfmt::package::DEFAULT_LIMITS;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use skiff_pkgfmt::Manifest;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AdminUser, AuthUser};
use crate::state::AppState;
use crate::templates::{PermissionRow, PluginConfirmPage, PluginView, PluginsPage};
use crate::util::{form_first, parse_form};

fn plugin_views(state: &AppState) -> Result<Vec<PluginView>, AppError> {
    let mut out = Vec::new();
    for p in state.db.list_plugins()? {
        let signed = !p.key_id.is_empty() && state.db.trusted_key(&p.key_id)?.is_some();
        let reduced = Manifest::parse(&p.manifest)
            .map(|m| !m.requires_full_sandbox)
            .unwrap_or(true);
        out.push(PluginView {
            id: p.id.clone(),
            name: p.name,
            version: p.version,
            api: p.api,
            status: p.status.clone(),
            enabled: p.enabled,
            key_id: p.key_id.chars().take(16).collect(),
            grants: p
                .grants
                .iter()
                .map(|g| format!("{}:{}", g.kind, g.value))
                .collect::<Vec<_>>()
                .join(", "),
            signed,
            reduced,
            running: false,
        });
    }
    Ok(out)
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    // 运行态从 Supervisor 查询（异步，单独收集）
    let mut plugins = plugin_views(&state)?;
    for p in plugins.iter_mut() {
        p.running = state.supervisor.is_running(&p.id).await;
    }
    let page = PluginsPage {
        ctx: handlers::page_ctx(&state, &user, "plugins"),
        plugins,
        can_admin: user.is_admin(),
        error: String::new(),
        notice: String::new(),
        allow_unsigned: state.cfg.allow_unsigned,
        reduced_allowed: state.cfg.allow_reduced_sandbox,
    };
    handlers::render(page)
}

/// 第一步：上传包 → 校验（结构/签名/API/能力）→ 权限确认页。
pub async fn install(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    mut multipart: Multipart,
) -> Result<Response, AppError> {
    let mut bytes: Option<(String, Bytes)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("上传解析失败: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name != "package" {
            continue;
        }
        let filename = field.file_name().unwrap_or("package.wbpkg").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(format!("读取上传内容失败: {e}")))?;
        bytes = Some((filename, data));
    }
    let Some((filename, data)) = bytes else {
        return Err(AppError::BadRequest("缺少 package 文件字段".into()));
    };
    if data.len() as u64 > DEFAULT_LIMITS.archive_bytes {
        return Err(AppError::BadRequest(format!(
            "包超过 {} 字节上限",
            DEFAULT_LIMITS.archive_bytes
        )));
    }

    // 存到 .incoming（root 0700），供确认页二次提交
    let incoming = state.cfg.data_dir.join("plugins").join(".incoming");
    std::fs::create_dir_all(&incoming).map_err(|e| AppError::Internal(e.to_string()))?;
    let token = skiff_identity::new_token().replace(['-', '_'], "");
    let token = token.chars().take(24).collect::<String>();
    let path = incoming.join(format!("{token}.wbpkg"));
    std::fs::write(&path, &data).map_err(|e| AppError::Internal(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    let meta = verify_for_confirm(&state, &path)?.meta;

    let permissions = meta
        .manifest
        .permission_strings()
        .into_iter()
        .map(|v| {
            let (cap, verb) = v.split_once(':').unwrap_or((v.as_str(), ""));
            PermissionRow {
                value: v.clone(),
                capability: cap.to_string(),
                verb: verb.to_string(),
            }
        })
        .collect();

    let page = PluginConfirmPage {
        ctx: handlers::page_ctx(&state, &user, "plugins"),
        token,
        id: meta.manifest.id.clone(),
        name: meta.manifest.name.clone(),
        version: meta.manifest.version.clone(),
        api: meta.manifest.api.clone(),
        key_id: meta.key_id.clone(),
        signed: meta.signature_verified,
        permissions,
        requires: meta.manifest.requires_str().join(", "),
        egress: if meta.manifest.network_egress.is_empty() {
            "无（默认拒绝出网）".into()
        } else {
            meta.manifest.network_egress.join(", ")
        },
        error: format!("来源文件: {filename}"),
    };
    handlers::render(page)
}

fn verify_for_confirm(
    state: &AppState,
    path: &FsPath,
) -> Result<skiff_plugin::TrustDecision, AppError> {
    // 判定逻辑与 CLI 完全同一份；这里只把「未导入公钥」翻成页面里的话术。
    skiff_plugin::resolve_trust(path, &state.db, state.cfg.allow_unsigned).map_err(|e| match e {
        skiff_plugin::TrustError::UnverifiedKey { key_id } => AppError::BadRequest(format!(
            "未导入该包的公钥（指纹 {key_id}）。请先在「设置 → 信任公钥」导入，再安装；\
             未签名包默认拒装。"
        )),
        other => AppError::BadRequest(other.to_string()),
    })
}

/// 第二步：权限确认后安装。
pub async fn confirm(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let token = form_first(&form, "token");
    if token.len() != 24 || !token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::BadRequest("确认令牌非法".into()));
    }
    let incoming = state.cfg.data_dir.join("plugins").join(".incoming");
    let path = incoming.join(format!("{token}.wbpkg"));
    if path.parent() != Some(incoming.as_path()) || !path.exists() {
        return Err(AppError::BadRequest("上传文件已过期，请重新上传".into()));
    }

    let trust = verify_for_confirm(&state, &path)?;
    let meta = trust.meta;
    let trusted = trust.trusted_key;
    let granted = form.get("grant").cloned().unwrap_or_default();
    let available = skiff_pkgfmt::manifest::Capability::available_in(state.cfg.form);

    let opts = skiff_plugin::InstallOptions {
        trusted_public_key: trusted,
        allow_unsigned: state.cfg.allow_unsigned,
        granted_permissions: granted.clone(),
        object_grants: vec![],
        available_capabilities: available,
        allow_reduced_sandbox: state.cfg.allow_reduced_sandbox,
        arch: skiff_pkgfmt::host_arch(),
    };

    let result = skiff_plugin::install_package(&state.db, &state.cfg.data_dir, &path, &opts);
    let _ = std::fs::remove_file(&path);
    let ip = handlers::client_ip(&state, &parts);
    match &result {
        Ok(info) => {
            let _ = state.db.audit(
                &user.user.username,
                &ip,
                "plugin.install",
                &info.id,
                "ok",
                &format!(
                    "version={} key_id={} grants={}",
                    info.version,
                    info.key_id,
                    info.grants.len()
                ),
            );
        }
        Err(e) => {
            let _ = state.db.audit(
                &user.user.username,
                &ip,
                "plugin.install",
                &meta.manifest.id,
                "fail",
                &e.to_string(),
            );
        }
    }
    result.map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(handlers::redirect(format!(
        "/{}/plugins",
        state.cfg.entry_prefix
    )))
}

pub async fn toggle(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let rec = state
        .db
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let enable = !rec.enabled;
    if !enable {
        state.supervisor.stop(&id).await;
    }
    state.db.set_plugin_enabled(&id, enable)?;
    state
        .db
        .set_plugin_status(&id, if enable { "installed" } else { "disabled" }, "")?;
    let _ = state.db.audit(
        &user.user.username,
        &handlers::client_ip(&state, &parts),
        "plugin.toggle",
        &id,
        "ok",
        &format!("enabled={enable}"),
    );
    Ok(handlers::redirect(format!(
        "/{}/plugins",
        state.cfg.entry_prefix
    )))
}

pub async fn remove(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let delete_data = form_first(&form, "delete_data") == "on";
    state.supervisor.stop(&id).await;
    skiff_plugin::uninstall_package(&state.db, &state.cfg.data_dir, &id, delete_data)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    let _ = state.db.audit(
        &user.user.username,
        &handlers::client_ip(&state, &parts),
        "plugin.uninstall",
        &id,
        "ok",
        &format!("delete_data={delete_data}"),
    );
    Ok(handlers::redirect(format!(
        "/{}/plugins",
        state.cfg.entry_prefix
    )))
}

/// 插件 UI 的 RPC 桥（父页面代发）。admin 专属，逐次审计。
pub async fn rpc(
    State(state): State<Arc<AppState>>,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let req: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| AppError::BadRequest(format!("请求体不是 JSON: {e}")))?;
    let method = req
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or_else(|| AppError::BadRequest("缺少 method".into()))?
        .to_string();
    let params = req
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let rec = state
        .db
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    if !rec.enabled {
        return Err(AppError::BadRequest("插件已停用".into()));
    }
    let result = state.supervisor.call(&rec, &method, params).await;
    let _ = state.db.audit(
        &user.user.username,
        &handlers::client_ip(&state, &parts),
        "plugin.rpc",
        &id,
        if result.is_ok() { "ok" } else { "fail" },
        &format!("method={method}"),
    );
    match result {
        Ok(v) => Ok(axum::Json(serde_json::json!({ "result": v })).into_response()),
        Err(e) => Err(AppError::BadRequest(e.to_string())),
    }
}

/// 托管插件 UI：`/plugins/{id}/ui/*`，沙箱 iframe 加载（CSP 收紧，§4.2）。
pub async fn plugin_ui(
    State(state): State<Arc<AppState>>,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let rec = state
        .db
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound("插件不存在".into()))?;
    if !rec.enabled {
        return Err(AppError::NotFound("插件已停用".into()));
    }
    let root = state
        .cfg
        .data_dir
        .join("plugins")
        .join(&rec.id)
        .join("versions")
        .join(&rec.active_version)
        .join("ui");
    let rel = path.replace('\\', "/");
    if rel.contains("..") {
        return Err(AppError::Forbidden("路径非法".into()));
    }
    let target = root.join(&rel);
    let canon_root =
        std::fs::canonicalize(&root).map_err(|_| AppError::NotFound("无 UI 资源".into()))?;
    let canon =
        std::fs::canonicalize(&target).map_err(|_| AppError::NotFound("资源不存在".into()))?;
    if !canon.starts_with(&canon_root) || !canon.is_file() {
        return Err(AppError::NotFound("资源不存在".into()));
    }
    let data = std::fs::read(&canon).map_err(|e| AppError::Internal(e.to_string()))?;
    let mime = mime_guess::from_path(&canon).first_or_octet_stream();
    let mut resp = Response::new(axum::body::Body::from(data));
    resp.headers_mut()
        .insert(CONTENT_TYPE, mime.to_string().parse().unwrap());
    resp.headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    resp.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; \
         object-src 'none'; frame-ancestors 'self'"
            .parse()
            .unwrap(),
    );
    Ok(resp)
}
