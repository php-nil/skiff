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
                "还没有运行环境，点击右上角「新建运行环境」安装后即可绑定站点".to_string(),
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

    let body = vec![kit::small_text(format!("共 {total} 个运行环境")), table];

    kit::page_with_actions(
        "运行环境",
        "镜像、实例与安装入口；未安装镜像会先拉取",
        vec![
            // 清单有 10 分钟缓存；急着装刚发的补丁时点它同步拉一次。
            kit::action_button(
                "刷新版本清单",
                vec![json!({ "type": "skiffRuntimeVersions", "props": {} })],
                "text",
            ),
            create_dialog(catalog),
        ],
        body,
    )
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

    // 版本下拉按类型各一份：选项来自镜像服务（`runtime-index/<kind>.json`），
    // 拉不到时回退内置副本（PHP）或目录大版本——清单少几版必须能解释，不能静默。
    let mut fields = vec![json!({
        "type": "select",
        "props": { "name": "kind", "label": "类型", "items": kinds }
    })];
    let mut initial = serde_json::Map::new();
    initial.insert("kind".into(), json!(default_kind));
    let mut notes: Vec<String> = Vec::new();
    let mut index_dir = String::new();
    let kind_rows = catalog
        .pointer("/kinds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for entry in &kind_rows {
        let key = text(entry, "/key");
        if key.is_empty() {
            continue;
        }
        let label = text(entry, "/label");
        let versions = kind_versions(catalog, key, label);
        if index_dir.is_empty() {
            index_dir = index_url_dir(catalog, key);
        }
        notes.push(versions.note);
        if versions.items.is_empty() {
            // 目录里没有该类型的版本（理论上不会）：只留「自定义版本」兜底。
            continue;
        }
        fields.push(json!({
            "type": "select",
            "visibleOn": format!("${{kind === '{key}'}}"),
            "props": {
                "name": format!("version_{key}"),
                "label": format!("版本（{label}）"),
                "items": versions.items,
                "hint": "选项由镜像服务发布（未发布时用目录里的大版本）；清单里没有的版本，填下方「自定义版本」。",
            }
        }));
        initial.insert(format!("version_{key}"), json!(versions.default));
    }
    fields.push(json!({
        "type": "input-text",
        "props": {
            "name": "version_custom",
            "label": "自定义版本（可选）",
            "hint": "清单里没有？填完整版本号（如 8.5.11 / 24.4.0 / 1.99.1）。填了就以它为准，优先于上方选择。",
        }
    }));
    fields.push(kit::text_input(
        "name",
        "实例名（可选，默认按类型自动生成）",
    ));
    fields.push(json!({
        "type": "select",
        "props": {
            "name": "apk_mirror",
            "label": "软件包源（仅 PHP 生效）",
            "hint": "自动 = 安装时在容器内实测各源吞吐后择快；选错源会让装扩展直接失败（不只是慢）。",
            "items": mirrors
        }
    }));
    initial.insert("apk_mirror".into(), json!(default_mirror));

    // 固定路径要写进提示：用户看到「清单是哪来的」，出问题也不必猜地址。
    let files: Vec<&str> = skiff_app::runtime_index::KINDS
        .iter()
        .filter_map(|kind| skiff_app::runtime_index::index_file(kind))
        .collect();
    let path_note = if index_dir.is_empty() {
        String::new()
    } else {
        format!("清单固定路径：{index_dir}/{}。", files.join(" / "))
    };
    let notice = format!(
        "版本清单来源：{}。{path_note}PHP 会按所选版本拉取并安装默认扩展；Node.js / Rust 按所选版本拉镜像启动。安装进度在列表状态列与详情日志中显示。",
        notes.join("；")
    );
    kit::dialog_form_button(kit::DialogForm {
        text: "新建运行环境",
        variant: "flat",
        title: "新建运行环境",
        width: 640,
        submit: json!({ "type": "skiffRuntimeCreate" }),
        fields,
        initial: Some(Value::Object(initial)),
        submit_text: "创建并安装",
        notice: &notice,
    })
}

