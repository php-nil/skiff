use axum::extract::Path;
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use rust_embed::RustEmbed;

use crate::error::AppError;

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

/// 版本化产物（文件名带版本，或请求带 `?v=`）的缓存策略：一年 + 不可变。
/// 升级后 URL 必变，浏览器不必回源校验。
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// 稳定别名（`budding.min.js` / `budding.css`）必须每次回源校验，避免升级后继续加载旧 bundle。
const CACHE_NO_CACHE: &str = "no-cache";

/// 静态资源（打进二进制，§2.2）。文件名带内容版本前缀由模板控制。
///
/// 预压缩变体（`.gz`，`build.rs` 生成）按 `Accept-Encoding` 协商返回，
/// `Content-Type` 始终按原始文件名判定；`.gz` 路径本身不对外提供（避免双重编码）。
pub async fn static_file(
    Path(path): Path<String>,
    uri: Uri,
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
    let (data, encoding, hash) = if accepts_gzip {
        match Assets::get(&format!("{rel}.gz")) {
            Some(gz) => {
                // ETag 必须对应**实际返回的字节**：gzip 变体与原始文件是两个表示，
                // 同一个 URL 下各自的 ETag 不同，配合 `Vary: Accept-Encoding` 才正确。
                let hash = gz.metadata.sha256_hash();
                (gz.data.into_owned(), Some("gzip"), hash)
            }
            None => {
                let hash = file.metadata.sha256_hash();
                (file.data.into_owned(), None, hash)
            }
        }
    } else {
        let hash = file.metadata.sha256_hash();
        (file.data.into_owned(), None, hash)
    };

    // 强 ETag：rust_embed 的内容 sha256，不加 `W/`。
    let etag = format!("\"{}\"", hex::encode(hash));
    let mut resp = if if_none_match_matches(headers.get(IF_NONE_MATCH), &etag) {
        let mut r = Response::new(axum::body::Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r
    } else {
        Response::new(axum::body::Body::from(data))
    };
    let h = resp.headers_mut();
    // `HeaderValue` 解析走显式兜底：静态资源在请求路径上，任何解析失败都不该 panic。
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(cache_control_for(rel, has_version_query(uri.query()))),
    );
    h.insert(
        ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"\"")),
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

/// `If-None-Match` 弱比较（RFC 9110 §13.1.2）：忽略 `W/` 前缀，按不透明标签比对；
/// `*` 匹配任意当前表示。强 ETag 与弱验证器在这里是等价的。
fn if_none_match_matches(header: Option<&HeaderValue>, etag: &str) -> bool {
    let Some(raw) = header.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    raw.split(',').any(|item| {
        let item = item.trim();
        item == "*" || item.strip_prefix("W/").unwrap_or(item) == etag
    })
}

/// 静态资源的缓存策略（SDUI D11）。
///
/// budding 产物没有内容 hash，因此分两类处理：
/// - 版本化文件名（`budding-<ver>.min.js` / `budding-<ver>.css`）或带 `?v=` 的请求：
///   URL 随升级变化，可长期强缓存；
/// - 稳定别名（`budding.min.js` / `budding.css`）：URL 不变，必须回源校验。
///
/// 其余资源维持原策略：`.min.` 资源缓存一天，其余 `no-cache`。
fn cache_control_for(rel: &str, versioned_query: bool) -> &'static str {
    if budding_versioned(rel) || versioned_query {
        CACHE_IMMUTABLE
    } else if budding_stable_alias(rel) {
        CACHE_NO_CACHE
    } else if rel.contains(".min.") {
        "public, max-age=86400"
    } else {
        CACHE_NO_CACHE
    }
}

/// 文件名是否形如 `budding-<ver>.min.js` / `budding-<ver>.css`（版本段非空）。
fn budding_versioned(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let Some(rest) = name.strip_prefix("budding-") else {
        return false;
    };
    let version = rest
        .strip_suffix(".min.js")
        .or_else(|| rest.strip_suffix(".css"));
    version.map(|v| !v.is_empty()).unwrap_or(false)
}

/// 是否为 budding 的稳定别名（升级后 URL 不变，必须 `no-cache`）。
fn budding_stable_alias(rel: &str) -> bool {
    matches!(
        rel.rsplit('/').next().unwrap_or(rel),
        "budding.min.js" | "budding.css"
    )
}

