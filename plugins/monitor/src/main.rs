//! `com.skiff.monitor`：主机监控（只读 `/proc`，零内核改动）。
//!
//! 采样不执行任何外部命令，也不走 Docker；`fs` 能力读 `/proc/stat|meminfo|loadavg|
//! uptime|net/dev|cpuinfo`，需要安装时授权 `path:/proc`。
//!
//! **周期完全交给内核调度器**（manifest `[[task]]`）：内核按 `sample_interval` 调
//! `sample` 取点进缓冲，按 `*/5` 调 `execute` 把批次落盘。插件内没有任何定时器，
//! 也不再有旧 `tick` 入口。`snapshot` 供 UI 现取现算（不写历史缓冲）。
//! CPU 使用率是**两次采样差值**，进程刚拉起时第一次没有基线，返回 `null`。

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const RING_FILE: &str = "samples.jsonl";
const PREV_FILE: &str = "prev.json";
/// UI `snapshot` 的独立基线文件。
///
/// 内核 60s 采样与 UI 刷新共用一份基线时，后一次调用会把前一次的速率窗口截成
/// 任意长度（可能只有 1 秒），历史点与实时卡片的速率口径都会漂移。
const PREV_UI_FILE: &str = "prev_ui.json";
/// 环形缓冲保留点数：60s 采样 ≈ 4 小时（契约 §9 的 `sample_interval=60s`）。
const RING_MAX: usize = 240;
/// 行数超过该值才压缩：追加到 2 倍上限再一次性收敛，摊薄重写成本。
const RING_COMPACT_AT: usize = RING_MAX * 2;
const TASK_ID: &str = "plugin:com.skiff.monitor:sample";

