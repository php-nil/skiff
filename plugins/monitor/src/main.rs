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
        // 展示用文案（百分比/容量/速率/时间）在插件内派生，schema 只做绑定。
        "snapshot" => {
            let mut sample = read_point(&p, PREV_UI_FILE)?;
            decorate_sample(&mut sample, &p);
            Ok(json!({
                "ok": true,
                "sample": sample,
                "ring_max": RING_MAX,
                "scheduler": { "driver": "kernel", "task_id": TASK_ID },
            }))
        }
        "history" => Ok(history(&p)),
        // 进程与网络：只读 `/proc`，与采样共用 path:/proc 授权；不结束进程、不改网络。
        "processes" => processes(&p, &params),
        "network" => network(&p, &params),
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
    let samples = ring_lines(plugin);
    // 新壳表格按「最新在前、最多 30 条」展示；旧 `samples` 原样保留。
    let rows: Vec<Value> = samples.iter().rev().take(30).map(history_row).collect();
    let rows_empty = rows.is_empty();
    json!({
        "samples": samples,
        "rows": rows,
        "rows_empty": rows_empty,
    })
}

/// 历史表格行：保留原始数值，另给展示文案，避免 schema 里做格式化。
fn history_row(sample: &Value) -> Value {
    let mem_pct = mem_percent(sample);
    let num = |key: &str| sample.get(key).and_then(Value::as_f64);
    json!({
        "t": sample.get("t").cloned().unwrap_or(json!(0)),
        "t_text": sample
            .get("t")
            .and_then(Value::as_u64)
            .map(date_text)
            .unwrap_or_else(|| "-".to_string()),
        "cpu_text": pct_text(num("cpu")),
        "mem_pct_text": pct_text(mem_pct),
        "load1_text": num("load1").map(|v| format!("{v:.2}")).unwrap_or_else(|| "-".to_string()),
        "net_rx_text": rate_text(num("net_rx_rate")),
        "net_tx_text": rate_text(num("net_tx_rate")),
    })
}

/// 采样点的内存占用率（百分比，未做 SQL/显示格式化）。
fn mem_percent(sample: &Value) -> Option<f64> {
    let total = sample.get("mem_total_kb").and_then(Value::as_u64)?;
    if total == 0 {
        return None;
    }
    let used = sample.get("mem_used_kb").and_then(Value::as_u64)?;
    Some(used as f64 / total as f64 * 100.0)
}

/// 给 `snapshot` 的采样点补展示字段；内核采样（`sample`）不走这里，
/// 否则派生文案会被写进历史。
fn decorate_sample(sample: &mut Value, plugin: &Plugin) {
    let cfg = common::load_config(plugin);
    let cpu_warn = common::cfg_u64(&cfg, "alert_cpu", 90) as f64;
    let mem_warn = common::cfg_u64(&cfg, "alert_mem", 90) as f64;
    let cpu = sample.get("cpu").and_then(Value::as_f64);
    let mem_pct = mem_percent(sample);
    let total_kb = sample
        .get("mem_total_kb")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let used_kb = sample
        .get("mem_used_kb")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let load = |key: &str| {
        sample
            .get(key)
            .and_then(Value::as_f64)
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "-".to_string())
    };
    let load_text = format!("{} / {} / {}", load("load1"), load("load5"), load("load15"));
    let rx_text = rate_text(sample.get("net_rx_rate").and_then(Value::as_f64));
    let tx_text = rate_text(sample.get("net_tx_rate").and_then(Value::as_f64));
    let uptime_text = sample
        .get("uptime")
        .and_then(Value::as_u64)
        .map(dur_text)
        .unwrap_or_else(|| "-".to_string());
    let t_text = sample
        .get("t")
        .and_then(Value::as_u64)
        .map(date_text)
        .unwrap_or_else(|| "-".to_string());
    let Some(obj) = sample.as_object_mut() else {
        return;
    };
    obj.insert("cpu_text".into(), json!(pct_text(cpu)));
    obj.insert(
        "cpu_high".into(),
        json!(cpu.map(|v| v >= cpu_warn).unwrap_or(false)),
    );
    obj.insert("mem_text".into(), json!(mem_text(used_kb, total_kb)));
    obj.insert(
        "mem_high".into(),
        json!(mem_pct.map(|v| v >= mem_warn).unwrap_or(false)),
    );
    obj.insert("load_text".into(), json!(load_text));
    obj.insert("net_rx_text".into(), json!(rx_text));
    obj.insert("net_tx_text".into(), json!(tx_text));
    obj.insert("uptime_text".into(), json!(uptime_text));
    obj.insert("t_text".into(), json!(t_text));
}

