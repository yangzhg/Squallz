//! The two core abstractions: single-stream compressors and archive
//! containers, plus their reader/writer handles.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::entry::{EntryMeta, EntryPath};
use crate::error::FormatError;
use crate::options::{
    CompressionLevel, CreateOptions, ExtractOptions, FormatCapabilities, FormatCreateBudget,
    OpenOptions, ResourceOptions, SafetyLimits, TestReport, TestSummary, UpdateOp,
};
use crate::progress::{ControlToken, ProgressSink};
use crate::safety::LimitsAccountant;

/// Chunk size of the streaming pumps; cancellation, guardrails and progress
/// are honoured at this granularity.
const STREAM_CHUNK: usize = 64 * 1024;

/// Readable, seekable input stream.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Stable identity of an already-opened physical source file.
///
/// The engine captures this from the open file handle before a format uses
/// the path to discover sibling files. Path-aware formats compare it with a
/// no-follow open of the current directory entry before trusting that hint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalFileIdentity {
    filesystem: u64,
    file: u64,
}

impl PhysicalFileIdentity {
    /// Creates an identity from the platform filesystem and file identifiers.
    pub fn new(filesystem: u64, file: u64) -> Self {
        Self { filesystem, file }
    }

    /// Platform filesystem or volume identifier.
    pub fn filesystem(self) -> u64 {
        self.filesystem
    }

    /// Stable file identifier within [`Self::filesystem`].
    pub fn file(self) -> u64 {
        self.file
    }
}

/// Ordered physical files that make up one native archive volume set.
///
/// Members remain in native data order. The primary open volume can differ
/// from the first member, as it does for split ZIP (`.z01 … .zip`). Paths stay
/// backend-only; callers should expose only the minimum display information
/// needed by the UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveSourceSet {
    primary: PathBuf,
    members: Vec<PathBuf>,
}

impl ArchiveSourceSet {
    /// Builds a source set from members in native archive order.
    pub fn from_ordered_members(members: Vec<PathBuf>) -> Result<Self, FormatError> {
        let primary = members.first().cloned().ok_or_else(|| {
            FormatError::CorruptArchive("archive source set has no members".into())
        })?;
        Self::from_primary_and_ordered_members(primary, members)
    }

    /// Builds a source set whose preferred open volume is not necessarily the
    /// first physical member.
    pub fn from_primary_and_ordered_members(
        primary: PathBuf,
        members: Vec<PathBuf>,
    ) -> Result<Self, FormatError> {
        if members.is_empty() {
            return Err(FormatError::CorruptArchive(
                "archive source set has no members".into(),
            ));
        }
        if !members.iter().any(|member| member == &primary) {
            return Err(FormatError::CorruptArchive(
                "archive source set primary is not a member".into(),
            ));
        }
        Ok(Self { primary, members })
    }

    /// Primary volume used to open the native archive set.
    pub fn primary(&self) -> &Path {
        &self.primary
    }

    /// Physical members in native archive order.
    pub fn members(&self) -> &[PathBuf] {
        &self.members
    }
}

/// Writable, seekable output stream.
pub trait WriteSeek: Write + Seek + Send {}
impl<T: Write + Seek + Send> WriteSeek for T {}

/// Bounds declared by a format that can create interoperable native volumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeVolumeLimits {
    /// Smallest requested target size accepted by the format.
    pub min_volume_size: u64,
    /// Largest requested target size accepted by the format.
    ///
    /// Formats with indivisible resources may emit a physical member larger
    /// than this target when their specification leaves no safe split point.
    pub max_volume_size: u64,
    /// Maximum number of physical volumes emitted by this implementation.
    pub max_volumes: u32,
}

/// Conservative preflight estimate for one native multi-volume output set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeVolumeBudget {
    /// Estimated bytes occupied by every physical member together.
    pub output_bytes: u64,
    /// Estimated number of physical members.
    pub volume_count: u64,
}

