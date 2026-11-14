//! 壳主题（全局）：主色、菜单底色、页面底色持久化在 `settings` 表，按全站生效。
//!
//! 颜色统一存 `#rrggbb` 小写；读取时对存量值再做一次校验，非法值回落到缺省，
//! 避免手改数据库把整站样式带坏。主色与菜单底色的对比度不足 4.5:1 时，
//! 按亮度方向逐步混白/混黑校正，保证激活色在菜单上可读。

use crate::AppState;

/// 壳主题主色的设置键。
pub const THEME_PRIMARY_KEY: &str = "ui.theme.primary";
/// 菜单底色（侧栏、顶栏、页签条共用）。
pub const THEME_MENU_KEY: &str = "ui.theme.menu";
/// 页面底色。
pub const THEME_PAGE_KEY: &str = "ui.theme.page";
/// 菜单底色是否启用自定义值（否则按页面底色自动取深/浅）。
pub const THEME_MENU_CUSTOM_KEY: &str = "ui.theme.menu_custom";

/// 缺省主色：与 budding/Vuetify 的 `primary` 缺省值一致。
pub const DEFAULT_PRIMARY: &str = "#1867c0";
/// 缺省页面底色。
pub const DEFAULT_PAGE: &str = "#f5f5f5";
/// 自动菜单底色：浅色页用白，深色页用近黑。
pub const AUTO_MENU_LIGHT: &str = "#ffffff";
pub const AUTO_MENU_DARK: &str = "#121212";
/// 可读性下限（WCAG AA 正文）。
pub const MIN_CONTRAST: f64 = 4.5;

/// 一次读取得到的完整主题（含派生文本色）。
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub primary: String,
    pub menu: String,
    pub page: String,
    pub menu_custom: bool,
    /// 菜单文字色（黑/白中对比度更高者）。
    pub menu_text: String,
    /// 页面文字色。
    pub page_text: String,
    /// 主色按钮上的文字色。
    pub on_primary: String,
    /// 主色是否因对比度不足被校正。
    pub adjusted: bool,
}

/// 保存主题的输入；空字符串表示恢复缺省值。
#[derive(Debug, Default, Clone)]
pub struct ThemeInput {
    pub primary: String,
    pub menu: String,
    pub page: String,
    pub menu_custom: bool,
}

/// 校验并规范化颜色：只接受 `#rrggbb`，输出小写。
pub fn normalize_color(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| "颜色必须是 #RRGGBB 格式".to_string())?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("颜色必须是 #RRGGBB 格式".into());
    }
    Ok(format!("#{}", hex.to_ascii_lowercase()))
}

fn rgb(hex: &str) -> (u8, u8, u8) {
    let r = u8::from_str_radix(&hex[1..3], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[3..5], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[5..7], 16).unwrap_or(0);
    (r, g, b)
}

fn to_hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

