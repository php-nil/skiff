//! 站点详情页的 SDUI Schema（服务端生成试点）。
//!
//! 试点范围：只把站点详情页从 `budding-boot.js` 的 `siteDetailSchema` 搬到内核，
//! 前端只 fetch 本模块产出的 JSON 再 embed。写操作仍是既有的 `skiffSiteWrite`
//! 动作，权限、CSRF 与审计口径不变；文件、终端、插件页继续由前端构建器拼装。
//!
//! 切 tab 只换渲染块，数据仍由一次 `GET /ui/v1/sites/{id}` 取齐（一并带回 schema）。

use serde_json::{json, Value};

use super::*;
use crate::util::encode_uri_component;

/// L3 子视图白名单；未知值回落到概览。
const TABS: [&str; 5] = ["overview", "domains", "config", "ssl", "backup"];

/// `/ui/v1` 路由前缀：与壳页 `data-api-base` 一致（JSON 命名空间不带入口前缀）。
const API_BASE: &str = "/ui/v1";

pub fn site_page_schema(data: &SiteDetailData, tab: &str, prefix: &str) -> Value {
    let view = if TABS.contains(&tab) { tab } else { "overview" };
    let site = &data.site;
    let api_sites = format!("{API_BASE}/sites/{}", site.id);
    let body = match view {
        "domains" => domains_section(data, &api_sites),
        "config" => config_section(data, &api_sites),
        "ssl" => ssl_section(data, &api_sites),
        "backup" => backup_section(data, &api_sites),
        _ => overview_section(data, prefix),
    };
    let subtitle = match view {
        "domains" => "附加域名增删；新域名需自行解析到本机".to_string(),
        "config" => "伪静态、限速与 vhost 配置；保存前做语法校验，失败拒绝写入".to_string(),
        "ssl" => "证书状态、强制 HTTPS 与 PEM 上传".to_string(),
        "backup" => "备份计划与已有归档；恢复前会自动打一份快照".to_string(),
        _ if data.can_manage => "站点概览；写操作复用既有权限、CSRF 与审计".to_string(),
        _ => "只读概览".to_string(),
    };
    json!({
        "type": "page",
        "props": { "title": site.domain, "subtitle": subtitle },
        "body": body
    })
}

fn overview_section(data: &SiteDetailData, prefix: &str) -> Vec<Value> {
    let site = &data.site;
    let file_href = |path: &str| format!("{prefix}/files?path={}", encode_uri_component(path));
    let mut rows = vec![
        (
            "站点目录",
            site.root_dir.clone(),
            Some(file_href(&site.root_dir)),
        ),
        (
            "文档根",
            site.index_dir.clone(),
            Some(file_href(&site.index_dir)),
        ),
        (
            "附加域名",
            if site.aliases.is_empty() {
                "—".to_string()
            } else {
                site.aliases.join("、")
            },
            None,
        ),
        ("创建时间", site.created_at.clone(), None),
    ];
    if !site.runtime_shared_hint.is_empty() {
        rows.push(("共享提示", site.runtime_shared_hint.clone(), None));
    }

    let mut body = Vec::new();
    if data.runtime_down && !data.runtime_alert.is_empty() {
        body.push(json!({
            "type": "alert",
            "props": { "type": "warning", "title": "运行环境不可用", "text": data.runtime_alert }
        }));
    }
    // 顶部指标条：状态 / 类型 / HTTPS / 运行环境一眼可扫，明细与操作往下放。
    body.push(json!({
        "type": "grid",
        "props": { "columns": 4, "gap": 12 },
        "content": { "body": [
            metric_card(
                "状态",
                site.status.label.as_str(),
                "",
                "站点状态",
                (site.status.code == "running").then_some("success"),
            ),
            metric_card("类型", site.type_label.as_str(), "", "站点类型", None),
            metric_card("HTTPS", site.https.label.as_str(), "", "证书", None),
            metric_card(
                "运行环境",
                if site.runtime_label.is_empty() {
                    "静态"
                } else {
                    site.runtime_label.as_str()
                },
                "",
                "接入方式",
                None,
            )
        ] }
    }));

    let row_nodes: Vec<Value> = rows
        .into_iter()
        .map(|(label, value, href)| {
            let mut line = vec![
                json!({ "type": "text", "props": { "text": label, "weight": "medium" } }),
                json!({ "type": "text", "props": { "text": value } }),
            ];
            if let Some(href) = href {
                line.push(json!({
                    "type": "link",
                    "props": { "text": "打开文件管理", "href": href }
                }));
            }
            flex(json!({ "gap": 12, "align": "center" }), line)
        })
        .collect();
    body.push(json!({
        "type": "card",
        "props": { "title": "站点信息" },
        "content": { "body": row_nodes }
    }));

    body.extend(overview_actions(data));
    body
}

