use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

const JOURNAL_VERSION: u32 = 1;
// A Windows extended-length path may contain almost 32K UTF-16 units. The
// journal stores four paths losslessly, so keep a bounded but platform-sized
// envelope rather than accepting a record that a later startup cannot read.
const JOURNAL_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const HOLDER_PREFIX: &str = ".squallz-trash-hold-";
static JOURNAL_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceCleanupRecord {
    pub(crate) original: PathBuf,
    pub(crate) staged: PathBuf,
    pub(crate) holder: PathBuf,
    preserved: PathBuf,
    identity: SourcePathIdentity,
}

impl SourceCleanupRecord {
    pub(crate) fn new(original: PathBuf, staged: PathBuf, holder: PathBuf) -> io::Result<Self> {
        let preserved = available_preserved_path(&original)?;
        let identity = source_path_identity(&original)?;
        Ok(Self {
            original,
            staged,
            holder,
            preserved,
            identity,
        })
    }

    #[cfg(test)]
    pub(crate) fn preserved_path(&self) -> &Path {
        &self.preserved
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceCleanupRecovery {
    None,
    Restored { path: PathBuf },
    Preserved { path: PathBuf },
    Changed { path: PathBuf },
    Cleared,
    CompletedUnknown { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourcePathIdentity {
    filesystem: u64,
    entry: u64,
}

pub(crate) struct SourceCleanupJournal {
    path: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct PendingSourceCleanup {
    journal_path: PathBuf,
    record: SourceCleanupRecord,
    _lock: File,
}

impl SourceCleanupJournal {
    pub(crate) fn load() -> Self {
        #[cfg(not(test))]
        let path = dirs::config_dir()
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
            .map(|base| base.join("Squallz").join("source-cleanup.json"));
        #[cfg(test)]
        let path = Some(std::env::temp_dir().join(format!(
            "squallz-source-journal-manager-{}-{}/source-cleanup.json",
            std::process::id(),
            JOURNAL_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )));
        Self { path }
    }

    #[cfg(test)]
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub(crate) fn begin(&self, record: &SourceCleanupRecord) -> io::Result<PendingSourceCleanup> {
        let path = self.path()?.to_path_buf();
        let lock = acquire_lock(&path)?;
        if journal_path_exists(&path)? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "an interrupted source cleanup still needs recovery",
            ));
        }
        validate_record_layout(record, true)?;
        validate_before_move(record)?;
        let document = JournalDocument::from_record(record)?;
        let bytes = serialize_journal(&document)?;
        write_new_journal(&path, &bytes)?;
        Ok(PendingSourceCleanup {
            journal_path: path,
            record: record.clone(),
            _lock: lock,
        })
    }

    pub(crate) fn recover_pending(&self) -> io::Result<SourceCleanupRecovery> {
        let Some(path) = self.path.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no private configuration directory is available",
            ));
        };
        let _lock = acquire_lock(path)?;
        let Some(document) = read_journal(path)? else {
            return Ok(SourceCleanupRecovery::None);
        };
        let record = document.into_record()?;
        recover_record(path, &record)
    }

    pub(crate) fn recovery_record_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn path(&self) -> io::Result<&Path> {
        self.path.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no private configuration directory is available",
            )
        })
    }
}

impl PendingSourceCleanup {
    pub(crate) fn record(&self) -> &SourceCleanupRecord {
        &self.record
    }

    pub(crate) fn sync_after_stage(&self) -> io::Result<()> {
        sync_staging_directories(&self.record)
    }

    pub(crate) fn confirm_staged_source(&self) -> io::Result<()> {
        ensure_source_identity(&self.record.staged, self.record.identity)
    }

    pub(crate) fn recover(self) -> io::Result<SourceCleanupRecovery> {
        recover_record(&self.journal_path, &self.record)
    }

