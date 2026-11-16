//! 宿主命令执行通道：**唯一实现**。
//!
//! 两类调用方共用这里，不允许各自拼 `Command`：
//! - 插件经 `exec` 能力面提交（内核先做授权判定，再交给本 crate 执行）；
//! - 内核自己发起的动作（作业执行体里的宿主命令、只读探测）。
//!
//! 授权不在这里：拿到什么就执行什么。把判定塞进来会让「谁来决定能跑什么」
//! 出现两个答案。
//!
//! 两种执行模式（同一套读流/超时/取消/清理）：
//! - [`HostSpec::privileged`]：经 `systemd-run` 起瞬态单元，**不继承主服务沙箱**、
//!   以 root 执行——特权动作（apt/systemctl/pure-pw/nft）走这条；
//! - [`HostSpec::direct`]：直接起子进程，不碰 systemd——只读探测与用户目录内的
//!   普通命令走这条，免得为一次 `openssl x509` 也 fork 一个单元。
//!
//! 两个入口是同一份逻辑的两种消费方式：
//! - [`run`]：等完再拿完整输出；
//! - [`run_stream`]：边跑边出行（命令执行作业要把过程写进运行日志）。

// 下面这些只在 Linux 实现里用到；非 Linux 是立即失败的 stub，不引管道类型。
#[cfg(target_os = "linux")]
use std::process::Stdio;

#[cfg(target_os = "linux")]
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
#[cfg(target_os = "linux")]
use tokio::sync::mpsc;

/// 默认超时：调用方没显式给时用。够跑完大多数配置类命令，又不至于卡住作业。
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// 一次宿主命令的规格。
///
/// `prefix` 只用于单元名可读性（`skiff-exec-*` / `skiff-ftp-*`）与排障定位。
#[derive(Debug, Clone)]
pub struct HostSpec<'a> {
    pub cmd: &'a str,
    pub args: &'a [String],
    pub prefix: &'a str,
    /// systemd 单元描述；留空时用 `cmd` 兜底。
    pub description: &'a str,
    pub stdin: Option<&'a str>,
    pub timeout_secs: u64,
    pub privileged: bool,
}

impl<'a> HostSpec<'a> {
    /// 特权执行：`systemd-run` 瞬态单元（root，脱离主服务沙箱）。
    pub fn privileged(cmd: &'a str, args: &'a [String]) -> Self {
        Self {
            cmd,
            args,
            prefix: "skiff-host",
            description: "",
            stdin: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            privileged: true,
        }
    }

    /// 普通执行：直接起子进程，不经 systemd。
    pub fn direct(cmd: &'a str, args: &'a [String]) -> Self {
        Self {
            privileged: false,
            ..Self::privileged(cmd, args)
        }
    }

    pub fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = prefix;
        self
    }

    pub fn description(mut self, description: &'a str) -> Self {
        self.description = description;
        self
    }

    pub fn stdin(mut self, stdin: Option<&'a str>) -> Self {
        self.stdin = stdin;
        self
    }

    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// 执行结果。退出码为 `None` 表示被信号终止（没有退出码可报）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// 通道层失败：调用方据此区分「命令跑了但非零退出」（不是错误）与「没跑完」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRunError {
    /// 起进程失败（`systemd-run` 缺失、权限不足、二进制不存在）。
    Spawn(String),
    /// 读写管道失败。
    Io(String),
    /// 超时：已尝试终止，但真实副作用可能仍在。
    Timeout { secs: u64, unit: String },
    /// 调用方请求取消：已终止。
    Cancelled,
}

impl HostRunError {
    /// 面向日志/用户的一句话说明。`cmd` 只用于拼接，不做任何转义或解析。
    pub fn message(&self, cmd: &str) -> String {
        match self {
            Self::Spawn(e) => format!("执行 {cmd} 失败：{e}"),
            Self::Io(e) => format!("等待 {cmd} 失败：{e}"),
            Self::Timeout { secs, unit } => {
                if unit.is_empty() {
                    format!("{cmd} 执行超时（{secs}s），已尝试终止")
                } else {
                    format!("{cmd} 执行超时（{secs}s），已尝试终止单元 {unit}")
                }
            }
            Self::Cancelled => format!("{cmd} 已被取消"),
        }
    }
}

