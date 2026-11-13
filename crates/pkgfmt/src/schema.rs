//! 插件随包页面 Schema（`ui/schema/<name>.json`）的静态校验。
//!
//! 插件功能页从 iframe 改为由壳页原生渲染后，schema 变成「插件作者提供的页面
//! 结构」，必须与内核页面区分信任等级。这里只做**渲染前**能判定的部分：
//!
//! - 节点类型白名单（未知类型直接拒装，不交给渲染器猜）；
//! - 禁止外链资源与外部 URL（`http(s)://`、协议相对、`data:`、`javascript:`）；
//! - 禁止能绕过 RPC 的字段（`api` / `url` / `href` / `html` / `raw`）；
//! - 动作白名单：只允许 `skiffPluginRpc` 与表单 `submit`；
//! - 节点数、嵌套深度、单文件字节数与字符串长度上限。
//!
//! 运行时仍由壳页的 `skiffPluginRpc` 动作统一走 `POST /plugins/{id}/rpc`
//! （CSRF + 权限矩阵 + 逐次审计），schema 本身不持有任何网络能力。

use serde_json::Value;

use crate::{PkgError, Result};

/// schema 文件字节上限（与评估稿一致）。
pub const SCHEMA_MAX_BYTES: usize = 128 * 1024;
/// 节点嵌套深度上限。
pub const SCHEMA_MAX_DEPTH: usize = 16;
/// 单文件节点数上限。
pub const SCHEMA_MAX_NODES: usize = 512;
/// 单个字符串值上限。
pub const SCHEMA_MAX_STRING: usize = 8192;
/// 数据源数量上限。
pub const SCHEMA_MAX_DATA_SOURCES: usize = 8;

/// 当前 schema 版本（顶层 `version`）。
pub const SCHEMA_VERSION: u64 = 1;

/// 允许的节点类型。只列壳页实际能渲染且已在本仓库页面里使用过的组件。
pub const NODE_TYPES: &[&str] = &[
    "page",
    "card",
    "alert",
    "text",
    "flex",
    "grid",
    "table",
    "form",
    "input-text",
    "select",
    "textarea",
    "checkbox",
    "button",
];

/// 允许的动作类型。写操作只能经 `skiffPluginRpc`；`submit` 只能出现在表单里；
/// 页内跳转只能经 `skiffPluginView`（宿主持有入口前缀与插件 id）。
pub const ACTION_TYPES: &[&str] = &["skiffPluginRpc", "skiffPluginView", "submit"];

/// 禁止出现的 props/节点键：它们能让 schema 自己发请求或注入原始 HTML。
const FORBIDDEN_KEYS: &[&str] = &["api", "url", "href", "src", "html", "raw", "target"];

/// 外部资源标记：任何字符串命中即拒。
const EXTERNAL_MARKERS: &[&str] = &[
    "http://",
    "https://",
    "//",
    "data:",
    "javascript:",
    "vbscript:",
    "file:",
    "mailto:",
];

fn invalid(message: impl Into<String>) -> PkgError {
    PkgError::Manifest(format!("MENU_SCHEMA_INVALID: {}", message.into()))
}

/// 校验 schema 文件名校验（相对 `ui/schema/`）：只允许 `[a-z0-9-].json`。
pub fn validate_file_name(name: &str) -> Result<()> {
    let ok = name.len() > 5
        && name.len() <= 64
        && name.ends_with(".json")
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !ok {
        return Err(invalid(format!(
            "schema 文件名必须是小写字母/数字/中划线 + .json（不含路径）: {name:?}"
        )));
    }
    Ok(())
}

/// 一个已通过静态校验的 schema 文档。
#[derive(Debug, Clone)]
pub struct PluginSchema {
    value: Value,
}