/// 一个类型的版本下拉：选项、预选值与来源说明（都内联在弹窗里，不额外请求）。
struct KindVersions {
    items: Vec<Value>,
    default: String,
    note: String,
}

/// 从指针处取字符串数组（缺字段/类型不符都退化成空列表）。
fn strings(value: Option<&Value>, pointer: &str) -> Vec<String> {
    value
        .and_then(|v| v.pointer(pointer))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 目录里该类型的大版本（清单为空/未配置镜像时的回退来源）。
fn catalog_versions(catalog: &Value, kind: &str) -> Vec<String> {
    strings(
        catalog
            .pointer("/kinds")
            .and_then(Value::as_array)
            .and_then(|list| list.iter().find(|k| text(k, "/key") == kind)),
        "/versions",
    )
}

/// 清单的固定目录（把 `<kind>.json` 去掉），只用于提示文案；没有镜像基址时为空。
fn index_url_dir(catalog: &Value, kind: &str) -> String {
    let url = text(catalog, &format!("/version_indexes/{kind}/url"));
    url.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

fn kind_versions(catalog: &Value, kind: &str, label: &str) -> KindVersions {
    let index = catalog.pointer(&format!("/version_indexes/{kind}"));
    let source = index
        .and_then(|v| v.pointer("/source"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let error = index
        .and_then(|v| v.pointer("/error"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let pending = index
        .and_then(|v| v.pointer("/pending"))
        .and_then(Value::as_bool)
        == Some(true);
    let remote = index
        .and_then(|v| v.pointer("/remote"))
        .and_then(Value::as_bool)
        == Some(true);
    let eol = strings(index, "/eol");
    let mut versions = strings(index, "/versions");
    if versions.is_empty() {
        versions = catalog_versions(catalog, kind);
    }
    // 正常版本在前、已 EOL 在后，组内保持清单的降序：默认选项永远是能长期用的那个。
    versions.sort_by_key(|v| eol.iter().any(|e| e == v));
    let items: Vec<Value> = versions
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let is_eol = eol.iter().any(|e| e == v);
            json!({
                "title": if is_eol {
                    format!("{v}（已 EOL）")
                } else if i == 0 {
                    format!("{v}（最新）")
                } else {
                    v.clone()
                },
                "value": v,
            })
        })
        .collect();
    // 预选：镜像服务声明的 default 优先（且不能被 EOL 覆盖），否则取首个未 EOL 的版本。
    let marked = index
        .and_then(|v| v.pointer("/default"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let default = if !marked.is_empty()
        && versions.iter().any(|v| v == marked)
        && !eol.iter().any(|v| v == marked)
    {
        marked.to_string()
    } else {
        versions.first().cloned().unwrap_or_default()
    };
    let fallback = if source == "builtin" {
        "内置副本"
    } else {
        "目录大版本"
    };
    let note = if source == "registry" {
        format!("{label} 镜像服务 · {} 个版本", versions.len())
    } else if !error.is_empty() {
        format!("{label} {fallback}：{error}")
    } else if pending && remote {
        format!("{label} {fallback}（点右上「刷新版本清单」取最新）")
    } else {
        format!("{label} {fallback}")
    };
    KindVersions {
        items,
        default,
        note,
    }
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
            "保存后会重启 PHP 容器生效，同环境站点在重启期间（约 3 秒）返回 503；非法值会被拒绝并保留原配置。",
            kit::form(
                json!({ "type": "skiffRuntimeConfig", "props": { "id": runtime_id } }),
                fields,
                "保存并重启 PHP 容器",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 目录 JSON（与 `catalog_json` 同形）：`php` 那份由测试参数控制，其余走目录回退。
    fn catalog(versions: &[&str], source: &str, error: &str, pending: bool) -> Value {
        json!({
            "kinds": [
                { "key": "php", "label": "PHP", "versions": ["8.5", "8.4", "8.3"] },
                { "key": "node", "label": "Node.js", "versions": ["24", "22", "20"] },
                { "key": "rust", "label": "Rust", "versions": ["1.99", "1.98"] }
            ],
            "default_php_version": "8.5",
            "version_indexes": {
                "php": {
                    "kind": "php",
                    "source": source,
                    "versions": versions,
                    "error": error,
                    "pending": pending
                },
                "node": {
                    "kind": "node",
                    "source": "catalog",
                    "versions": ["24", "22", "20"],
                    "error": "",
                    "pending": false
                },
                "rust": {
                    "kind": "rust",
                    "source": "catalog",
                    "versions": ["1.99", "1.98"],
                    "error": "",
                    "pending": false
                }
            },
            "apk_mirrors": [],
            "apk_mirror_current": { "fixed": false, "base": "", "label": "" }
        })
    }

    /// 深搜出所有带 `props.name` 的字段节点（弹窗 body 里嵌套多层）。
    fn fields(node: &Value, out: &mut Vec<Value>) {
        if node
            .pointer("/props/name")
            .and_then(Value::as_str)
            .is_some_and(|n| !n.is_empty())
        {
            out.push(node.clone());
        }
        match node {
            Value::Object(map) => {
                for child in map.values() {
                    fields(child, out);
                }
            }
            Value::Array(rows) => {
                for child in rows {
                    fields(child, out);
                }
            }
            _ => {}
        }
    }

    fn dialog_fields(catalog: &Value) -> Vec<Value> {
        let page = runtimes_schema(catalog, 0);
        let mut out = Vec::new();
        fields(&page, &mut out);
        out
    }

    fn find<'a>(list: &'a [Value], name: &str) -> &'a Value {
        list.iter()
            .find(|f| f.pointer("/props/name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("弹窗缺少字段 {name}"))
    }

    /// 镜像服务的清单直接变成下拉选项，第一项标注「最新」并默认选中。
    #[test]
    fn php_versions_come_from_registry_index() {
        let cat = catalog(&["8.5.13", "8.5.12", "8.4.28"], "registry", "", false);
        let list = dialog_fields(&cat);
        let field = find(&list, "version_php");
        assert_eq!(field["type"], "select");
        assert_eq!(field["visibleOn"], "${kind === 'php'}");
        let items = field["props"]["items"].as_array().expect("必须有选项");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["value"], "8.5.13");
        assert!(
            items[0]["title"].as_str().unwrap_or("").contains("最新"),
            "第一项要标出最新，否则用户不知道选哪个"
        );
        assert_eq!(items[1]["title"], "8.5.12");
    }

    /// Node.js / Rust 也走下拉，但各取自己那份清单：PHP 的补丁号不得串过去。
    #[test]
    fn node_and_rust_have_their_own_dropdowns() {
        let mut cat = catalog(&["8.5.13"], "registry", "", false);
        cat["version_indexes"]["node"]["source"] = json!("registry");
        cat["version_indexes"]["node"]["versions"] = json!(["24.4.0", "24.3.0", "22.5.1"]);
        let list = dialog_fields(&cat);

        let node = find(&list, "version_node");
        assert_eq!(node["type"], "select");
        assert_eq!(node["visibleOn"], "${kind === 'node'}");
        let values: Vec<&str> = node["props"]["items"]
            .as_array()
            .expect("node 必须有选项")
            .iter()
            .filter_map(|i| i["value"].as_str())
            .collect();
        assert_eq!(values, vec!["24.4.0", "24.3.0", "22.5.1"]);

        let rust = find(&list, "version_rust");
        assert_eq!(rust["visibleOn"], "${kind === 'rust'}");
        let values: Vec<&str> = rust["props"]["items"]
            .as_array()
            .expect("rust 必须有选项")
            .iter()
            .filter_map(|i| i["value"].as_str())
            .collect();
        assert_eq!(values, vec!["1.99", "1.98"], "未发布清单时回退目录大版本");

        // 初始值按类型各给一份，切换类型不需要用户重选。
        let page = runtimes_schema(&cat, 0).to_string();
        assert!(page.contains("\"version_node\":\"24.4.0\""), "{page}");
        assert!(page.contains("\"version_rust\":\"1.99\""), "{page}");
    }

    /// 清单以外的手填入口必须一直在（镜像服务不可达时是唯一出路）。
    #[test]
    fn custom_version_field_always_available() {
        let list = dialog_fields(&catalog(&["8.5.13"], "registry", "", false));
        let custom = find(&list, "version_custom");
        assert!(
            custom.get("visibleOn").is_none(),
            "自定义版本对任何类型都可见"
        );
        assert!(custom["props"]["hint"]
            .as_str()
            .unwrap_or("")
            .contains("优先"));
    }

    /// 清单为空（未预热/被清空）时回退目录大版本，并说明来源。
    #[test]
    fn empty_index_falls_back_to_catalog_versions() {
        let cat = catalog(&[], "", "", true);
        let list = dialog_fields(&cat);
        let items = find(&list, "version_php")["props"]["items"]
            .as_array()
            .expect("回退也要有选项")
            .clone();
        let values: Vec<&str> = items.iter().filter_map(|i| i["value"].as_str()).collect();
        assert_eq!(values, vec!["8.5", "8.4", "8.3"], "回退到目录大版本");
    }

    /// 拉取失败要把原因显示在弹窗提示里，不能静默少几版。
    #[test]
    fn index_error_is_surfaced_in_notice() {
        let cat = catalog(
            &["8.5", "8.4", "8.3"],
            "catalog",
            "清单获取失败：连接超时",
            false,
        );
        let page = runtimes_schema(&cat, 0);
        let text = page.to_string();
        assert!(
            text.contains("清单获取失败：连接超时"),
            "失败原因必须出现在页面上"
        );
    }

    /// 页头给出刷新入口：清单有缓存，急着装新补丁时用户需要主动拉一次。
    #[test]
    fn header_has_versions_refresh_action() {
        let page = runtimes_schema(&catalog(&["8.5.13"], "registry", "", false), 0);
        assert!(
            page.to_string().contains("\"skiffRuntimeVersions\""),
            "页头缺少「刷新版本清单」动作"
        );
    }

    /// 已 EOL 的版本标出来并排到最后：它还能装，但不该是默认/首选项。
    #[test]
    fn eol_versions_are_marked_and_sorted_last() {
        let mut cat = catalog(&["8.5.13", "8.4.28", "8.3.37"], "registry", "", false);
        cat["version_indexes"]["php"]["eol"] = json!(["8.3.37"]);
        let list = dialog_fields(&cat);
        let items = find(&list, "version_php")["props"]["items"]
            .as_array()
            .expect("必须有选项")
            .clone();
        let values: Vec<&str> = items.iter().filter_map(|i| i["value"].as_str()).collect();
        assert_eq!(values, vec!["8.5.13", "8.4.28", "8.3.37"]);
        assert!(items[0]["title"].as_str().unwrap_or("").contains("最新"));
        assert!(
            items[2]["title"].as_str().unwrap_or("").contains("已 EOL"),
            "EOL 必须写明，否则用户以为它还能长期用"
        );
    }

    /// 预选跟随清单的 default（镜像服务说了算），而不是永远取第一项。
    #[test]
    fn default_version_follows_index_default() {
        let mut cat = catalog(&["8.5.13", "8.5.12"], "registry", "", false);
        cat["version_indexes"]["php"]["default"] = json!("8.5.12");
        let page = runtimes_schema(&cat, 0).to_string();
        assert!(
            page.contains("\"version_php\":\"8.5.12\""),
            "预选没跟随清单 default：{page}"
        );
    }

    /// 固定路径必须显示在提示里：出问题不必猜清单来自哪里。
    #[test]
    fn notice_shows_fixed_index_url() {
        let mut cat = catalog(&["8.5.13"], "registry", "", false);
        cat["version_indexes"]["php"]["url"] =
            json!("http://10.66.52.7:8080/channels/stable/runtime-index/php.json");
        let page = runtimes_schema(&cat, 0).to_string();
        assert!(
            page.contains("channels/stable/runtime-index"),
            "提示里应给出清单的固定路径：{page}"
        );
        assert!(
            page.contains("php.json / node.json / rust.json"),
            "提示里应说明按类型取哪几个文件：{page}"
        );
    }
}
