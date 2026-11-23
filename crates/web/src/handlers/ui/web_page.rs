//! Web 服务页 Schema：状态 / 配置 / 日志 / 模块四个子视图。

use super::schema_kit::{self as kit, API_BASE};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false)
}

fn num_text(v: &Value, pointer: &str) -> String {
    match v.pointer(pointer) {
        Some(Value::Null) | None => "—".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn metric_card(title: &str, value: &str, caption: &str, color: &str) -> Value {
    let mut stat = json!({ "value": value, "label": caption, "suffix": "" });
    if !color.is_empty() {
        stat["color"] = json!(color);
    }
    json!({
        "type": "card",
        "props": { "title": title, "variant": "outlined" },
        "content": { "body": [ { "type": "statistic", "props": stat } ] }
    })
}

/// `query`：日志筛选的当前值（type / lines / level / status / q）。
pub fn web_schema(data: &Value, view: &str, query: &Value, logs: Option<&Value>) -> Value {
    let status = data.get("status").cloned().unwrap_or(Value::Null);
    let view = match view {
        "config" | "logs" | "modules" => view,
        _ => "status",
    };
    let mut body = Vec::new();
    if !flag(&status, "/available") {
        let reason = {
            let r = text(&status, "/reason");
            if r.is_empty() {
                let stub = text(&status, "/stub_reason");
                if stub.is_empty() {
                    "OpenResty 容器未运行；配置可以保存，但需要先启动容器才能生效。".to_string()
                } else {
                    stub.to_string()
                }
            } else {
                r.to_string()
            }
        };
        body.push(kit::alert("warning", "Web 服务不可用", reason));
    }

    match view {
        "status" => build_status(data, &status, &mut body),
        "config" => build_config(data, &mut body),
        "logs" => build_logs(query, logs, &mut body),
        _ => build_modules(data, &mut body),
    }

    let subtitle = match view {
        "config" => "主配置、性能参数与站点配置；保存前执行 nginx -t，失败拒绝写入",
        "logs" => "按需读取访问/错误日志尾部；默认参数为访问日志 200 行",
        "modules" => "nginx -V 编译参数与模块清单",
        _ => "OpenResty 运行状态、安装进度与容器运维",
    };
    kit::page("Web 服务", subtitle, body)
}

fn build_status(data: &Value, status: &Value, body: &mut Vec<Value>) {
    let container_state = text(status, "/container_state");
    body.push(json!({
        "type": "grid",
        "props": { "columns": 4, "gap": 12 },
        "content": { "body": [
            metric_card(
                "容器",
                if container_state.is_empty() { "未运行" } else { container_state },
                "OpenResty",
                if container_state == "running" { "success" } else { "" }
            ),
            metric_card("版本", {
                let v = text(status, "/version");
                if v.is_empty() { "—" } else { v }
            }, "镜像版本", ""),
            metric_card(
                "运行时长",
                {
                    let v = text(status, "/uptime");
                    if v.is_empty() { "—" } else { v }
                },
                &format!(
                    "最近 reload：{}",
                    {
                        let v = text(status, "/last_reload");
                        if v.is_empty() { "—" } else { v }
                    }
                ),
                ""
            ),
            metric_card(
                "累计请求",
                &num_text(status, "/requests"),
                &format!("active {}", num_text(status, "/active")),
                ""
            )
        ] }
    }));
    body.push(kit::detail_lines(
        "Nginx 运行指标",
        vec![
            format!("配置检查：{}", {
                let v = text(status, "/config_test");
                if v.is_empty() {
                    "—"
                } else {
                    v
                }
            }),
            format!(
                "accepts {} / handled {} / requests {}",
                num_text(status, "/accepts"),
                num_text(status, "/handled"),
                num_text(status, "/requests")
            ),
            format!(
                "reading {} / writing {} / waiting {}",
                num_text(status, "/reading"),
                num_text(status, "/writing"),
                num_text(status, "/waiting")
            ),
            format!(
                "探测地址：{}{}",
                {
                    let v = text(status, "/probe_url");
                    if v.is_empty() {
                        "—"
                    } else {
                        v
                    }
                },
                {
                    let err = text(status, "/probe_error");
                    if err.is_empty() {
                        String::new()
                    } else {
                        format!("（{err}）")
                    }
                }
            ),
            format!(
                "主配置：{}{}",
                {
                    let v = text(status, "/conf_path");
                    if v.is_empty() {
                        "—"
                    } else {
                        v
                    }
                },
                if flag(status, "/conf_has_stub") {
                    "（含 stub_status）"
                } else {
                    "（未含 stub_status）"
                }
            ),
        ],
    ));

    let install = data.get("install").cloned().unwrap_or(Value::Null);
    if let Some(node) = install_node(&install) {
        body.push(node);
        let job_id = text(&install, "/job_id");
        if !job_id.is_empty() {
            body.push(kit::action_button(
                "查看任务详情",
                vec![json!({ "type": "skiffQueryNav", "props": { "path": format!("${{prefix}}/jobs/{job_id}"), "query": {} } })],
                "text",
            ));
        }
        // 步骤表按 2 秒自动刷新：旧实现在壳脚本里定时改 DOM，改成 `source.autoRefresh`
        // 后由框架统一轮询，节点隐藏或离开页面时自动停止。
        if flag(&install, "/running") {
            let steps: Vec<Value> = install
                .get("steps")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let _ = steps;
            let mut table = kit::list_table(
                json!([
                    { "key": "name", "label": "步骤" },
                    {
                        "key": "state",
                        "label": "状态",
                        "text": "${state === 'ok' ? '完成' : state === 'running' ? '进行中' : state === 'failed' ? '失败' : '待执行'}"
                    },
                    { "key": "detail", "label": "说明" },
                    { "key": "started_at", "label": "开始" }
                ]),
                &format!("{API_BASE}/web/install"),
                "name",
                json!(null),
                kit::TableOpts {
                    empty_text: Some("还没有步骤".to_string()),
                    ..Default::default()
                },
            );
            table["source"]["autoRefresh"] = json!(2000);
            table["props"]["itemsPath"] = json!("data.steps");
            table["props"]["pagination"] = json!(false);
            table["props"]["paging"] = json!(null);
            body.push(table);
        }
    }

    if flag(data, "/can_admin") {
        let installed = !container_state.is_empty() && container_state != "missing";
        let running = container_state == "running";
        let mut ops = Vec::new();
        if !installed {
            if !flag(&install, "/running") {
                ops.push(web_button("安装 Web 服务", "install", "primary", "flat"));
            }
        } else {
            ops.push(web_button(
                if running { "停止" } else { "启动" },
                if running { "stop" } else { "start" },
                "",
                "tonal",
            ));
            ops.push(web_button("重启", "restart", "", "tonal"));
            ops.push(web_button("重新安装", "rebuild", "", "tonal"));
            ops.push(web_button("删除容器", "delete", "danger", "tonal"));
        }
        if flag(&install, "/running") {
            ops.push(kit::action_button(
                "刷新安装进度",
                vec![json!({ "type": "skiffQueryNav", "props": { "path": "${prefix}/sites/web", "query": {} } })],
                "tonal",
            ));
        }
        body.push(kit::card(
            "容器运维",
            vec![
                kit::text_line(
                    "Web 服务（OpenResty）是站点依赖的基础容器；停止或删除会立即影响所有站点。语言运行环境在「运行环境」页单独管理。",
                ),
                kit::flex(8, ops),
            ],
        ));
    }
}

fn install_node(install: &Value) -> Option<Value> {
    let running = flag(install, "/running");
    let error = text(install, "/error");
    let has_steps = install
        .get("steps")
        .and_then(Value::as_array)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    if !(running || !error.is_empty() || has_steps) {
        return None;
    }
    let interrupted = flag(install, "/interrupted");
    let (kind, title) = if running {
        ("info", "正在安装 Web 服务")
    } else if interrupted {
        ("warning", "安装已中断")
    } else if !error.is_empty() {
        ("danger", "安装失败")
    } else {
        ("success", "安装已完成")
    };
    Some(kit::alert(kind, title, install_text(install)))
}

fn install_text(install: &Value) -> String {
    let mut lines = Vec::new();
    if flag(install, "/running") {
        lines.push(format!(
            "状态：{}（{}%）",
            {
                let p = text(install, "/phase_label");
                if p.is_empty() {
                    "准备中"
                } else {
                    p
                }
            },
            install
                .pointer("/progress")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        ));
    } else if flag(install, "/interrupted") {
        lines.push("状态：已中断".to_string());
    } else if !text(install, "/error").is_empty() {
        lines.push("状态：失败".to_string());
    } else if flag(install, "/done") {
        lines.push("状态：已完成".to_string());
    } else {
        lines.push("状态：待安装".to_string());
    }
    let error = text(install, "/error");
    if !error.is_empty() {
        lines.push(format!("失败原因：{error}"));
    }
    lines.push(format!(
        "发起人：{}（{}） · 来源 IP：{}",
        or_dash(text(install, "/created_by")),
        or_dash(text(install, "/created_by_role")),
        or_dash(text(install, "/requester_ip"))
    ));
    lines.push(format!(
        "开始：{} · 结束：{} · 耗时：{}",
        or_dash(text(install, "/started_at")),
        or_dash(text(install, "/finished_at")),
        or_dash(text(install, "/duration"))
    ));
    if let Some(steps) = install.get("steps").and_then(Value::as_array) {
        for step in steps {
            let state = match text(step, "/state") {
                "ok" => "完成",
                "running" => "进行中",
                "failed" => "失败",
                _ => "待执行",
            };
            let detail = text(step, "/detail");
            let started = text(step, "/started_at");
            lines.push(format!(
                "· {}：{}{}{}",
                text(step, "/name"),
                state,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("（{detail}）")
                },
                if started.is_empty() {
                    String::new()
                } else {
                    format!(" [{started}]")
                }
            ));
        }
    }
    lines.join("\n")
}

fn web_button(text: &str, op: &str, color: &str, variant: &str) -> Value {
    let mut props = json!({ "text": text, "variant": variant });
    if !color.is_empty() {
        props["color"] = json!(color);
    }
    json!({
        "type": "button",
        "props": props,
        "onEvent": { "click": { "actions": [
            { "type": "skiffWebEnv", "props": { "op": op } }
        ] } }
    })
}

fn build_config(data: &Value, body: &mut Vec<Value>) {
    let config = data.get("config").cloned().unwrap_or(Value::Null);
    let params = config.get("params").cloned().unwrap_or(Value::Null);

    let main_path = text(&config, "/main_path");
    body.push(kit::write_card(
        "主配置",
        "保存前执行 nginx -t；失败会拒绝写入并保留当前配置。",
        kit::form(
            json!({ "type": "skiffWebConfig" }),
            vec![kit::textarea(
                "content",
                &format!(
                    "主配置 {}",
                    if main_path.is_empty() {
                        "nginx.conf"
                    } else {
                        main_path
                    }
                ),
                20,
            )],
            "校验并重载",
            Some(json!({ "content": text(&config, "/main_content") })),
        ),
    ));

    let mut connections = kit::text_input("worker_connections", "worker_connections（64–65535）");
    connections["props"]["rules"] = json!({
        "pattern": "^[0-9]+$",
        "messages": { "pattern": "worker_connections 需为 64–65535 的整数" }
    });
    let mut keepalive = kit::text_input("keepalive_timeout", "keepalive_timeout（0–3600 秒）");
    keepalive["props"]["rules"] = json!({
        "pattern": "^[0-9]+$",
        "messages": { "pattern": "keepalive_timeout 需为 0–3600 的整数" }
    });
    body.push(kit::write_card(
        "性能参数",
        "数字字段非法会被拒绝，不会静默存成默认值。",
        kit::form(
            json!({ "type": "skiffWebParams" }),
            vec![
                kit::text_input("worker_processes", "worker_processes"),
                connections,
                keepalive,
                kit::text_input("client_max_body_size", "client_max_body_size"),
                json!({ "type": "checkbox", "props": { "name": "gzip", "label": "启用 gzip" } }),
                kit::text_input("gzip_types", "gzip_types（空格分隔）"),
            ],
            "保存性能参数",
            Some(json!({
                "worker_processes": text(&params, "/worker_processes"),
                "worker_connections": num_text(&params, "/worker_connections"),
                "keepalive_timeout": num_text(&params, "/keepalive_timeout"),
                "client_max_body_size": text(&params, "/client_max_body_size"),
                "gzip": flag(&params, "/gzip"),
                "gzip_types": text(&params, "/gzip_types")
            })),
        ),
    ));

    let include_path = text(&config, "/include_path");
    let mut include_body = vec![kit::flex(
        8,
        vec![kit::text_line(format!(
            "include：{}",
            if include_path.is_empty() {
                "—"
            } else {
                include_path
            }
        ))],
    )];
    if text(&config, "/include_content").is_empty() {
        include_body.push(kit::text_line("（空）"));
    } else {
        include_body.push(kit::code_block(text(&config, "/include_content"), 260));
    }
    let mut site_cards = Vec::new();
    if let Some(sites) = config.get("sites").and_then(Value::as_array) {
        for site in sites {
            let content = text(site, "/content");
            site_cards.push(kit::card(
                &format!("{} · {}", text(site, "/domain"), text(site, "/path")),
                vec![if content.is_empty() {
                    kit::text_line("（空）")
                } else {
                    kit::code_block(content, 260)
                }],
            ));
        }
    }
    include_body.extend(site_cards);
    body.push(kit::card("包含配置与站点配置", include_body));
}

fn build_logs(query: &Value, logs: Option<&Value>, body: &mut Vec<Value>) {
    let log_form = kit::form(
        json!({ "type": "skiffWebLogs" }),
        vec![
            kit::select(
                "type",
                "日志",
                "",
                vec![("访问日志", "access"), ("错误日志", "error")],
            ),
            kit::select(
                "lines",
                "范围",
                "",
                vec![("50 行", "50"), ("200 行", "200"), ("500 行", "500")],
            ),
            kit::select(
                "level",
                "级别（错误日志）",
                "",
                vec![
                    ("全部", "all"),
                    ("error", "error"),
                    ("warn", "warn"),
                    ("info", "info"),
                ],
            ),
            kit::text_input("status", "状态码（如 404 / 4xx）"),
            kit::text_input("q", "关键字"),
        ],
        "加载日志",
        Some(json!({
            "type": if text(query, "/type").is_empty() { "access" } else { text(query, "/type") },
            "lines": if text(query, "/lines").is_empty() { "200" } else { text(query, "/lines") },
            "level": if text(query, "/level").is_empty() { "all" } else { text(query, "/level") },
            "status": text(query, "/status"),
            "q": text(query, "/q")
        })),
    );
    let mut log_body = vec![log_form];
    if let Some(logs) = logs {
        let rows = logs
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        log_body.push(kit::detail_lines(
            "读取范围",
            vec![
                format!("文件：{}", or_dash(text(logs, "/path"))),
                format!(
                    "本次加载 {} 行；已扫描 {}（{} 行），文件共 {}",
                    rows.len(),
                    text(logs, "/scanned_text"),
                    num_text(logs, "/scanned_lines"),
                    text(logs, "/file_text")
                ),
                if flag(logs, "/truncated") {
                    "已截断：只读取文件尾部 2 MiB".to_string()
                } else {
                    "未截断".to_string()
                },
                text(logs, "/reason").to_string(),
            ],
        ));
        if let Some(counts) = logs.get("status_counts").and_then(Value::as_array) {
            if !counts.is_empty() {
                log_body.push(kit::detail_lines(
                    "状态码分布",
                    counts
                        .iter()
                        .map(|c| format!("{}：{}", text(c, "/label"), num_text(c, "/count")))
                        .collect(),
                ));
            }
        }
        if rows.is_empty() {
            log_body.push(kit::alert("info", "", "本次查询没有日志行"));
        } else {
            let joined = rows
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            log_body.push(kit::code_block(&joined, 480));
        }
    } else {
        log_body.push(json!({ "type": "card", "content": { "body": [
            kit::text_line("默认不读取日志：选择日志与范围后点「加载日志」。")
        ] } }));
    }
    body.push(kit::card("日志", log_body));
}

fn build_modules(data: &Value, body: &mut Vec<Value>) {
    let modules = data.get("modules").cloned().unwrap_or(Value::Null);
    let configure: Vec<String> = modules
        .get("configure_args")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mods: Vec<String> = modules
        .get("modules")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut lines = vec![
        format!("版本：{}", {
            let v = text(&modules, "/version");
            if v.is_empty() {
                "—"
            } else {
                v
            }
        }),
        if flag(&modules, "/available") {
            String::new()
        } else {
            format!("不可用：{}", text(&modules, "/reason"))
        },
        format!("编译参数：{}", configure.join(" "),),
        format!("模块列表：{}", mods.join(" ")),
    ];
    lines.retain(|line| !line.is_empty());
    body.push(kit::card(
        "模块",
        vec![kit::detail_lines("nginx -V", lines)],
    ));
}

fn or_dash(value: &str) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}
