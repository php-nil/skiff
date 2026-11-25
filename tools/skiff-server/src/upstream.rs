//! 上游版本数据：官方生命周期表（版本 + EOL）与镜像仓库的 tag 摘要。
//!
//! 只服务 `runtimes` 子命令：把「上游现在有哪些版本」变成 `RuntimeEntry` 的输入。
//! 网络调用都收在这个模块里，规划与写盘是纯逻辑（见 `runtimes`），测试可用假实现替换。

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::build::version_key;

/// 单次响应上限：生命周期表不到 1MiB，超过视为异常。
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;

/// 每个发布分支默认保留的版本数。多留几个补丁号，老站点升级时才有降级余量。
pub const DEFAULT_PER_BRANCH: usize = 3;

/// Node.js 全量版本索引（含历史版本与 LTS 标记）。
const NODE_INDEX_URL: &str = "https://nodejs.org/dist/index.json";

/// 清单请求要接受的三类媒体类型：多架构索引、Docker manifest list、单架构 manifest。
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.docker.distribution.manifest.v2+json";

/// 一个发布分支（PHP `8.5` / Node.js `24` / Rust `1.100`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    /// 分支标识，如 `8.5` / `24` / `1.100`。
    pub cycle: String,
    /// 该分支当前最新版本，如 `8.5.13` / `24.4.0` / `1.100.0`。
    pub latest: String,
    /// EOL 日期（`YYYY-MM-DD`）；`None` 表示上游标为「未定/未 EOL」。
    pub eol: Option<String>,
    /// 是否已是 LTS（只有 Node.js 有意义）。
    pub lts: bool,
}

/// 上游数据源。测试用假实现，生产走 [`HttpUpstream`]。
pub trait Upstream {
    /// 某类型的全部发布分支（含已 EOL 的，筛选由调用方决定）。
    fn cycles(&self, kind: &str) -> Result<Vec<Cycle>>;
    /// 取镜像 tag 的摘要，返回 `sha256:<64 hex>`。
    fn image_digest(&self, image: &str) -> Result<String>;
    /// 某分支最近的至多 `per_branch` 个版本（含 `latest`，按版本降序）。
    ///
    /// 默认只回答 `latest`：拿不到历史版本时宁可少给，也不编造——清单里的版本都必须是
    /// 真的能拉起镜像的。`HttpUpstream` 按类型补齐。
    fn branch_versions(
        &self,
        _kind: &str,
        cycle: &Cycle,
        _per_branch: usize,
    ) -> Result<Vec<String>> {
        Ok(vec![cycle.latest.clone()])
    }
}

/// 走 HTTPS 的实现：endoflife.date 提供版本与 EOL，镜像仓库 v2 API 提供 digest。
pub struct HttpUpstream {
    agent: ureq::Agent,
    /// Node.js 索引 300KB 上下，按分支各拉一次太浪费，拉一次记住。
    node_index: Mutex<Option<BTreeMap<String, Vec<String>>>>,
}

impl HttpUpstream {
    pub fn new() -> Self {
        Self::build(None)
    }

    /// 经 HTTP 代理出网（内网机器直连不到 `docker.io` 时用）。`proxy` 形如 `http://10.66.10.22:1010`。
    ///
    /// 只认显式参数，不读 `HTTPS_PROXY`：本 crate 的 `ureq` 关了 `proxy-from-env`
    /// （开了会连本机 httptest 也走代理，测试在设了代理的环境里必红），
    /// 所以 `HTTPS_PROXY=… skiff-server runtimes` 是**不生效**的，必须写 `--proxy`。
    pub fn with_proxy(proxy: &str) -> Result<Self> {
        // ureq::Proxy::new 也接受裸主机名，于是「写错的值」会一路拖到 DNS 才报错
        // （报错指向真正要取的那个域名，看不出是代理配错了）。这里先卡协议前缀。
        if !proxy.contains("://") {
            bail!("--proxy 需要带协议，如 http://10.66.10.22:1010：{proxy}");
        }
        let proxy = ureq::Proxy::new(proxy)
            .with_context(|| format!("--proxy 不是合法代理地址: {proxy}"))?;
        Ok(Self::build(Some(proxy)))
    }

