//! 内核侧 UDS 服务 `/run/skiff/kernel.sock`：插件回调内核的唯一通道（§4.2）。
//!
//! 双重鉴权：`SO_PEERCRED` 的 UID ↔ 插件 id 映射（每个插件独立 UID），
//! 加上进程级 Bearer token（经 stdin 一次性交付）。只凭 socket 可达或 token 单一因素不授权。
//!
//! 能力面现状（诚实标注）：
//! - `identity`：返回插件自身身份与 granted 集；
//! - `config`：返回内核代管的 `config_schema` 取值（§5.2 两轨制的第一轨）；
//! - `fs`：仅限插件自身 `data/` 目录（landlock 未实现，越权靠路径判定兜住）；
//! - `docker`：白名单动词 + 对象 grant，走与 UI 相同的三层策略；
//! - `net`：**未实现**（nftables 收口属 M4 门禁），一律 501 + 审计。

use std::sync::Arc;

use skiff_app::AppState;

#[cfg(unix)]
pub fn spawn(state: Arc<AppState>, run_dir: std::path::PathBuf) {
    // 必须真后台：imp::run 是常驻 accept 循环，被调用方 .await 会永远堵在这，
    // HTTP serve 就排不到（Windows 的 stub 立即返回，掩盖过这个问题）。
    tokio::spawn(async move {
        match imp::run(state, run_dir).await {
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, "kernel.sock 服务未启动（插件回调能力不可用）"),
        }
    });
}

#[cfg(not(unix))]
pub fn spawn(_state: Arc<AppState>, _run_dir: std::path::PathBuf) {
    tracing::info!("非 unix 平台：跳过 kernel.sock（插件进程外隔离不可用）");
}

#[cfg(unix)]
mod imp {
    use super::*;

    use std::os::unix::fs::PermissionsExt;

