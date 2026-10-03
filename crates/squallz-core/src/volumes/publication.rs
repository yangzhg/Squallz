//! Durable publication and recovery of a split output family.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::api::FormatError;
use crate::destination_guard::path_state_digest;
use crate::filesystem_identity::open_regular_file_no_follow_read_write;
use crate::stored_os_string::StoredOsString;
use crate::{parent_or_current, sync_directory};

use super::{
    collect_managed_split_outputs, ensure_split_identity, ensure_split_state_binding, is_sqz_base,
    managed_split_output_name, observed_split_identity, remove_bound_split_staging,
    remove_staged_split_outputs, split_file_identity, split_path_identity,
    split_staging_matches_final, split_transaction_conflict, split_transaction_journal_path,
    split_transaction_output_name, transaction_backup_matches_output_family,
    validate_staged_split_outputs, with_preserved_split_debt, with_split_cleanup_errors,
    ManagedSplitOutputSnapshot, PreservedSplitOutput, SplitPathIdentity, SplitStagingBinding,
    SplitStagingPath, StagedSplitOutput,
};

static SPLIT_JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SPLIT_TRANSACTION_VERSION: u32 = 1;
const SPLIT_TRANSACTION_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct SplitPublicationTransaction {
    resolved: ResolvedSplitTransaction,
    open: OpenSplitTransaction,
}

enum CompletionOrigin {
    Recovery,
    NewPublication,
}

impl SplitPublicationTransaction {
    /// The caller retains the output-family lock through recovery and publication.
    pub(super) fn recover(base: &Path) -> Result<Vec<PreservedSplitOutput>, FormatError> {
        let Some(open) = open_split_transaction(base)? else {
            return Ok(Vec::new());
        };
        Self::from_open(base, open)?.finish(CompletionOrigin::Recovery)
    }

    pub(super) fn publish(
        base: &Path,
        staged: &[StagedSplitOutput],
        managed: Vec<ManagedSplitOutputSnapshot>,
    ) -> Result<Vec<PreservedSplitOutput>, FormatError> {
        Self::prepare_new(base, staged, managed)?.finish(CompletionOrigin::NewPublication)
    }

    fn from_open(base: &Path, open: OpenSplitTransaction) -> Result<Self, FormatError> {
        let resolved = resolve_split_transaction(base, &open.record)?;
        Ok(Self { resolved, open })
    }

    fn finish(self, origin: CompletionOrigin) -> Result<Vec<PreservedSplitOutput>, FormatError> {
        let preserved = match self.resume() {
            Ok(preserved) => preserved,
            Err(error) => {
                return Err(with_preserved_split_debt(
                    error,
                    &self.verified_backups(),
                    "verified previous outputs currently remain at",
                ));
            }
        };
        if let Err(error) = self.open.clear() {
            let context = match origin {
                CompletionOrigin::Recovery =>
                    "the interrupted split transaction was recovered and its previous outputs remain at",
                CompletionOrigin::NewPublication =>
                    "the new split output set was installed and its previous outputs remain at",
            };
            return Err(with_preserved_split_debt(error, &preserved, context));
        }
        Ok(preserved)
    }

    fn resume(&self) -> Result<Vec<PreservedSplitOutput>, FormatError> {
        self.resume_with(&mut |from, to| crate::move_path_no_replace(from, to))
    }

