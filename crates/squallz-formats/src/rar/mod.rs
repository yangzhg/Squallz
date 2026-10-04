//! RAR/CBR read bridge.
//!
//! Squallz does not create RAR archives and does not link unrar code into
//! the binary. This bridge prefers the `7zz`/`7z` external reader, uses a
//! compatible `bsdtar`/libarchive reader for a narrow set of single-file
//! decoder gaps, and can use an installed `unrar` executable for
//! confirmed-unencrypted RAR7 entry streams that 7-Zip cannot decode.
//! External readers only list or stream entries; extraction still flows
//! through the shared safe extraction engine.

mod bsdtar_listing;
mod volume;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveSourceSet, ArchiveWriter, ControlToken, CreateOptions,
    EntryMeta, EntryPath, EntryType, FormatCapabilities, FormatError, OpenOptions, Password,
    PhysicalFileIdentity, ReadSeek, WriteSeek,
};

#[cfg(all(test, unix))]
use squallz_format_api::SafetyLimits;

use crate::external_process::ControlledChild;
use crate::external_reader::{ExternalArchiveReader, ExternalArchiveSource};
use crate::sevenzip_bridge;
pub(crate) use volume::StagedRarSet;

const RAR4_MAGIC: &[u8] = b"Rar!\x1A\x07\x00";
const RAR5_MAGIC: &[u8] = b"Rar!\x1A\x07\x01\x00";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnrarBackendSource {
    Environment,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnrarBackendStatus {
    source: Option<UnrarBackendSource>,
    selected: Option<PathBuf>,
    executable: Option<PathBuf>,
    configured: bool,
}

impl UnrarBackendStatus {
    pub fn available(&self) -> bool {
        self.executable.is_some()
    }

    pub fn configured(&self) -> bool {
        self.configured
    }

    pub fn source(&self) -> Option<UnrarBackendSource> {
        self.source
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }
}

pub(crate) struct RarFormat;

impl ArchiveFormat for RarFormat {
    fn id(&self) -> &'static str {
        "rar"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rar", "cbr"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: false,
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: false,
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        head.starts_with(RAR4_MAGIC) || head.starts_with(RAR5_MAGIC)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        self.open_with_control(src, opts, &ControlToken::default())
    }

    fn open_with_control(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        let password = opts.password.clone();
        let staged = StagedRarSet::single_with_control(src, ctl)?;
        Ok(Box::new(open_staged_rar(staged, password, ctl)?))
    }

    fn open_file(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        self.open_file_with_control(
            source_path,
            source_identity,
            src,
            opts,
            &ControlToken::default(),
        )
    }

    fn open_file_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        let password = opts.password.clone();
        let staged =
            StagedRarSet::from_bound_file_with_control(source_path, source_identity, src, ctl)?;
        Ok(Box::new(open_staged_rar(staged, password, ctl)?))
    }

    fn probe_file_source_set(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        volume::probe_bound_file(source_path, source_identity, src)
    }

    fn probe_file_source_set_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
        ctl: &ControlToken,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        volume::probe_bound_file_with_control(source_path, source_identity, src, ctl)
    }

    fn create(
        &self,
        _dst: Box<dyn WriteSeek>,
        _opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Err(FormatError::Unsupported(
            "Squallz does not create RAR archives".into(),
        ))
    }
}

fn open_staged_rar(
    staged: StagedRarSet,
    password: Option<Password>,
    ctl: &ControlToken,
) -> Result<ExternalArchiveReader, FormatError> {
    let selected = RarBackend::select(
        staged.path(),
        staged.is_native_multivolume(),
        password.is_some(),
        ctl,
    )
    .map_err(|error| staged.remap_external_error(error))?;
    let backend = selected.backend;
    let listing = match selected.listing {
        Some(listing) => listing,
        None => backend
            .list_entries(staged.path(), password.as_ref(), ctl)
            .map_err(|error| staged.remap_external_error(error))?,
    };
    staged.validate_external_volume_properties(listing.archive)?;
    let entries = listing.entries;
    if entries.is_empty() && staged.len()? > 0 {
        return Err(FormatError::CorruptArchive(format!(
            "{} listed no entries for a non-empty RAR archive",
            backend.name()
        )));
    }
    Ok(ExternalArchiveReader::new(
        ExternalArchiveSource::Rar { staged, backend },
        entries,
        password,
        ctl,
    ))
}

pub(crate) enum RarBackend {
    SevenZip(PathBuf),
    SevenZipUnrar { sevenzip: PathBuf, unrar: PathBuf },
    Bsdtar(PathBuf),
}

struct RarListing {
    entries: Vec<EntryMeta>,
    archive: Option<sevenzip_bridge::SevenZipArchiveProperties>,
}

struct SelectedRarBackend {
    backend: RarBackend,
    listing: Option<RarListing>,
}

impl RarBackend {
    fn select(
        archive: &Path,
        native_multivolume: bool,
        password_supplied: bool,
        ctl: &ControlToken,
    ) -> Result<SelectedRarBackend, FormatError> {
        ctl.checkpoint()?;
        if password_supplied {
            return sevenzip_bridge::sevenzip_tool_if_configured_or_installed()
                .map(Self::SevenZip)
                .map(|backend| SelectedRarBackend {
                    backend,
                    listing: None,
                })
                .ok_or_else(|| {
                    FormatError::DependencyMissing("7zz/7z with secure RAR password input".into())
                });
        }
        if native_multivolume {
            let sevenzip =
                sevenzip_bridge::sevenzip_tool_if_configured_or_installed().ok_or_else(|| {
                    FormatError::DependencyMissing(
                        "7zz/7z with native RAR multi-volume support".into(),
                    )
                })?;
            let listing = sevenzip_bridge::list_entries_with_archive_properties(
                &sevenzip, archive, None, ctl,
            )?;
            let rar7_v6 = listing.compatibility.rar7_v6_confirmed_unencrypted;
            let listing = RarListing::from_sevenzip(listing);
            if rar7_v6 {
                if let Some(unrar) = unrar_tool_if_available() {
                    return Ok(SelectedRarBackend {
                        backend: Self::SevenZipUnrar { sevenzip, unrar },
                        listing: Some(listing),
                    });
                }
            }
            return Ok(SelectedRarBackend {
                backend: Self::SevenZip(sevenzip),
                listing: Some(listing),
            });
        }
        if std::env::var_os("SQUALLZ_BSDTAR").is_some() {
            return Ok(SelectedRarBackend {
                backend: Self::Bsdtar(bsdtar_tool()),
                listing: None,
            });
        }
        if let Some(tool) = sevenzip_bridge::sevenzip_tool_if_configured_or_installed() {
            let listing =
                sevenzip_bridge::list_entries_with_archive_properties(&tool, archive, None, ctl)?;
            return select_single_unencrypted_backend(
                tool,
                archive,
                listing,
                bsdtar_tool_if_available(),
                unrar_tool_if_available(),
                ctl,
            );
        }
        Ok(SelectedRarBackend {
            backend: Self::Bsdtar(bsdtar_tool()),
            listing: None,
        })
    }

