//! 审计查询（§17.7 R1）：`GET /api/v1/audit`。
//!
//! 数据源是 `state/audit.jsonl`（追加写）。只回 `at/key_id/action/submission_id/result`，
//! **不回签名、包内容或任何请求体**。审核者看全部；作者默认只看自己，显式查他人 403。

use std::fs;

use serde::Deserialize;
use serde_json::json;

use super::auth::Authenticated;
use super::{json_response, ApiError, ApiServer};
use crate::http::Response;

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    at: String,
    #[serde(default)]
    key_id: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    submission_id: String,
    #[serde(default)]
    result: String,
}

pub fn query(server: &ApiServer, auth: &Authenticated, query: &str) -> Result<Response, ApiError> {
    let mut sid: Option<String> = None;
    let mut key_id: Option<String> = None;
    let mut action: Option<String> = None;
    let mut result: Option<String> = None;
    let mut since: Option<String> = None;
    let mut until: Option<String> = None;
    let mut limit = DEFAULT_LIMIT;
    let mut offset = 0usize;

    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "sid" => sid = Some(value.to_string()),
            "key_id" => key_id = Some(value.to_string()),
            "action" => action = Some(value.to_string()),
            "result" => result = Some(value.to_string()),
            "since" => since = Some(value.to_string()),
            "until" => until = Some(value.to_string()),
            "limit" => {
                limit = value
                    .parse::<usize>()
                    .map_err(|_| ApiError::bad_request("limit 不是合法数字"))?
                    .clamp(1, MAX_LIMIT)
            }
            "offset" => {
                offset = value
                    .parse::<usize>()
                    .map_err(|_| ApiError::bad_request("offset 不是合法数字"))?
            }
            other => return Err(ApiError::bad_request(format!("未知查询参数: {other}"))),
        }
    }

    // 作者只能看自己的记录：显式查他人 403，不传则默认限定自己。
    if !auth.reviewer {
        match &key_id {
            Some(requested) if requested != &auth.key_id => return Err(ApiError::forbidden()),
            Some(_) => {}
            None => key_id = Some(auth.key_id.clone()),
        }
    }

    let entries = read_entries(server)?;
    let filtered: Vec<Entry> = entries
        .into_iter()
        .rev()
        .filter(|entry| {
            sid.as_ref()
                .map(|v| &entry.submission_id == v)
                .unwrap_or(true)
                && key_id.as_ref().map(|v| &entry.key_id == v).unwrap_or(true)
                && action.as_ref().map(|v| &entry.action == v).unwrap_or(true)
                && result.as_ref().map(|v| &entry.result == v).unwrap_or(true)
                && since.as_ref().map(|v| &entry.at >= v).unwrap_or(true)
                && until.as_ref().map(|v| &entry.at <= v).unwrap_or(true)
        })
        .collect();

    let total = filtered.len();
    let page: Vec<_> = filtered.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset + page.len();
    let next = if next_offset < total {
        Some(next_offset)
    } else {
        None
    };
    let body = json!({
        "entries": page.iter().map(|entry| json!({
            "at": entry.at,
            "key_id": entry.key_id,
            "action": entry.action,
            "submission_id": entry.submission_id,
            "result": entry.result,
        })).collect::<Vec<_>>(),
        "total": total,
        "next_offset": next,
    });
    Ok(json_response(200, &body))
}

fn read_entries(server: &ApiServer) -> Result<Vec<Entry>, ApiError> {
    let path = server.state.join("audit.jsonl");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        // 坏行（例如进程中断写了一半）跳过，不影响其余审计可查。
        if let Ok(entry) = serde_json::from_str::<Entry>(line) {
            out.push(entry);
        }
    }
    Ok(out)
}
