//! 最小 `multipart/form-data` 解析（只服务提交端点）。
//!
//! 严格优先：必须给出 boundary；boundary 长度与字符集受限；每部分头部数量、
//! 单行长度与总长都有上限；只接受 `entry` 与 `package`/`compose` 三个命名部分，
//! 其中 `package`（插件）与 `compose`（应用模板）二选一。
//! **文件名只允许出现在头部里，绝不参与任何路径拼接。**

use std::collections::BTreeMap;

use super::MAX_PACKAGE_BYTES;

const MAX_PARTS: usize = 8;
const MAX_PART_HEADERS: usize = 16;
const MAX_PART_HEADER_BYTES: usize = 4096;
const MAX_HEADER_LINE_BYTES: usize = 1024;
const MAX_ENTRY_BYTES: usize = 16 * 1024;
/// compose 文本上限（YAML 属于小型声明文件；64 MiB 的包体额度不适用于它）。
pub const MAX_COMPOSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct Parts {
    pub entry: Vec<u8>,
    pub package: Vec<u8>,
    pub compose: Vec<u8>,
}

/// 解析 `multipart/form-data`；错误信息只描述格式问题，不回显内容。
pub fn parse(content_type: &str, body: &[u8]) -> Result<Parts, String> {
    let boundary = boundary_of(content_type)?;
    let delimiter = format!("--{boundary}");
    let mut parts = Parts::default();
    let mut seen_entry = false;
    let mut seen_package = false;
    let mut seen_compose = false;
    let mut count = 0usize;

    let mut cursor = 0usize;
    loop {
        let Some(start) = find(&body[cursor..], delimiter.as_bytes()) else {
            return Err("multipart 缺少结束边界".to_string());
        };
        let after = cursor + start + delimiter.len();
        // 关闭边界：`--boundary--`
        if body[after..].starts_with(b"--") {
            break;
        }
        let Some(rest) = body[after..].strip_prefix(b"\r\n") else {
            return Err("multipart 边界后缺少 CRLF".to_string());
        };
        let after = after + 2;
        let Some(head_end) = find(rest, b"\r\n\r\n") else {
            return Err("multipart 部分缺少头部结束标记".to_string());
        };
        if head_end > MAX_PART_HEADER_BYTES {
            return Err("multipart 部分头部过长".to_string());
        }
        let headers = parse_headers(&rest[..head_end])?;
        let data_start = after + head_end + 4;
        let Some(next) = find(&body[data_start..], format!("\r\n{delimiter}").as_bytes()) else {
            return Err("multipart 部分缺少结束边界".to_string());
        };
        let data = &body[data_start..data_start + next];
        count += 1;
        if count > MAX_PARTS {
            return Err("multipart 部分过多".to_string());
        }
        match headers.get("content-disposition") {
            Some(value) => {
                let name =
                    disposition_name(value).ok_or_else(|| "multipart 部分缺少 name".to_string())?;
                match name.as_str() {
                    "entry" => {
                        if seen_entry {
                            return Err("entry 部分重复".to_string());
                        }
                        if data.len() > MAX_ENTRY_BYTES {
                            return Err("entry 部分过大".to_string());
                        }
                        seen_entry = true;
                        parts.entry = data.to_vec();
                    }
                    "package" => {
                        if seen_package {
                            return Err("package 部分重复".to_string());
                        }
                        if data.len() > MAX_PACKAGE_BYTES {
                            return Err("package 超过 64 MiB 上限".to_string());
                        }
                        seen_package = true;
                        parts.package = data.to_vec();
                    }
                    "compose" => {
                        if seen_compose {
                            return Err("compose 部分重复".to_string());
                        }
                        if data.len() > MAX_COMPOSE_BYTES {
                            return Err("compose 超过 1 MiB 上限".to_string());
                        }
                        seen_compose = true;
                        parts.compose = data.to_vec();
                    }
                    other => return Err(format!("未知的 multipart 部分: {other}")),
                }
            }
            None => return Err("multipart 部分缺少 Content-Disposition".to_string()),
        }
        cursor = data_start + next + 2;
    }

    if !seen_entry {
        return Err("multipart 必须包含 entry".to_string());
    }
    if seen_package == seen_compose {
        return Err("multipart 必须且只能包含 package 或 compose 之一".to_string());
    }
    Ok(parts)
}

