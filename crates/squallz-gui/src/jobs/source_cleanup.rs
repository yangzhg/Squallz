//! Verified source removal and recovery notifications after archive creation.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use squallz_core::api::{EntryPath, FormatError, ProgressSink};
use squallz_core::{
    lock_unpoisoned, sync_directory, CreateInputManifestEntry, CreateInputModifiedTime,
    PostSuccessAction,
};

use crate::source_cleanup_journal::{
    remove_empty_holder_if_identity, source_path_identity, PendingSourceCleanup,
    SourceCleanupJournal, SourceCleanupRecord, SourceCleanupRecovery, SourcePathIdentity,
    HOLDER_PREFIX,
};

static TRASH_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(super) struct TrashError;

pub(super) trait TrashAdapter: Send + Sync {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError>;
}

pub(super) struct SystemTrashAdapter;

impl TrashAdapter for SystemTrashAdapter {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trash::delete(path))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(TrashError),
        }
    }
}

pub(super) struct SourceCleanup {
    trash: Arc<dyn TrashAdapter>,
    journal: Arc<SourceCleanupJournal>,
    recovery: Mutex<SourceCleanupRecoveryState>,
}

impl SourceCleanup {
    pub(super) fn new(trash: Arc<dyn TrashAdapter>, journal: Arc<SourceCleanupJournal>) -> Self {
        let mut recovery = SourceCleanupRecoveryState::default();
        recovery.publish_new(source_cleanup_recovery_notice(
            journal.recover_pending(),
            journal.recovery_record_path(),
        ));
        Self {
            trash,
            journal,
            recovery: Mutex::new(recovery),
        }
    }

