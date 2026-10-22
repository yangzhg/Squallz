use std::io::Write;

use super::*;
use crate::api::NoProgress;

#[test]
fn staging_name_does_not_repeat_a_long_archive_name() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join(format!("{}.zip", "a".repeat(240)));
    fs::write(&target, b"archive").unwrap();
    let scope = Scope::archive(&target, &ControlToken::default()).unwrap();
    let stage = reserve_stage(&scope).unwrap();

    assert!(stage.path.file_name().unwrap().len() < 100);
    assert!(matches!(
        cleanup_stage(FormatError::Cancelled, stage),
        FormatError::Cancelled
    ));
    assert_eq!(fs::read(&target).unwrap(), b"archive");
}

#[test]
fn staging_cleanup_preserves_a_rebound_path() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("archive.zip");
    fs::write(&target, b"archive").unwrap();
    let scope = Scope::archive(&target, &ControlToken::default()).unwrap();
    let stage = reserve_stage(&scope).unwrap();
    let rebound = stage.path.clone();
    let held = directory.path().join("held-stage");
    crate::move_path_no_replace(&rebound, &held).unwrap();
    fs::write(&rebound, b"competitor").unwrap();

    let error = cleanup_stage(FormatError::Cancelled, stage);

    assert!(matches!(error, FormatError::Io(_)));
    assert_eq!(fs::read(&held).unwrap(), b"");
    assert_eq!(fs::read(&rebound).unwrap(), b"competitor");
    assert_eq!(fs::read(&target).unwrap(), b"archive");
}

fn created_stage_rebound(directory_replacement: bool) {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("archive.zip");
    fs::write(&target, b"archive").unwrap();
    let guard = crate::inspect_create_destination(&target, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    let path = directory.path().join(".create-test.tmp");
    let displaced = directory.path().join("displaced-stage");
    fs::write(&path, b"writer output").unwrap();
    let file = open_regular_file_no_follow(&path).unwrap();
    let identity = file_identity(&file).unwrap();
    crate::move_path_no_replace(&path, &displaced).unwrap();
    let competing_file = if directory_replacement {
        fs::create_dir(&path).unwrap();
        path.join("competitor")
    } else {
        path.clone()
    };
    fs::write(&competing_file, b"keep").unwrap();

    let error = commit_created_archive(
        &target,
        ReservedTempFile {
            path: path.clone(),
            file,
            identity,
        },
        guard,
        &NoProgress,
        &ControlToken::default(),
    )
    .unwrap_err();

    assert!(matches!(error, FormatError::Io(_)));
    assert_eq!(fs::read(&competing_file).unwrap(), b"keep");
    assert_eq!(fs::read(&displaced).unwrap(), b"writer output");
    assert_eq!(fs::read(&target).unwrap(), b"archive");
    assert_eq!(path.is_dir(), directory_replacement);
}

#[test]
fn created_stage_binding_preserves_an_unverifiable_directory() {
    created_stage_rebound(true);
}

#[test]
fn created_stage_binding_rejects_a_regular_file_rebound_after_writing() {
    created_stage_rebound(false);
}

#[test]
fn staging_cleanup_removes_a_readonly_reserved_file() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("archive.zip");
    fs::write(&target, b"archive").unwrap();
    let scope = Scope::archive(&target, &ControlToken::default()).unwrap();
    let mut stage = reserve_stage(&scope).unwrap();
    stage.file.write_all(b"owned writer").unwrap();
    stage.file.sync_all().unwrap();
    let path = stage.path.clone();
    let mut permissions = stage.file.metadata().unwrap().permissions();
    permissions.set_readonly(true);
    stage.file.set_permissions(permissions).unwrap();

    let error = cleanup_stage(FormatError::Cancelled, stage);

    assert!(matches!(error, FormatError::Cancelled));
    assert!(!path.exists());
    assert_eq!(fs::read(&target).unwrap(), b"archive");
}
