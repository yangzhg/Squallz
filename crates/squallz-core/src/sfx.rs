//! Self-extracting archive assembly and payload access.
//!
//! SFX v1 is a ZIP payload appended to a Squallz-aware PE or ELF stub,
//! followed by a fixed footer. macOS uses a separate app-bundle layout:
//! Apple does not permit arbitrary data appended to a signed Mach-O file.

mod bundle;
mod bundle_tree;
mod transaction;

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use squallz_format_api::{
    check_windows_portability, split_volume_name, ControlToken, CreateOptions, Detected, EntryPath,
    FormatError, OpenOptions, ProgressPhase, ProgressSink, ReadSeek, ResourceOptions,
};

use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};
use crate::{
    CreateArtifactKind, CreateCommitPolicy, CreateInputManifestEntry, CreateInputSummary,
    CreatePlan, Engine,
};

pub(crate) use transaction::classify_sfx_transaction_artifact;
pub use transaction::{sfx_recovery_details, SfxRecoveryDetails};

const FOOTER_MAGIC: [u8; 8] = *b"SQZSFX1\0";
const FOOTER_LEN: u64 = 32;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const LINUX_TEMPLATE_DATA_MAGIC: [u8; 8] = *b"SQZSFXD1";
const LINUX_TEMPLATE_DATA_HEADER_LEN: u64 = 48;

/// Marker compiled into the Squallz CLI binary so cross-target builders can
/// reject unrelated executable stubs.
pub const SFX_CLI_STUB_MARKER: [u8; 24] = *b"SQUALLZ_CLI_SFX_STUB_V1\0";

/// Marker compiled into the Squallz GUI binary used by macOS SFX app
/// templates.
pub const SFX_GUI_STUB_MARKER: [u8; 24] = *b"SQUALLZ_GUI_SFX_STUB_V1\0";

/// Chooses the default extraction directory below a caller-provided base.
///
/// Artifact names such as `...exe` have a `..` file stem. The chosen name
/// must be a portable path component and the artifact must have an extension,
/// so an extensionless executable cannot select itself. Invalid or missing
/// stems use a stable, ordinary folder name.
pub fn default_sfx_extract_destination(base: &Path, artifact: &Path) -> PathBuf {
    let folder = artifact
        .file_stem()
        .filter(|_| artifact.extension().is_some())
        .filter(|stem| {
            let mut components = Path::new(stem).components();
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
        })
        .filter(|stem| {
            stem.to_str()
                .is_some_and(|name| check_windows_portability(name).is_ok())
        })
        .unwrap_or_else(|| std::ffi::OsStr::new("extracted"));
    base.join(folder)
}

/// Finds the dedicated SFX runtime distributed beside a desktop executable or
/// CLI. A present candidate is returned even when it is invalid so callers can
/// surface a damaged installation directly.
pub fn discover_packaged_sfx_runtime(executable: &Path) -> Option<PathBuf> {
    let executable_dir = executable.parent()?;
    let mut directories = vec![
        executable_dir.to_path_buf(),
        executable_dir.join("bin"),
        executable_dir.join("resources/bin"),
    ];
    if let Some(prefix) = executable_dir.parent() {
        directories.push(prefix.join("lib/Squallz/bin"));
        directories.push(prefix.join("lib/squallz/bin"));
        directories.push(prefix.join("lib/squallz-gui/bin"));
    }
    directories
        .into_iter()
        .map(|directory| directory.join("sqz-sfx.stub"))
        .find(|candidate| fs::symlink_metadata(candidate).is_ok())
}

/// Physical SFX packaging layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfxLayout {
    /// PE/ELF executable with an appended ZIP and footer.
    SingleFile,
    /// macOS `.app` with the payload under `Contents/Resources`.
    MacosApp,
}

impl SfxLayout {
    /// Stable identifier used by CLI and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleFile => "single_file",
            Self::MacosApp => "macos_app",
        }
    }
}

/// Target operating system carried by an SFX artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SfxTarget {
    /// Windows Portable Executable.
    Windows,
    /// Linux ELF executable.
    Linux,
    /// macOS app bundle. Not a valid SFX v1 single-file target.
    Macos,
}

impl SfxTarget {
    /// Stable lower-case identifier used by CLI and JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Macos => "macos",
        }
    }

    /// Host target of the running build.
    pub fn host() -> Self {
        #[cfg(target_os = "windows")]
        {
            return Self::Windows;
        }
        #[cfg(target_os = "linux")]
        {
            return Self::Linux;
        }
        #[cfg(target_os = "macos")]
        {
            return Self::Macos;
        }
        #[allow(unreachable_code)]
        Self::Linux
    }

    fn footer_id(self) -> u8 {
        match self {
            Self::Windows => 1,
            Self::Linux => 2,
            Self::Macos => 3,
        }
    }

    fn from_footer_id(value: u8) -> Result<Self, FormatError> {
        match value {
            1 => Ok(Self::Windows),
            2 => Ok(Self::Linux),
            3 => Err(FormatError::CorruptArchive(
                "macOS is not a valid SFX v1 single-file target".into(),
            )),
            _ => Err(FormatError::CorruptArchive(format!(
                "unknown SFX target id {value}"
            ))),
        }
    }
}

/// Parsed metadata from an SFX v1 footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SfxInfo {
    /// Physical packaging layout.
    pub layout: SfxLayout,
    /// Target operating system.
    pub target: SfxTarget,
    /// Byte offset of the embedded ZIP payload.
    pub payload_offset: u64,
    /// Embedded ZIP payload length.
    pub payload_bytes: u64,
    /// CRC-32 of the entire embedded payload.
    pub payload_crc32: u32,
    /// SHA-256 used by the macOS bundle manifest.
    pub payload_sha256: Option<[u8; 32]>,
    /// Total SFX artifact length, including the footer and any final PE
    /// certificate table.
    pub total_bytes: u64,
    stub_bytes_value: u64,
}

impl SfxInfo {
    /// Stub length before the payload.
    pub fn stub_bytes(self) -> u64 {
        self.stub_bytes_value
    }
}

/// A verified single-file SFX payload bound to the file that was checked.
///
/// The held file is opened without following the final path component. Later
/// readers clone that handle instead of reopening `source_path`, so replacing
/// the executable path after verification cannot substitute another payload.
pub struct VerifiedSfxPayload {
    source_path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    info: SfxInfo,
}

impl std::fmt::Debug for VerifiedSfxPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedSfxPayload")
            .field("source_path", &self.source_path)
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl VerifiedSfxPayload {
    /// Footer metadata retained after the declared payload checksum passed.
    pub fn info(&self) -> SfxInfo {
        self.info
    }

    /// Original path used to bind the held file.
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(crate) fn open_reader(&self) -> Result<Box<dyn ReadSeek>, FormatError> {
        self.verify_held_state()?;
        let file = self.file.try_clone()?;
        if file_identity(&file)? != self.identity || !self.state.matches(&file.metadata()?) {
            return Err(FormatError::input_changed());
        }
        Ok(Box::new(SfxPayloadReader::from_file(file, self.info)))
    }

    pub(crate) fn verify_held_state(&self) -> Result<(), FormatError> {
        if file_identity(&self.file)? != self.identity
            || !self.state.matches(&self.file.metadata()?)
        {
            return Err(FormatError::input_changed());
        }
        Ok(())
    }
}

/// Options for SFX assembly.
#[derive(Debug, Clone, Copy)]
pub struct SfxBuildOptions {
    /// Explicit target platform.
    pub target: SfxTarget,
    /// Replace an existing destination.
    pub overwrite: bool,
    /// Buffer resource policy.
    pub resources: ResourceOptions,
}

impl Default for SfxBuildOptions {
    fn default() -> Self {
        Self {
            target: SfxTarget::host(),
            overwrite: false,
            resources: ResourceOptions::default(),
        }
    }
}

/// Result of a completed SFX build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfxBuildReport {
    /// Final output path.
    pub path: PathBuf,
    /// Target platform.
    pub target: SfxTarget,
    /// Physical packaging layout.
    pub layout: SfxLayout,
    /// Stub length.
    pub stub_bytes: u64,
    /// Payload length.
    pub payload_bytes: u64,
    /// Final artifact length.
    pub total_bytes: u64,
    /// CRC-32 stored in the footer.
    pub payload_crc32: u32,
    /// SHA-256 stored by macOS bundle manifests.
    pub payload_sha256: Option<[u8; 32]>,
    /// The final executable needs platform signing after assembly.
    pub requires_signing: bool,
    /// Verified previous outputs retained for explicit review after replacement.
    pub preserved_outputs: Vec<PathBuf>,
}

/// Completed SFX output plus the source entries used to build its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSfxBuildReport {
    pub sfx: SfxBuildReport,
    /// Complete writer-authoritative manifest in payload entry order.
    pub manifest: Vec<CreateInputManifestEntry>,
}

struct StagedSfx {
    path: PathBuf,
    identity: transaction::PathIdentity,
    held_file: Option<File>,
    report: SfxBuildReport,
    progress_total: u64,
}

impl StagedSfx {
    fn verify_held_identity(&self) -> Result<(), FormatError> {
        if self
            .held_file
            .as_ref()
            .is_some_and(|file| transaction::file_identity(file).ok() != Some(self.identity))
        {
            return Err(FormatError::Io(io::Error::other(
                "SFX staging handle identity changed",
            )));
        }
        Ok(())
    }

