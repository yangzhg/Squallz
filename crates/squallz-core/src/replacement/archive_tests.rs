use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::api::{ControlToken, EntryPath, FormatError, NoProgress, ProgressPhase, ProgressSink};
use crate::destination_guard::verify_destination_guard;
use crate::filesystem_identity::{
    file_identity, open_new_artifact, open_regular_file_no_follow, RegularFileState,
};
use crate::CreateArtifactKind;

use super::evidence::{ProofSlot, VerificationContext};
use super::model::{Authorization, Outcome, PersistFailure, Replacement};
use super::scope::{RecordPhase, Scope};
use super::{owner, prepare, record};

struct ArchiveFixture {
    _directory: tempfile::TempDir,
    target: PathBuf,
    stage: PathBuf,
    holder: PathBuf,
    protected: PathBuf,
    previous: PathBuf,
    retired: PathBuf,
    pending: PathBuf,
    journal: PathBuf,
    completed: PathBuf,
    owner: Option<Replacement<'static>>,
}

fn context<'a>(
    label: &'a EntryPath,
    progress: &'a dyn ProgressSink,
    control: &'a ControlToken,
) -> VerificationContext<'a> {
    VerificationContext {
        public_label: label,
        progress,
        control,
    }
}

fn archive_input(path: PathBuf, file: File) -> super::model::OwnedInput {
    super::model::OwnedInput::Archive {
        identity: file_identity(&file).unwrap(),
        state: RegularFileState::from_metadata(&file.metadata().unwrap()),
        path,
        file,
    }
}

impl ArchiveFixture {
    fn new(guarded: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let requested = directory.path().join("archive.zip");
        fs::write(&requested, b"old archive").unwrap();
        let control = ControlToken::default();
        let scope = Scope::archive(&requested, &control).unwrap();
        let target = scope.target_path();
        let old_file = open_regular_file_no_follow(&target).unwrap();
        let stage = scope.reserve_name("stage");
        let mut new_file = open_new_artifact(&stage).unwrap();
        new_file.write_all(b"new archive").unwrap();
        new_file.sync_all().unwrap();
        let authorization = if guarded {
            let guard = crate::inspect_create_destination(&target, CreateArtifactKind::Archive)
                .unwrap()
                .guard
                .unwrap();
            Authorization::Guarded {
                digest: verify_destination_guard(&target, CreateArtifactKind::Archive, guard)
                    .unwrap(),
            }
        } else {
            Authorization::ContentBound
        };
        let label = EntryPath::from_utf8("archive.zip");
        let prepared = prepare::prepare(
            scope,
            archive_input(target.clone(), old_file),
            archive_input(stage.clone(), new_file),
            authorization,
            &context(&label, &NoProgress, &control),
        )
        .map_err(|failure| failure.error)
        .unwrap();
        let layout = &prepared.bindings.layout;
        let scope = &prepared.bindings.scope;
        let paths = (
            layout.holder(scope),
            layout.protected(scope),
            layout.previous(scope),
            layout.retired(scope),
            scope.anchor(RecordPhase::Pending),
            scope.anchor(RecordPhase::Installing),
            scope.anchor(RecordPhase::Completed),
        );
        let owner = record::persist(prepared)
            .map_err(|failure| match failure {
                PersistFailure::Unpublished { error, .. } => error,
                PersistFailure::Retained(failure) => failure.error,
            })
            .unwrap();
        Self {
            _directory: directory,
            target,
            stage,
            holder: paths.0,
            protected: paths.1,
            previous: paths.2,
            retired: paths.3,
            pending: paths.4,
            journal: paths.5,
            completed: paths.6,
            owner: Some(owner),
        }
    }

    fn advance(&self, boundary: usize) {
        for (source, destination) in [
            (&self.stage, &self.protected),
            (&self.target, &self.previous),
            (&self.protected, &self.target),
        ]
        .into_iter()
        .take(boundary)
        {
            crate::move_path_no_replace(source, destination).unwrap();
        }
    }

    fn recover(&mut self, progress: &dyn ProgressSink) -> Result<(), FormatError> {
        drop(self.owner.take());
        let control = ControlToken::default();
        let scope = Scope::archive(&self.target, &control)?;
        crate::update::recover_scope_for_test(scope, progress, &control).map(drop)
    }

