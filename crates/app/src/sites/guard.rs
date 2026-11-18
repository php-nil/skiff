//! 站点守护巡检：把运行环境可用性变成站点 vhost 的维护态。
//!
//! 运行环境容器接入专用 bridge 后，站点 vhost 只指向静态分配的容器地址；宿主上
//! 同号端口与站点无关。但容器仍可能退出、crash-loop、地址漂移或应用不响应，
//! 这时必须把站点切到 503 维护态，而不是继续转发。
//!
//! 巡检由 `core` 的秒级循环驱动（不依赖用户打开页面）：每轮评估所有被运行中
//! 站点引用的运行环境，只在「可用 ↔ 维护」发生翻转时重写 vhost 并 reload nginx。

use std::path::Path;
use std::time::Duration;

use skiff_store::{Runtime, KIND_NODE, KIND_PHP, KIND_RUST, TYPE_RUNTIME};

use super::health::{check_runtime_ready, readiness_message};
use super::{nginx_container_running, runtime_container_name, SiteSettings};
use crate::runtime_catalog;
use crate::AppState;

/// 巡检周期：秒级发现窗口，远小于用户感知的「站点一直转给陌生进程」。
pub const SITE_GUARD_INTERVAL: Duration = Duration::from_secs(3);

/// 一个运行环境的最近一次可用性判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGuard {
    pub available: bool,
    /// 维护原因（`available = false` 时非空，含恢复路径）。
    pub reason: String,
    pub checked_at: i64,
}

impl RuntimeGuard {
    pub(crate) fn ok() -> Self {
        Self {
            available: true,
            reason: String::new(),
            checked_at: now(),
        }
    }

    fn down(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: reason.into(),
            checked_at: now(),
        }
    }
}

fn now() -> i64 {
    crate::time_fmt::unix_now()
}

impl AppState {
    /// 页面读取：运行环境最近一次判定。
    pub fn runtime_guard(&self, runtime_id: &str) -> Option<RuntimeGuard> {
        self.site_guard.lock().ok()?.get(runtime_id).cloned()
    }

    pub(crate) fn set_runtime_guard(&self, runtime_id: &str, guard: RuntimeGuard) {
        if let Ok(mut cache) = self.site_guard.lock() {
            cache.insert(runtime_id.to_string(), guard);
        }
    }

    /// 单次巡检。由 `core` 的循环驱动；失败只记日志，不影响主服务。
    pub async fn site_guard_tick(&self) {
        if !self.docker().available() {
            return;
        }
        let Ok(rows) = self.docker().list_containers().await else {
            return;
        };
        let Ok(sites) = self.db().list_sites() else {
            return;
        };
        let Ok(runtimes) = self.db().list_runtimes() else {
            return;
        };
        let s = self.site_settings();

        let mut active: Vec<(String, String)> = Vec::new(); // (runtime_id, domain)
        for site in sites
            .iter()
            .filter(|s| s.status == "running" && s.site_type == TYPE_RUNTIME)
        {
            if let Some(rid) = site.runtime_id.clone() {
                active.push((rid, site.domain.clone()));
            }
        }
        if active.is_empty() {
            return;
        }

        let mut changed_runtimes: Vec<String> = Vec::new();
        let mut evaluated: Vec<String> = Vec::new();
        for (rid, _) in &active {
            if evaluated.contains(rid) {
                continue;
            }
            evaluated.push(rid.clone());
            let Some(rt) = runtimes.iter().find(|r| r.id == *rid) else {
                continue;
            };
            let guard = self.evaluate_runtime(&s, rt, &rows).await;
            let prev = self.runtime_guard(rid);
            let changed = prev
                .as_ref()
                .is_none_or(|p| p.available != guard.available || p.reason != guard.reason);
            if changed {
                changed_runtimes.push(rid.clone());
            }
            self.set_runtime_guard(rid, guard);
        }
        if changed_runtimes.is_empty() {
            return;
        }

        let mut rewritten = false;
        for (rid, domain) in &active {
            if !changed_runtimes.contains(rid) {
                continue;
            }
            match self.write_site_vhost(&s, domain) {
                Ok(()) => rewritten = true,
                Err(e) => tracing::warn!(domain, error = %e, "巡检重写站点配置失败"),
            }
        }
        if !rewritten || !nginx_container_running(&rows) {
            return;
        }
        let _lock = super::nginx_write_lock().lock().await;
        if let Err(e) = self.validate_nginx_locked().await {
            tracing::warn!(error = %e, "巡检生成的配置未通过 nginx -t，保持原配置");
            return;
        }
        if let Err(e) = self.activate_nginx_locked().await {
            tracing::warn!(error = %e, "巡检重载 nginx 失败");
        }
    }

    async fn evaluate_runtime(
        &self,
        s: &SiteSettings,
        rt: &Runtime,
        rows: &[skiff_docker::ContainerRow],
    ) -> RuntimeGuard {
        let container = runtime_container_name(&rt.kind, &rt.name);
        match rt.kind.as_str() {
            KIND_PHP => {
                let socket = s.runtime_socket(&rt.kind, &rt.name);
                let running = rows
                    .iter()
                    .any(|c| c.name == container && c.state == "running");
                if !running {
                    return RuntimeGuard::down(format!(
                        "{} 容器未运行；站点已进入维护态，容器恢复后自动回到正常服务",
                        runtime_catalog::kind_label(&rt.kind)
                    ));
                }
                if !socket_alive(&socket) {
                    return RuntimeGuard::down(format!(
                        "{} socket {} 不可连接；站点已进入维护态，容器恢复后自动回到正常服务",
                        runtime_catalog::kind_label(&rt.kind),
                        socket.display()
                    ));
                }
                RuntimeGuard::ok()
            }
            KIND_NODE | KIND_RUST => {
                let Some(ip) = rt.container_ip.as_deref().filter(|ip| !ip.is_empty()) else {
                    return RuntimeGuard::down(
                        "运行环境尚未分配容器地址，请在运行环境页重建后再启用站点",
                    );
                };
                let Some(port) = rt.port else {
                    return RuntimeGuard::down("运行环境缺少内部端口，请重建后再启用站点");
                };
                let http_path = if rt.kind == KIND_NODE {
                    Some(crate::sites::NodeRuntimeConfig::from_json(&rt.config_json).health_path)
                } else {
                    None
                };
                match check_runtime_ready(self.docker(), &container, ip, port, http_path.as_deref())
                    .await
                {
                    Ok(()) => RuntimeGuard::ok(),
                    Err(failure) => RuntimeGuard::down(readiness_message(
                        &runtime_catalog::kind_label(&rt.kind),
                        ip,
                        port,
                        http_path.as_deref(),
                        &failure,
                    )),
                }
            }
            _ => RuntimeGuard::ok(),
        }
    }
}

#[cfg(unix)]
fn socket_alive(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_alive(_path: &Path) -> bool {
    false
}