    fn discard(&self) -> Result<(), FormatError> {
        self.verify_held_identity()?;
        transaction::discard_staged_path(
            &self.path,
            self.identity,
            self.report.layout,
            &self.report.path,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct SfxFooter {
    target: SfxTarget,
    payload_offset: u64,
    payload_bytes: u64,
    payload_crc32: u32,
}

impl SfxFooter {
    fn encode(self) -> [u8; FOOTER_LEN as usize] {
        let mut out = [0u8; FOOTER_LEN as usize];
        out[..8].copy_from_slice(&FOOTER_MAGIC);
        out[8] = self.target.footer_id();
        out[12..20].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[20..28].copy_from_slice(&self.payload_bytes.to_le_bytes());
        out[28..32].copy_from_slice(&self.payload_crc32.to_le_bytes());
        out
    }

    fn decode(
        bytes: &[u8; FOOTER_LEN as usize],
        footer_end: u64,
    ) -> Result<Option<Self>, FormatError> {
        if bytes[..8] != FOOTER_MAGIC {
            return Ok(None);
        }
        if bytes[9..12] != [0, 0, 0] {
            return Err(FormatError::CorruptArchive(
                "unsupported SFX footer flags".into(),
            ));
        }
        let target = SfxTarget::from_footer_id(bytes[8])?;
        let payload_offset = u64::from_le_bytes(copy_array(&bytes[12..20])?);
        let payload_bytes = u64::from_le_bytes(copy_array(&bytes[20..28])?);
        let payload_crc32 = u32::from_le_bytes(copy_array(&bytes[28..32])?);
        let footer_offset = footer_end
            .checked_sub(FOOTER_LEN)
            .ok_or_else(|| FormatError::CorruptArchive("truncated SFX footer".into()))?;
        let payload_end = payload_offset
            .checked_add(payload_bytes)
            .ok_or_else(|| FormatError::CorruptArchive("SFX payload bounds overflow".into()))?;
        if payload_offset == 0 || payload_bytes == 0 || payload_end != footer_offset {
            return Err(FormatError::CorruptArchive(
                "SFX payload bounds do not match the artifact length".into(),
            ));
        }
        Ok(Some(Self {
            target,
            payload_offset,
            payload_bytes,
            payload_crc32,
        }))
    }
}

fn copy_array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], FormatError> {
    bytes
        .try_into()
        .map_err(|_| FormatError::CorruptArchive("truncated SFX footer field".into()))
}

/// Reads an SFX footer without verifying the payload checksum.
pub fn inspect_sfx(path: &Path) -> Result<Option<SfxInfo>, FormatError> {
    let metadata = fs::metadata(path)?;
    if metadata.is_dir() {
        return bundle::inspect(path);
    }
    if !metadata.is_file() || metadata.len() < FOOTER_LEN {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    inspect_single_file_sfx(&mut file, metadata.len())
}

fn inspect_single_file_sfx(file: &mut File, file_len: u64) -> Result<Option<SfxInfo>, FormatError> {
    if file_len < FOOTER_LEN {
        return Ok(None);
    }
    let footer = match read_footer_ending_at(file, file_len)? {
        Some(footer) => footer,
        None => {
            let Some((certificate_offset, certificate_bytes)) =
                pe_certificate_table(file, file_len)?
            else {
                return Ok(None);
            };
            let Some(footer) = read_footer_before_certificate(
                file,
                certificate_offset,
                certificate_bytes,
                file_len,
            )?
            else {
                return Ok(None);
            };
            if footer.target != SfxTarget::Windows {
                return Err(FormatError::CorruptArchive(
                    "only a Windows SFX may precede a PE certificate table".into(),
                ));
            }
            footer
        }
    };
    Ok(Some(SfxInfo {
        layout: SfxLayout::SingleFile,
        target: footer.target,
        payload_offset: footer.payload_offset,
        payload_bytes: footer.payload_bytes,
        payload_crc32: footer.payload_crc32,
        payload_sha256: None,
        total_bytes: file_len,
        stub_bytes_value: footer.payload_offset,
    }))
}

fn read_footer_ending_at(
    file: &mut File,
    footer_end: u64,
) -> Result<Option<SfxFooter>, FormatError> {
    if footer_end < FOOTER_LEN {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(footer_end - FOOTER_LEN))?;
    let mut bytes = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut bytes)?;
    SfxFooter::decode(&bytes, footer_end)
}

fn read_footer_before_certificate(
    file: &mut File,
    certificate_offset: u64,
    certificate_bytes: u64,
    file_len: u64,
) -> Result<Option<SfxFooter>, FormatError> {
    if certificate_offset
        .checked_add(certificate_bytes)
        .is_none_or(|end| end != file_len)
    {
        return Err(FormatError::CorruptArchive(
            "PE certificate table does not end at the SFX file boundary".into(),
        ));
    }
    for padding in 0..=7u64 {
        let Some(footer_end) = certificate_offset.checked_sub(padding) else {
            continue;
        };
        if footer_end < FOOTER_LEN {
            continue;
        }
        if padding > 0 {
            file.seek(SeekFrom::Start(footer_end))?;
            let mut bytes = [0u8; 7];
            file.read_exact(&mut bytes[..padding as usize])?;
            if bytes[..padding as usize].iter().any(|value| *value != 0) {
                continue;
            }
        }
        if let Some(footer) = read_footer_ending_at(file, footer_end)? {
            return Ok(Some(footer));
        }
    }
    Ok(None)
}

fn pe_certificate_table(file: &mut File, file_len: u64) -> Result<Option<(u64, u64)>, FormatError> {
    if file_len < 64 {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)?;
    if dos[..2] != *b"MZ" {
        return Ok(None);
    }
    let pe_offset = u32::from_le_bytes(copy_array(&dos[0x3c..0x40])?) as u64;
    let coff_start = pe_offset
        .checked_add(4)
        .ok_or_else(|| FormatError::CorruptArchive("PE header offset overflow".into()))?;
    if coff_start.checked_add(20).is_none_or(|end| end > file_len) {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(pe_offset))?;
    let mut signature = [0u8; 4];
    file.read_exact(&mut signature)?;
    if signature != *b"PE\0\0" {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(coff_start + 16))?;
    let mut size_bytes = [0u8; 2];
    file.read_exact(&mut size_bytes)?;
    let optional_size = u16::from_le_bytes(size_bytes) as u64;
    let optional_start = coff_start + 20;
    if optional_size < 2
        || optional_start
            .checked_add(optional_size)
            .is_none_or(|end| end > file_len)
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(optional_start))?;
    let mut magic_bytes = [0u8; 2];
    file.read_exact(&mut magic_bytes)?;
    let (data_directory_offset, directory_count_offset) = match u16::from_le_bytes(magic_bytes) {
        0x10b => (96u64, 92u64),
        0x20b => (112u64, 108u64),
        _ => return Ok(None),
    };
    if directory_count_offset + 4 > optional_size {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(optional_start + directory_count_offset))?;
    let mut count_bytes = [0u8; 4];
    file.read_exact(&mut count_bytes)?;
    if u32::from_le_bytes(count_bytes) <= 4 {
        return Ok(None);
    }
    let certificate_entry = optional_start
        .checked_add(data_directory_offset)
        .and_then(|value| value.checked_add(4 * 8))
        .ok_or_else(|| FormatError::CorruptArchive("PE data directory offset overflow".into()))?;
    if certificate_entry
        .checked_add(8)
        .is_none_or(|end| end > optional_start + optional_size)
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(certificate_entry))?;
    let mut entry = [0u8; 8];
    file.read_exact(&mut entry)?;
    let offset = u32::from_le_bytes(copy_array(&entry[..4])?) as u64;
    let size = u32::from_le_bytes(copy_array(&entry[4..])?) as u64;
    if offset == 0 && size == 0 {
        return Ok(None);
    }
    if offset == 0
        || !offset.is_multiple_of(8)
        || size < 8
        || offset.checked_add(size).is_none_or(|end| end > file_len)
    {
        return Ok(None);
    }
    Ok(Some((offset, size)))
}

/// Verifies the payload footer checksum in a streaming pass.
pub fn verify_sfx_payload(
    path: &Path,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<SfxInfo, FormatError> {
    if fs::symlink_metadata(path)?.is_dir() {
        let info = inspect_sfx(path)?.ok_or_else(|| {
            FormatError::Unsupported(format!("{} is not a Squallz SFX artifact", path.display()))
        })?;
        return bundle::verify(path, info, resources, progress, ctl);
    }
    verify_and_open_sfx_payload(path, resources, progress, ctl).map(|payload| payload.info())
}

/// Verifies a Windows/Linux single-file SFX and retains the exact file handle
/// whose payload checksum passed.
///
/// macOS SFX app bundles use multiple files and remain supported by
/// [`verify_sfx_payload`]; this handle-oriented API deliberately accepts only
/// the single-file layout.
pub fn verify_and_open_sfx_payload(
    path: &Path,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<VerifiedSfxPayload, FormatError> {
    ctl.checkpoint()?;
    let path_metadata = fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(format!(
            "SFX artifact must not be a symbolic link: {}",
            path.display()
        )));
    }
    if path_metadata.is_dir() {
        return Err(FormatError::Unsupported(
            "verified SFX payload handles support Windows/Linux single-file artifacts only; macOS app bundles use verify_sfx_payload"
                .into(),
        ));
    }

    let mut file = open_regular_file_no_follow(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(FormatError::Unsupported(format!(
            "SFX artifact must be a regular file: {}",
            path.display()
        )));
    }
    let identity = file_identity(&file)?;
    let state = RegularFileState::from_metadata(&metadata);
    verify_sfx_path_binding(path, identity, &state)?;

    let info = inspect_single_file_sfx(&mut file, state.bytes())?.ok_or_else(|| {
        FormatError::Unsupported(format!("{} is not a Squallz SFX artifact", path.display()))
    })?;
    verify_sfx_file_binding(&file, identity, &state)?;
    verify_sfx_path_binding(path, identity, &state)?;
    verify_single_file_payload_checksum(&file, info, resources, progress, ctl)?;
    verify_sfx_file_binding(&file, identity, &state)?;
    verify_sfx_path_binding(path, identity, &state)?;
    ctl.checkpoint()?;

    Ok(VerifiedSfxPayload {
        source_path: path.to_path_buf(),
        file,
        identity,
        state,
        info,
    })
}

fn verify_single_file_payload_checksum(
    file: &File,
    info: SfxInfo,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut reader = SfxPayloadReader::from_file(file.try_clone()?, info);
    let buffer_len = resources.stream_buffer_size(COPY_BUFFER_BYTES)?;
    let mut buffer = vec![0u8; buffer_len];
    let mut hasher = Hasher::new();
    let mut remaining = info.payload_bytes;
    let mut done = 0u64;
    let label = EntryPath::from_utf8("payload.zip");
    while remaining > 0 {
        ctl.checkpoint()?;
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(FormatError::CorruptArchive(
                "SFX payload ended before its declared length".into(),
            ));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
        done += read as u64;
        progress.on_entry_progress(done, info.payload_bytes, &label, done, info.payload_bytes);
    }
    ctl.checkpoint()?;
    let actual = hasher.finalize();
    if actual != info.payload_crc32 {
        return Err(FormatError::CorruptArchive(format!(
            "SFX payload checksum mismatch: expected {:08x}, got {actual:08x}",
            info.payload_crc32
        )));
    }
    Ok(())
}

fn verify_sfx_file_binding(
    file: &File,
    identity: PathIdentity,
    state: &RegularFileState,
) -> Result<(), FormatError> {
    if file_identity(file)? != identity || !state.matches(&file.metadata()?) {
        return Err(FormatError::input_changed());
    }
    Ok(())
}

