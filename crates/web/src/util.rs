use std::collections::HashMap;

/// 解析 Cookie 头（只取第一段值，忽略属性）。
pub fn parse_cookies(header: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(h) = header else { return out };
    for part in h.split(';') {
        if let Some((k, v)) = part.trim().split_once('=') {
            out.entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        }
    }
    out
}

pub fn form_first(map: &HashMap<String, Vec<String>>, key: &str) -> String {
    map.get(key)
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_default()
}

/// HTML 转义（模板外的少量拼接场景）。
pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// 百分号编码（RFC 3986 unreserved 之外全编码）。
///
/// 用在 **URL 组件**：查询参数值、`Location` 头拼装。这里刻意不复用
/// `escape_html`——把 `&` 转成 `&amp;` 塞进 Location 会直接生成一个坏链接
/// （服务器会跳到一个带字面 `&amp;` 的 query），而 `<` 之类字符在头部是非法值。
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 截断字符串用于展示（避免把整段日志塞进列表页）。
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

pub fn human_size(bytes: i64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_first_takes_first_value() {
        let mut f: HashMap<String, Vec<String>> = HashMap::new();
        f.insert("grant".into(), vec!["x".into(), "y".into()]);
        assert_eq!(form_first(&f, "grant"), "x");
        // 缺失键给空串，调用方不必先判空。
        assert_eq!(form_first(&f, "absent"), "");
    }

    #[test]
    fn cookie_parsing() {
        let c = parse_cookies(Some("a=1; b=2; c=x=y"));
        assert_eq!(c.get("a").unwrap(), "1");
        assert_eq!(c.get("c").unwrap(), "x=y");
    }

    #[test]
    fn escaping_and_truncation() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(human_size(1536), "1.5 KB");
    }

    /// Location / query 组件必须走百分号编码：`&` 不能被转成 `&amp;`
    /// （那会让服务器跳到一个带字面 `&amp;` 的坏 query）。
    #[test]
    fn uri_component_encoding() {
        assert_eq!(
            encode_uri_component("/a/containers?x=1&y=2"),
            "%2Fa%2Fcontainers%3Fx%3D1%26y%3D2"
        );
        assert_eq!(encode_uri_component("abc-._~"), "abc-._~");
        assert_eq!(encode_uri_component("容器"), "%E5%AE%B9%E5%99%A8");
        assert_ne!(encode_uri_component("a&b"), escape_html("a&b"));
    }
}
