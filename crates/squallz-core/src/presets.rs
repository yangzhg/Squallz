//! Versioned archive presets shared by desktop, file-manager and CLI entry
//! layers. Presets describe reusable policy only: source paths, output paths,
//! temporary paths and plaintext credentials never belong in this file.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use squallz_format_api::{OverwritePolicy, SqzInnerFormat, SymlinkPolicy};
use thiserror::Error;

use crate::content_policy::CreateContentPolicy;

pub const PRESET_SCHEMA_VERSION: u32 = 1;
pub const MIN_SPLIT_SIZE_BYTES: u64 = 104_858;
pub const MAX_SPLIT_SIZE_BYTES: u64 = 9_007_199_254_740_991;
pub const BALANCED_CREATE_PRESET_ID: &str = "builtin.create.balanced-7z";
pub const CROSS_PLATFORM_CREATE_PRESET_ID: &str = "builtin.create.cross-platform-7z";
pub const SMART_EXTRACT_PRESET_ID: &str = "builtin.extract.smart";

const MAX_PRESET_FILE_BYTES: u64 = 1024 * 1024;
const CROSS_PLATFORM_CREATE_PRESET_LABEL: &str = "Cross-platform 7Z";
const MAX_PRESETS: usize = 64;
const MAX_PRESET_ID_BYTES: usize = 64;
const MAX_PRESET_LABEL_CHARS: usize = 40;
const MAX_FORMAT_ID_BYTES: usize = 32;
const MAX_EXCLUDE_RULES: usize = 64;
const MAX_EXCLUDE_RULE_BYTES: usize = 256;
const MAX_ENCODING_LABEL_BYTES: usize = 64;
static PRESET_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetId(String);

