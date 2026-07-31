//! Cross-process ownership and cleanup for plaintext preview files.

use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_core::api::PhysicalFileIdentity;
use squallz_core::{
    open_directory_no_follow, open_regular_file_no_follow, open_regular_file_no_follow_read_write,
    physical_file_identity, physical_path_identity,
};

const REGISTRY_NAME: &str = "squallz-preview-v1";
const SWEEP_LOCK_NAME: &str = ".sweep.lock";
const OWNER_SUFFIX: &str = ".owner";
const MARKER_NAME: &str = ".squallz-preview";
const OWNER_RECORD_VERSION: &str = "squallz-preview-v1";
const MAX_OWNER_RECORD_BYTES: u64 = 512;
const MAX_REGISTRY_ENTRIES: usize = 1_024;
const MAX_WORKSPACE_ENTRIES: usize = 256;
static REGISTRY_MUTEX: Mutex<()> = Mutex::new(());
static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceRecord {
    workspace: String,
    token: String,
}

impl WorkspaceRecord {
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
        if !lines.next()?.is_empty()
            || lines.next().is_some()
            || workspace != expected_workspace
            || !valid_workspace_name(workspace)
            || token.len() != 48
            || !token
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return None;
        }
        Some(Self {
            workspace: workspace.to_owned(),
            token: token.to_owned(),
        })
    }
}

pub(crate) struct PreviewWorkspace {
    registry: PathBuf,
    path: PathBuf,
    owner_path: PathBuf,
    owner: Mutex<Option<File>>,
    record: WorkspaceRecord,
}

