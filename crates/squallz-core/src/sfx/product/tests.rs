use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use super::*;
use crate::replacement::test_hooks::{self, Event};
use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CreateOptions, EntryMeta, EntryPath,
    FormatCapabilities, FormatRegistry, NoProgress, OpenOptions as ArchiveOpenOptions, ReadSeek,
    TestSummary, WriteSeek,
};

struct TestStage {
    path: PathBuf,
    held: File,
    layout: SfxLayout,
}
impl TestStage {
    fn input(&self) -> OwnedInput {
        let file = self.held.try_clone().unwrap();
        let identity = file_identity(&file).unwrap();
        assert_eq!(path_identity(&self.path).unwrap(), identity);
        match self.layout {
            SfxLayout::SingleFile => OwnedInput::SfxFile {
                path: self.path.clone(),
                file,
                identity,
            },
            SfxLayout::MacosApp => OwnedInput::SfxTree {
                path: self.path.clone(),
                root: file,
                identity,
            },
        }
    }
}
fn test_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "squallz-sfx-product-{tag}-{}-{}",
        std::process::id(),
        STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}
fn setup(tag: &str, previous: Option<&[u8]>) -> (PathBuf, PathBuf) {
    let dir = test_dir(tag);
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    let target = dir.join("package.exe");
    if let Some(previous) = previous {
        fs::write(&target, previous).unwrap();
    }
    (dir, target)
}
fn staged_file(destination: &Path, bytes: &[u8]) -> TestStage {
    let mut reserved = reserve_file_stage(destination, TemporaryKind::Stage).unwrap();
    reserved.file.write_all(bytes).unwrap();
    reserved.file.sync_all().unwrap();
    #[cfg(windows)]
    {
        let label = public_output_label(destination);
        let control = ControlToken::default();
        let context = VerificationContext {
            public_label: &label,
            progress: &NoProgress,
            control: &control,
        };
        seal_staging_file(
            &reserved.path,
            &mut reserved.file,
            reserved.identity,
            &context,
        )
        .unwrap();
    }
    TestStage {
        path: reserved.path,
        held: reserved.file,
        layout: SfxLayout::SingleFile,
    }
}
fn staged_app(destination: &Path, relative: &Path, bytes: &[u8]) -> TestStage {
    let (path, identity) = reserve_bundle_stage(destination).unwrap();
    let held = super::super::bundle::open_bundle_root(&path).unwrap();
    assert_eq!(file_identity(&held).unwrap(), identity);
    let member = path.join(relative);
    fs::create_dir_all(member.parent().unwrap()).unwrap();
    let mut output = File::create(member).unwrap();
    output.write_all(bytes).unwrap();
    output.sync_all().unwrap();
    drop(output);
    let tree = super::super::bundle_tree::BundleTree::new(&held, &path).unwrap();
    for directory in relative.ancestors().skip(1) {
        tree.sync_dir(directory).unwrap();
    }
    TestStage {
        path,
        held,
        layout: SfxLayout::MacosApp,
    }
}
fn reserve_disposal_stage(
    destination: &Path,
    layout: SfxLayout,
) -> Result<(PathBuf, PathIdentity), FormatError> {
    match layout {
        SfxLayout::SingleFile => {
            let reserved = reserve_file_stage(destination, TemporaryKind::Stage)?;
            Ok((reserved.path, reserved.identity))
        }
        SfxLayout::MacosApp => reserve_bundle_stage(destination),
    }
}
fn publish(
    stage: &TestStage,
    destination: &Path,
    policy: CreateCommitPolicy,
) -> Result<Vec<PathBuf>, FormatError> {
    let label = public_output_label(destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    publish_owned(stage.input(), destination, stage.layout, policy, &context)
}
fn journal_path(destination: &Path) -> PathBuf {
    Scope::inspect_sfx(destination)
        .unwrap()
        .anchor(RecordPhase::Installing)
}
fn completion_path(destination: &Path) -> PathBuf {
    Scope::inspect_sfx(destination)
        .unwrap()
        .anchor(RecordPhase::Completed)
}
fn begin_test_replacement(
    destination: &Path,
    stage: &TestStage,
) -> replacement::Replacement<'static> {
    let scope = Scope::sfx_directory(destination)
        .unwrap()
        .lock_target(destination)
        .unwrap_or_else(|f| panic!("{}", f.error));
    let previous = open_previous(&scope, stage.layout).unwrap();
    let label = public_output_label(destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    let prepared = replacement::prepare(
        scope,
        previous,
        stage.input(),
        Authorization::ContentBound,
        &context,
    )
    .unwrap_or_else(|f| panic!("{}", f.error));
    match replacement::persist(prepared) {
        Ok(owner) => owner,
        Err(PersistFailure::Unpublished { error, .. }) => panic!("{error}"),
        Err(PersistFailure::Retained(failure)) => panic!("{}", failure.error),
    }
}
fn resume(
    owner: replacement::Replacement<'static>,
    destination: &Path,
) -> Result<replacement::Installed<'static>, Failure> {
    let label = public_output_label(destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    replacement::commit_resume(owner, &context)
}
#[cfg(unix)]
fn rewrite_record_in_place(path: &Path) {
    let identity = path_identity(path).unwrap();
    let mut bytes = fs::read(path).unwrap();
    bytes[0] ^= 1;
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    assert_eq!(path_identity(path).unwrap(), identity);
}
fn acknowledge_completed_backup(error: &FormatError, destination: &Path) -> Vec<u8> {
    assert!(!sfx_recovery_requires_staging(error));
    let details = sfx_recovery_details(error).unwrap();
    let completion = completion_path(destination);
    assert!(details.paths.contains(&completion));
    let previous = details
        .paths
        .iter()
        .find(|path| path.file_name() == Some(OsStr::new("previous")) && path.is_file())
        .unwrap();
    let bytes = fs::read(previous).unwrap();
    fs::remove_file(previous).unwrap();
    preflight_destination(destination).unwrap();
    assert!(!completion.exists());
    bytes
}
fn cleanup(dir: &Path) {
    fs::remove_dir_all(dir).unwrap();
}

fn installation_move(from: &Path, to: &Path, destination: &Path) -> bool {
    matches!(
        to.file_name().and_then(OsStr::to_str),
        Some("replacement" | "previous")
    ) || (from.file_name() == Some(OsStr::new("replacement")) && to == destination)
}

fn interrupt_after_move(destination: &Path, ordinal: usize) -> test_hooks::Guard {
    let destination = destination.to_path_buf();
    let mut moves = 0;
    test_hooks::install(Box::new(move |event| {
        if let Event::AfterMove { from, to } = event {
            if installation_move(from, to, &destination) {
                moves += 1;
                if moves == ordinal {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated process interruption",
                    ));
                }
            }
        }
        Ok(())
    }))
}

