use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

/// `GET /ui/v1/overview` 的原始数据快照。
///
/// 只做读：容器列表（Docker）+ 插件/站点/运行环境/审计（一次阻塞投递），
/// 判定与计数留在各自调用方，避免两套页面口径分叉。
pub struct OverviewData {
    pub docker_ok: bool,
    pub docker_reason: String,
    pub containers: Vec<skiff_docker::ContainerRow>,
    pub plugins: Vec<skiff_store::PluginRecord>,
    pub sites: Vec<skiff_store::Site>,
    pub runtimes: Vec<skiff_store::Runtime>,
    pub totp_enabled: bool,
    pub audit: Vec<skiff_store::AuditEntry>,
    pub uptime_secs: u64,
}

/// 读取概览所需的全部数据；DB 部分合并为一次阻塞投递（§15）。
pub async fn load_overview(state: &AppState, user_id: i64) -> Result<OverviewData, AppError> {
    let docker_ok = state.docker().available();
    let containers = if docker_ok {
        state.docker().list_containers().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let (plugins, sites, runtimes, totp_enabled, audit) = {
        let db = std::sync::Arc::clone(state.db());
        let loaded = skiff_app::db_async::run(move || {
            let plugins = db.list_plugins()?;
            let sites = db.list_sites()?;
            let runtimes = db.list_runtimes()?;
            let totp_enabled = db
                .user_by_id(user_id)?
                .map(|u| u.totp_enabled)
                .unwrap_or(false);
            let audit = db.recent_audit(6)?;
            Ok::<_, skiff_store::StoreError>((plugins, sites, runtimes, totp_enabled, audit))
        })
        .await;
        match loaded {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(AppError::from(e)),
            Err(e) => {
                tracing::error!(error = %e, "投递概览页读库失败");
                return Err(AppError::Unavailable(format!("数据库暂不可用：{e}")));
            }
        }
    };
    Ok(OverviewData {
        docker_ok,
        docker_reason: state.docker_unavailable_reason().to_string(),
        containers,
        plugins,
        sites,
        runtimes,
        totp_enabled,
        audit,
        uptime_secs: state.started_at.elapsed().as_secs(),
    })
}

/// `/{prefix}/` → `/{prefix}`（307 保留方法与查询串）。
pub async fn root_redirect(State(state): State<Arc<AppState>>) -> Response {
    axum::response::Redirect::temporary(&format!("/{}", state.cfg.entry_prefix)).into_response()
}
