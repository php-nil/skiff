//! 同步 SQLite 的有界阻塞投递。
//!
//! `Db` 是 `std::sync::Mutex<rusqlite::Connection>`，同步调用如果直接跑在 tokio worker 上，
//! 一次慢查询（WAL checkpoint、大表清理）就会把 worker 占住；本机只有 2 个 worker，两个
//! 请求同时命中就会让 HTTP 与 `kernel.sock` 一起停摆（见 `docs/ARCH_REVIEW.md` §15）。
//!
//! 这里把「一段自包含的同步 DB 操作」投递到 `spawn_blocking`，并用全局信号量限制同时占用
//! 的阻塞线程数：SQLite 是单写者单连接，并发再高也只会在 `Db` 的互斥上排队，所以上限取
//! 与 worker 同阶的小值即可，避免 `spawn_blocking` 默认的巨大线程池把内存与磁盘都打满。
//!
//! **取消语义**：future 被丢弃（客户端断开 / 请求超时）后，已经投递的闭包会继续执行到结束，
//! 只是结果无人接收。DB 的写操作因此不会被截断，也不会留下半提交；permit 在闭包结束时释放。
//!
//! **语义边界**：闭包里只能放自包含的单条语句或完整事务（即 `Db` 现有方法），不要在里面
//! 再 await、也不要持有其它锁进来。

use std::cell::Cell;
use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

/// 同时占用阻塞线程的 DB 操作上限。实测单条语句亚毫秒级，4 已足够覆盖多入口并发；
/// 再多只会在 `Db` 的单连接互斥上排队，白占线程。
pub const MAX_CONCURRENT: usize = 4;

/// 排队等待上限。超过就直接失败并记一条 warn，不把请求无限期堆在信号量上：
/// 正常情况下单条语句亚毫秒级，等 2 s 只可能是长维护操作或异常，快速失败更容易被发现。
const QUEUE_WAIT: Duration = Duration::from_secs(2);

/// 排队超过这个时长就记一条 debug，用于观测「有界并发是否真的在排队」。
const QUEUE_LOG: Duration = Duration::from_millis(50);

static PERMITS: OnceLock<Semaphore> = OnceLock::new();

thread_local! {
    /// 当前线程是否正在执行本模块投递的 DB 闭包。用于拒绝「阻塞任务内部再投递」：
    /// 闭包里用 `block_on` 再进一次会占着 permit 等 permit，最终自锁。
    static IN_DB_BLOCK: Cell<bool> = const { Cell::new(false) };
}

fn permits() -> &'static Semaphore {
    PERMITS.get_or_init(|| Semaphore::new(MAX_CONCURRENT))
}

/// 投递失败：运行时关闭、排队超限、或闭包 panic。
#[derive(Debug)]
pub enum OffloadError {
    Closed,
    Busy { waited: Duration },
    Nested,
    Panic(String),
}

impl fmt::Display for OffloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OffloadError::Closed => write!(f, "运行时已关闭，DB 任务未能投递"),
            OffloadError::Busy { waited } => write!(
                f,
                "DB 阻塞队列已满（等待 {} ms 仍未拿到槽位）",
                waited.as_millis()
            ),
            OffloadError::Nested => write!(f, "禁止在 DB 阻塞任务内部再次投递 DB 任务"),
            OffloadError::Panic(e) => write!(f, "DB 阻塞任务异常退出：{e}"),
        }
    }
}

impl std::error::Error for OffloadError {}

/// 闭包内的哨兵：无论正常返回还是 panic，都把线程标记恢复。
struct BlockGuard;

impl Drop for BlockGuard {
    fn drop(&mut self) {
        IN_DB_BLOCK.with(|c| c.set(false));
    }
}

