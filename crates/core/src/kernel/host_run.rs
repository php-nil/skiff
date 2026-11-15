//! 特权命令执行通道：`systemd-run` 瞬态单元。
//!
//! `ftp` 与 `firewall` 两个能力面都需要「不继承主服务沙箱、以 root 执行固定
//! 二进制 + 参数数组」。这里收口 argv 拼装、stdin 投递与超时，避免同一段安全
//! 相关代码在两处各自演化。命令不经 shell，敏感内容只走 stdin。

#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "linux")]
use tokio::io::AsyncWriteExt;

/// 与能力面各自 `CommandOutput` 同形；由调用方转成 `skiff_plugin::*::CommandOutput`。
pub(super) struct HostOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// `systemd-run` 的完整 argv（不含可执行文件本身）。
///
/// 纯函数：单测在非 Linux 上也能验证「`--` 之后才是目标命令」与参数原样透传。
pub(super) fn systemd_run_argv(
    unit: &str,
    description: &str,
    cmd: &str,
    args: &[String],
) -> Vec<String> {
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

/// 用 `systemd-run` 起一个不继承主服务沙箱的瞬态单元执行并等待结果。
///
/// `prefix` 只用于单元名可读性（`skiff-ftp-*` / `skiff-firewall-*`）；`SEQ` 在
/// 本模块共享，保证同名调用不会撞单元。
#[cfg(target_os = "linux")]
pub(super) async fn run_host(
    prefix: &str,
    description: &str,
    cmd: &str,
    args: &[String],
    stdin: Option<&str>,
    timeout_secs: u64,
) -> Result<HostOutput, String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let unit = format!("{prefix}-{}-{n}", std::process::id());
    let full = systemd_run_argv(&unit, description, cmd, args);
    let argv: Vec<&str> = full.iter().map(String::as_str).collect();

    let mut command = tokio::process::Command::new("/usr/bin/systemd-run");
    command
        .args(&argv)
        .env("DEBIAN_FRONTEND", "noninteractive")
        .kill_on_drop(true)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("执行 {cmd} 失败：{e}"))?;
    if let Some(input) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(input.as_bytes())
                .await
                .map_err(|e| format!("写入 {cmd} stdin 失败：{e}"))?;
            // 必须显式关闭写端，`tee` 等命令要读到 EOF 才会退出。
            drop(pipe);
        }
    }
    let waited = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.max(1)),
        child.wait_with_output(),
    )
    .await;
    let out = match waited {
        Ok(waited) => waited.map_err(|e| format!("等待 {cmd} 失败：{e}"))?,
        Err(_) => {
            // `kill_on_drop` 只能终止本地 `systemd-run` 客户端，瞬态单元仍在执行；
            // 必须显式 stop，否则调用方按「失败」重试时真实副作用还在继续。
            stop_unit(&unit).await;
            return Err(format!(
                "{cmd} 执行超时（{timeout_secs}s），已尝试终止单元 {unit}"
            ));
        }
    };
    Ok(HostOutput {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// 超时后清理瞬态单元。
///
/// 清理自身也有 5 秒上限：stop 失败、systemctl 不存在或 systemd 无响应都只记
/// 日志，不改变「超时」这个主结论，也不会让调用方多等。
#[cfg(target_os = "linux")]
async fn stop_unit(unit: &str) {
    let stop = tokio::process::Command::new("/usr/bin/systemctl")
        .args(["stop", unit])
        .kill_on_drop(true)
        .status();
    match tokio::time::timeout(std::time::Duration::from_secs(5), stop).await {
        Ok(Ok(status)) if status.success() => {
            tracing::debug!(unit, "已终止超时的瞬态单元");
        }
        Ok(Ok(status)) => {
            tracing::warn!(unit, code = ?status.code(), "终止超时单元失败");
        }
        Ok(Err(e)) => {
            tracing::warn!(unit, error = %e, "执行 systemctl stop 失败");
        }
        Err(_) => {
            tracing::warn!(unit, "systemctl stop 超过 5 秒未返回");
        }
    }
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
}
