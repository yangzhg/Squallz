//! Concrete SFX producer reservations, disposal and public recovery reporting.
//! Replacement issuance, installation and acknowledgment use the shared owner.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::{
    output_exists_error, validate_publish_destination, SfxBuildReport, SfxLayout, StagedSfx,
};
use crate::api::{ControlToken, EntryPath, FormatError, NoProgress, ProgressPhase, ProgressSink};
use crate::archive_path::{checked_path_component, is_canonical_process_sequence};
use crate::destination_guard::{path_state_digest, verify_destination_guard};
use crate::filesystem_identity::open_regular_file_no_follow;
use crate::filesystem_identity::{file_identity, path_identity, PathIdentity};
use crate::replacement::{
    self, ActiveWriter, ArtifactRole, Authorization, Failure, Outcome, OwnedInput, PersistFailure,
    RecordPhase, Recovery, Scope, VerificationContext, Visibility,
};
use crate::stored_os_string::StoredOsString;
use crate::{parent_or_current, sync_directory, CreateArtifactKind, CreateCommitPolicy};

const CLEANUP_VERSION: u32 = 1;
const CLEANUP_MAX_BYTES: usize = 64 * 1024;
const CLEANUP_JOURNAL_NAME: &str = ".squallz-sfx-cleanup.json";
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfxRecoveryDetails {
    /// Exact self-extractor destination guarded by the transaction.
    pub target: PathBuf,
    /// Current transaction-owned paths that must be retained and inspected.
    pub paths: Vec<PathBuf>,
}

#[derive(Debug)]
struct SfxRecoveryIoError {
    message: String,
    details: SfxRecoveryDetails,
    retain_staging: bool,
}

impl fmt::Display for SfxRecoveryIoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SfxRecoveryIoError {}

pub fn sfx_recovery_details(error: &FormatError) -> Option<SfxRecoveryDetails> {
    let FormatError::Io(error) = error else {
        return None;
    };
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<SfxRecoveryIoError>())
        .map(|source| source.details.clone())
}

pub(super) fn sfx_recovery_requires_staging(error: &FormatError) -> bool {
    let FormatError::Io(error) = error else {
        return false;
    };
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<SfxRecoveryIoError>())
        .is_some_and(|source| source.retain_staging)
}

