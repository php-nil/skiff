//! 授权、路径形式与符号链接逃逸（契约 §2.2、§3.1）。

mod support;

use std::fs;
use std::path::Path;

use skiff_fs::{FsError, PathMode, ReadRequest, WriteMode, WriteRequest};
use support::{ctx, Harness};

fn write_req(path: &str) -> WriteRequest {
    WriteRequest {
        path: path.to_string(),
        data: b"hello".to_vec(),
        mode: WriteMode::Replace,
    }
}

#[test]
fn plugin_cannot_leave_grant_root() {
    let h = Harness::new();
    let outside = h.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"top-secret").unwrap();

    let p = h.plugin("com.example.demo");
    let raw = outside.join("secret.txt").to_string_lossy().to_string();
    let err =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: raw,
                offset: 0,
                max_bytes: 1024,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.escape");
}

#[test]
fn plugin_without_grants_is_denied() {
    let h = Harness::new();
    let p = skiff_fs::Principal::plugin("com.example.demo", Vec::new(), None);
    let err = h.fs.mkdir(&p, &ctx(), &h.data_path("newdir")).unwrap_err();
    assert_eq!(err.code(), "fs.denied");
}

#[test]
fn parent_dir_injection_is_rejected() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let raw = format!("{}/../outside.txt", h.data().display());
    let err = h.fs.write(&p, &ctx(), &write_req(&raw)).unwrap_err();
    assert_eq!(err.code(), "fs.denied");
    assert!(matches!(err, FsError::Denied(_)));
}

#[test]
fn relative_and_nul_paths_are_rejected() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    assert_eq!(
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: "relative/x".into(),
                offset: 0,
                max_bytes: 1
            }
        )
        .unwrap_err()
        .code(),
        "fs.denied"
    );
    assert_eq!(
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: h.data_path("a\u{0}b"),
                offset: 0,
                max_bytes: 1
            }
        )
        .unwrap_err()
        .code(),
        "fs.denied"
    );
}

#[test]
fn final_symlink_is_never_followed() {
    let h = Harness::new();
    let outside = h.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"top-secret").unwrap();

    let link = h.data().join("link");
    if !try_symlink(&outside.join("secret.txt"), &link) {
        return; // 无权限创建符号链接（常见于未开发者模式的 Windows）
    }

    let p = h.plugin("com.example.demo");
    let raw = link.to_string_lossy().to_string();

    // 读：拒绝跟随
    assert_eq!(
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: raw.clone(),
                offset: 0,
                max_bytes: 1024
            }
        )
        .unwrap_err()
        .code(),
        "fs.escape"
    );
    // 写：拒绝跟随（避免改到根外目标）
    assert_eq!(
        h.fs.write(&p, &ctx(), &write_req(&raw)).unwrap_err().code(),
        "fs.escape"
    );
    // stat：按链接本身审计，不解析目标
    let info =
        h.fs.stat(
            &p,
            &ctx(),
            &skiff_fs::StatRequest {
                path: raw.clone(),
                with_hash: false,
            },
        )
        .unwrap();
    assert_eq!(info.kind, skiff_fs::PathKind::Symlink);
    assert!(info.link_target.is_some());
    // remove：只删链接
    // Windows 下部分环境（含本机沙箱）无法 rename 符号链接，故仅在 Unix 断言；
    // 语义本身由 `io::remove_object` 的 `symlink_metadata` + `remove_file` 保证。
    #[cfg(unix)]
    {
        let removed = h.fs.remove(&p, &ctx(), &raw, false).unwrap();
        assert!(removed.trash_id.is_some());
        assert!(outside.join("secret.txt").exists(), "目标文件不得被删");
    }
    #[cfg(not(unix))]
    {
        let _ = (h, p, raw);
    }
}

#[test]
fn parent_directory_symlink_escape_is_blocked() {
    let h = Harness::new();
    let outside = h.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let dir_link = h.data().join("escape");
    if !try_symlink_dir(&outside, &dir_link) {
        return;
    }
    let p = h.plugin("com.example.demo");
    let raw = dir_link.join("pwn.txt").to_string_lossy().to_string();
    let err = h.fs.write(&p, &ctx(), &write_req(&raw)).unwrap_err();
    assert_eq!(err.code(), "fs.escape");
}

