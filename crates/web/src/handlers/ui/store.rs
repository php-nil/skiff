use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/store?refresh=1`：商店目录只读快照（列表 / 状态 / 升级提示）。
pub async fn store(
    State(state): State<Arc<AppState>>,
    UiAuthUser(_user): UiAuthUser,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let force = q.get("refresh").map(|v| v == "1").unwrap_or(false);
    let view = handlers::store::load_view(&state, force)
        .await
        .map_err(ApiError)?;
    let snap = view.snapshot;
    let total = snap.entries.total();
    let sections = vec![
        StoreSectionItem {
            title: "插件".into(),
            hint: "已验证目录条目可直接安装：下载 → 摘要/大小 → 验签 → 权限确认。".into(),
            installable: view.can_install,
            items: view.plugins.into_iter().map(catalog_item).collect(),
        },
        StoreSectionItem {
            title: "应用模板".into(),
            hint: "Docker 应用模板（Compose）；本页只展示清单。".into(),
            installable: false,
            items: snap.entries.apps.into_iter().map(catalog_item).collect(),
        },
        StoreSectionItem {
            title: "运行环境".into(),
            hint: "网站运行环境版本（PHP/Node/Rust 镜像）；本页只展示清单。".into(),
            installable: false,
            items: snap
                .entries
                .runtimes
                .into_iter()
                .map(catalog_item)
                .collect(),
        },
        StoreSectionItem {
            title: "升级通道".into(),
            hint: "内核升级包通道；升级操作不在本页发起。".into(),
            installable: false,
            items: snap
                .entries
                .upgrades
                .into_iter()
                .map(catalog_item)
                .collect(),
        },
    ];
    let upgrade = StoreUpgradeItem {
        current: view.upgrade.current,
        arch: view.upgrade.arch,
        available: view.upgrade.available,
        channel: view.upgrade.channel,
        min_from: view.upgrade.min_from,
        changelog_url: view.upgrade.changelog_url,
        note: view.upgrade.note,
        can_download: view.upgrade.can_download,
    };
    let mut resp = Json(json!({
        "data": {
            "state": StatusDto {
                code: catalog_state_code(snap.state).into(),
                label: crate::labels::catalog_state(snap.state).into(),
            },
            "message": snap.message,
            "channel": snap.channel,
            "seq": snap.seq.map(|v| v.to_string()).unwrap_or_else(|| "—".into()),
            "fetched_at": snap.fetched_at,
            "source": snap.source,
            "allow_unsigned": snap.allow_unsigned,
            "key_configured": snap.key_configured,
            "can_install": view.can_install,
            "total": total,
            "sections": sections,
            "upgrade": upgrade,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/store/plugins/{id}/install`：下载并验签，返回权限确认数据。
///
/// 与旧表单入口共用 `store::prepare`、同一审计动作与同一权限（`PluginManage`）；
/// 真正的安装要再调 `POST /ui/v1/plugins/confirm`（与旧确认页同一条收尾路径）。
pub async fn store_install(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<StoreInstallBody>,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    if !state.cfg.catalog.enabled {
        return Err(ApiError(AppError::NotFound("商店目录未启用".into())));
    }
    let ip = handlers::client_ip(&state, &parts);
    if let Err(e) = handlers::require_permission(&user, skiff_model::Permission::PluginManage) {
        handlers::store::audit(&state, &user, &ip, &id, "deny", "reason=permission");
        return Err(ApiError(e));
    }
    if state.cfg.catalog.public_key_hex().is_none() {
        handlers::store::audit(&state, &user, &ip, &id, "deny", "reason=no_key");
        return Err(ApiError(AppError::BadRequest(
            "未配置固定 Ed25519 公钥，不提供商店安装".into(),
        )));
    }
    let req = skiff_app::commands::store::StoreInstallRequest {
        plugin_id: id.clone(),
        version: body.version.filter(|v| !v.trim().is_empty()),
        form: state.cfg.form,
        allow_unsigned: state.cfg.allow_unsigned,
        allow_reduced_sandbox: state.cfg.allow_reduced_sandbox,
        data_dir: state.cfg.data_dir.clone(),
    };
    let worker = Arc::clone(&state);
    let prepared = match handlers::blocking_fs(move || {
        skiff_app::commands::store::prepare(worker.catalog(), worker.db(), &req)
    })
    .await?
    {
        Ok(prepared) => prepared,
        Err(e) => {
            handlers::store::audit(
                &state,
                &user,
                &ip,
                &id,
                "fail",
                &format!("reason={} {e}", e.reason()),
            );
            return Err(ApiError(AppError::BadRequest(e.message())));
        }
    };
    let meta = &prepared.trust.meta;
    handlers::store::audit(
        &state,
        &user,
        &ip,
        &meta.manifest.id,
        "ok",
        &format!(
            "reason=prepared version={} channel={} source={}",
            meta.manifest.version, prepared.channel, prepared.source
        ),
    );
    let mut resources = meta.manifest.resource_hints.clone();
    resources.extend(meta.manifest.interop_resource_hints());
    resources.sort();
    resources.dedup();
    let (interop_provides, interop_consumes) = handlers::plugins::interop_rows(&meta.manifest);
    let mut resp = Json(json!({
        "data": {
            "token": prepared.token,
            "id": meta.manifest.id,
            "name": meta.manifest.name,
            "version": meta.manifest.version,
            "api": meta.manifest.api,
            "key_id": meta.key_id,
            "signed": meta.signature_verified,
            "permissions": meta.manifest.permission_strings(),
            "resources": resources,
            "interop_provides": interop_provides,
            "interop_consumes": interop_consumes,
            "requires": meta.manifest.requires_str(),
            "egress": meta.manifest.network_egress,
            "channel": prepared.channel,
            "source": prepared.source,
        }
    }))
    .into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `POST /ui/v1/store/upgrades/{version}/download`：下载升级包到本地暂存并校验。
///
/// 仅管理员，写路径先过 `x-csrf-token`；业务与旧表单入口共用
/// `store::apply_upgrade_download`（同一公钥校验、暂存、sha256 与 `upgrade.download` 审计）。
pub async fn store_upgrade_download(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Path(version): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    handlers::check_csrf_header(&state, &user, &parts)?;
    let ip = handlers::client_ip(&state, &parts);
    let outcome = handlers::store::apply_upgrade_download(&state, &user, &ip, &version)
        .await
        .map_err(ApiError)?;
    match outcome {
        handlers::store::UpgradeDownloadOutcome::Staged(staged) => {
            let mut resp = Json(json!({
                "data": {
                    "version": staged.version,
                    "arch": staged.arch,
                    "channel": staged.channel,
                    "source": staged.source,
                    "sha256": staged.sha256,
                    "size": staged.size,
                    "path": staged.path.display().to_string(),
                    "reused": staged.reused,
                }
            }))
            .into_response();
            resp.headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(resp)
        }
        handlers::store::UpgradeDownloadOutcome::Failed(message) => {
            Err(ApiError(AppError::BadRequest(message)))
        }
    }
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ui/v1/store", get(store))
        .route("/ui/v1/store/plugins/{id}/install", post(store_install))
        .route(
            "/ui/v1/store/upgrades/{version}/download",
            post(store_upgrade_download),
        )
}
