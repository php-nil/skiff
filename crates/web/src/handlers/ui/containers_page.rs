//! 容器页族的页面 Schema（列表 / 详情 / 日志）。
//!
//! 终端页不在这里：WebSocket 地址必须由浏览器用自己的 origin 拼，
//! 服务端无法代劳（见 09-JSON_PAGES.md 的例外表）。

use super::dto::ContainerDetailData;
use super::schema_kit::{self as kit, API_BASE};
use serde_json::{json, Value};

/// 容器列表：全部 / 仅异常 两个页内筛选 + 游标表格 + 新建入口。
pub fn containers_schema(
    filter: &str,
    total: usize,
    docker_available: bool,
    docker_reason: &str,
    can_admin: bool,
) -> Value {
    let mut body = Vec::new();
    if !docker_available {
        body.push(kit::alert(
            "warning",
            "Docker 不可用",
            if docker_reason.is_empty() {
                "Docker 守护进程当前不可达，容器列表为空，相关操作会返回 503。"
            } else {
                docker_reason
            },
        ));
    }

    // 「全部 / 仅异常」是同 path 只换 query 的页内跳转：SDK 的页面加载键是
    // `path#name`，query 变化不会重载，必须走 `skiffQueryNav` 显式 refresh。
    let filter_button = |text: &str, active: bool, query: Value| {
        let mut props = json!({
            "text": text,
            "size": "small",
            "variant": if active { "tonal" } else { "text" }
        });
        if active {
            props["color"] = json!("primary");
        }
        json!({
            "type": "button",
            "props": props,
            "onEvent": { "click": { "actions": [
                { "type": "skiffQueryNav", "props": { "path": "${prefix}/docker/containers", "query": query } }
            ] } }
        })
    };
    let count_text = format!(
        "{}{} 个{}",
        if filter == "error" {
            "异常容器 "
        } else {
            "共 "
        },
        total,
        if docker_available {
            ""
        } else {
            "（Docker 不可用）"
        }
    );
    body.push(kit::flex(
        8,
        vec![
            filter_button("全部", filter != "error", json!({})),
            filter_button("仅异常", filter == "error", json!({ "filter": "error" })),
            kit::text_line(count_text),
        ],
    ));

    let mut table = kit::list_table(
        json!([
            { "key": "name", "label": "名称", "width": 200 },
            { "key": "short_id", "label": "ID", "width": 110 },
            { "key": "image", "label": "镜像", "width": 260 },
            { "key": "state", "label": "状态", "text": "${state.label}", "width": 100, "align": "center" },
            { "key": "status", "label": "状态描述", "width": 220 },
            { "key": "project", "label": "Compose 项目", "width": 160 }
        ]),
        "/containers",
        "id",
        if filter == "error" {
            json!({ "filter": "error" })
        } else {
            json!(null)
        },
        kit::TableOpts {
            empty_text: Some(
                if !docker_available {
                    "Docker 不可用，容器列表为空"
                } else if filter == "error" {
                    "没有异常容器"
                } else {
                    "还没有容器，可在右上角新建"
                }
                .to_string(),
            ),
            ..Default::default()
        },
    );
    table["id"] = json!("containers-table");
    table["props"]["actionsLabel"] = json!("操作");
    table["props"]["rowActions"] = json!([
        kit::row_button(
            "详情",
            vec![
                json!({ "type": "navigate", "props": { "url": "${prefix}/docker/containers/${id}" } })
            ],
            "primary"
        ),
        // 启停按行内状态二选一：行作用域直接读 `state.code`，与后端动作白名单同源。
        kit::row_button(
            "启停",
            vec![json!({
                "type": "condition",
                "props": {
                    "when": "${state.code === 'running'}",
                    "actions": [
                        container_request("${id}", "stop", "容器已停止", "停止容器失败"),
                        container_reload()
                    ],
                    "elseActions": [
                        container_request("${id}", "start", "容器已启动", "启动容器失败"),
                        container_reload()
                    ]
                }
            })],
            ""
        ),
        kit::row_button(
            "重启",
            vec![
                container_request("${id}", "restart", "容器已重启", "重启容器失败"),
                container_reload()
            ],
            "primary"
        ),
        kit::row_button(
            "删除",
            vec![
                json!({
                    "type": "confirm",
                    "props": {
                        "title": "删除容器",
                        "content": "确认删除 ${name}？删除后不可恢复，容器内未持久化的数据会丢失。",
                        "color": "error"
                    }
                }),
                container_request("${id}", "delete", "容器已删除", "删除容器失败"),
                container_reload()
            ],
            "danger"
        )
    ]);
    table["onEvent"] = json!({
        "rowClick": { "actions": [
            { "type": "navigate", "props": { "url": "${prefix}/docker/containers/${id}" } }
        ] }
    });
    body.push(table);

    // 主操作放页头右侧（budding 1.8.7 `content.actions`），不再沉到表格下方。
    let actions = if can_admin {
        vec![kit::dialog_form_button(kit::DialogForm {
            text: "新建容器",
            variant: "tonal",
            title: "新建容器",
            width: 720,
            submit: json!({ "type": "skiffDockerWrite", "props": { "op": "create" } }),
            fields: container_create_fields(),
            initial: None,
            submit_text: "创建容器",
            notice: "端口、环境变量、绑定与卷均按每行一条；网络每行一个名称。留空表示不设置。",
        })]
    } else {
        Vec::new()
    };

    kit::page_with_actions(
        "容器",
        "启停/重启/删除在详情页操作，资源限制可在详情页编辑",
        actions,
        body,
    )
}

