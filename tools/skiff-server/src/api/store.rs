//! 提交记录、幂等与状态机（§17.3/§17.4）。
//!
//! 落盘只用文件：`state/submissions/<sid>.json` 与 `<sid>/package.wbpkg`。
//! 幂等键是 `(id, version, sha256)`：同三元组重复提交返回同一 `sid` 且状态不变。
//! 非法迁移一律 412；版本冲突（同版本不同摘要、已发布）一律 409。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::auth::{sha256_hex, Authenticated};
use super::multipart;
use super::package;
use super::{json_response, now_rfc3339, ApiError, ApiServer, MAX_PACKAGE_BYTES};
use crate::events::{self, Event};
use crate::http::{Request, Response};
use crate::model::{PluginEntry, KNOWN_CHANNELS};

const SUBMISSION_SCHEMA: &str = "skiff.server.submission/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Rejected,
    Approved,
    Withdrawn,
    Published,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Rejected => "rejected",
            State::Approved => "approved",
            State::Withdrawn => "withdrawn",
            State::Published => "published",
        }
    }
}

/// 提交的条目类型（§17.3）：插件是既有形态，模板/运行环境在 R2c/R2d 接入。
///
/// 与 `TakedownInfo::kind`（yank / revoke）不是一回事，后者描述处置动作。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    #[default]
    Plugin,
    App,
    Runtime,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Plugin => "plugin",
            EntryKind::App => "app",
            EntryKind::Runtime => "runtime",
        }
    }

    /// 插件是唯一既有形态：记录里不落 `kind`，保持旧文件与旧客户端看到的字节不变。
    pub fn is_plugin(&self) -> bool {
        *self == EntryKind::Plugin
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMeta {
    pub summary: String,
    pub channel: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changelog_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub key_id: String,
    pub action: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub seq: u64,
    pub at: String,
}

/// 站内可见的撤回信息（不触发任何推送；`GET /submissions/{sid}` 直接展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakedownInfo {
    pub kind: String,
    pub reason: String,
    pub key_id: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub schema: String,
    pub sid: String,
    /// 缺省即插件：旧 `state/submissions/*.json` 没有该字段，照常解析。
    #[serde(default, skip_serializing_if = "EntryKind::is_plugin")]
    pub kind: EntryKind,
    pub plugin_id: String,
    pub version: String,
    pub api: String,
    pub key_id: String,
    pub sha256: String,
    pub size: u64,
    pub state: State,
    pub channel: String,
    pub entry: EntryMeta,
    pub requires: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseInfo>,
    /// 已发布版本被撤包/吊销后的站内提示（不推送；`GET /submissions/{sid}` 直接展示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub takedown: Option<TakedownInfo>,
}

fn submissions_dir(state: &Path) -> PathBuf {
    state.join("submissions")
}

fn record_path(state: &Path, sid: &str) -> PathBuf {
    submissions_dir(state).join(format!("{sid}.json"))
}

pub fn sid_of(plugin_id: &str, version: &str, sha256: &str) -> String {
    let digest = Sha256::digest(format!("{plugin_id}\0{version}\0{sha256}").as_bytes());
    format!("sub-{}", &hex::encode(digest)[..24])
}

fn write_record(state: &Path, submission: &Submission) -> Result<(), ApiError> {
    let dir = submissions_dir(state);
    fs::create_dir_all(&dir).map_err(|_| ApiError::internal())?;
    let path = record_path(state, &submission.sid);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(submission).map_err(|_| ApiError::internal())?;
    fs::write(&tmp, bytes).map_err(|_| ApiError::internal())?;
    fs::rename(&tmp, &path).map_err(|_| ApiError::internal())
}

fn read_record(state: &Path, sid: &str) -> Result<Option<Submission>, ApiError> {
    match fs::read(record_path(state, sid)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ApiError::internal()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ApiError::internal()),
    }
}

