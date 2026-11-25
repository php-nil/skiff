//! `runtimes` 子命令：把上游版本表变成 catalog 条目、版本清单与面板内置副本。
//!
//! 一次跑完四步：取上游分支 → 规划每类的目标版本 → 写
//! `catalog/<channel>/runtimes/<kind>-<version>.json` → `build` 出产物并回填内置副本。
//!
//! 只增不删：上游不再支持的旧条目默认保留（老站点升级还要选到它们），要清理得显式
//! `--prune`；`yanked` 条目一律不动——它是下架记录，不是过期数据。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::build::{build, read_dir_sorted, version_key, write, RUNTIME_INDEX_KINDS};
use crate::model::RuntimeEntry;
use crate::sign::Signer;
use crate::upstream::{Cycle, Upstream};

/// 一类运行环境的执行结果。
#[derive(Debug, Default, Clone)]
pub struct KindReport {
    /// 目标版本（数值降序）。
    pub versions: Vec<String>,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// 拿不到 digest / 版本号不合法而没写的版本，带原因。
    pub skipped: Vec<String>,
    /// 产物里该类清单的条目数（None 表示产物没这份文件）。
    pub listed: Option<usize>,
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub kinds: Vec<(String, KindReport)>,
    /// 因 `--prune` 删掉的条目文件（相对 catalog 渠道目录）。
    pub pruned: Vec<String>,
    /// 内置副本是否写入。
    pub builtin_written: bool,
    /// 写了几个产物文件（`--out` 时的整份产物数）。
    pub artifact_files: usize,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub channel: String,
    pub catalog: PathBuf,
    pub schema: PathBuf,
    pub kinds: Vec<String>,
    pub builtin: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub today: String,
    pub prune: bool,
    pub dry_run: bool,
    /// 每个发布分支保留几个版本（`1` 表示只要最新）。
    pub per_branch: usize,
}

/// 规划出来的一个版本。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Desired {
    version: String,
    eol: Option<String>,
    image: String,
}

/// 单个类型的预检结果：只含上游与仓库的答案，还没碰磁盘。
struct Planned {
    kind: String,
    resolved: Vec<(Desired, String)>,
    skipped: Vec<String>,
}