    pub(super) fn notice(&self) -> Option<SourceCleanupRecoveryNotice> {
        let mut recovery = lock_unpoisoned(&self.recovery);
        if recovery
            .notice
            .as_ref()
            .is_some_and(|notice| notice.status == "busy")
        {
            let notice = source_cleanup_recovery_notice(
                self.journal.recover_pending(),
                self.journal.recovery_record_path(),
            );
            recovery.refresh_if_changed(notice);
        }
        recovery.notice.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceCleanupRecoveryNotice {
    generation: u64,
    status: String,
    path: Option<String>,
    reason: Option<String>,
    journal_path: Option<String>,
}

fn source_cleanup_recovery_notice(
    recovery: io::Result<SourceCleanupRecovery>,
    journal_path: Option<&Path>,
) -> Option<SourceCleanupRecoveryNotice> {
    let notice = match recovery {
        Ok(SourceCleanupRecovery::None) => return None,
        Ok(SourceCleanupRecovery::Restored { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "restored".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Preserved { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "preserved".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Changed { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "changed".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Cleared) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "cleared".to_owned(),
            path: None,
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::CompletedUnknown { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "completed_unknown".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "busy".to_owned(),
            path: None,
            reason: None,
            journal_path: None,
        },
        Err(error) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "needs_attention".to_owned(),
            path: None,
            reason: Some(source_cleanup_recovery_reason(error.kind()).to_owned()),
            journal_path: journal_path.map(|path| path.to_string_lossy().into_owned()),
        },
    };
    Some(notice)
}

fn source_cleanup_recovery_reason(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::InvalidData => "journal_invalid",
        io::ErrorKind::PermissionDenied => "journal_permission_denied",
        io::ErrorKind::NotFound => "journal_unavailable",
        _ => "recovery_failed",
    }
}

#[derive(Default)]
struct SourceCleanupRecoveryState {
    generation: u64,
    notice: Option<SourceCleanupRecoveryNotice>,
}

impl SourceCleanupRecoveryState {
    fn publish_new(&mut self, notice: Option<SourceCleanupRecoveryNotice>) {
        self.install(notice);
    }

    fn refresh_if_changed(&mut self, notice: Option<SourceCleanupRecoveryNotice>) {
        if same_source_cleanup_notice(self.notice.as_ref(), notice.as_ref()) {
            return;
        }
        self.install(notice);
    }

    fn install(&mut self, mut notice: Option<SourceCleanupRecoveryNotice>) {
        if let Some(notice) = &mut notice {
            self.generation = self.generation.saturating_add(1);
            notice.generation = self.generation;
        }
        self.notice = notice;
    }
}

fn same_source_cleanup_notice(
    left: Option<&SourceCleanupRecoveryNotice>,
    right: Option<&SourceCleanupRecoveryNotice>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.status == right.status
                && left.path == right.path
                && left.reason == right.reason
                && left.journal_path == right.journal_path
        }
        _ => false,
    }
}

#[derive(Debug)]
pub(super) struct CleanupCandidate {
    path: PathBuf,
    identity: PathBuf,
    is_dir: bool,
    snapshot: FrozenTreeSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrozenPathKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenMetadata {
    kind: FrozenPathKind,
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenTreeSnapshot {
    fingerprint: blake3::Hash,
    entries: usize,
    supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FingerprintError {
    Cancelled,
    Unavailable,
}

impl FrozenMetadata {
    fn capture(metadata: &fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_file() {
            FrozenPathKind::File
        } else if file_type.is_dir() {
            FrozenPathKind::Directory
        } else if file_type.is_symlink() {
            FrozenPathKind::Symlink
        } else {
            FrozenPathKind::Other
        };
        Self {
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
        }
    }

    fn update_fingerprint(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&[match self.kind {
            FrozenPathKind::File => 1,
            FrozenPathKind::Directory => 2,
            FrozenPathKind::Symlink => 3,
            FrozenPathKind::Other => 4,
        }]);
        hasher.update(&self.len.to_le_bytes());
        update_system_time(hasher, self.modified);
        update_system_time(hasher, self.created);
        hasher.update(&[u8::from(self.readonly)]);
        #[cfg(unix)]
        {
            hasher.update(&self.device.to_le_bytes());
            hasher.update(&self.inode.to_le_bytes());
            hasher.update(&self.mode.to_le_bytes());
        }
    }
}

fn update_system_time(hasher: &mut blake3::Hasher, value: Option<SystemTime>) {
    let Some(value) = value else {
        hasher.update(&[0]);
        return;
    };
    match value.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            hasher.update(&[1]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
        Err(error) => {
            let duration = error.duration();
            hasher.update(&[2]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
    }
}

#[cfg(unix)]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;

    let bytes = value.as_bytes();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(target_os = "windows")]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<u16> = value.encode_wide().collect();
    hasher.update(&(units.len() as u64).to_le_bytes());
    for unit in units {
        hasher.update(&unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    let value = value.to_string_lossy();
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn update_fingerprint_entry(
    hasher: &mut blake3::Hasher,
    root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), ()> {
    let relative = path.strip_prefix(root).map_err(|_| ())?;
    update_os_str(hasher, relative.as_os_str());
    FrozenMetadata::capture(metadata).update_fingerprint(hasher);
    Ok(())
}

fn fingerprint_checkpoint(is_cancelled: &dyn Fn() -> bool) -> Result<(), FingerprintError> {
    if is_cancelled() {
        Err(FingerprintError::Cancelled)
    } else {
        Ok(())
    }
}

fn update_symlink_target_fingerprint(
    hasher: &mut blake3::Hasher,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FingerprintError> {
    if metadata.file_type().is_symlink() {
        hasher.update(b"symlink-target\0");
        let target = fs::read_link(path).map_err(|_| FingerprintError::Unavailable)?;
        update_os_str(hasher, target.as_os_str());
    }
    Ok(())
}

fn capture_tree_snapshot(
    root: &Path,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<FrozenTreeSnapshot, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let root_metadata = fs::symlink_metadata(root).map_err(|_| FingerprintError::Unavailable)?;
    let mut hasher = blake3::Hasher::new();
    let mut supported =
        root_metadata.is_file() || root_metadata.is_dir() || root_metadata.file_type().is_symlink();
    let mut entries_seen = 1usize;
    update_fingerprint_entry(&mut hasher, root, root, &root_metadata)
        .map_err(|_| FingerprintError::Unavailable)?;
    update_symlink_target_fingerprint(&mut hasher, root, &root_metadata)?;
    progress.on_progress(
        entries_seen as u64,
        0,
        &EntryPath::from_utf8(root.to_string_lossy().into_owned()),
    );
    if !root_metadata.is_dir() {
        return Ok(FrozenTreeSnapshot {
            fingerprint: hasher.finalize(),
            entries: entries_seen,
            supported,
        });
    }

    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        fingerprint_checkpoint(is_cancelled)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| FingerprintError::Unavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| FingerprintError::Unavailable)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            fingerprint_checkpoint(is_cancelled)?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| FingerprintError::Unavailable)?;
            update_fingerprint_entry(&mut hasher, root, &path, &metadata)
                .map_err(|_| FingerprintError::Unavailable)?;
            update_symlink_target_fingerprint(&mut hasher, &path, &metadata)?;
            supported &=
                metadata.is_file() || metadata.is_dir() || metadata.file_type().is_symlink();
            entries_seen = entries_seen.saturating_add(1);
            progress.on_progress(
                entries_seen as u64,
                0,
                &EntryPath::from_utf8(path.to_string_lossy().into_owned()),
            );
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(FrozenTreeSnapshot {
        fingerprint: hasher.finalize(),
        entries: entries_seen,
        supported,
    })
}

#[derive(Debug)]
pub(super) enum SourceCleanupPlan {
    NotRequested { kept: usize },
    Ready(Vec<CleanupCandidate>),
    Blocked { kept: usize },
    Failed { kept: usize },
}

impl SourceCleanupPlan {
    pub(super) fn requires_content_verification(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceCleanupStatus {
    NotRequested,
    Completed,
    Blocked,
    Partial,
    Failed,
    Cancelled,
}

impl SourceCleanupStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceCleanupResult {
    status: SourceCleanupStatus,
    moved: usize,
    kept: usize,
    recovery_required: usize,
}

impl SourceCleanupResult {
    pub(super) fn new(status: SourceCleanupStatus, moved: usize, kept: usize) -> Self {
        Self {
            status,
            moved,
            kept,
            recovery_required: 0,
        }
    }

    pub(super) fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "moved": self.moved,
            "kept": self.kept,
            "recovery_required": self.recovery_required,
        })
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ()> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|_| ())
    }
}

fn cleanup_candidate(
    path: &Path,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<CleanupCandidate, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let absolute = absolute_path(path).map_err(|_| FingerprintError::Unavailable)?;
    let metadata = fs::symlink_metadata(&absolute).map_err(|_| FingerprintError::Unavailable)?;
    if metadata.file_type().is_symlink() {
        let parent = absolute.parent().ok_or(FingerprintError::Unavailable)?;
        let file_name = absolute.file_name().ok_or(FingerprintError::Unavailable)?;
        let identity = fs::canonicalize(parent)
            .map_err(|_| FingerprintError::Unavailable)?
            .join(file_name);
        return Ok(CleanupCandidate {
            path: identity.clone(),
            identity,
            is_dir: false,
            snapshot: capture_tree_snapshot(&absolute, is_cancelled, progress)?,
        });
    }

    let identity = fs::canonicalize(&absolute).map_err(|_| FingerprintError::Unavailable)?;
    let snapshot = capture_tree_snapshot(&identity, is_cancelled, progress)?;
    Ok(CleanupCandidate {
        path: identity.clone(),
        identity,
        is_dir: metadata.is_dir(),
        snapshot,
    })
}

fn top_level_cleanup_candidates(
    inputs: &[PathBuf],
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<Vec<CleanupCandidate>, FingerprintError> {
    let mut candidates = inputs
        .iter()
        .map(|path| cleanup_candidate(path, is_cancelled, progress))
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort_by(|left, right| {
        left.identity
            .components()
            .count()
            .cmp(&right.identity.components().count())
            .then_with(|| left.identity.cmp(&right.identity))
    });

    let mut top_level: Vec<CleanupCandidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let nested_or_duplicate = top_level.iter().any(|selected| {
            selected.identity == candidate.identity
                || (selected.is_dir && candidate.identity.starts_with(&selected.identity))
        });
        if !nested_or_duplicate {
            top_level.push(candidate);
        }
    }
    Ok(top_level)
}

pub(super) fn prepare_source_cleanup(
    inputs: &[PathBuf],
    action: PostSuccessAction,
    excludes: &[String],
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<SourceCleanupPlan, FormatError> {
    if action != PostSuccessAction::TrashSource {
        return Ok(SourceCleanupPlan::NotRequested { kept: inputs.len() });
    }
    if !excludes.is_empty() {
        return Ok(SourceCleanupPlan::Blocked { kept: inputs.len() });
    }
    match top_level_cleanup_candidates(inputs, is_cancelled, progress) {
        Ok(candidates) => Ok(SourceCleanupPlan::Ready(candidates)),
        Err(FingerprintError::Cancelled) => Err(FormatError::Cancelled),
        Err(FingerprintError::Unavailable) => Ok(SourceCleanupPlan::Failed { kept: inputs.len() }),
    }
}

fn cleanup_is_blocked(candidates: &[CleanupCandidate], outputs: &[PathBuf]) -> Result<bool, ()> {
    for output in outputs {
        let output_metadata = fs::symlink_metadata(output).map_err(|_| ())?;
        let output_identity = fs::canonicalize(output).map_err(|_| ())?;
        for candidate in candidates {
            if candidate.identity == output_identity
                || (candidate.is_dir && output_identity.starts_with(&candidate.identity))
                || (output_metadata.is_dir() && candidate.identity.starts_with(&output_identity))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchivedSourceEntry {
    archive_paths: Vec<EntryPath>,
    entry_type: squallz_core::api::EntryType,
    size: u64,
    modified: Option<CreateInputModifiedTime>,
    unix_mode: Option<u32>,
    blake3: Option<[u8; 32]>,
}

impl ArchivedSourceEntry {
    fn matches_manifest_entry(&self, input: &CreateInputManifestEntry) -> bool {
        self.entry_type == input.entry_type
            && self.size == input.size
            && self.modified == input.modified
            && self.unix_mode == input.unix_mode
            && self.blake3 == input.blake3
    }
}

type ArchivedInputMap = BTreeMap<PathBuf, ArchivedSourceEntry>;

fn verify_manifest_archive_path(path: &EntryPath) -> Result<(), FingerprintError> {
    if path.encoding != "utf-8"
        || std::str::from_utf8(&path.raw).ok() != Some(path.display.as_str())
    {
        return Err(FingerprintError::Unavailable);
    }
    Ok(())
}

fn archived_input_map(
    archived_inputs: &[CreateInputManifestEntry],
) -> Result<ArchivedInputMap, FingerprintError> {
    let mut inputs = BTreeMap::new();
    let mut archive_sources = BTreeMap::new();
    for input in archived_inputs {
        if input.source_path.to_str().is_none() {
            return Err(FingerprintError::Unavailable);
        }
        verify_manifest_archive_path(&input.archive_path)?;
        match archive_sources.entry(input.archive_path.raw.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(input.source_path.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if entry.get() == &input.source_path => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(FingerprintError::Unavailable);
            }
        }

        let source = ArchivedSourceEntry {
            archive_paths: vec![input.archive_path.clone()],
            entry_type: input.entry_type.clone(),
            size: input.size,
            modified: input.modified,
            unix_mode: input.unix_mode,
            blake3: input.blake3,
        };
        match inputs.entry(input.source_path.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(source);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if !entry.get().matches_manifest_entry(input) {
                    return Err(FingerprintError::Unavailable);
                }
                if !entry.get().archive_paths.contains(&input.archive_path) {
                    entry
                        .get_mut()
                        .archive_paths
                        .push(input.archive_path.clone());
                }
            }
        }
    }
    Ok(inputs)
}

fn archived_input_belongs_to_candidate(path: &Path, candidate: &CleanupCandidate) -> bool {
    path == candidate.identity || (candidate.is_dir && path.starts_with(&candidate.identity))
}

fn verify_archived_file(
    path: &Path,
    expected_size: u64,
    expected_blake3: &[u8; 32],
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Ok(false);
    }
    let identity = fs::canonicalize(path).map_err(|_| FingerprintError::Unavailable)?;
    if identity != path {
        return Ok(false);
    }

    let mut file = fs::File::open(path).map_err(|_| FingerprintError::Unavailable)?;
    let current = EntryPath::from_utf8(path.to_string_lossy().into_owned());
    let mut hasher = blake3::Hasher::new();
    let mut file_bytes = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        fingerprint_checkpoint(is_cancelled)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| FingerprintError::Unavailable)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file_bytes = file_bytes.saturating_add(read as u64);
        progress.on_entry_progress(
            verified_bytes.saturating_add(file_bytes),
            total_bytes,
            &current,
            file_bytes.min(expected_size),
            expected_size,
        );
    }
    *verified_bytes = verified_bytes.saturating_add(file_bytes);
    if file_bytes != expected_size || hasher.finalize().as_bytes() != expected_blake3 {
        return Ok(false);
    }
    let final_metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    Ok(final_metadata.is_file()
        && final_metadata.len() == expected_size
        && fs::canonicalize(path).is_ok_and(|current| current == path))
}

fn verify_archived_entry(
    path: &Path,
    expected: &ArchivedSourceEntry,
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    if expected.archive_paths.is_empty() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    if metadata.modified().ok().map(CreateInputModifiedTime::from) != expected.modified {
        return Ok(false);
    }
    #[cfg(unix)]
    let current_mode = Some(metadata.mode());
    #[cfg(not(unix))]
    let current_mode = None;
    if current_mode != expected.unix_mode {
        return Ok(false);
    }

    match &expected.entry_type {
        squallz_core::api::EntryType::File => {
            let Some(blake3) = expected.blake3.as_ref() else {
                return Ok(false);
            };
            verify_archived_file(
                path,
                expected.size,
                blake3,
                verified_bytes,
                total_bytes,
                is_cancelled,
                progress,
            )
        }
        squallz_core::api::EntryType::Dir => {
            Ok(metadata.is_dir() && expected.size == 0 && expected.blake3.is_none())
        }
        squallz_core::api::EntryType::Symlink { target } => {
            if !metadata.file_type().is_symlink() || expected.size != 0 || expected.blake3.is_some()
            {
                return Ok(false);
            }
            let current_target = fs::read_link(path).map_err(|_| FingerprintError::Unavailable)?;
            Ok(std::str::from_utf8(target)
                .ok()
                .is_some_and(|target| current_target.to_str() == Some(target)))
        }
        squallz_core::api::EntryType::Hardlink { .. } | squallz_core::api::EntryType::Other => {
            Ok(false)
        }
    }
}

fn verify_cleanup_candidate_at(
    candidate: &CleanupCandidate,
    current_root: &Path,
    archived_inputs: &ArchivedInputMap,
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    if !candidate.snapshot.supported {
        return Ok(false);
    }
    let before = capture_tree_snapshot(current_root, is_cancelled, progress)?;
    if before != candidate.snapshot {
        return Ok(false);
    }

    let expected: Vec<_> = archived_inputs
        .iter()
        .filter(|(path, _)| archived_input_belongs_to_candidate(path, candidate))
        .collect();
    if expected.len() != candidate.snapshot.entries {
        return Ok(false);
    }
    for (path, archived) in expected {
        let relative = path
            .strip_prefix(&candidate.identity)
            .map_err(|_| FingerprintError::Unavailable)?;
        let current_path = if relative.as_os_str().is_empty() {
            current_root.to_path_buf()
        } else {
            current_root.join(relative)
        };
        if !verify_archived_entry(
            &current_path,
            archived,
            verified_bytes,
            total_bytes,
            is_cancelled,
            progress,
        )? {
            return Ok(false);
        }
    }

    let after = capture_tree_snapshot(current_root, is_cancelled, progress)?;
    Ok(after == candidate.snapshot)
}

struct StagedCleanupCandidate {
    pending: PendingSourceCleanup,
}

impl StagedCleanupCandidate {
    fn staged_path(&self) -> &Path {
        &self.pending.record().staged
    }
}

fn create_cleanup_staging_dir(source: &Path) -> io::Result<PathBuf> {
    create_cleanup_staging_dir_with(source, sync_directory, remove_empty_holder_if_identity)
}

fn create_cleanup_staging_dir_with<S, R>(
    source: &Path,
    mut sync: S,
    mut remove: R,
) -> io::Result<PathBuf>
where
    S: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, SourcePathIdentity) -> io::Result<()>,
{
    let parent = source.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup path has no parent directory",
        )
    })?;
    for _ in 0..1000u32 {
        let sequence = TRASH_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let holder = parent.join(format!("{HOLDER_PREFIX}{}-{sequence}", std::process::id()));
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700).create(&holder)
        };
        #[cfg(not(unix))]
        let created = fs::create_dir(&holder);
        match created {
            Ok(()) => {
                sync_created_cleanup_holder_with(&holder, parent, &mut sync, &mut remove)?;
                return Ok(holder);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a private source cleanup directory",
    ))
}

