//! ZIP update tests: add/delete/rename individually and combined, system
//! `unzip -t` interop, encrypted archives (raw copy without the password),
//! atomicity on failure.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use common::{command_exists, engine, TempDir};
use squallz_core::api::{
    CompressionLevel, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath, FormatError,
    NoProgress, OpenOptions, Password, ProgressPhase, ProgressSink, UpdateOp,
};

/// Builds a base archive with project/a.txt, project/sub/b.txt, project/c.log.
fn base_archive(dir: &Path, password: Option<&str>) -> PathBuf {
    let root = dir.join("project");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("sub/b.txt"), b"bravo").unwrap();
    fs::write(root.join("c.log"), b"log line").unwrap();
    let dest = dir.join("base.zip");
    let opts = CreateOptions {
        password: password.map(Password::new),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[root], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

const LARGE_RAW_COPY_ENTRY: &str = "raw-copy/large.bin";

fn large_stored_archive(dir: &Path) -> PathBuf {
    let root = dir.join("raw-copy");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("large.bin"), vec![b'R'; 2 * 1024 * 1024]).unwrap();
    fs::write(root.join("remove.txt"), b"remove me").unwrap();
    let dest = dir.join("large.zip");
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[root], &options, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn list_names(path: &Path, password: Option<&str>) -> Vec<String> {
    let opts = OpenOptions {
        password: password.map(Password::new),
        encoding_override: None,
    };
    let mut names: Vec<String> = engine()
        .list(path, &opts)
        .unwrap()
        .iter()
        .map(|e: &EntryMeta| e.path.display.clone())
        .collect();
    names.sort();
    names
}

fn run_update(path: &Path, ops: &[UpdateOp], opts: &CreateOptions) -> Result<(), FormatError> {
    engine().update(path, ops, opts, &NoProgress, &ControlToken::new())
}

fn assert_other_contains(err: FormatError, needle: &str) {
    match err {
        FormatError::Other(msg) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected FormatError::Other containing {needle}, got {other:?}"),
    }
}

/// `unzip -t` interop check (skipped when unzip is unavailable).
fn assert_unzip_t(path: &Path) {
    if !command_exists("unzip") {
        eprintln!("skipping unzip -t check: unzip not on PATH");
        return;
    }
    let out = Command::new("unzip").arg("-t").arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "unzip -t failed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

enum MutationPoint {
    RewriteStarted,
    RawCopyStarted,
    EntryRead(String),
}

struct OnceOnProgress<F> {
    point: MutationPoint,
    action: Mutex<Option<F>>,
}

impl<F> OnceOnProgress<F> {
    fn rewrite_started(action: F) -> Self {
        Self {
            point: MutationPoint::RewriteStarted,
            action: Mutex::new(Some(action)),
        }
    }

    fn raw_copy_started(action: F) -> Self {
        Self {
            point: MutationPoint::RawCopyStarted,
            action: Mutex::new(Some(action)),
        }
    }

    fn entry_read(path: &str, action: F) -> Self {
        Self {
            point: MutationPoint::EntryRead(path.to_owned()),
            action: Mutex::new(Some(action)),
        }
    }
}

impl<F: FnOnce() + Send> OnceOnProgress<F> {
    fn run(&self) {
        if let Some(action) = self.action.lock().unwrap().take() {
            action();
        }
    }
}

impl<F: FnOnce() + Send> ProgressSink for OnceOnProgress<F> {
    fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
        if matches!(self.point, MutationPoint::RewriteStarted)
            || (matches!(self.point, MutationPoint::RawCopyStarted) && !current.display.is_empty())
        {
            self.run();
        }
    }

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        _current_total: u64,
    ) {
        if current_done > 0
            && matches!(&self.point, MutationPoint::EntryRead(path) if path == &current.display)
        {
            self.run();
        }
    }
}

const PROCESS_WORKER_ROLE: &str = "SQUALLZ_ZIP_UPDATE_WORKER_ROLE";
const PROCESS_WORKER_ROOT: &str = "SQUALLZ_ZIP_UPDATE_WORKER_ROOT";
const PROCESS_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_OBSERVATION_WINDOW: Duration = Duration::from_millis(750);
const MARKER_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct WorkerProgress {
    archive: PathBuf,
    entered: PathBuf,
    release: Option<PathBuf>,
    required_entry: Option<&'static str>,
    fired: AtomicBool,
}

struct CancelOnScan {
    ctl: Arc<ControlToken>,
    scanned: AtomicU64,
    byte_progress: AtomicBool,
}