    fn name(&self) -> &'static str {
        match self {
            Self::SevenZip(_) => "7zz/7z",
            Self::SevenZipUnrar { .. } => "7zz/7z + unrar",
            Self::Bsdtar(_) => "bsdtar",
        }
    }

    fn list_entries(
        &self,
        archive: &Path,
        password: Option<&Password>,
        ctl: &ControlToken,
    ) -> Result<RarListing, FormatError> {
        match self {
            Self::SevenZip(tool) | Self::SevenZipUnrar { sevenzip: tool, .. } => {
                let listing = sevenzip_bridge::list_entries_with_archive_properties(
                    tool, archive, password, ctl,
                )?;
                Ok(RarListing::from_sevenzip(listing))
            }
            Self::Bsdtar(tool) => Ok(RarListing {
                entries: bsdtar_listing::list_entries(tool, archive, ctl)?,
                archive: None,
            }),
        }
    }

    pub(crate) fn read_entry(
        &self,
        archive: &Path,
        path: &EntryPath,
        password: Option<&Password>,
        control: &ControlToken,
    ) -> Result<Box<dyn Read>, FormatError> {
        match self {
            Self::SevenZip(tool) => {
                sevenzip_bridge::read_entry_stdout(tool, archive, path, password, control)
            }
            Self::SevenZipUnrar { unrar, .. } => {
                read_unrar_entry_stdout(unrar, archive, path, control)
            }
            Self::Bsdtar(tool) => read_bsdtar_entry_stdout(tool, archive, path, control),
        }
    }
}

impl RarListing {
    fn from_sevenzip(listing: sevenzip_bridge::SevenZipListing) -> Self {
        Self {
            entries: listing.entries,
            archive: Some(listing.archive),
        }
    }
}

fn select_single_unencrypted_backend(
    sevenzip: PathBuf,
    archive: &Path,
    listing: sevenzip_bridge::SevenZipListing,
    bsdtar: Option<PathBuf>,
    unrar: Option<PathBuf>,
    ctl: &ControlToken,
) -> Result<SelectedRarBackend, FormatError> {
    let compatibility = listing.compatibility;
    let listing = RarListing::from_sevenzip(listing);

    if compatibility.rar7_v6_confirmed_unencrypted {
        if let Some(bsdtar) = bsdtar {
            return Ok(SelectedRarBackend {
                backend: RarBackend::Bsdtar(bsdtar),
                listing: None,
            });
        }
        if let Some(unrar) = unrar {
            return Ok(SelectedRarBackend {
                backend: RarBackend::SevenZipUnrar { sevenzip, unrar },
                listing: Some(listing),
            });
        }
    } else if compatibility.legacy_p7zip_rar5_decoder_gap {
        if let Some(bsdtar) = bsdtar {
            match bsdtar_listing_matches(&listing.entries, &bsdtar, archive, ctl) {
                Ok(true) => {
                    return Ok(SelectedRarBackend {
                        backend: RarBackend::Bsdtar(bsdtar),
                        listing: Some(listing),
                    });
                }
                Err(FormatError::Cancelled) => return Err(FormatError::Cancelled),
                Ok(false) | Err(_) => {}
            }
        }
    }

    Ok(SelectedRarBackend {
        backend: RarBackend::SevenZip(sevenzip),
        listing: Some(listing),
    })
}

fn bsdtar_listing_matches(
    sevenzip_entries: &[EntryMeta],
    tool: &Path,
    archive: &Path,
    ctl: &ControlToken,
) -> Result<bool, FormatError> {
    if sevenzip_entries.iter().any(|entry| entry.encrypted) {
        return Ok(false);
    }
    let Some(sevenzip_files) = literal_regular_files(sevenzip_entries, false) else {
        return Ok(false);
    };
    let bsdtar_entries = bsdtar_listing::list_entries(tool, archive, ctl)?;
    let Some(bsdtar_files) = literal_regular_files(&bsdtar_entries, true) else {
        return Ok(false);
    };
    Ok(!sevenzip_files.is_empty() && sevenzip_files == bsdtar_files)
}

fn literal_regular_files(
    entries: &[EntryMeta],
    require_detailed_metadata: bool,
) -> Option<BTreeMap<Vec<u8>, u64>> {
    let mut files = BTreeMap::new();
    for entry in entries {
        if !matches!(entry.entry_type, EntryType::File)
            || entry.encrypted
            || (require_detailed_metadata && entry.unix_mode.is_none())
            || entry.path.raw != entry.path.display.as_bytes()
            || entry.path.raw.iter().any(|byte| {
                byte.is_ascii_control() || matches!(byte, b'*' | b'?' | b'[' | b']' | b'\\')
            })
            || files.insert(entry.path.raw.clone(), entry.size).is_some()
        {
            return None;
        }
    }
    Some(files)
}

