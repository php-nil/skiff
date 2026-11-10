//! 旗标生效验收：`--unsigned` / `--require-signature` 必须真正改变命令行为，
//! 而不只是「能被解析」。此前 `parse` 用去前缀后的名字与带 `--` 的旗标表比较，
//! 两边永远不相等，旗标被当成缺值的普通选项。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::Result;
use serde_json::{json, Value};
use skiff_server::build::{build, write};
use skiff_server::sign::Signer;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn catalog() -> PathBuf {
    repo_root().join("deploy/skiff-server/catalog")
}

fn schema() -> PathBuf {
    repo_root().join("deploy/skiff-server/schema")
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_skiff-server")
}

/// 以可控环境执行子命令：默认清掉签名与公钥变量，避免宿主环境影响断言。
fn run(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(bin());
    command.args(args);
    command.env_remove("SKIFF_SIGNING_KEY");
    command.env_remove("SKIFF_SERVER_PUBLIC_KEY");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("执行 skiff-server 失败")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn unsigned_manifest(out_dir: &Path) -> Result<Value> {
    let bytes = fs::read(out_dir.join("channels/stable/manifest.json"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[test]
fn gen_unsigned_flag_writes_placeholder_without_key() -> Result<()> {
    let out = tempfile::tempdir()?;
    let dist = out.path().join("dist");
    let output = run(
        &[
            "gen",
            "--unsigned",
            "--catalog",
            catalog().to_str().unwrap(),
            "--schema",
            schema().to_str().unwrap(),
            "--out",
            dist.to_str().unwrap(),
        ],
        &[],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let manifest = unsigned_manifest(&dist)?;
    assert_eq!(manifest["key_id"], json!("0".repeat(64)));
    let signature: Value =
        serde_json::from_slice(&fs::read(dist.join("channels/stable/manifest.json.sig"))?)?;
    assert_eq!(signature["sig"], json!(""), "占位产物必须没有签名");
    Ok(())
}

#[test]
fn unsigned_flag_wins_even_with_signing_key() -> Result<()> {
    let out = tempfile::tempdir()?;
    let dist = out.path().join("dist");
    let seed = "66".repeat(32);
    let env = [("SKIFF_SIGNING_KEY", seed.as_str())];
    let output = run(
        &[
            "gen",
            "--unsigned",
            "--catalog",
            catalog().to_str().unwrap(),
            "--schema",
            schema().to_str().unwrap(),
            "--out",
            dist.to_str().unwrap(),
        ],
        &env,
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let manifest = unsigned_manifest(&dist)?;
    assert_eq!(
        manifest["key_id"],
        json!("0".repeat(64)),
        "显式 --unsigned 必须压过环境里的私钥"
    );
    Ok(())
}

#[test]
fn gen_without_unsigned_or_key_fails_clearly() -> Result<()> {
    let out = tempfile::tempdir()?;
    let output = run(
        &[
            "gen",
            "--catalog",
            catalog().to_str().unwrap(),
            "--schema",
            schema().to_str().unwrap(),
            "--out",
            out.path().join("dist").to_str().unwrap(),
        ],
        &[],
    );
    assert!(!output.status.success(), "缺密钥的发布路径必须失败");
    assert!(
        stderr(&output).contains("SKIFF_SIGNING_KEY"),
        "错误必须点名环境变量: {}",
        stderr(&output)
    );
    Ok(())
}

#[test]
fn require_signature_flag_rejects_unsigned_dist() -> Result<()> {
    let artifact = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let dist = tempfile::tempdir()?;
    write(&artifact, dist.path())?;

    let catalog_dir = catalog();
    let schema_dir = schema();
    let base = [
        "check",
        "--catalog",
        catalog_dir.to_str().unwrap(),
        "--schema",
        schema_dir.to_str().unwrap(),
        "--dist",
        dist.path().to_str().unwrap(),
    ];

    // 不带旗标：未签名产物允许通过（仅警告）。
    let lax = run(&base, &[]);
    assert!(lax.status.success(), "stderr: {}", stderr(&lax));

    // 带旗标：同一份产物必须失败，证明旗标真的生效。
    let mut strict = base.to_vec();
    strict.push("--require-signature");
    let strict = run(&strict, &[]);
    assert!(!strict.status.success(), "带 --require-signature 必须失败");
    assert!(
        stderr(&strict).contains("未签名"),
        "错误必须说明未签名: {}",
        stderr(&strict)
    );
    Ok(())
}
