//! 编排页 Schema：项目列表 + 校验/部署表单。

use super::dto::ComposeProjectItem;
use super::schema_kit::{self as kit, TableOpts, API_BASE};
use serde_json::{json, Value};

pub fn compose_schema(
    projects: &[ComposeProjectItem],
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
                "Docker 守护进程当前不可达，项目列表为空。"
            } else {
                docker_reason
            },
        ));
    }

    let items: Vec<Value> = projects
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "managed": p.managed,
                "services": p.services,
                "running": p.running
            })
        })
        .collect();
    let mut table = kit::table(
        json!([
            { "key": "name", "label": "项目" },
            { "key": "managed", "label": "归属", "text": "${managed}" },
            { "key": "services", "label": "服务数", "type": "number" },
            { "key": "running", "label": "运行中", "type": "number" }
        ]),
        json!(items),
        "name",
        TableOpts {
            empty_text: Some(
                if docker_available {
                    "还没有 compose 项目，可在下方编辑并部署"
                } else {
                    "Docker 不可用，compose 项目列表为空"
                }
                .to_string(),
            ),
            ..Default::default()
        },
    );
    table["props"]["rowActions"] = json!([
        compose_button("启动", "start"),
        compose_button("停止", "stop"),
        compose_button("重启", "restart"),
        compose_button("清理", "down")
    ]);
    body.push(table);

    let mut actions = Vec::new();
    if can_admin {
        let fields = vec![
            kit::required_input(
                "project",
                "项目名（必填，不能为 skiff / sites）",
                "请填写项目名",
            ),
            {
                let mut node = kit::textarea("compose", "docker-compose.yml", 18);
                node["props"]["required"] = json!(true);
                node["props"]["rules"] =
                    json!({ "required": true, "messages": { "required": "请粘贴 compose 内容" } });
                node
            },
            kit::select(
                "action",
                "操作",
                "validate",
                vec![("仅校验", "validate"), ("校验并部署", "up")],
            ),
        ];
        actions.push(kit::dialog_form_button(kit::DialogForm {
            text: "新建 / 编辑编排",
            variant: "tonal",
            title: "新建 / 编辑编排",
            width: 780,
            submit: json!({ "type": "skiffComposeEdit" }),
            fields,
            initial: None,
            submit_text: "执行",
            notice: "compose 源文件不持久化：校验只解析，不触达 Docker；部署会创建/更新项目。",
        }));
    } else {
        body.push(kit::alert(
            "info",
            "",
            "编辑、校验与部署需要 Docker 生命周期权限。",
        ));
    }

    kit::page_with_actions(
        "编排",
        "项目列表、启停、校验与部署均在原生页面完成",
        actions,
        body,
    )
}

fn compose_button(text: &str, action: &str) -> Value {
    kit::action_button(
        text,
        vec![json!({ "type": "skiffComposeAction",
               "props": { "project": "${name}", "action": action } })],
        "tonal",
    )
}