    fn prepare_new(
        base: &Path,
        staged: &[StagedSplitOutput],
        managed: Vec<ManagedSplitOutputSnapshot>,
    ) -> Result<Self, FormatError> {
        let prepared = (|| {
            validate_staged_split_outputs(staged)?;
            let mut backups = Vec::with_capacity(managed.len());
            for entry in managed {
                backups.push(ResolvedSplitBackup {
                    backup: crate::sibling_temp_path(&entry.path, "split-backup")?,
                    identity: entry.identity,
                    state_digest: entry.state_digest,
                    original: entry.path,
                });
            }
            let mut outputs = Vec::with_capacity(staged.len());
            for output in staged {
                if split_file_identity(&output.file).ok() != Some(output.identity)
                    || split_path_identity(&output.part).ok() != Some(output.identity)
                {
                    return Err(split_transaction_conflict(
                        "the staged split output was replaced after writing",
                        [&output.part],
                    ));
                }
                let state_digest = path_state_digest(&output.part)?.ok_or_else(|| {
                    split_transaction_conflict(
                        "the staged split output disappeared while it was bound",
                        [&output.part],
                    )
                })?;
                if split_file_identity(&output.file)? != output.identity
                    || split_path_identity(&output.part)? != output.identity
                {
                    return Err(split_transaction_conflict(
                        "the staged split output identity changed while it was bound",
                        [&output.part],
                    ));
                }
                outputs.push(ResolvedSplitOutput {
                    staged: output.part.clone(),
                    final_path: output.final_path.clone(),
                    identity: output.identity,
                    state_digest,
                });
            }
            let resolved = ResolvedSplitTransaction {
                base: base.to_path_buf(),
                include_recovery: is_sqz_base(base),
                backups,
                outputs,
            };
            let record = split_transaction_record(&resolved)?;
            let resolved = resolve_split_transaction(base, &record)?;
            Ok::<_, FormatError>((resolved, record))
        })();
        let (resolved, record) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(with_split_cleanup_errors(
                    error,
                    remove_staged_split_outputs(staged),
                ));
            }
        };
        let open = match write_split_transaction_with_state(base, record) {
            Ok(open) => open,
            Err(failure) => {
                if failure.journal_published {
                    return Err(FormatError::Other(format!(
                        "{}; writer-owned split staging was retained because the transaction journal may already be durable",
                        failure.error
                    )));
                }
                return Err(with_split_cleanup_errors(
                    failure.error,
                    remove_staged_split_outputs(staged),
                ));
            }
        };
        Ok(Self { resolved, open })
    }

    fn ensure_journal_binding(&self) -> Result<(), FormatError> {
        let open = &self.open;
        if split_file_identity(&open.file).ok() != Some(open.identity)
            || split_path_identity(&open.path).ok() != Some(open.identity)
        {
            return Err(split_transaction_conflict(
                "the retained transaction journal no longer owns its published path",
                [&open.path],
            ));
        }
        let mut reader = open.file.try_clone().map_err(|error| {
            split_transaction_conflict(
                &format!("the retained transaction journal could not be read: {error}"),
                [&open.path],
            )
        })?;
        reader.seek(SeekFrom::Start(0)).map_err(|error| {
            split_transaction_conflict(
                &format!("the retained transaction journal could not be rewound: {error}"),
                [&open.path],
            )
        })?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut reader)
            .take((SPLIT_TRANSACTION_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                split_transaction_conflict(
                    &format!("the retained transaction journal could not be verified: {error}"),
                    [&open.path],
                )
            })?;
        if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES
            || *blake3::hash(&bytes).as_bytes() != open.content_digest
            || split_file_identity(&open.file).ok() != Some(open.identity)
            || split_path_identity(&open.path).ok() != Some(open.identity)
        {
            return Err(split_transaction_conflict(
                "the retained transaction journal changed after it was opened",
                [&open.path],
            ));
        }
        Ok(())
    }

    fn verified_backups(&self) -> Vec<PreservedSplitOutput> {
        self.resolved
            .backups
            .iter()
            .filter_map(|entry| {
                let identity_matches = matches!(
                    split_path_identity(&entry.backup),
                    Ok(identity) if identity == entry.identity
                );
                let state_matches = matches!(
                    path_state_digest(&entry.backup),
                    Ok(Some(actual)) if actual == entry.state_digest
                );
                (identity_matches && state_matches).then(|| PreservedSplitOutput {
                    path: entry.backup.clone(),
                    identity: entry.identity,
                    state_digest: entry.state_digest,
                })
            })
            .collect()
    }

    fn resume_with<M>(
        &self,
        move_no_replace: &mut M,
    ) -> Result<Vec<PreservedSplitOutput>, FormatError>
    where
        M: FnMut(&Path, &Path) -> io::Result<()>,
    {
        let transaction = &self.resolved;
        self.ensure_journal_binding()?;
        let output_identities = transaction
            .outputs
            .iter()
            .map(|entry| (entry.final_path.clone(), entry.identity))
            .collect::<HashMap<_, _>>();
        for entry in &transaction.backups {
            let original = observed_split_identity(&entry.original)?;
            let backup = observed_split_identity(&entry.backup)?;
            let installed_identity = output_identities.get(&entry.original).copied();
            match (original, backup) {
                (Some(original), None) if original == entry.identity => {
                    ensure_split_state_binding(
                        &entry.original,
                        entry.state_digest,
                        "previous output",
                    )?;
                    self.ensure_journal_binding()?;
                    if let Err(error) = move_no_replace(&entry.original, &entry.backup) {
                        return Err(split_transaction_conflict(
                            &format!("the previous output could not be backed up: {error}"),
                            [&entry.original, &entry.backup],
                        ));
                    }
                    if let Err(error) = sync_directory(parent_or_current(&entry.original)) {
                        return Err(split_transaction_conflict(
                            &format!("the backup rename could not be synchronized: {error}"),
                            [&entry.original, &entry.backup],
                        ));
                    }
                    ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
                    ensure_split_state_binding(
                        &entry.backup,
                        entry.state_digest,
                        "previous output backup",
                    )?;
                    ensure_split_missing(&entry.original, "previous output path")?;
                }
                (None, Some(backup)) if backup == entry.identity => {
                    ensure_split_state_binding(
                        &entry.backup,
                        entry.state_digest,
                        "previous output backup",
                    )?;
                }
                (Some(original), Some(backup))
                    if original == entry.identity && backup == entry.identity =>
                {
                    return Err(split_transaction_conflict(
                        "the previous output exists at both its original and backup paths",
                        [&entry.original, &entry.backup],
                    ));
                }
                (Some(original), Some(backup))
                    if Some(original) == installed_identity && backup == entry.identity =>
                {
                    ensure_split_state_binding(
                        &entry.backup,
                        entry.state_digest,
                        "previous output backup",
                    )?;
                }
                (Some(original), Some(backup)) => {
                    return Err(split_transaction_conflict(
                        &format!("an output or backup identity changed ({original:?}, {backup:?})"),
                        [&entry.original, &entry.backup],
                    ));
                }
                (None, None) => {
                    return Err(split_transaction_conflict(
                        "the previous output and its transaction backup are both missing",
                        [&entry.original, &entry.backup],
                    ));
                }
                (Some(original), None) => {
                    return Err(split_transaction_conflict(
                        &format!("the output path is occupied by another identity ({original:?})"),
                        [&entry.original, &entry.backup],
                    ));
                }
                (None, Some(backup)) => {
                    return Err(split_transaction_conflict(
                        &format!("the backup path is occupied by another identity ({backup:?})"),
                        [&entry.original, &entry.backup],
                    ));
                }
            }
        }

        // A resumed transaction can arrive here with some or all old outputs
        // already renamed. Rebind every backup before publishing another output.
        for entry in &transaction.backups {
            ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
            ensure_split_state_binding(
                &entry.backup,
                entry.state_digest,
                "previous output backup",
            )?;
        }

        for entry in &transaction.outputs {
            let staged = observed_split_identity(&entry.staged)?;
            let final_path = observed_split_identity(&entry.final_path)?;
            match (staged, final_path) {
                (Some(staged), None) if staged == entry.identity => {
                    ensure_split_state_binding(
                        &entry.staged,
                        entry.state_digest,
                        "staged split output",
                    )?;
                    self.ensure_journal_binding()?;
                    if let Err(error) = move_no_replace(&entry.staged, &entry.final_path) {
                        return Err(split_transaction_conflict(
                            &format!("the staged output could not be installed: {error}"),
                            [&entry.staged, &entry.final_path],
                        ));
                    }
                    if let Err(error) = sync_directory(parent_or_current(&entry.final_path)) {
                        return Err(split_transaction_conflict(
                            &format!("the output rename could not be synchronized: {error}"),
                            [&entry.staged, &entry.final_path],
                        ));
                    }
                    ensure_split_identity(
                        &entry.final_path,
                        entry.identity,
                        "installed split output",
                    )?;
                    ensure_split_state_binding(
                        &entry.final_path,
                        entry.state_digest,
                        "installed split output",
                    )?;
                    ensure_split_missing(&entry.staged, "split staging path")?;
                }
                (None, Some(final_path)) if final_path == entry.identity => {
                    ensure_split_state_binding(
                        &entry.final_path,
                        entry.state_digest,
                        "installed split output",
                    )?;
                }
                (Some(staged), Some(final_path))
                    if staged == entry.identity && final_path == entry.identity =>
                {
                    return Err(split_transaction_conflict(
                        "the new output exists at both its staging and final paths",
                        [&entry.staged, &entry.final_path],
                    ));
                }
                (Some(staged), Some(final_path)) => {
                    return Err(split_transaction_conflict(
                        &format!(
                            "a staged or final output identity changed ({staged:?}, {final_path:?})"
                        ),
                        [&entry.staged, &entry.final_path],
                    ));
                }
                (None, None) => {
                    return Err(split_transaction_conflict(
                        "the staged and final output are both missing",
                        [&entry.staged, &entry.final_path],
                    ));
                }
                (Some(staged), None) => {
                    return Err(split_transaction_conflict(
                        &format!("the staging path is occupied by another identity ({staged:?})"),
                        [&entry.staged, &entry.final_path],
                    ));
                }
                (None, Some(final_path)) => {
                    return Err(split_transaction_conflict(
                        &format!("the final path is occupied by another identity ({final_path:?})"),
                        [&entry.staged, &entry.final_path],
                    ));
                }
            }
        }

        self.ensure_journal_binding()?;
        self.sync_parent()?;
        for entry in &transaction.backups {
            ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
            ensure_split_state_binding(
                &entry.backup,
                entry.state_digest,
                "previous output backup",
            )?;
            if !output_identities.contains_key(&entry.original) {
                ensure_split_missing(&entry.original, "retired previous output path")?;
            }
        }
        for entry in &transaction.outputs {
            ensure_split_identity(&entry.final_path, entry.identity, "installed split output")?;
            ensure_split_state_binding(
                &entry.final_path,
                entry.state_digest,
                "installed split output",
            )?;
            ensure_split_missing(&entry.staged, "split staging path")?;
        }
        self.ensure_journal_binding()?;
        self.ensure_completed_family()?;
        Ok(transaction
            .backups
            .iter()
            .map(|entry| PreservedSplitOutput {
                path: entry.backup.clone(),
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
            .collect())
    }

    fn sync_parent(&self) -> Result<(), FormatError> {
        let output = self
            .resolved
            .outputs
            .first()
            .ok_or_else(|| FormatError::Other("split transaction has no outputs".into()))?;
        let parent = parent_or_current(&output.final_path);
        sync_directory(parent).map_err(|error| {
            split_transaction_conflict(
                &format!("the completed output set could not be synchronized: {error}"),
                [&output.final_path],
            )
        })
    }

    fn ensure_completed_family(&self) -> Result<(), FormatError> {
        let transaction = &self.resolved;
        let managed =
            collect_managed_split_outputs(&transaction.base, transaction.include_recovery)
                .map_err(|error| {
                    split_transaction_conflict(
                        &format!("the completed output family could not be enumerated: {error}"),
                        [&transaction.base],
                    )
                })?;
        if managed.len() != transaction.outputs.len() {
            return Err(split_transaction_conflict(
                "the completed output family contains a missing or unexpected managed member",
                [&transaction.base],
            ));
        }
        for path in managed {
            let Some(output) = transaction
                .outputs
                .iter()
                .find(|output| crate::same_path_entry(&output.final_path, &path))
            else {
                return Err(split_transaction_conflict(
                    "the completed output family contains an unexpected managed member",
                    [&transaction.base, &path],
                ));
            };
            ensure_split_identity(&path, output.identity, "installed split output")?;
            ensure_split_state_binding(&path, output.state_digest, "installed split output")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitJournalBackup {
    original: StoredOsString,
    backup: StoredOsString,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitJournalOutput {
    staged: StoredOsString,
    final_path: StoredOsString,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitTransactionRecord {
    version: u32,
    base_name: StoredOsString,
    backups: Vec<SplitJournalBackup>,
    outputs: Vec<SplitJournalOutput>,
}

#[derive(Debug)]
struct OpenSplitTransaction {
    path: PathBuf,
    file: File,
    identity: SplitPathIdentity,
    content_digest: [u8; 32],
    record: SplitTransactionRecord,
}

impl OpenSplitTransaction {
    fn clear(self) -> Result<(), FormatError> {
        remove_bound_split_staging(&self).map_err(|error| {
            FormatError::Other(format!(
                "could not securely clear split transaction journal {}: {error}",
                self.path.display()
            ))
        })
    }
}

#[derive(Debug)]
struct ResolvedSplitBackup {
    original: PathBuf,
    backup: PathBuf,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug)]
struct ResolvedSplitOutput {
    staged: PathBuf,
    final_path: PathBuf,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug)]
struct ResolvedSplitTransaction {
    base: PathBuf,
    include_recovery: bool,
    backups: Vec<ResolvedSplitBackup>,
    outputs: Vec<ResolvedSplitOutput>,
}

#[derive(Debug)]
struct SplitTransactionWriteFailure {
    error: FormatError,
    journal_published: bool,
}

impl SplitStagingBinding for OpenSplitTransaction {
    fn staging_path(&self) -> &Path {
        &self.path
    }

    fn staging_identity(&self) -> SplitPathIdentity {
        self.identity
    }

    fn staging_file(&self) -> &File {
        &self.file
    }
}

fn split_journal_name(path: &Path) -> Result<StoredOsString, FormatError> {
    let name = path.file_name().ok_or_else(|| {
        FormatError::Unsupported("split transaction path has no file name".into())
    })?;
    StoredOsString::from_os_str(name)
}

fn split_transaction_record(
    transaction: &ResolvedSplitTransaction,
) -> Result<SplitTransactionRecord, FormatError> {
    let base_name = split_journal_name(&transaction.base)?;
    let backups = transaction
        .backups
        .iter()
        .map(|entry| {
            Ok(SplitJournalBackup {
                original: split_journal_name(&entry.original)?,
                backup: split_journal_name(&entry.backup)?,
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    let outputs = transaction
        .outputs
        .iter()
        .map(|entry| {
            Ok(SplitJournalOutput {
                staged: split_journal_name(&entry.staged)?,
                final_path: split_journal_name(&entry.final_path)?,
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(SplitTransactionRecord {
        version: SPLIT_TRANSACTION_VERSION,
        base_name,
        backups,
        outputs,
    })
}

#[cfg(test)]
fn write_split_transaction(
    base: &Path,
    record: SplitTransactionRecord,
) -> Result<OpenSplitTransaction, FormatError> {
    write_split_transaction_with_state(base, record).map_err(|failure| failure.error)
}

fn write_split_transaction_with_state(
    base: &Path,
    record: SplitTransactionRecord,
) -> Result<OpenSplitTransaction, SplitTransactionWriteFailure> {
    let mut journal_published = false;
    let result = (|| -> Result<OpenSplitTransaction, FormatError> {
        let path = split_transaction_journal_path(base)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(FormatError::Io(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "split transaction journal already exists: {}",
                        path.display()
                    ),
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "split transaction journal exceeds {SPLIT_TRANSACTION_MAX_BYTES} bytes"
            )));
        }
        let parent = parent_or_current(&path);
        let file_name = path
            .file_name()
            .ok_or_else(|| FormatError::Unsupported("split journal has no file name".into()))?;
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            SPLIT_JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let temp_path = parent.join(temp_name);
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
        let file = options.open(&temp_path)?;
        let identity = split_file_identity(&file)?;
        if split_path_identity(&temp_path).ok() != Some(identity) {
            return Err(FormatError::Io(io::Error::other(format!(
                "split transaction staging changed while it was reserved and was left untouched: {}",
                temp_path.display()
            ))));
        }
        let mut temp = SplitStagingPath {
            path: temp_path,
            identity,
            file,
        };
        if let Err(error) = temp
            .file
            .write_all(&bytes)
            .and_then(|()| temp.file.sync_all())
        {
            let cleanup_errors = remove_bound_split_staging(&temp)
                .err()
                .map(|cleanup| vec![cleanup.to_string()])
                .unwrap_or_default();
            return Err(with_split_cleanup_errors(error.into(), cleanup_errors));
        }
        if split_file_identity(&temp.file)? != temp.identity
            || split_path_identity(&temp.path).ok() != Some(temp.identity)
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "split transaction staging changed after writing and was left untouched: {}",
                temp.path.display()
            ))));
        }
        if let Err(error) = crate::move_path_no_replace(&temp.path, &path) {
            let cleanup_errors = remove_bound_split_staging(&temp)
                .err()
                .map(|cleanup| vec![cleanup.to_string()])
                .unwrap_or_default();
            return Err(with_split_cleanup_errors(error.into(), cleanup_errors));
        }
        journal_published = true;
        if split_file_identity(&temp.file)? != temp.identity
            || split_path_identity(&path).ok() != Some(temp.identity)
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "published split transaction journal no longer matches its writer-owned staging file and was left for recovery: {}",
                path.display()
            ))));
        }
        sync_directory(parent).map_err(|error| {
            FormatError::from(io::Error::new(
                error.kind(),
                format!(
                    "published split transaction journal could not be synchronized and was left for recovery at {}: {error}",
                    path.display()
                ),
            ))
        })?;
        Ok(OpenSplitTransaction {
            path,
            file: temp.file,
            identity: temp.identity,
            content_digest: *blake3::hash(&bytes).as_bytes(),
            record,
        })
    })();
    result.map_err(|error| SplitTransactionWriteFailure {
        error,
        journal_published,
    })
}

