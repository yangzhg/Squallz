//! IPC data-transfer objects. Everything the frontend sees crosses through
//! these serde types; errors are structured `{key, params}` pairs rendered
//! by the frontend i18n store.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use squallz_core::api::{EntryMeta, EntryType, FormatError, ResourceOptions, SafetyLimits};
use squallz_core::{
    CreateCompletionAction, CreateContentPolicy, CreateDestinationGuard, CreateInputEstimate,
    CreatePlan, ExtractInputGuard, ExtractPlan, ExtractSpace, PostSuccessAction,
    SfxRecoveryDetails, SmartLayout,
};
use squallz_i18n::error_message;
use squallz_recovery::RecoveryCleanupDetails;

pub(crate) const PERFORMANCE_STREAM_BUFFER_MIN_BYTES: u64 =
    ResourceOptions::MIN_STREAM_BUFFER_BYTES;
pub(crate) const PERFORMANCE_STREAM_BUFFER_MAX_BYTES: u64 = 64 * 1024;

pub(crate) fn normalize_performance_stream_buffer_limit(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value > 0).map(|value| {
        value.clamp(
            PERFORMANCE_STREAM_BUFFER_MIN_BYTES,
            PERFORMANCE_STREAM_BUFFER_MAX_BYTES,
        )
    })
}

/// Structured engine error: a language-pack key plus placeholder values.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorDto {
    /// `error.*` language-pack key
    pub key: String,
    /// Placeholder name → value
    pub params: HashMap<String, String>,
    /// Log-only detail (English), shown only in the details expander
    pub detail: String,
}

impl From<&FormatError> for ErrorDto {
    fn from(e: &FormatError) -> Self {
        let msg = error_message(e);
        let detail = if e.missing_volume_path().is_some() {
            "required split volume is missing".to_owned()
        } else {
            e.to_string()
        };
        Self {
            key: msg.key.to_owned(),
            params: msg
                .params
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            detail,
        }
    }
}

impl From<FormatError> for ErrorDto {
    fn from(e: FormatError) -> Self {
        Self::from(&e)
    }
}

impl ErrorDto {
    pub fn from_engine(error: &FormatError) -> Self {
        if let Some(details) = squallz_recovery::recovery_cleanup_details(error) {
            return Self::recovery_cleanup(&details, error.to_string());
        }
        match squallz_core::sfx_recovery_details(error) {
            Some(details) => Self::sfx_recovery(&details, error.to_string()),
            None => Self::from(error),
        }
    }

    pub fn recovery_cleanup(details: &RecoveryCleanupDetails, detail: impl Into<String>) -> Self {
        let key = if details.workspace.is_none() {
            "error.recovery_cleanup_record"
        } else if details.output_ready {
            "error.recovery_cleanup_output_ready"
        } else {
            "error.recovery_cleanup_unconfirmed"
        };
        let mut params = HashMap::from([
            (
                "target".to_owned(),
                details.target.to_string_lossy().into_owned(),
            ),
            (
                "journal".to_owned(),
                details.journal.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(workspace) = &details.workspace {
            params.insert(
                "workspace".to_owned(),
                workspace.to_string_lossy().into_owned(),
            );
        }
        Self {
            key: key.to_owned(),
            params,
            detail: detail.into(),
        }
    }

    pub fn sfx_recovery(details: &SfxRecoveryDetails, detail: impl Into<String>) -> Self {
        let journal = details
            .paths
            .iter()
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        matches!(
                            name,
                            ".squallz-sfx-transaction.json"
                                | ".squallz-sfx-completed.json"
                                | ".squallz-sfx-cleanup.json"
                        ) || (name.starts_with(".squallz-sfx-transaction-")
                            && name.ends_with(".json"))
                    })
            })
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let paths = details
            .paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            key: "error.sfx_recovery".to_owned(),
            params: HashMap::from([
                (
                    "target".to_owned(),
                    details.target.to_string_lossy().into_owned(),
                ),
                ("journal".to_owned(), journal),
                ("paths".to_owned(), paths),
                ("count".to_owned(), details.paths.len().to_string()),
            ]),
            detail: detail.into(),
        }
    }

    pub fn other(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            key: "error.other".to_owned(),
            params: HashMap::from([("detail".to_owned(), detail.clone())]),
            detail,
        }
    }

    pub fn destination_inspection(detail: impl Into<String>) -> Self {
        Self {
            key: "error.destination_inspection_failed".to_owned(),
            params: HashMap::new(),
            detail: detail.into(),
        }
    }

    pub fn settings_write(detail: impl Into<String>) -> Self {
        Self {
            key: "error.settings_write".to_owned(),
            params: HashMap::new(),
            detail: detail.into(),
        }
    }

    pub fn secret_store(detail: impl Into<String>) -> Self {
        Self {
            key: "error.secret_store".to_owned(),
            params: HashMap::new(),
            detail: detail.into(),
        }
    }

    pub fn presets(detail: impl Into<String>, conflict: bool) -> Self {
        Self {
            key: if conflict {
                "error.presets_conflict".to_owned()
            } else {
                "error.presets_store".to_owned()
            },
            params: HashMap::new(),
            detail: detail.into(),
        }
    }

    pub fn invalid_preset(detail: impl Into<String>) -> Self {
        Self {
            key: "error.presets_invalid".to_owned(),
            params: HashMap::new(),
            detail: detail.into(),
        }
    }
}

/// Result of `open_archive`.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveInfo {
    /// Handle id for follow-up `list_entries` calls
    pub id: u64,
    /// User-facing source path. Nested archives use a stable virtual path
    /// that never exposes their private workspace location.
    pub path: String,
    /// Filesystem path for regular archives, or an opaque backend reference
    /// for a nested archive. This is passed back to archive commands but is
    /// never shown as a local path.
    pub source: String,
    /// File name shown in the breadcrumb
    pub name: String,
    /// Nested archives are temporary views and cannot be updated in place.
    pub read_only: bool,
    /// Format identifier (`zip` / `7z` / `tar.gz` …)
    pub format: String,
    /// Machine-readable structural state (`complete` or
    /// `zip_local_headers_recovered`).
    pub structure: String,
    /// Total number of entries
    pub entry_count: usize,
    /// Physical volume file names in archive order (`None` for single files)
    pub volumes: Option<Vec<String>>,
    /// Entry names decoded with a non-UTF-8 encoding.
    pub legacy_encoding_count: usize,
    /// Entry names that still contain replacement characters after decoding.
    pub garbled_count: usize,
    /// Most common non-UTF-8 decoding label, if any.
    pub suggested_encoding: Option<String>,
    /// User-selected archive-wide encoding override, if active.
    pub encoding_override: Option<String>,
}

