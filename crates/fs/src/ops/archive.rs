//! 压缩/解压：`archive` / `extract`，先支持 `zip` 与 `tar.gz`。
//!
//! 不经过 read/write 逐文件拼装：打包由服务端直接流式写压缩包；解压**边解边
//! 判断**——每个条目在写入前校验相对路径、条目数、单条体积与累计字节，超限
//! 立即中止并按记录逆序清理已解出的半成品，绝不做「先解再扫描」。
//!
//! 安全口径：
//! - 拒绝绝对路径、`..` 穿越、反斜杠路径与非 UTF-8 名称；
//! - 符号链接/硬链接/特殊文件默认不解出，只计数（不会悄悄写出指向外部的链接）；
//! - 目标目录与压缩包落点同样走受保护路径与配额执行点；
//! - 打包侧同样限制条目数、源总字节与递归深度。

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::audit::AuditAction;
use crate::config::QuotaLimit;
use crate::error::{io_error, io_error_ctx, FsError};
use crate::path::{PathKind, PathMode};
use crate::subject::Principal;

use super::{io as fsio, ok_args, OpContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

impl ArchiveFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::TarGz => "tar.gz",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "zip" => Some(ArchiveFormat::Zip),
            "tar.gz" | "tgz" | "targz" => Some(ArchiveFormat::TarGz),
            _ => None,
        }
    }

    pub fn from_path(path: &str) -> Option<Self> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".zip") {
            Some(ArchiveFormat::Zip)
        } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            Some(ArchiveFormat::TarGz)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveRequest {
    /// 要打包的绝对路径（文件/目录，可多选）。
    pub sources: Vec<String>,
    /// 压缩包绝对路径。
    pub dest: String,
    pub format: ArchiveFormat,
    /// 目标已存在时是否覆盖（默认拒绝）。
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArchiveOutcome {
    pub path: String,
    pub format: String,
    pub entries: u64,
    pub bytes: u64,
    pub skipped_symlinks: u64,
    pub skipped_special: u64,
}

#[derive(Debug, Clone)]
pub struct ExtractRequest {
    /// 压缩包绝对路径（扩展名决定格式）。
    pub archive: String,
    /// 解压目标目录（必须已存在的目录）。
    pub dest_dir: String,
    /// 同名文件已存在时是否覆盖（默认拒绝）。
    pub overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExtractOutcome {
    pub dest: String,
    pub entries: u64,
    pub files: u64,
    pub bytes: u64,
    pub skipped_symlinks: u64,
    pub skipped_special: u64,
}

#[derive(Debug, Clone, Copy)]
struct ArchiveLimits {
    max_entries: usize,
    max_total_bytes: u64,
    max_depth: usize,
}

struct PlanEntry {
    rel: String,
    src: PathBuf,
    dir: bool,
    size: u64,
    mode: u32,
    mtime: i64,
}

#[derive(Default)]
struct Plan {
    entries: Vec<PlanEntry>,
    total_bytes: u64,
    skipped_symlinks: u64,
    skipped_special: u64,
}

impl super::Fs {
    pub fn archive(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &ArchiveRequest,
    ) -> Result<ArchiveOutcome, FsError> {
        let limits = ArchiveLimits {
            max_entries: self.config().archive_max_entries,
            max_total_bytes: self.config().archive_max_total_bytes,
            max_depth: self.config().archive_max_depth,
        };
        let plan = match self.plan_sources(p, &req.sources, limits) {
            Ok(plan) => plan,
            Err(e) => {
                let src = req.sources.first().map(String::as_str).unwrap_or("");
                return self.fail(p, ctx, AuditAction::Archive, src, e);
            }
        };
        if plan.entries.is_empty() {
            let src = req.sources.first().map(String::as_str).unwrap_or("");
            return self.fail(
                p,
                ctx,
                AuditAction::Archive,
                src,
                FsError::Conflict("没有可打包的普通文件或目录".into()),
            );
        }

        let dr = match self.resolve(p, &req.dest, PathMode::Create) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Archive, &req.dest, e),
        };
        if dr.kind == PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Archive,
                &dr,
                FsError::Conflict("压缩包目标已存在且是目录".into()),
            );
        }
        if dr.kind == PathKind::File && !req.overwrite {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Archive,
                &dr,
                FsError::Conflict("压缩包目标已存在，请确认覆盖或改名".into()),
            );
        }
        if let Err(e) = self.ensure_not_protected(&dr, "写入压缩包") {
            return self.fail_resolved(p, ctx, AuditAction::Archive, &dr, e);
        }
        if let Err(e) = self.ensure_write_allowed(p, &dr) {
            return self.fail_resolved(p, ctx, AuditAction::Archive, &dr, e);
        }
        let old_size = if dr.kind == PathKind::File {
            fs::symlink_metadata(&dr.path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };
        let quota_target = self.quota_target(p, &dr);
        let files_delta = i64::from(dr.kind == PathKind::Missing);
        if let Err(e) = self.enforce_quota(quota_target.as_ref(), 0, files_delta) {
            return self.fail_resolved(p, ctx, AuditAction::Archive, &dr, e);
        }

        let out = self.locks.with_write(
            std::slice::from_ref(&dr.path),
            self.config().lock_timeout,
            || -> Result<u64, FsError> {
                match fs::symlink_metadata(&dr.path) {
                    Ok(meta) if meta.is_dir() => {
                        return Err(FsError::Conflict("压缩包目标已存在且是目录".into()));
                    }
                    Ok(_) if !req.overwrite => {
                        return Err(FsError::Conflict(
                            "压缩包目标已存在，请确认覆盖或改名".into(),
                        ));
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error(&dr.path, e)),
                }
                let (tmp, bytes) = write_archive_temp(&plan, &dr.path, req.format, limits)?;
                if let Err(e) = self.enforce_quota(
                    quota_target.as_ref(),
                    bytes as i64 - old_size as i64,
                    files_delta,
                ) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
                if let Err(e) = commit_temp(&tmp, &dr.path) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
                Ok(bytes)
            },
        );
        let bytes = match out {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(e)) | Err(e) => {
                return self.fail_resolved(p, ctx, AuditAction::Archive, &dr, e);
            }
        };

        if let Some(target) = quota_target.as_ref() {
            let delta = bytes as i64 - old_size as i64;
            self.quota().adjust_stats(&target.root, delta, files_delta);
        }

        let mut a = ok_args(AuditAction::Archive);
        if let Some(first) = req.sources.first() {
            a.src = Some(first.clone());
        }
        a.destination(&dr);
        a.bytes = bytes;
        a.detail = Some(format!(
            "format={}, entries={}, sources={}, skipped_symlinks={}, skipped_special={}",
            req.format.as_str(),
            plan.entries.len(),
            req.sources.len(),
            plan.skipped_symlinks,
            plan.skipped_special
        ));
        self.audit(p, ctx, a);
        Ok(ArchiveOutcome {
            path: dr.path.display().to_string(),
            format: req.format.as_str().to_string(),
            entries: plan.entries.len() as u64,
            bytes,
            skipped_symlinks: plan.skipped_symlinks,
            skipped_special: plan.skipped_special,
        })
    }

    pub fn extract(
        &self,
        p: &Principal,
        ctx: &OpContext,
        req: &ExtractRequest,
    ) -> Result<ExtractOutcome, FsError> {
        let ar = match self.resolve(p, &req.archive, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Extract, &req.archive, e),
        };
        if ar.kind != PathKind::File {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Extract,
                &ar,
                FsError::Conflict("压缩包必须是普通文件".into()),
            );
        }
        let dr = match self.resolve(p, &req.dest_dir, PathMode::Existing) {
            Ok(r) => r,
            Err(e) => return self.fail(p, ctx, AuditAction::Extract, &req.dest_dir, e),
        };
        if dr.kind != PathKind::Dir {
            return self.fail_resolved(
                p,
                ctx,
                AuditAction::Extract,
                &dr,
                FsError::Conflict("解压目标必须是已存在的目录".into()),
            );
        }
        let format = match ArchiveFormat::from_path(&ar.path.to_string_lossy()) {
            Some(f) => f,
            None => {
                let e = FsError::Unsupported(format!(
                    "无法从扩展名识别压缩格式（仅支持 .zip / .tar.gz / .tgz）: {}",
                    ar.path.display()
                ));
                return self.fail_resolved(p, ctx, AuditAction::Extract, &ar, e);
            }
        };
        if let Err(e) = self.ensure_not_protected(&dr, "解压到") {
            return self.fail_resolved(p, ctx, AuditAction::Extract, &dr, e);
        }
        if let Err(e) = self.ensure_write_allowed(p, &dr) {
            return self.fail_resolved(p, ctx, AuditAction::Extract, &dr, e);
        }

        let limits = ExtractLimits {
            max_entries: self.config().extract_max_entries,
            max_entry_bytes: self.config().extract_max_entry_bytes,
            max_total_bytes: self.config().extract_max_total_bytes,
        };
        let quota_target = self.quota_target(p, &dr);
        let mut st = ExtractState::new(dr.path.clone(), limits, req.overwrite);
        let result = self.run_extract(&ar.path, format, &mut st, quota_target.as_ref());
        match result {
            Ok(()) => {
                if let Some(target) = quota_target.as_ref() {
                    self.quota()
                        .adjust_stats(&target.root, st.delta_bytes, st.delta_files);
                }
                let mut a = ok_args(AuditAction::Extract);
                a.src = Some(ar.path.display().to_string());
                a.destination(&dr);
                a.bytes = st.total_bytes;
                a.detail = Some(format!(
                    "format={}, overwrite={}, entries={}, files={}, skipped_symlinks={}, skipped_special={}",
                    format.as_str(),
                    req.overwrite,
                    st.entries,
                    st.files,
                    st.skipped_symlinks,
                    st.skipped_special
                ));
                self.audit(p, ctx, a);
                Ok(ExtractOutcome {
                    dest: dr.path.display().to_string(),
                    entries: st.entries as u64,
                    files: st.files,
                    bytes: st.total_bytes,
                    skipped_symlinks: st.skipped_symlinks,
                    skipped_special: st.skipped_special,
                })
            }
            Err(e) => {
                // 超限/穿越/冲突：错误在检测点即刻中止循环，这里当场清理本次半成品；
                // 清理失败的路径必须回写进错误信息，不能让用户留下面上看不见的残留。
                let leftovers = st.cleanup();
                let e = if leftovers.is_empty() {
                    e
                } else {
                    e.with_note(&leftover_note(&leftovers))
                };
                self.fail_resolved(p, ctx, AuditAction::Extract, &dr, e)
            }
        }
    }

    fn run_extract(
        &self,
        archive: &Path,
        format: ArchiveFormat,
        st: &mut ExtractState,
        quota: Option<&QuotaLimit>,
    ) -> Result<(), FsError> {
        match format {
            ArchiveFormat::Zip => self.extract_zip(archive, st, quota),
            ArchiveFormat::TarGz => self.extract_tar_gz(archive, st, quota),
        }
    }

    fn extract_zip(
        &self,
        archive: &Path,
        st: &mut ExtractState,
        quota: Option<&QuotaLimit>,
    ) -> Result<(), FsError> {
        let file = fsio::open_read(archive)?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| zip_err(archive, e))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| zip_err(archive, e))?;
            st.count_entry()?;
            let name = entry.name().to_string();
            let rel = safe_rel(&name)?;
            let mode = entry.unix_mode().unwrap_or(0);
            let kind_bits = mode & 0o170000;
            if kind_bits == 0o120000 {
                st.skipped_symlinks += 1;
                continue;
            }
            if entry.is_dir() || name.ends_with('/') {
                st.ensure_dir(&rel)?;
                continue;
            }
            if kind_bits != 0 && kind_bits != 0o100000 {
                st.skipped_special += 1;
                continue;
            }
            let size = entry.size();
            self.extract_file(archive, &mut entry, &rel, size, st, quota)?;
        }
        Ok(())
    }

    fn extract_tar_gz(
        &self,
        archive: &Path,
        st: &mut ExtractState,
        quota: Option<&QuotaLimit>,
    ) -> Result<(), FsError> {
        let file = fsio::open_read(archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        let entries = tar
            .entries()
            .map_err(|e| io_error_ctx(archive, e, "读取 tar.gz 失败"))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| io_error_ctx(archive, e, "读取 tar.gz 条目失败"))?;
            st.count_entry()?;
            let path = entry
                .path()
                .map_err(|e| io_error_ctx(archive, e, "读取 tar.gz 条目路径失败"))?
                .to_string_lossy()
                .to_string();
            let rel = safe_rel(&path)?;
            let kind = entry.header().entry_type();
            if kind.is_symlink() || kind.is_hard_link() {
                st.skipped_symlinks += 1;
                continue;
            }
            if kind.is_dir() {
                st.ensure_dir(&rel)?;
                continue;
            }
            if kind.is_file() {
                let size = entry
                    .header()
                    .size()
                    .map_err(|e| io_error_ctx(archive, e, "读取 tar.gz 条目大小失败"))?;
                self.extract_file(archive, &mut entry, &rel, size, st, quota)?;
            } else {
                st.skipped_special += 1;
            }
        }
        Ok(())
    }

    fn extract_file<R: Read>(
        &self,
        archive: &Path,
        reader: &mut R,
        rel: &Path,
        declared: u64,
        st: &mut ExtractState,
        quota: Option<&QuotaLimit>,
    ) -> Result<(), FsError> {
        if declared > st.limits.max_entry_bytes {
            return Err(FsError::FileTooLarge(format!(
                "单条解压体积 {} 字节超过上限 {} 字节（{}）",
                declared,
                st.limits.max_entry_bytes,
                rel.display()
            )));
        }
        if let Some(parent) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            st.ensure_dir(parent)?;
        }
        let target = st.dest.join(rel);
        let (new_file, old_size) = match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(FsError::Escape(format!(
                    "解压目标已存在符号链接，拒绝覆盖: {}",
                    target.display()
                )));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(FsError::Conflict(format!(
                    "解压目标已存在同名目录: {}",
                    target.display()
                )));
            }
            Ok(meta) => {
                if !st.overwrite {
                    return Err(FsError::Conflict(format!(
                        "解压目标已存在同名文件: {}（如需覆盖请勾选）",
                        target.display()
                    )));
                }
                (false, meta.len())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (true, 0),
            Err(e) => return Err(io_error(&target, e)),
        };
        if st.total_bytes.saturating_add(declared) > st.limits.max_total_bytes {
            return Err(FsError::TooLarge(format!(
                "解压累计字节超过上限 {} 字节（条目 {}）",
                st.limits.max_total_bytes,
                rel.display()
            )));
        }
        let projected_delta = st.delta_bytes + declared as i64 - old_size as i64;
        let projected_files = st.delta_files + i64::from(new_file);
        self.enforce_quota(quota, projected_delta, projected_files)?;

        let mut out = open_extract_target(&target, new_file)?;
        if new_file {
            // 先登记再写：写一半失败时由 `extract` 的统一清理按记录移除半成品。
            st.created.push((target.clone(), false));
        }
        let mut written = 0u64;
        let mut buf = vec![0u8; 64 * 1024];
        let write_result = (|| -> Result<u64, FsError> {
            loop {
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| io_error_ctx(archive, e, "读取压缩包条目失败"))?;
                if n == 0 {
                    break;
                }
                written = written.saturating_add(n as u64);
                if written > declared {
                    return Err(FsError::TooLarge(format!(
                        "条目实际解出 {} 字节超过声明 {} 字节，疑似损坏或恶意压缩包（{}）",
                        written,
                        declared,
                        rel.display()
                    )));
                }
                if written > st.limits.max_entry_bytes {
                    return Err(FsError::FileTooLarge(format!(
                        "单条解压体积超过上限 {} 字节（{}）",
                        st.limits.max_entry_bytes,
                        rel.display()
                    )));
                }
                if st.total_bytes.saturating_add(written) > st.limits.max_total_bytes {
                    return Err(FsError::TooLarge(format!(
                        "解压累计字节超过上限 {} 字节（条目 {}）",
                        st.limits.max_total_bytes,
                        rel.display()
                    )));
                }
                out.write_all(&buf[..n]).map_err(|e| io_error(&target, e))?;
            }
            Ok(written)
        })();
        drop(out);
        let written = write_result?;
        st.total_bytes = st.total_bytes.saturating_add(written);
        st.files += 1;
        st.delta_bytes += written as i64 - old_size as i64;
        if new_file {
            st.delta_files += 1;
        }
        self.enforce_quota(quota, st.delta_bytes, st.delta_files)?;
        Ok(())
    }

    fn plan_sources(
        &self,
        p: &Principal,
        sources: &[String],
        limits: ArchiveLimits,
    ) -> Result<Plan, FsError> {
        let mut plan = Plan::default();
        let mut names = HashSet::new();
        for raw in sources {
            let r = self.resolve(p, raw, PathMode::ExistingLink)?;
            let base = r
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| FsError::Denied(format!("路径缺少可用的文件名: {raw}")))?;
            let meta = fs::symlink_metadata(&r.path).map_err(|e| io_error(&r.path, e))?;
            if meta.file_type().is_symlink() {
                plan.skipped_symlinks += 1;
                continue;
            }
            if meta.is_dir() {
                push_entry(
                    &mut plan,
                    &mut names,
                    base.to_string(),
                    r.path.clone(),
                    true,
                    0,
                    mode_of(&meta, 0o755),
                    mtime_of(&meta),
                    limits,
                )?;
                self.walk_dir(&r.path, base, 1, &mut plan, &mut names, limits)?;
            } else if meta.is_file() {
                push_entry(
                    &mut plan,
                    &mut names,
                    base.to_string(),
                    r.path.clone(),
                    false,
                    meta.len(),
                    mode_of(&meta, 0o644),
                    mtime_of(&meta),
                    limits,
                )?;
            } else {
                plan.skipped_special += 1;
            }
        }
        Ok(plan)
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_dir(
        &self,
        dir: &Path,
        prefix: &str,
        depth: usize,
        plan: &mut Plan,
        names: &mut HashSet<String>,
        limits: ArchiveLimits,
    ) -> Result<(), FsError> {
        if depth > limits.max_depth {
            return Err(FsError::TooLarge(format!(
                "打包目录深度超过上限 {}: {}",
                limits.max_depth,
                dir.display()
            )));
        }
        let mut items: Vec<fs::DirEntry> = fs::read_dir(dir)
            .map_err(|e| io_error(dir, e))?
            .collect::<Result<_, _>>()
            .map_err(|e| io_error(dir, e))?;
        items.sort_by_key(|e| e.file_name());
        for item in items {
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| FsError::Denied("目录含非 UTF-8 名称".into()))?;
            let rel = format!("{prefix}/{name}");
            let meta = fs::symlink_metadata(item.path()).map_err(|e| io_error(&item.path(), e))?;
            if meta.file_type().is_symlink() {
                plan.skipped_symlinks += 1;
                continue;
            }
            if meta.is_dir() {
                push_entry(
                    plan,
                    names,
                    rel.clone(),
                    item.path(),
                    true,
                    0,
                    mode_of(&meta, 0o755),
                    mtime_of(&meta),
                    limits,
                )?;
                self.walk_dir(&item.path(), &rel, depth + 1, plan, names, limits)?;
            } else if meta.is_file() {
                push_entry(
                    plan,
                    names,
                    rel,
                    item.path(),
                    false,
                    meta.len(),
                    mode_of(&meta, 0o644),
                    mtime_of(&meta),
                    limits,
                )?;
            } else {
                plan.skipped_special += 1;
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn push_entry(
    plan: &mut Plan,
    names: &mut HashSet<String>,
    rel: String,
    src: PathBuf,
    dir: bool,
    size: u64,
    mode: u32,
    mtime: i64,
    limits: ArchiveLimits,
) -> Result<(), FsError> {
    if plan.entries.len() + 1 > limits.max_entries {
        return Err(FsError::TooManyEntries(format!(
            "压缩包条目超过上限 {}",
            limits.max_entries
        )));
    }
    if !names.insert(rel.clone()) {
        return Err(FsError::Conflict(format!(
            "压缩包内条目重名: {rel}（请分开打包同名文件）"
        )));
    }
    if !dir {
        plan.total_bytes = plan.total_bytes.saturating_add(size);
        if plan.total_bytes > limits.max_total_bytes {
            return Err(FsError::TooLarge(format!(
                "打包源文件总字节超过上限 {} 字节",
                limits.max_total_bytes
            )));
        }
    }
    plan.entries.push(PlanEntry {
        rel,
        src,
        dir,
        size,
        mode,
        mtime,
    });
    Ok(())
}

fn mode_of(meta: &fs::Metadata, default: u32) -> u32 {
    fsio::mode_of(meta).unwrap_or(default)
}

fn mtime_of(meta: &fs::Metadata) -> i64 {
    fsio::mtime_of(meta).unwrap_or(0)
}

fn write_archive_temp(
    plan: &Plan,
    dest: &Path,
    format: ArchiveFormat,
    limits: ArchiveLimits,
) -> Result<(PathBuf, u64), FsError> {
    let parent = dest
        .parent()
        .ok_or_else(|| FsError::Denied("压缩包路径无父目录".into()))?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| FsError::Denied("压缩包路径缺少文件名或文件名非 UTF-8".into()))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}-{}",
        name,
        std::process::id(),
        tmp_seq()
    ));
    let result = (|| -> Result<(PathBuf, u64), FsError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_error(&tmp, e))?;
        match format {
            ArchiveFormat::Zip => write_zip(plan, &tmp, file, limits)?,
            ArchiveFormat::TarGz => write_tar_gz(plan, &tmp, file, limits)?,
        }
        let bytes = fs::symlink_metadata(&tmp)
            .map_err(|e| io_error(&tmp, e))?
            .len();
        Ok((tmp.clone(), bytes))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_zip(plan: &Plan, dest: &Path, file: File, limits: ArchiveLimits) -> Result<(), FsError> {
    let mut zip = zip::ZipWriter::new(file);
    let mut total = 0u64;
    for entry in &plan.entries {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(entry.mode);
        if entry.dir {
            let name = format!("{}/", entry.rel);
            zip.add_directory(name, options)
                .map_err(|e| zip_err(dest, e))?;
            continue;
        }
        zip.start_file(&entry.rel, options)
            .map_err(|e| zip_err(dest, e))?;
        let mut src = fsio::open_read(&entry.src)?.take(entry.size);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = src.read(&mut buf).map_err(|e| io_error(&entry.src, e))?;
            if n == 0 {
                break;
            }
            total = total.saturating_add(n as u64);
            if total > limits.max_total_bytes {
                return Err(FsError::TooLarge(format!(
                    "打包读取总字节超过上限 {} 字节",
                    limits.max_total_bytes
                )));
            }
            zip.write_all(&buf[..n])
                .map_err(|e| io_error_ctx(dest, e, "写入 zip 失败"))?;
        }
    }
    zip.finish().map_err(|e| zip_err(dest, e))?;
    Ok(())
}