/// Caller-owned native-volume sink.
///
/// The core owns file reservation, rollover, durable commit and cleanup. A
/// format owns its record layout and tells the sink which records must remain
/// on one physical volume.
pub trait NativeVolumeWriter: Send {
    /// Configured target bytes in one physical volume.
    fn volume_size(&self) -> u64;
    /// Returns the caller's preferred buffer size for format-owned copies.
    ///
    /// Sinks without a caller-level resource policy keep the format's
    /// requested default.
    fn stream_buffer_size(&self, default: usize) -> Result<usize, FormatError> {
        Ok(default)
    }
    /// Zero-based physical volume containing the next byte.
    fn disk_index(&self) -> u32;
    /// Byte offset of the next byte within the current physical volume.
    fn disk_offset(&self) -> u64;
    /// Starts a fresh volume when `record_len` would cross the current one.
    fn ensure_record_capacity(&mut self, record_len: u64) -> Result<(), FormatError>;
    /// Writes bytes, rolling over at the configured physical boundary.
    fn write_spanning(&mut self, bytes: &[u8]) -> Result<(), FormatError>;
    /// Starts a new format-defined physical member.
    ///
    /// This is used by formats whose encoder chooses complete member
    /// boundaries itself. Such a member may exceed [`Self::volume_size`] when
    /// the format contains an indivisible resource.
    fn begin_volume(&mut self) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(
            "this native volume sink does not accept format-defined boundaries".into(),
        ))
    }
    /// Writes bytes only to the current format-defined physical member.
    fn write_current_volume(&mut self, _bytes: &[u8]) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(
            "this native volume sink does not accept format-defined boundaries".into(),
        ))
    }
}

/// Write-side encoder handle produced by [`Compressor::compress_writer`].
/// `finish` flushes the trailing format structures (in place, so boxed
/// sinks can be finished through `dyn`).
pub trait CompressSink: Write + Send {
    /// Finishes the compressed stream. Must be called exactly once; the
    /// sink must not be written to afterwards.
    fn finish(&mut self) -> Result<(), FormatError>;
}

/// Factory producing fresh sequential streams over the same source, used by
/// [`ArchiveFormat::open_stream`]. Compound formats (`.tar.gz`) cannot seek
/// in the decompressed stream, but the engine can always restart it from the
/// underlying file; each call returns a new stream positioned at the start.
pub type StreamFactory = Box<dyn Fn() -> Result<Box<dyn Read + Send>, FormatError> + Send + Sync>;

/// File-system entries prepared by the engine for an archive update.
///
/// Formats consume these entries instead of reopening paths from
/// [`UpdateOp::Add`]. The engine keeps source identity checks, byte-count
/// validation, progress and cancellation on the same boundary as ordinary
/// archive creation.
pub trait PreparedUpdateAdditions: Send {
    /// Number of prepared entries, including directory and symbolic-link
    /// markers expanded from added directory trees.
    fn len(&self) -> usize;

    /// Whether this update has no additions.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Metadata for one prepared entry.
    fn meta(&self, index: usize) -> Option<&EntryMeta>;

    /// Streams one prepared entry into an update writer.
    fn add_entry(
        &mut self,
        index: usize,
        writer: &mut dyn ArchiveWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), FormatError>;
}