#[cfg(unix)]
#[test]
fn same_inode_journal_rewrite_blocks_every_transaction_move() {
    let (dir, destination) = setup("same-inode-before-resume", Some(b"previous output"));
    let staged = staged_file(&destination, b"replacement output");
    let owner = begin_test_replacement(&destination, &staged);
    let journal = owner.record.path.clone();
    let holder = owner.bindings.layout.holder(&owner.bindings.scope);
    rewrite_record_in_place(&journal);
    let moves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&moves);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeMove { .. }) {
            observed.set(observed.get() + 1);
        }
        Ok(())
    }));
    let error = resume(owner, &destination).err().unwrap();
    drop(hook);
    assert_eq!(moves.get(), 0);
    assert!(error.error.to_string().contains("changed"));
    assert_eq!(fs::read(&destination).unwrap(), b"previous output");
    assert_eq!(fs::read(&staged.path).unwrap(), b"replacement output");
    assert!(journal.exists());
    assert!(holder.exists());
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn same_inode_journal_rewrite_blocks_completion_publication() {
    let (dir, destination) = setup("same-inode-before-completion", Some(b"previous output"));
    let staged = staged_file(&destination, b"replacement output");
    let owner = begin_test_replacement(&destination, &staged);
    let installed = resume(owner, &destination).unwrap_or_else(|f| panic!("{}", f.error));
    let owner = &installed.owner;
    let journal = owner.record.path.clone();
    let holder = owner.bindings.layout.holder(&owner.bindings.scope);
    let previous = owner.bindings.layout.previous(&owner.bindings.scope);
    rewrite_record_in_place(&journal);
    let label = public_output_label(&destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    let failure = replacement::complete(installed, &context).err().unwrap();
    assert!(failure.error.to_string().contains("changed"));
    assert!(journal.exists());
    assert!(!completion_path(&destination).exists());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement output");
    assert_eq!(fs::read(&previous).unwrap(), b"previous output");
    assert!(holder.exists());
    cleanup(&dir);
}

#[test]
fn guarded_single_file_publish_rejects_same_length_content_changes_before_any_move() {
    let (dir, destination) = setup("guarded-content", Some(b"old-data"));
    let guard = crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
        .unwrap()
        .guard
        .unwrap();
    fs::write(&destination, b"new-data").unwrap();
    let staged = staged_file(&destination, b"replacement");
    let moves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&moves);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeMove { .. }) {
            observed.set(observed.get() + 1);
        }
        Ok(())
    }));
    let error = publish(
        &staged,
        &destination,
        CreateCommitPolicy::ReplaceIfUnchanged(guard),
    )
    .unwrap_err();
    drop(hook);
    assert!(error.is_destination_changed());
    assert_eq!(moves.get(), 0);
    assert_eq!(fs::read(&destination).unwrap(), b"new-data");
    assert_eq!(fs::read(&staged.path).unwrap(), b"replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn guarded_app_publish_rejects_deep_tree_changes_before_any_move() {
    let (dir, _) = setup("guarded-deep", None);
    let destination = dir.join("Package.app");
    let nested = Path::new("Contents/Resources/config/runtime.dat");
    fs::create_dir_all(destination.join(nested).parent().unwrap()).unwrap();
    fs::write(destination.join(nested), b"old-data").unwrap();
    let guard = crate::inspect_create_destination(&destination, CreateArtifactKind::SfxMacosApp)
        .unwrap()
        .guard
        .unwrap();
    fs::write(destination.join(nested), b"new-data").unwrap();
    let staged = staged_app(&destination, nested, b"replacement");
    let moves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&moves);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeMove { .. }) {
            observed.set(observed.get() + 1);
        }
        Ok(())
    }));
    let error = publish(
        &staged,
        &destination,
        CreateCommitPolicy::ReplaceIfUnchanged(guard),
    )
    .unwrap_err();
    drop(hook);
    assert!(error.is_destination_changed());
    assert_eq!(moves.get(), 0);
    assert_eq!(fs::read(destination.join(nested)).unwrap(), b"new-data");
    assert_eq!(fs::read(staged.path.join(nested)).unwrap(), b"replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn recovery_rejects_same_identity_content_changes_after_a_crash() {
    let (dir, destination) = setup("recovery-content", Some(b"previous"));
    let staged = staged_file(&destination, b"replacement");
    let previous = Rc::new(RefCell::new(None));
    let observed = Rc::clone(&previous);
    let write_blocked = Rc::new(Cell::new(false));
    #[cfg(windows)]
    let blocked_for_hook = Rc::clone(&write_blocked);
    let hook = test_hooks::install(Box::new(move |event| {
        if let Event::AfterMove { to, .. } = event {
            if to.file_name() == Some(OsStr::new("previous")) {
                #[cfg(not(windows))]
                fs::write(to, b"tampered")?;
                #[cfg(windows)]
                if let Err(error) = fs::write(to, b"tampered") {
                    assert_eq!(
                        error.raw_os_error(),
                        Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32)
                    );
                    blocked_for_hook.set(true);
                }
                observed.replace(Some(to.to_path_buf()));
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "simulated crash after same-inode content change",
                ));
            }
        }
        Ok(())
    }));
    let interruption =
        publish(&staged, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_details(&interruption).is_some());
    let error = preflight_destination(&destination).unwrap_err();
    let previous = previous.borrow().clone().unwrap();
    if write_blocked.get() {
        assert_eq!(fs::read(&previous).unwrap(), b"previous");
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_eq!(
            acknowledge_completed_backup(&error, &destination),
            b"previous"
        );
        cleanup(&dir);
        return;
    }
    assert!(!destination.exists());
    assert_eq!(fs::read(&previous).unwrap(), b"tampered");
    assert!(journal_path(&destination).exists());
    assert!(!completion_path(&destination).exists());
    assert!(sfx_recovery_details(&error)
        .unwrap()
        .paths
        .contains(&previous));
    assert!(error.to_string().contains("changed"));
    cleanup(&dir);
}