fn main() {
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    eprintln!("[{TASK_ID}] 采样由内核调度器驱动，插件内无定时器");
    if let Err(e) = plugin.serve(move |method, params| match method {
        // 内核采样出票 → 只读点，不写历史。
        "sample" => read_point(&p, PREV_FILE),
        // 内核执行出票 → 批次落盘并回执。
        "execute" => execute(&p, &params),
        // UI 现取现算，不写历史缓冲；基线用独立文件，不打断内核 60s 采样窗口。
        "snapshot" => Ok(json!({
            "ok": true,
            "sample": read_point(&p, PREV_UI_FILE)?,
            "ring_max": RING_MAX,
            "scheduler": { "driver": "kernel", "task_id": TASK_ID },
        })),
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
    json!({ "samples": ring_lines(plugin) })
}

/// 读回环形文件：一行一个采样点（JSON Lines）。
///
/// 文件只追加、定期压缩，行数最多 [`RING_COMPACT_AT`] 行；解析失败的行（例如
/// 崩溃留下的半行）直接跳过，不让一条坏行毁掉整段历史。
fn ring_lines(plugin: &Plugin) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(plugin.data_dir.join(RING_FILE)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
        .collect()
}

/// 内核采样出票：读 `/proc` 并算差值，返回单个采样点。
///
/// 基线写 `state_file`（内核采样与 UI 快照各用一份），速率为字节计量、
/// 除以距上次采样的秒数，单位 **B/s**。
fn read_point(plugin: &Plugin, state_file: &str) -> Result<Value, RpcError> {
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
    let prev = common::read_state(plugin, state_file);
    let prev_total = prev.get("total").and_then(|v| v.as_u64());
    let prev_idle = prev.get("idle").and_then(|v| v.as_u64());
    // 旧版只存入站累计（键名 `net`），兼容读取；旧基线没有 `net_tx` 时出站首轮不报速率。
    let prev_net_rx = prev
        .get("net_rx")
        .or_else(|| prev.get("net"))
        .and_then(|v| v.as_u64());
    let prev_net_tx = prev.get("net_tx").and_then(|v| v.as_u64());
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
    let elapsed = now.saturating_sub(prev_net_t);
    let net_rx_rate = counter_rate(net_rx, prev_net_rx, elapsed);
    let net_tx_rate = counter_rate(net_tx, prev_net_tx, elapsed);
    let cpus = count_cpus(&cpuinfo);

    let point = json!({
        "t": now,
        "cpu": cpu,
        "mem_total_kb": mem_total_kb,
        "mem_used_kb": mem_total_kb.saturating_sub(mem_avail_kb),
        "mem_avail_kb": mem_avail_kb,
        "load1": l1, "load5": l5, "load15": l15,
        "net_rx": net_rx, "net_tx": net_tx,
        "net_rx_rate": net_rx_rate, "net_tx_rate": net_tx_rate,
        "uptime": up,
        "cpus": cpus,
    });

    // 基线始终更新；历史只由内核 `execute` 批次统一落盘，UI 现取不写历史。
    common::write_state(
        plugin,
        state_file,
        &json!({
            "total": total, "idle": idle,
            "net_rx": net_rx, "net_tx": net_tx, "net_t": now,
        }),
    )
    .map_err(common::rpc_err)?;

    Ok(point)
}

/// 内核执行出票：把缓冲批次追加进环形历史并回 `persist` 水位。
///
/// 落盘口径：插件数据目录 `samples.jsonl` 一行一个采样点，只追新增批次，
/// 写放大与本批点数成正比、与历史长度无关；行数超过 2 倍上限时压缩为最近
/// [`RING_MAX`] 行。任一点缺少 `seq`/`data` 或写盘失败都返回错误——内核记失败
/// 且**不产生回执**，缓冲保留，下一轮重试。
fn execute(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let points = params
        .get("points")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if points.is_empty() {
        return Ok(json!({ "detail": "无待落盘点", "output_bytes": 0 }));
    }

    let rows = batch_rows(&points)?;
    let kept = append_rows(plugin, &rows)?;

    let last_seq = rows.last().map(|(seq, _)| *seq).unwrap_or(0);
    Ok(json!({
        "verdict": { "verdict": "persist", "persisted_upto": last_seq },
        "detail": format!("追加 {} 点，环形保留 {} 点", rows.len(), kept),
        "output_bytes": 0,
    }))
}

/// 追加写环形文件；返回写入后的保留点数。
///
/// 只在行数超过 [`RING_COMPACT_AT`] 时整体压缩一次，其余批次都是纯追加。
fn append_rows(plugin: &Plugin, rows: &[(u64, Value)]) -> Result<usize, RpcError> {
    use std::io::Write;
    let io_err = |e: std::io::Error| RpcError::internal(format!("写入 {RING_FILE} 失败: {e}"));
    std::fs::create_dir_all(&plugin.data_dir).map_err(io_err)?;
    let path = plugin.data_dir.join(RING_FILE);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io_err)?;
    for (_, data) in rows {
        let line = serde_json::to_string(data)
            .map_err(|e| RpcError::internal(format!("采样点序列化失败: {e}")))?;
        writeln!(file, "{line}").map_err(io_err)?;
    }
    file.flush().map_err(io_err)?;
    // Windows 上不能删除/改名仍被打开的文件：压缩前先释放句柄。
    drop(file);

    let lines = ring_lines(plugin);
    if lines.len() <= RING_COMPACT_AT {
        return Ok(lines.len());
    }
    let keep = &lines[lines.len() - RING_MAX..];
    let mut text = String::new();
    for v in keep {
        text.push_str(&v.to_string());
        text.push('\n');
    }
    let tmp = plugin.data_dir.join(format!(".{RING_FILE}.tmp"));
    std::fs::write(&tmp, text).map_err(io_err)?;
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&tmp, &path).map_err(io_err)?;
    Ok(RING_MAX)
}