fn bsdtar_tool() -> PathBuf {
    if let Some(path) = std::env::var_os("SQUALLZ_BSDTAR") {
        return PathBuf::from(path);
    }
    if Path::new("/usr/bin/bsdtar").exists() {
        return PathBuf::from("/usr/bin/bsdtar");
    }
    PathBuf::from("bsdtar")
}

fn bsdtar_tool_if_available() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SQUALLZ_BSDTAR") {
        return Some(PathBuf::from(path));
    }
    if Path::new("/usr/bin/bsdtar").exists() {
        return Some(PathBuf::from("/usr/bin/bsdtar"));
    }
    sevenzip_bridge::find_on_path(OsStr::new("bsdtar"), std::env::var_os("PATH").as_deref())
}

fn unrar_tool_if_available() -> Option<PathBuf> {
    unrar_backend_status().executable().map(Path::to_path_buf)
}

pub fn unrar_backend_status() -> UnrarBackendStatus {
    let configured = std::env::var_os("SQUALLZ_UNRAR");
    let search_path = std::env::var_os("PATH");
    detect_unrar_backend(configured.as_deref(), search_path.as_deref())
}

fn detect_unrar_backend(
    configured: Option<&OsStr>,
    search_path: Option<&OsStr>,
) -> UnrarBackendStatus {
    if let Some(configured) = configured {
        let selected = PathBuf::from(configured);
        let executable = sevenzip_bridge::resolve_command_path(&selected, search_path);
        return UnrarBackendStatus {
            source: Some(UnrarBackendSource::Environment),
            selected: Some(selected),
            executable,
            configured: true,
        };
    }

    if let Some(executable) = sevenzip_bridge::find_on_path(OsStr::new("unrar"), search_path) {
        return UnrarBackendStatus {
            source: Some(UnrarBackendSource::Path),
            selected: Some(executable.clone()),
            executable: Some(executable),
            configured: false,
        };
    }

    UnrarBackendStatus {
        source: None,
        selected: None,
        executable: None,
        configured: false,
    }
}

fn read_bsdtar_entry_stdout(
    tool: &Path,
    archive: &Path,
    path: &EntryPath,
    control: &ControlToken,
) -> Result<Box<dyn Read>, FormatError> {
    let mut child = Command::new(tool)
        .arg("-xOf")
        .arg(archive)
        .arg("--")
        .arg(&path.display)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| map_tool_spawn_error(error, "bsdtar with RAR/libarchive support"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FormatError::Other("bsdtar did not provide stdout".into()))?;
    Ok(Box::new(CommandStdoutReader {
        child: ControlledChild::new(child, control),
        stdout,
        backend: "bsdtar",
        entry: path.display.clone(),
        control: control.clone(),
        finished: false,
    }))
}

fn read_unrar_entry_stdout(
    tool: &Path,
    archive: &Path,
    path: &EntryPath,
    control: &ControlToken,
) -> Result<Box<dyn Read>, FormatError> {
    if path.display.bytes().any(|byte| matches!(byte, b'*' | b'?')) {
        return Err(FormatError::Unsupported(
            "unrar cannot select an entry containing wildcard characters literally".into(),
        ));
    }
    let mut child = Command::new(tool)
        .arg("p")
        .arg("-inul")
        .arg("-p-")
        .arg("-cfg-")
        .arg("-@")
        .arg(archive)
        .arg("--")
        .arg(&path.display)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| map_tool_spawn_error(error, "unrar with RAR7 read support"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FormatError::Other("unrar did not provide stdout".into()))?;
    Ok(Box::new(CommandStdoutReader {
        child: ControlledChild::new(child, control),
        stdout,
        backend: "unrar",
        entry: path.display.clone(),
        control: control.clone(),
        finished: false,
    }))
}

fn map_tool_spawn_error(e: io::Error, dependency: &'static str) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing(dependency.into())
    } else {
        FormatError::from(e)
    }
}

struct CommandStdoutReader {
    child: ControlledChild,
    stdout: ChildStdout,
    backend: &'static str,
    entry: String,
    control: ControlToken,
    finished: bool,
}

impl Read for CommandStdoutReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        self.control.checkpoint().map_err(io::Error::other)?;
        let n = match self.stdout.read(buf) {
            Ok(read) => read,
            Err(_) if self.control.is_cancelled() => {
                return Err(io::Error::other(FormatError::Cancelled));
            }
            Err(error) => return Err(error),
        };
        if n > 0 {
            return Ok(n);
        }
        let status = self.child.wait()?;
        self.finished = true;
        if self.control.is_cancelled() {
            Err(io::Error::other(FormatError::Cancelled))
        } else if status.success() {
            Ok(0)
        } else {
            Err(io::Error::other(format!(
                "{} failed while reading {}",
                self.backend, self.entry
            )))
        }
    }
}

impl Drop for CommandStdoutReader {
    fn drop(&mut self) {
        if !self.finished {
            self.child.terminate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_path(tag: &str, ext: &str) -> PathBuf {
        let sequence = TEMP_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "squallz-rar-{tag}-{}-{sequence}-{nanos}-{ext}",
            std::process::id()
        ))
    }

