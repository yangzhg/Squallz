use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use squallz_format_api::{ControlToken, EntryPath, FormatError, ProgressSink};

use crate::archive_path::checked_path_component;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow_read_write, path_identity, PathIdentity,
    RegularFileState,
};
use crate::stored_os_string::StoredOsString;

const HASH_BUFFER_BYTES: usize = 256 * 1024;
const JOURNAL_MAX_BYTES: usize = 256 * 1024;
const MAX_OUTPUT_SET_MEMBERS: usize = 64;
const JOURNAL_VERSION: u32 = 1;
static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// A file set whose exact staged members have been synchronized, hashed, and
/// bound to open file handles. Calling [`Self::commit_no_replace`] is the only
/// operation that makes the final names visible.
pub struct PreparedFileSetPublication {
    primary: PathBuf,
    parent: PathBuf,
    parent_file: File,
    parent_identity: PathIdentity,
    holder: PathBuf,
    holder_file: Option<File>,
    holder_identity: PathIdentity,
    members: Vec<PreparedMember>,
}

struct PreparedMember {
    name: OsString,
    staged: PathBuf,
    final_path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    digest: [u8; 32],
}

struct HashProgress<'a> {
    sink: &'a dyn ProgressSink,
    control: &'a ControlToken,
    completed: u64,
    total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalRecord {
    version: u32,
    primary: StoredOsString,
    holder: StoredOsString,
    holder_identity: PathIdentity,
    members: Vec<JournalMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalMember {
    name: StoredOsString,
    identity: PathIdentity,
    state: RegularFileState,
    digest: [u8; 32],
}

struct OpenJournal {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    content_digest: [u8; 32],
    record: JournalRecord,
}

struct JournalWriteFailure {
    error: FormatError,
    published: bool,
}

struct ResolvedMember {
    staged: PathBuf,
    final_path: PathBuf,
    identity: PathIdentity,
    state: RegularFileState,
    digest: [u8; 32],
}

struct ResolvedTransaction {
    parent: PathBuf,
    parent_file: File,
    parent_identity: PathIdentity,
    holder: PathBuf,
    holder_identity: PathIdentity,
    members: Vec<ResolvedMember>,
}

/// Validates and binds an exact set of regular files staged in one private
/// sibling directory.
///
/// `primary` selects the final parent and the member that must be published
/// last. Every staged path must be a direct child of `staging_directory`, and
/// final names are derived from those exact file names. Preparation never
/// changes a final destination.
pub fn prepare_file_set_publication(
    primary: &Path,
    staging_directory: &Path,
    staged_files: &[PathBuf],
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<PreparedFileSetPublication, FormatError> {
    control.checkpoint()?;
    if staged_files.is_empty() {
        return Err(FormatError::Unsupported(
            "file-set publication requires at least one staged file".into(),
        ));
    }
    if staged_files.len() > MAX_OUTPUT_SET_MEMBERS {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "file-set publication exceeds {MAX_OUTPUT_SET_MEMBERS} members"
        )));
    }

    let primary_name = checked_path_component(primary.file_name(), "primary output")?;
    let parent = fs::canonicalize(crate::parent_or_current(primary))?;
    let primary = parent.join(&primary_name);
    let parent_file = crate::open_directory(&parent)?;
    let parent_identity = file_identity(&parent_file)?;
    ensure_directory_binding(&parent, &parent_file, parent_identity, "destination parent")?;

    let holder_parent = fs::canonicalize(crate::parent_or_current(staging_directory))?;
    if holder_parent != parent {
        return Err(FormatError::Unsupported(
            "file-set staging directory must be beside the destination".into(),
        ));
    }
    let holder_metadata = fs::symlink_metadata(staging_directory)?;
    if !holder_metadata.is_dir() || holder_metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(
            "file-set staging path must be a real directory".into(),
        ));
    }
    let holder = fs::canonicalize(staging_directory)?;
    if holder.parent() != Some(parent.as_path()) {
        return Err(FormatError::Unsupported(
            "file-set staging directory must be a direct destination sibling".into(),
        ));
    }
    checked_path_component(holder.file_name(), "staging directory")?;
    let holder_file = crate::open_directory(&holder)?;
    let holder_identity = file_identity(&holder_file)?;
    ensure_directory_binding(&holder, &holder_file, holder_identity, "staging directory")?;

    let mut names = HashSet::with_capacity(staged_files.len());
    let mut members = Vec::with_capacity(staged_files.len());
    let mut total_bytes = 0u64;
    for staged in staged_files {
        control.checkpoint()?;
        let name = checked_path_component(staged.file_name(), "staged output")?;
        let staged_parent = fs::canonicalize(crate::parent_or_current(staged))?;
        if staged_parent != holder || !names.insert(name.clone()) {
            return Err(FormatError::Unsupported(
                "file-set staging contains an outside or duplicate member".into(),
            ));
        }
        let staged = holder.join(&name);
        let final_path = parent.join(&name);
        ensure_output_missing(&final_path)?;
        let file = open_regular_file_no_follow_read_write(&staged)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(FormatError::Unsupported(
                "file-set staging members must be regular files".into(),
            ));
        }
        let identity = file_identity(&file)?;
        if path_identity(&staged)? != identity {
            return Err(binding_error(
                "staged output changed while it was opened",
                [&staged],
            ));
        }
        file.sync_all()?;
        let state = RegularFileState::from_metadata(&file.metadata()?);
        total_bytes = total_bytes.saturating_add(state.bytes());
        members.push(PreparedMember {
            name,
            staged,
            final_path,
            file,
            identity,
            state,
            digest: [0; 32],
        });
    }
    if !names.contains(&primary_name) {
        return Err(FormatError::Unsupported(
            "file-set staging does not contain the primary output".into(),
        ));
    }
    validate_holder_inventory(&holder, &names)?;
    holder_file.sync_all()?;
    parent_file.sync_all()?;

    let mut hash_progress = HashProgress {
        sink: progress,
        control,
        completed: 0,
        total: total_bytes,
    };
    for member in &mut members {
        control.checkpoint()?;
        let current = EntryPath::from_utf8(member.name.to_string_lossy());
        member.digest = hash_prepared_member(member, &mut hash_progress, &current)?;
    }
    control.checkpoint()?;
    ensure_directory_binding(&parent, &parent_file, parent_identity, "destination parent")?;
    ensure_directory_binding(&holder, &holder_file, holder_identity, "staging directory")?;
    validate_holder_inventory(&holder, &names)?;

    members.sort_by(|left, right| left.name.cmp(&right.name));
    if let Some(index) = members
        .iter()
        .position(|member| member.name == primary_name)
    {
        let primary_member = members.remove(index);
        members.push(primary_member);
    }

    Ok(PreparedFileSetPublication {
        primary,
        parent,
        parent_file,
        parent_identity,
        holder,
        holder_file: Some(holder_file),
        holder_identity,
        members,
    })
}

