use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use squallz_format_api::{ControlToken, FormatError, PhysicalFileIdentity};

const JOURNAL_VERSION: u32 = 1;
const JOURNAL_MAX_BYTES: u64 = 16 * 1024;
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);
const JOURNAL_PREFIX: &str = ".squallz-par2-repair-";
const JOURNAL_SUFFIX: &str = ".json";
const WORKSPACE_SUFFIX: &str = ".work";
const OWNER_SUFFIX: &str = ".owner";
const DIGEST_DOMAIN: &[u8] = b"squallz-par2-repair-workspace-journal-v1\0";
const TARGET_DOMAIN: &[u8] = b"squallz-par2-repair-target-v1\0";

static WORKSPACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredIdentity {
    filesystem: u64,
    entry: u64,
}

impl From<PhysicalFileIdentity> for StoredIdentity {
    fn from(identity: PhysicalFileIdentity) -> Self {
        Self {
            filesystem: identity.filesystem(),
            entry: identity.file(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
enum WorkspaceState {
    Reserved {
        owner_token: String,
    },
    Active {
        workspace_identity: StoredIdentity,
        owner_identity: StoredIdentity,
        owner_token: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalRecord {
    version: u32,
    target_key: String,
    parent_identity: StoredIdentity,
    workspace: String,
    owner: String,
    workspace_state: WorkspaceState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    record: JournalRecord,
    digest: String,
}

#[derive(Debug)]
struct OpenJournal {
    path: PathBuf,
    file: File,
    identity: StoredIdentity,
    record: JournalRecord,
}

#[derive(Debug)]
pub(super) struct WorkspaceDebt {
    pub(super) journal: PathBuf,
    pub(super) workspace: Option<PathBuf>,
    pub(super) reason: String,
}

#[derive(Debug)]
pub(super) struct RepairWorkspaceTarget {
    parent: PathBuf,
    parent_file: File,
    parent_identity: StoredIdentity,
    key: String,
    journal: PathBuf,
    journal_temp: PathBuf,
    _lock: File,
}

#[derive(Debug)]
pub(super) struct RepairWorkspace {
    target: RepairWorkspaceTarget,
    journal: OpenJournal,
    path: PathBuf,
    directory: File,
    owner_path: PathBuf,
    owner: File,
    workspace_identity: StoredIdentity,
    owner_identity: StoredIdentity,
    owner_token: String,
}

struct ActiveWorkspaceBinding<'a> {
    workspace_identity: StoredIdentity,
    owner_identity: StoredIdentity,
    owner_token: &'a str,
}

struct ActiveCleanupOperations<R, S, C> {
    remove_workspace: R,
    sync_parent: S,
    clear_journal: C,
}

impl RepairWorkspaceTarget {
    pub(super) fn lock(output: &Path, control: &ControlToken) -> Result<Self, FormatError> {
        let requested_name = checked_output_name(output)?;
        let parent = fs::canonicalize(parent_or_current(output))?;
        let canonical_target = parent.join(requested_name);
        let key = target_key(&canonical_target);
        let lock_path = std::env::temp_dir().join(format!("squallz-par2-repair-{key}.lock"));
        let lock = acquire_lock(&lock_path, control)?;
        let parent_file = squallz_core::open_directory_no_follow(&parent)?;
        let parent_identity = squallz_core::physical_file_identity(&parent_file)?.into();
        let journal = parent.join(format!("{JOURNAL_PREFIX}{key}{JOURNAL_SUFFIX}"));
        let journal_temp = parent.join(format!("{JOURNAL_PREFIX}{key}.tmp"));
        Ok(Self {
            parent,
            parent_file,
            parent_identity,
            key,
            journal,
            journal_temp,
            _lock: lock,
        })
    }

    pub(super) fn recover_pending(&self) -> Result<(), WorkspaceDebt> {
        self.ensure_parent_binding()
            .map_err(|error| self.debt(None, error.to_string()))?;
        self.recover_interrupted_journal_write()?;
        let Some(journal) = self
            .read_journal()
            .map_err(|error| self.debt(None, error.to_string()))?
        else {
            return Ok(());
        };
        let workspace = self.workspace_path(&journal.record.workspace);
        let owner = self.workspace_path(&journal.record.owner);
        let workspace_state = journal.record.workspace_state.clone();
        match workspace_state {
            WorkspaceState::Reserved { owner_token } => {
                self.recover_reserved_workspace(journal, workspace, owner, &owner_token)
            }
            WorkspaceState::Active {
                workspace_identity,
                owner_identity,
                owner_token,
            } => self.cleanup_active_workspace(
                journal,
                workspace,
                owner,
                workspace_identity,
                owner_identity,
                &owner_token,
            ),
        }
    }

    pub(super) fn begin(self) -> Result<RepairWorkspace, WorkspaceDebt> {
        self.ensure_parent_binding()
            .map_err(|error| self.debt(None, error.to_string()))?;
        match self.read_journal() {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(self.debt(
                    None,
                    "an interrupted PAR2 repair record still needs recovery".to_owned(),
                ));
            }
            Err(error) => return Err(self.debt(None, error.to_string())),
        }

        let workspace_name = self.unique_workspace_name();
        let owner_name = format!("{workspace_name}{OWNER_SUFFIX}");
        let workspace = self.workspace_path(&workspace_name);
        let owner_path = self.workspace_path(&owner_name);
        let owner_token = owner_token(&self.key);
        let reserved = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: self.key.clone(),
            parent_identity: self.parent_identity,
            workspace: workspace_name,
            owner: owner_name,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token.clone(),
            },
        };
        let reserved_journal = self
            .write_new_journal(&reserved)
            .map_err(|error| self.debt(None, error.to_string()))?;

        let mut owner = match create_private_file(&owner_path) {
            Ok(file) => file,
            Err(error) => {
                return Err(self.debt(None, error.to_string()));
            }
        };
        if let Err(error) = owner
            .write_all(owner_token.as_bytes())
            .and_then(|()| owner.sync_all())
        {
            return Err(self.debt(None, error.to_string()));
        }
        if let Err(error) = fs4::FileExt::try_lock(&owner).map_err(io::Error::from) {
            return Err(self.debt(None, error.to_string()));
        }
        if let Err(error) = create_private_directory(&workspace) {
            let bound_workspace = fs::symlink_metadata(&workspace).ok().map(|_| workspace);
            return Err(self.debt(bound_workspace, error.to_string()));
        }

        let directory = squallz_core::open_directory_no_follow(&workspace)
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        let workspace_identity = squallz_core::physical_file_identity(&directory)
            .map(StoredIdentity::from)
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        let owner_identity = squallz_core::physical_file_identity(&owner)
            .map(StoredIdentity::from)
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        let active = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: self.key.clone(),
            parent_identity: self.parent_identity,
            workspace: reserved_journal.record.workspace.clone(),
            owner: reserved_journal.record.owner.clone(),
            workspace_state: WorkspaceState::Active {
                workspace_identity,
                owner_identity,
                owner_token: owner_token.clone(),
            },
        };
        let journal = self
            .replace_journal(reserved_journal, &active)
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;

        Ok(RepairWorkspace {
            target: self,
            journal,
            path: workspace,
            directory,
            owner_path,
            owner,
            workspace_identity,
            owner_identity,
            owner_token,
        })
    }

    fn cleanup_active_workspace(
        &self,
        journal: OpenJournal,
        workspace: PathBuf,
        owner_path: PathBuf,
        workspace_identity: StoredIdentity,
        owner_identity: StoredIdentity,
        owner_token: &str,
    ) -> Result<(), WorkspaceDebt> {
        self.cleanup_active_workspace_with(
            journal,
            workspace,
            owner_path,
            ActiveWorkspaceBinding {
                workspace_identity,
                owner_identity,
                owner_token,
            },
            ActiveCleanupOperations {
                remove_workspace: |path: &Path| fs::remove_dir_all(path),
                sync_parent: || self.parent_file.sync_all(),
                clear_journal: |journal| self.clear_journal(journal),
            },
        )
    }

    fn cleanup_active_workspace_with<R, S, C>(
        &self,
        journal: OpenJournal,
        workspace: PathBuf,
        owner_path: PathBuf,
        binding: ActiveWorkspaceBinding<'_>,
        mut operations: ActiveCleanupOperations<R, S, C>,
    ) -> Result<(), WorkspaceDebt>
    where
        R: FnMut(&Path) -> io::Result<()>,
        S: FnMut() -> io::Result<()>,
        C: FnMut(OpenJournal) -> io::Result<()>,
    {
        let ActiveWorkspaceBinding {
            workspace_identity,
            owner_identity,
            owner_token,
        } = binding;
        self.ensure_open_journal_binding(&journal)
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        match fs::symlink_metadata(&workspace) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.cleanup_owner_and_journal(
                    journal,
                    None,
                    owner_path,
                    owner_identity,
                    owner_token,
                );
            }
            Err(error) => return Err(self.debt(Some(workspace), error.to_string())),
            Ok(_) => {}
        }
        let (directory, owner) = self
            .open_bound_workspace(
                &workspace,
                &owner_path,
                workspace_identity,
                owner_identity,
                owner_token,
            )
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        drop(directory);
        match (operations.remove_workspace)(&workspace) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.debt(Some(workspace), error.to_string())),
        }
        (operations.sync_parent)()
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        self.remove_bound_owner(&owner_path, &owner, Some(owner_identity))
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        (operations.sync_parent)()
            .map_err(|error| self.debt(Some(workspace.clone()), error.to_string()))?;
        (operations.clear_journal)(journal)
            .map_err(|error| self.debt(Some(workspace), error.to_string()))
    }

    fn recover_reserved_workspace(
        &self,
        journal: OpenJournal,
        workspace: PathBuf,
        owner_path: PathBuf,
        owner_token: &str,
    ) -> Result<(), WorkspaceDebt> {
        self.ensure_open_journal_binding(&journal)
            .map_err(|error| self.debt(None, error.to_string()))?;
        match fs::symlink_metadata(&workspace) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.debt(Some(workspace), error.to_string())),
            Ok(_) => {
                return Err(self.debt(
                    Some(workspace),
                    "PAR2 repair stopped before the private workspace identity was committed; \
                     the exact path was left untouched"
                        .to_owned(),
                ));
            }
        }
        let owner = match fs::symlink_metadata(&owner_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self
                    .clear_journal(journal)
                    .map_err(|error| self.debt(None, error.to_string()));
            }
            Err(error) => return Err(self.debt(None, error.to_string())),
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(self.debt(
                    None,
                    "reserved PAR2 repair workspace owner is not a regular file".to_owned(),
                ));
            }
            Ok(_) => {
                let mut owner = squallz_core::open_regular_file_no_follow_read_write(&owner_path)
                    .map_err(|error| self.debt(None, error.to_string()))?;
                let metadata = owner
                    .metadata()
                    .map_err(|error| self.debt(None, error.to_string()))?;
                #[cfg(unix)]
                ensure_private_permissions(&metadata.permissions(), false)
                    .map_err(|error| self.debt(None, error.to_string()))?;
                if !metadata.is_file() || metadata.len() > 128 {
                    return Err(self.debt(
                        None,
                        "reserved PAR2 repair workspace owner is invalid".to_owned(),
                    ));
                }
                let identity = StoredIdentity::from(
                    squallz_core::physical_file_identity(&owner)
                        .map_err(|error| self.debt(None, error.to_string()))?,
                );
                let mut token = String::new();
                Read::by_ref(&mut owner)
                    .take(129)
                    .read_to_string(&mut token)
                    .map_err(|error| self.debt(None, error.to_string()))?;
                if token != owner_token
                    || StoredIdentity::from(
                        squallz_core::physical_path_identity(&owner_path)
                            .map_err(|error| self.debt(None, error.to_string()))?,
                    ) != identity
                {
                    return Err(self.debt(
                        None,
                        "reserved PAR2 repair workspace owner changed and was left untouched"
                            .to_owned(),
                    ));
                }
                fs4::FileExt::try_lock(&owner)
                    .map_err(io::Error::from)
                    .map_err(|error| self.debt(None, error.to_string()))?;
                owner
            }
        };
        self.remove_bound_owner(&owner_path, &owner, None)
            .map_err(|error| self.debt(None, error.to_string()))?;
        self.parent_file
            .sync_all()
            .map_err(|error| self.debt(None, error.to_string()))?;
        self.clear_journal(journal)
            .map_err(|error| self.debt(None, error.to_string()))
    }

    fn cleanup_owner_and_journal(
        &self,
        journal: OpenJournal,
        workspace: Option<PathBuf>,
        owner_path: PathBuf,
        owner_identity: StoredIdentity,
        owner_token: &str,
    ) -> Result<(), WorkspaceDebt> {
        let owner = match fs::symlink_metadata(&owner_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(self.debt(workspace, error.to_string())),
            Ok(_) => Some(
                self.open_bound_owner(&owner_path, owner_identity, owner_token)
                    .map_err(|error| self.debt(workspace.clone(), error.to_string()))?,
            ),
        };
        if let Some(owner) = owner {
            self.remove_bound_owner(&owner_path, &owner, Some(owner_identity))
                .map_err(|error| self.debt(workspace.clone(), error.to_string()))?;
            self.parent_file
                .sync_all()
                .map_err(|error| self.debt(workspace.clone(), error.to_string()))?;
        }
        self.clear_journal(journal)
            .map_err(|error| self.debt(workspace, error.to_string()))
    }

    fn remove_bound_owner(
        &self,
        owner_path: &Path,
        owner: &File,
        expected: Option<StoredIdentity>,
    ) -> io::Result<()> {
        let file_identity = StoredIdentity::from(squallz_core::physical_file_identity(owner)?);
        if expected.is_some_and(|identity| identity != file_identity)
            || StoredIdentity::from(squallz_core::physical_path_identity(owner_path)?)
                != file_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace owner changed and was left untouched",
            ));
        }
        match fs::remove_file(owner_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn open_bound_workspace(
        &self,
        workspace: &Path,
        owner_path: &Path,
        workspace_identity: StoredIdentity,
        owner_identity: StoredIdentity,
        owner_token: &str,
    ) -> io::Result<(File, File)> {
        self.ensure_parent_binding()?;
        let directory = squallz_core::open_directory_no_follow(workspace)?;
        #[cfg(unix)]
        ensure_private_permissions(&directory.metadata()?.permissions(), true)?;
        if StoredIdentity::from(squallz_core::physical_file_identity(&directory)?)
            != workspace_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace identity changed; it was left untouched",
            ));
        }
        let owner = self.open_bound_owner(owner_path, owner_identity, owner_token)?;
        if StoredIdentity::from(squallz_core::physical_path_identity(workspace)?)
            != workspace_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace changed during verification; it was left untouched",
            ));
        }
        if StoredIdentity::from(squallz_core::physical_path_identity(owner_path)?) != owner_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace owner changed during verification",
            ));
        }
        Ok((directory, owner))
    }

    fn open_bound_owner(
        &self,
        owner_path: &Path,
        owner_identity: StoredIdentity,
        owner_token: &str,
    ) -> io::Result<File> {
        let mut owner = squallz_core::open_regular_file_no_follow_read_write(owner_path)?;
        let metadata = owner.metadata()?;
        #[cfg(unix)]
        ensure_private_permissions(&metadata.permissions(), false)?;
        if !metadata.is_file()
            || metadata.len() > 128
            || StoredIdentity::from(squallz_core::physical_file_identity(&owner)?) != owner_identity
            || StoredIdentity::from(squallz_core::physical_path_identity(owner_path)?)
                != owner_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace owner changed; it was left untouched",
            ));
        }
        let mut token = String::new();
        Read::by_ref(&mut owner)
            .take(129)
            .read_to_string(&mut token)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("could not verify the private workspace owner: {error}"),
                )
            })?;
        if token != owner_token {
            return Err(io::Error::other(
                "private PAR2 repair workspace owner token changed; it was left untouched",
            ));
        }
        fs4::FileExt::try_lock(&owner)
            .map_err(io::Error::from)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("private PAR2 repair workspace is still in use: {error}"),
                )
            })?;
        if StoredIdentity::from(squallz_core::physical_path_identity(owner_path)?) != owner_identity
        {
            return Err(io::Error::other(
                "private PAR2 repair workspace owner changed during verification",
            ));
        }
        Ok(owner)
    }

    fn read_journal(&self) -> io::Result<Option<OpenJournal>> {
        self.read_journal_at(&self.journal)
    }

    fn read_journal_at(&self, path: &Path) -> io::Result<Option<OpenJournal>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record is not a regular file",
            ));
        }
        let mut file = squallz_core::open_regular_file_no_follow(path)?;
        let file_metadata = file.metadata()?;
        #[cfg(unix)]
        ensure_private_permissions(&file_metadata.permissions(), false)?;
        if !file_metadata.is_file() || file_metadata.len() > JOURNAL_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record is invalid or too large",
            ));
        }
        let identity = StoredIdentity::from(squallz_core::physical_file_identity(&file)?);
        if StoredIdentity::from(squallz_core::physical_path_identity(path)?) != identity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record identity changed while opening it",
            ));
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(JOURNAL_MAX_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > JOURNAL_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record is too large",
            ));
        }
        let document: JournalDocument = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.validate_document(&document)?;
        Ok(Some(OpenJournal {
            path: path.to_path_buf(),
            file,
            identity,
            record: document.record,
        }))
    }

    fn write_new_journal(&self, record: &JournalRecord) -> io::Result<OpenJournal> {
        let bytes = journal_bytes(record)?;
        let temp = self.journal_temp.clone();
        let mut file = create_private_file(&temp)?;
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        if let Err(error) = squallz_core::move_path_no_replace(&temp, &self.journal) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        self.parent_file.sync_all()?;
        self.read_journal()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "PAR2 repair recovery record disappeared after it was created",
            )
        })
    }

    fn replace_journal(
        &self,
        current: OpenJournal,
        record: &JournalRecord,
    ) -> io::Result<OpenJournal> {
        self.replace_journal_with(
            current,
            record,
            &mut |from, to| squallz_core::replace_file_atomically(from, to),
            &mut || self.parent_file.sync_all(),
        )
    }

    fn replace_journal_with<R, S>(
        &self,
        current: OpenJournal,
        record: &JournalRecord,
        replace: &mut R,
        sync_parent: &mut S,
    ) -> io::Result<OpenJournal>
    where
        R: FnMut(&Path, &Path) -> io::Result<()>,
        S: FnMut() -> io::Result<()>,
    {
        self.ensure_open_journal_binding(&current)?;
        let bytes = journal_bytes(record)?;
        let temp = self.journal_temp.clone();
        let mut file = create_private_file(&temp)?;
        let written = file.write_all(&bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        replace(&temp, &self.journal)?;
        sync_parent()?;
        self.read_journal()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "PAR2 repair recovery record disappeared after it was updated",
            )
        })
    }

    fn clear_journal(&self, journal: OpenJournal) -> io::Result<()> {
        self.clear_journal_with(
            journal,
            &mut |path: &Path| fs::remove_file(path),
            &mut || self.parent_file.sync_all(),
        )
    }

    fn clear_journal_with<R, S>(
        &self,
        journal: OpenJournal,
        remove: &mut R,
        sync_parent: &mut S,
    ) -> io::Result<()>
    where
        R: FnMut(&Path) -> io::Result<()>,
        S: FnMut() -> io::Result<()>,
    {
        self.ensure_parent_binding()?;
        match self.ensure_open_journal_binding(&journal) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        match remove(&journal.path) {
            Ok(()) => sync_parent(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn ensure_open_journal_binding(&self, journal: &OpenJournal) -> io::Result<()> {
        let reopened = self.read_journal_at(&journal.path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "PAR2 repair recovery record disappeared",
            )
        })?;
        if StoredIdentity::from(squallz_core::physical_file_identity(&journal.file)?)
            != journal.identity
            || reopened.identity != journal.identity
            || reopened.record != journal.record
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record identity or content changed and was left untouched",
            ));
        }
        Ok(())
    }

    fn recover_interrupted_journal_write(&self) -> Result<(), WorkspaceDebt> {
        let Some(temp) = self.read_journal_at(&self.journal_temp).map_err(|error| {
            self.debt_for(
                self.journal_temp.clone(),
                None,
                format!(
                    "temporary PAR2 repair recovery record {} needs attention: {error}",
                    self.journal_temp.display()
                ),
            )
        })?
        else {
            return Ok(());
        };
        let primary = self
            .read_journal()
            .map_err(|error| self.debt(None, error.to_string()))?;
        match primary {
            None => {
                self.ensure_open_journal_binding(&temp).map_err(|error| {
                    self.debt_for(self.journal_temp.clone(), None, error.to_string())
                })?;
                squallz_core::move_path_no_replace(&temp.path, &self.journal).map_err(|error| {
                    self.debt_for(self.journal_temp.clone(), None, error.to_string())
                })?;
                self.parent_file
                    .sync_all()
                    .map_err(|error| self.debt(None, error.to_string()))
            }
            Some(primary) if primary.record == temp.record => {
                self.ensure_open_journal_binding(&temp).map_err(|error| {
                    self.debt_for(self.journal_temp.clone(), None, error.to_string())
                })?;
                fs::remove_file(&temp.path).map_err(|error| {
                    self.debt_for(self.journal_temp.clone(), None, error.to_string())
                })?;
                self.parent_file
                    .sync_all()
                    .map_err(|error| self.debt(None, error.to_string()))
            }
            Some(primary) if journal_can_advance_from_reserved(&primary.record, &temp.record) => {
                self.ensure_open_journal_binding(&primary)
                    .map_err(|error| self.debt(None, error.to_string()))?;
                self.ensure_open_journal_binding(&temp).map_err(|error| {
                    self.debt_for(self.journal_temp.clone(), None, error.to_string())
                })?;
                squallz_core::replace_file_atomically(&temp.path, &self.journal).map_err(
                    |error| self.debt_for(self.journal_temp.clone(), None, error.to_string()),
                )?;
                self.parent_file
                    .sync_all()
                    .map_err(|error| self.debt(None, error.to_string()))
            }
            Some(_) => Err(self.debt_for(
                self.journal_temp.clone(),
                None,
                format!(
                    "temporary PAR2 repair recovery record {} conflicts with the active record and \
                     both were left untouched",
                    self.journal_temp.display()
                ),
            )),
        }
    }

    fn validate_document(&self, document: &JournalDocument) -> io::Result<()> {
        if document.record.version != JOURNAL_VERSION
            || document.record.target_key != self.key
            || document.record.parent_identity != self.parent_identity
            || !is_reserved_workspace_name(&document.record.workspace, &self.key)
            || document.record.owner != format!("{}{OWNER_SUFFIX}", document.record.workspace)
            || document.digest != journal_digest(&document.record)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PAR2 repair recovery record is damaged, belongs to another target, or was moved",
            ));
        }
        match &document.record.workspace_state {
            WorkspaceState::Reserved { owner_token } => {
                if !is_hex_digest(owner_token) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "PAR2 repair recovery record contains an invalid workspace owner",
                    ));
                }
            }
            WorkspaceState::Active {
                owner_token,
                workspace_identity,
                owner_identity,
            } if !is_hex_digest(owner_token)
                || invalid_identity(*workspace_identity)
                || invalid_identity(*owner_identity) =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PAR2 repair recovery record contains an invalid workspace binding",
                ));
            }
            WorkspaceState::Active { .. } => {}
        }
        Ok(())
    }

    fn ensure_parent_binding(&self) -> io::Result<()> {
        if StoredIdentity::from(squallz_core::physical_file_identity(&self.parent_file)?)
            != self.parent_identity
            || StoredIdentity::from(squallz_core::physical_path_identity(&self.parent)?)
                != self.parent_identity
        {
            return Err(io::Error::other(
                "PAR2 repair output directory changed and its recovery record was left untouched",
            ));
        }
        Ok(())
    }

    fn unique_workspace_name(&self) -> String {
        format!(
            "{JOURNAL_PREFIX}{}-{}-{}{}",
            &self.key[..16],
            std::process::id(),
            unique_nonce(),
            WORKSPACE_SUFFIX
        )
    }

    fn workspace_path(&self, name: &str) -> PathBuf {
        self.parent.join(name)
    }

    fn debt(&self, workspace: Option<PathBuf>, reason: String) -> WorkspaceDebt {
        self.debt_for(self.journal.clone(), workspace, reason)
    }

    fn debt_for(
        &self,
        journal: PathBuf,
        workspace: Option<PathBuf>,
        reason: String,
    ) -> WorkspaceDebt {
        WorkspaceDebt {
            journal,
            workspace,
            reason,
        }
    }

    #[cfg(test)]
    fn journal_path(&self) -> &Path {
        &self.journal
    }
}