fn sync_created_cleanup_holder_with<S, R>(
    holder: &Path,
    parent: &Path,
    sync: &mut S,
    remove: &mut R,
) -> io::Result<()>
where
    S: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, SourcePathIdentity) -> io::Result<()>,
{
    let identity = source_path_identity(holder)?;
    let durability = sync(holder).and_then(|()| sync(parent));
    if let Err(error) = durability {
        let _ = remove(holder, identity);
        let _ = sync(parent);
        return Err(error);
    }
    Ok(())
}

fn stage_cleanup_candidate(
    candidate: &CleanupCandidate,
    journal: &SourceCleanupJournal,
    recovery_notice: &Mutex<SourceCleanupRecoveryState>,
) -> Result<StagedCleanupCandidate, StagedRestoreOutcome> {
    let holder =
        create_cleanup_staging_dir(&candidate.path).map_err(|_| StagedRestoreOutcome::Restored)?;
    let Some(file_name) = candidate.path.file_name() else {
        let _ = fs::remove_dir(&holder);
        return Err(StagedRestoreOutcome::Restored);
    };
    let staged = holder.join(file_name);
    let record = match SourceCleanupRecord::new(candidate.path.clone(), staged, holder.clone()) {
        Ok(record) => record,
        Err(_) => {
            let _ = fs::remove_dir(&holder);
            return Err(StagedRestoreOutcome::Restored);
        }
    };
    let pending = match journal.begin(&record) {
        Ok(pending) => pending,
        Err(_) => {
            let _ = fs::remove_dir(&holder);
            let recovery = journal.recover_pending();
            let outcome = staged_recovery_outcome_for_begin(&recovery);
            lock_unpoisoned(recovery_notice).publish_new(source_cleanup_recovery_notice(
                recovery,
                journal.recovery_record_path(),
            ));
            return Err(outcome);
        }
    };
    let staged = StagedCleanupCandidate { pending };
    if squallz_core::move_path_no_replace(&candidate.path, staged.staged_path()).is_err() {
        return Err(restore_staged_cleanup(staged, false));
    }
    if staged.pending.sync_after_stage().is_err() {
        return Err(restore_staged_cleanup(staged, false));
    }
    Ok(staged)
}

