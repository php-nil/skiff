//! 容器引用解析：完整 id / 名称 / id 前缀 → 具体容器。
//!
//! 「什么算一个容器引用」是 Docker 领域的语义，属于 `skiff-docker`；表现层
//! （web）与内核服务（core）只是调用方。此前这份逻辑住在 `skiff-web`，导致
//! CLI/SDK 侧要用就得反向依赖表现层。

use crate::client::ContainerRow;

/// 容器引用解析失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    /// 引用为空。
    Empty,
    /// 没有匹配的容器。
    NotFound(String),
    /// id 前缀命中多个容器：容器操作是破坏性的，猜错比拒绝更糟。
    Ambiguous(String),
}

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchError::Empty => write!(f, "容器引用不能为空"),
            MatchError::NotFound(m) => write!(f, "{m}"),
            MatchError::Ambiguous(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MatchError {}

/// 按引用匹配容器（纯逻辑，与 Docker 列表解耦，便于单测）。
///
/// 顺序是语义的一部分：**精确 id / 名称优先于前缀**。单趟 `find` 会让列表顺序
/// 决定结果，出现「查 `abc` 却操作了 `abc123`」——精确命中被前面的前缀命中吃掉。
/// 前缀不唯一时报歧义而不是静默取第一个。
pub fn match_container(containers: &[ContainerRow], id: &str) -> Result<ContainerRow, MatchError> {
    if id.is_empty() {
        return Err(MatchError::Empty);
    }
    // 阶段一：精确 id / 名称。必须扫完整份列表才能进下一阶段，否则「前缀命中在前、
    // 精确命中在后」时仍会先返回前缀那个。
    if let Some(pos) = containers.iter().position(|c| c.id == id || c.name == id) {
        return Ok(containers[pos].clone());
    }
    // 阶段二：id 前缀，且要求唯一
    let mut hits = containers.iter().filter(|c| c.id.starts_with(id));
    let first = hits
        .next()
        .ok_or_else(|| MatchError::NotFound(format!("容器不存在: {id}")))?;
    if hits.next().is_some() {
        return Err(MatchError::Ambiguous(format!(
            "容器引用有歧义，请提供更长的 id 前缀: {id}"
        )));
    }
    Ok(first.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn row(id: &str, name: &str) -> ContainerRow {
        ContainerRow {
            id: id.into(),
            name: name.into(),
            image: "nginx".into(),
            image_id: "sha256:cdn".into(),
            state: "running".into(),
            status: "Up".into(),
            labels: BTreeMap::new(),
            project: None,
            owner: None,
            protected: false,
            unprotected_kernel: false,
        }
    }

    fn list() -> Vec<ContainerRow> {
        vec![
            row("abc123def456", "web"),
            row("dead0000beef", "db"),
            row("abc999zzz", "cache"),
        ]
    }

    #[test]
    fn exact_id_wins_over_prefix() {
        // 列表顺序不能决定结果：前缀命中排在精确命中前面也必须选精确那个。
        let got = match_container(&list(), "abc123def456").unwrap();
        assert_eq!(got.name, "web");
    }

    #[test]
    fn name_matches() {
        let got = match_container(&list(), "web").unwrap();
        assert_eq!(got.id, "abc123def456");
    }

    #[test]
    fn unique_prefix_matches() {
        let got = match_container(&list(), "dead").unwrap();
        assert_eq!(got.name, "db");
    }

    #[test]
    fn ambiguous_prefix_is_rejected() {
        // `abc` 同时命中 abc123def456 与 abc999zzz：拒绝，而不是取第一个。
        assert!(matches!(
            match_container(&list(), "abc"),
            Err(MatchError::Ambiguous(_))
        ));
    }

    #[test]
    fn unknown_reference_is_not_found() {
        assert!(matches!(
            match_container(&list(), "zzz"),
            Err(MatchError::NotFound(_))
        ));
    }

    #[test]
    fn empty_reference_is_rejected() {
        assert_eq!(match_container(&list(), "").unwrap_err(), MatchError::Empty);
    }
}
