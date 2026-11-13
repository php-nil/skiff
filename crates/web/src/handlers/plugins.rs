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
    GrantRow, PermissionRow, PluginConfirmPage, PluginInfoPage, PluginView, PluginViewPage,
    PluginsPage, TabView,
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
pub(crate) fn plugin_view(state: &AppState, p: &PluginRecord) -> Result<PluginView, AppError> {
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
        key_id_full: p.key_id.clone(),
        grants: p
            .grants
            .iter()
            .map(|g| format!("{}:{}", g.kind, g.value))
            .collect::<Vec<_>>()
            .join(", "),
        requires: manifest
            .as_ref()
            .map(|m| m.requires_str().join(", "))
            .unwrap_or_default(),
        permissions: manifest
            .as_ref()
            .map(|m| m.permission_strings().join(", "))
            .unwrap_or_default(),
        signed,
        reduced,
        running: false,
        ui_path: ui.map(|u| u.path.clone()).unwrap_or_default(),
        takedown_note: {
            // 已装版本被撤包时给出明确升级指向：有可安装新版本就报版本号，
            // 没有就说明当前无可装版本（不把用户留到点「安装」才被拒）。
            let mut note = state
                .catalog()
                .cached_takedown(&p.id, &p.version)
                .unwrap_or_default();
            if !note.is_empty() {
                match state.catalog().cached_installable_version(&p.id) {
                    Some(v) if skiff_plugin::install::version_gt(&v, &p.version) => {
                        note.push_str(&format!("；请升级到 {v}"))
                    }
                    _ => note.push_str("；当前没有可安装的新版本"),
                }
            }
            note
        },
    })
}

pub(crate) fn plugin_views(state: &AppState) -> Result<Vec<PluginView>, AppError> {
    state
        .db()
        .list_plugins()?
        .iter()
        .map(|p| plugin_view(state, p))
        .collect()
}

