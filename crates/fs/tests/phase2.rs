//! 阶段二：`chmod` / `chown` / `archive` / `extract` 回归。
//!
//! 重点覆盖安全闸：setuid 拒绝、受保护路径、绝对路径与 `..` 穿越、符号链接
//! 不解出、条目数/单条体积/累计字节超限后的中止与半成品清理。

mod support;

use std::fs;
use std::io::Write;
use std::path::Path;

use skiff_fs::{ArchiveFormat, ArchiveRequest, ExtractRequest, FsError};
use support::{ctx, Harness};

fn zip_file(path: &str, entries: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        zip.start_file(name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn tar_gz_file(
    path: &str,
    build: impl FnOnce(&mut tar::Builder<flate2::write::GzEncoder<fs::File>>),
) {
    let file = fs::File::create(path).unwrap();
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    build(&mut tar);
    tar.into_inner().unwrap().finish().unwrap();
}

fn tar_regular(
    tar: &mut tar::Builder<flate2::write::GzEncoder<fs::File>>,
    name: &str,
    data: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    header.set_cksum();
    tar.append_data(&mut header, name, data).unwrap();
}

fn count_files(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            n += count_files(&entry.path());
        } else {
            n += 1;
        }
    }
    n
}

#[test]
fn outcomes_serialize_with_stable_field_names() {
    let copy = serde_json::to_value(skiff_fs::CopyOutcome {
        from: "/a".into(),
        to: "/b".into(),
        bytes: 3,
        entries: 2,
    })
    .unwrap();
    assert_eq!(copy["from"], "/a");
    assert_eq!(copy["to"], "/b");
    assert_eq!(copy["bytes"], 3);
    assert_eq!(copy["entries"], 2);

    let rename = serde_json::to_value(skiff_fs::RenameOutcome {
        from: "/a".into(),
        to: "/b".into(),
    })
    .unwrap();
    assert_eq!(rename["from"], "/a");
    assert_eq!(rename["to"], "/b");

    let archive = serde_json::to_value(skiff_fs::ArchiveOutcome {
        path: "/out.zip".into(),
        format: "zip".into(),
        entries: 4,
        bytes: 99,
        skipped_symlinks: 1,
        skipped_special: 0,
    })
    .unwrap();
    assert_eq!(archive["path"], "/out.zip");
    assert_eq!(archive["format"], "zip");
    assert_eq!(archive["skipped_symlinks"], 1);

    let extract = serde_json::to_value(skiff_fs::ExtractOutcome {
        dest: "/out".into(),
        entries: 5,
        files: 3,
        bytes: 42,
        skipped_symlinks: 1,
        skipped_special: 2,
    })
    .unwrap();
    assert_eq!(extract["dest"], "/out");
    assert_eq!(extract["files"], 3);
    assert_eq!(extract["skipped_special"], 2);
}

#[test]
fn chmod_rejects_setuid_with_readable_reason() {
    let h = Harness::new();
    let p = h.admin();
    let path = h.data_path("bin/tool");
    h.seed(&path, b"x");

    let err = h.fs.chmod(&p, &ctx(), &path, 0o4755).unwrap_err();
    match err {
        FsError::Denied(msg) => assert!(msg.contains("setuid"), "原因应包含 setuid: {msg}"),
        other => panic!("应为 Denied，实际 {other:?}"),
    }
}

/// setuid 必须是**按位**判定：0o6000（setuid+setgid）与 0o7777（全位）
/// 都包含 setuid，不能只有「等于 0o4000」才拒绝。
#[test]
fn chmod_rejects_any_setuid_bits() {
    let h = Harness::new();
    let p = h.admin();
    let path = h.data_path("bin/tool");
    h.seed(&path, b"x");

    for mode in [0o6000, 0o7777] {
        let err = h.fs.chmod(&p, &ctx(), &path, mode).unwrap_err();
        match err {
            FsError::Denied(msg) => {
                assert!(
                    msg.contains("setuid"),
                    "0o{mode:04o} 原因应包含 setuid: {msg}"
                )
            }
            other => panic!("0o{mode:04o} 应为 Denied，实际 {other:?}"),
        }
    }
}

