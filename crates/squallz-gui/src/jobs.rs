//! Job execution: every compress/extract/test/convert runs through the core
//! [`JobQueue`]. Progress is forwarded as throttled `job://progress` events,
//! state changes as `job://state`; mid-job questions (conflicts, passwords)
//! park the worker on the [`AskBridge`] until the frontend answers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use squallz_core::api::{
    split_volume_name, CompressionLevel, ConflictDecision, ConflictResolver, ControlToken,
    CreateOptions, EntryMeta, EntryPath, ExtractProblemReporter, FormatError, OpenOptions,
    OverwritePolicy, Password, ProgressSink, RecoverySummary, SymlinkPolicy, UpdateOp,
};
use squallz_core::{
    analyze_extract_layout, ChecksumAlgorithm, JobId, JobQueue, JobState, SmartLayout,
};

use crate::audit::{self, OperationAudit, OperationAuditRecord};
use crate::bridge::{AskAnswer, AskBridge};
use crate::dto::{
    AskConflictEvent, AskPasswordEvent, BatchExtractItem, ErrorDto, JobSpec, ProgressEvent,
    SettingsDto, StateEvent,
};
use crate::events::{emit, EventSink, EV_ASK_CONFLICT, EV_ASK_PASSWORD, EV_PROGRESS, EV_STATE};
use crate::nested::extract_nested_archive_to_temp;
use crate::state::AppState;

/// Minimum interval between two progress events.
const PROGRESS_THROTTLE_MS: u128 = 60;

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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

fn entries_slice_or_empty(entries: Option<&[EntryMeta]>) -> &[EntryMeta] {
    match entries {
        Some(entries) => entries,
        None => &[],
    }
}

fn status_code_label(status_code: Option<i32>) -> String {
    match status_code {
        Some(code) => code.to_string(),
        None => "unknown".into(),
    }
}

