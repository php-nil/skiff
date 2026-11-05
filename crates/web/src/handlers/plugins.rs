use skiff_pkgfmt::package::DEFAULT_LIMITS;
use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_FRAME_OPTIONS};
use axum::http::request::Parts;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};

use skiff_pkgfmt::Manifest;
use skiff_store::PluginRecord;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::{check_csrf, AuthUser, PluginManagerUser};
use crate::state::AppState;
use crate::templates::{
    GrantRow, PermissionRow, PluginConfirmPage, PluginDetailPage, PluginView, PluginsPage, TabView,
};
use crate::util::{form_first, parse_form};

/// `?open=` 定位串的字符上限：够放一条深层路径，又不至于把 URL 撑爆。
const OPEN_PATH_MAX: usize = 512;

/// 插件 UI 的 CSP：脚本/样式只能来自插件包自身，网络请求全禁。
/// 抽成常量是为了让契约测试能直接断言「没有放开外链」。
const PLUGIN_UI_CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; \
     object-src 'none'; frame-ancestors 'self'";

/// 一条插件记录 → 列表/详情通用的视图。
fn plugin_view(state: &AppState, p: &PluginRecord) -> Result<PluginView, AppError> {
    let signed = !p.key_id.is_empty() && state.db().trusted_key(&p.key_id)?.is_some();
    let manifest = Manifest::parse(&p.manifest).ok();
    let reduced = manifest
        .as_ref()
        .map(|m| !m.requires_full_sandbox)
        .unwrap_or(true);
    let ui = manifest.as_ref().and_then(|m| m.ui.first());
    Ok(PluginView {
        id: p.id.clone(),
        name: p.name.clone(),
        version: p.version.clone(),
        api: p.api.clone(),
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
        ui_path: ui.map(|u| u.path.clone()).unwrap_or_default(),
        ui_label: ui.map(|u| u.label.clone()).unwrap_or_default(),
    })
}

fn plugin_views(state: &AppState) -> Result<Vec<PluginView>, AppError> {
    state
        .db()
        .list_plugins()?
        .iter()
        .map(|p| plugin_view(state, p))
        .collect()
}

/// manifest 里的插件间接口：`(对外提供的 method · label, 允许调用的目标列表)`。
fn interop_rows(manifest: &Manifest) -> (Vec<String>, Vec<String>) {
    let provides = manifest
        .interop
        .provides
        .iter()
        .map(|p| match p.label.as_deref().map(str::trim) {
            Some(l) if !l.is_empty() => format!("{} · {}", p.method, l),
            _ => p.method.clone(),
        })
        .collect();
    (provides, manifest.interop.consumes.to_vec())
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    // 运行态从 Supervisor 查询（异步，单独收集）
    let mut plugins = plugin_views(&state)?;
    for p in plugins.iter_mut() {
        p.running = state.supervisor().is_running(&p.id).await;
    }
    let enabled_count = plugins.iter().filter(|p| p.enabled).count();
    let disabled_count = plugins.len().saturating_sub(enabled_count);
    let tab = match q.get("tab").map(String::as_str) {
        Some("discover") => "discover",
        Some("updates") => "updates",
        _ => "installed",
    }
    .to_string();
    let page = PluginsPage {
        ctx: handlers::page_ctx(&state, &user, &format!("core/plugins/{tab}")),
        plugins,
        enabled_count,
        disabled_count,
        tab,
        can_admin: user.can(skiff_model::Permission::PluginManage),
        error: String::new(),
        notice: String::new(),
        allow_unsigned: state.cfg.allow_unsigned,
        reduced_allowed: state.cfg.allow_reduced_sandbox,
    };
    handlers::render(page)
}