impl PluginSchema {
    pub fn parse(bytes: &[u8]) -> Result<PluginSchema> {
        if bytes.len() > SCHEMA_MAX_BYTES {
            return Err(invalid(format!(
                "schema 超过 {SCHEMA_MAX_BYTES} 字节上限（实际 {}）",
                bytes.len()
            )));
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| invalid("schema 不是 UTF-8".to_string()))?;
        let value: Value = serde_json::from_str(text)
            .map_err(|e| invalid(format!("schema 不是合法 JSON: {e}")))?;
        validate_schema(&value)?;
        Ok(PluginSchema { value })
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// 校验完整 schema 文档。安装期与内核托管前共用这一处。
pub fn validate_schema(value: &Value) -> Result<()> {
    let obj = value.as_object().ok_or_else(|| invalid("顶层必须是对象"))?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "version" | "data" | "page") {
            return Err(invalid(format!("顶层不允许字段 {key:?}")));
        }
    }
    match obj.get("version").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => {}
        Some(v) => return Err(invalid(format!("不支持的 schema version {v}"))),
        None => return Err(invalid("缺少 version")),
    }

    if let Some(data) = obj.get("data") {
        let data = data.as_object().ok_or_else(|| invalid("data 必须是对象"))?;
        if data.len() > SCHEMA_MAX_DATA_SOURCES {
            return Err(invalid(format!(
                "data 最多 {SCHEMA_MAX_DATA_SOURCES} 个数据源，得到 {}",
                data.len()
            )));
        }
        for (name, decl) in data {
            validate_data_source(name, decl)?;
        }
    }

    let page = obj.get("page").ok_or_else(|| invalid("缺少 page"))?;
    let mut budget = SCHEMA_MAX_NODES;
    validate_node(page, 1, &mut budget)?;
    Ok(())
}

fn validate_data_source(name: &str, decl: &Value) -> Result<()> {
    if !is_token(name, 32) {
        return Err(invalid(format!("data 源名称非法: {name:?}")));
    }
    let obj = decl
        .as_object()
        .ok_or_else(|| invalid(format!("data.{name} 必须是对象")))?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "method" | "params") {
            return Err(invalid(format!("data.{name} 不允许字段 {key:?}")));
        }
    }
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("data.{name} 缺少 method")))?;
    if !is_method(method) {
        return Err(invalid(format!(
            "data.{name}.method 只允许小写字母/数字/点/下划线/中划线: {method:?}"
        )));
    }
    if let Some(params) = obj.get("params") {
        let params = params
            .as_object()
            .ok_or_else(|| invalid(format!("data.{name}.params 必须是对象")))?;
        for (k, v) in params {
            if !is_token(k, 64) {
                return Err(invalid(format!("data.{name}.params 键名非法: {k:?}")));
            }
            validate_scalar(v, 0, 2)?;
        }
    }
    Ok(())
}

fn validate_node(node: &Value, depth: usize, budget: &mut usize) -> Result<()> {
    if depth > SCHEMA_MAX_DEPTH {
        return Err(invalid(format!("节点嵌套超过 {SCHEMA_MAX_DEPTH} 层")));
    }
    if *budget == 0 {
        return Err(invalid(format!("节点数超过 {SCHEMA_MAX_NODES} 上限")));
    }
    *budget -= 1;

    let obj = node.as_object().ok_or_else(|| invalid("节点必须是对象"))?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "type" | "props" | "body" | "content" | "onEvent" | "when"
        ) {
            return Err(invalid(format!("节点不允许字段 {key:?}")));
        }
    }
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("节点缺少 type"))?;
    if !NODE_TYPES.contains(&ty) {
        return Err(invalid(format!(
            "不支持的节点类型 {ty:?}（可选 {NODE_TYPES:?}）"
        )));
    }
    if let Some(when) = obj.get("when") {
        let s = when.as_str().ok_or_else(|| invalid("when 必须是字符串"))?;
        validate_when(s)?;
    }
    if let Some(props) = obj.get("props") {
        validate_props(props, depth, budget)?;
    }
    if let Some(body) = obj.get("body") {
        validate_nodes(body, depth + 1, budget)?;
    }
    if let Some(content) = obj.get("content") {
        let c = content
            .as_object()
            .ok_or_else(|| invalid("content 必须是对象"))?;
        for key in c.keys() {
            if !matches!(key.as_str(), "body" | "actions") {
                return Err(invalid(format!("content 不允许字段 {key:?}")));
            }
        }
        if let Some(body) = c.get("body") {
            validate_nodes(body, depth + 1, budget)?;
        }
        if let Some(actions) = c.get("actions") {
            validate_nodes(actions, depth + 1, budget)?;
        }
    }
    if let Some(events) = obj.get("onEvent") {
        validate_events(events, ty)?;
    }
    Ok(())
}

