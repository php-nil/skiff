//! 文件管理页族：目录浏览 / 代码查看器 / 回收站 / 配额。
//!
//! 目录行模板是纯静态结构（`static/budding/pages/_file-row.json`），这里只注入
//! 数据与当前路径；行内的弹窗按钮沿用宿主同名动作，弹窗注册由壳脚本统一补录。

use super::schema_kit::{self as kit, TableOpts, API_BASE};
use serde_json::{json, Value};

/// 行模板随二进制编译进来：它不随请求变化，没必要每页重新拼装。
const FILE_ROW: &str = include_str!("../../../static/budding/pages/_file-row.json");

fn text<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, pointer: &str) -> bool {
    v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false)
}

fn rows<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// 人类可读体积：1024 进制，超过 10 只保留整数位。
pub fn human_size(bytes: i64) -> String {
    let mut n = bytes.max(0) as f64;
    if n < 1024.0 {
        return format!("{} B", bytes.max(0));
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut idx = 0usize;
    loop {
        n /= 1024.0;
        if n < 1024.0 || idx + 1 >= units.len() {
            break;
        }
        idx += 1;
    }
    if n >= 10.0 {
        format!("{} {}", n.round() as i64, units[idx])
    } else {
        format!("{} {}", (n * 10.0).round() / 10.0, units[idx])
    }
}

/// `YYYY-MM-DD HH:MM`（本地时区）；解析失败时原样返回。
fn minute_label(value: &str) -> String {
    if value.is_empty() {
        return "—".to_string();
    }
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => value.to_string(),
    }
}

fn mode_label(mode: Option<i64>) -> String {
    match mode {
        Some(m) => format!("0{:o}", m),
        None => "—".to_string(),
    }
}

fn row_template() -> Value {
    serde_json::from_str(FILE_ROW)
        .unwrap_or_else(|_| json!({ "type": "text", "props": { "text": "行模板缺失" } }))
}

