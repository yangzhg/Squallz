//! Tauri command surface: thin wrappers over [`AppState`], [`JobManager`]
//! and [`SettingsStore`]. All real logic lives in those modules so it can
//! be tested without a window.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, EventTarget, Manager, State, WebviewWindow};
use tauri_plugin_opener::OpenerExt;

use squallz_core::api::{
    ControlToken, Detected, FormatError, FormatKind, OpenOptions, OverwritePolicy, Password,
    ProgressSink, SymlinkPolicy,
};
#[cfg(test)]
use squallz_core::api::{NoProgress, SafetyLimits};
use squallz_core::{
    create_destination_has_conflict as core_create_destination_has_conflict,
    find_available_create_destination as core_find_available_create_destination,
    inspect_create_destination_with_progress as inspect_core_create_destination_with_progress,
    parent_or_current, ChecksumAlgorithm, CreateArtifactKind, CreateCompletionAction,
    CreateContentPolicy, CreateCredential, CreateOutput, EntryNameEncoding, PostSuccessAction,
    PresetDocument, PresetError, PresetStore, SfxTarget, VolumeMode,
};
use squallz_i18n::Localizer;

use crate::audit::{OperationAudit, OperationAuditRecord};
use crate::create_preflight::{
    DestinationInspectionProgress, PreflightRequestKind, PreflightRequests,
};
use crate::dto::{
    normalize_performance_stream_buffer_limit, ArchiveInfo, BatchExtractItem,
    CreateDestinationInspectionDto, CreateEstimateDto, CreatePlanDto, DiskSpaceDto, EntryDto,
    EntryPreviewDto, ErrorDto, ExternalTaskActionDto, ExtractPlanPreflightDto, FormatDto,
    IntegrationApplyResultDto, IntegrationRemoveResultDto, IntegrationStatusDto,
    IntegrationSystemDiagnosticsDto, JobSpec, LanguageDto, LocaleTable, NestedArchivePreviewDto,
    Page, PasswordBookStatusDto, SettingsDto, SfxCreateCapabilityDto,
};
use crate::events::EventSink;
use crate::integration;
use crate::jobs::{
    convert_create_options, create_job_request, expand_selection_with_control, JobManager,
    JobSnapshotDelta, JobStateSnapshot, SourceCleanupRecoveryNotice, MAX_PARALLEL_JOBS,
};
use crate::nested::extract_nested_archive_to_temp_limited;
use crate::open_files::{focus_main_window, OpenFileRequests, OpenFilesEvent};
use crate::preview_sessions::{
    preview_failure_kind, PreviewSessionManager, MAX_PREVIEW_ENTRY_BYTES,
};
use crate::secrets::{SecretStore, SharedSecretStore};
use crate::settings::SettingsStore;
use crate::state::{normalized_entry_path, AppState, DEFAULT_PAGE_SIZE};
use crate::validation_trace;
use serde_json::json;

const HISTORY_EXPORT_MAX_BYTES: usize = 1024 * 1024;
const PREFLIGHT_PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
const NESTED_PREVIEW_LIMIT: usize = 200;
const MAX_ARCHIVE_PAGE_SIZE: usize = 2_000;
const DEFAULT_OPERATION_AUDIT_LIMIT: usize = 80;
const CREATE_DESTINATION_PROBE_ATTEMPTS: usize = 32;

static CREATE_DESTINATION_PROBE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// `EventSink` backed by the real Tauri app handle.
pub struct TauriEvents {
    app: AppHandle,
    window_label: String,
}

impl TauriEvents {
    fn new(app: AppHandle, window: &WebviewWindow) -> Self {
        Self {
            app,
            window_label: window.label().to_owned(),
        }
    }
}

impl EventSink for TauriEvents {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        let target = EventTarget::webview_window(&self.window_label);
        if let Err(e) = self.app.emit_to(target, event, payload) {
            log::error!("events: emit {event} failed: {e}");
        }
    }
}

fn password_error(e: &FormatError) -> bool {
    matches!(
        e,
        FormatError::PasswordRequired | FormatError::WrongPassword
    )
}

#[cfg(test)]
fn open_archive_resolving_password(
    state: &AppState,
    secrets: &dyn SecretStore,
    owner_window: &str,
    path: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<ArchiveInfo, FormatError> {
    open_archive_resolving_password_with_entry_limit_and_control(
        state,
        secrets,
        owner_window,
        path,
        password,
        encoding,
        SafetyLimits::default().max_entries,
        &ControlToken::default(),
    )
}

#[allow(clippy::too_many_arguments)] // Each argument has a distinct archive-open role.
fn open_archive_resolving_password_with_entry_limit_and_control(
    state: &AppState,
    secrets: &dyn SecretStore,
    owner_window: &str,
    path: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
    max_entries: u64,
    control: &ControlToken,
) -> Result<ArchiveInfo, FormatError> {
    control.checkpoint()?;
    if let Some(password) = password {
        return state.open_archive_for_window_with_entry_limit_and_control(
            owner_window,
            path,
            Some(password),
            encoding,
            max_entries,
            control,
        );
    }

    match state.open_archive_for_window_with_entry_limit_and_control(
        owner_window,
        path,
        None,
        encoding,
        max_entries,
        control,
    ) {
        Ok(info) => Ok(info),
        Err(e) if password_error(&e) => {
            control.checkpoint()?;
            match secrets.get_archive_password(path) {
                Ok(Some(saved)) => {
                    control.checkpoint()?;
                    match state.open_archive_for_window_with_entry_limit_and_control(
                        owner_window,
                        path,
                        Some(saved.expose()),
                        encoding,
                        max_entries,
                        control,
                    ) {
                        Ok(info) => Ok(info),
                        Err(saved_error) if password_error(&saved_error) => {
                            Err(FormatError::WrongPassword)
                        }
                        Err(saved_error) => Err(saved_error),
                    }
                }
                Ok(None) => Err(e),
                Err(secret_error) => {
                    log::warn!("password book: cannot read stored password: {secret_error}");
                    Err(e)
                }
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
fn open_archive_source_resolving_password(
    state: &AppState,
    secrets: &dyn SecretStore,
    owner_window: &str,
    source: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<ArchiveInfo, ErrorDto> {
    open_archive_source_resolving_password_with_entry_limit_and_control(
        state,
        secrets,
        owner_window,
        source,
        password,
        encoding,
        SafetyLimits::default().max_entries,
        &ControlToken::default(),
    )
}

#[allow(clippy::too_many_arguments)] // Each argument has a distinct archive-open role.
fn open_archive_source_resolving_password_with_entry_limit_and_control(
    state: &AppState,
    secrets: &dyn SecretStore,
    owner_window: &str,
    source: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    max_entries: u64,
    control: &ControlToken,
) -> Result<ArchiveInfo, ErrorDto> {
    control.checkpoint().map_err(ErrorDto::from)?;
    let resolved = state
        .resolve_archive_source(source, Some(owner_window))
        .map_err(preview_error_dto)?;
    control.checkpoint().map_err(ErrorDto::from)?;
    if resolved.is_read_only() {
        return state
            .open_archive_source_with_entry_limit_and_control(
                owner_window,
                source,
                password,
                encoding,
                max_entries,
                control,
            )
            .map_err(|error| {
                preview_error_dto_with_paths(error, &[(resolved.path(), resolved.display_path())])
            });
    }
    open_archive_resolving_password_with_entry_limit_and_control(
        state,
        secrets,
        owner_window,
        resolved.path(),
        password,
        encoding,
        max_entries,
        control,
    )
    .map_err(ErrorDto::from)
}

fn archive_password_status_impl(
    secrets: &dyn SecretStore,
    path: &Path,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    let available = secrets.is_available();
    let saved = if available {
        secrets
            .has_archive_password(path)
            .map_err(|error| ErrorDto::secret_store(error.to_string()))?
    } else {
        false
    };
    Ok(PasswordBookStatusDto { available, saved })
}

fn remember_archive_password_impl(
    state: &AppState,
    secrets: &dyn SecretStore,
    path: &Path,
    password: &str,
    encoding: Option<&str>,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    if !secrets.is_available() {
        return Err(ErrorDto::secret_store(
            "persistent secret storage is not available on this platform",
        ));
    }
    state
        .verify_password(path, password, encoding)
        .map_err(ErrorDto::from)?;
    secrets
        .set_archive_password(path, password)
        .map_err(|error| ErrorDto::secret_store(error.to_string()))?;
    state.remember_password(path, password);
    Ok(PasswordBookStatusDto {
        available: true,
        saved: true,
    })
}

fn forget_archive_password_impl(
    state: &AppState,
    secrets: &dyn SecretStore,
    path: &Path,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    state.forget_password(path);
    secrets
        .delete_archive_password(path)
        .map_err(|error| ErrorDto::secret_store(error.to_string()))?;
    Ok(PasswordBookStatusDto {
        available: secrets.is_available(),
        saved: false,
    })
}

/// Opens an archive and caches its entry list. `PasswordRequired` comes
/// back as a structured error so the frontend can prompt and retry.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Mirrors the frontend invocation fields.
pub async fn open_archive(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    requests: State<'_, Arc<PreflightRequests>>,
    settings: State<'_, Arc<SettingsStore>>,
    path: String,
    password: Option<String>,
    encoding: Option<String>,
    request_id: Option<String>,
) -> Result<ArchiveInfo, ErrorDto> {
    let state = Arc::clone(state.inner());
    let secrets = Arc::clone(secrets.inner());
    let requests = Arc::clone(requests.inner());
    let max_entries = settings.get().safety_limits().max_entries;
    let owner_window = window.label().to_owned();
    let request = if let Some(request_id) = request_id.as_deref() {
        requests.begin_request(PreflightRequestKind::OpenArchive, &owner_window, request_id)
    } else {
        requests.begin_anonymous_request(PreflightRequestKind::OpenArchive, &owner_window)
    };
    let worker_control = request.control();
    let trace_path = path.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _request = request;
        open_archive_source_resolving_password_with_entry_limit_and_control(
            &state,
            secrets.as_ref(),
            &owner_window,
            &path,
            password.as_deref(),
            encoding.as_deref(),
            max_entries,
            &worker_control,
        )
    })
    .await;
    let result =
        task.map_err(|error| ErrorDto::other(format!("archive open task failed: {error}")))?;
    match &result {
        Ok(info) => validation_trace::trace(
            "open_archive.ok",
            json!({
                "path": trace_path,
                "format": info.format,
                "entry_count": info.entry_count,
            }),
        ),
        Err(e) => validation_trace::trace(
            "open_archive.err",
            json!({
                "path": trace_path,
                "error": e.key,
            }),
        ),
    }
    result
}

/// Cancels the matching archive-open request owned by this WebView.
#[tauri::command]
pub fn cancel_archive_open(
    window: WebviewWindow,
    requests: State<'_, Arc<PreflightRequests>>,
    request_id: String,
) {
    requests.cancel(
        PreflightRequestKind::OpenArchive,
        window.label(),
        &request_id,
    );
}

/// Releases a cached archive.
#[tauri::command]
pub fn close_archive(window: WebviewWindow, state: State<'_, Arc<AppState>>, id: u64) {
    state.close_archive_for_window(window.label(), id);
}

/// Records frontend-only validation evidence when `SQUALLZ_VALIDATION_TRACE` is set.
/// Normal app sessions do not create files or change behavior.
#[allow(dead_code)] // invoked from the frontend through Tauri's command macro
#[tauri::command]
pub fn record_validation_event(event: String, payload: serde_json::Value) {
    if event.starts_with("frontend.") {
        validation_trace::trace(&event, payload);
    }
}

/// Lets frontend-only screenshot surfaces avoid persisted WebView noise during
/// validation runs without touching normal user history.
#[allow(dead_code)] // invoked from the frontend through Tauri's command macro
#[tauri::command]
pub fn is_validation_session() -> bool {
    std::env::var_os("SQUALLZ_VALIDATION_TRACE").is_some()
}

/// Reports the native platform from Rust so layout does not rely on WebView
/// user-agent quirks.
#[allow(dead_code)] // invoked from the frontend through Tauri's command macro
#[tauri::command]
pub fn platform_kind() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Supplies deterministic drop paths to packaged-app validation runs. This is a
/// no-op unless `SQUALLZ_VALIDATION_DROP_PATHS` is present in the app environment.
#[allow(dead_code)] // invoked from the frontend through Tauri's command macro
#[tauri::command]
pub fn take_validation_drop_paths() -> Vec<String> {
    let Ok(raw) = std::env::var("SQUALLZ_VALIDATION_DROP_PATHS") else {
        return Vec::new();
    };
    let mut paths: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if paths.len() <= 1 {
        paths = raw
            .split('|')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    validation_trace::trace("validation_drop.take", json!({ "paths": paths.clone() }));
    paths
}

/// Pages one directory level of an opened archive (500/page by default).
#[tauri::command]
pub fn list_entries(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: u64,
    page: usize,
    page_size: Option<usize>,
    dir_prefix: Option<String>,
    filter: Option<String>,
) -> Result<Page, ErrorDto> {
    state
        .list_entries_for_window(
            window.label(),
            id,
            page,
            requested_page_size(page_size),
            requested_dir_prefix(dir_prefix.as_deref()),
            filter.as_deref(),
        )
        .map_err(ErrorDto::from)
}

/// Pages archive-wide, case-insensitive path matches for an opened archive.
#[tauri::command]
pub async fn search_entries(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: u64,
    page: usize,
    page_size: Option<usize>,
    query: String,
    generation: u64,
) -> Result<Option<Page>, ErrorDto> {
    let state = Arc::clone(state.inner());
    let owner_window = window.label().to_owned();
    let page_size = requested_page_size(page_size);
    tauri::async_runtime::spawn_blocking(move || {
        state.search_entries_for_window(&owner_window, id, page, page_size, &query, generation)
    })
    .await
    .map_err(|error| ErrorDto::other(format!("archive search task failed: {error}")))?
    .map_err(ErrorDto::from)
}

/// Cancels an older archive-wide scan when the UI clears search or navigates.
#[tauri::command]
pub fn cancel_archive_search(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    id: u64,
    generation: u64,
) -> Result<(), ErrorDto> {
    state
        .cancel_search_for_window(window.label(), id, generation)
        .map_err(ErrorDto::from)
}

fn requested_page_size(page_size: Option<usize>) -> usize {
    page_size
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_ARCHIVE_PAGE_SIZE)
}

fn requested_dir_prefix(dir_prefix: Option<&str>) -> &str {
    dir_prefix.map_or("", |dir_prefix| dir_prefix)
}

/// All registered formats with capabilities (drives the compress dialog).
/// Compound `tar.<compressor>` formats are synthesized from the registry so
/// the dropdown stays capability-driven without hardcoding.
#[tauri::command]
pub fn get_formats(state: State<'_, Arc<AppState>>) -> Vec<FormatDto> {
    let formats = state.engine.supported_formats();
    let mut out: Vec<FormatDto> = formats
        .iter()
        .map(|f| FormatDto {
            id: f.id.to_owned(),
            extensions: f.extensions.iter().map(|e| (*e).to_owned()).collect(),
            kind: match f.kind {
                FormatKind::Archive => "archive".to_owned(),
                FormatKind::Compressor => "compressor".to_owned(),
            },
            can_create: f.capabilities.can_create,
            can_extract: f.capabilities.can_extract,
            can_encrypt_data: f.capabilities.can_encrypt_data,
            can_encrypt_names: f.capabilities.can_encrypt_names,
            can_split: f.capabilities.can_split,
            can_update: f.capabilities.can_update,
            can_test: f.capabilities.can_test,
        })
        .collect();
    // tar + each registered compressor yields compound formats such as
    // tar.gz and tar.zst; the engine streams these without temp files.
    let tar = formats
        .iter()
        .find(|f| f.id == "tar" && f.capabilities.can_create);
    if let Some(tar) = tar {
        for comp in formats.iter().filter(|f| f.kind == FormatKind::Compressor) {
            let Some(ext) = comp.extensions.first() else {
                continue;
            };
            out.push(FormatDto {
                id: format!("tar.{ext}"),
                extensions: vec![format!("tar.{ext}")],
                kind: "archive".to_owned(),
                can_create: true,
                can_extract: true,
                can_encrypt_data: false,
                can_encrypt_names: false,
                can_split: tar.capabilities.can_split,
                can_update: false,
                can_test: true,
            });
        }
    }
    out
}

/// Folder-name stem of an archive path (`backup.tar.gz` → `backup`); the
/// extract dialog suggests `<dir>/<stem>` as the destination.
#[tauri::command]
pub fn archive_stem(state: State<'_, Arc<AppState>>, path: String) -> String {
    state.engine.archive_stem(Path::new(&path))
}

/// Returns the input-only estimate exposed by older desktop clients.
#[tauri::command]
pub async fn estimate_create_inputs(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    inputs: Vec<String>,
    excludes: Vec<String>,
    destination: String,
    split_output: bool,
    request_id: String,
) -> Result<CreateEstimateDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let events = TauriEvents::new(window.app_handle().clone(), &window);
    let inputs: Vec<PathBuf> = inputs.into_iter().map(PathBuf::from).collect();
    let destination = PathBuf::from(destination);
    tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now() - PREFLIGHT_PROGRESS_INTERVAL;
        let mut scanned_entries = 0usize;
        let estimate = state
            .engine
            .estimate_create_inputs_for_output_with_progress(
                &inputs,
                &excludes,
                &destination,
                split_output,
                |scanned, current| {
                    scanned_entries = scanned;
                    if scanned == 1
                        || scanned.is_multiple_of(128)
                        || last_emit.elapsed() >= PREFLIGHT_PROGRESS_INTERVAL
                    {
                        last_emit = Instant::now();
                        events.emit_json(
                            "create://preflight",
                            json!({
                                "request_id": request_id.as_str(),
                                "phase": "scanning",
                                "scanned": scanned,
                                "current": current,
                            }),
                        );
                    }
                },
            )
            .map_err(ErrorDto::from)?;
        events.emit_json(
            "create://preflight",
            json!({
                "request_id": request_id.as_str(),
                "phase": "done",
                "scanned": scanned_entries,
                "current": "",
            }),
        );
        Ok(CreateEstimateDto::from(estimate))
    })
    .await
    .map_err(|error| ErrorDto::other(format!("input estimate task failed: {error}")))?
}

/// Plans a frozen create job with the same options and output layout the
/// worker will execute. The returned budgets are conservative guardrails,
/// while the input totals come from a real source walk.
#[tauri::command]
pub async fn plan_create(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    jobs: State<'_, Arc<JobManager>>,
    settings: State<'_, Arc<SettingsStore>>,
    spec: JobSpec,
    request_id: String,
) -> Result<CreatePlanDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let request = create_job_request(&spec, &settings.get()).map_err(ErrorDto::from)?;
    let sfx_template = jobs.sfx_template_path();
    let events = TauriEvents::new(app, &window);
    tauri::async_runtime::spawn_blocking(move || -> Result<CreatePlanDto, FormatError> {
        let mut last_emit = Instant::now() - PREFLIGHT_PROGRESS_INTERVAL;
        let mut scanned_entries = 0usize;
        let plan = {
            let mut emit_progress = |scanned: usize, current: &str| {
                scanned_entries = scanned;
                if scanned == 1
                    || scanned.is_multiple_of(128)
                    || last_emit.elapsed() >= PREFLIGHT_PROGRESS_INTERVAL
                {
                    last_emit = Instant::now();
                    events.emit_json(
                        "create://preflight",
                        json!({
                            "request_id": request_id.as_str(),
                            "phase": "scanning",
                            "scanned": scanned,
                            "current": current,
                        }),
                    );
                }
            };
            match request.sfx_options() {
                Some(sfx_options) => {
                    let template = sfx_template.as_deref().ok_or_else(|| {
                        FormatError::DependencyMissing("Squallz SFX runtime template".into())
                    })?;
                    state.engine.plan_sfx_from_inputs_with_progress(
                        template,
                        &request.inputs,
                        &request.dest,
                        &request.options,
                        &sfx_options,
                        &mut emit_progress,
                    )
                }
                None => state.engine.plan_create_with_progress(
                    &request.dest,
                    &request.inputs,
                    &request.options,
                    &mut emit_progress,
                ),
            }
        }?;
        let plan = CreatePlanDto::from(plan).with_scanned_entries(scanned_entries);
        events.emit_json(
            "create://preflight",
            json!({
                "request_id": request_id.as_str(),
                "phase": "done",
                "scanned": scanned_entries,
                "current": "",
            }),
        );
        Ok(plan)
    })
    .await
    .map_err(|e| ErrorDto::other(format!("create plan task failed: {e}")))?
    .map_err(ErrorDto::from)
}

