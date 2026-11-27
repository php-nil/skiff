//! 运行环境域：版本清单、安装 / 启停 / 端口 / 发布 / 克隆 / 扩展 / PHP 与 Node 配置。

#[allow(unused_imports)]
use super::super::*;

/// 版本清单查询：`kind` 缺省 PHP（`refresh=true` 绕过缓存，页面刷新入口用它）。
#[derive(Debug, Default, Deserialize)]
pub struct VersionIndexQuery {
    pub kind: Option<String>,
    #[serde(default)]
    pub refresh: bool,
}

/// 运行环境最近一次异步任务（安装/扩展）；字段与页面抽屉同源。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeJobItem {
    pub state: String,
    pub label: String,
    pub step: String,
    pub progress: u8,
    pub error: String,
    pub active: bool,
    pub failed: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub started_at: String,
    pub updated_at: String,
}

/// 运行环境列表项（字段冻结，只增不改；与页面 `RuntimeRow` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeItem {
    pub id: String,
    pub kind: StatusDto,
    pub name: String,
    pub container_name: String,
    pub version: String,
    pub version_display: String,
    pub version_note: String,
    pub image: String,
    pub state: StatusDto,
    pub state_class: String,
    pub drift: bool,
    pub site_count: usize,
    pub container_state: String,
    pub endpoint_short: String,
    pub endpoint_full: String,
    pub implemented: bool,
    pub is_default: bool,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
    pub updated_at: String,
    pub job: Option<RuntimeJobItem>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeFileQuery {
    #[serde(default)]
    pub file: String,
    /// L3 子视图（overview / config / extensions / sites / manage）。
    #[serde(default)]
    pub tab: String,
    #[serde(default)]
    pub with_schema: bool,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeCreateBody {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// 缺省 `None` 表示沿用目录默认扩展（与旧表单 `ext_form` 缺省语义一致）。
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    /// 软件源（apk）：缺省/`null` = 不改动；`""`/`auto` = 恢复自动探测；
    /// 候选 key 或源根地址 = 固定为该源。仅对 PHP 等 Alpine 镜像生效。
    #[serde(default)]
    pub apk_mirror: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeActionBody {
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct PhpConfigBody {
    #[serde(default)]
    pub memory_limit: String,
    #[serde(default)]
    pub upload_max_filesize: String,
    #[serde(default)]
    pub post_max_size: String,
    #[serde(default)]
    pub max_execution_time: Option<u64>,
    #[serde(default)]
    pub pm: String,
    #[serde(default)]
    pub max_children: Option<u32>,
    #[serde(default)]
    pub start_servers: Option<u32>,
    #[serde(default)]
    pub min_spare_servers: Option<u32>,
    #[serde(default)]
    pub max_spare_servers: Option<u32>,
    #[serde(default)]
    pub max_requests: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct NodeConfigBody {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub health_path: String,
    #[serde(default)]
    pub node_env: String,
    #[serde(default)]
    pub env: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimePortBody {
    pub port: u16,
    #[serde(default)]
    pub confirm: bool,
}

/// `POST /ui/v1/runtimes/{id}/publish`：对外发布宿主端口；缺省或 `null` 表示取消。
#[derive(Debug, Deserialize)]
pub struct RuntimePublishBody {
    #[serde(default)]
    pub publish_port: Option<u16>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeCloneBody {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeExtensionsBody {
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExtensionBatchBody {
    pub op: String,
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PeclBody {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfigFileBody {
    pub key: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct RuntimeImagesBody {
    #[serde(default)]
    pub nginx_image: Option<String>,
    #[serde(default)]
    pub php_image: Option<String>,
}
