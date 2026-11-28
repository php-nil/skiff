//! PHP 专属配置读写与扩展安装辅助。
//!
//! 属于 sites::runtime 子模块；与 mod.rs / jobs.rs 共同组成 impl AppState。
use std::path::{Path, PathBuf};

use super::*;
use crate::AppState;

impl AppState {
    pub fn php_config_mounts_current(&self, id: &str) -> Result<bool, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        Ok(s.runtime_php_dir(&rt.kind, &rt.name)
            .join(PHP_CONFIG_SENTINEL)
            .exists())
    }

    pub fn php_config_files(&self, id: &str) -> Result<Vec<PhpConfigFile>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let mut files = vec![
            PhpConfigFile {
                key: "php.ini".into(),
                label: "php.ini".into(),
                host_path: s.runtime_php_ini(&rt.kind, &rt.name).display().to_string(),
                container_path: CONTAINER_PHP_INI.into(),
                exists: s.runtime_php_ini(&rt.kind, &rt.name).exists(),
                editable: true,
                note:
                    "PHP 主配置；页面「PHP 参数」中的 memory_limit 等四项写在 FPM 池，优先级更高。"
                        .into(),
                warnings: Vec::new(),
            },
            PhpConfigFile {
                key: "www.conf".into(),
                label: "www.conf（FPM 池）".into(),
                host_path: s
                    .runtime_pool_conf(&rt.kind, &rt.name)
                    .display()
                    .to_string(),
                container_path: CONTAINER_PHP_POOL.into(),
                exists: s.runtime_pool_conf(&rt.kind, &rt.name).exists(),
                editable: true,
                note: "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。"
                    .into(),
                warnings: Vec::new(),
            },
        ];
        let confd = s.runtime_php_confd(&rt.kind, &rt.name);
        if let Ok(entries) = std::fs::read_dir(&confd) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".ini") {
                    continue;
                }
                files.push(PhpConfigFile {
                    key: format!("conf.d/{name}"),
                    label: format!("扩展 {name}"),
                    host_path: entry.path().display().to_string(),
                    container_path: format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    exists: true,
                    editable: true,
                    note: "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。".into(),
                    warnings: Vec::new(),
                });
            }
        }
        Ok(files)
    }

    pub fn read_php_config_file(
        &self,
        id: &str,
        key: &str,
        actor: &str,
    ) -> Result<PhpConfigFileView, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        let content = if path.exists() {
            self.admin_fs()
                .read(actor, &path.display().to_string())
                .map_err(|e| SiteError::Internal(format!("读取 {key} 失败：{e}")))?
                .text
        } else {
            String::new()
        };
        Ok(PhpConfigFileView {
            file: meta,
            content,
        })
    }

    pub async fn save_php_config_file(
        &self,
        id: &str,
        key: &str,
        content: &str,
        actor: &str,
    ) -> Result<PhpConfigFile, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let (path, meta) = self.php_config_target(&rt, &s, key)?;
        // 旧实例可能还没有宿主托管的 php/ 目录：先补齐，否则候选文件无处落盘。
        self.ensure_php_config_files(&s, &rt.kind, &rt.name)
            .map_err(|e| SiteError::Internal(format!("准备 PHP 配置目录失败：{e}")))?;
        if content.len() > 256 * 1024 {
            return Err(SiteError::BadRequest(
                "配置文件超过 256 KiB 编辑上限".into(),
            ));
        }
        let container = runtime_container_name(&rt.kind, &rt.name);
        let running = self
            .docker()
            .list_containers()
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取容器状态失败：{e}")))?
            .iter()
            .any(|c| c.name == container && c.state == "running");
        if !running {
            return Err(SiteError::Unavailable(
                "运行环境未在运行，无法在容器内校验配置；请先启动实例".into(),
            ));
        }
        // 候选文件放在 web_root 内，容器内可直接读到同一个文件。
        let candidate = s
            .runtime_php_dir(&rt.kind, &rt.name)
            .join(format!("{key}.candidate"));
        self.admin_fs()
            .write(actor, &candidate.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入候选文件失败：{e}")))?;
        let candidate_container = s.container_path(&candidate);
        let (code, out) = match key {
            "www.conf" => {
                // php-fpm -t 只接受主配置；用临时主配置 include 候选池文件。
                let main = s
                    .runtime_php_dir(&rt.kind, &rt.name)
                    .join("php-fpm-test.conf");
                let main_text = format!(
                    "[global]\n\
                     error_log = /dev/stderr\n\
                     include = {candidate_container}\n"
                );
                self.admin_fs()
                    .write(actor, &main.display().to_string(), &main_text)
                    .map_err(|e| SiteError::Internal(format!("写入校验主配置失败：{e}")))?;
                let main_container = s.container_path(&main);
                self.docker()
                    .exec_capture(
                        &container,
                        vec![
                            "/usr/local/sbin/php-fpm".into(),
                            "-t".into(),
                            "-y".into(),
                            main_container,
                        ],
                    )
                    .await
                    .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?
            }
            _ => self
                .docker()
                .exec_capture(
                    &container,
                    vec![
                        "php".into(),
                        "-c".into(),
                        candidate_container,
                        "-r".into(),
                        "echo \"skiff-ok\";".into(),
                    ],
                )
                .await
                .map_err(|e| SiteError::Unavailable(format!("容器内校验失败：{e}")))?,
        };
        // 只拦语法错误与启动失败：退出码非 0，或（非池配置）没跑进 `-r` 就退出，
        // 或输出里出现 `syntax error`。最后一条不能省：PHP 的 ini 语法错误只打印到
        // stdout（`PHP:  syntax error, unexpected ...`），进程仍会执行 `-r` 并返回 0，
        // 只看退出码/`skiff-ok` 会把坏配置放过去（上机 FS 验收复现）。
        // `PHP Warning` 不阻止保存，但要收进 warnings 原文回显给用户。
        let invalid = code != 0
            || php_output_has_syntax_error(&out)
            || (key != "www.conf" && !out.contains("skiff-ok"));
        if invalid {
            let reason = out.trim();
            return Err(SiteError::BadRequest(if reason.is_empty() {
                "配置语法校验失败，已拒绝保存".into()
            } else {
                format!("配置语法校验失败，已拒绝保存：{reason}")
            }));
        }
        let warnings = php_validation_warnings(&out);
        let original = std::fs::read(&path).ok();
        self.admin_fs()
            .write(actor, &path.display().to_string(), content)
            .map_err(|e| SiteError::Internal(format!("写入 {key} 失败：{e}")))?;
        // 重启后必须复核容器仍在 running：Docker restart 返回成功不代表 FPM
        // 没被坏配置带崩（与 nginx 主配置保存同一条静默路径）。
        let problem = match self.docker_runtime_action(&container, Verb::Restart).await {
            Err(e) => Some(format!("{e}")),
            Ok(()) => {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                match self.docker().list_containers().await {
                    Ok(rows)
                        if rows
                            .iter()
                            .any(|c| c.name == container && c.state == "running") =>
                    {
                        None
                    }
                    Ok(_) => {
                        Some("容器重启后未保持 running（配置可能导致 FPM 启动失败）".to_string())
                    }
                    Err(e) => Some(format!("重启后核对容器状态失败：{e}")),
                }
            }
        };
        if let Some(reason) = problem {
            match &original {
                Some(bytes) if !bytes.is_empty() => {
                    if let Ok(text) = std::str::from_utf8(bytes) {
                        if let Err(we) =
                            self.admin_fs()
                                .write(actor, &path.display().to_string(), text)
                        {
                            tracing::error!(path = %path.display(), error = %we, "回滚站点配置写入失败");
                        }
                    }
                }
                _ => {
                    if let Err(e) = self.admin_fs().remove(actor, &path.display().to_string()) {
                        tracing::debug!(path = %path.display(), error = %e, "回滚新增的 PHP 配置文件失败");
                    }
                }
            }
            if let Err(e) = self.docker_runtime_action(&container, Verb::Restart).await {
                tracing::error!(error = %e, runtime = %container, "回滚 PHP 配置后重启运行环境失败");
            }
            return Err(SiteError::Unavailable(format!(
                "保存后重启 PHP-FPM 失败，已回滚 {key}：{reason}"
            )));
        }
        Ok(PhpConfigFile {
            exists: true,
            warnings,
            ..meta
        })
    }

    pub async fn php_effective_values(&self, id: &str) -> Result<Vec<PhpIniValue>, SiteError> {
        let rt = self.php_runtime(id)?;
        let s = self.site_settings();
        let container = runtime_container_name(&rt.kind, &rt.name);
        let (code, out) = self
            .docker()
            .exec_capture(&container, vec!["php".into(), "-i".into()])
            .await
            .map_err(|e| SiteError::Unavailable(format!("读取 php -i 失败：{e}")))?;
        if code != 0 {
            return Err(SiteError::Unavailable(format!(
                "php -i 退出码 {code}，无法读取生效值"
            )));
        }
        let effective = parse_php_i(&out);
        let file_text =
            std::fs::read_to_string(s.runtime_php_ini(&rt.kind, &rt.name)).unwrap_or_default();
        let file_values = parse_ini_assignments(&file_text);
        let managed = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
        ];
        let keys = [
            "memory_limit",
            "upload_max_filesize",
            "post_max_size",
            "max_execution_time",
            "display_errors",
            "error_reporting",
            "date.timezone",
            "opcache.enable",
        ];
        Ok(keys
            .iter()
            .map(|key| {
                let eff = effective
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（未读取）".into());
                let file = file_values
                    .iter()
                    .rev()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| "（文件未设置）".into());
                let source = if managed.contains(key) && file != "（文件未设置）" {
                    "FPM 池 php_admin_value 优先；文件值仅在其他 SAPI 生效".to_string()
                } else if file != "（文件未设置）" && file == eff {
                    "php.ini".to_string()
                } else if file != "（文件未设置）" {
                    "其他 conf.d / 镜像默认覆盖".to_string()
                } else {
                    "镜像默认或其他 conf.d".to_string()
                };
                PhpIniValue {
                    key: key.to_string(),
                    file_value: file,
                    effective: eff,
                    source,
                }
            })
            .collect())
    }

    fn php_runtime(&self, id: &str) -> Result<Runtime, SiteError> {
        let rt = self
            .db()
            .runtime_by_id(id)
            .map_err(|e| SiteError::Internal(e.to_string()))?
            .ok_or_else(|| SiteError::BadRequest(format!("运行环境不存在：{id}")))?;
        if rt.kind != skiff_store::KIND_PHP {
            return Err(SiteError::Unavailable(format!(
                "{} 运行环境没有 PHP 专属配置",
                rt.kind
            )));
        }
        Ok(rt)
    }

    fn php_config_target(
        &self,
        rt: &Runtime,
        s: &SiteSettings,
        key: &str,
    ) -> Result<(PathBuf, PhpConfigFile), SiteError> {
        let (path, container_path, label, note) = match key {
            "php.ini" => (
                s.runtime_php_ini(&rt.kind, &rt.name),
                CONTAINER_PHP_INI.to_string(),
                "php.ini".to_string(),
                "PHP 主配置；页面「PHP 参数」中的四项写在 FPM 池，优先级更高。",
            ),
            "www.conf" => (
                s.runtime_pool_conf(&rt.kind, &rt.name),
                CONTAINER_PHP_POOL.to_string(),
                "www.conf（FPM 池）".to_string(),
                "FPM 进程/监听配置；「PHP 参数」保存时会重写本文件（表单字段为权威）。",
            ),
            other => {
                let name = other
                    .strip_prefix("conf.d/")
                    .ok_or_else(|| SiteError::BadRequest(format!("未知配置文件：{other}")))?;
                let p = Path::new(name);
                if name.is_empty()
                    || p.components().count() != 1
                    || !name.ends_with(".ini")
                    || name.contains('\\')
                {
                    return Err(SiteError::BadRequest("扩展 ini 文件名不合法".into()));
                }
                (
                    s.runtime_php_confd(&rt.kind, &rt.name).join(name),
                    format!("{CONTAINER_PHP_SKIFF_CONFD}/{name}"),
                    format!("扩展 {name}"),
                    "宿主托管的扩展 ini，PHP 启动时在镜像 conf.d 之后扫描。",
                )
            }
        };
        Ok((
            path.clone(),
            PhpConfigFile {
                key: key.to_string(),
                label,
                host_path: path.display().to_string(),
                container_path,
                exists: path.exists(),
                editable: true,
                note: note.to_string(),
                warnings: Vec::new(),
            },
        ))
    }
}
