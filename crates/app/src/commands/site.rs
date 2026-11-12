//! 站点领域命令（对应 `actions::Resource::Site` 的 9 个动作）。
//!
//! 入参 DTO 只包含业务输入与 [`Actor`]；`handler` 负责表单解析、CSRF、文案与跳转，
//! 这里负责语义校验、执行与审计。错误统一用 [`SiteError`] 表达，由传输层映射。

use std::path::Path;

use skiff_store::Site;

use super::Actor;
use crate::sites::{
    site_root_for, validate_domain, NewSite, SiteError, SiteField, VhostSaveOutcome,
};
use crate::tasks::TaskService;
use crate::AppState;

/// 命令名（与 `actions` 表里的 `command` 后缀一一对应），供漂移测试使用。
pub const NAMES: &[&str] = &[
    "create",
    "batch",
    "save_config",
    "delete",
    "power",
    "set_default",
    "update_domains",
    "ssl",
    "backup",
];

/// 新建站点。`domain` / `root_dir` 是**原始输入**，规范化与路径推导在命令内完成。
#[derive(Debug, Clone)]
pub struct CreateSite {
    pub domain: String,
    pub root_dir: String,
    pub site_type: String,
    pub runtime_id: Option<String>,
    pub proxy_target: String,
    pub proxy_websocket: bool,
    pub aliases: Vec<String>,
    pub actor: Actor,
}

/// 用户只填目录名；容错剥掉 `/www/sites/` 或宿主机前缀，避免把整段路径当目录名。
fn root_slug(raw: &str, sites_dir: &Path) -> Option<String> {
    let value = raw.trim().replace('\\', "/");
    if value.is_empty() {
        return None;
    }
    let value = value.trim_start_matches('/').to_string();
    for prefix in ["www/sites/", "sites/"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return Some(rest.trim_start_matches('/').to_string());
        }
    }
    let host = sites_dir.to_string_lossy().replace('\\', "/");
    let host = host.trim_start_matches('/');
    if let Some(rest) = value.strip_prefix(&format!("{host}/")) {
        return Some(rest.to_string());
    }
    Some(value)
}

pub async fn create(state: &AppState, cmd: CreateSite) -> Result<Site, SiteError> {
    let settings = state.site_settings();
    let domain = validate_domain(&cmd.domain).map_err(|e| SiteError::at(SiteField::Domain, e))?;
    let slug = root_slug(&cmd.root_dir, &settings.sites_dir);
    let root_dir = site_root_for(&settings.sites_dir, &domain, slug.as_deref())
        .map_err(|e| SiteError::at(SiteField::RootDir, e))?;
    let req = NewSite {
        domain,
        root_dir: Some(root_dir.display().to_string()),
        site_type: cmd.site_type,
        runtime_id: cmd.runtime_id.filter(|v| !v.trim().is_empty()),
        proxy_target: cmd.proxy_target,
        proxy_websocket: cmd.proxy_websocket,
        aliases: cmd.aliases,
    };
    let site = state.create_site(req).await?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "sites.create",
        &site.domain,
        "ok",
        &site.root_dir,
    );
    Ok(site)
}

/// 站点启停。
#[derive(Debug, Clone)]
pub struct SitePower {
    pub domain: String,
    pub running: bool,
    pub actor: Actor,
}

impl SitePower {
    /// 审计动作名，与旧 handler 完全一致。
    pub fn audit_action(&self) -> &'static str {
        if self.running {
            "sites.start"
        } else {
            "sites.stop"
        }
    }
}

pub async fn power(state: &AppState, cmd: SitePower) -> Result<(), SiteError> {
    state.set_site_running(&cmd.domain, cmd.running).await?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        cmd.audit_action(),
        &cmd.domain,
        "ok",
        "",
    );
    Ok(())
}

/// 默认站点开关。
#[derive(Debug, Clone)]
pub struct SetDefaultSite {
    pub domain: String,
    pub enabled: bool,
    pub actor: Actor,
}

pub async fn set_default(state: &AppState, cmd: SetDefaultSite) -> Result<(), SiteError> {
    state.set_default_site(&cmd.domain, cmd.enabled).await?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "sites.default",
        &cmd.domain,
        "ok",
        if cmd.enabled { "set" } else { "unset" },
    );
    Ok(())
}

/// 站点域名（别名）动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasOp {
    Add,
    Remove,
}