fn open_split_transaction(base: &Path) -> Result<Option<OpenSplitTransaction>, FormatError> {
    let path = split_transaction_journal_path(base)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(format!(
            "split transaction journal must be a regular file: {}",
            path.display()
        )));
    }
    let mut file = open_regular_file_no_follow_read_write(&path)?;
    let identity = split_file_identity(&file)?;
    if split_path_identity(&path)? != identity {
        return Err(FormatError::Io(io::Error::other(
            "split transaction journal changed while it was opened",
        )));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((SPLIT_TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "split transaction journal exceeds {SPLIT_TRANSACTION_MAX_BYTES} bytes"
        )));
    }
    let record = serde_json::from_slice(&bytes)
        .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    Ok(Some(OpenSplitTransaction {
        path,
        file,
        identity,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    }))
}

fn resolve_split_transaction(
    base: &Path,
    record: &SplitTransactionRecord,
) -> Result<ResolvedSplitTransaction, FormatError> {
    if record.version != SPLIT_TRANSACTION_VERSION {
        return Err(FormatError::Unsupported(format!(
            "unsupported split transaction journal version: {}",
            record.version
        )));
    }
    let base_name = record.base_name.to_os_string()?;
    let parent = parent_or_current(base);
    let recorded_base = parent.join(&base_name);
    let requested_journal = split_transaction_journal_path(base)?;
    let recorded_journal = split_transaction_journal_path(&recorded_base)?;
    if !crate::same_path_entry(&requested_journal, &recorded_journal) {
        return Err(FormatError::Unsupported(
            "split transaction journal belongs to another output family".into(),
        ));
    }
    let base_name_utf8 = base_name.to_str().ok_or_else(|| {
        FormatError::Unsupported("split transaction base name must be UTF-8".into())
    })?;
    let include_recovery = is_sqz_base(&recorded_base);
    let mut original_names = HashSet::new();
    let mut backup_names = HashSet::new();
    let mut staged_names = HashSet::new();
    let mut final_names = HashSet::new();
    let mut backup_identities = HashSet::new();
    let mut output_identities = HashSet::new();
    let mut backups = Vec::with_capacity(record.backups.len());
    for entry in &record.backups {
        let original_name = checked_split_journal_component(&entry.original)?;
        let backup_name = checked_split_journal_component(&entry.backup)?;
        let original_utf8 = original_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction output name must be UTF-8".into())
        })?;
        let backup_utf8 = backup_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction backup name must be UTF-8".into())
        })?;
        if !transaction_backup_matches_output_family(
            &recorded_base,
            original_utf8,
            backup_utf8,
            entry.identity,
            include_recovery,
        ) || split_transaction_output_name(backup_utf8, "split-backup") != Some(original_utf8)
            || !original_names.insert(original_name.clone())
            || !backup_names.insert(backup_name.clone())
            || !backup_identities.insert(entry.identity)
        {
            return Err(FormatError::Unsupported(
                "split transaction journal contains an invalid or duplicate backup".into(),
            ));
        }
        backups.push(ResolvedSplitBackup {
            original: parent.join(original_name),
            backup: parent.join(backup_name),
            identity: entry.identity,
            state_digest: entry.state_digest,
        });
    }
    let mut outputs = Vec::with_capacity(record.outputs.len());
    for entry in &record.outputs {
        let staged_name = checked_split_journal_component(&entry.staged)?;
        let final_name = checked_split_journal_component(&entry.final_path)?;
        let staged_utf8 = staged_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction staging name must be UTF-8".into())
        })?;
        let final_utf8 = final_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction output name must be UTF-8".into())
        })?;
        if !managed_split_output_name(base_name_utf8, final_utf8, include_recovery)
            || !split_staging_matches_final(base_name_utf8, staged_utf8, final_utf8)
            || !staged_names.insert(staged_name.clone())
            || !final_names.insert(final_name.clone())
            || !output_identities.insert(entry.identity)
        {
            return Err(FormatError::Unsupported(
                "split transaction journal contains an invalid or duplicate staged output".into(),
            ));
        }
        outputs.push(ResolvedSplitOutput {
            staged: parent.join(staged_name),
            final_path: parent.join(final_name),
            identity: entry.identity,
            state_digest: entry.state_digest,
        });
    }
    if outputs.is_empty() || !backup_identities.is_disjoint(&output_identities) {
        return Err(FormatError::Unsupported(
            "split transaction journal contains ambiguous output identities".into(),
        ));
    }
    Ok(ResolvedSplitTransaction {
        base: recorded_base,
        include_recovery,
        backups,
        outputs,
    })
}

