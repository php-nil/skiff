//! 主体模型与授权根集合（契约 §2）。
//!
//! 两类主体只有两种形态，不存在「插件无限制读写」：
//! - `Admin`：宿主任意路径，但仍逐条审计；
//! - `Plugin`：只能落在安装时勾选的 `path:` 授权根与自身 `data/` 内。
//!
//! 授权根由调用方从数据库读出后传入（本 crate 不依赖 `skiff-store`），
//! 这里只做「根必须是真实存在的目录」的 canonical 化过滤：根被删/搬走后授权
//! 自动失效，不会因为库里存着旧字符串继续放行。

use std::path::{Path, PathBuf};

use crate::error::FsError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Admin { user: String },
    Plugin { id: String },
}

impl Subject {
    pub fn admin(user: impl Into<String>) -> Self {
        Subject::Admin { user: user.into() }
    }

    pub fn plugin(id: impl Into<String>) -> Self {
        Subject::Plugin { id: id.into() }
    }

    /// 审计表的 `actor_type`。
    pub fn actor_type(&self) -> &'static str {
        match self {
            Subject::Admin { .. } => "admin",
            Subject::Plugin { .. } => "plugin",
        }
    }

    /// 审计表的 `actor_id`。
    pub fn actor_id(&self) -> &str {
        match self {
            Subject::Admin { user } => user,
            Subject::Plugin { id } => id,
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, Subject::Admin { .. })
    }
}

/// 一次调用的主体 + 授权上下文。
#[derive(Debug, Clone)]
pub struct Principal {
    subject: Subject,
    grants: Vec<PathBuf>,
    data_root: Option<PathBuf>,
    allow_protected_write: bool,
}

/// 路径属于哪个 scope：插件 `data/` 还是宿主授权根。跨 scope 改名/复制一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Host,
    Data,
}

impl Principal {
    /// 管理员：无路径白名单，仍全部审计。
    pub fn admin(user: impl Into<String>) -> Self {
        Self {
            subject: Subject::admin(user),
            grants: Vec::new(),
            data_root: None,
            allow_protected_write: false,
        }
    }

    /// 插件：`grants` 是 `path:` 授权根（调用方传入，通常是数据库原始值），
    /// `data_root` 是插件自身 `data/` 目录（隐式授权根）。
    ///
    /// 根在这里统一 canonical 化并过滤掉不存在/非目录项；`data_root` 不存在时
    /// 不自动创建——创建数据目录属于安装/启用流程，不归文件原语。
    pub fn plugin(
        id: impl Into<String>,
        grants: impl IntoIterator<Item = PathBuf>,
        data_root: Option<PathBuf>,
    ) -> Self {
        let data_root = data_root.and_then(canonical_root);
        let mut roots: Vec<PathBuf> = grants.into_iter().filter_map(canonical_root).collect();
        if let Some(data) = &data_root {
            roots.push(data.clone());
        }
        roots.sort();
        roots.dedup();
        Self {
            subject: Subject::plugin(id),
            grants: roots,
            data_root,
            allow_protected_write: false,
        }
    }

    /// 受保护路径（`sites/<domain>/{ssl,conf,log}`）的额外写开关，由安装确认页
    /// 单独勾选（manifest 声明 `fs:write-protected`）。
    pub fn with_protected_write(mut self, allow: bool) -> Self {
        self.allow_protected_write = allow;
        self
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn grants(&self) -> &[PathBuf] {
        &self.grants
    }

    pub fn allow_protected_write(&self) -> bool {
        self.allow_protected_write
    }

    pub fn data_root(&self) -> Option<&PathBuf> {
        self.data_root.as_ref()
    }

    /// 路径属于宿主根还是插件 `data/`；管理员不区分（`None`）。
    pub fn scope_of(&self, path: &Path) -> Option<Scope> {
        if self.subject.is_admin() {
            return None;
        }
        match &self.data_root {
            Some(data) if path.starts_with(data) => Some(Scope::Data),
            _ => Some(Scope::Host),
        }
    }

    /// 插件是否只有 `data/` 一个开口（用于「未授权宿主路径」的判定口径）。
    pub fn has_host_grants(&self) -> bool {
        !self.grants.is_empty()
    }

    /// 目标路径命中的授权根；管理员返回 `None`（表示无根限制）。
    pub fn root_of(&self, path: &Path) -> Option<&PathBuf> {
        if self.subject.is_admin() {
            return None;
        }
        self.grants
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.as_os_str().len())
    }

    /// 删除/改名不允许把授权根本身当成目标（沿用现有 `fsgrant::is_root_itself` 口径）。
    pub fn is_grant_root_itself(&self, path: &Path) -> bool {
        !self.subject.is_admin() && self.grants.iter().any(|root| path == root)
    }
}

/// 授权根必须是真实存在的目录；不满足即视为授权失效。
fn canonical_root(path: PathBuf) -> Option<PathBuf> {
    let canon = crate::path::simplify_windows(std::fs::canonicalize(path).ok()?);
    canon.is_dir().then_some(canon)
}

/// 把原始 `path:` 字符串批量转成 canonical 授权根，供 `Principal::plugin` 使用。
pub fn canonical_grant_roots<I, S>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: Into<PathBuf>,
{
    paths
        .into_iter()
        .filter_map(|p| canonical_root(p.into()))
        .collect()
}

/// 校验授权根字符串形式（绝对路径、无 `..`、无 NUL）。供安装/管理面早失败用。
pub fn validate_grant_root(raw: &str) -> Result<(), FsError> {
    crate::path::normalize_absolute(raw).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_no_root_limit() {
        let p = Principal::admin("root");
        assert!(p.subject().is_admin());
        assert!(p.root_of(Path::new("/etc/passwd")).is_none());
        assert!(!p.is_grant_root_itself(Path::new("/")));
    }

    #[test]
    fn missing_grant_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("sub");
        std::fs::create_dir(&existing).unwrap();
        let p = Principal::plugin(
            "com.example.demo",
            vec![existing.clone(), dir.path().join("gone")],
            None,
        );
        let canon = crate::path::simplify_windows(std::fs::canonicalize(existing).unwrap());
        assert_eq!(p.grants().len(), 1);
        assert_eq!(p.grants()[0], canon);
        assert!(p.has_host_grants());
    }

    #[test]
    fn root_itself_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let p = Principal::plugin("p", vec![root.clone()], None);
        let canon = crate::path::simplify_windows(std::fs::canonicalize(&root).unwrap());
        assert!(p.is_grant_root_itself(&canon));
        assert!(!p.is_grant_root_itself(&canon.join("x")));
    }
}