impl AliasOp {
    fn audit_action(self) -> &'static str {
        match self {
            AliasOp::Add => "sites.domains.add",
            AliasOp::Remove => "sites.domains.remove",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpdateDomains {
    pub domain: String,
    pub op: AliasOp,
    pub alias: String,
    pub actor: Actor,
}

/// 返回落库后的别域名（已规范化）。
pub async fn update_domains(state: &AppState, cmd: UpdateDomains) -> Result<String, SiteError> {
    let result = match cmd.op {
        AliasOp::Add => state.add_site_alias(&cmd.domain, &cmd.alias).await,
        AliasOp::Remove => state.remove_site_alias(&cmd.domain, &cmd.alias).await,
    };
    match &result {
        Ok(name) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            cmd.op.audit_action(),
            &cmd.domain,
            "ok",
            name,
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            cmd.op.audit_action(),
            &cmd.domain,
            "fail",
            &e.to_string(),
        ),
    }
    result
}

/// SSL 子动作。`as_str` 只用于审计动作名，与旧表单字段取值一致。
#[derive(Debug, Clone)]
pub enum SslOp {
    Generate,
    Upload { cert_pem: String, key_pem: String },
    Force { enabled: bool },
    Disable,
    DeleteCert,
}

impl SslOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            SslOp::Generate => "generate",
            SslOp::Upload { .. } => "upload",
            SslOp::Force { .. } => "force",
            SslOp::Disable => "disable",
            SslOp::DeleteCert => "delete_cert",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SiteSsl {
    pub domain: String,
    pub op: SslOp,
    pub actor: Actor,
}

pub async fn ssl(state: &AppState, cmd: SiteSsl) -> Result<(), SiteError> {
    let result = match &cmd.op {
        SslOp::Generate => state.generate_self_signed(&cmd.domain).await,
        SslOp::Upload { cert_pem, key_pem } => {
            state.upload_cert(&cmd.domain, cert_pem, key_pem).await
        }
        SslOp::Force { enabled } => state.set_force_https(&cmd.domain, *enabled).await,
        SslOp::Disable => state.disable_tls(&cmd.domain).await,
        SslOp::DeleteCert => state.delete_cert(&cmd.domain).await,
    };
    let action = format!("sites.ssl.{}", cmd.op.as_str());
    match &result {
        Ok(()) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            &action,
            &cmd.domain,
            "ok",
            "",
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            &action,
            &cmd.domain,
            "failed",
            &e.to_string(),
        ),
    }
    result
}

/// 站点 vhost 配置保存。
#[derive(Debug, Clone)]
pub struct SaveSiteConfig {
    pub domain: String,
    pub content: String,
    pub actor: Actor,
}

pub async fn save_config(
    state: &AppState,
    cmd: SaveSiteConfig,
) -> Result<VhostSaveOutcome, SiteError> {
    let result = state.save_site_vhost(&cmd.domain, &cmd.content).await;
    match &result {
        Ok(outcome) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "sites.config.save",
            &cmd.domain,
            "ok",
            if outcome.reloaded {
                "已保存并重载 nginx"
            } else {
                "已保存并通过校验（站点未运行）"
            },
        ),
        Err(e) => state.audit_or_log(
            &cmd.actor.username,
            &cmd.actor.ip,
            "sites.config.save",
            &cmd.domain,
            "failed",
            &e.to_string(),
        ),
    }
    result
}

/// 站点删除。域名确认（`confirm` 字段）属于表单形状校验，留在传输层。
#[derive(Debug, Clone)]
pub struct DeleteSite {
    pub domain: String,
    pub actor: Actor,
}

pub async fn delete(state: &AppState, cmd: DeleteSite) -> Result<(), SiteError> {
    state.delete_site(&cmd.domain).await?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "sites.delete",
        &cmd.domain,
        "ok",
        "",
    );
    Ok(())
}

/// 批量动作。`Delete` 不提供批量入口：删除必须走单站确认页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOp {
    Start,
    Stop,
    Delete,
}

impl BatchOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BatchOp::Start => "start",
            BatchOp::Stop => "stop",
            BatchOp::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatchSites {
    pub op: BatchOp,
    pub domains: Vec<String>,
    pub actor: Actor,
}

/// 单站失败：保留领域错误，由传输层决定如何渲染。
#[derive(Debug, Clone)]
pub struct BatchFailure {
    pub domain: String,
    pub error: SiteError,
}

