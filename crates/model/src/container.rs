//! 容器规格与调用主体：跨 crate 共享的**值类型**（零 I/O、零判定逻辑）。
//!
//! 这些类型原本住在 `skiff-policy`。但它们全是纯数据 + 自描述的解析规则，
//! 判定引擎才是 policy 的职责——`skiff-compose` 为了 `ContainerSpec` 依赖
//! `skiff-policy`，等于把 `libc`（路径校验用）拖进一个「纯 serde_yaml 演算」的解析器。
//! 迁到 `skiff-model` 后，依赖关系变成 `compose → model`、`docker → model + policy`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Grant, Role};

/// 短 id 长度（Docker CLI 惯例：完整 id 的前 12 位）。
pub const SHORT_ID_LEN: usize = 12;

/// 取短 id。展示层与审计详情统一用它，避免各处各截各的长度。
pub fn short_id(id: &str) -> String {
    id.chars().take(SHORT_ID_LEN).collect()
}

// ---------------- 第三层：参数字段白名单的类型面 ----------------

/// MVP 容器创建的参数面（§6.4.3）。`deny_unknown_fields` 让未列字段**显式报错**
/// 而不是被忽略；`Privileged`/`CapAdd`/`Devices`/`SecurityOpt`/`VolumesFrom`/
/// 自定义 `Runtime`/host 命名空间等字段在类型层面根本不存在。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerSpec {
    pub image: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    #[serde(default)]
    pub restart: Option<String>,
    #[serde(default)]
    pub healthcheck: Option<Healthcheck>,
    #[serde(default)]
    pub networks: Vec<String>,
    /// Docker 网络模式。当前只允许 `host`（宿主机直装形态由内核自用；插件禁止）。
    #[serde(default)]
    pub network_mode: Option<String>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// 容器日志驱动（只允许 `local` / `json-file`），配合 `log_max_size` 控制单文件大小。
    #[serde(default)]
    pub log_driver: Option<String>,
    #[serde(default)]
    pub log_max_size: Option<String>,
    #[serde(default)]
    pub memory_limit_mb: Option<i64>,
    #[serde(default)]
    pub nano_cpus: Option<i64>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
    /// CPU 权重（Docker `CpuShares`，0 = 使用默认权重）。与 `nano_cpus` 互斥使用：
    /// `nano_cpus` 表达硬配额，`cpu_shares` 表达相对权重。
    #[serde(default)]
    pub cpu_shares: Option<i64>,
    /// CPU 周期（微秒），与 `cpu_quota` 搭配；缺省由 Engine 取 100000。
    #[serde(default)]
    pub cpu_period: Option<i64>,
    /// CPU 配额（微秒/周期）；-1 表示不限制。
    #[serde(default)]
    pub cpu_quota: Option<i64>,
    /// 内存+交换上限（MB）；-1 表示交换区不限制。只在提供 `memory_limit_mb` 时有意义。
    #[serde(default)]
    pub memory_swap_mb: Option<i64>,
}

/// 资源限制热更新参数（`containers.update`）。字段全部可选，`None` 表示「本次不修改」。
///
/// 与 `ContainerSpec` 分开的原因：创建时的资源字段是白名单的一部分，更新时只需要
/// 可热更新的那几项；两者共用同一套 `Policy::validate_limits` 校验。
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    #[serde(default)]
    pub cpu_shares: Option<i64>,
    #[serde(default)]
    pub cpu_period: Option<i64>,
    #[serde(default)]
    pub cpu_quota: Option<i64>,
    #[serde(default)]
    pub memory_limit_mb: Option<i64>,
    #[serde(default)]
    pub memory_swap_mb: Option<i64>,
    #[serde(default)]
    pub pids_limit: Option<i64>,
}

impl ResourceLimits {
    pub fn is_empty(&self) -> bool {
        self.cpu_shares.is_none()
            && self.cpu_period.is_none()
            && self.cpu_quota.is_none()
            && self.memory_limit_mb.is_none()
            && self.memory_swap_mb.is_none()
            && self.pids_limit.is_none()
    }

