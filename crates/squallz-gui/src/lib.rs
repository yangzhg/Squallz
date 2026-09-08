//! Squallz desktop app (Tauri 2). This crate exposes the GUI business
//! modules for tests/benchmarks and keeps the binary entrypoint thin.

mod app_update;
mod audit;
mod bridge;
mod commands;
mod create_preflight;
pub mod dto;
mod events;
mod integration;
mod jobs;
mod nested;
mod open_files;
mod preview_sessions;
mod preview_workspace;
mod secrets;
mod settings;
mod sfx_runtime;
mod source_cleanup_journal;
pub mod state;
mod validation_trace;

use std::{io, sync::Arc, time::Duration};

use audit::OperationAudit;
use create_preflight::PreflightRequests;
use jobs::JobManager;
use open_files::OpenFileRequests;
use preview_sessions::PreviewSessionManager;
use serde::Serialize;
use settings::SettingsStore;
use squallz_core::PresetStore;
use state::AppState;
use tauri::{Emitter, Manager};

const DEFAULT_NATIVE_DROP_DELAY_MS: u64 = 1_500;

pub fn run() {
    validation_trace::mark_process_start();
    let operation_audit = Arc::new(OperationAudit::load());
    let preset_store = match preset_store_path() {
        Ok(path) => Arc::new(PresetStore::new(path)),
        Err(error) => handle_startup_config_error(error),
    };
    let preview_sessions = match PreviewSessionManager::new() {
        Ok(sessions) => Arc::new(sessions),
        Err(_) => {
            eprintln!("private preview workspace is unavailable; preview features are disabled");
            Arc::new(PreviewSessionManager::unavailable())
        }
    };
    let settings = Arc::new(SettingsStore::load());
    let jobs = Arc::new(JobManager::with_audit_and_settings(
        Arc::clone(&operation_audit),
        &settings.get(),
    ));
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(AppState::new()))
        .manage(jobs)
        .manage(operation_audit)
        .manage(Arc::new(OpenFileRequests::default()))
        .manage(Arc::new(PreflightRequests::default()))
        .manage(preview_sessions)
        .manage(settings)
        .manage(preset_store)
        .manage(secrets::system_secret_store())
        .setup(|app| {
            run_validation_integration_gate();
            run_validation_native_drop_gate(app);
            let event = open_files::startup_event(std::env::args_os().skip(1));
            open_files::show_startup_open_event(app.handle(), event);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_update::check_for_updates,
            commands::open_archive,
            commands::cancel_archive_open,
            commands::close_archive,
            commands::record_validation_event,
            commands::is_validation_session,
            commands::platform_kind,
            commands::take_validation_drop_paths,
            commands::list_entries,
            commands::search_entries,
            commands::cancel_archive_search,
            commands::get_formats,
            commands::archive_stem,
            commands::estimate_create_inputs,
            commands::plan_create,
            commands::plan_convert,
            commands::cancel_convert_plan,
            commands::plan_extract,
            commands::cancel_extract_plan,
            commands::check_disk_space,
            commands::unique_create_destination,
            commands::inspect_create_destination,
            commands::cancel_create_destination_inspection,
            commands::create_destination_has_conflict,
            commands::temp_dir,
            commands::export_operation_history,
            commands::get_operation_audit,
            commands::export_operation_audit,
            commands::apply_integration_changes,
            commands::get_integration_status,
            commands::get_system_integration_diagnostics,
            commands::remove_integration_changes,
            commands::preview_nested_archive,
            commands::preview_archive_entry,
            commands::open_preview_session,
            commands::reveal_preview_session,
            commands::release_preview_session,
            commands::open_nested_archive,
            commands::get_sfx_create_capability,
            commands::get_macos_sfx_publisher_status,
            commands::submit_job,
            commands::job_snapshot,
            commands::job_snapshots,
            commands::dismiss_job_snapshots,
            commands::get_source_cleanup_recovery,
            commands::pause_job,
            commands::resume_job,
            commands::move_job_earlier,
            commands::move_job_later,
            commands::move_job_before,
            commands::cancel_job,
            commands::answer_conflict,
            commands::answer_password,
            commands::archive_password_status,
            commands::remember_archive_password,
            commands::forget_archive_password,
            commands::take_open_files,
            commands::open_file_listener_ready,
            commands::get_locale_table,
            commands::list_languages,
            commands::get_settings,
            commands::get_archive_presets,
            commands::save_archive_presets,
            commands::resolve_external_task_job,
            commands::set_theme,
            commands::set_language,
            commands::set_general_options,
            commands::set_ui_mode,
            commands::set_ui_density,
            commands::set_accent_palette,
            commands::set_safety_limits,
            commands::set_performance_options,
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => handle_startup_build_error(error),
    };
    app.run(|app, event| match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => {
            let jobs = app.state::<Arc<JobManager>>();
            let cancelled_jobs = jobs.release_window(&label);
            let preflight = app.state::<Arc<PreflightRequests>>();
            let cancelled_preflight = preflight.release_window(&label);
            validation_trace::trace(
                "window.jobs.release",
                serde_json::json!({
                    "label": label,
                    "cancelled_jobs": cancelled_jobs,
                    "cancelled_preflight": cancelled_preflight,
                }),
            );
            let state = app.state::<Arc<AppState>>();
            let released_archives = state.release_window(&label);
            validation_trace::trace(
                "window.archives.release",
                serde_json::json!({ "released_archives": released_archives }),
            );
            let previews = app.state::<Arc<PreviewSessionManager>>();
            let released_previews = previews.release_window(&label);
            validation_trace::trace(
                "window.previews.release",
                serde_json::json!({ "released_previews": released_previews }),
            );
        }
        tauri::RunEvent::ExitRequested { .. } => {
            let previews = app.state::<Arc<PreviewSessionManager>>();
            previews.begin_shutdown();
            let jobs = app.state::<Arc<JobManager>>();
            let cancelled_jobs = jobs.cancel_all();
            let preflight = app.state::<Arc<PreflightRequests>>();
            let cancelled_preflight = preflight.cancel_all();
            let state = app.state::<Arc<AppState>>();
            state.begin_shutdown();
            validation_trace::trace(
                "app.jobs.cancel_for_exit",
                serde_json::json!({
                    "cancelled_jobs": cancelled_jobs,
                    "cancelled_preflight": cancelled_preflight,
                }),
            );
        }
        tauri::RunEvent::Exit => {
            let jobs = app.state::<Arc<JobManager>>();
            let cancelled_jobs = jobs.cancel_all();
            jobs.wait_idle();
            let preflight = app.state::<Arc<PreflightRequests>>();
            let cancelled_preflight = preflight.cancel_all();
            preflight.wait_idle();
            let state = app.state::<Arc<AppState>>();
            let released_archives = state.shutdown();
            let previews = app.state::<Arc<PreviewSessionManager>>();
            previews.cleanup();
            validation_trace::trace(
                "app.jobs.drained_for_exit",
                serde_json::json!({
                    "cancelled_jobs": cancelled_jobs,
                    "cancelled_preflight": cancelled_preflight,
                    "released_archives": released_archives,
                }),
            );
        }
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
        tauri::RunEvent::Opened { urls } => {
            let paths: Vec<String> = urls
                .into_iter()
                .filter_map(|url| url.to_file_path().ok())
                .map(open_files::path_to_string)
                .collect();
            let has_open_files = !paths.is_empty();
            let queue = app.state::<Arc<OpenFileRequests>>();
            if let Some(event) = queue.push(paths) {
                open_files::emit_open_files(app, &event);
            } else if has_open_files {
                open_files::focus_main_window(app);
            }
        }
        _ => {}
    });
}

