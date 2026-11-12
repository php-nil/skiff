//! 撤包与密钥吊销（§17.4/§17.7 R1）。
//!
//! 与审核一样，API 只写 `state/` 下的记录，**不改产物、不签名**；真正的源数据
//! 变更与 `seq` 递增由 `release` job 完成。yank 不可逆：恢复只能发新版本。
//!
//! 索引契约不升版本：`plugins[].yanked` 与 `revoked[]` 是 v1 既有字段，R1 只是
//! 开始填充。已发布条目只增不删，撤包是打标而不是物理删除，`seq` 因此继续单调。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::auth::Authenticated;
use super::store::EntryKind;
use super::{json_response, now_rfc3339, ApiError, ApiServer};
use crate::events::{self, Event};
use crate::http::Response;

pub const TAKEDOWN_SCHEMA: &str = "skiff.server.takedown/v1";
pub const REVOKED_KEYS_SCHEMA: &str = "skiff.server.revoked-keys/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 单版本下架：源条目 `yanked = true`。
    Yank,
    /// 密钥吊销波及的版本：`yanked = true` 且进入 `revoked[]`。
    Revoke,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Yank => "yank",
            Kind::Revoke => "revoke",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pending,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Takedown {
    pub schema: String,
    pub id: String,
    pub kind: Kind,
    /// 撤包目标的条目类型（R2e）；插件为缺省，旧记录无需迁移。
    #[serde(default, skip_serializing_if = "EntryKind::is_plugin")]
    pub entry_kind: EntryKind,
    pub plugin_id: String,
    pub version: String,
    pub reason: String,
    pub channel: String,
    /// 发起撤包/吊销的审核钥。
    pub key_id: String,
    pub at: String,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ReleaseStamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseStamp {
    pub seq: u64,
    pub at: String,
}

/// `state/revoked_keys.json` 的一条：只用于鉴权拦截，产物侧由撤包记录驱动。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokedKey {
    pub key_id: String,
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokedKeysFile {
    schema: String,
    keys: Vec<RevokedKey>,
}

fn dir(state: &Path) -> PathBuf {
    state.join("takedowns")
}

fn record_path(state: &Path, id: &str) -> PathBuf {
    dir(state).join(format!("{id}.json"))
}

/// 记录 id：插件保持 R1 的派生式不变（旧记录继续命中）；其它 kind 加类型域前缀。
pub fn record_id(entry_kind: EntryKind, kind: Kind, id: &str, version: &str) -> String {
    let digest = if entry_kind.is_plugin() {
        Sha256::digest(format!("{}\0{id}\0{version}", kind.as_str()).as_bytes())
    } else {
        Sha256::digest(
            format!(
                "{}\0{}\0{id}\0{version}",
                kind.as_str(),
                entry_kind.as_str()
            )
            .as_bytes(),
        )
    };
    format!("tak-{}", &hex::encode(digest)[..16])
}

fn read_record(state: &Path, id: &str) -> Result<Option<Takedown>, ApiError> {
    match fs::read(record_path(state, id)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ApiError::internal()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ApiError::internal()),
    }
}

fn write_record(state: &Path, record: &Takedown) -> Result<(), ApiError> {
    fs::create_dir_all(dir(state)).map_err(|_| ApiError::internal())?;
    let path = record_path(state, &record.id);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| ApiError::internal())?;
    fs::write(&tmp, bytes).map_err(|_| ApiError::internal())?;
    fs::rename(&tmp, &path).map_err(|_| ApiError::internal())
}

/// 单条撤包/吊销的并发锁（与提交的 `<sid>.lock` 同一思路）。
struct RecordLock(PathBuf);

impl RecordLock {
    fn acquire(state: &Path, id: &str) -> Result<Self, ApiError> {
        fs::create_dir_all(dir(state)).map_err(|_| ApiError::internal())?;
        let path = dir(state).join(format!("{id}.lock"));
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => Ok(Self(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ApiError::precondition("takedown is being processed"))
            }
            Err(_) => Err(ApiError::internal()),
        }
    }
}

