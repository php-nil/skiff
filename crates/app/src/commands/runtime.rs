//! 运行环境领域命令（对应 `actions::Resource::Runtime` 的 15 个动作）。
//!
//! 与站点组同一口径：DTO 只带业务输入与调用主体，审计随命令产生；表单解析、
//! 文案、跳转与 HTTP 状态由 `handler` 负责。

use std::sync::Arc;

use skiff_model::Role;
use skiff_store::Runtime;

use super::Actor;
use crate::sites::{
    NodeRuntimeConfig, PhpConfigFile, PhpRuntimeConfig, RuntimeDeleteReport, SiteError,
};
use crate::AppState;

/// 命令名（与 `actions` 表里的 `command` 后缀一一对应），供漂移测试使用。
pub const NAMES: &[&str] = &[
    "create",
    "save_images",
    "save_config",
    "save_node_config",
    "save_config_file",
    "save_port",
    "clone",
    "action",
    "save_extensions",
    "install_extension",
    "uninstall_extension",
    "batch_extensions",
    "install_pecl",
    "probe",
    "unbind_site",
];

/// 新建运行环境。`kind` 为空时落 `php`（旧表单的默认值），由命令兜底。
#[derive(Debug, Clone)]
pub struct CreateRuntime {
    pub kind: String,
    pub name: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub actor: Actor,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct CreatedRuntime {
    pub runtime: Runtime,
    /// 是否已提交安装任务；未实现的类型只登记、不装容器。
    pub started: bool,
}

pub async fn create(
    state: &Arc<AppState>,
    cmd: CreateRuntime,
) -> Result<CreatedRuntime, SiteError> {
    let kind = if cmd.kind.trim().is_empty() {
        skiff_store::KIND_PHP
    } else {
        cmd.kind.trim()
    };
    let result = state
        .add_runtime(
            kind,
            &cmd.name,
            &cmd.version,
            &cmd.extensions,
            &cmd.actor.username,
            cmd.role.as_str(),
        )
        .await;
    match &result {
        Ok((rt, _)) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "sites.env.add",
            &rt.id,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "sites.env.add",
            &cmd.name,
            "failed",
            &e.to_string(),
        ),
    }
    result.map(|(runtime, started)| CreatedRuntime { runtime, started })
}

/// 运行环境页「镜像参数」：`None` 表示该字段未提交（保持原值）。
#[derive(Debug, Clone)]
pub struct SaveImages {
    pub nginx_image: Option<String>,
    pub php_image: Option<String>,
    pub actor: Actor,
}

pub fn save_images(state: &AppState, cmd: SaveImages) -> Result<(), SiteError> {
    state
        .save_runtime_images(cmd.nginx_image.as_deref(), cmd.php_image.as_deref())
        .map_err(SiteError::Internal)?;
    if let Err(e) = state.db().audit(
        &cmd.actor.username,
        &cmd.actor.ip,
        "sites.env.images",
        "-",
        "ok",
        "",
    ) {
        tracing::error!(error = %e, "运行环境镜像参数审计写入失败");
    }
    Ok(())
}

/// PHP 运行参数。表单解析与数值校验在 handler，命令只做保存与审计。
#[derive(Debug, Clone)]
pub struct SavePhpConfig {
    pub id: String,
    pub config: PhpRuntimeConfig,
    pub actor: Actor,
}

