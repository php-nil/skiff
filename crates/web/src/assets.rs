use axum::extract::Path;
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

use crate::error::AppError;

#[derive(RustEmbed)]
#[folder = "static"]
struct Assets;

/// 版本化产物（文件名带版本，或请求带 `?v=`）的缓存策略：一年 + 不可变。
/// 升级后 URL 必变，浏览器不必回源校验。
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// 稳定别名（`budding.min.js` / `budding.css`）必须每次回源校验，避免升级后继续加载旧 bundle。
const CACHE_NO_CACHE: &str = "no-cache";

/// 入库的 budding 产物版本：文件名与 `crates/web/static/budding/budding-<ver>.min.*` 一致，
/// 壳页引用与别名映射共用这一个来源（升级时只改这里）。
///
/// 上游 2026-11-18 打包（commit `69e3a72`）：功能集相对 11-17 的 `39d9e1d` 有增量，
/// **但上游未升版本号**（根 `package.json` 仍是 `1.8.1`）。版本化文件名的缓存策略是
/// 一年 `immutable`，沿用 `1.8.1` 会让浏览器继续吃旧包，因此按仓内既有约定追加
/// `-rN`：`1.8.1-r2`（`r1` = 11-17 的初始 1.8.1）。上游升版后回归纯语义版本。
///
/// 本版变化（宿主可感知）：
/// - `admin` 壳不再自带 `border` / `border-radius` / `box-shadow`，满屏宿主无需反向覆盖；
/// - `data` 组九处硬编码浅色改为 `--v-theme-*`，暗色主题不再需要宿主补丁；
/// - `page` 暴露 `init` / `refresh` 触点，`link` / `image` 支持 `click`（`link` 配了
///   `onEvent.click` 时不再走默认导航——仓内核页无此用法，已逐条核对）；
/// - 新增 `instance.diagnose()` 与 `loadSchema()` 收编装载流程，`bundleStrict`
///   缺省三态修复。
///
/// 主包 1,311,542 B / CSS 661,899 B；code Pack 89,982 B、terminal Pack 395,014 B
/// 字节未变（仅随版本号改名），图标 78 字形字节不变。
pub const BUDDING_VERSION: &str = "1.8.1-r2";

/// 入库的包内图标字体版本（`budding-icons-<ver>.min.css` / `.woff2`）。
///
/// budding 官方包自带 `Budding Icons` 子集字体（CSS 4,645 B + woff2 3,992 B，
/// 78 个字形，含镜像 / 磁盘 / 文件夹等业务菜单语义图标），宿主不再另存完整 @mdi/font。
/// 1.8.1 未改动字形与 woff2 字节，只随版本号重命名文件。
/// woff2 本身已压缩，体积按原字节计，`build.rs` 不为它生成 `.gz`。
pub const BUDDING_ICONS_VERSION: &str = "1.8.1";

/// 壳页自有资产（`skiff-shell.css` / `budding-boot.js` / `form-enter-submit.js`）的缓存版本。
///
/// 这几个文件由本仓库维护、文件名不带版本，但同样按 `?v=` 命中 `immutable`：
/// 不升版本就改内容，浏览器会拿着一年的旧包继续跑（旧壳 + 新服务端 = 最难查的
/// 一类缺陷）。**改任一个文件都必须同时升这里**，壳页测试会钉住字面量。
/// v5：`budding-boot.js` 补表单回车提交（budding 的 `form` 按钮没有原生 submit 语义）。
/// v6：回车补丁移出 `budding-boot.js`，独立为壳页与登录页共用的
/// `form-enter-submit.js`——登录页不加载启动脚本，v5 的补丁覆盖不到登录表单。
/// v7：`budding-boot.js` 内核文件管理页 schema 重写——新建目录 / 上传 / 新建文件
/// 三个内联表单改为按钮触发 dialog 弹窗，避免列表被表单挤爆首屏。
/// v8：运行环境页「镜像参数」card 挪到 Docker 设置页（镜像前缀 + 镜像 tag 同页维护）。
/// v9：页面 URL 分组（sites/docker/plugins/settings），前端路由表与页面内链接同步。
/// v13：站点详情 Schema 改由内核生成（试点），前端只 fetch + embed。
/// v14：站点列表 Schema 同样改由内核生成（`with_schema=true`），弹窗注册表由前端补录。
/// v15：UI 审计修复——登录页主题切换按钮 fixed 定位（不再被 body overflow:hidden 裁掉）；
///   budding 组件内 label 不受全局 flex 容器规则干扰，隐藏上游渲染的空 for 重复 label；
///   侧栏菜单启用纵向滚动；表格最小列宽防止过度压缩；tooltip z-index 提到顶层。
pub const SHELL_VERSION: &str = "18";

/// 稳定别名 → 版本化真实文件名：别名不复制第二份字节，只做映射（SDUI D11）。
fn resolve_alias(rel: &str) -> String {
    match rel {
        "budding/budding.min.js" => format!("budding/budding-{BUDDING_VERSION}.min.js"),
        "budding/budding.css" => format!("budding/budding-{BUDDING_VERSION}.min.css"),
        _ => rel.to_string(),
    }
}

