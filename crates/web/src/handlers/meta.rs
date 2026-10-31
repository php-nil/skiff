use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// 探针：不要求安全入口前缀，不泄漏内部状态。
pub async fn healthz(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": state.version,
        "uptime_secs": state.started_at.elapsed().as_secs(),
        "docker": if state.docker.available() { "available" } else { "unavailable" },
    }))
}
