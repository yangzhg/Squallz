//! Options, policies and capability declarations.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zeroize::Zeroizing;

use crate::entry::{EntryMeta, EntryPath};
use crate::error::FormatError;

/// Password. Wrapped with zeroize in memory (cleared on drop); `Debug`
/// output is redacted.
#[derive(Clone)]
pub struct Password(Zeroizing<String>);

impl Password {
    /// Constructs a password.
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }

    /// Exposes the plaintext (only at the moment it is handed to a backend).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

/// Overwrite policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverwritePolicy {
    /// Overwrite existing files
    Overwrite,
    /// Skip existing files (the safe default)
    #[default]
    Skip,
    /// Keep both (auto-rename to `name (1).ext`)
    RenameBoth,
    /// Ask through [`ConflictResolver`]; degrades to Skip when no resolver
    /// is provided
    Ask,
}

/// Symbolic link handling policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SymlinkPolicy {
    /// Restore as a symbolic link (Unix default)
    #[default]
    Preserve,
    /// Follow: extract a copy of the link target's content
    Follow,
    /// Ignore link entries
    Skip,
}

/// Decision produced by a conflict prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictDecision {
    /// Overwrite this entry
    Overwrite,
    /// Skip this entry
    Skip,
    /// Keep both under the given new name
    Rename(String),
    /// Abort the whole operation
    Abort,
}

/// Callback for the `Ask` overwrite policy: the CLI wires it to stdin, the
/// GUI to a dialog.
pub trait ConflictResolver: Send + Sync {
    /// Produces a decision for a single conflict.
    fn resolve(&self, existing: &Path, incoming: &EntryMeta) -> ConflictDecision;
}

/// Callback for non-fatal skipped entries during best-effort extraction.
pub trait ExtractProblemReporter: Send + Sync {
    /// Records one entry that could not be read or verified.
    fn skipped_entry(&self, path: &EntryPath, error: &FormatError);
}

/// Decompression-bomb guardrails. Exceeding a limit returns
/// [`crate::FormatError::ResourceLimitExceeded`].
#[derive(Debug, Clone, Copy)]
pub struct SafetyLimits {
    /// Upper bound on total extracted bytes
    pub max_output_bytes: u64,
    /// Upper bound on the number of entries
    pub max_entries: u64,
    /// Per-entry upper bound on the uncompressed/compressed ratio
    /// (legitimately high ratios — e.g. all-zero files — may need a larger
    /// value)
    pub max_compression_ratio: u32,
}

impl Default for SafetyLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 256 * 1024 * 1024 * 1024, // 256 GiB
            max_entries: 1_000_000,
            max_compression_ratio: 2048,
        }
    }
}

/// Thread and memory resource configuration.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceOptions {
    /// Worker thread count (`None` = automatic)
    pub threads: Option<usize>,
    /// Buffer memory cap in bytes (`None` = automatic)
    pub memory_limit: Option<u64>,
}

impl ResourceOptions {
    /// Smallest buffer budget Squallz will accept for stream pumps.
    pub const MIN_STREAM_BUFFER_BYTES: u64 = 8 * 1024;

    /// Returns a copy buffer size bounded by `memory_limit`.
    ///
    /// This controls Squallz-owned streaming buffers. Backend encoders may
    /// still keep their own dictionaries; callers must not present this as a
    /// whole-process RSS cap.
    pub fn stream_buffer_size(&self, default: usize) -> Result<usize, FormatError> {
        let Some(limit) = self.memory_limit else {
            return Ok(default);
        };
        if limit < Self::MIN_STREAM_BUFFER_BYTES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "memory limit {limit} bytes is below the {} byte streaming buffer minimum",
                Self::MIN_STREAM_BUFFER_BYTES
            )));
        }
        Ok(default.min(limit.min(usize::MAX as u64) as usize).max(1))
    }
}

/// Options for opening an archive.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Decryption password
    pub password: Option<Password>,
    /// Manually selected entry-name encoding (e.g. `"gbk"`); `None` = auto
    /// detection
    pub encoding_override: Option<String>,
}