    fn complete(&mut self, progress: &dyn ProgressSink) -> Result<(), FormatError> {
        let label = EntryPath::from_utf8("archive.zip");
        let control = ControlToken::default();
        let installed = owner::commit_resume(
            self.owner.take().unwrap(),
            &context(&label, &NoProgress, &control),
        )
        .map_err(|failure| failure.error)?;
        owner::complete(installed, &context(&label, progress, &control))
            .map_err(|failure| failure.error)
            .map(|outcome| {
                assert!(matches!(outcome, Outcome::Archive { .. }));
            })
    }

    fn completed_owner(&mut self, retain_proofs: bool) -> Replacement<'static> {
        let label = EntryPath::from_utf8("archive.zip");
        let control = ControlToken::default();
        let mut owner = owner::commit_resume(
            self.owner.take().unwrap(),
            &context(&label, &NoProgress, &control),
        )
        .map_err(|failure| failure.error)
        .unwrap()
        .owner;
        record::move_record(&mut owner.record, &self.completed, &owner.bindings.scope)
            .map_err(|failure| failure.error)
            .unwrap();
        if !retain_proofs {
            owner.bindings.proofs.previous = ProofSlot::Unverified;
            owner.bindings.proofs.replacement = ProofSlot::Unverified;
        }
        owner
    }

    fn assert_clean(&self) {
        for path in [
            &self.stage,
            &self.holder,
            &self.pending,
            &self.journal,
            &self.completed,
        ] {
            assert!(!path.exists(), "owned path remains: {}", path.display());
        }
        let mut isolated = self.holder.as_os_str().to_os_string();
        isolated.push(".empty-isolation");
        assert!(!PathBuf::from(isolated).exists());
    }
}

fn overwrite_same_length_preserving_mtime(path: &Path, bytes: &[u8]) {
    let before = RegularFileState::from_metadata(&fs::metadata(path).unwrap());
    assert_eq!(before.bytes(), bytes.len() as u64);
    let modified = fs::metadata(path).unwrap().modified().unwrap();
    for _ in 0..100 {
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = RegularFileState::from_metadata(&fs::metadata(path).unwrap());
        if before.equivalent_after_rename(&after) {
            #[cfg(unix)]
            if before != after {
                return;
            }
            #[cfg(not(unix))]
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("test filesystem did not preserve the requested modified time");
}

#[test]
fn recovery_finishes_each_committed_move_boundary() {
    for boundary in 0..4 {
        let mut fixture = ArchiveFixture::new(false);
        fixture.advance(boundary);
        fixture.recover(&NoProgress).unwrap();
        assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
        fixture.assert_clean();
    }
}

#[test]
fn guarded_recovery_rechecks_the_moved_destination_before_install() {
    let mut fixture = ArchiveFixture::new(true);
    fixture.advance(2);
    overwrite_same_length_preserving_mtime(&fixture.previous, b"bad archive");

    let error = fixture.recover(&NoProgress).unwrap_err();

    assert!(!error.is_destination_changed());
    assert!(!fixture.target.exists());
    assert_eq!(fs::read(&fixture.previous).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.protected).unwrap(), b"new archive");
    assert!(fixture.journal.exists());
}

#[test]
fn guarded_cleanup_preserves_changed_previous_and_retired_archives() {
    for retire_before_change in [false, true] {
        let mut fixture = ArchiveFixture::new(true);
        let owner = fixture.completed_owner(true);
        let changed = if retire_before_change {
            crate::move_path_no_replace(&fixture.previous, &fixture.retired).unwrap();
            &fixture.retired
        } else {
            &fixture.previous
        };
        overwrite_same_length_preserving_mtime(changed, b"bad archive");
        let label = EntryPath::from_utf8("archive.zip");
        let control = ControlToken::default();

        let error =
            owner::finish_completed_for_test(owner, &context(&label, &NoProgress, &control))
                .map_err(crate::update::archive_failure_for_test)
                .err()
                .expect("changed authorized archive was removed");

        assert!(!error.is_destination_changed());
        assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
        assert_eq!(fs::read(changed).unwrap(), b"bad archive");
        assert!(fixture.completed.exists());
        assert!(fixture.holder.exists());
    }
}

#[derive(Default)]
struct PhaseRecorder(Mutex<Vec<(ProgressPhase, bool)>>);
impl ProgressSink for PhaseRecorder {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}
    fn on_phase(&self, phase: ProgressPhase, cancellable: bool) {
        self.0.lock().unwrap().push((phase, cancellable));
    }
}

