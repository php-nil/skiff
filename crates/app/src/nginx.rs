//! 内核「网站 → Nginx」：状态、配置、日志、模块。
//!
//! nginx 仍跑在 `skiff-web-nginx` 容器（host 网络）。主配置由内核生成并挂载到
//! `/usr/local/openresty/nginx/conf/nginx.conf`，宿主位置
//! `<web_root>/nginx/nginx.conf`；修改走「候选文件 → 容器内 `nginx -t` → reload」
//! 三步，失败回滚并保留错误行。日志一律来自实际布局：站点行日志在
//! `<站点目录>/log/`，全局日志在 `<web_root>/log/`。

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use skiff_store::Db;

use crate::sites::{LogKind, SiteError, SiteSettings, NGINX_CONTAINER};
use crate::AppState;

/// 性能参数在 `settings` 表里的键。
pub const NGINX_PARAMS_KEY: &str = "nginx.params_json";
/// 内核最近一次成功 reload 的时间（`settings`）。
pub const NGINX_LAST_RELOAD_KEY: &str = "nginx.last_reload_at";

/// stub_status 监听地址与探测 URL（host 网络，仅宿主/容器本机可访问）。
pub const STUB_ADDR: &str = "127.0.0.1:8081";
pub const STUB_URL: &str = "http://127.0.0.1:8081/nginx_status";
/// 主配置里内核生成标记；有标记才允许内核追加 stub 段。
const MAIN_CONF_MARKER: &str = "# 由 Skiff 内核生成";

/// nginx 性能参数（主配置里由内核渲染，不手改文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NginxParams {
    /// `auto` 或正整数。
    pub worker_processes: String,
    pub worker_connections: u32,
    /// 秒；0 表示关闭 keepalive。
    pub keepalive_timeout: u32,
    /// 例如 `16m`。
    pub client_max_body_size: String,
    pub gzip: bool,
    pub gzip_types: String,
}

impl Default for NginxParams {
    fn default() -> Self {
        Self {
            worker_processes: "auto".into(),
            worker_connections: 1024,
            keepalive_timeout: 65,
            client_max_body_size: "16m".into(),
            gzip: true,
            gzip_types:
                "text/plain text/css application/json application/javascript text/xml application/xml"
                    .into(),
        }
    }
}

impl NginxParams {
    pub fn from_json(raw: &str) -> Self {
        let mut p = Self::default();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            return p;
        };
        if let Some(s) = v.get("worker_processes").and_then(|x| x.as_str()) {
            p.worker_processes = s.to_string();
        }
        if let Some(n) = v.get("worker_connections").and_then(|x| x.as_u64()) {
            p.worker_connections = n as u32;
        }
        if let Some(n) = v.get("keepalive_timeout").and_then(|x| x.as_u64()) {
            p.keepalive_timeout = n as u32;
        }
        if let Some(s) = v.get("client_max_body_size").and_then(|x| x.as_str()) {
            p.client_max_body_size = s.to_string();
        }
        if let Some(b) = v.get("gzip").and_then(|x| x.as_bool()) {
            p.gzip = b;
        }
        if let Some(s) = v.get("gzip_types").and_then(|x| x.as_str()) {
            p.gzip_types = s.to_string();
        }
        p
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "worker_processes": self.worker_processes,
            "worker_connections": self.worker_connections,
            "keepalive_timeout": self.keepalive_timeout,
            "client_max_body_size": self.client_max_body_size,
            "gzip": self.gzip,
            "gzip_types": self.gzip_types,
        })
        .to_string()
    }

    pub fn validate(&self) -> Result<(), SiteError> {
        let wp = self.worker_processes.trim();
        let ok_workers = wp == "auto"
            || (wp
                .parse::<u32>()
                .map(|n| (1..=512).contains(&n))
                .unwrap_or(false));
        if !ok_workers {
            return Err(SiteError::BadRequest(
                "worker_processes 只能是 auto 或 1–512 的整数".into(),
            ));
        }
        if !(64..=65535).contains(&self.worker_connections) {
            return Err(SiteError::BadRequest(
                "worker_connections 需在 64–65535 之间".into(),
            ));
        }
        if self.keepalive_timeout > 3600 {
            return Err(SiteError::BadRequest(
                "keepalive_timeout 不能超过 3600 秒".into(),
            ));
        }
        let body = self.client_max_body_size.trim();
        if body.is_empty() || body.contains(';') || body.contains('\n') {
            return Err(SiteError::BadRequest(
                "client_max_body_size 不能为空且不能包含分号/换行".into(),
            ));
        }
        if self.gzip_types.contains(';') || self.gzip_types.contains('\n') {
            return Err(SiteError::BadRequest("gzip_types 不能包含分号/换行".into()));
        }
        Ok(())
    }
}