#[test]
fn chmod_rejects_out_of_range_bits() {
    let h = Harness::new();
    let p = h.admin();
    let path = h.data_path("a.txt");
    h.seed(&path, b"x");
    let err = h.fs.chmod(&p, &ctx(), &path, 0o17777).unwrap_err();
    assert!(matches!(err, FsError::Denied(_)));
}

#[cfg(unix)]
#[test]
fn chmod_allows_setgid_and_chown_self() {
    use std::os::unix::fs::PermissionsExt;
    let h = Harness::new();
    let p = h.admin();
    let dir = h.data_path("shared");
    fs::create_dir_all(&dir).unwrap();

    h.fs.chmod(&p, &ctx(), &dir, 0o2775).unwrap();
    let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o2775);

    // sticky 位（1777）不含 setuid，必须放行。
    h.fs.chmod(&p, &ctx(), &dir, 0o1777).unwrap();
    let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o1777);

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let out = h.fs.chown(&p, &ctx(), &dir, uid, gid).unwrap();
    assert!(!out.root_owned);
    let events = h.audit_events();
    let ev = events
        .iter()
        .find(|e| e.action == "fs.chown")
        .expect("必须有 fs.chown 审计");
    assert!(ev
        .detail
        .as_deref()
        .unwrap_or("")
        .contains(&format!("{uid}:{gid}")));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn chown_to_root_marks_root_owner_in_audit_when_permitted() {
    if unsafe { libc::getuid() } != 0 {
        return;
    }
    let h = Harness::new();
    let p = h.admin();
    let path = h.data_path("owned.txt");
    h.seed(&path, b"x");
    let out = h.fs.chown(&p, &ctx(), &path, 0, 0).unwrap();
    assert!(out.root_owned);
    let ev = h
        .audit_events()
        .into_iter()
        .find(|e| e.action == "fs.chown")
        .expect("必须有 fs.chown 审计");
    assert!(ev
        .detail
        .as_deref()
        .unwrap_or("")
        .contains("root_owner=true"));
}

#[test]
fn chmod_chown_reject_protected_paths() {
    let h = Harness::with_config_at(
        |base| vec![base.join("protected")],
        |cfg| {
            let _ = cfg;
        },
    );
    let p = h.admin();
    let dir = h.base().join("protected");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("x.txt");
    fs::write(&path, b"x").unwrap();
    let raw = path.to_string_lossy().to_string();

    let err = h.fs.chmod(&p, &ctx(), &raw, 0o644).unwrap_err();
    assert!(matches!(err, FsError::Protected(_)));
    let err = h.fs.chown(&p, &ctx(), &raw, 1000, 1000).unwrap_err();
    assert!(matches!(err, FsError::Protected(_)));
}

#[test]
fn archive_zip_and_tar_gz_roundtrip() {
    let h = Harness::new();
    let p = h.admin();
    h.seed(&h.data_path("src/a.txt"), b"alpha");
    h.seed(&h.data_path("src/sub/b.txt"), b"beta");
    h.seed(&h.data_path("single.txt"), b"gamma");

    for (format, name) in [
        (ArchiveFormat::Zip, "out.zip"),
        (ArchiveFormat::TarGz, "out.tar.gz"),
    ] {
        let dest = h.data_path(name);
        let out =
            h.fs.archive(
                &p,
                &ctx(),
                &ArchiveRequest {
                    sources: vec![h.data_path("src"), h.data_path("single.txt")],
                    dest: dest.clone(),
                    format,
                    overwrite: false,
                },
            )
            .unwrap();
        assert!(out.bytes > 0);
        assert_eq!(out.entries, 5, "src/、a.txt、sub/、b.txt、single.txt");

        let unzip = h.data_path(&format!("unzip-{}", format.as_str().replace('.', "-")));
        fs::create_dir_all(&unzip).unwrap();
        let ex =
            h.fs.extract(
                &p,
                &ctx(),
                &ExtractRequest {
                    archive: dest,
                    dest_dir: unzip.clone(),
                    overwrite: false,
                },
            )
            .unwrap();
        assert_eq!(ex.files, 3);
        assert_eq!(
            fs::read(Path::new(&unzip).join("src/a.txt")).unwrap(),
            b"alpha"
        );
        assert_eq!(
            fs::read(Path::new(&unzip).join("src/sub/b.txt")).unwrap(),
            b"beta"
        );
        assert_eq!(
            fs::read(Path::new(&unzip).join("single.txt")).unwrap(),
            b"gamma"
        );
    }
}

