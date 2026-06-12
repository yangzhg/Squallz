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
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions, Password,
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
        .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = dir.path().join("out");
    engine
        .extract(
            &archive,
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
