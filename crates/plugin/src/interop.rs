//! 插件间接口调用（interop）的授权判定与目录（内核侧，**与 unix 无关**）。
//!
//! 一个插件要调用另一个插件，必须同时满足四件事：
//! 1. 自己有 `interop:call` 权限（安装时逐条确认）；
//! 2. 自己有 `plugin:<目标 id>` 对象授权（或 `plugin:*`）；
//! 3. 自己的 manifest `[interop].consumes` 白名单包含 `目标:方法`；
//! 4. 目标 manifest `[interop].provides` 声明了该方法。
//!
//! 四道闸都在这里判定，`kernel.sock` 只做鉴权、转发与审计——这样判定逻辑
//! 能在没有 socket / 没有数据库的环境里单测。防环（A→B→A）发生在内核守卫里，
//! 不属于本模块的纯判定。
//!
//! 目标插件仍需按自己的授权读数据/操作容器：interop 只转发 RPC，不代替目标
//! 的 `fs:`/`docker:` 判定。

// 这些函数只在 unix 上被 `kernel::imp` 调用；Windows 编译单元里没人用，
// `-D dead_code` 会误判。只在本模块放宽，Linux clippy 仍按真接线复核。
#![cfg_attr(not(unix), allow(dead_code))]

use skiff_pkgfmt::manifest::Manifest;
use skiff_store::{Grant, PluginRecord};

/// 调用跨插件接口所需的权限串（manifest `[[permissions]] capability = "interop"`）。
pub const PERM_CALL: &str = "interop:call";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyKind {
    Forbidden,
    NotFound,
    BadRequest,
    Conflict,
}

impl DenyKind {
    pub fn status(self) -> u16 {
        match self {
            DenyKind::Forbidden => 403,
            DenyKind::NotFound => 404,
            DenyKind::BadRequest => 400,
            DenyKind::Conflict => 409,
        }
    }

    /// 审计结果口径：越权记 `deny`，目标不存在/请求非法记 `fail`。
    pub fn audit_result(self) -> &'static str {
        match self {
            DenyKind::Forbidden | DenyKind::Conflict => "deny",
            DenyKind::NotFound | DenyKind::BadRequest => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteropDeny {
    pub kind: DenyKind,
    pub message: String,
}

impl InteropDeny {
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            kind: DenyKind::Forbidden,
            message: message.into(),
        }
    }
}

pub fn has_perm(grants: &[Grant], perm: &str) -> bool {
    grants.iter().any(|g| g.kind == "perm" && g.value == perm)
}

/// 是否持有目标插件（或全插件）的对象授权。
pub fn has_plugin_grant(grants: &[Grant], target: &str) -> bool {
    grants.iter().any(|g| {
        g.kind == "object" && (g.value == "plugin:*" || g.value == format!("plugin:{target}"))
    })
}

/// 四道闸的联合判定。`caller` 必须是已通过 UID+token 鉴权的插件 id。
pub fn authorize(
    caller: &str,
    caller_manifest: &Manifest,
    grants: &[Grant],
    target: &str,
    target_manifest: &Manifest,
    method: &str,
) -> Result<(), InteropDeny> {
    if caller == target {
        return Err(InteropDeny::forbidden("插件不能调用自己的接口"));
    }
    if !has_perm(grants, PERM_CALL) {
        return Err(InteropDeny::forbidden(format!(
            "缺少授权 {PERM_CALL}（安装时未勾选 interop:call）"
        )));
    }
    if !has_plugin_grant(grants, target) {
        return Err(InteropDeny::forbidden(format!(
            "缺少对象授权 plugin:{target}（安装时未勾选）"
        )));
    }
    if !caller_manifest.interop.allows(target, method) {
        return Err(InteropDeny::forbidden(format!(
            "{caller} 的 manifest 未声明 consumes {target}:{method}"
        )));
    }
    if !target_method_declared(target_manifest, method) {
        return Err(InteropDeny::forbidden(format!(
            "{target} 未在 manifest 中声明对外接口 {method}"
        )));
    }
    Ok(())
}

