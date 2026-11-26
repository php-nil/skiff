//! 插件能力面的 `fs` 实现（内核侧，**不含 HTTP**）。
//!
//! 入参是已解析的 JSON 体（`&Value`），出参是 `Result<Value, CapError>`：权限判定、
//! 路径解析、分页读写与审计都在这里；**状态码映射与响应组装在**
//! `crates/core/src/kernel/capability.rs`（口径见 `docs/sections/08-engineering.md`
//! 的「能力面 × 实现位置」表）。因此本 crate 不依赖 HTTP 框架，判定与编排在
//! Windows 开发机上也能真跑单测。
//!
//! 权限与路径是**两把钥匙**：
//! - 权限串（`fs:read` / `fs:write` / `fs:list` / `fs:mkdir` / `fs:remove` / `fs:rename`）；
//! - 绝对路径（宿主文件）额外要求命中安装时确认的 `path:` 对象授权根。
//!
//! 判定与 IO 细节见 [`crate::fsops`]，这里只负责权限、编排与审计。
//!
//! **依赖倒置**：本模块住在 `skiff-plugin`（能力面的正确归属地），但 `skiff-plugin`
//! 不能依赖 `skiff-app`（后者依赖前者，会成环）。因此宿主上下文由
//! [`CapabilityHost`] 抽象，`skiff-app::AppState` 实现它。

use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};
use skiff_store::Grant;

use crate::{fsgrant, fsops};

/// 能力面需要的宿主上下文。
///
/// 最小接口：能力面只要「插件数据根在哪」「这个插件被授了哪些权限」
/// 「审计往哪写」这三件事，不关心数据库与状态是怎么组装的。
pub trait CapabilityHost {
    /// 内核数据根（生产 `/www/skiff`；插件私有数据在其下 `plugins/<id>/data`）。
    fn data_dir(&self) -> &Path;
    /// 该插件已授予的权限（安装时逐条确认的结果）。
    ///
    /// 返回 `Arc` 而不是 `Vec`：宿主实现通常持有进程内快照，热路径上不应每次
    /// 复制整组 grants；一次能力调用会问多次（权限判定、路径判定、审计范围）。
    fn plugin_grants(&self, plugin_id: &str) -> Arc<[Grant]>;
    /// 审计落库。
    fn audit(
        &self,
        actor: &str,
        channel: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    );
}

/// 已通过双重鉴权的插件 id（由内核的 `plugin_auth` 中间件插入 request extension）。
#[derive(Clone, Debug)]
pub struct PluginId(pub String);

/// 能力面结果。
///
/// 只带「状态码数字 + 消息」，不碰任何 HTTP 类型：传输层按 [`CapError::status`]
/// 自己造响应（HTTP 4xx/5xx、CLI 退出码、SDUI 提示各管各的）。原因到状态码的
/// 对照仍由 [`fsops::FsError`] 持有，见 `docs/contract/KERNEL_FS_CONTRACT.md` §7.3。
#[derive(Debug)]
pub enum CapError {
    /// `fs` 判定或 IO 失败；审计已由调用点落库。
    Fs(fsops::FsError),
    /// 请求体本身不合法（base64 解码失败、单次写入超限）：装配层直接拒绝，**不落审计**。
    Rejected { status: u16, message: String },
}

impl CapError {
    pub fn status(&self) -> u16 {
        match self {
            Self::Fs(e) => e.status(),
            Self::Rejected { status, .. } => *status,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Fs(e) => e.message(),
            Self::Rejected { message, .. } => message,
        }
    }
}

impl From<fsops::FsError> for CapError {
    fn from(e: fsops::FsError) -> Self {
        Self::Fs(e)
    }
}

fn grants_of<H: CapabilityHost>(host: &H, id: &str) -> Arc<[Grant]> {
    host.plugin_grants(id)
}

fn has_perm<H: CapabilityHost>(host: &H, id: &str, perm: &str) -> bool {
    host.plugin_grants(id).iter().any(|g| g.permits(perm))
}

/// 缺授权：审计后返回拒绝（响应由传输层组装）。
fn deny<H: CapabilityHost>(host: &H, id: &str, perm: &str) -> CapError {
    // 权限名是 `fs:read` 形态，审计动作统一用点分（`fs.read`）——否则按 action
    // 过滤审计时同一个能力面会出现冒号与点两套拼法。
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        &perm.replace(':', "."),
        "",
        "deny",
        "缺少授权",
    );
    fsops::FsError::Forbidden(format!("缺少授权 {perm}")).into()
}

/// 审计附加字段：命中的授权根 + 站点域名。
///
/// 站点内容目录只拿 `path:/www/sites/<域名>/...`，能力面审计主体是 `plugin:<id>`；
/// 为了让定责可读，把「哪个授权根、哪个站点」写进 `detail`。这只影响审计文本，
/// 不改变授权判定（越界仍由 `fsops::resolve` 拒绝）。
fn audit_scope<H: CapabilityHost>(host: &H, id: &str, target: &str) -> String {
    let roots = fsgrant::roots_of(&host.plugin_grants(id));
    let root = roots
        .iter()
        .filter(|r| target == r.as_str() || target.starts_with(&format!("{r}/")))
        .max_by_key(|r| r.len())
        .cloned()
        .unwrap_or_default();
    let site = target
        .strip_prefix("/www/sites/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("");
    let mut out = String::new();
    if !root.is_empty() {
        out.push_str(&format!("grant={root}"));
    }
    if !site.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("site={site}"));
    }
    out
}

