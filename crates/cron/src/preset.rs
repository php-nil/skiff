//! 常用调度的「预设 ↔ 标准 5 段表达式」翻译。
//!
//! 内核只认标准 cron 表达式，本模块只负责输入与展示的翻译，不引入新的调度语义：
//! - [`Preset::to_expr`]：把用户选出来的调度翻译成标准表达式（落库仍是表达式）；
//! - [`detect_preset`]：把已存储的表达式反解成预设，匹配不到返回 `None`；
//! - [`describe_expr`]：给界面用的「人话」预览。
//!
//! 日 / 周字段沿用 crontab 的「或」语义；周字段 0 与 7 都表示周日。

use crate::CronExpr;

/// 界面上的常用调度选项。字段范围由 [`Preset::to_expr`] 校验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preset {
    /// 每隔 `every` 分钟（1–59）：`*/N * * * *`。
    Minutes { every: u32 },
    /// 每隔 `every` 小时（1–23），整点触发：`0 */N * * *`。
    Hours { every: u32 },
    /// 每天 `hour:minute`。
    Daily { hour: u32, minute: u32 },
    /// 每周指定几天（0=周日 … 6=周六）的 `hour:minute`。
    Weekly {
        days: Vec<u32>,
        hour: u32,
        minute: u32,
    },
    /// 每月 `day` 日的 `hour:minute`。
    Monthly { day: u32, hour: u32, minute: u32 },
    /// 周一至周五（工作日）的 `hour:minute`。
    Weekdays { hour: u32, minute: u32 },
}

impl Preset {
    /// 翻译成标准 5 段表达式；字段越界返回人话错误。
    pub fn to_expr(&self) -> Result<String, String> {
        match self {
            Self::Minutes { every } => {
                if !(1..=59).contains(every) {
                    return Err(format!("分钟间隔需在 1–59：{every}"));
                }
                Ok(if *every == 1 {
                    "* * * * *".to_string()
                } else {
                    format!("*/{every} * * * *")
                })
            }
            Self::Hours { every } => {
                if !(1..=23).contains(every) {
                    return Err(format!("小时间隔需在 1–23：{every}"));
                }
                Ok(if *every == 1 {
                    "0 * * * *".to_string()
                } else {
                    format!("0 */{every} * * *")
                })
            }
            Self::Daily { hour, minute } => {
                check_time(*hour, *minute)?;
                Ok(format!("{minute} {hour} * * *"))
            }
            Self::Weekly { days, hour, minute } => {
                check_time(*hour, *minute)?;
                let mut days = days.clone();
                if days.is_empty() {
                    return Err("每周调度至少要选一天".to_string());
                }
                if days.iter().any(|d| *d > 6) {
                    return Err(format!("星期取值需在 0–6（0=周日）：{days:?}"));
                }
                days.sort_unstable();
                days.dedup();
                let list = days
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(format!("{minute} {hour} * * {list}"))
            }
            Self::Monthly { day, hour, minute } => {
                check_time(*hour, *minute)?;
                if !(1..=31).contains(day) {
                    return Err(format!("每月日期需在 1–31：{day}"));
                }
                Ok(format!("{minute} {hour} {day} * *"))
            }
            Self::Weekdays { hour, minute } => {
                check_time(*hour, *minute)?;
                Ok(format!("{minute} {hour} * * 1-5"))
            }
        }
    }

    /// 界面预览用的中文描述，例如「每天 03:00 执行」。
    pub fn describe(&self) -> String {
        match self {
            Self::Minutes { every } if *every == 1 => "每分钟执行".to_string(),
            Self::Minutes { every } => format!("每 {every} 分钟执行"),
            Self::Hours { every } if *every == 1 => "每小时执行".to_string(),
            Self::Hours { every } => format!("每 {every} 小时执行"),
            Self::Daily { hour, minute } => format!("每天 {} 执行", hm(*hour, *minute)),
            Self::Weekly { days, hour, minute } => {
                format!("每{} {} 执行", day_list(days), hm(*hour, *minute))
            }
            Self::Monthly { day, hour, minute } => {
                format!("每月 {day} 日 {} 执行", hm(*hour, *minute))
            }
            Self::Weekdays { hour, minute } => {
                format!("每个工作日 {} 执行", hm(*hour, *minute))
            }
        }
    }
}

