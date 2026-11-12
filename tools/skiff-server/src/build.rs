//! 从源数据目录确定性生成静态产物。
//!
//! 生成规则：
//! - 每个 `catalog/<channel>/` 一份索引，条目数组按稳定键排序；
//! - 源条目的 `asset` 读取后按 sha256 落到 `blobs/sha256/<hex>`，并派生
//!   `*_sha256` / `*_url` / `size`，产物索引里不保留 `asset`；
//! - `generated_at` 与 `seq` 来自源数据，同输入必得同输出（不取系统时间）；
//! - manifest 最后生成，签名覆盖其原始字节，index 摘要因此被间接覆盖。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{
    blob_url, validate_channel, AppEntry, ChannelFile, IndexFile, ManifestFile, ManifestIndexRef,
    PluginEntry, RuntimeEntry, SignatureFile, UpgradeEntry,
};
use crate::schema::SchemaSet;
use crate::sign::{manifest_signing_input, Signer};

pub const SCHEMA_FILES: &[&str] = &[
    "app.schema.json",
    "channel.schema.json",
    "index.schema.json",
    "manifest.schema.json",
    "plugin.schema.json",
    "runtime.schema.json",
    "signature.schema.json",
    "upgrade.schema.json",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Artifact {
    fn put(&mut self, path: String, bytes: Vec<u8>) -> Result<()> {
        if let Some(existing) = self.files.get(&path) {
            if existing != &bytes {
                bail!("产物路径冲突且内容不同: {path}");
            }
        }
        self.files.insert(path, bytes);
        Ok(())
    }

    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn build(catalog_dir: &Path, schema_dir: &Path, signer: &Signer) -> Result<Artifact> {
    let schemas = SchemaSet::new(schema_dir);
    let mut artifact = Artifact::default();
    let mut blobs: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    for channel_dir in channel_directories(catalog_dir)? {
        let name = file_name(&channel_dir)?;
        let channel: ChannelFile = read_json(&channel_dir.join("channel.json"))?;
        schemas.validate("channel.schema.json", &to_value(&channel)?)?;
        channel.validate()?;
        if channel.channel != name {
            bail!(
                "渠道目录 {name} 与 channel.json 的 channel={} 不一致",
                channel.channel
            );
        }

        let mut plugins = Vec::new();
        for mut entry in load_entries::<PluginEntry>(
            &channel_dir.join("plugins"),
            "plugin.schema.json",
            &schemas,
        )? {
            entry.validate_source()?;
            if entry.channel != name {
                bail!(
                    "插件 {} 的 channel={} 与目录 {name} 不一致",
                    entry.id,
                    entry.channel
                );
            }
            let (sha, size) = collect_asset(&entry.asset, catalog_dir, &mut blobs)?;
            entry.pkg_sha256 = Some(sha.clone());
            entry.pkg_url = Some(blob_url(&sha));
            entry.size = Some(size);
            entry.asset = None;
            entry.validate_index()?;
            plugins.push(entry);
        }

        let mut apps = Vec::new();
        for mut entry in
            load_entries::<AppEntry>(&channel_dir.join("apps"), "app.schema.json", &schemas)?
        {
            entry.validate_source()?;
            let (sha, size) = collect_asset(&entry.asset, catalog_dir, &mut blobs)?;
            entry.compose_sha256 = Some(sha.clone());
            entry.compose_url = Some(blob_url(&sha));
            entry.size = Some(size);
            entry.asset = None;
            entry.validate_index()?;
            apps.push(entry);
        }

        let mut runtimes = load_entries::<RuntimeEntry>(
            &channel_dir.join("runtimes"),
            "runtime.schema.json",
            &schemas,
        )?;
        for entry in &runtimes {
            entry.validate()?;
        }

        let mut upgrades = Vec::new();
        for mut entry in load_entries::<UpgradeEntry>(
            &channel_dir.join("upgrades"),
            "upgrade.schema.json",
            &schemas,
        )? {
            entry.validate_source()?;
            if entry.channel != name {
                bail!(
                    "升级条目 {} 的 channel={} 与目录 {name} 不一致",
                    entry.version,
                    entry.channel
                );
            }
            let (sha, size) = collect_asset(&entry.asset, catalog_dir, &mut blobs)?;
            entry.pkg_sha256 = Some(sha.clone());
            entry.pkg_url = Some(blob_url(&sha));
            entry.size = Some(size);
            entry.asset = None;
            entry.validate_index()?;
            upgrades.push(entry);
        }

        plugins.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.version.cmp(&b.version)));
        apps.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.version.cmp(&b.version)));
        runtimes.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.version.cmp(&b.version)));
        upgrades.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.arch.cmp(&b.arch)));
        reject_duplicate_ids("插件", plugins.iter().map(|e| (&e.id, &e.version)))?;
        reject_duplicate_ids("应用", apps.iter().map(|e| (&e.id, &e.version)))?;
        reject_duplicate_ids("运行环境", runtimes.iter().map(|e| (&e.kind, &e.version)))?;
        // 同 kind 至多一个 default：这是 release 时的兜底，API 的元数据修订不做跨条目预判。
        let mut defaults: BTreeMap<&str, &str> = BTreeMap::new();
        for entry in &runtimes {
            if !entry.default {
                continue;
            }
            if let Some(previous) = defaults.insert(entry.kind.as_str(), entry.version.as_str()) {
                bail!(
                    "运行环境 {} 存在多个 default 版本: {} 与 {}",
                    entry.kind,
                    previous,
                    entry.version
                );
            }
        }
        reject_duplicate_ids("升级条目", upgrades.iter().map(|e| (&e.version, &e.arch)))?;

        // 吊销列表：可选源文件，条目同索引里的 `revoked[]`（只增不删）。
        let revoked_path = channel_dir.join("revoked.json");
        let revoked = if revoked_path.is_file() {
            let text = fs::read_to_string(&revoked_path)
                .with_context(|| format!("读取失败: {}", revoked_path.display()))?;
            serde_json::from_str::<Vec<crate::model::RevokedEntry>>(&text)
                .with_context(|| format!("解析失败: {}", revoked_path.display()))?
        } else {
            Vec::new()
        };

        let index = IndexFile {
            plugins,
            apps,
            runtimes,
            upgrades,
            revoked,
            ..IndexFile::empty(&name, channel.seq, &channel.generated_at)
        };
        index.validate()?;
        schemas.validate("index.schema.json", &to_value(&index)?)?;
        let index_bytes = pretty_json(&index)?;
        let index_sha = sha256_hex(&index_bytes);

        let manifest = ManifestFile {
            schema: crate::model::MANIFEST_SCHEMA.to_string(),
            channel: name.clone(),
            seq: channel.seq,
            generated_at: channel.generated_at.clone(),
            key_id: signer.key_id(),
            index: ManifestIndexRef {
                path: "index.json".to_string(),
                sha256: index_sha,
                size: index_bytes.len() as u64,
            },
        };
        manifest.validate()?;
        schemas.validate("manifest.schema.json", &to_value(&manifest)?)?;
        let manifest_bytes = pretty_json(&manifest)?;
        let signature = SignatureFile {
            alg: "ed25519".to_string(),
            key_id: signer.key_id(),
            sig: signer
                .sign(&manifest_signing_input(&manifest_bytes))
                .map(hex::encode)
                .unwrap_or_default(),
        };
        signature.validate()?;
        schemas.validate("signature.schema.json", &to_value(&signature)?)?;

        artifact.put(format!("channels/{name}/index.json"), index_bytes)?;
        artifact.put(format!("channels/{name}/manifest.json"), manifest_bytes)?;
        artifact.put(
            format!("channels/{name}/manifest.json.sig"),
            pretty_json(&signature)?,
        )?;
    }

    if artifact.files.is_empty() {
        bail!("源数据目录下没有任何渠道（期望 catalog/<channel>/channel.json）");
    }
    for (sha, bytes) in blobs {
        artifact.put(format!("blobs/sha256/{sha}"), bytes)?;
    }
    for schema_file in SCHEMA_FILES {
        let path = schema_dir.join(schema_file);
        let bytes =
            fs::read(&path).with_context(|| format!("读取 schema 失败: {}", path.display()))?;
        artifact.put(format!("schema/{schema_file}"), bytes)?;
    }
    Ok(artifact)
}

