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

/// 表单解码失败的位置与成因（对外只用于 400 文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormDecodeError {
    /// 出错字节在原始串中的下标。
    pub index: usize,
    pub reason: &'static str,
}

impl std::fmt::Display for FormDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（位置 {}）", self.reason, self.index)
    }
}

impl std::error::Error for FormDecodeError {}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// 严格百分号解码：`%` 后必须是两位十六进制，解码结果必须是合法 UTF-8。
///
/// 旧实现先用 `s.as_bytes()` 取字节位置，再用 `&s[i + 1..i + 3]` 当字符切片：
/// `%` 后面跟多字节字符（如 `%测`）时按字节切会落在字符边界中间直接 panic。
/// 登录页表单就走这条路径，未认证请求即可触发，因此必须只做字节级运算。
pub fn try_url_decode(s: &str) -> Result<String, FormDecodeError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = bytes.get(i + 1).copied().and_then(hex_val);
                let lo = bytes.get(i + 2).copied().and_then(hex_val);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi << 4 | lo);
                        i += 3;
                    }
                    _ => {
                        return Err(FormDecodeError {
                            index: i,
                            reason: "百分号转义不完整或含非十六进制字符",
                        })
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| FormDecodeError {
        index: e.utf8_error().valid_up_to(),
        reason: "转义后的字节不是合法 UTF-8",
    })
}

/// 宽松百分号解码：非法转义按字面量保留，非法 UTF-8 用 U+FFFD 替换。
///
/// 只用于**已认证**请求的历史行为兼容（坏字节不该让整张表单 400）。未认证入口
/// 一律走 `try_parse_form`，把坏编码变成明确的 400，而不是靠替换字符猜。
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
            b'%' => {
                let hi = bytes.get(i + 1).copied().and_then(hex_val);
                let lo = bytes.get(i + 2).copied().and_then(hex_val);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push(hi << 4 | lo);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
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

/// 解析 `application/x-www-form-urlencoded`（严格版）。
///
/// 任一字段的转义不合法即整体失败——表单是「要么全部按约定编码，要么拒收」，
/// 不做逐字段降级。
pub fn try_parse_form(body: &str) -> Result<HashMap<String, Vec<String>>, FormDecodeError> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (try_url_decode(k)?, try_url_decode(v)?),
            None => (try_url_decode(pair)?, String::new()),
        };
        out.entry(k).or_default().push(v);
    }
    Ok(out)
}

/// 解析 `application/x-www-form-urlencoded`（宽松版，已认证请求用）。
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

    /// `%` 后跟多字节字符：旧实现按字节切 `&s[i + 1..i + 3]`，会落在字符边界
    /// 中间 panic。登录页即可触发，所以这里既要求严格版报错，也要求宽松版不崩。
    #[test]
    fn url_decode_multibyte_after_percent_does_not_panic() {
        for bad in ["%测", "%容", "%%", "%", "100%", "%G1", "%1G"] {
            assert!(try_url_decode(bad).is_err(), "{bad:?} 应被严格解码拒收");
        }
        assert_eq!(url_decode("%测"), "%测");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%G1"), "%G1");
        // 合法序列仍要正常解码，不能因为加了严格校验就退化。
        assert_eq!(try_url_decode("%E6%B5%8B").unwrap(), "测");
    }

    /// 被截断的多字节 UTF-8（`%E6%B5` 只有 2/3 字节）与非法字节（`%FF`）都必须
    /// 在严格版报错，而不是变成替换字符混进账号名/密码。
    #[test]
    fn url_decode_rejects_invalid_utf8() {
        assert!(try_url_decode("%E6%B5").is_err());
        assert!(try_url_decode("%FF").is_err());
        assert!(try_url_decode("a%80b").is_err());
        assert!(try_parse_form("username=%E6%B5&password=x").is_err());
        // 宽松版只做替换，不 panic。
        assert_eq!(url_decode("%FF").chars().count(), 1);
    }

    /// 超长输入：`while i < len` 的字节推进不能退化成死循环或索引越界，
    /// 容量预分配也不能因为 `%XX` 展开而溢出。
    #[test]
    fn url_decode_handles_long_input() {
        let n = 200_000;
        let encoded = "%41".repeat(n);
        let decoded = try_url_decode(&encoded).expect("长转义串必须可解码");
        assert_eq!(decoded.len(), n);
        assert_eq!(decoded, "A".repeat(n));

        let raw = "a".repeat(1 << 20);
        assert_eq!(try_url_decode(&raw).unwrap(), raw);

        let mut body = String::new();
        for i in 0..5_000 {
            body.push_str(&format!("k{i}=v{i}&"));
        }
        assert_eq!(try_parse_form(&body).unwrap().len(), 5_000);
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