fn write_tar_gz(
    plan: &Plan,
    dest: &Path,
    file: File,
    limits: ArchiveLimits,
) -> Result<(), FsError> {
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    let mut total = 0u64;
    for entry in &plan.entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(entry.mode);
        header.set_mtime(entry.mtime.max(0) as u64);
        header.set_uid(0);
        header.set_gid(0);
        if entry.dir {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_cksum();
            tar.append_data(&mut header, format!("{}/", entry.rel), io::empty())
                .map_err(|e| io_error_ctx(dest, e, "写入 tar.gz 失败"))?;
            continue;
        }
        total = total.saturating_add(entry.size);
        if total > limits.max_total_bytes {
            return Err(FsError::TooLarge(format!(
                "打包源文件总字节超过上限 {} 字节",
                limits.max_total_bytes
            )));
        }
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(entry.size);
        header.set_cksum();
        let src = fsio::open_read(&entry.src)?;
        let mut reader = src.take(entry.size);
        tar.append_data(&mut header, &entry.rel, &mut reader)
            .map_err(|e| io_error_ctx(dest, e, "写入 tar.gz 失败"))?;
    }
    let encoder = tar
        .into_inner()
        .map_err(|e| io_error_ctx(dest, e, "收尾 tar.gz 失败"))?;
    encoder
        .finish()
        .map_err(|e| io_error_ctx(dest, e, "收尾 gzip 失败"))?;
    Ok(())
}