/// `systemd-run` 的完整 argv（不含可执行文件本身）。
///
/// 纯函数：单测在非 Linux 上也能验证「`--` 之后才是目标命令」与参数原样透传。
pub fn systemd_run_argv(unit: &str, description: &str, cmd: &str, args: &[String]) -> Vec<String> {
    let mut full: Vec<String> = [
        "--unit",
        unit,
        "--wait",
        "--pipe",
        "--collect",
        "--quiet",
        "--description",
        description,
        "--",
        cmd,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    full.extend(args.iter().cloned());
    full
}

/// 执行并等完，拿完整输出；每条输出行交给 `on_line`（`stream` 为 `"out"`/`"err"`）。
///
/// `cancel` 每 250ms 轮询一次，返回 true 即终止。限制输出由调用方自己做
/// （本 crate 只保留头部 [`ACCUM_CAP`]，逐行消费方自行决定保留多少）。
#[cfg(target_os = "linux")]
pub async fn run_stream<F>(
    spec: HostSpec<'_>,
    cancel: Option<&(dyn Fn() -> bool + Send + Sync)>,
    mut on_line: F,
) -> Result<HostOutput, HostRunError>
where
    F: FnMut(&str, &str) + Send,
{
    let (mut child, unit) = spawn(&spec).await?;
    let timeout_secs = spec.timeout_secs.max(1);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HostRunError::Io("子进程没有 stdout 管道（内部错误）".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| HostRunError::Io("子进程没有 stderr 管道（内部错误）".to_string()))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<(&'static str, String)>();
    let tx_err = tx.clone();
    let out_task = tokio::spawn(pump(stdout, "out", tx));
    let err_task = tokio::spawn(pump(stderr, "err", tx_err));

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut failure: Option<HostRunError> = None;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some((stream, line)) => on_line(stream, &line),
                None => break,
            },
            _ = tick.tick() => {
                if cancel.is_some_and(|c| c()) {
                    failure = Some(HostRunError::Cancelled);
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    failure = Some(HostRunError::Timeout {
                        secs: timeout_secs,
                        unit: unit.clone().unwrap_or_default(),
                    });
                    break;
                }
            }
        }
    }

    let (stdout, stderr) = (
        out_task.await.unwrap_or_default(),
        err_task.await.unwrap_or_default(),
    );

    if let Some(err) = failure {
        cleanup(&mut child, unit.as_deref()).await;
        return Err(err);
    }

    let waited = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
    let status = match waited {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(HostRunError::Io(e.to_string())),
        // 管道已 EOF 但进程 5 秒内没回收：按超时处理，避免无限等一个僵死单元。
        Err(_) => {
            cleanup(&mut child, unit.as_deref()).await;
            return Err(HostRunError::Timeout {
                secs: timeout_secs,
                unit: unit.unwrap_or_default(),
            });
        }
    };
    Ok(HostOutput {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr,
    })
}

/// 执行并等完，丢掉落过的行。给「拼好 argv 等结果」的调用方用。
#[cfg(target_os = "linux")]
pub async fn run(spec: HostSpec<'_>) -> Result<HostOutput, HostRunError> {
    run_stream(spec, None, |_, _| {}).await
}

