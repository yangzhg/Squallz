#![deny(unsafe_code)]
//! squallz-core: the engine layer.
//!
//! Exposes a format-agnostic high-level API ([`Engine`]) to the CLI/GUI,
//! hiding the registry, compound-format and split-volume details. core
//! never depends on a concrete format implementation.

pub use squallz_format_api as api;

mod archive_search;
mod checksum;
mod compound;
mod content_policy;
mod controlled_io;
mod convert;
mod create;
mod destination_guard;
mod duplicates;
mod extract_guard;
mod filesystem_identity;
mod filter;
mod inputs;
mod layout;
mod output_set;
mod presets;
mod queue;
mod sfx;
mod update;
mod volumes;

pub use archive_search::{
    fold_archive_search_path, fold_archive_search_query, rank_folded_archive_path,
    ArchivePathSearchRank,
};
pub use checksum::{
    ChecksumAlgorithm, ChecksumItem, ChecksumReport, ChecksumVerificationItem,
    ChecksumVerificationReport,
};
pub use content_policy::CreateContentPolicy;
pub use destination_guard::{
    create_destination_has_conflict, find_available_create_destination, inspect_create_destination,
    inspect_create_destination_with_progress, CreateArtifactKind, CreateCommitPolicy,
    CreateDestinationGuard, CreateDestinationState,
};
pub use duplicates::{DuplicateGroup, DuplicateScanReport};
pub use extract_guard::{build_extract_input_guard, ArchiveSourceState, ExtractInputGuard};
pub use filter::PathFilter;
pub use layout::{
    analyze_extract_layout, inspect_extract_space, ExtractPlan, ExtractScope, ExtractSpace,
    SmartLayout,
};
pub use output_set::{
    file_set_publication_pending, prepare_file_set_publication, recover_file_set_publication,
    PreparedFileSetPublication,
};
pub use presets::{
    ByteSize, CreateCompletionAction, CreateCredential, CreateDestination, CreateDestinationBase,
    CreateOutput, CreatePreset, EntryNameEncoding, ExistingOutputPolicy, ExtractCredential,
    ExtractDestination, ExtractDestinationBase, ExtractLayout, ExtractPreset, FormatId,
    FormatSpecificOptions, NamedPreset, PostSuccessAction, PresetBindings, PresetCompressionLevel,
    PresetDocument, PresetError, PresetId, PresetKind, PresetLabel, PresetStore,
    PresetValidationError, SfxTargetPolicy, SqzInnerFormat, SymlinkHandling, VolumeMode,
    BALANCED_CREATE_PRESET_ID, CROSS_PLATFORM_CREATE_PRESET_ID, MAX_SPLIT_SIZE_BYTES,
    MIN_SPLIT_SIZE_BYTES, PRESET_SCHEMA_VERSION, SMART_EXTRACT_PRESET_ID,
};
pub use queue::{
    Job, JobId, JobProgress, JobQueue, JobResources, JobState, QueueWaitReason, QueuedJobStatus,
};
pub use sfx::{
    default_sfx_extract_destination, discover_packaged_sfx_runtime, inspect_sfx,
    macos_sfx_bundle_for_executable, sfx_recovery_details, validate_sfx_template,
    verify_and_open_sfx_payload, verify_sfx_payload, SfxBuildOptions, SfxBuildReport, SfxInfo,
    SfxLayout, SfxRecoveryDetails, SfxTarget, VerifiedSfxBuildReport, VerifiedSfxPayload,
    SFX_CLI_STUB_MARKER, SFX_GUI_STUB_MARKER,
};
pub use volumes::{collect_volume_set, collect_volume_set_with_control, VolumeSet};

use std::fs::{self, File};
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use api::{
    ArchiveReader, ArchiveStructureStatus, ControlToken, CreateOptions, EntryMeta, EntryPath,
    EntryType, ExtractOptions, FormatError, FormatInfo, FormatRegistry, OpenOptions, ProgressSink,
    ReadSeek, SafetyLimits, TestReport, TestSummary, UpdateOp, TEST_PROBLEM_PREVIEW_LIMIT,
};
use compound::{decompress_factory, SingleFileArchiveReader};
use controlled_io::{controlled_result, ControlledReadSeek};
use volumes::MultiVolumeReader;

/// Engine: owns the registry and provides the high-level list/extract/
/// create/update/convert/test operations.
pub struct Engine {
    registry: FormatRegistry,
}

/// Integrity result that keeps payload verification separate from the
/// archive structure used to reach those payloads.
#[derive(Debug, Clone)]
pub struct ArchiveTestOutcome {
    pub summary: TestSummary,
    pub structure: ArchiveStructureStatus,
    payload_problem_count: u64,
}

impl ArchiveTestOutcome {
    /// Whether every readable payload entry passed verification. A recovered
    /// archive can satisfy this while still failing [`TestSummary::is_ok`]
    /// because its container structure is incomplete.
    pub const fn payload_is_ok(&self) -> bool {
        self.payload_problem_count == 0
    }

    /// Exact number of payload problems before the structural issue is added.
    pub const fn payload_problem_count(&self) -> u64 {
        self.payload_problem_count
    }

    /// Consumes the outcome and returns the complete integrity summary.
    pub fn into_summary(self) -> TestSummary {
        self.summary
    }
}

/// Preflight summary for local inputs before a create/update-add job starts.
///
/// This is intentionally an input-side estimate only: it never guesses the
/// compressed output size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CreateInputEstimate {
    pub input_count: usize,
    pub entries: usize,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CreateInputSummary {
    pub estimate: CreateInputEstimate,
    pub archive_budget_bytes: u64,
}

/// Conservative create budget derived from the current input manifest and
/// the selected output layout.
///
/// The byte fields are guardrails for free-space checks, not predictions of
/// the compressed size. Only [`CreateReport`] contains final output sizes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePlan {
    pub inputs: CreateInputEstimate,
    pub primary_output: PathBuf,
    /// Complete archive upper bound before optional volume splitting.
    pub archive_output_budget_bytes: u64,
    pub final_output_budget_bytes: u64,
    /// Upper-bound count of numbered data volumes for a split output. Recovery
    /// sidecars are not included.
    pub split_volume_count_budget: Option<u64>,
    /// Peak bytes needed on the destination filesystem while writing. The
    /// system-temp peak is folded into this field when both paths share a
    /// filesystem or their relationship cannot be established safely.
    pub workspace_budget_bytes: u64,
    /// Peak additional bytes needed below `std::env::temp_dir()` when it is on
    /// a different filesystem or that relationship cannot be established.
    /// Unknown relationships keep this gate in addition to the conservative
    /// destination fold above.
    pub system_temp_budget_bytes: u64,
}

/// Physical outputs committed by a successful archive creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateReport {
    /// File callers should open or reveal. Generic split archives point at
    /// `.001`; native sets point at the format's primary member.
    pub primary_output: PathBuf,
    /// Every newly committed physical output, including SQZ recovery sidecars.
    pub outputs: Vec<PathBuf>,
    /// Previous split-output files retained under transaction-owned backup
    /// names. Callers may offer an explicit recovery or cleanup action for
    /// these paths; unrelated historical backups are never included. This is
    /// a point-in-time report, not authorization for automatic path deletion.
    pub preserved_outputs: Vec<PathBuf>,
    /// Sum of the final output file sizes. Staging files are excluded.
    pub total_output_bytes: u64,
    /// Data-volume count for split output; recovery sidecars are excluded.
    pub split_volume_count: Option<usize>,
}

/// Content identity captured from the exact bytes consumed by archive
/// creation. Desktop source-cleanup verification uses this report without
/// reading every input once before compression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInputFingerprint {
    pub path: PathBuf,
    pub size: u64,
    pub blake3: [u8; 32],
}

/// Stable, totally ordered representation of an entry modification time.
///
/// The value is the signed nanosecond offset from the Unix epoch, giving
/// snapshots a stable, platform-neutral scalar representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CreateInputModifiedTime {
    pub unix_nanoseconds: i128,
}

impl From<std::time::SystemTime> for CreateInputModifiedTime {
    fn from(value: std::time::SystemTime) -> Self {
        let unix_nanoseconds = match value.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => {
                duration.as_secs() as i128 * 1_000_000_000 + i128::from(duration.subsec_nanos())
            }
            Err(error) => {
                let duration = error.duration();
                -(duration.as_secs() as i128 * 1_000_000_000 + i128::from(duration.subsec_nanos()))
            }
        };
        Self { unix_nanoseconds }
    }
}

/// One source entry accepted by the archive writer during verified creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInputManifestEntry {
    /// Absolute, normalized identity of the source entry. A symbolic link's
    /// final path component is preserved rather than resolved to its target.
    pub source_path: PathBuf,
    /// Path passed to the archive writer for this entry.
    pub archive_path: EntryPath,
    /// Entry type passed to the writer, including the exact link target.
    pub entry_type: EntryType,
    /// Uncompressed size passed to the writer.
    pub size: u64,
    /// Modification time passed to the writer in a stable representation.
    pub modified: Option<CreateInputModifiedTime>,
    /// Unix mode passed to the writer.
    pub unix_mode: Option<u32>,
    /// BLAKE3 of the exact bytes consumed by the writer for regular files.
    pub blake3: Option<[u8; 32]>,
}

/// Archive outputs plus the source entries accepted by the archive writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCreateReport {
    pub create: CreateReport,
    /// Regular-file fingerprints retained for API compatibility. New source
    /// verification should consume [`Self::manifest`].
    pub inputs: Vec<CreateInputFingerprint>,
    /// Complete writer-authoritative manifest in archive entry order.
    pub manifest: Vec<CreateInputManifestEntry>,
}

impl CreateInputEstimate {
    /// Conservative disk budget for create/update preflight.
    ///
    /// This is not a compressed output-size prediction. It reserves input
    /// bytes plus metadata/rewrite headroom so CLI and GUI use the same
    /// destination/temp-space guardrail before starting a write-heavy job.
    pub fn output_budget_bytes(self) -> u64 {
        const BASE_SLACK: u64 = 1024 * 1024;
        const ENTRY_SLACK: u64 = 1024;
        const INPUT_ROOT_SLACK: u64 = 4096;
        const CODEC_EXPANSION_DIVISOR: u64 = 16;

        let entry_slack = (self.entries as u64).saturating_mul(ENTRY_SLACK);
        let root_slack = (self.input_count as u64).saturating_mul(INPUT_ROOT_SLACK);
        let codec_slack = self.total_bytes.div_ceil(CODEC_EXPANSION_DIVISOR);
        let metadata_slack = BASE_SLACK
            .saturating_add(entry_slack)
            .saturating_add(root_slack)
            .saturating_add(codec_slack);
        self.total_bytes.saturating_add(metadata_slack)
    }
}

fn summarize_create_input_manifest<T: AsRef<inputs::InputItem>>(
    input_count: usize,
    items: &[T],
) -> CreateInputSummary {
    let mut estimate = CreateInputEstimate {
        input_count,
        ..CreateInputEstimate::default()
    };
    let mut archive_metadata_bytes = 0u64;
    for item in items {
        let item = item.as_ref();
        estimate.entries += 1;
        archive_metadata_bytes =
            archive_metadata_bytes.saturating_add(usize_to_u64(item.name.raw.len()));
        match item.entry_type {
            EntryType::File => {
                estimate.files += 1;
                estimate.total_bytes = estimate.total_bytes.saturating_add(item.size);
            }
            EntryType::Dir => estimate.directories += 1,
            EntryType::Symlink { ref target } => {
                estimate.symlinks += 1;
                archive_metadata_bytes =
                    archive_metadata_bytes.saturating_add(usize_to_u64(target.len()));
            }
            EntryType::Hardlink { ref target } => {
                archive_metadata_bytes =
                    archive_metadata_bytes.saturating_add(usize_to_u64(target.len()));
            }
            _ => {}
        }
    }
    create_input_summary(estimate, archive_metadata_bytes)
}

pub(crate) fn create_input_summary(
    estimate: CreateInputEstimate,
    archive_metadata_bytes: u64,
) -> CreateInputSummary {
    const ARCHIVE_METADATA_MULTIPLIER: u64 = 4;

    let archive_budget_bytes = estimate
        .output_budget_bytes()
        .saturating_add(archive_metadata_bytes.saturating_mul(ARCHIVE_METADATA_MULTIPLIER));
    CreateInputSummary {
        estimate,
        archive_budget_bytes,
    }
}

fn usize_to_u64(value: usize) -> u64 {
    let Ok(value) = u64::try_from(value) else {
        return u64::MAX;
    };
    value
}

/// Physical source of an archive: a single file or a `.001` volume set.
#[derive(Clone)]
pub(crate) enum Source {
    Single(PathBuf),
    Volumes { base: PathBuf, parts: VolumeSet },
}

impl Source {
    /// Resolves a path, expanding split volumes (any `x.zip.NNN` opens the
    /// whole gap-checked set).
    fn resolve_with_control(path: &Path, control: &ControlToken) -> Result<Self, FormatError> {
        control.checkpoint()?;
        match volumes::volume_base(path) {
            Some(base) => {
                let parts = collect_volume_set_with_control(path, control)?;
                control.checkpoint()?;
                Ok(Self::Volumes { base, parts })
            }
            None => {
                control.checkpoint()?;
                Ok(Self::Single(path.to_path_buf()))
            }
        }
    }

    fn open_stream_with_identity(
        &self,
        control: &ControlToken,
    ) -> Result<(Box<dyn ReadSeek>, Option<api::PhysicalFileIdentity>), FormatError> {
        control.checkpoint()?;
        match self {
            Self::Single(path) => {
                let file = File::open(path)?;
                control.checkpoint()?;
                #[cfg(any(unix, windows))]
                let identity = filesystem_identity::file_identity(&file)
                    .ok()
                    .map(filesystem_identity::PathIdentity::components)
                    .map(|(filesystem, entry)| api::PhysicalFileIdentity::new(filesystem, entry));
                #[cfg(not(any(unix, windows)))]
                let identity = None;
                control.checkpoint()?;
                Ok((Box::new(file), identity))
            }
            Self::Volumes { parts, .. } => Ok((
                Box::new(MultiVolumeReader::open_with_control(parts, control)?),
                None,
            )),
        }
    }

    /// Path used for naming and format detection (volume sets detect under
    /// their base name, `x.zip.001` → `x.zip`).
    fn display_path(&self) -> &Path {
        match self {
            Self::Single(path) => path,
            Self::Volumes { base, .. } => base,
        }
    }

    /// Returns the actual source path only when the stream represents one
    /// physical file. Generic `.001` sets deliberately have no single path.
    fn physical_path(&self) -> Option<&Path> {
        match self {
            Self::Single(path) => Some(path),
            Self::Volumes { .. } => None,
        }
    }

    fn generic_source_set_with_control(
        &self,
        control: &ControlToken,
    ) -> Result<Option<api::ArchiveSourceSet>, FormatError> {
        control.checkpoint()?;
        match self {
            Self::Single(_) => Ok(None),
            Self::Volumes { parts, .. } => {
                let mut members = Vec::with_capacity(parts.len());
                for part in parts.iter() {
                    control.checkpoint()?;
                    members.push(part.clone());
                }
                let source_set = api::ArchiveSourceSet::from_ordered_members(members)?;
                control.checkpoint()?;
                Ok(Some(source_set))
            }
        }
    }
}

struct OpenedArchive {
    format: String,
    reader: Box<dyn ArchiveReader>,
    generic_source_set: Option<api::ArchiveSourceSet>,
}

impl OpenedArchive {
    fn new(
        format: String,
        reader: Box<dyn ArchiveReader>,
        generic_source_set: Option<api::ArchiveSourceSet>,
    ) -> Self {
        Self {
            format,
            reader,
            generic_source_set,
        }
    }

    fn native_source_set(&self) -> Option<&api::ArchiveSourceSet> {
        self.reader.source_set()
    }

    fn effective_source_set(&self) -> Option<&api::ArchiveSourceSet> {
        self.native_source_set()
            .or(self.generic_source_set.as_ref())
    }

    fn inspect_source_state(
        &self,
        archive: &Path,
        control: &ControlToken,
    ) -> Result<ArchiveSourceState, FormatError> {
        self.reader.verify_source_set(control)?;
        let state = match self.effective_source_set() {
            Some(source_set) => {
                extract_guard::inspect_archive_source_state(source_set.members(), control)
            }
            None => extract_guard::inspect_archive_source_state(&[archive.to_path_buf()], control),
        }?;
        self.reader.verify_source_set(control)?;
        Ok(state)
    }
}

fn collect_reader_entries(
    reader: &mut dyn ArchiveReader,
    max_entries: u64,
    control: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    let mut entries = Vec::new();
    for entry in reader.entries() {
        control.checkpoint()?;
        push_reader_entry(&mut entries, entry?, max_entries)?;
    }
    control.checkpoint()?;
    Ok(entries)
}

fn collect_consumed_reader_entries(
    reader: Box<dyn ArchiveReader>,
    max_entries: u64,
    control: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    let mut entries = Vec::new();
    reader.consume_entries(&mut |entry| {
        control.checkpoint()?;
        push_reader_entry(&mut entries, entry, max_entries)
    })?;
    control.checkpoint()?;
    Ok(entries)
}

fn structure_problem(status: ArchiveStructureStatus) -> Option<&'static str> {
    match status {
        ArchiveStructureStatus::Complete => None,
        ArchiveStructureStatus::ZipLocalHeadersRecovered => Some(
            "ZIP central directory is missing or unreadable; entries were recovered from local headers",
        ),
    }
}