fn commit_temp(tmp: &Path, dest: &Path) -> Result<(), FsError> {
    #[cfg(windows)]
    {
        if fs::symlink_metadata(dest).is_ok() {
            fs::remove_file(dest).map_err(|e| io_error(dest, e))?;
        }
    }
    fs::rename(tmp, dest).map_err(|e| io_error(dest, e))
}

/// zip crate 的 `ZipError` 没有路径字段；这里统一补可读上下文。
fn zip_err(path: &Path, e: zip::result::ZipError) -> FsError {
    match e {
        zip::result::ZipError::Io(io) => io_error(path, io),
        other => FsError::Unsupported(format!(
            "压缩包格式不受支持或已损坏（{}）: {other}",
            path.display()
        )),
    }
}

struct ExtractLimits {
    max_entries: usize,
    max_entry_bytes: u64,
    max_total_bytes: u64,
}

struct ExtractState {
    dest: PathBuf,
    overwrite: bool,
    limits: ExtractLimits,
    entries: usize,
    files: u64,
    total_bytes: u64,
    delta_bytes: i64,
    delta_files: i64,
    skipped_symlinks: u64,
    skipped_special: u64,
    /// 本次新建的对象（逆序清理；覆盖已存在文件不算）。
    created: Vec<(PathBuf, bool)>,
}