fn boundary_of(content_type: &str) -> Result<String, String> {
    let mut pieces = content_type.split(';');
    let kind = pieces.next().unwrap_or("").trim().to_ascii_lowercase();
    if kind != "multipart/form-data" {
        return Err("Content-Type 必须是 multipart/form-data".to_string());
    }
    for piece in pieces {
        let Some((key, value)) = piece.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("boundary") {
            let value = value.trim().trim_matches('"');
            if value.is_empty() || value.len() > 70 {
                return Err("boundary 长度必须在 1..=70".to_string());
            }
            if !value.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(
                        c,
                        '\'' | '(' | ')' | '+' | '_' | ',' | '-' | '.' | '/' | ':' | '=' | '?'
                    )
            }) {
                return Err("boundary 含非法字符".to_string());
            }
            return Ok(value.to_string());
        }
    }
    Err("缺少 boundary 参数".to_string())
}

fn parse_headers(block: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(block).map_err(|_| "multipart 头部不是 UTF-8".to_string())?;
    let mut headers = BTreeMap::new();
    for (index, line) in text.split("\r\n").filter(|l| !l.is_empty()).enumerate() {
        if index >= MAX_PART_HEADERS {
            return Err("multipart 部分头部行过多".to_string());
        }
        if line.len() > MAX_HEADER_LINE_BYTES {
            return Err("multipart 部分头部行过长".to_string());
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err("multipart 部分头部格式错误".to_string());
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(headers)
}

fn disposition_name(value: &str) -> Option<String> {
    for piece in value.split(';') {
        let Some((key, raw)) = piece.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("name") {
            return Some(raw.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(boundary: &str, entry: &str, package: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        out.extend_from_slice(b"Content-Disposition: form-data; name=\"entry\"\r\n\r\n");
        out.extend_from_slice(entry.as_bytes());
        out.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        out.extend_from_slice(
            b"Content-Disposition: form-data; name=\"package\"; filename=\"x.wbpkg\"\r\n\r\n",
        );
        out.extend_from_slice(package);
        out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        out
    }

    #[test]
    fn parses_two_named_parts_and_ignores_filename() {
        let bytes = body("b0undary", r#"{"summary":"x","channel":"stable"}"#, b"PKG");
        let parts = parse("multipart/form-data; boundary=b0undary", &bytes).unwrap();
        assert_eq!(
            parts.entry,
            br#"{"summary":"x","channel":"stable"}"#.to_vec()
        );
        assert_eq!(parts.package, b"PKG".to_vec());
    }

    #[test]
    fn parses_compose_part_for_app_submissions() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"entry\"\r\n\r\n{\"id\":\"com.example.app\"}\r\n",
        );
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"compose\"\r\n\r\nservices:\r\n  web:\r\n    image: nginx\r\n",
        );
        bytes.extend_from_slice(b"--b--\r\n");
        let parts = parse("multipart/form-data; boundary=b", &bytes).unwrap();
        assert!(parts.package.is_empty());
        assert!(String::from_utf8(parts.compose)
            .unwrap()
            .contains("services:"));
    }

    #[test]
    fn rejects_package_and_compose_together() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"entry\"\r\n\r\n{}\r\n",
        );
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"package\"\r\n\r\nPKG\r\n",
        );
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"compose\"\r\n\r\nservices: {}\r\n",
        );
        bytes.extend_from_slice(b"--b--\r\n");
        assert!(parse("multipart/form-data; boundary=b", &bytes).is_err());
    }

    #[test]
    fn rejects_missing_boundary_and_non_multipart() {
        assert!(parse("application/json", b"").is_err());
        assert!(parse("multipart/form-data", b"").is_err());
    }

    #[test]
    fn rejects_duplicate_or_unknown_parts() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"entry\"\r\n\r\n{}\r\n",
        );
        bytes.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"entry\"\r\n\r\n{}\r\n",
        );
        bytes.extend_from_slice(b"--b--\r\n");
        assert!(parse("multipart/form-data; boundary=b", &bytes).is_err());

        let mut unknown = Vec::new();
        unknown.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nx\r\n",
        );
        unknown.extend_from_slice(b"--b--\r\n");
        assert!(parse("multipart/form-data; boundary=b", &unknown).is_err());
    }
}