fn plan_convert_impl(
    state: &AppState,
    owner_window: &str,
    settings: &SettingsDto,
    spec: &JobSpec,
    control: &ControlToken,
) -> Result<CreatePlanDto, ErrorDto> {
    let JobSpec::Convert {
        src,
        dest,
        src_encoding,
        src_password,
        ..
    } = spec
    else {
        return Err(ErrorDto::from(FormatError::Unsupported(
            "conversion planning requires a convert job".into(),
        )));
    };
    control.checkpoint().map_err(ErrorDto::from)?;
    let resolved = state
        .resolve_archive_source(src, Some(owner_window))
        .map_err(preview_error_dto)?;
    control.checkpoint().map_err(ErrorDto::from)?;
    let open = OpenOptions {
        password: src_password
            .as_deref()
            .map(Password::new)
            .or_else(|| state.password_for(resolved.path())),
        encoding_override: src_encoding.clone(),
    };
    let options = convert_create_options(spec, settings).map_err(ErrorDto::from)?;
    state
        .engine
        .plan_convert_with_control(resolved.path(), Path::new(dest), &open, &options, control)
        .map(CreatePlanDto::from)
        .map_err(|error| {
            preview_error_dto_with_paths(error, &[(resolved.path(), resolved.display_path())])
        })
}

/// Plans a frozen conversion job from archive metadata without extracting
/// entry contents. The worker repeats the same budget check before writing.
#[tauri::command]
pub async fn plan_convert(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    requests: State<'_, Arc<PreflightRequests>>,
    settings: State<'_, Arc<SettingsStore>>,
    spec: JobSpec,
    request_id: String,
) -> Result<CreatePlanDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let requests = Arc::clone(requests.inner());
    let settings = settings.get();
    let owner_window = window.label().to_owned();
    let request = requests.begin_request(
        PreflightRequestKind::ConvertPlan,
        &owner_window,
        &request_id,
    );
    let token = request.control();
    let worker_token = Arc::clone(&token);
    tauri::async_runtime::spawn_blocking(move || {
        let _request = request;
        plan_convert_impl(&state, &owner_window, &settings, &spec, &worker_token)
    })
    .await
    .map_err(|error| ErrorDto::other(format!("convert plan task failed: {error}")))?
}

/// Cancels the matching conversion plan owned by this WebView.
#[tauri::command]
pub fn cancel_convert_plan(
    window: WebviewWindow,
    requests: State<'_, Arc<PreflightRequests>>,
    request_id: String,
) {
    requests.cancel(
        PreflightRequestKind::ConvertPlan,
        window.label(),
        &request_id,
    );
}

#[allow(clippy::too_many_arguments)] // Mirrors the stable extraction-plan IPC contract.
fn plan_extract_impl(
    state: &AppState,
    owner_window: &str,
    source: &str,
    _display_path: &str,
    destination: &Path,
    selection: Option<&[String]>,
    smart: bool,
    encoding: Option<&str>,
    max_entries: u64,
    control: &ControlToken,
) -> Result<ExtractPlanPreflightDto, ErrorDto> {
    control.checkpoint().map_err(ErrorDto::from)?;
    let resolved = state
        .resolve_archive_source(source, Some(owner_window))
        .map_err(preview_error_dto)?;
    control.checkpoint().map_err(ErrorDto::from)?;
    let open = OpenOptions {
        password: state.password_for(resolved.path()),
        encoding_override: encoding.map(str::to_owned),
    };
    let logical_display_path = resolved.display_path();
    let plan = (|| -> Result<_, FormatError> {
        let (plan, space, input_guard) = state
            .engine
            .plan_extract_with_input_guard_and_entry_limit_controlled(
                resolved.path(),
                destination,
                Path::new(logical_display_path),
                smart,
                &open,
                max_entries,
                control,
                |entries, control| {
                    selection
                        .map(|paths| expand_selection_with_control(entries, paths, control))
                        .transpose()
                },
            )?;
        control.checkpoint()?;
        Ok(ExtractPlanPreflightDto::new(plan, space, input_guard))
    })();
    plan.map_err(|error| {
        preview_error_dto_with_paths(error, &[(resolved.path(), resolved.display_path())])
    })
}

/// Plans the exact extraction scope, final smart destination and currently
/// observable conflicts without exposing a nested archive's private source.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Mirrors the frontend invocation fields.
pub async fn plan_extract(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    requests: State<'_, Arc<PreflightRequests>>,
    settings: State<'_, Arc<SettingsStore>>,
    path: String,
    display_path: String,
    dest: String,
    selection: Option<Vec<String>>,
    smart: bool,
    encoding: Option<String>,
    request_id: Option<String>,
) -> Result<ExtractPlanPreflightDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let requests = Arc::clone(requests.inner());
    let max_entries = settings.get().safety_limits().max_entries;
    let owner_window = window.label().to_owned();
    let request = if let Some(request_id) = request_id.as_deref() {
        requests.begin_request(PreflightRequestKind::ExtractPlan, &owner_window, request_id)
    } else {
        requests.begin_anonymous_request(PreflightRequestKind::ExtractPlan, &owner_window)
    };
    let token = request.control();
    let worker_token = Arc::clone(&token);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _request = request;
        plan_extract_impl(
            &state,
            &owner_window,
            &path,
            &display_path,
            Path::new(&dest),
            selection.as_deref(),
            smart,
            encoding.as_deref(),
            max_entries,
            &worker_token,
        )
    })
    .await;
    task.map_err(|error| ErrorDto::other(format!("extract plan task failed: {error}")))?
}

/// Cancels the matching extraction plan owned by this WebView, or records the
/// one-shot id when its begin call is still in transit. Late cancellation of
/// a recently completed request is ignored.
#[tauri::command]
pub fn cancel_extract_plan(
    window: WebviewWindow,
    requests: State<'_, Arc<PreflightRequests>>,
    request_id: String,
) {
    requests.cancel(
        PreflightRequestKind::ExtractPlan,
        window.label(),
        &request_id,
    );
}

fn disk_space_preflight(path: &Path, required_bytes: u64) -> Result<DiskSpaceDto, FormatError> {
    let dir = if path.is_dir() {
        path
    } else {
        parent_or_current(path)
    };
    let available_bytes = fs4::available_space(dir)?;
    Ok(DiskSpaceDto {
        path: dir.to_string_lossy().into_owned(),
        required_bytes,
        available_bytes,
        ok: available_bytes >= required_bytes,
    })
}

/// Checks destination-volume free space before queuing a create/update job.
#[tauri::command]
pub fn check_disk_space(path: String, required_bytes: u64) -> Result<DiskSpaceDto, ErrorDto> {
    disk_space_preflight(Path::new(&path), required_bytes).map_err(ErrorDto::from)
}

/// Returns the system temporary directory used by backend archive rewrites.
#[tauri::command]
pub fn temp_dir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