fn pct_text(v: Option<f64>) -> String {
    v.map(|n| format!("{n:.1}%"))
        .unwrap_or_else(|| "-".to_string())
}

/// 容量：≥1 GiB 用 GiB，否则 MiB；0 视为未知。
fn kb_text(kb: u64) -> String {
    if kb == 0 {
        return "-".into();
    }
    if kb >= 1024 * 1024 {
        format!("{:.2} GiB", kb as f64 / 1048576.0)
    } else {
        format!("{:.0} MiB", kb as f64 / 1024.0)
    }
}

fn mem_text(used_kb: u64, total_kb: u64) -> String {
    if total_kb == 0 {
        return "-".into();
    }
    let pct = used_kb as f64 / total_kb as f64 * 100.0;
    format!("{} / {} ({pct:.1}%)", kb_text(used_kb), kb_text(total_kb))
}

fn rate_text(v: Option<f64>) -> String {
    let Some(n) = v else {
        return "-".into();
    };
    if n < 1024.0 {
        format!("{n:.0} B/s")
    } else if n < 1048576.0 {
        format!("{:.1} KiB/s", n / 1024.0)
    } else {
        format!("{:.2} MiB/s", n / 1048576.0)
    }
}

fn dur_text(secs: u64) -> String {
    if secs == 0 {
        return "-".into();
    }
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d} 天 {h} 时 {m} 分")
    } else {
        format!("{h} 时 {m} 分")
    }
}

/// Unix 秒 → `YYYY-MM-DD HH:MM:SS`（UTC）。
///
/// 插件依赖里没有时间库，这里用 Howard Hinnant 的 civil-from-days 算法，
/// 避免为一行格式化引入新依赖；宿主页面会标注 UTC。
fn date_text(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u64, u64) {
    // 以 0000-03-01 为纪元，400 年一个 era。
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
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

// ─────────────────────────── 进程视图（只读 /proc） ───────────────────────────

/// Linux 用户态 jiffies 频率；`/proc/[pid]/stat` 的 utime/stime/starttime 都是这个单位。
/// 极少数架构不是 100，这里按容器/主流 x86、arm64 的默认值处理，并在文档标注口径。
const USER_HZ: u64 = 100;
/// 单次进程列表最多扫描的 PID 数，避免极端主机把一次刷新拖成几千次 UDS 读取。
const PROC_SCAN_MAX: usize = 512;
/// 进程 CPU% 基线文件（与采样基线分开，避免互相打断窗口）。
const PREV_PROC_FILE: &str = "prev_proc.json";

#[derive(Debug, Clone)]
struct ProcStat {
    pid: u32,
    name: String,
    state: char,
    utime: u64,
    stime: u64,
    starttime: u64,
    rss_pages: u64,
}

/// 解析 `/proc/[pid]/stat`：第二列 comm 可能含空格与括号，必须从最后一个 `)` 之后切。
fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let pid = text.split_whitespace().next()?.parse::<u32>().ok()?;
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close <= open {
        return None;
    }
    let name = text[open + 1..close].to_string();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // rest[0] 是第 3 列 state；utime/stime 是第 14/15 列；starttime 第 22 列；rss 第 24 列。
    let state = rest.first()?.chars().next().unwrap_or('?');
    let num = |idx: usize| -> u64 { rest.get(idx).and_then(|v| v.parse().ok()).unwrap_or(0) };
    Some(ProcStat {
        pid,
        name,
        state,
        utime: num(11),
        stime: num(12),
        starttime: num(19),
        rss_pages: num(21),
    })
}

/// 从 `/proc/[pid]/status` 取 Uid 与 VmRSS（kB）。
fn parse_proc_status(text: &str) -> (u32, u64) {
    let mut uid = 0u32;
    let mut rss_kb = 0u64;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            uid = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kb = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        }
    }
    (uid, rss_kb)
}

/// `/etc/passwd` → uid 到用户名的映射；读不到（未授权）就退化为数字 uid。
fn load_passwd(plugin: &Plugin) -> std::collections::HashMap<u32, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(text) = plugin.fs_host_read_text("/etc/passwd") {
        for line in text.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 {
                if let Ok(uid) = parts[2].parse::<u32>() {
                    map.entry(uid).or_insert_with(|| parts[0].to_string());
                }
            }
        }
    }
    map
}