pub(super) fn merge_cleanup_result(
    original: FormatError,
    cleanup: Result<(), FormatError>,
    target: &Path,
) -> FormatError {
    let Err(cleanup) = cleanup else {
        return original;
    };
    let mut paths = Vec::new();
    if let Some(details) = sfx_recovery_details(&original) {
        paths.extend(details.paths);
    }
    if let Some(details) = sfx_recovery_details(&cleanup) {
        paths.extend(details.paths);
    }
    recovery_error_without_staging(
        target,
        paths,
        format!("{original}; SFX staging cleanup also failed: {cleanup}"),
    )
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalLayout {
    SingleFile,
    MacosApp,
}

impl From<SfxLayout> for JournalLayout {
    fn from(layout: SfxLayout) -> Self {
        match layout {
            SfxLayout::SingleFile => Self::SingleFile,
            SfxLayout::MacosApp => Self::MacosApp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupRecord {
    version: u32,
    kind: TemporaryKind,
    layout: JournalLayout,
    requested_destination: StoredOsString,
    staged: StoredOsString,
    quarantine: StoredOsString,
    identity: PathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TemporaryKind {
    Stage,
    Payload,
}
#[derive(Debug)]
struct OpenCleanup {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    content_digest: [u8; 32],
    record: CleanupRecord,
}

pub(super) fn reserve_bundle_stage(
    destination: &Path,
) -> Result<(PathBuf, PathIdentity), FormatError> {
    let scope = Scope::sfx_directory(destination)?;
    reconcile_cleanup(&scope)?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    for _ in 0..1000u32 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-sfx-stage-{}-{sequence}.tmp",
            std::process::id()
        ));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        match builder.create(&path) {
            Ok(()) => {
                let identity = path_identity(&path)?;
                return Ok((path, identity));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve SFX staging next to {}",
        destination.display()
    )))
}

pub(super) fn reserve_file_stage(
    destination: &Path,
    kind: TemporaryKind,
) -> Result<crate::ReservedTempFile, FormatError> {
    let scope = Scope::sfx_directory(destination)?;
    reconcile_cleanup(&scope)?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    let (namespace, extension, subject) = match kind {
        TemporaryKind::Stage => ("stage", "tmp", "staging"),
        TemporaryKind::Payload => ("payload", "zip", "payload staging"),
    };
    for _ in 0..1000u32 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-sfx-{namespace}-{}-{sequence}.{extension}",
            std::process::id()
        ));
        match replacement::open_new_artifact(&path) {
            Ok(file) => {
                let identity = file_identity(&file)?;
                let path_matches = match kind {
                    TemporaryKind::Stage => path_identity(&path)? == identity,
                    TemporaryKind::Payload => path_identity(&path).ok() == Some(identity),
                };
                if !path_matches || !file.metadata()?.is_file() {
                    return Err(FormatError::Io(io::Error::other(format!(
                        "SFX {subject} changed while it was reserved"
                    ))));
                }
                return Ok(crate::ReservedTempFile {
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
        "could not reserve SFX {subject} next to {}",
        destination.display()
    )))
}

pub(super) fn discard_staged_path(
    staged: &Path,
    staged_identity: PathIdentity,
    layout: SfxLayout,
    requested_destination: &Path,
) -> Result<(), FormatError> {
    discard_staged_path_inner(staged, staged_identity, layout, requested_destination).map_err(
        |error| {
            let parent = parent_or_current(requested_destination);
            let cleanup = parent.join(CLEANUP_JOURNAL_NAME);
            let mut paths =
                sfx_recovery_details(&error).map_or_else(Vec::new, |details| details.paths);
            paths.extend(current_paths([staged, cleanup.as_path()]));
            let identity_note = match observed_identity(staged) {
                Ok(Some(identity)) if identity == staged_identity => {
                    "the original staging identity is still present"
                }
                Ok(Some(_)) => "the staging name now has a different identity and was not deleted",
                Ok(None) => "the original staging name is absent; inspect the cleanup record",
                Err(_) => "the staging identity could not be rechecked",
            };
            paths.sort();
            paths.dedup();
            recovery_error_without_staging(
                requested_destination,
                paths,
                format!("SFX staging cleanup failed: {error}; {identity_note}"),
            )
        },
    )
}

fn discard_staged_path_inner(
    staged: &Path,
    staged_identity: PathIdentity,
    layout: SfxLayout,
    requested_destination: &Path,
) -> Result<(), FormatError> {
    let scope = Scope::sfx_directory(requested_destination)?;
    reconcile_cleanup(&scope)?;
    match fs::symlink_metadata(staged) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    if fs::canonicalize(parent_or_current(staged))?
        != fs::canonicalize(parent_or_current(requested_destination))?
    {
        return Err(FormatError::Unsupported(
            "SFX cleanup source and destination must share a directory".into(),
        ));
    }
    ensure_staged_identity(staged, staged_identity, layout)?;
    let state_digest = path_state_digest(staged)?.ok_or_else(|| {
        recovery_error_without_staging(
            requested_destination,
            current_paths([staged]),
            "SFX staging disappeared while its cleanup state was being recorded".into(),
        )
    })?;
    let parent = fs::canonicalize(parent_or_current(staged))?;
    let quarantine = reserve_cleanup_quarantine(&parent)?;
    let staged_name = staged
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX staging path has no file name".into()))?;
    let kind = if staging_name_is_reserved(staged_name) {
        TemporaryKind::Stage
    } else if layout == SfxLayout::SingleFile && payload_name_is_reserved(staged_name) {
        TemporaryKind::Payload
    } else {
        return Err(FormatError::Unsupported(
            "SFX cleanup source is outside the reserved internal namespace".into(),
        ));
    };
    let record = CleanupRecord {
        version: CLEANUP_VERSION,
        kind,
        layout: layout.into(),
        requested_destination: StoredOsString::from_os_str(
            requested_destination.file_name().ok_or_else(|| {
                FormatError::Unsupported("SFX destination has no file name".into())
            })?,
        )?,
        staged: StoredOsString::from_os_str(staged_name)?,
        quarantine: StoredOsString::from_os_str(quarantine.file_name().ok_or_else(|| {
            FormatError::Unsupported("SFX cleanup quarantine has no file name".into())
        })?)?,
        identity: staged_identity,
        state_digest,
    };
    write_cleanup_record(requested_destination, &record, &mut sync_directory).map_err(|error| {
        recovery_error_without_staging(
            requested_destination,
            vec![staged.to_path_buf()],
            format!("could not record SFX staging cleanup: {error}"),
        )
    })?;
    reconcile_cleanup(&scope)
}

fn write_cleanup_record<S>(
    path_in_directory: &Path,
    record: &CleanupRecord,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let parent = fs::canonicalize(parent_or_current(path_in_directory))?;
    let path = parent.join(CLEANUP_JOURNAL_NAME);
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(output_exists_error(&path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = serde_json::to_vec(record)
        .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    if bytes.len() > CLEANUP_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "SFX cleanup record exceeds {CLEANUP_MAX_BYTES} bytes"
        )));
    }
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".squallz-sfx-cleanup-journal-{}-{sequence}.tmp",
        std::process::id()
    ));
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
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

        options.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    let mut file = options.open(&temp)?;
    let identity = file_identity(&file).map_err(|error| {
        recovery_error_without_staging(
            path_in_directory,
            vec![temp.clone()],
            format!(
                "the SFX cleanup record temp file could not be bound and was left for recovery: {error}"
            ),
        )
    })?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        return Err(cleanup_unpublished_record_temp(
            error.into(),
            &temp,
            &file,
            identity,
            path_in_directory,
            &[],
            false,
        ));
    }
    if let Err(error) = replacement::move_path_no_replace(&temp, &path) {
        return Err(cleanup_unpublished_record_temp(
            error.into(),
            &temp,
            &file,
            identity,
            path_in_directory,
            &[],
            false,
        ));
    }
    sync(&parent)?;
    if path_identity(&path)? != identity || file_identity(&file)? != identity {
        return Err(recovery_error_without_staging(
            path_in_directory,
            vec![path],
            "SFX cleanup record identity changed during publication".into(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut published = Vec::new();
    Read::by_ref(&mut file)
        .take((CLEANUP_MAX_BYTES + 1) as u64)
        .read_to_end(&mut published)?;
    if published != bytes {
        return Err(recovery_error_without_staging(
            path_in_directory,
            vec![path],
            "SFX cleanup record contents changed during publication".into(),
        ));
    }
    Ok(())
}

fn reconcile_cleanup_record<S>(path_in_directory: &Path, sync: &mut S) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    reconcile_cleanup_with_disposal_move(path_in_directory, sync, &mut |from, to| {
        replacement::move_path_no_replace(from, to)
    })
}

fn reconcile_cleanup_with_disposal_move<S, R>(
    path_in_directory: &Path,
    sync: &mut S,
    disposal_move: &mut R,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, &Path) -> io::Result<()>,
{
    let parent = fs::canonicalize(parent_or_current(path_in_directory))?;
    let path = parent.join(CLEANUP_JOURNAL_NAME);
    let open = match fs::symlink_metadata(&path) {
        Ok(_) => read_cleanup_record(&path, path_in_directory)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let cleanup_target = parent.join(
        checked_component(&open.record.requested_destination)
            .map_err(|error| with_cleanup_details(error, path_in_directory, [&path].as_slice()))?,
    );
    let staged_name = checked_component(&open.record.staged)
        .map_err(|error| with_cleanup_details(error, &cleanup_target, [&path].as_slice()))?;
    if !cleanup_source_name_is_valid(&open.record, &staged_name) {
        return Err(recovery_error_without_staging(
            &cleanup_target,
            vec![path],
            "SFX cleanup record contains an invalid source path".into(),
        ));
    }
    let staged = parent.join(staged_name);
    let quarantine_name = checked_component(&open.record.quarantine)
        .map_err(|error| with_cleanup_details(error, &cleanup_target, [&path].as_slice()))?;
    let quarantine_text = quarantine_name
        .to_str()
        .ok_or_else(|| FormatError::Unsupported("SFX cleanup quarantine name must be UTF-8".into()))
        .map_err(|error| with_cleanup_details(error, &cleanup_target, [&path].as_slice()))?;
    if !cleanup_quarantine_name_is_reserved(quarantine_text) {
        return Err(recovery_error_without_staging(
            &cleanup_target,
            vec![path],
            "SFX cleanup record contains an invalid quarantine path".into(),
        ));
    }
    let quarantine = parent.join(quarantine_name);
    let layout = match open.record.layout {
        JournalLayout::SingleFile => SfxLayout::SingleFile,
        JournalLayout::MacosApp => SfxLayout::MacosApp,
    };
    let cleanup_paths = [&path, &staged, &quarantine];
    let staged_identity = observed_identity(&staged)
        .map_err(|error| with_cleanup_details(error, &cleanup_target, &cleanup_paths))?;
    let quarantine_identity = observed_identity(&quarantine)
        .map_err(|error| with_cleanup_details(error, &cleanup_target, &cleanup_paths))?;
    let expected_digest = open.record.state_digest;
    match (staged_identity, quarantine_identity) {
        (Some(identity), None) if identity == open.record.identity => {
            ensure_staged_identity(&staged, open.record.identity, layout)
                .map_err(|error| with_cleanup_details(error, &cleanup_target, &cleanup_paths))?;
            verify_cleanup_path_digest(
                &staged,
                expected_digest,
                &cleanup_target,
                &cleanup_paths,
                "staging source before quarantine",
            )?;
            ensure_open_cleanup_binding(&open)
                .map_err(|error| with_cleanup_details(error, &cleanup_target, &cleanup_paths))?;
            replacement::move_path_no_replace(&staged, &quarantine).map_err(|error| {
                with_cleanup_details(error.into(), &cleanup_target, &cleanup_paths)
            })?;
            verify_cleanup_path_digest(
                &quarantine,
                expected_digest,
                &cleanup_target,
                &cleanup_paths,
                "staging source after quarantine",
            )?;
            sync_rename_parents(&staged, &quarantine, sync).map_err(|error| {
                with_cleanup_details(error.into(), &cleanup_target, &cleanup_paths)
            })?;
            verify_cleanup_path_digest(
                &quarantine,
                expected_digest,
                &cleanup_target,
                &cleanup_paths,
                "quarantined staging source after synchronization",
            )?;
        }
        (None, Some(identity)) if identity == open.record.identity => {
            verify_cleanup_path_digest(
                &quarantine,
                expected_digest,
                &cleanup_target,
                &cleanup_paths,
                "recovered cleanup quarantine",
            )?;
        }
        (None, None) => {
            return clear_cleanup_record(open, &cleanup_target, sync)
                .map_err(|error| with_cleanup_details(error, &cleanup_target, [&path].as_slice()));
        }
        state => {
            return Err(recovery_error_without_staging(
                &cleanup_target,
                current_paths([&path, &staged, &quarantine]),
                format!("SFX cleanup paths changed identity: {state:?}"),
            ));
        }
    }
    ensure_open_cleanup_binding(&open)
        .map_err(|error| with_cleanup_details(error, &cleanup_target, &cleanup_paths))?;
    remove_cleanup_quarantine_with(
        CleanupDisposal {
            quarantine: &quarantine,
            expected_identity: open.record.identity,
            layout,
            expected_digest,
            cleanup_target: &cleanup_target,
            cleanup_paths: &cleanup_paths,
        },
        disposal_move,
        sync,
    )?;
    clear_cleanup_record(open, &cleanup_target, sync).map_err(|error| {
        with_cleanup_details(error, &cleanup_target, [&path, &quarantine].as_slice())
    })
}

struct CleanupDisposal<'a> {
    quarantine: &'a Path,
    expected_identity: PathIdentity,
    layout: SfxLayout,
    expected_digest: [u8; 32],
    cleanup_target: &'a Path,
    cleanup_paths: &'a [&'a PathBuf],
}

fn remove_cleanup_quarantine_with<R, S>(
    disposal_request: CleanupDisposal<'_>,
    disposal_move: &mut R,
    sync: &mut S,
) -> Result<(), FormatError>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    let CleanupDisposal {
        quarantine,
        expected_identity,
        layout,
        expected_digest,
        cleanup_target,
        cleanup_paths,
    } = disposal_request;
    let parent = fs::canonicalize(parent_or_current(quarantine))?;
    let disposal = reserve_cleanup_quarantine(&parent)?;
    let mut recovery_paths = cleanup_paths.to_vec();
    recovery_paths.push(&disposal);
    let mut isolated_identity = None;
    let result = (|| {
        ensure_staged_identity(quarantine, expected_identity, layout)?;
        verify_cleanup_path_digest(
            quarantine,
            expected_digest,
            cleanup_target,
            &recovery_paths,
            "cleanup quarantine before final isolation",
        )?;
        // Shorten the namespace race by atomically moving the verified entry
        // to a fresh name, then repeat both checks immediately before delete.
        disposal_move(quarantine, &disposal)?;
        isolated_identity = observed_identity(&disposal)?;
        sync_rename_parents(quarantine, &disposal, sync)?;
        ensure_staged_identity(&disposal, expected_identity, layout)?;
        verify_cleanup_path_digest(
            &disposal,
            expected_digest,
            cleanup_target,
            &recovery_paths,
            "cleanup quarantine after final isolation",
        )?;
        match layout {
            SfxLayout::SingleFile => fs::remove_file(&disposal)?,
            SfxLayout::MacosApp => fs::remove_dir_all(&disposal)?,
        }
        sync(&parent)?;
        Ok::<(), FormatError>(())
    })();
    let Err(error) = result else {
        return Ok(());
    };

    let restoration = match (
        observed_identity(quarantine),
        observed_identity(&disposal),
    ) {
        (Ok(None), Ok(Some(identity))) if isolated_identity == Some(identity) => replacement::move_path_no_replace(&disposal, quarantine)
            .and_then(|()| sync_rename_parents(&disposal, quarantine, sync))
            .map(|()| "the isolated path was restored to its recorded quarantine name".into())
            .unwrap_or_else(|restore_error| {
                format!(
                    "the isolated path could not be restored to its recorded quarantine name: {restore_error}"
                )
            }),
        (Ok(None), Ok(Some(_))) => {
            "the final isolation path changed after its move and was retained without restoration".into()
        }
        (Ok(Some(_)), Ok(Some(_))) => {
            "both the recorded quarantine and final isolation path remain occupied".into()
        }
        (Ok(Some(_)), Ok(None)) => "the recorded quarantine path remains occupied".into(),
        (Ok(None), Ok(None)) => "both cleanup paths are currently absent".into(),
        (source, isolated) => format!(
            "cleanup path identities could not be rechecked after the failed removal: source={source:?}, isolated={isolated:?}"
        ),
    };
    Err(recovery_error_without_staging(
        cleanup_target,
        current_paths(&recovery_paths),
        format!(
            "SFX cleanup final isolation failed without deleting an unverified path: {error}; {restoration}"
        ),
    ))
}