struct CancelDuringRawCopy {
    ctl: Arc<ControlToken>,
    fired: AtomicBool,
    observed: Mutex<Option<(u64, u64)>>,
}

struct PauseDuringRawCopy {
    ctl: Arc<ControlToken>,
    fired: AtomicBool,
    events: Arc<AtomicU64>,
    reached: mpsc::SyncSender<(u64, u64)>,
}

struct CancelOnRewriteComplete {
    ctl: Arc<ControlToken>,
    fired: AtomicBool,
}

struct ResumeOnDrop(Arc<ControlToken>);

impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        self.0.resume();
    }
}

#[derive(Default)]
struct PhaseTrace {
    phases: Mutex<Vec<(ProgressPhase, bool)>>,
    byte_events: Mutex<Vec<(ProgressPhase, u64, u64)>>,
}

impl ProgressSink for PhaseTrace {
    fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
        let phase = self.phases.lock().unwrap().last().map(|event| event.0);
        if let Some(phase) = phase {
            self.byte_events.lock().unwrap().push((phase, done, total));
        }
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.phases.lock().unwrap().push((phase, interruptible));
    }
}

impl ProgressSink for CancelOnScan {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        self.byte_progress.store(true, Ordering::SeqCst);
    }

    fn on_scan_progress(&self, entries: u64, _current: &EntryPath) {
        self.scanned.store(entries, Ordering::SeqCst);
        if entries == 2 {
            self.ctl.cancel();
        }
    }
}

impl ProgressSink for CancelDuringRawCopy {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        if current.display == LARGE_RAW_COPY_ENTRY
            && current_done > 0
            && current_done < current_total
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            *self.observed.lock().unwrap() = Some((current_done, current_total));
            self.ctl.cancel();
        }
    }
}

impl ProgressSink for PauseDuringRawCopy {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        if current.display == LARGE_RAW_COPY_ENTRY && current_done > 0 {
            self.events.fetch_add(1, Ordering::SeqCst);
            if current_done < current_total && !self.fired.swap(true, Ordering::SeqCst) {
                self.ctl.pause();
                self.reached.send((current_done, current_total)).unwrap();
            }
        }
    }
}

impl ProgressSink for CancelOnRewriteComplete {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        if total > 0
            && done == total
            && current.display.is_empty()
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            self.ctl.cancel();
        }
    }
}

impl ProgressSink for WorkerProgress {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(required_entry) = self.required_entry {
            let names = list_names(&self.archive, None);
            assert!(
                names.iter().any(|name| name == required_entry),
                "second update did not start from the first update result: {names:?}"
            );
        }
        fs::write(&self.entered, b"entered").unwrap();
        if let Some(release) = &self.release {
            wait_for_marker(release, PROCESS_WAIT_TIMEOUT);
        }
    }
}

struct ChildGuard {
    child: Option<Child>,
    role: &'static str,
}

impl ChildGuard {
    fn assert_success(mut self, timeout: Duration) {
        let started = Instant::now();
        loop {
            let status = self.child.as_mut().unwrap().try_wait().unwrap();
            if let Some(status) = status {
                self.child = None;
                assert!(
                    status.success(),
                    "{} update worker failed: {status}",
                    self.role
                );
                return;
            }
            if started.elapsed() >= timeout {
                let child = self.child.as_mut().unwrap();
                let _ = child.kill();
                let _ = child.wait();
                self.child = None;
                panic!(
                    "{} update worker did not exit within {timeout:?}",
                    self.role
                );
            }
            std::thread::sleep(MARKER_POLL_INTERVAL);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn worker_marker(root: &Path, role: &str, state: &str) -> PathBuf {
    root.join(format!("{role}-{state}.marker"))
}

fn wait_for_marker(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while !path.is_file() {
        assert!(
            started.elapsed() < timeout,
            "marker did not appear within {timeout:?}: {}",
            path.display()
        );
        std::thread::sleep(MARKER_POLL_INTERVAL);
    }
}

fn assert_marker_stays_absent(path: &Path, duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        assert!(
            !path.exists(),
            "second update entered rewrite while the first process held the target lock"
        );
        std::thread::sleep(MARKER_POLL_INTERVAL);
    }
    assert!(
        !path.exists(),
        "second update entered rewrite while the first process held the target lock"
    );
}

fn spawn_update_worker(root: &Path, role: &'static str) -> ChildGuard {
    let child = Command::new(std::env::current_exe().unwrap())
        .arg("zip_update_process_worker")
        .arg("--exact")
        .arg("--nocapture")
        .env(PROCESS_WORKER_ROLE, role)
        .env(PROCESS_WORKER_ROOT, root)
        .spawn()
        .unwrap();
    ChildGuard {
        child: Some(child),
        role,
    }
}

fn assert_update_source_change(error: FormatError, archive: &Path, original_archive: &[u8]) {
    assert!(matches!(
        error,
        FormatError::Io(ref error) if error.kind() == std::io::ErrorKind::InvalidData
    ));
    assert_eq!(fs::read(archive).unwrap(), original_archive);
    assert_no_update_temp(archive.parent().unwrap());
}

fn assert_no_update_temp(parent: &Path) {
    let legacy = fs::read_dir(parent).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains(".sqz-update-") && name.ends_with(".tmp")
    });
    assert!(!legacy, "legacy ZIP update temporary file remains");
    let artifacts = update_transaction_artifacts(parent);
    assert!(
        artifacts.is_empty(),
        "ZIP update transaction artifacts remain: {artifacts:?}"
    );
}

