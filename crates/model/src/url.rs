//! URL 形状常量：内核拼链接与 web 注册路由共用**同一份**定义。
//!
//! 内核页面全部由 budding 壳承载，深链统一走规范路径
//! （`/{prefix}/plugins/<id>/<view>`、`/{prefix}/<page>`）。

/// 拼一条插件功能页深链：`/{prefix}/plugins/{id}/{rest}`。
///
/// 兼容历史键：`plugin/<id>/<local>[/<l3>]` → 规范功能页；`overview` → 入口根。
pub fn menu_href(prefix: &str, rest: &str) -> String {
    if let Some(tail) = rest.strip_prefix("plugin/") {
        return format!("/{prefix}/plugins/{tail}");
    }
    if rest == "overview" {
        return format!("/{prefix}");
    }
    format!("/{prefix}/{rest}")
}

/// 拼一条入口内路径：`/{prefix}/{path}`。
pub fn entry_href(prefix: &str, path: &str) -> String {
    format!("/{prefix}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_keys_map_to_canonical_paths() {
        assert_eq!(
            menu_href("admin", "plugin/com.skiff.cron/list"),
            "/admin/plugins/com.skiff.cron/list"
        );
        assert_eq!(
            menu_href("admin", "plugin/com.skiff.cron/list/runs"),
            "/admin/plugins/com.skiff.cron/list/runs"
        );
        assert_eq!(menu_href("admin", "overview"), "/admin");
        assert_eq!(entry_href("admin", "plugins"), "/admin/plugins");
    }
}
