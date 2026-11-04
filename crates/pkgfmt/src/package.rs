//! `.wbpkg` 编解码：两遍安全解包 + 哈希清单 + 签名（§4.1、§4.3）。
//!
//! 第一遍**只解析不释放**：校验路径规则、条目类型、哈希与签名，并统计压缩包大小、
//! 解压总量、条目数；全部通过后第二遍才解到 staging 目录（root 所有、排他新建、
//! 拒绝链接与特殊文件）。任何一步失败都清理 staging，不留下“部分安装成功”。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::manifest::Manifest;
use crate::signature::{key_id_of, signing_input, verify_bytes, SigningKey};
use crate::{PkgError, Result};

#[derive(Debug, Clone, Copy)]
pub struct PackageLimits {
    pub archive_bytes: u64,
    pub total_bytes: u64,
    pub entries: usize,
    pub manifest_bytes: u64,
}

pub const DEFAULT_LIMITS: PackageLimits = PackageLimits {
    archive_bytes: 100 * 1024 * 1024,
    total_bytes: 512 * 1024 * 1024,
    entries: 4096,
    manifest_bytes: 1024 * 1024,
};

/// 包内架构目录名：`amd64` / `arm64`（与 Docker 官方命名一致）。
pub fn host_arch() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

pub const MANIFEST_NAME: &str = "manifest.toml";
pub const FILES_NAME: &str = "files.sha256";
pub const SIG_NAME: &str = "sig";

/// 进包的三个载荷目录。**其余一律不进包**——尤其是 `target/`（上百 MB 构建产物）
/// 和 `skiff-signing.key`（签名私钥）。整目录打包曾把私钥塞进分发包，
/// 等于把「谁都能出同源包」的那把钥匙公开。
pub const PAYLOAD_DIRS: &[&str] = &["bin", "ui", "migrations"];

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct PackageMeta {
    pub manifest: Manifest,
    pub manifest_raw: Vec<u8>,
    pub files_list_raw: Vec<u8>,
    pub files: Vec<FileEntry>,
    /// 包内声明的公钥指纹（manifest.key_id）
    pub key_id: String,
    /// 实际参与验签的公钥指纹；未导入公钥时等于 key_id（仅展示用）
    pub public_key_hex: Option<String>,
    pub signature_verified: bool,
    pub payload_bytes: u64,
    pub package_bytes: u64,
}

impl PackageMeta {
    pub fn arch_binary(&self, arch: &str) -> Option<&FileEntry> {
        let want = format!("bin/{arch}/plugin");
        self.files.iter().find(|f| f.path == want)
    }
}

#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// 已导入信任库的公钥（TOFU 首次信任由上层导入后传入）
    pub trusted_public_key: Option<String>,
    /// 开发配置项：仅 `allow_unsigned = true` 时允许未签名/未验签安装
    pub allow_unsigned: bool,
    pub limits: PackageLimits,
    /// 目标架构：`amd64` / `arm64`
    pub arch: String,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            trusted_public_key: None,
            allow_unsigned: false,
            limits: DEFAULT_LIMITS,
            arch: host_arch(),
        }
    }
}

#[derive(Debug)]
pub struct UnpackedPackage {
    pub meta: PackageMeta,
    pub staging: PathBuf,
}

