//! 7Z tests: roundtrip, AES-encrypted content, header (file name)
//! encryption, system 7-Zip interop when available. All fixtures are
//! generated in code.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use common::{command_exists, engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryPath, EntryType, ExtractOptions, ExtractReport, FormatError,
    NoProgress, OpenOptions, OverwritePolicy, Password,
};

fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "seven zip content 中文").unwrap();
    fs::write(root.join("sub/b.bin"), vec![42u8; 50_000]).unwrap();
    fs::write(root.join("run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
}

fn extract_opts() -> ExtractOptions {
    ExtractOptions::default()
}

/// Locates a system 7-Zip binary (7zz is the official macOS build).
fn system_7z() -> Option<&'static str> {
    ["7zz", "7z"].into_iter().find(|c| command_exists(c))
}

#[test]
fn sevenz_roundtrip_list_test_extract() {
    let dir = TempDir::new("7z-roundtrip");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("out.7z");
    engine
        .create(
            &archive,
            std::slice::from_ref(&root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    let empty_out = dir.path().join("empty-selection");
    let empty_report = engine
        .extract_with_report(
            &archive,
            &empty_out,
            Some(&[]),
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        empty_report,
        ExtractReport {
            destination: empty_out.clone(),
            ..ExtractReport::default()
        }
    );
    assert!(!empty_out.exists());

    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    let file = entries
        .iter()
        .find(|e| e.path.display == "tree/a.txt")
        .expect("a.txt listed");
    assert!(file.crc32.is_some());
    assert!(!file.encrypted);
    let script = entries
        .iter()
        .find(|e| e.path.display == "tree/run.sh")
        .unwrap();
    assert_eq!(script.unix_mode.map(|m| m & 0o7777), Some(0o755));

    let report = engine
        .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = dir.path().join("out");
    let extract_report = engine
        .extract_with_report(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(extract_report.destination, out);
    assert_eq!(extract_report.selected_entries, entries.len() as u64);
    assert_eq!(extract_report.failed + extract_report.skipped, 0);
    assert_eq!(
        extract_report.created
            + extract_report.directories
            + extract_report.replaced
            + extract_report.renamed,
        extract_report.selected_entries
    );
    assert_eq!(
        extract_report.output_bytes,
        entries
            .iter()
            .filter(|entry| matches!(entry.entry_type, EntryType::File))
            .map(|entry| entry.size)
            .sum::<u64>()
    );
    assert_eq!(
        fs::read_to_string(out.join("tree/a.txt")).unwrap(),
        "seven zip content 中文"
    );
    assert_eq!(
        fs::read(out.join("tree/sub/b.bin")).unwrap(),
        vec![42u8; 50_000]
    );
    let mode = fs::metadata(out.join("tree/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o755);
}

#[test]
fn sevenz_encrypted_content_requires_password() {
    let dir = TempDir::new("7z-aes");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("secret.7z");
    let opts = CreateOptions {
        password: Some(Password::new("correct horse")),
        encrypt_filenames: false,
        ..CreateOptions::default()
    };
    engine
        .create(&archive, &[root], &opts, &NoProgress, &ctl)
        .unwrap();

    // Without a password: names are visible (header not encrypted), but
    // content access fails.
    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "tree/a.txt"));
    assert!(entries.iter().any(|e| e.encrypted));
    let out = dir.path().join("nopw");
    let err = engine
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            FormatError::PasswordRequired | FormatError::WrongPassword | FormatError::Io(_)
        ),
        "unexpected error without password: {err:?}"
    );

    // Best-effort extraction must not turn a wrong password into a
    // successful skipped-entry result.
    let wrong = OpenOptions {
        password: Some(Password::new("wrong horse")),
        ..OpenOptions::default()
    };
    let best_effort = ExtractOptions {
        best_effort: true,
        overwrite: OverwritePolicy::Overwrite,
        ..extract_opts()
    };
    let out = dir.path().join("wrongpw");
    fs::create_dir_all(out.join("tree")).unwrap();
    fs::write(out.join("tree/a.txt"), b"keep existing").unwrap();
    let error = engine
        .extract(
            &archive,
            &out,
            None,
            &wrong,
            &best_effort,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::WrongPassword), "{error:?}");
    assert_eq!(fs::read(out.join("tree/a.txt")).unwrap(), b"keep existing");
    assert!(fs::read_dir(out.join("tree")).unwrap().all(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .is_none_or(|name| !name.starts_with(".squallz-extract-"))
    }));

    let error = engine
        .test_summary(&archive, &wrong, &NoProgress, &ctl)
        .unwrap_err();
    assert!(matches!(error, FormatError::WrongPassword), "{error:?}");

    // With the password everything decrypts.
    let open = OpenOptions {
        password: Some(Password::new("correct horse")),
        ..OpenOptions::default()
    };
    let out = dir.path().join("withpw");
    engine
        .extract(
            &archive,
            &out,
            None,
            &open,
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("tree/a.txt")).unwrap(),
        "seven zip content 中文"
    );
}