/// Abstraction one: single-stream compressor (gzip/bzip2/xz/zstd/lz4/brotli).
/// Has no notion of a "file list"; combined with tar it forms compound
/// formats such as `.tar.gz`.
///
/// Implementations provide the two stream-wrapping constructors
/// ([`Compressor::compress_writer`] / [`Compressor::decompress_reader`]);
/// the chunked pumps ([`Compressor::compress`] / [`Compressor::decompress`])
/// are derived from them, so every backend gets cancellation, progress and
/// the decompression-bomb guardrail for free.
pub trait Compressor: Send + Sync {
    /// Format identifier, e.g. `"gzip"`
    fn id(&self) -> &'static str;
    /// Extensions (without the dot), e.g. `["gz"]`
    fn extensions(&self) -> &'static [&'static str];

    /// Magic-number sniffing on the first bytes of a file. Used as the last
    /// detection step for extensionless compressed streams; formats without
    /// a reliable magic (brotli) keep the default `false`.
    fn sniff(&self, _head: &[u8]) -> bool {
        false
    }

    /// Wraps `dst` into an encoding sink. The caller streams plain data into
    /// the sink and must call [`CompressSink::finish`] at the end.
    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError>;

    /// Wraps `src` into a decoding reader yielding plain data. Output-byte
    /// guardrails are enforced by whoever consumes the reader (the pump
    /// below or the shared extraction sink).
    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError>;

    /// Best-effort uncompressed size of a compressed file (e.g. the gzip
    /// ISIZE trailer). Implementations must rewind `src` to the start.
    /// `None` when the format does not record it.
    fn uncompressed_size_hint(&self, _src: &mut dyn ReadSeek) -> Option<u64> {
        None
    }

    /// Compresses one stream in 64 KiB chunks. Progress is reported as
    /// `(consumed_input_bytes, 0)` — the total is unknown at this level and
    /// supplied by engine-side wrappers.
    fn compress(
        &self,
        src: &mut (dyn Read + Send),
        dst: &mut (dyn Write + Send),
        level: CompressionLevel,
        res: &ResourceOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut sink = self.compress_writer(Box::new(dst), level, res)?;
        let label = EntryPath::from_utf8("");
        let mut buf = vec![0u8; res.stream_buffer_size(STREAM_CHUNK)?];
        let mut done = 0u64;
        loop {
            ctl.checkpoint()?;
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n])?;
            done += n as u64;
            progress.on_progress(done, 0, &label);
        }
        sink.finish()
    }

    /// Decompresses one stream in 64 KiB chunks, charging every output byte
    /// against the guardrails. Progress is `(produced_output_bytes, 0)`.
    fn decompress(
        &self,
        src: &mut (dyn Read + Send),
        dst: &mut (dyn Write + Send),
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut reader = self.decompress_reader(Box::new(src))?;
        let mut accountant = LimitsAccountant::new(*limits);
        let label = EntryPath::from_utf8("");
        let mut buf = vec![0u8; STREAM_CHUNK];
        let mut done = 0u64;
        loop {
            ctl.checkpoint()?;
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            accountant.add_output_bytes(n as u64)?;
            dst.write_all(&buf[..n])?;
            done += n as u64;
            progress.on_progress(done, 0, &label);
        }
    }
}