/// `/favicon.ico`：浏览器默认探测路径（不带安全入口前缀）。
///
/// 内容复用入库的 `favicon.svg`（矢量，16/32 均清晰），页面同时用
/// `<link rel="icon" type="image/svg+xml">` 指向带版本查询的静态路径。
pub async fn favicon() -> Result<Response, AppError> {
    let file = Assets::get("favicon.svg").ok_or_else(|| AppError::NotFound("图标不存在".into()))?;
    let mut resp = Response::new(axum::body::Body::from(file.data.into_owned()));
    let h = resp.headers_mut();
    h.insert(CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    h.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    Ok(resp)
}

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
    // 稳定别名映射到同名的版本化产物：字节只有一份，缓存策略仍按别名本身判定。
    let asset = resolve_alias(rel);

    // 磁盘覆盖路径：测试/快速迭代时，设置 SKIFF_EXTRA_STATIC_DIR 可让指定目录下的文件
    // 覆盖 rust-embed 编译入库的同名静态资源——改完 JS/CSS SCP 上去即可生效，无需重编二进制。
    // 命中磁盘分支时跳过 gzip 协商（简化复杂度），ETag 用文件内容 sha256 与入库版保持一致。
    if let Some(extra_dir) = std::env::var_os("SKIFF_EXTRA_STATIC_DIR") {
        let disk_path = PathBuf::from(&extra_dir).join(&asset);
        if let Ok(data) = fs::read(&disk_path) {
            let mime = mime_guess::from_path(&asset).first_or_octet_stream();
            let hash = sha256_bytes(&data);
            let etag = format!("\"{}\"", hex::encode(hash));
            let mut resp = if if_none_match_matches(headers.get(IF_NONE_MATCH), &etag) {
                let mut r = Response::new(axum::body::Body::empty());
                *r.status_mut() = StatusCode::NOT_MODIFIED;
                r
            } else {
                Response::new(axum::body::Body::from(data))
            };
            let h = resp.headers_mut();
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
            return Ok(resp);
        }
    }

    let file = Assets::get(&asset).ok_or_else(|| AppError::NotFound("静态资源不存在".into()))?;
    let mime = mime_guess::from_path(&asset).first_or_octet_stream();

    let accepts_gzip = headers
        .get(ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(accepts_gzip_header)
        .unwrap_or(false);
    let (data, encoding, hash) = if accepts_gzip {
        match Assets::get(&format!("{asset}.gz")) {
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
        .or_else(|| rest.strip_suffix(".css"))
        .or_else(|| rest.strip_suffix(".woff2"));
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

/// 运行时对任意字节切片计算 SHA-256（磁盘覆盖路径的 ETag 与 rust-embed 编译时 hash 对齐）。
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
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

    /// 图标必须随二进制入库；否则 `/favicon.ico` 会静默 404。
    #[test]
    fn favicon_asset_is_embedded() {
        let asset = Assets::get("favicon.svg").expect("static/favicon.svg 必须入库");
        assert!(String::from_utf8_lossy(&asset.data).contains("<svg"));
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
            cache_control_for("budding/budding-icons-1.8.0-r7.woff2", false),
            CACHE_IMMUTABLE
        );
        assert_eq!(
            cache_control_for("budding/budding-icons-1.8.0-r7.min.css", false),
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
        // 非 `.min.` 的壳引导脚本同属无版本化静态，不缓存
        assert_eq!(cache_control_for("skiff.js", false), CACHE_NO_CACHE);
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

    /// 稳定别名只做映射，不复制字节：解析结果必须是同上架版本化文件名。
    #[test]
    fn stable_aliases_map_to_versioned_artifacts() {
        assert_eq!(
            resolve_alias("budding/budding.min.js"),
            format!("budding/budding-{BUDDING_VERSION}.min.js")
        );
        assert_eq!(
            resolve_alias("budding/budding.css"),
            format!("budding/budding-{BUDDING_VERSION}.min.css")
        );
        assert_eq!(resolve_alias("skiff.css"), "skiff.css");
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

    #[test]
    fn every_static_asset_has_gzip_variant() {
        // 与 build.rs 共用同一份跳过列表：已压缩格式不再生成 `.gz`（再压只会增加字节）。
        for name in Assets::iter() {
            if name.ends_with(".gz") {
                continue;
            }
            let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if crate::asset_rules::ALREADY_COMPRESSED.contains(&ext.as_str()) {
                continue;
            }
            assert!(
                Assets::get(&format!("{name}.gz")).is_some(),
                "缺少预压缩变体: {name}"
            );
        }
    }

    /// budding 的分包文件名由 `budding-boot.js` 按 `data-budding-version` 拼接；
    /// 主包升级但漏带分包时只会在浏览器运行时 404，这里提前锁死。
    #[test]
    fn budding_packs_match_declared_version() {
        for pack in ["code", "terminal"] {
            let name = format!("budding/budding-{pack}-{BUDDING_VERSION}.min.js");
            assert!(
                Assets::get(&name).is_some(),
                "缺少 budding 分包: {name}（检查 static/budding 与 BUDDING_VERSION）"
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
