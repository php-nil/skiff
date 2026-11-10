//! 商店（Skiff-server P1）：只读展示渠道目录，不提供安装。
//!
//! 页面只消费 `CatalogService::snapshot`：拉取/验签/缓存/防回滚都在 `skiff_app::catalog`
//! 内完成，这里不做任何网络或文件 I/O（快照读取放在阻塞线程池）。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Response;

use skiff_app::catalog::CatalogState;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::{StorePage, StoreSection};

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
                hint: "官方渠道插件包；安装仍走「插件 → 已安装 / 发现」。".into(),
                items: snap.entries.plugins,
            },
            StoreSection {
                title: "应用模板".into(),
                hint: "Docker 应用模板（Compose）；本页只展示清单。".into(),
                items: snap.entries.apps,
            },
            StoreSection {
                title: "运行环境".into(),
                hint: "网站运行环境版本（PHP/Node/Rust 镜像）。".into(),
                items: snap.entries.runtimes,
            },
            StoreSection {
                title: "升级通道".into(),
                hint: "内核升级包通道；升级操作不在本页发起。".into(),
                items: snap.entries.upgrades,
            },
        ],
        total,
    })
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
    fn renders_status_rows_and_has_no_install_action() {
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
                hint: "只读".into(),
                items: vec![item("com.example.hello")],
            }],
            total: 1,
        };
        let html = page.render().unwrap();
        assert!(html.contains("目录未更新"), "状态提示缺失");
        assert!(html.contains("com.example.hello"), "条目缺失");
        assert!(html.contains("API 1.0"), "兼容性列缺失");
        assert!(html.contains("允许未签名"), "开发模式警示缺失");
        // 只读页面：不得出现安装入口（base.html 的退出登录表单不算写操作入口）。
        assert!(!html.contains("安装</"), "不应提供安装按钮");
        assert!(!html.contains("plugins/install"), "不应提供安装入口");
        assert!(!html.contains("plugins/confirm"), "不应提供安装确认入口");
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