/// One row of the entry list (a real entry or a synthesized directory).
#[derive(Debug, Clone, Serialize)]
pub struct EntryDto {
    /// Full display path inside the archive (`a/b/c.txt`; directories end
    /// with `/` so a selection can be expanded by prefix)
    pub path: String,
    /// Base name shown in the name column
    pub display: String,
    /// `"file"` / `"dir"` / `"symlink"` / `"hardlink"` / `"other"`
    pub entry_type: String,
    /// Uncompressed size (0 for synthesized directories)
    pub size: u64,
    /// Compressed size when the format reports one
    pub compressed: Option<u64>,
    /// Modification time as Unix seconds
    pub modified: Option<u64>,
    /// CRC32 checksum
    pub crc: Option<u32>,
    /// Whether the content is encrypted
    pub encrypted: bool,
    /// Encoding label used to decode the display name
    pub encoding: String,
}

impl EntryDto {
    /// Builds a DTO from an engine entry plus its normalized display path.
    pub fn from_meta(meta: &EntryMeta, normalized: String, base_name: String) -> Self {
        let entry_type = match meta.entry_type {
            EntryType::File => "file",
            EntryType::Dir => "dir",
            EntryType::Symlink { .. } => "symlink",
            EntryType::Hardlink { .. } => "hardlink",
            EntryType::Other => "other",
        };
        Self {
            path: normalized,
            display: base_name,
            entry_type: entry_type.to_owned(),
            size: meta.size,
            compressed: meta.compressed_size,
            modified: meta.modified.and_then(unix_seconds),
            crc: meta.crc32,
            encrypted: meta.encrypted,
            encoding: meta.path.encoding.to_owned(),
        }
    }

    /// Builds a synthesized directory row (`dir_path` ends with `/`).
    pub fn synthesized_dir(dir_path: String, base_name: String) -> Self {
        Self {
            path: dir_path,
            display: base_name,
            entry_type: "dir".to_owned(),
            size: 0,
            compressed: None,
            modified: None,
            crc: None,
            encrypted: false,
            encoding: "utf-8".to_owned(),
        }
    }
}

fn unix_seconds(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// One page of entry rows.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// Total row count at this level (after filtering)
    pub total: usize,
    /// Page index requested
    pub page: usize,
    /// Rows of this page
    pub items: Vec<EntryDto>,
}

/// Format capability info for the compress dialog.
#[derive(Debug, Clone, Serialize)]
pub struct FormatDto {
    pub id: String,
    pub extensions: Vec<String>,
    pub kind: String,
    pub can_create: bool,
    pub can_extract: bool,
    pub can_encrypt_data: bool,
    pub can_encrypt_names: bool,
    pub can_split: bool,
    pub can_update: bool,
    pub can_test: bool,
}

/// Shared core create plan used by desktop preflight.
///
/// The byte fields are conservative free-space budgets, not compressed-size
/// predictions. A completed job returns the exact physical outputs separately.
#[derive(Debug, Clone, Serialize)]
pub struct CreatePlanDto {
    pub input_count: usize,
    pub entries: usize,
    pub deduplicated_entries: usize,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub total_bytes: u64,
    pub output_budget_bytes: u64,
    pub primary_output: String,
    pub archive_output_budget_bytes: u64,
    pub final_output_budget_bytes: u64,
    pub split_volume_count_budget: Option<u64>,
    pub workspace_budget_bytes: u64,
    pub system_temp_budget_bytes: u64,
}

/// Legacy input-only create estimate retained for command compatibility.
#[derive(Debug, Clone, Serialize)]
pub struct CreateEstimateDto {
    pub input_count: usize,
    pub entries: usize,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub total_bytes: u64,
    pub output_budget_bytes: u64,
}

impl From<CreateInputEstimate> for CreateEstimateDto {
    fn from(value: CreateInputEstimate) -> Self {
        Self {
            input_count: value.input_count,
            entries: value.entries,
            files: value.files,
            directories: value.directories,
            symlinks: value.symlinks,
            total_bytes: value.total_bytes,
            output_budget_bytes: value.output_budget_bytes(),
        }
    }
}

impl From<CreatePlan> for CreatePlanDto {
    fn from(value: CreatePlan) -> Self {
        Self {
            input_count: value.inputs.input_count,
            entries: value.inputs.entries,
            deduplicated_entries: 0,
            files: value.inputs.files,
            directories: value.inputs.directories,
            symlinks: value.inputs.symlinks,
            total_bytes: value.inputs.total_bytes,
            output_budget_bytes: value.inputs.output_budget_bytes(),
            primary_output: value.primary_output.to_string_lossy().into_owned(),
            archive_output_budget_bytes: value.archive_output_budget_bytes,
            final_output_budget_bytes: value.final_output_budget_bytes,
            split_volume_count_budget: value.split_volume_count_budget,
            workspace_budget_bytes: value.workspace_budget_bytes,
            system_temp_budget_bytes: value.system_temp_budget_bytes,
        }
    }
}

impl CreatePlanDto {
    pub(crate) fn with_scanned_entries(mut self, scanned_entries: usize) -> Self {
        self.deduplicated_entries = scanned_entries.saturating_sub(self.entries);
        self
    }
}

/// Shared core extraction plan used by desktop preflight and job results.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractPlanDto {
    pub requested_destination: String,
    pub destination: String,
    pub layout: String,
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub hardlinks: u64,
    pub other: u64,
    pub total_bytes: u64,
    pub estimated_conflicts: u64,
}

impl From<ExtractPlan> for ExtractPlanDto {
    fn from(value: ExtractPlan) -> Self {
        Self {
            requested_destination: value.requested_destination.to_string_lossy().into_owned(),
            destination: value.destination.to_string_lossy().into_owned(),
            layout: match value.layout {
                SmartLayout::DirectExtract => "direct",
                SmartLayout::WrapInFolder => "wrap_in_folder",
            }
            .to_owned(),
            entries: value.scope.entries,
            files: value.scope.files,
            directories: value.scope.directories,
            symlinks: value.scope.symlinks,
            hardlinks: value.scope.hardlinks,
            other: value.scope.other,
            total_bytes: value.scope.total_bytes,
            estimated_conflicts: value.estimated_conflicts,
        }
    }
}

/// Extraction plan plus the destination-volume capacity observed during the
/// same read-only preflight.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractPlanPreflightDto {
    #[serde(flatten)]
    pub plan: ExtractPlanDto,
    pub input_guard: ExtractInputGuard,
    pub required_free_bytes: u64,
    pub available_bytes: u64,
    pub space_ok: bool,
}

