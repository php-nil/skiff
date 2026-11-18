//! 镜像源（registry mirror）测速。
//!
//! 判据是**下载真实层数据的吞吐**，不是「响应快」：`/v2/` 对匿名请求回 401 或
//! 空 200，耗时毫秒级，与带宽无关；manifest 也只有几 KB。所以先解析 manifest
//! 拿到首层 digest，再对 blob 发 Range 请求读满 [`SAMPLE_BYTES`]。
//!
//! 结果只代表**本机此刻**的链路：不落库做自动决策、不做跨机器比较，选择权在
//! 用户。模型与口径见 `docs/impl/SOURCE_MIRRORS.md`。

use std::io::Read;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 单源采样字节数。不足它说明没下动，不能判「快」。
pub const SAMPLE_BYTES: u64 = 3 * 1024 * 1024;
/// 单源硬上限。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// 采样不足这么多字节一律判不可用——`APKINDEX` 那次的教训：小样本的瞬时速率
/// 会被首包延迟与边缘缓存主导，用它排序会自动把用户送到最慢的源。
pub const MIN_USABLE_BYTES: u64 = 512 * 1024;
/// 探测用的公共镜像：几乎所有 mirror 都代理它，且层大小稳定（数 MB）。
const PROBE_REPO: &str = "library/alpine";
const PROBE_TAG: &str = "latest";
const ACCEPT_MANIFEST: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    /// 源地址（已去掉尾部 `/`），用于把结果对回候选行。
    pub base: String,
    pub ok: bool,
    /// 这次实际读到多少字节——必须回传，否则「3 MB / 4 ms」这种缓存命中会被当成带宽。
    pub bytes: u64,
    /// 纯下载时长（不含 manifest 解析）。
    pub elapsed_ms: u64,
    pub bytes_per_sec: u64,
    /// 是否走了匿名 token 挑战。
    pub via_token: bool,
    pub error: Option<String>,
}

/// 同步探测一个源；调用方负责放到 `spawn_blocking`。
pub fn probe_registry(base: &str) -> ProbeResult {
    let base = base.trim().trim_end_matches('/').to_string();
    match sample_layer(&base) {
        Ok((bytes, elapsed, via_token)) => {
            let secs = elapsed.as_secs_f64().max(0.001);
            ProbeResult {
                base,
                ok: true,
                bytes,
                elapsed_ms: elapsed.as_millis() as u64,
                bytes_per_sec: (bytes as f64 / secs) as u64,
                via_token,
                error: None,
            }
        }
        Err(e) => ProbeResult {
            base,
            ok: false,
            bytes: 0,
            elapsed_ms: 0,
            bytes_per_sec: 0,
            via_token: false,
            error: Some(e),
        },
    }
}

fn sample_layer(base: &str) -> Result<(u64, Duration, bool), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(PROBE_TIMEOUT)
        .timeout_connect(Duration::from_secs(4))
        .redirects(3)
        .build();
    let manifest_url = format!("{base}/v2/{PROBE_REPO}/manifests/{PROBE_TAG}");

    let mut token: Option<String> = None;
    let mut via_token = false;
    let manifest = match get_json(&agent, &manifest_url, None) {
        Ok(v) => v,
        Err(ProbeError::Auth(challenge)) => {
            let t = fetch_token(&agent, &challenge, PROBE_REPO)?;
            let v = get_json(&agent, &manifest_url, Some(&t)).map_err(|e| e.text())?;
            via_token = true;
            token = Some(t);
            v
        }
        Err(e) => return Err(e.text()),
    };

    // `alpine:latest` 通常是 index（多架构），要再取一次子清单才是层列表。
    let digest = match first_layer(&manifest) {
        Some(d) => d,
        None => {
            let child = child_for_amd64(&manifest)
                .ok_or_else(|| "清单既无 layers，也没有 linux/amd64 子清单".to_string())?;
            let child_url = format!("{base}/v2/{PROBE_REPO}/manifests/{child}");
            let child_manifest =
                get_json(&agent, &child_url, token.as_deref()).map_err(|e| e.text())?;
            first_layer(&child_manifest).ok_or_else(|| "子清单无 layers".to_string())?
        }
    };

    let blob_url = format!("{base}/v2/{PROBE_REPO}/blobs/{digest}");
    let (bytes, elapsed) = read_blob(&agent, &blob_url, token.as_deref())?;
    Ok((bytes, elapsed, via_token))
}

/// 读满 [`SAMPLE_BYTES`] 或读尽；中途超时按已读字节结算（慢源也要给出数字）。
fn read_blob(
    agent: &ureq::Agent,
    url: &str,
    token: Option<&str>,
) -> Result<(u64, Duration), String> {
    let mut req = agent
        .get(url)
        .set("Range", &format!("bytes=0-{}", SAMPLE_BYTES - 1))
        .timeout(PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        // 忽略 Range 直接 200 也是合法响应，只是会读多些字节。
        Err(ureq::Error::Status(code, _)) => return Err(format!("blob 请求 HTTP {code}")),
        Err(ureq::Error::Transport(e)) => return Err(e.to_string()),
    };

    let started = Instant::now();
    let mut reader = resp.into_reader().take(SAMPLE_BYTES);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total += n as u64,
            // 读到一半超时/断开：已读字节仍能反映速率，不丢弃。
            Err(_) => break,
        }
    }
    let elapsed = started.elapsed();
    if total < MIN_USABLE_BYTES {
        return Err(format!(
            "只读到 {total} 字节（需 ≥{}），判为不可用",
            MIN_USABLE_BYTES
        ));
    }
    Ok((total, elapsed))
}

