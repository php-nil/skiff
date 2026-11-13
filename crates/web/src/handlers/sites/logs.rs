//! 站点日志：按域名读取访问/错误日志。

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;

use crate::error::AppError;
use crate::handlers;
use crate::middleware::AuthUser;
use crate::state::AppState;
use crate::templates::SiteLogsPage;

use super::*;

pub async fn logs(
    State(state): State<Arc<AppState>>,
    AuthUser(user): AuthUser,
    Path(domain): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let keyword = q.get("q").cloned().unwrap_or_default();
    // 新一代前端（budding 壳）承载日志查看器；旧 SSR 只保留给显式 ?legacy=1
    // 的无 JS / 兼容请求，面板内不再导航到它。
    if !q.get("legacy").map(|v| v == "1").unwrap_or(false) {
        return Ok(crate::handlers::spa::shell_page(&state));
    }
    // 日志读取是同步文件 IO（尾部最多 2MB），放到阻塞线程池，避免占住 async worker。
    let st = state.clone();
    let domain_for_read = domain.clone();
    let keyword_for_read = keyword.clone();
    let (access, error_log) = crate::handlers::blocking_fs(move || {
        st.site_logs(&domain_for_read, 200, &keyword_for_read)
    })
    .await?
    .map_err(map_err)?;
    let log_dir = state
        .db()
        .site_by_domain(&domain)?
        .map(|s| {
            skiff_app::sites::site_log_dir(FsPath::new(&s.root_dir))
                .display()
                .to_string()
        })
        .unwrap_or_default();
    let notice = if q.contains_key("created") {
        "站点已创建；下面是站点探针首次访问日志（若为空，请用访问入口打开一次）。"
    } else {
        ""
    };
    handlers::render(SiteLogsPage {
        ctx: handlers::page_ctx(&state, &user, "core/sites/list"),
        domain,
        log_dir,
        access_log: access,
        error_log,
        keyword,
        notice: notice.to_string(),
    })
}
