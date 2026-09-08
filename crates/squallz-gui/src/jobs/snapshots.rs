//! Window-scoped job state and bounded revision history.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use squallz_core::{JobResources, QueueWaitReason};

use crate::dto::{ErrorDto, JobSpec};

const MAX_TERMINAL_SNAPSHOTS: usize = 100;
const MAX_SNAPSHOT_CHANGES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOrigin {
    App,
    FileManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobInteraction {
    Conflict,
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct JobProgressSnapshot {
    pub done: u64,
    pub total: u64,
    pub current: String,
    pub current_done: u64,
    pub current_total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_entries: Option<u64>,
    pub speed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    pub interruptible: bool,
}

impl Default for JobProgressSnapshot {
    fn default() -> Self {
        Self {
            done: 0,
            total: 0,
            current: String::new(),
            current_done: 0,
            current_total: 0,
            scanned_entries: None,
            speed: 0,
            phase: None,
            interruptible: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobStateSnapshot {
    pub id: u64,
    pub version: u64,
    pub spec: JobSpec,
    pub origin: JobOrigin,
    pub owned_by_requester: bool,
    pub state: String,
    pub queue_position: Option<u64>,
    pub queue_wait_reason: Option<QueueWaitReason>,
    pub cpu_threads: usize,
    pub stream_buffer_limit_bytes: Option<u64>,
    pub progress: JobProgressSnapshot,
    pub error: Option<ErrorDto>,
    pub result: Option<serde_json::Value>,
    pub interaction: Option<JobInteraction>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobSnapshotDelta {
    pub revision: u64,
    pub reset: bool,
    pub upserts: Vec<JobStateSnapshot>,
    pub removed: Vec<u64>,
}

#[derive(Clone)]
struct StoredJobSnapshot {
    id: u64,
    version: u64,
    spec: JobSpec,
    origin: JobOrigin,
    owner_window: Option<String>,
    state: String,
    queue_position: Option<u64>,
    queue_wait_reason: Option<QueueWaitReason>,
    cpu_threads: usize,
    stream_buffer_limit_bytes: Option<u64>,
    progress: JobProgressSnapshot,
    error: Option<ErrorDto>,
    result: Option<serde_json::Value>,
    interaction: Option<JobInteraction>,
    dismissed_by: HashSet<String>,
}

enum SnapshotRemovalAudience {
    OriginalViewers(Option<String>),
    Observer(String),
}

struct SnapshotRemoval {
    id: u64,
    version: u64,
    audience: SnapshotRemovalAudience,
}

#[derive(Default)]
pub(super) struct JobSnapshotStore {
    revision: u64,
    jobs: BTreeMap<u64, StoredJobSnapshot>,
    terminal_order: VecDeque<u64>,
    changes: VecDeque<u64>,
    removals: VecDeque<SnapshotRemoval>,
}

impl JobSnapshotStore {
    fn next_revision(&mut self) -> u64 {
        self.revision = self.revision.saturating_add(1);
        self.changes.push_back(self.revision);
        while self.changes.len() > MAX_SNAPSHOT_CHANGES {
            self.changes.pop_front();
        }
        self.revision
    }

    #[cfg(test)]
    pub(super) fn insert(
        &mut self,
        id: u64,
        owner_window: Option<String>,
        spec: JobSpec,
        state: &str,
    ) -> u64 {
        self.insert_with_resources(id, owner_window, spec, state, JobResources::default(), None)
    }

    pub(super) fn insert_with_resources(
        &mut self,
        id: u64,
        owner_window: Option<String>,
        spec: JobSpec,
        state: &str,
        resources: JobResources,
        stream_buffer_limit_bytes: Option<u64>,
    ) -> u64 {
        let version = self.next_revision();
        let origin = if owner_window.as_deref() == Some("main") || owner_window.is_none() {
            JobOrigin::App
        } else {
            JobOrigin::FileManager
        };
        self.jobs.insert(
            id,
            StoredJobSnapshot {
                id,
                version,
                spec,
                origin,
                owner_window,
                state: state.to_owned(),
                queue_position: None,
                queue_wait_reason: None,
                cpu_threads: resources.cpu_threads,
                stream_buffer_limit_bytes,
                progress: JobProgressSnapshot::default(),
                error: None,
                result: None,
                interaction: None,
                dismissed_by: HashSet::new(),
            },
        );
        if is_terminal_snapshot_state(state) {
            self.terminal_order.push_back(id);
            self.prune_terminal();
        }
        version
    }

    pub(super) fn set_state(
        &mut self,
        id: u64,
        state: &str,
        error: Option<ErrorDto>,
        result: Option<serde_json::Value>,
    ) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if is_terminal_snapshot_state(&current.state) || current.state == state {
            return None;
        }
        let leaving_queue = current.state == "queued" && state != "queued";
        let entering_terminal = is_terminal_snapshot_state(state);
        let version = self.next_revision();
        {
            let record = self.jobs.get_mut(&id)?;
            record.version = version;
            record.state = state.to_owned();
            record.error = error;
            record.result = result;
            if state != "queued" {
                record.queue_position = None;
                record.queue_wait_reason = None;
            }
        }
        if leaving_queue {
            self.normalize_queue_positions();
        }
        if entering_terminal {
            let record = self.jobs.get_mut(&id)?;
            record.interaction = None;
            self.terminal_order.push_back(id);
            self.prune_terminal();
        }
        Some(version)
    }

    pub(super) fn set_starting_state(&mut self, id: u64, state: &str) -> Option<u64> {
        if self.jobs.get(&id)?.state != "queued" {
            return None;
        }
        self.set_state(id, state, None, None)
    }

    pub(super) fn set_progress(&mut self, id: u64, progress: JobProgressSnapshot) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if is_terminal_snapshot_state(&current.state) || current.progress == progress {
            return None;
        }
        let version = self.next_revision();
        let record = self.jobs.get_mut(&id)?;
        record.version = version;
        record.progress = progress;
        Some(version)
    }

    pub(super) fn set_interaction(
        &mut self,
        id: u64,
        interaction: Option<JobInteraction>,
    ) -> Option<u64> {
        let current = self.jobs.get(&id)?;
        if current.interaction == interaction
            || (is_terminal_snapshot_state(&current.state) && interaction.is_some())
        {
            return None;
        }
        let version = self.next_revision();
        let record = self.jobs.get_mut(&id)?;
        record.version = version;
        record.interaction = interaction;
        Some(version)
    }

    fn sync_queue_positions(&mut self, ordered: &[u64]) {
        let statuses = ordered
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, (index + 1) as u64, None))
            .collect::<Vec<_>>();
        self.sync_queue_statuses(&statuses);
    }

    pub(super) fn sync_queue_statuses(&mut self, statuses: &[(u64, u64, Option<QueueWaitReason>)]) {
        let statuses = statuses
            .iter()
            .map(|(id, position, reason)| (*id, (*position, *reason)))
            .collect::<HashMap<_, _>>();
        let changes = self
            .jobs
            .iter()
            .filter_map(|(id, record)| {
                let next = (record.state == "queued")
                    .then(|| statuses.get(id).copied())
                    .flatten();
                let (position, reason) = next
                    .map(|(position, reason)| (Some(position), reason))
                    .unwrap_or((None, None));
                (record.queue_position != position || record.queue_wait_reason != reason)
                    .then_some((*id, position, reason))
            })
            .collect::<Vec<_>>();
        for (id, position, reason) in changes {
            let version = self.next_revision();
            if let Some(record) = self.jobs.get_mut(&id) {
                record.version = version;
                record.queue_position = position;
                record.queue_wait_reason = reason;
            }
        }
    }

    fn normalize_queue_positions(&mut self) {
        let mut ordered = self
            .jobs
            .values()
            .filter(|record| record.state == "queued")
            .map(|record| (record.queue_position.unwrap_or(u64::MAX), record.id))
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        let ids = ordered.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
        self.sync_queue_positions(&ids);
    }

    pub(super) fn snapshot(&self, requester: &str, id: u64) -> Option<JobStateSnapshot> {
        self.jobs
            .get(&id)
            .filter(|record| stored_snapshot_visible_to(record, requester))
            .map(|record| snapshot_for_requester(record, requester))
    }

    pub(super) fn delta(&self, requester: &str, since: Option<u64>) -> JobSnapshotDelta {
        let reset = match since {
            None => true,
            Some(revision) if revision > self.revision => true,
            Some(revision) => self
                .changes
                .front()
                .is_some_and(|first| revision < first.saturating_sub(1)),
        };
        let baseline = since.unwrap_or_default();
        let upserts = self
            .jobs
            .values()
            .filter(|record| stored_snapshot_visible_to(record, requester))
            .filter(|record| reset || record.version > baseline)
            .map(|record| snapshot_for_requester(record, requester))
            .collect();
        let removed = if reset {
            Vec::new()
        } else {
            let mut seen = HashSet::new();
            self.removals
                .iter()
                .filter(|removal| removal.version > baseline)
                .filter(|removal| removal_visible_to(removal, requester))
                .filter(|removal| seen.insert(removal.id))
                .map(|removal| removal.id)
                .collect()
        };
        JobSnapshotDelta {
            revision: self.revision,
            reset,
            upserts,
            removed,
        }
    }

    pub(super) fn dismiss(&mut self, requester: &str, ids: &[u64]) -> Result<(), ErrorDto> {
        let mut seen = HashSet::new();
        let unique = ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        for id in &unique {
            let Some(record) = self.jobs.get(id) else {
                return Err(job_unavailable_error());
            };
            if !snapshot_in_original_scope(record.owner_window.as_deref(), requester)
                || !is_terminal_snapshot_state(&record.state)
            {
                return Err(job_unavailable_error());
            }
        }
        for id in unique {
            let newly_dismissed = self
                .jobs
                .get_mut(&id)
                .is_some_and(|record| record.dismissed_by.insert(requester.to_owned()));
            if newly_dismissed {
                let version = self.next_revision();
                self.removals.push_back(SnapshotRemoval {
                    id,
                    version,
                    audience: SnapshotRemovalAudience::Observer(requester.to_owned()),
                });
                self.trim_removals();
            }
        }
        Ok(())
    }

    fn prune_terminal(&mut self) {
        while self.terminal_order.len() > MAX_TERMINAL_SNAPSHOTS {
            if let Some(id) = self.terminal_order.pop_front() {
                self.remove_without_terminal_scan(id);
            }
        }
    }

    fn remove_without_terminal_scan(&mut self, id: u64) {
        let Some(record) = self.jobs.remove(&id) else {
            return;
        };
        let version = self.next_revision();
        self.removals.push_back(SnapshotRemoval {
            id,
            version,
            audience: SnapshotRemovalAudience::OriginalViewers(record.owner_window),
        });
        self.trim_removals();
    }

    fn trim_removals(&mut self) {
        while self.removals.len() > MAX_SNAPSHOT_CHANGES {
            self.removals.pop_front();
        }
    }
}

fn snapshot_in_original_scope(owner_window: Option<&str>, requester: &str) -> bool {
    requester == "main" || owner_window == Some(requester)
}

fn stored_snapshot_visible_to(record: &StoredJobSnapshot, requester: &str) -> bool {
    snapshot_in_original_scope(record.owner_window.as_deref(), requester)
        && !record.dismissed_by.contains(requester)
}

fn removal_visible_to(removal: &SnapshotRemoval, requester: &str) -> bool {
    match &removal.audience {
        SnapshotRemovalAudience::OriginalViewers(owner_window) => {
            snapshot_in_original_scope(owner_window.as_deref(), requester)
        }
        SnapshotRemovalAudience::Observer(observer) => observer == requester,
    }
}

fn snapshot_for_requester(record: &StoredJobSnapshot, requester: &str) -> JobStateSnapshot {
    JobStateSnapshot {
        id: record.id,
        version: record.version,
        spec: record.spec.clone(),
        origin: record.origin,
        owned_by_requester: record.owner_window.as_deref() == Some(requester),
        state: record.state.clone(),
        queue_position: record.queue_position,
        queue_wait_reason: record.queue_wait_reason,
        cpu_threads: record.cpu_threads,
        stream_buffer_limit_bytes: record.stream_buffer_limit_bytes,
        progress: record.progress.clone(),
        error: record.error.clone(),
        result: record.result.clone(),
        interaction: record.interaction,
    }
}

fn is_terminal_snapshot_state(state: &str) -> bool {
    matches!(state, "done" | "failed" | "cancelled")
}

pub(super) fn job_unavailable_error() -> ErrorDto {
    ErrorDto::other("job is unavailable to this window")
}

#[cfg(test)]
mod tests {
    use super::super::test_support::checksum_job;
    use super::*;
    use std::path::Path;

    #[test]
    fn snapshot_store_scopes_jobs_and_tracks_deltas() {
        let mut store = JobSnapshotStore::default();
        let main_version = store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("main-input")),
            "queued",
        );
        let owner_version = store.insert_with_resources(
            2,
            Some("task-owner".into()),
            checksum_job(Path::new("task-input")),
            "queued",
            JobResources::new(3),
            Some(512 * 1024 * 1024),
        );
        assert!(owner_version > main_version);

        let main = store.delta("main", None);
        assert!(main.reset);
        assert_eq!(main.upserts.len(), 2);
        assert_eq!(main.upserts[0].origin, JobOrigin::App);
        assert!(main.upserts[0].owned_by_requester);
        assert_eq!(main.upserts[1].origin, JobOrigin::FileManager);
        assert!(!main.upserts[1].owned_by_requester);

        let owner = store.delta("task-owner", None);
        assert_eq!(owner.upserts.len(), 1);
        assert_eq!(owner.upserts[0].id, 2);
        assert!(owner.upserts[0].owned_by_requester);
        assert_eq!(owner.upserts[0].origin, JobOrigin::FileManager);
        assert!(store.delta("task-other", None).upserts.is_empty());

        let progress_version = store
            .set_progress(
                2,
                JobProgressSnapshot {
                    done: 4,
                    total: 10,
                    current: "payload.bin".into(),
                    current_done: 2,
                    current_total: 8,
                    scanned_entries: None,
                    speed: 64,
                    ..JobProgressSnapshot::default()
                },
            )
            .unwrap();
        assert!(progress_version > owner_version);
        let done_version = store
            .set_state(
                2,
                "done",
                None,
                Some(serde_json::json!({"operation": "checksum"})),
            )
            .unwrap();
        assert!(done_version > progress_version);
        assert!(store.set_state(2, "running", None, None).is_none());
        assert!(store
            .set_progress(2, JobProgressSnapshot::default())
            .is_none());
        assert_eq!(store.snapshot("main", 2).unwrap().state, "done");

        let denied = store.dismiss("task-other", &[2]).unwrap_err();
        assert_eq!(denied.key, "error.other");
        assert!(store.snapshot("main", 2).is_some());
        let main_baseline = store.revision;
        store.dismiss("task-owner", &[2]).unwrap();
        let removed = store.delta("task-owner", Some(done_version));
        assert!(!removed.reset);
        assert!(removed.upserts.is_empty());
        assert_eq!(removed.removed, vec![2]);
        assert!(store.snapshot("task-owner", 2).is_none());
        assert!(store.snapshot("main", 2).is_some());
        assert!(store.delta("task-owner", None).upserts.is_empty());
        let main_after_owner_dismiss = store.delta("main", Some(main_baseline));
        assert!(main_after_owner_dismiss.upserts.is_empty());
        assert!(main_after_owner_dismiss.removed.is_empty());

        let serialized = serde_json::to_string(&main.upserts[1]).unwrap();
        assert!(!serialized.contains("task-owner"));
        assert!(serialized.contains("owned_by_requester"));
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert!(value["interaction"].is_null());
        assert!(value["queue_position"].is_null());
        assert!(value["queue_wait_reason"].is_null());
        assert_eq!(value["cpu_threads"], 3);
        assert_eq!(value["stream_buffer_limit_bytes"], 512 * 1024 * 1024);
    }

    #[test]
    fn snapshot_store_tracks_authoritative_queue_positions() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("first-input")),
            "queued",
        );
        store.insert(
            2,
            Some("task-owner".into()),
            checksum_job(Path::new("second-input")),
            "queued",
        );
        store.insert(
            3,
            Some("main".into()),
            checksum_job(Path::new("third-input")),
            "queued",
        );

        store.sync_queue_positions(&[1, 2, 3]);
        assert_eq!(store.snapshot("main", 1).unwrap().queue_position, Some(1));
        assert_eq!(store.snapshot("main", 2).unwrap().queue_position, Some(2));
        assert_eq!(store.snapshot("main", 3).unwrap().queue_position, Some(3));

        let baseline = store.revision;
        store.sync_queue_positions(&[3, 1, 2]);
        let delta = store.delta("main", Some(baseline));
        assert_eq!(
            delta
                .upserts
                .iter()
                .map(|snapshot| (snapshot.id, snapshot.queue_position))
                .collect::<Vec<_>>(),
            vec![(1, Some(2)), (2, Some(3)), (3, Some(1))]
        );

        store.set_state(3, "running", None, None).unwrap();
        assert_eq!(store.snapshot("main", 3).unwrap().queue_position, None);
        assert_eq!(store.snapshot("main", 1).unwrap().queue_position, Some(1));
        assert_eq!(store.snapshot("main", 2).unwrap().queue_position, Some(2));
    }

    #[test]
    fn snapshot_starting_state_cannot_override_a_published_pause() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("queued-input")),
            "queued",
        );
        assert!(store.set_starting_state(1, "running").is_some());
        assert_eq!(store.snapshot("main", 1).unwrap().state, "running");

        store.insert(
            2,
            Some("main".into()),
            checksum_job(Path::new("paused-input")),
            "queued",
        );
        let paused_version = store.set_state(2, "paused", None, None).unwrap();
        assert!(store.set_starting_state(2, "running").is_none());
        let paused = store.snapshot("main", 2).unwrap();
        assert_eq!(paused.state, "paused");
        assert_eq!(paused.version, paused_version);
    }

    #[test]
    fn snapshot_dismissal_is_scoped_to_each_observer_and_survives_reset() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            10,
            Some("task-owner".into()),
            checksum_job(Path::new("main-dismissed")),
            "done",
        );
        let baseline = store.insert(
            11,
            Some("task-owner".into()),
            checksum_job(Path::new("owner-dismissed")),
            "done",
        );

        store.dismiss("main", &[10]).unwrap();
        store.dismiss("task-owner", &[11]).unwrap();

        assert!(store.snapshot("main", 10).is_none());
        assert!(store.snapshot("main", 11).is_some());
        assert!(store.snapshot("task-owner", 10).is_some());
        assert!(store.snapshot("task-owner", 11).is_none());
        assert_eq!(store.delta("main", Some(baseline)).removed, vec![10]);
        assert_eq!(store.delta("task-owner", Some(baseline)).removed, vec![11]);

        let main_reset = store.delta("main", None);
        assert!(main_reset.reset);
        assert_eq!(
            main_reset
                .upserts
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![11]
        );
        let owner_reset = store.delta("task-owner", None);
        assert!(owner_reset.reset);
        assert_eq!(
            owner_reset
                .upserts
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![10]
        );
        assert_eq!(store.jobs.len(), 2);

        let revision = store.revision;
        store.dismiss("main", &[10]).unwrap();
        store.dismiss("task-owner", &[11]).unwrap();
        assert_eq!(store.revision, revision);
    }

    #[test]
    fn snapshot_store_bounds_history_without_pruning_active_jobs() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("active-input")),
            "running",
        );
        for done in 1..=(MAX_SNAPSHOT_CHANGES as u64 + 1) {
            store
                .set_progress(
                    1,
                    JobProgressSnapshot {
                        done,
                        total: MAX_SNAPSHOT_CHANGES as u64 + 1,
                        ..JobProgressSnapshot::default()
                    },
                )
                .unwrap();
        }

        assert_eq!(store.changes.len(), MAX_SNAPSHOT_CHANGES);
        let stale = store.delta("main", Some(0));
        assert!(stale.reset);
        assert_eq!(stale.upserts.len(), 1);
        assert_eq!(stale.upserts[0].id, 1);
        assert_eq!(stale.upserts[0].state, "running");

        let mut tombstones = JobSnapshotStore::default();
        for id in 1..=(MAX_SNAPSHOT_CHANGES as u64 + 1) {
            tombstones.insert(
                id,
                Some("main".into()),
                checksum_job(Path::new("terminal-input")),
                "done",
            );
            tombstones.dismiss("main", &[id]).unwrap();
        }
        assert_eq!(tombstones.removals.len(), MAX_SNAPSHOT_CHANGES);
        assert!(!tombstones.removals.iter().any(|removal| removal.id == 1));
        assert!(tombstones
            .removals
            .iter()
            .any(|removal| removal.id == MAX_SNAPSHOT_CHANGES as u64 + 1));
    }

    #[test]
    fn snapshot_store_keeps_only_the_newest_terminal_jobs() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("main".into()),
            checksum_job(Path::new("active-input")),
            "running",
        );
        for id in 2..=(MAX_TERMINAL_SNAPSHOTS as u64 + 2) {
            store.insert(
                id,
                Some("main".into()),
                checksum_job(Path::new("terminal-input")),
                "done",
            );
        }

        assert!(store.jobs.contains_key(&1));
        assert!(!store.jobs.contains_key(&2));
        assert_eq!(store.terminal_order.len(), MAX_TERMINAL_SNAPSHOTS);
        assert_eq!(store.jobs.len(), MAX_TERMINAL_SNAPSHOTS + 1);
        let delta = store.delta("main", Some(0));
        assert!(!delta.reset);
        assert!(delta.removed.contains(&2));
    }

    #[test]
    fn terminal_retention_removal_reaches_every_original_observer() {
        let mut store = JobSnapshotStore::default();
        store.insert(
            1,
            Some("task-owner".into()),
            checksum_job(Path::new("oldest-terminal")),
            "done",
        );
        store.dismiss("main", &[1]).unwrap();
        let after_main_dismiss = store.revision;
        for id in 2..=(MAX_TERMINAL_SNAPSHOTS as u64 + 1) {
            store.insert(
                id,
                Some("task-owner".into()),
                checksum_job(Path::new("newer-terminal")),
                "done",
            );
        }

        assert!(!store.jobs.contains_key(&1));
        assert_eq!(store.terminal_order.len(), MAX_TERMINAL_SNAPSHOTS);
        assert_eq!(
            store.delta("main", Some(after_main_dismiss)).removed,
            vec![1]
        );
        assert_eq!(
            store.delta("task-owner", Some(after_main_dismiss)).removed,
            vec![1]
        );
        assert!(!store
            .delta("main", None)
            .upserts
            .iter()
            .any(|snapshot| snapshot.id == 1));
        assert!(!store
            .delta("task-owner", None)
            .upserts
            .iter()
            .any(|snapshot| snapshot.id == 1));
    }
}
