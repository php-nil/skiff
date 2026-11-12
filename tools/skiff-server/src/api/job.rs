//! 发布 job（§17.5）：把 `approved` 提交落成源数据，再复用 `publish` 生成签名产物。
//!
//! **签名仍离线**：本模块只通过 `SKIFF_SIGNING_KEY` 环境变量取私钥，且不假设
//! 「生成方 = 分发方」——可以只产出 staging（`--staging-out`），交给发布机/CI
//! 搬运；只有显式 `--dist --swap` 才做线上目录换入（同机部署属测试姿态）。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::api::now_rfc3339;
use crate::api::store::{self, EntryKind, RuntimeRevision, Submission, TakedownInfo};
use crate::api::takedown::{self, Kind, Takedown};
use crate::events::{self, Event};
use crate::model::{AppEntry, ChannelFile, PluginEntry, RuntimeEntry};
use crate::sign::Signer;

#[derive(Debug, Clone)]
pub struct JobOptions {
    pub state: PathBuf,
    pub catalog: PathBuf,
    pub schema: PathBuf,
    pub channel: String,
    /// 只产出 staging 目录（与 `--dist` 互斥）。
    pub staging_out: Option<PathBuf>,
    /// 线上产物目录（必须显式 `--swap`）。
    pub dist: Option<PathBuf>,
    pub swap: bool,
    /// 由调用方给定 seq（离线发布机常用）。
    pub seq: Option<u64>,
    /// seq 只读基准：从该产物目录的 manifest 读取。
    pub prev: Option<PathBuf>,
    /// 只处理单个提交。
    pub submission: Option<String>,
    pub public_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JobReport {
    /// 已标记发布的 `(sid, seq)`。
    pub released: Vec<(String, u64)>,
    /// 已随本次发布生效的撤包/吊销记录 id。
    pub takedowns: Vec<String>,
    pub seq: u64,
    pub output: PathBuf,
    pub swapped: bool,
}

pub fn run(options: &JobOptions) -> Result<JobReport> {
    match run_inner(options) {
        Ok(Some(report)) => {
            let ids: Vec<String> = report.released.iter().map(|(sid, _)| sid.clone()).collect();
            // 通知是 best-effort：写 outbox 失败不能让已完成的发布回滚。
            let _ = events::emit(
                &options.state,
                &Event::new(
                    events::EVENT_TYPE_SUBMISSION_PUBLISHED,
                    &format!("{}:{}:release", options.channel, report.seq),
                    &options.channel,
                )
                .submissions(&ids)
                .seq(report.seq),
            );
            Ok(report)
        }
        Ok(None) => {
            bail!("没有 approved 的提交，也没有待发布的撤包/吊销；先 approve 或先发起 yank/revoke")
        }
        Err(error) => {
            let _ = events::emit(
                &options.state,
                &Event::new(
                    events::EVENT_TYPE_RELEASE_FAILED,
                    &format!("{}:{}", options.channel, now_rfc3339()),
                    &options.channel,
                )
                .error_code("release_failed"),
            );
            Err(error)
        }
    }
}

fn run_inner(options: &JobOptions) -> Result<Option<JobReport>> {
    validate_destination(options)?;

    let mut approved = store::approved_for(&options.state, &options.channel)
        .map_err(|_| anyhow::anyhow!("读取提交状态失败"))?;
    if let Some(only) = &options.submission {
        approved.retain(|r| &r.sid == only);
        if approved.is_empty() {
            bail!("提交 {only} 不存在或不是 approved");
        }
    }
    let mut takedowns =
        takedown::pending(&options.state).map_err(|_| anyhow::anyhow!("读取撤包状态失败"))?;
    takedowns.retain(|record| record.channel == options.channel);
    let revisions = store::pending_runtime_revisions(&options.state)
        .map_err(|_| anyhow::anyhow!("读取运行环境修订失败"))?
        .into_iter()
        .filter(|revision| revision.channel == options.channel)
        .collect::<Vec<_>>();
    if approved.is_empty() && takedowns.is_empty() && revisions.is_empty() {
        return Ok(None);
    }

    let prev = resolve_prev(options);
    let seq = resolve_seq(options, prev.as_deref())?;
    let now = now_rfc3339();

    // 私钥与固定公钥必须在写源数据之前就绪：缺钥时不能先改源。
    let signer = Signer::from_env()?;
    let public_key = options
        .public_key
        .clone()
        .or_else(|| std::env::var("SKIFF_SERVER_PUBLIC_KEY").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("缺少固定公钥：请设置 SKIFF_SERVER_PUBLIC_KEY 或 --public-key")
        })?;

