//! 概览页「需要关注的事情」：只做业务判定与排序，不产生文案、链接或样式。
//!
//! 输入是已探测到的真实信号（会话、初始口令文件、Docker 可用性、异常容器、停用插件、
//! TOTP 状态）；输出是稳定 code、危险级别与计数。中文标题、说明与深链由传输层映射。

/// 影响级别：危险在前，警告在后。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionLevel {
    Danger,
    Warn,
}

impl AttentionLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Danger => "danger",
            Self::Warn => "warn",
        }
    }
}

/// 稳定的事项 code：展示层据此映射文案与深链，禁止按标题反向匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionCode {
    PasswordChangeRequired,
    InitialPasswordPending,
    DockerUnavailable,
    AbnormalContainers,
    AbnormalSites,
    DisabledPlugins,
    TotpDisabled,
}

impl AttentionCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PasswordChangeRequired => "password.change_required",
            Self::InitialPasswordPending => "password.initial_pending",
            Self::DockerUnavailable => "docker.unavailable",
            Self::AbnormalContainers => "containers.abnormal",
            Self::AbnormalSites => "sites.abnormal",
            Self::DisabledPlugins => "plugins.disabled",
            Self::TotpDisabled => "totp.disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionItem {
    pub code: AttentionCode,
    pub level: AttentionLevel,
    /// 事项涉及的实体数量；单条布尔信号为 0。
    pub count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct AttentionSignals {
    pub must_change_password: bool,
    pub initial_password_pending: bool,
    pub docker_available: bool,
    pub abnormal_containers: usize,
    pub abnormal_sites: usize,
    pub disabled_plugins: usize,
    pub totp_enabled: bool,
}

/// 汇总关注项，并按「危险 → 警告」稳定排序；同级保持生成顺序。
pub fn collect(signals: AttentionSignals) -> Vec<AttentionItem> {
    let item = |code, level, count| AttentionItem { code, level, count };
    let mut items = Vec::new();
    if signals.must_change_password {
        items.push(item(
            AttentionCode::PasswordChangeRequired,
            AttentionLevel::Danger,
            0,
        ));
    } else if signals.initial_password_pending {
        items.push(item(
            AttentionCode::InitialPasswordPending,
            AttentionLevel::Warn,
            0,
        ));
    }
    if !signals.docker_available {
        items.push(item(
            AttentionCode::DockerUnavailable,
            AttentionLevel::Danger,
            0,
        ));
    }
    if signals.abnormal_containers > 0 {
        items.push(item(
            AttentionCode::AbnormalContainers,
            AttentionLevel::Danger,
            signals.abnormal_containers,
        ));
    }
    if signals.abnormal_sites > 0 {
        items.push(item(
            AttentionCode::AbnormalSites,
            AttentionLevel::Danger,
            signals.abnormal_sites,
        ));
    }
    if signals.disabled_plugins > 0 {
        items.push(item(
            AttentionCode::DisabledPlugins,
            AttentionLevel::Warn,
            signals.disabled_plugins,
        ));
    }
    if !signals.totp_enabled {
        items.push(item(AttentionCode::TotpDisabled, AttentionLevel::Warn, 0));
    }
    items.sort_by_key(|i| i.level);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> AttentionSignals {
        AttentionSignals {
            must_change_password: false,
            initial_password_pending: false,
            docker_available: true,
            abnormal_containers: 0,
            abnormal_sites: 0,
            disabled_plugins: 0,
            totp_enabled: true,
        }
    }

    #[test]
    fn healthy_has_no_attention_items() {
        assert!(collect(healthy()).is_empty());
    }

    #[test]
    fn initial_password_is_warn_only_without_force_change() {
        let items = collect(AttentionSignals {
            initial_password_pending: true,
            ..healthy()
        });
        assert_eq!(
            items,
            vec![AttentionItem {
                code: AttentionCode::InitialPasswordPending,
                level: AttentionLevel::Warn,
                count: 0,
            }]
        );
    }

    #[test]
    fn items_are_danger_first_and_keep_generation_order() {
        let items = collect(AttentionSignals {
            must_change_password: true,
            initial_password_pending: true,
            docker_available: false,
            abnormal_containers: 2,
            abnormal_sites: 1,
            disabled_plugins: 3,
            totp_enabled: false,
        });
        let codes: Vec<AttentionCode> = items.iter().map(|i| i.code).collect();
        assert_eq!(
            codes,
            vec![
                AttentionCode::PasswordChangeRequired,
                AttentionCode::DockerUnavailable,
                AttentionCode::AbnormalContainers,
                AttentionCode::AbnormalSites,
                AttentionCode::DisabledPlugins,
                AttentionCode::TotpDisabled,
            ]
        );
        let levels: Vec<AttentionLevel> = items.iter().map(|i| i.level).collect();
        assert_eq!(
            levels,
            vec![
                AttentionLevel::Danger,
                AttentionLevel::Danger,
                AttentionLevel::Danger,
                AttentionLevel::Danger,
                AttentionLevel::Warn,
                AttentionLevel::Warn,
            ]
        );
        assert_eq!(items[2].count, 2);
        assert_eq!(items[3].count, 1);
        assert_eq!(items[4].count, 3);
    }
}
