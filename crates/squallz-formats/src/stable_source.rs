use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_format_api::{
    ArchiveSourceSet, ControlToken, FormatError, PhysicalFileIdentity, ReadSeek,
};

const STAGING_ROOT_NAME: &str = "squallz-format-staging-v1";
const SWEEP_LOCK_NAME: &str = ".sweep.lock";
const OWNER_SUFFIX: &str = ".owner";
const WORKSPACE_MARKER_NAME: &str = ".squallz-staging";
const OWNER_RECORD_VERSION: &str = "squallz-format-staging-v1";
const MAX_OWNER_RECORD_LEN: u64 = 512;
const MAX_STAGING_ROOT_ENTRIES: usize = 4_096;
const MAX_STAGING_WORKSPACE_ENTRIES: usize = 1_000_002;
const STAGING_COPY_CHUNK_SIZE: usize = 64 * 1024;
static STAGING_REGISTRY_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceIdentity {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(windows)]
    volume_serial: u64,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(windows)]
    creation_time: Option<u64>,
    #[cfg(windows)]
    last_write_time: Option<u64>,
}

impl SourceIdentity {
    pub(crate) fn from_file(file: &File) -> Result<Self, FormatError> {
        let metadata = file.metadata()?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(windows)]
        let information = winapi_util::file::information(file)?;
        #[cfg(windows)]
        if information.volume_serial_number() == 0 && information.file_index() == 0 {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "the filesystem did not provide a stable archive volume identity",
            )));
        }

        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            #[cfg(windows)]
            volume_serial: information.volume_serial_number(),
            #[cfg(windows)]
            file_index: information.file_index(),
            #[cfg(windows)]
            creation_time: information.creation_time(),
            #[cfg(windows)]
            last_write_time: information.last_write_time(),
        })
    }

    pub(crate) fn physical_identity(&self) -> PhysicalFileIdentity {
        #[cfg(unix)]
        {
            PhysicalFileIdentity::new(self.device, self.inode)
        }
        #[cfg(windows)]
        {
            PhysicalFileIdentity::new(self.volume_serial, self.file_index)
        }
        #[cfg(not(any(unix, windows)))]
        {
            PhysicalFileIdentity::new(0, 0)
        }
    }
}

pub(crate) struct BoundSourceSet {
    source_set: ArchiveSourceSet,
    bindings: Vec<(PathBuf, SourceIdentity)>,
}

impl BoundSourceSet {
    pub(crate) fn new(
        source_set: ArchiveSourceSet,
        bindings: Vec<(PathBuf, SourceIdentity)>,
    ) -> Result<Self, FormatError> {
        if source_set.members().len() != bindings.len()
            || source_set
                .members()
                .iter()
                .zip(&bindings)
                .any(|(member, (path, _))| member != path)
        {
            return Err(FormatError::CorruptArchive(
                "archive source bindings do not match the physical source set".into(),
            ));
        }
        Ok(Self {
            source_set,
            bindings,
        })
    }

    pub(crate) fn source_set(&self) -> &ArchiveSourceSet {
        &self.source_set
    }