/// fs 类失败统一出口：审计 + 把错误原样交给传输层（越权记 `deny`，其余记 `fail`）。
fn fs_fail<H: CapabilityHost>(
    host: &H,
    id: &str,
    perm: &str,
    target: &str,
    e: fsops::FsError,
) -> CapError {
    let scope = audit_scope(host, id, target);
    let mut detail = e.message().to_string();
    if !scope.is_empty() {
        detail.push_str("; ");
        detail.push_str(&scope);
    }
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        &perm.replace(':', "."),
        target,
        e.audit_result(),
        &detail,
    );
    e.into()
}

fn fs_ok<H: CapabilityHost>(host: &H, id: &str, perm: &str, target: &str) {
    let scope = audit_scope(host, id, target);
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        &perm.replace(':', "."),
        target,
        "ok",
        scope.as_str(),
    );
}

/// 内核自有目录的保护（FS-08）：即便 `path:` 授权根覆盖（如 `path:/www` 含
/// `/www/skiff`），也不允许插件读/写/删/改名/列目录本身，并记一条 `fs.protected`。
/// 列父目录不在这里拒——受保护子项由 [`filter_protected`] 过滤掉。
fn protected_deny<H: CapabilityHost>(
    host: &H,
    id: &str,
    target: &str,
    path: &Path,
) -> Option<CapError> {
    if !fsgrant::is_protected(host.data_dir(), path) {
        return None;
    }
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        "fs.protected",
        target,
        "deny",
        "受保护路径（内核数据根/插件目录），拒绝访问",
    );
    Some(
        fsops::FsError::Forbidden("受保护路径：内核数据根与插件目录不允许插件访问".to_string())
            .into(),
    )
}

/// 列父目录时隐藏受保护子项（FS-08）：整页报错会让插件连 `/www` 都列不了，
/// 因此按名字过滤 `items`，并据保留项重建 `entries`（两者同源，避免前端两套口径）。
fn filter_protected(data: &Path, dir: &Path, mut v: Value) -> Value {
    let items: Vec<Value> = v
        .get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|item| {
                    item.get("name")
                        .and_then(|n| n.as_str())
                        .map(|name| !fsgrant::hides_listed_entry(data, &dir.join(name)))
                        .unwrap_or(true)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let entries: Vec<String> = items
        .iter()
        .filter_map(|i| i.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    if let Some(obj) = v.as_object_mut() {
        obj.insert("entries".to_string(), json!(entries));
        obj.insert("items".to_string(), json!(items));
    }
    v
}

/// 把可能阻塞的文件系统调用挪到阻塞线程池。
///
/// 能力面 handler 跑在 tokio worker 上；`read_dir`、整文件写的
/// `sync_all`（临时文件 + fsync + rename）以及大目录逐项 `canonicalize`
/// 都可能在慢盘上阻塞数百毫秒，直接在 worker 上做会拖住内核的其它请求。
async fn offload_fs<T, F>(f: F) -> Result<T, fsops::FsError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, fsops::FsError> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r,
        Err(e) => Err(fsops::FsError::Io(format!("文件操作后台任务失败: {e}"))),
    }
}

pub async fn fs_read<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:read";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
    let max = body
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(fsops::MAX_IO as u64) as usize;
    let as_b64 = body.get("encoding").and_then(|v| v.as_str()) == Some("base64");

    let grants = grants_of(host, id);
    let (path, scope) = fsops::resolve(host.data_dir(), id, &grants, raw, false)
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    // 读同样会泄露内核机密（`skiff.db` 含口令哈希/会话/密钥），与写一并拦。
    if scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, raw, &path) {
            return Err(e);
        }
    }
    let io_path = path.clone();
    let chunk = offload_fs(move || fsops::read_chunk(&io_path, offset, max))
        .await
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    fs_ok(host, id, perm, raw);
    let mut out = json!({ "offset": chunk.offset, "size": chunk.size, "eof": chunk.eof });
    if as_b64 {
        out["b64"] = json!(base64::engine::general_purpose::STANDARD.encode(&chunk.bytes));
    } else {
        out["data"] = json!(String::from_utf8_lossy(&chunk.bytes));
    }
    Ok(out)
}