enum ProbeError {
    /// 401 + `WWW-Authenticate` 挑战，需要换 token 重试。
    Auth(String),
    Other(String),
}

impl ProbeError {
    fn text(self) -> String {
        match self {
            ProbeError::Auth(ch) => format!("需要认证但未能取得 token：{ch}"),
            ProbeError::Other(e) => e,
        }
    }
}

fn get_json(
    agent: &ureq::Agent,
    url: &str,
    token: Option<&str>,
) -> Result<serde_json::Value, ProbeError> {
    let mut req = agent.get(url).set("Accept", ACCEPT_MANIFEST);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401, resp)) => {
            return Err(ProbeError::Auth(
                resp.header("www-authenticate").unwrap_or("").to_string(),
            ))
        }
        Err(ureq::Error::Status(code, _)) => return Err(ProbeError::Other(format!("HTTP {code}"))),
        Err(ureq::Error::Transport(e)) => return Err(ProbeError::Other(e.to_string())),
    };
    let mut body = Vec::new();
    // manifest 正常只有几 KB；上限防止 mirror 返回 HTML 错误页把内存吃满。
    let _ = resp.into_reader().take(512 * 1024).read_to_end(&mut body);
    serde_json::from_slice(&body).map_err(|e| {
        // 部分 mirror 对不存在的仓库回 HTML 200，这里必须判不可用而不是「空清单」。
        ProbeError::Other(format!("响应不是 JSON（{e}），可能是镜像未代理该仓库"))
    })
}

/// 用 `WWW-Authenticate: Bearer realm=...,service=...` 换匿名 token。
fn fetch_token(agent: &ureq::Agent, challenge: &str, repo: &str) -> Result<String, String> {
    let realm = challenge_field(challenge, "realm")
        .ok_or_else(|| format!("认证挑战缺少 realm：{challenge}"))?;
    let mut url = format!("{realm}?scope=repository:{repo}:pull");
    if let Some(service) = challenge_field(challenge, "service") {
        url = format!("{realm}?service={service}&scope=repository:{repo}:pull");
    }
    let resp = agent
        .get(&url)
        .timeout(PROBE_TIMEOUT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("取 token HTTP {code}"),
            ureq::Error::Transport(t) => t.to_string(),
        })?;
    let mut body = Vec::new();
    let _ = resp.into_reader().take(64 * 1024).read_to_end(&mut body);
    let v: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| format!("token 响应不是 JSON：{e}"))?;
    v["token"]
        .as_str()
        .or_else(|| v["access_token"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "token 响应里没有 token 字段".to_string())
}

/// 从 `Bearer realm="x",service="y"` 里取字段值。
fn challenge_field(challenge: &str, key: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let at = lower.find(&format!("{key}="))? + key.len() + 1;
    let rest = challenge.get(at..)?.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        return stripped.find('"').map(|end| stripped[..end].to_string());
    }
    rest.split([',', ' '])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_layer(manifest: &serde_json::Value) -> Option<String> {
    manifest["layers"]
        .as_array()?
        .first()?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}

fn child_for_amd64(index: &serde_json::Value) -> Option<String> {
    index["manifests"]
        .as_array()?
        .iter()
        .find(|m| {
            m["platform"]["os"].as_str() == Some("linux")
                && m["platform"]["architecture"].as_str() == Some("amd64")
        })?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_fields_parse_quoted_and_bare() {
        let ch = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io""#;
        assert_eq!(
            challenge_field(ch, "realm").as_deref(),
            Some("https://auth.docker.io/token")
        );
        assert_eq!(
            challenge_field(ch, "service").as_deref(),
            Some("registry.docker.io")
        );
        assert_eq!(
            challenge_field("Bearer realm=plain", "realm").as_deref(),
            Some("plain")
        );
        assert!(challenge_field("Bearer", "realm").is_none());
    }

    #[test]
    fn layer_is_taken_from_manifest_or_index() {
        let m = serde_json::json!({ "layers": [{ "digest": "sha256:aaa" }] });
        assert_eq!(first_layer(&m).as_deref(), Some("sha256:aaa"));
        assert!(first_layer(&serde_json::json!({ "manifests": [] })).is_none());

        let index = serde_json::json!({ "manifests": [
            { "digest": "sha256:arm", "platform": { "os": "linux", "architecture": "arm64" } },
            { "digest": "sha256:amd", "platform": { "os": "linux", "architecture": "amd64" } }
        ]});
        assert_eq!(child_for_amd64(&index).as_deref(), Some("sha256:amd"));
        // 只有 arm 时不能瞎选一个：宁可判不可用，也不要拿不同架构的层去比速度。
        let arm_only = serde_json::json!({ "manifests": [
            { "digest": "sha256:arm", "platform": { "os": "linux", "architecture": "arm64" } }
        ]});
        assert!(child_for_amd64(&arm_only).is_none());
    }

    #[test]
    fn probe_result_never_reports_zero_as_fast() {
        // 不可达：ok=false 且不带速率，UI 才能区分「没测到」与「0 KB/s」。
        let r = probe_registry("http://127.0.0.1:1");
        assert!(!r.ok, "{r:?}");
        assert_eq!(r.bytes_per_sec, 0);
        assert!(r.error.is_some());
    }
}
