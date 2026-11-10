//! 运行环境（`site_runtimes`）的持久化。
//!
//! 运行环境是站点的「版本基线 + 运行实例」业务对象，不是 Docker 容器的别名。
//! `site_runtimes` 是它的唯一事实源；`sites.runtime_id` 只引用，不复制版本信息。
//!
//! P1 起 PHP（socket）、Rust 与 Node.js（固定端口进程）均可运行；Go 允许落行
//! （字段预留），但服务层禁止安装/启动/创建绑定站点，避免出现「假运行中」。

use super::{now, Db, Result};
use crate::error::StoreError;
use rusqlite::{params, OptionalExtension, Row};

pub const KIND_PHP: &str = "php";
pub const KIND_NODE: &str = "node";
pub const KIND_GO: &str = "go";
pub const KIND_RUST: &str = "rust";

/// 用户创建的真实实例。
pub const ORIGIN_INSTANCE: &str = "instance";
/// 迁移种入、从未安装的占位行（只作模板，不进列表）。
pub const ORIGIN_SEED: &str = "seed";

pub fn validate_kind(kind: &str) -> Result<&'static str> {
    match kind {
        KIND_PHP => Ok(KIND_PHP),
        KIND_NODE => Ok(KIND_NODE),
        KIND_GO => Ok(KIND_GO),
        KIND_RUST => Ok(KIND_RUST),
        other => Err(StoreError::Conflict(format!("未知运行环境类型：{other}"))),
    }
}