impl PreparedFileSetPublication {
    /// Durably publishes the prepared set without replacing any existing path.
    ///
    /// A journal is synchronized before the first rename. If the process stops
    /// during publication, calling [`recover_file_set_publication`] with the
    /// same primary output resumes the exact bound transaction.
    pub fn commit_no_replace(mut self) -> Result<Vec<PathBuf>, FormatError> {
        let _lock = lock_output_set(&self.primary)?;
        recover_file_set_publication_locked(&self.primary)?;
        self.verify_bindings()?;
        for member in &self.members {
            ensure_output_missing(&member.final_path)?;
        }

        let record = self.journal_record()?;
        let journal = match write_journal(&self.primary, record) {
            Ok(journal) => journal,
            Err(failure) if failure.published => {
                return Err(FormatError::Other(format!(
                    "{}; staged outputs were retained because the file-set publication journal may already be durable",
                    failure.error
                )));
            }
            Err(failure) => return Err(failure.error),
        };

        for member in &self.members {
            ensure_journal_binding(&journal)?;
            verify_prepared_member(member)?;
            ensure_directory_binding(
                &self.parent,
                &self.parent_file,
                self.parent_identity,
                "destination parent",
            )?;
            if let Err(error) = crate::move_path_no_replace(&member.staged, &member.final_path) {
                return Err(publication_recovery_error(
                    &format!("a staged output could not be installed: {error}"),
                    [&journal.path, &member.staged, &member.final_path],
                ));
            }
            self.parent_file.sync_all().map_err(|error| {
                publication_recovery_error(
                    &format!("an output rename could not be synchronized: {error}"),
                    [&journal.path, &member.final_path],
                )
            })?;
            verify_installed_member(member)?;
        }

        ensure_journal_binding(&journal)?;
        for member in &self.members {
            verify_installed_member(member)?;
        }
        ensure_directory_binding(
            &self.holder,
            self.holder_file.as_ref().ok_or_else(|| {
                FormatError::Other("file-set staging directory handle is unavailable".into())
            })?,
            self.holder_identity,
            "staging directory",
        )?;
        ensure_empty_directory(&self.holder)?;
        self.holder_file.take();
        fs::remove_dir(&self.holder).map_err(|error| {
            publication_recovery_error(
                &format!("the empty staging directory could not be removed: {error}"),
                [&journal.path, &self.holder],
            )
        })?;
        self.parent_file.sync_all().map_err(|error| {
            publication_recovery_error(
                &format!("staging cleanup could not be synchronized: {error}"),
                [&journal.path, &self.holder],
            )
        })?;
        clear_journal(journal, &self.parent_file)?;

        let mut outputs = self
            .members
            .iter()
            .map(|member| member.final_path.clone())
            .collect::<Vec<_>>();
        outputs.sort();
        if let Some(index) = outputs.iter().position(|path| *path == self.primary) {
            let primary = outputs.remove(index);
            outputs.insert(0, primary);
        }
        Ok(outputs)
    }