fn legacy_update_temp_path(archive: &Path) -> PathBuf {
    let name = archive.file_name().unwrap().to_string_lossy();
    archive.with_file_name(format!(".{name}.sqz-update-{}.tmp", std::process::id()))
}

fn update_transaction_artifacts(parent: &Path) -> Vec<String> {
    let mut artifacts: Vec<String> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            name.starts_with(".squallz-update-").then_some(name)
        })
        .collect();
    artifacts.sort();
    artifacts
}

#[test]
fn update_add_file_and_directory() {
    let tmp = TempDir::new("update-add");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"newcomer").unwrap();
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(&extra_dir).unwrap();
    fs::write(extra_dir.join("inner.txt"), b"inside").unwrap();

    let ops = vec![
        UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("new.txt"),
        },
        UpdateOp::Add {
            src: extra_dir,
            dest: EntryPath::from_utf8("extra"),
        },
    ];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"new.txt".to_string()));
    assert!(names.contains(&"extra/inner.txt".to_string()));
    assert!(names.iter().any(|n| n.starts_with("project/a.txt")));
    assert_unzip_t(&archive);
}

#[test]
fn engine_update_reports_ordered_monotonic_phases() {
    let tmp = TempDir::new("update-progress-phases");
    let archive = base_archive(tmp.path(), None);
    let progress = PhaseTrace::default();

    engine()
        .update(
            &archive,
            &[UpdateOp::Rename {
                from: EntryPath::from_utf8("project/a.txt"),
                to: EntryPath::from_utf8("project/renamed.txt"),
            }],
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(
        *progress.phases.lock().unwrap(),
        vec![
            (ProgressPhase::UpdateRewrite, true),
            (ProgressPhase::UpdateVerify, true),
            (ProgressPhase::UpdateCommit, false),
            (ProgressPhase::UpdateCleanup, false),
        ]
    );
    let events = progress.byte_events.lock().unwrap();
    for phase in [ProgressPhase::UpdateRewrite, ProgressPhase::UpdateVerify] {
        let phase_events: Vec<(u64, u64)> = events
            .iter()
            .filter_map(|(event_phase, done, total)| {
                (*event_phase == phase).then_some((*done, *total))
            })
            .collect();
        assert!(!phase_events.is_empty());
        let total = phase_events[0].1;
        assert!(total > 0);
        assert!(phase_events.iter().all(|event| event.1 == total));
        assert!(phase_events.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(phase_events.last().map(|event| event.0), Some(total));
    }
    assert_no_update_temp(archive.parent().unwrap());
}

#[test]
fn rewrite_progress_excludes_deleted_entry_bytes() {
    let tmp = TempDir::new("update-delete-progress-total");
    let archive = base_archive(tmp.path(), None);
    let mut source = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut expected_total = 0u64;
    for index in 0..source.len() {
        let entry = source.by_index_raw(index).unwrap();
        if entry.name().trim_end_matches('/') != "project/a.txt" {
            expected_total = expected_total.saturating_add(entry.compressed_size());
        }
    }
    drop(source);
    let progress = PhaseTrace::default();

    engine()
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "project/a.txt".into(),
            }],
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    let events = progress.byte_events.lock().unwrap();
    let rewrite_events: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|(phase, done, total)| {
            (*phase == ProgressPhase::UpdateRewrite).then_some((*done, *total))
        })
        .collect();
    assert!(!rewrite_events.is_empty());
    assert!(rewrite_events
        .iter()
        .all(|(_, total)| *total == expected_total));
    assert_eq!(
        rewrite_events.last().map(|(done, _)| *done),
        Some(expected_total)
    );
    assert!(!list_names(&archive, None).contains(&"project/a.txt".to_owned()));
}

