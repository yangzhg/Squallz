//! Cross-platform 7-Zip/7zz read bridge for long-tail unpack-only formats.
//!
//! The bridge lists entries and streams individual files through stdout so
//! extraction still flows through Squallz's shared safe extraction engine.

mod diagnostics;
pub(crate) mod listing;
mod process;
mod wim_volume;
mod wim_writer;

pub(crate) use listing::SevenZipArchiveProperties;
pub(crate) use wim_volume::StagedSplitWimSet;
pub use wim_writer::{wimlib_backend_status, WimlibBackendSource, WimlibBackendStatus};

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use squallz_format_api::{
    split_volume_name, ArchiveFormat, ArchiveReader, ArchiveSourceSet, ArchiveWriter, ControlToken,
    CreateOptions, EntryMeta, EntryPath, FormatCapabilities, FormatCreateBudget, FormatError,
    NativeVolumeBudget, NativeVolumeLimits, NativeVolumeWriter, OpenOptions, Password,
    PhysicalFileIdentity, ProgressSink, ReadSeek, SplitOutputMode, WriteSeek,
};

#[cfg(test)]
use squallz_format_api::EntryType;
#[cfg(all(test, unix))]
use squallz_format_api::SafetyLimits;

use crate::external_reader::{ExternalArchiveReader, ExternalArchiveSource};

use diagnostics::DiagnosticCapture;
use process::SevenZipProcess;

struct SevenZipSpec {
    id: &'static str,
    extensions: &'static [&'static str],
}

pub(crate) struct SevenZipBridgeFormat {
    spec: &'static SevenZipSpec,
}

