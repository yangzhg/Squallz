//! Tauri command surface: thin wrappers over [`AppState`], [`JobManager`]
//! and [`SettingsStore`]. All real logic lives in those modules so it can
//! be tested without a window.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use tauri::{AppHandle, Emitter, State};

use squallz_core::api::{Detected, FormatError, FormatKind, OpenOptions};
use squallz_i18n::Localizer;

use crate::audit::{OperationAudit, OperationAuditRecord};
use crate::bridge::AskAnswer;
use crate::dto::{
    ArchiveInfo, CreateEstimateDto, DiskSpaceDto, EntryDto, EntryPreviewDto, ErrorDto, FormatDto,
    IntegrationApplyResultDto, IntegrationRemoveResultDto, IntegrationStatusDto, JobSpec,
    LanguageDto, LocaleTable, NestedArchivePreviewDto, Page, PasswordBookStatusDto, SettingsDto,
};
use crate::events::EventSink;
use crate::integration;
use crate::jobs::JobManager;
use crate::nested::extract_nested_archive_to_temp;
use crate::open_files::{focus_main_window, OpenFileRequests, OpenFilesEvent};
use crate::secrets::{SecretStore, SharedSecretStore};
use crate::settings::SettingsStore;
use crate::state::{normalized_entry_path, AppState, DEFAULT_PAGE_SIZE};
use crate::validation_trace;
use serde_json::json;

const HISTORY_EXPORT_MAX_BYTES: usize = 1024 * 1024;
const PREFLIGHT_PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
const INLINE_PREVIEW_MAX_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_OPERATION_AUDIT_LIMIT: usize = 80;

/// `EventSink` backed by the real Tauri app handle.
pub struct TauriEvents(pub AppHandle);

