//! API Token 的 scope 词汇（§6.1「API Token 带 scope」）。
//!
//! 语义（本轮定稿，此前 `scopes` 只存不判）：
//! - scope 是**收窄**，不是提权：token 的有效权限 = 所属用户角色的权限 ∩ scope；
//! - 空 scope 集 = 该 token 不能访问 `/api/v1` 任何需要 scope 的端点（fail-closed）；
//! - `docker:write` 蕴含 `docker:read`（能重启就能列，否则每个写调用都得配两条）；
//! - `*` 等于全部 scope，但仍受角色约束（viewer 的 `*` 也做不了 exec）。
//!
//! 解析是**严格**的：未知 scope 直接报错而不是忽略——拼错一个词应当立刻可见，
//! 而不是变成「这个 token 什么都干不了」的哑谜。

use std::fmt;

/// API 能力（粗粒度）。细到动词的授权由 `skiff-policy` 负责，scope 只管「这类 API 能不能碰」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    DockerRead,
    DockerWrite,
    PluginsRead,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::DockerRead => "docker:read",
            Scope::DockerWrite => "docker:write",
            Scope::PluginsRead => "plugins:read",
        }
    }

    /// 全部 scope（`*` 展开后的集合）。
    pub fn all() -> &'static [Scope] {
        &[Scope::DockerRead, Scope::DockerWrite, Scope::PluginsRead]
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Self::all().iter().copied().find(|x| x.as_str() == s)
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一组 scope。`Default` 是空集——「没写 scope」等于「什么都不许」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopeSet {
    docker_read: bool,
    docker_write: bool,
    plugins_read: bool,
}

impl ScopeSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn all() -> Self {
        Self {
            docker_read: true,
            docker_write: true,
            plugins_read: true,
        }
    }

    /// 解析逗号分隔的 scope 串；空串合法（空集），未知词报错。
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut set = Self::default();
        for raw in s.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            if item == "*" {
                return Ok(Self::all());
            }
            match Scope::parse(item) {
                Some(Scope::DockerRead) => set.docker_read = true,
                // 蕴含关系在解析时就展开，集合本身就是规范形式（便于相等比较）
                Some(Scope::DockerWrite) => {
                    set.docker_write = true;
                    set.docker_read = true;
                }
                Some(Scope::PluginsRead) => set.plugins_read = true,
                None => return Err(format!("未知 scope: {item}")),
            }
        }
        Ok(set)
    }

    pub fn is_empty(&self) -> bool {
        !self.docker_read && !self.docker_write && !self.plugins_read
    }

    /// `docker:write` 蕴含 `docker:read`——写权限不给读权限只会逼调用方多配一条。
    pub fn allows(&self, scope: Scope) -> bool {
        match scope {
            Scope::DockerRead => self.docker_read || self.docker_write,
            Scope::DockerWrite => self.docker_write,
            Scope::PluginsRead => self.plugins_read,
        }
    }

    /// 规范化字符串（写库/打印用），顺序固定便于比对。
    ///
    /// `docker:write` 一定伴随 `docker:read` 落库：蕴含关系写成显式两条，读库的人
    /// 不用去翻 `allows()` 的实现才知道这个 token 到底能读不能读。
    pub fn to_wire(&self) -> String {
        let mut out: Vec<&str> = Vec::new();
        if self.docker_read || self.docker_write {
            out.push(Scope::DockerRead.as_str());
        }
        if self.docker_write {
            out.push(Scope::DockerWrite.as_str());
        }
        if self.plugins_read {
            out.push(Scope::PluginsRead.as_str());
        }
        out.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_grants_nothing() {
        let s = ScopeSet::parse("").unwrap();
        assert!(s.is_empty());
        for scope in Scope::all() {
            assert!(!s.allows(*scope), "空 scope 集不应允许 {scope}");
        }
    }

    #[test]
    fn write_implies_read_but_not_the_reverse() {
        let s = ScopeSet::parse("docker:write").unwrap();
        assert!(s.allows(Scope::DockerWrite));
        assert!(s.allows(Scope::DockerRead), "写应蕴含读");
        assert!(!s.allows(Scope::PluginsRead));

        let r = ScopeSet::parse("docker:read").unwrap();
        assert!(r.allows(Scope::DockerRead));
        assert!(!r.allows(Scope::DockerWrite), "读不应蕴含写");
    }

    #[test]
    fn star_grants_everything() {
        let s = ScopeSet::parse("*").unwrap();
        for scope in Scope::all() {
            assert!(s.allows(*scope));
        }
    }

    #[test]
    fn unknown_scope_is_an_error_not_a_silent_drop() {
        // 旧 CLI 写入的占位值 `api` 必须显式报错，而不是被当成「全都允许」
        assert!(ScopeSet::parse("api").is_err());
        assert!(ScopeSet::parse("docker:reed").is_err());
        assert!(ScopeSet::parse("docker:read, nope").is_err());
    }

    #[test]
    fn wire_format_is_canonical() {
        let s = ScopeSet::parse(" plugins:read , docker:write ").unwrap();
        assert_eq!(s.to_wire(), "docker:read,docker:write,plugins:read");
        assert_eq!(ScopeSet::parse(&s.to_wire()).unwrap(), s);
    }
}