fn checked_split_journal_component(name: &StoredOsString) -> Result<OsString, FormatError> {
    let name = name.to_os_string()?;
    let path = Path::new(&name);
    if path.file_name() != Some(path.as_os_str())
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err(FormatError::Unsupported(
            "split transaction journal path is not a single file name".into(),
        ));
    }
    Ok(name)
}

fn ensure_split_missing(path: &Path, role: &str) -> Result<(), FormatError> {
    match observed_split_identity(path)? {
        None => Ok(()),
        Some(identity) => Err(split_transaction_conflict(
            &format!("{role} is occupied ({identity:?})"),
            [path, path],
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        bind_preserved_split_outputs, snapshot_managed_split_outputs, split_into_volumes,
        test_support::*, volume_path, ControlToken, SplitStagingId,
    };
    use super::*;

    fn write_split_transaction_fixture(
        base: &Path,
        staged: &[StagedSplitOutput],
    ) -> SplitPublicationTransaction {
        let managed = snapshot_managed_split_outputs(base, false).unwrap();
        let transaction = SplitPublicationTransaction::prepare_new(base, staged, managed).unwrap();
        drop(transaction);
        let open = open_split_transaction(base).unwrap().unwrap();
        SplitPublicationTransaction::from_open(base, open).unwrap()
    }

    #[test]
    fn split_transaction_preserves_and_recovers_a_failed_backup_move() {
        let dir = temp_dir("transaction-backup-failure");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_second = volume_path(&base, 2);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_second, b"old second").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let transaction = write_split_transaction_fixture(&base, &staged);
        let blocked = transaction
            .resolved
            .backups
            .iter()
            .find(|entry| entry.original == old_second)
            .unwrap();
        let mut failed = false;
        let error = transaction
            .resume_with(&mut |from, to| {
                if !failed && from == blocked.original.as_path() && to == blocked.backup.as_path() {
                    failed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "controlled previous-output backup failure",
                    ));
                }
                crate::move_path_no_replace(from, to)
            })
            .unwrap_err();
        assert!(failed, "the exact previous-output backup move did not run");

        assert!(error
            .to_string()
            .contains("previous output could not be backed up"));
        assert!(error.to_string().contains("manual recovery"));
        assert!(!base.exists());
        assert!(!old_first.exists());
        assert_eq!(std::fs::read(&old_second).unwrap(), b"old second");
        assert_eq!(std::fs::read(&old_stale).unwrap(), b"old stale");
        let backups = split_backup_paths(&dir);
        assert_eq!(backups.len(), 2);
        assert_eq!(transaction.verified_backups().len(), 2);
        for output in &transaction.resolved.outputs {
            assert_eq!(
                split_path_identity(&output.staged).unwrap(),
                output.identity
            );
            assert_eq!(
                path_state_digest(&output.staged).unwrap(),
                Some(output.state_digest)
            );
        }
        assert!(split_transaction_journal_path(&base).unwrap().exists());

        drop(transaction);
        let preserved =
            bind_preserved_split_outputs(SplitPublicationTransaction::recover(&base).unwrap())
                .unwrap();
        assert_eq!(preserved.len(), 4);
        let mut previous = preserved
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        previous.sort();
        assert_eq!(
            previous,
            vec![
                b"old first".to_vec(),
                b"old second".to_vec(),
                b"old stale".to_vec(),
                b"old unsplit".to_vec(),
            ]
        );
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(std::fs::read(&old_second).unwrap(), b"new second");
        assert!(!base.exists());
        assert!(!old_stale.exists());
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_transaction_preserves_and_recovers_a_failed_later_install() {
        let dir = temp_dir("transaction-install-failure");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_second = volume_path(&base, 2);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_second, b"old second").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let transaction = write_split_transaction_fixture(&base, &staged);
        let blocked = &transaction.resolved.outputs[1];
        let mut failed = false;
        let error = transaction
            .resume_with(&mut |from, to| {
                if !failed && from == blocked.staged.as_path() && to == blocked.final_path.as_path()
                {
                    failed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "controlled later-volume install failure",
                    ));
                }
                crate::move_path_no_replace(from, to)
            })
            .unwrap_err();
        assert!(failed, "the exact later-volume install move did not run");

        assert!(error
            .to_string()
            .contains("staged output could not be installed"));
        assert!(error.to_string().contains("manual recovery"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(split_path_identity(&old_first).unwrap(), staged[0].identity);
        assert!(!staged[0].part.exists());
        assert_eq!(std::fs::read(&staged[1].part).unwrap(), b"new second");
        assert!(!old_second.exists());
        assert!(!base.exists());
        assert!(!old_stale.exists());
        assert_eq!(split_backup_paths(&dir).len(), 4);
        assert_eq!(transaction.verified_backups().len(), 4);
        assert!(split_transaction_journal_path(&base).unwrap().exists());

        drop(transaction);
        let preserved =
            bind_preserved_split_outputs(SplitPublicationTransaction::recover(&base).unwrap())
                .unwrap();
        let mut previous = preserved
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        previous.sort();
        assert_eq!(
            previous,
            vec![
                b"old first".to_vec(),
                b"old second".to_vec(),
                b"old stale".to_vec(),
                b"old unsplit".to_vec(),
            ]
        );
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(std::fs::read(&old_second).unwrap(), b"new second");
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_recovery_preserves_a_rebound_installed_output() {
        let dir = temp_dir("durable-installed-output-swap");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let published_first = staged[0].final_path.clone();
        let displaced = dir.join("displaced-installed-output");
        let transaction = write_split_transaction_fixture(&base, &staged);
        let backup = transaction.resolved.backups[0].backup.clone();
        crate::move_path_no_replace(&old_first, &backup).unwrap();
        crate::move_path_no_replace(&staged[0].part, &published_first).unwrap();
        crate::move_path_no_replace(&published_first, &displaced).unwrap();
        std::fs::write(&published_first, b"competitor first").unwrap();
        sync_directory(&dir).unwrap();

        drop(transaction);
        let error = SplitPublicationTransaction::recover(&base).unwrap_err();

        let backups = split_backup_paths(&dir);
        assert!(error.to_string().contains("manual recovery"));
        assert!(error.to_string().contains("identity changed"));
        assert_eq!(
            std::fs::read(&published_first).unwrap(),
            b"competitor first"
        );
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"old first");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"new first");
        assert_eq!(std::fs::read(&staged[1].part).unwrap(), b"new second");
        assert!(!staged[1].final_path.exists());
        assert!(error.to_string().contains(&backup.display().to_string()));
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_returns_preserved_transaction_backups_after_install() {
        let dir = temp_dir("commit-cleanup-debt");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let managed = snapshot_managed_split_outputs(&base, false).unwrap();
        let mut preserved = bind_preserved_split_outputs(
            SplitPublicationTransaction::publish(&base, &staged, managed).unwrap(),
        )
        .unwrap();

        assert_eq!(std::fs::read(volume_path(&base, 1)).unwrap(), b"new first");
        assert_eq!(std::fs::read(volume_path(&base, 2)).unwrap(), b"new second");
        assert!(!base.exists());
        assert!(!old_stale.exists());
        preserved.sort();
        let mut backups = split_backup_paths(&dir);
        backups.sort();
        assert_eq!(preserved, backups);
        assert_eq!(preserved.len(), 3);
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old unsplit"));
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old first"));
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old stale"));
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_transaction_preserves_a_backup_replaced_after_install() {
        let dir = temp_dir("commit-cleanup-race");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first"]);
        let staged_part = staged[0].part.clone();
        let displaced_backup = dir.join("displaced-transaction-backup");
        let transaction = write_split_transaction_fixture(&base, &staged);
        let transaction_backup = transaction.resolved.backups[0].backup.clone();
        let mut displaced = false;

        let error = transaction
            .resume_with(&mut |from, to| {
                crate::move_path_no_replace(from, to)?;
                if !displaced && from == staged_part.as_path() && to == old_first.as_path() {
                    displaced = true;
                    crate::move_path_no_replace(&transaction_backup, &displaced_backup)?;
                    std::fs::write(&transaction_backup, b"competitor backup")?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(displaced, "the real staged-to-final move did not run");

        assert!(error
            .to_string()
            .contains("previous output backup identity changed"));
        assert!(error
            .to_string()
            .contains("no competing path was removed or overwritten"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(
            std::fs::read(&transaction_backup).unwrap(),
            b"competitor backup"
        );
        assert_eq!(std::fs::read(&displaced_backup).unwrap(), b"old first");
        assert_eq!(split_path_identity(&old_first).unwrap(), staged[0].identity);
        assert!(!staged[0].part.exists());
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        assert!(transaction.verified_backups().is_empty());
        drop(transaction);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_transaction_preserves_a_backup_missing_after_install() {
        let dir = temp_dir("commit-backup-missing");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first"]);
        let staged_part = staged[0].part.clone();
        let displaced_backup = dir.join("displaced-transaction-backup");
        let transaction = write_split_transaction_fixture(&base, &staged);
        let transaction_backup = transaction.resolved.backups[0].backup.clone();
        let mut displaced = false;

        let error = transaction
            .resume_with(&mut |from, to| {
                crate::move_path_no_replace(from, to)?;
                if !displaced && from == staged_part.as_path() && to == old_first.as_path() {
                    displaced = true;
                    crate::move_path_no_replace(&transaction_backup, &displaced_backup)?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(displaced, "the real staged-to-final move did not run");

        assert!(error
            .to_string()
            .contains("previous output backup is missing"));
        assert!(error
            .to_string()
            .contains("no competing path was removed or overwritten"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(std::fs::read(&displaced_backup).unwrap(), b"old first");
        assert!(!transaction_backup.exists());
        assert_eq!(split_path_identity(&old_first).unwrap(), staged[0].identity);
        assert!(!staged[0].part.exists());
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        assert!(transaction.verified_backups().is_empty());
        drop(transaction);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_transaction_recovers_a_partially_installed_set() {
        let dir = temp_dir("durable-partial-recovery");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        sync_directory(&dir).unwrap();

        let preserved = SplitPublicationTransaction::recover(&base).unwrap();

        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].path, backup);
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert!(!staged.exists());
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_transaction_recovers_through_a_case_alias() {
        let dir = temp_dir("durable-case-alias-recovery");
        let recorded_base = dir.join("Archive.zip");
        let requested_base = dir.join("archive.zip");
        let final_path = volume_path(&recorded_base, 1);
        let requested_final_path = volume_path(&requested_base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        if !crate::same_path_entry(&final_path, &requested_final_path) {
            // A case-only alias cannot address this entry on a case-sensitive volume.
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }

        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: recorded_base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&recorded_base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        sync_directory(&dir).unwrap();

        let preserved = SplitPublicationTransaction::recover(&requested_base).unwrap();

        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].path, backup);
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert!(!staged.exists());
        assert!(!split_transaction_journal_path(&recorded_base)
            .unwrap()
            .exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_recovery_rejects_a_rewritten_backup_with_the_same_length() {
        let dir = temp_dir("durable-rewritten-backup");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old-output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new-output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        std::fs::write(&backup, b"new-backup").unwrap();
        sync_directory(&dir).unwrap();

        let error = SplitPublicationTransaction::recover(&base).unwrap_err();

        assert!(error.to_string().contains("contents changed"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"new-backup");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new-output");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_recovery_rejects_a_rewritten_installed_output_with_the_same_length() {
        let dir = temp_dir("durable-rewritten-output");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new-output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: Vec::new(),
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        assert_eq!(record.version, SPLIT_TRANSACTION_VERSION);
        assert_eq!(
            record.outputs[0].state_digest,
            transaction.outputs[0].state_digest
        );
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&staged, &final_path).unwrap();
        std::fs::write(&final_path, b"bad-output").unwrap();
        sync_directory(&dir).unwrap();

        let error = SplitPublicationTransaction::recover(&base).unwrap_err();

        assert!(error.to_string().contains("contents changed"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"bad-output");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_completion_rejects_a_late_unexpected_family_member() {
        let dir = temp_dir("durable-late-family-member");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        crate::move_path_no_replace(&staged, &final_path).unwrap();
        let late = volume_path(&base, 99);
        std::fs::write(&late, b"late competitor").unwrap();
        sync_directory(&dir).unwrap();

        let error = SplitPublicationTransaction::recover(&base).unwrap_err();

        assert!(error.to_string().contains("unexpected managed member"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&late).unwrap(), b"late competitor");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_transaction_journal_collision_is_not_an_output_conflict() {
        let dir = temp_dir("split-journal-collision-category");
        let base = dir.join("archive.zip");
        let journal = split_transaction_journal_path(&base).unwrap();
        std::fs::write(&journal, b"existing transaction state").unwrap();
        let record = split_transaction_record(&ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: Vec::new(),
            outputs: Vec::new(),
        })
        .unwrap();

        let error = write_split_transaction(&base, record).unwrap_err();

        assert!(matches!(
            &error,
            FormatError::Io(io_error) if io_error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert!(!error.is_output_exists());
        assert_eq!(
            std::fs::read(&journal).unwrap(),
            b"existing transaction state"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_split_journal_publication_securely_discards_writer_owned_staging() {
        let dir = temp_dir("split-journal-collision-cleanup");
        let base = dir.join("archive.zip");
        let journal = split_transaction_journal_path(&base).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();
        let staging_id = SplitStagingId::new();
        let staged = [b"new first".as_slice(), b"new second".as_slice()]
            .into_iter()
            .enumerate()
            .map(|(index, contents)| {
                let final_path = volume_path(&base, index as u64 + 1);
                let (part, mut file) = reserve_test_split_staging_file(&final_path, staging_id);
                file.write_all(contents).unwrap();
                file.sync_all().unwrap();
                StagedSplitOutput {
                    identity: split_file_identity(&file).unwrap(),
                    part,
                    final_path,
                    file,
                }
            })
            .collect::<Vec<_>>();

        let error = SplitPublicationTransaction::publish(&base, &staged, Vec::new()).unwrap_err();

        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(!staged.iter().any(|output| output.part.exists()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_journal_cleanup_preserves_a_rebound_competitor() {
        let dir = temp_dir("split-journal-cleanup-rebound");
        let base = dir.join("archive.zip");
        let record = split_transaction_record(&ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: Vec::new(),
            outputs: Vec::new(),
        })
        .unwrap();
        let open = write_split_transaction(&base, record).unwrap();
        let journal = open.path.clone();
        let displaced = dir.join("displaced-journal");
        crate::move_path_no_replace(&journal, &displaced).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();

        let error = open.clear().unwrap_err();

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebound_split_journal_stops_before_the_first_transaction_move() {
        let dir = temp_dir("split-journal-resume-rebound");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let resolved = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_file_identity(&staged_file).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&resolved).unwrap();
        let transaction = SplitPublicationTransaction::from_open(
            &base,
            write_split_transaction(&base, record).unwrap(),
        )
        .unwrap();
        let journal = transaction.open.path.clone();
        let displaced_journal = dir.join("displaced-journal");
        crate::move_path_no_replace(&journal, &displaced_journal).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();

        let error = transaction.resume().unwrap_err();

        assert!(error.to_string().contains("transaction journal"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old output");
        assert!(!backup.exists());
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(std::fs::metadata(&displaced_journal).unwrap().len() > 0);
        drop(transaction);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rewritten_split_journal_stops_before_the_first_transaction_move() {
        let dir = temp_dir("split-journal-resume-rewrite");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let resolved = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_file_identity(&staged_file).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&resolved).unwrap();
        let transaction = SplitPublicationTransaction::from_open(
            &base,
            write_split_transaction(&base, record).unwrap(),
        )
        .unwrap();
        std::fs::write(&transaction.open.path, b"rewritten journal").unwrap();

        let error = transaction.resume().unwrap_err();

        assert!(error.to_string().contains("transaction journal changed"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old output");
        assert!(!backup.exists());
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        drop(transaction);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn next_split_run_reports_recovered_and_current_transaction_backups() {
        let dir = temp_dir("durable-recovery-debt");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"interrupted output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let old_backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: old_backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged,
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &old_backup).unwrap();
        sync_directory(&dir).unwrap();

        let tmp = dir.join("next.tmp");
        std::fs::write(&tmp, b"current output").unwrap();
        let report = split_into_volumes(&tmp, &base, 1024, &ControlToken::new()).unwrap();

        assert_eq!(report.preserved_outputs.len(), 2);
        assert!(report.preserved_outputs.contains(&old_backup));
        let mut prior_contents = report
            .preserved_outputs
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        prior_contents.sort();
        assert_eq!(
            prior_contents,
            vec![b"interrupted output".to_vec(), b"old output".to_vec()]
        );
        assert_eq!(std::fs::read(&final_path).unwrap(), b"current output");
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_recovery_leaves_competing_output_and_bound_paths_untouched() {
        let dir = temp_dir("durable-recovery-competitor");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        std::fs::write(&final_path, b"late competitor").unwrap();
        sync_directory(&dir).unwrap();
        let competitor_identity = split_path_identity(&final_path).unwrap();

        let error = SplitPublicationTransaction::recover(&base).unwrap_err();

        assert!(error.to_string().contains("manual recovery"));
        assert!(error
            .to_string()
            .contains("verified previous outputs currently remain at"));
        assert!(error
            .to_string()
            .contains(&final_path.display().to_string()));
        assert!(error.to_string().contains(&backup.display().to_string()));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"late competitor");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        assert_eq!(
            split_path_identity(&final_path).unwrap(),
            competitor_identity
        );
        assert_eq!(
            split_path_identity(&backup).unwrap(),
            transaction.backups[0].identity
        );
        assert_eq!(
            path_state_digest(&backup).unwrap(),
            Some(transaction.backups[0].state_digest)
        );
        assert_eq!(
            split_path_identity(&staged).unwrap(),
            transaction.outputs[0].identity
        );
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