pub async fn save_config(state: &AppState, cmd: SavePhpConfig) -> Result<(), SiteError> {
    let result = state
        .save_php_runtime_config(&cmd.id, &cmd.config, &cmd.actor.username)
        .await;
    match &result {
        Ok(_) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.config",
            &cmd.id,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.config",
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    result.map(|_| ())
}

/// Node.js 配置保存结果：配置变化才重建，重建失败不回滚已保存的配置。
#[derive(Debug, Clone)]
pub enum NodeConfigOutcome {
    Unchanged,
    RebuildSubmitted,
    RebuildFailed(String),
}

#[derive(Debug, Clone)]
pub struct SaveNodeConfig {
    pub id: String,
    pub command: String,
    pub health_path: String,
    pub node_env: String,
    pub env_text: String,
    pub actor: Actor,
    pub role: Role,
}

pub async fn save_node_config(
    state: &Arc<AppState>,
    cmd: SaveNodeConfig,
) -> Result<NodeConfigOutcome, SiteError> {
    let default = NodeRuntimeConfig::default();
    let mut config = NodeRuntimeConfig {
        command: non_empty(&cmd.command, &default.command),
        health_path: non_empty(&cmd.health_path, &default.health_path),
        node_env: non_empty(&cmd.node_env, &default.node_env),
        env: Vec::new(),
    };
    config.set_env_text(&cmd.env_text)?;
    let result = state
        .save_node_runtime_config(&cmd.id, &config, &cmd.actor.username)
        .await;
    match &result {
        Ok(_) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.node_config",
            &cmd.id,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.node_config",
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    let (_, changed) = result?;
    if !changed {
        return Ok(NodeConfigOutcome::Unchanged);
    }
    match state
        .start_runtime_install(&cmd.id, &cmd.actor.username, cmd.role.as_str())
        .await
    {
        Ok(_) => Ok(NodeConfigOutcome::RebuildSubmitted),
        Err(e) => Ok(NodeConfigOutcome::RebuildFailed(e.to_string())),
    }
}

fn non_empty(value: &str, default: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        default.to_string()
    } else {
        value.to_string()
    }
}

/// 单个 PHP 配置文件保存；警告随结果返回，不阻止保存。
#[derive(Debug, Clone)]
pub struct SaveConfigFile {
    pub id: String,
    pub key: String,
    pub content: String,
    pub actor: Actor,
}

pub async fn save_config_file(
    state: &AppState,
    cmd: SaveConfigFile,
) -> Result<PhpConfigFile, SiteError> {
    let result = state
        .save_php_config_file(&cmd.id, &cmd.key, &cmd.content, &cmd.actor.username)
        .await;
    match &result {
        Ok(_) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.config_file",
            &cmd.id,
            "ok",
            &cmd.key,
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.config_file",
            &cmd.id,
            "failed",
            &format!("{}: {e}", cmd.key),
        ),
    }
    result
}

/// 进程型环境固定端口。是否已被站点绑定的二次确认由 handler 先行处理。
#[derive(Debug, Clone)]
pub struct SavePort {
    pub id: String,
    pub port: u16,
    pub actor: Actor,
    pub role: Role,
}