    pub(crate) fn complete_trash(&self) -> io::Result<()> {
        validate_record_layout(&self.record, true)?;
        validate_holder_inventory(&self.record)?;
        if path_exists(&self.record.staged)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the source cleanup adapter reported success but the staged source remains",
            ));
        }
        if path_exists(&self.record.preserved)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the source cleanup preservation path is unexpectedly occupied",
            ));
        }
        clear_then_remove_holder(&self.journal_path, &self.record)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    version: u32,
    original: StoredPath,
    staged: StoredPath,
    holder: StoredPath,
    preserved: StoredPath,
    identity: SourcePathIdentity,
}

impl JournalDocument {
    fn from_record(record: &SourceCleanupRecord) -> io::Result<Self> {
        Ok(Self {
            version: JOURNAL_VERSION,
            original: StoredPath::from_path(&record.original)?,
            staged: StoredPath::from_path(&record.staged)?,
            holder: StoredPath::from_path(&record.holder)?,
            preserved: StoredPath::from_path(&record.preserved)?,
            identity: record.identity,
        })
    }

    fn into_record(self) -> io::Result<SourceCleanupRecord> {
        if self.version != JOURNAL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported source cleanup journal version",
            ));
        }
        Ok(SourceCleanupRecord {
            original: self.original.into_path()?,
            staged: self.staged.into_path()?,
            holder: self.holder.into_path()?,
            preserved: self.preserved.into_path()?,
            identity: self.identity,
        })
    }
}

fn serialize_journal(document: &JournalDocument) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(document)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() as u64 > JOURNAL_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup journal exceeds the recovery size limit",
        ));
    }
    Ok(bytes)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "units", rename_all = "snake_case")]
enum StoredPath {
    UnixBytes(Vec<u8>),
    WindowsWide(Vec<u16>),
    Utf8(String),
}

impl StoredPath {
    fn from_path(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            let bytes = path.as_os_str().as_bytes();
            if bytes.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source cleanup path contains a null byte",
                ));
            }
            Ok(Self::UnixBytes(bytes.to_vec()))
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            let units: Vec<u16> = path.as_os_str().encode_wide().collect();
            if units.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source cleanup path contains a null unit",
                ));
            }
            Ok(Self::WindowsWide(units))
        }
        #[cfg(not(any(unix, windows)))]
        {
            path.to_str()
                .map(|value| Self::Utf8(value.to_owned()))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source cleanup path is not valid UTF-8",
                    )
                })
        }
    }

    fn into_path(self) -> io::Result<PathBuf> {
        match self {
            Self::UnixBytes(bytes) => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;

                    if bytes.contains(&0) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "source cleanup path contains a null byte",
                        ));
                    }
                    Ok(PathBuf::from(OsString::from_vec(bytes)))
                }
                #[cfg(not(unix))]
                {
                    let _ = bytes;
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "source cleanup journal belongs to another platform",
                    ))
                }
            }
            Self::WindowsWide(units) => {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt;

                    if units.contains(&0) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "source cleanup path contains a null unit",
                        ));
                    }
                    Ok(PathBuf::from(OsString::from_wide(&units)))
                }
                #[cfg(not(windows))]
                {
                    let _ = units;
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "source cleanup journal belongs to another platform",
                    ))
                }
            }
            Self::Utf8(value) => {
                if value.as_bytes().contains(&0) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "source cleanup path contains a null byte",
                    ));
                }
                Ok(PathBuf::from(value))
            }
        }
    }
}