fn validate_operation_history_export(contents: &str) -> Result<(), FormatError> {
    if contents.trim().is_empty() {
        return Err(FormatError::Unsupported(
            "operation history export is empty".into(),
        ));
    }
    if contents.len() > HISTORY_EXPORT_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(
            "operation history export exceeds 1 MiB".into(),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(contents).map_err(|e| {
        FormatError::Unsupported(format!("operation history export is not valid JSON: {e}"))
    })?;
    let records = value
        .get("records")
        .and_then(|records| records.as_array())
        .ok_or_else(|| {
            FormatError::Unsupported("operation history export missing records".into())
        })?;
    for record in records {
        for field in ["id", "status", "title", "detail"] {
            if !record.get(field).is_some_and(|value| value.is_string()) {
                return Err(FormatError::Unsupported(format!(
                    "operation history record missing string field {field}"
                )));
            }
        }
        if !record.get("time").is_some_and(|value| value.is_number()) {
            return Err(FormatError::Unsupported(
                "operation history record missing numeric field time".into(),
            ));
        }
    }
    Ok(())
}

fn export_operation_history_impl(path: &Path, contents: &str) -> Result<(), FormatError> {
    validate_operation_history_export(contents)?;
    if path.is_dir() {
        return Err(FormatError::Unsupported(format!(
            "operation history export target is a directory: {}",
            path.display()
        )));
    }
    let parent = parent_or_current(path);
    fs::create_dir_all(parent)?;
    let file_name = operation_history_file_name(path);
    let tmp = parent.join(format!(".{file_name}.part-{}", std::process::id()));
    let write_result = (|| -> Result<(), FormatError> {
        let mut file = File::create(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(path)?;
                fs::rename(&tmp, path)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

/// Writes the sanitized local operation-history audit JSON selected by the
/// frontend to a user-chosen file.
#[tauri::command]
pub fn export_operation_history(path: String, contents: String) -> Result<(), ErrorDto> {
    export_operation_history_impl(Path::new(&path), &contents).map_err(ErrorDto::from)
}

/// Returns the backend-generated desktop operation audit, newest first.
#[tauri::command]
pub fn get_operation_audit(
    audit: State<'_, Arc<OperationAudit>>,
    limit: Option<usize>,
) -> Vec<OperationAuditRecord> {
    audit.recent(operation_audit_limit(limit))
}

fn operation_audit_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_OPERATION_AUDIT_LIMIT)
}

/// Exports the backend-generated operation audit to a user-selected JSON file.
#[tauri::command]
pub fn export_operation_audit(
    audit: State<'_, Arc<OperationAudit>>,
    path: String,
) -> Result<(), ErrorDto> {
    audit.export_json(Path::new(&path)).map_err(ErrorDto::from)
}

/// Installs the visible platform integration actions.
#[tauri::command]
pub fn apply_integration_changes(
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<IntegrationApplyResultDto, ErrorDto> {
    let language = resolved_settings_language(&settings);
    integration::apply_visible_integrations_for_language(Some(&language))
        .map_err(|e| ErrorDto::other(format!("cannot apply desktop integrations: {e}")))
}

/// Reads platform integration status without changing the system.
#[tauri::command]
pub fn get_integration_status(
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<IntegrationStatusDto, ErrorDto> {
    let language = resolved_settings_language(&settings);
    integration::integration_status_for_language(Some(&language))
        .map_err(|e| ErrorDto::other(format!("cannot read desktop integration status: {e}")))
}

/// Reads system-owned default handlers and file-manager visibility evidence.
#[tauri::command]
pub fn get_system_integration_diagnostics() -> IntegrationSystemDiagnosticsDto {
    integration::system_integration_diagnostics()
}

/// Removes the visible platform integration actions.
#[tauri::command]
pub fn remove_integration_changes(
    settings: State<'_, Arc<SettingsStore>>,
) -> Result<IntegrationRemoveResultDto, ErrorDto> {
    let language = resolved_settings_language(&settings);
    integration::remove_visible_integrations_for_language(Some(&language))
        .map_err(|e| ErrorDto::other(format!("cannot remove desktop integrations: {e}")))
}

fn format_label_from_name(state: &AppState, name: &str) -> String {
    match state.engine.registry().detect_by_name(name) {
        Some(detected) => detected_format_label(detected),
        None => "archive".to_owned(),
    }
}

fn detected_format_label(detected: Detected) -> String {
    match detected {
        Detected::Archive(format) => format.id().to_owned(),
        Detected::Compressed {
            compressor,
            inner_archive,
        } => match inner_archive {
            Some(inner) => format!("{}.{}", inner.id(), compressor.id()),
            None => compressor.id().to_owned(),
        },
    }
}

fn entry_base_name(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let base = match trimmed.rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ => path,
    };
    base.to_owned()
}

fn nested_archive_display_path(outer_display_path: &str, entry_path: &str) -> String {
    let outer_path = Path::new(outer_display_path);
    let parent = parent_or_current(outer_path);
    let outer_name = outer_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("archive");
    let nested_name = entry_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(safe_virtual_archive_component)
        .collect::<Vec<_>>()
        .join(" › ");
    let nested_name = if nested_name.is_empty() {
        "_".to_owned()
    } else {
        nested_name
    };
    parent
        .join(format!("{outer_name} › {nested_name}"))
        .to_string_lossy()
        .into_owned()
}

fn safe_virtual_archive_component(component: &str) -> String {
    let mut sanitized: String = component
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    while sanitized.ends_with([' ', '.']) {
        sanitized.pop();
    }
    if sanitized.is_empty() {
        "_".to_owned()
    } else {
        sanitized
    }
}

pub(crate) fn preview_error_dto(error: FormatError) -> ErrorDto {
    match preview_failure_kind(&error) {
        Some(kind) => ErrorDto {
            key: kind.key().to_owned(),
            params: HashMap::new(),
            detail: error.to_string(),
        },
        None => ErrorDto::from(error),
    }
}

fn preview_error_dto_with_paths(error: FormatError, replacements: &[(&Path, &str)]) -> ErrorDto {
    let mut dto = preview_error_dto(error);
    for (physical, display) in replacements {
        let physical = physical.to_string_lossy();
        if physical.is_empty() || physical == *display {
            continue;
        }
        dto.detail = dto.detail.replace(physical.as_ref(), display);
        for value in dto.params.values_mut() {
            *value = value.replace(physical.as_ref(), display);
        }
    }
    dto
}

fn preview_nested_archive_impl(
    state: &AppState,
    sessions: &PreviewSessionManager,
    owner: &str,
    outer_source: &str,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<NestedArchivePreviewDto, ErrorDto> {
    let outer = state
        .resolve_archive_source(outer_source, Some(owner))
        .map_err(preview_error_dto)?;
    let reservation = sessions.reserve(owner).map_err(preview_error_dto)?;
    let workspace = reservation.workspace_path().map_err(preview_error_dto)?;
    let (temp, _) = extract_nested_archive_to_temp_limited(
        state,
        outer.path(),
        entry_path,
        password,
        encoding,
        workspace,
        MAX_PREVIEW_ENTRY_BYTES,
    )
    .map_err(|error| {
        preview_error_dto_with_paths(error, &[(outer.path(), outer.display_path())])
    })?;
    let temp_path = temp.to_path_buf();
    let nested_display_path = nested_archive_display_path(outer.display_path(), entry_path);
    let entries = state
        .engine
        .list(temp.as_ref(), &OpenOptions::default())
        .map_err(|error| {
            preview_error_dto_with_paths(
                error,
                &[
                    (outer.path(), outer.display_path()),
                    (&temp_path, nested_display_path.as_str()),
                ],
            )
        })?;
    let entry_count = entries.len();
    let items = entries
        .iter()
        .take(NESTED_PREVIEW_LIMIT)
        .map(|meta| {
            let normalized = normalized_entry_path(meta);
            EntryDto::from_meta(meta, normalized.clone(), entry_base_name(&normalized))
        })
        .collect();
    Ok(NestedArchivePreviewDto {
        outer_path: outer_source.to_owned(),
        entry_path: entry_path.to_owned(),
        format: format_label_from_name(state, entry_path),
        entry_count,
        truncated: entry_count > NESTED_PREVIEW_LIMIT,
        items,
    })
}

fn entry_is_archive_like(state: &AppState, entry_path: &str) -> bool {
    matches!(
        state.engine.registry().detect_by_name(entry_path),
        Some(Detected::Archive(_))
            | Some(Detected::Compressed {
                inner_archive: Some(_),
                ..
            })
    )
}

fn preview_trace_extension(path: &Path) -> Option<String> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => Some(ext.to_ascii_lowercase()),
        _ => None,
    }
}

fn preview_trace_payload(
    action: &str,
    status: &str,
    path: &Path,
    error: Option<&str>,
) -> serde_json::Value {
    json!({
        "action": action,
        "status": status,
        "platform": platform_kind(),
        "extension": preview_trace_extension(path),
        "error": error,
    })
}

fn trace_preview_opener(action: &str, status: &str, path: &Path, error: Option<&str>) {
    validation_trace::trace(
        &format!("preview.{action}.{status}"),
        preview_trace_payload(action, status, path, error),
    );
}

#[tauri::command]
pub fn open_preview_session(
    app: AppHandle,
    window: WebviewWindow,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    preview_id: String,
) -> Result<(), ErrorDto> {
    let path = sessions
        .path_for_external_use(&preview_id, window.label())
        .map_err(preview_error_dto)?;
    trace_preview_opener("open", "request", &path, None);
    match app
        .opener()
        .open_path(path.to_string_lossy().into_owned(), None::<String>)
    {
        Ok(()) => {
            sessions.external_use_succeeded(&preview_id, window.label());
            trace_preview_opener("open", "ok", &path, None);
            Ok(())
        }
        Err(_) => {
            sessions.external_use_failed(&preview_id, window.label());
            trace_preview_opener("open", "err", &path, Some("opener_failed"));
            Err(ErrorDto::other("open preview failed"))
        }
    }
}

#[tauri::command]
pub fn reveal_preview_session(
    app: AppHandle,
    window: WebviewWindow,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    preview_id: String,
) -> Result<(), ErrorDto> {
    let path = sessions
        .path_for_external_use(&preview_id, window.label())
        .map_err(preview_error_dto)?;
    trace_preview_opener("reveal", "request", &path, None);
    match app.opener().reveal_item_in_dir(&path) {
        Ok(()) => {
            sessions.external_use_succeeded(&preview_id, window.label());
            trace_preview_opener("reveal", "ok", &path, None);
            Ok(())
        }
        Err(_) => {
            sessions.external_use_failed(&preview_id, window.label());
            trace_preview_opener("reveal", "err", &path, Some("opener_failed"));
            Err(ErrorDto::other("reveal preview failed"))
        }
    }
}

#[tauri::command]
pub fn release_preview_session(
    window: WebviewWindow,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    preview_id: String,
) -> Result<bool, ErrorDto> {
    sessions
        .release(&preview_id, window.label())
        .map_err(preview_error_dto)
}

fn preview_archive_entry_impl(
    state: &AppState,
    sessions: &PreviewSessionManager,
    owner: &str,
    outer_source: &str,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<EntryPreviewDto, ErrorDto> {
    let outer = state
        .resolve_archive_source(outer_source, Some(owner))
        .map_err(preview_error_dto)?;
    let prepared = sessions
        .prepare_archive_entry(owner, state, outer.path(), entry_path, password, encoding)
        .map_err(|error| {
            preview_error_dto_with_paths(error, &[(outer.path(), outer.display_path())])
        })?;
    Ok(EntryPreviewDto {
        outer_path: outer_source.to_owned(),
        entry_path: entry_path.to_owned(),
        display_name: prepared.display_name,
        preview_id: prepared.id,
        size: prepared.size,
        archive_like: entry_is_archive_like(state, entry_path),
    })
}

fn operation_history_file_name(path: &Path) -> &str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => "operation-history.json",
    }
}

/// Reads an archive entry as another archive and returns its first rows.
#[tauri::command]
pub async fn preview_nested_archive(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<NestedArchivePreviewDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let sessions = Arc::clone(sessions.inner());
    let owner = window.label().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        preview_nested_archive_impl(
            &state,
            &sessions,
            &owner,
            &outer_path,
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
        )
    })
    .await
    .map_err(|e| ErrorDto::other(format!("nested preview task failed: {e}")))?
}

/// Extracts one archive entry to a temporary file for local preview/reveal.
#[tauri::command]
pub async fn preview_archive_entry(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<EntryPreviewDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let sessions = Arc::clone(sessions.inner());
    let owner = window.label().to_owned();
    let trace_extension = preview_trace_extension(Path::new(&entry_path));
    tauri::async_runtime::spawn_blocking(move || {
        let result = preview_archive_entry_impl(
            &state,
            &sessions,
            &owner,
            &outer_path,
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
        );
        match &result {
            Ok(preview) => validation_trace::trace(
                "preview_archive_entry.ok",
                json!({
                    "extension": trace_extension,
                    "size": preview.size,
                    "archive_like": preview.archive_like,
                    "system_open": true,
                }),
            ),
            Err(e) => validation_trace::trace(
                "preview_archive_entry.err",
                json!({
                    "error": e.key,
                }),
            ),
        }
        result
    })
    .await
    .map_err(|e| ErrorDto::other(format!("entry preview task failed: {e}")))?
}

#[allow(clippy::too_many_arguments)] // Each argument has a distinct nested-open role.
fn open_nested_archive_impl(
    state: &AppState,
    sessions: &PreviewSessionManager,
    owner: &str,
    outer_source: &str,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    max_entries: u64,
) -> Result<ArchiveInfo, ErrorDto> {
    let outer = state
        .resolve_archive_source(outer_source, Some(owner))
        .map_err(preview_error_dto)?;
    let reservation = sessions.reserve(owner).map_err(preview_error_dto)?;
    let workspace = reservation.workspace_path().map_err(preview_error_dto)?;
    let (temp, size) = extract_nested_archive_to_temp_limited(
        state,
        outer.path(),
        entry_path,
        password,
        encoding,
        workspace,
        MAX_PREVIEW_ENTRY_BYTES,
    )
    .map_err(|error| {
        preview_error_dto_with_paths(error, &[(outer.path(), outer.display_path())])
    })?;
    let display_name = entry_base_name(entry_path);
    let display_path = nested_archive_display_path(outer.display_path(), entry_path);
    let temp_path = temp.to_path_buf();
    state
        .open_archive_with_owned_temp_and_entry_limit(
            owner,
            temp,
            reservation,
            size,
            display_path.clone(),
            display_name,
            max_entries,
        )
        .map_err(|error| {
            preview_error_dto_with_paths(
                error,
                &[
                    (outer.path(), outer.display_path()),
                    (&temp_path, display_path.as_str()),
                ],
            )
        })
}