impl Drop for UnpackedPackage {
    fn drop(&mut self) {
        // staging 由调用方通过 `persist` 认领；未认领则清理，避免残留“部分安装”。
        // Windows 上杀毒/索引会短暂占用刚解包的文件，`remove_dir_all` 可能返回
        // `ERROR_DIR_NOT_EMPTY`（不是真有残留）；重试几次即可，Linux 首次即成功。
        for attempt in 0..5 {
            if !self.staging.exists() || fs::remove_dir_all(&self.staging).is_ok() {
                return;
            }
            if attempt < 4 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

impl UnpackedPackage {
    /// 认领 staging 目录（调用方负责原子 rename 到最终位置）。
    pub fn persist(mut self) -> (PackageMeta, PathBuf) {
        let path = std::mem::take(&mut self.staging);
        let meta = self.meta.clone();
        (meta, path)
    }
}

struct RawEntry {
    path: String,
    size: u64,
    sha256: String,
    is_files: bool,
    is_sig: bool,
}

fn is_within(dir: &Path, candidate: &Path) -> bool {
    let Ok(dir) = fs::canonicalize(dir) else {
        return false;
    };
    let Ok(cand) = fs::canonicalize(candidate) else {
        return false;
    };
    cand.starts_with(dir)
}

/// 归一化并校验包内路径：禁止绝对路径、`..`、`.`、空组件、反斜杠、NUL。
fn normalize_entry_path(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(PkgError::Format("包内存在空路径条目".into()));
    }
    if raw.len() > 1024 {
        return Err(PkgError::Format(format!("包内路径过长: {raw}")));
    }
    if raw.contains('\0') || raw.contains('\\') {
        return Err(PkgError::Format(format!("包内路径含非法字符: {raw}")));
    }
    if skiff_model::is_posix_absolute(raw) {
        return Err(PkgError::Format(format!("包内路径不得是绝对路径: {raw}")));
    }
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(PkgError::Format("包内存在空路径条目".into()));
    }
    let mut parts = Vec::new();
    for seg in trimmed.split('/') {
        if seg.is_empty() {
            return Err(PkgError::Format(format!("包内路径含空组件: {raw}")));
        }
        if seg == "." || seg == ".." {
            return Err(PkgError::Format(format!("包内路径含 . 或 ..: {raw}")));
        }
        parts.push(seg);
    }
    Ok(parts.join("/"))
}

/// 同时写文件并计算哈希，避免「校验一份、落盘另一份」的两遍不一致。
struct TeeWriter<'a> {
    inner: &'a mut File,
    hasher: Sha256,
    written: u64,
}

impl<'a> TeeWriter<'a> {
    fn new(inner: &'a mut File) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            written: 0,
        }
    }
    fn finish(self) -> (String, u64) {
        (hex::encode(self.hasher.finalize()), self.written)
    }
}

impl Write for TeeWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct HashWriter {
    hasher: Sha256,
    written: u64,
}

impl HashWriter {
    fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            written: 0,
        }
    }
    fn finish(self) -> (String, u64) {
        (hex::encode(self.hasher.finalize()), self.written)
    }
}

impl Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(buf);
        self.written += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn open_archive(
    path: &Path,
) -> Result<tar::Archive<zstd::stream::read::Decoder<'static, BufReader<File>>>> {
    let file = File::open(path).map_err(PkgError::Io)?;
    // zstd 的 Decoder::new 内部已经套了一层 BufReader，这里直接传 File，
    // 否则类型变成 Decoder<BufReader<BufReader<File>>>（纯浪费 + 类型对不上）。
    let decoder =
        zstd::stream::read::Decoder::new(file).map_err(|e| PkgError::Zstd(e.to_string()))?;
    Ok(tar::Archive::new(decoder))
}