const SPECS: &[SevenZipSpec] = &[
    SevenZipSpec {
        id: "wim",
        extensions: &["wim", "swm", "esd"],
    },
    SevenZipSpec {
        id: "apfs",
        extensions: &["apfs"],
    },
    SevenZipSpec {
        id: "ar",
        extensions: &["ar", "a", "deb", "lib"],
    },
    SevenZipSpec {
        id: "arj",
        extensions: &["arj"],
    },
    SevenZipSpec {
        id: "cab",
        extensions: &["cab"],
    },
    SevenZipSpec {
        id: "chm",
        extensions: &["chm", "chw", "chi", "chq"],
    },
    SevenZipSpec {
        id: "cpio",
        extensions: &["cpio"],
    },
    SevenZipSpec {
        id: "cramfs",
        extensions: &["cramfs"],
    },
    SevenZipSpec {
        id: "dmg",
        extensions: &["dmg"],
    },
    SevenZipSpec {
        id: "ext",
        extensions: &["ext", "ext2", "ext3", "ext4"],
    },
    SevenZipSpec {
        id: "fat",
        extensions: &["fat"],
    },
    SevenZipSpec {
        id: "gpt",
        extensions: &["gpt"],
    },
    SevenZipSpec {
        id: "hfs",
        extensions: &["hfs", "hfsx"],
    },
    SevenZipSpec {
        id: "ihex",
        extensions: &["ihex", "hex"],
    },
    SevenZipSpec {
        id: "iso",
        extensions: &["iso"],
    },
    SevenZipSpec {
        id: "lzh",
        extensions: &["lzh", "lha"],
    },
    SevenZipSpec {
        id: "lzma",
        extensions: &["lzma"],
    },
    SevenZipSpec {
        id: "mbr",
        extensions: &["mbr"],
    },
    SevenZipSpec {
        id: "msi",
        extensions: &["msi", "msp"],
    },
    SevenZipSpec {
        id: "nsis",
        extensions: &["nsis"],
    },
    SevenZipSpec {
        id: "ntfs",
        extensions: &["ntfs"],
    },
    SevenZipSpec {
        id: "qcow2",
        extensions: &["qcow", "qcow2", "qcow2c"],
    },
    SevenZipSpec {
        id: "rpm",
        extensions: &["rpm"],
    },
    SevenZipSpec {
        id: "squashfs",
        extensions: &["squashfs"],
    },
    SevenZipSpec {
        id: "udf",
        extensions: &["udf"],
    },
    SevenZipSpec {
        id: "uefi",
        extensions: &["scap", "uefif"],
    },
    SevenZipSpec {
        id: "vdi",
        extensions: &["vdi"],
    },
    SevenZipSpec {
        id: "vhd",
        extensions: &["vhd"],
    },
    SevenZipSpec {
        id: "vhdx",
        extensions: &["vhdx"],
    },
    SevenZipSpec {
        id: "vmdk",
        extensions: &["vmdk"],
    },
    SevenZipSpec {
        id: "xar",
        extensions: &["xar", "pkg"],
    },
    SevenZipSpec {
        id: "z",
        extensions: &["z", "taz"],
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SevenZipBackendSource {
    Application,
    Environment,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SevenZipBackendStatus {
    source: Option<SevenZipBackendSource>,
    selected: Option<PathBuf>,
    executable: Option<PathBuf>,
    configured: bool,
}

impl SevenZipBackendStatus {
    pub fn available(&self) -> bool {
        self.executable.is_some()
    }

    pub fn configured(&self) -> bool {
        self.configured
    }

    pub fn source(&self) -> Option<SevenZipBackendSource> {
        self.source
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }
}

pub(crate) fn formats() -> impl Iterator<Item = SevenZipBridgeFormat> {
    SPECS.iter().map(|spec| SevenZipBridgeFormat { spec })
}

impl ArchiveFormat for SevenZipBridgeFormat {
    fn id(&self) -> &'static str {
        self.spec.id
    }

    fn extensions(&self) -> &'static [&'static str] {
        self.spec.extensions
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: self.spec.id == "wim",
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: self.spec.id == "wim",
            can_update: false,
            can_test: true,
        }
    }

    fn validate_create_name(&self, name: &str) -> Result<(), FormatError> {
        let name = split_volume_name(name).map_or(name, |(base, _)| base);
        let split_wim = self.spec.id == "wim"
            && Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("swm"));
        if split_wim {
            return Err(FormatError::split_wim_creation_unsupported());
        }
        Ok(())
    }

    fn validate_create_options(&self, name: &str, opts: &CreateOptions) -> Result<(), FormatError> {
        if self.spec.id != "wim" {
            return self.validate_create_name(name);
        }
        let name = split_volume_name(name).map_or(name, |(base, _)| base);
        let split_wim_name = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("swm"));
        match (split_wim_name, opts.split_size, opts.split_mode) {
            (true, Some(_), SplitOutputMode::Native) => Ok(()),
            (true, _, _) => Err(FormatError::split_wim_creation_unsupported()),
            (false, Some(_), SplitOutputMode::Native) => Err(FormatError::Unsupported(
                "native Split WIM output must use a .swm name".into(),
            )),
            (false, _, _) => Ok(()),
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        match self.spec.id {
            "wim" => head.starts_with(b"MSWIM\0\0\0"),
            "ar" => head.starts_with(b"!<arch>\n"),
            "cab" => head.starts_with(b"MSCF"),
            "rpm" => head.starts_with(&[0xED, 0xAB, 0xEE, 0xDB]),
            "xar" => head.starts_with(b"xar!"),
            _ => false,
        }
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
        mut src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        ctl.checkpoint()?;
        if self.spec.id == "wim" {
            reject_split_wim(&mut *src)?;
        }
        Ok(Box::new(open_tool_archive(
            src,
            self.spec,
            opts.password.clone(),
            ctl,
        )?))
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
        ctl.checkpoint()?;
        if self.spec.id != "wim" {
            return self.open_with_control(src, opts, ctl);
        }
        match wim_volume::bind_file_with_control(source_path, source_identity, src, ctl)? {
            wim_volume::BoundWimSource::Single(src) => Ok(Box::new(open_tool_archive(
                src,
                self.spec,
                opts.password.clone(),
                ctl,
            )?)),
            wim_volume::BoundWimSource::Split(discovered, selected_src) => {
                let tool = sevenzip_tool()?;
                let staged = wim_volume::StagedSplitWimSet::from_discovered_with_control(
                    discovered,
                    selected_src,
                    ctl,
                )?;
                let password = opts.password.clone();
                let raw_entries =
                    list_entries_with_control(&tool, staged.path(), password.as_ref(), ctl)
                        .map_err(|error| staged.remap_external_error(error))?;
                let (entries, _) = normalize_entries(self.spec, raw_entries);
                if entries.is_empty() && fs::metadata(staged.path())?.len() > 0 {
                    return Err(FormatError::CorruptArchive(format!(
                        "7-Zip listed no entries for a non-empty {} archive",
                        self.spec.id
                    )));
                }
                Ok(Box::new(ExternalArchiveReader::new(
                    ExternalArchiveSource::SplitWim { staged, tool },
                    entries,
                    password,
                    ctl,
                )))
            }
        }
    }

    fn probe_file_source_set(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        if self.spec.id == "wim" {
            return wim_volume::probe_bound_file(source_path, source_identity, src);
        }
        Ok(None)
    }

    fn probe_file_source_set_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
        ctl: &ControlToken,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        if self.spec.id == "wim" {
            return wim_volume::probe_bound_file_with_control(
                source_path,
                source_identity,
                src,
                ctl,
            );
        }
        ctl.checkpoint()?;
        Ok(None)
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        if self.spec.id == "wim" {
            return wim_writer::create(dst, opts);
        }
        Err(FormatError::Unsupported(format!(
            "format {} is currently read-only through the 7-Zip bridge",
            self.spec.id
        )))
    }

    fn create_with_control(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        ctl.checkpoint()?;
        if self.spec.id == "wim" {
            return wim_writer::create_with_control(dst, opts, ctl);
        }
        Err(FormatError::Unsupported(format!(
            "format {} is currently read-only through the 7-Zip bridge",
            self.spec.id
        )))
    }

    fn native_volume_limits(&self) -> Option<NativeVolumeLimits> {
        (self.spec.id == "wim").then(wim_writer::native_volume_limits)
    }

    fn native_volume_primary_index(&self, volume_count: u32) -> Result<u32, FormatError> {
        if self.spec.id == "wim" {
            wim_writer::native_volume_primary_index(volume_count)
        } else {
            volume_count
                .checked_sub(1)
                .ok_or_else(|| FormatError::Other("native volume writer produced no output".into()))
        }
    }

    fn native_volume_budget(
        &self,
        archive_bytes: u64,
        entry_count: u64,
        volume_size: u64,
    ) -> Result<NativeVolumeBudget, FormatError> {
        if self.spec.id == "wim" {
            wim_writer::native_volume_budget(archive_bytes, entry_count, volume_size)
        } else {
            Err(FormatError::Unsupported(format!(
                "format {} does not support native volume creation",
                self.id()
            )))
        }
    }

    fn native_volume_path(
        &self,
        destination: &Path,
        disk_index: u32,
        _primary_volume: bool,
    ) -> Result<PathBuf, FormatError> {
        if self.spec.id == "wim" {
            wim_writer::native_volume_path(destination, disk_index)
        } else {
            Err(FormatError::Unsupported(format!(
                "format {} does not support native volume creation",
                self.id()
            )))
        }
    }

    fn write_native_volumes(
        &self,
        source: &mut dyn ReadSeek,
        output: &mut dyn NativeVolumeWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        if self.spec.id == "wim" {
            wim_writer::write_native_volumes(source, output, progress, ctl)
        } else {
            Err(FormatError::Unsupported(format!(
                "format {} does not support native volume creation",
                self.id()
            )))
        }
    }

    fn create_budget(
        &self,
        content_bytes: u64,
        archive_bytes: u64,
        opts: &CreateOptions,
    ) -> Result<FormatCreateBudget, FormatError> {
        if self.spec.id == "wim" {
            return wim_writer::create_budget(content_bytes, archive_bytes, opts);
        }
        Ok(FormatCreateBudget::direct(archive_bytes))
    }
}