#[test]
fn engine_update_preserves_preexisting_legacy_fixed_temp_file() {
    let tmp = TempDir::new("update-legacy-temp-sentinel");
    let archive = base_archive(tmp.path(), None);
    let legacy_temp = legacy_update_temp_path(&archive);
    let sentinel = b"owned by another process";
    fs::write(&legacy_temp, sentinel).unwrap();

    run_update(
        &archive,
        &[UpdateOp::Delete {
            pattern: "*.log".into(),
        }],
        &CreateOptions::default(),
    )
    .unwrap();

    assert_eq!(fs::read(&legacy_temp).unwrap(), sentinel);
    assert!(!list_names(&archive, None)
        .iter()
        .any(|name| name.ends_with(".log")));
    assert_unzip_t(&archive);
}

#[cfg(unix)]
#[test]
fn engine_update_preserves_legacy_fixed_temp_symlink_and_victim() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new("update-legacy-temp-symlink");
    let archive = base_archive(tmp.path(), None);
    let legacy_temp = legacy_update_temp_path(&archive);
    let victim = tmp.path().join("victim.txt");
    let victim_contents = b"must remain untouched";
    fs::write(&victim, victim_contents).unwrap();
    symlink(&victim, &legacy_temp).unwrap();

    run_update(
        &archive,
        &[UpdateOp::Delete {
            pattern: "*.log".into(),
        }],
        &CreateOptions::default(),
    )
    .unwrap();

    assert!(fs::symlink_metadata(&legacy_temp)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&legacy_temp).unwrap(), victim);
    assert_eq!(fs::read(&victim).unwrap(), victim_contents);
    assert_unzip_t(&archive);
}

#[test]
fn update_rejects_target_rebind_during_rewrite_without_overwriting_competitor() {
    let tmp = TempDir::new("update-target-rebind");
    let archive = base_archive(tmp.path(), None);
    let held_original = tmp.path().join("held-original.zip");
    let competitor = tmp.path().join("competitor.zip");
    let original_archive = fs::read(&archive).unwrap();
    let competitor_contents = b"late competing target";
    fs::write(&competitor, competitor_contents).unwrap();
    let artifacts_before = update_transaction_artifacts(tmp.path());
    let archive_for_action = archive.clone();
    let held_for_action = held_original.clone();
    let competitor_for_action = competitor.clone();
    let rebound = Arc::new(AtomicBool::new(false));
    let rebound_for_action = Arc::clone(&rebound);
    let progress = OnceOnProgress::raw_copy_started(move || {
        fs::rename(&archive_for_action, &held_for_action).unwrap();
        fs::rename(&competitor_for_action, &archive_for_action).unwrap();
        rebound_for_action.store(true, Ordering::SeqCst);
    });

    let error = engine()
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "*.log".into(),
            }],
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(rebound.load(Ordering::SeqCst));
    assert!(matches!(error, FormatError::Io(_)), "{error:?}");
    assert_eq!(fs::read(&held_original).unwrap(), original_archive);
    assert_eq!(fs::read(&archive).unwrap(), competitor_contents);
    assert_eq!(update_transaction_artifacts(tmp.path()), artifacts_before);
}

#[cfg(unix)]
#[test]
fn update_preserves_archive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("update-permissions");
    let archive = base_archive(tmp.path(), None);
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o640)).unwrap();

    run_update(
        &archive,
        &[UpdateOp::Delete {
            pattern: "*.log".into(),
        }],
        &CreateOptions::default(),
    )
    .unwrap();

    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_unzip_t(&archive);
}

