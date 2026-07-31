use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::{
    ArchiveFormat, ControlToken, CreateOptions, EntryPath, FormatError, PreparedUpdateAdditions,
    ProgressPhase, ProgressSink, UpdateOp,
};
use crate::destination_guard::{
    verify_destination_guard, verify_destination_guard_with_progress, verify_path_state_digest,
};
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, open_regular_file_no_follow_for_cleanup,
    path_identity, PathIdentity, RegularFileState,
};
use crate::{CreateArtifactKind, CreateDestinationGuard};

const TRANSACTION_VERSION_V1: u32 = 1;
const TRANSACTION_VERSION: u32 = 2;
const JOURNAL_MAX_BYTES: usize = 16 * 1024;
const DIGEST_BUFFER_BYTES: usize = 256 * 1024;
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
static ARTIFACT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct BoundSource {
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    permissions: Permissions,
}

struct ReservedStage {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
}

struct PreparedStage {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
}

struct TransactionDigests {
    source: [u8; 32],
    staging: [u8; 32],
    source_path_state: Option<[u8; 32]>,
}

/// Process-local evidence only. Callers must revalidate both the held handle
/// and the path's exact post-digest state, then fall back to a full digest when
/// either has changed.
#[derive(Debug)]
struct VerifiedBoundFile {
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionRecord {
    version: u32,
    key: String,
    target_name: String,
    staging_name: String,
    holder_name: String,
    source_identity: PathIdentity,
    source_state: RegularFileState,
    source_digest: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_path_state_digest: Option<[u8; 32]>,
    staging_identity: PathIdentity,
    staging_state: RegularFileState,
    staging_digest: [u8; 32],
    holder_identity: PathIdentity,
}

struct OpenRecord {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    bytes: Vec<u8>,
    record: TransactionRecord,
}

struct ResolvedTransaction {
    target: PathBuf,
    staging: PathBuf,
    holder: PathBuf,
    previous: PathBuf,
    replacement: PathBuf,
    retired: PathBuf,
    pending: PathBuf,
    journal: PathBuf,
    completion: PathBuf,
    source_identity: PathIdentity,
    source_state: RegularFileState,
    source_digest: [u8; 32],
    source_path_state_digest: Option<[u8; 32]>,
    staging_identity: PathIdentity,
    staging_state: RegularFileState,
    staging_digest: [u8; 32],
    holder_identity: PathIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedFile {
    identity: PathIdentity,
    state: RegularFileState,
}

#[derive(Clone, Copy)]
enum StateMatch {
    Exact,
    AfterRename,
}

struct TransactionSnapshot {
    staging: Option<ObservedFile>,
    target: Option<ObservedFile>,
    previous: Option<ObservedFile>,
    replacement: Option<ObservedFile>,
    retired: Option<ObservedFile>,
    holder_present: bool,
}

enum JournalPublishError {
    BeforePublish(FormatError),
    Published(FormatError),
}

enum OpenRecordMoveError {
    NotMoved(Box<OpenRecord>, FormatError),
    Published(FormatError),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    format: &dyn ArchiveFormat,
    requested_target: &Path,
    ops: &[UpdateOp],
    additions: &mut dyn PreparedUpdateAdditions,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    ctl.checkpoint()?;
    let requested = canonical_requested_target(requested_target)?;
    let parent = parent_or_current(&requested).to_path_buf();
    let directory_key = path_key(b"squallz-update-directory-v1\0", &parent)?;
    let _directory_lock = acquire_lock(
        &std::env::temp_dir().join(format!("squallz-update-directory-{directory_key}.lock")),
        "directory coordination",
        ctl,
    )?;

    let target = canonical_update_target(requested_target)?;
    let key = path_key(b"squallz-update-target-v1\0", &target)?;
    let _target_lock = acquire_lock(
        &std::env::temp_dir().join(format!("squallz-update-target-{key}.lock")),
        "target coordination",
        ctl,
    )?;

    let recovery_control = ControlToken::default();
    recover_existing(
        &target,
        &key,
        ProgressPhase::UpdateRecovery,
        progress,
        &recovery_control,
    )?;
    reject_unjournaled_artifacts(&target, &key)?;
    ctl.checkpoint()?;

    let source = bind_source(&target)?;
    let addition_bytes = addition_bytes(additions)?;
    let required =
        format.estimate_update_staging_bytes(source.state.bytes(), addition_bytes, opts)?;
    if fs4::available_space(parent_or_current(&target))? < required {
        return Err(FormatError::DiskFull);
    }

    let stage = reserve_stage(&target, &key)?;
    let source_stream = match source.file.try_clone() {
        Ok(file) => file,
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    let output_stream = match stage.file.try_clone() {
        Ok(file) => file,
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    progress.on_phase(ProgressPhase::UpdateRewrite, true);
    if let Err(error) = format.rewrite_update(
        Box::new(source_stream),
        Box::new(output_stream),
        ops,
        additions,
        opts,
        progress,
        ctl,
    ) {
        return Err(cleanup_stage(error, stage));
    }

    if let Err(error) = stage.file.set_permissions(source.permissions.clone()) {
        return Err(cleanup_stage(error.into(), stage));
    }
    if let Err(error) = stage.file.sync_all() {
        return Err(cleanup_stage(error.into(), stage));
    }
    if let Err(error) = validate_source(&target, &source) {
        return Err(cleanup_stage(error, stage));
    }
    let stage = match prepare_stage(stage) {
        Ok(stage) => stage,
        Err((error, stage)) => return Err(cleanup_stage(error, stage)),
    };
    if let Err(error) = ctl.checkpoint() {
        return Err(cleanup_prepared_stage(error, stage));
    }
    progress.on_phase(ProgressPhase::UpdateVerify, true);
    let digests = match bind_transaction_digests(&target, &source, &stage, progress, ctl) {
        Ok(digests) => digests,
        Err(error) => return Err(cleanup_prepared_stage(error, stage)),
    };

    let (holder, holder_identity) = match reserve_holder(&target, &key) {
        Ok(holder) => holder,
        Err(error) => return Err(cleanup_prepared_stage(error, stage)),
    };
    let transaction = transaction_for(
        &target,
        &key,
        &source,
        &stage,
        digests,
        holder,
        holder_identity,
    );
    let record = match record_for(&transaction, &key) {
        Ok(record) => record,
        Err(error) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
    };
    progress.on_phase(ProgressPhase::UpdateCommit, false);
    if let Err(error) = ctl.checkpoint() {
        let error = cleanup_empty_holder(error, &transaction);
        return Err(cleanup_prepared_stage(error, stage));
    }
    let active_stage = match retain_prepared_stage(&transaction, &stage) {
        Ok(active_stage) => active_stage,
        Err(error) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
    };
    let journal = match write_journal(&transaction, record) {
        Ok(journal) => journal,
        Err(JournalPublishError::BeforePublish(error)) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
        Err(JournalPublishError::Published(error)) => return Err(error),
    };
    // The phase callback makes future controls unavailable before the last
    // checkpoint above. Once the durable record exists, finish or leave a
    // recoverable record instead of reporting a partial commit as cancelled.
    let commit_control = ControlToken::default();
    let installed_verification =
        resume_transaction(&transaction, Some(active_stage), progress, &commit_control)?;
    drop(stage);
    let completion = publish_completion(journal, &transaction)?;
    drop(source);
    progress.on_phase(ProgressPhase::UpdateCleanup, false);
    cleanup_completed(
        completion,
        &transaction,
        Some(installed_verification),
        progress,
        &commit_control,
    )
}

pub(super) fn commit_created_archive(
    requested_target: &Path,
    staged: &Path,
    staged_file: File,
    staged_identity: PathIdentity,
    guard: CreateDestinationGuard,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let stage = bind_created_stage(staged, staged_file, staged_identity)?;
    commit_created_archive_bound(requested_target, guard, progress, ctl, stage)
}

fn commit_created_archive_bound(
    requested_target: &Path,
    guard: CreateDestinationGuard,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    stage: ReservedStage,
) -> Result<(), FormatError> {
    if let Err(error) = ctl.checkpoint() {
        return Err(cleanup_stage(error, stage));
    }
    let requested = match canonical_requested_target(requested_target) {
        Ok(requested) => requested,
        Err(error) => {
            let error = guarded_target_resolution_error(requested_target, error);
            return Err(cleanup_stage(error, stage));
        }
    };
    let parent = parent_or_current(&requested).to_path_buf();
    let directory_key = match path_key(b"squallz-update-directory-v1\0", &parent) {
        Ok(key) => key,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };
    let directory_lock = match acquire_lock(
        &std::env::temp_dir().join(format!("squallz-update-directory-{directory_key}.lock")),
        "directory coordination",
        ctl,
    ) {
        Ok(lock) => lock,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };

    let target = match canonical_update_target(&requested) {
        Ok(target) => target,
        Err(error) => {
            let error = guarded_target_resolution_error(&requested, error);
            return Err(cleanup_stage(error, stage));
        }
    };
    let key = match path_key(b"squallz-update-target-v1\0", &target) {
        Ok(key) => key,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };
    let target_lock = match acquire_lock(
        &std::env::temp_dir().join(format!("squallz-update-target-{key}.lock")),
        "target coordination",
        ctl,
    ) {
        Ok(lock) => lock,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };

    let recovery_control = ControlToken::default();
    if let Err(error) = recover_existing(
        &target,
        &key,
        ProgressPhase::OutputRecovery,
        progress,
        &recovery_control,
    ) {
        return Err(cleanup_stage(error, stage));
    }
    if let Err(error) = reject_unjournaled_artifacts(&target, &key) {
        return Err(cleanup_stage(error, stage));
    }
    if let Err(error) = ctl.checkpoint() {
        return Err(cleanup_stage(error, stage));
    }

    progress.on_phase(ProgressPhase::OutputVerify, true);
    let source_path_state_digest = match verify_destination_guard_with_progress(
        &requested,
        CreateArtifactKind::Archive,
        guard,
        progress,
        ctl,
    ) {
        Ok(digest) => digest,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };
    let source = match bind_source(&target) {
        Ok(source) => source,
        Err(error) => {
            let error = guarded_source_bind_error(&requested, guard, error);
            return Err(cleanup_stage(error, stage));
        }
    };
    if let Err(error) = validate_source(&target, &source) {
        let error = guarded_state_error_or_original(&requested, guard, error);
        return Err(cleanup_stage(error, stage));
    }

    let stage = adopt_created_stage(stage, &target, &key)?;
    let stage = match prepare_stage(stage) {
        Ok(stage) => stage,
        Err((error, stage)) => return Err(cleanup_stage(error, stage)),
    };
    if let Err(error) = ctl.checkpoint() {
        return Err(cleanup_prepared_stage(error, stage));
    }
    let mut digests = match bind_transaction_digests(&target, &source, &stage, progress, ctl) {
        Ok(digests) => digests,
        Err(error) => {
            let error = guarded_state_error_or_original(&requested, guard, error);
            return Err(cleanup_prepared_stage(error, stage));
        }
    };
    if let Err(error) = verify_path_state_digest(source_path_state_digest, &target, &requested) {
        return Err(cleanup_prepared_stage(error, stage));
    }
    if let Err(error) = validate_source(&target, &source) {
        let error = guarded_state_error_or_original(&requested, guard, error);
        return Err(cleanup_prepared_stage(error, stage));
    }
    digests.source_path_state = Some(source_path_state_digest);

    let (holder, holder_identity) = match reserve_holder(&target, &key) {
        Ok(holder) => holder,
        Err(error) => return Err(cleanup_prepared_stage(error, stage)),
    };
    let transaction = transaction_for(
        &target,
        &key,
        &source,
        &stage,
        digests,
        holder,
        holder_identity,
    );
    let record = match record_for(&transaction, &key) {
        Ok(record) => record,
        Err(error) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
    };
    progress.on_phase(ProgressPhase::OutputCommit, false);
    if let Err(error) = ctl.checkpoint() {
        let error = cleanup_empty_holder(error, &transaction);
        return Err(cleanup_prepared_stage(error, stage));
    }
    let active_stage = match retain_prepared_stage(&transaction, &stage) {
        Ok(active_stage) => active_stage,
        Err(error) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
    };
    let journal = match write_journal(&transaction, record) {
        Ok(journal) => journal,
        Err(JournalPublishError::BeforePublish(error)) => {
            let error = cleanup_empty_holder(error, &transaction);
            return Err(cleanup_prepared_stage(error, stage));
        }
        Err(JournalPublishError::Published(error)) => return Err(error),
    };
    let commit_control = ControlToken::default();
    let installed_verification =
        resume_transaction(&transaction, Some(active_stage), progress, &commit_control)?;
    drop(stage);
    let completion = publish_completion(journal, &transaction)?;
    drop(source);
    progress.on_phase(ProgressPhase::OutputCleanup, false);
    let result = cleanup_completed(
        completion,
        &transaction,
        Some(installed_verification),
        progress,
        &commit_control,
    );
    drop(target_lock);
    drop(directory_lock);
    result
}

fn guarded_target_resolution_error(destination: &Path, error: FormatError) -> FormatError {
    match &error {
        FormatError::Unsupported(_) => FormatError::destination_changed(destination.to_path_buf()),
        FormatError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            FormatError::destination_changed(destination.to_path_buf())
        }
        _ => error,
    }
}

fn guarded_state_error_or_original(
    destination: &Path,
    guard: CreateDestinationGuard,
    original: FormatError,
) -> FormatError {
    if matches!(&original, FormatError::Cancelled) {
        return original;
    }
    match verify_destination_guard(destination, CreateArtifactKind::Archive, guard) {
        Ok(_) => original,
        Err(error) => error,
    }
}

fn guarded_source_bind_error(
    destination: &Path,
    guard: CreateDestinationGuard,
    original: FormatError,
) -> FormatError {
    match &original {
        FormatError::Unsupported(_) => {
            return FormatError::destination_changed(destination.to_path_buf());
        }
        FormatError::Io(error) if error.kind() == io::ErrorKind::NotFound => {
            return FormatError::destination_changed(destination.to_path_buf());
        }
        _ => {}
    }
    guarded_state_error_or_original(destination, guard, original)
}

fn addition_bytes(additions: &dyn PreparedUpdateAdditions) -> Result<u64, FormatError> {
    let mut bytes = 0u64;
    for index in 0..additions.len() {
        let meta = additions.meta(index).ok_or_else(|| {
            FormatError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared update entry index is out of range",
            ))
        })?;
        bytes = bytes.saturating_add(meta.size);
    }
    Ok(bytes)
}

fn canonical_requested_target(target: &Path) -> Result<PathBuf, FormatError> {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid archive file name".into()))?;
    Ok(fs::canonicalize(parent_or_current(target))?.join(name))
}

fn canonical_update_target(target: &Path) -> Result<PathBuf, FormatError> {
    let requested = canonical_requested_target(target)?;
    match fs::symlink_metadata(&requested) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(FormatError::Unsupported(format!(
                "archive updates do not follow a symbolic-link target: {}",
                requested.display()
            )))
        }
        Ok(metadata) if metadata.is_file() => Ok(fs::canonicalize(requested)?),
        Ok(_) => Err(FormatError::Unsupported(format!(
            "archive update target is not a regular file: {}",
            requested.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(requested),
        Err(error) => Err(error.into()),
    }
}

fn bind_source(target: &Path) -> Result<BoundSource, FormatError> {
    let metadata = fs::symlink_metadata(target)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FormatError::Unsupported(format!(
            "archive update target is not a regular file: {}",
            target.display()
        )));
    }
    let file = open_regular_file_no_follow(target)?;
    let file_metadata = file.metadata()?;
    let identity = file_identity(&file)?;
    if !file_metadata.is_file() || path_identity(target)? != identity {
        return Err(FormatError::Io(io::Error::other(format!(
            "archive update target changed while it was opened: {}",
            target.display()
        ))));
    }
    Ok(BoundSource {
        state: RegularFileState::from_metadata(&file_metadata),
        permissions: file_metadata.permissions(),
        file,
        identity,
    })
}

