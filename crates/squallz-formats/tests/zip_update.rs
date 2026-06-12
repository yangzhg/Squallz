//! ZIP update tests: add/delete/rename individually and combined, system
//! `unzip -t` interop, encrypted archives (raw copy without the password),
//! atomicity on failure.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{command_exists, engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryMeta, EntryPath, FormatError, NoProgress, OpenOptions,
    Password, UpdateOp,
};

/// Builds a base archive with project/a.txt, project/sub/b.txt, project/c.log.
fn base_archive(dir: &Path, password: Option<&str>) -> PathBuf {
    let root = dir.join("project");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("sub/b.txt"), b"bravo").unwrap();
    fs::write(root.join("c.log"), b"log line").unwrap();
    let dest = dir.join("base.zip");
    let opts = CreateOptions {
        password: password.map(Password::new),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[root], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn list_names(path: &Path, password: Option<&str>) -> Vec<String> {
    let opts = OpenOptions {
        password: password.map(Password::new),
        encoding_override: None,
    };
    let mut names: Vec<String> = engine()
        .list(path, &opts)
        .unwrap()
        .iter()
        .map(|e: &EntryMeta| e.path.display.clone())
        .collect();
    names.sort();
    names
}

fn run_update(path: &Path, ops: &[UpdateOp], opts: &CreateOptions) -> Result<(), FormatError> {
    engine().update(path, ops, opts, &NoProgress, &ControlToken::new())
}

fn assert_other_contains(err: FormatError, needle: &str) {
    match err {
        FormatError::Other(msg) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected FormatError::Other containing {needle}, got {other:?}"),
    }
}

/// `unzip -t` interop check (skipped when unzip is unavailable).
fn assert_unzip_t(path: &Path) {
    if !command_exists("unzip") {
        eprintln!("skipping unzip -t check: unzip not on PATH");
        return;
    }
    let out = Command::new("unzip").arg("-t").arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "unzip -t failed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn update_add_file_and_directory() {
    let tmp = TempDir::new("update-add");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"newcomer").unwrap();
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(&extra_dir).unwrap();
    fs::write(extra_dir.join("inner.txt"), b"inside").unwrap();

    let ops = vec![
        UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("new.txt"),
        },
        UpdateOp::Add {
            src: extra_dir,
            dest: EntryPath::from_utf8("extra"),
        },
    ];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"new.txt".to_string()));
    assert!(names.contains(&"extra/inner.txt".to_string()));
    assert!(names.iter().any(|n| n.starts_with("project/a.txt")));
    assert_unzip_t(&archive);
}

#[test]
fn update_add_empty_directory_entry() {
    let tmp = TempDir::new("update-add-dir");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::AddDir {
        path: EntryPath::from_utf8("empty-folder"),
    }];

    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"empty-folder/".to_string()), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_add_directory_applies_create_excludes() {
    let tmp = TempDir::new("update-add-excludes");
    let archive = base_archive(tmp.path(), None);
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(extra_dir.join("node_modules/pkg")).unwrap();
    fs::create_dir_all(extra_dir.join(".git")).unwrap();
    fs::write(extra_dir.join("keep.txt"), b"keep").unwrap();
    fs::write(extra_dir.join("drop.tmp"), b"drop").unwrap();
    fs::write(extra_dir.join("node_modules/pkg/index.js"), b"drop").unwrap();
    fs::write(extra_dir.join(".git/config"), b"drop").unwrap();

    let ops = vec![UpdateOp::Add {
        src: extra_dir,
        dest: EntryPath::from_utf8("extra"),
    }];
    let opts = CreateOptions {
        excludes: vec!["node_modules".into(), ".git".into(), "*.tmp".into()],
        ..CreateOptions::default()
    };
    run_update(&archive, &ops, &opts).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"extra/keep.txt".to_string()));
    assert!(
        !names.iter().any(|n| n.contains("node_modules")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.contains(".git")), "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_delete_by_glob() {
    let tmp = TempDir::new("update-delete");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Delete {
        pattern: "*.log".into(),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.ends_with(".log")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("a.txt")));

    // Deleting a directory name prunes its subtree.
    let ops = vec![UpdateOp::Delete {
        pattern: "project/sub".into(),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.contains("sub")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_rename_entry() {
    let tmp = TempDir::new("update-rename");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Rename {
        from: EntryPath::from_utf8("project/a.txt"),
        to: EntryPath::from_utf8("project/renamed.txt"),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&"project/renamed.txt".to_string()));
    assert!(!names.contains(&"project/a.txt".to_string()));
    assert_unzip_t(&archive);

    // The renamed entry's content is intact.
    let opts = OpenOptions::default();
    let mut reader = engine().open(&archive, &opts).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/renamed.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"alpha");

    // Renaming a missing entry fails and leaves the archive intact.
    let before = fs::read(&archive).unwrap();
    let ops = vec![UpdateOp::Rename {
        from: EntryPath::from_utf8("missing.txt"),
        to: EntryPath::from_utf8("whatever.txt"),
    }];
    let err = run_update(&archive, &ops, &CreateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Other(_)));
    assert_eq!(
        fs::read(&archive).unwrap(),
        before,
        "archive must be untouched"
    );
}