pub fn run(upstream: &dyn Upstream, options: &Options) -> Result<Report> {
    let channel_dir = options.catalog.join(&options.channel);
    if !channel_dir.is_dir() {
        bail!("渠道目录不存在: {}", channel_dir.display());
    }
    let runtimes_dir = channel_dir.join("runtimes");
    let existing = load_existing(&runtimes_dir)?;
    let mut report = Report::default();

    // 阶段 1（只读）：算清每类写哪些版本、摘要拿不拿得到。任何一类「有目标版本但一个都
    // 拿不到摘要」就整体退出——先写一半再失败会留下半新半旧的目录，比压根不跑更坏。
    let mut planned: Vec<Planned> = Vec::new();
    for kind in &options.kinds {
        let mut skipped = Vec::new();
        let cycles = upstream
            .cycles(kind)
            .with_context(|| format!("取 {kind} 上游版本失败"))?;

        // 每个受支持分支再问一次「最近几个版本」。这个接口挂掉不该挡住整类：退回
        // 只用 latest（功能不残），但要留下原因，否则「为什么只有一个版本」无从查起。
        let mut desired = Vec::new();
        let mut seen = BTreeSet::new();
        for cycle in supported_cycles(&cycles, kind, &options.today) {
            let versions = match upstream.branch_versions(kind, cycle, options.per_branch) {
                Ok(versions) => versions,
                Err(error) => {
                    skipped.push(format!(
                        "{}（历史版本查询失败，只保留 {}：{}）",
                        cycle.cycle,
                        cycle.latest,
                        one_line(&error)
                    ));
                    vec![cycle.latest.clone()]
                }
            };
            desired.extend(desired_from(
                kind,
                cycle,
                &versions,
                &mut seen,
                &mut skipped,
            ));
        }
        desired.sort_by_key(|item| {
            std::cmp::Reverse((version_key(&item.version), item.version.clone()))
        });

        // 已有的同 (kind, version, image) 条目沿用旧 digest：版本没变就不该再打一次仓库。
        let mut reused = BTreeMap::new();
        for entry in existing.iter().filter(|entry| entry.kind == *kind) {
            reused.insert(
                (entry.version.clone(), entry.image.clone()),
                entry.image_digest.clone(),
            );
        }

        let mut resolved = Vec::new();
        for item in &desired {
            let digest = match reused.get(&(item.version.clone(), item.image.clone())) {
                Some(digest) => digest.clone(),
                // 演练只为看清「上游多了哪些版本」，不该被仓库连通性挡住：给个合法占位，
                // 反正不落盘（写了才会进签名索引，那才是不能糊弄的地方）。
                None if options.dry_run => format!("sha256:{}", "0".repeat(64)),
                None => match upstream.image_digest(&item.image) {
                    Ok(digest) => digest,
                    Err(error) => {
                        skipped.push(format!("{}（{}）", item.version, one_line(&error)));
                        continue;
                    }
                },
            };
            resolved.push((item.clone(), digest));
        }
        if !desired.is_empty() && resolved.is_empty() {
            // 带上首个失败原因：否则「都拿不到」既可能是 tag 没就绪，也可能是代理/网络不通，
            // 现场只能靠猜（`--proxy` 配错时尤其明显）。
            let cause = skipped
                .last()
                .map(|item| format!("；首个原因：{item}"))
                .unwrap_or_default();
            bail!(
                "{kind}：{} 个目标版本都拿不到镜像摘要（需要能访问 registry-1.docker.io；\
                 直连不通时加 --proxy <url>；tag 也可能尚未就绪），未写任何文件{cause}",
                desired.len()
            );
        }
        planned.push(Planned {
            kind: kind.clone(),
            resolved,
            skipped,
        });
    }

    // 阶段 2（写盘）：先写目标版本，再处理旧条目；演练只算数不落盘。
    for plan in &planned {
        let kind = &plan.kind;
        let mut kind_report = KindReport {
            skipped: plan.skipped.clone(),
            ..KindReport::default()
        };
        let versions: Vec<String> = plan
            .resolved
            .iter()
            .map(|(item, _)| item.version.clone())
            .collect();
        let default = pick_default(&existing, kind, &versions);

        let mut written = BTreeSet::new();
        for (item, digest) in &plan.resolved {
            let entry = RuntimeEntry {
                kind: kind.clone(),
                version: item.version.clone(),
                image: item.image.clone(),
                image_digest: digest.clone(),
                eol: item.eol.clone(),
                api_min: api_min_for(&existing, kind),
                default: default.as_deref() == Some(item.version.as_str()),
                yanked: false,
                yank_reason: None,
            };
            entry
                .validate()
                .with_context(|| format!("生成的条目不合法: {kind} {}", item.version))?;
            let path = runtimes_dir.join(format!("{kind}-{}.json", item.version));
            match write_entry(&path, &entry, options.dry_run)? {
                WriteOutcome::Added => kind_report.added += 1,
                WriteOutcome::Updated => kind_report.updated += 1,
                WriteOutcome::Unchanged => kind_report.unchanged += 1,
            }
            written.insert(item.version.clone());
        }
        kind_report.versions = versions;

        // 上游一个可用版本都没有（接口故障 / 全 EOL）时不动旧条目：此时 prune 会把整类
        // 删光，摘掉 default 也会让清单没有默认项。
        if plan.resolved.is_empty() {
            report.kinds.push((kind.clone(), kind_report));
            continue;
        }

        // 清理：只删「同类型 + 不在目标集合 + 未 yanked」的条目，且必须显式 --prune。
        // 不删时也要把它的 default 摘掉——默认版本只能有一个，留着会让清单出现两个默认项。
        for entry in existing.iter().filter(|entry| entry.kind == *kind) {
            if written.contains(&entry.version) || entry.yanked {
                continue;
            }
            let name = format!("{kind}-{}.json", entry.version);
            if options.prune {
                let path = runtimes_dir.join(&name);
                if !options.dry_run {
                    fs::remove_file(&path)
                        .with_context(|| format!("删除过期条目失败: {}", path.display()))?;
                }
                report.pruned.push(name);
                continue;
            }
            if entry.default {
                let mut fixed = entry.clone();
                fixed.default = false;
                let path = runtimes_dir.join(&name);
                if matches!(
                    write_entry(&path, &fixed, options.dry_run)?,
                    WriteOutcome::Updated
                ) {
                    kind_report.updated += 1;
                }
            }
            kind_report.skipped.push(format!(
                "{}（不在本次目标集合，保留；要删加 --prune）",
                entry.version
            ));
        }
        report.kinds.push((kind.clone(), kind_report));
    }

    // 演练不写条目，`build` 读到的还是旧磁盘内容，产物与清单预览都会骗人——直接跳过。
    if options.dry_run {
        return Ok(report);
    }

    // 产物：清单与内置副本都取自同一次 build，字节天然一致（check --builtin-index 直接过）。
    let artifact = build(&options.catalog, &options.schema, &Signer::Unsigned)
        .context("用更新后的源数据生成产物失败")?;
    let prefix = format!("channels/{}/runtime-index", options.channel);
    for (kind, kind_report) in &mut report.kinds {
        let path = format!("{prefix}/{kind}.json");
        kind_report.listed = artifact.files.get(&path).map(|bytes| count_versions(bytes));
    }

    if let Some(builtin) = &options.builtin {
        let path = format!("{prefix}/php.json");
        match artifact.files.get(&path) {
            Some(bytes) => {
                let target = builtin.join("php.json");
                if !options.dry_run {
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).with_context(|| {
                            format!("创建内置副本目录失败: {}", parent.display())
                        })?;
                    }
                    if fs::read(&target).ok().as_deref() != Some(&bytes[..]) {
                        fs::write(&target, bytes)
                            .with_context(|| format!("写入内置副本失败: {}", target.display()))?;
                        report.builtin_written = true;
                    }
                } else if fs::read(&target).ok().as_deref() != Some(&bytes[..]) {
                    report.builtin_written = true;
                }
            }
            None => eprintln!("警告：产物里没有 {path}（php 没有可用条目），内置副本保持原样"),
        }
    }

    if let Some(out) = &options.out {
        if options.dry_run {
            report.artifact_files = artifact.files.len();
        } else {
            write(&artifact, out).with_context(|| format!("写产物失败: {}", out.display()))?;
            report.artifact_files = artifact.files.len();
        }
    }

    Ok(report)
}