impl EventSink for TauriEvents {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        if let Err(e) = self.0.emit(event, payload) {
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

fn open_archive_resolving_password(
    state: &AppState,
    secrets: &dyn SecretStore,
    path: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<ArchiveInfo, FormatError> {
    if let Some(password) = password {
        return state.open_archive(path, Some(password), encoding);
    }

    match state.open_archive(path, None, encoding) {
        Ok(info) => Ok(info),
        Err(e) if password_error(&e) => match secrets.get_archive_password(path) {
            Ok(Some(saved)) => match state.open_archive(path, Some(saved.expose()), encoding) {
                Ok(info) => Ok(info),
                Err(saved_error) if password_error(&saved_error) => Err(FormatError::WrongPassword),
                Err(saved_error) => Err(saved_error),
            },
            Ok(None) => Err(e),
            Err(secret_error) => {
                log::warn!("password book: cannot read stored password: {secret_error}");
                Err(e)
            }
        },
        Err(e) => Err(e),
    }
}

fn archive_password_status_impl(secrets: &dyn SecretStore, path: &Path) -> PasswordBookStatusDto {
    let available = secrets.is_available();
    let saved = if available {
        match secrets.has_archive_password(path) {
            Ok(saved) => saved,
            Err(e) => {
                log::warn!("password book: cannot read status: {e}");
                false
            }
        }
    } else {
        false
    };
    PasswordBookStatusDto { available, saved }
}

fn remember_archive_password_impl(
    state: &AppState,
    secrets: &dyn SecretStore,
    path: &Path,
    password: &str,
    encoding: Option<&str>,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    if !secrets.is_available() {
        return Err(ErrorDto::other(
            "persistent secret storage is not available on this platform",
        ));
    }
    state
        .verify_password(path, password, encoding)
        .map_err(ErrorDto::from)?;
    secrets
        .set_archive_password(path, password)
        .map_err(|e| ErrorDto::other(e.to_string()))?;
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
    secrets
        .delete_archive_password(path)
        .map_err(|e| ErrorDto::other(e.to_string()))?;
    state.forget_password(path);
    Ok(PasswordBookStatusDto {
        available: secrets.is_available(),
        saved: false,
    })
}

/// Opens an archive and caches its entry list. `PasswordRequired` comes
/// back as a structured error so the frontend can prompt and retry.
#[tauri::command]
pub fn open_archive(
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<ArchiveInfo, ErrorDto> {
    let result = open_archive_resolving_password(
        &state,
        secrets.inner().as_ref(),
        Path::new(&path),
        password.as_deref(),
        encoding.as_deref(),
    );
    match &result {
        Ok(info) => validation_trace::trace(
            "open_archive.ok",
            json!({
                "path": path,
                "format": info.format,
                "entry_count": info.entry_count,
            }),
        ),
        Err(e) => validation_trace::trace(
            "open_archive.err",
            json!({
                "path": path,
                "error": ErrorDto::from(e).key,
            }),
        ),
    }
    result.map_err(ErrorDto::from)
}

/// Releases a cached archive.
#[tauri::command]
pub fn close_archive(state: State<'_, Arc<AppState>>, id: u64) {
    state.close_archive(id);
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
    state: State<'_, Arc<AppState>>,
    id: u64,
    page: usize,
    page_size: Option<usize>,
    dir_prefix: Option<String>,
    filter: Option<String>,
) -> Result<Page, ErrorDto> {
    state
        .list_entries(
            id,
            page,
            requested_page_size(page_size),
            requested_dir_prefix(dir_prefix.as_deref()),
            filter.as_deref(),
        )
        .map_err(ErrorDto::from)
}

fn requested_page_size(page_size: Option<usize>) -> usize {
    match page_size {
        Some(page_size) => page_size,
        None => DEFAULT_PAGE_SIZE,
    }
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

/// Estimates local create inputs after applying the same exclude rules as a
/// real create/update-add job. The result is input-side only, not a compressed
/// output-size guess.
#[tauri::command]
pub async fn estimate_create_inputs(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    inputs: Vec<String>,
    excludes: Vec<String>,
) -> Result<CreateEstimateDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let inputs: Vec<PathBuf> = inputs.into_iter().map(PathBuf::from).collect();
    tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = Instant::now() - PREFLIGHT_PROGRESS_INTERVAL;
        let app_progress = app.clone();
        state
            .engine
            .estimate_create_inputs_with_progress(&inputs, &excludes, |scanned, current| {
                if scanned == 1
                    || scanned % 128 == 0
                    || last_emit.elapsed() >= PREFLIGHT_PROGRESS_INTERVAL
                {
                    last_emit = Instant::now();
                    let _ = app_progress.emit(
                        "create://preflight",
                        json!({
                            "phase": "scanning",
                            "scanned": scanned,
                            "current": current,
                        }),
                    );
                }
            })
            .inspect(|estimate| {
                let _ = app.emit(
                    "create://preflight",
                    json!({
                        "phase": "done",
                        "scanned": estimate.entries,
                        "current": "",
                    }),
                );
            })
            .map(CreateEstimateDto::from)
            .map_err(ErrorDto::from)
    })
    .await
    .map_err(|e| ErrorDto::other(format!("input estimate task failed: {e}")))?
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
    limit.map_or(DEFAULT_OPERATION_AUDIT_LIMIT, |limit| limit)
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

fn preview_nested_archive_impl(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    limit: usize,
) -> Result<NestedArchivePreviewDto, FormatError> {
    let temp = extract_nested_archive_to_temp(state, outer_path, entry_path, password, encoding)?;
    let entries = state.engine.list(&temp, &OpenOptions::default());
    let _ = std::fs::remove_file(&temp);
    let entries = entries?;
    let entry_count = entries.len();
    let items = entries
        .iter()
        .take(limit)
        .map(|meta| {
            let normalized = normalized_entry_path(meta);
            EntryDto::from_meta(meta, normalized.clone(), entry_base_name(&normalized))
        })
        .collect();
    Ok(NestedArchivePreviewDto {
        outer_path: outer_path.to_string_lossy().into_owned(),
        entry_path: entry_path.to_owned(),
        format: format_label_from_name(state, entry_path),
        entry_count,
        truncated: entry_count > limit,
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

fn preview_image_mime(entry_path: &str) -> Option<&'static str> {
    let ext = Path::new(entry_path)
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn inline_preview_data_url(
    temp_path: &Path,
    entry_path: &str,
    size: u64,
) -> Option<(String, String)> {
    let mime = preview_image_mime(entry_path)?;
    if size > INLINE_PREVIEW_MAX_BYTES {
        return None;
    }
    let bytes = fs::read(temp_path).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some((mime.to_owned(), format!("data:{mime};base64,{encoded}")))
}

fn preview_temp_path(path: &str) -> Result<PathBuf, ErrorDto> {
    let path = PathBuf::from(path);
    let canonical = fs::canonicalize(&path)
        .map_err(|_| ErrorDto::other("preview file is no longer available"))?;
    let temp_dir = fs::canonicalize(std::env::temp_dir())
        .map_err(|e| ErrorDto::other(format!("cannot resolve temp directory: {e}")))?;
    let generated_preview = generated_preview_file_name(&canonical);
    if !generated_preview || !canonical.starts_with(temp_dir) {
        return Err(ErrorDto::other("invalid preview file path"));
    }
    Ok(canonical)
}

fn generated_preview_file_name(path: &Path) -> bool {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name.starts_with("squallz-nested-"),
        None => false,
    }
}

fn preview_trace_file_name(path: &Path) -> String {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => "preview-file".to_owned(),
    }
}

fn preview_trace_extension(path: &Path) -> Option<String> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => Some(ext.to_ascii_lowercase()),
        _ => None,
    }
}

fn preview_trace_temp_scoped(path: &Path) -> bool {
    match fs::canonicalize(std::env::temp_dir()) {
        Ok(temp_dir) => match fs::canonicalize(path) {
            Ok(candidate) => candidate.starts_with(temp_dir),
            Err(_) => path.starts_with(temp_dir),
        },
        Err(_) => false,
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
        "file_name": preview_trace_file_name(path),
        "extension": preview_trace_extension(path),
        "generated_preview": generated_preview_file_name(path),
        "temp_scoped": preview_trace_temp_scoped(path),
        "error": error,
    })
}

fn trace_preview_opener(action: &str, status: &str, path: &Path, error: Option<&str>) {
    validation_trace::trace(
        &format!("preview.{action}.{status}"),
        preview_trace_payload(action, status, path, error),
    );
}

fn run_system_command(command: &mut Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "system command exited with {status}"
        )))
    }
}