    fn verify_bindings(&self) -> Result<(), FormatError> {
        ensure_directory_binding(
            &self.parent,
            &self.parent_file,
            self.parent_identity,
            "destination parent",
        )?;
        let holder_file = self.holder_file.as_ref().ok_or_else(|| {
            FormatError::Other("file-set staging directory handle is unavailable".into())
        })?;
        ensure_directory_binding(
            &self.holder,
            holder_file,
            self.holder_identity,
            "staging directory",
        )?;
        let names = self
            .members
            .iter()
            .map(|member| member.name.clone())
            .collect::<HashSet<_>>();
        validate_holder_inventory(&self.holder, &names)?;
        for member in &self.members {
            verify_prepared_member(member)?;
        }
        Ok(())
    }

    fn journal_record(&self) -> Result<JournalRecord, FormatError> {
        Ok(JournalRecord {
            version: JOURNAL_VERSION,
            primary: StoredOsString::from_os_string(
                self.primary
                    .file_name()
                    .ok_or_else(|| {
                        FormatError::Unsupported("file-set primary output has no file name".into())
                    })?
                    .to_os_string(),
            )?,
            holder: StoredOsString::from_os_string(
                self.holder
                    .file_name()
                    .ok_or_else(|| {
                        FormatError::Unsupported(
                            "file-set staging directory has no file name".into(),
                        )
                    })?
                    .to_os_string(),
            )?,
            holder_identity: self.holder_identity,
            members: self
                .members
                .iter()
                .map(|member| {
                    Ok(JournalMember {
                        name: StoredOsString::from_os_string(member.name.clone())?,
                        identity: member.identity,
                        state: member.state.clone(),
                        digest: member.digest,
                    })
                })
                .collect::<Result<Vec<_>, FormatError>>()?,
        })
    }
}

/// Resumes an interrupted publication for `primary`, if a journal exists.
/// Returns `true` when a transaction was found and completed.
pub fn recover_file_set_publication(primary: &Path) -> Result<bool, FormatError> {
    let _lock = lock_output_set(primary)?;
    recover_file_set_publication_locked(primary)
}

/// Returns whether a publication journal currently occupies the expected path.
///
/// Callers use this only to decide whether transaction-owned staging must be
/// retained after an error; recovery still validates the journal itself.
pub fn file_set_publication_pending(primary: &Path) -> bool {
    journal_path(primary)
        .ok()
        .is_some_and(|path| fs::symlink_metadata(path).is_ok())
}