/// 第一遍：只解析、不释放。
fn scan(path: &Path, opts: &VerifyOptions) -> Result<(PackageMeta, Vec<RawEntry>)> {
    let meta = fs::metadata(path).map_err(PkgError::Io)?;
    if meta.len() > opts.limits.archive_bytes {
        return Err(PkgError::Format(format!(
            "压缩包超过上限 {} 字节（实际 {}）",
            opts.limits.archive_bytes,
            meta.len()
        )));
    }

    let mut archive = open_archive(path)?;
    let mut raw: Vec<RawEntry> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total: u64 = 0;
    let mut manifest: Option<Vec<u8>> = None;
    let mut files_list: Option<Vec<u8>> = None;
    let mut sig: Option<Vec<u8>> = None;

    let entries = archive
        .entries()
        .map_err(|e| PkgError::Tar(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| PkgError::Tar(e.to_string()))?;
        if raw.len() + 1 > opts.limits.entries {
            return Err(PkgError::Format(format!(
                "条目数超过上限 {}",
                opts.limits.entries
            )));
        }
        let raw_path = String::from_utf8(entry.path_bytes().to_vec())
            .map_err(|_| PkgError::Format("包内路径不是合法 UTF-8".into()))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            // 目录不单独记哈希；只校验路径规则
            normalize_entry_path(raw_path.trim_end_matches('/'))?;
            continue;
        }
        if !kind.is_file() {
            return Err(PkgError::Unsupported(format!(
                "只允许普通文件与目录，发现 {:?}: {raw_path}",
                kind
            )));
        }
        let path_norm = normalize_entry_path(&raw_path)?;
        if !seen.insert(path_norm.clone()) {
            return Err(PkgError::Format(format!("包内存在重复路径: {path_norm}")));
        }

        let size = entry.size();
        total = total.saturating_add(size);
        if total > opts.limits.total_bytes {
            return Err(PkgError::Format(format!(
                "解压总量超过上限 {} 字节",
                opts.limits.total_bytes
            )));
        }

        let is_manifest = path_norm == MANIFEST_NAME;
        let is_files = path_norm == FILES_NAME;
        let is_sig = path_norm == SIG_NAME;

        if is_manifest && size > opts.limits.manifest_bytes {
            return Err(PkgError::Format(format!(
                "manifest 超过 {} 字节上限",
                opts.limits.manifest_bytes
            )));
        }

        if is_manifest || is_files || is_sig {
            // 小文件直接读入内存（受 manifest_bytes / 默认上限约束）
            let cap = if is_manifest {
                opts.limits.manifest_bytes
            } else {
                64 * 1024
            };
            if size > cap {
                return Err(PkgError::Format(format!("{path_norm} 体积异常: {size}")));
            }
            let mut buf = Vec::with_capacity(size as usize);
            entry.read_to_end(&mut buf).map_err(PkgError::Io)?;
            match (is_manifest, is_files, is_sig) {
                (true, _, _) => manifest = Some(buf.clone()),
                (_, true, _) => files_list = Some(buf.clone()),
                (_, _, true) => sig = Some(buf.clone()),
                _ => unreachable!(),
            }
            let mut h = Sha256::new();
            h.update(&buf);
            raw.push(RawEntry {
                path: path_norm,
                size,
                sha256: hex::encode(h.finalize()),
                is_files,
                is_sig,
            });
        } else {
            let mut hw = HashWriter::new();
            std::io::copy(&mut entry, &mut hw).map_err(PkgError::Io)?;
            let (sha256, written) = hw.finish();
            if written != size {
                return Err(PkgError::Format(format!(
                    "条目 {path_norm} 声明大小 {size} 与实际 {written} 不一致"
                )));
            }
            raw.push(RawEntry {
                path: path_norm,
                size,
                sha256,
                is_files,
                is_sig,
            });
        }
    }

    let manifest_raw = manifest.ok_or_else(|| PkgError::Format("缺少 manifest.toml".into()))?;
    let files_list_raw = files_list.ok_or_else(|| PkgError::Format("缺少 files.sha256".into()))?;
    let manifest_text = std::str::from_utf8(&manifest_raw)
        .map_err(|_| PkgError::Manifest("manifest.toml 不是 UTF-8".into()))?;
    let manifest = Manifest::parse(manifest_text)?;
    // 菜单/槽位引用的 ui 文件必须真的打进包；否则安装后是死链。
    manifest.validate_menu_assets(|p| raw.iter().any(|e| e.path == format!("ui/{p}")))?;

    if let Some(bin) = raw
        .iter()
        .find(|e| e.path == format!("bin/{}/plugin", opts.arch))
    {
        if bin.size == 0 {
            return Err(PkgError::Format(format!(
                "bin/{}/plugin 为空文件，拒绝安装",
                opts.arch
            )));
        }
    } else {
        return Err(PkgError::Unsupported(format!(
            "包内缺少 bin/{}/plugin（多架构是发布者责任，该架构就拒装，不静默降级）",
            opts.arch
        )));
    }

    // ---- 验签 ----
    let (signature_verified, public_key_hex) = match (&opts.trusted_public_key, &sig) {
        (Some(pk), Some(sig_bytes)) => {
            let expected = key_id_of(
                &hex::decode(pk.trim())
                    .map_err(|_| PkgError::Signature("信任库公钥不是 hex".into()))?,
            );
            if expected != manifest.key_id {
                return Err(PkgError::Signature(format!(
                    "包声明 key_id {} 与信任公钥指纹 {expected} 不一致（换密钥视为新信任，需重新确认指纹）",
                    manifest.key_id
                )));
            }
            verify_bytes(
                pk,
                &signing_input(&manifest_raw, &files_list_raw),
                sig_bytes,
            )?;
            (true, Some(pk.clone()))
        }
        (Some(pk), None) => {
            if opts.allow_unsigned {
                (false, Some(pk.clone()))
            } else {
                return Err(PkgError::Signature("缺少 sig（未签名包默认拒装）".into()));
            }
        }
        (None, _) => {
            if !opts.allow_unsigned {
                return Err(PkgError::Signature(
                    "未导入该包的签名公钥：请带外核对指纹后导入，再安装（TOFU 首次信任）".into(),
                ));
            }
            (false, None)
        }
    };

    // ---- files.sha256：覆盖范围、排序、哈希一致性 ----
    let files_list_text = std::str::from_utf8(&files_list_raw)
        .map_err(|_| PkgError::Hash("files.sha256 不是 UTF-8".into()))?;
    let mut listed: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for (i, line) in files_list_text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let Some((hash, path)) = line.split_once("  ") else {
            return Err(PkgError::Hash(format!(
                "files.sha256 第 {} 行格式错误（应为 <64hex><两个空格><路径>）",
                i + 1
            )));
        };
        if hash.len() != 64
            || !hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(PkgError::Hash(format!(
                "files.sha256 第 {} 行哈希非法",
                i + 1
            )));
        }
        let path = normalize_entry_path(path)?;
        if path == FILES_NAME || path == SIG_NAME {
            return Err(PkgError::Hash(format!(
                "files.sha256 不得包含自身或 sig: {path}"
            )));
        }
        if let Some(prev) = &last {
            if prev.as_bytes() >= path.as_bytes() {
                return Err(PkgError::Hash(format!(
                    "files.sha256 必须按路径字节升序且不可重复: {path}"
                )));
            }
        }
        last = Some(path.clone());
        listed.insert(path, hash.to_string());
    }

    let payload: BTreeSet<String> = raw
        .iter()
        .filter(|e| !e.is_files && !e.is_sig)
        .map(|e| e.path.clone())
        .collect();
    let listed_set: BTreeSet<String> = listed.keys().cloned().collect();
    if payload != listed_set {
        let missing: Vec<_> = payload.difference(&listed_set).cloned().collect();
        let extra: Vec<_> = listed_set.difference(&payload).cloned().collect();
        return Err(PkgError::Hash(format!(
            "哈希清单与包内容不一致（未列: {missing:?}；多列: {extra:?}）"
        )));
    }
    for e in &raw {
        if e.is_files || e.is_sig {
            continue;
        }
        if listed.get(&e.path) != Some(&e.sha256) {
            return Err(PkgError::Hash(format!("文件哈希不匹配: {}", e.path)));
        }
    }

    // key_id 必须在 manifest 被移进结构体之前取出来
    let key_id = manifest.key_id.clone();
    Ok((
        PackageMeta {
            manifest,
            manifest_raw,
            files_list_raw,
            files: raw
                .iter()
                .filter(|e| !e.is_sig)
                .map(|e| FileEntry {
                    path: e.path.clone(),
                    size: e.size,
                    sha256: e.sha256.clone(),
                })
                .collect(),
            key_id,
            public_key_hex,
            signature_verified,
            payload_bytes: total,
            package_bytes: meta.len(),
        },
        raw,
    ))
}

