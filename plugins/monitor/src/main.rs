//! `com.skiff.monitor`：主机监控（只读 `/proc`，零内核改动）。
//!
//! 采样不执行任何外部命令，也不走 Docker；`fs` 能力读 `/proc/stat|meminfo|loadavg|
//! uptime|net/dev|cpuinfo`，需要安装时授权 `path:/proc`。
//!
//! 采样点由内核 cron 每 5 分钟调一次 `tick` 追加；`snapshot` 供 UI 现取现算（不影响
//! 历史缓冲）。CPU 使用率是**两次采样差值**，进程刚拉起时第一次没有基线，返回 `null`。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const RING_FILE: &str = "samples.json";
const PREV_FILE: &str = "prev.json";
const RING_MAX: usize = 240;

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    if let Err(e) = plugin.serve(move |method, params| match method {
        "snapshot" => sample(&p, false, &params),
        "tick" => sample(&p, true, &params),
        "history" => Ok(history(&p)),
        "config" => Ok(config(&p)),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

fn config(plugin: &Plugin) -> Value {
    Value::Object(
        common::load_config(plugin)
            .into_iter()
            .map(|(k, v)| (k, Value::from(v)))
            .collect(),
    )
}

fn history(plugin: &Plugin) -> Value {
    let st = common::read_state(plugin, RING_FILE);
    json!({ "samples": st.get("samples").cloned().unwrap_or(json!([])) })
}

/// 采样：读 `/proc` 并算差值；`store = true` 时把结果追加进环形缓冲。
fn sample(plugin: &Plugin, store: bool, _params: &Value) -> Result<Value, RpcError> {
    let now = common::now_secs();
    let read = |name: &str| -> Result<String, RpcError> {
        plugin
            .fs_host_read_text(&format!("/proc/{name}"))
            .map_err(|e| {
                let msg = format!("{e}");
                if msg.contains("403") {
                    RpcError::invalid_params(
                        "缺少 path:/proc 授权：安装插件时需勾选 path:/proc（或由管理员追加）"
                            .to_string(),
                    )
                } else {
                    common::rpc_err(e)
                }
            })
    };

    let stat = read("stat")?;
    let meminfo = read("meminfo")?;
    let loadavg = read("loadavg")?;
    let uptime = read("uptime")?;
    let netdev = read("net/dev")?;
    let cpuinfo = read("cpuinfo")?;

    let Some((total, idle)) = parse_stat(&stat) else {
        return Err(RpcError::internal("/proc/stat 第一行无法解析".to_string()));
    };
    let prev = common::read_state(plugin, PREV_FILE);
    let prev_total = prev.get("total").and_then(|v| v.as_u64());
    let prev_idle = prev.get("idle").and_then(|v| v.as_u64());
    let prev_net = prev.get("net").and_then(|v| v.as_u64()).unwrap_or(0);
    let prev_net_t = prev.get("net_t").and_then(|v| v.as_u64()).unwrap_or(0);

    let cpu: Option<f64> = match (prev_total, prev_idle) {
        (Some(pt), Some(pi)) if total > pt => {
            let dtotal = (total - pt) as f64;
            let didle = idle.saturating_sub(pi) as f64;
            Some(((dtotal - didle) / dtotal * 100.0 * 10.0).round() / 10.0)
        }
        _ => None,
    };

    let (mem_total_kb, mem_avail_kb) = parse_meminfo(&meminfo);
    let (l1, l5, l15) = parse_loadavg(&loadavg);
    let up = parse_uptime(&uptime);
    let (net_rx, net_tx) = parse_net(netdev.lines());
    let net_rx_rate: Option<f64> = if prev_net_t > 0 && now > prev_net_t {
        Some(net_rx.saturating_sub(prev_net) as f64 / (now - prev_net_t) as f64)
    } else {
        None
    };
    let cpus = count_cpus(&cpuinfo);

    let point = json!({
        "t": now,
        "cpu": cpu,
        "mem_total_kb": mem_total_kb,
        "mem_used_kb": mem_total_kb.saturating_sub(mem_avail_kb),
        "mem_avail_kb": mem_avail_kb,
        "load1": l1, "load5": l5, "load15": l15,
        "net_rx": net_rx, "net_tx": net_tx, "net_rx_rate": net_rx_rate,
        "uptime": up,
        "cpus": cpus,
    });

    // 基线始终更新；历史只在 `store`（cron tick）时追加，避免每次开 UI 都写盘。
    common::write_state(
        plugin,
        PREV_FILE,
        &json!({ "total": total, "idle": idle, "net": net_rx, "net_t": now }),
    )
    .map_err(common::rpc_err)?;

    if store {
        let mut ring = common::read_state(plugin, RING_FILE);
        if !ring.is_object() {
            ring = json!({});
        }
        let obj = ring.as_object_mut().expect("上面刚保证是对象");
        let arr = obj.entry("samples").or_insert_with(|| json!([]));
        if let Some(arr) = arr.as_array_mut() {
            arr.push(point.clone());
            if arr.len() > RING_MAX {
                let drop = arr.len() - RING_MAX;
                arr.drain(0..drop);
            }
            common::write_state(plugin, RING_FILE, &ring).map_err(common::rpc_err)?;
        }
    }

    Ok(json!({ "ok": true, "sample": point, "ring_max": RING_MAX }))
}

fn parse_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse::<u64>().ok())
        .collect();
    if nums.len() < 4 {
        return None;
    }
    let total: u64 = nums.iter().sum();
    // idle + iowait 视为「没干活」的时间（与 uptime/htop 口径一致）
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    Some((total, idle))
}

