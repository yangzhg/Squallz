//! Cross-platform 7-Zip/7zz read bridge for long-tail unpack-only formats.
//!
//! The bridge lists entries and streams individual files through stdout so
//! extraction still flows through Squallz's shared safe extraction engine.

mod wim_volume;
mod wim_writer;

pub use wim_writer::{wimlib_backend_status, WimlibBackendSource, WimlibBackendStatus};

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use squallz_format_api::{
    split_volume_name, ArchiveFormat, ArchiveReader, ArchiveSourceSet, ArchiveWriter,
    BoundedProblemLog, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType,
    FormatCapabilities, FormatCreateBudget, FormatError, NativeVolumeBudget, NativeVolumeLimits,
    NativeVolumeWriter, OpenOptions, Password, PhysicalFileIdentity, ProgressSink, ReadSeek,
    SplitOutputMode, TestReport, TestSummary, WriteSeek, TEST_PROBLEM_PREVIEW_LIMIT,
};

use crate::external_process::{self, ControlledChild};

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
        Ok(Box::new(SevenZipArchiveReader::open(
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
            wim_volume::BoundWimSource::Single(src) => Ok(Box::new(SevenZipArchiveReader::open(
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
                Ok(Box::new(SevenZipArchiveReader::open_split_wim(
                    staged,
                    tool,
                    self.spec,
                    opts.password.clone(),
                    ctl,
                )?))
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

struct SevenZipArchiveReader {
    source: SevenZipArchiveSource,
    tool: PathBuf,
    entries: Vec<EntryMeta>,
    backend_paths: BTreeMap<String, String>,
    password: Option<Password>,
    control: ControlToken,
}

impl SevenZipArchiveReader {
    fn open(
        src: Box<dyn ReadSeek>,
        spec: &'static SevenZipSpec,
        password: Option<Password>,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        let tool = sevenzip_tool()?;
        let temp = TempArchive::from_reader(src, spec.id)?;
        Self::open_source(
            SevenZipArchiveSource::Single(temp),
            tool,
            spec,
            password,
            ctl,
        )
    }

    fn open_split_wim(
        staged: wim_volume::StagedSplitWimSet,
        tool: PathBuf,
        spec: &'static SevenZipSpec,
        password: Option<Password>,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        Self::open_source(
            SevenZipArchiveSource::SplitWim(staged),
            tool,
            spec,
            password,
            ctl,
        )
    }

    fn open_source(
        source: SevenZipArchiveSource,
        tool: PathBuf,
        spec: &'static SevenZipSpec,
        password: Option<Password>,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        let raw_entries = list_entries_with_control(&tool, source.path(), password.as_ref(), ctl)
            .map_err(|error| source.remap_external_error(error))?;
        let (entries, backend_paths) = normalize_entries(spec, raw_entries);
        if entries.is_empty() && source.len()? > 0 {
            return Err(FormatError::CorruptArchive(format!(
                "7-Zip listed no entries for a non-empty {} archive",
                spec.id
            )));
        }
        Ok(Self {
            source,
            tool,
            entries,
            backend_paths,
            password,
            control: ctl.clone(),
        })
    }

    fn read_entry_with_control(
        &self,
        path: &EntryPath,
        control: &ControlToken,
    ) -> Result<Box<dyn Read>, FormatError> {
        require_password_for_entry(&self.entries, path, self.password.as_ref())?;
        let backend_path = backend_path_for(&self.backend_paths, path);
        spawn_entry_reader(
            &self.tool,
            self.source.path(),
            backend_path,
            &path.display,
            self.password.as_ref(),
            control,
        )
    }

    fn test_with_problem_recorder(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &squallz_format_api::ControlToken,
        mut record_problem: impl FnMut(String),
    ) -> Result<u64, FormatError> {
        let entries = self.entries.clone();
        let total = entries.len() as u64;
        let mut entries_tested = 0u64;
        for meta in entries {
            ctl.checkpoint()?;
            if !matches!(meta.entry_type, EntryType::File) {
                continue;
            }
            match self.read_entry_with_control(&meta.path, ctl) {
                Ok(mut data) => {
                    let mut sink = io::sink();
                    if let Err(e) = io::copy(&mut data, &mut sink) {
                        let e = recoverable_stream_error(e)?;
                        record_problem(format!("{}: {e}", meta.path.display));
                    }
                }
                Err(e) => {
                    let e = recoverable_test_error(e)?;
                    record_problem(format!("{}: {e}", meta.path.display));
                }
            }
            entries_tested += 1;
            progress.on_progress(entries_tested, total, &meta.path);
        }
        progress.on_progress(entries_tested, entries_tested, &EntryPath::from_utf8(""));
        Ok(entries_tested)
    }
}

impl ArchiveReader for SevenZipArchiveReader {
    fn source_set(&self) -> Option<&ArchiveSourceSet> {
        self.source.source_set()
    }

    fn verify_source_set(&self, ctl: &squallz_format_api::ControlToken) -> Result<(), FormatError> {
        self.source.verify_source_set(ctl)
    }

    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.entries.clone().into_iter().map(Ok))
    }

    fn consume_entries(
        mut self: Box<Self>,
        visitor: &mut dyn FnMut(EntryMeta) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        for entry in std::mem::take(&mut self.entries) {
            visitor(entry)?;
        }
        Ok(())
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        self.read_entry_with_control(path, &self.control)
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &squallz_format_api::ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut problems = Vec::new();
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.push(problem))?;
        Ok(TestReport {
            entries_tested,
            problems,
            recovery: None,
        })
    }

    fn test_summary(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &squallz_format_api::ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.record(problem))?;
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery: None,
        })
    }
}

