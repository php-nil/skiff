//! 文件管理：根目录、条目、列表数据与全部写操作请求体（含回收站与配额）。

#[allow(unused_imports)]
use super::super::*;

#[derive(Debug, Serialize)]
pub struct FileRootItem {
    pub label: String,
    pub path: String,
}

/// 目录项：只出机器可读字段（时间按 D9 出 RFC3339 UTC），本地化归前端。
#[derive(Debug, Serialize)]
pub struct FileEntryItem {
    pub name: String,
    /// 绝对路径；前端直接用于下载 / 重命名 / 删除，不再自行拼接。
    pub path: String,
    pub dir: bool,
    pub symlink: bool,
    pub size: u64,
    pub mtime: Option<String>,
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub nlink: u64,
    pub link_target: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FilesData {
    pub path: String,
    pub parent: Option<String>,
    pub roots: Vec<FileRootItem>,
    pub entries: Vec<FileEntryItem>,
    /// 条目数超过单页上限（1000），前端必须提示而不是静默截断。
    pub truncated: bool,
    pub max_edit_bytes: usize,
    /// 分片上传单片字节数，前端上传适配器按它切片。
    pub upload_chunk_bytes: usize,
    /// 是否具备 Admin（回收站/配额入口显隐；页面本身仍各自鉴权）。
    pub can_admin: bool,
}

#[derive(Debug, Deserialize)]
pub struct FilePathBody {
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct FileRenameBody {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// `POST /ui/v1/files/write`：文本写入（内容可以为空，用于新建空文件）。
#[derive(Debug, Deserialize)]
pub struct FileWriteBody {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct FileCopyBody {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize)]
pub struct FileChmodBody {
    #[serde(default)]
    pub path: String,
    /// 八进制字符串（`0755` / `0o2775`）或数字；两种都要接受。
    #[serde(default)]
    pub mode: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct FileChownBody {
    #[serde(default)]
    pub path: String,
    /// 缺失与 `null` 都必须拒绝，避免默认成 0（root）。
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct FileArchiveBody {
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize)]
pub struct FileExtractBody {
    #[serde(default)]
    pub archive: String,
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /ui/v1/files/trash/restore`：按回收站编号还原，可选覆盖同名目标。
#[derive(Debug, Deserialize)]
pub struct TrashRestoreBody {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub overwrite: bool,
}

/// `POST /ui/v1/files/trash/purge`：按回收站编号彻底删除。
#[derive(Debug, Deserialize)]
pub struct TrashIdBody {
    #[serde(default)]
    pub id: String,
}

/// `POST /ui/v1/files/quotas/set`：设置目录配额（字节数 / 文件数均可单独为 null）。
#[derive(Debug, Deserialize)]
pub struct QuotaSetBody {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<u64>,
    #[serde(default)]
    pub max_files: Option<u64>,
}