/// 新建容器字段：列表弹窗与 `/docker/containers/new` 深链页共用同一份定义。
pub fn container_create_fields() -> Vec<Value> {
    vec![
        kit::required_input("image", "镜像（必填）", "请输入镜像"),
        kit::text_input("name", "容器名（可选）"),
        kit::textarea("ports", "端口映射（host:container[/udp]，每行一条）", 3),
        kit::textarea("env", "环境变量（KEY=VALUE，每行一条）", 3),
        kit::textarea("binds", "宿主绑定（源:目标[:ro]，每行一条）", 3),
        kit::textarea("volumes", "命名卷（卷名:目标[:ro]，每行一条）", 3),
        kit::text_input("restart", "重启策略（如 unless-stopped）"),
        kit::textarea("networks", "网络（每行一个，可留空）", 3),
        kit::text_input("cpu_shares", "CPU 份额"),
        kit::text_input("cpu_period", "CPU 周期（μs）"),
        kit::text_input("cpu_quota", "CPU 配额（μs）"),
        kit::text_input("memory_limit_mb", "内存上限（MB）"),
        kit::text_input("memory_swap_mb", "内存+交换（MB）"),
        kit::text_input("pids_limit", "PIDs 上限"),
    ]
}

/// 新建容器页骨架（深链 `/docker/containers/new` 的落点）。
pub fn container_new_schema() -> Value {
    kit::page(
        "新建容器",
        "提交后进入容器详情页",
        vec![
            kit::alert(
                "info",
                "",
                "端口、环境变量、绑定与卷均按每行一条；网络每行一个名称。留空表示不设置。",
            ),
            kit::form(
                json!({ "type": "skiffDockerWrite", "props": { "op": "create" } }),
                container_create_fields(),
                "创建容器",
                None,
            ),
        ],
    )
}

