use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::api::{ControlToken, FormatError};
use crate::archive_path::is_canonical_process_sequence;
use crate::filesystem_identity::{file_identity, path_identity, PathIdentity};

use super::evidence::invalid;
use super::model::{Name, PairEvidence, ReplacementRecord};

static SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct HeldDirectory {
    pub path: PathBuf,
    pub file: File,
    pub identity: PathIdentity,
}
impl HeldDirectory {
    pub(crate) fn open(path: PathBuf, expected: PathIdentity) -> Result<Self, FormatError> {
        let file = crate::open_directory(&path)?;
        Self::bind(path, file, expected)
    }
    fn bind(path: PathBuf, file: File, expected: PathIdentity) -> Result<Self, FormatError> {
        let bound = Self {
            path,
            file,
            identity: expected,
        };
        bound.verify()?;
        Ok(bound)
    }
    pub(crate) fn verify(&self) -> Result<(), FormatError> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || file_identity(&self.file)? != self.identity
            || path_identity(&self.path)? != self.identity
        {
            return Err(invalid("held replacement directory changed"));
        }
        Ok(())
    }
    pub(crate) fn sync(&self) -> Result<(), FormatError> {
        self.verify()?;
        #[cfg(test)]
        super::test_hooks::emit(super::test_hooks::Event::BeforeSync)?;
        #[cfg(test)]
        super::test_hooks::sync(&self.path, &self.file)?;
        #[cfg(not(test))]
        self.file.sync_all()?;
        self.verify()
    }
}