fn validate_record_layout(record: &SourceCleanupRecord, require_holder: bool) -> io::Result<()> {
    if !record.original.is_absolute()
        || !record.staged.is_absolute()
        || !record.holder.is_absolute()
        || !record.preserved.is_absolute()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal paths must be absolute",
        ));
    }
    let original_parent = record.original.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup original has no parent directory",
        )
    })?;
    let original_name = record.original.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup original has no file name",
        )
    })?;
    let valid_preserved = (1..=1000u32).any(|attempt| {
        preserved_path(&record.original, attempt).as_deref() == Some(record.preserved.as_path())
    });
    if record.holder.parent() != Some(original_parent)
        || record.staged.parent() != Some(record.holder.as_path())
        || record.staged.file_name() != Some(original_name)
        || record.preserved.parent() != Some(original_parent)
        || !valid_preserved
        || !record
            .holder
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(HOLDER_PREFIX))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal layout is invalid",
        ));
    }
    let canonical_parent = fs::canonicalize(original_parent)?;
    if canonical_parent != original_parent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup parent must not traverse symbolic links",
        ));
    }
    match fs::symlink_metadata(&record.holder) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder is not a real directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !require_holder => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_before_move(record: &SourceCleanupRecord) -> io::Result<()> {
    validate_holder_inventory(record)?;
    if !path_exists(&record.original)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "source cleanup original no longer exists",
        ));
    }
    ensure_source_identity(&record.original, record.identity)?;
    if path_exists(&record.staged)? || path_exists(&record.preserved)? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "source cleanup staging or preservation path is already occupied",
        ));
    }
    Ok(())
}

fn validate_holder_inventory(record: &SourceCleanupRecord) -> io::Result<()> {
    let holder_metadata = match fs::symlink_metadata(&record.holder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if path_exists(&record.staged)? {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source cleanup staged path exists without its holder",
                ));
            }
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !holder_metadata.is_dir() || holder_metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder is not a real directory",
        ));
    }
    let mut entries = fs::read_dir(&record.holder)?;
    let first = entries.next().transpose()?.map(|entry| entry.path());
    if entries.next().transpose()?.is_some()
        || first
            .as_deref()
            .is_some_and(|path| path != record.staged.as_path())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder contains unexpected entries",
        ));
    }
    let staged_exists = path_exists(&record.staged)?;
    if staged_exists != first.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder inventory changed during recovery",
        ));
    }
    Ok(())
}

fn recover_record(path: &Path, record: &SourceCleanupRecord) -> io::Result<SourceCleanupRecovery> {
    recover_record_with_sync(path, record, sync_recovery_directories)
}

fn recover_record_with_sync(
    path: &Path,
    record: &SourceCleanupRecord,
    sync: impl FnOnce(&SourceCleanupRecord) -> io::Result<()>,
) -> io::Result<SourceCleanupRecovery> {
    validate_record_layout(record, false)?;
    validate_holder_inventory(record)?;
    let original_identity = source_path_identity_if_exists(&record.original)?;
    let staged_identity = source_path_identity_if_exists(&record.staged)?;
    let preserved_identity = source_path_identity_if_exists(&record.preserved)?;
    if staged_identity.is_some() && preserved_identity.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup staged and preservation paths are both occupied",
        ));
    }

    let outcome = if let Some(preserved_identity) = preserved_identity {
        if preserved_identity == record.identity {
            SourceCleanupRecovery::Preserved {
                path: record.preserved.clone(),
            }
        } else {
            SourceCleanupRecovery::Changed {
                path: record.preserved.clone(),
            }
        }
    } else if let Some(staged_identity) = staged_identity {
        let destination = if original_identity.is_some() {
            &record.preserved
        } else {
            &record.original
        };
        move_staged_source(record, destination, staged_identity)?;
        if staged_identity != record.identity {
            SourceCleanupRecovery::Changed {
                path: destination.clone(),
            }
        } else if destination == &record.original {
            SourceCleanupRecovery::Restored {
                path: record.original.clone(),
            }
        } else {
            SourceCleanupRecovery::Preserved {
                path: record.preserved.clone(),
            }
        }
    } else if original_identity == Some(record.identity) {
        SourceCleanupRecovery::Cleared
    } else if original_identity.is_some() {
        SourceCleanupRecovery::Changed {
            path: record.original.clone(),
        }
    } else {
        SourceCleanupRecovery::CompletedUnknown {
            path: record.original.clone(),
        }
    };

    validate_holder_inventory(record)?;
    clear_then_remove_holder_with_sync(path, record, sync)?;
    Ok(outcome)
}