fn recover_file_set_publication_locked(primary: &Path) -> Result<bool, FormatError> {
    let Some(journal) = open_journal(primary)? else {
        return Ok(false);
    };
    let transaction = resolve_transaction(primary, &journal.record)?;
    resume_transaction(&transaction, &journal)?;
    clear_journal(journal, &transaction.parent_file)?;
    Ok(true)
}

fn resolve_transaction(
    primary: &Path,
    record: &JournalRecord,
) -> Result<ResolvedTransaction, FormatError> {
    if record.version != JOURNAL_VERSION {
        return Err(FormatError::Unsupported(format!(
            "unsupported file-set publication journal version: {}",
            record.version
        )));
    }
    if record.members.is_empty() || record.members.len() > MAX_OUTPUT_SET_MEMBERS {
        return Err(FormatError::Unsupported(
            "file-set publication journal has an invalid member count".into(),
        ));
    }

    let primary_name = checked_stored_component(&record.primary, "primary output")?;
    let requested_name = checked_path_component(primary.file_name(), "primary output")?;
    if primary_name != requested_name {
        return Err(FormatError::Unsupported(
            "file-set publication journal belongs to another output".into(),
        ));
    }
    let parent = fs::canonicalize(crate::parent_or_current(primary))?;
    let parent_file = crate::open_directory(&parent)?;
    let parent_identity = file_identity(&parent_file)?;
    ensure_directory_binding(&parent, &parent_file, parent_identity, "destination parent")?;

    let holder_name = checked_stored_component(&record.holder, "staging directory")?;
    let holder = parent.join(holder_name);
    let mut names = HashSet::with_capacity(record.members.len());
    let mut identities = Vec::with_capacity(record.members.len());
    let mut members = Vec::with_capacity(record.members.len());
    for member in &record.members {
        let name = checked_stored_component(&member.name, "output member")?;
        if !names.insert(name.clone()) || identities.contains(&member.identity) {
            return Err(FormatError::Unsupported(
                "file-set publication journal contains duplicate members".into(),
            ));
        }
        identities.push(member.identity);
        members.push(ResolvedMember {
            staged: holder.join(&name),
            final_path: parent.join(&name),
            identity: member.identity,
            state: member.state.clone(),
            digest: member.digest,
        });
    }
    if !names.contains(&primary_name)
        || members
            .last()
            .and_then(|member| member.final_path.file_name())
            != Some(primary_name.as_os_str())
    {
        return Err(FormatError::Unsupported(
            "file-set publication journal does not publish its primary output last".into(),
        ));
    }

    Ok(ResolvedTransaction {
        parent,
        parent_file,
        parent_identity,
        holder,
        holder_identity: record.holder_identity,
        members,
    })
}