fn overview_actions(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    if !data.can_manage {
        return vec![json!({
            "type": "alert",
            "props": { "type": "info", "text": "写操作需要管理员权限；当前为只读视图。" }
        })];
    }
    let mut actions = Vec::new();
    if site.status.code == "running" {
        actions.push(site_btn(
            "停止站点",
            json!({ "op": "power", "action": "stop", "id": site.id, "domain": site.domain }),
            "warning",
            None,
        ));
    } else {
        actions.push(site_btn(
            "启动站点",
            json!({ "op": "power", "action": "start", "id": site.id, "domain": site.domain }),
            "primary",
            None,
        ));
    }
    actions.push(site_btn(
        if site.is_default {
            "取消默认站点"
        } else {
            "设为默认站点"
        },
        json!({ "op": "default", "enabled": !site.is_default, "id": site.id, "domain": site.domain }),
        "secondary",
        None,
    ));
    actions.push(json!({
        "type": "button",
        "props": { "text": "删除站点", "color": "danger", "variant": "text" },
        "onEvent": { "click": { "actions": [
            {
                "type": "confirm",
                "props": {
                    "title": "删除站点",
                    "content": "删除后不可恢复：站点目录、配置与证书会一并移除。",
                    "level": "error",
                    "confirmText": "确认删除",
                    "requireText": site.domain
                }
            },
            {
                "type": "skiffSiteWrite",
                "props": { "op": "delete", "id": site.id, "domain": site.domain }
            }
        ] } }
    }));
    vec![json!({
        "type": "card",
        "props": { "title": "站点操作" },
        "content": { "body": [flex(json!({ "gap": 8, "wrap": true }), actions)] }
    })]
}

fn config_section(data: &SiteDetailData, api_sites: &str) -> Vec<Value> {
    let site = &data.site;
    let is_php = site.site_type == "runtime"
        && data.runtime.as_ref().map(|r| r.kind.code.as_str()) == Some("php");
    let rewrite = if site.rewrite.is_empty() {
        "none"
    } else {
        site.rewrite.as_str()
    };
    let limit_rate = site.limit_rate_kbps.to_string();

    let mut out = Vec::new();
    if data.can_manage {
        let mut fields = Vec::new();
        if is_php {
            fields.push(json!({
                "type": "select",
                "props": {
                    "name": "rewrite",
                    "label": "伪静态",
                    "items": [
                        { "title": "无（默认）", "value": "none" },
                        { "title": "WordPress", "value": "wordpress" },
                        { "title": "Laravel", "value": "laravel" },
                        { "title": "ThinkPHP", "value": "thinkphp" }
                    ],
                    "defaultValue": rewrite
                }
            }));
        }
        fields.push(json!({
            "type": "input-text",
            "props": {
                "name": "limit_rate_kbps",
                "label": "出口限速（KB/s，0 表示不限制）",
                "defaultValue": limit_rate
            }
        }));
        out.push(write_card(
            "伪静态与流量限制",
            if is_php {
                "PHP 伪静态预设与 nginx limit_rate；保存后重生成 vhost"
            } else {
                "nginx limit_rate；保存后重生成 vhost"
            },
            write_form(
                format!("{api_sites}/policy"),
                site_write_action(json!({ "op": "policy", "id": site.id, "domain": site.domain })),
                fields,
                "保存策略",
                Some(json!({ "rewrite": rewrite, "limit_rate_kbps": limit_rate })),
            ),
        ));
    } else {
        out.push(json!({
            "type": "card",
            "props": { "title": "伪静态与流量限制" },
            "content": { "body": [
                text_line(format!("伪静态：{rewrite}")),
                text_line(format!(
                    "出口限速：{}",
                    if site.limit_rate_kbps == 0 {
                        "不限制".to_string()
                    } else {
                        format!("{} KB/s", site.limit_rate_kbps)
                    }
                ))
            ] }
        }));
    }

    if !data.config.error.is_empty() {
        out.push(json!({
            "type": "alert",
            "props": { "type": "warning", "title": "配置读取失败", "text": data.config.error }
        }));
    } else {
        let path = if data.config.path.is_empty() {
            "—"
        } else {
            data.config.path.as_str()
        };
        let code_props = json!({
            "language": "nginx",
            "className": "skiff-code-no-lang",
            "modelValue": data.config.content
        });
        if data.can_manage {
            let mut props = code_props;
            props["name"] = json!("content");
            props["readonly"] = json!(false);
            props["height"] = json!(420);
            props["lineNumbers"] = json!(true);
            props["wrap"] = json!(false);
            props["copyable"] = json!(true);
            props["ariaLabel"] = json!("vhost 配置内容");
            props["rules"] = json!({
                "required": true,
                "messages": { "required": "配置内容不能为空" }
            });
            out.push(write_card(
                "vhost 配置",
                &format!(
                    "{path}{}",
                    if data.config.active {
                        " · 已启用"
                    } else {
                        " · 未启用"
                    }
                ),
                write_form(
                    format!("{api_sites}/config"),
                    site_write_action(
                        json!({ "op": "config", "id": site.id, "domain": site.domain }),
                    ),
                    vec![json!({
                        "type": "code",
                        "className": "skiff-code-no-lang",
                        "props": props
                    })],
                    "保存配置",
                    None,
                ),
            ));
        } else {
            out.push(write_card(
                "vhost 配置",
                &format!("{path}（只读）"),
                json!({
                    "type": "code",
                    "className": "skiff-code-no-lang",
                    "props": code_props
                }),
            ));
        }
    }
    out
}

