//! 镜像同步：从主站按内容寻址补齐缺失 blob，再更新清单并做严格自检。
//!
//! 主站可以是 HTTPS 基地址，也可以是本地产物目录（演练/离线搬运）。
//! 同步只增不删：旧 blob 会保留（内容寻址，旧清单仍可能需要），清理由运维负责。
//!
//! 步骤固定为六步，任一步失败都在错误里带步骤号与上下文：
//! 拉取清单 → 校验主站清单（验签/摘要/seq 对齐）→ 读取镜像状态并检查 seq →
//! 下载缺失 blob → 写入索引/清单/schema → 同步后自检（验签/摘要/seq 不回退）。

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::build::{read_dir_sorted, sha256_hex, SCHEMA_FILES};
use crate::check;
use crate::model::{blob_url, IndexFile, ManifestFile, SignatureFile};
use crate::schema::SchemaSet;
use crate::sign::{key_id_of_hex, manifest_signing_input, verify_detached};

/// 单个对象上限：升级包/插件包远小于此值，超出视为异常响应。
pub const MAX_OBJECT_BYTES: u64 = 256 * 1024 * 1024;

/// 主站数据源（HTTP 或本地目录）。
pub trait Source {
    fn fetch(&self, relative: &str) -> Result<Vec<u8>>;
}

pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Source for DirSource {
    fn fetch(&self, relative: &str) -> Result<Vec<u8>> {
        let path = self.root.join(relative);
        fs::read(&path).with_context(|| format!("读取主站文件失败: {}", path.display()))
    }
}

pub struct HttpSource {
    base: String,
    agent: ureq::Agent,
}

impl HttpSource {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(120))
                .build(),
        }
    }
}