/// 进程列表：排序、筛选、分页都在插件内完成；CPU% 是**距上次刷新**的差值。
fn processes(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let now = common::now_secs();
    let list = plugin
        .fs_host_list("/proc")
        .map_err(|e| fs_err(e, "读取 /proc 目录"))?;
    let items = list
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut pids: Vec<u32> = items
        .iter()
        .filter(|it| it.get("dir").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|it| it.get("name").and_then(Value::as_str))
        .filter_map(|n| n.parse::<u32>().ok())
        .collect();
    pids.sort_unstable();
    let truncated = pids.len() > PROC_SCAN_MAX;
    pids.truncate(PROC_SCAN_MAX);

    let uptime = plugin
        .fs_host_read_text("/proc/uptime")
        .ok()
        .map(|t| parse_uptime(&t))
        .unwrap_or(0);
    let (mem_total_kb, _) = plugin
        .fs_host_read_text("/proc/meminfo")
        .ok()
        .map(|t| parse_meminfo(&t))
        .unwrap_or((0, 0));
    let stat_total = plugin
        .fs_host_read_text("/proc/stat")
        .ok()
        .and_then(|t| parse_stat(&t))
        .map(|(total, _)| total)
        .unwrap_or(0);
    let users = load_passwd(plugin);

    let prev = common::read_state(plugin, PREV_PROC_FILE);
    let prev_t = prev.get("t").and_then(Value::as_u64).unwrap_or(0);
    let prev_total = prev.get("total").and_then(Value::as_u64).unwrap_or(0);
    let prev_ticks = prev
        .get("ticks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let can_delta = prev_t > 0 && now > prev_t && stat_total > prev_total;
    let dtotal = stat_total.saturating_sub(prev_total) as f64;

    let mut rows = Vec::with_capacity(pids.len());
    let mut ticks = serde_json::Map::new();
    let mut scanned = 0usize;
    for pid in pids {
        let Ok(stat_text) = plugin.fs_host_read_text(&format!("/proc/{pid}/stat")) else {
            continue; // 进程在扫描期间退出
        };
        let Some(st) = parse_proc_stat(&stat_text) else {
            continue;
        };
        let status_text = plugin
            .fs_host_read_text(&format!("/proc/{pid}/status"))
            .unwrap_or_default();
        let (uid, rss_kb) = parse_proc_status(&status_text);
        let cmdline = plugin
            .fs_host_read_text(&format!("/proc/{pid}/cmdline"))
            .unwrap_or_default();
        let command = cmdline.replace('\0', " ").trim().to_string();
        let ticks_now = st.utime.saturating_add(st.stime);
        ticks.insert(pid.to_string(), json!(ticks_now));

        let cpu = if can_delta && dtotal > 0.0 {
            let key = pid.to_string();
            prev_ticks.get(&key).and_then(Value::as_u64).map(|before| {
                let d = ticks_now.saturating_sub(before) as f64;
                (d / dtotal * 100.0 * 10.0).round() / 10.0
            })
        } else {
            None
        };
        let started = if uptime > 0 {
            now.saturating_sub(uptime)
                .saturating_add(st.starttime / USER_HZ)
        } else {
            0
        };
        let mem_kb = if rss_kb > 0 { rss_kb } else { st.rss_pages * 4 };
        let user = users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
        scanned += 1;
        rows.push(json!({
            "pid": st.pid,
            "user": user,
            "uid": uid,
            "state": state_text(st.state),
            "cpu": cpu,
            "mem_kb": mem_kb,
            "mem_pct": if mem_total_kb > 0 {
                ((mem_kb as f64 / mem_total_kb as f64) * 1000.0).round() / 10.0
            } else {
                0.0
            },
            "started": started,
            "elapsed_secs": now.saturating_sub(started),
            "name": st.name,
            "command": if command.is_empty() { format!("[{}]", st.name) } else { command },
        }));
    }

    // 基线始终更新；下次刷新才有 CPU% 差值。
    common::write_state(
        plugin,
        PREV_PROC_FILE,
        &json!({ "t": now, "total": stat_total, "ticks": ticks }),
    )
    .map_err(common::rpc_err)?;

    let q = params
        .get("q")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let user_filter = params
        .get("user")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !q.is_empty() || !user_filter.is_empty() {
        rows.retain(|r| {
            let hit = |v: &Value| {
                v.as_str()
                    .map(|s| s.to_ascii_lowercase().contains(&q))
                    .unwrap_or(false)
            };
            // PID 是 JSON 数字：按数值比较，避免为比较临时造一个 String。
            let q_pid = q.parse::<u64>().ok();
            let q_hit = q.is_empty()
                || hit(&r["command"])
                || hit(&r["name"])
                || (q_pid.is_some() && r["pid"].as_u64() == q_pid);
            let u_hit = user_filter.is_empty()
                || r["user"]
                    .as_str()
                    .map(|s| s.to_ascii_lowercase().contains(&user_filter))
                    .unwrap_or(false);
            q_hit && u_hit
        });
    }

    let sort = params.get("sort").and_then(Value::as_str).unwrap_or("cpu");
    let order = params
        .get("order")
        .and_then(Value::as_str)
        .unwrap_or(if sort == "pid" { "asc" } else { "desc" })
        .to_ascii_lowercase();
    let key = |v: &Value| -> f64 {
        match sort {
            "mem" | "memory" => v["mem_kb"].as_f64().unwrap_or(0.0),
            "pid" => v["pid"].as_f64().unwrap_or(0.0),
            "started" => v["started"].as_f64().unwrap_or(0.0),
            _ => v["cpu"].as_f64().unwrap_or(-1.0),
        }
    };
    rows.sort_by(|a, b| {
        let (x, y) = (key(a), key(b));
        if order == "asc" {
            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    // 展示字段在分页前补齐；旧字段保持原样。
    for row in rows.iter_mut() {
        decorate_process_row(row);
    }

    let total = rows.len();
    let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
    let page = params
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let page_size = if all {
        total as u64
    } else {
        params
            .get("page_size")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 200)
    };
    let start = ((page - 1) as usize * page_size as usize).min(total);
    let end = (start + page_size as usize).min(total);
    let page_items = if all { rows } else { rows[start..end].to_vec() };

    Ok(json!({
        "items": page_items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "scanned": scanned,
        "truncated": truncated,
        "cpu_baseline": can_delta,
        "user_source": if users.is_empty() { "uid" } else { "passwd" },
        "note": if can_delta {
            "CPU% 为距上次刷新的差值；首次刷新为 null"
        } else {
            "首次刷新没有 CPU 基线，CPU% 为 null；再刷新一次即可"
        },
    }))
}

fn state_text(c: char) -> &'static str {
    match c {
        'R' => "running",
        'S' => "sleeping",
        'D' => "disk-sleep",
        'Z' => "zombie",
        'T' => "stopped",
        't' => "tracing",
        'I' => "idle",
        'X' => "dead",
        _ => "unknown",
    }
}

fn state_zh(state: &str) -> &'static str {
    match state {
        "running" => "运行",
        "sleeping" => "休眠",
        "disk-sleep" => "不可中断",
        "zombie" => "僵尸",
        "stopped" => "停止",
        "tracing" => "跟踪",
        "idle" => "空闲",
        "dead" => "已退出",
        _ => "未知",
    }
}

/// 进程表的展示字段：保留 `state` / `cpu` / `mem_kb` / `started` 供排序，
/// 另给可读文案；`mem_mb` 是表格数字列用的数值。
fn decorate_process_row(row: &mut Value) {
    let cpu = row.get("cpu").and_then(Value::as_f64);
    let mem_mb = (row.get("mem_kb").and_then(Value::as_u64).unwrap_or(0) as f64 / 1024.0 * 10.0)
        .round()
        / 10.0;
    let started = row.get("started").and_then(Value::as_u64).unwrap_or(0);
    let state = row
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(obj) = row.as_object_mut() else {
        return;
    };
    obj.insert("cpu_text".into(), json!(pct_text(cpu)));
    obj.insert("mem_mb".into(), json!(mem_mb));
    obj.insert(
        "started_text".into(),
        json!(if started > 0 {
            date_text(started)
        } else {
            "-".to_string()
        }),
    );
    obj.insert("state_text".into(), json!(state_zh(&state)));
}

// ─────────────────────────── 网络视图（只读 /proc/net） ───────────────────────────

/// 单次读取的连接上限，防止高并发主机把响应撑爆。
const NET_ROW_MAX: usize = 5000;

fn network(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let protocol = params
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("all")
        .to_ascii_lowercase();
    let want_tcp = protocol != "udp";
    let want_udp = protocol != "tcp";
    let state_filter = params
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("all")
        .to_ascii_lowercase();

    let mut rows = Vec::new();
    let mut read_error = None;
    for (file, proto, v6) in [
        ("tcp", "tcp", false),
        ("tcp6", "tcp", true),
        ("udp", "udp", false),
        ("udp6", "udp", true),
    ] {
        if (proto == "tcp" && !want_tcp) || (proto == "udp" && !want_udp) {
            continue;
        }
        match plugin.fs_host_read_text(&format!("/proc/net/{file}")) {
            Ok(text) => rows.extend(parse_net_connections(&text, proto, v6)),
            Err(e) => {
                read_error.get_or_insert_with(|| format!("{e}"));
            }
        }
        if rows.len() >= NET_ROW_MAX {
            break;
        }
    }
    rows.truncate(NET_ROW_MAX);

    let q = params
        .get("q")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !q.is_empty() {
        rows.retain(|r| {
            let text_hit = ["local", "remote", "state", "protocol"].iter().any(|k| {
                r.get(*k)
                    .and_then(Value::as_str)
                    .map(|s| s.to_ascii_lowercase().contains(&q))
                    .unwrap_or(false)
            });
            // uid / 端口是 JSON 数字：按数值匹配，q 解析不成就只做文本匹配。
            let q_num = q.parse::<u64>().ok();
            let num_hit = |key: &str| q_num.is_some() && r[key].as_u64() == q_num;
            text_hit || num_hit("uid") || num_hit("local_port") || num_hit("remote_port")
        });
    }
    if state_filter != "all" && !state_filter.is_empty() {
        rows.retain(|r| {
            let st = r.get("state").and_then(Value::as_str).unwrap_or("");
            let listening = r.get("listening").and_then(Value::as_bool).unwrap_or(false);
            match state_filter.as_str() {
                "listen" | "listening" => listening,
                other => st.eq_ignore_ascii_case(other),
            }
        });
    }

    // 监听在前，其次按本地端口；让「谁在对外提供服务」一眼可见。
    rows.sort_by(|a, b| {
        let la = a.get("listening").and_then(Value::as_bool).unwrap_or(false);
        let lb = b.get("listening").and_then(Value::as_bool).unwrap_or(false);
        lb.cmp(&la).then_with(|| {
            let pa = a.get("local_port").and_then(Value::as_u64).unwrap_or(0);
            let pb = b.get("local_port").and_then(Value::as_u64).unwrap_or(0);
            pa.cmp(&pb)
        })
    });

    for row in rows.iter_mut() {
        decorate_net_row(row);
    }

    let total = rows.len();
    let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
    let page = params
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let page_size = if all {
        total as u64
    } else {
        params
            .get("page_size")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 200)
    };
    let start = ((page - 1) as usize * page_size as usize).min(total);
    let end = (start + page_size as usize).min(total);
    let page_items = if all { rows } else { rows[start..end].to_vec() };

    Ok(json!({
        "items": page_items,
        "total": total,
        "page": page,
        "page_size": page_size,
        "read_error": read_error,
        "owner_note": "插件降权运行，/proc/[pid]/fd 只对本进程可读，无法可靠把套接字映射到 PID；这里展示内核 /proc/net/* 给出的 uid",
    }))
}