#[test]
fn completed_transaction_rechecks_backup_content_before_cleanup() {
    let (dir, destination) = setup("completed-content", Some(b"previous"));
    let staged = staged_file(&destination, b"replacement");
    let preserved = publish(&staged, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    fs::write(&preserved[0], b"tampered").unwrap();
    let error = preflight_destination(&destination).unwrap_err();
    assert!(!sfx_recovery_requires_staging(&error));
    assert!(error.to_string().contains("changed"));
    assert!(completion_path(&destination).exists());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"tampered");
    cleanup(&dir);
}

#[test]
fn artifact_matcher_only_accepts_exact_destination_transaction_names() {
    let (dir, destination) = setup("artifact-names", Some(b"previous"));
    let staged = staged_file(&destination, b"replacement");
    let owner = begin_test_replacement(&destination, &staged);
    let scope = &owner.bindings.scope;
    let holder = owner.bindings.layout.holder(scope);
    for path in [
        owner.record.path.clone(),
        staged.path.clone(),
        holder.clone(),
        holder.join("previous"),
        holder.join("replacement"),
        scope.reserve_name("journal"),
    ] {
        assert!(scope.reserved_artifact(&path));
    }
    for path in [
        dir.join(".squallz-sfx-not-the-destination-10-1"),
        dir.join(".squallz-sfx-holder-0-1"),
        dir.join(".squallz-sfx-holder-10-1-extra"),
        dir.join(".other.exe.sfx-0.tmp.other.exe"),
        holder.join("unrelated"),
    ] {
        assert!(!scope.reserved_artifact(&path));
    }
    drop(owner);
    cleanup(&dir);
}

#[test]
fn artifact_matcher_rejects_ordinary_entries_before_filesystem_resolution() {
    assert!(
        !classify_sfx_transaction_artifact(Path::new("/another-missing-parent/readme.txt"))
            .unwrap()
    );
    assert!(!classify_sfx_transaction_artifact(Path::new("relative/missing/photos")).unwrap());
}

#[test]
fn pending_transaction_is_recovered_through_its_recorded_case_alias() {
    let (dir, _) = setup("case-alias-recovery", None);
    if !case_aliases_share_an_entry(&dir) {
        cleanup(&dir);
        return;
    }
    let destination = dir.join("Package.exe");
    let alias = dir.join("package.exe");
    fs::write(&destination, b"previous").unwrap();
    let first = staged_file(&alias, b"first replacement");
    let hook = interrupt_after_move(&destination, 2);
    let error = publish(&first, &alias, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert_eq!(
        sfx_recovery_details(&error).unwrap().target.file_name(),
        Some(OsStr::new("Package.exe"))
    );
    assert!(!destination.exists());
    assert!(journal_path(&alias).exists());
    let second = staged_file(&alias, b"second replacement");
    let pending = publish(&second, &alias, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    assert_eq!(acknowledge_completed_backup(&pending, &alias), b"previous");
    let preserved = publish(&second, &alias, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn absent_case_alias_publishers_share_directory_and_destination_locks() {
    use std::sync::Barrier;
    let (dir, _) = setup("case-alias-concurrency", None);
    if !case_aliases_share_an_entry(&dir) {
        cleanup(&dir);
        return;
    }
    let destination = dir.join("Package.exe");
    let alias = dir.join("package.exe");
    let upper = staged_file(&destination, b"upper replacement");
    let lower = staged_file(&alias, b"lower replacement");
    let barrier = Arc::new(Barrier::new(3));
    let gate = Arc::clone(&barrier);
    let target = destination.clone();
    let upper_job = std::thread::spawn(move || {
        gate.wait();
        publish(&upper, &target, CreateCommitPolicy::ReplaceExisting)
    });
    let gate = Arc::clone(&barrier);
    let lower_job = std::thread::spawn(move || {
        gate.wait();
        publish(&lower, &alias, CreateCommitPolicy::ReplaceExisting)
    });
    barrier.wait();
    let mut outputs = upper_job.join().unwrap().unwrap();
    outputs.extend(lower_job.join().unwrap().unwrap());
    outputs.push(destination.clone());
    let mut contents = outputs
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();
    contents.sort();
    assert_eq!(
        contents,
        vec![b"lower replacement".to_vec(), b"upper replacement".to_vec()]
    );
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn repeated_sfx_rebuilds_exclude_owned_transaction_artifacts_from_source_payload() {
    let dir = test_dir("source-exclusions");
    let source = dir.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload.txt"), b"user payload").unwrap();
    let destination = source.join("package.exe");
    let stub = write_test_pe_stub(&dir);
    let engine = test_sfx_engine();
    let options = CreateOptions::default();
    let sfx_options = crate::SfxBuildOptions {
        target: crate::SfxTarget::Windows,
        overwrite: true,
        ..crate::SfxBuildOptions::default()
    };
    let build = || {
        engine.create_sfx_from_inputs_with_verification(
            &stub,
            std::slice::from_ref(&source),
            &destination,
            &options,
            &sfx_options,
            &NoProgress,
            &ControlToken::new(),
        )
    };
    let assert_clean = |manifest: &[crate::CreateInputManifestEntry]| {
        assert!(manifest
            .iter()
            .any(|entry| entry.archive_path.to_string().ends_with("payload.txt")));
        assert!(manifest
            .iter()
            .all(|entry| !has_sfx_artifact_shape(&entry.source_path)
                && !crate::same_path_entry(&destination, &entry.source_path)));
    };
    assert_clean(&build().unwrap().manifest);
    let stage = staged_file(&destination, b"interrupted replacement");
    let hook = test_hooks::install(Box::new(|event| {
        if let Event::BeforeMove { to, .. } = event {
            if to.file_name() == Some(OsStr::new("replacement")) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "simulated stop before replacement move",
                ));
            }
        }
        Ok(())
    }));
    let interruption =
        publish(&stage, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    let recovery = sfx_recovery_details(&interruption).unwrap();
    assert!(recovery.paths.iter().all(|path| path.exists()));
    assert!(recovery
        .paths
        .iter()
        .all(|path| has_sfx_artifact_shape(path)));
    let pending = build().unwrap_err();
    assert!(!acknowledge_completed_backup(&pending, &destination).is_empty());
    for rebuild in 0..3 {
        let report = build().unwrap();
        assert_clean(&report.manifest);
        assert_eq!(report.sfx.preserved_outputs.len(), 1);
        assert!(report
            .sfx
            .preserved_outputs
            .iter()
            .all(|path| path.is_file()));
        if rebuild < 2 {
            fs::remove_file(&report.sfx.preserved_outputs[0]).unwrap();
        }
    }
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn interrupted_backup_move_is_resumed_by_the_next_exact_destination_publish() {
    let (dir, destination) = setup("resume-backup", Some(b"previous"));
    let first = staged_file(&destination, b"first replacement");
    let hook = interrupt_after_move(&destination, 2);
    let error = publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_details(&error).is_some());
    assert!(journal_path(&destination).exists());
    let second = staged_file(&destination, b"second replacement");
    let pending = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    assert_eq!(
        acknowledge_completed_backup(&pending, &destination),
        b"previous"
    );
    let preserved = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn first_transaction_move_failure_keeps_staging_and_recovers_on_the_next_publish() {
    let (dir, destination) = setup("first-move-failure", Some(b"previous"));
    let first = staged_file(&destination, b"first replacement");
    let hook = test_hooks::install(Box::new(|event| {
        if let Event::BeforeMove { to, .. } = event {
            if to.file_name() == Some(OsStr::new("replacement")) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated first replacement move failure",
                ));
            }
        }
        Ok(())
    }));
    let error = publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_requires_staging(&error));
    let details = sfx_recovery_details(&error).unwrap();
    assert!(details.paths.iter().all(|path| path.exists()));
    assert!(details.paths.contains(&first.path));
    assert!(details.paths.contains(&journal_path(&destination)));
    assert!(details.paths.iter().any(|path| path.is_dir()));
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    assert_eq!(fs::read(&first.path).unwrap(), b"first replacement");
    let second = staged_file(&destination, b"second replacement");
    let pending = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    assert_eq!(
        acknowledge_completed_backup(&pending, &destination),
        b"previous"
    );
    let preserved = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    cleanup(&dir);
}