/// Abstraction two: archive container (zip/tar/7z/rar/iso/...).
pub trait ArchiveFormat: Send + Sync {
    /// Format identifier, e.g. `"zip"`
    fn id(&self) -> &'static str;
    /// Extensions (without the dot) including aliases, e.g.
    /// `["zip", "jar", "apk", "cbz"]`
    fn extensions(&self) -> &'static [&'static str];
    /// Capability declaration
    fn capabilities(&self) -> FormatCapabilities;
    /// Validates extension-specific creation boundaries before inputs are
    /// scanned or an output is reserved.
    fn validate_create_name(&self, _name: &str) -> Result<(), FormatError> {
        Ok(())
    }
    /// Validates name and option combinations before inputs are scanned.
    ///
    /// Existing formats keep their name-only validation through the default.
    fn validate_create_options(
        &self,
        name: &str,
        _opts: &CreateOptions,
    ) -> Result<(), FormatError> {
        self.validate_create_name(name)
    }
    /// Magic-number sniffing. `head` holds up to 512 bytes from the start of
    /// the file (the tar `ustar` magic lives at offset 257), `tail` up to
    /// 64 bytes from the end (ZIP keeps its central directory there and SFX
    /// archives start with MZ, hence both windows).
    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool;
    /// Opens for reading.
    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError>;
    /// Controlled variant of [`ArchiveFormat::open`].
    ///
    /// The default preserves existing format implementations and checks the
    /// token around their open call. Formats that wait on an external decoder
    /// or another blocking backend should override this method and propagate
    /// `ctl` into that wait.
    fn open_with_control(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        ctl.checkpoint()?;
        let reader = self.open(src, opts)?;
        ctl.checkpoint()?;
        Ok(reader)
    }
    /// Opens one physical archive file while retaining its source path as a
    /// hint for formats whose native layout spans sibling files.
    ///
    /// `src` is the authoritative content of the already-opened selected
    /// file. Implementations must not reopen `source_path` in place of it.
    /// The default keeps existing single-stream format implementations
    /// source-compatible.
    fn open_file(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        let _ = source_path;
        let _ = source_identity;
        self.open(src, opts)
    }
    /// Controlled variant of [`ArchiveFormat::open_file`].
    ///
    /// Formats that override `open_file` with a blocking backend should also
    /// override this method so cancellation reaches that backend.
    fn open_file_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        ctl.checkpoint()?;
        let reader = self.open_file(source_path, source_identity, src, opts)?;
        ctl.checkpoint()?;
        Ok(reader)
    }
    /// Detects a native archive volume set without opening an extraction
    /// backend or copying complete volume contents.
    ///
    /// `src` is the authoritative already-opened selected file and must remain
    /// usable after this call. Implementations that inspect sibling paths must
    /// validate their headers and stable file identities before returning a
    /// set. Single-file formats keep the default `None`.
    fn probe_file_source_set(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        let _ = source_path;
        let _ = source_identity;
        let _ = src;
        Ok(None)
    }
    /// Controlled variant of [`ArchiveFormat::probe_file_source_set`].
    ///
    /// Formats that enumerate or inspect sibling volumes should override this
    /// method so pause and cancellation are observed between filesystem calls.
    fn probe_file_source_set_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
        ctl: &ControlToken,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        ctl.checkpoint()?;
        let source_set = self.probe_file_source_set(source_path, source_identity, src)?;
        ctl.checkpoint()?;
        Ok(source_set)
    }
    /// Opens for reading from a restartable sequential stream (no `Seek`).
    /// This is how compound formats (`.tar.gz`) are read without a temp
    /// file: the engine hands a factory that re-creates the decompressed
    /// stream on demand. Formats that require random access keep the
    /// default `Unsupported`.
    fn open_stream(
        &self,
        _source: StreamFactory,
        _opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot read from a non-seekable stream",
            self.id()
        )))
    }
    /// Creates for writing (returns `Unsupported` when `can_create=false`).
    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError>;
    /// Controlled variant of [`ArchiveFormat::create`].
    ///
    /// Formats whose writer performs blocking setup or finalization should
    /// retain a clone of `ctl` in the returned writer.
    fn create_with_control(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        ctl.checkpoint()?;
        let writer = self.create(dst, opts)?;
        ctl.checkpoint()?;
        Ok(writer)
    }
    /// Declares support and physical bounds for native multi-volume creation.
    fn native_volume_limits(&self) -> Option<NativeVolumeLimits> {
        None
    }
    /// Returns the physical member users and external tools should open first.
    ///
    /// The default matches native ZIP, whose final member is the entry point.
    fn native_volume_primary_index(&self, volume_count: u32) -> Result<u32, FormatError> {
        volume_count
            .checked_sub(1)
            .ok_or_else(|| FormatError::Other("native volume writer produced no output".into()))
    }
    /// Estimates the final native volume set for preflight and disk guards.
    fn native_volume_budget(
        &self,
        archive_bytes: u64,
        entry_count: u64,
        volume_size: u64,
    ) -> Result<NativeVolumeBudget, FormatError> {
        let limits = self.native_volume_limits().ok_or_else(|| {
            FormatError::Unsupported(format!(
                "format {} does not support native volume creation",
                self.id()
            ))
        })?;
        if volume_size == 0 {
            return Err(FormatError::Unsupported(
                "native volume size must be greater than zero".into(),
            ));
        }
        // Fixed-boundary formats may replace their end records and add a
        // spanning marker. This includes the largest legal ZIP comment.
        let output_bytes = archive_bytes.saturating_add(128 * 1024);
        let byte_volumes = output_bytes
            .saturating_add(volume_size - 1)
            .checked_div(volume_size)
            .unwrap_or(0)
            .max(1);
        let record_rollovers = entry_count.saturating_mul(2).saturating_add(3);
        let volume_count = byte_volumes
            .saturating_add(record_rollovers)
            .min(u64::from(limits.max_volumes));
        Ok(NativeVolumeBudget {
            output_bytes,
            volume_count,
        })
    }
    /// Maps a zero-based native volume index to its final sibling path.
    ///
    /// `final_volume` identifies the member users and external tools should
    /// open first. Implementations must return a sibling of `destination`.
    fn native_volume_path(
        &self,
        _destination: &Path,
        _disk_index: u32,
        _final_volume: bool,
    ) -> Result<PathBuf, FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} does not support native volume creation",
            self.id()
        )))
    }
    /// Rewrites one complete archive into the format's native volume layout.
    ///
    /// The source is the caller-owned, fully written single-file archive.
    /// Implementations must stream payload data and must not reopen paths.
    fn write_native_volumes(
        &self,
        _source: &mut dyn ReadSeek,
        _output: &mut dyn NativeVolumeWriter,
        _progress: &dyn ProgressSink,
        _ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} does not support native volume creation",
            self.id()
        )))
    }
    /// Refines core's generic archive bound for format-specific layout and
    /// temporary-file requirements.
    fn create_budget(
        &self,
        _content_bytes: u64,
        archive_bytes: u64,
        _opts: &CreateOptions,
    ) -> Result<FormatCreateBudget, FormatError> {
        Ok(FormatCreateBudget::direct(archive_bytes))
    }
    /// Creates for writing into a forward-only sink (no `Seek`). This is how
    /// compound formats (`.tar.gz`) are written without a temp file: the
    /// destination is a live compression stream. Formats that must seek
    /// while writing keep the default `Unsupported`.
    fn create_stream(
        &self,
        _dst: Box<dyn Write + Send>,
        _opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot write to a non-seekable stream",
            self.id()
        )))
    }
    /// Append/delete/rename (returns `Unsupported` when `can_update=false`).
    /// Implementation contract: write to a temporary file + atomic rename,
    /// pre-check disk space.
    fn update(
        &self,
        _src: &Path,
        _ops: &[UpdateOp],
        _opts: &CreateOptions,
        _progress: &dyn ProgressSink,
        _ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot update existing archives",
            self.id()
        )))
    }

    /// Whether this implementation consumes engine-prepared additions.
    /// Formats that keep the default return value continue to receive the
    /// original [`UpdateOp`] path contract without an extra input scan.
    fn accepts_prepared_update_additions(&self) -> bool {
        false
    }

    /// Updates an archive using additions already bound to their source
    /// objects by the engine. Existing format implementations keep their
    /// previous behavior through the default forwarding implementation.
    fn update_with_prepared_additions(
        &self,
        src: &Path,
        ops: &[UpdateOp],
        _additions: &mut dyn PreparedUpdateAdditions,
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.update(src, ops, opts, progress, ctl)
    }

    /// Whether this implementation can rewrite an update into caller-owned
    /// streams. The caller retains responsibility for locking, source
    /// identity checks, durable staging and committing the replacement.
    fn supports_update_rewrite(&self) -> bool {
        false
    }

    /// Estimated staging space next to the target for a caller-managed update
    /// rewrite. This is an early disk-space guard, not a promise that the
    /// encoder cannot exceed it. Formats can add container-specific overhead
    /// to the source and uncompressed addition sizes.
    fn estimate_update_staging_bytes(
        &self,
        source_bytes: u64,
        addition_bytes: u64,
        _opts: &CreateOptions,
    ) -> Result<u64, FormatError> {
        Ok(source_bytes.saturating_add(addition_bytes))
    }

    /// Rewrites an archive update using only the caller-owned streams. The
    /// implementation must not reopen or commit either stream by path.
    #[allow(clippy::too_many_arguments)]
    fn rewrite_update(
        &self,
        _source: Box<dyn ReadSeek>,
        _output: Box<dyn WriteSeek>,
        _ops: &[UpdateOp],
        _additions: &mut dyn PreparedUpdateAdditions,
        _opts: &CreateOptions,
        _progress: &dyn ProgressSink,
        _ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot rewrite archive updates into caller-owned streams",
            self.id()
        )))
    }
}

