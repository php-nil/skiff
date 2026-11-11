//! 发布面事件 outbox（§17 R2a）。
//!
//! 业务侧只做一件事：把一条不可变事件写进 `state/outbox/pending/`。网络投递由独立的
//! `skiff-server notify` 进程完成，因此 webhook 失败不会拖住 API 主路径。
//!
//! `event_id` 由业务键确定性派生：同一业务动作重复 emit 时文件名相同，`create_new`
//! 直接去重。事件内容不含包体、请求体或任何签名。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EVENT_SCHEMA: &str = "skiff.server.event/v1";
pub const EVENT_TYPE_SUBMISSION_CREATED: &str = "submission.created";
pub const EVENT_TYPE_SUBMISSION_APPROVED: &str = "submission.approved";
pub const EVENT_TYPE_SUBMISSION_REJECTED: &str = "submission.rejected";
pub const EVENT_TYPE_SUBMISSION_WITHDRAWN: &str = "submission.withdrawn";
pub const EVENT_TYPE_SUBMISSION_PUBLISHED: &str = "submission.published";
pub const EVENT_TYPE_TAKEDOWN_YANKED: &str = "takedown.yanked";
pub const EVENT_TYPE_KEY_REVOKED: &str = "key.revoked";
pub const EVENT_TYPE_RELEASE_FAILED: &str = "release.failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subject {
    pub kind: String,
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub schema: String,
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub at: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_key_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submission_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reopened: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Event {
    pub fn new(event_type: &str, key: &str, channel: &str) -> Self {
        Self {
            schema: EVENT_SCHEMA.to_string(),
            event_id: event_id(event_type, key),
            event_type: event_type.to_string(),
            at: crate::api::now_rfc3339(),
            channel: channel.to_string(),
            actor_key_id: None,
            subject: None,
            submission_ids: Vec::new(),
            seq: None,
            reopened: false,
            reason: None,
            error_code: None,
        }
    }

    pub fn actor(mut self, key_id: &str) -> Self {
        self.actor_key_id = Some(key_id.to_string());
        self
    }

    pub fn subject(mut self, kind: &str, id: &str, version: &str) -> Self {
        self.subject = Some(Subject {
            kind: kind.to_string(),
            id: id.to_string(),
            version: version.to_string(),
        });
        self
    }

    pub fn submission(mut self, sid: &str) -> Self {
        self.submission_ids = vec![sid.to_string()];
        self
    }

    pub fn submissions(mut self, ids: &[String]) -> Self {
        self.submission_ids = ids.to_vec();
        self
    }

    pub fn seq(mut self, seq: u64) -> Self {
        self.seq = Some(seq);
        self
    }

    pub fn reopened(mut self, reopened: bool) -> Self {
        self.reopened = reopened;
        self
    }

    pub fn reason(mut self, reason: &str) -> Self {
        self.reason = Some(reason.to_string());
        self
    }

    pub fn error_code(mut self, code: &str) -> Self {
        self.error_code = Some(code.to_string());
        self
    }
}

/// 稳定事件 id：同一业务动作重复 emit 得到同一个 id。
pub fn event_id(event_type: &str, key: &str) -> String {
    let digest = Sha256::digest(format!("{event_type}\0{key}").as_bytes());
    format!("evt-{}", &hex::encode(digest)[..24])
}

pub fn pending_dir(state: &Path) -> PathBuf {
    state.join("outbox").join("pending")
}

pub fn state_dir(state: &Path) -> PathBuf {
    state.join("outbox").join("state")
}

pub fn delivered_dir(state: &Path) -> PathBuf {
    state.join("outbox").join("delivered")
}

pub fn dead_dir(state: &Path) -> PathBuf {
    state.join("outbox").join("dead")
}

/// 写入 outbox。**调用方必须忽略返回值**：通知失败不得改变业务结果。
pub fn emit(state: &Path, event: &Event) -> Result<(), String> {
    let dir = pending_dir(state);
    fs::create_dir_all(&dir).map_err(|e| format!("创建 outbox 失败: {e}"))?;
    let path = dir.join(format!("{}.json", event.event_id));
    let bytes = serde_json::to_vec_pretty(event).map_err(|e| e.to_string())?;
    let mut file = match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        // 确定性 id 已存在：同一次业务动作，直接去重。
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("写 outbox 失败: {error}")),
    };
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.flush()) {
        let _ = fs::remove_file(&path);
        return Err(format!("写 outbox 失败: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_is_deterministic_and_business_scoped() {
        let a = event_id(EVENT_TYPE_SUBMISSION_CREATED, "sid-1");
        let b = event_id(EVENT_TYPE_SUBMISSION_CREATED, "sid-1");
        let c = event_id(EVENT_TYPE_SUBMISSION_CREATED, "sid-2");
        let d = event_id(EVENT_TYPE_SUBMISSION_APPROVED, "sid-1");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert!(a.starts_with("evt-"));
    }

    #[test]
    fn emit_deduplicates_by_event_id() {
        let dir = tempfile::tempdir().unwrap();
        let event = Event::new(EVENT_TYPE_SUBMISSION_CREATED, "sid-1", "stable")
            .actor("a".repeat(64).as_str())
            .subject("plugin", "com.example.demo", "1.0.0")
            .submission("sid-1");
        emit(dir.path(), &event).unwrap();
        emit(dir.path(), &event).unwrap();
        let count = fs::read_dir(pending_dir(dir.path())).unwrap().count();
        assert_eq!(count, 1, "同一业务动作只能落一条事件");
    }

    #[test]
    fn payload_never_contains_package_or_signature() {
        let event =
            Event::new(EVENT_TYPE_SUBMISSION_CREATED, "sid-1", "stable").submission("sid-1");
        let text = serde_json::to_string(&event).unwrap();
        for forbidden in ["package", "signature", "wbpkg"] {
            assert!(!text.contains(forbidden), "{text}");
        }
        assert!(text.contains("\"type\":\"submission.created\""));
    }
}