fn resume_transaction(
    transaction: &ResolvedTransaction,
    journal: &OpenJournal,
) -> Result<(), FormatError> {
    ensure_journal_binding(journal)?;
    ensure_directory_binding(
        &transaction.parent,
        &transaction.parent_file,
        transaction.parent_identity,
        "destination parent",
    )?;

    for member in &transaction.members {
        let staged_identity = observed_identity(&member.staged)?;
        let final_identity = observed_identity(&member.final_path)?;
        match (staged_identity, final_identity) {
            (Some(staged), None) if staged == member.identity => {
                verify_resolved_member(&member.staged, member, false)?;
                ensure_journal_binding(journal)?;
                if let Err(error) = crate::move_path_no_replace(&member.staged, &member.final_path)
                {
                    return Err(publication_recovery_error(
                        &format!("a staged output could not be installed: {error}"),
                        [&journal.path, &member.staged, &member.final_path],
                    ));
                }
                transaction.parent_file.sync_all().map_err(|error| {
                    publication_recovery_error(
                        &format!("an output rename could not be synchronized: {error}"),
                        [&journal.path, &member.final_path],
                    )
                })?;
                verify_resolved_member(&member.final_path, member, true)?;
            }
            (None, Some(final_path)) if final_path == member.identity => {
                verify_resolved_member(&member.final_path, member, true)?;
            }
            (Some(_), Some(_)) => {
                return Err(publication_recovery_error(
                    "an output exists at both its staged and final paths",
                    [&journal.path, &member.staged, &member.final_path],
                ));
            }
            (None, None) => {
                return Err(publication_recovery_error(
                    "an output is missing from both its staged and final paths",
                    [&journal.path, &member.staged, &member.final_path],
                ));
            }
            _ => {
                return Err(publication_recovery_error(
                    "an output path is occupied by another file identity",
                    [&journal.path, &member.staged, &member.final_path],
                ));
            }
        }
    }

    for member in &transaction.members {
        verify_resolved_member(&member.final_path, member, true)?;
        ensure_missing(&member.staged, "staged output")?;
    }
    match fs::symlink_metadata(&transaction.holder) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || path_identity(&transaction.holder)? != transaction.holder_identity
            {
                return Err(publication_recovery_error(
                    "the retained staging directory identity changed",
                    [&journal.path, &transaction.holder],
                ));
            }
            ensure_empty_directory(&transaction.holder)?;
            fs::remove_dir(&transaction.holder).map_err(|error| {
                publication_recovery_error(
                    &format!("the empty staging directory could not be removed: {error}"),
                    [&journal.path, &transaction.holder],
                )
            })?;
            transaction.parent_file.sync_all().map_err(|error| {
                publication_recovery_error(
                    &format!("staging cleanup could not be synchronized: {error}"),
                    [&journal.path, &transaction.holder],
                )
            })?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    ensure_journal_binding(journal)?;
    Ok(())
}

fn hash_prepared_member(
    member: &mut PreparedMember,
    progress: &mut HashProgress<'_>,
    current: &EntryPath,
) -> Result<[u8; 32], FormatError> {
    member.file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; HASH_BUFFER_BYTES];
    let mut file_done = 0u64;
    loop {
        progress.control.checkpoint()?;
        let read = member.file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        let read = read as u64;
        file_done = file_done.saturating_add(read);
        progress.completed = progress.completed.saturating_add(read);
        progress.sink.on_entry_progress(
            progress.completed,
            progress.total,
            current,
            file_done,
            member.state.bytes(),
        );
    }
    if member.state.bytes() == 0 {
        progress
            .sink
            .on_entry_progress(progress.completed, progress.total, current, 0, 0);
    }
    if file_identity(&member.file)? != member.identity
        || path_identity(&member.staged)? != member.identity
        || !member.state.matches(&member.file.metadata()?)
        || !member.state.matches(&fs::symlink_metadata(&member.staged)?)
    {
        return Err(binding_error(
            "staged output changed while it was hashed",
            [&member.staged],
        ));
    }
    Ok(*hasher.finalize().as_bytes())
}

fn hash_file(file: &mut File) -> Result<[u8; 32], FormatError> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; HASH_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn verify_prepared_member(member: &PreparedMember) -> Result<(), FormatError> {
    if file_identity(&member.file)? != member.identity
        || path_identity(&member.staged)? != member.identity
        || !member.state.matches(&member.file.metadata()?)
        || !member.state.matches(&fs::symlink_metadata(&member.staged)?)
    {
        return Err(binding_error(
            "staged output changed after preparation",
            [&member.staged],
        ));
    }
    Ok(())
}

fn verify_installed_member(member: &PreparedMember) -> Result<(), FormatError> {
    let handle_state = RegularFileState::from_metadata(&member.file.metadata()?);
    let path_state = RegularFileState::from_metadata(&fs::symlink_metadata(&member.final_path)?);
    if file_identity(&member.file)? != member.identity
        || path_identity(&member.final_path)? != member.identity
        || !member.state.equivalent_after_rename(&handle_state)
        || !member.state.equivalent_after_rename(&path_state)
    {
        return Err(publication_recovery_error(
            "an installed output no longer matches its prepared file",
            [&member.final_path],
        ));
    }
    Ok(())
}