fn domains_section(data: &SiteDetailData, api_sites: &str) -> Vec<Value> {
    let site = &data.site;
    let mut body = Vec::new();
    if site.aliases.is_empty() {
        body.push(text_line("暂无附加域名。"));
    } else {
        for alias in &site.aliases {
            let mut line = vec![text_line(alias.as_str())];
            if data.can_manage {
                line.push(site_btn(
                    "删除",
                    json!({
                        "op": "domains",
                        "action": "remove",
                        "alias": alias,
                        "id": site.id,
                        "domain": site.domain
                    }),
                    "danger",
                    Some("text"),
                ));
            }
            body.push(flex(json!({ "gap": 8 }), line));
        }
    }
    if data.can_manage {
        body.push(write_form(
            format!("{api_sites}/domains"),
            site_write_action(json!({
                "op": "domains",
                "action": "add",
                "id": site.id,
                "domain": site.domain
            })),
            vec![json!({
                "type": "input-text",
                "props": {
                    "name": "alias",
                    "label": "新增附加域名",
                    "required": true,
                    "rules": { "required": true, "messages": { "required": "请输入域名" } }
                }
            })],
            "添加",
            None,
        ));
    }
    vec![json!({
        "type": "card",
        "props": { "title": "附加域名" },
        "content": { "body": body }
    })]
}