/// Read handle of an opened archive.
pub trait ArchiveReader: Send {
    /// Native physical volume set backing this opened reader, when known.
    ///
    /// Generic byte-split streams are tracked by the engine instead. Formats
    /// return this only after validating the native container headers.
    fn source_set(&self) -> Option<&ArchiveSourceSet> {
        None
    }

    /// Revalidates any original physical sources retained by this reader.
    ///
    /// Readers backed by private staging should compare the current paths with
    /// the identities captured while staging. The engine calls this before and
    /// after each source-state snapshot so it cannot combine staged content
    /// from one object generation with path metadata from another.
    fn verify_source_set(&self, _ctl: &ControlToken) -> Result<(), FormatError> {
        Ok(())
    }

    /// Streams entry metadata (huge archives are never loaded wholesale).
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_>;

    /// Transfers entry metadata out of a reader that will not be used again.
    ///
    /// The default preserves compatibility by visiting [`ArchiveReader::entries`].
    /// Readers that retain decoded path tables can override this method and
    /// move those paths into the visitor instead of cloning them.
    fn consume_entries(
        mut self: Box<Self>,
        visitor: &mut dyn FnMut(EntryMeta) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        for entry in self.entries() {
            visitor(entry?)?;
        }
        Ok(())
    }

    /// Extracts all entries (or a selection) into `dest`.
    ///
    /// The default implementation is the shared safe extraction engine
    /// ([`crate::extract_entries`]): Zip-Slip rejection, decompression-bomb
    /// guardrails, symlink-breakout protection, overwrite/symlink policies
    /// and permission restore. Formats may override it for performance, but
    /// any override must uphold the same safety guarantees.
    fn extract(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        crate::extract::extract_entries(self, dest, selection, opts, progress, ctl)
    }