/// Extracts an archive entry to a persistent temp file and opens it as the
/// active browse archive.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Mirrors the frontend invocation fields.
pub async fn open_nested_archive(
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    sessions: State<'_, Arc<PreviewSessionManager>>,
    settings: State<'_, Arc<SettingsStore>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<ArchiveInfo, ErrorDto> {
    let state = Arc::clone(state.inner());
    let sessions = Arc::clone(sessions.inner());
    let max_entries = settings.get().safety_limits().max_entries;
    let owner = window.label().to_owned();
    let trace_extension = preview_trace_extension(Path::new(&entry_path));
    tauri::async_runtime::spawn_blocking(move || {
        let result = open_nested_archive_impl(
            &state,
            &sessions,
            &owner,
            &outer_path,
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
            max_entries,
        );
        match &result {
            Ok(info) => validation_trace::trace(
                "open_nested_archive.ok",
                json!({
                    "extension": trace_extension,
                    "format": info.format,
                    "entry_count": info.entry_count,
                }),
            ),
            Err(e) => validation_trace::trace(
                "open_nested_archive.err",
                json!({
                    "extension": trace_extension,
                    "error": e.key,
                }),
            ),
        }
        result
    })
    .await
    .map_err(|e| ErrorDto::other(format!("nested open task failed: {e}")))?
}

/// Reports whether this installation contains its host SFX template.
#[tauri::command]
pub async fn get_sfx_create_capability(
    jobs: State<'_, Arc<JobManager>>,
) -> Result<SfxCreateCapabilityDto, ErrorDto> {
    let jobs = Arc::clone(jobs.inner());
    tauri::async_runtime::spawn_blocking(move || jobs.sfx_capability())
        .await
        .map_err(|error| ErrorDto::other(format!("SFX capability check failed: {error}")))
}

/// Reports Developer ID Application identities available to the macOS publisher.
#[tauri::command]
pub async fn get_macos_sfx_publisher_status(
) -> Result<crate::dto::MacosSfxPublisherStatusDto, ErrorDto> {
    tauri::async_runtime::spawn_blocking(|| {
        #[cfg(target_os = "macos")]
        {
            let identities = squallz_publish::macos_signing_identities().map_err(ErrorDto::from)?;
            Ok(crate::dto::MacosSfxPublisherStatusDto {
                available: !identities.is_empty(),
                status: if identities.is_empty() {
                    "missing_identity".into()
                } else {
                    "available".into()
                },
                identities,
            })
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(crate::dto::MacosSfxPublisherStatusDto {
                available: false,
                status: "unsupported".into(),
                identities: Vec::new(),
            })
        }
    })
    .await
    .map_err(|error| ErrorDto::other(format!("SFX publisher check failed: {error}")))?
}

/// Submits a job to the queue; progress/state arrive as `job://*` events.
#[tauri::command]
pub fn submit_job(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Arc<AppState>>,
    jobs: State<'_, Arc<JobManager>>,
    settings: State<'_, Arc<SettingsStore>>,
    spec: JobSpec,
) -> Result<u64, ErrorDto> {
    let owner_window = window.label().to_owned();
    jobs.submit_for_window(
        owner_window,
        Arc::clone(state.inner()),
        Arc::new(TauriEvents::new(app, &window)),
        spec,
        settings.get(),
    )
    .map_err(ErrorDto::from)
}

#[tauri::command]
pub fn job_snapshot(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Option<JobStateSnapshot> {
    jobs.snapshot_for_window(window.label(), id).ok()
}

#[tauri::command]
pub fn job_snapshots(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    since: Option<u64>,
) -> JobSnapshotDelta {
    jobs.snapshots_for_window(window.label(), since)
}

#[tauri::command]
pub fn dismiss_job_snapshots(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    ids: Vec<u64>,
) -> Result<(), ErrorDto> {
    jobs.dismiss_snapshots_for_window(window.label(), &ids)
}

#[tauri::command]
pub fn get_source_cleanup_recovery(
    jobs: State<'_, Arc<JobManager>>,
) -> Option<SourceCleanupRecoveryNotice> {
    jobs.source_cleanup_recovery()
}

/// Pauses a job at its next chunk boundary.
#[tauri::command]
pub fn pause_job(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Result<(), ErrorDto> {
    jobs.pause_for_window(window.label(), id)
}

/// Resumes a paused job.
#[tauri::command]
pub fn resume_job(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Result<(), ErrorDto> {
    jobs.resume_for_window(window.label(), id)
}

/// Moves a queued job one position earlier in the shared queue.
#[tauri::command]
pub fn move_job_earlier(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Result<(), ErrorDto> {
    jobs.move_earlier_for_window(window.label(), id)
}

/// Moves a queued job one position later in the shared queue.
#[tauri::command]
pub fn move_job_later(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Result<(), ErrorDto> {
    jobs.move_later_for_window(window.label(), id)
}

/// Places a queued job before another waiting job, or at the end of the
/// shared queue when `before_id` is absent.
#[tauri::command]
pub fn move_job_before(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
    before_id: Option<u64>,
) -> Result<(), ErrorDto> {
    jobs.move_before_for_window(window.label(), id, before_id)
}

/// Cancels a queued or running job.
#[tauri::command]
pub fn cancel_job(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
) -> Result<(), ErrorDto> {
    jobs.cancel_for_window(window.label(), id)
}

/// Answers a `job://ask-conflict` prompt.
#[tauri::command]
pub fn answer_conflict(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
    decision: String,
    apply_all: bool,
) -> Result<(), ErrorDto> {
    jobs.answer_conflict_for_window(window.label(), id, decision, apply_all)
}

/// Answers a `job://ask-password` prompt (`None` = the user cancelled).
#[tauri::command]
pub fn answer_password(
    window: WebviewWindow,
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
    password: Option<String>,
) -> Result<(), ErrorDto> {
    jobs.answer_password_for_window(window.label(), id, password)
}

/// Persistent password-book status for one archive path.
#[tauri::command]
pub async fn archive_password_status(
    secrets: State<'_, SharedSecretStore>,
    path: String,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    let secrets = Arc::clone(secrets.inner());
    tauri::async_runtime::spawn_blocking(move || {
        archive_password_status_impl(secrets.as_ref(), Path::new(&path))
    })
    .await
    .map_err(|error| ErrorDto::other(format!("password-book status task failed: {error}")))?
}

/// Verifies and saves the current archive password in the platform store.
#[tauri::command]
pub async fn remember_archive_password(
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    path: String,
    password: String,
    encoding: Option<String>,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let secrets = Arc::clone(secrets.inner());
    tauri::async_runtime::spawn_blocking(move || {
        remember_archive_password_impl(
            state.as_ref(),
            secrets.as_ref(),
            Path::new(&path),
            &password,
            encoding.as_deref(),
        )
    })
    .await
    .map_err(|error| ErrorDto::other(format!("password-book save task failed: {error}")))?
}

/// Forgets the current archive password from both Keychain and session cache.
#[tauri::command]
pub async fn forget_archive_password(
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    path: String,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let secrets = Arc::clone(secrets.inner());
    tauri::async_runtime::spawn_blocking(move || {
        forget_archive_password_impl(state.as_ref(), secrets.as_ref(), Path::new(&path))
    })
    .await
    .map_err(|error| ErrorDto::other(format!("password-book forget task failed: {error}")))?
}

/// Returns file paths that were opened by the OS before the frontend drained
/// launch paths. Realtime delivery starts only after `open_file_listener_ready`
/// so the cold open-file screen can avoid loading the JS event listener first.
#[tauri::command]
pub fn take_open_files(
    app: AppHandle,
    open_files: State<'_, Arc<OpenFileRequests>>,
) -> OpenFilesEvent {
    let event = open_files.drain_pending();
    if !event.paths.is_empty() && !event.is_external_task() {
        focus_main_window(&app);
    }
    event
}

/// Marks the frontend's `app://open-files` listener ready and returns paths
/// queued while the listener module was loading.
#[tauri::command]
pub fn open_file_listener_ready(
    app: AppHandle,
    open_files: State<'_, Arc<OpenFileRequests>>,
) -> OpenFilesEvent {
    let event = open_files.mark_listener_ready();
    if !event.paths.is_empty() && !event.is_external_task() {
        focus_main_window(&app);
    }
    event
}

/// Resolves the effective language: explicit request → persisted setting →
/// system locale → en-US.
fn localizer(settings: &SettingsStore, explicit: Option<&str>) -> Localizer {
    let persisted = settings.get().language;
    Localizer::load(explicit.or(persisted.as_deref()))
}

fn resolved_settings_language(settings: &SettingsStore) -> String {
    localizer(settings, None).language().to_owned()
}

/// Full locale table for the frontend i18n store.
#[tauri::command]
pub fn get_locale_table(
    settings: State<'_, Arc<SettingsStore>>,
    lang: Option<String>,
) -> LocaleTable {
    let loc = localizer(&settings, lang.as_deref());
    LocaleTable {
        lang: loc.language().to_owned(),
        table: loc.table(),
    }
}

/// Available languages with their self-described names (`meta.name`).
#[tauri::command]
pub fn list_languages(settings: State<'_, Arc<SettingsStore>>) -> Vec<LanguageDto> {
    localizer(&settings, None)
        .language_names()
        .into_iter()
        .map(|(tag, name)| LanguageDto { tag, name })
        .collect()
}

/// Current persisted settings.
#[tauri::command]
pub fn get_settings(settings: State<'_, Arc<SettingsStore>>) -> SettingsDto {
    settings.get()
}

fn preset_error_dto(error: PresetError) -> ErrorDto {
    if matches!(error, PresetError::Validation(_)) {
        return ErrorDto::invalid_preset(error.to_string());
    }
    let conflict = matches!(error, PresetError::RevisionConflict { .. });
    ErrorDto::presets(error.to_string(), conflict)
}

/// Loads the shared, versioned preset document. Corrupt or newer documents
/// return an error and remain untouched on disk.
#[tauri::command]
pub fn get_archive_presets(
    presets: State<'_, Arc<PresetStore>>,
) -> Result<PresetDocument, ErrorDto> {
    presets.load().map_err(preset_error_dto)
}

/// Replaces the preset document only when the caller still has the latest
/// revision. Validation and persistence happen before the new snapshot is
/// returned to the frontend.
#[tauri::command]
pub fn save_archive_presets(
    presets: State<'_, Arc<PresetStore>>,
    expected_revision: u64,
    document: PresetDocument,
) -> Result<PresetDocument, ErrorDto> {
    presets
        .compare_and_swap(expected_revision, document)
        .map_err(preset_error_dto)
}

/// Resolves a Finder/file-manager action against the same preset snapshot as
/// the main window. The returned `JobSpec` is complete and will not change if
/// the preset is edited after submission.
#[tauri::command]
pub fn resolve_external_task_job(
    state: State<'_, Arc<AppState>>,
    presets: State<'_, Arc<PresetStore>>,
    action: ExternalTaskActionDto,
    paths: Vec<String>,
    output: Option<String>,
    checksum_algorithm: ChecksumAlgorithm,
    checksum_excludes: Vec<String>,
) -> Result<Option<JobSpec>, ErrorDto> {
    let document = presets.load().map_err(preset_error_dto)?;
    resolve_external_task_job_impl(
        &state,
        &document,
        action,
        paths,
        output,
        checksum_algorithm,
        checksum_excludes,
    )
}

fn resolve_external_task_job_impl(
    state: &AppState,
    document: &PresetDocument,
    action: ExternalTaskActionDto,
    paths: Vec<String>,
    output: Option<String>,
    checksum_algorithm: ChecksumAlgorithm,
    checksum_excludes: Vec<String>,
) -> Result<Option<JobSpec>, ErrorDto> {
    let paths: Vec<String> = paths.into_iter().filter(|path| !path.is_empty()).collect();
    let Some(first) = paths.first() else {
        return Ok(None);
    };
    match action {
        ExternalTaskActionDto::Checksum => Ok(Some(JobSpec::Checksum {
            inputs: paths,
            excludes: checksum_excludes,
            algorithm: checksum_algorithm,
        })),
        ExternalTaskActionDto::ExtractHere | ExternalTaskActionDto::ExtractToFolder => {
            resolve_external_extract_job(state, document, action, paths).map(Some)
        }
        ExternalTaskActionDto::ExtractSfx => Ok(Some(JobSpec::Extract {
            path: first.clone(),
            dest: output
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| external_archive_folder(state, first)),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Ask,
            symlinks: squallz_core::api::SymlinkPolicy::Skip,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: true,
            best_effort: false,
        })),
        ExternalTaskActionDto::CompressTo7z => {
            let (
                level,
                encrypt_names,
                split_size,
                content_policy,
                excludes,
                completion,
                post_success,
                test_after_create,
            ) = if let Some(options) = bound_create_preset(document)? {
                if options.format.as_str() != "7z"
                    || options.credential != CreateCredential::None
                    || options.output != CreateOutput::Archive
                {
                    return Err(ErrorDto::presets(
                        "file-manager create preset is not compatible with Compress to 7Z",
                        false,
                    ));
                }
                let split_size = match options.volumes {
                    VolumeMode::Single => None,
                    VolumeMode::Split { size_bytes } => Some(size_bytes.get()),
                };
                (
                    options.level.get(),
                    options.encrypt_names,
                    split_size,
                    options.content_policy,
                    options.excludes.clone(),
                    options.completion,
                    options.post_success,
                    options.test_after_create,
                )
            } else {
                (
                    5,
                    false,
                    None,
                    CreateContentPolicy::KeepAllFiles,
                    Vec::new(),
                    CreateCompletionAction::None,
                    PostSuccessAction::KeepSource,
                    false,
                )
            };
            Ok(Some(JobSpec::Compress {
                inputs: paths.clone(),
                dest: external_compress_output(state, &paths, output.as_deref()),
                level,
                password: None,
                encrypt_names,
                split_size,
                split_mode: squallz_core::api::SplitOutputMode::Generic,
                excludes,
                content_policy,
                sqz_inner_format: None,
                sfx_target: None,
                completion,
                post_success,
                test_after_create,
                replace_existing: false,
                replacement_guard: None,
            }))
        }
        ExternalTaskActionDto::TestArchive => Ok(Some(JobSpec::Test {
            path: first.clone(),
            encoding: None,
            password: None,
        })),
    }
}

fn bound_create_preset(
    document: &PresetDocument,
) -> Result<Option<&squallz_core::CreatePreset>, ErrorDto> {
    let Some(id) = document.bindings.file_manager_create.as_ref() else {
        return Ok(None);
    };
    let preset = document
        .preset(id)
        .ok_or_else(|| ErrorDto::presets("file-manager create preset is missing", false))?;
    preset.create_options().map(Some).ok_or_else(|| {
        ErrorDto::presets("file-manager create binding is not a create preset", false)
    })
}

fn bound_extract_preset(
    document: &PresetDocument,
) -> Result<Option<&squallz_core::ExtractPreset>, ErrorDto> {
    let Some(id) = document.bindings.file_manager_extract.as_ref() else {
        return Ok(None);
    };
    let preset = document
        .preset(id)
        .ok_or_else(|| ErrorDto::presets("file-manager extract preset is missing", false))?;
    preset.extract_options().map(Some).ok_or_else(|| {
        ErrorDto::presets(
            "file-manager extract binding is not an extract preset",
            false,
        )
    })
}

fn resolve_external_extract_job(
    state: &AppState,
    document: &PresetDocument,
    action: ExternalTaskActionDto,
    paths: Vec<String>,
) -> Result<JobSpec, ErrorDto> {
    let (overwrite, symlinks, encoding) = if let Some(options) = bound_extract_preset(document)? {
        let encoding = match &options.encoding {
            EntryNameEncoding::Auto => None,
            EntryNameEncoding::Named { label } => Some(label.clone()),
        };
        (options.existing_output, options.symlinks, encoding)
    } else {
        (OverwritePolicy::Ask, SymlinkPolicy::Preserve, None)
    };
    let smart = action == ExternalTaskActionDto::ExtractHere;
    if paths.len() == 1 {
        let path = paths.into_iter().next().ok_or_else(|| {
            ErrorDto::other("external extract path disappeared during resolution")
        })?;
        return Ok(JobSpec::Extract {
            dest: external_extract_destination(state, action, &path),
            path,
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite,
            symlinks,
            smart,
            encoding,
            password: None,
            verify_sfx: false,
            best_effort: false,
        });
    }
    Ok(JobSpec::BatchExtract {
        items: paths
            .into_iter()
            .map(|path| BatchExtractItem {
                dest: external_extract_destination(state, action, &path),
                path,
                encoding: encoding.clone(),
                password: None,
                best_effort: false,
            })
            .collect(),
        overwrite,
        symlinks,
        smart,
    })
}

fn external_extract_destination(
    state: &AppState,
    action: ExternalTaskActionDto,
    path: &str,
) -> String {
    let source = Path::new(path);
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    if action == ExternalTaskActionDto::ExtractHere {
        return parent.to_string_lossy().into_owned();
    }
    parent
        .join(state.engine.archive_stem(source))
        .to_string_lossy()
        .into_owned()
}

fn external_archive_folder(state: &AppState, path: &str) -> String {
    external_extract_destination(state, ExternalTaskActionDto::ExtractToFolder, path)
}

fn external_compress_output(state: &AppState, paths: &[String], requested: Option<&str>) -> String {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return requested.to_owned();
    }
    let first = paths
        .first()
        .map_or_else(|| Path::new("Archive"), Path::new);
    let parent = first.parent().unwrap_or_else(|| Path::new(""));
    let stem = if paths.len() == 1 {
        state.engine.archive_stem(first)
    } else {
        "Archive".to_owned()
    };
    parent
        .join(format!("{stem}.7z"))
        .to_string_lossy()
        .into_owned()
}

fn create_destination_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn validate_create_destination_parent(proposed: &Path) -> io::Result<()> {
    if proposed.file_name().is_none_or(|name| name.is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "create destination must include a file name",
        ));
    }
    let parent = create_destination_parent(proposed);
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "create destination parent is not a directory",
        ));
    }
    for _ in 0..CREATE_DESTINATION_PROBE_ATTEMPTS {
        let counter = CREATE_DESTINATION_PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let probe = parent.join(format!(
            ".squallz-write-probe-{}-{counter}.tmp",
            std::process::id()
        ));
        match File::options().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                return fs::remove_file(probe);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a create destination probe",
    ))
}

