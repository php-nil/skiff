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
                    "还没有容器，可在下方新建"
                }
                .to_string(),
            ),
            ..Default::default()
        },
    );
    table["props"]["actionsLabel"] = json!("操作");
    table["props"]["rowActions"] = json!([
        kit::row_button(
            "详情",
            json!({ "type": "navigate", "props": { "url": "${prefix}/docker/containers/${id}" } }),
            "primary"
        ),
        kit::row_button(
            "启停",
            json!({ "type": "skiffContainerToggle",
                   "props": { "id": "${id}", "name": "${name}", "status": "${state.code}" } }),
            ""
        ),
        kit::row_button(
            "重启",
            json!({ "type": "skiffContainerAction",
                   "props": { "id": "${id}", "name": "${name}", "action": "restart" } }),
            "primary"
        ),
        kit::row_button(
            "删除",
            json!({ "type": "skiffContainerAction",
                   "props": { "id": "${id}", "name": "${name}", "action": "delete" } }),
            "danger"
        )
    ]);
    table["onEvent"] = json!({
        "rowClick": { "actions": [
            { "type": "navigate", "props": { "url": "${prefix}/docker/containers/${id}" } }
        ] }
    });
    body.push(table);

    if can_admin {
        body.push(kit::flex(
            8,
            vec![kit::action_button(
                "新建容器",
                json!({ "type": "navigate", "props": { "url": "${prefix}/docker/containers/new" } }),
                "",
            )],
        ));
    }

    kit::page(
        "容器",
        "启停/重启/删除在详情页操作，资源限制可在详情页编辑",
        body,
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
    if !actions.is_empty() {
        body.push(kit::flex(8, actions));
    }

    body.push(kit::detail_lines(
        "基本信息",
        vec![
            format!("名称：{}（{}）", c.name, c.short_id),
            format!("镜像：{}", data.image),
            format!("状态：{} · {}", c.state.label, c.status),
            format!("创建时间：{}", or_dash(&data.created_at)),
            format!("健康检查：{}", data.health),
            format!("启动命令：{}", or_dash(&data.cmd)),
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

    kit::page(
        format!("{} · 容器", c.name),
        "操作与资源限制均走 /ui/v1",
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
        &format!("{API_BASE}/containers/{}/limits", enc(container_id)),
        json!({ "type": "skiffDockerWrite", "props": { "op": "limits", "id": container_id } }),
        fields,
        "保存限制",
        None,
    )
}

fn container_op(text: &str, action: &str, c: &super::dto::ContainerItem) -> Value {
    kit::action_button(
        text,
        json!({ "type": "skiffContainerAction",
               "props": { "id": c.id, "name": c.name, "action": action } }),
        "tonal",
    )
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
            kit::action_button("刷新", json!({ "type": "skiffLogReload" }), "text"),
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

fn or_dash(value: &str) -> &str {
    if value.is_empty() {
        "—"
    } else {
        value
    }
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
