//! System "open file with Squallz" bridge.
//!
//! macOS sends Finder/open events through Tauri's run loop before the
//! frontend may have drained launch paths or registered a JS listener. This
//! module keeps those paths until the frontend explicitly announces that the
//! realtime listener is installed.

use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, EventTarget, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::validation_trace;

pub const OPEN_FILES_EVENT: &str = "app://open-files";
pub const EXTERNAL_TASK_ACTION_ARG: &str = "--squallz-action";
pub const EXTERNAL_TASK_OUTPUT_ARG: &str = "--squallz-output";
const SFX_EXTERNAL_ACTION: &str = "extract-sfx";
const TASK_WINDOW_LABEL_PREFIX: &str = "task-";
#[cfg(test)]
const TASK_WINDOW_CAPABILITY_PATTERN: &str = "task-*";
const TASK_WINDOW_TITLE: &str = "Squallz Task";
const TASK_WINDOW_INNER_SIZE: (f64, f64) = (780.0, 560.0);
const TASK_WINDOW_MIN_INNER_SIZE: (f64, f64) = (620.0, 420.0);
const TASK_WINDOW_INDEX: &str = "index.html";
const TASK_WINDOW_QUERY_MODE: &str = "taskWindow";
const TASK_WINDOW_QUERY_MODE_VALUE: &str = "1";
const TASK_WINDOW_QUERY_ACTION: &str = "externalTask";
const TASK_WINDOW_QUERY_OUTPUT: &str = "externalOutput";
const TASK_WINDOW_QUERY_PATH: &str = "externalPath";
static TASK_WINDOW_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OpenFilesEvent {
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, PartialEq)]
struct TaskWindowLaunchConfig {
    label: String,
    title: &'static str,
    url: PathBuf,
    inner_size: (f64, f64),
    min_inner_size: (f64, f64),
}

#[derive(Debug, Default)]
struct OpenFileRequestsInner {
    frontend_ready: bool,
    pending: OpenFilesEvent,
}

#[derive(Debug, Default)]
pub struct OpenFileRequests {
    inner: Mutex<OpenFileRequestsInner>,
}

fn lock_open_file_queue<'a>(
    mutex: &'a Mutex<OpenFileRequestsInner>,
    action: &str,
) -> MutexGuard<'a, OpenFileRequestsInner> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::error!("open-file queue: mutex poisoned while {action}; recovering");
            poisoned.into_inner()
        }
    }
}

impl OpenFileRequests {
    /// Adds opened paths. If the frontend has already called `take`, return
    /// an event that should be emitted immediately; otherwise keep them.
    pub fn push(&self, paths: Vec<String>) -> Option<OpenFilesEvent> {
        self.push_event(OpenFilesEvent::from_paths(paths))
    }

    pub fn push_event(&self, event: OpenFilesEvent) -> Option<OpenFilesEvent> {
        let event = event.normalized();
        if event.paths.is_empty() {
            return None;
        }
        validation_trace::trace(
            "open_files.push",
            json!({
                "paths": event.paths.clone(),
                "action": event.action.clone(),
                "output": event.output.clone(),
            }),
        );

        let mut inner = lock_open_file_queue(&self.inner, "pushing paths");
        if inner.frontend_ready {
            Some(event)
        } else {
            inner.pending.merge(event);
            None
        }
    }

    /// Drains pending launch/open paths without switching later events to
    /// realtime delivery. The frontend calls this before it loads the heavier
    /// JS event listener so the first open-file render stays on the short path.
    pub fn drain_pending(&self) -> OpenFilesEvent {
        let mut inner = lock_open_file_queue(&self.inner, "draining pending paths");
        let event = std::mem::take(&mut inner.pending);
        validation_trace::trace(
            "open_files.take",
            json!({
                "paths": event.paths.clone(),
                "action": event.action.clone(),
                "output": event.output.clone(),
            }),
        );
        event
    }