impl Drop for RecordLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// POST /api/v1/{plugins|apps|runtimes}/…/yank
///
/// 插件沿用 R1 语义（`yanked` 打标）；应用与运行环境没有作者密钥，撤包同时把该版本
/// 登记到各自的 `revoked_apps[]` / `revoked_runtimes[]` 命名空间。
pub fn yank(
    server: &ApiServer,
    auth: &Authenticated,
    entry_kind: EntryKind,
    id: &str,
    version: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    auth.require_reviewer()?;
    let reason = optional_reason(body)?.unwrap_or_else(|| "manual takedown".to_string());
    let takedown_id = record_id(entry_kind, Kind::Yank, id, version);
    let _lock = RecordLock::acquire(&server.state, &takedown_id)?;

    if let Some(existing) = read_record(&server.state, &takedown_id)? {
        server.audit(&auth.key_id, "yank", &existing.id, "duplicate");
        return Ok(json_response(
            200,
            &serde_json::to_value(&existing).unwrap_or_default(),
        ));
    }
    // 源数据必须真实存在该版本；否则只会得到一份空操作。
    let Some(entry) = find_source_entry(&server.catalog, &server.channel, entry_kind, id, version)?
    else {
        return Err(ApiError::new(404, "not_found", "not found"));
    };
    if entry
        .get("yanked")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        // 已经撤回（可能来自密钥吊销或运维改源数据）：无可再撤。
        return Err(ApiError::conflict("该版本已撤回"));
    }
    if revoked_in_source(&server.catalog, &server.channel, entry_kind, id, version)? {
        return Err(ApiError::conflict("该版本已在吊销列表中"));
    }

    let record = Takedown {
        schema: TAKEDOWN_SCHEMA.to_string(),
        id: takedown_id.clone(),
        kind: Kind::Yank,
        entry_kind,
        plugin_id: id.to_string(),
        version: version.to_string(),
        reason,
        channel: server.channel.clone(),
        key_id: auth.key_id.clone(),
        at: now_rfc3339(),
        state: State::Pending,
        release: None,
    };
    write_record(&server.state, &record)?;
    server.audit(&auth.key_id, "yank", &takedown_id, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(
            events::EVENT_TYPE_TAKEDOWN_YANKED,
            &takedown_id,
            &server.channel,
        )
        .actor(&auth.key_id)
        .subject(entry_kind.as_str(), id, version)
        .submission(&takedown_id)
        .reason(&record.reason),
    );
    Ok(json_response(
        202,
        &serde_json::to_value(&record).unwrap_or_default(),
    ))
}

/// POST /api/v1/keys/{key_id}/revoke
///
/// 吊销只增不减：重复吊销返回 200 同一条。吊销立即生效（鉴权层直接拒绝），
/// 历史版本进入待发布队列，由 `release` 写 `revoked[]` 并加 `yanked`。
pub fn revoke_key(
    server: &ApiServer,
    auth: &Authenticated,
    key_id: &str,
    body: &[u8],
) -> Result<Response, ApiError> {
    auth.require_reviewer()?;
    let reason = optional_reason(body)?.unwrap_or_else(|| "key revoked".to_string());
    let mut file = read_revoked_keys(&server.state)?;
    if let Some(existing) = file.keys.iter().find(|k| k.key_id == key_id) {
        server.audit(&auth.key_id, "revoke", key_id, "duplicate");
        return Ok(json_response(
            200,
            &serde_json::to_value(existing).unwrap_or_default(),
        ));
    }
    // 只允许吊销已登记的主体（作者或审核者），避免写出无主记录。
    let registered = super::auth::key_registered(server, key_id)?;
    if !registered {
        return Err(ApiError::new(404, "not_found", "not found"));
    }
    let entry = RevokedKey {
        key_id: key_id.to_string(),
        reason: reason.clone(),
        at: now_rfc3339(),
    };
    file.keys.push(entry.clone());
    write_revoked_keys(&server.state, &file)?;

    // 该 key 的历史版本全部登记为待撤：产物侧只增不删，索引里打 `yanked` + `revoked`。
    let at = now_rfc3339();
    for (plugin_id, version, owner) in entries_by_key(&server.catalog, &server.channel)? {
        if owner != key_id {
            continue;
        }
        let id = record_id(EntryKind::Plugin, Kind::Revoke, &plugin_id, &version);
        if read_record(&server.state, &id)?.is_some() {
            continue;
        }
        let record = Takedown {
            schema: TAKEDOWN_SCHEMA.to_string(),
            id,
            kind: Kind::Revoke,
            entry_kind: EntryKind::Plugin,
            plugin_id,
            version,
            reason: reason.clone(),
            channel: server.channel.clone(),
            key_id: auth.key_id.clone(),
            at: at.clone(),
            state: State::Pending,
            release: None,
        };
        write_record(&server.state, &record)?;
    }

    server.audit(&auth.key_id, "revoke", key_id, "ok");
    let _ = events::emit(
        &server.state,
        &Event::new(events::EVENT_TYPE_KEY_REVOKED, key_id, &server.channel)
            .actor(&auth.key_id)
            .submission(key_id)
            .reason(&entry.reason),
    );
    Ok(json_response(
        202,
        &serde_json::to_value(&entry).unwrap_or_default(),
    ))
}

