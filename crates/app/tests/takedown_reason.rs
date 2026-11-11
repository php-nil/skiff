//! R1 端到端：源数据里的撤包原因 → `skiff-server` 构建产物 → 客户端视图。
//!
//! 这里刻意用真实构建器而不是手写索引，确保「新增可选字段」在整条链路上都成立：
//! 旧索引不出现该字段，新索引带上原因，客户端解析与安装提示都能读到。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use skiff_app::catalog::fetch::{Conditional, HttpReply, Transport};
use skiff_app::catalog::model::Index;
use skiff_app::catalog::{CatalogService, CatalogSettings};
use skiff_server::build::build;
use skiff_server::sign::Signer;

const PLUGIN_ID: &str = "com.example.demo";
const VERSION: &str = "1.0.0";
const REASON: &str = "policy violation";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stable = dir.path().join("catalog/stable");
        for sub in ["plugins", "apps", "runtimes", "upgrades"] {
            fs::create_dir_all(stable.join(sub)).unwrap();
        }
        fs::create_dir_all(dir.path().join("catalog/assets")).unwrap();
        fs::write(
            stable.join("channel.json"),
            br#"{
  "schema": "skiff.server.channel/v1",
  "channel": "stable",
  "seq": 1,
  "generated_at": "2026-11-11T00:00:00Z"
}
"#,
        )
        .unwrap();
        fs::write(
            stable.join("plugins/com.example.demo.json"),
            format!(
                r#"{{
  "id": "{PLUGIN_ID}",
  "version": "{VERSION}",
  "api": "1.0",
  "channel": "stable",
  "key_id": "{}",
  "requires": ["docker"],
  "summary": "示例插件",
  "published_at": "2026-11-09T00:00:00Z",
  "yanked": true,
  "yank_reason": "{REASON}",
  "asset": "assets/demo.wbpkg"
}}
"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        fs::write(dir.path().join("catalog/assets/demo.wbpkg"), b"demo").unwrap();
        Self { dir }
    }

    fn schema(&self) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/skiff-server/schema")
    }

    fn artifact(&self) -> HashMap<String, Vec<u8>> {
        let artifact = build(
            &self.dir.path().join("catalog"),
            &self.schema(),
            &Signer::Unsigned,
        )
        .expect("源数据必须能构建");
        artifact.files.into_iter().collect()
    }
}

/// 假传输层：把构建产物按镜像 URL 原样返回，不触网。
struct FakeTransport {
    files: HashMap<String, Vec<u8>>,
}

impl Transport for FakeTransport {
    fn get(&self, url: &str, _cond: &Conditional<'_>) -> Result<HttpReply, String> {
        let body = self
            .files
            .get(url)
            .ok_or_else(|| format!("missing fixture: {url}"))?
            .clone();
        Ok(HttpReply {
            status: 200,
            body,
            etag: None,
            last_modified: None,
        })
    }
}

#[test]
fn takedown_reason_is_visible_from_source_to_client_view() {
    let fx = Fixture::new();
    let files = fx.artifact();
    let index_bytes = files
        .get("channels/stable/index.json")
        .expect("构建产物必须包含索引");
    let index: Index = serde_json::from_slice(index_bytes).expect("客户端必须能解析新索引");
    assert!(index.plugins[0].yanked);
    assert_eq!(index.plugins[0].yank_reason.as_deref(), Some(REASON));

    let mut mirrored = HashMap::new();
    for (path, bytes) in &files {
        mirrored.insert(format!("https://mirror.test/{path}"), bytes.clone());
    }
    let settings = CatalogSettings {
        enabled: true,
        channel: "stable".to_string(),
        mirrors: vec!["https://mirror.test".to_string()],
        allow_unsigned: true,
        timeout: Duration::from_secs(5),
        ..CatalogSettings::default()
    };
    let cache = fx.dir.path().join("cache");
    let service = CatalogService::with_transport(
        settings,
        cache,
        Arc::new(FakeTransport { files: mirrored }),
    );

    let snapshot = service.snapshot(false);
    assert!(
        !matches!(snapshot.state, skiff_app::catalog::CatalogState::Rejected),
        "客户端拒绝索引: {}",
        snapshot.message
    );
    let package = service
        .plugin_package(PLUGIN_ID, Some(VERSION))
        .unwrap_or_else(|| panic!("客户端找不到条目: {}", snapshot.message));
    assert!(package.entry.yanked);
    assert_eq!(package.entry.yank_reason.as_deref(), Some(REASON));

    let note = service
        .cached_takedown(PLUGIN_ID, VERSION)
        .expect("已安装视图必须给出撤包提示");
    assert!(note.contains(REASON), "客户端视图必须显示撤包原因: {note}");
}
