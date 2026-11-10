//! 站点目录布局：宿主/容器路径派生、属主归还与落盘辅助。

use super::*;

/// 站点目录内的固定子目录（一站一目录，整目录备份即整站备份）。
pub const SITE_LOG_DIR: &str = "log";
pub const SITE_SSL_DIR: &str = "ssl";
pub const SITE_CONF_DIR: &str = "conf";

/// 递归统计目录体积与文件数：不跟随符号链接，读不到的条目跳过。
pub(crate) fn dir_usage(root: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    if !root.exists() {
        return (bytes, files);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            // 符号链接可能指向站点目录之外：只计为文件，不跟进去统计体积。
            if ft.is_symlink() {
                files += 1;
            } else if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                files += 1;
                if let Ok(meta) = entry.metadata() {
                    bytes = bytes.saturating_add(meta.len());
                }
            }
        }
    }
    (bytes, files)
}

/// 站点文档根（`index/`）；files 插件与「目录」页的授权根只能是它。
pub fn site_index_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_INDEX_DIR)
}

/// 把单个路径归还站点统一身份；`lchown` 不跟随符号链接。
///
/// 非 root 运行（开发机 / 受限部署）时只记一条告警，不阻断业务；
/// 真实环境由安装脚本保证 Skiff 以 root 运行，缺失的属主由迁移子命令兜底。
#[cfg(unix)]
pub(crate) fn lchown_site(path: &Path) {
    use std::os::unix::fs::lchown;
    if let Err(e) = lchown(
        path,
        Some(skiff_model::SITE_UID),
        Some(skiff_model::SITE_GID),
    ) {
        eprintln!("[skiff] 归还站点属主失败 {}：{e}", path.display());
    }
}

#[cfg(not(unix))]
pub(crate) fn lchown_site(_path: &Path) {}

/// 递归把站点文档根归还统一身份（`skiff-web` 201000:201000）。
///
/// 用 `symlink_metadata` 判定类型：符号链接只改链接本身，绝不顺着链接读目录，
/// 避免把改动带到站点之外。
pub fn align_site_index_owner(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let is_dir = match std::fs::symlink_metadata(&path) {
            Ok(m) => {
                if m.file_type().is_symlink() {
                    lchown_site(&path);
                    continue;
                }
                m.is_dir()
            }
            Err(_) => continue,
        };
        lchown_site(&path);
        if !is_dir {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() && !ft.is_symlink() {
                stack.push(entry.path());
            } else {
                // 普通文件或符号链接：只改自身，不跟随
                lchown_site(&entry.path());
            }
        }
    }
}

pub fn site_log_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_LOG_DIR)
}

pub fn site_ssl_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_SSL_DIR)
}

pub fn site_conf_dir(site_dir: &Path) -> PathBuf {
    site_dir.join(SITE_CONF_DIR)
}

impl SiteSettings {
    fn container_rel(&self, host: &Path) -> String {
        let rel = host.strip_prefix(&self.web_root).unwrap_or(host);
        // 容器内一律 POSIX 路径：Windows 开发机上 `Path` 会带反斜杠。
        let rel = rel.to_string_lossy().replace('\\', "/");
        format!("{CONTAINER_WEB_ROOT}/{}", rel.trim_start_matches('/'))
    }