fn reject_split_wim(src: &mut dyn ReadSeek) -> Result<(), FormatError> {
    if wim_volume::is_split_wim(src)? {
        return Err(FormatError::split_wim_unsupported());
    }
    Ok(())
}

fn open_tool_archive(
    src: Box<dyn ReadSeek>,
    spec: &'static SevenZipSpec,
    password: Option<Password>,
    ctl: &ControlToken,
) -> Result<ExternalArchiveReader, FormatError> {
    let tool = sevenzip_tool()?;
    let archive = TempArchive::from_reader(src, spec.id)?;
    let raw_entries = list_entries_with_control(&tool, archive.path(), password.as_ref(), ctl)?;
    let (entries, backend_paths) = normalize_entries(spec, raw_entries);
    if entries.is_empty() && archive.len()? > 0 {
        return Err(FormatError::CorruptArchive(format!(
            "7-Zip listed no entries for a non-empty {} archive",
            spec.id
        )));
    }
    Ok(ExternalArchiveReader::new(
        ExternalArchiveSource::ToolArchive {
            archive,
            tool,
            backend_paths,
        },
        entries,
        password,
        ctl,
    ))
}

fn normalize_entries(
    spec: &SevenZipSpec,
    mut entries: Vec<EntryMeta>,
) -> (Vec<EntryMeta>, BTreeMap<String, String>) {
    let mut backend_paths = BTreeMap::new();
    if !matches!(spec.id, "lzma" | "z") || entries.len() != 1 {
        return (entries, backend_paths);
    }

    let Some(entry) = entries.first_mut() else {
        return (entries, backend_paths);
    };
    if !Path::new(&entry.path.display).is_absolute() {
        return (entries, backend_paths);
    }
    let safe_name = "payload".to_owned();
    entry.path = EntryPath::from_utf8(&safe_name);
    backend_paths.insert(safe_name, String::new());
    (entries, backend_paths)
}

pub(crate) fn backend_path_for<'a>(
    backend_paths: &'a BTreeMap<String, String>,
    path: &'a EntryPath,
) -> &'a str {
    match backend_paths.get(&path.display) {
        Some(backend_path) => backend_path.as_str(),
        None => path.display.as_str(),
    }
}

pub fn sevenzip_backend_status() -> SevenZipBackendStatus {
    let configured = std::env::var_os("SQUALLZ_7Z");
    let application_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let search_path = std::env::var_os("PATH");
    detect_sevenzip_backend(
        configured.as_deref(),
        application_dir.as_deref(),
        search_path.as_deref(),
    )
}

fn detect_sevenzip_backend(
    configured: Option<&OsStr>,
    application_dir: Option<&Path>,
    search_path: Option<&OsStr>,
) -> SevenZipBackendStatus {
    if let Some(configured) = configured {
        let selected = PathBuf::from(configured);
        let executable = resolve_command_path(&selected, search_path);
        return SevenZipBackendStatus {
            source: Some(SevenZipBackendSource::Environment),
            selected: Some(selected),
            executable,
            configured: true,
        };
    }

    if let Some(application_dir) = application_dir {
        for candidate in ["7zz", "7z", "7za"] {
            if let Some(executable) = executable_in_dir(application_dir, OsStr::new(candidate)) {
                return SevenZipBackendStatus {
                    source: Some(SevenZipBackendSource::Application),
                    selected: Some(executable.clone()),
                    executable: Some(executable),
                    configured: false,
                };
            }
        }
    }

    for candidate in ["7zz", "7z", "7za"] {
        if let Some(executable) = find_on_path(OsStr::new(candidate), search_path) {
            return SevenZipBackendStatus {
                source: Some(SevenZipBackendSource::Path),
                selected: Some(executable.clone()),
                executable: Some(executable),
                configured: false,
            };
        }
    }

    SevenZipBackendStatus {
        source: None,
        selected: None,
        executable: None,
        configured: false,
    }
}

pub(crate) fn sevenzip_tool_if_configured_or_installed() -> Option<PathBuf> {
    sevenzip_backend_status()
        .executable()
        .map(Path::to_path_buf)
}

fn sevenzip_tool() -> Result<PathBuf, FormatError> {
    sevenzip_tool_if_configured_or_installed()
        .ok_or_else(|| FormatError::DependencyMissing("7zz/7z".into()))
}

pub(crate) fn resolve_command_path(command: &Path, search_path: Option<&OsStr>) -> Option<PathBuf> {
    if command.is_absolute() || command.components().count() > 1 {
        return command_is_executable(command).then(|| command.to_path_buf());
    }
    find_on_path(command.as_os_str(), search_path)
}

pub(crate) fn find_on_path(name: &OsStr, search_path: Option<&OsStr>) -> Option<PathBuf> {
    let search_path = search_path?;
    std::env::split_paths(search_path).find_map(|dir| executable_in_dir(&dir, name))
}

fn executable_in_dir(dir: &Path, name: &OsStr) -> Option<PathBuf> {
    let candidate = dir.join(name);
    if command_is_executable(&candidate) {
        return Some(candidate);
    }
    #[cfg(windows)]
    {
        let mut executable = candidate;
        executable.set_extension("exe");
        if command_is_executable(&executable) {
            return Some(executable);
        }
    }
    None
}

fn command_is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
pub(crate) fn list_entries(
    tool: &Path,
    archive: &Path,
    password: Option<&Password>,
) -> Result<Vec<EntryMeta>, FormatError> {
    list_entries_with_control(tool, archive, password, &ControlToken::default())
}

pub(crate) fn list_entries_with_control(
    tool: &Path,
    archive: &Path,
    password: Option<&Password>,
    ctl: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    Ok(run_7z_listing(tool, archive, password, ctl)?.entries)
}

