//! Webhook 投递（§17 R2a）：`skiff-server notify` 专用，API 进程不发起网络请求。
//!
//! 语义是 **at-least-once**：事件文件不可变，投递状态写旁路，接收方按
//! `X-Skiff-Event-Id` 去重。2xx 成功；网络错误/超时/408/429/5xx 重试；
//! 其余 4xx 直接进 dead-letter。签名使用独立域与独立 Ed25519 密钥。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::events::{self, Event};

pub const WEBHOOK_SIGNING_DOMAIN: &str = "SKIFF-SERVER-WEBHOOK-v1";
pub const ENV_WEBHOOK_KEY: &str = "SKIFF_WEBHOOK_SIGNING_KEY";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Subscription {
    fn matches(&self, event: &Event) -> bool {
        self.enabled
            && (self.events.is_empty() || self.events.iter().any(|e| e == &event.event_type))
            && (self.channels.is_empty() || self.channels.iter().any(|c| c == &event.channel))
    }
}

#[derive(Debug, Clone)]
pub struct NotifyOptions {
    pub timeout: Duration,
    pub max_attempts: u32,
    /// 第 N 次失败后的等待；不足时取最后一项。
    pub retry_delays: Vec<Duration>,
    pub allow_private: bool,
    /// 仅测试/本地联调允许 http://；生产默认只允许 https。
    pub allow_http: bool,
    pub pending_days: u64,
    pub dead_days: u64,
}

impl Default for NotifyOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            max_attempts: 8,
            retry_delays: vec![
                Duration::from_secs(5),
                Duration::from_secs(30),
                Duration::from_secs(120),
                Duration::from_secs(600),
                Duration::from_secs(3600),
                Duration::from_secs(6 * 3600),
            ],
            allow_private: false,
            allow_http: false,
            pending_days: 30,
            dead_days: 90,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct NotifyReport {
    pub delivered: usize,
    pub retried: usize,
    pub dead: usize,
    pub skipped: usize,
    pub pruned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeliveryState {
    event_id: String,
    subscription_id: String,
    attempts: u32,
    /// Unix 秒；到点前不重试。
    next_at: i64,
    delivered: bool,
    dead: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

pub fn load_config(path: &Path) -> Result<WebhookConfig> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("读取 webhooks 配置失败: {}", path.display()))?;
    let config: WebhookConfig = serde_json::from_str(&text)
        .with_context(|| format!("解析 webhooks 配置失败: {}", path.display()))?;
    let mut ids = std::collections::BTreeSet::new();
    for sub in &config.subscriptions {
        if sub.id.trim().is_empty() {
            bail!("webhook 订阅 id 不能为空");
        }
        if !ids.insert(sub.id.clone()) {
            bail!("webhook 订阅 id 重复: {}", sub.id);
        }
        check_url(&sub.url, true)?;
    }
    Ok(config)
}

