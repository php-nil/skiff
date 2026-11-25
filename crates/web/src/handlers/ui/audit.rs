use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

#[allow(unused_imports)]
use super::*;

/// `GET /ui/v1/audit?q=&cat=&limit=&cursor=`：审计流水（只读，`AuditRead`）。
///
/// 关键字命中 actor / ip / action / target / detail；`cat` 按动作命名空间分类过滤
/// （`skiff_model::AuditCategory`，未知值按全部分类）。游标是 `AuditScope` 签名游标，
/// 载荷为上一页末项的 `(ts, id)`，与「关键字 + 分类」指纹绑定，换筛选后旧游标直接拒绝。
pub async fn audit(
    State(state): State<Arc<AppState>>,
    UiAuthUser(user): UiAuthUser,
    Query(q): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    guard::permission(&user, skiff_model::Permission::AuditRead)?;
    let keyword = q.q.as_deref().unwrap_or("").trim().to_string();
    let category = q.cat.as_deref().and_then(skiff_model::AuditCategory::parse);
    let category_key = category.map(|c| c.key()).unwrap_or("");
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    // 复用持久化的 CSRF 派生密钥：签名域由 `AuditScope`（`cursor:audit:`）隔离。
    let codec = CursorCodec::<AuditScope>::new(state.csrf_secret.clone());
    let filter = FilterSet::new()
        .with("q", &keyword)
        .with("cat", category_key);
    let before = match q.cursor.as_deref().filter(|c| !c.is_empty()) {
        Some(cursor) => Some(parse_audit_cursor(&codec, cursor, &filter)?),
        None => None,
    };
    let mut rows = state
        .db()
        .audit_page(&keyword, category, before, limit + 1)
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
        .map(|a| {
            let category = skiff_model::AuditCategory::of(&a.action);
            AuditItem {
                id: a.id,
                ts: rfc3339(a.ts),
                ts_label: crate::util::fmt_local_time(Some(a.ts), "%Y-%m-%d %H:%M:%S", "—"),
                category: category.key().to_string(),
                category_label: category.label().to_string(),
                actor: a.actor,
                ip: a.ip,
                action: a.action,
                target: a.target,
                result: a.result,
                detail: crate::util::truncate(&a.detail, 200),
            }
        })
        .collect();
    // 页面骨架按需生成：翻页跳转不带 `with_schema`，不会重复拼结构。
    let schema = q.with_schema.then(|| {
        let prefix = format!("/{}", state.cfg.entry_prefix);
        super::audit_page::audit_page_schema(
            &items,
            &keyword,
            category,
            has_more,
            next_cursor.as_deref(),
            &prefix,
        )
    });
    let mut body = json!({
        "data": {
            "items": items,
            "keyword": keyword,
            "category": category_key,
            "has_more": has_more,
            "next_cursor": next_cursor,
        }
    });
    if let Some(schema) = schema {
        body["schema"] = schema;
    }
    Ok(Json(body))
}

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ui/v1/audit", get(audit))
}