impl ExtractPlanPreflightDto {
    pub fn new(plan: ExtractPlan, space: ExtractSpace, input_guard: ExtractInputGuard) -> Self {
        Self {
            plan: ExtractPlanDto::from(plan),
            input_guard,
            required_free_bytes: space.required_bytes,
            available_bytes: space.available_bytes,
            space_ok: space.is_sufficient(),
        }
    }
}

/// Destination-volume disk preflight for create/update jobs.
#[derive(Debug, Clone, Serialize)]
pub struct DiskSpaceDto {
    pub path: String,
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub ok: bool,
}

/// Host SFX target bundled with this desktop installation.
#[derive(Debug, Clone, Serialize)]
pub struct SfxCreateCapabilityDto {
    pub target: String,
    pub extension: String,
    pub available: bool,
    pub status: String,
    pub requires_signing: bool,
}

/// Developer ID identities available for publishing a macOS SFX.
#[derive(Debug, Clone, Serialize)]
pub struct MacosSfxPublisherStatusDto {
    pub available: bool,
    pub status: String,
    pub identities: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDestinationInspectionDto {
    pub conflict: bool,
    pub guard: Option<CreateDestinationGuard>,
}

/// One-level preview of an archive stored as an entry inside another archive.
#[derive(Debug, Clone, Serialize)]
pub struct NestedArchivePreviewDto {
    pub outer_path: String,
    pub entry_path: String,
    pub format: String,
    pub entry_count: usize,
    pub truncated: bool,
    pub items: Vec<EntryDto>,
}

/// Result of extracting one archive entry into a temporary preview file.
#[derive(Debug, Clone, Serialize)]
pub struct EntryPreviewDto {
    pub outer_path: String,
    pub entry_path: String,
    pub display_name: String,
    pub preview_id: String,
    pub size: u64,
    pub archive_like: bool,
}

/// Job submission parameters (`submit_job`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSpec {
    /// Create an archive from local inputs.
    Compress {
        inputs: Vec<String>,
        dest: String,
        level: u8,
        password: Option<String>,
        encrypt_names: bool,
        split_size: Option<u64>,
        #[serde(default)]
        split_mode: Option<String>,
        excludes: Vec<String>,
        #[serde(default)]
        content_policy: Option<CreateContentPolicy>,
        #[serde(default)]
        sqz_inner_format: Option<String>,
        #[serde(default)]
        sfx_target: Option<String>,
        #[serde(default)]
        completion: Option<CreateCompletionAction>,
        #[serde(default)]
        post_success: Option<PostSuccessAction>,
        /// Reopen the committed output and read every entry before reporting
        /// success. Source cleanup also requires this check.
        #[serde(default)]
        test_after_create: Option<bool>,
        /// `Some(true)` is supplied only after a native Save panel confirms
        /// replacement. Omitted legacy payloads retain their prior behavior.
        #[serde(default)]
        replace_existing: Option<bool>,
        /// Opaque core authorization captured immediately before the native
        /// replacement confirmation. It must never enter task snapshots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement_guard: Option<CreateDestinationGuard>,
    },
    /// Publish a separate Developer ID-signed and notarized macOS SFX app.
    PublishMacosSfx {
        source: String,
        output: String,
        identity: String,
        notary_profile: String,
    },
    /// Extract an archive (optionally a selection of display paths;
    /// directory selections end with `/` and expand by prefix).
    Extract {
        path: String,
        dest: String,
        /// Final destination returned by the most recent extraction plan.
        /// Omitted callers retain the legacy replan-and-run behavior.
        #[serde(default)]
        expected_destination: Option<String>,
        /// Opaque binding to the source and selected scope returned by the
        /// most recent extraction preflight. It must not enter task snapshots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_input_guard: Option<ExtractInputGuard>,
        selection: Option<Vec<String>>,
        overwrite: String,
        symlinks: String,
        smart: bool,
        encoding: Option<String>,
        password: Option<String>,
        #[serde(default)]
        verify_sfx: bool,
        #[serde(default)]
        best_effort: bool,
    },
    /// Extract multiple archives as one foreground GUI job. Archives run in
    /// sequence so the UI has one modal, one cancel control, and one result.
    BatchExtract {
        items: Vec<BatchExtractItem>,
        overwrite: String,
        symlinks: String,
        smart: bool,
    },
    /// Extract the contents of an archive entry that is itself an archive.
    ExtractNested {
        outer_path: String,
        entry_path: String,
        dest: String,
        overwrite: String,
        symlinks: String,
        smart: bool,
        encoding: Option<String>,
        password: Option<String>,
        #[serde(default)]
        best_effort: bool,
    },
    /// Integrity test.
    Test {
        path: String,
        encoding: Option<String>,
        password: Option<String>,
    },
    /// Format conversion.
    Convert {
        src: String,
        dest: String,
        level: u8,
        src_encoding: Option<String>,
        src_password: Option<String>,
        dest_password: Option<String>,
        encrypt_names: bool,
        #[serde(default)]
        split_size: Option<u64>,
        #[serde(default)]
        split_mode: Option<String>,
        /// New callers always state whether replacement was confirmed.
        /// Omitted legacy payloads retain their prior replacement behavior.
        #[serde(default)]
        replace_existing: Option<bool>,
        /// Opaque authorization for the destination state the user confirmed.
        /// It must never enter task snapshots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement_guard: Option<CreateDestinationGuard>,
    },
    /// Export a SQZ container to a standard archive.
    ExportSqz {
        src: String,
        dest: String,
        level: u8,
        dest_password: Option<String>,
        /// New callers always state whether replacement was confirmed.
        /// Omitted legacy payloads retain their prior replacement behavior.
        #[serde(default)]
        replace_existing: Option<bool>,
        /// Opaque authorization for the destination state the user confirmed.
        /// It must never enter task snapshots.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replacement_guard: Option<CreateDestinationGuard>,
    },
    /// Rewrite a damaged SQZ container into a new repaired SQZ.
    RepairSqz {
        src: String,
        dest: String,
        level: u8,
    },
    /// Rebuild a ZIP-family archive whose central directory is missing while
    /// local headers and payloads are still intact.
    RepairZip {
        src: String,
        dest: String,
        level: u8,
    },
    /// Create external PAR2 recovery data for the current archive.
    Protect {
        path: String,
        redundancy: u8,
        recovery: Option<String>,
    },
    /// Verify external PAR2 recovery data.
    VerifyRecovery {
        path: String,
        recovery: Option<String>,
    },
    /// Repair an archive using external PAR2 recovery data.
    RepairRecovery {
        path: String,
        output: Option<String>,
        #[serde(default)]
        output_directory: bool,
        recovery: Option<String>,
    },
    /// Update an existing archive with append/delete/rename operations.
    Update {
        path: String,
        add: Vec<String>,
        delete: Vec<String>,
        rename: Vec<RenameSpec>,
        #[serde(default)]
        mkdir: Vec<String>,
        #[serde(default)]
        excludes: Vec<String>,
        #[serde(default)]
        content_policy: Option<CreateContentPolicy>,
        password: Option<String>,
        level: u8,
    },
    /// Compute local-file checksums without modifying inputs.
    Checksum {
        inputs: Vec<String>,
        #[serde(default)]
        excludes: Vec<String>,
        #[serde(default = "default_checksum_algorithm")]
        algorithm: String,
    },
    /// Verify a checksum manifest without modifying inputs.
    ChecksumCheck {
        manifest: String,
        #[serde(default = "default_checksum_algorithm")]
        algorithm: String,
    },
    /// Scan local files for duplicate content without modifying anything.
    DuplicateScan {
        inputs: Vec<String>,
        #[serde(default)]
        excludes: Vec<String>,
        #[serde(default = "default_duplicate_min_size")]
        min_size: u64,
    },
}