    // 1) 源数据：包体 + 条目 + 撤包标记 + 运行环境元数据修订 + 渠道 seq。全部先写源数据，再触发 publish。
    write_sources(options, &approved, &takedowns, seq, &now)?;
    apply_runtime_revisions(options, &revisions)?;

    // 2) 产物：publish 在内存完成验签自检后才写盘；staging 不碰线上目录。
    let (output, swapped) = match (&options.staging_out, &options.dist) {
        (Some(staging), _) => {
            crate::release::publish(
                &options.catalog,
                &options.schema,
                staging,
                prev.as_deref(),
                &signer,
                &public_key,
            )
            .context("生成 staging 失败")?;
            (staging.clone(), false)
        }
        (None, Some(dist)) => {
            let staging = sibling_staging(dist);
            let _ = fs::remove_dir_all(&staging);
            crate::release::publish(
                &options.catalog,
                &options.schema,
                &staging,
                prev.as_deref(),
                &signer,
                &public_key,
            )
            .context("生成产物失败（线上目录未改动）")?;
            swap_into(&staging, dist).context("换入线上目录失败（已回滚上一版）")?;
            (dist.clone(), true)
        }
        _ => unreachable!("validate_destination 已保证"),
    };

    // 3) 回写状态。staging 模式表示「产物已生成」，搬运由调用方完成。
    let mut released = Vec::new();
    for submission in &approved {
        store::mark_published(&options.state, &submission.sid, seq)
            .map_err(|_| anyhow::anyhow!("回写发布状态失败: {}", submission.sid))?;
        released.push((submission.sid.clone(), seq));
    }
    let mut released_takedowns = Vec::new();
    for record in &takedowns {
        takedown::mark_released(&options.state, &record.id, seq)
            .map_err(|_| anyhow::anyhow!("回写撤包状态失败: {}", record.id))?;
        // 站内可见状态：已安装/已发布方能在自己的提交详情里看到撤包原因。
        store::attach_takedown(
            &options.state,
            record.entry_kind,
            &record.plugin_id,
            &record.version,
            &TakedownInfo {
                kind: record.kind.as_str().to_string(),
                reason: record.reason.clone(),
                key_id: record.key_id.clone(),
                at: record.at.clone(),
            },
        )
        .map_err(|_| anyhow::anyhow!("回写撤包提示失败: {}", record.id))?;
        released_takedowns.push(record.id.clone());
    }
    for revision in &revisions {
        store::mark_runtime_revision_released(&options.state, &revision.id, seq)
            .map_err(|_| anyhow::anyhow!("回写运行环境修订状态失败: {}", revision.id))?;
    }
    append_release_log(
        options,
        &released,
        &released_takedowns,
        seq,
        &output,
        swapped,
    );
    Ok(Some(JobReport {
        released,
        takedowns: released_takedowns,
        seq,
        output,
        swapped,
    }))
}

/// 已发布运行环境的元数据修订：只改 `eol` / `default`，镜像与 digest 必须与源条目一致。
fn apply_runtime_revisions(options: &JobOptions, revisions: &[RuntimeRevision]) -> Result<()> {
    for revision in revisions {
        let path = options
            .catalog
            .join(&options.channel)
            .join("runtimes")
            .join(format!("{}-{}.json", revision.kind, revision.version));
        let text = fs::read_to_string(&path)
            .with_context(|| format!("读取运行环境源条目失败: {}", path.display()))?;
        let mut entry: RuntimeEntry = serde_json::from_str(&text)
            .with_context(|| format!("解析运行环境源条目失败: {}", path.display()))?;
        if entry.image_digest != revision.image_digest {
            bail!(
                "运行环境 {} {} 的 image_digest 与修订不一致（digest 不可改）",
                revision.kind,
                revision.version
            );
        }
        entry.eol = revision.eol.clone();
        entry.default = revision.default;
        entry
            .validate()
            .with_context(|| format!("运行环境修订不合法: {}", revision.id))?;
        let bytes = serde_json::to_vec_pretty(&entry)?;
        fs::write(&path, bytes_with_newline(bytes))
            .with_context(|| format!("写入运行环境源条目失败: {}", path.display()))?;
    }
    Ok(())
}

fn validate_destination(options: &JobOptions) -> Result<()> {
    match (&options.staging_out, &options.dist) {
        (Some(_), Some(_)) => bail!("--staging-out 与 --dist 互斥"),
        (None, None) => bail!("必须给出 --staging-out，或 --dist 且显式 --swap"),
        (Some(staging), None) => {
            if options.swap {
                bail!("--swap 只与 --dist 搭配");
            }
            if staging.exists() {
                let empty = fs::read_dir(staging)
                    .map(|mut it| it.next().is_none())
                    .unwrap_or(false);
                if !empty {
                    bail!(
                        "--staging-out 已存在且非空，拒绝覆盖: {}",
                        staging.display()
                    );
                }
            }
            Ok(())
        }
        (None, Some(_)) => {
            if !options.swap {
                bail!("--dist 必须显式加 --swap 才允许换入线上目录");
            }
            Ok(())
        }
    }
}