    /// Extracts entries and returns completed per-entry outcome counts.
    ///
    /// This report-returning variant preserves [`ArchiveReader::extract`] for
    /// callers that do not need outcome details. Performance-oriented readers
    /// should override both methods when their extraction path is single-pass.
    fn extract_with_report(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<crate::ExtractReport, FormatError> {
        crate::extract::extract_entries_with_report(self, dest, selection, opts, progress, ctl)
    }

    /// Streams a single entry (GUI preview, nested archives, format
    /// conversion).
    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError>;

    /// Compatibility integrity test returning the complete problem list.
    /// Product frontends should prefer [`ArchiveReader::test_summary`].
    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError>;

    /// Integrity test with an exact problem count and bounded diagnostic
    /// preview.
    ///
    /// The compatibility default adapts [`ArchiveReader::test`]. Readers
    /// that can encounter many independent problems should override this
    /// method and collect directly into a bounded log.
    fn test_summary(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        self.test(progress, ctl).map(TestSummary::from)
    }
}

/// Write handle of an archive being created.
pub trait ArchiveWriter: Send {
    /// Writes one entry; pass `None` for data-less entries
    /// (directories/symlinks).
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError>;
    /// Finishes writing (flushes trailing structures such as the central
    /// directory).
    fn finish(self: Box<Self>) -> Result<(), FormatError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyArchiveFormat;
    struct EmptyPreparedAdditions;
    struct LegacyTestReader;

    impl ArchiveReader for LegacyTestReader {
        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            Box::new(std::iter::empty())
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            Err(FormatError::Unsupported("legacy test reader".into()))
        }

        fn test(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestReport, FormatError> {
            Ok(TestReport {
                entries_tested: 25,
                problems: (0..25).map(|index| format!("problem-{index}")).collect(),
                recovery: None,
            })
        }
    }

    #[test]
    fn test_summary_default_adapts_legacy_reader() {
        let mut reader = LegacyTestReader;
        let summary = reader
            .test_summary(&crate::NoProgress, &ControlToken::new())
            .unwrap();

        assert_eq!(summary.entries_tested, 25);
        assert_eq!(summary.problems.total, 25);
        assert_eq!(
            summary.problems.messages.len(),
            crate::TEST_PROBLEM_PREVIEW_LIMIT
        );
        assert_eq!(summary.problems.omitted(), 5);
    }

    #[test]
    fn source_set_can_keep_native_order_with_a_different_primary() {
        let first = PathBuf::from("archive.z01");
        let primary = PathBuf::from("archive.zip");
        let set = ArchiveSourceSet::from_primary_and_ordered_members(
            primary.clone(),
            vec![first.clone(), primary.clone()],
        )
        .unwrap();

        assert_eq!(set.primary(), primary);
        assert_eq!(set.members(), &[first, primary]);
        assert!(ArchiveSourceSet::from_primary_and_ordered_members(
            PathBuf::from("other.zip"),
            vec![PathBuf::from("archive.z01"), PathBuf::from("archive.zip")],
        )
        .is_err());
    }

