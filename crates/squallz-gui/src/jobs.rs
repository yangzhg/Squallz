//! Job execution: every compress/extract/test/convert runs through the core
//! [`JobQueue`]. Progress is forwarded as throttled `job://progress` events,
//! state changes as `job://state`; mid-job questions (conflicts, passwords)
//! park the worker on the [`AskBridge`] until the frontend answers.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use squallz_core::api::{
    ArchiveSourceSet, ArchiveStructureStatus, BoundedProblemLog, CompressionLevel,
    ConflictDecision, ConflictResolver, ControlToken, CreateOptions, Detected, EntryMeta,
    EntryPath, ExtractProblemReporter, ExtractReport, FormatError, OpenOptions, OverwritePolicy,
    Password, ProblemPreview, ProgressPhase, ProgressSink, RecoverySummary, SymlinkPolicy,
    UpdateOp,
};
use squallz_core::{
    create_destination_has_conflict, is_plain_sqz_path, is_sqz_archive_path, is_zip_family_path,
    sync_directory, validate_sfx_template, CreateArtifactKind, CreateCommitPolicy,
    CreateDestinationGuard, CreateInputManifestEntry, CreateInputModifiedTime, CreateReport,
    Engine, ExtractInputGuard, ExtractPlan, JobId, JobQueue, JobResources, JobState,
    PostSuccessAction, QueueWaitReason, SfxBuildOptions, SfxBuildReport, SfxTarget,
};
use squallz_publish::{publish_macos_sfx, MacosSfxPublishPhase};

use crate::audit::{self, OperationAudit, OperationAuditRecord};
use crate::bridge::{AskAnswer, AskBridge};
use crate::dto::{
    AskConflictEvent, AskPasswordEvent, BatchExtractItem, ErrorDto, ExtractPlanDto, JobSpec,
    ProgressEvent, SettingsDto, SfxCreateCapabilityDto, StateEvent,
};
use crate::events::{emit, EventSink, EV_ASK_CONFLICT, EV_ASK_PASSWORD, EV_PROGRESS, EV_STATE};
use crate::nested::{create_nested_job_workspace, extract_nested_archive_to_temp_for_job};
use crate::source_cleanup_journal::{
    remove_empty_holder_if_identity, source_path_identity, PendingSourceCleanup,
    SourceCleanupJournal, SourceCleanupRecord, SourceCleanupRecovery, SourcePathIdentity,
    HOLDER_PREFIX,
};
use crate::state::{AppState, ResolvedArchiveSource};
use squallz_core::lock_unpoisoned;

/// Minimum interval between two progress events.
const PROGRESS_THROTTLE_MS: u128 = 60;
const MAX_TERMINAL_SNAPSHOTS: usize = 100;
const MAX_SNAPSHOT_CHANGES: usize = 512;
pub(crate) const MAX_PARALLEL_JOBS: usize = 8;
const MAX_AUTOMATIC_PARALLEL_JOBS: usize = 4;
static TRASH_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct QueueConfig {
    worker_threads: usize,
    max_running: usize,
    cpu_thread_budget: usize,
}

impl QueueConfig {
    fn sequential() -> Self {
        Self {
            worker_threads: 1,
            max_running: 1,
            cpu_thread_budget: 1,
        }
    }

    fn from_settings(settings: &SettingsDto) -> Self {
        let cpu_thread_budget = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);
        Self {
            worker_threads: MAX_PARALLEL_JOBS,
            max_running: resolved_parallel_job_limit(
                settings.performance_parallel_jobs,
                cpu_thread_budget,
            ),
            cpu_thread_budget,
        }
    }
}

fn resolved_parallel_job_limit(configured: Option<usize>, cpu_threads: usize) -> usize {
    configured.map_or_else(
        || {
            cpu_threads
                .div_ceil(4)
                .clamp(1, MAX_AUTOMATIC_PARALLEL_JOBS)
        },
        |value| value.clamp(1, MAX_PARALLEL_JOBS),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CpuReservationProfile {
    SingleThread,
    ConfigurableEncoder,
    HostParallel,
}

fn create_cpu_reservation_profile(engine: &Engine, destination: &str) -> CpuReservationProfile {
    match engine.registry().detect_by_name(destination) {
        Some(Detected::Archive(format)) if format.id() == "wim" => {
            CpuReservationProfile::ConfigurableEncoder
        }
        Some(Detected::Compressed { compressor, .. }) if compressor.id() == "zstd" => {
            CpuReservationProfile::ConfigurableEncoder
        }
        _ => CpuReservationProfile::SingleThread,
    }
}

fn scheduler_cpu_profile(engine: &Engine, spec: &JobSpec) -> CpuReservationProfile {
    match spec {
        JobSpec::Compress { dest, .. } => create_cpu_reservation_profile(engine, dest),
        JobSpec::Convert { dest, .. } | JobSpec::ExportSqz { dest, .. } => {
            create_cpu_reservation_profile(engine, dest)
        }
        JobSpec::Protect { .. }
        | JobSpec::VerifyRecovery { .. }
        | JobSpec::RepairRecovery { .. } => CpuReservationProfile::HostParallel,
        JobSpec::PublishMacosSfx { .. }
        | JobSpec::Extract { .. }
        | JobSpec::BatchExtract { .. }
        | JobSpec::ExtractNested { .. }
        | JobSpec::Test { .. }
        | JobSpec::RepairSqz { .. }
        | JobSpec::RepairZip { .. }
        | JobSpec::Update { .. }
        | JobSpec::Checksum { .. }
        | JobSpec::ChecksumCheck { .. }
        | JobSpec::DuplicateScan { .. } => CpuReservationProfile::SingleThread,
    }
}

fn scheduler_resources(
    profile: CpuReservationProfile,
    settings: &SettingsDto,
    cpu_thread_budget: usize,
) -> JobResources {
    let cpu_thread_budget = cpu_thread_budget.max(1);
    let cpu_threads = match profile {
        CpuReservationProfile::SingleThread => 1,
        CpuReservationProfile::ConfigurableEncoder => settings
            .resource_options()
            .threads
            .unwrap_or(cpu_thread_budget.min(64)),
        CpuReservationProfile::HostParallel => cpu_thread_budget,
    };
    JobResources::new(cpu_threads.min(cpu_thread_budget))
}

fn settings_for_job_execution(
    mut settings: SettingsDto,
    profile: CpuReservationProfile,
    resources: JobResources,
) -> SettingsDto {
    if profile == CpuReservationProfile::ConfigurableEncoder {
        settings.performance_threads = Some(resources.cpu_threads);
    }
    settings
}

fn job_stream_buffer_limit_bytes(spec: &JobSpec, settings: &SettingsDto) -> Option<u64> {
    match spec {
        JobSpec::Compress { .. }
        | JobSpec::Extract { .. }
        | JobSpec::PublishMacosSfx { .. }
        | JobSpec::BatchExtract { .. }
        | JobSpec::ExtractNested { .. }
        | JobSpec::Convert { .. }
        | JobSpec::ExportSqz { .. }
        | JobSpec::RepairSqz { .. }
        | JobSpec::RepairZip { .. }
        | JobSpec::Update { .. } => settings.resource_options().memory_limit,
        JobSpec::Test { .. }
        | JobSpec::Checksum { .. }
        | JobSpec::ChecksumCheck { .. }
        | JobSpec::DuplicateScan { .. }
        | JobSpec::Protect { .. }
        | JobSpec::VerifyRecovery { .. }
        | JobSpec::RepairRecovery { .. } => None,
    }
}

fn job_supports_pause(spec: &JobSpec) -> bool {
    !matches!(
        spec,
        JobSpec::PublishMacosSfx { .. }
            | JobSpec::Protect { .. }
            | JobSpec::VerifyRecovery { .. }
            | JobSpec::RepairRecovery { .. }
    )
}

fn metadata_len_or_zero(meta: Option<&fs::Metadata>) -> u64 {
    match meta {
        Some(meta) => meta.len(),
        None => 0,
    }
}

fn path_stem_or_empty(path: &Path) -> String {
    match path.file_stem() {
        Some(stem) => stem.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn path_parent_or_empty(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) => parent,
        None => Path::new(""),
    }
}

fn path_file_name_or_empty(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn batch_archive_label(path: &Path) -> String {
    let name = path_file_name_or_empty(path);
    if name.is_empty() {
        path.to_string_lossy().into_owned()
    } else {
        name
    }
}

struct BatchExtractWorkItem {
    execution: BatchExtractItem,
    display: BatchExtractItem,
}

fn batch_extract_work_item(
    items: &[BatchExtractItem],
    display_items: &[BatchExtractItem],
    index: usize,
) -> BatchExtractWorkItem {
    let execution = items[index].clone();
    let display = display_items
        .get(index)
        .cloned()
        .unwrap_or_else(|| execution.clone());
    BatchExtractWorkItem { execution, display }
}

fn normalize_batch_extract_items_with(
    items: &[BatchExtractItem],
    display_items: &[BatchExtractItem],
    mut source_set_for: impl FnMut(&Path) -> Result<Option<ArchiveSourceSet>, FormatError>,
) -> Vec<BatchExtractWorkItem> {
    let mut indices_by_path: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        indices_by_path
            .entry(PathBuf::from(&item.path))
            .or_default()
            .push(index);
    }

    let mut consumed = vec![false; items.len()];
    let mut normalized = Vec::with_capacity(items.len());
    for index in 0..items.len() {
        if consumed[index] {
            continue;
        }
        let source_set = match source_set_for(Path::new(&items[index].path)) {
            Ok(Some(source_set)) if source_set.members().len() > 1 => source_set,
            // Discovery is only a grouping gate. The ordinary extraction path
            // remains authoritative and reports format or I/O failures.
            Ok(_) | Err(_) => {
                normalized.push(batch_extract_work_item(items, display_items, index));
                continue;
            }
        };

        let mut family_indices = Vec::new();
        let mut includes_current = false;
        let mut primary_index = None;
        for member in source_set.members() {
            let Some(member_indices) = indices_by_path.get(member) else {
                continue;
            };
            for &member_index in member_indices {
                if consumed[member_index] {
                    continue;
                }
                includes_current |= member_index == index;
                if member == source_set.primary() && primary_index.is_none() {
                    primary_index = Some(member_index);
                }
                family_indices.push(member_index);
            }
        }
        if !includes_current || family_indices.len() < 2 {
            normalized.push(batch_extract_work_item(items, display_items, index));
            continue;
        }

        let representative = primary_index.unwrap_or(family_indices[0]);
        for &member_index in &family_indices {
            consumed[member_index] = true;
        }
        normalized.push(batch_extract_work_item(
            items,
            display_items,
            representative,
        ));
    }
    normalized
}

fn status_code_label(status_code: Option<i32>) -> String {
    match status_code {
        Some(code) => code.to_string(),
        None => "unknown".into(),
    }
}

#[derive(Debug)]
struct TrashError;

trait TrashAdapter: Send + Sync {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError>;
}

struct SystemTrashAdapter;

impl TrashAdapter for SystemTrashAdapter {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| trash::delete(path))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(TrashError),
        }
    }
}

#[derive(Clone)]
struct ManagedJob {
    queue_id: JobId,
    cancel_flag: Arc<AtomicBool>,
    owner_window: Option<String>,
    events: Arc<dyn EventSink>,
    pausable: bool,
}

struct PreparedJob {
    execution_spec: JobSpec,
    snapshot_spec: JobSpec,
    _source_leases: Vec<ResolvedArchiveSource>,
    redactions: Vec<(String, String)>,
}

impl PreparedJob {
    fn new(
        state: &AppState,
        owner_window: Option<&str>,
        spec: &JobSpec,
    ) -> Result<Self, FormatError> {
        let mut execution_spec = spec.clone();
        let mut snapshot_spec = spec.clone();
        let mut source_leases = Vec::new();
        let mut redactions = Vec::new();
        match (&mut execution_spec, &mut snapshot_spec) {
            (JobSpec::Extract { path, .. }, JobSpec::Extract { path: shown, .. })
            | (JobSpec::Test { path, .. }, JobSpec::Test { path: shown, .. }) => {
                resolve_job_source(
                    state,
                    owner_window,
                    path,
                    shown,
                    true,
                    &mut source_leases,
                    &mut redactions,
                )?;
            }
            (
                JobSpec::BatchExtract { items, .. },
                JobSpec::BatchExtract {
                    items: shown_items, ..
                },
            ) => {
                for (item, shown) in items.iter_mut().zip(shown_items) {
                    resolve_job_source(
                        state,
                        owner_window,
                        &mut item.path,
                        &mut shown.path,
                        true,
                        &mut source_leases,
                        &mut redactions,
                    )?;
                }
            }
            (
                JobSpec::ExtractNested { outer_path, .. },
                JobSpec::ExtractNested {
                    outer_path: shown, ..
                },
            ) => {
                resolve_job_source(
                    state,
                    owner_window,
                    outer_path,
                    shown,
                    true,
                    &mut source_leases,
                    &mut redactions,
                )?;
            }
            (JobSpec::Convert { src, .. }, JobSpec::Convert { src: shown, .. })
            | (JobSpec::ExportSqz { src, .. }, JobSpec::ExportSqz { src: shown, .. })
            | (JobSpec::RepairSqz { src, .. }, JobSpec::RepairSqz { src: shown, .. })
            | (JobSpec::RepairZip { src, .. }, JobSpec::RepairZip { src: shown, .. }) => {
                resolve_job_source(
                    state,
                    owner_window,
                    src,
                    shown,
                    true,
                    &mut source_leases,
                    &mut redactions,
                )?;
            }
            (JobSpec::Update { path, .. }, JobSpec::Update { path: shown, .. }) => {
                resolve_job_source(
                    state,
                    owner_window,
                    path,
                    shown,
                    false,
                    &mut source_leases,
                    &mut redactions,
                )?;
            }
            (JobSpec::Protect { path, .. }, JobSpec::Protect { path: shown, .. })
            | (JobSpec::VerifyRecovery { path, .. }, JobSpec::VerifyRecovery { path: shown, .. })
            | (JobSpec::RepairRecovery { path, .. }, JobSpec::RepairRecovery { path: shown, .. }) =>
            {
                resolve_job_source(
                    state,
                    owner_window,
                    path,
                    shown,
                    false,
                    &mut source_leases,
                    &mut redactions,
                )?;
            }
            _ => {}
        }
        snapshot_spec = snapshot_spec.redacted_for_snapshot();
        Ok(Self {
            execution_spec,
            snapshot_spec,
            _source_leases: source_leases,
            redactions,
        })
    }
}

fn resolve_job_source(
    state: &AppState,
    owner_window: Option<&str>,
    source: &mut String,
    shown_source: &mut String,
    allow_read_only: bool,
    leases: &mut Vec<ResolvedArchiveSource>,
    redactions: &mut Vec<(String, String)>,
) -> Result<(), FormatError> {
    let resolved = state.resolve_archive_source(source, owner_window)?;
    if resolved.is_read_only() && !allow_read_only {
        return Err(FormatError::Unsupported(
            "nested archives are read-only; extract or convert them to save changes".to_owned(),
        ));
    }
    let physical = resolved.path().to_string_lossy().into_owned();
    let display = resolved.display_path().to_owned();
    shown_source.clone_from(&display);
    source.clear();
    source.push_str(&physical);
    if resolved.is_read_only() && physical != display {
        redactions.push((physical, display));
    }
    leases.push(resolved);
    Ok(())
}

fn redact_source_text(value: &str, redactions: &[(String, String)]) -> String {
    redactions
        .iter()
        .fold(value.to_owned(), |text, (path, display)| {
            text.replace(path, display)
        })
}

fn redact_source_error(error: &mut ErrorDto, redactions: &[(String, String)]) {
    error.detail = redact_source_text(&error.detail, redactions);
    for value in error.params.values_mut() {
        *value = redact_source_text(value, redactions);
    }
}

