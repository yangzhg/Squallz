use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use squallz_format_api::FormatError;

use super::{output_exists_error, validate_publish_destination, SfxLayout};
use crate::archive_path::{checked_path_component, is_canonical_process_sequence};
use crate::destination_guard::{
    path_state_digest, verify_destination_guard, verify_moved_path_state,
};
pub(super) use crate::filesystem_identity::{file_identity, path_identity, PathIdentity};
use crate::stored_os_string::StoredOsString;
use crate::{
    parent_or_current, sync_directory, CreateArtifactKind, CreateCommitPolicy,
    CreateDestinationGuard,
};

const TRANSACTION_VERSION: u32 = 1;
const CLEANUP_VERSION: u32 = 1;
const TRANSACTION_MAX_BYTES: usize = 64 * 1024;
const TRANSACTION_JOURNAL_NAME: &str = ".squallz-sfx-transaction.json";
const TRANSACTION_COMPLETION_NAME: &str = ".squallz-sfx-completed.json";
const CLEANUP_JOURNAL_NAME: &str = ".squallz-sfx-cleanup.json";
static TRANSACTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) struct ReservedSingleFileStage {
    pub(super) path: PathBuf,
    pub(super) file: File,
    pub(super) identity: PathIdentity,
}

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BoundOutput {
    path: PathBuf,
    identity: PathIdentity,
    digest: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
struct TransactionDigests {
    previous: [u8; 32],
    replacement: [u8; 32],
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
struct TransactionRecord {
    version: u32,
    layout: JournalLayout,
    destination: StoredOsString,
    requested_destination: StoredOsString,
    staged: StoredOsString,
    holder: StoredOsString,
    holder_identity: PathIdentity,
    previous_identity: PathIdentity,
    replacement_identity: PathIdentity,
    previous_digest: [u8; 32],
    replacement_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupRecord {
    version: u32,
    kind: CleanupKind,
    layout: JournalLayout,
    requested_destination: StoredOsString,
    staged: StoredOsString,
    quarantine: StoredOsString,
    identity: PathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CleanupKind {
    Stage,
    Payload,
}

#[derive(Debug)]
struct OpenTransaction {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    content_digest: [u8; 32],
    record: TransactionRecord,
}

#[derive(Debug)]
struct OpenCleanup {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    content_digest: [u8; 32],
    record: CleanupRecord,
}

#[derive(Debug)]
struct ResolvedTransaction {
    layout: SfxLayout,
    destination: PathBuf,
    staged: PathBuf,
    holder: PathBuf,
    previous: PathBuf,
    replacement: PathBuf,
    completion: PathBuf,
    journal: PathBuf,
    holder_identity: PathIdentity,
    previous_identity: PathIdentity,
    replacement_identity: PathIdentity,
    previous_digest: [u8; 32],
    replacement_digest: [u8; 32],
}

pub(super) fn replace_bound_staged_path(
    staged: &Path,
    staged_identity: PathIdentity,
    destination: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
) -> Result<Vec<PathBuf>, FormatError> {
    replace_bound_staged_path_with(
        staged,
        staged_identity,
        destination,
        layout,
        commit_policy,
        &mut |from, to| crate::move_path_no_replace(from, to),
        &mut sync_directory,
    )
    .map_err(|error| with_active_staging_path(error, staged))
}

pub(super) fn preflight_destination(destination: &Path) -> Result<(), FormatError> {
    let _directory_lock = lock_destination_directory(destination)?;
    reconcile_cleanup(destination, &mut sync_directory)?;
    reconcile_completed_transaction(destination, &mut sync_directory)?;
    let destination = resolve_publish_destination(destination)?;
    let _lock = lock_destination(&destination)?;
    let recovered = recover_transaction(
        &destination,
        None,
        &mut |from, to| crate::move_path_no_replace(from, to),
        &mut sync_directory,
    )?;
    if recovered.is_empty() {
        Ok(())
    } else {
        Err(recovered_outputs_pending_ack(&destination, &recovered))
    }
}

#[cfg(test)]
fn replace_staged_path(
    staged: &Path,
    destination: &Path,
    layout: SfxLayout,
    overwrite: bool,
) -> Result<Vec<PathBuf>, FormatError> {
    replace_bound_staged_path(
        staged,
        path_identity(staged)?,
        destination,
        layout,
        commit_policy_from_overwrite(overwrite),
    )
}

pub(super) fn reserve_staged_path(
    destination: &Path,
    layout: SfxLayout,
) -> Result<(PathBuf, PathIdentity), FormatError> {
    if layout == SfxLayout::SingleFile {
        let reserved = reserve_single_file_stage(destination)?;
        let path = reserved.path;
        let identity = reserved.identity;
        drop(reserved.file);
        return Ok((path, identity));
    }
    let _directory_lock = lock_destination_directory(destination)?;
    reconcile_cleanup(destination, &mut sync_directory)?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    for _ in 0..1000u32 {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-sfx-stage-{}-{sequence}.tmp",
            std::process::id()
        ));
        let result = match layout {
            SfxLayout::SingleFile => Err(io::Error::other(
                "single-file staging must use its retained reservation",
            )),
            SfxLayout::MacosApp => {
                #[cfg(unix)]
                let builder = {
                    use std::os::unix::fs::DirBuilderExt;

                    let mut builder = fs::DirBuilder::new();
                    builder.mode(0o700);
                    builder
                };
                #[cfg(not(unix))]
                let builder = fs::DirBuilder::new();
                builder.create(&path)
            }
        };
        match result {
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

pub(super) fn reserve_single_file_stage(
    destination: &Path,
) -> Result<ReservedSingleFileStage, FormatError> {
    let _directory_lock = lock_destination_directory(destination)?;
    reconcile_cleanup(destination, &mut sync_directory)?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    for _ in 0..1000u32 {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-sfx-stage-{}-{sequence}.tmp",
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
                    return Err(FormatError::Io(io::Error::other(
                        "SFX staging changed while it was reserved",
                    )));
                }
                return Ok(ReservedSingleFileStage {
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
        "could not reserve SFX staging next to {}",
        destination.display()
    )))
}

pub(super) fn reserve_payload_path(
    destination: &Path,
) -> Result<crate::ReservedTempFile, FormatError> {
    let _directory_lock = lock_destination_directory(destination)?;
    reconcile_cleanup(destination, &mut sync_directory)?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    for _ in 0..1000u32 {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-sfx-payload-{}-{sequence}.zip",
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
                if path_identity(&path).ok() != Some(identity) || !file.metadata()?.is_file() {
                    return Err(FormatError::Io(io::Error::other(
                        "SFX payload staging changed while it was reserved",
                    )));
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
        "could not reserve SFX payload staging next to {}",
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
    let _directory_lock = lock_destination_directory(requested_destination)?;
    reconcile_cleanup(requested_destination, &mut sync_directory)?;
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
        CleanupKind::Stage
    } else if layout == SfxLayout::SingleFile && payload_name_is_reserved(staged_name) {
        CleanupKind::Payload
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
    reconcile_cleanup(requested_destination, &mut sync_directory)
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
    if bytes.len() > TRANSACTION_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "SFX cleanup record exceeds {TRANSACTION_MAX_BYTES} bytes"
        )));
    }
    let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
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
    if let Err(error) = crate::move_path_no_replace(&temp, &path) {
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
    file.read_to_end(&mut published)?;
    if published != bytes {
        return Err(recovery_error_without_staging(
            path_in_directory,
            vec![path],
            "SFX cleanup record contents changed during publication".into(),
        ));
    }
    Ok(())
}

fn reconcile_cleanup<S>(path_in_directory: &Path, sync: &mut S) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    reconcile_cleanup_with_disposal_move(path_in_directory, sync, &mut |from, to| {
        crate::move_path_no_replace(from, to)
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
            crate::move_path_no_replace(&staged, &quarantine).map_err(|error| {
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
        (Ok(None), Ok(Some(_))) => crate::move_path_no_replace(&disposal, quarantine)
            .and_then(|()| sync_rename_parents(&disposal, quarantine, sync))
            .map(|()| "the isolated path was restored to its recorded quarantine name".into())
            .unwrap_or_else(|restore_error| {
                format!(
                    "the isolated path could not be restored to its recorded quarantine name: {restore_error}"
                )
            }),
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
        current_paths_from_slice(&recovery_paths),
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
        current_paths_from_slice(cleanup_paths),
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
        current_paths_from_slice(paths),
        format!("SFX cleanup requires manual recovery: {error}"),
    )
}

fn current_paths_from_slice(paths: &[&PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| match fs::symlink_metadata(path) {
            Ok(_) => Some((*path).to_path_buf()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => Some((*path).to_path_buf()),
        })
        .collect()
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
        .take((TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > TRANSACTION_MAX_BYTES {
        return Err(recovery_error_without_staging(
            recovery_target,
            vec![path.to_path_buf()],
            format!("SFX cleanup record exceeds {TRANSACTION_MAX_BYTES} bytes"),
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

fn current_paths<const N: usize>(paths: [&Path; N]) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| match fs::symlink_metadata(path) {
            Ok(_) => Some(path.to_path_buf()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => Some(path.to_path_buf()),
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
            let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
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
        crate::move_path_no_replace(source, &quarantine)?;
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

fn with_active_staging_path(error: FormatError, staged: &Path) -> FormatError {
    if !sfx_recovery_requires_staging(&error) {
        return error;
    }
    let Some(details) = sfx_recovery_details(&error) else {
        return error;
    };
    let mut paths = details.paths;
    match fs::symlink_metadata(staged) {
        Ok(_) => paths.push(canonical_destination(staged).unwrap_or_else(|_| staged.to_path_buf())),
        Err(io_error) if io_error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => paths.push(staged.to_path_buf()),
    }
    recovery_error(&details.target, paths, error.to_string())
}

#[cfg(test)]
fn matches_sfx_transaction_artifact(_destination: &Path, candidate: &Path) -> bool {
    matches!(
        classify_sfx_transaction_artifact(candidate),
        Ok(true) | Err(_)
    )
}

pub(crate) fn classify_sfx_transaction_artifact(candidate: &Path) -> Result<bool, FormatError> {
    if !has_sfx_artifact_shape(candidate) {
        return Ok(false);
    }
    let Some(name) = candidate.file_name().and_then(OsStr::to_str) else {
        return Ok(false);
    };
    let candidate_parent = parent_or_current(candidate);

    if name == CLEANUP_JOURNAL_NAME || cleanup_journal_temp_name_is_reserved(name) {
        let parent = fs::canonicalize(candidate_parent)?;
        return classify_cleanup_owned_path(candidate, &parent, candidate);
    }

    if name == TRANSACTION_JOURNAL_NAME {
        let parent = fs::canonicalize(candidate_parent)?;
        return classify_record_owned_path(candidate, &parent, candidate, false);
    }
    if name == TRANSACTION_COMPLETION_NAME {
        let parent = fs::canonicalize(candidate_parent)?;
        return classify_record_owned_path(candidate, &parent, candidate, true);
    }
    if journal_temp_name_is_reserved(name) {
        let parent = fs::canonicalize(candidate_parent)?;
        return classify_record_owned_path(candidate, &parent, candidate, false);
    }

    let holder = if holder_name_is_reserved(name) {
        Some(candidate.to_path_buf())
    } else if matches!(name, "previous" | "replacement")
        && candidate_parent
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(holder_name_is_reserved)
    {
        Some(candidate_parent.to_path_buf())
    } else {
        None
    };
    let record_parent = holder
        .as_deref()
        .map(parent_or_current)
        .unwrap_or(candidate_parent);
    let parent = fs::canonicalize(record_parent)?;
    if holder.is_none() {
        let cleanup = parent.join(CLEANUP_JOURNAL_NAME);
        match fs::symlink_metadata(&cleanup) {
            Ok(_) if classify_cleanup_owned_path(&cleanup, &parent, candidate)? => {
                return Ok(true);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let active = parent.join(TRANSACTION_JOURNAL_NAME);
    match fs::symlink_metadata(&active) {
        Ok(_) => return classify_record_owned_path(&active, &parent, candidate, false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let completed = parent.join(TRANSACTION_COMPLETION_NAME);
    match fs::symlink_metadata(&completed) {
        Ok(_) => {
            if classify_record_owned_path(&completed, &parent, candidate, true)? {
                return Ok(true);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let strict_orphan = staging_name_is_reserved(OsStr::new(name))
        || payload_name_is_reserved(OsStr::new(name))
        || cleanup_quarantine_name_is_reserved(name)
        || holder.is_some();
    if strict_orphan {
        return Err(recovery_error_without_staging(
            &parent.join("SFX-output"),
            vec![candidate.to_path_buf()],
            format!(
                "an SFX internal path has no matching durable transaction record at {}; inspect or remove it before creating another archive",
                candidate.display()
            ),
        ));
    }
    Ok(false)
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

fn classify_record_owned_path(
    record_path: &Path,
    parent: &Path,
    candidate: &Path,
    completed: bool,
) -> Result<bool, FormatError> {
    let recovery_target = parent.join("SFX-output");
    let description = if completed {
        "completed transaction index"
    } else {
        "replacement journal"
    };
    let open = read_transaction_file(record_path, &recovery_target, description)?;
    let destination_name = checked_component(&open.record.destination).map_err(|error| {
        recovery_error_without_staging(
            &recovery_target,
            vec![record_path.to_path_buf()],
            format!("SFX {description} has an invalid destination binding: {error}"),
        )
    })?;
    let destination = parent.join(destination_name);
    let transaction = resolve_transaction_paths(&destination, &open.record).map_err(|error| {
        recovery_error_without_staging(
            &destination,
            vec![record_path.to_path_buf()],
            format!("SFX {description} is invalid and was left untouched: {error}"),
        )
    })?;
    let state = if completed {
        validate_completed_record_state(&transaction)
    } else {
        validate_pending_record_state(&transaction)
    };
    if let Err(reason) = state {
        let mut paths = transaction_recovery_paths(&transaction);
        paths.push(record_path.to_path_buf());
        return Err(recovery_error_without_staging(
            &destination,
            paths,
            format!("SFX {description} failed identity validation: {reason}"),
        ));
    }
    let owned = if completed {
        vec![
            record_path.to_path_buf(),
            transaction.holder,
            transaction.previous,
        ]
    } else {
        vec![
            record_path.to_path_buf(),
            transaction.staged,
            transaction.holder,
            transaction.previous,
            transaction.replacement,
        ]
    };
    Ok(owned
        .iter()
        .any(|path| crate::same_path_entry(path, candidate)))
}

fn validate_pending_record_state(transaction: &ResolvedTransaction) -> Result<(), String> {
    validate_holder(transaction).map_err(|error| error.to_string())?;
    if observed_identity(&transaction.completion)
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("active and completed transaction records overlap".into());
    }
    let locations = [
        (
            &transaction.staged,
            transaction.replacement_identity,
            transaction.replacement_digest,
        ),
        (
            &transaction.replacement,
            transaction.replacement_identity,
            transaction.replacement_digest,
        ),
        (
            &transaction.destination,
            transaction.replacement_identity,
            transaction.replacement_digest,
        ),
        (
            &transaction.destination,
            transaction.previous_identity,
            transaction.previous_digest,
        ),
        (
            &transaction.previous,
            transaction.previous_identity,
            transaction.previous_digest,
        ),
    ];
    let mut replacement_entries = 0usize;
    let mut previous_entries = 0usize;
    for (path, expected, digest) in locations {
        if let Some(identity) = observed_identity(path).map_err(|error| error.to_string())? {
            if identity == expected {
                let observed = path_state_digest(path).map_err(|error| error.to_string())?;
                if observed != Some(digest) {
                    return Err(format!("content changed at {}", path.display()));
                }
                if expected == transaction.replacement_identity {
                    replacement_entries += 1;
                } else {
                    previous_entries += 1;
                }
            } else if path != &transaction.destination {
                return Err(format!("unexpected identity at {}", path.display()));
            }
        }
    }
    if replacement_entries == 0 || previous_entries == 0 {
        return Err("transaction identities are no longer reachable".into());
    }
    Ok(())
}

fn validate_completed_record_state(transaction: &ResolvedTransaction) -> Result<(), String> {
    if observed_identity(&transaction.destination).map_err(|error| error.to_string())?
        != Some(transaction.replacement_identity)
    {
        return Err("completed destination identity changed".into());
    }
    let observed =
        path_state_digest(&transaction.destination).map_err(|error| error.to_string())?;
    if observed != Some(transaction.replacement_digest) {
        return Err("completed destination content changed".into());
    }
    let holder = match fs::symlink_metadata(&transaction.holder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !holder.is_dir()
        || holder.file_type().is_symlink()
        || path_identity(&transaction.holder).map_err(|error| error.to_string())?
            != transaction.holder_identity
    {
        return Err("completed transaction holder identity or type changed".into());
    }
    let mut count = 0usize;
    for entry in fs::read_dir(&transaction.holder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() != OsStr::new("previous") {
            return Err(format!(
                "completed transaction holder contains unexpected entry {}",
                entry.path().display()
            ));
        }
        count += 1;
        if count > 1 {
            return Err("completed transaction holder contains too many entries".into());
        }
    }
    if let Some(identity) =
        observed_identity(&transaction.previous).map_err(|error| error.to_string())?
    {
        if identity != transaction.previous_identity {
            return Err("completed previous-output identity changed".into());
        }
        let observed =
            path_state_digest(&transaction.previous).map_err(|error| error.to_string())?;
        if observed != Some(transaction.previous_digest) {
            return Err("completed previous-output content changed".into());
        }
    }
    Ok(())
}

fn journal_temp_name_is_reserved(name: &str) -> bool {
    name.strip_prefix(".squallz-sfx-journal-")
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(is_canonical_process_sequence)
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
    matches!(name, Some("previous" | "replacement"))
        && candidate
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .is_some_and(|parent| parent.starts_with(".squallz-sfx-"))
}

#[cfg(test)]
fn commit_policy_from_overwrite(overwrite: bool) -> CreateCommitPolicy {
    if overwrite {
        CreateCommitPolicy::ReplaceExisting
    } else {
        CreateCommitPolicy::NoReplace
    }
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

fn replace_bound_staged_path_with<M, S>(
    staged: &Path,
    staged_identity: PathIdentity,
    destination: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<Vec<PathBuf>, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    // A directory-scoped lock closes the only gap where two differently-spelled
    // names can both appear absent on a case-insensitive filesystem. Keep it
    // until publication is complete so the second publisher can resolve the
    // entry created by the first before choosing its destination lock.
    let _directory_lock = lock_destination_directory(destination)?;
    reconcile_cleanup(destination, sync)?;
    ensure_staged_identity(staged, staged_identity, layout)?;
    reconcile_completed_transaction(destination, sync)?;
    let guarded_request = matches!(commit_policy, CreateCommitPolicy::ReplaceIfUnchanged(_));
    let requested_destination = canonical_requested_destination(destination)
        .map_err(|error| guarded_sfx_destination_error(destination, guarded_request, error))?;
    let destination = resolve_publish_destination(destination)
        .map_err(|error| guarded_sfx_destination_error(destination, guarded_request, error))?;
    let _lock = lock_destination(&destination)?;
    let recovered = recover_transaction(&destination, Some(staged), move_no_replace, sync)?;
    if !recovered.is_empty() {
        verify_bound_outputs(&destination, &recovered)?;
        return Err(recovered_outputs_pending_ack(&destination, &recovered));
    }
    let previous_guard = match commit_policy {
        CreateCommitPolicy::ReplaceIfUnchanged(guard) => Some(guard),
        CreateCommitPolicy::ReplaceExisting | CreateCommitPolicy::NoReplace => None,
    };
    let guarded_previous_digest = match previous_guard {
        Some(guard) => Some(verify_destination_guard(
            &requested_destination,
            artifact_kind(layout),
            guard,
        )?),
        None => None,
    };
    let exists = match validate_guarded_publish_destination(
        &destination,
        &requested_destination,
        layout,
        commit_policy_allows_replace(commit_policy),
        guarded_previous_digest,
    ) {
        Ok(exists) => exists,
        Err(error) => {
            let error =
                guarded_sfx_destination_error(&requested_destination, guarded_request, error);
            return Err(with_recovered_debt(error, &destination, &recovered));
        }
    };
    let replacement_digest = required_path_state_digest(
        staged,
        &destination,
        "the staged SFX replacement disappeared before publication",
    )?;
    if !exists {
        ensure_unjournaled_digest(
            staged,
            replacement_digest,
            &destination,
            "staged SFX replacement",
        )?;
        if let Err(error) = install_no_replace(staged, &destination, move_no_replace) {
            return Err(with_recovered_debt(error, &destination, &recovered));
        }
        ensure_unjournaled_digest(
            &destination,
            replacement_digest,
            &destination,
            "newly installed SFX output",
        )?;
        if let Err(error) = sync_rename_parents(staged, &destination, sync) {
            return Err(with_recovered_debt(
                recovery_error(
                    &destination,
                    vec![destination.clone()],
                    format!(
                        "the new SFX output was installed at {}, but its parent directory could not be synchronized: {error}",
                        destination.display()
                    ),
                ),
                &destination,
                &recovered,
            ));
        }
        ensure_staged_destination_identity(&destination, staged_identity, layout)?;
        ensure_unjournaled_digest(
            &destination,
            replacement_digest,
            &destination,
            "newly installed SFX output",
        )?;
        verify_bound_outputs(&destination, &recovered)?;
        return Ok(recovered.into_iter().map(|output| output.path).collect());
    }

    let previous_digest = match guarded_previous_digest {
        Some(digest) => digest,
        None => required_path_state_digest(
            &destination,
            &destination,
            "the existing SFX output disappeared before transaction publication",
        )?,
    };

    let (open, transaction) = match begin_transaction(
        &destination,
        &requested_destination,
        staged,
        staged_identity,
        layout,
        TransactionDigests {
            previous: previous_digest,
            replacement: replacement_digest,
        },
        sync,
    ) {
        Ok(transaction) => transaction,
        Err(error) => return Err(with_recovered_debt(error, &destination, &recovered)),
    };
    let current = match resume_transaction(
        &transaction,
        &open,
        None,
        previous_guard,
        move_no_replace,
        sync,
    ) {
        Ok(current) => current,
        Err(error) => {
            return Err(with_recovered_debt(
                with_transaction_details(error, &transaction),
                &destination,
                &recovered,
            ));
        }
    };
    if let Err(error) = clear_transaction(open, &transaction, sync) {
        return Err(with_recovered_debt(
            recovery_error(
                &destination,
                transaction_recovery_paths(&transaction),
                format!(
                    "the SFX output and previous-output backup are durable, but the transaction journal could not be cleared: {error}"
                ),
            ),
            &destination,
            &recovered,
        ));
    }
    let mut preserved = recovered;
    preserved.extend(current);
    preserved.sort();
    preserved.dedup();
    verify_bound_outputs(&destination, &preserved)?;
    Ok(preserved.into_iter().map(|output| output.path).collect())
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

fn recovered_outputs_pending_ack(destination: &Path, recovered: &[BoundOutput]) -> FormatError {
    let mut paths = vec![completion_path(destination)];
    for output in recovered {
        paths.push(parent_or_current(&output.path).to_path_buf());
        paths.push(output.path.clone());
    }
    recovery_error_without_staging(
        destination,
        paths,
        "the interrupted SFX replacement was completed and its previous output is retained; test the current output, delete the listed backup path when it is no longer needed, then try again"
            .into(),
    )
}

#[cfg(test)]
fn replace_staged_path_with<M, S>(
    staged: &Path,
    destination: &Path,
    layout: SfxLayout,
    overwrite: bool,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<Vec<PathBuf>, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    replace_bound_staged_path_with(
        staged,
        path_identity(staged)?,
        destination,
        layout,
        commit_policy_from_overwrite(overwrite),
        move_no_replace,
        sync,
    )
}

#[cfg(test)]
fn replace_staged_path_with_policy<M, S>(
    staged: &Path,
    destination: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<Vec<PathBuf>, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    replace_bound_staged_path_with(
        staged,
        path_identity(staged)?,
        destination,
        layout,
        commit_policy,
        move_no_replace,
        sync,
    )
}

fn begin_transaction<S>(
    destination: &Path,
    requested_destination: &Path,
    staged: &Path,
    staged_identity: PathIdentity,
    layout: SfxLayout,
    digests: TransactionDigests,
    sync: &mut S,
) -> Result<(OpenTransaction, ResolvedTransaction), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let parent = parent_or_current(destination);
    let staged_parent = fs::canonicalize(parent_or_current(staged))?;
    if staged_parent != parent {
        return Err(FormatError::Unsupported(
            "SFX staging and destination paths must share a directory".into(),
        ));
    }
    let destination_name = destination
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX destination has no file name".into()))?;
    let requested_name = requested_destination.file_name().ok_or_else(|| {
        FormatError::Unsupported("SFX requested destination has no file name".into())
    })?;
    if canonical_destination(requested_destination)? != destination {
        return Err(FormatError::Unsupported(
            "SFX requested destination no longer resolves to the locked output".into(),
        ));
    }
    let staged_name = staged
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX staging path has no file name".into()))?;
    if !staging_name_is_reserved(staged_name) {
        return Err(FormatError::Unsupported(
            "SFX staging path is outside the reserved transaction namespace".into(),
        ));
    }

    let previous_identity = path_identity(destination)?;
    let replacement_identity = path_identity(staged)?;
    if replacement_identity != staged_identity {
        return Err(FormatError::Io(io::Error::other(
            "SFX staging identity changed before transaction publication",
        )));
    }
    if previous_identity == replacement_identity {
        return Err(FormatError::Unsupported(
            "SFX staging and previous output identities must differ".into(),
        ));
    }
    verify_prejournal_digests(destination, requested_destination, staged, digests)?;
    let (holder, holder_identity) = reserve_holder(destination, sync)?;
    let transaction = ResolvedTransaction {
        layout,
        destination: destination.to_path_buf(),
        staged: staged.to_path_buf(),
        previous: holder.join("previous"),
        replacement: holder.join("replacement"),
        completion: parent.join(TRANSACTION_COMPLETION_NAME),
        journal: journal_path(destination)?,
        holder: holder.clone(),
        holder_identity,
        previous_identity,
        replacement_identity,
        previous_digest: digests.previous,
        replacement_digest: digests.replacement,
    };
    let record = TransactionRecord {
        version: TRANSACTION_VERSION,
        layout: layout.into(),
        destination: StoredOsString::from_os_str(destination_name)?,
        requested_destination: StoredOsString::from_os_str(requested_name)?,
        staged: StoredOsString::from_os_str(staged_name)?,
        holder: StoredOsString::from_os_str(holder.file_name().ok_or_else(|| {
            FormatError::Unsupported("SFX transaction holder has no file name".into())
        })?)?,
        holder_identity,
        previous_identity,
        replacement_identity,
        previous_digest: digests.previous,
        replacement_digest: digests.replacement,
    };
    if let Err(error) =
        verify_prejournal_digests(destination, requested_destination, staged, digests)
    {
        remove_empty_holder(&holder, holder_identity, sync);
        return Err(error);
    }
    match write_transaction(destination, record, sync) {
        Ok(open) => Ok((open, transaction)),
        Err(error) => {
            if sfx_recovery_details(&error).is_none() {
                remove_empty_holder(&holder, holder_identity, sync);
            }
            Err(error)
        }
    }
}

fn verify_prejournal_digests(
    destination: &Path,
    requested_destination: &Path,
    staged: &Path,
    digests: TransactionDigests,
) -> Result<(), FormatError> {
    match path_state_digest(destination) {
        Ok(Some(observed)) if observed == digests.previous => {}
        Ok(_) => {
            return Err(FormatError::destination_changed(
                requested_destination.to_path_buf(),
            ));
        }
        Err(error) if error.is_destination_changed() => {
            return Err(FormatError::destination_changed(
                requested_destination.to_path_buf(),
            ));
        }
        Err(error) => return Err(error),
    }
    match path_state_digest(staged) {
        Ok(Some(observed)) if observed == digests.replacement => Ok(()),
        Ok(_) => Err(FormatError::Io(io::Error::other(
            "SFX staging content changed before transaction publication",
        ))),
        Err(error) => Err(FormatError::Io(io::Error::other(format!(
            "SFX staging content could not be verified before transaction publication: {error}"
        )))),
    }
}

fn recover_transaction<M, S>(
    destination: &Path,
    active_staged: Option<&Path>,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<Vec<BoundOutput>, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    let Some(open) = open_transaction(destination)? else {
        return Ok(Vec::new());
    };
    let transaction = resolve_transaction(destination, &open.record).map_err(|error| {
        recovery_error(
            destination,
            vec![open.path.clone()],
            format!(
                "the SFX replacement journal at {} is invalid and was left untouched: {error}",
                open.path.display()
            ),
        )
    })?;
    let preserved = resume_transaction(
        &transaction,
        &open,
        active_staged,
        None,
        move_no_replace,
        sync,
    )
    .map_err(|error| with_transaction_details(error, &transaction))?;
    clear_transaction(open, &transaction, sync).map_err(|error| {
        recovery_error(
            destination,
            transaction_recovery_paths(&transaction),
            format!(
                "the interrupted SFX replacement was recovered, but its journal could not be cleared: {error}"
            ),
        )
    })?;
    verify_bound_outputs(destination, &preserved)?;
    Ok(preserved)
}

fn resume_transaction<M, S>(
    transaction: &ResolvedTransaction,
    open: &OpenTransaction,
    active_staged: Option<&Path>,
    previous_guard: Option<CreateDestinationGuard>,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<Vec<BoundOutput>, FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_open_transaction_binding(open)?;
    validate_holder(transaction)?;
    verify_reachable_transaction_digests(transaction)?;
    let staged = observed_identity(&transaction.staged)?;
    let replacement = observed_identity(&transaction.replacement)?;
    let destination = observed_identity(&transaction.destination)?;
    match (staged, replacement, destination) {
        (Some(staged), Some(replacement), _)
            if staged == transaction.replacement_identity
                && replacement == transaction.replacement_identity =>
        {
            remove_duplicate_source(
                transaction,
                open,
                &transaction.staged,
                &transaction.replacement,
                transaction.replacement_identity,
                transaction.replacement_digest,
                "staged replacement",
                sync,
            )?;
        }
        (Some(staged), None, Some(destination))
            if staged == transaction.replacement_identity
                && destination == transaction.replacement_identity =>
        {
            remove_duplicate_source(
                transaction,
                open,
                &transaction.staged,
                &transaction.destination,
                transaction.replacement_identity,
                transaction.replacement_digest,
                "staged replacement",
                sync,
            )?;
        }
        (Some(staged), None, _) if staged == transaction.replacement_identity => {
            move_bound(
                transaction,
                open,
                &transaction.staged,
                &transaction.replacement,
                transaction.replacement_identity,
                transaction.replacement_digest,
                "staged replacement",
                move_no_replace,
                sync,
            )?;
        }
        (staged, Some(replacement), _)
            if replacement == transaction.replacement_identity
                && staged_is_absent_or_active(
                    staged,
                    &transaction.staged,
                    transaction.replacement_identity,
                    active_staged,
                )? => {}
        (staged, None, Some(destination))
            if destination == transaction.replacement_identity
                && staged_is_absent_or_active(
                    staged,
                    &transaction.staged,
                    transaction.replacement_identity,
                    active_staged,
                )? => {}
        state => {
            return Err(transaction_conflict(
                transaction,
                format!("staged replacement state changed: {state:?}"),
            ));
        }
    }

    let destination = observed_identity(&transaction.destination)?;
    let previous = observed_identity(&transaction.previous)?;
    match (destination, previous) {
        (Some(destination), Some(previous))
            if destination == transaction.previous_identity
                && previous == transaction.previous_identity =>
        {
            remove_duplicate_source(
                transaction,
                open,
                &transaction.destination,
                &transaction.previous,
                transaction.previous_identity,
                transaction.previous_digest,
                "previous output",
                sync,
            )?;
        }
        (Some(destination), None) if destination == transaction.previous_identity => {
            move_bound(
                transaction,
                open,
                &transaction.destination,
                &transaction.previous,
                transaction.previous_identity,
                transaction.previous_digest,
                "previous output",
                move_no_replace,
                sync,
            )?;
        }
        (None, Some(previous)) if previous == transaction.previous_identity => {}
        (Some(destination), Some(previous))
            if destination == transaction.replacement_identity
                && previous == transaction.previous_identity => {}
        state => {
            return Err(transaction_conflict(
                transaction,
                format!("previous output state changed: {state:?}"),
            ));
        }
    }
    ensure_bound_state(
        &transaction.previous,
        transaction.previous_identity,
        transaction.previous_digest,
        transaction,
        "previous-output backup",
    )?;
    if let Some(guard) = previous_guard {
        verify_moved_path_state(guard, &transaction.previous, &transaction.destination).map_err(
            |error| {
                transaction_conflict(
                    transaction,
                    format!("previous-output backup content changed after it was moved: {error}"),
                )
            },
        )?;
    }

    let replacement = observed_identity(&transaction.replacement)?;
    let destination = observed_identity(&transaction.destination)?;
    match (replacement, destination) {
        (Some(replacement), Some(destination))
            if replacement == transaction.replacement_identity
                && destination == transaction.replacement_identity =>
        {
            remove_duplicate_source(
                transaction,
                open,
                &transaction.replacement,
                &transaction.destination,
                transaction.replacement_identity,
                transaction.replacement_digest,
                "new SFX output",
                sync,
            )?;
        }
        (Some(replacement), None) if replacement == transaction.replacement_identity => {
            move_bound(
                transaction,
                open,
                &transaction.replacement,
                &transaction.destination,
                transaction.replacement_identity,
                transaction.replacement_digest,
                "new SFX output",
                move_no_replace,
                sync,
            )?;
        }
        (None, Some(destination)) if destination == transaction.replacement_identity => {}
        state => {
            return Err(transaction_conflict(
                transaction,
                format!("new output state changed: {state:?}"),
            ));
        }
    }

    sync(parent_or_current(&transaction.destination))?;
    sync(&transaction.holder)?;
    ensure_bound_state(
        &transaction.destination,
        transaction.replacement_identity,
        transaction.replacement_digest,
        transaction,
        "new SFX output",
    )?;
    ensure_bound_state(
        &transaction.previous,
        transaction.previous_identity,
        transaction.previous_digest,
        transaction,
        "previous-output backup",
    )?;
    let staged = observed_identity(&transaction.staged)?;
    if !staged_is_absent_or_active(
        staged,
        &transaction.staged,
        transaction.replacement_identity,
        active_staged,
    )? {
        return Err(transaction_conflict(
            transaction,
            format!(
                "staging path is unexpectedly occupied at {}: {staged:?}",
                transaction.staged.display()
            ),
        ));
    }
    ensure_missing(
        &transaction.replacement,
        transaction,
        "transaction replacement path",
    )?;
    validate_holder(transaction)?;
    ensure_open_transaction_binding(open)?;
    Ok(vec![BoundOutput {
        path: transaction.previous.clone(),
        identity: transaction.previous_identity,
        digest: transaction.previous_digest,
    }])
}

fn staged_is_absent_or_active(
    observed: Option<PathIdentity>,
    transaction_staged: &Path,
    transaction_replacement_identity: PathIdentity,
    active_staged: Option<&Path>,
) -> Result<bool, FormatError> {
    let Some(observed) = observed else {
        return Ok(true);
    };
    let Some(active_staged) = active_staged else {
        return Ok(false);
    };
    if !crate::same_path_entry(transaction_staged, active_staged)
        || observed == transaction_replacement_identity
    {
        return Ok(false);
    }
    Ok(path_identity(active_staged)? == observed)
}

#[allow(clippy::too_many_arguments)]
fn move_bound<M, S>(
    transaction: &ResolvedTransaction,
    open: &OpenTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    digest: [u8; 32],
    role: &str,
    move_no_replace: &mut M,
    sync: &mut S,
) -> Result<(), FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_bound_state(source, identity, digest, transaction, role)?;
    ensure_missing(destination, transaction, role)?;
    ensure_open_transaction_binding(open)?;
    move_no_replace(source, destination).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "could not move {role} from {} to {} without replacement: {error}",
                source.display(),
                destination.display()
            ),
        )
    })?;
    ensure_bound_state(destination, identity, digest, transaction, role)?;
    sync_rename_parents(source, destination, sync).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "could not durably record the {role} move from {} to {}: {error}",
                source.display(),
                destination.display()
            ),
        )
    })?;
    ensure_bound_state(destination, identity, digest, transaction, role)?;
    ensure_missing(source, transaction, role)
}

#[allow(clippy::too_many_arguments)]
fn remove_duplicate_source<S>(
    transaction: &ResolvedTransaction,
    open: &OpenTransaction,
    source: &Path,
    destination: &Path,
    identity: PathIdentity,
    digest: [u8; 32],
    role: &str,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_bound_state(source, identity, digest, transaction, role)?;
    ensure_bound_state(destination, identity, digest, transaction, role)?;
    if transaction.layout != SfxLayout::SingleFile {
        return Err(transaction_conflict(
            transaction,
            format!(
                "the {role} appears at both {} and {} with one directory identity; automatic deduplication is unsafe",
                source.display(),
                destination.display()
            ),
        ));
    }
    for path in [source, destination] {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(transaction_conflict(
                transaction,
                format!(
                    "the duplicated {role} is not a regular file at {}",
                    path.display()
                ),
            ));
        }
    }
    sync(parent_or_current(destination)).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "could not synchronize the durable duplicate {role} at {} before removing its source alias: {error}",
                destination.display()
            ),
        )
    })?;
    ensure_open_transaction_binding(open)?;
    if let Err(error) = remove_bound_path_via_quarantine(
        source,
        identity,
        SfxLayout::SingleFile,
        &transaction.destination,
        role,
        sync,
    ) {
        let mut paths = transaction_recovery_paths(transaction);
        if let Some(details) = sfx_recovery_details(&error) {
            paths.extend(details.paths);
        }
        return Err(recovery_error(
            &transaction.destination,
            paths,
            format!("SFX replacement requires manual recovery: {error}"),
        ));
    }
    ensure_missing(source, transaction, role)?;
    ensure_bound_state(destination, identity, digest, transaction, role)
}

fn install_no_replace<M>(
    staged: &Path,
    destination: &Path,
    move_no_replace: &mut M,
) -> Result<(), FormatError>
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
{
    match move_no_replace(staged, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(output_exists_error(destination))
        }
        Err(error) => Err(error.into()),
    }
}

fn transaction_conflict(transaction: &ResolvedTransaction, reason: String) -> FormatError {
    recovery_error(
        &transaction.destination,
        transaction_recovery_paths(transaction),
        format!(
            "SFX replacement requires manual recovery: {reason}; no existing destination was overwritten. Every related path observed by the transaction is listed for inspection: target {}, previous-output backup {}, replacement {}, staging {}, and journal {}",
            transaction.destination.display(),
            transaction.previous.display(),
            transaction.replacement.display(),
            transaction.staged.display(),
            transaction.journal.display()
        ),
    )
}

fn transaction_recovery_paths(transaction: &ResolvedTransaction) -> Vec<PathBuf> {
    let mut paths = vec![transaction.journal.clone()];
    for path in [
        &transaction.holder,
        &transaction.staged,
        &transaction.previous,
        &transaction.replacement,
        &transaction.completion,
    ] {
        match fs::symlink_metadata(path) {
            Ok(_) => paths.push(path.clone()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => paths.push(path.clone()),
        }
    }
    paths
}

fn with_transaction_details(error: FormatError, transaction: &ResolvedTransaction) -> FormatError {
    if sfx_recovery_details(&error).is_some() {
        error
    } else {
        transaction_conflict(transaction, error.to_string())
    }
}

fn with_recovered_debt(
    error: FormatError,
    destination: &Path,
    recovered: &[BoundOutput],
) -> FormatError {
    if recovered.is_empty() {
        return error;
    }
    let mut paths = recovered
        .iter()
        .map(|output| output.path.clone())
        .collect::<Vec<_>>();
    if let Some(details) = sfx_recovery_details(&error) {
        paths.extend(details.paths);
    }
    recovery_error(
        destination,
        paths,
        format!(
            "{error}; an interrupted SFX replacement was recovered and its previous output remains at: {}",
            recovered
                .iter()
                .map(|output| output.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

fn verify_bound_outputs(destination: &Path, outputs: &[BoundOutput]) -> Result<(), FormatError> {
    for output in outputs {
        match observed_identity(&output.path) {
            Ok(Some(identity)) if identity == output.identity => {
                match path_state_digest(&output.path) {
                    Ok(Some(observed)) if observed == output.digest => {}
                    Ok(_) => {
                        return Err(bound_output_conflict(
                            destination,
                            outputs,
                            format!(
                                "preserved SFX output content changed at {}",
                                output.path.display()
                            ),
                        ));
                    }
                    Err(error) => {
                        return Err(bound_output_conflict(
                            destination,
                            outputs,
                            format!(
                                "preserved SFX output content could not be verified at {}: {error}",
                                output.path.display()
                            ),
                        ));
                    }
                }
            }
            Ok(observed) => {
                return Err(bound_output_conflict(
                    destination,
                    outputs,
                    format!(
                        "preserved SFX output identity changed at {}: expected {:?}, observed {observed:?}",
                        output.path.display(),
                        output.identity
                    ),
                ));
            }
            Err(error) => {
                return Err(bound_output_conflict(
                    destination,
                    outputs,
                    format!(
                        "preserved SFX output could not be verified at {}: {error}",
                        output.path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn bound_output_conflict(
    destination: &Path,
    outputs: &[BoundOutput],
    reason: String,
) -> FormatError {
    let mut paths = Vec::new();
    for output in outputs {
        for path in [
            output.path.clone(),
            parent_or_current(parent_or_current(&output.path)).join(TRANSACTION_COMPLETION_NAME),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(_) => paths.push(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => paths.push(path),
            }
        }
    }
    recovery_error(
        destination,
        paths,
        format!("SFX replacement requires manual recovery: {reason}"),
    )
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

fn write_transaction<S>(
    destination: &Path,
    record: TransactionRecord,
    sync: &mut S,
) -> Result<OpenTransaction, FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let path = journal_path(destination)?;
    let parent = parent_or_current(&path);
    let holder = parent.join(checked_component(&record.holder)?);
    match fs::symlink_metadata(&path) {
        Ok(_) => return Err(output_exists_error(&path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = serde_json::to_vec(&record)
        .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    if bytes.len() > TRANSACTION_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "SFX transaction journal exceeds {TRANSACTION_MAX_BYTES} bytes"
        )));
    }
    let temp = parent.join(format!(
        ".squallz-sfx-journal-{}-{}.tmp",
        std::process::id(),
        TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

        options.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let identity = file_identity(&file).map_err(|error| {
        recovery_error(
            destination,
            vec![temp.clone(), holder.clone()],
            format!(
                "the SFX transaction journal temp file could not be bound and was left for recovery: {error}"
            ),
        )
    })?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        return Err(cleanup_unpublished_record_temp(
            error.into(),
            &temp,
            &file,
            identity,
            destination,
            std::slice::from_ref(&holder),
            true,
        ));
    }
    if let Err(error) = crate::move_path_no_replace(&temp, &path) {
        return Err(cleanup_unpublished_record_temp(
            error.into(),
            &temp,
            &file,
            identity,
            destination,
            std::slice::from_ref(&holder),
            true,
        ));
    }
    if let Err(error) = sync(parent) {
        return Err(recovery_error(
            destination,
            vec![path.clone(), holder.clone()],
            format!(
                "the SFX replacement journal was published at {}, but its parent directory could not be synchronized: {error}",
                path.display()
            ),
        ));
    }
    if path_identity(&path).ok() != Some(identity) || file_identity(&file).ok() != Some(identity) {
        return Err(recovery_error(
            destination,
            vec![path.clone(), holder],
            format!(
                "the SFX replacement journal changed during publication: {}",
                path.display()
            ),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut published = Vec::new();
    Read::by_ref(&mut file)
        .take((TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut published)?;
    if published != bytes {
        return Err(recovery_error(
            destination,
            vec![path.clone(), holder],
            format!(
                "the SFX replacement journal contents changed during publication: {}",
                path.display()
            ),
        ));
    }
    Ok(OpenTransaction {
        path,
        file,
        identity,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    })
}

fn open_transaction(destination: &Path) -> Result<Option<OpenTransaction>, FormatError> {
    let path = journal_path(destination)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => read_transaction_file(&path, destination, "replacement journal").map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn ensure_open_transaction_binding(open: &OpenTransaction) -> Result<(), FormatError> {
    if bound_record_content_is_current(&open.path, &open.file, open.identity, open.content_digest)?
    {
        return Ok(());
    }
    Err(FormatError::Io(io::Error::other(format!(
        "SFX transaction record identity or contents changed at {}; the record was left untouched",
        open.path.display()
    ))))
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
        .take((TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > TRANSACTION_MAX_BYTES
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

fn read_transaction_file(
    path: &Path,
    recovery_target: &Path,
    description: &str,
) -> Result<OpenTransaction, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(recovery_error(
            recovery_target,
            vec![path.to_path_buf()],
            format!(
                "SFX {description} must be a regular file: {}",
                path.display()
            ),
        ));
    }
    let mut file = open_journal_file(path)?;
    let identity = file_identity(&file)?;
    if path_identity(path)? != identity {
        return Err(recovery_error(
            recovery_target,
            vec![path.to_path_buf()],
            format!("SFX {description} changed while it was opened"),
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > TRANSACTION_MAX_BYTES {
        return Err(recovery_error(
            recovery_target,
            vec![path.to_path_buf()],
            format!(
                "SFX {description} exceeds {TRANSACTION_MAX_BYTES} bytes and was left untouched"
            ),
        ));
    }
    let record = serde_json::from_slice(&bytes).map_err(|error| {
        recovery_error(
            recovery_target,
            vec![path.to_path_buf()],
            format!("SFX {description} is invalid and was left untouched: {error}"),
        )
    })?;
    let open = OpenTransaction {
        path: path.to_path_buf(),
        file,
        identity,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    };
    ensure_open_transaction_binding(&open)?;
    Ok(open)
}

fn resolve_transaction(
    destination: &Path,
    record: &TransactionRecord,
) -> Result<ResolvedTransaction, FormatError> {
    let transaction = resolve_transaction_paths(destination, record)?;
    validate_holder(&transaction)?;
    Ok(transaction)
}

fn resolve_transaction_paths(
    destination: &Path,
    record: &TransactionRecord,
) -> Result<ResolvedTransaction, FormatError> {
    if record.version != TRANSACTION_VERSION {
        return Err(FormatError::Unsupported(format!(
            "unsupported SFX transaction journal version: {}",
            record.version
        )));
    }
    let destination_name = checked_component(&record.destination)?;
    if destination.file_name() != Some(destination_name.as_os_str()) {
        return Err(FormatError::Unsupported(
            "SFX transaction journal belongs to another destination".into(),
        ));
    }
    let layout = match record.layout {
        JournalLayout::SingleFile => SfxLayout::SingleFile,
        JournalLayout::MacosApp => SfxLayout::MacosApp,
    };
    let staged_name = checked_component(&record.staged)?;
    checked_component(&record.requested_destination)?;
    if !staging_name_is_reserved(&staged_name) {
        return Err(FormatError::Unsupported(
            "SFX transaction journal contains an invalid staging path".into(),
        ));
    }
    let holder_name = checked_component(&record.holder)?;
    let holder_name_text = holder_name.to_str().ok_or_else(|| {
        FormatError::Unsupported("SFX transaction holder name must be UTF-8".into())
    })?;
    if !holder_name_is_reserved(holder_name_text) {
        return Err(FormatError::Unsupported(
            "SFX transaction journal contains an invalid holder path".into(),
        ));
    }
    if record.holder_identity == record.previous_identity
        || record.holder_identity == record.replacement_identity
        || record.previous_identity == record.replacement_identity
    {
        return Err(FormatError::Unsupported(
            "SFX transaction journal contains ambiguous identities".into(),
        ));
    }
    let parent = parent_or_current(destination);
    let holder = parent.join(holder_name);
    let transaction = ResolvedTransaction {
        layout,
        destination: destination.to_path_buf(),
        staged: parent.join(staged_name),
        previous: holder.join("previous"),
        replacement: holder.join("replacement"),
        completion: parent.join(TRANSACTION_COMPLETION_NAME),
        journal: journal_path(destination)?,
        holder,
        holder_identity: record.holder_identity,
        previous_identity: record.previous_identity,
        replacement_identity: record.replacement_identity,
        previous_digest: record.previous_digest,
        replacement_digest: record.replacement_digest,
    };
    Ok(transaction)
}

fn clear_transaction<S>(
    mut open: OpenTransaction,
    transaction: &ResolvedTransaction,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_open_transaction_binding(&open)?;
    verify_final_transaction_outputs(transaction)?;
    ensure_missing(
        &transaction.completion,
        transaction,
        "completed transaction index",
    )?;
    ensure_open_transaction_binding(&open)?;
    let journal = open.path.clone();
    crate::move_path_no_replace(&journal, &transaction.completion).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "could not publish the completed transaction index at {}: {error}",
                transaction.completion.display()
            ),
        )
    })?;
    open.path = transaction.completion.clone();
    sync_rename_parents(&journal, &transaction.completion, sync).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "could not durably publish the completed transaction index at {}: {error}",
                transaction.completion.display()
            ),
        )
    })?;
    ensure_open_transaction_binding(&open).map_err(|error| {
        transaction_conflict(
            transaction,
            format!("completed transaction index changed after publication: {error}"),
        )
    })?;
    verify_final_transaction_outputs(transaction)?;
    ensure_open_transaction_binding(&open)
}

fn verify_final_transaction_outputs(transaction: &ResolvedTransaction) -> Result<(), FormatError> {
    ensure_bound_state(
        &transaction.destination,
        transaction.replacement_identity,
        transaction.replacement_digest,
        transaction,
        "new SFX output",
    )?;
    ensure_bound_state(
        &transaction.previous,
        transaction.previous_identity,
        transaction.previous_digest,
        transaction,
        "previous-output backup",
    )
}

fn validate_holder(transaction: &ResolvedTransaction) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(&transaction.holder)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || path_identity(&transaction.holder)? != transaction.holder_identity
    {
        return Err(transaction_conflict(
            transaction,
            "transaction holder identity or type changed".into(),
        ));
    }
    let mut count = 0usize;
    for entry in fs::read_dir(&transaction.holder)? {
        let entry = entry?;
        let name = entry.file_name();
        if name != OsStr::new("previous") && name != OsStr::new("replacement") {
            return Err(transaction_conflict(
                transaction,
                format!(
                    "transaction holder contains an unexpected entry: {}",
                    entry.path().display()
                ),
            ));
        }
        count += 1;
        if count > 2 {
            return Err(transaction_conflict(
                transaction,
                "transaction holder contains too many entries".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_identity(
    path: &Path,
    expected: PathIdentity,
    transaction: &ResolvedTransaction,
    role: &str,
) -> Result<(), FormatError> {
    match observed_identity(path)? {
        Some(identity) if identity == expected => Ok(()),
        Some(identity) => Err(transaction_conflict(
            transaction,
            format!(
                "{role} identity changed at {}: {identity:?}",
                path.display()
            ),
        )),
        None => Err(transaction_conflict(
            transaction,
            format!("{role} is missing at {}", path.display()),
        )),
    }
}

fn ensure_bound_state(
    path: &Path,
    identity: PathIdentity,
    digest: [u8; 32],
    transaction: &ResolvedTransaction,
    role: &str,
) -> Result<(), FormatError> {
    ensure_identity(path, identity, transaction, role)?;
    let observed = path_state_digest(path).map_err(|error| {
        transaction_conflict(
            transaction,
            format!(
                "{role} content could not be verified at {}: {error}",
                path.display()
            ),
        )
    })?;
    if observed == Some(digest) {
        return Ok(());
    }
    Err(transaction_conflict(
        transaction,
        format!("{role} content changed at {}", path.display()),
    ))
}

fn verify_reachable_transaction_digests(
    transaction: &ResolvedTransaction,
) -> Result<(), FormatError> {
    for (path, identity, digest, role) in [
        (
            &transaction.staged,
            transaction.replacement_identity,
            transaction.replacement_digest,
            "staged replacement",
        ),
        (
            &transaction.replacement,
            transaction.replacement_identity,
            transaction.replacement_digest,
            "transaction replacement",
        ),
        (
            &transaction.destination,
            transaction.replacement_identity,
            transaction.replacement_digest,
            "new SFX output",
        ),
        (
            &transaction.destination,
            transaction.previous_identity,
            transaction.previous_digest,
            "previous SFX output",
        ),
        (
            &transaction.previous,
            transaction.previous_identity,
            transaction.previous_digest,
            "previous-output backup",
        ),
    ] {
        if observed_identity(path)? == Some(identity) {
            ensure_bound_state(path, identity, digest, transaction, role)?;
        }
    }
    Ok(())
}

fn ensure_missing(
    path: &Path,
    transaction: &ResolvedTransaction,
    role: &str,
) -> Result<(), FormatError> {
    match observed_identity(path)? {
        None => Ok(()),
        Some(identity) => Err(transaction_conflict(
            transaction,
            format!(
                "{role} is unexpectedly occupied at {}: {identity:?}",
                path.display()
            ),
        )),
    }
}

fn observed_identity(path: &Path) -> Result<Option<PathIdentity>, FormatError> {
    match path_identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn reserve_holder<S>(
    destination: &Path,
    sync: &mut S,
) -> Result<(PathBuf, PathIdentity), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let parent = parent_or_current(destination);
    let prefix = ".squallz-sfx-holder-";
    for _ in 0..1000u32 {
        let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let holder = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
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
                sync(parent)?;
                return Ok((holder, identity));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve an SFX transaction holder next to {}",
        destination.display()
    )))
}

fn remove_empty_holder<S>(holder: &Path, identity: PathIdentity, sync: &mut S)
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let _ = remove_bound_path_via_quarantine(
        holder,
        identity,
        SfxLayout::MacosApp,
        holder,
        "the unused SFX transaction holder",
        sync,
    );
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
        (CleanupKind::Stage, _) => staging_name_is_reserved(name),
        (CleanupKind::Payload, JournalLayout::SingleFile) => payload_name_is_reserved(name),
        (CleanupKind::Payload, JournalLayout::MacosApp) => false,
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

fn canonical_requested_destination(destination: &Path) -> Result<PathBuf, FormatError> {
    let name = destination
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX destination has no file name".into()))?;
    Ok(fs::canonicalize(parent_or_current(destination))?.join(name))
}

fn canonical_destination(destination: &Path) -> Result<PathBuf, FormatError> {
    let name = destination
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX destination has no file name".into()))?;
    let parent = fs::canonicalize(parent_or_current(destination))?;
    let requested = parent.join(name);
    match fs::symlink_metadata(&requested) {
        Ok(metadata) if !metadata.file_type().is_symlink() => {
            // realpath/canonicalize returns the directory entry's spelling on
            // case-insensitive filesystems. That makes aliases share one key.
            Ok(fs::canonicalize(requested)?)
        }
        Ok(_) => Ok(requested),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(requested),
        Err(error) => Err(error.into()),
    }
}

fn reconcile_completed_transaction<S>(
    requested_destination: &Path,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    let requested = canonical_requested_destination(requested_destination)?;
    let completion = completion_path(&requested);
    let open = match fs::symlink_metadata(&completion) {
        Ok(_) => read_transaction_file(&completion, &requested, "completed transaction index")
            .map_err(|error| {
                recovery_error_without_staging(
                    &requested,
                    vec![completion.clone()],
                    error.to_string(),
                )
            })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let destination_name = checked_component(&open.record.destination).map_err(|error| {
        recovery_error_without_staging(
            &requested,
            vec![completion.clone()],
            format!("completed SFX transaction has an invalid destination: {error}"),
        )
    })?;
    let destination = parent_or_current(&requested).join(destination_name);
    let transaction = resolve_transaction_paths(&destination, &open.record).map_err(|error| {
        recovery_error_without_staging(
            &destination,
            vec![completion.clone()],
            format!("completed SFX transaction is invalid and was left untouched: {error}"),
        )
    })?;

    let installed = observed_identity(&transaction.destination)?;
    if installed != Some(transaction.replacement_identity) {
        let mut paths = vec![
            completion.clone(),
            transaction.holder.clone(),
            transaction.previous.clone(),
        ];
        if installed.is_some() {
            paths.push(transaction.destination.clone());
        }
        return Err(recovery_error_without_staging(
            &destination,
            paths,
            format!(
                "the completed SFX destination is missing or changed at {}; do not delete the retained previous output",
                transaction.destination.display()
            ),
        ));
    }
    verify_completed_path_digest(
        &transaction,
        &transaction.destination,
        transaction.replacement_digest,
        "completed SFX destination",
    )?;

    let holder_metadata = match fs::symlink_metadata(&transaction.holder) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let Some(holder_metadata) = holder_metadata else {
        return remove_completed_index(open, &destination, sync);
    };
    if !holder_metadata.is_dir()
        || holder_metadata.file_type().is_symlink()
        || path_identity(&transaction.holder)? != transaction.holder_identity
    {
        return Err(recovery_error_without_staging(
            &destination,
            vec![completion, transaction.holder],
            "completed SFX holder identity or type changed; inspect it before trying again".into(),
        ));
    }

    let previous = observed_identity(&transaction.previous)?;
    if let Some(identity) = previous {
        let reason = if identity == transaction.previous_identity {
            verify_completed_path_digest(
                &transaction,
                &transaction.previous,
                transaction.previous_digest,
                "completed SFX backup",
            )?;
            format!(
                "the previous SFX output is still retained at {}; test the current output, delete that backup path when it is no longer needed, then try again",
                transaction.previous.display()
            )
        } else {
            format!(
                "the completed SFX backup identity changed at {}; inspect it before trying again",
                transaction.previous.display()
            )
        };
        return Err(recovery_error_without_staging(
            &destination,
            vec![completion, transaction.holder, transaction.previous],
            reason,
        ));
    }
    let mut entries = fs::read_dir(&transaction.holder)?;
    if let Some(entry) = entries.next() {
        let entry = entry?;
        return Err(recovery_error_without_staging(
            &destination,
            vec![completion, transaction.holder, entry.path()],
            "the completed SFX holder contains an unexpected path; inspect it before trying again"
                .into(),
        ));
    }
    if path_identity(&transaction.holder)? != transaction.holder_identity {
        return Err(recovery_error_without_staging(
            &destination,
            vec![completion, transaction.holder],
            "the completed SFX holder changed before cleanup".into(),
        ));
    }
    ensure_open_transaction_binding(&open).map_err(|error| {
        recovery_error_without_staging(
            &destination,
            current_paths([&completion, &transaction.holder]),
            format!("the completed SFX index changed before holder cleanup: {error}"),
        )
    })?;
    remove_bound_path_via_quarantine(
        &transaction.holder,
        transaction.holder_identity,
        SfxLayout::MacosApp,
        &destination,
        "the empty completed SFX holder",
        sync,
    )?;
    remove_completed_index(open, &destination, sync)
}

fn verify_completed_path_digest(
    transaction: &ResolvedTransaction,
    path: &Path,
    expected: [u8; 32],
    role: &str,
) -> Result<(), FormatError> {
    let observed = path_state_digest(path).map_err(|error| {
        recovery_error_without_staging(
            &transaction.destination,
            current_paths([&transaction.completion, &transaction.holder, path]),
            format!(
                "the {role} could not be content-verified at {}: {error}",
                path.display()
            ),
        )
    })?;
    if observed == Some(expected) {
        return Ok(());
    }
    Err(recovery_error_without_staging(
        &transaction.destination,
        current_paths([&transaction.completion, &transaction.holder, path]),
        format!(
            "the {role} content changed at {}; transaction cleanup was left pending",
            path.display()
        ),
    ))
}

fn remove_completed_index<S>(
    open: OpenTransaction,
    destination: &Path,
    sync: &mut S,
) -> Result<(), FormatError>
where
    S: FnMut(&Path) -> io::Result<()>,
{
    ensure_open_transaction_binding(&open).map_err(|error| {
        recovery_error_without_staging(
            destination,
            vec![open.path.clone()],
            format!("the completed SFX index changed before cleanup: {error}"),
        )
    })?;
    remove_bound_path_via_quarantine(
        &open.path,
        open.identity,
        SfxLayout::SingleFile,
        destination,
        "the completed SFX index",
        sync,
    )
}

fn resolve_publish_destination(destination: &Path) -> Result<PathBuf, FormatError> {
    let requested_name = destination
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("SFX destination has no file name".into()))?;
    let requested = canonical_destination(destination)?;
    let Some(open) = open_transaction(&requested)? else {
        return Ok(requested);
    };
    let journal = open.path.clone();
    let record = open.record;
    drop(open.file);
    let recorded_name = checked_component(&record.destination).map_err(|error| {
        recovery_error(
            &requested,
            vec![journal.clone()],
            format!(
                "the directory SFX transaction journal at {} has an invalid destination and was left untouched: {error}",
                journal.display()
            ),
        )
    })?;
    let recorded = parent_or_current(&requested).join(&recorded_name);
    let requested_alias = checked_component(&record.requested_destination).map_err(|error| {
        recovery_error(
            &requested,
            vec![journal.clone()],
            format!(
                "the directory SFX transaction journal at {} has an invalid requested destination and was left untouched: {error}",
                journal.display()
            ),
        )
    })?;
    let requested_matches_record = requested.file_name() == Some(recorded_name.as_os_str())
        || requested_name == recorded_name.as_os_str()
        || requested_name == requested_alias.as_os_str();
    if requested_matches_record {
        return Ok(recorded);
    }

    let paths = resolve_transaction(&recorded, &record)
        .map(|transaction| transaction_recovery_paths(&transaction))
        .unwrap_or_else(|_| vec![journal]);
    Err(recovery_error(
        &recorded,
        paths,
        format!(
            "an unfinished SFX replacement for {} already owns this directory; recover that exact target before publishing {}",
            recorded.display(),
            requested.display()
        ),
    ))
}

fn destination_key(destination: &Path) -> Result<String, FormatError> {
    let canonical = canonical_destination(destination)?;
    path_key(b"squallz-sfx-destination-v1\0", &canonical)
}

fn destination_directory_key(destination: &Path) -> Result<String, FormatError> {
    let parent = fs::canonicalize(parent_or_current(destination))?;
    path_key(b"squallz-sfx-directory-v1\0", &parent)
}

fn path_key(domain: &[u8], canonical: &Path) -> Result<String, FormatError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        hasher.update(canonical.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        for unit in canonical.as_os_str().encode_wide() {
            hasher.update(&unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let path = canonical.to_str().ok_or_else(|| {
            FormatError::Unsupported("SFX transaction paths must be UTF-8 on this platform".into())
        })?;
        hasher.update(path.as_bytes());
    }
    Ok(hasher.finalize().to_string())
}

fn journal_path(destination: &Path) -> Result<PathBuf, FormatError> {
    Ok(parent_or_current(destination).join(TRANSACTION_JOURNAL_NAME))
}

fn completion_path(destination: &Path) -> PathBuf {
    parent_or_current(destination).join(TRANSACTION_COMPLETION_NAME)
}

fn holder_name_is_reserved(name: &str) -> bool {
    name.strip_prefix(".squallz-sfx-holder-")
        .is_some_and(is_canonical_process_sequence)
}

fn lock_destination(destination: &Path) -> Result<File, FormatError> {
    let path = std::env::temp_dir().join(format!(
        "squallz-sfx-transaction-{}.lock",
        destination_key(destination)?
    ));
    lock_file(&path, "transaction")
}

fn lock_destination_directory(destination: &Path) -> Result<File, FormatError> {
    let path = std::env::temp_dir().join(format!(
        "squallz-sfx-directory-{}.lock",
        destination_directory_key(destination)?
    ));
    lock_file(&path, "directory coordination")
}

fn lock_file(path: &Path, purpose: &str) -> Result<File, FormatError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(FormatError::Io(io::Error::other(format!(
                "SFX {purpose} lock must be a regular file: {}",
                path.display()
            ))));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = options.open(path)?;
    fs4::FileExt::lock(&file)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || path_identity(path)? != file_identity(&file)?
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "SFX {purpose} lock changed or became unsafe while it was opened"
        ))));
    }
    Ok(file)
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use squallz_format_api::{
        ArchiveFormat, ArchiveReader, ArchiveWriter, ControlToken, CreateOptions, EntryMeta,
        EntryPath, FormatCapabilities, FormatRegistry, NoProgress,
        OpenOptions as ArchiveOpenOptions, ReadSeek, TestSummary, WriteSeek,
    };

    use super::*;

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

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            Err(FormatError::Unsupported(
                "test ZIP reader has no materialized entries".into(),
            ))
        }

        fn test_summary(
            &mut self,
            _progress: &dyn squallz_format_api::ProgressSink,
            _control: &ControlToken,
        ) -> Result<TestSummary, FormatError> {
            Ok(TestSummary::default())
        }
    }

    fn test_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "squallz-sfx-transaction-{tag}-{}-{}",
            std::process::id(),
            TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn staged_file(destination: &Path, bytes: &[u8]) -> PathBuf {
        let (staged, _) = reserve_staged_path(destination, SfxLayout::SingleFile).unwrap();
        fs::write(&staged, bytes).unwrap();
        staged
    }

    fn staged_app(destination: &Path, relative: &Path, bytes: &[u8]) -> PathBuf {
        let (staged, _) = reserve_staged_path(destination, SfxLayout::MacosApp).unwrap();
        let path = staged.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        staged
    }

    #[cfg(unix)]
    fn begin_test_transaction(
        tag: &str,
    ) -> (
        PathBuf,
        PathBuf,
        PathBuf,
        OpenTransaction,
        ResolvedTransaction,
    ) {
        let dir = test_dir(tag);
        fs::create_dir(&dir).unwrap();
        let dir = fs::canonicalize(dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous output").unwrap();
        let staged = staged_file(&destination, b"replacement output");
        let staged_identity = path_identity(&staged).unwrap();
        let digests = TransactionDigests {
            previous: path_state_digest(&destination).unwrap().unwrap(),
            replacement: path_state_digest(&staged).unwrap().unwrap(),
        };
        let (open, transaction) = begin_transaction(
            &destination,
            &destination,
            &staged,
            staged_identity,
            SfxLayout::SingleFile,
            digests,
            &mut sync_directory,
        )
        .unwrap();
        (dir, destination, staged, open, transaction)
    }

    #[cfg(unix)]
    fn rewrite_record_in_place(open: &OpenTransaction) {
        let identity = path_identity(&open.path).unwrap();
        let mut bytes = fs::read(&open.path).unwrap();
        bytes[0] ^= 1;
        let mut writer = OpenOptions::new().write(true).open(&open.path).unwrap();
        writer.seek(SeekFrom::Start(0)).unwrap();
        writer.write_all(&bytes).unwrap();
        writer.sync_all().unwrap();
        assert_eq!(path_identity(&open.path).unwrap(), identity);
    }

    fn acknowledge_completed_backup(error: &FormatError, destination: &Path) -> Vec<u8> {
        assert!(!sfx_recovery_requires_staging(error));
        let details = sfx_recovery_details(error).unwrap();
        let completion = completion_path(destination);
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &completion)));
        let previous = details
            .paths
            .iter()
            .find(|path| path.file_name() == Some(OsStr::new("previous")) && path.is_file())
            .cloned()
            .unwrap();
        let bytes = fs::read(&previous).unwrap();
        fs::remove_file(previous).unwrap();
        preflight_destination(destination).unwrap();
        assert!(!completion.exists());
        bytes
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

    #[cfg(unix)]
    #[test]
    fn same_inode_journal_rewrite_blocks_every_transaction_move() {
        let (dir, destination, staged, open, transaction) =
            begin_test_transaction("same-inode-journal-before-resume");
        rewrite_record_in_place(&open);
        let mut moves = 0usize;

        let error = resume_transaction(
            &transaction,
            &open,
            None,
            None,
            &mut |from, to| {
                moves += 1;
                crate::move_path_no_replace(from, to)
            },
            &mut sync_directory,
        )
        .unwrap_err();

        assert_eq!(moves, 0);
        assert!(error.to_string().contains("contents changed"));
        assert_eq!(fs::read(&destination).unwrap(), b"previous output");
        assert_eq!(fs::read(&staged).unwrap(), b"replacement output");
        assert!(open.path.exists());
        assert!(transaction.holder.exists());
        cleanup(&dir, &destination);
    }

    #[cfg(unix)]
    #[test]
    fn same_inode_journal_rewrite_blocks_completion_publication() {
        let (dir, destination, _staged, open, transaction) =
            begin_test_transaction("same-inode-journal-before-clear");
        resume_transaction(
            &transaction,
            &open,
            None,
            None,
            &mut |from, to| crate::move_path_no_replace(from, to),
            &mut sync_directory,
        )
        .unwrap();
        rewrite_record_in_place(&open);

        let error = clear_transaction(open, &transaction, &mut sync_directory).unwrap_err();

        assert!(error.to_string().contains("contents changed"));
        assert!(transaction.journal.exists());
        assert!(!transaction.completion.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"replacement output");
        assert_eq!(fs::read(&transaction.previous).unwrap(), b"previous output");
        assert!(transaction.holder.exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn guarded_single_file_publish_rejects_same_length_content_changes_before_any_move() {
        let dir = test_dir("guarded-single-content-change");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"old-data").unwrap();
        let guard =
            crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
                .unwrap()
                .guard
                .unwrap();
        fs::write(&destination, b"new-data").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let mut moves = 0usize;

        let error = replace_staged_path_with_policy(
            &staged,
            &destination,
            SfxLayout::SingleFile,
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
            &mut |from, to| {
                moves += 1;
                crate::move_path_no_replace(from, to)
            },
            &mut sync_directory,
        )
        .unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(moves, 0);
        assert_eq!(fs::read(&destination).unwrap(), b"new-data");
        assert_eq!(fs::read(&staged).unwrap(), b"replacement");
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn guarded_publish_rejects_a_destination_removed_after_guard_verification() {
        let dir = test_dir("guarded-destination-removed-after-verification");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"old-data").unwrap();
        let guard =
            crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
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
        cleanup(&dir, &destination);
    }

    #[test]
    fn guarded_single_file_publish_maps_a_late_directory_to_destination_changed() {
        let dir = test_dir("guarded-single-file-late-directory");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"old-data").unwrap();
        let guard =
            crate::inspect_create_destination(&destination, CreateArtifactKind::SfxSingleFile)
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
        cleanup(&dir, &destination);
    }

    #[test]
    fn guarded_app_publish_maps_a_late_file_to_destination_changed() {
        let dir = test_dir("guarded-app-late-file");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("Package.app");
        fs::create_dir(&destination).unwrap();
        let guard =
            crate::inspect_create_destination(&destination, CreateArtifactKind::SfxMacosApp)
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
        cleanup(&dir, &destination);
    }

    #[test]
    fn guarded_app_publish_rejects_deep_tree_changes_before_any_move() {
        let dir = test_dir("guarded-app-deep-change");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("Package.app");
        let nested = Path::new("Contents/Resources/config/runtime.dat");
        fs::create_dir_all(destination.join(nested).parent().unwrap()).unwrap();
        fs::write(destination.join(nested), b"old-data").unwrap();
        let guard =
            crate::inspect_create_destination(&destination, CreateArtifactKind::SfxMacosApp)
                .unwrap()
                .guard
                .unwrap();
        fs::write(destination.join(nested), b"new-data").unwrap();
        let staged = staged_app(&destination, nested, b"replacement");
        let mut moves = 0usize;

        let error = replace_staged_path_with_policy(
            &staged,
            &destination,
            SfxLayout::MacosApp,
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
            &mut |from, to| {
                moves += 1;
                crate::move_path_no_replace(from, to)
            },
            &mut sync_directory,
        )
        .unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(moves, 0);
        assert_eq!(fs::read(destination.join(nested)).unwrap(), b"new-data");
        assert_eq!(fs::read(staged.join(nested)).unwrap(), b"replacement");
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn recovery_rejects_same_identity_content_changes_after_a_crash() {
        let dir = test_dir("recovery-content-change");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let mut moves = 0usize;
        let mut previous = None;

        let interruption = replace_staged_path_with(
            &staged,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                crate::move_path_no_replace(from, to)?;
                if moves == 2 {
                    fs::write(to, b"tampered")?;
                    previous = Some(to.to_path_buf());
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated crash after changing the moved previous output",
                    ));
                }
                Ok(())
            },
            &mut sync_directory,
        )
        .unwrap_err();
        assert!(sfx_recovery_details(&interruption).is_some());

        let recovery = preflight_destination(&destination).unwrap_err();
        let details = sfx_recovery_details(&recovery).unwrap();
        let previous = previous.unwrap();
        assert!(!destination.exists());
        assert_eq!(fs::read(&previous).unwrap(), b"tampered");
        assert!(journal_path(&destination).unwrap().exists());
        assert!(!completion_path(&destination).exists());
        assert!(details.paths.contains(&previous));
        assert!(recovery.to_string().contains("content changed"));
        cleanup(&dir, &destination);
    }

    #[test]
    fn completed_transaction_rechecks_backup_content_before_cleanup() {
        let dir = test_dir("completed-content-change");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let staged = staged_file(&destination, b"replacement");

        let preserved =
            replace_staged_path(&staged, &destination, SfxLayout::SingleFile, true).unwrap();
        assert_eq!(preserved.len(), 1);
        fs::write(&preserved[0], b"tampered").unwrap();

        let error = preflight_destination(&destination).unwrap_err();
        assert!(!sfx_recovery_requires_staging(&error));
        assert!(error.to_string().contains("backup content changed"));
        assert!(completion_path(&destination).exists());
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"tampered");
        cleanup(&dir, &destination);
    }

    #[test]
    fn cleanup_preserves_a_quarantined_app_when_a_deep_member_arrives() {
        let dir = test_dir("cleanup-deep-member");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("Package.app");
        let (staged, staged_identity) =
            reserve_staged_path(&destination, SfxLayout::MacosApp).unwrap();
        let original = staged.join("Contents/Resources/original.dat");
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::write(&original, b"owned staging data").unwrap();
        let state_digest = path_state_digest(&staged).unwrap().unwrap();
        let parent = fs::canonicalize(&dir).unwrap();
        let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
        let record = CleanupRecord {
            version: CLEANUP_VERSION,
            kind: CleanupKind::Stage,
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

        let error = reconcile_cleanup(&destination, &mut |path| {
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
        cleanup(&dir, &destination);
    }

    #[test]
    fn cleanup_preserves_a_file_rebound_after_digest_validation() {
        let dir = test_dir("cleanup-file-post-digest-rebind");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        let (staged, staged_identity) =
            reserve_staged_path(&destination, SfxLayout::SingleFile).unwrap();
        fs::write(&staged, b"owned staging data").unwrap();
        let state_digest = path_state_digest(&staged).unwrap().unwrap();
        let parent = fs::canonicalize(&dir).unwrap();
        let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
        let record = CleanupRecord {
            version: CLEANUP_VERSION,
            kind: CleanupKind::Stage,
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

        let error = reconcile_cleanup_with_disposal_move(
            &destination,
            &mut sync_directory,
            &mut |from, to| {
                if !injected {
                    fs::rename(from, &displaced)?;
                    fs::rename(&competitor, from)?;
                    injected = true;
                }
                crate::move_path_no_replace(from, to)
            },
        )
        .unwrap_err();

        assert!(injected);
        assert_eq!(fs::read(&quarantine).unwrap(), b"unrelated competitor");
        assert_eq!(fs::read(&displaced).unwrap(), b"owned staging data");
        assert!(dir.join(CLEANUP_JOURNAL_NAME).exists());
        assert!(error
            .to_string()
            .contains("without deleting an unverified path"));
        cleanup(&dir, &destination);
    }

    #[test]
    fn cleanup_preserves_a_deep_directory_rebound_after_digest_validation() {
        let dir = test_dir("cleanup-deep-post-digest-rebind");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("Package.app");
        let (staged, staged_identity) =
            reserve_staged_path(&destination, SfxLayout::MacosApp).unwrap();
        let nested = staged.join("Contents/Resources/nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("owned.dat"), b"owned nested data").unwrap();
        let state_digest = path_state_digest(&staged).unwrap().unwrap();
        let parent = fs::canonicalize(&dir).unwrap();
        let quarantine = reserve_cleanup_quarantine(&parent).unwrap();
        let record = CleanupRecord {
            version: CLEANUP_VERSION,
            kind: CleanupKind::Stage,
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

        let error = reconcile_cleanup_with_disposal_move(
            &destination,
            &mut sync_directory,
            &mut |from, to| {
                if !injected {
                    let nested = from.join("Contents/Resources/nested");
                    fs::rename(&nested, &displaced)?;
                    fs::rename(&competitor, &nested)?;
                    injected = true;
                }
                crate::move_path_no_replace(from, to)
            },
        )
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
        cleanup(&dir, &destination);
    }

    #[test]
    fn artifact_matcher_only_accepts_exact_destination_transaction_names() {
        let dir = test_dir("artifact-matcher");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        let other_destination = dir.join("other.exe");
        let staged = staged_file(&destination, b"replacement");
        let journal = journal_path(&destination).unwrap();
        let (holder, holder_identity) = reserve_holder(&destination, &mut sync_directory).unwrap();
        let key = destination_key(&destination).unwrap();
        let journal_temp = dir.join(format!(".squallz-sfx-journal-{}-1.tmp", std::process::id(),));

        assert!(matches_sfx_transaction_artifact(&destination, &journal));
        assert!(matches_sfx_transaction_artifact(&destination, &staged));
        assert!(matches_sfx_transaction_artifact(&destination, &holder));
        assert!(matches_sfx_transaction_artifact(
            &destination,
            &holder.join("previous")
        ));
        assert!(matches_sfx_transaction_artifact(
            &destination,
            &holder.join("replacement")
        ));
        assert!(matches_sfx_transaction_artifact(
            &destination,
            &journal_temp
        ));

        assert!(matches_sfx_transaction_artifact(
            &other_destination,
            &journal
        ));
        assert!(!matches_sfx_transaction_artifact(
            &destination,
            &dir.join(".squallz-sfx-not-the-destination-10-1")
        ));
        assert!(!matches_sfx_transaction_artifact(
            &destination,
            &dir.join(format!(".squallz-sfx-{}-0-1", &key[..16]))
        ));
        assert!(!matches_sfx_transaction_artifact(
            &destination,
            &dir.join(format!(".squallz-sfx-{}-10-1-extra", &key[..16]))
        ));
        assert!(!matches_sfx_transaction_artifact(
            &destination,
            &dir.join(format!(
                ".other.exe.sfx-{}-0.tmp.other.exe",
                std::process::id()
            ))
        ));
        assert!(!matches_sfx_transaction_artifact(
            &destination,
            &holder.join("unrelated")
        ));

        remove_empty_holder(&holder, holder_identity, &mut sync_directory);
        cleanup(&dir, &destination);
    }

    #[test]
    fn artifact_matcher_rejects_ordinary_entries_before_filesystem_resolution() {
        assert!(!matches_sfx_transaction_artifact(
            Path::new("/definitely-missing-parent/package.exe"),
            Path::new("/another-missing-parent/readme.txt")
        ));
        assert!(!matches_sfx_transaction_artifact(
            Path::new("relative/missing/package.exe"),
            Path::new("relative/missing/photos")
        ));
    }

    #[test]
    fn pending_transaction_is_recovered_through_its_recorded_case_alias() {
        let dir = test_dir("case-alias-recovery");
        fs::create_dir(&dir).unwrap();
        if !case_aliases_share_an_entry(&dir) {
            fs::remove_dir_all(&dir).unwrap();
            return;
        }

        let destination = dir.join("Package.exe");
        let alias = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let first_stage = staged_file(&alias, b"first replacement");
        let mut moves = 0usize;
        let interruption = replace_staged_path_with(
            &first_stage,
            &alias,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                let result = crate::move_path_no_replace(from, to);
                if moves == 2 && result.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated stop after moving the aliased destination",
                    ));
                }
                result
            },
            &mut sync_directory,
        )
        .unwrap_err();
        let details = sfx_recovery_details(&interruption).unwrap();
        assert_eq!(details.target.file_name(), Some(OsStr::new("Package.exe")));
        assert!(!destination.exists());
        assert!(journal_path(&alias).unwrap().exists());

        let second_stage = staged_file(&alias, b"second replacement");
        let pending =
            replace_staged_path(&second_stage, &alias, SfxLayout::SingleFile, true).unwrap_err();
        assert_eq!(acknowledge_completed_backup(&pending, &alias), b"previous");
        let preserved =
            replace_staged_path(&second_stage, &alias, SfxLayout::SingleFile, true).unwrap();
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        assert_eq!(
            destination_key(&destination).unwrap(),
            destination_key(&alias).unwrap()
        );
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn absent_case_alias_publishers_share_directory_and_destination_locks() {
        use std::sync::{Arc, Barrier};

        let dir = test_dir("case-alias-concurrency");
        fs::create_dir(&dir).unwrap();
        if !case_aliases_share_an_entry(&dir) {
            fs::remove_dir_all(&dir).unwrap();
            return;
        }

        let destination = dir.join("Package.exe");
        let alias = dir.join("package.exe");
        let upper_stage = staged_file(&destination, b"upper replacement");
        let lower_stage = staged_file(&alias, b"lower replacement");
        let barrier = Arc::new(Barrier::new(3));
        let upper_barrier = Arc::clone(&barrier);
        let upper_destination = destination.clone();
        let upper = std::thread::spawn(move || {
            upper_barrier.wait();
            replace_staged_path(
                &upper_stage,
                &upper_destination,
                SfxLayout::SingleFile,
                true,
            )
        });
        let lower_barrier = Arc::clone(&barrier);
        let lower_destination = alias.clone();
        let lower = std::thread::spawn(move || {
            lower_barrier.wait();
            replace_staged_path(
                &lower_stage,
                &lower_destination,
                SfxLayout::SingleFile,
                true,
            )
        });
        barrier.wait();
        let mut outputs = upper.join().unwrap().unwrap();
        outputs.extend(lower.join().unwrap().unwrap());
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
        assert_eq!(
            destination_key(&destination).unwrap(),
            destination_key(&alias).unwrap()
        );
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn repeated_sfx_rebuilds_exclude_owned_transaction_artifacts_from_source_payload() {
        let dir = test_dir("rebuild-source-exclusions");
        let source = dir.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("payload.txt"), b"user payload").unwrap();
        let destination = source.join("package.exe");
        let stub = write_test_pe_stub(&dir);
        let engine = test_sfx_engine();
        let create_options = CreateOptions::default();
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
                &create_options,
                &sfx_options,
                &NoProgress,
                &ControlToken::new(),
            )
        };
        let assert_clean_manifest = |manifest: &[crate::CreateInputManifestEntry]| {
            assert!(manifest
                .iter()
                .any(|entry| entry.archive_path.to_string().ends_with("payload.txt")));
            assert!(manifest.iter().all(|entry| {
                !matches_sfx_transaction_artifact(&destination, &entry.source_path)
                    && !crate::same_path_entry(&destination, &entry.source_path)
            }));
        };

        let initial = build().unwrap();
        assert_clean_manifest(&initial.manifest);

        let interrupted_stage = staged_file(&destination, b"interrupted replacement");
        let interruption = replace_staged_path_with(
            &interrupted_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |_from, _to| {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "simulated process stop before the first transaction move",
                ))
            },
            &mut sync_directory,
        )
        .unwrap_err();
        let recovery = sfx_recovery_details(&interruption).unwrap();
        assert!(recovery.paths.iter().all(|path| path.exists()));
        assert!(recovery
            .paths
            .iter()
            .all(|path| matches_sfx_transaction_artifact(&destination, path)));

        let pending = build().unwrap_err();
        assert!(!acknowledge_completed_backup(&pending, &destination).is_empty());

        for rebuild in 0..3 {
            let verified = build().unwrap();
            assert_clean_manifest(&verified.manifest);
            assert_eq!(verified.sfx.preserved_outputs.len(), 1);
            assert!(verified
                .sfx
                .preserved_outputs
                .iter()
                .all(|path| path.is_file()));
            if rebuild < 2 {
                fs::remove_file(&verified.sfx.preserved_outputs[0]).unwrap();
            }
        }
        assert!(!journal_path(&destination).unwrap().exists());

        cleanup(&dir, &destination);
    }

    fn cleanup(dir: &Path, destination: &Path) {
        let lock = std::env::temp_dir().join(format!(
            "squallz-sfx-transaction-{}.lock",
            destination_key(destination).unwrap_or_default()
        ));
        let directory_lock = std::env::temp_dir().join(format!(
            "squallz-sfx-directory-{}.lock",
            destination_directory_key(destination).unwrap_or_default()
        ));
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_file(lock);
        let _ = fs::remove_file(directory_lock);
    }

    #[test]
    fn interrupted_backup_move_is_resumed_by_the_next_exact_destination_publish() {
        let dir = test_dir("resume-backup");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let first_stage = staged_file(&destination, b"first replacement");
        let mut moves = 0usize;

        let error = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                let result = crate::move_path_no_replace(from, to);
                if moves == 2 && result.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated process stop after the previous output move",
                    ));
                }
                result
            },
            &mut sync_directory,
        )
        .unwrap_err();
        assert!(sfx_recovery_details(&error).is_some());
        assert!(journal_path(&destination).unwrap().exists());

        let second_stage = staged_file(&destination, b"second replacement");
        let pending = replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true)
            .unwrap_err();
        assert_eq!(
            acknowledge_completed_backup(&pending, &destination),
            b"previous"
        );
        let preserved =
            replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn first_transaction_move_failure_keeps_staging_and_recovers_on_the_next_publish() {
        let dir = test_dir("first-move-failure");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let first_stage = staged_file(&destination, b"first replacement");

        let error = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |_from, _to| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated failure before the first transaction move",
                ))
            },
            &mut sync_directory,
        )
        .unwrap_err();
        let error = with_active_staging_path(error, &first_stage);
        assert!(super::super::recovery_error_requires_staging(&error));
        let details = sfx_recovery_details(&error).unwrap();
        assert!(details.paths.iter().all(|path| path.exists()));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &first_stage)));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal_path(&destination).unwrap())));
        assert!(details.paths.iter().any(|path| path.is_dir()));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read(&first_stage).unwrap(), b"first replacement");

        let second_stage = staged_file(&destination, b"second replacement");
        let pending = replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true)
            .unwrap_err();
        assert_eq!(
            acknowledge_completed_backup(&pending, &destination),
            b"previous"
        );
        let preserved =
            replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true).unwrap();
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        cleanup(&dir, &destination);
    }

    #[test]
    fn journal_parent_sync_failure_keeps_recoverable_state_for_the_next_publish() {
        let dir = test_dir("journal-parent-sync-failure");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let first_stage = staged_file(&destination, b"first replacement");
        let mut sync_calls = 0usize;

        let error = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| crate::move_path_no_replace(from, to),
            &mut |path| {
                sync_calls += 1;
                if sync_calls == 2 {
                    return Err(io::Error::other(
                        "simulated parent sync failure after journal publication",
                    ));
                }
                sync_directory(path)
            },
        )
        .unwrap_err();
        let error = with_active_staging_path(error, &first_stage);
        assert!(super::super::recovery_error_requires_staging(&error));
        let details = sfx_recovery_details(&error).unwrap();
        assert!(details.paths.iter().all(|path| path.exists()));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &first_stage)));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal_path(&destination).unwrap())));
        assert!(details.paths.iter().any(|path| path.is_dir()));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");

        let second_stage = staged_file(&destination, b"second replacement");
        let pending = replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true)
            .unwrap_err();
        assert_eq!(
            acknowledge_completed_backup(&pending, &destination),
            b"previous"
        );
        let preserved =
            replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true).unwrap();
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn duplicate_rename_identities_are_replayed_without_losing_either_output() {
        for duplicate_after_move in 1..=3usize {
            let dir = test_dir(&format!("duplicate-rename-{duplicate_after_move}"));
            fs::create_dir(&dir).unwrap();
            let destination = dir.join("package.exe");
            fs::write(&destination, b"previous").unwrap();
            let staged = staged_file(&destination, b"replacement");
            let mut moves = 0usize;

            let interrupted = replace_staged_path_with(
                &staged,
                &destination,
                SfxLayout::SingleFile,
                true,
                &mut |from, to| {
                    moves += 1;
                    crate::move_path_no_replace(from, to)?;
                    if moves == duplicate_after_move {
                        fs::hard_link(to, from)?;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "simulated crash after the destination directory entry persisted first",
                        ));
                    }
                    Ok(())
                },
                &mut sync_directory,
            )
            .unwrap_err();
            assert!(sfx_recovery_details(&interrupted).is_some());

            let pending = preflight_destination(&destination).unwrap_err();
            assert_eq!(fs::read(&destination).unwrap(), b"replacement");
            assert_eq!(
                acknowledge_completed_backup(&pending, &destination),
                b"previous"
            );
            assert!(!journal_path(&destination).unwrap().exists());
            cleanup(&dir, &destination);
        }
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

    #[test]
    fn new_destination_sync_failure_reports_the_installed_output_for_recovery() {
        let dir = test_dir("new-destination-sync-failure");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        let first_stage = staged_file(&destination, b"first replacement");

        let error = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| crate::move_path_no_replace(from, to),
            &mut |_path| {
                Err(io::Error::other(
                    "simulated parent sync failure after destination installation",
                ))
            },
        )
        .unwrap_err();

        let details = sfx_recovery_details(&error).unwrap();
        assert_eq!(details.target, canonical_destination(&destination).unwrap());
        assert_eq!(details.paths, vec![details.target.clone()]);
        assert!(error
            .to_string()
            .contains("simulated parent sync failure after destination installation"));
        assert!(!first_stage.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"first replacement");

        let second_stage = staged_file(&destination, b"second replacement");
        let preserved =
            replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true).unwrap();
        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        cleanup(&dir, &destination);
    }

    #[test]
    fn interrupted_final_install_returns_the_durable_previous_output_on_retry() {
        let dir = test_dir("resume-installed");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let first_stage = staged_file(&destination, b"first replacement");
        let mut moves = 0usize;

        let error = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                let result = crate::move_path_no_replace(from, to);
                if moves == 3 && result.is_ok() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated process stop after final installation",
                    ));
                }
                result
            },
            &mut sync_directory,
        )
        .unwrap_err();
        let details = sfx_recovery_details(&error).unwrap();
        assert_eq!(details.target, canonical_destination(&destination).unwrap());
        assert_eq!(details.paths.len(), 3);
        assert!(details.paths.iter().all(|path| path.exists()));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal_path(&destination).unwrap())));
        let holder = details.paths.iter().find(|path| path.is_dir()).unwrap();
        assert!(details
            .paths
            .iter()
            .any(|path| path == &holder.join("previous") && path.is_file()));

        let second_stage = staged_file(&destination, b"second replacement");
        let pending = replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true)
            .unwrap_err();
        assert_eq!(
            acknowledge_completed_backup(&pending, &destination),
            b"previous"
        );
        let preserved =
            replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true).unwrap();

        assert_eq!(preserved.len(), 1);
        assert_eq!(fs::read(&preserved[0]).unwrap(), b"first replacement");
        assert_eq!(fs::read(&destination).unwrap(), b"second replacement");
        assert!(!journal_path(&destination).unwrap().exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn changed_previous_output_identity_fails_closed_and_reports_every_current_path() {
        let dir = test_dir("changed-previous-identity");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        let competitor = dir.join("competitor.exe");
        fs::write(&destination, b"previous").unwrap();
        fs::write(&competitor, b"competing entry").unwrap();
        let first_stage = staged_file(&destination, b"first replacement");
        let mut moves = 0usize;
        let mut previous_path = None;

        let interruption = replace_staged_path_with(
            &first_stage,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                let result = crate::move_path_no_replace(from, to);
                if moves == 2 && result.is_ok() {
                    previous_path = Some(to.to_path_buf());
                    fs::remove_file(to)?;
                    crate::move_path_no_replace(&competitor, to)?;
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated identity change after the previous output move",
                    ));
                }
                result
            },
            &mut sync_directory,
        )
        .unwrap_err();
        assert!(sfx_recovery_details(&interruption).is_some());

        let second_stage = staged_file(&destination, b"second replacement");
        let error = replace_staged_path(&second_stage, &destination, SfxLayout::SingleFile, true)
            .unwrap_err();
        let details = sfx_recovery_details(&error).unwrap();
        let previous = previous_path.unwrap();
        assert!(!destination.exists());
        assert_eq!(fs::read(&previous).unwrap(), b"competing entry");
        assert_eq!(fs::read(&second_stage).unwrap(), b"second replacement");
        assert!(details.paths.iter().all(|path| path.exists()));
        assert!(details.paths.contains(&previous));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal_path(&destination).unwrap())));
        assert!(details
            .paths
            .iter()
            .any(|path| path.file_name() == Some(OsStr::new("replacement"))
                && fs::read(path).unwrap() == b"first replacement"));

        cleanup(&dir, &destination);
    }

    #[test]
    fn preserved_output_identity_is_rechecked_after_the_journal_is_cleared() {
        use std::cell::{Cell, RefCell};

        let dir = test_dir("preserved-rebind-after-clear");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        let competitor = dir.join("competitor.exe");
        fs::write(&destination, b"previous").unwrap();
        fs::write(&competitor, b"unrelated entry").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let journal = journal_path(&destination).unwrap();
        let previous_path = RefCell::new(None::<PathBuf>);
        let journal_was_published = Cell::new(false);
        let replaced_after_clear = Cell::new(false);

        let error = replace_staged_path_with(
            &staged,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                let result = crate::move_path_no_replace(from, to);
                if result.is_ok() && to.file_name() == Some(OsStr::new("previous")) {
                    previous_path.replace(Some(to.to_path_buf()));
                }
                result
            },
            &mut |path| {
                if journal.exists() {
                    journal_was_published.set(true);
                }
                if journal_was_published.get() && !journal.exists() && !replaced_after_clear.get() {
                    let previous = previous_path
                        .borrow()
                        .clone()
                        .ok_or_else(|| io::Error::other("previous output path was not captured"))?;
                    fs::remove_file(&previous)?;
                    crate::move_path_no_replace(&competitor, &previous)?;
                    replaced_after_clear.set(true);
                }
                sync_directory(path)
            },
        )
        .unwrap_err();

        let previous = previous_path.into_inner().unwrap();
        let details = sfx_recovery_details(&error).unwrap();
        assert!(replaced_after_clear.get());
        assert_eq!(details.target, canonical_destination(&destination).unwrap());
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &completion_path(&destination))));
        assert!(details.paths.contains(&previous));
        assert!(error
            .to_string()
            .contains("previous-output backup identity changed"));
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_eq!(fs::read(&previous).unwrap(), b"unrelated entry");
        assert!(!journal.exists());
        cleanup(&dir, &destination);
    }

    #[test]
    fn oversized_exact_destination_journal_fails_closed_without_scanning_or_moving_outputs() {
        let dir = test_dir("bounded-journal");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let journal = journal_path(&destination).unwrap();
        fs::write(&journal, vec![b'x'; TRANSACTION_MAX_BYTES + 1]).unwrap();
        sync_directory(&dir).unwrap();

        let error =
            replace_staged_path(&staged, &destination, SfxLayout::SingleFile, true).unwrap_err();

        let details = sfx_recovery_details(&error).unwrap();
        assert_eq!(details.target, canonical_destination(&destination).unwrap());
        assert_eq!(details.paths.len(), 2);
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal)));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &staged)));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read(&staged).unwrap(), b"replacement");
        assert_eq!(
            fs::metadata(journal).unwrap().len(),
            (TRANSACTION_MAX_BYTES + 1) as u64
        );
        cleanup(&dir, &destination);
    }

    #[test]
    fn journal_rejects_unknown_fields_without_moving_any_output() {
        let dir = test_dir("strict-journal");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("package.exe");
        fs::write(&destination, b"previous").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let staged_identity = path_identity(&staged).unwrap();
        let previous_digest = path_state_digest(&destination).unwrap().unwrap();
        let replacement_digest = path_state_digest(&staged).unwrap().unwrap();
        let (open, _transaction) = begin_transaction(
            &canonical_destination(&destination).unwrap(),
            &canonical_requested_destination(&destination).unwrap(),
            &staged,
            staged_identity,
            SfxLayout::SingleFile,
            TransactionDigests {
                previous: previous_digest,
                replacement: replacement_digest,
            },
            &mut sync_directory,
        )
        .unwrap();
        let journal = open.path.clone();
        drop(open);
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
        sync_directory(&dir).unwrap();

        let error =
            replace_staged_path(&staged, &destination, SfxLayout::SingleFile, true).unwrap_err();

        let details = sfx_recovery_details(&error).unwrap();
        assert_eq!(details.paths.len(), 2);
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &journal)));
        assert!(details
            .paths
            .iter()
            .any(|path| crate::same_path_entry(path, &staged)));
        assert_eq!(fs::read(&destination).unwrap(), b"previous");
        assert_eq!(fs::read(&staged).unwrap(), b"replacement");
        cleanup(&dir, &destination);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn journal_keeps_non_utf8_destination_and_staging_names_losslessly() {
        use std::os::unix::ffi::OsStringExt;

        let dir = test_dir("non-utf8");
        fs::create_dir(&dir).unwrap();
        let destination = dir.join(OsString::from_vec(b"package-\xff.exe".to_vec()));
        fs::write(&destination, b"previous").unwrap();
        let staged = staged_file(&destination, b"replacement");
        let mut moves = 0usize;
        let _ = replace_staged_path_with(
            &staged,
            &destination,
            SfxLayout::SingleFile,
            true,
            &mut |from, to| {
                moves += 1;
                if moves == 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "simulated stop before the first move",
                    ));
                }
                crate::move_path_no_replace(from, to)
            },
            &mut sync_directory,
        );
        let bytes = fs::read(journal_path(&destination).unwrap()).unwrap();
        let record: TransactionRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            record.destination.to_os_string().unwrap(),
            destination.file_name().unwrap()
        );
        assert_eq!(
            record.staged.to_os_string().unwrap(),
            staged.file_name().unwrap()
        );
        cleanup(&dir, &destination);
    }
}
