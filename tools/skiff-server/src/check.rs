//! 产物校验：schema、摘要、索引引用完整性与 `seq` 单调。
//!
//! `check` 不信任输出目录：所有判断都基于产物自身字节重新计算，不读取生成器
//! 的中间状态。未提供 `--dist` 时先在内存生成一遍再校验，等价于「源数据能否
//! 产出合法产物」。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::build::{build, read_dir_sorted, sha256_hex, SCHEMA_FILES};
use crate::model::{IndexFile, ManifestFile, SignatureFile};
use crate::schema::SchemaSet;
use crate::sign::{
    is_placeholder_key, key_id_of_hex, manifest_signing_input, verify_detached, Signer,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub channels: usize,
    pub plugins: usize,
    pub apps: usize,
    pub runtimes: usize,
    pub upgrades: usize,
    pub blobs: usize,
}

pub fn run(
    catalog_dir: &Path,
    schema_dir: &Path,
    dist: Option<&Path>,
    prev: Option<&Path>,
    require_signature: bool,
) -> Result<()> {
    run_with_public_key(catalog_dir, schema_dir, dist, prev, require_signature, None)
}

/// 与 `run` 相同，但额外用固定公钥做密码学验签。
///
/// 发布门禁必须走这一入口：只检查 `sig` 非空不能证明清单由持有私钥的一方签发。
pub fn run_with_public_key(
    catalog_dir: &Path,
    schema_dir: &Path,
    dist: Option<&Path>,
    prev: Option<&Path>,
    require_signature: bool,
    public_key: Option<&str>,
) -> Result<()> {
    let schemas = SchemaSet::new(schema_dir);
    let (files, summary) = match dist {
        Some(dist) => {
            let files = read_tree(dist)?;
            let summary =
                validate_artifact_with(&files, &schemas, require_signature, public_key, false)?;
            (files, summary)
        }
        None => {
            if public_key.is_some() {
                bail!("使用固定公钥验签时必须提供 --dist（内存生成的产物没有可验证的签名）");
            }
            let artifact = build(catalog_dir, schema_dir, &Signer::Unsigned)?;
            let summary =
                validate_artifact_with(&artifact.files, &schemas, require_signature, None, false)?;
            (artifact.files, summary)
        }
    };
    if let Some(prev) = prev {
        validate_seq_monotonic(&files, prev)?;
    }
    println!(
        "✓ 校验通过：{} 个渠道 / {} 插件 / {} 应用 / {} 运行环境 / {} 升级条目 / {} 个 blob",
        summary.channels,
        summary.plugins,
        summary.apps,
        summary.runtimes,
        summary.upgrades,
        summary.blobs
    );
    Ok(())
}

pub fn validate_artifact(
    files: &BTreeMap<String, Vec<u8>>,
    schemas: &SchemaSet,
    require_signature: bool,
) -> Result<Summary> {
    validate_artifact_with(files, schemas, require_signature, None, false)
}