fn add_structure_problem_to_summary(summary: &mut TestSummary, status: ArchiveStructureStatus) {
    let Some(problem) = structure_problem(status) else {
        return;
    };
    summary.problems.total = summary.problems.total.saturating_add(1);
    if TEST_PROBLEM_PREVIEW_LIMIT == 0 {
        return;
    }
    if summary.problems.messages.len() >= TEST_PROBLEM_PREVIEW_LIMIT {
        summary
            .problems
            .messages
            .truncate(TEST_PROBLEM_PREVIEW_LIMIT.saturating_sub(1));
    }
    summary.problems.messages.insert(0, problem.to_owned());
}

fn add_structure_problem_to_report(report: &mut TestReport, status: ArchiveStructureStatus) {
    if let Some(problem) = structure_problem(status) {
        report.problems.insert(0, problem.to_owned());
    }
}

fn push_reader_entry(
    entries: &mut Vec<EntryMeta>,
    entry: EntryMeta,
    max_entries: u64,
) -> Result<(), FormatError> {
    let within_limit = match u64::try_from(entries.len()) {
        Ok(entry_index) => entry_index < max_entries,
        Err(_) => false,
    };
    if !within_limit {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "archive contains more than {max_entries} entries"
        )));
    }
    entries.push(entry);
    Ok(())
}

impl Engine {
    /// Builds an engine from the given registry (provided by
    /// squallz-formats).
    pub fn new(registry: FormatRegistry) -> Self {
        Self { registry }
    }

    /// Accesses the registry.
    pub fn registry(&self) -> &FormatRegistry {
        &self.registry
    }

    /// Discovers the physical files that form one archive source set.
    ///
    /// Generic `.001` byte streams come from core's gap-checked volume
    /// collector. Native container volumes are returned only after the format
    /// implementation validates their headers and stable file identities.
    /// Single-file archives return `None`.
    pub fn archive_source_set(
        &self,
        path: &Path,
    ) -> Result<Option<api::ArchiveSourceSet>, FormatError> {
        self.archive_source_set_with_control(path, &ControlToken::default())
    }

    /// Controlled variant of [`Engine::archive_source_set`].
    pub fn archive_source_set_with_control(
        &self,
        path: &Path,
        control: &ControlToken,
    ) -> Result<Option<api::ArchiveSourceSet>, FormatError> {
        control.checkpoint()?;
        let source = Source::resolve_with_control(path, control)?;
        if let Source::Volumes { parts, .. } = &source {
            let mut members = Vec::with_capacity(parts.len());
            for part in parts.iter() {
                control.checkpoint()?;
                members.push(part.clone());
            }
            let source_set = api::ArchiveSourceSet::from_ordered_members(members)?;
            control.checkpoint()?;
            return Ok(Some(source_set));
        }

        let (stream, source_identity) = source.open_stream_with_identity(control)?;
        let mut stream = ControlledReadSeek::boxed(stream, control);
        let (head, tail) = controlled_result(control, sniff_window(&mut *stream))?;
        let name = source
            .display_path()
            .file_name()
            .and_then(|value| value.to_str());
        let Some(source_path) = source.physical_path() else {
            control.checkpoint()?;
            return Ok(None);
        };
        match self.registry.detect(name, &head, &tail) {
            Some(api::Detected::Archive(format)) => controlled_result(
                control,
                format.probe_file_source_set_with_control(
                    source_path,
                    source_identity,
                    &mut *stream,
                    control,
                ),
            ),
            _ => {
                control.checkpoint()?;
                Ok(None)
            }
        }
    }

    /// Captures the stable physical state of the source members discoverable
    /// without running a decoder or reading the full payload.
    pub fn inspect_archive_source_state(
        &self,
        path: &Path,
        control: &ControlToken,
    ) -> Result<ArchiveSourceState, FormatError> {
        control.checkpoint()?;
        let members = match self.archive_source_set_with_control(path, control)? {
            Some(source_set) => source_set.members().to_vec(),
            None => vec![path.to_path_buf()],
        };
        extract_guard::inspect_archive_source_state(&members, control)
    }

    /// Resolves every physical source file that PAR2 protection must cover.
    ///
    /// Generic byte-split archives and native container volume sets use the
    /// same validated discovery path as archive opening. A regular single-file
    /// archive remains a one-element source set.
    pub fn recovery_protect_sources(&self, path: &Path) -> Result<Vec<PathBuf>, FormatError> {
        match self.archive_source_set(path)? {
            Some(source_set) => Ok(source_set.members().to_vec()),
            None => Ok(vec![path.to_path_buf()]),
        }
    }

    /// Opens an archive and returns a read handle. Generic byte-split sets
    /// (`x.zip.001`) and validated native container volumes are resolved
    /// transparently.
    pub fn open(
        &self,
        path: &Path,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        self.open_with_control(path, opts, &ControlToken::default())
    }