fn validate_nodes(value: &Value, depth: usize, budget: &mut usize) -> Result<()> {
    let arr = value
        .as_array()
        .ok_or_else(|| invalid("body/content.body/content.actions 必须是数组"))?;
    for node in arr {
        validate_node(node, depth, budget)?;
    }
    Ok(())
}

fn validate_props(props: &Value, depth: usize, budget: &mut usize) -> Result<()> {
    validate_value(props, depth, budget, 0)
}

fn validate_value(value: &Value, depth: usize, budget: &mut usize, nest: usize) -> Result<()> {
    if nest > 8 {
        return Err(invalid("props 嵌套过深"));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(s) => validate_string(s),
        Value::Array(arr) => {
            for v in arr {
                validate_value(v, depth, budget, nest + 1)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            // 形如节点的对象（`rowActions` 里的按钮）按节点校验，避免用 props
            // 绕过类型/动作白名单。注意 `alert` 的 `props.type = "info"` 是
            // 展示变体，不是节点类型，不能误判成节点。
            let node_type = map.get("type").and_then(Value::as_str);
            let looks_node = node_type.is_some_and(|t| NODE_TYPES.contains(&t))
                || map.contains_key("body")
                || map.contains_key("content")
                || map.contains_key("onEvent");
            if looks_node {
                return validate_node(value, depth + 1, budget);
            }
            for (k, v) in map {
                if FORBIDDEN_KEYS.contains(&k.as_str()) {
                    return Err(invalid(format!("不允许字段 {k:?}（会绕过 RPC/注入资源）")));
                }
                if k == "onEvent" {
                    validate_events(v, "")?;
                    continue;
                }
                validate_value(v, depth, budget, nest + 1)?;
            }
            Ok(())
        }
    }
}

fn validate_events(events: &Value, node_type: &str) -> Result<()> {
    let obj = events
        .as_object()
        .ok_or_else(|| invalid("onEvent 必须是对象"))?;
    for (name, spec) in obj {
        if !matches!(name.as_str(), "click" | "submit") {
            return Err(invalid(format!("不支持的交互事件 {name:?}")));
        }
        if name == "submit" && node_type != "form" {
            return Err(invalid("submit 事件只能声明在 form 节点上"));
        }
        let actions = action_list(spec)?;
        for action in actions {
            validate_action(action, node_type)?;
        }
    }
    Ok(())
}

fn action_list(spec: &Value) -> Result<&Vec<Value>> {
    if let Some(arr) = spec.as_array() {
        return Ok(arr);
    }
    let obj = spec
        .as_object()
        .ok_or_else(|| invalid("事件必须是动作数组或 { actions: [...] }"))?;
    for key in obj.keys() {
        if key != "actions" {
            return Err(invalid(format!("事件不允许字段 {key:?}")));
        }
    }
    obj.get("actions")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("事件缺少 actions 数组"))
}