pub(crate) enum Domain {
    Archive { key: String },
    Sfx,
}
enum Coordination {
    Inspection,
    Directory { directory: File },
    Target { directory: File, target: File },
}
pub(crate) struct Scope {
    pub parent: HeldDirectory,
    pub domain: Domain,
    pub(super) requested: Name,
    pub(super) target: Name,
    coordination: Coordination,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordPhase {
    Pending,
    Installing,
    Completed,
}

impl Scope {
    pub(crate) fn archive(destination: &Path, control: &ControlToken) -> Result<Self, FormatError> {
        let requested = canonical_requested(destination)?;
        if requested.file_name().and_then(OsStr::to_str).is_none() {
            return Err(FormatError::Unsupported("invalid archive file name".into()));
        }
        let parent_path = crate::parent_or_current(&requested).to_path_buf();
        let directory_key = path_key(b"squallz-update-directory-v1\0", &parent_path)?;
        let directory = acquire_lock(
            &std::env::temp_dir().join(format!("squallz-update-directory-{directory_key}.lock")),
            Some(control),
        )?;
        let target = canonical_target(&requested, true)?;
        let key = path_key(b"squallz-update-target-v1\0", &target)?;
        let target_lock = acquire_lock(
            &std::env::temp_dir().join(format!("squallz-update-target-{key}.lock")),
            Some(control),
        )?;
        Ok(Self {
            parent: HeldDirectory::open(parent_path.clone(), path_identity(&parent_path)?)?,
            requested: Name::from_path(&requested)?,
            target: Name::from_path(&target)?,
            domain: Domain::Archive { key },
            coordination: Coordination::Target {
                directory,
                target: target_lock,
            },
        })
    }
    pub(crate) fn sfx_directory(destination: &Path) -> Result<Self, FormatError> {
        let mut scope = Self::inspect_sfx(destination)?;
        let key = path_key(b"squallz-sfx-directory-v1\0", scope.parent_path())?;
        let directory = acquire_lock(
            &std::env::temp_dir().join(format!("squallz-sfx-directory-{key}.lock")),
            None,
        )?;
        scope.coordination = Coordination::Directory { directory };
        scope.parent = HeldDirectory::open(scope.parent.path.clone(), scope.parent.identity)?;
        scope.verify()?;
        Ok(scope)
    }
    pub(crate) fn inspect_sfx(destination: &Path) -> Result<Self, FormatError> {
        let requested = canonical_requested(destination)?;
        let parent = crate::parent_or_current(&requested).to_path_buf();
        let identity = path_identity(&parent)?;
        Ok(Self {
            parent: HeldDirectory::bind(
                parent.clone(),
                crate::open_directory_no_follow(&parent)?,
                identity,
            )?,
            target: Name::from_path(&canonical_target(&requested, false)?)?,
            requested: Name::from_path(&requested)?,
            domain: Domain::Sfx,
            coordination: Coordination::Inspection,
        })
    }
    pub(crate) fn inspection_target(
        &mut self,
        record: &ReplacementRecord,
    ) -> Result<(), FormatError> {
        if !matches!(self.coordination, Coordination::Inspection) || !self.is_sfx() {
            return Err(invalid(
                "only read-only SFX inspection may select a recorded target",
            ));
        }
        self.target = record.target.clone();
        self.requested = record.requested.clone();
        Ok(())
    }
    /// The directory lock closes the case-alias gap before the target lock.
    pub(crate) fn lock_target(mut self, destination: &Path) -> Result<Self, super::model::Failure> {
        let mut observed_record = false;
        let work = (|| {
            if !self.is_sfx() || !matches!(self.coordination, Coordination::Directory { .. }) {
                return Err(invalid("target lock requires SFX directory coordination"));
            }
            self.verify()?;
            let requested = canonical_requested(destination)?;
            if crate::parent_or_current(&requested) != self.parent_path() {
                return Err(invalid("SFX target moved outside its held parent"));
            }
            self.requested = Name::from_path(&requested)?;
            self.target = Name::from_path(&canonical_target(&requested, false)?)?;
            let existing = self.existing_record();
            observed_record = !matches!(existing, Ok(None));
            if let Some(path) = existing? {
                let (_, record) = super::record::read(&path, &self)?;
                self.validate_record(&record)?;
                self.target = record.target;
            }
            let key = path_key(b"squallz-sfx-destination-v1\0", &self.target_path())?;
            let target = acquire_lock(
                &std::env::temp_dir().join(format!("squallz-sfx-transaction-{key}.lock")),
                None,
            )?;
            let coordination = std::mem::replace(&mut self.coordination, Coordination::Inspection);
            self.coordination = match coordination {
                Coordination::Directory { directory } => Coordination::Target { directory, target },
                _ => return Err(invalid("directory coordination ownership changed")),
            };
            self.verify_locked()
        })();
        match work {
            Ok(()) => Ok(self),
            Err(error) => {
                let mut failure = super::owner::scope_failure(&self, error);
                if !observed_record && matches!(self.existing_record(), Ok(None)) {
                    failure.visibility = super::model::Visibility::Unpublished;
                }
                Err(failure)
            }
        }
    }
    pub(crate) fn parent_path(&self) -> &Path {
        &self.parent.path
    }
    pub(crate) fn requested_path(&self) -> PathBuf {
        self.requested.join(self.parent_path())
    }
    pub(crate) fn target_path(&self) -> PathBuf {
        self.target.join(self.parent_path())
    }
    pub(crate) fn is_sfx(&self) -> bool {
        matches!(self.domain, Domain::Sfx)
    }
    pub(crate) fn archive_key(&self) -> Result<&str, FormatError> {
        match &self.domain {
            Domain::Archive { key } => Ok(key),
            Domain::Sfx => Err(invalid("not archive scope")),
        }
    }
    pub(crate) fn limit(&self) -> usize {
        if self.is_sfx() {
            64 * 1024
        } else {
            16 * 1024
        }
    }
    pub(crate) fn verify(&self) -> Result<(), FormatError> {
        self.parent.verify()
    }
    pub(crate) fn verify_locked(&self) -> Result<(), FormatError> {
        match &self.coordination {
            Coordination::Target { directory, target } => {
                if !directory.metadata()?.is_file() || !target.metadata()?.is_file() {
                    return Err(invalid("replacement coordination handle changed"));
                }
                self.verify()
            }
            _ => Err(invalid(
                "replacement mutation requires directory and target locks",
            )),
        }
    }
    pub(crate) fn sync(&self) -> Result<(), FormatError> {
        self.parent.sync()
    }
    /// Restore only the first post-move identity observed by the caller.
    pub(crate) fn restore_known(
        &self,
        from: &Path,
        to: &Path,
        observed: PathIdentity,
    ) -> Result<(), FormatError> {
        self.verify_locked()?;
        if identity_at(from)? != Some(observed) || identity_at(to)?.is_some() {
            return Ok(());
        }
        super::move_path_no_replace(from, to)?;
        self.sync()?;
        if identity_at(to)? != Some(observed) || identity_at(from)?.is_some() {
            return Err(invalid(
                "known unexpected move restoration remained uncertain",
            ));
        }
        Ok(())
    }
    pub(crate) fn anchor(&self, phase: RecordPhase) -> PathBuf {
        let name = match &self.domain {
            Domain::Archive { key } => {
                let suffix = match phase {
                    RecordPhase::Pending => ".pending",
                    RecordPhase::Installing => "",
                    RecordPhase::Completed => ".completed",
                };
                format!(".squallz-update-{key}{suffix}.json")
            }
            Domain::Sfx => match phase {
                RecordPhase::Pending | RecordPhase::Installing => {
                    ".squallz-sfx-transaction.json".into()
                }
                RecordPhase::Completed => ".squallz-sfx-completed.json".into(),
            },
        };
        self.parent_path().join(name)
    }
    pub(crate) fn phase(&self, path: &Path) -> Result<RecordPhase, FormatError> {
        self.phases()
            .into_iter()
            .find(|phase| self.anchor(*phase) == path)
            .ok_or_else(|| invalid("record does not occupy a trusted replacement anchor"))
    }
    pub(super) fn phases(&self) -> Vec<RecordPhase> {
        let mut phases = vec![RecordPhase::Installing, RecordPhase::Completed];
        if !self.is_sfx() {
            phases.push(RecordPhase::Pending);
        }
        phases
    }
    pub(crate) fn existing_record(&self) -> Result<Option<PathBuf>, FormatError> {
        let mut found = None;
        for phase in self.phases() {
            let path = self.anchor(phase);
            if identity_at(&path)?.is_some() {
                if found.is_some() {
                    return Err(invalid(
                        "multiple durable replacement records require recovery",
                    ));
                }
                found = Some(path);
            }
        }
        Ok(found)
    }
    pub(crate) fn require_clear(&self) -> Result<(), FormatError> {
        self.verify_locked()?;
        if self.existing_record()?.is_some() {
            return Err(invalid("replacement record already owns the target"));
        }
        if let Domain::Archive { key } = &self.domain {
            for entry in fs::read_dir(self.parent_path())? {
                let entry = entry?;
                if self.reserved_artifact(&entry.path()) {
                    return Err(FormatError::Io(io::Error::other(format!(
                        "reserved update work path has no durable record and was left untouched: {} (target {key})", entry.path().display()))));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn reserved_name(&self, name: &OsStr, kind: &str) -> bool {
        let Some(name) = name.to_str() else {
            return false;
        };
        let prefix = self.private_prefix(kind);
        let Some(value) = name.strip_prefix(&prefix) else {
            return false;
        };
        let value = if matches!(kind, "stage" | "journal") {
            let Some(value) = value.strip_suffix(".tmp") else {
                return false;
            };
            value
        } else {
            value
        };
        is_canonical_process_sequence(value)
    }
    pub(crate) fn reserve_name(&self, kind: &str) -> PathBuf {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let prefix = self.private_prefix(kind);
        let suffix = if matches!(kind, "stage" | "journal") {
            ".tmp"
        } else {
            ""
        };
        self.parent_path()
            .join(format!("{prefix}{}-{sequence}{suffix}", std::process::id()))
    }
    fn private_prefix(&self, kind: &str) -> String {
        match &self.domain {
            Domain::Archive { key } => format!(".squallz-update-{kind}-{}-", &key[..16]),
            Domain::Sfx => format!(".squallz-sfx-{kind}-"),
        }
    }
    pub(crate) fn validate_record(&self, record: &ReplacementRecord) -> Result<(), FormatError> {
        self.verify()?;
        if record.version != 1
            || record.parent_identity != self.parent.identity
            || record.evidence.previous().identity() == record.evidence.replacement().identity()
            || record.holder_identity == record.evidence.previous().identity()
            || record.holder_identity == record.evidence.replacement().identity()
            || !self.reserved_name(record.stage.os(), "stage")
            || !self.reserved_name(record.holder.os(), "holder")
            || record.stage.os() == record.holder.os()
        {
            return Err(invalid("record identity or private layout is not admitted"));
        }
        match (&self.domain, &record.evidence) {
            (Domain::Archive { .. }, PairEvidence::Archive { .. })
            | (Domain::Sfx, PairEvidence::SfxFile { .. } | PairEvidence::SfxTree { .. }) => {}
            _ => {
                return Err(invalid(
                    "record evidence does not match the trusted scoped anchor",
                ))
            }
        }
        self.validate_target(record)?;
        for name in [&record.requested, &record.target] {
            let path = name.join(self.parent_path());
            if self
                .phases()
                .into_iter()
                .any(|phase| self.anchor(phase) == path)
                || (self.is_sfx() && path == self.parent_path().join(".squallz-sfx-cleanup.json"))
                || name.os() == record.stage.os()
                || name.os() == record.holder.os()
            {
                return Err(invalid(
                    "public output collides with owned replacement work",
                ));
            }
        }
        Ok(())
    }
    fn validate_target(&self, record: &ReplacementRecord) -> Result<(), FormatError> {
        if self.is_sfx() {
            if matches!(self.coordination, Coordination::Inspection) {
                return Ok(());
            }
            if self.requested.os() != record.requested.os()
                && self.requested.os() != record.target.os()
                && self.target.os() != record.target.os()
            {
                return Err(invalid("another SFX target owns this directory"));
            }
        } else if path_key(
            b"squallz-update-target-v1\0",
            &record.target.join(self.parent_path()),
        )? != self.archive_key()?
        {
            return Err(invalid(
                "archive record target does not match its trusted key",
            ));
        }
        Ok(())
    }
    pub(crate) fn reserved_artifact(&self, candidate: &Path) -> bool {
        if self
            .phases()
            .into_iter()
            .any(|phase| self.anchor(phase) == candidate)
        {
            return true;
        }
        let Some(name) = candidate.file_name() else {
            return false;
        };
        if ["stage", "holder", "journal"]
            .into_iter()
            .any(|kind| self.reserved_name(name, kind))
        {
            return true;
        }
        if let Some(name) = name.to_str() {
            for (suffix, kind) in [
                (".empty-isolation", "holder"),
                (".previous-alias", "holder"),
                (".alias-isolation", "stage"),
                (".discard", "journal"),
            ] {
                if name
                    .strip_suffix(suffix)
                    .is_some_and(|base| self.reserved_name(OsStr::new(base), kind))
                {
                    return true;
                }
            }
        }
        matches!(
            name.to_str(),
            Some("previous" | "replacement" | "retired" | "alias-replacement")
        ) && candidate
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|parent| self.reserved_name(parent, "holder"))
    }
}

pub(crate) fn identity_at(path: &Path) -> Result<Option<PathIdentity>, FormatError> {
    match path_identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn canonical_requested(path: &Path) -> Result<PathBuf, FormatError> {
    let name = Name::from_path(path)?;
    Ok(name.join(&fs::canonicalize(crate::parent_or_current(path))?))
}
fn canonical_target(path: &Path, archive: bool) -> Result<PathBuf, FormatError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if archive && (metadata.file_type().is_symlink() || !metadata.is_file()) => {
            Err(FormatError::Unsupported(
                "archive target is not a regular file".into(),
            ))
        }
        Ok(metadata) if !metadata.file_type().is_symlink() => Ok(fs::canonicalize(path)?),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn path_key(domain: &[u8], canonical: &Path) -> Result<String, FormatError> {
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
        hasher.update(
            canonical
                .to_str()
                .ok_or_else(|| invalid("lock path must be UTF-8 on this platform"))?
                .as_bytes(),
        );
    }
    Ok(hasher.finalize().to_string())
}
fn acquire_lock(path: &Path, control: Option<&ControlToken>) -> Result<File, FormatError> {
    if fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(invalid("replacement lock must be a regular file"));
    }
    let file = open_lock_file(path)?;
    if !file.metadata()?.is_file() || path_identity(path)? != file_identity(&file)? {
        return Err(invalid("replacement lock changed while opening"));
    }
    if let Some(control) = control {
        loop {
            control.checkpoint()?;
            match fs4::FileExt::try_lock(&file) {
                Ok(()) => break,
                Err(fs4::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(50)),
                Err(fs4::TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    } else {
        fs4::FileExt::lock(&file)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || path_identity(path)? != file_identity(&file)?
    {
        return Err(invalid("replacement lock changed while acquiring"));
    }
    Ok(file)
}
#[cfg(unix)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};
    Ok(File::from(open(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?))
}
#[cfg(windows)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
#[cfg(not(any(unix, windows)))]
fn open_lock_file(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}