/// Extraction options.
#[derive(Clone)]
pub struct ExtractOptions {
    /// Overwrite policy
    pub overwrite: OverwritePolicy,
    /// Callback for the `Ask` policy
    pub resolver: Option<Arc<dyn ConflictResolver>>,
    /// Symbolic link policy
    pub symlinks: SymlinkPolicy,
    /// Whether to restore Unix permission bits
    pub restore_permissions: bool,
    /// Decompression-bomb guardrails
    pub limits: SafetyLimits,
    /// Threads and memory
    pub resources: ResourceOptions,
    /// Continue past per-entry read/integrity failures when possible.
    ///
    /// This never weakens password, cancellation, path-safety, symlink,
    /// resource-limit or disk errors; those remain fatal.
    pub best_effort: bool,
    /// Receives skipped-entry problems when [`Self::best_effort`] is true.
    pub problem_reporter: Option<Arc<dyn ExtractProblemReporter>>,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            overwrite: OverwritePolicy::default(),
            resolver: None,
            symlinks: SymlinkPolicy::default(),
            restore_permissions: true,
            limits: SafetyLimits::default(),
            resources: ResourceOptions::default(),
            best_effort: false,
            problem_reporter: None,
        }
    }
}

impl fmt::Debug for ExtractOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractOptions")
            .field("overwrite", &self.overwrite)
            .field("resolver", &self.resolver.as_ref().map(|_| "<dyn>"))
            .field("symlinks", &self.symlinks)
            .field("restore_permissions", &self.restore_permissions)
            .field("limits", &self.limits)
            .field("resources", &self.resources)
            .field("best_effort", &self.best_effort)
            .field(
                "problem_reporter",
                &self.problem_reporter.as_ref().map(|_| "<dyn>"),
            )
            .finish()
    }
}

/// Compression level (CLI numbers 0–9 are mapped through
/// [`CompressionLevel::from_numeric`]; backend-specific mappings are
/// documented in docs/level-mapping.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    /// Store only, no compression
    Store,
    /// Fastest
    Fastest,
    /// Fast
    Fast,
    /// Standard
    #[default]
    Normal,
    /// Maximum
    Maximum,
    /// Ultra
    Ultra,
}

impl CompressionLevel {
    /// Maps the CLI `--level 0..=9` to a level.
    pub fn from_numeric(n: u8) -> Self {
        match n {
            0 => Self::Store,
            1 => Self::Fastest,
            2..=3 => Self::Fast,
            4..=6 => Self::Normal,
            7..=8 => Self::Maximum,
            _ => Self::Ultra,
        }
    }
}

/// Options for creating an archive.
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// Compression level
    pub level: CompressionLevel,
    /// Encryption password
    pub password: Option<Password>,
    /// Encrypt file names (only effective for formats with
    /// `can_encrypt_names`, e.g. 7z)
    pub encrypt_filenames: bool,
    /// Split volume size in bytes (semantics: `.001` byte splitting)
    pub split_size: Option<u64>,
    /// Threads and memory
    pub resources: ResourceOptions,
    /// Exclude patterns (glob)
    pub excludes: Vec<String>,
    /// SQZ-specific creation profile.
    pub sqz: SqzCreateOptions,
}

/// SQZ creation profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqzCreateOptions {
    /// Payload profile label written into the SQZ descriptor.
    ///
    /// Current v1 stores a transparent entry set. Future profiles may wrap a
    /// standard inner archive payload such as `7z` or `zip`.
    pub inner_format: String,
    /// Requested payload recovery redundancy percentage.
    pub recovery_percent: u8,
}

impl Default for SqzCreateOptions {
    fn default() -> Self {
        Self {
            inner_format: "sqz".to_string(),
            // Preserve the original v1 8 data + 2 parity behavior.
            recovery_percent: 25,
        }
    }
}

impl SqzCreateOptions {
    /// Fixed payload data-shard count for SQZ v1.
    pub const DATA_SHARDS: usize = 8;

    /// Maps a requested redundancy percentage onto whole RS parity shards.
    pub fn parity_shards(&self) -> usize {
        Self::parity_shards_for_percent(self.recovery_percent)
    }

    /// Maps a requested redundancy percentage onto whole RS parity shards.
    pub fn parity_shards_for_percent(percent: u8) -> usize {
        let percent = percent.clamp(1, 100) as usize;
        let shards = (Self::DATA_SHARDS * percent).div_ceil(100);
        shards.clamp(1, 255 - Self::DATA_SHARDS)
    }
}