    fn build(proxy: Option<ureq::Proxy>) -> Self {
        let mut builder = ureq::AgentBuilder::new().timeout(Duration::from_secs(30));
        if let Some(proxy) = proxy {
            builder = builder.proxy(proxy);
        }
        Self {
            agent: builder.build(),
            node_index: Mutex::new(None),
        }
    }

    fn get_text(&self, url: &str) -> Result<String> {
        let response = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("GET {url} 失败"))?;
        let mut text = String::new();
        response
            .into_reader()
            .take(MAX_BODY_BYTES)
            .read_to_string(&mut text)
            .with_context(|| format!("读取 {url} 响应失败"))?;
        Ok(text)
    }

    /// 401 时按 `WWW-Authenticate` 的 Bearer 挑战换 token 再取一次。
    fn manifest_digest(&self, registry: &str, repo: &str, tag: &str) -> Result<String> {
        let url = format!("https://{}/v2/{repo}/manifests/{tag}", api_host(registry));
        match self.agent.get(&url).set("Accept", MANIFEST_ACCEPT).call() {
            Ok(response) => digest_of(response, &url),
            Err(ureq::Error::Status(401, response)) => {
                let challenge = response
                    .header("WWW-Authenticate")
                    .unwrap_or_default()
                    .to_string();
                let token = self.token(&challenge, repo)?;
                let response = self
                    .agent
                    .get(&url)
                    .set("Accept", MANIFEST_ACCEPT)
                    .set("Authorization", &format!("Bearer {token}"))
                    .call()
                    .with_context(|| format!("GET {url} 失败（带 token 重试）"))?;
                digest_of(response, &url)
            }
            Err(ureq::Error::Status(code, _)) => {
                bail!("{url} 返回 {code}：镜像 tag 不存在或仓库拒绝访问")
            }
            Err(error) => Err(anyhow::Error::new(error)).with_context(|| format!("GET {url} 失败")),
        }
    }

    fn token(&self, challenge: &str, repo: &str) -> Result<String> {
        let params = challenge_params(challenge);
        let Some(realm) = params.get("realm") else {
            bail!("镜像仓库要求认证但没给 realm（或只支持 Basic）: {challenge}");
        };
        let scope = params
            .get("scope")
            .cloned()
            .unwrap_or_else(|| format!("repository:{repo}:pull"));
        let mut url = format!("{realm}?scope={scope}");
        if let Some(service) = params.get("service") {
            url.push_str(&format!("&service={service}"));
        }
        let body = self
            .get_text(&url)
            .with_context(|| format!("取镜像仓库 token 失败: {url}"))?;
        let value: Value = serde_json::from_str(&body).context("token 响应不是 JSON")?;
        for key in ["token", "access_token"] {
            if let Some(token) = value[key].as_str() {
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
        bail!("token 响应里没有 token: {url}")
    }

    /// PHP 某分支的历史版本：从 `latest` 逐个减补丁号，用 php.net 的版本页确认存在。
    ///
    /// 判据是响应的 `version` 字段等于请求值——php.net 对不存在的版本仍回 200，
    /// 正文是 `{"error":"Unknown version"}`，只看状态码会把它当成有效版本。
    fn php_versions(&self, cycle: &Cycle, per_branch: usize) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for candidate in candidate_patches(&cycle.latest, per_branch) {
            if candidate == cycle.latest || self.php_release_exists(&candidate)? {
                out.push(candidate);
            } else {
                // 补丁号是连续的，断了就说明没有更早的，不必再探。
                break;
            }
        }
        Ok(out)
    }

    fn php_release_exists(&self, version: &str) -> Result<bool> {
        let url = format!("https://www.php.net/releases/index.php?json&version={version}");
        let body = self.get_text(&url)?;
        let value: Value =
            serde_json::from_str(&body).with_context(|| format!("{url} 响应不是 JSON"))?;
        Ok(value["version"].as_str() == Some(version))
    }

    /// Node.js 某大版本下的最近版本（跨 minor：`24.22.3` / `24.22.0` / `24.21.0`）。
    fn node_versions(&self, cycle: &Cycle, per_branch: usize) -> Result<Vec<String>> {
        let mut guard = self
            .node_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            let body = self.get_text(NODE_INDEX_URL)?;
            *guard = Some(node_versions_by_major(&body)?);
        }
        let found: Vec<String> = guard
            .as_ref()
            .expect("上面刚填过")
            .get(&cycle.cycle)
            .map(|versions| versions.iter().take(per_branch).cloned().collect())
            .unwrap_or_default();
        // 索引里没有这个 major（上游数据不同步）时退回 latest，总比整条丢掉好。
        Ok(if found.is_empty() {
            vec![cycle.latest.clone()]
        } else {
            found
        })
    }

    /// Rust 某分支的历史补丁：`channel-rust-<ver>.toml` 存在即该版本发布过。
    fn rust_versions(&self, cycle: &Cycle, per_branch: usize) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for candidate in candidate_patches(&cycle.latest, per_branch) {
            if candidate == cycle.latest || self.rust_channel_exists(&candidate)? {
                out.push(candidate);
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn rust_channel_exists(&self, version: &str) -> Result<bool> {
        let url = format!("https://static.rust-lang.org/dist/channel-rust-{version}.toml");
        // 用 HEAD：这份清单近 1MB，只为判断存在性不值得下载。
        match self.agent.head(&url).call() {
            Ok(_) => Ok(true),
            Err(ureq::Error::Status(404, _)) => Ok(false),
            Err(error) => {
                Err(anyhow::Error::new(error)).with_context(|| format!("HEAD {url} 失败"))
            }
        }
    }
}

impl Default for HttpUpstream {
    fn default() -> Self {
        Self::new()
    }
}

impl Upstream for HttpUpstream {
    fn cycles(&self, kind: &str) -> Result<Vec<Cycle>> {
        let product = match kind {
            "php" => "php",
            "node" => "nodejs",
            "rust" => "rust",
            other => bail!("没有对应上游类型: {other}"),
        };
        let url = format!("https://endoflife.date/api/{product}.json");
        let body = self.get_text(&url)?;
        parse_cycles(&body).with_context(|| format!("解析 {url} 失败"))
    }

    fn image_digest(&self, image: &str) -> Result<String> {
        let (registry, repo, tag) = split_image(image)?;
        self.manifest_digest(&registry, &repo, &tag)
    }

    fn branch_versions(&self, kind: &str, cycle: &Cycle, per_branch: usize) -> Result<Vec<String>> {
        let per_branch = per_branch.max(1);
        match kind {
            "php" => self.php_versions(cycle, per_branch),
            "node" => self.node_versions(cycle, per_branch),
            "rust" => self.rust_versions(cycle, per_branch),
            other => bail!("没有对应上游类型: {other}"),
        }
    }
}

/// 从 `latest` 往前数 `count` 个补丁版本（含 `latest`）。只动最后一段，减到 0 就停。
///
/// `8.5.13` + 3 → `["8.5.13", "8.5.12", "8.5.11"]`；单段版本（`24`）原样返回。
pub fn candidate_patches(latest: &str, count: usize) -> Vec<String> {
    let mut out = vec![latest.to_string()];
    if count <= 1 {
        return out;
    }
    let Some((base, patch)) = latest.rsplit_once('.') else {
        return out;
    };
    let Ok(patch) = patch.parse::<u32>() else {
        return out;
    };
    for step in 1..count as u32 {
        if patch < step {
            break;
        }
        out.push(format!("{base}.{}", patch - step));
    }
    out
}

/// Node.js 版本索引按大版本分组（大版本 → 版本降序）。
///
/// 索引里混着 `v27.0.0-alpha.0.1.1` 这类预发布版，只收 `vX.Y.Z` 三段纯数字。
pub fn node_versions_by_major(body: &str) -> Result<BTreeMap<String, Vec<String>>> {
    let rows: Vec<Value> = serde_json::from_str(body).context("Node.js 版本索引不是 JSON 数组")?;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let Some(version) = row["version"]
            .as_str()
            .and_then(|raw| raw.strip_prefix('v'))
        else {
            continue;
        };
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() != 3
            || !parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        out.entry(parts[0].to_string())
            .or_default()
            .push(version.to_string());
    }
    for versions in out.values_mut() {
        versions.sort_by_cached_key(|version| std::cmp::Reverse(version_key(version)));
        versions.dedup();
    }
    Ok(out)
}