fn resolve_prev(options: &JobOptions) -> Option<PathBuf> {
    if let Some(prev) = &options.prev {
        return Some(prev.clone());
    }
    options
        .dist
        .as_ref()
        .filter(|dir| dir.join("channels").is_dir())
        .cloned()
}

fn resolve_seq(options: &JobOptions, prev: Option<&Path>) -> Result<u64> {
    if let Some(seq) = options.seq {
        if seq == 0 {
            bail!("--seq 必须从 1 开始");
        }
        return Ok(seq);
    }
    let Some(prev) = prev else {
        return Ok(1);
    };
    let manifest = prev
        .join("channels")
        .join(&options.channel)
        .join("manifest.json");
    let text = fs::read_to_string(&manifest)
        .with_context(|| format!("读取上一版 manifest 失败: {}", manifest.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context("上一版 manifest 不是合法 JSON")?;
    let seq = value
        .get("seq")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow::anyhow!("上一版 manifest 缺少 seq"))?;
    Ok(seq + 1)
}

fn write_sources(
    options: &JobOptions,
    approved: &[Submission],
    takedowns: &[Takedown],
    seq: u64,
    now: &str,
) -> Result<()> {
    let channel_dir = options.catalog.join(&options.channel);
    let channel_file = channel_dir.join("channel.json");
    if !channel_file.is_file() {
        bail!("源数据缺少渠道配置: {}", channel_file.display());
    }
    let mut channel: ChannelFile =
        serde_json::from_slice(&fs::read(&channel_file).context("读取 channel.json 失败")?)
            .context("channel.json 解析失败")?;
    if channel.channel != options.channel {
        bail!("channel.json 的 channel 与 --channel 不一致");
    }

    let assets = options.catalog.join("assets");
    fs::create_dir_all(&assets).context("创建 assets 目录失败")?;
    let plugins = channel_dir.join("plugins");
    fs::create_dir_all(&plugins).context("创建 plugins 目录失败")?;
    let apps = channel_dir.join("apps");
    fs::create_dir_all(&apps).context("创建 apps 目录失败")?;

    for submission in approved {
        match submission.kind {
            EntryKind::Plugin => {
                let source = store::package_path(&options.state, &submission.sid);
                let asset_name = format!("assets/{}.wbpkg", submission.sha256);
                let target = options.catalog.join(&asset_name);
                fs::copy(&source, &target)
                    .with_context(|| format!("复制包体失败: {}", source.display()))?;

                let entry = PluginEntry {
                    id: submission.plugin_id.clone(),
                    version: submission.version.clone(),
                    api: submission.api.clone(),
                    channel: submission.channel.clone(),
                    key_id: submission.key_id.clone(),
                    requires: submission.requires.clone(),
                    summary: submission.entry.summary.clone(),
                    published_at: now.to_string(),
                    yanked: false,
                    yank_reason: None,
                    pkg_sha256: None,
                    pkg_url: None,
                    size: None,
                    asset: Some(asset_name.clone()),
                };
                entry.validate_source().context("源条目不合法")?;
                let path = plugins.join(format!("{}-{}.json", entry.id, entry.version));
                if path.exists() {
                    bail!(
                        "源数据已存在同版本条目（可能来自另一份提交）: {}",
                        path.display()
                    );
                }
                let bytes = serde_json::to_vec_pretty(&entry)?;
                fs::write(&path, bytes_with_newline(bytes))
                    .with_context(|| format!("写入条目失败: {}", path.display()))?;
            }
            EntryKind::App => {
                let meta = submission.app.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("应用提交 {} 缺少 app 元数据", submission.sid)
                })?;
                let source = store::compose_path(&options.state, &submission.sid);
                let asset_name = format!("assets/{}.yaml", submission.sha256);
                let target = options.catalog.join(&asset_name);
                fs::copy(&source, &target)
                    .with_context(|| format!("复制 compose 失败: {}", source.display()))?;

                let entry = AppEntry {
                    id: submission.plugin_id.clone(),
                    version: submission.version.clone(),
                    summary: submission.entry.summary.clone(),
                    min_kernel: meta.min_kernel.clone(),
                    images: meta.images.clone(),
                    env: meta.env.clone(),
                    ports: meta.ports.clone(),
                    volumes: meta.volumes.clone(),
                    compose_sha256: None,
                    compose_url: None,
                    size: None,
                    asset: Some(asset_name),
                    yanked: false,
                    yank_reason: None,
                };
                entry.validate_source().context("源条目不合法")?;
                let path = apps.join(format!("{}-{}.json", entry.id, entry.version));
                if path.exists() {
                    bail!(
                        "源数据已存在同版本应用（可能来自另一份提交）: {}",
                        path.display()
                    );
                }
                let bytes = serde_json::to_vec_pretty(&entry)?;
                fs::write(&path, bytes_with_newline(bytes))
                    .with_context(|| format!("写入条目失败: {}", path.display()))?;
            }
            EntryKind::Runtime => {
                let meta = submission.runtime.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("运行环境提交 {} 缺少 runtime 元数据", submission.sid)
                })?;
                let runtimes = channel_dir.join("runtimes");
                fs::create_dir_all(&runtimes).context("创建 runtimes 目录失败")?;
                let entry = RuntimeEntry {
                    kind: submission.plugin_id.clone(),
                    version: submission.version.clone(),
                    image: meta.image.clone(),
                    image_digest: meta.image_digest.clone(),
                    eol: meta.eol.clone(),
                    api_min: submission.api.clone(),
                    default: meta.default,
                    yanked: false,
                    yank_reason: None,
                };
                entry.validate().context("源条目不合法")?;
                let path = runtimes.join(format!("{}-{}.json", entry.kind, entry.version));
                if path.exists() {
                    bail!(
                        "源数据已存在同版本运行环境（可能来自另一份提交）: {}",
                        path.display()
                    );
                }
                let bytes = serde_json::to_vec_pretty(&entry)?;
                fs::write(&path, bytes_with_newline(bytes))
                    .with_context(|| format!("写入条目失败: {}", path.display()))?;
            }
        }
    }

    // 撤包/吊销：只改既有条目的 `yanked` 与追加对应 kind 的吊销列表，不删除任何条目。
    for record in takedowns {
        takedown::mark_source_yanked(
            &options.catalog,
            &options.channel,
            record.entry_kind,
            &record.plugin_id,
            &record.version,
            &record.reason,
        )
        .map_err(anyhow::Error::msg)?;
        let revoke = match record.kind {
            Kind::Revoke => record.entry_kind.is_plugin(),
            // 应用/运行环境没有作者密钥可吊销：撤包即登记到该 kind 的独立命名空间。
            Kind::Yank => !record.entry_kind.is_plugin(),
        };
        if revoke {
            let sha = takedown::source_digest(
                &options.catalog,
                &options.channel,
                record.entry_kind,
                &record.plugin_id,
                &record.version,
            )
            .map_err(anyhow::Error::msg)?;
            takedown::append_source_revocation(
                &options.catalog,
                &options.channel,
                record,
                sha.as_deref(),
            )
            .map_err(anyhow::Error::msg)?;
        }
    }

    channel.seq = seq;
    channel.generated_at = now.to_string();
    channel.validate()?;
    let bytes = serde_json::to_vec_pretty(&channel)?;
    fs::write(&channel_file, bytes_with_newline(bytes))
        .with_context(|| format!("写入 channel.json 失败: {}", channel_file.display()))?;
    Ok(())
}

