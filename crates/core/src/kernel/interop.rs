//! 插件间调用（interop）：`call` / `catalog`。
//!
//! 四道闸的**判定**住在 `skiff_plugin::interop`（权限、`plugin:<目标>` 对象授权、
//! 调用方 `consumes` 白名单、目标 `provides` 声明）；这里负责查库、防环、超时与审计。
//! 目标插件仍按自己的授权读写，转发不放宽任何一方。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use skiff_app::AppState;

use skiff_plugin::capability as cap;
use skiff_plugin::interop::{self, InteropDeny};

/// 插件间调用的单次上限：目标插件被自动拉起 + 一次 RPC。超时只断这次调用，
/// 不熔断目标进程（进程故障由 supervisor 的失败计数负责）。
const INTEROP_TIMEOUT: Duration = Duration::from_secs(15);

/// interop 拒绝的统一出口：审计 + 状态码。越权记 `deny`，请求非法/不存在记 `fail`。
///
/// 审计 target 记**目标插件**（不是方法名），这样按插件过滤能看到完整调用链。
fn interop_deny(
    state: &AppState,
    caller: &str,
    target: &str,
    method: &str,
    d: InteropDeny,
) -> Response {
    if let Err(e) = state.db().audit(
        &format!("plugin:{caller}"),
        "uds",
        "interop.call",
        target,
        d.kind.audit_result(),
        &format!("{method}: {}", d.message),
    ) {
        tracing::error!(error = %e, "插件互调拒绝审计写入失败");
    }
    let status = StatusCode::from_u16(d.kind.status()).unwrap_or(StatusCode::FORBIDDEN);
    (status, d.message).into_response()
}

fn manifest_deny(
    state: &AppState,
    caller: &str,
    target: &str,
    method: &str,
    e: String,
) -> Response {
    interop_deny(
        state,
        caller,
        target,
        method,
        InteropDeny {
            kind: interop::DenyKind::BadRequest,
            message: e,
        },
    )
}

/// 调用方当前可用的插件接口目录（只读）。
///
/// 与 `call` 共用同一套判定：UI 里能看到的项一定调得通，看不到的调了一定 403。
pub(super) async fn interop_catalog(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let caller = cap::plugin_of(&req);
    let Some(rec) = state.db().plugin(&caller).ok().flatten() else {
        return interop_deny(
            &state,
            &caller,
            "",
            "catalog",
            InteropDeny {
                kind: interop::DenyKind::NotFound,
                message: "调用方插件记录不存在".into(),
            },
        );
    };
    let manifest = match skiff_pkgfmt::Manifest::parse(&rec.manifest) {
        Ok(m) => m,
        Err(e) => {
            return manifest_deny(
                &state,
                &caller,
                "",
                "catalog",
                format!("manifest 不可解析: {e}"),
            )
        }
    };
    let records = state.db().list_plugins().unwrap_or_default();
    let interfaces = interop::catalog(&caller, &manifest, &rec.grants, &records);
    if let Err(e) = state.db().audit(
        &format!("plugin:{caller}"),
        "uds",
        "interop.catalog",
        "",
        "ok",
        &format!("{} 个可调用目标", interfaces.len()),
    ) {
        tracing::error!(error = %e, "插件互调目录审计写入失败");
    }
    Json(json!({
        "permission": interop::PERM_CALL,
        "interfaces": interfaces,
    }))
    .into_response()
}

