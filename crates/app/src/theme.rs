//! 壳主题（全局）：主色持久化在 `settings` 表，按全站生效。
//!
//! 最小集合只有主色一项；颜色统一存 `#rrggbb` 小写。读取时对存量值再做一次
//! 校验，非法值回落到缺省主色，避免手改数据库把整站样式带坏。

use crate::AppState;

/// 壳主题主色的设置键。
pub const THEME_PRIMARY_KEY: &str = "ui.theme.primary";
/// 缺省主色：与 budding/Vuetify 的 `primary` 缺省值一致。
pub const DEFAULT_PRIMARY: &str = "#1867c0";

/// 校验并规范化主色：只接受 `#rrggbb`，输出小写。
pub fn normalize_primary(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| "主色必须是 #RRGGBB 格式".to_string())?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("主色必须是 #RRGGBB 格式".into());
    }
    Ok(format!("#{}", hex.to_ascii_lowercase()))
}

/// 当前生效的主色：未设置或存量非法都回落到缺省值。
pub fn primary(state: &AppState) -> String {
    state
        .db()
        .get_setting(THEME_PRIMARY_KEY)
        .ok()
        .flatten()
        .and_then(|raw| normalize_primary(&raw).ok())
        .unwrap_or_else(|| DEFAULT_PRIMARY.to_string())
}

/// 保存主色并返回落库后的规范值；空串表示恢复缺省值。
pub fn save_primary(state: &AppState, raw: &str) -> Result<String, String> {
    let value = if raw.trim().is_empty() {
        DEFAULT_PRIMARY.to_string()
    } else {
        normalize_primary(raw)?
    };
    state
        .db()
        .set_setting(THEME_PRIMARY_KEY, &value)
        .map_err(|e| format!("保存主题失败：{e}"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_accepts_rrggbb_and_normalizes_case() {
        assert_eq!(normalize_primary("#A1B2C3").unwrap(), "#a1b2c3");
        assert_eq!(normalize_primary("  #00ff7F ").unwrap(), "#00ff7f");
    }

    #[test]
    fn primary_rejects_non_hex_and_wrong_length() {
        for raw in ["red", "123456", "#12345", "#1234567", "#gggggg", ""] {
            assert!(normalize_primary(raw).is_err(), "{raw} 不应通过校验");
        }
    }

    #[test]
    fn default_primary_is_valid() {
        assert_eq!(normalize_primary(DEFAULT_PRIMARY).unwrap(), DEFAULT_PRIMARY);
    }
}