impl ExtractState {
    fn new(dest: PathBuf, limits: ExtractLimits, overwrite: bool) -> Self {
        Self {
            dest,
            overwrite,
            limits,
            entries: 0,
            files: 0,
            total_bytes: 0,
            delta_bytes: 0,
            delta_files: 0,
            skipped_symlinks: 0,
            skipped_special: 0,
            created: Vec::new(),
        }
    }

    fn count_entry(&mut self) -> Result<(), FsError> {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(FsError::TooManyEntries(format!(
                "解压条目超过上限 {}",
                self.limits.max_entries
            )));
        }
        Ok(())
    }

    fn ensure_dir(&mut self, rel: &Path) -> Result<PathBuf, FsError> {
        let mut cur = self.dest.clone();
        for c in rel.components() {
            let Component::Normal(name) = c else {
                return Err(FsError::Denied(format!(
                    "解压条目路径非法: {}",
                    rel.display()
                )));
            };
            cur.push(name);
            match fs::symlink_metadata(&cur) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(FsError::Escape(format!(
                        "解压路径组件是符号链接: {}",
                        cur.display()
                    )));
                }
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(FsError::Conflict(format!(
                        "解压路径组件不是目录: {}",
                        cur.display()
                    )));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&cur).map_err(|e| io_error(&cur, e))?;
                    self.created.push((cur.clone(), true));
                }
                Err(e) => return Err(io_error(&cur, e)),
            }
        }
        Ok(cur)
    }

    /// 逆序清理本次新建的对象，返回**未能清理**的路径；调用方必须把它们
    /// 写进错误信息，否则用户会看到「报错但目录里还留着半成品」。
    fn cleanup(&mut self) -> Vec<PathBuf> {
        let mut leftovers = Vec::new();
        for (path, is_dir) in self.created.drain(..).rev() {
            let removed = if is_dir {
                fs::remove_dir(&path)
            } else {
                fs::remove_file(&path)
            };
            // 已经不存在的不算残留（可能是并发删除）；仍存在但删不掉才是。
            if removed.is_err() && path.exists() {
                leftovers.push(path);
            }
        }
        leftovers
    }
}