fn verify_cleanup_path_digest(
    path: &Path,
    expected: [u8; 32],
    cleanup_target: &Path,
    cleanup_paths: &[&PathBuf],
    phase: &str,
) -> Result<(), FormatError> {
    let observed = path_state_digest(path)
        .map_err(|error| with_cleanup_details(error, cleanup_target, cleanup_paths))?;
    if observed == Some(expected) {
        return Ok(());
    }
    Err(recovery_error_without_staging(
        cleanup_target,
        current_paths(cleanup_paths),
        format!(
            "SFX cleanup tree changed during {phase} at {}; the cleanup record and current path were retained",
            path.display()
        ),
    ))
}

fn with_cleanup_details(error: FormatError, target: &Path, paths: &[&PathBuf]) -> FormatError {
    if sfx_recovery_details(&error).is_some() {
        return error;
    }
    recovery_error_without_staging(
        target,
        current_paths(paths),
        format!("SFX cleanup requires manual recovery: {error}"),
    )
}

fn clear_cleanup_record<S>(
    open: OpenCleanup,
    recovery_target: &Path,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_open_cleanup_binding(&open).map_err(|error| {
        recovery_error_without_staging(
            recovery_target,
            vec![open.path.clone()],
            format!("SFX cleanup record changed before removal: {error}"),
        )
    })?;
    remove_bound_path_via_quarantine(
        &open.path,
        open.identity,
        SfxLayout::SingleFile,
        recovery_target,
        "the SFX cleanup record",
        sync,
    )
}

