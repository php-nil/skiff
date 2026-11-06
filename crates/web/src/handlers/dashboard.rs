use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::{AuditRow, DashboardPage, DashboardTodo};

/// `/{prefix}/` → `/{prefix}`（307 保留方法与查询串）。
pub async fn root_redirect(State(state): State<Arc<AppState>>) -> Response {
    axum::response::Redirect::temporary(&format!("/{}", state.cfg.entry_prefix)).into_response()
}

pub async fn index(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
) -> Result<Response, AppError> {
    let docker_ok = state.docker().available();
    let containers = if docker_ok {
        state.docker().list_containers().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let running = containers.iter().filter(|c| c.state == "running").count();
    let abnormal: Vec<_> = containers
        .iter()
        .filter(|c| handlers::container_abnormal(&c.state))
        .collect();
    let image_count = if docker_ok {
        match state.docker().list_images().await {
            Ok(v) => v.len().to_string(),
            Err(_) => "-".into(),
        }
    } else {
        "-".into()
    };
    let plugins = state.db().list_plugins()?;
    let plugin_enabled = plugins.iter().filter(|p| p.enabled).count();
    let plugin_disabled = plugins.len().saturating_sub(plugin_enabled);
    let sites = state.db().list_sites()?;
    let site_abnormal = sites.iter().filter(|s| s.status != "running").count();
    let runtimes = state.db().list_runtimes()?;
    let runtime_total = runtimes.len();
    let mut runtime_abnormal = 0usize;
    let mut runtime_uninstalled = 0usize;
    for r in &runtimes {
        let name = if r.kind == skiff_store::KIND_PHP {
            skiff_app::sites::runtime_container_name(&r.kind, &r.name)
        } else {
            format!("skiff-runtime-{}", r.id)
        };
        match containers.iter().find(|c| c.name == name) {
            Some(c) if c.state != "running" => runtime_abnormal += 1,
            // 未安装（没有容器）不算异常，只是尚未安装。
            None => runtime_uninstalled += 1,
            Some(_) => {}
        }
    }
    let totp_enabled = state
        .db()
        .user_by_id(user.user.id)?
        .map(|u| u.totp_enabled)
        .unwrap_or(false);
    let prefix = state.cfg.entry_prefix.clone();
    let docker_reason = state.docker_unavailable_reason().to_string();
    let initial_admin_pending = state.cfg.data_dir.join("initial-admin.txt").exists();
    let todos = dashboard_todos(
        &prefix,
        user.must_change_password(),
        initial_admin_pending,
        docker_ok,
        &docker_reason,
        abnormal.len(),
        plugin_disabled,
        totp_enabled,
        site_abnormal,
    );
    let audit = state
        .db()
        .recent_audit(6)?
        .into_iter()
        .map(|a| AuditRow {
            ts: chrono::DateTime::from_timestamp(a.ts, 0)
                .map(|d| d.format("%m-%d %H:%M:%S").to_string())
                .unwrap_or_default(),
            actor: a.actor,
            ip: a.ip,
            action: a.action,
            target: a.target,
            result: a.result,
            detail: crate::util::truncate(&a.detail, 80),
        })
        .collect();

    // 固定页槽位（P2）：插件停用/未授权时返回空表，模板自动回落内核默认内容。
    let mounts_health = crate::menu::mount_views(&state, &user, "overview.health");
    let mounts_actions = crate::menu::mount_views(&state, &user, "overview.actions");
    let mounts_side = crate::menu::mount_views(&state, &user, "overview.side");

    let page = DashboardPage {
        ctx: handlers::page_ctx(&state, &user, "dashboard"),
        running,
        total: containers.len(),
        image_count,
        plugin_total: plugins.len(),
        plugin_enabled,
        site_total: sites.len(),
        site_abnormal,
        runtime_total,
        runtime_abnormal,
        runtime_uninstalled,
        abnormal_total: abnormal.len(),
        abnormal: abnormal
            .iter()
            .take(6)
            .map(|c| handlers::container_view(c))
            .collect(),
        uptime: format_uptime(state.started_at.elapsed().as_secs()),
        docker_reason,
        todos,
        audit,
        mounts_health,
        mounts_actions,
        mounts_side,
        error: String::new(),
    };
    handlers::render(page)
}

/// 概览待办：只由**真实信号**生成（会话强制改密、初始口令未改、Docker 可用性、
/// 异常容器、停用插件、TOTP 状态）。按影响排序：危险 → 警告；同级保持生成顺序。
#[allow(clippy::too_many_arguments)]
fn dashboard_todos(
    prefix: &str,
    must_change: bool,
    initial_admin_pending: bool,
    docker_ok: bool,
    docker_reason: &str,
    abnormal_containers: usize,
    disabled_plugins: usize,
    totp_enabled: bool,
    abnormal_sites: usize,
) -> Vec<DashboardTodo> {
    let link = |path: &str| {
        if path.is_empty() {
            format!("/{prefix}")
        } else {
            format!("/{prefix}/{path}")
        }
    };
    let todo =
        |level: &str, title: String, detail: String, href: String, action: &str| DashboardTodo {
            level: level.into(),
            title,
            detail,
            href,
            action: action.into(),
        };

    let mut todos = Vec::new();
    if must_change {
        todos.push(todo(
            "danger",
            "首次登录必须修改口令".into(),
            "当前会话在改密前不能执行其他管理操作。".into(),
            link("settings?force=1"),
            "去改口令",
        ));
    } else if initial_admin_pending {
        // 默认口径下不强制跳转，只给一条可忽略的建议。
        todos.push(todo(
            "warn",
            "建议修改初始口令".into(),
            "当前口令仍保存在数据目录的 initial-admin.txt；修改成功后文件会自动删除。".into(),
            link("settings#password"),
            "去修改",
        ));
    }
    if !docker_ok {
        let reason = if docker_reason.trim().is_empty() {
            "Docker 守护进程当前不可达"
        } else {
            docker_reason
        };
        todos.push(todo(
            "danger",
            "Docker 不可用".into(),
            format!("{reason}；容器、镜像、网络、卷操作会返回 503。"),
            link("containers"),
            "查看容器页",
        ));
    }
    if abnormal_containers > 0 {
        todos.push(todo(
            "danger",
            format!("{abnormal_containers} 个容器处于异常状态"),
            "exited / dead / restarting / paused 需要人工确认；进容器详情可看日志与重启。".into(),
            link("containers?filter=error"),
            "查看异常容器",
        ));
    }
    if abnormal_sites > 0 {
        todos.push(todo(
            "danger",
            format!("{abnormal_sites} 个站点已停止或异常"),
            "站点配置未启用或 nginx 重载失败；进「网站列表」可启动站点并查看日志。".into(),
            link("m/core/sites/list"),
            "查看站点",
        ));
    }
    if disabled_plugins > 0 {
        todos.push(todo(
            "warn",
            format!("{disabled_plugins} 个插件已停用"),
            "停用的插件不会出现在「应用」菜单；需要时到插件中心重新启用。".into(),
            link("plugins"),
            "去插件中心",
        ));
    }
    if !totp_enabled {
        todos.push(todo(
            "warn",
            "两步验证未开启".into(),
            "账号目前只靠口令保护；开启 TOTP 后登录会多一步校验。".into(),
            link("settings#totp"),
            "去开启",
        ));
    }

    todos.sort_by_key(|t| match t.level.as_str() {
        "danger" => 0,
        "warn" => 1,
        _ => 2,
    });
    todos
}

fn format_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}天 {h}小时")
    } else if h > 0 {
        format!("{h}小时 {m}分")
    } else {
        format!("{m}分")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todos_are_empty_when_everything_is_healthy() {
        let todos = dashboard_todos("p", false, false, true, "", 0, 0, true, 0);
        assert!(todos.is_empty());
    }

    /// 默认口径：初始口令未改时只给非阻塞警告，不产生强制跳转条目。
    #[test]
    fn initial_password_pending_only_suggests_change() {
        let todos = dashboard_todos("p", false, true, true, "", 0, 0, true, 0);
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].level, "warn");
        assert_eq!(todos[0].href, "/p/settings#password");
        assert!(todos[0].title.contains("建议修改初始口令"));
    }

    #[test]
    fn todos_are_impact_ordered_and_deep_linked() {
        let todos = dashboard_todos("p", true, false, false, "socket missing", 2, 1, false, 1);
        let levels: Vec<&str> = todos.iter().map(|t| t.level.as_str()).collect();
        assert_eq!(
            levels,
            ["danger", "danger", "danger", "danger", "warn", "warn"]
        );

        let hrefs: Vec<&str> = todos.iter().map(|t| t.href.as_str()).collect();
        assert_eq!(
            hrefs,
            [
                "/p/settings?force=1",
                "/p/containers",
                "/p/containers?filter=error",
                "/p/m/core/sites/list",
                "/p/plugins",
                "/p/settings#totp",
            ]
        );
    }
}