fn validate_source(target: &Path, source: &BoundSource) -> Result<(), FormatError> {
    let metadata = source.file.metadata()?;
    if file_identity(&source.file)? != source.identity
        || path_identity(target)? != source.identity
        || !source.state.matches(&metadata)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "archive update target changed while the replacement was being written: {}",
            target.display()
        ))));
    }
    Ok(())
}

fn bind_transaction_digests(
    target: &Path,
    source: &BoundSource,
    stage: &PreparedStage,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<TransactionDigests, FormatError> {
    let total = source.state.bytes().saturating_add(stage.state.bytes());
    let label = verification_label(target);
    let source_digest = digest_bound_file(
        &source.file,
        target,
        source.identity,
        &source.state,
        "archive update source",
        0,
        total,
        &label,
        progress,
        ctl,
    )?;
    let staging_digest = digest_bound_file(
        &stage.file,
        &stage.path,
        stage.identity,
        &stage.state,
        "archive update staging",
        source.state.bytes(),
        total,
        &label,
        progress,
        ctl,
    )?;
    progress.on_progress(total, total, &label);
    Ok(TransactionDigests {
        source: source_digest,
        staging: staging_digest,
        source_path_state: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn digest_bound_file(
    file: &File,
    path: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    role: &str,
    completed_before: u64,
    total: u64,
    label: &EntryPath,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<[u8; 32], FormatError> {
    let metadata = file.metadata()?;
    if file_identity(file)? != identity
        || path_identity(path)? != identity
        || !state.matches(&metadata)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "{role} changed before content verification: {}",
            path.display()
        ))));
    }

    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; DIGEST_BUFFER_BYTES];
    let mut completed = 0u64;
    progress.on_progress(completed_before, total, label);
    loop {
        ctl.checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        completed = completed.saturating_add(read as u64);
        progress.on_progress(completed_before.saturating_add(completed), total, label);
    }
    if completed != state.bytes() {
        return Err(FormatError::Io(io::Error::other(format!(
            "{role} length changed during content verification at {}",
            path.display()
        ))));
    }
    let metadata = file.metadata()?;
    if file_identity(file)? != identity
        || path_identity(path)? != identity
        || !state.matches(&metadata)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "{role} changed during content verification: {}",
            path.display()
        ))));
    }
    Ok(*hasher.finalize().as_bytes())
}

fn verification_label(target: &Path) -> EntryPath {
    EntryPath::from_utf8(
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.display().to_string()),
    )
}

fn bind_created_stage(
    path: &Path,
    file: File,
    expected_identity: PathIdentity,
) -> Result<ReservedStage, FormatError> {
    let identity =
        file_identity(&file).map_err(|error| unbound_created_stage_error(path, error))?;
    if identity != expected_identity {
        return Err(FormatError::Io(io::Error::other(format!(
            "created archive staging was replaced after writing and the competing path was left untouched: {}",
            path.display()
        ))));
    }
    let stage = ReservedStage {
        path: path.to_path_buf(),
        file,
        identity,
    };
    let path_metadata = match fs::symlink_metadata(&stage.path) {
        Ok(metadata) => metadata,
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    let file_metadata = match stage.file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    let path_identity_matches =
        path_identity(&stage.path).is_ok_and(|identity| identity == stage.identity);
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || !file_metadata.is_file()
        || !path_identity_matches
    {
        return Err(cleanup_stage(
            FormatError::Io(io::Error::other(format!(
                "created archive staging changed while it was opened: {}",
                stage.path.display()
            ))),
            stage,
        ));
    }
    if let Err(error) = stage.file.sync_all() {
        return Err(cleanup_stage(error.into(), stage));
    }
    let metadata = match stage.file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    let file_identity_matches = file_identity(&stage.file).is_ok_and(|id| id == stage.identity);
    let path_identity_matches = path_identity(&stage.path).is_ok_and(|id| id == stage.identity);
    if !file_identity_matches || !path_identity_matches || !metadata.is_file() {
        return Err(cleanup_stage(
            FormatError::Io(io::Error::other(format!(
                "created archive staging changed while it was synchronized: {}",
                stage.path.display()
            ))),
            stage,
        ));
    }
    Ok(stage)
}

fn unbound_created_stage_error(path: &Path, error: io::Error) -> FormatError {
    FormatError::from(io::Error::new(
        error.kind(),
        format!(
            "{error}; created archive staging ownership could not be verified, so the path was left untouched for recovery: {}",
            path.display()
        ),
    ))
}

fn adopt_created_stage(
    mut stage: ReservedStage,
    target: &Path,
    key: &str,
) -> Result<ReservedStage, FormatError> {
    let canonical_stage = match canonical_requested_target(&stage.path) {
        Ok(path) => path,
        Err(error) => return Err(cleanup_stage(error, stage)),
    };
    if parent_or_current(&canonical_stage) != parent_or_current(target) {
        return Err(cleanup_stage(
            FormatError::Unsupported(
                "created archive staging must be next to its destination".into(),
            ),
            stage,
        ));
    }
    let prefix = format!(".squallz-update-stage-{}-", &key[..16]);
    for _ in 0..1000u32 {
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let adopted = parent_or_current(target)
            .join(format!("{prefix}{}-{sequence}.tmp", std::process::id()));
        match crate::move_path_no_replace(&stage.path, &adopted) {
            Ok(()) => {
                let original = std::mem::replace(&mut stage.path, adopted);
                if let Err(error) = sync_rename_parents(&original, &stage.path) {
                    return Err(cleanup_stage(error.into(), stage));
                }
                let metadata = match stage.file.metadata() {
                    Ok(metadata) => metadata,
                    Err(error) => return Err(cleanup_stage(error.into(), stage)),
                };
                let file_identity_matches =
                    file_identity(&stage.file).is_ok_and(|id| id == stage.identity);
                let path_identity_matches =
                    path_identity(&stage.path).is_ok_and(|id| id == stage.identity);
                if !file_identity_matches || !path_identity_matches || !metadata.is_file() {
                    return Err(cleanup_stage(
                        FormatError::Io(io::Error::other(format!(
                            "created archive staging changed while it was adopted: {}",
                            stage.path.display()
                        ))),
                        stage,
                    ));
                }
                return Ok(stage);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(cleanup_stage(error.into(), stage)),
        }
    }
    Err(cleanup_stage(
        FormatError::Unsupported(format!(
            "could not adopt created archive staging next to {}",
            target.display()
        )),
        stage,
    ))
}

fn reserve_stage(target: &Path, key: &str) -> Result<ReservedStage, FormatError> {
    let prefix = format!(".squallz-update-stage-{}-", &key[..16]);
    for _ in 0..1000u32 {
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent_or_current(target)
            .join(format!("{prefix}{}-{sequence}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            };

            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        match options.open(&path) {
            Ok(file) => {
                let identity = file_identity(&file)?;
                if path_identity(&path)? != identity || !file.metadata()?.is_file() {
                    drop(file);
                    return Err(FormatError::Io(io::Error::other(format!(
                        "archive update staging file changed while it was reserved: {}",
                        path.display()
                    ))));
                }
                return Ok(ReservedStage {
                    path,
                    file,
                    identity,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve update staging next to {}",
        target.display()
    )))
}

fn prepare_stage(stage: ReservedStage) -> Result<PreparedStage, (FormatError, ReservedStage)> {
    let metadata = match stage.file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return Err((error.into(), stage)),
    };
    let file_identity_matches = file_identity(&stage.file).is_ok_and(|id| id == stage.identity);
    let path_identity_matches = path_identity(&stage.path).is_ok_and(|id| id == stage.identity);
    if !metadata.is_file() || !file_identity_matches || !path_identity_matches {
        return Err((
            FormatError::Io(io::Error::other(format!(
                "archive update staging file changed before commit: {}",
                stage.path.display()
            ))),
            stage,
        ));
    }
    Ok(PreparedStage {
        path: stage.path,
        file: stage.file,
        identity: stage.identity,
        state: RegularFileState::from_metadata(&metadata),
    })
}

fn retain_prepared_stage(
    transaction: &ResolvedTransaction,
    stage: &PreparedStage,
) -> Result<VerifiedBoundFile, FormatError> {
    let metadata = stage.file.metadata().map_err(|error| {
        transaction_error(
            transaction,
            format!("could not inspect the retained archive staging file: {error}"),
        )
    })?;
    if !metadata.is_file()
        || file_identity(&stage.file).ok() != Some(stage.identity)
        || path_identity(&stage.path).ok() != Some(stage.identity)
        || !stage.state.matches(&metadata)
    {
        return Err(transaction_error(
            transaction,
            format!(
                "the retained archive staging file changed before commit: {}",
                stage.path.display()
            ),
        ));
    }
    let file = stage.file.try_clone().map_err(|error| {
        transaction_error(
            transaction,
            format!("could not retain the archive staging file for commit: {error}"),
        )
    })?;
    Ok(VerifiedBoundFile {
        file,
        identity: stage.identity,
        state: stage.state.clone(),
    })
}

fn cleanup_stage(error: FormatError, stage: ReservedStage) -> FormatError {
    cleanup_bound_stage(error, stage.path, stage.file, stage.identity, None)
}

fn cleanup_prepared_stage(error: FormatError, stage: PreparedStage) -> FormatError {
    cleanup_bound_stage(
        error,
        stage.path,
        stage.file,
        stage.identity,
        Some(&stage.state),
    )
}

fn cleanup_bound_stage(
    original: FormatError,
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: Option<&RegularFileState>,
) -> FormatError {
    let cleanup = (|| -> Result<(), FormatError> {
        let metadata = file.metadata()?;
        if file_identity(&file)? != identity
            || path_identity(&path)? != identity
            || !metadata.is_file()
            || state.is_some_and(|state| !state.matches(&metadata))
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "update staging changed before cleanup and was left untouched: {}",
                path.display()
            ))));
        }
        remove_open_file(&path, &file, metadata.permissions())?;
        sync_directory(parent_or_current(&path))?;
        Ok(())
    })();
    match cleanup {
        Ok(()) => original,
        Err(cleanup) => FormatError::Io(io::Error::other(format!(
            "{original}; update staging cleanup also failed: {cleanup}"
        ))),
    }
}