enum SevenZipArchiveSource {
    Single(TempArchive),
    SplitWim(wim_volume::StagedSplitWimSet),
}

impl SevenZipArchiveSource {
    fn path(&self) -> &Path {
        match self {
            Self::Single(temp) => temp.path(),
            Self::SplitWim(staged) => staged.path(),
        }
    }

    fn len(&self) -> Result<u64, FormatError> {
        match self {
            Self::Single(temp) => temp.len(),
            Self::SplitWim(staged) => Ok(fs::metadata(staged.path())?.len()),
        }
    }

    fn source_set(&self) -> Option<&ArchiveSourceSet> {
        match self {
            Self::Single(_) => None,
            Self::SplitWim(staged) => Some(staged.source_set()),
        }
    }

    fn verify_source_set(
        &self,
        control: &squallz_format_api::ControlToken,
    ) -> Result<(), FormatError> {
        match self {
            Self::Single(_) => control.checkpoint(),
            Self::SplitWim(staged) => staged.verify_source_set(control),
        }
    }

    fn remap_external_error(&self, error: FormatError) -> FormatError {
        match self {
            Self::Single(_) => error,
            Self::SplitWim(staged) => staged.remap_external_error(error),
        }
    }
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

fn backend_path_for<'a>(
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
    let output = run_7z_output(tool, archive, &["l", "-slt"], password, ctl)?;
    parse_7z_list(&output.stdout)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SevenZipArchiveProperties {
    pub(crate) multivolume: Option<bool>,
    pub(crate) volume_index: Option<u64>,
    pub(crate) volume_count: Option<u64>,
}

pub(crate) struct SevenZipListing {
    pub(crate) entries: Vec<EntryMeta>,
    pub(crate) archive: SevenZipArchiveProperties,
    pub(crate) stdout: Vec<u8>,
}

pub(crate) fn list_entries_with_archive_properties(
    tool: &Path,
    archive: &Path,
    password: Option<&Password>,
    ctl: &ControlToken,
) -> Result<SevenZipListing, FormatError> {
    let output = run_7z_output(tool, archive, &["l", "-slt"], password, ctl)?;
    let entries = parse_7z_list(&output.stdout)?;
    let archive = parse_7z_archive_properties(&output.stdout)?;
    Ok(SevenZipListing {
        entries,
        archive,
        stdout: output.stdout,
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

fn spawn_entry_reader(
    tool: &Path,
    archive: &Path,
    backend_path: &str,
    display_path: &str,
    password: Option<&Password>,
    control: &ControlToken,
) -> Result<Box<dyn Read>, FormatError> {
    let stdin = password_stdio(password)?;
    let mut command = Command::new(tool);
    command.arg("x").arg("-so").arg(archive);
    if !backend_path.is_empty() {
        command.arg("--").arg(backend_path);
    }
    let mut child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_tool_spawn_error)?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_child(&mut child);
            return Err(FormatError::Other(
                "7-Zip did not provide an output stream".into(),
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_child(&mut child);
            return Err(FormatError::Other(
                "7-Zip did not provide a diagnostic stream".into(),
            ));
        }
    };
    let diagnostics = thread::spawn(move || capture_diagnostics(stderr));
    if let Err(error) = write_password(&mut child, password) {
        terminate_child(&mut child);
        let _ = diagnostics.join();
        return Err(error);
    }
    Ok(Box::new(CommandStdoutReader {
        child: ControlledChild::new(child, control),
        stdout,
        diagnostics: Some(diagnostics),
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

fn password_stdio(password: Option<&Password>) -> Result<Stdio, FormatError> {
    if password.is_some_and(|password| {
        password
            .expose()
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, b'\r' | b'\n'))
    }) {
        return Err(FormatError::Unsupported(
            "7-Zip bridge passwords cannot contain line breaks".into(),
        ));
    }
    Ok(if password.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
}

fn write_password(child: &mut Child, password: Option<&Password>) -> Result<(), FormatError> {
    let Some(password) = password else {
        return Ok(());
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| FormatError::Other("7-Zip did not provide a credential stream".into()))?;
    stdin
        .write_all(password.expose().as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .map_err(FormatError::from)
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

const MAX_EXTERNAL_DIAGNOSTIC_BYTES: usize = 64 * 1024;

fn capture_diagnostics(mut stderr: ChildStderr) -> io::Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stderr.read(&mut buffer)?;
        if read == 0 {
            return Ok(captured);
        }
        let remaining = MAX_EXTERNAL_DIAGNOSTIC_BYTES.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
    }
}

pub(crate) fn recoverable_stream_error(error: io::Error) -> Result<io::Error, FormatError> {
    match FormatError::from(error) {
        FormatError::Io(error) => Ok(error),
        error => Err(error),
    }
}

pub(crate) fn recoverable_test_error(error: FormatError) -> Result<FormatError, FormatError> {
    match error {
        FormatError::PasswordRequired | FormatError::WrongPassword => Err(error),
        error => Ok(error),
    }
}

fn parse_7z_list(stdout: &[u8]) -> Result<Vec<EntryMeta>, FormatError> {
    let text = String::from_utf8_lossy(stdout);
    let mut entries = Vec::new();
    let mut block = BTreeMap::<String, String>::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            push_list_block(&mut entries, &mut block);
            continue;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            block.insert(key.trim().to_owned(), value.to_owned());
        }
    }
    infer_directory_entries(&mut entries);
    Ok(entries)
}

fn parse_7z_archive_properties(stdout: &[u8]) -> Result<SevenZipArchiveProperties, FormatError> {
    let text = String::from_utf8_lossy(stdout);
    let mut properties = None;
    let mut block = BTreeMap::<String, String>::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if block.contains_key("Type") && block.contains_key("Physical Size") {
                if properties.is_some() {
                    return Err(FormatError::CorruptArchive(
                        "7-Zip reported more than one archive metadata block".into(),
                    ));
                }
                properties = Some(SevenZipArchiveProperties {
                    multivolume: parse_7z_flag(&block, "Multivolume")?,
                    volume_index: parse_7z_u64(&block, "Volume Index")?,
                    volume_count: parse_7z_u64(&block, "Volumes")?,
                });
            }
            block.clear();
            continue;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            block.insert(key.trim().to_owned(), value.to_owned());
        }
    }
    Ok(properties.unwrap_or_default())
}

fn parse_7z_flag(block: &BTreeMap<String, String>, key: &str) -> Result<Option<bool>, FormatError> {
    match block.get(key).map(|value| value.trim()) {
        Some("+") => Ok(Some(true)),
        Some("-") => Ok(Some(false)),
        Some(_) => Err(FormatError::CorruptArchive(format!(
            "7-Zip reported an invalid {key} value"
        ))),
        None => Ok(None),
    }
}

fn parse_7z_u64(block: &BTreeMap<String, String>, key: &str) -> Result<Option<u64>, FormatError> {
    block
        .get(key)
        .map(|value| {
            value.trim().parse::<u64>().map_err(|_| {
                FormatError::CorruptArchive(format!("7-Zip reported an invalid {key} value"))
            })
        })
        .transpose()
}

fn infer_directory_entries(entries: &mut [EntryMeta]) {
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    for entry in entries {
        if matches!(entry.entry_type, EntryType::Dir) {
            continue;
        }
        let prefix = format!("{}/", entry.path.display.trim_end_matches('/'));
        if paths.iter().any(|path| path.starts_with(&prefix)) {
            entry.entry_type = EntryType::Dir;
            entry.size = 0;
            entry.compressed_size = None;
        }
    }
}

fn push_list_block(entries: &mut Vec<EntryMeta>, block: &mut BTreeMap<String, String>) {
    let Some(path) = block.get("Path").cloned() else {
        block.clear();
        return;
    };
    if block.contains_key("Type") && block.contains_key("Physical Size") {
        block.clear();
        return;
    }
    let is_entry = block.contains_key("Folder")
        || block.contains_key("Size")
        || block.contains_key("Packed Size")
        || block.contains_key("Attributes")
        || block.contains_key("CRC")
        || block.contains_key("Encrypted")
        || block.contains_key("Type");
    if !is_entry || path.is_empty() || path == "." || path == "./" {
        block.clear();
        return;
    }

    let attrs = block_text(block, "Attributes");
    let folder = block.get("Folder").is_some_and(|value| value.trim() == "+")
        || attrs.bytes().any(|b| b == b'D')
        || block
            .get("Type")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("directory"));
    let entry_type = if folder {
        EntryType::Dir
    } else {
        EntryType::File
    };
    let size = list_entry_size(block, folder);
    let compressed_size = block
        .get("Packed Size")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let crc32 = block
        .get("CRC")
        .and_then(|value| u32::from_str_radix(value.trim(), 16).ok());
    let encrypted = block
        .get("Encrypted")
        .is_some_and(|value| value.trim() == "+");
    entries.push(EntryMeta {
        path: EntryPath::from_utf8(&path),
        entry_type,
        size,
        compressed_size,
        modified: None,
        unix_mode: None,
        crc32,
        encrypted,
    });
    block.clear();
}

fn block_text<'a>(block: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    match block.get(key) {
        Some(value) => value.as_str(),
        None => "",
    }
}

