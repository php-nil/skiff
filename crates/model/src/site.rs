//! 站点统一身份：宿主与 PHP 容器共用同一组**固定** uid/gid。
//!
//! 目的不是某个具体数字好看，而是「统一可辨识」：站点文档根、FTP 上传、面板文件管理写入、
//! 插件写宿主路径，四处落盘后归属同一个账号，管理员 `ls -ln` 一眼就能判断权限是否正确。
//!
//! 约束：
//! - 数值固定，任何机器一致；安装前 preflight 发现被占用必须拒绝，**不静默换号**。
//! - 避开宿主机常见的人类账号号段（1000 左右）与插件 UID 号段（200000–200999）；
//!   201000 由部署清单与安装脚本共同保证。
//! - 名字固定，容器内按名字建号后才能与池模板 `user/group` 对上。

/// 站点统一身份的用户名与组名（宿主与 PHP 容器内同名）。
pub const SITE_USER: &str = "skiff-web";

/// 站点统一身份的固定 UID；任何机器一致，不做动态分配。
pub const SITE_UID: u32 = 201000;

/// 站点统一身份的固定 GID。
pub const SITE_GID: u32 = 201000;

/// 站点文档根在站点目录下的固定名字：`<web_root>/sites/<domain>/index`。
pub const SITE_INDEX_DIR: &str = "index";

/// 判断宿主路径是否位于某个站点的 `index/` 子树。
///
/// 只做词法判断（不访问文件系统），供面板写入与插件 `fs` 写宿主路径后归还属主使用。
pub fn is_site_index_path(path: &std::path::Path) -> bool {
    // 带 `..` 的路径不做归属判定：调用方给的多半是未规范化的用户输入，
    // 按字面匹配会把 `index/../log` 误判成文档根。
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    let comps: Vec<_> = path.components().map(|c| c.as_os_str()).collect();
    comps
        .windows(3)
        .any(|w| w[0] == "sites" && w[2] == SITE_INDEX_DIR && !w[1].is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_site_index_subtree() {
        assert!(is_site_index_path(Path::new("/www/sites/a.com/index")));
        assert!(is_site_index_path(Path::new(
            "/www/sites/a.com/index/wp-content/uploads/x.jpg"
        )));
        // 站点目录下其它子树不算
        assert!(!is_site_index_path(Path::new("/www/sites/a.com/log/x.log")));
        // 缺域名段不算
        assert!(!is_site_index_path(Path::new("/www/sites/index")));
        // 插件数据目录里的同名段不算
        assert!(!is_site_index_path(Path::new(
            "/www/skiff/plugins/x/data/index"
        )));
        // 未规范化路径不参与判定（避免 index/../log 被误判）
        assert!(!is_site_index_path(Path::new(
            "/www/sites/a.com/index/../log/x.log"
        )));
    }
}
