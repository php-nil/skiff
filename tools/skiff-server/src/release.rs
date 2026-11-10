//! 发布路径：生成 → 签名 → 严格自检 → 落盘。
//!
//! 单条命令保证三件事：私钥来自环境变量、产物必须真的可验签、`seq` 不回退。
//! 全部校验都在写盘之前完成，任何失败都不会留下一份「看起来正常但不可发布」的目录。
//! 上传步骤因托管方案未定（`docs/SKIFF_SERVER.md` §15），由运维按文档执行。

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::build::{build, write, Artifact};
use crate::check;
use crate::schema::SchemaSet;
use crate::sign::Signer;

/// 生成签名产物、内存严格自检、通过后落盘。
///
/// `prev` 是上一版产物目录；提供时必须 `seq` 严格递增。`signer` 不能是占位，
/// 且其公钥必须与 `public_key` 完全一致——否则会产出一份客户端无法验证的清单。
pub fn publish(
    catalog_dir: &Path,
    schema_dir: &Path,
    out_dir: &Path,
    prev: Option<&Path>,
    signer: &Signer,
    public_key: &str,
) -> Result<Artifact> {
    let actual = signer
        .public_key_hex()
        .ok_or_else(|| anyhow::anyhow!("发布签名器不能是未签名占位"))?;
    if actual != public_key {
        bail!("签名公钥 {actual} 与发布公钥 {public_key} 不一致：客户端会拒绝该清单");
    }

    let artifact = build(catalog_dir, schema_dir, signer)?;
    let schemas = SchemaSet::new(schema_dir);
    check::validate_artifact_with(&artifact.files, &schemas, true, Some(public_key), false)
        .context("发布自检失败（验签/摘要/结构）：已中止，输出目录未改动")?;
    if let Some(prev) = prev {
        check::validate_seq_monotonic(&artifact.files, prev)
            .context("发布自检失败（seq 未递增）：已中止，输出目录未改动")?;
    }

    write(&artifact, out_dir)?;
    Ok(artifact)
}