/// 在阻塞线程池里执行一段同步 DB 操作（受 [`MAX_CONCURRENT`] 约束）。
///
/// 返回值只包一层 `Result`：业务错误仍由闭包自己的 `Result` 表达，调用方的错误映射不用改。
///
/// - 超过并发上限时最多排队 [`QUEUE_WAIT`]；超时返回 [`OffloadError::Busy`] 并打 warn。
/// - 闭包里禁止再调本函数（返回 [`OffloadError::Nested`]），避免占着 permit 等 permit。
pub async fn run<T, F>(f: F) -> Result<T, OffloadError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if IN_DB_BLOCK.with(|c| c.get()) {
        tracing::error!("嵌套投递 DB 阻塞任务已被拒绝（会自锁）");
        return Err(OffloadError::Nested);
    }
    let queued_at = Instant::now();
    let permit = match tokio::time::timeout(QUEUE_WAIT, permits().acquire()).await {
        Ok(Ok(p)) => p,
        Ok(Err(_)) => return Err(OffloadError::Closed),
        Err(_) => {
            let waited = queued_at.elapsed();
            tracing::warn!(
                waited_ms = waited.as_millis(),
                limit = MAX_CONCURRENT,
                "DB 阻塞队列等待超限，本次调用快速失败"
            );
            return Err(OffloadError::Busy { waited });
        }
    };
    let waited = queued_at.elapsed();
    if waited >= QUEUE_LOG {
        tracing::debug!(
            waited_ms = waited.as_millis(),
            limit = MAX_CONCURRENT,
            "DB 阻塞任务排队后执行"
        );
    }
    tokio::task::spawn_blocking(move || {
        // permit 跟着闭包走：即使 future 被取消，也会在闭包真正结束时才归还。
        let _permit = permit;
        IN_DB_BLOCK.with(|c| c.set(true));
        let _guard = BlockGuard;
        f()
    })
    .await
    .map_err(|e| OffloadError::Panic(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// 并发上限生效：12 个任务同时投递，任一瞬间最多 `MAX_CONCURRENT` 个闭包在跑。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn caps_concurrent_blocking_tasks() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..12 {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                run(move || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    active.fetch_sub(1, Ordering::SeqCst);
                })
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak > 1, "并发未生效：peak={peak}");
        assert!(peak <= MAX_CONCURRENT, "超过上限：peak={peak}");
    }

    /// 客户端断开（future 被丢弃）后，已投递的写操作仍执行完，结果丢弃即可。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_future_still_completes_write() {
        let done = Arc::new(AtomicBool::new(false));
        let started = Arc::new(tokio::sync::Notify::new());
        let flag = Arc::clone(&done);
        let began = Arc::clone(&started);
        let mut fut = Box::pin(run(move || {
            began.notify_one();
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        // 等闭包真正开始执行，再丢弃 future：模拟「处理中客户端断开」。
        // 不能先 timeout —— 并发测试可能正占满 permit，超时会在投递前就取消。
        tokio::select! {
            _ = started.notified() => {}
            res = &mut fut => panic!("闭包不应在开始信号前结束：{res:?}"),
        }
        drop(fut);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            done.load(Ordering::SeqCst),
            "future 被丢弃后阻塞任务必须继续执行到结束"
        );
    }

    /// 闭包的业务错误原样返回，调用方的错误映射不需要改。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn business_error_passes_through() {
        let got: Result<Result<(), &str>, OffloadError> = run(|| Err("db unavailable")).await;
        assert_eq!(got.unwrap().unwrap_err(), "db unavailable");
    }

    /// 闭包 panic 被转成投递错误，不会拖垮调用方任务。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_becomes_offload_error() {
        let got: Result<(), OffloadError> = run(|| panic!("boom")).await;
        assert!(matches!(got, Err(OffloadError::Panic(_))));
    }

    /// 阻塞任务内部再投递会被拒绝：否则会占着 permit 等 permit 自锁。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nested_offload_is_rejected() {
        let got = run(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(run(|| 1)).unwrap_err()
        })
        .await
        .unwrap();
        assert!(matches!(got, OffloadError::Nested));
    }
}