pub async fn save_port(state: &Arc<AppState>, cmd: SavePort) -> Result<(), SiteError> {
    let result = state
        .change_runtime_port(&cmd.id, cmd.port, &cmd.actor.username, cmd.role.as_str())
        .await;
    match &result {
        Ok(()) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.port",
            &cmd.id,
            "ok",
            &format!("port={}", cmd.port),
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.port",
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    result
}

#[derive(Debug, Clone)]
pub struct CloneRuntime {
    pub id: String,
    pub name: String,
    pub actor: Actor,
    pub role: Role,
}

pub async fn clone(state: &Arc<AppState>, cmd: CloneRuntime) -> Result<Runtime, SiteError> {
    let result = state
        .clone_runtime(&cmd.id, &cmd.name, &cmd.actor.username, cmd.role.as_str())
        .await;
    match &result {
        Ok(rt) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.clone",
            &cmd.id,
            "ok",
            &format!("clone={}", rt.id),
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.clone",
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    result
}

/// 运行环境动作。`web` 是内核自建 Web 服务，不落库，走独立入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOp {
    Start,
    Stop,
    Restart,
    Rebuild,
    Sync,
    Delete,
    Install,
}

impl RuntimeOp {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeOp::Start => "start",
            RuntimeOp::Stop => "stop",
            RuntimeOp::Restart => "restart",
            RuntimeOp::Rebuild => "rebuild",
            RuntimeOp::Sync => "sync",
            RuntimeOp::Delete => "delete",
            RuntimeOp::Install => "install",
        }
    }

    /// 一个环境可被多个站点反代：停用类动作需要先列出影响面并二次确认。
    fn needs_confirm(self) -> bool {
        matches!(
            self,
            RuntimeOp::Stop | RuntimeOp::Restart | RuntimeOp::Rebuild | RuntimeOp::Install
        )
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeAction {
    pub id: String,
    pub op: RuntimeOp,
    /// 用户已看过影响面并确认。
    pub confirmed: bool,
    pub actor: Actor,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub enum RuntimeActionOutcome {
    /// 有站点绑定且未确认：把影响面交回 handler 渲染提示。
    NeedsConfirm {
        domains: Vec<String>,
    },
    /// Web 服务动作完成（不落库）。
    Web,
    Deleted(RuntimeDeleteReport),
    Done,
}

pub async fn action(
    state: &Arc<AppState>,
    cmd: RuntimeAction,
) -> Result<RuntimeActionOutcome, SiteError> {
    let action = cmd.op.as_str();
    if cmd.id == "web" {
        if cmd.op == RuntimeOp::Sync {
            return Err(SiteError::BadRequest(format!(
                "Web 服务不支持操作：{action}"
            )));
        }
        let result = state.web_env_action(action).await;
        match &result {
            Ok(()) => state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                &format!("web_env.{action}"),
                "-",
                "ok",
                "",
            ),
            Err(e) => state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                &format!("web_env.{action}"),
                "-",
                "failed",
                &e.to_string(),
            ),
        }
        result?;
        return Ok(RuntimeActionOutcome::Web);
    }
    if cmd.op.needs_confirm() && !cmd.confirmed {
        let domains = state
            .db()
            .runtime_site_domains(&cmd.id)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        if !domains.is_empty() {
            return Ok(RuntimeActionOutcome::NeedsConfirm { domains });
        }
    }
    if cmd.op == RuntimeOp::Delete {
        let result = state.delete_site_runtime(&cmd.id).await;
        match &result {
            Ok(report) => state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                "runtimes.delete",
                &cmd.id,
                "ok",
                &report.completed.join("；"),
            ),
            Err(e) => state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                "runtimes.delete",
                &cmd.id,
                "failed",
                &e.to_string(),
            ),
        }
        return result.map(RuntimeActionOutcome::Deleted);
    }
    let result = state
        .runtime_action(&cmd.id, action, &cmd.actor.username, cmd.role.as_str())
        .await;
    match &result {
        Ok(()) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            &format!("runtimes.{action}"),
            &cmd.id,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            &format!("runtimes.{action}"),
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    result.map(|_| RuntimeActionOutcome::Done)
}

#[derive(Debug, Clone)]
pub struct SaveExtensions {
    pub id: String,
    pub extensions: Vec<String>,
    pub actor: Actor,
    pub role: Role,
}

/// 返回扩展集合是否真的变化（未变化不提交任务）。
pub async fn save_extensions(
    state: &Arc<AppState>,
    cmd: SaveExtensions,
) -> Result<bool, SiteError> {
    let result = state
        .update_runtime_extensions(
            &cmd.id,
            &cmd.extensions,
            &cmd.actor.username,
            cmd.role.as_str(),
        )
        .await;
    match &result {
        Ok(_) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions",
            &cmd.id,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions",
            &cmd.id,
            "failed",
            &e.to_string(),
        ),
    }
    result
}

#[derive(Debug, Clone)]
pub struct ExtensionChange {
    pub id: String,
    pub key: String,
    pub actor: Actor,
    pub role: Role,
}

pub async fn install_extension(
    state: &Arc<AppState>,
    cmd: ExtensionChange,
) -> Result<(), SiteError> {
    let result = state
        .install_extension(&cmd.id, &cmd.key, &cmd.actor.username, cmd.role.as_str())
        .await;
    audit_extension(state, &cmd, &result);
    result.map(|_| ())
}

pub async fn uninstall_extension(
    state: &Arc<AppState>,
    cmd: ExtensionChange,
) -> Result<(), SiteError> {
    let result = state
        .uninstall_extension(&cmd.id, &cmd.key, &cmd.actor.username, cmd.role.as_str())
        .await;
    audit_extension(state, &cmd, &result);
    result.map(|_| ())
}

fn audit_extension(state: &AppState, cmd: &ExtensionChange, result: &Result<bool, SiteError>) {
    match result {
        Ok(_) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions",
            &cmd.id,
            "ok",
            &cmd.key,
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions",
            &cmd.id,
            "failed",
            &format!("{}: {e}", cmd.key),
        ),
    }
}

#[derive(Debug, Clone)]
pub struct BatchExtensions {
    pub id: String,
    pub install: bool,
    pub keys: Vec<String>,
    pub actor: Actor,
    pub role: Role,
}

