//! `runtimes` 子命令验收：上游版本表 → catalog 条目 → 清单 → 内置副本。
//!
//! 全部用假上游（不联网）：版本选择、EOL 过滤、默认版本转移、digest 复用与跳过、
//! 幂等、`--prune` 清理，都在这里固定下来。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::{json, Value};
use skiff_server::build::{build, sha256_hex};
use skiff_server::runtimes::{self, KindReport, Options, Report};
use skiff_server::sign::Signer;
use skiff_server::upstream::{Cycle, Upstream};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schema() -> PathBuf {
    repo_root().join("deploy/skiff-server/schema")
}

/// 假上游：版本表与 digest 都由用例给定，`fail` 里的镜像模拟仓库不可达。
#[derive(Default)]
struct Fake {
    cycles: BTreeMap<String, Vec<Cycle>>,
    /// `(kind, cycle)` → 该分支的版本列表；没给的走 trait 默认（只有 latest）。
    versions: BTreeMap<(String, String), Vec<String>>,
    fail: BTreeSet<String>,
    /// 这些 kind 的 `branch_versions` 直接报错，模拟上游小接口不可用。
    fail_versions: BTreeSet<String>,
}

impl Fake {
    fn with(mut self, kind: &str, cycles: Vec<Cycle>) -> Self {
        self.cycles.insert(kind.to_string(), cycles);
        self
    }

    fn with_versions(mut self, kind: &str, cycle: &str, versions: &[&str]) -> Self {
        self.versions.insert(
            (kind.to_string(), cycle.to_string()),
            versions.iter().map(|item| item.to_string()).collect(),
        );
        self
    }

    fn failing(mut self, image: &str) -> Self {
        self.fail.insert(image.to_string());
        self
    }

    fn with_versions_failing(mut self, kind: &str) -> Self {
        self.fail_versions.insert(kind.to_string());
        self
    }
}

impl Upstream for Fake {
    fn cycles(&self, kind: &str) -> Result<Vec<Cycle>> {
        Ok(self.cycles.get(kind).cloned().unwrap_or_default())
    }

    fn image_digest(&self, image: &str) -> Result<String> {
        if self.fail.contains(image) {
            bail!("模拟仓库不可达");
        }
        Ok(format!("sha256:{}", sha256_hex(image.as_bytes())))
    }

    fn branch_versions(&self, kind: &str, cycle: &Cycle, per_branch: usize) -> Result<Vec<String>> {
        if self.fail_versions.contains(kind) {
            bail!("模拟上游版本列表接口 500");
        }
        let versions = self
            .versions
            .get(&(kind.to_string(), cycle.cycle.clone()))
            .cloned()
            .unwrap_or_else(|| vec![cycle.latest.clone()]);
        Ok(versions.into_iter().take(per_branch).collect())
    }
}

fn cycle(name: &str, latest: &str, eol: Option<&str>, lts: bool) -> Cycle {
    Cycle {
        cycle: name.to_string(),
        latest: latest.to_string(),
        eol: eol.map(str::to_string),
        lts,
    }
}

fn setup_catalog(root: &Path) -> Result<PathBuf> {
    let channel = root.join("catalog/stable");
    fs::create_dir_all(channel.join("runtimes"))?;
    fs::write(
        channel.join("channel.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "skiff.server.channel/v1",
            "channel": "stable",
            "seq": 1,
            "generated_at": "2026-11-25T00:00:00Z"
        }))?,
    )?;
    Ok(root.join("catalog"))
}

fn options(catalog: &Path) -> Options {
    Options {
        channel: "stable".to_string(),
        catalog: catalog.to_path_buf(),
        schema: schema(),
        kinds: runtimes::all_kinds(),
        builtin: Some(catalog.join("../builtin")),
        out: None,
        today: "2026-11-25".to_string(),
        prune: false,
        dry_run: false,
        per_branch: 3,
    }
}

fn entry_of(catalog: &Path, name: &str) -> Result<Value> {
    let path = catalog.join("stable/runtimes").join(name);
    Ok(serde_json::from_slice(&fs::read(&path)?)?)
}

fn report_of<'a>(report: &'a Report, kind: &str) -> &'a KindReport {
    &report
        .kinds
        .iter()
        .find(|(name, _)| name == kind)
        .unwrap_or_else(|| panic!("缺 {kind} 的报告"))
        .1
}

fn upstream() -> Fake {
    Fake::default()
        .with(
            "php",
            vec![
                cycle("8.6", "8.6.0", Some("2030-12-31"), false),
                cycle("8.5", "8.5.13", Some("2030-12-31"), false),
                cycle("8.2", "8.2.30", Some("2026-01-01"), false),
            ],
        )
        .with(
            "node",
            vec![
                cycle("24", "24.4.0", Some("2029-04-30"), true),
                cycle("25", "25.1.0", Some("2029-06-01"), false),
            ],
        )
        .with(
            "rust",
            vec![
                cycle("1.100", "1.100.0", None, false),
                cycle("1.99", "1.99.1", Some("2026-11-12"), false),
            ],
        )
}