fn verify_sfx_path_binding(
    path: &Path,
    identity: PathIdentity,
    state: &RegularFileState,
) -> Result<(), FormatError> {
    let identity_before = path_identity(path).map_err(|_| FormatError::input_changed())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| FormatError::input_changed())?;
    if identity_before != identity
        || metadata.file_type().is_symlink()
        || !state.matches(&metadata)
        || path_identity(path).map_err(|_| FormatError::input_changed())? != identity
    {
        return Err(FormatError::input_changed());
    }
    Ok(())
}

impl Engine {
    /// Builds a conservative workspace plan for creating a ZIP payload and
    /// wrapping it in a self-extracting artifact.
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn plan_sfx_from_inputs(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
    ) -> Result<CreatePlan, FormatError> {
        self.plan_sfx_from_inputs_with_progress(
            stub,
            inputs,
            dest,
            create_opts,
            sfx_opts,
            |_count, _path| {},
        )
    }

    /// Progress-reporting variant of [`Engine::plan_sfx_from_inputs`].
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn plan_sfx_from_inputs_with_progress(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        progress: impl FnMut(usize, &str),
    ) -> Result<CreatePlan, FormatError> {
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "self-extracting archives require one complete ZIP payload".into(),
            ));
        }
        let validation_ctl = ControlToken::new();
        let validated_template = validate_sfx_template_for_build(stub, sfx_opts, &validation_ctl)?;
        let input_summary = crate::create::prepare_sfx_input_summary_with_progress(
            self,
            inputs,
            dest,
            create_opts,
            progress,
        )?;
        plan_sfx_from_summary(dest, input_summary, &validated_template)
    }

    /// Assembles a Squallz-aware PE/ELF stub and a ZIP payload into an SFX v1
    /// artifact. The payload is streamed and the destination is committed by
    /// sibling rename only after the footer and checksum are complete.
    pub fn create_sfx(
        &self,
        stub: &Path,
        archive: &Path,
        dest: &Path,
        opts: &SfxBuildOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<SfxBuildReport, FormatError> {
        self.create_sfx_with_policy(
            stub,
            archive,
            dest,
            opts,
            commit_policy_from_overwrite(opts.overwrite),
            progress,
            ctl,
        )
    }

    /// Policy-controlled variant of [`Engine::create_sfx`]. The commit policy
    /// is authoritative for destination replacement; the remaining build
    /// options retain their existing meaning.
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn create_sfx_with_policy(
        &self,
        stub: &Path,
        archive: &Path,
        dest: &Path,
        opts: &SfxBuildOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<SfxBuildReport, FormatError> {
        transaction::preflight_destination(dest)?;
        let layout = if opts.target == SfxTarget::Macos {
            SfxLayout::MacosApp
        } else {
            SfxLayout::SingleFile
        };
        verify_commit_policy_destination(dest, layout, commit_policy, progress, ctl)?;
        let opts = options_for_commit_policy(opts, commit_policy);
        let staged = self.stage_sfx(stub, archive, dest, &opts, progress, ctl)?;
        publish_staged_sfx(staged, dest, commit_policy, progress, ctl)
    }

    fn stage_sfx(
        &self,
        stub: &Path,
        archive: &Path,
        dest: &Path,
        opts: &SfxBuildOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<StagedSfx, FormatError> {
        if opts.target == SfxTarget::Macos {
            let prepared = bundle::prepare_template(stub)?;
            let payload = BoundSfxPayload::open(self, archive)?;
            return bundle::stage(self, prepared, payload, dest, opts, progress, ctl);
        }
        validate_build_paths(stub, archive, dest, opts.overwrite)?;
        let validated_template = match validate_sfx_template_for_build(stub, opts, ctl)? {
            ValidatedSfxTemplate::SingleFile(template) => template,
            ValidatedSfxTemplate::Macos(_) => {
                return Err(FormatError::Unsupported(
                    "macOS SFX templates require the app-bundle layout".into(),
                ));
            }
        };
        let payload = BoundSfxPayload::open(self, archive)?;
        self.stage_single_file_sfx(stub, payload, dest, opts, progress, ctl, validated_template)
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_single_file_sfx(
        &self,
        stub: &Path,
        mut payload: BoundSfxPayload,
        dest: &Path,
        opts: &SfxBuildOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        mut validated_template: ValidatedSingleFileTemplate,
    ) -> Result<StagedSfx, FormatError> {
        validate_build_paths(stub, payload.path(), dest, opts.overwrite)?;
        payload.verify()?;
        let payload_bytes = payload.len();
        let stub_bytes = validated_template.stub_bytes;
        let total_bytes = stub_bytes
            .checked_add(payload_bytes)
            .and_then(|value| value.checked_add(FOOTER_LEN))
            .ok_or_else(|| FormatError::ResourceLimitExceeded("SFX size overflow".into()))?;
        ensure_destination_space(dest, total_bytes)?;

        let reserved = transaction::reserve_single_file_stage(dest)?;
        let tmp = reserved.path.clone();
        let staged_identity = reserved.identity;
        let result = (|| {
            let mut output = reserved.file.try_clone()?;
            let mut done = 0u64;
            verify_single_file_template_binding(stub, &validated_template)?;
            validated_template
                .file
                .seek(SeekFrom::Start(validated_template.stub_offset))?;
            let expected_digest = validated_template.expected_digest;
            copy_plain_file(
                stub,
                &mut validated_template.file,
                &mut output,
                &opts.resources,
                progress,
                ctl,
                &mut done,
                total_bytes,
                "stub",
                stub_bytes,
                expected_digest,
            )?;
            verify_single_file_template_binding(stub, &validated_template)?;
            let payload_crc32 = copy_payload(
                &mut payload,
                &mut output,
                &opts.resources,
                progress,
                ctl,
                &mut done,
                total_bytes,
                payload_bytes,
            )?;
            output.write_all(
                &SfxFooter {
                    target: opts.target,
                    payload_offset: stub_bytes,
                    payload_bytes,
                    payload_crc32,
                }
                .encode(),
            )?;
            finalize_single_file_stage_with(
                &mut || output.sync_all(),
                &mut || {
                    let reader = self.open_with_control(&tmp, &OpenOptions::default(), ctl)?;
                    drop(reader);
                    Ok(())
                },
                &mut || {
                    copy_executable_permissions(
                        validated_template.permissions.clone(),
                        &reserved.file,
                    )
                },
            )?;
            drop(output);
            if transaction::file_identity(&reserved.file)? != staged_identity
                || transaction::path_identity(&tmp)? != staged_identity
            {
                return Err(FormatError::Io(io::Error::other(
                    "SFX staging changed after writing",
                )));
            }
            Ok(StagedSfx {
                path: tmp.clone(),
                identity: staged_identity,
                held_file: Some(reserved.file),
                progress_total: total_bytes,
                report: SfxBuildReport {
                    path: dest.to_path_buf(),
                    target: opts.target,
                    layout: SfxLayout::SingleFile,
                    stub_bytes,
                    payload_bytes,
                    total_bytes,
                    payload_crc32,
                    payload_sha256: None,
                    requires_signing: true,
                    preserved_outputs: Vec::new(),
                },
            })
        })();
        match result {
            Ok(staged) => Ok(staged),
            Err(error) => Err(transaction::merge_cleanup_result(
                error,
                transaction::discard_staged_path(
                    &tmp,
                    staged_identity,
                    SfxLayout::SingleFile,
                    dest,
                ),
                dest,
            )),
        }
    }

    /// Creates the ZIP payload and wraps it in a target SFX as one cancellable
    /// operation. The intermediate ZIP is placed beside the destination and is
    /// removed after assembly.
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn create_sfx_from_inputs(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<SfxBuildReport, FormatError> {
        self.create_sfx_from_inputs_with_policy(
            stub,
            inputs,
            dest,
            create_opts,
            sfx_opts,
            commit_policy_from_overwrite(sfx_opts.overwrite),
            progress,
            ctl,
        )
    }

    /// Policy-controlled variant of [`Engine::create_sfx_from_inputs`].
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn create_sfx_from_inputs_with_policy(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<SfxBuildReport, FormatError> {
        self.create_sfx_from_inputs_internal(
            stub,
            inputs,
            dest,
            create_opts,
            sfx_opts,
            commit_policy,
            progress,
            ctl,
            false,
        )
        .map(|report| report.sfx)
    }

    /// Creates an SFX and reports every source entry accepted by its ZIP
    /// writer. Regular-file hashes come from the payload writer's input stream.
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn create_sfx_from_inputs_with_verification(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<VerifiedSfxBuildReport, FormatError> {
        self.create_sfx_from_inputs_with_verification_and_policy(
            stub,
            inputs,
            dest,
            create_opts,
            sfx_opts,
            commit_policy_from_overwrite(sfx_opts.overwrite),
            progress,
            ctl,
        )
    }

    /// Policy-controlled variant of
    /// [`Engine::create_sfx_from_inputs_with_verification`].
    #[allow(clippy::too_many_arguments)] // engine facade: each parameter has a distinct role
    pub fn create_sfx_from_inputs_with_verification_and_policy(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<VerifiedSfxBuildReport, FormatError> {
        self.create_sfx_from_inputs_internal(
            stub,
            inputs,
            dest,
            create_opts,
            sfx_opts,
            commit_policy,
            progress,
            ctl,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)] // shared SFX pipeline; each role is distinct
    fn create_sfx_from_inputs_internal(
        &self,
        stub: &Path,
        inputs: &[PathBuf],
        dest: &Path,
        create_opts: &CreateOptions,
        sfx_opts: &SfxBuildOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        capture_input_manifest: bool,
    ) -> Result<VerifiedSfxBuildReport, FormatError> {
        transaction::preflight_destination(dest)?;
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "self-extracting archives require one complete ZIP payload".into(),
            ));
        }
        let layout = if sfx_opts.target == SfxTarget::Macos {
            SfxLayout::MacosApp
        } else {
            SfxLayout::SingleFile
        };
        verify_commit_policy_destination(dest, layout, commit_policy, progress, ctl)?;
        let sfx_opts = options_for_commit_policy(sfx_opts, commit_policy);
        validate_publish_destination(dest, layout, commit_policy_allows_replace(commit_policy))?;
        let validated_template = validate_sfx_template_for_build(stub, &sfx_opts, ctl)?;

        let payload_reservation = transaction::reserve_payload_path(dest)?;
        let payload = payload_reservation.path.clone();
        let payload_identity = payload_reservation.identity;
        let create_reservation = clone_reserved_payload_for_create(&payload_reservation, dest)?;
        let result = (|| {
            let prepared = crate::create::prepare_unsplit_create_with_reserved_outputs(
                self,
                &payload,
                inputs,
                &[dest],
                create_opts,
                |_count, _path| {},
            )?;
            let plan = plan_sfx_from_summary(dest, prepared.summary(), &validated_template)?;
            ensure_destination_space(dest, plan.workspace_budget_bytes)?;
            let verified = crate::create::create_prepared_into_reserved_output(
                self,
                &payload,
                inputs,
                &[dest],
                create_opts,
                progress,
                ctl,
                prepared,
                capture_input_manifest,
                create_reservation,
            )?;
            let input_manifest = verified.manifest;
            let bound_payload = BoundSfxPayload::from_reserved(self, payload_reservation)?;
            ctl.checkpoint()?;
            let staged = match validated_template {
                ValidatedSfxTemplate::Macos(prepared) => bundle::stage(
                    self,
                    prepared,
                    bound_payload,
                    dest,
                    &sfx_opts,
                    progress,
                    ctl,
                ),
                ValidatedSfxTemplate::SingleFile(template) => self.stage_single_file_sfx(
                    stub,
                    bound_payload,
                    dest,
                    &sfx_opts,
                    progress,
                    ctl,
                    template,
                ),
            }?;
            Ok((staged, input_manifest))
        })();
        match result {
            Ok((staged, input_manifest)) => {
                publish_staged_sfx_after_cleanup(staged, dest, commit_policy, progress, ctl, || {
                    transaction::discard_staged_path(
                        &payload,
                        payload_identity,
                        SfxLayout::SingleFile,
                        dest,
                    )
                })
                .map(|sfx| VerifiedSfxBuildReport {
                    sfx,
                    manifest: input_manifest,
                })
            }
            Err(error) => Err(transaction::merge_cleanup_result(
                error,
                transaction::discard_staged_path(
                    &payload,
                    payload_identity,
                    SfxLayout::SingleFile,
                    dest,
                ),
                dest,
            )),
        }
    }
}

fn clone_reserved_payload_for_create(
    reserved: &crate::ReservedTempFile,
    destination: &Path,
) -> Result<crate::ReservedTempFile, FormatError> {
    clone_reserved_payload_for_create_with(reserved, destination, File::try_clone)
}

fn clone_reserved_payload_for_create_with(
    reserved: &crate::ReservedTempFile,
    destination: &Path,
    clone_file: impl FnOnce(&File) -> io::Result<File>,
) -> Result<crate::ReservedTempFile, FormatError> {
    let file = match clone_file(&reserved.file) {
        Ok(file) => file,
        Err(error) => {
            return Err(transaction::merge_cleanup_result(
                error.into(),
                crate::remove_bound_temp_file(&reserved.path, &reserved.file, reserved.identity),
                destination,
            ));
        }
    };
    Ok(crate::ReservedTempFile {
        path: reserved.path.clone(),
        file,
        identity: reserved.identity,
    })
}

struct ValidatedSingleFileTemplate {
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    stub_offset: u64,
    stub_bytes: u64,
    expected_digest: Option<[u8; 32]>,
    permissions: fs::Permissions,
}

pub(super) struct BoundSfxPayload {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
}

impl BoundSfxPayload {
    fn open(engine: &Engine, path: &Path) -> Result<Self, FormatError> {
        let file = open_regular_file_no_follow(path)?;
        let identity = file_identity(&file)?;
        Self::from_file(engine, path, file, identity)
    }

    fn from_reserved(
        engine: &Engine,
        reserved: crate::ReservedTempFile,
    ) -> Result<Self, FormatError> {
        let path = reserved.path;
        Self::from_file(engine, &path, reserved.file, reserved.identity)
    }

    fn from_file(
        engine: &Engine,
        path: &Path,
        file: File,
        expected_identity: PathIdentity,
    ) -> Result<Self, FormatError> {
        let state = RegularFileState::from_metadata(&file.metadata()?);
        let payload = Self {
            path: path.to_path_buf(),
            file,
            identity: expected_identity,
            state,
        };
        payload.verify()?;
        validate_zip_payload(engine, path)?;
        payload.verify()?;
        Ok(payload)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn len(&self) -> u64 {
        self.state.bytes()
    }

    pub(super) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(super) fn verify(&self) -> Result<(), FormatError> {
        let path_metadata = fs::symlink_metadata(&self.path)?;
        if !path_metadata.file_type().is_file()
            || file_identity(&self.file)? != self.identity
            || path_identity(&self.path)? != self.identity
            || !self.state.matches(&self.file.metadata()?)
            || !self.state.matches(&path_metadata)
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "SFX payload changed while it was being consumed: {}",
                self.path.display()
            ))));
        }
        Ok(())
    }
}