fn read_cleanup_record(path: &Path, recovery_target: &Path) -> Result<OpenCleanup, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            "SFX cleanup record must be a regular file".into(),
        ));
    }
    let mut file = open_journal_file(path)?;
    let identity = file_identity(&file)?;
    if path_identity(path)? != identity {
        return Err(recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            "SFX cleanup record changed while it was opened".into(),
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((CLEANUP_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > CLEANUP_MAX_BYTES {
        return Err(recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            format!("SFX cleanup record exceeds {CLEANUP_MAX_BYTES} bytes"),
        ));
    }
    let record: CleanupRecord = serde_json::from_slice(&bytes).map_err(|error| {
        recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            format!("SFX cleanup record is invalid: {error}"),
        )
    })?;
    if record.version != CLEANUP_VERSION {
        return Err(recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            format!("unsupported SFX cleanup record version: {}", record.version),
        ));
    }
    let open = OpenCleanup {
        path: path.to_path_buf(),
        file,
        identity,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    };
    ensure_open_cleanup_binding(&open)?;
    Ok(open)
}

fn current_paths<I, P>(paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    paths
        .into_iter()
        .filter_map(|path| match fs::symlink_metadata(path.as_ref()) {
            Ok(_) => Some(path.as_ref().to_path_buf()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => Some(path.as_ref().to_path_buf()),
        })
        .collect()
}

fn cleanup_quarantine_name_is_reserved(name: &str) -> bool {
    name.strip_prefix(".squallz-sfx-cleanup-")
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(is_canonical_process_sequence)
}

fn reserve_cleanup_quarantine(parent: &Path) -> Result<PathBuf, FormatError> {
    (0..1000u32)
        .find_map(|_| {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".squallz-sfx-cleanup-{}-{sequence}.tmp",
                std::process::id()
            ));
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Some(candidate),
                _ => None,
            }
        })
        .ok_or_else(|| FormatError::Unsupported("could not reserve SFX cleanup quarantine".into()))
}

fn remove_bound_path_via_quarantine<S>(
    source: &Path,
    expected: PathIdentity,
    layout: SfxLayout,
    recovery_target: &Path,
    role: &str,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let parent = fs::canonicalize(parent_or_current(source))?;
    let quarantine = reserve_cleanup_quarantine(&parent)?;
    let result = (|| {
        ensure_staged_identity(source, expected, layout)?;
        replacement::move_path_no_replace(source, &quarantine)?;
        sync_rename_parents(source, &quarantine, sync)?;
        ensure_staged_identity(&quarantine, expected, layout)?;
        match layout {
            SfxLayout::SingleFile => fs::remove_file(&quarantine)?,
            SfxLayout::MacosApp => fs::remove_dir(&quarantine)?,
        }
        sync(&parent)?;
        Ok::<(), FormatError>(())
    })();
    result.map_err(|error| {
        recovery_error_without_staging(
            recovery_target,
            current_paths([source, quarantine.as_path()]),
            format!("could not safely remove {role} through an identity-bound quarantine: {error}"),
        )
    })
}

fn cleanup_journal_temp_name_is_reserved(name: &str) -> bool {
    name.strip_prefix(".squallz-sfx-cleanup-journal-")
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(is_canonical_process_sequence)
}