pub fn write(artifact: &Artifact, out_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        fs::remove_dir_all(out_dir)
            .with_context(|| format!("清理输出目录失败: {}", out_dir.display()))?;
    }
    for (relative, bytes) in &artifact.files {
        let path = out_dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
        fs::write(&path, bytes).with_context(|| format!("写入失败: {}", path.display()))?;
    }
    Ok(())
}

fn channel_directories(catalog_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for path in read_dir_sorted(catalog_dir)? {
        if !path.is_dir() {
            bail!("源数据根目录只允许子目录: {}", path.display());
        }
        let name = file_name(&path)?;
        if name == "assets" {
            continue;
        }
        validate_channel(&name)?;
        if !path.join("channel.json").is_file() {
            bail!("渠道目录 {} 缺少 channel.json", path.display());
        }
        dirs.push(path);
    }
    dirs.sort();
    Ok(dirs)
}

fn load_entries<T: DeserializeOwned>(
    dir: &Path,
    schema_file: &str,
    schemas: &SchemaSet,
) -> Result<Vec<T>> {
    let mut entries = Vec::new();
    for path in read_dir_sorted(dir)? {
        if !path.is_file() {
            bail!("条目目录只允许文件: {}", path.display());
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            bail!("条目目录只允许 .json: {}", path.display());
        }
        let value: Value = read_json(&path)?;
        schemas
            .validate(schema_file, &value)
            .with_context(|| format!("schema 校验失败: {}", path.display()))?;
        let entry = serde_json::from_value(value)
            .with_context(|| format!("条目结构解析失败: {}", path.display()))?;
        entries.push(entry);
    }
    Ok(entries)
}

fn collect_asset(
    asset: &Option<String>,
    catalog_dir: &Path,
    blobs: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(String, u64)> {
    let asset = asset
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("源条目缺少 asset"))?;
    let path = catalog_dir.join(asset);
    let bytes = fs::read(&path).with_context(|| format!("读取 asset 失败: {}", path.display()))?;
    let sha = sha256_hex(&bytes);
    let size = bytes.len() as u64;
    blobs.insert(sha.clone(), bytes);
    Ok((sha, size))
}

fn reject_duplicate_ids<'a>(
    kind: &str,
    pairs: impl Iterator<Item = (&'a String, &'a String)>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (id, version) in pairs {
        if !seen.insert((id.clone(), version.clone())) {
            bail!("{kind}条目重复: {id} {version}");
        }
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("读取失败: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("JSON 解析失败: {}", path.display()))
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).context("JSON 序列化失败")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn to_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).context("JSON 值转换失败")
}

pub fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("读取目录失败: {}", dir.display()))?
    {
        paths.push(entry.context("读取目录项失败")?.path());
    }
    paths.sort();
    Ok(paths)
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("路径没有合法文件名: {}", path.display()))
}
