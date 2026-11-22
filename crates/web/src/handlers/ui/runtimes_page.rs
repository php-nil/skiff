//! 运行环境页族：列表（含新建弹窗）与详情（五个子视图）。

use super::schema_kit::{self as kit, TableOpts, API_BASE};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn num(v: &Value, pointer: &str) -> i64 {
    v.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

/// 运行环境列表：游标表格 + 新建弹窗（类型/版本/实例名/软件源）。
pub fn runtimes_schema(catalog: &Value, total: i64) -> Value {
    let mut table = kit::list_table(
        json!([
            { "key": "kind", "label": "类型", "text": "${kind.label}" },
            { "key": "name", "label": "实例" },
            { "key": "version_display", "label": "版本" },
            { "key": "version_note", "label": "说明" },
            { "key": "state", "label": "状态", "text": "${state.label}" },
            { "key": "site_count", "label": "站点数", "type": "number" },
            { "key": "endpoint_short", "label": "接入" }
        ]),
        "/runtimes",
        "id",
        json!(null),
        TableOpts {
            empty_text: Some(
                "还没有运行环境，点击下方「新建运行环境」安装后即可绑定站点".to_string(),
            ),
            ..Default::default()
        },
    );
    table["onEvent"] = json!({
        "rowClick": { "actions": [
            { "type": "navigate", "props": { "url": "${prefix}/sites/runtimes/${id}" } }
        ] }
    });
    table["props"]["actionsLabel"] = json!("操作");
    table["props"]["rowActions"] = json!([
        kit::row_button(
            "详情",
            vec![
                json!({ "type": "navigate", "props": { "url": "${prefix}/sites/runtimes/${id}" } })
            ],
            "primary"
        ),
        kit::row_button(
            "日志",
            vec![
                json!({ "type": "navigate", "props": { "url": "${prefix}/sites/runtimes/${id}/logs" } })
            ],
            ""
        )
    ]);

    let tools = vec![create_dialog(catalog)];
    let body = vec![
        kit::toolbar(
            Some(kit::small_text(format!("共 {total} 个运行环境"))),
            tools,
        ),
        table,
    ];

    kit::page("运行环境", "镜像、实例与安装入口；未安装镜像会先拉取", body)
}

fn create_dialog(catalog: &Value) -> Value {
    let kinds: Vec<Value> = catalog
        .pointer("/kinds")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|k| json!({ "title": text(k, "/label"), "value": text(k, "/key") }))
                .collect()
        })
        .unwrap_or_default();
    // 软件源只作用于 PHP（Alpine 镜像内 apk）。候选来自后端单一来源；
    // 当前若固定为候选之外的自定义源，也要补进列表，避免提交时悄悄改回自动。
    let mut mirrors = vec![json!({ "title": "自动（安装时实测本机最快）", "value": "auto" })];
    if let Some(list) = catalog.pointer("/apk_mirrors").and_then(Value::as_array) {
        for m in list {
            mirrors.push(json!({ "title": text(m, "/label"), "value": text(m, "/base") }));
        }
    }
    let fixed = catalog
        .pointer("/apk_mirror_current/fixed")
        .and_then(Value::as_bool)
        == Some(true);
    let current_base = text(catalog, "/apk_mirror_current/base");
    if fixed && !current_base.is_empty() {
        let known = mirrors
            .iter()
            .any(|m| m.get("value").and_then(Value::as_str) == Some(current_base));
        if !known {
            mirrors.push(json!({
                "title": format!("{current_base}（自定义，当前固定）"),
                "value": current_base
            }));
        }
    }
    let default_php = text(catalog, "/default_php_version");
    let default_kind = kinds
        .first()
        .and_then(|k| k.get("value"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let default_mirror = if fixed && !current_base.is_empty() {
        current_base.to_string()
    } else {
        "auto".to_string()
    };

    let fields = vec![
        json!({ "type": "select", "props": { "name": "kind", "label": "类型", "items": kinds } }),
        json!({
            "type": "input-text",
            "props": {
                "name": "version",
                "label": "版本",
                "hint": format!(
                    "留空使用该类型默认版本；PHP 默认 {}",
                    if default_php.is_empty() { "8.5" } else { default_php }
                )
            }
        }),
        kit::text_input("name", "实例名（可选，默认按类型自动生成）"),
        json!({
            "type": "select",
            "props": {
                "name": "apk_mirror",
                "label": "软件包源（仅 PHP 生效）",
                "hint": "自动 = 安装时在容器内实测各源吞吐后择快；选错源会让装扩展直接失败（不只是慢）。",
                "items": mirrors
            }
        }),
    ];
    let initial = json!({ "kind": default_kind, "apk_mirror": default_mirror });
    let form = kit::form(
        &format!("{API_BASE}/runtimes"),
        json!({ "type": "skiffRuntimeCreate" }),
        fields,
        "创建并安装",
        Some(initial),
    );
    json!({
        "type": "button",
        "props": { "text": "新建运行环境", "variant": "flat", "color": "primary" },
        "action": {
            "type": "dialog",
            "props": { "title": "新建运行环境", "width": 640 },
            "content": {
                "body": [
                    kit::alert(
                        "info",
                        "",
                        "PHP 会按所选版本拉取镜像并安装默认扩展；其余类型按目录能力登记或安装。安装进度在列表状态列与详情日志中显示。"
                    ),
                    form
                ],
                "footer": [
                    { "type": "button", "props": { "text": "取消", "variant": "outlined" },
                      "action": { "type": "close" } }
                ]
            }
        }
    })
}

/// 运行环境详情：按 L3 子视图（概览 / 配置 / 扩展 / 站点 / 管理）分块。
pub fn runtime_detail_schema(data: &Value, runtime_id: &str, tab: &str) -> Value {
    let view = if tab.is_empty() { "overview" } else { tab };
    let rt = data.get("runtime").cloned().unwrap_or(Value::Null);
    let name = text(&rt, "/name");
    let site_count = num(&rt, "/site_count");
    let overview = vec![json!({
        "type": "card",
        "props": { "title": format!("{} / {}", text(&rt, "/kind/label"), name) },
        "content": { "body": [
            kit::detail_lines("实例", vec![
                format!(
                    "版本：{}（{}）",
                    {
                        let d = text(&rt, "/version_display");
                        if d.is_empty() { text(&rt, "/version") } else { d }
                    },
                    text(&rt, "/image")
                ),
                format!(
                    "状态：{}；容器：{}",
                    text(&rt, "/state/label"),
                    if text(&rt, "/container_state").is_empty() { "—" } else { text(&rt, "/container_state") }
                ),
                format!(
                    "接入：{}",
                    {
                        let full = text(&rt, "/endpoint_full");
                        if full.is_empty() { text(&rt, "/endpoint_short") } else { full }
                    }
                ),
                format!("站点数：{site_count}")
            ]),
            kit::flex(8, vec![
                runtime_op("启动", "start", runtime_id, ""),
                runtime_op("停止", "stop", runtime_id,
                    if site_count > 0 { "该环境被站点使用，停止会影响全部绑定站点。确认停止？" } else { "" }),
                runtime_op("重启", "restart", runtime_id,
                    if site_count > 0 { "该环境被站点使用，重启会短暂中断全部绑定站点。确认重启？" } else { "" }),
                runtime_op("重建", "rebuild", runtime_id,
                    "重建会按当前配置重新创建容器，期间服务中断。确认重建？"),
                runtime_op("同步", "sync", runtime_id, ""),
                runtime_op("删除", "delete", runtime_id,
                    "删除运行环境不可恢复，绑定站点会受影响。确认删除？")
            ]),
            kit::flex(8, vec![kit::action_button(
                "实测版本",
                vec![json!({ "type": "skiffRuntimeProbe", "props": { "id": runtime_id } })],
                "tonal"
            )])
        ] }
    })];

    let mut config: Vec<Value> = Vec::new();
    let mut extensions: Vec<Value> = Vec::new();
    let mut sites: Vec<Value> = Vec::new();
    let mut manage: Vec<Value> = Vec::new();

    if data
        .get("config_mounts_stale")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        config.push(kit::alert(
            "warning",
            "配置目录未挂载",
            "当前容器未挂载 php.ini / conf.d，配置能保存但不生效；请先执行「重建」再编辑配置。",
        ));
    }

    if data.get("is_php").and_then(Value::as_bool).unwrap_or(false) {
        let cfg = data.get("config").cloned().unwrap_or(Value::Null);
        let initial = json!({
            "memory_limit": text(&cfg, "/memory_limit"),
            "upload_max_filesize": text(&cfg, "/upload_max_filesize"),
            "post_max_size": text(&cfg, "/post_max_size"),
            "max_execution_time": num(&cfg, "/max_execution_time").to_string(),
            "pm": text(&cfg, "/pm"),
            "max_children": num(&cfg, "/max_children").to_string(),
            "start_servers": num(&cfg, "/start_servers").to_string(),
            "min_spare_servers": num(&cfg, "/min_spare_servers").to_string(),
            "max_spare_servers": num(&cfg, "/max_spare_servers").to_string(),
            "max_requests": num(&cfg, "/max_requests").to_string()
        });
        let fields = [
            ("memory_limit", "memory_limit"),
            ("upload_max_filesize", "upload_max_filesize"),
            ("post_max_size", "post_max_size"),
            ("max_execution_time", "max_execution_time（秒）"),
            ("pm", "进程管理 pm（dynamic/static/ondemand）"),
            ("max_children", "max_children"),
            ("start_servers", "start_servers"),
            ("min_spare_servers", "min_spare_servers"),
            ("max_spare_servers", "max_spare_servers"),
            ("max_requests", "max_requests"),
        ]
        .into_iter()
        .map(|(field, label)| kit::text_input(field, label))
        .collect();
        config.push(kit::write_card(
            "PHP 运行参数",
            "保存后自动重载 FPM；非法值会被拒绝并保留原配置。",
            kit::form(
                &format!("{API_BASE}/runtimes/{}/config", enc(runtime_id)),
                json!({ "type": "skiffRuntimeConfig", "props": { "id": runtime_id } }),
                fields,
                "保存并重载 FPM",
                Some(initial),
            ),
        ));

        let file_items: Vec<Value> = data
            .get("files")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|f| {
                        json!({
                            "key": text(f, "/key"),
                            "label": text(f, "/label"),
                            "state": if f.get("exists").and_then(Value::as_bool).unwrap_or(false) {
                                "已就绪"
                            } else {
                                "未创建"
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut file_table = kit::table(
            json!([
                { "key": "label", "label": "配置文件" },
                { "key": "state", "label": "状态" }
            ]),
            json!(file_items),
            "key",
            TableOpts {
                empty_text: Some("没有可编辑的配置文件".to_string()),
                ..Default::default()
            },
        );
        file_table["props"]["rowActions"] = json!([kit::row_button(
            "编辑",
            vec![json!({ "type": "skiffRuntimeConfigEdit",
                   "props": { "id": runtime_id, "key": "${key}" } })],
            "primary"
        )]);
        config.push(kit::card("配置文件", vec![file_table]));

        if let Some(editor) = data.get("editor").filter(|e| !e.is_null()) {
            let initial = json!({ "content": text(editor, "/content") });
            config.push(kit::write_card(
                &format!("编辑：{}", text(editor, "/label")),
                {
                    let note = text(editor, "/note");
                    if note.is_empty() {
                        "保存前会在容器内做语法校验。"
                    } else {
                        note
                    }
                },
                kit::form(
                    &format!("{API_BASE}/runtimes/{}/config-file", enc(runtime_id)),
                    json!({ "type": "skiffRuntimeFile",
                           "props": { "id": runtime_id, "key": text(editor, "/key") } }),
                    vec![kit::textarea(
                        "content",
                        &format!(
                            "{}（{}）",
                            text(editor, "/label"),
                            text(editor, "/container_path")
                        ),
                        16,
                    )],
                    "保存配置文件",
                    Some(initial),
                ),
            ));
        }

        let ext_items: Vec<Value> = data
            .get("extensions")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .map(|e| {
                        let note = text(e, "/support_note");
                        json!({
                            "key": text(e, "/key"),
                            "label": text(e, "/label"),
                            "category": text(e, "/category"),
                            "source": text(e, "/source"),
                            "state": if e.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                                "已启用"
                            } else {
                                "未启用"
                            },
                            "note": if note.is_empty() { text(e, "/description") } else { note }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut ext_table = kit::table(
            json!([
                { "key": "label", "label": "扩展" },
                { "key": "category", "label": "分类" },
                { "key": "source", "label": "来源" },
                { "key": "state", "label": "状态" },
                { "key": "note", "label": "说明" }
            ]),
            json!(ext_items),
            "key",
            TableOpts {
                empty_text: Some("还没有可用扩展".to_string()),
                ..Default::default()
            },
        );
        ext_table["props"]["rowActions"] = json!([
            kit::row_button(
                "安装",
                vec![json!({ "type": "skiffRuntimeExt",
                       "props": { "id": runtime_id, "key": "${key}", "op": "install" } })],
                "primary"
            ),
            kit::row_button(
                "卸载",
                vec![json!({ "type": "skiffRuntimeExt",
                       "props": {
                           "id": runtime_id, "key": "${key}", "op": "uninstall",
                           "confirm_text": "卸载扩展需要重启容器，绑定站点会短暂中断。确认卸载 ${key}？"
                       } })],
                "danger"
            )
        ]);
        extensions.push(kit::card("PHP 扩展", vec![ext_table]));
        extensions.push(kit::write_card(
            "按名字安装 PECL 扩展",
            "用于目录里没有的扩展；安装任务会编译并重启一次容器。",
            kit::form(
                &format!("{API_BASE}/runtimes/{}/pecl", enc(runtime_id)),
                json!({ "type": "skiffRuntimePecl", "props": { "id": runtime_id } }),
                vec![
                    kit::text_input("name", "PECL 扩展名"),
                    kit::text_input("version", "版本（可选）"),
                ],
                "安装 PECL 扩展",
                None,
            ),
        ));
    }

    if data
        .get("is_node")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let node = data.get("node").cloned().unwrap_or(Value::Null);
        let port = data.get("port").cloned().unwrap_or(Value::Null);
        let initial = json!({
            "command": text(&node, "/command"),
            "health_path": text(&node, "/health_path"),
            "node_env": text(&node, "/node_env"),
            "env": text(&node, "/env")
        });
        let mut command = kit::text_input("command", "启动命令");
        command["props"]["hint"] = json!("如 npm start；留空使用镜像默认");
        config.push(kit::write_card(
            "Node.js 配置",
            "配置变化会触发一次重建；未变化不重启。",
            kit::form(
                &format!("{API_BASE}/runtimes/{}/node-config", enc(runtime_id)),
                json!({ "type": "skiffRuntimeNode", "props": { "id": runtime_id } }),
                vec![
                    command,
                    kit::text_input("health_path", "健康检查路径（如 /healthz）"),
                    kit::text_input("node_env", "NODE_ENV"),
                    kit::textarea("env", "额外环境变量（每行 KEY=VALUE）", 6),
                ],
                "保存 Node.js 配置",
                Some(initial),
            ),
        ));
        if port
            .get("editable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let range = text(&port, "/range");
            let mut field = kit::text_input(
                "port",
                &format!(
                    "固定端口{}",
                    if range.is_empty() {
                        String::new()
                    } else {
                        format!("（{range}）")
                    }
                ),
            );
            field["props"]["rules"] = json!({
                "pattern": "^[0-9]{1,5}$",
                "messages": { "pattern": "端口需为 1–65535 的整数" }
            });
            let port_notice = if site_count > 0 {
                format!("当前有 {site_count} 个站点绑定，改端口会重写全部站点反代并重建容器。")
            } else {
                "端口由内核分配，修改后会重建容器。".to_string()
            };
            config.push(kit::write_card(
                "端口",
                &port_notice,
                kit::form(
                    &format!("{API_BASE}/runtimes/{}/port", enc(runtime_id)),
                    json!({ "type": "skiffRuntimePort", "props": { "id": runtime_id } }),
                    vec![field],
                    "保存端口",
                    Some(json!({ "port": num(&port, "/value").to_string() })),
                ),
            ));
        }
    }

    let site_items: Vec<Value> = data
        .get("sites")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|s| {
                    json!({
                        "domain": text(s, "/domain"),
                        "site_type": text(s, "/site_type"),
                        "status": text(s, "/status")
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if !site_items.is_empty() {
        let mut site_table = kit::table(
            json!([
                { "key": "domain", "label": "站点" },
                { "key": "site_type", "label": "类型" },
                { "key": "status", "label": "状态" }
            ]),
            json!(site_items),
            "domain",
            TableOpts::default(),
        );
        site_table["props"]["rowActions"] = json!([kit::row_button(
            "解绑",
            vec![json!({ "type": "skiffRuntimeUnbind",
                   "props": { "id": runtime_id, "domain": "${domain}" } })],
            "danger"
        )]);
        sites.push(kit::card("绑定站点", vec![site_table]));
    }

    manage.push(kit::write_card(
        "克隆",
        "复制当前配置与新实例；端口由内核重新分配，名称可留空（默认追加 -copy）。",
        kit::form(
            &format!("{API_BASE}/runtimes/{}/clone", enc(runtime_id)),
            json!({ "type": "skiffRuntimeClone", "props": { "id": runtime_id } }),
            vec![kit::text_input("name", "新实例名（可留空）")],
            "克隆并安装",
            None,
        ),
    ));
    manage.push(kit::card(
        "说明",
        vec![kit::text_line(
            "安装、扩展与配置文件编辑都在后台任务中执行；失败原因会写审计并显示在日志页。",
        )],
    ));

    let subtitle = match view {
        "config" => "运行参数与配置文件；保存前在容器内做语法校验",
        "extensions" => "已装扩展与 PECL 安装；装完会重启一次容器",
        "sites" => "绑定到该环境的站点；解绑后站点需重新指定运行环境",
        "manage" => "克隆新实例；端口由内核重新分配",
        _ => "实例状态与运维动作；写操作需要管理员权限",
    };
    let body = match view {
        "config" => config,
        "extensions" => extensions,
        "sites" => sites,
        "manage" => manage,
        _ => overview,
    };
    kit::page(
        if name.is_empty() { runtime_id } else { name },
        subtitle,
        body,
    )
}

/// 运行环境日志：与容器日志同形态（尾部快照 + 刷新）。
pub fn runtime_logs_schema(runtime_id: &str, data: &Value) -> Value {
    let raw = text(data, "/text");
    let lines: Vec<&str> = raw.split('\n').filter(|line| !line.is_empty()).collect();
    let mut body = vec![kit::flex(
        8,
        vec![
            kit::link(
                "返回运行环境详情",
                format!("${{prefix}}/sites/runtimes/{}", enc(runtime_id)),
            ),
            kit::action_button("刷新", vec![json!({ "type": "skiffReload" })], "text"),
        ],
    )];
    if lines.is_empty() {
        body.push(kit::alert("info", "", "暂无日志。"));
    } else {
        body.push(kit::code_block(&lines.join("\n"), 560));
    }
    body.push(kit::small_text(format!(
        "共 {} 行 · 取尾部 {} 行",
        lines.len(),
        num(data, "/tail")
    )));
    kit::page(
        format!("{} · 运行环境日志", runtime_id),
        "尾部快照，不做 SSE 实时流",
        body,
    )
}

fn runtime_op(text: &str, op: &str, runtime_id: &str, confirm: &str) -> Value {
    kit::action_button(
        text,
        vec![json!({ "type": "skiffRuntimeAction",
               "props": { "id": runtime_id, "op": op, "confirm_text": confirm } })],
        if op == "delete" { "text" } else { "tonal" },
    )
}

fn enc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