pub fn signing_key_from_env() -> Result<SigningKey> {
    let text = std::env::var(ENV_WEBHOOK_KEY)
        .with_context(|| format!("缺少 {ENV_WEBHOOK_KEY}（64 位十六进制种子）"))?;
    let bytes = hex::decode(text.trim()).context("webhook 签名私钥不是合法十六进制")?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("webhook 签名私钥必须是 32 字节种子"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// 单轮投递：读取 pending、按订阅投递、更新状态、必要时移动事件。
pub fn run_once(
    state: &Path,
    config: &WebhookConfig,
    signing: &SigningKey,
    options: &NotifyOptions,
) -> Result<NotifyReport> {
    let _lock = NotifyLock::acquire(state)?;
    let mut report = NotifyReport {
        pruned: prune(state, options)?,
        ..NotifyReport::default()
    };

    let agent = ureq::AgentBuilder::new().timeout(options.timeout).build();
    let mut paths = list_json(&events::pending_dir(state))?;
    paths.sort();
    for path in paths {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(event) = serde_json::from_slice::<Event>(&bytes) else {
            // 坏文件不阻塞后续事件。
            move_file(&path, &events::dead_dir(state))?;
            report.dead += 1;
            continue;
        };
        let matching: Vec<&Subscription> = config
            .subscriptions
            .iter()
            .filter(|sub| sub.matches(&event))
            .collect();
        if matching.is_empty() {
            move_file(&path, &events::delivered_dir(state))?;
            report.skipped += 1;
            continue;
        }

        let mut all_done = true;
        let mut any_dead = false;
        for sub in matching {
            let mut delivery = load_delivery(state, &event.event_id, &sub.id)?;
            if delivery.delivered || delivery.dead {
                any_dead |= delivery.dead;
                continue;
            }
            let now = now_secs();
            if delivery.next_at > now {
                all_done = false;
                continue;
            }
            if let Err(message) = check_url(&sub.url, options.allow_http) {
                delivery.dead = true;
                delivery.last_error = Some(message.to_string());
                save_delivery(state, &delivery)?;
                any_dead = true;
                report.dead += 1;
                continue;
            }
            match deliver(&agent, sub, &event, signing) {
                Ok(()) => {
                    delivery.delivered = true;
                    delivery.attempts += 1;
                    delivery.last_status = Some(200);
                    save_delivery(state, &delivery)?;
                    report.delivered += 1;
                }
                Err(failure) => {
                    delivery.attempts += 1;
                    delivery.last_status = failure.status;
                    delivery.last_error = Some(failure.message);
                    if failure.retryable && delivery.attempts < options.max_attempts {
                        let delay = retry_delay(options, delivery.attempts, failure.retry_after);
                        delivery.next_at = now_secs() + delay.as_secs() as i64;
                        save_delivery(state, &delivery)?;
                        report.retried += 1;
                        all_done = false;
                    } else {
                        delivery.dead = true;
                        save_delivery(state, &delivery)?;
                        any_dead = true;
                        report.dead += 1;
                    }
                }
            }
        }

        if all_done {
            // 再确认一次所有订阅的最终状态（本轮可能跳过了已完成的订阅）。
            let mut complete = true;
            let mut dead = false;
            for sub in config.subscriptions.iter().filter(|s| s.matches(&event)) {
                let delivery = load_delivery(state, &event.event_id, &sub.id)?;
                if !(delivery.delivered || delivery.dead) {
                    complete = false;
                    break;
                }
                dead |= delivery.dead;
            }
            if complete {
                let target = if dead || any_dead {
                    events::dead_dir(state)
                } else {
                    events::delivered_dir(state)
                };
                move_file(&path, &target)?;
                cleanup_delivery(state, &event.event_id, config)?;
            }
        }
    }
    Ok(report)
}

struct Failure {
    retryable: bool,
    status: Option<u16>,
    message: String,
    retry_after: Option<Duration>,
}

fn deliver(
    agent: &ureq::Agent,
    sub: &Subscription,
    event: &Event,
    signing: &SigningKey,
) -> Result<(), Failure> {
    let body = serde_json::to_vec_pretty(event).map_err(|error| Failure {
        retryable: false,
        status: None,
        message: error.to_string(),
        retry_after: None,
    })?;
    let timestamp = now_secs().to_string();
    let digest = hex::encode(sha2::Sha256::digest(&body));
    let input = format!(
        "{WEBHOOK_SIGNING_DOMAIN}\0{}\0{timestamp}\0{digest}",
        event.event_id
    );
    let signature = hex::encode(signing.sign(input.as_bytes()).to_bytes());
    let result = agent
        .post(&sub.url)
        .set("content-type", "application/json")
        .set("x-skiff-event-id", &event.event_id)
        .set("x-skiff-timestamp", &timestamp)
        .set("x-skiff-signature", &format!("v1={signature}"))
        .send_bytes(&body);
    match result {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, response)) => {
            let retry_after = response
                .header("retry-after")
                .and_then(|value| value.parse::<u64>().ok())
                .map(|secs| Duration::from_secs(secs.min(6 * 3600)));
            Err(Failure {
                retryable: code == 408 || code == 429 || (500..600).contains(&code),
                status: Some(code),
                message: format!("http {code}"),
                retry_after,
            })
        }
        Err(ureq::Error::Transport(transport)) => Err(Failure {
            retryable: true,
            status: None,
            message: transport.to_string(),
            retry_after: None,
        }),
    }
}

