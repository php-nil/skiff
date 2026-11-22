//! 站点页族的补充页面：新建站点与站点日志。
//!
//! 站点列表与详情在 `sites.rs` / `site_page.rs`；这两个页面原先拼在壳脚本里。

use super::schema_kit::{self as kit, API_BASE};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// 新建站点：类型 / 域名 / 目录 / 运行环境 / 反代 / 别名。
///
/// `runtimes` 由调用方按当前用户可见范围给出（每项含 `id` / `name` /
/// `kind_label` / `version` / `state_code` / `state_label`）。
pub fn site_new_schema(runtimes: &[Value], type_query: &str) -> Value {
    let items: Vec<Value> = runtimes
        .iter()
        .map(|r| {
            // 运行中或不带状态标签时省略后缀，只有异常态才提示。
            let label = text(r, "state_label");
            let suffix = if text(r, "state_code") == "running" || label.is_empty() {
                String::new()
            } else {
                format!("（{label}）")
            };
            json!({
                "title": format!(
                    "{} {} · {}{}",
                    text(r, "kind_label"),
                    text(r, "version"),
                    text(r, "name"),
                    suffix
                ),
                "value": text(r, "id")
            })
        })
        .collect();

    // 字段定义与列表页「新建站点」弹窗共用一份（`site_page::site_create_fields`）：
    // 两处各写一份必然漂移，深链页与弹窗会出现不同字段或不同校验。
    let fields = super::site_page::site_create_fields(items);
    // `defaultValue` 只影响展示、不进入表单值，类型预选必须走 initialValues。
    let initial = json!({
        "site_type": if type_query == "runtime" || type_query == "proxy" {
            type_query
        } else {
            "static"
        }
    });

    kit::page(
        "新建站点",
        "提交后进入站点详情页",
        vec![
            kit::alert(
                "info",
                "",
                "运行环境站点需选择已运行的实例；静态站点只需目录；反向代理需填目标地址。别名每行一个。",
            ),
            kit::form(
                &format!("{API_BASE}/sites"),
                json!({ "type": "skiffSiteWrite", "props": { "op": "create" } }),
                fields,
                "创建站点",
                Some(initial),
            ),
        ],
    )
}

/// 站点日志：访问/错误切换 + 尾部快照 + 关键字过滤（只过滤尾部窗口）。
pub fn site_logs_schema(
    site_id: i64,
    domain: &str,
    part: &str,
    tail: &str,
    keyword: &str,
    lines: &[String],
    log_dir: &str,
) -> Value {
    let switch = |text: &str, target: &str| {
        kit::action_button(
            text,
            json!({ "type": "skiffLogSwitch",
                   "props": { "id": site_id, "domain": domain, "part": target,
                              "tail": tail, "q": keyword } }),
            "tonal",
        )
    };
    let mut body = vec![kit::flex(
        8,
        vec![
            kit::link("返回站点详情", format!("${{prefix}}/sites/{site_id}")),
            switch("访问日志", "access"),
            switch("错误日志", "error"),
            kit::action_button("刷新", json!({ "type": "skiffReload" }), "text"),
        ],
    )];

    body.push(json!({
        "type": "form",
        "onEvent": { "submit": [
            { "type": "skiffLogFilter",
              "props": { "id": site_id, "domain": domain, "part": part, "tail": tail } }
        ] },
        "props": {
            "api": { "method": "POST", "url": format!("{API_BASE}/sites/logs"), "sendOn": false },
            "actionsAlign": "end"
        },
        "body": [
            {
                "type": "input-text",
                "props": {
                    "name": "q",
                    "label": "关键字（只过滤文件尾部窗口）",
                    "defaultValue": keyword
                }
            }
        ],
        "content": {
            "actions": [
                { "type": "button", "props": { "text": "过滤", "variant": "tonal" },
                  "action": { "type": "submit" } }
            ]
        }
    }));

    if lines.is_empty() {
        body.push(kit::alert("info", "", "暂无日志。"));
    } else {
        body.push(kit::code_block(&lines.join("\n"), 560));
    }
    body.push(kit::small_text(format!(
        "共 {} 行（文件尾部最多 2MB 窗口、最新在前）· {}",
        lines.len(),
        log_dir
    )));

    kit::page(
        format!(
            "{} · {}",
            domain,
            if part == "error" {
                "错误日志"
            } else {
                "访问日志"
            }
        ),
        "定高滚动 · 按需拉取（不加载 SSE）",
        body,
    )
}