/// 未清理残留的可读列表：最多列 8 条，其余只报数量，避免错误信息被撑爆。
fn leftover_note(paths: &[PathBuf]) -> String {
    let mut list: Vec<String> = paths
        .iter()
        .take(8)
        .map(|p| p.display().to_string())
        .collect();
    if paths.len() > list.len() {
        list.push(format!("…另有 {} 个", paths.len() - list.len()));
    }
    format!(
        "注意：{} 个已解出对象未能自动清理，请手动检查：{}",
        paths.len(),
        list.join("、")
    )
}

fn open_extract_target(path: &Path, create_new: bool) -> Result<File, FsError> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if create_new {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    {
        fsio::ensure_not_symlink(path)?;
    }
    opts.open(path).map_err(|e| io_error(path, e))
}

/// 解压条目相对路径校验：拒绝绝对路径、`..`、反斜杠与非 UTF-8。
fn safe_rel(raw: &str) -> Result<PathBuf, FsError> {
    if raw.is_empty() {
        return Err(FsError::Denied("压缩包条目路径为空".into()));
    }
    if raw.contains('\0') {
        return Err(FsError::Denied("压缩包条目路径含 NUL".into()));
    }
    if raw.contains('\\') {
        return Err(FsError::Denied(format!(
            "压缩包条目路径含反斜杠，拒绝解出: {raw}"
        )));
    }
    let p = Path::new(raw);
    if p.is_absolute() {
        return Err(FsError::Denied(format!(
            "压缩包条目使用绝对路径，拒绝解出: {raw}"
        )));
    }
    let bytes = raw.as_bytes();
    if raw.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return Err(FsError::Denied(format!(
            "压缩包条目使用绝对路径，拒绝解出: {raw}"
        )));
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(name) => {
                if name.to_str().is_none() {
                    return Err(FsError::Denied(format!(
                        "压缩包条目路径含非 UTF-8 组件: {raw}"
                    )));
                }
                out.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::Escape(format!(
                    "压缩包条目含 .. 穿越，拒绝解出: {raw}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::Denied(format!(
                    "压缩包条目使用绝对路径，拒绝解出: {raw}"
                )));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(FsError::Denied(format!("压缩包条目路径归一后为空: {raw}")));
    }
    Ok(out)
}

fn tmp_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_rejects_traversal_and_absolute() {
        assert!(safe_rel("../etc/passwd").is_err());
        assert!(safe_rel("a/../../b").is_err());
        assert!(safe_rel("/etc/passwd").is_err());
        assert!(safe_rel("C:/Windows/x").is_err());
        assert!(safe_rel("a\\b").is_err());
        assert!(safe_rel("./a/b").is_ok());
        assert_eq!(safe_rel("a/b").unwrap(), PathBuf::from("a").join("b"));
    }

    /// 回滚失败时 `cleanup` 必须把仍存在的路径交回调用方，而不是静默吞掉。
    #[test]
    fn cleanup_reports_paths_it_cannot_remove() {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path().join("busy");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("child.txt"), b"x").unwrap();

        let limits = ExtractLimits {
            max_entries: 1,
            max_entry_bytes: 1,
            max_total_bytes: 1,
        };
        let mut st = ExtractState::new(td.path().to_path_buf(), limits, false);
        st.created.push((dir.clone(), true));

        let leftovers = st.cleanup();
        assert_eq!(leftovers, vec![dir]);
        assert!(leftover_note(&leftovers).contains("未能自动清理"));
    }
}