enum ValidatedSfxTemplate {
    SingleFile(ValidatedSingleFileTemplate),
    Macos(bundle::PreparedTemplate),
}

#[derive(Clone, Copy)]
struct SingleFileTemplateContent {
    offset: u64,
    bytes: u64,
    expected_digest: Option<[u8; 32]>,
}

fn single_file_template_content(
    file: &mut File,
    file_bytes: u64,
    target: SfxTarget,
    resources: &ResourceOptions,
    ctl: &ControlToken,
) -> Result<SingleFileTemplateContent, FormatError> {
    if file_bytes < LINUX_TEMPLATE_DATA_MAGIC.len() as u64 {
        return Ok(SingleFileTemplateContent {
            offset: 0,
            bytes: file_bytes,
            expected_digest: None,
        });
    }

    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; LINUX_TEMPLATE_DATA_MAGIC.len()];
    file.read_exact(&mut magic)?;
    if magic != LINUX_TEMPLATE_DATA_MAGIC {
        return Ok(SingleFileTemplateContent {
            offset: 0,
            bytes: file_bytes,
            expected_digest: None,
        });
    }
    if target != SfxTarget::Linux {
        return Err(FormatError::Unsupported(
            "Linux SFX template data cannot be used for another target".into(),
        ));
    }

    let mut length = [0u8; 8];
    let mut expected_digest = [0u8; 32];
    file.read_exact(&mut length)?;
    file.read_exact(&mut expected_digest)?;
    let content_bytes = u64::from_le_bytes(length);
    let expected_file_bytes = LINUX_TEMPLATE_DATA_HEADER_LEN
        .checked_add(content_bytes)
        .ok_or_else(|| {
            FormatError::Unsupported("Linux SFX template data length overflow".into())
        })?;
    if content_bytes == 0 || expected_file_bytes != file_bytes {
        return Err(FormatError::Unsupported(
            "Linux SFX template data has an invalid length".into(),
        ));
    }

    let mut hasher = Sha256::new();
    let mut remaining = content_bytes;
    let mut buffer = vec![0u8; resources.stream_buffer_size(COPY_BUFFER_BYTES)?];
    while remaining > 0 {
        ctl.checkpoint()?;
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Linux SFX template data was truncated",
            )));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let actual_digest: [u8; 32] = hasher.finalize().into();
    if actual_digest != expected_digest {
        return Err(FormatError::Unsupported(
            "Linux SFX template data failed its SHA-256 check".into(),
        ));
    }

    Ok(SingleFileTemplateContent {
        offset: LINUX_TEMPLATE_DATA_HEADER_LEN,
        bytes: content_bytes,
        expected_digest: Some(expected_digest),
    })
}

fn content_has_sfx_footer(
    file: &mut File,
    content: SingleFileTemplateContent,
) -> Result<bool, FormatError> {
    if content.bytes < FOOTER_LEN {
        return Ok(false);
    }
    let footer_offset = content
        .offset
        .checked_add(content.bytes - FOOTER_LEN)
        .ok_or_else(|| FormatError::CorruptArchive("SFX template offset overflow".into()))?;
    file.seek(SeekFrom::Start(footer_offset))?;
    let mut bytes = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut bytes)?;
    Ok(SfxFooter::decode(&bytes, content.bytes)?.is_some())
}

/// Validates that an SFX runtime template is a first-party Squallz runtime for
/// the requested target. The template is only read; no payload or output is
/// created.
pub fn validate_sfx_template(
    stub: &Path,
    opts: &SfxBuildOptions,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    validate_sfx_template_for_build(stub, opts, ctl).map(|_| ())
}

fn validate_sfx_template_for_build(
    stub: &Path,
    opts: &SfxBuildOptions,
    ctl: &ControlToken,
) -> Result<ValidatedSfxTemplate, FormatError> {
    if opts.target == SfxTarget::Macos {
        let prepared = bundle::prepare_template(stub)?;
        prepared.validate_runtime(&opts.resources, ctl)?;
        return Ok(ValidatedSfxTemplate::Macos(prepared));
    }

    let path_metadata = fs::symlink_metadata(stub)?;
    if !path_metadata.file_type().is_file() {
        return Err(FormatError::Unsupported(
            "SFX stub must be a non-symlink regular file".into(),
        ));
    }
    let mut stub_file = open_regular_file_no_follow(stub)?;
    let metadata = stub_file.metadata()?;
    if !metadata.is_file() {
        return Err(FormatError::Unsupported(
            "SFX stub must be a non-symlink regular file".into(),
        ));
    }
    let identity = file_identity(&stub_file)?;
    let state = RegularFileState::from_metadata(&metadata);
    let content = single_file_template_content(
        &mut stub_file,
        metadata.len(),
        opts.target,
        &opts.resources,
        ctl,
    )?;
    let contains_sfx = if content.offset == 0 {
        inspect_single_file_sfx(&mut stub_file, content.bytes)?.is_some()
    } else {
        content_has_sfx_footer(&mut stub_file, content)?
    };
    if contains_sfx {
        return Err(FormatError::Unsupported(
            "an existing SFX artifact cannot be reused as a stub".into(),
        ));
    }
    stub_file.seek(SeekFrom::Start(content.offset))?;
    let detected_target = executable_target_from_file(&mut stub_file, stub)?;
    if detected_target != opts.target {
        return Err(FormatError::Unsupported(format!(
            "SFX stub target {} does not match requested target {}",
            detected_target.as_str(),
            opts.target.as_str()
        )));
    }
    if opts.target == SfxTarget::Windows
        && pe_certificate_table(&mut stub_file, metadata.len())?.is_some()
    {
        return Err(FormatError::Unsupported(
            "use an unsigned Squallz Windows stub and sign the completed SFX artifact".into(),
        ));
    }
    stub_file.seek(SeekFrom::Start(content.offset))?;
    let mut content_reader = (&mut stub_file).take(content.bytes);
    if !file_has_marker(
        &mut content_reader,
        &SFX_CLI_STUB_MARKER,
        &opts.resources,
        ctl,
    )? {
        return Err(FormatError::Unsupported(
            "the selected executable is not a Squallz SFX-capable CLI stub".into(),
        ));
    }
    stub_file.seek(SeekFrom::Start(content.offset))?;
    let template = ValidatedSingleFileTemplate {
        file: stub_file,
        identity,
        state,
        stub_offset: content.offset,
        stub_bytes: content.bytes,
        expected_digest: content.expected_digest,
        permissions: metadata.permissions(),
    };
    verify_single_file_template_binding(stub, &template)?;
    Ok(ValidatedSfxTemplate::SingleFile(template))
}