#[test]
fn journal_parent_sync_failure_keeps_recoverable_state_for_the_next_publish() {
    let (dir, destination) = setup("journal-parent-sync", Some(b"previous"));
    let first = staged_file(&destination, b"first replacement");
    let journal = journal_path(&destination);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeSync) && journal.exists() {
            return Err(io::Error::other(
                "simulated parent sync failure after journal publication",
            ));
        }
        Ok(())
    }));
    let error = publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_requires_staging(&error));
    let details = sfx_recovery_details(&error).unwrap();
    assert!(details.paths.iter().all(|path| path.exists()));
    assert!(details.paths.contains(&first.path));
    assert!(details.paths.contains(&journal_path(&destination)));
    assert!(details.paths.iter().any(|path| path.is_dir()));
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    let second = staged_file(&destination, b"second replacement");
    let pending = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    assert_eq!(
        acknowledge_completed_backup(&pending, &destination),
        b"previous"
    );
    let preserved = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
#[cfg(any(unix, windows))]
fn duplicate_rename_identities_are_replayed_without_losing_either_output() {
    for ordinal in 1..=3 {
        let (dir, destination) = setup(&format!("duplicate-rename-{ordinal}"), Some(b"previous"));
        let stage = staged_file(&destination, b"replacement");
        let target = destination.clone();
        let mut moves = 0;
        let hook = test_hooks::install(Box::new(move |event| {
            if let Event::AfterMove { from, to } = event {
                if installation_move(from, to, &target) {
                    moves += 1;
                    if moves == ordinal {
                        fs::hard_link(to, from)?;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "simulated destination directory entry persisted first",
                        ));
                    }
                }
            }
            Ok(())
        }));
        let error = publish(&stage, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
        drop(hook);
        assert!(sfx_recovery_details(&error).is_some());
        let pending = preflight_destination(&destination).unwrap_err();
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_eq!(
            acknowledge_completed_backup(&pending, &destination),
            b"previous"
        );
        assert!(!journal_path(&destination).exists());
        cleanup(&dir);
    }
}

#[test]
fn new_destination_sync_failure_reports_the_installed_output_for_recovery() {
    let (dir, destination) = setup("direct-sync-failure", None);
    let first = staged_file(&destination, b"first replacement");
    let hook = test_hooks::install(Box::new(|event| {
        if matches!(event, Event::BeforeSync) {
            return Err(io::Error::other(
                "simulated parent sync failure after destination installation",
            ));
        }
        Ok(())
    }));
    let error = publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    let details = sfx_recovery_details(&error).unwrap();
    assert_eq!(details.target, destination);
    assert_eq!(details.paths, vec![destination.clone()]);
    assert!(error
        .to_string()
        .contains("simulated parent sync failure after destination installation"));
    assert!(!first.path.exists());
    assert!(!journal_path(&destination).exists());
    assert_eq!(fs::read(&destination).unwrap(), b"first replacement");
    let second = staged_file(&destination, b"second replacement");
    let preserved = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    cleanup(&dir);
}

#[test]
fn interrupted_final_install_returns_the_durable_previous_output_on_retry() {
    let (dir, destination) = setup("resume-installed", Some(b"previous"));
    let first = staged_file(&destination, b"first replacement");
    let hook = interrupt_after_move(&destination, 3);
    let error = publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    let details = sfx_recovery_details(&error).unwrap();
    assert_eq!(details.target, destination);
    assert!(details.paths.iter().all(|path| path.exists()));
    assert!(details.paths.contains(&journal_path(&destination)));
    let holder = details.paths.iter().find(|path| path.is_dir()).unwrap();
    assert!(details.paths.contains(&holder.join("previous")));
    let second = staged_file(&destination, b"second replacement");
    let pending = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    assert_eq!(
        acknowledge_completed_backup(&pending, &destination),
        b"previous"
    );
    let preserved = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap();
    assert_eq!(preserved.len(), 1);
    assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
    assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn changed_previous_output_identity_fails_closed_and_reports_every_current_path() {
    let (dir, destination) = setup("changed-previous-identity", Some(b"previous"));
    let competitor = dir.join("competitor.exe");
    fs::write(&competitor, b"competing entry").unwrap();
    let first = staged_file(&destination, b"first replacement");
    let previous = Rc::new(RefCell::new(None));
    let observed = Rc::clone(&previous);
    let hook = test_hooks::install(Box::new(move |event| {
        if let Event::AfterMove { to, .. } = event {
            if to.file_name() == Some(OsStr::new("previous")) {
                observed.replace(Some(to.to_path_buf()));
                fs::remove_file(to)?;
                crate::move_path_no_replace(&competitor, to)?;
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "simulated identity change after previous-output move",
                ));
            }
        }
        Ok(())
    }));
    let interruption =
        publish(&first, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_details(&interruption).is_some());
    let second = staged_file(&destination, b"second replacement");
    let error = publish(&second, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    let previous = previous.borrow().clone().unwrap();
    let details = sfx_recovery_details(&error).unwrap();
    assert!(!destination.exists());
    assert_eq!(fs::read(&previous).unwrap(), b"competing entry");
    assert_eq!(fs::read(&second.path).unwrap(), b"second replacement");
    assert!(details.paths.iter().all(|path| path.exists()));
    assert!(details.paths.contains(&previous));
    assert!(details.paths.contains(&journal_path(&destination)));
    assert!(details
        .paths
        .iter()
        .any(|path| path.file_name() == Some(OsStr::new("replacement"))
            && fs::read(path).unwrap() == b"first replacement"));
    cleanup(&dir);
}