    use axum::body::Body;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{Request, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::{Json, Router};
    use http_body_util::BodyExt;
    use hyper::body::Incoming;
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;
    use serde_json::{json, Value};
    use skiff_docker::{Actor, Verb};
    use skiff_store::Grant;
    use tokio::net::UnixListener;
    use tower::ServiceExt;

    #[derive(Clone, Copy, Debug)]
    struct PeerUid(u32);

    pub async fn run(state: Arc<AppState>, run_dir: std::path::PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(&run_dir)?;
        let sock = run_dir.join("kernel.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock)?;
        // 插件进程的 gid 是它自己的独立 gid，只能靠**附加组** skiff-plugins 拿到
        // 这个 socket 的组权限。所以这里必须把属组改成运行组，否则插件连上来就是
        // EACCES——所有能力调用全灭，且报错发生在插件侧，很难定位。
        match skiff_plugin::install::shared_gid() {
            Some(gid) => {
                if let Ok(c) = std::ffi::CString::new(sock.to_string_lossy().as_bytes()) {
                    // SAFETY: c 是合法 C 字符串；uid 传 -1 表示「不改属主」。
                    let rc = unsafe { libc::chown(c.as_ptr(), u32::MAX, gid) };
                    if rc != 0 {
                        tracing::warn!(
                            error = %std::io::Error::last_os_error(),
                            "kernel.sock 改属组失败，插件可能连不上"
                        );
                    }
                }
                let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o660));
            }
            None => {
                // 手跑（没走 install.sh，没有运行组）：退化为 0666。
                // 这不是提权缺口——每个连接都要过 SO_PEERCRED(uid↔插件) + Bearer token。
                tracing::warn!(
                    "运行组 skiff-plugins 不存在：kernel.sock 退化为 0666（鉴权仍靠 peer uid + token）"
                );
                let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666));
            }
        }

        // 路由常量来自 skiff-wire，与插件 SDK 共用一份；写死字面量曾导致
        // SDK 调用 `capability/config` 而内核没有该路由（永久 404）。
        use skiff_wire::routes;
        let router = Router::new()
            .route(routes::CAP_IDENTITY_SELF, post(identity_self))
            .route(routes::CAP_CONFIG, post(config_get))
            .route(routes::CAP_FS_READ, post(fs_read))
            .route(routes::CAP_FS_WRITE, post(fs_write))
            .route(routes::CAP_FS_LIST, post(fs_list))
            .route(routes::CAP_DOCKER_CALL, post(docker_call))
            .route(routes::CAP_NET_APPLY, post(net_unimplemented))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                plugin_auth,
            ))
            .with_state(state.clone());

        tracing::info!(socket = %sock.display(), "kernel.sock 监听中");
        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "kernel.sock accept 失败");
                    continue;
                }
            };
            let uid = match stream.peer_cred() {
                Ok(cred) => cred.uid(),
                Err(e) => {
                    tracing::warn!(error = %e, "取 SO_PEERCRED 失败，拒绝连接");
                    continue;
                }
            };
            let router = router.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(move |req: Request<Incoming>| {
                    let router = router.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let bytes = body
                            .collect()
                            .await
                            .map(|c| c.to_bytes())
                            .unwrap_or_else(|_| Bytes::new());
                        let mut req = Request::from_parts(parts, Body::from(bytes));
                        req.extensions_mut().insert(PeerUid(uid));
                        router.oneshot(req).await
                    }
                });
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, svc)
                    .await
                {
                    tracing::debug!(error = %e, "kernel.sock 连接结束");
                }
            });
        }
    }

    async fn plugin_auth(
        State(state): State<Arc<AppState>>,
        mut req: Request<Body>,
        next: axum::middleware::Next,
    ) -> Response {
        let pid = req.extensions().get::<PeerUid>().map(|p| p.0);
        let Some(uid) = pid else {
            return (StatusCode::FORBIDDEN, "缺少 peer 凭据").into_response();
        };
        // UID → 插件 id
        let plugin_id = match uid_to_plugin(&state, uid).await {
            Some(id) => id,
            None => {
                let _ = state.db.audit(
                    &format!("uid:{uid}"),
                    "uds",
                    "capability.auth",
                    "",
                    "deny",
                    "UID 未映射到插件",
                );
                return (StatusCode::FORBIDDEN, "UID 未映射到任何插件").into_response();
            }
        };
        let token = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("")
            .trim()
            .to_string();
        let expected = state
            .supervisor
            .token_of(&plugin_id)
            .await
            .unwrap_or_default();
        if expected.is_empty() || !skiff_identity::token_matches(&expected, &token) {
            let _ = state.db.audit(
                &format!("plugin:{plugin_id}"),
                "uds",
                "capability.auth",
                &plugin_id,
                "deny",
                "token 不匹配",
            );
            return (StatusCode::FORBIDDEN, "插件身份令牌无效").into_response();
        }
        req.extensions_mut().insert(PluginId(plugin_id));
        next.run(req).await
    }

    #[derive(Clone, Debug)]
    struct PluginId(String);

    async fn uid_to_plugin(state: &AppState, uid: u32) -> Option<String> {
        state
            .db
            .list_plugins()
            .ok()?
            .into_iter()
            .find(|p| skiff_plugin::install::plugin_uid(&p.id) == uid)
            .map(|p| p.id)
    }

    async fn identity_self(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let Some(id) = req.extensions().get::<PluginId>().map(|p| p.0.clone()) else {
            return (StatusCode::FORBIDDEN, "未认证").into_response();
        };
        match state.db.plugin(&id) {
            Ok(Some(p)) => Json(json!({
                "id": p.id,
                "version": p.version,
                "api": p.api,
                "granted": p.grants.iter().map(|g| format!("{}:{}", g.kind, g.value)).collect::<Vec<_>>(),
            }))
            .into_response(),
            _ => (StatusCode::NOT_FOUND, "插件不存在").into_response(),
        }
    }

    /// `config_schema` 声明项的取值。内核代管于 `skiff.db`（§5.2），插件运行时一次性取回。
    ///
    /// 与 SDK `Plugin::config()` 一一对应——此前 SDK 打 `/api/v1/capability/config` 而内核
    /// 没有这条路由，插件只要读到配置就拿到 404，属于「文档有、代码没有」的硬缺口。
    ///
    /// `secret` 项**原样返回**：调用方已被 UID + Bearer token 双因素绑定为插件自身，
    /// 值本身也不在插件可写目录里；但**禁止**把值写进日志或审计 detail。
    async fn config_get(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let rows = match state.db.plugin_config(&id) {
            Ok(v) => v,
            Err(e) => {
                let _ = state.db.audit(
                    &format!("plugin:{id}"),
                    "uds",
                    "capability.config",
                    "",
                    "fail",
                    &e.to_string(),
                );
                return (StatusCode::INTERNAL_SERVER_ERROR, "读取插件配置失败").into_response();
            }
        };
        let mut obj = serde_json::Map::with_capacity(rows.len());
        for (k, v, _secret) in rows {
            obj.insert(k, Value::String(v));
        }
        // 只记条数，绝不记 key 之外的任何内容（secret 值不得入审计）。
        let _ = state.db.audit(
            &format!("plugin:{id}"),
            "uds",
            "capability.config",
            "",
            "ok",
            &format!("{} 项", obj.len()),
        );
        Json(Value::Object(obj)).into_response()
    }

    /// 内核代写文件后把属主还给插件 UID。
    ///
    /// 内核是 root，`fs.write` 落盘的文件默认归 root；插件随后用 `SKIFF_PLUGIN_DATA_DIR`
    /// 直接改同一个文件就会被自己的权限挡住——「能力写成功、插件自己打不开」的经典坑。
    fn own_by_plugin(path: &std::path::Path, id: &str) {
        use std::os::unix::fs::PermissionsExt;
        let uid = skiff_plugin::install::plugin_uid(id);
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        if let Ok(c) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
            // SAFETY: 合法 C 字符串；失败只发生在非 root 运行的情况。
            unsafe { libc::chown(c.as_ptr(), uid, uid) };
        }
    }

    fn plugin_data_dir(state: &AppState, id: &str) -> std::path::PathBuf {
        state.cfg.data_dir.join("plugins").join(id).join("data")
    }

    fn resolve_in_data(
        state: &AppState,
        id: &str,
        rel: &str,
    ) -> Result<std::path::PathBuf, &'static str> {
        if rel.contains("..") || rel.starts_with('/') || rel.contains('\0') {
            return Err("路径非法");
        }
        let root = plugin_data_dir(state, id);
        std::fs::create_dir_all(&root).map_err(|_| "创建数据目录失败")?;
        let canon_root = std::fs::canonicalize(&root).map_err(|_| "数据目录不可用")?;
        let target = root.join(rel);
        // 目标可能不存在（写场景）：校验父目录
        let parent = target.parent().ok_or("路径无父目录")?;
        std::fs::create_dir_all(parent).map_err(|_| "创建父目录失败")?;
        let canon_parent = std::fs::canonicalize(parent).map_err(|_| "父目录不可用")?;
        if !canon_parent.starts_with(&canon_root) {
            return Err("越权路径");
        }
        Ok(target)
    }

    async fn fs_read(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        if !has_perm(&state, &id, "fs:read") {
            return deny(&state, &id, "fs:read");
        }
        let body = read_json(req).await;
        let rel = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
        match resolve_in_data(&state, &id, rel) {
            Ok(path) => match std::fs::read(&path) {
                Ok(data) => Json(json!({ "data": String::from_utf8_lossy(&data) })).into_response(),
                Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
            },
            Err(m) => (StatusCode::FORBIDDEN, m).into_response(),
        }
    }

    async fn fs_write(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        if !has_perm(&state, &id, "fs:write") {
            return deny(&state, &id, "fs:write");
        }
        let body = read_json(req).await;
        let rel = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let data = body.get("data").and_then(|v| v.as_str()).unwrap_or("");
        match resolve_in_data(&state, &id, rel) {
            Ok(path) => match std::fs::write(&path, data) {
                Ok(()) => {
                    own_by_plugin(&path, &id);
                    Json(json!({ "ok": true })).into_response()
                }
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            },
            Err(m) => (StatusCode::FORBIDDEN, m).into_response(),
        }
    }

    async fn fs_list(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        if !has_perm(&state, &id, "fs:list") {
            return deny(&state, &id, "fs:list");
        }
        let body = read_json(req).await;
        let rel = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
        match resolve_in_data(&state, &id, rel) {
            Ok(path) => {
                let mut names = Vec::new();
                if let Ok(rd) = std::fs::read_dir(&path) {
                    for e in rd.flatten() {
                        names.push(e.file_name().to_string_lossy().to_string());
                    }
                }
                Json(json!({ "entries": names })).into_response()
            }
            Err(m) => (StatusCode::FORBIDDEN, m).into_response(),
        }
    }

    async fn docker_call(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let body = read_json(req).await;
        let verb_str = body.get("verb").and_then(|v| v.as_str()).unwrap_or("");
        let Some(verb) = Verb::parse(verb_str) else {
            return (StatusCode::FORBIDDEN, "动词不在白名单").into_response();
        };
        let target = body.get("target").map(|t| skiff_docker::ObjectRef {
            id: t
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            name: t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            labels: t
                .get("labels")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
        });

        let grants: Vec<Grant> = match state.db.plugin(&id) {
            Ok(Some(p)) => p.grants,
            _ => Vec::new(),
        };
        let actor = Actor::Plugin {
            id: id.clone(),
            grants,
        };
        if let Err(e) = state.cfg.policy.check(&actor, verb, target.as_ref()) {
            let _ = state.db.audit(
                &format!("plugin:{id}"),
                "uds",
                &verb.audit_action(),
                target
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default()
                    .as_str(),
                "deny",
                &e.to_string(),
            );
            return (StatusCode::FORBIDDEN, e.to_string()).into_response();
        }

        // 动词 → Docker 方法的映射只在 `Docker::execute` 一处，这里不再自己 match
        let result = state.docker.execute(verb, target.as_ref()).await;

        match result {
            Ok(v) => {
                let _ = state.db.audit(
                    &format!("plugin:{id}"),
                    "uds",
                    &verb.audit_action(),
                    target
                        .as_ref()
                        .map(|t| t.name.clone())
                        .unwrap_or_default()
                        .as_str(),
                    "ok",
                    "",
                );
                Json(json!({ "result": v })).into_response()
            }
            Err(e) => {
                // 「动词需要专用参数」不是引擎故障，如实回 501 而不是 502
                let status = match e {
                    skiff_docker::DockerError::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
                    _ => StatusCode::BAD_GATEWAY,
                };
                let _ = state.db.audit(
                    &format!("plugin:{id}"),
                    "uds",
                    &verb.audit_action(),
                    "",
                    if status == StatusCode::NOT_IMPLEMENTED {
                        "unsupported"
                    } else {
                        "fail"
                    },
                    &e.to_string(),
                );
                (status, e.to_string()).into_response()
            }
        }
    }

    async fn net_unimplemented(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
        let id = req
            .extensions()
            .get::<PluginId>()
            .map(|p| p.0.clone())
            .unwrap_or_default();
        let _ = state.db.audit(
            &format!("plugin:{id}"),
            "uds",
            "net.apply",
            "",
            "unsupported",
            "nftables 出网收口未实现（M4 门禁）",
        );
        (
            StatusCode::NOT_IMPLEMENTED,
            "net 能力未实现：nftables owner-match 收口属 M4 硬门槛（§9.1 #17）",
        )
            .into_response()
    }

    fn has_perm(state: &AppState, id: &str, perm: &str) -> bool {
        state
            .db
            .plugin(id)
            .ok()
            .flatten()
            .map(|p| p.grants.iter().any(|g| g.kind == "perm" && g.value == perm))
            .unwrap_or(false)
    }

    fn deny(state: &AppState, id: &str, perm: &str) -> Response {
        // 权限名是 `fs:read` 形态，审计动作统一用点分（`fs.read`）——否则按 action
        // 过滤审计时同一个能力面会出现冒号与点两套拼法。
        let _ = state.db.audit(
            &format!("plugin:{id}"),
            "uds",
            &perm.replace(':', "."),
            "",
            "deny",
            "缺少授权",
        );
        (StatusCode::FORBIDDEN, format!("缺少授权 {perm}")).into_response()
    }

    async fn read_json(req: Request<Body>) -> Value {
        let bytes = match req.into_body().collect().await {
            Ok(c) => c.to_bytes(),
            Err(_) => return Value::Null,
        };
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }
}