fn all_records(state: &Path) -> Result<Vec<Submission>, ApiError> {
    let dir = submissions_dir(state);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        if let Ok(record) = serde_json::from_slice::<Submission>(&bytes) {
            out.push(record);
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.sid.cmp(&b.sid)));
    Ok(out)
}

/// 单提交独占锁：并发 approve 只有一个能进入状态迁移。
struct SidLock {
    path: PathBuf,
}

impl SidLock {
    fn acquire(state: &Path, sid: &str) -> Result<Self, ApiError> {
        let path = submissions_dir(state).join(format!("{sid}.lock"));
        fs::create_dir_all(submissions_dir(state)).map_err(|_| ApiError::internal())?;
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => Ok(Self { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ApiError::precondition("submission is being processed"))
            }
            Err(_) => Err(ApiError::internal()),
        }
    }
}

impl Drop for SidLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 临时文件守卫：任何失败路径都不在 state 里留半截包。
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// POST /api/v1/submissions
pub fn create(
    server: &ApiServer,
    auth: &Authenticated,
    request: &Request,
    body: &[u8],
) -> Result<Response, ApiError> {
    let author = auth.require_author()?.clone();
    let content_type = request
        .header("content-type")
        .ok_or_else(|| ApiError::bad_request("缺少 Content-Type"))?;
    let parts = multipart::parse(content_type, body).map_err(|message| {
        if message.contains("64 MiB") {
            ApiError::new(413, "payload_too_large", message)
        } else {
            ApiError::bad_request(message)
        }
    })?;
    if parts.package.is_empty() {
        return Err(ApiError::unprocessable("package 为空"));
    }
    if parts.package.len() > MAX_PACKAGE_BYTES {
        return Err(ApiError::new(
            413,
            "payload_too_large",
            "package 超过 64 MiB 上限",
        ));
    }
    let entry: EntryMeta = serde_json::from_slice(&parts.entry)
        .map_err(|e| ApiError::unprocessable(format!("entry 不是合法 JSON: {e}")))?;
    if !KNOWN_CHANNELS.contains(&entry.channel.as_str()) {
        return Err(ApiError::unprocessable(format!(
            "未知渠道: {}",
            entry.channel
        )));
    }

    let sha = sha256_hex(&parts.package);
    let size = parts.package.len() as u64;
    let tmp_dir = server.state.join("tmp");
    fs::create_dir_all(&tmp_dir).map_err(|_| ApiError::internal())?;
    let tmp_path = tmp_dir.join(format!("{sha}.wbpkg"));
    fs::write(&tmp_path, &parts.package).map_err(|_| ApiError::internal())?;
    let _tmp_guard = TempFile(tmp_path.clone());

    let meta = package::verify(&tmp_path, &auth.public_key)?;
    let manifest = &meta.manifest;
    if manifest.key_id != auth.key_id {
        return Err(ApiError::unprocessable("包内 key_id 与请求签名密钥不一致"));
    }
    let requires: Vec<String> = manifest
        .requires
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    if !entry.requires.is_empty() && entry.requires != requires {
        return Err(ApiError::unprocessable(
            "entry.requires 与包内 manifest 的能力面声明不一致",
        ));
    }
    if !namespace_allows(&author.id_prefixes, &manifest.id) {
        return Err(ApiError::forbidden());
    }
    if meta.package_bytes > MAX_PACKAGE_BYTES as u64 {
        return Err(ApiError::new(
            413,
            "payload_too_large",
            "package 超过 64 MiB 上限",
        ));
    }

    let sid = sid_of(&manifest.id, &manifest.version, &sha);
    let _lock = SidLock::acquire(&server.state, &sid)?;
    let now = now_rfc3339();

    // 幂等：同 (id, version, sha256) 返回同一提交。
    if let Some(existing) = read_record(&server.state, &sid)? {
        if existing.state == State::Rejected || existing.state == State::Withdrawn {
            // 被驳回/撤回后允许同一摘要重新进入待审（不新建记录）。
            let mut reopened = existing.clone();
            reopened.state = State::Pending;
            reopened.updated_at = now;
            write_record(&server.state, &reopened)?;
            server.audit(&auth.key_id, "submit", &sid, "reopened");
            // 通知失败不影响提交结果：outbox 写入是 best-effort。
            let _ = events::emit(
                &server.state,
                &Event::new(
                    events::EVENT_TYPE_SUBMISSION_CREATED,
                    &format!("{sid}:reopened:{}", reopened.updated_at),
                    &server.channel,
                )
                .actor(&auth.key_id)
                .subject("plugin", &reopened.plugin_id, &reopened.version)
                .submission(&sid)
                .reopened(true),
            );
            return Ok(json_response(
                200,
                &serde_json::to_value(&reopened).unwrap_or_default(),
            ));
        }
        server.audit(&auth.key_id, "submit", &sid, "duplicate");
        return Ok(json_response(
            200,
            &serde_json::to_value(&existing).unwrap_or_default(),
        ));
    }

    // 版本不可变：同 (id, version) 已有别的摘要，或已发布。
    for other in all_records(&server.state)? {
        if other.plugin_id == manifest.id && other.version == manifest.version {
            if other.state == State::Published {
                return Err(ApiError::conflict("该版本已发布，版本不可变"));
            }
            if other.sha256 != sha {
                return Err(ApiError::conflict("同版本已有不同摘要的提交"));
            }
        }
    }

    let final_dir = submissions_dir(&server.state).join(&sid);
    fs::create_dir_all(&final_dir).map_err(|_| ApiError::internal())?;
    let final_path = final_dir.join("package.wbpkg");
    fs::rename(&tmp_path, &final_path).map_err(|_| ApiError::internal())?;

    let source_entry = PluginEntry {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        api: manifest.api.clone(),
        channel: entry.channel.clone(),
        key_id: manifest.key_id.clone(),
        requires: requires.clone(),
        summary: entry.summary.clone(),
        published_at: now.clone(),
        yanked: false,
        yank_reason: None,
        pkg_sha256: None,
        pkg_url: None,
        size: None,
        asset: Some(format!("assets/{sha}.wbpkg")),
    };
    source_entry
        .validate_source()
        .map_err(|e| ApiError::unprocessable(format!("条目不合法: {e}")))?;

    let submission = Submission {
        schema: SUBMISSION_SCHEMA.to_string(),
        sid: sid.clone(),
        kind: EntryKind::Plugin,
        plugin_id: manifest.id.clone(),
        version: manifest.version.clone(),
        api: manifest.api.clone(),
        key_id: auth.key_id.clone(),
        sha256: sha,
        size,
        state: State::Pending,
        channel: entry.channel.clone(),
        entry,
        requires,
        created_at: now.clone(),
        updated_at: now,
        review: None,
        release: None,
        takedown: None,
    };
    write_record(&server.state, &submission)?;
    server.audit(&auth.key_id, "submit", &sid, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(events::EVENT_TYPE_SUBMISSION_CREATED, &sid, &server.channel)
            .actor(&auth.key_id)
            .subject("plugin", &submission.plugin_id, &submission.version)
            .submission(&sid),
    );
    Ok(json_response(
        201,
        &serde_json::to_value(&submission).unwrap_or_default(),
    ))
}

