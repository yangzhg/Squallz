use super::super::test_support::{deterministic_payload, temp_dir, FakeTrashAdapter};
use super::*;

#[derive(Debug, PartialEq, Eq)]
enum CleanupHolderEvent {
    Sync(PathBuf),
    Remove(PathBuf),
}

#[test]
fn cleanup_holder_is_synced_before_staging_can_continue() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-holder-sync-order")).unwrap();
    let source = dir.join("source.txt");
    fs::write(&source, b"source").unwrap();
    let events = std::cell::RefCell::new(Vec::new());

    let holder = create_cleanup_staging_dir_with(
        &source,
        |path| {
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Sync(path.to_path_buf()));
            Ok(())
        },
        |path, identity| {
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Remove(path.to_path_buf()));
            remove_empty_holder_if_identity(path, identity)
        },
    )
    .unwrap();

    assert_eq!(
        events.into_inner(),
        vec![
            CleanupHolderEvent::Sync(holder.clone()),
            CleanupHolderEvent::Sync(dir.clone()),
        ]
    );
    assert!(source.exists());
    fs::remove_dir(&holder).unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleanup_holder_sync_failure_removes_only_the_created_identity() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-holder-sync-failure")).unwrap();
    let source = dir.join("source.txt");
    fs::write(&source, b"source").unwrap();
    let events = std::cell::RefCell::new(Vec::new());
    let sync_calls = std::cell::Cell::new(0usize);

    let error = create_cleanup_staging_dir_with(
        &source,
        |path| {
            let call = sync_calls.get();
            sync_calls.set(call + 1);
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Sync(path.to_path_buf()));
            if call == 0 {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected holder sync failure",
                ))
            } else {
                Ok(())
            }
        },
        |path, identity| {
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Remove(path.to_path_buf()));
            remove_empty_holder_if_identity(path, identity)
        },
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    let events = events.into_inner();
    let holder = match &events[0] {
        CleanupHolderEvent::Sync(path) => path.clone(),
        CleanupHolderEvent::Remove(_) => panic!("holder sync must be first"),
    };
    assert_eq!(
        events,
        vec![
            CleanupHolderEvent::Sync(holder.clone()),
            CleanupHolderEvent::Remove(holder.clone()),
            CleanupHolderEvent::Sync(dir.clone()),
        ]
    );
    assert!(source.exists());
    assert!(!holder.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleanup_parent_sync_failure_removes_holder_and_resyncs_parent() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-parent-sync-failure")).unwrap();
    let source = dir.join("source.txt");
    fs::write(&source, b"source").unwrap();
    let events = std::cell::RefCell::new(Vec::new());
    let failed_parent_sync = std::cell::Cell::new(false);

    let error = create_cleanup_staging_dir_with(
        &source,
        |path| {
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Sync(path.to_path_buf()));
            if path == dir && !failed_parent_sync.replace(true) {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected parent sync failure",
                ))
            } else {
                Ok(())
            }
        },
        |path, identity| {
            events
                .borrow_mut()
                .push(CleanupHolderEvent::Remove(path.to_path_buf()));
            remove_empty_holder_if_identity(path, identity)
        },
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    let events = events.into_inner();
    let holder = match &events[0] {
        CleanupHolderEvent::Sync(path) => path.clone(),
        CleanupHolderEvent::Remove(_) => panic!("holder sync must be first"),
    };
    assert_eq!(
        events,
        vec![
            CleanupHolderEvent::Sync(holder.clone()),
            CleanupHolderEvent::Sync(dir.clone()),
            CleanupHolderEvent::Remove(holder.clone()),
            CleanupHolderEvent::Sync(dir.clone()),
        ]
    );
    assert!(source.exists());
    assert!(!holder.exists());
    fs::remove_dir_all(dir).unwrap();
}

struct PreparedCleanup {
    plan: SourceCleanupPlan,
    archived_inputs: Vec<CreateInputManifestEntry>,
}