    pub fn container_sites_dir(&self) -> String {
        self.container_rel(&self.sites_dir)
    }
    pub fn container_conf_dir(&self) -> &'static str {
        "/etc/nginx/conf.d"
    }

    /// 宿主站点目录 → 容器内路径；调用方保证 `host` 在 `web_root` 内。
    pub fn container_path(&self, host: &Path) -> String {
        self.container_rel(host)
    }

    /// 站点日志：`<站点目录>/log/access.log|error.log`。
    pub fn log_file(&self, site_dir: &Path, kind: LogKind) -> PathBuf {
        site_log_dir(site_dir).join(format!("{}.log", kind.as_str()))
    }

    pub fn container_log(&self, site_dir: &Path, kind: LogKind) -> String {
        self.container_rel(&self.log_file(site_dir, kind))
    }

    /// 某环境实例的独立运行目录：`<runtime>/<kind>/<name>/`。
    pub fn runtime_home(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_dir.join(kind).join(name)
    }

    /// PHP 实例 socket 的宿主路径（0660 root:nobody）。
    pub fn runtime_socket(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("php-fpm.sock")
    }

    /// PHP 实例的 FPM 池配置（宿主文件，覆盖镜像默认 www.conf）。
    pub fn runtime_pool_conf(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("www.conf")
    }

    /// PHP 实例的宿主托管配置目录：`<runtime>/<kind>/<name>/php/`。
    pub fn runtime_php_dir(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("php")
    }

    /// 宿主托管的主配置文件（挂到容器 `/usr/local/etc/php/php.ini`）。
    pub fn runtime_php_ini(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_php_dir(kind, name).join("php.ini")
    }

    /// 宿主托管的扩展 ini 目录（挂到容器 `/usr/local/etc/php/conf.d-skiff`）。
    pub fn runtime_php_confd(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_php_dir(kind, name).join("conf.d")
    }

    /// PHP 实例的独立日志目录。
    pub fn runtime_log_dir(&self, kind: &str, name: &str) -> PathBuf {
        self.runtime_home(kind, name).join("log")
    }

    /// socket 的容器内路径；vhost 的 `fastcgi_pass` 必须用它。
    pub fn runtime_socket_container(&self, kind: &str, name: &str) -> String {
        runtime_container_socket(kind, name)
    }

    /// nginx 全局日志目录（错误日志与访问汇总）。
    pub fn global_log_dir(&self) -> PathBuf {
        self.web_root.join("log")
    }

    /// 内核生成并挂载进 nginx 容器的主配置。
    pub fn nginx_main_conf(&self) -> PathBuf {
        self.web_root.join("nginx").join("nginx.conf")
    }

    pub fn container_nginx_main_conf(&self) -> &'static str {
        "/usr/local/openresty/nginx/conf/nginx.conf"
    }

    /// 内核随包分发的本地工具目录：`<web_root>/skiff/tools`。
    ///
    /// 该目录整体挂载进 PHP 容器（容器内 `/www/skiff/tools`），扩展安装器从这里
    /// 读取，不再在运行时访问 GitHub。
    pub fn tools_dir(&self) -> PathBuf {
        self.web_root.join("skiff").join("tools")
    }

    /// 本地 mlocati 扩展安装器（宿主路径）。
    pub fn php_extension_installer(&self) -> PathBuf {
        self.tools_dir().join("install-php-extensions")
    }

    /// 宿主侧系统包缓存（alpine `apk` / debian `apt` 下载的包文件）。
    ///
    /// 挂进每个 PHP 容器的 `/var/cache/apk`。仅挂载目录还不够：apk-tools 3 在
    /// `/etc/apk/cache` 不存在时会回退到该静态目录，但默认不缓存 `.apk`；安装器
    /// prelude 会写 `/etc/apk/config` 打开 `cache-packages`，并设置
    /// `IPE_KEEP_SYSPKG_CACHE=1` 阻止收尾清理，才能让重建/同机多环境复用
    /// 已下载的依赖包。
    pub fn system_pkg_cache_dir(&self) -> PathBuf {
        self.runtime_dir.join("cache").join("apk")
    }

    /// 站点证书：`<站点目录>/ssl/cert.pem|key.pem`。
    pub fn cert_file(&self, site_dir: &Path) -> PathBuf {
        site_ssl_dir(site_dir).join("cert.pem")
    }

    pub fn key_file(&self, site_dir: &Path) -> PathBuf {
        site_ssl_dir(site_dir).join("key.pem")
    }

    /// 站点 vhost：`<站点目录>/conf/<域名>.conf`。
    pub fn vhost_file(&self, site_dir: &Path, domain: &str) -> PathBuf {
        site_conf_dir(site_dir).join(format!("{domain}.conf"))
    }

    pub fn vhost_disabled(&self, site_dir: &Path, domain: &str) -> PathBuf {
        site_conf_dir(site_dir).join(format!("{domain}.conf.disabled"))
    }

    /// 站点 include 入口内容：通配加载各站点自己的 `conf/*.conf`。
    ///
    /// `.conf.disabled` 不匹配 `*.conf`，因此停用站点天然不会被 nginx 加载。
    pub fn sites_include_conf(&self) -> String {
        format!(
            "# Skiff 站点 vhost 入口：每个站点的配置在自己的 conf/ 下，\n\
             # 备份 {sites}/<域名>/ 即备份整站。\n\
             include {sites}/*/conf/*.conf;\n",
            sites = self.container_sites_dir()
        )
    }
}

/// 内容一致时跳过写入；内容变化或文件缺失时原子落盘并设置权限。
pub(crate) fn write_if_changed(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    let unchanged = std::fs::read(path).map(|old| old == bytes).unwrap_or(false);
    if !unchanged {
        // 临时名必须唯一：同一路径可能被请求与巡检并发写入，共用 `x.tmp`
        // 会让后写者的截断覆盖掉先写者已写完的内容。
        let tmp = crate::atomic::sibling_temp(path);
        std::fs::write(&tmp, bytes).map_err(|e| format!("写入 {} 失败：{e}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))
                .map_err(|e| format!("设置 {} 权限失败：{e}", tmp.display()))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, path).map_err(|e| format!("替换 {} 失败：{e}", path.display()))?;
    }
    Ok(())
}

/// 只在文件缺失时写入：环境 `app/` 是用户的真实代码目录，重建/克隆不得覆盖已有文件。
pub(crate) fn seed_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    write_if_changed(path, bytes, mode)
}

/// Windows 上杀软/索引服务短暂占用刚写入的文件时 `remove_dir_all` 会返回
/// `Directory not empty`（os error 145）；用有界退避重试，最终仍失败才报错。
pub(crate) fn remove_dir_all_retry(path: &Path) -> std::io::Result<()> {
    let mut delay = std::time::Duration::from_millis(20);
    for attempt in 0..6 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 5 => {
                tracing::debug!(path = %path.display(), error = %e, "删除站点目录失败，重试");
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