fn parse_meminfo(text: &str) -> (u64, u64) {
    let get = |key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let total = get("MemTotal:");
    // 老内核没有 MemAvailable：退化为 Free+Buffers+Cached
    let mut avail = get("MemAvailable:");
    if avail == 0 {
        avail = get("MemFree:") + get("Buffers:") + get("Cached:");
    }
    (total, avail)
}

fn parse_loadavg(text: &str) -> (f64, f64, f64) {
    let mut it = text.split_whitespace();
    let n = |v: Option<&str>| -> f64 { v.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) };
    (n(it.next()), n(it.next()), n(it.next()))
}

fn parse_uptime(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or(0)
}

/// 汇总 `/proc/net/dev` 的 rx/tx 字节数（跳过 `lo`：回环流量不代表真实网卡负载）。
fn parse_net<'a>(lines: impl Iterator<Item = &'a str>) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    for line in lines {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface.is_empty() || iface == "lo" {
            continue;
        }
        let nums: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|v| v.parse::<u64>().ok())
            .collect();
        if nums.is_empty() {
            continue;
        }
        rx += nums[0];
        if nums.len() > 8 {
            tx += nums[8];
        }
    }
    (rx, tx)
}

fn count_cpus(cpuinfo: &str) -> u64 {
    cpuinfo
        .lines()
        .filter(|l| l.trim_start().starts_with("processor"))
        .count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpu_stat_line() {
        let text = "cpu  100 20 30 400 50 0 0 0\ncpu0 1 2 3 4\n";
        let (total, idle) = parse_stat(text).unwrap();
        assert_eq!(total, 600);
        assert_eq!(idle, 450);
        assert!(parse_stat("cpu0 1 2 3 4\n").is_none());
        assert!(parse_stat("cpu 1 2 3\n").is_none());
    }

    #[test]
    fn meminfo_prefers_available() {
        let text = "MemTotal:       1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\nBuffers: 10 kB\nCached: 20 kB\n";
        assert_eq!(parse_meminfo(text), (1000, 400));
        // 没有 MemAvailable 时退化
        let old = "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 10 kB\nCached: 20 kB\n";
        assert_eq!(parse_meminfo(old), (1000, 130));
    }

    #[test]
    fn loadavg_and_uptime() {
        assert_eq!(
            parse_loadavg("0.12 0.34 0.56 1/100 12345\n"),
            (0.12, 0.34, 0.56)
        );
        assert_eq!(parse_uptime("12345.67 999.0\n"), 12345);
    }

    #[test]
    fn net_skips_loopback_and_sums_interfaces() {
        let text = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 5000 10 0 0 0 0 0 0 5000 10 0 0 0 0 0 0
  eth0: 100 2 0 0 0 0 0 0 300 3 0 0 0 0 0 0
  eth1: 200 4 0 0 0 0 0 0 400 5 0 0 0 0 0 0
";
        let (rx, tx) = parse_net(text.lines());
        assert_eq!(rx, 300);
        assert_eq!(tx, 700);
    }

    #[test]
    fn counts_processors() {
        let text = "processor : 0\nmodel name : x\nprocessor : 1\n";
        assert_eq!(count_cpus(text), 2);
    }
}
