//! 时间与本地时区注入。
//!
//! cron 表达式按宿主本地时间解释（契约 §3.3：P1 不提供时区选择）。为了让测试完全确定，
//! 时间与「本地时间 ↔ Unix 秒」的换算都走 [`Clock`]：生产用 [`SystemClock`]，
//! 测试用 [`ManualClock`]，DST 边界用测试内自定义时钟模拟。

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{LocalResult, NaiveDateTime, TimeZone};

pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
    /// Unix 秒 → 本地墙钟时间。
    fn to_local(&self, epoch: i64) -> NaiveDateTime;
    /// 本地墙钟时间 → Unix 秒；DST 跳过的整点返回 `None`（cron 直接跳过该次）。
    fn local_to_epoch(&self, local: NaiveDateTime) -> Option<i64>;
}

/// 生产时钟：跟随宿主本地时区与 DST 规则。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        chrono::Local::now().timestamp()
    }

    fn to_local(&self, epoch: i64) -> NaiveDateTime {
        chrono::Local
            .timestamp_opt(epoch, 0)
            .single()
            .map(|dt| dt.naive_local())
            .unwrap_or(NaiveDateTime::MIN)
    }

    fn local_to_epoch(&self, local: NaiveDateTime) -> Option<i64> {
        match chrono::Local.from_local_datetime(&local) {
            LocalResult::Single(dt) => Some(dt.timestamp()),
            // 回拨造成的重复时刻取较早一次，保证 cron 不会漏掉当天的触发
            LocalResult::Ambiguous(early, _) => Some(early.timestamp()),
            LocalResult::None => None,
        }
    }
}

/// 测试 / 回放用时钟：固定偏移（秒），时间可手动推进。
#[derive(Debug)]
pub struct ManualClock {
    now: AtomicI64,
    offset_secs: i32,
}

impl ManualClock {
    pub fn new(start: i64) -> Self {
        Self {
            now: AtomicI64::new(start),
            offset_secs: 0,
        }
    }

    /// `offset_secs` 是本地时间相对 UTC 的偏移（东八区为 `8 * 3600`）。
    pub fn with_offset(start: i64, offset_secs: i32) -> Self {
        Self {
            now: AtomicI64::new(start),
            offset_secs,
        }
    }

    pub fn set(&self, epoch: i64) {
        self.now.store(epoch, Ordering::SeqCst);
    }

    pub fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }

    pub fn offset_secs(&self) -> i32 {
        self.offset_secs
    }
}

impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn to_local(&self, epoch: i64) -> NaiveDateTime {
        chrono::DateTime::from_timestamp(epoch + self.offset_secs as i64, 0)
            .map(|dt| dt.naive_utc())
            .unwrap_or(NaiveDateTime::MIN)
    }

    fn local_to_epoch(&self, local: NaiveDateTime) -> Option<i64> {
        Some(local.and_utc().timestamp() - self.offset_secs as i64)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn naive(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mm, ss)
            .unwrap()
    }

    #[test]
    fn manual_clock_respects_offset() {
        let clock = ManualClock::with_offset(0, 8 * 3600);
        assert_eq!(clock.to_local(0), naive(1970, 1, 1, 8, 0, 0));
        let local = naive(2026, 11, 5, 3, 30, 0);
        assert_eq!(
            clock.local_to_epoch(local),
            Some(local.and_utc().timestamp() - 8 * 3600)
        );
        clock.set(100);
        clock.advance(50);
        assert_eq!(clock.now(), 150);
    }
}
