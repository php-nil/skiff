//! Web 服务页 Schema：状态 / 配置 / 日志 / 模块四个子视图。

use super::schema_kit::{self as kit, flag, metric_card, text, API_BASE};
use serde_json::{json, Value};

fn num_text(v: &Value, pointer: &str) -> String {
    match v.pointer(pointer) {
        Some(Value::Null) | None => "—".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// `query`：日志筛选的当前值（type / lines / level / status / q）。
pub fn web_schema(data: &Value, view: &str, query: &Value, logs: Option<&Value>) -> Value {
    let status = data.get("status").cloned().unwrap_or(Value::Null);
    let view = match view {
        "config" | "logs" | "modules" => view,
        _ => "status",
    };
    let mut body = Vec::new();
    // 页头主操作：只有状态视图有（启停 / 重启 / 安装）。
    let mut actions = Vec::new();
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
        "status" => build_status(data, &status, &mut body, &mut actions),
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
    kit::page_with_actions("Web 服务", subtitle, actions, body)
}

fn build_status(data: &Value, status: &Value, body: &mut Vec<Value>, actions: &mut Vec<Value>) {
    let container_state = text(status, "/container_state");
    body.push(json!({
        "type": "grid",
        "props": { "columns": 4, "gap": 12 },
        "content": { "body": [
            metric_card(
                "容器",
                if container_state.is_empty() { "未运行" } else { container_state },
                "",
                "OpenResty",
                (container_state == "running").then_some("success")
            ),
            metric_card("版本", {
                let v = text(status, "/version");
                if v.is_empty() { "—" } else { v }
            }, "", "镜像版本", None),
            metric_card(
                "运行时长",
                {
                    let v = text(status, "/uptime");
                    if v.is_empty() { "—" } else { v }
                },
                "",
                &format!(
                    "最近 reload：{}",
                    {
                        let v = text(status, "/last_reload");
                        if v.is_empty() { "—" } else { v }
                    }
                ),
                None
            ),
            metric_card(
                "累计请求",
                num_text(status, "/requests"),
                "",
                &format!("active {}", num_text(status, "/active")),
                None
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
    let install_running = flag(&install, "/running");
    let installed = !container_state.is_empty() && container_state != "missing";
    let running = container_state == "running";

    if flag(data, "/can_admin") {
        // 启停与重启是日常运维动作，跟标题同一行放右上角；破坏性动作留在正文卡片。
        if installed {
            actions.push(web_button(
                if running { "停止" } else { "启动" },
                if running { "stop" } else { "start" },
                "",
                "tonal",
            ));
            actions.push(web_button("重启", "restart", "", "tonal"));
        } else if !install_running {
            actions.push(web_button("安装 Web 服务", "install", "primary", "flat"));
        }
    }

    // 安装过程分两态：
    // - 进行中 / 失败 / 中断 → 顶部一行状态（告警条）+ 步骤表 + 发起人小字；
    // - 已完成 → 页尾一行摘要，不占首屏（见 `install_summary`）。
    let succeeded = install_succeeded(&install);
    if !succeeded {
        if let Some(node) = install_alert(&install) {
            body.push(node);
            if !text(&install, "/job_id").is_empty() {
                body.push(job_detail_button(&install));
            }
            if !steps_of(&install).is_empty() {
                body.push(install_steps_table(install_running));
            }
            if let Some(meta) = install_meta(&install) {
                body.push(meta);
            }
            if install_running && flag(data, "/can_admin") {
                body.push(kit::action_button(
                    "刷新安装进度",
                    vec![json!({ "type": "skiffQueryNav", "props": { "path": "${prefix}/sites/web", "query": {} } })],
                    "tonal",
                ));
            }
        }
    }

    if installed && flag(data, "/can_admin") {
        body.push(kit::card(
            "重新安装与删除",
            vec![
                kit::text_line(
                    "重新安装会按当前配置重建 OpenResty 容器；删除容器后所有站点立即不可访问。两者都保留配置与站点目录。语言运行环境在「运行环境」页单独管理。",
                ),
                kit::flex(
                    json!({ "gap": 8 }),
                    vec![
                        web_button("重新安装", "rebuild", "", "tonal"),
                        web_button("删除容器", "delete", "danger", "tonal"),
                    ],
                ),
            ],
        ));
    }

    // 完成态收敛成页尾一行摘要：安装过程细节进任务详情，不占首屏视觉重心。
    if succeeded {
        body.push(install_summary(&install));
    }
}

/// 步骤清单；`steps` 缺失或为空时返回空切片。
fn steps_of(install: &Value) -> &[Value] {
    install
        .get("steps")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// 安装是否已成功结束：非进行中、未中断、无错误，且确实留下步骤（从未安装过时为假）。
fn install_succeeded(install: &Value) -> bool {
    !flag(install, "/running")
        && !flag(install, "/interrupted")
        && text(install, "/error").is_empty()
        && !steps_of(install).is_empty()
}

/// 进行中 / 失败 / 中断时的顶部告警；已完成不占顶部（见 [`install_summary`]）。
fn install_alert(install: &Value) -> Option<Value> {
    let (kind, title) = if flag(install, "/running") {
        ("info", "正在安装 Web 服务")
    } else if flag(install, "/interrupted") {
        ("warning", "安装已中断")
    } else if !text(install, "/error").is_empty() {
        ("danger", "安装失败")
    } else {
        return None;
    };
    Some(kit::alert(kind, title, install_headline(install)))
}

/// 告警正文只放**一行**状态。
///
/// budding 的 alert 正文是单个文本节点（不吃 `\n`），多行拼装会挤成一大段——这正是
/// 「安装进度挤成一坨」的老问题。步骤进表格、发起人进小字，告警只留结论。
fn install_headline(install: &Value) -> String {
    if flag(install, "/running") {
        let phase = match text(install, "/phase_label") {
            "" => "准备中",
            label => label,
        };
        let progress = install
            .pointer("/progress")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        return format!("状态：{phase}（{progress}%）");
    }
    if flag(install, "/interrupted") {
        return "状态：已中断（服务重启等原因），可重新安装".to_string();
    }
    format!("失败原因：{}", text(install, "/error"))
}

/// 发起人 / 来源 IP / 开始时间：放步骤表下面一行小字。
fn install_meta(install: &Value) -> Option<Value> {
    let (by, ip, started) = (
        text(install, "/created_by"),
        text(install, "/requester_ip"),
        text(install, "/started_at"),
    );
    if by.is_empty() && ip.is_empty() && started.is_empty() {
        return None;
    }
    Some(kit::small_text(format!(
        "发起人 {}（{}） · 来源 IP {} · 开始 {}",
        kit::or_dash(by),
        kit::or_dash(text(install, "/created_by_role")),
        kit::or_dash(ip),
        kit::or_dash(started)
    )))
}

/// 已完成安装的摘要卡（页尾）：一行结果 + 任务详情入口。
///
/// 旧实现把「安装已完成」连同每一步明细塞进顶部全宽 success 告警，安装成功后首屏
/// 被一大块绿色提示占住，而它其实是历史信息。改成低调摘要，过程要看就进任务详情。
fn install_summary(install: &Value) -> Value {
    let line = format!(
        "{} · {} 步 · 耗时 {} · 发起人 {}（{}） · 结束 {}",
        if flag(install, "/done") {
            "已完成"
        } else {
            "已结束"
        },
        steps_of(install).len(),
        kit::or_dash(text(install, "/duration")),
        kit::or_dash(text(install, "/created_by")),
        kit::or_dash(text(install, "/created_by_role")),
        kit::or_dash(text(install, "/finished_at")),
    );
    let mut body = vec![kit::small_text(line)];
    if !text(install, "/job_id").is_empty() {
        body.push(job_detail_button(install));
    }
    kit::card("最近一次安装", body)
}

/// 安装步骤表；`auto_refresh` 只在进行中开启（失败态是一次性快照，不必轮询）。
fn install_steps_table(auto_refresh: bool) -> Value {
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
    if auto_refresh {
        // 进行中按 2 秒轮询：由框架统一驱动，节点隐藏或离开页面时自动停止。
        table["source"]["autoRefresh"] = json!(2000);
    }
    table["props"]["itemsPath"] = json!("data.steps");
    table["props"]["pagination"] = json!(false);
    table["props"]["paging"] = json!(null);
    table
}

/// 作业详情入口。作业早已收编到「任务审计 → 任务中心」，路径是
/// `/tasks/jobs/<id>`；旧实现写 `/jobs/<id>`（已被删除的旧路径）。
fn job_detail_button(install: &Value) -> Value {
    let job_id = text(install, "/job_id");
    kit::action_button(
        "查看任务详情",
        vec![
            json!({ "type": "skiffQueryNav", "props": { "path": format!("${{prefix}}/tasks/jobs/{job_id}"), "query": {} } }),
        ],
        "text",
    )
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
        json!({ "gap": 8 }),
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
                format!("文件：{}", kit::or_dash(text(logs, "/path"))),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn page_data(container_state: &str, install: Value) -> Value {
        json!({
            "status": { "available": true, "container_state": container_state },
            "install": install,
            "can_admin": true,
            "config": {},
            "modules": {},
        })
    }

    fn action_labels(page: &Value) -> Vec<String> {
        page["content"]["actions"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|b| b["props"]["text"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn card_titles(page: &Value) -> Vec<String> {
        page["body"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|n| n["type"] == "card")
                    .map(|n| n["props"]["title"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 安装完成后首屏不再出现整块绿色告警：结论收敛成页尾摘要卡，细节进任务详情。
    #[test]
    fn finished_install_collapses_to_footer_summary() {
        let install = json!({
            "job_id": "j1",
            "running": false, "done": true, "interrupted": false, "error": "",
            "duration": "16 秒", "created_by": "admin", "created_by_role": "admin",
            "finished_at": "2026-11-24T20:10:00+08:00",
            "steps": [{ "key": "check", "name": "检查 Docker", "state": "ok", "detail": "" }],
        });
        let page = web_schema(&page_data("running", install), "status", &json!({}), None);
        let body = page["body"].as_array().expect("页面必须有正文");
        assert!(
            !body
                .iter()
                .any(|n| n["type"] == "alert" && n["props"]["type"] == "success"),
            "完成态不得再占用顶部告警位"
        );
        let last = body.last().expect("摘要卡必须在页尾");
        assert_eq!(last["props"]["title"], "最近一次安装");
        let summary = last["content"]["body"].to_string();
        assert!(summary.contains("已完成") && summary.contains("16 秒"));
        assert!(
            summary.contains("/tasks/jobs/j1"),
            "任务详情入口必须指向 /tasks/jobs/<id>（旧 /jobs 路径已下线）"
        );
        // 启停与重启在页头，正文不再重复。
        assert_eq!(action_labels(&page), vec!["停止", "重启"]);
    }

    /// 进行中仍然是顶部告警 + 轮询步骤表，且此时不提供「安装」入口。
    #[test]
    fn running_install_keeps_progress_alert_and_steps_table() {
        let install = json!({
            "job_id": "j2",
            "running": true, "done": false, "interrupted": false, "error": "",
            "phase_label": "拉取镜像", "progress": 30,
            "steps": [{ "key": "pull", "name": "拉取镜像", "state": "running", "detail": "" }],
        });
        let page = web_schema(&page_data("missing", install), "status", &json!({}), None);
        let body = page["body"].as_array().expect("页面必须有正文");
        let alert = body
            .iter()
            .find(|n| n["type"] == "alert" && n["props"]["title"] == "正在安装 Web 服务")
            .expect("进行中必须有顶部进度告警");
        // 告警正文只有一行状态：多行明细会被 budding 的 alert 折叠成一段。
        assert_eq!(alert["props"]["text"], json!("状态：拉取镜像（30%）"));
        let table = body
            .iter()
            .find(|n| n["type"] == "table")
            .expect("进行中必须给出步骤表");
        assert_eq!(table["source"]["autoRefresh"], json!(2000));
        assert_eq!(table["props"]["itemsPath"], json!("data.steps"));
        assert!(card_titles(&page).iter().all(|t| t != "最近一次安装"));
        assert!(
            action_labels(&page).is_empty(),
            "安装进行中不得再给「安装 Web 服务」入口"
        );
    }

    /// 失败态：顶部 danger 告警 + 正文可点进任务详情，不出现完成摘要。
    #[test]
    fn failed_install_keeps_error_alert() {
        let install = json!({
            "job_id": "j3",
            "running": false, "done": false, "interrupted": false,
            "error": "宿主端口 80 已被占用",
            "steps": [{ "key": "check", "name": "检查 Docker 与 80 端口", "state": "failed", "detail": "" }],
        });
        let page = web_schema(&page_data("missing", install), "status", &json!({}), None);
        let body = page["body"].as_array().expect("页面必须有正文");
        let alert = body
            .iter()
            .find(|n| n["type"] == "alert" && n["props"]["type"] == "danger")
            .expect("失败必须有 danger 告警");
        assert!(alert["props"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("宿主端口 80 已被占用"));
        // 失败态的步骤表是一次性快照：不轮询，但要看得到哪一步炸了。
        let table = body
            .iter()
            .find(|n| n["type"] == "table")
            .expect("失败态也要给出步骤表");
        assert!(
            table["source"].get("autoRefresh").is_none(),
            "失败态不该继续轮询"
        );
        assert!(
            body.iter()
                .any(|n| n["type"] == "button" && n["props"]["text"] == "查看任务详情"),
            "失败态必须能进任务详情"
        );
        assert!(card_titles(&page).iter().all(|t| t != "最近一次安装"));
        assert_eq!(action_labels(&page), vec!["安装 Web 服务"]);
    }

    /// 已安装但容器未运行：页头给「启动」，不再出现「停止」。
    #[test]
    fn stopped_container_offers_start_in_header() {
        let page = web_schema(
            &page_data("exited", json!({ "running": false, "steps": [] })),
            "status",
            &json!({}),
            None,
        );
        assert_eq!(action_labels(&page), vec!["启动", "重启"]);
        // 「重新安装 / 删除」留在正文卡片，不进页头。
        assert!(card_titles(&page).iter().any(|t| t == "重新安装与删除"));
    }
}
