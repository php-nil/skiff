//! 调度表达式求值：把 [`Schedule`] 解释成确定的「下一个触发边界 + 抖动」。
//!
//! 两个刻意的设计：
//!
//! 1. 边界与抖动分开存（`next_boundary` / `next_due`）。cron 抖动最大 600s，可能跨过
//!    下一个分钟边界，若只记抖动后的时间会导致漏触发或重复触发。
//! 2. 抖动是 `task_id + boundary` 的确定性哈希，不用随机数——同一任务每一个触发点
//!    抖动稳定，重放/测试可复现，也不会因为进程重启而整点堆叠变回原样。

#[cfg(test)]
use chrono::{DateTime, NaiveDateTime};
use skiff_cron::CronExpr;

use crate::clock::Clock;
use crate::model::Schedule;

/// cron 命中本地不存在时刻（DST 跳过）时的最大重试次数；正常表达式 0–2 次即命中。
const MAX_SKIPPED_LOCAL_MATCHES: usize = 64;

pub(crate) enum ParsedSchedule {
    Cron { expr: CronExpr, jitter_secs: u64 },
    Interval { secs: i64 },
}

impl ParsedSchedule {
    pub(crate) fn parse(schedule: &Schedule) -> Result<Self, String> {
        match schedule {
            Schedule::Cron {
                expr, jitter_secs, ..
            } => {
                let parsed =
                    CronExpr::parse(expr).ok_or_else(|| format!("cron 表达式无法解析：{expr}"))?;
                Ok(Self::Cron {
                    expr: parsed,
                    jitter_secs: *jitter_secs,
                })
            }
            Schedule::Interval { secs, .. } => Ok(Self::Interval {
                secs: (*secs).max(1) as i64,
            }),
        }
    }

    /// 严格晚于 `after` 的第一个调度边界（不带抖动）。
    pub(crate) fn next_boundary_after(&self, clock: &dyn Clock, after: i64) -> Option<i64> {
        match self {
            Self::Interval { secs } => Some((after.div_euclid(*secs) + 1) * secs),
            Self::Cron { expr, .. } => {
                let mut local = clock.to_local(after);
                for _ in 0..MAX_SKIPPED_LOCAL_MATCHES {
                    let next_local = expr.next_after(local)?;
                    if let Some(epoch) = clock.local_to_epoch(next_local) {
                        if epoch > after {
                            return Some(epoch);
                        }
                    }
                    // DST 跳过的本地时刻或换算后仍不前进：继续找下一次命中。
                    local = next_local;
                }
                None
            }
        }
    }

    /// 边界对应的实际触发时间：cron 加确定性抖动，间隔不加。
    pub(crate) fn due_at(&self, boundary: i64, task_id: &str) -> i64 {
        match self {
            Self::Cron { jitter_secs, .. } => {
                boundary + jitter_offset(task_id, boundary, *jitter_secs)
            }
            Self::Interval { .. } => boundary,
        }
    }

    /// 从 `from_boundary` 起统计 `due <= now` 的触发次数，并返回最后一个被计入的边界。
    ///
    /// `scan_cap` 防止「停机一年后每分钟任务」把 tick 拖死；超过上限时次数饱和，
    /// 调用方按缺失量记录 missed_count（宁可少报也不阻塞）。
    pub(crate) fn count_due(
        &self,
        clock: &dyn Clock,
        from_boundary: i64,
        task_id: &str,
        now: i64,
        scan_cap: u64,
    ) -> (u64, i64) {
        match self {
            Self::Interval { secs } => {
                if now < from_boundary {
                    return (0, from_boundary);
                }
                let count = (now - from_boundary) / *secs + 1;
                let last = from_boundary + (count - 1) * *secs;
                (count as u64, last)
            }
            Self::Cron { .. } => {
                let mut count = 0u64;
                let mut boundary = from_boundary;
                let mut last = from_boundary;
                for _ in 0..scan_cap {
                    if self.due_at(boundary, task_id) > now {
                        break;
                    }
                    count += 1;
                    last = boundary;
                    match self.next_boundary_after(clock, boundary) {
                        Some(next) => boundary = next,
                        None => break,
                    }
                }
                (count, last)
            }
        }
    }

    /// 首次排期：`now` 之后的下一个边界与实际触发时间。
    pub(crate) fn first_after(
        &self,
        clock: &dyn Clock,
        now: i64,
        task_id: &str,
    ) -> Option<(i64, i64)> {
        let boundary = self.next_boundary_after(clock, now)?;
        Some((boundary, self.due_at(boundary, task_id)))
    }
}

