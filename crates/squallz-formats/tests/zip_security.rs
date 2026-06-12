//! Security tests (PLAN.md §2.3): Zip Slip, symlink breakout,
//! decompression-bomb guardrails and overwrite policies.

mod common;

use std::fs;

use common::{build_stored_zip, engine, RawZipEntry, TempDir};
use squallz_format_api::{
    ControlToken, CreateOptions, Detected, EntryMeta, EntryPath, EntryType, ExtractOptions,
    FormatError, NoProgress, OpenOptions, OverwritePolicy, SafetyLimits,
};

fn default_open() -> OpenOptions {
    OpenOptions::default()
}

#[test]
fn zip_slip_dotdot_entry_is_rejected() {
    let tmp = TempDir::new("slip-dotdot");
    let archive = tmp.path().join("evil.zip");
    fs::write(
        &archive,
        build_stored_zip(&[RawZipEntry {
            name: b"../evil.txt".to_vec(),
            data: b"pwned".to_vec(),
        }]),
    )
    .unwrap();

    let dest = tmp.path().join("dest");
    let err = engine()
        .extract(
            &archive,
            &dest,
            None,
            &default_open(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PathTraversal(_)), "{err:?}");
    // Nothing escaped above the destination directory.
    assert!(!tmp.path().join("evil.txt").exists());
}

#[test]
fn zip_slip_absolute_path_entry_is_rejected() {
    let tmp = TempDir::new("slip-abs");
    let archive = tmp.path().join("evil-abs.zip");
    let outside = tmp.path().join("outside-target.txt");
    let abs_name = outside.to_string_lossy().into_owned().into_bytes();
    fs::write(
        &archive,
        build_stored_zip(&[RawZipEntry {
            name: abs_name,
            data: b"pwned".to_vec(),
        }]),
    )
    .unwrap();

    let dest = tmp.path().join("dest");
    let err = engine()
        .extract(
            &archive,
            &dest,
            None,
            &default_open(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PathTraversal(_)), "{err:?}");
    assert!(!outside.exists());
}

#[test]
#[cfg(unix)]
fn symlink_breakout_write_is_rejected() {
    let tmp = TempDir::new("symlink-breakout");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();

    // Handcraft via our own writer: a symlink pointing outside the
    // destination, then a file whose path traverses that symlink.
    let archive = tmp.path().join("breakout.zip");
    let registry = squallz_formats::registry();
    let Some(Detected::Archive(format)) = registry.detect_by_name("breakout.zip") else {
        panic!("zip not registered");
    };
    let mut writer = format
        .create(
            Box::new(fs::File::create(&archive).unwrap()),
            &CreateOptions::default(),
        )
        .unwrap();
    let link_meta = EntryMeta {
        path: EntryPath::from_utf8("out"),
        entry_type: EntryType::Symlink {
            target: outside.to_string_lossy().into_owned().into_bytes(),
        },
        size: 0,
        compressed_size: None,
        modified: None,
        unix_mode: Some(0o120_777),
        crc32: None,
        encrypted: false,
    };
    writer.add_entry(&link_meta, None).unwrap();
    let mut data: &[u8] = b"pwned";
    let file_meta = EntryMeta {
        path: EntryPath::from_utf8("out/inner.txt"),
        entry_type: EntryType::File,
        size: 5,
        compressed_size: None,
        modified: None,
        unix_mode: Some(0o644),
        crc32: None,
        encrypted: false,
    };
    writer.add_entry(&file_meta, Some(&mut data)).unwrap();
    writer.finish().unwrap();

    let dest = tmp.path().join("dest");
    let err = engine()
        .extract(
            &archive,
            &dest,
            None,
            &default_open(),
            &ExtractOptions::default(), // Preserve symlinks
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::SymlinkBreakout(_)), "{err:?}");
    // Nothing was written through the link into the outside directory.
    assert!(!outside.join("inner.txt").exists());
}

#[test]
fn bomb_output_byte_limit_aborts() {
    let tmp = TempDir::new("bomb-bytes");
    let src = tmp.path().join("big.txt");
    fs::write(&src, vec![b'A'; 4096]).unwrap();
    let archive = tmp.path().join("big.zip");
    let eng = engine();
    eng.create(
        &archive,
        &[src],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();

    let opts = ExtractOptions {
        limits: SafetyLimits {
            max_output_bytes: 1024,
            ..SafetyLimits::default()
        },
        ..ExtractOptions::default()
    };
    let err = eng
        .extract(
            &archive,
            &tmp.path().join("dest"),
            None,
            &default_open(),
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(
        matches!(err, FormatError::ResourceLimitExceeded(_)),
        "{err:?}"
    );
}

#[test]
fn bomb_entry_count_limit_aborts() {
    let tmp = TempDir::new("bomb-entries");
    let dir = tmp.path().join("many");
    fs::create_dir_all(&dir).unwrap();
    for i in 0..3 {
        fs::write(dir.join(format!("f{i}.txt")), b"x").unwrap();
    }
    let archive = tmp.path().join("many.zip");
    let eng = engine();
    eng.create(
        &archive,
        &[dir],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();

    let opts = ExtractOptions {
        limits: SafetyLimits {
            max_entries: 2,
            ..SafetyLimits::default()
        },
        ..ExtractOptions::default()
    };
    let err = eng
        .extract(
            &archive,
            &tmp.path().join("dest"),
            None,
            &default_open(),
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(
        matches!(err, FormatError::ResourceLimitExceeded(_)),
        "{err:?}"
    );
}

/// Builds an archive containing `a.txt` with the given content and returns
/// its path.
fn archive_with_a_txt(tmp: &TempDir, content: &[u8]) -> std::path::PathBuf {
    let src = tmp.path().join("a.txt");
    fs::write(&src, content).unwrap();
    let archive = tmp.path().join("a.zip");
    engine()
        .create(
            &archive,
            &[src],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    archive
}

fn extract_with_policy(archive: &std::path::Path, dest: &std::path::Path, policy: OverwritePolicy) {
    let opts = ExtractOptions {
        overwrite: policy,
        ..ExtractOptions::default()
    };
    engine()
        .extract(
            archive,
            dest,
            None,
            &default_open(),
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
}

#[test]
fn overwrite_policy_skip_keeps_existing() {
    let tmp = TempDir::new("ow-skip");
    let archive = archive_with_a_txt(&tmp, b"new");
    let dest = tmp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();
    extract_with_policy(&archive, &dest, OverwritePolicy::Skip);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
}

#[test]
fn overwrite_policy_overwrite_replaces() {
    let tmp = TempDir::new("ow-all");
    let archive = archive_with_a_txt(&tmp, b"new");
    let dest = tmp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();
    extract_with_policy(&archive, &dest, OverwritePolicy::Overwrite);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"new");
}

#[test]
fn overwrite_policy_rename_keeps_both() {
    let tmp = TempDir::new("ow-rename");
    let archive = archive_with_a_txt(&tmp, b"new");
    let dest = tmp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();
    extract_with_policy(&archive, &dest, OverwritePolicy::RenameBoth);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read(dest.join("a (1).txt")).unwrap(), b"new");
}

#[test]
fn overwrite_policy_ask_without_resolver_degrades_to_skip() {
    let tmp = TempDir::new("ow-ask");
    let archive = archive_with_a_txt(&tmp, b"new");
    let dest = tmp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("a.txt"), b"old").unwrap();
    extract_with_policy(&archive, &dest, OverwritePolicy::Ask);
    assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"old");
}
