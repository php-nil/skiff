//! 命令行入口：`gen` 生成静态产物，`check` 做发布前校验。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Result};
use skiff_server::build::{build, write};
use skiff_server::check;
use skiff_server::sign::Signer;

const HELP: &str = "\
Skiff-server P0：商店清单校验与静态产物生成

用法:
  skiff-server gen   [--catalog <dir>] [--schema <dir>] [--out <dir>] [--sign-key <hex>]
  skiff-server check [--catalog <dir>] [--schema <dir>] [--dist <dir>] [--prev <dir>] [--require-signature]

选项:
  --catalog <dir>       源数据目录，默认 deploy/skiff-server/catalog
  --schema <dir>        JSON Schema 目录，默认 deploy/skiff-server/schema
  --out <dir>           gen 的输出目录，默认 target/skiff-server/dist（生成前会清空）
  --sign-key <hex>      32 字节 Ed25519 种子的十六进制；缺省为未签名占位
  --dist <dir>          校验已生成的产物目录；缺省则先在内存生成再校验
  --prev <dir>          上一版产物目录，用于拒绝 seq 回退
  --require-signature   占位/空签名直接判失败（发布门禁用）

退出码: 0 通过，1 失败。
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") => gen(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("help" | "-h" | "--help") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => bail!("未知子命令: {other}\n\n{HELP}"),
    }
}

fn gen(args: &[String]) -> Result<()> {
    let (options, _) = parse(args, &[])?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let schema = path_option(&options, "--schema", "deploy/skiff-server/schema");
    let out = path_option(&options, "--out", "target/skiff-server/dist");
    let signer = Signer::from_seed_hex(options.get("--sign-key").map(String::as_str))?;

    let artifact = build(&catalog, &schema, &signer)?;
    write(&artifact, &out)?;
    println!("生成 {} 个文件 → {}", artifact.files.len(), out.display());
    if signer.is_unsigned() {
        println!("注意：当前为未签名占位产物，客户端必须拒绝；发布前必须接入签名流水线");
    }
    Ok(())
}

fn check_cmd(args: &[String]) -> Result<()> {
    let (options, flags) = parse(args, &["--require-signature"])?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let schema = path_option(&options, "--schema", "deploy/skiff-server/schema");
    let dist = options.get("--dist").map(Path::new);
    let prev = options.get("--prev").map(Path::new);
    check::run(
        &catalog,
        &schema,
        dist,
        prev,
        flags.contains("--require-signature"),
    )
}

fn path_option(options: &BTreeMap<String, String>, name: &str, default: &str) -> PathBuf {
    options
        .get(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn parse(args: &[String], flags: &[&str]) -> Result<(BTreeMap<String, String>, BTreeSet<String>)> {
    let mut options = BTreeMap::new();
    let mut seen_flags = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let Some(name) = arg.strip_prefix("--") else {
            bail!("无法识别的参数: {arg}");
        };
        if flags.contains(&name) {
            seen_flags.insert(name.to_string());
            index += 1;
            continue;
        }
        if let Some((key, value)) = name.split_once('=') {
            options.insert(format!("--{key}"), value.to_string());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("参数 --{name} 缺少取值"))?;
        options.insert(format!("--{name}"), value.clone());
        index += 2;
    }
    Ok((options, seen_flags))
}
