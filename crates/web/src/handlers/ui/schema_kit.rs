//! 服务端拼 UI JSON 的共享构件。
//!
//! 定位：本模块属于 **`crates/web` 的 UI 输出端**，不是内核——只拼 JSON 形状，
//! 不读业务状态、不判定权限；权限结论由各页 handler 算好后传入。
//! 与 `site_page.rs` 同一层，供各页 page 模块复用，避免每页各写一份表格骨架。

use serde_json::{json, Value};

/// `/ui/v1` 路由前缀：与壳页 `data-api-base` 一致（JSON 命名空间不带入口前缀）。
pub const API_BASE: &str = "/ui/v1";

/// 页面根节点。
pub fn page(title: impl Into<String>, subtitle: impl Into<String>, body: Vec<Value>) -> Value {
    json!({
        "type": "page",
        "props": { "title": title.into(), "subtitle": subtitle.into() },
        "body": body
    })
}

/// 一行纯文本。
pub fn text_line(text: impl Into<String>) -> Value {
    json!({ "type": "text", "props": { "text": text.into() } })
}

/// 小号说明文本（表格下方的计数、采集时间等）。
pub fn small_text(text: impl Into<String>) -> Value {
    json!({ "type": "text", "props": { "text": text.into(), "size": "small" } })
}

/// 提示条；`title` 为空时不写该字段（组件不渲染空标题）。
pub fn alert(kind: &str, title: &str, text: impl Into<String>) -> Value {
    let mut props = json!({ "type": kind, "text": text.into() });
    if !title.is_empty() {
        props["title"] = json!(title);
    }
    json!({ "type": "alert", "props": props })
}

/// 普通链接（页面内跳转或返回上级）。
pub fn link(text: &str, href: impl Into<String>) -> Value {
    json!({ "type": "link", "props": { "text": text, "href": href.into() } })
}

/// 横向排布的一组节点。
pub fn flex(gap: i64, body: Vec<Value>) -> Value {
    json!({ "type": "flex", "props": { "gap": gap }, "content": { "body": body } })
}

/// 两端对齐的工具条：左计数、右动作组。
pub fn toolbar(left: Option<Value>, right: Vec<Value>) -> Value {
    let mut body = Vec::new();
    if let Some(left) = left {
        body.push(left);
    }
    body.push(json!({
        "type": "flex",
        "props": { "gap": 8, "wrap": true, "justify": "end" },
        "content": { "body": right }
    }));
    json!({
        "type": "flex",
        "props": { "gap": 12, "wrap": true, "align": "center", "justify": "space-between" },
        "content": { "body": body }
    })
}

/// 触发宿主动作 / 内联动作链的按钮。
pub fn action_button(text: &str, actions: Vec<Value>, variant: &str) -> Value {
    let mut props = json!({ "text": text, "variant": variant });
    if variant.is_empty() {
        props = json!({ "text": text });
    }
    json!({ "type": "button", "props": props, "onEvent": { "click": { "actions": actions } } })
}

/// 行内文字按钮（表格行操作用，样式统一）。
pub fn row_button(text: &str, actions: Vec<Value>, color: &str) -> Value {
    let mut props = json!({ "text": text, "variant": "text", "size": "small" });
    if !color.is_empty() {
        props["color"] = json!(color);
    }
    json!({ "type": "button", "props": props, "onEvent": { "click": { "actions": actions } } })
}

/// 卡片：标题 + 可选说明 + 正文节点。
pub fn card(title: &str, body: Vec<Value>) -> Value {
    json!({ "type": "card", "props": { "title": title }, "content": { "body": body } })
}

/// 写操作卡片：说明 + 表单（容器限制、compose、设置页共用）。
pub fn write_card(title: &str, notice: &str, form: Value) -> Value {
    let mut body = Vec::new();
    if !notice.is_empty() {
        body.push(text_line(notice));
    }
    body.push(form);
    card(title, body)
}

/// 表单：内置提交只触发 `onEvent.submit`（`sendOn: false`），真正请求由动作链发出。
///
/// `api` 必须存在——缺了组件会弹「表单未配置 api」的警告；`sendOn: false` 保证
/// 它永远不会自己发请求。
pub fn form(
    api_url: &str,
    submit: Value,
    fields: Vec<Value>,
    submit_text: &str,
    initial: Option<Value>,
) -> Value {
    let mut props = json!({
        "api": { "method": "POST", "url": api_url, "sendOn": false },
        "actionsAlign": "end"
    });
    if let Some(initial) = initial {
        props["initialValues"] = initial;
    }
    json!({
        "type": "form",
        "props": props,
        "onEvent": { "submit": [submit] },
        "body": fields,
        "content": {
            "actions": [
                {
                    "type": "button",
                    "props": { "text": submit_text, "color": "primary" },
                    "action": { "type": "submit" }
                }
            ]
        }
    })
}

/// 单行输入。
pub fn text_input(name: &str, label: &str) -> Value {
    json!({ "type": "input-text", "props": { "name": name, "label": label } })
}

/// 必填单行输入。
pub fn required_input(name: &str, label: &str, message: &str) -> Value {
    json!({
        "type": "input-text",
        "props": {
            "name": name,
            "label": label,
            "required": true,
            "rules": { "required": true, "messages": { "required": message } }
        }
    })
}