fn reserve_holder(target: &Path, key: &str) -> Result<(PathBuf, PathIdentity), FormatError> {
    let prefix = format!(".squallz-update-holder-{}-", &key[..16]);
    for _ in 0..1000u32 {
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let holder =
            parent_or_current(target).join(format!("{prefix}{}-{sequence}", std::process::id()));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        match builder.create(&holder) {
            Ok(()) => {
                let identity = path_identity(&holder)?;
                if let Err(error) = sync_directory(parent_or_current(&holder)) {
                    let _ = fs::remove_dir(&holder);
                    return Err(error.into());
                }
                return Ok((holder, identity));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve an update transaction holder next to {}",
        target.display()
    )))
}

fn transaction_for(
    target: &Path,
    key: &str,
    source: &BoundSource,
    stage: &PreparedStage,
    digests: TransactionDigests,
    holder: PathBuf,
    holder_identity: PathIdentity,
) -> ResolvedTransaction {
    let journal = journal_path(target, key);
    let completion = completion_path(target, key);
    ResolvedTransaction {
        target: target.to_path_buf(),
        staging: stage.path.clone(),
        previous: holder.join("previous"),
        replacement: holder.join("replacement"),
        retired: holder.join("retired"),
        holder,
        pending: pending_path(target, key),
        journal,
        completion,
        source_identity: source.identity,
        source_state: source.state.clone(),
        source_digest: digests.source,
        source_path_state_digest: digests.source_path_state,
        staging_identity: stage.identity,
        staging_state: stage.state.clone(),
        staging_digest: digests.staging,
        holder_identity,
    }
}

fn record_for(
    transaction: &ResolvedTransaction,
    key: &str,
) -> Result<TransactionRecord, FormatError> {
    Ok(TransactionRecord {
        version: if transaction.source_path_state_digest.is_some() {
            TRANSACTION_VERSION
        } else {
            TRANSACTION_VERSION_V1
        },
        key: key.to_owned(),
        target_name: component_string(&transaction.target, "archive update target")?,
        staging_name: component_string(&transaction.staging, "update staging")?,
        holder_name: component_string(&transaction.holder, "update transaction holder")?,
        source_identity: transaction.source_identity,
        source_state: transaction.source_state.clone(),
        source_digest: transaction.source_digest,
        source_path_state_digest: transaction.source_path_state_digest,
        staging_identity: transaction.staging_identity,
        staging_state: transaction.staging_state.clone(),
        staging_digest: transaction.staging_digest,
        holder_identity: transaction.holder_identity,
    })
}

fn write_journal(
    transaction: &ResolvedTransaction,
    record: TransactionRecord,
) -> Result<OpenRecord, JournalPublishError> {
    for (path, role) in [
        (&transaction.pending, "pending update record"),
        (&transaction.journal, "update transaction journal"),
        (&transaction.completion, "completed update record"),
    ] {
        if path_exists(path).map_err(JournalPublishError::BeforePublish)? {
            return Err(JournalPublishError::BeforePublish(FormatError::Io(
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{role} already exists: {}", path.display()),
                ),
            )));
        }
    }
    let bytes = serde_json::to_vec(&record).map_err(|error| {
        JournalPublishError::BeforePublish(FormatError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            error,
        )))
    })?;
    if bytes.len() > JOURNAL_MAX_BYTES {
        return Err(JournalPublishError::BeforePublish(
            FormatError::ResourceLimitExceeded(format!(
                "update transaction journal exceeds {JOURNAL_MAX_BYTES} bytes"
            )),
        ));
    }
    let (temp, mut file, identity) = reserve_record_temp(transaction, &record.key)
        .map_err(JournalPublishError::BeforePublish)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        return Err(cleanup_unpublished_record(
            transaction,
            temp,
            file,
            identity,
            error.into(),
        ));
    }
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(cleanup_unpublished_record(
                transaction,
                temp,
                file,
                identity,
                error.into(),
            ));
        }
    };
    if file_identity(&file).ok() != Some(identity)
        || path_identity(&temp).ok() != Some(identity)
        || !metadata.is_file()
    {
        return Err(cleanup_unpublished_record(
            transaction,
            temp,
            file,
            identity,
            FormatError::Io(io::Error::other(
                "update transaction record changed while it was written",
            )),
        ));
    }
    if let Err(error) = verify_open_bytes(&mut file, &bytes) {
        return Err(cleanup_unpublished_record(
            transaction,
            temp,
            file,
            identity,
            error.into(),
        ));
    }
    let open = OpenRecord {
        path: temp,
        file,
        identity,
        state: RegularFileState::from_metadata(&metadata),
        bytes,
        record,
    };
    let pending = match move_open_record(
        open,
        &transaction.pending,
        transaction,
        "pending update record",
    ) {
        Ok(open) => open,
        Err(OpenRecordMoveError::NotMoved(open, error)) => {
            return Err(cleanup_unpublished_open_record(transaction, *open, error));
        }
        Err(OpenRecordMoveError::Published(error)) => {
            return Err(JournalPublishError::Published(error));
        }
    };
    publish_journal(pending, transaction).map_err(JournalPublishError::Published)
}

fn reserve_record_temp(
    transaction: &ResolvedTransaction,
    key: &str,
) -> Result<(PathBuf, File, PathIdentity), FormatError> {
    let prefix = format!(".squallz-update-journal-{}-", &key[..16]);
    for _ in 0..1000u32 {
        let sequence = ARTIFACT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent_or_current(&transaction.target)
            .join(format!("{prefix}{}-{sequence}.tmp", std::process::id()));
        match open_new_artifact(&path) {
            Ok(file) => {
                let identity = file_identity(&file)?;
                if path_identity(&path)? != identity || !file.metadata()?.is_file() {
                    return Err(FormatError::Io(io::Error::other(format!(
                        "update transaction record changed while it was reserved: {}",
                        path.display()
                    ))));
                }
                return Ok((path, file, identity));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve an update transaction record next to {}",
        transaction.target.display()
    )))
}

fn cleanup_unpublished_open_record(
    transaction: &ResolvedTransaction,
    open: OpenRecord,
    original: FormatError,
) -> JournalPublishError {
    cleanup_unpublished_record(transaction, open.path, open.file, open.identity, original)
}

fn cleanup_unpublished_record(
    transaction: &ResolvedTransaction,
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    original: FormatError,
) -> JournalPublishError {
    let cleanup = (|| -> Result<(), FormatError> {
        let metadata = file.metadata()?;
        if file_identity(&file)? != identity
            || path_identity(&path)? != identity
            || metadata.file_type().is_symlink()
            || !metadata.is_file()
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "unpublished update record changed and was left untouched: {}",
                path.display()
            ))));
        }
        remove_open_file(&path, &file, metadata.permissions())?;
        sync_directory(parent_or_current(&path))?;
        Ok(())
    })();
    match cleanup {
        Ok(()) => JournalPublishError::BeforePublish(original),
        Err(cleanup) => JournalPublishError::Published(transaction_error(
            transaction,
            format!(
                "{original}; the unpublished transaction record could not be cleaned safely: {cleanup}"
            ),
        )),
    }
}

fn recover_existing(
    target: &Path,
    key: &str,
    recovery_phase: ProgressPhase,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let pending = pending_path(target, key);
    let journal = journal_path(target, key);
    let completion = completion_path(target, key);
    let pending_exists = path_exists(&pending)?;
    let journal_exists = path_exists(&journal)?;
    let completion_exists = path_exists(&completion)?;
    if [pending_exists, journal_exists, completion_exists]
        .into_iter()
        .filter(|exists| *exists)
        .count()
        > 1
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "multiple update transaction records exist for {}; inspect {}, {}, and {}",
            target.display(),
            pending.display(),
            journal.display(),
            completion.display()
        ))));
    }
    if pending_exists || journal_exists || completion_exists {
        progress.on_phase(recovery_phase, false);
    }
    if completion_exists {
        let open = open_record(&completion, target, "completed update transaction")?;
        let transaction = resolve_transaction(target, key, &open.record)?;
        return cleanup_completed(open, &transaction, None, progress, ctl);
    }
    if journal_exists {
        let open = open_record(&journal, target, "update transaction journal")?;
        let transaction = resolve_transaction(target, key, &open.record)?;
        let installed_verification = resume_transaction(&transaction, None, progress, ctl)?;
        let completion = publish_completion(open, &transaction)?;
        return cleanup_completed(
            completion,
            &transaction,
            Some(installed_verification),
            progress,
            ctl,
        );
    }
    if pending_exists {
        let open = open_record(&pending, target, "pending update transaction")?;
        let transaction = resolve_transaction(target, key, &open.record)?;
        let journal = publish_journal(open, &transaction)?;
        let installed_verification = resume_transaction(&transaction, None, progress, ctl)?;
        let completion = publish_completion(journal, &transaction)?;
        return cleanup_completed(
            completion,
            &transaction,
            Some(installed_verification),
            progress,
            ctl,
        );
    }
    Ok(())
}

fn open_record(path: &Path, target: &Path, description: &str) -> Result<OpenRecord, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FormatError::Io(io::Error::other(format!(
            "{description} is not a regular file: {}",
            path.display()
        ))));
    }
    let mut file = open_regular_file_no_follow(path)?;
    let identity = file_identity(&file)?;
    let state = RegularFileState::from_metadata(&file.metadata()?);
    if path_identity(path)? != identity {
        return Err(FormatError::Io(io::Error::other(format!(
            "{description} changed while it was opened: {}",
            path.display()
        ))));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((JOURNAL_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > JOURNAL_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "{description} exceeds {JOURNAL_MAX_BYTES} bytes"
        )));
    }
    if file_identity(&file)? != identity
        || path_identity(path)? != identity
        || !state.matches(&file.metadata()?)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "{description} changed while it was read: {}",
            path.display()
        ))));
    }
    let record = serde_json::from_slice(&bytes).map_err(|error| {
        FormatError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{description} is invalid for {}: {error}", target.display()),
        ))
    })?;
    Ok(OpenRecord {
        path: path.to_path_buf(),
        file,
        identity,
        state,
        bytes,
        record,
    })
}

fn resolve_transaction(
    target: &Path,
    key: &str,
    record: &TransactionRecord,
) -> Result<ResolvedTransaction, FormatError> {
    match (record.version, record.source_path_state_digest) {
        (TRANSACTION_VERSION_V1, None) | (TRANSACTION_VERSION, Some(_)) => {}
        (TRANSACTION_VERSION_V1, Some(_)) => {
            return Err(FormatError::Unsupported(
                "version 1 update transactions cannot contain a guarded destination digest".into(),
            ));
        }
        (TRANSACTION_VERSION, None) => {
            return Err(FormatError::Unsupported(
                "version 2 update transaction is missing its guarded destination digest".into(),
            ));
        }
        (version, _) => {
            return Err(FormatError::Unsupported(format!(
                "unsupported update transaction version: {version}"
            )));
        }
    }
    if record.key != key {
        return Err(FormatError::Unsupported(
            "update transaction belongs to a different target key".into(),
        ));
    }
    let target_name = checked_component(&record.target_name, "update target")?;
    if target.file_name() != Some(target_name.as_os_str()) {
        return Err(FormatError::Unsupported(
            "update transaction belongs to a different archive".into(),
        ));
    }
    let staging_name = checked_component(&record.staging_name, "update staging")?;
    let holder_name = checked_component(&record.holder_name, "update holder")?;
    if !staging_name_is_reserved(&record.staging_name, key)
        || !holder_name_is_reserved(&record.holder_name, key)
    {
        return Err(FormatError::Unsupported(
            "update transaction contains an invalid internal path".into(),
        ));
    }
    if record.source_identity == record.staging_identity
        || record.source_identity == record.holder_identity
        || record.staging_identity == record.holder_identity
    {
        return Err(FormatError::Unsupported(
            "update transaction contains ambiguous file identities".into(),
        ));
    }
    let parent = parent_or_current(target);
    let holder = parent.join(holder_name);
    Ok(ResolvedTransaction {
        target: target.to_path_buf(),
        staging: parent.join(staging_name),
        previous: holder.join("previous"),
        replacement: holder.join("replacement"),
        retired: holder.join("retired"),
        pending: pending_path(target, key),
        journal: journal_path(target, key),
        completion: completion_path(target, key),
        holder,
        source_identity: record.source_identity,
        source_state: record.source_state.clone(),
        source_digest: record.source_digest,
        source_path_state_digest: record.source_path_state_digest,
        staging_identity: record.staging_identity,
        staging_state: record.staging_state.clone(),
        staging_digest: record.staging_digest,
        holder_identity: record.holder_identity,
    })
}