fn collect_test_input_manifest(
    path: &Path,
    manifest: &mut BTreeMap<PathBuf, CreateInputManifestEntry>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let source_path = if metadata.file_type().is_symlink() {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "test symlink has no parent")
        })?;
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "test symlink has no name")
        })?;
        fs::canonicalize(parent)?.join(name)
    } else {
        fs::canonicalize(path)?
    };
    let entry_type = if metadata.file_type().is_symlink() {
        squallz_core::api::EntryType::Symlink {
            target: fs::read_link(path)?
                .to_string_lossy()
                .into_owned()
                .into_bytes(),
        }
    } else if metadata.is_dir() {
        squallz_core::api::EntryType::Dir
    } else if metadata.is_file() {
        squallz_core::api::EntryType::File
    } else {
        squallz_core::api::EntryType::Other
    };
    let bytes = if metadata.is_file() {
        Some(fs::read(path)?)
    } else {
        None
    };
    #[cfg(unix)]
    let unix_mode = Some(metadata.mode());
    #[cfg(not(unix))]
    let unix_mode = None;
    manifest
        .entry(source_path.clone())
        .or_insert_with(|| CreateInputManifestEntry {
            source_path,
            archive_path: EntryPath::from_utf8(path.to_string_lossy().into_owned()),
            entry_type,
            size: bytes.as_ref().map_or(0, |bytes| bytes.len() as u64),
            modified: metadata.modified().ok().map(CreateInputModifiedTime::from),
            unix_mode,
            blake3: bytes.as_ref().map(|bytes| *blake3::hash(bytes).as_bytes()),
        });
    if metadata.is_dir() {
        let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            collect_test_input_manifest(&child.path(), manifest)?;
        }
    }
    Ok(())
}

fn prepare_cleanup(
    inputs: &[PathBuf],
    action: PostSuccessAction,
    excludes: &[String],
) -> PreparedCleanup {
    let plan = prepare_source_cleanup(
        inputs,
        action,
        excludes,
        &|| false,
        &squallz_core::api::NoProgress,
    )
    .unwrap();
    let mut archived_inputs = BTreeMap::new();
    for input in inputs {
        let _ = collect_test_input_manifest(input, &mut archived_inputs);
    }
    PreparedCleanup {
        plan,
        archived_inputs: archived_inputs.into_values().collect(),
    }
}

fn finish_cleanup(
    prepared: PreparedCleanup,
    outputs: &[PathBuf],
    trash_adapter: &FakeTrashAdapter,
    is_cancelled: &dyn Fn() -> bool,
) -> SourceCleanupResult {
    let journal_root = outputs
        .first()
        .and_then(|path| path.parent())
        .unwrap_or_else(|| Path::new("."));
    let journal = SourceCleanupJournal::at_path(journal_root.join(format!(
        ".source-cleanup-test-{}-{}/journal.json",
        std::process::id(),
        TRASH_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )));
    SourceCleanup::new(Arc::new(trash_adapter.clone()), Arc::new(journal)).complete(
        prepared.plan,
        outputs,
        &prepared.archived_inputs,
        is_cancelled,
        &squallz_core::api::NoProgress,
    )
}

fn test_manifest_entry(source_path: PathBuf, archive_path: EntryPath) -> CreateInputManifestEntry {
    CreateInputManifestEntry {
        source_path,
        archive_path,
        entry_type: squallz_core::api::EntryType::Dir,
        size: 0,
        modified: None,
        unix_mode: None,
        blake3: None,
    }
}

#[test]
fn archived_input_map_preserves_each_archive_path_for_a_repeated_source() {
    let source = PathBuf::from("source");
    let first_path = EntryPath::from_utf8("source");
    let second_path = EntryPath::from_utf8("nested/source");
    let archived = archived_input_map(&[
        test_manifest_entry(source.clone(), first_path.clone()),
        test_manifest_entry(source.clone(), second_path.clone()),
    ])
    .unwrap();

    assert_eq!(
        archived.get(&source).unwrap().archive_paths,
        vec![first_path, second_path]
    );
}

#[test]
fn archived_input_map_rejects_distinct_sources_for_one_archive_path() {
    let archive_path = EntryPath::from_utf8("shared");
    let result = archived_input_map(&[
        test_manifest_entry(PathBuf::from("first"), archive_path.clone()),
        test_manifest_entry(PathBuf::from("second"), archive_path),
    ]);

    assert!(matches!(result, Err(FingerprintError::Unavailable)));
}

