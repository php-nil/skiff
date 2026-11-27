//! 站点域：列表项、详情（证书 / 运行环境 / 配置 / 备份）、查询与全部写操作请求体。

#[allow(unused_imports)]
use super::super::*;

#[derive(Debug, Clone, Serialize)]
pub struct SiteHttps {
    pub enabled: bool,
    pub force_https: bool,
    pub label: String,
    pub class: String,
}

/// 站点列表项（字段冻结，只增不改；与页面 `SiteView` 同源）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteItem {
    /// 站点主键：页面深链与详情接口只暴露它，域名变更不影响 URL。
    pub id: i64,
    pub domain: String,
    pub root_dir: String,
    pub index_dir: String,
    pub site_type: String,
    pub type_label: String,
    pub runtime_id: String,
    pub runtime_label: String,
    pub runtime_name: String,
    pub runtime_shared_hint: String,
    pub proxy_target: String,
    pub proxy_websocket: bool,
    pub endpoint: String,
    pub status: StatusDto,
    /// 巡检判定不可用的原因；空串表示正常（状态标签此时为「维护中」）。
    pub maintenance_reason: String,
    pub https: SiteHttps,
    pub is_default: bool,
    pub aliases: Vec<String>,
    /// 伪静态预设键（`none` / `wordpress` / `laravel` / `thinkphp`）。
    pub rewrite: String,
    /// 出口限速（KB/s）；0 表示不限制。
    pub limit_rate_kbps: u32,
    /// RFC3339 UTC 单字段（D9）。
    pub created_at: String,
}

/// 站点详情里的证书信息（只读）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteSsl {
    pub enabled: bool,
    pub force_https: bool,
    pub cert_exists: bool,
    pub cert_path: String,
    pub key_path: String,
    pub subject: String,
    pub issuer: String,
    pub expires: String,
}

/// 站点详情里绑定的运行环境（只读快照）。
#[derive(Debug, Clone, Serialize)]
pub struct SiteRuntimeRef {
    pub id: String,
    pub name: String,
    pub kind: StatusDto,
    pub version: String,
    pub state: StatusDto,
    pub site_count: usize,
}

/// 站点 vhost 配置快照；读取失败时 `error` 非空，其余字段为空。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteConfigInfo {
    pub path: String,
    pub content: String,
    pub active: bool,
    pub error: String,
}

/// 站点备份开关快照（任务服务未初始化时 `enabled=false`、`available=false`）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteBackupInfo {
    pub available: bool,
    pub enabled: bool,
    pub task_id: String,
    /// 备份间隔：`daily` / `weekly` / `monthly`。
    pub interval: String,
    /// 本地执行时间 `HH:MM`。
    pub time: String,
    pub last_text: String,
    pub next_text: String,
    /// 已有归档（最近若干份，按时间倒序）；空表表示还没跑过备份。
    pub archives: Vec<SiteArchiveItem>,
}

/// 一份站点备份归档：页面直接渲染，不需要再请求一次。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteArchiveItem {
    /// 归档文件名，恢复接口原样回传。
    pub name: String,
    pub bytes: u64,
    /// 人类可读大小，避免前端各写一套换算。
    pub size_text: String,
    /// 归档时间（本地时区）。
    pub modified_text: String,
}

/// 站点详情响应数据（列表项字段 + 证书 + 运行环境状态 + 配置/备份 + 写权限）。
#[derive(Debug, Serialize)]
pub struct SiteDetailData {
    #[serde(flatten)]
    pub site: SiteItem,
    pub ssl: SiteSsl,
    pub runtime: Option<SiteRuntimeRef>,
    pub runtime_down: bool,
    pub runtime_alert: String,
    pub config: SiteConfigInfo,
    pub backup: SiteBackupInfo,
    pub can_manage: bool,
}

/// `GET /ui/v1/sites` 的查询参数：列表参数 + 是否附带服务端 Schema。
///
/// `with_schema` 只在页面加载时由前端传 `true`；表格自身的游标分页不带该参数，
/// 避免每次翻页都重建页面骨架（其中包含运行环境选项查询）。
/// 注意这里不复用 `ListQuery` 的 `flatten`：`serde_urlencoded` 对 flatten 支持不可靠。
#[derive(Debug, Default, Deserialize)]
pub struct SitesQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

/// `GET /ui/v1/sites/{id}` 的查询参数：L3 子视图（缺省 / 未知值按概览）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteDetailQuery {
    pub tab: Option<String>,
}

/// `POST /ui/v1/sites` 的请求体：字段与旧新建表单同名。
#[derive(Debug, Default, Deserialize)]
pub struct SiteCreateBody {
    #[serde(default)]
    pub site_type: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub aliases_bulk: String,
    #[serde(default)]
    pub root_dir: String,
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub proxy_target: String,
    #[serde(default)]
    pub proxy_websocket: bool,
}

/// `POST /ui/v1/sites/{id}/domains`：新增 / 删除别名（`SITE_UPDATE_DOMAINS`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteDomainBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub alias: String,
}

/// `POST /ui/v1/sites/{id}/config`：保存 vhost 配置（`SITE_SAVE_CONFIG`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteConfigBody {
    #[serde(default)]
    pub content: String,
}

/// `POST /ui/v1/sites/{id}/ssl`：证书生成 / 上传 / 强制跳转 / 关闭 / 删除（`SITE_SSL`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteSslBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cert_pem: String,
    #[serde(default)]
    pub key_pem: String,
}

/// `POST /ui/v1/sites/{id}/policy`：伪静态预设 + 出口限速。
#[derive(Debug, Default, Deserialize)]
pub struct SitePolicyBody {
    #[serde(default)]
    pub rewrite: String,
    #[serde(default)]
    pub limit_rate_kbps: u32,
}

/// `POST /ui/v1/sites/{id}/backup`：备份开关与计划。
#[derive(Debug, Default, Deserialize)]
pub struct SiteBackupBody {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub interval: String,
    #[serde(default)]
    pub time: String,
}

/// `POST /ui/v1/sites/{id}/backups/restore`：用归档覆盖站点目录（`SITE_RESTORE`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteRestoreBody {
    /// 归档文件名（只能是 `<data>/backups/sites/<域名>/` 下的一层文件名）。
    #[serde(default)]
    pub archive: String,
    /// 用户手输的域名；命令层要求与站点完全一致。
    #[serde(default)]
    pub confirm: String,
}

/// `POST /ui/v1/sites/{id}/delete`：删除站点，服务端强制域名确认（`SITE_DELETE`）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteDeleteBody {
    #[serde(default)]
    pub confirm: String,
}

/// `POST /ui/v1/sites/batch`：批量启停（`SITE_BATCH_CREATE`；批量删除仍由命令层拒绝）。
#[derive(Debug, Default, Deserialize)]
pub struct SiteBatchBody {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub domains: Vec<String>,
}