fn resume_transaction(
    transaction: &ResolvedTransaction,
    active_stage: Option<VerifiedBoundFile>,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<VerifiedBoundFile, FormatError> {
    let mut snapshot = transaction_snapshot(transaction)?;
    let mut replacement_verification = active_stage;
    let mut source_state_verified_at_previous = false;
    let mut installed_verification = None;
    if is_expected(
        snapshot.staging.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::Exact,
    ) && snapshot.replacement.is_none()
        && is_expected(
            snapshot.target.as_ref(),
            transaction.source_identity,
            &transaction.source_state,
            StateMatch::Exact,
        )
        && snapshot.previous.is_none()
        && snapshot.retired.is_none()
        && snapshot.holder_present
    {
        verify_guarded_source_state(transaction, &transaction.target)?;
        let moved_stage = move_bound_retained(
            transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            progress,
            ctl,
            replacement_verification.take(),
        )?;
        replacement_verification = Some(moved_stage);
        snapshot = transaction_snapshot(transaction)?;
    }

    if is_expected(
        snapshot.replacement.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::AfterRename,
    ) && is_expected(
        snapshot.target.as_ref(),
        transaction.source_identity,
        &transaction.source_state,
        StateMatch::Exact,
    ) && snapshot.previous.is_none()
        && snapshot.staging.is_none()
        && snapshot.retired.is_none()
        && snapshot.holder_present
    {
        verify_guarded_source_state(transaction, &transaction.target)?;
        replacement_verification = verify_transaction_digest_with_retained(
            transaction,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_digest,
            "staged replacement",
            progress,
            ctl,
            replacement_verification.take(),
            false,
        )?;
        if replacement_verification.is_none() {
            return Err(transaction_error(
                transaction,
                "the staged replacement content changed before the target move".into(),
            ));
        }
        move_bound(
            transaction,
            &transaction.target,
            &transaction.previous,
            transaction.source_identity,
            &transaction.source_state,
            &transaction.source_digest,
            StateMatch::Exact,
            "previous archive",
            progress,
            ctl,
        )?;
        snapshot = transaction_snapshot(transaction)?;
    }

    if is_expected(
        snapshot.replacement.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::AfterRename,
    ) && snapshot.target.is_none()
        && is_expected(
            snapshot.previous.as_ref(),
            transaction.source_identity,
            &transaction.source_state,
            StateMatch::AfterRename,
        )
        && snapshot.staging.is_none()
        && snapshot.retired.is_none()
        && snapshot.holder_present
    {
        verify_guarded_source_state(transaction, &transaction.previous)?;
        source_state_verified_at_previous = true;
        let replacement_verification = verify_transaction_digest_with_retained(
            transaction,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_digest,
            "staged replacement",
            progress,
            ctl,
            replacement_verification.take(),
            false,
        )?
        .ok_or_else(|| {
            transaction_error(
                transaction,
                "the staged replacement content changed before installation".into(),
            )
        })?;
        installed_verification = Some(move_bound_retained(
            transaction,
            &transaction.replacement,
            &transaction.target,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::AfterRename,
            "updated archive",
            progress,
            ctl,
            Some(replacement_verification),
        )?);
        snapshot = transaction_snapshot(transaction)?;
    }

    let installed = is_expected(
        snapshot.target.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::AfterRename,
    );
    let previous_is_bound = is_expected(
        snapshot.previous.as_ref(),
        transaction.source_identity,
        &transaction.source_state,
        StateMatch::AfterRename,
    );
    if previous_is_bound && !source_state_verified_at_previous {
        verify_guarded_source_state(transaction, &transaction.previous)?;
    }
    let old_is_absent = snapshot.previous.is_none() && snapshot.retired.is_none();
    if !installed
        || snapshot.staging.is_some()
        || snapshot.replacement.is_some()
        || !(previous_is_bound || old_is_absent)
        || snapshot.retired.is_some()
        || (previous_is_bound && !snapshot.holder_present)
    {
        return Err(transaction_error(
            transaction,
            format!(
                "update transaction paths do not match a recoverable state: {}",
                describe_snapshot(&snapshot)
            ),
        ));
    }
    if installed_verification.as_ref().is_none_or(|verified| {
        !verified_bound_file_is_current(verified, &transaction.target, transaction.staging_identity)
    }) {
        installed_verification = verify_transaction_digest(
            transaction,
            &transaction.target,
            transaction.staging_identity,
            &transaction.staging_digest,
            "installed archive",
            progress,
            ctl,
        )?;
    }
    if installed_verification.is_none() {
        return Err(transaction_error(
            transaction,
            "the installed archive content does not match the staged replacement".into(),
        ));
    }
    sync_directory(parent_or_current(&transaction.target)).map_err(|error| {
        transaction_error(
            transaction,
            format!(
                "the updated archive may already be installed, but its directory could not be synchronized: {error}"
            ),
        )
    })?;
    if snapshot.holder_present {
        sync_directory(&transaction.holder).map_err(|error| {
            transaction_error(
                transaction,
                format!(
                    "the updated archive may already be installed, but its transaction holder could not be synchronized: {error}"
                ),
            )
        })?;
    }
    installed_verification.ok_or_else(|| {
        transaction_error(
            transaction,
            "the installed archive could not be retained for cleanup verification".into(),
        )
    })
}

fn verify_guarded_source_state(
    transaction: &ResolvedTransaction,
    current_path: &Path,
) -> Result<(), FormatError> {
    match transaction.source_path_state_digest {
        Some(expected) => verify_path_state_digest(expected, current_path, &transaction.target)
            .map_err(|error| {
                transaction_error(
                    transaction,
                    format!(
                        "the authorized previous archive state changed at {}: {error}",
                        current_path.display()
                    ),
                )
            }),
        None => Ok(()),
    }
}

fn publish_journal(
    open: OpenRecord,
    transaction: &ResolvedTransaction,
) -> Result<OpenRecord, FormatError> {
    if open.path != transaction.pending {
        return Err(transaction_error(
            transaction,
            "the pending update record is at an unexpected path".into(),
        ));
    }
    match move_open_record(
        open,
        &transaction.journal,
        transaction,
        "update transaction journal",
    ) {
        Ok(open) => Ok(open),
        Err(OpenRecordMoveError::NotMoved(_, error))
        | Err(OpenRecordMoveError::Published(error)) => Err(error),
    }
}

fn publish_completion(
    open: OpenRecord,
    transaction: &ResolvedTransaction,
) -> Result<OpenRecord, FormatError> {
    if open.path != transaction.journal {
        return Err(transaction_error(
            transaction,
            "the active update record is not the expected journal".into(),
        ));
    }
    match move_open_record(
        open,
        &transaction.completion,
        transaction,
        "completed update record",
    ) {
        Ok(open) => Ok(open),
        Err(OpenRecordMoveError::NotMoved(_, error))
        | Err(OpenRecordMoveError::Published(error)) => Err(error),
    }
}

fn move_open_record(
    open: OpenRecord,
    destination: &Path,
    transaction: &ResolvedTransaction,
    role: &str,
) -> Result<OpenRecord, OpenRecordMoveError> {
    move_open_record_with(
        open,
        destination,
        transaction,
        role,
        &mut |from, to| crate::move_path_no_replace(from, to),
        &mut sync_rename_parents,
    )
}

fn move_open_record_with<M, S>(
    mut open: OpenRecord,
    destination: &Path,
    transaction: &ResolvedTransaction,
    role: &str,
    move_no_replace: &mut M,
    sync_move: &mut S,
) -> Result<OpenRecord, OpenRecordMoveError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path, &Path) -> io::Result<()>,
{
    if let Err(error) = validate_open_record(&open) {
        return Err(OpenRecordMoveError::NotMoved(
            Box::new(open),
            transaction_error(
                transaction,
                format!("the {role} changed before publication: {error}"),
            ),
        ));
    }
    match path_exists(destination) {
        Ok(false) => {}
        Ok(true) => {
            return Err(OpenRecordMoveError::NotMoved(
                Box::new(open),
                transaction_error(
                    transaction,
                    format!(
                        "the {role} path is already occupied: {}",
                        destination.display()
                    ),
                ),
            ));
        }
        Err(error) => return Err(OpenRecordMoveError::NotMoved(Box::new(open), error)),
    }

    let source = open.path.clone();
    if let Err(error) = move_no_replace(&source, destination) {
        return Err(OpenRecordMoveError::NotMoved(
            Box::new(open),
            transaction_error(
                transaction,
                format!(
                    "could not publish the {role} from {} to {} without replacement: {error}",
                    source.display(),
                    destination.display()
                ),
            ),
        ));
    }

    let sync_error = sync_move(&source, destination).err();
    let source_after = match observe_entry_identity(&source) {
        Ok(identity) => identity,
        Err(error) => {
            return Err(OpenRecordMoveError::Published(transaction_error(
                transaction,
                format!("could not inspect the {role} source after publication: {error}"),
            )));
        }
    };
    let destination_after = match observe_entry_identity(destination) {
        Ok(identity) => identity,
        Err(error) => {
            return Err(OpenRecordMoveError::Published(transaction_error(
                transaction,
                format!("could not inspect the {role} after publication: {error}"),
            )));
        }
    };
    let expected_at_destination = source_after.is_none()
        && destination_after == Some(open.identity)
        && file_identity(&open.file).ok() == Some(open.identity);
    if expected_at_destination {
        if let Err(error) = verify_open_bytes(&mut open.file, &open.bytes) {
            let disposition =
                restore_unexpected_move(&source, destination, open.identity, move_no_replace);
            return Err(OpenRecordMoveError::Published(transaction_error(
                transaction,
                format!("the {role} contents changed during publication: {error}; {disposition}"),
            )));
        }
        let metadata = match open.file.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                return Err(OpenRecordMoveError::Published(transaction_error(
                    transaction,
                    format!("the published {role} is no longer a regular file"),
                )));
            }
            Err(error) => {
                return Err(OpenRecordMoveError::Published(transaction_error(
                    transaction,
                    format!("could not bind the published {role}: {error}"),
                )));
            }
        };
        open.path = destination.to_path_buf();
        open.state = RegularFileState::from_metadata(&metadata);
        if let Some(error) = sync_error {
            return Err(OpenRecordMoveError::Published(transaction_error(
                transaction,
                format!("the {role} was published but could not be synchronized: {error}"),
            )));
        }
        return Ok(open);
    }

    let disposition = match (source_after, destination_after) {
        (None, Some(identity)) => {
            restore_unexpected_move(&source, destination, identity, move_no_replace)
        }
        (Some(_), Some(_)) => format!(
            "both changed record paths are retained at {} and {}",
            source.display(),
            destination.display()
        ),
        (Some(_), None) => format!(
            "the source record is retained at {}, while {} is missing",
            source.display(),
            destination.display()
        ),
        (None, None) => format!(
            "both record paths are missing: {} and {}",
            source.display(),
            destination.display()
        ),
    };
    let sync_note = sync_error
        .map(|error| format!("; directory synchronization also failed: {error}"))
        .unwrap_or_default();
    Err(OpenRecordMoveError::Published(transaction_error(
        transaction,
        format!("the {role} changed during publication; {disposition}{sync_note}"),
    )))
}

fn cleanup_completed(
    completion: OpenRecord,
    transaction: &ResolvedTransaction,
    installed_verification: Option<VerifiedBoundFile>,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    validate_open_record(&completion).map_err(|error| {
        transaction_error(
            transaction,
            format!("the completed update record changed before cleanup: {error}"),
        )
    })?;
    if completion.path != transaction.completion {
        return Err(transaction_error(
            transaction,
            "the completed update record is at an unexpected path".into(),
        ));
    }
    let mut snapshot = transaction_snapshot(transaction)?;
    if !is_expected(
        snapshot.target.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::AfterRename,
    ) || snapshot.staging.is_some()
        || snapshot.replacement.is_some()
    {
        return Err(transaction_error(
            transaction,
            format!(
                "the installed archive changed before cleanup: {}",
                describe_snapshot(&snapshot)
            ),
        ));
    }
    let mut retired_verification = None;
    let retired_present = if snapshot.holder_present {
        if is_expected(
            snapshot.previous.as_ref(),
            transaction.source_identity,
            &transaction.source_state,
            StateMatch::AfterRename,
        ) && snapshot.retired.is_none()
        {
            verify_guarded_source_state(transaction, &transaction.previous)?;
            retired_verification = Some(move_bound(
                transaction,
                &transaction.previous,
                &transaction.retired,
                transaction.source_identity,
                &transaction.source_state,
                &transaction.source_digest,
                StateMatch::AfterRename,
                "retired archive",
                progress,
                ctl,
            )?);
            verify_guarded_source_state(transaction, &transaction.retired)?;
            snapshot = transaction_snapshot(transaction)?;
        }
        if snapshot.previous.is_some()
            || (snapshot.retired.is_some()
                && !is_expected(
                    snapshot.retired.as_ref(),
                    transaction.source_identity,
                    &transaction.source_state,
                    StateMatch::AfterRename,
                ))
        {
            return Err(transaction_error(
                transaction,
                format!(
                    "the previous archive changed before cleanup: {}",
                    describe_snapshot(&snapshot)
                ),
            ));
        }
        let retired_present = snapshot.retired.is_some();
        if retired_present && retired_verification.is_none() {
            verify_guarded_source_state(transaction, &transaction.retired)?;
            retired_verification = verify_transaction_digest(
                transaction,
                &transaction.retired,
                transaction.source_identity,
                &transaction.source_digest,
                "retired archive",
                progress,
                ctl,
            )?;
        }
        if retired_present && retired_verification.is_none() {
            return Err(transaction_error(
                transaction,
                format!(
                    "retired archive content changed before cleanup at {}",
                    transaction.retired.display()
                ),
            ));
        }
        retired_present
    } else if snapshot.previous.is_some() || snapshot.retired.is_some() {
        return Err(transaction_error(
            transaction,
            "the update holder is missing while an old archive path remains".into(),
        ));
    } else {
        false
    };

    let installed = observe_regular(transaction, &transaction.target, "installed archive")?;
    if !is_expected(
        installed.as_ref(),
        transaction.staging_identity,
        &transaction.staging_state,
        StateMatch::AfterRename,
    ) {
        return Err(transaction_error(
            transaction,
            "the installed archive changed before the transaction record was cleared".into(),
        ));
    }
    let installed_is_verified = installed_verification.as_ref().is_some_and(|verified| {
        verified_bound_file_is_current(verified, &transaction.target, transaction.staging_identity)
    });
    if !installed_is_verified
        && !transaction_digest_matches(
            transaction,
            &transaction.target,
            transaction.staging_identity,
            &transaction.staging_digest,
            "installed archive",
            progress,
            ctl,
        )?
    {
        return Err(transaction_error(
            transaction,
            "the installed archive content changed before the transaction record was cleared"
                .into(),
        ));
    }
    if retired_present {
        verify_guarded_source_state(transaction, &transaction.retired)?;
        if retired_verification.as_ref().is_none_or(|verified| {
            !verified_bound_file_is_current(
                verified,
                &transaction.retired,
                transaction.source_identity,
            )
        }) {
            retired_verification = verify_transaction_digest(
                transaction,
                &transaction.retired,
                transaction.source_identity,
                &transaction.source_digest,
                "retired archive",
                progress,
                ctl,
            )?;
        }
        let retired_verification = retired_verification.ok_or_else(|| {
            transaction_error(
                transaction,
                format!(
                    "retired archive content changed before removal at {}",
                    transaction.retired.display()
                ),
            )
        })?;
        remove_verified_bound_file(
            transaction,
            &transaction.retired,
            retired_verification,
            "retired archive",
        )?;
    }
    if snapshot.holder_present {
        remove_empty_holder(transaction)?;
    }
    validate_open_record(&completion).map_err(|error| {
        transaction_error(
            transaction,
            format!("the completed update record changed before removal: {error}"),
        )
    })?;
    fs::remove_file(&completion.path).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not remove the completed update record: {error}"),
        )
    })?;
    sync_directory(parent_or_current(&completion.path)).map_err(|error| {
        transaction_error(
            transaction,
            format!(
                "the archive is updated, but completion-record removal could not be synchronized: {error}"
            ),
        )
    })?;
    Ok(())
}