fn list_entry_size(block: &BTreeMap<String, String>, folder: bool) -> u64 {
    if folder {
        return 0;
    }
    if let Some(value) = block.get("Size") {
        if let Ok(size) = value.trim().parse::<u64>() {
            return size;
        }
    }
    0
}

fn run_7z_output(
    tool: &Path,
    archive: &Path,
    args: &[&str],
    password: Option<&Password>,
    ctl: &ControlToken,
) -> Result<std::process::Output, FormatError> {
    ctl.checkpoint()?;
    let stdin = password_stdio(password)?;
    let mut child = Command::new(tool)
        .args(args)
        .arg(archive)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_tool_spawn_error)?;
    let password_write = write_password(&mut child, password);
    let output = external_process::wait_with_output(child, ctl, "7-Zip")?;
    if !output.status.success() {
        return Err(map_tool_failure(
            &output.stderr,
            &output.stdout,
            password.is_some(),
        ));
    }
    password_write?;
    Ok(output)
}

fn map_tool_spawn_error(e: io::Error) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("7zz/7z external format bridge".into())
    } else {
        FormatError::from(e)
    }
}

fn map_tool_failure(stderr: &[u8], stdout: &[u8], password_supplied: bool) -> FormatError {
    if let Some(name) = missing_volume_name(stdout) {
        return FormatError::missing_volume(name);
    }
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    if lower.contains("unsupported") || lower.contains("not implemented") {
        FormatError::DependencyMissing("7zz/7z external format bridge".into())
    } else if let Some(error) = password_failure(stderr, stdout, password_supplied) {
        error
    } else {
        FormatError::CorruptArchive(if detail.is_empty() {
            "7-Zip could not read archive".into()
        } else {
            detail
        })
    }
}