impl JobSpec {
    /// Returns the task description that may be mirrored to another app
    /// window. Credentials never cross that boundary; non-secret options
    /// continue to describe the operation that actually ran.
    pub(crate) fn redacted_for_snapshot(&self) -> Self {
        let mut redacted = self.clone();
        match &mut redacted {
            Self::Compress {
                password,
                replacement_guard,
                ..
            } => {
                *password = None;
                *replacement_guard = None;
            }
            Self::Extract {
                password,
                expected_input_guard,
                ..
            } => {
                *password = None;
                *expected_input_guard = None;
            }
            Self::ExtractNested { password, .. }
            | Self::Test { password, .. }
            | Self::Update { password, .. } => {
                *password = None;
            }
            Self::BatchExtract { items, .. } => {
                for item in items {
                    item.password = None;
                }
            }
            Self::Convert {
                src_password,
                dest_password,
                replacement_guard,
                ..
            } => {
                *src_password = None;
                *dest_password = None;
                *replacement_guard = None;
            }
            Self::ExportSqz {
                dest_password,
                replacement_guard,
                ..
            } => {
                *dest_password = None;
                *replacement_guard = None;
            }
            Self::PublishMacosSfx {
                identity,
                notary_profile,
                ..
            } => {
                identity.clear();
                notary_profile.clear();
            }
            Self::RepairSqz { .. }
            | Self::RepairZip { .. }
            | Self::Protect { .. }
            | Self::VerifyRecovery { .. }
            | Self::RepairRecovery { .. }
            | Self::Checksum { .. }
            | Self::ChecksumCheck { .. }
            | Self::DuplicateScan { .. } => {}
        }
        redacted
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExtractItem {
    pub path: String,
    pub dest: String,
    pub encoding: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub best_effort: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameSpec {
    pub from: String,
    pub to: String,
}

fn default_duplicate_min_size() -> u64 {
    1
}

fn default_checksum_algorithm() -> String {
    "sha256".into()
}

/// Progress event payload (`job://progress`, throttled to ≥60 ms).
#[derive(Debug, Clone, Serialize)]
pub struct ProgressEvent {
    pub id: u64,
    /// Global snapshot revision for stale-event rejection.
    pub version: u64,
    pub done: u64,
    /// 0 = unknown total (indeterminate progress bar)
    pub total: u64,
    pub current: String,
    /// Bytes processed within the current entry; 0 when unknown.
    pub current_done: u64,
    /// Total bytes for the current entry; 0 when unknown.
    pub current_total: u64,
    /// Entries prepared during an input scan. Omitted for byte progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_entries: Option<u64>,
    /// Smoothed throughput in bytes/second
    pub speed: u64,
}

/// State event payload (`job://state`).
#[derive(Debug, Clone, Serialize)]
pub struct StateEvent {
    pub id: u64,
    /// Global snapshot revision for stale-event rejection.
    pub version: u64,
    /// `queued|running|paused|done|failed|cancelled`
    pub state: String,
    /// Structured error for `failed`
    pub error: Option<ErrorDto>,
}

/// Conflict prompt payload (`job://ask-conflict`).
#[derive(Debug, Clone, Serialize)]
pub struct AskConflictEvent {
    pub id: u64,
    /// Existing file (absolute path)
    pub existing_path: String,
    pub existing_size: u64,
    pub existing_modified: Option<u64>,
    /// Incoming archive entry
    pub incoming_path: String,
    pub incoming_size: u64,
    pub incoming_modified: Option<u64>,
}

/// Password prompt payload (`job://ask-password`).
#[derive(Debug, Clone, Serialize)]
pub struct AskPasswordEvent {
    pub id: u64,
    /// Archive file name (dialog hint)
    pub name: String,
    /// Whether the previous attempt was wrong (true) or none was set
    pub wrong: bool,
}

/// Current archive password-book state.
#[derive(Debug, Clone, Serialize)]
pub struct PasswordBookStatusDto {
    /// Whether a persistent secret store is available on this platform/session.
    pub available: bool,
    /// Whether this archive has a password saved in the persistent store.
    pub saved: bool,
}

/// One installed desktop/file-manager integration action.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationActionDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub path: String,
    pub script_path: String,
}

/// Health of the installed files for one desktop integration action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationActionHealthStateDto {
    Healthy,
    Missing,
    Damaged,
}

/// One expected desktop integration action and the evidence found on disk.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationActionHealthDto {
    pub id: String,
    pub name: String,
    pub state: IntegrationActionHealthStateDto,
    pub issue: Option<String>,
}

/// Overall state of the platform integration files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationHealthStateDto {
    Healthy,
    NeedsRepair,
    Missing,
    Unavailable,
}

/// Result of applying the visible integration settings.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationApplyResultDto {
    pub platform: String,
    pub services_dir: String,
    pub script_dir: String,
    pub installed: Vec<IntegrationActionDto>,
    pub unsupported: Vec<String>,
}

/// Current desktop/file-manager integration status.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationStatusDto {
    pub platform: String,
    pub services_dir: String,
    pub script_dir: String,
    pub health: IntegrationHealthStateDto,
    pub actions: Vec<IntegrationActionHealthDto>,
    pub can_repair: bool,
    pub can_remove: bool,
    pub installed: Vec<IntegrationActionDto>,
    pub missing: Vec<String>,
    pub unsupported: Vec<String>,
}