#[test]
fn preserved_output_identity_is_rechecked_after_the_journal_is_cleared() {
    let (dir, destination) = setup("backup-rebind-after-clear", Some(b"previous"));
    let competitor = dir.join("competitor.exe");
    fs::write(&competitor, b"unrelated entry").unwrap();
    let staged = staged_file(&destination, b"replacement");
    let journal = journal_path(&destination);
    let completion = completion_path(&destination);
    let previous = Rc::new(RefCell::new(None::<PathBuf>));
    let observed = Rc::clone(&previous);
    let rebound = Rc::new(Cell::new(false));
    let changed = Rc::clone(&rebound);
    let hook = test_hooks::install(Box::new(move |event| {
        if let Event::AfterMove { to, .. } = event {
            if to.file_name() == Some(OsStr::new("previous")) {
                observed.replace(Some(to.to_path_buf()));
            }
        }
        if matches!(event, Event::BeforeSync)
            && !journal.exists()
            && completion.exists()
            && !changed.get()
        {
            let path = observed
                .borrow()
                .clone()
                .ok_or_else(|| io::Error::other("previous path was not captured"))?;
            fs::remove_file(&path)?;
            crate::move_path_no_replace(&competitor, &path)?;
            changed.set(true);
        }
        Ok(())
    }));
    let error = publish(&staged, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    let previous = previous.borrow().clone().unwrap();
    let details = sfx_recovery_details(&error).unwrap();
    assert!(rebound.get());
    assert_eq!(details.target, destination);
    assert!(details.paths.contains(&completion_path(&destination)));
    assert!(details.paths.contains(&previous));
    assert!(error.to_string().contains("changed"));
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert_eq!(fs::read(&previous).unwrap(), b"unrelated entry");
    assert!(!journal_path(&destination).exists());
    cleanup(&dir);
}

#[test]
fn oversized_exact_destination_journal_fails_closed_without_scanning_or_moving_outputs() {
    let (dir, destination) = setup("bounded-journal", Some(b"previous"));
    let staged = staged_file(&destination, b"replacement");
    let scope = Scope::inspect_sfx(&destination).unwrap();
    let journal = scope.anchor(RecordPhase::Installing);
    let limit = scope.limit();
    drop(scope);
    fs::write(&journal, vec![b'x'; limit + 1]).unwrap();
    sync_directory(&dir).unwrap();
    let moves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&moves);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeMove { .. }) {
            observed.set(observed.get() + 1);
        }
        Ok(())
    }));
    let error = publish(&staged, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    let details = sfx_recovery_details(&error).unwrap();
    assert_eq!(details.target, destination);
    assert!(details.paths.contains(&journal));
    assert_eq!(moves.get(), 0);
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    assert_eq!(fs::read(&staged.path).unwrap(), b"replacement");
    assert_eq!(fs::metadata(journal).unwrap().len(), (limit + 1) as u64);
    cleanup(&dir);
}

#[test]
fn journal_rejects_unknown_fields_without_moving_any_output() {
    let (dir, destination) = setup("strict-journal", Some(b"previous"));
    let staged = staged_file(&destination, b"replacement");
    let owner = begin_test_replacement(&destination, &staged);
    let journal = owner.record.path.clone();
    drop(owner);
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::Value::Bool(true));
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&journal)
        .unwrap();
    serde_json::to_writer(&file, &value).unwrap();
    file.sync_all().unwrap();
    drop(file);
    sync_directory(&dir).unwrap();
    let moves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&moves);
    let hook = test_hooks::install(Box::new(move |event| {
        if matches!(event, Event::BeforeMove { .. }) {
            observed.set(observed.get() + 1);
        }
        Ok(())
    }));
    let error = publish(&staged, &destination, CreateCommitPolicy::ReplaceExisting).unwrap_err();
    drop(hook);
    assert!(sfx_recovery_details(&error)
        .unwrap()
        .paths
        .contains(&journal));
    assert_eq!(moves.get(), 0);
    assert_eq!(fs::read(&destination).unwrap(), b"previous");
    assert_eq!(fs::read(&staged.path).unwrap(), b"replacement");
    cleanup(&dir);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn journal_keeps_non_utf8_destination_and_staging_names_losslessly() {
    use std::os::unix::ffi::OsStringExt;
    let (dir, _) = setup("non-utf8", None);
    let destination = dir.join(OsString::from_vec(b"package-\xff.exe".to_vec()));
    fs::write(&destination, b"previous").unwrap();
    let staged = staged_file(&destination, b"replacement");
    let owner = begin_test_replacement(&destination, &staged);
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&owner.record.path).unwrap()).unwrap();
    assert!(value.is_object());
    assert_eq!(
        owner.bindings.layout.record().target.os(),
        destination.file_name().unwrap()
    );
    assert_eq!(
        owner.bindings.layout.record().stage.os(),
        staged.path.file_name().unwrap()
    );
    let installed = resume(owner, &destination).unwrap_or_else(|f| panic!("{}", f.error));
    let label = public_output_label(&destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    let outcome =
        replacement::complete(installed, &context).unwrap_or_else(|f| panic!("{}", f.error));
    let Outcome::Sfx { backup, .. } = outcome else {
        panic!("SFX completion expected")
    };
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert_eq!(fs::read(backup).unwrap(), b"previous");
    cleanup(&dir);
}