#[cfg(target_os = "macos")]
fn open_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("open").arg(path))
}

#[cfg(target_os = "windows")]
fn open_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("cmd").args(["/C", "start", ""]).arg(path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("xdg-open").arg(path))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn open_path_with_system(_path: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "opening preview is not supported on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn reveal_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("open").arg("-R").arg(path))
}

#[cfg(target_os = "windows")]
fn reveal_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("explorer").arg(format!("/select,{}", path.display())))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_path_with_system(path: &Path) -> io::Result<()> {
    run_system_command(Command::new("xdg-open").arg(parent_or_current(path)))
}

#[cfg(not(any(unix, target_os = "windows")))]
fn reveal_path_with_system(_path: &Path) -> io::Result<()> {
    Err(io::Error::other(
        "revealing preview is not supported on this platform",
    ))
}

#[tauri::command]
pub fn open_preview_path(path: String) -> Result<(), ErrorDto> {
    let path = preview_temp_path(&path)?;
    trace_preview_opener("open", "request", &path, None);
    match open_path_with_system(&path) {
        Ok(()) => {
            trace_preview_opener("open", "ok", &path, None);
            Ok(())
        }
        Err(e) => {
            let message = e.to_string();
            trace_preview_opener("open", "err", &path, Some(&message));
            Err(ErrorDto::other(format!("open preview failed: {message}")))
        }
    }
}