/// 目录页：位置卡（面包屑 + 根）+ 目录内容 + 操作工具栏。
pub fn files_schema(data: &Value, can_admin: bool) -> Value {
    let path = text(data, "/path");
    let mut body = Vec::new();
    if flag(data, "/truncated") {
        body.push(kit::alert(
            "warning",
            "",
            "目录条目超过单页上限（1000 条），当前只显示前 1000 条；请进入子目录继续浏览。",
        ));
    }

    let mut crumbs = vec![kit::link(
        "根目录",
        format!("${{prefix}}/files?path={}", enc("/")),
    )];
    let mut acc = String::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(seg);
        crumbs.push(kit::text_line("/"));
        crumbs.push(kit::link(
            seg,
            format!("${{prefix}}/files?path={}", enc(&acc)),
        ));
    }
    let roots = rows(data, "/roots");
    let roots_node = if roots.is_empty() {
        kit::text_line("可直接在面包屑路径内浏览。")
    } else {
        kit::flex(
            12,
            roots
                .iter()
                .map(|r| {
                    kit::link(
                        text(r, "/label"),
                        format!("${{prefix}}/files?path={}", enc(text(r, "/path"))),
                    )
                })
                .collect(),
        )
    };
    body.push(kit::card("位置", vec![kit::flex(4, crumbs), roots_node]));

    if can_admin {
        body.push(kit::flex(
            12,
            vec![
                kit::link("回收站", "${prefix}/files/trash"),
                kit::link("配额管理", "${prefix}/files/quotas"),
            ],
        ));
    }

    let items: Vec<Value> = rows(data, "/entries")
        .iter()
        .map(|e| {
            let dir = flag(e, "/dir");
            let name = text(e, "/name");
            let size = e.pointer("/size").and_then(Value::as_i64).unwrap_or(0);
            let mode = e.pointer("/mode").and_then(Value::as_i64);
            let owner = text(e, "/owner");
            json!({
                "name": text(e, "/name"),
                "name_label": if dir { format!("{name}/") } else { name.to_string() },
                "path": text(e, "/path"),
                "dir": dir,
                "size_label": if dir { "—".to_string() } else { human_size(size) },
                "mtime_label": minute_label(text(e, "/mtime")),
                "mode_label": mode_label(mode),
                "owner_label": if owner.is_empty() { "—".to_string() } else { owner.to_string() },
                "can_extract": !dir
                    && (name.ends_with(".zip") || name.ends_with(".tar.gz") || name.ends_with(".tgz"))
            })
        })
        .collect();
    let count = items.len();
    body.push(json!({
        "type": "virtual-list",
        "props": {
            "items": items,
            "itemKey": "path",
            "itemHeight": 52,
            "height": 560,
            "emptyText": "目录为空",
            "ariaLabel": "目录条目"
        },
        "content": { "body": [ row_template() ] }
    }));
    body.push(kit::small_text(format!(
        "共 {count} 项；目录在前、按名称排序。删除进回收站，可在回收站恢复。"
    )));

    // 工具栏：三个按钮分别触发弹窗，避免内联表单挤爆首屏。
    let name_rule = json!({
        "required": true,
        "pattern": "^[^/\\\\]+$",
        "messages": { "required": "请输入名称", "pattern": "名称不能包含 / 或 \\" }
    });
    let mkdir = json!({
        "type": "button",
        "props": { "text": "新建目录", "variant": "tonal" },
        "action": {
            "type": "dialog",
            "props": { "title": "新建目录", "width": 480 },
            "content": {
                "body": [{
                    "type": "form",
                    "onEvent": { "submit": [
                        { "type": "skiffFileMkdir", "props": { "dir": path } }
                    ] },
                    "props": { "submitMode": "event" },
                    "body": [{
                        "type": "input-text",
                        "props": {
                            "name": "name",
                            "label": "目录名（将创建在当前目录下）",
                            "required": true,
                            "rules": name_rule
                        }
                    }]
                }],
                "footer": [
                    { "type": "button", "props": { "text": "重置", "variant": "text" }, "action": { "type": "reset" } },
                    { "type": "button", "props": { "text": "取消", "variant": "outlined" }, "action": { "type": "close" } },
                    { "type": "button", "props": { "text": "创建", "color": "primary" }, "action": { "type": "submit" } }
                ]
            }
        }
    });

    let upload = json!({
        "type": "button",
        "props": { "text": "上传文件", "variant": "tonal" },
        "action": {
            "type": "dialog",
            "props": { "title": "上传文件", "width": 560 },
            "content": {
                "body": [
                    kit::alert(
                        "info",
                        "",
                        format!("将上传到当前目录 {path}；大文件自动分片 256 KiB。")
                    ),
                    {
                        "type": "file",
                        "props": {
                            "name": "file",
                            "label": "选择文件",
                            "multiple": false,
                            "uploadField": "file",
                            "uploadApi": {
                                "method": "POST",
                                "url": format!("{API_BASE}/files/upload?dir={}", enc(path))
                            },
                            "uploadProgressTarget": "uploadProgress"
                        }
                    }
                ],
                "footer": [
                    { "type": "button", "props": { "text": "关闭", "variant": "outlined" },
                      "action": { "type": "close" } }
                ]
            }
        }
    });

    let write = json!({
        "type": "button",
        "props": { "text": "新建文件", "variant": "tonal" },
        "action": {
            "type": "dialog",
            "props": { "title": "新建文件", "width": 640 },
            "content": {
                "body": [{
                    "type": "form",
                    "onEvent": { "submit": [
                        { "type": "skiffFileWrite", "props": { "dir": path } }
                    ] },
                    "props": { "submitMode": "event" },
                    "body": [
                        {
                            "type": "input-text",
                            "props": {
                                "name": "name",
                                "label": "文件名（在当前目录创建或覆盖）",
                                "required": true,
                                "rules": {
                                    "required": true,
                                    "pattern": "^[^/\\\\]+$",
                                    "messages": { "required": "请输入文件名", "pattern": "文件名不能包含 / 或 \\" }
                                }
                            }
                        },
                        { "type": "textarea", "props": { "name": "content", "label": "文件内容", "rows": 8 } }
                    ]
                }],
                "footer": [
                    { "type": "button", "props": { "text": "重置", "variant": "text" }, "action": { "type": "reset" } },
                    { "type": "button", "props": { "text": "取消", "variant": "outlined" }, "action": { "type": "close" } },
                    { "type": "button", "props": { "text": "保存", "color": "primary" }, "action": { "type": "submit" } }
                ]
            }
        }
    });

    body.push(kit::card(
        "操作",
        vec![kit::flex(12, vec![mkdir, upload, write])],
    ));

    kit::page("文件管理", "平铺目录 + 面包屑（不使用树形）", body)
}

