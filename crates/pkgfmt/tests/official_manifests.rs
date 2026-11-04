//! 官方插件 manifest 固化校验。
//!
//! `plugins/` 是**第二 workspace**，根三关（`cargo test --workspace`）不会编译它；
//! 官方 manifest 与内核 manifest 契约漂移时，只有等到打包（Linux 打包机/CI
//! `pkg-chain`）才会炸。这里借 `skiff-pkgfmt` 的同一份解析器把它们提前验一遍。

use std::path::{Path, PathBuf};

use skiff_pkgfmt::Manifest;

fn plugins_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

#[test]
fn official_manifests_parse_and_match_directory_ids() {
    let dir = plugins_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("读取官方插件目录失败 {}: {e}", dir.display()));
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let path = entry.expect("目录项读取失败").path();
        let manifest_path = path.join("manifest.toml");
        // `common/` 是共享库、不是插件包，没有 manifest。
        if !manifest_path.is_file() {
            continue;
        }
        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("插件目录名必须是 UTF-8")
            .to_string();
        let text = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("读取 {} 失败: {e}", manifest_path.display()));
        let m = Manifest::parse(&text)
            .unwrap_or_else(|e| panic!("{} 解析失败: {e}", manifest_path.display()));
        // 约定：目录名 = id 最后一段（`plugins/cron/` → `com.skiff.cron`）。
        assert_eq!(
            m.id,
            format!("com.skiff.{dir_name}"),
            "{} 的 id 必须与目录名一致",
            manifest_path.display()
        );
        // P2：官方插件全部改为声明式菜单，且声明引用的 ui 文件必须真实存在。
        assert!(
            !m.menu.is_empty(),
            "{} 必须声明至少一个 [[menu]]",
            manifest_path.display()
        );
        let ui_dir = path.join("ui");
        m.validate_menu_assets(|p| ui_dir.join(p).is_file())
            .unwrap_or_else(|e| panic!("{} 菜单/槽位引用的 ui 缺失: {e}", manifest_path.display()));
        if dir_name == "monitor" {
            assert!(
                m.mount.iter().any(|x| x.slot == "overview.health"),
                "monitor 必须声明 overview.health 槽位"
            );
        }
        ids.push(m.id);
    }
    ids.sort();
    assert!(ids.len() >= 4, "官方插件至少 4 个，实际只找到: {ids:?}");
    assert!(
        ids.iter().any(|id| id == "com.skiff.cron"),
        "计划任务插件缺失: {ids:?}"
    );
}