#[derive(Debug, Clone, Default)]
pub struct BatchReport {
    pub succeeded: usize,
    pub failures: Vec<BatchFailure>,
}

pub async fn batch(state: &AppState, cmd: BatchSites) -> Result<BatchReport, SiteError> {
    if cmd.op == BatchOp::Delete {
        // 批量删除没有「输入域名确认」这一关，留着就是绕过删除确认的后门：
        // 一律拒绝并引导到单站确认页（那里会展示将被删除的量并要求输入域名）。
        return Err(SiteError::BadRequest(
            "删除站点请走「站点详情 → 删除确认页」（需输入域名确认）".into(),
        ));
    }
    let running = cmd.op == BatchOp::Start;
    let audit_action = format!("sites.batch.{}", cmd.op.as_str());
    let mut report = BatchReport::default();
    for domain in &cmd.domains {
        match state.set_site_running(domain, running).await {
            Ok(()) => {
                report.succeeded += 1;
                state.audit_or_log(
                    &cmd.actor.username,
                    &cmd.actor.ip,
                    &audit_action,
                    domain,
                    "ok",
                    "",
                );
            }
            Err(e) => {
                state.audit_or_log(
                    &cmd.actor.username,
                    &cmd.actor.ip,
                    &audit_action,
                    domain,
                    "failed",
                    &e.to_string(),
                );
                report.failures.push(BatchFailure {
                    domain: domain.clone(),
                    error: e,
                });
            }
        }
    }
    Ok(report)
}

/// 站点备份（每日 03:00 的内置任务）。
#[derive(Debug, Clone)]
pub struct SetSiteBackup {
    pub domain: String,
    pub enabled: bool,
    pub actor: Actor,
}