    /// Opens an archive while honoring pause and cancellation during format
    /// detection, metadata parsing and stream-backed staging.
    pub fn open_with_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        self.open_identified_with_control(path, opts, control)
            .map(|opened| opened.reader)
    }

    /// Opens a previously verified single-file SFX through its retained file
    /// handle. The executable path is never reopened, and only a ZIP format
    /// implementation may accept the bounded payload stream.
    pub fn open_verified_sfx_with_control(
        &self,
        payload: &VerifiedSfxPayload,
        opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        control.checkpoint()?;
        payload.verify_held_state()?;
        let mut stream = ControlledReadSeek::boxed(payload.open_reader()?, control);
        let (head, tail) = controlled_result(control, sniff_window(&mut *stream))?;
        let reader = match self.registry.detect(Some("payload.zip"), &head, &tail) {
            Some(api::Detected::Archive(format)) if format.id() == "zip" => {
                controlled_result(control, format.open_with_control(stream, opts, control))?
            }
            _ => {
                return Err(FormatError::CorruptArchive(
                    "SFX payload is not a supported ZIP archive".into(),
                ))
            }
        };
        payload.verify_held_state()?;
        control.checkpoint()?;
        Ok(reader)
    }

    fn open_identified_with_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<OpenedArchive, FormatError> {
        control.checkpoint()?;
        if let Some(stream) = controlled_result(control, sfx::open_sfx_payload(path))? {
            let mut stream = ControlledReadSeek::boxed(stream, control);
            let (head, tail) = controlled_result(control, sniff_window(&mut *stream))?;
            return match self.registry.detect(Some("payload.zip"), &head, &tail) {
                Some(api::Detected::Archive(format)) if format.id() == "zip" => {
                    let reader = controlled_result(
                        control,
                        format.open_with_control(stream, opts, control),
                    )?;
                    Ok(OpenedArchive::new("zip".to_owned(), reader, None))
                }
                _ => Err(FormatError::CorruptArchive(
                    "SFX payload is not a supported ZIP archive".into(),
                )),
            };
        }
        let source = Source::resolve_with_control(path, control)?;
        let generic_source_set = source.generic_source_set_with_control(control)?;
        let (stream, source_identity) = source.open_stream_with_identity(control)?;
        let mut stream = ControlledReadSeek::boxed(stream, control);
        let (head, tail) = controlled_result(control, sniff_window(&mut *stream))?;
        let name = source
            .display_path()
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        match self.registry.detect(name.as_deref(), &head, &tail) {
            Some(api::Detected::Archive(f)) => {
                let format = f.id().to_owned();
                let reader = match source.physical_path() {
                    Some(source_path) => f.open_file_with_control(
                        source_path,
                        source_identity,
                        stream,
                        opts,
                        control,
                    ),
                    None => f.open_with_control(stream, opts, control),
                };
                let reader = controlled_result(control, reader)?;
                Ok(OpenedArchive::new(format, reader, generic_source_set))
            }
            Some(api::Detected::Compressed {
                compressor,
                inner_archive,
            }) => match inner_archive {
                // Compound (x.tar.gz): the inner archive reads the
                // restartable decompressed stream — no temp file.
                Some(archive) => {
                    let format = format!("{}.{}", archive.id(), compressor.id());
                    let factory = decompress_factory(stream, Arc::clone(&compressor), control);
                    let reader = controlled_result(control, archive.open_stream(factory, opts))?;
                    Ok(OpenedArchive::new(format, reader, generic_source_set))
                }
                // Plain single stream (x.gz): single-entry virtual
                // archive named after the file minus the extension.
                None => {
                    let format = compressor.id().to_owned();
                    let mut hint = 0;
                    if let Some(size) = compressor.uncompressed_size_hint(&mut *stream) {
                        hint = size;
                    }
                    control.checkpoint()?;
                    let factory = decompress_factory(stream, Arc::clone(&compressor), control);
                    Ok(OpenedArchive::new(
                        format,
                        Box::new(SingleFileArchiveReader::new(
                            source.display_path(),
                            factory,
                            hint,
                        )),
                        generic_source_set,
                    ))
                }
            },
            None => Err(FormatError::Unsupported(format!(
                "unrecognized format: {}",
                path.display()
            ))),
        }
    }

    /// Lists entries and returns the format selected from the archive bytes.
    pub fn list_with_format(
        &self,
        path: &Path,
        opts: &OpenOptions,
    ) -> Result<(String, Vec<EntryMeta>), FormatError> {
        self.list_with_format_and_source_set(path, opts)
            .map(|(format, entries, _)| (format, entries))
    }

    /// Lists entries, returning the detected format and any native physical
    /// volume set retained by the opened reader.
    pub fn list_with_format_and_source_set(
        &self,
        path: &Path,
        opts: &OpenOptions,
    ) -> Result<(String, Vec<EntryMeta>, Option<api::ArchiveSourceSet>), FormatError> {
        self.list_with_format_and_source_set_with_control(path, opts, &ControlToken::default())
    }

    /// Lists entries and retains native source-set metadata while honoring
    /// pause and cancellation throughout opening and metadata iteration.
    pub fn list_with_format_and_source_set_with_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<(String, Vec<EntryMeta>, Option<api::ArchiveSourceSet>), FormatError> {
        self.list_with_format_and_source_set_with_entry_limit_and_control(
            path,
            opts,
            SafetyLimits::default().max_entries,
            control,
        )
    }

    /// Lists entries with an explicit metadata-count limit while retaining
    /// source-set metadata and honoring pause and cancellation.
    pub fn list_with_format_and_source_set_with_entry_limit_and_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        max_entries: u64,
        control: &ControlToken,
    ) -> Result<(String, Vec<EntryMeta>, Option<api::ArchiveSourceSet>), FormatError> {
        self.list_with_format_source_set_and_structure_with_entry_limit_and_control(
            path,
            opts,
            max_entries,
            control,
        )
        .map(|(format, entries, source_set, _)| (format, entries, source_set))
    }

    /// Lists entries while retaining the reader's explicit structural state.
    /// Compatibility list methods intentionally keep their existing return
    /// shapes and delegate here.
    pub fn list_with_format_source_set_and_structure_with_entry_limit_and_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        max_entries: u64,
        control: &ControlToken,
    ) -> Result<
        (
            String,
            Vec<EntryMeta>,
            Option<api::ArchiveSourceSet>,
            ArchiveStructureStatus,
        ),
        FormatError,
    > {
        control.checkpoint()?;
        let opened = self.open_identified_with_control(path, opts, control)?;
        let source_set = opened.native_source_set().cloned();
        let structure = opened.reader.structure_status();
        let OpenedArchive { format, reader, .. } = opened;
        let entries = collect_consumed_reader_entries(reader, max_entries, control)?;
        Ok((format, entries, source_set, structure))
    }

    /// Lists entries.
    pub fn list(&self, path: &Path, opts: &OpenOptions) -> Result<Vec<EntryMeta>, FormatError> {
        self.list_with_control(path, opts, &ControlToken::default())
    }

    /// Lists entries together with the structural state used to reach them.
    pub fn list_with_structure(
        &self,
        path: &Path,
        opts: &OpenOptions,
    ) -> Result<(Vec<EntryMeta>, ArchiveStructureStatus), FormatError> {
        self.list_with_format_source_set_and_structure_with_entry_limit_and_control(
            path,
            opts,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .map(|(_, entries, _, structure)| (entries, structure))
    }

    /// Lists entries while checking the shared pause/cancellation token
    /// between archive entries.
    pub fn list_with_control(
        &self,
        path: &Path,
        opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<Vec<EntryMeta>, FormatError> {
        self.list_with_format_and_source_set_with_control(path, opts, control)
            .map(|(_, entries, _)| entries)
    }

    /// Extracts everything or a selection of entries.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn extract(
        &self,
        path: &Path,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut reader = self.open_with_control(path, open_opts, ctl)?;
        controlled_result(
            ctl,
            reader.extract(dest, selection, extract_opts, progress, ctl),
        )
    }

    /// Extracts everything or a selection and returns completed per-entry
    /// outcome counts.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn extract_with_report(
        &self,
        path: &Path,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<api::ExtractReport, FormatError> {
        let mut reader = self.open_with_control(path, open_opts, ctl)?;
        controlled_result(
            ctl,
            reader.extract_with_report(dest, selection, extract_opts, progress, ctl),
        )
    }

    /// Opens an archive and builds a read-only extraction preflight. The
    /// physical archive path is used only to read entries; smart folder naming
    /// comes from `archive_display_path` so nested staging names never leak.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_extract(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        selection: Option<&[EntryPath]>,
        smart: bool,
        open_opts: &OpenOptions,
    ) -> Result<ExtractPlan, FormatError> {
        self.plan_extract_with_control(
            archive,
            requested_destination,
            archive_display_path,
            selection,
            smart,
            open_opts,
            &ControlToken::default(),
        )
    }

    /// Controlled variant of [`Engine::plan_extract`] used by interactive
    /// callers that must stop stale read-only work promptly.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_extract_with_control(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        selection: Option<&[EntryPath]>,
        smart: bool,
        open_opts: &OpenOptions,
        control: &ControlToken,
    ) -> Result<ExtractPlan, FormatError> {
        let entries = self.list_with_control(archive, open_opts, control)?;
        self.plan_extract_from_entries_with_control(
            requested_destination,
            archive_display_path,
            &entries,
            selection,
            smart,
            control,
        )
    }

    /// Builds a read-only extraction plan and an opaque binding to the exact
    /// source set retained by the opened reader.
    ///
    /// The selector can expand frontend paths against the reader's complete
    /// metadata list. Source members are checked before metadata iteration,
    /// after selection and again after destination planning, so the returned
    /// guard never authorizes a plan observed across a source-state change.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_extract_with_input_guard_controlled<F>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        control: &ControlToken,
        select: F,
    ) -> Result<(ExtractPlan, ExtractSpace, ExtractInputGuard), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
    {
        self.plan_extract_with_input_guard_and_entry_limit_controlled(
            archive,
            requested_destination,
            archive_display_path,
            smart,
            open_opts,
            SafetyLimits::default().max_entries,
            control,
            select,
        )
    }

    /// Builds a guarded extraction preflight with an explicit metadata-count
    /// limit. The limit is checked before another entry is retained in memory.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_extract_with_input_guard_and_entry_limit_controlled<F>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        max_entries: u64,
        control: &ControlToken,
        select: F,
    ) -> Result<(ExtractPlan, ExtractSpace, ExtractInputGuard), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
    {
        control.checkpoint()?;
        let mut opened = self.open_identified_with_control(archive, open_opts, control)?;
        let source_before = opened.inspect_source_state(archive, control)?;
        let entries = collect_reader_entries(&mut *opened.reader, max_entries, control)?;
        let selection = select(&entries, control)?;
        let source_after = opened.inspect_source_state(archive, control)?;
        if source_after != source_before {
            return Err(FormatError::input_changed());
        }
        let plan = self.plan_extract_from_entries_with_control(
            requested_destination,
            archive_display_path,
            &entries,
            selection.as_deref(),
            smart,
            control,
        )?;
        control.checkpoint()?;
        let space = inspect_extract_space(&plan)?;
        control.checkpoint()?;
        let source_final = opened.inspect_source_state(archive, control)?;
        if source_final != source_after {
            return Err(FormatError::input_changed());
        }
        let input_guard =
            build_extract_input_guard(source_final, &entries, selection.as_deref(), control)?;
        Ok((plan, space, input_guard))
    }

    /// Builds the same extraction preflight from an already listed archive.
    /// Layout always considers `entries` in full; selection affects only the
    /// scope and conflict snapshot.
    pub fn plan_extract_from_entries(
        &self,
        requested_destination: &Path,
        archive_display_path: &Path,
        entries: &[EntryMeta],
        selection: Option<&[EntryPath]>,
        smart: bool,
    ) -> Result<ExtractPlan, FormatError> {
        self.plan_extract_from_entries_with_control(
            requested_destination,
            archive_display_path,
            entries,
            selection,
            smart,
            &ControlToken::default(),
        )
    }

    /// Controlled variant of [`Engine::plan_extract_from_entries`].
    pub fn plan_extract_from_entries_with_control(
        &self,
        requested_destination: &Path,
        archive_display_path: &Path,
        entries: &[EntryMeta],
        selection: Option<&[EntryPath]>,
        smart: bool,
        control: &ControlToken,
    ) -> Result<ExtractPlan, FormatError> {
        let archive_folder_name = self.archive_stem(archive_display_path);
        layout::build_extract_plan(
            requested_destination,
            &archive_folder_name,
            entries,
            selection,
            smart,
            control,
        )
    }

    /// Lists, plans, and extracts through one opened archive reader. The
    /// selector receives the complete entry list and can derive an exact
    /// selection for include filters or directory expansion. `validate_plan`
    /// runs after planning and before the reader can create the destination,
    /// allowing queued callers to reject a stale preflight. Keeping the reader
    /// alive avoids reopening password-protected, split, nested, or streamed
    /// archives between the worker's preflight and extraction pass.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_and_extract_with_report<F, V>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        select: F,
        validate_plan: V,
    ) -> Result<(ExtractPlan, api::ExtractReport), FormatError>
    where
        F: FnOnce(&[EntryMeta]) -> Option<Vec<EntryPath>>,
        V: FnOnce(&ExtractPlan) -> Result<(), FormatError>,
    {
        self.plan_and_extract_with_report_controlled(
            archive,
            requested_destination,
            archive_display_path,
            smart,
            open_opts,
            extract_opts,
            progress,
            ctl,
            |entries, _| Ok(select(entries)),
            validate_plan,
        )
    }

    /// Controlled-selection variant of
    /// [`Engine::plan_and_extract_with_report`]. The selector can checkpoint
    /// while expanding a large path filter without changing the established
    /// facade used by existing callers.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_and_extract_with_report_controlled<F, V>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        select: F,
        validate_plan: V,
    ) -> Result<(ExtractPlan, api::ExtractReport), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
        V: FnOnce(&ExtractPlan) -> Result<(), FormatError>,
    {
        self.plan_and_extract_with_report_and_structure_controlled(
            archive,
            requested_destination,
            archive_display_path,
            smart,
            open_opts,
            extract_opts,
            progress,
            ctl,
            select,
            validate_plan,
        )
        .map(|(plan, report, _)| (plan, report))
    }

    /// Controlled extraction that also reports the structure of the reader
    /// used for the operation. This does not reopen or rescan the archive.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_and_extract_with_report_and_structure_controlled<F, V>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        select: F,
        validate_plan: V,
    ) -> Result<(ExtractPlan, api::ExtractReport, ArchiveStructureStatus), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
        V: FnOnce(&ExtractPlan) -> Result<(), FormatError>,
    {
        self.plan_and_extract_with_report_guarded_and_structure_controlled(
            archive,
            requested_destination,
            archive_display_path,
            smart,
            open_opts,
            extract_opts,
            progress,
            ctl,
            None,
            select,
            validate_plan,
        )
    }

    /// Controlled extraction with an optional input guard from
    /// [`Engine::plan_extract_with_input_guard_controlled`].
    ///
    /// Guarded callers compare complete entry metadata, selected scope and the
    /// actual native or generic source set retained by this opened reader
    /// before planning. Source state is checked once more after destination
    /// validation and space inspection, immediately before extraction.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn plan_and_extract_with_report_guarded_controlled<F, V>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        expected_input_guard: Option<ExtractInputGuard>,
        select: F,
        validate_plan: V,
    ) -> Result<(ExtractPlan, api::ExtractReport), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
        V: FnOnce(&ExtractPlan) -> Result<(), FormatError>,
    {
        self.plan_and_extract_with_report_guarded_and_structure_controlled(
            archive,
            requested_destination,
            archive_display_path,
            smart,
            open_opts,
            extract_opts,
            progress,
            ctl,
            expected_input_guard,
            select,
            validate_plan,
        )
        .map(|(plan, report, _)| (plan, report))
    }

    /// Guarded extraction that also reports the structure of the same reader
    /// used for guard validation, planning, and extraction.
    #[allow(clippy::too_many_arguments)] // shared guarded extraction implementation
    pub fn plan_and_extract_with_report_guarded_and_structure_controlled<F, V>(
        &self,
        archive: &Path,
        requested_destination: &Path,
        archive_display_path: &Path,
        smart: bool,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        expected_input_guard: Option<ExtractInputGuard>,
        select: F,
        validate_plan: V,
    ) -> Result<(ExtractPlan, api::ExtractReport, ArchiveStructureStatus), FormatError>
    where
        F: FnOnce(&[EntryMeta], &ControlToken) -> Result<Option<Vec<EntryPath>>, FormatError>,
        V: FnOnce(&ExtractPlan) -> Result<(), FormatError>,
    {
        ctl.checkpoint()?;
        let mut opened = self.open_identified_with_control(archive, open_opts, ctl)?;
        let structure = opened.reader.structure_status();
        let source_before = expected_input_guard
            .map(|_| opened.inspect_source_state(archive, ctl))
            .transpose()?;
        let entries =
            collect_reader_entries(&mut *opened.reader, extract_opts.limits.max_entries, ctl)?;
        let selection = select(&entries, ctl)?;
        ctl.checkpoint()?;
        let source_after = source_before
            .map(|before| {
                let after = opened.inspect_source_state(archive, ctl)?;
                if after != before {
                    return Err(FormatError::input_changed());
                }
                Ok(after)
            })
            .transpose()?;
        if let (Some(expected), Some(source)) = (expected_input_guard, source_after) {
            let observed = build_extract_input_guard(source, &entries, selection.as_deref(), ctl)?;
            if observed != expected {
                return Err(FormatError::input_changed());
            }
        }
        let plan = self.plan_extract_from_entries_with_control(
            requested_destination,
            archive_display_path,
            &entries,
            selection.as_deref(),
            smart,
            ctl,
        )?;
        ctl.checkpoint()?;
        validate_plan(&plan)?;
        ctl.checkpoint()?;
        if !inspect_extract_space(&plan)?.is_sufficient() {
            return Err(FormatError::DiskFull);
        }
        ctl.checkpoint()?;
        if let Some(previous) = source_after {
            let current = opened.inspect_source_state(archive, ctl)?;
            if current != previous {
                return Err(FormatError::input_changed());
            }
        }
        drop(entries);
        let report = controlled_result(
            ctl,
            opened.reader.extract_with_report(
                &plan.destination,
                selection.as_deref(),
                extract_opts,
                progress,
                ctl,
            ),
        )?;
        Ok((plan, report, structure))
    }

    /// Integrity test.
    pub fn test(
        &self,
        path: &Path,
        opts: &OpenOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut reader = self.open_with_control(path, opts, ctl)?;
        let structure = reader.structure_status();
        let mut report = controlled_result(ctl, reader.test(progress, ctl))?;
        add_structure_problem_to_report(&mut report, structure);
        Ok(report)
    }

    /// Integrity test with an exact problem count and bounded diagnostic
    /// preview.
    pub fn test_summary(
        &self,
        path: &Path,
        opts: &OpenOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        self.test_summary_with_structure(path, opts, progress, ctl)
            .map(ArchiveTestOutcome::into_summary)
    }

    /// Integrity test that retains typed structure status and the payload-only
    /// problem count. ZIP index repair uses this to accept readable local
    /// payloads without pretending the damaged source archive is complete.
    pub fn test_summary_with_structure(
        &self,
        path: &Path,
        opts: &OpenOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ArchiveTestOutcome, FormatError> {
        let mut reader = self.open_with_control(path, opts, ctl)?;
        let structure = reader.structure_status();
        let mut summary = controlled_result(ctl, reader.test_summary(progress, ctl))?;
        let payload_problem_count = summary.problems.total;
        add_structure_problem_to_summary(&mut summary, structure);
        Ok(ArchiveTestOutcome {
            summary,
            structure,
            payload_problem_count,
        })
    }

    /// Creates an archive. The output format is chosen by the extension of
    /// `dest` (compound suffixes like `.tar.gz` / aliases like `.tgz`
    /// included); `opts.excludes` globs prune the inputs. With
    /// `opts.split_size`, [`SplitOutputMode::Generic`] writes `dest.001`,
    /// `dest.002`, ... byte-split volumes. Formats that advertise native
    /// volume support can instead use [`SplitOutputMode::Native`]; ZIP then
    /// writes `.z01`, `.z02`, ... with the final `.zip` as its primary
    /// member. Call [`Engine::create_with_report`] when the caller must
    /// surface retained backups from a split replacement.
    pub fn create(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.create_with_report(dest, inputs, opts, progress, ctl)
            .map(drop)
    }

    /// Creates an archive and returns the newly committed outputs together
    /// with any transaction-owned backups retained during split replacement.
    /// [`Engine::create`] discards this report.
    pub fn create_with_report(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<CreateReport, FormatError> {
        create::create(self, dest, inputs, opts, progress, ctl)
    }

    /// Creates an archive using an explicit final publication policy.
    /// `ReplaceIfUnchanged` binds replacement to the destination state
    /// returned by [`inspect_create_destination`].
    pub fn create_with_report_policy(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<CreateReport, FormatError> {
        create::create_report_with_policy(self, dest, inputs, opts, policy, progress, ctl)
    }

    /// Creates an archive without replacing an output that appears before
    /// the final commit. Split creation rejects any existing member of the
    /// managed output family while holding the split commit lock.
    pub fn create_with_report_no_replace(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<CreateReport, FormatError> {
        create::create_no_replace(self, dest, inputs, opts, progress, ctl)
    }

    /// Creates an archive and reports every source entry accepted by the
    /// writer. Regular-file hashes come from the writer's input stream, so no
    /// extra content pass is performed.
    pub fn create_with_verification(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<VerifiedCreateReport, FormatError> {
        create::create_verified(
            self,
            dest,
            inputs,
            opts,
            progress,
            ctl,
            CreateCommitPolicy::ReplaceExisting,
        )
    }

    /// Verified creation using an explicit final publication policy.
    pub fn create_with_verification_policy(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<VerifiedCreateReport, FormatError> {
        create::create_verified(self, dest, inputs, opts, progress, ctl, policy)
    }

    /// Verified creation with commit-time no-replace semantics.
    pub fn create_with_verification_no_replace(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<VerifiedCreateReport, FormatError> {
        create::create_verified(
            self,
            dest,
            inputs,
            opts,
            progress,
            ctl,
            CreateCommitPolicy::NoReplace,
        )
    }

    /// Builds a conservative output/workspace plan using the same input and
    /// output-family semantics as archive creation.
    pub fn plan_create(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
    ) -> Result<CreatePlan, FormatError> {
        self.plan_create_with_progress(dest, inputs, opts, |_count, _path| {})
    }

    /// Progress-reporting variant of [`Engine::plan_create`].
    pub fn plan_create_with_progress(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: impl FnMut(usize, &str),
    ) -> Result<CreatePlan, FormatError> {
        create::plan_create_with_progress(self, dest, inputs, opts, progress)
    }

    /// Builds a conservative output/workspace plan for an archive conversion.
    ///
    /// Source metadata is read without extracting entry contents. The returned
    /// byte fields are free-space guardrails, not compressed-size predictions.
    pub fn plan_convert(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
    ) -> Result<CreatePlan, FormatError> {
        self.plan_convert_with_control(src, dest, open_opts, create_opts, &ControlToken::default())
    }

    /// Controlled variant of [`Engine::plan_convert`] for interactive callers.
    pub fn plan_convert_with_control(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        ctl: &ControlToken,
    ) -> Result<CreatePlan, FormatError> {
        let detect_name = dest
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
        create::validate_create_target_name(self, detect_name, create_opts)?;
        let entries = self.list_with_control(src, open_opts, ctl)?;
        convert::plan_convert_from_entries(self, dest, &entries, create_opts)
    }

    /// Walks local inputs with the same exclude semantics as archive creation
    /// and returns a non-compression estimate for the UI/preflight layer.
    pub fn estimate_create_inputs(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
    ) -> Result<CreateInputEstimate, FormatError> {
        self.estimate_create_inputs_with_progress(inputs, excludes, |_count, _path| {})
    }

    /// Same as [`Engine::estimate_create_inputs`], reporting each included
    /// filesystem candidate before overlapping roots are merged. The final
    /// estimate can therefore contain fewer entries than the last scan count.
    pub fn estimate_create_inputs_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        mut progress: impl FnMut(usize, &str),
    ) -> Result<CreateInputEstimate, FormatError> {
        let filter = PathFilter::new(excludes)?;
        let items = inputs::collect_inputs_with_progress(inputs, &filter, |count, path| {
            progress(count, &path.display);
        })?;
        Ok(summarize_create_input_manifest(inputs.len(), &items).estimate)
    }

    /// Estimates inputs while applying the output-family exclusions used by
    /// archive creation. This keeps an existing destination, split volumes,
    /// recovery sidecars, or SFX bundle out of a source-directory estimate.
    pub fn estimate_create_inputs_for_output(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        output: &Path,
        split_output: bool,
    ) -> Result<CreateInputEstimate, FormatError> {
        self.estimate_create_inputs_for_output_with_progress(
            inputs,
            excludes,
            output,
            split_output,
            |_count, _path| {},
        )
    }

    /// Output-aware variant of [`Engine::estimate_create_inputs_with_progress`].
    pub fn estimate_create_inputs_for_output_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        output: &Path,
        split_output: bool,
        progress: impl FnMut(usize, &str),
    ) -> Result<CreateInputEstimate, FormatError> {
        Ok(self
            .estimate_create_input_summary_for_output_with_progress(
                inputs,
                excludes,
                output,
                split_output,
                progress,
            )?
            .estimate)
    }

    pub(crate) fn estimate_create_input_summary_for_output_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        output: &Path,
        split_output: bool,
        mut progress: impl FnMut(usize, &str),
    ) -> Result<CreateInputSummary, FormatError> {
        let filter = PathFilter::new(excludes)?;
        let items = create::collect_inputs_for_output_estimate(
            inputs,
            &filter,
            output,
            split_output,
            |count, path| progress(count, &path.display),
        )?;
        Ok(summarize_create_input_manifest(inputs.len(), &items))
    }

    /// Finds duplicate local files with the same input walking and exclude
    /// semantics used by archive creation.
    pub fn find_duplicate_files(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        min_size: u64,
    ) -> Result<DuplicateScanReport, FormatError> {
        duplicates::find_duplicates(inputs, excludes, min_size)
    }

    /// Computes checksums for local files and recursively scanned folders,
    /// using the same exclude semantics as archive creation.
    pub fn checksum_files(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        algorithm: ChecksumAlgorithm,
    ) -> Result<ChecksumReport, FormatError> {
        checksum::checksum_files(inputs, excludes, algorithm)
    }

    /// Computes checksums with chunk-level progress and cancellation.
    pub fn checksum_files_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        algorithm: ChecksumAlgorithm,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ChecksumReport, FormatError> {
        checksum::checksum_files_with_progress(inputs, excludes, algorithm, progress, ctl)
    }

    /// Verifies a `sha256sum`-style checksum manifest. Relative paths are
    /// resolved from the manifest file's parent directory.
    pub fn verify_checksum_manifest(
        &self,
        manifest: &Path,
        algorithm: ChecksumAlgorithm,
    ) -> Result<ChecksumVerificationReport, FormatError> {
        checksum::verify_checksum_manifest(manifest, algorithm)
    }

    /// Verifies a checksum manifest with chunk-level progress and cancellation.
    pub fn verify_checksum_manifest_with_progress(
        &self,
        manifest: &Path,
        algorithm: ChecksumAlgorithm,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ChecksumVerificationReport, FormatError> {
        checksum::verify_checksum_manifest_with_progress(manifest, algorithm, progress, ctl)
    }

    /// Applies append/delete/rename operations to an existing archive
    /// (formats with `can_update`). Stream-rewrite formats use the core's
    /// durable target transaction; legacy formats retain their own update
    /// implementation.
    pub fn update(
        &self,
        path: &Path,
        ops: &[UpdateOp],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| FormatError::Unsupported("invalid archive file name".into()))?;
        if api::split_volume_name(name).is_some() {
            return Err(FormatError::Unsupported(
                "updating split volume sets is not supported".into(),
            ));
        }
        match self.registry.detect_by_name(name) {
            Some(api::Detected::Archive(f)) => {
                if !f.capabilities().can_update {
                    return Err(FormatError::Unsupported(format!(
                        "format {} does not support updating",
                        f.id()
                    )));
                }
                if f.supports_update_rewrite() {
                    update::run_update_rewrite(f.as_ref(), path, ops, opts, progress, ctl)
                } else if f.accepts_prepared_update_additions() {
                    let mut additions = update::prepare_additions(ops, opts, progress, ctl)?;
                    f.update_with_prepared_additions(path, ops, &mut additions, opts, progress, ctl)
                } else {
                    f.update(path, ops, opts, progress, ctl)
                }
            }
            _ => Err(FormatError::Unsupported(format!(
                "updating this format is not supported: {name}"
            ))),
        }
    }

    /// Converts an archive into another format, streaming entry by entry
    /// (no extraction to disk). `open_opts` applies to the source and
    /// `create_opts` (password and level) to the destination.
    ///
    /// Split conversion must use [`Self::convert_with_report`], because a
    /// replacement can retain previous volumes that the caller must show to
    /// the user.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "split conversion requires convert_with_report so preserved previous outputs cannot be hidden"
                    .into(),
            ));
        }
        self.convert_with_report(src, dest, open_opts, create_opts, progress, ctl)
            .map(drop)
    }

    /// Converts an archive and returns every committed destination artifact.
    /// Split replacements report transaction-owned previous volumes through
    /// [`CreateReport::preserved_outputs`]; callers must surface those exact
    /// paths before offering any cleanup action.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert_with_report(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<CreateReport, FormatError> {
        convert::convert(self, src, dest, open_opts, create_opts, None, progress, ctl)
    }

    /// Converts an archive using an explicit destination publication policy.
    ///
    /// [`CreateCommitPolicy::NoReplace`] refuses an occupied destination,
    /// while [`CreateCommitPolicy::ReplaceIfUnchanged`] only replaces the
    /// exact destination state captured before the caller asked for consent.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert_with_policy(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "split conversion requires convert_with_report_policy so preserved previous outputs cannot be hidden"
                    .into(),
            ));
        }
        self.convert_with_report_policy(
            src,
            dest,
            open_opts,
            create_opts,
            commit_policy,
            progress,
            ctl,
        )
        .map(drop)
    }

    /// Converts an archive using an explicit destination publication policy
    /// and returns every committed destination artifact.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert_with_report_policy(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        commit_policy: CreateCommitPolicy,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<CreateReport, FormatError> {
        convert::convert(
            self,
            src,
            dest,
            open_opts,
            create_opts,
            Some(commit_policy),
            progress,
            ctl,
        )
    }

    /// Converts an archive and reports whether `src` and `dest` name the same
    /// existing file. Conversion always commits from a same-directory staging
    /// file with one atomic replacement.
    ///
    /// Split output is always rejected. Use [`Self::convert_with_report`] for
    /// split conversion so every committed and preserved artifact is visible
    /// to the caller.
    ///
    /// Returns `true` when the destination was replaced in place.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert_with_atomic_replace(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<bool, FormatError> {
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "convert_with_atomic_replace does not support split output; use convert_with_report"
                    .into(),
            ));
        }
        let in_place = same_existing_path(src, dest);
        self.convert(src, dest, open_opts, create_opts, progress, ctl)?;
        Ok(in_place)
    }

    /// Folder-name stem of an archive path: the file name minus split
    /// suffix and recognized format extensions (`backup.tar.gz` →
    /// `backup`). Used by smart extraction to name the wrapping folder.
    pub fn archive_stem(&self, path: &Path) -> String {
        let name = match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => String::new(),
        };
        let stem = self.registry.display_stem(&name);
        if stem.is_empty() {
            "extracted".to_string()
        } else {
            stem
        }
    }

    /// All supported formats (for `sqz info` / the GUI).
    pub fn supported_formats(&self) -> Vec<FormatInfo> {
        self.registry.formats()
    }
}