    /// Switches later open-file events to realtime delivery and returns paths
    /// that arrived between launch-path drain and listener installation.
    pub fn mark_listener_ready(&self) -> OpenFilesEvent {
        let mut inner = lock_open_file_queue(&self.inner, "marking listener ready");
        inner.frontend_ready = true;
        let event = std::mem::take(&mut inner.pending);
        validation_trace::trace(
            "open_files.listener_ready",
            json!({
                "paths": event.paths.clone(),
                "action": event.action.clone(),
                "output": event.output.clone(),
            }),
        );
        event
    }
}

impl OpenFilesEvent {
    pub fn from_paths(paths: Vec<String>) -> Self {
        Self {
            paths,
            action: None,
            output: None,
        }
        .normalized()
    }

    fn normalized(mut self) -> Self {
        self.paths = unique_paths(self.paths);
        self.action = clean_option(self.action);
        self.output = clean_option(self.output);
        self
    }

    pub(crate) fn is_external_task(&self) -> bool {
        self.action.is_some() && !self.paths.is_empty()
    }

    fn merge(&mut self, event: OpenFilesEvent) {
        let event = event.normalized();
        self.paths.extend(event.paths);
        self.paths = unique_paths(std::mem::take(&mut self.paths));
        if self.action.is_none() {
            self.action = event.action;
        }
        if self.output.is_none() {
            self.output = event.output;
        }
    }
}

pub fn emit_open_files(app: &AppHandle, event: &OpenFilesEvent) {
    if event.paths.is_empty() {
        return;
    }
    if event.is_external_task() && open_external_task_window(app, event) {
        return;
    }
    focus_main_window(app);
    let target = EventTarget::webview_window("main");
    if let Err(e) = app.emit_to(target, OPEN_FILES_EVENT, event) {
        log::error!("events: emit {OPEN_FILES_EVENT} failed: {e}");
    }
}

pub(crate) fn show_startup_open_event(app: &AppHandle, event: OpenFilesEvent) {
    if event.is_external_task() {
        if open_external_task_window(app, &event) {
            close_main_window(app);
            return;
        }
        log::warn!("external task window unavailable; falling back to main window task mode");
    }
    let has_open_files = !event.paths.is_empty();
    let queue = app.state::<Arc<OpenFileRequests>>();
    if let Some(event) = queue.push_event(event) {
        emit_open_files(app, &event);
    } else if has_open_files {
        focus_main_window(app);
    }
}

pub(crate) fn focus_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        validation_trace::trace("window.focus", json!({ "found": false }));
        return;
    };
    let show = window.show().map_err(|e| e.to_string());
    let unminimize = window.unminimize().map_err(|e| e.to_string());
    let focus = window.set_focus().map_err(|e| e.to_string());
    let app_activation = activate_app();
    validation_trace::trace(
        "window.focus",
        json!({
            "found": true,
            "show_ok": show.is_ok(),
            "unminimize_ok": unminimize.is_ok(),
            "focus_ok": focus.is_ok(),
            "app_activation": app_activation,
            "show_err": show.err(),
            "unminimize_err": unminimize.err(),
            "focus_err": focus.err(),
        }),
    );
}

fn close_main_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        validation_trace::trace("window.main.close", json!({ "found": false }));
        return;
    };
    let close = window.close().map_err(|e| e.to_string());
    validation_trace::trace(
        "window.main.close",
        json!({
            "found": true,
            "close_ok": close.is_ok(),
            "close_err": close.err(),
        }),
    );
}