fn password_failure(stderr: &[u8], stdout: &[u8], password_supplied: bool) -> Option<FormatError> {
    const DIAGNOSTICS: &[&str] = &[
        "wrong password",
        "incorrect password",
        "password is incorrect",
        "enter password",
        "password required",
        "password is required",
        "requires a password",
        "no password",
        "password was not supplied",
        "password was not provided",
        "password is not defined",
    ];
    let is_password_failure = [stderr, stdout].iter().any(|output| {
        let output = String::from_utf8_lossy(output).to_ascii_lowercase();
        DIAGNOSTICS
            .iter()
            .any(|diagnostic| output.contains(diagnostic))
    });
    is_password_failure.then_some(if password_supplied {
        FormatError::WrongPassword
    } else {
        FormatError::PasswordRequired
    })
}

fn missing_volume_name(stdout: &[u8]) -> Option<&str> {
    const PREFIX: &str = "ERROR = Missing volume : ";
    let text = std::str::from_utf8(stdout).ok()?;
    text.lines().find_map(|line| {
        let name = line.trim().strip_prefix(PREFIX)?.trim();
        if safe_external_file_name(name) {
            Some(name)
        } else {
            None
        }
    })
}

fn safe_external_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && Path::new(name).file_name() == Some(OsStr::new(name))
}

