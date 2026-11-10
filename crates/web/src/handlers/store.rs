//! 商店（Skiff-server P1/P2）：渠道目录展示 + 插件安装入口。
//!
//! 页面只消费 `CatalogService::snapshot`：拉取/验签/缓存/防回滚都在 `skiff_app::catalog`
//! 内完成。安装入口同样不在这里下载或验签——`skiff_app::commands::store::prepare`
//! 负责「下载 → 摘要/大小 → 验签 → 暂存」，本模块只做权限判定、审计与确认页渲染；
//! 最终安装仍由确认页 POST `/plugins/confirm` 走既有 `install_package`。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::response::Response;

use skiff_app::catalog::CatalogState;
use skiff_model::Permission;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::{AppState, CurrentUser};
use crate::templates::{PermissionRow, PluginConfirmPage, StorePage, StoreSection};

/// 商店安装的统一审计动作：成功暂存、各类拒绝都落这一条。
const AUDIT_ACTION: &str = "store.install";

pub async fn page(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    if !state.cfg.catalog.enabled {
        return Err(AppError::NotFound("商店目录未启用".into()));
    }
    let force = matches!(q.get("refresh").map(String::as_str), Some("1"));
    let worker = Arc::clone(&state);
    let snap = handlers::blocking_fs(move || worker.catalog().snapshot(force)).await?;

    let tone = match snap.state {
        CatalogState::Fresh => "ok",
        CatalogState::Cached | CatalogState::Offline | CatalogState::Disabled => "warn",
        CatalogState::Rejected => "",
    };
    // 只有公钥已配置且目录没有被拒时才给安装入口：未配置公钥的目录无法完成包验签。
    let can_install = snap.key_configured
        && !matches!(snap.state, CatalogState::Rejected | CatalogState::Disabled);
    let total = snap.entries.total();
    handlers::render(StorePage {
        ctx: handlers::page_ctx(&state, &user, "core/plugins/store"),
        state_label: snap.state.label().to_string(),
        tone: tone.to_string(),
        message: snap.message.clone(),
        channel: snap.channel.clone(),
        seq: snap
            .seq
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into()),
        fetched_at: if snap.fetched_at.is_empty() {
            "—".into()
        } else {
            snap.fetched_at.clone()
        },
        source: if snap.source.is_empty() {
            "—".into()
        } else {
            snap.source.clone()
        },
        allow_unsigned: snap.allow_unsigned,
        key_configured: snap.key_configured,
        sections: vec![
            StoreSection {
                title: "插件".into(),
                hint: "已验证目录条目可直接安装：下载 → 摘要/大小 → 验签 → 权限确认。".into(),
                installable: can_install,
                items: snap.entries.plugins,
            },
            StoreSection {
                title: "应用模板".into(),
                hint: "Docker 应用模板（Compose）；本页只展示清单。".into(),
                installable: false,
                items: snap.entries.apps,
            },
            StoreSection {
                title: "运行环境".into(),
                hint: "网站运行环境版本（PHP/Node/Rust 镜像）；本页只展示清单。".into(),
                installable: false,
                items: snap.entries.runtimes,
            },
            StoreSection {
                title: "升级通道".into(),
                hint: "内核升级包通道；升级操作不在本页发起。".into(),
                installable: false,
                items: snap.entries.upgrades,
            },
        ],
        total,
    })
}

/// 商店安装第一步：按已验证目录条目下载并验签，成功后渲染权限确认页。
///
/// 拒绝语义按原因落 `store.install` 审计，并回到商店页展示提示；
/// 任何失败都不会在 `.incoming` 留下暂存文件（由 `prepare` 保证）。
pub async fn install(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    parts: Parts,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    if !state.cfg.catalog.enabled {
        return Err(AppError::NotFound("商店目录未启用".into()));
    }
    let ip = handlers::client_ip(&state, &parts);
    if let Err(e) = handlers::require_permission(&user, Permission::PluginManage) {
        audit(&state, &user, &ip, &id, "deny", "reason=permission");
        return Err(e);
    }
    if state.cfg.catalog.public_key_hex().is_none() {
        audit(&state, &user, &ip, &id, "deny", "reason=no_key");
        return Err(AppError::BadRequest(
            "未配置固定 Ed25519 公钥，不提供商店安装".into(),
        ));
    }

    let req = skiff_app::commands::store::StoreInstallRequest {
        plugin_id: id.clone(),
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
            audit(
                &state,
                &user,
                &ip,
                &id,
                "fail",
                &format!("reason={} {e}", e.reason()),
            );
            return Ok(handlers::flash_redirect(
                &state,
                &user,
                skiff_app::FlashKind::Error,
                e.message(),
                format!("/{}/store", state.cfg.entry_prefix),
            ));
        }
    };

    let meta = &prepared.trust.meta;
    audit(
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
    let mut resources = meta.manifest.resource_hints.clone();
    resources.extend(meta.manifest.interop_resource_hints());
    resources.sort();
    resources.dedup();
    let (interop_provides, interop_consumes) =
        crate::handlers::plugins::interop_rows(&meta.manifest);

    handlers::render(PluginConfirmPage {
        ctx: handlers::page_ctx(&state, &user, "core/plugins/store"),
        token: prepared.token,
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
        error: format!(
            "来源：商店目录 {} · {}（版本 {}）",
            prepared.channel, prepared.source, meta.manifest.version
        ),
    })
}

