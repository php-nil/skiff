//! 内核「网站 → Nginx」：状态、配置、日志、模块。
//!
//! nginx 仍跑在 `skiff-web-nginx` 容器（host 网络）。主配置由内核生成并挂载到
//! `/usr/local/openresty/nginx/conf/nginx.conf`，宿主位置
//! `<web_root>/nginx/nginx.conf`；修改走「候选文件 → 容器内 `nginx -t` → reload」
//! 三步，失败回滚并保留错误行。日志一律来自实际布局：站点行日志在
//! `<站点目录>/log/`，全局日志在 `<web_root>/log/`。

use std::path::Path;

use skiff_store::Db;

use crate::sites::{LogKind, SiteError, SiteSettings, NGINX_CONTAINER};
use crate::AppState;

/// 性能参数在 `settings` 表里的键。
pub const NGINX_PARAMS_KEY: &str = "nginx.params_json";

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
pub fn render_main_nginx_conf(p: &NginxParams) -> String {
    format!(
        "# 由 Skiff 内核生成；手工修改请走「网站 → Nginx → 配置」，保存前会执行 nginx -t。\n\
         worker_processes {wp};\n\
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
         \x20           allow 127.0.0.1;\n\
         \x20           deny all;\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n",
        wp = p.worker_processes.trim(),
        wc = p.worker_connections,
        kt = p.keepalive_timeout,
        body = p.client_max_body_size.trim(),
        gzip = if p.gzip { "on" } else { "off" },
        gtypes = p.gzip_types.trim(),
    )
}

/// 首次安装/重建时确保主配置存在；已存在则不动（用户可能改过）。
pub fn ensure_nginx_main_conf(s: &SiteSettings, db: &Db) -> std::io::Result<()> {
    std::fs::create_dir_all(s.global_log_dir())?;
    if let Some(parent) = s.nginx_main_conf().parent() {
        std::fs::create_dir_all(parent)?;
    }
    if s.nginx_main_conf().is_file() {
        return Ok(());
    }
    let params = db
        .get_setting(NGINX_PARAMS_KEY)
        .ok()
        .flatten()
        .map(|raw| NginxParams::from_json(&raw))
        .unwrap_or_default();
    std::fs::write(s.nginx_main_conf(), render_main_nginx_conf(&params))
}

