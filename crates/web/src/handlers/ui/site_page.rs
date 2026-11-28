//! 站点页族的 UI Schema（服务端产出）。
//!
//! 定位：本模块属于 **`crates/web` 的 UI 输出端**，不是内核——它只读视图数据、
//! 拼 JSON，业务语义仍在 `skiff_app` 的命令与视图层，本模块不感知不变量与事务。
//! 布局留在这里的理由：结构随 `tab`、`can_manage` 与站点记录本身变化，静态资产
//! 表达不了；这类页面由服务端产 JSON 是正当形态（见 docs/sdui/SDUI_HANDBOOK
//! §2.9 与 SDUI_BUDDING_INTEGRATION/09-JSON_PAGES.md §9.1）。
//!
//! 前端只 fetch 本模块产出的 JSON 再 embed；写操作仍是既有的 `skiffSiteWrite`
//! 动作，权限、CSRF 与审计口径不变；文件、终端、插件页继续由前端构建器拼装。
//!
//! 切 tab 只换渲染块，数据仍由一次 `GET /ui/v1/sites/{id}` 取齐（一并带回 schema）。
//!
//! **拆分（2026-11-28）**：原单文件 898 行按 tab 拆为子目录，入口仍在本文件；
//! 对外 `pub` 接口不变，调用方（`sites.rs`、`sites_extra_page.rs`）无需改路径。

mod backup;
mod config;
mod domains;
mod overview;
mod ssl;

use serde_json::{json, Value};

use super::schema_kit::{self as kit};
use super::*;

/// L3 子视图白名单；未知值回落到概览。
const TABS: [&str; 5] = ["overview", "domains", "config", "ssl", "backup"];

pub fn site_page_schema(data: &SiteDetailData, tab: &str, prefix: &str) -> Value {
    let view = if TABS.contains(&tab) { tab } else { "overview" };
    let site = &data.site;
    let body = match view {
        "domains" => domains::section(data),
        "config" => config::section(data),
        "ssl" => ssl::section(data),
        "backup" => backup::section(data),
        _ => overview::section(data, prefix),
    };
    let subtitle = match view {
        "domains" => "附加域名增删；新域名需自行解析到本机".to_string(),
        "config" => "伪静态、限速与 vhost 配置；保存前做语法校验，失败拒绝写入".to_string(),
        "ssl" => "证书状态、强制 HTTPS 与 PEM 上传".to_string(),
        "backup" => "备份计划与已有归档；恢复前会自动打一份快照".to_string(),
        _ if data.can_manage => "站点概览；写操作复用既有权限、CSRF 与审计".to_string(),
        _ => "只读概览".to_string(),
    };
    // 概览页的站点操作放页头右侧；其它页签各自把操作留在内容区内。
    let actions = if view == "overview" {
        overview::header_actions(data)
    } else {
        Vec::new()
    };
    kit::page_with_actions(site.domain.clone(), subtitle, actions, body)
}