/// Current default application for one extension declared by the app bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationDefaultHandlerStateDto {
    Squallz,
    Other,
    Unknown,
}

/// Read-only LaunchServices evidence for one declared extension.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationDefaultHandlerDto {
    pub extension: String,
    pub state: IntegrationDefaultHandlerStateDto,
    pub application_name: Option<String>,
}

/// Aggregate state across every extension declared by the app bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationDefaultHandlersStateDto {
    Squallz,
    Mixed,
    Other,
    Unknown,
    Unavailable,
}

/// Default-handler summary kept separate from managed action-file health.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationDefaultHandlersDto {
    pub state: IntegrationDefaultHandlersStateDto,
    pub total: usize,
    pub checked: usize,
    pub squallz: usize,
    pub handlers: Vec<IntegrationDefaultHandlerDto>,
}

/// The platform can require a manual visibility check even when managed files are healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationFileManagerVisibilityStateDto {
    ManualCheck,
    Unsupported,
}

/// Read-only file-manager visibility boundary and a stable reason code for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationFileManagerVisibilityDto {
    pub state: IntegrationFileManagerVisibilityStateDto,
    pub reason: String,
}

/// Where an optional format backend was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackendSourceDto {
    Application,
    Environment,
    Path,
}

/// Read-only runtime backend health. Paths are deliberately not exposed to the WebView.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeBackendStatusDto {
    pub id: String,
    pub available: bool,
    pub configured: bool,
    pub source: Option<RuntimeBackendSourceDto>,
    pub tool: Option<String>,
}

/// Runtime and system-owned integration evidence. This never changes user preferences.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationSystemDiagnosticsDto {
    pub platform: String,
    pub backends: Vec<RuntimeBackendStatusDto>,
    pub default_handlers: IntegrationDefaultHandlersDto,
    pub file_manager_visibility: IntegrationFileManagerVisibilityDto,
}

/// Result of removing platform integration actions.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationRemoveResultDto {
    pub platform: String,
    pub services_dir: String,
    pub script_dir: String,
    pub removed: Vec<IntegrationActionDto>,
    pub missing: Vec<String>,
    pub unsupported: Vec<String>,
}

/// Available language (settings dropdown).
#[derive(Debug, Clone, Serialize)]
pub struct LanguageDto {
    pub tag: String,
    /// Self-described name from the pack's `meta.name`
    pub name: String,
}

/// Persisted GUI settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SettingsDto {
    /// `"system" | "light" | "dark"`
    pub theme: Option<String>,
    /// BCP 47 tag; `None` = follow the system
    pub language: Option<String>,
    /// `"modern" | "classic"`; `None` = first-run selection not completed
    pub ui_mode: Option<String>,
    /// `"compact" | "standard" | "comfort"`; affects only desktop chrome density.
    pub ui_density: Option<String>,
    /// Appearance accent palette id (`aqua` / `sage` / `nordic` / ...).
    pub accent_palette: Option<String>,
    /// Optional validated custom accent color (`#RRGGBB`).
    pub custom_accent: Option<String>,
    /// Whether custom accent colors are clamped into readable light/dark variants.
    pub accent_contrast_guard: Option<bool>,
    /// Optional default directory used as the parent for GUI extract destinations.
    pub default_extract_dir: Option<String>,
    /// Optional default directory used as the parent for GUI create destinations.
    pub default_create_dir: Option<String>,
    /// Reveal the destination folder in Finder after a successful extract job.
    pub reveal_after_extract: bool,
    /// `None` preserves the default of checking the stable channel automatically.
    pub check_updates_automatically: Option<bool>,
    /// Upper bound on total extracted bytes.
    pub safety_max_output_bytes: Option<u64>,
    /// Upper bound on archive entries.
    pub safety_max_entries: Option<u64>,
    /// Per-entry uncompressed/compressed ratio limit.
    pub safety_max_compression_ratio: Option<u32>,
    /// Compression worker threads (`None` = automatic).
    pub performance_threads: Option<usize>,
    /// Squallz-owned stream buffer cap in bytes (`None` = automatic).
    pub performance_memory_limit_bytes: Option<u64>,
    /// Maximum simultaneous archive jobs (`None` = CPU-aware automatic).
    pub performance_parallel_jobs: Option<usize>,
}

impl SettingsDto {
    pub fn automatic_update_checks_enabled(&self) -> bool {
        self.check_updates_automatically != Some(false)
    }

    pub fn safety_limits(&self) -> SafetyLimits {
        let default = SafetyLimits::default();
        SafetyLimits {
            max_output_bytes: safety_u64_or_default(
                self.safety_max_output_bytes,
                default.max_output_bytes,
            ),
            max_entries: safety_u64_or_default(self.safety_max_entries, default.max_entries),
            max_compression_ratio: safety_u32_or_default(
                self.safety_max_compression_ratio,
                default.max_compression_ratio,
            ),
        }
    }

    pub fn resource_options(&self) -> ResourceOptions {
        ResourceOptions {
            threads: self.performance_threads.map(|v| v.clamp(1, 64)),
            memory_limit: normalize_performance_stream_buffer_limit(
                self.performance_memory_limit_bytes,
            ),
        }
    }
}

fn safety_u64_or_default(value: Option<u64>, default: u64) -> u64 {
    value.map_or(default, |value| value).max(1)
}

fn safety_u32_or_default(value: Option<u32>, default: u32) -> u32 {
    value.map_or(default, |value| value).max(1)
}

