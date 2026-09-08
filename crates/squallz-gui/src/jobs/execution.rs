//! Adapts queued GUI jobs to shared core operations and interactive prompts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use squallz_core::api::{
    ArchiveSourceSet, ArchiveStructureStatus, BoundedProblemLog, CompressionLevel,
    ConflictDecision, ConflictResolver, ControlToken, CreateOptions, EntryMeta, EntryPath,
    ExtractProblemReporter, ExtractReport, FormatError, OpenOptions, OverwritePolicy, Password,
    ProblemPreview, ProgressPhase, ProgressSink, RecoverySummary, SymlinkPolicy, UpdateOp,
};
use squallz_core::{
    create_destination_has_conflict, is_plain_sqz_path, is_sqz_archive_path, is_zip_family_path,
    lock_unpoisoned, CreateArtifactKind, CreateCommitPolicy, CreateDestinationGuard, CreateReport,
    Engine, ExtractInputGuard, ExtractPlan, PostSuccessAction, SfxBuildOptions, SfxBuildReport,
    SfxTarget,
};
use squallz_publish::{publish_macos_sfx, MacosSfxPublishPhase};

use crate::audit;
use crate::bridge::{AskAnswer, AskBridge};
use crate::dto::{
    AskConflictEvent, AskPasswordEvent, BatchExtractItem, ErrorDto, ExtractPlanDto, JobSpec,
    SettingsDto,
};
use crate::events::{emit, EventSink, EV_ASK_CONFLICT, EV_ASK_PASSWORD};
use crate::nested::{create_nested_job_workspace, extract_nested_archive_to_temp_for_job};
use crate::state::AppState;

use super::progress::BatchProgressSink;
use super::redact_format_error_path;
use super::snapshots::{JobInteraction, JobSnapshotStore};
use super::source_cleanup::{
    prepare_source_cleanup, SourceCleanup, SourceCleanupResult, SourceCleanupStatus,
};

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

#[derive(Clone, Copy)]
pub(super) struct JobContext<'a> {
    pub(super) gui_id: u64,
    pub(super) state: &'a AppState,
    pub(super) settings: &'a SettingsDto,
    pub(super) bridge: &'a Arc<AskBridge>,
    pub(super) events: &'a Arc<dyn EventSink>,
    pub(super) ctl: &'a ControlToken,
    pub(super) cancel_flag: &'a Arc<AtomicBool>,
    pub(super) sink: &'a dyn ProgressSink,
    pub(super) snapshots: &'a Arc<Mutex<JobSnapshotStore>>,
    pub(super) sfx_template: Option<&'a Path>,
    pub(super) source_cleanup: &'a SourceCleanup,
}

impl JobContext<'_> {
    /// Retries password-protected work through the shared dialog and caches a
    /// proven-good prompted password for the session.
    fn with_password<R>(
        &self,
        archive: &Path,
        display_name: Option<&str>,
        explicit: Option<&str>,
        mut f: impl FnMut(Option<&Password>) -> Result<R, FormatError>,
    ) -> Result<R, FormatError> {
        let Self {
            state,
            bridge,
            events,
            snapshots,
            ctl,
            cancel_flag,
            gui_id,
            ..
        } = *self;
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
                        &**events,
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

struct ExtractRequest<'a> {
    archive: &'a Path,
    archive_display_name: &'a str,
    dest: &'a Path,
    expected_destination: Option<&'a Path>,
    expected_input_guard: Option<ExtractInputGuard>,
    selection: Option<&'a [String]>,
    overwrite: OverwritePolicy,
    symlinks: SymlinkPolicy,
    smart: bool,
    encoding: Option<&'a str>,
    password: Option<&'a str>,
    verify_sfx: bool,
    best_effort: bool,
}