#[test]
fn archived_input_map_rejects_inexact_archive_paths() {
    let inexact_paths = [
        EntryPath::from_raw(b"raw-name".to_vec(), "display-name".to_owned(), "utf-8"),
        EntryPath::from_raw(vec![0xff], "replacement-name".to_owned(), "utf-8"),
        EntryPath::from_raw(b"legacy-name".to_vec(), "legacy-name".to_owned(), "GBK"),
    ];

    for archive_path in inexact_paths {
        let result =
            archived_input_map(&[test_manifest_entry(PathBuf::from("source"), archive_path)]);
        assert!(matches!(result, Err(FingerprintError::Unavailable)));
    }
}

#[cfg(unix)]
#[test]
fn archived_input_map_rejects_a_non_utf8_source_path() {
    use std::os::unix::ffi::OsStringExt;

    let source = PathBuf::from(std::ffi::OsString::from_vec(
        b"source-with-invalid-utf8-\xff".to_vec(),
    ));
    assert!(source.to_str().is_none());

    let result = archived_input_map(&[test_manifest_entry(source, EntryPath::from_utf8("source"))]);

    assert!(matches!(result, Err(FingerprintError::Unavailable)));
}

#[cfg(unix)]
#[test]
fn archived_entry_verification_rejects_a_lossy_symlink_target_match() {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    let dir = temp_dir("source-cleanup-non-utf8-symlink-target");
    let source = dir.join("source-link");
    let target = PathBuf::from(std::ffi::OsString::from_vec(
        b"target-with-invalid-utf8-\xff".to_vec(),
    ));
    assert!(target.to_str().is_none());
    symlink(&target, &source).unwrap();
    let metadata = fs::symlink_metadata(&source).unwrap();
    let expected = ArchivedSourceEntry {
        archive_paths: vec![EntryPath::from_utf8("source-link")],
        entry_type: squallz_core::api::EntryType::Symlink {
            target: target.to_string_lossy().into_owned().into_bytes(),
        },
        size: 0,
        modified: metadata.modified().ok().map(CreateInputModifiedTime::from),
        unix_mode: Some(metadata.mode()),
        blake3: None,
    };
    let mut verified_bytes = 0;

    let verified = verify_archived_entry(
        &source,
        &expected,
        &mut verified_bytes,
        0,
        &|| false,
        &squallz_core::api::NoProgress,
    )
    .unwrap();

    assert!(!verified);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_moves_deduplicated_top_level_inputs() {
    let dir = temp_dir("source-cleanup-complete");
    let source_dir = dir.join("source");
    let nested = source_dir.join("nested.txt");
    let second = dir.join("second.txt");
    let output = dir.join("archive.zip");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(&nested, b"nested").unwrap();
    std::fs::write(&second, b"second").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let fake = FakeTrashAdapter::default();

    let plan = prepare_cleanup(
        &[source_dir.clone(), nested, source_dir, second],
        PostSuccessAction::TrashSource,
        &[],
    );
    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(
        result,
        SourceCleanupResult {
            status: SourceCleanupStatus::Completed,
            moved: 2,
            kept: 0,
            recovery_required: 0,
        }
    );
    assert_eq!(fake.calls().len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_blocks_output_inside_source_before_trashing() {
    let dir = temp_dir("source-cleanup-blocked");
    let source = dir.join("source");
    let output = source.join("archive.zip");
    std::fs::create_dir_all(&source).unwrap();
    let plan = prepare_cleanup(&[source], PostSuccessAction::TrashSource, &[]);
    std::fs::write(&output, b"archive").unwrap();
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(
        result,
        SourceCleanupResult {
            status: SourceCleanupStatus::Blocked,
            moved: 0,
            kept: 1,
            recovery_required: 0,
        }
    );
    assert!(fake.calls().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_blocks_a_source_changed_after_preflight() {
    let dir = temp_dir("source-cleanup-changed");
    let source = dir.join("source.txt");
    let output = dir.join("archive.zip");
    std::fs::write(&source, b"before").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let plan = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );
    std::fs::write(&source, b"changed after create preflight").unwrap();
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(result.status, SourceCleanupStatus::Blocked);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"changed after create preflight"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_reports_failed_when_preflight_cannot_freeze_source() {
    let dir = temp_dir("source-cleanup-failed");
    let missing = dir.join("missing.txt");
    let output = dir.join("archive.zip");
    std::fs::write(&output, b"archive").unwrap();
    let fake = FakeTrashAdapter::default();

    let plan = prepare_cleanup(&[missing], PostSuccessAction::TrashSource, &[]);
    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(result.status, SourceCleanupStatus::Failed);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn source_cleanup_uses_the_writer_symlink_target_as_authority() {
    use std::os::unix::fs::symlink;

    let dir = temp_dir("source-cleanup-symlink-manifest");
    let first_target = dir.join("first.txt");
    let second_target = dir.join("second.txt");
    let source = dir.join("source-link");
    let output = dir.join("archive.zip");
    std::fs::write(&first_target, b"first").unwrap();
    std::fs::write(&second_target, b"second").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    symlink("first.txt", &source).unwrap();
    let mut prepared = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );
    let entry = prepared.archived_inputs.first_mut().unwrap();
    entry.entry_type = squallz_core::api::EntryType::Symlink {
        target: b"second.txt".to_vec(),
    };
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(prepared, &[output], &fake, &|| false);

    assert_eq!(result.status, SourceCleanupStatus::Blocked);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    assert_eq!(
        std::fs::read_link(&source).unwrap(),
        PathBuf::from("first.txt")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_blocks_a_directory_with_changed_child_content() {
    let dir = temp_dir("source-cleanup-directory-changed");
    let source = dir.join("source");
    let child = source.join("child.txt");
    let output = dir.join("archive.zip");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(&child, b"before").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let plan = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );
    std::fs::write(&child, b"changed after archive preparation").unwrap();
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(result.status, SourceCleanupStatus::Blocked);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    assert!(source.exists());
    assert_eq!(
        std::fs::read(&child).unwrap(),
        b"changed after archive preparation"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_is_blocked_when_archive_excludes_are_active() {
    let dir = temp_dir("source-cleanup-excludes");
    let source = dir.join("source");
    let output = dir.join("archive.zip");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("photo.jpg"), b"included").unwrap();
    std::fs::write(source.join("original.raw"), b"excluded").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let plan = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &["*.raw".to_owned()],
    );
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| false);

    assert_eq!(result.status, SourceCleanupStatus::Blocked);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    assert!(source.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_content_verification_rejects_different_bytes() {
    let dir = temp_dir("source-cleanup-content-fingerprint");
    let source = dir.join("source.bin");
    std::fs::write(&source, b"first payload").unwrap();
    let expected = *blake3::hash(b"first payload").as_bytes();

    std::fs::write(&source, b"other payload").unwrap();
    let mut verified_bytes = 0;
    let unchanged = verify_archived_file(
        &source,
        b"first payload".len() as u64,
        &expected,
        &mut verified_bytes,
        b"first payload".len() as u64,
        &|| false,
        &squallz_core::api::NoProgress,
    )
    .unwrap();

    assert!(!unchanged);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_restore_collision_preserves_the_verified_source_beside_it() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-restore-collision")).unwrap();
    let original = dir.join("source.txt");
    let holder = dir.join(".squallz-trash-hold-test");
    let staged_path = holder.join("source.txt");
    std::fs::create_dir(&holder).unwrap();
    std::fs::write(&original, b"verified source").unwrap();
    let record = SourceCleanupRecord::new(original.clone(), staged_path, holder.clone()).unwrap();
    let preserved = record.preserved_path().to_path_buf();
    let journal = SourceCleanupJournal::at_path(dir.join("config/source-cleanup.json"));
    let pending = journal.begin(&record).unwrap();
    squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
    pending.sync_after_stage().unwrap();
    std::fs::write(&original, b"late competitor").unwrap();
    let staged = StagedCleanupCandidate { pending };

    let outcome = restore_staged_cleanup(staged, false);

    assert_eq!(outcome, StagedRestoreOutcome::Preserved);
    assert_eq!(std::fs::read(&original).unwrap(), b"late competitor");
    assert_eq!(std::fs::read(preserved).unwrap(), b"verified source");
    assert!(!holder.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn interrupted_cleanup_remains_visible_to_every_observer() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-startup-restore")).unwrap();
    let parent = dir.join("sources");
    let original = parent.join("source.txt");
    let holder = parent.join(".squallz-trash-hold-startup");
    let staged = holder.join("source.txt");
    fs::create_dir_all(&holder).unwrap();
    fs::write(&original, b"source").unwrap();
    let record = SourceCleanupRecord::new(original.clone(), staged, holder).unwrap();
    let journal = Arc::new(SourceCleanupJournal::at_path(
        dir.join("config/source-cleanup.json"),
    ));
    let pending = journal.begin(&record).unwrap();
    squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
    pending.sync_after_stage().unwrap();
    drop(pending);

    let cleanup = SourceCleanup::new(Arc::new(FakeTrashAdapter::default()), Arc::clone(&journal));

    assert_eq!(fs::read(&original).unwrap(), b"source");
    let notice = cleanup.notice().unwrap();
    assert_eq!(notice.generation, 1);
    assert_eq!(notice.status, "restored");
    assert_eq!(notice.path.as_deref(), original.to_str());
    assert_eq!(cleanup.notice(), Some(notice));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stage_collision_publishes_recovery_after_cached_none_or_notice() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-late-journal")).unwrap();
    let journal = Arc::new(SourceCleanupJournal::at_path(
        dir.join("config/source-cleanup.json"),
    ));
    let fake = Arc::new(FakeTrashAdapter::default());
    let cleanup = SourceCleanup::new(fake.clone(), Arc::clone(&journal));
    assert_eq!(cleanup.notice(), None);

    for attempt in 1..=2 {
        let stale_parent = dir.join(format!("stale-{attempt}"));
        let stale_original = stale_parent.join("source.txt");
        let stale_holder = stale_parent.join(format!("{HOLDER_PREFIX}stale"));
        fs::create_dir_all(&stale_holder).unwrap();
        fs::write(&stale_original, b"stale source").unwrap();
        let stale_record = SourceCleanupRecord::new(
            stale_original.clone(),
            stale_holder.join("source.txt"),
            stale_holder,
        )
        .unwrap();
        let pending = journal.begin(&stale_record).unwrap();
        squallz_core::move_path_no_replace(&stale_record.original, &stale_record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        let source = dir.join(format!("source-{attempt}.txt"));
        let output = dir.join(format!("archive-{attempt}.zip"));
        fs::write(&source, b"new source").unwrap();
        fs::write(&output, b"archive").unwrap();
        let prepared = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        let result = cleanup.complete(
            prepared.plan,
            &[output],
            &prepared.archived_inputs,
            &|| false,
            &squallz_core::api::NoProgress,
        );

        assert_eq!(result.status, SourceCleanupStatus::Failed);
        assert!(source.exists());
        assert!(fake.calls().is_empty());
        let notice = cleanup.notice().unwrap();
        assert_eq!(notice.generation, attempt);
        assert_eq!(notice.status, "restored");
        assert_eq!(notice.path.as_deref(), stale_original.to_str());
        assert_eq!(cleanup.notice(), Some(notice));
    }

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn active_cleanup_lock_blocks_trash_and_reports_busy() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-manager-busy")).unwrap();
    let active_parent = dir.join("active");
    let active_original = active_parent.join("active.txt");
    let active_holder = active_parent.join(".squallz-trash-hold-active");
    fs::create_dir_all(&active_holder).unwrap();
    fs::write(&active_original, b"active").unwrap();
    let active_record = SourceCleanupRecord::new(
        active_original,
        active_holder.join("active.txt"),
        active_holder,
    )
    .unwrap();
    let journal = Arc::new(SourceCleanupJournal::at_path(
        dir.join("config/source-cleanup.json"),
    ));
    let pending = journal.begin(&active_record).unwrap();
    let fake = Arc::new(FakeTrashAdapter::default());
    let cleanup = SourceCleanup::new(fake.clone(), Arc::clone(&journal));
    let initial = cleanup.notice().unwrap();
    assert_eq!(initial.status, "busy");

    let source = dir.join("source.txt");
    let output = dir.join("archive.zip");
    fs::write(&source, b"source").unwrap();
    fs::write(&output, b"archive").unwrap();
    let prepared = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );
    let result = cleanup.complete(
        prepared.plan,
        &[output],
        &prepared.archived_inputs,
        &|| false,
        &squallz_core::api::NoProgress,
    );

    assert_eq!(result.status, SourceCleanupStatus::Failed);
    assert!(source.exists());
    assert!(fake.calls().is_empty());
    let busy = cleanup.notice().unwrap();
    assert_eq!(busy.generation, initial.generation + 1);
    assert_eq!(busy.status, "busy");
    assert_eq!(cleanup.notice(), Some(busy));

    drop(pending);
    let cleared = cleanup.notice().unwrap();
    assert_eq!(cleared.generation, initial.generation + 2);
    assert_eq!(cleared.status, "cleared");
    assert!(active_record.original.exists());
    assert!(!active_record.holder.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_cleanup_journal_blocks_trash_and_requests_attention() {
    let dir = fs::canonicalize(temp_dir("source-cleanup-manager-corrupt")).unwrap();
    let journal_path = dir.join("config/source-cleanup.json");
    fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    fs::write(&journal_path, b"{\"version\":1").unwrap();
    let journal = Arc::new(SourceCleanupJournal::at_path(journal_path.clone()));
    let fake = Arc::new(FakeTrashAdapter::default());
    let cleanup = SourceCleanup::new(fake.clone(), Arc::clone(&journal));
    let initial = cleanup.notice().unwrap();
    assert_eq!(initial.status, "needs_attention");
    let source = dir.join("source.txt");
    let output = dir.join("archive.zip");
    fs::write(&source, b"source").unwrap();
    fs::write(&output, b"archive").unwrap();
    let prepared = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );

    let result = cleanup.complete(
        prepared.plan,
        &[output],
        &prepared.archived_inputs,
        &|| false,
        &squallz_core::api::NoProgress,
    );

    assert_eq!(result.status, SourceCleanupStatus::Failed);
    assert!(source.exists());
    assert!(fake.calls().is_empty());
    let notice = cleanup.notice().unwrap();
    assert_eq!(notice.generation, initial.generation + 1);
    assert_eq!(notice.status, "needs_attention");
    assert_eq!(notice.reason.as_deref(), Some("journal_invalid"));
    assert_eq!(notice.journal_path.as_deref(), journal_path.to_str());
    assert_eq!(notice.path, None);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_preflight_fingerprint_observes_cancellation() {
    let dir = temp_dir("source-cleanup-preflight-cancel");
    let source = dir.join("source.bin");
    std::fs::write(&source, deterministic_payload(1024 * 1024)).unwrap();
    let result = prepare_source_cleanup(
        &[source],
        PostSuccessAction::TrashSource,
        &[],
        &|| true,
        &squallz_core::api::NoProgress,
    );

    assert!(matches!(result, Err(FormatError::Cancelled)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_postcheck_cancellation_prevents_trash() {
    let dir = temp_dir("source-cleanup-postcheck-cancel");
    let source = dir.join("source.bin");
    let output = dir.join("archive.zip");
    std::fs::write(&source, deterministic_payload(1024 * 1024)).unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let plan = prepare_cleanup(
        std::slice::from_ref(&source),
        PostSuccessAction::TrashSource,
        &[],
    );
    let checkpoints = std::sync::atomic::AtomicUsize::new(0);
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| {
        checkpoints.fetch_add(1, Ordering::Relaxed) >= 5
    });

    assert_eq!(result.status, SourceCleanupStatus::Cancelled);
    assert_eq!(result.moved, 0);
    assert_eq!(result.kept, 1);
    assert!(fake.calls().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn source_cleanup_stops_moving_sources_after_cancellation() {
    let dir = temp_dir("source-cleanup-cancelled");
    let first = dir.join("first.txt");
    let second = dir.join("second.txt");
    let output = dir.join("archive.zip");
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&second, b"second").unwrap();
    std::fs::write(&output, b"archive").unwrap();
    let plan = prepare_cleanup(&[first, second], PostSuccessAction::TrashSource, &[]);
    let fake = FakeTrashAdapter::default();

    let result = finish_cleanup(plan, &[output], &fake, &|| !fake.calls().is_empty());

    assert_eq!(result.status, SourceCleanupStatus::Cancelled);
    assert_eq!(result.moved, 1);
    assert_eq!(result.kept, 1);
    assert_eq!(fake.calls().len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