    pub(crate) fn verify_current(
        &self,
        kind: &str,
        control: &ControlToken,
    ) -> Result<(), FormatError> {
        for (path, identity) in &self.bindings {
            control.checkpoint()?;
            if verify_source_binding(path, identity, kind).is_err() {
                control.checkpoint()?;
                return Err(FormatError::input_changed());
            }
        }
        control.checkpoint()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StagingKind {
    Rar,
    Wim,
    WimCreate,
    Zip,
}

impl StagingKind {
    fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "rar-volume" => Some(Self::Rar),
            "wim-volume" => Some(Self::Wim),
            "wim-native-split" => Some(Self::WimCreate),
            "zip-volume" => Some(Self::Zip),
            _ => None,
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Rar => "rar-volume",
            Self::Wim => "wim-volume",
            Self::WimCreate => "wim-native-split",
            Self::Zip => "zip-volume",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StagingOwnerRecord {
    workspace: String,
    token: String,
}

impl StagingOwnerRecord {
    fn new(workspace: String, sequence: u64, nanos: u128) -> Self {
        Self {
            workspace,
            token: format!("{nanos:032x}{sequence:016x}"),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        format!(
            "{OWNER_RECORD_VERSION}\n{}\n{}\n",
            self.workspace, self.token
        )
        .into_bytes()
    }

    fn parse(bytes: &[u8], expected_workspace: &str) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.split('\n');
        if lines.next()? != OWNER_RECORD_VERSION {
            return None;
        }
        let workspace = lines.next()?;
        let token = lines.next()?;
        if !lines.next()?.is_empty() || lines.next().is_some() {
            return None;
        }
        if workspace != expected_workspace
            || parse_workspace_kind(workspace).is_none()
            || token.len() != 48
            || !token
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return None;
        }
        Some(Self {
            workspace: workspace.to_string(),
            token: token.to_string(),
        })
    }
}

pub(crate) struct PrivateStagingDir {
    path: PathBuf,
    owner_path: PathBuf,
    owner: Option<File>,
    record: StagingOwnerRecord,
}

impl PrivateStagingDir {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for PrivateStagingDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Deref for PrivateStagingDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

impl Drop for PrivateStagingDir {
    fn drop(&mut self) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        let owner_identity = SourceIdentity::from_file(&owner).ok();
        let cleaned = cleanup_owned_workspace(&self.path, &self.record).unwrap_or(false);
        drop(owner);
        // Keep the sibling owner record when cleanup is incomplete so a later
        // process can retry without guessing which directory belongs to us.
        if cleaned {
            if let Some(identity) = owner_identity {
                let _ = remove_private_file_if_bound(&self.owner_path, &identity);
            }
        }
    }
}

pub(crate) fn resolve_selected_regular_path(
    source_path: &Path,
    expected_identity: PhysicalFileIdentity,
    kind: &str,
    control: &ControlToken,
) -> Result<(PathBuf, SourceIdentity), FormatError> {
    control.checkpoint()?;
    let source_metadata = fs::symlink_metadata(source_path)?;
    if !is_regular_source_metadata(&source_metadata) {
        return Err(FormatError::CorruptArchive(format!(
            "selected {kind} is not a regular file"
        )));
    }
    let source_file = open_regular_file_no_follow(source_path, kind)?;
    let source_identity = SourceIdentity::from_file(&source_file)?;
    if source_identity.physical_identity() != expected_identity {
        return Err(FormatError::CorruptArchive(format!(
            "selected {kind} path changed after it was opened"
        )));
    }
    let source_name = source_path.file_name().ok_or_else(|| {
        FormatError::CorruptArchive(format!("selected {kind} path has no file name"))
    })?;
    #[cfg(not(unix))]
    let source_canonical = fs::canonicalize(source_path)?;

    let mut identity_match = None;
    for entry in fs::read_dir(parent_or_current(source_path))? {
        control.checkpoint()?;
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !is_regular_source_metadata(&metadata) {
            continue;
        }
        #[cfg(unix)]
        let same_file = {
            use std::os::unix::fs::MetadataExt;
            metadata.dev() == source_metadata.dev() && metadata.ino() == source_metadata.ino()
        };
        #[cfg(not(unix))]
        let same_file =
            fs::canonicalize(entry.path()).is_ok_and(|candidate| candidate == source_canonical);
        if !same_file {
            continue;
        }
        if entry.file_name() == source_name {
            verify_source_binding(source_path, &source_identity, kind)?;
            return Ok((entry.path(), source_identity));
        }
        if identity_match.replace(entry.path()).is_some() {
            return Err(FormatError::CorruptArchive(format!(
                "selected {kind} has more than one directory entry"
            )));
        }
    }
    control.checkpoint()?;
    verify_source_binding(source_path, &source_identity, kind)?;
    let path = identity_match.ok_or_else(|| {
        FormatError::CorruptArchive(format!(
            "selected {kind} changed before its sibling set was discovered"
        ))
    })?;
    verify_source_binding(&path, &source_identity, kind)?;
    control.checkpoint()?;
    Ok((path, source_identity))
}

pub(crate) fn copy_selected_stream(
    src: &mut dyn ReadSeek,
    destination: &Path,
    control: &ControlToken,
) -> Result<(), FormatError> {
    control.checkpoint()?;
    src.seek(SeekFrom::Start(0))?;
    control.checkpoint()?;
    let mut output = create_private_file(destination)?;
    copy_stream_with_control(src, &mut output, control)?;
    control.checkpoint()?;
    output.flush()?;
    control.checkpoint()
}

pub(crate) fn copy_stable_source(
    source: &Path,
    expected: &SourceIdentity,
    destination: &Path,
    kind: &str,
    control: &ControlToken,
) -> Result<(), FormatError> {
    control.checkpoint()?;
    let mut input = open_regular_file_no_follow(source, kind)?;
    let initial = SourceIdentity::from_file(&input)?;
    if &initial != expected {
        return Err(FormatError::CorruptArchive(format!(
            "a {kind} changed before it was staged"
        )));
    }
    verify_source_binding(source, &initial, kind)?;
    control.checkpoint()?;
    let mut output = create_private_file(destination)?;
    copy_stream_with_control(&mut input, &mut output, control)?;
    control.checkpoint()?;
    output.flush()?;
    control.checkpoint()?;
    let final_identity = SourceIdentity::from_file(&input)?;
    if final_identity != initial {
        return Err(FormatError::CorruptArchive(format!(
            "a {kind} changed while it was being staged"
        )));
    }
    verify_source_binding(source, expected, kind)?;
    control.checkpoint()
}

fn copy_stream_with_control(
    input: &mut dyn Read,
    output: &mut dyn Write,
    control: &ControlToken,
) -> Result<(), FormatError> {
    let mut buffer = [0_u8; STAGING_COPY_CHUNK_SIZE];
    loop {
        control.checkpoint()?;
        let read = loop {
            match input.read(&mut buffer) {
                Ok(read) => break read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    control.checkpoint()?;
                }
                Err(error) => return Err(error.into()),
            }
        };
        if read == 0 {
            return control.checkpoint();
        }

        let mut written = 0;
        while written < read {
            control.checkpoint()?;
            match output.write(&buffer[written..read]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to stage the complete archive volume",
                    )
                    .into());
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    control.checkpoint()?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        control.checkpoint()?;
    }
}

pub(crate) fn create_private_staging_dir(prefix: &str) -> Result<PrivateStagingDir, FormatError> {
    create_private_staging_dir_in(&std::env::temp_dir(), prefix)
}

pub(crate) fn harden_private_staging_members(
    staging: &PrivateStagingDir,
) -> Result<(), FormatError> {
    let kind = parse_workspace_kind(&staging.record.workspace)
        .ok_or_else(|| FormatError::Other("private archive staging kind is unavailable".into()))?;
    for entry in fs::read_dir(staging.path())? {
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(WORKSPACE_MARKER_NAME) {
            continue;
        }
        if !archive_file_name_allowed(kind, &name) {
            return Err(FormatError::CorruptArchive(
                "private archive staging contains an unexpected member".into(),
            ));
        }
        let file = open_regular_file_no_follow(&entry.path(), "private archive staging member")?;
        harden_private_regular_file(&file)?;
    }
    Ok(())
}

fn create_private_staging_dir_in(
    base: &Path,
    prefix: &str,
) -> Result<PrivateStagingDir, FormatError> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let _process_guard = match STAGING_REGISTRY_MUTEX.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let kind = StagingKind::from_prefix(prefix)
        .ok_or_else(|| FormatError::Other("unsupported private archive staging kind".into()))?;
    let registry = base.join(STAGING_ROOT_NAME);
    create_or_verify_private_directory(&registry)?;
    let sweep_lock = open_or_create_private_lock_file(&registry.join(SWEEP_LOCK_NAME))?;
    fs4::FileExt::lock(&sweep_lock)?;
    reclaim_stale_workspaces(&registry)?;