#[tauri::command]
pub fn reveal_preview_path(path: String) -> Result<(), ErrorDto> {
    let path = preview_temp_path(&path)?;
    trace_preview_opener("reveal", "request", &path, None);
    match reveal_path_with_system(&path) {
        Ok(()) => {
            trace_preview_opener("reveal", "ok", &path, None);
            Ok(())
        }
        Err(e) => {
            let message = e.to_string();
            trace_preview_opener("reveal", "err", &path, Some(&message));
            Err(ErrorDto::other(format!("reveal preview failed: {message}")))
        }
    }
}

fn preview_archive_entry_impl(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<EntryPreviewDto, FormatError> {
    let temp = extract_nested_archive_to_temp(state, outer_path, entry_path, password, encoding)?;
    let size = metadata_len_or_zero(&temp);
    let inline_preview = inline_preview_data_url(&temp, entry_path, size);
    Ok(EntryPreviewDto {
        outer_path: outer_path.to_string_lossy().into_owned(),
        entry_path: entry_path.to_owned(),
        display_name: entry_base_name(entry_path),
        temp_path: temp.to_string_lossy().into_owned(),
        size,
        archive_like: entry_is_archive_like(state, entry_path),
        preview_mime: inline_preview.as_ref().map(|(mime, _)| mime.clone()),
        preview_data_url: inline_preview.map(|(_, data_url)| data_url),
    })
}

fn parent_or_current(path: &Path) -> &Path {
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn operation_history_file_name(path: &Path) -> &str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => "operation-history.json",
    }
}

fn metadata_len_or_zero(path: &Path) -> u64 {
    match fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) => {
            log::debug!(
                "entry preview: cannot read metadata for {}: {e}",
                path.display()
            );
            0
        }
    }
}

/// Reads an archive entry as another archive and returns its first rows.
#[tauri::command]
pub async fn preview_nested_archive(
    state: State<'_, Arc<AppState>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<NestedArchivePreviewDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || {
        preview_nested_archive_impl(
            &state,
            Path::new(&outer_path),
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
            200,
        )
        .map_err(ErrorDto::from)
    })
    .await
    .map_err(|e| ErrorDto::other(format!("nested preview task failed: {e}")))?
}

/// Extracts one archive entry to a temporary file for local preview/reveal.
#[tauri::command]
pub async fn preview_archive_entry(
    state: State<'_, Arc<AppState>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<EntryPreviewDto, ErrorDto> {
    let state = Arc::clone(state.inner());
    let trace_outer = outer_path.clone();
    let trace_entry = entry_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = preview_archive_entry_impl(
            &state,
            Path::new(&outer_path),
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
        );
        match &result {
            Ok(preview) => {
                let temp_path = Path::new(&preview.temp_path);
                validation_trace::trace(
                    "preview_archive_entry.ok",
                    json!({
                        "outer_path": trace_outer,
                        "entry_path": trace_entry,
                        "temp_file_name": preview_trace_file_name(temp_path),
                        "temp_extension": preview_trace_extension(temp_path),
                        "temp_generated_preview": generated_preview_file_name(temp_path),
                        "temp_scoped": preview_trace_temp_scoped(temp_path),
                        "size": preview.size,
                        "archive_like": preview.archive_like,
                        "inline_preview": preview.preview_data_url.is_some(),
                        "preview_mime": preview.preview_mime.as_deref(),
                    }),
                );
            }
            Err(e) => validation_trace::trace(
                "preview_archive_entry.err",
                json!({
                    "outer_path": trace_outer,
                    "entry_path": trace_entry,
                    "error": ErrorDto::from(e).key,
                }),
            ),
        }
        result.map_err(ErrorDto::from)
    })
    .await
    .map_err(|e| ErrorDto::other(format!("entry preview task failed: {e}")))?
}