/// FNV-1a：任务 id + 边界 → `[0, max]` 内的稳定偏移。
fn jitter_offset(task_id: &str, boundary: i64, max: u64) -> i64 {
    if max == 0 {
        return 0;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in task_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for byte in boundary.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % (max + 1)) as i64
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::clock::{Clock, ManualClock};
    use crate::model::Schedule;

    fn naive(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mm, ss)
            .unwrap()
    }

    fn parsed(schedule: &Schedule) -> ParsedSchedule {
        ParsedSchedule::parse(schedule).unwrap()
    }

    #[test]
    fn interval_aligns_to_epoch() {
        let clock = ManualClock::new(0);
        let schedule = parsed(&Schedule::interval(10));
        assert_eq!(schedule.next_boundary_after(&clock, 100), Some(110));
        assert_eq!(schedule.next_boundary_after(&clock, 109), Some(110));
        assert_eq!(schedule.count_due(&clock, 10, "t", 55, 10_000), (5, 50));
        assert_eq!(schedule.count_due(&clock, 10, "t", 9, 10_000), (0, 10));
    }

    #[test]
    fn cron_uses_injected_local_timezone() {
        // UTC+8：本地 2026-11-05 03:29:30
        let offset: i32 = 8 * 3600;
        let start = naive(2026, 11, 5, 3, 29, 30).and_utc().timestamp() - i64::from(offset);
        let clock = ManualClock::with_offset(start, offset);
        let schedule = parsed(&Schedule::cron("30 3 * * *"));
        let (boundary, due) = schedule.first_after(&clock, start, "daily").unwrap();
        assert_eq!(clock.to_local(boundary), naive(2026, 11, 5, 3, 30, 0));
        assert_eq!(due, boundary, "没有抖动时 due 与边界一致");
        assert_eq!(
            schedule.next_boundary_after(&clock, boundary),
            Some(boundary + 24 * 3600)
        );
    }

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let schedule = parsed(&Schedule::Cron {
            expr: "30 3 * * *".to_string(),
            tz: crate::model::CronTz::Local,
            jitter_secs: 120,
        });
        let boundary = 1_000_000;
        let first = schedule.due_at(boundary, "backup-day");
        let second = schedule.due_at(boundary, "backup-day");
        assert_eq!(first, second);
        assert!((boundary..=boundary + 120).contains(&first));
        let other = schedule.due_at(boundary, "another-task");
        assert!((boundary..=boundary + 120).contains(&other));
        // 相邻边界抖动独立，不放大到跨越周期
        let next = schedule.due_at(boundary + 86400, "backup-day");
        assert!((boundary + 86400..=boundary + 86400 + 120).contains(&next));
    }

    /// 模拟春季 DST 跳变：跳变前 UTC+0，跳变后 UTC+1，本地 02:00–03:00 不存在。
    struct DstClock {
        transition: i64,
    }

    impl Clock for DstClock {
        fn now(&self) -> i64 {
            self.transition
        }

        fn to_local(&self, epoch: i64) -> NaiveDateTime {
            let offset = if epoch < self.transition { 0 } else { 3600 };
            DateTime::from_timestamp(epoch + offset, 0)
                .map(|dt| dt.naive_utc())
                .unwrap()
        }

        fn local_to_epoch(&self, local: NaiveDateTime) -> Option<i64> {
            let secs = local.and_utc().timestamp();
            for offset in [0i64, 3600] {
                let epoch = secs - offset;
                let actual = if epoch < self.transition { 0 } else { 3600 };
                if actual == offset {
                    return Some(epoch);
                }
            }
            None
        }
    }

    #[test]
    fn cron_skips_nonexistent_local_time_across_dst_gap() {
        // 跳变时刻 2026-03-08 02:00 UTC（本地时间从 02:00 直接跳到 03:00）
        let transition = naive(2026, 3, 8, 2, 0, 0).and_utc().timestamp();
        let clock = DstClock { transition };
        // 02:30 在跳变当天不存在
        assert_eq!(clock.local_to_epoch(naive(2026, 3, 8, 2, 30, 0)), None);

        let schedule = parsed(&Schedule::cron("30 2 * * *"));
        let start = naive(2026, 3, 7, 3, 0, 0).and_utc().timestamp();
        let next = schedule.next_boundary_after(&clock, start).unwrap();
        // 跳过 3/8 的不存在时刻，落在 3/9 本地 02:30
        assert_eq!(clock.to_local(next), naive(2026, 3, 9, 2, 30, 0));
    }
}