/// 校验包（第一遍）。不写盘、不释放条目。
pub fn verify_file(path: &Path, opts: &VerifyOptions) -> Result<PackageMeta> {
    let (mut meta, _raw) = scan(path, opts)?;
    meta.key_id = meta.manifest.key_id.clone();
    Ok(meta)
}

/// 校验 + 两遍解包到 staging 目录（调用方负责最终原子 rename）。
pub fn verify_and_stage(
    path: &Path,
    staging_root: &Path,
    opts: &VerifyOptions,
) -> Result<UnpackedPackage> {
    let (mut meta, raw) = scan(path, opts)?;
    meta.key_id = meta.manifest.key_id.clone();

    let staging = staging_root.join(format!(
        ".staging-{}-{}",
        meta.manifest.id.replace('.', "_"),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&staging).map_err(PkgError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&staging, fs::Permissions::from_mode(0o700));
    }

    match extract_pass(path, &staging, &raw, opts) {
        Ok(()) => {}
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    }

    Ok(UnpackedPackage { meta, staging })
}

fn extract_pass(
    path: &Path,
    staging: &Path,
    expected: &[RawEntry],
    opts: &VerifyOptions,
) -> Result<()> {
    let mut archive = open_archive(path)?;
    let entries = archive
        .entries()
        .map_err(|e| PkgError::Tar(e.to_string()))?;
    let expected: BTreeMap<&str, &RawEntry> =
        expected.iter().map(|e| (e.path.as_str(), e)).collect();
    let mut written_total: u64 = 0;
    let mut extracted: BTreeSet<String> = BTreeSet::new();

    for entry in entries {
        let mut entry = entry.map_err(|e| PkgError::Tar(e.to_string()))?;
        let raw_path = String::from_utf8(entry.path_bytes().to_vec())
            .map_err(|_| PkgError::Format("包内路径不是合法 UTF-8".into()))?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let path_norm = normalize_entry_path(&raw_path)?;
        if path_norm == SIG_NAME || path_norm == FILES_NAME {
            // 签名与清单是元数据，不进版本目录
            continue;
        }
        // 目录创建：逐段创建，禁止 .. （normalize 已保证）
        let dest = staging.join(
            path_norm
                .split('/')
                .collect::<Vec<_>>()
                .iter()
                .collect::<PathBuf>(),
        );
        if !is_within(staging, dest.parent().unwrap_or(staging)) && dest.parent().is_some() {
            // 父目录可能尚未创建；用逐段拼装后的路径再判定
        }
        let Some(parent) = dest.parent() else {
            return Err(PkgError::Format(format!("路径无父目录: {path_norm}")));
        };
        fs::create_dir_all(parent).map_err(PkgError::Io)?;
        if !is_within(staging, parent) {
            return Err(PkgError::Format(format!(
                "解包路径逃逸 staging: {path_norm}"
            )));
        }

        let some = expected.get(path_norm.as_str());
        let Some(exp) = some else {
            return Err(PkgError::Format(format!(
                "第二遍发现第一遍未见的条目: {path_norm}"
            )));
        };
        if !extracted.insert(path_norm.clone()) {
            return Err(PkgError::Format(format!("重复条目: {path_norm}")));
        }

        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true) // 排他新建，禁止覆盖
            .open(&dest)
            .map_err(PkgError::Io)?;
        let (sha, n) = {
            let mut tee = TeeWriter::new(&mut f);
            let mut limited = (&mut entry).take(exp.size + 1);
            std::io::copy(&mut limited, &mut tee).map_err(PkgError::Io)?;
            tee.finish()
        };
        f.flush().map_err(PkgError::Io)?;
        drop(f);
        if n != exp.size {
            return Err(PkgError::Format(format!(
                "两遍之间内容变化或大小不符: {path_norm}（{n} != {}）",
                exp.size
            )));
        }
        if sha != exp.sha256 {
            return Err(PkgError::Hash(format!(
                "解包时哈希不匹配（疑似两遍之间被替换）: {path_norm}"
            )));
        }

        written_total = written_total.saturating_add(n);
        if written_total > opts.limits.total_bytes {
            return Err(PkgError::Format("解包总量超过上限".into()));
        }
    }

    // 事后完整性：所有预期条目都解出来了
    for exp in expected.values() {
        if exp.is_sig || exp.is_files {
            continue;
        }
        if !extracted.contains(&exp.path) {
            return Err(PkgError::Format(format!("第二遍缺少条目: {}", exp.path)));
        }
    }
    Ok(())
}