pub fn target_method_declared(manifest: &Manifest, method: &str) -> bool {
    manifest.interop.provides.iter().any(|p| p.method == method)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CatalogMethod {
    pub method: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub methods: Vec<CatalogMethod>,
}

/// 调用方**实际可调用**的接口目录：只列出同时满足 consumes 与 `plugin:` 授权的
/// 已启用插件。UI 据此只提供有效目标，避免用户选到「点了必 403」的项。
pub fn catalog(
    caller: &str,
    caller_manifest: &Manifest,
    grants: &[Grant],
    records: &[PluginRecord],
) -> Vec<CatalogEntry> {
    if !has_perm(grants, PERM_CALL) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for rec in records {
        if !rec.enabled || rec.id == caller || !has_plugin_grant(grants, &rec.id) {
            continue;
        }
        let Ok(manifest) = Manifest::parse(&rec.manifest) else {
            continue;
        };
        let methods: Vec<CatalogMethod> = manifest
            .interop
            .provides
            .iter()
            .filter(|p| caller_manifest.interop.allows(&rec.id, &p.method))
            .map(|p| CatalogMethod {
                method: p.method.clone(),
                label: p.label.clone().unwrap_or_else(|| p.method.clone()),
                description: p.description.clone().unwrap_or_default(),
            })
            .collect();
        if methods.is_empty() {
            continue;
        }
        out.push(CatalogEntry {
            id: rec.id.clone(),
            name: rec.name.clone(),
            methods,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_pkgfmt::Manifest;

    fn manifest(id: &str, provides: &[&str], consumes: &[&str]) -> Manifest {
        let provides = provides
            .iter()
            .map(|m| format!("{{ method = \"{m}\" }}"))
            .collect::<Vec<_>>()
            .join(", ");
        let consumes = consumes
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            r#"
id = "{id}"
name = "Demo"
version = "0.1.0"
key_id = "0000000000000000000000000000000000000000000000000000000000000001"
api = "1.0"
requires = ["interop", "identity"]

[[permissions]]
capability = "interop"
verbs = ["call"]

[[permissions]]
capability = "identity"
verbs = ["self"]

[interop]
provides = [{provides}]
consumes = [{consumes}]
"#
        );
        Manifest::parse(&text).unwrap()
    }

    fn grants() -> Vec<Grant> {
        vec![
            Grant::perm(PERM_CALL.to_string()),
            Grant::object("plugin:com.skiff.cron".to_string()),
        ]
    }

    fn caller() -> Manifest {
        manifest(
            "com.skiff.nginx",
            &["backup"],
            &["com.skiff.cron:jobs.create"],
        )
    }

    fn target() -> Manifest {
        manifest("com.skiff.cron", &["jobs.create"], &[])
    }

    #[test]
    fn happy_path_needs_all_four_checks() {
        assert!(authorize(
            "com.skiff.nginx",
            &caller(),
            &grants(),
            "com.skiff.cron",
            &target(),
            "jobs.create"
        )
        .is_ok());
    }

    #[test]
    fn each_missing_grant_is_rejected_with_its_own_reason() {
        let no_perm = vec![Grant::object("plugin:com.skiff.cron".to_string())];
        let e = authorize(
            "com.skiff.nginx",
            &caller(),
            &no_perm,
            "com.skiff.cron",
            &target(),
            "jobs.create",
        )
        .unwrap_err();
        assert!(e.message.contains("interop:call"), "{}", e.message);

        let no_object = vec![Grant::perm(PERM_CALL.to_string())];
        let e = authorize(
            "com.skiff.nginx",
            &caller(),
            &no_object,
            "com.skiff.cron",
            &target(),
            "jobs.create",
        )
        .unwrap_err();
        assert!(e.message.contains("plugin:com.skiff.cron"), "{}", e.message);

        // 白名单只写了一个方法：另一个方法必须拒
        let e = authorize(
            "com.skiff.nginx",
            &caller(),
            &grants(),
            "com.skiff.cron",
            &target(),
            "jobs.remove",
        )
        .unwrap_err();
        assert!(e.message.contains("consumes"), "{}", e.message);

        // 目标没声明该方法：白名单放行（用 `*`），仍应卡在 provides 这一道
        let wildcard = manifest("com.skiff.nginx", &["backup"], &["*"]);
        let e = authorize(
            "com.skiff.nginx",
            &wildcard,
            &grants(),
            "com.skiff.cron",
            &target(),
            "backup",
        )
        .unwrap_err();
        assert!(e.message.contains("未在 manifest 中声明"), "{}", e.message);
    }

    #[test]
    fn self_call_is_forbidden() {
        let e = authorize(
            "com.skiff.cron",
            &target(),
            &grants(),
            "com.skiff.cron",
            &target(),
            "jobs.create",
        )
        .unwrap_err();
        assert!(e.message.contains("自己"), "{}", e.message);
        assert_eq!(e.kind.status(), 403);
    }

    /// `plugin:*` 等价于对所有插件授权，但仍要过 consumes 白名单。
    #[test]
    fn plugin_star_covers_every_target() {
        let g = vec![
            Grant::perm(PERM_CALL.to_string()),
            Grant::object("plugin:*".to_string()),
        ];
        let scheduler = manifest("com.skiff.cron", &["jobs.create"], &["*"]);
        assert!(authorize(
            "com.skiff.cron",
            &scheduler,
            &g,
            "com.skiff.nginx",
            &caller(),
            "backup"
        )
        .is_ok());
        // 自调仍拒
        assert!(authorize(
            "com.skiff.cron",
            &scheduler,
            &g,
            "com.skiff.cron",
            &scheduler,
            "jobs.create"
        )
        .is_err());
    }

    /// 目录只列「真正调得通」的项：没声明 consumes 或没授权的目标不出现。
    #[test]
    fn catalog_lists_only_callable_interfaces() {
        let recs = vec![
            rec("com.skiff.cron", "计划任务", true, &target()),
            rec("com.skiff.other", "Other", true, &caller()),
            rec("com.skiff.cron", "计划任务", false, &target()),
        ];
        let list = catalog("com.skiff.nginx", &caller(), &grants(), &recs);
        assert_eq!(list.len(), 1, "{list:?}");
        assert_eq!(list[0].id, "com.skiff.cron");
        assert_eq!(list[0].methods[0].method, "jobs.create");

        // 没有 interop:call：目录为空（不泄露可调用面）
        assert!(catalog("com.skiff.nginx", &caller(), &[], &recs).is_empty());
    }

    fn rec(id: &str, name: &str, enabled: bool, m: &Manifest) -> PluginRecord {
        PluginRecord {
            id: id.into(),
            name: name.into(),
            version: "0.1.0".into(),
            api: "1.0".into(),
            key_id: "0".repeat(64),
            manifest: toml::to_string(m).unwrap(),
            enabled,
            status: "installed".into(),
            status_detail: String::new(),
            active_version: "0.1.0".into(),
            installed_at: 0,
            grants: Vec::new(),
        }
    }
}