/// `public_key` 非空时对每个渠道的 manifest 做密码学验签；`allow_orphan_blobs`
/// 供镜像目录使用（内容寻址的旧 blob 会保留，不能按孤儿拒绝）。
pub fn validate_artifact_with(
    files: &BTreeMap<String, Vec<u8>>,
    schemas: &SchemaSet,
    require_signature: bool,
    public_key: Option<&str>,
    allow_orphan_blobs: bool,
) -> Result<Summary> {
    for schema_file in SCHEMA_FILES {
        if !files.contains_key(&format!("schema/{schema_file}")) {
            bail!("产物缺少 schema/{schema_file}");
        }
    }

    let channels = channel_names(files)?;
    if channels.is_empty() {
        bail!("产物里没有任何渠道（channels/<channel>/manifest.json）");
    }

    let mut summary = Summary {
        channels: channels.len(),
        ..Summary::default()
    };
    let mut referenced = BTreeSet::new();

    for channel in &channels {
        let prefix = format!("channels/{channel}");
        let manifest_bytes = files
            .get(&format!("{prefix}/manifest.json"))
            .ok_or_else(|| anyhow::anyhow!("渠道 {channel} 缺少 manifest.json"))?;
        let manifest_value: Value = parse_json(manifest_bytes, &format!("{prefix}/manifest.json"))?;
        schemas
            .validate("manifest.schema.json", &manifest_value)
            .with_context(|| format!("manifest schema 校验失败: {channel}"))?;
        let manifest: ManifestFile = serde_json::from_value(manifest_value)
            .with_context(|| format!("manifest 结构解析失败: {channel}"))?;
        manifest.validate()?;

        let sig_bytes = files
            .get(&format!("{prefix}/manifest.json.sig"))
            .ok_or_else(|| anyhow::anyhow!("渠道 {channel} 缺少 manifest.json.sig"))?;
        let sig_value: Value = parse_json(sig_bytes, &format!("{prefix}/manifest.json.sig"))?;
        schemas
            .validate("signature.schema.json", &sig_value)
            .with_context(|| format!("签名 schema 校验失败: {channel}"))?;
        let signature: SignatureFile = serde_json::from_value(sig_value)
            .with_context(|| format!("签名结构解析失败: {channel}"))?;
        signature.validate()?;

        if signature.key_id != manifest.key_id {
            bail!("渠道 {channel} 的 manifest.key_id 与签名 key_id 不一致");
        }
        if is_placeholder_key(&manifest.key_id) || signature.sig.is_empty() {
            if require_signature {
                bail!("渠道 {channel} 未签名：P0 占位产物不得发布，客户端必须拒绝");
            }
            eprintln!("注意：渠道 {channel} 为未签名占位，不得发布；发布请走 publish（见 docs/SKIFF_SERVER.md §14）");
        }

        if let Some(public_key_hex) = public_key {
            let expected = key_id_of_hex(public_key_hex)
                .with_context(|| format!("渠道 {channel} 的固定公钥不合法"))?;
            if signature.key_id != expected || manifest.key_id != expected {
                bail!("渠道 {channel} 的签名 key_id 与固定公钥不一致");
            }
            if signature.sig.is_empty() {
                bail!("渠道 {channel} 未签名，无法用固定公钥验签");
            }
            let sig = hex::decode(&signature.sig)
                .with_context(|| format!("渠道 {channel} 的签名不是十六进制"))?;
            verify_detached(
                public_key_hex,
                &manifest_signing_input(manifest_bytes),
                &sig,
            )
            .with_context(|| format!("渠道 {channel} 的 manifest 验签失败"))?;
        }

        let index_bytes = files
            .get(&format!("{prefix}/{}", manifest.index.path))
            .ok_or_else(|| anyhow::anyhow!("渠道 {channel} 缺少 {}", manifest.index.path))?;
        if sha256_hex(index_bytes) != manifest.index.sha256 {
            bail!("渠道 {channel} 的 index 摘要与 manifest 不一致");
        }
        if index_bytes.len() as u64 != manifest.index.size {
            bail!("渠道 {channel} 的 index 大小与 manifest 不一致");
        }
        let index_value: Value = parse_json(index_bytes, &format!("{prefix}/index.json"))?;
        schemas
            .validate("index.schema.json", &index_value)
            .with_context(|| format!("index schema 校验失败: {channel}"))?;
        let index: IndexFile = serde_json::from_value(index_value)
            .with_context(|| format!("index 结构解析失败: {channel}"))?;
        index.validate()?;
        if index.channel != *channel {
            bail!("渠道 {channel} 的 index.channel={} 不一致", index.channel);
        }
        if index.seq != manifest.seq {
            bail!("渠道 {channel} 的 index.seq 与 manifest 不一致");
        }

        summary.plugins += index.plugins.len();
        summary.apps += index.apps.len();
        summary.runtimes += index.runtimes.len();
        summary.upgrades += index.upgrades.len();

        for entry in &index.plugins {
            let label = format!("插件 {} {}", entry.id, entry.version);
            check_reference(
                files,
                &label,
                entry.pkg_sha256.as_deref(),
                entry.pkg_url.as_deref(),
                entry.size,
                &mut referenced,
            )?;
        }
        for entry in &index.apps {
            let label = format!("应用 {} {}", entry.id, entry.version);
            check_reference(
                files,
                &label,
                entry.compose_sha256.as_deref(),
                entry.compose_url.as_deref(),
                entry.size,
                &mut referenced,
            )?;
        }
        for entry in &index.upgrades {
            let label = format!("升级条目 {} {}", entry.version, entry.arch);
            check_reference(
                files,
                &label,
                entry.pkg_sha256.as_deref(),
                entry.pkg_url.as_deref(),
                entry.size,
                &mut referenced,
            )?;
        }
    }

    if !allow_orphan_blobs {
        for path in files.keys().filter(|key| key.starts_with("blobs/sha256/")) {
            if !referenced.contains(path) {
                bail!("存在未被索引引用的 blob: {path}");
            }
        }
    }
    summary.blobs = referenced.len();
    Ok(summary)
}

