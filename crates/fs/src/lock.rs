//! 路径锁（契约 §3.4）：同路径串行、跨路径并行、`rename/copy` 双路径按字典序加锁。
//!
//! 实现是进程内的键控读写锁表：key 为 lexical 归一后的绝对路径。等待有上限
//! （默认 5s），超时返回 `fs.busy`，绝不无限阻塞。锁表按需创建、引用归零即回收。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, TryLockError};
use std::time::{Duration, Instant};

use crate::error::FsError;

/// 契约 §3.4 的锁等待上限。
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// 轮询间隔：std 没有带超时的 `RwLock`，用短轮询换取可移植性。
const RETRY: Duration = Duration::from_millis(2);

#[derive(Debug, Default)]
pub struct PathLocks {
    inner: Mutex<HashMap<PathBuf, Arc<RwLock<()>>>>,
}

impl PathLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// 读-读并行。多个路径按字典序加锁，避免交叉加锁死锁。
    pub fn with_read<T>(
        &self,
        paths: &[PathBuf],
        timeout: Duration,
        f: impl FnOnce() -> T,
    ) -> Result<T, FsError> {
        self.with(paths, timeout, false, f)
    }

    /// 写-写、写-读串行。`rename/copy` 传 `[src, dst]`，内部会排序。
    pub fn with_write<T>(
        &self,
        paths: &[PathBuf],
        timeout: Duration,
        f: impl FnOnce() -> T,
    ) -> Result<T, FsError> {
        self.with(paths, timeout, true, f)
    }

    fn with<T>(
        &self,
        paths: &[PathBuf],
        timeout: Duration,
        write: bool,
        f: impl FnOnce() -> T,
    ) -> Result<T, FsError> {
        if paths.is_empty() {
            return Ok(f());
        }
        let arcs = self.acquire_entries(paths);
        let deadline = Instant::now() + timeout;
        let mut guards: Vec<LockGuard<'_>> = Vec::with_capacity(arcs.len());
        for arc in &arcs {
            guards.push(acquire_one(arc, write, deadline, paths)?);
        }
        let out = f();
        drop(guards);
        drop(arcs);
        self.release_entries();
        Ok(out)
    }

    /// 只拿锁表里的 `Arc`，不持有 map 锁去等待路径锁（否则会把整张表串行化）。
    fn acquire_entries(&self, paths: &[PathBuf]) -> Vec<Arc<RwLock<()>>> {
        let mut keys: Vec<PathBuf> = paths.to_vec();
        keys.sort();
        keys.dedup();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        keys.into_iter()
            .map(|k| {
                map.entry(k)
                    .or_insert_with(|| Arc::new(RwLock::new(())))
                    .clone()
            })
            .collect()
    }

    fn release_entries(&self) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, v| Arc::strong_count(v) > 1);
    }

    #[cfg(test)]
    pub fn tracked_paths(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// 两种 guard 合并成一个枚举，避免把 `with` 写成两份泛型代码。
/// guard 只用于 RAII，字段本身不被读取。
#[allow(dead_code)]
enum LockGuard<'a> {
    Read(std::sync::RwLockReadGuard<'a, ()>),
    Write(std::sync::RwLockWriteGuard<'a, ()>),
}

fn acquire_one<'a>(
    arc: &'a Arc<RwLock<()>>,
    write: bool,
    deadline: Instant,
    paths: &[PathBuf],
) -> Result<LockGuard<'a>, FsError> {
    loop {
        if write {
            match arc.try_write() {
                Ok(g) => return Ok(LockGuard::Write(g)),
                Err(TryLockError::Poisoned(e)) => return Ok(LockGuard::Write(e.into_inner())),
                Err(TryLockError::WouldBlock) => {}
            }
        } else {
            match arc.try_read() {
                Ok(g) => return Ok(LockGuard::Read(g)),
                Err(TryLockError::Poisoned(e)) => return Ok(LockGuard::Read(e.into_inner())),
                Err(TryLockError::WouldBlock) => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(FsError::Busy(format!(
                "路径锁等待超时: {}",
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        std::thread::sleep(RETRY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn same_path_writes_are_serialized() {
        let locks = Arc::new(PathLocks::new());
        let path = PathBuf::from("/tmp/lock-a");
        let (hold_tx, hold_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let l1 = locks.clone();
        let p1 = path.clone();
        let t = std::thread::spawn(move || {
            l1.with_write(&[p1], DEFAULT_LOCK_TIMEOUT, || {
                done_tx.send(()).unwrap();
                hold_rx.recv().unwrap();
            })
            .unwrap();
        });
        done_rx.recv().unwrap();

        let l2 = locks.clone();
        let p2 = path.clone();
        let busy = l2
            .with_write(&[p2], Duration::from_millis(10), || ())
            .unwrap_err();
        assert_eq!(busy.code(), "fs.busy");

        hold_tx.send(()).unwrap();
        t.join().unwrap();
        // 释放后同路径可再次加锁
        locks
            .with_write(&[path], DEFAULT_LOCK_TIMEOUT, || ())
            .unwrap();
        assert_eq!(locks.tracked_paths(), 0);
    }

    #[test]
    fn different_paths_run_in_parallel() {
        let locks = PathLocks::new();
        let a = PathBuf::from("/tmp/lock-b");
        let b = PathBuf::from("/tmp/lock-c");
        let got = locks
            .with_read(&[a], Duration::from_millis(100), || 1)
            .unwrap()
            + locks
                .with_write(&[b], Duration::from_millis(100), || 2)
                .unwrap();
        assert_eq!(got, 3);
    }
}
