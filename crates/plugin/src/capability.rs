//! 插件能力面的 `fs` 路由实现（内核侧）。
//!
//! 这一层**不依赖 unix**：`kernel.sock` 的监听只存在于 unix（`skiff::kernel`），
//! 但路径判定、分页读写、审计这些逻辑与平台无关。把它们放在独立 crate，
//! Windows 上的单测也能真跑（否则「只在 Linux CI 编得到」的代码在开发机上
//! 连类型检查都过不了）。
//!
//! 权限与路径是**两把钥匙**：
//! - 权限串（`fs:read` / `fs:write` / `fs:list` / `fs:mkdir` / `fs:remove` / `fs:rename`）；
//! - 绝对路径（宿主文件）额外要求命中安装时确认的 `path:` 对象授权根。
//!
//! 判定与 IO 细节见 [`crate::fsops`]，这里只负责 HTTP 形态、权限与审计。
//!
//! **依赖倒置**：本模块住在 `skiff-plugin`（能力面的正确归属地），但 `skiff-plugin`
//! 不能依赖 `skiff-app`（后者依赖前者，会成环）。因此宿主上下文由
//! [`CapabilityHost`] 抽象，`skiff-app::AppState` 实现它。

// 这些 handler 只在 unix 上挂到 `kernel.sock` 的路由表；Windows 的
// 普通编译单元里没人调用它们，`-D dead_code` 会当成死代码——那是平台差异，不是真死。
// 只在本模块放宽，别扩大到整个 crate（Linux clippy 仍按真接线复核）。
#![cfg_attr(not(unix), allow(dead_code))]

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use http_body_util::BodyExt;
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
    fn plugin_grants(&self, plugin_id: &str) -> Vec<Grant>;
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

pub fn plugin_of(req: &Request<Body>) -> String {
    req.extensions()
        .get::<PluginId>()
        .map(|p| p.0.clone())
        .unwrap_or_default()
}

pub fn grants_of<H: CapabilityHost>(host: &H, id: &str) -> Vec<Grant> {
    host.plugin_grants(id)
}

pub fn has_perm<H: CapabilityHost>(host: &H, id: &str, perm: &str) -> bool {
    host.plugin_grants(id)
        .iter()
        .any(|g| g.kind == "perm" && g.value == perm)
}

pub fn deny<H: CapabilityHost>(host: &H, id: &str, perm: &str) -> Response {
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
    (StatusCode::FORBIDDEN, format!("缺少授权 {perm}")).into_response()
}

pub async fn read_json(req: Request<Body>) -> Value {
    let bytes = match req.into_body().collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => return Value::Null,
    };
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// fs 类失败统一出口：审计 + 状态码（越权记 `deny`，其余记 `fail`）。
fn fs_fail<H: CapabilityHost>(
    host: &H,
    id: &str,
    perm: &str,
    target: &str,
    e: fsops::FsError,
) -> Response {
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        &perm.replace(':', "."),
        target,
        e.audit_result(),
        e.message(),
    );
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, e.message().to_string()).into_response()
}

fn fs_ok<H: CapabilityHost>(host: &H, id: &str, perm: &str, target: &str) {
    host.audit(
        &format!("plugin:{id}"),
        "uds",
        &perm.replace(':', "."),
        target,
        "ok",
        "",
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
) -> Option<Response> {
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
        (
            StatusCode::FORBIDDEN,
            "受保护路径：内核数据根与插件目录不允许插件访问".to_string(),
        )
            .into_response(),
    )
}

/// 列父目录时隐藏受保护子项（FS-08）：整页报错会让插件连 `/www` 都列不了，
/// 因此按名字过滤 `items`，并据保留项重建 `entries`（两者同源，避免前端两套口径）。
fn filter_protected<H: CapabilityHost>(host: &H, dir: &Path, mut v: Value) -> Value {
    let data = host.data_dir();
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

pub async fn fs_read<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:read";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
    let max = body
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(fsops::MAX_IO as u64) as usize;
    let as_b64 = body.get("encoding").and_then(|v| v.as_str()) == Some("base64");

    let grants = grants_of(host, &id);
    let (path, scope) = match fsops::resolve(host.data_dir(), &id, &grants, raw, false) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    // 读同样会泄露内核机密（`skiff.db` 含口令哈希/会话/密钥），与写一并拦。
    if scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, raw, &path) {
            return r;
        }
    }
    let chunk = match fsops::read_chunk(&path, offset, max) {
        Ok(c) => c,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    fs_ok(host, &id, perm, raw);
    let mut out = json!({ "offset": chunk.offset, "size": chunk.size, "eof": chunk.eof });
    if as_b64 {
        out["b64"] = json!(base64::engine::general_purpose::STANDARD.encode(&chunk.bytes));
    } else {
        out["data"] = json!(String::from_utf8_lossy(&chunk.bytes));
    }
    Json(out).into_response()
}