pub(crate) struct SevenZipListing {
    pub(crate) entries: Vec<EntryMeta>,
    pub(crate) archive: SevenZipArchiveProperties,
    pub(crate) compatibility: listing::RarCompatibility,
}

pub(crate) fn list_entries_with_archive_properties(
    tool: &Path,
    archive: &Path,
    password: Option<&Password>,
    ctl: &ControlToken,
) -> Result<SevenZipListing, FormatError> {
    let listing = run_7z_listing(tool, archive, password, ctl)?;
    Ok(SevenZipListing {
        entries: listing.entries,
        archive: listing.archive?,
        compatibility: listing.compatibility,
    })
}

pub(crate) fn read_entry_stdout(
    tool: &Path,
    archive: &Path,
    path: &EntryPath,
    password: Option<&Password>,
    control: &ControlToken,
) -> Result<Box<dyn Read>, FormatError> {
    spawn_entry_reader(
        tool,
        archive,
        &path.display,
        &path.display,
        password,
        control,
    )
}

pub(crate) fn spawn_entry_reader(
    tool: &Path,
    archive: &Path,
    backend_path: &str,
    display_path: &str,
    password: Option<&Password>,
    control: &ControlToken,
) -> Result<Box<dyn Read>, FormatError> {
    let mut command = Command::new(tool);
    command.arg("x").arg("-so").arg(archive);
    if !backend_path.is_empty() {
        command.arg("--").arg(backend_path);
    }
    Ok(Box::new(CommandStdoutReader {
        process: SevenZipProcess::spawn(command, password, control)?,
        password_supplied: password.is_some(),
        entry: display_path.to_owned(),
        control: control.clone(),
        finished: false,
    }))
}

pub(crate) fn require_password_for_entry(
    entries: &[EntryMeta],
    path: &EntryPath,
    password: Option<&Password>,
) -> Result<(), FormatError> {
    if password.is_none()
        && entries
            .iter()
            .any(|entry| entry.path.raw == path.raw && entry.encrypted)
    {
        Err(FormatError::PasswordRequired)
    } else {
        Ok(())
    }
}

pub(crate) fn recoverable_test_error(error: FormatError) -> Result<FormatError, FormatError> {
    match error {
        FormatError::PasswordRequired
        | FormatError::WrongPassword
        | FormatError::Cancelled
        | FormatError::ResourceLimitExceeded(_) => Err(error),
        error => Ok(error),
    }
}

fn run_7z_listing(
    tool: &Path,
    archive: &Path,
    password: Option<&Password>,
    control: &ControlToken,
) -> Result<listing::ParsedListing, FormatError> {
    let mut request_utc = true;
    loop {
        let mut command = Command::new(tool);
        command.args(["l", "-slt"]);
        if request_utc {
            command.arg("-slmu");
        }
        command.arg(archive);
        control.checkpoint()?;
        let mut stdout = DiagnosticCapture::for_stdout()?;
        let mut process = SevenZipProcess::spawn(command, password, control)?;
        let parsed = listing::read(BufReader::new(DiagnosticReader {
            reader: &mut process.stdout,
            capture: &mut stdout,
            control,
        }));
        if parsed.is_err() {
            process.terminate();
        }
        let exit = process.finish()?;
        let parsed = parsed?;
        if !exit.status.success() {
            let stdout = stdout.finish();
            if request_utc
                && exit.status.code() == Some(7)
                && parsed.entries.is_empty()
                && parsed
                    .archive
                    .as_ref()
                    .is_ok_and(|properties| *properties == SevenZipArchiveProperties::default())
                && exit.diagnostics.utc_switch_unsupported()
                && stdout.password_failure(password.is_some()).is_none()
                && !stdout.has_missing_volume()
            {
                // Retry the original listing contract once for an older tool's
                // explicit switch rejection; unmarked timestamps stay unknown.
                request_utc = false;
                continue;
            }
            return Err(diagnostics::map_output_error(
                &exit.diagnostics,
                &stdout,
                password.is_some(),
            ));
        }
        exit.password_write?;
        return Ok(parsed);
    }
}

struct DiagnosticReader<'a, R> {
    reader: R,
    capture: &'a mut DiagnosticCapture,
    control: &'a ControlToken,
}

impl<R: Read> Read for DiagnosticReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.control.checkpoint().map_err(io::Error::other)?;
        let read = self.reader.read(buffer)?;
        self.control.checkpoint().map_err(io::Error::other)?;
        self.capture.observe(&buffer[..read]);
        Ok(read)
    }
}

struct CommandStdoutReader {
    process: SevenZipProcess,
    password_supplied: bool,
    entry: String,
    control: ControlToken,
    finished: bool,
}

impl CommandStdoutReader {
    fn stop(&mut self) {
        self.process.terminate();
        let _ = self.process.finish();
        self.finished = true;
    }
}

impl Read for CommandStdoutReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished || buf.is_empty() {
            return Ok(0);
        }
        if let Err(error) = self.control.checkpoint() {
            self.stop();
            return Err(io::Error::other(error));
        }
        let n = loop {
            match self.process.stdout.read(buf) {
                Ok(read) => break read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.stop();
                    return Err(if self.control.is_cancelled() {
                        io::Error::other(FormatError::Cancelled)
                    } else {
                        error
                    });
                }
            }
        };
        if n > 0 {
            return Ok(n);
        }
        let exit = self.process.finish();
        self.finished = true;
        let exit = exit.map_err(io::Error::other)?;
        if exit.status.success() {
            exit.password_write?;
            Ok(0)
        } else if let Some(error) = exit.diagnostics.password_failure(self.password_supplied) {
            Err(io::Error::other(error))
        } else {
            Err(io::Error::other(format!(
                "7-Zip failed while reading {}",
                self.entry
            )))
        }
    }
}

pub(crate) struct TempArchive {
    path: PathBuf,
}