pub(crate) fn same_existing_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    matches!(
        (fs::canonicalize(a), fs::canonicalize(b)),
        (Ok(a), Ok(b)) if a == b
    )
}

pub(crate) fn same_path_entry(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if matches!(
        (std::path::absolute(a), std::path::absolute(b)),
        (Ok(a), Ok(b)) if a == b
    ) {
        return true;
    }
    let (Some(a_name), Some(b_name)) = (a.file_name(), b.file_name()) else {
        return false;
    };
    if !entry_names_may_alias(a_name, b_name) {
        return false;
    }
    let a_parent = a
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let b_parent = b
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let (Ok(a_parent), Ok(b_parent)) = (fs::canonicalize(a_parent), fs::canonicalize(b_parent))
    else {
        return false;
    };
    if a_parent != b_parent {
        return false;
    }
    if a_name == b_name {
        return true;
    }
    if fs::symlink_metadata(a).is_err() || fs::symlink_metadata(b).is_err() {
        return false;
    }
    if !same_directory_entry_metadata(a, b) {
        return false;
    }
    let Ok(entries) = fs::read_dir(&a_parent) else {
        return false;
    };
    let mut saw_a_name = false;
    let mut saw_b_name = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        saw_a_name |= name == a_name;
        saw_b_name |= name == b_name;
    }
    if saw_a_name && saw_b_name {
        return false;
    }
    true
}

pub(crate) fn entry_names_may_alias(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if a == b {
        return true;
    }
    let (Some(a), Some(b)) = (a.to_str(), b.to_str()) else {
        return true;
    };
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    true
}

#[cfg(unix)]
fn same_directory_entry_metadata(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    matches!(
        (fs::symlink_metadata(a), fs::symlink_metadata(b)),
        (Ok(a), Ok(b)) if a.dev() == b.dev() && a.ino() == b.ino()
    )
}

#[cfg(not(unix))]
fn same_directory_entry_metadata(a: &Path, b: &Path) -> bool {
    matches!(
        (fs::canonicalize(a), fs::canonicalize(b)),
        (Ok(a), Ok(b)) if a == b
    )
}

fn sibling_temp_path(dest: &Path, purpose: &str) -> Result<PathBuf, FormatError> {
    let parent = match dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    };
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy())
        .ok_or_else(|| FormatError::Unsupported("destination path has no file name".into()))?;
    for attempt in 0..1000u32 {
        let candidate = parent.join(format!(
            ".{name}.{purpose}-{}-{attempt}.tmp.{name}",
            std::process::id()
        ));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not allocate a temporary path next to {}",
        dest.display()
    )))
}

pub(crate) struct ReservedTempFile {
    pub(crate) path: PathBuf,
    pub(crate) file: File,
    pub(crate) identity: filesystem_identity::PathIdentity,
}

pub(crate) fn reserve_bound_sibling_temp_file(
    dest: &Path,
    purpose: &str,
) -> Result<ReservedTempFile, FormatError> {
    for _ in 0..1000u32 {
        let candidate = sibling_temp_path(dest, purpose)?;
        let mut options = fs::OpenOptions::new();
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
        match options.open(&candidate) {
            Ok(file) => {
                let identity = filesystem_identity::file_identity(&file).map_err(|error| {
                    FormatError::from(io::Error::new(
                        error.kind(),
                        format!(
                            "{error}; temporary output ownership could not be verified and the path was left untouched: {}",
                            candidate.display()
                        ),
                    ))
                })?;
                if filesystem_identity::path_identity(&candidate).ok() != Some(identity) {
                    return Err(FormatError::Io(io::Error::other(format!(
                        "temporary output changed while it was reserved and was left untouched: {}",
                        candidate.display()
                    ))));
                }
                return Ok(ReservedTempFile {
                    path: candidate,
                    file,
                    identity,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve temporary output next to {}",
        dest.display()
    )))
}

pub(crate) fn remove_bound_temp_file(
    path: &Path,
    file: &File,
    expected: filesystem_identity::PathIdentity,
) -> Result<(), FormatError> {
    if filesystem_identity::file_identity(file)? != expected {
        return Err(FormatError::Io(io::Error::other(format!(
            "temporary output handle identity changed and the path was left untouched: {}",
            path.display()
        ))));
    }
    match filesystem_identity::path_identity(path) {
        Ok(identity) if identity == expected => {
            let quarantine = sibling_temp_path(path, "cleanup")?;
            move_path_no_replace(path, &quarantine)?;
            if filesystem_identity::file_identity(file)? != expected
                || filesystem_identity::path_identity(&quarantine).ok() != Some(expected)
            {
                return Err(FormatError::Io(io::Error::other(format!(
                    "a competing temporary output was isolated and left untouched for recovery: {}",
                    quarantine.display()
                ))));
            }
            fs::remove_file(&quarantine).map_err(|error| {
                FormatError::from(io::Error::new(
                    error.kind(),
                    format!(
                        "could not remove isolated temporary output {}: {error}",
                        quarantine.display()
                    ),
                ))
            })?;
            open_parent_directory(path)?.sync_all().map_err(|error| {
                FormatError::from(io::Error::new(
                    error.kind(),
                    format!(
                        "could not synchronize cleanup of isolated temporary output {}: {error}",
                        quarantine.display()
                    ),
                ))
            })?;
            Ok(())
        }
        Ok(_) => Err(FormatError::Io(io::Error::other(format!(
            "temporary output identity changed and the competing path was left untouched: {}",
            path.display()
        )))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub(crate) fn replace_file(tmp: &Path, dest: &Path) -> Result<(), FormatError> {
    let parent = open_parent_directory(dest)?;
    replace_file_with(
        tmp,
        dest,
        &mut |from, to| atomic_replace_file(from, to),
        &mut || parent.sync_all(),
    )
}

#[cfg(unix)]
fn atomic_replace_file(src: &Path, dest: &Path) -> io::Result<()> {
    fs::rename(src, dest)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn atomic_replace_file(src: &Path, dest: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains a null character",
            ));
        }
        value.push(0);
        Ok(value)
    }

    let src = wide_path(src)?;
    let dest = wide_path(dest)?;
    // SAFETY: both buffers remain valid null-terminated UTF-16 strings for
    // this synchronous call. COPY_ALLOWED is deliberately omitted so the
    // operation cannot fall back to a non-atomic copy/delete sequence.
    if unsafe {
        MoveFileExW(
            src.as_ptr(),
            dest.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn atomic_replace_file(_src: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic file replacement is unavailable on this platform",
    ))
}

pub(crate) fn open_parent_directory(path: &Path) -> io::Result<File> {
    open_directory(parent_directory(path))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Returns the stable physical identity of the current no-follow path entry.
///
/// Callers that make a security decision from this value should also retain an
/// open handle and compare it with [`physical_file_identity`] before mutating
/// the path.
pub fn physical_path_identity(path: &Path) -> io::Result<api::PhysicalFileIdentity> {
    filesystem_identity::path_identity(path)
        .map(filesystem_identity::PathIdentity::components)
        .map(|(filesystem, entry)| api::PhysicalFileIdentity::new(filesystem, entry))
}

/// Returns the stable physical identity of an already-opened file or directory.
pub fn physical_file_identity(file: &File) -> io::Result<api::PhysicalFileIdentity> {
    filesystem_identity::file_identity(file)
        .map(filesystem_identity::PathIdentity::components)
        .map(|(filesystem, entry)| api::PhysicalFileIdentity::new(filesystem, entry))
}

/// Opens a regular file without following a symbolic link or reparse point.
pub fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    ensure_regular_path_entry(path)?;
    let file = filesystem_identity::open_regular_file_no_follow(path)?;
    ensure_open_regular_binding(path, &file)?;
    Ok(file)
}

/// Opens a regular file for reading and writing without following a symbolic
/// link or reparse point.
pub fn open_regular_file_no_follow_read_write(path: &Path) -> io::Result<File> {
    ensure_regular_path_entry(path)?;
    let file = filesystem_identity::open_regular_file_no_follow_read_write(path)?;
    ensure_open_regular_binding(path, &file)?;
    Ok(file)
}

/// Opens a directory without following a symbolic link or reparse point.
///
/// The returned handle is checked against the path entry after opening so a
/// caller can retain it as a stable binding across later validation.
pub fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    let before = fs::symlink_metadata(path)?;
    if path_entry_is_link_or_reparse(&before) || !before.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a real directory",
        ));
    }
    let file = open_directory_no_follow_impl(path)?;
    let opened = file.metadata()?;
    if !opened.is_dir()
        || physical_path_identity(path)? != physical_file_identity(&file)?
        || path_entry_is_link_or_reparse(&fs::symlink_metadata(path)?)
    {
        return Err(io::Error::other(
            "directory identity changed while it was being opened",
        ));
    }
    Ok(file)
}

fn ensure_regular_path_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if path_entry_is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a real regular file",
        ));
    }
    Ok(())
}

fn ensure_open_regular_binding(path: &Path, file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file()
        || physical_path_identity(path)? != physical_file_identity(file)?
        || path_entry_is_link_or_reparse(&fs::symlink_metadata(path)?)
    {
        return Err(io::Error::other(
            "regular file identity changed while it was being opened",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn path_entry_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn path_entry_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(windows)]
pub(crate) fn open_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_directory(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "durable file replacement is unavailable on this platform",
    ))
}

#[cfg(unix)]
fn open_directory_no_follow_impl(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};

    let file = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(File::from(file))
}

#[cfg(windows)]
fn open_directory_no_follow_impl(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_directory_no_follow_impl(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Atomically replaces one file path with another on the same filesystem.
///
/// The caller must synchronize `src` before this operation and synchronize the
/// containing directory afterwards when crash durability is required.
pub fn replace_file_atomically(src: &Path, dest: &Path) -> io::Result<()> {
    atomic_replace_file(src, dest)
}

/// Durably publishes a staged file without replacing an existing destination.
///
/// The staged file is synchronized before an atomic same-filesystem move. The
/// destination directory and, when different, the source directory are then
/// synchronized. If `dest` already exists, this returns a contextual output
/// conflict for which [`FormatError::is_output_exists`] is true and leaves both
/// paths unchanged. An error synchronizing either directory after the move
/// means the destination may be visible even though this function returned an
/// error. The staged path must remain exclusively owned by the caller;
/// symbolic links and other non-regular entries are rejected.
pub fn publish_file_no_replace(tmp: &Path, dest: &Path) -> Result<(), FormatError> {
    let source_parent_path = parent_directory(tmp);
    let destination_parent_path = parent_directory(dest);
    let source_parent = if source_parent_path == destination_parent_path {
        None
    } else {
        Some(open_directory(source_parent_path)?)
    };
    let destination_parent = open_directory(destination_parent_path)?;
    publish_file_no_replace_with(
        tmp,
        dest,
        &mut |path| sync_staged_file(path),
        &mut |from, to| move_path_no_replace(from, to),
        &mut || {
            destination_parent.sync_all().map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "failed to synchronize the destination directory after publishing {}: {error}",
                        dest.display()
                    ),
                )
            })
        },
        &mut || match &source_parent {
            Some(parent) => parent.sync_all().map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "failed to synchronize the staging directory after publishing {}: {error}",
                        dest.display()
                    ),
                )
            }),
            None => Ok(()),
        },
    )
}

/// Durably publishes a staged directory without replacing an existing destination.
///
/// Every staged entry must be a regular file or directory. Files and directories
/// are synchronized before the directory is moved with the platform's atomic
/// same-filesystem no-replace primitive. If `dest` already exists, both paths
/// remain unchanged and the returned error satisfies
/// [`FormatError::is_output_exists`].
pub fn publish_directory_no_replace(tmp: &Path, dest: &Path) -> Result<(), FormatError> {
    let source_parent_path = parent_directory(tmp);
    let destination_parent_path = parent_directory(dest);
    let source_parent = if source_parent_path == destination_parent_path {
        None
    } else {
        Some(open_directory(source_parent_path)?)
    };
    let destination_parent = open_directory(destination_parent_path)?;

    sync_staged_directory_tree(tmp)?;
    publish_file_no_replace_move_with(tmp, dest, &mut |from, to| move_path_no_replace(from, to))?;
    destination_parent.sync_all().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to synchronize the destination directory after publishing {}: {error}",
                dest.display()
            ),
        )
    })?;
    if let Some(parent) = source_parent {
        parent.sync_all().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to synchronize the staging directory after publishing {}: {error}",
                    dest.display()
                ),
            )
        })?;
    }
    Ok(())
}

pub(crate) fn publish_bound_file_no_replace(
    tmp: &Path,
    file: &File,
    expected: filesystem_identity::PathIdentity,
    dest: &Path,
) -> Result<(), FormatError> {
    if filesystem_identity::file_identity(file)? != expected
        || filesystem_identity::path_identity(tmp).ok() != Some(expected)
        || !file.metadata()?.is_file()
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "writer-owned staging changed before publication: {}",
            tmp.display()
        ))));
    }
    file.sync_all()?;
    let source_parent_path = parent_directory(tmp);
    let destination_parent_path = parent_directory(dest);
    let source_parent = if source_parent_path == destination_parent_path {
        None
    } else {
        Some(open_directory(source_parent_path)?)
    };
    let destination_parent = open_directory(destination_parent_path)?;
    publish_file_no_replace_move_with(tmp, dest, &mut |from, to| move_path_no_replace(from, to))?;
    if filesystem_identity::file_identity(file)? != expected
        || filesystem_identity::path_identity(dest).ok() != Some(expected)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "published output no longer matches the writer-owned staging file: {}",
            dest.display()
        ))));
    }
    destination_parent.sync_all()?;
    if let Some(parent) = source_parent {
        parent.sync_all()?;
    }
    if filesystem_identity::file_identity(file)? != expected
        || filesystem_identity::path_identity(dest).ok() != Some(expected)
    {
        return Err(FormatError::Io(io::Error::other(format!(
            "published output changed before durable completion: {}",
            dest.display()
        ))));
    }
    Ok(())
}

/// Publishes one member of a batch whose staged data is already durable.
/// The caller must hold the destination parent open and synchronize it after
/// all attempted moves, including a partial failure.
pub(crate) fn publish_file_no_replace_already_synced(
    tmp: &Path,
    dest: &Path,
) -> Result<(), FormatError> {
    publish_file_no_replace_move_with(tmp, dest, &mut |from, to| move_path_no_replace(from, to))
}

fn publish_file_no_replace_with<F, R, D, S>(
    tmp: &Path,
    dest: &Path,
    sync_staged: &mut F,
    rename: &mut R,
    sync_destination_parent: &mut D,
    sync_source_parent: &mut S,
) -> Result<(), FormatError>
where
    F: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, &Path) -> io::Result<()>,
    D: FnMut() -> io::Result<()>,
    S: FnMut() -> io::Result<()>,
{
    sync_staged(tmp)?;
    publish_file_no_replace_move_with(tmp, dest, rename)?;
    sync_destination_parent()?;
    sync_source_parent()?;
    Ok(())
}

fn publish_file_no_replace_move_with<R>(
    tmp: &Path,
    dest: &Path,
    rename: &mut R,
) -> Result<(), FormatError>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
{
    match rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(output_exists_error(dest))
        }
        Err(error) => Err(error.into()),
    }
}

