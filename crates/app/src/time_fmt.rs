//! 时间格式化的单一出口（SDUI D9）。
//!
//! 现状是页面各自拼 `%Y-%m-%d %H:%M`（本地时区）、目录元数据用 RFC3339、
//! 库里存 epoch 秒。切换到 JSON 传输层后，内核只输出机器可读时间，本地化归前端。
//! 本模块只提供 `epoch 秒 → RFC3339 UTC`，**不改动任何现有页面输出**。

use chrono::{DateTime, SecondsFormat};

/// epoch 秒 → RFC3339 UTC 字符串（秒级精度，固定 `Z` 后缀）。
///
/// 例：`0` → `1970-01-01T00:00:00Z`；`-1` → `1969-12-31T23:59:59Z`。
///
/// 超出 chrono 可表示范围（约公元 ±262000 年）时返回空串：调用方应把它当作
/// 「无时间」处理，而不是把 1970 当真实时间——静默编造时间比空值更难排查。
pub fn epoch_to_rfc3339_utc(epoch_secs: i64) -> String {
    DateTime::from_timestamp(epoch_secs, 0)
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_unix_epoch() {
        assert_eq!(epoch_to_rfc3339_utc(0), "1970-01-01T00:00:00Z");
    }

    /// 负数（1970 之前）同样是合法时间，不能截断成 0。
    #[test]
    fn negative_epoch_precedes_unix_epoch() {
        assert_eq!(epoch_to_rfc3339_utc(-1), "1969-12-31T23:59:59Z");
        assert_eq!(epoch_to_rfc3339_utc(-86_400), "1969-12-31T00:00:00Z");
    }

    /// 闰秒附近：Unix 时间没有 23:59:60，2016-12-31 的最后一秒必须落到 23:59:59，
    /// 下一秒直接跨年。
    #[test]
    fn leap_second_boundary_crosses_year() {
        assert_eq!(epoch_to_rfc3339_utc(1_483_228_799), "2016-12-31T23:59:59Z");
        assert_eq!(epoch_to_rfc3339_utc(1_483_228_800), "2017-01-01T00:00:00Z");
    }

    /// 普通跨年边界：2024-12-31 最后一秒 → 2025-01-01 零点。
    #[test]
    fn year_boundary_is_exact() {
        assert_eq!(epoch_to_rfc3339_utc(1_735_689_599), "2024-12-31T23:59:59Z");
        assert_eq!(epoch_to_rfc3339_utc(1_735_689_600), "2025-01-01T00:00:00Z");
    }

    /// 超出可表示范围返回空串，不伪造时间。
    #[test]
    fn out_of_range_yields_empty() {
        assert_eq!(epoch_to_rfc3339_utc(i64::MAX), "");
        assert_eq!(epoch_to_rfc3339_utc(i64::MIN), "");
    }
}