fn staged_recovery_outcome_for_begin(
    recovery: &io::Result<SourceCleanupRecovery>,
) -> StagedRestoreOutcome {
    match recovery {
        Ok(SourceCleanupRecovery::Restored { .. } | SourceCleanupRecovery::Cleared) => {
            StagedRestoreOutcome::Restored
        }
        Ok(SourceCleanupRecovery::Preserved { .. }) => StagedRestoreOutcome::Preserved,
        Ok(SourceCleanupRecovery::Changed { .. }) => StagedRestoreOutcome::RestoredNeedsReview,
        Ok(SourceCleanupRecovery::None | SourceCleanupRecovery::CompletedUnknown { .. })
        | Err(_) => StagedRestoreOutcome::Failed,
    }
}

fn staged_recovery_outcome(
    recovery: io::Result<SourceCleanupRecovery>,
    review_restored: bool,
) -> StagedRestoreOutcome {
    match recovery {
        Ok(SourceCleanupRecovery::Restored { .. } | SourceCleanupRecovery::Cleared) => {
            if review_restored {
                StagedRestoreOutcome::RestoredNeedsReview
            } else {
                StagedRestoreOutcome::Restored
            }
        }
        Ok(SourceCleanupRecovery::Preserved { .. }) => StagedRestoreOutcome::Preserved,
        Ok(SourceCleanupRecovery::Changed { .. }) => StagedRestoreOutcome::RestoredNeedsReview,
        Ok(SourceCleanupRecovery::None | SourceCleanupRecovery::CompletedUnknown { .. })
        | Err(_) => StagedRestoreOutcome::Failed,
    }
}