fn redact_source_json(value: &mut serde_json::Value, redactions: &[(String, String)]) {
    match value {
        serde_json::Value::String(text) => {
            *text = redact_source_text(text, redactions);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_source_json(item, redactions);
            }
        }
        serde_json::Value::Object(fields) => {
            for item in fields.values_mut() {
                redact_source_json(item, redactions);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn redact_format_error_path(error: FormatError, path: &str, display: &str) -> FormatError {
    if path.is_empty() || !error.to_string().contains(path) {
        return error;
    }
    let replace = |detail: String| detail.replace(path, display);
    if let Some(output) = error.destination_changed_path() {
        return FormatError::destination_changed(PathBuf::from(replace(
            output.display().to_string(),
        )));
    }
    if let Some(output) = error.output_exists_path() {
        return FormatError::output_exists(PathBuf::from(replace(output.display().to_string())));
    }
    match error {
        FormatError::Io(error) => {
            FormatError::from(io::Error::new(error.kind(), replace(error.to_string())))
        }
        FormatError::Unsupported(detail) => FormatError::Unsupported(replace(detail)),
        FormatError::CorruptArchive(detail) => FormatError::CorruptArchive(replace(detail)),
        FormatError::PasswordRequired => FormatError::PasswordRequired,
        FormatError::WrongPassword => FormatError::WrongPassword,
        FormatError::Cancelled => FormatError::Cancelled,
        FormatError::PathTraversal(detail) => FormatError::PathTraversal(replace(detail)),
        FormatError::SymlinkBreakout(detail) => FormatError::SymlinkBreakout(replace(detail)),
        FormatError::ResourceLimitExceeded(detail) => {
            FormatError::ResourceLimitExceeded(replace(detail))
        }
        FormatError::UnsafeFileName(detail) => FormatError::UnsafeFileName(replace(detail)),
        FormatError::DiskFull => FormatError::DiskFull,
        FormatError::DependencyMissing(detail) => FormatError::DependencyMissing(replace(detail)),
        FormatError::Other(detail) => FormatError::Other(replace(detail)),
    }
}

#[derive(Default)]
struct JobRegistry {
    jobs: HashMap<u64, ManagedJob>,
    released_windows: HashSet<String>,
    shutting_down: bool,
}

/// GUI job manager: owns the queue and each job's submitting window.
pub struct JobManager {
    queue: JobQueue,
    cpu_thread_budget: usize,
    next_id: AtomicU64,
    audit: Arc<OperationAudit>,
    /// The extra cancel flag lets question waits observe cancellation even
    /// though the queue's own token is not shareable across that boundary.
    registry: Mutex<JobRegistry>,
    snapshots: Arc<Mutex<JobSnapshotStore>>,
    sfx_template: Option<PathBuf>,
    trash_adapter: Arc<dyn TrashAdapter>,
    source_cleanup_journal: Arc<SourceCleanupJournal>,
    source_cleanup_recovery: Arc<Mutex<SourceCleanupRecoveryState>>,
    /// Worker ↔ UI question bridge
    pub bridge: Arc<AskBridge>,
}

impl JobManager {
    /// Builds the manager with a single worker.
    pub fn new() -> Self {
        #[cfg(test)]
        {
            Self::with_audit(Arc::new(OperationAudit::memory()))
        }
        #[cfg(not(test))]
        {
            Self::with_audit(Arc::new(OperationAudit::load()))
        }
    }

    pub fn with_audit(audit: Arc<OperationAudit>) -> Self {
        Self::with_audit_and_template(audit, crate::sfx_runtime::discover_host_template())
    }

    pub(crate) fn with_audit_and_settings(
        audit: Arc<OperationAudit>,
        settings: &SettingsDto,
    ) -> Self {
        Self::with_dependencies_and_journal(
            audit,
            crate::sfx_runtime::discover_host_template(),
            Arc::new(SystemTrashAdapter),
            Arc::new(SourceCleanupJournal::load()),
            QueueConfig::from_settings(settings),
        )
    }

    fn with_audit_and_template(audit: Arc<OperationAudit>, sfx_template: Option<PathBuf>) -> Self {
        Self::with_dependencies(audit, sfx_template, Arc::new(SystemTrashAdapter))
    }

    fn with_dependencies(
        audit: Arc<OperationAudit>,
        sfx_template: Option<PathBuf>,
        trash_adapter: Arc<dyn TrashAdapter>,
    ) -> Self {
        Self::with_dependencies_and_journal(
            audit,
            sfx_template,
            trash_adapter,
            Arc::new(SourceCleanupJournal::load()),
            QueueConfig::sequential(),
        )
    }

    fn with_dependencies_and_journal(
        audit: Arc<OperationAudit>,
        sfx_template: Option<PathBuf>,
        trash_adapter: Arc<dyn TrashAdapter>,
        source_cleanup_journal: Arc<SourceCleanupJournal>,
        queue_config: QueueConfig,
    ) -> Self {
        let mut source_cleanup_recovery = SourceCleanupRecoveryState::default();
        source_cleanup_recovery.publish_new(source_cleanup_recovery_notice(
            source_cleanup_journal.recover_pending(),
            source_cleanup_journal.recovery_record_path(),
        ));
        Self {
            queue: JobQueue::with_resource_limits(
                queue_config.worker_threads,
                queue_config.max_running,
                queue_config.cpu_thread_budget,
            ),
            cpu_thread_budget: queue_config.cpu_thread_budget,
            next_id: AtomicU64::new(1),
            audit,
            registry: Mutex::new(JobRegistry::default()),
            snapshots: Arc::new(Mutex::new(JobSnapshotStore::default())),
            sfx_template,
            trash_adapter,
            source_cleanup_journal,
            source_cleanup_recovery: Arc::new(Mutex::new(source_cleanup_recovery)),
            bridge: Arc::new(AskBridge::default()),
        }
    }

    #[cfg(test)]
    fn with_test_sfx_template(audit: Arc<OperationAudit>, template: PathBuf) -> Self {
        Self::with_audit_and_template(audit, Some(template))
    }

    #[cfg(test)]
    fn with_test_trash_adapter(
        audit: Arc<OperationAudit>,
        trash_adapter: Arc<dyn TrashAdapter>,
    ) -> Self {
        Self::with_dependencies(audit, None, trash_adapter)
    }

    #[cfg(test)]
    fn with_test_trash_adapter_and_journal(
        audit: Arc<OperationAudit>,
        trash_adapter: Arc<dyn TrashAdapter>,
        source_cleanup_journal: Arc<SourceCleanupJournal>,
    ) -> Self {
        Self::with_dependencies_and_journal(
            audit,
            None,
            trash_adapter,
            source_cleanup_journal,
            QueueConfig::sequential(),
        )
    }

    /// Submits a job; events for its whole life cycle carry the returned id.
    #[cfg(test)]
    pub fn submit(
        &self,
        state: Arc<AppState>,
        events: Arc<dyn EventSink>,
        spec: JobSpec,
        settings: SettingsDto,
    ) -> u64 {
        match self.submit_with_owner(None, state, events, spec, settings) {
            Ok(id) => id,
            Err(error) => panic!("test job source should be valid: {error}"),
        }
    }

    #[cfg(test)]
    fn submit_for_test_window(
        &self,
        owner_window: String,
        state: Arc<AppState>,
        events: Arc<dyn EventSink>,
        spec: JobSpec,
        settings: SettingsDto,
    ) -> u64 {
        match self.submit_for_window(owner_window, state, events, spec, settings) {
            Ok(id) => id,
            Err(error) => panic!("test job source should be valid: {error}"),
        }
    }

    /// Submits a job owned by the native window that invoked the command.
    pub fn submit_for_window(
        &self,
        owner_window: String,
        state: Arc<AppState>,
        events: Arc<dyn EventSink>,
        spec: JobSpec,
        settings: SettingsDto,
    ) -> Result<u64, FormatError> {
        self.submit_with_owner(Some(owner_window), state, events, spec, settings)
    }

    fn submit_with_owner(
        &self,
        owner_window: Option<String>,
        state: Arc<AppState>,
        events: Arc<dyn EventSink>,
        spec: JobSpec,
        settings: SettingsDto,
    ) -> Result<u64, FormatError> {
        let prepared = PreparedJob::new(&state, owner_window.as_deref(), &spec)?;
        let spec = prepared.snapshot_spec;
        self.cleanup_terminal_queue_slots();
        let gui_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let bridge = Arc::clone(&self.bridge);
        let audit = Arc::clone(&self.audit);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel_flag);
        let snapshots = Arc::clone(&self.snapshots);
        let sfx_template = self.sfx_template.clone();
        let trash_adapter = Arc::clone(&self.trash_adapter);
        let source_cleanup_journal = Arc::clone(&self.source_cleanup_journal);
        let source_cleanup_recovery = Arc::clone(&self.source_cleanup_recovery);
        let cpu_profile = scheduler_cpu_profile(&state.engine, &spec);
        let job_resources = scheduler_resources(cpu_profile, &settings, self.cpu_thread_budget);
        let stream_buffer_limit_bytes = job_stream_buffer_limit_bytes(&spec, &settings);
        let settings = settings_for_job_execution(settings, cpu_profile, job_resources);
        let pausable = job_supports_pause(&spec);
        let mut registry = lock_unpoisoned(&self.registry);
        if registry.shutting_down
            || owner_window
                .as_ref()
                .is_some_and(|owner| registry.released_windows.contains(owner))
        {
            lock_unpoisoned(&self.snapshots).insert_with_resources(
                gui_id,
                owner_window,
                spec.redacted_for_snapshot(),
                "cancelled",
                job_resources,
                stream_buffer_limit_bytes,
            );
            return Ok(gui_id);
        }
        let queued_version = lock_unpoisoned(&self.snapshots).insert_with_resources(
            gui_id,
            owner_window.clone(),
            spec.redacted_for_snapshot(),
            "queued",
            job_resources,
            stream_buffer_limit_bytes,
        );
        emit_state(&*events, gui_id, queued_version, "queued", None);
        let owner_events = Arc::clone(&events);
        let execution_spec = prepared.execution_spec;
        let source_leases = prepared._source_leases;
        let redactions = prepared.redactions;
        let queue_id = self.queue.submit_with_resources(
            Box::new(move |ctl, queue_sink| {
                let _source_leases = source_leases;
                let starting_state = if ctl.is_paused() { "paused" } else { "running" };
                if let Some(version) =
                    lock_unpoisoned(&snapshots).set_starting_state(gui_id, starting_state)
                {
                    emit_state(&*events, gui_id, version, starting_state, None);
                }
                let sink = EmitProgress::new(
                    gui_id,
                    Arc::clone(&events),
                    Arc::clone(&snapshots),
                    queue_sink,
                    &redactions,
                );
                let outcome = run_job(
                    &execution_spec,
                    &spec,
                    gui_id,
                    &state,
                    &settings,
                    &bridge,
                    &events,
                    ctl,
                    &flag,
                    &sink,
                    &snapshots,
                    sfx_template.as_deref(),
                    &*trash_adapter,
                    &source_cleanup_journal,
                    &source_cleanup_recovery,
                );
                sink.flush();
                match outcome {
                    Ok(mut result) => {
                        if let Some(result) = result.as_mut() {
                            redact_source_json(result, &redactions);
                        }
                        record_job_audit(&audit, gui_id, &spec, "done", result.as_ref(), None);
                        if let Some(version) = lock_unpoisoned(&snapshots).set_state(
                            gui_id,
                            "done",
                            None,
                            result.clone(),
                        ) {
                            emit(
                                &*events,
                                EV_STATE,
                                &StateEventWithResult {
                                    id: gui_id,
                                    version,
                                    state: "done",
                                    error: None,
                                    result,
                                },
                            );
                        }
                        Ok(())
                    }
                    Err(FormatError::Cancelled) => {
                        record_job_audit(&audit, gui_id, &spec, "cancelled", None, None);
                        if let Some(version) =
                            lock_unpoisoned(&snapshots).set_state(gui_id, "cancelled", None, None)
                        {
                            emit_state(&*events, gui_id, version, "cancelled", None);
                        }
                        Err(FormatError::Cancelled)
                    }
                    Err(e) => {
                        let mut error = ErrorDto::from_engine(&e);
                        redact_source_error(&mut error, &redactions);
                        let queue_error = FormatError::Other(format!("job failed: {}", error.key));
                        record_job_audit(
                            &audit,
                            gui_id,
                            &spec,
                            "failed",
                            None,
                            Some(error.key.clone()),
                        );
                        if let Some(version) = lock_unpoisoned(&snapshots).set_state(
                            gui_id,
                            "failed",
                            Some(error.clone()),
                            None,
                        ) {
                            emit_state(&*events, gui_id, version, "failed", Some(error));
                        }
                        Err(queue_error)
                    }
                }
            }),
            job_resources,
        );
        registry.jobs.insert(
            gui_id,
            ManagedJob {
                queue_id,
                cancel_flag,
                owner_window,
                events: owner_events,
                pausable,
            },
        );
        drop(registry);
        self.sync_queue_positions();
        Ok(gui_id)
    }

    /// Pauses a job (takes effect at the next chunk boundary).
    pub fn pause_for_window(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        let entry = self.managed_job_for_control(requester, gui_id)?;
        if !entry.pausable {
            return Err(job_unavailable_error());
        }
        let before = self
            .queue
            .state(entry.queue_id)
            .ok_or_else(job_unavailable_error)?;
        if !matches!(before, JobState::Queued | JobState::Running) {
            return Err(job_unavailable_error());
        }
        if !self.queue.try_pause(entry.queue_id) {
            return Err(job_unavailable_error());
        }
        let after = self.queue.state(entry.queue_id);
        if after != Some(JobState::Paused)
            && !(before == JobState::Queued && after == Some(JobState::Queued))
        {
            return Err(job_unavailable_error());
        }
        let Some(version) =
            lock_unpoisoned(&self.snapshots).set_state(gui_id, "paused", None, None)
        else {
            return Err(job_unavailable_error());
        };
        emit_state(&*entry.events, gui_id, version, "paused", None);
        Ok(())
    }

    /// Resumes a paused job.
    pub fn resume_for_window(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        let entry = self.managed_job_for_control(requester, gui_id)?;
        let snapshot_paused = lock_unpoisoned(&self.snapshots)
            .snapshot(requester, gui_id)
            .is_some_and(|snapshot| snapshot.state == "paused");
        if !snapshot_paused {
            return Err(job_unavailable_error());
        }
        self.queue.resume(entry.queue_id);
        let resumed_state = match self.queue.state(entry.queue_id) {
            Some(JobState::Queued) => "queued",
            Some(JobState::Running) => "running",
            _ => return Err(job_unavailable_error()),
        };
        let Some(version) =
            lock_unpoisoned(&self.snapshots).set_state(gui_id, resumed_state, None, None)
        else {
            return Err(job_unavailable_error());
        };
        emit_state(&*entry.events, gui_id, version, resumed_state, None);
        if resumed_state == "queued" {
            self.sync_queue_positions();
        }
        Ok(())
    }

    pub fn move_earlier_for_window(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        self.move_queued_for_window(requester, gui_id, QueueMove::Earlier)
    }

    pub fn move_later_for_window(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        self.move_queued_for_window(requester, gui_id, QueueMove::Later)
    }

    pub fn move_before_for_window(
        &self,
        requester: &str,
        gui_id: u64,
        before_gui_id: Option<u64>,
    ) -> Result<(), ErrorDto> {
        let entry = self.managed_job_for_control(requester, gui_id)?;
        let before_entry = before_gui_id
            .map(|before_id| self.managed_job_for_control(requester, before_id))
            .transpose()?;
        let reorderable = {
            let snapshots = lock_unpoisoned(&self.snapshots);
            snapshots
                .snapshot(requester, gui_id)
                .is_some_and(|snapshot| snapshot.state == "queued")
                && before_gui_id.is_none_or(|before_id| {
                    snapshots
                        .snapshot(requester, before_id)
                        .is_some_and(|snapshot| snapshot.state == "queued")
                })
        };
        if !reorderable {
            return Err(job_unavailable_error());
        }
        if !self
            .queue
            .move_queued_before(entry.queue_id, before_entry.map(|entry| entry.queue_id))
        {
            return Err(job_unavailable_error());
        }
        self.sync_queue_positions();
        Ok(())
    }

    fn move_queued_for_window(
        &self,
        requester: &str,
        gui_id: u64,
        direction: QueueMove,
    ) -> Result<(), ErrorDto> {
        let entry = self.managed_job_for_control(requester, gui_id)?;
        let reorderable = lock_unpoisoned(&self.snapshots)
            .snapshot(requester, gui_id)
            .is_some_and(|snapshot| snapshot.state == "queued");
        if !reorderable {
            return Err(job_unavailable_error());
        }
        let moved = match direction {
            QueueMove::Earlier => self.queue.move_queued_earlier(entry.queue_id),
            QueueMove::Later => self.queue.move_queued_later(entry.queue_id),
        };
        if !moved {
            return Err(job_unavailable_error());
        }
        self.sync_queue_positions();
        Ok(())
    }

    fn sync_queue_positions(&self) {
        let queue_statuses = self.queue.queued_job_statuses();
        let gui_statuses = {
            let registry = lock_unpoisoned(&self.registry);
            let by_queue = registry
                .jobs
                .iter()
                .map(|(gui_id, entry)| (entry.queue_id, *gui_id))
                .collect::<HashMap<_, _>>();
            queue_statuses
                .iter()
                .filter_map(|status| {
                    by_queue.get(&status.id).copied().map(|gui_id| {
                        (
                            gui_id,
                            u64::try_from(status.position).unwrap_or(u64::MAX),
                            status.wait_reason,
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        lock_unpoisoned(&self.snapshots).sync_queue_statuses(&gui_statuses);
    }

    /// Cancels a job. A queued job is dropped immediately; a running one
    /// unwinds at its next checkpoint (open question dialogs are released
    /// through the per-job cancel flag) and reports `cancelled` itself.
    pub fn cancel_for_window(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        let entry = self.managed_job_for_control(requester, gui_id)?;
        if self.cancel_managed_job(gui_id, &entry) {
            Ok(())
        } else {
            Err(job_unavailable_error())
        }
    }

    pub fn answer_conflict_for_window(
        &self,
        requester: &str,
        gui_id: u64,
        decision: String,
        apply_all: bool,
    ) -> Result<(), ErrorDto> {
        self.ensure_exact_owner_interaction(requester, gui_id, JobInteraction::Conflict)?;
        if !self.bridge.answer(
            gui_id,
            AskAnswer::Conflict {
                decision,
                apply_all,
            },
        ) {
            return Err(job_unavailable_error());
        }
        Ok(())
    }

    pub fn answer_password_for_window(
        &self,
        requester: &str,
        gui_id: u64,
        password: Option<String>,
    ) -> Result<(), ErrorDto> {
        self.ensure_exact_owner_interaction(requester, gui_id, JobInteraction::Password)?;
        if !self.bridge.answer(gui_id, AskAnswer::Password(password)) {
            return Err(job_unavailable_error());
        }
        Ok(())
    }

    /// Releases a native window and cancels every non-terminal job it owns.
    /// Released labels are never accepted again, so a close racing with an
    /// in-flight IPC submission cannot leave an orphaned job behind.
    pub fn release_window(&self, window_label: &str) -> usize {
        let entries = {
            let mut registry = lock_unpoisoned(&self.registry);
            registry.released_windows.insert(window_label.to_owned());
            registry
                .jobs
                .iter()
                .filter(|(_, entry)| entry.owner_window.as_deref() == Some(window_label))
                .map(|(gui_id, entry)| (*gui_id, entry.clone()))
                .collect::<Vec<_>>()
        };
        entries
            .iter()
            .filter(|(gui_id, entry)| self.cancel_managed_job(*gui_id, entry))
            .count()
    }

    /// Cancels all unfinished jobs before application shutdown.
    pub fn cancel_all(&self) -> usize {
        let entries = {
            let mut registry = lock_unpoisoned(&self.registry);
            registry.shutting_down = true;
            registry
                .jobs
                .iter()
                .map(|(gui_id, entry)| (*gui_id, entry.clone()))
                .collect::<Vec<_>>()
        };
        entries
            .iter()
            .filter(|(gui_id, entry)| self.cancel_managed_job(*gui_id, entry))
            .count()
    }

    fn cancel_managed_job(&self, gui_id: u64, entry: &ManagedJob) -> bool {
        let Some(state) = self.queue.state(entry.queue_id) else {
            return false;
        };
        if state.is_terminal() {
            return false;
        }
        if entry.cancel_flag.swap(true, Ordering::Relaxed) {
            return false;
        }
        if !self.queue.try_cancel(entry.queue_id) {
            entry.cancel_flag.store(false, Ordering::Relaxed);
            return false;
        }
        self.bridge.wake_cancelled(gui_id);
        if self.queue.state(entry.queue_id) == Some(JobState::Cancelled) {
            if let Some(version) =
                lock_unpoisoned(&self.snapshots).set_state(gui_id, "cancelled", None, None)
            {
                emit_state(&*entry.events, gui_id, version, "cancelled", None);
            }
        }
        true
    }

    /// Blocks until all queued and running work has drained.
    pub fn wait_idle(&self) {
        self.queue.wait_idle();
        self.cleanup_terminal_queue_slots();
    }

    pub(crate) fn set_parallel_jobs(&self, configured: Option<usize>) {
        self.queue.set_max_running(resolved_parallel_job_limit(
            configured,
            self.cpu_thread_budget,
        ));
    }

    fn managed_job_for_control(
        &self,
        requester: &str,
        gui_id: u64,
    ) -> Result<ManagedJob, ErrorDto> {
        let entry = lock_unpoisoned(&self.registry).jobs.get(&gui_id).cloned();
        match entry {
            Some(entry)
                if requester == "main" || entry.owner_window.as_deref() == Some(requester) =>
            {
                Ok(entry)
            }
            _ => Err(job_unavailable_error()),
        }
    }

    fn ensure_exact_owner(&self, requester: &str, gui_id: u64) -> Result<(), ErrorDto> {
        let owner = lock_unpoisoned(&self.registry)
            .jobs
            .get(&gui_id)
            .and_then(|entry| entry.owner_window.clone());
        if owner.as_deref() == Some(requester) {
            Ok(())
        } else {
            Err(job_unavailable_error())
        }
    }

    fn ensure_exact_owner_interaction(
        &self,
        requester: &str,
        gui_id: u64,
        interaction: JobInteraction,
    ) -> Result<(), ErrorDto> {
        self.ensure_exact_owner(requester, gui_id)?;
        let current = lock_unpoisoned(&self.snapshots).snapshot(requester, gui_id);
        if current.is_some_and(|snapshot| snapshot.interaction == Some(interaction)) {
            Ok(())
        } else {
            Err(job_unavailable_error())
        }
    }

    fn cleanup_terminal_queue_slots(&self) {
        let terminal = {
            let mut registry = lock_unpoisoned(&self.registry);
            let ids = registry
                .jobs
                .iter()
                .filter_map(|(gui_id, entry)| {
                    self.queue
                        .state(entry.queue_id)
                        .filter(JobState::is_terminal)
                        .map(|state| (*gui_id, state))
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|(gui_id, state)| {
                    registry
                        .jobs
                        .remove(&gui_id)
                        .map(|entry| (gui_id, entry, state))
                })
                .collect::<Vec<_>>()
        };
        for (gui_id, entry, state) in terminal {
            self.reconcile_core_terminal_snapshot(gui_id, &*entry.events, &state);
            self.queue.forget_terminal(entry.queue_id);
        }
    }

    fn reconcile_core_terminal_snapshot(
        &self,
        gui_id: u64,
        events: &dyn EventSink,
        state: &JobState,
    ) {
        let (state_name, error) = match state {
            JobState::Done => ("done", None),
            JobState::Cancelled => ("cancelled", None),
            JobState::Failed(detail) => ("failed", Some(ErrorDto::other(detail.clone()))),
            JobState::Queued | JobState::Running | JobState::Paused => return,
        };
        if let Some(version) =
            lock_unpoisoned(&self.snapshots).set_state(gui_id, state_name, error.clone(), None)
        {
            emit_state(events, gui_id, version, state_name, error);
        }
    }

    pub fn snapshot_for_window(
        &self,
        requester: &str,
        gui_id: u64,
    ) -> Result<JobStateSnapshot, ErrorDto> {
        self.cleanup_terminal_queue_slots();
        self.sync_queue_positions();
        lock_unpoisoned(&self.snapshots)
            .snapshot(requester, gui_id)
            .ok_or_else(job_unavailable_error)
    }

    pub fn snapshots_for_window(&self, requester: &str, since: Option<u64>) -> JobSnapshotDelta {
        self.cleanup_terminal_queue_slots();
        self.sync_queue_positions();
        lock_unpoisoned(&self.snapshots).delta(requester, since)
    }

    pub fn dismiss_snapshots_for_window(
        &self,
        requester: &str,
        ids: &[u64],
    ) -> Result<(), ErrorDto> {
        self.cleanup_terminal_queue_slots();
        lock_unpoisoned(&self.snapshots).dismiss(requester, ids)
    }

    #[cfg(test)]
    pub fn snapshot(&self, gui_id: u64) -> Option<JobStateSnapshot> {
        lock_unpoisoned(&self.snapshots).snapshot("main", gui_id)
    }

    pub fn source_cleanup_recovery(&self) -> Option<SourceCleanupRecoveryNotice> {
        let mut recovery = lock_unpoisoned(&self.source_cleanup_recovery);
        let retry_busy = recovery
            .notice
            .as_ref()
            .is_some_and(|notice| notice.status == "busy");
        if retry_busy {
            let notice = source_cleanup_recovery_notice(
                self.source_cleanup_journal.recover_pending(),
                self.source_cleanup_journal.recovery_record_path(),
            );
            recovery.refresh_if_changed(notice);
        }
        recovery.notice.clone()
    }

    pub fn sfx_capability(&self) -> SfxCreateCapabilityDto {
        let target = SfxTarget::host();
        let status = match self.sfx_template.as_deref() {
            None => "missing",
            Some(template) => {
                let options = SfxBuildOptions {
                    target,
                    ..SfxBuildOptions::default()
                };
                if validate_sfx_template(template, &options, &ControlToken::new()).is_ok() {
                    "available"
                } else {
                    "invalid"
                }
            }
        };
        SfxCreateCapabilityDto {
            target: target.as_str().to_owned(),
            extension: crate::sfx_runtime::output_extension(target).to_owned(),
            available: status == "available",
            status: status.to_owned(),
            requires_signing: true,
        }
    }

    pub(crate) fn sfx_template_path(&self) -> Option<PathBuf> {
        self.sfx_template.clone()
    }
}

impl Drop for JobManager {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourceCleanupRecoveryNotice {
    generation: u64,
    status: String,
    path: Option<String>,
    reason: Option<String>,
    journal_path: Option<String>,
}

fn source_cleanup_recovery_notice(
    recovery: io::Result<SourceCleanupRecovery>,
    journal_path: Option<&Path>,
) -> Option<SourceCleanupRecoveryNotice> {
    let notice = match recovery {
        Ok(SourceCleanupRecovery::None) => return None,
        Ok(SourceCleanupRecovery::Restored { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "restored".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Preserved { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "preserved".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Changed { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "changed".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::Cleared) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "cleared".to_owned(),
            path: None,
            reason: None,
            journal_path: None,
        },
        Ok(SourceCleanupRecovery::CompletedUnknown { path }) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "completed_unknown".to_owned(),
            path: Some(path.to_string_lossy().into_owned()),
            reason: None,
            journal_path: None,
        },
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "busy".to_owned(),
            path: None,
            reason: None,
            journal_path: None,
        },
        Err(error) => SourceCleanupRecoveryNotice {
            generation: 0,
            status: "needs_attention".to_owned(),
            path: None,
            reason: Some(source_cleanup_recovery_reason(error.kind()).to_owned()),
            journal_path: journal_path.map(|path| path.to_string_lossy().into_owned()),
        },
    };
    Some(notice)
}

fn source_cleanup_recovery_reason(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::InvalidData => "journal_invalid",
        io::ErrorKind::PermissionDenied => "journal_permission_denied",
        io::ErrorKind::NotFound => "journal_unavailable",
        _ => "recovery_failed",
    }
}

#[derive(Default)]
struct SourceCleanupRecoveryState {
    generation: u64,
    notice: Option<SourceCleanupRecoveryNotice>,
}

impl SourceCleanupRecoveryState {
    fn publish_new(&mut self, notice: Option<SourceCleanupRecoveryNotice>) {
        self.install(notice);
    }

    fn refresh_if_changed(&mut self, notice: Option<SourceCleanupRecoveryNotice>) {
        if same_source_cleanup_notice(self.notice.as_ref(), notice.as_ref()) {
            return;
        }
        self.install(notice);
    }

    fn install(&mut self, mut notice: Option<SourceCleanupRecoveryNotice>) {
        if let Some(notice) = &mut notice {
            self.generation = self.generation.saturating_add(1);
            notice.generation = self.generation;
        }
        self.notice = notice;
    }
}

fn same_source_cleanup_notice(
    left: Option<&SourceCleanupRecoveryNotice>,
    right: Option<&SourceCleanupRecoveryNotice>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.status == right.status
                && left.path == right.path
                && left.reason == right.reason
                && left.journal_path == right.journal_path
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOrigin {
    App,
    FileManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobInteraction {
    Conflict,
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct JobProgressSnapshot {
    pub done: u64,
    pub total: u64,
    pub current: String,
    pub current_done: u64,
    pub current_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_entries: Option<u64>,
    pub speed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub interruptible: bool,
}

impl Default for JobProgressSnapshot {
    fn default() -> Self {
        Self {
            done: 0,
            total: 0,
            current: String::new(),
            current_done: 0,
            current_total: 0,
            scanned_entries: None,
            speed: 0,
            phase: None,
            interruptible: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobStateSnapshot {
    pub id: u64,
    pub version: u64,
    pub spec: JobSpec,
    pub origin: JobOrigin,
    pub owned_by_requester: bool,
    pub state: String,
    pub queue_position: Option<u64>,
    pub queue_wait_reason: Option<QueueWaitReason>,
    pub cpu_threads: usize,
    pub stream_buffer_limit_bytes: Option<u64>,
    pub progress: JobProgressSnapshot,
    pub error: Option<ErrorDto>,
    pub result: Option<serde_json::Value>,
    pub interaction: Option<JobInteraction>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobSnapshotDelta {
    pub revision: u64,
    pub reset: bool,
    pub upserts: Vec<JobStateSnapshot>,
    pub removed: Vec<u64>,
}

#[derive(Clone)]
struct StoredJobSnapshot {
    id: u64,
    version: u64,
    spec: JobSpec,
    origin: JobOrigin,
    owner_window: Option<String>,
    state: String,
    queue_position: Option<u64>,
    queue_wait_reason: Option<QueueWaitReason>,
    cpu_threads: usize,
    stream_buffer_limit_bytes: Option<u64>,
    progress: JobProgressSnapshot,
    error: Option<ErrorDto>,
    result: Option<serde_json::Value>,
    interaction: Option<JobInteraction>,
    dismissed_by: HashSet<String>,
}

enum SnapshotRemovalAudience {
    OriginalViewers(Option<String>),
    Observer(String),
}

struct SnapshotRemoval {
    id: u64,
    version: u64,
    audience: SnapshotRemovalAudience,
}

#[derive(Default)]
struct JobSnapshotStore {
    revision: u64,
    jobs: BTreeMap<u64, StoredJobSnapshot>,
    terminal_order: VecDeque<u64>,
    changes: VecDeque<u64>,
    removals: VecDeque<SnapshotRemoval>,
}

#[derive(Clone, Copy)]
enum QueueMove {
    Earlier,
    Later,
}

impl JobSnapshotStore {
    fn next_revision(&mut self) -> u64 {
        self.revision = self.revision.saturating_add(1);
        self.changes.push_back(self.revision);
        while self.changes.len() > MAX_SNAPSHOT_CHANGES {
            self.changes.pop_front();
        }
        self.revision
    }

    #[cfg(test)]
    fn insert(&mut self, id: u64, owner_window: Option<String>, spec: JobSpec, state: &str) -> u64 {
        self.insert_with_resources(id, owner_window, spec, state, JobResources::default(), None)
    }

    fn insert_with_resources(
        &mut self,
        id: u64,
        owner_window: Option<String>,
        spec: JobSpec,
        state: &str,
        resources: JobResources,
        stream_buffer_limit_bytes: Option<u64>,
    ) -> u64 {
        let version = self.next_revision();
        let origin = if owner_window.as_deref() == Some("main") || owner_window.is_none() {
            JobOrigin::App
        } else {
            JobOrigin::FileManager
        };
        self.jobs.insert(
            id,
            StoredJobSnapshot {
                id,
                version,
                spec,
                origin,
                owner_window,
                state: state.to_owned(),
                queue_position: None,
                queue_wait_reason: None,
                cpu_threads: resources.cpu_threads,
                stream_buffer_limit_bytes,
                progress: JobProgressSnapshot::default(),
                error: None,
                result: None,
                interaction: None,
                dismissed_by: HashSet::new(),
            },
        );
        if is_terminal_snapshot_state(state) {
            self.terminal_order.push_back(id);
            self.prune_terminal();
        }
        version
    }

    fn set_state(
        &mut self,
        id: u64,
        state: &str,
        error: Option<ErrorDto>,
        result: Option<serde_json::Value>,
    ) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if is_terminal_snapshot_state(&current.state) || current.state == state {
            return None;
        }
        let leaving_queue = current.state == "queued" && state != "queued";
        let entering_terminal = is_terminal_snapshot_state(state);
        let version = self.next_revision();
        {
            let record = self.jobs.get_mut(&id)?;
            record.version = version;
            record.state = state.to_owned();
            record.error = error;
            record.result = result;
            if state != "queued" {
                record.queue_position = None;
                record.queue_wait_reason = None;
            }
        }
        if leaving_queue {
            self.normalize_queue_positions();
        }
        if entering_terminal {
            let record = self.jobs.get_mut(&id)?;
            record.interaction = None;
            self.terminal_order.push_back(id);
            self.prune_terminal();
        }
        Some(version)
    }

    fn set_starting_state(&mut self, id: u64, state: &str) -> Option<u64> {
        if self.jobs.get(&id)?.state != "queued" {
            return None;
        }
        self.set_state(id, state, None, None)
    }

    fn set_progress(&mut self, id: u64, progress: JobProgressSnapshot) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if is_terminal_snapshot_state(&current.state) || current.progress == progress {
            return None;
        }
        let version = self.next_revision();
        let record = self.jobs.get_mut(&id)?;
        record.version = version;
        record.progress = progress;
        Some(version)
    }

    fn set_interaction(&mut self, id: u64, interaction: Option<JobInteraction>) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if current.interaction == interaction
            || (is_terminal_snapshot_state(&current.state) && interaction.is_some())
        {
            return None;
        }
        let version = self.next_revision();
        let record = self.jobs.get_mut(&id)?;
        record.version = version;
        record.interaction = interaction;
        Some(version)
    }

    fn sync_queue_positions(&mut self, ordered: &[u64]) {
        let statuses = ordered
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, (index + 1) as u64, None))
            .collect::<Vec<_>>();
        self.sync_queue_statuses(&statuses);
    }

    fn sync_queue_statuses(&mut self, statuses: &[(u64, u64, Option<QueueWaitReason>)]) {
        let statuses = statuses
            .iter()
            .map(|(id, position, reason)| (*id, (*position, *reason)))
            .collect::<HashMap<_, _>>();
        let changes = self
            .jobs
            .iter()
            .filter_map(|(id, record)| {
                let next = (record.state == "queued")
                    .then(|| statuses.get(id).copied())
                    .flatten();
                let (position, reason) = next
                    .map(|(position, reason)| (Some(position), reason))
                    .unwrap_or((None, None));
                (record.queue_position != position || record.queue_wait_reason != reason)
                    .then_some((*id, position, reason))
            })
            .collect::<Vec<_>>();
        for (id, position, reason) in changes {
            let version = self.next_revision();
            if let Some(record) = self.jobs.get_mut(&id) {
                record.version = version;
                record.queue_position = position;
                record.queue_wait_reason = reason;
            }
        }
    }

    fn normalize_queue_positions(&mut self) {
        let mut ordered = self
            .jobs
            .values()
            .filter(|record| record.state == "queued")
            .map(|record| (record.queue_position.unwrap_or(u64::MAX), record.id))
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        let ids = ordered.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
        self.sync_queue_positions(&ids);
    }

    fn snapshot(&self, requester: &str, id: u64) -> Option<JobStateSnapshot> {
        self.jobs
            .get(&id)
            .filter(|record| stored_snapshot_visible_to(record, requester))
            .map(|record| snapshot_for_requester(record, requester))
    }

    fn delta(&self, requester: &str, since: Option<u64>) -> JobSnapshotDelta {
        let reset = match since {
            None => true,
            Some(revision) if revision > self.revision => true,
            Some(revision) => self
                .changes
                .front()
                .is_some_and(|first| revision < first.saturating_sub(1)),
        };
        let baseline = since.unwrap_or_default();
        let upserts = self
            .jobs
            .values()
            .filter(|record| stored_snapshot_visible_to(record, requester))
            .filter(|record| reset || record.version > baseline)
            .map(|record| snapshot_for_requester(record, requester))
            .collect();
        let removed = if reset {
            Vec::new()
        } else {
            let mut seen = HashSet::new();
            self.removals
                .iter()
                .filter(|removal| removal.version > baseline)
                .filter(|removal| removal_visible_to(removal, requester))
                .filter(|removal| seen.insert(removal.id))
                .map(|removal| removal.id)
                .collect()
        };
        JobSnapshotDelta {
            revision: self.revision,
            reset,
            upserts,
            removed,
        }
    }

    fn dismiss(&mut self, requester: &str, ids: &[u64]) -> Result<(), ErrorDto> {
        let mut seen = HashSet::new();
        let unique = ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        for id in &unique {
            let Some(record) = self.jobs.get(id) else {
                return Err(job_unavailable_error());
            };
            if !snapshot_in_original_scope(record.owner_window.as_deref(), requester)
                || !is_terminal_snapshot_state(&record.state)
            {
                return Err(job_unavailable_error());
            }
        }
        for id in unique {
            let newly_dismissed = self
                .jobs
                .get_mut(&id)
                .is_some_and(|record| record.dismissed_by.insert(requester.to_owned()));
            if newly_dismissed {
                let version = self.next_revision();
                self.removals.push_back(SnapshotRemoval {
                    id,
                    version,
                    audience: SnapshotRemovalAudience::Observer(requester.to_owned()),
                });
                self.trim_removals();
            }
        }
        Ok(())
    }

    fn prune_terminal(&mut self) {
        while self.terminal_order.len() > MAX_TERMINAL_SNAPSHOTS {
            if let Some(id) = self.terminal_order.pop_front() {
                self.remove_without_terminal_scan(id);
            }
        }
    }

    fn remove_without_terminal_scan(&mut self, id: u64) {
        let Some(record) = self.jobs.remove(&id) else {
            return;
        };
        let version = self.next_revision();
        self.removals.push_back(SnapshotRemoval {
            id,
            version,
            audience: SnapshotRemovalAudience::OriginalViewers(record.owner_window),
        });
        self.trim_removals();
    }

    fn trim_removals(&mut self) {
        while self.removals.len() > MAX_SNAPSHOT_CHANGES {
            self.removals.pop_front();
        }
    }
}

fn snapshot_in_original_scope(owner_window: Option<&str>, requester: &str) -> bool {
    requester == "main" || owner_window == Some(requester)
}

fn stored_snapshot_visible_to(record: &StoredJobSnapshot, requester: &str) -> bool {
    snapshot_in_original_scope(record.owner_window.as_deref(), requester)
        && !record.dismissed_by.contains(requester)
}

fn removal_visible_to(removal: &SnapshotRemoval, requester: &str) -> bool {
    match &removal.audience {
        SnapshotRemovalAudience::OriginalViewers(owner_window) => {
            snapshot_in_original_scope(owner_window.as_deref(), requester)
        }
        SnapshotRemovalAudience::Observer(observer) => observer == requester,
    }
}

fn snapshot_for_requester(record: &StoredJobSnapshot, requester: &str) -> JobStateSnapshot {
    JobStateSnapshot {
        id: record.id,
        version: record.version,
        spec: record.spec.clone(),
        origin: record.origin,
        owned_by_requester: record.owner_window.as_deref() == Some(requester),
        state: record.state.clone(),
        queue_position: record.queue_position,
        queue_wait_reason: record.queue_wait_reason,
        cpu_threads: record.cpu_threads,
        stream_buffer_limit_bytes: record.stream_buffer_limit_bytes,
        progress: record.progress.clone(),
        error: record.error.clone(),
        result: record.result.clone(),
        interaction: record.interaction,
    }
}

fn is_terminal_snapshot_state(state: &str) -> bool {
    matches!(state, "done" | "failed" | "cancelled")
}

fn job_unavailable_error() -> ErrorDto {
    ErrorDto::other("job is unavailable to this window")
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

/// `job://state` payload extended with an optional job result (e.g. the
/// test report counters).
#[derive(serde::Serialize)]
struct StateEventWithResult {
    id: u64,
    version: u64,
    state: &'static str,
    error: Option<ErrorDto>,
    result: Option<serde_json::Value>,
}

fn record_job_audit(
    audit: &OperationAudit,
    gui_id: u64,
    spec: &JobSpec,
    state: &str,
    result: Option<&serde_json::Value>,
    error_key: Option<String>,
) {
    let summary = audit::summarize_job(spec);
    let record = OperationAuditRecord {
        id: gui_id,
        time: audit::now_millis(),
        kind: summary.kind,
        state: state.to_owned(),
        title: summary.title,
        detail: summary.detail,
        result_summary: audit::summarize_result(result),
        error_key,
    };
    if let Err(e) = audit.append(record) {
        log::warn!("operation audit: cannot append job {gui_id}: {e}");
    }
}

fn emit_state(events: &dyn EventSink, id: u64, version: u64, state: &str, error: Option<ErrorDto>) {
    emit(
        events,
        EV_STATE,
        &StateEvent {
            id,
            version,
            state: state.to_owned(),
            error,
        },
    );
}

/// Progress sink that forwards to the queue snapshot and emits throttled
/// `job://progress` events with a derived speed.
struct EmitProgress<'a> {
    id: u64,
    events: Arc<dyn EventSink>,
    snapshots: Arc<Mutex<JobSnapshotStore>>,
    queue_sink: &'a dyn ProgressSink,
    redactions: &'a [(String, String)],
    inner: Mutex<ProgressWindow>,
}

struct ProgressWindow {
    last_emit: Instant,
    last_done: u64,
    speed: u64,
    latest: Option<ProgressSnapshot>,
    latest_current_file: Option<ProgressSnapshot>,
    phase: Option<String>,
    interruptible: bool,
}

#[derive(Clone)]
struct ProgressSnapshot {
    done: u64,
    total: u64,
    current: String,
    current_done: u64,
    current_total: u64,
    scanned_entries: Option<u64>,
    phase: Option<String>,
    interruptible: bool,
}

#[derive(serde::Serialize)]
struct JobProgressEvent {
    #[serde(flatten)]
    base: ProgressEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    interruptible: bool,
}

const BATCH_PROGRESS_SCALE: u64 = 1_000;

struct BatchProgressSink<'a> {
    inner: &'a dyn ProgressSink,
    total_archives: u64,
    state: Mutex<BatchProgressState>,
}

struct BatchProgressState {
    index: u64,
    archive: String,
}

impl<'a> BatchProgressSink<'a> {
    fn new(inner: &'a dyn ProgressSink, total_archives: usize) -> Self {
        Self {
            inner,
            total_archives: total_archives.max(1) as u64,
            state: Mutex::new(BatchProgressState {
                index: 0,
                archive: String::new(),
            }),
        }
    }

    fn start_archive(&self, index: usize, archive: String) {
        {
            let mut state = lock_unpoisoned(&self.state);
            state.index = index as u64;
            state.archive = archive.clone();
        }
        self.emit(index as u64 * BATCH_PROGRESS_SCALE, archive, 0, 0);
    }

    fn finish_archive(&self, index: usize, archive: String) {
        self.emit(((index as u64) + 1) * BATCH_PROGRESS_SCALE, archive, 0, 0);
    }

    fn emit(&self, done: u64, current: String, current_done: u64, current_total: u64) {
        let total = self.total_archives.saturating_mul(BATCH_PROGRESS_SCALE);
        self.inner.on_entry_progress(
            done.min(total),
            total,
            &EntryPath::from_utf8(current),
            current_done,
            current_total,
        );
    }
}

impl ProgressSink for BatchProgressSink<'_> {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.on_entry_progress(done, total, current, 0, 0);
    }

    fn on_entry_progress(
        &self,
        done: u64,
        total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        let state = lock_unpoisoned(&self.state);
        let archive_done = if total > 0 {
            match done.saturating_mul(BATCH_PROGRESS_SCALE).checked_div(total) {
                Some(value) => value.min(BATCH_PROGRESS_SCALE),
                None => 0,
            }
        } else {
            0
        };
        let global_done = state
            .index
            .saturating_mul(BATCH_PROGRESS_SCALE)
            .saturating_add(archive_done);
        let current = if current.display.is_empty() {
            state.archive.clone()
        } else {
            format!("{}: {}", state.archive, current.display)
        };
        drop(state);
        self.emit(global_done, current, current_done, current_total);
    }

    fn on_scan_progress(&self, scanned_entries: u64, current: &EntryPath) {
        let state = lock_unpoisoned(&self.state);
        let current = if current.display.is_empty() {
            state.archive.clone()
        } else {
            format!("{}: {}", state.archive, current.display)
        };
        drop(state);
        self.inner
            .on_scan_progress(scanned_entries, &EntryPath::from_utf8(current));
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.inner.on_phase(phase, interruptible);
    }
}

impl<'a> EmitProgress<'a> {
    fn new(
        id: u64,
        events: Arc<dyn EventSink>,
        snapshots: Arc<Mutex<JobSnapshotStore>>,
        queue_sink: &'a dyn ProgressSink,
        redactions: &'a [(String, String)],
    ) -> Self {
        Self {
            id,
            events,
            snapshots,
            queue_sink,
            redactions,
            inner: Mutex::new(ProgressWindow {
                last_emit: Instant::now(),
                last_done: 0,
                speed: 0,
                latest: None,
                latest_current_file: None,
                phase: None,
                interruptible: true,
            }),
        }
    }

    fn redact_current(&self, current: &EntryPath) -> Option<EntryPath> {
        self.redactions
            .iter()
            .any(|(path, _)| !path.is_empty() && current.display.contains(path))
            .then(|| EntryPath::from_utf8(redact_source_text(&current.display, self.redactions)))
    }

    /// Emits the final pending snapshot so the bar lands on its true value.
    fn flush(&self) {
        let mut w = lock_unpoisoned(&self.inner);
        let current_file = w.latest_current_file.take();
        let latest = w.latest.take();
        let speed = w.speed;
        drop(w);

        match (current_file, latest) {
            (Some(entry), Some(latest)) if latest.current_total == 0 => {
                self.emit_event(entry, speed);
                self.emit_event(latest, speed);
            }
            (_, Some(latest)) => {
                self.emit_event(latest, speed);
            }
            (Some(entry), None) => {
                self.emit_event(entry, speed);
            }
            (None, None) => {}
        }
    }

    fn emit_event(&self, snapshot: ProgressSnapshot, speed: u64) {
        let speed = if snapshot.scanned_entries.is_some() {
            0
        } else {
            speed
        };
        let progress = JobProgressSnapshot {
            done: snapshot.done,
            total: snapshot.total,
            current: snapshot.current,
            current_done: snapshot.current_done,
            current_total: snapshot.current_total,
            scanned_entries: snapshot.scanned_entries,
            speed,
            phase: snapshot.phase,
            interruptible: snapshot.interruptible,
        };
        let Some(version) =
            lock_unpoisoned(&self.snapshots).set_progress(self.id, progress.clone())
        else {
            return;
        };
        emit(
            &*self.events,
            EV_PROGRESS,
            &JobProgressEvent {
                base: ProgressEvent {
                    id: self.id,
                    version,
                    done: progress.done,
                    total: progress.total,
                    current: progress.current,
                    current_done: progress.current_done,
                    current_total: progress.current_total,
                    scanned_entries: progress.scanned_entries,
                    speed: progress.speed,
                },
                phase: progress.phase,
                interruptible: progress.interruptible,
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_progress(
        &self,
        done: u64,
        total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
        scanned_entries: Option<u64>,
    ) {
        let mut w = lock_unpoisoned(&self.inner);
        let indeterminate_phase = matches!(
            w.phase.as_deref(),
            Some(
                "output_commit"
                    | "output_cleanup"
                    | "output_recovery"
                    | "update_recovery"
                    | "update_commit"
                    | "update_cleanup"
            )
        );
        let (done, total, current_done, current_total) = if indeterminate_phase {
            (0, 0, 0, 0)
        } else {
            (done, total, current_done, current_total)
        };
        let percentage_phase = current_total == 0
            && matches!(
                w.phase.as_deref(),
                Some(
                    "recovery_prepare"
                        | "recovery_verify"
                        | "recovery_process"
                        | "recovery_finalize"
                )
            );
        let elapsed = w.last_emit.elapsed().as_millis();
        let snapshot = ProgressSnapshot {
            done,
            total,
            current: current.display.clone(),
            current_done,
            current_total,
            scanned_entries,
            phase: w.phase.clone(),
            interruptible: w.interruptible,
        };
        if current_total > 0 {
            w.latest_current_file = Some(snapshot.clone());
        }
        if elapsed < PROGRESS_THROTTLE_MS {
            w.latest = Some(snapshot);
            return;
        }
        if scanned_entries.is_some() || percentage_phase {
            w.speed = 0;
            w.last_done = 0;
        } else {
            // Instantaneous byte speed over the emit window, lightly smoothed.
            let delta = done.saturating_sub(w.last_done);
            let instant = (delta as u128 * 1000 / elapsed.max(1)) as u64;
            w.speed = if w.speed == 0 {
                instant
            } else {
                (w.speed * 3 + instant) / 4
            };
            w.last_done = done;
        }
        w.last_emit = Instant::now();
        w.latest = None;
        let current_file = if current_total == 0 {
            w.latest_current_file.take()
        } else {
            w.latest_current_file = None;
            None
        };
        let speed = w.speed;
        drop(w);
        if let Some(entry) = current_file {
            self.emit_event(entry, speed);
        }
        self.emit_event(snapshot, speed);
    }

    fn record_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.flush();
        let Some(phase) = progress_phase_name(phase) else {
            return;
        };
        let snapshot = {
            let mut w = lock_unpoisoned(&self.inner);
            w.last_emit = Instant::now();
            w.last_done = 0;
            w.speed = 0;
            w.latest = None;
            w.latest_current_file = None;
            w.phase = Some(phase.to_owned());
            w.interruptible = interruptible;
            ProgressSnapshot {
                done: 0,
                total: 0,
                current: String::new(),
                current_done: 0,
                current_total: 0,
                scanned_entries: None,
                phase: w.phase.clone(),
                interruptible,
            }
        };
        self.emit_event(snapshot, 0);
    }
}

impl ProgressSink for EmitProgress<'_> {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.on_entry_progress(done, total, current, 0, 0);
    }

    fn on_entry_progress(
        &self,
        done: u64,
        total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        let redacted = self.redact_current(current);
        let current = redacted.as_ref().unwrap_or(current);
        self.queue_sink
            .on_entry_progress(done, total, current, current_done, current_total);
        self.record_progress(done, total, current, current_done, current_total, None);
    }

    fn on_scan_progress(&self, scanned_entries: u64, current: &EntryPath) {
        let redacted = self.redact_current(current);
        let current = redacted.as_ref().unwrap_or(current);
        self.queue_sink.on_scan_progress(scanned_entries, current);
        self.record_progress(0, 0, current, 0, 0, Some(scanned_entries));
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.queue_sink.on_phase(phase, interruptible);
        self.record_phase(phase, interruptible);
    }
}

fn progress_phase_name(phase: ProgressPhase) -> Option<&'static str> {
    match phase {
        ProgressPhase::RecoveryPrepare => Some("recovery_prepare"),
        ProgressPhase::RecoveryVerify => Some("recovery_verify"),
        ProgressPhase::RecoveryProcess => Some("recovery_process"),
        ProgressPhase::RecoveryFinalize => Some("recovery_finalize"),
        ProgressPhase::OutputRecovery => Some("output_recovery"),
        ProgressPhase::OutputSplit => Some("output_split"),
        ProgressPhase::OutputVerify => Some("output_verify"),
        ProgressPhase::OutputCommit => Some("output_commit"),
        ProgressPhase::OutputCleanup => Some("output_cleanup"),
        ProgressPhase::UpdateRecovery => Some("update_recovery"),
        ProgressPhase::UpdateRewrite => Some("update_rewrite"),
        ProgressPhase::UpdateVerify => Some("update_verify"),
        ProgressPhase::UpdateCommit => Some("update_commit"),
        ProgressPhase::UpdateCleanup => Some("update_cleanup"),
        ProgressPhase::SfxPublishVerify => Some("sfx_publish_verify"),
        ProgressPhase::SfxPublishSign => Some("sfx_publish_sign"),
        ProgressPhase::SfxPublishNotarize => Some("sfx_publish_notarize"),
        ProgressPhase::SfxPublishFinalize => Some("sfx_publish_finalize"),
        _ => None,
    }
}

/// Conflict resolver backed by the frontend dialog.
struct GuiConflictResolver {
    gui_id: u64,
    events: Arc<dyn EventSink>,
    bridge: Arc<AskBridge>,
    snapshots: Arc<Mutex<JobSnapshotStore>>,
    /// Per-job cancel flag (releases the wait when the job is cancelled)
    cancel_flag: Arc<AtomicBool>,
    /// Decision to apply to every further conflict ("apply to all")
    all: Mutex<Option<String>>,
}

impl GuiConflictResolver {
    fn apply(decision: &str, existing: &Path) -> ConflictDecision {
        match decision {
            "overwrite" => ConflictDecision::Overwrite,
            "rename" => ConflictDecision::Rename(auto_renamed_name(existing)),
            "abort" => ConflictDecision::Abort,
            _ => ConflictDecision::Skip,
        }
    }
}

impl ConflictResolver for GuiConflictResolver {
    fn resolve(&self, existing: &Path, incoming: &EntryMeta) -> ConflictDecision {
        if let Some(decision) = lock_unpoisoned(&self.all).clone() {
            return Self::apply(&decision, existing);
        }
        let meta = std::fs::symlink_metadata(existing).ok();
        let _ = lock_unpoisoned(&self.snapshots)
            .set_interaction(self.gui_id, Some(JobInteraction::Conflict));
        self.bridge.prepare(self.gui_id);
        emit(
            &*self.events,
            EV_ASK_CONFLICT,
            &AskConflictEvent {
                id: self.gui_id,
                existing_path: existing.to_string_lossy().into_owned(),
                existing_size: metadata_len_or_zero(meta.as_ref()),
                existing_modified: meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
                incoming_path: incoming.path.display.clone(),
                incoming_size: incoming.size,
                incoming_modified: incoming
                    .modified
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs()),
            },
        );
        let cancelled = || self.cancel_flag.load(Ordering::Relaxed);
        let answer = self.bridge.wait(self.gui_id, &cancelled);
        let _ = lock_unpoisoned(&self.snapshots).set_interaction(self.gui_id, None);
        match answer {
            Some(AskAnswer::Conflict {
                decision,
                apply_all,
            }) => {
                if apply_all {
                    *lock_unpoisoned(&self.all) = Some(decision.clone());
                }
                Self::apply(&decision, existing)
            }
            // Cancelled or an unexpected answer: abort safely.
            _ => ConflictDecision::Abort,
        }
    }
}

#[derive(Default)]
struct ExtractProblemCollector {
    problems: BoundedProblemLog,
}

impl ExtractProblemCollector {
    fn summary(&self) -> ProblemPreview {
        self.problems.snapshot()
    }
}

impl ExtractProblemReporter for ExtractProblemCollector {
    fn skipped_entry(&self, path: &EntryPath, error: &FormatError) {
        self.problems.record(format!("{}: {error}", path.display));
    }
}

/// Picks the first free `name (n).ext` sibling (mirrors the engine's
/// RenameBoth policy; the conflict dialog's Keep Both button).
fn auto_renamed_name(existing: &Path) -> String {
    let stem = path_stem_or_empty(existing);
    let ext = existing
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let parent = path_parent_or_empty(existing);
    for n in 1u32..=u32::MAX {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if std::fs::symlink_metadata(parent.join(&name)).is_err() {
            return name;
        }
    }
    let suffix = format!("{}-{}", std::process::id(), audit::now_millis());
    match &ext {
        Some(ext) => format!("{stem} ({suffix}).{ext}"),
        None => format!("{stem} ({suffix})"),
    }
}

/// Retries `f` with passwords from the frontend dialog: PasswordRequired /
/// WrongPassword park the job and ask; cancelling the dialog cancels the
/// job. A proven-good prompted password is cached for the session.
#[allow(clippy::too_many_arguments)] // internal helper, each role distinct
fn with_gui_password<R>(
    state: &AppState,
    bridge: &AskBridge,
    events: &dyn EventSink,
    snapshots: &Mutex<JobSnapshotStore>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    gui_id: u64,
    archive: &Path,
    display_name: Option<&str>,
    explicit: Option<&str>,
    mut f: impl FnMut(Option<&Password>) -> Result<R, FormatError>,
) -> Result<R, FormatError> {
    let mut current = explicit
        .map(Password::new)
        .or_else(|| state.password_for(archive));
    let mut prompted = false;
    loop {
        match f(current.as_ref()) {
            Ok(r) => {
                if prompted {
                    if let Some(pw) = &current {
                        state.remember_password(archive, pw.expose());
                    }
                }
                return Ok(r);
            }
            Err(e @ (FormatError::PasswordRequired | FormatError::WrongPassword)) => {
                let name = display_name
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| path_file_name_or_empty(archive));
                let _ = lock_unpoisoned(snapshots)
                    .set_interaction(gui_id, Some(JobInteraction::Password));
                bridge.prepare(gui_id);
                emit(
                    events,
                    EV_ASK_PASSWORD,
                    &AskPasswordEvent {
                        id: gui_id,
                        name,
                        wrong: matches!(e, FormatError::WrongPassword),
                    },
                );
                let cancelled = || ctl.is_cancelled() || cancel_flag.load(Ordering::Relaxed);
                let answer = bridge.wait(gui_id, &cancelled);
                let _ = lock_unpoisoned(snapshots).set_interaction(gui_id, None);
                match answer {
                    Some(AskAnswer::Password(Some(pw))) => {
                        current = Some(Password::new(pw));
                        prompted = true;
                    }
                    // Dialog cancelled (or job cancelled): stop the job.
                    _ => return Err(FormatError::Cancelled),
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn extract_result_json(
    plan: ExtractPlan,
    report: ExtractReport,
    structure: ArchiveStructureStatus,
    best_effort: bool,
    problems: ProblemPreview,
) -> serde_json::Value {
    let problems_truncated = problems.is_truncated();
    let mut result = serde_json::json!({
        "dest": report.destination.to_string_lossy(),
        "best_effort": best_effort,
        "problems": problems.messages,
        "problems_total": problems.total,
        "problems_truncated": problems_truncated,
        "plan": ExtractPlanDto::from(plan),
        "counts": {
            "destination": report.destination.to_string_lossy(),
            "selected_entries": report.selected_entries,
            "created": report.created,
            "directories": report.directories,
            "skipped": report.skipped,
            "replaced": report.replaced,
            "renamed": report.renamed,
            "failed": report.failed,
            "output_bytes": report.output_bytes,
        },
    });
    if !structure.is_complete() {
        result["structure"] = serde_json::json!(structure.id());
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_extract_archive_job(
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    snapshots: &Arc<Mutex<JobSnapshotStore>>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
    gui_id: u64,
    archive: &Path,
    archive_display_name: &str,
    dest: &Path,
    expected_destination: Option<&Path>,
    expected_input_guard: Option<ExtractInputGuard>,
    selection: Option<&[String]>,
    overwrite: &OverwritePolicy,
    symlinks: &SymlinkPolicy,
    smart: bool,
    encoding: Option<String>,
    password: Option<&str>,
    verify_sfx: bool,
    best_effort: bool,
) -> Result<(serde_json::Value, ArchiveStructureStatus), FormatError> {
    if verify_sfx {
        squallz_core::verify_sfx_payload(archive, &settings.resource_options(), sink, ctl)?;
    }
    let policy = *overwrite;
    let resolver: Option<Arc<dyn ConflictResolver>> = if policy == OverwritePolicy::Ask {
        Some(Arc::new(GuiConflictResolver {
            gui_id,
            events: Arc::clone(events),
            bridge: Arc::clone(bridge),
            snapshots: Arc::clone(snapshots),
            cancel_flag: Arc::clone(cancel_flag),
            all: Mutex::new(None),
        }))
    } else {
        None
    };
    let problem_collector = Arc::new(ExtractProblemCollector::default());
    let problem_reporter = if best_effort {
        Some(Arc::clone(&problem_collector) as Arc<dyn ExtractProblemReporter>)
    } else {
        None
    };
    let x_opts = squallz_core::api::ExtractOptions {
        overwrite: policy,
        resolver,
        symlinks: *symlinks,
        limits: settings.safety_limits(),
        resources: settings.resource_options(),
        best_effort,
        problem_reporter,
        ..Default::default()
    };
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    let (plan, report, structure) = with_gui_password(
        state,
        bridge,
        &**events,
        snapshots,
        ctl,
        cancel_flag,
        gui_id,
        &archive,
        Some(archive_display_name),
        password,
        |pw| {
            let open = OpenOptions {
                password: pw.cloned(),
                encoding_override: encoding.clone(),
            };
            state
                .engine
                .plan_and_extract_with_report_guarded_and_structure_controlled(
                    &archive,
                    &dest,
                    Path::new(archive_display_name),
                    smart,
                    &open,
                    &x_opts,
                    sink,
                    ctl,
                    expected_input_guard,
                    |entries, control| {
                        selection
                            .map(|paths| expand_selection_with_control(entries, paths, control))
                            .transpose()
                    },
                    |plan| match expected_destination {
                        Some(expected) if plan.destination != expected => {
                            Err(FormatError::destination_changed(&plan.destination))
                        }
                        _ => Ok(()),
                    },
                )
        },
    )?;
    let result = extract_result_json(
        plan,
        report,
        structure,
        best_effort,
        problem_collector.summary(),
    );
    Ok((result, structure))
}

#[allow(clippy::too_many_arguments)] // batch job shares the same GUI job context as extract
fn run_batch_extract_job(
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    snapshots: &Arc<Mutex<JobSnapshotStore>>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
    gui_id: u64,
    items: &[BatchExtractItem],
    display_items: &[BatchExtractItem],
    overwrite: &OverwritePolicy,
    symlinks: &SymlinkPolicy,
    smart: bool,
) -> Result<serde_json::Value, FormatError> {
    if items.is_empty() {
        return Err(FormatError::Unsupported(
            "batch extract requires at least one archive".into(),
        ));
    }

    let work_items = normalize_batch_extract_items_with(items, display_items, |path| {
        state.engine.archive_source_set(path)
    });
    let batch_sink = BatchProgressSink::new(sink, work_items.len());
    let mut outputs = Vec::new();
    let mut failures = Vec::new();
    let mut recovered_archives = 0usize;

    for (index, work_item) in work_items.iter().enumerate() {
        ctl.checkpoint()?;
        let item = &work_item.execution;
        let archive = PathBuf::from(&item.path);
        let dest = PathBuf::from(&item.dest);
        let shown_path = work_item.display.path.as_str();
        let label = batch_archive_label(Path::new(shown_path));
        batch_sink.start_archive(index, label.clone());
        match run_extract_archive_job(
            state,
            settings,
            bridge,
            events,
            snapshots,
            ctl,
            cancel_flag,
            &batch_sink,
            gui_id,
            &archive,
            &label,
            &dest,
            None,
            None,
            None,
            overwrite,
            symlinks,
            smart,
            item.encoding.clone(),
            item.password.as_deref(),
            false,
            item.best_effort,
        ) {
            Ok((mut result, structure)) => {
                result["archive"] = serde_json::json!(shown_path);
                if structure == ArchiveStructureStatus::ZipLocalHeadersRecovered {
                    recovered_archives = recovered_archives.saturating_add(1);
                }
                outputs.push(result);
            }
            Err(FormatError::Cancelled) => return Err(FormatError::Cancelled),
            Err(error) => {
                let dto = ErrorDto::from(&error);
                failures.push(serde_json::json!({
                    "archive": shown_path,
                    "error": {
                        "key": dto.key,
                        "params": dto.params,
                        "detail": dto.detail,
                    },
                }));
            }
        }
        batch_sink.finish_archive(index, label);
    }

    let mut result = serde_json::json!({
        "operation": "batch_extract",
        "archives": work_items.len(),
        "selected_archives": items.len(),
        "collapsed_volumes": items.len().saturating_sub(work_items.len()),
        "extracted": outputs.len(),
        "failed": failures.len(),
        "outputs": outputs,
        "failures": failures,
    });
    if recovered_archives > 0 {
        result["structure"] =
            serde_json::json!(ArchiveStructureStatus::ZipLocalHeadersRecovered.id());
        result["recovered_archives"] = serde_json::json!(recovered_archives);
    }
    Ok(result)
}

pub(crate) struct CreateJobRequest {
    pub(crate) inputs: Vec<PathBuf>,
    pub(crate) dest: PathBuf,
    pub(crate) options: CreateOptions,
    pub(crate) sfx_target: Option<SfxTarget>,
    pub(crate) post_success: PostSuccessAction,
    pub(crate) test_after_create: bool,
    pub(crate) replace_existing: bool,
    pub(crate) commit_policy: CreateCommitPolicy,
}

fn job_output_commit_policy(
    replace_existing: bool,
    replacement_guard: Option<CreateDestinationGuard>,
    operation: &str,
) -> Result<CreateCommitPolicy, FormatError> {
    match (replace_existing, replacement_guard) {
        (false, None) => Ok(CreateCommitPolicy::NoReplace),
        (false, Some(_)) => Err(FormatError::Unsupported(format!(
            "a replacement guard cannot be used by a no-replace {operation} job"
        ))),
        (true, Some(guard)) => Ok(CreateCommitPolicy::ReplaceIfUnchanged(guard)),
        (true, None) => Err(FormatError::Unsupported(format!(
            "a {operation} job cannot replace an existing output without a destination guard"
        ))),
    }
}

impl CreateJobRequest {
    pub(crate) fn sfx_options(&self) -> Option<SfxBuildOptions> {
        self.sfx_target.map(|target| SfxBuildOptions {
            target,
            overwrite: self.replace_existing,
            resources: self.options.resources,
        })
    }

    fn artifact_kind(&self) -> CreateArtifactKind {
        match self.sfx_target {
            Some(SfxTarget::Macos) => CreateArtifactKind::SfxMacosApp,
            Some(SfxTarget::Windows | SfxTarget::Linux) => CreateArtifactKind::SfxSingleFile,
            None if self.options.split_size.is_some() => CreateArtifactKind::SplitArchive,
            None => CreateArtifactKind::Archive,
        }
    }

    fn reject_existing_no_replace_destination(&self) -> Result<(), FormatError> {
        if !matches!(self.commit_policy, CreateCommitPolicy::NoReplace) {
            return Ok(());
        }
        if create_destination_has_conflict(&self.dest, self.artifact_kind())? {
            return Err(FormatError::output_exists(self.dest.clone()));
        }
        Ok(())
    }
}

/// Converts the GUI create contract into the one core request used by both
/// preflight and the worker. Keeping this conversion shared prevents a plan
/// from silently using different format, split, exclusion, or resource
/// settings than the job that follows it.
pub(crate) fn create_job_request(
    spec: &JobSpec,
    settings: &SettingsDto,
) -> Result<CreateJobRequest, FormatError> {
    let JobSpec::Compress {
        inputs,
        dest,
        level,
        password,
        encrypt_names,
        split_size,
        split_mode,
        excludes,
        content_policy,
        sqz_inner_format,
        sfx_target,
        completion: _,
        post_success,
        test_after_create,
        replace_existing,
        replacement_guard,
    } = spec
    else {
        return Err(FormatError::Unsupported(
            "create planning requires a compress job".into(),
        ));
    };

    let mut options = CreateOptions {
        level: CompressionLevel::from_numeric(*level),
        password: password.as_deref().map(Password::new),
        encrypt_filenames: *encrypt_names,
        split_size: *split_size,
        split_mode: *split_mode,
        resources: settings.resource_options(),
        excludes: content_policy.resolve_excludes(excludes),
        ..CreateOptions::default()
    };
    if let Some(inner_format) = sqz_inner_format {
        options.sqz.inner_format = *inner_format;
    }
    let sfx_target = *sfx_target;
    if let Some(target) = sfx_target {
        if target != SfxTarget::host() {
            return Err(FormatError::Unsupported(format!(
                "desktop SFX creation only supports the current {} target",
                SfxTarget::host().as_str()
            )));
        }
    }

    let replace_existing = *replace_existing;
    let commit_policy = job_output_commit_policy(replace_existing, *replacement_guard, "create")?;
    Ok(CreateJobRequest {
        inputs: inputs.iter().map(PathBuf::from).collect(),
        dest: PathBuf::from(dest),
        options,
        sfx_target,
        post_success: *post_success,
        test_after_create: *test_after_create || *post_success == PostSuccessAction::TrashSource,
        replace_existing,
        commit_policy,
    })
}

/// Resolves the destination-writing options shared by conversion preflight
/// and the queued worker.
pub(crate) fn convert_create_options(
    spec: &JobSpec,
    settings: &SettingsDto,
) -> Result<CreateOptions, FormatError> {
    let JobSpec::Convert {
        level,
        dest_password,
        encrypt_names,
        split_size,
        split_mode,
        ..
    } = spec
    else {
        return Err(FormatError::Unsupported(
            "conversion planning requires a convert job".into(),
        ));
    };
    Ok(CreateOptions {
        level: CompressionLevel::from_numeric(*level),
        password: dest_password.as_deref().map(Password::new),
        encrypt_filenames: *encrypt_names,
        split_size: *split_size,
        split_mode: *split_mode,
        resources: settings.resource_options(),
        ..CreateOptions::default()
    })
}

#[derive(Debug)]
struct CleanupCandidate {
    path: PathBuf,
    identity: PathBuf,
    is_dir: bool,
    snapshot: FrozenTreeSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrozenPathKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenMetadata {
    kind: FrozenPathKind,
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrozenTreeSnapshot {
    fingerprint: blake3::Hash,
    entries: usize,
    supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FingerprintError {
    Cancelled,
    Unavailable,
}

impl FrozenMetadata {
    fn capture(metadata: &fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_file() {
            FrozenPathKind::File
        } else if file_type.is_dir() {
            FrozenPathKind::Directory
        } else if file_type.is_symlink() {
            FrozenPathKind::Symlink
        } else {
            FrozenPathKind::Other
        };
        Self {
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            mode: metadata.mode(),
        }
    }

    fn update_fingerprint(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&[match self.kind {
            FrozenPathKind::File => 1,
            FrozenPathKind::Directory => 2,
            FrozenPathKind::Symlink => 3,
            FrozenPathKind::Other => 4,
        }]);
        hasher.update(&self.len.to_le_bytes());
        update_system_time(hasher, self.modified);
        update_system_time(hasher, self.created);
        hasher.update(&[u8::from(self.readonly)]);
        #[cfg(unix)]
        {
            hasher.update(&self.device.to_le_bytes());
            hasher.update(&self.inode.to_le_bytes());
            hasher.update(&self.mode.to_le_bytes());
        }
    }
}

fn update_system_time(hasher: &mut blake3::Hasher, value: Option<SystemTime>) {
    let Some(value) = value else {
        hasher.update(&[0]);
        return;
    };
    match value.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            hasher.update(&[1]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
        Err(error) => {
            let duration = error.duration();
            hasher.update(&[2]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
    }
}

#[cfg(unix)]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;

    let bytes = value.as_bytes();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(target_os = "windows")]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt;

    let units: Vec<u16> = value.encode_wide().collect();
    hasher.update(&(units.len() as u64).to_le_bytes());
    for unit in units {
        hasher.update(&unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, target_os = "windows")))]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    let value = value.to_string_lossy();
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn update_fingerprint_entry(
    hasher: &mut blake3::Hasher,
    root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), ()> {
    let relative = path.strip_prefix(root).map_err(|_| ())?;
    update_os_str(hasher, relative.as_os_str());
    FrozenMetadata::capture(metadata).update_fingerprint(hasher);
    Ok(())
}

fn fingerprint_checkpoint(is_cancelled: &dyn Fn() -> bool) -> Result<(), FingerprintError> {
    if is_cancelled() {
        Err(FingerprintError::Cancelled)
    } else {
        Ok(())
    }
}

fn update_symlink_target_fingerprint(
    hasher: &mut blake3::Hasher,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FingerprintError> {
    if metadata.file_type().is_symlink() {
        hasher.update(b"symlink-target\0");
        let target = fs::read_link(path).map_err(|_| FingerprintError::Unavailable)?;
        update_os_str(hasher, target.as_os_str());
    }
    Ok(())
}

fn capture_tree_snapshot(
    root: &Path,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<FrozenTreeSnapshot, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let root_metadata = fs::symlink_metadata(root).map_err(|_| FingerprintError::Unavailable)?;
    let mut hasher = blake3::Hasher::new();
    let mut supported =
        root_metadata.is_file() || root_metadata.is_dir() || root_metadata.file_type().is_symlink();
    let mut entries_seen = 1usize;
    update_fingerprint_entry(&mut hasher, root, root, &root_metadata)
        .map_err(|_| FingerprintError::Unavailable)?;
    update_symlink_target_fingerprint(&mut hasher, root, &root_metadata)?;
    progress.on_progress(
        entries_seen as u64,
        0,
        &EntryPath::from_utf8(root.to_string_lossy().into_owned()),
    );
    if !root_metadata.is_dir() {
        return Ok(FrozenTreeSnapshot {
            fingerprint: hasher.finalize(),
            entries: entries_seen,
            supported,
        });
    }

    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        fingerprint_checkpoint(is_cancelled)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| FingerprintError::Unavailable)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| FingerprintError::Unavailable)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            fingerprint_checkpoint(is_cancelled)?;
            let path = entry.path();
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| FingerprintError::Unavailable)?;
            update_fingerprint_entry(&mut hasher, root, &path, &metadata)
                .map_err(|_| FingerprintError::Unavailable)?;
            update_symlink_target_fingerprint(&mut hasher, &path, &metadata)?;
            supported &=
                metadata.is_file() || metadata.is_dir() || metadata.file_type().is_symlink();
            entries_seen = entries_seen.saturating_add(1);
            progress.on_progress(
                entries_seen as u64,
                0,
                &EntryPath::from_utf8(path.to_string_lossy().into_owned()),
            );
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(FrozenTreeSnapshot {
        fingerprint: hasher.finalize(),
        entries: entries_seen,
        supported,
    })
}

#[derive(Debug)]
enum SourceCleanupPlan {
    NotRequested { kept: usize },
    Ready(Vec<CleanupCandidate>),
    Blocked { kept: usize },
    Failed { kept: usize },
}

impl SourceCleanupPlan {
    fn requires_content_verification(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceCleanupStatus {
    NotRequested,
    Completed,
    Blocked,
    Partial,
    Failed,
    Cancelled,
}

impl SourceCleanupStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceCleanupResult {
    status: SourceCleanupStatus,
    moved: usize,
    kept: usize,
    recovery_required: usize,
}

impl SourceCleanupResult {
    fn new(status: SourceCleanupStatus, moved: usize, kept: usize) -> Self {
        Self {
            status,
            moved,
            kept,
            recovery_required: 0,
        }
    }

    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status.as_str(),
            "moved": self.moved,
            "kept": self.kept,
            "recovery_required": self.recovery_required,
        })
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, ()> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .map_err(|_| ())
    }
}

fn cleanup_candidate(
    path: &Path,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<CleanupCandidate, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let absolute = absolute_path(path).map_err(|_| FingerprintError::Unavailable)?;
    let metadata = fs::symlink_metadata(&absolute).map_err(|_| FingerprintError::Unavailable)?;
    if metadata.file_type().is_symlink() {
        let parent = absolute.parent().ok_or(FingerprintError::Unavailable)?;
        let file_name = absolute.file_name().ok_or(FingerprintError::Unavailable)?;
        let identity = fs::canonicalize(parent)
            .map_err(|_| FingerprintError::Unavailable)?
            .join(file_name);
        return Ok(CleanupCandidate {
            path: identity.clone(),
            identity,
            is_dir: false,
            snapshot: capture_tree_snapshot(&absolute, is_cancelled, progress)?,
        });
    }

    let identity = fs::canonicalize(&absolute).map_err(|_| FingerprintError::Unavailable)?;
    let snapshot = capture_tree_snapshot(&identity, is_cancelled, progress)?;
    Ok(CleanupCandidate {
        path: identity.clone(),
        identity,
        is_dir: metadata.is_dir(),
        snapshot,
    })
}

fn top_level_cleanup_candidates(
    inputs: &[PathBuf],
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<Vec<CleanupCandidate>, FingerprintError> {
    let mut candidates = inputs
        .iter()
        .map(|path| cleanup_candidate(path, is_cancelled, progress))
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort_by(|left, right| {
        left.identity
            .components()
            .count()
            .cmp(&right.identity.components().count())
            .then_with(|| left.identity.cmp(&right.identity))
    });

    let mut top_level: Vec<CleanupCandidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let nested_or_duplicate = top_level.iter().any(|selected| {
            selected.identity == candidate.identity
                || (selected.is_dir && candidate.identity.starts_with(&selected.identity))
        });
        if !nested_or_duplicate {
            top_level.push(candidate);
        }
    }
    Ok(top_level)
}

fn prepare_source_cleanup(
    inputs: &[PathBuf],
    action: PostSuccessAction,
    excludes: &[String],
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<SourceCleanupPlan, FormatError> {
    if action != PostSuccessAction::TrashSource {
        return Ok(SourceCleanupPlan::NotRequested { kept: inputs.len() });
    }
    if !excludes.is_empty() {
        return Ok(SourceCleanupPlan::Blocked { kept: inputs.len() });
    }
    match top_level_cleanup_candidates(inputs, is_cancelled, progress) {
        Ok(candidates) => Ok(SourceCleanupPlan::Ready(candidates)),
        Err(FingerprintError::Cancelled) => Err(FormatError::Cancelled),
        Err(FingerprintError::Unavailable) => Ok(SourceCleanupPlan::Failed { kept: inputs.len() }),
    }
}

fn cleanup_is_blocked(candidates: &[CleanupCandidate], outputs: &[PathBuf]) -> Result<bool, ()> {
    for output in outputs {
        let output_metadata = fs::symlink_metadata(output).map_err(|_| ())?;
        let output_identity = fs::canonicalize(output).map_err(|_| ())?;
        for candidate in candidates {
            if candidate.identity == output_identity
                || (candidate.is_dir && output_identity.starts_with(&candidate.identity))
                || (output_metadata.is_dir() && candidate.identity.starts_with(&output_identity))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchivedSourceEntry {
    archive_paths: Vec<EntryPath>,
    entry_type: squallz_core::api::EntryType,
    size: u64,
    modified: Option<CreateInputModifiedTime>,
    unix_mode: Option<u32>,
    blake3: Option<[u8; 32]>,
}

impl ArchivedSourceEntry {
    fn matches_manifest_entry(&self, input: &CreateInputManifestEntry) -> bool {
        self.entry_type == input.entry_type
            && self.size == input.size
            && self.modified == input.modified
            && self.unix_mode == input.unix_mode
            && self.blake3 == input.blake3
    }
}

type ArchivedInputMap = BTreeMap<PathBuf, ArchivedSourceEntry>;

fn verify_manifest_archive_path(path: &EntryPath) -> Result<(), FingerprintError> {
    if path.encoding != "utf-8"
        || std::str::from_utf8(&path.raw).ok() != Some(path.display.as_str())
    {
        return Err(FingerprintError::Unavailable);
    }
    Ok(())
}

fn archived_input_map(
    archived_inputs: &[CreateInputManifestEntry],
) -> Result<ArchivedInputMap, FingerprintError> {
    let mut inputs = BTreeMap::new();
    let mut archive_sources = BTreeMap::new();
    for input in archived_inputs {
        if input.source_path.to_str().is_none() {
            return Err(FingerprintError::Unavailable);
        }
        verify_manifest_archive_path(&input.archive_path)?;
        match archive_sources.entry(input.archive_path.raw.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(input.source_path.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry)
                if entry.get() == &input.source_path => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(FingerprintError::Unavailable);
            }
        }

        let source = ArchivedSourceEntry {
            archive_paths: vec![input.archive_path.clone()],
            entry_type: input.entry_type.clone(),
            size: input.size,
            modified: input.modified,
            unix_mode: input.unix_mode,
            blake3: input.blake3,
        };
        match inputs.entry(input.source_path.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(source);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if !entry.get().matches_manifest_entry(input) {
                    return Err(FingerprintError::Unavailable);
                }
                if !entry.get().archive_paths.contains(&input.archive_path) {
                    entry
                        .get_mut()
                        .archive_paths
                        .push(input.archive_path.clone());
                }
            }
        }
    }
    Ok(inputs)
}

fn archived_input_belongs_to_candidate(path: &Path, candidate: &CleanupCandidate) -> bool {
    path == candidate.identity || (candidate.is_dir && path.starts_with(&candidate.identity))
}

fn verify_archived_file(
    path: &Path,
    expected_size: u64,
    expected_blake3: &[u8; 32],
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    if !metadata.is_file() || metadata.len() != expected_size {
        return Ok(false);
    }
    let identity = fs::canonicalize(path).map_err(|_| FingerprintError::Unavailable)?;
    if identity != path {
        return Ok(false);
    }

    let mut file = fs::File::open(path).map_err(|_| FingerprintError::Unavailable)?;
    let current = EntryPath::from_utf8(path.to_string_lossy().into_owned());
    let mut hasher = blake3::Hasher::new();
    let mut file_bytes = 0u64;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        fingerprint_checkpoint(is_cancelled)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| FingerprintError::Unavailable)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file_bytes = file_bytes.saturating_add(read as u64);
        progress.on_entry_progress(
            verified_bytes.saturating_add(file_bytes),
            total_bytes,
            &current,
            file_bytes.min(expected_size),
            expected_size,
        );
    }
    *verified_bytes = verified_bytes.saturating_add(file_bytes);
    if file_bytes != expected_size || hasher.finalize().as_bytes() != expected_blake3 {
        return Ok(false);
    }
    let final_metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    Ok(final_metadata.is_file()
        && final_metadata.len() == expected_size
        && fs::canonicalize(path).is_ok_and(|current| current == path))
}

fn verify_archived_entry(
    path: &Path,
    expected: &ArchivedSourceEntry,
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    fingerprint_checkpoint(is_cancelled)?;
    if expected.archive_paths.is_empty() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| FingerprintError::Unavailable)?;
    if metadata.modified().ok().map(CreateInputModifiedTime::from) != expected.modified {
        return Ok(false);
    }
    #[cfg(unix)]
    let current_mode = Some(metadata.mode());
    #[cfg(not(unix))]
    let current_mode = None;
    if current_mode != expected.unix_mode {
        return Ok(false);
    }

    match &expected.entry_type {
        squallz_core::api::EntryType::File => {
            let Some(blake3) = expected.blake3.as_ref() else {
                return Ok(false);
            };
            verify_archived_file(
                path,
                expected.size,
                blake3,
                verified_bytes,
                total_bytes,
                is_cancelled,
                progress,
            )
        }
        squallz_core::api::EntryType::Dir => {
            Ok(metadata.is_dir() && expected.size == 0 && expected.blake3.is_none())
        }
        squallz_core::api::EntryType::Symlink { target } => {
            if !metadata.file_type().is_symlink() || expected.size != 0 || expected.blake3.is_some()
            {
                return Ok(false);
            }
            let current_target = fs::read_link(path).map_err(|_| FingerprintError::Unavailable)?;
            Ok(std::str::from_utf8(target)
                .ok()
                .is_some_and(|target| current_target.to_str() == Some(target)))
        }
        squallz_core::api::EntryType::Hardlink { .. } | squallz_core::api::EntryType::Other => {
            Ok(false)
        }
    }
}

fn verify_cleanup_candidate_at(
    candidate: &CleanupCandidate,
    current_root: &Path,
    archived_inputs: &ArchivedInputMap,
    verified_bytes: &mut u64,
    total_bytes: u64,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> Result<bool, FingerprintError> {
    if !candidate.snapshot.supported {
        return Ok(false);
    }
    let before = capture_tree_snapshot(current_root, is_cancelled, progress)?;
    if before != candidate.snapshot {
        return Ok(false);
    }

    let expected: Vec<_> = archived_inputs
        .iter()
        .filter(|(path, _)| archived_input_belongs_to_candidate(path, candidate))
        .collect();
    if expected.len() != candidate.snapshot.entries {
        return Ok(false);
    }
    for (path, archived) in expected {
        let relative = path
            .strip_prefix(&candidate.identity)
            .map_err(|_| FingerprintError::Unavailable)?;
        let current_path = if relative.as_os_str().is_empty() {
            current_root.to_path_buf()
        } else {
            current_root.join(relative)
        };
        if !verify_archived_entry(
            &current_path,
            archived,
            verified_bytes,
            total_bytes,
            is_cancelled,
            progress,
        )? {
            return Ok(false);
        }
    }

    let after = capture_tree_snapshot(current_root, is_cancelled, progress)?;
    Ok(after == candidate.snapshot)
}

struct StagedCleanupCandidate {
    pending: PendingSourceCleanup,
}

impl StagedCleanupCandidate {
    fn staged_path(&self) -> &Path {
        &self.pending.record().staged
    }
}

fn create_cleanup_staging_dir(source: &Path) -> io::Result<PathBuf> {
    create_cleanup_staging_dir_with(source, sync_directory, remove_empty_holder_if_identity)
}

fn create_cleanup_staging_dir_with<S, R>(
    source: &Path,
    mut sync: S,
    mut remove: R,
) -> io::Result<PathBuf>
where
    S: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, SourcePathIdentity) -> io::Result<()>,
{
    let parent = source.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source cleanup path has no parent directory",
        )
    })?;
    for _ in 0..1000u32 {
        let sequence = TRASH_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let holder = parent.join(format!("{HOLDER_PREFIX}{}-{sequence}", std::process::id()));
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;

            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700).create(&holder)
        };
        #[cfg(not(unix))]
        let created = fs::create_dir(&holder);
        match created {
            Ok(()) => {
                sync_created_cleanup_holder_with(&holder, parent, &mut sync, &mut remove)?;
                return Ok(holder);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a private source cleanup directory",
    ))
}

fn sync_created_cleanup_holder_with<S, R>(
    holder: &Path,
    parent: &Path,
    sync: &mut S,
    remove: &mut R,
) -> io::Result<()>
where
    S: FnMut(&Path) -> io::Result<()>,
    R: FnMut(&Path, SourcePathIdentity) -> io::Result<()>,
{
    let identity = source_path_identity(holder)?;
    let durability = sync(holder).and_then(|()| sync(parent));
    if let Err(error) = durability {
        let _ = remove(holder, identity);
        let _ = sync(parent);
        return Err(error);
    }
    Ok(())
}

fn stage_cleanup_candidate(
    candidate: &CleanupCandidate,
    journal: &SourceCleanupJournal,
    recovery_notice: Option<&Mutex<SourceCleanupRecoveryState>>,
) -> Result<StagedCleanupCandidate, StagedRestoreOutcome> {
    let holder =
        create_cleanup_staging_dir(&candidate.path).map_err(|_| StagedRestoreOutcome::Restored)?;
    let Some(file_name) = candidate.path.file_name() else {
        let _ = fs::remove_dir(&holder);
        return Err(StagedRestoreOutcome::Restored);
    };
    let staged = holder.join(file_name);
    let record = match SourceCleanupRecord::new(candidate.path.clone(), staged, holder.clone()) {
        Ok(record) => record,
        Err(_) => {
            let _ = fs::remove_dir(&holder);
            return Err(StagedRestoreOutcome::Restored);
        }
    };
    let pending = match journal.begin(&record) {
        Ok(pending) => pending,
        Err(_) => {
            let _ = fs::remove_dir(&holder);
            let recovery = journal.recover_pending();
            let outcome = staged_recovery_outcome_for_begin(&recovery);
            if let Some(notice) = recovery_notice {
                lock_unpoisoned(notice).publish_new(source_cleanup_recovery_notice(
                    recovery,
                    journal.recovery_record_path(),
                ));
            }
            return Err(outcome);
        }
    };
    let staged = StagedCleanupCandidate { pending };
    if squallz_core::move_path_no_replace(&candidate.path, staged.staged_path()).is_err() {
        return Err(restore_staged_cleanup(staged, false));
    }
    if staged.pending.sync_after_stage().is_err() {
        return Err(restore_staged_cleanup(staged, false));
    }
    Ok(staged)
}

fn staged_recovery_outcome_for_begin(
    recovery: &io::Result<SourceCleanupRecovery>,
) -> StagedRestoreOutcome {
    match recovery {
        Ok(SourceCleanupRecovery::Restored { .. } | SourceCleanupRecovery::Cleared) => {
            StagedRestoreOutcome::Restored
        }
        Ok(SourceCleanupRecovery::Preserved { .. }) => StagedRestoreOutcome::Preserved,
        Ok(SourceCleanupRecovery::Changed { .. }) => StagedRestoreOutcome::RestoredNeedsReview,
        Ok(SourceCleanupRecovery::None | SourceCleanupRecovery::CompletedUnknown { .. })
        | Err(_) => StagedRestoreOutcome::Failed,
    }
}

fn staged_recovery_outcome(
    recovery: io::Result<SourceCleanupRecovery>,
    review_restored: bool,
) -> StagedRestoreOutcome {
    match recovery {
        Ok(SourceCleanupRecovery::Restored { .. } | SourceCleanupRecovery::Cleared) => {
            if review_restored {
                StagedRestoreOutcome::RestoredNeedsReview
            } else {
                StagedRestoreOutcome::Restored
            }
        }
        Ok(SourceCleanupRecovery::Preserved { .. }) => StagedRestoreOutcome::Preserved,
        Ok(SourceCleanupRecovery::Changed { .. }) => StagedRestoreOutcome::RestoredNeedsReview,
        Ok(SourceCleanupRecovery::None | SourceCleanupRecovery::CompletedUnknown { .. })
        | Err(_) => StagedRestoreOutcome::Failed,
    }
}

fn restore_staged_cleanup(
    staged: StagedCleanupCandidate,
    review_restored: bool,
) -> StagedRestoreOutcome {
    staged_recovery_outcome(staged.pending.recover(), review_restored)
}

fn staged_path_exists(staged: &StagedCleanupCandidate) -> bool {
    match fs::symlink_metadata(staged.staged_path()) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn cleanup_failed_after_stage(
    moved: usize,
    total: usize,
    outcome: StagedRestoreOutcome,
) -> SourceCleanupResult {
    cleanup_result_after_restore(moved, total, SourceCleanupStatus::Failed, outcome)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StagedRestoreOutcome {
    Restored,
    RestoredNeedsReview,
    Preserved,
    Failed,
}

impl StagedRestoreOutcome {
    fn needs_recovery(self) -> bool {
        self != Self::Restored
    }
}

fn cleanup_result_after_restore(
    moved: usize,
    total: usize,
    restored_status: SourceCleanupStatus,
    outcome: StagedRestoreOutcome,
) -> SourceCleanupResult {
    let recovery_required = usize::from(outcome.needs_recovery());
    let status = if recovery_required == 0 {
        restored_status
    } else if moved == 0 {
        SourceCleanupStatus::Failed
    } else {
        SourceCleanupStatus::Partial
    };
    SourceCleanupResult {
        status,
        moved,
        kept: total.saturating_sub(moved),
        recovery_required,
    }
}

#[cfg(test)]
fn complete_source_cleanup(
    plan: SourceCleanupPlan,
    outputs: &[PathBuf],
    archived_inputs: &[CreateInputManifestEntry],
    trash_adapter: &dyn TrashAdapter,
    journal: &SourceCleanupJournal,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> SourceCleanupResult {
    complete_source_cleanup_with_notice(
        plan,
        outputs,
        archived_inputs,
        trash_adapter,
        journal,
        None,
        is_cancelled,
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn complete_source_cleanup_with_notice(
    plan: SourceCleanupPlan,
    outputs: &[PathBuf],
    archived_inputs: &[CreateInputManifestEntry],
    trash_adapter: &dyn TrashAdapter,
    journal: &SourceCleanupJournal,
    recovery_notice: Option<&Mutex<SourceCleanupRecoveryState>>,
    is_cancelled: &dyn Fn() -> bool,
    progress: &dyn ProgressSink,
) -> SourceCleanupResult {
    let candidates = match plan {
        SourceCleanupPlan::NotRequested { kept } => {
            return SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, kept);
        }
        SourceCleanupPlan::Ready(candidates) => candidates,
        SourceCleanupPlan::Blocked { kept } => {
            return SourceCleanupResult::new(SourceCleanupStatus::Blocked, 0, kept);
        }
        SourceCleanupPlan::Failed { kept } => {
            return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, kept);
        }
    };
    let archived_inputs = match archived_input_map(archived_inputs) {
        Ok(inputs) => inputs,
        Err(_) => {
            return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, candidates.len());
        }
    };
    match cleanup_is_blocked(&candidates, outputs) {
        Ok(true) => {
            return SourceCleanupResult::new(SourceCleanupStatus::Blocked, 0, candidates.len());
        }
        Ok(false) => {}
        Err(()) => {
            return SourceCleanupResult::new(SourceCleanupStatus::Failed, 0, candidates.len());
        }
    }

    let total_bytes = archived_inputs
        .values()
        .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
        .fold(0u64, |total, entry| total.saturating_add(entry.size));
    let mut verified_bytes = 0u64;
    let mut moved = 0usize;
    for candidate in &candidates {
        if is_cancelled() {
            return SourceCleanupResult::new(
                SourceCleanupStatus::Cancelled,
                moved,
                candidates.len().saturating_sub(moved),
            );
        }
        let staged = match stage_cleanup_candidate(candidate, journal, recovery_notice) {
            Ok(staged) => staged,
            Err(outcome) => {
                return cleanup_failed_after_stage(moved, candidates.len(), outcome);
            }
        };
        let staged_unchanged = match verify_cleanup_candidate_at(
            candidate,
            staged.staged_path(),
            &archived_inputs,
            &mut verified_bytes,
            total_bytes,
            is_cancelled,
            progress,
        ) {
            Ok(unchanged) => unchanged,
            Err(FingerprintError::Cancelled) => {
                let outcome = restore_staged_cleanup(staged, false);
                return cleanup_result_after_restore(
                    moved,
                    candidates.len(),
                    SourceCleanupStatus::Cancelled,
                    outcome,
                );
            }
            Err(FingerprintError::Unavailable) => false,
        };
        let cancelled = is_cancelled();
        if !staged_unchanged || cancelled {
            let outcome = restore_staged_cleanup(staged, false);
            let restored_status = if cancelled {
                SourceCleanupStatus::Cancelled
            } else if moved == 0 {
                SourceCleanupStatus::Blocked
            } else {
                SourceCleanupStatus::Partial
            };
            return cleanup_result_after_restore(moved, candidates.len(), restored_status, outcome);
        }
        if staged.pending.confirm_staged_source().is_err() {
            let outcome = restore_staged_cleanup(staged, false);
            return cleanup_result_after_restore(
                moved,
                candidates.len(),
                SourceCleanupStatus::Blocked,
                outcome,
            );
        }
        if trash_adapter.move_to_trash(staged.staged_path()).is_ok() {
            let staged_removed = !staged_path_exists(&staged);
            if staged.pending.complete_trash().is_ok() {
                moved += 1;
            } else {
                if staged_removed {
                    moved += 1;
                }
                let outcome = restore_staged_cleanup(staged, true);
                return cleanup_failed_after_stage(moved, candidates.len(), outcome);
            }
        } else {
            let outcome = restore_staged_cleanup(staged, true);
            if outcome.needs_recovery() {
                return cleanup_result_after_restore(
                    moved,
                    candidates.len(),
                    SourceCleanupStatus::Failed,
                    outcome,
                );
            }
        }
    }
    let kept = candidates.len().saturating_sub(moved);
    let status = if kept == 0 {
        SourceCleanupStatus::Completed
    } else if moved == 0 {
        SourceCleanupStatus::Failed
    } else {
        SourceCleanupStatus::Partial
    };
    SourceCleanupResult::new(status, moved, kept)
}

fn test_created_archive(
    engine: &Engine,
    path: &Path,
    password: Option<&Password>,
    enabled: bool,
    sink: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<Option<u64>, FormatError> {
    if !enabled {
        return Ok(None);
    }
    sink.on_phase(ProgressPhase::OutputVerify, true);
    let report = engine.test_summary(
        path,
        &OpenOptions {
            password: password.cloned(),
            encoding_override: None,
        },
        sink,
        ctl,
    )?;
    if report.is_ok() {
        return Ok(Some(report.entries_tested));
    }
    let preview = report.problems.messages.join("; ");
    let detail = if preview.is_empty() {
        format!(
            "created archive failed integrity testing with {} problem(s): {}",
            report.problems.total,
            path.display()
        )
    } else {
        format!(
            "created archive failed integrity testing: {}: {preview}",
            path.display()
        )
    };
    Err(FormatError::CorruptArchive(detail))
}

fn create_report_result(
    report: CreateReport,
    operation: &'static str,
    source_cleanup: SourceCleanupResult,
    integrity_entries: Option<u64>,
) -> serde_json::Value {
    let volume_count = report.split_volume_count.unwrap_or(1);
    let split = report.split_volume_count.is_some();
    let primary_output = report.primary_output.to_string_lossy().into_owned();
    let outputs: Vec<String> = report
        .outputs
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let preserved_outputs: Vec<String> = report
        .preserved_outputs
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    serde_json::json!({
        "operation": operation,
        "primary_output": primary_output,
        "outputs": outputs,
        "preserved_outputs": preserved_outputs,
        "total_bytes": report.total_output_bytes,
        "volume_count": volume_count,
        "split": split,
        "tested_after_create": integrity_entries.is_some(),
        "entries_tested_after_create": integrity_entries,
        "source_cleanup": source_cleanup.to_json(),
    })
}

fn sfx_report_result(
    report: SfxBuildReport,
    source_cleanup: SourceCleanupResult,
    integrity_entries: Option<u64>,
) -> serde_json::Value {
    let primary_output = report.path.to_string_lossy().into_owned();
    let preserved_outputs = report
        .preserved_outputs
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    serde_json::json!({
        "operation": "create_sfx",
        "primary_output": primary_output.clone(),
        "outputs": [primary_output],
        "preserved_outputs": preserved_outputs,
        "volume_count": 1,
        "split": false,
        "target": report.target.as_str(),
        "layout": report.layout.as_str(),
        "payload_bytes": report.payload_bytes,
        "total_bytes": report.total_bytes,
        "requires_signing": report.requires_signing,
        "tested_after_create": integrity_entries.is_some(),
        "entries_tested_after_create": integrity_entries,
        "source_cleanup": source_cleanup.to_json(),
    })
}

#[allow(clippy::too_many_arguments)] // worker entry point, each role distinct
fn run_job(
    spec: &JobSpec,
    display_spec: &JobSpec,
    gui_id: u64,
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
    snapshots: &Arc<Mutex<JobSnapshotStore>>,
    sfx_template: Option<&Path>,
    trash_adapter: &dyn TrashAdapter,
    source_cleanup_journal: &SourceCleanupJournal,
    source_cleanup_recovery: &Mutex<SourceCleanupRecoveryState>,
) -> Result<Option<serde_json::Value>, FormatError> {
    match spec {
        JobSpec::Compress { .. } => {
            let request = create_job_request(spec, settings)?;
            request.reject_existing_no_replace_destination()?;
            let cleanup_plan = prepare_source_cleanup(
                &request.inputs,
                request.post_success,
                &request.options.excludes,
                &|| ctl.is_cancelled() || cancel_flag.load(Ordering::Relaxed),
                sink,
            )?;
            let verify_sources = cleanup_plan.requires_content_verification();
            let Some(sfx_options) = request.sfx_options() else {
                let (report, archived_manifest) = if verify_sources {
                    let verified = state.engine.create_with_verification_policy(
                        &request.dest,
                        &request.inputs,
                        &request.options,
                        request.commit_policy,
                        sink,
                        ctl,
                    )?;
                    (verified.create, verified.manifest)
                } else {
                    let report = state.engine.create_with_report_policy(
                        &request.dest,
                        &request.inputs,
                        &request.options,
                        request.commit_policy,
                        sink,
                        ctl,
                    )?;
                    (report, Vec::new())
                };
                let integrity_entries = test_created_archive(
                    &state.engine,
                    &report.primary_output,
                    request.options.password.as_ref(),
                    request.test_after_create,
                    sink,
                    ctl,
                )?;
                let source_cleanup = complete_source_cleanup_with_notice(
                    cleanup_plan,
                    &report.outputs,
                    &archived_manifest,
                    trash_adapter,
                    source_cleanup_journal,
                    Some(source_cleanup_recovery),
                    &|| ctl.is_cancelled() || cancel_flag.load(Ordering::Relaxed),
                    sink,
                );
                return Ok(Some(create_report_result(
                    report,
                    "create",
                    source_cleanup,
                    integrity_entries,
                )));
            };
            let template = sfx_template.ok_or_else(|| {
                FormatError::DependencyMissing("Squallz SFX runtime template".into())
            })?;
            let (report, archived_manifest) = if verify_sources {
                let verified = state
                    .engine
                    .create_sfx_from_inputs_with_verification_and_policy(
                        template,
                        &request.inputs,
                        &request.dest,
                        &request.options,
                        &sfx_options,
                        request.commit_policy,
                        sink,
                        ctl,
                    )?;
                (verified.sfx, verified.manifest)
            } else {
                (
                    state.engine.create_sfx_from_inputs_with_policy(
                        template,
                        &request.inputs,
                        &request.dest,
                        &request.options,
                        &sfx_options,
                        request.commit_policy,
                        sink,
                        ctl,
                    )?,
                    Vec::new(),
                )
            };
            let integrity_entries = test_created_archive(
                &state.engine,
                &report.path,
                request.options.password.as_ref(),
                request.test_after_create,
                sink,
                ctl,
            )?;
            let source_cleanup = complete_source_cleanup_with_notice(
                cleanup_plan,
                std::slice::from_ref(&report.path),
                &archived_manifest,
                trash_adapter,
                source_cleanup_journal,
                Some(source_cleanup_recovery),
                &|| ctl.is_cancelled() || cancel_flag.load(Ordering::Relaxed),
                sink,
            );
            Ok(Some(sfx_report_result(
                report,
                source_cleanup,
                integrity_entries,
            )))
        }
        JobSpec::PublishMacosSfx {
            source,
            output,
            identity,
            notary_profile,
        } => {
            let mut phase = |next| match next {
                MacosSfxPublishPhase::Verify => {
                    sink.on_phase(ProgressPhase::SfxPublishVerify, true);
                }
                MacosSfxPublishPhase::Sign => {
                    sink.on_phase(ProgressPhase::SfxPublishSign, true);
                }
                MacosSfxPublishPhase::Notarize => {
                    sink.on_phase(ProgressPhase::SfxPublishNotarize, true);
                }
                MacosSfxPublishPhase::Finalize => {
                    sink.on_phase(ProgressPhase::SfxPublishFinalize, true);
                }
                MacosSfxPublishPhase::Commit => {
                    sink.on_phase(ProgressPhase::OutputCommit, false);
                }
            };
            let report = publish_macos_sfx(
                ctl,
                Path::new(source),
                Path::new(output),
                identity,
                notary_profile,
                &settings.resource_options(),
                sink,
                sink,
                &mut phase,
            )?;
            Ok(Some(serde_json::json!({
                "operation": "sfx_publish_macos",
                "source": report.source.to_string_lossy(),
                "primary_output": report.output.to_string_lossy(),
                "outputs": [report.output.to_string_lossy()],
                "target": report.info.target.as_str(),
                "layout": report.info.layout.as_str(),
                "payload_bytes": report.info.payload_bytes,
                "total_bytes": report.info.total_bytes,
                "signature": "developer_id",
                "team_id": report.team_id,
                "notarization": "Accepted",
                "submission_id": report.submission_id,
                "stapled": true,
                "codesign_verified": true,
                "gatekeeper_verified": true,
                "checksum_verified": true,
                "source_preserved": true,
                "requires_signing": false,
                "auto_run": false,
            })))
        }
        JobSpec::Extract {
            path,
            dest,
            expected_destination,
            expected_input_guard,
            selection,
            overwrite,
            symlinks,
            smart,
            encoding,
            password,
            verify_sfx,
            best_effort,
        } => {
            let archive = PathBuf::from(path);
            let dest = PathBuf::from(dest);
            let display_path = match display_spec {
                JobSpec::Extract { path, .. } => path.as_str(),
                _ => path.as_str(),
            };
            let display_name = batch_archive_label(Path::new(display_path));
            let (result, _) = run_extract_archive_job(
                state,
                settings,
                bridge,
                events,
                snapshots,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                &archive,
                &display_name,
                &dest,
                expected_destination.as_deref().map(Path::new),
                *expected_input_guard,
                selection.as_deref(),
                overwrite,
                symlinks,
                *smart,
                encoding.clone(),
                password.as_deref(),
                *verify_sfx,
                *best_effort,
            )?;
            Ok(Some(result))
        }
        JobSpec::BatchExtract {
            items,
            overwrite,
            symlinks,
            smart,
        } => {
            let display_items = match display_spec {
                JobSpec::BatchExtract { items, .. } => items.as_slice(),
                _ => items.as_slice(),
            };
            let result = run_batch_extract_job(
                state,
                settings,
                bridge,
                events,
                snapshots,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                items,
                display_items,
                overwrite,
                symlinks,
                *smart,
            )?;
            Ok(Some(result))
        }
        JobSpec::ExtractNested {
            outer_path,
            entry_path,
            dest,
            overwrite,
            symlinks,
            smart,
            encoding,
            password,
            best_effort,
        } => {
            let outer = PathBuf::from(outer_path);
            let dest = PathBuf::from(dest);
            let outer_display_path = match display_spec {
                JobSpec::ExtractNested { outer_path, .. } => outer_path.as_str(),
                _ => outer_path.as_str(),
            };
            let outer_display_name = batch_archive_label(Path::new(outer_display_path));
            let display_name = entry_path
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(entry_path);
            let workspace = create_nested_job_workspace()?;
            let limits = settings.safety_limits();
            let temp = with_gui_password(
                state,
                bridge,
                &**events,
                snapshots,
                ctl,
                cancel_flag,
                gui_id,
                &outer,
                Some(&outer_display_name),
                password.as_deref(),
                |resolved_password| {
                    extract_nested_archive_to_temp_for_job(
                        state,
                        &outer,
                        entry_path,
                        resolved_password,
                        encoding.as_deref(),
                        workspace.path(),
                        limits,
                        sink,
                        ctl,
                    )
                },
            )?;
            let temp_path = temp.to_path_buf();
            let extraction = run_extract_archive_job(
                state,
                settings,
                bridge,
                events,
                snapshots,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                &temp_path,
                display_name,
                &dest,
                None,
                None,
                None,
                overwrite,
                symlinks,
                *smart,
                None,
                None,
                false,
                *best_effort,
            );
            state.forget_password(&temp_path);
            let physical = temp_path.to_string_lossy();
            let (result, _) = extraction.map_err(|error| {
                redact_format_error_path(error, physical.as_ref(), display_name)
            })?;
            Ok(Some(result))
        }
        JobSpec::Test {
            path,
            encoding,
            password,
        } => {
            let archive = PathBuf::from(path);
            let display_path = match display_spec {
                JobSpec::Test { path, .. } => path.as_str(),
                _ => path.as_str(),
            };
            let display_name = batch_archive_label(Path::new(display_path));
            let outcome = with_gui_password(
                state,
                bridge,
                &**events,
                snapshots,
                ctl,
                cancel_flag,
                gui_id,
                &archive,
                Some(&display_name),
                password.as_deref(),
                |pw| {
                    let open = OpenOptions {
                        password: pw.cloned(),
                        encoding_override: encoding.clone(),
                    };
                    state
                        .engine
                        .test_summary_with_structure(&archive, &open, sink, ctl)
                },
            )?;
            let structure = outcome.structure;
            let report = outcome.into_summary();
            let ok = report.is_ok();
            let entries_tested = report.entries_tested;
            let problems_total = report.problems.total;
            let problems_truncated = report.problems.is_truncated();
            let problems = report.problems.messages;
            let mut result = serde_json::json!({
                "ok": ok,
                "entries": entries_tested,
                "entries_tested": entries_tested,
                "problems": problems,
                "problems_total": problems_total,
                "problems_truncated": problems_truncated,
            });
            if !structure.is_complete() {
                result["structure"] = serde_json::json!(structure.id());
            }
            Ok(Some(result))
        }
        JobSpec::Convert {
            src,
            dest,
            src_encoding,
            src_password,
            replace_existing,
            replacement_guard,
            ..
        } => {
            let src_path = PathBuf::from(src);
            let display_src = match display_spec {
                JobSpec::Convert { src, .. } => src.as_str(),
                _ => src.as_str(),
            };
            let display_name = batch_archive_label(Path::new(display_src));
            let create = convert_create_options(spec, settings)?;
            let commit_policy =
                job_output_commit_policy(*replace_existing, *replacement_guard, "convert")?;
            let report = with_gui_password(
                state,
                bridge,
                &**events,
                snapshots,
                ctl,
                cancel_flag,
                gui_id,
                &src_path,
                Some(&display_name),
                src_password.as_deref(),
                |pw| {
                    let open = OpenOptions {
                        password: pw.cloned(),
                        encoding_override: src_encoding.clone(),
                    };
                    state.engine.convert_with_report_policy(
                        &src_path,
                        Path::new(dest),
                        &open,
                        &create,
                        commit_policy,
                        sink,
                        ctl,
                    )
                },
            )?;
            Ok(Some(create_report_result(
                report,
                "convert",
                SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, 0),
                None,
            )))
        }
        JobSpec::ExportSqz {
            src,
            dest,
            level,
            dest_password,
            replace_existing,
            replacement_guard,
        } => {
            let src_path = PathBuf::from(src);
            let dest_path = PathBuf::from(dest);
            if !is_sqz_archive_path(&src_path) {
                return Err(FormatError::Unsupported(
                    "export expects a .sqz source container".into(),
                ));
            }
            if is_sqz_archive_path(&dest_path) {
                return Err(FormatError::Unsupported(
                    "export output must be a standard archive, not .sqz".into(),
                ));
            }
            let create = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                password: dest_password.as_deref().map(Password::new),
                resources: settings.resource_options(),
                ..CreateOptions::default()
            };
            let commit_policy =
                job_output_commit_policy(*replace_existing, *replacement_guard, "export")?;
            state.engine.convert_with_policy(
                &src_path,
                &dest_path,
                &OpenOptions::default(),
                &create,
                commit_policy,
                sink,
                ctl,
            )?;
            Ok(Some(serde_json::json!({
                "dest": dest_path.to_string_lossy(),
            })))
        }
        JobSpec::RepairSqz { src, dest, level } => {
            let src_path = PathBuf::from(src);
            let dest_path = PathBuf::from(dest);
            if !is_sqz_archive_path(&src_path) {
                return Err(FormatError::Unsupported(
                    "SQZ repair expects a .sqz source container".into(),
                ));
            }
            if !is_plain_sqz_path(&dest_path) {
                return Err(FormatError::Unsupported(
                    "SQZ repair output must be a .sqz container".into(),
                ));
            }
            let create = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                resources: settings.resource_options(),
                ..CreateOptions::default()
            };
            let test_report =
                state
                    .engine
                    .test_summary(&src_path, &OpenOptions::default(), sink, ctl)?;
            if !test_report.is_ok() {
                let detail = if test_report.problems.messages.is_empty() {
                    "archive integrity test failed".to_owned()
                } else {
                    test_report.problems.messages.join("; ")
                };
                return Err(FormatError::CorruptArchive(detail));
            }
            let in_place = state.engine.convert_with_atomic_replace(
                &src_path,
                &dest_path,
                &OpenOptions::default(),
                &create,
                sink,
                ctl,
            )?;
            Ok(Some(serde_json::json!({
                "dest": dest_path.to_string_lossy(),
                "in_place": in_place,
                "recovery": test_report.recovery.as_ref().map(recovery_summary_json),
            })))
        }
        JobSpec::RepairZip { src, dest, level } => {
            let src_path = PathBuf::from(src);
            let dest_path = PathBuf::from(dest);
            if !is_zip_family_path(&src_path) {
                return Err(FormatError::Unsupported(
                    "ZIP index rebuild expects a ZIP-family source archive".into(),
                ));
            }
            if !is_zip_family_path(&dest_path) {
                return Err(FormatError::Unsupported(
                    "ZIP index rebuild output must be a ZIP-family archive".into(),
                ));
            }
            let source_test = state.engine.test_summary_with_structure(
                &src_path,
                &OpenOptions::default(),
                sink,
                ctl,
            )?;
            if !source_test.payload_is_ok() {
                return Err(FormatError::CorruptArchive(
                    source_test.summary.problems.messages.join("; "),
                ));
            }
            let source_entries = source_test.summary.entries_tested;
            let create = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                resources: settings.resource_options(),
                ..CreateOptions::default()
            };
            let in_place = state.engine.convert_with_atomic_replace(
                &src_path,
                &dest_path,
                &OpenOptions::default(),
                &create,
                sink,
                ctl,
            )?;
            Ok(Some(serde_json::json!({
                "operation": "repair_zip",
                "tool": "zip-local-header-rebuild",
                "dest": dest_path.to_string_lossy(),
                "in_place": in_place,
                "source_entries": source_entries,
            })))
        }
        JobSpec::Protect {
            path,
            redundancy,
            recovery,
        } => {
            let archive = PathBuf::from(path);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let sources = state.engine.recovery_protect_sources(&archive)?;
            let report = squallz_recovery::protect_files_controlled(
                &archive,
                *redundancy,
                recovery.as_deref(),
                &sources,
                sink,
                ctl,
            )?;
            finish_protect_report(report)
        }
        JobSpec::VerifyRecovery { path, recovery } => {
            let archive = PathBuf::from(path);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let report =
                squallz_recovery::verify_controlled(&archive, recovery.as_deref(), sink, ctl)?;
            recovery_report_json(report)
        }
        JobSpec::RepairRecovery {
            path,
            output,
            output_directory,
            recovery,
        } => {
            let archive = PathBuf::from(path);
            let output = output.as_deref().map(PathBuf::from);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let report = if *output_directory {
                let directory = output.as_deref().ok_or_else(|| {
                    FormatError::Unsupported(
                        "PAR2 directory repair requires an output directory".into(),
                    )
                })?;
                squallz_recovery::repair_to_directory_controlled(
                    &archive,
                    directory,
                    recovery.as_deref(),
                    sink,
                    ctl,
                )?
            } else {
                squallz_recovery::repair_controlled(
                    &archive,
                    output.as_deref(),
                    recovery.as_deref(),
                    sink,
                    ctl,
                )?
            };
            recovery_report_json(report)
        }
        JobSpec::Update {
            path,
            add,
            delete,
            rename,
            mkdir,
            excludes,
            content_policy,
            password,
            level,
        } => {
            let archive = PathBuf::from(path);
            let mut ops = Vec::new();
            for src in add {
                let src = PathBuf::from(src);
                let dest = path_file_name_or_empty(&src);
                ops.push(UpdateOp::Add {
                    src,
                    dest: EntryPath::from_utf8(dest),
                });
            }
            for dir in mkdir {
                ops.push(UpdateOp::AddDir {
                    path: EntryPath::from_utf8(dir.clone()),
                });
            }
            for pattern in delete {
                ops.push(UpdateOp::Delete {
                    pattern: pattern.clone(),
                });
            }
            for item in rename {
                ops.push(UpdateOp::Rename {
                    from: EntryPath::from_utf8(item.from.clone()),
                    to: EntryPath::from_utf8(item.to.clone()),
                });
            }
            if ops.is_empty() {
                return Err(FormatError::Unsupported(
                    "no archive update operations".into(),
                ));
            }
            let opts = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                password: password.as_deref().map(Password::new),
                resources: settings.resource_options(),
                excludes: content_policy.resolve_excludes(excludes),
                ..CreateOptions::default()
            };
            state.engine.update(&archive, &ops, &opts, sink, ctl)?;
            Ok(Some(serde_json::json!({
                "archive": archive.to_string_lossy(),
                "operations": ops.len(),
            })))
        }
        JobSpec::Checksum {
            inputs,
            excludes,
            algorithm,
        } => {
            if inputs.is_empty() {
                return Err(FormatError::Unsupported(
                    "checksum needs at least one input".into(),
                ));
            }
            let algorithm = *algorithm;
            ctl.checkpoint()?;
            sink.on_progress(0, 0, &EntryPath::from_utf8("Computing checksums"));
            let inputs = inputs.iter().map(PathBuf::from).collect::<Vec<_>>();
            let report = state
                .engine
                .checksum_files_with_progress(&inputs, excludes, algorithm, sink, ctl)?;
            ctl.checkpoint()?;
            Ok(Some(serde_json::json!({
                "ok": true,
                "operation": "checksum",
                "algorithm": report.algorithm.id(),
                "input_count": report.input_count,
                "entries_scanned": report.entries_scanned,
                "files_hashed": report.files_hashed,
                "bytes_hashed": report.bytes_hashed,
                "items": report.items.iter().map(|item| serde_json::json!({
                    "path": item.path.to_string_lossy().into_owned(),
                    "size": item.size,
                    "digest": &item.digest,
                })).collect::<Vec<_>>(),
            })))
        }
        JobSpec::ChecksumCheck {
            manifest,
            algorithm,
        } => {
            if manifest.trim().is_empty() {
                return Err(FormatError::Unsupported(
                    "checksum verification needs a manifest".into(),
                ));
            }
            let algorithm = *algorithm;
            ctl.checkpoint()?;
            sink.on_progress(0, 0, &EntryPath::from_utf8("Verifying checksum manifest"));
            let report = state.engine.verify_checksum_manifest_with_progress(
                Path::new(manifest),
                algorithm,
                sink,
                ctl,
            )?;
            ctl.checkpoint()?;
            Ok(Some(serde_json::json!({
                "ok": report.is_ok(),
                "operation": "checksum_check",
                "algorithm": report.algorithm.id(),
                "manifest": report.manifest.to_string_lossy().into_owned(),
                "checked": report.checked,
                "passed": report.passed,
                "failed": report.failed,
                "bytes_hashed": report.bytes_hashed,
                "items": report.items.iter().map(|item| serde_json::json!({
                    "path": item.path.to_string_lossy().into_owned(),
                    "expected": &item.expected,
                    "actual": &item.actual,
                    "ok": item.ok,
                    "error": &item.error,
                })).collect::<Vec<_>>(),
            })))
        }
        JobSpec::DuplicateScan {
            inputs,
            excludes,
            min_size,
        } => {
            if inputs.is_empty() {
                return Err(FormatError::Unsupported(
                    "duplicate scan needs at least one input".into(),
                ));
            }
            ctl.checkpoint()?;
            sink.on_progress(0, 0, &EntryPath::from_utf8("Scanning duplicate candidates"));
            let inputs = inputs.iter().map(PathBuf::from).collect::<Vec<_>>();
            let report = state
                .engine
                .find_duplicate_files(&inputs, excludes, *min_size)?;
            ctl.checkpoint()?;
            Ok(Some(serde_json::json!({
                "operation": "duplicates",
                "hash_algorithm": "blake3",
                "input_count": report.input_count,
                "entries_scanned": report.entries_scanned,
                "files_scanned": report.files_scanned,
                "bytes_scanned": report.bytes_scanned,
                "min_size": min_size,
                "candidate_files": report.candidate_files,
                "hashed_bytes": report.hashed_bytes,
                "duplicate_groups": report.duplicate_groups(),
                "duplicate_files": report.duplicate_files(),
                "reclaimable_bytes": report.reclaimable_bytes(),
                "groups": report.groups.iter().map(|group| serde_json::json!({
                    "hash": group.hash,
                    "hash_algorithm": "blake3",
                    "size": group.size,
                    "count": group.count(),
                    "reclaimable_bytes": group.reclaimable_bytes(),
                    "paths": group.paths.iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })))
        }
    }
}

fn recovery_summary_json(summary: &RecoverySummary) -> serde_json::Value {
    serde_json::json!({
        "scheme": &summary.scheme,
        "block_size": summary.block_size,
        "total_blocks": summary.total_blocks,
        "data_shards": summary.data_shards,
        "parity_shards": summary.parity_shards,
        "recovery_blocks_available": summary.recovery_blocks_available,
        "damaged_blocks": summary.damaged_blocks,
        "repaired_blocks": summary.repaired_blocks,
        "unrepaired_blocks": summary.unrepaired_blocks,
        "repair_possible": summary.repair_possible,
    })
}

fn recovery_report_json(
    report: squallz_recovery::RecoveryReport,
) -> Result<Option<serde_json::Value>, FormatError> {
    serde_json::to_value(report)
        .map(Some)
        .map_err(|e| FormatError::Other(format!("cannot serialize recovery report: {e}")))
}

fn finish_protect_report(
    report: squallz_recovery::RecoveryReport,
) -> Result<Option<serde_json::Value>, FormatError> {
    if report.ok {
        return recovery_report_json(report);
    }

    let detail = if report.stderr.is_empty() {
        format!(
            "PAR2 {} failed with status {}",
            report.operation,
            status_code_label(report.status_code)
        )
    } else {
        report.stderr
    };
    Err(FormatError::Other(detail))
}

/// Expands a display-path selection against the entry list: items ending
/// with `/` select by prefix (whole directories), others match exactly.
#[cfg(test)]
pub(crate) fn expand_selection(entries: &[EntryMeta], selection: &[String]) -> Vec<EntryPath> {
    entries
        .iter()
        .filter(|entry| selection_matches(entry, selection))
        .map(|e| e.path.clone())
        .collect()
}

pub(crate) fn expand_selection_with_control(
    entries: &[EntryMeta],
    selection: &[String],
    control: &ControlToken,
) -> Result<Vec<EntryPath>, FormatError> {
    let mut expanded = Vec::new();
    for entry in entries {
        control.checkpoint()?;
        let display = crate::state::normalized_entry_path(entry);
        let mut matched = false;
        for selected in selection {
            control.checkpoint()?;
            matched = if selected.ends_with('/') {
                display.starts_with(selected.as_str())
            } else {
                display == *selected
            };
            if matched {
                break;
            }
        }
        if matched {
            expanded.push(entry.path.clone());
        }
    }
    control.checkpoint()?;
    Ok(expanded)
}

#[cfg(test)]
fn selection_matches(entry: &EntryMeta, selection: &[String]) -> bool {
    let display = crate::state::normalized_entry_path(entry);
    selection.iter().any(|selected| {
        if selected.ends_with('/') {
            display.starts_with(selected.as_str())
        } else {
            display == *selected
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{
        BatchExtractItem, ExternalTaskActionDto, JobSpec, PERFORMANCE_STREAM_BUFFER_MAX_BYTES,
    };
    use crate::preview_sessions::PreviewSessionManager;
    use squallz_core::ChecksumAlgorithm;
    use std::io::Write as _;
    use std::sync::Mutex as StdMutex;

    #[cfg(unix)]
    static EXTERNAL_TOOL_ENV_LOCK: StdMutex<()> = StdMutex::new(());

    #[cfg(unix)]
    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    #[cfg(unix)]
    impl EnvRestore {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// Buffering event sink for tests.
    #[derive(Default)]
    struct TestSink {
        events: StdMutex<Vec<(String, serde_json::Value)>>,
    }

    impl EventSink for TestSink {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            self.events
                .lock()
                .unwrap()
                .push((event.to_owned(), payload));
        }
    }

    #[derive(Default)]
    struct RecordingProgressSink {
        paths: StdMutex<Vec<String>>,
    }

    impl ProgressSink for RecordingProgressSink {
        fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
            self.paths.lock().unwrap().push(current.display.clone());
        }

        fn on_scan_progress(&self, _scanned_entries: u64, current: &EntryPath) {
            self.paths.lock().unwrap().push(current.display.clone());
        }
    }

    #[test]
    fn scheduler_limits_and_cpu_reservations_are_conservative() {
        assert_eq!(resolved_parallel_job_limit(None, 1), 1);
        assert_eq!(resolved_parallel_job_limit(None, 4), 1);
        assert_eq!(resolved_parallel_job_limit(None, 8), 2);
        assert_eq!(resolved_parallel_job_limit(None, 16), 4);
        assert_eq!(resolved_parallel_job_limit(Some(0), 16), 1);
        assert_eq!(resolved_parallel_job_limit(Some(99), 16), 8);

        let state = AppState::new();
        let zip = JobSpec::RepairZip {
            src: "broken.zip".into(),
            dest: "repaired.zip".into(),
            level: 6,
        };
        let zstd = compress_file_job(Path::new("input.bin"), Path::new("archive.tar.zst"));
        let wim = compress_file_job(Path::new("input.bin"), Path::new("archive.wim"));
        let sqz_7z = compress_file_job_with_inner_format(
            Path::new("input.bin"),
            Path::new("archive.sqz"),
            Some(squallz_core::SqzInnerFormat::SevenZip),
        );
        let recovery = JobSpec::Protect {
            path: "archive.zip".into(),
            redundancy: 10,
            recovery: None,
        };
        let light = JobSpec::Checksum {
            inputs: vec!["input.bin".into()],
            excludes: Vec::new(),
            algorithm: ChecksumAlgorithm::Blake3,
        };
        let automatic = SettingsDto::default();
        let manual = SettingsDto {
            performance_threads: Some(3),
            ..SettingsDto::default()
        };

        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &zip), &automatic, 8),
            JobResources::new(1)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &light), &automatic, 8),
            JobResources::new(1)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &zstd), &automatic, 8),
            JobResources::new(8)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &zstd), &manual, 8),
            JobResources::new(3)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &wim), &automatic, 8),
            JobResources::new(8)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &sqz_7z), &automatic, 8),
            JobResources::new(1)
        );
        assert_eq!(
            scheduler_resources(scheduler_cpu_profile(&state.engine, &recovery), &manual, 8),
            JobResources::new(8)
        );

        let execution = settings_for_job_execution(
            automatic.clone(),
            scheduler_cpu_profile(&state.engine, &zstd),
            JobResources::new(8),
        );
        assert_eq!(execution.resource_options().threads, Some(8));
        let serial_execution = settings_for_job_execution(
            manual.clone(),
            scheduler_cpu_profile(&state.engine, &zip),
            JobResources::new(1),
        );
        assert_eq!(serial_execution.resource_options().threads, Some(3));

        let custom_buffer = SettingsDto {
            performance_memory_limit_bytes: Some(512 * 1024 * 1024),
            ..SettingsDto::default()
        };
        assert_eq!(
            job_stream_buffer_limit_bytes(&zip, &custom_buffer),
            Some(PERFORMANCE_STREAM_BUFFER_MAX_BYTES)
        );
        assert_eq!(job_stream_buffer_limit_bytes(&light, &custom_buffer), None);
        assert_eq!(
            job_stream_buffer_limit_bytes(&zip, &SettingsDto::default()),
            None
        );
        assert!(job_supports_pause(&light));
        assert!(!job_supports_pause(&JobSpec::Protect {
            path: "archive.zip".into(),
            redundancy: 10,
            recovery: None,
        }));
        assert!(!job_supports_pause(&JobSpec::VerifyRecovery {
            path: "archive.zip".into(),
            recovery: None,
        }));
        assert!(!job_supports_pause(&JobSpec::RepairRecovery {
            path: "archive.zip".into(),
            output: None,
            output_directory: false,
            recovery: None,
        }));
    }

    #[test]
    fn extract_result_keeps_problem_preview_separate_from_core_counts() {
        let destination = PathBuf::from("output/archive");
        let result = extract_result_json(
            ExtractPlan {
                requested_destination: PathBuf::from("output"),
                destination: destination.clone(),
                layout: squallz_core::SmartLayout::WrapInFolder,
                scope: squallz_core::ExtractScope {
                    entries: 9,
                    files: 5,
                    directories: 1,
                    symlinks: 1,
                    hardlinks: 1,
                    other: 1,
                    total_bytes: 8192,
                },
                estimated_conflicts: 4,
            },
            ExtractReport {
                destination: destination.clone(),
                selected_entries: 9,
                created: 2,
                directories: 1,
                skipped: 3,
                replaced: 1,
                renamed: 1,
                failed: 1,
                output_bytes: 4096,
            },
            ArchiveStructureStatus::Complete,
            true,
            ProblemPreview {
                total: 1,
                messages: vec!["broken.txt: invalid data".to_owned()],
            },
        );

        assert_eq!(result["dest"], destination.to_string_lossy().as_ref());
        assert!(result.get("skipped").is_none());
        assert_eq!(result["problems"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["problems_total"], 1);
        assert_eq!(result["problems_truncated"], false);
        assert_eq!(result["plan"]["estimated_conflicts"], 4);
        assert_eq!(result["counts"]["destination"], result["dest"]);
        assert_eq!(result["counts"]["selected_entries"], 9);
        assert_eq!(result["counts"]["created"], 2);
        assert_eq!(result["counts"]["directories"], 1);
        assert_eq!(result["counts"]["skipped"], 3);
        assert_eq!(result["counts"]["replaced"], 1);
        assert_eq!(result["counts"]["renamed"], 1);
        assert_eq!(result["counts"]["failed"], 1);
        assert_eq!(result["counts"]["output_bytes"], 4096);
    }

    #[test]
    fn private_path_redaction_keeps_the_original_error_category() {
        let private = "/private/squallz-preview/inner.zip";
        let error = redact_format_error_path(
            FormatError::CorruptArchive(format!("invalid footer in {private}")),
            private,
            "inner.zip",
        );

        match error {
            FormatError::CorruptArchive(detail) => {
                assert_eq!(detail, "invalid footer in inner.zip");
            }
            other => panic!("expected corrupt archive, got {other:?}"),
        }

        let error = redact_format_error_path(
            FormatError::Io(io::Error::new(
                io::ErrorKind::StorageFull,
                format!("no space left while writing {private}"),
            )),
            private,
            "inner.zip",
        );
        assert!(matches!(error, FormatError::DiskFull));
    }

    #[test]
    fn output_conflict_redaction_keeps_the_contextual_marker() {
        let private = "/private/squallz-preview/archive.repaired.zip";
        let error = redact_format_error_path(
            FormatError::output_exists(private),
            private,
            "archive.repaired.zip",
        );

        assert!(error.is_output_exists());
        assert_eq!(
            error.output_exists_path(),
            Some(Path::new("archive.repaired.zip"))
        );
        assert!(!error.to_string().contains("/private/squallz-preview"));
        assert_eq!(ErrorDto::from_engine(&error).key, "error.output_exists");
    }

    #[derive(Default)]
    struct FakeTrashAdapter {
        fail_names: Vec<String>,
        calls: StdMutex<Vec<PathBuf>>,
    }

    impl FakeTrashAdapter {
        fn failing(names: &[&str]) -> Self {
            Self {
                fail_names: names.iter().map(|name| (*name).to_owned()).collect(),
                calls: StdMutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<PathBuf> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TrashAdapter for FakeTrashAdapter {
        fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            let name = path
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            if self.fail_names.contains(&name) {
                Err(TrashError)
            } else {
                let removed = match fs::symlink_metadata(path) {
                    Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                        fs::remove_dir_all(path)
                    }
                    Ok(_) => fs::remove_file(path),
                    Err(error) => Err(error),
                };
                removed.map_err(|_| TrashError)
            }
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-jobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_password_protected_zip(dir: &Path, state: &AppState) -> PathBuf {
        let src = dir.join("secret-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("secret.txt"), b"window ownership fixture").unwrap();
        let archive = dir.join("secret.zip");
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&src),
                &CreateOptions {
                    password: Some(Password::new("secret")),
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        archive
    }

    fn password_test_job(archive: &Path) -> JobSpec {
        JobSpec::Test {
            path: archive.to_string_lossy().into_owned(),
            encoding: None,
            password: None,
        }
    }

    fn checksum_job(input: &Path) -> JobSpec {
        JobSpec::Checksum {
            inputs: vec![input.to_string_lossy().into_owned()],
            excludes: Vec::new(),
            algorithm: ChecksumAlgorithm::Sha256,
        }
    }

    fn compress_file_job(input: &Path, output: &Path) -> JobSpec {
        compress_file_job_with_inner_format(input, output, None)
    }

    fn compress_file_job_with_inner_format(
        input: &Path,
        output: &Path,
        sqz_inner_format: Option<squallz_core::SqzInnerFormat>,
    ) -> JobSpec {
        JobSpec::Compress {
            inputs: vec![input.to_string_lossy().into_owned()],
            dest: output.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: Vec::new(),
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum CleanupHolderEvent {
        Sync(PathBuf),
        Remove(PathBuf),
    }

    #[test]
    fn cleanup_holder_is_synced_before_staging_can_continue() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-holder-sync-order")).unwrap();
        let source = dir.join("source.txt");
        fs::write(&source, b"source").unwrap();
        let events = std::cell::RefCell::new(Vec::new());

        let holder = create_cleanup_staging_dir_with(
            &source,
            |path| {
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Sync(path.to_path_buf()));
                Ok(())
            },
            |path, identity| {
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Remove(path.to_path_buf()));
                remove_empty_holder_if_identity(path, identity)
            },
        )
        .unwrap();

        assert_eq!(
            events.into_inner(),
            vec![
                CleanupHolderEvent::Sync(holder.clone()),
                CleanupHolderEvent::Sync(dir.clone()),
            ]
        );
        assert!(source.exists());
        fs::remove_dir(&holder).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_holder_sync_failure_removes_only_the_created_identity() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-holder-sync-failure")).unwrap();
        let source = dir.join("source.txt");
        fs::write(&source, b"source").unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let sync_calls = std::cell::Cell::new(0usize);

        let error = create_cleanup_staging_dir_with(
            &source,
            |path| {
                let call = sync_calls.get();
                sync_calls.set(call + 1);
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Sync(path.to_path_buf()));
                if call == 0 {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected holder sync failure",
                    ))
                } else {
                    Ok(())
                }
            },
            |path, identity| {
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Remove(path.to_path_buf()));
                remove_empty_holder_if_identity(path, identity)
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let events = events.into_inner();
        let holder = match &events[0] {
            CleanupHolderEvent::Sync(path) => path.clone(),
            CleanupHolderEvent::Remove(_) => panic!("holder sync must be first"),
        };
        assert_eq!(
            events,
            vec![
                CleanupHolderEvent::Sync(holder.clone()),
                CleanupHolderEvent::Remove(holder.clone()),
                CleanupHolderEvent::Sync(dir.clone()),
            ]
        );
        assert!(source.exists());
        assert!(!holder.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_parent_sync_failure_removes_holder_and_resyncs_parent() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-parent-sync-failure")).unwrap();
        let source = dir.join("source.txt");
        fs::write(&source, b"source").unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let failed_parent_sync = std::cell::Cell::new(false);

        let error = create_cleanup_staging_dir_with(
            &source,
            |path| {
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Sync(path.to_path_buf()));
                if path == dir && !failed_parent_sync.replace(true) {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected parent sync failure",
                    ))
                } else {
                    Ok(())
                }
            },
            |path, identity| {
                events
                    .borrow_mut()
                    .push(CleanupHolderEvent::Remove(path.to_path_buf()));
                remove_empty_holder_if_identity(path, identity)
            },
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let events = events.into_inner();
        let holder = match &events[0] {
            CleanupHolderEvent::Sync(path) => path.clone(),
            CleanupHolderEvent::Remove(_) => panic!("holder sync must be first"),
        };
        assert_eq!(
            events,
            vec![
                CleanupHolderEvent::Sync(holder.clone()),
                CleanupHolderEvent::Sync(dir.clone()),
                CleanupHolderEvent::Remove(holder.clone()),
                CleanupHolderEvent::Sync(dir.clone()),
            ]
        );
        assert!(source.exists());
        assert!(!holder.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    struct PreparedCleanup {
        plan: SourceCleanupPlan,
        archived_inputs: Vec<CreateInputManifestEntry>,
    }

    fn collect_test_input_manifest(
        path: &Path,
        manifest: &mut BTreeMap<PathBuf, CreateInputManifestEntry>,
    ) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        let source_path = if metadata.file_type().is_symlink() {
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "test symlink has no parent")
            })?;
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "test symlink has no name")
            })?;
            fs::canonicalize(parent)?.join(name)
        } else {
            fs::canonicalize(path)?
        };
        let entry_type = if metadata.file_type().is_symlink() {
            squallz_core::api::EntryType::Symlink {
                target: fs::read_link(path)?
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes(),
            }
        } else if metadata.is_dir() {
            squallz_core::api::EntryType::Dir
        } else if metadata.is_file() {
            squallz_core::api::EntryType::File
        } else {
            squallz_core::api::EntryType::Other
        };
        let bytes = if metadata.is_file() {
            Some(fs::read(path)?)
        } else {
            None
        };
        #[cfg(unix)]
        let unix_mode = Some(metadata.mode());
        #[cfg(not(unix))]
        let unix_mode = None;
        manifest
            .entry(source_path.clone())
            .or_insert_with(|| CreateInputManifestEntry {
                source_path,
                archive_path: EntryPath::from_utf8(path.to_string_lossy().into_owned()),
                entry_type,
                size: bytes.as_ref().map_or(0, |bytes| bytes.len() as u64),
                modified: metadata.modified().ok().map(CreateInputModifiedTime::from),
                unix_mode,
                blake3: bytes.as_ref().map(|bytes| *blake3::hash(bytes).as_bytes()),
            });
        if metadata.is_dir() {
            let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|entry| entry.file_name());
            for child in children {
                collect_test_input_manifest(&child.path(), manifest)?;
            }
        }
        Ok(())
    }

    fn prepare_cleanup(
        inputs: &[PathBuf],
        action: PostSuccessAction,
        excludes: &[String],
    ) -> PreparedCleanup {
        let plan = prepare_source_cleanup(
            inputs,
            action,
            excludes,
            &|| false,
            &squallz_core::api::NoProgress,
        )
        .unwrap();
        let mut archived_inputs = BTreeMap::new();
        for input in inputs {
            let _ = collect_test_input_manifest(input, &mut archived_inputs);
        }
        PreparedCleanup {
            plan,
            archived_inputs: archived_inputs.into_values().collect(),
        }
    }

    fn finish_cleanup(
        prepared: PreparedCleanup,
        outputs: &[PathBuf],
        trash_adapter: &dyn TrashAdapter,
        is_cancelled: &dyn Fn() -> bool,
    ) -> SourceCleanupResult {
        let journal_root = outputs
            .first()
            .and_then(|path| path.parent())
            .unwrap_or_else(|| Path::new("."));
        let journal = SourceCleanupJournal::at_path(journal_root.join(format!(
            ".source-cleanup-test-{}-{}/journal.json",
            std::process::id(),
            TRASH_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )));
        complete_source_cleanup(
            prepared.plan,
            outputs,
            &prepared.archived_inputs,
            trash_adapter,
            &journal,
            is_cancelled,
            &squallz_core::api::NoProgress,
        )
    }

    fn deterministic_payload(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_u32;
        let mut bytes = Vec::with_capacity(len);
        for _ in 0..len {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bytes.push((state >> 24) as u8);
        }
        bytes
    }

    fn test_manifest_entry(
        source_path: PathBuf,
        archive_path: EntryPath,
    ) -> CreateInputManifestEntry {
        CreateInputManifestEntry {
            source_path,
            archive_path,
            entry_type: squallz_core::api::EntryType::Dir,
            size: 0,
            modified: None,
            unix_mode: None,
            blake3: None,
        }
    }

    #[test]
    fn archived_input_map_preserves_each_archive_path_for_a_repeated_source() {
        let source = PathBuf::from("source");
        let first_path = EntryPath::from_utf8("source");
        let second_path = EntryPath::from_utf8("nested/source");
        let archived = archived_input_map(&[
            test_manifest_entry(source.clone(), first_path.clone()),
            test_manifest_entry(source.clone(), second_path.clone()),
        ])
        .unwrap();

        assert_eq!(
            archived.get(&source).unwrap().archive_paths,
            vec![first_path, second_path]
        );
    }

    #[test]
    fn archived_input_map_rejects_distinct_sources_for_one_archive_path() {
        let archive_path = EntryPath::from_utf8("shared");
        let result = archived_input_map(&[
            test_manifest_entry(PathBuf::from("first"), archive_path.clone()),
            test_manifest_entry(PathBuf::from("second"), archive_path),
        ]);

        assert!(matches!(result, Err(FingerprintError::Unavailable)));
    }

    #[test]
    fn archived_input_map_rejects_inexact_archive_paths() {
        let inexact_paths = [
            EntryPath::from_raw(b"raw-name".to_vec(), "display-name".to_owned(), "utf-8"),
            EntryPath::from_raw(vec![0xff], "replacement-name".to_owned(), "utf-8"),
            EntryPath::from_raw(b"legacy-name".to_vec(), "legacy-name".to_owned(), "GBK"),
        ];

        for archive_path in inexact_paths {
            let result =
                archived_input_map(&[test_manifest_entry(PathBuf::from("source"), archive_path)]);
            assert!(matches!(result, Err(FingerprintError::Unavailable)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn archived_input_map_rejects_a_non_utf8_source_path() {
        use std::os::unix::ffi::OsStringExt;

        let source = PathBuf::from(std::ffi::OsString::from_vec(
            b"source-with-invalid-utf8-\xff".to_vec(),
        ));
        assert!(source.to_str().is_none());

        let result =
            archived_input_map(&[test_manifest_entry(source, EntryPath::from_utf8("source"))]);

        assert!(matches!(result, Err(FingerprintError::Unavailable)));
    }

    #[cfg(unix)]
    #[test]
    fn archived_entry_verification_rejects_a_lossy_symlink_target_match() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let dir = temp_dir("source-cleanup-non-utf8-symlink-target");
        let source = dir.join("source-link");
        let target = PathBuf::from(std::ffi::OsString::from_vec(
            b"target-with-invalid-utf8-\xff".to_vec(),
        ));
        assert!(target.to_str().is_none());
        symlink(&target, &source).unwrap();
        let metadata = fs::symlink_metadata(&source).unwrap();
        let expected = ArchivedSourceEntry {
            archive_paths: vec![EntryPath::from_utf8("source-link")],
            entry_type: squallz_core::api::EntryType::Symlink {
                target: target.to_string_lossy().into_owned().into_bytes(),
            },
            size: 0,
            modified: metadata.modified().ok().map(CreateInputModifiedTime::from),
            unix_mode: Some(metadata.mode()),
            blake3: None,
        };
        let mut verified_bytes = 0;

        let verified = verify_archived_entry(
            &source,
            &expected,
            &mut verified_bytes,
            0,
            &|| false,
            &squallz_core::api::NoProgress,
        )
        .unwrap();

        assert!(!verified);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_moves_deduplicated_top_level_inputs() {
        let dir = temp_dir("source-cleanup-complete");
        let source_dir = dir.join("source");
        let nested = source_dir.join("nested.txt");
        let second = dir.join("second.txt");
        let output = dir.join("archive.zip");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(&nested, b"nested").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let fake = FakeTrashAdapter::default();

        let plan = prepare_cleanup(
            &[source_dir.clone(), nested, source_dir, second],
            PostSuccessAction::TrashSource,
            &[],
        );
        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(
            result,
            SourceCleanupResult {
                status: SourceCleanupStatus::Completed,
                moved: 2,
                kept: 0,
                recovery_required: 0,
            }
        );
        assert_eq!(fake.calls().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_blocks_output_inside_source_before_trashing() {
        let dir = temp_dir("source-cleanup-blocked");
        let source = dir.join("source");
        let output = source.join("archive.zip");
        std::fs::create_dir_all(&source).unwrap();
        let plan = prepare_cleanup(&[source], PostSuccessAction::TrashSource, &[]);
        std::fs::write(&output, b"archive").unwrap();
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(
            result,
            SourceCleanupResult {
                status: SourceCleanupStatus::Blocked,
                moved: 0,
                kept: 1,
                recovery_required: 0,
            }
        );
        assert!(fake.calls().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_blocks_a_source_changed_after_preflight() {
        let dir = temp_dir("source-cleanup-changed");
        let source = dir.join("source.txt");
        let output = dir.join("archive.zip");
        std::fs::write(&source, b"before").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let plan = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        std::fs::write(&source, b"changed after create preflight").unwrap();
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(result.status, SourceCleanupStatus::Blocked);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"changed after create preflight"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_reports_failed_when_preflight_cannot_freeze_source() {
        let dir = temp_dir("source-cleanup-failed");
        let missing = dir.join("missing.txt");
        let output = dir.join("archive.zip");
        std::fs::write(&output, b"archive").unwrap();
        let fake = FakeTrashAdapter::default();

        let plan = prepare_cleanup(&[missing], PostSuccessAction::TrashSource, &[]);
        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(result.status, SourceCleanupStatus::Failed);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_cleanup_uses_the_writer_symlink_target_as_authority() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("source-cleanup-symlink-manifest");
        let first_target = dir.join("first.txt");
        let second_target = dir.join("second.txt");
        let source = dir.join("source-link");
        let output = dir.join("archive.zip");
        std::fs::write(&first_target, b"first").unwrap();
        std::fs::write(&second_target, b"second").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        symlink("first.txt", &source).unwrap();
        let mut prepared = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        let entry = prepared.archived_inputs.first_mut().unwrap();
        entry.entry_type = squallz_core::api::EntryType::Symlink {
            target: b"second.txt".to_vec(),
        };
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(prepared, &[output], &fake, &|| false);

        assert_eq!(result.status, SourceCleanupStatus::Blocked);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        assert_eq!(
            std::fs::read_link(&source).unwrap(),
            PathBuf::from("first.txt")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_blocks_a_directory_with_changed_child_content() {
        let dir = temp_dir("source-cleanup-directory-changed");
        let source = dir.join("source");
        let child = source.join("child.txt");
        let output = dir.join("archive.zip");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(&child, b"before").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let plan = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        std::fs::write(&child, b"changed after archive preparation").unwrap();
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(result.status, SourceCleanupStatus::Blocked);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        assert!(source.exists());
        assert_eq!(
            std::fs::read(&child).unwrap(),
            b"changed after archive preparation"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_is_blocked_when_archive_excludes_are_active() {
        let dir = temp_dir("source-cleanup-excludes");
        let source = dir.join("source");
        let output = dir.join("archive.zip");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("photo.jpg"), b"included").unwrap();
        std::fs::write(source.join("original.raw"), b"excluded").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let plan = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &["*.raw".to_owned()],
        );
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| false);

        assert_eq!(result.status, SourceCleanupStatus::Blocked);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        assert!(source.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_content_verification_rejects_different_bytes() {
        let dir = temp_dir("source-cleanup-content-fingerprint");
        let source = dir.join("source.bin");
        std::fs::write(&source, b"first payload").unwrap();
        let expected = *blake3::hash(b"first payload").as_bytes();

        std::fs::write(&source, b"other payload").unwrap();
        let mut verified_bytes = 0;
        let unchanged = verify_archived_file(
            &source,
            b"first payload".len() as u64,
            &expected,
            &mut verified_bytes,
            b"first payload".len() as u64,
            &|| false,
            &squallz_core::api::NoProgress,
        )
        .unwrap();

        assert!(!unchanged);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_restore_collision_preserves_the_verified_source_beside_it() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-restore-collision")).unwrap();
        let original = dir.join("source.txt");
        let holder = dir.join(".squallz-trash-hold-test");
        let staged_path = holder.join("source.txt");
        std::fs::create_dir(&holder).unwrap();
        std::fs::write(&original, b"verified source").unwrap();
        let record =
            SourceCleanupRecord::new(original.clone(), staged_path, holder.clone()).unwrap();
        let preserved = record.preserved_path().to_path_buf();
        let journal = SourceCleanupJournal::at_path(dir.join("config/source-cleanup.json"));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        std::fs::write(&original, b"late competitor").unwrap();
        let staged = StagedCleanupCandidate { pending };

        let outcome = restore_staged_cleanup(staged, false);

        assert_eq!(outcome, StagedRestoreOutcome::Preserved);
        assert_eq!(std::fs::read(&original).unwrap(), b"late competitor");
        assert_eq!(std::fs::read(preserved).unwrap(), b"verified source");
        assert!(!holder.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn job_manager_reports_interrupted_cleanup_to_each_window() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-startup-restore")).unwrap();
        let parent = dir.join("sources");
        let original = parent.join("source.txt");
        let holder = parent.join(".squallz-trash-hold-startup");
        let staged = holder.join("source.txt");
        fs::create_dir_all(&holder).unwrap();
        fs::write(&original, b"source").unwrap();
        let record = SourceCleanupRecord::new(original.clone(), staged, holder).unwrap();
        let journal = Arc::new(SourceCleanupJournal::at_path(
            dir.join("config/source-cleanup.json"),
        ));
        let pending = journal.begin(&record).unwrap();
        squallz_core::move_path_no_replace(&record.original, &record.staged).unwrap();
        pending.sync_after_stage().unwrap();
        drop(pending);

        let manager = JobManager::with_test_trash_adapter_and_journal(
            Arc::new(OperationAudit::memory()),
            Arc::new(FakeTrashAdapter::default()),
            Arc::clone(&journal),
        );

        assert_eq!(fs::read(&original).unwrap(), b"source");
        let notice = manager.source_cleanup_recovery().unwrap();
        assert_eq!(notice.generation, 1);
        assert_eq!(notice.status, "restored");
        assert_eq!(notice.path.as_deref(), original.to_str());
        assert_eq!(manager.source_cleanup_recovery(), Some(notice));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stage_collision_publishes_recovery_after_cached_none_or_notice() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-late-journal")).unwrap();
        let journal = Arc::new(SourceCleanupJournal::at_path(
            dir.join("config/source-cleanup.json"),
        ));
        let fake = Arc::new(FakeTrashAdapter::default());
        let manager = JobManager::with_test_trash_adapter_and_journal(
            Arc::new(OperationAudit::memory()),
            fake.clone(),
            Arc::clone(&journal),
        );
        assert_eq!(manager.source_cleanup_recovery(), None);

        for attempt in 1..=2 {
            let stale_parent = dir.join(format!("stale-{attempt}"));
            let stale_original = stale_parent.join("source.txt");
            let stale_holder = stale_parent.join(format!("{HOLDER_PREFIX}stale"));
            fs::create_dir_all(&stale_holder).unwrap();
            fs::write(&stale_original, b"stale source").unwrap();
            let stale_record = SourceCleanupRecord::new(
                stale_original.clone(),
                stale_holder.join("source.txt"),
                stale_holder,
            )
            .unwrap();
            let pending = journal.begin(&stale_record).unwrap();
            squallz_core::move_path_no_replace(&stale_record.original, &stale_record.staged)
                .unwrap();
            pending.sync_after_stage().unwrap();
            drop(pending);

            let source = dir.join(format!("source-{attempt}.txt"));
            let output = dir.join(format!("archive-{attempt}.zip"));
            fs::write(&source, b"new source").unwrap();
            fs::write(&output, b"archive").unwrap();
            let prepared = prepare_cleanup(
                std::slice::from_ref(&source),
                PostSuccessAction::TrashSource,
                &[],
            );
            let result = complete_source_cleanup_with_notice(
                prepared.plan,
                &[output],
                &prepared.archived_inputs,
                &*fake,
                &manager.source_cleanup_journal,
                Some(&manager.source_cleanup_recovery),
                &|| false,
                &squallz_core::api::NoProgress,
            );

            assert_eq!(result.status, SourceCleanupStatus::Failed);
            assert!(source.exists());
            assert!(fake.calls().is_empty());
            let notice = manager.source_cleanup_recovery().unwrap();
            assert_eq!(notice.generation, attempt);
            assert_eq!(notice.status, "restored");
            assert_eq!(notice.path.as_deref(), stale_original.to_str());
            assert_eq!(manager.source_cleanup_recovery(), Some(notice));
        }

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_cleanup_lock_blocks_trash_and_reports_busy() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-manager-busy")).unwrap();
        let active_parent = dir.join("active");
        let active_original = active_parent.join("active.txt");
        let active_holder = active_parent.join(".squallz-trash-hold-active");
        fs::create_dir_all(&active_holder).unwrap();
        fs::write(&active_original, b"active").unwrap();
        let active_record = SourceCleanupRecord::new(
            active_original,
            active_holder.join("active.txt"),
            active_holder,
        )
        .unwrap();
        let journal = Arc::new(SourceCleanupJournal::at_path(
            dir.join("config/source-cleanup.json"),
        ));
        let pending = journal.begin(&active_record).unwrap();
        let fake = Arc::new(FakeTrashAdapter::default());
        let manager = JobManager::with_test_trash_adapter_and_journal(
            Arc::new(OperationAudit::memory()),
            fake.clone(),
            Arc::clone(&journal),
        );

        let source = dir.join("source.txt");
        let output = dir.join("archive.zip");
        fs::write(&source, b"source").unwrap();
        fs::write(&output, b"archive").unwrap();
        let prepared = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        let result = complete_source_cleanup(
            prepared.plan,
            &[output],
            &prepared.archived_inputs,
            &*fake,
            &manager.source_cleanup_journal,
            &|| false,
            &squallz_core::api::NoProgress,
        );

        assert_eq!(result.status, SourceCleanupStatus::Failed);
        assert!(source.exists());
        assert!(fake.calls().is_empty());
        let busy = manager.source_cleanup_recovery().unwrap();
        assert_eq!(busy.generation, 1);
        assert_eq!(busy.status, "busy");
        assert_eq!(manager.source_cleanup_recovery(), Some(busy));

        drop(pending);
        let cleared = manager.source_cleanup_recovery().unwrap();
        assert_eq!(cleared.generation, 2);
        assert_eq!(cleared.status, "cleared");
        assert!(active_record.original.exists());
        assert!(!active_record.holder.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_cleanup_journal_blocks_trash_and_requests_attention() {
        let dir = fs::canonicalize(temp_dir("source-cleanup-manager-corrupt")).unwrap();
        let journal_path = dir.join("config/source-cleanup.json");
        fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
        fs::write(&journal_path, b"{\"version\":1").unwrap();
        let journal = Arc::new(SourceCleanupJournal::at_path(journal_path.clone()));
        let fake = Arc::new(FakeTrashAdapter::default());
        let manager = JobManager::with_test_trash_adapter_and_journal(
            Arc::new(OperationAudit::memory()),
            fake.clone(),
            Arc::clone(&journal),
        );
        let source = dir.join("source.txt");
        let output = dir.join("archive.zip");
        fs::write(&source, b"source").unwrap();
        fs::write(&output, b"archive").unwrap();
        let prepared = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );

        let result = complete_source_cleanup(
            prepared.plan,
            &[output],
            &prepared.archived_inputs,
            &*fake,
            &manager.source_cleanup_journal,
            &|| false,
            &squallz_core::api::NoProgress,
        );

        assert_eq!(result.status, SourceCleanupStatus::Failed);
        assert!(source.exists());
        assert!(fake.calls().is_empty());
        let notice = manager.source_cleanup_recovery().unwrap();
        assert_eq!(notice.generation, 1);
        assert_eq!(notice.status, "needs_attention");
        assert_eq!(notice.reason.as_deref(), Some("journal_invalid"));
        assert_eq!(notice.journal_path.as_deref(), journal_path.to_str());
        assert_eq!(notice.path, None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_preflight_fingerprint_observes_cancellation() {
        let dir = temp_dir("source-cleanup-preflight-cancel");
        let source = dir.join("source.bin");
        std::fs::write(&source, deterministic_payload(1024 * 1024)).unwrap();
        let result = prepare_source_cleanup(
            &[source],
            PostSuccessAction::TrashSource,
            &[],
            &|| true,
            &squallz_core::api::NoProgress,
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_postcheck_cancellation_prevents_trash() {
        let dir = temp_dir("source-cleanup-postcheck-cancel");
        let source = dir.join("source.bin");
        let output = dir.join("archive.zip");
        std::fs::write(&source, deterministic_payload(1024 * 1024)).unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let plan = prepare_cleanup(
            std::slice::from_ref(&source),
            PostSuccessAction::TrashSource,
            &[],
        );
        let checkpoints = std::sync::atomic::AtomicUsize::new(0);
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| {
            checkpoints.fetch_add(1, Ordering::Relaxed) >= 5
        });

        assert_eq!(result.status, SourceCleanupStatus::Cancelled);
        assert_eq!(result.moved, 0);
        assert_eq!(result.kept, 1);
        assert!(fake.calls().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_stops_moving_sources_after_cancellation() {
        let dir = temp_dir("source-cleanup-cancelled");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        let output = dir.join("archive.zip");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        std::fs::write(&output, b"archive").unwrap();
        let plan = prepare_cleanup(&[first, second], PostSuccessAction::TrashSource, &[]);
        let fake = FakeTrashAdapter::default();

        let result = finish_cleanup(plan, &[output], &fake, &|| !fake.calls().is_empty());

        assert_eq!(result.status, SourceCleanupStatus::Cancelled);
        assert_eq!(result.moved, 1);
        assert_eq!(result.kept, 1);
        assert_eq!(fake.calls().len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    fn write_macos_sfx_template(path: &Path) {
        let executable = path.join("Contents/MacOS/squallz-gui");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(path.join("Contents/Resources")).unwrap();
        let mut bytes = vec![0u8; 512];
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[0x80..0x80 + squallz_core::SFX_GUI_STUB_MARKER.len()]
            .copy_from_slice(&squallz_core::SFX_GUI_STUB_MARKER);
        std::fs::write(executable, bytes).unwrap();
        std::fs::write(
            path.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>squallz-gui</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#,
        )
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    fn write_host_sfx_template(path: &Path) {
        write_macos_sfx_template(path);
    }

    #[cfg(target_os = "windows")]
    fn write_host_sfx_template(path: &Path) {
        let mut bytes = vec![0u8; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
        bytes[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        bytes[0x104..0x108].copy_from_slice(&16u32.to_le_bytes());
        bytes[0x190..0x190 + squallz_core::SFX_CLI_STUB_MARKER.len()]
            .copy_from_slice(&squallz_core::SFX_CLI_STUB_MARKER);
        std::fs::write(path, bytes).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn write_host_sfx_template(path: &Path) {
        let mut bytes = vec![0u8; 128];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[0x40..0x40 + squallz_core::SFX_CLI_STUB_MARKER.len()]
            .copy_from_slice(&squallz_core::SFX_CLI_STUB_MARKER);
        std::fs::write(path, bytes).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn sfx_capability_distinguishes_missing_invalid_and_available_runtimes() {
        let missing = JobManager::with_audit_and_template(Arc::new(OperationAudit::memory()), None);
        let capability = missing.sfx_capability();
        assert!(!capability.available);
        assert_eq!(capability.status, "missing");

        let dir = temp_dir("sfx-capability-status");
        let invalid_path = dir.join("invalid-runtime");
        std::fs::write(&invalid_path, b"not an executable runtime").unwrap();
        let invalid =
            JobManager::with_test_sfx_template(Arc::new(OperationAudit::memory()), invalid_path);
        let capability = invalid.sfx_capability();
        assert!(!capability.available);
        assert_eq!(capability.status, "invalid");

        let valid_path = match SfxTarget::host() {
            SfxTarget::Macos => dir.join("Squallz.app"),
            SfxTarget::Windows => dir.join("sqz.exe"),
            SfxTarget::Linux => dir.join("sqz"),
        };
        write_host_sfx_template(&valid_path);
        let available =
            JobManager::with_test_sfx_template(Arc::new(OperationAudit::memory()), valid_path);
        let capability = available.sfx_capability();
        assert!(capability.available);
        assert_eq!(capability.status, "available");

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn corrupt_sqz_payload_byte(path: &Path) {
        let mut bytes = std::fs::read(path).unwrap();
        assert!(bytes.len() > 64);
        assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
        let descriptor_len = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
        let payload_start = 64 + descriptor_len;
        assert!(
            payload_start < bytes.len(),
            "payload starts outside archive"
        );
        bytes[payload_start] ^= 0xA5;
        std::fs::write(path, bytes).unwrap();
    }

    fn write_incompressible_file(path: &Path, len: usize) {
        let mut state = 0x9E37_79B9u32;
        let data: Vec<u8> = (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        std::fs::write(path, data).unwrap();
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    #[cfg(unix)]
    fn rar5_test_volume(index: u64, has_next: bool) -> Vec<u8> {
        fn encode_vint(mut value: u64) -> Vec<u8> {
            let mut bytes = Vec::new();
            loop {
                let mut byte = (value & 0x7f) as u8;
                value >>= 7;
                if value != 0 {
                    byte |= 0x80;
                }
                bytes.push(byte);
                if value == 0 {
                    return bytes;
                }
            }
        }

        fn block(header_type: u64, fields: &[u8]) -> Vec<u8> {
            let mut header = encode_vint(header_type);
            header.extend(encode_vint(0));
            header.extend_from_slice(fields);
            let size = encode_vint(header.len() as u64);
            let mut checksum_data = size.clone();
            checksum_data.extend_from_slice(&header);
            let mut block = crc32(&checksum_data).to_le_bytes().to_vec();
            block.extend(size);
            block.extend(header);
            block
        }

        let mut bytes = b"Rar!\x1A\x07\x01\x00".to_vec();
        let mut archive_flags = 0x0001;
        let mut main_fields = Vec::new();
        if index > 0 {
            archive_flags |= 0x0002;
        }
        main_fields.extend(encode_vint(archive_flags));
        if index > 0 {
            main_fields.extend(encode_vint(index));
        }
        bytes.extend(block(1, &main_fields));
        bytes.extend(block(5, &encode_vint(u64::from(has_next))));
        bytes
    }

    #[cfg(unix)]
    fn split_zip_test_final_volume(final_disk: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 22];
        bytes[..4].copy_from_slice(b"PK\x05\x06");
        bytes[4..6].copy_from_slice(&final_disk.to_le_bytes());
        bytes[6..8].copy_from_slice(&final_disk.to_le_bytes());
        bytes
    }

    fn build_stored_zip(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            let size = data.len() as u32;
            let name_len = name.len() as u16;

            out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0x21u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(data);

            central.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&name_len.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
        }
        let central_offset = out.len() as u32;
        let central_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn states_of(events: &[(String, serde_json::Value)], id: u64) -> Vec<String> {
        events
            .iter()
            .filter(|(name, p)| name == EV_STATE && p["id"] == id)
            .map(|(_, p)| p["state"].as_str().unwrap().to_owned())
            .collect()
    }

    fn real_current_file_progress_events(
        events: &[(String, serde_json::Value)],
        id: u64,
    ) -> Vec<&serde_json::Value> {
        events
            .iter()
            .filter(|(name, payload)| {
                name == EV_PROGRESS
                    && payload["id"] == id
                    && payload["current_total"].as_u64().unwrap_or(0) > 0
            })
            .map(|(_, payload)| payload)
            .collect()
    }

    fn assert_real_current_file_progress(
        events: &[(String, serde_json::Value)],
        id: u64,
        operation: &str,
    ) {
        let progress = real_current_file_progress_events(events, id);
        assert!(
            !progress.is_empty(),
            "{operation} job should emit a real current-file progress event with current_total > 0"
        );
        assert!(
            progress.iter().any(|payload| {
                let done = payload["current_done"].as_u64().unwrap_or(0);
                let total = payload["current_total"].as_u64().unwrap_or(0);
                total > 0 && done <= total
            }),
            "{operation} job current-file progress should keep current_done bounded by current_total"
        );
    }

    fn wait_for_event(
        sink: &TestSink,
        timeout: std::time::Duration,
        predicate: impl Fn(&(String, serde_json::Value)) -> bool,
    ) {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if sink.events.lock().unwrap().iter().any(&predicate) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("timed out waiting for event");
    }

    fn wait_for_state(sink: &TestSink, id: u64, state: &str, timeout: std::time::Duration) {
        wait_for_event(sink, timeout, |(name, payload)| {
            name == EV_STATE && payload["id"] == id && payload["state"] == state
        });
    }

    fn wait_for_password_prompt(sink: &TestSink, id: u64) {
        wait_for_event(
            sink,
            std::time::Duration::from_secs(2),
            |(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id,
        );
    }

    fn wait_for_password_prompt_count(sink: &TestSink, id: u64, expected: usize) {
        let started = Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(2) {
            let count = sink
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
                .count();
            if count >= expected {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("timed out waiting for password prompt {expected} for job {id}");
    }

    fn wait_for_snapshot_state(
        manager: &JobManager,
        id: u64,
        state: &str,
        timeout: std::time::Duration,
    ) {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if manager
                .snapshot(id)
                .is_some_and(|snapshot| snapshot.state == state)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("timed out waiting for job {id} snapshot state {state}");
    }

    fn done_result(events: &[(String, serde_json::Value)], id: u64) -> Option<serde_json::Value> {
        events
            .iter()
            .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "done")
            .and_then(|(_, p)| p.get("result").cloned())
    }

    #[test]
    fn snapshot_store_scopes_jobs_and_tracks_deltas() {
        let mut store = JobSnapshotStore::default();
        let main_version = store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("main-input")),
            "queued",
        );
        let owner_version = store.insert_with_resources(
            2,
            Some("task-owner".into()),
            checksum_job(Path::new("task-input")),
            "queued",
            JobResources::new(3),
            Some(512 * 1024 * 1024),
        );
        assert!(owner_version > main_version);

        let main = store.delta("main", None);
        assert!(main.reset);
        assert_eq!(main.upserts.len(), 2);
        assert_eq!(main.upserts[0].origin, JobOrigin::App);
        assert!(main.upserts[0].owned_by_requester);
        assert_eq!(main.upserts[1].origin, JobOrigin::FileManager);
        assert!(!main.upserts[1].owned_by_requester);

        let owner = store.delta("task-owner", None);
        assert_eq!(owner.upserts.len(), 1);
        assert_eq!(owner.upserts[0].id, 2);
        assert!(owner.upserts[0].owned_by_requester);
        assert_eq!(owner.upserts[0].origin, JobOrigin::FileManager);
        assert!(store.delta("task-other", None).upserts.is_empty());

        let progress_version = store
            .set_progress(
                2,
                JobProgressSnapshot {
                    done: 4,
                    total: 10,
                    current: "payload.bin".into(),
                    current_done: 2,
                    current_total: 8,
                    scanned_entries: None,
                    speed: 64,
                    ..JobProgressSnapshot::default()
                },
            )
            .unwrap();
        assert!(progress_version > owner_version);
        let done_version = store
            .set_state(
                2,
                "done",
                None,
                Some(serde_json::json!({"operation": "checksum"})),
            )
            .unwrap();
        assert!(done_version > progress_version);
        assert!(store.set_state(2, "running", None, None).is_none());
        assert!(store
            .set_progress(2, JobProgressSnapshot::default())
            .is_none());
        assert_eq!(store.snapshot("main", 2).unwrap().state, "done");

        let denied = store.dismiss("task-other", &[2]).unwrap_err();
        assert_eq!(denied.key, "error.other");
        assert!(store.snapshot("main", 2).is_some());
        let main_baseline = store.revision;
        store.dismiss("task-owner", &[2]).unwrap();
        let removed = store.delta("task-owner", Some(done_version));
        assert!(!removed.reset);
        assert!(removed.upserts.is_empty());
        assert_eq!(removed.removed, vec![2]);
        assert!(store.snapshot("task-owner", 2).is_none());
        assert!(store.snapshot("main", 2).is_some());
        assert!(store.delta("task-owner", None).upserts.is_empty());
        let main_after_owner_dismiss = store.delta("main", Some(main_baseline));
        assert!(main_after_owner_dismiss.upserts.is_empty());
        assert!(main_after_owner_dismiss.removed.is_empty());

        let serialized = serde_json::to_string(&main.upserts[1]).unwrap();
        assert!(!serialized.contains("task-owner"));
        assert!(serialized.contains("owned_by_requester"));
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(value["interaction"].is_null());
        assert!(value["queue_position"].is_null());
        assert!(value["queue_wait_reason"].is_null());
        assert_eq!(value["cpu_threads"], 3);
        assert_eq!(value["stream_buffer_limit_bytes"], 512 * 1024 * 1024);
    }

    #[test]
    fn snapshot_store_tracks_authoritative_queue_positions() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("first-input")),
            "queued",
        );
        store.insert(
            2,
            Some("task-owner".into()),
            checksum_job(Path::new("second-input")),
            "queued",
        );
        store.insert(
            3,
            Some("main".into()),
            checksum_job(Path::new("third-input")),
            "queued",
        );

        store.sync_queue_positions(&[1, 2, 3]);
        assert_eq!(store.snapshot("main", 1).unwrap().queue_position, Some(1));
        assert_eq!(store.snapshot("main", 2).unwrap().queue_position, Some(2));
        assert_eq!(store.snapshot("main", 3).unwrap().queue_position, Some(3));

        let baseline = store.revision;
        store.sync_queue_positions(&[3, 1, 2]);
        let delta = store.delta("main", Some(baseline));
        assert_eq!(
            delta
                .upserts
                .iter()
                .map(|snapshot| (snapshot.id, snapshot.queue_position))
                .collect::<Vec<_>>(),
            vec![(1, Some(2)), (2, Some(3)), (3, Some(1))]
        );

        store.set_state(3, "running", None, None).unwrap();
        assert_eq!(store.snapshot("main", 3).unwrap().queue_position, None);
        assert_eq!(store.snapshot("main", 1).unwrap().queue_position, Some(1));
        assert_eq!(store.snapshot("main", 2).unwrap().queue_position, Some(2));
    }

    #[test]
    fn main_window_can_reorder_visible_queued_jobs_without_cross_owner_access() {
        let manager = JobManager::new();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        manager.queue.submit(Box::new(move |_ctl, _progress| {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.recv().unwrap();

        let order = Arc::new(StdMutex::new(Vec::new()));
        let submit_marker = |marker| {
            let order = Arc::clone(&order);
            manager.queue.submit(Box::new(move |_ctl, _progress| {
                order.lock().unwrap().push(marker);
                Ok(())
            }))
        };
        let first_queue_id = submit_marker(1);
        let second_queue_id = submit_marker(2);
        let third_queue_id = submit_marker(3);
        let first_id = 101;
        let second_id = 102;
        let third_id = 103;
        let events = Arc::new(TestSink::default());
        {
            let mut snapshots = lock_unpoisoned(&manager.snapshots);
            snapshots.insert(
                first_id,
                Some("task-first".into()),
                checksum_job(Path::new("first-input")),
                "queued",
            );
            snapshots.insert(
                second_id,
                Some("task-second".into()),
                checksum_job(Path::new("second-input")),
                "queued",
            );
            snapshots.insert(
                third_id,
                Some("task-third".into()),
                checksum_job(Path::new("third-input")),
                "queued",
            );
        }
        {
            let mut registry = lock_unpoisoned(&manager.registry);
            registry.jobs.insert(
                first_id,
                ManagedJob {
                    queue_id: first_queue_id,
                    cancel_flag: Arc::new(AtomicBool::new(false)),
                    owner_window: Some("task-first".into()),
                    events: events.clone(),
                    pausable: true,
                },
            );
            registry.jobs.insert(
                second_id,
                ManagedJob {
                    queue_id: second_queue_id,
                    cancel_flag: Arc::new(AtomicBool::new(false)),
                    owner_window: Some("task-second".into()),
                    events,
                    pausable: true,
                },
            );
            registry.jobs.insert(
                third_id,
                ManagedJob {
                    queue_id: third_queue_id,
                    cancel_flag: Arc::new(AtomicBool::new(false)),
                    owner_window: Some("task-third".into()),
                    events: Arc::new(TestSink::default()),
                    pausable: true,
                },
            );
        }
        manager.sync_queue_positions();
        assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(1));
        assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(2));
        assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(3));
        assert_eq!(
            manager.snapshot(first_id).unwrap().queue_wait_reason,
            Some(QueueWaitReason::ParallelLimit)
        );
        assert_eq!(
            manager.snapshot(second_id).unwrap().queue_wait_reason,
            Some(QueueWaitReason::QueueOrder)
        );
        let first_snapshot = serde_json::to_value(manager.snapshot(first_id).unwrap()).unwrap();
        assert_eq!(first_snapshot["queue_wait_reason"], "parallel_limit");
        assert_eq!(first_snapshot["cpu_threads"], 1);

        assert!(manager
            .move_earlier_for_window("task-first", second_id)
            .is_err());
        assert!(manager
            .move_before_for_window("task-second", second_id, Some(first_id))
            .is_err());
        manager
            .move_before_for_window("main", third_id, Some(first_id))
            .unwrap();
        assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(1));
        assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(2));
        assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(3));

        manager
            .move_before_for_window("main", first_id, None)
            .unwrap();
        assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(1));
        assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(2));
        assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(3));

        manager.move_earlier_for_window("main", second_id).unwrap();
        assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(1));
        assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(2));
        assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(3));
        assert!(manager.move_earlier_for_window("main", second_id).is_err());

        gate_tx.send(()).unwrap();
        manager.queue.wait_idle();
        assert_eq!(*order.lock().unwrap(), vec![2, 3, 1]);
    }

    #[test]
    fn snapshot_starting_state_cannot_override_a_published_pause() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("queued-input")),
            "queued",
        );
        assert!(store.set_starting_state(1, "running").is_some());
        assert_eq!(store.snapshot("main", 1).unwrap().state, "running");

        store.insert(
            2,
            Some("main".into()),
            checksum_job(Path::new("paused-input")),
            "queued",
        );
        let paused_version = store.set_state(2, "paused", None, None).unwrap();
        assert!(store.set_starting_state(2, "running").is_none());
        let paused = store.snapshot("main", 2).unwrap();
        assert_eq!(paused.state, "paused");
        assert_eq!(paused.version, paused_version);
    }

    #[test]
    fn snapshot_dismissal_is_scoped_to_each_observer_and_survives_reset() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            10,
            Some("task-owner".into()),
            checksum_job(Path::new("main-dismissed")),
            "done",
        );
        let baseline = store.insert(
            11,
            Some("task-owner".into()),
            checksum_job(Path::new("owner-dismissed")),
            "done",
        );

        store.dismiss("main", &[10]).unwrap();
        store.dismiss("task-owner", &[11]).unwrap();

        assert!(store.snapshot("main", 10).is_none());
        assert!(store.snapshot("main", 11).is_some());
        assert!(store.snapshot("task-owner", 10).is_some());
        assert!(store.snapshot("task-owner", 11).is_none());
        assert_eq!(store.delta("main", Some(baseline)).removed, vec![10]);
        assert_eq!(store.delta("task-owner", Some(baseline)).removed, vec![11]);

        let main_reset = store.delta("main", None);
        assert!(main_reset.reset);
        assert_eq!(
            main_reset
                .upserts
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![11]
        );
        let owner_reset = store.delta("task-owner", None);
        assert!(owner_reset.reset);
        assert_eq!(
            owner_reset
                .upserts
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![10]
        );
        assert_eq!(store.jobs.len(), 2);

        let revision = store.revision;
        store.dismiss("main", &[10]).unwrap();
        store.dismiss("task-owner", &[11]).unwrap();
        assert_eq!(store.revision, revision);
    }

    #[test]
    fn snapshot_store_bounds_history_without_pruning_active_jobs() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("active-input")),
            "running",
        );
        for done in 1..=(MAX_SNAPSHOT_CHANGES as u64 + 1) {
            store
                .set_progress(
                    1,
                    JobProgressSnapshot {
                        done,
                        total: MAX_SNAPSHOT_CHANGES as u64 + 1,
                        ..JobProgressSnapshot::default()
                    },
                )
                .unwrap();
        }

        assert_eq!(store.changes.len(), MAX_SNAPSHOT_CHANGES);
        let stale = store.delta("main", Some(0));
        assert!(stale.reset);
        assert_eq!(stale.upserts.len(), 1);
        assert_eq!(stale.upserts[0].id, 1);
        assert_eq!(stale.upserts[0].state, "running");

        let mut tombstones = JobSnapshotStore::default();
        for id in 1..=(MAX_SNAPSHOT_CHANGES as u64 + 1) {
            tombstones.insert(
                id,
                Some("main".into()),
                checksum_job(Path::new("terminal-input")),
                "done",
            );
            tombstones.dismiss("main", &[id]).unwrap();
        }
        assert_eq!(tombstones.removals.len(), MAX_SNAPSHOT_CHANGES);
        assert!(!tombstones.removals.iter().any(|removal| removal.id == 1));
        assert!(tombstones
            .removals
            .iter()
            .any(|removal| removal.id == MAX_SNAPSHOT_CHANGES as u64 + 1));
    }

    #[test]
    fn snapshot_store_keeps_only_the_newest_terminal_jobs() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("active-input")),
            "running",
        );
        for id in 2..=(MAX_TERMINAL_SNAPSHOTS as u64 + 2) {
            store.insert(
                id,
                Some("main".into()),
                checksum_job(Path::new("terminal-input")),
                "done",
            );
        }

        assert!(store.jobs.contains_key(&1));
        assert!(!store.jobs.contains_key(&2));
        assert_eq!(store.terminal_order.len(), MAX_TERMINAL_SNAPSHOTS);
        assert_eq!(store.jobs.len(), MAX_TERMINAL_SNAPSHOTS + 1);
        let delta = store.delta("main", Some(0));
        assert!(!delta.reset);
        assert!(delta.removed.contains(&2));
    }

    #[test]
    fn terminal_retention_removal_reaches_every_original_observer() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("task-owner".into()),
            checksum_job(Path::new("oldest-terminal")),
            "done",
        );
        store.dismiss("main", &[1]).unwrap();
        let after_main_dismiss = store.revision;
        for id in 2..=(MAX_TERMINAL_SNAPSHOTS as u64 + 1) {
            store.insert(
                id,
                Some("task-owner".into()),
                checksum_job(Path::new("newer-terminal")),
                "done",
            );
        }

        assert!(!store.jobs.contains_key(&1));
        assert_eq!(store.terminal_order.len(), MAX_TERMINAL_SNAPSHOTS);
        assert_eq!(
            store.delta("main", Some(after_main_dismiss)).removed,
            vec![1]
        );
        assert_eq!(
            store.delta("task-owner", Some(after_main_dismiss)).removed,
            vec![1]
        );
        assert!(!store
            .delta("main", None)
            .upserts
            .iter()
            .any(|snapshot| snapshot.id == 1));
        assert!(!store
            .delta("task-owner", None)
            .upserts
            .iter()
            .any(|snapshot| snapshot.id == 1));
    }

    #[test]
    fn snapshot_controls_use_requester_scope_and_owner_event_sink() {
        let dir = temp_dir("snapshot-owner-controls");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let manager = JobManager::new();
        let owner_sink = Arc::new(TestSink::default());
        let owner_events: Arc<dyn EventSink> = owner_sink.clone();
        let id = manager.submit_for_test_window(
            "task-owner".into(),
            Arc::clone(&state),
            owner_events,
            password_test_job(&archive),
            SettingsDto::default(),
        );

        wait_for_password_prompt(&owner_sink, id);
        let owner_snapshot = manager.snapshot_for_window("task-owner", id).unwrap();
        assert!(owner_snapshot.owned_by_requester);
        assert_eq!(owner_snapshot.interaction, Some(JobInteraction::Password));
        let main_snapshot = manager.snapshot_for_window("main", id).unwrap();
        assert!(!main_snapshot.owned_by_requester);
        assert_eq!(main_snapshot.origin, JobOrigin::FileManager);
        let denied = manager.snapshot_for_window("task-other", id).unwrap_err();
        assert_eq!(denied.key, "error.other");
        assert_eq!(
            manager.pause_for_window("task-other", id).unwrap_err().key,
            "error.other"
        );

        manager.pause_for_window("main", id).unwrap();
        manager.resume_for_window("main", id).unwrap();
        assert_eq!(
            manager
                .answer_password_for_window("main", id, None)
                .unwrap_err()
                .key,
            "error.other"
        );
        assert_eq!(
            manager
                .answer_conflict_for_window("task-owner", id, "skip".into(), false)
                .unwrap_err()
                .key,
            "error.other"
        );
        manager
            .answer_password_for_window("task-owner", id, None)
            .unwrap();
        wait_for_state(
            &owner_sink,
            id,
            "cancelled",
            std::time::Duration::from_secs(2),
        );
        assert_eq!(
            manager
                .answer_password_for_window("task-owner", id, None)
                .unwrap_err()
                .key,
            "error.other"
        );
        manager.wait_idle();

        let terminal = manager.snapshot_for_window("main", id).unwrap();
        assert_eq!(terminal.state, "cancelled");
        assert_eq!(terminal.interaction, None);
        let states = owner_sink.events.lock().unwrap().clone();
        assert_eq!(
            states_of(&states, id),
            vec!["queued", "running", "paused", "running", "cancelled"]
        );
        let versions = states
            .iter()
            .filter(|(name, payload)| name == EV_STATE && payload["id"] == id)
            .map(|(_, payload)| payload["version"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            manager.cancel_for_window("main", u64::MAX).unwrap_err().key,
            "error.other"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conflict_answers_require_the_owner_and_current_prompt_type() {
        let dir = temp_dir("snapshot-conflict-answer");
        let archive = dir.join("conflict.zip");
        std::fs::write(&archive, build_stored_zip(&[(b"same.txt", b"new bytes")])).unwrap();
        let output = dir.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let existing = output.join("same.txt");
        std::fs::write(&existing, b"original bytes").unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let owner_sink = Arc::new(TestSink::default());
        let owner_events: Arc<dyn EventSink> = owner_sink.clone();
        let id = manager.submit_for_test_window(
            "task-conflict-owner".into(),
            state,
            owner_events,
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: output.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Ask,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );

        wait_for_event(
            &owner_sink,
            std::time::Duration::from_secs(2),
            |(name, payload)| name == EV_ASK_CONFLICT && payload["id"] == id,
        );
        assert_eq!(
            manager
                .snapshot_for_window("task-conflict-owner", id)
                .unwrap()
                .interaction,
            Some(JobInteraction::Conflict)
        );
        assert_eq!(
            manager
                .answer_password_for_window("task-conflict-owner", id, None)
                .unwrap_err()
                .key,
            "error.other"
        );
        assert_eq!(
            manager
                .answer_conflict_for_window("main", id, "skip".into(), false)
                .unwrap_err()
                .key,
            "error.other"
        );
        manager
            .answer_conflict_for_window("task-conflict-owner", id, "skip".into(), false)
            .unwrap();
        wait_for_state(&owner_sink, id, "done", std::time::Duration::from_secs(2));
        assert_eq!(
            manager
                .answer_conflict_for_window("task-conflict-owner", id, "skip".into(), false)
                .unwrap_err()
                .key,
            "error.other"
        );
        manager.wait_idle();

        let terminal = manager.snapshot_for_window("main", id).unwrap();
        assert_eq!(terminal.interaction, None);
        assert_eq!(std::fs::read(existing).unwrap(), b"original bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn queued_pause_does_not_claim_the_worker_until_resumed() {
        let dir = temp_dir("queued-pause-snapshot");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let input = dir.join("queued.txt");
        std::fs::write(&input, b"queued pause").unwrap();
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let blocker_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&archive),
            SettingsDto::default(),
        );
        wait_for_password_prompt(&sink, blocker_id);
        let queued_id = manager.submit(state, events, checksum_job(&input), SettingsDto::default());
        let queue_id = lock_unpoisoned(&manager.registry)
            .jobs
            .get(&queued_id)
            .unwrap()
            .queue_id;
        assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
        assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, Some(1));

        manager.pause_for_window("main", queued_id).unwrap();
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
        assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, None);
        manager.resume_for_window("main", queued_id).unwrap();
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "queued");
        assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, Some(1));
        assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
        manager.pause_for_window("main", queued_id).unwrap();
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
        manager.cancel_for_window("main", blocker_id).unwrap();
        wait_for_state(
            &sink,
            blocker_id,
            "cancelled",
            std::time::Duration::from_secs(2),
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
        assert!(!states_of(&sink.events.lock().unwrap(), queued_id)
            .iter()
            .any(|state| state == "running"));

        manager.resume_for_window("main", queued_id).unwrap();
        wait_for_state(&sink, queued_id, "done", std::time::Duration::from_secs(2));
        manager.wait_idle();
        let observed = states_of(&sink.events.lock().unwrap(), queued_id);
        assert!(
            observed == vec!["queued", "paused", "queued", "paused", "running", "done"]
                || observed
                    == vec!["queued", "paused", "queued", "paused", "queued", "running", "done"],
            "unexpected queued pause sequence: {observed:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn core_terminal_failure_reconciles_a_nonterminal_snapshot() {
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let gui_id = 91;
        lock_unpoisoned(&manager.snapshots).insert(
            gui_id,
            Some("main".into()),
            checksum_job(Path::new("panic-input")),
            "running",
        );
        let queue_id = manager.queue.submit(Box::new(|_ctl, _progress| {
            panic!("job panic fixture");
        }));
        lock_unpoisoned(&manager.registry).jobs.insert(
            gui_id,
            ManagedJob {
                queue_id,
                cancel_flag: Arc::new(AtomicBool::new(false)),
                owner_window: Some("main".into()),
                events,
                pausable: true,
            },
        );

        manager.wait_idle();

        let snapshot = manager.snapshot_for_window("main", gui_id).unwrap();
        assert_eq!(snapshot.state, "failed");
        assert_eq!(snapshot.error.unwrap().detail, "job panicked");
        assert_eq!(
            states_of(&sink.events.lock().unwrap(), gui_id),
            vec!["failed"]
        );
        assert!(manager.queue.state(queue_id).is_none());
        assert!(!lock_unpoisoned(&manager.registry)
            .jobs
            .contains_key(&gui_id));
    }

    fn poison_lock<T>(mutex: &std::sync::Mutex<T>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison lock for regression coverage");
        }));
        assert!(result.is_err());
    }

    #[test]
    fn job_manager_registry_recovers_after_poison() {
        let dir = temp_dir("map-poison");
        let src = dir.join("data");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello poison").unwrap();
        let zip = dir.join("poison.zip");

        let manager = JobManager::new();
        poison_lock(&manager.registry);

        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![src.to_string_lossy().into_owned()],
                dest: zip.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(manager.snapshot(id).unwrap().state, "done");
        assert!(manager.pause_for_window("main", id).is_err());
        assert!(manager.resume_for_window("main", id).is_err());
        assert!(manager.cancel_for_window("main", id).is_err());
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn create_job_request_is_shared_for_plan_and_worker_options() {
        let spec = JobSpec::Compress {
            inputs: vec!["source-a".into(), "source-b".into()],
            dest: "archive.sqz".into(),
            level: 8,
            password: Some("secret".into()),
            encrypt_names: true,
            split_size: Some(32 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec!["*.tmp".into()],
            content_policy: squallz_core::CreateContentPolicy::CrossPlatformClean,
            sqz_inner_format: Some(squallz_core::SqzInnerFormat::Zip),
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: true,
            replace_existing: false,
            replacement_guard: None,
        };
        let settings = SettingsDto {
            performance_threads: Some(3),
            performance_memory_limit_bytes: Some(32 * 1024),
            ..SettingsDto::default()
        };

        let request = create_job_request(&spec, &settings).unwrap();
        assert_eq!(
            request.inputs,
            vec![PathBuf::from("source-a"), PathBuf::from("source-b")]
        );
        assert_eq!(request.dest, PathBuf::from("archive.sqz"));
        assert_eq!(request.options.level, CompressionLevel::from_numeric(8));
        assert!(request.options.password.is_some());
        assert!(request.options.encrypt_filenames);
        assert_eq!(request.options.split_size, Some(32 * 1024));
        assert_eq!(
            request.options.excludes,
            vec![".DS_Store", "._*", "__MACOSX", "*.tmp"]
        );
        assert_eq!(
            request.options.sqz.inner_format,
            squallz_core::SqzInnerFormat::Zip
        );
        assert_eq!(request.options.resources.threads, Some(3));
        assert_eq!(request.options.resources.memory_limit, Some(32 * 1024));
        assert!(request.sfx_options().is_none());
        assert_eq!(request.post_success, PostSuccessAction::KeepSource);
        assert!(request.test_after_create);
        assert!(!request.replace_existing);
        assert_eq!(request.commit_policy, CreateCommitPolicy::NoReplace);

        let not_create = JobSpec::Test {
            path: "archive.zip".into(),
            encoding: None,
            password: None,
        };
        assert!(matches!(
            create_job_request(&not_create, &SettingsDto::default()),
            Err(FormatError::Unsupported(_))
        ));
    }

    #[test]
    fn convert_create_options_are_shared_for_plan_and_worker() {
        let spec = JobSpec::Convert {
            src: "source.zip".into(),
            dest: "converted.7z".into(),
            level: 8,
            src_encoding: Some("GBK".into()),
            src_password: Some("source secret".into()),
            dest_password: Some("destination secret".into()),
            encrypt_names: true,
            split_size: Some(256 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        };
        let settings = SettingsDto {
            performance_threads: Some(3),
            performance_memory_limit_bytes: Some(32 * 1024),
            ..SettingsDto::default()
        };

        let options = convert_create_options(&spec, &settings).unwrap();
        assert_eq!(options.level, CompressionLevel::from_numeric(8));
        assert!(options.password.is_some());
        assert!(options.encrypt_filenames);
        assert_eq!(options.split_size, Some(256 * 1024));
        assert_eq!(options.resources.threads, Some(3));
        assert_eq!(options.resources.memory_limit, Some(32 * 1024));

        let not_convert = JobSpec::Test {
            path: "archive.zip".into(),
            encoding: None,
            password: None,
        };
        assert!(matches!(
            convert_create_options(&not_convert, &SettingsDto::default()),
            Err(FormatError::Unsupported(_))
        ));
    }

    #[test]
    fn job_policy_enums_reject_unknown_values() {
        assert!(serde_json::from_str::<squallz_core::api::SplitOutputMode>("\"generic\"").is_ok());
        assert!(
            serde_json::from_str::<squallz_core::api::SplitOutputMode>("\"format_specific\"")
                .is_err()
        );
        assert!(serde_json::from_str::<squallz_core::api::OverwritePolicy>("\"replace\"").is_err());
        assert!(serde_json::from_str::<squallz_core::api::SymlinkPolicy>("\"unsafe\"").is_err());
        assert!(serde_json::from_str::<squallz_core::SqzInnerFormat>("\"raw\"").is_err());
        assert!(serde_json::from_str::<SfxTarget>("\"darwin\"").is_err());
        assert!(serde_json::from_str::<ChecksumAlgorithm>("\"sha-256\"").is_err());
        assert!(serde_json::from_str::<ExternalTaskActionDto>("\"extract\"").is_err());
    }

    #[test]
    fn create_job_request_preserves_the_confirmed_replacement_guard() {
        let dir = temp_dir("create-request-replacement-guard");
        let destination = dir.join("archive.zip");
        std::fs::write(&destination, b"existing archive").unwrap();
        let guard = squallz_core::inspect_create_destination(
            &destination,
            squallz_core::CreateArtifactKind::Archive,
        )
        .unwrap()
        .guard
        .unwrap();
        let mut spec = compress_file_job(Path::new("source.txt"), &destination);
        let JobSpec::Compress {
            replace_existing,
            replacement_guard,
            ..
        } = &mut spec
        else {
            unreachable!();
        };
        *replace_existing = true;
        *replacement_guard = Some(guard);

        let request = create_job_request(&spec, &SettingsDto::default()).unwrap();

        assert_eq!(
            request.commit_policy,
            CreateCommitPolicy::ReplaceIfUnchanged(guard)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn create_job_request_rejects_a_guard_on_a_no_replace_job() {
        let dir = temp_dir("create-request-invalid-guard");
        let destination = dir.join("archive.zip");
        std::fs::write(&destination, b"existing archive").unwrap();
        let guard = squallz_core::inspect_create_destination(
            &destination,
            squallz_core::CreateArtifactKind::Archive,
        )
        .unwrap()
        .guard
        .unwrap();
        let mut spec = compress_file_job(Path::new("source.txt"), &destination);
        let JobSpec::Compress {
            replace_existing,
            replacement_guard,
            ..
        } = &mut spec
        else {
            unreachable!();
        };
        *replace_existing = false;
        *replacement_guard = Some(guard);

        assert!(matches!(
            create_job_request(&spec, &SettingsDto::default()),
            Err(FormatError::Unsupported(_))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn output_jobs_require_a_guard_for_replacement() {
        for operation in ["create", "convert", "export"] {
            assert_eq!(
                job_output_commit_policy(false, None, operation).unwrap(),
                CreateCommitPolicy::NoReplace
            );
            let error = job_output_commit_policy(true, None, operation).unwrap_err();
            assert!(matches!(
                error,
                FormatError::Unsupported(detail)
                    if detail.contains(operation) && detail.contains("destination guard")
            ));
        }
    }

    #[test]
    fn create_result_reports_preserved_split_outputs_for_review() {
        let primary = PathBuf::from("新归档.zip.001");
        let preserved = vec![
            PathBuf::from(".新归档.zip.001.split-backup-7"),
            PathBuf::from(".新归档.zip.002.split-backup-7"),
        ];
        let result = create_report_result(
            CreateReport {
                primary_output: primary.clone(),
                outputs: vec![primary.clone()],
                preserved_outputs: preserved.clone(),
                total_output_bytes: 42,
                split_volume_count: Some(1),
            },
            "create",
            SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, 1),
            None,
        );

        assert_eq!(
            result["primary_output"].as_str(),
            Some(primary.to_string_lossy().as_ref())
        );
        assert_eq!(
            result["preserved_outputs"],
            serde_json::json!([
                preserved[0].to_string_lossy(),
                preserved[1].to_string_lossy(),
            ])
        );
        assert_eq!(result["split"], true);
        assert_eq!(result["tested_after_create"], false);
        assert!(result["entries_tested_after_create"].is_null());
    }

    #[test]
    fn sfx_result_reports_preserved_previous_outputs_for_the_shared_warning_ui() {
        let primary = PathBuf::from("Installer.app");
        let preserved = PathBuf::from(".squallz-sfx-a18e9f52-7-1/previous");
        let result = sfx_report_result(
            SfxBuildReport {
                path: primary.clone(),
                target: SfxTarget::Macos,
                layout: squallz_core::SfxLayout::MacosApp,
                stub_bytes: 12,
                payload_bytes: 30,
                total_bytes: 42,
                payload_crc32: 0,
                payload_sha256: Some([7; 32]),
                requires_signing: true,
                preserved_outputs: vec![preserved.clone()],
            },
            SourceCleanupResult::new(SourceCleanupStatus::NotRequested, 0, 1),
            Some(7),
        );

        assert_eq!(
            result["primary_output"].as_str(),
            Some(primary.to_string_lossy().as_ref())
        );
        assert_eq!(
            result["preserved_outputs"],
            serde_json::json!([preserved.to_string_lossy()])
        );
        assert_eq!(result["operation"], "create_sfx");
        assert_eq!(result["requires_signing"], true);
        assert_eq!(result["tested_after_create"], true);
        assert_eq!(result["entries_tested_after_create"], 7);
    }

    #[test]
    fn job_local_locks_recover_after_poison() {
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let no_progress = squallz_core::api::NoProgress;
        let snapshots = Arc::new(Mutex::new(JobSnapshotStore::default()));
        lock_unpoisoned(&snapshots).insert(
            42,
            None,
            JobSpec::Checksum {
                inputs: vec!["payload.bin".into()],
                excludes: Vec::new(),
                algorithm: ChecksumAlgorithm::Sha256,
            },
            "running",
        );
        let progress = EmitProgress::new(
            42,
            Arc::clone(&events),
            Arc::clone(&snapshots),
            &no_progress,
            &[],
        );
        let entry = EntryPath::from_utf8("payload.bin");

        poison_lock(&progress.inner);
        progress.on_entry_progress(10, 100, &entry, 4, 20);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let progress_payload = events_snapshot
            .iter()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("progress event should be emitted after flush");
        assert_eq!(progress_payload["current"], "payload.bin");
        assert_eq!(progress_payload["current_done"], 4);
        assert_eq!(progress_payload["current_total"], 20);

        progress.on_scan_progress(7, &EntryPath::from_utf8("assets/icons"));
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let scan_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("scan progress event should be emitted after flush");
        assert_eq!(scan_payload["done"], 0);
        assert_eq!(scan_payload["total"], 0);
        assert_eq!(scan_payload["current"], "assets/icons");
        assert_eq!(scan_payload["scanned_entries"], 7);
        assert_eq!(scan_payload["speed"], 0);

        progress.on_phase(ProgressPhase::UpdateVerify, true);
        progress.on_progress(25, 100, &entry);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let verify_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("verification progress should be emitted");
        assert_eq!(verify_payload["phase"], "update_verify");
        assert_eq!(verify_payload["done"], 25);
        assert_eq!(verify_payload["total"], 100);
        assert_eq!(verify_payload["interruptible"], true);

        progress.on_phase(ProgressPhase::OutputSplit, true);
        progress.on_progress(48, 96, &EntryPath::from_utf8("archive.zip.001"));
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let split_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("volume progress should be emitted");
        assert_eq!(split_payload["phase"], "output_split");
        assert_eq!(split_payload["done"], 48);
        assert_eq!(split_payload["total"], 96);
        assert_eq!(split_payload["current"], "archive.zip.001");
        assert_eq!(split_payload["interruptible"], true);

        progress.on_phase(ProgressPhase::RecoveryProcess, false);
        progress.on_progress(375, 1000, &entry);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let recovery_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("recovery progress should be emitted");
        assert_eq!(recovery_payload["phase"], "recovery_process");
        assert_eq!(recovery_payload["done"], 375);
        assert_eq!(recovery_payload["total"], 1000);
        assert_eq!(recovery_payload["speed"], 0);
        assert_eq!(recovery_payload["interruptible"], false);

        progress.on_phase(ProgressPhase::UpdateCommit, false);
        progress.on_progress(64, 128, &entry);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let commit_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("commit progress should be emitted");
        assert_eq!(commit_payload["phase"], "update_commit");
        assert_eq!(commit_payload["done"], 0);
        assert_eq!(commit_payload["total"], 0);
        assert_eq!(commit_payload["speed"], 0);
        assert_eq!(commit_payload["interruptible"], false);

        progress.on_phase(ProgressPhase::OutputCommit, false);
        progress.on_progress(64, 128, &entry);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let publish_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("output publication progress should be emitted");
        assert_eq!(publish_payload["phase"], "output_commit");
        assert_eq!(publish_payload["done"], 0);
        assert_eq!(publish_payload["total"], 0);
        assert_eq!(publish_payload["interruptible"], false);

        progress.on_phase(ProgressPhase::OutputRecovery, false);
        progress.flush();
        let events_snapshot = sink.events.lock().unwrap().clone();
        let recovery_payload = events_snapshot
            .iter()
            .rev()
            .find(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload)
            .expect("output recovery progress should be emitted");
        assert_eq!(recovery_payload["phase"], "output_recovery");
        assert_eq!(recovery_payload["interruptible"], false);

        let collector = ExtractProblemCollector::default();
        for index in 0..25 {
            collector.skipped_entry(
                &entry,
                &FormatError::Other(format!("damaged sample {index}")),
            );
        }
        let summary = collector.summary();
        assert_eq!(summary.total, 25);
        assert_eq!(summary.messages.len(), 20);
        assert!(summary.messages[0].contains("payload.bin"));
        assert!(summary.is_truncated());
        assert_eq!(summary.omitted(), 5);

        let dir = temp_dir("resolver-poison");
        let existing = dir.join("exists.txt");
        std::fs::write(&existing, b"old").unwrap();
        let resolver = GuiConflictResolver {
            gui_id: 7,
            events,
            bridge: Arc::new(AskBridge::default()),
            snapshots,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            all: Mutex::new(Some("overwrite".into())),
        };
        poison_lock(&resolver.all);
        let incoming = EntryMeta {
            path: EntryPath::from_utf8("incoming.txt"),
            entry_type: squallz_core::api::EntryType::File,
            size: 3,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        assert_eq!(
            resolver.resolve(&existing, &incoming),
            ConflictDecision::Overwrite
        );

        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
            && payload["id"] == 42
            && payload["current"] == "payload.bin"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn progress_redacts_private_source_paths_before_queue_and_window_snapshots() {
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let queue_sink = RecordingProgressSink::default();
        let snapshots = Arc::new(Mutex::new(JobSnapshotStore::default()));
        lock_unpoisoned(&snapshots).insert(
            42,
            None,
            JobSpec::ExtractNested {
                outer_path: "outer.zip".into(),
                entry_path: "inner.zip".into(),
                dest: "output".into(),
                overwrite: squallz_core::api::OverwritePolicy::RenameBoth,
                symlinks: squallz_core::api::SymlinkPolicy::Skip,
                smart: true,
                encoding: None,
                password: None,
                best_effort: false,
            },
            "running",
        );
        let private = "/private/tmp/squallz-nested-job-42/inner.zip";
        let display = "outer.zip!/inner.zip";
        let redactions = vec![(private.to_owned(), display.to_owned())];
        let progress = EmitProgress::new(
            42,
            Arc::clone(&events),
            Arc::clone(&snapshots),
            &queue_sink,
            &redactions,
        );

        progress.on_entry_progress(
            10,
            100,
            &EntryPath::from_utf8(format!("{private}: docs/readme.txt")),
            4,
            20,
        );
        progress.flush();
        progress.on_scan_progress(
            7,
            &EntryPath::from_utf8(format!("scanning {private}/assets")),
        );
        progress.flush();

        let expected = [
            format!("{display}: docs/readme.txt"),
            format!("scanning {display}/assets"),
        ];
        assert_eq!(*queue_sink.paths.lock().unwrap(), expected);

        let emitted = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, payload)| name == EV_PROGRESS && payload["id"] == 42)
            .map(|(_, payload)| payload["current"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(emitted, expected);
        assert!(emitted.iter().all(|current| !current.contains(private)));

        let snapshot = lock_unpoisoned(&snapshots).snapshot("main", 42).unwrap();
        assert_eq!(snapshot.progress.current, expected[1]);
        assert!(!snapshot.progress.current.contains(private));
    }

    #[test]
    fn partial_source_cleanup_keeps_successful_archive_job_done() {
        let dir = temp_dir("source-cleanup-partial");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        let archive = dir.join("archive.zip");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let fake = Arc::new(FakeTrashAdapter::failing(&["second.txt"]));
        let manager =
            JobManager::with_test_trash_adapter(Arc::new(OperationAudit::memory()), fake.clone());
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink;

        let id = manager.submit(
            state,
            events,
            JobSpec::Compress {
                inputs: vec![
                    first.to_string_lossy().into_owned(),
                    second.to_string_lossy().into_owned(),
                ],
                dest: archive.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::TrashSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.state, "done");
        assert!(archive.exists());
        let result = snapshot.result.as_ref().unwrap();
        assert_eq!(result["tested_after_create"], true);
        assert_eq!(result["entries_tested_after_create"], 2);
        let cleanup = &result["source_cleanup"];
        assert_eq!(cleanup["status"], "partial");
        assert_eq!(cleanup["moved"], 1);
        assert_eq!(cleanup["kept"], 1);
        assert_eq!(cleanup["recovery_required"], 1);
        assert!(!cleanup.to_string().contains(dir.to_string_lossy().as_ref()));
        assert_eq!(fake.calls().len(), 2);
        assert!(second.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_source_cleanup_keeps_successful_archive_job_done() {
        let dir = temp_dir("source-cleanup-worker-failed");
        let source = dir.join("source.txt");
        let archive = dir.join("archive.zip");
        std::fs::write(&source, b"source").unwrap();
        let fake = Arc::new(FakeTrashAdapter::failing(&["source.txt"]));
        let manager =
            JobManager::with_test_trash_adapter(Arc::new(OperationAudit::memory()), fake.clone());
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink;

        let id = manager.submit(
            state,
            events,
            JobSpec::Compress {
                inputs: vec![source.to_string_lossy().into_owned()],
                dest: archive.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::TrashSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.state, "done");
        assert!(archive.exists());
        let result = snapshot.result.as_ref().unwrap();
        assert_eq!(result["tested_after_create"], true);
        assert_eq!(result["entries_tested_after_create"], 1);
        let cleanup = &result["source_cleanup"];
        assert_eq!(cleanup["status"], "failed");
        assert_eq!(cleanup["moved"], 0);
        assert_eq!(cleanup["kept"], 1);
        assert_eq!(cleanup["recovery_required"], 1);
        assert!(!cleanup.to_string().contains(dir.to_string_lossy().as_ref()));
        assert_eq!(fake.calls().len(), 1);
        assert!(source.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn create_worker_refuses_existing_destination_and_split_family() {
        let dir = temp_dir("create-destination-race");
        let source = dir.join("source.txt");
        let archive = dir.join("archive.zip");
        let split_archive = dir.join("split.zip");
        let late_volume = dir.join("split.zip.1000");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&archive, b"do not replace").unwrap();
        std::fs::write(&late_volume, b"do not replace volume").unwrap();
        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink;

        let existing_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![source.to_string_lossy().into_owned()],
                dest: archive.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        let split_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![source.to_string_lossy().into_owned()],
                dest: split_archive.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: Some(8 * 1024),
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(manager.snapshot(existing_id).unwrap().state, "failed");
        assert_eq!(manager.snapshot(split_id).unwrap().state, "failed");
        assert_eq!(std::fs::read(&archive).unwrap(), b"do not replace");
        assert_eq!(
            std::fs::read(&late_volume).unwrap(),
            b"do not replace volume"
        );
        assert!(!split_archive.exists());

        let replacement_guard =
            squallz_core::inspect_create_destination(&archive, CreateArtifactKind::Archive)
                .unwrap()
                .guard
                .unwrap();
        let replace_id = manager.submit(
            state,
            events,
            JobSpec::Compress {
                inputs: vec![source.to_string_lossy().into_owned()],
                dest: archive.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: true,
                replacement_guard: Some(replacement_guard),
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(manager.snapshot(replace_id).unwrap().state, "done");
        assert_ne!(std::fs::read(&archive).unwrap(), b"do not replace");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A compress job followed by an extract job runs end to end through
    /// the queue, emitting queued → running → done with progress events.
    #[test]
    fn compress_then_extract_round_trip() {
        let dir = temp_dir("roundtrip");
        let src = dir.join("data");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello squallz").unwrap();
        let zip = dir.join("out.zip");

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id1 = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![src.to_string_lossy().into_owned()],
                dest: zip.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        let out = dir.join("out");
        let id2 = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Extract {
                path: zip.to_string_lossy().into_owned(),
                dest: out.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(out.join("data/hello.txt").exists());
        let create_snapshot = manager.snapshot(id1).unwrap();
        assert_eq!(create_snapshot.state, "done");
        let create_result = create_snapshot.result.as_ref().unwrap();
        assert_eq!(create_result["operation"], "create");
        assert_eq!(
            create_result["primary_output"].as_str(),
            Some(zip.to_string_lossy().as_ref())
        );
        assert_eq!(create_result["outputs"].as_array().map(Vec::len), Some(1));
        assert_eq!(create_result["volume_count"], 1);
        assert_eq!(create_result["split"], false);
        assert_eq!(create_result["source_cleanup"]["status"], "not_requested");
        assert_eq!(create_result["source_cleanup"]["moved"], 0);
        assert_eq!(create_result["source_cleanup"]["kept"], 1);
        assert_eq!(
            create_result["total_bytes"].as_u64(),
            Some(std::fs::metadata(&zip).unwrap().len())
        );
        let snapshot = manager.snapshot(id2).unwrap();
        assert_eq!(snapshot.state, "done");
        let extract_result = snapshot.result.as_ref().unwrap();
        assert_eq!(
            extract_result.get("dest").and_then(|dest| dest.as_str()),
            Some(out.to_string_lossy().as_ref())
        );
        assert_eq!(
            extract_result["plan"]["destination"],
            extract_result["dest"]
        );
        assert_eq!(extract_result["plan"]["layout"], "direct");
        assert_eq!(extract_result["plan"]["entries"], 2);
        assert_eq!(extract_result["counts"]["selected_entries"], 2);
        assert_eq!(extract_result["counts"]["created"], 1);
        assert_eq!(extract_result["counts"]["directories"], 1);
        assert_eq!(extract_result["counts"]["skipped"], 0);
        assert_eq!(extract_result["counts"]["replaced"], 0);
        assert_eq!(extract_result["counts"]["renamed"], 0);
        assert_eq!(extract_result["counts"]["failed"], 0);
        assert_eq!(extract_result["counts"]["output_bytes"], 13);
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id1), vec!["queued", "running", "done"]);
        assert_eq!(states_of(&events, id2), vec!["queued", "running", "done"]);
        assert_eq!(done_result(&events, id1).as_ref(), Some(create_result));
        assert_real_current_file_progress(&events, id1, "compress");
        assert_real_current_file_progress(&events, id2, "extract");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_job_rejects_a_stale_expected_destination_before_writing() {
        let dir = temp_dir("extract-stale-destination");
        let source = dir.join("data");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("hello.txt"), b"hello squallz").unwrap();
        let archive = dir.join("archive.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&source),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let destination = dir.join("output");
        let stale_destination = dir.join("previous-preview");
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            state,
            events,
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: destination.to_string_lossy().into_owned(),
                expected_destination: Some(stale_destination.to_string_lossy().into_owned()),
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap();
        let failed = events
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
            })
            .expect("stale extraction plan must fail");
        assert_eq!(
            failed.1["error"]["key"].as_str(),
            Some("error.destination_changed")
        );
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_job_rejects_an_archive_replaced_after_preflight() {
        let dir = temp_dir("extract-stale-input");
        let original_source = dir.join("original");
        std::fs::create_dir_all(&original_source).unwrap();
        std::fs::write(original_source.join("hello.txt"), b"original payload").unwrap();
        let archive = dir.join("archive.zip");
        let state = Arc::new(AppState::new());
        let control = ControlToken::new();
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&original_source),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &control,
            )
            .unwrap();
        let destination = dir.join("output");
        let (_, _, input_guard) = state
            .engine
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

        let replacement_source = dir.join("replacement");
        std::fs::create_dir_all(&replacement_source).unwrap();
        std::fs::write(
            replacement_source.join("different.txt"),
            b"replacement payload",
        )
        .unwrap();
        let replacement_archive = dir.join("replacement.zip");
        state
            .engine
            .create(
                &replacement_archive,
                &[replacement_source],
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &control,
            )
            .unwrap();
        std::fs::remove_file(&archive).unwrap();
        std::fs::rename(&replacement_archive, &archive).unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            state,
            events,
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: destination.to_string_lossy().into_owned(),
                expected_destination: Some(destination.to_string_lossy().into_owned()),
                expected_input_guard: Some(input_guard),
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap();
        let failed = events
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
            })
            .expect("stale extraction input must fail");
        assert_eq!(
            failed.1["error"]["key"].as_str(),
            Some("error.input_changed")
        );
        assert!(!destination.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_compress_job_reports_the_committed_output_set() {
        let dir = temp_dir("split-create-report");
        let src = dir.join("payload.bin");
        std::fs::write(&src, deterministic_payload(96 * 1024)).unwrap();
        let archive = dir.join("parts.zip");

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            state,
            events,
            JobSpec::Compress {
                inputs: vec![src.to_string_lossy().into_owned()],
                dest: archive.to_string_lossy().into_owned(),
                level: 1,
                password: None,
                encrypt_names: false,
                split_size: Some(8 * 1024),
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: true,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.state, "done");
        let result = snapshot.result.as_ref().unwrap();
        assert_eq!(result["operation"], "create");
        assert_eq!(result["split"], true);
        assert_eq!(result["tested_after_create"], true);
        assert!(result["entries_tested_after_create"]
            .as_u64()
            .is_some_and(|entries| entries > 0));
        let outputs = result["outputs"].as_array().unwrap();
        let volume_count = result["volume_count"].as_u64().unwrap() as usize;
        assert!(volume_count > 1);
        assert_eq!(outputs.len(), volume_count);
        assert_eq!(
            result["primary_output"].as_str(),
            outputs.first().and_then(|path| path.as_str())
        );
        assert!(result["primary_output"]
            .as_str()
            .is_some_and(|path| path.ends_with("parts.zip.001")));
        assert!(!archive.exists());

        let mut actual_total = 0_u64;
        for output in outputs {
            let path = output.as_str().unwrap();
            let metadata = std::fs::metadata(path).unwrap();
            assert!(metadata.is_file());
            actual_total = actual_total.saturating_add(metadata.len());
        }
        assert_eq!(result["total_bytes"].as_u64(), Some(actual_total));

        let events = sink.events.lock().unwrap();
        assert_eq!(done_result(&events, id).as_ref(), Some(result));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn compress_job_forwards_the_sqz_inner_format() {
        let spec = JobSpec::Compress {
            inputs: vec!["data".into()],
            dest: "profile.sqz".into(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: Some(squallz_core::SqzInnerFormat::Zip),
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        };

        let request = create_job_request(&spec, &SettingsDto::default()).unwrap();
        assert_eq!(
            request.options.sqz.inner_format,
            squallz_core::SqzInnerFormat::Zip
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn create_sfx_job_uses_the_shared_queue_and_core() {
        let dir = temp_dir("create-sfx");
        let src = dir.join("notes");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello from the GUI job").unwrap();
        let template = dir.join("Squallz.app");
        write_macos_sfx_template(&template);
        let output = dir.join("Notes.app");
        let audit = Arc::new(OperationAudit::memory());
        let manager = JobManager::with_test_sfx_template(audit, template);
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![src.to_string_lossy().into_owned()],
                dest: output.to_string_lossy().into_owned(),
                level: 5,
                password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: Some(SfxTarget::Macos),
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: true,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.state, "done");
        assert_eq!(
            snapshot
                .result
                .as_ref()
                .and_then(|result| result.get("operation"))
                .and_then(|value| value.as_str()),
            Some("create_sfx")
        );
        let result = snapshot.result.as_ref().unwrap();
        assert_eq!(result["source_cleanup"]["status"], "not_requested");
        assert_eq!(
            result["primary_output"].as_str(),
            Some(output.to_string_lossy().as_ref())
        );
        assert_eq!(result["outputs"].as_array().map(Vec::len), Some(1));
        assert_eq!(result["volume_count"], 1);
        assert_eq!(result["split"], false);
        assert_eq!(result["tested_after_create"], true);
        assert!(result["entries_tested_after_create"]
            .as_u64()
            .is_some_and(|entries| entries > 0));
        assert!(result["total_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0));
        assert_eq!(result["requires_signing"], true);
        assert!(output
            .join("Contents/Resources/squallz-sfx/payload.zip")
            .exists());
        let entries = state.engine.list(&output, &OpenOptions::default()).unwrap();
        assert_eq!(entries[0].path.display, "notes/");
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        assert_eq!(done_result(&events, id).as_ref(), Some(result));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn batch_extract_runs_multiple_archives_as_one_job() {
        let dir = temp_dir("batch-extract");
        let src_a = dir.join("alpha");
        let src_b = dir.join("bravo");
        std::fs::create_dir_all(&src_a).unwrap();
        std::fs::create_dir_all(&src_b).unwrap();
        std::fs::write(src_a.join("one.txt"), b"alpha one").unwrap();
        std::fs::write(src_b.join("two.txt"), b"bravo two").unwrap();
        let zip_a = dir.join("alpha.zip");
        let zip_b = dir.join("bravo.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &zip_a,
                std::slice::from_ref(&src_a),
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        state
            .engine
            .create(
                &zip_b,
                std::slice::from_ref(&src_b),
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let mut zip_b_bytes = std::fs::read(&zip_b).unwrap();
        let central_start = zip_b_bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists in sample");
        zip_b_bytes.truncate(central_start);
        std::fs::write(&zip_b, zip_b_bytes).unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out_a = dir.join("out-a");
        let out_b = dir.join("out-b");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::BatchExtract {
                items: vec![
                    BatchExtractItem {
                        path: zip_a.to_string_lossy().into_owned(),
                        dest: out_a.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                    BatchExtractItem {
                        path: zip_b.to_string_lossy().into_owned(),
                        dest: out_b.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                ],
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out_a.join("alpha/one.txt")).unwrap(),
            b"alpha one"
        );
        assert_eq!(
            std::fs::read(out_b.join("bravo/two.txt")).unwrap(),
            b"bravo two"
        );
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["operation"], "batch_extract");
        assert_eq!(result["archives"], 2);
        assert_eq!(result["extracted"], 2);
        assert_eq!(result["failed"], 0);
        assert_eq!(result["outputs"].as_array().unwrap().len(), 2);
        assert_eq!(result["outputs"][0]["plan"]["layout"], "direct");
        assert_eq!(result["outputs"][0]["counts"]["created"], 1);
        assert_eq!(result["outputs"][0]["counts"]["failed"], 0);
        assert!(result["outputs"][0].get("structure").is_none());
        assert_eq!(
            result["outputs"][1]["structure"],
            "zip_local_headers_recovered"
        );
        assert_eq!(result["structure"], "zip_local_headers_recovered");
        assert_eq!(result["recovered_archives"], 1);
        assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
            && payload["id"] == id
            && payload["total"].as_u64().unwrap_or(0) == 2 * BATCH_PROGRESS_SCALE));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_job_reports_recovered_zip_structure() {
        let dir = temp_dir("extract-recovered-zip-structure");
        let archive = dir.join("missing-central-directory.zip");
        let destination = dir.join("output");
        let mut bytes = build_stored_zip(&[(b"recoverable.txt", b"recoverable payload")]);
        let central_start = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists in sample");
        bytes.truncate(central_start);
        std::fs::write(&archive, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: destination.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(destination.join("recoverable.txt")).unwrap(),
            b"recoverable payload"
        );
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["structure"], "zip_local_headers_recovered");
        assert_eq!(result["counts"]["created"], 1);
        assert_eq!(result["counts"]["failed"], 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn batch_extract_collapses_only_confirmed_volume_members() {
        let part1 = PathBuf::from("/archives/sample.part1.rar");
        let part2 = PathBuf::from("/archives/sample.part2.rar");
        let part3 = PathBuf::from("/archives/sample.part3.rar");
        let zip = PathBuf::from("/archives/other.zip");
        let source_set = ArchiveSourceSet::from_ordered_members(vec![
            part1.clone(),
            part2.clone(),
            part3.clone(),
        ])
        .unwrap();
        let make_item = |path: &Path, dest: &str| BatchExtractItem {
            path: path.to_string_lossy().into_owned(),
            dest: dest.to_owned(),
            encoding: None,
            password: None,
            best_effort: false,
        };
        let items = vec![
            make_item(&part3, "/output/part3"),
            make_item(&zip, "/output/zip"),
            make_item(&part1, "/output/part1"),
            make_item(&part2, "/output/part2"),
        ];
        let display_items = vec![
            make_item(Path::new("shown-part3.rar"), "/output/part3"),
            make_item(Path::new("shown-other.zip"), "/output/zip"),
            make_item(Path::new("shown-part1.rar"), "/output/part1"),
            make_item(Path::new("shown-part2.rar"), "/output/part2"),
        ];

        let normalized = normalize_batch_extract_items_with(&items, &display_items, |path| {
            if source_set.members().iter().any(|member| member == path) {
                Ok(Some(source_set.clone()))
            } else {
                Ok(None)
            }
        });

        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].execution.path, part1.to_string_lossy());
        assert_eq!(normalized[0].execution.dest, "/output/part1");
        assert_eq!(normalized[0].display.path, "shown-part1.rar");
        assert_eq!(normalized[1].execution.path, zip.to_string_lossy());
    }

    #[test]
    fn batch_extract_keeps_candidates_separate_when_source_probe_fails() {
        let make_item = |path: &str| BatchExtractItem {
            path: path.to_owned(),
            dest: format!("/output/{path}"),
            encoding: None,
            password: None,
            best_effort: false,
        };
        let items = vec![make_item("sample.part1.rar"), make_item("sample.part2.rar")];

        let normalized = normalize_batch_extract_items_with(&items, &items, |_| {
            Err(FormatError::CorruptArchive(
                "source set is not confirmed".into(),
            ))
        });

        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].execution.path, "sample.part1.rar");
        assert_eq!(normalized[1].execution.path, "sample.part2.rar");
    }

    #[test]
    fn batch_extract_prefers_a_selected_primary_after_native_volume_collapse() {
        let first = PathBuf::from("/archives/sample.z01");
        let second = PathBuf::from("/archives/sample.z02");
        let primary = PathBuf::from("/archives/sample.zip");
        let source_set = ArchiveSourceSet::from_primary_and_ordered_members(
            primary.clone(),
            vec![first.clone(), second.clone(), primary.clone()],
        )
        .unwrap();
        let make_item = |path: &Path| BatchExtractItem {
            path: path.to_string_lossy().into_owned(),
            dest: format!("/output/{}", path.file_name().unwrap().to_string_lossy()),
            encoding: None,
            password: None,
            best_effort: false,
        };
        let items = vec![make_item(&second), make_item(&primary), make_item(&first)];

        let normalized = normalize_batch_extract_items_with(&items, &items, |path| {
            if source_set.members().iter().any(|member| member == path) {
                Ok(Some(source_set.clone()))
            } else {
                Ok(None)
            }
        });

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].execution.path, primary.to_string_lossy());
        assert_eq!(normalized[0].execution.dest, "/output/sample.zip");
    }

    #[cfg(unix)]
    #[test]
    fn native_rar_family_is_listed_and_extracted_once_when_all_volumes_are_selected() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
        let dir = temp_dir("batch-native-rar");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        let tool = dir.join("fake-7z.sh");
        let log = dir.join("fake-7z.log");
        std::fs::write(&first, rar5_test_volume(0, true)).unwrap();
        std::fs::write(&second, rar5_test_volume(1, false)).unwrap();
        std::fs::write(
            &tool,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
case "$1" in
  l)
    cat <<'EOF'
Path = hello.txt
Folder = -
Size = 5
Packed Size = 5
CRC = 3610A686
Encrypted = -

EOF
    ;;
  x)
    printf 'hello'
    ;;
  *)
    exit 64
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
        let _tool_env = EnvRestore::set("SQUALLZ_7Z", &tool);
        let _log_env = EnvRestore::set("SQUALLZ_FAKE_7Z_LOG", &log);

        let state = Arc::new(AppState::new());
        let info = state.open_archive(&second, None, None).unwrap();
        assert_eq!(
            info.volumes,
            Some(vec![
                "sample.part1.rar".to_owned(),
                "sample.part2.rar".to_owned()
            ])
        );
        state.close_archive(info.id);
        std::fs::write(&log, b"").unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out_first = dir.join("out-first");
        let out_second = dir.join("out-second");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::BatchExtract {
                items: vec![
                    BatchExtractItem {
                        path: second.to_string_lossy().into_owned(),
                        dest: out_second.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                    BatchExtractItem {
                        path: first.to_string_lossy().into_owned(),
                        dest: out_first.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                ],
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Skip,
                smart: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out_first.join("hello.txt")).unwrap(),
            b"hello"
        );
        assert!(!out_second.exists());
        let recorded_events = sink.events.lock().unwrap().clone();
        let result = done_result(&recorded_events, id).unwrap();
        assert_eq!(result["archives"], 1);
        assert_eq!(result["selected_archives"], 2);
        assert_eq!(result["collapsed_volumes"], 1);
        assert_eq!(result["extracted"], 1);
        assert_eq!(result["failed"], 0);
        assert!(recorded_events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == id
                && payload["total"].as_u64() == Some(BATCH_PROGRESS_SCALE)
        }));
        let tool_log = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            tool_log
                .lines()
                .filter(|line| line.starts_with("l -slt"))
                .count(),
            1
        );
        assert_eq!(
            tool_log
                .lines()
                .filter(|line| line.starts_with("x -so"))
                .count(),
            1
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_split_zip_family_opens_from_middle_and_extracts_once_via_primary() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
        let dir = temp_dir("batch-native-split-zip");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let primary = dir.join("sample.zip");
        let tool = dir.join("fake-7z.sh");
        let log = dir.join("fake-7z.log");
        std::fs::write(&first, b"first split ZIP data volume").unwrap();
        std::fs::write(&second, b"second split ZIP data volume").unwrap();
        std::fs::write(&primary, split_zip_test_final_volume(2)).unwrap();
        std::fs::write(
            &tool,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
archive="$3"
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.zip"
test -f "$stage/archive.z01"
test -f "$stage/archive.z02"
test -f "$stage/archive.zip"
case "$1" in
  l)
    cat <<'EOF'
Path = hello.txt
Folder = -
Size = 5
Packed Size = 5
CRC = 3610A686
Encrypted = -

EOF
    ;;
  x)
    printf 'hello'
    ;;
  *)
    exit 64
    ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&tool, permissions).unwrap();
        let _tool_env = EnvRestore::set("SQUALLZ_7Z", &tool);
        let _log_env = EnvRestore::set("SQUALLZ_FAKE_7Z_LOG", &log);

        let state = Arc::new(AppState::new());
        let info = state.open_archive(&second, None, None).unwrap();
        assert_eq!(
            info.volumes,
            Some(vec![
                "sample.z01".to_owned(),
                "sample.z02".to_owned(),
                "sample.zip".to_owned(),
            ])
        );
        state.close_archive(info.id);
        std::fs::write(&log, b"").unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out_first = dir.join("out-first");
        let out_second = dir.join("out-second");
        let out_primary = dir.join("out-primary");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::BatchExtract {
                items: vec![
                    BatchExtractItem {
                        path: second.to_string_lossy().into_owned(),
                        dest: out_second.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                    BatchExtractItem {
                        path: first.to_string_lossy().into_owned(),
                        dest: out_first.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                    BatchExtractItem {
                        path: primary.to_string_lossy().into_owned(),
                        dest: out_primary.to_string_lossy().into_owned(),
                        encoding: None,
                        password: None,
                        best_effort: false,
                    },
                ],
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Skip,
                smart: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out_primary.join("hello.txt")).unwrap(),
            b"hello"
        );
        assert!(!out_first.exists());
        assert!(!out_second.exists());
        let recorded_events = sink.events.lock().unwrap().clone();
        let result = done_result(&recorded_events, id).unwrap();
        assert_eq!(result["archives"], 1);
        assert_eq!(result["selected_archives"], 3);
        assert_eq!(result["collapsed_volumes"], 2);
        assert_eq!(result["extracted"], 1);
        assert_eq!(result["failed"], 0);
        let tool_log = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            tool_log
                .lines()
                .filter(|line| line.starts_with("l -slt"))
                .count(),
            1
        );
        assert_eq!(
            tool_log
                .lines()
                .filter(|line| line.starts_with("x -so"))
                .count(),
            1
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancel_password_prompt_reports_cancelled_without_poll_delay() {
        let dir = temp_dir("cancel-password-latency");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&archive),
            SettingsDto::default(),
        );

        wait_for_password_prompt(&sink, id);
        let cancel_start = Instant::now();
        manager.cancel_for_window("main", id).unwrap();
        wait_for_state(&sink, id, "cancelled", std::time::Duration::from_secs(2));
        let cancel_ms = cancel_start.elapsed().as_millis();
        println!("JOB_METRIC gui_cancel_prompt_to_cancelled_ms={cancel_ms}");
        assert!(
            cancel_ms <= 120,
            "password-prompt cancel took {cancel_ms}ms; expected sub-120ms state feedback"
        );

        manager.wait_idle();
        let events = sink.events.lock().unwrap();
        assert!(states_of(&events, id).contains(&"cancelled".to_owned()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releasing_window_cancels_password_wait_and_advances_other_owner() {
        let dir = temp_dir("release-password-owner");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let next_input = dir.join("next.txt");
        std::fs::write(&next_input, b"next owner").unwrap();
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let waiting_id = manager.submit_for_test_window(
            "task-password-1".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&archive),
            SettingsDto::default(),
        );
        wait_for_password_prompt(&sink, waiting_id);

        let next_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            checksum_job(&next_input),
            SettingsDto::default(),
        );

        assert_eq!(manager.release_window("task-password-other"), 0);
        assert_eq!(manager.snapshot(waiting_id).unwrap().state, "running");
        manager.pause_for_window("main", waiting_id).unwrap();
        assert_eq!(manager.snapshot(waiting_id).unwrap().state, "paused");
        assert_eq!(manager.release_window("task-password-1"), 1);
        wait_for_snapshot_state(
            &manager,
            waiting_id,
            "cancelled",
            std::time::Duration::from_secs(2),
        );
        wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
        manager.wait_idle();

        assert_eq!(manager.release_window("task-password-1"), 0);
        let recorded = sink.events.lock().unwrap().clone();
        assert_eq!(
            states_of(&recorded, waiting_id),
            vec!["queued", "running", "paused", "cancelled"]
        );
        assert_eq!(
            states_of(&recorded, next_id),
            vec!["queued", "running", "done"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releasing_window_cancels_queued_jobs_and_rejects_late_submission() {
        let dir = temp_dir("release-queued-owner");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let queued_input = dir.join("queued.txt");
        std::fs::write(&queued_input, b"must not be archived").unwrap();
        let queued_output = dir.join("cancelled.zip");
        let next_input = dir.join("next.txt");
        std::fs::write(&next_input, b"next in fifo").unwrap();
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let blocker_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&archive),
            SettingsDto::default(),
        );
        wait_for_password_prompt(&sink, blocker_id);

        let queued_id = manager.submit_for_test_window(
            "task-queued-1".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            compress_file_job(&queued_input, &queued_output),
            SettingsDto::default(),
        );
        let next_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            checksum_job(&next_input),
            SettingsDto::default(),
        );

        assert_eq!(manager.release_window("task-queued-1"), 1);
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "cancelled");
        let late_id = manager.submit_for_test_window(
            "task-queued-1".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            checksum_job(&queued_input),
            SettingsDto::default(),
        );
        assert_eq!(manager.snapshot(late_id).unwrap().state, "cancelled");

        manager.cancel_for_window("main", blocker_id).unwrap();
        wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
        manager.wait_idle();

        assert!(!queued_output.exists());
        let recorded = sink.events.lock().unwrap().clone();
        assert!(!states_of(&recorded, queued_id)
            .iter()
            .any(|state| matches!(state.as_str(), "running" | "done" | "failed")));
        assert!(!states_of(&recorded, late_id)
            .iter()
            .any(|state| matches!(state.as_str(), "running" | "done" | "failed")));
        assert_eq!(
            states_of(&recorded, next_id),
            vec!["queued", "running", "done"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releasing_window_serializes_with_an_in_flight_submission() {
        let dir = temp_dir("release-submit-race");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let manager = Arc::new(JobManager::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let start = Arc::new(std::sync::Barrier::new(3));

        let submit_manager = Arc::clone(&manager);
        let submit_state = Arc::clone(&state);
        let submit_events = Arc::clone(&events);
        let submit_start = Arc::clone(&start);
        let submitter = std::thread::spawn(move || {
            submit_start.wait();
            submit_manager.submit_for_test_window(
                "task-race-1".into(),
                submit_state,
                submit_events,
                password_test_job(&archive),
                SettingsDto::default(),
            )
        });

        let release_manager = Arc::clone(&manager);
        let release_start = Arc::clone(&start);
        let releaser = std::thread::spawn(move || {
            release_start.wait();
            release_manager.release_window("task-race-1")
        });

        start.wait();
        let id = submitter.join().unwrap();
        let released = releaser.join().unwrap();
        assert!(released <= 1);
        wait_for_snapshot_state(&manager, id, "cancelled", std::time::Duration::from_secs(2));
        manager.wait_idle();

        let recorded = sink.events.lock().unwrap().clone();
        assert!(!states_of(&recorded, id)
            .iter()
            .any(|state| matches!(state.as_str(), "done" | "failed")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releasing_window_aborts_conflict_without_touching_existing_file() {
        let dir = temp_dir("release-conflict-owner");
        let archive = dir.join("conflict.zip");
        std::fs::write(&archive, build_stored_zip(&[(b"same.txt", b"new bytes")])).unwrap();
        let output = dir.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let existing = output.join("same.txt");
        std::fs::write(&existing, b"original bytes").unwrap();
        let state = Arc::new(AppState::new());
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let conflict_id = manager.submit_for_test_window(
            "task-conflict-1".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: output.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Ask,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        wait_for_event(
            &sink,
            std::time::Duration::from_secs(2),
            |(name, payload)| name == EV_ASK_CONFLICT && payload["id"] == conflict_id,
        );

        let next_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            checksum_job(&existing),
            SettingsDto::default(),
        );
        assert_eq!(manager.release_window("task-conflict-1"), 1);
        wait_for_snapshot_state(
            &manager,
            conflict_id,
            "cancelled",
            std::time::Duration::from_secs(2),
        );
        wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
        manager.wait_idle();

        assert_eq!(std::fs::read(&existing).unwrap(), b"original bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_all_releases_questions_and_queued_jobs_for_exit() {
        let dir = temp_dir("cancel-all-exit");
        let state = Arc::new(AppState::new());
        let archive = create_password_protected_zip(&dir, &state);
        let queued_input = dir.join("queued.txt");
        std::fs::write(&queued_input, b"queued on exit").unwrap();
        let queued_output = dir.join("queued.zip");
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let waiting_id = manager.submit_for_test_window(
            "task-exit-1".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&archive),
            SettingsDto::default(),
        );
        wait_for_password_prompt(&sink, waiting_id);
        let queued_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            compress_file_job(&queued_input, &queued_output),
            SettingsDto::default(),
        );

        assert_eq!(manager.cancel_all(), 2);
        assert_eq!(manager.cancel_all(), 0);
        let late_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            checksum_job(&queued_input),
            SettingsDto::default(),
        );
        assert_eq!(manager.snapshot(late_id).unwrap().state, "cancelled");
        manager.wait_idle();
        assert_eq!(manager.snapshot(waiting_id).unwrap().state, "cancelled");
        assert_eq!(manager.snapshot(queued_id).unwrap().state, "cancelled");
        assert!(!queued_output.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn completed_jobs_are_written_to_backend_audit_log() {
        let dir = temp_dir("audit-log");
        let src = dir.join("secret-source");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello audit").unwrap();
        let zip = dir.join("audited.zip");
        let audit_path = dir.join("audit").join("operation-audit.jsonl");
        let audit = Arc::new(OperationAudit::with_path(audit_path.clone(), 20));
        let manager = JobManager::with_audit(Arc::clone(&audit));
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Compress {
                inputs: vec![src.to_string_lossy().into_owned()],
                dest: zip.to_string_lossy().into_owned(),
                level: 5,
                password: Some("audit-password-must-not-appear".into()),
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                sqz_inner_format: None,
                sfx_target: None,
                completion: squallz_core::CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: false,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let recent = audit.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, id);
        assert_eq!(recent[0].kind, "compress");
        assert_eq!(recent[0].state, "done");
        assert!(recent[0].detail.contains("audited.zip"));
        assert!(!recent[0].detail.contains("audit-password"));
        assert!(std::fs::read_to_string(audit_path)
            .unwrap()
            .contains("\"kind\":\"compress\""));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_nested_job_extracts_inner_archive() {
        let dir = temp_dir("extract-nested");
        let inner_src = dir.join("inner-src");
        std::fs::create_dir_all(&inner_src).unwrap();
        std::fs::write(inner_src.join("hello.txt"), b"hello nested job").unwrap();
        let inner_name = "inner-job-cleanup.zip";
        let inner = dir.join(inner_name);
        let outer = dir.join("outer.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &inner,
                std::slice::from_ref(&inner_src),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        state
            .engine
            .create(
                &outer,
                std::slice::from_ref(&inner),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ExtractNested {
                outer_path: outer.to_string_lossy().into_owned(),
                entry_path: inner_name.into(),
                dest: out.to_string_lossy().into_owned(),
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: true,
                encoding: None,
                password: None,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out.join("inner-src/hello.txt")).unwrap(),
            b"hello nested job"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        assert_real_current_file_progress(&events, id, "extract nested");
        let expected_dest = out.to_string_lossy().into_owned();
        assert_eq!(
            done_result(&events, id)
                .and_then(|value| value["dest"].as_str().map(str::to_owned))
                .as_deref(),
            Some(expected_dest.as_str())
        );
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["plan"]["destination"], expected_dest);
        assert_eq!(result["plan"]["layout"], "direct");
        assert_eq!(result["counts"]["selected_entries"], 2);
        assert_eq!(result["counts"]["created"], 1);
        assert_eq!(result["counts"]["directories"], 1);
        assert_eq!(result["counts"]["failed"], 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_nested_job_prompts_separately_for_outer_and_inner_passwords() {
        let dir = temp_dir("extract-nested-passwords");
        let inner_src = dir.join("inner-secret-src");
        std::fs::create_dir_all(&inner_src).unwrap();
        std::fs::write(inner_src.join("secret.txt"), b"two-stage password").unwrap();
        let inner_name = "inner-protected.zip";
        let inner = dir.join(inner_name);
        let outer_name = "outer-protected.zip";
        let outer = dir.join(outer_name);
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &inner,
                std::slice::from_ref(&inner_src),
                &CreateOptions {
                    password: Some(Password::new("inner-secret")),
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        state
            .engine
            .create(
                &outer,
                std::slice::from_ref(&inner),
                &CreateOptions {
                    password: Some(Password::new("outer-secret")),
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");
        let id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ExtractNested {
                outer_path: outer.to_string_lossy().into_owned(),
                entry_path: inner_name.into(),
                dest: out.to_string_lossy().into_owned(),
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: true,
                encoding: None,
                password: None,
                best_effort: false,
            },
            SettingsDto::default(),
        );

        wait_for_password_prompt_count(&sink, id, 1);
        {
            let recorded = sink.events.lock().unwrap();
            let first = recorded
                .iter()
                .find(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
                .unwrap();
            assert_eq!(first.1["name"], outer_name);
        }
        manager
            .answer_password_for_window("main", id, Some("outer-secret".into()))
            .unwrap();

        wait_for_password_prompt_count(&sink, id, 2);
        {
            let recorded = sink.events.lock().unwrap();
            let prompts = recorded
                .iter()
                .filter(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
                .collect::<Vec<_>>();
            assert_eq!(prompts[1].1["name"], inner_name);
        }
        manager
            .answer_password_for_window("main", id, Some("inner-secret".into()))
            .unwrap();
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out.join("inner-secret-src/secret.txt")).unwrap(),
            b"two-stage password"
        );
        assert_eq!(state.cached_password_paths(), vec![outer.clone()]);
        let recorded = sink.events.lock().unwrap();
        assert_eq!(states_of(&recorded, id), vec!["queued", "running", "done"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_nested_job_applies_safety_limit_before_writing_temp_archive() {
        let dir = temp_dir("extract-nested-limit");
        let inner_src = dir.join("inner-src");
        std::fs::create_dir_all(&inner_src).unwrap();
        std::fs::write(inner_src.join("payload.txt"), b"larger than one byte").unwrap();
        let inner_name = "inner-limited.zip";
        let inner = dir.join(inner_name);
        let outer = dir.join("outer.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &inner,
                std::slice::from_ref(&inner_src),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        state
            .engine
            .create(
                &outer,
                std::slice::from_ref(&inner),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ExtractNested {
                outer_path: outer.to_string_lossy().into_owned(),
                entry_path: inner_name.into(),
                dest: out.to_string_lossy().into_owned(),
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: true,
                encoding: None,
                password: None,
                best_effort: false,
            },
            SettingsDto {
                safety_max_output_bytes: Some(1),
                ..SettingsDto::default()
            },
        );
        manager.wait_idle();

        let recorded = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded, id),
            vec!["queued", "running", "failed"]
        );
        let failure = recorded
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
            })
            .unwrap();
        assert_eq!(failure.1["error"]["key"], "error.resource_limit");
        assert!(!out.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn queued_opaque_nested_source_stays_leased_and_never_reaches_public_state() {
        let dir = temp_dir("queued-nested-lease");
        let inner_src = dir.join("inner-src");
        std::fs::create_dir_all(&inner_src).unwrap();
        std::fs::write(inner_src.join("leased.txt"), b"leased source").unwrap();
        let inner_name = "inner-leased.zip";
        let inner = dir.join(inner_name);
        let outer = dir.join("outer-physical.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &inner,
                std::slice::from_ref(&inner_src),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        state
            .engine
            .create(
                &outer,
                std::slice::from_ref(&inner),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let previews = PreviewSessionManager::new().unwrap();
        let preview_root = previews.root_path().unwrap().to_path_buf();
        let reservation = previews.reserve("lease-test").unwrap();
        let mut pending = tempfile::Builder::new()
            .prefix("owned-outer-")
            .suffix(".zip")
            .tempfile_in(reservation.workspace_path().unwrap())
            .unwrap();
        let mut source_file = fs::File::open(&outer).unwrap();
        let size = std::io::copy(&mut source_file, pending.as_file_mut()).unwrap();
        pending.as_file_mut().flush().unwrap();
        let physical_path = pending.path().to_path_buf();
        let display_path = dir
            .join("displayed-outer.zip")
            .to_string_lossy()
            .into_owned();
        let archive = state
            .open_archive_with_owned_temp_and_entry_limit(
                "lease-test",
                pending.into_temp_path(),
                reservation,
                size,
                display_path.clone(),
                "displayed-outer.zip".into(),
                squallz_core::api::SafetyLimits::default().max_entries,
            )
            .unwrap();

        let blocker = create_password_protected_zip(&dir.join("blocker"), &state);
        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");
        let nested_job = JobSpec::ExtractNested {
            outer_path: archive.source.clone(),
            entry_path: inner_name.into(),
            dest: out.to_string_lossy().into_owned(),
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: true,
            encoding: None,
            password: None,
            best_effort: false,
        };
        let next_id = manager.next_id.load(Ordering::Relaxed);
        let foreign_error = manager
            .submit_for_window(
                "other-window".into(),
                Arc::clone(&state),
                Arc::clone(&events),
                nested_job.clone(),
                SettingsDto::default(),
            )
            .unwrap_err();
        assert_eq!(foreign_error.to_string(), "archive is no longer available");
        assert_eq!(manager.next_id.load(Ordering::Relaxed), next_id);
        assert!(manager.audit.recent(10).is_empty());
        assert!(sink.events.lock().unwrap().is_empty());

        let blocker_id = manager.submit_for_test_window(
            "main".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            password_test_job(&blocker),
            SettingsDto::default(),
        );
        wait_for_password_prompt(&sink, blocker_id);

        let id = manager.submit_for_test_window(
            "lease-test".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            nested_job,
            SettingsDto::default(),
        );
        state.close_archive_for_window("lease-test", archive.id);

        assert_eq!(manager.snapshot(id).unwrap().state, "queued");
        assert!(physical_path.exists());
        manager.cancel_for_window("main", blocker_id).unwrap();
        wait_for_state(&sink, id, "done", std::time::Duration::from_secs(2));
        manager.wait_idle();

        assert_eq!(
            std::fs::read(out.join("inner-src/leased.txt")).unwrap(),
            b"leased source"
        );
        assert!(!physical_path.exists());
        let snapshot = serde_json::to_string(&manager.snapshot(id).unwrap()).unwrap();
        let events_json = serde_json::to_string(&*sink.events.lock().unwrap()).unwrap();
        let audit = manager
            .audit
            .recent(10)
            .into_iter()
            .find(|record| record.id == id)
            .unwrap();
        let audit_json = serde_json::to_string(&audit).unwrap();
        for private in [
            physical_path.to_string_lossy().into_owned(),
            preview_root.to_string_lossy().into_owned(),
            archive.source,
        ] {
            assert!(!snapshot.contains(&private), "snapshot exposed {private}");
            assert!(!events_json.contains(&private), "event exposed {private}");
            assert!(!audit_json.contains(&private), "audit exposed {private}");
        }
        assert!(snapshot.contains(&display_path));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Convert jobs use the same GUI queue path as the dialog submits.
    #[test]
    fn convert_job_round_trip_through_queue() {
        let dir = temp_dir("convert");
        let src_dir = dir.join("data");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("hello.txt"), b"hello from convert").unwrap();
        let zip = dir.join("source.zip");
        let sevenz = dir.join("converted.7z");

        AppState::new()
            .engine
            .create(
                &zip,
                &[src_dir],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Convert {
                src: zip.to_string_lossy().into_owned(),
                dest: sevenz.to_string_lossy().into_owned(),
                level: 6,
                src_encoding: None,
                src_password: None,
                dest_password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(sevenz.exists());
        let entries = AppState::new()
            .engine
            .list(&sevenz, &OpenOptions::default())
            .unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.path.display == "data/hello.txt"));
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "done"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn convert_job_reports_split_output_set() {
        let dir = temp_dir("convert-split");
        let input = dir.join("payload.txt");
        let source = dir.join("source.zip");
        let destination = dir.join("converted.7z");
        let primary = dir.join("converted.7z.001");
        let second = dir.join("converted.7z.002");
        write_incompressible_file(&input, 700 * 1024);

        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &source,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink;
        let id = manager.submit(
            Arc::clone(&state),
            events,
            JobSpec::Convert {
                src: source.to_string_lossy().into_owned(),
                dest: destination.to_string_lossy().into_owned(),
                level: 6,
                src_encoding: None,
                src_password: None,
                dest_password: Some("destination secret".into()),
                encrypt_names: true,
                split_size: Some(256 * 1024),
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(!destination.exists());
        assert!(primary.is_file());
        assert!(second.is_file());
        let snapshot = manager.snapshot(id).unwrap();
        assert_eq!(snapshot.state, "done");
        let result = snapshot.result.as_ref().unwrap();
        assert_eq!(result["operation"], "convert");
        assert_eq!(result["split"], true);
        assert!(result["volume_count"]
            .as_u64()
            .is_some_and(|count| count >= 2));
        assert_eq!(
            result["primary_output"].as_str(),
            Some(primary.to_string_lossy().as_ref())
        );
        assert!(result["outputs"]
            .as_array()
            .is_some_and(|outputs| outputs.len() >= 2));
        assert!(state
            .engine
            .list(&primary, &OpenOptions::default())
            .is_err());
        let entries = state
            .engine
            .list(
                &primary,
                &OpenOptions {
                    password: Some(Password::new("destination secret")),
                    ..OpenOptions::default()
                },
            )
            .unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.path.display == "payload.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repair_zip_job_rebuilds_missing_central_directory() {
        let dir = temp_dir("repair-zip");
        let damaged = dir.join("missing-central.zip");
        let repaired = dir.join("rebuilt.zip");
        let mut bytes = build_stored_zip(&[(b"hello.txt", b"hello from zip repair")]);
        let central_start = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists in sample");
        bytes.truncate(central_start);
        std::fs::write(&damaged, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairZip {
                src: damaged.to_string_lossy().into_owned(),
                dest: repaired.to_string_lossy().into_owned(),
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let rebuilt = std::fs::read(&repaired).unwrap();
        assert!(rebuilt.windows(4).any(|window| window == b"PK\x01\x02"));
        assert!(rebuilt.windows(4).any(|window| window == b"PK\x05\x06"));
        let out = dir.join("out");
        state
            .engine
            .extract(
                &repaired,
                &out,
                None,
                &OpenOptions::default(),
                &squallz_core::api::ExtractOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(out.join("hello.txt")).unwrap(),
            b"hello from zip repair"
        );
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "done"]
        );
        let result = done_result(&recorded_events, id).unwrap();
        assert_eq!(result["operation"].as_str(), Some("repair_zip"));
        assert_eq!(result["tool"].as_str(), Some("zip-local-header-rebuild"));
        assert_eq!(
            result["dest"].as_str(),
            Some(repaired.to_string_lossy().as_ref())
        );
        assert_eq!(result["source_entries"].as_u64(), Some(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repair_zip_job_refuses_damaged_local_header_payloads() {
        let dir = temp_dir("repair-zip-damaged");
        let damaged = dir.join("damaged-missing-central.zip");
        let repaired = dir.join("must-not-exist.zip");
        let mut bytes = build_stored_zip(&[(b"bad.txt", b"visible payload")]);
        let central_start = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists in sample");
        bytes.truncate(central_start);
        let payload_pos = bytes
            .windows(b"visible payload".len())
            .position(|window| window == b"visible payload")
            .expect("payload exists in sample");
        bytes[payload_pos] ^= 0xA5;
        std::fs::write(&damaged, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairZip {
                src: damaged.to_string_lossy().into_owned(),
                dest: repaired.to_string_lossy().into_owned(),
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(!repaired.exists());
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "failed"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn convert_job_streams_rar_bridge_to_zip() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
        let dir = temp_dir("convert-rar");
        let rar = dir.join("source.rar");
        let zip = dir.join("converted.zip");
        let tool = dir.join("fake-bsdtar.sh");
        std::fs::write(&rar, b"Rar!\x1A\x07\x01\x00").unwrap();

        std::fs::write(
            &tool,
            r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          26 Jan  1  2020 hello.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  for arg in "$@"; do
    last="$arg"
  done
  case "$last" in
    hello.txt) printf 'hello from gui rar convert' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();
        let _tool_env = EnvRestore::set("SQUALLZ_BSDTAR", &tool);

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Convert {
                src: rar.to_string_lossy().into_owned(),
                dest: zip.to_string_lossy().into_owned(),
                level: 6,
                src_encoding: None,
                src_password: None,
                dest_password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(zip.is_file(), "converted ZIP missing");
        let entries = state.engine.list(&zip, &OpenOptions::default()).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.path.display == "hello.txt"));
        let out = dir.join("out");
        state
            .engine
            .extract(
                &zip,
                &out,
                None,
                &OpenOptions::default(),
                &squallz_core::api::ExtractOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(out.join("hello.txt")).unwrap(),
            b"hello from gui rar convert"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// SQZ export is a named GUI job so the desktop app exposes a clear
    /// no-lock-in action instead of forcing users through generic conversion.
    #[test]
    fn export_sqz_job_round_trip_through_queue() {
        let dir = temp_dir("export-sqz");
        let src_dir = dir.join("data");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("hello.txt"), b"hello from sqz export").unwrap();
        let sqz = dir.join("source.sqz");
        let zip = dir.join("exported.zip");

        AppState::new()
            .engine
            .create(
                &sqz,
                &[src_dir],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ExportSqz {
                src: sqz.to_string_lossy().into_owned(),
                dest: zip.to_string_lossy().into_owned(),
                level: 6,
                dest_password: None,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(zip.exists());
        let entries = AppState::new()
            .engine
            .list(&zip, &OpenOptions::default())
            .unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.path.display == "data/hello.txt"));
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "done"]
        );
        let result = done_result(&recorded_events, id).unwrap();
        assert_eq!(
            result["dest"].as_str(),
            Some(zip.to_string_lossy().as_ref())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn conversion_jobs_without_replace_permission_preserve_existing_outputs() {
        let dir = temp_dir("conversion-output-policy");
        let input = dir.join("hello.txt");
        let zip = dir.join("source.zip");
        let sqz = dir.join("source.sqz");
        let converted = dir.join("converted.7z");
        let exported = dir.join("exported.zip");
        std::fs::write(&input, b"output policy").unwrap();

        let state = Arc::new(AppState::new());
        for archive in [&zip, &sqz] {
            state
                .engine
                .create(
                    archive,
                    std::slice::from_ref(&input),
                    &CreateOptions::default(),
                    &squallz_core::api::NoProgress,
                    &ControlToken::new(),
                )
                .unwrap();
        }
        std::fs::write(&converted, b"keep converted output").unwrap();
        std::fs::write(&exported, b"keep exported output").unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let convert_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Convert {
                src: zip.to_string_lossy().into_owned(),
                dest: converted.to_string_lossy().into_owned(),
                level: 6,
                src_encoding: None,
                src_password: None,
                dest_password: None,
                encrypt_names: false,
                split_size: None,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        let export_id = manager.submit(
            state,
            events,
            JobSpec::ExportSqz {
                src: sqz.to_string_lossy().into_owned(),
                dest: exported.to_string_lossy().into_owned(),
                level: 6,
                dest_password: None,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(std::fs::read(&converted).unwrap(), b"keep converted output");
        assert_eq!(std::fs::read(&exported).unwrap(), b"keep exported output");
        assert_eq!(manager.snapshot(convert_id).unwrap().state, "failed");
        assert_eq!(manager.snapshot(export_id).unwrap().state, "failed");
        let recorded_events = sink.events.lock().unwrap();
        for id in [convert_id, export_id] {
            assert_eq!(
                states_of(&recorded_events, id),
                vec!["queued", "running", "failed"]
            );
            let failed = recorded_events
                .iter()
                .find(|(name, payload)| {
                    name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
                })
                .unwrap();
            assert_eq!(
                failed.1["error"]["key"].as_str(),
                Some("error.output_exists")
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_sqz_job_keeps_output_changed_after_confirmation() {
        let dir = temp_dir("export-sqz-stale-output");
        let input = dir.join("hello.txt");
        std::fs::write(&input, b"hello from protected export").unwrap();
        let sqz = dir.join("source.sqz");
        let output = dir.join("exported.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &sqz,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        std::fs::write(&output, b"output presented for confirmation").unwrap();
        let guard = squallz_core::inspect_create_destination(&output, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        std::fs::write(&output, b"newer output from another app").unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            state,
            events,
            JobSpec::ExportSqz {
                src: sqz.to_string_lossy().into_owned(),
                dest: output.to_string_lossy().into_owned(),
                level: 6,
                dest_password: None,
                replace_existing: true,
                replacement_guard: Some(guard),
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(
            std::fs::read(&output).unwrap(),
            b"newer output from another app"
        );
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains(".convert-")
                || name.contains("replace-backup")
                || name.starts_with(".squallz-update-")
        }));
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "failed"]
        );
        let failed = recorded_events
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
            })
            .unwrap();
        assert_eq!(
            failed.1["error"]["key"].as_str(),
            Some("error.destination_changed")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_sqz_source_jobs_accept_first_volume() {
        let dir = temp_dir("split-sqz-source");
        let input = dir.join("data.bin");
        write_incompressible_file(&input, 100 * 1024);
        let split_sqz = dir.join("source.sqz");
        AppState::new()
            .engine
            .create(
                &split_sqz,
                std::slice::from_ref(&input),
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    split_size: Some(30 * 1024),
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(!split_sqz.exists());
        let first = dir.join("source.sqz.001");
        assert!(first.is_file());
        assert!(dir.join("source.sqz.002").is_file());

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let zip = dir.join("exported.zip");
        let repaired = dir.join("repaired.sqz");

        let export_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ExportSqz {
                src: first.to_string_lossy().into_owned(),
                dest: zip.to_string_lossy().into_owned(),
                level: 6,
                dest_password: None,
                replace_existing: false,
                replacement_guard: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        std::fs::remove_file(dir.join("source.sqz.002")).unwrap();
        let repair_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairSqz {
                src: first.to_string_lossy().into_owned(),
                dest: repaired.to_string_lossy().into_owned(),
                level: 6,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let engine = AppState::new().engine;
        let entries = engine.list(&zip, &OpenOptions::default()).unwrap();
        assert!(entries.iter().any(|entry| entry.path.display == "data.bin"));
        let report = engine
            .test_summary(
                &repaired,
                &OpenOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(report.is_ok(), "problems: {:?}", report.problems);

        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, export_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&recorded_events, repair_id),
            vec!["queued", "running", "done"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn repair_sqz_job_rewrites_recovered_container() {
        let dir = temp_dir("repair-sqz");
        let src_dir = dir.join("data");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("hello.txt"), b"hello from sqz repair").unwrap();
        let damaged = dir.join("damaged.sqz");
        let repaired = dir.join("repaired.sqz");

        AppState::new()
            .engine
            .create(
                &damaged,
                &[src_dir],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        corrupt_sqz_payload_byte(&damaged);

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairSqz {
                src: damaged.to_string_lossy().into_owned(),
                dest: repaired.to_string_lossy().into_owned(),
                level: 6,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(repaired.exists());
        let report = AppState::new()
            .engine
            .test_summary(
                &repaired,
                &OpenOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(report.is_ok(), "problems: {:?}", report.problems);
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "done"]
        );
        let result = done_result(&recorded_events, id).unwrap();
        assert_eq!(
            result["dest"].as_str(),
            Some(repaired.to_string_lossy().as_ref())
        );
        assert_eq!(result["in_place"].as_bool(), Some(false));
        assert_eq!(
            result["recovery"]["scheme"].as_str(),
            Some("sqz-embedded-rs-gf8")
        );
        assert_eq!(result["recovery"]["damaged_blocks"].as_u64(), Some(1));
        assert_eq!(result["recovery"]["repaired_blocks"].as_u64(), Some(1));
        assert_eq!(result["recovery"]["unrepaired_blocks"].as_u64(), Some(0));
        assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
        drop(recorded_events);

        let in_place_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairSqz {
                src: damaged.to_string_lossy().into_owned(),
                dest: damaged.to_string_lossy().into_owned(),
                level: 6,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let report = AppState::new()
            .engine
            .test_summary(
                &damaged,
                &OpenOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(report.is_ok(), "problems: {:?}", report.problems);
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, in_place_id),
            vec!["queued", "running", "done"]
        );
        let result = done_result(&recorded_events, in_place_id).unwrap();
        assert_eq!(
            result["dest"].as_str(),
            Some(damaged.to_string_lossy().as_ref())
        );
        assert_eq!(result["in_place"].as_bool(), Some(true));
        assert_eq!(result["recovery"]["damaged_blocks"].as_u64(), Some(1));
        assert_eq!(result["recovery"]["repaired_blocks"].as_u64(), Some(1));
        assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
        drop(recorded_events);

        let split_input = dir.join("large.bin");
        write_incompressible_file(&split_input, 100 * 1024);
        let split_base = dir.join("split-damaged.sqz");
        state
            .engine
            .create(
                &split_base,
                std::slice::from_ref(&split_input),
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    split_size: Some(30 * 1024),
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let split_first = dir.join("split-damaged.sqz.001");
        std::fs::remove_file(dir.join("split-damaged.sqz.002")).unwrap();
        let split_repaired = dir.join("split-repaired.sqz");

        let split_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairSqz {
                src: split_first.to_string_lossy().into_owned(),
                dest: split_repaired.to_string_lossy().into_owned(),
                level: 6,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let report = AppState::new()
            .engine
            .test_summary(
                &split_repaired,
                &OpenOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(report.is_ok(), "problems: {:?}", report.problems);
        let recorded_events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&recorded_events, split_id),
            vec!["queued", "running", "done"]
        );
        let result = done_result(&recorded_events, split_id).unwrap();
        assert_eq!(
            result["dest"].as_str(),
            Some(split_repaired.to_string_lossy().as_ref())
        );
        assert_eq!(result["in_place"].as_bool(), Some(false));
        assert_eq!(
            result["recovery"]["scheme"].as_str(),
            Some("sqz-embedded-rs-gf8")
        );
        assert_eq!(result["recovery"]["unrepaired_blocks"].as_u64(), Some(0));
        assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_job_deletes_selected_entry() {
        let dir = temp_dir("update-delete");
        let src = dir.join("data");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("keep.txt"), b"keep").unwrap();
        std::fs::write(src.join("drop.txt"), b"drop").unwrap();
        let archive = dir.join("out.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&src),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Update {
                path: archive.to_string_lossy().into_owned(),
                add: vec![],
                delete: vec!["data/drop.txt".into()],
                rename: vec![],
                mkdir: vec![],
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                password: None,
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let entries = state
            .engine
            .list(&archive, &OpenOptions::default())
            .unwrap();
        assert!(entries.iter().any(|e| e.path.display == "data/keep.txt"));
        assert!(!entries.iter().any(|e| e.path.display == "data/drop.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_job_add_directory_applies_content_policy_and_explicit_excludes() {
        let dir = temp_dir("update-add-excludes");
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("base.txt"), b"base").unwrap();
        let archive = dir.join("out.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&seed),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let extra = dir.join("extra");
        std::fs::create_dir_all(extra.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(extra.join("__MACOSX")).unwrap();
        std::fs::write(extra.join("keep.txt"), b"keep").unwrap();
        std::fs::write(extra.join(".env"), b"MODE=test").unwrap();
        std::fs::write(extra.join(".DS_Store"), b"finder metadata").unwrap();
        std::fs::write(extra.join("._keep.txt"), b"appledouble metadata").unwrap();
        std::fs::write(extra.join("__MACOSX/metadata"), b"metadata").unwrap();
        std::fs::write(extra.join("drop.tmp"), b"drop").unwrap();
        std::fs::write(extra.join("node_modules/pkg/index.js"), b"drop").unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Update {
                path: archive.to_string_lossy().into_owned(),
                add: vec![extra.to_string_lossy().into_owned()],
                delete: vec![],
                rename: vec![],
                mkdir: vec![],
                excludes: vec!["node_modules".into(), "*.tmp".into()],
                content_policy: squallz_core::CreateContentPolicy::CrossPlatformClean,
                password: None,
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let names: Vec<String> = state
            .engine
            .list(&archive, &OpenOptions::default())
            .unwrap()
            .into_iter()
            .map(|e| e.path.display)
            .collect();
        assert!(names.iter().any(|name| name == "extra/keep.txt"));
        assert!(names.iter().any(|name| name == "extra/.env"));
        assert!(!names.iter().any(|name| name.contains("node_modules")));
        assert!(!names.iter().any(|name| name.contains("__MACOSX")));
        assert!(!names.iter().any(|name| name.ends_with(".DS_Store")));
        assert!(!names.iter().any(|name| name.ends_with("._keep.txt")));
        assert!(!names.iter().any(|name| name.ends_with(".tmp")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn duplicate_scan_job_reports_groups_without_modifying_files() {
        let dir = temp_dir("duplicate-scan-job");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("a.bin"), b"same bytes").unwrap();
        std::fs::write(root.join("b.bin"), b"same bytes").unwrap();
        std::fs::write(root.join("unique.bin"), b"unique bytes").unwrap();
        std::fs::write(root.join("cache").join("ignored.bin"), b"same bytes").unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::DuplicateScan {
                inputs: vec![root.to_string_lossy().into_owned()],
                excludes: vec!["cache".into()],
                min_size: 1,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(std::fs::read(root.join("a.bin")).unwrap(), b"same bytes");
        assert_eq!(std::fs::read(root.join("b.bin")).unwrap(), b"same bytes");
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).expect("duplicate result");
        assert_eq!(result["operation"], "duplicates");
        assert_eq!(result["hash_algorithm"], "blake3");
        assert_eq!(result["duplicate_groups"].as_u64(), Some(1));
        assert_eq!(result["duplicate_files"].as_u64(), Some(2));
        assert_eq!(result["groups"][0]["count"].as_u64(), Some(2));
        assert_eq!(result["groups"][0]["paths"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_job_hashes_files_with_shared_excludes() {
        let dir = temp_dir("checksum-job");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("keep.txt"), b"abc").unwrap();
        std::fs::write(root.join("target").join("ignored.txt"), b"ignore").unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Checksum {
                inputs: vec![root.to_string_lossy().into_owned()],
                excludes: vec!["target".into()],
                algorithm: ChecksumAlgorithm::Sha256,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"abc");
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
            && payload["id"] == id
            && payload["done"] == 3
            && payload["total"] == 3
            && payload["current"]
                .as_str()
                .is_some_and(|current| current.ends_with("keep.txt"))));
        let result = done_result(&events, id).expect("checksum result");
        assert_eq!(result["operation"], "checksum");
        assert_eq!(result["algorithm"], "sha256");
        assert_eq!(result["files_hashed"].as_u64(), Some(1));
        assert_eq!(result["bytes_hashed"].as_u64(), Some(3));
        assert_eq!(
            result["items"][0]["digest"],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(!result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"]
                .as_str()
                .unwrap_or_default()
                .contains("ignored")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_check_job_reports_manifest_mismatch() {
        let dir = temp_dir("checksum-check-job");
        std::fs::write(dir.join("good.txt"), b"abc").unwrap();
        std::fs::write(dir.join("bad.txt"), b"changed").unwrap();
        std::fs::write(
            dir.join("SHA256SUMS"),
            concat!(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  good.txt\n",
                "0000000000000000000000000000000000000000000000000000000000000000  bad.txt\n",
            ),
        )
        .unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::ChecksumCheck {
                manifest: dir.join("SHA256SUMS").to_string_lossy().into_owned(),
                algorithm: ChecksumAlgorithm::Sha256,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).expect("checksum check result");
        assert_eq!(result["operation"], "checksum_check");
        assert_eq!(result["ok"].as_bool(), Some(false));
        assert_eq!(result["checked"].as_u64(), Some(2));
        assert_eq!(result["passed"].as_u64(), Some(1));
        assert_eq!(result["failed"].as_u64(), Some(1));
        assert_eq!(std::fs::read(dir.join("bad.txt")).unwrap(), b"changed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_job_creates_empty_directory_entry() {
        let dir = temp_dir("update-mkdir");
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("base.txt"), b"base").unwrap();
        let archive = dir.join("out.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&seed),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Update {
                path: archive.to_string_lossy().into_owned(),
                add: vec![],
                delete: vec![],
                rename: vec![],
                mkdir: vec!["new-folder".into()],
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                password: None,
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let names: Vec<String> = state
            .engine
            .list(&archive, &OpenOptions::default())
            .unwrap()
            .into_iter()
            .map(|e| e.path.display)
            .collect();
        assert!(names.iter().any(|name| name == "new-folder/"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_job_reports_target_conflict_as_failed() {
        let dir = temp_dir("update-conflict");
        let seed = dir.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("a.txt"), b"alpha").unwrap();
        std::fs::write(seed.join("b.txt"), b"bravo").unwrap();
        let archive = dir.join("out.zip");
        let state = Arc::new(AppState::new());
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&seed),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let before = std::fs::read(&archive).unwrap();

        let manager = JobManager::new();
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Update {
                path: archive.to_string_lossy().into_owned(),
                add: vec![],
                delete: vec![],
                rename: vec![crate::dto::RenameSpec {
                    from: "seed/a.txt".into(),
                    to: "seed/b.txt".into(),
                }],
                mkdir: vec![],
                excludes: vec![],
                content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
                password: None,
                level: 5,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "failed"]);
        let failed = events
            .iter()
            .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "failed")
            .expect("failed state");
        assert_eq!(failed.1["error"]["key"].as_str(), Some("error.other"));
        assert!(
            failed.1["error"]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("already exists")),
            "{:?}",
            failed.1
        );
        assert_eq!(std::fs::read(&archive).unwrap(), before);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_job_uses_submitted_safety_limits() {
        let dir = temp_dir("limits");
        let src = dir.join("data");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("payload.txt"), b"payload over one byte").unwrap();
        let zip = dir.join("limited.zip");

        AppState::new()
            .engine
            .create(
                &zip,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");

        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Extract {
                path: zip.to_string_lossy().into_owned(),
                dest: out.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::Skip,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: false,
            },
            SettingsDto {
                safety_max_output_bytes: Some(1),
                ..SettingsDto::default()
            },
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap();
        let failed = events
            .iter()
            .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "failed");
        assert_eq!(
            failed.and_then(|(_, p)| p["error"]["key"].as_str()),
            Some("error.resource_limit")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_job_best_effort_reports_skipped_entries() {
        let dir = temp_dir("best-effort");
        let archive = dir.join("damaged.zip");
        let good_name = b"good.txt";
        let good_data = b"safe bytes";
        let bad_name = b"bad.txt";
        let bad_data = b"broken bytes";
        let mut bytes = build_stored_zip(&[(good_name, good_data), (bad_name, bad_data)]);
        let bad_data_offset = 30 + good_name.len() + good_data.len() + 30 + bad_name.len();
        bytes[bad_data_offset] ^= 0xFF;
        std::fs::write(&archive, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let out = dir.join("out");
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Extract {
                path: archive.to_string_lossy().into_owned(),
                dest: out.to_string_lossy().into_owned(),
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: squallz_core::api::OverwritePolicy::RenameBoth,
                symlinks: squallz_core::api::SymlinkPolicy::Preserve,
                smart: false,
                encoding: None,
                password: None,
                verify_sfx: false,
                best_effort: true,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(std::fs::read(out.join("good.txt")).unwrap(), good_data);
        assert!(!out.join("bad.txt").exists());
        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["best_effort"], true);
        assert_eq!(result["problems_total"], 1);
        assert_eq!(result["counts"]["selected_entries"], 2);
        assert_eq!(result["counts"]["created"], 1);
        assert_eq!(result["counts"]["failed"], 1);
        assert!(result["problems"][0].as_str().unwrap().contains("bad.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_job_keeps_exact_problem_total_with_bounded_messages() {
        let dir = temp_dir("test-bounded-problems");
        let archive = dir.join("damaged.zip");
        let names = (0..25)
            .map(|index| format!("damaged-{index:02}.txt").into_bytes())
            .collect::<Vec<_>>();
        let payloads = (0..25)
            .map(|index| format!("squallz-damaged-payload-{index:02}").into_bytes())
            .collect::<Vec<_>>();
        let entries = names
            .iter()
            .zip(&payloads)
            .map(|(name, payload)| (name.as_slice(), payload.as_slice()))
            .collect::<Vec<_>>();
        let mut bytes = build_stored_zip(&entries);
        for payload in &payloads {
            let offset = bytes
                .windows(payload.len())
                .position(|window| window == payload)
                .unwrap();
            bytes[offset] ^= 0xFF;
        }
        std::fs::write(&archive, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Test {
                path: archive.to_string_lossy().into_owned(),
                encoding: None,
                password: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["entries"], 25);
        assert_eq!(result["entries_tested"], 25);
        assert_eq!(result["problems_total"], 25);
        assert_eq!(result["problems_truncated"], true);
        assert_eq!(result["problems"].as_array().map(Vec::len), Some(20));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recovered_zip_test_job_reports_typed_structure_status() {
        let dir = temp_dir("test-recovered-zip-structure");
        let archive = dir.join("missing-central-directory.zip");
        let mut bytes = build_stored_zip(&[(b"recoverable.txt", b"recoverable payload")]);
        let central_start = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists in sample");
        bytes.truncate(central_start);
        std::fs::write(&archive, bytes).unwrap();

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Test {
                path: archive.to_string_lossy().into_owned(),
                encoding: None,
                password: None,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let result = done_result(&events, id).unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["entries_tested"], 1);
        assert_eq!(result["problems_total"], 1);
        assert_eq!(result["structure"], "zip_local_headers_recovered");
        assert_eq!(result["problems"].as_array().map(Vec::len), Some(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_recovery_report_keeps_metrics_for_the_result_ui() {
        let report = squallz_recovery::RecoveryReport {
            ok: false,
            operation: "verify",
            archive: PathBuf::from("damaged.zip"),
            recovery: PathBuf::from("damaged.zip.par2"),
            outputs: Vec::new(),
            output: None,
            tool: PathBuf::from("rust-par2"),
            redundancy_percent: None,
            source_file_count: 1,
            status_code: None,
            metrics: Some(squallz_recovery::RecoveryMetrics {
                all_correct: false,
                repair_possible: true,
                blocks_needed: 3,
                recovery_blocks_available: 4,
                blocks_repaired: None,
                files_repaired: None,
                no_damage: false,
            }),
            stdout: "repair_possible=true".to_owned(),
            stderr: "damage found".to_owned(),
        };

        let result = recovery_report_json(report).unwrap().unwrap();
        assert_eq!(result["ok"].as_bool(), Some(false));
        assert_eq!(result["metrics"]["repair_possible"].as_bool(), Some(true));
        assert_eq!(result["metrics"]["blocks_needed"].as_u64(), Some(3));
        assert_eq!(
            result["metrics"]["recovery_blocks_available"].as_u64(),
            Some(4)
        );
    }

    #[cfg(unix)]
    #[test]
    fn recovery_job_bridges_to_external_par2_tool() {
        use base64::Engine as _;
        use std::os::unix::fs::PermissionsExt;

        let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();

        let dir = temp_dir("recovery");
        let archive = dir.join("protected.zip");
        let recovery = dir.join("protected.zip.par2");
        let recovery_fixture = dir.join("protected.zip.fixture.par2");
        let recovery_volume_fixture = dir.join("protected.zip.fixture.vol0+1.par2");
        let split_dir = dir.join("split");
        std::fs::create_dir(&split_dir).unwrap();
        let split_first = split_dir.join("set.zip.001");
        let split_second = split_dir.join("set.zip.002");
        let split_recovery = split_dir.join("set.zip.par2");
        let multi_first = dir.join("set.zip.001");
        let multi_second = dir.join("set.zip.002");
        let multi_recovery = dir.join("set.zip.par2");
        let multi_recovery_volume = dir.join("set.zip.vol0+4.par2");
        let multi_output = dir.join("repaired-set");
        let tool = dir.join("fake-par2");
        let log = dir.join("fake-par2.log");
        std::fs::write(&archive, b"archive bytes").unwrap();
        std::fs::write(&split_first, b"first-volume-original\n").unwrap();
        std::fs::write(&split_second, b"second-volume-original\n").unwrap();
        std::fs::write(&multi_first, b"damaged").unwrap();
        let fixture = base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/protected.zip.par2.b64").trim(),
            )
            .unwrap();
        std::fs::write(&recovery_fixture, fixture).unwrap();
        std::fs::write(
            &recovery_volume_fixture,
            base64::engine::general_purpose::STANDARD
                .decode(
                    include_str!(
                        "../../squallz-recovery/tests/fixtures/protected.zip.vol0+1.par2.b64"
                    )
                    .trim(),
                )
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            &multi_recovery,
            base64::engine::general_purpose::STANDARD
                .decode(
                    include_str!("../../squallz-recovery/tests/fixtures/multi-set.zip.par2.b64")
                        .trim(),
                )
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            &multi_recovery_volume,
            base64::engine::general_purpose::STANDARD
                .decode(
                    include_str!(
                        "../../squallz-recovery/tests/fixtures/multi-set.zip.vol0+4.par2.b64"
                    )
                    .trim(),
                )
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            &tool,
            r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
    printf 'Constructing: 25.0%%\rProcessing: 75.0%%\rWriting recovery packets\nDone\n'
    if [ "$(basename "$5")" = "set.zip.001" ]; then
      cp "$SQUALLZ_FAKE_PAR2_MULTI_FIXTURE" "$4"
      cp "$SQUALLZ_FAKE_PAR2_MULTI_VOLUME_FIXTURE" "${4%.par2}.vol0+4.par2"
    else
      cp "$SQUALLZ_FAKE_PAR2_FIXTURE" "$4"
      cp "$SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE" "${4%.par2}.vol0+1.par2"
    fi
    ;;
  verify|repair)
    printf 'Loading: 20.0%%\r'
    recovery="$2"
    base=""
    case "$2" in
      -B*)
        base="${2#-B}"
        recovery="$3"
        ;;
    esac
    test -f "$recovery" || exit 2
    if [ "$1" = verify ]; then
      printf 'Verifying source files:\nScanning: 80.0%%\rDone\n'
    fi
    if [ "$1" = repair ]; then
      printf 'Processing: 60.0%%\r'
      if [ -n "$base" ] && [ "$(basename "${recovery%.par2}")" = "set.zip" ]; then
        printf 'first-volume-original\n' > "$base/set.zip.001"
        printf 'second-volume-original\n' > "$base/set.zip.002"
      elif [ -n "$base" ]; then
        target="$base/$(basename "${recovery%.par2}")"
        printf 'archive bytes' > "$target"
      else
        target="${recovery%.par2}"
        printf 'archive bytes' > "$target"
      fi
      printf 'Writing recovered data\nRepair complete.\n'
    fi
    ;;
  *)
    exit 64
    ;;
esac
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();
        let _tool_env = EnvRestore::set("SQUALLZ_PAR2", &tool);
        let _log_env = EnvRestore::set("SQUALLZ_FAKE_PAR2_LOG", &log);
        let _fixture_env = EnvRestore::set("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture);
        let _volume_fixture_env =
            EnvRestore::set("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture);
        let _multi_fixture_env =
            EnvRestore::set("SQUALLZ_FAKE_PAR2_MULTI_FIXTURE", &multi_recovery);
        let _multi_volume_fixture_env = EnvRestore::set(
            "SQUALLZ_FAKE_PAR2_MULTI_VOLUME_FIXTURE",
            &multi_recovery_volume,
        );

        let manager = JobManager::new();
        let state = Arc::new(AppState::new());
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();

        let protect_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Protect {
                path: archive.to_string_lossy().into_owned(),
                redundancy: 12,
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let split_protect_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::Protect {
                path: split_second.to_string_lossy().into_owned(),
                redundancy: 18,
                recovery: Some(split_recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let verify_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::VerifyRecovery {
                path: archive.to_string_lossy().into_owned(),
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let repair_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairRecovery {
                path: archive.to_string_lossy().into_owned(),
                output: None,
                output_directory: false,
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let copy_output = dir.join("restored.zip");
        let repair_copy_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairRecovery {
                path: archive.to_string_lossy().into_owned(),
                output: Some(copy_output.to_string_lossy().into_owned()),
                output_directory: false,
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let repair_set_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairRecovery {
                path: multi_first.to_string_lossy().into_owned(),
                output: Some(multi_output.to_string_lossy().into_owned()),
                output_directory: true,
                recovery: Some(multi_recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert_eq!(std::fs::read(&copy_output).unwrap(), b"archive bytes");
        assert_eq!(
            std::fs::read(multi_output.join("set.zip.001")).unwrap(),
            b"first-volume-original\n"
        );
        assert_eq!(
            std::fs::read(multi_output.join("set.zip.002")).unwrap(),
            b"second-volume-original\n"
        );
        assert_eq!(std::fs::read(&multi_first).unwrap(), b"damaged");
        assert!(!multi_second.exists());
        assert!(std::fs::read_dir(&multi_output).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".par2")
        }));
        std::fs::write(&copy_output, b"existing repaired output\n").unwrap();
        let source_before_conflict = std::fs::read(&archive).unwrap();
        let conflict_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairRecovery {
                path: archive.to_string_lossy().into_owned(),
                output: Some(copy_output.to_string_lossy().into_owned()),
                output_directory: false,
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        let events = sink.events.lock().unwrap();
        assert_eq!(
            states_of(&events, protect_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&events, split_protect_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&events, verify_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&events, repair_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&events, repair_copy_id),
            vec!["queued", "running", "done"]
        );
        assert_eq!(
            states_of(&events, repair_set_id),
            vec!["queued", "running", "done"]
        );
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == protect_id
                && payload["phase"] == "recovery_process"
                && payload["interruptible"] == true
        }));
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == protect_id
                && payload["phase"] == "recovery_finalize"
                && payload["interruptible"] == false
        }));
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == verify_id
                && payload["phase"] == "recovery_verify"
                && payload["interruptible"] == true
        }));
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == repair_id
                && payload["phase"] == "recovery_process"
                && payload["interruptible"] == false
        }));
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == repair_copy_id
                && payload["phase"] == "recovery_prepare"
                && payload["interruptible"] == true
        }));
        assert!(events.iter().any(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == repair_copy_id
                && payload["phase"] == "recovery_verify"
                && payload["interruptible"] == false
        }));
        assert_eq!(
            states_of(&events, conflict_id),
            vec!["queued", "running", "failed"]
        );
        assert!(recovery.is_file());
        assert!(split_recovery.is_file());
        assert_eq!(
            done_result(&events, protect_id)
                .and_then(|v| v["operation"].as_str().map(str::to_owned)),
            Some("protect".to_owned())
        );
        assert_eq!(
            done_result(&events, protect_id)
                .and_then(|value| value["outputs"].as_array().map(Vec::len)),
            Some(2)
        );
        let copy_result = done_result(&events, repair_copy_id).unwrap();
        assert_eq!(copy_result["operation"].as_str(), Some("repair"));
        assert_eq!(
            copy_result["output"].as_str(),
            Some(copy_output.to_string_lossy().as_ref())
        );
        let set_result = done_result(&events, repair_set_id).unwrap();
        assert_eq!(set_result["operation"].as_str(), Some("repair"));
        assert_eq!(set_result["source_file_count"].as_u64(), Some(2));
        assert_eq!(
            set_result["output"].as_str(),
            Some(multi_output.to_string_lossy().as_ref())
        );
        assert_eq!(std::fs::read(&archive).unwrap(), b"archive bytes");
        assert_eq!(std::fs::read(&archive).unwrap(), source_before_conflict);
        assert_eq!(
            std::fs::read(&copy_output).unwrap(),
            b"existing repaired output\n"
        );
        let conflict = events
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == conflict_id && payload["state"] == "failed"
            })
            .expect("PAR2 output conflict must emit a failed state");
        assert_eq!(
            conflict.1["error"]["key"].as_str(),
            Some("error.output_exists")
        );
        assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".sqz-par2-repair-")
        }));
        let log = std::fs::read_to_string(&log).unwrap();
        assert!(log.contains("create -r12"), "log: {log}");
        assert!(log.contains("create -r18"), "log: {log}");
        assert!(
            log.contains(split_first.to_string_lossy().as_ref()),
            "log: {log}"
        );
        assert!(
            log.contains(split_second.to_string_lossy().as_ref()),
            "log: {log}"
        );
        assert!(log.contains("verify"), "log: {log}");
        assert!(log.contains("repair"), "log: {log}");
        assert!(log.contains("repair -B"), "log: {log}");
        assert!(log.contains("set.zip.par2"), "log: {log}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Selection expansion: directories by prefix, files exactly.
    #[test]
    fn selection_expansion() {
        let metas: Vec<EntryMeta> = ["a/x.txt", "a/y.txt", "b/z.txt", "top.txt"]
            .iter()
            .map(|n| EntryMeta {
                path: EntryPath::from_utf8(*n),
                entry_type: squallz_core::api::EntryType::File,
                size: 1,
                compressed_size: None,
                modified: None,
                unix_mode: None,
                crc32: None,
                encrypted: false,
            })
            .collect();
        let sel = expand_selection(&metas, &["a/".to_owned(), "top.txt".to_owned()]);
        let names: Vec<&str> = sel.iter().map(|p| p.display.as_str()).collect();
        assert_eq!(names, vec!["a/x.txt", "a/y.txt", "top.txt"]);
    }
}