impl TempArchive {
    fn from_reader(src: Box<dyn ReadSeek>, tag: &str) -> Result<Self, FormatError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "squallz-7z-{}-{}-{}.{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
            system_time_nanos(SystemTime::now()),
            tag
        ));
        Self::from_reader_at(src, path)
    }

    fn from_reader_at(mut src: Box<dyn ReadSeek>, path: PathBuf) -> Result<Self, FormatError> {
        src.seek(SeekFrom::Start(0))?;
        let mut out = crate::stable_source::create_private_file(&path)?;
        let archive = Self { path };
        let staged = io::copy(&mut src, &mut out).and_then(|_| out.flush());
        drop(out);
        if let Err(error) = staged {
            drop(archive);
            return Err(FormatError::from(error));
        }
        Ok(archive)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn len(&self) -> Result<u64, FormatError> {
        Ok(fs::metadata(&self.path)?.len())
    }
}

fn system_time_nanos(time: SystemTime) -> u128 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Cursor;
    use std::thread;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_path(tag: &str, ext: &str) -> PathBuf {
        std::env::temp_dir().join(format!("squallz-7z-{tag}-{}.{ext}", std::process::id()))
    }

    struct FailingReadSeek {
        source: Cursor<Vec<u8>>,
        reads: usize,
    }

    impl Read for FailingReadSeek {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.reads > 0 {
                return Err(io::Error::other("injected archive staging read failure"));
            }
            self.reads += 1;
            let limit = buffer.len().min(3);
            self.source.read(&mut buffer[..limit])
        }
    }

    impl Seek for FailingReadSeek {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.source.seek(position)
        }
    }

    #[test]
    fn temp_archive_staging_is_private_no_replace_and_failure_safe() {
        let path = temp_path("private-stage", "wim");
        let _ = fs::remove_file(&path);

        let archive = TempArchive::from_reader_at(
            Box::new(Cursor::new(b"private archive bytes".to_vec())),
            path.clone(),
        )
        .unwrap();
        assert_eq!(archive.path(), path);
        assert_eq!(fs::read(&path).unwrap(), b"private archive bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(archive);
        assert!(!path.exists());

        fs::write(&path, b"keep existing").unwrap();
        let collision = TempArchive::from_reader_at(
            Box::new(Cursor::new(b"replacement".to_vec())),
            path.clone(),
        );
        assert!(matches!(
            collision,
            Err(FormatError::Io(ref error)) if error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&path).unwrap(), b"keep existing");
        fs::remove_file(&path).unwrap();

        let failure = TempArchive::from_reader_at(
            Box::new(FailingReadSeek {
                source: Cursor::new(b"partial archive".to_vec()),
                reads: 0,
            }),
            path.clone(),
        );
        assert!(matches!(failure, Err(FormatError::Io(_))));
        assert!(!path.exists());
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

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn sevenzip_backend_status_distinguishes_configuration_application_and_path() {
        let root = temp_path("backend-status", "dir");
        let _ = fs::remove_dir_all(&root);
        let application_dir = root.join("application");
        let path_dir = root.join("path");
        let application_tool = write_test_executable(&application_dir, "7zz");
        let path_tool = write_test_executable(&path_dir, "7zz");
        let search_path = std::env::join_paths([path_dir]).unwrap();

        let missing_override = root.join("missing-override");
        let configured = detect_sevenzip_backend(
            Some(missing_override.as_os_str()),
            Some(&application_dir),
            Some(search_path.as_os_str()),
        );
        assert!(!configured.available());
        assert!(configured.configured());
        assert_eq!(
            configured.source(),
            Some(SevenZipBackendSource::Environment)
        );
        assert_eq!(configured.selected(), Some(missing_override.as_path()));
        assert_eq!(configured.executable(), None);

        let application =
            detect_sevenzip_backend(None, Some(&application_dir), Some(search_path.as_os_str()));
        assert!(application.available());
        assert!(!application.configured());
        assert_eq!(
            application.source(),
            Some(SevenZipBackendSource::Application)
        );
        assert_eq!(application.executable(), Some(application_tool.as_path()));

        let path = detect_sevenzip_backend(None, None, Some(search_path.as_os_str()));
        assert!(path.available());
        assert_eq!(path.source(), Some(SevenZipBackendSource::Path));
        assert_eq!(path.executable(), Some(path_tool.as_path()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sevenzip_bridge_declares_read_only_capabilities() {
        let format = SevenZipBridgeFormat { spec: &SPECS[4] };
        assert_eq!(format.id(), "cab");
        assert_eq!(format.extensions(), ["cab"]);
        let caps = format.capabilities();
        assert!(!caps.can_create);
        assert!(caps.can_extract);
        assert!(caps.can_test);
        let wim = SevenZipBridgeFormat { spec: &SPECS[0] };
        assert!(wim.capabilities().can_create);
        assert!(wim.capabilities().can_split);
        assert!(wim.validate_create_name("image.wim").is_ok());
        assert!(wim.validate_create_name("image.esd").is_ok());
        assert!(wim
            .validate_create_name("image.SWM")
            .unwrap_err()
            .is_split_wim_creation_unsupported());
        assert!(wim
            .validate_create_name("image.swm.001")
            .unwrap_err()
            .is_split_wim_creation_unsupported());
        let native = CreateOptions {
            split_size: Some(100 * 1024 * 1024),
            split_mode: SplitOutputMode::Native,
            ..CreateOptions::default()
        };
        assert!(wim.validate_create_options("image.swm", &native).is_ok());
        assert!(matches!(
            wim.validate_create_options("image.wim", &native),
            Err(FormatError::Unsupported(detail)) if detail.contains(".swm")
        ));
        assert!(wim.sniff(b"MSWIM\0\0\0more", &[]));
        assert!(SevenZipBridgeFormat { spec: &SPECS[2] }.sniff(b"!<arch>\n", &[]));
    }

    #[test]
    fn wim_header_split_detection_uses_flags_and_part_counts() {
        fn header(flags: u32, part_number: u16, total_parts: u16) -> io::Cursor<Vec<u8>> {
            let mut bytes = vec![0u8; 208];
            bytes[..8].copy_from_slice(b"MSWIM\0\0\0");
            bytes[8..12].copy_from_slice(&208u32.to_le_bytes());
            bytes[16..20].copy_from_slice(&flags.to_le_bytes());
            bytes[24..40].copy_from_slice(&[0x42; 16]);
            bytes[40..42].copy_from_slice(&part_number.to_le_bytes());
            bytes[42..44].copy_from_slice(&total_parts.to_le_bytes());
            io::Cursor::new(bytes)
        }

        let mut regular = header(0, 1, 1);
        regular.set_position(7);
        reject_split_wim(&mut regular).unwrap();
        assert_eq!(regular.position(), 7);

        for mut split in [header(0x0000_0008, 1, 1), header(0, 2, 1), header(0, 1, 2)] {
            let error = reject_split_wim(&mut split).unwrap_err();
            assert!(error.is_split_wim_unsupported());
        }
    }

    #[test]
    fn backend_path_falls_back_to_display_path() {
        let path = EntryPath::from_utf8("hello.txt");
        let backend_paths = BTreeMap::new();
        assert_eq!(backend_path_for(&backend_paths, &path), "hello.txt");

        let mut backend_paths = BTreeMap::new();
        backend_paths.insert("hello.txt".to_owned(), "raw/backend/path.txt".to_owned());
        assert_eq!(
            backend_path_for(&backend_paths, &path),
            "raw/backend/path.txt"
        );
    }

    #[test]
    fn sevenzip_stream_listing_normalizes_temp_absolute_path() {
        let raw = vec![EntryMeta {
            path: EntryPath::from_utf8("/tmp/squallz-7z-temp.lzma"),
            entry_type: EntryType::File,
            size: 0,
            compressed_size: Some(32),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }];
        let spec = SPECS.iter().find(|spec| spec.id == "lzma").unwrap();
        let (entries, backend_paths) = normalize_entries(spec, raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "payload");
        assert_eq!(backend_paths.get("payload").map(String::as_str), Some(""));
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_stream_bridge_reads_without_entry_argument() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_7Z",
            old: std::env::var_os("SQUALLZ_7Z"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-stream-7z", "sh");
        let log = temp_path("fake-stream-7z", "log");
        let archive = temp_path("fake-stream", "lzma");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ]; then
  cat <<'EOF'
Path = /tmp/squallz-7z-temp.lzma
Type = lzma
Method = LZMA:23

----------
Size =
Packed Size =
Method = LZMA:23

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  if [ "$#" -ne 3 ]; then
    printf 'stream extraction must not pass an entry path\n' >&2
    exit 9
  fi
  printf 'stream payload'
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
        fs::write(&archive, b"fake lzma").unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let spec = SPECS.iter().find(|spec| spec.id == "lzma").unwrap();
        let mut reader = SevenZipBridgeFormat { spec }
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "payload");

        let mut payload = String::new();
        reader
            .read_entry(&entries[0].path, &mut |entry| {
                entry.read_to_string(&mut payload)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(payload, "stream payload");

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.lines().any(|line| line.starts_with("x -so ")));
        assert!(!log.contains(" -- "), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_bridge_uses_tool_for_listing_testing_and_entry_streams() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_7Z",
            old: std::env::var_os("SQUALLZ_7Z"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-7z", "sh");
        let log = temp_path("fake-7z", "log");
        let archive = temp_path("fake-archive", "cab");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ]; then
  cat <<'EOF'
Path = docs
Folder = +
Size = 0
Attributes = D
Modified = 2023-11-14 22:13:20Z

Path = hello.txt
Folder = -
Size = 28
Packed Size = 12
CRC = 1234ABCD
Encrypted = -
Modified = 2023-11-14 22:13:21.1234567Z

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
    hello.txt) printf 'hello from 7z bridge payload' ;;
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
        fs::write(&archive, b"MSCF fake cab").unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let mut reader = SevenZipBridgeFormat { spec: &SPECS[4] }
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 28);
        assert_eq!(entries[1].compressed_size, Some(12));
        assert_eq!(entries[1].crc32, Some(0x1234_ABCD));
        assert_eq!(entries[0].modified, None);
        assert_eq!(entries[1].modified, None);
        assert_eq!(entries[2].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path, &mut |entry| {
                entry.read_to_string(&mut hello)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(hello, "hello from 7z bridge payload");

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
        assert!(log.contains("l -slt"));
        assert!(log.contains("l -slt -slmu"), "{log}");
        assert!(log.contains("x -so"));
        assert!(log.contains("-- -dash.txt"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_listing_retries_only_an_unsupported_utc_switch() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };
        let _restore_mode = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_TIME_MODE",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_TIME_MODE"),
        };
        let script = temp_path("fake-utc-switch", "sh");
        let log = temp_path("fake-utc-switch", "log");
        let archive = temp_path("fake-utc-switch", "zip");
        fs::write(
            &script,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" != "l" ] || [ "$2" != "-slt" ]; then exit 9; fi
if [ "$3" = "-slmu" ]; then
  if [ "$SQUALLZ_FAKE_7Z_TIME_MODE" = "modern" ]; then
    printf 'Type = zip\nPhysical Size = 1\n\nPath = retained.txt\nSize = 3\nModified = 2023-11-14 22:13:21.1234567Z\n'
    exit 0
  fi
  printf '\nCommand Line Error:\nUnknown switch:\n-slmu\n' >&2
  case "$SQUALLZ_FAKE_7Z_TIME_MODE" in
    password) printf 'Wrong password?\n' ;;
    corrupt) printf 'Data Error\n' >&2 ;;
    missing) printf 'ERROR = Missing volume : missing.cab\n' ;;
    entries) printf 'Path = retained.txt\nSize = 3\n' ;;
    status) exit 2 ;;
  esac
  exit 7
fi
if [ "$SQUALLZ_FAKE_7Z_TIME_MODE" != "legacy" ]; then exit 9; fi
printf 'Type = zip\nPhysical Size = 1\n\nPath = retained.txt\nSize = 3\nModified = 2023-11-15 06:13:20\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&archive, b"fake archive").unwrap();
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        for mode in [
            "modern", "legacy", "password", "corrupt", "missing", "entries", "status",
        ] {
            fs::write(&log, "").unwrap();
            std::env::set_var("SQUALLZ_FAKE_7Z_TIME_MODE", mode);
            let result = list_entries(&script, &archive, None);
            let calls = fs::read_to_string(&log).unwrap();
            let calls = calls.lines().collect::<Vec<_>>();
            assert!(calls[0].starts_with("l -slt -slmu "), "{mode}: {calls:?}");
            if matches!(mode, "modern" | "legacy") {
                let entries = result.unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].path.display, "retained.txt");
                assert_eq!(entries[0].size, 3);
                if mode == "modern" {
                    assert_eq!(
                        entries[0].modified,
                        Some(
                            std::time::UNIX_EPOCH
                                + std::time::Duration::new(1_700_000_001, 123_456_700)
                        )
                    );
                    assert_eq!(calls.len(), 1);
                } else {
                    assert_eq!(entries[0].modified, None);
                    assert_eq!(calls.len(), 2);
                    assert!(calls[1].starts_with("l -slt "));
                    assert!(!calls[1].contains("-slmu"));
                }
            } else {
                let error = result.unwrap_err();
                assert_eq!(calls.len(), 1, "{mode}: {calls:?}");
                if mode == "password" {
                    assert!(matches!(error, FormatError::PasswordRequired), "{error:?}");
                }
            }
        }

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_listing_cancellation_terminates_the_external_tool() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let script = temp_path("cancelled-listing-7z", "sh");
        let archive = temp_path("cancelled-listing-archive", "7z");
        fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        fs::write(&archive, b"fake archive").unwrap();

        let blocked_password = Password::new("x".repeat(1024 * 1024));
        for password in [None, Some(&blocked_password)] {
            let control = ControlToken::default();
            let cancelling_control = control.clone();
            let canceller = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                cancelling_control.cancel();
            });
            let started = Instant::now();
            let error =
                list_entries_with_control(&script, &archive, password, &control).unwrap_err();
            canceller.join().unwrap();

            assert!(matches!(error, FormatError::Cancelled));
            assert!(started.elapsed() < Duration::from_secs(5));
        }
        // Cancellation while waiting for the tool must skip directory inference.
        fs::write(
            &script,
            "#!/bin/sh\ntest \"$3\" = \"-slmu\"\ncat \"$4\"\nprintf ready > \"$4.ready\"\nexec sleep 30\n",
        )
        .unwrap();
        let listing: String = (0..30_000)
            .map(|index| format!("Path = file-{index:05}.txt\nSize = 0\n\n"))
            .collect();
        fs::write(&archive, listing).unwrap();
        let ready = archive.with_extension("7z.ready");
        let control = ControlToken::default();
        let cancelling_control = control.clone();
        let ready_for_cancel = ready.clone();
        let canceller = thread::spawn(move || {
            let waiting = Instant::now();
            while !ready_for_cancel.exists() && waiting.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(10));
            }
            let ready = ready_for_cancel.exists();
            let cancelled_at = Instant::now();
            cancelling_control.cancel();
            (ready, cancelled_at)
        });
        let error = list_entries_with_control(&script, &archive, None, &control).unwrap_err();
        let (output_ready, cancelled_at) = canceller.join().unwrap();
        assert!(output_ready, "tool did not finish writing its listing");
        assert!(matches!(error, FormatError::Cancelled));
        assert!(cancelled_at.elapsed() < Duration::from_secs(2));
        fs::remove_file(ready).unwrap();
        fs::remove_file(script).unwrap();
        fs::remove_file(archive).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_entry_stream_cancellation_terminates_the_external_tool() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let script = temp_path("cancelled-entry-7z", "sh");
        let archive = temp_path("cancelled-entry-archive", "7z");
        fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        fs::write(&archive, b"fake archive").unwrap();

        let blocked_password = Password::new("x".repeat(1024 * 1024));
        for password in [None, Some(&blocked_password)] {
            let control = ControlToken::default();
            let mut reader = spawn_entry_reader(
                &script, &archive, "file.txt", "file.txt", password, &control,
            )
            .unwrap();
            let cancelling_control = control.clone();
            let canceller = thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                cancelling_control.cancel();
            });
            let started = Instant::now();
            let error = reader.read(&mut [0u8; 1]).unwrap_err();
            canceller.join().unwrap();

            assert!(matches!(FormatError::from(error), FormatError::Cancelled));
            assert!(started.elapsed() < Duration::from_secs(5));
            drop(reader);
        }
        let started = Instant::now();
        drop(
            spawn_entry_reader(
                &script,
                &archive,
                "file.txt",
                "file.txt",
                Some(&blocked_password),
                &ControlToken::default(),
            )
            .unwrap(),
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        fs::remove_file(script).unwrap();
        fs::remove_file(archive).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_passwords_use_stdin_and_keep_typed_failures() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-password-7z", "sh");
        let log = temp_path("fake-password-7z", "log");
        let archive = temp_path("fake-password-archive", "7z");
        let script_body = r#"#!/bin/sh
set -eu
case "$*" in
  *bridge-fixture-password*) printf 'password leaked through arguments\n' >&2; exit 8 ;;