/// 打包目录为 `.wbpkg`：哈希清单 → 签名 → tar.zst（确定性输出）。
pub fn pack_dir(src: &Path, out: &Path, seed_hex: &str) -> Result<()> {
    let manifest_path = src.join(MANIFEST_NAME);
    let manifest_raw = fs::read(&manifest_path).map_err(PkgError::Io)?;
    let manifest_text = std::str::from_utf8(&manifest_raw)
        .map_err(|_| PkgError::Manifest("manifest.toml 不是 UTF-8".into()))?;
    let manifest = Manifest::parse(manifest_text)?;
    let key = SigningKey::from_seed_hex(seed_hex)?;
    if key.key_id() != manifest.key_id {
        return Err(PkgError::Signature(format!(
            "manifest.key_id（{}）与签名私钥对应公钥指纹（{}）不一致",
            manifest.key_id,
            key.key_id()
        )));
    }

    let mut files: Vec<(String, PathBuf, u64, String)> = Vec::new();
    // 只走白名单目录（见 PAYLOAD_DIRS 的注释：target/ 与私钥不能进包）
    for sub in PAYLOAD_DIRS {
        let dir = src.join(sub);
        if dir.is_dir() {
            collect_files(src, &dir, &mut files)?;
        }
    }
    if files.is_empty() {
        return Err(PkgError::Format(format!(
            "打包目录里没有 bin/ ui/ migrations/ 中的任何文件: {}",
            src.display()
        )));
    }
    // manifest 本身也要进哈希清单（它在 tar 里单独写，但校验时按清单核对）
    {
        let mut h = Sha256::new();
        h.update(&manifest_raw);
        files.push((
            MANIFEST_NAME.to_string(),
            manifest_path.clone(),
            manifest_raw.len() as u64,
            hex::encode(h.finalize()),
        ));
    }
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));

    let mut list = String::new();
    for (rel, _, size, hash) in &files {
        let _ = size;
        list.push_str(hash);
        list.push_str("  ");
        list.push_str(rel);
        list.push('\n');
    }
    let list_bytes = list.into_bytes();
    let sig = key.sign(&signing_input(&manifest_raw, &list_bytes));

    // 写 tar.zst（确定性：固定 mtime / mode，条目按路径排序）
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(PkgError::Io)?;
    }
    let file = File::create(out).map_err(PkgError::Io)?;
    let encoder = zstd::stream::write::Encoder::new(file, 19)
        .map_err(|e| PkgError::Zstd(e.to_string()))?
        .auto_finish();
    let mut builder = tar::Builder::new(encoder);

    append_bytes(&mut builder, MANIFEST_NAME, &manifest_raw)?;
    for (rel, abs, _, _) in &files {
        if rel == MANIFEST_NAME {
            continue;
        }
        let data = fs::read(abs).map_err(PkgError::Io)?;
        append_bytes(&mut builder, rel, &data)?;
    }
    append_bytes(&mut builder, FILES_NAME, &list_bytes)?;
    append_bytes(&mut builder, SIG_NAME, &sig)?;
    builder.finish().map_err(|e| PkgError::Tar(e.to_string()))?;
    Ok(())
}