#[test]
fn concurrent_updates_are_serialized_against_the_latest_archive() {
    let tmp = TempDir::new("update-concurrent");
    let archive = base_archive(tmp.path(), None);
    let first_source = tmp.path().join("first.txt");
    let second_source = tmp.path().join("second.txt");
    fs::write(&first_source, b"first").unwrap();
    fs::write(&second_source, b"second").unwrap();
    let barrier = Arc::new(Barrier::new(2));

    let workers = [(first_source, "first.txt"), (second_source, "second.txt")]
        .into_iter()
        .map(|(source, destination)| {
            let archive = archive.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                engine().update(
                    &archive,
                    &[UpdateOp::Add {
                        src: source,
                        dest: EntryPath::from_utf8(destination),
                    }],
                    &CreateOptions::default(),
                    &NoProgress,
                    &ControlToken::new(),
                )
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().unwrap().unwrap();
    }

    let names = list_names(&archive, None);
    assert!(names.contains(&"first.txt".to_owned()));
    assert!(names.contains(&"second.txt".to_owned()));
    assert_unzip_t(&archive);
}

#[test]
fn zip_update_process_worker() {
    let Some(role) = std::env::var_os(PROCESS_WORKER_ROLE) else {
        return;
    };
    let role = role.to_str().unwrap();
    let root = PathBuf::from(std::env::var_os(PROCESS_WORKER_ROOT).unwrap());
    let archive = root.join("base.zip");
    let (source, destination, release, required_entry) = match role {
        "first" => (
            root.join("first-source.txt"),
            "first.txt",
            Some(worker_marker(&root, role, "release")),
            None,
        ),
        "second" => (
            root.join("second-source.txt"),
            "second.txt",
            None,
            Some("first.txt"),
        ),
        _ => panic!("unknown ZIP update worker role: {role}"),
    };
    let progress = WorkerProgress {
        archive: archive.clone(),
        entered: worker_marker(&root, role, "entered"),
        release,
        required_entry,
        fired: AtomicBool::new(false),
    };

    fs::write(worker_marker(&root, role, "ready"), b"ready").unwrap();
    engine()
        .update(
            &archive,
            &[UpdateOp::Add {
                src: source,
                dest: EntryPath::from_utf8(destination),
            }],
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();
}

#[test]
fn cross_process_updates_wait_for_the_target_lock_and_use_the_latest_archive() {
    let tmp = TempDir::new("update-cross-process");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("first-source.txt"), b"first").unwrap();
    fs::write(tmp.path().join("second-source.txt"), b"second").unwrap();

    let first = spawn_update_worker(tmp.path(), "first");
    wait_for_marker(
        &worker_marker(tmp.path(), "first", "entered"),
        PROCESS_WAIT_TIMEOUT,
    );

    let second = spawn_update_worker(tmp.path(), "second");
    wait_for_marker(
        &worker_marker(tmp.path(), "second", "ready"),
        PROCESS_WAIT_TIMEOUT,
    );
    let second_entered = worker_marker(tmp.path(), "second", "entered");
    assert_marker_stays_absent(&second_entered, LOCK_OBSERVATION_WINDOW);

    fs::write(worker_marker(tmp.path(), "first", "release"), b"release").unwrap();
    first.assert_success(PROCESS_WAIT_TIMEOUT);
    wait_for_marker(&second_entered, PROCESS_WAIT_TIMEOUT);
    second.assert_success(PROCESS_WAIT_TIMEOUT);

    let names = list_names(&archive, None);
    assert!(names.contains(&"first.txt".to_owned()), "{names:?}");
    assert!(names.contains(&"second.txt".to_owned()), "{names:?}");
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&archive);
}

#[test]
fn update_add_rejects_same_length_source_replacement_and_preserves_archive() {
    let tmp = TempDir::new("update-add-source-replacement");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("source.bin");
    let replacement = tmp.path().join("replacement.bin");
    fs::write(&source, [b'A'; 32]).unwrap();
    fs::write(&replacement, [b'B'; 32]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_for_action = Arc::clone(&changed);
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::remove_file(&source_for_action).unwrap();
        fs::rename(&replacement, &source_for_action).unwrap();
        changed_for_action.store(true, Ordering::SeqCst);
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source.bin"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(changed.load(Ordering::SeqCst));
    assert_update_source_change(error, &archive, &original_archive);
}

#[cfg(unix)]
#[test]
fn update_add_rejects_in_place_rewrite_with_restored_mtime() {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    use std::time::Duration;

    let tmp = TempDir::new("update-add-source-rewrite");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("source.bin");
    fs::write(&source, [b'A'; 32]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let original_metadata = fs::metadata(&source).unwrap();
    let original_modified = original_metadata.modified().unwrap();
    let original_changed = (original_metadata.ctime(), original_metadata.ctime_nsec());
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        let mut changed = original_changed;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&source_for_action)
                .unwrap();
            file.write_all(&[b'B'; 32]).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(original_modified))
                .unwrap();
            drop(file);
            let metadata = fs::metadata(&source_for_action).unwrap();
            changed = (metadata.ctime(), metadata.ctime_nsec());
            if changed != original_changed {
                break;
            }
        }
        assert_ne!(changed, original_changed);
        assert_eq!(
            fs::metadata(&source_for_action)
                .unwrap()
                .modified()
                .unwrap(),
            original_modified
        );
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source.bin"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert_update_source_change(error, &archive, &original_archive);
}