fn check_reference(
    files: &BTreeMap<String, Vec<u8>>,
    label: &str,
    digest: Option<&str>,
    url: Option<&str>,
    size: Option<u64>,
    referenced: &mut BTreeSet<String>,
) -> Result<()> {
    let digest = digest.ok_or_else(|| anyhow::anyhow!("{label} 缺少内容摘要"))?;
    let url = url.ok_or_else(|| anyhow::anyhow!("{label} 缺少内容地址"))?;
    let size = size.ok_or_else(|| anyhow::anyhow!("{label} 缺少字节数"))?;
    let bytes = files
        .get(url)
        .ok_or_else(|| anyhow::anyhow!("{label} 引用的 blob 不存在: {url}"))?;
    let actual = sha256_hex(bytes);
    if actual != digest {
        bail!("{label} 的 blob 摘要不匹配（声明 {digest}，实为 {actual}）");
    }
    if bytes.len() as u64 != size {
        bail!(
            "{label} 的 blob 大小不匹配（声明 {size}，实为 {}）",
            bytes.len()
        );
    }
    referenced.insert(url.to_string());
    Ok(())
}

/// 产物自身的 `seq` 必须严格大于 `prev` 目录里同渠道的 `seq`。
pub fn validate_seq_monotonic(files: &BTreeMap<String, Vec<u8>>, prev: &Path) -> Result<()> {
    for channel in channel_names(files)? {
        let current = manifest_of(files, &channel)?;
        let prev_path = prev.join("channels").join(&channel).join("manifest.json");
        if !prev_path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&prev_path)
            .with_context(|| format!("读取上一版 manifest 失败: {}", prev_path.display()))?;
        let previous: ManifestFile = serde_json::from_str(&text)
            .with_context(|| format!("解析上一版 manifest 失败: {}", prev_path.display()))?;
        if current.seq <= previous.seq {
            bail!(
                "渠道 {channel} 的 seq 未递增：当前 {}，上一版 {}（回退被拒绝）",
                current.seq,
                previous.seq
            );
        }
    }
    Ok(())
}

fn manifest_of(files: &BTreeMap<String, Vec<u8>>, channel: &str) -> Result<ManifestFile> {
    let path = format!("channels/{channel}/manifest.json");
    let bytes = files
        .get(&path)
        .ok_or_else(|| anyhow::anyhow!("缺少 {path}"))?;
    serde_json::from_slice(bytes).with_context(|| format!("解析 {path} 失败"))
}

fn channel_names(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for key in files.keys() {
        let Some(rest) = key.strip_prefix("channels/") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once('/') else {
            continue;
        };
        if tail == "manifest.json" {
            names.insert(name.to_string());
        }
    }
    Ok(names.into_iter().collect())
}

pub fn read_tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    if !root.is_dir() {
        bail!("产物目录不存在或不是目录: {}", root.display());
    }
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for path in read_dir_sorted(&dir)? {
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .with_context(|| format!("路径不在产物根下: {}", path.display()))?;
                let key = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let bytes =
                    fs::read(&path).with_context(|| format!("读取产物失败: {}", path.display()))?;
                files.insert(key, bytes);
            }
        }
    }
    Ok(files)
}

fn parse_json(bytes: &[u8], label: &str) -> Result<Value> {
    serde_json::from_slice(bytes).with_context(|| format!("JSON 解析失败: {label}"))
}
