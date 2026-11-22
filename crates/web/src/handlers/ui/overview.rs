use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/overview`：只读概览计数。
///
/// 复用 `GET /{p}` 的同一批查询（容器列表 + 插件/站点/运行环境列表），不新增聚合 SQL；
/// 本批不含列表、写操作、图表或实时刷新，图表位由前端 schema 明确占位。
pub async fn overview(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<PageFlagQuery>,
) -> Result<Response, ApiError> {
    let data = crate::handlers::dashboard::load_overview(&state, user.user.id).await?;
    let containers = data.containers;
    let running = containers.iter().filter(|c| c.state == "running").count();
    let plugin_enabled = data.plugins.iter().filter(|p| p.enabled).count();
    let site_abnormal = data.sites.iter().filter(|s| s.status != "running").count();
    let mut runtime_abnormal = 0usize;
    let mut runtime_uninstalled = 0usize;
    for r in &data.runtimes {
        let name = skiff_app::sites::runtime_container_name_for(&r.kind, &r.name, &r.id);
        match containers.iter().find(|c| c.name == name) {
            Some(c) if c.state != "running" => runtime_abnormal += 1,
            None => runtime_uninstalled += 1,
            Some(_) => {}
        }
    }
    let docker = if data.docker_ok {
        OverviewDocker {
            status: OverviewStatus {
                code: "available",
                label: "可用",
            },
            reason: String::new(),
        }
    } else {
        OverviewDocker {
            status: OverviewStatus {
                code: "unavailable",
                label: "不可用",
            },
            reason: data.docker_reason,
        }
    };
    let collected_at = skiff_app::time_fmt::epoch_to_rfc3339_utc(chrono::Utc::now().timestamp())
        .unwrap_or_default();
    // 从已启用插件的 manifest 里找 `overview.health` 槽位提供者；不读槽位 HTML，
    // 只把插件 id 交给前端走 RPC（权限与审计仍由 RPC 通道负责）。
    // 没有可用提供者时给出可读原因（停用 / 未安装），避免概览静默缺块。
    let mut health_plugin = None;
    let mut health_disabled: Option<String> = None;
    for p in &data.plugins {
        let Some(manifest) = skiff_pkgfmt::Manifest::parse(&p.manifest).ok() else {
            continue;
        };
        if !manifest.mount.iter().any(|m| m.slot == "overview.health") {
            continue;
        }
        if p.enabled {
            health_plugin = Some(p.id.clone());
            break;
        }
        if health_disabled.is_none() {
            health_disabled = Some(p.id.clone());
        }
    }
    let health_reason = if health_plugin.is_some() {
        String::new()
    } else if let Some(id) = health_disabled {
        format!("主机监控插件 {id} 已停用；启用后此处显示 CPU、内存与负载。")
    } else {
        "未安装声明 overview.health 的插件；安装并启用主机监控插件后，此处显示系统状态。"
            .to_string()
    };
    let counts = OverviewCounts {
        collected_at,
        docker,
        containers: OverviewContainers {
            running,
            total: containers.len(),
        },
        sites: OverviewSites {
            total: data.sites.len(),
            abnormal: site_abnormal,
        },
        plugins: OverviewPlugins {
            total: data.plugins.len(),
            enabled: plugin_enabled,
            disabled: data.plugins.len().saturating_sub(plugin_enabled),
        },
        runtimes: OverviewRuntimes {
            total: data.runtimes.len(),
            abnormal: runtime_abnormal,
            uninstalled: runtime_uninstalled,
        },
        health_plugin,
        health_reason,
    };
    let schema = if q.with_schema {
        let health = load_health(
            &state,
            counts.health_plugin.as_deref(),
            &counts.health_reason,
        )
        .await;
        Some(super::overview_page::overview_schema(
            &serde_json::to_value(&counts).unwrap_or_default(),
            &health,
        ))
    } else {
        None
    };
    let mut body = json!({ "data": counts });
    if let Some(schema) = schema {
        body["schema"] = schema;
    }
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// 主机监控槽位：直接走插件 RPC 的 supervisor 通道，失败也返回可读原因。
///
/// 页面以前在浏览器里调 `/plugins/{id}/rpc`；页面改由服务端产出后，同一通道在
/// 这里发起——权限、审计与失败文案口径不变。
async fn load_health(state: &AppState, plugin: Option<&str>, reason: &str) -> serde_json::Value {
    let Some(id) = plugin else {
        return json!({ "error": reason });
    };
    let rec = match state.db().plugin(id) {
        Ok(Some(rec)) => rec,
        Ok(None) => return json!({ "error": format!("插件不存在：{id}") }),
        Err(e) => return json!({ "error": e.to_string() }),
    };
    match state.supervisor().call(&rec, "snapshot", json!({})).await {
        Ok(value) => value
            .get("sample")
            .cloned()
            .unwrap_or_else(|| json!({ "error": "插件没有返回采样数据" })),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ui/v1/overview", get(overview))
}