fn classify_cleanup_owned_path(
    record_path: &Path,
    parent: &Path,
    candidate: &Path,
) -> Result<bool, FormatError> {
    let recovery_target = parent.join("SFX-cleanup");
    let open = read_cleanup_record(record_path, &recovery_target)?;
    let recovery_target = parent.join(checked_component(&open.record.requested_destination)?);
    let staged_name = checked_component(&open.record.staged)?;
    if !cleanup_source_name_is_valid(&open.record, &staged_name) {
        return Err(recovery_error_without_staging(
            &recovery_target,
            vec![record_path.to_path_buf()],
            "SFX cleanup record contains an invalid source path".into(),
        ));
    }
    let staged = parent.join(staged_name);
    let quarantine_name = checked_component(&open.record.quarantine)?;
    let quarantine_text = quarantine_name.to_str().ok_or_else(|| {
        FormatError::Unsupported("SFX cleanup quarantine name must be UTF-8".into())
    })?;
    if !cleanup_quarantine_name_is_reserved(quarantine_text) {
        return Err(recovery_error_without_staging(
            &recovery_target,
            vec![record_path.to_path_buf()],
            "SFX cleanup record contains an invalid quarantine path".into(),
        ));
    }
    let quarantine = parent.join(quarantine_name);
    let staged_identity = observed_identity(&staged)?;
    let quarantine_identity = observed_identity(&quarantine)?;
    let state_valid = match (staged_identity, quarantine_identity) {
        (Some(identity), None) | (None, Some(identity)) => identity == open.record.identity,
        (None, None) => true,
        _ => false,
    };
    if !state_valid {
        return Err(recovery_error_without_staging(
            &recovery_target,
            current_paths([record_path, &staged, &quarantine]),
            "SFX cleanup record paths changed identity".into(),
        ));
    }
    let current = match (staged_identity, quarantine_identity) {
        (Some(_), None) => Some(staged.as_path()),
        (None, Some(_)) => Some(quarantine.as_path()),
        (None, None) | (Some(_), Some(_)) => None,
    };
    if let Some(current) = current {
        let observed = path_state_digest(current).map_err(|error| {
            recovery_error_without_staging(
                &recovery_target,
                current_paths([record_path, &staged, &quarantine]),
                format!("SFX cleanup tree could not be verified: {error}"),
            )
        })?;
        if observed != Some(open.record.state_digest) {
            return Err(recovery_error_without_staging(
                &recovery_target,
                current_paths([record_path, &staged, &quarantine]),
                "SFX cleanup tree changed after its record was published".into(),
            ));
        }
    }
    let owned = [record_path, staged.as_path(), quarantine.as_path()]
        .into_iter()
        .any(|path| crate::same_path_entry(path, candidate));
    if !owned
        && candidate.file_name().is_some_and(|name| {
            staging_name_is_reserved(name)
                || payload_name_is_reserved(name)
                || name
                    .to_str()
                    .is_some_and(cleanup_quarantine_name_is_reserved)
        })
    {
        return Err(recovery_error_without_staging(
            &recovery_target,
            current_paths([record_path, &staged, &quarantine, candidate]),
            format!(
                "an additional unregistered SFX cleanup path exists at {}",
                candidate.display()
            ),
        ));
    }
    Ok(owned)
}

fn commit_policy_allows_replace(commit_policy: CreateCommitPolicy) -> bool {
    !matches!(commit_policy, CreateCommitPolicy::NoReplace)
}

fn artifact_kind(layout: SfxLayout) -> CreateArtifactKind {
    match layout {
        SfxLayout::SingleFile => CreateArtifactKind::SfxSingleFile,
        SfxLayout::MacosApp => CreateArtifactKind::SfxMacosApp,
    }
}

fn required_path_state_digest(
    path: &Path,
    destination: &Path,
    missing_reason: &str,
) -> Result<[u8; 32], FormatError> {
    path_state_digest(path)?.ok_or_else(|| {
        recovery_error(
            destination,
            current_paths([path]),
            format!("SFX publication requires manual recovery: {missing_reason}"),
        )
    })
}

fn ensure_unjournaled_digest(
    path: &Path,
    expected: [u8; 32],
    destination: &Path,
    role: &str,
) -> Result<(), FormatError> {
    let observed = path_state_digest(path)?;
    if observed == Some(expected) {
        return Ok(());
    }
    Err(recovery_error(
        destination,
        current_paths([path, destination]),
        format!(
            "SFX publication requires manual recovery: the {role} content changed at {}",
            path.display()
        ),
    ))
}

fn validate_guarded_publish_destination(
    destination: &Path,
    requested_destination: &Path,
    layout: SfxLayout,
    allow_replace: bool,
    guarded_previous_digest: Option<[u8; 32]>,
) -> Result<bool, FormatError> {
    let exists = match validate_publish_destination(destination, layout, allow_replace) {
        Ok(exists) => exists,
        Err(FormatError::Unsupported(_)) if guarded_previous_digest.is_some() => {
            return Err(FormatError::destination_changed(
                requested_destination.to_path_buf(),
            ));
        }
        Err(FormatError::Io(error))
            if guarded_previous_digest.is_some()
                && matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) =>
        {
            return Err(FormatError::destination_changed(
                requested_destination.to_path_buf(),
            ));
        }
        Err(error) => return Err(error),
    };
    if guarded_previous_digest.is_some() && !exists {
        return Err(FormatError::destination_changed(
            requested_destination.to_path_buf(),
        ));
    }
    Ok(exists)
}

fn guarded_sfx_destination_error(
    destination: &Path,
    guarded: bool,
    error: FormatError,
) -> FormatError {
    if !guarded
        || sfx_recovery_details(&error).is_some()
        || matches!(&error, FormatError::Cancelled)
    {
        return error;
    }
    match error {
        FormatError::Unsupported(_) | FormatError::ResourceLimitExceeded(_) => {
            FormatError::destination_changed(destination.to_path_buf())
        }
        FormatError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            FormatError::destination_changed(destination.to_path_buf())
        }
        error => error,
    }
}

fn recovery_error(destination: &Path, paths: Vec<PathBuf>, message: String) -> FormatError {
    recovery_error_with_staging_policy(destination, paths, message, true)
}

fn recovery_error_without_staging(
    destination: &Path,
    paths: Vec<PathBuf>,
    message: String,
) -> FormatError {
    recovery_error_with_staging_policy(destination, paths, message, false)
}

fn recovery_error_with_staging_policy(
    destination: &Path,
    mut paths: Vec<PathBuf>,
    message: String,
    retain_staging: bool,
) -> FormatError {
    paths.sort();
    paths.dedup();
    FormatError::Io(io::Error::other(SfxRecoveryIoError {
        message,
        details: SfxRecoveryDetails {
            target: destination.to_path_buf(),
            paths,
        },
        retain_staging,
    }))
}

fn cleanup_unpublished_record_temp(
    original: FormatError,
    temp: &Path,
    file: &File,
    identity: PathIdentity,
    recovery_target: &Path,
    related_paths: &[PathBuf],
    retain_staging: bool,
) -> FormatError {
    match crate::remove_bound_temp_file(temp, file, identity) {
        Ok(()) => original,
        Err(cleanup) => {
            let mut paths = vec![temp.to_path_buf()];
            paths.extend_from_slice(related_paths);
            recovery_error_with_staging_policy(
                recovery_target,
                paths,
                format!(
                    "{original}; the unpublished SFX record temp could not be cleaned safely: {cleanup}"
                ),
                retain_staging,
            )
        }
    }
}

fn ensure_open_cleanup_binding(open: &OpenCleanup) -> Result<(), FormatError> {
    if bound_record_content_is_current(&open.path, &open.file, open.identity, open.content_digest)?
    {
        return Ok(());
    }
    Err(FormatError::Io(io::Error::other(format!(
        "SFX cleanup record identity or contents changed at {}; the record was left untouched",
        open.path.display()
    ))))
}

