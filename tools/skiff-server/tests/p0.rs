//! P0 验收：schema 拒绝非法条目、摘要与引用校验、确定性生成、`seq` 回退被拒。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};
use skiff_server::build::{build, sha256_hex, write};
use skiff_server::check;
use skiff_server::schema::SchemaSet;
use skiff_server::sign::{manifest_signing_input, verify_detached, Signer};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn catalog() -> PathBuf {
    repo_root().join("deploy/skiff-server/catalog")
}

fn schema() -> PathBuf {
    repo_root().join("deploy/skiff-server/schema")
}

fn plugin_sample() -> Value {
    let text = fs::read_to_string(catalog().join("stable/plugins/com.example.hello.json"))
        .expect("读取插件样例失败");
    serde_json::from_str(&text).expect("插件样例不是合法 JSON")
}

#[test]
fn samples_pass_schema_and_semantics() -> Result<()> {
    check::run(&catalog(), &schema(), None, None, false)?;
    Ok(())
}

#[test]
fn schema_rejects_invalid_entries() -> Result<()> {
    let schemas = SchemaSet::new(schema());
    let base = plugin_sample();
    schemas.validate("plugin.schema.json", &base)?;

    let mut unknown = base.clone();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("surprise".to_string(), json!(1));
    let error = schemas
        .validate("plugin.schema.json", &unknown)
        .expect_err("未知字段必须被拒绝");
    assert!(error.to_string().contains("surprise"), "{error}");

    let mut bad_channel = base.clone();
    bad_channel["channel"] = json!("beta");
    assert!(schemas
        .validate("plugin.schema.json", &bad_channel)
        .is_err());

    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("summary");
    let error = schemas
        .validate("plugin.schema.json", &missing)
        .expect_err("缺必填字段必须被拒绝");
    assert!(error.to_string().contains("summary"), "{error}");

    let mut bad_type = base;
    bad_type["yanked"] = json!("no");
    assert!(schemas.validate("plugin.schema.json", &bad_type).is_err());
    Ok(())
}

#[test]
fn generated_index_contains_samples() -> Result<()> {
    let artifact = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let index: Value = serde_json::from_slice(artifact.get("channels/stable/index.json").unwrap())?;
    assert_eq!(index["schema"], json!("skiff.server.index/v1"));
    assert_eq!(index["plugins"].as_array().unwrap().len(), 1);
    assert_eq!(index["apps"].as_array().unwrap().len(), 1);
    assert_eq!(index["runtimes"].as_array().unwrap().len(), 1);
    assert_eq!(index["upgrades"].as_array().unwrap().len(), 1);
    assert!(artifact.get("channels/stable/manifest.json.sig").is_some());
    let blobs = artifact
        .files
        .keys()
        .filter(|path| path.starts_with("blobs/sha256/"))
        .count();
    assert_eq!(blobs, 3);
    Ok(())
}

#[test]
fn tampered_or_missing_blob_is_rejected() -> Result<()> {
    let artifact = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let dir = tempfile::tempdir()?;
    write(&artifact, dir.path())?;
    check::run(&catalog(), &schema(), Some(dir.path()), None, false)?;

    let blob = artifact
        .files
        .keys()
        .find(|path| path.starts_with("blobs/sha256/"))
        .unwrap()
        .clone();
    let path = dir.path().join(&blob);

    let mut bytes = fs::read(&path)?;
    bytes.push(b'x');
    fs::write(&path, &bytes)?;
    let error = check::run(&catalog(), &schema(), Some(dir.path()), None, false)
        .expect_err("篡改 blob 必须被发现");
    assert!(
        error.to_string().contains("摘要") || error.to_string().contains("大小"),
        "{error}"
    );

    fs::remove_file(&path)?;
    let error = check::run(&catalog(), &schema(), Some(dir.path()), None, false)
        .expect_err("缺失 blob 必须被发现");
    assert!(error.to_string().contains("不存在"), "{error}");
    Ok(())
}

#[test]
fn generation_is_deterministic() -> Result<()> {
    let first = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let second = build(&catalog(), &schema(), &Signer::Unsigned)?;
    assert_eq!(first.files, second.files);

    // 固定测试种子，只用于验证签名接口的确定性，不是发布密钥。
    let seed = "11".repeat(32);
    let signer_a = Signer::from_seed_hex(Some(&seed))?;
    let signer_b = Signer::from_seed_hex(Some(&seed))?;
    let signed_a = build(&catalog(), &schema(), &signer_a)?;
    let signed_b = build(&catalog(), &schema(), &signer_b)?;
    assert_eq!(signed_a.files, signed_b.files);
    assert_ne!(first.files, signed_a.files);
    Ok(())
}

#[test]
fn seq_regression_is_rejected() -> Result<()> {
    let artifact = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let current = tempfile::tempdir()?;
    let previous = tempfile::tempdir()?;
    write(&artifact, current.path())?;
    write(&artifact, previous.path())?;

    let manifest_path = previous.path().join("channels/stable/manifest.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    manifest["seq"] = json!(2);
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    let error = check::run(
        &catalog(),
        &schema(),
        Some(current.path()),
        Some(previous.path()),
        false,
    )
    .expect_err("seq 回退必须被拒绝");
    assert!(error.to_string().contains("seq"), "{error}");
    Ok(())
}

#[test]
fn unsigned_artifact_fails_strict_mode() -> Result<()> {
    let artifact = build(&catalog(), &schema(), &Signer::Unsigned)?;
    let dir = tempfile::tempdir()?;
    write(&artifact, dir.path())?;
    let error = check::run(&catalog(), &schema(), Some(dir.path()), None, true)
        .expect_err("未签名产物在发布门禁下必须失败");
    assert!(error.to_string().contains("未签名"), "{error}");
    Ok(())
}

#[test]
fn signing_interface_produces_verifiable_signature() -> Result<()> {
    let seed = "22".repeat(32);
    let signer = Signer::from_seed_hex(Some(&seed))?;
    let artifact = build(&catalog(), &schema(), &signer)?;
    let manifest = artifact
        .get("channels/stable/manifest.json")
        .expect("缺少 manifest");
    let signature: Value =
        serde_json::from_slice(artifact.get("channels/stable/manifest.json.sig").unwrap())?;
    let signature_hex = signature["sig"].as_str().unwrap();
    assert_eq!(signature_hex.len(), 128);

    let public_key = signer.public_key_hex().unwrap();
    verify_detached(
        &public_key,
        &manifest_signing_input(manifest),
        &hex::decode(signature_hex)?,
    )?;

    let mut tampered = manifest.to_vec();
    tampered.push(b' ');
    assert!(verify_detached(
        &public_key,
        &manifest_signing_input(&tampered),
        &hex::decode(signature_hex)?,
    )
    .is_err());

    // 摘要派生自 asset，换字节必然换摘要。
    let blob = artifact
        .files
        .keys()
        .find(|path| path.starts_with("blobs/sha256/"))
        .unwrap();
    assert_eq!(
        sha256_hex(artifact.get(blob).unwrap()),
        blob.rsplit('/').next().unwrap()
    );
    Ok(())
}