/// 连接表的展示字段：`protocol_text` 区分 v4/v6，`key` 供表格 rowKey。
fn decorate_net_row(row: &mut Value) {
    let protocol = row
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let v6 = row.get("v6").and_then(Value::as_bool).unwrap_or(false);
    let label = match (protocol.as_str(), v6) {
        ("tcp", true) => "TCP6",
        ("tcp", false) => "TCP",
        ("udp", true) => "UDP6",
        _ => "UDP",
    };
    let local = row.get("local").and_then(Value::as_str).unwrap_or("");
    let remote = row.get("remote").and_then(Value::as_str).unwrap_or("");
    let inode = row.get("inode").and_then(Value::as_u64).unwrap_or(0);
    let key = format!("{label}:{local}>{remote}:{inode}");
    let Some(obj) = row.as_object_mut() else {
        return;
    };
    obj.insert("protocol_text".into(), json!(label));
    obj.insert("key".into(), json!(key));
}

/// 解析 `/proc/net/tcp[6]` / `udp[6]`。地址是小端十六进制，端口也是十六进制。
fn parse_net_connections(text: &str, protocol: &str, v6: bool) -> Vec<Value> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        let Some((local_ip, local_port)) = parse_hex_addr(cols[1], v6) else {
            continue;
        };
        let Some((remote_ip, remote_port)) = parse_hex_addr(cols[2], v6) else {
            continue;
        };
        let state_code = cols[3];
        let uid = cols[7].parse::<u32>().unwrap_or(0);
        let inode = cols[9].parse::<u64>().unwrap_or(0);
        // UDP 的 07 表示「未连接」套接字，沿用 TCP 状态表会误显示为 CLOSE。
        let state = if protocol == "udp" && state_code == "07" {
            "UNCONN"
        } else {
            tcp_state(state_code)
        };
        let listening = protocol == "tcp" && state_code == "0A";
        out.push(json!({
            "protocol": protocol,
            "local": format!("{}:{}", local_ip, local_port),
            "local_port": local_port,
            "remote": format!("{}:{}", remote_ip, remote_port),
            "remote_port": remote_port,
            "state": state,
            "listening": listening,
            "uid": uid,
            "inode": inode,
            "pid": null,
            "process": null,
            "v6": v6,
        }));
    }
    out
}