#[test]
fn update_rejects_target_conflicts_without_explicit_delete() {
    let tmp = TempDir::new("update-conflicts");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"replacement").unwrap();

    let before = fs::read(&archive).unwrap();
    let err = run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntryPath::from_utf8("project/a.txt"),
            to: EntryPath::from_utf8("project/sub/b.txt"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("project/a.txt"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::AddDir {
            path: EntryPath::from_utf8("project"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[
            UpdateOp::Rename {
                from: EntryPath::from_utf8("project/a.txt"),
                to: EntryPath::from_utf8("dup.txt"),
            },
            UpdateOp::Rename {
                from: EntryPath::from_utf8("project/sub/b.txt"),
                to: EntryPath::from_utf8("dup.txt"),
            },
        ],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "duplicate update target");
    assert_eq!(fs::read(&archive).unwrap(), before);

    run_update(
        &archive,
        &[
            UpdateOp::Delete {
                pattern: "project/a.txt".into(),
            },
            UpdateOp::Add {
                src: tmp.path().join("new.txt"),
                dest: EntryPath::from_utf8("project/a.txt"),
            },
        ],
        &CreateOptions::default(),
    )
    .unwrap();

    let mut reader = engine().open(&archive, &OpenOptions::default()).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/a.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"replacement");
    assert_unzip_t(&archive);
}

#[test]
fn update_combined_add_delete_rename() {
    let tmp = TempDir::new("update-combo");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("fresh.txt"), b"fresh").unwrap();
    let ops = vec![
        UpdateOp::Add {
            src: tmp.path().join("fresh.txt"),
            dest: EntryPath::from_utf8("fresh.txt"),
        },
        UpdateOp::Delete {
            pattern: "*.log".into(),
        },
        UpdateOp::Rename {
            from: EntryPath::from_utf8("project/sub/b.txt"),
            to: EntryPath::from_utf8("project/sub/beta.txt"),
        },
    ];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&"fresh.txt".to_string()));
    assert!(names.contains(&"project/sub/beta.txt".to_string()));
    assert!(!names.iter().any(|n| n.ends_with(".log")));
    assert!(!names.contains(&"project/sub/b.txt".to_string()));
    assert_unzip_t(&archive);
}

#[test]
fn update_encrypted_archive_without_password_keeps_encryption() {
    let tmp = TempDir::new("update-encrypted");
    let archive = base_archive(tmp.path(), Some("secret"));
    fs::write(tmp.path().join("plain.txt"), b"added later").unwrap();
    // No password supplied: old entries are raw-copied still encrypted.
    let ops = vec![UpdateOp::Add {
        src: tmp.path().join("plain.txt"),
        dest: EntryPath::from_utf8("plain.txt"),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let opts = OpenOptions::default();
    let entries = engine().list(&archive, &opts).unwrap();
    let old = entries
        .iter()
        .find(|e| e.path.display == "project/a.txt")
        .unwrap();
    assert!(old.encrypted, "raw-copied entry must stay encrypted");
    let new = entries
        .iter()
        .find(|e| e.path.display == "plain.txt")
        .unwrap();
    assert!(!new.encrypted);

    // Old content still decrypts with the original password.
    let open = OpenOptions {
        password: Some(Password::new("secret")),
        encoding_override: None,
    };
    let mut reader = engine().open(&archive, &open).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/a.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"alpha");
}

#[test]
fn update_unsupported_format_is_rejected() {
    let tmp = TempDir::new("update-unsupported");
    let root = tmp.path().join("d");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("f.txt"), b"x").unwrap();
    let dest = tmp.path().join("a.tar");
    engine()
        .create(
            &dest,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let ops = vec![UpdateOp::Delete {
        pattern: "f.txt".into(),
    }];
    let err = run_update(&dest, &ops, &CreateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}