/// Moves a file-system entry without replacing an existing destination.
///
/// Files, directories, and symbolic links are moved with the platform's
/// atomic same-filesystem rename primitive. Symbolic links are moved as links;
/// their targets are not followed. If `dest` already exists, this returns
/// [`io::ErrorKind::AlreadyExists`] and leaves both paths unchanged.
pub fn move_path_no_replace(src: &Path, dest: &Path) -> io::Result<()> {
    move_path_no_replace_impl(src, dest)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn move_path_no_replace_impl(src: &Path, dest: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};

    renameat_with(CWD, src, CWD, dest, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn move_path_no_replace_impl(src: &Path, dest: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains a null character",
            ));
        }
        value.push(0);
        Ok(value)
    }

    let src = wide_path(src)?;
    let dest = wide_path(dest)?;
    // SAFETY: both pointers remain valid null-terminated UTF-16 strings for
    // this synchronous call. Zero flags deliberately omit replacement and
    // cross-volume copy behavior.
    if unsafe { MoveFileExW(src.as_ptr(), dest.as_ptr(), 0) } != 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == ERROR_ALREADY_EXISTS as i32 || code == ERROR_FILE_EXISTS as i32 => {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, error))
        }
        _ => Err(error),
    }
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
fn move_path_no_replace_impl(_src: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

pub(crate) fn output_exists_error(dest: &Path) -> FormatError {
    FormatError::output_exists(dest)
}

#[cfg(test)]
fn replace_file_with<R, S>(
    tmp: &Path,
    dest: &Path,
    replace: &mut R,
    sync_parent: &mut S,
) -> Result<(), FormatError>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
    S: FnMut() -> std::io::Result<()>,
{
    sync_staged_file(tmp)?;
    replace(tmp, dest)?;
    sync_parent()?;
    Ok(())
}

fn sync_staged_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("staged output is not a regular file: {}", path.display()),
        ));
    }
    fs::OpenOptions::new().write(true).open(path)?.sync_all()
}

fn sync_staged_directory_tree(root: &Path) -> io::Result<()> {
    const MAX_ENTRIES: usize = 131_072;

    let root_metadata = fs::symlink_metadata(root)?;
    if !root_metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "staged output is not a regular directory: {}",
                root.display()
            ),
        ));
    }

    let mut pending = vec![root.to_path_buf()];
    let mut directories = Vec::new();
    let mut entry_count = 0usize;
    while let Some(directory) = pending.pop() {
        directories.push(directory.clone());
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entry_count = entry_count.checked_add(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "staged directory entry count overflow",
                )
            })?;
            if entry_count > MAX_ENTRIES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "staged directory contains more than {MAX_ENTRIES} entries: {}",
                        root.display()
                    ),
                ));
            }

            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                fs::OpenOptions::new().write(true).open(&path)?.sync_all()?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "staged directory contains a non-regular entry: {}",
                        path.display()
                    ),
                ));
            }
        }
    }

    for directory in directories.into_iter().rev() {
        open_directory(&directory)?.sync_all()?;
    }
    Ok(())
}