fn audit(state: &AppState, user: &CurrentUser, ip: &str, target: &str, result: &str, detail: &str) {
    let _ = state.db().audit(
        &user.user.username,
        ip,
        AUDIT_ACTION,
        target,
        result,
        detail,
    );
}

#[cfg(test)]
mod tests {
    use askama::Template;
    use skiff_app::catalog::CatalogItem;

    use crate::menu::PageMenu;
    use crate::templates::{PageCtx, StoreSection};

    use super::*;

    fn ctx() -> PageCtx {
        PageCtx {
            prefix: "p1".into(),
            username: "admin".into(),
            role: "admin".into(),
            csrf: "csrf".into(),
            dev_http: false,
            version: "0.1.0".into(),
            docker_ok: true,
            must_change: false,
            active: "core/plugins/store".into(),
            menu: PageMenu {
                layout: "auto".into(),
                width: "auto".into(),
                max_width: 0,
                groups: Vec::new(),
                pinned: Vec::new(),
            },
            is_admin: false,
            nav_plugins: Vec::new(),
            flash_notice: String::new(),
            flash_error: String::new(),
        }
    }

    fn item(name: &str) -> CatalogItem {
        CatalogItem {
            name: name.into(),
            version: "1.0.0".into(),
            source: "stable".into(),
            updated_at: "2026-11-10T00:00:00Z".into(),
            compatibility: "API 1.0".into(),
            note: "示例条目".into(),
            warn: String::new(),
        }
    }

    #[test]
    fn renders_status_rows_and_install_entry() {
        let page = StorePage {
            ctx: ctx(),
            state_label: "目录未更新".into(),
            tone: "warn".into(),
            message: "目录未更新（304），展示本地缓存".into(),
            channel: "stable".into(),
            seq: "7".into(),
            fetched_at: "2026-11-10T00:00:00Z".into(),
            source: "https://cdn.example/skiff".into(),
            allow_unsigned: true,
            key_configured: true,
            sections: vec![StoreSection {
                title: "插件".into(),
                hint: "已验证目录条目可直接安装".into(),
                installable: true,
                items: vec![item("com.example.hello")],
            }],
            total: 1,
        };
        let html = page.render().unwrap();
        assert!(html.contains("目录未更新"), "状态提示缺失");
        assert!(html.contains("com.example.hello"), "条目缺失");
        assert!(html.contains("API 1.0"), "兼容性列缺失");
        assert!(html.contains("允许未签名"), "开发模式警示缺失");
        assert!(
            html.contains("/p1/store/plugins/com.example.hello/install"),
            "公钥已配置时必须提供安装入口: {html}"
        );
        assert!(html.contains(">安装</a>"), "安装按钮文案缺失");
    }

    #[test]
    fn hides_install_entry_without_key_or_for_warned_item() {
        let mut warned = item("com.example.yanked");
        warned.warn = "已撤回".into();
        let page = StorePage {
            ctx: ctx(),
            state_label: "校验失败".into(),
            tone: String::new(),
            message: "验签失败".into(),
            channel: "stable".into(),
            seq: "—".into(),
            fetched_at: "—".into(),
            source: "—".into(),
            allow_unsigned: false,
            key_configured: false,
            sections: vec![StoreSection {
                title: "插件".into(),
                hint: "未配置公钥不提供安装".into(),
                installable: false,
                items: vec![warned],
            }],
            total: 1,
        };
        let html = page.render().unwrap();
        assert!(html.contains("校验失败"));
        assert!(html.contains("未配置固定公钥"));
        assert!(!html.contains("/install"), "未配置公钥时不得提供安装入口");
        assert!(!html.contains(">安装</a>"), "未配置公钥时不得出现安装按钮");
    }

    #[test]
    fn warned_item_is_marked_not_installable() {
        let mut warned = item("com.example.yanked");
        warned.warn = "已撤回".into();
        let page = StorePage {
            ctx: ctx(),
            state_label: "已更新".into(),
            tone: "ok".into(),
            message: "ok".into(),
            channel: "stable".into(),
            seq: "8".into(),
            fetched_at: "2026-11-10T00:00:00Z".into(),
            source: "https://cdn.example/skiff".into(),
            allow_unsigned: false,
            key_configured: true,
            sections: vec![StoreSection {
                title: "插件".into(),
                hint: "撤回条目不可安装".into(),
                installable: true,
                items: vec![warned],
            }],
            total: 1,
        };
        let html = page.render().unwrap();
        assert!(html.contains("已撤回"));
        assert!(html.contains("不可安装"));
        assert!(
            !html.contains("com.example.yanked/install"),
            "撤回条目不得渲染安装链接: {html}"
        );
    }

    #[test]
    fn renders_empty_section_hint() {
        let page = StorePage {
            ctx: ctx(),
            state_label: "校验失败".into(),
            tone: String::new(),
            message: "验签失败".into(),
            channel: "stable".into(),
            seq: "—".into(),
            fetched_at: "—".into(),
            source: "—".into(),
            allow_unsigned: false,
            key_configured: false,
            sections: vec![StoreSection {
                title: "升级通道".into(),
                hint: "只读".into(),
                installable: false,
                items: Vec::new(),
            }],
            total: 0,
        };
        let html = page.render().unwrap();
        assert!(html.contains("校验失败"));
        assert!(html.contains("升级通道暂无条目"));
        assert!(html.contains("未配置固定公钥"));
    }
}