fn verify_resolved_member(
    path: &Path,
    member: &ResolvedMember,
    moved: bool,
) -> Result<(), FormatError> {
    let mut file = open_regular_file_no_follow_read_write(path).map_err(|error| {
        publication_recovery_error(
            &format!("a transaction output could not be opened: {error}"),
            [path],
        )
    })?;
    let handle_state = RegularFileState::from_metadata(&file.metadata()?);
    let path_state = RegularFileState::from_metadata(&fs::symlink_metadata(path)?);
    let state_matches = if moved {
        member.state.equivalent_after_rename(&handle_state)
            && member.state.equivalent_after_rename(&path_state)
    } else {
        member.state == handle_state && member.state == path_state
    };
    if file_identity(&file)? != member.identity
        || path_identity(path)? != member.identity
        || !state_matches
        || hash_file(&mut file)? != member.digest
    {
        return Err(publication_recovery_error(
            "a transaction output identity or content changed",
            [path],
        ));
    }
    Ok(())
}

fn validate_holder_inventory(
    holder: &Path,
    expected: &HashSet<OsString>,
) -> Result<(), FormatError> {
    let mut observed = HashSet::with_capacity(expected.len());
    for entry in fs::read_dir(holder)? {
        let entry = entry?;
        let name = entry.file_name();
        if !expected.contains(&name) || !observed.insert(name) {
            return Err(FormatError::Unsupported(
                "file-set staging contains an unexpected member".into(),
            ));
        }
    }
    if observed == *expected {
        Ok(())
    } else {
        Err(FormatError::Unsupported(
            "file-set staging is missing an expected member".into(),
        ))
    }
}

fn ensure_empty_directory(path: &Path) -> Result<(), FormatError> {
    if fs::read_dir(path)?.next().is_none() {
        Ok(())
    } else {
        Err(publication_recovery_error(
            "the transaction staging directory contains an unexpected member",
            [path],
        ))
    }
}

fn ensure_directory_binding(
    path: &Path,
    file: &File,
    expected: PathIdentity,
    role: &str,
) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || file_identity(file)? != expected
        || path_identity(path)? != expected
    {
        return Err(binding_error(&format!("{role} identity changed"), [path]));
    }
    Ok(())
}

fn ensure_output_missing(path: &Path) -> Result<(), FormatError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(FormatError::output_exists(path)),
    }
}

fn ensure_missing(path: &Path, role: &str) -> Result<(), FormatError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(publication_recovery_error(
            &format!("{role} is still occupied"),
            [path],
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

fn write_journal(
    primary: &Path,
    record: JournalRecord,
) -> Result<OpenJournal, JournalWriteFailure> {
    let mut published = false;
    let result = (|| -> Result<OpenJournal, FormatError> {
        let path = journal_path(primary)?;
        ensure_output_missing(&path)?;
        let bytes = serde_json::to_vec(&record)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if bytes.len() > JOURNAL_MAX_BYTES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "file-set publication journal exceeds {JOURNAL_MAX_BYTES} bytes"
            )));
        }
        let parent = crate::parent_or_current(&path);
        let file_name = path
            .file_name()
            .ok_or_else(|| FormatError::Unsupported("file-set journal has no file name".into()))?;
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let temp_path = parent.join(temp_name);
        let mut file = create_journal_file(&temp_path)?;
        let identity = file_identity(&file)?;
        if path_identity(&temp_path)? != identity {
            return Err(binding_error(
                "journal staging changed while it was reserved",
                [&temp_path],
            ));
        }
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            let _ = remove_bound_file(&temp_path, &file, identity);
            return Err(error.into());
        }
        if let Err(error) = crate::publish_bound_file_no_replace(&temp_path, &file, identity, &path)
        {
            published = path_identity(&path).ok() == Some(identity);
            if !published {
                let _ = remove_bound_file(&temp_path, &file, identity);
            }
            return Err(error);
        }
        published = true;
        let state = RegularFileState::from_metadata(&file.metadata()?);
        Ok(OpenJournal {
            path,
            file,
            identity,
            state,
            content_digest: *blake3::hash(&bytes).as_bytes(),
            record,
        })
    })();
    result.map_err(|error| JournalWriteFailure { error, published })
}

