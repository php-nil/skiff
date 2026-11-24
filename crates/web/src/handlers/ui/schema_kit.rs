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

/// 带页头主操作的页面根节点（budding 1.8.7 `content.actions`）。
///
/// 标题与操作同一行、操作右对齐，窄宽度整体换行到标题下方。`actions` 为空时退回
/// 普通 [`page`]，避免声明空插槽。
pub fn page_with_actions(
    title: impl Into<String>,
    subtitle: impl Into<String>,
    actions: Vec<Value>,
    body: Vec<Value>,
) -> Value {
    if actions.is_empty() {
        return page(title, subtitle, body);
    }
    json!({
        "type": "page",
        "props": { "title": title.into(), "subtitle": subtitle.into() },
        "content": { "actions": actions },
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

/// 表单：提交只触发 `onEvent.submit` 的动作链，组件自身不发请求、不弹成功提示。
///
/// `submitMode: "event"`（budding 1.8.6 起）是这条链的**声明方式**：只做本地校验并
/// 触发 `onEvent.submit`，不请求 `api`、不执行 `onSuccess` / `onError`。此前靠
/// `api: { sendOn: false }` 假装有 api 来避开「未配置 api」警告，1.8.6 后那套
/// 注入（含插件 schema 的 no-op api）全部删除。
pub fn form(submit: Value, fields: Vec<Value>, submit_text: &str, initial: Option<Value>) -> Value {
    let mut node = form_body(submit, fields, initial);
    node["content"] = json!({
        "actions": [
            {
                "type": "button",
                "props": { "text": submit_text, "color": "primary" },
                "action": { "type": "submit" }
            }
        ]
    });
    node
}

/// 不带自身动作区的表单：提交 / 重置按钮交给弹窗页脚（budding 1.8.5 起，
/// 页脚的 `submit` / `reset` 解析本图层唯一表单）。
pub fn form_body(submit: Value, fields: Vec<Value>, initial: Option<Value>) -> Value {
    let mut props = json!({
        "submitMode": "event",
        "actionsAlign": "end"
    });
    if let Some(initial) = initial {
        props["initialValues"] = initial;
    }
    json!({
        "type": "form",
        "props": props,
        "onEvent": { "submit": [submit] },
        "body": fields
    })
}

/// 弹窗表单规格：按钮统一放图层页脚（重置 / 取消 / 提交），正文可带一行说明。
pub struct DialogForm<'a> {
    pub text: &'a str,
    pub variant: &'a str,
    pub title: &'a str,
    pub width: u32,
    pub submit: Value,
    pub fields: Vec<Value>,
    pub initial: Option<Value>,
    pub submit_text: &'a str,
    pub notice: &'a str,
}

/// 打开「弹窗 + 表单」的按钮。页脚按钮不需要表单上下文，滚动时吸底可见。
///
/// 表单自身不再带动作区：1.8.5 起图层页脚的 `submit` / `reset` 直接作用于
/// 本层唯一表单，避免正文底部与页脚出现两排保存按钮。
pub fn dialog_form_button(spec: DialogForm<'_>) -> Value {
    let mut body = Vec::new();
    if !spec.notice.is_empty() {
        body.push(alert("info", "", spec.notice));
    }
    body.push(form_body(spec.submit, spec.fields, spec.initial));
    json!({
        "type": "button",
        "props": { "text": spec.text, "variant": spec.variant, "color": "primary" },
        "action": {
            "type": "dialog",
            "props": { "title": spec.title, "width": spec.width },
            "content": {
                "body": body,
                "footer": [
                    {
                        "type": "button",
                        "props": { "text": "重置", "variant": "text" },
                        "action": { "type": "reset" }
                    },
                    {
                        "type": "button",
                        "props": { "text": "取消", "variant": "outlined" },
                        "action": { "type": "close" }
                    },
                    {
                        "type": "button",
                        "props": { "text": spec.submit_text, "color": "primary" },
                        "action": { "type": "submit" }
                    }
                ]
            }
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
            // 提交只跑 `onEvent.submit` 的动作链（跳转由 `skiffSearch` 完成），
            // 组件自身不发请求——1.8.6 的 `submitMode: "event"` 就是这件事。
            "submitMode": "event"
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

/// 写操作后整页重取：详情页没有可重载的列表，新状态由服务端重新计算。
/// 壳脚本注册为 `skiffReload`，只做 `location.reload()` 重新取骨架。
pub fn reload_page() -> Value {
    json!({ "type": "skiffReload" })
}

/// 空串回落为破折号「—」：表格/详情里缺省字段的统一显示口径。
pub fn or_dash(value: &str) -> &str {
    if value.is_empty() {
        "—"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 弹窗表单的保存/重置必须写在图层页脚，表单正文不再重复一排按钮。
    #[test]
    fn dialog_form_puts_actions_in_layer_footer() {
        let button = dialog_form_button(DialogForm {
            text: "新建",
            variant: "tonal",
            title: "新建",
            width: 560,
            submit: json!({ "type": "skiffThingWrite" }),
            fields: vec![required_input("name", "名称", "请输入名称")],
            initial: None,
            submit_text: "创建",
            notice: "说明",
        });
        let footer = button["action"]["content"]["footer"]
            .as_array()
            .expect("dialog 必须有 footer");
        let actions: Vec<&str> = footer
            .iter()
            .map(|b| b["action"]["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(actions, vec!["reset", "close", "submit"]);
        let form = &button["action"]["content"]["body"][1];
        assert_eq!(form["type"], "form");
        assert!(
            form.get("content").is_none(),
            "表单正文不应再有 content.actions"
        );
    }

    /// 整页重取动作形状固定：壳脚本只认 `skiffReload`，多余字段会让前端误判。
    #[test]
    fn reload_page_emits_skiff_reload() {
        let v = reload_page();
        assert_eq!(v["type"], "skiffReload");
        assert_eq!(v.as_object().map(|m| m.len()).unwrap_or(0), 1);
    }

    /// 空操作区退回普通页面，避免页头声明空插槽；非空时写 `content.actions`。
    #[test]
    fn page_with_actions_omits_empty_slot() {
        let plain = page_with_actions("标题", "说明", Vec::new(), vec![text_line("正文")]);
        assert!(plain.get("content").is_none());
        assert_eq!(plain["props"]["title"], "标题");

        let with_actions = page_with_actions(
            "标题",
            "说明",
            vec![action_button("新建", Vec::new(), "primary")],
            vec![text_line("正文")],
        );
        assert_eq!(
            with_actions["content"]["actions"][0]["props"]["text"],
            "新建"
        );
    }

    /// 空串回落「—」，非空原样借用返回（不复制）。
    #[test]
    fn or_dash_falls_back_to_em_dash() {
        assert_eq!(or_dash(""), "—");
        assert_eq!(or_dash("running"), "running");
    }
}