impl PreviewWorkspace {
    pub(crate) fn create_in(base: &Path) -> io::Result<Self> {
        let _process_guard = lock_unpoisoned(&REGISTRY_MUTEX);
        let registry = base.join(REGISTRY_NAME);
        create_or_verify_private_directory(&registry)?;
        let sweep_lock = open_or_create_private_lock_file(&registry.join(SWEEP_LOCK_NAME))?;
        fs4::FileExt::lock(&sweep_lock)?;
        reclaim_stale_workspaces(&registry)?;

        for _ in 0..128 {
            let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let workspace = format!("workspace-{}-{sequence}-{nanos}", std::process::id());
            let path = registry.join(&workspace);
            let owner_path = registry.join(format!("{workspace}{OWNER_SUFFIX}"));
            let mut owner = match create_private_lock_file(&owner_path) {
                Ok(owner) => owner,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            fs4::FileExt::lock(&owner)?;
            let record = WorkspaceRecord::new(workspace, sequence, nanos);
            write_record(&mut owner, &record)?;
            verify_private_file_binding(&owner_path, &owner)?;
            create_private_directory(&path)?;
            write_marker(&path, &record)?;
            verify_private_directory(&path)?;
            return Ok(Self {
                registry,
                path,
                owner_path,
                owner: Mutex::new(Some(owner)),
                record,
            });
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve private preview workspace",
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cleanup(&self) {
        let _process_guard = lock_unpoisoned(&REGISTRY_MUTEX);
        let Ok(sweep_lock) = open_or_create_private_lock_file(&self.registry.join(SWEEP_LOCK_NAME))
        else {
            return;
        };
        if fs4::FileExt::lock(&sweep_lock).is_err() {
            return;
        }
        let Some(owner) = lock_unpoisoned(&self.owner).take() else {
            return;
        };
        let owner_identity = physical_file_identity(&owner).ok();
        let cleaned = cleanup_owned_workspace(&self.path, &self.record).unwrap_or(false);
        drop(owner);
        if cleaned {
            if let Some(identity) = owner_identity {
                let _ = remove_private_file_if_bound(&self.owner_path, identity);
            }
        }
    }
}

impl Drop for PreviewWorkspace {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn reclaim_stale_workspaces(registry: &Path) -> io::Result<()> {
    let mut entries_seen = 0usize;
    for entry in fs::read_dir(registry)? {
        let entry = entry?;
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_REGISTRY_ENTRIES {
            return Err(io::Error::other(
                "private preview registry has too many entries",
            ));
        }
        let Some(workspace) = parse_owner_name(&entry.file_name()) else {
            continue;
        };
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !is_private_regular_metadata(&metadata) {
            continue;
        }
        let Some(mut owner) = open_private_lock_file(&entry.path())? else {
            continue;
        };
        match fs4::FileExt::try_lock(&owner) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => continue,
            Err(fs4::TryLockError::Error(error)) => return Err(error),
        }
        let Some(record) = read_record(&mut owner, &workspace)? else {
            continue;
        };
        if !cleanup_owned_workspace(&registry.join(&record.workspace), &record)? {
            continue;
        }
        let owner_identity = physical_file_identity(&owner)?;
        drop(owner);
        remove_private_file_if_bound(&entry.path(), owner_identity)?;
    }
    Ok(())
}

fn cleanup_owned_workspace(workspace: &Path, record: &WorkspaceRecord) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(workspace) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if !is_private_directory_metadata(&metadata) || open_directory_no_follow(workspace).is_err() {
        return Ok(false);
    }

    let expected_marker = record.bytes();
    let mut marker_identity = None;
    let mut marker_matches = false;
    let mut entries_seen = 0usize;
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_WORKSPACE_ENTRIES {
            return Ok(false);
        }
        let name = entry.file_name();
        if name == OsStr::new(MARKER_NAME) {
            if marker_identity.is_some() {
                return Ok(false);
            }
            let Some((bytes, identity)) = read_private_file(&entry.path())? else {
                return Ok(false);
            };
            marker_matches = bytes == expected_marker;
            marker_identity = Some(identity);
        } else if plaintext_name_allowed(&name) {
            if !is_private_regular_metadata(&fs::symlink_metadata(entry.path())?) {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    if entries_seen > 0 && !marker_matches {
        return Ok(false);
    }

    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == OsStr::new(MARKER_NAME) {
            continue;
        }
        if !plaintext_name_allowed(&name) || !remove_private_regular_file(&entry.path())? {
            return Ok(false);
        }
    }
    if let Some(identity) = marker_identity {
        if !remove_private_file_if_bound(&workspace.join(MARKER_NAME), identity)? {
            return Ok(false);
        }
    }
    match fs::remove_dir(workspace) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(error) => Err(error),
    }
}

fn write_record(owner: &mut File, record: &WorkspaceRecord) -> io::Result<()> {
    owner.set_len(0)?;
    owner.seek(SeekFrom::Start(0))?;
    owner.write_all(&record.bytes())?;
    owner.sync_all()
}

fn read_record(owner: &mut File, expected_workspace: &str) -> io::Result<Option<WorkspaceRecord>> {
    owner.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    owner
        .take(MAX_OWNER_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OWNER_RECORD_BYTES {
        return Ok(None);
    }
    Ok(WorkspaceRecord::parse(&bytes, expected_workspace))
}

fn write_marker(workspace: &Path, record: &WorkspaceRecord) -> io::Result<()> {
    let mut marker = create_private_file(&workspace.join(MARKER_NAME))?;
    marker.write_all(&record.bytes())?;
    marker.sync_all()
}

fn read_private_file(path: &Path) -> io::Result<Option<(Vec<u8>, PhysicalFileIdentity)>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !is_private_regular_metadata(&metadata) {
        return Ok(None);
    }
    let file = open_regular_file_no_follow(path)?;
    let identity = physical_file_identity(&file)?;
    if physical_path_identity(path)? != identity {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(MAX_OWNER_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OWNER_RECORD_BYTES {
        return Ok(None);
    }
    Ok(Some((bytes, identity)))
}

fn remove_private_regular_file(path: &Path) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if !is_private_regular_metadata(&metadata) {
        return Ok(false);
    }
    let file = open_regular_file_no_follow(path)?;
    let identity = physical_file_identity(&file)?;
    if physical_path_identity(path)? != identity {
        return Ok(false);
    }
    drop(file);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

fn remove_private_file_if_bound(path: &Path, expected: PhysicalFileIdentity) -> io::Result<bool> {
    let file = match open_regular_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if !is_private_regular_metadata(&file.metadata()?)
        || physical_file_identity(&file)? != expected
        || physical_path_identity(path)? != expected
    {
        return Ok(false);
    }
    drop(file);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

fn parse_owner_name(name: &OsStr) -> Option<String> {
    let workspace = name.to_str()?.strip_suffix(OWNER_SUFFIX)?;
    valid_workspace_name(workspace).then(|| workspace.to_owned())
}

fn valid_workspace_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("workspace-") else {
        return false;
    };
    let mut fields = rest.split('-');
    fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .is_some()
        && fields
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some()
        && fields
            .next()
            .and_then(|value| value.parse::<u128>().ok())
            .is_some()
        && fields.next().is_none()
}

fn plaintext_name_allowed(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(rest) = name
        .strip_prefix("entry-")
        .or_else(|| name.strip_prefix("nested-"))
    else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 128
        && rest
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn create_or_verify_private_directory(path: &Path) -> io::Result<()> {
    match create_private_directory(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
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

fn verify_private_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !is_private_directory_metadata(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private preview directory has unsafe permissions",
        ));
    }
    let directory = open_directory_no_follow(path)?;
    if physical_file_identity(&directory)? != physical_path_identity(path)? {
        return Err(io::Error::other(
            "private preview directory identity changed",
        ));
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
    true
}

fn is_private_regular_metadata(metadata: &Metadata) -> bool {
    if !metadata.file_type().is_file() {
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
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn open_or_create_private_lock_file(path: &Path) -> io::Result<File> {
    if let Some(file) = open_private_lock_file(path)? {
        return Ok(file);
    }
    match create_private_lock_file(path) {
        Ok(file) => {
            verify_private_file_binding(path, &file)?;
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => open_private_lock_file(path)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "private preview lock is unsafe",
                )
            }),
        Err(error) => Err(error),
    }
}

fn open_private_lock_file(path: &Path) -> io::Result<Option<File>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !is_private_regular_metadata(&metadata) {
        return Ok(None);
    }
    let file = match open_regular_file_no_follow_read_write(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if verify_private_file_binding(path, &file).is_err() {
        return Ok(None);
    }
    Ok(Some(file))
}

fn verify_private_file_binding(path: &Path, file: &File) -> io::Result<()> {
    if !is_private_regular_metadata(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private preview lock has unsafe permissions",
        ));
    }
    let identity = physical_file_identity(file)?;
    if physical_path_identity(path)? != identity {
        return Err(io::Error::other("private preview lock identity changed"));
    }
    Ok(())
}

fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    const CRASH_WORKER_MODE: &str = "SQUALLZ_PREVIEW_CRASH_WORKER";
    const CRASH_WORKER_BASE: &str = "SQUALLZ_PREVIEW_CRASH_BASE";
    const CRASH_WORKER_TEST: &str =
        "preview_workspace::tests::preview_workspace_forced_kill_worker";
    const CRASH_WORKER_TIMEOUT: Duration = Duration::from_secs(10);
    const CRASH_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(20);

    fn write_private_test_file(path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = create_private_file(path)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    fn abandon_workspace(workspace: PreviewWorkspace) {
        let owner = lock_unpoisoned(&workspace.owner)
            .take()
            .expect("preview owner should be available");
        drop(owner);
        drop(workspace);
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
                .env(CRASH_WORKER_MODE, "1")
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
                    .ok_or_else(|| io::Error::other("preview crash worker is unavailable"))?;
                if let Some(status) = child.try_wait()? {
                    self.child = None;
                    return Err(io::Error::other(format!(
                        "preview crash worker exited before becoming ready: {status}"
                    )));
                }
                if started.elapsed() >= CRASH_WORKER_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "preview crash worker did not become ready",
                    ));
                }
                std::thread::sleep(CRASH_WORKER_POLL_INTERVAL);
            }
        }

        fn force_kill_and_wait(&mut self) -> io::Result<ExitStatus> {
            let mut child = self
                .child
                .take()
                .ok_or_else(|| io::Error::other("preview crash worker is unavailable"))?;
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
    fn live_workspace_is_not_reclaimed_by_another_instance() {
        let base = tempfile::tempdir().expect("test base should initialize");
        let first =
            PreviewWorkspace::create_in(base.path()).expect("first workspace should initialize");
        let first_root = first.path.clone();
        let first_owner = first.owner_path.clone();
        let first_file = first.path.join("entry-live.txt");
        write_private_test_file(&first_file, b"private preview")
            .expect("live preview should be written");

        let second =
            PreviewWorkspace::create_in(base.path()).expect("second workspace should initialize");
        assert!(first_root.exists());
        assert!(first_owner.exists());
        assert!(first_file.exists());

        drop(second);
        assert!(first_root.exists());
        drop(first);
        assert!(!first_root.exists());
        assert!(!first_owner.exists());
    }

    #[test]
    fn stale_owned_workspace_is_reclaimed_by_the_next_instance() {
        let base = tempfile::tempdir().expect("test base should initialize");
        let stale =
            PreviewWorkspace::create_in(base.path()).expect("stale workspace should initialize");
        let stale_root = stale.path.clone();
        let stale_owner = stale.owner_path.clone();
        write_private_test_file(&stale_root.join("entry-stale.txt"), b"private preview")
            .expect("stale preview should be written");
        abandon_workspace(stale);

        let replacement = PreviewWorkspace::create_in(base.path())
            .expect("replacement workspace should initialize");
        assert!(!stale_root.exists());
        assert!(!stale_owner.exists());
        drop(replacement);
    }

    #[test]
    fn stale_workspace_with_unknown_member_is_left_untouched() {
        let base = tempfile::tempdir().expect("test base should initialize");
        let stale =
            PreviewWorkspace::create_in(base.path()).expect("stale workspace should initialize");
        let stale_root = stale.path.clone();
        let stale_owner = stale.owner_path.clone();
        let unrelated = stale_root.join("notes.bin");
        write_private_test_file(&unrelated, b"do not remove")
            .expect("unrelated file should be written");
        abandon_workspace(stale);

        let replacement = PreviewWorkspace::create_in(base.path())
            .expect("replacement workspace should initialize");
        assert_eq!(
            fs::read(&unrelated).expect("unrelated file should remain"),
            b"do not remove"
        );
        assert!(stale_root.exists());
        assert!(stale_owner.exists());
        drop(replacement);
    }

    #[test]
    fn stale_workspace_with_mismatched_marker_is_left_untouched() {
        let base = tempfile::tempdir().expect("test base should initialize");
        let stale =
            PreviewWorkspace::create_in(base.path()).expect("stale workspace should initialize");
        let stale_root = stale.path.clone();
        let stale_owner = stale.owner_path.clone();
        let preview = stale_root.join("entry-stale.txt");
        write_private_test_file(&preview, b"private preview")
            .expect("stale preview should be written");
        fs::write(stale_root.join(MARKER_NAME), b"mismatched marker")
            .expect("marker should be changed");
        abandon_workspace(stale);

        let replacement = PreviewWorkspace::create_in(base.path())
            .expect("replacement workspace should initialize");
        assert_eq!(
            fs::read(&preview).expect("preview should remain"),
            b"private preview"
        );
        assert!(stale_root.exists());
        assert!(stale_owner.exists());
        drop(replacement);
    }

    #[cfg(unix)]
    #[test]
    fn stale_workspace_with_symlink_is_left_untouched() {
        use std::os::unix::fs::symlink;

        let base = tempfile::tempdir().expect("test base should initialize");
        let outside = base.path().join("outside.txt");
        fs::write(&outside, b"outside").expect("outside file should be written");
        let stale =
            PreviewWorkspace::create_in(base.path()).expect("stale workspace should initialize");
        let stale_root = stale.path.clone();
        let stale_owner = stale.owner_path.clone();
        let link = stale_root.join("entry-link");
        symlink(&outside, &link).expect("preview symlink should be created");
        abandon_workspace(stale);

        let replacement = PreviewWorkspace::create_in(base.path())
            .expect("replacement workspace should initialize");
        assert!(link.symlink_metadata().is_ok());
        assert_eq!(
            fs::read(&outside).expect("outside file should remain"),
            b"outside"
        );
        assert!(stale_root.exists());
        assert!(stale_owner.exists());
        drop(replacement);
    }

    #[test]
    fn preview_workspace_forced_kill_worker() -> Result<(), Box<dyn std::error::Error>> {
        if std::env::var_os(CRASH_WORKER_MODE).is_none() {
            return Ok(());
        }
        let base = std::env::var_os(CRASH_WORKER_BASE)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("preview crash worker base is missing"))?;
        let workspace = PreviewWorkspace::create_in(&base)?;
        write_private_test_file(
            &workspace.path.join("nested-crash.zip"),
            b"private nested archive",
        )?;
        let workspace_name = workspace
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| io::Error::other("preview workspace name is not UTF-8"))?;
        write_private_test_file(&base.join("worker.ready"), workspace_name.as_bytes())?;
        wait_for_forced_termination(&workspace)
    }

    #[test]
    fn forced_process_termination_reclaims_exact_workspace(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let base = tempfile::tempdir()?;
        let ready = base.path().join("worker.ready");
        let adjacent = base.path().join("adjacent.bin");
        write_private_test_file(&adjacent, b"do not touch")?;

        let mut worker = CrashWorker::spawn(base.path())?;
        worker.wait_until_ready(&ready)?;
        let workspace_name = String::from_utf8(fs::read(&ready)?)?;
        let registry = base.path().join(REGISTRY_NAME);
        let crashed_workspace = registry.join(&workspace_name);
        let crashed_owner = registry.join(format!("{workspace_name}{OWNER_SUFFIX}"));
        assert!(crashed_workspace.join("nested-crash.zip").is_file());
        assert!(crashed_owner.is_file());

        let live_probe = PreviewWorkspace::create_in(base.path())?;
        assert!(crashed_workspace.exists());
        assert!(crashed_owner.exists());
        drop(live_probe);

        let status = worker.force_kill_and_wait()?;
        assert!(
            !status.success(),
            "preview crash worker exited successfully instead of being terminated"
        );

        let replacement = PreviewWorkspace::create_in(base.path())?;
        assert!(!crashed_workspace.exists());
        assert!(!crashed_owner.exists());
        assert_eq!(fs::read(&adjacent)?, b"do not touch");
        drop(replacement);
        Ok(())
    }
}
