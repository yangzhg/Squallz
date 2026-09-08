//! Throttled job events and aggregate progress for archive batches.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use squallz_core::api::{EntryPath, ProgressPhase, ProgressSink};
use squallz_core::lock_unpoisoned;

use crate::dto::ProgressEvent;
use crate::events::{emit, EventSink, EV_PROGRESS};

use super::redact_source_text;
use super::snapshots::{JobProgressSnapshot, JobSnapshotStore};

const PROGRESS_THROTTLE_MS: u128 = 60;

/// Progress sink that forwards to the queue snapshot and emits throttled
/// `job://progress` events with a derived speed.
pub(super) struct EmitProgress<'a> {
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

pub(super) const BATCH_PROGRESS_SCALE: u64 = 1_000;

pub(super) struct BatchProgressSink<'a> {
    inner: &'a dyn ProgressSink,
    total_archives: u64,
    state: Mutex<BatchProgressState>,
}

struct BatchProgressState {
    index: u64,
    archive: String,
}

impl<'a> BatchProgressSink<'a> {
    pub(super) fn new(inner: &'a dyn ProgressSink, total_archives: usize) -> Self {
        Self {
            inner,
            total_archives: total_archives.max(1) as u64,
            state: Mutex::new(BatchProgressState {
                index: 0,
                archive: String::new(),
            }),
        }
    }

    pub(super) fn start_archive(&self, index: usize, archive: String) {
        {
            let mut state = lock_unpoisoned(&self.state);
            state.index = index as u64;
            state.archive = archive.clone();
        }
        self.emit(index as u64 * BATCH_PROGRESS_SCALE, archive, 0, 0);
    }

    pub(super) fn finish_archive(&self, index: usize, archive: String) {
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
    pub(super) fn new(
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
    pub(super) fn flush(&self) {
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

#[cfg(test)]
mod tests {
    use super::super::test_support::{checksum_job, RecordingProgressSink, TestSink};
    use super::*;
    use crate::dto::JobSpec;

    #[test]
    fn progress_distinguishes_bytes_scanning_and_commit_phases() {
        let events = Arc::new(TestSink::default());
        let snapshots = Arc::new(Mutex::new(JobSnapshotStore::default()));
        lock_unpoisoned(&snapshots).insert(
            1,
            Some("main".into()),
            checksum_job(std::path::Path::new("input.bin")),
            "running",
        );
        let progress = EmitProgress::new(
            1,
            events,
            Arc::clone(&snapshots),
            &squallz_core::api::NoProgress,
            &[],
        );
        let entry = EntryPath::from_utf8("input.bin");
        progress.on_entry_progress(10, 100, &entry, 4, 20);
        progress.flush();
        let snapshot = lock_unpoisoned(&snapshots).snapshot("main", 1).unwrap();
        assert_eq!(
            (
                snapshot.progress.current_done,
                snapshot.progress.current_total
            ),
            (4, 20)
        );

        progress.on_scan_progress(7, &entry);
        progress.flush();
        let snapshot = lock_unpoisoned(&snapshots).snapshot("main", 1).unwrap();
        assert_eq!(snapshot.progress.scanned_entries, Some(7));
        assert_eq!((snapshot.progress.total, snapshot.progress.speed), (0, 0));

        for (phase, name, interruptible, counts) in [
            (
                ProgressPhase::UpdateVerify,
                "update_verify",
                true,
                (25, 100),
            ),
            (ProgressPhase::OutputSplit, "output_split", true, (25, 100)),
            (
                ProgressPhase::RecoveryProcess,
                "recovery_process",
                false,
                (25, 100),
            ),
            (ProgressPhase::UpdateCommit, "update_commit", false, (0, 0)),
            (ProgressPhase::OutputCommit, "output_commit", false, (0, 0)),
            (
                ProgressPhase::OutputRecovery,
                "output_recovery",
                false,
                (0, 0),
            ),
        ] {
            progress.on_phase(phase, interruptible);
            progress.on_progress(25, 100, &entry);
            progress.flush();
            let snapshot = lock_unpoisoned(&snapshots).snapshot("main", 1).unwrap();
            assert_eq!(snapshot.progress.phase.as_deref(), Some(name));
            assert_eq!(
                (snapshot.progress.done, snapshot.progress.total),
                counts,
                "{name}"
            );
            if !interruptible {
                assert_eq!(snapshot.progress.speed, 0, "{name}");
            }
            assert_eq!(snapshot.progress.interruptible, interruptible, "{name}");
        }
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
}