fn transaction_snapshot(
    transaction: &ResolvedTransaction,
) -> Result<TransactionSnapshot, FormatError> {
    let holder_present = validate_holder(transaction)?;
    Ok(TransactionSnapshot {
        staging: observe_regular(transaction, &transaction.staging, "update staging")?,
        target: observe_regular(transaction, &transaction.target, "archive target")?,
        previous: observe_regular(transaction, &transaction.previous, "previous archive")?,
        replacement: observe_regular(transaction, &transaction.replacement, "staged replacement")?,
        retired: observe_regular(transaction, &transaction.retired, "retired archive")?,
        holder_present,
    })
}

fn validate_holder(transaction: &ResolvedTransaction) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(&transaction.holder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || path_identity(&transaction.holder)? != transaction.holder_identity
    {
        return Err(transaction_error(
            transaction,
            "the update transaction holder identity or type changed".into(),
        ));
    }
    let mut count = 0usize;
    for entry in fs::read_dir(&transaction.holder)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != "previous" && name != "replacement" && name != "retired" {
            return Err(transaction_error(
                transaction,
                format!(
                    "the update transaction holder contains an unexpected path: {}",
                    entry.path().display()
                ),
            ));
        }
        count += 1;
        if count > 3 {
            return Err(transaction_error(
                transaction,
                "the update transaction holder contains too many entries".into(),
            ));
        }
    }
    Ok(true)
}

fn observe_regular(
    transaction: &ResolvedTransaction,
    path: &Path,
    role: &str,
) -> Result<Option<ObservedFile>, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(transaction_error(
            transaction,
            format!("{role} is not a regular file: {}", path.display()),
        ));
    }
    Ok(Some(ObservedFile {
        identity: path_identity(path)?,
        state: RegularFileState::from_metadata(&metadata),
    }))
}

fn is_expected(
    observed: Option<&ObservedFile>,
    identity: PathIdentity,
    state: &RegularFileState,
    state_match: StateMatch,
) -> bool {
    observed.is_some_and(|observed| {
        observed.identity == identity
            && match state_match {
                StateMatch::Exact => observed.state == *state,
                StateMatch::AfterRename => state.equivalent_after_rename(&observed.state),
            }
    })
}

#[allow(clippy::too_many_arguments)]
fn move_bound(
    transaction: &ResolvedTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    digest: &[u8; 32],
    source_match: StateMatch,
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<VerifiedBoundFile, FormatError> {
    move_bound_retained(
        transaction,
        source,
        destination,
        identity,
        state,
        digest,
        source_match,
        role,
        progress,
        ctl,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn move_bound_retained(
    transaction: &ResolvedTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    digest: &[u8; 32],
    source_match: StateMatch,
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    retained: Option<VerifiedBoundFile>,
) -> Result<VerifiedBoundFile, FormatError> {
    move_bound_with_retained(
        transaction,
        source,
        destination,
        identity,
        state,
        digest,
        source_match,
        role,
        progress,
        ctl,
        retained,
        &mut |from, to| crate::move_path_no_replace(from, to),
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn move_bound_with<M>(
    transaction: &ResolvedTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    digest: &[u8; 32],
    source_match: StateMatch,
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    move_no_replace: &mut M,
) -> Result<VerifiedBoundFile, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
{
    move_bound_with_retained(
        transaction,
        source,
        destination,
        identity,
        state,
        digest,
        source_match,
        role,
        progress,
        ctl,
        None,
        move_no_replace,
    )
}

#[allow(clippy::too_many_arguments)]
fn move_bound_with_retained<M>(
    transaction: &ResolvedTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    digest: &[u8; 32],
    source_match: StateMatch,
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    retained: Option<VerifiedBoundFile>,
    move_no_replace: &mut M,
) -> Result<VerifiedBoundFile, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
{
    let observed = observe_regular(transaction, source, role)?;
    if !is_expected(observed.as_ref(), identity, state, source_match) {
        return Err(transaction_error(
            transaction,
            format!("{role} changed at {}", source.display()),
        ));
    }
    if path_exists(destination)? {
        return Err(transaction_error(
            transaction,
            format!(
                "cannot move {role} without replacing the existing path at {}",
                destination.display()
            ),
        ));
    }
    let source_verification = match retained {
        Some(retained) => Some(
            verify_transaction_digest_with_retained(
                transaction,
                source,
                identity,
                digest,
                role,
                progress,
                ctl,
                Some(retained),
                false,
            )?
            .ok_or_else(|| {
                transaction_error(
                    transaction,
                    format!("the retained {role} changed before its transaction move"),
                )
            })?,
        ),
        None => None,
    };
    move_no_replace(source, destination).map_err(|error| {
        transaction_error(
            transaction,
            format!(
                "could not move {role} from {} to {} without replacement: {error}",
                source.display(),
                destination.display()
            ),
        )
    })?;
    let sync_error = sync_rename_parents(source, destination).err();
    let source_after = observe_entry_identity(source).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not inspect the {role} source after its transaction move: {error}"),
        )
    })?;
    let destination_after = observe_entry_identity(destination).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not inspect the {role} destination after its transaction move: {error}"),
        )
    })?;
    let destination_verification =
        if path_matches_expected(destination, identity, state, StateMatch::AfterRename)? {
            verify_transaction_digest_with_retained(
                transaction,
                destination,
                identity,
                digest,
                role,
                progress,
                ctl,
                source_verification,
                true,
            )?
        } else {
            None
        };
    let destination_is_expected = destination_verification.is_some();

    if source_after.is_none() && destination_is_expected {
        if let Some(error) = &sync_error {
            return Err(transaction_error(
                transaction,
                format!(
                    "the {role} move from {} to {} completed but could not be synchronized: {error}",
                    source.display(),
                    destination.display()
                ),
            ));
        }
        return destination_verification.ok_or_else(|| {
            transaction_error(
                transaction,
                format!("the moved {role} could not be retained for verification"),
            )
        });
    }

    let disposition = match (source_after, destination_after) {
        (None, Some(unexpected_identity)) if !destination_is_expected => {
            restore_unexpected_move(source, destination, unexpected_identity, move_no_replace)
        }
        (Some(_), Some(_)) if destination_is_expected => format!(
            "the expected {role} is retained at {}, and the source path is occupied at {}",
            destination.display(),
            source.display()
        ),
        (Some(_), Some(_)) => format!(
            "both changed entries are retained at {} and {}",
            source.display(),
            destination.display()
        ),
        (Some(_), None) => format!(
            "the source path is occupied at {}, while the moved entry is no longer present at {}",
            source.display(),
            destination.display()
        ),
        (None, None) => format!(
            "both transaction paths are now missing: {} and {}",
            source.display(),
            destination.display()
        ),
        (None, Some(_)) => format!(
            "the unexpected entry is retained at {}",
            destination.display()
        ),
    };
    let sync_note = sync_error
        .map(|error| format!("; the forward move also failed directory synchronization: {error}"))
        .unwrap_or_default();
    Err(transaction_error(
        transaction,
        format!("{role} changed during its transaction move; {disposition}{sync_note}"),
    ))
}

fn path_matches_expected(
    path: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
    state_match: StateMatch,
) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(false);
    }
    let observed = ObservedFile {
        identity: path_identity(path)?,
        state: RegularFileState::from_metadata(&metadata),
    };
    Ok(is_expected(Some(&observed), identity, state, state_match))
}

fn observe_entry_identity(path: &Path) -> io::Result<Option<PathIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(_) => path_identity(path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn transaction_digest_matches(
    transaction: &ResolvedTransaction,
    path: &Path,
    identity: PathIdentity,
    digest: &[u8; 32],
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<bool, FormatError> {
    Ok(
        verify_transaction_digest(transaction, path, identity, digest, role, progress, ctl)?
            .is_some(),
    )
}

#[allow(clippy::too_many_arguments)]
fn verify_transaction_digest_with_retained(
    transaction: &ResolvedTransaction,
    path: &Path,
    identity: PathIdentity,
    digest: &[u8; 32],
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    retained: Option<VerifiedBoundFile>,
    force_digest: bool,
) -> Result<Option<VerifiedBoundFile>, FormatError> {
    let Some(retained) = retained else {
        return verify_transaction_digest(transaction, path, identity, digest, role, progress, ctl);
    };
    if !force_digest {
        return Ok(verified_bound_file_is_current(&retained, path, identity).then_some(retained));
    }
    let handle_metadata = retained.file.metadata()?;
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if retained.identity != identity
        || !handle_metadata.is_file()
        || path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || file_identity(&retained.file)? != identity
        || path_identity(path)? != identity
    {
        return Ok(None);
    }
    let state = RegularFileState::from_metadata(&handle_metadata);
    if !state.matches(&path_metadata) {
        return Ok(None);
    }
    let label = verification_label(&transaction.target);
    let actual = digest_bound_file(
        &retained.file,
        path,
        identity,
        &state,
        role,
        0,
        state.bytes(),
        &label,
        progress,
        ctl,
    )
    .map_err(|error| {
        transaction_error(
            transaction,
            format!("could not verify retained {role} content: {error}"),
        )
    })?;
    if actual != *digest {
        return Ok(None);
    }
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_identity(path)? != identity
        || !state.matches(&path_metadata)
    {
        return Ok(None);
    }
    Ok(Some(VerifiedBoundFile {
        file: retained.file,
        identity,
        state,
    }))
}

fn verify_transaction_digest(
    transaction: &ResolvedTransaction,
    path: &Path,
    identity: PathIdentity,
    digest: &[u8; 32],
    role: &str,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<Option<VerifiedBoundFile>, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || path_identity(path)? != identity
    {
        return Ok(None);
    }
    let file = match open_regular_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file_identity(&file)? != identity || path_identity(path)? != identity {
        return Ok(None);
    }
    let state = RegularFileState::from_metadata(&file.metadata()?);
    let label = verification_label(&transaction.target);
    let actual = digest_bound_file(
        &file,
        path,
        identity,
        &state,
        role,
        0,
        state.bytes(),
        &label,
        progress,
        ctl,
    )
    .map_err(|error| {
        transaction_error(
            transaction,
            format!("could not verify {role} content: {error}"),
        )
    })?;
    if actual != *digest {
        return Ok(None);
    }
    Ok(Some(VerifiedBoundFile {
        file,
        identity,
        state,
    }))
}

fn verified_bound_file_is_current(
    verified: &VerifiedBoundFile,
    path: &Path,
    identity: PathIdentity,
) -> bool {
    if verified.identity != identity {
        return false;
    }
    let Ok(handle_metadata) = verified.file.metadata() else {
        return false;
    };
    if !handle_metadata.is_file()
        || file_identity(&verified.file).ok() != Some(identity)
        || !verified.state.matches(&handle_metadata)
    {
        return false;
    }
    let Ok(path_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    !path_metadata.file_type().is_symlink()
        && path_metadata.is_file()
        && path_identity(path).ok() == Some(identity)
        && verified.state.matches(&path_metadata)
}

fn restore_unexpected_move<M>(
    source: &Path,
    destination: &Path,
    unexpected_identity: PathIdentity,
    move_no_replace: &mut M,
) -> String
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
{
    match observe_entry_identity(source) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return format!(
                "the unexpected entry is retained at {} because {} is occupied",
                destination.display(),
                source.display()
            );
        }
        Err(error) => {
            return format!(
                "the unexpected entry is retained at {} because {} could not be inspected: {error}",
                destination.display(),
                source.display()
            );
        }
    }
    match observe_entry_identity(destination) {
        Ok(Some(identity)) if identity == unexpected_identity => {}
        Ok(Some(_)) => {
            return format!(
                "the entry at {} changed again and was retained",
                destination.display()
            );
        }
        Ok(None) => {
            return format!(
                "the unexpected entry disappeared from {} before it could be restored",
                destination.display()
            );
        }
        Err(error) => {
            return format!(
                "the unexpected entry is retained at {} because it could not be inspected: {error}",
                destination.display()
            );
        }
    }
    if let Err(error) = move_no_replace(destination, source) {
        return format!(
            "the unexpected entry is retained at {} because it could not be restored to {} without replacement: {error}",
            destination.display(),
            source.display()
        );
    }
    let sync_error = sync_rename_parents(destination, source).err();
    let restored = observe_entry_identity(source).ok() == Some(Some(unexpected_identity));
    let destination_cleared = observe_entry_identity(destination).ok() == Some(None);
    if restored && destination_cleared {
        return match sync_error {
            Some(error) => format!(
                "the unexpected entry was restored to {} without replacement, but the restoration could not be synchronized: {error}",
                source.display()
            ),
            None => format!(
                "the unexpected entry was restored to {} without replacement",
                source.display()
            ),
        };
    }
    format!(
        "the unexpected entry could not be verified after restoration; inspect {} and {}",
        source.display(),
        destination.display()
    )
}

fn remove_verified_bound_file(
    transaction: &ResolvedTransaction,
    path: &Path,
    verified: VerifiedBoundFile,
    role: &str,
) -> Result<(), FormatError> {
    let VerifiedBoundFile {
        file: verified_file,
        identity,
        state,
    } = verified;
    // The Windows cleanup handle needs attribute-write access for readonly
    // files, which conflicts with the retained read proof. Rebind immediately
    // and validate the proof's exact post-digest state before removal.
    drop(verified_file);
    let file = open_regular_file_no_follow_for_cleanup(path).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not bind {role} before cleanup: {error}"),
        )
    })?;
    let metadata = file.metadata()?;
    if file_identity(&file)? != identity
        || path_identity(path)? != identity
        || !state.matches(&metadata)
    {
        return Err(transaction_error(
            transaction,
            format!("{role} changed before cleanup at {}", path.display()),
        ));
    }
    remove_open_file(path, &file, metadata.permissions()).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not remove {role} at {}: {error}", path.display()),
        )
    })?;
    sync_directory(parent_or_current(path)).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not synchronize {role} cleanup: {error}"),
        )
    })?;
    Ok(())
}