pub async fn fs_write<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:write";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let bytes = match body.get("b64").and_then(|v| v.as_str()) {
        Some(b) => match base64::engine::general_purpose::STANDARD.decode(b) {
            Ok(d) => d,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("base64 解码失败: {e}")).into_response()
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
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("单次写入上限 {} 字节，请分页上传", fsops::MAX_IO),
        )
            .into_response();
    }
    let offset = body.get("offset").and_then(|v| v.as_u64());
    let append = body
        .get("append")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let grants = grants_of(host, &id);
    let (path, scope) = match fsops::resolve(host.data_dir(), &id, &grants, raw, true) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    if scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, raw, &path) {
            return r;
        }
    }
    let (wrote, size) = match fsops::write_chunk(&path, &bytes, offset, append) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    // 只在插件自己的 data/ 里把属主交还插件；宿主文件保持宿主原有属主（多为 root:root）。
    if scope == fsops::Scope::Data {
        fsops::own_by_plugin(&path, &id);
    }
    fs_ok(host, &id, perm, raw);
    Json(json!({ "ok": true, "wrote": wrote, "size": size })).into_response()
}

pub async fn fs_list<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:list";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, &id);
    let (path, scope) = match fsops::resolve(host.data_dir(), &id, &grants, raw, false) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    // 列受保护目录本身 → 403；列其父目录 → 过滤子项（不整页报错）。
    if scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, raw, &path) {
            return r;
        }
    }
    match fsops::list(&path) {
        Ok(v) => {
            let v = if scope == fsops::Scope::Host {
                filter_protected(host, &path, v)
            } else {
                v
            };
            fs_ok(host, &id, perm, raw);
            Json(v).into_response()
        }
        Err(e) => fs_fail(host, &id, perm, raw, e),
    }
}

/// 建目录（宿主或 data）。只建一层：多级目录由插件从授权根逐层下发，
/// 免得一次 `create_dir_all` 在宿主上悄悄铺开一棵树。
pub async fn fs_mkdir<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:mkdir";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, &id);
    let (path, scope) = match fsops::resolve(host.data_dir(), &id, &grants, raw, true) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    if scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, raw, &path) {
            return r;
        }
    }
    match fsops::make_dir(&path) {
        Ok(()) => {
            fs_ok(host, &id, perm, raw);
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => fs_fail(host, &id, perm, raw, e),
    }
}

/// 删除文件/目录。**授权根本身不许删**：授权一个目录 ≠ 授权毁掉这个目录。
pub async fn fs_remove<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:remove";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, &id);
    let (path, scope) = match fsops::resolve(host.data_dir(), &id, &grants, raw, false) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, raw, e),
    };
    if scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, raw, &path) {
            return r;
        }
    }
    if fsops::is_scope_root(host.data_dir(), &id, &grants, &path, scope) {
        return fs_fail(
            host,
            &id,
            perm,
            raw,
            fsops::FsError::Forbidden("不允许删除授权根目录本身".into()),
        );
    }
    match fsops::remove(&path) {
        Ok(was_dir) => {
            fs_ok(host, &id, perm, raw);
            Json(json!({ "ok": true, "dir": was_dir })).into_response()
        }
        Err(e) => fs_fail(host, &id, perm, raw, e),
    }
}

/// 改名/移动。源不得是授权根；目标已存在一律拒（覆盖式改名是误操作放大器）。
pub async fn fs_rename<H: CapabilityHost>(host: &H, req: Request<Body>) -> Response {
    let id = plugin_of(&req);
    let perm = "fs:rename";
    if !has_perm(host, &id, perm) {
        return deny(host, &id, perm);
    }
    let body = read_json(req).await;
    let from_raw = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let to_raw = body.get("to").and_then(|v| v.as_str()).unwrap_or("");
    let grants = grants_of(host, &id);
    let (from, from_scope) = match fsops::resolve(host.data_dir(), &id, &grants, from_raw, false) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, from_raw, e),
    };
    let (to, to_scope) = match fsops::resolve(host.data_dir(), &id, &grants, to_raw, true) {
        Ok(v) => v,
        Err(e) => return fs_fail(host, &id, perm, to_raw, e),
    };
    if from_scope != to_scope {
        return fs_fail(
            host,
            &id,
            perm,
            from_raw,
            fsops::FsError::Forbidden("不允许在 data/ 与宿主路径之间搬运".into()),
        );
    }
    if from_scope == fsops::Scope::Host {
        if let Some(r) = protected_deny(host, &id, from_raw, &from) {
            return r;
        }
        if let Some(r) = protected_deny(host, &id, to_raw, &to) {
            return r;
        }
    }
    if fsops::is_scope_root(host.data_dir(), &id, &grants, &from, from_scope) {
        return fs_fail(
            host,
            &id,
            perm,
            from_raw,
            fsops::FsError::Forbidden("不允许改名授权根目录本身".into()),
        );
    }
    match fsops::rename(&from, &to) {
        Ok(()) => {
            fs_ok(host, &id, perm, from_raw);
            Json(json!({ "ok": true, "to": to_raw })).into_response()
        }
        Err(e) => fs_fail(host, &id, perm, from_raw, e),
    }
}