fn ssl_section(data: &SiteDetailData, api_sites: &str) -> Vec<Value> {
    let site = &data.site;
    let ssl = &data.ssl;
    let mut body = vec![
        text_line(if ssl.cert_exists {
            "已检测到证书文件"
        } else {
            "未检测到证书文件"
        }),
        text_line(format!("证书路径：{}", dash(&ssl.cert_path))),
        text_line(format!("主体：{}", dash(&ssl.subject))),
        text_line(format!("签发者：{}", dash(&ssl.issuer))),
        text_line(format!("到期：{}", dash(&ssl.expires))),
    ];
    if data.can_manage {
        let upload_action = site_write_action(
            json!({ "op": "ssl", "action": "upload", "id": site.id, "domain": site.domain }),
        );
        body.push(flex(
            json!({ "gap": 8, "wrap": true }),
            vec![
                site_btn(
                    "生成自签证书",
                    json!({ "op": "ssl", "action": "generate", "id": site.id, "domain": site.domain }),
                    "primary",
                    None,
                ),
                site_btn(
                    if ssl.force_https {
                        "关闭强制 HTTPS"
                    } else {
                        "开启强制 HTTPS"
                    },
                    json!({ "op": "ssl", "action": "force", "enabled": !ssl.force_https, "id": site.id, "domain": site.domain }),
                    "secondary",
                    None,
                ),
                site_btn(
                    "关闭 HTTPS",
                    json!({ "op": "ssl", "action": "disable", "id": site.id, "domain": site.domain }),
                    "warning",
                    None,
                ),
                site_btn(
                    "删除证书",
                    json!({ "op": "ssl", "action": "delete_cert", "id": site.id, "domain": site.domain }),
                    "danger",
                    Some("text"),
                ),
                json!({
                    "type": "button",
                    "props": { "text": "上传已有证书（PEM）", "variant": "tonal" },
                    "action": {
                        "type": "dialog",
                        "props": {
                            "title": "上传已有证书（PEM）",
                            "width": 620
                        },
                        "content": {
                            "body": [
                                {
                                    "type": "alert",
                                    "props": {
                                        "type": "info",
                                        "text": "上传后将替换现有证书并自动启用 HTTPS；格式错误的 PEM 会被拒绝。"
                                    }
                                },
                                write_form(
                                    format!("{api_sites}/ssl"),
                                    upload_action,
                                    vec![
                                        json!({
                                            "type": "textarea",
                                            "props": {
                                                "name": "cert_pem",
                                                "label": "证书 PEM",
                                                "rows": 8,
                                                "placeholder": "-----BEGIN CERTIFICATE-----"
                                            }
                                        }),
                                        json!({
                                            "type": "textarea",
                                            "props": {
                                                "name": "key_pem",
                                                "label": "私钥 PEM（保存为 0600）",
                                                "rows": 8,
                                                "placeholder": "-----BEGIN PRIVATE KEY-----"
                                            }
                                        })
                                    ],
                                    "上传并启用",
                                    None,
                                )
                            ],
                            "footer": [
                                {
                                    "type": "button",
                                    "props": { "text": "取消", "variant": "outlined" },
                                    "action": { "type": "close" }
                                }
                            ]
                        }
                    }
                })
            ],
        ));
    }
    vec![json!({
        "type": "card",
        "props": { "title": "证书 / HTTPS" },
        "content": { "body": body }
    })]
}

fn backup_section(data: &SiteDetailData, api_sites: &str) -> Vec<Value> {
    let site = &data.site;
    let backup = &data.backup;
    let mut body = Vec::new();
    if !backup.available {
        body.push(json!({
            "type": "alert",
            "props": { "type": "warning", "text": "内核任务服务未初始化，站点备份暂不可用。" }
        }));
    } else {
        let interval = if backup.interval.is_empty() {
            "daily"
        } else {
            backup.interval.as_str()
        };
        let time = if backup.time.is_empty() {
            "03:00"
        } else {
            backup.time.as_str()
        };
        let interval_label = match interval {
            "daily" => "每天",
            "weekly" => "每周一",
            "monthly" => "每月 1 日",
            other => other,
        };
        body.push(text_line(if backup.enabled {
            format!("备份已开启（{interval_label} {time}）")
        } else {
            "备份未开启".to_string()
        }));
        body.push(text_line(format!(
            "上次：{}",
            if backup.last_text.is_empty() {
                "从未执行"
            } else {
                backup.last_text.as_str()
            }
        )));
        body.push(text_line(format!("下次：{}", dash(&backup.next_text))));
        if data.can_manage {
            body.push(site_btn(
                if backup.enabled {
                    "关闭备份"
                } else {
                    "开启备份"
                },
                json!({
                    "op": "backup",
                    "enabled": !backup.enabled,
                    "id": site.id,
                    "domain": site.domain,
                    "interval": interval,
                    "time": time
                }),
                if backup.enabled { "warning" } else { "primary" },
                None,
            ));
            body.push(write_form(
                format!("{api_sites}/backup"),
                site_write_action(json!({
                    "op": "backup",
                    "id": site.id,
                    "domain": site.domain,
                    "enabled": backup.enabled
                })),
                vec![
                    json!({
                        "type": "select",
                        "props": {
                            "name": "interval",
                            "label": "备份间隔",
                            "items": [
                                { "title": "每天", "value": "daily" },
                                { "title": "每周一", "value": "weekly" },
                                { "title": "每月 1 日", "value": "monthly" }
                            ],
                            "defaultValue": interval
                        }
                    }),
                    json!({
                        "type": "input-text",
                        "props": {
                            "name": "time",
                            "label": "执行时间（HH:MM）",
                            "defaultValue": time,
                            "rules": {
                                "pattern": "^([01]\\d|2[0-3]):[0-5]\\d$",
                                "messages": { "pattern": "时间格式应为 HH:MM（例如 03:00）" }
                            }
                        }
                    }),
                ],
                "保存备份计划",
                Some(json!({ "interval": interval, "time": time })),
            ));
        }
    }
    vec![
        json!({
            "type": "card",
            "props": { "title": "站点备份" },
            "content": { "body": body }
        }),
        archive_card(data),
    ]
}