/// 仓库里现有的条目：PHP 一条大版本占位（`8.5`），默认版本指向它。
fn write_legacy_php_entry(catalog: &Path) -> Result<()> {
    fs::write(
        catalog.join("stable/runtimes/php-8.5.json"),
        serde_json::to_vec_pretty(&json!({
            "kind": "php",
            "version": "8.5",
            "image": "php:8.5-fpm-alpine",
            "image_digest": format!("sha256:{}", "bb".repeat(32)),
            "eol": null,
            "api_min": "1.0",
            "default": true
        }))?,
    )?;
    Ok(())
}

#[test]
fn refresh_writes_entries_index_and_builtin_copy() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let options = options(&catalog);

    let report = runtimes::run(&upstream(), &options)?;

    // 条目：PHP 取全部受支持分支，已 EOL 的 8.2 不进；Node 只 LTS；Rust 只 stable。
    let php = entry_of(&catalog, "php-8.6.0.json")?;
    assert_eq!(php["image"], json!("php:8.6.0-fpm-alpine"));
    assert_eq!(php["eol"], json!("2030-12-31"));
    assert_eq!(php["default"], json!(true), "最高版本接任默认");
    assert_eq!(php["api_min"], json!("1.0"), "沿用同类既有条目的 api_min");
    assert!(catalog.join("stable/runtimes/php-8.5.13.json").is_file());
    assert!(!catalog.join("stable/runtimes/php-8.2.30.json").exists());
    assert!(
        catalog.join("stable/runtimes/php-8.5.json").is_file(),
        "未 --prune 不删旧条目"
    );
    let legacy = entry_of(&catalog, "php-8.5.json")?;
    assert_eq!(legacy["default"], json!(false), "默认版本必须唯一");

    assert!(catalog.join("stable/runtimes/node-24.4.0.json").is_file());
    assert!(!catalog.join("stable/runtimes/node-25.1.0.json").exists());
    let node = entry_of(&catalog, "node-24.4.0.json")?;
    assert_eq!(node["image"], json!("node:24.4.0-alpine"));
    assert_eq!(node["default"], json!(true));

    assert!(catalog.join("stable/runtimes/rust-1.100.0.json").is_file());
    assert!(!catalog.join("stable/runtimes/rust-1.99.1.json").exists());
    assert_eq!(
        entry_of(&catalog, "rust-1.100.0.json")?["image"],
        json!("rust:1.100.0-slim")
    );

    // 清单：PHP 三条（8.6.0 / 8.5.13 / 旧占位 8.5），Node / Rust 各一条。
    assert_eq!(report_of(&report, "php").listed, Some(3));
    assert_eq!(report_of(&report, "node").listed, Some(1));
    assert_eq!(report_of(&report, "rust").listed, Some(1));
    assert_eq!(report_of(&report, "php").versions, vec!["8.6.0", "8.5.13"]);

    // 内置副本必须就是产物的那份字节，`check --builtin-index` 才不会红。
    let artifact = build(&catalog, &schema(), &Signer::Unsigned)?;
    assert_eq!(
        fs::read(catalog.join("../builtin/php.json"))?,
        artifact.files["channels/stable/runtime-index/php.json"],
        "内置副本必须与产物同字节"
    );
    assert!(report.builtin_written);
    Ok(())
}

#[test]
fn second_run_is_a_noop() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let options = options(&catalog);

    let first = runtimes::run(&upstream(), &options)?;
    assert!(first.builtin_written);
    let second = runtimes::run(&upstream(), &options)?;

    for kind in ["php", "node", "rust"] {
        let item = report_of(&second, kind);
        assert_eq!(item.added, 0, "{kind} 不该重复新增");
        assert_eq!(item.updated, 0, "{kind} 不该重复更新");
    }
    assert_eq!(
        report_of(&second, "php").unchanged,
        2,
        "两个目标版本都应命中既有文件"
    );
    assert!(!second.builtin_written, "字节没变不该重写内置副本");
    Ok(())
}

#[test]
fn prune_removes_entries_upstream_no_longer_supports() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let mut options = options(&catalog);
    options.prune = true;

    let report = runtimes::run(&upstream(), &options)?;

    assert!(!catalog.join("stable/runtimes/php-8.5.json").exists());
    assert_eq!(report.pruned, vec!["php-8.5.json".to_string()]);
    assert_eq!(report_of(&report, "php").listed, Some(2));
    Ok(())
}

#[test]
fn digest_failure_skips_version_and_moves_default() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    let options = options(&catalog);
    let upstream = upstream().failing("php:8.6.0-fpm-alpine");

    let report = runtimes::run(&upstream, &options)?;

    let php = report_of(&report, "php");
    assert!(
        php.skipped.iter().any(|item| item.starts_with("8.6.0")),
        "取不到 digest 的版本要写进跳过清单: {:?}",
        php.skipped
    );
    assert!(!catalog.join("stable/runtimes/php-8.6.0.json").exists());
    let fallback = entry_of(&catalog, "php-8.5.13.json")?;
    assert_eq!(
        fallback["default"],
        json!(true),
        "跳过后默认版本要落到能写的最高版本上"
    );
    assert_eq!(php.listed, Some(1));
    Ok(())
}

