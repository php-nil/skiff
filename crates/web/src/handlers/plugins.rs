use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_FRAME_OPTIONS};
use axum::http::request::Parts;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};

use skiff_pkgfmt::Manifest;
use skiff_plugin::ipc::IpcError;
use skiff_plugin::supervisor::SupervisorError;
use skiff_store::PluginRecord;

use crate::error::{ApiError, AppError};
use crate::handlers;
use crate::middleware::PluginManagerUser;
use crate::templates::{PluginView, PluginViewPage, TabView};
use skiff_app::AppState;

/// `?open=` 定位串的字符上限：够放一条深层路径，又不至于把 URL 撑爆。
const OPEN_PATH_MAX: usize = 512;

/// 插件 UI 的 CSP：脚本/样式只能来自插件包自身，网络请求全禁。
/// 抽成常量是为了让契约测试能直接断言「没有放开外链」。
const PLUGIN_UI_CSP: &str =
    "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; \
     object-src 'none'; frame-ancestors 'self'";

/// 一条插件记录 → 工作台需要的少量视图字段（其余管理信息走 `/ui/v1` 的 app 读模型）。
pub(crate) fn plugin_view(state: &AppState, p: &PluginRecord) -> Result<PluginView, AppError> {
    let r = skiff_app::views::plugins::plugin_row(state, p)?;
    Ok(PluginView {
        id: r.id,
        name: r.name,
        running: r.running,
    })
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

/// 规范功能页（`/{p}/plugins/{id}/<view>`）：视图名 → 所属 L2/L3，
/// 内核渲染 L3 标签栏后把 iframe 指向 `ui/<view>.html`（未知视图 404）。
pub async fn view(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path((id, view_name)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    // 默认交付 budding 壳页：壳页优先读随包 schema 原生渲染。
    // `?legacy=1` 是**没有 schema 的旧视图**（如 monitor/health、testkit/mount）的
    // iframe 宿主入口——壳页在 schema 404 时自动加这个参数回跳，属于现役路径。
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
    let wb =
        skiff_app::menu::resolve_plugin_view(&state, &user, &id, &view_name).ok_or_else(|| {
            AppError::NotFound(format!(
                "插件 {id} 不存在视图 {view_name}，或该视图对当前角色不可见"
            ))
        })?;
    render_workbench(&state, &user, &rec, wb, q.get("open").map(String::as_str)).await
}
/// 工作台公共渲染：nonce 每次打开重新生成，父页面只代发带有该 nonce 的
/// `skiff.rpc` 消息；iframe 内容按 `style-src 'self' 'unsafe-inline'`、
/// `connect-src 'none'` 托管，拿不到面板 Cookie 也发不出网。
async fn render_workbench(
    state: &AppState,
    user: &skiff_app::CurrentUser,
    rec: &PluginRecord,
    wb: skiff_app::menu::WorkbenchTabs,
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
pub(crate) fn verify_for_confirm(
    state: &AppState,
    path: &FsPath,
) -> Result<skiff_plugin::TrustDecision, AppError> {
    // 判定逻辑与 CLI 完全同一份；这里只把「未导入公钥」翻成页面里的话术。
    let trust = skiff_plugin::resolve_trust(path, state.db(), state.cfg.allow_unsigned).map_err(
        |e| match e {
            skiff_plugin::TrustError::UnverifiedKey { key_id } => AppError::BadRequest(format!(
                "未导入该包的公钥（指纹 {key_id}）。请先在「设置 → 信任公钥」导入，再安装；\
                     未签名包默认拒装。"
            )),
            other => AppError::BadRequest(other.to_string()),
        },
    )?;
    // 直传安装与商店安装共用同一兼容判定；不兼容的包在确认页之前就拒绝，
    // 避免用户走完授权页才看到失败。
    let api = &trust.meta.manifest.api;
    let kernel = skiff_app::tasks::KERNEL_API;
    if !skiff_pkgfmt::manifest::api_compatible(api, kernel) {
        return Err(AppError::PluginApi(format!(
            "插件 {id} {version} 要求内核 API {api}，当前内核为 {kernel}；\
             请升级 Skiff 或改用兼容版本",
            id = trust.meta.manifest.id,
            version = trust.meta.manifest.version,
        )));
    }
    Ok(trust)
}

/// 安装包并统一留痕：成功/失败都写 `plugin.install` 审计，并清理 `.incoming` 暂存。
///
/// 首次安装（确认页提交）与重装/升级（跳过确认页）共用这一条收尾路径，
/// 避免两个入口的审计字段、临时文件清理和错误话术各写一遍。
pub(crate) async fn commit_install(
    state: &AppState,
    user: &skiff_app::CurrentUser,
    ip: &str,
    path: &FsPath,
    opts: &skiff_plugin::InstallOptions,
    fail_id: &str,
) -> Result<(), AppError> {
    let tasks = state.tasks().cloned();
    let result = skiff_app::commands::plugin::install_with_tasks(
        state.db(),
        &state.cfg.data_dir,
        path,
        opts,
        tasks.as_ref(),
        state.admin_fs(),
    )
    .await;
    match &result {
        Ok(info) => {
            state.audit_or_log(
                &user.user.username,
                ip,
                "plugin.install",
                &info.id,
                "ok",
                &skiff_app::commands::plugin::install_detail(info),
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
/// 启停插件的共用实现：停用先停进程，再落库并审计；返回新状态。
pub(crate) async fn apply_toggle(
    state: &Arc<AppState>,
    user: &skiff_app::CurrentUser,
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
/// 卸载插件的共用实现：停进程 → 命令层收尾（删包/回收任务/菜单偏好）→ 审计。
pub(crate) async fn apply_remove(
    state: &Arc<AppState>,
    user: &skiff_app::CurrentUser,
    id: &str,
    delete_data: bool,
    ip: &str,
) -> Result<(), AppError> {
    state.supervisor().stop(id).await;
    let tasks = state.tasks().cloned();
    skiff_app::commands::plugin::remove_with_tasks(
        state.db(),
        &state.cfg.data_dir,
        id,
        delete_data,
        tasks.as_ref(),
    )
    .await
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    state.audit_or_log(
        &user.user.username,
        ip,
        "plugin.uninstall",
        id,
        "ok",
        &skiff_app::commands::plugin::uninstall_detail(delete_data),
    );
    Ok(())
}
/// RPC 错误→HTTP 状态：内核面已有的状态语义原样透传，未知状态才归 400。
///
/// 上游状态由 `skiff_wire::RpcFailure` 在协议层解析一次，这里只读结构化字段，
/// 不再对错误文案做字符串匹配。
///
/// 逐个语义对齐而不是「只挑几个特例」：`fail2ban` 缺失等依赖类失败在内核面是
/// 503，代理层若压成 400，用户会看到「400 请求非法 + 正文说是 503」的自相矛盾。
/// 同理 500 是内核内部错误（不是客户端输入问题）、401 是未认证、504 是下游超时。
///
/// 文案保持内核 detail 原样，不在代理层追加授权提示——是否补提示由插件按
/// 具体错误语义决定，避免「内置任务保护」被误报成权限不足。
fn rpc_error(e: &SupervisorError) -> AppError {
    let message = e.to_string();
    let upstream = match e {
        SupervisorError::Ipc(IpcError::Rpc(failure)) => failure.upstream_status,
        _ => None,
    };
    match upstream {
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
///
/// 成功返回 `{ result }`，失败返回统一 JSON 错误信封（`{ error: { code, message } }`）。
/// 旧 iframe 桥先按 JSON 解析、再回退 HTML 文本；新壳直接读 `error.message`，
/// 不把可读原因退化成「服务返回状态 400」。
pub async fn rpc(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(user): PluginManagerUser,
    Path(id): Path<String>,
    parts: Parts,
    body: String,
) -> Result<Response, ApiError> {
    // CSRF 对 loopback CLI 请求也 bypass——loopback 自身已经是可信面。
    let ip = handlers::client_ip(&state, &parts);
    let is_loopback = ip == "127.0.0.1" || ip == "::1";
    if !is_loopback {
        handlers::check_csrf_header(&state, &user, &parts)?;
    }
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
        .plugin(&id)
        .map_err(AppError::from)?
        .ok_or_else(|| AppError::NotFound(format!("插件不存在: {id}")))?;
    if !rec.enabled {
        return Err(AppError::BadRequest("插件已停用".into()).into());
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
        Err(e) => Err(ApiError(rpc_error(&e))),
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
///
/// 权限口径与 schema / RPC 一致（`PluginManagerUser`）：插件 UI 只由已登录的
/// 面板页面以 iframe 加载，同源子资源请求自带会话 Cookie；插件 SDK 只经父页面
/// postMessage 调 RPC，没有任何匿名加载依赖。
pub async fn plugin_ui(
    State(state): State<Arc<AppState>>,
    PluginManagerUser(_user): PluginManagerUser,
    Path((id, path)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let rec = state
        .db()
        .plugin(&id)?
        .ok_or_else(|| AppError::NotFound("插件不存在".into()))?;
    if !rec.enabled {
        return Err(AppError::NotFound("插件已停用".into()));
    }
    let root =
        skiff_plugin::install::plugin_ui_dir(&state.cfg.data_dir, &rec.id, &rec.active_version);
    let rel = path.replace('\\', "/");
    if skiff_model::has_parent_segment(&rel) {
        return Err(AppError::Forbidden("路径非法".into()));
    }
    let target = root.join(&rel);
    let canon_root =
        std::fs::canonicalize(&root).map_err(|_| AppError::NotFound("无 UI 资源".into()))?;
    let canon =
        std::fs::canonicalize(&target).map_err(|_| AppError::NotFound("资源不存在".into()))?;
    if !skiff_model::within_root(&canon_root, &canon) || !canon.is_file() {
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
/// 能取到 schema 的人，才有资格让壳页代发 RPC。没有 schema 的旧 HTML 视图
/// 这里返回 404，壳页据此带 `?legacy=1` 回落到 iframe 工作台。
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
    skiff_app::menu::resolve_plugin_view(&state, &user, &id, &view_name).ok_or_else(|| {
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
    let root =
        skiff_plugin::install::plugin_ui_dir(&state.cfg.data_dir, &rec.id, &rec.active_version)
            .join("schema");
    let target = root.join(&name);
    let canon_root = std::fs::canonicalize(&root)
        .map_err(|_| AppError::NotFound("该插件没有 schema 目录".into()))?;
    let canon = std::fs::canonicalize(&target)
        .map_err(|_| AppError::NotFound(format!("schema 不存在: {name}")))?;
    if !skiff_model::within_root(&canon_root, &canon) || !canon.is_file() {
        return Err(AppError::NotFound(format!("schema 不存在: {name}")));
    }
    let data = std::fs::read(&canon).map_err(|e| AppError::Internal(e.to_string()))?;
    // 安装期已校验；这里再跑一次，防止运行期文件被替换成未校验内容。
    // 表单提交由 `submitMode: "event"` 声明（校验期强制），内核不再注入 no-op api。
    let parsed = skiff_pkgfmt::PluginSchema::parse(&data)
        .map_err(|e| AppError::PluginSchema(format!("schema 校验失败: {e}")))?;
    let body = serde_json::to_vec(parsed.value())
        .map_err(|e| AppError::Internal(format!("schema 序列化失败: {e}")))?;
    let mut resp = Response::new(axum::body::Body::from(body));
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

    /// 构造与 Supervisor 包装后同形的结构化 RPC 错误。
    fn rpc_failure(upstream: Option<u16>, detail: &str) -> SupervisorError {
        let message = match upstream {
            Some(status) => format!("协议错误: 内核返回 HTTP {status}: {detail}"),
            None => detail.to_string(),
        };
        SupervisorError::Ipc(IpcError::Rpc(skiff_plugin::ipc::RpcFailure {
            code: -32603,
            message,
            upstream_status: upstream,
        }))
    }

    /// RPC 代理透传内核状态码；文案保持内核 detail 原样，不追加授权提示。
    #[test]
    fn rpc_error_passes_kernel_status_through() {
        let forbidden = rpc_failure(Some(403), "内置任务不可删除（只能停用）");
        match rpc_error(&forbidden) {
            AppError::Forbidden(m) => assert_eq!(m, forbidden.to_string(), "403 文案必须原样保留"),
            other => panic!("内核 403 应映射 HTTP 403，实际 {other:?}"),
        }

        let not_found = rpc_failure(Some(404), "任务不存在");
        match rpc_error(&not_found) {
            AppError::NotFound(m) => assert_eq!(m, not_found.to_string()),
            other => panic!("内核 404 应映射 HTTP 404，实际 {other:?}"),
        }

        let conflict = rpc_failure(Some(409), "任务已在运行");
        match rpc_error(&conflict) {
            AppError::Conflict(m) => assert_eq!(m, conflict.to_string()),
            other => panic!("内核 409 应映射 HTTP 409，实际 {other:?}"),
        }

        let no_space = rpc_failure(Some(507), "文件系统配额已用尽");
        match rpc_error(&no_space) {
            AppError::NoSpace(m) => assert_eq!(m, no_space.to_string(), "507 文案必须原样保留"),
            other => panic!("内核 507 应映射 HTTP 507，实际 {other:?}"),
        }

        // 依赖类失败（如 fail2ban 未安装）内核面是 503，代理层必须同口径，
        // 否则会出现「400 请求非法 + 正文说 503」的自相矛盾。
        let unavailable = rpc_failure(Some(503), "fail2ban 未安装");
        match rpc_error(&unavailable) {
            AppError::Unavailable(m) => {
                assert_eq!(m, unavailable.to_string(), "503 文案必须原样保留")
            }
            other => panic!("内核 503 应映射 HTTP 503，实际 {other:?}"),
        }

        let internal = rpc_failure(Some(500), "内核内部错误");
        match rpc_error(&internal) {
            AppError::Internal(m) => assert_eq!(m, internal.to_string(), "500 文案必须原样保留"),
            other => panic!("内核 500 应映射 HTTP 500，实际 {other:?}"),
        }

        let unauthorized = rpc_failure(Some(401), "未认证");
        match rpc_error(&unauthorized) {
            AppError::Unauthorized(m) => assert_eq!(m, unauthorized.to_string()),
            other => panic!("内核 401 应映射 HTTP 401，实际 {other:?}"),
        }

        let timeout = rpc_failure(Some(504), "等待特权命令超时");
        match rpc_error(&timeout) {
            AppError::Timeout(m) => assert_eq!(m, timeout.to_string()),
            other => panic!("内核 504 应映射 HTTP 504，实际 {other:?}"),
        }

        // 未识别的状态与普通插件业务错误仍归 400，并保留原文。
        let unknown = rpc_failure(Some(418), "未知语义");
        assert!(matches!(rpc_error(&unknown), AppError::BadRequest(m) if m == unknown.to_string()));
        let business = rpc_failure(None, "任务名不能为空");
        assert!(
            matches!(rpc_error(&business), AppError::BadRequest(m) if m == business.to_string()),
            "普通业务错误应保持 400 且文案不变"
        );
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
        let tpl = include_str!("../../templates/plugin_view.html");
        assert!(tpl.contains(SANDBOX), "插件 iframe 沙箱属性被改动");
        assert!(
            !tpl.contains("allow-same-origin"),
            "插件 iframe 不得加 allow-same-origin"
        );
    }
}