/// 实例名规则：`^[a-z0-9][a-z0-9.-]{0,31}$`。
///
/// 名字只用于本地目录与容器名，不能带路径分隔符；首字符必须是字母或数字，
/// 因此也不会以点/连字符开头。
pub fn validate_runtime_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > 32 {
        return Err(StoreError::Conflict(
            "运行环境名需为 1–32 个字符（小写字母、数字、点、连字符）".into(),
        ));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or(' ');
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(StoreError::Conflict(
            "运行环境名必须以小写字母或数字开头".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
    {
        return Err(StoreError::Conflict(
            "运行环境名只能包含小写字母、数字、点与连字符".into(),
        ));
    }
    Ok(name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    /// 稳定 slug：`php-8.5` / `node-22` / `go-1.24`，创建后不可改。
    pub id: String,
    pub kind: String,
    /// 本地实例名（同 kind 内唯一）；目录与容器名只从它推导。
    pub name: String,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    /// running / stopped，用户期望。
    pub desired_state: String,
    /// installing / running / stopped / error，最近一次探测结果（缓存）。
    pub last_status: String,
    /// 仅 PHP：FPM socket 的宿主绝对路径；vhost 由它换算容器路径，禁止硬编码。
    pub socket_path: Option<String>,
    /// 仅进程型环境（Rust / Node.js）：环境固定监听端口（host 网络）；站点 vhost 反代到它。
    /// PHP 走 socket，此列为空。
    pub port: Option<u16>,
    /// 同一 kind 内最多一个默认版本。
    pub is_default: bool,
    /// 专属配置的规范化 JSON 快照（PHP ini / FPM）。
    pub config_json: String,
    /// 已启用的 PHP 扩展（catalog key 的逗号分隔串；清单本身在应用层单一来源）。
    pub extensions: String,
    /// `instance`：用户创建的真实实例；`seed`：迁移种入、未安装的占位行（只作模板，不进列表）。
    pub origin: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewRuntime {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    pub socket_path: Option<String>,
    /// 仅进程型环境（Rust / Node.js）：固定监听端口。
    pub port: Option<u16>,
    pub is_default: bool,
}

fn row_to_runtime(r: &Row<'_>) -> rusqlite::Result<Runtime> {
    Ok(Runtime {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        version: r.get(3)?,
        image: r.get(4)?,
        image_digest: r.get(5)?,
        desired_state: r.get(6)?,
        last_status: r.get(7)?,
        socket_path: r.get(8)?,
        port: r.get(9)?,
        is_default: r.get::<_, i64>(10)? != 0,
        config_json: r.get(11)?,
        extensions: r.get(12)?,
        origin: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

const RUNTIME_COLS: &str = "id, kind, name, version, image, image_digest, desired_state, \
                            last_status, socket_path, port, is_default, config_json, \
                            extensions, origin, created_at, updated_at";

impl Db {
    pub fn create_runtime(&self, r: &NewRuntime) -> Result<Runtime> {
        let kind = validate_kind(&r.kind)?;
        let name = validate_runtime_name(&r.name)?;
        let id = r.id.trim();
        if id.is_empty() {
            return Err(StoreError::Conflict("运行环境 id 不能为空".into()));
        }
        let conn = self.conn();
        let ts = now();
        let tx = conn.unchecked_transaction()?;
        // 先把同 kind 的旧默认清掉再插入：`idx_runtime_default` 是部分唯一索引，
        // 反过来写会让「创建新的默认实例」直接被约束拒绝并误报为「已存在」。
        if r.is_default {
            tx.execute(
                "UPDATE site_runtimes SET is_default = 0 WHERE kind = ?1",
                params![kind],
            )?;
        }
        tx.execute(
            "INSERT INTO site_runtimes(id, kind, name, version, image, image_digest,
                                       desired_state, last_status, socket_path, port, is_default,
                                       config_json, extensions, origin, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'running', 'stopped', ?7, ?8, ?9, '{}', '', 'instance', ?10, ?10)",
            params![
                id,
                kind,
                name,
                r.version.trim(),
                r.image,
                r.image_digest,
                r.socket_path,
                r.port.map(i64::from),
                i64::from(r.is_default),
                ts
            ],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                StoreError::Conflict(format!("运行环境已存在：{kind}/{name}"))
            }
            other => StoreError::Sqlite(other),
        })?;
        tx.commit()?;
        drop(conn);
        self.runtime_by_id(id)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }

    pub fn runtime_by_name(&self, kind: &str, name: &str) -> Result<Option<Runtime>> {
        let conn = self.conn();
        conn.query_row(
            &format!("SELECT {RUNTIME_COLS} FROM site_runtimes WHERE kind = ?1 AND name = ?2"),
            params![kind, name],
            row_to_runtime,
        )
        .optional()
        .map_err(StoreError::from)
    }

    /// 幂等落行：已存在则只补齐镜像与 socket（不覆盖用户改过的期望状态与配置）。
    pub fn ensure_runtime(&self, r: &NewRuntime) -> Result<Runtime> {
        match self.runtime_by_id(&r.id)? {
            Some(existing) => {
                self.update_runtime_image(
                    &r.id,
                    &r.image,
                    r.socket_path.as_deref(),
                    &r.image_digest,
                )?;
                Ok(self.runtime_by_id(&r.id)?.unwrap_or(existing))
            }
            None => self.create_runtime(r),
        }
    }

    pub fn list_runtimes(&self) -> Result<Vec<Runtime>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUNTIME_COLS} FROM site_runtimes ORDER BY kind ASC, version DESC"
        ))?;
        let rows = stmt.query_map([], row_to_runtime)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn runtime_by_id(&self, id: &str) -> Result<Option<Runtime>> {
        let conn = self.conn();
        conn.query_row(
            &format!("SELECT {RUNTIME_COLS} FROM site_runtimes WHERE id = ?1"),
            params![id],
            row_to_runtime,
        )
        .optional()
        .map_err(StoreError::from)
    }

    pub fn update_runtime_image(
        &self,
        id: &str,
        image: &str,
        socket_path: Option<&str>,
        image_digest: &str,
    ) -> Result<()> {
        let conn = self.conn();
        // 空 digest 表示「本次调用未探测」：保留库里已有值，避免每次启动校准把它清零。
        let n = conn.execute(
            "UPDATE site_runtimes SET image = ?2, socket_path = ?3,
                 image_digest = CASE WHEN ?4 = '' THEN image_digest ELSE ?4 END,
                 updated_at = ?5
             WHERE id = ?1",
            params![id, image, socket_path, image_digest, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 写回探测结果与期望状态；`last_status` 只作缓存，不参与业务判定。
    pub fn set_runtime_status(&self, id: &str, desired: &str, last: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE site_runtimes SET desired_state = ?2, last_status = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, desired, last, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 写回已启用的扩展集合（逗号分隔，调用方按 catalog 规范化）。
    pub fn set_runtime_extensions(&self, id: &str, extensions: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE site_runtimes SET extensions = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, extensions, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn set_runtime_config(&self, id: &str, config_json: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE site_runtimes SET config_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, config_json, now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 修改进程型环境（Rust / Node.js）的固定端口。
    ///
    /// 调用方必须先用 `runtime_catalog::port_range` 校验区间与占用；这里只负责落库，
    /// 端口是环境属性，站点侧不持有。
    pub fn set_runtime_port(&self, id: &str, port: u16) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute(
            "UPDATE site_runtimes SET port = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, i64::from(port), now()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// 切换同 kind 的默认版本；部分唯一索引逐行校验，先清旧默认再置新。
    pub fn set_runtime_default(&self, id: &str) -> Result<()> {
        // 先查 kind 再拿连接，避免持锁期间再次 conn() 造成自锁。
        let kind = self
            .runtime_by_id(id)?
            .ok_or_else(|| StoreError::NotFound(id.to_string()))?
            .kind;
        let conn = self.conn();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE site_runtimes SET is_default = 0, updated_at = ?2
             WHERE kind = ?1 AND is_default = 1",
            params![kind, now()],
        )?;
        tx.execute(
            "UPDATE site_runtimes SET is_default = 1, updated_at = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 绑定到该运行环境的站点域名（删除保护要用它列出影响面）。
    pub fn runtime_site_domains(&self, runtime_id: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT domain FROM sites WHERE runtime_id = ?1 ORDER BY created_at ASC, domain ASC",
        )?;
        let rows = stmt.query_map(params![runtime_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn runtime_site_count(&self, runtime_id: &str) -> Result<usize> {
        let conn = self.conn();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sites WHERE runtime_id = ?1",
            params![runtime_id],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    /// 删除运行环境。存在绑定站点时由外键 `ON DELETE RESTRICT` 拒绝；
    /// 服务层还会先做一次可读校验并列出域名，这里只做兜底。
    pub fn delete_runtime(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        let n = conn.execute("DELETE FROM site_runtimes WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn php(id: &str, version: &str, default: bool) -> NewRuntime {
        NewRuntime {
            id: id.into(),
            kind: KIND_PHP.into(),
            name: id.into(),
            version: version.into(),
            image: "php:8.5-fpm-alpine".into(),
            image_digest: "sha256:demo".into(),
            socket_path: Some(format!("/www/runtime/php/{id}/php-fpm.sock")),
            port: None,
            is_default: default,
        }
    }

    #[test]
    fn runtime_crud_and_unique_kind_name() {
        let db = Db::open_in_memory().unwrap();
        // v7 迁移会种入 php-8.5 默认行。
        let seeded = db.runtime_by_id("php-8.5").unwrap().unwrap();
        assert_eq!(seeded.kind, KIND_PHP);
        assert_eq!(seeded.name, "php-8.5");
        assert!(seeded.is_default);
        assert!(
            seeded.socket_path.is_none(),
            "从未安装的种子行不登记 socket 路径"
        );

        let dup = db.create_runtime(&php("php-8.5", "8.5", false));
        assert!(matches!(dup, Err(StoreError::Conflict(_))));

        let other = db.create_runtime(&php("php-8.4", "8.4", false)).unwrap();
        assert!(!other.is_default);
        assert_eq!(db.list_runtimes().unwrap().len(), 2);

        // 同版本不同 name 允许（name 才是唯一标识）。
        let second = db.create_runtime(&php("php-8.5-a", "8.5", false)).unwrap();
        assert_eq!(second.name, "php-8.5-a");
        assert_eq!(second.version, "8.5");
        assert_eq!(db.list_runtimes().unwrap().len(), 3);
    }

    #[test]
    fn create_runtime_can_replace_default_of_same_kind() {
        let db = Db::open_in_memory().unwrap();
        // v7 迁移种入的 php-8.5 已是默认；新实例应能直接抢过默认位。
        let rt = db.create_runtime(&php("php-8.4", "8.4", true)).unwrap();
        assert!(rt.is_default);
        assert!(!db.runtime_by_id("php-8.5").unwrap().unwrap().is_default);
        assert_eq!(
            db.list_runtimes()
                .unwrap()
                .iter()
                .filter(|r| r.is_default)
                .count(),
            1
        );
    }

    #[test]
    fn create_runtime_failure_does_not_clear_existing_default() {
        let db = Db::open_in_memory().unwrap();
        db.create_runtime(&php("php-8.4", "8.4", false)).unwrap();
        // 重复 name 触发唯一约束：清默认与插入必须一起回滚。
        let dup = db.create_runtime(&php("php-8.4", "8.4", true));
        assert!(matches!(dup, Err(StoreError::Conflict(_))));
        assert!(db.runtime_by_id("php-8.5").unwrap().unwrap().is_default);
    }

    #[test]
    fn ensure_runtime_writes_digest_but_never_clears_it_with_a_blank_probe() {
        let db = Db::open_in_memory().unwrap();
        let mut rt = php("php-8.5", "8.5", true);
        rt.image_digest = "sha256:first".into();
        let saved = db.ensure_runtime(&rt).unwrap();
        assert_eq!(saved.image_digest, "sha256:first");

        // 启动校准只传空 digest：必须保留旧值。
        rt.image_digest = String::new();
        let kept = db.ensure_runtime(&rt).unwrap();
        assert_eq!(kept.image_digest, "sha256:first");

        // 安装/重建探测到新 digest：覆盖旧值。
        rt.image_digest = "sha256:second".into();
        let updated = db.ensure_runtime(&rt).unwrap();
        assert_eq!(updated.image_digest, "sha256:second");
    }

    #[test]
    fn default_is_exclusive_per_kind() {
        let db = Db::open_in_memory().unwrap();
        db.create_runtime(&php("php-8.4", "8.4", false)).unwrap();
        db.set_runtime_default("php-8.4").unwrap();
        let rows = db.list_runtimes().unwrap();
        assert_eq!(rows.iter().filter(|r| r.is_default).count(), 1);
        assert!(db.runtime_by_id("php-8.4").unwrap().unwrap().is_default);
        assert!(!db.runtime_by_id("php-8.5").unwrap().unwrap().is_default);
    }

    #[test]
    fn runtime_name_rules_and_cross_kind_same_name() {
        for bad in ["", ".php", "-php", "PHP-8.5", "php_85", "php 85", "a/b"] {
            assert!(validate_runtime_name(bad).is_err(), "应拒绝：{bad}");
        }
        assert!(validate_runtime_name("php-8.5").is_ok());
        assert!(validate_runtime_name("php-8.5-a").is_ok());

        let db = Db::open_in_memory().unwrap();
        // 同 kind 重名必须拒绝。
        let dup = db.create_runtime(&php("php-8.5", "8.5", false));
        assert!(matches!(dup, Err(StoreError::Conflict(_))));

        // 跨 kind 同名允许（路径天然按 kind 分开）。
        let node = NewRuntime {
            id: "node-php-8.5".into(),
            kind: KIND_NODE.into(),
            name: "php-8.5".into(),
            version: "22".into(),
            image: "node:22".into(),
            image_digest: String::new(),
            socket_path: None,
            port: Some(9200),
            is_default: false,
        };
        db.create_runtime(&node).unwrap();
        let stored = db.runtime_by_name(KIND_NODE, "php-8.5").unwrap().unwrap();
        // 进程型环境（Node / Rust）的固定端口随记录持久化，重建时据此复用。
        assert_eq!(stored.port, Some(9200));

        // 同版本多实例允许：name 不同即可。
        let mut same_version = php("php-8.5-a", "8.5", false);
        same_version.id = "php-8.5-a".into();
        db.create_runtime(&same_version).unwrap();
        assert_eq!(
            db.runtime_by_name(KIND_PHP, "php-8.5-a")
                .unwrap()
                .unwrap()
                .version,
            "8.5"
        );
        assert_eq!(db.list_runtimes().unwrap().len(), 3);
    }

    #[test]
    fn status_and_config_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.set_runtime_status("php-8.5", "stopped", "stopped")
            .unwrap();
        let rt = db.runtime_by_id("php-8.5").unwrap().unwrap();
        assert_eq!(rt.desired_state, "stopped");
        assert_eq!(rt.last_status, "stopped");

        db.set_runtime_config("php-8.5", r#"{"memory_limit":"256M"}"#)
            .unwrap();
        assert_eq!(
            db.runtime_by_id("php-8.5").unwrap().unwrap().config_json,
            r#"{"memory_limit":"256M"}"#
        );
    }
}