fn plan_sfx_from_summary(
    dest: &Path,
    input_summary: CreateInputSummary,
    validated_template: &ValidatedSfxTemplate,
) -> Result<CreatePlan, FormatError> {
    let payload_budget = input_summary.archive_budget_bytes;
    let final_output_budget_bytes = match validated_template {
        ValidatedSfxTemplate::Macos(prepared) => prepared.output_budget(dest, payload_budget)?,
        ValidatedSfxTemplate::SingleFile(template) => {
            single_file_output_budget(template.stub_bytes, payload_budget)?
        }
    };
    Ok(create_sfx_plan(
        dest,
        input_summary,
        final_output_budget_bytes,
    ))
}

fn single_file_output_budget(stub_bytes: u64, payload_budget: u64) -> Result<u64, FormatError> {
    stub_bytes
        .checked_add(payload_budget)
        .and_then(|bytes| bytes.checked_add(FOOTER_LEN))
        .ok_or_else(|| FormatError::ResourceLimitExceeded("SFX size overflow".into()))
}

fn create_sfx_plan(
    dest: &Path,
    input_summary: CreateInputSummary,
    final_output_budget_bytes: u64,
) -> CreatePlan {
    let payload_budget = input_summary.archive_budget_bytes;
    CreatePlan {
        inputs: input_summary.estimate,
        primary_output: dest.to_path_buf(),
        archive_output_budget_bytes: payload_budget,
        final_output_budget_bytes,
        split_volume_count_budget: None,
        workspace_budget_bytes: payload_budget.saturating_add(final_output_budget_bytes),
        system_temp_budget_bytes: 0,
    }
}

fn finalize_single_file_stage_with<S, V, P>(
    sync: &mut S,
    verify: &mut V,
    copy_permissions: &mut P,
) -> Result<(), FormatError>
where
    S: FnMut() -> io::Result<()>,
    V: FnMut() -> Result<(), FormatError>,
    P: FnMut() -> Result<(), FormatError>,
{
    sync()?;
    verify()?;
    copy_permissions()?;
    sync()?;
    Ok(())
}

fn publish_staged_sfx_after_cleanup(
    staged: StagedSfx,
    dest: &Path,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    cleanup: impl FnOnce() -> Result<(), FormatError>,
) -> Result<SfxBuildReport, FormatError> {
    if let Err(error) = cleanup() {
        let target = staged.report.path.clone();
        return Err(transaction::merge_cleanup_result(
            error,
            staged.discard(),
            &target,
        ));
    }
    publish_staged_sfx(staged, dest, commit_policy, progress, ctl)
}

fn publish_staged_sfx(
    mut staged: StagedSfx,
    dest: &Path,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<SfxBuildReport, FormatError> {
    staged.verify_held_identity()?;
    progress.on_phase(ProgressPhase::OutputCommit, false);
    if let Err(error) = ctl.checkpoint() {
        let target = staged.report.path.clone();
        return Err(transaction::merge_cleanup_result(
            error,
            staged.discard(),
            &target,
        ));
    }
    staged.report.preserved_outputs = match replace_staged_path(
        &staged.path,
        staged.identity,
        dest,
        staged.report.layout,
        commit_policy,
    ) {
        Ok(preserved_outputs) => preserved_outputs,
        Err(error) => {
            if !recovery_error_requires_staging(&error) {
                let target = staged.report.path.clone();
                return Err(transaction::merge_cleanup_result(
                    error,
                    staged.discard(),
                    &target,
                ));
            }
            return Err(error);
        }
    };
    progress.on_progress(
        staged.progress_total,
        staged.progress_total,
        &EntryPath::from_utf8(""),
    );
    Ok(staged.report)
}

fn recovery_error_requires_staging(error: &FormatError) -> bool {
    transaction::sfx_recovery_requires_staging(error)
}

fn replace_staged_path(
    staged: &Path,
    staged_identity: transaction::PathIdentity,
    dest: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
) -> Result<Vec<PathBuf>, FormatError> {
    transaction::replace_bound_staged_path(staged, staged_identity, dest, layout, commit_policy)
}

fn commit_policy_from_overwrite(overwrite: bool) -> CreateCommitPolicy {
    if overwrite {
        CreateCommitPolicy::ReplaceExisting
    } else {
        CreateCommitPolicy::NoReplace
    }
}

fn commit_policy_allows_replace(commit_policy: CreateCommitPolicy) -> bool {
    !matches!(commit_policy, CreateCommitPolicy::NoReplace)
}

fn options_for_commit_policy(
    options: &SfxBuildOptions,
    commit_policy: CreateCommitPolicy,
) -> SfxBuildOptions {
    SfxBuildOptions {
        overwrite: commit_policy_allows_replace(commit_policy),
        ..*options
    }
}

fn verify_commit_policy_destination(
    destination: &Path,
    layout: SfxLayout,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let CreateCommitPolicy::ReplaceIfUnchanged(guard) = commit_policy else {
        return Ok(());
    };
    let kind = match layout {
        SfxLayout::SingleFile => CreateArtifactKind::SfxSingleFile,
        SfxLayout::MacosApp => CreateArtifactKind::SfxMacosApp,
    };
    crate::destination_guard::verify_destination_guard_with_progress(
        destination,
        kind,
        guard,
        progress,
        ctl,
    )
    .map(drop)
}

fn output_exists_error(dest: &Path) -> FormatError {
    FormatError::Unsupported(format!("SFX output already exists: {}", dest.display()))
}

pub(super) fn validate_publish_destination(
    dest: &Path,
    layout: SfxLayout,
    overwrite: bool,
) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(dest) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !overwrite {
        return Err(output_exists_error(dest));
    }
    let expected_type = match layout {
        SfxLayout::SingleFile => {
            metadata.file_type().is_file() || metadata.file_type().is_symlink()
        }
        SfxLayout::MacosApp => metadata.is_dir() && !metadata.file_type().is_symlink(),
    };
    if !expected_type {
        let expected = match layout {
            SfxLayout::SingleFile => "a regular file or symlink",
            SfxLayout::MacosApp => "a non-symlink app directory",
        };
        return Err(FormatError::Unsupported(format!(
            "SFX output must be {expected}: {}",
            dest.display()
        )));
    }
    Ok(true)
}

#[cfg(test)]
fn remove_staged_path(path: &Path, layout: SfxLayout) -> Result<(), FormatError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if layout == SfxLayout::SingleFile
                && (metadata.file_type().is_file() || metadata.file_type().is_symlink()) =>
        {
            fs::remove_file(path)?;
        }
        Ok(metadata)
            if layout == SfxLayout::MacosApp
                && metadata.is_dir()
                && !metadata.file_type().is_symlink() =>
        {
            fs::remove_dir_all(path)?;
        }
        Ok(_) => {
            return Err(FormatError::Unsupported(
                "SFX staging path changed to an unexpected file type".into(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn validate_build_paths(
    stub: &Path,
    archive: &Path,
    dest: &Path,
    overwrite: bool,
) -> Result<(), FormatError> {
    if !fs::metadata(stub)?.is_file() {
        return Err(FormatError::Unsupported(
            "SFX stub must be a regular file".into(),
        ));
    }
    if !fs::metadata(archive)?.is_file() {
        return Err(FormatError::Unsupported(
            "SFX payload must be a regular archive file".into(),
        ));
    }
    if crate::same_existing_path(stub, archive)
        || crate::same_existing_path(stub, dest)
        || crate::same_existing_path(archive, dest)
    {
        return Err(FormatError::Unsupported(
            "SFX stub, payload, and output paths must be different".into(),
        ));
    }
    validate_publish_destination(dest, SfxLayout::SingleFile, overwrite)?;
    if inspect_sfx(stub)?.is_some() {
        return Err(FormatError::Unsupported(
            "an existing SFX artifact cannot be reused as a stub".into(),
        ));
    }
    if inspect_sfx(archive)?.is_some() {
        return Err(FormatError::Unsupported(
            "an SFX artifact cannot be nested as the payload of another SFX artifact".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_zip_payload(engine: &Engine, archive: &Path) -> Result<(), FormatError> {
    let name = archive
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| FormatError::Unsupported("SFX payload file name is not UTF-8".into()))?;
    if split_volume_name(name).is_some() {
        return Err(FormatError::Unsupported(
            "SFX v1 requires one complete ZIP payload, not a split volume".into(),
        ));
    }
    match engine.registry().detect_by_name(name) {
        Some(Detected::Archive(format)) if format.id() == "zip" => {}
        _ => {
            return Err(FormatError::Unsupported(
                "SFX v1 supports ZIP-compatible payloads only".into(),
            ));
        }
    }
    let _reader = engine.open(archive, &OpenOptions::default())?;
    Ok(())
}

fn ensure_destination_space(dest: &Path, required: u64) -> Result<(), FormatError> {
    let parent = dest
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if fs4::available_space(parent)? < required {
        return Err(FormatError::DiskFull);
    }
    Ok(())
}

pub(super) fn executable_target_from_file(
    file: &mut File,
    path: &Path,
) -> Result<SfxTarget, FormatError> {
    let mut head = [0u8; 64];
    let read = file.read(&mut head)?;
    if read >= 4 && head[..4] == [0x7f, b'E', b'L', b'F'] {
        return Ok(SfxTarget::Linux);
    }
    if read >= 4 && is_macho_magic(&head[..4]) {
        return Ok(SfxTarget::Macos);
    }
    if read >= 64 && head[..2] == *b"MZ" {
        let pe_offset = u32::from_le_bytes(copy_array(&head[0x3c..0x40])?) as u64;
        file.seek(SeekFrom::Start(pe_offset))?;
        let mut signature = [0u8; 4];
        file.read_exact(&mut signature)?;
        if signature == *b"PE\0\0" {
            return Ok(SfxTarget::Windows);
        }
    }
    Err(FormatError::Unsupported(format!(
        "unrecognized executable stub: {}",
        path.display()
    )))
}

fn is_macho_magic(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    )
}

pub(super) fn file_has_marker<R: Read + ?Sized>(
    reader: &mut R,
    marker: &[u8],
    resources: &ResourceOptions,
    ctl: &ControlToken,
) -> Result<bool, FormatError> {
    let buffer_len = resources
        .stream_buffer_size(COPY_BUFFER_BYTES)?
        .max(marker.len());
    let mut buffer = vec![0u8; buffer_len + marker.len() - 1];
    let mut carry = 0usize;
    loop {
        ctl.checkpoint()?;
        let read = reader.read(&mut buffer[carry..])?;
        let used = carry + read;
        if buffer[..used]
            .windows(marker.len())
            .any(|window| window == marker)
        {
            return Ok(true);
        }
        if read == 0 {
            return Ok(false);
        }
        carry = used.min(marker.len() - 1);
        buffer.copy_within(used - carry..used, 0);
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_plain_file(
    source: &Path,
    source_file: &mut File,
    output: &mut File,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    overall_done: &mut u64,
    overall_total: u64,
    label: &str,
    expected_len: u64,
    expected_digest: Option<[u8; 32]>,
) -> Result<(), FormatError> {
    let mut current_done = 0u64;
    let mut remaining = expected_len;
    let mut buffer = vec![0u8; resources.stream_buffer_size(COPY_BUFFER_BYTES)?];
    let mut hasher = expected_digest.map(|_| Sha256::new());
    let entry = EntryPath::from_utf8(label);
    while remaining > 0 {
        ctl.checkpoint()?;
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = source_file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{} changed while building SFX", source.display()),
            )));
        }
        output.write_all(&buffer[..read])?;
        if let Some(hasher) = &mut hasher {
            hasher.update(&buffer[..read]);
        }
        remaining -= read as u64;
        current_done += read as u64;
        *overall_done += read as u64;
        progress.on_entry_progress(
            *overall_done,
            overall_total,
            &entry,
            current_done,
            expected_len,
        );
    }
    reject_trailing_source_bytes(source_file, source)?;
    if let (Some(hasher), Some(expected_digest)) = (hasher, expected_digest) {
        let actual_digest: [u8; 32] = hasher.finalize().into();
        if actual_digest != expected_digest {
            return Err(FormatError::input_changed());
        }
    }
    Ok(())
}

fn verify_single_file_template_binding(
    path: &Path,
    template: &ValidatedSingleFileTemplate,
) -> Result<(), FormatError> {
    let handle_metadata = template.file.metadata()?;
    let path_metadata = fs::symlink_metadata(path)?;
    if !path_metadata.file_type().is_file()
        || file_identity(&template.file)? != template.identity
        || path_identity(path)? != template.identity
        || !template.state.matches(&handle_metadata)
        || !template.state.matches(&path_metadata)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "SFX runtime changed while building: {}",
            path.display()
        ))));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn copy_payload(
    source: &mut BoundSfxPayload,
    output: &mut File,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    overall_done: &mut u64,
    overall_total: u64,
    expected_len: u64,
) -> Result<u32, FormatError> {
    source.verify()?;
    source.file_mut().seek(SeekFrom::Start(0))?;
    let mut current_done = 0u64;
    let mut remaining = expected_len;
    let mut buffer = vec![0u8; resources.stream_buffer_size(COPY_BUFFER_BYTES)?];
    let mut hasher = Hasher::new();
    let entry = EntryPath::from_utf8("payload.zip");
    while remaining > 0 {
        ctl.checkpoint()?;
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = source.file_mut().read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{} changed while building SFX", source.path().display()),
            )));
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
        current_done += read as u64;
        *overall_done += read as u64;
        progress.on_entry_progress(
            *overall_done,
            overall_total,
            &entry,
            current_done,
            expected_len,
        );
    }
    let source_path = source.path().to_path_buf();
    reject_trailing_source_bytes(source.file_mut(), &source_path)?;
    source.verify()?;
    Ok(hasher.finalize())
}