/// 插件间接口调用：`{ plugin, method, params }` → 目标 RPC 的 `result`。
///
/// 参数里的 `_interop.caller` 由内核注入（覆盖调用方自带的同名键），目标插件据此
/// 记录来源、做幂等 upsert；调用方无法伪造来源。
pub(super) async fn interop_call(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let caller = cap::plugin_of(&req);
    let body = cap::read_json(req).await;
    let target = body
        .get("plugin")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let params = body.get("params").cloned().unwrap_or_else(|| json!({}));

    if target.is_empty() || method.is_empty() || !params.is_object() {
        return interop_deny(
            &state,
            &caller,
            &target,
            &method,
            InteropDeny {
                kind: interop::DenyKind::BadRequest,
                message: "plugin/method 不能为空，且 params 必须是 JSON 对象".into(),
            },
        );
    }

    let Some(caller_rec) = state.db().plugin(&caller).ok().flatten() else {
        return interop_deny(
            &state,
            &caller,
            &target,
            &method,
            InteropDeny {
                kind: interop::DenyKind::NotFound,
                message: "调用方插件记录不存在".into(),
            },
        );
    };
    let caller_manifest = match skiff_pkgfmt::Manifest::parse(&caller_rec.manifest) {
        Ok(m) => m,
        Err(e) => {
            return manifest_deny(
                &state,
                &caller,
                &target,
                &method,
                format!("调用方 manifest 不可解析: {e}"),
            )
        }
    };

    let Some(target_rec) = state.db().plugin(&target).ok().flatten() else {
        return interop_deny(
            &state,
            &caller,
            &target,
            &method,
            InteropDeny {
                kind: interop::DenyKind::NotFound,
                message: format!("目标插件 {target} 未安装"),
            },
        );
    };
    if !target_rec.enabled {
        return interop_deny(
            &state,
            &caller,
            &target,
            &method,
            InteropDeny {
                kind: interop::DenyKind::Conflict,
                message: format!("目标插件 {target} 未启用"),
            },
        );
    }
    let target_manifest = match skiff_pkgfmt::Manifest::parse(&target_rec.manifest) {
        Ok(m) => m,
        Err(e) => {
            return manifest_deny(
                &state,
                &caller,
                &target,
                &method,
                format!("目标 manifest 不可解析: {e}"),
            )
        }
    };

    if let Err(d) = interop::authorize(
        &caller,
        &caller_manifest,
        &caller_rec.grants,
        &target,
        &target_manifest,
        &method,
    ) {
        return interop_deny(&state, &caller, &target, &method, d);
    }

    // 防环：目标正在被另一次 interop 调用占用时直接拒。
    // 只做单层守卫（A→B→A 会在第二层被挡），不做全链路追踪——插件 RPC 是单进程
    // 串行的，多层守卫会误伤正常的并发来源。
    if !state.enter_interop(&target).await {
        return interop_deny(
            &state,
            &caller,
            &target,
            &method,
            InteropDeny {
                kind: interop::DenyKind::Conflict,
                message: format!("目标插件 {target} 正在处理另一次插件间调用，请稍后重试"),
            },
        );
    }

    let mut params = params;
    if let Some(obj) = params.as_object_mut() {
        obj.insert("_interop".into(), json!({ "caller": caller }));
    }

    let outcome = tokio::time::timeout(
        INTEROP_TIMEOUT,
        state.supervisor().call(&target_rec, &method, params),
    )
    .await;
    // 无论成功/失败/超时都必须释放占用，否则目标会被永久挡在 409 上。
    state.leave_interop(&target).await;

    match outcome {
        Ok(Ok(result)) => {
            if let Err(e) = state.db().audit(
                &format!("plugin:{caller}"),
                "uds",
                "interop.call",
                &target,
                "ok",
                &method,
            ) {
                tracing::error!(error = %e, "插件互调审计写入失败");
            }
            Json(json!({ "plugin": target, "method": method, "result": result })).into_response()
        }
        Ok(Err(e)) => {
            if let Err(ae) = state.db().audit(
                &format!("plugin:{caller}"),
                "uds",
                "interop.call",
                &target,
                "fail",
                &format!("{method}: {e}"),
            ) {
                tracing::error!(error = %ae, "插件互调审计写入失败");
            }
            (
                StatusCode::BAD_GATEWAY,
                format!("目标插件 {target} 调用失败: {e}"),
            )
                .into_response()
        }
        Err(_) => {
            let _ = state.db().audit(
                &format!("plugin:{caller}"),
                "uds",
                "interop.call",
                &target,
                "fail",
                &format!("{method}: {}s 超时", INTEROP_TIMEOUT.as_secs()),
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!(
                    "目标插件 {target} 在 {}s 内未返回（可能正在处理长任务）",
                    INTEROP_TIMEOUT.as_secs()
                ),
            )
                .into_response()
        }
    }
}