fn bound_record_content_is_current(
    path: &Path,
    file: &File,
    identity: PathIdentity,
    content_digest: [u8; 32],
) -> io::Result<bool> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_identity(path)? != identity
        || file_identity(file)? != identity
    {
        return Ok(false);
    }
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take((CLEANUP_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > CLEANUP_MAX_BYTES
        || *blake3::hash(&bytes).as_bytes() != content_digest
        || file_identity(file)? != identity
    {
        return Ok(false);
    }
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(!path_metadata.file_type().is_symlink()
        && path_metadata.is_file()
        && path_identity(path)? == identity)
}

fn observed_identity(path: &Path) -> Result<Option<PathIdentity>, FormatError> {
    match path_identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn checked_component(name: &StoredOsString) -> Result<OsString, FormatError> {
    let name = name.to_os_string()?;
    checked_path_component(Some(&name), "SFX transaction path")
}

fn staging_name_is_reserved(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(".squallz-sfx-stage-"))
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(is_canonical_process_sequence)
}

fn payload_name_is_reserved(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(".squallz-sfx-payload-"))
        .and_then(|name| name.strip_suffix(".zip"))
        .is_some_and(is_canonical_process_sequence)
}

fn cleanup_source_name_is_valid(record: &CleanupRecord, name: &OsStr) -> bool {
    match (record.kind, record.layout) {
        (TemporaryKind::Stage, _) => staging_name_is_reserved(name),
        (TemporaryKind::Payload, JournalLayout::SingleFile) => payload_name_is_reserved(name),
        (TemporaryKind::Payload, JournalLayout::MacosApp) => false,
    }
}

fn ensure_staged_identity(
    path: &Path,
    expected: PathIdentity,
    layout: SfxLayout,
) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    let type_matches = match layout {
        SfxLayout::SingleFile => metadata.is_file(),
        SfxLayout::MacosApp => metadata.is_dir(),
    };
    if metadata.file_type().is_symlink() || !type_matches || path_identity(path)? != expected {
        return Err(FormatError::Io(io::Error::other(
            "SFX staging identity or layout changed before publication",
        )));
    }
    Ok(())
}

fn ensure_staged_destination_identity(
    destination: &Path,
    expected: PathIdentity,
    layout: SfxLayout,
) -> Result<(), FormatError> {
    ensure_staged_identity(destination, expected, layout).map_err(|error| {
        recovery_error(
            destination,
            vec![destination.to_path_buf()],
            format!("the newly installed SFX output could not be identity-bound: {error}"),
        )
    })
}

fn sync_rename_parents<S>(source: &Path, destination: &Path, sync: &mut S) -> io::Result<()>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let source_parent = parent_or_current(source);
    let destination_parent = parent_or_current(destination);
    sync(destination_parent)?;
    if source_parent != destination_parent {
        sync(source_parent)?;
    }
    Ok(())
}

fn open_journal_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

        options.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    options.open(path)
}

pub(super) fn reconcile_cleanup(scope: &Scope) -> Result<(), FormatError> {
    scope.verify()?;
    reconcile_cleanup_record(&scope.requested_path(), &mut sync_directory)?;
    scope.verify()
}

pub(super) fn preflight_destination(destination: &Path) -> Result<(), FormatError> {
    let scope = Scope::sfx_directory(destination)?;
    reconcile_cleanup(&scope)?;
    let scope = scope
        .lock_target(destination)
        .map_err(|failure| replacement_error(failure, destination, false))?;
    let label = public_output_label(destination);
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    match replacement::recover(scope, ActiveWriter::None, &context)
        .map_err(|failure| replacement_error(failure, destination, false))?
    {
        Recovery::Clear(scope) => scope.verify(),
        Recovery::Backup { scope, backup } => Err(backup_pending_ack(&scope, &backup)),
    }
}

pub(super) fn publish_staged_sfx_after_cleanup(
    mut staged: StagedSfx,
    destination: &Path,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    control: &ControlToken,
    cleanup: impl FnOnce() -> Result<(), FormatError>,
) -> Result<SfxBuildReport, FormatError> {
    if let Err(error) = cleanup() {
        return Err(merge_cleanup_result(error, staged.discard(), destination));
    }
    publish_staged_sfx(staged, destination, commit_policy, progress, control)
}

