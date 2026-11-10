//! 目录拉取的传输层：`Transport` 只描述「带条件头的 GET」，真实实现走 ureq，
//! 单测注入假实现，绝不触网。

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

/// 响应体上限：目录 JSON 只有几十 KB，超过即视为异常源。
pub const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// 插件包 blob 的下载上限：与安装侧的 `DEFAULT_LIMITS.archive_bytes`（100MiB）同量级。
pub const MAX_BLOB_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct Conditional<'a> {
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub trait Transport: Send + Sync {
    fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String>;

    /// 二进制 blob 下载（安装链路用）。目录 JSON 的 8MiB 上限装不下插件包，
    /// 因此单独放宽；假传输层无需覆写，默认复用 `get`。
    fn get_blob(&self, url: &str) -> Result<HttpReply, String> {
        self.get(url, &Conditional::default())
    }
}

/// 真实实现：同步阻塞，调用方必须放在 `spawn_blocking`。
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new(timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(timeout)
            .redirects(3)
            .build();
        Self { agent }
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String> {
        let mut req = self.agent.get(url);
        if let Some(etag) = cond.etag {
            req = req.set("If-None-Match", etag);
        }
        if let Some(last) = cond.last_modified {
            req = req.set("If-Modified-Since", last);
        }
        match req.call() {
            Ok(resp) => Ok(read_reply(resp, MAX_BODY_BYTES)),
            // 条件请求的 304 与 4xx/5xx 都从 Status 分支返回，交给上层判定。
            Err(ureq::Error::Status(code, resp)) => {
                let mut reply = read_reply(resp, MAX_BODY_BYTES);
                reply.status = code;
                Ok(reply)
            }
            Err(ureq::Error::Transport(e)) => Err(e.to_string()),
        }
    }

    fn get_blob(&self, url: &str) -> Result<HttpReply, String> {
        match self.agent.get(url).call() {
            Ok(resp) => Ok(read_reply(resp, MAX_BLOB_BYTES)),
            Err(ureq::Error::Status(code, resp)) => {
                let mut reply = read_reply(resp, MAX_BLOB_BYTES);
                reply.status = code;
                Ok(reply)
            }
            Err(ureq::Error::Transport(e)) => Err(e.to_string()),
        }
    }
}

fn read_reply(resp: ureq::Response, limit: u64) -> HttpReply {
    let status = resp.status();
    let header = |name: &str| resp.header(name).map(str::to_string);
    let (etag, last_modified) = (header("etag"), header("last-modified"));
    let mut body = Vec::new();
    // 读失败按空体处理：上层会用摘要/大小校验兜底，不会把残缺内容当有效清单。
    let _ = resp.into_reader().take(limit).read_to_end(&mut body);
    HttpReply {
        status,
        body,
        etag,
        last_modified,
    }
}

/// 供 `AppState` 持有的可替换句柄。
pub type SharedTransport = Arc<dyn Transport>;

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeTransport {
        replies: Mutex<HashMap<String, Vec<HttpReply>>>,
        pub calls: Mutex<Vec<(String, ConditionalOwned)>>,
    }

    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ConditionalOwned {
        pub etag: Option<String>,
        pub last_modified: Option<String>,
    }

    impl FakeTransport {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// 为同一 URL 排队多个响应；用完后重复最后一个。
        pub fn push(&self, url: &str, reply: HttpReply) {
            self.replies
                .lock()
                .unwrap()
                .entry(url.to_string())
                .or_default()
                .push(reply);
        }

        pub fn replies_for(&self, url: &str) -> usize {
            self.replies
                .lock()
                .unwrap()
                .get(url)
                .map(Vec::len)
                .unwrap_or(0)
        }

        pub fn calls(&self) -> Vec<(String, ConditionalOwned)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: &str, cond: &Conditional<'_>) -> Result<HttpReply, String> {
            self.calls.lock().unwrap().push((
                url.to_string(),
                ConditionalOwned {
                    etag: cond.etag.map(str::to_string),
                    last_modified: cond.last_modified.map(str::to_string),
                },
            ));
            let mut map = self.replies.lock().unwrap();
            let queue = map
                .get_mut(url)
                .ok_or_else(|| format!("假传输层没有该地址: {url}"))?;
            if queue.is_empty() {
                return Err(format!("假传输层响应队列为空: {url}"));
            }
            // 按入队顺序消费；测试需要几次响应就按顺序 push 几次。
            Ok(queue.remove(0))
        }
    }
}