/// 返回实际变更的扩展数；0 表示全部已是目标状态，不写审计。
pub async fn batch_extensions(
    state: &Arc<AppState>,
    cmd: BatchExtensions,
) -> Result<usize, SiteError> {
    let result = state
        .apply_extension_selection(
            &cmd.id,
            cmd.install,
            &cmd.keys,
            &cmd.actor.username,
            cmd.role.as_str(),
        )
        .await;
    match &result {
        Ok(0) => {}
        Ok(n) => {
            let verb = if cmd.install { "安装" } else { "卸载" };
            state.audit_or_log(
                &cmd.actor.username,
                &cmd.actor.ip,
                "runtimes.extensions.batch",
                &cmd.id,
                "ok",
                &format!("{verb} {n} 项：{}", cmd.keys.join(", ")),
            );
        }
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions.batch",
            &cmd.id,
            "failed",
            &format!("{}: {e}", cmd.keys.join(", ")),
        ),
    }
    result
}

#[derive(Debug, Clone)]
pub struct InstallPecl {
    pub id: String,
    pub name: String,
    pub version: String,
    pub actor: Actor,
    pub role: Role,
}

/// PECL 安装：成功路径沿用旧行为，不写审计；失败写 `runtimes.extensions`。
pub async fn install_pecl(state: &Arc<AppState>, cmd: InstallPecl) -> Result<(), SiteError> {
    let result = state
        .install_pecl_extension(
            &cmd.id,
            &cmd.name,
            &cmd.version,
            &cmd.actor.username,
            cmd.role.as_str(),
        )
        .await;
    if let Err(e) = &result {
        state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.extensions",
            &cmd.id,
            "failed",
            &e.to_string(),
        );
    }
    result.map(|_| ())
}

#[derive(Debug, Clone)]
pub struct ProbeRuntime {
    pub id: String,
    pub actor: Actor,
}

/// 实测版本：只有主动点击才进容器执行；失败不写审计。
pub async fn probe(state: &AppState, cmd: ProbeRuntime) -> Result<String, SiteError> {
    let result = state.probe_runtime_version(&cmd.id).await;
    if let Ok(version) = &result {
        state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.probe",
            &cmd.id,
            "ok",
            version,
        );
    }
    result
}

#[derive(Debug, Clone)]
pub struct UnbindSite {
    pub id: String,
    pub domain: String,
    pub actor: Actor,
}