fn namespace_allows(prefixes: &[String], id: &str) -> bool {
    prefixes.iter().any(|prefix| {
        id == prefix
            || id
                .strip_prefix(prefix)
                .map(|rest| rest.starts_with('.'))
                .unwrap_or(false)
    })
}

/// GET /api/v1/submissions[?state=pending]
pub fn list(server: &ApiServer, auth: &Authenticated, query: &str) -> Result<Response, ApiError> {
    let filter = parse_state_filter(query)?;
    let mut out = Vec::new();
    for record in all_records(&server.state)? {
        if let Some(state) = filter {
            if record.state != state {
                continue;
            }
        }
        if !auth.reviewer && record.key_id != auth.key_id {
            continue;
        }
        out.push(record);
    }
    out.truncate(200);
    Ok(json_response(200, &json!({ "submissions": out })))
}

fn parse_state_filter(query: &str) -> Result<Option<State>, ApiError> {
    let mut filter = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key != "state" {
            return Err(ApiError::bad_request(format!("未知查询参数: {key}")));
        }
        filter = Some(match value {
            "pending" => State::Pending,
            "approved" => State::Approved,
            "rejected" => State::Rejected,
            "withdrawn" => State::Withdrawn,
            "published" => State::Published,
            _ => return Err(ApiError::bad_request("state 取值非法")),
        });
    }
    Ok(filter)
}