/// 插件详情页：沙箱 iframe + RPC 桥。
///
/// nonce 每次打开重新生成，父页面只代发带有该 nonce 的 `skiff.rpc` 消息；
/// iframe 内容按 `style-src 'self' 'unsafe-inline'`、`connect-src 'none'` 托管，
/// 拿不到面板 Cookie 也发不出网。
pub async fn detail(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let mut plugin = plugin_view(&state, &rec)?;
    plugin.running = state.supervisor().is_running(&plugin.id).await;
    if plugin.ui_path.is_empty() {
        return Err(AppError::NotFound(format!(
            "插件 {id} 的 manifest 未声明 ui 入口"
        )));
    }
    let nonce = skiff_identity::new_token()
        .replace(['-', '_'], "")
        .chars()
        .take(24)
        .collect::<String>();
    // 跨插件跳转的定位建议（`?open=`）：只做长度与控制字符清洗，随后原样交给
    // 目标插件——真正能不能读，仍由目标插件自己的 `path:` 授权根判定。
    let open = sanitize_open(q.get("open").map(String::as_str).unwrap_or(""));
    // 菜单深链：`?menu=<local|plugin/<id>/<local>>` 选中 L2，`?l3=<local>` 选中 L3。
    // 老包没有 `[[menu]]` 时只认 L2 定位串（保持 P1 行为），不再拼 L3。
    let manifest = Manifest::parse(&rec.manifest).ok();
    let menu_raw = q.get("menu").map(String::as_str).unwrap_or("");
    let l3_raw = q.get("l3").map(String::as_str);
    let mut target_ui = plugin.ui_path.clone();
    let mut active = format!("plugin:{}", plugin.id);
    let mut workbench_title = plugin.ui_label.clone();
    let mut menu_key = String::new();
    let mut active_l3 = String::new();
    let mut active_tab = String::new();
    let mut tabs: Vec<TabView> = Vec::new();
    if !menu_raw.trim().is_empty() {
        let local = crate::menu::menu_local(&plugin.id, menu_raw)
            .ok_or_else(|| AppError::NotFound(format!("未知菜单定位: {menu_raw}")))?;
        let declared = manifest
            .as_ref()
            .map(|m| !m.menu.is_empty())
            .unwrap_or(false);
        if declared {
            // `legacy` 是 P1 老深链：插件升级为声明式后落到首个可见 L2。
            let wb = crate::menu::workbench_tabs(&state, &user, &plugin.id, &local, l3_raw)
                .or_else(|| {
                    if local != "legacy" {
                        return None;
                    }
                    manifest.as_ref()?.menu.iter().find_map(|d| {
                        crate::menu::workbench_tabs(&state, &user, &plugin.id, &d.id, l3_raw)
                    })
                })
                .ok_or_else(|| {
                    AppError::NotFound(format!("菜单不存在或当前角色不可见: {local}"))
                })?;
            target_ui = wb.active_ui.clone();
            active = wb.active_key.clone();
            workbench_title = wb.l2_label.clone();
            menu_key = wb.l2_key.clone();
            active_l3 = wb.active_local.clone();
            active_tab = wb.active_tab.clone();
            tabs = wb
                .items
                .iter()
                .map(|t| TabView {
                    key: t.key.clone(),
                    local: t.local.clone(),
                    label: t.label.clone(),
                    href: t.href.clone(),
                    icon: t.icon.clone(),
                    tab: t.tab.clone(),
                    ui: t.ui.clone(),
                    src: iframe_url(
                        &state.cfg.entry_prefix,
                        &plugin.id,
                        &t.ui,
                        &nonce,
                        &wb.l2_key,
                        &t.tab,
                        "",
                    ),
                    active: t.local == wb.active_local,
                })
                .collect();
        } else {
            // 老包：`[[ui]]` 单入口，定位串只用于保持 P1 深链。
            active = format!("plugin/{}/{}", plugin.id, local);
        }
    }
    let iframe_src = iframe_url(
        &state.cfg.entry_prefix,
        &plugin.id,
        &target_ui,
        &nonce,
        &menu_key,
        &active_tab,
        &open,
    );
    // 页面上下文顺带算出顶栏插件菜单；同一份「已启用且有 UI 入口」清单
    // 也直接当 iframe 跳转白名单用（避免为白名单再扫一遍插件表）。
    let ctx = handlers::page_ctx(&state, &user, &active);
    let nav_plugins = ctx
        .nav_plugins
        .iter()
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let rpc_url = format!("/{}/plugins/{}/rpc", state.cfg.entry_prefix, plugin.id);
    let grants = rec
        .grants
        .iter()
        .map(|g| GrantRow {
            kind: g.kind.clone(),
            value: g.value.clone(),
        })
        .collect();
    let (interop_provides, interop_consumes) = Manifest::parse(&rec.manifest)
        .map(|m| interop_rows(&m))
        .unwrap_or_default();
    let page = PluginDetailPage {
        ctx,
        plugin,
        iframe_src,
        rpc_url,
        nonce,
        nav_plugins,
        workbench_title,
        tabs,
        menu_key,
        active_l3,
        active_tab,
        active_ui: target_ui,
        interop_provides,
        interop_consumes,
        grants,
        can_admin: user.can(skiff_model::Permission::PluginManage),
        error: String::new(),
    };
    handlers::render(page)
}