#[test]
fn recovery_reports_one_uninterruptible_phase() {
    let mut fixture = ArchiveFixture::new(false);
    let progress = PhaseRecorder::default();

    fixture.recover(&progress).unwrap();

    assert_eq!(
        *progress.0.lock().unwrap(),
        vec![(ProgressPhase::UpdateRecovery, false)]
    );
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

#[test]
fn recovery_never_replaces_a_late_competitor() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(2);
    fs::write(&fixture.target, b"competitor").unwrap();

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"competitor");
    assert_eq!(fs::read(&fixture.previous).unwrap(), b"old archive");
    assert_eq!(fs::read(&fixture.protected).unwrap(), b"new archive");
    assert!(fixture.journal.exists());

    fs::remove_file(&fixture.target).unwrap();
    fixture.recover(&NoProgress).unwrap();
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

#[test]
fn recovery_preserves_a_competing_directory_and_its_contents() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(2);
    fs::create_dir(&fixture.target).unwrap();
    fs::write(fixture.target.join("marker"), b"keep").unwrap();

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(fixture.target.join("marker")).unwrap(), b"keep");
    assert_eq!(fs::read(&fixture.previous).unwrap(), b"old archive");
    assert_eq!(fs::read(&fixture.protected).unwrap(), b"new archive");
    assert!(fixture.journal.exists());

    fs::remove_dir_all(&fixture.target).unwrap();
    fixture.recover(&NoProgress).unwrap();
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

#[test]
fn durable_pending_record_resumes_and_cleans_the_transaction() {
    let mut fixture = ArchiveFixture::new(false);
    crate::move_path_no_replace(&fixture.journal, &fixture.pending).unwrap();
    crate::sync_directory(crate::parent_or_current(&fixture.target)).unwrap();

    fixture.recover(&NoProgress).unwrap();

    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

#[test]
fn malformed_pending_record_preserves_every_transaction_path() {
    let mut fixture = ArchiveFixture::new(false);
    fs::remove_file(&fixture.journal).unwrap();
    fs::write(&fixture.pending, b"{incomplete").unwrap();
    crate::sync_directory(crate::parent_or_current(&fixture.target)).unwrap();

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"new archive");
    assert_eq!(fs::read(&fixture.pending).unwrap(), b"{incomplete");
    assert!(fixture.holder.is_dir());
}

#[test]
fn recovery_rejects_same_length_rewrite_after_staging_move() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(1);
    overwrite_same_length_preserving_mtime(&fixture.protected, b"bad archive");

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
    assert_eq!(fs::read(&fixture.protected).unwrap(), b"bad archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.journal.exists());
}

#[test]
fn recovery_rejects_same_length_rewrite_after_install() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(3);
    overwrite_same_length_preserving_mtime(&fixture.target, b"bad archive");

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.previous).unwrap(), b"old archive");
    assert!(fixture.journal.exists());
}

#[test]
fn completed_recovery_preserves_old_archive_when_installed_content_changed() {
    let mut fixture = ArchiveFixture::new(false);
    drop(fixture.completed_owner(false));
    overwrite_same_length_preserving_mtime(&fixture.target, b"bad archive");

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.previous).unwrap(), b"old archive");
    assert!(!fixture.retired.exists());
    assert!(fixture.completed.exists());
    assert!(fixture.holder.exists());
}

#[test]
fn recovery_rejects_same_length_staging_rewrite_before_first_move() {
    let mut fixture = ArchiveFixture::new(false);
    overwrite_same_length_preserving_mtime(&fixture.stage, b"bad archive");

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
    assert!(!fixture.protected.exists());
    assert!(fixture.journal.exists());
}

#[test]
fn recovery_rejects_same_length_target_rewrite_before_second_move() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(1);
    overwrite_same_length_preserving_mtime(&fixture.target, b"bad archive");

    assert!(fixture.recover(&NoProgress).is_err());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.protected).unwrap(), b"new archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.journal.exists());
}

#[test]
fn waiting_for_an_update_lock_can_be_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("archive.zip");
    fs::write(&target, b"archive").unwrap();
    let first = Scope::archive(&target, &ControlToken::default()).unwrap();
    let control = ControlToken::new();
    let waiter_control = control.clone();
    let waiter_target = target.clone();
    let waiter =
        std::thread::spawn(move || Scope::archive(&waiter_target, &waiter_control).map(drop));
    std::thread::sleep(Duration::from_millis(100));
    control.cancel();

    assert!(matches!(
        waiter.join().unwrap(),
        Err(FormatError::Cancelled)
    ));
    assert_eq!(fs::read(&target).unwrap(), b"archive");
    drop(first);
}

