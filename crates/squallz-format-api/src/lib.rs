#![forbid(unsafe_code)]
//! squallz-format-api: unified abstractions for the format layer.
//!
//! Design principles (see PLAN.md §3.3):
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
mod links;
mod options;
mod progress;
mod registry;
mod safety;
mod traits;

pub use entry::{EntryMeta, EntryPath, EntryType};
pub use error::FormatError;
pub use extract::{extract_entries, ExtractSink};
pub use options::{
    CompressionLevel, ConflictDecision, ConflictResolver, CreateOptions, ExtractOptions,
    ExtractProblemReporter, FormatCapabilities, OpenOptions, OverwritePolicy, Password,
    RecoverySummary, ResourceOptions, SafetyLimits, SqzCreateOptions, SymlinkPolicy, TestReport,
    UpdateOp,
};
pub use progress::{ControlToken, NoProgress, ProgressSink};
pub use registry::{split_volume_name, Detected, FormatInfo, FormatKind, FormatRegistry};
pub use safety::{check_windows_portability, sanitize_entry_path, LimitsAccountant};
pub use traits::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CompressSink, Compressor, ReadSeek, StreamFactory,
    WriteSeek,
};
