//! 把面板里的镜像源真正落到 `/etc/docker/daemon.json`。
//!
//! 流程：读旧文件 → 合并 `registry-mirrors`（保留其它键）→ JSON 校验 →
//! 原子写入（原内容留在内存，不落 `.bak`）→ `systemctl reload docker`（SIGHUP）
//! → `docker info` 校验内存态已切换 → 提交；任何失败或**请求被取消**都由守卫还原原文件。
//!
//! **为什么是 reload 不是 restart**：`registry-mirrors` 属于 dockerd 支持 SIGHUP
//! 热重载的键（同 `insecure-registries` / `debug` / `labels` / `runtimes`），重载
//! **不重建也不重启任何运行中的容器**；需要 restart 的只有 `data-root` /
//! `storage-driver` / `log-driver` / `iptables` 这类。早前按「有运行中容器就不动」
//! 降级，结果是配置写完**根本不生效**、还要用户自己去维护窗口手工 restart。
//!
//! unit 没有 `ExecReload`（`CanReload=no`）时才必须 restart：此时如实报告
//! 「已写入，重启 Docker 后生效」，不在有容器运行时替用户重启。
//!
//! 预算：整段外部变更必须严格短于 web 层请求超时（30 秒），否则 504 会先于
//! 「结果未知」返回，用户既拿不到准确结论，也等不到回滚。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skiff_docker::DockerService;

use crate::admin_fs::AdminFs;

pub const DAEMON_JSON: &str = "/etc/docker/daemon.json";