/// 起进程；返回子进程与单元名（`direct` 模式没有单元，用于决定清理方式）。
#[cfg(target_os = "linux")]
async fn spawn(
    spec: &HostSpec<'_>,
) -> Result<(tokio::process::Child, Option<String>), HostRunError> {
    use std::sync::atomic::{AtomicU64, Ordering};

    // 同一进程内唯一：同名调用（并发安装/批量封禁）不会撞单元名。
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let (mut command, unit) = if spec.privileged {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let unit = format!("{}-{}-{n}", spec.prefix, std::process::id());
        let description = if spec.description.is_empty() {
            spec.cmd
        } else {
            spec.description
        };
        let argv = systemd_run_argv(&unit, description, spec.cmd, spec.args);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let mut c = tokio::process::Command::new("/usr/bin/systemd-run");
        // 装包时若弹出交互提示，作业会一直挂着等 stdin。
        c.args(&refs).env("DEBIAN_FRONTEND", "noninteractive");
        (c, Some(unit))
    } else {
        let mut c = tokio::process::Command::new(spec.cmd);
        c.args(spec.args);
        (c, None)
    };

    command
        .kill_on_drop(true)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| HostRunError::Spawn(e.to_string()))?;
    if let Some(input) = spec.stdin {
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(input.as_bytes())
                .await
                .map_err(|e| HostRunError::Io(format!("写入 stdin 失败：{e}")))?;
            // 必须显式关闭写端，`tee` 等命令要读到 EOF 才会退出。
            drop(pipe);
        }
    }
    Ok((child, unit))
}

/// 终止：特权模式必须显式 stop 单元（`kill_on_drop` 只杀本地客户端，
/// 真实子进程在单元里继续跑）；`direct` 模式直接 kill 子进程。
#[cfg(target_os = "linux")]
async fn cleanup(child: &mut tokio::process::Child, unit: Option<&str>) {
    if let Some(unit) = unit {
        stop_unit(unit).await;
    }
    let _ = child.start_kill();
}

/// 单路累积上限：够「拼好 argv 等结果」的调用方拿到完整文本，又不让一次执行
/// 把内存吃满。逐行消费方（作业日志）另有自己的保留策略。
#[cfg(target_os = "linux")]
const ACCUM_CAP: usize = 1024 * 1024;

/// 逐行读取一路管道：交给通道，同时累积头部文本。
///
/// 用 `read_until` 而不是 `lines()`：后者遇到非 UTF-8 会直接报错中断，
/// 而命令输出的中文/日志片段里非 UTF-8 字节并不罕见。
#[cfg(target_os = "linux")]
async fn pump<R>(
    reader: R,
    stream: &'static str,
    tx: mpsc::UnboundedSender<(&'static str, String)>,
) -> String
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut buf: Vec<u8> = Vec::new();
    let mut acc = String::new();
    let mut capped = false;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf)
                    .trim_end_matches(['\n', '\r'])
                    .to_string();
                if !capped {
                    if acc.len() + line.len() + 1 > ACCUM_CAP {
                        capped = true;
                    } else {
                        acc.push_str(&line);
                        acc.push('\n');
                    }
                }
                // 空行进作业日志只是噪音；解析 stdout/stderr 的调用方也不需要它。
                if !line.trim().is_empty() && tx.send((stream, line)).is_err() {
                    break;
                }
            }
        }
    }
    acc
}

/// 超时/取消后清理瞬态单元。
///
/// 清理自身也有 5 秒上限：stop 失败、systemctl 不存在或 systemd 无响应都只记
/// 日志，不改变「超时/取消」这个主结论，也不会让调用方多等。
#[cfg(target_os = "linux")]
async fn stop_unit(unit: &str) {
    let stop = tokio::process::Command::new("/usr/bin/systemctl")
        .args(["stop", unit])
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(std::time::Duration::from_secs(5), stop).await {
        Ok(Ok(status)) if status.success() => {
            tracing::debug!(unit, "已终止瞬态单元");
        }
        Ok(Ok(status)) => {
            tracing::warn!(unit, code = ?status.code(), "终止单元失败");
        }
        Ok(Err(e)) => {
            tracing::warn!(unit, error = %e, "执行 systemctl stop 失败");
        }
        Err(_) => {
            tracing::warn!(unit, "systemctl stop 超过 5 秒未返回");
        }
    }
}