#[test]
fn verified_cleanup_removes_a_readonly_retired_archive() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(3);
    crate::move_path_no_replace(&fixture.previous, &fixture.retired).unwrap();
    let mut permissions = fs::metadata(&fixture.retired).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&fixture.retired, permissions).unwrap();
    let label = EntryPath::from_utf8("archive.zip");
    let control = ControlToken::default();

    super::moves::remove_retired(
        fixture.owner.as_mut().unwrap(),
        &context(&label, &NoProgress, &control),
    )
    .unwrap();

    assert!(!fixture.retired.exists());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
}

fn orphan_fixture(holder: bool, marker: bool) -> ArchiveFixture {
    let mut fixture = ArchiveFixture::new(false);
    drop(fixture.owner.take());
    fs::remove_file(&fixture.journal).unwrap();
    if holder {
        fs::remove_file(&fixture.stage).unwrap();
        if marker {
            fs::write(fixture.holder.join("marker"), b"keep").unwrap();
        }
    } else {
        fs::remove_dir(&fixture.holder).unwrap();
        fs::write(&fixture.stage, b"partial update").unwrap();
    }
    fixture
}

#[test]
fn unjournaled_empty_holder_is_preserved_for_inspection() {
    let fixture = orphan_fixture(true, false);
    let scope = Scope::archive(&fixture.target, &ControlToken::default()).unwrap();

    assert!(scope.require_clear().is_err());
    assert!(fixture.holder.is_dir());
    assert_eq!(fs::read_dir(&fixture.holder).unwrap().count(), 0);
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
}

#[test]
fn nonempty_orphan_holder_is_preserved_for_inspection() {
    let fixture = orphan_fixture(true, true);
    let scope = Scope::archive(&fixture.target, &ControlToken::default()).unwrap();

    assert!(scope.require_clear().is_err());
    assert_eq!(fs::read(fixture.holder.join("marker")).unwrap(), b"keep");
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
}

#[test]
fn unjournaled_staging_path_is_preserved_for_inspection() {
    let fixture = orphan_fixture(false, false);
    let scope = Scope::archive(&fixture.target, &ControlToken::default()).unwrap();

    assert!(scope.require_clear().is_err());
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"partial update");
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
}

#[cfg(unix)]
#[test]
fn symbolic_link_targets_are_rejected_without_following_them() {
    let directory = tempfile::tempdir().unwrap();
    let victim = directory.path().join("victim.zip");
    let link = directory.path().join("archive.zip");
    fs::write(&victim, b"archive").unwrap();
    std::os::unix::fs::symlink(&victim, &link).unwrap();

    assert!(matches!(
        Scope::archive(&link, &ControlToken::default()),
        Err(FormatError::Unsupported(_))
    ));
    assert_eq!(fs::read(&victim).unwrap(), b"archive");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn symbolic_link_lock_paths_are_not_followed() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("archive.zip");
    let victim = directory.path().join("victim");
    fs::write(&target, b"archive").unwrap();
    fs::write(&victim, b"keep").unwrap();
    let canonical = fs::canonicalize(&target).unwrap();
    let key = super::scope::path_key(b"squallz-update-target-v1\0", &canonical).unwrap();
    let lock = std::env::temp_dir().join(format!("squallz-update-target-{key}.lock"));
    std::os::unix::fs::symlink(&victim, &lock).unwrap();

    let result = Scope::archive(&target, &ControlToken::default());

    assert!(matches!(result, Err(FormatError::Io(_))));
    assert_eq!(fs::read(&victim).unwrap(), b"keep");
    assert_eq!(fs::read(&target).unwrap(), b"archive");
    assert!(fs::symlink_metadata(&lock)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&lock).unwrap();
}