/// 内核生成的主配置：固定 stub_status 监听在 `127.0.0.1:8081`，不对外暴露。
///
/// `worker_rlimit_nofile` 显式跟随 `worker_connections`：容器默认软限制 1024，
/// 只调大 `worker_connections` 会让 `nginx -t` 告警
/// `worker_connections exceed open file resource limit`（退出码仍为 0），
/// 属于「保存成功但带告警」的静默路径。下限保留 1024，避免把 fd 限制压得过低。
pub fn render_main_nginx_conf(p: &NginxParams) -> String {
    let rlimit_nofile = p.worker_connections.max(1024);
    format!(
        "# 由 Skiff 内核生成；手工修改请走「网站 → Nginx → 配置」，保存前会执行 nginx -t。\n\
         worker_processes {wp};\n\
         worker_rlimit_nofile {rlimit};\n\
         error_log /www/log/nginx-error.log warn;\n\
         pid /var/run/nginx.pid;\n\
         \n\
         events {{\n    worker_connections {wc};\n}}\n\
         \n\
         http {{\n\
         \x20   include /usr/local/openresty/nginx/conf/mime.types;\n\
         \x20   default_type application/octet-stream;\n\
         \x20   log_format main '$remote_addr - $remote_user [$time_local] \"$request\" '\n\
         \x20                   '$status $body_bytes_sent \"$http_referer\" \"$http_user_agent\"';\n\
         \x20   access_log /www/log/nginx-access.log main;\n\
         \x20   sendfile on;\n\
         \x20   keepalive_timeout {kt}s;\n\
         \x20   client_max_body_size {body};\n\
         \x20   gzip {gzip};\n\
         \x20   gzip_types {gtypes};\n\
         \x20   include /etc/nginx/conf.d/*.conf;\n\
         \x20   server {{\n\
         \x20       listen 127.0.0.1:8081;\n\
         \x20       location = /nginx_status {{\n\
         \x20           stub_status;\n\
         \x20           access_log off;\n\
         \x20           allow 127.0.0.1;\n\
         \x20           deny all;\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n",
        wp = p.worker_processes.trim(),
        rlimit = rlimit_nofile,
        wc = p.worker_connections,
        kt = p.keepalive_timeout,
        body = p.client_max_body_size.trim(),
        gzip = if p.gzip { "on" } else { "off" },
        gtypes = p.gzip_types.trim(),
    )
}

/// 首次安装/重建时确保主配置存在且包含内核 stub_status 段。
///
/// 已存在时不动用户正文，只做两件事：内核生成的文件若丢失 stub 段则补回；
/// 非内核生成的文件不碰（状态页会提示“当前主配置不是内核生成”）。
pub fn ensure_nginx_main_conf(s: &SiteSettings, db: &Db) -> std::io::Result<()> {
    std::fs::create_dir_all(s.global_log_dir())?;
    if let Some(parent) = s.nginx_main_conf().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let params = db
        .get_setting(NGINX_PARAMS_KEY)
        .ok()
        .flatten()
        .map(|raw| NginxParams::from_json(&raw))
        .unwrap_or_default();
    let path = s.nginx_main_conf();
    if path.is_file() {
        let current = std::fs::read_to_string(&path)?;
        if current.contains(MAIN_CONF_MARKER) && !current.contains("stub_status;") {
            std::fs::write(&path, ensure_stub_block(&current))?;
        }
        return Ok(());
    }
    std::fs::write(&path, render_main_nginx_conf(&params))
}

/// 在内核生成的主配置末尾补回 stub_status server 段。
fn ensure_stub_block(conf: &str) -> String {
    let block = "    server {\n\
                 \x20       listen 127.0.0.1:8081;\n\
                 \x20       location = /nginx_status {\n\
                 \x20           stub_status;\n\
                 \x20           access_log off;\n\
                 \x20           allow 127.0.0.1;\n\
                 \x20           deny all;\n\
                 \x20       }\n\
                 \x20   }\n";
    match conf.rfind('}') {
        Some(idx) => format!("{}{}{}", &conf[..idx], block, &conf[idx..]),
        None => format!("{conf}\n{block}"),
    }
}

#[derive(Debug, Clone)]
pub struct NginxStatusView {
    pub available: bool,
    pub reason: String,
    pub container_state: String,
    pub version: String,
    pub config_test: String,
    /// 容器 `docker ps` 的运行时长文案（如 `Up 6 minutes`）；未运行则原样给出状态。
    pub uptime: String,
    /// 内核最近一次成功 reload 的时间；空串表示暂无记录。
    pub last_reload: String,
    /// 探测地址与失败原因（不可得时用于诊断，禁止静默留空）。
    pub probe_url: String,
    pub probe_error: String,
    /// 宿主主配置路径，以及它是否包含内核 stub_status 段。
    pub conf_path: String,
    pub conf_has_stub: bool,
    pub active: Option<u64>,
    pub accepts: Option<u64>,
    pub handled: Option<u64>,
    pub requests: Option<u64>,
    pub reading: Option<u64>,
    pub writing: Option<u64>,
    pub waiting: Option<u64>,
    /// stub_status 不可得时的原因（不显示 0 假装成功）。
    pub stub_reason: String,
}