pub(super) fn publish_staged_sfx(
    mut staged: StagedSfx,
    destination: &Path,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<SfxBuildReport, FormatError> {
    progress.on_phase(ProgressPhase::OutputCommit, false);
    let result = (|| {
        control.checkpoint()?;
        staged.verify_held_identity()?;
        let label = public_output_label(destination);
        let context = VerificationContext {
            public_label: &label,
            progress,
            control,
        };
        #[cfg(windows)]
        if staged.report.layout == SfxLayout::SingleFile {
            let held = staged.held_file.as_mut().ok_or_else(|| {
                FormatError::Unsupported(
                    "SFX publication requires its original staging reservation".into(),
                )
            })?;
            seal_staging_file(&staged.path, held, staged.identity, &context)?;
        }
        let held = staged.held_file.as_ref().ok_or_else(|| {
            FormatError::Unsupported(
                "SFX publication requires its original staging reservation".into(),
            )
        })?;
        let file = held.try_clone()?;
        let writer = match staged.report.layout {
            SfxLayout::SingleFile => OwnedInput::SfxFile {
                path: staged.path.clone(),
                file,
                identity: staged.identity,
            },
            SfxLayout::MacosApp => OwnedInput::SfxTree {
                path: staged.path.clone(),
                root: file,
                identity: staged.identity,
            },
        };
        publish_owned(
            writer,
            destination,
            staged.report.layout,
            commit_policy,
            &context,
        )
    })();
    match result {
        Ok(backups) => {
            staged.report.preserved_outputs = backups;
            progress.on_progress(
                staged.progress_total,
                staged.progress_total,
                &EntryPath::from_utf8(""),
            );
            Ok(staged.report)
        }
        Err(error) if !sfx_recovery_requires_staging(&error) => {
            Err(merge_cleanup_result(error, staged.discard(), destination))
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn seal_staging_file(
    path: &Path,
    file: &mut File,
    identity: PathIdentity,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    use crate::filesystem_identity::RegularFileState;
    use squallz_format_api::reopen_readonly_file;

    let metadata = file.metadata()?;
    let state = RegularFileState::from_metadata(&metadata);
    let readonly_permission = metadata.permissions().readonly();
    let verify = |held: &File, expected: &RegularFileState| -> Result<(), FormatError> {
        let metadata = fs::symlink_metadata(path)?;
        let held_metadata = held.metadata()?;
        if metadata.file_type().is_symlink()
            || !expected.matches(&metadata)
            || !expected.matches(&held_metadata)
            || metadata.permissions().readonly() != readonly_permission
            || held_metadata.permissions().readonly() != readonly_permission
            || file_identity(held)? != identity
            || path_identity(path)? != identity
        {
            return Err(FormatError::Io(io::Error::other(
                "SFX staging changed during read-only handle handoff",
            )));
        }
        Ok(())
    };
    verify(file, &state)?;
    file.sync_all()?;
    let synced = RegularFileState::from_metadata(&file.metadata()?);
    if synced.bytes() != state.bytes() {
        return Err(FormatError::Io(io::Error::other(
            "SFX staging length changed while it was synchronized",
        )));
    }
    let state = synced;
    verify(file, &state)?;
    let digest = replacement::hash_payload(file, &state, context)?;
    verify(file, &state)?;
    let readonly = reopen_readonly_file(file, true)?;
    verify(&readonly, &state)?;
    // Close the sole write handle while a readonly handle still binds the object.
    *file = readonly;
    let strict = reopen_readonly_file(file, false)?;
    *file = strict;
    // Windows finishes the last write timestamp only after the writer closes.
    // Refresh that state only after binding the same object and proving its bytes.
    let sealed = RegularFileState::from_metadata(&file.metadata()?);
    verify(file, &sealed)?;
    if sealed.bytes() != state.bytes()
        || replacement::hash_payload(file, &sealed, context)? != digest
    {
        return Err(FormatError::Io(io::Error::other(
            "SFX staging contents changed during read-only handle handoff",
        )));
    }
    context.control.checkpoint()?;
    verify(file, &sealed)
}

fn publish_owned(
    writer: OwnedInput,
    destination: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
    context: &VerificationContext<'_>,
) -> Result<Vec<PathBuf>, FormatError> {
    let guarded = matches!(commit_policy, CreateCommitPolicy::ReplaceIfUnchanged(_));
    let scope = Scope::sfx_directory(destination)
        .map_err(|error| guarded_sfx_destination_error(destination, guarded, error))?;
    reconcile_cleanup(&scope)?;
    verify_owned_path(&writer, writer.path(), layout)?;
    let active = ActiveWriter::With(&writer);
    let scope = scope.lock_target(destination).map_err(|failure| {
        guarded_sfx_destination_error(
            destination,
            guarded,
            replacement_error(failure, destination, false),
        )
    })?;
    let scope = match replacement::recover(scope, active, context)
        .map_err(|failure| replacement_error(failure, destination, false))?
    {
        Recovery::Clear(scope) => scope,
        Recovery::Backup { scope, backup } => {
            return Err(backup_pending_ack(&scope, &backup));
        }
    };
    let requested = scope.requested_path();
    let target = scope.target_path();
    let authorized_digest = match commit_policy {
        CreateCommitPolicy::ReplaceIfUnchanged(guard) => Some(verify_destination_guard(
            &requested,
            artifact_kind(layout),
            guard,
        )?),
        CreateCommitPolicy::NoReplace | CreateCommitPolicy::ReplaceExisting => None,
    };
    let exists = validate_guarded_publish_destination(
        &target,
        &requested,
        layout,
        commit_policy_allows_replace(commit_policy),
        authorized_digest,
    )
    .map_err(|error| guarded_sfx_destination_error(&requested, guarded, error))?;
    if !exists {
        publish_direct(&scope, writer, layout, context)?;
        return Ok(Vec::new());
    }
    let previous = open_previous(&scope, layout)
        .map_err(|error| guarded_sfx_destination_error(&requested, guarded, error))?;
    let authorization = match authorized_digest {
        Some(digest) => Authorization::Guarded { digest },
        None => Authorization::ContentBound,
    };
    let prepared = replacement::prepare(scope, previous, writer, authorization, context).map_err(
        |failure| replacement_error(replacement::discard_prepare(failure), destination, false),
    )?;
    let transaction = match replacement::persist(prepared) {
        Ok(transaction) => transaction,
        Err(PersistFailure::Unpublished { prepared, error }) => {
            return Err(replacement_error(
                replacement::discard_unpublished(*prepared, error),
                destination,
                false,
            ));
        }
        Err(PersistFailure::Retained(failure)) => {
            return Err(replacement_error(failure, destination, true));
        }
    };
    let noncancelled = ControlToken::default();
    let context = VerificationContext {
        public_label: context.public_label,
        progress: context.progress,
        control: &noncancelled,
    };
    let installed = replacement::commit_resume(transaction, &context)
        .map_err(|failure| replacement_error(failure, destination, true))?;
    match replacement::complete(installed, &context)
        .map_err(|failure| replacement_error(failure, destination, true))?
    {
        Outcome::Sfx { backup, .. } => Ok(vec![backup]),
        Outcome::SfxCleared { scope } => Err(recovery_error(
            &scope.target_path(),
            vec![scope.target_path()],
            "new SFX publication completed without its required previous-output backup".into(),
        )),
        Outcome::Archive { .. } => Err(FormatError::Unsupported(
            "archive completion cannot satisfy an SFX publication".into(),
        )),
    }
}

fn open_previous(scope: &Scope, layout: SfxLayout) -> Result<OwnedInput, FormatError> {
    scope.verify()?;
    let path = scope.target_path();
    let file = match layout {
        SfxLayout::SingleFile => open_regular_file_no_follow(&path)?,
        SfxLayout::MacosApp => crate::open_directory_no_follow(&path)?,
    };
    let identity = file_identity(&file)?;
    ensure_staged_identity(&path, identity, layout)?;
    let result = match layout {
        SfxLayout::SingleFile => OwnedInput::SfxFile {
            path,
            file,
            identity,
        },
        SfxLayout::MacosApp => OwnedInput::SfxTree {
            path,
            root: file,
            identity,
        },
    };
    scope.verify()?;
    Ok(result)
}

fn verify_owned_path(
    writer: &OwnedInput,
    path: &Path,
    layout: SfxLayout,
) -> Result<(), FormatError> {
    let held = match writer {
        OwnedInput::SfxFile { file, .. } if layout == SfxLayout::SingleFile => file,
        OwnedInput::SfxTree { root, .. } if layout == SfxLayout::MacosApp => root,
        _ => {
            return Err(FormatError::Unsupported(
                "SFX writer layout and held reservation disagree".into(),
            ))
        }
    };
    if file_identity(held)? != writer.identity() {
        return Err(FormatError::Io(io::Error::other(
            "SFX held writer identity changed",
        )));
    }
    ensure_staged_identity(path, writer.identity(), layout)
}

fn publish_direct(
    scope: &Scope,
    writer: OwnedInput,
    layout: SfxLayout,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    let target = scope.target_path();
    context.control.checkpoint()?;
    scope.verify()?;
    verify_owned_path(&writer, writer.path(), layout)?;
    let digest = required_path_state_digest(writer.path(), &target, "the staged SFX disappeared")?;
    context.control.checkpoint()?;
    ensure_unjournaled_digest(writer.path(), digest, &target, "staged SFX replacement")?;
    verify_owned_path(&writer, writer.path(), layout)?;
    let moved = match replacement::move_path_no_replace(writer.path(), &target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(output_exists_error(&target))
        }
        Err(error) => Err(error.into()),
    };
    if let Err(error) = moved {
        match observed_identity(&target) {
            Ok(Some(identity)) if identity == writer.identity() => {}
            Ok(_) => return Err(error),
            Err(observe_error) => {
                return Err(recovery_error(
                    &target, current_paths([writer.path(), &target]),
                    format!("SFX direct publication move failed: {error}; the output could not be inspected: {observe_error}"),
                ));
            }
        }
        return Err(recovery_error(
            &target,
            current_paths([writer.path(), &target]),
            format!("SFX direct publication did not finish its move: {error}"),
        ));
    }
    let finalize = (|| {
        verify_owned_path(&writer, &target, layout)?;
        ensure_unjournaled_digest(&target, digest, &target, "newly installed SFX output")?;
        scope.sync()?;
        scope.verify()?;
        ensure_staged_destination_identity(&target, writer.identity(), layout)?;
        verify_owned_path(&writer, &target, layout)?;
        ensure_unjournaled_digest(&target, digest, &target, "newly installed SFX output")?;
        Ok::<(), FormatError>(())
    })();
    finalize.map_err(|error| recovery_error(
        &target, vec![target.clone()],
        format!("the new SFX output was installed, but its final binding or parent synchronization failed: {error}"),
    ))
}

fn public_output_label(path: &Path) -> EntryPath {
    EntryPath::from_utf8(
        path.file_name()
            .unwrap_or_else(|| OsStr::new("SFX-output"))
            .to_string_lossy(),
    )
}

fn backup_pending_ack(scope: &Scope, backup: &Path) -> FormatError {
    recovery_error_without_staging(
        &scope.target_path(),
        vec![
            scope.anchor(RecordPhase::Completed),
            parent_or_current(backup).to_path_buf(),
            backup.to_path_buf(),
        ],
        format!(
            "the SFX replacement is complete and its previous output is retained at '{}'; test the current output, delete that backup when it is no longer needed, then try again",
            backup.display(),
        ),
    )
}

fn replacement_error(failure: Failure, target: &Path, retain_writer: bool) -> FormatError {
    let target = failure
        .artifacts
        .iter()
        .find(|artifact| matches!(artifact.role, ArtifactRole::Target))
        .map_or(target, |artifact| artifact.path.as_path());
    let paths = current_paths(
        failure
            .artifacts
            .iter()
            .filter(|artifact| {
                !matches!(
                    artifact.role,
                    ArtifactRole::ActiveWriter | ArtifactRole::Target
                )
            })
            .map(|artifact| artifact.path.as_path()),
    );
    let owns_debt = failure.artifacts.iter().any(|artifact| {
        !matches!(
            artifact.role,
            ArtifactRole::WriterStage | ArtifactRole::ActiveWriter | ArtifactRole::Target
        ) && paths.iter().any(|path| path == &artifact.path)
    });
    if failure.visibility == Visibility::Unpublished && !owns_debt {
        return failure.error;
    }
    recovery_error_with_staging_policy(target, paths, failure.error.to_string(), retain_writer)
}

pub(crate) fn classify_sfx_transaction_artifact(candidate: &Path) -> Result<bool, FormatError> {
    if !has_sfx_artifact_shape(candidate) {
        return Ok(false);
    }
    let name = candidate.file_name().and_then(OsStr::to_str);
    let parent = fs::canonicalize(parent_or_current(candidate))?;
    if name.is_some_and(|name| {
        name == CLEANUP_JOURNAL_NAME || cleanup_journal_temp_name_is_reserved(name)
    }) {
        return classify_cleanup_owned_path(candidate, &parent, candidate);
    }
    let cleanup = parent.join(CLEANUP_JOURNAL_NAME);
    match fs::symlink_metadata(&cleanup) {
        Ok(_) if classify_cleanup_owned_path(&cleanup, &parent, candidate)? => return Ok(true),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let label = EntryPath::from_utf8("SFX-output");
    let control = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: &NoProgress,
        control: &control,
    };
    if let Some(owned) = replacement::classify_sfx_artifact(candidate, &context)
        .map_err(|failure| replacement_error(failure, &parent.join("SFX-output"), false))?
    {
        if owned {
            return Ok(true);
        }
    }
    if name.is_some_and(|name| {
        staging_name_is_reserved(OsStr::new(name))
            || payload_name_is_reserved(OsStr::new(name))
            || cleanup_quarantine_name_is_reserved(name)
    }) {
        return Err(recovery_error_without_staging(
            &parent.join("SFX-output"), vec![candidate.to_path_buf()],
            "an SFX internal path has no matching durable record; inspect it before creating another archive".into(),
        ));
    }
    Ok(false)
}

fn has_sfx_artifact_shape(candidate: &Path) -> bool {
    let name = candidate.file_name().and_then(OsStr::to_str);
    if name.is_some_and(|name| name.starts_with(".squallz-sfx-")) {
        return true;
    }
    if name.is_some_and(|name| {
        name.starts_with('.') && name.contains(".sfx-") && name.contains(".tmp.")
    }) {
        return true;
    }
    matches!(
        name,
        Some("previous" | "replacement" | "retired" | "alias-replacement")
    ) && candidate
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .is_some_and(|parent| parent.starts_with(".squallz-sfx-"))
}

#[cfg(test)]
mod tests;