#[test]
fn sevenz_encrypted_header_requires_password_to_list() {
    let dir = TempDir::new("7z-header");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("hidden.7z");
    let opts = CreateOptions {
        password: Some(Password::new("hidden names")),
        encrypt_filenames: true,
        ..CreateOptions::default()
    };
    engine
        .create(&archive, &[root], &opts, &NoProgress, &ctl)
        .unwrap();

    // Without a password even listing must fail with PasswordRequired.
    let err = engine.list(&archive, &OpenOptions::default()).unwrap_err();
    assert!(
        matches!(err, FormatError::PasswordRequired),
        "expected PasswordRequired, got {err:?}"
    );

    // With the password the names appear.
    let open = OpenOptions {
        password: Some(Password::new("hidden names")),
        ..OpenOptions::default()
    };
    let entries = engine.list(&archive, &open).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "tree/a.txt"));
}

#[cfg(unix)]
#[test]
fn sevenz_create_reports_symlink_unsupported_with_entry_and_target() {
    let dir = TempDir::new("7z-symlink-unsupported");
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("real.txt"), b"data").unwrap();
    std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("out.7z");

    let err = engine
        .create(
            &archive,
            std::slice::from_ref(&root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(
                detail.contains("symbolic link"),
                "detail must name the entry type: {detail}"
            );
            assert!(
                detail.contains("tree/link.txt"),
                "detail must name the entry: {detail}"
            );
            assert!(
                detail.contains("real.txt"),
                "detail must name the link target: {detail}"
            );
            assert!(
                detail.contains("tar or zip"),
                "detail must suggest a preserving format: {detail}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn sevenz_interop_with_system_7zip() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH (covered by self-read tests)");
        return;
    };
    let dir = TempDir::new("7z-interop");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();

    // Ours → system: `7zz t` validates the archive.
    let archive = dir.path().join("ours.7z");
    engine
        .create(
            &archive,
            std::slice::from_ref(&root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let check = Command::new(bin).arg("t").arg(&archive).output().unwrap();
    assert!(
        check.status.success(),
        "{bin} t failed: {}",
        String::from_utf8_lossy(&check.stdout)
    );

    // System → ours: list/extract a 7zz-created archive.
    let sys_archive = dir.path().join("system.7z");
    let create = Command::new(bin)
        .arg("a")
        .arg(&sys_archive)
        .arg("tree")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(create.status.success());
    let entries = engine.list(&sys_archive, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display.contains("a.txt")));
    let out = dir.path().join("out");
    engine
        .extract(
            &sys_archive,
            &out,
            None,
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("tree/a.txt")).unwrap(),
        "seven zip content 中文"
    );
}

#[test]
fn sevenz_solid_stream_drains_unselected_and_conflicting_entries() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH");
        return;
    };
    let dir = TempDir::new("7z-solid-drain");
    let source = dir.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), b"first solid entry").unwrap();
    fs::write(source.join("z.txt"), b"later solid entry").unwrap();
    let archive = dir.path().join("solid.7z");
    let create = Command::new(bin)
        .args(["a", "-ms=on"])
        .arg(&archive)
        .args(["a.txt", "z.txt"])
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{bin} failed to create solid archive: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let engine = engine();
    let ctl = ControlToken::new();
    let selected_out = dir.path().join("selected");
    engine
        .extract(
            &archive,
            &selected_out,
            Some(&[EntryPath::from_utf8("z.txt")]),
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert!(!selected_out.join("a.txt").exists());
    assert_eq!(
        fs::read(selected_out.join("z.txt")).unwrap(),
        b"later solid entry"
    );

    let conflict_out = dir.path().join("conflict");
    fs::create_dir_all(&conflict_out).unwrap();
    fs::write(conflict_out.join("a.txt"), b"keep existing").unwrap();
    let skip = ExtractOptions {
        overwrite: OverwritePolicy::Skip,
        ..extract_opts()
    };
    engine
        .extract(
            &archive,
            &conflict_out,
            None,
            &OpenOptions::default(),
            &skip,
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(conflict_out.join("a.txt")).unwrap(),
        b"keep existing"
    );
    assert_eq!(
        fs::read(conflict_out.join("z.txt")).unwrap(),
        b"later solid entry"
    );
}