#[derive(Debug, Clone)]
pub struct NginxSiteConfView {
    pub domain: String,
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct NginxConfigView {
    pub main_path: String,
    pub main_content: String,
    pub include_path: String,
    pub include_content: String,
    pub sites: Vec<NginxSiteConfView>,
    pub params: NginxParams,
}

#[derive(Debug, Clone)]
pub struct StatusCount {
    pub label: String,
    pub count: u64,
}

/// 单次日志读取的字节上限：从文件尾反向读，避免大文件拖垮请求。
pub const LOG_TAIL_BYTES: usize = 2 * 1024 * 1024;

/// 日志页查询条件；全部来自 GET 参数，未点「加载」时不触碰文件。
#[derive(Debug, Clone, Default)]
pub struct NginxLogQuery {
    pub loaded: bool,
    /// `access` | `error`；其余值按 access 处理。
    pub kind: String,
    pub keyword: String,
    /// 错误日志级别（all/emerg/alert/crit/error/warn/notice/info）。
    pub level: String,
    /// 访问日志状态码过滤：空 / `404` / `4xx`。
    pub status: String,
    /// 本次加载的行数（50–500）。
    pub page_lines: usize,
    /// 已加载匹配行数；「加载更多」从这里继续。
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct NginxLogView {
    pub loaded: bool,
    pub kind: String,
    pub keyword: String,
    pub level: String,
    pub status: String,
    pub page_lines: usize,
    pub offset: usize,
    pub next_offset: usize,
    pub rows: Vec<String>,
    /// 展示用日志路径；访问日志含多个文件时给出文件数说明。
    pub path: String,
    pub reason: String,
    /// 本次实际扫描的行数与字节数（仅尾部范围内）。
    pub scanned_lines: u64,
    pub scanned_bytes: u64,
    pub scanned_text: String,
    /// 已检查日志文件的总大小。
    pub file_bytes: u64,
    pub file_text: String,
    /// 是否因 2 MiB 上限截断（更早内容未读取）。
    pub truncated: bool,
    pub has_more: bool,
    /// 仅统计 `rows` 这批已加载行的状态码分布。
    pub status_counts: Vec<StatusCount>,
}

#[derive(Debug, Clone)]
struct TailChunk {
    lines: Vec<String>,
    scanned_bytes: u64,
    file_bytes: u64,
    truncated: bool,
}

/// 字节数 → 人类可读文本（日志页只做展示）。
fn bytes_text(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

#[derive(Debug, Clone)]
pub struct NginxModuleView {
    pub available: bool,
    pub reason: String,
    pub version: String,
    pub configure_args: Vec<String>,
    pub modules: Vec<String>,
}

pub(crate) fn error_line(out: &str) -> String {
    out.lines()
        .find(|l| l.contains("emerg") || l.contains("error") || l.contains("[warn]"))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| out.trim().to_string())
}

/// 收集 `nginx -t` 的告警行。
///
/// `nginx -t` 有告警时退出码仍为 0；只看退出码会把「配置生效但带问题」当成完全成功。
fn test_warnings(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.contains("[warn]"))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 只读文件尾部的 `budget` 字节（最多 2 MiB），按行倒序返回。
///
/// 用 `seek` 从末尾定位，绝不整文件读进内存；截断在行中间时丢掉那段残行。
fn read_tail_chunk(path: &Path, budget: usize) -> Result<TailChunk, String> {
    if !path.is_file() {
        return Err(format!("日志文件不存在：{}", path.display()));
    }
    let mut f = std::fs::File::open(path).map_err(|e| format!("读取日志失败：{e}"))?;
    let file_bytes = f
        .metadata()
        .map_err(|e| format!("读取日志元信息失败：{e}"))?
        .len();
    let want = (budget as u64).min(file_bytes);
    let start = file_bytes - want;
    let mut buf = vec![0u8; want as usize];
    if want > 0 {
        f.seek(SeekFrom::Start(start))
            .map_err(|e| format!("定位日志文件失败：{e}"))?;
        f.read_exact(&mut buf)
            .map_err(|e| format!("读取日志失败：{e}"))?;
    }
    let truncated = start > 0;
    // 截断点正好落在换行之后时，第一段就是完整行；否则要丢掉那段残行。
    let mut drop_first = false;
    if truncated {
        f.seek(SeekFrom::Start(start - 1))
            .map_err(|e| format!("定位日志文件失败：{e}"))?;
        let mut prev = [0u8; 1];
        f.read_exact(&mut prev)
            .map_err(|e| format!("读取日志失败：{e}"))?;
        drop_first = prev[0] != b'\n';
    }
    let mut raw: Vec<&[u8]> = buf.split(|b| *b == b'\n').collect();
    if raw.last().map(|s| s.is_empty()).unwrap_or(false) {
        raw.pop();
    }
    if drop_first && !raw.is_empty() {
        raw.remove(0);
    }
    let lines = raw
        .iter()
        .rev()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    Ok(TailChunk {
        lines,
        scanned_bytes: want,
        file_bytes,
        truncated,
    })
}

fn status_of_line(line: &str) -> Option<u16> {
    line.split(|c: char| c.is_whitespace()).find_map(|tok| {
        let t = tok.trim_matches(|c: char| c == '"' || c == '[' || c == ']');
        if t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse::<u16>().ok().filter(|n| (100..600).contains(n))
        } else {
            None
        }
    })
}

/// 状态码过滤：空值不过滤；支持精确 `404` 与类别 `4xx`。
fn status_matches(line: &str, filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    let Some(code) = status_of_line(line) else {
        return false;
    };
    if let Some(class) = filter.strip_suffix("xx") {
        let Ok(n) = class.parse::<u16>() else {
            return true;
        };
        return code / 100 == n;
    }
    match filter.parse::<u16>() {
        Ok(n) => code == n,
        Err(_) => true,
    }
}

/// 已加载行的状态码分布（只统计传入的这一批，不代表全量）。
fn status_buckets(lines: &[String]) -> Vec<StatusCount> {
    let mut c2 = 0u64;
    let mut c3 = 0u64;
    let mut c4 = 0u64;
    let mut c5 = 0u64;
    for line in lines {
        match status_of_line(line) {
            Some(n) if n < 300 => c2 += 1,
            Some(n) if n < 400 => c3 += 1,
            Some(n) if n < 500 => c4 += 1,
            Some(_) => c5 += 1,
            None => {}
        }
    }
    vec![
        StatusCount {
            label: "2xx".into(),
            count: c2,
        },
        StatusCount {
            label: "3xx".into(),
            count: c3,
        },
        StatusCount {
            label: "4xx".into(),
            count: c4,
        },
        StatusCount {
            label: "5xx".into(),
            count: c5,
        },
    ]
}

/// 解析 stub_status 文本；缺失任一行返回 None，调用方要如实说明不可得。
pub fn parse_stub_status(out: &str) -> Option<(u64, u64, u64, u64, u64, u64, u64)> {
    let mut active = None;
    let mut accepts = None;
    let mut handled = None;
    let mut requests = None;
    let mut reading = None;
    let mut writing = None;
    let mut waiting = None;
    for line in out.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Active connections:") {
            active = rest.trim().parse().ok();
        } else if let Some(rest) = l.strip_prefix("Reading:") {
            let rest = rest.trim_start();
            reading = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|s| s.trim().parse().ok());
            writing = rest
                .split("Writing:")
                .nth(1)
                .and_then(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.trim().parse().ok());
            waiting = rest
                .split("Waiting:")
                .nth(1)
                .and_then(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.trim().parse().ok());
        } else if l.starts_with("server accepts handled requests") {
            // 下一行是三个数字；在循环里提前消费不了，留到行首数字判定。
        } else {
            let nums: Vec<u64> = l
                .split_whitespace()
                .take(3)
                .filter_map(|s| s.parse().ok())
                .collect();
            if nums.len() == 3 && accepts.is_none() {
                accepts = Some(nums[0]);
                handled = Some(nums[1]);
                requests = Some(nums[2]);
            }
        }
    }
    Some((
        active?,
        accepts?,
        handled?,
        requests?,
        reading.unwrap_or(0),
        writing.unwrap_or(0),
        waiting.unwrap_or(0),
    ))
}