/// Locale table response (`get_locale_table`).
#[derive(Debug, Clone, Serialize)]
pub struct LocaleTable {
    /// Resolved language tag
    pub lang: String,
    /// Full key→value table (en-US fallback merged in)
    pub table: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, UNIX_EPOCH};

    use squallz_core::api::{EntryMeta, EntryPath, EntryType, FormatError};
    use squallz_core::{
        CreateInputEstimate, CreatePlan, ExtractInputGuard, ExtractPlan, ExtractScope,
        SfxRecoveryDetails, SmartLayout,
    };
    use squallz_recovery::RecoveryCleanupDetails;

    use super::{
        CreatePlanDto, EntryDto, ErrorDto, ExtractPlanDto, ExtractPlanPreflightDto, JobSpec,
        SettingsDto, PERFORMANCE_STREAM_BUFFER_MAX_BYTES, PERFORMANCE_STREAM_BUFFER_MIN_BYTES,
    };

    #[test]
    fn existing_output_error_has_a_safe_actionable_key() {
        let error = FormatError::output_exists("/private/archive.zip");

        let dto = ErrorDto::from_engine(&error);

        assert_eq!(dto.key, "error.output_exists");
        assert!(dto.params.is_empty());
        assert!(dto.detail.contains("/private/archive.zip"));
    }

    #[test]
    fn missing_volume_error_hides_its_parent_directory_from_visible_detail() {
        let error = FormatError::missing_volume("/private/downloads/archive.7z.004");

        let dto = ErrorDto::from_engine(&error);

        assert_eq!(dto.key, "gui.error.corrupt.volume_missing");
        assert_eq!(
            dto.params.get("name").map(String::as_str),
            Some("archive.7z.004")
        );
        assert_eq!(dto.detail, "required split volume is missing");
        assert!(!dto.detail.contains("/private"));
    }

    #[test]
    fn destination_inspection_failure_keeps_internal_detail_out_of_params() {
        let dto = ErrorDto::destination_inspection("worker join failed: internal detail");

        assert_eq!(dto.key, "error.destination_inspection_failed");
        assert!(dto.params.is_empty());
        assert_eq!(dto.detail, "worker join failed: internal detail");
    }

    #[test]
    fn sfx_recovery_error_keeps_target_journal_and_every_inspection_path() {
        let target = PathBuf::from("/tmp/Installer.app");
        let journal = PathBuf::from("/tmp/.squallz-sfx-transaction.json");
        let holder = PathBuf::from("/tmp/.squallz-sfx-deadbeef-7-1");
        let previous = holder.join("previous");
        let dto = ErrorDto::sfx_recovery(
            &SfxRecoveryDetails {
                target: target.clone(),
                paths: vec![journal.clone(), holder.clone(), previous.clone()],
            },
            "injected transaction conflict",
        );

        assert_eq!(dto.key, "error.sfx_recovery");
        assert_eq!(dto.params["target"], target.to_string_lossy());
        assert_eq!(dto.params["journal"], journal.to_string_lossy());
        assert_eq!(dto.params["count"], "3");
        assert_eq!(
            dto.params["paths"],
            [journal, holder, previous]
                .iter()
                .map(|path| path.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(dto.detail, "injected transaction conflict");
    }

    #[test]
    fn sfx_recovery_error_recognizes_each_fixed_transaction_record() {
        for name in [
            ".squallz-sfx-transaction.json",
            ".squallz-sfx-completed.json",
            ".squallz-sfx-cleanup.json",
        ] {
            let record = PathBuf::from("/tmp").join(name);
            let dto = ErrorDto::sfx_recovery(
                &SfxRecoveryDetails {
                    target: PathBuf::from("/tmp/Installer.app"),
                    paths: vec![record.clone()],
                },
                "injected recovery debt",
            );
            assert_eq!(dto.params["journal"], record.to_string_lossy());
        }
    }

    #[test]
    fn par2_cleanup_error_distinguishes_ready_and_unconfirmed_outputs() {
        let target = PathBuf::from("/tmp/archive.repaired.zip");
        let workspace = PathBuf::from("/tmp/.archive.repaired.zip.sqz-par2-repair-7-11.work");
        let journal = PathBuf::from("/tmp/.squallz-par2-repair-a1.json");

        for (output_ready, key) in [
            (true, "error.recovery_cleanup_output_ready"),
            (false, "error.recovery_cleanup_unconfirmed"),
        ] {
            let dto = ErrorDto::recovery_cleanup(
                &RecoveryCleanupDetails {
                    target: target.clone(),
                    workspace: Some(workspace.clone()),
                    journal: journal.clone(),
                    output_ready,
                },
                "injected cleanup failure",
            );

            assert_eq!(dto.key, key);
            assert_eq!(dto.params["target"], target.to_string_lossy());
            assert_eq!(dto.params["workspace"], workspace.to_string_lossy());
            assert_eq!(dto.params["journal"], journal.to_string_lossy());
            assert_eq!(dto.detail, "injected cleanup failure");
        }
    }

    #[test]
    fn par2_damaged_record_does_not_invent_a_workspace_path() {
        let target = PathBuf::from("/tmp/archive.repaired.zip");
        let journal = PathBuf::from("/tmp/.squallz-par2-repair-a1.json");
        let dto = ErrorDto::recovery_cleanup(
            &RecoveryCleanupDetails {
                target: target.clone(),
                workspace: None,
                journal: journal.clone(),
                output_ready: false,
            },
            "injected damaged record",
        );

        assert_eq!(dto.key, "error.recovery_cleanup_record");
        assert_eq!(dto.params["target"], target.to_string_lossy());
        assert_eq!(dto.params["journal"], journal.to_string_lossy());
        assert!(!dto.params.contains_key("workspace"));
    }

    #[test]
    fn create_plan_dto_keeps_input_and_budget_fields_distinct() {
        let inputs = CreateInputEstimate {
            input_count: 2,
            entries: 7,
            files: 4,
            directories: 2,
            symlinks: 1,
            total_bytes: 4096,
        };
        let expected_output_budget = inputs.output_budget_bytes();
        let dto = CreatePlanDto::from(CreatePlan {
            inputs,
            primary_output: PathBuf::from("archive.zip.001"),
            archive_output_budget_bytes: 6144,
            final_output_budget_bytes: 8192,
            split_volume_count_budget: Some(3),
            workspace_budget_bytes: 12_288,
            system_temp_budget_bytes: 4096,
        })
        .with_scanned_entries(10);

        assert_eq!(dto.input_count, 2);
        assert_eq!(dto.entries, 7);
        assert_eq!(dto.deduplicated_entries, 3);
        assert_eq!(dto.files, 4);
        assert_eq!(dto.directories, 2);
        assert_eq!(dto.symlinks, 1);
        assert_eq!(dto.total_bytes, 4096);
        assert_eq!(dto.output_budget_bytes, expected_output_budget);
        assert_eq!(dto.primary_output, "archive.zip.001");
        assert_eq!(dto.archive_output_budget_bytes, 6144);
        assert_eq!(dto.final_output_budget_bytes, 8192);
        assert_eq!(dto.split_volume_count_budget, Some(3));
        assert_eq!(dto.workspace_budget_bytes, 12_288);
        assert_eq!(dto.system_temp_budget_bytes, 4096);
    }

    #[test]
    fn extract_plan_dto_keeps_layout_scope_and_conflicts() {
        let dto = ExtractPlanDto::from(ExtractPlan {
            requested_destination: PathBuf::from("output"),
            destination: PathBuf::from("output/archive"),
            layout: SmartLayout::WrapInFolder,
            scope: ExtractScope {
                entries: 7,
                files: 3,
                directories: 1,
                symlinks: 1,
                hardlinks: 1,
                other: 1,
                total_bytes: 4096,
            },
            estimated_conflicts: 2,
        });

        assert_eq!(dto.requested_destination, "output");
        assert_eq!(dto.destination, "output/archive");
        assert_eq!(dto.layout, "wrap_in_folder");
        assert_eq!(dto.entries, 7);
        assert_eq!(dto.files, 3);
        assert_eq!(dto.directories, 1);
        assert_eq!(dto.symlinks, 1);
        assert_eq!(dto.hardlinks, 1);
        assert_eq!(dto.other, 1);
        assert_eq!(dto.total_bytes, 4096);
        assert_eq!(dto.estimated_conflicts, 2);
    }

    #[test]
    fn extract_preflight_flattens_plan_and_capacity_for_ipc() {
        let input_guard =
            serde_json::from_str::<ExtractInputGuard>(&format!("\"sqeg1_{}\"", "00".repeat(32)))
                .unwrap();
        let dto = ExtractPlanPreflightDto::new(
            ExtractPlan {
                requested_destination: PathBuf::from("output"),
                destination: PathBuf::from("output/archive"),
                layout: SmartLayout::WrapInFolder,
                scope: ExtractScope {
                    entries: 2,
                    files: 1,
                    total_bytes: 4096,
                    ..ExtractScope::default()
                },
                estimated_conflicts: 0,
            },
            squallz_core::ExtractSpace {
                required_bytes: 12_288,
                available_bytes: 8192,
            },
            input_guard,
        );

        let value = serde_json::to_value(dto).unwrap();
        assert_eq!(value["destination"], "output/archive");
        assert_eq!(value["total_bytes"], 4096);
        assert_eq!(value["required_free_bytes"], 12_288);
        assert_eq!(value["available_bytes"], 8192);
        assert_eq!(value["space_ok"], false);
        assert_eq!(value["input_guard"], format!("sqeg1_{}", "00".repeat(32)));
    }

    #[test]
    fn settings_safety_limits_default_and_clamp() {
        assert!(!SettingsDto::default().reveal_after_extract);
        assert!(SettingsDto::default().automatic_update_checks_enabled());
        assert!(!SettingsDto {
            check_updates_automatically: Some(false),
            ..SettingsDto::default()
        }
        .automatic_update_checks_enabled());

        let limits = SettingsDto::default().safety_limits();
        assert_eq!(limits.max_output_bytes, 256 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_entries, 1_000_000);
        assert_eq!(limits.max_compression_ratio, 2048);

        let custom = SettingsDto {
            safety_max_output_bytes: Some(0),
            safety_max_entries: Some(50),
            safety_max_compression_ratio: Some(0),
            ..SettingsDto::default()
        }
        .safety_limits();
        assert_eq!(custom.max_output_bytes, 1);
        assert_eq!(custom.max_entries, 50);
        assert_eq!(custom.max_compression_ratio, 1);
    }

    #[test]
    fn settings_resource_options_default_and_clamp() {
        assert_eq!(SettingsDto::default().resource_options().threads, None);
        assert_eq!(SettingsDto::default().resource_options().memory_limit, None);

        let custom = SettingsDto {
            performance_threads: Some(999),
            performance_memory_limit_bytes: Some(512 * 1024 * 1024),
            ..SettingsDto::default()
        }
        .resource_options();
        assert_eq!(custom.threads, Some(64));
        assert_eq!(
            custom.memory_limit,
            Some(PERFORMANCE_STREAM_BUFFER_MAX_BYTES)
        );

        let below_minimum = SettingsDto {
            performance_memory_limit_bytes: Some(1),
            ..SettingsDto::default()
        }
        .resource_options();
        assert_eq!(
            below_minimum.memory_limit,
            Some(PERFORMANCE_STREAM_BUFFER_MIN_BYTES)
        );
    }

    #[test]
    fn entry_dto_maps_types_encoding_and_pre_epoch_time() {
        let mut meta = EntryMeta {
            path: EntryPath::from_raw(vec![0xc4, 0xe3], "你.txt".to_owned(), "GBK"),
            entry_type: EntryType::Symlink {
                target: b"target.txt".to_vec(),
            },
            size: 42,
            compressed_size: Some(21),
            modified: Some(UNIX_EPOCH + Duration::from_secs(7)),
            unix_mode: Some(0o644),
            crc32: Some(0x1234),
            encrypted: true,
        };
        let dto = EntryDto::from_meta(&meta, "links/you.txt".to_owned(), "you.txt".to_owned());

        assert_eq!(dto.path, "links/you.txt");
        assert_eq!(dto.display, "you.txt");
        assert_eq!(dto.entry_type, "symlink");
        assert_eq!(dto.size, 42);
        assert_eq!(dto.compressed, Some(21));
        assert_eq!(dto.modified, Some(7));
        assert_eq!(dto.crc, Some(0x1234));
        assert!(dto.encrypted);
        assert_eq!(dto.encoding, "GBK");

        meta.entry_type = EntryType::Hardlink {
            target: b"target.txt".to_vec(),
        };
        assert_eq!(
            EntryDto::from_meta(&meta, "hard".to_owned(), "hard".to_owned()).entry_type,
            "hardlink"
        );

        meta.entry_type = EntryType::Other;
        meta.modified = Some(UNIX_EPOCH - Duration::from_secs(1));
        assert_eq!(
            EntryDto::from_meta(&meta, "other".to_owned(), "other".to_owned()).modified,
            None
        );

        let synthesized = EntryDto::synthesized_dir("dir/".to_owned(), "dir".to_owned());
        assert_eq!(synthesized.entry_type, "dir");
        assert_eq!(synthesized.size, 0);
        assert_eq!(synthesized.encoding, "utf-8");
    }

    #[test]
    fn snapshot_job_specs_remove_credentials_and_opaque_guards() {
        let secret = "snapshot-secret-must-not-appear";
        let replacement_guard = format!("sqcg1_01{}", "00".repeat(64));
        let input_guard = format!("sqeg1_{}", "00".repeat(32));
        let specs = vec![
            serde_json::json!({
                "kind": "compress", "inputs": ["source"], "dest": "archive.7z",
                "level": 5, "password": secret, "encrypt_names": true,
                "split_size": null, "excludes": [],
                "replace_existing": true, "replacement_guard": replacement_guard.clone()
            }),
            serde_json::json!({
                "kind": "extract", "path": "archive.7z", "dest": "out",
                "selection": null, "overwrite": "ask", "symlinks": "preserve",
                "smart": false, "encoding": null, "password": secret,
                "expected_input_guard": input_guard,
                "best_effort": false
            }),
            serde_json::json!({
                "kind": "batch_extract", "items": [{
                    "path": "archive.7z", "dest": "out", "encoding": null,
                    "password": secret, "best_effort": false
                }], "overwrite": "ask", "symlinks": "preserve", "smart": false
            }),
            serde_json::json!({
                "kind": "extract_nested", "outer_path": "outer.zip",
                "entry_path": "inner.7z", "dest": "out", "overwrite": "ask",
                "symlinks": "preserve", "smart": false, "encoding": null,
                "password": secret, "best_effort": false
            }),
            serde_json::json!({
                "kind": "test", "path": "archive.7z", "encoding": null,
                "password": secret
            }),
            serde_json::json!({
                "kind": "convert", "src": "source.7z", "dest": "dest.7z",
                "level": 5, "src_encoding": null, "src_password": secret,
                "dest_password": secret, "encrypt_names": true,
                "replace_existing": true, "replacement_guard": replacement_guard.clone()
            }),
            serde_json::json!({
                "kind": "export_sqz", "src": "source.sqz", "dest": "dest.7z",
                "level": 5, "dest_password": secret,
                "replace_existing": true, "replacement_guard": replacement_guard
            }),
            serde_json::json!({
                "kind": "update", "path": "archive.7z", "add": [], "delete": [],
                "rename": [], "mkdir": [], "excludes": [], "password": secret,
                "level": 5
            }),
            serde_json::json!({
                "kind": "publish_macos_sfx", "source": "Unsigned.app",
                "output": "Published.app", "identity": secret,
                "notary_profile": secret
            }),
        ];

        for value in specs {
            let encrypt_names = value.get("encrypt_names").cloned();
            let spec: JobSpec = serde_json::from_value(value).expect("valid credential job spec");
            let redacted = serde_json::to_value(spec.redacted_for_snapshot())
                .expect("redacted job spec serializes");
            assert!(!redacted.to_string().contains(secret));
            assert!(!redacted.to_string().contains("sqcg1_"));
            assert!(!redacted.to_string().contains("sqeg1_"));
            if let Some(expected) = encrypt_names {
                assert_eq!(redacted["encrypt_names"], expected);
            }
        }
    }

    #[test]
    fn job_spec_serde_defaults_match_frontend_contract() {
        let compress: JobSpec = serde_json::from_str(
            r#"{
              "kind":"compress",
              "inputs":["source"],
              "dest":"archive.zip",
              "level":5,
              "password":null,
              "encrypt_names":false,
              "split_size":null,
              "excludes":[]
            }"#,
        )
        .expect("valid compress job spec");
        match compress {
            JobSpec::Compress {
                content_policy,
                sfx_target,
                sqz_inner_format,
                completion,
                post_success,
                test_after_create,
                ..
            } => {
                assert!(content_policy.is_none());
                assert!(sfx_target.is_none());
                assert!(sqz_inner_format.is_none());
                assert!(completion.is_none());
                assert!(post_success.is_none());
                assert!(test_after_create.is_none());
            }
            other => panic!("unexpected job spec: {other:?}"),
        }

        let extract: JobSpec = serde_json::from_str(
            r#"{
              "kind":"extract",
              "path":"archive.zip",
              "dest":"out",
              "selection":null,
              "overwrite":"skip",
              "symlinks":"preserve",
              "smart":true,
              "encoding":null,
              "password":null
            }"#,
        )
        .expect("valid extract job spec");
        match extract {
            JobSpec::Extract {
                best_effort,
                expected_destination,
                expected_input_guard,
                verify_sfx,
                ..
            } => {
                assert!(!best_effort);
                assert!(expected_destination.is_none());
                assert!(expected_input_guard.is_none());
                assert!(!verify_sfx);
            }
            other => panic!("unexpected job spec: {other:?}"),
        }

        let convert: JobSpec = serde_json::from_str(
            r#"{
              "kind":"convert",
              "src":"source.zip",
              "dest":"output.7z",
              "level":5,
              "src_encoding":null,
              "src_password":null,
              "dest_password":null,
              "encrypt_names":false
            }"#,
        )
        .expect("valid legacy convert job spec");
        match convert {
            JobSpec::Convert {
                replace_existing,
                replacement_guard,
                split_size,
                ..
            } => {
                assert!(replace_existing.is_none());
                assert!(replacement_guard.is_none());
                assert!(split_size.is_none());
            }
            other => panic!("unexpected job spec: {other:?}"),
        }

        let export: JobSpec = serde_json::from_str(
            r#"{
              "kind":"export_sqz",
              "src":"source.sqz",
              "dest":"output.zip",
              "level":5,
              "dest_password":null
            }"#,
        )
        .expect("valid legacy export job spec");
        match export {
            JobSpec::ExportSqz {
                replace_existing,
                replacement_guard,
                ..
            } => {
                assert!(replace_existing.is_none());
                assert!(replacement_guard.is_none());
            }
            other => panic!("unexpected job spec: {other:?}"),
        }

        let batch: JobSpec = serde_json::from_str(
            r#"{
              "kind":"batch_extract",
              "items":[{"path":"one.zip","dest":"out","encoding":null,"password":null}],
              "overwrite":"ask",
              "symlinks":"preserve",
              "smart":true
            }"#,
        )
        .expect("valid batch extract job spec");
        match batch {
            JobSpec::BatchExtract { items, smart, .. } => {
                assert!(smart);
                assert_eq!(items.len(), 1);
                assert!(!items[0].best_effort);
            }
            other => panic!("unexpected job spec: {other:?}"),
        }

        let checksum: JobSpec =
            serde_json::from_str(r#"{"kind":"checksum","inputs":["a.txt"],"excludes":[]}"#)
                .expect("valid checksum job spec");
        match checksum {
            JobSpec::Checksum { algorithm, .. } => assert_eq!(algorithm, "sha256"),
            other => panic!("unexpected job spec: {other:?}"),
        }

        let duplicates: JobSpec =
            serde_json::from_str(r#"{"kind":"duplicate_scan","inputs":["."],"excludes":[]}"#)
                .expect("valid duplicate scan job spec");
        match duplicates {
            JobSpec::DuplicateScan { min_size, .. } => assert_eq!(min_size, 1),
            other => panic!("unexpected job spec: {other:?}"),
        }
    }
}