/// 非 Linux（开发机）：通道不可用，立即失败。
///
/// 不能静默返回成功：调用方（作业执行体）会据此写「完成」，等于伪造结果。
#[cfg(not(target_os = "linux"))]
pub async fn run(spec: HostSpec<'_>) -> Result<HostOutput, HostRunError> {
    Err(HostRunError::Spawn(format!(
        "宿主命令执行仅在 Linux 可用：{}",
        spec.cmd
    )))
}

/// 非 Linux（开发机）：通道不可用，立即失败。
#[cfg(not(target_os = "linux"))]
pub async fn run_stream<F>(
    spec: HostSpec<'_>,
    _cancel: Option<&(dyn Fn() -> bool + Send + Sync)>,
    _on_line: F,
) -> Result<HostOutput, HostRunError>
where
    F: FnMut(&str, &str) + Send,
{
    Err(HostRunError::Spawn(format!(
        "宿主命令执行仅在 Linux 可用：{}",
        spec.cmd
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_puts_command_after_separator_and_passes_args_verbatim() {
        let args = vec!["--flag".to_string(), "a b".to_string()];
        let argv = systemd_run_argv("skiff-ftp-1-2", "desc", "/usr/bin/true", &args);
        let sep = argv.iter().position(|a| a == "--").expect("必须有 -- 分隔");
        assert_eq!(argv[sep + 1], "/usr/bin/true");
        assert_eq!(&argv[sep + 2..], ["--flag", "a b"]);
        assert!(argv.iter().any(|a| a == "--wait"));
        assert!(argv.iter().any(|a| a == "--collect"));
    }

    #[test]
    fn unit_and_description_are_forwarded() {
        let argv = systemd_run_argv("skiff-firewall-9-0", "fw mutation", "cmd", &[]);
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--unit" && w[1] == "skiff-firewall-9-0"));
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--description" && w[1] == "fw mutation"));
    }

    /// `direct` 只翻 `privileged`，其余默认值（超时、前缀）与特权模式一致：
    /// 两种模式的区别只在「怎么起进程」，不在「怎么等结果」。
    #[test]
    fn direct_only_flips_privileged() {
        let args = vec!["x".to_string()];
        let a = HostSpec::privileged("/bin/x", &args);
        let b = HostSpec::direct("/bin/x", &args);
        assert!(a.privileged && !b.privileged);
        assert_eq!(a.timeout_secs, b.timeout_secs);
        assert_eq!(a.prefix, b.prefix);
        assert_eq!(a.args, b.args);
    }

    #[test]
    fn builder_overrides_only_what_is_set() {
        let args: Vec<String> = Vec::new();
        let spec = HostSpec::direct("/usr/bin/ip", &args)
            .prefix("skiff-probe")
            .timeout(8)
            .stdin(Some("secret"));
        assert_eq!(spec.prefix, "skiff-probe");
        assert_eq!(spec.timeout_secs, 8);
        assert_eq!(spec.stdin, Some("secret"));
        assert_eq!(spec.cmd, "/usr/bin/ip");
        assert!(!spec.privileged);
    }

    #[test]
    fn timeout_message_names_the_unit_but_cancel_does_not() {
        let t = HostRunError::Timeout {
            secs: 30,
            unit: "skiff-exec-1-2".into(),
        };
        assert!(t.message("/usr/bin/apt-get").contains("30s"));
        assert!(t.message("/usr/bin/apt-get").contains("skiff-exec-1-2"));
        assert!(HostRunError::Cancelled
            .message("/usr/bin/apt-get")
            .contains("取消"));
    }

    /// `direct` 模式没有单元名：文案不能说「终止单元 xxx」。
    #[test]
    fn timeout_message_without_unit_stays_readable() {
        let t = HostRunError::Timeout {
            secs: 8,
            unit: String::new(),
        };
        let msg = t.message("/usr/bin/ip");
        assert!(msg.contains("8s"));
        assert!(!msg.contains("单元"));
    }
}
