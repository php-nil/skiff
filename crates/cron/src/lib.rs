//! 5 段 cron 表达式的解析 / 匹配 / 下一次触发时间。
//!
//! 这份实现被三处共用，避免各写一套后漂移：
//! - 内核 `plugin.cron` 调度（manifest `cron`）；
//! - `skiff-pkgfmt` 的 manifest 校验（装包时就拒绝非法表达式）；
//! - 官方「计划任务」插件（用户输入的表达式与 UI 的「下一次运行」）。
//!
//! 支持 `分 时 日 月 周`，语法为 `*`、`?`、单值、`a-b`、步长（`*/n`、`a-b/n`、`a/n`）
//! 与逗号列表；不做秒级、不做 `@reboot` 这类扩展。日与周按 crontab 惯例取**或**，
//! 周字段 0 与 7 都表示周日。

use chrono::{Datelike, NaiveDateTime, TimeDelta, Timelike};

pub mod preset;
pub use preset::{describe_expr, detect_preset, Preset};

/// 最多向前搜索 8 年：覆盖 `2 月 29 日` 这类闰年表达式，且不会让 UI 卡住。
const MAX_SEARCH_MINUTES: i64 = 8 * 366 * 24 * 60;

#[derive(Debug, Clone)]
pub struct CronExpr {
    minute: Vec<bool>,
    hour: Vec<bool>,
    dom: Vec<bool>,
    month: Vec<bool>,
    dow: Vec<bool>,
    dom_any: bool,
    dow_any: bool,
}

impl CronExpr {
    /// 解析表达式；任何字段越界或格式不支持都返回 `None`（调用方跳过该条）。
    pub fn parse(expr: &str) -> Option<CronExpr> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return None;
        }
        let (minute, _) = field(fields[0], 0, 59)?;
        let (hour, _) = field(fields[1], 0, 23)?;
        let (dom, dom_any) = field(fields[2], 1, 31)?;
        let (month, _) = field(fields[3], 1, 12)?;
        let (dow, dow_any) = field(fields[4], 0, 7)?;
        Some(CronExpr {
            minute,
            hour,
            dom,
            month,
            dow,
            dom_any,
            dow_any,
        })
    }

    pub fn matches(&self, t: &NaiveDateTime) -> bool {
        if !self
            .minute
            .get(t.minute() as usize)
            .copied()
            .unwrap_or(false)
        {
            return false;
        }
        if !self.hour.get(t.hour() as usize).copied().unwrap_or(false) {
            return false;
        }
        if !self
            .month
            .get((t.month() as usize).wrapping_sub(1))
            .copied()
            .unwrap_or(false)
        {
            return false;
        }

        let w = t.weekday().num_days_from_sunday() as usize;
        let dom_ok = self
            .dom
            .get((t.day() as usize).wrapping_sub(1))
            .copied()
            .unwrap_or(false);
        // 7 与 0 都是周日：两者任一为真都算命中
        let dow_ok = self.dow.get(w).copied().unwrap_or(false)
            || (w == 0 && self.dow.get(7).copied().unwrap_or(false));

        match (self.dom_any, self.dow_any) {
            (true, true) => true,
            (false, true) => dom_ok,
            (true, false) => dow_ok,
            (false, false) => dom_ok || dow_ok,
        }
    }

    /// 严格在 `from` 之后（下一分钟起）的第一个命中时间；找不到时返回 `None`。
    ///
    /// UI 用来展示「下一次运行」；搜索上限见 [`MAX_SEARCH_MINUTES`]。
    pub fn next_after(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        let start = from
            .with_second(0)?
            .with_nanosecond(0)?
            .checked_add_signed(TimeDelta::minutes(1))?;
        let mut t = start;
        for _ in 0..MAX_SEARCH_MINUTES {
            if self.matches(&t) {
                return Some(t);
            }
            t = t.checked_add_signed(TimeDelta::minutes(1))?;
        }
        None
    }
}

/// 单条表达式的匹配便捷函数：非法表达式恒为 `false`（不 panic、不抛错）。
pub fn expr_matches(expr: &str, t: &NaiveDateTime) -> bool {
    CronExpr::parse(expr).map(|e| e.matches(t)).unwrap_or(false)
}

/// 校验表达式是否可被调度（供 manifest 与计划任务表单共用）。
pub fn is_valid(expr: &str) -> bool {
    CronExpr::parse(expr).is_some()
}

/// 解析一个字段为「按值索引的布尔表」，同时返回它是否等价于 `*`。
fn field(spec: &str, min: u32, max: u32) -> Option<(Vec<bool>, bool)> {
    let mut set = vec![false; (max - min + 1) as usize];
    if spec.is_empty() {
        return None;
    }
    let mut any = false;
    for item in spec.split(',') {
        let item_any = expand(item, min, max, &mut set)?;
        any |= item_any;
    }
    // `a-b` 恰好覆盖整段时语义也等价于 `*`（`0-59` 与 `*` 在「日/周」语义上相同）
    let all = set.iter().all(|v| *v);
    Some((set, any || all))
}