/// 容器详情：操作条 + 六块信息卡 + 资源限制表单。
pub fn container_detail_schema(data: &ContainerDetailData) -> Value {
    let c = &data.container;
    let mut body = Vec::new();
    if !data.docker_reason.is_empty() {
        body.push(kit::alert("warning", "Docker 不可用", &data.docker_reason));
    }
    if c.unprotected_kernel {
        body.push(kit::alert(
            "warning",
            "",
            "该容器是内核托管的存量实例但缺少保护标签，重建 Web 服务或运行环境后才会生效。",
        ));
    }

    // 日志只需 DockerRead，与详情读取同权限；终端才需要 DockerExec。
    let mut actions = vec![kit::link(
        "查看日志",
        format!("${{prefix}}/docker/containers/{}/logs", enc(&c.id)),
    )];
    if data.can_exec {
        actions.push(kit::link(
            "打开终端",
            format!("${{prefix}}/docker/containers/{}/terminal", enc(&c.id)),
        ));
    }
    if data.can_operate {
        if c.state.code == "running" {
            actions.push(container_op("重启", "restart", c));
            actions.push(container_op("停止", "stop", c));
            // 强制终止：Engine 的 kill 跳过优雅停止，只用于卡死容器。
            actions.push(container_op("强制终止", "kill", c));
        } else {
            actions.push(container_op("启动", "start", c));
        }
    }
    if data.can_delete {
        actions.push(container_op("删除", "delete", c));
    }
    body.push(kit::detail_lines(
        "基本信息",
        vec![
            format!("名称：{}（{}）", c.name, c.short_id),
            format!("镜像：{}", data.image),
            format!("状态：{} · {}", c.state.label, c.status),
            format!("创建时间：{}", kit::or_dash(&data.created_at)),
            format!("健康检查：{}", data.health),
            format!("启动命令：{}", kit::or_dash(&data.cmd)),
        ],
    ));
    body.push(kit::detail_lines(
        "端口",
        if data.ports.is_empty() {
            vec!["未映射端口".to_string()]
        } else {
            data.ports
                .iter()
                .map(|p| {
                    format!(
                        "{}{}/{}",
                        if p.host.is_empty() {
                            String::new()
                        } else {
                            format!("{} → ", p.host)
                        },
                        p.container,
                        p.protocol
                    )
                })
                .collect()
        },
    ));
    body.push(kit::detail_lines(
        "挂载",
        if data.mounts.is_empty() {
            vec!["无挂载".to_string()]
        } else {
            data.mounts
                .iter()
                .map(|m| {
                    format!(
                        "{} → {}{} [{}]",
                        m.source,
                        m.target,
                        if m.read_only { "（只读）" } else { "" },
                        m.kind
                    )
                })
                .collect()
        },
    ));
    body.push(kit::detail_lines(
        "网络",
        if data.networks.is_empty() {
            vec!["未接入网络".to_string()]
        } else {
            data.networks.clone()
        },
    ));
    body.push(kit::detail_lines(
        "资源限制",
        data.limits_effective
            .iter()
            .map(|l| format!("{}：{}", l.label, l.value))
            .collect(),
    ));
    body.push(kit::detail_lines(
        "标签",
        if data.labels.is_empty() {
            vec!["无标签".to_string()]
        } else {
            data.labels
                .iter()
                .map(|l| format!("{} = {}", l.key, l.value))
                .collect()
        },
    ));

    if data.can_delete {
        body.push(kit::write_card(
            "编辑资源限制",
            "留空表示清除该限制（Engine 默认）。当前生效值见上方「资源限制」。",
            limits_form(&c.id, data),
        ));
    } else {
        body.push(kit::alert(
            "info",
            "",
            "资源限制编辑需要 Docker 生命周期权限。",
        ));
    }

    kit::page_with_actions(
        format!("{} · 容器", c.name),
        "操作与资源限制均走 /ui/v1",
        actions,
        body,
    )
}

fn limits_form(container_id: &str, data: &ContainerDetailData) -> Value {
    let field = |name: &str, label: &str, current: &str| {
        let placeholder = if current.is_empty() {
            "当前未设置".to_string()
        } else {
            format!("当前 {current}")
        };
        json!({
            "type": "input-text",
            "props": { "name": name, "label": label, "placeholder": placeholder }
        })
    };
    let l = &data.limits;
    let fields = vec![
        field("cpu_shares", "CPU 份额", &l.cpu_shares),
        field("cpu_period", "CPU 周期（μs）", &l.cpu_period),
        field("cpu_quota", "CPU 配额（μs）", &l.cpu_quota),
        field("memory_limit_mb", "内存上限（MB）", &l.memory_limit_mb),
        field("memory_swap_mb", "内存+交换（MB）", &l.memory_swap_mb),
        field("pids_limit", "PIDs 上限", &l.pids_limit),
    ];
    kit::form(
        json!({ "type": "skiffDockerWrite", "props": { "op": "limits", "id": container_id } }),
        fields,
        "保存限制",
        None,
    )
}