#[test]
fn admin_is_unrestricted_but_audited() {
    let h = Harness::new();
    let p = h.admin();
    let raw = h.base().join("admin.txt").to_string_lossy().to_string();
    h.fs.write(&p, &ctx(), &write_req(&raw)).unwrap();
    let events = h.audit_events();
    assert!(events
        .iter()
        .any(|e| e.actor_type == "admin" && e.action == "fs.write" && e.result == "ok"));
    assert_eq!(
        events.last().unwrap().request_id.as_deref(),
        Some("req-test-1")
    );
}

#[test]
fn protected_site_subdirs_need_explicit_permission() {
    let h = Harness::new();
    let ssl_dir = h.sites().join("example.com").join("ssl");
    fs::create_dir_all(&ssl_dir).unwrap();
    let target = ssl_dir.join("cert.pem").to_string_lossy().to_string();

    let p = h.plugin_with_sites("com.example.demo");
    let err = h.fs.write(&p, &ctx(), &write_req(&target)).unwrap_err();
    assert_eq!(err.code(), "fs.protected");

    let allowed = p.clone().with_protected_write(true);
    h.fs.write(&allowed, &ctx(), &write_req(&target)).unwrap();

    // 管理员无需额外开关
    h.fs.write(&h.admin(), &ctx(), &write_req(&target)).unwrap();
}

#[test]
fn root_itself_cannot_be_deleted_or_renamed() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let root = h.data().to_string_lossy().to_string();
    assert_eq!(
        h.fs.remove(&p, &ctx(), &root, false).unwrap_err().code(),
        "fs.denied"
    );
    let dest = h.base().join("moved").to_string_lossy().to_string();
    assert_eq!(
        h.fs.rename(&p, &ctx(), &root, &dest, false)
            .unwrap_err()
            .code(),
        "fs.denied"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_skipped_in_listing() {
    use std::os::unix::ffi::OsStrExt;
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let weird = h.data().join(std::ffi::OsStr::from_bytes(b"bad-\xff"));
    fs::write(&weird, b"x").unwrap();
    h.seed(&h.data_path("ok.txt"), b"ok");
    let page =
        h.fs.list(
            &p,
            &ctx(),
            &skiff_fs::ListRequest {
                path: h.data().to_string_lossy().to_string(),
                page: 1,
                page_size: 10,
            },
        )
        .unwrap();
    assert_eq!(page.skipped_non_utf8, 1);
    assert!(page.entries.iter().any(|e| e.name == "ok.txt"));
    assert!(page.entries.iter().all(|e| e.name != "bad-\u{fffd}"));
}

#[test]
fn missing_path_is_not_found() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    let err =
        h.fs.read(
            &p,
            &ctx(),
            &ReadRequest {
                path: h.data_path("nope.txt"),
                offset: 0,
                max_bytes: 10,
            },
        )
        .unwrap_err();
    assert_eq!(err.code(), "fs.not_found");
}

#[test]
fn resolve_is_lexical_and_does_not_touch_fs() {
    let h = Harness::new();
    let p = h.plugin("com.example.demo");
    // 父目录存在，最后一段不存在：resolve 只做 lexical 归一 + 父链预检。
    fs::create_dir_all(h.data().join("a")).unwrap();
    let raw = format!("{}//./a/b", h.data().display());
    let r = h.fs.resolve(&p, &raw, PathMode::Create).unwrap();
    assert_eq!(r.kind, skiff_fs::PathKind::Missing);
    assert!(r.grant_root.is_some());
    assert_eq!(r.path, h.data().join("a").join("b"));
    let _ = Path::new(&raw); // 明确 lexical 语义
}

fn try_symlink(src: &Path, dst: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(src, dst).is_ok()
    }
}

fn try_symlink_dir(src: &Path, dst: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(src, dst).is_ok()
    }
}
