use axum::extract::Path;
use axum::http::header::{ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, VARY};
use axum::http::HeaderMap;
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
    h.insert(CONTENT_TYPE, mime.to_string().parse().unwrap());
    h.insert(
        CACHE_CONTROL,
        if rel.contains(".min.") {
            "public, max-age=86400"
        } else {
            "no-cache"
        }
        .parse()
        .unwrap(),
    );
    h.insert(VARY, "Accept-Encoding".parse().unwrap());
    if let Some(enc) = encoding {
        h.insert(CONTENT_ENCODING, enc.parse().unwrap());
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

    #[test]
    fn files_js_builds_absolute_api_urls() {
        // 回归：`prefix` 不带前导斜杠，apiUrl 必须补 `/`，否则在
        // `/{p}/m/core/files` 页面上会被解析成 `/{p}/m/core/{p}/files/api/...` → 404。
        let js = Assets::get("files.js").expect("files.js 应已嵌入");
        let text = String::from_utf8_lossy(&js.data);
        assert!(
            text.contains(r#"return "/" + prefix + "/files/api" + path;"#),
            "files.js 的 apiUrl 必须返回以 `/` 开头的绝对路径"
        );
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