/// `tasks` 由调用方解析：任务服务未初始化属于依赖问题，由传输层决定响应形态。
pub async fn backup(
    state: &AppState,
    tasks: &TaskService,
    cmd: SetSiteBackup,
) -> Result<(), SiteError> {
    tasks
        .set_site_backup(&cmd.domain, cmd.enabled, &cmd.actor.username)
        .await
        .map_err(SiteError::Unavailable)?;
    state.audit_or_log(
        &cmd.actor.username,
        &cmd.actor.ip,
        "sites.backup",
        &cmd.domain,
        "ok",
        if cmd.enabled {
            "开启站点备份"
        } else {
            "关闭站点备份"
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::state_at;

    fn actor() -> Actor {
        Actor {
            username: "admin".into(),
            ip: "127.0.0.1".into(),
        }
    }

    fn create_cmd(domain: &str, root_dir: &str) -> CreateSite {
        CreateSite {
            domain: domain.into(),
            root_dir: root_dir.into(),
            site_type: "static".into(),
            runtime_id: None,
            proxy_target: String::new(),
            proxy_websocket: false,
            aliases: Vec::new(),
            actor: actor(),
        }
    }

    /// 直接写库造一个站点记录：命令的语义校验大多在触达 Docker 之前完成。
    fn seed_site(state: &AppState, domain: &str) {
        let root = state.site_settings().sites_dir.join(domain);
        state
            .db()
            .create_site(&skiff_store::NewSite {
                domain: domain.into(),
                root_dir: root.display().to_string(),
                site_type: "static".into(),
                runtime_id: None,
                proxy_target: None,
                proxy_websocket: false,
            })
            .unwrap();
    }

    #[test]
    fn root_slug_strips_known_prefixes_only() {
        let sites = Path::new("/www/sites");
        assert_eq!(root_slug("", sites), None);
        assert_eq!(root_slug("/www/sites/shop", sites).as_deref(), Some("shop"));
        assert_eq!(root_slug("sites/a", sites).as_deref(), Some("a"));
        assert_eq!(
            root_slug("/www/sites/shop", sites).as_deref(),
            Some("shop"),
            "宿主机绝对路径也应剥成目录名"
        );
        assert_eq!(root_slug("custom", sites).as_deref(), Some("custom"));
    }

    #[tokio::test]
    async fn create_validates_shape_before_touching_docker() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = create(&state, create_cmd("bad domain", ""))
            .await
            .unwrap_err();
        assert_eq!(err.field(), Some(SiteField::Domain), "{err}");
        assert!(
            err.to_string().contains("域名只能包含"),
            "非法域名文案不得变化：{err}"
        );
        assert!(state.db().site_by_domain("bad domain").unwrap().is_none());

        let err = create(&state, create_cmd("shop.local", ".."))
            .await
            .unwrap_err();
        assert_eq!(err.field(), Some(SiteField::RootDir), "{err}");
        assert!(
            err.to_string().contains("站点目录"),
            "越界目录文案不得变化：{err}"
        );
    }

    #[tokio::test]
    async fn create_alias_conflict_carries_alias_field() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let mut cmd = create_cmd("shop.local", "");
        cmd.aliases = vec!["shop.local".into()];
        let err = create(&state, cmd).await.unwrap_err();
        assert_eq!(err.field(), Some(SiteField::Aliases), "{err}");
        assert!(
            err.to_string().contains("主域名"),
            "别名文案不得变化：{err}"
        );
    }

    #[tokio::test]
    async fn create_reports_unavailable_when_web_service_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let err = create(&state, create_cmd("shop.local", ""))
            .await
            .unwrap_err();
        assert!(
            matches!(err, SiteError::Unavailable(ref m) if m.contains("Web 服务（OpenResty）未就绪")),
            "环境未就绪必须是依赖不可用：{err}"
        );
        assert!(
            state.db().recent_audit(10).unwrap().is_empty(),
            "失败的新建不应留下成功审计"
        );
    }

    #[tokio::test]
    async fn power_and_delete_reject_unknown_site() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = power(
            &state,
            SitePower {
                domain: "nope.local".into(),
                running: true,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));

        let err = delete(
            &state,
            DeleteSite {
                domain: "nope.local".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));

        assert!(
            state.db().recent_audit(10).unwrap().is_empty(),
            "未成功的启停/删除不写审计"
        );
    }

    #[tokio::test]
    async fn update_domains_rejects_main_domain_and_duplicates_with_audit() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        seed_site(&state, "shop.local");

        let err = update_domains(
            &state,
            UpdateDomains {
                domain: "shop.local".into(),
                op: AliasOp::Remove,
                alias: "shop.local".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("主域名")));

        state
            .db()
            .add_site_aliases("shop.local", &["www.shop.local".to_string()])
            .unwrap();
        let err = update_domains(
            &state,
            UpdateDomains {
                domain: "shop.local".into(),
                op: AliasOp::Add,
                alias: "www.shop.local".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::Conflict(ref m) if m.contains("别名已存在")));

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "sites.domains.add");
        assert_eq!(rows[0].target, "shop.local");
        assert_eq!(rows[0].result, "fail");
        assert!(rows[0].detail.contains("别名已存在"));
    }

    #[tokio::test]
    async fn batch_rejects_delete_and_reports_partial_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = batch(
            &state,
            BatchSites {
                op: BatchOp::Delete,
                domains: vec!["a.local".into()],
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("删除确认页")));

        let report = batch(
            &state,
            BatchSites {
                op: BatchOp::Start,
                domains: vec!["missing.local".into()],
                actor: actor(),
            },
        )
        .await
        .unwrap();
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].domain, "missing.local");
        assert!(
            matches!(report.failures[0].error, SiteError::BadRequest(ref m) if m.contains("站点不存在"))
        );

        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "sites.batch.start");
        assert_eq!(rows[0].target, "missing.local");
        assert_eq!(rows[0].result, "failed");
        assert_eq!(rows[0].detail, "站点不存在：missing.local");
    }

    #[tokio::test]
    async fn ssl_and_save_config_return_domain_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = ssl(
            &state,
            SiteSsl {
                domain: "nope.local".into(),
                op: SslOp::Generate,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));
        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "sites.ssl.generate");
        assert_eq!(rows[0].result, "failed");

        let err = save_config(
            &state,
            SaveSiteConfig {
                domain: "nope.local".into(),
                content: "server {}".into(),
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));
        let rows = state.db().recent_audit(1).unwrap();
        assert_eq!(rows[0].action, "sites.config.save");
        assert_eq!(rows[0].result, "failed");
    }

    #[tokio::test]
    async fn set_default_and_backup_reject_unknown_site() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());

        let err = set_default(
            &state,
            SetDefaultSite {
                domain: "nope.local".into(),
                enabled: true,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::BadRequest(ref m) if m.contains("站点不存在")));

        state.init_tasks().unwrap();
        let svc = state.tasks().unwrap();
        let err = backup(
            &state,
            svc,
            SetSiteBackup {
                domain: "nope.local".into(),
                enabled: true,
                actor: actor(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiteError::Unavailable(ref m) if m == "站点不存在"));
    }
}