fn move_staged_source(
    record: &SourceCleanupRecord,
    destination: &Path,
    staged_identity: SourcePathIdentity,
) -> io::Result<()> {
    squallz_core::move_path_no_replace(&record.staged, destination)?;
    ensure_source_identity(destination, staged_identity)
}

fn source_path_identity_if_exists(path: &Path) -> io::Result<Option<SourcePathIdentity>> {
    match source_path_identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn ensure_source_identity(path: &Path, expected: SourcePathIdentity) -> io::Result<()> {
    if source_path_identity(path)? == expected {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "source cleanup path identity changed",
    ))
}

#[cfg(unix)]
pub(crate) fn source_path_identity(path: &Path) -> io::Result<SourcePathIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    Ok(SourcePathIdentity {
        filesystem: metadata.dev(),
        entry: metadata.ino(),
    })
}

#[cfg(windows)]
pub(crate) fn source_path_identity(path: &Path) -> io::Result<SourcePathIdentity> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let information = winapi_util::file::information(&file)?;
    Ok(SourcePathIdentity {
        filesystem: information.volume_serial_number(),
        entry: information.file_index(),
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn source_path_identity(_path: &Path) -> io::Result<SourcePathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "source cleanup path identity is unavailable on this platform",
    ))
}

fn available_preserved_path(original: &Path) -> io::Result<PathBuf> {
    for attempt in 1..=1000u32 {
        let path = preserved_path(original, attempt).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "source cleanup path cannot be preserved beside its original location",
            )
        })?;
        if !path_exists(&path)? {
            return Ok(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free preservation path is available beside the original source",
    ))
}

fn preserved_path(original: &Path, attempt: u32) -> Option<PathBuf> {
    let parent = original.parent()?;
    let mut name = OsString::from(original.file_name()?);
    name.push(".squallz-preserved");
    if attempt > 1 {
        name.push(format!("-{attempt}"));
    }
    Some(parent.join(name))
}

fn read_journal(path: &Path) -> io::Result<Option<JournalDocument>> {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal is not a regular file",
        ));
    }
    let file = OpenOptions::new().read(true).open(path)?;
    let file_metadata = file.metadata()?;
    if !file_metadata.is_file() || !same_file_identity(path, &file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal identity changed while opening it",
        ));
    }
    if file_metadata.len() > JOURNAL_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal is too large",
        ));
    }
    let mut bytes = Vec::new();
    file.take(JOURNAL_MAX_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > JOURNAL_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal is too large",
        ));
    }
    let document = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Some(document))
}

fn write_new_journal(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup journal has no parent directory",
        )
    })?;
    create_dir_all_durable(parent)?;
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup journal has no file name",
        )
    })?;
    let sequence = JOURNAL_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let temp_path = path.with_file_name(temp_name);

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let mut file = options.open(&temp_path)?;
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = squallz_core::move_path_no_replace(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    sync_directory(parent)
}

fn acquire_lock(journal_path: &Path) -> io::Result<File> {
    let parent = journal_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup journal has no parent directory",
        )
    })?;
    create_dir_all_durable(parent)?;
    let lock_path = journal_path.with_file_name("source-cleanup.lock");
    let mut created = false;
    let file = {
        let mut create = OpenOptions::new();
        create.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            create.mode(0o600);
        }
        match create.open(&lock_path) {
            Ok(file) => {
                created = true;
                file
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => OpenOptions::new()
                .read(true)
                .write(true)
                .truncate(false)
                .open(&lock_path)?,
            Err(error) => return Err(error),
        }
    };
    fs4::FileExt::try_lock(&file).map_err(io::Error::from)?;
    let path_metadata = fs::symlink_metadata(&lock_path)?;
    let file_metadata = file.metadata()?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || !file_metadata.is_file()
        || !same_file_identity(&lock_path, &file)?
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup lock is not a stable regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if file_metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "source cleanup lock permissions are too broad",
            ));
        }
    }
    if created {
        file.sync_all()?;
        sync_directory(parent)?;
    }
    Ok(file)
}