    for _ in 0..128 {
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let workspace = format!(
            "{}-{}-{sequence}-{nanos}",
            kind.prefix(),
            std::process::id()
        );
        let path = registry.join(&workspace);
        let owner_path = registry.join(format!("{workspace}{OWNER_SUFFIX}"));
        let owner = match create_private_lock_file(&owner_path) {
            Ok(owner) => owner,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        let record = StagingOwnerRecord::new(workspace, sequence, nanos);
        let mut staging = PrivateStagingDir {
            path,
            owner_path,
            owner: Some(owner),
            record,
        };
        let owner = staging.owner.as_mut().ok_or_else(|| {
            FormatError::Other("private archive staging owner is unavailable".into())
        })?;
        fs4::FileExt::lock(owner)?;
        write_owner_record(owner, &staging.record)?;
        verify_private_file_binding(&staging.owner_path, owner)?;
        create_private_directory(&staging.path)?;
        write_workspace_marker(&staging.path, &staging.record)?;
        verify_private_directory(&staging.path)?;
        return Ok(staging);
    }
    Err(FormatError::Other(
        "could not reserve private archive staging directory".into(),
    ))
}

fn reclaim_stale_workspaces(registry: &Path) -> Result<(), FormatError> {
    let mut entries_seen = 0usize;
    for entry in fs::read_dir(registry)? {
        let entry = entry?;
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_STAGING_ROOT_ENTRIES {
            return Err(FormatError::ResourceLimitExceeded(
                "private archive staging registry has too many entries".into(),
            ));
        }
        let Some((workspace, _kind)) = parse_owner_name(&entry.file_name()) else {
            continue;
        };
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !is_private_regular_metadata(&metadata) {
            continue;
        }
        let Some(mut owner) = open_private_lock_file(&entry.path())? else {
            continue;
        };
        match fs4::FileExt::try_lock(&owner) {
            Ok(()) => {}
            // A reader keeps this lock for the entire lifetime of its staged
            // archive, so a busy owner is never eligible for reclamation.
            Err(fs4::TryLockError::WouldBlock) => continue,
            Err(fs4::TryLockError::Error(error)) => return Err(error.into()),
        }
        let Some(record) = read_owner_record(&mut owner, &workspace)? else {
            continue;
        };
        let workspace_path = registry.join(&record.workspace);
        if !cleanup_owned_workspace(&workspace_path, &record)? {
            continue;
        }
        let owner_identity = SourceIdentity::from_file(&owner)?;
        drop(owner);
        remove_private_file_if_bound(&entry.path(), &owner_identity)?;
    }
    Ok(())
}

fn cleanup_owned_workspace(
    workspace: &Path,
    record: &StagingOwnerRecord,
) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(workspace) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    if !is_private_directory_metadata(&metadata) {
        return Ok(false);
    }
    let kind = match parse_workspace_kind(&record.workspace) {
        Some(kind) => kind,
        None => return Ok(false),
    };
    let expected_marker = record.bytes();
    let mut marker_identity = None;
    let mut marker_matches = false;
    let mut archive_count = 0usize;
    let mut entries_seen = 0usize;
    // Validate the complete directory before deleting any archive member.
    // Unknown names and non-regular entries make the workspace ineligible.
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_STAGING_WORKSPACE_ENTRIES {
            return Ok(false);
        }
        let name = entry.file_name();
        if name == OsStr::new(WORKSPACE_MARKER_NAME) {
            if marker_identity.is_some() {
                return Ok(false);
            }
            let Some((bytes, identity)) = read_private_file(&entry.path())? else {
                return Ok(false);
            };
            marker_matches = bytes == expected_marker;
            marker_identity = Some(identity);
        } else if archive_file_name_allowed(kind, &name) {
            if !is_private_regular_metadata(&fs::symlink_metadata(entry.path())?) {
                return Ok(false);
            }
            archive_count = archive_count.saturating_add(1);
        } else {
            return Ok(false);
        }
    }
    if archive_count > 0 && !marker_matches {
        return Ok(false);
    }

    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(WORKSPACE_MARKER_NAME) {
            continue;
        }
        if !archive_file_name_allowed(kind, &name) {
            return Ok(false);
        }
        if !remove_private_regular_file(&entry.path())? {
            return Ok(false);
        }
    }

    if let Some(identity) = marker_identity {
        let marker_path = workspace.join(WORKSPACE_MARKER_NAME);
        if !remove_private_file_if_bound(&marker_path, &identity)? {
            return Ok(false);
        }
    }
    match fs::remove_dir(workspace) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn remove_private_regular_file(path: &Path) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    if !is_private_regular_metadata(&metadata) {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn write_owner_record(owner: &mut File, record: &StagingOwnerRecord) -> Result<(), FormatError> {
    owner.set_len(0)?;
    owner.seek(SeekFrom::Start(0))?;
    owner.write_all(&record.bytes())?;
    owner.sync_all()?;
    Ok(())
}

fn read_owner_record(
    owner: &mut File,
    expected_workspace: &str,
) -> Result<Option<StagingOwnerRecord>, FormatError> {
    owner.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    owner
        .take(MAX_OWNER_RECORD_LEN + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OWNER_RECORD_LEN {
        return Ok(None);
    }
    Ok(StagingOwnerRecord::parse(&bytes, expected_workspace))
}

fn write_workspace_marker(
    workspace: &Path,
    record: &StagingOwnerRecord,
) -> Result<(), FormatError> {
    let mut marker = create_private_file(&workspace.join(WORKSPACE_MARKER_NAME))?;
    marker.write_all(&record.bytes())?;
    marker.sync_all()?;
    Ok(())
}

fn read_private_file(path: &Path) -> Result<Option<(Vec<u8>, SourceIdentity)>, FormatError> {
    let Some(identity) = private_file_identity(path)? else {
        return Ok(None);
    };
    let file = open_regular_file_no_follow(path, "private archive staging marker")?;
    if SourceIdentity::from_file(&file)? != identity {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_OWNER_RECORD_LEN + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OWNER_RECORD_LEN {
        return Ok(None);
    }
    Ok(Some((bytes, identity)))
}

fn private_file_identity(path: &Path) -> Result<Option<SourceIdentity>, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !is_private_regular_metadata(&metadata) {
        return Ok(None);
    }
    let file = match open_regular_file_no_follow(path, "private archive staging file") {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let identity = SourceIdentity::from_file(&file)?;
    if verify_source_binding(path, &identity, "private archive staging file").is_err() {
        return Ok(None);
    }
    Ok(Some(identity))
}

fn remove_private_file_if_bound(
    path: &Path,
    expected: &SourceIdentity,
) -> Result<bool, FormatError> {
    let current = match private_file_identity(path)? {
        Some(current) => current,
        None => {
            return Ok(matches!(
                fs::symlink_metadata(path),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            ));
        }
    };
    if &current != expected {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn parse_owner_name(name: &OsStr) -> Option<(String, StagingKind)> {
    let workspace = name.to_str()?.strip_suffix(OWNER_SUFFIX)?;
    let kind = parse_workspace_kind(workspace)?;
    Some((workspace.to_string(), kind))
}

fn parse_workspace_kind(workspace: &str) -> Option<StagingKind> {
    for kind in [
        StagingKind::Rar,
        StagingKind::Wim,
        StagingKind::WimCreate,
        StagingKind::Zip,
    ] {
        let Some(rest) = workspace
            .strip_prefix(kind.prefix())
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            continue;
        };
        let mut fields = rest.split('-');
        fields.next()?.parse::<u32>().ok()?;
        fields.next()?.parse::<u64>().ok()?;
        fields.next()?.parse::<u128>().ok()?;
        if fields.next().is_none() {
            return Some(kind);
        }
    }
    None
}

fn archive_file_name_allowed(kind: StagingKind, name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    match kind {
        StagingKind::Rar => {
            name == "archive.rar"
                || numeric_name(name, "archive.part", ".rar", 1)
                || numeric_name(name, "archive.r", "", 2)
        }
        StagingKind::Wim => name == "archive.swm" || numeric_name(name, "archive", ".swm", 1),
        StagingKind::WimCreate => name == "source.wim" || split_wim_output_name_allowed(name),
        StagingKind::Zip => name == "archive.zip" || numeric_name(name, "archive.z", "", 2),
    }
}

fn split_wim_output_name_allowed(name: &str) -> bool {
    if name == "archive.swm" {
        return true;
    }
    let Some(digits) = name
        .strip_prefix("archive")
        .and_then(|rest| rest.strip_suffix(".swm"))
    else {
        return false;
    };
    if digits.starts_with('0') {
        return false;
    }
    digits
        .parse::<u32>()
        .is_ok_and(|part_number| (2..=u32::from(u16::MAX)).contains(&part_number))
}

fn numeric_name(name: &str, prefix: &str, suffix: &str, minimum_digits: usize) -> bool {
    let Some(digits) = name
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return false;
    };
    digits.len() >= minimum_digits
        && digits.len() <= 20
        && digits.as_bytes().iter().all(u8::is_ascii_digit)
}

fn create_or_verify_private_directory(path: &Path) -> Result<(), FormatError> {
    match create_private_directory(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    verify_private_directory(path)
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn verify_private_directory(path: &Path) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !is_private_directory_metadata(&metadata) {
        return Err(FormatError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private archive staging directory has unsafe ownership or permissions",
        )));
    }
    Ok(())
}

fn is_private_directory_metadata(metadata: &Metadata) -> bool {
    if !metadata.file_type().is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return false;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn is_private_regular_metadata(metadata: &Metadata) -> bool {
    if !is_regular_source_metadata(metadata) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return false;
        }
    }
    true
}

fn create_private_lock_file(path: &Path) -> io::Result<File> {
    let mut options = private_lock_file_options();
    options.create_new(true);
    options.open(path)
}

fn open_or_create_private_lock_file(path: &Path) -> Result<File, FormatError> {
    let mut options = private_lock_file_options();
    options.create(true);
    let file = options.open(path)?;
    verify_private_file_binding(path, &file)?;
    Ok(file)
}

fn open_private_lock_file(path: &Path) -> Result<Option<File>, FormatError> {
    let options = private_lock_file_options();
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if private_lock_file_is_still_bound(path, &file)? {
        Ok(Some(file))
    } else {
        Ok(None)
    }
}

fn private_lock_file_is_still_bound(path: &Path, file: &File) -> Result<bool, FormatError> {
    match verify_private_file_binding(path, file) {
        Ok(()) => Ok(true),
        // A live staging guard may remove its owner after this sweep has
        // opened the directory entry. The unbound file is not eligible for
        // cleanup, so preserve it and continue instead of failing the new
        // staging reservation.
        Err(FormatError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(FormatError::CorruptArchive(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn private_lock_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC;
        options.mode(0o600).custom_flags(flags.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn verify_private_file_binding(path: &Path, file: &File) -> Result<(), FormatError> {
    if !is_private_regular_metadata(&file.metadata()?) {
        return Err(FormatError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private archive staging lock has unsafe ownership or permissions",
        )));
    }
    let identity = SourceIdentity::from_file(file)?;
    verify_source_binding(path, &identity, "private archive staging lock")
}

pub(crate) fn create_private_file(path: &Path) -> Result<File, FormatError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(crate) fn harden_private_regular_file(file: &File) -> Result<(), FormatError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    if !is_private_regular_metadata(&file.metadata()?) {
        return Err(FormatError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private archive staging file has unsafe ownership or permissions",
        )));
    }
    Ok(())
}

pub(crate) fn open_regular_file_no_follow(path: &Path, kind: &str) -> Result<File, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !is_regular_source_metadata(&metadata) {
        return Err(FormatError::CorruptArchive(format!(
            "{kind} is not a regular file"
        )));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC;
        options.custom_flags(flags.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !is_regular_source_metadata(&file.metadata()?) {
        return Err(FormatError::CorruptArchive(format!(
            "{kind} is not a regular file"
        )));
    }
    Ok(file)
}

pub(crate) fn verify_source_binding(
    path: &Path,
    expected: &SourceIdentity,
    kind: &str,
) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !is_regular_source_metadata(&metadata) {
        return Err(FormatError::CorruptArchive(format!(
            "{kind} path changed while it was being read"
        )));
    }
    let current = open_regular_file_no_follow(path, kind)?;
    if SourceIdentity::from_file(&current)? != *expected {
        return Err(FormatError::CorruptArchive(format!(
            "{kind} path changed while it was being read"
        )));
    }
    Ok(())
}

pub(crate) fn is_regular_source_metadata(metadata: &Metadata) -> bool {
    if !metadata.file_type().is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

pub(crate) fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn native_split_wim_staging_accepts_only_canonical_members() {
        let kind = StagingKind::WimCreate;
        for name in [
            "source.wim",
            "archive.swm",
            "archive2.swm",
            "archive65535.swm",
        ] {
            assert!(archive_file_name_allowed(kind, OsStr::new(name)), "{name}");
        }
        for name in [
            "source2.wim",
            "archive1.swm",
            "archive02.swm",
            "archive65536.swm",
            "archive.swm.tmp",
        ] {
            assert!(!archive_file_name_allowed(kind, OsStr::new(name)), "{name}");
        }
    }

    const CRASH_WORKER_MODE: &str = "SQUALLZ_FORMAT_STAGING_CRASH_WORKER";
    const CRASH_WORKER_BASE: &str = "SQUALLZ_FORMAT_STAGING_CRASH_BASE";
    const CRASH_WORKER_TEST: &str = "stable_source::tests::private_staging_forced_kill_worker";
    const CRASH_WORKER_TIMEOUT: Duration = Duration::from_secs(10);
    const CRASH_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(20);

    fn test_base(tag: &str) -> io::Result<PathBuf> {
        static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let base = std::env::temp_dir().join(format!(
            "squallz-format-staging-test-{tag}-{}-{sequence}-{nanos}",
            std::process::id()
        ));
        create_private_directory(&base)?;
        Ok(base)
    }

    fn write_private_test_file(path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = create_private_file(path).map_err(|error| match error {
            FormatError::Io(error) => error,
            other => io::Error::other(other.to_string()),
        })?;
        file.write_all(contents)?;
        file.sync_all()
    }

    struct CancellingReadSeek {
        inner: io::Cursor<Vec<u8>>,
        control: ControlToken,
        reads: usize,
    }

    impl Read for CancellingReadSeek {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buffer)?;
            self.reads += 1;
            if read > 0 && self.reads == 1 {
                self.control.cancel();
            }
            Ok(read)
        }
    }

    impl Seek for CancellingReadSeek {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn staging_copy_observes_cancellation_between_bounded_io_calls() {
        let base = test_base("copy-cancel").expect("create test directory");
        let destination = base.join("archive.part");
        let control = ControlToken::default();
        let mut source = CancellingReadSeek {
            inner: io::Cursor::new(vec![0x5a; STAGING_COPY_CHUNK_SIZE * 3]),
            control: control.clone(),
            reads: 0,
        };

        let result = copy_selected_stream(&mut source, &destination, &control);

        assert!(matches!(result, Err(FormatError::Cancelled)));
        assert_eq!(source.reads, 1);
        assert_eq!(
            fs::metadata(&destination)
                .expect("inspect partial staging file")
                .len(),
            0
        );
        fs::remove_dir_all(base).expect("remove test directory");
    }

    struct CrashWorker {
        child: Option<Child>,
    }

    impl CrashWorker {
        fn spawn(base: &Path) -> io::Result<Self> {
            let child = Command::new(std::env::current_exe()?)
                .arg(CRASH_WORKER_TEST)
                .arg("--exact")
                .arg("--nocapture")
                .env(CRASH_WORKER_MODE, "rar-volume")
                .env(CRASH_WORKER_BASE, base)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            Ok(Self { child: Some(child) })
        }

        fn wait_until_ready(&mut self, ready: &Path) -> io::Result<()> {
            let started = Instant::now();
            loop {
                if ready.is_file() {
                    return Ok(());
                }
                let child = self
                    .child
                    .as_mut()
                    .ok_or_else(|| io::Error::other("staging crash worker is unavailable"))?;
                if let Some(status) = child.try_wait()? {
                    self.child = None;
                    return Err(io::Error::other(format!(
                        "staging crash worker exited before becoming ready: {status}"
                    )));
                }
                if started.elapsed() >= CRASH_WORKER_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "staging crash worker did not become ready",
                    ));
                }
                std::thread::sleep(CRASH_WORKER_POLL_INTERVAL);
            }
        }

        fn force_kill_and_wait(&mut self) -> io::Result<ExitStatus> {
            let mut child = self
                .child
                .take()
                .ok_or_else(|| io::Error::other("staging crash worker is unavailable"))?;
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

    fn wait_for_forced_termination<T>(_held: &T) -> ! {
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn removed_owner_opened_by_sweep_is_not_reported_as_archive_corruption(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = test_base("owner-unbound")?;
        let owner_path = base.join("rar-volume-1-1-1.owner");
        let owner = create_private_lock_file(&owner_path)?;
        fs::remove_file(&owner_path)?;

        assert!(!private_lock_file_is_still_bound(&owner_path, &owner)?);

        drop(owner);
        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn live_staging_is_not_reclaimed_and_normal_drop_removes_owned_files(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = test_base("live")?;
        let first = create_private_staging_dir_in(&base, "rar-volume")?;
        let first_path = first.path().to_path_buf();
        let first_owner = first.owner_path.clone();
        write_private_test_file(&first.join("archive.rar"), b"live RAR data")?;

        let second = create_private_staging_dir_in(&base, "zip-volume")?;
        let second_path = second.path().to_path_buf();
        let second_owner = second.owner_path.clone();
        write_private_test_file(&second.join("archive.zip"), b"live ZIP data")?;

        assert_eq!(fs::read(first.join("archive.rar"))?, b"live RAR data");
        assert_eq!(fs::read(second.join("archive.zip"))?, b"live ZIP data");
        drop(second);
        assert!(!second_path.exists());
        assert!(!second_owner.exists());
        assert!(first_path.exists());
        assert!(first_owner.exists());

        drop(first);
        assert!(!first_path.exists());
        assert!(!first_owner.exists());
        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn stale_sweep_leaves_workspace_with_unowned_entries_untouched(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = test_base("isolation")?;
        let seed = create_private_staging_dir_in(&base, "rar-volume")?;
        let registry = seed
            .path()
            .parent()
            .ok_or_else(|| io::Error::other("staging workspace has no registry"))?
            .to_path_buf();
        drop(seed);

        let workspace_name = "rar-volume-4294967295-1-1";
        let record = StagingOwnerRecord::new(workspace_name.to_string(), 1, 1);
        let owner_path = registry.join(format!("{workspace_name}{OWNER_SUFFIX}"));
        let mut owner = create_private_lock_file(&owner_path)?;
        owner.write_all(&record.bytes())?;
        owner.sync_all()?;
        let workspace = registry.join(workspace_name);
        create_private_directory(&workspace)?;
        write_workspace_marker(&workspace, &record)?;
        let unrelated = workspace.join("notes.bin");
        write_private_test_file(&unrelated, b"not a staged archive member")?;
        drop(owner);

        let trigger = create_private_staging_dir_in(&base, "zip-volume")?;
        assert_eq!(fs::read(&unrelated)?, b"not a staged archive member");
        assert!(workspace.exists());
        assert!(owner_path.exists());
        drop(trigger);
        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn private_staging_forced_kill_worker() -> Result<(), Box<dyn std::error::Error>> {
        let Some(prefix) = std::env::var_os(CRASH_WORKER_MODE) else {
            return Ok(());
        };
        let prefix = prefix
            .to_str()
            .ok_or_else(|| io::Error::other("staging crash worker mode is not UTF-8"))?;
        let base = std::env::var_os(CRASH_WORKER_BASE)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("staging crash worker base is missing"))?;
        let staging = create_private_staging_dir_in(&base, prefix)?;
        write_private_test_file(&staging.join("archive.rar"), b"private crash data")?;
        let workspace = staging
            .path()
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| io::Error::other("staging workspace name is not UTF-8"))?;
        write_private_test_file(&base.join("worker.ready"), workspace.as_bytes())?;
        wait_for_forced_termination(&staging)
    }

    #[test]
    fn forced_process_termination_reclaims_exact_staging_workspace(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = test_base("forced-kill")?;
        let ready = base.join("worker.ready");
        let adjacent = base.join("adjacent.bin");
        write_private_test_file(&adjacent, b"do not touch")?;

        let mut worker = CrashWorker::spawn(&base)?;
        worker.wait_until_ready(&ready)?;
        let workspace_name = String::from_utf8(fs::read(&ready)?)?;
        let registry = base.join(STAGING_ROOT_NAME);
        let crashed_workspace = registry.join(&workspace_name);
        let crashed_owner = registry.join(format!("{workspace_name}{OWNER_SUFFIX}"));
        assert!(crashed_workspace.join("archive.rar").is_file());
        assert!(crashed_owner.is_file());

        let live_probe = create_private_staging_dir_in(&base, "zip-volume")?;
        assert_eq!(
            fs::read(crashed_workspace.join("archive.rar"))?,
            b"private crash data"
        );
        assert!(crashed_owner.is_file());
        drop(live_probe);

        let status = worker.force_kill_and_wait()?;
        assert!(
            !status.success(),
            "staging crash worker exited successfully instead of being terminated"
        );

        let replacement = create_private_staging_dir_in(&base, "zip-volume")?;
        assert!(!crashed_workspace.exists());
        assert!(!crashed_owner.exists());
        assert_eq!(fs::read(&adjacent)?, b"do not touch");
        drop(replacement);
        fs::remove_dir_all(base)?;
        Ok(())
    }
}