#[test]
fn extract_rejects_traversal_and_absolute_entries() {
    let h = Harness::new();
    let p = h.admin();
    let archive = h.data_path("evil.zip");
    zip_file(
        &archive,
        &[
            ("../evil.txt", b"evil"),
            ("/abs.txt", b"abs"),
            ("ok.txt", b"ok"),
        ],
    );
    let dest = h.data_path("evil-out");
    fs::create_dir_all(&dest).unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive: archive.clone(),
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, FsError::Denied(_) | FsError::Escape(_)),
        "穿越/绝对路径必须被拒绝: {err:?}"
    );
    assert!(!h.base().join("evil.txt").exists(), "不得写到目标目录之外");
    assert_eq!(count_files(Path::new(&dest)), 0, "中止后必须清理半成品");
}

#[test]
fn extract_symlink_entries_are_skipped() {
    let h = Harness::new();
    let p = h.admin();
    let archive = h.data_path("links.tar.gz");
    let outside = h.base().join("outside.txt");
    fs::write(&outside, b"secret").unwrap();
    tar_gz_file(&archive, |tar| {
        tar_regular(tar, "real.txt", b"real");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_mode(0o777);
        header.set_size(0);
        header.set_link_name("../outside.txt").unwrap();
        header.set_cksum();
        tar.append_data(&mut header, "escape-link", std::io::empty())
            .unwrap();
    });
    let dest = h.data_path("links-out");
    fs::create_dir_all(&dest).unwrap();

    let out =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap();
    assert_eq!(out.files, 1);
    assert_eq!(out.skipped_symlinks, 1);
    assert!(!Path::new(&dest).join("escape-link").exists());
    assert_eq!(fs::read(outside).unwrap(), b"secret");
}

#[test]
fn extract_total_limit_aborts_and_cleans_partial_output() {
    let h = Harness::with_config(|cfg| {
        cfg.extract_max_total_bytes = 10;
        cfg.extract_max_entry_bytes = 10;
    });
    let p = h.admin();
    let archive = h.data_path("two.zip");
    zip_file(&archive, &[("a.txt", b"12345678"), ("b.txt", b"87654321")]);
    let dest = h.data_path("two-out");
    fs::create_dir_all(&dest).unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, FsError::TooLarge(_)),
        "应为 TooLarge: {err:?}"
    );
    assert_eq!(
        count_files(Path::new(&dest)),
        0,
        "超限后必须清理已解出的半成品"
    );
}

#[test]
fn extract_entry_limit_aborts_and_cleans() {
    let h = Harness::with_config(|cfg| {
        cfg.extract_max_entries = 2;
    });
    let p = h.admin();
    let archive = h.data_path("many.zip");
    zip_file(
        &archive,
        &[("a.txt", b"a"), ("b.txt", b"b"), ("c.txt", b"c")],
    );
    let dest = h.data_path("many-out");
    fs::create_dir_all(&dest).unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, FsError::TooManyEntries(_)),
        "应为 TooManyEntries: {err:?}"
    );
    assert_eq!(count_files(Path::new(&dest)), 0);
}