/// GUI job manager: owns the queue and the gui-id ↔ queue-id mapping.
pub struct JobManager {
    queue: JobQueue,
    next_id: AtomicU64,
    audit: Arc<OperationAudit>,
    /// gui id → (queue id, cancel flag). The extra flag lets question
    /// waits (conflict/password dialogs) observe cancellation even though
    /// the queue's own token is not shareable across the trait boundary.
    map: Mutex<HashMap<u64, (JobId, Arc<AtomicBool>)>>,
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
        Self {
            queue: JobQueue::new(1),
            next_id: AtomicU64::new(1),
            audit,
            map: Mutex::new(HashMap::new()),
            bridge: Arc::new(AskBridge::default()),
        }
    }

    /// Submits a job; events for its whole life cycle carry the returned id.
    pub fn submit(
        &self,
        state: Arc<AppState>,
        events: Arc<dyn EventSink>,
        spec: JobSpec,
        settings: SettingsDto,
    ) -> u64 {
        let gui_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        emit_state(&*events, gui_id, "queued", None);
        let bridge = Arc::clone(&self.bridge);
        let audit = Arc::clone(&self.audit);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel_flag);
        let queue_id = self.queue.submit(Box::new(move |ctl, queue_sink| {
            emit_state(&*events, gui_id, "running", None);
            let sink = EmitProgress::new(gui_id, Arc::clone(&events), queue_sink);
            let outcome = run_job(
                &spec, gui_id, &state, &settings, &bridge, &events, ctl, &flag, &sink,
            );
            sink.flush();
            match outcome {
                Ok(result) => {
                    record_job_audit(&audit, gui_id, &spec, "done", result.as_ref(), None);
                    emit(
                        &*events,
                        EV_STATE,
                        &StateEventWithResult {
                            id: gui_id,
                            state: "done",
                            error: None,
                            result,
                        },
                    );
                    Ok(())
                }
                Err(FormatError::Cancelled) => {
                    record_job_audit(&audit, gui_id, &spec, "cancelled", None, None);
                    emit_state(&*events, gui_id, "cancelled", None);
                    Err(FormatError::Cancelled)
                }
                Err(e) => {
                    let error = ErrorDto::from(&e);
                    record_job_audit(
                        &audit,
                        gui_id,
                        &spec,
                        "failed",
                        None,
                        Some(error.key.clone()),
                    );
                    emit_state(&*events, gui_id, "failed", Some(error));
                    Err(e)
                }
            }
        }));
        lock_unpoisoned(&self.map).insert(gui_id, (queue_id, cancel_flag));
        gui_id
    }

    /// Pauses a job (takes effect at the next chunk boundary).
    pub fn pause(&self, events: &dyn EventSink, gui_id: u64) {
        if let Some(qid) = self.queue_id(gui_id) {
            self.queue.pause(qid);
            if self.queue.state(qid) == Some(JobState::Paused) {
                emit_state(events, gui_id, "paused", None);
            }
        }
    }

    /// Resumes a paused job.
    pub fn resume(&self, events: &dyn EventSink, gui_id: u64) {
        if let Some(qid) = self.queue_id(gui_id) {
            self.queue.resume(qid);
            if self.queue.state(qid) == Some(JobState::Running) {
                emit_state(events, gui_id, "running", None);
            }
        }
    }

    /// Cancels a job. A queued job is dropped immediately; a running one
    /// unwinds at its next checkpoint (open question dialogs are released
    /// through the per-job cancel flag) and reports `cancelled` itself.
    pub fn cancel(&self, events: &dyn EventSink, gui_id: u64) {
        let entry = lock_unpoisoned(&self.map).get(&gui_id).cloned();
        if let Some((qid, flag)) = entry {
            flag.store(true, Ordering::Relaxed);
            self.bridge.wake_cancelled(gui_id);
            self.queue.cancel(qid);
            if self.queue.state(qid) == Some(JobState::Cancelled) {
                emit_state(events, gui_id, "cancelled", None);
            }
        }
    }

    /// Test/shutdown helper: blocks until the queue drains.
    #[cfg(test)]
    pub fn wait_idle(&self) {
        self.queue.wait_idle();
    }

    fn queue_id(&self, gui_id: u64) -> Option<JobId> {
        lock_unpoisoned(&self.map).get(&gui_id).map(|(qid, _)| *qid)
    }
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