fn preset_store_path() -> io::Result<std::path::PathBuf> {
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no private user configuration directory is available",
            )
        })?;
    Ok(base.join("Squallz").join("presets.json"))
}

fn handle_startup_config_error(error: io::Error) -> ! {
    eprintln!("failed to locate the Squallz configuration directory: {error}");
    std::process::exit(1);
}

fn handle_startup_build_error(error: tauri::Error) -> ! {
    eprintln!("failed to build Squallz desktop app: {error}");
    std::process::exit(1);
}

fn validation_json_or_error<T: Serialize>(value: T) -> serde_json::Value {
    match serde_json::to_value(value) {
        Ok(value) => value,
        Err(error) => serde_json::json!({
            "serialization_error": error.to_string(),
        }),
    }
}

fn native_drop_delay_ms(raw: Option<String>) -> u64 {
    match raw.and_then(|value| value.parse::<u64>().ok()) {
        Some(delay_ms) => delay_ms,
        None => DEFAULT_NATIVE_DROP_DELAY_MS,
    }
}

fn run_validation_integration_gate() {
    if std::env::var("SQUALLZ_VALIDATION_INTEGRATION").as_deref() != Ok("1") {
        return;
    }

    match integration::apply_visible_integrations() {
        Ok(result) => {
            validation_trace::trace("integration.apply.ok", validation_json_or_error(result))
        }
        Err(e) => {
            validation_trace::trace(
                "integration.apply.err",
                serde_json::json!({ "error": e.to_string() }),
            );
            return;
        }
    }

    match integration::integration_status() {
        Ok(result) => validation_trace::trace(
            "integration.status.after_apply",
            validation_json_or_error(result),
        ),
        Err(e) => validation_trace::trace(
            "integration.status.err",
            serde_json::json!({
                "phase": "after_apply",
                "error": e.to_string(),
            }),
        ),
    }

    validation_trace::trace(
        "integration.system_diagnostics",
        validation_json_or_error(integration::system_integration_diagnostics()),
    );

    if std::env::var("SQUALLZ_VALIDATION_INTEGRATION_KEEP").as_deref() == Ok("1") {
        validation_trace::trace(
            "integration.keep.ok",
            serde_json::json!({
                "reason": "SQUALLZ_VALIDATION_INTEGRATION_KEEP=1",
            }),
        );
        return;
    }

    match integration::remove_visible_integrations() {
        Ok(result) => {
            validation_trace::trace("integration.remove.ok", validation_json_or_error(result))
        }
        Err(e) => {
            validation_trace::trace(
                "integration.remove.err",
                serde_json::json!({ "error": e.to_string() }),
            );
            return;
        }
    }

    match integration::integration_status() {
        Ok(result) => validation_trace::trace(
            "integration.status.after_remove",
            validation_json_or_error(result),
        ),
        Err(e) => validation_trace::trace(
            "integration.status.err",
            serde_json::json!({
                "phase": "after_remove",
                "error": e.to_string(),
            }),
        ),
    }
}