/// GET /api/v1/submissions/{sid}
pub fn get(server: &ApiServer, auth: &Authenticated, sid: &str) -> Result<Response, ApiError> {
    let record = read_record(&server.state, sid)?
        .ok_or_else(|| ApiError::new(404, "not_found", "not found"))?;
    if !auth.reviewer && record.key_id != auth.key_id {
        return Err(ApiError::forbidden());
    }
    Ok(json_response(
        200,
        &serde_json::to_value(&record).unwrap_or_default(),
    ))
}

/// POST /api/v1/submissions/{sid}/approve
pub fn approve(server: &ApiServer, auth: &Authenticated, sid: &str) -> Result<Response, ApiError> {
    auth.require_reviewer()?;
    let _lock = SidLock::acquire(&server.state, sid)?;
    let mut record = read_record(&server.state, sid)?
        .ok_or_else(|| ApiError::new(404, "not_found", "not found"))?;
    if record.key_id == auth.key_id {
        return Err(ApiError::forbidden());
    }
    // 提交者密钥已吊销：历史版本只能下架，不允许再审出新发布。
    if super::takedown::revoked_key(&server.state, &record.key_id)? {
        return Err(ApiError::conflict("提交者密钥已吊销，不能继续审核"));
    }
    match record.state {
        State::Approved | State::Published => {
            server.audit(&auth.key_id, "approve", sid, "duplicate");
            return Ok(json_response(
                200,
                &serde_json::to_value(&record).unwrap_or_default(),
            ));
        }
        State::Pending => {}
        _ => return Err(ApiError::precondition("当前状态不允许 approve")),
    }
    record.state = State::Approved;
    record.updated_at = now_rfc3339();
    record.review = Some(Review {
        key_id: auth.key_id.clone(),
        action: "approve".to_string(),
        at: record.updated_at.clone(),
        reason_code: None,
        comment: None,
    });
    write_record(&server.state, &record)?;
    server.audit(&auth.key_id, "approve", sid, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(events::EVENT_TYPE_SUBMISSION_APPROVED, sid, &server.channel)
            .actor(&auth.key_id)
            .subject("plugin", &record.plugin_id, &record.version)
            .submission(sid),
    );
    Ok(json_response(
        202,
        &serde_json::to_value(&record).unwrap_or_default(),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectBody {
    reason_code: String,
    #[serde(default)]
    comment: Option<String>,
}

/// POST /api/v1/submissions/{sid}/reject
pub fn reject(
    server: &ApiServer,
    auth: &Authenticated,
    sid: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    auth.require_reviewer()?;
    let payload: RejectBody = if body.is_empty() {
        return Err(ApiError::bad_request("reject 需要 JSON body"));
    } else {
        serde_json::from_slice(body)
            .map_err(|_| ApiError::bad_request("reject body 不是合法 JSON"))?
    };
    if !matches!(
        payload.reason_code.as_str(),
        "invalid_signature" | "schema" | "id_ownership" | "policy" | "other"
    ) {
        return Err(ApiError::bad_request("reason_code 非法"));
    }
    let _lock = SidLock::acquire(&server.state, sid)?;
    let mut record = read_record(&server.state, sid)?
        .ok_or_else(|| ApiError::new(404, "not_found", "not found"))?;
    if record.key_id == auth.key_id {
        return Err(ApiError::forbidden());
    }
    if record.state != State::Pending {
        return Err(ApiError::precondition("当前状态不允许 reject"));
    }
    record.state = State::Rejected;
    record.updated_at = now_rfc3339();
    record.review = Some(Review {
        key_id: auth.key_id.clone(),
        action: "reject".to_string(),
        at: record.updated_at.clone(),
        reason_code: Some(payload.reason_code),
        comment: payload.comment,
    });
    write_record(&server.state, &record)?;
    server.audit(&auth.key_id, "reject", sid, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(events::EVENT_TYPE_SUBMISSION_REJECTED, sid, &server.channel)
            .actor(&auth.key_id)
            .subject("plugin", &record.plugin_id, &record.version)
            .submission(sid),
    );
    Ok(json_response(
        200,
        &serde_json::to_value(&record).unwrap_or_default(),
    ))
}

/// POST /api/v1/submissions/{sid}/withdraw
pub fn withdraw(server: &ApiServer, auth: &Authenticated, sid: &str) -> Result<Response, ApiError> {
    auth.require_author()?;
    let _lock = SidLock::acquire(&server.state, sid)?;
    let mut record = read_record(&server.state, sid)?
        .ok_or_else(|| ApiError::new(404, "not_found", "not found"))?;
    if record.key_id != auth.key_id {
        return Err(ApiError::forbidden());
    }
    if record.state != State::Pending {
        return Err(ApiError::precondition("只有待审提交可以撤回"));
    }
    record.state = State::Withdrawn;
    record.updated_at = now_rfc3339();
    write_record(&server.state, &record)?;
    server.audit(&auth.key_id, "withdraw", sid, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(
            events::EVENT_TYPE_SUBMISSION_WITHDRAWN,
            sid,
            &server.channel,
        )
        .actor(&auth.key_id)
        .subject("plugin", &record.plugin_id, &record.version)
        .submission(sid),
    );
    Ok(json_response(
        200,
        &serde_json::to_value(&record).unwrap_or_default(),
    ))
}

/// 发布 job 用：列出某渠道的 `approved` 提交。
pub fn approved_for(state: &Path, channel: &str) -> Result<Vec<Submission>, ApiError> {
    Ok(all_records(state)?
        .into_iter()
        .filter(|r| r.state == State::Approved && r.channel == channel)
        .collect())
}

/// 发布 job 用：标记已发布并记录 `seq`（幂等：已是 published 时只补 release 信息）。
pub fn mark_published(state: &Path, sid: &str, seq: u64) -> Result<(), ApiError> {
    let _lock = SidLock::acquire(state, sid)?;
    let mut record = read_record(state, sid)?.ok_or_else(ApiError::internal)?;
    if record.state == State::Published {
        return Ok(());
    }
    if record.state != State::Approved {
        return Err(ApiError::precondition("只有 approved 提交可以标记发布"));
    }
    record.state = State::Published;
    record.updated_at = now_rfc3339();
    record.release = Some(ReleaseInfo {
        seq,
        at: record.updated_at.clone(),
    });
    write_record(state, &record)
}

/// 发布 job 用：给已发布提交回写撤包/吊销提示（按 `(plugin_id, version)` 命中）。
pub fn attach_takedown(
    state: &Path,
    plugin_id: &str,
    version: &str,
    info: &TakedownInfo,
) -> Result<usize, ApiError> {
    let mut count = 0;
    for mut record in all_records(state)? {
        if record.state != State::Published
            || record.plugin_id != plugin_id
            || record.version != version
        {
            continue;
        }
        if record.takedown.is_some() {
            continue;
        }
        record.takedown = Some(info.clone());
        record.updated_at = now_rfc3339();
        write_record(state, &record)?;
        count += 1;
    }
    Ok(count)
}

/// 包体路径（发布 job 复制到 `catalog/assets/`）。
pub fn package_path(state: &Path, sid: &str) -> PathBuf {
    submissions_dir(state).join(sid).join("package.wbpkg")
}