impl JobContext<'_> {
    fn extract(
        &self,
        request: ExtractRequest<'_>,
    ) -> Result<(serde_json::Value, ArchiveStructureStatus), FormatError> {
        let Self {
            state,
            settings,
            bridge,
            events,
            snapshots,
            ctl,
            cancel_flag,
            sink,
            gui_id,
            ..
        } = *self;
        let ExtractRequest {
            archive,
            archive_display_name,
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
        } = request;
        if verify_sfx {
            squallz_core::verify_sfx_payload(archive, &settings.resource_options(), sink, ctl)?;
        }
        let policy = overwrite;
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
            symlinks,
            limits: settings.safety_limits(),
            resources: settings.resource_options(),
            best_effort,
            problem_reporter,
            ..Default::default()
        };
        let archive = archive.to_path_buf();
        let dest = dest.to_path_buf();
        let (plan, report, structure) =
            self.with_password(&archive, Some(archive_display_name), password, |pw| {
                let open = OpenOptions {
                    password: pw.cloned(),
                    encoding_override: encoding.map(str::to_owned),
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
            })?;
        let result = extract_result_json(
            plan,
            report,
            structure,
            best_effort,
            problem_collector.summary(),
        );
        Ok((result, structure))
    }

    fn extract_batch(
        &self,
        items: &[BatchExtractItem],
        display_items: &[BatchExtractItem],
        overwrite: &OverwritePolicy,
        symlinks: &SymlinkPolicy,
        smart: bool,
    ) -> Result<serde_json::Value, FormatError> {
        let Self {
            state, ctl, sink, ..
        } = *self;
        if items.is_empty() {
            return Err(FormatError::Unsupported(
                "batch extract requires at least one archive".into(),
            ));
        }

        let work_items = normalize_batch_extract_items_with(items, display_items, |path| {
            state.engine.archive_source_set(path)
        });
        let batch_sink = BatchProgressSink::new(sink, work_items.len());
        let batch_context = JobContext {
            sink: &batch_sink,
            ..*self
        };
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
            match batch_context.extract(ExtractRequest {
                archive: &archive,
                archive_display_name: &label,
                dest: &dest,
                expected_destination: None,
                expected_input_guard: None,
                selection: None,
                overwrite: *overwrite,
                symlinks: *symlinks,
                smart,
                encoding: item.encoding.as_deref(),
                password: item.password.as_deref(),
                verify_sfx: false,
                best_effort: item.best_effort,
            }) {
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

impl JobContext<'_> {
    pub(super) fn run(
        &self,
        spec: &JobSpec,
        display_spec: &JobSpec,
    ) -> Result<Option<serde_json::Value>, FormatError> {
        let Self {
            state,
            settings,
            ctl,
            cancel_flag,
            sink,
            sfx_template,
            source_cleanup,
            ..
        } = *self;
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
                    let source_cleanup = source_cleanup.complete(
                        cleanup_plan,
                        &report.outputs,
                        &archived_manifest,
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
                let source_cleanup = source_cleanup.complete(
                    cleanup_plan,
                    std::slice::from_ref(&report.path),
                    &archived_manifest,
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
                let (result, _) = self.extract(ExtractRequest {
                    archive: &archive,
                    archive_display_name: &display_name,
                    dest: &dest,
                    expected_destination: expected_destination.as_deref().map(Path::new),
                    expected_input_guard: *expected_input_guard,
                    selection: selection.as_deref(),
                    overwrite: *overwrite,
                    symlinks: *symlinks,
                    smart: *smart,
                    encoding: encoding.as_deref(),
                    password: password.as_deref(),
                    verify_sfx: *verify_sfx,
                    best_effort: *best_effort,
                })?;
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
                let result =
                    self.extract_batch(items, display_items, overwrite, symlinks, *smart)?;
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
                let temp = self.with_password(
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
                let extraction = self.extract(ExtractRequest {
                    archive: &temp_path,
                    archive_display_name: display_name,
                    dest: &dest,
                    expected_destination: None,
                    expected_input_guard: None,
                    selection: None,
                    overwrite: *overwrite,
                    symlinks: *symlinks,
                    smart: *smart,
                    encoding: None,
                    password: None,
                    verify_sfx: false,
                    best_effort: *best_effort,
                });
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
                let outcome =
                    self.with_password(&archive, Some(&display_name), password.as_deref(), |pw| {
                        let open = OpenOptions {
                            password: pw.cloned(),
                            encoding_override: encoding.clone(),
                        };
                        state
                            .engine
                            .test_summary_with_structure(&archive, &open, sink, ctl)
                    })?;
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
                let report = self.with_password(
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
mod tests;
