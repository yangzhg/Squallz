use std::fs;
use std::path::Path;

use serde_json::json;
use squallz_core::api::{OverwritePolicy, SymlinkPolicy};
use squallz_core::lock_unpoisoned;

use super::*;
use crate::jobs::snapshots::JobSnapshotStore;
use crate::jobs::test_support::{checksum_job, compress_file_job};

fn extract_spec(dest: &Path) -> JobSpec {
    JobSpec::Extract {
        path: "source.zip".into(),
        dest: dest.to_string_lossy().into_owned(),
        expected_destination: None,
        expected_input_guard: None,
        selection: None,
        overwrite: OverwritePolicy::Ask,
        symlinks: SymlinkPolicy::Preserve,
        smart: true,
        encoding: None,
        password: None,
        verify_sfx: false,
        best_effort: false,
    }
}

fn completed(spec: JobSpec, result: serde_json::Value) -> JobStateSnapshot {
    let mut store = JobSnapshotStore::default();
    store.insert(1, Some("task-owner".into()), spec, "queued");
    store.set_state(1, "done", None, Some(result));
    store.snapshot("task-owner", 1).unwrap()
}

#[test]
fn extraction_opens_the_reported_destination_not_the_requested_base() {
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("中文输出");
    fs::create_dir(&actual).unwrap();
    let snapshot = completed(extract_spec(dir.path()), json!({"dest": actual}));
    assert_eq!(output_path(&snapshot).unwrap(), actual);

    let nested = completed(
        JobSpec::ExtractNested {
            outer_path: "outer.zip".into(),
            entry_path: "inner.zip".into(),
            dest: dir.path().to_string_lossy().into_owned(),
            overwrite: OverwritePolicy::Ask,
            symlinks: SymlinkPolicy::Preserve,
            smart: true,
            encoding: None,
            password: None,
            best_effort: false,
        },
        json!({"dest": actual}),
    );
    assert_eq!(output_path(&nested).unwrap(), actual);
}

#[test]
fn batch_output_requires_a_successfully_reported_destination() {
    let dir = tempfile::tempdir().unwrap();
    let mut snapshot = completed(
        JobSpec::BatchExtract {
            items: Vec::new(),
            overwrite: OverwritePolicy::Ask,
            symlinks: SymlinkPolicy::Preserve,
            smart: true,
        },
        json!({"outputs": [{"dest": dir.path()}], "failed": 1}),
    );
    assert_eq!(output_path(&snapshot).unwrap(), dir.path());
    snapshot.result = Some(json!({"outputs": [], "failed": 1}));
    assert!(output_path(&snapshot).is_err());
}

#[test]
fn incomplete_missing_and_wrong_type_outputs_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let valid = completed(extract_spec(dir.path()), json!({"dest": dir.path()}));
    for state in ["queued", "running", "paused", "failed", "cancelled"] {
        let mut snapshot = valid.clone();
        snapshot.state = state.into();
        assert!(output_path(&snapshot).is_err(), "{state}");
    }
    let file = dir.path().join("not-a-folder");
    fs::write(&file, b"data").unwrap();
    for result in [
        None,
        Some(json!({})),
        Some(json!({"dest": ""})),
        Some(json!({"dest": dir.path().join("missing")})),
        Some(json!({"dest": file})),
    ] {
        let mut snapshot = valid.clone();
        snapshot.result = result;
        assert!(output_path(&snapshot).is_err());
    }
}

#[test]
fn archive_and_recovery_outputs_preserve_the_existing_opening_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("result.zip");
    fs::write(&archive, b"synthetic output").unwrap();
    let convert = JobSpec::Convert {
        src: "source.7z".into(),
        dest: "requested.zip".into(),
        level: 5,
        src_encoding: None,
        src_password: None,
        dest_password: None,
        encrypt_names: false,
        split_size: None,
        split_mode: squallz_core::api::SplitOutputMode::Generic,
        replace_existing: false,
        replacement_guard: None,
    };
    assert_eq!(
        output_path(&completed(
            convert.clone(),
            json!({"primary_output": archive, "split": false})
        ))
        .unwrap(),
        archive
    );
    for split in [json!(true), serde_json::Value::Null] {
        assert!(output_path(&completed(
            convert.clone(),
            json!({"primary_output": archive, "split": split})
        ))
        .is_err());
    }
    for spec in [
        JobSpec::ExportSqz {
            src: "source.sqz".into(),
            dest: "requested.zip".into(),
            level: 5,
            dest_password: None,
            replace_existing: false,
            replacement_guard: None,
        },
        JobSpec::RepairSqz {
            src: "source.sqz".into(),
            dest: "requested.sqz".into(),
            level: 5,
        },
        JobSpec::RepairZip {
            src: "source.zip".into(),
            dest: "requested.zip".into(),
            level: 5,
        },
    ] {
        assert_eq!(
            output_path(&completed(spec, json!({"dest": archive}))).unwrap(),
            archive
        );
    }
    let repair = JobSpec::RepairRecovery {
        path: archive.to_string_lossy().into_owned(),
        output: None,
        output_directory: false,
        recovery: None,
    };
    assert_eq!(
        output_path(&completed(
            repair.clone(),
            json!({"ok": true, "archive": archive})
        ))
        .unwrap(),
        archive
    );
    assert!(output_path(&completed(repair, json!({"ok": false, "output": archive}))).is_err());

    for spec in [
        checksum_job(&archive),
        compress_file_job(&archive, &archive),
        JobSpec::Protect {
            path: "source.zip".into(),
            redundancy: 10,
            recovery: None,
        },
    ] {
        assert!(output_path(&completed(
            spec,
            json!({"primary_output": archive, "dest": archive})
        ))
        .is_err());
    }
}

#[test]
fn published_apps_and_repaired_directories_use_directory_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let published = completed(
        JobSpec::PublishMacosSfx {
            source: "Source.app".into(),
            output: "Published.app".into(),
            identity: String::new(),
            notary_profile: String::new(),
        },
        json!({"primary_output": dir.path()}),
    );
    assert_eq!(output_path(&published).unwrap(), dir.path());
    let repaired = completed(
        JobSpec::RepairRecovery {
            path: "source.zip".into(),
            output: Some("requested".into()),
            output_directory: true,
            recovery: None,
        },
        json!({"ok": true, "output": dir.path()}),
    );
    assert_eq!(output_path(&repaired).unwrap(), dir.path());
}

#[test]
fn output_access_obeys_window_visibility_and_dismissal() {
    let dir = tempfile::tempdir().unwrap();
    let manager = JobManager::new();
    {
        let mut snapshots = lock_unpoisoned(&manager.snapshots);
        snapshots.insert(
            42,
            Some("task-owner".into()),
            extract_spec(dir.path()),
            "queued",
        );
        snapshots.set_state(42, "done", None, Some(json!({"dest": dir.path()})));
    }
    for requester in ["main", "task-owner"] {
        assert_eq!(
            manager.openable_output_for_window(requester, 42).unwrap(),
            dir.path()
        );
    }
    assert!(manager
        .openable_output_for_window("task-other", 42)
        .is_err());
    assert!(manager.openable_output_for_window("main", 43).is_err());
    manager
        .dismiss_snapshots_for_window("task-owner", &[42])
        .unwrap();
    assert!(manager
        .openable_output_for_window("task-owner", 42)
        .is_err());
    assert!(manager.openable_output_for_window("main", 42).is_ok());
}