impl PresetId {
    pub fn new(value: impl Into<String>) -> Result<Self, PresetValidationError> {
        let value = value.into();
        validate_identifier("preset id", &value, MAX_PRESET_ID_BYTES)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetLabel(String);

impl PresetLabel {
    pub fn new(value: impl Into<String>) -> Result<Self, PresetValidationError> {
        let value = value.into();
        validate_label(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormatId(String);

impl FormatId {
    pub fn new(value: impl Into<String>) -> Result<Self, PresetValidationError> {
        let value = value.into();
        validate_identifier("format id", &value, MAX_FORMAT_ID_BYTES)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(u64);

impl ByteSize {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for ByteSize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let value = raw.parse::<u64>().map_err(serde::de::Error::custom)?;
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    Create,
    Extract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetCompressionLevel(u8);

impl PresetCompressionLevel {
    pub fn new(value: u8) -> Result<Self, PresetValidationError> {
        if (1..=9).contains(&value) {
            Ok(Self(value))
        } else {
            Err(PresetValidationError::new(
                "compression level must be between 1 and 9",
            ))
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreateCredential {
    None,
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtractCredential {
    PromptWhenNeeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SfxTargetPolicy {
    CurrentPlatform,
    Macos,
    Windows,
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreateOutput {
    Archive,
    SelfExtracting { target: SfxTargetPolicy },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VolumeMode {
    Single,
    Split { size_bytes: ByteSize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FormatSpecificOptions {
    None,
    Sqz { inner_format: SqzInnerFormat },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateDestinationBase {
    Ask,
    SourceParent,
    DefaultDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDestination {
    pub base: CreateDestinationBase,
    pub existing_output: OverwritePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateCompletionAction {
    None,
    RevealOutput,
    OpenInSquallz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostSuccessAction {
    KeepSource,
    TrashSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePreset {
    pub format: FormatId,
    pub level: PresetCompressionLevel,
    pub credential: CreateCredential,
    pub encrypt_names: bool,
    pub volumes: VolumeMode,
    pub content_policy: CreateContentPolicy,
    pub excludes: Vec<String>,
    pub output: CreateOutput,
    pub destination: CreateDestination,
    pub format_options: FormatSpecificOptions,
    pub completion: CreateCompletionAction,
    pub post_success: PostSuccessAction,
    pub test_after_create: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractDestinationBase {
    ArchiveParent,
    DefaultDirectory,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractLayout {
    Direct,
    Smart,
    ArchiveFolder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractDestination {
    pub base: ExtractDestinationBase,
    pub layout: ExtractLayout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryNameEncoding {
    Auto,
    Named { label: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractPreset {
    pub destination: ExtractDestination,
    pub existing_output: OverwritePolicy,
    pub symlinks: SymlinkPolicy,
    pub encoding: EntryNameEncoding,
    pub credential: ExtractCredential,
    pub post_success: PostSuccessAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NamedPreset {
    Create {
        id: PresetId,
        label: PresetLabel,
        built_in: bool,
        options: CreatePreset,
    },
    Extract {
        id: PresetId,
        label: PresetLabel,
        built_in: bool,
        options: ExtractPreset,
    },
}

impl NamedPreset {
    pub fn id(&self) -> &PresetId {
        match self {
            Self::Create { id, .. } | Self::Extract { id, .. } => id,
        }
    }

    pub fn label(&self) -> &PresetLabel {
        match self {
            Self::Create { label, .. } | Self::Extract { label, .. } => label,
        }
    }

    pub const fn kind(&self) -> PresetKind {
        match self {
            Self::Create { .. } => PresetKind::Create,
            Self::Extract { .. } => PresetKind::Extract,
        }
    }

    pub const fn built_in(&self) -> bool {
        match self {
            Self::Create { built_in, .. } | Self::Extract { built_in, .. } => *built_in,
        }
    }

    pub fn create_options(&self) -> Option<&CreatePreset> {
        match self {
            Self::Create { options, .. } => Some(options),
            Self::Extract { .. } => None,
        }
    }

    pub fn extract_options(&self) -> Option<&ExtractPreset> {
        match self {
            Self::Extract { options, .. } => Some(options),
            Self::Create { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PresetBindings {
    pub app_default_create: Option<PresetId>,
    pub app_default_extract: Option<PresetId>,
    pub file_manager_create: Option<PresetId>,
    pub file_manager_extract: Option<PresetId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub presets: Vec<NamedPreset>,
    pub bindings: PresetBindings,
}

#[derive(Deserialize)]
struct PresetSchemaVersion {
    schema_version: u64,
}

const fn default_create_destination() -> CreateDestination {
    CreateDestination {
        base: CreateDestinationBase::Ask,
        existing_output: OverwritePolicy::Ask,
    }
}

impl Default for PresetDocument {
    fn default() -> Self {
        Self::seeded()
    }
}

impl PresetDocument {
    pub fn seeded() -> Self {
        let balanced = balanced_create_preset();
        let cross_platform = cross_platform_create_preset();
        let extract = smart_extract_preset();
        Self {
            schema_version: PRESET_SCHEMA_VERSION,
            revision: 0,
            presets: vec![balanced, cross_platform, extract],
            bindings: PresetBindings {
                app_default_create: Some(preset_id(CROSS_PLATFORM_CREATE_PRESET_ID)),
                app_default_extract: Some(preset_id(SMART_EXTRACT_PRESET_ID)),
                file_manager_create: Some(preset_id(CROSS_PLATFORM_CREATE_PRESET_ID)),
                file_manager_extract: Some(preset_id(SMART_EXTRACT_PRESET_ID)),
            },
        }
    }

    pub fn preset(&self, id: &PresetId) -> Option<&NamedPreset> {
        self.presets.iter().find(|preset| preset.id() == id)
    }

    pub fn validate(&self) -> Result<(), PresetValidationError> {
        if self.schema_version != PRESET_SCHEMA_VERSION {
            return Err(PresetValidationError::new(format!(
                "unsupported preset schema version {}",
                self.schema_version
            )));
        }
        validate_preset_collection(
            &self.presets,
            MAX_PRESETS,
            &[
                BALANCED_CREATE_PRESET_ID,
                CROSS_PLATFORM_CREATE_PRESET_ID,
                SMART_EXTRACT_PRESET_ID,
            ],
        )?;
        validate_builtin_preset(&self.presets, balanced_create_preset())?;
        validate_builtin_preset(&self.presets, cross_platform_create_preset())?;
        validate_builtin_preset(&self.presets, smart_extract_preset())?;
        validate_preset_bindings(&self.presets, &self.bindings)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PresetValidationError {
    message: String,
}

impl PresetValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum PresetError {
    #[error("preset storage failed: {0}")]
    Io(#[from] io::Error),
    #[error("preset document is not valid JSON: {0}")]
    Decode(String),
    #[error("preset schema version {found} is not supported")]
    UnsupportedVersion { found: u64 },
    #[error("preset document is invalid: {0}")]
    Validation(#[from] PresetValidationError),
    #[error("preset document changed (expected revision {expected}, found {actual})")]
    RevisionConflict { expected: u64, actual: u64 },
}

#[derive(Debug, Clone)]
pub struct PresetStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl PresetStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let lock_path = sibling_artifact_path(&path, "lock", None);
        Self { path, lock_path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<PresetDocument, PresetError> {
        self.ensure_parent()?;
        let lock = self.open_lock_file()?;
        fs4::FileExt::lock_shared(&lock)?;
        read_document(&self.path)
    }

    pub fn compare_and_swap(
        &self,
        expected_revision: u64,
        mut replacement: PresetDocument,
    ) -> Result<PresetDocument, PresetError> {
        self.ensure_parent()?;
        let lock = self.open_lock_file()?;
        fs4::FileExt::lock(&lock)?;
        let current = read_document(&self.path)?;
        if current.revision != expected_revision {
            return Err(PresetError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        if replacement.revision != expected_revision {
            return Err(PresetError::RevisionConflict {
                expected: expected_revision,
                actual: replacement.revision,
            });
        }
        replacement.revision = expected_revision.checked_add(1).ok_or_else(|| {
            PresetValidationError::new("preset document revision cannot increase further")
        })?;
        replacement.validate()?;
        let mut contents = serde_json::to_vec_pretty(&replacement)
            .map_err(|error| PresetError::Decode(error.to_string()))?;
        contents.push(b'\n');
        write_document_atomically(&self.path, &contents)?;
        Ok(replacement)
    }

    fn ensure_parent(&self) -> io::Result<()> {
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "preset path must have a parent directory",
            )
        })?;
        fs::create_dir_all(parent)
    }

    fn open_lock_file(&self) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&self.lock_path)
    }
}

fn balanced_create_preset() -> NamedPreset {
    NamedPreset::Create {
        id: preset_id(BALANCED_CREATE_PRESET_ID),
        label: preset_label("Balanced 7Z"),
        built_in: true,
        options: CreatePreset {
            format: format_id("7z"),
            level: PresetCompressionLevel(5),
            credential: CreateCredential::None,
            encrypt_names: false,
            volumes: VolumeMode::Single,
            content_policy: CreateContentPolicy::Custom,
            excludes: Vec::new(),
            output: CreateOutput::Archive,
            destination: default_create_destination(),
            format_options: FormatSpecificOptions::None,
            completion: CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
        },
    }
}

fn cross_platform_create_preset() -> NamedPreset {
    NamedPreset::Create {
        id: preset_id(CROSS_PLATFORM_CREATE_PRESET_ID),
        label: preset_label(CROSS_PLATFORM_CREATE_PRESET_LABEL),
        built_in: true,
        options: CreatePreset {
            format: format_id("7z"),
            level: PresetCompressionLevel(5),
            credential: CreateCredential::None,
            encrypt_names: false,
            volumes: VolumeMode::Single,
            content_policy: CreateContentPolicy::CrossPlatformClean,
            excludes: Vec::new(),
            output: CreateOutput::Archive,
            destination: default_create_destination(),
            format_options: FormatSpecificOptions::None,
            completion: CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
        },
    }
}

fn smart_extract_preset() -> NamedPreset {
    NamedPreset::Extract {
        id: preset_id(SMART_EXTRACT_PRESET_ID),
        label: preset_label("Smart extract"),
        built_in: true,
        options: ExtractPreset {
            destination: ExtractDestination {
                base: ExtractDestinationBase::DefaultDirectory,
                layout: ExtractLayout::Smart,
            },
            existing_output: OverwritePolicy::Ask,
            symlinks: SymlinkPolicy::Preserve,
            encoding: EntryNameEncoding::Auto,
            credential: ExtractCredential::PromptWhenNeeded,
            post_success: PostSuccessAction::KeepSource,
        },
    }
}

fn preset_id(value: &str) -> PresetId {
    PresetId(value.to_owned())
}

fn preset_label(value: &str) -> PresetLabel {
    PresetLabel(value.to_owned())
}

fn format_id(value: &str) -> FormatId {
    FormatId(value.to_owned())
}

fn validate_identifier(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), PresetValidationError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(PresetValidationError::new(format!(
            "{field} must contain 1 to {max_bytes} bytes"
        )));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(PresetValidationError::new(format!(
            "{field} contains unsupported characters"
        )));
    }
    Ok(())
}

fn validate_label(value: &str) -> Result<(), PresetValidationError> {
    let chars = value.chars().count();
    if value.trim() != value || chars == 0 || chars > MAX_PRESET_LABEL_CHARS {
        return Err(PresetValidationError::new(format!(
            "preset label must contain 1 to {MAX_PRESET_LABEL_CHARS} characters without outer whitespace"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(PresetValidationError::new(
            "preset label cannot contain control characters",
        ));
    }
    Ok(())
}

fn validate_preset_collection(
    presets: &[NamedPreset],
    max_presets: usize,
    reserved_ids: &[&str],
) -> Result<(), PresetValidationError> {
    if presets.len() > max_presets {
        return Err(PresetValidationError::new(format!(
            "preset count exceeds {max_presets}"
        )));
    }

    let mut ids = HashSet::new();
    let mut labels_by_kind: HashMap<PresetKind, HashSet<String>> = HashMap::new();
    for preset in presets {
        validate_identifier("preset id", preset.id().as_str(), MAX_PRESET_ID_BYTES)?;
        validate_label(preset.label().as_str())?;
        if !ids.insert(preset.id().as_str()) {
            return Err(PresetValidationError::new(format!(
                "duplicate preset id {}",
                preset.id().as_str()
            )));
        }
        let normalized_label = preset.label().as_str().to_lowercase();
        if !labels_by_kind
            .entry(preset.kind())
            .or_default()
            .insert(normalized_label)
        {
            return Err(PresetValidationError::new(format!(
                "duplicate {:?} preset label",
                preset.kind()
            )));
        }
        validate_named_preset(preset, reserved_ids)?;
    }
    Ok(())
}

fn validate_named_preset(
    preset: &NamedPreset,
    reserved_ids: &[&str],
) -> Result<(), PresetValidationError> {
    let id = preset.id().as_str();
    let is_reserved = reserved_ids.contains(&id);
    if preset.built_in() && !is_reserved {
        return Err(PresetValidationError::new(
            "only Squallz built-in preset ids may be marked built_in",
        ));
    }
    if !preset.built_in() && is_reserved {
        return Err(PresetValidationError::new(
            "built-in preset ids are reserved",
        ));
    }
    match preset {
        NamedPreset::Create { options, .. } => validate_create_preset(options),
        NamedPreset::Extract { options, .. } => validate_extract_preset(options),
    }
}

fn validate_create_preset(options: &CreatePreset) -> Result<(), PresetValidationError> {
    validate_identifier("format id", options.format.as_str(), MAX_FORMAT_ID_BYTES)?;
    PresetCompressionLevel::new(options.level.get())?;
    if !matches!(
        (
            options.destination.base,
            options.destination.existing_output
        ),
        (CreateDestinationBase::Ask, OverwritePolicy::Ask)
            | (
                CreateDestinationBase::SourceParent | CreateDestinationBase::DefaultDirectory,
                OverwritePolicy::RenameBoth
            )
    ) {
        return Err(PresetValidationError::new(
            "create destination uses an unsupported base and existing-output combination",
        ));
    }
    if options.credential == CreateCredential::Prompt
        && !matches!(options.format.as_str(), "zip" | "7z")
    {
        return Err(PresetValidationError::new(
            "credential prompts are supported only by ZIP and 7z presets",
        ));
    }
    if options.encrypt_names && options.credential == CreateCredential::None {
        return Err(PresetValidationError::new(
            "file-name encryption requires a credential policy",
        ));
    }
    if options.encrypt_names && options.format.as_str() != "7z" {
        return Err(PresetValidationError::new(
            "file-name encryption is supported only by 7z presets",
        ));
    }
    if let VolumeMode::Split { size_bytes } = options.volumes {
        if !(MIN_SPLIT_SIZE_BYTES..=MAX_SPLIT_SIZE_BYTES).contains(&size_bytes.get()) {
            return Err(PresetValidationError::new(format!(
                "split size must be between {MIN_SPLIT_SIZE_BYTES} and {MAX_SPLIT_SIZE_BYTES} bytes"
            )));
        }
    }
    if options.completion == CreateCompletionAction::OpenInSquallz
        && options.volumes != VolumeMode::Single
    {
        return Err(PresetValidationError::new(
            "split archive presets cannot open the output in Squallz",
        ));
    }
    if options.completion == CreateCompletionAction::OpenInSquallz
        && matches!(options.output, CreateOutput::SelfExtracting { .. })
    {
        return Err(PresetValidationError::new(
            "self-extracting presets cannot open the output in Squallz",
        ));
    }
    if options.content_policy != CreateContentPolicy::Custom && !options.excludes.is_empty() {
        return Err(PresetValidationError::new(
            "only custom content policies may store exclude rules",
        ));
    }
    if options.excludes.len() > MAX_EXCLUDE_RULES {
        return Err(PresetValidationError::new(format!(
            "exclude rule count exceeds {MAX_EXCLUDE_RULES}"
        )));
    }
    for rule in &options.excludes {
        if rule.is_empty()
            || rule.trim() != rule
            || rule.len() > MAX_EXCLUDE_RULE_BYTES
            || rule.contains('\0')
        {
            return Err(PresetValidationError::new(
                "exclude rules must be trimmed, non-empty and bounded",
            ));
        }
    }
    crate::PathFilter::new(&options.excludes).map_err(|error| {
        PresetValidationError::new(format!("exclude rules are invalid: {error}"))
    })?;
    if options.post_success == PostSuccessAction::TrashSource
        && !options
            .content_policy
            .resolve_excludes(&options.excludes)
            .is_empty()
    {
        return Err(PresetValidationError::new(
            "source cleanup requires every selected item to be included",
        ));
    }
    if options.post_success == PostSuccessAction::TrashSource && !options.test_after_create {
        return Err(PresetValidationError::new(
            "source cleanup requires creation-time integrity testing",
        ));
    }
    if matches!(options.output, CreateOutput::SelfExtracting { .. }) {
        if options.format.as_str() != "zip" {
            return Err(PresetValidationError::new(
                "self-extracting presets require ZIP format",
            ));
        }
        if options.volumes != VolumeMode::Single {
            return Err(PresetValidationError::new(
                "self-extracting presets cannot create split volumes",
            ));
        }
        if options.encrypt_names {
            return Err(PresetValidationError::new(
                "self-extracting ZIP presets cannot encrypt file names",
            ));
        }
    }
    match &options.format_options {
        FormatSpecificOptions::None => {
            if options.format.as_str() == "sqz" {
                return Err(PresetValidationError::new(
                    "SQZ presets must choose an inner format",
                ));
            }
        }
        FormatSpecificOptions::Sqz { .. } => {
            if options.format.as_str() != "sqz" {
                return Err(PresetValidationError::new(
                    "SQZ format options require SQZ output",
                ));
            }
        }
    }
    Ok(())
}

fn validate_extract_preset(options: &ExtractPreset) -> Result<(), PresetValidationError> {
    if options.post_success != PostSuccessAction::KeepSource {
        return Err(PresetValidationError::new(
            "extract presets cannot remove the source archive",
        ));
    }
    if !matches!(
        (options.destination.base, options.destination.layout),
        (
            ExtractDestinationBase::DefaultDirectory,
            ExtractLayout::Smart | ExtractLayout::ArchiveFolder
        ) | (ExtractDestinationBase::ArchiveParent, ExtractLayout::Direct)
            | (ExtractDestinationBase::Ask, ExtractLayout::Direct)
    ) {
        return Err(PresetValidationError::new(
            "extract destination uses an unsupported base and layout combination",
        ));
    }
    if let EntryNameEncoding::Named { label } = &options.encoding {
        if label.is_empty()
            || label.trim() != label
            || label.len() > MAX_ENCODING_LABEL_BYTES
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(PresetValidationError::new(
                "encoding label contains unsupported characters",
            ));
        }
    }
    Ok(())
}

fn validate_builtin_preset(
    presets: &[NamedPreset],
    canonical: NamedPreset,
) -> Result<(), PresetValidationError> {
    let actual = presets
        .iter()
        .find(|preset| preset.id() == canonical.id())
        .ok_or_else(|| {
            PresetValidationError::new(format!(
                "required built-in preset {} is missing",
                canonical.id().as_str()
            ))
        })?;
    if actual != &canonical {
        return Err(PresetValidationError::new(format!(
            "built-in preset {} cannot be changed",
            canonical.id().as_str()
        )));
    }
    Ok(())
}

fn validate_binding(
    presets: &[NamedPreset],
    field: &str,
    id: Option<&PresetId>,
    kind: PresetKind,
) -> Result<(), PresetValidationError> {
    let Some(id) = id else {
        return Ok(());
    };
    let preset = presets
        .iter()
        .find(|preset| preset.id() == id)
        .ok_or_else(|| {
            PresetValidationError::new(format!("{field} references missing preset {}", id.as_str()))
        })?;
    if preset.kind() != kind {
        return Err(PresetValidationError::new(format!(
            "{field} references the wrong preset kind"
        )));
    }
    Ok(())
}

fn validate_preset_bindings(
    presets: &[NamedPreset],
    bindings: &PresetBindings,
) -> Result<(), PresetValidationError> {
    validate_binding(
        presets,
        "app_default_create",
        bindings.app_default_create.as_ref(),
        PresetKind::Create,
    )?;
    validate_binding(
        presets,
        "app_default_extract",
        bindings.app_default_extract.as_ref(),
        PresetKind::Extract,
    )?;
    validate_binding(
        presets,
        "file_manager_create",
        bindings.file_manager_create.as_ref(),
        PresetKind::Create,
    )?;
    validate_binding(
        presets,
        "file_manager_extract",
        bindings.file_manager_extract.as_ref(),
        PresetKind::Extract,
    )?;

    if let Some(id) = bindings.file_manager_create.as_ref() {
        let preset = presets
            .iter()
            .find(|preset| preset.id() == id)
            .ok_or_else(|| {
                PresetValidationError::new("file_manager_create references a missing preset")
            })?;
        let options = preset.create_options().ok_or_else(|| {
            PresetValidationError::new("file_manager_create must reference a create preset")
        })?;
        if options.format.as_str() != "7z"
            || options.credential != CreateCredential::None
            || options.output != CreateOutput::Archive
            || options.completion == CreateCompletionAction::OpenInSquallz
            || options.post_success != PostSuccessAction::KeepSource
        {
            return Err(PresetValidationError::new(
                "file-manager create preset must be a standard 7z archive without a credential prompt, in-app completion or source cleanup",
            ));
        }
    }
    if let Some(id) = bindings.file_manager_extract.as_ref() {
        let preset = presets
            .iter()
            .find(|preset| preset.id() == id)
            .ok_or_else(|| {
                PresetValidationError::new("file_manager_extract references a missing preset")
            })?;
        let options = preset.extract_options().ok_or_else(|| {
            PresetValidationError::new("file_manager_extract must reference an extract preset")
        })?;
        if options.post_success != PostSuccessAction::KeepSource {
            return Err(PresetValidationError::new(
                "file-manager extract preset cannot remove the source archive",
            ));
        }
    }
    Ok(())
}

fn read_document(path: &Path) -> Result<PresetDocument, PresetError> {
    let source = readable_document_path(path);
    let file = match File::open(&source) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PresetDocument::seeded());
        }
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > MAX_PRESET_FILE_BYTES {
        return Err(PresetError::Decode(format!(
            "preset file exceeds {MAX_PRESET_FILE_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::new();
    file.take(MAX_PRESET_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PRESET_FILE_BYTES {
        return Err(PresetError::Decode(format!(
            "preset file exceeds {MAX_PRESET_FILE_BYTES} bytes"
        )));
    }
    decode_document(&bytes)
}

fn decode_document(bytes: &[u8]) -> Result<PresetDocument, PresetError> {
    let version: PresetSchemaVersion =
        serde_json::from_slice(bytes).map_err(|error| PresetError::Decode(error.to_string()))?;
    if version.schema_version != u64::from(PRESET_SCHEMA_VERSION) {
        return Err(PresetError::UnsupportedVersion {
            found: version.schema_version,
        });
    }
    let document: PresetDocument =
        serde_json::from_slice(bytes).map_err(|error| PresetError::Decode(error.to_string()))?;
    document.validate()?;
    Ok(document)
}

fn readable_document_path(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    #[cfg(target_os = "windows")]
    {
        let backup = sibling_artifact_path(path, "backup", None);
        if backup.exists() {
            return backup;
        }
    }
    path.to_path_buf()
}

fn write_document_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let sequence = PRESET_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = sibling_artifact_path(path, "tmp", Some(sequence));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp = options.open(&temp_path)?;
    let write_result = temp.write_all(contents).and_then(|()| temp.sync_all());
    drop(temp);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = replace_document_file(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    crate::sync_directory(crate::parent_or_current(path))
}

#[cfg(not(target_os = "windows"))]
fn replace_document_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temp_path, path)
}

#[cfg(target_os = "windows")]
fn replace_document_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    let backup_path = sibling_artifact_path(path, "backup", None);
    prepare_windows_replacement(path, &backup_path)?;
    let had_current = path.exists();
    if had_current {
        fs::rename(path, &backup_path)?;
    }
    match fs::rename(temp_path, path) {
        Ok(()) => {
            if had_current {
                fs::remove_file(backup_path)?;
            }
            Ok(())
        }
        Err(error) => {
            if had_current {
                let _ = fs::rename(&backup_path, path);
            }
            Err(error)
        }
    }
}

#[cfg(any(test, target_os = "windows"))]
fn prepare_windows_replacement(path: &Path, backup_path: &Path) -> io::Result<()> {
    if backup_path.exists() && !path.exists() {
        fs::rename(backup_path, path)?;
    }
    if backup_path.exists() {
        fs::remove_file(backup_path)?;
    }
    Ok(())
}

fn sibling_artifact_path(path: &Path, tag: &str, sequence: Option<u64>) -> PathBuf {
    let file_name = path
        .file_name()
        .map_or_else(|| OsString::from("presets.json"), OsString::from);
    let mut artifact = OsString::from(".");
    artifact.push(file_name);
    artifact.push(format!(".{tag}"));
    if let Some(sequence) = sequence {
        artifact.push(format!("-{}-{sequence}", std::process::id()));
    }
    path.with_file_name(artifact)
}

#[cfg(test)]
mod tests {
    use super::{
        cross_platform_create_preset, decode_document, format_id, prepare_windows_replacement,
        preset_id, preset_label, sibling_artifact_path, smart_extract_preset,
        validate_create_preset, validate_extract_preset, ByteSize, CreateCompletionAction,
        CreateContentPolicy, CreateCredential, CreateDestination, CreateDestinationBase,
        CreateOutput, CreatePreset, ExtractCredential, ExtractDestination, ExtractDestinationBase,
        ExtractLayout, ExtractPreset, FormatSpecificOptions, NamedPreset, OverwritePolicy,
        PostSuccessAction, PresetCompressionLevel, PresetDocument, PresetError, PresetStore,
        SqzInnerFormat, SymlinkPolicy, VolumeMode, BALANCED_CREATE_PRESET_ID,
        CROSS_PLATFORM_CREATE_PRESET_ID, MAX_PRESET_FILE_BYTES, MAX_SPLIT_SIZE_BYTES,
        MIN_SPLIT_SIZE_BYTES, PRESET_SCHEMA_VERSION,
    };

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "squallz-preset-{name}-{}-{unique}",
                std::process::id()
            ))
            .join("presets.json")
    }

    fn custom_create(id: &str, label: &str) -> NamedPreset {
        NamedPreset::Create {
            id: preset_id(id),
            label: preset_label(label),
            built_in: false,
            options: CreatePreset {
                format: format_id("zip"),
                level: PresetCompressionLevel::new(6).expect("valid test level"),
                credential: CreateCredential::Prompt,
                encrypt_names: false,
                volumes: VolumeMode::Split {
                    size_bytes: ByteSize::new(MIN_SPLIT_SIZE_BYTES),
                },
                content_policy: CreateContentPolicy::Custom,
                excludes: vec![".git".to_owned()],
                output: CreateOutput::Archive,
                destination: CreateDestination {
                    base: CreateDestinationBase::Ask,
                    existing_output: OverwritePolicy::Ask,
                },
                format_options: FormatSpecificOptions::None,
                completion: CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
            },
        }
    }

    fn set_last_create_split_size(document: &mut PresetDocument, size: u64) {
        let NamedPreset::Create { options, .. } = document
            .presets
            .last_mut()
            .expect("custom preset should be present")
        else {
            panic!("custom preset should be a create preset");
        };
        options.volumes = VolumeMode::Split {
            size_bytes: ByteSize::new(size),
        };
    }

    #[test]
    fn seeded_document_round_trips_without_plaintext_password_fields() {
        let document = PresetDocument::seeded();
        document.validate().expect("seeded presets should validate");
        let json = serde_json::to_string_pretty(&document).expect("preset JSON should serialize");
        assert!(!json.contains("password"), "{json}");
        assert!(!json.contains("secret_ref"), "{json}");
        assert!(json.contains("\"schema_version\": 1"), "{json}");
        let decoded = decode_document(json.as_bytes()).expect("preset JSON should round-trip");
        assert_eq!(decoded, document);
    }

    #[test]
    fn seeded_create_bindings_use_the_immutable_cross_platform_preset() {
        let document = PresetDocument::seeded();
        let cross_platform_id = preset_id(CROSS_PLATFORM_CREATE_PRESET_ID);
        assert_eq!(
            document.bindings.app_default_create.as_ref(),
            Some(&cross_platform_id)
        );
        assert_eq!(
            document.bindings.file_manager_create.as_ref(),
            Some(&cross_platform_id)
        );

        let expected = cross_platform_create_preset();
        assert_eq!(document.preset(expected.id()), Some(&expected));
        let options = expected
            .create_options()
            .expect("cross-platform built-in should be a create preset");
        assert_eq!(
            options.content_policy,
            CreateContentPolicy::CrossPlatformClean
        );
        assert!(options.excludes.is_empty());
        assert_eq!(
            options.destination,
            CreateDestination {
                base: CreateDestinationBase::Ask,
                existing_output: OverwritePolicy::Ask,
            }
        );
        assert_eq!(options.completion, CreateCompletionAction::None);
        assert_eq!(options.post_success, PostSuccessAction::KeepSource);
        assert!(!options.test_after_create);

        let balanced = document
            .preset(&preset_id(BALANCED_CREATE_PRESET_ID))
            .and_then(NamedPreset::create_options)
            .expect("balanced built-in should remain available");
        assert_eq!(balanced.content_policy, CreateContentPolicy::Custom);
    }

    #[test]
    fn byte_size_serializes_as_a_decimal_string_and_rejects_numbers() {
        let json = serde_json::to_string(&ByteSize::new(4_294_967_296))
            .expect("byte size should serialize");
        assert_eq!(json, "\"4294967296\"");
        assert!(serde_json::from_str::<ByteSize>("4294967296").is_err());
    }

    #[test]
    fn unsupported_versions_and_unknown_fields_are_rejected() {
        let mut unknown_document = PresetDocument::seeded();
        unknown_document.schema_version = PRESET_SCHEMA_VERSION + 1;
        let unknown = serde_json::to_vec(&unknown_document).expect("serialize unknown version");
        assert!(matches!(
            decode_document(&unknown),
            Err(PresetError::UnsupportedVersion { found: 2 })
        ));

        let mut value = serde_json::to_value(PresetDocument::seeded()).expect("serialize document");
        value
            .as_object_mut()
            .expect("document should be an object")
            .insert("future_field".to_owned(), serde_json::json!(true));
        let bytes = serde_json::to_vec(&value).expect("serialize modified document");
        assert!(matches!(
            decode_document(&bytes),
            Err(PresetError::Decode(_))
        ));

        let mut value = serde_json::to_value(PresetDocument::seeded()).expect("serialize document");
        let options = value
            .get_mut("presets")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|presets| presets.first_mut())
            .and_then(|preset| preset.get_mut("options"))
            .and_then(serde_json::Value::as_object_mut)
            .expect("create options should be an object");
        assert!(options.remove("completion").is_some());
        let bytes = serde_json::to_vec(&value).expect("serialize incomplete document");
        assert!(matches!(
            decode_document(&bytes),
            Err(PresetError::Decode(_))
        ));
    }

    #[test]
    fn duplicate_json_fields_and_secret_references_are_rejected() {
        let json = serde_json::to_string(&PresetDocument::seeded()).expect("serialize document");
        let duplicate = json.replacen("\"revision\":0", "\"revision\":0,\"revision\":1", 1);
        assert!(matches!(
            decode_document(duplicate.as_bytes()),
            Err(PresetError::Decode(_))
        ));

        let secret_reference = json.replacen(
            "\"credential\":{\"kind\":\"none\"}",
            "\"credential\":{\"kind\":\"secret_ref\",\"secret_ref\":\"keyring:test\"}",
            1,
        );
        assert_ne!(secret_reference, json);
        assert!(matches!(
            decode_document(secret_reference.as_bytes()),
            Err(PresetError::Decode(_))
        ));
    }

    #[test]
    fn only_custom_content_policy_accepts_explicit_excludes() {
        let mut options = cross_platform_create_preset()
            .create_options()
            .expect("cross-platform built-in should be a create preset")
            .clone();

        options.excludes.push("*.tmp".to_owned());
        assert!(validate_create_preset(&options).is_err());

        options.content_policy = CreateContentPolicy::KeepAllFiles;
        assert!(validate_create_preset(&options).is_err());

        options.content_policy = CreateContentPolicy::Custom;
        assert!(validate_create_preset(&options).is_ok());
    }

    #[test]
    fn split_sizes_follow_gui_and_javascript_boundaries() {
        assert_eq!(MIN_SPLIT_SIZE_BYTES, 104_858);
        assert_eq!(MAX_SPLIT_SIZE_BYTES, 9_007_199_254_740_991);

        let mut document = PresetDocument::seeded();
        document
            .presets
            .push(custom_create("user.create.boundary", "Boundary"));
        assert!(document.validate().is_ok());

        set_last_create_split_size(&mut document, MAX_SPLIT_SIZE_BYTES);
        assert!(document.validate().is_ok());
        set_last_create_split_size(&mut document, MIN_SPLIT_SIZE_BYTES - 1);
        assert!(document.validate().is_err());
        set_last_create_split_size(&mut document, MAX_SPLIT_SIZE_BYTES + 1);
        assert!(document.validate().is_err());
    }

    #[test]
    fn sqz_payload_and_exclude_globs_use_runtime_schema_rules() {
        let mut document = PresetDocument::seeded();
        let mut preset = custom_create("user.create.sqz", "Native SQZ payload");
        let NamedPreset::Create { options, .. } = &mut preset else {
            panic!("custom preset should be a create preset");
        };
        options.format = format_id("sqz");
        options.credential = CreateCredential::None;
        options.volumes = VolumeMode::Single;
        options.format_options = FormatSpecificOptions::Sqz {
            inner_format: SqzInnerFormat::Sqz,
        };
        document.presets.push(preset);
        document.validate().expect("native SQZ should validate");
        let json = serde_json::to_string(&document).expect("serialize SQZ preset");
        assert!(json.contains("\"inner_format\":\"sqz\""), "{json}");

        let NamedPreset::Create { options, .. } = document
            .presets
            .last_mut()
            .expect("SQZ preset should be present")
        else {
            panic!("SQZ preset should be a create preset");
        };
        options.excludes = vec!["[".to_owned()];
        assert!(document.validate().is_err());
    }

    #[test]
    fn create_destination_accepts_only_executable_combinations() {
        let bases = [
            CreateDestinationBase::Ask,
            CreateDestinationBase::SourceParent,
            CreateDestinationBase::DefaultDirectory,
        ];
        let policies = [
            OverwritePolicy::Ask,
            OverwritePolicy::Skip,
            OverwritePolicy::Overwrite,
            OverwritePolicy::RenameBoth,
        ];

        for base in bases {
            for existing_output in policies {
                let mut options = cross_platform_create_preset()
                    .create_options()
                    .expect("cross-platform built-in should be a create preset")
                    .clone();
                options.destination = CreateDestination {
                    base,
                    existing_output,
                };
                let expected = matches!(
                    (base, existing_output),
                    (CreateDestinationBase::Ask, OverwritePolicy::Ask)
                        | (
                            CreateDestinationBase::SourceParent
                                | CreateDestinationBase::DefaultDirectory,
                            OverwritePolicy::RenameBoth
                        )
                );
                assert_eq!(
                    validate_create_preset(&options).is_ok(),
                    expected,
                    "unexpected validation result for {base:?} + {existing_output:?}"
                );
            }
        }
    }

    #[test]
    fn extract_destination_accepts_only_canonical_gui_combinations() {
        let combinations = [
            (
                ExtractDestinationBase::DefaultDirectory,
                ExtractLayout::Smart,
                true,
            ),
            (
                ExtractDestinationBase::DefaultDirectory,
                ExtractLayout::ArchiveFolder,
                true,
            ),
            (
                ExtractDestinationBase::ArchiveParent,
                ExtractLayout::Direct,
                true,
            ),
            (ExtractDestinationBase::Ask, ExtractLayout::Direct, true),
            (
                ExtractDestinationBase::DefaultDirectory,
                ExtractLayout::Direct,
                false,
            ),
            (
                ExtractDestinationBase::ArchiveParent,
                ExtractLayout::Smart,
                false,
            ),
            (
                ExtractDestinationBase::ArchiveParent,
                ExtractLayout::ArchiveFolder,
                false,
            ),
            (ExtractDestinationBase::Ask, ExtractLayout::Smart, false),
            (
                ExtractDestinationBase::Ask,
                ExtractLayout::ArchiveFolder,
                false,
            ),
        ];

        for (base, layout, expected) in combinations {
            let mut options = smart_extract_preset()
                .extract_options()
                .expect("built-in extract preset should contain extract options")
                .clone();
            options.destination = ExtractDestination { base, layout };
            assert_eq!(
                validate_extract_preset(&options).is_ok(),
                expected,
                "unexpected validation result for {base:?} + {layout:?}"
            );
        }
    }

    #[test]
    fn capability_conflicts_are_rejected() {
        let mut document = PresetDocument::seeded();
        document.presets.push(NamedPreset::Create {
            id: preset_id("user.invalid-sfx"),
            label: preset_label("Invalid SFX"),
            built_in: false,
            options: CreatePreset {
                format: format_id("7z"),
                level: PresetCompressionLevel::new(5).expect("valid test level"),
                credential: CreateCredential::None,
                encrypt_names: false,
                volumes: VolumeMode::Single,
                content_policy: CreateContentPolicy::Custom,
                excludes: Vec::new(),
                output: CreateOutput::SelfExtracting {
                    target: super::SfxTargetPolicy::CurrentPlatform,
                },
                destination: CreateDestination {
                    base: CreateDestinationBase::Ask,
                    existing_output: OverwritePolicy::Ask,
                },
                format_options: FormatSpecificOptions::None,
                completion: CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
            },
        });
        assert!(document.validate().is_err());
    }

    #[test]
    fn open_in_squallz_rejects_split_and_self_extracting_outputs() {
        let preset = custom_create("user.create.open", "Open after creating");
        let NamedPreset::Create { mut options, .. } = preset else {
            panic!("custom preset should be a create preset");
        };
        options.completion = CreateCompletionAction::OpenInSquallz;
        assert!(validate_create_preset(&options).is_err());

        options.volumes = VolumeMode::Single;
        options.output = CreateOutput::SelfExtracting {
            target: super::SfxTargetPolicy::CurrentPlatform,
        };
        assert!(validate_create_preset(&options).is_err());

        options.output = CreateOutput::Archive;
        assert!(validate_create_preset(&options).is_ok());

        options.volumes = VolumeMode::Split {
            size_bytes: ByteSize::new(MIN_SPLIT_SIZE_BYTES),
        };
        options.completion = CreateCompletionAction::RevealOutput;
        assert!(validate_create_preset(&options).is_ok());
    }

    #[test]
    fn create_actions_round_trip() {
        let mut document = PresetDocument::seeded();
        let mut preset = custom_create("user.create.actions", "Create actions");
        let NamedPreset::Create { options, .. } = &mut preset else {
            panic!("custom preset should be a create preset");
        };
        options.destination = CreateDestination {
            base: CreateDestinationBase::SourceParent,
            existing_output: OverwritePolicy::RenameBoth,
        };
        options.completion = CreateCompletionAction::RevealOutput;
        options.post_success = PostSuccessAction::TrashSource;
        options.test_after_create = true;
        options.content_policy = CreateContentPolicy::KeepAllFiles;
        options.excludes.clear();
        document.presets.push(preset);

        document.validate().expect("create actions should validate");
        let bytes = serde_json::to_vec(&document).expect("serialize create actions");
        let decoded = decode_document(&bytes).expect("decode create actions");
        assert_eq!(decoded, document);
    }

    #[test]
    fn source_cleanup_rejects_content_policies_that_skip_selected_items() {
        let mut preset = custom_create("user.create.cleanup-excludes", "Cleanup exclusions");
        let NamedPreset::Create { options, .. } = &mut preset else {
            panic!("custom preset should be a create preset");
        };
        options.post_success = PostSuccessAction::TrashSource;
        options.test_after_create = true;
        options.excludes = vec!["*.raw".to_owned()];
        assert!(validate_create_preset(options).is_err());

        options.excludes.clear();
        options.content_policy = CreateContentPolicy::CrossPlatformClean;
        assert!(validate_create_preset(options).is_err());

        options.content_policy = CreateContentPolicy::KeepAllFiles;
        options.test_after_create = false;
        assert!(validate_create_preset(options).is_err());
        options.test_after_create = true;
        assert!(validate_create_preset(options).is_ok());
    }

    #[test]
    fn credential_prompt_requires_a_format_with_data_encryption() {
        let cases = [
            ("zip", true),
            ("7z", true),
            ("sqz", false),
            ("tar.zst", false),
            ("wim", false),
            ("future-format", false),
        ];
        for (format, expected) in cases {
            let mut preset = custom_create("user.create.credential", "Credential policy");
            let NamedPreset::Create { options, .. } = &mut preset else {
                panic!("custom preset should be a create preset");
            };
            options.format = format_id(format);
            options.volumes = VolumeMode::Single;
            options.format_options = if format == "sqz" {
                FormatSpecificOptions::Sqz {
                    inner_format: SqzInnerFormat::Sqz,
                }
            } else {
                FormatSpecificOptions::None
            };
            assert_eq!(
                validate_create_preset(options).is_ok(),
                expected,
                "unexpected credential validation for {format}"
            );
        }
    }

    #[test]
    fn deleting_a_bound_preset_requires_clearing_its_binding() {
        let mut document = PresetDocument::seeded();
        document
            .presets
            .retain(|preset| preset.id().as_str() != BALANCED_CREATE_PRESET_ID);
        assert!(document.validate().is_err());
    }

    #[test]
    fn file_manager_bindings_reject_source_cleanup() {
        let create_id = preset_id("user.create.file-manager-trash");
        let mut create = custom_create(create_id.as_str(), "File manager cleanup");
        let NamedPreset::Create { options, .. } = &mut create else {
            panic!("custom preset should be a create preset");
        };
        options.format = format_id("7z");
        options.credential = CreateCredential::None;
        options.volumes = VolumeMode::Single;
        options.post_success = PostSuccessAction::TrashSource;
        options.test_after_create = true;
        let mut document = PresetDocument::seeded();
        document.presets.push(create);
        document.bindings.file_manager_create = Some(create_id);
        assert!(document.validate().is_err());

        let extract_id = preset_id("user.extract.file-manager-trash");
        let mut options = smart_extract_preset()
            .extract_options()
            .expect("smart extract fixture should contain extract options")
            .clone();
        options.post_success = PostSuccessAction::TrashSource;
        let extract = NamedPreset::Extract {
            id: extract_id.clone(),
            label: preset_label("Extract and clean up"),
            built_in: false,
            options,
        };
        let mut document = PresetDocument::seeded();
        document.presets.push(extract);
        document.bindings.file_manager_extract = Some(extract_id);
        assert!(document.validate().is_err());
    }

    #[test]
    fn file_manager_create_binding_rejects_in_app_completion() {
        let create_id = preset_id("user.create.file-manager-open");
        let mut create = custom_create(create_id.as_str(), "Open after file manager create");
        let NamedPreset::Create { options, .. } = &mut create else {
            panic!("custom preset should be a create preset");
        };
        options.format = format_id("7z");
        options.credential = CreateCredential::None;
        options.volumes = VolumeMode::Single;
        options.completion = CreateCompletionAction::OpenInSquallz;
        let mut document = PresetDocument::seeded();
        document.presets.push(create);
        document.bindings.file_manager_create = Some(create_id);

        assert!(document.validate().is_err());
    }

    #[test]
    fn store_persists_with_revision_compare_and_swap() {
        let path = temp_path("cas");
        let store = PresetStore::new(&path);
        let initial = store
            .load()
            .expect("missing preset file should use defaults");
        assert_eq!(initial.revision, 0);

        let mut replacement = initial.clone();
        replacement
            .presets
            .push(custom_create("user.create.portable", "Portable"));
        let saved = store
            .compare_and_swap(0, replacement)
            .expect("first update should persist");
        assert_eq!(saved.revision, 1);
        assert_eq!(store.load().expect("saved document should load"), saved);

        let stale = PresetDocument::seeded();
        assert!(matches!(
            store.compare_and_swap(0, stale),
            Err(PresetError::RevisionConflict {
                expected: 0,
                actual: 1
            })
        ));
        let _ = std::fs::remove_dir_all(path.parent().expect("test path should have parent"));
    }

    #[test]
    fn corrupt_file_is_preserved_byte_for_byte() {
        let path = temp_path("corrupt");
        std::fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create test directory");
        let corrupt = b"{ not valid json";
        std::fs::write(&path, corrupt).expect("write corrupt preset fixture");
        let store = PresetStore::new(&path);
        assert!(matches!(store.load(), Err(PresetError::Decode(_))));
        assert_eq!(std::fs::read(&path).expect("read corrupt fixture"), corrupt);
        let _ = std::fs::remove_dir_all(path.parent().expect("test path should have parent"));
    }

    #[test]
    fn oversized_file_is_rejected_without_rewriting_it() {
        let path = temp_path("oversized");
        std::fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create test directory");
        let oversized = vec![b' '; (MAX_PRESET_FILE_BYTES + 1) as usize];
        std::fs::write(&path, &oversized).expect("write oversized preset fixture");
        let store = PresetStore::new(&path);
        assert!(matches!(store.load(), Err(PresetError::Decode(_))));
        assert_eq!(
            std::fs::metadata(&path)
                .expect("read oversized fixture metadata")
                .len(),
            MAX_PRESET_FILE_BYTES + 1
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("test path should have parent"));
    }

    #[test]
    fn windows_replacement_recovers_a_lone_backup_before_cleanup() {
        let path = temp_path("backup-recovery");
        std::fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create test directory");
        let backup = sibling_artifact_path(&path, "backup", None);
        std::fs::write(&backup, b"last-good-copy").expect("write backup fixture");

        prepare_windows_replacement(&path, &backup).expect("recover lone backup");

        assert_eq!(
            std::fs::read(&path).expect("read recovered canonical file"),
            b"last-good-copy"
        );
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(path.parent().expect("test path should have parent"));
    }

    #[test]
    fn extract_policy_schema_round_trips_all_job_fields() {
        let preset = NamedPreset::Extract {
            id: preset_id("user.extract.review"),
            label: preset_label("Review conflicts"),
            built_in: false,
            options: ExtractPreset {
                destination: ExtractDestination {
                    base: ExtractDestinationBase::Ask,
                    layout: ExtractLayout::Direct,
                },
                existing_output: OverwritePolicy::RenameBoth,
                symlinks: SymlinkPolicy::Skip,
                encoding: super::EntryNameEncoding::Named {
                    label: "gb18030".to_owned(),
                },
                credential: ExtractCredential::PromptWhenNeeded,
                post_success: PostSuccessAction::KeepSource,
            },
        };
        let mut document = PresetDocument::seeded();
        document.presets.push(preset);
        let bytes = serde_json::to_vec(&document).expect("serialize extract preset");
        let decoded = decode_document(&bytes).expect("decode extract preset");
        assert_eq!(decoded, document);
        assert_eq!(decoded.schema_version, PRESET_SCHEMA_VERSION);
    }

    #[test]
    fn current_extract_presets_reject_unimplemented_source_cleanup() {
        let mut options = smart_extract_preset()
            .extract_options()
            .expect("built-in extract preset should contain extract options")
            .clone();
        options.post_success = PostSuccessAction::TrashSource;

        let error = validate_extract_preset(&options)
            .expect_err("extract source cleanup must not be accepted silently");

        assert!(error
            .to_string()
            .contains("cannot remove the source archive"));
    }
}