    fn write_test_executable(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        });
        fs::write(&path, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
        path
    }

    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                old: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn rar_format_declares_read_only_capabilities_and_magic() {
        let format = RarFormat;
        assert_eq!(format.id(), "rar");
        assert_eq!(format.extensions(), ["rar", "cbr"]);
        let caps = format.capabilities();
        assert!(!caps.can_create);
        assert!(caps.can_extract);
        assert!(caps.can_test);
        assert!(format.sniff(RAR4_MAGIC, &[]));
        assert!(format.sniff(RAR5_MAGIC, &[]));
    }

    #[test]
    fn unrar_backend_status_distinguishes_configuration_and_path() {
        let root = temp_path("unrar-backend-status", "dir");
        let _ = fs::remove_dir_all(&root);
        let path_dir = root.join("path");
        let path_tool = write_test_executable(&path_dir, "unrar");
        let search_path = std::env::join_paths([path_dir]).unwrap();

        let missing_override = root.join("missing-override");
        let configured = detect_unrar_backend(
            Some(missing_override.as_os_str()),
            Some(search_path.as_os_str()),
        );
        assert!(!configured.available());
        assert!(configured.configured());
        assert_eq!(configured.source(), Some(UnrarBackendSource::Environment));
        assert_eq!(configured.selected(), Some(missing_override.as_path()));
        assert_eq!(configured.executable(), None);

        let path = detect_unrar_backend(None, Some(search_path.as_os_str()));
        assert!(path.available());
        assert!(!path.configured());
        assert_eq!(path.source(), Some(UnrarBackendSource::Path));
        assert_eq!(path.selected(), Some(path_tool.as_path()));
        assert_eq!(path.executable(), Some(path_tool.as_path()));

        let missing = detect_unrar_backend(None, None);
        assert!(!missing.available());
        assert!(!missing.configured());
        assert_eq!(missing.source(), None);
        assert_eq!(missing.selected(), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rar_open_reports_missing_external_tool() {
        let _guard = env_lock();
        let _restore_tool = EnvRestore::capture("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_BSDTAR", "/definitely/missing/squallz-bsdtar");

        let path = temp_path("missing", "rar");
        fs::write(&path, RAR5_MAGIC).unwrap();
        let err = match RarFormat.open(
            Box::new(File::open(&path).unwrap()),
            &OpenOptions::default(),
        ) {
            Ok(_) => panic!("RAR open should fail when SQUALLZ_BSDTAR points to a missing tool"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::DependencyMissing(_)), "{err:?}");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rar_create_is_unsupported() {
        let path = temp_path("create", "rar");
        let err = match RarFormat.create(
            Box::new(File::create(&path).unwrap()),
            &CreateOptions::default(),
        ) {
            Ok(_) => panic!("RAR creation should be unsupported"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::Unsupported(_)), "{err:?}");
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_rejects_empty_listing_from_nonempty_rar() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore::capture("SQUALLZ_BSDTAR");

        let script = temp_path("empty-bsdtar", "sh");
        let archive = temp_path("empty-listing", "rar");
        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "-tf" ] || [ "$1" = "-tvf" ]; then
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        fs::write(&archive, RAR5_MAGIC).unwrap();
        std::env::set_var("SQUALLZ_BSDTAR", &script);

        let err = match RarFormat.open(
            Box::new(File::open(&archive).unwrap()),
            &OpenOptions::default(),
        ) {
            Ok(_) => panic!("non-empty RAR with empty bridge listing must not open as healthy"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_prefers_7z_for_listing_testing_and_entry_streams() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_7z = EnvRestore::capture("SQUALLZ_7Z");
        let _restore_bsdtar = EnvRestore::capture("SQUALLZ_BSDTAR");
        let _restore_log = EnvRestore::capture("SQUALLZ_FAKE_7Z_LOG");

        let script = temp_path("fake-7z", "sh");
        let log = temp_path("fake-7z", "log");
        let archive = temp_path("fake-7z-archive", "rar");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = docs
Folder = +
Size = 0
Attributes = D

Path = hello.txt
Folder = -
Size = 21
Packed Size = 12
CRC = 1234ABCD
Encrypted = -

Path = -dash.txt
Folder = -
Size = 18
Packed Size = 9
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from rar via 7z' ;;
    -dash.txt) printf 'dash entry content' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
        fs::write(&script, script_body).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        let _ = fs::remove_file(&log);
        fs::write(&archive, RAR5_MAGIC).unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::remove_var("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let mut reader = RarFormat
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 21);
        assert_eq!(entries[1].crc32, Some(0x1234ABCD));
        assert_eq!(entries[2].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path, &mut |entry| {
                entry.read_to_string(&mut hello)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(hello, "hello from rar via 7z");

        let mut dash = String::new();
        reader
            .read_entry(&entries[2].path, &mut |entry| {
                entry.read_to_string(&mut dash)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(dash, "dash entry content");

        let report = reader
            .test_summary(
                &squallz_format_api::SafetyLimits::default(),
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        for limits in [
            SafetyLimits {
                max_output_bytes: 1,
                ..SafetyLimits::default()
            },
            SafetyLimits {
                max_entries: 1,
                ..SafetyLimits::default()
            },
        ] {
            let result = reader.test_summary(
                &limits,
                &squallz_format_api::NoProgress,
                &ControlToken::default(),
            );
            assert!(
                matches!(result, Err(FormatError::ResourceLimitExceeded(_))),
                "{result:?}"
            );
        }

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("l -slt"), "{log}");
        assert!(log.contains("x -so"), "{log}");
        assert!(log.contains("-- -dash.txt"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn rar_native_multivolume_uses_private_first_volume_from_any_member() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_7z = EnvRestore::capture("SQUALLZ_7Z");
        let _restore_bsdtar = EnvRestore::capture("SQUALLZ_BSDTAR");
        let _restore_log = EnvRestore::capture("SQUALLZ_FAKE_7Z_LOG");

        let source_dir = temp_path("native-volume-source", "dir");
        let _ = fs::remove_dir_all(&source_dir);
        fs::create_dir_all(&source_dir).unwrap();
        let first = source_dir.join("sample.part001.rar");
        let second = source_dir.join("sample.part002.rar");
        fs::write(&first, volume::test_rar5_volume(0, true)).unwrap();
        fs::write(&second, volume::test_rar5_volume(1, false)).unwrap();

        let script = temp_path("native-volume-7z", "sh");
        let log = temp_path("native-volume-7z", "log");
        fs::write(
            &script,
            r#"#!/bin/sh
set -eu
case "$*" in
  *native-volume-password*) exit 8 ;;
esac
IFS= read -r password
test "$password" = "native-volume-password"
archive="$3"
if [ "$1" = "l" ]; then
  test "$3" = "-slmu"
  archive="$4"
fi
printf '%s\n' "$archive" >> "$SQUALLZ_FAKE_7Z_LOG"
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.part001.rar"
test -f "$stage/archive.part001.rar"
test -f "$stage/archive.part002.rar"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = hello.txt
Folder = -
Size = 20
Packed Size = 12
CRC = 1234ABCD
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  printf 'native volume entry'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let _ = fs::remove_file(&log);
        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::remove_var("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let options = OpenOptions {
            password: Some(Password::new("native-volume-password")),
            ..OpenOptions::default()
        };
        let mut reader = RarFormat
            .open_file(
                &second,
                Some(volume::test_physical_identity(&second).unwrap()),
                Box::new(File::open(&second).unwrap()),
                &options,
            )
            .unwrap();
        let source_set = reader.source_set().unwrap();
        assert_eq!(source_set.primary(), first);
        assert_eq!(source_set.members(), &[first.clone(), second.clone()]);
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "hello.txt");

        let first_log = fs::read_to_string(&log).unwrap();
        let staged_path = PathBuf::from(first_log.lines().next().unwrap());
        let staged_root = staged_path.parent().unwrap().to_path_buf();
        assert_eq!(
            staged_path.file_name(),
            Some(std::ffi::OsStr::new("archive.part001.rar"))
        );
        assert!(!first_log.contains(&source_dir.to_string_lossy().into_owned()));

        let mut contents = String::new();
        reader
            .read_entry(&entries[0].path, &mut |entry| {
                entry.read_to_string(&mut contents)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(contents, "native volume entry");
        let complete_log = fs::read_to_string(&log).unwrap();
        assert_eq!(complete_log.lines().count(), 2);
        assert!(complete_log
            .lines()
            .all(|line| line == staged_path.to_string_lossy()));
        assert!(!complete_log.contains("native-volume-password"));

        drop(reader);
        assert!(!staged_root.exists());
        fs::remove_file(script).unwrap();
        fs::remove_file(log).unwrap();
        fs::remove_dir_all(source_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rar_header_encrypted_multivolume_is_verified_and_opened_from_any_member() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_7z = EnvRestore::capture("SQUALLZ_7Z");
        let _restore_bsdtar = EnvRestore::capture("SQUALLZ_BSDTAR");

        let source_dir = temp_path("header-encrypted-volume-source", "dir");
        let _ = fs::remove_dir_all(&source_dir);
        fs::create_dir_all(&source_dir).unwrap();
        let first = source_dir.join("secret.part001.rar");
        let second = source_dir.join("secret.part002.rar");
        let encrypted_header = volume::test_rar5_encrypted_header();
        fs::write(&first, &encrypted_header).unwrap();
        fs::write(&second, &encrypted_header).unwrap();

        let identity = volume::test_physical_identity(&second).unwrap();
        let mut selected = File::open(&second).unwrap();
        let probed = RarFormat
            .probe_file_source_set(&second, Some(identity), &mut selected)
            .unwrap()
            .unwrap();
        assert_eq!(probed.primary(), first);
        assert_eq!(probed.members(), &[first.clone(), second.clone()]);

        let script = temp_path("header-encrypted-volume-7z", "sh");
        fs::write(
            &script,
            r#"#!/bin/sh
set -eu
case "$*" in
  *header-encrypted-volume-password*) exit 8 ;;
esac
IFS= read -r password
test "$password" = "header-encrypted-volume-password"
archive="$3"
if [ "$1" = "l" ]; then
  test "$3" = "-slmu"
  archive="$4"
fi
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.part001.rar"
test -f "$stage/archive.part001.rar"
test -f "$stage/archive.part002.rar"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = /private/stage/archive.part001.rar
Type = Rar5
Physical Size = 1024
Total Physical Size = 2048
Encrypted = +
Multivolume = +
Volume Index = 0
Volumes = 2

----------
Path = private.txt
Folder = -
Size = 23
Packed Size = 16
Encrypted = +

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  printf 'header encrypted entry'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::remove_var("SQUALLZ_BSDTAR");

        let options = OpenOptions {
            password: Some(Password::new("header-encrypted-volume-password")),
            ..OpenOptions::default()
        };
        let mut reader = RarFormat
            .open_file(
                &second,
                Some(volume::test_physical_identity(&second).unwrap()),
                Box::new(File::open(&second).unwrap()),
                &options,
            )
            .unwrap();
        let source_set = reader.source_set().unwrap();
        assert_eq!(source_set.primary(), first);
        assert_eq!(source_set.members(), &[first.clone(), second.clone()]);
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "private.txt");
        assert!(entries[0].encrypted);

        let mut contents = String::new();
        reader
            .read_entry(&entries[0].path, &mut |entry| {
                entry.read_to_string(&mut contents)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(contents, "header encrypted entry");
        drop(reader);

        let third = source_dir.join("secret.part003.rar");
        fs::write(&third, encrypted_header).unwrap();
        let error = match RarFormat.open_file(
            &second,
            Some(volume::test_physical_identity(&second).unwrap()),
            Box::new(File::open(&second).unwrap()),
            &options,
        ) {
            Ok(_) => panic!("mismatched encrypted RAR volume count was accepted"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            FormatError::CorruptArchive(detail)
                if detail.contains("expected 3, 7-Zip reported 2")
        ));

        fs::remove_file(script).unwrap();
        fs::remove_dir_all(source_dir).unwrap();
    }

    #[test]
    fn bsdtar_fallback_requires_unambiguous_detailed_regular_files() {
        let entry = |raw: Vec<u8>, display: &str, entry_type: EntryType| EntryMeta {
            path: EntryPath::from_raw(raw, display.to_owned(), "utf-8"),
            entry_type,
            size: 1,
            compressed_size: None,
            modified: None,
            unix_mode: Some(0o100644),
            crc32: None,
            encrypted: false,
        };
        let safe = entry(b"safe.txt".to_vec(), "safe.txt", EntryType::File);
        assert_eq!(
            literal_regular_files(std::slice::from_ref(&safe), true)
                .unwrap()
                .get(b"safe.txt".as_slice()),
            Some(&1)
        );
        let mut missing_detail = safe.clone();
        missing_detail.unix_mode = None;
        assert!(literal_regular_files(&[missing_detail], true).is_none());
        assert!(
            literal_regular_files(&[entry(b"*.txt".to_vec(), "*.txt", EntryType::File)], true)
                .is_none()
        );
        assert!(literal_regular_files(&[entry(vec![0xff], "�", EntryType::File)], true).is_none());
        assert!(literal_regular_files(
            &[entry(b"folder".to_vec(), "folder", EntryType::Dir)],
            true
        )
        .is_none());
        assert!(literal_regular_files(
            &[entry(
                b"link".to_vec(),
                "link",
                EntryType::Symlink {
                    target: b"safe.txt".to_vec(),
                },
            )],
            true,
        )
        .is_none());
        assert!(literal_regular_files(
            &[
                entry(b"same.txt".to_vec(), "same.txt", EntryType::File),
                entry(b"same.txt".to_vec(), "same.txt", EntryType::File),
            ],
            true,
        )
        .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_p7zip_rar5_uses_only_a_matching_bsdtar_stream_backend() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let root = temp_path("legacy-p7zip-fallback", "dir");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("sample.rar");
        let sevenzip = root.join("7z");
        let bsdtar = root.join("bsdtar");
        let wrong_path = root.join("wrong-path-bsdtar");
        let wrong_size = root.join("wrong-size-bsdtar");
        fs::write(&archive, RAR5_MAGIC).unwrap();
        fs::write(
            &sevenzip,
            r#"#!/bin/sh
set -eu
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  printf 'Unsupported Method\n' >&2
  exit 2
fi
exit 3
"#,
        )
        .unwrap();
        fs::write(
            &bsdtar,
            r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'hello.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf '%s\n' '-rw-r--r--  0 0  0  20 Jan  1  2020 hello.txt'
  exit 0
fi
if [ "$1" = "-xOf" ] && [ "$3" = "--" ] && [ "$4" = "hello.txt" ]; then
  printf 'decoded by libarchive'
  exit 0
fi
exit 4
"#,
        )
        .unwrap();
        fs::write(
            &wrong_path,
            r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'different.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf '%s\n' '-rw-r--r--  0 0  0  20 Jan  1  2020 different.txt'
  exit 0
fi
exit 4
"#,
        )
        .unwrap();
        fs::write(
            &wrong_size,
            r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then printf 'hello.txt\n'; exit 0; fi
if [ "$1" = "-tvf" ]; then
  printf '%s\n' '-rw-r--r--  0 0  0  19 Jan  1  2020 hello.txt'
  exit 0
fi
exit 4
"#,
        )
        .unwrap();
        for tool in [&sevenzip, &bsdtar, &wrong_path, &wrong_size] {
            let mut permissions = fs::metadata(tool).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(tool, permissions).unwrap();
        }

        let listing = || {
            let parsed = sevenzip_bridge::listing::read(
                b"p7zip Version 16.02 (locale=C)\nType = Rar5\n\n\
                Path = hello.txt\nSize = 20\nPacked Size = 12\nCRC = 1234ABCD\n\
                Method = m5:17\nEncrypted = -\n"
                    .as_slice(),
            )
            .unwrap();
            sevenzip_bridge::SevenZipListing {
                entries: parsed.entries,
                archive: parsed.archive.unwrap(),
                compatibility: parsed.compatibility,
            }
        };
        let selected = select_single_unencrypted_backend(
            sevenzip.clone(),
            &archive,
            listing(),
            Some(bsdtar.clone()),
            None,
            &ControlToken::default(),
        )
        .unwrap();
        assert!(matches!(&selected.backend, RarBackend::Bsdtar(_)));
        let entry = &selected.listing.as_ref().unwrap().entries[0];
        assert_eq!(entry.path.display, "hello.txt");
        assert_eq!(entry.size, 20);
        assert_eq!(entry.crc32, Some(0x1234ABCD));
        let mut contents = String::new();
        selected
            .backend
            .read_entry(&archive, &entry.path, None, &ControlToken::default())
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "decoded by libarchive");

        for fallback in [&wrong_path, &wrong_size] {
            let selected = select_single_unencrypted_backend(
                sevenzip.clone(),
                &archive,
                listing(),
                Some(fallback.clone()),
                None,
                &ControlToken::default(),
            )
            .unwrap();
            assert!(matches!(&selected.backend, RarBackend::SevenZip(_)));
        }
        let selected = select_single_unencrypted_backend(
            sevenzip,
            &archive,
            listing(),
            Some(wrong_path),
            None,
            &ControlToken::default(),
        )
        .unwrap();
        let entry = &selected.listing.as_ref().unwrap().entries[0];
        let error = selected
            .backend
            .read_entry(&archive, &entry.path, None, &ControlToken::default())
            .unwrap()
            .read_to_end(&mut Vec::new())
            .unwrap_err();
        assert!(error.to_string().contains("7-Zip failed while reading"));

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rar7_v6_multivolume_lists_with_7z_and_streams_with_unrar() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_7z = EnvRestore::capture("SQUALLZ_7Z");
        let _restore_unrar = EnvRestore::capture("SQUALLZ_UNRAR");
        let _restore_bsdtar = EnvRestore::capture("SQUALLZ_BSDTAR");
        let _restore_log = EnvRestore::capture("SQUALLZ_FAKE_RAR7_LOG");

        let source_dir = temp_path("rar7-v6-source", "dir");
        let _ = fs::remove_dir_all(&source_dir);
        fs::create_dir_all(&source_dir).unwrap();
        let first = source_dir.join("sample.part1.rar");
        let second = source_dir.join("sample.part2.rar");
        fs::write(&first, volume::test_rar5_volume(0, true)).unwrap();
        fs::write(&second, volume::test_rar5_volume(1, false)).unwrap();

        let sevenzip = temp_path("rar7-v6-7z", "sh");
        let unrar = temp_path("rar7-v6-unrar", "sh");
        let log = temp_path("rar7-v6", "log");
        fs::write(
            &sevenzip,
            r#"#!/bin/sh
set -eu
printf '7z %s\n' "$*" >> "$SQUALLZ_FAKE_RAR7_LOG"
test "$1" = "l"
test "$2" = "-slt"
test "$3" = "-slmu"
archive="$4"
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.part1.rar"
test -f "$stage/archive.part1.rar"
test -f "$stage/archive.part2.rar"
cat <<'EOF'
Path = /private/stage/archive.part1.rar
Type = Rar5
Physical Size = 2048
Method = v6:128K:m3
Encrypted = -
Multivolume = +
Volume Index = 0
Volumes = 2

----------
Path = -dash.txt
Folder = -
Size = 18
Packed Size = 12
Method = v6:m3:128K
Encrypted = -

EOF
"#,
        )
        .unwrap();
        fs::write(
            &unrar,
            r#"#!/bin/sh
set -eu
printf 'unrar %s\n' "$*" >> "$SQUALLZ_FAKE_RAR7_LOG"
test "$1" = "p"
test "$2" = "-inul"
test "$3" = "-p-"
test "$4" = "-cfg-"
test "$5" = "-@"
archive="$6"
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.part1.rar"
test -f "$stage/archive.part1.rar"
test -f "$stage/archive.part2.rar"
test "$7" = "--"
test "$8" = "-dash.txt"
printf 'rar7 via unrar'
"#,
        )
        .unwrap();
        for script in [&sevenzip, &unrar] {
            let mut permissions = fs::metadata(script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(script, permissions).unwrap();
        }
        let _ = fs::remove_file(&log);
        std::env::set_var("SQUALLZ_7Z", &sevenzip);
        std::env::set_var("SQUALLZ_UNRAR", &unrar);
        std::env::remove_var("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_FAKE_RAR7_LOG", &log);

        let mut reader = RarFormat
            .open_file(
                &second,
                Some(volume::test_physical_identity(&second).unwrap()),
                Box::new(File::open(&second).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let source_set = reader.source_set().unwrap();
        assert_eq!(source_set.primary(), first);
        assert_eq!(source_set.members(), &[first.clone(), second.clone()]);
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "-dash.txt");

        let mut contents = String::new();
        reader
            .read_entry(&entries[0].path, &mut |entry| {
                entry.read_to_string(&mut contents)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(contents, "rar7 via unrar");

        let report = reader
            .test_summary(
                &squallz_format_api::SafetyLimits::default(),
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 1);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let log_text = fs::read_to_string(&log).unwrap();
        assert_eq!(
            log_text
                .lines()
                .filter(|line| line.starts_with("7z "))
                .count(),
            1
        );
        assert_eq!(
            log_text
                .lines()
                .filter(|line| line.starts_with("unrar "))
                .count(),
            2
        );
        assert!(
            log_text.contains("unrar p -inul -p- -cfg- -@"),
            "{log_text}"
        );
        assert!(log_text.contains("-- -dash.txt"), "{log_text}");
        assert!(matches!(
            read_unrar_entry_stdout(
                &unrar,
                &first,
                &EntryPath::from_utf8("wildcard-*.txt"),
                &ControlToken::default(),
            ),
            Err(FormatError::Unsupported(_))
        ));

        let password_backend = RarBackend::select(&first, true, true, &ControlToken::default())
            .expect("7z password backend")
            .backend;
        assert!(matches!(password_backend, RarBackend::SevenZip(_)));

        drop(reader);
        fs::remove_file(sevenzip).unwrap();
        fs::remove_file(unrar).unwrap();
        fs::remove_file(log).unwrap();
        fs::remove_dir_all(source_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bsdtar_verbose_listing_cancellation_is_not_downgraded_to_missing_metadata() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let script = temp_path("cancelled-bsdtar", "sh");
        let archive = temp_path("cancelled-bsdtar-archive", "rar");
        fs::write(&archive, RAR4_MAGIC).unwrap();

        for verbose in ["exec sleep 30", "exec 1>&-; exec sleep 30"] {
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"-tf\" ]; then printf 'file.txt\\n'; exit 0; fi\n{verbose}\n"
                ),
            )
            .unwrap();
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();

            let control = ControlToken::default();
            let cancelling_control = control.clone();
            let canceller = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                cancelling_control.cancel();
            });
            let started = Instant::now();
            let error = bsdtar_listing::list_entries(&script, &archive, &control).unwrap_err();
            canceller.join().unwrap();

            assert!(matches!(error, FormatError::Cancelled));
            assert!(started.elapsed() < Duration::from_secs(5));
        }
        fs::remove_file(script).unwrap();
        fs::remove_file(archive).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bsdtar_entry_stream_cancellation_terminates_the_external_tool() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let script = temp_path("cancelled-entry-bsdtar", "sh");
        let archive = temp_path("cancelled-entry-bsdtar-archive", "rar");
        fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        fs::write(&archive, RAR4_MAGIC).unwrap();

        let control = ControlToken::default();
        let mut reader = read_bsdtar_entry_stdout(
            &script,
            &archive,
            &EntryPath::from_utf8("file.txt"),
            &control,
        )
        .unwrap();
        let cancelling_control = control.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancelling_control.cancel();
        });
        let started = Instant::now();
        let error = reader.read(&mut [0u8; 1]).unwrap_err();
        canceller.join().unwrap();

        assert!(matches!(FormatError::from(error), FormatError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(reader);
        fs::remove_file(script).unwrap();
        fs::remove_file(archive).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_uses_bsdtar_for_listing_testing_and_entry_streams() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore::capture("SQUALLZ_BSDTAR");
        let _restore_log = EnvRestore::capture("SQUALLZ_FAKE_BSDTAR_LOG");

        let script = temp_path("fake-bsdtar", "sh");
        let log = temp_path("fake-bsdtar", "log");
        let archive = temp_path("fake-archive", "rar");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_BSDTAR_LOG"
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\nlink\n-dash.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          21 Jan  1  2020 hello.txt\n'
  printf 'lrwxrwxrwx  0 0      0           0 Jan  1  2020 link -> hello.txt\n'
  printf -- '-rw-r--r--  0 0      0          18 Jan  1  2020 -dash.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from rar bridge' ;;
    -dash.txt) printf 'dash entry content' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
        fs::write(&script, script_body).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        let _ = fs::remove_file(&log);
        fs::write(&archive, RAR5_MAGIC).unwrap();

        std::env::set_var("SQUALLZ_BSDTAR", &script);
        std::env::set_var("SQUALLZ_FAKE_BSDTAR_LOG", &log);

        let mut reader = RarFormat
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 4);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 21);
        assert_eq!(entries[1].unix_mode, Some(0o100644));
        assert_eq!(entries[2].path.display, "link");
        assert!(matches!(
            &entries[2].entry_type,
            EntryType::Symlink { target } if target == b"hello.txt"
        ));
        assert_eq!(entries[3].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path, &mut |entry| {
                entry.read_to_string(&mut hello)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(hello, "hello from rar bridge");

        let mut dash = String::new();
        reader
            .read_entry(&entries[3].path, &mut |entry| {
                entry.read_to_string(&mut dash)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(dash, "dash entry content");

        let report = reader
            .test_summary(
                &squallz_format_api::SafetyLimits::default(),
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let log_contents = fs::read_to_string(&log).unwrap();
        assert!(log_contents.contains("-tf"));
        assert!(log_contents.contains("-xOf"));
        assert!(log_contents.contains("-- -dash.txt"), "{log_contents}");

        for extra_rows in [false, true] {
            let extra = if extra_rows {
                "printf '%s\\n' '-rw-r--r-- 0 0 0 13 Jan 1 2020 missing' 'lrwxrwxrwx 0 0 0 0 Jan 1 2020 link -> target  with   spaces -> suffix'\nprintf '%s' '-rw-r--r-- 0 0 0 999 Jan 1 2020 ignored'"
            } else {
                ""
            };
            fs::write(
                &script,
                format!(
                    r#"#!/bin/sh
if [ "$1" = "-tf" ]; then
  printf '\r\nfolder/\r\ndupe\tname\r\ndupe\tname\r\nbroken\r\nmissing\r\nlast\377'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf '%s\r\n' '-rw-r--r-- 0 0 0 99 Jan 1 2020 empty-name-row' 'drwxr-xr-x 0 0 0 0 Jan 1 2020 folder/' '-rw-r--r-- 0 0 0 41 Jan 1 2020 dupe name' '-rw-r----- 0 0 0 42 Jan 1 2020 dupe name' 'malformed row'
  {extra}
  exit 0
fi
exit 3
"#
                ),
            )
            .unwrap();
            let entries =
                bsdtar_listing::list_entries(&script, &archive, &ControlToken::default()).unwrap();
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.path.raw.as_slice())
                    .collect::<Vec<_>>(),
                [
                    b"folder/".as_slice(),
                    b"dupe\tname".as_slice(),
                    b"dupe\tname".as_slice(),
                    b"broken".as_slice(),
                    b"missing".as_slice(),
                    b"last\xff".as_slice(),
                ]
            );
            assert_eq!(entries[1].path.display, "dupe\tname");
            assert_eq!(entries[5].path.display, "last�");
            assert_eq!(
                entries.iter().map(|entry| entry.size).collect::<Vec<_>>(),
                [0, 41, 42, 0, if extra_rows { 13 } else { 0 }, 0]
            );
            assert!(matches!(entries[0].entry_type, EntryType::Dir));
            assert_eq!(entries[0].unix_mode, Some(0o040755));
            assert_eq!(entries[1].unix_mode, Some(0o100644));
            assert_eq!(entries[2].unix_mode, Some(0o100640));
            assert_eq!(entries[3].unix_mode, None);
            if extra_rows {
                assert_eq!(entries[4].unix_mode, Some(0o100644));
                assert_eq!(entries[5].unix_mode, Some(0o120777));
                assert!(matches!(
                    &entries[5].entry_type,
                    EntryType::Symlink { target } if target == b"target with spaces -> suffix"
                ));
            } else {
                assert_eq!(entries[4].unix_mode, None);
                assert_eq!(entries[5].unix_mode, None);
                assert!(matches!(entries[5].entry_type, EntryType::File));
            }
        }

        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "-tf" ]; then printf 'docs/\nhello.txt\nlink\n-dash.txt\n'; exit 0; fi
if [ "$1" = "-tvf" ]; then
  printf '%s\n' 'drwxr-xr-x 0 0 0 0 Jan 1 2020 docs/' '-rw-r--r-- 0 0 0 21 Jan 1 2020 hello.txt'
  printf 'verbose listing failed after a valid prefix\n' >&2
  exit 2
fi
exit 3
"#,
        )
        .unwrap();
        let entries =
            bsdtar_listing::list_entries(&script, &archive, &ControlToken::default()).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert!(entries
            .iter()
            .skip(1)
            .all(|entry| matches!(entry.entry_type, EntryType::File)));
        assert!(entries
            .iter()
            .all(|entry| entry.size == 0 && entry.unix_mode.is_none()));

        for message in [
            "unsupported password",
            "password required",
            "  names listing failed after a valid prefix  ",
        ] {
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nprintf 'partial.txt\\n'\nprintf '%s\\n' '{message}' >&2\nexit 2\n"
                ),
            )
            .unwrap();
            let error = bsdtar_listing::list_entries(&script, &archive, &ControlToken::default())
                .unwrap_err();
            match message {
                "unsupported password" => {
                    assert!(matches!(error, FormatError::DependencyMissing(_)))
                }
                "password required" => assert!(matches!(error, FormatError::PasswordRequired)),
                _ => assert!(
                    matches!(error, FormatError::CorruptArchive(detail) if detail == message.trim())
                ),
            }
        }

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }
}
