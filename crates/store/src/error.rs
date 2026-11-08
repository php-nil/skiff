use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "数据库 schema 版本 {found} 高于本二进制支持的 {supported}，拒绝启动（向前迁移不可逆）"
    )]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("记录不存在: {0}")]
    NotFound(String),
    #[error("唯一约束冲突: {0}")]
    Conflict(String),
    #[error("目标忙: {0}")]
    Busy(String),
    #[error("数据格式错误: {0}")]
    Decode(String),
}