fn append_bytes<W: Write>(builder: &mut tar::Builder<W>, name: &str, data: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, name, data)
        .map_err(|e| PkgError::Tar(e.to_string()))
}

fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf, u64, String)>,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(PkgError::Io)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(PkgError::Io)?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().map_err(PkgError::Io)?;
        if meta.is_dir() {
            collect_files(root, &path, out)?;
            continue;
        }
        if !meta.is_file() {
            return Err(PkgError::Unsupported(format!(
                "打包目录里只允许普通文件: {}",
                path.display()
            )));
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| PkgError::Format("路径不在打包根内".into()))?
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if rel == SIG_NAME || rel == FILES_NAME {
            continue; // 生成物，不参与哈希清单
        }
        let data = fs::read(&path).map_err(PkgError::Io)?;
        let mut h = Sha256::new();
        h.update(&data);
        out.push((rel, path.clone(), meta.len(), hex::encode(h.finalize())));
        let _ = name;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, data: &[u8]) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        let mut f = File::create(path).unwrap();
        f.write_all(data).unwrap();
    }

    struct Fixture {
        dir: tempfile::TempDir,
        pkg: PathBuf,
        public_hex: String,
        key_id: String,
    }

    fn build_pkg() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let key = SigningKey::generate();
        let manifest = format!(
            r#"id = "com.example.demo"
name = "Demo"
version = "0.1.0"
key_id = "{}"
api = "1.0"
requires = ["docker"]
idle_timeout_sec = 300

[[permissions]]
capability = "docker"
verbs = ["containers.list"]
"#,
            key.key_id()
        );
        write(&src.join("manifest.toml"), manifest.as_bytes());
        write(&src.join("bin/amd64/plugin"), b"#!/bin/sh\necho demo\n");
        write(&src.join("ui/index.html"), b"<html>demo</html>");
        write(&src.join("migrations/001.sql"), b"SELECT 1;\n");
        let pkg = dir.path().join("demo-0.1.0.wbpkg");
        pack_dir(&src, &pkg, &key.seed_hex()).unwrap();
        Fixture {
            dir,
            pkg,
            public_hex: key.public_hex(),
            key_id: key.key_id(),
        }
    }

    fn opts(f: &Fixture) -> VerifyOptions {
        VerifyOptions {
            trusted_public_key: Some(f.public_hex.clone()),
            allow_unsigned: false,
            limits: DEFAULT_LIMITS,
            arch: "amd64".into(),
        }
    }

    /// 打包只收 bin/ ui/ migrations/：`target/` 与签名私钥都不能进分发包。
    ///
    /// 这条是回归用——整目录打包时，2.9 MB 的私钥和上百 MB 的 target/ 会一起被装进
    /// .wbpkg，前者是直接把签名密钥发给所有人，后者会让包大到装不进去。
    #[test]
    fn pack_excludes_build_dir_and_signing_key() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let key = SigningKey::generate();
        let manifest = format!(
            r#"id = "com.example.demo"
name = "Demo"
version = "0.1.0"
key_id = "{}"
api = "1.0"
requires = ["docker"]
"#,
            key.key_id()
        );
        write(&src.join("manifest.toml"), manifest.as_bytes());
        write(
            &src.join("bin/amd64/plugin"),
            b"#!/bin/sh
echo demo
",
        );
        write(&src.join("ui/index.html"), b"<html>demo</html>");
        write(&src.join("skiff-signing.key"), b"deadbeef\n");
        write(&src.join("Cargo.toml"), b"[package]\nname = \"demo\"\n");
        write(&src.join("target/release/plugin"), b"junk-junk-junk");
        write(&src.join("src/main.rs"), b"fn main() {}");

        let pkg = dir.path().join("demo.wbpkg");
        pack_dir(&src, &pkg, &key.seed_hex()).unwrap();

        let f = File::open(&pkg).unwrap();
        let dec = zstd::stream::read::Decoder::new(f).unwrap();
        let mut ar = tar::Archive::new(dec);
        let mut names = Vec::new();
        for e in ar.entries().unwrap() {
            let e = e.unwrap();
            let p = String::from_utf8(e.path_bytes().to_vec()).unwrap();
            if !e.header().entry_type().is_dir() {
                names.push(p);
            }
        }
        assert!(names.iter().any(|n| n == "bin/amd64/plugin"), "{names:?}");
        assert!(names.iter().any(|n| n == "ui/index.html"), "{names:?}");
        for bad in [
            "skiff-signing.key",
            "Cargo.toml",
            "target/release/plugin",
            "src/main.rs",
        ] {
            assert!(!names.iter().any(|n| n == bad), "{bad} 不该进包: {names:?}");
        }
    }

    #[test]
    fn pack_then_verify_roundtrip() {
        let f = build_pkg();
        let meta = verify_file(&f.pkg, &opts(&f)).unwrap();
        assert_eq!(meta.manifest.id, "com.example.demo");
        assert!(meta.signature_verified);
        assert_eq!(meta.key_id, f.key_id);
        assert!(meta.arch_binary("amd64").is_some());
        assert_eq!(meta.files.len(), 5); // manifest/bin/ui/migration/files.sha256
    }

    #[test]
    fn unsigned_rejected_by_default() {
        let f = build_pkg();
        let mut o = opts(&f);
        o.trusted_public_key = None;
        let e = verify_file(&f.pkg, &o).unwrap_err().to_string();
        assert!(e.contains("未导入"), "{e}");

        o.allow_unsigned = true;
        let meta = verify_file(&f.pkg, &o).unwrap();
        assert!(!meta.signature_verified);
    }

    #[test]
    fn wrong_key_rejected() {
        let f = build_pkg();
        let other = SigningKey::generate();
        let mut o = opts(&f);
        o.trusted_public_key = Some(other.public_hex());
        let e = verify_file(&f.pkg, &o).unwrap_err().to_string();
        assert!(e.contains("key_id"), "{e}");
    }

    #[test]
    fn tampered_payload_rejected() {
        let f = build_pkg();
        // 直接用 tar 改内容而不重算哈希：解包前就该被 files.sha256 拦住
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.wbpkg");
        let mut out_entries: Vec<(String, Vec<u8>)> = Vec::new();
        let file = File::open(&f.pkg).unwrap();
        let dec = zstd::stream::read::Decoder::new(BufReader::new(file)).unwrap();
        let mut ar = tar::Archive::new(dec);
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let p = String::from_utf8(e.path_bytes().to_vec()).unwrap();
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).unwrap();
            if p == "ui/index.html" {
                buf = b"<html>evil</html>".to_vec();
            }
            out_entries.push((p, buf));
        }
        let out = File::create(&broken).unwrap();
        // 显式收尾：先结束 tar，再 finish zstd 帧。
        // 依赖 Drop 顺序（auto_finish + Builder::finish 返回值被丢弃）会让 zstd 帧尾
        // 有时来不及写完，读回来的报错就变成 "incomplete frame" 而不是我们要验的哈希不匹配。
        let mut enc = zstd::stream::write::Encoder::new(out, 19).unwrap();
        {
            let mut b = tar::Builder::new(&mut enc);
            for (p, data) in out_entries {
                append_bytes(&mut b, &p, &data).unwrap();
            }
            b.finish().unwrap();
        }
        enc.finish().unwrap();

        let e = verify_file(&broken, &opts(&f)).unwrap_err();
        assert!(matches!(e, PkgError::Hash(_)), "应被哈希清单拦截: {e}");
    }

    #[test]
    fn staging_extracts_files_and_cleans_on_drop() {
        let f = build_pkg();
        let stage_root = f.dir.path().join("stage");
        fs::create_dir_all(&stage_root).unwrap();
        let unpacked = verify_and_stage(&f.pkg, &stage_root, &opts(&f)).unwrap();
        let staging = unpacked.staging.clone();
        assert!(staging.join("manifest.toml").exists());
        assert!(staging.join("bin/amd64/plugin").exists());
        assert!(!staging.join(SIG_NAME).exists());
        drop(unpacked);
        assert!(!staging.exists(), "未认领的 staging 必须被清理");
    }

    #[test]
    fn missing_arch_binary_rejected() {
        let f = build_pkg();
        let mut o = opts(&f);
        o.arch = "arm64".into();
        let e = verify_file(&f.pkg, &o).unwrap_err().to_string();
        assert!(e.contains("bin/arm64/plugin"), "{e}");
    }

    #[test]
    fn path_traversal_in_list_rejected() {
        assert!(normalize_entry_path("../etc/passwd").is_err());
        assert!(normalize_entry_path("/etc/passwd").is_err());
        assert!(normalize_entry_path("a//b").is_err());
        assert!(normalize_entry_path("a/./b").is_err());
        assert!(normalize_entry_path("a\\b").is_err());
        assert_eq!(
            normalize_entry_path("ui/index.html").unwrap(),
            "ui/index.html"
        );
    }
}