fn validate_action(action: &Value, node_type: &str) -> Result<()> {
    let obj = action
        .as_object()
        .ok_or_else(|| invalid("动作必须是对象"))?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "type" | "props") {
            return Err(invalid(format!("动作不允许字段 {key:?}")));
        }
    }
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("动作缺少 type"))?;
    if !ACTION_TYPES.contains(&ty) {
        return Err(invalid(format!(
            "不允许的动作 {ty:?}（可选 {ACTION_TYPES:?}）"
        )));
    }
    if ty == "submit" && node_type != "form" {
        return Err(invalid("submit 动作只能出现在 form 的提交事件里"));
    }
    if ty == "skiffPluginRpc" {
        let props = obj
            .get("props")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("skiffPluginRpc 缺少 props"))?;
        for key in props.keys() {
            if !matches!(
                key.as_str(),
                "method" | "params" | "confirm" | "reload" | "view" | "label"
            ) {
                return Err(invalid(format!("skiffPluginRpc 不允许字段 {key:?}")));
            }
        }
        let method = props
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("skiffPluginRpc 缺少 method"))?;
        if !is_method(method) {
            return Err(invalid(format!("skiffPluginRpc.method 非法: {method:?}")));
        }
    }
    if ty == "skiffPluginView" {
        let props = obj
            .get("props")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("skiffPluginView 缺少 props"))?;
        for key in props.keys() {
            if !matches!(key.as_str(), "view" | "query") {
                return Err(invalid(format!("skiffPluginView 不允许字段 {key:?}")));
            }
        }
        let view = props
            .get("view")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("skiffPluginView 缺少 view"))?;
        if !is_token(view, 32) {
            return Err(invalid(format!("skiffPluginView.view 非法: {view:?}")));
        }
        // 只读下钻：query 只能是标量键值，宿主据此拼当前插件页的 query，
        // 不允许携带 URL / 外链。
        if let Some(query) = props.get("query") {
            let query = query
                .as_object()
                .ok_or_else(|| invalid("skiffPluginView.query 必须是对象"))?;
            for (key, value) in query {
                if !is_token(key, 32) {
                    return Err(invalid(format!("skiffPluginView.query 键名非法: {key:?}")));
                }
                match value {
                    Value::Null | Value::Bool(_) | Value::Number(_) => {}
                    Value::String(s) => validate_string(s)?,
                    _ => {
                        return Err(invalid(
                            "skiffPluginView.query 的值只能是标量（字符串/数字/布尔）",
                        ))
                    }
                }
            }
        }
    }
    if let Some(props) = obj.get("props") {
        let mut budget = SCHEMA_MAX_NODES;
        validate_props(props, 0, &mut budget)?;
    }
    Ok(())
}

fn validate_when(s: &str) -> Result<()> {
    let raw = s.trim();
    let raw = raw.strip_prefix('!').unwrap_or(raw).trim();
    let Some(inner) = raw.strip_prefix("{{").and_then(|v| v.strip_suffix("}}")) else {
        return Err(invalid(format!(
            "when 只接受 {{{{data.path}}}} 或 !{{{{data.path}}}}，得到 {s:?}"
        )));
    };
    let path = inner.trim();
    if !path.starts_with("data.")
        || !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(invalid(format!("when 绑定路径非法: {s:?}")));
    }
    Ok(())
}

fn validate_string(s: &str) -> Result<()> {
    if s.len() > SCHEMA_MAX_STRING {
        return Err(invalid(format!("字符串超过 {SCHEMA_MAX_STRING} 字节上限")));
    }
    for marker in EXTERNAL_MARKERS {
        if s.contains(marker) {
            return Err(invalid(format!("不允许外部资源/链接 {marker:?}: {s:?}")));
        }
    }
    Ok(())
}

fn validate_scalar(value: &Value, depth: usize, max_depth: usize) -> Result<()> {
    if depth > max_depth {
        return Err(invalid("数据源参数嵌套过深"));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(s) => validate_string(s),
        Value::Array(arr) => arr
            .iter()
            .try_for_each(|v| validate_scalar(v, depth + 1, max_depth)),
        Value::Object(map) => map
            .values()
            .try_for_each(|v| validate_scalar(v, depth + 1, max_depth)),
    }
}