fn emit_state(events: &dyn EventSink, id: u64, state: &str, error: Option<ErrorDto>) {
    emit(
        events,
        EV_STATE,
        &StateEvent {
            id,
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
    queue_sink: &'a dyn ProgressSink,
    inner: Mutex<ProgressWindow>,
}

struct ProgressWindow {
    last_emit: Instant,
    last_done: u64,
    speed: u64,
    latest: Option<ProgressSnapshot>,
    latest_current_file: Option<ProgressSnapshot>,
}

#[derive(Clone)]
struct ProgressSnapshot {
    done: u64,
    total: u64,
    current: String,
    current_done: u64,
    current_total: u64,
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
}

impl<'a> EmitProgress<'a> {
    fn new(id: u64, events: Arc<dyn EventSink>, queue_sink: &'a dyn ProgressSink) -> Self {
        Self {
            id,
            events,
            queue_sink,
            inner: Mutex::new(ProgressWindow {
                last_emit: Instant::now(),
                last_done: 0,
                speed: 0,
                latest: None,
                latest_current_file: None,
            }),
        }
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
        emit(
            &*self.events,
            EV_PROGRESS,
            &ProgressEvent {
                id: self.id,
                done: snapshot.done,
                total: snapshot.total,
                current: snapshot.current,
                current_done: snapshot.current_done,
                current_total: snapshot.current_total,
                speed,
            },
        );
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
        self.queue_sink
            .on_entry_progress(done, total, current, current_done, current_total);
        let mut w = lock_unpoisoned(&self.inner);
        let elapsed = w.last_emit.elapsed().as_millis();
        let snapshot = ProgressSnapshot {
            done,
            total,
            current: current.display.clone(),
            current_done,
            current_total,
        };
        if current_total > 0 {
            w.latest_current_file = Some(snapshot.clone());
        }
        if elapsed < PROGRESS_THROTTLE_MS {
            w.latest = Some(snapshot);
            return;
        }
        // Instantaneous speed over the emit window, lightly smoothed.
        let delta = done.saturating_sub(w.last_done);
        let instant = (delta as u128 * 1000 / elapsed.max(1)) as u64;
        w.speed = if w.speed == 0 {
            instant
        } else {
            (w.speed * 3 + instant) / 4
        };
        w.last_emit = Instant::now();
        w.last_done = done;
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
}

/// Conflict resolver backed by the frontend dialog.
struct GuiConflictResolver {
    gui_id: u64,
    events: Arc<dyn EventSink>,
    bridge: Arc<AskBridge>,
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
        match self.bridge.wait(self.gui_id, &cancelled) {
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
    problems: Mutex<Vec<String>>,
}

impl ExtractProblemCollector {
    fn count(&self) -> usize {
        lock_unpoisoned(&self.problems).len()
    }

    fn preview(&self) -> Vec<String> {
        lock_unpoisoned(&self.problems)
            .iter()
            .take(20)
            .cloned()
            .collect()
    }
}

impl ExtractProblemReporter for ExtractProblemCollector {
    fn skipped_entry(&self, path: &EntryPath, error: &FormatError) {
        lock_unpoisoned(&self.problems).push(format!("{}: {error}", path.display));
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
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    gui_id: u64,
    archive: &Path,
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
                let name = path_file_name_or_empty(archive);
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
                match bridge.wait(gui_id, &cancelled) {
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

fn overwrite_policy(s: &str) -> OverwritePolicy {
    match s {
        "overwrite" => OverwritePolicy::Overwrite,
        "rename" => OverwritePolicy::RenameBoth,
        "ask" => OverwritePolicy::Ask,
        _ => OverwritePolicy::Skip,
    }
}

fn symlink_policy(s: &str) -> SymlinkPolicy {
    match s {
        "follow" => SymlinkPolicy::Follow,
        "skip" => SymlinkPolicy::Skip,
        _ => SymlinkPolicy::Preserve,
    }
}

fn is_sqz_source_path(path: &Path) -> bool {
    is_plain_sqz_path(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                split_volume_name(name).is_some_and(|(base, _)| is_plain_sqz_path(Path::new(base)))
            })
}

fn is_plain_sqz_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
}

fn is_plain_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "zip" | "jar" | "apk" | "cbz" | "ipa"
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn run_extract_archive_job(
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
    gui_id: u64,
    archive: &Path,
    dest: &Path,
    selection: Option<&[String]>,
    overwrite: &str,
    symlinks: &str,
    smart: bool,
    encoding: Option<String>,
    password: Option<&str>,
    best_effort: bool,
) -> Result<serde_json::Value, FormatError> {
    let policy = overwrite_policy(overwrite);
    let resolver: Option<Arc<dyn ConflictResolver>> = if policy == OverwritePolicy::Ask {
        Some(Arc::new(GuiConflictResolver {
            gui_id,
            events: Arc::clone(events),
            bridge: Arc::clone(bridge),
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
        symlinks: symlink_policy(symlinks),
        limits: settings.safety_limits(),
        resources: settings.resource_options(),
        best_effort,
        problem_reporter,
        ..Default::default()
    };
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    let final_dest = with_gui_password(
        state,
        bridge,
        &**events,
        ctl,
        cancel_flag,
        gui_id,
        &archive,
        password,
        |pw| {
            let open = OpenOptions {
                password: pw.cloned(),
                encoding_override: encoding.clone(),
            };
            // Smart layout and display-path selections both need the entry list
            // up front.
            let entries = if smart || selection.is_some() {
                Some(state.engine.list(&archive, &open)?)
            } else {
                None
            };
            let selected: Option<Vec<EntryPath>> = match (selection, entries.as_ref()) {
                (Some(sel), Some(entries)) => Some(expand_selection(entries, sel)),
                _ => None,
            };
            let mut final_dest = dest.clone();
            if smart {
                if let SmartLayout::WrapInFolder =
                    analyze_extract_layout(entries_slice_or_empty(entries.as_deref()))
                {
                    final_dest = dest.join(state.engine.archive_stem(&archive));
                }
            }
            state.engine.extract(
                &archive,
                &final_dest,
                selected.as_deref(),
                &open,
                &x_opts,
                sink,
                ctl,
            )?;
            Ok(final_dest)
        },
    )?;
    Ok(serde_json::json!({
        "dest": final_dest.to_string_lossy(),
        "best_effort": best_effort,
        "skipped": problem_collector.count(),
        "problems": problem_collector.preview(),
    }))
}

#[allow(clippy::too_many_arguments)] // batch job shares the same GUI job context as extract
fn run_batch_extract_job(
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
    gui_id: u64,
    items: &[BatchExtractItem],
    overwrite: &str,
    symlinks: &str,
    smart: bool,
) -> Result<serde_json::Value, FormatError> {
    if items.is_empty() {
        return Err(FormatError::Unsupported(
            "batch extract requires at least one archive".into(),
        ));
    }

    let batch_sink = BatchProgressSink::new(sink, items.len());
    let mut outputs = Vec::new();
    let mut failures = Vec::new();
    let mut skipped_total = 0usize;

    for (index, item) in items.iter().enumerate() {
        ctl.checkpoint()?;
        let archive = PathBuf::from(&item.path);
        let dest = PathBuf::from(&item.dest);
        let label = batch_archive_label(&archive);
        batch_sink.start_archive(index, label.clone());
        match run_extract_archive_job(
            state,
            settings,
            bridge,
            events,
            ctl,
            cancel_flag,
            &batch_sink,
            gui_id,
            &archive,
            &dest,
            None,
            overwrite,
            symlinks,
            smart,
            item.encoding.clone(),
            item.password.as_deref(),
            item.best_effort,
        ) {
            Ok(result) => {
                let skipped = match result.get("skipped").and_then(|value| value.as_u64()) {
                    Some(value) => value as usize,
                    None => 0,
                };
                let dest = match result.get("dest").and_then(|value| value.as_str()) {
                    Some(value) => value.to_owned(),
                    None => String::new(),
                };
                let best_effort = result
                    .get("best_effort")
                    .and_then(|value| value.as_bool())
                    .is_some_and(|value| value);
                skipped_total = skipped_total.saturating_add(skipped);
                outputs.push(serde_json::json!({
                    "archive": archive.to_string_lossy(),
                    "dest": dest,
                    "skipped": skipped,
                    "best_effort": best_effort,
                }));
            }
            Err(FormatError::Cancelled) => return Err(FormatError::Cancelled),
            Err(error) => {
                let dto = ErrorDto::from(&error);
                failures.push(serde_json::json!({
                    "archive": archive.to_string_lossy(),
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

    Ok(serde_json::json!({
        "operation": "batch_extract",
        "archives": items.len(),
        "extracted": outputs.len(),
        "failed": failures.len(),
        "skipped": skipped_total,
        "outputs": outputs,
        "failures": failures,
    }))
}

/// Executes one job spec on the worker thread. Returns an optional result
/// payload attached to the final `done` state event.
#[allow(clippy::too_many_arguments)] // worker entry point, each role distinct
fn run_job(
    spec: &JobSpec,
    gui_id: u64,
    state: &AppState,
    settings: &SettingsDto,
    bridge: &Arc<AskBridge>,
    events: &Arc<dyn EventSink>,
    ctl: &ControlToken,
    cancel_flag: &Arc<AtomicBool>,
    sink: &dyn ProgressSink,
) -> Result<Option<serde_json::Value>, FormatError> {
    match spec {
        JobSpec::Compress {
            inputs,
            dest,
            level,
            password,
            encrypt_names,
            split_size,
            excludes,
        } => {
            let opts = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                password: password.as_deref().map(Password::new),
                encrypt_filenames: *encrypt_names,
                split_size: *split_size,
                resources: settings.resource_options(),
                excludes: excludes.clone(),
                ..CreateOptions::default()
            };
            let inputs: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
            state
                .engine
                .create(Path::new(dest), &inputs, &opts, sink, ctl)?;
            Ok(None)
        }
        JobSpec::Extract {
            path,
            dest,
            selection,
            overwrite,
            symlinks,
            smart,
            encoding,
            password,
            best_effort,
        } => {
            let archive = PathBuf::from(path);
            let dest = PathBuf::from(dest);
            let result = run_extract_archive_job(
                state,
                settings,
                bridge,
                events,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                &archive,
                &dest,
                selection.as_deref(),
                overwrite,
                symlinks,
                *smart,
                encoding.clone(),
                password.as_deref(),
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
            let result = run_batch_extract_job(
                state,
                settings,
                bridge,
                events,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                items,
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
            let temp = extract_nested_archive_to_temp(
                state,
                &outer,
                entry_path,
                password.as_deref(),
                encoding.as_deref(),
            )?;
            let result = run_extract_archive_job(
                state,
                settings,
                bridge,
                events,
                ctl,
                cancel_flag,
                sink,
                gui_id,
                &temp,
                &dest,
                None,
                overwrite,
                symlinks,
                *smart,
                None,
                None,
                *best_effort,
            );
            let cleanup = fs::remove_file(&temp);
            if result.is_ok() {
                if let Err(e) = cleanup {
                    log::warn!(
                        "nested extract: could not remove temp {}: {e}",
                        temp.display()
                    );
                }
            }
            Ok(Some(result?))
        }
        JobSpec::Test {
            path,
            encoding,
            password,
        } => {
            let archive = PathBuf::from(path);
            let report = with_gui_password(
                state,
                bridge,
                &**events,
                ctl,
                cancel_flag,
                gui_id,
                &archive,
                password.as_deref(),
                |pw| {
                    let open = OpenOptions {
                        password: pw.cloned(),
                        encoding_override: encoding.clone(),
                    };
                    state.engine.test(&archive, &open, sink, ctl)
                },
            )?;
            Ok(Some(serde_json::json!({
                "ok": report.is_ok(),
                "entries": report.entries_tested,
                "problems": report.problems.len(),
            })))
        }
        JobSpec::Convert {
            src,
            dest,
            level,
            src_encoding,
            src_password,
            dest_password,
            encrypt_names,
        } => {
            let src_path = PathBuf::from(src);
            let create = CreateOptions {
                level: CompressionLevel::from_numeric(*level),
                password: dest_password.as_deref().map(Password::new),
                encrypt_filenames: *encrypt_names,
                resources: settings.resource_options(),
                ..CreateOptions::default()
            };
            with_gui_password(
                state,
                bridge,
                &**events,
                ctl,
                cancel_flag,
                gui_id,
                &src_path,
                src_password.as_deref(),
                |pw| {
                    let open = OpenOptions {
                        password: pw.cloned(),
                        encoding_override: src_encoding.clone(),
                    };
                    state
                        .engine
                        .convert(&src_path, Path::new(dest), &open, &create, sink, ctl)
                },
            )?;
            Ok(None)
        }
        JobSpec::ExportSqz {
            src,
            dest,
            level,
            dest_password,
        } => {
            let src_path = PathBuf::from(src);
            let dest_path = PathBuf::from(dest);
            if !is_sqz_source_path(&src_path) {
                return Err(FormatError::Unsupported(
                    "export expects a .sqz source container".into(),
                ));
            }
            if is_sqz_source_path(&dest_path) {
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
            state.engine.convert(
                &src_path,
                &dest_path,
                &OpenOptions::default(),
                &create,
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
            if !is_sqz_source_path(&src_path) {
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
            let test_report = state
                .engine
                .test(&src_path, &OpenOptions::default(), sink, ctl)?;
            if !test_report.is_ok() {
                return Err(FormatError::CorruptArchive(test_report.problems.join("; ")));
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
            if !is_plain_zip_path(&src_path) {
                return Err(FormatError::Unsupported(
                    "ZIP index rebuild expects a ZIP-family source archive".into(),
                ));
            }
            if !is_plain_zip_path(&dest_path) {
                return Err(FormatError::Unsupported(
                    "ZIP index rebuild output must be a ZIP-family archive".into(),
                ));
            }
            let test_report = state
                .engine
                .test(&src_path, &OpenOptions::default(), sink, ctl)?;
            if !test_report.is_ok() {
                return Err(FormatError::CorruptArchive(test_report.problems.join("; ")));
            }
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
                "source_entries": test_report.entries_tested,
            })))
        }
        JobSpec::Protect {
            path,
            redundancy,
            recovery,
        } => {
            ctl.checkpoint()?;
            sink.on_progress(0, 0, &EntryPath::from_utf8("Creating PAR2 recovery data"));
            let archive = PathBuf::from(path);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let report = squallz_recovery::protect(&archive, *redundancy, recovery.as_deref())?;
            finish_recovery_report(report, false)
        }
        JobSpec::VerifyRecovery { path, recovery } => {
            ctl.checkpoint()?;
            sink.on_progress(0, 0, &EntryPath::from_utf8("Verifying PAR2 recovery data"));
            let archive = PathBuf::from(path);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let report = squallz_recovery::verify(&archive, recovery.as_deref())?;
            finish_recovery_report(report, true)
        }
        JobSpec::RepairRecovery {
            path,
            output,
            recovery,
        } => {
            ctl.checkpoint()?;
            sink.on_progress(
                0,
                0,
                &EntryPath::from_utf8("Repairing with PAR2 recovery data"),
            );
            let archive = PathBuf::from(path);
            let output = output.as_deref().map(PathBuf::from);
            let recovery = recovery.as_deref().map(PathBuf::from);
            let report =
                squallz_recovery::repair(&archive, output.as_deref(), recovery.as_deref())?;
            finish_recovery_report(report, true)
        }
        JobSpec::Update {
            path,
            add,
            delete,
            rename,
            mkdir,
            excludes,
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
                excludes: excludes.clone(),
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
            let algorithm = parse_checksum_algorithm(algorithm)?;
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
            let algorithm = parse_checksum_algorithm(algorithm)?;
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

fn parse_checksum_algorithm(value: &str) -> Result<ChecksumAlgorithm, FormatError> {
    ChecksumAlgorithm::parse_alias(value).ok_or_else(|| {
        FormatError::Unsupported(format!("unsupported checksum algorithm: {}", value.trim()))
    })
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

fn finish_recovery_report(
    report: squallz_recovery::RecoveryReport,
    corrupt_on_failure: bool,
) -> Result<Option<serde_json::Value>, FormatError> {
    if report.ok {
        return serde_json::to_value(report)
            .map(Some)
            .map_err(|e| FormatError::Other(format!("cannot serialize recovery report: {e}")));
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
    if corrupt_on_failure {
        Err(FormatError::CorruptArchive(detail))
    } else {
        Err(FormatError::Other(detail))
    }
}

/// Expands a display-path selection against the entry list: items ending
/// with `/` select by prefix (whole directories), others match exactly.
fn expand_selection(entries: &[EntryMeta], selection: &[String]) -> Vec<EntryPath> {
    entries
        .iter()
        .filter(|e| {
            let display = crate::state::normalized_entry_path(e);
            selection.iter().any(|sel| {
                if sel.ends_with('/') {
                    display.starts_with(sel.as_str())
                } else {
                    display == *sel
                }
            })
        })
        .map(|e| e.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::{BatchExtractItem, JobSpec};
    use std::sync::Mutex as StdMutex;

    static EXTERNAL_TOOL_ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-jobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn nested_temp_files_for_process() -> Vec<PathBuf> {
        let prefix = format!("squallz-nested-{}-", std::process::id());
        let mut files: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
            })
            .collect();
        files.sort();
        files
    }

    fn nested_temp_files_for_entry(entry_name: &str) -> Vec<PathBuf> {
        let suffix = format!("-{entry_name}");
        nested_temp_files_for_process()
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(&suffix))
            })
            .collect()
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

    fn done_result(events: &[(String, serde_json::Value)], id: u64) -> Option<serde_json::Value> {
        events
            .iter()
            .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "done")
            .and_then(|(_, p)| p.get("result").cloned())
    }

    fn poison_lock<T>(mutex: &std::sync::Mutex<T>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison lock for regression coverage");
        }));
        assert!(result.is_err());
    }

    #[test]
    fn job_manager_map_recovers_after_poison() {
        let dir = temp_dir("map-poison");
        let src = dir.join("data");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hello poison").unwrap();
        let zip = dir.join("poison.zip");

        let manager = JobManager::new();
        poison_lock(&manager.map);

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
                excludes: vec![],
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(manager.queue_id(id).is_some());
        manager.pause(&*events, id);
        manager.resume(&*events, id);
        manager.cancel(&*events, id);
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn job_local_locks_recover_after_poison() {
        let sink = Arc::new(TestSink::default());
        let events: Arc<dyn EventSink> = sink.clone();
        let no_progress = squallz_core::api::NoProgress;
        let progress = EmitProgress::new(42, Arc::clone(&events), &no_progress);
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

        let collector = ExtractProblemCollector::default();
        poison_lock(&collector.problems);
        collector.skipped_entry(&entry, &FormatError::Other("damaged sample".into()));
        assert_eq!(collector.count(), 1);
        assert!(collector.preview()[0].contains("payload.bin"));

        let dir = temp_dir("resolver-poison");
        let existing = dir.join("exists.txt");
        std::fs::write(&existing, b"old").unwrap();
        let resolver = GuiConflictResolver {
            gui_id: 7,
            events,
            bridge: Arc::new(AskBridge::default()),
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
                excludes: vec![],
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
                selection: None,
                overwrite: "skip".into(),
                symlinks: "preserve".into(),
                smart: false,
                encoding: None,
                password: None,
                best_effort: false,
            },
            SettingsDto::default(),
        );
        manager.wait_idle();

        assert!(out.join("data/hello.txt").exists());
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id1), vec!["queued", "running", "done"]);
        assert_eq!(states_of(&events, id2), vec!["queued", "running", "done"]);
        assert_real_current_file_progress(&events, id1, "compress");
        assert_real_current_file_progress(&events, id2, "extract");
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
                overwrite: "skip".into(),
                symlinks: "preserve".into(),
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
        assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
            && payload["id"] == id
            && payload["total"].as_u64().unwrap_or(0) == 2 * BATCH_PROGRESS_SCALE));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_password_prompt_reports_cancelled_without_poll_delay() {
        let dir = temp_dir("cancel-password-latency");
        let src = dir.join("secret-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("secret.txt"), b"cancel latency").unwrap();
        let archive = dir.join("secret.zip");
        let state = Arc::new(AppState::new());
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

        let manager = JobManager::new();
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

        wait_for_event(
            &sink,
            std::time::Duration::from_secs(2),
            |(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id,
        );
        let cancel_start = Instant::now();
        manager.cancel(&*events, id);
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
                excludes: vec![],
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
    fn extract_nested_job_extracts_inner_archive_and_cleans_temp() {
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

        for path in nested_temp_files_for_entry(inner_name) {
            let _ = std::fs::remove_file(path);
        }
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
                overwrite: "skip".into(),
                symlinks: "preserve".into(),
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
        assert_eq!(
            nested_temp_files_for_entry(inner_name),
            Vec::<PathBuf>::new()
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
        let expected_dest = out.to_string_lossy().into_owned();
        assert_eq!(
            done_result(&events, id)
                .and_then(|value| value["dest"].as_str().map(str::to_owned))
                .as_deref(),
            Some(expected_dest.as_str())
        );

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
            .test(
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
            .test(
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
            .test(
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
            .test(
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
    fn update_job_add_directory_applies_excludes() {
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
        std::fs::write(extra.join("keep.txt"), b"keep").unwrap();
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
        assert!(!names.iter().any(|name| name.contains("node_modules")));
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
                algorithm: "sha256".into(),
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
                algorithm: "sha256".into(),
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
                selection: None,
                overwrite: "skip".into(),
                symlinks: "preserve".into(),
                smart: false,
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
                selection: None,
                overwrite: "rename".into(),
                symlinks: "preserve".into(),
                smart: false,
                encoding: None,
                password: None,
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
        assert_eq!(result["skipped"], 1);
        assert!(result["problems"][0].as_str().unwrap().contains("bad.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_job_bridges_to_external_par2_tool() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();

        let dir = temp_dir("recovery");
        let archive = dir.join("protected.zip");
        let recovery = dir.join("protected.zip.par2");
        let tool = dir.join("fake-par2");
        let log = dir.join("fake-par2.log");
        std::fs::write(&archive, b"archive bytes").unwrap();
        std::fs::write(
            &tool,
            r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
    printf 'fake recovery data\n' > "$3"
    ;;
  verify|repair)
    test -f "$2" || exit 2
    if [ "$1" = repair ]; then
      target="${2%.par2}"
      printf 'repaired bytes\n' > "$target"
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
                recovery: Some(recovery.to_string_lossy().into_owned()),
            },
            SettingsDto::default(),
        );
        let copy_archive = dir.join("damaged.zip");
        let copy_recovery = dir.join("damaged.zip.par2");
        let copy_output = dir.join("restored.zip");
        std::fs::write(&copy_archive, b"damaged bytes").unwrap();
        std::fs::write(&copy_recovery, b"fake recovery data").unwrap();
        let repair_copy_id = manager.submit(
            Arc::clone(&state),
            Arc::clone(&events),
            JobSpec::RepairRecovery {
                path: copy_archive.to_string_lossy().into_owned(),
                output: Some(copy_output.to_string_lossy().into_owned()),
                recovery: Some(copy_recovery.to_string_lossy().into_owned()),
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
        assert!(recovery.is_file());
        assert_eq!(
            done_result(&events, protect_id)
                .and_then(|v| v["operation"].as_str().map(str::to_owned)),
            Some("protect".to_owned())
        );
        let copy_result = done_result(&events, repair_copy_id).unwrap();
        assert_eq!(copy_result["operation"].as_str(), Some("repair"));
        assert_eq!(
            copy_result["output"].as_str(),
            Some(copy_output.to_string_lossy().as_ref())
        );
        assert_eq!(std::fs::read(&copy_archive).unwrap(), b"damaged bytes");
        assert_eq!(std::fs::read(&copy_output).unwrap(), b"repaired bytes\n");
        let log = std::fs::read_to_string(&log).unwrap();
        assert!(log.contains("create -r12"), "log: {log}");
        assert!(log.contains("verify"), "log: {log}");
        assert!(log.contains("repair"), "log: {log}");

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
