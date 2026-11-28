//! 站点详情页 · 概览 tab。
//!
//! 顶部指标卡（状态/类型/HTTPS/运行环境）+ 站点信息卡片（目录、附加域名、创建时间）。

use serde_json::{json, Value};

use super::super::schema_kit::{self as kit, metric_card};
use super::*;
use crate::util::encode_uri_component;

pub fn section(data: &SiteDetailData, prefix: &str) -> Vec<Value> {
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
        body.push(kit::alert(
            "warning",
            "运行环境不可用",
            data.runtime_alert.clone(),
        ));
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
            kit::flex(json!({ "gap": 12, "align": "center" }), line)
        })
        .collect();
    body.push(kit::card("站点信息", row_nodes));

    if !data.can_manage {
        body.push(kit::alert(
            "info",
            "",
            "写操作需要管理员权限；当前为只读视图。",
        ));
    }
    body
}

/// 概览页头右侧的站点操作：启停、默认、删除。
pub fn header_actions(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    if !data.can_manage {
        return Vec::new();
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
                    "color": "error",
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
    actions
}