fn channel(value: u8) -> f64 {
    let v = value as f64 / 255.0;
    if v <= 0.039_28 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 相对亮度。
pub fn luminance(hex: &str) -> f64 {
    let (r, g, b) = rgb(hex);
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// 两色的对比度（1.0–21.0）。
pub fn contrast(a: &str, b: &str) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// 背景上最可读的黑/白文字色。
pub fn best_on(background: &str) -> String {
    if contrast("#000000", background) >= contrast("#ffffff", background) {
        "#000000".to_string()
    } else {
        "#ffffff".to_string()
    }
}

fn mix(color: &str, target: &str, ratio: f64) -> String {
    let (r1, g1, b1) = rgb(color);
    let (r2, g2, b2) = rgb(target);
    let f = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * ratio).round() as u8;
    to_hex((f(r1, r2), f(g1, g2), f(b1, b2)))
}

/// 校正前景色直到与背景对比度达到 `MIN_CONTRAST`；返回（颜色, 是否校正）。
///
/// 方向取决于背景亮度：深底混白、浅底混黑，每步 5%。若极限白/黑仍不足
/// （极少数中灰背景），返回对比度最高的那一端。
pub fn ensure_contrast(foreground: &str, background: &str) -> (String, bool) {
    if contrast(foreground, background) >= MIN_CONTRAST {
        return (foreground.to_string(), false);
    }
    let target = if luminance(background) < 0.5 {
        "#ffffff"
    } else {
        "#000000"
    };
    // 从原色开始逐步混向目标色：只要达到 4.5 就停，避免一步到纯白/纯黑。
    let mut best = foreground.to_string();
    let mut best_ratio = contrast(foreground, background);
    for step in 1..=20 {
        let candidate = mix(foreground, target, step as f64 * 0.05);
        let ratio = contrast(&candidate, background);
        if ratio >= MIN_CONTRAST {
            return (candidate, true);
        }
        if ratio > best_ratio {
            best = candidate;
            best_ratio = ratio;
        }
    }
    (best, true)
}

fn read_setting(state: &AppState, key: &str) -> Option<String> {
    state.db().get_setting(key).ok().flatten()
}

fn read_color(state: &AppState, key: &str, default: &str) -> String {
    read_setting(state, key)
        .and_then(|raw| normalize_color(&raw).ok())
        .unwrap_or_else(|| default.to_string())
}

fn read_menu_custom(state: &AppState) -> bool {
    matches!(
        read_setting(state, THEME_MENU_CUSTOM_KEY).as_deref(),
        Some("1") | Some("true")
    )
}

/// 自动菜单底色：浅色页白、深色页近黑。
pub fn auto_menu(page: &str) -> String {
    if luminance(page) > 0.5 {
        AUTO_MENU_LIGHT.to_string()
    } else {
        AUTO_MENU_DARK.to_string()
    }
}

fn build(primary: &str, page: &str, menu_custom: bool, menu_raw: Option<String>) -> Theme {
    let menu = if menu_custom {
        menu_raw.unwrap_or_else(|| auto_menu(page))
    } else {
        auto_menu(page)
    };
    let (primary, adjusted) = ensure_contrast(primary, &menu);
    Theme {
        menu_text: best_on(&menu),
        page_text: best_on(page),
        on_primary: best_on(&primary),
        primary,
        menu,
        page: page.to_string(),
        menu_custom,
        adjusted,
    }
}

/// 当前生效的主题：未设置或存量非法都回落到缺省值。
pub fn current(state: &AppState) -> Theme {
    let page = read_color(state, THEME_PAGE_KEY, DEFAULT_PAGE);
    let menu_custom = read_menu_custom(state);
    let stored_menu =
        read_setting(state, THEME_MENU_KEY).and_then(|raw| normalize_color(&raw).ok());
    let primary = read_color(state, THEME_PRIMARY_KEY, DEFAULT_PRIMARY);
    build(&primary, &page, menu_custom, stored_menu)
}

/// 保存主题并返回落库后的规范值；空串表示恢复该字段缺省值。
pub fn save(state: &AppState, input: &ThemeInput) -> Result<Theme, String> {
    let primary = if input.primary.trim().is_empty() {
        DEFAULT_PRIMARY.to_string()
    } else {
        normalize_color(&input.primary).map_err(|e| format!("主色{e}"))?
    };
    let page = if input.page.trim().is_empty() {
        DEFAULT_PAGE.to_string()
    } else {
        normalize_color(&input.page).map_err(|e| format!("页面底色{e}"))?
    };
    let menu = if input.menu.trim().is_empty() {
        None
    } else {
        Some(normalize_color(&input.menu).map_err(|e| format!("菜单底色{e}"))?)
    };
    let theme = build(&primary, &page, input.menu_custom, menu.clone());
    let db = state.db();
    db.set_setting(THEME_PRIMARY_KEY, &theme.primary)
        .map_err(|e| format!("保存主题失败：{e}"))?;
    db.set_setting(THEME_PAGE_KEY, &theme.page)
        .map_err(|e| format!("保存主题失败：{e}"))?;
    db.set_setting(THEME_MENU_KEY, menu.as_deref().unwrap_or(""))
        .map_err(|e| format!("保存主题失败：{e}"))?;
    db.set_setting(
        THEME_MENU_CUSTOM_KEY,
        if input.menu_custom { "1" } else { "0" },
    )
    .map_err(|e| format!("保存主题失败：{e}"))?;
    Ok(theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_accepts_rrggbb_and_normalizes_case() {
        assert_eq!(normalize_color("#A1B2C3").unwrap(), "#a1b2c3");
        assert_eq!(normalize_color("  #00ff7F ").unwrap(), "#00ff7f");
    }

    #[test]
    fn primary_rejects_non_hex_and_wrong_length() {
        for raw in ["red", "123456", "#12345", "#1234567", "#gggggg", ""] {
            assert!(normalize_color(raw).is_err(), "{raw} 不应通过校验");
        }
    }

    #[test]
    fn default_primary_is_valid() {
        assert_eq!(normalize_color(DEFAULT_PRIMARY).unwrap(), DEFAULT_PRIMARY);
        assert_eq!(normalize_color(DEFAULT_PAGE).unwrap(), DEFAULT_PAGE);
    }

    #[test]
    fn contrast_helpers_choose_readable_text() {
        assert_eq!(best_on("#ffffff"), "#000000");
        assert_eq!(best_on("#101010"), "#ffffff");
        assert!(contrast("#000000", "#ffffff") > 20.0);
    }

    #[test]
    fn low_contrast_primary_is_corrected() {
        // 深蓝底 + 中蓝主色：校正后必须达到 4.5，但不应一步变成纯白。
        let (fixed, adjusted) = ensure_contrast("#1867c0", "#1f2a44");
        assert!(adjusted);
        assert_ne!(fixed, "#ffffff", "校正应逐步混色，不能直接变纯白");
        assert!(
            contrast(&fixed, "#1f2a44") >= MIN_CONTRAST,
            "校正后对比度不足：{}",
            contrast(&fixed, "#1f2a44")
        );
        assert!(
            contrast(&fixed, "#1f2a44") < 8.0,
            "校正过头：{}",
            contrast(&fixed, "#1f2a44")
        );
    }

    #[test]
    fn auto_menu_follows_page_luminance() {
        assert_eq!(auto_menu("#ffffff"), AUTO_MENU_LIGHT);
        assert_eq!(auto_menu("#000000"), AUTO_MENU_DARK);
    }

    #[test]
    fn build_keeps_readable_menu_and_page_text() {
        let theme = build("#1867c0", "#f5f5f5", false, None);
        assert_eq!(theme.menu, AUTO_MENU_LIGHT);
        assert_eq!(theme.menu_text, "#000000");
        assert_eq!(theme.page_text, "#000000");
    }
}