/// 站点列表页 Schema（服务端生成试点第二批）。
///
/// 只生成页面骨架：表格按 `source` 自行取 `/ui/v1/sites`，分页请求不带
/// `with_schema`，因此不会重复查询运行环境或重建 Schema。
pub fn sites_page_schema(
    total: usize,
    can_admin: bool,
    runtimes: &[skiff_app::sites::RuntimeView],
    prefix: &str,
) -> Value {
    let runtime_items: Vec<Value> = runtimes
        .iter()
        .filter(|v| v.is_listed())
        .map(|v| {
            let row = skiff_app::views::runtimes::row(v);
            let mut title = format!("{} {} · {}", row.kind_label, row.version, row.name);
            if v.effective_state != "running" {
                title.push_str(&format!(
                    "（{}）",
                    crate::handlers::sites::runtime_state_text(v)
                ));
            }
            json!({ "title": title, "value": row.id })
        })
        .collect();

    let row_url = format!("{prefix}/sites/${{id}}");
    let mut table = json!({
        "type": "table",
        "source": { "url": format!("{}/sites", kit::API_BASE), "params": {} },
        "props": {
            "items": [],
            "columns": [
                { "key": "domain", "label": "域名" },
                { "key": "type_label", "label": "类型", "width": 110 },
                { "key": "status", "label": "状态", "width": 110, "text": "${status.label}" },
                { "key": "https", "label": "HTTPS", "width": 140, "text": "${https.label}" },
                { "key": "runtime_name", "label": "运行环境", "width": 130 },
                { "key": "created_at", "label": "创建时间", "width": 120, "type": "date" }
            ],
            "rowKey": "domain",
            "pagination": false,
            "perPage": -1,
            "density": "compact",
            "emptyText": "还没有站点，创建后会生成整站目录、nginx vhost 与 PHP 探针",
            "paging": "cursor",
            "cursorParam": "cursor",
            "itemsPath": "data.items",
            "totalPath": "data.total",
            "cursorPath": "data.next_cursor",
            "loadMoreText": "加载更多"
        },
        "onEvent": { "rowClick": { "actions": [
            { "type": "navigate", "props": { "url": row_url } }
        ] } }
    });
    if can_admin {
        table["props"]["actionsLabel"] = json!("操作");
        table["props"]["rowActions"] = json!([
            {
                "type": "button",
                "props": { "text": "详情", "variant": "text", "color": "primary", "size": "small" },
                "onEvent": { "click": { "actions": [
                    { "type": "navigate", "props": { "url": row_url } }
                ] } }
            },
            {
                "type": "button",
                "props": { "text": "启动/停止", "variant": "text", "size": "small" },
                "onEvent": { "click": { "actions": [
                    {
                        "type": "skiffSiteToggle",
                        "props": { "id": "${id}", "domain": "${domain}", "status": "${status.code}" }
                    }
                ] } }
            }
        ]);
    }

    let mut tools = Vec::new();
    if can_admin {
        tools.push(kit::dialog_form_button(kit::DialogForm {
            text: "批量启停",
            variant: "text",
            title: "批量启停站点",
            width: 520,
            submit: site_write_action(json!({ "op": "batch" })),
            fields: vec![
                json!({
                    "type": "select",
                    "props": {
                        "name": "action",
                        "label": "操作",
                        "defaultValue": "start",
                        "items": [
                            { "title": "启动", "value": "start" },
                            { "title": "停止", "value": "stop" }
                        ]
                    }
                }),
                json!({
                    "type": "textarea",
                    "props": {
                        "name": "domains",
                        "label": "域名（每行一个）",
                        "rows": 5,
                        "required": true,
                        "rules": { "required": true, "messages": { "required": "请填写域名" } }
                    }
                }),
            ],
            initial: None,
            submit_text: "执行批量操作",
            notice: "每行一个域名，只对已存在的站点生效；删除仍在站点详情的删除确认里。",
        }));
        tools.push(kit::dialog_form_button(kit::DialogForm {
            text: "新建站点",
            variant: "flat",
            title: "新建站点",
            width: 640,
            submit: site_write_action(json!({ "op": "create" })),
            fields: site_create_fields(runtime_items),
            initial: Some(json!({ "site_type": "static" })),
            submit_text: "创建站点",
            notice: "运行环境站点需选择已运行的实例；静态站点只需目录；反向代理需填目标地址。别名每行一个。",
        }));
    }

    let count_text = format!(
        "共 {total} 个站点{}",
        if can_admin {
            ""
        } else {
            "（只读，写操作需管理员）"
        }
    );
    kit::page_with_actions(
        "网站",
        "站点列表；启停、删除与新建都在本页完成",
        tools,
        vec![kit::small_text(count_text), table],
    )
}

/// 新建站点字段：列表弹窗与 `/sites/new` 深链页共用同一份定义。
pub(super) fn site_create_fields(runtime_items: Vec<Value>) -> Vec<Value> {
    vec![
        json!({
            "type": "select",
            "props": {
                "name": "site_type",
                "label": "站点类型",
                "items": [
                    { "title": "运行环境（PHP / Node）", "value": "runtime" },
                    { "title": "静态站点", "value": "static" },
                    { "title": "反向代理", "value": "proxy" }
                ]
            }
        }),
        kit::required_input("domain", "域名（必填）", "请输入域名"),
        kit::text_input("root_dir", "站点目录（可留空，由系统生成）"),
        json!({
            "type": "select",
            "props": {
                "name": "runtime_id",
                "label": "运行环境（runtime 类型必选）",
                "items": runtime_items
            }
        }),
        kit::text_input(
            "proxy_target",
            "反代目标（proxy 类型必填，如 http://127.0.0.1:3000）",
        ),
        json!({
            "type": "checkbox",
            "props": { "name": "proxy_websocket", "label": "反代启用 WebSocket" }
        }),
        kit::textarea("aliases", "附加域名（每行一个）", 3),
    ]
}

pub(crate) fn site_write_action(props: Value) -> Value {
    json!({ "type": "skiffSiteWrite", "props": props })
}

pub(crate) fn site_btn(text: &str, props: Value, color: &str, variant: Option<&str>) -> Value {
    json!({
        "type": "button",
        "props": { "text": text, "color": color, "variant": variant.unwrap_or("tonal") },
        "onEvent": { "click": { "actions": [site_write_action(props)] } }
    })
}