/// 组装插件 iframe URL：统一带 nonce；有 L2/L3 时补 `menu`（完整稳定键）和 `tab`。
fn iframe_url(
    prefix: &str,
    id: &str,
    ui: &str,
    nonce: &str,
    menu_key: &str,
    tab: &str,
    open: &str,
) -> String {
    let mut url = format!("/{prefix}/plugins/{id}/ui/{ui}?nonce={nonce}");
    if !menu_key.is_empty() {
        url.push_str("&menu=");
        url.push_str(menu_key);
    }
    if !tab.is_empty() {
        url.push_str("&tab=");
        url.push_str(tab);
    }
    if !open.is_empty() {
        url.push_str("&open=");
        url.push_str(&crate::util::encode_uri_component(open));
    }
    url
}

/// 清洗跨插件跳转的定位串：去掉控制字符（含换行/制表符，避免被拼进头部或
/// 日志时出岔子），并按字符数截到 512。空串表示「不定位，打开默认目录」。
fn sanitize_open(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(OPEN_PATH_MAX)
        .collect()
}

/// 第一步：上传包 → 校验（结构/签名/API/能力）→ 权限确认页。
pub async fn install(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
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

    // 对象授权 = 声明的 resource_hints + 由 `[interop].consumes` 派生的 `plugin:`。
    // 后者必须**显式勾选**：插件间调用不能因为「装了个插件」就顺带获得调用权。
    let mut resources = meta.manifest.resource_hints.clone();
    resources.extend(meta.manifest.interop_resource_hints());
    resources.sort();
    resources.dedup();
    let (interop_provides, interop_consumes) = interop_rows(&meta.manifest);

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
        resources,
        interop_provides,
        interop_consumes,
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
    skiff_plugin::resolve_trust(path, state.db(), state.cfg.allow_unsigned).map_err(|e| match e {
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
    PluginManagerUser(user): PluginManagerUser,
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
    let granted = form.get("grant").cloned().unwrap_or_default();

    // 对象授权：manifest 声明的 resource_hints 勾选值 + 管理员手填（每行/逗号一条）。
    // 逐条做形式校验，坏值在确认页就报错，别等到装完才在能力调用上 403。
    let mut objects: Vec<String> = form.get("resource").cloned().unwrap_or_default();
    for part in form_first(&form, "extra_resource")
        .split(['\n', '\r', ',', ' ', '\t'])
        .map(str::trim)
    {
        if !part.is_empty() {
            objects.push(part.to_string());
        }
    }
    objects.sort();
    objects.dedup();
    for o in &objects {
        skiff_pkgfmt::manifest::validate_resource(o)
            .map_err(|e| AppError::BadRequest(format!("对象授权 {o} 非法：{e}")))?;
    }

    let opts = skiff_plugin::install_options(
        &trust,
        granted.clone(),
        objects.clone(),
        state.cfg.allow_unsigned,
        state.cfg.form,
        state.cfg.allow_reduced_sandbox,
    );

    let result = skiff_plugin::install_package(state.db(), &state.cfg.data_dir, &path, &opts);
    let _ = std::fs::remove_file(&path);
    let ip = handlers::client_ip(&state, &parts);
    match &result {
        Ok(info) => {
            let _ = state.db().audit(
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
            let _ = state.db().audit(
                &user.user.username,
                &ip,
                "plugin.install",
                &trust.meta.manifest.id,
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
    PluginManagerUser(user): PluginManagerUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let enable = !rec.enabled;
    if !enable {
        state.supervisor().stop(&id).await;
    }
    state.db().set_plugin_enabled(&id, enable)?;
    state
        .db()
        .set_plugin_status(&id, if enable { "installed" } else { "disabled" }, "")?;
    let _ = state.db().audit(
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
    PluginManagerUser(user): PluginManagerUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, AppError> {
    let form = parse_form(&body);
    check_csrf(&state, &user, &form_first(&form, "csrf"))?;
    let delete_data = form_first(&form, "delete_data") == "on";
    state.supervisor().stop(&id).await;
    skiff_plugin::uninstall_package(state.db(), &state.cfg.data_dir, &id, delete_data)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    // 契约 §2.6：卸载后清掉所有用户偏好里该插件的顺序/隐藏/固定残留。
    crate::menu::cleanup_plugin_prefs(state.db(), &id);
    let _ = state.db().audit(
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
    PluginManagerUser(user): PluginManagerUser,
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
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    if !rec.enabled {
        return Err(AppError::BadRequest("插件已停用".into()));
    }
    let result = state.supervisor().call(&rec, &method, params).await;
    let _ = state.db().audit(
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
        .db()
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
    if skiff_model::has_parent_segment(&rel) {
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
    // `entry_guard` 默认给所有响应加 `X-Frame-Options: DENY`（这里只覆盖已存在的），
    // 插件 UI 恰恰要嵌进面板详情页的 iframe，所以显式改成 SAMEORIGIN。
    resp.headers_mut()
        .insert(X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    resp.headers_mut()
        .insert(CONTENT_SECURITY_POLICY, PLUGIN_UI_CSP.parse().unwrap());
    Ok(resp)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 控制字符一律清掉：定位串会被拼进 iframe URL 与审计日志，
    /// 换行/制表符没有任何正当用途。
    #[test]
    fn sanitize_open_strips_control_chars() {
        assert_eq!(sanitize_open("/var/www\n/a\rb\tc"), "/var/www/abc");
        assert_eq!(sanitize_open("\u{0}/etc/passwd"), "/etc/passwd");
        assert!(sanitize_open("").is_empty());
        assert!(sanitize_open("\n\t").is_empty());
    }

    /// 超长串按字符（不是字节）截断，避免非 ASCII 路径被切出半个字。
    #[test]
    fn sanitize_open_truncates_to_char_limit() {
        let long = "a".repeat(OPEN_PATH_MAX + 100);
        assert_eq!(sanitize_open(&long).chars().count(), OPEN_PATH_MAX);
        let cjk = "文".repeat(OPEN_PATH_MAX + 100);
        assert_eq!(sanitize_open(&cjk).chars().count(), OPEN_PATH_MAX);
        // 边界：正好等于上限时原样保留
        let exact = "a".repeat(OPEN_PATH_MAX);
        assert_eq!(sanitize_open(&exact), exact);
    }

    /// 插件 UI 只能加载随包同源资源：脚本/图片/连接都不能出现外链与通配。
    #[test]
    fn plugin_ui_csp_forbids_external_resources() {
        assert!(
            PLUGIN_UI_CSP.contains("default-src 'none'"),
            "缺省必须全部拒绝"
        );
        assert!(
            PLUGIN_UI_CSP.contains("script-src 'self'"),
            "脚本只能来自插件包自身（禁止内联与 CDN）"
        );
        assert!(
            PLUGIN_UI_CSP.contains("connect-src 'none'"),
            "插件 UI 不得发出网络请求"
        );
        assert!(
            !PLUGIN_UI_CSP.contains("'unsafe-eval'")
                && !PLUGIN_UI_CSP.contains("script-src *")
                && !PLUGIN_UI_CSP.contains("https:"),
            "不得放开 eval 或任意来源脚本: {PLUGIN_UI_CSP}"
        );
    }
}