/// 该 key 是否已被吊销（鉴权层调用；读失败即 503，失败关闭）。
pub fn revoked_key(state: &Path, key_id: &str) -> Result<bool, ApiError> {
    Ok(read_revoked_keys(state)?
        .keys
        .iter()
        .any(|k| k.key_id == key_id))
}

fn read_revoked_keys(state: &Path) -> Result<RevokedKeysFile, ApiError> {
    let path = state.join("revoked_keys.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::new(503, "unavailable", "revoked_keys.json 非法")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RevokedKeysFile {
            schema: REVOKED_KEYS_SCHEMA.to_string(),
            keys: Vec::new(),
        }),
        Err(_) => Err(ApiError::internal()),
    }
}

fn write_revoked_keys(state: &Path, file: &RevokedKeysFile) -> Result<(), ApiError> {
    let path = state.join("revoked_keys.json");
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(file).map_err(|_| ApiError::internal())?;
    fs::write(&tmp, bytes).map_err(|_| ApiError::internal())?;
    fs::rename(&tmp, &path).map_err(|_| ApiError::internal())?;
    Ok(())
}

fn optional_reason(body: &[u8]) -> Result<Option<String>, ApiError> {
    if body.is_empty() {
        return Ok(None);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        #[serde(default)]
        reason: Option<String>,
    }
    let parsed: Body =
        serde_json::from_slice(body).map_err(|_| ApiError::bad_request("body 不是合法 JSON"))?;
    match parsed.reason {
        Some(reason) => {
            let reason = reason.trim().to_string();
            if reason.is_empty() || reason.chars().count() > 200 {
                return Err(ApiError::bad_request("reason 必须是 1..=200 个字符"));
            }
            Ok(Some(reason))
        }
        None => Ok(None),
    }
}

pub fn pending(state: &Path) -> Result<Vec<Takedown>, ApiError> {
    let Ok(entries) = fs::read_dir(dir(state)) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(record) = serde_json::from_slice::<Takedown>(&bytes) else {
            continue;
        };
        if record.state == State::Pending {
            out.push(record);
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// 发布 job 专用：把记录标记为已随某个 `seq` 发布。
///
/// 不加锁：job 是 `Pending → Released` 的唯一写入者；并发 yank 命中同一条记录时
/// 只会读到旧状态并走 200 幂等分支，不会造成重复发布。
pub fn mark_released(state: &Path, id: &str, seq: u64) -> Result<(), ApiError> {
    let mut record = read_record(state, id)?.ok_or_else(ApiError::internal)?;
    if record.state == State::Released {
        return Ok(());
    }
    record.state = State::Released;
    record.release = Some(ReleaseStamp {
        seq,
        at: now_rfc3339(),
    });
    write_record(state, &record)
}

/// 源数据目录名：插件 / 应用 / 运行环境。
fn source_dir(entry_kind: EntryKind) -> &'static str {
    match entry_kind {
        EntryKind::Plugin => "plugins",
        EntryKind::App => "apps",
        EntryKind::Runtime => "runtimes",
    }
}

/// 源条目里表示「条目 id」的字段：运行环境用 `kind`，其余用 `id`。
fn source_id_field(entry_kind: EntryKind) -> &'static str {
    match entry_kind {
        EntryKind::Runtime => "kind",
        _ => "id",
    }
}

/// 源数据里的条目（按 kind 目录 + id 字段），用于确认撤包目标真实存在。
fn find_source_entry(
    catalog: &Path,
    channel: &str,
    entry_kind: EntryKind,
    id: &str,
    version: &str,
) -> Result<Option<Value>, ApiError> {
    let dir = catalog.join(channel).join(source_dir(entry_kind));
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if value
            .get(source_id_field(entry_kind))
            .and_then(Value::as_str)
            == Some(id)
            && value.get("version").and_then(Value::as_str) == Some(version)
        {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn revocation_file(entry_kind: EntryKind) -> &'static str {
    match entry_kind {
        EntryKind::Plugin => "revoked.json",
        EntryKind::App => "revoked_apps.json",
        EntryKind::Runtime => "revoked_runtimes.json",
    }
}

fn revoked_in_source(
    catalog: &Path,
    channel: &str,
    entry_kind: EntryKind,
    id: &str,
    version: &str,
) -> Result<bool, ApiError> {
    let name = revocation_file(entry_kind);
    let path = catalog.join(channel).join(name);
    let Ok(bytes) = fs::read(&path) else {
        return Ok(false);
    };
    let list: Vec<Value> = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::new(503, "unavailable", format!("{name} 非法")))?;
    Ok(list.iter().any(|v| {
        v.get("id").and_then(Value::as_str) == Some(id)
            && v.get("version").and_then(Value::as_str) == Some(version)
    }))
}