fn unique_create_destination_path(proposed: &Path, split: bool) -> io::Result<PathBuf> {
    validate_create_destination_parent(proposed)?;
    let kind = if split {
        CreateArtifactKind::SplitArchive
    } else {
        CreateArtifactKind::Archive
    };
    core_find_available_create_destination(proposed, kind).map_err(format_error_as_io)
}

fn format_error_as_io(error: FormatError) -> io::Error {
    match error {
        FormatError::Io(error) => error,
        other => io::Error::other(other),
    }
}

/// Returns a non-conflicting create destination without overwriting an
/// existing archive or any member of its split-volume family.
#[tauri::command]
pub async fn unique_create_destination(proposed: String, split: bool) -> Result<String, ErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        unique_create_destination_path(Path::new(&proposed), split)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| {
                ErrorDto::other(format!(
                    "create destination parent is unavailable ({:?})",
                    error.kind()
                ))
            })
    })
    .await
    .map_err(|error| ErrorDto::other(format!("destination naming task failed: {error}")))?
}

/// Reports whether a selected create destination or, for split output, any
/// managed family member already exists. The frontend asks for explicit
/// replacement confirmation before it authorizes an overwrite job.
#[tauri::command]
pub fn create_destination_has_conflict(proposed: String, split: bool) -> Result<bool, ErrorDto> {
    let kind = if split {
        CreateArtifactKind::SplitArchive
    } else {
        CreateArtifactKind::Archive
    };
    core_create_destination_has_conflict(Path::new(&proposed), kind).map_err(ErrorDto::from)
}

/// Captures the exact core-managed destination state immediately before the
/// native replacement confirmation. The returned guard is opaque to the
/// frontend and is revalidated by the final core commit transaction.
#[tauri::command]
pub async fn inspect_create_destination(
    window: WebviewWindow,
    requests: State<'_, Arc<PreflightRequests>>,
    proposed: String,
    split: bool,
    sfx_target: Option<SfxTarget>,
    request_id: String,
) -> Result<CreateDestinationInspectionDto, ErrorDto> {
    let owner = window.label().to_owned();
    let events: Arc<dyn EventSink> =
        Arc::new(TauriEvents::new(window.app_handle().clone(), &window));
    let requests = Arc::clone(requests.inner());
    let request =
        requests.begin_request(PreflightRequestKind::CreateDestination, &owner, &request_id);
    let token = request.control();
    let worker_token = Arc::clone(&token);
    let worker_request_id = request_id.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _request = request;
        let progress = DestinationInspectionProgress::new(events, worker_request_id);
        let result = inspect_create_destination_impl_with_progress(
            &proposed,
            split,
            sfx_target,
            &progress,
            &worker_token,
        );
        progress.flush();
        result
    })
    .await;
    task.map_err(|error| {
        ErrorDto::destination_inspection(format!("destination inspection task failed: {error}"))
    })?
}

/// Cancels the matching destination inspection owned by this WebView, or
/// records the one-shot id when its begin call is still in transit. Late
/// cancellation of a recently completed request is ignored.
#[tauri::command]
pub fn cancel_create_destination_inspection(
    window: WebviewWindow,
    requests: State<'_, Arc<PreflightRequests>>,
    request_id: String,
) {
    requests.cancel(
        PreflightRequestKind::CreateDestination,
        window.label(),
        &request_id,
    );
}

#[cfg(test)]
fn inspect_create_destination_impl(
    proposed: &str,
    split: bool,
    sfx_target: Option<SfxTarget>,
) -> Result<CreateDestinationInspectionDto, ErrorDto> {
    inspect_create_destination_impl_with_progress(
        proposed,
        split,
        sfx_target,
        &NoProgress,
        &ControlToken::default(),
    )
}

fn inspect_create_destination_impl_with_progress(
    proposed: &str,
    split: bool,
    sfx_target: Option<SfxTarget>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<CreateDestinationInspectionDto, ErrorDto> {
    let kind = match sfx_target {
        Some(SfxTarget::Macos) => CreateArtifactKind::SfxMacosApp,
        Some(SfxTarget::Windows | SfxTarget::Linux) => CreateArtifactKind::SfxSingleFile,
        None if split => CreateArtifactKind::SplitArchive,
        None => CreateArtifactKind::Archive,
    };
    let state =
        inspect_core_create_destination_with_progress(Path::new(proposed), kind, progress, control)
            .map_err(ErrorDto::from)?;
    Ok(CreateDestinationInspectionDto {
        conflict: state.conflict,
        guard: state.guard,
    })
}

/// Persists the theme (`system` / `light` / `dark`).
#[tauri::command]
pub fn set_theme(
    settings: State<'_, Arc<SettingsStore>>,
    theme: String,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| s.theme = Some(theme))
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

/// Persists the language (`None` = follow the system).
#[tauri::command]
pub fn set_language(
    settings: State<'_, Arc<SettingsStore>>,
    language: Option<String>,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| s.language = language)
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

fn apply_general_options(
    settings: &mut SettingsDto,
    language: Option<String>,
    default_extract_dir: Option<String>,
    default_create_dir: Option<String>,
    reveal_after_extract: bool,
    check_updates_automatically: Option<bool>,
) {
    settings.language = language;
    settings.default_extract_dir = default_extract_dir.filter(|value| !value.trim().is_empty());
    settings.default_create_dir = default_create_dir.filter(|value| !value.trim().is_empty());
    settings.reveal_after_extract = reveal_after_extract;
    if let Some(enabled) = check_updates_automatically {
        settings.check_updates_automatically = Some(enabled);
    }
}

/// Persists General settings that belong to the desktop shell.
#[tauri::command]
pub fn set_general_options(
    settings: State<'_, Arc<SettingsStore>>,
    language: Option<String>,
    default_extract_dir: Option<String>,
    default_create_dir: Option<String>,
    reveal_after_extract: bool,
    check_updates_automatically: Option<bool>,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| {
            apply_general_options(
                s,
                language,
                default_extract_dir,
                default_create_dir,
                reveal_after_extract,
                check_updates_automatically,
            )
        })
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

/// Persists the UI mode (`modern` / `classic`).
#[tauri::command]
pub fn set_ui_mode(
    settings: State<'_, Arc<SettingsStore>>,
    ui_mode: String,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| s.ui_mode = Some(ui_mode))
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

/// Persists the desktop UI density (`compact` / `standard` / `comfort`).
#[tauri::command]
pub fn set_ui_density(
    settings: State<'_, Arc<SettingsStore>>,
    ui_density: String,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| {
            s.ui_density = Some(if valid_ui_density(&ui_density) {
                ui_density
            } else {
                "standard".to_owned()
            });
        })
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

/// Persists Appearance / Colors palette settings.
#[tauri::command]
pub fn set_accent_palette(
    settings: State<'_, Arc<SettingsStore>>,
    accent_palette: String,
    custom_accent: Option<String>,
    accent_contrast_guard: Option<bool>,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| apply_accent_palette(s, accent_palette, custom_accent, accent_contrast_guard))
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

fn apply_accent_palette(
    settings: &mut SettingsDto,
    accent_palette: String,
    custom_accent: Option<String>,
    accent_contrast_guard: Option<bool>,
) {
    let next_palette = if valid_accent_palette(&accent_palette) {
        accent_palette
    } else {
        "aqua".to_owned()
    };

    if let Some(normalized) = custom_accent.as_deref().and_then(|value| {
        valid_hex_color(value)
            .then(|| normalize_hex_color(value))
            .flatten()
    }) {
        settings.custom_accent = Some(normalized);
    } else if next_palette == "custom" && settings.custom_accent.is_none() {
        settings.custom_accent = Some("#2DD4BF".to_owned());
    }

    if let Some(value) = accent_contrast_guard {
        settings.accent_contrast_guard = Some(value);
    } else if settings.accent_contrast_guard.is_none() {
        settings.accent_contrast_guard = Some(true);
    }

    settings.accent_palette = Some(next_palette);
}

fn valid_accent_palette(value: &str) -> bool {
    matches!(
        value,
        "aqua" | "sage" | "nordic" | "copper" | "aubergine" | "mono" | "custom"
    )
}

fn valid_ui_density(value: &str) -> bool {
    matches!(value, "compact" | "standard" | "comfort")
}

fn valid_hex_color(value: &str) -> bool {
    normalize_hex_color(value).is_some()
}

fn normalize_hex_color(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex.to_ascii_uppercase()))
    } else {
        None
    }
}

/// Persists decompression-bomb guardrails. `None` restores the default.
#[tauri::command]
pub fn set_safety_limits(
    settings: State<'_, Arc<SettingsStore>>,
    max_output_bytes: Option<u64>,
    max_entries: Option<u64>,
    max_compression_ratio: Option<u32>,
) -> Result<SettingsDto, ErrorDto> {
    settings
        .update(|s| {
            s.safety_max_output_bytes = max_output_bytes.filter(|v| *v > 0);
            s.safety_max_entries = max_entries.filter(|v| *v > 0);
            s.safety_max_compression_ratio = max_compression_ratio.filter(|v| *v > 0);
        })
        .map_err(|error| ErrorDto::settings_write(error.to_string()))
}