/// 从上游分支表挑出要发布的分支。
///
/// 口径：PHP 取全部受支持分支（含刚 GA 的新分支），Node.js 只取已进 LTS 的分支，
/// Rust 只取当前 stable；EOL 日期已过（`<= today`）的一律不收。
fn supported_cycles<'a>(cycles: &'a [Cycle], kind: &str, today: &str) -> Vec<&'a Cycle> {
    cycles
        .iter()
        .filter(|cycle| match kind {
            "php" => cycle.eol.as_deref().is_none_or(|eol| eol > today),
            "node" => cycle.lts && cycle.eol.as_deref().is_none_or(|eol| eol > today),
            "rust" => cycle.eol.is_none(),
            _ => false,
        })
        .collect()
}

/// 一个分支的版本列表 → 待写条目。非法版本号与重复版本在这里被挡掉。
///
/// 保留的版本共用分支的 EOL：补丁之间没有独立生命周期。
fn desired_from(
    kind: &str,
    cycle: &Cycle,
    versions: &[String],
    seen: &mut BTreeSet<String>,
    skipped: &mut Vec<String>,
) -> Vec<Desired> {
    let mut out = Vec::new();
    for version in versions {
        if !is_plain_version(version) {
            skipped.push(format!(
                "{}（上游版本号 `{version}` 不是 1–3 段数字，面板不认）",
                cycle.cycle
            ));
            continue;
        }
        if !seen.insert(version.clone()) {
            continue;
        }
        out.push(Desired {
            image: image_for(kind, version),
            version: version.clone(),
            eol: cycle.eol.clone(),
        });
    }
    out
}