fn parse_hex_addr(s: &str, v6: bool) -> Option<(String, u16)> {
    let (addr, port) = s.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if v6 { hex_v6(addr)? } else { hex_v4(addr)? };
    Some((ip, port))
}

fn hex_v4(addr: &str) -> Option<String> {
    if addr.len() != 8 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        // /proc/net/tcp 用小端存储：每个 32 位字内字节顺序反转。
        let start = (3 - i) * 2;
        *byte = u8::from_str_radix(&addr[start..start + 2], 16).ok()?;
    }
    Some(format!(
        "{}.{}.{}.{}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    ))
}

/// IPv6 在 `/proc/net/tcp6` 里是 4 个 32 位小端字；逐字数按字节反转后拼成 8 组 16 位。
fn hex_v6(addr: &str) -> Option<String> {
    if addr.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for word in 0..4 {
        let base = word * 8;
        for i in 0..4 {
            let start = base + (3 - i) * 2;
            bytes[word * 4 + i] = u8::from_str_radix(&addr[start..start + 2], 16).ok()?;
        }
    }
    let groups: Vec<String> = (0..8)
        .map(|i| format!("{:x}", (bytes[i * 2] as u16) << 8 | bytes[i * 2 + 1] as u16))
        .collect();
    Some(groups.join(":"))
}

fn tcp_state(code: &str) -> &'static str {
    match code {
        "01" => "ESTABLISHED",
        "02" => "SYN_SENT",
        "03" => "SYN_RECV",
        "04" => "FIN_WAIT1",
        "05" => "FIN_WAIT2",
        "06" => "TIME_WAIT",
        "07" => "CLOSE",
        "08" => "CLOSE_WAIT",
        "09" => "LAST_ACK",
        "0A" => "LISTEN",
        "0B" => "CLOSING",
        "0C" => "NEW_SYN_RECV",
        _ => "UNKNOWN",
    }
}