/// 发布 job 用：把 yanked 标记与原因写回源条目（只改这两个字段，保留其余内容）。
pub fn mark_source_yanked(
    catalog: &Path,
    channel: &str,
    entry_kind: EntryKind,
    id: &str,
    version: &str,
    reason: &str,
) -> Result<(), String> {
    let dir = catalog.join(channel).join(source_dir(entry_kind));
    let entries = fs::read_dir(&dir).map_err(|e| format!("读取源条目失败: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if value
            .get(source_id_field(entry_kind))
            .and_then(Value::as_str)
            != Some(id)
            || value.get("version").and_then(Value::as_str) != Some(version)
        {
            continue;
        }
        if value.get("yanked").and_then(Value::as_bool) == Some(true)
            && value.get("yank_reason").and_then(Value::as_str).is_some()
        {
            return Ok(());
        }
        value["yanked"] = Value::Bool(true);
        if !reason.is_empty() {
            value["yank_reason"] = Value::String(reason.to_string());
        }
        let mut out = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        out.push(b'\n');
        fs::write(&path, out).map_err(|e| format!("写回源条目失败: {e}"))?;
        return Ok(());
    }
    Err(format!("源数据中找不到 {id} {version}"))
}

/// 发布 job 用：把吊销条目追加到该 kind 的吊销源文件（按 id+version 去重）。
pub fn append_source_revocation(
    catalog: &Path,
    channel: &str,
    record: &Takedown,
    sha256: Option<&str>,
) -> Result<(), String> {
    let entry_kind = record.entry_kind;
    let (id, version, reason, at) = (
        record.plugin_id.as_str(),
        record.version.as_str(),
        record.reason.as_str(),
        record.at.as_str(),
    );
    let name = revocation_file(entry_kind);
    let path = catalog.join(channel).join(name);
    let mut list: Vec<Value> = if path.is_file() {
        let bytes = fs::read(&path).map_err(|e| format!("读取 {name} 失败: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("解析 {name} 失败: {e}"))?
    } else {
        Vec::new()
    };
    if list.iter().any(|v| {
        v.get("id").and_then(Value::as_str) == Some(id)
            && v.get("version").and_then(Value::as_str) == Some(version)
    }) {
        return Ok(());
    }
    let mut entry = json!({
        "id": id,
        "version": version,
        "reason": reason,
        "at": at,
    });
    if let Some(sha) = sha256 {
        entry["sha256"] = Value::String(sha.to_string());
    }
    list.push(entry);
    let mut out = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
    out.push(b'\n');
    fs::write(&path, out).map_err(|e| format!("写入 {name} 失败: {e}"))
}

/// 发布 job 用：源条目对应的摘要（应用取 compose 摘要，运行环境取镜像 digest 的 hex 部分）。
pub fn source_digest(
    catalog: &Path,
    channel: &str,
    entry_kind: EntryKind,
    id: &str,
    version: &str,
) -> Result<Option<String>, String> {
    let Some(value) = find_source_entry(catalog, channel, entry_kind, id, version)
        .map_err(|_| "读取源条目失败".to_string())?
    else {
        return Ok(None);
    };
    let digest = match entry_kind {
        // 应用源条目只有 `asset = assets/<sha>.yaml`，摘要从文件名取。
        EntryKind::App => value
            .get("asset")
            .and_then(Value::as_str)
            .and_then(|asset| asset.rsplit('/').next())
            .and_then(|name| name.strip_suffix(".yaml"))
            .map(ToString::to_string),
        // 运行环境存的是 `sha256:<hex>`，吊销列表按 hex64 口径。
        EntryKind::Runtime => value
            .get("image_digest")
            .and_then(Value::as_str)
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .map(ToString::to_string),
        EntryKind::Plugin => value
            .get("pkg_sha256")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    };
    Ok(digest)
}

/// 发布 job / 吊销登记用：某渠道源数据里的全部插件条目。
pub fn entries_by_key(
    catalog: &Path,
    channel: &str,
) -> Result<Vec<(String, String, String)>, ApiError> {
    let dir = catalog.join(channel).join("plugins");
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
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let (Some(id), Some(version), Some(key_id)) = (
            value.get("id").and_then(Value::as_str),
            value.get("version").and_then(Value::as_str),
            value.get("key_id").and_then(Value::as_str),
        ) else {
            continue;
        };
        out.push((id.to_string(), version.to_string(), key_id.to_string()));
    }
    Ok(out)
}