/// 归档与恢复：列出已有 `.tar`，逐个给「恢复」入口。
///
/// 恢复是覆盖式动作，确认框要求手输域名——与删除站点同一档；执行体在任务服务里
/// 先预检、再打恢复前快照、最后解包，所以这里不需要再让用户选「要不要备份」。
fn archive_card(data: &SiteDetailData) -> Value {
    let site = &data.site;
    let mut rows = Vec::new();
    if data.backup.archives.is_empty() {
        rows.push(text_line(
            "还没有归档。开启备份计划后由调度器生成；恢复前建议先有一份较新的归档。",
        ));
    } else {
        rows.push(text_line(format!(
            "可用归档（最近 {} 份，按时间倒序）",
            data.backup.archives.len()
        )));
        for archive in &data.backup.archives {
            let mut line = vec![
                json!({ "type": "text", "props": { "text": archive.name, "weight": "medium" } }),
                json!({ "type": "text", "props": { "text": archive.size_text } }),
                json!({ "type": "text", "props": { "text": archive.modified_text } }),
            ];
            if data.can_manage {
                line.push(json!({
                    "type": "button",
                    "props": { "text": "恢复", "color": "danger", "variant": "text" },
                    "onEvent": { "click": { "actions": [
                        {
                            "type": "confirm",
                            "props": {
                                "title": "从归档恢复",
                                "content": format!(
                                    "会用 {} 覆盖站点目录：同名文件被替换，归档里没有的文件保留。恢复前会自动打一份快照，过程在后台执行，进度与结果见任务中心。",
                                    archive.name
                                ),
                                "level": "error",
                                "confirmText": "确认恢复",
                                "requireText": site.domain
                            }
                        },
                        site_write_action(json!({
                            "op": "restore",
                            "id": site.id,
                            "domain": site.domain,
                            "archive": archive.name
                        }))
                    ] } }
                }));
            }
            rows.push(flex(json!({ "gap": 12, "align": "center" }), line));
        }
    }
    json!({
        "type": "card",
        "props": { "title": "归档与恢复" },
        "content": { "body": rows }
    })
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
        "source": { "url": format!("{API_BASE}/sites"), "params": {} },
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
        tools.push(dialog_button(DialogSpec {
            text: "批量启停",
            variant: "text",
            title: "批量启停站点",
            width: 520,
            url: format!("{API_BASE}/sites/batch"),
            action: site_write_action(json!({ "op": "batch" })),
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
        tools.push(dialog_button(DialogSpec {
            text: "新建站点",
            variant: "flat",
            title: "新建站点",
            width: 640,
            url: format!("{API_BASE}/sites"),
            action: site_write_action(json!({ "op": "create" })),
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
    json!({
        "type": "page",
        "props": { "title": "网站", "subtitle": "站点列表；启停、删除与新建都在本页完成" },
        "body": [
            flex(
                json!({ "gap": 12, "wrap": true, "align": "center", "justify": "space-between" }),
                vec![
                    json!({ "type": "text", "props": { "text": count_text, "size": "small" } }),
                    flex(json!({ "gap": 8, "wrap": true, "justify": "end" }), tools)
                ]
            ),
            table
        ]
    })
}

/// 新建站点字段：列表弹窗与 `/sites/new` 深链页共用同一份定义。
fn site_create_fields(runtime_items: Vec<Value>) -> Vec<Value> {
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
        json!({
            "type": "input-text",
            "props": {
                "name": "domain",
                "label": "域名（必填）",
                "required": true,
                "rules": { "required": true, "messages": { "required": "请输入域名" } }
            }
        }),
        json!({
            "type": "input-text",
            "props": { "name": "root_dir", "label": "站点目录（可留空，由系统生成）" }
        }),
        json!({
            "type": "select",
            "props": {
                "name": "runtime_id",
                "label": "运行环境（runtime 类型必选）",
                "items": runtime_items
            }
        }),
        json!({
            "type": "input-text",
            "props": { "name": "proxy_target", "label": "反代目标（proxy 类型必填，如 http://127.0.0.1:3000）" }
        }),
        json!({
            "type": "checkbox",
            "props": { "name": "proxy_websocket", "label": "反代启用 WebSocket" }
        }),
        json!({
            "type": "textarea",
            "props": { "name": "aliases", "label": "附加域名（每行一个）", "rows": 3 }
        }),
    ]
}

struct DialogSpec<'a> {
    text: &'a str,
    variant: &'a str,
    title: &'a str,
    width: u32,
    url: String,
    action: Value,
    fields: Vec<Value>,
    initial: Option<Value>,
    submit_text: &'a str,
    notice: &'a str,
}

/// 弹窗表单按钮：与前端 `dialogFormButton` 同形（打开 dialog → 表单 → 自定义动作）。
fn dialog_button(spec: DialogSpec<'_>) -> Value {
    let mut body = Vec::new();
    if !spec.notice.is_empty() {
        body.push(json!({
            "type": "alert",
            "props": { "type": "info", "text": spec.notice }
        }));
    }
    body.push(write_form(
        spec.url,
        spec.action,
        spec.fields,
        spec.submit_text,
        spec.initial,
    ));
    json!({
        "type": "button",
        "props": { "text": spec.text, "variant": spec.variant, "color": "primary" },
        "action": {
            "type": "dialog",
            "props": {
                "title": spec.title,
                "width": spec.width
            },
            "content": {
                "body": body,
                "footer": [
                    {
                        "type": "button",
                        "props": { "text": "取消", "variant": "outlined" },
                        "action": { "type": "close" }
                    }
                ]
            }
        }
    })
}

fn dash(value: &str) -> &str {
    if value.is_empty() {
        "—"
    } else {
        value
    }
}

fn text_line(text: impl Into<String>) -> Value {
    json!({ "type": "text", "props": { "text": text.into() } })
}

fn flex(props: Value, body: Vec<Value>) -> Value {
    json!({ "type": "flex", "props": props, "content": { "body": body } })
}

/// 指标卡：大号数值 + 一行补充说明；`color` 只影响统计值的语义色。
fn metric_card(
    title: &str,
    value: impl Into<String>,
    suffix: &str,
    caption: &str,
    color: Option<&str>,
) -> Value {
    let mut stat = json!({ "value": value.into(), "label": caption, "suffix": suffix });
    if let Some(color) = color {
        stat["color"] = json!(color);
    }
    json!({
        "type": "card",
        "props": { "title": title, "variant": "outlined" },
        "content": { "body": [{ "type": "statistic", "props": stat }] }
    })
}

fn write_card(title: &str, notice: &str, form: Value) -> Value {
    let mut content = Vec::new();
    if !notice.is_empty() {
        content.push(text_line(notice));
    }
    content.push(form);
    json!({
        "type": "card",
        "props": { "title": title },
        "content": { "body": content }
    })
}

fn write_form(
    url: String,
    submit: Value,
    fields: Vec<Value>,
    submit_text: &str,
    initial_values: Option<Value>,
) -> Value {
    let mut props = json!({
        "api": { "method": "POST", "url": url, "sendOn": false },
        "actionsAlign": "end"
    });
    if let Some(initial_values) = initial_values {
        props["initialValues"] = initial_values;
    }
    json!({
        "type": "form",
        "props": props,
        "onEvent": { "submit": [submit] },
        "body": fields,
        "content": { "actions": [
            {
                "type": "button",
                "props": { "text": submit_text, "color": "primary" },
                "action": { "type": "submit" }
            }
        ] }
    })
}

fn site_write_action(props: Value) -> Value {
    json!({ "type": "skiffSiteWrite", "props": props })
}

fn site_btn(text: &str, props: Value, color: &str, variant: Option<&str>) -> Value {
    json!({
        "type": "button",
        "props": { "text": text, "color": color, "variant": variant.unwrap_or("tonal") },
        "onEvent": { "click": { "actions": [site_write_action(props)] } }
    })
}