/// 把已存储的标准表达式反解成预设；非法表达式或无法对应到预设时返回 `None`。
pub fn detect_preset(expr: &str) -> Option<Preset> {
    let e = CronExpr::parse(expr)?;
    // 所有预设都按「每月」通配；月份字段另有限制的一律落自定义。
    if !e.month.iter().all(|v| *v) {
        return None;
    }

    let minute = single(&e.minute);
    let hour = single(&e.hour);
    if let (Some(m), Some(h)) = (minute, hour) {
        // 日与周都通配 → 每天。
        if e.dom_any && e.dow_any {
            return Some(Preset::Daily { hour: h, minute: m });
        }
        // 只在日字段限制 → 每月某日。
        if !e.dom_any && e.dow_any {
            let day = single(&e.dom)? + 1;
            return Some(Preset::Monthly {
                day,
                hour: h,
                minute: m,
            });
        }
        // 只在周字段限制 → 每周 / 工作日。
        if e.dom_any && !e.dow_any {
            let days = dow_days(&e.dow);
            if days.is_empty() {
                return None;
            }
            if matches!(days.as_slice(), [1, 2, 3, 4, 5]) {
                return Some(Preset::Weekdays { hour: h, minute: m });
            }
            return Some(Preset::Weekly {
                days,
                hour: h,
                minute: m,
            });
        }
        // 日与周都受限：crontab 是「或」，没有对应预设。
        return None;
    }

    // 其余预设都要求日 / 周通配。
    if !(e.dom_any && e.dow_any) {
        return None;
    }
    if e.hour.iter().all(|v| *v) {
        if let Some(n) = step_from_zero(&e.minute) {
            return Some(Preset::Minutes { every: n });
        }
    }
    if minute == Some(0) {
        if e.hour.iter().all(|v| *v) {
            return Some(Preset::Hours { every: 1 });
        }
        if let Some(n) = step_from_zero(&e.hour) {
            return Some(Preset::Hours { every: n });
        }
    }
    None
}

/// 给界面用的「人话」预览；只有能对应到预设时才返回，其余由界面显示「自定义」。
pub fn describe_expr(expr: &str) -> Option<String> {
    detect_preset(expr).map(|p| p.describe())
}

fn check_time(hour: u32, minute: u32) -> Result<(), String> {
    if hour > 23 {
        return Err(format!("小时需在 0–23：{hour}"));
    }
    if minute > 59 {
        return Err(format!("分钟需在 0–59：{minute}"));
    }
    Ok(())
}

fn hm(hour: u32, minute: u32) -> String {
    format!("{hour:02}:{minute:02}")
}

/// 周字段里唯一为真的下标。
fn single(set: &[bool]) -> Option<u32> {
    let mut found = None;
    for (i, on) in set.iter().enumerate() {
        if *on {
            if found.is_some() {
                return None;
            }
            found = Some(i as u32);
        }
    }
    found
}

/// 从 0 开始、固定步长命中的集合 → 步长；其它形态（含单个 0）返回 `None`。
fn step_from_zero(set: &[bool]) -> Option<u32> {
    if set.first().copied() != Some(true) {
        return None;
    }
    let step = set.iter().skip(1).position(|v| *v)? + 1;
    let mut i = 0;
    while i < set.len() {
        if !set[i] {
            return None;
        }
        i += step;
    }
    Some(step as u32)
}

/// 周字段 → 0–6 的日列表；7 与 0 都是周日。
fn dow_days(dow: &[bool]) -> Vec<u32> {
    let mut days: Vec<u32> = (0..7).filter(|i| dow[*i]).map(|i| i as u32).collect();
    if dow.get(7).copied().unwrap_or(false) && !days.contains(&0) {
        days.push(0);
    }
    days.sort_unstable();
    days
}