fn container_op(text: &str, action: &str, c: &super::dto::ContainerItem) -> Value {
    let id = enc(&c.id);
    let mut actions = Vec::new();
    if action == "kill" {
        actions.push(json!({
            "type": "confirm",
            "props": {
                "title": "强制终止容器",
                "content": format!(
                    "kill 会直接终止 {} 内的进程，不等待优雅退出，未落盘的数据可能丢失。",
                    c.name
                ),
                "color": "error"
            }
        }));
    }
    let (success, error) = match action {
        "restart" => ("容器已重启", "重启容器失败"),
        "stop" => ("容器已停止", "停止容器失败"),
        "start" => ("容器已启动", "启动容器失败"),
        "kill" => ("容器已强制终止", "强制终止失败"),
        "delete" => ("容器已删除", "删除容器失败"),
        _ => ("容器操作已提交", "容器操作失败"),
    };
    actions.push(container_request(&id, action, success, error));
    if action == "delete" {
        actions.push(json!({
            "type": "navigate",
            "props": { "url": "${prefix}/docker/containers" }
        }));
    } else {
        actions.push(kit::reload_page());
    }
    kit::action_button(text, actions, "tonal")
}

/// 容器写请求：接口统一是 `POST /containers/{id}/{action}`，确认与提示由动作链声明。
fn container_request(id: &str, action: &str, success: &str, error: &str) -> Value {
    let mut props = json!({
        "api": {
            "method": "POST",
            "url": format!("{API_BASE}/containers/{id}/{action}")
        }
    });
    if !success.is_empty() {
        props["messages"] = json!({ "success": success, "error": error });
    }
    json!({ "type": "request", "props": props })
}

/// 列表行删除后刷新本表格；组件 `id` 由本页表格声明，不依赖自动刷新。
fn container_reload() -> Value {
    json!({
        "type": "invoke",
        "props": { "component": "containers-table", "command": "reload" }
    })
}

/// 容器日志：尾部快照 + 刷新，无实时跟随。
pub fn container_logs_schema(
    container_id: &str,
    name: &str,
    state: &str,
    tail: usize,
    text: &str,
) -> Value {
    let lines: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
    let mut body = vec![kit::flex(
        8,
        vec![
            kit::link(
                "返回容器详情",
                format!("${{prefix}}/docker/containers/{}", enc(container_id)),
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
        "共 {} 行 · 取尾部 {} 行 · 状态 {}",
        lines.len(),
        tail,
        if state.is_empty() { "—" } else { state }
    )));
    kit::page(
        format!(
            "{} · 容器日志",
            if name.is_empty() { container_id } else { name }
        ),
        "尾部快照；点「刷新」取最新，暂不建实时跟随",
        body,
    )
}

/// 路径段编码：与 JS 的 `encodeURIComponent` 同语义（`/` 也编码）。
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

    /// 新建入口属于页头主操作（`content.actions`），不再混在正文末尾。
    #[test]
    fn containers_schema_puts_create_entry_in_header_actions() {
        let page = containers_schema("", 2, true, "", true);
        assert_eq!(page["props"]["title"], "容器");
        assert_eq!(page["content"]["actions"][0]["props"]["text"], "新建容器");
        assert!(
            !page["body"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["props"]["text"] == "新建容器"),
            "正文不应再保留第二个新建入口"
        );
    }

    /// 没有管理员权限时退回普通页面，不声明空操作区。
    #[test]
    fn containers_schema_omits_header_actions_without_admin() {
        let page = containers_schema("", 0, true, "", false);
        assert!(page.get("content").is_none());
    }
}