fn open_nested_archive_impl(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<ArchiveInfo, FormatError> {
    let temp = extract_nested_archive_to_temp(state, outer_path, entry_path, password, encoding)?;
    match state.open_archive(&temp, None, None) {
        Ok(mut info) => {
            info.name = entry_base_name(entry_path);
            Ok(info)
        }
        Err(e) => {
            let _ = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Extracts an archive entry to a persistent temp file and opens it as the
/// active browse archive.
#[tauri::command]
pub async fn open_nested_archive(
    state: State<'_, Arc<AppState>>,
    outer_path: String,
    entry_path: String,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<ArchiveInfo, ErrorDto> {
    let state = Arc::clone(state.inner());
    let trace_outer = outer_path.clone();
    let trace_entry = entry_path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = open_nested_archive_impl(
            &state,
            Path::new(&outer_path),
            &entry_path,
            password.as_deref(),
            encoding.as_deref(),
        );
        match &result {
            Ok(info) => validation_trace::trace(
                "open_nested_archive.ok",
                json!({
                    "outer_path": trace_outer,
                    "entry_path": trace_entry,
                    "path": info.path,
                    "format": info.format,
                    "entry_count": info.entry_count,
                }),
            ),
            Err(e) => validation_trace::trace(
                "open_nested_archive.err",
                json!({
                    "outer_path": trace_outer,
                    "entry_path": trace_entry,
                    "error": ErrorDto::from(e).key,
                }),
            ),
        }
        result.map_err(ErrorDto::from)
    })
    .await
    .map_err(|e| ErrorDto::other(format!("nested open task failed: {e}")))?
}

/// Submits a job to the queue; progress/state arrive as `job://*` events.
#[tauri::command]
pub fn submit_job(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    jobs: State<'_, Arc<JobManager>>,
    settings: State<'_, Arc<SettingsStore>>,
    spec: JobSpec,
) -> u64 {
    jobs.submit(
        Arc::clone(state.inner()),
        Arc::new(TauriEvents(app)),
        spec,
        settings.get(),
    )
}

/// Pauses a job at its next chunk boundary.
#[tauri::command]
pub fn pause_job(app: AppHandle, jobs: State<'_, Arc<JobManager>>, id: u64) {
    jobs.pause(&TauriEvents(app), id);
}

/// Resumes a paused job.
#[tauri::command]
pub fn resume_job(app: AppHandle, jobs: State<'_, Arc<JobManager>>, id: u64) {
    jobs.resume(&TauriEvents(app), id);
}

/// Cancels a queued or running job.
#[tauri::command]
pub fn cancel_job(app: AppHandle, jobs: State<'_, Arc<JobManager>>, id: u64) {
    jobs.cancel(&TauriEvents(app), id);
}

/// Answers a `job://ask-conflict` prompt.
#[tauri::command]
pub fn answer_conflict(
    jobs: State<'_, Arc<JobManager>>,
    id: u64,
    decision: String,
    apply_all: bool,
) {
    jobs.bridge.answer(
        id,
        AskAnswer::Conflict {
            decision,
            apply_all,
        },
    );
}

/// Answers a `job://ask-password` prompt (`None` = the user cancelled).
#[tauri::command]
pub fn answer_password(jobs: State<'_, Arc<JobManager>>, id: u64, password: Option<String>) {
    jobs.bridge.answer(id, AskAnswer::Password(password));
}

/// Persistent password-book status for one archive path.
#[tauri::command]
pub fn archive_password_status(
    secrets: State<'_, SharedSecretStore>,
    path: String,
) -> PasswordBookStatusDto {
    archive_password_status_impl(secrets.as_ref(), Path::new(&path))
}

/// Verifies and saves the current archive password in the platform store.
#[tauri::command]
pub fn remember_archive_password(
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    path: String,
    password: String,
    encoding: Option<String>,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    remember_archive_password_impl(
        state.as_ref(),
        secrets.as_ref(),
        Path::new(&path),
        &password,
        encoding.as_deref(),
    )
}

/// Forgets the current archive password from both Keychain and session cache.
#[tauri::command]
pub fn forget_archive_password(
    state: State<'_, Arc<AppState>>,
    secrets: State<'_, SharedSecretStore>,
    path: String,
) -> Result<PasswordBookStatusDto, ErrorDto> {
    forget_archive_password_impl(state.as_ref(), secrets.as_ref(), Path::new(&path))
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

/// Persists the theme (`system` / `light` / `dark`).
#[tauri::command]
pub fn set_theme(settings: State<'_, Arc<SettingsStore>>, theme: String) -> SettingsDto {
    settings.update(|s| s.theme = Some(theme))
}

/// Persists the language (`None` = follow the system).
#[tauri::command]
pub fn set_language(
    settings: State<'_, Arc<SettingsStore>>,
    language: Option<String>,
) -> SettingsDto {
    settings.update(|s| s.language = language)
}

fn apply_general_options(
    settings: &mut SettingsDto,
    language: Option<String>,
    default_extract_dir: Option<String>,
    reveal_after_extract: bool,
) {
    settings.language = language;
    settings.default_extract_dir = default_extract_dir
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    settings.reveal_after_extract = reveal_after_extract;
}

/// Persists General settings that belong to the desktop shell.
#[tauri::command]
pub fn set_general_options(
    settings: State<'_, Arc<SettingsStore>>,
    language: Option<String>,
    default_extract_dir: Option<String>,
    reveal_after_extract: bool,
) -> SettingsDto {
    settings
        .update(|s| apply_general_options(s, language, default_extract_dir, reveal_after_extract))
}

/// Persists the UI mode (`modern` / `classic`).
#[tauri::command]
pub fn set_ui_mode(settings: State<'_, Arc<SettingsStore>>, ui_mode: String) -> SettingsDto {
    settings.update(|s| s.ui_mode = Some(ui_mode))
}

/// Persists the desktop UI density (`compact` / `standard` / `comfort`).
#[tauri::command]
pub fn set_ui_density(settings: State<'_, Arc<SettingsStore>>, ui_density: String) -> SettingsDto {
    settings.update(|s| {
        s.ui_density = Some(if valid_ui_density(&ui_density) {
            ui_density
        } else {
            "standard".to_owned()
        });
    })
}

/// Persists Appearance / Colors palette settings.
#[tauri::command]
pub fn set_accent_palette(
    settings: State<'_, Arc<SettingsStore>>,
    accent_palette: String,
    custom_accent: Option<String>,
    accent_contrast_guard: Option<bool>,
) -> SettingsDto {
    settings
        .update(|s| apply_accent_palette(s, accent_palette, custom_accent, accent_contrast_guard))
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
) -> SettingsDto {
    settings.update(|s| {
        s.safety_max_output_bytes = max_output_bytes.filter(|v| *v > 0);
        s.safety_max_entries = max_entries.filter(|v| *v > 0);
        s.safety_max_compression_ratio = max_compression_ratio.filter(|v| *v > 0);
    })
}

/// Persists compression resource settings. `None` restores automatic resource choices.
#[tauri::command]
pub fn set_performance_options(
    settings: State<'_, Arc<SettingsStore>>,
    threads: Option<usize>,
    memory_limit_bytes: Option<u64>,
) -> SettingsDto {
    settings.update(|s| {
        s.performance_threads = threads.filter(|v| *v > 0).map(|v| v.min(64));
        s.performance_memory_limit_bytes = memory_limit_bytes.filter(|v| *v > 0);
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use base64::Engine as _;
    use squallz_core::api::{
        CompressionLevel, ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress,
        OpenOptions, Password,
    };
    use squallz_core::Engine;

    use super::{
        apply_accent_palette, apply_general_options, archive_password_status_impl,
        disk_space_preflight, export_operation_history_impl, forget_archive_password_impl,
        format_label_from_name, inline_preview_data_url, open_archive_resolving_password,
        open_nested_archive_impl, preview_archive_entry_impl, preview_nested_archive_impl,
        preview_trace_payload, remember_archive_password_impl, valid_accent_palette,
        valid_hex_color, INLINE_PREVIEW_MAX_BYTES,
    };
    use crate::dto::SettingsDto;
    use crate::secrets::{tests::MemorySecretStore, SecretStore};
    use crate::state::{AppState, DEFAULT_PAGE_SIZE};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-command-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
    fn preview_opener_trace_payload_redacts_full_temp_path() {
        let dir = temp_dir("preview-opener-trace");
        let path = dir.join("squallz-nested-safe-preview.pdf");
        std::fs::write(&path, b"preview").unwrap();

        let payload = preview_trace_payload("open", "request", &path, None);
        let serialized = payload.to_string();
        let dir_text = dir.to_string_lossy();

        assert_eq!(payload["action"], "open");
        assert_eq!(payload["status"], "request");
        assert_eq!(payload["file_name"], "squallz-nested-safe-preview.pdf");
        assert_eq!(payload["extension"], "pdf");
        assert_eq!(payload["generated_preview"], true);
        assert_eq!(payload["temp_scoped"], true);
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
    fn general_options_update_language_default_dir_and_reveal_preference() {
        let mut settings = SettingsDto::default();

        apply_general_options(
            &mut settings,
            Some("zh-CN".to_owned()),
            Some("  /tmp/Squallz Extracts  ".to_owned()),
            true,
        );
        assert_eq!(settings.language.as_deref(), Some("zh-CN"));
        assert_eq!(
            settings.default_extract_dir.as_deref(),
            Some("/tmp/Squallz Extracts")
        );
        assert!(settings.reveal_after_extract);

        apply_general_options(&mut settings, None, Some("  ".to_owned()), false);
        assert_eq!(settings.language, None);
        assert_eq!(settings.default_extract_dir, None);
        assert!(!settings.reveal_after_extract);
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

    #[test]
    fn nested_archive_preview_lists_inner_entries() {
        let dir = temp_dir("nested-preview");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);

        let preview =
            preview_nested_archive_impl(&state, &outer, "inner.zip", None, None, 20).unwrap();
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

        let preview =
            preview_archive_entry_impl(&state, &archive, "src/note.txt", None, None).unwrap();
        assert_eq!(preview.entry_path, "src/note.txt");
        assert_eq!(preview.display_name, "note.txt");
        assert_eq!(preview.size, 10);
        assert!(!preview.archive_like);
        assert_eq!(std::fs::read(&preview.temp_path).unwrap(), b"preview me");

        let _ = std::fs::remove_file(preview.temp_path);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn image_entry_preview_returns_inline_data_url() {
        let dir = temp_dir("entry-preview-image");
        let state = AppState::new();
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=")
            .unwrap();
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

        let preview =
            preview_archive_entry_impl(&state, &archive, "src/pixel.png", None, None).unwrap();
        assert_eq!(preview.preview_mime.as_deref(), Some("image/png"));
        assert!(preview
            .preview_data_url
            .as_deref()
            .unwrap_or_default()
            .starts_with("data:image/png;base64,"));

        let _ = std::fs::remove_file(preview.temp_path);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn image_preview_inline_latency_and_large_boundary() {
        let dir = temp_dir("entry-preview-image-latency");
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=")
            .unwrap();
        let small = dir.join("pixel.png");
        std::fs::write(&small, &png).unwrap();

        let small_start = Instant::now();
        let (_, data_url) = inline_preview_data_url(&small, "pixel.png", png.len() as u64)
            .expect("tiny png should inline");
        let small_ms = small_start.elapsed().as_millis();
        println!("PREVIEW_METRIC image_preview_inline_data_url_ms={small_ms}");
        println!(
            "PREVIEW_METRIC image_preview_small_data_url_bytes={}",
            data_url.len()
        );
        assert!(data_url.starts_with("data:image/png;base64,"));
        assert!(
            small_ms <= 50,
            "tiny image inline preview took {small_ms}ms"
        );

        let large = dir.join("large.png");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(INLINE_PREVIEW_MAX_BYTES + 1)
            .unwrap();
        let large_start = Instant::now();
        let inline = inline_preview_data_url(&large, "large.png", INLINE_PREVIEW_MAX_BYTES + 1);
        let large_ms = large_start.elapsed().as_millis();
        println!("PREVIEW_METRIC image_preview_large_inline_boundary_ms={large_ms}");
        assert!(inline.is_none(), "large images must not be inlined");
        assert!(
            large_ms <= 50,
            "large image inline boundary took {large_ms}ms"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_returns_cached_inner_archive() {
        let dir = temp_dir("nested-open");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);

        let info = open_nested_archive_impl(&state, &outer, "inner.zip", None, None).unwrap();
        assert_eq!(info.format, "zip");
        assert_eq!(info.name, "inner.zip");
        let temp = PathBuf::from(&info.path);
        assert!(
            temp.exists(),
            "nested temp archive should persist while open"
        );

        let page = state
            .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, "", None)
            .unwrap();
        assert!(page.items.iter().any(|entry| entry.path == "inner-src/"));

        let nested_page = state
            .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, "inner-src/", None)
            .unwrap();
        assert!(nested_page
            .items
            .iter()
            .any(|entry| entry.path == "inner-src/hello.txt"));

        state.close_archive(info.id);
        let _ = std::fs::remove_file(temp);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_temp_can_feed_extract() {
        let dir = temp_dir("nested-extract");
        let state = AppState::new();
        let outer = make_nested_zip_archive(&state, &dir);

        let info = open_nested_archive_impl(&state, &outer, "inner.zip", None, None).unwrap();
        let temp = PathBuf::from(&info.path);
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
        assert_eq!(
            std::fs::read(dest.join("inner-src/hello.txt")).unwrap(),
            b"hello nested"
        );

        state.close_archive(info.id);
        let _ = std::fs::remove_file(temp);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_nested_archive_parallel_uses_distinct_temp_files() {
        let dir = temp_dir("nested-parallel");
        let state = std::sync::Arc::new(AppState::new());
        let outer = make_nested_zip_archive(state.as_ref(), &dir);

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let state = std::sync::Arc::clone(&state);
                let outer = outer.clone();
                std::thread::spawn(move || {
                    open_nested_archive_impl(state.as_ref(), &outer, "inner.zip", None, None)
                        .unwrap()
                })
            })
            .collect();
        let infos: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let mut paths = std::collections::HashSet::new();
        for info in &infos {
            assert!(
                paths.insert(info.path.clone()),
                "duplicate nested temp path: {}",
                info.path
            );
            assert!(
                Path::new(&info.path).exists(),
                "nested temp archive should persist while open"
            );
        }

        for info in infos {
            let temp = PathBuf::from(&info.path);
            state.close_archive(info.id);
            let _ = std::fs::remove_file(temp);
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

        let info = open_archive_resolving_password(&state, &secrets, &archive, None, None).unwrap();
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

        let info =
            open_archive_resolving_password(&state, &secrets, &archive, Some("secret"), None)
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

        let initial = archive_password_status_impl(&secrets, &archive);
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
        assert!(archive_password_status_impl(&secrets, &archive).saved);
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
        assert!(!archive_password_status_impl(&secrets, &archive).saved);
        assert!(state.password_for(&archive).is_none());

        std::fs::remove_dir_all(&dir).unwrap();
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