struct CommandStdoutReader {
    child: ControlledChild,
    stdout: ChildStdout,
    diagnostics: Option<JoinHandle<io::Result<Vec<u8>>>>,
    password_supplied: bool,
    entry: String,
    control: ControlToken,
    finished: bool,
}

impl CommandStdoutReader {
    fn finish_diagnostics(&mut self) -> io::Result<Vec<u8>> {
        let Some(handle) = self.diagnostics.take() else {
            return Ok(Vec::new());
        };
        handle
            .join()
            .map_err(|_| io::Error::other("7-Zip diagnostic reader stopped unexpectedly"))?
    }
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
        let diagnostics = self.finish_diagnostics()?;
        if self.control.is_cancelled() {
            Err(io::Error::other(FormatError::Cancelled))
        } else if status.success() {
            Ok(0)
        } else if let Some(error) = password_failure(&diagnostics, &[], self.password_supplied) {
            Err(io::Error::other(error))
        } else {
            Err(io::Error::other(format!(
                "7-Zip failed while reading {}",
                self.entry
            )))
        }
    }
}

impl Drop for CommandStdoutReader {
    fn drop(&mut self) {
        if !self.finished {
            self.child.terminate();
        }
        if let Some(handle) = self.diagnostics.take() {
            let _ = handle.join();
        }
    }
}

struct TempArchive {
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

    fn path(&self) -> &Path {
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
    fn sevenzip_missing_volume_diagnostic_accepts_only_one_file_name() {
        let stdout = br#"
Path = /private/stage/archive.part1.rar
ERROR = Missing volume : archive.part3.rar
"#;
        assert_eq!(missing_volume_name(stdout), Some("archive.part3.rar"));
        assert!(matches!(
            map_tool_failure(b"", stdout, false),
            FormatError::CorruptArchive(detail)
                if detail == "missing volume: archive.part3.rar"
        ));
        assert_eq!(
            missing_volume_name(b"ERROR = Missing volume : ../secret.rar\n"),
            None
        );
        assert_eq!(
            missing_volume_name(b"ERROR = Missing volume : child/secret.rar\n"),
            None
        );
        assert!(matches!(
            map_tool_failure(b"Cannot read password-notes.txt", b"", false),
            FormatError::CorruptArchive(_)
        ));
        assert!(matches!(
            map_tool_failure(b"Cannot open archive. Wrong password?", b"", true),
            FormatError::WrongPassword
        ));
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
    fn sevenzip_listing_skips_archive_metadata_block() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.wim
Type = wim
Physical Size = 1351
Size = 17
Packed Size = 17
Images = 1

Path = project
Folder = +
Attributes = D

Path = project/README.txt
Folder = -
Size = 10
Packed Size = 10
Attributes = N

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path.display, "project");
        assert_eq!(entries[1].path.display, "project/README.txt");
        assert_eq!(entries[1].size, 10);
        assert!(!entries
            .iter()
            .any(|entry| entry.path.display.starts_with('/')));
    }

