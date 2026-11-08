//! 内核受保护前缀：删除/改名/覆盖写硬拒，其它 `/etc` 路径不受影响。

mod support;

use skiff_fs::{WriteMode, WriteRequest};
use support::{ctx, Harness};

#[test]
fn protected_prefix_blocks_delete_rename_and_overwrite() {
    let h = Harness::with_config_at(|base| vec![base.join("etc/skiff")], |_| {});
    let etc = h.base().join("etc");
    let config = etc.join("skiff/config.toml");
    let nginx = etc.join("nginx/nginx.conf");
    h.seed(&config.to_string_lossy(), b"cfg");
    h.seed(&nginx.to_string_lossy(), b"ngx");

    let admin = h.admin();
    let config_s = config.to_string_lossy().to_string();
    let nginx_s = nginx.to_string_lossy().to_string();

    let err = h.fs.remove(&admin, &ctx(), &config_s, false).unwrap_err();
    assert_eq!(err.code(), "fs.protected", "{}", err.message());

    let err =
        h.fs.rename(
            &admin,
            &ctx(),
            &config_s,
            &etc.join("skiff/config.bak").to_string_lossy(),
            false,
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.protected");

    let err =
        h.fs.write(
            &admin,
            &ctx(),
            &WriteRequest {
                path: config_s.clone(),
                data: b"pwned".to_vec(),
                mode: WriteMode::Replace,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.protected");

    // 前缀目录本身（整树删除）同样拒绝。
    let err =
        h.fs.remove(&admin, &ctx(), &etc.join("skiff").to_string_lossy(), false)
            .unwrap_err();
    assert_eq!(err.code(), "fs.protected");

    // 在受保护目录内新建文件同样拒绝，避免绕过覆盖写。
    let err =
        h.fs.write(
            &admin,
            &ctx(),
            &WriteRequest {
                path: etc.join("skiff/new.toml").to_string_lossy().to_string(),
                data: b"x".to_vec(),
                mode: WriteMode::Replace,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.protected");

    // 保护区外的 /etc 仍可正常删除。
    h.fs.remove(&admin, &ctx(), &nginx_s, false).unwrap();
    assert!(!nginx.exists());
}
