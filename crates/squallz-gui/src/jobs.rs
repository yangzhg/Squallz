//! Job execution: every compress/extract/test/convert runs through the core
//! [`JobQueue`]. Progress is forwarded as throttled `job://progress` events,
//! state changes as `job://state`; mid-job questions (conflicts, passwords)
//! park the worker on the [`AskBridge`] until the frontend answers.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use squallz_core::api::{ControlToken, Detected, FormatError};
use squallz_core::{
    lock_unpoisoned, validate_sfx_template, Engine, JobId, JobQueue, JobResources, JobState,
    SfxBuildOptions, SfxTarget,
};

use crate::audit::{self, OperationAudit, OperationAuditRecord};
use crate::bridge::{AskAnswer, AskBridge};
use crate::dto::{ErrorDto, JobSpec, SettingsDto, SfxCreateCapabilityDto, StateEvent};
use crate::events::{emit, EventSink, EV_STATE};
use crate::source_cleanup_journal::SourceCleanupJournal;
use crate::state::{AppState, ResolvedArchiveSource};

mod execution;
mod snapshots;
mod source_cleanup;

pub use source_cleanup::SourceCleanupRecoveryNotice;
use source_cleanup::{SourceCleanup, SystemTrashAdapter, TrashAdapter};
mod progress;

use execution::JobContext;
pub(crate) use execution::{
    convert_create_options, create_job_request, expand_selection_with_control,
};
use progress::EmitProgress;

#[cfg(test)]
mod test_support;

use snapshots::{job_unavailable_error, JobSnapshotStore};
pub use snapshots::{JobInteraction, JobSnapshotDelta, JobStateSnapshot};

pub(crate) const MAX_PARALLEL_JOBS: usize = 8;
const MAX_AUTOMATIC_PARALLEL_JOBS: usize = 4;

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
    source_cleanup: Arc<SourceCleanup>,
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
        let source_cleanup = Arc::new(SourceCleanup::new(trash_adapter, source_cleanup_journal));
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
            source_cleanup,
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
        let source_cleanup = Arc::clone(&self.source_cleanup);
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
                let outcome = JobContext {
                    gui_id,
                    state: &state,
                    settings: &settings,
                    bridge: &bridge,
                    events: &events,
                    ctl,
                    cancel_flag: &flag,
                    sink: &sink,
                    snapshots: &snapshots,
                    sfx_template: sfx_template.as_deref(),
                    source_cleanup: &source_cleanup,
                }
                .run(&execution_spec, &spec);
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
        self.cancel_managed_jobs(&entries)
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
        self.cancel_managed_jobs(&entries)
    }

    fn cancel_managed_job(&self, gui_id: u64, entry: &ManagedJob) -> bool {
        self.cancel_managed_jobs(&[(gui_id, entry.clone())]) == 1
    }

    fn cancel_managed_jobs(&self, entries: &[(u64, ManagedJob)]) -> usize {
        let queue_ids = entries
            .iter()
            .map(|(_, entry)| entry.queue_id)
            .collect::<Vec<_>>();
        let cancelled = self
            .queue
            .try_cancel_many(&queue_ids)
            .into_iter()
            .collect::<HashSet<_>>();

        for (gui_id, entry) in entries {
            if !cancelled.contains(&entry.queue_id) {
                continue;
            }
            entry.cancel_flag.store(true, Ordering::Relaxed);
            self.bridge.wake_cancelled(*gui_id);
            if self.queue.state(entry.queue_id) == Some(JobState::Cancelled) {
                if let Some(version) =
                    lock_unpoisoned(&self.snapshots).set_state(*gui_id, "cancelled", None, None)
                {
                    emit_state(&*entry.events, *gui_id, version, "cancelled", None);
                }
            }
        }
        cancelled.len()
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
        self.source_cleanup.notice()
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

#[derive(Clone, Copy)]
enum QueueMove {
    Earlier,
    Later,
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

#[cfg(test)]
mod tests;