/// 代码查看器：只读高亮 + 可编辑副本（二进制与截断文件不给编辑入口）。
pub fn file_view_schema(data: &Value) -> Value {
    let raw = text(data, "/text");
    let binary = raw.contains('\u{0}');
    let path = text(data, "/path");
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
    let parent = match path.rfind(['/', '\\']) {
        Some(idx) if idx > 0 => path[..idx].to_string(),
        Some(_) => "/".to_string(),
        None => "/".to_string(),
    };

    let mut body = vec![kit::flex(
        12,
        vec![
            kit::link(
                "返回目录",
                format!("${{prefix}}/files?path={}", enc(&parent)),
            ),
            kit::action_button(
                "下载",
                vec![json!({ "type": "skiffFileDownload", "props": { "path": path } })],
                "text",
            ),
        ],
    )];
    if binary {
        body.push(kit::alert(
            "warning",
            "",
            "该文件疑似二进制（含 NUL 字节），未渲染内容。",
        ));
    }
    if flag(data, "/truncated") {
        body.push(kit::alert(
            "info",
            "",
            format!(
                "文件超过单次读取上限（{} 字节），当前只显示开头部分；可下载查看完整内容。",
                data.pointer("/max_edit_bytes")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            ),
        ));
    }
    body.push(json!({
        "type": "code",
        "props": {
            "modelValue": if binary { "" } else { raw },
            "language": language_of(&name),
            "readonly": true,
            "height": 560,
            "lineNumbers": true,
            "wrap": false,
            "copyable": true,
            "ariaLabel": name
        }
    }));
    if !binary && !flag(data, "/truncated") {
        let mut content = kit::textarea("content", "文件内容", 20);
        content["props"]["required"] = json!(true);
        content["props"]["rules"] =
            json!({ "required": true, "messages": { "required": "内容不能为空" } });
        body.push(kit::write_card(
            "编辑内容",
            "保存会覆盖原文件；权限与路径约束仍在服务端执行。",
            kit::form(
                json!({ "type": "skiffFileWrite", "props": { "path": path } }),
                vec![content],
                "保存文件",
                Some(json!({ "content": raw })),
            ),
        ));
    }
    body.push(kit::small_text(format!(
        "大小 {}{} · 只读 · 高亮由 @budding/code Pack 在前端完成",
        human_size(data.pointer("/size").and_then(Value::as_i64).unwrap_or(0)),
        if flag(data, "/truncated") {
            "（已截断）"
        } else {
            ""
        }
    )));

    kit::page(
        format!("{name} · 代码查看器"),
        "只读，纯前端高亮（不引 WASM）",
        body,
    )
}