#[test]
fn installed_archive_recovery_accepts_an_already_removed_previous_output() {
    let mut fixture = ArchiveFixture::new(false);
    fixture.advance(3);
    fs::remove_file(&fixture.previous).unwrap();

    fixture.recover(&NoProgress).unwrap();

    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

fn entry_move_race(previous: bool, rewrite_content: bool, occupy_restore: bool) {
    use std::cell::Cell;
    use std::rc::Rc;

    let mut fixture = ArchiveFixture::new(false);
    if previous {
        fixture.advance(1);
    }
    let source = if previous {
        fixture.target.clone()
    } else {
        fixture.stage.clone()
    };
    let destination = if previous {
        fixture.previous.clone()
    } else {
        fixture.protected.clone()
    };
    let held = fixture._directory.path().join("held-original");
    let competitor = fixture._directory.path().join("competitor");
    fs::write(&competitor, b"moved competitor").unwrap();
    let calls = Rc::new(Cell::new(0));
    let observed_calls = calls.clone();
    let moved_source = source.clone();
    let moved_destination = destination.clone();
    let held_path = held.clone();
    let hook = super::test_hooks::install_move(Box::new(move |from, to| {
        if from == moved_source && to == moved_destination {
            observed_calls.set(observed_calls.get() + 1);
            if rewrite_content {
                overwrite_same_length_preserving_mtime(from, b"bad archive");
            } else {
                crate::move_path_no_replace(from, &held_path)?;
                crate::move_path_no_replace(&competitor, from)?;
            }
        } else if from == moved_destination && to == moved_source {
            observed_calls.set(observed_calls.get() + 1);
            if occupy_restore {
                fs::write(to, b"late source entry")?;
            }
        }
        crate::move_path_no_replace(from, to)
    }));
    let label = EntryPath::from_utf8("archive.zip");
    let control = ControlToken::default();
    let owner = fixture.owner.as_mut().unwrap();

    let error = super::moves::checked_move(
        owner,
        if previous {
            super::moves::Entry::Previous
        } else {
            super::moves::Entry::Replacement
        },
        &source,
        &destination,
        super::evidence::StatePoint::BeforeMove,
        &context(&label, &NoProgress, &control),
    )
    .unwrap_err();

    assert!(!matches!(error, FormatError::Cancelled));
    assert_eq!(calls.get(), 2, "the observed moved entry must be restored");
    assert!(owner.visibility >= super::model::Visibility::OutputMayBeVisible);
    assert!(fixture.journal.exists());
    if occupy_restore {
        assert_eq!(fs::read(&source).unwrap(), b"late source entry");
        assert_eq!(fs::read(&destination).unwrap(), b"moved competitor");
    } else {
        assert!(!destination.exists());
        assert_eq!(
            fs::read(&source).unwrap(),
            if rewrite_content {
                b"bad archive".as_slice()
            } else {
                b"moved competitor".as_slice()
            }
        );
    }
    if !rewrite_content {
        assert_eq!(
            fs::read(&held).unwrap(),
            if previous {
                b"old archive".as_slice()
            } else {
                b"new archive".as_slice()
            }
        );
    }
    if previous {
        assert_eq!(fs::read(&fixture.protected).unwrap(), b"new archive");
    } else {
        assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
    }
    drop(hook);
}

#[test]
fn staging_rebind_during_move_is_restored_without_replacement() {
    entry_move_race(false, false, false);
}

#[test]
fn target_rebind_during_move_is_restored_without_replacement() {
    entry_move_race(true, false, false);
}

#[test]
fn failed_rebind_restoration_retains_both_competing_entries() {
    entry_move_race(true, false, true);
}

#[test]
fn staging_content_rewrite_during_move_is_restored() {
    entry_move_race(false, true, false);
}

#[test]
fn target_content_rewrite_during_move_is_restored() {
    entry_move_race(true, true, false);
}

#[test]
fn pending_publication_sync_failure_is_classified_as_published() {
    let mut fixture = ArchiveFixture::new(false);
    let parent = crate::parent_or_current(&fixture.target).to_path_buf();
    let hook = super::test_hooks::install_sync(Box::new(move |path| {
        if path == parent {
            Err(std::io::Error::other("simulated directory sync failure"))
        } else {
            crate::sync_directory(path)
        }
    }));
    let owner = fixture.owner.as_mut().unwrap();

    let failure = record::move_record(&mut owner.record, &fixture.pending, &owner.bindings.scope)
        .expect_err("a moved pending record was incorrectly acknowledged");

    assert!(failure.published);
    assert!(failure
        .error
        .to_string()
        .contains("simulated directory sync failure"));
    assert!(fixture.pending.is_file());
    assert!(!fixture.journal.exists());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"old archive");
    assert_eq!(fs::read(&fixture.stage).unwrap(), b"new archive");
    assert!(fixture.holder.is_dir());
    drop(hook);
}