pub async fn fs_write<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:write";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let bytes = match body.get("b64").and_then(|v| v.as_str()) {
        Some(b) => match base64::engine::general_purpose::STANDARD.decode(b) {
            Ok(d) => d,
            Err(e) => {
                return Err(CapError::Rejected {
                    status: 400,
                    message: format!("base64 解码失败: {e}"),
                })
            }
        },
        None => body
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .as_bytes()
            .to_vec(),
    };
    if bytes.len() > fsops::MAX_IO {
        return Err(CapError::Rejected {
            status: 413,
            message: format!("单次写入上限 {} 字节，请分页上传", fsops::MAX_IO),
        });
    }
    let offset = body.get("offset").and_then(|v| v.as_u64());
    let append = body
        .get("append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let grants = grants_of(host, id);
    let (path, scope) = fsops::resolve(host.data_dir(), id, &grants, raw, true)
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    if scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, raw, &path) {
            return Err(e);
        }
    }
    let io_path = path.clone();
    let owner_id = id.to_string();
    let owned = scope == fsops::Scope::Data;
    let (wrote, size) = offload_fs(move || {
        let r = fsops::write_chunk(&io_path, &bytes, offset, append)?;
        // 只在插件自己的 data/ 里把属主交还插件；宿主文件保持宿主原有属主（多为 root:root）。
        if owned {
            fsops::own_by_plugin(&io_path, &owner_id);
        }
        Ok(r)
    })
    .await
    .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    fs_ok(host, id, perm, raw);
    Ok(json!({ "ok": true, "wrote": wrote, "size": size }))
}

pub async fn fs_list<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:list";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, id);
    let (path, scope) = fsops::resolve(host.data_dir(), id, &grants, raw, false)
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    // 列受保护目录本身 → 403；列其父目录 → 过滤子项（不整页报错）。
    if scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, raw, &path) {
            return Err(e);
        }
    }
    let io_path = path.clone();
    let data_dir = host.data_dir().to_path_buf();
    let is_host = scope == fsops::Scope::Host;
    let listed = offload_fs(move || {
        let v = fsops::list(&io_path)?;
        Ok(if is_host {
            filter_protected(&data_dir, &io_path, v)
        } else {
            v
        })
    })
    .await
    .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    fs_ok(host, id, perm, raw);
    Ok(listed)
}

/// 建目录（宿主或 data）。只建一层：多级目录由插件从授权根逐层下发，
/// 免得一次 `create_dir_all` 在宿主上悄悄铺开一棵树。
pub async fn fs_mkdir<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:mkdir";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, id);
    let (path, scope) = fsops::resolve(host.data_dir(), id, &grants, raw, true)
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    if scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, raw, &path) {
            return Err(e);
        }
    }
    let io_path = path.clone();
    offload_fs(move || fsops::make_dir(&io_path))
        .await
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    fs_ok(host, id, perm, raw);
    Ok(json!({ "ok": true }))
}

/// 删除文件/目录。**授权根本身不许删**：授权一个目录 ≠ 授权毁掉这个目录。
pub async fn fs_remove<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:remove";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, id);
    let (path, scope) = fsops::resolve(host.data_dir(), id, &grants, raw, false)
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    if scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, raw, &path) {
            return Err(e);
        }
    }
    if fsops::is_scope_root(host.data_dir(), id, &grants, &path, scope) {
        return Err(fs_fail(
            host,
            id,
            perm,
            raw,
            fsops::FsError::Forbidden("不允许删除授权根目录本身".into()),
        ));
    }
    let io_path = path.clone();
    let was_dir = offload_fs(move || fsops::remove(&io_path))
        .await
        .map_err(|e| fs_fail(host, id, perm, raw, e))?;
    fs_ok(host, id, perm, raw);
    Ok(json!({ "ok": true, "dir": was_dir }))
}

/// 改名/移动。源不得是授权根；目标已存在一律拒（覆盖式改名是误操作放大器）。
pub async fn fs_rename<H: CapabilityHost>(
    host: &H,
    id: &str,
    body: &Value,
) -> Result<Value, CapError> {
    let perm = "fs:rename";
    if !has_perm(host, id, perm) {
        return Err(deny(host, id, perm));
    }
    let from_raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let to_raw = body.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, id);
    let (from, from_scope) = fsops::resolve(host.data_dir(), id, &grants, from_raw, false)
        .map_err(|e| fs_fail(host, id, perm, from_raw, e))?;
    let (to, to_scope) = fsops::resolve(host.data_dir(), id, &grants, to_raw, true)
        .map_err(|e| fs_fail(host, id, perm, to_raw, e))?;
    if from_scope != to_scope {
        return Err(fs_fail(
            host,
            id,
            perm,
            from_raw,
            fsops::FsError::Forbidden("不允许在 data/ 与宿主路径之间搬运".into()),
        ));
    }
    if from_scope == fsops::Scope::Host {
        if let Some(e) = protected_deny(host, id, from_raw, &from) {
            return Err(e);
        }
        if let Some(e) = protected_deny(host, id, to_raw, &to) {
            return Err(e);
        }
    }
    if fsops::is_scope_root(host.data_dir(), id, &grants, &from, from_scope) {
        return Err(fs_fail(
            host,
            id,
            perm,
            from_raw,
            fsops::FsError::Forbidden("不允许改名授权根本身".into()),
        ));
    }
    offload_fs(move || fsops::rename(&from, &to))
        .await
        .map_err(|e| fs_fail(host, id, perm, from_raw, e))?;
    fs_ok(host, id, perm, from_raw);
    Ok(json!({ "ok": true, "to": to_raw }))
}