#[test]
fn extract_single_entry_limit_is_enforced() {
    let h = Harness::with_config(|cfg| {
        cfg.extract_max_entry_bytes = 5;
        cfg.extract_max_total_bytes = 1024;
    });
    let p = h.admin();
    let archive = h.data_path("big.zip");
    zip_file(&archive, &[("big.txt", b"123456")]);
    let dest = h.data_path("big-out");
    fs::create_dir_all(&dest).unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, FsError::FileTooLarge(_)),
        "应为 FileTooLarge: {err:?}"
    );
    assert_eq!(count_files(Path::new(&dest)), 0);
}

#[test]
fn archive_entry_limit_is_enforced() {
    let h = Harness::with_config(|cfg| {
        cfg.archive_max_entries = 3;
    });
    let p = h.admin();
    for name in ["a", "b", "c"] {
        h.seed(&h.data_path(&format!("src/{name}.txt")), b"x");
    }
    let dest = h.data_path("too-many.zip");
    let err =
        h.fs.archive(
            &p,
            &ctx(),
            &ArchiveRequest {
                sources: vec![h.data_path("src")],
                dest,
                format: ArchiveFormat::Zip,
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, FsError::TooManyEntries(_)),
        "应为 TooManyEntries: {err:?}"
    );
}

#[test]
fn archive_rejects_protected_destination() {
    let h = Harness::with_config_at(
        |base| vec![base.join("protected")],
        |cfg| {
            let _ = cfg;
        },
    );
    let p = h.admin();
    h.seed(&h.data_path("src/a.txt"), b"a");
    let dest = h.base().join("protected").join("out.zip");
    let err =
        h.fs.archive(
            &p,
            &ctx(),
            &ArchiveRequest {
                sources: vec![h.data_path("src")],
                dest: dest.to_string_lossy().to_string(),
                format: ArchiveFormat::Zip,
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(matches!(err, FsError::Protected(_)));
}

#[test]
fn extract_rejects_protected_destination() {
    let h = Harness::with_config_at(
        |base| vec![base.join("protected")],
        |cfg| {
            let _ = cfg;
        },
    );
    let p = h.admin();
    let archive = h.data_path("ok.zip");
    zip_file(&archive, &[("a.txt", b"a")]);
    let dest = h.base().join("protected");
    fs::create_dir_all(&dest).unwrap();
    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.to_string_lossy().to_string(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(matches!(err, FsError::Protected(_)));
}

#[cfg(unix)]
#[test]
fn extract_refuses_symlink_in_destination_path() {
    let h = Harness::new();
    let p = h.admin();
    let archive = h.data_path("ok.zip");
    zip_file(&archive, &[("sub/x.txt", b"x")]);
    let dest = h.data_path("sym-out");
    fs::create_dir_all(&dest).unwrap();
    let outside = h.data_path("outside-dir");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, Path::new(&dest).join("sub")).unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(matches!(err, FsError::Escape(_)), "应为 Escape: {err:?}");
    assert!(!Path::new(&outside).join("x.txt").exists());
}

#[test]
fn extract_conflict_without_overwrite_is_rejected() {
    let h = Harness::new();
    let p = h.admin();
    let archive = h.data_path("ok.zip");
    zip_file(&archive, &[("a.txt", b"new")]);
    let dest = h.data_path("conflict-out");
    fs::create_dir_all(&dest).unwrap();
    fs::write(Path::new(&dest).join("a.txt"), b"old").unwrap();

    let err =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive: archive.clone(),
                dest_dir: dest.clone(),
                overwrite: false,
            },
        )
        .unwrap_err();
    assert!(matches!(err, FsError::Conflict(_)));
    let out =
        h.fs.extract(
            &p,
            &ctx(),
            &ExtractRequest {
                archive,
                dest_dir: dest.clone(),
                overwrite: true,
            },
        )
        .unwrap();
    assert_eq!(out.files, 1);
    assert_eq!(fs::read(Path::new(&dest).join("a.txt")).unwrap(), b"new");
}
