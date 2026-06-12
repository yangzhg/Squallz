//! TAR roundtrip tests: permissions, symlinks, hardlink entries, Chinese
//! names, deep directories. All fixtures are generated in code.

mod common;

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryType, ExtractOptions, FormatError, NoProgress, OpenOptions,
};

/// Builds the fixture tree: executable file, Chinese name, deep nesting,
/// symlink.
fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("深层/a/b/c/d")).unwrap();
    fs::write(root.join("普通文件.txt"), "中文内容 chinese content").unwrap();
    fs::write(root.join("深层/a/b/c/d/deep.txt"), "deep").unwrap();
    fs::write(root.join("run.sh"), "#!/bin/sh\necho ok\n").unwrap();
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o751)).unwrap();
    std::os::unix::fs::symlink("普通文件.txt", root.join("link.txt")).unwrap();
}

fn missing_link_target_tar(path: &Path, entry_type: tar::EntryType) {
    let file = fs::File::create(path).unwrap();
    let mut builder = tar::Builder::new(file);
    let mut header = tar::Header::new_gnu();
    header.set_path("broken-link").unwrap();
    header.set_mode(0o777);
    header.set_size(0);
    header.set_entry_type(entry_type);
    header.set_cksum();
    builder.append(&header, io::empty()).unwrap();
    builder.finish().unwrap();
}

#[test]
fn tar_roundtrip_permissions_symlink_unicode_deep() {
    let dir = TempDir::new("tar-roundtrip");
    let root = dir.path().join("tree");
    build_tree(&root);
    let archive = dir.path().join("out.tar");
    let engine = engine();
    let ctl = ControlToken::new();

    engine
        .create(
            &archive,
            std::slice::from_ref(&root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    // Listing: types, mode and symlink target survive.
    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    let by_name = |name: &str| {
        entries
            .iter()
            .find(|e| e.path.display == name)
            .unwrap_or_else(|| panic!("{name} not listed"))
    };
    assert_eq!(
        by_name("tree/run.sh").unix_mode.map(|m| m & 0o7777),
        Some(0o751)
    );
    let link = by_name("tree/link.txt");
    match &link.entry_type {
        EntryType::Symlink { target } => assert_eq!(target, "普通文件.txt".as_bytes()),
        other => panic!("expected symlink, got {other:?}"),
    }
    assert!(matches!(
        by_name("tree/深层/a/b/c/d/deep.txt").entry_type,
        EntryType::File
    ));

    // Extraction: contents, permissions and the link itself.
    let out = dir.path().join("out");
    engine
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("tree/普通文件.txt")).unwrap(),
        "中文内容 chinese content"
    );
    assert_eq!(
        fs::read_to_string(out.join("tree/深层/a/b/c/d/deep.txt")).unwrap(),
        "deep"
    );
    let mode = fs::metadata(out.join("tree/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o751);
    let target = fs::read_link(out.join("tree/link.txt")).unwrap();
    assert_eq!(target, Path::new("普通文件.txt"));

    // Integrity test passes.
    let report = engine
        .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    assert!(report.entries_tested >= 7);
}

#[test]
fn tar_link_entries_without_targets_are_reported() {
    let dir = TempDir::new("tar-missing-link-target");
    let engine = engine();
    let ctl = ControlToken::new();

    for (name, entry_type, expected_kind) in [
        ("symlink.tar", tar::EntryType::Symlink, "symlink"),
        ("hardlink.tar", tar::EntryType::Link, "hardlink"),
    ] {
        let archive = dir.path().join(name);
        missing_link_target_tar(&archive, entry_type);

        let err = engine.list(&archive, &OpenOptions::default()).unwrap_err();
        assert!(
            matches!(
                err,
                FormatError::CorruptArchive(ref detail)
                    if detail.contains(expected_kind)
                        && detail.contains("missing target")
                        && detail.contains("broken-link")
            ),
            "{name}: wrong list error: {err:?}"
        );

        let report = engine
            .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
            .unwrap();
        assert!(!report.is_ok(), "{name}: test must report the bad entry");
        assert!(
            report.problems.iter().any(|problem| {
                problem.contains(expected_kind)
                    && problem.contains("missing target")
                    && problem.contains("broken-link")
            }),
            "{name}: missing target problem not reported: {:?}",
            report.problems
        );
    }
}

/// Hardlink entries must map to EntryType::Hardlink. The fixture is written
/// with the tar crate directly because filesystem hardlinks are
/// indistinguishable from plain files when walking inputs.
#[test]
fn tar_hardlink_entries_are_mapped() {
    let dir = TempDir::new("tar-hardlink");
    let archive = dir.path().join("links.tar");
    {
        let file = fs::File::create(&archive).unwrap();
        let mut builder = tar::Builder::new(file);
        let data = b"original";
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(data.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, "original.txt", data.as_slice())
            .unwrap();
        let mut link = tar::Header::new_gnu();
        link.set_mode(0o644);
        link.set_size(0);
        link.set_entry_type(tar::EntryType::Link);
        builder
            .append_link(&mut link, "copy.txt", "original.txt")
            .unwrap();
        builder.finish().unwrap();
    }

    let engine = engine();
    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    let link = entries
        .iter()
        .find(|e| e.path.display == "copy.txt")
        .expect("hardlink listed");
    match &link.entry_type {
        EntryType::Hardlink { target } => assert_eq!(target, b"original.txt"),
        other => panic!("expected hardlink, got {other:?}"),
    }

    // Extraction must not fail; hardlink materialisation itself is I4.
    let out = dir.path().join("out");
    let ctl = ControlToken::new();
    engine
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("original.txt")).unwrap(),
        "original"
    );
}

/// Sniffing: a tar file without its extension is still recognized through
/// the 512-byte head window (`ustar` at offset 257).
#[test]
fn tar_sniff_without_extension() {
    let dir = TempDir::new("tar-sniff");
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), "x").unwrap();
    let archive = dir.path().join("archive.tar");
    let engine = engine();
    let ctl = ControlToken::new();
    engine
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let anonymous = dir.path().join("mystery.bin");
    fs::rename(&archive, &anonymous).unwrap();
    let entries = engine.list(&anonymous, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "tree/a.txt"));
}

/// tar has no encryption: creating with a password must fail loudly.
#[test]
fn tar_create_with_password_is_rejected() {
    let dir = TempDir::new("tar-pw");
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), "x").unwrap();
    let engine = engine();
    let ctl = ControlToken::new();
    let opts = CreateOptions {
        password: Some(squallz_core::api::Password::new("secret")),
        ..CreateOptions::default()
    };
    let err = engine
        .create(
            &dir.path().join("out.tar"),
            &[root],
            &opts,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        squallz_core::api::FormatError::Unsupported(_)
    ));
}
