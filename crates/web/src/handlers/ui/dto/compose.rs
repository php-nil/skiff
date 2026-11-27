//! Compose 项目：项目与服务、校验 / 启动 / 停止 / 源文件。

#[allow(unused_imports)]
use super::super::*;

#[derive(Debug, Serialize)]
pub struct ComposeProjectItem {
    pub name: String,
    pub managed: bool,
    pub services: usize,
    pub running: usize,
}

#[derive(Debug, Serialize)]
pub struct ComposeServiceItem {
    pub service: String,
    pub container: String,
    pub id: String,
    pub state: StatusDto,
    pub status: String,
    pub image: String,
    pub managed: bool,
    pub logs: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ComposeQuery {
    pub project: Option<String>,
    #[serde(default)]
    pub with_schema: bool,
}

#[derive(Debug, Deserialize)]
pub struct ComposeDownBody {
    pub project: String,
    #[serde(default)]
    pub remove_volumes: bool,
}

/// compose 源文本 + 可选项目名（与旧页面同名）。
#[derive(Debug, Default, Deserialize)]
pub struct ComposeSourceBody {
    #[serde(default)]
    pub compose: String,
    #[serde(default)]
    pub project: String,
}