#[cfg(unix)]
#[test]
fn update_add_rechecks_source_path_after_streaming() {
    let tmp = TempDir::new("update-add-source-rebind");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("stream.bin");
    let replacement = tmp.path().join("replacement.bin");
    fs::write(&source, vec![b'A'; 1024 * 1024]).unwrap();
    fs::write(&replacement, vec![b'B'; 1024 * 1024]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_for_action = Arc::clone(&changed);
    let source_for_action = source.clone();
    let progress = OnceOnProgress::entry_read("stream.bin", move || {
        fs::remove_file(&source_for_action).unwrap();
        fs::rename(&replacement, &source_for_action).unwrap();
        changed_for_action.store(true, Ordering::SeqCst);
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("stream.bin"),
    }];
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };

    let error = engine()
        .update(&archive, &ops, &options, &progress, &ControlToken::new())
        .unwrap_err();

    assert!(changed.load(Ordering::SeqCst));
    assert_update_source_change(error, &archive, &original_archive);
}

#[test]
fn update_add_can_cancel_while_streaming_one_file() {
    let tmp = TempDir::new("update-add-cancel-stream");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("stream.bin");
    fs::write(&source, vec![b'A'; 1024 * 1024]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let control_for_action = Arc::clone(&control);
    let progress = OnceOnProgress::entry_read("stream.bin", move || {
        control_for_action.cancel();
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("stream.bin"),
    }];
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };

    let error = engine()
        .update(&archive, &ops, &options, &progress, &control)
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn update_can_cancel_during_unchanged_entry_raw_copy() {
    let tmp = TempDir::new("update-cancel-raw-copy");
    let archive = large_stored_archive(tmp.path());
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let progress = CancelDuringRawCopy {
        ctl: Arc::clone(&control),
        fired: AtomicBool::new(false),
        observed: Mutex::new(None),
    };

    let error = engine()
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "remove.txt".into(),
            }],
            &CreateOptions::default(),
            &progress,
            &control,
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert!(progress.fired.load(Ordering::SeqCst));
    let (current_done, current_total) = progress.observed.lock().unwrap().unwrap();
    assert!(current_done > 0);
    assert!(current_done < current_total);
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn legacy_update_honors_cancel_from_final_rewrite_progress() {
    let tmp = TempDir::new("legacy-update-final-cancel");
    let archive = base_archive(tmp.path(), None);
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let progress = CancelOnRewriteComplete {
        ctl: Arc::clone(&control),
        fired: AtomicBool::new(false),
    };
    let registry = squallz_formats::registry();
    let format = match registry.detect_by_name("base.zip") {
        Some(Detected::Archive(format)) => format,
        _ => panic!("ZIP format is not registered"),
    };

    let error = format
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "*.log".into(),
            }],
            &CreateOptions::default(),
            &progress,
            &control,
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert!(progress.fired.load(Ordering::SeqCst));
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn update_can_pause_and_resume_during_unchanged_entry_raw_copy() {
    let tmp = TempDir::new("update-pause-raw-copy");
    let archive = large_stored_archive(tmp.path());
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let control_for_worker = Arc::clone(&control);
    let archive_for_worker = archive.clone();
    let events = Arc::new(AtomicU64::new(0));
    let events_for_worker = Arc::clone(&events);
    let (reached_tx, reached_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let progress = PauseDuringRawCopy {
            ctl: Arc::clone(&control_for_worker),
            fired: AtomicBool::new(false),
            events: events_for_worker,
            reached: reached_tx,
        };
        let result = engine().update(
            &archive_for_worker,
            &[UpdateOp::Delete {
                pattern: "remove.txt".into(),
            }],
            &CreateOptions::default(),
            &progress,
            &control_for_worker,
        );
        done_tx.send(result).unwrap();
    });
    let resume_on_drop = ResumeOnDrop(Arc::clone(&control));

    let (current_done, current_total) = reached_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let paused = control.is_paused();
    let paused_result = done_rx.recv_timeout(Duration::from_millis(250));
    let still_running = matches!(&paused_result, Err(mpsc::RecvTimeoutError::Timeout));
    let events_while_paused = events.load(Ordering::SeqCst);
    let target_unchanged = fs::read(&archive).unwrap() == original_archive;
    control.resume();
    drop(resume_on_drop);
    let result = match paused_result {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            done_rx.recv_timeout(Duration::from_secs(30)).unwrap()
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("ZIP update worker disconnected while paused")
        }
    };
    worker.join().unwrap();

    assert!(current_done > 0);
    assert!(current_done < current_total);
    assert!(paused);
    assert!(still_running);
    assert_eq!(events_while_paused, 1);
    assert!(events.load(Ordering::SeqCst) > events_while_paused);
    assert!(target_unchanged);
    result.unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&LARGE_RAW_COPY_ENTRY.to_owned()));
    assert!(!names.iter().any(|name| name.ends_with("remove.txt")));
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&archive);
}