/// `fs` 能力错误统一翻译；403 时给出授权排查提示。
fn fs_err(e: impl std::fmt::Display, what: &str) -> RpcError {
    let msg = format!("{e}");
    if msg.contains("403") {
        RpcError::invalid_params(format!(
            "缺少 path:/proc 授权，无法{what}；安装插件时需勾选 path:/proc"
        ))
    } else {
        RpcError::internal(format!("{what}失败：{msg}"))
    }
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

    // ── 进程视图 ──

    #[test]
    fn proc_stat_splits_on_last_paren() {
        // comm 可含空格与括号；字段必须从最后一个 `)` 之后按列号取。
        let line =
            "1234 (my app (x)) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23";
        let st = parse_proc_stat(line).unwrap();
        assert_eq!(st.pid, 1234);
        assert_eq!(st.name, "my app (x)");
        assert_eq!(st.state, 'S');
        // 本行 state 之后的值依次为 1..23，故 rest[i] == i（i >= 1）。
        assert_eq!(st.utime, 11); // 第 14 列 = rest[11]
        assert_eq!(st.stime, 12); // 第 15 列 = rest[12]
        assert_eq!(st.starttime, 19); // 第 22 列 = rest[19]
        assert_eq!(st.rss_pages, 21); // 第 24 列 = rest[21]
        assert!(parse_proc_stat("not a stat line").is_none());
    }

    #[test]
    fn proc_status_reads_uid_and_rss() {
        let text = "Name:\tpure-ftpd\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t   4096 kB\n";
        assert_eq!(parse_proc_status(text), (1000, 4096));
        assert_eq!(parse_proc_status("Name:\tx\n"), (0, 0));
    }

    // ── 网络视图 ──

    #[test]
    fn hex_addr_decodes_little_endian() {
        assert_eq!(hex_v4("0100007F").as_deref(), Some("127.0.0.1"));
        assert_eq!(hex_v4("00000000").as_deref(), Some("0.0.0.0"));
        assert!(hex_v4("0100").is_none());
        assert_eq!(
            hex_v6("00000000000000000000000001000000").as_deref(),
            Some("0:0:0:0:0:0:0:1")
        );
    }

    #[test]
    fn net_connections_parse_tcp_table() {
        let text = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345
   1: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 23456
";
        let rows = parse_net_connections(text, "tcp", false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["local"], json!("127.0.0.1:53"));
        assert_eq!(rows[0]["state"], json!("LISTEN"));
        assert_eq!(rows[0]["listening"], json!(true));
        assert_eq!(rows[0]["uid"], json!(0));
        // 非监听连接排不上 listening，状态按 TCP 状态码表翻译。
        assert_eq!(rows[1]["local"], json!("127.0.0.1:8080"));
        assert_eq!(rows[1]["remote"], json!("127.0.0.1:54321"));
        assert_eq!(rows[1]["state"], json!("ESTABLISHED"));
        assert_eq!(rows[1]["listening"], json!(false));
        assert_eq!(rows[1]["uid"], json!(1000));
    }

    #[test]
    fn tcp_state_codes_match_proc_net() {
        assert_eq!(tcp_state("0A"), "LISTEN");
        assert_eq!(tcp_state("01"), "ESTABLISHED");
        assert_eq!(tcp_state("FF"), "UNKNOWN");
    }

    // ── 原生 schema 的展示字段 ──

    #[test]
    fn date_text_is_utc() {
        assert_eq!(date_text(0), "1970-01-01 00:00:00");
        assert_eq!(date_text(1_700_000_000), "2023-11-14 22:13:20");
        // 闰日：2024-02-29 00:00:00 UTC
        assert_eq!(date_text(1_709_164_800), "2024-02-29 00:00:00");
    }

    #[test]
    fn rate_and_duration_texts() {
        assert_eq!(rate_text(Some(512.0)), "512 B/s");
        assert_eq!(rate_text(Some(2048.0)), "2.0 KiB/s");
        assert_eq!(rate_text(Some(3.0 * 1048576.0)), "3.00 MiB/s");
        assert_eq!(rate_text(None), "-");
        assert_eq!(dur_text(0), "-");
        assert_eq!(dur_text(90061), "1 天 1 时 1 分");
    }

    #[test]
    fn memory_texts() {
        assert_eq!(mem_text(0, 0), "-");
        // 512 MiB / 1 GiB
        assert_eq!(mem_text(524_288, 1_048_576), "512 MiB / 1.00 GiB (50.0%)");
        assert_eq!(pct_text(None), "-");
        assert_eq!(pct_text(Some(12.34)), "12.3%");
    }

    #[test]
    fn process_rows_get_display_fields() {
        let mut row = json!({
            "pid": 1,
            "user": "root",
            "state": "sleeping",
            "cpu": null,
            "mem_kb": 2048,
            "started": 1_700_000_000u64,
        });
        decorate_process_row(&mut row);
        assert_eq!(row["cpu_text"], json!("-"));
        assert_eq!(row["mem_mb"], json!(2.0));
        assert_eq!(row["started_text"], json!("2023-11-14 22:13:20"));
        assert_eq!(row["state_text"], json!("休眠"));
        // 原始字段不丢，旧 UI 仍可直接使用。
        assert_eq!(row["mem_kb"], json!(2048));
    }

    #[test]
    fn net_rows_get_protocol_and_key() {
        let mut row = json!({
            "protocol": "tcp", "v6": true,
            "local": "0.0.0.0:80", "remote": "0.0.0.0:0", "inode": 7,
        });
        decorate_net_row(&mut row);
        assert_eq!(row["protocol_text"], json!("TCP6"));
        assert_eq!(row["key"], json!("TCP6:0.0.0.0:80>0.0.0.0:0:7"));
    }
}