#[derive(Default)]
struct DigestPassCounter(AtomicUsize);
impl ProgressSink for DigestPassCounter {
    fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
        if done == 0 && total > 0 {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn same_process_cleanup_reuses_installed_digest_verification() {
    let mut fixture = ArchiveFixture::new(false);
    let progress = DigestPassCounter::default();

    fixture.complete(&progress).unwrap();

    assert_eq!(progress.0.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    fixture.assert_clean();
}

#[cfg(unix)]
struct RewriteInstalledWhenRetiredAppears {
    target: PathBuf,
    retired: PathBuf,
    rewritten: AtomicBool,
}

#[cfg(unix)]
impl ProgressSink for RewriteInstalledWhenRetiredAppears {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        if self.retired.exists() && !self.rewritten.swap(true, Ordering::SeqCst) {
            overwrite_same_length_preserving_mtime(&self.target, b"bad archive");
        }
    }
}

#[cfg(unix)]
#[test]
fn changed_installed_file_invalidates_same_process_digest_verification() {
    let mut fixture = ArchiveFixture::new(false);
    let progress = RewriteInstalledWhenRetiredAppears {
        target: fixture.target.clone(),
        retired: fixture.retired.clone(),
        rewritten: AtomicBool::new(false),
    };

    assert!(fixture.complete(&progress).is_err());

    assert!(progress.rewritten.load(Ordering::SeqCst));
    assert_eq!(fs::read(&fixture.target).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.retired).unwrap(), b"old archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.completed.exists());
    assert!(fixture.holder.exists());
}

#[cfg(unix)]
struct RewriteRetiredDuringVerification {
    retired: PathBuf,
    rewritten: AtomicBool,
}

#[cfg(unix)]
impl ProgressSink for RewriteRetiredDuringVerification {
    fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
        if done == 0
            && total > 0
            && self.retired.exists()
            && !self.rewritten.swap(true, Ordering::SeqCst)
        {
            overwrite_same_length_preserving_mtime(&self.retired, b"bad archive");
        }
    }
}

#[cfg(unix)]
#[test]
fn cleanup_preserves_retired_file_changed_during_installed_verification() {
    let mut fixture = ArchiveFixture::new(false);
    let owner = fixture.completed_owner(false);
    crate::move_path_no_replace(&fixture.previous, &fixture.retired).unwrap();
    let progress = RewriteRetiredDuringVerification {
        retired: fixture.retired.clone(),
        rewritten: AtomicBool::new(false),
    };
    let label = EntryPath::from_utf8("archive.zip");
    let control = ControlToken::default();

    assert!(
        owner::finish_completed_for_test(owner, &context(&label, &progress, &control),).is_err()
    );

    assert!(progress.rewritten.load(Ordering::SeqCst));
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    assert_eq!(fs::read(&fixture.retired).unwrap(), b"bad archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.completed.exists());
    assert!(fixture.holder.exists());
}

#[cfg(unix)]
#[test]
fn completed_recovery_keeps_retired_archive_when_installed_content_changes_during_cleanup() {
    let mut fixture = ArchiveFixture::new(false);
    drop(fixture.completed_owner(false));
    let progress = RewriteInstalledWhenRetiredAppears {
        target: fixture.target.clone(),
        retired: fixture.retired.clone(),
        rewritten: AtomicBool::new(false),
    };

    assert!(fixture.recover(&progress).is_err());

    assert!(progress.rewritten.load(Ordering::SeqCst));
    assert_eq!(fs::read(&fixture.target).unwrap(), b"bad archive");
    assert_eq!(fs::read(&fixture.retired).unwrap(), b"old archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.completed.exists());
    assert!(fixture.holder.exists());
}

#[cfg(unix)]
#[test]
fn guarded_retirement_rechecks_mode_changes_after_the_move() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = ArchiveFixture::new(true);
    let previous = fixture.previous.clone();
    let retired = fixture.retired.clone();
    let changed = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = changed.clone();
    let hook = super::test_hooks::install_move(Box::new(move |from, to| {
        crate::move_path_no_replace(from, to)?;
        if from == previous && to == retired {
            let permissions = fs::metadata(to)?.permissions();
            fs::set_permissions(to, fs::Permissions::from_mode(permissions.mode() ^ 0o100))?;
            observed.set(true);
        }
        Ok(())
    }));

    assert!(fixture.complete(&NoProgress).is_err());

    assert!(changed.get());
    assert_eq!(fs::read(&fixture.target).unwrap(), b"new archive");
    assert_eq!(fs::read(&fixture.retired).unwrap(), b"old archive");
    assert!(!fixture.previous.exists());
    assert!(fixture.completed.exists());
    assert!(fixture.holder.exists());
    drop(hook);
}