/// 解析 endoflife.date 的分支表。
///
/// `eol` / `lts` 在该 API 里既可能是 `false` 也可能是日期字符串，所以按 `Value` 读。
pub fn parse_cycles(body: &str) -> Result<Vec<Cycle>> {
    let rows: Vec<Value> = serde_json::from_str(body).context("生命周期表不是 JSON 数组")?;
    let mut out = Vec::new();
    for row in rows {
        let (Some(cycle), Some(latest)) = (row["cycle"].as_str(), row["latest"].as_str()) else {
            continue;
        };
        out.push(Cycle {
            cycle: cycle.to_string(),
            latest: latest.to_string(),
            eol: row["eol"].as_str().map(str::to_string),
            lts: row["lts"].is_string(),
        });
    }
    Ok(out)
}

/// Registry **API** 的主机名：Docker Hub 的 `docker.io` 是网页域名，`/v2/…` 会被
/// 301 到 `www.docker.com`（响应里没有 `Docker-Content-Digest`），API 在 `registry-1.docker.io`。
fn api_host(registry: &str) -> &str {
    if registry.eq_ignore_ascii_case("docker.io") {
        "registry-1.docker.io"
    } else {
        registry
    }
}

/// `php:8.5.13-fpm-alpine` → (`docker.io`, `library/php`, `8.5.13-fpm-alpine`)。
///
/// 只接受带 tag 的镜像：没有 tag 就没有可固定的摘要。
pub fn split_image(image: &str) -> Result<(String, String, String)> {
    let (name, tag) = match image.rsplit_once(':') {
        Some((name, tag)) if !tag.is_empty() && !tag.contains('/') => (name, tag),
        _ => bail!("镜像必须带 tag: {image}"),
    };
    let (registry, repo) = match name.split_once('/') {
        Some((host, rest))
            if host.contains('.')
                || host.contains(':')
                || host.eq_ignore_ascii_case("localhost") =>
        {
            (host.to_string(), rest.to_string())
        }
        _ => ("docker.io".to_string(), name.to_string()),
    };
    let repo = if registry == "docker.io" && !repo.contains('/') {
        format!("library/{repo}")
    } else {
        repo
    };
    Ok((registry, repo, tag.to_string()))
}