fn bytes_with_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

fn sibling_staging(dist: &Path) -> PathBuf {
    let name = dist.file_name().and_then(|n| n.to_str()).unwrap_or("dist");
    dist.with_file_name(format!(".{name}.new-{}", std::process::id()))
}

/// 目录换入：旧目录先改名保留，新的换入成功后再删；任一步失败都回滚旧版。
fn swap_into(staging: &Path, dist: &Path) -> Result<()> {
    let name = dist.file_name().and_then(|n| n.to_str()).unwrap_or("dist");
    let backup = dist.with_file_name(format!(".{name}.old-{}", std::process::id()));
    let had_previous = dist.exists();
    if had_previous {
        fs::rename(dist, &backup)
            .with_context(|| format!("备份旧产物目录失败: {}", dist.display()))?;
    }
    if let Err(error) = fs::rename(staging, dist) {
        if had_previous {
            let _ = fs::rename(&backup, dist);
        }
        return Err(error).context("换入新产物失败");
    }
    if had_previous {
        let _ = fs::remove_dir_all(&backup);
    }
    Ok(())
}

fn append_release_log(
    options: &JobOptions,
    released: &[(String, u64)],
    takedowns: &[String],
    seq: u64,
    output: &Path,
    swapped: bool,
) {
    let line = json!({
        "at": now_rfc3339(),
        "action": "release",
        "channel": options.channel,
        "seq": seq,
        "submissions": released.iter().map(|(sid, _)| sid).collect::<Vec<_>>(),
        "takedowns": takedowns,
        "output": output.display().to_string(),
        "swapped": swapped,
    });
    let mut text = line.to_string();
    text.push('\n');
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(options.state.join("audit.jsonl"))
        .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()));
}
