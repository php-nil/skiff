//! Web 服务页：子视图与日志筛选、入口绑定与参数。

#[allow(unused_imports)]
use super::super::*;

/// Web 服务页：子视图、日志筛选值与页面骨架开关。
#[derive(Debug, Default, Deserialize)]
pub struct WebPageQuery {
    #[serde(default)]
    pub view: String,
    #[serde(default)]
    pub with_schema: bool,
    /// `1` / `true` 表示本次要读日志（与 `/web/logs` 的 `load` 同义）。
    #[serde(default)]
    pub load: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub lines: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WebLogsQuery {
    /// `1` / `true` 都表示读取；前端 URL 用 `load=1`。
    #[serde(default)]
    pub load: String,
    /// `access` | `error`；缺省按 access。
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub lines: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct WebConfigBody {
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct WebParamsBody {
    #[serde(default)]
    pub worker_processes: String,
    pub worker_connections: u32,
    pub keepalive_timeout: u32,
    #[serde(default)]
    pub client_max_body_size: String,
    #[serde(default)]
    pub gzip: bool,
    #[serde(default)]
    pub gzip_types: String,
}