impl RepairWorkspace {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn cleanup(self) -> Result<(), WorkspaceDebt> {
        self.target
            .ensure_open_journal_binding(&self.journal)
            .map_err(|error| self.target.debt(Some(self.path.clone()), error.to_string()))?;
        if StoredIdentity::from(
            squallz_core::physical_file_identity(&self.directory)
                .map_err(|error| self.target.debt(Some(self.path.clone()), error.to_string()))?,
        ) != self.workspace_identity
            || StoredIdentity::from(
                squallz_core::physical_file_identity(&self.owner).map_err(|error| {
                    self.target.debt(Some(self.path.clone()), error.to_string())
                })?,
            ) != self.owner_identity
        {
            return Err(self.target.debt(
                Some(self.path),
                "private PAR2 repair workspace handles changed; the paths were left untouched"
                    .to_owned(),
            ));
        }
        drop(self.owner);
        drop(self.directory);
        self.target.cleanup_active_workspace(
            self.journal,
            self.path,
            self.owner_path,
            self.workspace_identity,
            self.owner_identity,
            &self.owner_token,
        )
    }
}

fn checked_output_name(output: &Path) -> Result<&OsStr, FormatError> {
    let name = output.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 repair output must name a file or directory: {}",
            output.display()
        ))
    })?;
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(name)
    } else {
        Err(FormatError::Unsupported(
            "PAR2 repair output has an invalid file name".to_owned(),
        ))
    }
}

fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn target_key(target: &Path) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(TARGET_DOMAIN);
    hasher.update(target.as_os_str().as_encoded_bytes());
    hasher.finalize().to_hex().to_string()
}

fn owner_token(key: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(key.as_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&unique_nonce().to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

fn journal_bytes(record: &JournalRecord) -> io::Result<Vec<u8>> {
    let document = JournalDocument {
        record: record.clone(),
        digest: journal_digest(record)?,
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() as u64 > JOURNAL_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PAR2 repair recovery record is too large",
        ));
    }
    Ok(bytes)
}

fn journal_digest(record: &JournalRecord) -> io::Result<String> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(&bytes);
    Ok(hasher.finalize().to_hex().to_string())
}

fn journal_can_advance_from_reserved(current: &JournalRecord, next: &JournalRecord) -> bool {
    current.version == next.version
        && current.target_key == next.target_key
        && current.parent_identity == next.parent_identity
        && current.workspace == next.workspace
        && current.owner == next.owner
        && matches!(
            (&current.workspace_state, &next.workspace_state),
            (
                WorkspaceState::Reserved {
                    owner_token: current_token,
                },
                WorkspaceState::Active {
                    owner_token: next_token,
                    ..
                }
            ) if current_token == next_token
        )
}

fn is_reserved_workspace_name(name: &str, key: &str) -> bool {
    let Some(rest) = name.strip_prefix(JOURNAL_PREFIX) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix(&key[..16]) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('-') else {
        return false;
    };
    let Some(numbers) = rest.strip_suffix(WORKSPACE_SUFFIX) else {
        return false;
    };
    let mut parts = numbers.split('-');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(pid), Some(nonce), None)
            if !pid.is_empty()
                && !nonce.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && nonce.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid_identity(identity: StoredIdentity) -> bool {
    identity.filesystem == 0 && identity.entry == 0
}