/// 整段镜像源变更（写文件 + 重启 + 探测 + 可能的回滚重启）的总预算。
///
/// 必须小于 `skiff_web::middleware::REQUEST_TIMEOUT`（30 秒），留出文件 IO、
/// 设置落库与响应组装的时间。web 侧有单测锁定这个大小关系。
pub const MIRROR_APPLY_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
/// 单次 `systemctl` 调用的上限。
const SYSTEMCTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// SIGHUP 是异步的：reload 返回后 dockerd 才读新配置，给它一点时间再判定生效。
const RELOAD_SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
/// 统计运行中容器数的上限：bollard 默认 30 秒会吃满整个请求预算。
const CONTAINER_LIST_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct MirrorApply {
    /// 是否实际写入（内容没变化时为 false）。
    pub written: bool,
    pub mode: MirrorApplyMode,
    /// 变更前的运行中容器数；仅 `NeedsRestart` 时用于提示影响面（读不到为 `None`）。
    pub running_containers: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorApplyMode {
    /// 内容与原文件一致，未写入。
    Unchanged,
    /// 已写入并经 SIGHUP 生效，运行中的容器未受影响。
    Reloaded,
    /// 已写入，但该 unit 不支持 reload——重启 Docker 后才生效，容器未动。
    NeedsRestart,
}

/// 写 `daemon.json` 的还原守卫：未提交前无论正常返回、错误返回还是 future 被
/// 取消（请求超时）都会把原文件写回；提交后 `disarm`，不再动文件。
struct DaemonJsonGuard {
    path: PathBuf,
    original: Option<String>,
    /// 内核写通道：还原也是一次文件写，同样要走 `fs` 并留审计。
    kfs: Arc<AdminFs>,
    armed: bool,
}

impl DaemonJsonGuard {
    fn new(path: &Path, original: &str, kfs: Arc<AdminFs>) -> Self {
        Self {
            path: path.to_path_buf(),
            original: if original.is_empty() {
                None
            } else {
                Some(original.to_string())
            },
            kfs,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn restore(&mut self) -> Result<(), String> {
        self.armed = false;
        match &self.original {
            Some(text) => self
                .kfs
                .write_kernel(&self.path, text.as_bytes())
                .map_err(|e| format!("还原 daemon.json 失败：{e}")),
            // 原本不存在：删掉刚写的那份。文件已不在（并发删除）按成功处理。
            None if !self.path.exists() => Ok(()),
            None => self
                .kfs
                .remove_kernel(&self.path)
                .map_err(|e| format!("删除新建的 daemon.json 失败：{e}")),
        }
    }
}

impl Drop for DaemonJsonGuard {
    fn drop(&mut self) {
        if self.armed {
            // 取消/提前返回时的最后一道防线；同步写回，不能等异步回滚。
            if let Err(e) = self.restore() {
                tracing::error!(error = %e, path = %self.path.display(), "取消后还原 daemon.json 失败");
            } else {
                tracing::warn!(path = %self.path.display(), "daemon.json 变更未提交，已还原");
            }
        }
    }
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
    kfs: &Arc<AdminFs>,
) -> Result<MirrorApply, String> {
    let mirrors = parse_mirrors(raw)?;
    // 只有「文件确实不存在」才按空配置合并；读取失败（EACCES/EIO）时若继续，
    // 合并结果只剩 registry-mirrors，随后整文件覆盖会丢掉 data-root 等既有键。
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    let merged = merge_daemon_json(&existing, &mirrors)?;
    if merged.trim() == existing.trim() {
        return Ok(MirrorApply {
            written: false,
            mode: MirrorApplyMode::Unchanged,
            running_containers: None,
        });
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    }
    let deadline = tokio::time::Instant::now() + MIRROR_APPLY_BUDGET;
    // 走内核文件通道（`fs`）：原子替换 + 一条 `fs.write` 审计（actor=system）。
    kfs.write_kernel(path, merged.as_bytes())
        .map_err(|e| format!("写入 daemon.json 失败：{e}"))?;
    // 从这里开始进入「外部状态已变」区间：任何提前返回或 future 被取消，
    // 守卫都会把原内容同步写回，不需要 `.bak` 文件。
    let mut guard = DaemonJsonGuard::new(path, &existing, Arc::clone(kfs));
    // 只用于 `NeedsRestart` 时告知影响面，不参与决策；读不到就只说「需重启」。
    let running = running_containers(docker).await;

    let reloaded = reload_docker(deadline).await;
    if reloaded.is_ok() && mirrors_settled(docker, &mirrors, deadline).await {
        guard.disarm();
        return Ok(MirrorApply {
            written: true,
            mode: MirrorApplyMode::Reloaded,
            running_containers: running,
        });
    }
    // reload 不可用（unit 没有 `ExecReload`）：配置本身没问题，只是要重启才生效。
    // 保留文件——用户要的就是这份配置；有容器在跑时不替用户重启。
    if !can_reload(deadline).await {
        guard.disarm();
        return Ok(MirrorApply {
            written: true,
            mode: MirrorApplyMode::NeedsRestart,
            running_containers: running,
        });
    }
    // reload 可用却没切过去：dockerd 拒了这份配置，或 systemctl 本身失败——回滚。
    let reason = reloaded.err().unwrap_or_else(|| {
        format!(
            "reload 后 {} 秒内镜像源未切换",
            RELOAD_SETTLE_BUDGET.as_secs()
        )
    });
    let restore_note = match guard.restore() {
        Ok(()) => "已还原原 daemon.json".to_string(),
        Err(e) => format!("还原 daemon.json 失败（{e}），请人工核对"),
    };
    // 把旧配置重新装回 dockerd 内存；SIGHUP 幂等，失败也不改变「已回滚」这个结论。
    if let Err(e) = reload_docker(deadline).await {
        tracing::warn!(error = %e, "回滚后重新 reload docker 失败");
    }
    Err(format!("{reason}；{restore_note}"))
}

/// 统计运行中容器数；失败或超时返回 `None`（不阻塞、不影响镜像源变更本身）。
async fn running_containers(docker: &DockerService) -> Option<usize> {
    match tokio::time::timeout(CONTAINER_LIST_BUDGET, docker.list_containers()).await {
        Ok(Ok(rows)) => Some(rows.iter().filter(|r| r.state == "running").count()),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "读取容器列表失败，跳过影响面统计");
            None
        }
        Err(_) => {
            tracing::warn!("读取容器列表超时，跳过影响面统计");
            None
        }
    }
}

/// 等 dockerd 把新配置装进内存态：轮询 `docker info` 里的 mirrors。
///
/// SIGHUP 是异步的，`systemctl reload` 返回只代表信号已送达。判定依据是
/// **daemon 内存态**而不是文件内容——文件写得对但不生效（配置被 dockerd 拒绝）
/// 正是需要回滚的那种失败。
async fn mirrors_settled(
    docker: &DockerService,
    want: &[String],
    deadline: tokio::time::Instant,
) -> bool {
    let until = (tokio::time::Instant::now() + RELOAD_SETTLE_BUDGET).min(deadline);
    loop {
        if let Ok(Ok(info)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), docker.info()).await
        {
            if same_mirrors(&mirrors_of(&info), want) {
                return true;
            }
        }
        if tokio::time::Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// 从 `docker info` 的 JSON 里取生效中的镜像源。
///
/// 不绑定字段路径：bollard 的 `SystemInfo` 与 `docker info --format` 的键名并不
/// 完全一致，直接按 `registry_config.mirrors` 取会在版本变化时静默变空——那会
/// 让「未生效」被误判成「已生效」。这里深度优先找名为 `mirrors` 的字符串数组。
fn mirrors_of(info: &serde_json::Value) -> Vec<String> {
    fn find(v: &serde_json::Value) -> Option<Vec<String>> {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if k.eq_ignore_ascii_case("mirrors") {
                        if let Some(arr) = val.as_array() {
                            return Some(
                                arr.iter()
                                    .filter_map(|x| x.as_str())
                                    .map(str::to_string)
                                    .collect(),
                            );
                        }
                    }
                }
                map.values().find_map(find)
            }
            serde_json::Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    find(info).unwrap_or_default()
}

/// 比较期望与实际：dockerd 会给地址补尾部 `/`，比较前统一去掉。
fn same_mirrors(actual: &[String], want: &[String]) -> bool {
    fn norm(s: &str) -> String {
        s.trim().trim_end_matches('/').to_ascii_lowercase()
    }
    actual.len() == want.len() && actual.iter().zip(want).all(|(a, b)| norm(a) == norm(b))
}

/// 让 dockerd 重读配置（SIGHUP）；不重启进程，运行中的容器不受影响。
async fn reload_docker(deadline: tokio::time::Instant) -> Result<(), String> {
    systemctl(deadline, "reload", "systemctl reload docker").await
}

/// 该 unit 是否支持 reload（是否有 `ExecReload`）。不支持时只能 restart 才生效。
async fn can_reload(deadline: tokio::time::Instant) -> bool {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return false;
    }
    let limit = remaining.min(std::time::Duration::from_secs(3));
    let args: Vec<String> = ["show", "docker", "-p", "CanReload"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let spec = skiff_host::HostSpec::privileged("systemctl", &args)
        .prefix("skiff-docker")
        .description("skiff docker can-reload")
        .timeout(limit.as_secs().max(1));
    match crate::host::run_host(spec).await {
        Ok(out) if out.success => out.stdout.trim().eq_ignore_ascii_case("yes"),
        Ok(out) => {
            tracing::warn!(code = ?out.code, stderr = %out.stderr, "systemctl show CanReload 失败");
            false
        }
        Err(e) => {
            tracing::warn!(error = %e.message("systemctl show"), "查询 Reload 能力失败");
            false
        }
    }
}

async fn systemctl(
    deadline: tokio::time::Instant,
    action: &str,
    label: &str,
) -> Result<(), String> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(format!(
            "请求预算（{} 秒）已用完，未执行 {label}",
            MIRROR_APPLY_BUDGET.as_secs()
        ));
    }
    let limit = remaining.min(SYSTEMCTL_TIMEOUT);
    // 统一执行通道（`skiff_host`）：跑在 systemd-run 瞬态单元里，超时后通道会
    // `systemctl stop` 单元——比只杀本地客户端干净。文案仍要区分「结果未知」。
    let args: Vec<String> = [action, "docker"].iter().map(|s| s.to_string()).collect();
    let description = format!("skiff docker {action}");
    let spec = skiff_host::HostSpec::privileged("systemctl", &args)
        .prefix("skiff-docker")
        .description(&description)
        .timeout(limit.as_secs().max(1));
    match crate::host::run_host(spec).await {
        Ok(out) if out.success => Ok(()),
        Ok(out) => Err(format!("{label} 退出码 {:?}", out.code)),
        Err(skiff_host::HostRunError::Timeout { .. }) => Err(format!(
            "{label} 超过 {} 秒未返回（结果未知）",
            limit.as_secs()
        )),
        Err(e) => Err(e.message(label)),
    }
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
    fn mirrors_are_read_from_docker_info_without_binding_path() {
        // `docker info` 的实际形态：registry_config 下 mirrors，地址带尾斜杠。
        let info = serde_json::json!({
            "ID": "abc",
            "RegistryConfig": { "Mirrors": ["https://a.example/", "https://b.example/"] }
        });
        assert_eq!(
            mirrors_of(&info),
            vec!["https://a.example/", "https://b.example/"]
        );
        // 用户清空镜像源时 daemon 不返回该键：按空列表处理，不能 panic，
        // 否则「清空镜像源」会被判成「未生效」而误回滚。
        assert!(mirrors_of(&serde_json::json!({ "ID": "abc" })).is_empty());
    }

    #[test]
    fn mirror_comparison_ignores_trailing_slash_and_case() {
        let want = vec!["https://A.example".to_string()];
        assert!(same_mirrors(&["https://a.example/".to_string()], &want));
        assert!(!same_mirrors(&[], &want));
        assert!(!same_mirrors(&["https://b.example".to_string()], &want));
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

    /// 测试用的内核文件通道：内存库 + 临时数据根，审计不落盘但走完整路径。
    fn kfs_with_db(dir: &Path) -> (Arc<AdminFs>, Arc<skiff_store::Db>) {
        let db = Arc::new(skiff_store::Db::open_in_memory().unwrap());
        let fs = Arc::new(AdminFs::with_protected(
            dir.to_path_buf(),
            dir.join("www"),
            Arc::clone(&db),
            30,
            Vec::new(),
        ));
        (fs, db)
    }

    fn kfs(dir: &Path) -> Arc<AdminFs> {
        kfs_with_db(dir).0
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skiff-daemon-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    /// 模拟请求在「已写新值、尚未重启完成」之间被取消：守卫 drop 必须把原文件写回，
    /// 且不留下任何 `.bak` 文件。
    #[test]
    fn cancelled_apply_restores_original_without_bak_files() {
        let dir = temp_dir("cancel");
        let path = dir.join("daemon.json");
        let original = r#"{"log-driver":"local"}"#;
        std::fs::write(&path, original).expect("写原文件");
        let fs = kfs(&dir);
        {
            let mut guard = DaemonJsonGuard::new(&path, original, Arc::clone(&fs));
            fs.write_kernel(
                &path,
                br#"{"log-driver":"local","registry-mirrors":["https://new.example"]}"#,
            )
            .expect("写新值");
            // 未 disarm，直接离开作用域 = 请求被取消。
            let _ = &mut guard;
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let baks: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".bak"))
            .collect();
        assert!(baks.is_empty(), "不应遗留备份文件：{baks:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn committed_guard_keeps_new_content() {
        let dir = temp_dir("commit");
        let path = dir.join("daemon.json");
        std::fs::write(&path, r#"{"a":1}"#).expect("写原文件");
        let fs = kfs(&dir);
        let mut guard = DaemonJsonGuard::new(&path, r#"{"a":1}"#, Arc::clone(&fs));
        fs.write_kernel(&path, br#"{"a":2}"#).expect("写新值");
        guard.disarm();
        drop(guard);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"a":2}"#);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn guard_removes_file_that_did_not_exist_before() {
        let dir = temp_dir("create");
        let path = dir.join("daemon.json");
        let fs = kfs(&dir);
        let guard = DaemonJsonGuard::new(&path, "", Arc::clone(&fs));
        fs.write_kernel(&path, br#"{"registry-mirrors":[]}"#)
            .expect("写新值");
        drop(guard);
        assert!(!path.exists(), "原先不存在的文件应被删除");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 内核写系统配置必须留下审计：`actor=system` + `fs.write` + 目标路径。
    /// 这条是「用记录代替权限配置」的最小断言——写发生了，就查得到。
    #[test]
    fn kernel_write_leaves_audit_trace() {
        let dir = temp_dir("audit");
        let (fs, db) = kfs_with_db(&dir);
        let path = dir.join("daemon.json");
        fs.write_kernel(&path, br#"{"registry-mirrors":[]}"#)
            .expect("写入");

        let rows = db.recent_audit(20).expect("读审计");
        let hit = rows
            .iter()
            .find(|r| r.action == "fs.write" && r.target.ends_with("daemon.json"))
            .expect("应当有一条 fs.write 审计");
        assert_eq!(hit.actor, "system");
        assert_eq!(hit.result, "ok");
        std::fs::remove_dir_all(&dir).ok();
    }
}