fn reject_trailing_source_bytes(file: &mut File, source: &Path) -> Result<(), FormatError> {
    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        return Err(FormatError::Io(io::Error::other(format!(
            "{} changed while building SFX",
            source.display()
        ))));
    }
    Ok(())
}

#[cfg(unix)]
fn copy_executable_permissions(
    mut permissions: fs::Permissions,
    dest: &File,
) -> Result<(), FormatError> {
    use std::os::unix::fs::PermissionsExt;

    permissions.set_mode(permissions.mode() | 0o100);
    dest.set_permissions(permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_executable_permissions(
    permissions: fs::Permissions,
    dest: &File,
) -> Result<(), FormatError> {
    dest.set_permissions(permissions)?;
    Ok(())
}

pub(crate) fn open_sfx_payload(path: &Path) -> Result<Option<Box<dyn ReadSeek>>, FormatError> {
    let Some(info) = inspect_sfx(path)? else {
        return Ok(None);
    };
    if info.layout == SfxLayout::MacosApp {
        return bundle::open_payload(path).map(|file| Some(Box::new(file) as Box<dyn ReadSeek>));
    }
    Ok(Some(Box::new(SfxPayloadReader::new(path, info)?)))
}

/// Returns the enclosing macOS SFX bundle for an executable under
/// `Contents/MacOS`, if the fixed manifest is present.
pub fn macos_sfx_bundle_for_executable(executable: &Path) -> Option<PathBuf> {
    bundle::for_executable(executable)
}

struct SfxPayloadReader {
    file: File,
    start: u64,
    len: u64,
    position: u64,
}

impl SfxPayloadReader {
    fn new(path: &Path, info: SfxInfo) -> Result<Self, FormatError> {
        Ok(Self::from_file(File::open(path)?, info))
    }

    fn from_file(file: File, info: SfxInfo) -> Self {
        Self {
            file,
            start: info.payload_offset,
            len: info.payload_bytes,
            position: 0,
        }
    }
}

impl Read for SfxPayloadReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(0);
        }
        let limit = remaining.min(buffer.len() as u64) as usize;
        let offset = self
            .start
            .checked_add(self.position)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "SFX read overflow"))?;
        let read = read_file_at(&self.file, &mut buffer[..limit], offset)?;
        self.position = self.position.saturating_add(read as u64);
        Ok(read)
    }
}

impl Seek for SfxPayloadReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(value) => i128::from(self.len) + i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
        };
        if next < 0 || next > i128::from(self.len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SFX payload seek outside bounds",
            ));
        }
        let next = next as u64;
        self.position = next;
        Ok(next)
    }
}

