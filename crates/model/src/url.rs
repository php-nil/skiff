//! URL 形状常量：内核拼链接与 web 注册路由共用**同一份**定义。
//!
//! 背景：`/{prefix}/m/...` 过去是 `app/menu.rs` 里的一串 `format!` 与
//! `web/src/lib.rs` 里的路由字符串各写一遍，靠人眼对齐；web 改路由、menu 静默失效。
//! 这里把「段名 + 路由形状 + 拼接函数」收成一处，两侧都从这里取值。

/// 菜单深链的路径段（`/{prefix}/m/...`）。
pub const MENU_SEGMENT: &str = "m";

/// axum 路由形状：入口前缀段 + 菜单深链。
pub const MENU_ROUTE: &str = "/{p}/m/{*rest}";

/// 拼一条菜单深链：`/{prefix}/m/{rest}`。
pub fn menu_href(prefix: &str, rest: &str) -> String {
    format!("/{prefix}/{MENU_SEGMENT}/{rest}")
}

/// 拼一条入口内路径：`/{prefix}/{path}`。
pub fn entry_href(prefix: &str, path: &str) -> String {
    format!("/{prefix}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_shape_and_segment_stay_aligned() {
        assert!(MENU_ROUTE.contains(&format!("/{MENU_SEGMENT}/")));
        assert_eq!(
            menu_href("admin", "core/dashboard"),
            "/admin/m/core/dashboard"
        );
        assert_eq!(entry_href("admin", "plugins"), "/admin/plugins");
    }
}