fn create_dir_all_durable(path: &Path) -> io::Result<()> {
    create_dir_all_durable_with(path, sync_directory)
}

fn create_dir_all_durable_with(
    path: &Path,
    mut sync: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut existing = path.to_path_buf();
    loop {
        match fs::metadata(&existing) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "source cleanup journal parent is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(existing.clone());
                let parent = directory_parent(&existing).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "source cleanup journal has no existing parent directory",
                    )
                })?;
                if parent == existing {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "source cleanup journal has no existing parent directory",
                    ));
                }
                existing = parent.to_path_buf();
            }
            Err(error) => return Err(error),
        }
    }

    sync_directory_entry_with(&existing, &mut sync)?;
    for directory in missing.iter().rev() {
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source cleanup journal parent is not a real directory",
            ));
        }
        sync_directory_entry_with(directory, &mut sync)?;
    }
    Ok(())
}

fn sync_directory_entry_with(
    path: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    sync(path)?;
    if let Some(parent) = directory_parent(path) {
        sync(parent)?;
    }
    Ok(())
}

fn directory_parent(path: &Path) -> Option<&Path> {
    path.parent().map(|parent| {
        if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        }
    })
}

#[cfg(unix)]
fn same_file_identity(path: &Path, file: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let path_metadata = fs::symlink_metadata(path)?;
    let file_metadata = file.metadata()?;
    Ok(path_metadata.dev() == file_metadata.dev() && path_metadata.ino() == file_metadata.ino())
}

#[cfg(windows)]
fn same_file_identity(path: &Path, file: &File) -> io::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let path_file = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let path_information = winapi_util::file::information(&path_file)?;
    let file_information = winapi_util::file::information(file)?;
    Ok(
        path_information.volume_serial_number() == file_information.volume_serial_number()
            && path_information.file_index() == file_information.file_index(),
    )
}

#[cfg(not(any(unix, windows)))]
fn same_file_identity(_path: &Path, _file: &File) -> io::Result<bool> {
    Ok(true)
}

fn clear_then_remove_holder(path: &Path, record: &SourceCleanupRecord) -> io::Result<()> {
    clear_then_remove_holder_with_sync(path, record, sync_recovery_directories)
}

fn clear_then_remove_holder_with_sync(
    path: &Path,
    record: &SourceCleanupRecord,
    sync: impl FnOnce(&SourceCleanupRecord) -> io::Result<()>,
) -> io::Result<()> {
    validate_holder_inventory(record)?;
    sync(record)?;
    clear_journal(path)?;
    if remove_empty_holder(&record.holder).is_err() {
        log::warn!("source cleanup: an empty staging directory could not be removed");
    }
    Ok(())
}

fn clear_journal(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path)?;
            sync_parent(path)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup journal is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn journal_path_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn path_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn remove_empty_holder(holder: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(holder) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder is not a real directory",
        ));
    }
    if fs::read_dir(holder)?.next().transpose()?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::DirectoryNotEmpty,
            "source cleanup holder contains unexpected entries",
        ));
    }
    fs::remove_dir(holder)?;
    sync_parent(holder)
}

pub(crate) fn remove_empty_holder_if_identity(
    holder: &Path,
    expected: SourcePathIdentity,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(holder)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || source_path_identity(holder)? != expected
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder identity or type changed",
        ));
    }
    if fs::read_dir(holder)?.next().transpose()?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::DirectoryNotEmpty,
            "source cleanup holder contains unexpected entries",
        ));
    }
    if source_path_identity(holder)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder identity changed before removal",
        ));
    }
    fs::remove_dir(holder)
}