#[test]
fn new_sfx_completion_rejects_a_missing_previous_backup() {
    let (dir, destination) = setup("missing-previous-backup", Some(b"previous"));
    let stage = staged_file(&destination, b"replacement");
    let installed = resume(begin_test_replacement(&destination, &stage), &destination)
        .unwrap_or_else(|f| panic!("{}", f.error));
    let previous = installed
        .owner
        .bindings
        .layout
        .previous(&installed.owner.bindings.scope);
    fs::remove_file(&previous).unwrap();
    let label = public_output_label(&destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    let failure = replacement::complete(installed, &context).err().unwrap();
    assert!(failure
        .error
        .to_string()
        .contains("requires its previous-output backup"));
    assert!(journal_path(&destination).exists());
    assert!(!completion_path(&destination).exists());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert!(!previous.exists());
    cleanup(&dir);
}

#[test]
fn exact_old_stage_path_requires_a_current_held_writer() {
    for mode in ["held", "bare-path", "rebound"] {
        let (dir, destination) = setup(&format!("active-old-stage-{mode}"), Some(b"previous"));
        let old_stage = staged_file(&destination, b"interrupted replacement");
        let hook = interrupt_after_move(&destination, 1);
        assert!(publish(
            &old_stage,
            &destination,
            CreateCommitPolicy::ReplaceExisting
        )
        .is_err());
        drop(hook);
        assert!(!old_stage.path.exists());
        let mut held = replacement::open_new_artifact(&old_stage.path).unwrap();
        held.write_all(b"current writer").unwrap();
        held.sync_all().unwrap();
        let identity = file_identity(&held).unwrap();
        #[cfg(windows)]
        {
            let label = public_output_label(&destination);
            let control = ControlToken::default();
            let context = VerificationContext {
                public_label: &label,
                progress: &NoProgress,
                control: &control,
            };
            seal_staging_file(&old_stage.path, &mut held, identity, &context).unwrap();
        }
        let claimed_path = old_stage.path.clone();
        let displaced = dir.join("displaced-current-writer");
        let input = match mode {
            "bare-path" => {
                let other = staged_file(&destination, b"unrelated handle");
                OwnedInput::SfxFile {
                    path: claimed_path.clone(),
                    identity: file_identity(&other.held).unwrap(),
                    file: other.held,
                }
            }
            "rebound" => {
                fs::rename(&claimed_path, &displaced).unwrap();
                fs::write(&claimed_path, b"unrelated competitor").unwrap();
                OwnedInput::SfxFile {
                    path: claimed_path.clone(),
                    identity,
                    file: held,
                }
            }
            _ => OwnedInput::SfxFile {
                path: claimed_path.clone(),
                identity,
                file: held,
            },
        };
        let scope = Scope::sfx_directory(&destination)
            .unwrap()
            .lock_target(&destination)
            .unwrap_or_else(|f| panic!("{}", f.error));
        let label = public_output_label(&destination);
        let control = ControlToken::default();
        let context = VerificationContext {
            public_label: &label,
            progress: &NoProgress,
            control: &control,
        };
        let result = replacement::recover(scope, ActiveWriter::With(&input), &context);
        if mode == "held" {
            let Recovery::Backup { scope, backup } =
                result.unwrap_or_else(|f| panic!("{}", f.error))
            else {
                panic!("retained previous backup expected")
            };
            assert_eq!(fs::read(backup).unwrap(), b"previous");
            assert_eq!(fs::read(&destination).unwrap(), b"interrupted replacement");
            assert_eq!(fs::read(&claimed_path).unwrap(), b"current writer");
            assert_eq!(path_identity(&claimed_path).unwrap(), identity);
            drop(scope);
        } else {
            let failure = result.err().unwrap();
            assert!(failure
                .artifacts
                .iter()
                .any(|artifact| artifact.path == claimed_path));
            assert_eq!(fs::read(&destination).unwrap(), b"previous");
            assert!(journal_path(&destination).exists());
            if mode == "rebound" {
                assert_eq!(fs::read(&claimed_path).unwrap(), b"unrelated competitor");
                assert_eq!(fs::read(&displaced).unwrap(), b"current writer");
            } else {
                assert_eq!(fs::read(&claimed_path).unwrap(), b"current writer");
            }
        }
        cleanup(&dir);
    }
}

#[test]
fn tree_root_rebinding_and_symlink_aliases_do_not_receive_regular_alias_authority() {
    let (dir, _) = setup("tree-root-rebind", None);
    let destination = dir.join("Package.app");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("member"), b"previous").unwrap();
    let stage = staged_app(&destination, Path::new("member"), b"replacement");
    let owner = begin_test_replacement(&destination, &stage);
    let displaced = dir.join("displaced-root");
    fs::rename(&stage.path, &displaced).unwrap();
    fs::create_dir(&stage.path).unwrap();
    fs::write(stage.path.join("member"), b"unrelated root").unwrap();
    let failure = resume(owner, &destination).err().unwrap();
    assert!(matches!(failure.error, FormatError::Io(_)));
    assert!(failure
        .artifacts
        .iter()
        .any(|artifact| artifact.path == stage.path));
    assert_eq!(fs::read(destination.join("member")).unwrap(), b"previous");
    assert_eq!(
        fs::read(stage.path.join("member")).unwrap(),
        b"unrelated root"
    );
    assert_eq!(fs::read(displaced.join("member")).unwrap(), b"replacement");
    assert!(journal_path(&destination).exists());
    cleanup(&dir);

    #[cfg(unix)]
    {
        let (dir, _) = setup("tree-symlink-alias", None);
        let destination = dir.join("Package.app");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("member"), b"previous").unwrap();
        let stage = staged_app(&destination, Path::new("member"), b"replacement");
        let owner = begin_test_replacement(&destination, &stage);
        let protected = owner.bindings.layout.protected(&owner.bindings.scope);
        let hook = interrupt_after_move(&destination, 1);
        assert!(resume(owner, &destination).is_err());
        drop(hook);
        std::os::unix::fs::symlink(&protected, &stage.path).unwrap();
        let input = OwnedInput::SfxTree {
            path: stage.path.clone(),
            root: stage.held.try_clone().unwrap(),
            identity: file_identity(&stage.held).unwrap(),
        };
        let scope = Scope::sfx_directory(&destination)
            .unwrap()
            .lock_target(&destination)
            .unwrap_or_else(|f| panic!("{}", f.error));
        let label = public_output_label(&destination);
        let control = ControlToken::default();
        let context = VerificationContext {
            public_label: &label,
            progress: &NoProgress,
            control: &control,
        };
        assert!(replacement::recover(scope, ActiveWriter::With(&input), &context).is_err());
        assert!(fs::symlink_metadata(&stage.path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(protected.join("member")).unwrap(), b"replacement");
        assert_eq!(fs::read(destination.join("member")).unwrap(), b"previous");
        assert!(journal_path(&destination).exists());
        cleanup(&dir);
    }
}

struct TestZipFormat;