/// 宿主侧 TCP 探测：nginx 用 host 网络时，8081 就在宿主回环上，不依赖容器内 curl/wget。
fn probe_stub_host() -> Result<String, String> {
    let addr = STUB_ADDR
        .parse()
        .map_err(|e| format!("探测地址无效（{STUB_ADDR}）：{e}"))?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("连接 {STUB_ADDR} 失败：{e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("设置读超时失败：{e}"))?;
    stream
        .write_all(b"GET /nginx_status HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("发送探测请求失败：{e}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|e| format!("读取探测响应失败：{e}"))?;
    let (head, body) = buf
        .split_once("\r\n\r\n")
        .ok_or_else(|| "探测响应不是完整 HTTP 报文".to_string())?;
    let code = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("?");
    if code != "200" {
        return Err(format!("{STUB_URL} 返回 HTTP {code}"));
    }
    Ok(body.to_string())
}

/// 容器内兜底探测：OpenResty 自带 `resty`，用 Lua socket 发同一请求。
fn probe_stub_resty_args() -> Vec<String> {
    vec![
        "resty".into(),
        "-e".into(),
        "local s=ngx.socket.tcp();s:settimeout(2000);\
         local ok,err=s:connect(\"127.0.0.1\",8081);\
         if not ok then print(\"CONNECT_FAIL: \"..tostring(err)) os.exit(1) end;\
         s:send(\"GET /nginx_status HTTP/1.0\\r\\nHost: 127.0.0.1\\r\\nConnection: close\\r\\n\\r\\n\");\
         local d,e=s:receive(\"*a\");s:close();\
         if not d then print(\"READ_FAIL: \"..tostring(e)) os.exit(1) end;print(d)"
            .into(),
    ]
}

/// 简单逐行差异摘要（失败回滚时给出到底改了哪几行）。
pub(crate) fn diff_summary(original: &str, candidate: &str, limit: usize) -> String {
    if original.is_empty() {
        return "当前主配置不存在（本次为新建）".into();
    }
    let old: Vec<&str> = original.lines().collect();
    let new: Vec<&str> = candidate.lines().collect();
    let mut out: Vec<String> = Vec::new();
    for i in 0..old.len().max(new.len()) {
        let a = old.get(i).copied().unwrap_or("<缺行>");
        let b = new.get(i).copied().unwrap_or("<缺行>");
        if a != b {
            out.push(format!("第 {} 行 - {} + {}", i + 1, a.trim(), b.trim()));
            if out.len() >= limit {
                out.push("…".into());
                break;
            }
        }
    }
    if out.is_empty() {
        "内容与当前主配置相同".into()
    } else {
        out.join("；")
    }
}

fn matches_level(line: &str, level: &str) -> bool {
    if level.is_empty() || level == "all" {
        return true;
    }
    let needle = format!("[{level}]");
    line.to_ascii_lowercase().contains(&needle)
}

fn nginx_level_label(level: &str) -> &'static str {
    match level {
        "emerg" => "emerg",
        "alert" => "alert",
        "crit" => "crit",
        "error" => "error",
        "warn" => "warn",
        "notice" => "notice",
        "info" => "info",
        _ => "all",
    }
}

impl AppState {
    async fn nginx_container(&self) -> Result<(String, String, String), SiteError> {
        if !self.docker().available() {
            return Err(SiteError::Unavailable(format!(
                "Docker 不可用：{}",
                self.docker_unavailable_reason()
            )));
        }
        let rows = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器列表失败：{e}")))?;
        let c = rows
            .iter()
            .find(|c| c.name == NGINX_CONTAINER)
            .ok_or_else(|| SiteError::Unavailable("nginx 运行环境未安装".into()))?;
        Ok((c.id.clone(), c.state.clone(), c.status.clone()))
    }