/// 请求是否带非空 `v` 查询参数（`?v=<版本>`）：不同版本即不同缓存键。
fn has_version_query(query: Option<&str>) -> bool {
    query
        .map(|q| {
            q.split('&').any(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                name == "v" && !value.is_empty()
            })
        })
        .unwrap_or(false)
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

    /// D11：版本化产物可长缓存，稳定别名必须回源校验，其余资源策略不变。
    #[test]
    fn cache_control_follows_versioning() {
        assert_eq!(
            cache_control_for("budding/budding-0.1.0.min.js", false),
            CACHE_IMMUTABLE
        );
        assert_eq!(
            cache_control_for("budding/budding-0.1.0.css", false),
            CACHE_IMMUTABLE
        );
        assert_eq!(
            cache_control_for("budding/budding.min.js", false),
            CACHE_NO_CACHE
        );
        assert_eq!(
            cache_control_for("budding/budding.css", false),
            CACHE_NO_CACHE
        );
        // `?v=` 让任意资源变成版本化 URL
        assert_eq!(cache_control_for("skiff.css", true), CACHE_IMMUTABLE);
        assert_eq!(
            cache_control_for("budding/budding.min.js", true),
            CACHE_IMMUTABLE
        );
        // 既有资源策略不变
        assert_eq!(cache_control_for("skiff.css", false), CACHE_NO_CACHE);
        assert_eq!(
            cache_control_for("htmx.min.js", false),
            "public, max-age=86400"
        );
        // 只有 `budding-<非空版本>` 才算版本化；名字不匹配时回落到原有规则
        assert_eq!(
            cache_control_for("budding-.min.js", false),
            "public, max-age=86400"
        );
        assert_eq!(
            cache_control_for("budding-extra.txt", false),
            CACHE_NO_CACHE
        );
    }

    #[test]
    fn version_query_detection() {
        assert!(has_version_query(Some("v=0.1.0")));
        assert!(has_version_query(Some("a=1&v=0.1.0")));
        assert!(has_version_query(Some("v=1&x=%2F")));
        assert!(!has_version_query(Some("v=")));
        assert!(!has_version_query(Some("v")));
        assert!(!has_version_query(Some("version=1")));
        assert!(!has_version_query(None));
    }

    // ---- ETag / 304（强 ETag 取 rust_embed 的内容 sha256，且与实际返回字节一致）----

    const CSS: &str = "skiff.css";

    async fn fetch(path: &str, headers: HeaderMap) -> Response {
        static_file(
            Path(path.to_string()),
            Uri::from_static("/p/static/x"),
            headers,
        )
        .await
        .expect("静态资源应存在")
    }

    async fn body_of(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("读取响应体")
            .to_vec()
    }

    fn etag_of(resp: &Response) -> String {
        resp.headers()
            .get(ETAG)
            .expect("静态资源必须带 ETag")
            .to_str()
            .expect("ETag 必须是可见 ASCII")
            .to_string()
    }

    fn with_if_none_match(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(IF_NONE_MATCH, HeaderValue::from_str(value).unwrap());
        h
    }

    fn with_gzip() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        h
    }

    /// 强 ETag 形如 `"<64 位十六进制>"`，同一内容多次请求必须相同。
    #[tokio::test]
    async fn etag_is_strong_and_stable() {
        let first = etag_of(&fetch(CSS, HeaderMap::new()).await);
        let second = etag_of(&fetch(CSS, HeaderMap::new()).await);
        assert_eq!(first, second);
        assert!(first.starts_with('"') && first.ends_with('"'), "{first}");
        assert_eq!(first.len(), 66, "64 位 sha256 十六进制 + 两个引号: {first}");
        assert!(first[1..65].chars().all(|c| c.is_ascii_hexdigit()));
        let body = body_of(fetch(CSS, HeaderMap::new()).await).await;
        assert!(!body.is_empty());
    }

    /// 命中 `If-None-Match`：304、空体、ETag 与 Cache-Control 原样保留。
    #[tokio::test]
    async fn if_none_match_hits_304() {
        let first = fetch(CSS, HeaderMap::new()).await;
        let etag = etag_of(&first);
        let cache = first
            .headers()
            .get(CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let resp = fetch(CSS, with_if_none_match(&etag)).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(etag_of(&resp), etag);
        assert_eq!(
            resp.headers().get(CACHE_CONTROL).unwrap().to_str().unwrap(),
            cache,
            "304 的缓存分层必须与 200 一致"
        );
        assert!(body_of(resp).await.is_empty());
    }

    /// 弱验证器、列表与 `*` 都按弱比较命中。
    #[tokio::test]
    async fn weak_and_listed_validators_hit_304() {
        let etag = etag_of(&fetch(CSS, HeaderMap::new()).await);
        for value in [
            format!("W/{etag}"),
            format!("\"other\", {etag}"),
            "*".to_string(),
        ] {
            let resp = fetch(CSS, with_if_none_match(&value)).await;
            assert_eq!(resp.status(), StatusCode::NOT_MODIFIED, "{value}");
        }
    }

    #[tokio::test]
    async fn if_none_match_miss_returns_200() {
        let resp = fetch(CSS, with_if_none_match("\"deadbeef\"")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!body_of(resp).await.is_empty());
    }

    /// gzip 变体与原始文件是两个表示：ETag 必须不同，且各自的验证器只匹配自己。
    #[tokio::test]
    async fn gzip_variant_has_its_own_etag() {
        let identity = fetch(CSS, HeaderMap::new()).await;
        let identity_etag = etag_of(&identity);
        let gz = fetch(CSS, with_gzip()).await;
        assert_eq!(
            gz.headers()
                .get(CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "Accept-Encoding: gzip 时应返回预压缩变体"
        );
        let gz_etag = etag_of(&gz);
        assert_ne!(identity_etag, gz_etag, "两个表示的 ETag 不能相同");

        // 拿 identity 的 ETag 请求 gzip 表示：不命中，返回 200。
        let mut headers = with_gzip();
        headers.insert(
            IF_NONE_MATCH,
            HeaderValue::from_str(&identity_etag).unwrap(),
        );
        assert_eq!(fetch(CSS, headers).await.status(), StatusCode::OK);

        // gzip 自己的 ETag 命中 304。
        let mut headers = with_gzip();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_str(&gz_etag).unwrap());
        let resp = fetch(CSS, headers).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert!(body_of(resp).await.is_empty());
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
        // 补分隔符，否则拼成 `/{p}/filesquotas` → 整页 404。
        // 回归 3：所有字面量调用点都带前导 `/`，两种写法叠加也不会出现双斜杠以外的偏差。
        // 基址为 `/{p}/files`：页面内 JSON 与页面路径同构，不再插 `api` 段（URL 契约 §4）。
        for name in ["files.js", "fs_quotas.js", "fs_trash.js"] {
            let js = Assets::get(name).unwrap_or_else(|| panic!("{name} 应已嵌入"));
            let text = String::from_utf8_lossy(&js.data);
            assert!(
                text.contains(r#"return "/" + prefix + "/files" + suffix;"#),
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
                    "{name} 的 API 调用路径缺少前导 `/`，会拼出 /files{p}：{p}"
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