    impl PreparedUpdateAdditions for EmptyPreparedAdditions {
        fn len(&self) -> usize {
            0
        }

        fn meta(&self, _index: usize) -> Option<&EntryMeta> {
            None
        }

        fn add_entry(
            &mut self,
            _index: usize,
            _writer: &mut dyn ArchiveWriter,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
            _completed_bytes: u64,
            _total_bytes: u64,
        ) -> Result<(), FormatError> {
            Err(FormatError::Other(
                "empty prepared additions cannot write an entry".into(),
            ))
        }
    }

    impl ArchiveFormat for DummyArchiveFormat {
        fn id(&self) -> &'static str {
            "dummy"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["dummy"]
        }

        fn capabilities(&self) -> FormatCapabilities {
            FormatCapabilities::default()
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("dummy open".to_string()))
        }

        fn create(
            &self,
            _dst: Box<dyn WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("dummy create".to_string()))
        }
    }

    fn unsupported_message(result: Result<(), FormatError>) -> String {
        match result {
            Err(FormatError::Unsupported(message)) => message,
            other => panic!("expected unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn archive_format_default_stream_and_update_errors_name_the_format() {
        let format = DummyArchiveFormat;

        assert!(!format.accepts_prepared_update_additions());
        assert!(!format.supports_update_rewrite());

        let open_message = match format.open_stream(
            Box::new(|| Ok(Box::new(std::io::empty()) as Box<dyn Read + Send>)),
            &OpenOptions::default(),
        ) {
            Err(FormatError::Unsupported(message)) => message,
            _ => panic!("expected unsupported open_stream error"),
        };
        assert!(open_message.contains("dummy"));
        assert!(open_message.contains("non-seekable stream"));

        let create_message =
            match format.create_stream(Box::new(Vec::<u8>::new()), &CreateOptions::default()) {
                Err(FormatError::Unsupported(message)) => message,
                _ => panic!("expected unsupported create_stream error"),
            };
        assert!(create_message.contains("dummy"));
        assert!(create_message.contains("non-seekable stream"));

        let update_message = unsupported_message(format.update(
            Path::new("archive.dummy"),
            &[],
            &CreateOptions::default(),
            &crate::NoProgress,
            &ControlToken::default(),
        ));
        assert!(update_message.contains("dummy"));
        assert!(update_message.contains("update existing archives"));

        let mut additions = EmptyPreparedAdditions;
        let prepared_message = unsupported_message(format.update_with_prepared_additions(
            Path::new("archive.dummy"),
            &[],
            &mut additions,
            &CreateOptions::default(),
            &crate::NoProgress,
            &ControlToken::default(),
        ));
        assert_eq!(prepared_message, update_message);

        let rewrite_message = unsupported_message(format.rewrite_update(
            Box::new(std::io::Cursor::new(Vec::<u8>::new())),
            Box::new(std::io::Cursor::new(Vec::<u8>::new())),
            &[],
            &mut additions,
            &CreateOptions::default(),
            &crate::NoProgress,
            &ControlToken::default(),
        ));
        assert!(rewrite_message.contains("dummy"));
        assert!(rewrite_message.contains("caller-owned streams"));
        assert_eq!(
            format
                .estimate_update_staging_bytes(128, 64, &CreateOptions::default())
                .unwrap(),
            192
        );
    }

    #[test]
    fn archive_format_default_file_open_forwards_to_stream_open() {
        let format = DummyArchiveFormat;
        let error = match format.open_file(
            Path::new("archive.dummy"),
            None,
            Box::new(std::io::Cursor::new(Vec::<u8>::new())),
            &OpenOptions::default(),
        ) {
            Ok(_) => panic!("dummy file open should forward to its unsupported stream open"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            FormatError::Unsupported(message) if message == "dummy open"
        ));
    }

    #[test]
    fn archive_format_default_source_probe_observes_control() {
        let format = DummyArchiveFormat;
        let control = ControlToken::default();
        control.cancel();
        let mut source = std::io::Cursor::new(Vec::<u8>::new());

        let result = format.probe_file_source_set_with_control(
            Path::new("archive.dummy"),
            None,
            &mut source,
            &control,
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
    }
}