/// 多行输入。
pub fn textarea(name: &str, label: &str, rows: i64) -> Value {
    json!({ "type": "textarea", "props": { "name": name, "label": label, "rows": rows } })
}

/// 下拉选择；`items` 为 `(标题, 值)`。
pub fn select(name: &str, label: &str, default: &str, items: Vec<(&str, &str)>) -> Value {
    json!({
        "type": "select",
        "props": {
            "name": name,
            "label": label,
            "defaultValue": default,
            "items": items
                .into_iter()
                .map(|(title, value)| json!({ "title": title, "value": value }))
                .collect::<Vec<_>>()
        }
    })
}

/// 只读代码块：日志与配置文本用，自带复制。
pub fn code_block(text: &str, height: i64) -> Value {
    json!({
        "type": "code",
        "props": {
            "modelValue": text,
            "language": "plaintext",
            "readonly": true,
            "height": height,
            "lineNumbers": true,
            "wrap": false,
            "copyable": true
        }
    })
}

/// 卡片内堆多行文本：详情页的键值信息用它，比表格省视觉。
pub fn detail_lines(title: &str, lines: Vec<String>) -> Value {
    json!({
        "type": "card",
        "props": { "title": title },
        "content": { "body": lines.into_iter().map(text_line).collect::<Vec<_>>() }
    })
}

/// 表格的可选项；未给出的字段不写进 props，保持与组件默认值一致。
#[derive(Default)]
pub struct TableOpts {
    pub empty_text: Option<String>,
    /// 定高 + 内部滚动；长列表用它，避免整页被撑开。
    pub height: Option<i64>,
    /// 展开行区域节点（作用域含行字段），放原始 JSON 这类「需要时再看」的内容。
    pub expand: Option<Value>,
}

/// 表格节点：行数据由调用方一次给全（详情页的本地数组）。
pub fn table(columns: Value, items: Value, row_key: &str, opts: TableOpts) -> Value {
    let empty_text = opts.empty_text.unwrap_or_else(|| "暂无数据".to_string());
    let mut props = json!({
        "items": items,
        "columns": columns,
        "rowKey": row_key,
        "pagination": false,
        // 本地行一次给全，不能被组件默认每页 10 条截断。
        "perPage": -1,
        "density": "compact",
        "emptyText": empty_text
    });
    if let Some(height) = opts.height {
        props["virtual"] = json!(true);
        props["height"] = json!(height);
    }
    if let Some(expand) = opts.expand {
        props["expand"] = expand;
    }
    json!({ "type": "table", "props": props })
}

/// 游标列表：行数据由组件按 `cursor` 自行追加。
pub fn list_table(
    columns: Value,
    path: &str,
    row_key: &str,
    params: Value,
    opts: TableOpts,
) -> Value {
    let mut node = table(columns, json!([]), row_key, opts);
    node["source"] = json!({ "url": format!("{API_BASE}{path}"), "params": params });
    if let Some(props) = node.get_mut("props").and_then(|p| p.as_object_mut()) {
        props.insert("paging".into(), json!("cursor"));
        props.insert("cursorParam".into(), json!("cursor"));
        props.insert("itemsPath".into(), json!("data.items"));
        props.insert("totalPath".into(), json!("data.total"));
        props.insert("cursorPath".into(), json!("data.next_cursor"));
        props.insert("loadMoreText".into(), json!("加载更多"));
    }
    node
}

/// 关键字搜索表单：提交即整页跳转。
///
/// 为什么不用 `form.props.api`：搜索改的是页面地址（换 `?q=`），不是提交一份数据。
/// 用 `request` 提交会让地址栏与结果脱节，刷新后筛选丢失。动作统一走宿主的
/// `skiffSearch`（读表单 `q`、按 `URLSearchParams` 编码、整页跳转）。
pub fn search_form(label: &str, value: &str, base: &str, with_reset: bool) -> Value {
    let mut actions = Vec::new();
    if with_reset {
        // 重置必须整页跳转：内页 link 只改地址栏，表单默认值与已取到的结果都不刷新。
        actions.push(json!({
            "type": "button",
            "props": { "text": "重置", "variant": "text" },
            "action": { "type": "skiffSearch", "props": { "base": base, "q": "" } }
        }));
    }
    actions.push(json!({
        "type": "button",
        "props": { "text": "查询", "variant": "tonal" },
        "action": { "type": "submit" }
    }));
    json!({
        "type": "form",
        "onEvent": { "submit": [ { "type": "skiffSearch", "props": { "base": base } } ] },
        "props": {
            "actionsAlign": "end",
            // 只在本页触发 `onEvent.submit`：`sendOn: false` 让内置提交永不发送。
            // 但 `api` 必须存在——缺了组件会弹「表单未配置 api」的警告。
            "api": { "method": "POST", "url": format!("{API_BASE}/_noop"), "sendOn": false }
        },
        "body": [
            {
                "type": "input-text",
                "props": { "name": "q", "label": label, "defaultValue": value }
            }
        ],
        "content": { "actions": actions }
    })
}