fn restore_staged_cleanup(
    staged: StagedCleanupCandidate,
    review_restored: bool,
) -> StagedRestoreOutcome {
    staged_recovery_outcome(staged.pending.recover(), review_restored)
}

fn staged_path_exists(staged: &StagedCleanupCandidate) -> bool {
    match fs::symlink_metadata(staged.staged_path()) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn cleanup_failed_after_stage(
    moved: usize,
    total: usize,
    outcome: StagedRestoreOutcome,
) -> SourceCleanupResult {
    cleanup_result_after_restore(moved, total, SourceCleanupStatus::Failed, outcome)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedRestoreOutcome {
    Restored,
    RestoredNeedsReview,
    Preserved,
    Failed,
}

impl StagedRestoreOutcome {
    fn needs_recovery(self) -> bool {
        self != Self::Restored
    }
}

fn cleanup_result_after_restore(
    moved: usize,
    total: usize,
    restored_status: SourceCleanupStatus,
    outcome: StagedRestoreOutcome,
) -> SourceCleanupResult {
    let recovery_required = usize::from(outcome.needs_recovery());
    let status = if recovery_required == 0 {
        restored_status
    } else if moved == 0 {
        SourceCleanupStatus::Failed
    } else {
        SourceCleanupStatus::Partial
    };
    SourceCleanupResult {
        status,
        moved,
        kept: total.saturating_sub(moved),
        recovery_required,
    }
}

impl SourceCleanup {
    pub(super) fn complete(
        &self,
        plan: SourceCleanupPlan,
        outputs: &[PathBuf],
        archived_inputs: &[CreateInputManifestEntry],
        is_cancelled: &dyn Fn() -> bool,
        progress: &dyn ProgressSink,
    ) -> SourceCleanupResult {
        let journal = &*self.journal;
        let trash_adapter = &*self.trash;
        let candidates = match plan {
            SourceCleanupPlan::NotRequested { kept } => {
                return SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, kept);
            }
            SourceCleanupPlan::Ready(candidates) => candidates,
            SourceCleanupPlan::Blocked { kept } => {
                return SourceCleanupResult::new(SourceCleanupStatus::Blocked, 0, kept);
            }
            SourceCleanupPlan::Failed { kept } => {
                return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, kept);
            }
        };
        let archived_inputs = match archived_input_map(archived_inputs) {
            Ok(inputs) => inputs,
            Err(_) => {
                return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, candidates.len());
            }
        };
        match cleanup_is_blocked(&candidates, outputs) {
            Ok(true) => {
                return SourceCleanupResult::new(SourceCleanupStatus::Blocked, 0, candidates.len());
            }
            Ok(false) => {}
            Err(()) => {
                return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, candidates.len());
            }
        }

        let total_bytes = archived_inputs
            .values()
            .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
            .fold(0u64, |total, entry| total.saturating_add(entry.size));
        let mut verified_bytes = 0u64;
        let mut moved = 0usize;
        for candidate in &candidates {
            if is_cancelled() {
                return SourceCleanupResult::new(
                    SourceCleanupStatus::Cancelled,
                    moved,
                    candidates.len().saturating_sub(moved),
                );
            }
            let staged = match stage_cleanup_candidate(candidate, journal, &self.recovery) {
                Ok(staged) => staged,
                Err(outcome) => {
                    return cleanup_failed_after_stage(moved, candidates.len(), outcome);
                }
            };
            let staged_unchanged = match verify_cleanup_candidate_at(
                candidate,
                staged.staged_path(),
                &archived_inputs,
                &mut verified_bytes,
                total_bytes,
                is_cancelled,
                progress,
            ) {
                Ok(unchanged) => unchanged,
                Err(FingerprintError::Cancelled) => {
                    let outcome = restore_staged_cleanup(staged, false);
                    return cleanup_result_after_restore(
                        moved,
                        candidates.len(),
                        SourceCleanupStatus::Cancelled,
                        outcome,
                    );
                }
                Err(FingerprintError::Unavailable) => false,
            };
            let cancelled = is_cancelled();
            if !staged_unchanged || cancelled {
                let outcome = restore_staged_cleanup(staged, false);
                let restored_status = if cancelled {
                    SourceCleanupStatus::Cancelled
                } else if moved == 0 {
                    SourceCleanupStatus::Blocked
                } else {
                    SourceCleanupStatus::Partial
                };
                return cleanup_result_after_restore(
                    moved,
                    candidates.len(),
                    restored_status,
                    outcome,
                );
            }
            if staged.pending.confirm_staged_source().is_err() {
                let outcome = restore_staged_cleanup(staged, false);
                return cleanup_result_after_restore(
                    moved,
                    candidates.len(),
                    SourceCleanupStatus::Blocked,
                    outcome,
                );
            }
            if trash_adapter.move_to_trash(staged.staged_path()).is_ok() {
                let staged_removed = !staged_path_exists(&staged);
                if staged.pending.complete_trash().is_ok() {
                    moved += 1;
                } else {
                    if staged_removed {
                        moved += 1;
                    }
                    let outcome = restore_staged_cleanup(staged, true);
                    return cleanup_failed_after_stage(moved, candidates.len(), outcome);
                }
            } else {
                let outcome = restore_staged_cleanup(staged, true);
                if outcome.needs_recovery() {
                    return cleanup_result_after_restore(
                        moved,
                        candidates.len(),
                        SourceCleanupStatus::Failed,
                        outcome,
                    );
                }
            }
        }
        let kept = candidates.len().saturating_sub(moved);
        let status = if kept == 0 {
            SourceCleanupStatus::Completed
        } else if moved == 0 {
            SourceCleanupStatus::Failed
        } else {
            SourceCleanupStatus::Partial
        };
        SourceCleanupResult::new(status, moved, kept)
    }
}

#[cfg(test)]
mod tests;
