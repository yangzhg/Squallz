//! Squallz desktop app (Tauri 2). This crate exposes the GUI business
//! modules for tests/benchmarks and keeps the binary entrypoint thin.

mod audit;
mod bridge;
mod commands;
pub mod dto;
mod events;
mod integration;
mod jobs;
mod nested;
mod open_files;
mod secrets;
mod settings;
pub mod state;
mod validation_trace;

use std::{sync::Arc, time::Duration};

use audit::OperationAudit;
use jobs::JobManager;
use open_files::OpenFileRequests;
use serde::Serialize;
use settings::SettingsStore;
use state::AppState;
use tauri::{Emitter, Manager};

const DEFAULT_NATIVE_DROP_DELAY_MS: u64 = 1_500;

pub fn run() {
    validation_trace::mark_process_start();
    let operation_audit = Arc::new(OperationAudit::load());
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(AppState::new()))
        .manage(Arc::new(JobManager::with_audit(Arc::clone(
            &operation_audit,
        ))))
        .manage(operation_audit)
        .manage(Arc::new(OpenFileRequests::default()))
        .manage(Arc::new(SettingsStore::load()))
        .manage(secrets::system_secret_store())
        .setup(|app| {
            run_validation_integration_gate();
            run_validation_native_drop_gate(app);
            let event = open_files::event_from_args(std::env::args_os().skip(1));
            open_files::show_startup_open_event(app.handle(), event);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_archive,
            commands::close_archive,
            commands::record_validation_event,
            commands::is_validation_session,
            commands::platform_kind,
            commands::take_validation_drop_paths,
            commands::list_entries,
            commands::get_formats,
            commands::archive_stem,
            commands::estimate_create_inputs,
            commands::check_disk_space,
            commands::temp_dir,
            commands::export_operation_history,
            commands::get_operation_audit,
            commands::export_operation_audit,
            commands::apply_integration_changes,
            commands::get_integration_status,
            commands::remove_integration_changes,
            commands::preview_nested_archive,
            commands::preview_archive_entry,
            commands::open_preview_path,
            commands::reveal_preview_path,
            commands::open_nested_archive,
            commands::submit_job,
            commands::pause_job,
            commands::resume_job,
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
    app.run(|app, event| {
        #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
        let _ = app;
        match event {
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
        }
    });
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