impl Source for HttpSource {
    fn fetch(&self, relative: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}", self.base, relative);
        let response = self
            .agent
            .get(&url)
            .call()
            .with_context(|| format!("GET {url} 失败"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_OBJECT_BYTES + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("读取响应失败: {url}"))?;
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            bail!("{url} 超过单对象上限 {MAX_OBJECT_BYTES} 字节");
        }
        Ok(bytes)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SyncReport {
    pub blobs_total: usize,
    pub blobs_reused: usize,
    pub blobs_fetched: usize,
    pub bytes_fetched: u64,
    /// 清单是否变化（同一 `seq` 且字节一致时为 false）。
    pub manifest_written: bool,
}

/// 执行一次单渠道同步。
pub fn run(
    source: &dyn Source,
    channel: &str,
    mirror: &Path,
    public_key: &str,
) -> Result<SyncReport> {
    let channel_prefix = format!("channels/{channel}");

    // 步骤 1/6：拉取主站清单、签名与索引。
    let manifest_bytes = source
        .fetch(&format!("{channel_prefix}/manifest.json"))
        .context("步骤 1/6 拉取主站 manifest 失败")?;
    let sig_bytes = source
        .fetch(&format!("{channel_prefix}/manifest.json.sig"))
        .context("步骤 1/6 拉取主站 manifest.json.sig 失败")?;
    let index_bytes = source
        .fetch(&format!("{channel_prefix}/index.json"))
        .context("步骤 1/6 拉取主站 index.json 失败")?;

    // 步骤 2/6：主站清单必须能通过固定公钥验签，且索引摘要/seq/channel 对齐。
    let (manifest, index) = validate_source(
        channel,
        &manifest_bytes,
        &sig_bytes,
        &index_bytes,
        public_key,
    )
    .context("步骤 2/6 校验主站清单失败")?;

    // 步骤 3/6：读取镜像现有状态，拒绝 seq 回退与「同 seq 不同内容」。
    let existing_manifest = mirror.join(&channel_prefix).join("manifest.json");
    let existing_bytes = existing_manifest
        .is_file()
        .then(|| fs::read(&existing_manifest))
        .transpose()
        .with_context(|| {
            format!(
                "读取镜像现有 manifest 失败: {}",
                existing_manifest.display()
            )
        })?;
    let previous_seq = match existing_bytes.as_deref() {
        Some(bytes) => {
            let previous: ManifestFile = serde_json::from_slice(bytes).with_context(|| {
                format!(
                    "解析镜像现有 manifest 失败: {}",
                    existing_manifest.display()
                )
            })?;
            if manifest.seq < previous.seq {
                bail!(
                    "步骤 3/6 seq 回退被拒绝：主站 {}，镜像 {}",
                    manifest.seq,
                    previous.seq
                );
            }
            if manifest.seq == previous.seq && bytes != manifest_bytes.as_slice() {
                bail!(
                    "步骤 3/6 同一 seq（{}）的清单内容不同：必须递增源数据 seq 后再发布",
                    manifest.seq
                );
            }
            Some(previous.seq)
        }
        None => None,
    };
    let manifest_written = existing_bytes.as_deref() != Some(manifest_bytes.as_slice());

    // 步骤 4/6：按内容寻址补齐缺失 blob。
    let mut report = SyncReport::default();
    for (sha, size) in referenced_blobs(channel, &index)? {
        report.blobs_total += 1;
        if blob_matches(mirror, &sha, size)? {
            report.blobs_reused += 1;
            continue;
        }
        let relative = blob_url(&sha);
        let bytes = source
            .fetch(&relative)
            .with_context(|| format!("步骤 4/6 下载 blob {sha} 失败"))?;
        if bytes.len() as u64 != size {
            bail!(
                "步骤 4/6 blob {sha} 大小不符：声明 {size}，主站 {}",
                bytes.len()
            );
        }
        let actual = sha256_hex(&bytes);
        if actual != sha {
            bail!("步骤 4/6 blob 摘要不符：声明 {sha}，主站 {actual}");
        }
        write_atomic(mirror, &relative, &bytes)
            .with_context(|| format!("步骤 4/6 写入 blob {sha} 失败"))?;
        report.blobs_fetched += 1;
        report.bytes_fetched += size;
    }

    // 步骤 5/6：blob 齐了再更新 schema / 索引 / 清单 / 签名（清单最后，减少半更新窗口）。
    for schema_file in SCHEMA_FILES {
        let relative = format!("schema/{schema_file}");
        let bytes = source
            .fetch(&relative)
            .with_context(|| format!("步骤 5/6 拉取 {relative} 失败"))?;
        write_atomic(mirror, &relative, &bytes)
            .with_context(|| format!("步骤 5/6 写入 {relative} 失败"))?;
    }
    write_atomic(
        mirror,
        &format!("{channel_prefix}/index.json"),
        &index_bytes,
    )
    .context("步骤 5/6 写入 index.json 失败")?;
    write_atomic(
        mirror,
        &format!("{channel_prefix}/manifest.json"),
        &manifest_bytes,
    )
    .context("步骤 5/6 写入 manifest.json 失败")?;
    write_atomic(
        mirror,
        &format!("{channel_prefix}/manifest.json.sig"),
        &sig_bytes,
    )
    .context("步骤 5/6 写入 manifest.json.sig 失败")?;
    report.manifest_written = manifest_written;

    // 步骤 6/6：同步后自检。只看本渠道 + schema + blob，镜像里其它渠道/旧 blob 不参与。
    self_check(mirror, channel, public_key, previous_seq).context("步骤 6/6 同步后自检失败")?;

    Ok(report)
}

fn validate_source(
    channel: &str,
    manifest_bytes: &[u8],
    sig_bytes: &[u8],
    index_bytes: &[u8],
    public_key: &str,
) -> Result<(ManifestFile, IndexFile)> {
    let manifest: ManifestFile =
        serde_json::from_slice(manifest_bytes).context("manifest 解析失败")?;
    manifest.validate()?;
    if manifest.channel != channel {
        bail!(
            "manifest.channel={} 与目标渠道 {channel} 不一致",
            manifest.channel
        );
    }

    let signature: SignatureFile = serde_json::from_slice(sig_bytes).context("签名解析失败")?;
    signature.validate()?;
    if signature.key_id != manifest.key_id {
        bail!("manifest.key_id 与签名 key_id 不一致");
    }
    let expected = key_id_of_hex(public_key).context("固定公钥不合法")?;
    if manifest.key_id != expected {
        bail!("manifest.key_id 与固定公钥不一致");
    }
    let sig = hex::decode(&signature.sig).context("签名不是十六进制")?;
    verify_detached(public_key, &manifest_signing_input(manifest_bytes), &sig)
        .context("manifest 验签失败")?;

    if sha256_hex(index_bytes) != manifest.index.sha256 {
        bail!("index 摘要与 manifest 不一致");
    }
    if index_bytes.len() as u64 != manifest.index.size {
        bail!("index 大小与 manifest 不一致");
    }
    let index: IndexFile = serde_json::from_slice(index_bytes).context("index 解析失败")?;
    index.validate()?;
    if index.channel != channel {
        bail!(
            "index.channel={} 与目标渠道 {channel} 不一致",
            index.channel
        );
    }
    if index.seq != manifest.seq {
        bail!("index.seq 与 manifest.seq 不一致");
    }
    Ok((manifest, index))
}

/// 索引里引用的全部 blob（去重，保留声明大小）。
fn referenced_blobs(channel: &str, index: &IndexFile) -> Result<Vec<(String, u64)>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut push =
        |sha: Option<&str>, url: Option<&str>, size: Option<u64>, label: &str| -> Result<()> {
            let sha = sha.ok_or_else(|| anyhow::anyhow!("{channel} 的{label}缺少内容摘要"))?;
            let url = url.ok_or_else(|| anyhow::anyhow!("{channel} 的{label}缺少内容地址"))?;
            let size = size.ok_or_else(|| anyhow::anyhow!("{channel} 的{label}缺少字节数"))?;
            if url != blob_url(sha) {
                bail!("{channel} 的{label}内容地址与摘要不一致: {url}");
            }
            if seen.insert(sha.to_string()) {
                out.push((sha.to_string(), size));
            }
            Ok(())
        };
    for entry in &index.plugins {
        push(
            entry.pkg_sha256.as_deref(),
            entry.pkg_url.as_deref(),
            entry.size,
            &format!("插件 {} ", entry.id),
        )?;
    }
    for entry in &index.apps {
        push(
            entry.compose_sha256.as_deref(),
            entry.compose_url.as_deref(),
            entry.size,
            &format!("应用 {} ", entry.id),
        )?;
    }
    for entry in &index.upgrades {
        push(
            entry.pkg_sha256.as_deref(),
            entry.pkg_url.as_deref(),
            entry.size,
            &format!("升级条目 {} ", entry.version),
        )?;
    }
    Ok(out)
}