#[derive(Debug, Clone)]
pub struct NginxStatusView {
    pub available: bool,
    pub reason: String,
    pub container_state: String,
    pub version: String,
    pub config_test: String,
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

#[derive(Debug, Clone)]
pub struct NginxLogView {
    pub error_path: String,
    pub error_lines: Vec<String>,
    pub error_reason: String,
    pub access_path: String,
    pub access_reason: String,
    pub status_counts: Vec<StatusCount>,
    pub recent: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NginxModuleView {
    pub available: bool,
    pub reason: String,
    pub version: String,
    pub configure_args: Vec<String>,
    pub modules: Vec<String>,
}

fn error_line(out: &str) -> String {
    out.lines()
        .find(|l| l.contains("emerg") || l.contains("error") || l.contains("[warn]"))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| out.trim().to_string())
}

fn read_tail(path: &Path, max: usize) -> Result<Vec<String>, String> {
    if !path.is_file() {
        return Err(format!("日志文件不存在：{}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取日志失败：{e}"))?;
    Ok(text
        .lines()
        .rev()
        .take(max)
        .map(|l| l.to_string())
        .collect())
}

fn status_of_line(line: &str) -> Option<u16> {
    line.split(|c: char| c.is_whitespace()).find_map(|tok| {
        let t = tok.trim_matches(|c| c == '"' || c == '[' || c == ']');
        if t.len() == 3 && t.bytes().all(|b| b.is_ascii_digit()) {
            t.parse::<u16>().ok().filter(|n| (100..600).contains(n))
        } else {
            None
        }
    })
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

impl AppState {
    async fn nginx_container(&self) -> Result<(String, String), SiteError> {
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
        Ok((c.id.clone(), c.state.clone()))
    }

    async fn apply_nginx_main(&self, s: &SiteSettings, content: &str) -> Result<(), SiteError> {
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
            let _ = std::fs::remove_file(&candidate);
            return Err(SiteError::Conflict(format!(
                "nginx -t 未通过（{}）：{}",
                container_candidate,
                error_line(&out)
            )));
        }
        let original = std::fs::read(s.nginx_main_conf()).ok();
        std::fs::rename(&candidate, s.nginx_main_conf())
            .map_err(|e| SiteError::Internal(format!("替换主配置失败：{e}")))?;
        if let Err(e) = self.restart_nginx().await {
            if let Some(bytes) = original {
                let _ = std::fs::write(s.nginx_main_conf(), bytes);
                let _ = self.restart_nginx().await;
            }
            return Err(SiteError::Unavailable(format!(
                "配置已回滚：重载 nginx 失败：{e}"
            )));
        }
        Ok(())
    }

    pub async fn nginx_status(&self) -> NginxStatusView {
        let s = self.site_settings();
        let mut v = NginxStatusView {
            available: false,
            reason: String::new(),
            container_state: "missing".into(),
            version: String::new(),
            config_test: String::new(),
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
            Ok((_id, state)) => {
                v.available = true;
                v.container_state = state;
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
                format!("配置校验通过（{}）", s.nginx_main_conf().display())
            } else {
                format!("配置校验失败：{}", error_line(&out))
            };
        } else {
            v.config_test = "无法执行 nginx -t（容器不可执行）".into();
        }
        let status_url = "http://127.0.0.1:8081/nginx_status";
        let fetched = self
            .docker()
            .exec_capture(
                NGINX_CONTAINER,
                vec![
                    "curl".into(),
                    "-fsS".into(),
                    "--max-time".into(),
                    "3".into(),
                    status_url.into(),
                ],
            )
            .await
            .ok()
            .filter(|(code, _)| *code == 0);
        let out = match fetched {
            Some((_, out)) => Ok(out),
            None => match self
                .docker()
                .exec_capture(
                    NGINX_CONTAINER,
                    vec![
                        "wget".into(),
                        "-qO-".into(),
                        "--timeout=3".into(),
                        status_url.into(),
                    ],
                )
                .await
            {
                Ok((0, out)) => Ok(out),
                Ok((_, out)) => Err(format!("读取 stub_status 失败：{}", error_line(&out))),
                Err(e) => Err(format!(
                    "读取 stub_status 失败（容器内没有可用的 curl/wget）：{e}"
                )),
            },
        };
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
            Err(e) => v.stub_reason = e,
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

    pub async fn save_nginx_main(&self, content: &str) -> Result<(), SiteError> {
        if content.trim().is_empty() {
            return Err(SiteError::BadRequest("主配置不能为空".into()));
        }
        let s = self.site_settings();
        self.apply_nginx_main(&s, content).await
    }

    pub async fn save_nginx_params(&self, params: &NginxParams) -> Result<(), SiteError> {
        params.validate()?;
        let s = self.site_settings();
        self.apply_nginx_main(&s, &render_main_nginx_conf(params))
            .await?;
        self.db()
            .set_setting(NGINX_PARAMS_KEY, &params.to_json())
            .map_err(|e| SiteError::Internal(format!("保存 nginx 参数失败：{e}")))
    }

    pub fn nginx_logs(&self, keyword: &str) -> NginxLogView {
        let s = self.site_settings();
        let error_path = s.global_log_dir().join("nginx-error.log");
        let kw = keyword.trim().to_ascii_lowercase();
        let mut error_reason = String::new();
        let error_lines = match read_tail(&error_path, 200) {
            Ok(lines) => lines
                .into_iter()
                .filter(|l| kw.is_empty() || l.to_ascii_lowercase().contains(&kw))
                .take(100)
                .collect(),
            Err(e) => {
                error_reason = e;
                Vec::new()
            }
        };
        let global_access = s.global_log_dir().join("nginx-access.log");
        let mut paths = vec![global_access.clone()];
        let sites = self.db().list_sites().unwrap_or_default();
        let mut site_lines: Vec<(String, Vec<String>)> = Vec::new();
        for site in &sites {
            let dir = std::path::PathBuf::from(&site.root_dir);
            let path = s.log_file(&dir, LogKind::Access);
            if path.is_file() {
                if let Ok(lines) = read_tail(&path, 200) {
                    paths.push(path);
                    site_lines.push((site.domain.clone(), lines));
                }
            }
        }
        let mut all: Vec<String> = Vec::new();
        for (_, lines) in &site_lines {
            all.extend(lines.iter().cloned());
        }
        if let Ok(lines) = read_tail(&global_access, 200) {
            all.extend(lines);
        }
        all.retain(|l| kw.is_empty() || l.to_ascii_lowercase().contains(&kw));
        all.sort_by(|a, b| b.cmp(a));
        let mut c2 = 0u64;
        let mut c3 = 0u64;
        let mut c4 = 0u64;
        let mut c5 = 0u64;
        for line in &all {
            match status_of_line(line) {
                Some(n) if n < 300 => c2 += 1,
                Some(n) if n < 400 => c3 += 1,
                Some(n) if n < 500 => c4 += 1,
                Some(_) => c5 += 1,
                None => {}
            }
        }
        let access_path = if paths.len() > 1 {
            format!("{} 个日志文件（全局 + 站点）", paths.len())
        } else {
            global_access.display().to_string()
        };
        let access_reason = if all.is_empty() {
            format!(
                "暂无可读访问日志；已检查：{}",
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            )
        } else {
            String::new()
        };
        NginxLogView {
            error_path: error_path.display().to_string(),
            error_lines,
            error_reason,
            access_path,
            access_reason,
            status_counts: vec![
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
            ],
            recent: all.into_iter().take(30).collect(),
        }
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
