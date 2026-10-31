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

/// 简易 URL 解码（表单值）。
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 解析 `application/x-www-form-urlencoded`。
pub fn parse_form(body: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(pair), String::new()),
        };
        out.entry(k).or_default().push(v);
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
    fn form_parsing_handles_encoding() {
        let f = parse_form("username=%E6%B5%8B%E8%AF%95&password=a+b&grant=x&grant=y");
        assert_eq!(form_first(&f, "username"), "测试");
        assert_eq!(form_first(&f, "password"), "a b");
        assert_eq!(f.get("grant").unwrap().len(), 2);
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