fn day_list(days: &[u32]) -> String {
    const NAMES: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
    days.iter()
        .filter_map(|d| NAMES.get(*d as usize))
        .copied()
        .collect::<Vec<_>>()
        .join("、")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_to_expr_covers_every_option() {
        let cases = [
            (Preset::Minutes { every: 1 }, "* * * * *"),
            (Preset::Minutes { every: 5 }, "*/5 * * * *"),
            (Preset::Minutes { every: 59 }, "*/59 * * * *"),
            (Preset::Hours { every: 1 }, "0 * * * *"),
            (Preset::Hours { every: 6 }, "0 */6 * * *"),
            (Preset::Daily { hour: 3, minute: 0 }, "0 3 * * *"),
            (
                Preset::Weekly {
                    days: vec![5, 1, 3],
                    hour: 9,
                    minute: 30,
                },
                "30 9 * * 1,3,5",
            ),
            (
                Preset::Monthly {
                    day: 1,
                    hour: 4,
                    minute: 0,
                },
                "0 4 1 * *",
            ),
            (Preset::Weekdays { hour: 9, minute: 0 }, "0 9 * * 1-5"),
        ];
        for (preset, want) in cases {
            assert_eq!(preset.to_expr().unwrap(), want, "预设 {preset:?}");
        }
    }

    #[test]
    fn preset_rejects_out_of_range_values() {
        assert!(Preset::Minutes { every: 0 }.to_expr().is_err());
        assert!(Preset::Minutes { every: 60 }.to_expr().is_err());
        assert!(Preset::Hours { every: 0 }.to_expr().is_err());
        assert!(Preset::Hours { every: 24 }.to_expr().is_err());
        assert!(Preset::Daily {
            hour: 24,
            minute: 0
        }
        .to_expr()
        .is_err());
        assert!(Preset::Weekly {
            days: vec![],
            hour: 0,
            minute: 0
        }
        .to_expr()
        .is_err());
        assert!(Preset::Weekly {
            days: vec![7],
            hour: 0,
            minute: 0
        }
        .to_expr()
        .is_err());
        assert!(Preset::Monthly {
            day: 0,
            hour: 0,
            minute: 0
        }
        .to_expr()
        .is_err());
    }

    #[test]
    fn expr_round_trips_back_to_preset() {
        let presets = [
            Preset::Minutes { every: 5 },
            Preset::Hours { every: 6 },
            Preset::Daily { hour: 3, minute: 0 },
            Preset::Weekly {
                days: vec![1, 3, 5],
                hour: 9,
                minute: 30,
            },
            Preset::Monthly {
                day: 15,
                hour: 2,
                minute: 30,
            },
            Preset::Weekdays { hour: 9, minute: 0 },
        ];
        for preset in presets {
            let expr = preset.to_expr().unwrap();
            assert_eq!(detect_preset(&expr), Some(preset.clone()), "表达式 {expr}");
            assert_eq!(
                describe_expr(&expr),
                Some(preset.describe()),
                "表达式 {expr}"
            );
        }
    }

    #[test]
    fn equivalent_forms_are_recognised() {
        // 周日写 0 或 7 等价。
        for expr in ["0 9 * * 0", "0 9 * * 7"] {
            assert_eq!(
                detect_preset(expr),
                Some(Preset::Weekly {
                    days: vec![0],
                    hour: 9,
                    minute: 0,
                })
            );
        }
        // 工作日写成列表也识别为「工作日」。
        assert_eq!(
            detect_preset("0 9 * * 1,2,3,4,5"),
            Some(Preset::Weekdays { hour: 9, minute: 0 })
        );
        // 整点每小时 / 每分钟。
        assert_eq!(detect_preset("0 * * * *"), Some(Preset::Hours { every: 1 }));
        assert_eq!(
            detect_preset("* * * * *"),
            Some(Preset::Minutes { every: 1 })
        );
    }

    #[test]
    fn custom_expressions_fall_back_without_losing_text() {
        for expr in ["0,30 9 * * *", "0 0 13 * 5", "5/10 * * * *", "0 9 * 1 *"] {
            assert!(detect_preset(expr).is_none(), "{expr} 不该匹配任何预设");
            assert!(describe_expr(expr).is_none());
        }
        for bad in ["", "* * * *", "60 * * * *", "@reboot", "0 0 0 * *"] {
            assert!(detect_preset(bad).is_none(), "{bad:?} 不该匹配任何预设");
        }
    }

    #[test]
    fn description_is_human_readable() {
        assert_eq!(
            Preset::Daily { hour: 3, minute: 0 }.describe(),
            "每天 03:00 执行"
        );
        assert_eq!(
            Preset::Weekly {
                days: vec![1, 3, 5],
                hour: 9,
                minute: 30
            }
            .describe(),
            "每周一、周三、周五 09:30 执行"
        );
        assert_eq!(
            Preset::Monthly {
                day: 1,
                hour: 4,
                minute: 0
            }
            .describe(),
            "每月 1 日 04:00 执行"
        );
        assert_eq!(
            Preset::Weekdays { hour: 9, minute: 0 }.describe(),
            "每个工作日 09:00 执行"
        );
        assert_eq!(Preset::Minutes { every: 5 }.describe(), "每 5 分钟执行");
        assert_eq!(Preset::Hours { every: 2 }.describe(), "每 2 小时执行");
    }
}