#[test]
fn sevenz_selection_skips_an_unrelated_damaged_block() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH");
        return;
    };
    const FIRST: &[u8] = b"FIRST_BLOCK_UNIQUE_0123456789";
    const SECOND: &[u8] = b"SECOND_BLOCK_VALID_abcdefghij";
    let dir = TempDir::new("7z-independent-selection");
    let source = dir.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), FIRST).unwrap();
    fs::write(source.join("z.txt"), SECOND).unwrap();
    let archive = dir.path().join("non-solid.7z");
    let create = Command::new(bin)
        .args(["a", "-t7z", "-m0=Copy", "-ms=off"])
        .arg(&archive)
        .args(["a.txt", "z.txt"])
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{bin} failed to create non-solid archive: {}",
        String::from_utf8_lossy(&create.stderr)
    );
    let mut bytes = fs::read(&archive).unwrap();
    let first_offset = bytes
        .windows(FIRST.len())
        .position(|window| window == FIRST)
        .expect("COPY block contains the first marker");
    bytes[first_offset] ^= 0x5a;
    fs::write(&archive, bytes).unwrap();

    let out = dir.path().join("out");
    engine()
        .extract(
            &archive,
            &out,
            Some(&[EntryPath::from_utf8("z.txt")]),
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

    assert!(!out.join("a.txt").exists());
    assert_eq!(fs::read(out.join("z.txt")).unwrap(), SECOND);

    let best_effort_out = dir.path().join("best-effort");
    let best_effort = ExtractOptions {
        best_effort: true,
        ..extract_opts()
    };
    let report = engine()
        .extract_with_report(
            &archive,
            &best_effort_out,
            None,
            &OpenOptions::default(),
            &best_effort,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

    assert_eq!(report.selected_entries, 2);
    assert_eq!(report.failed, 1);
    assert_eq!(report.created, 1);
    assert_eq!(report.skipped + report.replaced + report.renamed, 0);
    assert_eq!(report.output_bytes, SECOND.len() as u64);
    assert!(!best_effort_out.join("a.txt").exists());
    assert_eq!(fs::read(best_effort_out.join("z.txt")).unwrap(), SECOND);
}

#[test]
fn sevenz_best_effort_stops_a_damaged_solid_block_and_continues_the_next_block() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH");
        return;
    };
    let dir = TempDir::new("7z-best-effort-solid-boundary");
    let source = dir.path().join("source");
    fs::create_dir_all(&source).unwrap();
    let pseudorandom = |mut state: u32| {
        (0..256 * 1024)
            .map(move |_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect::<Vec<_>>()
    };
    let first = pseudorandom(1);
    let same_block = pseudorandom(2);
    let next_block = pseudorandom(3);
    fs::write(source.join("a.txt"), &first).unwrap();
    fs::write(source.join("b.txt"), &same_block).unwrap();
    fs::write(source.join("z.txt"), &next_block).unwrap();
    let archive = dir.path().join("two-solid-blocks.7z");
    let create = Command::new(bin)
        .args(["a", "-t7z", "-ms=2f"])
        .arg(&archive)
        .args(["a.txt", "b.txt", "z.txt"])
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{bin} failed to create bounded solid blocks: {}",
        String::from_utf8_lossy(&create.stderr)
    );
    let parsed = sevenz_rust2::ArchiveReader::new(
        fs::File::open(&archive).unwrap(),
        sevenz_rust2::Password::empty(),
    )
    .unwrap();
    let parsed_archive = parsed.archive();
    let block_for = |name: &str| {
        let index = parsed_archive
            .files
            .iter()
            .position(|entry| entry.name() == name)
            .unwrap();
        parsed_archive.stream_map.file_block_index[index].unwrap()
    };
    assert_eq!(block_for("a.txt"), block_for("b.txt"));
    assert_ne!(block_for("a.txt"), block_for("z.txt"));
    let first_block = block_for("a.txt");
    let pack_index = parsed_archive.stream_map.block_first_pack_stream_index()[first_block];
    let packed_offset = 32
        + parsed_archive.pack_pos() as usize
        + parsed_archive.stream_map.pack_stream_offsets()[pack_index] as usize;
    let packed_size = parsed_archive.pack_sizes()[pack_index] as usize;
    assert!(packed_size > 16);
    drop(parsed);
    let mut bytes = fs::read(&archive).unwrap();
    bytes[packed_offset + packed_size / 4] ^= 0x5a;
    fs::write(&archive, bytes).unwrap();

    let out = dir.path().join("out");
    let opts = ExtractOptions {
        best_effort: true,
        ..extract_opts()
    };
    let report = engine()
        .extract_with_report(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &opts,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

    assert_eq!(report.failed, 2);
    assert_eq!(report.created, 1);
    assert_eq!(report.selected_entries, 3);
    assert!(!out.join("a.txt").exists());
    assert!(!out.join("b.txt").exists());
    assert_eq!(fs::read(out.join("z.txt")).unwrap(), next_block);

    let selected_out = dir.path().join("selected-out");
    let selection = [EntryPath::from_utf8("b.txt"), EntryPath::from_utf8("z.txt")];
    let report = engine()
        .extract_with_report(
            &archive,
            &selected_out,
            Some(&selection),
            &OpenOptions::default(),
            &opts,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

    assert_eq!(report.selected_entries, 2);
    assert_eq!(report.failed, 1);
    assert_eq!(report.created, 1);
    assert!(!selected_out.join("a.txt").exists());
    assert!(!selected_out.join("b.txt").exists());
    assert_eq!(fs::read(selected_out.join("z.txt")).unwrap(), next_block);
}

#[test]
fn sevenz_mixed_archive_keeps_encryption_and_error_classification_per_block() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH");
        return;
    };
    const SECRET: &[u8] = b"ENCRYPTED_BLOCK_secret_payload";
    const PLAIN: &[u8] = b"PLAIN_BLOCK_corruption_marker";
    let dir = TempDir::new("7z-mixed-encryption");
    let source = dir.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("secret.txt"), SECRET).unwrap();
    fs::write(source.join("plain.txt"), PLAIN).unwrap();
    let archive = dir.path().join("mixed.7z");
    let encrypted_create = Command::new(bin)
        .args(["a", "-t7z", "-m0=Copy", "-ms=off", "-psecret", "-y"])
        .arg(&archive)
        .arg("secret.txt")
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        encrypted_create.status.success(),
        "{bin} failed to create encrypted block: {}",
        String::from_utf8_lossy(&encrypted_create.stderr)
    );
    let plain_append = Command::new(bin)
        .args(["a", "-t7z", "-m0=Copy", "-ms=off", "-y"])
        .arg(&archive)
        .arg("plain.txt")
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        plain_append.status.success(),
        "{bin} failed to append plain block: {}",
        String::from_utf8_lossy(&plain_append.stderr)
    );

    let open = OpenOptions {
        password: Some(Password::new("secret")),
        ..OpenOptions::default()
    };
    let archive_entries = engine().list(&archive, &open).unwrap();
    let secret = archive_entries
        .iter()
        .find(|entry| entry.path.display == "secret.txt")
        .expect("secret entry listed");
    let plain = archive_entries
        .iter()
        .find(|entry| entry.path.display == "plain.txt")
        .expect("plain entry listed");
    assert!(secret.encrypted);
    assert!(!plain.encrypted);

    let mut bytes = fs::read(&archive).unwrap();
    let plain_offset = bytes
        .windows(PLAIN.len())
        .position(|window| window == PLAIN)
        .expect("COPY block contains the plain marker");
    bytes[plain_offset] ^= 0x5a;
    fs::write(&archive, bytes).unwrap();

    let out = dir.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("plain.txt"), b"keep existing").unwrap();
    let opts = ExtractOptions {
        overwrite: OverwritePolicy::Overwrite,
        ..extract_opts()
    };
    let error = engine()
        .extract(
            &archive,
            &out,
            Some(&[EntryPath::from_utf8("plain.txt")]),
            &open,
            &opts,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();

    assert!(
        matches!(error, FormatError::Io(_) | FormatError::CorruptArchive(_)),
        "plain block damage must not be classified as a password error: {error:?}"
    );
    assert_eq!(fs::read(out.join("plain.txt")).unwrap(), b"keep existing");
    assert!(fs::read_dir(&out).unwrap().all(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.file_name().into_string().ok())
            .is_none_or(|name| !name.starts_with(".squallz-extract-"))
    }));
}

#[cfg(unix)]
#[test]
fn sevenz_first_safety_error_prevents_later_block_writes() {
    let Some(bin) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH");
        return;
    };
    let dir = TempDir::new("7z-first-error");
    let source = dir.path().join("source");
    fs::create_dir_all(source.join("blocked")).unwrap();
    fs::write(source.join("blocked/file.txt"), b"must stay outside").unwrap();
    fs::write(source.join("good.txt"), b"must not be written").unwrap();
    let archive = dir.path().join("independent-blocks.7z");
    let create = Command::new(bin)
        .args(["a", "-ms=off"])
        .arg(&archive)
        .args(["blocked/file.txt", "good.txt"])
        .current_dir(&source)
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{bin} failed to create archive: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let out = dir.path().join("out");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, out.join("blocked")).unwrap();
    let error = engine()
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &extract_opts(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();

    assert!(
        matches!(error, FormatError::SymlinkBreakout(_)),
        "{error:?}"
    );
    assert!(!outside.join("file.txt").exists());
    assert!(!out.join("good.txt").exists());
}