fn open_external_task_window(app: &AppHandle, event: &OpenFilesEvent) -> bool {
    let config = task_window_launch_config(event, next_task_window_label());
    let window = WebviewWindowBuilder::new(app, config.label.clone(), WebviewUrl::App(config.url))
        .title(config.title)
        .inner_size(config.inner_size.0, config.inner_size.1)
        .min_inner_size(config.min_inner_size.0, config.min_inner_size.1)
        .decorations(true)
        .shadow(true)
        .focused(true)
        .visible(true)
        .center()
        .build();

    match window {
        Ok(_) => {
            validation_trace::trace(
                "window.task.open",
                json!({
                    "label": config.label,
                    "action": event.action.clone(),
                    "paths": event.paths.clone(),
                }),
            );
            true
        }
        Err(e) => {
            validation_trace::trace(
                "window.task.open_err",
                json!({
                    "label": config.label,
                    "error": e.to_string(),
                    "action": event.action.clone(),
                    "paths": event.paths.clone(),
                }),
            );
            false
        }
    }
}

fn next_task_window_label() -> String {
    let sequence = TASK_WINDOW_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{TASK_WINDOW_LABEL_PREFIX}{}-{sequence}",
        std::process::id()
    )
}

fn task_window_launch_config(event: &OpenFilesEvent, label: String) -> TaskWindowLaunchConfig {
    TaskWindowLaunchConfig {
        label,
        title: TASK_WINDOW_TITLE,
        url: external_task_window_path(event),
        inner_size: TASK_WINDOW_INNER_SIZE,
        min_inner_size: TASK_WINDOW_MIN_INNER_SIZE,
    }
}

fn external_task_window_path(event: &OpenFilesEvent) -> PathBuf {
    let mut query = vec![query_pair(
        TASK_WINDOW_QUERY_MODE,
        TASK_WINDOW_QUERY_MODE_VALUE,
    )];
    if let Some(action) = event.action.as_deref() {
        query.push(query_pair(TASK_WINDOW_QUERY_ACTION, action));
    }
    if let Some(output) = event.output.as_deref() {
        query.push(query_pair(TASK_WINDOW_QUERY_OUTPUT, output));
    }
    for path in &event.paths {
        query.push(query_pair(TASK_WINDOW_QUERY_PATH, path));
    }
    PathBuf::from(format!("{TASK_WINDOW_INDEX}?{}", query.join("&")))
}

fn query_pair(key: &str, value: &str) -> String {
    format!("{key}={}", query_component(value))
}

fn query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize]);
                out.push(HEX[(byte & 0x0f) as usize]);
            }
        }
    }
    out
}

const HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
];

#[cfg(target_os = "macos")]
fn activate_app() -> serde_json::Value {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return json!({ "main_thread": false });
    };
    let ns_app = NSApplication::sharedApplication(mtm);
    let hidden_before = ns_app.isHidden();
    let active_before = ns_app.isActive();
    ns_app.unhide(None);
    ns_app.activate();
    #[allow(deprecated)]
    ns_app.activateIgnoringOtherApps(true);

    json!({
        "main_thread": true,
        "hidden_before": hidden_before,
        "active_before": active_before,
        "hidden_after": ns_app.isHidden(),
        "active_after": ns_app.isActive(),
    })
}

#[cfg(not(target_os = "macos"))]
fn activate_app() -> serde_json::Value {
    json!({ "platform": "non-macos" })
}

pub fn event_from_args(args: impl IntoIterator<Item = OsString>) -> OpenFilesEvent {
    let mut paths = Vec::new();
    let mut action = None;
    let mut output = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let display = arg.to_string_lossy();
        if display.starts_with("-psn_") {
            continue;
        }
        if display == EXTERNAL_TASK_ACTION_ARG {
            action = iter
                .next()
                .map(|value| value.to_string_lossy().into_owned());
            continue;
        }
        if let Some(value) = display
            .strip_prefix(EXTERNAL_TASK_ACTION_ARG)
            .and_then(|suffix| suffix.strip_prefix('='))
        {
            action = Some(value.to_owned());
            continue;
        }
        if display == EXTERNAL_TASK_OUTPUT_ARG {
            output = iter
                .next()
                .map(|value| value.to_string_lossy().into_owned());
            continue;
        }
        if let Some(value) = display
            .strip_prefix(EXTERNAL_TASK_OUTPUT_ARG)
            .and_then(|suffix| suffix.strip_prefix('='))
        {
            output = Some(value.to_owned());
            continue;
        }
        if let Some(path) = existing_path_to_string(PathBuf::from(arg)) {
            paths.push(path);
        }
    }
    OpenFilesEvent {
        paths,
        action,
        output,
    }
    .normalized()
}

