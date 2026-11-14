use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/audit?q=&limit=&cursor=`：审计流水（只读，`AuditRead`）。
///
/// 关键字命中 actor / ip / action / target / detail；游标是 `AuditScope` 签名游标，
/// 载荷为上一页末项的 `(ts, id)`，与筛选条件指纹绑定，换关键字后旧游标直接拒绝。
pub async fn audit(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    crate::authz::authorize_for(&user, skiff_model::Permission::AuditRead).map_err(ApiError)?;
    let keyword = q.q.as_deref().unwrap_or("").trim().to_string();
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    // 复用持久化的 CSRF 派生密钥：签名域由 `AuditScope`（`cursor:audit:`）隔离。
    let codec = CursorCodec::<AuditScope>::new(state.csrf_secret.clone());
    let filter = FilterSet::new().with("q", &keyword);
    let before = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
        Some(cursor) => Some(parse_audit_cursor(&codec, cursor, &filter)?),
        None => None,
    };
    let mut rows = state
        .db()
        .audit_page(&keyword, before, limit + 1)
        .map_err(|e| ApiError(AppError::Internal(e.to_string())))?;
    // 多取一条只用于判断下一页；游标取本页最后一条的排序键。
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = if has_more {
        rows.last()
            .map(|a| codec.encode(&[a.ts.to_string(), a.id.to_string()], &filter))
    } else {
        None
    };
    let items: Vec<AuditItem> = rows
        .into_iter()
        .map(|a| AuditItem {
            id: a.id,
            ts: rfc3339(a.ts),
            actor: a.actor,
            ip: a.ip,
            action: a.action,
            target: a.target,
            result: a.result,
            detail: crate::util::truncate(&a.detail, 200),
        })
        .collect();
    Ok(Json(json!({
        "data": {
            "items": items,
            "keyword": keyword,
            "has_more": has_more,
            "next_cursor": next_cursor,
        }
    })))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ui/v1/audit", get(audit))
}