/// Reads up to 512 bytes from the head (the tar `ustar` magic sits at offset
/// 257) and 64 bytes from the tail of the stream for magic-number sniffing,
/// rewinding to the start afterwards.
fn sniff_window(stream: &mut dyn ReadSeek) -> Result<(Vec<u8>, Vec<u8>), FormatError> {
    let len = stream.seek(SeekFrom::End(0))?;
    let head_len = len.min(512) as usize;
    let mut head = vec![0u8; head_len];
    stream.seek(SeekFrom::Start(0))?;
    stream.read_exact(&mut head)?;
    let tail_len = len.min(64);
    let mut tail = vec![0u8; tail_len as usize];
    stream.seek(SeekFrom::End(-(tail_len as i64)))?;
    stream.read_exact(&mut tail)?;
    stream.seek(SeekFrom::Start(0))?;
    Ok((head, tail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestArchiveFormat {
        collision_path: Option<PathBuf>,
    }

    struct TestArchiveWriter {
        output: Box<dyn api::WriteSeek>,
        collision_path: Option<PathBuf>,
    }

    struct ShortReadArchiveFormat;

    struct ShortReadArchiveWriter {
        output: Box<dyn api::WriteSeek>,
    }

    #[cfg(unix)]
    struct RebindingArchiveFormat {
        source: PathBuf,
        replacement: PathBuf,
    }

    #[cfg(unix)]
    struct RebindingArchiveWriter {
        output: Box<dyn api::WriteSeek>,
        source: PathBuf,
        replacement: PathBuf,
    }

    struct TestStreamCompressor {
        mutate_source: Option<PathBuf>,
    }

    struct TestCompressSink<'a> {
        output: Box<dyn Write + Send + 'a>,
        mutate_source: Option<PathBuf>,
    }

    struct CountingExtractFormat {
        opens: Arc<AtomicUsize>,
        extracts: Arc<AtomicUsize>,
        entry_count: usize,
        entry_size: u64,
        source_set: Option<api::ArchiveSourceSet>,
        source_verifications: Option<Arc<AtomicUsize>>,
        structure: ArchiveStructureStatus,
    }

    struct CountingExtractReader {
        listed: bool,
        extracts: Arc<AtomicUsize>,
        entry_count: usize,
        entry_size: u64,
        source_set: Option<api::ArchiveSourceSet>,
        source_verifications: Option<Arc<AtomicUsize>>,
        structure: ArchiveStructureStatus,
    }

    struct FileOpenProbeFormat {
        stream_opens: Arc<AtomicUsize>,
        file_opens: Arc<AtomicUsize>,
        controlled_stream_opens: Arc<AtomicUsize>,
        controlled_file_opens: Arc<AtomicUsize>,
        source_probes: Arc<AtomicUsize>,
        controlled_source_probes: Arc<AtomicUsize>,
        source_path: Arc<std::sync::Mutex<Option<PathBuf>>>,
        bytes: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    struct CancellingOpenFormat {
        control: ControlToken,
    }

    impl api::ArchiveFormat for CancellingOpenFormat {
        fn id(&self) -> &'static str {
            "cancel-open"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["cancelopen"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities::default()
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            mut src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            let mut byte = [0_u8; 1];
            src.read_exact(&mut byte)?;
            self.control.cancel();
            src.read_exact(&mut byte)?;
            Err(FormatError::Other(
                "cancelled open unexpectedly continued".into(),
            ))
        }

        fn create(
            &self,
            _dst: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("cancel-open create".into()))
        }
    }

    impl FileOpenProbeFormat {
        fn record(
            &self,
            mut src: Box<dyn api::ReadSeek>,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            let mut bytes = Vec::new();
            src.read_to_end(&mut bytes)?;
            *self.bytes.lock().unwrap() = bytes;
            Err(FormatError::Unsupported("probe complete".into()))
        }
    }

    impl api::ArchiveFormat for FileOpenProbeFormat {
        fn id(&self) -> &'static str {
            "file-open-probe"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["probe"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities::default()
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            self.stream_opens.fetch_add(1, Ordering::SeqCst);
            self.record(src)
        }

        fn open_file(
            &self,
            source_path: &Path,
            source_identity: Option<api::PhysicalFileIdentity>,
            src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            self.file_opens.fetch_add(1, Ordering::SeqCst);
            assert!(source_identity.is_some());
            *self.source_path.lock().unwrap() = Some(source_path.to_path_buf());
            self.record(src)
        }

        fn open_with_control(
            &self,
            src: Box<dyn api::ReadSeek>,
            opts: &OpenOptions,
            ctl: &ControlToken,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            ctl.checkpoint()?;
            self.controlled_stream_opens.fetch_add(1, Ordering::SeqCst);
            self.open(src, opts)
        }

        fn open_file_with_control(
            &self,
            source_path: &Path,
            source_identity: Option<api::PhysicalFileIdentity>,
            src: Box<dyn api::ReadSeek>,
            opts: &OpenOptions,
            ctl: &ControlToken,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            ctl.checkpoint()?;
            self.controlled_file_opens.fetch_add(1, Ordering::SeqCst);
            self.open_file(source_path, source_identity, src, opts)
        }

        fn probe_file_source_set(
            &self,
            _source_path: &Path,
            source_identity: Option<api::PhysicalFileIdentity>,
            _src: &mut dyn api::ReadSeek,
        ) -> Result<Option<api::ArchiveSourceSet>, FormatError> {
            assert!(source_identity.is_some());
            self.source_probes.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn probe_file_source_set_with_control(
            &self,
            source_path: &Path,
            source_identity: Option<api::PhysicalFileIdentity>,
            src: &mut dyn api::ReadSeek,
            ctl: &ControlToken,
        ) -> Result<Option<api::ArchiveSourceSet>, FormatError> {
            ctl.checkpoint()?;
            self.controlled_source_probes.fetch_add(1, Ordering::SeqCst);
            self.probe_file_source_set(source_path, source_identity, src)
        }

        fn create(
            &self,
            _dst: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("probe create".into()))
        }
    }

    impl api::ArchiveFormat for CountingExtractFormat {
        fn id(&self) -> &'static str {
            "counted-extract"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["counted"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities::default()
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(CountingExtractReader {
                listed: false,
                extracts: Arc::clone(&self.extracts),
                entry_count: self.entry_count,
                entry_size: self.entry_size,
                source_set: self.source_set.clone(),
                source_verifications: self.source_verifications.clone(),
                structure: self.structure,
            }))
        }

        fn create(
            &self,
            _dst: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("counted extract create".into()))
        }
    }

    impl ArchiveReader for CountingExtractReader {
        fn structure_status(&self) -> ArchiveStructureStatus {
            self.structure
        }

        fn source_set(&self) -> Option<&api::ArchiveSourceSet> {
            self.source_set.as_ref()
        }

        fn verify_source_set(&self, ctl: &ControlToken) -> Result<(), FormatError> {
            ctl.checkpoint()?;
            if let Some(verifications) = &self.source_verifications {
                verifications.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }

        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            self.listed = true;
            let entry_count = self.entry_count;
            let entry_size = self.entry_size;
            Box::new((0..entry_count).map(move |index| {
                let path = if entry_count == 1 {
                    "payload.txt".to_owned()
                } else {
                    format!("payload-{index}.txt")
                };
                Ok(EntryMeta {
                    path: EntryPath::from_utf8(path),
                    entry_type: EntryType::File,
                    size: entry_size,
                    compressed_size: Some(entry_size),
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                })
            }))
        }

        fn extract_with_report(
            &mut self,
            dest: &Path,
            selection: Option<&[EntryPath]>,
            _opts: &ExtractOptions,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<api::ExtractReport, FormatError> {
            self.extracts.fetch_add(1, Ordering::SeqCst);
            assert!(
                self.listed,
                "the same reader must be listed before extraction"
            );
            assert!(selection.is_none());
            Ok(api::ExtractReport {
                destination: dest.to_path_buf(),
                selected_entries: 1,
                created: 1,
                output_bytes: 7,
                ..api::ExtractReport::default()
            })
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            Ok(Box::new(io::empty()))
        }

        fn test(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestReport, FormatError> {
            Ok(TestReport::default())
        }
    }

    impl api::ArchiveFormat for TestArchiveFormat {
        fn id(&self) -> &'static str {
            "test"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["test"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities {
                can_create: true,
                can_split: true,
                ..api::FormatCapabilities::default()
            }
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("test open".into()))
        }

        fn create(
            &self,
            output: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Ok(Box::new(TestArchiveWriter {
                output,
                collision_path: self.collision_path.clone(),
            }))
        }
    }

    impl api::ArchiveWriter for TestArchiveWriter {
        fn add_entry(
            &mut self,
            _meta: &EntryMeta,
            data: Option<&mut dyn Read>,
        ) -> Result<(), FormatError> {
            if let Some(data) = data {
                io::copy(data, &mut self.output)?;
            }
            Ok(())
        }

        fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
            self.output.flush()?;
            if let Some(path) = &self.collision_path {
                fs::write(path, b"late competitor")?;
            }
            Ok(())
        }
    }

    impl api::ArchiveFormat for ShortReadArchiveFormat {
        fn id(&self) -> &'static str {
            "short-read"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["short"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities {
                can_create: true,
                ..api::FormatCapabilities::default()
            }
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("short-read test open".into()))
        }

        fn create(
            &self,
            output: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Ok(Box::new(ShortReadArchiveWriter { output }))
        }
    }

    impl api::ArchiveWriter for ShortReadArchiveWriter {
        fn add_entry(
            &mut self,
            _meta: &EntryMeta,
            data: Option<&mut dyn Read>,
        ) -> Result<(), FormatError> {
            let Some(data) = data else {
                return Ok(());
            };
            let mut byte = [0u8; 1];
            let read = data.read(&mut byte)?;
            self.output.write_all(&byte[..read])?;
            Ok(())
        }

        fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
            self.output.flush()?;
            Ok(())
        }
    }

    #[cfg(unix)]
    impl api::ArchiveFormat for RebindingArchiveFormat {
        fn id(&self) -> &'static str {
            "rebind"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["rebind"]
        }

        fn capabilities(&self) -> api::FormatCapabilities {
            api::FormatCapabilities {
                can_create: true,
                ..api::FormatCapabilities::default()
            }
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn api::ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("rebind test open".into()))
        }

        fn create(
            &self,
            output: Box<dyn api::WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn api::ArchiveWriter>, FormatError> {
            Ok(Box::new(RebindingArchiveWriter {
                output,
                source: self.source.clone(),
                replacement: self.replacement.clone(),
            }))
        }
    }

    #[cfg(unix)]
    impl api::ArchiveWriter for RebindingArchiveWriter {
        fn add_entry(
            &mut self,
            _meta: &EntryMeta,
            data: Option<&mut dyn Read>,
        ) -> Result<(), FormatError> {
            if let Some(data) = data {
                io::copy(data, &mut self.output)?;
                std::fs::remove_file(&self.source)?;
                std::fs::rename(&self.replacement, &self.source)?;
            }
            Ok(())
        }

        fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
            self.output.flush()?;
            Ok(())
        }
    }

    impl api::Compressor for TestStreamCompressor {
        fn id(&self) -> &'static str {
            "test-stream"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["tstream"]
        }

        fn compress_writer<'a>(
            &self,
            output: Box<dyn Write + Send + 'a>,
            _level: api::CompressionLevel,
            _resources: &api::ResourceOptions,
        ) -> Result<Box<dyn api::CompressSink + 'a>, FormatError> {
            Ok(Box::new(TestCompressSink {
                output,
                mutate_source: self.mutate_source.clone(),
            }))
        }

        fn decompress_reader<'a>(
            &self,
            source: Box<dyn Read + Send + 'a>,
        ) -> Result<Box<dyn Read + Send + 'a>, FormatError> {
            Ok(source)
        }
    }

    impl Write for TestCompressSink<'_> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.flush()
        }
    }

    impl api::CompressSink for TestCompressSink<'_> {
        fn finish(&mut self) -> Result<(), FormatError> {
            self.output.flush()?;
            if let Some(source) = &self.mutate_source {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(source)?
                    .write_all(b"!")?;
            }
            Ok(())
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn counting_extract_engine(
        entry_count: usize,
        entry_size: u64,
    ) -> (Engine, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let opens = Arc::new(AtomicUsize::new(0));
        let extracts = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(CountingExtractFormat {
            opens: Arc::clone(&opens),
            extracts: Arc::clone(&extracts),
            entry_count,
            entry_size,
            source_set: None,
            source_verifications: None,
            structure: ArchiveStructureStatus::Complete,
        }));
        (Engine::new(registry), opens, extracts)
    }

    #[test]
    fn archive_listing_rejects_entries_beyond_the_metadata_limit() {
        let dir = temp_dir("list-entry-limit");
        let archive = dir.join("archive.counted");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(3, 7);

        let error = engine
            .list_with_format_and_source_set_with_entry_limit_and_control(
                &archive,
                &OpenOptions::default(),
                2,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::ResourceLimitExceeded(detail)
                if detail == "archive contains more than 2 entries"
        ));
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extraction_preflight_rejects_entries_beyond_the_metadata_limit() {
        let dir = temp_dir("plan-entry-limit");
        let archive = dir.join("archive.counted");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(3, 7);

        let error = engine
            .plan_extract_with_input_guard_and_entry_limit_controlled(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                2,
                &ControlToken::default(),
                |_, _| Ok(None),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::ResourceLimitExceeded(detail)
                if detail == "archive contains more than 2 entries"
        ));
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extraction_worker_enforces_the_frozen_entry_limit() {
        let dir = temp_dir("extract-entry-limit");
        let archive = dir.join("archive.counted");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(3, 7);
        let mut extract_options = ExtractOptions::default();
        extract_options.limits.max_entries = 2;

        let error = engine
            .plan_and_extract_with_report(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &extract_options,
                &api::NoProgress,
                &ControlToken::default(),
                |_| None,
                |_| Ok(()),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::ResourceLimitExceeded(detail)
                if detail == "archive contains more than 2 entries"
        ));
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn planned_extract_reuses_one_open_archive_reader() {
        let dir = temp_dir("planned-extract-one-open");
        let archive = dir.join("archive.counted");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(1, 7);

        let (plan, report) = engine
            .plan_and_extract_with_report(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &api::NoProgress,
                &ControlToken::default(),
                |_| None,
                |_| Ok(()),
            )
            .unwrap();

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 1);
        assert_eq!(plan.scope.entries, 1);
        assert_eq!(report.destination, destination);
        assert_eq!(report.created, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_extract_reports_structure_from_the_same_reader() {
        let dir = temp_dir("guarded-extract-structure");
        let archive = dir.join("archive.counted");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let opens = Arc::new(AtomicUsize::new(0));
        let extracts = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(CountingExtractFormat {
            opens: Arc::clone(&opens),
            extracts: Arc::clone(&extracts),
            entry_count: 1,
            entry_size: 7,
            source_set: None,
            source_verifications: None,
            structure: ArchiveStructureStatus::ZipLocalHeadersRecovered,
        }));
        let engine = Engine::new(registry);

        let (_, _, structure) = engine
            .plan_and_extract_with_report_guarded_and_structure_controlled(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &api::NoProgress,
                &ControlToken::default(),
                None,
                |_, _| Ok(None),
                |_| Ok(()),
            )
            .unwrap();

        assert_eq!(structure, ArchiveStructureStatus::ZipLocalHeadersRecovered);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_extract_binds_the_opened_readers_source_set() {
        let dir = temp_dir("planned-extract-reader-source-set");
        let archive = dir.join("archive.counted");
        let companion = dir.join("archive.part2");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        std::fs::write(&companion, b"companion").unwrap();
        let source_set = api::ArchiveSourceSet::from_primary_and_ordered_members(
            archive.clone(),
            vec![archive.clone(), companion.clone()],
        )
        .unwrap();
        let opens = Arc::new(AtomicUsize::new(0));
        let extracts = Arc::new(AtomicUsize::new(0));
        let source_verifications = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(CountingExtractFormat {
            opens: Arc::clone(&opens),
            extracts: Arc::clone(&extracts),
            entry_count: 1,
            entry_size: 7,
            source_set: Some(source_set),
            source_verifications: Some(Arc::clone(&source_verifications)),
            structure: ArchiveStructureStatus::Complete,
        }));
        let engine = Engine::new(registry);
        let control = ControlToken::default();

        let (_, _, input_guard) = engine
            .plan_extract_with_input_guard_controlled(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &control,
                |_, _| Ok(None),
            )
            .unwrap();
        std::fs::write(&companion, b"changed companion").unwrap();

        let error = engine
            .plan_and_extract_with_report_guarded_controlled(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &api::NoProgress,
                &control,
                Some(input_guard),
                |_, _| Ok(None),
                |_| Ok(()),
            )
            .unwrap_err();

        assert!(error.is_input_changed());
        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        assert_eq!(source_verifications.load(Ordering::SeqCst), 10);
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn planned_extract_rejects_a_stale_plan_before_extraction() {
        let dir = temp_dir("planned-extract-stale-plan");
        let archive = dir.join("archive.counted");
        let destination = dir.join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(1, 7);

        let error = engine
            .plan_and_extract_with_report(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &api::NoProgress,
                &ControlToken::default(),
                |_| None,
                |plan| Err(FormatError::destination_changed(&plan.destination)),
            )
            .unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn planned_extract_rejects_insufficient_space_before_creating_destination() {
        let dir = temp_dir("planned-extract-disk-full");
        let archive = dir.join("archive.counted");
        let destination = dir.join("nested").join("output");
        std::fs::write(&archive, b"archive").unwrap();
        let (engine, opens, extracts) = counting_extract_engine(1, u64::MAX);

        let error = engine
            .plan_and_extract_with_report(
                &archive,
                &destination,
                &archive,
                false,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &api::NoProgress,
                &ControlToken::default(),
                |_| None,
                |_| Ok(()),
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::DiskFull));
        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(extracts.load(Ordering::SeqCst), 0);
        assert!(!destination.exists());
        assert!(!dir.join("nested").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn test_archive_engine() -> Engine {
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        Engine::new(registry)
    }

    #[test]
    fn engine_passes_a_physical_path_only_for_single_file_sources() {
        let dir = temp_dir("file-open-hook");
        let single = dir.join("single.probe");
        std::fs::write(&single, b"single").unwrap();
        let first = dir.join("split.probe.001");
        let second = dir.join("split.probe.002");
        std::fs::write(&first, b"first-").unwrap();
        std::fs::write(&second, b"second").unwrap();

        let stream_opens = Arc::new(AtomicUsize::new(0));
        let file_opens = Arc::new(AtomicUsize::new(0));
        let controlled_stream_opens = Arc::new(AtomicUsize::new(0));
        let controlled_file_opens = Arc::new(AtomicUsize::new(0));
        let source_probes = Arc::new(AtomicUsize::new(0));
        let controlled_source_probes = Arc::new(AtomicUsize::new(0));
        let source_path = Arc::new(std::sync::Mutex::new(None));
        let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(FileOpenProbeFormat {
            stream_opens: Arc::clone(&stream_opens),
            file_opens: Arc::clone(&file_opens),
            controlled_stream_opens: Arc::clone(&controlled_stream_opens),
            controlled_file_opens: Arc::clone(&controlled_file_opens),
            source_probes: Arc::clone(&source_probes),
            controlled_source_probes: Arc::clone(&controlled_source_probes),
            source_path: Arc::clone(&source_path),
            bytes: Arc::clone(&bytes),
        }));
        let engine = Engine::new(registry);

        let single_error = match engine.open(&single, &OpenOptions::default()) {
            Ok(_) => panic!("probe single-file open should return its marker error"),
            Err(error) => error,
        };
        assert!(matches!(
            single_error,
            FormatError::Unsupported(message) if message == "probe complete"
        ));
        assert_eq!(file_opens.load(Ordering::SeqCst), 1);
        assert_eq!(stream_opens.load(Ordering::SeqCst), 0);
        assert_eq!(controlled_file_opens.load(Ordering::SeqCst), 1);
        assert_eq!(controlled_stream_opens.load(Ordering::SeqCst), 0);
        assert_eq!(*source_path.lock().unwrap(), Some(single.clone()));
        assert_eq!(*bytes.lock().unwrap(), b"single");

        let source_set = engine
            .archive_source_set_with_control(&single, &ControlToken::default())
            .unwrap();
        assert!(source_set.is_none());
        assert_eq!(source_probes.load(Ordering::SeqCst), 1);
        assert_eq!(controlled_source_probes.load(Ordering::SeqCst), 1);

        *source_path.lock().unwrap() = None;
        let split_error = match engine.open(&second, &OpenOptions::default()) {
            Ok(_) => panic!("probe split-volume open should return its marker error"),
            Err(error) => error,
        };
        assert!(matches!(
            split_error,
            FormatError::Unsupported(message) if message == "probe complete"
        ));
        assert_eq!(file_opens.load(Ordering::SeqCst), 1);
        assert_eq!(stream_opens.load(Ordering::SeqCst), 1);
        assert_eq!(controlled_file_opens.load(Ordering::SeqCst), 1);
        assert_eq!(controlled_stream_opens.load(Ordering::SeqCst), 1);
        assert_eq!(*source_path.lock().unwrap(), None);
        assert_eq!(*bytes.lock().unwrap(), b"first-second");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn controlled_open_maps_mid_parse_io_interruption_to_cancelled() {
        let dir = temp_dir("controlled-open-cancel");
        let archive = dir.join("archive.cancelopen");
        std::fs::write(&archive, b"archive").unwrap();
        let control = ControlToken::default();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(CancellingOpenFormat {
            control: control.clone(),
        }));
        let engine = Engine::new(registry);

        let error = match engine.open_with_control(&archive, &OpenOptions::default(), &control) {
            Ok(_) => panic!("controlled open should stop during format parsing"),
            Err(error) => error,
        };

        assert!(matches!(error, FormatError::Cancelled));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn assert_no_create_staging(dir: &Path) {
        assert!(!std::fs::read_dir(dir).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".create-")));
    }

    fn assert_no_create_commit_artifacts(dir: &Path) {
        assert!(!std::fs::read_dir(dir).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains(".create-") || name.starts_with(".squallz-update-")
        }));
    }

    fn canonical_test_destination(path: &Path) -> PathBuf {
        std::fs::canonicalize(path.parent().unwrap())
            .unwrap()
            .join(path.file_name().unwrap())
    }

    fn guarded_test_create_error(
        dest: &Path,
        input: &Path,
        guard: CreateDestinationGuard,
    ) -> FormatError {
        let input = input.to_path_buf();
        test_archive_engine()
            .create_with_report_policy(
                dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                CreateCommitPolicy::ReplaceIfUnchanged(guard),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err()
    }

    fn prepare_test_create(
        engine: &Engine,
        dest: &Path,
        input: &Path,
    ) -> create::PreparedCreateInputs {
        create::prepare_unsplit_create_with_reserved_outputs(
            engine,
            dest,
            &[input.to_path_buf()],
            &[],
            &CreateOptions::default(),
            |_count, _path| {},
        )
        .unwrap()
    }

    fn run_prepared_test_create(
        engine: &Engine,
        dest: &Path,
        input: &Path,
        prepared: create::PreparedCreateInputs,
    ) -> FormatError {
        create::create_prepared_with_reserved_outputs(
            engine,
            dest,
            &[input.to_path_buf()],
            &[],
            &CreateOptions::default(),
            &api::NoProgress,
            &ControlToken::new(),
            prepared,
            false,
        )
        .unwrap_err()
    }

    #[test]
    fn unknown_format_is_rejected() {
        let dir = temp_dir("unknown");
        let f = dir.join("blob.unknown");
        std::fs::write(&f, b"not an archive at all").unwrap();
        let engine = Engine::new(FormatRegistry::new());
        let err = engine.list(&f, &OpenOptions::default()).unwrap_err();
        assert!(matches!(err, FormatError::Unsupported(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovery_protect_sources_cover_the_complete_byte_split_set() {
        let dir = temp_dir("recovery-protect-split");
        let first = dir.join("archive.zip.001");
        let second = dir.join("archive.zip.002");
        let third = dir.join("archive.zip.003");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::fs::write(&third, b"third").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        assert_eq!(
            engine.recovery_protect_sources(&second).unwrap(),
            vec![first, second, third]
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovery_protect_sources_keep_a_single_file() {
        let dir = temp_dir("recovery-protect-single");
        let archive = dir.join("archive.bin");
        std::fs::write(&archive, b"single").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        assert_eq!(
            engine.recovery_protect_sources(&archive).unwrap(),
            vec![archive]
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_file_preserves_a_late_destination_when_install_fails() {
        let dir = temp_dir("replace-race");
        let dest = dir.join("archive.zip");
        let tmp = dir.join("archive.tmp");
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&tmp, b"new").unwrap();

        let error = replace_file_with(
            &tmp,
            &dest,
            &mut |_from, to| {
                std::fs::write(to, b"late competitor")?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected install failure",
                ))
            },
            &mut || panic!("parent sync must not run after a failed replacement"),
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::Io(_)));
        assert_eq!(std::fs::read(&dest).unwrap(), b"late competitor");
        assert_eq!(std::fs::read(&tmp).unwrap(), b"new");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("replace-backup")
        }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_file_does_not_reverse_commit_when_parent_sync_fails() {
        let dir = temp_dir("replace-sync");
        let dest = dir.join("archive.zip");
        let tmp = dir.join("archive.tmp");
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&tmp, b"new").unwrap();

        let error = replace_file_with(
            &tmp,
            &dest,
            &mut |from, to| std::fs::rename(from, to),
            &mut || {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected directory sync failure",
                ))
            },
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::Io(_)));
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!tmp.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_file_atomically_replaces_without_backup_artifacts() {
        let dir = temp_dir("replace-atomic");
        let dest = dir.join("archive.zip");
        let tmp = dir.join("archive.tmp");
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&tmp, b"new").unwrap();

        replace_file(&tmp, &dest).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!tmp.exists());
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("replace-backup")
        }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn physical_identity_binds_an_open_file_to_its_path() {
        let dir = temp_dir("physical-identity");
        let path = dir.join("state.json");
        std::fs::write(&path, b"state").unwrap();
        let file = std::fs::File::open(&path).unwrap();

        assert_eq!(
            physical_path_identity(&path).unwrap(),
            physical_file_identity(&file).unwrap()
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_follow_directory_open_rejects_a_symbolic_link() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("directory-no-follow");
        let real = dir.join("real");
        let alias = dir.join("alias");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();

        assert!(open_directory_no_follow(&real).is_ok());
        assert!(open_directory_no_follow(&alias).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_follow_regular_open_rejects_a_symbolic_link() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("regular-no-follow");
        let real = dir.join("real.json");
        let alias = dir.join("alias.json");
        std::fs::write(&real, b"state").unwrap();
        symlink(&real, &alias).unwrap();

        assert!(open_regular_file_no_follow(&real).is_ok());
        assert!(open_regular_file_no_follow_read_write(&real).is_ok());
        assert!(open_regular_file_no_follow(&alias).is_err());
        assert!(open_regular_file_no_follow_read_write(&alias).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_create_preserves_a_destination_created_during_write() {
        let dir = temp_dir("create-no-replace-race");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"archive payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: Some(dest.clone()),
        }));
        let engine = Engine::new(registry);
        let progress = api::NoProgress;
        let ctl = ControlToken::new();

        let error = engine
            .create_with_report_no_replace(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &progress,
                &ctl,
            )
            .unwrap_err();

        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(dest.as_path()));
        assert_eq!(std::fs::read(&dest).unwrap(), b"late competitor");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains(".create-") || name.contains(".split-")
        }));

        let engine = test_archive_engine();
        engine
            .create_with_report(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &progress,
                &ctl,
            )
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"archive payload");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_create_replaces_the_exact_inspected_destination() {
        let dir = temp_dir("create-guard-success");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"new archive payload").unwrap();
        std::fs::write(&dest, b"old archive payload").unwrap();
        let state = inspect_create_destination(&dest, CreateArtifactKind::Archive).unwrap();
        let guard = state.guard.unwrap();
        let engine = test_archive_engine();

        let verified = engine
            .create_with_verification_policy(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                CreateCommitPolicy::ReplaceIfUnchanged(guard),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"new archive payload");
        assert_eq!(verified.create.primary_output, dest);
        assert_eq!(verified.create.outputs, vec![dest.clone()]);
        assert_no_create_commit_artifacts(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_create_preserves_a_destination_changed_during_write() {
        let dir = temp_dir("create-guard-race");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"new archive payload").unwrap();
        std::fs::write(&dest, b"old archive payload").unwrap();
        let state = inspect_create_destination(&dest, CreateArtifactKind::Archive).unwrap();
        let guard = state.guard.unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: Some(dest.clone()),
        }));
        let engine = Engine::new(registry);

        let error = engine
            .create_with_report_policy(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                CreateCommitPolicy::ReplaceIfUnchanged(guard),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();

        assert!(error.is_destination_changed());
        let canonical_dest = canonical_test_destination(&dest);
        assert_eq!(
            error.destination_changed_path(),
            Some(canonical_dest.as_path())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"late competitor");
        assert_no_create_commit_artifacts(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_create_reports_a_removed_destination_as_changed() {
        let dir = temp_dir("create-guard-missing");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"new archive payload").unwrap();
        std::fs::write(&dest, b"old archive payload").unwrap();
        let guard = inspect_create_destination(&dest, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        let canonical_dest = canonical_test_destination(&dest);
        std::fs::remove_file(&dest).unwrap();

        let error = guarded_test_create_error(&dest, &input, guard);

        assert!(error.is_destination_changed());
        assert_eq!(
            error.destination_changed_path(),
            Some(canonical_dest.as_path())
        );
        assert!(!dest.exists());
        assert_no_create_commit_artifacts(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guarded_create_reports_a_directory_replacement_as_changed() {
        let dir = temp_dir("create-guard-directory");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"new archive payload").unwrap();
        std::fs::write(&dest, b"old archive payload").unwrap();
        let guard = inspect_create_destination(&dest, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        let canonical_dest = canonical_test_destination(&dest);
        std::fs::remove_file(&dest).unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("marker"), b"keep directory").unwrap();

        let error = guarded_test_create_error(&dest, &input, guard);

        assert!(error.is_destination_changed());
        assert_eq!(
            error.destination_changed_path(),
            Some(canonical_dest.as_path())
        );
        assert_eq!(
            std::fs::read(dest.join("marker")).unwrap(),
            b"keep directory"
        );
        assert_no_create_commit_artifacts(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn guarded_create_reports_a_symbolic_link_replacement_as_changed() {
        let dir = temp_dir("create-guard-symlink");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        let competitor = dir.join("competitor.test");
        std::fs::write(&input, b"new archive payload").unwrap();
        std::fs::write(&dest, b"old archive payload").unwrap();
        std::fs::write(&competitor, b"keep competitor").unwrap();
        let guard = inspect_create_destination(&dest, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        let canonical_dest = canonical_test_destination(&dest);
        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(&competitor, &dest).unwrap();

        let error = guarded_test_create_error(&dest, &input, guard);

        assert!(error.is_destination_changed());
        assert_eq!(
            error.destination_changed_path(),
            Some(canonical_dest.as_path())
        );
        assert!(std::fs::symlink_metadata(&dest)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&competitor).unwrap(), b"keep competitor");
        assert_no_create_commit_artifacts(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_create_ignores_entries_added_after_manifest_scan() {
        let dir = temp_dir("prepared-create-members");
        let source = dir.join("source");
        let dest = dir.join("payload.test");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("early.txt"), b"early entry").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);
        let options = CreateOptions::default();
        let prepared = create::prepare_unsplit_create_with_reserved_outputs(
            &engine,
            &dest,
            std::slice::from_ref(&source),
            &[],
            &options,
            |_count, _path| {},
        )
        .unwrap();

        std::fs::write(source.join("late.txt"), b"late entry").unwrap();
        let verified = create::create_prepared_with_reserved_outputs(
            &engine,
            &dest,
            std::slice::from_ref(&source),
            &[],
            &options,
            &api::NoProgress,
            &ControlToken::new(),
            prepared,
            true,
        )
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"early entry");
        assert!(verified
            .manifest
            .iter()
            .all(|entry| !entry.archive_path.to_string().ends_with("late.txt")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_create_failure_cleans_staging_and_preserves_reserved_output() {
        let dir = temp_dir("prepared-create-cleanup");
        let source = dir.join("source");
        let input = source.join("input.txt");
        let dest = dir.join("payload.test");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(&input, b"source entry").unwrap();
        std::fs::write(&dest, b"reserved payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);
        let options = CreateOptions::default();
        let prepared = create::prepare_unsplit_create_with_reserved_outputs(
            &engine,
            &dest,
            std::slice::from_ref(&source),
            &[],
            &options,
            |_count, _path| {},
        )
        .unwrap();
        std::fs::remove_file(&input).unwrap();

        let error = create::create_prepared_with_reserved_outputs(
            &engine,
            &dest,
            std::slice::from_ref(&source),
            &[],
            &options,
            &api::NoProgress,
            &ControlToken::new(),
            prepared,
            false,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::NotFound
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"reserved payload");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".create-")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_create_rejects_same_length_file_replacement() {
        let dir = temp_dir("prepared-create-replacement");
        let input = dir.join("input.bin");
        let replacement = dir.join("replacement.bin");
        let dest = dir.join("payload.test");
        std::fs::write(&input, [b'A'; 16]).unwrap();
        std::fs::write(&replacement, [b'B'; 16]).unwrap();
        std::fs::write(&dest, b"reserved payload").unwrap();
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::remove_file(&input).unwrap();
        std::fs::rename(&replacement, &input).unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"reserved payload");
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_create_rejects_size_drift_without_verification() {
        let dir = temp_dir("prepared-create-size-drift");
        let input = dir.join("input.bin");
        let dest = dir.join("payload.test");
        std::fs::write(&input, b"source payload").unwrap();
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::OpenOptions::new()
            .append(true)
            .open(&input)
            .unwrap()
            .write_all(b"!")
            .unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_create_rejects_empty_directory_replacement() {
        let dir = temp_dir("prepared-create-directory-replacement");
        let input = dir.join("input");
        let replacement = dir.join("replacement");
        let dest = dir.join("payload.test");
        std::fs::create_dir(&input).unwrap();
        std::fs::create_dir(&replacement).unwrap();
        std::fs::write(&dest, b"reserved payload").unwrap();
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::remove_dir(&input).unwrap();
        std::fs::rename(&replacement, &input).unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"reserved payload");
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn prepared_create_rejects_regular_file_changed_to_symlink() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("prepared-create-file-to-link");
        let input = dir.join("input.bin");
        let target = dir.join("target.bin");
        let dest = dir.join("payload.test");
        std::fs::write(&input, [b'A'; 16]).unwrap();
        std::fs::write(&target, [b'B'; 16]).unwrap();
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::remove_file(&input).unwrap();
        symlink(&target, &input).unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn prepared_create_rejects_symlink_target_change() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("prepared-create-link-target");
        let first = dir.join("first.bin");
        let second = dir.join("second.bin");
        let input = dir.join("input-link");
        let dest = dir.join("payload.test");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        symlink(&first, &input).unwrap();
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::remove_file(&input).unwrap();
        symlink(&second, &input).unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn prepared_create_rejects_in_place_rewrite_with_restored_mtime() {
        use std::os::unix::fs::MetadataExt;
        use std::time::Duration;

        let dir = temp_dir("prepared-create-ctime-drift");
        let input = dir.join("input.bin");
        let dest = dir.join("payload.test");
        std::fs::write(&input, [b'A'; 16]).unwrap();
        let original_metadata = std::fs::metadata(&input).unwrap();
        let original_modified = original_metadata.modified().unwrap();
        let original_changed = (original_metadata.ctime(), original_metadata.ctime_nsec());
        let engine = test_archive_engine();
        let prepared = prepare_test_create(&engine, &dest, &input);

        let mut changed = original_changed;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&input)
                .unwrap();
            file.write_all(&[b'B'; 16]).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(original_modified))
                .unwrap();
            drop(file);
            let metadata = std::fs::metadata(&input).unwrap();
            changed = (metadata.ctime(), metadata.ctime_nsec());
            if changed != original_changed {
                break;
            }
        }
        assert_ne!(changed, original_changed);
        assert_eq!(
            std::fs::metadata(&input).unwrap().modified().unwrap(),
            original_modified
        );
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ordinary_create_rejects_a_writer_that_underreads_input() {
        let dir = temp_dir("create-underread");
        let input = dir.join("input.bin");
        let dest = dir.join("payload.short");
        std::fs::write(&input, b"source payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(ShortReadArchiveFormat));
        let engine = Engine::new(registry);

        let error = engine
            .create(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_create_rechecks_the_source_path_after_streaming() {
        let dir = temp_dir("create-post-read-rebind");
        let input = dir.join("input.bin");
        let replacement = dir.join("replacement.bin");
        let dest = dir.join("payload.rebind");
        std::fs::write(&input, [b'A'; 16]).unwrap();
        std::fs::write(&replacement, [b'B'; 16]).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(RebindingArchiveFormat {
            source: input.clone(),
            replacement,
        }));
        let engine = Engine::new(registry);

        let error = engine
            .create(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(&input).unwrap(), [b'B'; 16]);
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ordinary_single_stream_rechecks_input_after_streaming() {
        let dir = temp_dir("stream-post-read-change");
        let input = dir.join("input.bin");
        let dest = dir.join("payload.tstream");
        std::fs::write(&input, b"source payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_compressor(Arc::new(TestStreamCompressor {
            mutate_source: Some(input.clone()),
        }));
        let engine = Engine::new(registry);

        let error = engine
            .create(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(&input).unwrap(), b"source payload!");
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn single_stream_deduplicates_regular_path_aliases() {
        let dir = temp_dir("stream-regular-alias");
        let input = dir.join("input.bin");
        let alias = dir.join(".").join("input.bin");
        let dest = dir.join("payload.tstream");
        std::fs::write(&input, b"source payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_compressor(Arc::new(TestStreamCompressor {
            mutate_source: None,
        }));
        let engine = Engine::new(registry);

        engine
            .create(
                &dest,
                &[input, alias],
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"source payload");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn single_stream_rejects_distinct_hard_link_paths() {
        let dir = temp_dir("stream-hard-links");
        let input = dir.join("input.bin");
        let link = dir.join("input-link.bin");
        let dest = dir.join("payload.tstream");
        std::fs::write(&input, b"source payload").unwrap();
        std::fs::hard_link(&input, &link).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_compressor(Arc::new(TestStreamCompressor {
            mutate_source: None,
        }));
        let engine = Engine::new(registry);

        let error = engine
            .create(
                &dest,
                &[input, link],
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();

        assert!(
            matches!(error, FormatError::Unsupported(ref detail) if detail.contains("exactly one file"))
        );
        assert!(!dest.exists());
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn single_stream_rejects_multiple_final_symlink_aliases() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("stream-symlink-aliases");
        let input = dir.join("input.bin");
        let first_link = dir.join("input-link.bin");
        let second_link = dir.join("input-link-two.bin");
        std::fs::write(&input, b"source payload").unwrap();
        symlink(&input, &first_link).unwrap();
        symlink(&input, &second_link).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_compressor(Arc::new(TestStreamCompressor {
            mutate_source: None,
        }));
        let engine = Engine::new(registry);
        let cases = [
            vec![input.clone(), first_link.clone()],
            vec![first_link.clone(), input.clone()],
            vec![first_link.clone(), second_link.clone()],
            vec![first_link.clone(), first_link.clone()],
        ];

        for (index, inputs) in cases.into_iter().enumerate() {
            let dest = dir.join(format!("rejected-{index}.tstream"));
            let error = engine
                .create(
                    &dest,
                    &inputs,
                    &CreateOptions::default(),
                    &api::NoProgress,
                    &ControlToken::new(),
                )
                .unwrap_err();
            assert!(
                matches!(error, FormatError::Unsupported(ref detail) if detail.contains("exactly one file"))
            );
            assert!(!dest.exists());
        }

        let dest = dir.join("single-link.tstream");
        engine
            .create(
                &dest,
                std::slice::from_ref(&first_link),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"source payload");
        assert!(std::fs::symlink_metadata(&first_link)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_single_stream_rejects_same_length_replacement() {
        let dir = temp_dir("prepared-stream-replacement");
        let input = dir.join("input.bin");
        let replacement = dir.join("replacement.bin");
        let dest = dir.join("payload.tstream");
        std::fs::write(&input, [b'A'; 16]).unwrap();
        std::fs::write(&replacement, [b'B'; 16]).unwrap();
        std::fs::write(&dest, b"reserved payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_compressor(Arc::new(TestStreamCompressor {
            mutate_source: None,
        }));
        let engine = Engine::new(registry);
        let prepared = prepare_test_create(&engine, &dest, &input);

        std::fs::remove_file(&input).unwrap();
        std::fs::rename(&replacement, &input).unwrap();
        let error = run_prepared_test_create(&engine, &dest, &input, prepared);

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"reserved payload");
        assert_no_create_staging(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_split_create_rejects_a_late_output_family_member() {
        let dir = temp_dir("create-split-no-replace-race");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        let first_volume = dir.join("archive.test.001");
        std::fs::write(&input, vec![7u8; 2500]).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: Some(first_volume.clone()),
        }));
        let engine = Engine::new(registry);
        let progress = api::NoProgress;
        let ctl = ControlToken::new();
        let options = CreateOptions {
            split_size: Some(1024),
            ..CreateOptions::default()
        };

        let error = engine
            .create_with_report_no_replace(
                &dest,
                std::slice::from_ref(&input),
                &options,
                &progress,
                &ctl,
            )
            .unwrap_err();

        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(first_volume.as_path()));
        assert_eq!(std::fs::read(&first_volume).unwrap(), b"late competitor");
        assert!(!dir.join("archive.test.002").exists());
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".split-")
        }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_create_commits_single_and_split_outputs_when_free() {
        let dir = temp_dir("create-no-replace-success");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&input, b"archive payload").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);

        let report = engine
            .create_with_report_no_replace(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"archive payload");
        assert_eq!(report.primary_output, dest);
        assert_eq!(report.outputs, vec![dest.clone()]);
        assert!(report.preserved_outputs.is_empty());
        assert_eq!(report.total_output_bytes, b"archive payload".len() as u64);

        let split_dest = dir.join("split.test");
        let split_report = engine
            .create_with_report_no_replace(
                &split_dest,
                std::slice::from_ref(&input),
                &CreateOptions {
                    split_size: Some(1024),
                    ..CreateOptions::default()
                },
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let mut split_contents = Vec::new();
        for output in &split_report.outputs {
            split_contents.extend(std::fs::read(output).unwrap());
        }
        assert_eq!(split_contents, b"archive payload");
        assert_eq!(split_report.primary_output, dir.join("split.test.001"));
        assert_eq!(split_report.split_volume_count, Some(1));
        assert!(split_report.preserved_outputs.is_empty());
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains(".create-") || name.contains(".split-")
        }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn three_consecutive_creates_exclude_source_cleanup_holders() {
        let dir = temp_dir("create-source-cleanup-holders");
        let root = dir.join("project");
        let active = root.join(format!(".squallz-trash-hold-{}-1", std::process::id()));
        let stale = root.join(".squallz-trash-hold-424242-9");
        let similar = root.join(".squallz-trash-hold-42-7-notes");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&similar).unwrap();
        std::fs::write(active.join("active.bin"), b"active").unwrap();
        std::fs::write(stale.join("stale.bin"), b"stale").unwrap();
        std::fs::write(similar.join("similar.bin"), b"similar").unwrap();
        std::fs::write(root.join("payload.bin"), b"payload").unwrap();

        let dest = root.join("archive.test");
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);

        for _ in 0..3 {
            let report = engine
                .create_with_report(
                    &dest,
                    std::slice::from_ref(&root),
                    &CreateOptions::default(),
                    &api::NoProgress,
                    &ControlToken::new(),
                )
                .unwrap();

            assert_eq!(std::fs::read(&dest).unwrap(), b"similarpayload");
            assert_eq!(report.total_output_bytes, b"similarpayload".len() as u64);
            assert_eq!(std::fs::read(active.join("active.bin")).unwrap(), b"active");
            assert_eq!(std::fs::read(stale.join("stale.bin")).unwrap(), b"stale");
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_split_create_reports_only_its_owned_backups() {
        let dir = temp_dir("create-split-replace-backups");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        let orphan = dir.join(".archive.test.001.split-backup-999-0.tmp.archive.test.001");
        let old_payload = vec![0x11; 2500];
        let new_payload = vec![0x22; 2500];
        std::fs::write(&input, &old_payload).unwrap();
        std::fs::write(&orphan, b"unowned recovery artifact").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);
        let options = CreateOptions {
            split_size: Some(1024),
            ..CreateOptions::default()
        };

        let first = engine
            .create_with_report(
                &dest,
                std::slice::from_ref(&input),
                &options,
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(first.preserved_outputs.is_empty());
        assert!(orphan.exists());

        std::fs::write(&input, &new_payload).unwrap();
        let second = engine
            .create_with_report(
                &dest,
                std::slice::from_ref(&input),
                &options,
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(second.preserved_outputs.len(), first.outputs.len());
        assert!(!second.preserved_outputs.contains(&orphan));
        assert!(orphan.exists());
        let mut actual_owned_backups = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path != &orphan && path.to_string_lossy().contains(".split-backup-"))
            .collect::<Vec<_>>();
        actual_owned_backups.sort();
        let mut reported_backups = second.preserved_outputs.clone();
        reported_backups.sort();
        assert_eq!(reported_backups, actual_owned_backups);
        let mut preserved_payload = Vec::new();
        for backup in &second.preserved_outputs {
            preserved_payload.extend(std::fs::read(backup).unwrap());
        }
        assert_eq!(preserved_payload, old_payload);
        let mut installed_payload = Vec::new();
        for output in &second.outputs {
            installed_payload.extend(std::fs::read(output).unwrap());
        }
        assert_eq!(installed_payload, new_payload);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn consecutive_split_rebuilds_inside_the_source_do_not_archive_transaction_backups() {
        let dir = temp_dir("create-split-inside-source");
        let source = dir.join("project");
        std::fs::create_dir(&source).unwrap();
        let payload = vec![0x5a; 2500];
        std::fs::write(source.join("source.bin"), &payload).unwrap();
        let dest = source.join("archive.test");
        let interrupted = source.join(".archive.test.split-999-0.tmp.archive.test");
        std::fs::write(&interrupted, b"interrupted full split staging archive").unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);
        let options = CreateOptions {
            split_size: Some(1024),
            ..CreateOptions::default()
        };

        for run in 0..3 {
            let report = engine
                .create_with_report(
                    &dest,
                    std::slice::from_ref(&source),
                    &options,
                    &api::NoProgress,
                    &ControlToken::new(),
                )
                .unwrap();
            let mut archived = Vec::new();
            for output in &report.outputs {
                archived.extend(std::fs::read(output).unwrap());
            }
            assert_eq!(
                archived, payload,
                "split rebuild {run} archived an output artifact"
            );
        }

        assert!(std::fs::read_dir(&source).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".split-backup-")
        }));
        assert!(interrupted.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verified_create_reports_exact_bytes_consumed_by_the_writer() {
        let dir = temp_dir("create-verified-inputs");
        let input = dir.join("source.bin");
        let dest = dir.join("archive.test");
        let payload = b"content fingerprint captured during create";
        std::fs::write(&input, payload).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));
        let engine = Engine::new(registry);

        let verified = engine
            .create_with_verification_no_replace(
                &dest,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(verified.create.outputs, vec![dest]);
        assert_eq!(verified.inputs.len(), 1);
        assert_eq!(
            verified.inputs[0].path,
            std::fs::canonicalize(input).unwrap()
        );
        assert_eq!(verified.inputs[0].size, payload.len() as u64);
        assert_eq!(verified.inputs[0].blake3, *blake3::hash(payload).as_bytes());
        assert_eq!(verified.manifest.len(), 1);
        let entry = &verified.manifest[0];
        assert_eq!(entry.source_path, verified.inputs[0].path);
        assert_eq!(entry.archive_path, EntryPath::from_utf8("source.bin"));
        assert_eq!(entry.entry_type, EntryType::File);
        assert_eq!(entry.size, payload.len() as u64);
        assert_eq!(entry.blake3, Some(verified.inputs[0].blake3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verified_create_manifest_tracks_directory_and_file_metadata() {
        let dir = temp_dir("create-verified-manifest-metadata");
        let root = dir.join("project");
        let input = root.join("payload.bin");
        let dest = dir.join("archive.test");
        let payload = b"writer-authoritative manifest";
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&input, payload).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750)).unwrap();
            std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let root_metadata = std::fs::symlink_metadata(&root).unwrap();
        let input_metadata = std::fs::symlink_metadata(&input).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));

        let verified = Engine::new(registry)
            .create_with_verification_no_replace(
                &dest,
                std::slice::from_ref(&root),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert_eq!(verified.manifest.len(), 2);
        let directory = &verified.manifest[0];
        assert_eq!(directory.source_path, std::fs::canonicalize(&root).unwrap());
        assert_eq!(directory.archive_path, EntryPath::from_utf8("project"));
        assert_eq!(directory.entry_type, EntryType::Dir);
        assert_eq!(directory.size, 0);
        assert_eq!(
            directory.modified,
            root_metadata
                .modified()
                .ok()
                .map(CreateInputModifiedTime::from)
        );
        assert_eq!(directory.unix_mode, test_unix_mode(&root_metadata));
        assert_eq!(directory.blake3, None);

        let file = &verified.manifest[1];
        assert_eq!(file.source_path, std::fs::canonicalize(&input).unwrap());
        assert_eq!(
            file.archive_path,
            EntryPath::from_utf8("project/payload.bin")
        );
        assert_eq!(file.entry_type, EntryType::File);
        assert_eq!(file.size, payload.len() as u64);
        assert_eq!(
            file.modified,
            input_metadata
                .modified()
                .ok()
                .map(CreateInputModifiedTime::from)
        );
        assert_eq!(file.unix_mode, test_unix_mode(&input_metadata));
        assert_eq!(file.blake3, Some(*blake3::hash(payload).as_bytes()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_create_manifest_preserves_symlink_identity_and_target() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("create-verified-manifest-symlink");
        let target = dir.join("target.bin");
        let link = dir.join("alias.bin");
        let dest = dir.join("archive.test");
        std::fs::write(&target, b"target stays outside this input").unwrap();
        symlink("target.bin", &link).unwrap();
        let link_metadata = std::fs::symlink_metadata(&link).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(TestArchiveFormat {
            collision_path: None,
        }));

        let verified = Engine::new(registry)
            .create_with_verification_no_replace(
                &dest,
                std::slice::from_ref(&link),
                &CreateOptions::default(),
                &api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        assert!(verified.inputs.is_empty());
        assert_eq!(verified.manifest.len(), 1);
        let entry = &verified.manifest[0];
        assert_eq!(
            entry.source_path,
            std::fs::canonicalize(&dir).unwrap().join("alias.bin")
        );
        assert_eq!(entry.archive_path, EntryPath::from_utf8("alias.bin"));
        assert_eq!(
            entry.entry_type,
            EntryType::Symlink {
                target: b"target.bin".to_vec()
            }
        );
        assert_eq!(entry.size, 0);
        assert_eq!(
            entry.modified,
            link_metadata
                .modified()
                .ok()
                .map(CreateInputModifiedTime::from)
        );
        assert_eq!(entry.unix_mode, test_unix_mode(&link_metadata));
        assert_eq!(entry.blake3, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    fn test_unix_mode(metadata: &std::fs::Metadata) -> Option<u32> {
        use std::os::unix::fs::PermissionsExt;

        Some(metadata.permissions().mode())
    }

    #[cfg(not(unix))]
    fn test_unix_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
        None
    }

    #[test]
    fn no_replace_publish_maps_conflicts_and_consumes_staging_on_success() {
        let dir = temp_dir("publish-no-replace");
        let staged = dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();
        std::fs::write(&dest, b"existing payload").unwrap();

        let error = publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| Ok(()),
            &mut |_from, _to| {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "injected destination conflict",
                ))
            },
            &mut || panic!("a conflicting publication must not sync the destination parent"),
            &mut || panic!("a conflicting publication must not sync the source parent"),
        )
        .unwrap_err();
        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(dest.as_path()));
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing payload");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new payload");

        std::fs::remove_file(&dest).unwrap();
        publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| Ok(()),
            &mut |from, to| std::fs::rename(from, to),
            &mut || Ok(()),
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new payload");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_publish_stops_before_move_when_staged_sync_fails() {
        let dir = temp_dir("publish-no-replace-staged-sync");
        let staged = dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();
        std::fs::write(&dest, b"existing payload").unwrap();

        let error = publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected staged sync failure",
                ))
            },
            &mut |_from, _to| panic!("move must not run after a staged sync failure"),
            &mut || panic!("destination sync must not run after a staged sync failure"),
            &mut || panic!("source sync must not run after a staged sync failure"),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing payload");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new payload");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_publish_reports_parent_sync_failure_after_commit() {
        let dir = temp_dir("publish-no-replace-parent-sync");
        let staged = dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();

        let error = publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| Ok(()),
            &mut |from, to| std::fs::rename(from, to),
            &mut || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected parent sync failure",
                ))
            },
            &mut || panic!("source sync must not run after a destination sync failure"),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"new payload");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_publish_syncs_both_parents_after_a_cross_directory_move() {
        use std::cell::RefCell;

        let dir = temp_dir("publish-no-replace-cross-directory");
        let staging_dir = dir.join("work");
        std::fs::create_dir(&staging_dir).unwrap();
        let staged = staging_dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();
        let sync_order = RefCell::new(Vec::new());

        publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| Ok(()),
            &mut |from, to| std::fs::rename(from, to),
            &mut || {
                sync_order.borrow_mut().push("destination");
                Ok(())
            },
            &mut || {
                sync_order.borrow_mut().push("source");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(*sync_order.borrow(), ["destination", "source"]);
        assert_eq!(std::fs::read(&dest).unwrap(), b"new payload");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_publish_reports_source_sync_failure_after_commit() {
        let dir = temp_dir("publish-no-replace-source-sync");
        let staging_dir = dir.join("work");
        std::fs::create_dir(&staging_dir).unwrap();
        let staged = staging_dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();

        let error = publish_file_no_replace_with(
            &staged,
            &dest,
            &mut |_path| Ok(()),
            &mut |from, to| std::fs::rename(from, to),
            &mut || Ok(()),
            &mut || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected source parent sync failure",
                ))
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::PermissionDenied
        ));
        assert_eq!(std::fs::read(&dest).unwrap(), b"new payload");
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_publish_rejects_non_regular_staging() {
        let dir = temp_dir("publish-no-replace-non-regular-staging");
        let staged = dir.join("staged");
        let dest = dir.join("archive.zip");
        std::fs::create_dir(&staged).unwrap();

        let error = publish_file_no_replace(&staged, &dest).unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidInput
        ));
        assert!(staged.is_dir());
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_replace_publish_rejects_symbolic_link_staging() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("publish-no-replace-symlink-staging");
        let target = dir.join("target");
        let staged = dir.join("staged");
        let dest = dir.join("archive.zip");
        std::fs::write(&target, b"target payload").unwrap();
        symlink(&target, &staged).unwrap();

        let error = publish_file_no_replace(&staged, &dest).unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidInput
        ));
        assert_eq!(std::fs::read_link(&staged).unwrap(), target);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_directory_publish_moves_a_complete_regular_tree() {
        let dir = temp_dir("publish-directory-no-replace");
        let work = dir.join("work");
        let staged = work.join("repaired-set");
        let dest = dir.join("repaired-set");
        std::fs::create_dir_all(staged.join("nested")).unwrap();
        std::fs::write(staged.join("first.bin"), b"first").unwrap();
        std::fs::write(staged.join("nested/second.bin"), b"second").unwrap();

        publish_directory_no_replace(&staged, &dest).unwrap();

        assert_eq!(std::fs::read(dest.join("first.bin")).unwrap(), b"first");
        assert_eq!(
            std::fs::read(dest.join("nested/second.bin")).unwrap(),
            b"second"
        );
        assert!(!staged.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_directory_publish_preserves_a_late_destination_conflict() {
        let dir = temp_dir("publish-directory-conflict");
        let work = dir.join("work");
        let staged = work.join("repaired-set");
        let dest = dir.join("repaired-set");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(staged.join("archive.bin"), b"repaired").unwrap();
        std::fs::write(dest.join("existing.bin"), b"existing").unwrap();

        let error = publish_directory_no_replace(&staged, &dest).unwrap_err();

        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(dest.as_path()));
        assert_eq!(
            std::fs::read(staged.join("archive.bin")).unwrap(),
            b"repaired"
        );
        assert_eq!(
            std::fs::read(dest.join("existing.bin")).unwrap(),
            b"existing"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_replace_directory_publish_rejects_symbolic_link_members() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("publish-directory-symlink");
        let staged = dir.join("staged");
        let dest = dir.join("repaired-set");
        let outside = dir.join("outside.bin");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, staged.join("archive.bin")).unwrap();

        let error = publish_directory_no_replace(&staged, &dest).unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidInput
        ));
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert!(std::fs::symlink_metadata(staged.join("archive.bin"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_no_replace_move_preserves_existing_destination() {
        let dir = temp_dir("windows-rename-no-replace");
        let staged = dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();
        std::fs::write(&dest, b"existing payload").unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing payload");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new payload");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_move_supports_directories_and_preserves_conflicts() {
        let dir = temp_dir("directory-move-no-replace");
        let staged = dir.join("staged");
        let dest = dir.join("destination");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("source.txt"), b"source directory").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("existing.txt"), b"existing directory").unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(staged.join("source.txt")).unwrap(),
            b"source directory"
        );
        assert_eq!(
            std::fs::read(dest.join("existing.txt")).unwrap(),
            b"existing directory"
        );

        std::fs::remove_dir_all(&dest).unwrap();
        move_path_no_replace(&staged, &dest).unwrap();
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read(dest.join("source.txt")).unwrap(),
            b"source directory"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_replace_move_moves_a_symbolic_link_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink-move-no-replace");
        let staged = dir.join("staged-link");
        let dest = dir.join("destination-link");
        symlink("source-target", &staged).unwrap();
        symlink("existing-target", &dest).unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_link(&staged).unwrap(),
            Path::new("source-target")
        );
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            Path::new("existing-target")
        );

        std::fs::remove_file(&dest).unwrap();
        move_path_no_replace(&staged, &dest).unwrap();
        assert!(matches!(
            std::fs::symlink_metadata(&staged),
            Err(ref error) if error.kind() == io::ErrorKind::NotFound
        ));
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            Path::new("source-target")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimate_create_inputs_counts_and_applies_excludes() {
        let dir = temp_dir("estimate");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("notes.tmp"), b"skip").unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), b"skip").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let estimate = engine
            .estimate_create_inputs(
                std::slice::from_ref(&root),
                &["node_modules".to_owned(), "*.tmp".to_owned()],
            )
            .unwrap();
        assert_eq!(estimate.input_count, 1);
        assert_eq!(estimate.files, 1);
        assert_eq!(estimate.directories, 2);
        assert_eq!(estimate.entries, 3);
        assert_eq!(estimate.total_bytes, b"fn main() {}".len() as u64);
        assert_eq!(
            estimate.output_budget_bytes(),
            b"fn main() {}".len() as u64 + 1024 * 1024 + 3 * 1024 + 4096 + 1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimate_create_inputs_excludes_source_cleanup_holders() {
        let dir = temp_dir("estimate-source-cleanup-holders");
        let root = dir.join("project");
        let active = root.join(format!(".squallz-trash-hold-{}-2", std::process::id()));
        let stale = root.join(".squallz-trash-hold-424242-10");
        let similar = root.join(".squallz-trash-hold-42-8-notes");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&similar).unwrap();
        std::fs::write(active.join("active.bin"), b"active").unwrap();
        std::fs::write(stale.join("stale.bin"), b"stale").unwrap();
        std::fs::write(similar.join("similar.bin"), b"similar").unwrap();
        std::fs::write(root.join("payload.bin"), b"payload").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let estimate = engine
            .estimate_create_inputs(std::slice::from_ref(&root), &[])
            .unwrap();

        assert_eq!(estimate.input_count, 1);
        assert_eq!(estimate.entries, 4);
        assert_eq!(estimate.directories, 2);
        assert_eq!(estimate.files, 2);
        assert_eq!(estimate.total_bytes, b"similarpayload".len() as u64);

        let error = engine
            .estimate_create_inputs(std::slice::from_ref(&active), &[])
            .unwrap_err();
        assert!(matches!(
            error,
            FormatError::Unsupported(message)
                if message.contains("internal transaction artifact cannot be archived directly")
        ));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimate_create_inputs_reports_scan_progress() {
        let dir = temp_dir("estimate-progress");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("notes.tmp"), b"skip").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let mut progress = Vec::new();
        let estimate = engine
            .estimate_create_inputs_with_progress(
                std::slice::from_ref(&root),
                &["*.tmp".to_owned()],
                |count, path| progress.push((count, path.to_owned())),
            )
            .unwrap();

        assert_eq!(estimate.entries, 3);
        assert_eq!(
            progress,
            vec![
                (1, "project".to_owned()),
                (2, "project/src".to_owned()),
                (3, "project/src/main.rs".to_owned())
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_aware_estimate_excludes_existing_split_and_recovery_family() {
        let dir = temp_dir("estimate-output-family");
        let root = dir.join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("payload.bin"), b"payload").unwrap();
        std::fs::write(root.join("backup.sqz"), vec![1; 128]).unwrap();
        std::fs::write(root.join("backup.sqz.001"), vec![2; 256]).unwrap();
        std::fs::write(root.join("backup.sqz.002.part"), vec![3; 512]).unwrap();
        std::fs::write(root.join("backup.sqz.rev001"), vec![4; 1024]).unwrap();
        std::fs::write(root.join("backup.sqz.rev002.part"), vec![5; 2048]).unwrap();
        std::fs::write(root.join("backup.sqz.rev000"), b"keep").unwrap();

        let output = root.join("backup.sqz");
        let estimate = Engine::new(FormatRegistry::new())
            .estimate_create_inputs_for_output(std::slice::from_ref(&root), &[], &output, true)
            .unwrap();

        assert_eq!(estimate.files, 2);
        assert_eq!(
            estimate.total_bytes,
            b"payload".len() as u64 + b"keep".len() as u64
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_aware_estimate_rejects_explicit_output_bundle_child() {
        let dir = temp_dir("estimate-output-child");
        let output = dir.join("Archive.app");
        let child = output.join("Contents/Resources/source.bin");
        std::fs::create_dir_all(child.parent().unwrap()).unwrap();
        std::fs::write(&child, b"source").unwrap();

        let error = Engine::new(FormatRegistry::new())
            .estimate_create_inputs_for_output(std::slice::from_ref(&child), &[], &output, false)
            .unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert_eq!(std::fs::read(&child).unwrap(), b"source");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_aware_split_estimate_rejects_a_directory_base() {
        let dir = temp_dir("estimate-split-directory");
        let input = dir.join("source.bin");
        let output = dir.join("archive.zip");
        std::fs::write(&input, b"source").unwrap();
        std::fs::create_dir(&output).unwrap();

        let error = Engine::new(FormatRegistry::new())
            .estimate_create_inputs_for_output(std::slice::from_ref(&input), &[], &output, true)
            .unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert!(output.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_duplicate_files_groups_by_size_and_hash_with_excludes() {
        let dir = temp_dir("duplicates");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("src/a.bin"), b"same payload").unwrap();
        std::fs::write(root.join("src/b.bin"), b"same payload").unwrap();
        std::fs::write(root.join("src/c.bin"), b"same length!").unwrap();
        std::fs::write(root.join("cache/d.bin"), b"same payload").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .find_duplicate_files(std::slice::from_ref(&root), &["cache".to_owned()], 1)
            .unwrap();

        assert_eq!(report.input_count, 1);
        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.duplicate_groups(), 1);
        assert_eq!(report.duplicate_files(), 2);
        assert_eq!(report.reclaimable_bytes(), b"same payload".len() as u64);
        assert_eq!(report.groups[0].paths.len(), 2);
        assert!(report.groups[0]
            .paths
            .iter()
            .any(|path| path.ends_with("src/a.bin")));
        assert!(report.groups[0]
            .paths
            .iter()
            .any(|path| path.ends_with("src/b.bin")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_files_hashes_files_with_shared_excludes() {
        let dir = temp_dir("checksum");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"abc").unwrap();
        std::fs::write(root.join("target/ignored.txt"), b"ignore").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .checksum_files(
                std::slice::from_ref(&root),
                &["target".to_owned()],
                ChecksumAlgorithm::Sha256,
            )
            .unwrap();

        assert_eq!(report.algorithm, ChecksumAlgorithm::Sha256);
        assert_eq!(report.input_count, 1);
        assert_eq!(report.files_hashed, 1);
        assert_eq!(report.bytes_hashed, 3);
        assert_eq!(
            report.items[0].digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(report.items[0].path.ends_with("src/a.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_checksum_manifest_reports_matches_and_mismatches() {
        let dir = temp_dir("checksum-verify");
        std::fs::write(dir.join("good.txt"), b"abc").unwrap();
        std::fs::write(dir.join("bad.txt"), b"changed").unwrap();
        std::fs::write(
            dir.join("SHA256SUMS"),
            concat!(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  good.txt\n",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  bad.txt\n",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  missing.txt\n",
            ),
        )
        .unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .verify_checksum_manifest(&dir.join("SHA256SUMS"), ChecksumAlgorithm::Sha256)
            .unwrap();

        assert!(!report.is_ok());
        assert_eq!(report.checked, 3);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 2);
        assert_eq!(
            report.items[0].actual.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(report.items[1].actual.is_some());
        assert!(report.items[2].error.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