fn acquire_lock(path: &Path, control: &ControlToken) -> Result<File, FormatError> {
    let file = match create_private_file(path) {
        Ok(file) => {
            file.sync_all()?;
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            squallz_core::open_regular_file_no_follow_read_write(path)?
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    ensure_private_permissions(&metadata.permissions(), false)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || squallz_core::physical_path_identity(path)?
            != squallz_core::physical_file_identity(&file)?
    {
        return Err(FormatError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "PAR2 repair target lock is not a stable regular file",
        )));
    }
    loop {
        control.checkpoint()?;
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => break,
            Err(fs4::TryLockError::WouldBlock) => {
                std::thread::sleep(LOCK_POLL_INTERVAL);
            }
            Err(fs4::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || squallz_core::physical_path_identity(path)?
            != squallz_core::physical_file_identity(&file)?
    {
        return Err(FormatError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "PAR2 repair target lock changed while it was being acquired",
        )));
    }
    Ok(file)
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    let result = {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)
    };
    #[cfg(not(unix))]
    let result = fs::create_dir(path);
    result
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    options.open(path)
}

fn unique_nonce() -> u64 {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ WORKSPACE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

#[cfg(unix)]
fn ensure_private_permissions(permissions: &fs::Permissions, directory: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = permissions.mode();
    let required = if directory { 0o700 } else { 0o600 };
    if mode & 0o077 != 0 || mode & required != required {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "PAR2 repair private state permissions are unsafe",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::Instant;

    const CRASH_WORKER_MODE: &str = "SQUALLZ_PAR2_CRASH_WORKER_MODE";
    const CRASH_WORKER_ROOT: &str = "SQUALLZ_PAR2_CRASH_WORKER_ROOT";
    const CRASH_WORKER_TEST: &str = "repair_workspace::tests::par2_repair_forced_kill_worker";
    const CRASH_WORKER_TIMEOUT: Duration = Duration::from_secs(10);
    const CRASH_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(20);

    fn test_root(tag: &str) -> io::Result<PathBuf> {
        let root = std::env::temp_dir().join(format!(
            "squallz-repair-workspace-{tag}-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn debt_result<T>(result: Result<T, WorkspaceDebt>) -> io::Result<T> {
        result.map_err(|debt| io::Error::other(debt.reason))
    }

    fn lock_path(key: &str) -> PathBuf {
        std::env::temp_dir().join(format!("squallz-par2-repair-{key}.lock"))
    }

    fn write_test_journal(path: &Path, record: &JournalRecord) -> io::Result<()> {
        let mut journal = create_private_file(path)?;
        journal.write_all(&journal_bytes(record)?)?;
        journal.sync_all()
    }

    fn rewrite_test_journal(path: &Path, record: &JournalRecord) -> io::Result<()> {
        let mut journal = squallz_core::open_regular_file_no_follow_read_write(path)?;
        journal.set_len(0)?;
        journal.write_all(&journal_bytes(record)?)?;
        journal.sync_all()
    }

    struct CrashWorker {
        child: Option<Child>,
        mode: &'static str,
    }

    impl CrashWorker {
        fn spawn(root: &Path, mode: &'static str) -> io::Result<Self> {
            let child = Command::new(std::env::current_exe()?)
                .arg(CRASH_WORKER_TEST)
                .arg("--exact")
                .arg("--nocapture")
                .env(CRASH_WORKER_MODE, mode)
                .env(CRASH_WORKER_ROOT, root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            Ok(Self {
                child: Some(child),
                mode,
            })
        }

        fn wait_until_ready(&mut self, ready: &Path) -> io::Result<()> {
            let started = Instant::now();
            loop {
                if ready.is_file() {
                    return Ok(());
                }
                let child = self.child.as_mut().ok_or_else(|| {
                    io::Error::other(format!(
                        "{} crash worker no longer has a child process",
                        self.mode
                    ))
                })?;
                if let Some(status) = child.try_wait()? {
                    self.child = None;
                    return Err(io::Error::other(format!(
                        "{} crash worker exited before becoming ready: {status}",
                        self.mode
                    )));
                }
                if started.elapsed() >= CRASH_WORKER_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "{} crash worker did not become ready within {:?}",
                            self.mode, CRASH_WORKER_TIMEOUT
                        ),
                    ));
                }
                std::thread::sleep(CRASH_WORKER_POLL_INTERVAL);
            }
        }

        fn force_kill_and_wait(&mut self) -> io::Result<ExitStatus> {
            let mut child = self.child.take().ok_or_else(|| {
                io::Error::other(format!(
                    "{} crash worker no longer has a child process",
                    self.mode
                ))
            })?;
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            child.kill()?;
            child.wait()
        }
    }

    impl Drop for CrashWorker {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn write_synced_test_file(path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = create_private_file(path)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    struct PreparedActiveTransition {
        root: PathBuf,
        output: PathBuf,
        key: String,
        target: RepairWorkspaceTarget,
        reserved: OpenJournal,
        active: JournalRecord,
        workspace: PathBuf,
        owner_path: PathBuf,
        owner: File,
        directory: File,
    }

    fn prepared_active_transition(
        tag: &str,
    ) -> Result<PreparedActiveTransition, Box<dyn std::error::Error>> {
        let root = test_root(tag)?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace_name = target.unique_workspace_name();
        let owner_name = format!("{workspace_name}{OWNER_SUFFIX}");
        let workspace = target.workspace_path(&workspace_name);
        let owner_path = target.workspace_path(&owner_name);
        let token = owner_token(&target.key);
        let reserved_record = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: workspace_name.clone(),
            owner: owner_name.clone(),
            workspace_state: WorkspaceState::Reserved {
                owner_token: token.clone(),
            },
        };
        let reserved = target.write_new_journal(&reserved_record)?;

        let mut owner = create_private_file(&owner_path)?;
        owner.write_all(token.as_bytes())?;
        owner.sync_all()?;
        fs4::FileExt::try_lock(&owner).map_err(io::Error::from)?;
        create_private_directory(&workspace)?;
        let directory = squallz_core::open_directory_no_follow(&workspace)?;
        let owner_identity = StoredIdentity::from(squallz_core::physical_file_identity(&owner)?);
        let workspace_identity =
            StoredIdentity::from(squallz_core::physical_file_identity(&directory)?);
        write_synced_test_file(&workspace.join("private.bin"), b"private repair data")?;
        let active = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: workspace_name,
            owner: owner_name,
            workspace_state: WorkspaceState::Active {
                workspace_identity,
                owner_identity,
                owner_token: token,
            },
        };
        Ok(PreparedActiveTransition {
            root,
            output,
            key,
            target,
            reserved,
            active,
            workspace,
            owner_path,
            owner,
            directory,
        })
    }

    fn wait_for_forced_termination<T>(_held_state: &T, ready: &Path) -> io::Result<()> {
        write_synced_test_file(ready, b"ready")?;
        loop {
            std::thread::park();
        }
    }

    fn run_active_crash_worker(root: &Path, ready: &Path) -> io::Result<()> {
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())
            .map_err(io::Error::other)?;
        debt_result(target.recover_pending())?;
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        write_synced_test_file(&workspace_path.join("private.bin"), b"private repair data")?;
        wait_for_forced_termination(&workspace, ready)
    }

    fn run_active_journal_temp_crash_worker(root: &Path, ready: &Path) -> io::Result<()> {
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())
            .map_err(io::Error::other)?;
        debt_result(target.recover_pending())?;
        let workspace_name = target.unique_workspace_name();
        let owner_name = format!("{workspace_name}{OWNER_SUFFIX}");
        let workspace_path = target.workspace_path(&workspace_name);
        let owner_path = target.workspace_path(&owner_name);
        let token = owner_token(&target.key);
        let reserved = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: workspace_name.clone(),
            owner: owner_name.clone(),
            workspace_state: WorkspaceState::Reserved {
                owner_token: token.clone(),
            },
        };
        drop(target.write_new_journal(&reserved)?);

        let mut owner = create_private_file(&owner_path)?;
        owner.write_all(token.as_bytes())?;
        owner.sync_all()?;
        fs4::FileExt::try_lock(&owner).map_err(io::Error::from)?;
        create_private_directory(&workspace_path)?;
        let directory = squallz_core::open_directory_no_follow(&workspace_path)?;
        let owner_identity = StoredIdentity::from(squallz_core::physical_file_identity(&owner)?);
        let workspace_identity =
            StoredIdentity::from(squallz_core::physical_file_identity(&directory)?);
        write_synced_test_file(&workspace_path.join("private.bin"), b"private repair data")?;
        let active = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: workspace_name,
            owner: owner_name,
            workspace_state: WorkspaceState::Active {
                workspace_identity,
                owner_identity,
                owner_token: token,
            },
        };
        write_test_journal(&target.journal_temp, &active)?;

        wait_for_forced_termination(&(target, owner, directory), ready)
    }

    #[test]
    fn par2_repair_forced_kill_worker() -> Result<(), Box<dyn std::error::Error>> {
        let Some(mode) = std::env::var_os(CRASH_WORKER_MODE) else {
            return Ok(());
        };
        let mode = mode.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "crash worker mode is not valid UTF-8",
            )
        })?;
        let root = std::env::var_os(CRASH_WORKER_ROOT)
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("missing crash worker environment variable {CRASH_WORKER_ROOT}"),
                )
            })?;
        let ready = root.join("worker.ready");
        match mode {
            "active" => run_active_crash_worker(&root, &ready)?,
            "active-journal-temp" => run_active_journal_temp_crash_worker(&root, &ready)?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown crash worker mode {mode}"),
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn forced_process_termination_releases_locks_and_replays_durable_workspace_states(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["active", "active-journal-temp"] {
            let root = test_root(mode)?;
            let output = root.join("archive.repaired.zip");
            let ready = root.join("worker.ready");
            let unrelated = root.join("unrelated.bin");
            fs::write(&unrelated, b"do not touch")?;

            let mut worker = CrashWorker::spawn(&root, mode)?;
            worker.wait_until_ready(&ready)?;
            assert_eq!(fs::read(&ready)?, b"ready");
            let status = worker.force_kill_and_wait()?;
            assert!(
                !status.success(),
                "{mode} crash worker exited successfully instead of being terminated"
            );

            let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
            let key = target.key.clone();
            assert!(target.journal.exists());
            assert_eq!(target.journal_temp.exists(), mode == "active-journal-temp");
            let record = if mode == "active-journal-temp" {
                target.read_journal_at(&target.journal_temp)?
            } else {
                target.read_journal()?
            }
            .ok_or_else(|| io::Error::other("crash worker left no recovery record"))?
            .record;
            let workspace = target.workspace_path(&record.workspace);
            let owner = target.workspace_path(&record.owner);
            assert!(workspace.exists());
            assert!(owner.exists());

            debt_result(target.recover_pending())?;

            assert!(!target.journal.exists());
            assert!(!target.journal_temp.exists());
            assert!(!workspace.exists());
            assert!(!owner.exists());
            assert_eq!(fs::read(&unrelated)?, b"do not touch");
            drop(target);
            fs::remove_dir_all(&root)?;
            let _ = fs::remove_file(lock_path(&key));
        }
        Ok(())
    }

    #[test]
    fn next_exact_target_replays_the_bound_active_workspace(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("replay")?;
        let output = root.join("archive.repaired.zip");
        let unrelated = root.join(".squallz-par2-repair-unrelated.work");
        create_private_directory(&unrelated)?;
        fs::write(unrelated.join("keep"), b"keep")?;

        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let journal = target.journal_path().to_path_buf();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let owner_path = workspace.owner_path.clone();
        assert_eq!(owner_path.parent(), workspace_path.parent());
        assert!(!owner_path.starts_with(&workspace_path));
        fs::write(workspace_path.join("data.bin"), b"private repair data")?;
        drop(workspace);

        assert!(journal.exists());
        assert!(workspace_path.exists());
        assert!(owner_path.exists());
        let different_output = root.join("other.repaired.zip");
        let different = RepairWorkspaceTarget::lock(&different_output, &ControlToken::default())?;
        let different_key = different.key.clone();
        debt_result(different.recover_pending())?;
        drop(different);
        assert!(journal.exists());
        assert!(workspace_path.exists());

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        debt_result(replay.recover_pending())?;
        drop(replay);

        assert!(!journal.exists());
        assert!(!workspace_path.exists());
        assert!(!owner_path.exists());
        assert_eq!(fs::read(unrelated.join("keep"))?, b"keep");
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        let _ = fs::remove_file(lock_path(&different_key));
        Ok(())
    }

    #[test]
    fn missing_bound_workspace_only_clears_its_record() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("missing")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let journal = target.journal_path().to_path_buf();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let owner_path = workspace.owner_path.clone();
        drop(workspace);
        fs::remove_dir_all(&workspace_path)?;

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        debt_result(replay.recover_pending())?;
        drop(replay);

        assert!(!journal.exists());
        assert!(!workspace_path.exists());
        assert!(!owner_path.exists());
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn reserved_journal_temp_is_promoted_and_cleared_after_interrupted_publish(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("reserved-journal-temp")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace_name = target.unique_workspace_name();
        let record = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            owner: format!("{workspace_name}{OWNER_SUFFIX}"),
            workspace: workspace_name,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&target.key),
            },
        };
        write_test_journal(&target.journal_temp, &record)?;

        debt_result(target.recover_pending())?;

        assert!(!target.journal_temp.exists());
        assert!(!target.journal.exists());
        drop(target);
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn active_journal_temp_advances_reserved_record_and_replays_exact_workspace(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let fixture = prepared_active_transition("active-journal-temp")?;
        write_test_journal(&fixture.target.journal_temp, &fixture.active)?;
        drop(fixture.reserved);
        drop(fixture.owner);
        drop(fixture.directory);

        debt_result(fixture.target.recover_pending())?;

        assert!(!fixture.workspace.exists());
        assert!(!fixture.owner_path.exists());
        assert!(!fixture.target.journal_temp.exists());
        assert!(!fixture.target.journal.exists());
        drop(fixture.target);
        fs::remove_dir_all(&fixture.root)?;
        let _ = fs::remove_file(lock_path(&fixture.key));
        Ok(())
    }

    #[test]
    fn active_record_replace_failure_keeps_a_replayable_temporary_record(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let fixture = prepared_active_transition("active-replace-failure")?;
        let error = fixture
            .target
            .replace_journal_with(
                fixture.reserved,
                &fixture.active,
                &mut |_from, _to| Err(io::Error::other("injected active record replace failure")),
                &mut || panic!("parent sync must not run after a replace failure"),
            )
            .expect_err("the injected journal replacement must fail");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        let primary = fixture
            .target
            .read_journal()?
            .ok_or_else(|| io::Error::other("reserved record disappeared"))?
            .record;
        let temporary = fixture
            .target
            .read_journal_at(&fixture.target.journal_temp)?
            .ok_or_else(|| io::Error::other("active temporary record disappeared"))?
            .record;
        assert!(matches!(
            primary.workspace_state,
            WorkspaceState::Reserved { .. }
        ));
        assert_eq!(temporary, fixture.active);
        drop(fixture.owner);
        drop(fixture.directory);
        drop(fixture.target);

        let replay = RepairWorkspaceTarget::lock(&fixture.output, &ControlToken::default())?;
        debt_result(replay.recover_pending())?;
        drop(replay);

        assert!(!fixture.workspace.exists());
        assert!(!fixture.owner_path.exists());
        fs::remove_dir_all(&fixture.root)?;
        let _ = fs::remove_file(lock_path(&fixture.key));
        Ok(())
    }

    #[test]
    fn active_record_parent_sync_failure_replays_the_committed_record(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let fixture = prepared_active_transition("active-parent-sync-failure")?;
        let error = fixture
            .target
            .replace_journal_with(
                fixture.reserved,
                &fixture.active,
                &mut |from, to| squallz_core::replace_file_atomically(from, to),
                &mut || {
                    Err(io::Error::other(
                        "injected active record parent sync failure",
                    ))
                },
            )
            .expect_err("the injected parent sync must fail");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(!fixture.target.journal_temp.exists());
        let primary = fixture
            .target
            .read_journal()?
            .ok_or_else(|| io::Error::other("active record disappeared"))?
            .record;
        assert_eq!(primary, fixture.active);
        drop(fixture.owner);
        drop(fixture.directory);
        drop(fixture.target);

        let replay = RepairWorkspaceTarget::lock(&fixture.output, &ControlToken::default())?;
        debt_result(replay.recover_pending())?;
        drop(replay);

        assert!(!fixture.workspace.exists());
        assert!(!fixture.owner_path.exists());
        fs::remove_dir_all(&fixture.root)?;
        let _ = fs::remove_file(lock_path(&fixture.key));
        Ok(())
    }

    #[test]
    fn active_cleanup_failures_leave_a_replayable_record() -> Result<(), Box<dyn std::error::Error>>
    {
        for fail_remove in [true, false] {
            let root = test_root(if fail_remove {
                "active-remove-failure"
            } else {
                "active-remove-sync-failure"
            })?;
            let output = root.join("archive.repaired.zip");
            let workspace = debt_result(
                RepairWorkspaceTarget::lock(&output, &ControlToken::default())?.begin(),
            )?;
            let workspace_path = workspace.path().to_path_buf();
            let owner_path = workspace.owner_path.clone();
            let key = workspace.target.key.clone();
            drop(workspace);

            let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
            let journal = target
                .read_journal()?
                .ok_or_else(|| io::Error::other("active record disappeared"))?;
            let (
                WorkspaceState::Active {
                    workspace_identity,
                    owner_identity,
                    owner_token,
                },
                workspace_name,
                owner_name,
            ) = (
                journal.record.workspace_state.clone(),
                journal.record.workspace.clone(),
                journal.record.owner.clone(),
            )
            else {
                return Err(io::Error::other("expected an active workspace record").into());
            };
            let mut sync_calls = 0usize;
            let debt = target
                .cleanup_active_workspace_with(
                    journal,
                    target.workspace_path(&workspace_name),
                    target.workspace_path(&owner_name),
                    ActiveWorkspaceBinding {
                        workspace_identity,
                        owner_identity,
                        owner_token: &owner_token,
                    },
                    ActiveCleanupOperations {
                        remove_workspace: |path: &Path| {
                            if fail_remove {
                                Err(io::Error::other("injected workspace removal failure"))
                            } else {
                                fs::remove_dir_all(path)
                            }
                        },
                        sync_parent: || {
                            sync_calls += 1;
                            if !fail_remove && sync_calls == 1 {
                                Err(io::Error::other(
                                    "injected workspace removal parent sync failure",
                                ))
                            } else {
                                target.parent_file.sync_all()
                            }
                        },
                        clear_journal: |journal| target.clear_journal(journal),
                    },
                )
                .expect_err("the injected cleanup operation must fail");

            assert_eq!(debt.workspace.as_deref(), Some(workspace_path.as_path()));
            assert!(target.journal.exists());
            assert_eq!(workspace_path.exists(), fail_remove);
            assert!(owner_path.exists());
            debt_result(target.recover_pending())?;
            assert!(!target.journal.exists());
            assert!(!workspace_path.exists());
            assert!(!owner_path.exists());
            drop(target);
            fs::remove_dir_all(&root)?;
            let _ = fs::remove_file(lock_path(&key));
        }
        Ok(())
    }

    #[test]
    fn journal_removal_failure_keeps_the_record_for_retry() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = test_root("journal-remove-failure")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace_name = target.unique_workspace_name();
        let record = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            owner: format!("{workspace_name}{OWNER_SUFFIX}"),
            workspace: workspace_name,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&target.key),
            },
        };
        let journal = target.write_new_journal(&record)?;
        let error = target
            .clear_journal_with(
                journal,
                &mut |_path| Err(io::Error::other("injected journal removal failure")),
                &mut || panic!("parent sync must not run after a removal failure"),
            )
            .expect_err("the injected journal removal must fail");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(target.journal.exists());
        debt_result(target.recover_pending())?;
        assert!(!target.journal.exists());
        drop(target);
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn damaged_journal_temp_is_reported_without_deleting_adjacent_paths(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("damaged-journal-temp")?;
        let output = root.join("archive.repaired.zip");
        let victim = root.join("victim");
        create_private_directory(&victim)?;
        fs::write(victim.join("keep"), b"keep")?;
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let mut journal_temp = create_private_file(&target.journal_temp)?;
        journal_temp.write_all(b"{\"record\":\"incomplete\"}")?;
        journal_temp.sync_all()?;
        drop(journal_temp);

        let debt = target
            .recover_pending()
            .expect_err("a damaged temporary record must fail closed");

        assert_eq!(debt.journal, target.journal_temp);
        assert_eq!(debt.workspace, None);
        assert!(debt.journal.exists());
        assert!(!target.journal.exists());
        assert_eq!(fs::read(victim.join("keep"))?, b"keep");
        fs::remove_file(&debt.journal)?;
        debt_result(target.recover_pending())?;
        drop(target);
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn conflicting_journal_temp_preserves_both_records_and_bound_workspace(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("conflicting-journal-temp")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let owner_path = workspace.owner_path.clone();
        let journal = workspace.target.journal.clone();
        let journal_temp = workspace.target.journal_temp.clone();
        let conflicting_workspace = workspace.target.unique_workspace_name();
        let conflicting = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: workspace.target.key.clone(),
            parent_identity: workspace.target.parent_identity,
            owner: format!("{conflicting_workspace}{OWNER_SUFFIX}"),
            workspace: conflicting_workspace,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&workspace.target.key),
            },
        };
        write_test_journal(&journal_temp, &conflicting)?;
        drop(workspace);

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let debt = replay
            .recover_pending()
            .expect_err("conflicting durable records must fail closed");

        assert_eq!(debt.journal, journal_temp);
        assert_eq!(debt.workspace, None);
        assert!(journal.exists());
        assert!(debt.journal.exists());
        assert!(workspace_path.exists());
        assert!(owner_path.exists());
        fs::remove_file(&debt.journal)?;
        debt_result(replay.recover_pending())?;
        drop(replay);
        assert!(!journal.exists());
        assert!(!workspace_path.exists());
        assert!(!owner_path.exists());

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn rebound_workspace_is_never_deleted() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("rebound")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let journal = workspace.target.journal.clone();
        drop(workspace);

        let original = root.join("original-workspace");
        fs::rename(&workspace_path, &original)?;
        create_private_directory(&workspace_path)?;
        fs::write(workspace_path.join("competitor"), b"do not delete")?;

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let debt = replay
            .recover_pending()
            .expect_err("a rebound workspace must fail closed");
        assert_eq!(debt.workspace.as_deref(), Some(workspace_path.as_path()));
        assert_eq!(
            fs::read(workspace_path.join("competitor"))?,
            b"do not delete"
        );
        assert!(original.exists());
        assert!(journal.exists());

        fs::remove_dir_all(&workspace_path)?;
        fs::rename(&original, &workspace_path)?;
        debt_result(replay.recover_pending())?;
        drop(replay);
        assert!(!workspace_path.exists());
        assert!(!journal.exists());

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn damaged_record_does_not_authorize_any_workspace_deletion(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("damaged")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let journal = workspace.target.journal.clone();
        drop(workspace);
        let valid_record = fs::read(&journal)?;
        fs::write(&journal, b"{\"record\":\"damaged\"}")?;

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let debt = replay
            .recover_pending()
            .expect_err("a damaged record must fail closed");
        assert_eq!(debt.workspace, None);
        assert_eq!(debt.journal, journal);
        assert!(workspace_path.exists());

        fs::write(&debt.journal, valid_record)?;
        debt_result(replay.recover_pending())?;
        drop(replay);
        assert!(!workspace_path.exists());

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn same_inode_journal_rewrite_invalidates_the_open_cleanup_authority(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("same-inode-journal-rewrite")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace = debt_result(target.begin())?;
        let workspace_path = workspace.path().to_path_buf();
        let owner_path = workspace.owner_path.clone();
        let journal_path = workspace.journal.path.clone();
        let original = workspace.journal.record.clone();
        let conflicting_workspace = workspace.target.unique_workspace_name();
        let conflicting = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: workspace.target.key.clone(),
            parent_identity: workspace.target.parent_identity,
            owner: format!("{conflicting_workspace}{OWNER_SUFFIX}"),
            workspace: conflicting_workspace,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&workspace.target.key),
            },
        };
        rewrite_test_journal(&journal_path, &conflicting)?;

        let error = workspace
            .target
            .ensure_open_journal_binding(&workspace.journal)
            .expect_err("an in-place record rewrite must invalidate the open authority");

        assert!(error.to_string().contains("content changed"));
        assert!(workspace_path.exists());
        assert!(owner_path.exists());
        rewrite_test_journal(&journal_path, &original)?;
        workspace
            .target
            .ensure_open_journal_binding(&workspace.journal)?;
        drop(workspace);

        let replay = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        debt_result(replay.recover_pending())?;
        drop(replay);
        assert!(!journal_path.exists());
        assert!(!workspace_path.exists());
        assert!(!owner_path.exists());

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn reserved_workspace_requires_exact_manual_cleanup_before_retry(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("reserved")?;
        let output = root.join("archive.repaired.zip");
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let workspace_name = target.unique_workspace_name();
        let owner_name = format!("{workspace_name}{OWNER_SUFFIX}");
        let workspace = target.workspace_path(&workspace_name);
        let record = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: workspace_name,
            owner: owner_name,
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&target.key),
            },
        };
        drop(target.write_new_journal(&record)?);
        create_private_directory(&workspace)?;

        let debt = target
            .recover_pending()
            .expect_err("an unbound reserved directory must not be deleted");
        assert_eq!(debt.workspace.as_deref(), Some(workspace.as_path()));
        assert!(workspace.exists());
        fs::remove_dir_all(&workspace)?;
        debt_result(target.recover_pending())?;
        assert!(!target.journal.exists());
        drop(target);

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn traversal_name_with_a_valid_digest_is_still_rejected(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = test_root("traversal")?;
        let output = root.join("archive.repaired.zip");
        let victim = root.join("victim");
        create_private_directory(&victim)?;
        fs::write(victim.join("keep"), b"keep")?;
        let target = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = target.key.clone();
        let record = JournalRecord {
            version: JOURNAL_VERSION,
            target_key: target.key.clone(),
            parent_identity: target.parent_identity,
            workspace: "../victim".to_owned(),
            owner: "../victim.owner".to_owned(),
            workspace_state: WorkspaceState::Reserved {
                owner_token: owner_token(&target.key),
            },
        };
        let bytes = journal_bytes(&record)?;
        let mut journal = create_private_file(&target.journal)?;
        journal.write_all(&bytes)?;
        journal.sync_all()?;
        drop(journal);

        let debt = target
            .recover_pending()
            .expect_err("a traversal workspace name must be rejected");
        assert_eq!(debt.workspace, None);
        assert_eq!(fs::read(victim.join("keep"))?, b"keep");
        fs::remove_file(&target.journal)?;
        drop(target);

        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }

    #[test]
    fn waiting_for_the_same_target_lock_can_be_cancelled() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = test_root("lock-cancel")?;
        let output = root.join("archive.repaired.zip");
        let held = RepairWorkspaceTarget::lock(&output, &ControlToken::default())?;
        let key = held.key.clone();
        let control = ControlToken::new();
        let cancel = control.clone();
        let cancel_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(75));
            cancel.cancel();
        });

        let error = RepairWorkspaceTarget::lock(&output, &control)
            .expect_err("waiting for the same repair target must observe cancellation");
        cancel_thread
            .join()
            .map_err(|_| io::Error::other("cancel thread panicked"))?;
        assert!(matches!(error, FormatError::Cancelled));

        drop(held);
        fs::remove_dir_all(&root)?;
        let _ = fs::remove_file(lock_path(&key));
        Ok(())
    }
}