/// Persists compression resource settings. `None` restores automatic resource choices.
#[tauri::command]
pub fn set_performance_options(
    settings: State<'_, Arc<SettingsStore>>,
    jobs: State<'_, Arc<JobManager>>,
    threads: Option<usize>,
    memory_limit_bytes: Option<u64>,
    parallel_jobs: Option<usize>,
) -> Result<SettingsDto, ErrorDto> {
    let saved = settings
        .update(|s| {
            s.performance_threads = threads.filter(|v| *v > 0).map(|v| v.min(64));
            s.performance_memory_limit_bytes =
                normalize_performance_stream_buffer_limit(memory_limit_bytes);
            s.performance_parallel_jobs = parallel_jobs
                .filter(|value| *value > 0)
                .map(|value| value.min(MAX_PARALLEL_JOBS));
        })
        .map_err(|error| ErrorDto::settings_write(error.to_string()))?;
    jobs.set_parallel_jobs(saved.performance_parallel_jobs);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use crate::preview_sessions::PreviewSessionManager;
    use squallz_core::api::{
        CompressionLevel, ControlToken, CreateOptions, EntryPath, ExtractOptions, FormatError,
        NoProgress, OpenOptions, Password, ProgressSink, SafetyLimits,
    };
    use squallz_core::api::{OverwritePolicy, SymlinkPolicy};
    use squallz_core::{
        ByteSize, ChecksumAlgorithm, CreateCompletionAction, CreateCredential, CreateDestination,
        CreateDestinationBase, CreateOutput, CreatePreset, Engine, EntryNameEncoding,
        ExtractCredential, ExtractDestination, ExtractDestinationBase, ExtractLayout,
        ExtractPreset, FormatId, FormatSpecificOptions, NamedPreset, PostSuccessAction,
        PresetCompressionLevel, PresetId, PresetLabel, VolumeMode,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use super::{
        apply_accent_palette, apply_general_options, archive_password_status_impl,
        bound_create_preset, bound_extract_preset, create_destination_has_conflict,
        disk_space_preflight, export_operation_history_impl, forget_archive_password_impl,
        format_label_from_name, inspect_create_destination_impl,
        inspect_create_destination_impl_with_progress, open_archive_resolving_password,
        open_archive_source_resolving_password, open_nested_archive_impl, plan_convert_impl,
        plan_extract_impl, preview_archive_entry_impl, preview_nested_archive_impl,
        preview_trace_payload, remember_archive_password_impl, requested_page_size,
        resolve_external_task_job_impl, unique_create_destination_path, valid_accent_palette,
        valid_hex_color, MAX_ARCHIVE_PAGE_SIZE,
    };
    use crate::dto::{ErrorDto, ExternalTaskActionDto, JobSpec, SettingsDto};
    use crate::secrets::{tests::MemorySecretStore, tests::ReadFailingSecretStore, SecretStore};
    use crate::state::{AppState, DEFAULT_PAGE_SIZE};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-command-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn archive_page_size_uses_safe_default_and_bounds() {
        assert_eq!(requested_page_size(None), DEFAULT_PAGE_SIZE);
        assert_eq!(requested_page_size(Some(0)), 1);
        assert_eq!(requested_page_size(Some(42)), 42);
        assert_eq!(requested_page_size(Some(usize::MAX)), MAX_ARCHIVE_PAGE_SIZE);
    }

    fn make_header_encrypted_7z(dir: &Path) -> PathBuf {
        let src = dir.join("secret-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("secret.txt"), b"classified").unwrap();
        let dest = dir.join("secret.7z");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .create(
                &dest,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    password: Some(Password::new("secret")),
                    encrypt_filenames: true,
                    ..CreateOptions::default()
                },
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        dest
    }

    #[test]
    fn disk_space_preflight_reports_available_capacity() {
        let dir = temp_dir("disk-space");
        let target = dir.join("archive.zip");
        let check = disk_space_preflight(&target, 1).unwrap();

        assert_eq!(check.path, dir.to_string_lossy().as_ref());
        assert_eq!(check.required_bytes, 1);
        assert!(check.available_bytes > 0);
        assert!(check.ok);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_plan_uses_the_session_password_and_core_layout() {
        let dir = temp_dir("extract-plan-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        let info = open_archive_resolving_password(
            &state,
            &MemorySecretStore::new(),
            "test-window",
            &archive,
            Some("secret"),
            None,
        )
        .unwrap();
        let destination = dir.join("output");
        let archive_text = archive.to_string_lossy();

        let plan = plan_extract_impl(
            &state,
            "test-window",
            archive_text.as_ref(),
            archive_text.as_ref(),
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .unwrap();
        assert_eq!(
            plan.plan.requested_destination,
            destination.to_string_lossy()
        );
        assert_eq!(plan.plan.destination, destination.to_string_lossy());
        assert_eq!(plan.plan.layout, "direct");
        assert_eq!(plan.plan.files, 1);
        assert!(plan.plan.entries >= 1);
        assert_eq!(plan.plan.total_bytes, 10);
        assert!(plan.required_free_bytes >= plan.plan.total_bytes);
        assert!(plan.available_bytes > 0);
        assert!(plan.space_ok);

        state.forget_password(&archive);
        let error = plan_extract_impl(
            &state,
            "test-window",
            archive_text.as_ref(),
            archive_text.as_ref(),
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert_eq!(error.key, "error.password_required");

        state.close_archive_for_window("test-window", info.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn convert_plan_uses_the_session_password_and_core_budget() {
        let dir = temp_dir("convert-plan-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        let info = open_archive_resolving_password(
            &state,
            &MemorySecretStore::new(),
            "test-window",
            &archive,
            Some("secret"),
            None,
        )
        .unwrap();
        let destination = dir.join("converted.7z");
        let spec = JobSpec::Convert {
            src: archive.to_string_lossy().into_owned(),
            dest: destination.to_string_lossy().into_owned(),
            level: 7,
            src_encoding: None,
            src_password: None,
            dest_password: Some("new secret".into()),
            encrypt_names: true,
            split_size: Some(128 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        };

        let plan = plan_convert_impl(
            &state,
            "test-window",
            &SettingsDto::default(),
            &spec,
            &ControlToken::default(),
        )
        .unwrap();
        assert_eq!(plan.input_count, 1);
        assert_eq!(plan.files, 1);
        assert_eq!(plan.total_bytes, 10);
        assert_eq!(
            plan.primary_output,
            dir.join("converted.7z.001").to_string_lossy()
        );
        assert!(plan.split_volume_count_budget.is_some());
        assert!(!destination.exists());

        state.forget_password(&archive);
        let error = plan_convert_impl(
            &state,
            "test-window",
            &SettingsDto::default(),
            &spec,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert_eq!(error.key, "error.password_required");

        state.close_archive_for_window("test-window", info.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn convert_plan_honors_cancellation_before_resolving_the_source() {
        let state = AppState::new();
        let control = ControlToken::new();
        control.cancel();
        let spec = JobSpec::Convert {
            src: "/source/that/does/not/exist.zip".into(),
            dest: "/destination/converted.7z".into(),
            level: 6,
            src_encoding: None,
            src_password: None,
            dest_password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        };

        let error = plan_convert_impl(
            &state,
            "test-window",
            &SettingsDto::default(),
            &spec,
            &control,
        )
        .unwrap_err();

        assert_eq!(error.key, "error.cancelled");
    }

    #[test]
    fn extract_plan_honors_a_cancelled_request_before_resolving_the_source() {
        let state = AppState::new();
        let destination = temp_dir("extract-plan-cancelled");
        let control = ControlToken::new();
        control.cancel();

        let error = plan_extract_impl(
            &state,
            "test-window",
            "/source/that/does/not/exist.zip",
            "/source/that/does/not/exist.zip",
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &control,
        )
        .unwrap_err();

        assert_eq!(error.key, "error.cancelled");
        std::fs::remove_dir_all(&destination).unwrap();
    }

    #[test]
    fn extract_plan_uses_the_resolved_display_path_for_smart_layout() {
        let dir = temp_dir("extract-plan-display-path");
        let source = dir.join("loose.txt");
        std::fs::write(&source, b"payload").unwrap();
        let archive = dir.join("actual-name.zip");
        let state = AppState::new();
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&source),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let destination = dir.join("output");

        let plan = plan_extract_impl(
            &state,
            "test-window",
            archive.to_string_lossy().as_ref(),
            "/untrusted/spoofed-name.zip",
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(plan.plan.layout, "wrap_in_folder");
        assert_eq!(
            plan.plan.destination,
            destination.join("actual-name").to_string_lossy()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preview_opener_trace_payload_omits_session_identity_and_path() {
        let dir = temp_dir("preview-opener-trace");
        let path = dir.join("squallz-nested-safe-preview.pdf");
        std::fs::write(&path, b"preview").unwrap();

        let payload = preview_trace_payload("open", "request", &path, None);
        let serialized = payload.to_string();
        let dir_text = dir.to_string_lossy();

        assert_eq!(payload["action"], "open");
        assert_eq!(payload["status"], "request");
        assert_eq!(payload["extension"], "pdf");
        assert!(payload.get("file_name").is_none());
        assert!(payload.get("preview_id").is_none());
        assert!(payload.get("path").is_none());
        assert!(!serialized.contains(dir_text.as_ref()));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn format_label_from_name_uses_detected_format_or_archive_fallback() {
        let state = AppState::new();

        assert_eq!(format_label_from_name(&state, "sample.zip"), "zip");
        assert_eq!(format_label_from_name(&state, "backup.tar.zst"), "tar.zstd");
        assert_eq!(
            format_label_from_name(&state, "unknown.squallz-test"),
            "archive"
        );
    }

    #[test]
    fn external_extract_actions_keep_their_distinct_destination_layouts() {
        let state = AppState::new();
        let presets = squallz_core::PresetDocument::seeded();
        let archive = "/Users/example/Downloads/backup.tar.gz".to_owned();

        let here = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::ExtractHere,
            vec![archive.clone()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("extract-here should resolve")
        .expect("extract-here should produce a job");
        match here {
            JobSpec::Extract { dest, smart, .. } => {
                assert_eq!(dest, "/Users/example/Downloads");
                assert!(smart);
            }
            other => panic!("expected extract job, got {other:?}"),
        }

        let folder = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::ExtractToFolder,
            vec![archive],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("extract-to-folder should resolve")
        .expect("extract-to-folder should produce a job");
        match folder {
            JobSpec::Extract { dest, smart, .. } => {
                assert_eq!(dest, "/Users/example/Downloads/backup");
                assert!(!smart, "archive folder must not be wrapped a second time");
            }
            other => panic!("expected extract job, got {other:?}"),
        }
    }

    #[test]
    fn external_create_uses_the_bound_preset_snapshot() {
        let state = AppState::new();
        let presets = squallz_core::PresetDocument::seeded();
        let job = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::CompressTo7z,
            vec!["/Users/example/Documents/report".to_owned()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("create action should resolve")
        .expect("create action should produce a job");
        match job {
            JobSpec::Compress {
                dest,
                level,
                password,
                ..
            } => {
                assert_eq!(dest, "/Users/example/Documents/report.7z");
                assert_eq!(level, 5);
                assert_eq!(password, None);
            }
            other => panic!("expected compress job, got {other:?}"),
        }
    }

    #[test]
    fn external_actions_use_safe_defaults_when_file_manager_presets_are_unbound() {
        let state = AppState::new();
        let mut presets = squallz_core::PresetDocument::seeded();
        presets.bindings.file_manager_create = None;
        presets.bindings.file_manager_extract = None;
        presets
            .validate()
            .expect("unbound presets should remain valid");

        assert!(bound_create_preset(&presets)
            .expect("unbound create lookup should succeed")
            .is_none());
        assert!(bound_extract_preset(&presets)
            .expect("unbound extract lookup should succeed")
            .is_none());

        let create = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::CompressTo7z,
            vec!["/Users/example/Documents/report".to_owned()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("unbound create action should use safe defaults")
        .expect("create action should produce a job");
        match create {
            JobSpec::Compress {
                level,
                encrypt_names,
                split_size,
                content_policy,
                excludes,
                sqz_inner_format,
                completion,
                post_success,
                ..
            } => {
                assert_eq!(level, 5);
                assert!(!encrypt_names);
                assert_eq!(split_size, None);
                assert_eq!(
                    content_policy,
                    squallz_core::CreateContentPolicy::KeepAllFiles
                );
                assert!(excludes.is_empty());
                assert_eq!(sqz_inner_format, None);
                assert_eq!(completion, CreateCompletionAction::None);
                assert_eq!(post_success, PostSuccessAction::KeepSource);
            }
            other => panic!("expected compress job, got {other:?}"),
        }

        let extract = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::ExtractHere,
            vec!["/Users/example/Downloads/report.7z".to_owned()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("unbound extract action should use safe defaults")
        .expect("extract action should produce a job");
        match extract {
            JobSpec::Extract {
                overwrite,
                symlinks,
                encoding,
                ..
            } => {
                assert!(matches!(overwrite, OverwritePolicy::Ask));
                assert!(matches!(symlinks, SymlinkPolicy::Preserve));
                assert_eq!(encoding, None);
            }
            other => panic!("expected extract job, got {other:?}"),
        }
    }

    #[test]
    fn external_actions_apply_bound_policy_without_overriding_explicit_destination_action() {
        let state = AppState::new();
        let mut presets = squallz_core::PresetDocument::seeded();

        let create_id = PresetId::new("user.create.finder-fast").expect("valid create preset id");
        presets.presets.push(NamedPreset::Create {
            id: create_id.clone(),
            label: PresetLabel::new("Finder fast").expect("valid create preset label"),
            built_in: false,
            options: CreatePreset {
                format: FormatId::new("7z").expect("valid format id"),
                level: PresetCompressionLevel::new(8).expect("valid compression level"),
                credential: CreateCredential::None,
                encrypt_names: false,
                volumes: VolumeMode::Split {
                    size_bytes: ByteSize::new(2 * 1024 * 1024),
                },
                content_policy: squallz_core::CreateContentPolicy::Custom,
                excludes: vec!["*.tmp".to_owned()],
                output: CreateOutput::Archive,
                destination: CreateDestination {
                    base: CreateDestinationBase::Ask,
                    existing_output: OverwritePolicy::Ask,
                },
                format_options: FormatSpecificOptions::None,
                completion: CreateCompletionAction::None,
                post_success: PostSuccessAction::KeepSource,
                test_after_create: true,
            },
        });
        presets.bindings.file_manager_create = Some(create_id);

        let extract_id =
            PresetId::new("user.extract.finder-safe").expect("valid extract preset id");
        presets.presets.push(NamedPreset::Extract {
            id: extract_id.clone(),
            label: PresetLabel::new("Finder safe").expect("valid extract preset label"),
            built_in: false,
            options: ExtractPreset {
                destination: ExtractDestination {
                    base: ExtractDestinationBase::Ask,
                    layout: ExtractLayout::Direct,
                },
                existing_output: OverwritePolicy::RenameBoth,
                symlinks: SymlinkPolicy::Skip,
                encoding: EntryNameEncoding::Named {
                    label: "GBK".to_owned(),
                },
                credential: ExtractCredential::PromptWhenNeeded,
                post_success: PostSuccessAction::KeepSource,
            },
        });
        presets.bindings.file_manager_extract = Some(extract_id);
        presets
            .validate()
            .expect("bound preset document should be valid");

        let create = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::CompressTo7z,
            vec!["/Users/example/Documents/report".to_owned()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("bound create action should resolve")
        .expect("create action should produce a job");
        match create {
            JobSpec::Compress {
                level,
                split_size,
                content_policy,
                excludes,
                completion,
                post_success,
                test_after_create,
                ..
            } => {
                assert_eq!(level, 8);
                assert_eq!(split_size, Some(2 * 1024 * 1024));
                assert_eq!(content_policy, squallz_core::CreateContentPolicy::Custom);
                assert_eq!(excludes, vec!["*.tmp"]);
                assert_eq!(completion, CreateCompletionAction::None);
                assert_eq!(post_success, PostSuccessAction::KeepSource);
                assert!(test_after_create);
            }
            other => panic!("expected compress job, got {other:?}"),
        }

        let extract = resolve_external_task_job_impl(
            &state,
            &presets,
            ExternalTaskActionDto::ExtractHere,
            vec!["/Users/example/Downloads/backup.tar.gz".to_owned()],
            None,
            ChecksumAlgorithm::Sha256,
            Vec::new(),
        )
        .expect("bound extract action should resolve")
        .expect("extract action should produce a job");
        match extract {
            JobSpec::Extract {
                dest,
                smart,
                overwrite,
                symlinks,
                encoding,
                ..
            } => {
                assert_eq!(dest, "/Users/example/Downloads");
                assert!(smart);
                assert!(matches!(overwrite, OverwritePolicy::RenameBoth));
                assert!(matches!(symlinks, SymlinkPolicy::Skip));
                assert_eq!(encoding.as_deref(), Some("GBK"));
            }
            other => panic!("expected extract job, got {other:?}"),
        }
    }

    #[test]
    fn general_options_update_language_default_dir_and_reveal_preference() {
        let mut settings = SettingsDto::default();

        apply_general_options(
            &mut settings,
            Some("zh-CN".to_owned()),
            Some("  /tmp/Squallz Extracts  ".to_owned()),
            Some("  /tmp/Squallz Archives  ".to_owned()),
            true,
            Some(false),
        );
        assert_eq!(settings.language.as_deref(), Some("zh-CN"));
        assert_eq!(
            settings.default_extract_dir.as_deref(),
            Some("  /tmp/Squallz Extracts  ")
        );
        assert_eq!(
            settings.default_create_dir.as_deref(),
            Some("  /tmp/Squallz Archives  ")
        );
        assert!(settings.reveal_after_extract);
        assert!(!settings.automatic_update_checks_enabled());

        apply_general_options(
            &mut settings,
            None,
            Some("  ".to_owned()),
            Some("\t".to_owned()),
            false,
            None,
        );
        assert_eq!(settings.language, None);
        assert_eq!(settings.default_extract_dir, None);
        assert_eq!(settings.default_create_dir, None);
        assert!(!settings.reveal_after_extract);
        assert!(!settings.automatic_update_checks_enabled());
    }

    #[test]
    fn unique_create_destination_avoids_files_and_split_volume_families() {
        let dir = temp_dir("unique-create-destination");
        let archive = dir.join("archive.zip");

        assert_eq!(
            unique_create_destination_path(&archive, false).unwrap(),
            archive
        );
        std::fs::write(&archive, b"existing archive").unwrap();
        assert_eq!(
            unique_create_destination_path(&archive, false).unwrap(),
            dir.join("archive (2).zip")
        );

        std::fs::remove_file(&archive).unwrap();
        std::fs::write(dir.join("archive.zip.1000"), b"orphan volume").unwrap();
        std::fs::write(dir.join("archive (2).zip.001"), b"existing family").unwrap();
        assert_eq!(
            unique_create_destination_path(&archive, true).unwrap(),
            dir.join("archive (3).zip")
        );
        assert_eq!(
            unique_create_destination_path(&archive, false).unwrap(),
            archive
        );

        std::fs::remove_file(dir.join("archive.zip.1000")).unwrap();
        std::fs::remove_file(dir.join("archive (2).zip.001")).unwrap();
        std::fs::write(dir.join("archive.zip.rev001"), b"recovery sidecar").unwrap();
        assert_eq!(
            unique_create_destination_path(&archive, true).unwrap(),
            archive
        );

        let sqz = dir.join("archive.sqz");
        std::fs::write(dir.join("archive.sqz.rev001"), b"recovery sidecar").unwrap();
        assert_eq!(
            unique_create_destination_path(&sqz, true).unwrap(),
            dir.join("archive (2).sqz")
        );

        let compound = dir.join("archive.tar.zst");
        std::fs::write(&compound, b"existing compound archive").unwrap();
        assert_eq!(
            unique_create_destination_path(&compound, false).unwrap(),
            dir.join("archive (2).tar.zst")
        );

        let unavailable = dir.join("missing").join("archive.zip");
        assert_eq!(
            unique_create_destination_path(&unavailable, false)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn create_destination_conflict_includes_split_family_only_when_requested() {
        let dir = temp_dir("create-destination-conflict");
        let base = dir.join("archive.zip");
        std::fs::write(dir.join("archive.zip.001"), b"existing volume").unwrap();

        assert!(
            !create_destination_has_conflict(base.to_string_lossy().into_owned(), false,).unwrap()
        );
        assert!(
            create_destination_has_conflict(base.to_string_lossy().into_owned(), true,).unwrap()
        );
        std::fs::write(&base, b"existing archive").unwrap();
        assert!(
            create_destination_has_conflict(base.to_string_lossy().into_owned(), false,).unwrap()
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn destination_inspection_returns_a_content_bound_split_guard() {
        let dir = temp_dir("create-destination-inspection");
        let first = dir.join("archive.zip.001");
        std::fs::write(&first, b"old volume").unwrap();

        let first_display = first.to_string_lossy();
        let before = inspect_create_destination_impl(&first_display, true, None).unwrap();
        assert!(before.conflict);
        assert!(before.guard.is_some());

        std::fs::write(&first, b"new volume").unwrap();
        let after = inspect_create_destination_impl(&first_display, true, None).unwrap();
        assert_ne!(before.guard, after.guard);

        std::fs::remove_dir_all(dir).unwrap();
    }

    struct CancelDestinationProgress {
        control: Arc<ControlToken>,
        observed_bytes: AtomicU64,
    }

    impl ProgressSink for CancelDestinationProgress {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > 0 {
                self.observed_bytes.store(done, Ordering::Relaxed);
                self.control.cancel();
            }
        }
    }

    #[test]
    fn destination_inspection_forwards_progress_and_cancelled_error() {
        let dir = temp_dir("create-destination-inspection-cancel");
        let target = dir.join("archive.zip");
        std::fs::write(&target, vec![0x5a; 256 * 1024]).unwrap();
        let control = ControlToken::new();
        let progress = CancelDestinationProgress {
            control: Arc::clone(&control),
            observed_bytes: AtomicU64::new(0),
        };

        let error = inspect_create_destination_impl_with_progress(
            &target.to_string_lossy(),
            false,
            None,
            &progress,
            &control,
        )
        .unwrap_err();

        assert_eq!(error.key, "error.cancelled");
        assert!(progress.observed_bytes.load(Ordering::Relaxed) > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_operation_history_writes_sanitized_json() {
        let dir = temp_dir("history-export");
        let target = dir.join("history.json");
        let contents = r#"{
  "generatedAt": "2026-06-12T00:00:00.000Z",
  "filter": "all",
  "records": [
    {
      "id": "1",
      "time": 1781199120000,
      "status": "done",
      "title": "Create archive queued",
      "detail": "backup.zip"
    }
  ]
}"#;

        export_operation_history_impl(&target, contents).unwrap();
        let written = std::fs::read_to_string(&target).unwrap();
        assert!(written.contains("\"Create archive queued\""));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn export_operation_history_rejects_invalid_payload() {
        let dir = temp_dir("history-export-invalid");
        let target = dir.join("history.json");
        let err = export_operation_history_impl(&target, r#"{"records":[{"title":"missing"}]}"#)
            .unwrap_err();
        assert!(matches!(err, FormatError::Unsupported(_)), "{err:?}");
        assert!(!target.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn make_nested_zip_archive(state: &AppState, dir: &Path) -> PathBuf {
        let inner_src = dir.join("inner-src");
        std::fs::create_dir_all(&inner_src).unwrap();
        std::fs::write(inner_src.join("hello.txt"), b"hello nested").unwrap();
        let inner = dir.join("inner.zip");
        state
            .engine
            .create(
                &inner,
                std::slice::from_ref(&inner_src),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let outer = dir.join("outer.zip");
        state
            .engine
            .create(
                &outer,
                std::slice::from_ref(&inner),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        outer
    }

    fn make_outer_archive_with_file(
        state: &AppState,
        dir: &Path,
        file_name: &str,
        contents: &[u8],
    ) -> PathBuf {
        let file = dir.join(file_name);
        std::fs::write(&file, contents).unwrap();
        let outer = dir.join(format!("outer-{file_name}.zip"));
        state
            .engine
            .create(
                &outer,
                &[file],
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        outer
    }

    fn make_outer_archive_with_renamed_zip(
        state: &AppState,
        dir: &Path,
        file_name: &str,
    ) -> PathBuf {
        let nested = make_nested_zip_archive(state, dir);
        let source = dir.join("inner.zip");
        let renamed = dir.join(file_name);
        std::fs::copy(source, &renamed).unwrap();
        let outer = dir.join(format!("renamed-{file_name}.zip"));
        state
            .engine
            .create(
                &outer,
                &[renamed],
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        std::fs::remove_file(nested).unwrap();
        outer
    }

    fn assert_error_hides_private_path(error: &ErrorDto, root: &Path, path: Option<&Path>) {
        let serialized = serde_json::to_string(error).unwrap();
        let root = root.to_string_lossy();
        assert!(!error.detail.contains(root.as_ref()));
        assert!(error
            .params
            .values()
            .all(|value| !value.contains(root.as_ref())));
        assert!(!serialized.contains(root.as_ref()));
        if let Some(path) = path {
            let path = path.to_string_lossy();
            assert!(!error.detail.contains(path.as_ref()));
            assert!(error
                .params
                .values()
                .all(|value| !value.contains(path.as_ref())));
            assert!(!serialized.contains(path.as_ref()));
        }
    }

    #[test]
    fn nested_archive_preview_lists_inner_entries() {
        let dir = temp_dir("nested-preview");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();

        let preview = preview_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "inner.zip",
            None,
            None,
        )
        .unwrap();
        assert_eq!(preview.entry_path, "inner.zip");
        assert_eq!(preview.format, "zip");
        assert!(!preview.truncated);
        assert!(preview
            .items
            .iter()
            .any(|entry| entry.path == "inner-src/hello.txt"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nested_preview_and_open_errors_hide_materialized_temp_paths() {
        let dir = temp_dir("nested-error-redaction");
        let state = AppState::new();
        let outer = make_outer_archive_with_file(&state, &dir, "mystery.bin", b"not an archive");
        let sessions = PreviewSessionManager::new().unwrap();
        let preview_root = sessions.root_path().unwrap().to_path_buf();

        let preview_error = preview_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "mystery.bin",
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(preview_error.key, "error.unsupported");
        assert_error_hides_private_path(&preview_error, &preview_root, None);

        let open_error = open_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "mystery.bin",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap_err();
        assert_eq!(open_error.key, "error.unsupported");
        assert_error_hides_private_path(&open_error, &preview_root, None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opaque_nested_source_errors_hide_the_cached_physical_path() {
        let dir = temp_dir("opaque-source-error-redaction");
        let state = AppState::new();
        let outer = make_outer_archive_with_renamed_zip(&state, &dir, "inner.blob");
        let sessions = PreviewSessionManager::new().unwrap();
        let info = open_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "inner.blob",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap();
        let resolved = state
            .resolve_archive_source(&info.source, Some("test-window"))
            .unwrap();
        let physical = resolved.path().to_path_buf();
        std::fs::write(&physical, b"no longer an archive").unwrap();
        drop(resolved);
        let preview_root = sessions.root_path().unwrap().to_path_buf();

        let preview_error = preview_archive_entry_impl(
            &state,
            &sessions,
            "test-window",
            &info.source,
            "inner-src/hello.txt",
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(preview_error.key, "error.unsupported");
        assert_error_hides_private_path(&preview_error, &preview_root, Some(&physical));

        let reopen_error = open_archive_source_resolving_password(
            &state,
            &MemorySecretStore::new(),
            "test-window",
            &info.source,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(reopen_error.key, "error.unsupported");
        assert_error_hides_private_path(&reopen_error, &preview_root, Some(&physical));

        state.close_archive_for_window("test-window", info.id);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_entry_preview_extracts_real_temp_file() {
        let dir = temp_dir("entry-preview");
        let state = AppState::new();
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("note.txt"), b"preview me").unwrap();
        let archive = dir.join("preview.zip");
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&src),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let sessions = PreviewSessionManager::new().unwrap();
        let preview = preview_archive_entry_impl(
            &state,
            &sessions,
            "test-window",
            &archive.to_string_lossy(),
            "src/note.txt",
            None,
            None,
        )
        .unwrap();
        assert_eq!(preview.entry_path, "src/note.txt");
        assert_eq!(preview.display_name, "note.txt");
        assert_eq!(preview.size, 10);
        assert!(!preview.archive_like);
        assert!(!preview.preview_id.contains("note"));
        let serialized = serde_json::to_value(&preview).unwrap();
        assert!(serialized.get("preview_id").is_some());
        assert!(serialized.get("temp_path").is_none());
        assert!(sessions
            .path_for_external_use(&preview.preview_id, "other-window")
            .is_err());
        let preview_path = sessions
            .path_for_external_use(&preview.preview_id, "test-window")
            .unwrap();
        assert_eq!(std::fs::read(preview_path).unwrap(), b"preview me");

        sessions.cleanup();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn image_entry_preview_is_materialized_for_system_open() {
        let dir = temp_dir("entry-preview-image-system-open");
        let state = AppState::new();
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let png = b"image bytes for system opener";
        std::fs::write(src.join("pixel.png"), png).unwrap();
        let archive = dir.join("preview-image.zip");
        state
            .engine
            .create(
                &archive,
                std::slice::from_ref(&src),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();

        let sessions = PreviewSessionManager::new().unwrap();
        let preview = preview_archive_entry_impl(
            &state,
            &sessions,
            "test-window",
            &archive.to_string_lossy(),
            "src/pixel.png",
            None,
            None,
        )
        .unwrap();
        let preview_path = sessions
            .path_for_external_use(&preview.preview_id, "test-window")
            .unwrap();
        assert_eq!(std::fs::read(&preview_path).unwrap(), png);
        sessions.external_use_succeeded(&preview.preview_id, "test-window");

        assert!(!sessions
            .release(&preview.preview_id, "test-window")
            .unwrap());
        assert_eq!(std::fs::read(&preview_path).unwrap(), png);
        sessions.cleanup();
        assert!(!preview_path.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_returns_cached_inner_archive() {
        let dir = temp_dir("nested-open");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();

        let info = open_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "inner.zip",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap();
        assert_eq!(info.format, "zip");
        assert_eq!(info.name, "inner.zip");
        assert!(info.read_only);
        assert!(info.source.starts_with("squallz-archive://"));
        assert!(!info
            .path
            .contains(sessions.root_path().unwrap().to_string_lossy().as_ref()));
        let source = state
            .resolve_archive_source(&info.source, Some("test-window"))
            .unwrap();
        let temp = source.path().to_path_buf();
        assert!(
            temp.exists(),
            "nested temp archive should persist while open"
        );
        drop(source);

        let foreign_error = state
            .list_entries_for_window("other-window", info.id, 0, DEFAULT_PAGE_SIZE, "", None)
            .unwrap_err()
            .to_string();
        let unknown_error = state
            .list_entries_for_window("other-window", u64::MAX, 0, DEFAULT_PAGE_SIZE, "", None)
            .unwrap_err()
            .to_string();
        let foreign_source_error =
            match state.resolve_archive_source(&info.source, Some("other-window")) {
                Ok(_) => panic!("a foreign owner resolved a nested archive"),
                Err(error) => error.to_string(),
            };
        assert_eq!(foreign_error, unknown_error);
        assert_eq!(foreign_error, foreign_source_error);
        assert!(state
            .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, "", None)
            .is_err());
        state.close_archive_for_window("other-window", info.id);
        assert!(temp.exists(), "a foreign close must be a silent no-op");

        let page = state
            .list_entries_for_window("test-window", info.id, 0, DEFAULT_PAGE_SIZE, "", None)
            .unwrap();
        assert!(page.items.iter().any(|entry| entry.path == "inner-src/"));

        let nested_page = state
            .list_entries_for_window(
                "test-window",
                info.id,
                0,
                DEFAULT_PAGE_SIZE,
                "inner-src/",
                None,
            )
            .unwrap();
        assert!(nested_page
            .items
            .iter()
            .any(|entry| entry.path == "inner-src/hello.txt"));

        state.close_archive_for_window("test-window", info.id);
        assert!(
            !temp.exists(),
            "closing the cache should delete its plaintext"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_plan_keeps_opaque_nested_sources_owner_bound() {
        let dir = temp_dir("nested-extract-plan-owner");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();
        let info = open_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "inner.zip",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap();
        let destination = dir.join("nested-plan-output");

        let foreign = plan_extract_impl(
            &state,
            "other-window",
            &info.source,
            &info.path,
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .unwrap_err();
        assert_eq!(foreign.detail, "archive is no longer available");

        let plan = plan_extract_impl(
            &state,
            "test-window",
            &info.source,
            "/untrusted/spoofed-name.zip",
            &destination,
            None,
            true,
            None,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .unwrap();
        assert_eq!(plan.plan.destination, destination.to_string_lossy());
        assert_eq!(plan.plan.layout, "direct");
        assert_eq!(plan.plan.files, 1);

        state.close_archive_for_window("test-window", info.id);
        sessions.cleanup();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn releasing_a_window_drops_its_nested_archive_plaintext() {
        let dir = temp_dir("nested-window-release");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();
        let info = open_nested_archive_impl(
            &state,
            &sessions,
            "released-window",
            &outer.to_string_lossy(),
            "inner.zip",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap();
        let source = state
            .resolve_archive_source(&info.source, Some("released-window"))
            .unwrap();
        let temp = source.path().to_path_buf();
        drop(source);

        assert_eq!(state.release_window("released-window"), 1);
        assert!(
            !temp.exists(),
            "window release must delete nested plaintext"
        );
        assert!(state
            .open_archive_source("released-window", &info.source, None, None)
            .is_err());
        assert_eq!(state.release_window("released-window"), 0);
        assert_eq!(sessions.release_window("released-window"), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_shutdown_drops_nested_plaintext_for_every_window() {
        let dir = temp_dir("nested-shutdown");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();
        let mut temps = Vec::new();
        for owner in ["window-a", "window-b"] {
            let info = open_nested_archive_impl(
                &state,
                &sessions,
                owner,
                &outer.to_string_lossy(),
                "inner.zip",
                None,
                None,
                SafetyLimits::default().max_entries,
            )
            .unwrap();
            let source = state
                .resolve_archive_source(&info.source, Some(owner))
                .unwrap();
            temps.push(source.path().to_path_buf());
        }

        state.begin_shutdown();
        assert_eq!(state.shutdown(), 2);
        assert_eq!(state.shutdown(), 0);
        assert!(temps.iter().all(|path| !path.exists()));
        sessions.cleanup();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_temp_can_feed_extract() {
        let dir = temp_dir("nested-extract");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);
        let sessions = PreviewSessionManager::new().unwrap();

        let info = open_nested_archive_impl(
            &state,
            &sessions,
            "test-window",
            &outer.to_string_lossy(),
            "inner.zip",
            None,
            None,
            SafetyLimits::default().max_entries,
        )
        .unwrap();
        let source = state
            .resolve_archive_source(&info.source, Some("test-window"))
            .unwrap();
        let temp = source.path().to_path_buf();
        let dest = dir.join("nested-out");

        state
            .engine
            .extract(
                &temp,
                &dest,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        drop(source);
        assert_eq!(
            std::fs::read(dest.join("inner-src/hello.txt")).unwrap(),
            b"hello nested"
        );

        state.close_archive_for_window("test-window", info.id);
        assert!(
            !temp.exists(),
            "closing the cache should delete its plaintext"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_parallel_uses_distinct_temp_files() {
        let dir = temp_dir("nested-parallel");
        let state = std::sync::Arc::new(AppState::new());
        let sessions = std::sync::Arc::new(PreviewSessionManager::new().unwrap());
        let outer = make_nested_zip_archive(state.as_ref(), &dir);

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let state = std::sync::Arc::clone(&state);
                let sessions = std::sync::Arc::clone(&sessions);
                let outer = outer.clone();
                std::thread::spawn(move || {
                    open_nested_archive_impl(
                        state.as_ref(),
                        sessions.as_ref(),
                        "test-window",
                        &outer.to_string_lossy(),
                        "inner.zip",
                        None,
                        None,
                        SafetyLimits::default().max_entries,
                    )
                    .unwrap()
                })
            })
            .collect();
        let infos: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let preview_root = sessions
            .root_path()
            .expect("preview workspace should be available");
        let mut paths = std::collections::HashSet::new();
        for info in &infos {
            let source = state
                .resolve_archive_source(&info.source, Some("test-window"))
                .unwrap();
            assert!(
                paths.insert(source.path().to_path_buf()),
                "duplicate nested temp source"
            );
            assert!(
                source.path().exists(),
                "nested temp archive should persist while open"
            );
            assert_ne!(Path::new(&info.path), source.path());
            assert!(
                !Path::new(&info.path).starts_with(preview_root),
                "display path must not expose the preview workspace"
            );
        }

        for info in infos {
            let source = state
                .resolve_archive_source(&info.source, Some("test-window"))
                .unwrap();
            let temp = source.path().to_path_buf();
            drop(source);
            state.close_archive_for_window("test-window", info.id);
            assert!(
                !temp.exists(),
                "closing the cache should delete its plaintext"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_archive_uses_saved_password_after_session_miss() {
        let dir = temp_dir("saved-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        let secrets = MemorySecretStore::new();
        secrets.insert(archive.clone(), "secret");

        let info =
            open_archive_resolving_password(&state, &secrets, "test-window", &archive, None, None)
                .unwrap();
        assert_eq!(info.format, "7z");
        assert_eq!(
            state.password_for(&archive).as_ref().map(Password::expose),
            Some("secret")
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn manual_password_takes_precedence_over_saved_password() {
        let dir = temp_dir("manual-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        let secrets = MemorySecretStore::new();
        secrets.insert(archive.clone(), "wrong");

        let info = open_archive_resolving_password(
            &state,
            &secrets,
            "test-window",
            &archive,
            Some("secret"),
            None,
        )
        .unwrap();
        assert_eq!(info.format, "7z");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn password_book_commands_verify_before_save_and_forget() {
        let dir = temp_dir("password-book-runtime");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        let secrets = MemorySecretStore::new();

        let initial = archive_password_status_impl(&secrets, &archive).unwrap();
        assert!(initial.available);
        assert!(!initial.saved);

        let err =
            remember_archive_password_impl(&state, &secrets, &archive, "wrong", None).unwrap_err();
        assert_ne!(
            err.key, "error.other",
            "wrong passwords must come from engine validation"
        );
        assert!(!secrets.has_archive_password(&archive).unwrap());
        assert!(state.password_for(&archive).is_none());

        let saved =
            remember_archive_password_impl(&state, &secrets, &archive, "secret", None).unwrap();
        assert!(saved.available);
        assert!(saved.saved);
        assert!(
            archive_password_status_impl(&secrets, &archive)
                .unwrap()
                .saved
        );
        assert_eq!(
            secrets
                .get_archive_password(&archive)
                .unwrap()
                .as_ref()
                .map(Password::expose),
            Some("secret")
        );
        assert_eq!(
            state.password_for(&archive).as_ref().map(Password::expose),
            Some("secret")
        );

        let forgotten = forget_archive_password_impl(&state, &secrets, &archive).unwrap();
        assert!(forgotten.available);
        assert!(!forgotten.saved);
        assert!(
            !archive_password_status_impl(&secrets, &archive)
                .unwrap()
                .saved
        );
        assert!(state.password_for(&archive).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn password_book_status_reports_secret_store_failures() {
        let error = archive_password_status_impl(
            &ReadFailingSecretStore,
            Path::new("/tmp/locked-password-book.7z"),
        )
        .unwrap_err();

        assert_eq!(error.key, "error.secret_store");
        assert!(error.params.is_empty());
        assert!(error.detail.contains("locked"));
    }

    #[test]
    fn forgetting_always_clears_the_session_password_when_persistent_delete_fails() {
        let path = Path::new("/tmp/locked-password-book.7z");
        let state = AppState::new();
        state.remember_password(path, "session secret");

        let error = forget_archive_password_impl(&state, &ReadFailingSecretStore, path)
            .expect_err("persistent delete must still be reported");

        assert_eq!(error.key, "error.secret_store");
        assert!(state.password_for(path).is_none());
    }

    #[test]
    fn accent_palette_validation_accepts_only_known_palettes() {
        assert!(valid_accent_palette("aqua"));
        assert!(valid_accent_palette("mono"));
        assert!(valid_accent_palette("custom"));
        assert!(!valid_accent_palette("black"));
        assert!(!valid_accent_palette("../theme"));
    }

    #[test]
    fn accent_color_validation_requires_hex_triplet() {
        assert!(valid_hex_color("#2DD4BF"));
        assert!(valid_hex_color("#0ea5e9"));
        assert!(!valid_hex_color("2DD4BF"));
        assert!(!valid_hex_color("#2DD4B"));
        assert!(!valid_hex_color("#2DD4BFG"));
    }

    #[test]
    fn accent_palette_preserves_existing_custom_accent_when_omitted() {
        let mut settings = SettingsDto {
            accent_palette: Some("custom".into()),
            custom_accent: Some("#D946EF".into()),
            ..SettingsDto::default()
        };

        apply_accent_palette(&mut settings, "aqua".into(), None, None);

        assert_eq!(settings.accent_palette.as_deref(), Some("aqua"));
        assert_eq!(settings.custom_accent.as_deref(), Some("#D946EF"));
        assert_eq!(settings.accent_contrast_guard, Some(true));
    }

    #[test]
    fn accent_palette_normalizes_and_defaults_custom_accent() {
        let mut settings = SettingsDto::default();

        apply_accent_palette(
            &mut settings,
            "custom".into(),
            Some("#0ea5e9".into()),
            Some(false),
        );

        assert_eq!(settings.accent_palette.as_deref(), Some("custom"));
        assert_eq!(settings.custom_accent.as_deref(), Some("#0EA5E9"));
        assert_eq!(settings.accent_contrast_guard, Some(false));

        settings.custom_accent = None;
        apply_accent_palette(&mut settings, "custom".into(), None, Some(true));

        assert_eq!(settings.custom_accent.as_deref(), Some("#2DD4BF"));
        assert_eq!(settings.accent_contrast_guard, Some(true));
    }
}