fn run_validation_native_drop_gate(app: &tauri::App) {
    let Ok(raw_paths) = std::env::var("SQUALLZ_VALIDATION_NATIVE_DROP_PATHS") else {
        return;
    };
    let paths: Vec<String> = raw_paths
        .split('|')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if paths.is_empty() {
        return;
    }
    let delay_ms =
        native_drop_delay_ms(std::env::var("SQUALLZ_VALIDATION_NATIVE_DROP_DELAY_MS").ok());
    let app = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        let position = serde_json::json!({ "x": 420, "y": 280 });
        let drop_payload = serde_json::json!({
            "paths": paths,
            "position": position,
        });
        validation_trace::trace("native_drop.validation.emit", drop_payload.clone());
        if let Err(e) = app.emit_to("main", "tauri://drag-enter", &drop_payload) {
            validation_trace::trace(
                "native_drop.validation.emit_err",
                serde_json::json!({ "event": "tauri://drag-enter", "error": e.to_string() }),
            );
            return;
        }
        let over_payload = serde_json::json!({ "position": position });
        if let Err(e) = app.emit_to("main", "tauri://drag-over", &over_payload) {
            validation_trace::trace(
                "native_drop.validation.emit_err",
                serde_json::json!({ "event": "tauri://drag-over", "error": e.to_string() }),
            );
            return;
        }
        if let Err(e) = app.emit_to("main", "tauri://drag-drop", &drop_payload) {
            validation_trace::trace(
                "native_drop.validation.emit_err",
                serde_json::json!({ "event": "tauri://drag-drop", "error": e.to_string() }),
            );
        }
    });
}