/// 默认版本：原 default 条目还在可写集合里就保持；否则取其中版本最高的那个。
fn pick_default(existing: &[RuntimeEntry], kind: &str, versions: &[String]) -> Option<String> {
    let current = existing
        .iter()
        .find(|entry| entry.kind == kind && entry.default)
        .map(|entry| entry.version.as_str());
    if let Some(current) = current {
        if versions.iter().any(|version| version == current) {
            return Some(current.to_string());
        }
    }
    versions.first().cloned()
}

fn api_min_for(existing: &[RuntimeEntry], kind: &str) -> String {
    existing
        .iter()
        .find(|entry| entry.kind == kind)
        .map(|entry| entry.api_min.clone())
        .unwrap_or_else(|| "1.0".to_string())
}

/// 与内核 `runtime_catalog::image_for` 的裸名部分一致（前缀在运行期拼，不进目录）。
fn image_for(kind: &str, version: &str) -> String {
    match kind {
        "php" => format!("php:{version}-fpm-alpine"),
        "node" => format!("node:{version}-alpine"),
        "rust" => format!("rust:{version}-slim"),
        other => format!("{other}:{version}"),
    }
}

/// 面板的清单解析只认 1–3 段纯数字（`24` 也算合法），这里提前挡掉。
pub fn is_plain_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn load_existing(dir: &Path) -> Result<Vec<RuntimeEntry>> {
    let mut entries = Vec::new();
    for path in read_dir_sorted(dir)? {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("读取条目失败: {}", path.display()))?;
        let entry: RuntimeEntry = serde_json::from_slice(&bytes)
            .with_context(|| format!("解析条目失败: {}", path.display()))?;
        entries.push(entry);
    }
    Ok(entries)
}

enum WriteOutcome {
    Added,
    Updated,
    Unchanged,
}

fn write_entry(path: &Path, entry: &RuntimeEntry, dry_run: bool) -> Result<WriteOutcome> {
    let mut bytes = serde_json::to_vec_pretty(entry).context("条目序列化失败")?;
    bytes.push(b'\n');
    let existed = path.is_file();
    if fs::read(path).ok().as_deref() == Some(&bytes[..]) {
        return Ok(WriteOutcome::Unchanged);
    }
    if !dry_run {
        fs::write(path, &bytes).with_context(|| format!("写入条目失败: {}", path.display()))?;
    }
    Ok(if existed {
        WriteOutcome::Updated
    } else {
        WriteOutcome::Added
    })
}

fn count_versions(bytes: &[u8]) -> usize {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| value["versions"].as_array().map(Vec::len))
        .unwrap_or(0)
}

fn one_line(error: &anyhow::Error) -> String {
    error
        .to_string()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 默认要刷的三类（与 `build` 的清单类型同一份常量）。
pub fn all_kinds() -> Vec<String> {
    RUNTIME_INDEX_KINDS
        .iter()
        .map(|kind| kind.to_string())
        .collect()
}

/// 目标渠道里实际发布的类型（默认三类，`--kinds` 可收窄）。
pub fn parse_kinds(value: &str) -> Result<Vec<String>> {
    let mut kinds = Vec::new();
    for part in value.split(',') {
        let kind = part.trim();
        if kind.is_empty() {
            continue;
        }
        if !RUNTIME_INDEX_KINDS.contains(&kind) {
            bail!("未知运行环境类型: {kind}（只支持 php / node / rust）");
        }
        kinds.push(kind.to_string());
    }
    if kinds.is_empty() {
        bail!("--kinds 不能为空");
    }
    Ok(kinds)
}

/// `YYYY-MM-DD`（UTC）。用自带的 civil 换算，避免为一个日期引入依赖。
pub fn today_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    civil_from_days((seconds / 86_400) as i64)
}

fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(18_262), "2020-01-01");
        assert_eq!(civil_from_days(20_782), "2026-11-25");
    }

    #[test]
    fn plain_version_accepts_one_to_three_numeric_segments() {
        for good in ["8.5.13", "24", "1.100.0", "8.5"] {
            assert!(is_plain_version(good), "{good} 应合法");
        }
        for bad in ["8.5.13-beta", "8.5.13.1", "", "8..5", "eight"] {
            assert!(!is_plain_version(bad), "{bad} 应被拒");
        }
    }

    #[test]
    fn supported_cycles_filters_by_kind_and_eol() {
        // 上游每个产品的接口只返回自己的分支，测试也按类型分开给。
        let php_cycles = vec![
            Cycle {
                cycle: "8.6".into(),
                latest: "8.6.0".into(),
                eol: Some("2030-12-31".into()),
                lts: false,
            },
            Cycle {
                cycle: "8.2".into(),
                latest: "8.2.30".into(),
                eol: Some("2026-01-01".into()),
                lts: false,
            },
        ];
        let node_cycles = vec![
            Cycle {
                cycle: "24".into(),
                latest: "24.4.0".into(),
                eol: Some("2029-04-30".into()),
                lts: true,
            },
            Cycle {
                cycle: "25".into(),
                latest: "25.1.0".into(),
                eol: Some("2029-06-01".into()),
                lts: false,
            },
        ];
        let rust_cycles = vec![
            Cycle {
                cycle: "1.100".into(),
                latest: "1.100.0".into(),
                eol: None,
                lts: false,
            },
            Cycle {
                cycle: "1.99".into(),
                latest: "1.99.1".into(),
                eol: Some("2026-11-12".into()),
                lts: false,
            },
        ];
        let php = supported_cycles(&php_cycles, "php", "2026-11-25");
        assert_eq!(php.len(), 1, "已 EOL 的 8.2 必须被过滤");
        assert_eq!(php[0].latest, "8.6.0");

        let node = supported_cycles(&node_cycles, "node", "2026-11-25");
        assert_eq!(node.len(), 1, "非 LTS 的 25 不收");
        assert_eq!(node[0].latest, "24.4.0");

        let rust = supported_cycles(&rust_cycles, "rust", "2026-11-25");
        assert_eq!(rust.len(), 1, "只留未 EOL 的 stable");
        assert_eq!(rust[0].latest, "1.100.0");
    }

    #[test]
    fn desired_from_keeps_every_patch_and_shares_branch_eol() {
        let cycle = Cycle {
            cycle: "8.5".into(),
            latest: "8.5.13".into(),
            eol: Some("2030-12-31".into()),
            lts: false,
        };
        let versions = ["8.5.13", "8.5.12", "8.5.11"].map(str::to_string);
        let mut seen = BTreeSet::new();
        let mut skipped = Vec::new();
        let out = desired_from("php", &cycle, &versions, &mut seen, &mut skipped);

        assert_eq!(out.len(), 3, "三个补丁都要成条目");
        assert_eq!(out[0].version, "8.5.13");
        assert_eq!(out[0].image, "php:8.5.13-fpm-alpine");
        assert_eq!(out[2].version, "8.5.11");
        assert!(
            out.iter()
                .all(|item| item.eol.as_deref() == Some("2030-12-31")),
            "同一分支的补丁共用分支 EOL"
        );

        // 同一版本被两个分支返回时只写一次（否则后写的会覆盖先写的 image）。
        let again = desired_from("php", &cycle, &versions, &mut seen, &mut skipped);
        assert!(again.is_empty(), "已出现过的版本不该重复产出");
    }

    #[test]
    fn desired_from_rejects_versions_the_panel_cannot_parse() {
        let cycle = Cycle {
            cycle: "24".into(),
            latest: "24.22.3".into(),
            eol: None,
            lts: true,
        };
        let versions = ["24.22.3", "24.22.3-rc.1"].map(str::to_string);
        let mut seen = BTreeSet::new();
        let mut skipped = Vec::new();
        let out = desired_from("node", &cycle, &versions, &mut seen, &mut skipped);

        assert_eq!(out.len(), 1);
        assert!(
            skipped.iter().any(|item| item.contains("24.22.3-rc.1")),
            "被拒的版本要说明原因: {skipped:?}"
        );
    }
}
