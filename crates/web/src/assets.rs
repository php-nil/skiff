use axum::extract::Path;
use axum::http::header::{ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use rust_embed::RustEmbed;

use crate::error::AppError;

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

/// 静态资源（打进二进制，§2.2）。文件名带内容版本前缀由模板控制。
///
/// 预压缩变体（`.gz`，`build.rs` 生成）按 `Accept-Encoding` 协商返回，
/// `Content-Type` 始终按原始文件名判定；`.gz` 路径本身不对外提供（避免双重编码）。
pub async fn static_file(
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let rel = path.trim_start_matches('/');
    if rel.contains("..") || rel.is_empty() {
        return Err(AppError::NotFound("静态资源不存在".into()));
    }
    if rel.ends_with(".gz") {
        return Err(AppError::NotFound("静态资源不存在".into()));
    }
    let file = Assets::get(rel).ok_or_else(|| AppError::NotFound("静态资源不存在".into()))?;
    let mime = mime_guess::from_path(rel).first_or_octet_stream();

    let accepts_gzip = headers
        .get(ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(accepts_gzip_header)
        .unwrap_or(false);
    let (data, encoding) = if accepts_gzip {
        match Assets::get(&format!("{rel}.gz")) {
            Some(gz) => (gz.data.into_owned(), Some("gzip")),
            None => (file.data.into_owned(), None),
        }
    } else {
        (file.data.into_owned(), None)
    };

    let mut resp = Response::new(axum::body::Body::from(data));
    let h = resp.headers_mut();
    // `HeaderValue` 解析走显式兜底：静态资源在请求路径上，任何解析失败都不该 panic。
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if rel.contains(".min.") {
            "public, max-age=86400"
        } else {
            "no-cache"
        }),
    );
    h.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(enc) = encoding {
        h.insert(
            CONTENT_ENCODING,
            HeaderValue::from_str(enc).unwrap_or_else(|_| HeaderValue::from_static("identity")),
        );
    }
    Ok(resp)
}

/// `Accept-Encoding` 是否接受 gzip（`q=0` 视为明确拒绝）。
fn accepts_gzip_header(v: &str) -> bool {
    v.split(',').any(|item| {
        let mut parts = item.trim().split(';');
        let Some(name) = parts.next() else {
            return false;
        };
        if !name.trim().eq_ignore_ascii_case("gzip") {
            return false;
        }
        !parts.any(|p| {
            let p = p.trim();
            let Some(qv) = p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")) else {
                return false;
            };
            qv.trim().parse::<f32>().map(|q| q <= 0.0).unwrap_or(false)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_negotiation() {
        assert!(accepts_gzip_header("gzip, deflate, br"));
        assert!(accepts_gzip_header("gzip;q=1.0"));
        assert!(accepts_gzip_header("br;q=0.9, gzip;q=0.8"));
        assert!(!accepts_gzip_header("deflate"));
        assert!(!accepts_gzip_header("gzip;q=0"));
        assert!(!accepts_gzip_header("gzip;q=0.0"));
        assert!(accepts_gzip_header("gzip;q=0.5"));
    }

    /// 收集静态文件里字面量形式的 API 调用路径，供「拼接分隔符」回归使用。
    fn quoted_api_paths(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for marker in ["request(\"", "api(\"", "post(\"", "apiUrl(\""] {
            let mut rest = text;
            while let Some(pos) = rest.find(marker) {
                rest = &rest[pos + marker.len()..];
                match rest.find('"') {
                    Some(end) => out.push(rest[..end].to_string()),
                    None => break,
                }
            }
        }
        out
    }

    #[test]
    fn file_api_js_builds_absolute_and_slash_joined_urls() {
        // 回归 1：`prefix` 不带前导斜杠，apiUrl 必须补 `/`，否则在
        // `/{p}/m/core/files` 页面上会被解析成 `/{p}/m/core/{p}/files/api/...` → 404。
        // 回归 2：调用方可能传 `"quotas"` 这类无前导 `/` 的路径，apiUrl 必须在拼接处
        // 补分隔符，否则拼成 `/{p}/files/apiquotas` → 整页 404。
        // 回归 3：所有字面量调用点都带前导 `/`，两种写法叠加也不会出现双斜杠以外的偏差。
        for name in ["files.js", "fs_quotas.js", "fs_trash.js"] {
            let js = Assets::get(name).unwrap_or_else(|| panic!("{name} 应已嵌入"));
            let text = String::from_utf8_lossy(&js.data);
            assert!(
                text.contains(r#"return "/" + prefix + "/files/api" + suffix;"#),
                "{name} 的 apiUrl 必须返回以 `/` 开头的绝对路径"
            );
            assert!(
                text.contains(r#"var suffix = path.charAt(0) === "/" ? path : "/" + path;"#),
                "{name} 的 apiUrl 必须为无前导 `/` 的 path 补分隔符"
            );
            let mut paths = quoted_api_paths(&text);
            assert!(!paths.is_empty(), "{name} 应有字面量 API 调用点");
            paths.sort();
            paths.dedup();
            for p in paths {
                assert!(
                    p.starts_with('/'),
                    "{name} 的 API 调用路径缺少前导 `/`，会拼出 /files/api{p}：{p}"
                );
            }
        }
    }

    #[test]
    fn every_static_asset_has_gzip_variant() {
        for name in Assets::iter() {
            if name.ends_with(".gz") {
                continue;
            }
            assert!(
                Assets::get(&format!("{name}.gz")).is_some(),
                "缺少预压缩变体: {name}"
            );
        }
    }

    #[test]
    fn gzip_variant_is_valid_gzip_of_original() {
        use std::io::Read;
        let orig = Assets::get("skiff.css").expect("skiff.css 应已嵌入");
        let gz = Assets::get("skiff.css.gz").expect("skiff.css.gz 应由 build.rs 生成");
        let mut dec = flate2::read::GzDecoder::new(&gz.data[..]);
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();
        assert_eq!(out.as_slice(), orig.data.as_ref());
    }
}