impl ArchiveFormat for TestZipFormat {
    fn id(&self) -> &'static str {
        "zip"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zip"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_test: true,
            ..FormatCapabilities::default()
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        head.starts_with(b"TESTZIP\0")
    }

    fn open(
        &self,
        _source: Box<dyn ReadSeek>,
        _options: &ArchiveOpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Ok(Box::new(TestZipReader))
    }

    fn create(
        &self,
        mut destination: Box<dyn WriteSeek>,
        _options: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        destination.write_all(b"TESTZIP\0")?;
        Ok(Box::new(TestZipWriter { destination }))
    }
}

struct TestZipWriter {
    destination: Box<dyn WriteSeek>,
}

impl ArchiveWriter for TestZipWriter {
    fn add_entry(
        &mut self,
        metadata: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        let path = &metadata.path.raw;
        self.destination
            .write_all(&(path.len() as u64).to_le_bytes())?;
        self.destination.write_all(path)?;
        if let Some(data) = data {
            io::copy(data, &mut self.destination)?;
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
        self.destination.flush()?;
        Ok(())
    }
}

struct TestZipReader;

impl ArchiveReader for TestZipReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(std::iter::empty())
    }

    fn read_entry(
        &mut self,
        _path: &EntryPath,
        _consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(
            "test ZIP reader has no materialized entries".into(),
        ))
    }

    fn test_summary(
        &mut self,
        _limits: &squallz_format_api::SafetyLimits,
        _progress: &dyn squallz_format_api::ProgressSink,
        _control: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        Ok(TestSummary::default())
    }
}

fn case_aliases_share_an_entry(directory: &Path) -> bool {
    let mixed = directory.join("SquallzCaseProbe");
    let lower = directory.join("squallzcaseprobe");
    fs::write(&mixed, b"probe").unwrap();
    let shared = crate::same_path_entry(&mixed, &lower);
    fs::remove_file(mixed).unwrap();
    shared
}

fn test_sfx_engine() -> crate::Engine {
    let mut registry = FormatRegistry::new();
    registry.register_archive(Arc::new(TestZipFormat));
    crate::Engine::new(registry)
}