pub fn startup_event(args: impl IntoIterator<Item = OsString>) -> OpenFilesEvent {
    match std::env::current_exe()
        .ok()
        .and_then(|executable| sfx_event_for_executable(&executable))
    {
        Some(event) => event,
        None => event_from_args(args),
    }
}

fn sfx_event_for_executable(executable: &Path) -> Option<OpenFilesEvent> {
    let bundle = squallz_core::macos_sfx_bundle_for_executable(executable)?;
    let output = squallz_core::default_sfx_extract_destination(bundle.parent()?, &bundle);
    Some(OpenFilesEvent {
        paths: vec![path_to_string(bundle)],
        action: Some(SFX_EXTERNAL_ACTION.to_owned()),
        output: Some(path_to_string(output)),
    })
}

pub fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn existing_path_to_string(path: PathBuf) -> Option<String> {
    if !path.exists() {
        return None;
    }
    Some(path_to_string(canonical_or_original(&path)))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    match path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            log::debug!(
                "open-file path: canonicalize {} failed: {e}",
                path.display()
            );
            path.to_path_buf()
        }
    }
}

fn unique_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn clean_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        canonical_or_original, event_from_args, external_task_window_path, query_component,
        sfx_event_for_executable, OpenFileRequests, OpenFilesEvent, EXTERNAL_TASK_ACTION_ARG,
        EXTERNAL_TASK_OUTPUT_ARG, TASK_WINDOW_CAPABILITY_PATTERN,
    };

    #[test]
    fn queue_holds_paths_until_frontend_is_ready() {
        let queue = OpenFileRequests::default();

        assert_eq!(queue.push(vec!["/tmp/a.zip".into()]), None);
        assert_eq!(
            queue.push(vec!["/tmp/a.zip".into(), "/tmp/b.7z".into()]),
            None
        );

        assert_eq!(
            queue.drain_pending(),
            OpenFilesEvent::from_paths(vec!["/tmp/a.zip".into(), "/tmp/b.7z".into()])
        );
    }

    #[test]
    fn queue_returns_immediate_event_after_frontend_is_ready() {
        let queue = OpenFileRequests::default();

        assert_eq!(queue.mark_listener_ready(), OpenFilesEvent::default());
        assert_eq!(
            queue.push(vec!["/tmp/a.zip".into(), "/tmp/a.zip".into()]),
            Some(OpenFilesEvent::from_paths(vec!["/tmp/a.zip".into()]))
        );
    }

    #[test]
    fn drain_pending_does_not_switch_to_realtime_delivery() {
        let queue = OpenFileRequests::default();

        assert_eq!(queue.push(vec!["/tmp/launch.zip".into()]), None);
        assert_eq!(
            queue.drain_pending(),
            OpenFilesEvent::from_paths(vec!["/tmp/launch.zip".into()])
        );

        assert_eq!(queue.push(vec!["/tmp/between.7z".into()]), None);
        assert_eq!(
            queue.mark_listener_ready(),
            OpenFilesEvent::from_paths(vec!["/tmp/between.7z".into()])
        );
        assert_eq!(
            queue.push(vec!["/tmp/live.sqz".into()]),
            Some(OpenFilesEvent::from_paths(vec!["/tmp/live.sqz".into()]))
        );
    }

    #[test]
    fn queue_recovers_after_poison_and_keeps_open_paths() {
        let queue = Arc::new(OpenFileRequests::default());
        let poison_queue = Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            let _guard = poison_queue.inner.lock().unwrap();
            panic!("poison open-file queue");
        });
        assert!(handle.join().is_err());

        assert_eq!(queue.push(vec!["/tmp/recovered.zip".into()]), None);
        assert_eq!(
            queue.drain_pending(),
            OpenFilesEvent::from_paths(vec!["/tmp/recovered.zip".into()])
        );
    }

    #[test]
    fn queue_preserves_external_action_intent() {
        let queue = OpenFileRequests::default();

        assert_eq!(
            queue.push_event(OpenFilesEvent {
                paths: vec!["/tmp/photos".into()],
                action: Some(" checksum ".into()),
                output: Some(" ".into()),
            }),
            None,
        );

        assert_eq!(
            queue.drain_pending(),
            OpenFilesEvent {
                paths: vec!["/tmp/photos".into()],
                action: Some("checksum".into()),
                output: None,
            }
        );
    }

    #[test]
    fn external_task_detection_requires_action_and_paths() {
        assert!(!OpenFilesEvent::from_paths(vec!["/tmp/a.zip".into()]).is_external_task());
        assert!(!OpenFilesEvent {
            paths: vec![],
            action: Some("checksum".into()),
            output: None,
        }
        .is_external_task());
        assert!(OpenFilesEvent {
            paths: vec!["/tmp/a.zip".into()],
            action: Some("checksum".into()),
            output: None,
        }
        .is_external_task());
    }

    #[test]
    fn embedded_macos_bundle_launches_the_shared_sfx_extract_task() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "squallz-open-files-sfx-{}-{nonce}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let bundle = dir.join("Photos.app");
        let executable = bundle.join("Contents/MacOS/squallz-gui");
        fs::create_dir_all(bundle.join("Contents/Resources/squallz-sfx")).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"stub").unwrap();
        fs::write(
            bundle.join("Contents/Resources/squallz-sfx/manifest.v1"),
            b"manifest",
        )
        .unwrap();

        let event = sfx_event_for_executable(&executable).unwrap();
        assert_eq!(event.action.as_deref(), Some("extract-sfx"));
        assert_eq!(event.paths, vec![bundle.to_string_lossy().into_owned()]);
        assert_eq!(
            event.output.as_deref(),
            Some(dir.join("Photos").to_string_lossy().as_ref())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn embedded_macos_bundle_keeps_special_stems_below_its_parent() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "squallz-open-files-sfx-special-{}-{nonce}",
            std::process::id()
        ));
        let bundle = dir.join("...app");
        let executable = bundle.join("Contents/MacOS/squallz-gui");
        fs::create_dir_all(bundle.join("Contents/Resources/squallz-sfx")).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"stub").unwrap();
        fs::write(
            bundle.join("Contents/Resources/squallz-sfx/manifest.v1"),
            b"manifest",
        )
        .unwrap();

        let event = sfx_event_for_executable(&executable).unwrap();
        assert_eq!(
            event.output.as_deref(),
            Some(dir.join("extracted").to_string_lossy().as_ref())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn external_task_window_path_reuses_frontend_task_window_query() {
        let event = OpenFilesEvent {
            paths: vec!["/tmp/photos/a file.jpg".into(), "/tmp/photos/b.jpg".into()],
            action: Some("checksum".into()),
            output: Some("/tmp/out file.sha256".into()),
        };

        let url = external_task_window_path(&event)
            .to_string_lossy()
            .into_owned();

        assert!(url.starts_with("index.html?taskWindow=1&externalTask=checksum"));
        assert!(url.contains("externalOutput=%2Ftmp%2Fout%20file.sha256"));
        assert!(url.contains("externalPath=%2Ftmp%2Fphotos%2Fa%20file.jpg"));
        assert!(url.contains("externalPath=%2Ftmp%2Fphotos%2Fb.jpg"));
    }

    #[test]
    fn external_task_window_labels_are_unique_per_native_window() {
        let first = super::next_task_window_label();
        let second = super::next_task_window_label();

        assert_ne!(first, second);
        assert!(first.starts_with(super::TASK_WINDOW_LABEL_PREFIX));
        assert!(second.starts_with(super::TASK_WINDOW_LABEL_PREFIX));
    }

    #[test]
    fn external_task_window_has_only_the_desktop_permissions_it_uses() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/task.json")).unwrap();
        let windows = capability["windows"].as_array().unwrap();
        assert!(windows
            .iter()
            .any(|window| window == TASK_WINDOW_CAPABILITY_PATTERN));

        let permissions = capability["permissions"].as_array().unwrap();
        for required in [
            "core:event:allow-listen",
            "core:event:allow-unlisten",
            "core:window:allow-show",
            "core:window:allow-unminimize",
            "core:window:allow-set-focus",
            "core:window:allow-close",
            "opener:allow-open-path",
            "opener:allow-reveal-item-in-dir",
        ] {
            assert!(permissions.iter().any(|permission| permission == required));
        }
        assert!(!permissions.iter().any(|permission| {
            permission
                .as_str()
                .is_some_and(|permission| permission.starts_with("dialog:"))
        }));
        assert!(!permissions
            .iter()
            .any(|permission| permission == "core:window:allow-destroy"));
        assert!(!permissions
            .iter()
            .any(|permission| permission == "core:window:allow-hide"));
    }

    #[test]
    fn external_task_window_path_preserves_every_external_action_handoff() {
        for action in [
            "checksum",
            "extract-here",
            "extract-to-folder",
            "extract-sfx",
            "compress-to-7z",
            "test-archive",
        ] {
            let event = OpenFilesEvent {
                paths: vec![format!("/tmp/{action}.zip")],
                action: Some(action.into()),
                output: None,
            };

            let url = external_task_window_path(&event)
                .to_string_lossy()
                .into_owned();

            assert!(url.starts_with("index.html?taskWindow=1&externalTask="));
            assert!(url.contains(&format!("externalTask={}", query_component(action))));
            assert!(url.contains(&format!(
                "externalPath={}",
                query_component(&format!("/tmp/{action}.zip"))
            )));
            assert!(!url.contains("externalOutput="));
        }
    }

    #[test]
    fn query_component_percent_encodes_paths_without_plus_spaces() {
        assert_eq!(
            query_component("/tmp/a b/c+d?.zip"),
            "%2Ftmp%2Fa%20b%2Fc%2Bd%3F.zip"
        );
    }

    #[test]
    fn startup_args_keep_only_real_paths() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("squallz-open-files-{nonce}"));
        fs::create_dir(&dir).unwrap();
        let archive = dir.join("sample.zip");
        fs::write(&archive, b"zip").unwrap();

        let event = event_from_args([
            OsString::from("-psn_0_123"),
            OsString::from("--ignored"),
            archive.clone().into_os_string(),
        ]);

        assert_eq!(event.action, None);
        assert_eq!(event.output, None);
        assert_eq!(
            event.paths,
            vec![archive
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_args_parse_external_task_intent() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("squallz-open-action-{nonce}"));
        fs::create_dir(&dir).unwrap();
        let target = dir.join("photo.jpg");
        let output = dir.join("photo.7z");
        fs::write(&target, b"photo").unwrap();

        let event = event_from_args([
            OsString::from(format!("{EXTERNAL_TASK_ACTION_ARG}=checksum")),
            OsString::from(EXTERNAL_TASK_OUTPUT_ARG),
            output.clone().into_os_string(),
            target.clone().into_os_string(),
        ]);

        assert_eq!(event.action.as_deref(), Some("checksum"));
        assert_eq!(
            event.output.as_deref(),
            Some(output.to_string_lossy().as_ref())
        );
        assert_eq!(
            event.paths,
            vec![target
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn canonicalize_failure_uses_original_path() {
        let missing = std::env::temp_dir().join("squallz-open-files-missing-path");
        assert_eq!(canonical_or_original(&missing), missing);
    }
}