    /// 校验、替换并重启 nginx；返回 `nginx -t` 的告警行（无告警为空）。
    async fn apply_nginx_main(
        &self,
        s: &SiteSettings,
        content: &str,
    ) -> Result<Vec<String>, SiteError> {
        let candidate = s.nginx_main_conf().with_extension("conf.candidate");
        std::fs::write(&candidate, content)
            .map_err(|e| SiteError::Internal(format!("写入候选配置失败：{e}")))?;
        let container_candidate = s.container_path(&candidate);
        let (code, out) = self
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec![
                    "nginx".into(),
                    "-t".into(),
                    "-c".into(),
                    container_candidate.clone(),
                ],
            )
            .await
            .map_err(|e| SiteError::Unavailable(format!("执行 nginx -t 失败：{e}")))?;
        if code != 0 {
            let original = std::fs::read_to_string(s.nginx_main_conf()).unwrap_or_default();
            let diff = diff_summary(&original, content, 6);
            let _ = std::fs::remove_file(&candidate);
            return Err(SiteError::Conflict(format!(
                "nginx -t 未通过（{}）：{}；已回滚未替换；差异：{}",
                container_candidate,
                error_line(&out),
                diff
            )));
        }
        let original = std::fs::read(s.nginx_main_conf()).ok();
        std::fs::rename(&candidate, s.nginx_main_conf())
            .map_err(|e| SiteError::Internal(format!("替换主配置失败：{e}")))?;
        if let Err(e) = self.restart_nginx().await {
            if let Some(bytes) = original.clone() {
                let _ = std::fs::write(s.nginx_main_conf(), bytes);
                let _ = self.restart_nginx().await;
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚：重载 nginx 失败：{e}"
            )));
        }
        // Docker restart 返回成功不代表 nginx 仍在运行：配置里的
        // `worker_rlimit_nofile` 超出容器硬限制等错误会让 master 启动后立即退出。
        // 短暂启动窗口后核对容器状态，避免「页面提示已保存、实际站点全挂」。
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let env = self.site_env_status().await;
        if env.installed && env.nginx_state != "running" {
            if let Some(bytes) = original.clone() {
                let _ = std::fs::write(s.nginx_main_conf(), bytes);
                let _ = self.restart_nginx().await;
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚：nginx 未能保持运行（当前状态 {}）",
                env.nginx_state
            )));
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let _ = self.db().set_setting(NGINX_LAST_RELOAD_KEY, &stamp);
        Ok(test_warnings(&out))
    }

    pub async fn nginx_status(&self) -> NginxStatusView {
        let s = self.site_settings();
        let main = s.nginx_main_conf();
        let conf_has_stub = main
            .is_file()
            .then(|| std::fs::read_to_string(&main).ok())
            .flatten()
            .map(|t| t.contains("stub_status;"))
            .unwrap_or(false);
        let last_reload = self
            .db()
            .get_setting(NGINX_LAST_RELOAD_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut v = NginxStatusView {
            available: false,
            reason: String::new(),
            container_state: "missing".into(),
            version: String::new(),
            config_test: String::new(),
            uptime: String::new(),
            last_reload,
            probe_url: STUB_URL.into(),
            probe_error: String::new(),
            conf_path: main.display().to_string(),
            conf_has_stub,
            active: None,
            accepts: None,
            handled: None,
            requests: None,
            reading: None,
            writing: None,
            waiting: None,
            stub_reason: String::new(),
        };
        match self.nginx_container().await {
            Ok((_id, state, status)) => {
                v.available = true;
                v.container_state = state;
                v.uptime = status;
            }
            Err(e) => {
                v.reason = e.to_string();
                return v;
            }
        }
        if let Ok((_, out)) = self
            .docker()
            .exec_capture(NGINX_CONTAINER, vec!["nginx".into(), "-v".into()])
            .await
        {
            v.version = out
                .trim()
                .trim_start_matches("nginx version:")
                .trim()
                .to_string();
        }
        if let Ok((code, out)) = self
            .docker()
            .exec_capture(NGINX_CONTAINER, vec!["nginx".into(), "-t".into()])
            .await
        {
            v.config_test = if code == 0 {
                let warnings = test_warnings(&out);
                if warnings.is_empty() {
                    "配置校验通过".into()
                } else {
                    format!(
                        "配置校验通过（{} 条告警：{}）",
                        warnings.len(),
                        warnings.join("；")
                    )
                }
            } else {
                format!("配置校验失败：{}", error_line(&out))
            };
        } else {
            v.config_test = "无法执行 nginx -t（容器不可执行）".into();
        }

        // 探测顺序：宿主 TCP（host 网络，最可靠）→ 容器内 resty → wget/curl。
        let mut attempts: Vec<String> = Vec::new();
        let mut out: Result<String, ()> = match probe_stub_host() {
            Ok(body) => Ok(body),
            Err(e) => {
                attempts.push(format!("宿主 TCP {STUB_ADDR}：{e}"));
                match self
                    .docker()
                    .exec_capture(NGINX_CONTAINER, probe_stub_resty_args())
                    .await
                {
                    Ok((0, text)) if text.contains("active connections") => Ok(text),
                    Ok((_, text)) => {
                        attempts.push(format!("容器内 resty：{}", error_line(&text)));
                        Err(())
                    }
                    Err(e) => {
                        attempts.push(format!("容器内 resty 不可用：{e}"));
                        Err(())
                    }
                }
            }
        };
        if out.is_err() {
            let mut fallback: Option<String> = None;
            for tool in [
                vec!["wget", "-qO-", "--timeout=3", STUB_URL],
                vec!["curl", "-fsS", "--max-time", "3", STUB_URL],
            ] {
                match self
                    .docker()
                    .exec_capture(
                        NGINX_CONTAINER,
                        tool.iter().map(|s| s.to_string()).collect(),
                    )
                    .await
                {
                    Ok((0, text)) if text.contains("active connections") => {
                        fallback = Some(text);
                        break;
                    }
                    Ok((_, text)) => attempts.push(format!("{}：{}", tool[0], error_line(&text))),
                    Err(e) => attempts.push(format!("{} 不可用：{e}", tool[0])),
                }
            }
            if let Some(text) = fallback {
                out = Ok(text);
            }
        }
        match out {
            Ok(text) => match parse_stub_status(&text) {
                Some((a, ac, h, r, rd, wr, wt)) => {
                    v.active = Some(a);
                    v.accepts = Some(ac);
                    v.handled = Some(h);
                    v.requests = Some(r);
                    v.reading = Some(rd);
                    v.writing = Some(wr);
                    v.waiting = Some(wt);
                }
                None => {
                    v.stub_reason =
                        "stub_status 返回内容无法解析（可能未加载 ngx_http_stub_status_module）"
                            .into();
                }
            },
            Err(()) => {
                let mut detail = attempts.join("；");
                if !v.conf_has_stub {
                    detail.push_str(&format!(
                        "；内核主配置 {} 未包含 stub_status 段（容器可能仍在使用镜像默认 nginx.conf）",
                        v.conf_path
                    ));
                }
                v.stub_reason = format!("{STUB_URL} 不可得：{detail}");
                v.probe_error = detail;
            }
        }
        v
    }

    pub async fn nginx_config_view(&self) -> Result<NginxConfigView, SiteError> {
        let s = self.site_settings();
        ensure_nginx_main_conf(&s, self.db())
            .map_err(|e| SiteError::Internal(format!("写入 nginx 主配置失败：{e}")))?;
        let params = self.nginx_params();
        let main_content = std::fs::read_to_string(s.nginx_main_conf()).unwrap_or_default();
        let include_path = s.conf_dir.join("10-skiff-sites.conf");
        let include_content = std::fs::read_to_string(&include_path).unwrap_or_default();
        let mut sites = Vec::new();
        let list = self
            .db()
            .list_sites()
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        for site in list {
            let dir = std::path::PathBuf::from(&site.root_dir);
            let conf = s.vhost_file(&dir, &site.domain);
            let disabled = s.vhost_disabled(&dir, &site.domain);
            let (path, content) = if conf.is_file() {
                (
                    conf.clone(),
                    std::fs::read_to_string(&conf).unwrap_or_default(),
                )
            } else if disabled.is_file() {
                (
                    disabled.clone(),
                    std::fs::read_to_string(&disabled).unwrap_or_default(),
                )
            } else {
                (conf.clone(), String::new())
            };
            sites.push(NginxSiteConfView {
                domain: site.domain,
                path: path.display().to_string(),
                content,
            });
        }
        Ok(NginxConfigView {
            main_path: s.nginx_main_conf().display().to_string(),
            main_content,
            include_path: include_path.display().to_string(),
            include_content,
            sites,
            params,
        })
    }

    pub fn nginx_params(&self) -> NginxParams {
        self.db()
            .get_setting(NGINX_PARAMS_KEY)
            .ok()
            .flatten()
            .map(|raw| NginxParams::from_json(&raw))
            .unwrap_or_default()
    }

    /// 保存主配置；返回 `nginx -t` 的告警行（无告警为空）。
    pub async fn save_nginx_main(&self, content: &str) -> Result<Vec<String>, SiteError> {
        if content.trim().is_empty() {
            return Err(SiteError::BadRequest("主配置不能为空".into()));
        }
        let s = self.site_settings();
        self.apply_nginx_main(&s, content).await
    }

    /// 保存性能参数：先落库再应用，应用失败时把库回滚到旧值，避免「配置已换、页面仍显示旧参数」。
    ///
    /// 返回 `nginx -t` 的告警行（无告警为空）。
    pub async fn save_nginx_params(&self, params: &NginxParams) -> Result<Vec<String>, SiteError> {
        params.validate()?;
        let s = self.site_settings();
        let previous = self.db().get_setting(NGINX_PARAMS_KEY).ok().flatten();
        self.db()
            .set_setting(NGINX_PARAMS_KEY, &params.to_json())
            .map_err(|e| SiteError::Internal(format!("保存 nginx 参数失败：{e}")))?;
        match self
            .apply_nginx_main(&s, &render_main_nginx_conf(params))
            .await
        {
            Ok(warnings) => Ok(warnings),
            Err(e) => {
                let rollback = previous.unwrap_or_else(|| NginxParams::default().to_json());
                let _ = self.db().set_setting(NGINX_PARAMS_KEY, &rollback);
                Err(e)
            }
        }
    }

    /// 按条件读取日志尾部。
    ///
    /// `loaded == false` 时只返回空视图，**不触碰任何文件**；加载后按类型、
    /// 关键字、级别/状态码过滤，单次最多从尾部反向读 [`LOG_TAIL_BYTES`]。
    pub fn nginx_logs(&self, q: &NginxLogQuery) -> NginxLogView {
        let s = self.site_settings();
        let kind = if q.kind == "error" { "error" } else { "access" }.to_string();
        let page_lines = q.page_lines.clamp(50, 500);
        let mut v = NginxLogView {
            loaded: q.loaded,
            kind: kind.clone(),
            keyword: q.keyword.clone(),
            level: nginx_level_label(&q.level).to_string(),
            status: q.status.trim().to_string(),
            page_lines,
            offset: q.offset,
            next_offset: q.offset.saturating_add(page_lines),
            rows: Vec::new(),
            path: String::new(),
            reason: String::new(),
            scanned_lines: 0,
            scanned_bytes: 0,
            scanned_text: bytes_text(0),
            file_bytes: 0,
            file_text: bytes_text(0),
            truncated: false,
            has_more: false,
            status_counts: Vec::new(),
        };
        if !q.loaded {
            return v;
        }

        let error_path = s.global_log_dir().join("nginx-error.log");
        let files: Vec<PathBuf> = if kind == "error" {
            vec![error_path.clone()]
        } else {
            let mut paths = Vec::new();
            let global = s.global_log_dir().join("nginx-access.log");
            if global.is_file() {
                paths.push(global);
            }
            let sites = self.db().list_sites().unwrap_or_default();
            for site in &sites {
                let p = s.log_file(&PathBuf::from(&site.root_dir), LogKind::Access);
                if p.is_file() {
                    paths.push(p);
                }
            }
            // 按修改时间从新到旧，优先取最近写入的日志文件。
            paths.sort_by(|a, b| {
                let ma = a.metadata().and_then(|m| m.modified()).ok();
                let mb = b.metadata().and_then(|m| m.modified()).ok();
                mb.cmp(&ma)
            });
            paths
        };
        if kind == "access" && files.is_empty() {
            v.path = format!("{}（未发现访问日志文件）", s.global_log_dir().display());
            v.reason = format!(
                "暂无可读访问日志；已检查全局日志目录 {} 与各站点 log/ 目录",
                s.global_log_dir().display()
            );
            return v;
        }

        let mut errors: Vec<String> = Vec::new();
        let mut scanned: Vec<String> = Vec::new();
        let mut budget = LOG_TAIL_BYTES;
        for path in &files {
            if budget == 0 {
                v.truncated = true;
                break;
            }
            match read_tail_chunk(path, budget) {
                Ok(chunk) => {
                    v.file_bytes = v.file_bytes.saturating_add(chunk.file_bytes);
                    v.scanned_bytes = v.scanned_bytes.saturating_add(chunk.scanned_bytes);
                    v.scanned_lines = v.scanned_lines.saturating_add(chunk.lines.len() as u64);
                    v.truncated |= chunk.truncated;
                    budget = budget.saturating_sub(chunk.scanned_bytes as usize);
                    scanned.extend(chunk.lines);
                }
                Err(e) => errors.push(e),
            }
        }
        v.scanned_text = bytes_text(v.scanned_bytes);
        v.file_text = bytes_text(v.file_bytes);
        if kind == "access" {
            v.path = if files.len() > 1 {
                format!(
                    "{} 个访问日志文件（全局 + 站点，按修改时间从新到旧）",
                    files.len()
                )
            } else {
                files[0].display().to_string()
            };
        } else {
            v.path = error_path.display().to_string();
        }
        if errors.len() == files.len() {
            v.reason = errors.join("；");
            return v;
        }

        let kw = v.keyword.trim().to_ascii_lowercase();
        let level = v.level.clone();
        let status = v.status.clone();
        let mut matching: Vec<String> = Vec::new();
        for line in scanned {
            if !kw.is_empty() && !line.to_ascii_lowercase().contains(&kw) {
                continue;
            }
            if kind == "error" && !matches_level(&line, &level) {
                continue;
            }
            if kind == "access" && !status_matches(&line, &status) {
                continue;
            }
            matching.push(line);
        }
        v.has_more = matching.len() > v.offset.saturating_add(page_lines);
        v.rows = matching
            .into_iter()
            .skip(v.offset)
            .take(page_lines)
            .collect();
        if kind == "access" {
            v.status_counts = status_buckets(&v.rows);
        }
        v
    }

    pub async fn nginx_modules(&self) -> NginxModuleView {
        let mut v = NginxModuleView {
            available: false,
            reason: String::new(),
            version: String::new(),
            configure_args: Vec::new(),
            modules: Vec::new(),
        };
        if let Err(e) = self.nginx_container().await {
            v.reason = e.to_string();
            return v;
        }
        match self
            .docker()
            .exec_capture(NGINX_CONTAINER, vec!["nginx".into(), "-V".into()])
            .await
        {
            Ok((_code, out)) => {
                v.available = true;
                for line in out.lines() {
                    if let Some(rest) = line.trim().strip_prefix("nginx version:") {
                        v.version = rest.trim().to_string();
                    }
                    if let Some((_, args)) = line.split_once("configure arguments:") {
                        v.configure_args = args
                            .split("--")
                            .map(|s| format!("--{}", s.trim()))
                            .filter(|s| s != "--")
                            .collect();
                        v.modules = v
                            .configure_args
                            .iter()
                            .filter(|a| {
                                a.starts_with("--with-http_")
                                    || a.starts_with("--with-stream")
                                    || a.starts_with("--add-module")
                            })
                            .cloned()
                            .collect();
                    }
                }
                if v.configure_args.is_empty() {
                    v.reason = "nginx -V 输出里没有 configure arguments".into();
                }
            }
            Err(e) => v.reason = format!("无法执行 nginx -V：{e}"),
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_conf_has_stub_status_on_loopback_only() {
        let conf = render_main_nginx_conf(&NginxParams::default());
        assert!(conf.contains("listen 127.0.0.1:8081;"), "{conf}");
        assert!(conf.contains("location = /nginx_status"), "{conf}");
        assert!(conf.contains("stub_status;"), "{conf}");
        assert!(conf.contains("access_log off;"), "{conf}");
        assert!(conf.contains("allow 127.0.0.1;"), "{conf}");
        assert!(conf.contains("deny all;"), "{conf}");
        // 不占 9000、不对外监听状态端点。
        assert!(!conf.contains("9000"));
    }

    #[test]
    fn main_conf_sets_worker_rlimit_nofile_above_connections() {
        let big = NginxParams {
            worker_connections: 2048,
            ..Default::default()
        };
        let conf = render_main_nginx_conf(&big);
        assert!(conf.contains("worker_rlimit_nofile 2048;"), "{conf}");
        assert!(conf.contains("worker_connections 2048;"), "{conf}");
        assert_eq!(conf.matches("worker_rlimit_nofile").count(), 1);
        // 下限 1024，避免把 worker_connections 调小时压低 fd 限制。
        let small = NginxParams {
            worker_connections: 64,
            ..Default::default()
        };
        assert!(
            render_main_nginx_conf(&small).contains("worker_rlimit_nofile 1024;"),
            "{}",
            render_main_nginx_conf(&small)
        );
    }

    #[test]
    fn nginx_test_warnings_are_collected() {
        let out = "nginx: the configuration file /www/nginx/nginx.conf syntax is ok\n\
                   nginx: [warn] 2048 worker_connections exceed open file resource limit: 1024\n\
                   nginx: configuration file /www/nginx/nginx.conf test is successful\n";
        let warnings = test_warnings(out);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("worker_connections exceed"));
        assert!(test_warnings("nginx: configuration file ... test is successful").is_empty());
    }

    #[test]
    fn stale_kernel_conf_gets_stub_block_back_once() {
        let stale = "# 由 Skiff 内核生成\nworker_processes auto;\nevents {}\nhttp {\n}\n";
        assert!(!stale.contains("stub_status;"));
        let fixed = ensure_stub_block(stale);
        assert!(fixed.contains("listen 127.0.0.1:8081;"), "{fixed}");
        assert!(fixed.contains("stub_status;"), "{fixed}");
        // 只追加一段；再次 ensure 时由调用方 `contains("stub_status;")` 短路。
        assert_eq!(fixed.matches("location = /nginx_status").count(), 1);
    }

    #[test]
    fn diff_summary_points_at_changed_line() {
        let old = "worker_processes auto;\nworker_connections 1024;\n";
        let new = "worker_processes 4;\nworker_connections 1024;\n";
        let diff = diff_summary(old, new, 6);
        assert!(diff.contains("第 1 行"), "{diff}");
        assert!(diff.contains("worker_processes auto"), "{diff}");
        assert!(diff.contains("worker_processes 4"), "{diff}");
        assert_eq!(diff_summary(old, old, 6), "内容与当前主配置相同");
        assert!(diff_summary("", new, 6).contains("当前主配置不存在"));
    }

    #[test]
    fn stub_status_parser_reads_all_seven_numbers() {
        let body = "Active connections: 3 \n\
                    server accepts handled requests\n\
                    \x20 12 12 40 \n\
                    Reading: 0 Writing: 1 Waiting: 2 \n";
        let got = parse_stub_status(body).expect("应解析");
        assert_eq!(got, (3, 12, 12, 40, 0, 1, 2));
    }

    #[test]
    fn log_level_filter_and_normalization() {
        assert!(matches_level("[error] 123#0: upstream timed out", "error"));
        assert!(!matches_level("[warn] 123#0: something", "error"));
        assert!(matches_level("[warn] 123#0: something", "all"));
        assert_eq!(nginx_level_label("bogus"), "all");
        assert_eq!(nginx_level_label("crit"), "crit");
    }

    #[test]
    fn tail_reader_reads_bounded_bytes_and_keeps_complete_lines() {
        // 每行 20 字节；200 字节预算恰好 10 行，不应因截断丢整行。
        let dir = std::env::temp_dir().join(format!("skiff-nginx-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("access.log");
        let content: String = (0..3000)
            .map(|i| format!("line {i:04} 200 0.001\n"))
            .collect();
        std::fs::write(&path, &content).unwrap();

        let chunk = read_tail_chunk(&path, 200).expect("读取尾部");
        assert_eq!(chunk.scanned_bytes, 200);
        assert_eq!(chunk.file_bytes, content.len() as u64);
        assert!(chunk.truncated, "只读尾部必须标记截断");
        assert_eq!(chunk.lines.len(), 10);
        assert_eq!(
            chunk.lines.first().map(String::as_str),
            Some("line 2999 200 0.001")
        );
        assert_eq!(
            chunk.lines.last().map(String::as_str),
            Some("line 2990 200 0.001")
        );

        let full = read_tail_chunk(&path, LOG_TAIL_BYTES).expect("读取全文");
        assert!(!full.truncated);
        assert_eq!(full.lines.len(), 3000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_reader_drops_partial_first_line() {
        let dir = std::env::temp_dir().join(format!("skiff-nginx-part-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("error.log");
        std::fs::write(&path, "aaaa 200\nbbbb 404\ncccc 500\n").unwrap();
        // 从倒数第 14 字节开始，切在 "bbbb 404\n" 中间。
        let chunk = read_tail_chunk(&path, 14).expect("读取尾部");
        assert!(chunk.truncated);
        assert_eq!(chunk.lines, vec!["cccc 500".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_filter_supports_exact_and_class() {
        let line = r#"1.2.3.4 - - [01/Jan/2026:00:00:00 +0800] "GET / HTTP/1.1" 404 123 "-""#;
        assert!(status_matches(line, "404"));
        assert!(status_matches(line, "4xx"));
        assert!(!status_matches(line, "200"));
        assert!(!status_matches(line, "5xx"));
        assert!(status_matches(line, ""));
        assert!(status_matches(line, "bogus"));
    }

    #[test]
    fn resty_fallback_uses_openresty_lua_socket() {
        let args = probe_stub_resty_args();
        assert_eq!(args.first().map(String::as_str), Some("resty"));
        assert_eq!(args.get(1).map(String::as_str), Some("-e"));
        let script = &args[2];
        assert!(script.contains("ngx.socket.tcp()"), "{script}");
        assert!(script.contains("nginx_status"), "{script}");
        assert!(script.contains("print(d)"), "{script}");
    }
}