    /// 从容器规格提取资源字段（创建路径的校验复用同一份规则）。
    pub fn from_spec(spec: &ContainerSpec) -> Self {
        Self {
            cpu_shares: spec.cpu_shares,
            cpu_period: spec.cpu_period,
            cpu_quota: spec.cpu_quota,
            memory_limit_mb: spec.memory_limit_mb,
            memory_swap_mb: spec.memory_swap_mb,
            pids_limit: spec.pids_limit,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PortMapping {
    pub host: u16,
    pub container: u16,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Healthcheck {
    pub test: Vec<String>,
    #[serde(default)]
    pub interval_secs: Option<i64>,
    #[serde(default)]
    pub timeout_secs: Option<i64>,
    #[serde(default)]
    pub retries: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Mount {
    /// named volume（只允许默认 local driver，且 driver_opts 为空）
    Volume {
        source: String,
        target: String,
        #[serde(default)]
        read_only: bool,
    },
    /// 宿主绑定挂载（native 形态 + 授权根内）
    Bind {
        source: String,
        target: String,
        #[serde(default)]
        read_only: bool,
    },
}

impl Mount {
    pub fn read_only(&self) -> bool {
        match self {
            Mount::Bind { read_only, .. } | Mount::Volume { read_only, .. } => *read_only,
        }
    }
}

// ---------------- 调用主体与对象视图 ----------------

/// 三层策略的调用主体。
#[derive(Debug, Clone)]
pub enum Actor {
    User { username: String, role: Role },
    Plugin { id: String, grants: Vec<Grant> },
}

impl Actor {
    pub fn label(&self) -> String {
        match self {
            Actor::User { username, .. } => format!("user:{username}"),
            Actor::Plugin { id, .. } => format!("plugin:{id}"),
        }
    }
}

/// 容器/网络/卷的可见对象视图（来自 bollard，策略只看这几个字段）。
#[derive(Debug, Clone, Default)]
pub struct ObjectRef {
    pub id: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

impl ObjectRef {
    /// 短 id（Docker 惯例前 12 位）。
    pub fn short_id(&self) -> String {
        short_id(&self.id)
    }

    pub fn is_protected(&self) -> bool {
        self.labels
            .get(crate::verb::LABEL_PROTECTED)
            .map(|v| v == "true")
            .unwrap_or(false)
            // Skiff 自身容器（compose 服务名 skiff）与内核数据卷恒拒绝
            || self.name == "skiff"
            || self.name.starts_with("skiff_")
    }

    pub fn owner(&self) -> Option<&str> {
        self.labels
            .get(crate::verb::LABEL_OWNER)
            .map(|s| s.as_str())
    }

    pub fn project(&self) -> Option<&str> {
        self.labels
            .get(crate::verb::LABEL_PROJECT)
            .map(|s| s.as_str())
    }

    /// 1Panel / 宝塔托管资源：插件默认不可写（§6.4.2）。
    pub fn is_foreign_panel(&self) -> bool {
        self.labels.contains_key("createdBy")
            || self.name.starts_with("1panel")
            || self.name.starts_with("baota")
            || self.name.starts_with("bt_")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_rejects_unknown_fields() {
        // 未列字段（privileged / cap_add）必须在**反序列化**就失败，
        // 而不是等到某个 handler 想起来检查。
        let json = r#"{"image":"nginx","privileged":true}"#;
        assert!(serde_json::from_str::<ContainerSpec>(json).is_err());
        let json = r#"{"image":"nginx","cap_add":["SYS_ADMIN"]}"#;
        assert!(serde_json::from_str::<ContainerSpec>(json).is_err());
    }

    #[test]
    fn spec_accepts_whitelisted_fields() {
        let json = r#"{
            "image":"nginx:1.27-alpine",
            "name":"web",
            "env":{"TZ":"Asia/Shanghai"},
            "ports":[{"host":18081,"container":80}],
            "restart":"unless-stopped",
            "memory_limit_mb":128
        }"#;
        let spec: ContainerSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.image, "nginx:1.27-alpine");
        assert_eq!(spec.ports[0].host, 18081);
    }

    #[test]
    fn spec_accepts_resource_fields() {
        let json = r#"{
            "image":"nginx",
            "cpu_shares":512,
            "cpu_period":100000,
            "cpu_quota":50000,
            "memory_swap_mb":-1,
            "pids_limit":64
        }"#;
        let spec: ContainerSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.cpu_shares, Some(512));
        assert_eq!(spec.cpu_period, Some(100000));
        assert_eq!(spec.cpu_quota, Some(50000));
        assert_eq!(spec.memory_swap_mb, Some(-1));
        assert_eq!(spec.pids_limit, Some(64));
        let limits = ResourceLimits::from_spec(&spec);
        assert_eq!(limits.cpu_shares, Some(512));
        assert_eq!(limits.memory_swap_mb, Some(-1));
    }

    #[test]
    fn resource_limits_reject_unknown_fields() {
        let json = r#"{"cpu_shares":512,"cpuset_cpus":"0-1"}"#;
        assert!(serde_json::from_str::<ResourceLimits>(json).is_err());
        let empty = ResourceLimits::default();
        assert!(empty.is_empty());
        assert!(!ResourceLimits {
            pids_limit: Some(64),
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn mount_read_only_flag() {
        let m = Mount::Bind {
            source: "/etc".into(),
            target: "/data".into(),
            read_only: true,
        };
        assert!(m.read_only());
    }

    #[test]
    fn object_ref_protection_rules() {
        let mut obj = ObjectRef {
            id: "abc".into(),
            name: "web".into(),
            labels: BTreeMap::new(),
        };
        assert!(!obj.is_protected());
        obj.labels
            .insert(crate::verb::LABEL_PROTECTED.to_string(), "true".to_string());
        assert!(obj.is_protected());

        let skiff = ObjectRef {
            id: "x".into(),
            name: "skiff".into(),
            labels: BTreeMap::new(),
        };
        assert!(skiff.is_protected(), "Skiff 自身容器恒受保护");
    }
}
