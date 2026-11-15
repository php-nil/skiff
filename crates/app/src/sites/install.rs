//! 运行环境安装的进度视图、互斥表与环境探测结果。
//!
//! 从 `sites.rs` 拆出：这些类型都围绕「一次安装的生命周期」，与站点 CRUD /
//! vhost 渲染无关；拆分后 `sites.rs` 只保留站点模型与对外再导出。

use serde::{Deserialize, Serialize};

use super::StepState;
use crate::jobs as app_jobs;

/// 一步安装进度。`state` 对外只用 `pending/running/ok/failed`；
/// 时间戳为 Unix 秒，`detail` 成功时是结果摘要、失败时是错误原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallStep {
    pub key: String,
    pub name: String,
    pub state: StepState,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
}

/// Web 服务安装的唯一进度视图：页面、`/ui/v1/web`、`/ui/v1/web/install`
/// 与旧 SSR 都从同一份作业快照构建，不存在第二套口径。
#[derive(Debug, Clone, Default)]
pub struct InstallProgress {
    pub job_id: Option<String>,
    pub status: String,
    pub running: bool,
    pub done: bool,
    pub interrupted: bool,
    pub phase: String,
    pub phase_label: String,
    pub progress: i64,
    pub error: String,
    pub created_by: String,
    pub created_by_role: String,
    pub requester_ip: String,
    pub created_at: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: i64,
    pub steps: Vec<InstallStep>,
}

impl InstallProgress {
    /// 从任务中心最新一条 `web.install` 作业构建视图；没有历史时返回 `idle`。
    pub fn from_job(job: Option<&app_jobs::Job>) -> Self {
        let Some(job) = job else {
            return Self {
                status: "idle".into(),
                ..Self::default()
            };
        };
        let steps: Vec<InstallStep> = serde_json::from_value(job.steps.clone()).unwrap_or_default();
        let error = job
            .error_detail
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                steps
                    .iter()
                    .rev()
                    .find(|s| s.state == StepState::Failed)
                    .map(|s| s.detail.clone())
            })
            .unwrap_or_default();
        Self {
            job_id: Some(job.id.clone()),
            status: job.status.as_str().to_string(),
            running: job.active(),
            done: job.status.terminal(),
            interrupted: job.error_class.as_deref() == Some("interrupted"),
            phase: job.phase.clone(),
            phase_label: job.phase_label.clone(),
            progress: job.progress,
            error,
            created_by: job.created_by.clone(),
            created_by_role: job.created_by_role.clone(),
            requester_ip: job.requester_ip.clone(),
            created_at: Some(job.created_at),
            started_at: job.started_at,
            finished_at: job.finished_at,
            duration_ms: if job.active() {
                let started = job.started_at.unwrap_or(job.created_at);
                (unix_now() - started).max(0) * 1000
            } else {
                job.duration_ms
            },
            steps,
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 运行环境安装互斥表：键为 `kind:name`。
///
/// 只防「同一实例被并发安装」；不同实例互不影响。守卫 drop 时自动释放，
/// 安装失败也不会把实例永久锁死。
#[derive(Default)]
pub struct InstallLocks {
    busy: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl std::fmt::Debug for InstallLocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallLocks").finish_non_exhaustive()
    }
}

pub struct InstallLock<'a> {
    locks: &'a InstallLocks,
    key: String,
}

impl Drop for InstallLock<'_> {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.locks.busy.lock() {
            busy.remove(&self.key);
        }
    }
}

impl InstallLocks {
    pub fn try_begin(&self, key: &str) -> Option<InstallLock<'_>> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.insert(key.to_string()) {
            return None;
        }
        Some(InstallLock {
            locks: self,
            key: key.to_string(),
        })
    }
}

/// 运行环境探测结果（列表页/环境页共用）。
#[derive(Debug, Clone)]
pub struct SiteEnvStatus {
    pub docker_available: bool,
    pub docker_reason: String,
    pub nginx_state: String,
    /// Web 服务容器实际镜像 id（列表「Web 服务」行展示；缺失时为空）。
    pub nginx_image_id: String,
    pub installed: bool,
    pub port: u16,
    pub port_message: String,
    pub nginx_image: String,
    pub php_image: String,
}

impl SiteEnvStatus {
    /// **仅 Web 服务（OpenResty）就绪**。PHP/Node/Go/Rust 是独立运行环境，
    /// 由建站时按所选实例单独校验，不做任何合并判断。
    pub fn ready(&self) -> bool {
        self.docker_available && self.installed && self.nginx_state == "running"
    }
}