/// 回收站：保留期说明 + 恢复 / 彻底删除。
pub fn trash_schema(data: &Value) -> Value {
    let retention = data
        .pointer("/retention_days")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut body = vec![
        kit::alert(
            "info",
            "",
            format!(
                "删除的文件先移入回收站，默认保留 {retention} 天，到期由内置任务 trash-prune 彻底删除。数据根、plugins/、官方目录与 skiff.db 在删除和恢复路径上都会被硬拒。"
            ),
        ),
        kit::flex(
            8,
            vec![
                kit::action_button("刷新", vec![json!({ "type": "skiffReload" })], "secondary"),
                kit::action_button(
                    "清空回收站",
                    vec![
                        json!({
                            "type": "confirm",
                            "props": {
                                "title": "清空回收站",
                                "content": "将永久删除全部条目，且不会进入任何备份。确认清空？",
                                "color": "error"
                            }
                        }),
                        trash_request("clear", json!({}), "已清空回收站", "清空回收站失败"),
                        reload_page()
                    ],
                    "danger",
                ),
            ],
        ),
    ];

    let items: Vec<Value> = rows(data, "/items")
        .iter()
        .map(|item| {
            json!({
                "id": item.get("id").cloned().unwrap_or(Value::Null),
                "original_path": text(item, "/original_path"),
                "path_label": text(item, "/original_path"),
                "kind_label": match text(item, "/kind") {
                    "dir" => "目录",
                    "symlink" => "链接",
                    _ => "文件",
                },
                "size_label": human_size(item.pointer("/size").and_then(Value::as_i64).unwrap_or(0)),
                "files_label": item.pointer("/files").and_then(Value::as_i64).unwrap_or(0).to_string(),
                "deleted_label": minute_label(text(item, "/deleted_at")),
                "actor_label": format!("{}:{}", text(item, "/actor_type"), text(item, "/actor_id")),
                "expires_label": minute_label(text(item, "/expires_at"))
            })
        })
        .collect();

    if items.is_empty() {
        body.push(
            json!({ "type": "card", "content": { "body": [kit::text_line("回收站为空。")] } }),
        );
    } else {
        let mut table = kit::table(
            json!([
                { "key": "path_label", "label": "原路径" },
                { "key": "kind_label", "label": "类型" },
                { "key": "size_label", "label": "大小" },
                { "key": "files_label", "label": "文件数" },
                { "key": "deleted_label", "label": "删除时间" },
                { "key": "actor_label", "label": "操作者" },
                { "key": "expires_label", "label": "到期时间" }
            ]),
            json!(items),
            "id",
            TableOpts::default(),
        );
        table["props"]["rowActions"] = json!([
            kit::row_button(
                "恢复",
                vec![
                    // 动作组：先尝试 overwrite=false 恢复；失败时 onError 接管，
                    // 弹出覆盖确认后以 overwrite=true 重试。
                    json!({
                        "actions": [trash_request(
                            "restore",
                            json!({ "id": "${id}", "overwrite": false }),
                            "",
                            ""
                        )],
                        "onError": [
                            json!({
                                "type": "confirm",
                                "props": {
                                    "title": "恢复失败",
                                    "content": "目标已存在或恢复失败。是否覆盖现有对象后重试？",
                                    "color": "error"
                                }
                            }),
                            trash_request(
                                "restore",
                                json!({ "id": "${id}", "overwrite": true }),
                                "已覆盖恢复",
                                "恢复失败"
                            )
                        ]
                    }),
                    reload_page()
                ],
                "primary"
            ),
            kit::row_button(
                "彻底删除",
                vec![
                    json!({
                        "type": "confirm",
                        "props": {
                            "title": "彻底删除",
                            "content": "确认彻底删除 ${path_label}？不可恢复。",
                            "color": "error"
                        }
                    }),
                    trash_request(
                        "purge",
                        json!({ "id": "${id}" }),
                        "已彻底删除",
                        "彻底删除失败"
                    ),
                    reload_page()
                ],
                "danger"
            )
        ]);
        body.push(table);
    }

    kit::page("回收站", "恢复与彻底删除复用既有 files JSON 接口", body)
}