/// 展开一个逗号项；返回该项是否是 `*`/`?`。
fn expand(item: &str, min: u32, max: u32, set: &mut [bool]) -> Option<bool> {
    let (range, step) = match item.split_once('/') {
        Some((r, s)) => (r, s.parse::<u32>().ok()?),
        None => (item, 1),
    };
    if step == 0 {
        return None;
    }
    let mark = |set: &mut [bool], lo: u32, hi: u32, step: u32| -> Option<()> {
        let mut v = lo;
        while v <= hi {
            set[(v - min) as usize] = true;
            v = v.checked_add(step)?;
        }
        Some(())
    };
    match range {
        "*" | "?" => {
            mark(set, min, max, step)?;
            Some(true)
        }
        _ => {
            if let Some((a, b)) = range.split_once('-') {
                let a: u32 = a.parse().ok()?;
                let b: u32 = b.parse().ok()?;
                if a > b || a < min || b > max {
                    return None;
                }
                mark(set, a, b, step)?;
            } else {
                let v: u32 = range.parse().ok()?;
                if !(min..=max).contains(&v) {
                    return None;
                }
                // 单值带步长（`5/10`）按标准 cron 是「5 起每 10 个」
                let hi = if item.contains('/') { max } else { v };
                mark(set, v, hi, step)?;
            }
            Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mm, 0)
            .unwrap()
    }

    #[test]
    fn every_minute_matches() {
        let e = CronExpr::parse("* * * * *").unwrap();
        assert!(e.matches(&at(2026, 11, 1, 12, 0)));
        assert!(e.matches(&at(2026, 11, 1, 12, 59)));
        assert!(e.matches(&at(2026, 11, 1, 23, 59)));
    }

    #[test]
    fn step_and_fixed_values() {
        let e = CronExpr::parse("*/5 * * * *").unwrap();
        assert!(e.matches(&at(2026, 11, 1, 10, 0)));
        assert!(e.matches(&at(2026, 11, 1, 10, 55)));
        assert!(!e.matches(&at(2026, 11, 1, 10, 56)));

        let e = CronExpr::parse("30 3 * * *").unwrap();
        assert!(e.matches(&at(2026, 11, 1, 3, 30)));
        assert!(!e.matches(&at(2026, 11, 1, 4, 30)));
        assert!(!e.matches(&at(2026, 11, 1, 3, 31)));
    }

    #[test]
    fn lists_and_ranges() {
        let e = CronExpr::parse("0,20,40 8-10 * * *").unwrap();
        assert!(e.matches(&at(2026, 11, 1, 9, 20)));
        assert!(!e.matches(&at(2026, 11, 1, 11, 20)));
        assert!(!e.matches(&at(2026, 11, 1, 9, 10)));
        // 单值带步长：5 起、每 10 分钟
        let e = CronExpr::parse("5/10 * * * *").unwrap();
        assert!(e.matches(&at(2026, 11, 1, 1, 45)));
        assert!(!e.matches(&at(2026, 11, 1, 1, 46)));
    }

    /// 日/周都受限时取「或」：13 号周五之外的日子也要触发（crontab 惯例）。
    #[test]
    fn day_and_weekday_are_or_semantics() {
        let e = CronExpr::parse("0 0 13 * 5").unwrap();
        // 2026-11-13 是周五：两者都命中
        assert!(e.matches(&at(2026, 11, 13, 0, 0)));
        // 2026-11-06 是周五但不是 13 号：周命中
        assert!(e.matches(&at(2026, 11, 6, 0, 0)));
        // 2026-11-15 是周日也不是 13 号：都不命中
        assert!(!e.matches(&at(2026, 11, 15, 0, 0)));
    }

    /// 周字段 0 与 7 都是周日。
    #[test]
    fn sunday_is_both_zero_and_seven() {
        let e0 = CronExpr::parse("0 0 * * 0").unwrap();
        let e7 = CronExpr::parse("0 0 * * 7").unwrap();
        assert!(e0.matches(&at(2026, 11, 1, 0, 0)) && e7.matches(&at(2026, 11, 1, 0, 0)));
        assert!(!e0.matches(&at(2026, 11, 2, 0, 0)));
    }

    #[test]
    fn invalid_expressions_never_fire() {
        for bad in [
            "",
            "* * * *",
            "60 * * * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "@reboot",
        ] {
            assert!(
                !expr_matches(bad, &at(2026, 11, 1, 0, 0)),
                "{bad:?} 不该命中"
            );
            assert!(CronExpr::parse(bad).is_none(), "{bad:?} 不该解析成功");
            assert!(!is_valid(bad), "{bad:?} 不该通过校验");
        }
        assert!(is_valid("*/5 * * * *"));
    }

    #[test]
    fn next_after_finds_the_following_minute() {
        let e = CronExpr::parse("*/5 * * * *").unwrap();
        let from = at(2026, 11, 1, 10, 1);
        assert_eq!(e.next_after(from).unwrap(), at(2026, 11, 1, 10, 5));
        // 边界：正好命中时，下一次仍是「之后」而不是它自己
        let from = at(2026, 11, 1, 10, 5);
        assert_eq!(e.next_after(from).unwrap(), at(2026, 11, 1, 10, 10));
    }

    /// 跨日/跨月：每天 03:00 的表达式在 23:59 后应落到次日 03:00。
    #[test]
    fn next_after_crosses_day_boundary() {
        let e = CronExpr::parse("0 3 * * *").unwrap();
        assert_eq!(
            e.next_after(at(2026, 11, 1, 23, 59)).unwrap(),
            at(2026, 11, 2, 3, 0)
        );
        // 每月 1 日 00:00：从 1 日 00:01 起应落到下月 1 日
        let e = CronExpr::parse("0 0 1 * *").unwrap();
        assert_eq!(
            e.next_after(at(2026, 11, 1, 0, 1)).unwrap(),
            at(2026, 12, 1, 0, 0)
        );
    }
}