fn is_token(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_method(s: &str) -> bool {
    is_token(s, 64)
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "version": 1,
            "data": { "status": { "method": "status" } },
            "page": {
                "type": "page",
                "props": { "title": "容器守护" },
                "body": [
                    {
                        "type": "table",
                        "props": {
                            "items": "{{data.status.containers}}",
                            "rowKey": "name",
                            "pagination": false,
                            "columns": [
                                { "key": "name", "label": "容器" },
                                { "key": "total", "label": "累计" }
                            ]
                        }
                    }
                ]
            }
        })
    }

    #[test]
    fn accepts_minimal_schema() {
        PluginSchema::parse(&serde_json::to_vec(&sample()).unwrap()).unwrap();
    }

    #[test]
    fn rejects_unknown_node_type() {
        let mut v = sample();
        v["page"]["body"][0]["type"] = json!("html");
        let e = validate_schema(&v).unwrap_err().to_string();
        assert!(e.contains("MENU_SCHEMA_INVALID"), "{e}");
        assert!(e.contains("html"), "{e}");
    }

    #[test]
    fn rejects_external_url_and_api_fields() {
        let mut v = sample();
        v["page"]["body"][0]["props"]["api"] = json!({ "url": "/x" });
        assert!(validate_schema(&v).is_err());

        let mut v = sample();
        v["page"]["props"]["title"] = json!("https://evil.example/x");
        assert!(validate_schema(&v)
            .unwrap_err()
            .to_string()
            .contains("外部资源"));

        let mut v = sample();
        v["page"]["body"][0]["props"]["columns"][0]["label"] = json!("//cdn.example/x");
        assert!(validate_schema(&v).is_err());
    }

    #[test]
    fn only_allows_rpc_actions() {
        let mut v = sample();
        v["page"]["body"].as_array_mut().unwrap().push(json!({
            "type": "button",
            "props": { "text": "刷新" },
            "onEvent": { "click": { "actions": [ { "type": "navigate", "props": { "url": "/x" } } ] } }
        }));
        let e = validate_schema(&v).unwrap_err().to_string();
        assert!(e.contains("navigate"), "{e}");

        let mut v = sample();
        v["page"]["body"].as_array_mut().unwrap().push(json!({
            "type": "button",
            "props": { "text": "重启" },
            "onEvent": { "click": { "actions": [ { "type": "skiffPluginRpc", "props": { "method": "restart", "params": { "name": "${name}" } } } ] } }
        }));
        validate_schema(&v).unwrap();
    }

    #[test]
    fn skiff_plugin_view_query_is_scalar_only() {
        let mut v = sample();
        v["page"]["body"].as_array_mut().unwrap().push(json!({
            "type": "button",
            "props": { "text": "查看封禁" },
            "onEvent": { "click": { "actions": [ {
                "type": "skiffPluginView",
                "props": { "view": "fail2ban", "query": { "jail": "${name}" } }
            } ] } }
        }));
        validate_schema(&v).unwrap();

        // query 只承载标量；嵌套对象会变成 URL 里的未知结构，直接拒装。
        let mut bad = sample();
        bad["page"]["body"].as_array_mut().unwrap().push(json!({
            "type": "button",
            "props": { "text": "x" },
            "onEvent": { "click": { "actions": [ {
                "type": "skiffPluginView",
                "props": { "view": "fail2ban", "query": { "jail": { "a": 1 } } }
            } ] } }
        }));
        let e = validate_schema(&bad).unwrap_err().to_string();
        assert!(e.contains("标量"), "{e}");
    }

    #[test]
    fn rejects_depth_and_node_budget() {
        let mut node = json!({ "type": "text", "props": { "text": "x" } });
        for _ in 0..SCHEMA_MAX_DEPTH {
            node = json!({ "type": "flex", "content": { "body": [node] } });
        }
        let v = json!({ "version": 1, "page": node });
        assert!(validate_schema(&v).is_err());

        let many: Vec<Value> = (0..=SCHEMA_MAX_NODES)
            .map(|_| json!({ "type": "text", "props": { "text": "x" } }))
            .collect();
        let v = json!({
            "version": 1,
            "page": { "type": "page", "body": many }
        });
        assert!(validate_schema(&v)
            .unwrap_err()
            .to_string()
            .contains("节点数"));
    }

    #[test]
    fn validates_file_names() {
        assert!(validate_file_name("containers.json").is_ok());
        assert!(validate_file_name("containers").is_err());
        assert!(validate_file_name("../x.json").is_err());
        assert!(validate_file_name("Containers.json").is_err());
    }
}