/// 镜像里的 blob 是否已存在且大小、sha256 都正确。
fn blob_matches(mirror: &Path, sha: &str, size: u64) -> Result<bool> {
    let path = mirror.join("blobs").join("sha256").join(sha);
    if !path.is_file() {
        return Ok(false);
    }
    let meta = fs::metadata(&path)
        .with_context(|| format!("读取镜像 blob 元数据失败: {}", path.display()))?;
    if meta.len() != size {
        return Ok(false);
    }
    let bytes =
        fs::read(&path).with_context(|| format!("读取镜像 blob 失败: {}", path.display()))?;
    Ok(sha256_hex(&bytes) == sha)
}

/// 同内容不重写；否则写 `.part` 后 rename，避免镜像对外暴露半个文件。
fn write_atomic(root: &Path, relative: &str, bytes: &[u8]) -> Result<bool> {
    let path = root.join(relative);
    if path.is_file() && fs::read(&path).ok().as_deref() == Some(bytes) {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("目标路径没有父目录: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("创建目录失败: {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("目标路径没有合法文件名: {}", path.display()))?;
    let part = parent.join(format!("{name}.part"));
    fs::write(&part, bytes).with_context(|| format!("写入临时文件失败: {}", part.display()))?;
    fs::rename(&part, &path).with_context(|| {
        format!(
            "重命名临时文件失败: {} → {}",
            part.display(),
            path.display()
        )
    })?;
    Ok(true)
}

/// 自检只看本次同步的渠道：镜像里的旧 blob 允许保留，其它渠道不参与本次判定。
fn self_check(
    mirror: &Path,
    channel: &str,
    public_key: &str,
    previous_seq: Option<u64>,
) -> Result<()> {
    let all = check::read_tree(mirror)?;
    let prefix = format!("channels/{channel}/");
    let files: std::collections::BTreeMap<String, Vec<u8>> = all
        .into_iter()
        .filter(|(path, _)| {
            path.starts_with(&prefix)
                || path.starts_with("schema/")
                || path.starts_with("blobs/sha256/")
        })
        .collect();
    let schemas = SchemaSet::new(mirror.join("schema"));
    let summary = check::validate_artifact_with(&files, &schemas, true, Some(public_key), true)?;

    let manifest_path = format!("{prefix}manifest.json");
    let manifest: ManifestFile = serde_json::from_slice(
        files
            .get(&manifest_path)
            .ok_or_else(|| anyhow::anyhow!("自检时缺少 {manifest_path}"))?,
    )
    .context("自检解析 manifest 失败")?;
    if let Some(previous) = previous_seq {
        if manifest.seq < previous {
            bail!(
                "自检发现 seq 回退：同步后 {}，同步前 {previous}",
                manifest.seq
            );
        }
    }
    println!(
        "✓ 镜像自检通过：渠道 {channel} / {} 插件 / {} 应用 / {} 升级条目 / {} 个 blob",
        summary.plugins, summary.apps, summary.upgrades, summary.blobs
    );
    Ok(())
}

/// 把本地目录整体复制成主站演练源（测试与离线搬运用）。
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.is_dir() {
        bail!("源目录不存在或不是目录: {}", from.display());
    }
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((src, dst)) = stack.pop() {
        fs::create_dir_all(&dst).with_context(|| format!("创建目录失败: {}", dst.display()))?;
        for path in read_dir_sorted(&src)? {
            let name = path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("路径没有文件名: {}", path.display()))?;
            let target = dst.join(name);
            if path.is_dir() {
                stack.push((path, target));
            } else if path.is_file() {
                fs::copy(&path, &target).with_context(|| {
                    format!("复制失败: {} → {}", path.display(), target.display())
                })?;
            }
        }
    }
    Ok(())
}
