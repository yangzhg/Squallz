#![deny(unsafe_code)]
//! squallz-format-api: unified abstractions for the format layer.
//!
//! Design principles:
//! - Two abstractions: [`Compressor`] (single stream, gzip/zstd/...) and
//!   [`ArchiveFormat`] (container, zip/tar/7z/...). Compound formats
//!   (`.tar.gz`) are detected by the registry as "outer compressor + inner
//!   archive".
//! - Interfaces operate on `Read + Seek` streams rather than paths, enabling
//!   nested archives and in-memory sources.
//! - Entry names keep their raw bytes as the source of truth
//!   ([`EntryPath::raw`]); the display name is decoded per encoding, which
//!   handles legacy encodings (CP936 etc.).
//! - Progress reporting is shareable across threads; cancellation and pausing
//!   go through [`ControlToken`].
//! - The safe extraction engine ([`extract_entries`]) lives here so every
//!   archive format gets Zip-Slip/zip-bomb/symlink-breakout protection for
//!   free via the default [`ArchiveReader::extract`] implementation.

mod entry;
mod error;
mod extract;
mod file_ops;
mod links;
mod options;
mod progress;
mod registry;
mod safety;
mod testing;
mod traits;

pub use entry::{unix_seconds, EntryMeta, EntryPath, EntryType};
pub use error::FormatError;
pub use extract::{
    empty_extract_report, extract_entries, extract_entries_with_report,
    first_free_numbered_sibling_name, ExtractReport, ExtractSink,
};
#[cfg(windows)]
pub use file_ops::reopen_readonly_file;
pub use file_ops::{atomic_replace_file, move_path_no_replace};
pub use options::{
    BoundedProblemLog, CompressionLevel, ConflictDecision, ConflictResolver, CreateOptions,
    EntrySelection, ExtractOptions, ExtractProblemReporter, FormatCapabilities, FormatCreateBudget,
    OpenOptions, OverwritePolicy, Password, ProblemPreview, RecoverySummary, ResourceOptions,
    SafetyLimits, SplitOutputMode, SqzCreateOptions, SqzInnerFormat, SymlinkPolicy, TestSummary,
    UpdateOp, UpdateOptions, EXTRACT_PROBLEM_PREVIEW_LIMIT, TEST_PROBLEM_PREVIEW_LIMIT,
};
pub use progress::{ControlToken, NoProgress, ProgressPhase, ProgressSink};
pub use registry::{split_volume_name, Detected, FormatInfo, FormatKind, FormatRegistry};
pub use safety::{check_windows_portability, sanitize_entry_path, LimitsAccountant};
pub use testing::test_entry_data;
pub use traits::{
    ArchiveFormat, ArchiveReader, ArchiveSourceSet, ArchiveStructureStatus, ArchiveWriter,
    CompressSink, Compressor, EntryStreamConsumer, NativeVolumeBudget, NativeVolumeLimits,
    NativeVolumeWriter, PhysicalFileIdentity, PreparedUpdateAdditions, ReadSeek, StreamFactory,
    WriteSeek,
};