fn write_test_pe_stub(directory: &Path) -> PathBuf {
    let path = directory.join("sfx-stub.exe");
    let mut bytes = vec![0u8; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    let marker = crate::SFX_CLI_STUB_MARKER;
    bytes[0x100..0x100 + marker.len()].copy_from_slice(&marker);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn guarded_publish_rejects_a_destination_removed_after_guard_verification() {
    let dir = test_dir("guarded-destination-removed-after-verification");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("package.exe");
    fs::write(&destination, b"old-data").unwrap();
    let guard = crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
        .unwrap()
        .guard
        .unwrap();
    let guarded_digest = crate::destination_guard::verify_destination_guard(
        &destination,
        CreateArtifactKind::SfxSingleFile,
        guard,
    )
    .unwrap();

    fs::remove_file(&destination).unwrap();
    let error = validate_guarded_publish_destination(
        &destination,
        &destination,
        SfxLayout::SingleFile,
        true,
        Some(guarded_digest),
    )
    .unwrap_err();

    assert!(error.is_destination_changed());
    assert!(!destination.exists());
    cleanup(&dir);
}

#[test]
fn guarded_single_file_publish_maps_a_late_directory_to_destination_changed() {
    let dir = test_dir("guarded-single-file-late-directory");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("package.exe");
    fs::write(&destination, b"old-data").unwrap();
    let guard = crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
        .unwrap()
        .guard
        .unwrap();
    let guarded_digest = crate::destination_guard::verify_destination_guard(
        &destination,
        CreateArtifactKind::SfxSingleFile,
        guard,
    )
    .unwrap();

    fs::remove_file(&destination).unwrap();
    fs::create_dir(&destination).unwrap();
    let error = validate_guarded_publish_destination(
        &destination,
        &destination,
        SfxLayout::SingleFile,
        true,
        Some(guarded_digest),
    )
    .unwrap_err();

    assert!(error.is_destination_changed());
    assert!(destination.is_dir());
    cleanup(&dir);
}

#[test]
fn guarded_app_publish_maps_a_late_file_to_destination_changed() {
    let dir = test_dir("guarded-app-late-file");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("Package.app");
    fs::create_dir(&destination).unwrap();
    let guard = crate::inspect_create_destination(&destination, CreateArtifactKind::SfxMacosApp)
        .unwrap()
        .guard
        .unwrap();
    let guarded_digest = crate::destination_guard::verify_destination_guard(
        &destination,
        CreateArtifactKind::SfxMacosApp,
        guard,
    )
    .unwrap();

    fs::remove_dir(&destination).unwrap();
    fs::write(&destination, b"late-file").unwrap();
    let error = validate_guarded_publish_destination(
        &destination,
        &destination,
        SfxLayout::MacosApp,
        true,
        Some(guarded_digest),
    )
    .unwrap_err();

    assert!(error.is_destination_changed());
    assert_eq!(fs::read(&destination).unwrap(), b"late-file");
    cleanup(&dir);
}

#[test]
fn cleanup_preserves_a_quarantined_app_when_a_deep_member_arrives() {
    let dir = test_dir("cleanup-deep-member");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("Package.app");
    let (staged, staged_identity) =
        reserve_disposal_stage(&destination, SfxLayout::MacosApp).unwrap();
    let original = staged.join("Contents/Resources/original.dat");
    fs::create_dir_all(original.parent().unwrap()).unwrap();
    fs::write(&original, b"owned staging data").unwrap();
    let state_digest = path_state_digest(&staged).unwrap().unwrap();
    let parent = fs::canonicalize(&dir).unwrap();
    let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
    let record = CleanupRecord {
        version: CLEANUP_VERSION,
        kind: TemporaryKind::Stage,
        layout: JournalLayout::MacosApp,
        requested_destination: StoredOsString::from_os_str(destination.file_name().unwrap())
            .unwrap(),
        staged: StoredOsString::from_os_str(staged.file_name().unwrap()).unwrap(),
        quarantine: StoredOsString::from_os_str(quarantine.file_name().unwrap()).unwrap(),
        identity: staged_identity,
        state_digest,
    };
    write_cleanup_record(&destination, &record, &mut sync_directory).unwrap();
    let mut injected = false;

    let error = reconcile_cleanup_record(&destination, &mut |path| {
        if !injected && quarantine.exists() {
            let late = quarantine.join("Contents/Resources/late/deep/member.dat");
            fs::create_dir_all(late.parent().unwrap())?;
            fs::write(late, b"late unowned data")?;
            injected = true;
        }
        sync_directory(path)
    })
    .unwrap_err();

    assert!(injected);
    assert!(!staged.exists());
    assert!(quarantine.exists());
    assert_eq!(
        fs::read(quarantine.join("Contents/Resources/late/deep/member.dat")).unwrap(),
        b"late unowned data"
    );
    assert!(dir.join(CLEANUP_JOURNAL_NAME).exists());
    assert!(error.to_string().contains("cleanup tree changed"));
    assert!(!sfx_recovery_requires_staging(&error));
    cleanup(&dir);
}

#[test]
fn cleanup_preserves_a_file_rebound_after_digest_validation() {
    let dir = test_dir("cleanup-file-post-digest-rebind");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("package.exe");
    let (staged, staged_identity) =
        reserve_disposal_stage(&destination, SfxLayout::SingleFile).unwrap();
    fs::write(&staged, b"owned staging data").unwrap();
    let state_digest = path_state_digest(&staged).unwrap().unwrap();
    let parent = fs::canonicalize(&dir).unwrap();
    let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
    let record = CleanupRecord {
        version: CLEANUP_VERSION,
        kind: TemporaryKind::Stage,
        layout: JournalLayout::SingleFile,
        requested_destination: StoredOsString::from_os_str(destination.file_name().unwrap())
            .unwrap(),
        staged: StoredOsString::from_os_str(staged.file_name().unwrap()).unwrap(),
        quarantine: StoredOsString::from_os_str(quarantine.file_name().unwrap()).unwrap(),
        identity: staged_identity,
        state_digest,
    };
    write_cleanup_record(&destination, &record, &mut sync_directory).unwrap();
    crate::move_path_no_replace(&staged, &quarantine).unwrap();
    let competitor = dir.join("competitor.txt");
    let displaced = dir.join("owned-displaced.txt");
    fs::write(&competitor, b"unrelated competitor").unwrap();
    let mut injected = false;

    let error =
        reconcile_cleanup_with_disposal_move(&destination, &mut sync_directory, &mut |from, to| {
            if !injected {
                fs::rename(from, &displaced)?;
                fs::rename(&competitor, from)?;
                injected = true;
            }
            crate::move_path_no_replace(from, to)
        })
        .unwrap_err();

    assert!(injected);
    assert_eq!(fs::read(&quarantine).unwrap(), b"unrelated competitor");
    assert_eq!(fs::read(&displaced).unwrap(), b"owned staging data");
    assert!(dir.join(CLEANUP_JOURNAL_NAME).exists());
    assert!(error
        .to_string()
        .contains("without deleting an unverified path"));
    cleanup(&dir);
}

#[test]
fn cleanup_preserves_a_deep_directory_rebound_after_digest_validation() {
    let dir = test_dir("cleanup-deep-post-digest-rebind");
    fs::create_dir(&dir).unwrap();
    let destination = dir.join("Package.app");
    let (staged, staged_identity) =
        reserve_disposal_stage(&destination, SfxLayout::MacosApp).unwrap();
    let nested = staged.join("Contents/Resources/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("owned.dat"), b"owned nested data").unwrap();
    let state_digest = path_state_digest(&staged).unwrap().unwrap();
    let parent = fs::canonicalize(&dir).unwrap();
    let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
    let record = CleanupRecord {
        version: CLEANUP_VERSION,
        kind: TemporaryKind::Stage,
        layout: JournalLayout::MacosApp,
        requested_destination: StoredOsString::from_os_str(destination.file_name().unwrap())
            .unwrap(),
        staged: StoredOsString::from_os_str(staged.file_name().unwrap()).unwrap(),
        quarantine: StoredOsString::from_os_str(quarantine.file_name().unwrap()).unwrap(),
        identity: staged_identity,
        state_digest,
    };
    write_cleanup_record(&destination, &record, &mut sync_directory).unwrap();
    crate::move_path_no_replace(&staged, &quarantine).unwrap();
    let competitor = dir.join("competitor-nested");
    let competitor_member = competitor.join("deep/member.dat");
    fs::create_dir_all(competitor_member.parent().unwrap()).unwrap();
    fs::write(&competitor_member, b"unrelated nested data").unwrap();
    let displaced = dir.join("owned-nested-displaced");
    let mut injected = false;

    let error =
        reconcile_cleanup_with_disposal_move(&destination, &mut sync_directory, &mut |from, to| {
            if !injected {
                let nested = from.join("Contents/Resources/nested");
                fs::rename(&nested, &displaced)?;
                fs::rename(&competitor, &nested)?;
                injected = true;
            }
            crate::move_path_no_replace(from, to)
        })
        .unwrap_err();

    assert!(injected);
    assert_eq!(
        fs::read(quarantine.join("Contents/Resources/nested/deep/member.dat")).unwrap(),
        b"unrelated nested data"
    );
    assert_eq!(
        fs::read(displaced.join("owned.dat")).unwrap(),
        b"owned nested data"
    );
    assert!(dir.join(CLEANUP_JOURNAL_NAME).exists());
    assert!(error
        .to_string()
        .contains("without deleting an unverified path"));
    cleanup(&dir);
}

#[test]
fn quarantine_identity_mismatch_preserves_the_rebound_entry() {
    let dir = test_dir("quarantine-rebind");
    fs::create_dir(&dir).unwrap();
    let source = dir.join("fixed-record.json");
    let attacker = dir.join("attacker.txt");
    let displaced = dir.join("displaced-record.json");
    fs::write(&source, b"owned record").unwrap();
    fs::write(&attacker, b"unrelated entry").unwrap();
    let identity = path_identity(&source).unwrap();
    let mut rebound = None;

    let error = remove_bound_path_via_quarantine(
        &source,
        identity,
        SfxLayout::SingleFile,
        &source,
        "test record",
        &mut |_parent| {
            if rebound.is_none() {
                let quarantine = fs::read_dir(&dir)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .and_then(OsStr::to_str)
                            .is_some_and(cleanup_quarantine_name_is_reserved)
                    })
                    .ok_or_else(|| io::Error::other("cleanup quarantine was not published"))?;
                fs::rename(&quarantine, &displaced)?;
                fs::rename(&attacker, &quarantine)?;
                rebound = Some(quarantine);
            }
            Ok(())
        },
    )
    .unwrap_err();

    let rebound = rebound.unwrap();
    let details = sfx_recovery_details(&error).unwrap();
    assert!(details
        .paths
        .iter()
        .any(|path| crate::same_path_entry(path, &rebound)));
    assert_eq!(fs::read(&rebound).unwrap(), b"unrelated entry");
    assert_eq!(fs::read(&displaced).unwrap(), b"owned record");
    fs::remove_dir_all(dir).unwrap();
}