/// Capability declaration of a format.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatCapabilities {
    /// Supports creation
    pub can_create: bool,
    /// Supports extraction
    pub can_extract: bool,
    /// Supports content encryption
    pub can_encrypt_data: bool,
    /// Supports file-name encryption (ZIP=false, 7z=true)
    pub can_encrypt_names: bool,
    /// Supports split volumes
    pub can_split: bool,
    /// Supports append/delete/rename
    pub can_update: bool,
    /// Supports integrity testing
    pub can_test: bool,
}

/// Integrity test report.
#[derive(Debug, Clone, Default)]
pub struct TestReport {
    /// Number of entries tested
    pub entries_tested: u64,
    /// Problem list (empty = passed); log-only text, not user-facing copy
    pub problems: Vec<String>,
    /// Optional archive recovery summary. Formats without recovery data leave
    /// this as `None`.
    pub recovery: Option<RecoverySummary>,
}

impl TestReport {
    /// Whether everything passed.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Summary of recovery data and damage observed while opening/testing an
/// archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySummary {
    /// Recovery scheme identifier, e.g. `sqz-embedded-rs-gf8`.
    pub scheme: String,
    /// Protected block size in bytes.
    pub block_size: u64,
    /// Total protected data blocks.
    pub total_blocks: u64,
    /// Data shards per Reed-Solomon group.
    pub data_shards: u64,
    /// Parity shards per Reed-Solomon group.
    pub parity_shards: u64,
    /// Total parity/recovery blocks present across all groups.
    pub recovery_blocks_available: u64,
    /// Number of damaged protected data blocks detected.
    pub damaged_blocks: u64,
    /// Number of damaged data blocks reconstructed successfully.
    pub repaired_blocks: u64,
    /// Number of damaged data blocks that exceeded recovery capacity.
    pub unrepaired_blocks: u64,
    /// Whether all detected protected-block damage was recoverable.
    pub repair_possible: bool,
}

/// Mutation of an existing archive.
#[derive(Debug, Clone)]
pub enum UpdateOp {
    /// Add a local file/directory
    Add {
        /// Local source path
        src: PathBuf,
        /// Destination path inside the archive
        dest: EntryPath,
    },
    /// Add an empty directory entry
    AddDir {
        /// Directory path inside the archive
        path: EntryPath,
    },
    /// Delete entries by glob
    Delete {
        /// Glob pattern
        pattern: String,
    },
    /// Rename an entry
    Rename {
        /// Old path
        from: EntryPath,
        /// New path
        to: EntryPath,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_numeric_mapping() {
        assert_eq!(CompressionLevel::from_numeric(0), CompressionLevel::Store);
        assert_eq!(CompressionLevel::from_numeric(5), CompressionLevel::Normal);
        assert_eq!(CompressionLevel::from_numeric(9), CompressionLevel::Ultra);
        assert_eq!(CompressionLevel::from_numeric(99), CompressionLevel::Ultra);
    }

    #[test]
    fn password_debug_redacted() {
        let p = Password::new("secret");
        assert_eq!(format!("{p:?}"), "Password(***)");
        assert_eq!(p.expose(), "secret");
    }

    #[test]
    fn memory_limit_bounds_stream_buffer() {
        assert_eq!(
            ResourceOptions::default()
                .stream_buffer_size(64 * 1024)
                .unwrap(),
            64 * 1024
        );

        let capped = ResourceOptions {
            threads: None,
            memory_limit: Some(16 * 1024),
        };
        assert_eq!(capped.stream_buffer_size(64 * 1024).unwrap(), 16 * 1024);

        let too_small = ResourceOptions {
            threads: None,
            memory_limit: Some(1024),
        };
        assert!(matches!(
            too_small.stream_buffer_size(64 * 1024),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn sqz_recovery_percent_maps_to_bounded_parity_shards() {
        assert_eq!(SqzCreateOptions::parity_shards_for_percent(0), 1);
        assert_eq!(SqzCreateOptions::parity_shards_for_percent(1), 1);
        assert_eq!(SqzCreateOptions::parity_shards_for_percent(25), 2);
        assert_eq!(SqzCreateOptions::parity_shards_for_percent(100), 8);

        let opts = SqzCreateOptions {
            recovery_percent: 100,
            ..SqzCreateOptions::default()
        };
        assert_eq!(opts.parity_shards(), 8);
    }
}
