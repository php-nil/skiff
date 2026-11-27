//! 容器与资源域：容器列表 / 详情 / 限额、镜像拉取、网络与卷。

#[allow(unused_imports)]
use super::super::*;

#[derive(Debug, Default, Deserialize)]
pub struct ContainerListQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    /// `error` 只保留异常容器（概览待办深链），其余值视为不筛选。
    pub filter: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

/// 容器列表项（字段冻结，只增不改；与旧表格同一 `container_view`）。
#[derive(Debug, Clone, Serialize)]
pub struct ContainerItem {
    pub id: String,
    pub short_id: String,
    pub name: String,
    pub image: String,
    pub state: StatusDto,
    pub status: String,
    pub project: String,
    pub protected: bool,
    pub unprotected_kernel: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerPort {
    pub host: String,
    pub container: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerMount {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLabel {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerLimitLine {
    pub label: String,
    pub value: String,
}

/// 资源限制的原始表单值（字符串保留「默认/不限制」语义）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ContainerLimits {
    pub cpu_shares: String,
    pub cpu_period: String,
    pub cpu_quota: String,
    pub memory_limit_mb: String,
    pub memory_swap_mb: String,
    pub pids_limit: String,
}

/// 容器详情（只读）：基本信息按块拆分，字段与旧详情页同源。
#[derive(Debug, Serialize)]
pub struct ContainerDetailData {
    pub container: ContainerItem,
    pub image: String,
    pub cmd: String,
    /// RFC3339 UTC（D9）；Docker 返回原值。
    pub created_at: String,
    pub health: String,
    pub ports: Vec<ContainerPort>,
    pub mounts: Vec<ContainerMount>,
    pub networks: Vec<String>,
    pub labels: Vec<ContainerLabel>,
    pub limits: ContainerLimits,
    pub limits_effective: Vec<ContainerLimitLine>,
    pub can_exec: bool,
    pub can_operate: bool,
    pub can_delete: bool,
    pub docker_reason: String,
}

/// `POST /ui/v1/containers` 的请求体：与旧表单同名，空串按未填处理。
#[derive(Debug, Default, Deserialize)]
pub struct ContainerCreateBody {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ports: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub binds: String,
    #[serde(default)]
    pub volumes: String,
    #[serde(default)]
    pub restart: String,
    #[serde(default)]
    pub cpu_shares: String,
    #[serde(default)]
    pub cpu_period: String,
    #[serde(default)]
    pub cpu_quota: String,
    #[serde(default)]
    pub memory_limit_mb: String,
    #[serde(default)]
    pub memory_swap_mb: String,
    #[serde(default)]
    pub pids_limit: String,
    #[serde(default)]
    pub networks: Vec<String>,
}

/// `POST /ui/v1/containers/{id}/limits` 的请求体：空串沿用旧表单语义（0 = 清除）。
#[derive(Debug, Default, Deserialize)]
pub struct ContainerLimitsBody {
    #[serde(default)]
    pub cpu_shares: String,
    #[serde(default)]
    pub cpu_period: String,
    #[serde(default)]
    pub cpu_quota: String,
    #[serde(default)]
    pub memory_limit_mb: String,
    #[serde(default)]
    pub memory_swap_mb: String,
    #[serde(default)]
    pub pids_limit: String,
}

/// 镜像 / 网络 / 卷的通用行。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceItem {
    pub id: String,
    pub name: String,
    pub meta: String,
    pub warn: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkContainerItem {
    pub name: String,
    pub ipv4: String,
    pub ipv6: String,
    pub mac: String,
}

/// 网络行（字段冻结，只增不改；与旧页面 `NetworkRowView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct NetworkItem {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub ipam_driver: String,
    pub subnet: String,
    pub gateway: String,
    pub internal: String,
    pub attachable: String,
    pub container_count: String,
    pub containers_known: bool,
    pub containers: Vec<NetworkContainerItem>,
    pub builtin: bool,
    pub can_remove: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ImagePullBody {
    #[serde(default)]
    pub image: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct NameBody {
    #[serde(default)]
    pub name: String,
}