/// 配额管理：设置、重扫与取消。
pub fn quotas_schema(data: &Value) -> Value {
    let quotas: Vec<Value> = rows(data, "/quotas")
        .iter()
        .map(|q| {
            let max_bytes = q.pointer("/max_bytes").and_then(Value::as_i64);
            let max_files = q.pointer("/max_files").and_then(Value::as_i64);
            json!({
                "root": text(q, "/root"),
                "bytes_label": format!(
                    "{} / {}",
                    human_size(q.pointer("/used_bytes").and_then(Value::as_i64).unwrap_or(0)),
                    match max_bytes {
                        Some(v) => human_size(v),
                        None => "不限".to_string(),
                    }
                ),
                "files_label": format!(
                    "{} / {}",
                    q.pointer("/used_files").and_then(Value::as_i64).unwrap_or(0),
                    match max_files {
                        Some(v) => v.to_string(),
                        None => "不限".to_string(),
                    }
                ),
                "truncated_label": if flag(q, "/truncated") { "不完整（仅下界）" } else { "" },
                "updated_label": minute_label(text(q, "/updated_at"))
            })
        })
        .collect();

    let mut body = vec![
        kit::alert(
            "info",
            "",
            "已用量是缓存计数：每次成功操作后增量修正，外部改动可能造成偏差；「重新统计」会递归扫描整棵目录树，成本为 O(条目数)。",
        ),
        kit::write_card(
            "设置配额",
            "容量上限与文件数上限至少填一项；留空表示不限。重复保存同一路径会覆盖原配额。",
            kit::form(
                json!({ "type": "skiffQuotaWrite", "props": { "op": "set" } }),
                vec![
                    kit::required_input("path", "目录树（绝对路径）", "请输入绝对路径"),
                    kit::text_input("max_bytes", "容量上限（字节，留空不限）"),
                    kit::text_input("max_files", "文件数上限（留空不限）"),
                ],
                "保存配额",
                None,
            ),
        ),
    ];

    if quotas.is_empty() {
        body.push(json!({ "type": "card", "content": { "body": [kit::text_line("尚未设置目录配额。")] } }));
    } else {
        let mut table = kit::table(
            json!([
                { "key": "root", "label": "目录树" },
                { "key": "bytes_label", "label": "已用容量 / 上限" },
                { "key": "files_label", "label": "已用文件数 / 上限" },
                { "key": "truncated_label", "label": "统计" },
                { "key": "updated_label", "label": "最近统计" }
            ]),
            json!(quotas),
            "root",
            TableOpts::default(),
        );
        table["props"]["rowActions"] = json!([
            kit::row_button(
                "重新统计",
                vec![
                    json!({ "type": "skiffQuotaWrite", "props": { "op": "scan", "path": "${root}" } })
                ],
                "primary"
            ),
            kit::row_button(
                "取消配额",
                vec![
                    json!({ "type": "skiffQuotaWrite", "props": { "op": "cancel", "path": "${root}" } })
                ],
                "danger"
            )
        ]);
        body.push(table);
    }

    kit::page(
        "配额管理",
        "设置、重扫与取消均复用既有 files JSON 接口",
        body,
    )
}

/// 回收站写请求：端点挂在入口前缀下（`${prefix}/files/trash/*`），
/// CSRF 由平台请求适配器统一补。
fn trash_request(op: &str, data: Value, success: &str, error: &str) -> Value {
    let mut props = json!({
        "api": {
            "method": "POST",
            "url": format!("${{prefix}}/files/trash/{op}"),
            "data": data
        }
    });
    if !success.is_empty() {
        props["messages"] = json!({ "success": success, "error": error });
    }
    json!({ "type": "request", "props": props })
}

/// 回收站与配额页的数据都来自页面骨架，写操作后整页重取。
fn reload_page() -> Value {
    json!({ "type": "skiffReload" })
}

/// 代码块语言：只覆盖常见扩展，其余按纯文本（高亮由前端 Pack 完成）。
fn language_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" => "typescript",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "sh" | "bash" => "bash",
        "php" => "php",
        "html" | "htm" => "html",
        "css" => "css",
        "md" => "markdown",
        "conf" | "nginx" => "nginx",
        "sql" => "sql",
        "py" => "python",
        "go" => "go",
        _ => "plaintext",
    }
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