fn retry_delay(options: &NotifyOptions, attempts: u32, retry_after: Option<Duration>) -> Duration {
    let index = attempts.saturating_sub(1) as usize;
    let base = options
        .retry_delays
        .get(index)
        .or_else(|| options.retry_delays.last())
        .copied()
        .unwrap_or(Duration::from_secs(30))
        .min(Duration::from_secs(6 * 3600));
    retry_after.map(|value| value.max(base)).unwrap_or(base)
}

fn check_url(url: &str, allow_http: bool) -> Result<()> {
    let rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if allow_http {
        url.strip_prefix("http://")
            .ok_or_else(|| anyhow::anyhow!("webhook URL 只允许 http(s)"))?
    } else {
        bail!("webhook URL 只允许 https");
    };
    let host = rest
        .split(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.is_empty() {
        bail!("webhook URL 缺少 host");
    }
    if !allow_http && is_private_host(host) {
        bail!("webhook URL 不得指向回环或私有地址: {host}（内部目标需显式放行）");
    }
    Ok(())
}

fn is_private_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        return true;
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

fn delivery_path(state: &Path, event_id: &str, sub_id: &str) -> PathBuf {
    events::state_dir(state).join(format!("{event_id}.{sub_id}.json"))
}

fn load_delivery(state: &Path, event_id: &str, sub_id: &str) -> Result<DeliveryState> {
    let path = delivery_path(state, event_id, sub_id);
    match fs::read(&path) {
        Ok(bytes) => Ok(
            serde_json::from_slice(&bytes).unwrap_or_else(|_| DeliveryState {
                event_id: event_id.to_string(),
                subscription_id: sub_id.to_string(),
                attempts: 0,
                next_at: 0,
                delivered: false,
                dead: false,
                last_status: None,
                last_error: None,
            }),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DeliveryState {
            event_id: event_id.to_string(),
            subscription_id: sub_id.to_string(),
            attempts: 0,
            next_at: 0,
            delivered: false,
            dead: false,
            last_status: None,
            last_error: None,
        }),
        Err(error) => Err(error).context("读取投递状态失败"),
    }
}

fn save_delivery(state: &Path, delivery: &DeliveryState) -> Result<()> {
    let dir = events::state_dir(state);
    fs::create_dir_all(&dir)?;
    let path = delivery_path(state, &delivery.event_id, &delivery.subscription_id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(delivery)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn cleanup_delivery(state: &Path, event_id: &str, config: &WebhookConfig) -> Result<()> {
    for sub in &config.subscriptions {
        let _ = fs::remove_file(delivery_path(state, event_id, &sub.id));
    }
    Ok(())
}

fn move_file(path: &Path, target_dir: &Path) -> Result<()> {
    fs::create_dir_all(target_dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("outbox 文件名缺失"))?;
    fs::rename(path, target_dir.join(name)).context("移动 outbox 事件失败")?;
    Ok(())
}

fn list_json(dir: &Path) -> Result<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    Ok(entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect())
}

/// 保留期：超期 pending 进 dead，超期 dead 删除。
fn prune(state: &Path, options: &NotifyOptions) -> Result<usize> {
    let mut pruned = 0;
    let now = SystemTime::now();
    for (dir, days, delete) in [
        (events::pending_dir(state), options.pending_days, false),
        (events::dead_dir(state), options.dead_days, true),
    ] {
        for path in list_json(&dir)? {
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let age = now.duration_since(modified).unwrap_or_default();
            if age < Duration::from_secs(days.saturating_mul(86400)) {
                continue;
            }
            if delete {
                let _ = fs::remove_file(&path);
            } else {
                move_file(&path, &events::dead_dir(state))?;
            }
            pruned += 1;
        }
    }
    Ok(pruned)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

struct NotifyLock(PathBuf);

impl NotifyLock {
    fn acquire(state: &Path) -> Result<Self> {
        let dir = state.join("outbox");
        fs::create_dir_all(&dir)?;
        let path = dir.join("notify.lock");
        // 崩溃遗留的锁超过 10 分钟视为过期。
        if let Ok(meta) = fs::metadata(&path) {
            if meta
                .modified()
                .ok()
                .and_then(|at| at.elapsed().ok())
                .map(|age| age > Duration::from_secs(600))
                .unwrap_or(false)
            {
                let _ = fs::remove_file(&path);
            }
        }
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(_) => Ok(Self(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("已有 notify 进程在运行")
            }
            Err(error) => Err(error).context("创建 notify 锁失败"),
        }
    }
}

impl Drop for NotifyLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use ed25519_dalek::{Signature, Verifier};

    use super::*;
    use crate::events::{EVENT_TYPE_SUBMISSION_CREATED, EVENT_TYPE_TAKEDOWN_YANKED};

    struct Reply {
        status: u16,
        retry_after: Option<u64>,
        sleep: Option<Duration>,
    }

    #[derive(Clone)]
    struct Record {
        headers: String,
        body: Vec<u8>,
    }

    fn spawn(responses: Vec<Reply>) -> (String, Arc<Mutex<Vec<Record>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        std::thread::spawn(move || {
            for reply in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break None;
                    };
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(pos + 4);
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + length {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                sink.lock().unwrap().push(Record {
                    headers: head,
                    body: buf[head_end..].to_vec(),
                });
                if let Some(sleep) = reply.sleep {
                    std::thread::sleep(sleep);
                }
                let mut response = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: 0\r\nConnection: close\r\n",
                    reply.status
                );
                if let Some(seconds) = reply.retry_after {
                    response.push_str(&format!("Retry-After: {seconds}\r\n"));
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://{addr}/hook"), records)
    }

    fn options() -> NotifyOptions {
        NotifyOptions {
            timeout: Duration::from_secs(2),
            max_attempts: 3,
            retry_delays: vec![Duration::ZERO],
            allow_private: true,
            allow_http: true,
            pending_days: 30,
            dead_days: 90,
        }
    }

    fn config(url: &str, events: &[&str]) -> WebhookConfig {
        WebhookConfig {
            schema: "skiff.server.webhooks/v1".to_string(),
            subscriptions: vec![Subscription {
                id: "ops".to_string(),
                url: url.to_string(),
                events: events.iter().map(|e| e.to_string()).collect(),
                channels: vec![],
                enabled: true,
            }],
        }
    }

    fn emit_event(state: &Path) {
        events::emit(
            state,
            &Event::new(EVENT_TYPE_SUBMISSION_CREATED, "sid-1", "stable")
                .actor(&"a".repeat(64))
                .subject("plugin", "com.example.demo", "1.0.0")
                .submission("sid-1"),
        )
        .unwrap();
    }

    #[test]
    fn delivers_and_signature_verifies() {
        let (url, records) = spawn(vec![Reply {
            status: 200,
            retry_after: None,
            sleep: None,
        }]);
        let dir = tempfile::tempdir().unwrap();
        emit_event(dir.path());
        let signing = SigningKey::from_bytes(&[7u8; 32]);

        let report = run_once(dir.path(), &config(&url, &[]), &signing, &options()).unwrap();
        assert_eq!(report.delivered, 1);
        assert_eq!(
            list_json(&events::pending_dir(dir.path())).unwrap().len(),
            0
        );

        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        let event: Event = serde_json::from_slice(&record.body).unwrap();
        let timestamp = header(&record.headers, "x-skiff-timestamp").unwrap();
        let signature = header(&record.headers, "x-skiff-signature").unwrap();
        let digest = hex::encode(sha2::Sha256::digest(&record.body));
        let input = format!(
            "{WEBHOOK_SIGNING_DOMAIN}\0{}\0{timestamp}\0{digest}",
            event.event_id
        );
        let signature = signature.strip_prefix("v1=").unwrap();
        let signature = Signature::from_slice(&hex::decode(signature).unwrap()).unwrap();
        signing
            .verifying_key()
            .verify(input.as_bytes(), &signature)
            .expect("签名必须可验");
        assert_eq!(
            header(&record.headers, "x-skiff-event-id").unwrap(),
            event.event_id
        );
    }

    #[test]
    fn retries_5xx_then_succeeds() {
        let (url, records) = spawn(vec![
            Reply {
                status: 500,
                retry_after: None,
                sleep: None,
            },
            Reply {
                status: 200,
                retry_after: None,
                sleep: None,
            },
        ]);
        let dir = tempfile::tempdir().unwrap();
        emit_event(dir.path());
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let config = config(&url, &[]);

        let first = run_once(dir.path(), &config, &signing, &options()).unwrap();
        assert_eq!(first.retried, 1);
        assert_eq!(
            list_json(&events::pending_dir(dir.path())).unwrap().len(),
            1
        );

        let second = run_once(dir.path(), &config, &signing, &options()).unwrap();
        assert_eq!(second.delivered, 1);
        assert_eq!(records.lock().unwrap().len(), 2);
        assert_eq!(
            list_json(&events::pending_dir(dir.path())).unwrap().len(),
            0
        );
    }

    #[test]
    fn does_not_retry_4xx() {
        let (url, records) = spawn(vec![Reply {
            status: 400,
            retry_after: None,
            sleep: None,
        }]);
        let dir = tempfile::tempdir().unwrap();
        emit_event(dir.path());
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let config = config(&url, &[]);

        let report = run_once(dir.path(), &config, &signing, &options()).unwrap();
        assert_eq!(report.dead, 1);
        assert_eq!(records.lock().unwrap().len(), 1);
        assert_eq!(list_json(&events::dead_dir(dir.path())).unwrap().len(), 1);
        run_once(dir.path(), &config, &signing, &options()).unwrap();
        assert_eq!(records.lock().unwrap().len(), 1, "4xx 不得重试");
    }

    #[test]
    fn timeout_is_retried_and_eventually_dead() {
        let (url, records) = spawn(vec![Reply {
            status: 200,
            retry_after: None,
            sleep: Some(Duration::from_millis(400)),
        }]);
        let dir = tempfile::tempdir().unwrap();
        emit_event(dir.path());
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let config = config(&url, &[]);
        let mut opts = options();
        opts.timeout = Duration::from_millis(50);
        opts.max_attempts = 2;

        let first = run_once(dir.path(), &config, &signing, &opts).unwrap();
        assert_eq!(first.retried, 1);
        let second = run_once(dir.path(), &config, &signing, &opts).unwrap();
        assert_eq!(second.dead, 1);
        assert_eq!(list_json(&events::dead_dir(dir.path())).unwrap().len(), 1);
        assert_eq!(records.lock().unwrap().len(), 1);
    }

    #[test]
    fn event_filters_skip_unmatched() {
        let (url, records) = spawn(vec![]);
        let dir = tempfile::tempdir().unwrap();
        emit_event(dir.path());
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let report = run_once(
            dir.path(),
            &config(&url, &[EVENT_TYPE_TAKEDOWN_YANKED]),
            &signing,
            &options(),
        )
        .unwrap();
        assert_eq!(report.skipped, 1);
        assert_eq!(records.lock().unwrap().len(), 0);
    }

    fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
        headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}