esac
if env | grep -F 'bridge-fixture-password' >/dev/null; then
  printf 'password leaked through environment\n' >&2
  exit 8
fi
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
archive="$3"
if [ "$1" = "l" ]; then
  test "$3" = "-slmu"
  archive="$4"
fi
case "$(cat "$archive")" in
  late-password)
    dd if=/dev/zero bs=4096 count=20 2>/dev/null | tr '\000' x >&2
    printf '\nWrong password?\n' >&2
    exit 2 ;;
  late-volume)
    dd if=/dev/zero bs=4096 count=20 2>/dev/null | tr '\000' x
    printf '\nERROR = Missing volume : archive.part3.rar\n'
    printf 'Unsupported format. Wrong password?\n' >&2
    exit 2 ;;
  closed-input-success)
    exec 0<&-
    exit 0 ;;
esac
if ! IFS= read -r password; then
  printf 'Enter password:\n' >&2
  exit 255
fi
if [ "$password" != "bridge-fixture-password" ]; then
  printf 'Wrong password?\n' >&2
  exit 2
fi
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = secret.txt
Folder = -
Size = 14
Packed Size = 9
Encrypted = +

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  printf 'secret payload'
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
        fs::write(&archive, b"fake encrypted archive").unwrap();
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let correct = Password::new("bridge-fixture-password");
        let wrong = Password::new("bridge-fixture-wrong");
        let entries = list_entries(&script, &archive, Some(&correct)).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].encrypted);

        let error = list_entries(&script, &archive, Some(&wrong)).unwrap_err();
        assert!(matches!(error, FormatError::WrongPassword), "{error:?}");
        let error = list_entries(&script, &archive, None).unwrap_err();
        assert!(matches!(error, FormatError::PasswordRequired), "{error:?}");

        let mut payload = String::new();
        read_entry_stdout(
            &script,
            &archive,
            &entries[0].path,
            Some(&correct),
            &ControlToken::default(),
        )
        .unwrap()
        .read_to_string(&mut payload)
        .unwrap();
        assert_eq!(payload, "secret payload");

        let mut wrong_reader = read_entry_stdout(
            &script,
            &archive,
            &entries[0].path,
            Some(&wrong),
            &ControlToken::default(),
        )
        .unwrap();
        let error = wrong_reader.read_to_end(&mut Vec::new()).unwrap_err();
        let error = FormatError::from(error);
        assert!(matches!(error, FormatError::WrongPassword), "{error:?}");

        let blocked_password = Password::new("x".repeat(1024 * 1024));
        fs::write(&archive, b"late-password").unwrap();
        for password in [None, Some(&blocked_password)] {
            let error = list_entries(&script, &archive, password).unwrap_err();
            assert!(matches!(
                (password.is_some(), error),
                (true, FormatError::WrongPassword) | (false, FormatError::PasswordRequired)
            ));
            let mut reader = read_entry_stdout(
                &script,
                &archive,
                &entries[0].path,
                password,
                &ControlToken::default(),
            )
            .unwrap();
            let error = FormatError::from(reader.read_to_end(&mut Vec::new()).unwrap_err());
            assert!(matches!(
                (password.is_some(), error),
                (true, FormatError::WrongPassword) | (false, FormatError::PasswordRequired)
            ));
        }
        fs::write(&archive, b"late-volume").unwrap();
        let error = list_entries(&script, &archive, Some(&blocked_password)).unwrap_err();
        assert!(
            matches!(error, FormatError::CorruptArchive(detail) if detail == "missing volume: archive.part3.rar")
        );
        fs::write(&archive, b"closed-input-success").unwrap();
        assert!(matches!(
            list_entries(&script, &archive, Some(&blocked_password)),
            Err(FormatError::Io(_))
        ));
        let mut reader = read_entry_stdout(
            &script,
            &archive,
            &entries[0].path,
            Some(&blocked_password),
            &ControlToken::default(),
        )
        .unwrap();
        let error = FormatError::from(reader.read_to_end(&mut Vec::new()).unwrap_err());
        assert!(matches!(error, FormatError::Io(_)));

        let log = fs::read_to_string(&log).unwrap();
        assert!(!log.contains("bridge-fixture-password"), "{log}");
        assert!(
            !log.split_whitespace()
                .any(|argument| argument == "-p" || argument.starts_with("-p")),
            "{log}"
        );

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn wim_bridge_creates_through_wimlib_writer() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_wimlib = EnvRestore {
            key: "SQUALLZ_WIMLIB",
            old: std::env::var_os("SQUALLZ_WIMLIB"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_WIMLIB_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_WIMLIB_LOG"),
        };

        let script = temp_path("fake-wimlib", "sh");
        let log = temp_path("fake-wimlib", "log");
        let archive = temp_path("created-wim", "wim");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_WIMLIB_LOG"
if [ "$1" = "capture" ]; then
  src="$2"
  out="$3"
  [ -d "$src/project/sub" ]
  [ "$(cat "$src/project/a.txt")" = "hello wim" ]
  [ "$(cat "$src/project/sub/b.txt")" = "nested wim" ]
  printf 'MSWIM\000\000\000fake-wim' > "$out"
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
        let _ = fs::remove_file(&archive);
        std::env::set_var("SQUALLZ_WIMLIB", &script);
        std::env::set_var("SQUALLZ_FAKE_WIMLIB_LOG", &log);

        let format = SevenZipBridgeFormat { spec: &SPECS[0] };
        let mut writer = format
            .create(
                Box::new(File::create(&archive).unwrap()),
                &CreateOptions::default(),
            )
            .unwrap();
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project"),
                    entry_type: EntryType::Dir,
                    size: 0,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                None,
            )
            .unwrap();
        let mut a = io::Cursor::new(b"hello wim".to_vec());
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project/a.txt"),
                    entry_type: EntryType::File,
                    size: 9,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                Some(&mut a),
            )
            .unwrap();
        let mut b = io::Cursor::new(b"nested wim".to_vec());
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project/sub/b.txt"),
                    entry_type: EntryType::File,
                    size: 10,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                Some(&mut b),
            )
            .unwrap();
        writer.finish().unwrap();

        assert!(fs::read(&archive).unwrap().starts_with(b"MSWIM\0\0\0"));
        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("capture"), "{log}");
        assert!(log.contains("--compress=LZX"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }
}