#[test]
fn update_add_can_cancel_while_scanning_a_directory() {
    let tmp = TempDir::new("update-add-cancel-scan");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("incoming");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"a").unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let ctl = ControlToken::new();
    let progress = CancelOnScan {
        ctl: Arc::clone(&ctl),
        scanned: AtomicU64::new(0),
        byte_progress: AtomicBool::new(false),
    };
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("incoming"),
    }];

    let error = engine()
        .update(&archive, &ops, &CreateOptions::default(), &progress, &ctl)
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(progress.scanned.load(Ordering::SeqCst), 2);
    assert!(!progress.byte_progress.load(Ordering::SeqCst));
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[cfg(unix)]
#[test]
fn update_add_rejects_symbolic_link_target_change() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new("update-add-link-target");
    let archive = base_archive(tmp.path(), None);
    let first = tmp.path().join("first.txt");
    let second = tmp.path().join("second.txt");
    let source = tmp.path().join("source-link");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    symlink(&first, &source).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::remove_file(&source_for_action).unwrap();
        symlink(&second, &source_for_action).unwrap();
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source-link"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert_update_source_change(error, &archive, &original_archive);
}

#[test]
fn update_add_ignores_directory_members_created_after_preparation() {
    let tmp = TempDir::new("update-add-late-member");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("extra");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("ready.txt"), b"ready").unwrap();
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::write(source_for_action.join("late.txt"), b"late").unwrap();
    });
    let ops = [UpdateOp::Add {
        src: source.clone(),
        dest: EntryPath::from_utf8("extra"),
    }];

    engine()
        .update(
            &archive,
            &ops,
            &CreateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    assert!(source.join("late.txt").is_file());
    let names = list_names(&archive, None);
    assert!(names.contains(&"extra/ready.txt".to_owned()), "{names:?}");
    assert!(!names.contains(&"extra/late.txt".to_owned()), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_add_empty_directory_entry() {
    let tmp = TempDir::new("update-add-dir");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::AddDir {
        path: EntryPath::from_utf8("empty-folder"),
    }];

    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"empty-folder/".to_string()), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_add_directory_applies_create_excludes() {
    let tmp = TempDir::new("update-add-excludes");
    let archive = base_archive(tmp.path(), None);
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(extra_dir.join("node_modules/pkg")).unwrap();
    fs::create_dir_all(extra_dir.join(".git")).unwrap();
    fs::write(extra_dir.join("keep.txt"), b"keep").unwrap();
    fs::write(extra_dir.join("drop.tmp"), b"drop").unwrap();
    fs::write(extra_dir.join("node_modules/pkg/index.js"), b"drop").unwrap();
    fs::write(extra_dir.join(".git/config"), b"drop").unwrap();

    let ops = vec![UpdateOp::Add {
        src: extra_dir,
        dest: EntryPath::from_utf8("extra"),
    }];
    let opts = CreateOptions {
        excludes: vec!["node_modules".into(), ".git".into(), "*.tmp".into()],
        ..CreateOptions::default()
    };
    run_update(&archive, &ops, &opts).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"extra/keep.txt".to_string()));
    assert!(
        !names.iter().any(|n| n.contains("node_modules")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.contains(".git")), "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_delete_by_glob() {
    let tmp = TempDir::new("update-delete");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Delete {
        pattern: "*.log".into(),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.ends_with(".log")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("a.txt")));

    // Deleting a directory name prunes its subtree.
    let ops = vec![UpdateOp::Delete {
        pattern: "project/sub".into(),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.contains("sub")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_rename_entry() {
    let tmp = TempDir::new("update-rename");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Rename {
        from: EntryPath::from_utf8("project/a.txt"),
        to: EntryPath::from_utf8("project/renamed.txt"),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&"project/renamed.txt".to_string()));
    assert!(!names.contains(&"project/a.txt".to_string()));
    assert_unzip_t(&archive);

    // The renamed entry's content is intact.
    let opts = OpenOptions::default();
    let mut reader = engine().open(&archive, &opts).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/renamed.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"alpha");

    // Renaming a missing entry fails and leaves the archive intact.
    let before = fs::read(&archive).unwrap();
    let ops = vec![UpdateOp::Rename {
        from: EntryPath::from_utf8("missing.txt"),
        to: EntryPath::from_utf8("whatever.txt"),
    }];
    let err = run_update(&archive, &ops, &CreateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Other(_)));
    assert_eq!(
        fs::read(&archive).unwrap(),
        before,
        "archive must be untouched"
    );
}

