//! 审计动作分类（§6.3 审计流水）。
//!
//! 分类只由动作名的命名空间（`.` 前一段）决定，不单独落库：写审计的地方本来就按
//! `<域>.<动作>` 命名（`site.create` / `fs.write` / `auth.login`），读侧按同一份映射
//! 分类与筛选，存量数据不需要迁移。
//!
//! 这里是分类口径的**唯一来源**：`store` 的筛选 SQL 与审计页的分类下拉都从这里取，
//! 新增动作域只改 [`AuditCategory::namespaces`]，两处一起生效。

/// 审计分类。`Other` 是兜底：动作命名空间不在任何已登记集合里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditCategory {
    Auth,
    Site,
    Docker,
    Files,
    Plugin,
    Task,
    System,
    Other,
}

impl AuditCategory {
    /// 页面下拉与筛选白名单的顺序。
    pub const ALL: [AuditCategory; 8] = [
        AuditCategory::Auth,
        AuditCategory::Site,
        AuditCategory::Docker,
        AuditCategory::Files,
        AuditCategory::Plugin,
        AuditCategory::Task,
        AuditCategory::System,
        AuditCategory::Other,
    ];

    /// 稳定键：用于 `?cat=` 与接口返回。
    pub fn key(self) -> &'static str {
        match self {
            AuditCategory::Auth => "auth",
            AuditCategory::Site => "site",
            AuditCategory::Docker => "docker",
            AuditCategory::Files => "files",
            AuditCategory::Plugin => "plugin",
            AuditCategory::Task => "task",
            AuditCategory::System => "system",
            AuditCategory::Other => "other",
        }
    }

    /// 展示文案。
    pub fn label(self) -> &'static str {
        match self {
            AuditCategory::Auth => "认证与账号",
            AuditCategory::Site => "站点",
            AuditCategory::Docker => "容器与镜像",
            AuditCategory::Files => "文件",
            AuditCategory::Plugin => "插件",
            AuditCategory::Task => "任务",
            AuditCategory::System => "系统",
            AuditCategory::Other => "其他",
        }
    }

    /// 该分类覆盖的动作命名空间；`Other` 为空，由兜底逻辑处理。
    pub fn namespaces(self) -> &'static [&'static str] {
        match self {
            AuditCategory::Auth => &["auth", "login", "logout", "token", "admin"],
            AuditCategory::Site => &["site", "sites", "web", "nginx", "runtimes"],
            AuditCategory::Docker => &[
                "docker",
                "container",
                "image",
                "network",
                "volume",
                "compose",
            ],
            AuditCategory::Files => &["fs", "file", "files"],
            AuditCategory::Plugin => &["plugin", "store", "catalog"],
            AuditCategory::Task => &["task", "tasks", "job", "jobs", "scheduler"],
            AuditCategory::System => &[
                "system",
                "capability",
                "interop",
                "exec",
                "ftp",
                "firewall",
                "audit",
                "settings",
                "backup",
                "net",
            ],
            AuditCategory::Other => &[],
        }
    }

    /// 动作名 → 分类。取 `.` 前的命名空间；未登记即 `Other`。
    pub fn of(action: &str) -> Self {
        let namespace = action.split('.').next().unwrap_or("");
        Self::ALL
            .into_iter()
            .find(|c| c.namespaces().contains(&namespace))
            .unwrap_or(Self::Other)
    }

    /// `?cat=` 解析；未知值返回 `None`（按「全部分类」处理）。
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        Self::ALL.into_iter().find(|c| c.key() == raw)
    }

    /// 全部已登记命名空间（`Other` 的 SQL 反选条件用）。
    pub fn all_namespaces() -> Vec<&'static str> {
        Self::ALL
            .iter()
            .flat_map(|c| c.namespaces().iter().copied())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_namespace_maps_to_category() {
        assert_eq!(AuditCategory::of("auth.login"), AuditCategory::Auth);
        assert_eq!(AuditCategory::of("login"), AuditCategory::Auth);
        assert_eq!(AuditCategory::of("sites.create"), AuditCategory::Site);
        assert_eq!(
            AuditCategory::of("docker.containers.start"),
            AuditCategory::Docker
        );
        assert_eq!(AuditCategory::of("fs.write"), AuditCategory::Files);
        assert_eq!(
            AuditCategory::of("plugin.grant_upgrade"),
            AuditCategory::Plugin
        );
        assert_eq!(AuditCategory::of("task.run"), AuditCategory::Task);
        assert_eq!(AuditCategory::of("tasks.toggle"), AuditCategory::Task);
        assert_eq!(AuditCategory::of("job.cancel"), AuditCategory::Task);
        assert_eq!(
            AuditCategory::of("admin.reset_password"),
            AuditCategory::Auth
        );
        assert_eq!(AuditCategory::of("nginx.config.save"), AuditCategory::Site);
        assert_eq!(
            AuditCategory::of("settings.ui.update"),
            AuditCategory::System
        );
        assert_eq!(AuditCategory::of("exec.run"), AuditCategory::System);
        assert_eq!(AuditCategory::of("custom.thing"), AuditCategory::Other);
        assert_eq!(AuditCategory::of(""), AuditCategory::Other);
    }

    #[test]
    fn keys_roundtrip_and_unknown_is_all() {
        for c in AuditCategory::ALL {
            assert_eq!(AuditCategory::parse(c.key()), Some(c));
        }
        assert_eq!(AuditCategory::parse(""), None);
        assert_eq!(AuditCategory::parse("nope"), None);
    }

    #[test]
    fn namespaces_are_unique_and_lowercase() {
        let all = AuditCategory::all_namespaces();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "命名空间重复会让分类命中歧义");
        assert!(all
            .iter()
            .all(|n| n.chars().all(|c| c.is_ascii_lowercase() || c == '_')));
    }
}
