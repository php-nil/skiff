//! 领域命令：handler 只做「解析 → 调用命令 → 映射响应」，业务动作都在这里。
//!
//! 每个命令对应 [`crate::actions`] 里的一个动作：入参是显式 DTO，出参是领域结果
//! 或领域错误；审计随命令产生，handler 不再代写。DTO 只承载业务输入与调用主体，
//! **不含展示字段**（文案、URL、CSS 类、按钮状态由传输层决定）。
//!
//! 命令不做权限判定：写入口已由 `web::authz` 按动作注册表校验，命令只被已授权的
//! 调用方使用；这样旧页面、`/api/v1` 与未来的 SDUI 动作端点拿到的是同一份业务结论。

pub mod docker;
pub mod file;
pub mod job;
pub mod runtime;
pub mod site;
pub mod store;
pub mod task;
pub mod upgrade;

/// 命令调用主体：只承载审计需要的身份与来源，不参与权限判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub username: String,
    pub ip: String,
}

/// 命令层单测共用的最小装配：内存库 + 不可达 Docker + 临时目录。
///
/// 故意让 Docker 指向不存在的 socket：命令的语义校验与错误映射应当在触达
/// 容器之前就有确定结论，测试不依赖真实引擎（也不起 HTTP）。
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;
    use std::sync::Arc;

    use skiff_docker::{DockerService, Form, Policy, Tier};
    use skiff_identity::SessionLimits;
    use skiff_plugin::{Supervisor, SupervisorConfig};

    use crate::{AppState, WebConfig};

    pub fn state_at(root: &Path) -> Arc<AppState> {
        let db = Arc::new(skiff_store::Db::open_in_memory().unwrap());
        let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
        let sup = Arc::new(Supervisor::new(
            SupervisorConfig::new(root.join("data"), root.join("run")),
            db.clone(),
        ));
        let cfg = WebConfig {
            data_dir: root.join("data"),
            web_root: root.join("www"),
            trash_retention_days: 30,
            config_path: root.join("config.toml"),
            listen: "127.0.0.1:18080".parse().unwrap(),
            entry_prefix: "p".into(),
            entry_prefix_previous: None,
            tls_enabled: false,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: true,
            dev_http: true,
            trusted_proxies: vec![],
            external_url: None,
            tier: Tier::Basic,
            form: Form::Native,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            policy: Policy::new(Tier::Basic, Form::Native, vec![]).unwrap(),
            session: SessionLimits::default(),
            login_concurrency: 1,
            ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
            ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
            catalog: Default::default(),
        };
        AppState::new(db, docker, sup, cfg, "csrf-test".into(), "test")
    }
}
