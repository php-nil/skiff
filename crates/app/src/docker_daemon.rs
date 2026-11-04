//! 把面板里的镜像源真正落到 `/etc/docker/daemon.json`。
//!
//! 流程：读旧文件 → 合并 `registry-mirrors`（保留其它键）→ JSON 校验 →
//! 备份 → 原子写入 → （无运行中容器时）`systemctl restart docker` → 失败回滚。
//! 有运行中容器时**不硬重启**，只提示管理员在维护窗口重启，避免把业务打断。

use std::path::{Path, PathBuf};

use skiff_docker::DockerService;

pub const DAEMON_JSON: &str = "/etc/docker/daemon.json";

#[derive(Debug, Clone)]
pub struct MirrorApply {
    /// 是否实际写入（内容没变化时为 false）。
    pub written: bool,
    /// 是否重启了 docker。
    pub restarted: bool,
    /// 重启前检测到的运行中容器数（>0 时不重启）。
    pub running_containers: usize,
    /// 备份文件路径（首次创建 daemon.json 时为 None）。
    pub backup: Option<PathBuf>,
}

/// 解析界面里填的镜像源：逗号/空白/换行分隔，必须带 http(s)://。
pub fn parse_mirrors(raw: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = raw
        .split(|c: char| c == ',' || c == '，' || c.is_whitespace())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .collect();
    for m in &list {
        if !(m.starts_with("http://") || m.starts_with("https://")) {
            return Err(format!("镜像源必须以 http:// 或 https:// 开头：{m}"));
        }
    }
    Ok(list)
}

/// 合并镜像源到已有 daemon.json 文本（空文件按 `{}` 处理），返回格式化后的 JSON。
pub fn merge_daemon_json(existing: &str, mirrors: &[String]) -> Result<String, String> {
    let mut root: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| format!("daemon.json 不是合法 JSON：{e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "daemon.json 顶层必须是对象".to_string())?;
    if mirrors.is_empty() {
        obj.remove("registry-mirrors");
    } else {
        obj.insert(
            "registry-mirrors".to_string(),
            serde_json::Value::Array(
                mirrors
                    .iter()
                    .map(|m| serde_json::Value::String(m.clone()))
                    .collect(),
            ),
        );
    }
    serde_json::to_string_pretty(&root).map_err(|e| format!("序列化失败：{e}"))
}

/// 写入并（在安全前提下）重启 Docker；失败时恢复备份并再次尝试重启。
pub async fn apply_registry_mirror(
    docker: &DockerService,
    path: &Path,
    raw: &str,
) -> Result<MirrorApply, String> {
    let mirrors = parse_mirrors(raw)?;
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let merged = merge_daemon_json(&existing, &mirrors)?;
    if merged.trim() == existing.trim() {
        return Ok(MirrorApply {
            written: false,
            restarted: false,
            running_containers: 0,
            backup: None,
        });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    }
    let backup = if path.is_file() {
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let bak = path.with_file_name(format!("daemon.json.bak.{ts}"));
        std::fs::copy(path, &bak).map_err(|e| format!("备份 daemon.json 失败：{e}"))?;
        Some(bak)
    } else {
        None
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &merged).map_err(|e| format!("写入临时文件失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换 daemon.json 失败：{e}"))?;

    let rows = docker
        .list_containers()
        .await
        .map_err(|e| format!("写入后读取容器列表失败：{e}"))?;
    let running = rows.iter().filter(|r| r.state == "running").count();
    if running > 0 {
        return Ok(MirrorApply {
            written: true,
            restarted: false,
            running_containers: running,
            backup,
        });
    }

    let restarted = restart_docker().await;
    // dockerd 重启会打断面板已持有的连接池：第一次探测常因旧连接失效而报错，
    // 重试几次再判定，避免把「已恢复但首个请求失败」误判成重启失败而回滚。
    let healthy = restarted.is_ok() && docker_ready(docker).await;
    if !healthy {
        let reason = restarted
            .err()
            .unwrap_or_else(|| "docker 重启后不可用".to_string());
        if let Some(bak) = &backup {
            let _ = std::fs::copy(bak, path);
        } else {
            let _ = std::fs::remove_file(path);
        }
        let _ = restart_docker().await;
        return Err(format!("{reason}；已恢复原 daemon.json"));
    }
    Ok(MirrorApply {
        written: true,
        restarted: true,
        running_containers: 0,
        backup,
    })
}

async fn docker_ready(docker: &DockerService) -> bool {
    for _ in 0..8 {
        if docker.list_containers().await.is_ok() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    false
}

async fn restart_docker() -> Result<(), String> {
    let status = tokio::process::Command::new("systemctl")
        .args(["restart", "docker"])
        .status()
        .await
        .map_err(|e| format!("无法执行 systemctl restart docker：{e}"))?;
    if !status.success() {
        return Err(format!(
            "systemctl restart docker 退出码 {:?}",
            status.code()
        ));
    }
    // 等 socket 重新可用（最多 30 秒）。
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if std::path::Path::new("/var/run/docker.sock").exists() {
            return Ok(());
        }
    }
    Err("等待 docker.sock 恢复超时".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_list_must_have_scheme() {
        assert_eq!(
            parse_mirrors("https://a.example, https://b.example/").unwrap(),
            vec![
                "https://a.example".to_string(),
                "https://b.example".to_string()
            ]
        );
        assert!(parse_mirrors("a.example").is_err());
        assert!(parse_mirrors("").unwrap().is_empty());
    }

    #[test]
    fn merge_keeps_other_keys_and_replaces_mirrors() {
        let existing = r#"{"log-driver":"local","registry-mirrors":["https://old.example"]}"#;
        let out = merge_daemon_json(existing, &["https://new.example".into()]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["log-driver"], "local");
        assert_eq!(v["registry-mirrors"][0], "https://new.example");
    }

    #[test]
    fn empty_mirrors_removes_key_and_keeps_file_valid() {
        let out = merge_daemon_json(r#"{"debug":true}"#, &[]).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("registry-mirrors").is_none());
        assert_eq!(v["debug"], true);
    }

    #[test]
    fn invalid_existing_json_is_rejected_not_overwritten() {
        assert!(merge_daemon_json("{oops", &["https://a.example".into()]).is_err());
        assert!(merge_daemon_json("[]", &["https://a.example".into()]).is_err());
    }
}
