//! 镜像同步验收：缺失 blob 才传、seq 单调、同步后自检、失败步骤可定位。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};
use skiff_server::build::{build, sha256_hex, write};
use skiff_server::sign::{manifest_signing_input, Signer};
use skiff_server::sync::{self, DirSource};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn catalog() -> PathBuf {
    repo_root().join("deploy/skiff-server/catalog")
}

fn schema() -> PathBuf {
    repo_root().join("deploy/skiff-server/schema")
}

fn signer() -> Result<(Signer, String)> {
    // 固定测试种子，只用于验收签名链路，不是发布密钥。
    let signer = Signer::from_seed_hex(Some(&"44".repeat(32)))?;
    let public_key = signer.public_key_hex().unwrap();
    Ok((signer, public_key))
}

fn publish_primary(dir: &Path, signer: &Signer) -> Result<()> {
    let artifact = build(&catalog(), &schema(), signer)?;
    write(&artifact, dir)?;
    Ok(())
}

/// 在已有主站目录上追加一个插件与 blob，并把 seq 递增到 2（重新签名）。
fn bump_release(primary: &Path, signer: &Signer) -> Result<Vec<u8>> {
    let index_path = primary.join("channels/stable/index.json");
    let mut index: Value = serde_json::from_slice(&fs::read(&index_path)?)?;
    let asset = b"second package payload".to_vec();
    let sha = sha256_hex(&asset);
    let mut entry = index["plugins"][0].clone();
    entry["id"] = json!("com.example.hello2");
    entry["version"] = json!("1.0.0");
    entry["pkg_sha256"] = json!(sha);
    entry["pkg_url"] = json!(format!("blobs/sha256/{sha}"));
    entry["size"] = json!(asset.len());
    index["plugins"].as_array_mut().unwrap().push(entry);
    index["seq"] = json!(2);
    index["generated_at"] = json!("2026-11-11T00:00:00Z");
    let index_bytes = serde_json::to_vec(&index)?;
    fs::write(&index_path, &index_bytes)?;
    fs::write(primary.join("blobs/sha256").join(&sha), &asset)?;

    let manifest_path = primary.join("channels/stable/manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest["seq"] = json!(2);
    manifest["generated_at"] = json!("2026-11-11T00:00:00Z");
    manifest["index"]["sha256"] = json!(sha256_hex(&index_bytes));
    manifest["index"]["size"] = json!(index_bytes.len());
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    fs::write(&manifest_path, &manifest_bytes)?;

    let signature = signer
        .sign(&manifest_signing_input(&manifest_bytes))
        .unwrap();
    fs::write(
        primary.join("channels/stable/manifest.json.sig"),
        serde_json::to_vec(&json!({
            "alg": "ed25519",
            "key_id": signer.key_id(),
            "sig": hex::encode(signature),
        }))?,
    )?;
    Ok(asset)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    sync::copy_tree(from, to)
}

#[test]
fn sync_copies_missing_blobs_and_passes_self_check() -> Result<()> {
    let (signer, public_key) = signer()?;
    let primary = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;

    let report = sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;
    assert_eq!(report.blobs_total, 3);
    assert_eq!(report.blobs_fetched, 3);
    assert_eq!(report.blobs_reused, 0);
    assert!(report.manifest_written);
    for path in [
        "channels/stable/manifest.json",
        "channels/stable/manifest.json.sig",
        "channels/stable/index.json",
        "schema/manifest.schema.json",
    ] {
        assert!(mirror.path().join(path).is_file(), "缺少 {path}");
    }
    assert!(mirror.path().join("blobs/sha256").read_dir()?.count() >= 3);

    // 再同步一次：全部复用、清单未变化。
    let again = sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;
    assert_eq!(again.blobs_fetched, 0);
    assert_eq!(again.blobs_reused, 3);
    assert!(!again.manifest_written);
    Ok(())
}

#[test]
fn resync_fetches_only_the_new_blob() -> Result<()> {
    let (signer, public_key) = signer()?;
    let primary = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;
    sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;
    let old_blobs = mirror.path().join("blobs/sha256").read_dir()?.count();

    let asset = bump_release(primary.path(), &signer)?;
    let report = sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;
    assert_eq!(report.blobs_total, 4);
    assert_eq!(report.blobs_reused, 3);
    assert_eq!(report.blobs_fetched, 1);
    assert!(report.manifest_written);
    let new_blob = mirror.path().join("blobs/sha256").join(sha256_hex(&asset));
    assert!(new_blob.is_file(), "新 blob 必须落镜像");
    assert!(
        mirror.path().join("blobs/sha256").read_dir()?.count() > old_blobs,
        "旧 blob 必须保留"
    );
    Ok(())
}

#[test]
fn seq_rollback_is_rejected() -> Result<()> {
    let (signer, public_key) = signer()?;
    let primary = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;
    bump_release(primary.path(), &signer)?;
    sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;

    // 用旧版（seq=1）覆盖主站，同步必须拒绝回退。
    let old = tempfile::tempdir()?;
    publish_primary(old.path(), &signer)?;
    let error = sync::run(
        &DirSource::new(old.path()),
        "stable",
        mirror.path(),
        &public_key,
    )
    .expect_err("seq 回退必须被拒绝");
    assert!(error.to_string().contains("seq 回退"), "{error:#}");
    Ok(())
}

#[test]
fn same_seq_with_different_content_is_rejected() -> Result<()> {
    let (signer, public_key) = signer()?;
    let primary = tempfile::tempdir()?;
    let variant = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;
    copy_dir(primary.path(), variant.path())?;

    // 同 seq、不同 generated_at，并重新签名（签名本身有效）。
    let manifest_path = variant.path().join("channels/stable/manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    manifest["generated_at"] = json!("2026-11-10T08:00:00Z");
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    fs::write(&manifest_path, &manifest_bytes)?;
    let signature = signer
        .sign(&manifest_signing_input(&manifest_bytes))
        .unwrap();
    fs::write(
        variant.path().join("channels/stable/manifest.json.sig"),
        serde_json::to_vec(&json!({
            "alg": "ed25519",
            "key_id": signer.key_id(),
            "sig": hex::encode(signature),
        }))?,
    )?;

    sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )?;
    let error = sync::run(
        &DirSource::new(variant.path()),
        "stable",
        mirror.path(),
        &public_key,
    )
    .expect_err("同 seq 不同内容必须被拒绝");
    assert!(error.to_string().contains("同一 seq"), "{error:#}");
    Ok(())
}

#[test]
fn wrong_public_key_is_rejected_before_blobs() -> Result<()> {
    let (signer, _) = signer()?;
    let primary = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;
    let other = Signer::from_seed_hex(Some(&"55".repeat(32)))?
        .public_key_hex()
        .unwrap();

    let error = sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &other,
    )
    .expect_err("签名不匹配必须失败");
    assert!(error.to_string().contains("步骤 2/6"), "{error:#}");
    assert!(
        !mirror.path().join("channels/stable/manifest.json").exists(),
        "验签失败不得写入清单"
    );
    Ok(())
}

#[test]
fn tampered_blob_fails_with_step_and_leaves_no_manifest() -> Result<()> {
    let (signer, public_key) = signer()?;
    let primary = tempfile::tempdir()?;
    let mirror = tempfile::tempdir()?;
    publish_primary(primary.path(), &signer)?;

    // 篡改主站 blob（保持长度），让大小校验通过、摘要校验失败。
    let blob = primary
        .path()
        .join("blobs/sha256")
        .read_dir()?
        .next()
        .unwrap()?
        .path();
    let mut bytes = fs::read(&blob)?;
    bytes[0] ^= 0xff;
    fs::write(&blob, bytes)?;

    let error = sync::run(
        &DirSource::new(primary.path()),
        "stable",
        mirror.path(),
        &public_key,
    )
    .expect_err("摘要不符必须失败");
    assert!(error.to_string().contains("步骤 4/6"), "{error:#}");
    assert!(error.to_string().contains("摘要不符"), "{error:#}");
    assert!(
        !mirror.path().join("channels/stable/manifest.json").exists(),
        "blob 未通过校验不得更新清单"
    );
    Ok(())
}