/// manifest 里的插件间接口：`(对外提供的 method · label, 允许调用的目标列表)`。
pub(crate) fn interop_rows(manifest: &Manifest) -> (Vec<String>, Vec<String>) {
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

/// 插件信息与管理页（`/{p}/plugins/{id}`）：内核渲染插件身份、授权、
/// 菜单关联与管理动作，不嵌 iframe（功能界面见 `/{p}/plugins/{id}/<view>`）。
pub async fn info(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let mut plugin = plugin_view(&state, &rec)?;
    plugin.running = state.supervisor().is_running(&plugin.id).await;
    let manifest = Manifest::parse(&rec.manifest).ok();
    let (menus, entry_href) = manifest
        .as_ref()
        .map(|m| crate::menu::plugin_info_menu(&state, &user, &id, m))
        .unwrap_or_default();
    let (interop_provides, interop_consumes) =
        manifest.as_ref().map(interop_rows).unwrap_or_default();
    let grants = rec
        .grants
        .iter()
        .map(|g| GrantRow {
            kind: g.kind.clone(),
            value: g.value.clone(),
        })
        .collect();
    let page = PluginInfoPage {
        ctx: handlers::page_ctx(&state, &user, "core/plugins/installed"),
        plugin,
        grants,
        interop_provides,
        interop_consumes,
        menus,
        entry_href,
        can_admin: user.can(skiff_model::Permission::PluginManage),
        error: String::new(),
        notice: String::new(),
    };
    handlers::render(page)
}

/// 规范功能页（`/{p}/plugins/{id}/<view>`）：视图名 → 所属 L2/L3，
/// 内核渲染 L3 标签栏后把 iframe 指向 `ui/<view>.html`（未知视图 404）。
pub async fn view(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path((id, view_name)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    // 站点文件插件（`com.skiff.files`）是可选入口：未安装/未启用，或受限形态没有
    // fs 能力面时，回退到内核「文件管理」页（不 404）。插件可用时按正常视图渲染。
    if id == "com.skiff.files" {
        let usable = state
            .db()
            .plugin(&id)
            .ok()
            .flatten()
            .map(|r| r.enabled)
            .unwrap_or(false)
            && crate::middleware::check_fs_capability(&state).is_ok();
        if !usable {
            let to = format!("/{}/files", state.cfg.entry_prefix);
            let mut resp = axum::response::Response::new(axum::body::Body::empty());
            *resp.status_mut() = axum::http::StatusCode::FOUND;
            if let Ok(value) = axum::http::HeaderValue::from_str(&to) {
                resp.headers_mut()
                    .insert(axum::http::header::LOCATION, value);
            }
            return Ok(resp);
        }
    }
    // 新一代前端（budding 壳）不承载 iframe：任何直接导航都交壳页渲染
    // 「插件功能页正在切换为原生渲染」占位。旧 SSR 工作台仅保留给显式兼容请求
    // （`?legacy=1`，例如仍需嵌入的旧页面），面板内没有任何导航入口。
    //
    // 不做 `Sec-Fetch-Dest` 判断：真实浏览器经反向代理/安全网关后该头可能被剥离，
    // 会导致直达被误判成嵌入请求。显式参数不受代理影响。
    if !q.get("legacy").map(|v| v == "1").unwrap_or(false) {
        return Ok(crate::handlers::spa::shell_page(&state));
    }
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let wb = crate::menu::resolve_plugin_view(&state, &user, &id, &view_name).ok_or_else(|| {
        AppError::NotFound(format!(
            "插件 {id} 不存在视图 {view_name}，或该视图对当前角色不可见"
        ))
    })?;
    render_workbench(&state, &user, &rec, wb, q.get("open").map(String::as_str)).await
}

/// 菜单深链（`/{p}/m/plugin/<id>[/<l2>[/<l3>]]`）。
///
/// `legacy` 是 P1 老深链：插件升级为声明式菜单后落到首个可见 L2，
/// 避免旧收藏与旧入口失效。
pub async fn menu_view(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path((id, l2_raw, l3_raw)): Path<(String, Option<String>, Option<String>)>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let wb = match l2_raw.as_deref() {
        None => crate::menu::default_plugin_view(&state, &user, &id),
        Some(raw) => {
            let resolved = crate::menu::workbench_tabs(&state, &user, &id, raw, l3_raw.as_deref());
            if resolved.is_some() {
                resolved
            } else if crate::menu::menu_local(&id, raw).as_deref() == Some("legacy") {
                // 老深链降级：按 manifest 顺序尝试首个可解析的可见 L2。
                Manifest::parse(&rec.manifest).ok().and_then(|m| {
                    m.menu.iter().find_map(|d| {
                        crate::menu::workbench_tabs(&state, &user, &id, &d.id, l3_raw.as_deref())
                    })
                })
            } else {
                None
            }
        }
    }
    .ok_or_else(|| {
        AppError::NotFound(format!(
            "插件 {id} 的菜单不存在或当前角色不可见: {}",
            l2_raw.as_deref().unwrap_or("")
        ))
    })?;
    render_workbench(&state, &user, &rec, wb, q.get("open").map(String::as_str)).await
}

/// 工作台公共渲染：nonce 每次打开重新生成，父页面只代发带有该 nonce 的
/// `skiff.rpc` 消息；iframe 内容按 `style-src 'self' 'unsafe-inline'`、
/// `connect-src 'none'` 托管，拿不到面板 Cookie 也发不出网。
async fn render_workbench(
    state: &AppState,
    user: &crate::state::CurrentUser,
    rec: &PluginRecord,
    wb: crate::menu::WorkbenchTabs,
    open_raw: Option<&str>,
) -> Result<Response, AppError> {
    let mut plugin = plugin_view(state, rec)?;
    plugin.running = state.supervisor().is_running(&plugin.id).await;
    if wb.active_ui.is_empty() {
        return Err(AppError::NotFound(format!(
            "插件 {} 的 manifest 未声明可用的 ui 视图",
            plugin.id
        )));
    }
    let nonce = skiff_identity::new_token()
        .replace(['-', '_'], "")
        .chars()
        .take(24)
        .collect::<String>();
    // 跨插件跳转的定位建议（`?open=`）：只做长度与控制字符清洗，随后原样交给
    // 目标插件——真正能不能读，仍由目标插件自己的 `path:` 授权根判定。
    let open = sanitize_open(open_raw.unwrap_or(""));
    let tabs = wb
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
    let iframe_src = iframe_url(
        &state.cfg.entry_prefix,
        &plugin.id,
        &wb.active_ui,
        &nonce,
        &wb.l2_key,
        &wb.active_tab,
        &open,
    );
    // 页面上下文顺带算出顶栏插件菜单；同一份「已启用且有 UI 入口」清单
    // 也直接当 iframe 跳转白名单用（避免为白名单再扫一遍插件表）。
    let ctx = handlers::page_ctx(state, user, &wb.l2_key);
    let nav_plugins = ctx
        .nav_plugins
        .iter()
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let rpc_url = format!("/{}/plugins/{}/rpc", state.cfg.entry_prefix, plugin.id);
    let page = PluginViewPage {
        ctx,
        plugin,
        info_href: format!("/{}/plugins/{}", state.cfg.entry_prefix, rec.id),
        iframe_src,
        rpc_url,
        nonce,
        nav_plugins,
        workbench_title: wb.l2_label,
        tabs,
        menu_key: wb.l2_key,
        active_l3: wb.active_local,
        active_tab: wb.active_tab,
        active_ui: wb.active_ui,
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
    parts: Parts,
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

    let trust = verify_for_confirm(&state, &path)?;
    let meta = &trust.meta;

    // 重装/升级：按同一插件 id 找回既有授权，直接完成安装，不再逐条弹确认页。
    // 选择集传空表示「不新增人工授权」；install_package 沿用库内 grants 并按新
    // manifest 的声明集过滤，相对旧 manifest 新增声明的权限由它补齐（敏感档写审计）。
    if state.db().plugin(&meta.manifest.id)?.is_some() {
        let opts = skiff_plugin::install_options(
            &trust,
            Vec::new(),
            Vec::new(),
            state.cfg.allow_unsigned,
            state.cfg.form,
            state.cfg.allow_reduced_sandbox,
            &user.user.username,
        );
        let ip = handlers::client_ip(&state, &parts);
        commit_install(&state, &user, &ip, &path, &opts, &meta.manifest.id).await?;
        return Ok(handlers::redirect(format!(
            "/{}/plugins",
            state.cfg.entry_prefix
        )));
    }

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

pub(crate) fn verify_for_confirm(
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

/// 安装包并统一留痕：成功/失败都写 `plugin.install` 审计，并清理 `.incoming` 暂存。
///
/// 首次安装（确认页提交）与重装/升级（跳过确认页）共用这一条收尾路径，
/// 避免两个入口的审计字段、临时文件清理和错误话术各写一遍。
pub(crate) async fn commit_install(
    state: &AppState,
    user: &crate::state::CurrentUser,
    ip: &str,
    path: &FsPath,
    opts: &skiff_plugin::InstallOptions,
    fail_id: &str,
) -> Result<(), AppError> {
    let result = skiff_plugin::install_package(state.db(), &state.cfg.data_dir, path, opts);
    let _ = std::fs::remove_file(path);
    if result.is_ok() {
        // manifest `[[task]]` 声明的内核任务随安装即时种入，不要求重启。
        if let Some(tasks) = state.tasks() {
            tasks.sync_manifest_tasks().await;
        }
    }
    match &result {
        Ok(info) => {
            state.audit_or_log(
                &user.user.username,
                ip,
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
            state.audit_or_log(
                &user.user.username,
                ip,
                "plugin.install",
                fail_id,
                "fail",
                &e.to_string(),
            );
        }
    }
    result
        .map(|_| ())
        .map_err(|e| AppError::BadRequest(e.to_string()))
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
        &user.user.username,
    );

    let ip = handlers::client_ip(&state, &parts);
    commit_install(&state, &user, &ip, &path, &opts, &trust.meta.manifest.id).await?;
    Ok(handlers::redirect(format!(
        "/{}/plugins",
        state.cfg.entry_prefix
    )))
}

/// 启停插件的共用实现：停用先停进程，再落库并审计；返回新状态。
pub(crate) async fn apply_toggle(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    id: &str,
    ip: &str,
) -> Result<bool, AppError> {
    let rec = state
        .db()
        .plugin(id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    let enable = !rec.enabled;
    if !enable {
        state.supervisor().stop(id).await;
    }
    state
        .set_plugin_enabled(id, enable, &user.user.username, ip)
        .map_err(AppError::Internal)?;
    Ok(enable)
}

/// 卸载插件的共用实现：停进程 → 删包/数据 → 回收任务与菜单偏好 → 审计。
pub(crate) async fn apply_remove(
    state: &Arc<AppState>,
    user: &crate::state::CurrentUser,
    id: &str,
    delete_data: bool,
    ip: &str,
) -> Result<(), AppError> {
    state.supervisor().stop(id).await;
    skiff_plugin::uninstall_package(state.db(), &state.cfg.data_dir, id, delete_data)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    // 卸载即回收该插件名下的内核任务；否则 source=plugin:<id> 的任务会指向不存在的插件。
    if let Some(tasks) = state.tasks() {
        tasks.drop_plugin_tasks(id).await;
    }
    // 契约 §2.6：卸载后清掉所有用户偏好里该插件的顺序/隐藏/固定残留。
    crate::menu::cleanup_plugin_prefs(state.db(), id);
    state.audit_or_log(
        &user.user.username,
        ip,
        "plugin.uninstall",
        id,
        "ok",
        &format!("delete_data={delete_data}"),
    );
    Ok(())
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
    let ip = handlers::client_ip(&state, &parts);
    apply_toggle(&state, &user, &id, &ip).await?;
    // 信息页内操作回信息页；插件中心内操作仍回列表（`next` 只接受本站前缀）。
    Ok(handlers::redirect(
        safe_plugin_next(&state, form_first(&form, "next"))
            .unwrap_or_else(|| format!("/{}/plugins", state.cfg.entry_prefix)),
    ))
}

fn safe_plugin_next(state: &AppState, raw: String) -> Option<String> {
    let expected = format!("/{}/plugins/", state.cfg.entry_prefix);
    if raw.starts_with(&expected) && !raw.contains("..") && !raw.contains("//") {
        Some(raw)
    } else {
        None
    }
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
    let ip = handlers::client_ip(&state, &parts);
    apply_remove(&state, &user, &id, delete_data, &ip).await?;
    Ok(handlers::redirect(format!(
        "/{}/plugins",
        state.cfg.entry_prefix
    )))
}

/// 从插件错误链里提取内核能力面返回的 HTTP 状态码。
///
/// 插件 SDK 把内核非 200 响应折叠成 `协议错误: 内核返回 HTTP <code>: <detail>`，
/// 再经 JSON-RPC `-32603` 与 Supervisor 包装后只剩字符串。这里只认这一固定片段，
/// 避免把插件业务文案里出现的数字误判成状态码。
fn kernel_http_status(message: &str) -> Option<u16> {
    let (_, tail) = message.split_once("内核返回 HTTP ")?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// RPC 错误→HTTP 状态：内核面已有的状态语义原样透传，未知状态才归 400。
///
/// 逐个语义对齐而不是「只挑几个特例」：`fail2ban` 缺失等依赖类失败在内核面是
/// 503，代理层若压成 400，用户会看到「400 请求非法 + 正文说是 503」的自相矛盾。
/// 同理 500 是内核内部错误（不是客户端输入问题）、401 是未认证、504 是下游超时。
///
/// 文案保持内核 detail 原样，不在代理层追加授权提示——是否补提示由插件按
/// 具体错误语义决定，避免「内置任务保护」被误报成权限不足。
fn rpc_error(message: String) -> AppError {
    match kernel_http_status(&message) {
        Some(401) => AppError::Unauthorized(message),
        Some(403) => AppError::Forbidden(message),
        Some(404) => AppError::NotFound(message),
        Some(409) => AppError::Conflict(message),
        Some(500) => AppError::Internal(message),
        Some(507) => AppError::NoSpace(message),
        Some(503) => AppError::Unavailable(message),
        Some(504) => AppError::Timeout(message),
        _ => AppError::BadRequest(message),
    }
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
    state.audit_or_log(
        &user.user.username,
        &handlers::client_ip(&state, &parts),
        "plugin.rpc",
        &id,
        if result.is_ok() { "ok" } else { "fail" },
        &format!("method={method}"),
    );
    match result {
        Ok(v) => Ok(axum::Json(serde_json::json!({ "result": v })).into_response()),
        Err(e) => Err(rpc_error(e.to_string())),
    }
}

/// 内核统一抑制插件自带的页面级标题。
///
/// 插件功能页的标题（插件名 + 视图名）由父页面渲染；插件不得重复。第三方插件
/// 若必须保留自己的标题，用 `data-plugin-title` 标记，内核在托管 HTML 时注入这条
/// 规则把它隐藏——不做「隐藏第一个 h1」这类启发式，避免误伤区块小标题。
const PLUGIN_TITLE_GUARD: &str =
    "<style data-kernel-title-guard>[data-plugin-title]{display:none!important}</style>";

/// 在 HTML 的 `</head>` 前注入标题抑制规则；没有 `</head>` 时前置（由浏览器归位）。
fn inject_title_guard(html: &[u8]) -> Vec<u8> {
    let lower = html.to_ascii_lowercase();
    let at = lower
        .windows(b"</head>".len())
        .position(|w| w == b"</head>")
        .unwrap_or(0);
    let mut out = Vec::with_capacity(html.len() + PLUGIN_TITLE_GUARD.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(PLUGIN_TITLE_GUARD.as_bytes());
    out.extend_from_slice(&html[at..]);
    out
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
    let mut data = std::fs::read(&canon).map_err(|e| AppError::Internal(e.to_string()))?;
    let mime = mime_guess::from_path(&canon).first_or_octet_stream();
    if mime == "text/html" {
        data = inject_title_guard(&data);
    }
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
/// 托管插件页面 Schema：`/plugins/{id}/schema/{view}`。
///
/// 权限口径与 `POST /plugins/{id}/rpc` **完全一致**（`PluginManagerUser`）：
/// 能取到 schema 的人，才有资格让壳页代发 RPC。旧 HTML 视图没有 schema，
/// 这里返回 404，由旧 SSR（`/m/plugin/...`）继续承载。
pub async fn schema(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path((id, view_name)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    if !rec.enabled {
        return Err(AppError::NotFound("插件已停用".into()));
    }
    // 视图必须属于该插件且对当前角色可见（与菜单/工作台同源判定）。
    crate::menu::resolve_plugin_view(&state, &user, &id, &view_name).ok_or_else(|| {
        AppError::NotFound(format!(
            "插件 {id} 不存在视图 {view_name}，或该视图对当前角色不可见"
        ))
    })?;
    let manifest = Manifest::parse(&rec.manifest)
        .map_err(|e| AppError::Internal(format!("插件 manifest 非法: {e}")))?;
    let Some(name) = manifest.schema_for_view(&view_name).map(str::to_string) else {
        return Err(AppError::NotFound(format!(
            "视图 {view_name} 仍为旧 HTML 界面，没有随包 schema"
        )));
    };
    let root = state
        .cfg
        .data_dir
        .join("plugins")
        .join(&rec.id)
        .join("versions")
        .join(&rec.active_version)
        .join("ui")
        .join("schema");
    let target = root.join(&name);
    let canon_root = std::fs::canonicalize(&root)
        .map_err(|_| AppError::NotFound("该插件没有 schema 目录".into()))?;
    let canon = std::fs::canonicalize(&target)
        .map_err(|_| AppError::NotFound(format!("schema 不存在: {name}")))?;
    if !canon.starts_with(&canon_root) || !canon.is_file() {
        return Err(AppError::NotFound(format!("schema 不存在: {name}")));
    }
    let data = std::fs::read(&canon).map_err(|e| AppError::Internal(e.to_string()))?;
    // 安装期已校验；这里再跑一次，防止运行期文件被替换成未校验内容。
    skiff_pkgfmt::PluginSchema::parse(&data)
        .map_err(|e| AppError::Internal(format!("schema 校验失败: {e}")))?;
    let mut resp = Response::new(axum::body::Body::from(data));
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
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

    /// 页面级标题由内核提供：残留的自标记标题必须在 iframe 内被统一隐藏。
    #[test]
    fn title_guard_is_injected_into_plugin_html() {
        let html =
            b"<html><head><title>x</title></head><body><h1 data-plugin-title>x</h1></body></html>";
        let out = inject_title_guard(html);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("data-kernel-title-guard"));
        assert!(text.contains("[data-plugin-title]{display:none!important}"));
        assert!(
            text.find("data-kernel-title-guard").unwrap() < text.find("</head>").unwrap(),
            "规则必须在 </head> 前生效: {text}"
        );
    }

    /// 非标准 HTML 也不能破坏文档：没有 head 时前置，内容一个字节不改。
    #[test]
    fn title_guard_prepends_without_head() {
        let html = b"<body><h1 data-plugin-title>x</h1></body>";
        let out = inject_title_guard(html);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("<style data-kernel-title-guard>"));
        assert!(text.ends_with("<body><h1 data-plugin-title>x</h1></body>"));
    }

    /// RPC 代理透传内核状态码；文案保持内核 detail 原样，不追加授权提示。
    #[test]
    fn rpc_error_passes_kernel_status_through() {
        let forbidden =
            "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 403: 内置任务不可删除（只能停用）";
        match rpc_error(forbidden.to_string()) {
            AppError::Forbidden(m) => assert_eq!(m, forbidden, "403 文案必须原样保留"),
            other => panic!("内核 403 应映射 HTTP 403，实际 {other:?}"),
        }

        let not_found = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 404: 任务不存在";
        match rpc_error(not_found.to_string()) {
            AppError::NotFound(m) => assert_eq!(m, not_found),
            other => panic!("内核 404 应映射 HTTP 404，实际 {other:?}"),
        }

        let conflict = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 409: 任务已在运行";
        match rpc_error(conflict.to_string()) {
            AppError::Conflict(m) => assert_eq!(m, conflict),
            other => panic!("内核 409 应映射 HTTP 409，实际 {other:?}"),
        }

        let no_space = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 507: 文件系统配额已用尽";
        match rpc_error(no_space.to_string()) {
            AppError::NoSpace(m) => assert_eq!(m, no_space, "507 文案必须原样保留"),
            other => panic!("内核 507 应映射 HTTP 507，实际 {other:?}"),
        }

        // 依赖类失败（如 fail2ban 未安装）内核面是 503，代理层必须同口径，
        // 否则会出现「400 请求非法 + 正文说 503」的自相矛盾。
        let unavailable = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 503: fail2ban 未安装";
        match rpc_error(unavailable.to_string()) {
            AppError::Unavailable(m) => assert_eq!(m, unavailable, "503 文案必须原样保留"),
            other => panic!("内核 503 应映射 HTTP 503，实际 {other:?}"),
        }

        let internal = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 500: 内核内部错误";
        match rpc_error(internal.to_string()) {
            AppError::Internal(m) => assert_eq!(m, internal, "500 文案必须原样保留"),
            other => panic!("内核 500 应映射 HTTP 500，实际 {other:?}"),
        }

        let unauthorized = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 401: 未认证";
        match rpc_error(unauthorized.to_string()) {
            AppError::Unauthorized(m) => assert_eq!(m, unauthorized),
            other => panic!("内核 401 应映射 HTTP 401，实际 {other:?}"),
        }

        let timeout = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 504: 等待特权命令超时";
        match rpc_error(timeout.to_string()) {
            AppError::Timeout(m) => assert_eq!(m, timeout),
            other => panic!("内核 504 应映射 HTTP 504，实际 {other:?}"),
        }

        // 未识别的状态与普通插件业务错误仍归 400，并保留原文。
        let unknown = "IPC: 插件返回错误 -32603: 协议错误: 内核返回 HTTP 418: 未知语义";
        assert!(matches!(rpc_error(unknown.to_string()), AppError::BadRequest(m) if m == unknown));
        let business = "IPC: 插件返回错误 -32603: 任务名不能为空";
        assert!(
            matches!(rpc_error(business.to_string()), AppError::BadRequest(m) if m == business),
            "普通业务错误应保持 400 且文案不变"
        );
    }

    /// 只认 `内核返回 HTTP <code>` 这一段，业务文案里的数字不算状态码。
    #[test]
    fn kernel_status_is_parsed_conservatively() {
        assert_eq!(kernel_http_status("内核返回 HTTP 403: x"), Some(403));
        assert_eq!(kernel_http_status("HTTP 403 与业务错误"), None);
        assert_eq!(kernel_http_status("内核返回 HTTP abc: x"), None);
        assert_eq!(kernel_http_status("端口 8080 被占用"), None);
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

    /// SDUI 切换前的护栏（D12）：插件 UI 的独立 CSP 逐字冻结，不得放宽。
    #[test]
    fn plugin_ui_csp_is_frozen() {
        const EXPECTED: &str = "default-src 'none'; script-src 'self'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; \
             form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'self'";
        assert_eq!(PLUGIN_UI_CSP, EXPECTED);
        assert!(PLUGIN_UI_CSP.contains("frame-ancestors 'self'"));
        assert!(!PLUGIN_UI_CSP.contains("'unsafe-eval'"));
        assert!(!PLUGIN_UI_CSP.contains('*'));
        assert!(!PLUGIN_UI_CSP.contains("http:") && !PLUGIN_UI_CSP.contains("https:"));
    }

    /// 插件 iframe 的沙箱属性与「不加 allow-same-origin」的边界不得改动。
    #[test]
    fn plugin_iframe_sandbox_is_frozen() {
        const SANDBOX: &str = r#"sandbox="allow-scripts allow-modals allow-forms""#;
        for tpl in [
            include_str!("../../templates/plugin_view.html"),
            include_str!("../../templates/mount_card.html"),
        ] {
            assert!(tpl.contains(SANDBOX), "插件 iframe 沙箱属性被改动");
            assert!(
                !tpl.contains("allow-same-origin"),
                "插件 iframe 不得加 allow-same-origin"
            );
        }
    }
}