    #[test]
    fn sevenzip_listing_reports_native_volume_properties() {
        let stdout = br#"
Path = /private/stage/archive.part001.rar
Type = Rar5
Physical Size = 2048
Total Physical Size = 4558
Multivolume = +
Volume Index = 0
Volumes = 3

----------
Path = private.txt
Folder = -
Size = 128
Packed Size = 96
Encrypted = +

"#;

        let properties = parse_7z_archive_properties(stdout).unwrap();
        assert_eq!(properties.multivolume, Some(true));
        assert_eq!(properties.volume_index, Some(0));
        assert_eq!(properties.volume_count, Some(3));
        assert_eq!(parse_7z_list(stdout).unwrap().len(), 1);
    }

    #[test]
    fn sevenzip_listing_keeps_xar_typed_entries() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.xar
Type = Xar
Physical Size = 979
Method = SHA1

Path = hello.txt
Size = 12
Packed Size = 20
Mode = -rw-r--r--
Type = file

Path = dir
Size =
Packed Size =
Mode = drwxr-xr-x
Type = directory

Path = dir/nested.txt
Size = 13
Packed Size = 21
Mode = -rw-r--r--
Type = file

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "hello.txt");
        assert_eq!(entries[0].size, 12);
        assert!(matches!(entries[0].entry_type, EntryType::File));
        assert_eq!(entries[1].path.display, "dir");
        assert!(matches!(entries[1].entry_type, EntryType::Dir));
        assert_eq!(entries[2].path.display, "dir/nested.txt");
        assert_eq!(entries[2].size, 13);
    }

    #[test]
    fn sevenzip_listing_skips_cpio_root_dot_entry() {
        let stdout = br#"
Path = .
Folder = +
Size = 0
Packed Size = 0

Path = ./sub
Folder = +
Size = 0
Packed Size = 0

Path = ./sub/data.txt
Folder = -
Size = 15
Packed Size = 16

Path = ./README.txt
Folder = -
Size = 14
Packed Size = 16

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "./sub");
        assert_eq!(entries[1].path.display, "./sub/data.txt");
        assert_eq!(entries[2].path.display, "./README.txt");
        assert!(!entries.iter().any(|entry| entry.path.display == "."));
    }

    #[test]
    fn sevenzip_listing_infers_directory_prefix_entries() {
        let stdout = br#"
Path = sub
Folder = -
Size = 0

Path = README.txt
Folder = -
Size = 15
Packed Size = 4096

Path = sub/data.txt
Folder = -
Size = 16
Packed Size = 4096

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[0].path.display, "sub");
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[0].compressed_size, None);
        assert!(matches!(entries[1].entry_type, EntryType::File));
        assert!(matches!(entries[2].entry_type, EntryType::File));
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
        use std::io::Read;
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
            .read_entry(&entries[0].path)
            .unwrap()
            .read_to_string(&mut payload)
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
        use std::io::Read;
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

Path = hello.txt
Folder = -
Size = 28
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
        assert_eq!(entries[2].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path)
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        assert_eq!(hello, "hello from 7z bridge payload");

        let mut dash = String::new();
        reader
            .read_entry(&entries[2].path)
            .unwrap()
            .read_to_string(&mut dash)
            .unwrap();
        assert_eq!(dash, "dash entry content");

        let report = reader
            .test(
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("l -slt"));
        assert!(log.contains("x -so"));
        assert!(log.contains("-- -dash.txt"), "{log}");

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

        let control = ControlToken::default();
        let cancelling_control = control.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancelling_control.cancel();
        });
        let started = Instant::now();
        let error = list_entries_with_control(&script, &archive, None, &control).unwrap_err();
        canceller.join().unwrap();

        assert!(matches!(error, FormatError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
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

        let control = ControlToken::default();
        let mut reader =
            spawn_entry_reader(&script, &archive, "file.txt", "file.txt", None, &control).unwrap();
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