pub async fn unbind_site(state: &AppState, cmd: UnbindSite) -> Result<(), SiteError> {
    let result = state.unbind_runtime_site(&cmd.id, &cmd.domain).await;
    match &result {
        Ok(()) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.unbind",
            &cmd.id,
            "ok",
            &cmd.domain,
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "runtimes.unbind",
            &cmd.id,
            "failed",
            &format!("{}: {e}", cmd.domain),
        ),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::state_at;
    use skiff_store::{NewRuntime, KIND_PHP};

    fn actor() -> Actor {
        Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        }
    }

    fn seed_runtime(state: &AppState, id: &str, kind: &str) {
        state
            .db()
            .create_runtime(&NewRuntime {
                id: id.into(),
                kind: kind.into(),
                name: id.into(),
                version: "8.3".into(),
                image: String::new(),
                image_digest: String::new(),
                socket_path: None,
                port: None,
                is_default: false,
            })
            .unwrap();
    }

    fn seed_site_with_runtime(state: &AppState, domain: &str, runtime_id: &str) {
        let root = state.site_settings().sites_dir.join(domain);
        state
            .db()
            .create_site(&skiff_store::NewSite {
                domain: domain.into(),
                root_dir: root.display().to_string(),
                site_type: skiff_store::TYPE_RUNTIME.into(),
                runtime_id: Some(runtime_id.into()),
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
    }

    fn create_cmd(kind: &str, version: &str) -> CreateRuntime {
        CreateRuntime {
            kind: kind.into(),
            name: "test".into(),
            version: version.into(),
            extensions: Vec::new(),
            actor: actor(),
            role: Role::Admin,
        }
    }

    #[tokio::test]
    async fn create_validates_kind_and_version_before_docker() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = create(&state, create_cmd("ruby", "3.3")).await.unwrap_err();
        assert!(
            matches!(err, SiteError::BadRequest(ref m) if m.contains("未知运行环境类型")),
            "{err}"
        );

        // 空类型回落 php，空版本被版本目录拒绝。
        let err = create(&state, create_cmd("", "")).await.unwrap_err();
        assert!(
            matches!(err, SiteError::BadRequest(ref m) if m.contains("版本无效")),
            "{err}"
        );
        let rows = state.db().recent_audit(2).unwrap();
        assert_eq!(rows.len(), 2, "两次失败都应留审计：{rows:?}");
        assert!(rows
            .iter()
            .all(|r| r.action == "sites.env.add" && r.result == "failed"));
    }

    #[tokio::test]
    async fn web_service_rejects_sync_before_docker() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let err = action(
            &state,
            RuntimeAction {
                id: "web".into(),
                op: RuntimeOp::Sync,
                confirmed: false,
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("不支持操作：sync")));
        assert!(state.db().recent_audit(10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn action_reports_missing_runtime_and_rejects_bound_delete() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = action(
            &state,
            RuntimeAction {
                id: "nope".into(),
                op: RuntimeOp::Start,
                confirmed: false,
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("运行环境不存在")));

        seed_runtime(&state, "php1", KIND_PHP);
        seed_site_with_runtime(&state, "shop.local", "php1");
        let err = action(
            &state,
            RuntimeAction {
                id: "php1".into(),
                op: RuntimeOp::Delete,
                confirmed: true,
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::Conflict(ref m) if m.contains("仍被以下站点绑定")));

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "runtimes.delete");
        assert_eq!(rows[0].result, "failed");
    }

    #[tokio::test]
    async fn stop_with_bound_site_requires_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        seed_runtime(&state, "php1", KIND_PHP);
        seed_site_with_runtime(&state, "shop.local", "php1");

        let outcome = action(
            &state,
            RuntimeAction {
                id: "php1".into(),
                op: RuntimeOp::Stop,
                confirmed: false,
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap();
        match outcome {
            RuntimeActionOutcome::NeedsConfirm { domains } => {
                assert_eq!(domains, vec!["shop.local".to_string()]);
            }
            other => panic!("应返回影响面等待确认：{other:?}"),
        }
        assert!(
            state.db().recent_audit(10).unwrap().is_empty(),
            "未确认前不应执行、也不应写审计"
        );
    }

    #[tokio::test]
    async fn save_paths_reject_unknown_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = save_port(
            &state,
            SavePort {
                id: "nope".into(),
                port: 9000,
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("运行环境不存在")));

        let err = save_config_file(
            &state,
            SaveConfigFile {
                id: "nope".into(),
                key: "php.ini".into(),
                content: String::new(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("运行环境不存在")));

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "runtimes.config_file");
        assert_eq!(rows[0].result, "failed");
    }

    #[tokio::test]
    async fn node_config_is_unavailable_on_php_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        seed_runtime(&state, "php1", KIND_PHP);

        let err = save_node_config(
            &state,
            SaveNodeConfig {
                id: "php1".into(),
                command: String::new(),
                health_path: String::new(),
                node_env: String::new(),
                env_text: String::new(),
                actor: actor(),
                role: Role::Admin,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SiteError::Unavailable(ref m) if m.contains("没有 Node.js 专属配置")),
            "{err}"
        );
        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "runtimes.node_config");
        assert_eq!(rows[0].result, "failed");
    }

    #[tokio::test]
    async fn save_images_persists_and_audits_without_docker() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        save_images(
            &state,
            SaveImages {
                nginx_image: Some("openresty/skiff-test:1".into()),
                php_image: None,
                actor: actor(),
            },
        )
        .unwrap();
        assert_eq!(
            state.site_settings().nginx_image,
            "openresty/skiff-test:1",
            "镜像参数应落库"
        );
        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "sites.env.images");
        assert_eq!(rows[0].target, "-");
        assert_eq!(rows[0].result, "ok");
    }

    #[tokio::test]
    async fn unbind_rejects_unknown_site_with_audit() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        seed_runtime(&state, "php1", KIND_PHP);

        let err = unbind_site(
            &state,
            UnbindSite {
                id: "php1".into(),
                domain: "nope.local".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));
        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "runtimes.unbind");
        assert_eq!(rows[0].result, "failed");
    }
}