fn open_journal(primary: &Path) -> Result<Option<OpenJournal>, FormatError> {
    let path = journal_path(primary)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(FormatError::Unsupported(
                "file-set publication journal must be a regular file".into(),
            ))
        }
    }
    let mut file = open_regular_file_no_follow_read_write(&path)?;
    let identity = file_identity(&file)?;
    if path_identity(&path)? != identity {
        return Err(binding_error(
            "file-set journal changed while it was opened",
            [&path],
        ));
    }
    let state = RegularFileState::from_metadata(&file.metadata()?);
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((JOURNAL_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > JOURNAL_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "file-set publication journal exceeds {JOURNAL_MAX_BYTES} bytes"
        )));
    }
    let record = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(OpenJournal {
        path,
        file,
        identity,
        state,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    }))
}

fn ensure_journal_binding(journal: &OpenJournal) -> Result<(), FormatError> {
    if file_identity(&journal.file).ok() != Some(journal.identity)
        || path_identity(&journal.path).ok() != Some(journal.identity)
        || !journal.state.matches(&journal.file.metadata()?)
        || !journal.state.matches(&fs::symlink_metadata(&journal.path)?)
    {
        return Err(publication_recovery_error(
            "the retained publication journal identity changed",
            [&journal.path],
        ));
    }
    let mut reader = journal.file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take((JOURNAL_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > JOURNAL_MAX_BYTES || *blake3::hash(&bytes).as_bytes() != journal.content_digest
    {
        return Err(publication_recovery_error(
            "the retained publication journal contents changed",
            [&journal.path],
        ));
    }
    Ok(())
}

fn clear_journal(journal: OpenJournal, parent: &File) -> Result<(), FormatError> {
    ensure_journal_binding(&journal)?;
    remove_bound_file(&journal.path, &journal.file, journal.identity).map_err(|error| {
        FormatError::Other(format!(
            "could not securely clear file-set publication journal {}: {error}",
            journal.path.display()
        ))
    })?;
    parent.sync_all()?;
    Ok(())
}

fn remove_bound_file(path: &Path, file: &File, identity: PathIdentity) -> io::Result<()> {
    if file_identity(file)? != identity || path_identity(path)? != identity {
        return Err(io::Error::other(
            "writer-owned file identity changed before cleanup",
        ));
    }
    fs::remove_file(path)
}

fn journal_path(primary: &Path) -> Result<PathBuf, FormatError> {
    let name = primary.file_name().ok_or_else(|| {
        FormatError::Unsupported("file-set primary output has no file name".into())
    })?;
    let mut journal_name = OsString::from(".");
    journal_name.push(name);
    journal_name.push(".squallz-output-set.json");
    Ok(crate::parent_or_current(primary).join(journal_name))
}

fn lock_output_set(primary: &Path) -> Result<File, FormatError> {
    let parent = fs::canonicalize(crate::parent_or_current(primary))?;
    let name = checked_path_component(primary.file_name(), "primary output")?;
    let mut identity = blake3::Hasher::new();
    identity.update(parent.as_os_str().to_string_lossy().as_bytes());
    identity.update(b"\0");
    identity.update(name.to_string_lossy().as_bytes());
    let lock_path =
        std::env::temp_dir().join(format!("squallz-output-set-{}.lock", identity.finalize()));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    fs4::FileExt::lock(&lock)?;
    Ok(lock)
}

fn checked_stored_component(name: &StoredOsString, role: &str) -> Result<OsString, FormatError> {
    let name = name.to_os_string()?;
    checked_path_component(Some(&name), role)
}

fn binding_error<const N: usize>(reason: &str, paths: [&Path; N]) -> FormatError {
    let mut paths = paths
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    FormatError::Io(io::Error::other(format!("{reason}: {}", paths.join(", "))))
}

fn publication_recovery_error<const N: usize>(reason: &str, paths: [&Path; N]) -> FormatError {
    let mut paths = paths
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    FormatError::Io(io::Error::other(format!(
        "file-set publication requires recovery: {reason}; no competing path was removed or overwritten: {}",
        paths.join(", ")
    )))
}

fn create_journal_file(path: &Path) -> io::Result<File> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_format_api::NoProgress;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct CancelOnProgress {
        control: Arc<ControlToken>,
    }

    impl ProgressSink for CancelOnProgress {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
            self.control.cancel();
        }
    }

    fn test_dir(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "squallz-output-set-{tag}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn fixture(
        root: &Path,
    ) -> Result<(PathBuf, PathBuf, Vec<PathBuf>), Box<dyn std::error::Error>> {
        fs::create_dir_all(root)?;
        let root = fs::canonicalize(root)?;
        let holder = root.join(".publish.work");
        fs::create_dir(&holder)?;
        let index = holder.join("archive.par2");
        let volume = holder.join("archive.vol00+01.par2");
        fs::write(&index, b"index")?;
        fs::write(&volume, b"volume")?;
        Ok((root.join("archive.par2"), holder, vec![index, volume]))
    }

    #[test]
    fn publishes_a_complete_set_without_replacing_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = test_dir("publish");
        let (primary, holder, staged) = fixture(&root)?;
        let prepared = prepare_file_set_publication(
            &primary,
            &holder,
            &staged,
            &NoProgress,
            &ControlToken::default(),
        )?;
        let outputs = prepared.commit_no_replace()?;

        assert_eq!(outputs.first(), Some(&primary));
        assert_eq!(fs::read(&primary)?, b"index");
        assert_eq!(fs::read(root.join("archive.vol00+01.par2"))?, b"volume");
        assert!(!holder.exists());
        assert!(!file_set_publication_pending(&primary));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn existing_destination_preserves_the_staged_set() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_dir("collision");
        let (primary, holder, staged) = fixture(&root)?;
        fs::write(&primary, b"competitor")?;

        let error = prepare_file_set_publication(
            &primary,
            &holder,
            &staged,
            &NoProgress,
            &ControlToken::default(),
        )
        .err()
        .ok_or("expected output conflict")?;

        assert!(error.is_output_exists());
        assert_eq!(fs::read(&primary)?, b"competitor");
        assert_eq!(fs::read(&staged[0])?, b"index");
        assert_eq!(fs::read(&staged[1])?, b"volume");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn unexpected_staging_member_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_dir("inventory");
        let (primary, holder, staged) = fixture(&root)?;
        fs::write(holder.join("foreign.tmp"), b"foreign")?;

        let error = prepare_file_set_publication(
            &primary,
            &holder,
            &staged,
            &NoProgress,
            &ControlToken::default(),
        )
        .err()
        .ok_or("expected staging inventory rejection")?;

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert!(!primary.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn cancellation_during_hashing_leaves_every_output_staged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_dir("cancel");
        let (primary, holder, staged) = fixture(&root)?;
        fs::write(&staged[0], vec![b'i'; HASH_BUFFER_BYTES + 1])?;
        let control = ControlToken::new();
        let progress = CancelOnProgress {
            control: Arc::clone(&control),
        };

        let error = prepare_file_set_publication(&primary, &holder, &staged, &progress, &control)
            .err()
            .ok_or("expected publication preparation cancellation")?;

        assert!(matches!(error, FormatError::Cancelled));
        assert!(!primary.exists());
        assert!(staged.iter().all(|path| path.is_file()));
        assert!(!file_set_publication_pending(&primary));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn interrupted_transaction_resumes_the_exact_bound_members(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_dir("resume");
        let (primary, holder, staged) = fixture(&root)?;
        let prepared = prepare_file_set_publication(
            &primary,
            &holder,
            &staged,
            &NoProgress,
            &ControlToken::default(),
        )?;
        let record = prepared.journal_record()?;
        let journal = write_journal(&primary, record).map_err(|failure| failure.error)?;
        let first = prepared
            .members
            .first()
            .ok_or("prepared set has no members")?;
        crate::move_path_no_replace(&first.staged, &first.final_path)?;
        prepared.parent_file.sync_all()?;
        drop(journal);
        drop(prepared);

        assert!(recover_file_set_publication(&primary)?);
        assert_eq!(fs::read(&primary)?, b"index");
        assert_eq!(fs::read(root.join("archive.vol00+01.par2"))?, b"volume");
        assert!(!holder.exists());
        assert!(!file_set_publication_pending(&primary));
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