#[test]
fn dry_run_changes_nothing() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let mut options = options(&catalog);
    options.dry_run = true;

    let report = runtimes::run(&upstream(), &options)?;

    assert!(!catalog.join("stable/runtimes/php-8.6.0.json").exists());
    assert!(
        !catalog.join("../builtin/php.json").exists(),
        "演练不该创建内置副本"
    );
    assert!(
        !report.builtin_written,
        "演练只报告条目层面，不该声称写过内置副本"
    );
    assert_eq!(
        report_of(&report, "php").listed,
        None,
        "演练不 build，清单条目数未知"
    );
    assert_eq!(report_of(&report, "php").added, 2);
    Ok(())
}

#[test]
fn fails_before_writing_when_no_digest_can_be_resolved() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let mut options = options(&catalog);
    options.kinds = vec!["php".to_string()];
    // 两个目标版本都取不到摘要：必须整体失败，不能把旧条目的 default 摘掉。
    let upstream = Fake::default()
        .with(
            "php",
            vec![
                cycle("8.6", "8.6.0", Some("2030-12-31"), false),
                cycle("8.5", "8.5.13", Some("2030-12-31"), false),
            ],
        )
        .failing("php:8.6.0-fpm-alpine")
        .failing("php:8.5.13-fpm-alpine");

    let error = runtimes::run(&upstream, &options).expect_err("全部失败必须报错");
    assert!(
        format!("{error}").contains("都拿不到镜像摘要"),
        "错误要说清原因: {error}"
    );
    assert!(!catalog.join("stable/runtimes/php-8.6.0.json").exists());
    let legacy = entry_of(&catalog, "php-8.5.json")?;
    assert_eq!(legacy["default"], json!(true), "失败时旧条目一个字都不该动");
    Ok(())
}

#[test]
fn keeps_three_versions_per_branch() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    write_legacy_php_entry(&catalog)?;
    let options = options(&catalog);
    // 8.5 分支有三个补丁：老站点要能选到 8.5.12 / 8.5.11 再升级。
    let upstream = upstream().with_versions("php", "8.5", &["8.5.13", "8.5.12", "8.5.11"]);

    let report = runtimes::run(&upstream, &options)?;

    for version in ["8.5.13", "8.5.12", "8.5.11"] {
        let entry = entry_of(&catalog, &format!("php-{version}.json"))?;
        assert_eq!(entry["image"], json!(format!("php:{version}-fpm-alpine")));
        assert_eq!(entry["eol"], json!("2030-12-31"), "补丁共用分支 EOL");
        assert_eq!(
            entry["default"],
            json!(false),
            "{version} 不是最高版本，不该当默认"
        );
    }
    assert_eq!(
        entry_of(&catalog, "php-8.6.0.json")?["default"],
        json!(true)
    );
    assert_eq!(
        report_of(&report, "php").versions,
        vec!["8.6.0", "8.5.13", "8.5.12", "8.5.11"]
    );
    // 四条目标版本 + 未 prune 的旧占位 8.5。
    assert_eq!(report_of(&report, "php").listed, Some(5));
    Ok(())
}

#[test]
fn per_branch_one_keeps_only_the_latest() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    let mut options = options(&catalog);
    options.per_branch = 1;
    let upstream = upstream().with_versions("php", "8.5", &["8.5.13", "8.5.12", "8.5.11"]);

    runtimes::run(&upstream, &options)?;

    assert!(catalog.join("stable/runtimes/php-8.5.13.json").is_file());
    assert!(!catalog.join("stable/runtimes/php-8.5.12.json").exists());
    assert!(!catalog.join("stable/runtimes/php-8.5.11.json").exists());
    Ok(())
}

#[test]
fn branch_versions_failure_falls_back_to_latest() -> Result<()> {
    let root = tempfile::tempdir()?;
    let catalog = setup_catalog(root.path())?;
    let options = options(&catalog);
    let upstream = upstream().with_versions_failing("php");

    let report = runtimes::run(&upstream, &options)?;

    // 历史版本查不到不影响最新版落盘，但要留下原因，否则「只有一个版本」无从解释。
    assert!(catalog.join("stable/runtimes/php-8.6.0.json").is_file());
    assert!(catalog.join("stable/runtimes/php-8.5.13.json").is_file());
    let php = report_of(&report, "php");
    assert!(
        php.skipped
            .iter()
            .any(|item| item.starts_with("8.5") && item.contains("历史版本查询失败")),
        "降级要写进跳过清单: {:?}",
        php.skipped
    );
    assert_eq!(report_of(&report, "node").versions, vec!["24.4.0"]);
    Ok(())
}