fn digest_of(response: ureq::Response, url: &str) -> Result<String> {
    let digest = response
        .header("Docker-Content-Digest")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{url} 没返回 Docker-Content-Digest，无法固定镜像版本"))?;
    if !is_sha256_digest(&digest) {
        bail!("{url} 返回的摘要不是 sha256:<64 hex>: {digest}");
    }
    Ok(digest)
}

fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// 拆 `Bearer realm="…",service="…",scope="…"`；Basic 挑战返回空表（调用方报错）。
fn challenge_params(challenge: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let rest = match challenge.split_once(' ') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("bearer") => rest,
        _ => return out,
    };
    for part in rest.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        out.insert(
            key.trim().to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_requires_a_scheme() {
        // 裸主机名会被 ureq 当合法代理，错误一路拖到 DNS，看不出来是代理配错了。
        let Err(error) = HttpUpstream::with_proxy("not a url") else {
            panic!("缺协议必须直接拒绝");
        };
        assert!(format!("{error}").contains("需要带协议"), "{error}");
        assert!(HttpUpstream::with_proxy("http://10.66.10.22:1010").is_ok());
    }

    #[test]
    fn docker_hub_uses_the_registry_api_host() {
        // 打 docker.io 会 301 到 www.docker.com，拿不到 digest 头；API 只在 registry-1。
        assert_eq!(api_host("docker.io"), "registry-1.docker.io");
        assert_eq!(api_host("Docker.IO"), "registry-1.docker.io");
        // 私有仓库/其它 registry 原样透传，不能瞎改域名。
        assert_eq!(api_host("harbor.example.com"), "harbor.example.com");
        assert_eq!(api_host("ghcr.io"), "ghcr.io");
    }

    #[test]
    fn split_image_handles_docker_hub_and_private_registry() {
        assert_eq!(
            split_image("php:8.5.13-fpm-alpine").unwrap(),
            (
                "docker.io".to_string(),
                "library/php".to_string(),
                "8.5.13-fpm-alpine".to_string()
            )
        );
        assert_eq!(
            split_image("harbor.example.com/skiff/node:24.4.0-alpine").unwrap(),
            (
                "harbor.example.com".to_string(),
                "skiff/node".to_string(),
                "24.4.0-alpine".to_string()
            )
        );
        assert_eq!(
            split_image("127.0.0.1:5000/rust:1.100.0-slim").unwrap(),
            (
                "127.0.0.1:5000".to_string(),
                "rust".to_string(),
                "1.100.0-slim".to_string()
            )
        );
        assert!(split_image("php").is_err());
    }

    #[test]
    fn parse_cycles_reads_dates_and_lts_flags() {
        let body = r#"[
            {"cycle":"8.5","latest":"8.5.13","eol":"2030-12-31","lts":false},
            {"cycle":"8.4","latest":"8.4.28","eol":false},
            {"cycle":"26","latest":"26.13.1","eol":"2029-04-30","lts":"2026-10-28"}
        ]"#;
        let cycles = parse_cycles(body).unwrap();
        assert_eq!(cycles.len(), 3);
        assert_eq!(cycles[0].eol.as_deref(), Some("2030-12-31"));
        assert!(!cycles[0].lts);
        assert_eq!(cycles[1].eol, None);
        assert!(cycles[2].lts, "LTS 日期字符串必须识别为 true");
    }

    #[test]
    fn candidate_patches_counts_down_and_stops_at_zero() {
        assert_eq!(
            candidate_patches("8.5.13", 3),
            vec!["8.5.13", "8.5.12", "8.5.11"]
        );
        assert_eq!(candidate_patches("8.6.0", 3), vec!["8.6.0"], "到 0 就停");
        assert_eq!(candidate_patches("8.6.1", 5), vec!["8.6.1", "8.6.0"]);
        assert_eq!(candidate_patches("24", 3), vec!["24"], "单段版本不动");
        assert_eq!(candidate_patches("8.5.13", 1), vec!["8.5.13"]);
        assert_eq!(candidate_patches("8.5.13", 0), vec!["8.5.13"]);
    }

    #[test]
    fn node_index_groups_by_major_and_drops_prereleases() {
        let body = r#"[
            {"version":"v27.0.0-alpha.0.1.1","lts":false},
            {"version":"v24.22.3","lts":"Krypton"},
            {"version":"v24.22.0","lts":"Krypton"},
            {"version":"v24.9.0","lts":"Krypton"},
            {"version":"v25.1.0","lts":false},
            {"version":"v22.23.5","lts":"Jod"}
        ]"#;
        let index = node_versions_by_major(body).unwrap();
        assert_eq!(
            index["24"],
            vec!["24.22.3", "24.22.0", "24.9.0"],
            "按版本降序，预发布版不进"
        );
        assert_eq!(index["25"], vec!["25.1.0"]);
        assert!(!index.contains_key("27"), "v27.0.0-alpha 不是三段纯数字");
    }

    #[test]
    fn challenge_params_parses_bearer() {
        let params = challenge_params(
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/php:pull""#,
        );
        assert_eq!(params["service"], "registry.docker.io");
        assert_eq!(params["scope"], "repository:library/php:pull");
        assert!(challenge_params(r#"Basic realm="harbor""#).is_empty());
    }
}