fn sync_staging_directories(record: &SourceCleanupRecord) -> io::Result<()> {
    sync_staging_directories_with(record, sync_directory)
}

fn sync_staging_directories_with(
    record: &SourceCleanupRecord,
    mut sync: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = record.original.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup path has no parent directory",
        )
    })?;
    match fs::symlink_metadata(&record.holder) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            sync_move_directories_with(parent, &record.holder, &mut sync)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder is not a real directory",
        )),
        Err(error) => Err(error),
    }
}

fn sync_recovery_directories(record: &SourceCleanupRecord) -> io::Result<()> {
    sync_recovery_directories_with(record, sync_directory)
}

fn sync_recovery_directories_with(
    record: &SourceCleanupRecord,
    mut sync: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = record.original.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup path has no parent directory",
        )
    })?;
    match fs::symlink_metadata(&record.holder) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            sync_move_directories_with(&record.holder, parent, &mut sync)
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source cleanup holder is not a real directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => sync(parent),
        Err(error) => Err(error),
    }
}

fn sync_move_directories_with(
    source: &Path,
    destination: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    sync(destination)?;
    if source != destination {
        sync(source)?;
    }
    Ok(())
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no parent directory to synchronize",
        )
    })?;
    sync_directory(parent)
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "durable source cleanup is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let sequence = JOURNAL_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "squallz-source-journal-{name}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }

    fn fixture(root: &Path, name: &OsString) -> (SourceCleanupJournal, SourceCleanupRecord) {
        let parent = root.join("source-parent");
        fs::create_dir_all(&parent).unwrap();
        let original = parent.join(name);
        fs::write(&original, b"source").unwrap();
        let holder = parent.join(format!("{HOLDER_PREFIX}fixture"));
        fs::create_dir(&holder).unwrap();
        let staged = holder.join(name);
        let record = SourceCleanupRecord::new(original, staged, holder).unwrap();
        let journal = SourceCleanupJournal::at_path(root.join("config/source-cleanup.json"));
        (journal, record)
    }

    #[test]
    fn post_stage_syncs_destination_holder_before_source_parent() {
        let root = temp_dir("post-stage-sync-order");
        let (_, record) = fixture(&root, &OsString::from("source.txt"));
        let parent = record.original.parent().unwrap().to_path_buf();
        let mut synced = Vec::new();

        sync_staging_directories_with(&record, |path| {
            synced.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();

        assert_eq!(synced, vec![record.holder.clone(), parent]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_syncs_destination_parent_before_source_holder() {
        let root = temp_dir("recovery-sync-order");
        let (_, record) = fixture(&root, &OsString::from("source.txt"));
        let parent = record.original.parent().unwrap().to_path_buf();
        let mut synced = Vec::new();

        sync_recovery_directories_with(&record, |path| {
            synced.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();

        assert_eq!(synced, vec![parent, record.holder.clone()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn move_directory_sync_deduplicates_the_same_directory() {
        let root = temp_dir("move-sync-same-directory");
        let mut synced = Vec::new();

        sync_move_directories_with(&root, &root, &mut |path| {
            synced.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();

        assert_eq!(synced, vec![root.clone()]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identity_safe_holder_cleanup_leaves_a_replacement_in_place() {
        let root = temp_dir("holder-replacement");
        let holder = root.join(format!("{HOLDER_PREFIX}replacement"));
        let displaced = root.join("displaced-holder");
        fs::create_dir(&holder).unwrap();
        let original_identity = source_path_identity(&holder).unwrap();
        fs::rename(&holder, &displaced).unwrap();
        fs::create_dir(&holder).unwrap();
        assert_ne!(source_path_identity(&holder).unwrap(), original_identity);

        let error = remove_empty_holder_if_identity(&holder, original_identity).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(holder.is_dir());
        assert!(displaced.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_journal_directory_creation_syncs_each_new_directory_entry() {
        let root = temp_dir("durable-directory-first-create");
        let config = root.join("config");
        let target = config.join("Squallz");
        let mut synced = Vec::new();

        create_dir_all_durable_with(&target, |path| {
            synced.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();

        assert!(target.is_dir());
        assert_eq!(
            synced,
            vec![
                root.clone(),
                root.parent().unwrap().to_path_buf(),
                config.clone(),
                root.clone(),
                target.clone(),
                config,
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_directory_creation_retries_a_failed_parent_sync() {
        let root = temp_dir("durable-directory-retry");
        let config = root.join("config");
        let target = config.join("Squallz");
        let mut sync_calls = 0usize;

        let error = create_dir_all_durable_with(&target, |_| {
            sync_calls += 1;
            if sync_calls == 4 {
                Err(io::Error::other("simulated parent directory sync failure"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(config.is_dir());
        assert!(!target.exists());

        let mut retried = Vec::new();
        create_dir_all_durable_with(&target, |path| {
            retried.push(path.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            retried,
            vec![config.clone(), root.clone(), target.clone(), config]
        );
        assert!(target.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_record_before_move_clears_without_touching_the_source() {
        let root = temp_dir("before-move");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Cleared
        );
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert!(!record.holder.exists());
        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_staged_source_is_restored_idempotently() {
        let root = temp_dir("restore");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Restored {
                path: record.original.clone(),
            }
        );
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert!(!record.holder.exists());
        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_sync_failure_keeps_the_journal_for_the_next_startup() {
        let root = temp_dir("recovery-sync-failure");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        drop(pending);

        let error = recover_record_with_sync(journal.path().unwrap(), &record, |_| {
            Err(io::Error::other("simulated directory sync failure"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(journal.path().unwrap().exists());
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert!(record.holder.exists());
        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Cleared
        );
        assert!(!journal.path().unwrap().exists());
        assert!(!record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_preserves_a_staged_source_beside_a_late_conflict() {
        let root = temp_dir("collision");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        fs::write(&record.original, b"competitor").unwrap();
        drop(pending);

        let preserved = match journal.recover_pending().unwrap() {
            SourceCleanupRecovery::Preserved { path } => path,
            outcome => panic!("unexpected recovery outcome: {outcome:?}"),
        };
        assert_eq!(fs::read(&record.original).unwrap(), b"competitor");
        assert_eq!(fs::read(preserved).unwrap(), b"source");
        assert!(!record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_restores_an_untrusted_staged_replacement_without_deleting_it() {
        let root = temp_dir("staged-identity-swap");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        let safe = record.holder.parent().unwrap().join("source-safe.txt");
        squallz_core::move_path_no_replace(&record.staged, &safe).unwrap();
        fs::write(&record.staged, b"competitor").unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Changed {
                path: record.original.clone(),
            }
        );
        assert_eq!(fs::read(&safe).unwrap(), b"source");
        assert_eq!(fs::read(&record.original).unwrap(), b"competitor");
        assert!(!record.staged.exists());
        assert!(!record.holder.exists());
        assert!(!journal.path().unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_restores_the_entry_moved_after_an_original_path_replacement() {
        let root = temp_dir("original-identity-swap");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        let archived_source = record.holder.parent().unwrap().join("archived-source.txt");

        squallz_core::move_path_no_replace(&record.original, &archived_source).unwrap();
        fs::write(&record.original, b"replacement").unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        assert!(pending.confirm_staged_source().is_err());
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Changed {
                path: record.original.clone(),
            }
        );
        assert_eq!(fs::read(&archived_source).unwrap(), b"source");
        assert_eq!(fs::read(&record.original).unwrap(), b"replacement");
        assert!(!record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_does_not_claim_a_competing_preservation_path_as_the_original() {
        let root = temp_dir("preserved-identity-swap");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        fs::write(&record.preserved, b"competitor").unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Changed {
                path: record.preserved.clone(),
            }
        );
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert_eq!(fs::read(&record.preserved).unwrap(), b"competitor");
        assert!(!record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn crash_after_preservation_still_reports_the_preserved_path() {
        let root = temp_dir("preserved-crash");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        fs::write(&record.original, b"competitor").unwrap();
        squallz_core::move_path_no_replace(&record.staged, &record.preserved).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Preserved {
                path: record.preserved.clone(),
            }
        );
        assert_eq!(fs::read(&record.preserved).unwrap(), b"source");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_staged_source_is_treated_as_an_unknown_completed_move() {
        let root = temp_dir("completed");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        fs::remove_file(&record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::CompletedUnknown {
                path: record.original.clone(),
            }
        );
        assert!(!record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_cleanup_lock_blocks_a_second_instance() {
        let root = temp_dir("lock");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        let second = SourceCleanupJournal::at_path(journal.path().unwrap().to_path_buf());

        assert_eq!(
            second.recover_pending().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            second.begin(&record).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(fs::read(&record.original).unwrap(), b"source");

        drop(pending);
        assert_eq!(
            second.recover_pending().unwrap(),
            SourceCleanupRecovery::Cleared
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_trash_must_remove_the_staged_source_before_clear() {
        let root = temp_dir("trash-confirmation");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();

        assert_eq!(
            pending.complete_trash().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(journal.path().unwrap().exists());
        assert!(record.staged.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_or_traversing_journals_fail_closed() {
        let root = temp_dir("tampered");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let journal_path = journal.path().unwrap().to_path_buf();
        fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        fs::write(&journal_path, b"{\"version\":1").unwrap();
        assert!(journal.recover_pending().is_err());
        assert_eq!(fs::read(&record.original).unwrap(), b"source");

        fs::remove_file(&journal_path).unwrap();
        let mut document = JournalDocument::from_record(&record).unwrap();
        document.staged = StoredPath::from_path(&record.holder.join("../victim")).unwrap();
        fs::write(&journal_path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(journal.recover_pending().is_err());
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert!(record.holder.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unexpected_holder_entries_are_never_deleted() {
        let root = temp_dir("holder-entry");
        let (journal, record) = fixture(&root, &OsString::from("source.txt"));
        let pending = journal.begin(&record).unwrap();
        let unexpected = record.holder.join("unexpected.txt");
        fs::write(&unexpected, b"keep").unwrap();
        drop(pending);

        assert!(journal.recover_pending().is_err());
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        assert_eq!(fs::read(unexpected).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn journal_size_limit_accepts_platform_length_paths_and_rejects_larger_records() {
        fn document_with_units(units: usize) -> JournalDocument {
            let path = || StoredPath::WindowsWide(vec![u16::MAX; units]);
            JournalDocument {
                version: JOURNAL_VERSION,
                original: path(),
                staged: path(),
                holder: path(),
                preserved: path(),
                identity: SourcePathIdentity {
                    filesystem: 1,
                    entry: 2,
                },
            }
        }

        assert!(serialize_journal(&document_with_units(32_767)).is_ok());
        assert_eq!(
            serialize_journal(&document_with_units(100_000))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_sync_uses_a_flushable_handle() {
        let root = temp_dir("windows-directory-sync");
        sync_directory(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_serialize_without_loss() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/source-\xff".to_vec()));
        let restored = StoredPath::from_path(&path).unwrap().into_path().unwrap();
        assert_eq!(restored, path);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn non_utf8_paths_recover_without_loss() {
        use std::os::unix::ffi::OsStringExt;

        let root = temp_dir("non-utf8");
        let name = OsString::from_vec(b"source-\xff".to_vec());
        let (journal, record) = fixture(&root, &name);
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        assert_eq!(
            journal.recover_pending().unwrap(),
            SourceCleanupRecovery::Restored {
                path: record.original.clone(),
            }
        );
        assert_eq!(fs::read(&record.original).unwrap(), b"source");
        fs::remove_dir_all(root).unwrap();
    }
}