#[cfg(windows)]
// Windows maps this flag to FILE_ATTRIBUTE_READONLY. Unix uses the separate
// implementation below, so clearing it cannot broaden Unix mode bits.
#[allow(clippy::permissions_set_readonly_false)]
fn remove_open_file(path: &Path, file: &File, original: Permissions) -> io::Result<()> {
    if !original.readonly() {
        return fs::remove_file(path);
    }
    let mut writable = original.clone();
    writable.set_readonly(false);
    file.set_permissions(writable)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = file.set_permissions(original);
            Err(error)
        }
    }
}

#[cfg(not(windows))]
fn remove_open_file(path: &Path, _file: &File, _original: Permissions) -> io::Result<()> {
    fs::remove_file(path)
}

fn remove_empty_holder(transaction: &ResolvedTransaction) -> Result<(), FormatError> {
    if !validate_holder(transaction)? {
        return Ok(());
    }
    if fs::read_dir(&transaction.holder)?.next().is_some() {
        return Err(transaction_error(
            transaction,
            "the update transaction holder is not empty after cleanup".into(),
        ));
    }
    if path_identity(&transaction.holder)? != transaction.holder_identity {
        return Err(transaction_error(
            transaction,
            "the update transaction holder changed before removal".into(),
        ));
    }
    fs::remove_dir(&transaction.holder).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not remove the empty update transaction holder: {error}"),
        )
    })?;
    sync_directory(parent_or_current(&transaction.holder)).map_err(|error| {
        transaction_error(
            transaction,
            format!("could not synchronize update holder cleanup: {error}"),
        )
    })?;
    Ok(())
}

fn cleanup_empty_holder(original: FormatError, transaction: &ResolvedTransaction) -> FormatError {
    match remove_empty_holder(transaction) {
        Ok(()) => original,
        Err(cleanup) => FormatError::Io(io::Error::other(format!(
            "{original}; unused update holder cleanup also failed: {cleanup}"
        ))),
    }
}

fn reject_unjournaled_artifacts(target: &Path, key: &str) -> Result<(), FormatError> {
    for entry in fs::read_dir(parent_or_current(target))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !holder_name_is_reserved(&name, key)
            && !staging_name_is_reserved(&name, key)
            && !journal_temp_name_is_reserved(&name, key)
        {
            continue;
        }
        return Err(FormatError::Io(io::Error::other(format!(
            "reserved update work path has no durable transaction record and was left untouched; inspect {}",
            entry.path().display()
        ))));
    }
    Ok(())
}

fn validate_open_record(open: &OpenRecord) -> Result<(), FormatError> {
    if file_identity(&open.file)? != open.identity
        || path_identity(&open.path)? != open.identity
        || !open.state.matches(&open.file.metadata()?)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "update transaction record changed at {}",
            open.path.display()
        ))));
    }
    Ok(())
}

fn verify_open_bytes(file: &mut File, expected: &[u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut actual = Vec::new();
    Read::by_ref(file)
        .take((JOURNAL_MAX_BYTES + 1) as u64)
        .read_to_end(&mut actual)?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal contents differ from the published record",
        ));
    }
    Ok(())
}

fn open_new_artifact(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        };

        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn acquire_lock(path: &Path, purpose: &str, ctl: &ControlToken) -> Result<File, FormatError> {
    let file = open_lock_file(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || path_identity(path)? != file_identity(&file)? {
        return Err(FormatError::Io(io::Error::other(format!(
            "update {purpose} lock is not a stable regular file: {}",
            path.display()
        ))));
    }
    loop {
        ctl.checkpoint()?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => break,
            Err(fs4::TryLockError::WouldBlock) => std::thread::sleep(LOCK_POLL_INTERVAL),
            Err(fs4::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || path_identity(path)? != file_identity(&file)?
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "update {purpose} lock changed or became unsafe: {}",
            path.display()
        ))));
    }
    Ok(file)
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};

    let file = open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?;
    Ok(File::from(file))
}

#[cfg(windows)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn transaction_error(transaction: &ResolvedTransaction, reason: String) -> FormatError {
    FormatError::Io(io::Error::other(format!(
        "archive update requires recovery: {reason}; no existing destination was replaced. Unexpected entries are restored without replacement when possible and otherwise retained at the reported paths. Inspect target {}, pending record {}, journal {}, completion {}, staging {}, and holder {}",
        transaction.target.display(),
        transaction.pending.display(),
        transaction.journal.display(),
        transaction.completion.display(),
        transaction.staging.display(),
        transaction.holder.display()
    )))
}

fn describe_snapshot(snapshot: &TransactionSnapshot) -> String {
    format!(
        "staging={:?}, target={:?}, previous={:?}, replacement={:?}, retired={:?}, holder={}",
        snapshot.staging.as_ref().map(|file| file.identity),
        snapshot.target.as_ref().map(|file| file.identity),
        snapshot.previous.as_ref().map(|file| file.identity),
        snapshot.replacement.as_ref().map(|file| file.identity),
        snapshot.retired.as_ref().map(|file| file.identity),
        snapshot.holder_present
    )
}

fn journal_path(target: &Path, key: &str) -> PathBuf {
    parent_or_current(target).join(format!(".squallz-update-{key}.json"))
}

fn pending_path(target: &Path, key: &str) -> PathBuf {
    parent_or_current(target).join(format!(".squallz-update-{key}.pending.json"))
}

fn completion_path(target: &Path, key: &str) -> PathBuf {
    parent_or_current(target).join(format!(".squallz-update-{key}.completed.json"))
}

fn staging_name_is_reserved(name: &str, key: &str) -> bool {
    name.strip_prefix(&format!(".squallz-update-stage-{}-", &key[..16]))
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(valid_pid_sequence)
}

fn holder_name_is_reserved(name: &str, key: &str) -> bool {
    name.strip_prefix(&format!(".squallz-update-holder-{}-", &key[..16]))
        .is_some_and(valid_pid_sequence)
}

fn journal_temp_name_is_reserved(name: &str, key: &str) -> bool {
    name.strip_prefix(&format!(".squallz-update-journal-{}-", &key[..16]))
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(valid_pid_sequence)
}

fn valid_pid_sequence(value: &str) -> bool {
    let Some((pid, sequence)) = value.split_once('-') else {
        return false;
    };
    !sequence.contains('-')
        && canonical_positive_integer(pid)
        && canonical_positive_integer(sequence)
        && pid.parse::<u32>().is_ok()
        && sequence.parse::<u64>().is_ok()
}

fn canonical_positive_integer(value: &str) -> bool {
    let Some((&first, rest)) = value.as_bytes().split_first() else {
        return false;
    };
    (b'1'..=b'9').contains(&first) && rest.iter().all(u8::is_ascii_digit)
}

fn component_string(path: &Path, role: &str) -> Result<String, FormatError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| FormatError::Unsupported(format!("{role} has an invalid file name")))
}

fn checked_component(name: &str, role: &str) -> Result<std::ffi::OsString, FormatError> {
    let path = Path::new(name);
    if path.file_name() != Some(path.as_os_str())
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err(FormatError::Unsupported(format!(
            "{role} is not a single file name"
        )));
    }
    Ok(path.as_os_str().to_owned())
}