/// 内核批次点必须带 `seq` 与对象型 `data`；任一缺失都算协议错误，
/// 返回错误让内核保留缓冲并重试，不能静默跳过（否则水位会前进、点会丢）。
fn batch_rows(points: &[Value]) -> Result<Vec<(u64, Value)>, RpcError> {
    let mut rows = Vec::with_capacity(points.len());
    for p in points {
        let seq = p
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or_else(|| RpcError::invalid_params("缓冲点缺少 seq".to_string()))?;
        let data = p
            .get("data")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| RpcError::invalid_params("缓冲点缺少 data 对象".to_string()))?;
        rows.push((seq, data));
    }
    Ok(rows)
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

/// 计数器差值 / 采样间隔秒数；缺基线或间隔为 0 时不报速率（避免除零与伪 0）。
fn counter_rate(cur: u64, prev: Option<u64>, elapsed_secs: u64) -> Option<f64> {
    if elapsed_secs == 0 {
        return None;
    }
    prev.map(|p| cur.saturating_sub(p) as f64 / elapsed_secs as f64)
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
    fn net_rate_is_bytes_per_second() {
        // 2048 字节 / 2 秒 = 1024 B/s（不是 bit/s，也不是按 KB 计）。
        assert_eq!(counter_rate(2048, Some(1024), 2), Some(512.0));
        assert_eq!(counter_rate(2048, Some(0), 2), Some(1024.0));
        // 缺基线（首次采样/旧版 prev.json）与同秒采样都不报伪速率。
        assert_eq!(counter_rate(2048, None, 2), None);
        assert_eq!(counter_rate(2048, Some(1024), 0), None);
        // 计数器重置后不回绕成负数。
        assert_eq!(counter_rate(10, Some(100), 2), Some(0.0));
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

    #[test]
    fn batch_rows_keeps_order_and_rejects_short_points() {
        let points = vec![
            json!({ "seq": 7, "at": 1, "data": { "t": 1 } }),
            json!({ "seq": 9, "at": 2, "data": { "t": 2 } }),
        ];
        let rows = batch_rows(&points).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.last().unwrap().0, 9);
        assert_eq!(rows[0].1["t"], json!(1));

        assert!(batch_rows(&[json!({ "data": { "t": 1 } })]).is_err());
        assert!(batch_rows(&[json!({ "seq": 1, "data": 3 })]).is_err());
    }

    fn test_plugin() -> Plugin {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("skiff-monitor-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Plugin {
            id: "com.skiff.monitor".into(),
            api: "1.2".into(),
            socket: dir.join("s.sock"),
            data_dir: dir.clone(),
            cache_dir: dir.join("cache"),
            run_dir: dir.join("run"),
            kernel_socket: dir.join("kernel.sock"),
            granted: vec![],
            token: String::new(),
        }
    }

    #[test]
    fn ring_appends_then_compacts() {
        let p = test_plugin();
        let rows: Vec<(u64, Value)> = (0..500u64).map(|i| (i, json!({ "t": i }))).collect();
        // 超过 2 倍上限才压缩，保留最近 RING_MAX 点。
        assert_eq!(append_rows(&p, &rows).unwrap(), RING_MAX);
        let lines = ring_lines(&p);
        assert_eq!(lines.len(), RING_MAX);
        assert_eq!(lines[0]["t"], json!(260));
        assert_eq!(lines[RING_MAX - 1]["t"], json!(499));

        // 未到压缩阈值时纯追加：行数增加且历史不重写。
        let more: Vec<(u64, Value)> = (500..505u64).map(|i| (i, json!({ "t": i }))).collect();
        assert_eq!(append_rows(&p, &more).unwrap(), RING_MAX + 5);

        // 崩溃留下的半行：解析失败只跳过该行，不影响其它历史。
        let path = p.data_dir.join(RING_FILE);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"t\": truncated\n");
        std::fs::write(&path, text).unwrap();
        assert_eq!(ring_lines(&p).len(), RING_MAX + 5);
        let _ = std::fs::remove_dir_all(&p.data_dir);
    }
}