#[cfg(unix)]
fn read_file_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;

    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_file_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;

    file.seek_read(buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_file_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use squallz_format_api::{
        ArchiveFormat, ArchiveReader, ArchiveWriter, EntryMeta, FormatCapabilities, FormatRegistry,
        NoProgress, TestSummary, WriteSeek,
    };

    struct TestZipFormat;

    struct TestZipReader {
        bytes: Vec<u8>,
    }

    impl ArchiveFormat for TestZipFormat {
        fn id(&self) -> &'static str {
            "zip"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["zip"]
        }

        fn capabilities(&self) -> FormatCapabilities {
            FormatCapabilities {
                can_extract: true,
                can_test: true,
                ..FormatCapabilities::default()
            }
        }

        fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
            head.starts_with(b"TESTZIP\0")
        }

        fn open(
            &self,
            mut src: Box<dyn ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            let mut bytes = Vec::new();
            src.read_to_end(&mut bytes)?;
            Ok(Box::new(TestZipReader { bytes }))
        }

        fn create(
            &self,
            _dst: Box<dyn WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("test reader is read-only".into()))
        }
    }

    impl ArchiveReader for TestZipReader {
        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            Box::new(std::iter::empty())
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            Ok(Box::new(Cursor::new(self.bytes.clone())))
        }

        fn test_summary(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestSummary, FormatError> {
            Ok(TestSummary::default())
        }
    }

    fn temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "squallz-sfx-{tag}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    fn write_single_file_sfx(path: &Path, payload: &[u8], payload_crc32: u32) {
        let footer = SfxFooter {
            target: SfxTarget::Linux,
            payload_offset: 4,
            payload_bytes: payload.len() as u64,
            payload_crc32,
        };
        let mut bytes = b"STUB".to_vec();
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&footer.encode());
        fs::write(path, bytes).unwrap();
    }

    fn write_valid_single_file_sfx(path: &Path, payload: &[u8]) {
        write_single_file_sfx(path, payload, crc32fast::hash(payload));
    }

    fn write_linux_template_data(path: &Path, runtime: &[u8]) {
        let mut data = Vec::with_capacity(LINUX_TEMPLATE_DATA_HEADER_LEN as usize + runtime.len());
        data.extend_from_slice(&LINUX_TEMPLATE_DATA_MAGIC);
        data.extend_from_slice(&(runtime.len() as u64).to_le_bytes());
        data.extend_from_slice(&Sha256::digest(runtime));
        data.extend_from_slice(runtime);
        fs::write(path, data).unwrap();
    }

    struct MutatingTemplateProgress {
        path: PathBuf,
        offset: u64,
        original_modified: std::time::SystemTime,
        mutated: AtomicBool,
    }

    impl ProgressSink for MutatingTemplateProgress {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

        fn on_entry_progress(
            &self,
            _done: u64,
            _total: u64,
            current: &EntryPath,
            _current_done: u64,
            _current_total: u64,
        ) {
            if current.display != "stub" || self.mutated.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut file = fs::OpenOptions::new().write(true).open(&self.path).unwrap();
            file.seek(SeekFrom::Start(self.offset)).unwrap();
            file.write_all(&[0x5a]).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(self.original_modified))
                .unwrap();
        }
    }

    #[test]
    fn default_extract_destination_stays_below_its_base() {
        let base = Path::new("/tmp/packages");
        assert_eq!(
            default_sfx_extract_destination(base, Path::new("Release.exe")),
            base.join("Release")
        );
        for artifact in [
            "...exe",
            "...run",
            "...app",
            "squallz",
            "CON.exe",
            "name..run",
            "name .app",
            "a:b.exe",
        ] {
            let destination = default_sfx_extract_destination(base, Path::new(artifact));
            assert_eq!(destination, base.join("extracted"));
            assert_eq!(destination.parent(), Some(base));
        }
    }

    #[test]
    fn packaged_runtime_discovery_uses_platform_resource_locations() {
        let root = temp_file("packaged-runtime-discovery");
        let _ = fs::remove_dir_all(&root);
        let executable = root.join("usr/bin/sqz");
        let runtime = root.join("usr/lib/squallz-gui/bin/sqz-sfx.stub");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        fs::write(&executable, b"cli").unwrap();
        fs::write(&runtime, b"runtime").unwrap();

        assert_eq!(discover_packaged_sfx_runtime(&executable), Some(runtime));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packaged_runtime_discovery_reports_a_present_invalid_candidate() {
        let root = temp_file("packaged-runtime-invalid");
        let _ = fs::remove_dir_all(&root);
        let executable = root.join("Squallz.exe");
        let runtime = root.join("bin/sqz-sfx.stub");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(&executable, b"gui").unwrap();

        assert_eq!(discover_packaged_sfx_runtime(&executable), Some(runtime));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn payload_clone_failure_removes_the_writer_owned_reservation() {
        let dir = temp_file("payload-clone-failure");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let destination = dir.join("output.exe");
        let reserved = transaction::reserve_payload_path(&destination).unwrap();
        let payload = reserved.path.clone();

        let error = match clone_reserved_payload_for_create_with(&reserved, &destination, |_| {
            Err(io::Error::other("injected payload handle clone failure"))
        }) {
            Ok(_) => panic!("payload clone failure unexpectedly succeeded"),
            Err(error) => error,
        };

        assert!(matches!(error, FormatError::Io(_)));
        assert!(!payload.exists());
        drop(reserved);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn footer_round_trip_keeps_payload_bounds() {
        let footer = SfxFooter {
            target: SfxTarget::Windows,
            payload_offset: 400,
            payload_bytes: 600,
            payload_crc32: 0x1234_5678,
        };
        let parsed = SfxFooter::decode(&footer.encode(), 1032).unwrap().unwrap();
        assert_eq!(parsed.target, SfxTarget::Windows);
        assert_eq!(parsed.payload_offset, 400);
        assert_eq!(parsed.payload_bytes, 600);
        assert_eq!(parsed.payload_crc32, 0x1234_5678);
    }

    #[test]
    fn footer_rejects_mismatched_bounds() {
        let footer = SfxFooter {
            target: SfxTarget::Linux,
            payload_offset: 10,
            payload_bytes: 20,
            payload_crc32: 0,
        };
        let err = SfxFooter::decode(&footer.encode(), 100).unwrap_err();
        assert!(matches!(err, FormatError::CorruptArchive(_)));
    }

    #[test]
    fn bounded_reader_never_exposes_stub_or_footer() {
        let path = temp_file("reader");
        let footer = SfxFooter {
            target: SfxTarget::Linux,
            payload_offset: 4,
            payload_bytes: 7,
            payload_crc32: crc32fast::hash(b"PAYLOAD"),
        };
        let mut bytes = b"STUBPAYLOAD".to_vec();
        bytes.extend_from_slice(&footer.encode());
        fs::write(&path, bytes).unwrap();

        let info = inspect_sfx(&path).unwrap().unwrap();
        let mut reader = SfxPayloadReader::new(&path, info).unwrap();
        let mut payload = Vec::new();
        reader.read_to_end(&mut payload).unwrap();
        assert_eq!(payload, b"PAYLOAD");
        assert_eq!(reader.seek(SeekFrom::Start(0)).unwrap(), 0);
        assert!(reader.seek(SeekFrom::End(1)).is_err());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verified_payload_readers_have_independent_positions() {
        let path = temp_file("verified-reader-positions");
        let payload_bytes = b"TESTZIP\0payload";
        write_valid_single_file_sfx(&path, payload_bytes);

        let payload = verify_and_open_sfx_payload(
            &path,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        let mut first = payload.open_reader().unwrap();
        let mut second = payload.open_reader().unwrap();

        let mut prefix = [0u8; 4];
        first.read_exact(&mut prefix).unwrap();
        assert_eq!(&prefix, b"TEST");

        let mut complete = Vec::new();
        second.read_to_end(&mut complete).unwrap();
        assert_eq!(complete, payload_bytes);

        let mut suffix = Vec::new();
        first.read_to_end(&mut suffix).unwrap();
        assert_eq!(suffix, &payload_bytes[4..]);

        drop(first);
        drop(second);
        drop(payload);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verified_open_never_reopens_a_rebound_source_path() {
        let dir = temp_file("verified-path-rebind");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let path = dir.join("package.run");
        let retained_path = dir.join("verified-original.run");
        let original = b"TESTZIP\0original";
        let replacement = b"TESTZIP\0replacement";
        write_valid_single_file_sfx(&path, original);

        let payload = verify_and_open_sfx_payload(
            &path,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        assert_eq!(payload.source_path(), path);
        assert_eq!(payload.info().payload_bytes, original.len() as u64);

        fs::rename(&path, &retained_path).unwrap();
        write_valid_single_file_sfx(&path, replacement);

        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestZipFormat));
        let engine = Engine::new(registry);
        match engine.open_verified_sfx_with_control(
            &payload,
            &OpenOptions::default(),
            &ControlToken::default(),
        ) {
            Ok(mut reader) => {
                let mut opened = Vec::new();
                reader
                    .read_entry(&EntryPath::from_utf8("payload"))
                    .unwrap()
                    .read_to_end(&mut opened)
                    .unwrap();
                assert_eq!(opened, original);
                assert_ne!(opened, replacement);
            }
            Err(error) => assert!(error.is_input_changed(), "unexpected error: {error}"),
        }

        drop(payload);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verified_open_rejects_a_non_zip_payload_registry() {
        let path = temp_file("verified-non-zip-registry");
        write_valid_single_file_sfx(&path, b"not a registered zip");
        let payload = verify_and_open_sfx_payload(
            &path,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let error = match engine.open_verified_sfx_with_control(
            &payload,
            &OpenOptions::default(),
            &ControlToken::default(),
        ) {
            Ok(_) => panic!("non-ZIP payload registry unexpectedly opened"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::CorruptArchive(_)));

        drop(payload);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verified_payload_rejects_a_checksum_mismatch() {
        let path = temp_file("verified-checksum-mismatch");
        write_single_file_sfx(&path, b"TESTZIP\0damaged", crc32fast::hash(b"other"));

        let error = verify_and_open_sfx_payload(
            &path,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::CorruptArchive(_)));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verified_payload_api_rejects_bundle_layouts_explicitly() {
        let path = temp_file("verified-bundle-layout").with_extension("app");
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();

        let error = verify_and_open_sfx_payload(
            &path,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(_)));

        fs::remove_dir(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_payload_api_does_not_follow_a_symbolic_link() {
        use std::os::unix::fs::symlink;

        let target = temp_file("verified-symlink-target");
        let link = temp_file("verified-symlink-link");
        let _ = fs::remove_file(&target);
        let _ = fs::remove_file(&link);
        write_valid_single_file_sfx(&target, b"TESTZIP\0payload");
        symlink(&target, &link).unwrap();

        let error = verify_and_open_sfx_payload(
            &link,
            &ResourceOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(_)));

        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
    }

    #[test]
    fn inspect_rejects_tampered_footer_bounds() {
        let path = temp_file("tampered");
        let footer = SfxFooter {
            target: SfxTarget::Windows,
            payload_offset: 2,
            payload_bytes: 3,
            payload_crc32: 0,
        };
        let mut bytes = b"MZpayload".to_vec();
        bytes.extend_from_slice(&footer.encode());
        fs::write(&path, bytes).unwrap();

        let err = inspect_sfx(&path).unwrap_err();
        assert!(matches!(err, FormatError::CorruptArchive(_)));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn single_file_stage_syncs_again_after_copying_executable_permissions() {
        let events = std::cell::RefCell::new(Vec::new());

        finalize_single_file_stage_with(
            &mut || {
                events.borrow_mut().push("sync");
                Ok(())
            },
            &mut || {
                events.borrow_mut().push("verify");
                Ok(())
            },
            &mut || {
                events.borrow_mut().push("permissions");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            events.into_inner(),
            vec!["sync", "verify", "permissions", "sync"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn single_file_output_gains_owner_execute_from_data_template_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_file("data-template-permissions");
        let file = File::create(&path).unwrap();

        copy_executable_permissions(fs::Permissions::from_mode(0o644), &file).unwrap();

        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o744);
        drop(file);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn linux_template_data_validates_only_its_bounded_runtime() {
        let path = temp_file("linux-template-data-valid");
        let mut runtime = b"\x7fELF first-party runtime ".to_vec();
        runtime.extend_from_slice(&SFX_CLI_STUB_MARKER);
        runtime.extend_from_slice(b" trailing runtime bytes");
        write_linux_template_data(&path, &runtime);
        let options = SfxBuildOptions {
            target: SfxTarget::Linux,
            ..SfxBuildOptions::default()
        };

        let ValidatedSfxTemplate::SingleFile(mut template) =
            validate_sfx_template_for_build(&path, &options, &ControlToken::default()).unwrap()
        else {
            panic!("Linux data template must use the single-file layout");
        };
        assert_eq!(template.stub_offset, LINUX_TEMPLATE_DATA_HEADER_LEN);
        assert_eq!(template.stub_bytes, runtime.len() as u64);
        template
            .file
            .seek(SeekFrom::Start(template.stub_offset))
            .unwrap();
        let mut copied = Vec::new();
        template
            .file
            .take(template.stub_bytes)
            .read_to_end(&mut copied)
            .unwrap();
        assert_eq!(copied, runtime);

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn linux_template_data_rejects_trailing_or_modified_bytes() {
        let path = temp_file("linux-template-data-invalid");
        let mut runtime = b"\x7fELF runtime ".to_vec();
        runtime.extend_from_slice(&SFX_CLI_STUB_MARKER);
        let options = SfxBuildOptions {
            target: SfxTarget::Linux,
            ..SfxBuildOptions::default()
        };

        write_linux_template_data(&path, &runtime);
        let mut trailing = fs::OpenOptions::new().append(true).open(&path).unwrap();
        trailing.write_all(b"late").unwrap();
        drop(trailing);
        let error = validate_sfx_template(&path, &options, &ControlToken::default()).unwrap_err();
        assert!(
            matches!(error, FormatError::Unsupported(message) if message.contains("invalid length"))
        );

        write_linux_template_data(&path, &runtime);
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(LINUX_TEMPLATE_DATA_HEADER_LEN))
            .unwrap();
        file.write_all(b"X").unwrap();
        drop(file);
        let error = validate_sfx_template(&path, &options, &ControlToken::default()).unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(message) if message.contains("SHA-256")));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn linux_template_data_enforces_length_target_and_marker() {
        let path = temp_file("linux-template-data-contract");
        let linux_options = SfxBuildOptions {
            target: SfxTarget::Linux,
            ..SfxBuildOptions::default()
        };

        write_linux_template_data(&path, b"\x7fELF runtime without marker");
        let error =
            validate_sfx_template(&path, &linux_options, &ControlToken::default()).unwrap_err();
        assert!(
            matches!(error, FormatError::Unsupported(message) if message.contains("not a Squallz SFX-capable"))
        );

        let mut runtime = b"\x7fELF runtime ".to_vec();
        runtime.extend_from_slice(&SFX_CLI_STUB_MARKER);
        write_linux_template_data(&path, &runtime);
        let windows_options = SfxBuildOptions {
            target: SfxTarget::Windows,
            ..SfxBuildOptions::default()
        };
        let error =
            validate_sfx_template(&path, &windows_options, &ControlToken::default()).unwrap_err();
        assert!(
            matches!(error, FormatError::Unsupported(message) if message.contains("another target"))
        );

        let mut overflow = LINUX_TEMPLATE_DATA_MAGIC.to_vec();
        overflow.extend_from_slice(&u64::MAX.to_le_bytes());
        overflow.extend_from_slice(&[0u8; 32]);
        fs::write(&path, overflow).unwrap();
        let error =
            validate_sfx_template(&path, &linux_options, &ControlToken::default()).unwrap_err();
        assert!(
            matches!(error, FormatError::Unsupported(message) if message.contains("length overflow"))
        );

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn linux_template_data_build_writes_only_the_raw_runtime() {
        let dir = temp_file("linux-template-data-build");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let template_path = dir.join("sqz-sfx.stub");
        let archive_path = dir.join("payload.zip");
        let output_path = dir.join("package.run");
        let mut runtime = b"\x7fELF first-party runtime ".to_vec();
        runtime.extend_from_slice(&SFX_CLI_STUB_MARKER);
        write_linux_template_data(&template_path, &runtime);
        fs::write(&archive_path, b"TESTZIP\0payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestZipFormat));
        let engine = Engine::new(registry);
        let options = SfxBuildOptions {
            target: SfxTarget::Linux,
            ..SfxBuildOptions::default()
        };

        let report = engine
            .create_sfx(
                &template_path,
                &archive_path,
                &output_path,
                &options,
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();

        assert_eq!(report.stub_bytes, runtime.len() as u64);
        let output = fs::read(&output_path).unwrap();
        assert!(output.starts_with(&runtime));
        assert!(!output.starts_with(&LINUX_TEMPLATE_DATA_MAGIC));
        assert_eq!(
            inspect_sfx(&output_path).unwrap().unwrap().stub_bytes(),
            runtime.len() as u64
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_ne!(
                fs::metadata(&output_path).unwrap().permissions().mode() & 0o100,
                0
            );
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn linux_template_data_copy_rechecks_digest_before_publish() {
        let dir = temp_file("linux-template-data-copy-digest");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let template_path = dir.join("sqz-sfx.stub");
        let archive_path = dir.join("payload.zip");
        let output_path = dir.join("package.run");
        let mut runtime = vec![0u8; 24 * 1024];
        runtime[..4].copy_from_slice(b"\x7fELF");
        runtime[64..64 + SFX_CLI_STUB_MARKER.len()].copy_from_slice(&SFX_CLI_STUB_MARKER);
        write_linux_template_data(&template_path, &runtime);
        fs::write(&archive_path, b"TESTZIP\0payload").unwrap();
        let original_modified = fs::metadata(&template_path).unwrap().modified().unwrap();
        let progress = MutatingTemplateProgress {
            path: template_path.clone(),
            offset: LINUX_TEMPLATE_DATA_HEADER_LEN + 12 * 1024,
            original_modified,
            mutated: AtomicBool::new(false),
        };
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestZipFormat));
        let engine = Engine::new(registry);
        let options = SfxBuildOptions {
            target: SfxTarget::Linux,
            resources: ResourceOptions {
                memory_limit: Some(ResourceOptions::MIN_STREAM_BUFFER_BYTES),
                ..ResourceOptions::default()
            },
            ..SfxBuildOptions::default()
        };

        let error = engine
            .create_sfx(
                &template_path,
                &archive_path,
                &output_path,
                &options,
                &progress,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(progress.mutated.load(Ordering::SeqCst));
        assert!(error.is_input_changed(), "unexpected error: {error}");
        assert!(!output_path.exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_failure_discards_staging_before_publish_for_each_layout() {
        for layout in [SfxLayout::SingleFile, SfxLayout::MacosApp] {
            let suffix = layout.as_str();
            let dest = temp_file(&format!("cleanup-failure-dest-{suffix}"));
            let _ = fs::remove_file(&dest);
            let _ = fs::remove_dir_all(&dest);

            if layout == SfxLayout::MacosApp {
                fs::create_dir(&dest).unwrap();
                fs::write(dest.join("original"), b"keep").unwrap();
            } else {
                fs::write(&dest, b"keep").unwrap();
            }
            let (staged_path, _) = transaction::reserve_staged_path(&dest, layout).unwrap();
            if layout == SfxLayout::MacosApp {
                fs::write(staged_path.join("replacement"), b"new").unwrap();
            } else {
                fs::write(&staged_path, b"new").unwrap();
            }

            let staged = StagedSfx {
                path: staged_path.clone(),
                identity: transaction::path_identity(&staged_path).unwrap(),
                held_file: None,
                progress_total: 1,
                report: SfxBuildReport {
                    path: dest.clone(),
                    target: if layout == SfxLayout::MacosApp {
                        SfxTarget::Macos
                    } else {
                        SfxTarget::Windows
                    },
                    layout,
                    stub_bytes: 1,
                    payload_bytes: 1,
                    total_bytes: 1,
                    payload_crc32: 0,
                    payload_sha256: None,
                    requires_signing: true,
                    preserved_outputs: Vec::new(),
                },
            };
            let error = publish_staged_sfx_after_cleanup(
                staged,
                &dest,
                commit_policy_from_overwrite(true),
                &squallz_format_api::NoProgress,
                &ControlToken::default(),
                || {
                    Err(FormatError::Io(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected intermediate cleanup failure",
                    )))
                },
            )
            .unwrap_err();

            assert!(matches!(error, FormatError::Io(_)));
            assert!(!staged_path.exists());
            if layout == SfxLayout::MacosApp {
                assert_eq!(fs::read(dest.join("original")).unwrap(), b"keep");
            } else {
                assert_eq!(fs::read(&dest).unwrap(), b"keep");
            }
            remove_staged_path(&dest, layout).unwrap();
        }
    }

    #[test]
    fn publish_rejects_cross_layout_destinations_without_removing_them() {
        let single_dest = temp_file("single-destination-directory");
        let _ = fs::remove_dir_all(&single_dest);
        fs::create_dir(&single_dest).unwrap();
        fs::write(single_dest.join("keep"), b"directory must survive").unwrap();
        let (single_stage, _) =
            transaction::reserve_staged_path(&single_dest, SfxLayout::SingleFile).unwrap();
        fs::write(&single_stage, b"single replacement").unwrap();

        let error = publish_staged_sfx(
            test_staged_sfx(single_stage.clone(), SfxLayout::SingleFile),
            &single_dest,
            commit_policy_from_overwrite(true),
            &squallz_format_api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(_)));
        assert_eq!(
            fs::read(single_dest.join("keep")).unwrap(),
            b"directory must survive"
        );
        assert!(!single_stage.exists());

        let bundle_dest = temp_file("bundle-destination-file").with_extension("app");
        let _ = fs::remove_file(&bundle_dest);
        fs::write(&bundle_dest, b"file must survive").unwrap();
        let (bundle_stage, _) =
            transaction::reserve_staged_path(&bundle_dest, SfxLayout::MacosApp).unwrap();
        fs::write(bundle_stage.join("replacement"), b"bundle replacement").unwrap();

        let error = publish_staged_sfx(
            test_staged_sfx(bundle_stage.clone(), SfxLayout::MacosApp),
            &bundle_dest,
            commit_policy_from_overwrite(true),
            &squallz_format_api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(_)));
        assert_eq!(fs::read(&bundle_dest).unwrap(), b"file must survive");
        assert!(!bundle_stage.exists());

        fs::remove_dir_all(single_dest).unwrap();
        fs::remove_file(bundle_dest).unwrap();
    }

    #[test]
    fn no_overwrite_publish_is_atomic_and_preserves_existing_output() {
        let dest = temp_file("no-clobber-destination");
        let _ = fs::remove_file(&dest);
        fs::write(&dest, b"previous output").unwrap();
        let (staged_path, _) =
            transaction::reserve_staged_path(&dest, SfxLayout::SingleFile).unwrap();
        fs::write(&staged_path, b"new output").unwrap();

        let error = publish_staged_sfx(
            test_staged_sfx(staged_path.clone(), SfxLayout::SingleFile),
            &dest,
            commit_policy_from_overwrite(false),
            &squallz_format_api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert_eq!(fs::read(&dest).unwrap(), b"previous output");
        assert!(!staged_path.exists());
        fs::remove_file(dest).unwrap();
    }

    #[test]
    fn no_overwrite_single_file_publish_uses_atomic_rename_path() {
        let dest = temp_file("no-clobber-rename-destination");
        let _ = fs::remove_file(&dest);
        let (staged_path, _) =
            transaction::reserve_staged_path(&dest, SfxLayout::SingleFile).unwrap();
        fs::write(&staged_path, b"new output").unwrap();

        publish_staged_sfx(
            test_staged_sfx(staged_path.clone(), SfxLayout::SingleFile),
            &dest,
            commit_policy_from_overwrite(false),
            &squallz_format_api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), b"new output");
        assert!(!staged_path.exists());
        fs::remove_file(dest).unwrap();
    }

    #[test]
    fn overwrite_publish_reports_the_verified_previous_output() {
        let dir = temp_file("overwrite-dir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let dest = dir.join("output.exe");
        fs::write(&dest, b"previous output").unwrap();
        let (staged_path, _) =
            transaction::reserve_staged_path(&dest, SfxLayout::SingleFile).unwrap();
        fs::write(&staged_path, b"new output").unwrap();
        let mut staged = test_staged_sfx(staged_path.clone(), SfxLayout::SingleFile);
        staged.report.path.clone_from(&dest);

        let report = publish_staged_sfx(
            staged,
            &dest,
            commit_policy_from_overwrite(true),
            &squallz_format_api::NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), b"new output");
        assert_eq!(report.preserved_outputs.len(), 1);
        let backup = &report.preserved_outputs[0];
        assert_eq!(fs::read(backup).unwrap(), b"previous output");

        fs::remove_dir_all(dir).unwrap();
    }

    fn test_staged_sfx(path: PathBuf, layout: SfxLayout) -> StagedSfx {
        let identity = transaction::path_identity(&path).unwrap();
        StagedSfx {
            path: path.clone(),
            identity,
            held_file: None,
            progress_total: 1,
            report: SfxBuildReport {
                path,
                target: if layout == SfxLayout::MacosApp {
                    SfxTarget::Macos
                } else {
                    SfxTarget::Windows
                },
                layout,
                stub_bytes: 1,
                payload_bytes: 1,
                total_bytes: 1,
                payload_crc32: 0,
                payload_sha256: None,
                requires_signing: true,
                preserved_outputs: Vec::new(),
            },
        }
    }
}