fn path_exists(path: &Path) -> Result<bool, FormatError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn sync_rename_parents(source: &Path, destination: &Path) -> io::Result<()> {
    let source_parent = parent_or_current(source);
    let destination_parent = parent_or_current(destination);
    sync_directory(destination_parent)?;
    if source_parent != destination_parent {
        sync_directory(source_parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    crate::open_directory(path)?.sync_all()
}

fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn path_key(domain: &[u8], path: &Path) -> Result<String, FormatError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        hasher.update(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        for unit in path.as_os_str().encode_wide() {
            hasher.update(&unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let path = path.to_str().ok_or_else(|| {
            FormatError::Unsupported("archive update paths must be UTF-8 on this platform".into())
        })?;
        hasher.update(path.as_bytes());
    }
    Ok(hasher.finalize().to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "squallz-update-transaction-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn journaled_transaction(label: &str) -> (PathBuf, String, ResolvedTransaction) {
        journaled_transaction_with_guard(label, false)
    }

    fn journaled_transaction_with_guard(
        label: &str,
        guarded: bool,
    ) -> (PathBuf, String, ResolvedTransaction) {
        let dir = temp_dir(label);
        let requested = dir.join("archive.zip");
        fs::write(&requested, b"old archive").unwrap();
        let source_path_state_digest = if guarded {
            let state =
                crate::inspect_create_destination(&requested, CreateArtifactKind::Archive).unwrap();
            let guard = state.guard.unwrap();
            Some(verify_destination_guard(&requested, CreateArtifactKind::Archive, guard).unwrap())
        } else {
            None
        };
        let target = canonical_update_target(&requested).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let source = bind_source(&target).unwrap();
        let mut stage = reserve_stage(&target, &key).unwrap();
        stage.file.write_all(b"new archive").unwrap();
        stage.file.sync_all().unwrap();
        let stage = prepare_stage(stage).map_err(|(error, _)| error).unwrap();
        let mut digests = bind_transaction_digests(
            &target,
            &source,
            &stage,
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        digests.source_path_state = source_path_state_digest;
        let (holder, holder_identity) = reserve_holder(&target, &key).unwrap();
        let transaction = transaction_for(
            &target,
            &key,
            &source,
            &stage,
            digests,
            holder,
            holder_identity,
        );
        let record = record_for(&transaction, &key).unwrap();
        assert_eq!(
            record.version,
            if guarded {
                TRANSACTION_VERSION
            } else {
                TRANSACTION_VERSION_V1
            }
        );
        let journal = write_journal(&transaction, record)
            .map_err(|error| match error {
                JournalPublishError::BeforePublish(error)
                | JournalPublishError::Published(error) => error,
            })
            .unwrap();
        drop(journal);
        drop(stage);
        drop(source);
        (target, key, transaction)
    }

    fn recover_for_test(target: &Path, key: &str) -> Result<(), FormatError> {
        recover_existing(
            target,
            key,
            ProgressPhase::UpdateRecovery,
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
    }

    struct RewriteInstalledWhenRetiredAppears {
        target: PathBuf,
        retired: PathBuf,
        rewritten: AtomicBool,
    }

    impl ProgressSink for RewriteInstalledWhenRetiredAppears {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
            if self.retired.exists() && !self.rewritten.swap(true, Ordering::SeqCst) {
                overwrite_same_length_preserving_mtime(&self.target, b"bad archive");
            }
        }
    }

    #[derive(Default)]
    struct DigestPassCounter {
        passes: AtomicUsize,
    }

    impl ProgressSink for DigestPassCounter {
        fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
            if done == 0 && total > 0 {
                self.passes.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    #[cfg(unix)]
    struct RewriteRetiredOnSecondDigest {
        retired: PathBuf,
        passes: AtomicUsize,
    }

    #[derive(Default)]
    struct PhaseRecorder {
        phases: Mutex<Vec<(ProgressPhase, bool)>>,
    }

    impl ProgressSink for PhaseRecorder {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

        fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
            self.phases.lock().unwrap().push((phase, interruptible));
        }
    }

    #[cfg(unix)]
    impl ProgressSink for RewriteRetiredOnSecondDigest {
        fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
            if done == 0 && total > 0 && self.passes.fetch_add(1, Ordering::SeqCst) == 1 {
                overwrite_same_length_preserving_mtime(&self.retired, b"bad archive");
            }
        }
    }

    fn assert_transaction_clean(transaction: &ResolvedTransaction) {
        for path in [
            &transaction.staging,
            &transaction.holder,
            &transaction.pending,
            &transaction.journal,
            &transaction.completion,
        ] {
            assert!(
                !path.exists(),
                "transaction path remains: {}",
                path.display()
            );
        }
    }

    #[test]
    fn same_process_cleanup_reuses_installed_digest_verification() {
        let (target, _key, transaction) = journaled_transaction("reuse-installed-verification");
        let progress = DigestPassCounter::default();
        let ctl = ControlToken::default();
        let installed_verification =
            resume_transaction(&transaction, None, &progress, &ctl).unwrap();
        let journal = open_record(
            &transaction.journal,
            &transaction.target,
            "active update record",
        )
        .unwrap();
        let completion = publish_completion(journal, &transaction).unwrap();
        progress.passes.store(0, Ordering::SeqCst);

        cleanup_completed(
            completion,
            &transaction,
            Some(installed_verification),
            &progress,
            &ctl,
        )
        .unwrap();

        assert_eq!(progress.passes.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_transaction_clean(&transaction);
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn changed_installed_file_invalidates_same_process_digest_verification() {
        let (target, _key, transaction) =
            journaled_transaction("invalidate-installed-verification");
        let ctl = ControlToken::default();
        let installed_verification =
            resume_transaction(&transaction, None, &crate::api::NoProgress, &ctl).unwrap();
        let journal = open_record(
            &transaction.journal,
            &transaction.target,
            "active update record",
        )
        .unwrap();
        let completion = publish_completion(journal, &transaction).unwrap();
        let progress = RewriteInstalledWhenRetiredAppears {
            target: target.clone(),
            retired: transaction.retired.clone(),
            rewritten: AtomicBool::new(false),
        };

        let error = cleanup_completed(
            completion,
            &transaction,
            Some(installed_verification),
            &progress,
            &ctl,
        )
        .unwrap_err();

        assert!(progress.rewritten.load(Ordering::SeqCst));
        assert!(error.to_string().contains(
            "installed archive content changed before the transaction record was cleared"
        ));
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.retired).unwrap(), b"old archive");
        assert!(!transaction.previous.exists());
        assert!(transaction.completion.exists());
        assert!(transaction.holder.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_retired_file_changed_during_installed_verification() {
        let (target, _key, transaction) =
            journaled_transaction("retired-changed-during-installed-verification");
        let ctl = ControlToken::default();
        let installed_verification =
            resume_transaction(&transaction, None, &crate::api::NoProgress, &ctl).unwrap();
        drop(installed_verification);
        let journal = open_record(
            &transaction.journal,
            &transaction.target,
            "active update record",
        )
        .unwrap();
        let completion = publish_completion(journal, &transaction).unwrap();
        let progress = RewriteRetiredOnSecondDigest {
            retired: transaction.retired.clone(),
            passes: AtomicUsize::new(0),
        };

        let error = cleanup_completed(completion, &transaction, None, &progress, &ctl).unwrap_err();

        assert!(error
            .to_string()
            .contains("retired archive content changed before removal"));
        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_eq!(fs::read(&transaction.retired).unwrap(), b"bad archive");
        assert!(transaction.completion.exists());
        assert!(transaction.holder.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn recovery_finishes_each_committed_move_boundary() {
        for step in 0..4 {
            let (target, key, transaction) =
                journaled_transaction(&format!("move-boundary-{step}"));
            if step >= 1 {
                crate::move_path_no_replace(&transaction.staging, &transaction.replacement)
                    .unwrap();
            }
            if step >= 2 {
                crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
            }
            if step >= 3 {
                crate::move_path_no_replace(&transaction.replacement, &transaction.target).unwrap();
            }

            recover_for_test(&target, &key).unwrap();

            assert_eq!(fs::read(&target).unwrap(), b"new archive");
            assert_transaction_clean(&transaction);
            fs::remove_dir_all(parent_or_current(&target)).unwrap();
        }
    }

    #[test]
    fn guarded_recovery_rechecks_the_moved_destination_before_install() {
        let (target, key, transaction) =
            journaled_transaction_with_guard("guarded-moved-source", true);
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        overwrite_same_length_preserving_mtime(&transaction.previous, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(!error.is_destination_changed());
        assert!(error
            .to_string()
            .contains("the authorized previous archive state changed"));
        assert!(!target.exists());
        assert_eq!(fs::read(&transaction.previous).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn guarded_cleanup_preserves_changed_previous_and_retired_archives() {
        for retire_before_change in [false, true] {
            let label = if retire_before_change {
                "guarded-retired-cleanup"
            } else {
                "guarded-previous-cleanup"
            };
            let (target, _key, transaction) = journaled_transaction_with_guard(label, true);
            let ctl = ControlToken::default();
            let installed =
                resume_transaction(&transaction, None, &crate::api::NoProgress, &ctl).unwrap();
            let journal = open_record(
                &transaction.journal,
                &transaction.target,
                "active update record",
            )
            .unwrap();
            let completion = publish_completion(journal, &transaction).unwrap();
            let changed = if retire_before_change {
                crate::move_path_no_replace(&transaction.previous, &transaction.retired).unwrap();
                &transaction.retired
            } else {
                &transaction.previous
            };
            overwrite_same_length_preserving_mtime(changed, b"bad archive");

            let error = cleanup_completed(
                completion,
                &transaction,
                Some(installed),
                &crate::api::NoProgress,
                &ctl,
            )
            .unwrap_err();

            assert!(!error.is_destination_changed());
            assert!(error
                .to_string()
                .contains("the authorized previous archive state changed"));
            assert_eq!(fs::read(&target).unwrap(), b"new archive");
            assert_eq!(fs::read(changed).unwrap(), b"bad archive");
            assert!(transaction.completion.exists());
            assert!(transaction.holder.exists());
            fs::remove_dir_all(parent_or_current(&target)).unwrap();
        }
    }

    #[test]
    fn recovery_reports_one_uninterruptible_phase() {
        let (target, key, transaction) = journaled_transaction("recovery-progress-phase");
        let progress = PhaseRecorder::default();

        recover_existing(
            &target,
            &key,
            ProgressPhase::UpdateRecovery,
            &progress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(
            *progress.phases.lock().unwrap(),
            vec![(ProgressPhase::UpdateRecovery, false)]
        );
        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_transaction_clean(&transaction);
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn recovery_never_replaces_a_late_competitor() {
        let (target, key, transaction) = journaled_transaction("late-competitor");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        fs::write(&target, b"competitor").unwrap();

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("no existing destination was replaced"));
        assert_eq!(fs::read(&target).unwrap(), b"competitor");
        assert_eq!(fs::read(&transaction.previous).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        assert!(transaction.journal.exists());

        fs::remove_file(&target).unwrap();
        recover_for_test(&target, &key).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_transaction_clean(&transaction);
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn recovery_preserves_a_competing_directory_and_its_contents() {
        let (target, key, transaction) = journaled_transaction("directory-competitor");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("marker"), b"keep").unwrap();

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
        assert_eq!(fs::read(target.join("marker")).unwrap(), b"keep");
        assert_eq!(fs::read(&transaction.previous).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");

        fs::remove_dir_all(&target).unwrap();
        recover_for_test(&target, &key).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_transaction_clean(&transaction);
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn waiting_for_an_update_lock_can_be_cancelled() {
        let dir = temp_dir("cancel-lock");
        let path = dir.join("update.lock");
        let first = acquire_lock(&path, "test", &ControlToken::default()).unwrap();
        let control = ControlToken::new();
        let waiter_control = control.clone();
        let waiter_path = path.clone();
        let waiter = std::thread::spawn(move || {
            acquire_lock(&waiter_path, "test", &waiter_control).map(drop)
        });
        std::thread::sleep(Duration::from_millis(100));
        control.cancel();

        let result = waiter.join().unwrap();

        assert!(matches!(result, Err(FormatError::Cancelled)));
        drop(first);
        fs::remove_file(path).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staging_name_does_not_repeat_a_long_archive_name() {
        let dir = temp_dir("long-name");
        let archive = dir.join(format!("{}.zip", "a".repeat(240)));
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();

        let stage = reserve_stage(&target, &key).unwrap();

        assert!(stage.path.file_name().unwrap().len() < 100);
        let error = cleanup_stage(FormatError::Cancelled, stage);
        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(fs::read(&target).unwrap(), b"archive");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staging_cleanup_preserves_a_rebound_path() {
        let dir = temp_dir("rebound-stage");
        let archive = dir.join("archive.zip");
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let stage = reserve_stage(&target, &key).unwrap();
        let rebound = stage.path.clone();
        let held = dir.join("held-stage");
        crate::move_path_no_replace(&rebound, &held).unwrap();
        fs::write(&rebound, b"competitor").unwrap();

        let error = cleanup_stage(FormatError::Cancelled, stage);

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(fs::read(&held).unwrap(), b"");
        assert_eq!(fs::read(&rebound).unwrap(), b"competitor");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn created_stage_binding_preserves_an_unverifiable_directory() {
        let dir = temp_dir("created-stage-directory");
        let stage = dir.join(".create-test.tmp");
        let displaced = dir.join("displaced-stage");
        fs::write(&stage, b"writer output").unwrap();
        let file = open_regular_file_no_follow(&stage).unwrap();
        let stage_identity = file_identity(&file).unwrap();
        crate::move_path_no_replace(&stage, &displaced).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("competitor"), b"keep").unwrap();

        let error = match bind_created_stage(&stage, file, stage_identity) {
            Ok(bound) => {
                let cleanup = cleanup_stage(FormatError::Cancelled, bound);
                panic!("directory was unexpectedly bound: {cleanup}");
            }
            Err(error) => error,
        };

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(fs::read(stage.join("competitor")).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn created_stage_binding_rejects_a_regular_file_rebound_after_writing() {
        let dir = temp_dir("created-stage-rebound");
        let stage = dir.join(".create-test.tmp");
        let displaced = dir.join("displaced-stage");
        fs::write(&stage, b"writer output").unwrap();
        let file = open_regular_file_no_follow(&stage).unwrap();
        let writer_identity = file_identity(&file).unwrap();
        crate::move_path_no_replace(&stage, &displaced).unwrap();
        fs::write(&stage, b"competitor").unwrap();

        let error = match bind_created_stage(&stage, file, writer_identity) {
            Ok(bound) => {
                let cleanup = cleanup_stage(FormatError::Cancelled, bound);
                panic!("rebound stage was unexpectedly bound: {cleanup}");
            }
            Err(error) => error,
        };

        let message = error.to_string();
        assert!(message.contains("changed"), "{message}");
        assert!(message.contains("left untouched"), "{message}");
        assert_eq!(fs::read(&stage).unwrap(), b"competitor");
        assert_eq!(fs::read(&displaced).unwrap(), b"writer output");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staging_cleanup_removes_a_readonly_reserved_file() {
        let dir = temp_dir("readonly-stage-cleanup");
        let archive = dir.join("archive.zip");
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let stage = reserve_stage(&target, &key).unwrap();
        let path = stage.path.clone();
        let mut permissions = stage.file.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        stage.file.set_permissions(permissions).unwrap();

        let error = cleanup_stage(FormatError::Cancelled, stage);

        assert!(matches!(error, FormatError::Cancelled));
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verified_cleanup_removes_a_readonly_retired_archive() {
        let (target, _key, transaction) = journaled_transaction("readonly-retired-cleanup");
        crate::move_path_no_replace(&target, &transaction.previous).unwrap();
        crate::move_path_no_replace(&transaction.previous, &transaction.retired).unwrap();
        let mut permissions = fs::metadata(&transaction.retired).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&transaction.retired, permissions).unwrap();

        let verified = verify_transaction_digest(
            &transaction,
            &transaction.retired,
            transaction.source_identity,
            &transaction.source_digest,
            "retired archive",
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap()
        .unwrap();

        remove_verified_bound_file(
            &transaction,
            &transaction.retired,
            verified,
            "retired archive",
        )
        .unwrap();

        assert!(!transaction.retired.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn durable_pending_record_resumes_and_cleans_the_transaction() {
        let (target, key, transaction) = journaled_transaction("pending-recovery");
        crate::move_path_no_replace(&transaction.journal, &transaction.pending).unwrap();
        sync_directory(parent_or_current(&target)).unwrap();

        recover_for_test(&target, &key).unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new archive");
        assert_transaction_clean(&transaction);
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn malformed_pending_record_preserves_every_transaction_path() {
        let (target, key, transaction) = journaled_transaction("malformed-pending");
        fs::remove_file(&transaction.journal).unwrap();
        fs::write(&transaction.pending, b"{incomplete").unwrap();
        sync_directory(parent_or_current(&target)).unwrap();

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("pending update transaction is invalid"));
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.staging).unwrap(), b"new archive");
        assert_eq!(fs::read(&transaction.pending).unwrap(), b"{incomplete");
        assert!(transaction.holder.is_dir());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn pending_publication_sync_failure_is_classified_as_published() {
        let (target, _key, transaction) = journaled_transaction("pending-sync-failure");
        let temp = parent_or_current(&target).join("record-before-pending.tmp");
        crate::move_path_no_replace(&transaction.journal, &temp).unwrap();
        let open = open_record(&temp, &target, "test update record").unwrap();

        let error = match move_open_record_with(
            open,
            &transaction.pending,
            &transaction,
            "pending update record",
            &mut |from, to| crate::move_path_no_replace(from, to),
            &mut |_, _| Err(io::Error::other("simulated directory sync failure")),
        ) {
            Ok(_) => panic!("pending publication unexpectedly succeeded"),
            Err(error) => error,
        };

        let OpenRecordMoveError::Published(error) = error else {
            panic!("a moved pending record must be classified as published");
        };
        assert!(error.to_string().contains("could not be synchronized"));
        assert!(transaction.pending.is_file());
        assert!(!temp.exists());
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.staging).unwrap(), b"new archive");
        assert!(transaction.holder.is_dir());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn unjournaled_empty_holder_is_preserved_for_inspection() {
        let dir = temp_dir("empty-orphan-holder");
        let archive = dir.join("archive.zip");
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let (holder, _) = reserve_holder(&target, &key).unwrap();

        let error = reject_unjournaled_artifacts(&target, &key).unwrap_err();

        assert!(error.to_string().contains("no durable transaction record"));
        assert!(holder.is_dir());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonempty_orphan_holder_is_preserved_for_inspection() {
        let dir = temp_dir("nonempty-orphan-holder");
        let archive = dir.join("archive.zip");
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let (holder, _) = reserve_holder(&target, &key).unwrap();
        fs::write(holder.join("marker"), b"keep").unwrap();

        let error = reject_unjournaled_artifacts(&target, &key).unwrap_err();

        assert!(error.to_string().contains("no durable transaction record"));
        assert_eq!(fs::read(holder.join("marker")).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unjournaled_staging_path_is_preserved_for_inspection() {
        let dir = temp_dir("orphan-stage");
        let archive = dir.join("archive.zip");
        fs::write(&archive, b"archive").unwrap();
        let target = canonical_update_target(&archive).unwrap();
        let key = path_key(b"squallz-update-target-v1\0", &target).unwrap();
        let mut stage = reserve_stage(&target, &key).unwrap();
        stage.file.write_all(b"partial update").unwrap();
        stage.file.sync_all().unwrap();

        let error = reject_unjournaled_artifacts(&target, &key).unwrap_err();

        assert!(error.to_string().contains("no durable transaction record"));
        assert_eq!(fs::read(&stage.path).unwrap(), b"partial update");
        drop(stage);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn staging_rebind_during_move_is_restored_without_replacement() {
        let (target, _key, transaction) = journaled_transaction("stage-move-rebind");
        let parent = parent_or_current(&target);
        let held_expected = parent.join("held-expected-stage");
        let competitor = parent.join("stage-competitor");
        fs::write(&competitor, b"stage competitor").unwrap();
        let mut calls = 0usize;

        let error = move_bound_with(
            &transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            &crate::api::NoProgress,
            &ControlToken::default(),
            &mut |from, to| {
                calls += 1;
                if calls == 1 {
                    crate::move_path_no_replace(from, &held_expected)?;
                    crate::move_path_no_replace(&competitor, from)?;
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("restored"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&transaction.staging).unwrap(), b"stage competitor");
        assert!(!transaction.replacement.exists());
        assert_eq!(fs::read(&held_expected).unwrap(), b"new archive");
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn target_rebind_during_move_is_restored_without_replacement() {
        let (target, _key, transaction) = journaled_transaction("target-move-rebind");
        move_bound(
            &transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        let parent = parent_or_current(&target);
        let held_expected = parent.join("held-expected-target");
        let competitor = parent.join("target-competitor");
        fs::write(&competitor, b"target competitor").unwrap();
        let mut calls = 0usize;

        let error = move_bound_with(
            &transaction,
            &transaction.target,
            &transaction.previous,
            transaction.source_identity,
            &transaction.source_state,
            &transaction.source_digest,
            StateMatch::Exact,
            "previous archive",
            &crate::api::NoProgress,
            &ControlToken::default(),
            &mut |from, to| {
                calls += 1;
                if calls == 1 {
                    crate::move_path_no_replace(from, &held_expected)?;
                    crate::move_path_no_replace(&competitor, from)?;
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("restored"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&target).unwrap(), b"target competitor");
        assert!(!transaction.previous.exists());
        assert_eq!(fs::read(&held_expected).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn failed_rebind_restoration_retains_both_competing_entries() {
        let (target, _key, transaction) = journaled_transaction("target-move-retained");
        move_bound(
            &transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        let parent = parent_or_current(&target);
        let held_expected = parent.join("held-old-target");
        let competitor = parent.join("target-race-entry");
        fs::write(&competitor, b"moved competitor").unwrap();
        let mut calls = 0usize;

        let error = move_bound_with(
            &transaction,
            &transaction.target,
            &transaction.previous,
            transaction.source_identity,
            &transaction.source_state,
            &transaction.source_digest,
            StateMatch::Exact,
            "previous archive",
            &crate::api::NoProgress,
            &ControlToken::default(),
            &mut |from, to| {
                calls += 1;
                if calls == 1 {
                    crate::move_path_no_replace(from, &held_expected)?;
                    crate::move_path_no_replace(&competitor, from)?;
                } else if calls == 2 {
                    fs::write(to, b"late source entry")?;
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("retained"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&target).unwrap(), b"late source entry");
        assert_eq!(
            fs::read(&transaction.previous).unwrap(),
            b"moved competitor"
        );
        assert_eq!(fs::read(&held_expected).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        fs::remove_dir_all(parent).unwrap();
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
                if after != before {
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
    fn staging_content_rewrite_during_move_is_restored() {
        let (target, _key, transaction) = journaled_transaction("stage-content-move-race");
        let mut calls = 0usize;

        let error = move_bound_with(
            &transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            &crate::api::NoProgress,
            &ControlToken::default(),
            &mut |from, to| {
                calls += 1;
                if calls == 1 {
                    overwrite_same_length_preserving_mtime(from, b"bad archive");
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("restored"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&transaction.staging).unwrap(), b"bad archive");
        assert!(!transaction.replacement.exists());
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn target_content_rewrite_during_move_is_restored() {
        let (target, _key, transaction) = journaled_transaction("target-content-move-race");
        move_bound(
            &transaction,
            &transaction.staging,
            &transaction.replacement,
            transaction.staging_identity,
            &transaction.staging_state,
            &transaction.staging_digest,
            StateMatch::Exact,
            "staged replacement",
            &crate::api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        let mut calls = 0usize;

        let error = move_bound_with(
            &transaction,
            &transaction.target,
            &transaction.previous,
            transaction.source_identity,
            &transaction.source_state,
            &transaction.source_digest,
            StateMatch::Exact,
            "previous archive",
            &crate::api::NoProgress,
            &ControlToken::default(),
            &mut |from, to| {
                calls += 1;
                if calls == 1 {
                    overwrite_same_length_preserving_mtime(from, b"bad archive");
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("restored"));
        assert_eq!(calls, 2);
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert!(!transaction.previous.exists());
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn recovery_rejects_same_length_rewrite_after_staging_move() {
        let (target, key, transaction) = journaled_transaction("changed-moved-stage-content");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        overwrite_same_length_preserving_mtime(&transaction.replacement, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("staged replacement content changed"));
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"bad archive");
        assert!(!transaction.previous.exists());
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn recovery_rejects_same_length_rewrite_after_install() {
        let (target, key, transaction) = journaled_transaction("changed-installed-content");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        crate::move_path_no_replace(&transaction.replacement, &transaction.target).unwrap();
        overwrite_same_length_preserving_mtime(&target, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("installed archive content does not match"));
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.previous).unwrap(), b"old archive");
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn completed_recovery_preserves_old_archive_when_installed_content_changed() {
        let (target, key, transaction) =
            journaled_transaction("changed-completed-installed-content");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        crate::move_path_no_replace(&transaction.replacement, &transaction.target).unwrap();
        let journal = open_record(
            &transaction.journal,
            &transaction.target,
            "active update record",
        )
        .unwrap();
        let completion = publish_completion(journal, &transaction).unwrap();
        drop(completion);
        overwrite_same_length_preserving_mtime(&target, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error.to_string().contains(
            "installed archive content changed before the transaction record was cleared"
        ));
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.retired).unwrap(), b"old archive");
        assert!(!transaction.previous.exists());
        assert!(transaction.completion.exists());
        assert!(transaction.holder.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[test]
    fn completed_recovery_keeps_retired_archive_when_installed_content_changes_during_cleanup() {
        let (target, key, transaction) =
            journaled_transaction("changed-installed-content-during-cleanup");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        crate::move_path_no_replace(&transaction.target, &transaction.previous).unwrap();
        crate::move_path_no_replace(&transaction.replacement, &transaction.target).unwrap();
        let journal = open_record(
            &transaction.journal,
            &transaction.target,
            "active update record",
        )
        .unwrap();
        let completion = publish_completion(journal, &transaction).unwrap();
        drop(completion);
        let progress = RewriteInstalledWhenRetiredAppears {
            target: target.clone(),
            retired: transaction.retired.clone(),
            rewritten: AtomicBool::new(false),
        };

        let error = recover_existing(
            &target,
            &key,
            ProgressPhase::UpdateRecovery,
            &progress,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(progress.rewritten.load(Ordering::SeqCst));
        assert!(error.to_string().contains(
            "installed archive content changed before the transaction record was cleared"
        ));
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.retired).unwrap(), b"old archive");
        assert!(!transaction.previous.exists());
        assert!(transaction.completion.exists());
        assert!(transaction.holder.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_same_length_staging_rewrite_before_first_move() {
        let (target, key, transaction) = journaled_transaction("changed-stage-state");
        overwrite_same_length_preserving_mtime(&transaction.staging, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("do not match a recoverable state"));
        assert_eq!(fs::read(&target).unwrap(), b"old archive");
        assert_eq!(fs::read(&transaction.staging).unwrap(), b"bad archive");
        assert!(!transaction.replacement.exists());
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_same_length_target_rewrite_before_second_move() {
        let (target, key, transaction) = journaled_transaction("changed-target-state");
        crate::move_path_no_replace(&transaction.staging, &transaction.replacement).unwrap();
        overwrite_same_length_preserving_mtime(&target, b"bad archive");

        let error = recover_for_test(&target, &key).unwrap_err();

        assert!(error
            .to_string()
            .contains("do not match a recoverable state"));
        assert_eq!(fs::read(&target).unwrap(), b"bad archive");
        assert_eq!(fs::read(&transaction.replacement).unwrap(), b"new archive");
        assert!(!transaction.previous.exists());
        assert!(transaction.journal.exists());
        fs::remove_dir_all(parent_or_current(&target)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_targets_are_rejected_without_following_them() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("target-symlink");
        let archive = dir.join("real.zip");
        let alias = dir.join("alias.zip");
        fs::write(&archive, b"archive").unwrap();
        symlink(&archive, &alias).unwrap();

        let error = canonical_update_target(&alias).unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert!(fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&archive).unwrap(), b"archive");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_lock_paths_are_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("lock-symlink");
        let victim = dir.join("victim");
        let lock = dir.join("update.lock");
        fs::write(&victim, b"unchanged").unwrap();
        symlink(&victim, &lock).unwrap();

        let error = acquire_lock(&lock, "test", &ControlToken::default()).unwrap_err();

        assert!(matches!(error, FormatError::Io(_)));
        assert_eq!(fs::read(&victim).unwrap(), b"unchanged");
        assert!(fs::symlink_metadata(&lock)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(dir).unwrap();
    }
}
