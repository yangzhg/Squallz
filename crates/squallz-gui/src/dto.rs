//! IPC data-transfer objects. Everything the frontend sees crosses through
//! these serde types; errors are structured `{key, params}` pairs rendered
//! by the frontend i18n store.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use squallz_core::api::{EntryMeta, EntryType, FormatError, ResourceOptions, SafetyLimits};
use squallz_core::CreateInputEstimate;
use squallz_i18n::error_message;

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
        Self {
            key: msg.key.to_owned(),
            params: msg
                .params
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
            detail: e.to_string(),
        }
    }
}

impl From<FormatError> for ErrorDto {
    fn from(e: FormatError) -> Self {
        Self::from(&e)
    }
}

impl ErrorDto {
    pub fn other(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            key: "error.other".to_owned(),
            params: HashMap::from([("detail".to_owned(), detail.clone())]),
            detail,
        }
    }
}

/// Result of `open_archive`.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveInfo {
    /// Handle id for follow-up `list_entries` calls
    pub id: u64,
    /// Absolute path (the volume base name for split sets)
    pub path: String,
    /// File name shown in the breadcrumb
    pub name: String,
    /// Format identifier (`zip` / `7z` / `tar.gz` …)
    pub format: String,
    /// Total number of entries
    pub entry_count: usize,
    /// Volume file names of a `.001` split set (`None` for single files)
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

/// Input-side estimate for create/update-add preflight. This never guesses the
/// compressed output size.
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

/// Destination-volume disk preflight for create/update jobs.
#[derive(Debug, Clone, Serialize)]
pub struct DiskSpaceDto {
    pub path: String,
    pub required_bytes: u64,
    pub available_bytes: u64,
    pub ok: bool,
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
    pub temp_path: String,
    pub size: u64,
    pub archive_like: bool,
    pub preview_mime: Option<String>,
    pub preview_data_url: Option<String>,
}

/// Job submission parameters (`submit_job`).
#[derive(Debug, Clone, Deserialize)]
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
        excludes: Vec<String>,
    },
    /// Extract an archive (optionally a selection of display paths;
    /// directory selections end with `/` and expand by prefix).
    Extract {
        path: String,
        dest: String,
        selection: Option<Vec<String>>,
        overwrite: String,
        symlinks: String,
        smart: bool,
        encoding: Option<String>,
        password: Option<String>,
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
    },
    /// Export a SQZ container to a standard archive.
    ExportSqz {
        src: String,
        dest: String,
        level: u8,
        dest_password: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct BatchExtractItem {
    pub path: String,
    pub dest: String,
    pub encoding: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub best_effort: bool,
}

#[derive(Debug, Clone, Deserialize)]
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
    pub done: u64,
    /// 0 = unknown total (indeterminate progress bar)
    pub total: u64,
    pub current: String,
    /// Bytes processed within the current entry; 0 when unknown.
    pub current_done: u64,
    /// Total bytes for the current entry; 0 when unknown.
    pub current_total: u64,
    /// Smoothed throughput in bytes/second
    pub speed: u64,
}

/// State event payload (`job://state`).
#[derive(Debug, Clone, Serialize)]
pub struct StateEvent {
    pub id: u64,
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
    pub installed: Vec<IntegrationActionDto>,
    pub missing: Vec<String>,
    pub unsupported: Vec<String>,
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
    /// Reveal the destination folder in Finder after a successful extract job.
    pub reveal_after_extract: bool,
    /// Upper bound on total extracted bytes.
    pub safety_max_output_bytes: Option<u64>,
    /// Upper bound on archive entries.
    pub safety_max_entries: Option<u64>,
    /// Per-entry uncompressed/compressed ratio limit.
    pub safety_max_compression_ratio: Option<u32>,
    /// Compression worker threads (`None` = automatic).
    pub performance_threads: Option<usize>,
    /// Squallz-owned stream buffer budget in bytes (`None` = automatic).
    pub performance_memory_limit_bytes: Option<u64>,
}

impl SettingsDto {
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
            memory_limit: self.performance_memory_limit_bytes.filter(|v| *v > 0),
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
    use std::time::{Duration, UNIX_EPOCH};

    use squallz_core::api::{EntryMeta, EntryPath, EntryType};

    use super::{EntryDto, JobSpec, SettingsDto};

    #[test]
    fn settings_safety_limits_default_and_clamp() {
        assert!(!SettingsDto::default().reveal_after_extract);

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
        assert_eq!(custom.memory_limit, Some(512 * 1024 * 1024));
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
    fn job_spec_serde_defaults_match_frontend_contract() {
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
            JobSpec::Extract { best_effort, .. } => assert!(!best_effort),
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