#[test]
fn update_rejects_target_conflicts_without_explicit_delete() {
    let tmp = TempDir::new("update-conflicts");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"replacement").unwrap();

    let before = fs::read(&archive).unwrap();
    let err = run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntryPath::from_utf8("project/a.txt"),
            to: EntryPath::from_utf8("project/sub/b.txt"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("project/a.txt"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::AddDir {
            path: EntryPath::from_utf8("project"),
        }],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[
            UpdateOp::Rename {
                from: EntryPath::from_utf8("project/a.txt"),
                to: EntryPath::from_utf8("dup.txt"),
            },
            UpdateOp::Rename {
                from: EntryPath::from_utf8("project/sub/b.txt"),
                to: EntryPath::from_utf8("dup.txt"),
            },
        ],
        &CreateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "duplicate update target");
    assert_eq!(fs::read(&archive).unwrap(), before);

    run_update(
        &archive,
        &[
            UpdateOp::Delete {
                pattern: "project/a.txt".into(),
            },
            UpdateOp::Add {
                src: tmp.path().join("new.txt"),
                dest: EntryPath::from_utf8("project/a.txt"),
            },
        ],
        &CreateOptions::default(),
    )
    .unwrap();

    let mut reader = engine().open(&archive, &OpenOptions::default()).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/a.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"replacement");
    assert_unzip_t(&archive);
}

#[test]
fn update_combined_add_delete_rename() {
    let tmp = TempDir::new("update-combo");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("fresh.txt"), b"fresh").unwrap();
    let ops = vec![
        UpdateOp::Add {
            src: tmp.path().join("fresh.txt"),
            dest: EntryPath::from_utf8("fresh.txt"),
        },
        UpdateOp::Delete {
            pattern: "*.log".into(),
        },
        UpdateOp::Rename {
            from: EntryPath::from_utf8("project/sub/b.txt"),
            to: EntryPath::from_utf8("project/sub/beta.txt"),
        },
    ];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&"fresh.txt".to_string()));
    assert!(names.contains(&"project/sub/beta.txt".to_string()));
    assert!(!names.iter().any(|n| n.ends_with(".log")));
    assert!(!names.contains(&"project/sub/b.txt".to_string()));
    assert_unzip_t(&archive);
}

#[test]
fn update_encrypted_archive_without_password_keeps_encryption() {
    let tmp = TempDir::new("update-encrypted");
    let archive = base_archive(tmp.path(), Some("secret"));
    fs::write(tmp.path().join("plain.txt"), b"added later").unwrap();
    // No password supplied: old entries are raw-copied still encrypted.
    let ops = vec![UpdateOp::Add {
        src: tmp.path().join("plain.txt"),
        dest: EntryPath::from_utf8("plain.txt"),
    }];
    run_update(&archive, &ops, &CreateOptions::default()).unwrap();

    let opts = OpenOptions::default();
    let entries = engine().list(&archive, &opts).unwrap();
    let old = entries
        .iter()
        .find(|e| e.path.display == "project/a.txt")
        .unwrap();
    assert!(old.encrypted, "raw-copied entry must stay encrypted");
    let new = entries
        .iter()
        .find(|e| e.path.display == "plain.txt")
        .unwrap();
    assert!(!new.encrypted);

    // Old content still decrypts with the original password.
    let open = OpenOptions {
        password: Some(Password::new("secret")),
        encoding_override: None,
    };
    let mut reader = engine().open(&archive, &open).unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut reader
            .read_entry(&EntryPath::from_utf8("project/a.txt"))
            .unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, b"alpha");
}

#[test]
fn update_unsupported_format_is_rejected() {
    let tmp = TempDir::new("update-unsupported");
    let root = tmp.path().join("d");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("f.txt"), b"x").unwrap();
    let dest = tmp.path().join("a.tar");
    engine()
        .create(
            &dest,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let ops = vec![UpdateOp::Delete {
        pattern: "f.txt".into(),
    }];
    let err = run_update(&dest, &ops, &CreateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}
