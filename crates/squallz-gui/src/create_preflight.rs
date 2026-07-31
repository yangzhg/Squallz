//! Request-scoped control and progress for work that runs before a job enters
//! the shared queue.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::json;
use squallz_core::api::{ControlToken, EntryPath, ProgressSink};

use crate::events::EventSink;

pub(crate) const EV_CREATE_PREFLIGHT: &str = "create://preflight";
const DESTINATION_PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
const MAX_PENDING_CANCELLATIONS: usize = 4_096;
const MAX_RECENTLY_COMPLETED_REQUESTS: usize = 4_096;

type RequestKey = (String, PreflightRequestKind, PreflightRequestId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PreflightRequestId {
    Named(String),
    Anonymous(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PreflightRequestKind {
    CreateDestination,
    ConvertPlan,
    ExtractPlan,
    OpenArchive,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Owns cancellation tokens for preflight requests. The request kind, window
/// label and opaque frontend request id keep unrelated work isolated.
#[derive(Default)]
pub(crate) struct PreflightRequests {
    registry: Mutex<PreflightRegistry>,
    idle: Condvar,
    next_anonymous_id: AtomicU64,
}

pub(crate) struct PreflightRequestLease {
    requests: Arc<PreflightRequests>,
    kind: PreflightRequestKind,
    owner: String,
    request_id: PreflightRequestId,
    token: Arc<ControlToken>,
}

#[derive(Default)]
struct PreflightRegistry {
    active: HashMap<RequestKey, Arc<ControlToken>>,
    inflight: HashSet<usize>,
    pending_cancellations: HashSet<RequestKey>,
    pending_cancellation_order: VecDeque<RequestKey>,
    recently_completed: HashSet<RequestKey>,
    recently_completed_order: VecDeque<RequestKey>,
    released_windows: HashSet<String>,
    shutting_down: bool,
}

impl PreflightRegistry {
    fn remember_pending_cancellation(&mut self, key: RequestKey) {
        if !self.pending_cancellations.insert(key.clone()) {
            return;
        }
        if self.pending_cancellation_order.len() >= MAX_PENDING_CANCELLATIONS {
            if let Some(oldest) = self.pending_cancellation_order.pop_front() {
                self.pending_cancellations.remove(&oldest);
            }
        }
        self.pending_cancellation_order.push_back(key);
    }

    fn consume_pending_cancellation(&mut self, key: &RequestKey) -> bool {
        if !self.pending_cancellations.remove(key) {
            return false;
        }
        self.pending_cancellation_order
            .retain(|candidate| candidate != key);
        true
    }

    fn remember_completed(&mut self, key: RequestKey) {
        if !self.recently_completed.insert(key.clone()) {
            return;
        }
        if self.recently_completed_order.len() >= MAX_RECENTLY_COMPLETED_REQUESTS {
            if let Some(oldest) = self.recently_completed_order.pop_front() {
                self.recently_completed.remove(&oldest);
            }
        }
        self.recently_completed_order.push_back(key);
    }

    fn forget_completed(&mut self, key: &RequestKey) {
        if !self.recently_completed.remove(key) {
            return;
        }
        self.recently_completed_order
            .retain(|candidate| candidate != key);
    }

    fn remove_request_history_for_window(&mut self, owner: &str) {
        self.pending_cancellations
            .retain(|(request_owner, _, _)| request_owner != owner);
        self.pending_cancellation_order
            .retain(|(request_owner, _, _)| request_owner != owner);
        self.recently_completed
            .retain(|(request_owner, _, _)| request_owner != owner);
        self.recently_completed_order
            .retain(|(request_owner, _, _)| request_owner != owner);
    }
}

impl PreflightRequests {
    pub(crate) fn begin_request(
        self: &Arc<Self>,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: &str,
    ) -> PreflightRequestLease {
        self.begin_request_with_id(
            kind,
            owner,
            PreflightRequestId::Named(request_id.to_owned()),
        )
    }

    pub(crate) fn begin_anonymous_request(
        self: &Arc<Self>,
        kind: PreflightRequestKind,
        owner: &str,
    ) -> PreflightRequestLease {
        let request_id =
            PreflightRequestId::Anonymous(self.next_anonymous_id.fetch_add(1, Ordering::Relaxed));
        self.begin_request_with_id(kind, owner, request_id)
    }

    fn begin_request_with_id(
        self: &Arc<Self>,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: PreflightRequestId,
    ) -> PreflightRequestLease {
        let token = self.begin_with_id(kind, owner, &request_id);
        PreflightRequestLease {
            requests: Arc::clone(self),
            kind,
            owner: owner.to_owned(),
            request_id,
            token,
        }
    }

    #[cfg(test)]
    fn begin(
        &self,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: &str,
    ) -> Arc<ControlToken> {
        self.begin_with_id(
            kind,
            owner,
            &PreflightRequestId::Named(request_id.to_owned()),
        )
    }

    fn begin_with_id(
        &self,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: &PreflightRequestId,
    ) -> Arc<ControlToken> {
        let token = ControlToken::new();
        let mut registry = lock_unpoisoned(&self.registry);
        let key = (owner.to_owned(), kind, request_id.clone());
        if registry.shutting_down || registry.released_windows.contains(owner) {
            token.cancel();
            return token;
        }
        registry.inflight.insert(Arc::as_ptr(&token) as usize);
        registry.forget_completed(&key);
        let cancelled_before_begin = registry.consume_pending_cancellation(&key);
        let previous = registry.active.insert(key, Arc::clone(&token));
        drop(registry);
        if let Some(previous) = previous {
            previous.cancel();
        }
        if cancelled_before_begin {
            token.cancel();
        }
        token
    }

    pub(crate) fn cancel(&self, kind: PreflightRequestKind, owner: &str, request_id: &str) -> bool {
        let key = (
            owner.to_owned(),
            kind,
            PreflightRequestId::Named(request_id.to_owned()),
        );
        let token = {
            let mut registry = lock_unpoisoned(&self.registry);
            let token = registry.active.get(&key).cloned();
            if token.is_none()
                && !registry.shutting_down
                && !registry.released_windows.contains(owner)
                && !registry.recently_completed.contains(&key)
            {
                registry.remember_pending_cancellation(key);
            }
            token
        };
        if let Some(token) = token {
            token.cancel();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn complete(
        &self,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: &str,
        completed: &Arc<ControlToken>,
    ) {
        self.complete_with_id(
            kind,
            owner,
            &PreflightRequestId::Named(request_id.to_owned()),
            completed,
        );
    }

    fn complete_with_id(
        &self,
        kind: PreflightRequestKind,
        owner: &str,
        request_id: &PreflightRequestId,
        completed: &Arc<ControlToken>,
    ) {
        let key = (owner.to_owned(), kind, request_id.clone());
        let mut registry = lock_unpoisoned(&self.registry);
        let completed_current = registry
            .active
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, completed));
        if completed_current {
            registry.active.remove(&key);
            if matches!(&key.2, PreflightRequestId::Named(_)) {
                registry.remember_completed(key);
            }
        }
        if registry.inflight.remove(&(Arc::as_ptr(completed) as usize))
            && registry.inflight.is_empty()
        {
            self.idle.notify_all();
        }
    }

    pub(crate) fn release_window(&self, owner: &str) -> usize {
        let mut registry = lock_unpoisoned(&self.registry);
        registry.released_windows.insert(owner.to_owned());
        registry.remove_request_history_for_window(owner);
        let keys = registry
            .active
            .keys()
            .filter(|(request_owner, _, _)| request_owner == owner)
            .cloned()
            .collect::<Vec<_>>();
        let mut cancelled = 0;
        for key in keys {
            if let Some(token) = registry.active.remove(&key) {
                token.cancel();
                cancelled += 1;
            }
        }
        cancelled
    }

    pub(crate) fn cancel_all(&self) -> usize {
        let tokens = {
            let mut registry = lock_unpoisoned(&self.registry);
            registry.shutting_down = true;
            registry.pending_cancellations.clear();
            registry.pending_cancellation_order.clear();
            registry.recently_completed.clear();
            registry.recently_completed_order.clear();
            registry
                .active
                .drain()
                .map(|(_, token)| token)
                .collect::<Vec<_>>()
        };
        for token in &tokens {
            token.cancel();
        }
        tokens.len()
    }

    /// Waits until every registered blocking worker has returned. Shutdown
    /// uses this before releasing archive and preview resources pinned by a
    /// preflight request.
    pub(crate) fn wait_idle(&self) {
        let mut registry = lock_unpoisoned(&self.registry);
        while !registry.inflight.is_empty() {
            registry = match self.idle.wait(registry) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }
}

impl PreflightRequestLease {
    pub(crate) fn control(&self) -> Arc<ControlToken> {
        Arc::clone(&self.token)
    }
}

impl Drop for PreflightRequestLease {
    fn drop(&mut self) {
        self.requests
            .complete_with_id(self.kind, &self.owner, &self.request_id, &self.token);
    }
}

#[derive(Clone)]
struct DestinationProgressSnapshot {
    processed_bytes: u64,
    current: String,
}

struct DestinationProgressWindow {
    last_emit: Instant,
    pending: Option<DestinationProgressSnapshot>,
}

/// Throttles the core's chunk-level digest events without inventing a total.
/// `flush` is called before the IPC request completes so the frontend sees
/// the exact final byte count even when the last chunk arrived inside the
/// throttle window.
pub(crate) struct DestinationInspectionProgress {
    events: Arc<dyn EventSink>,
    request_id: String,
    inner: Mutex<DestinationProgressWindow>,
}

impl DestinationInspectionProgress {
    pub(crate) fn new(events: Arc<dyn EventSink>, request_id: String) -> Self {
        Self {
            events,
            request_id,
            inner: Mutex::new(DestinationProgressWindow {
                last_emit: Instant::now() - DESTINATION_PROGRESS_INTERVAL,
                pending: None,
            }),
        }
    }

    pub(crate) fn flush(&self) {
        let snapshot = lock_unpoisoned(&self.inner).pending.take();
        if let Some(snapshot) = snapshot {
            self.emit(snapshot);
        }
    }

    fn emit(&self, snapshot: DestinationProgressSnapshot) {
        self.events.emit_json(
            EV_CREATE_PREFLIGHT,
            json!({
                "request_id": self.request_id,
                "phase": "destination",
                "processed_bytes": snapshot.processed_bytes,
                "total_bytes": 0,
                "current": snapshot.current,
            }),
        );
    }
}

impl ProgressSink for DestinationInspectionProgress {
    fn on_progress(&self, done: u64, _total: u64, current: &EntryPath) {
        let snapshot = DestinationProgressSnapshot {
            processed_bytes: done,
            current: current.display.clone(),
        };
        let emit = {
            let mut window = lock_unpoisoned(&self.inner);
            window.pending = Some(snapshot);
            if window.last_emit.elapsed() >= DESTINATION_PROGRESS_INTERVAL {
                window.last_emit = Instant::now();
                window.pending.take()
            } else {
                None
            }
        };
        if let Some(snapshot) = emit {
            self.emit(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingEvents {
        values: Mutex<Vec<serde_json::Value>>,
    }

    impl EventSink for RecordingEvents {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            assert_eq!(event, EV_CREATE_PREFLIGHT);
            lock_unpoisoned(&self.values).push(payload);
        }
    }

    #[test]
    fn cancellation_is_scoped_to_the_exact_window_and_request() {
        let requests = PreflightRequests::default();
        let first = requests.begin(PreflightRequestKind::ExtractPlan, "main", "same-id");
        let other_window = requests.begin(PreflightRequestKind::ExtractPlan, "task-1", "same-id");

        assert!(requests.cancel(PreflightRequestKind::ExtractPlan, "main", "same-id"));
        assert!(first.is_cancelled());
        assert!(!other_window.is_cancelled());
        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "missing"));
    }

    #[test]
    fn cancellation_is_scoped_to_the_request_kind() {
        let requests = PreflightRequests::default();
        let create = requests.begin(PreflightRequestKind::CreateDestination, "main", "same-id");
        let extract = requests.begin(PreflightRequestKind::ExtractPlan, "main", "same-id");
        let open = requests.begin(PreflightRequestKind::OpenArchive, "main", "same-id");

        assert!(requests.cancel(PreflightRequestKind::OpenArchive, "main", "same-id"));
        assert!(!create.is_cancelled());
        assert!(!extract.is_cancelled());
        assert!(open.is_cancelled());
    }

    #[test]
    fn cancellation_before_begin_is_applied_to_the_named_request() {
        let requests = Arc::new(PreflightRequests::default());

        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "early"));
        let request = requests.begin_request(PreflightRequestKind::ExtractPlan, "main", "early");

        assert!(request.control().is_cancelled());
        drop(request);
        requests.wait_idle();
    }

    #[test]
    fn anonymous_requests_cannot_collide_with_frontend_request_ids() {
        let requests = Arc::new(PreflightRequests::default());
        let request = requests.begin_anonymous_request(PreflightRequestKind::ExtractPlan, "main");
        let control = request.control();

        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "0"));
        assert!(!control.is_cancelled());
        drop(request);
        requests.wait_idle();
    }

    #[test]
    fn pending_cancellations_are_bounded() {
        let requests = PreflightRequests::default();
        for index in 0..=MAX_PENDING_CANCELLATIONS {
            assert!(!requests.cancel(
                PreflightRequestKind::ExtractPlan,
                "main",
                &format!("request-{index}")
            ));
        }

        assert_eq!(
            lock_unpoisoned(&requests.registry)
                .pending_cancellations
                .len(),
            MAX_PENDING_CANCELLATIONS
        );
        let evicted = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request-0");
        let newest = requests.begin(
            PreflightRequestKind::ExtractPlan,
            "main",
            &format!("request-{MAX_PENDING_CANCELLATIONS}"),
        );
        assert!(!evicted.is_cancelled());
        assert!(newest.is_cancelled());
        requests.complete(
            PreflightRequestKind::ExtractPlan,
            "main",
            "request-0",
            &evicted,
        );
        requests.complete(
            PreflightRequestKind::ExtractPlan,
            "main",
            &format!("request-{MAX_PENDING_CANCELLATIONS}"),
            &newest,
        );
    }

    #[test]
    fn recently_completed_requests_are_bounded() {
        let requests = PreflightRequests::default();
        for index in 0..=MAX_RECENTLY_COMPLETED_REQUESTS {
            let request_id = format!("request-{index}");
            let token = requests.begin(PreflightRequestKind::ExtractPlan, "main", &request_id);
            requests.complete(
                PreflightRequestKind::ExtractPlan,
                "main",
                &request_id,
                &token,
            );
        }

        assert_eq!(
            lock_unpoisoned(&requests.registry).recently_completed.len(),
            MAX_RECENTLY_COMPLETED_REQUESTS
        );
    }

    #[test]
    fn stale_completion_does_not_remove_a_replacement_request() {
        let requests = PreflightRequests::default();
        let first = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");
        let replacement = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");

        assert!(first.is_cancelled());
        requests.complete(PreflightRequestKind::ExtractPlan, "main", "request", &first);
        assert!(requests.cancel(PreflightRequestKind::ExtractPlan, "main", "request"));
        assert!(replacement.is_cancelled());
    }

    #[test]
    fn completed_requests_leave_the_registry() {
        let requests = PreflightRequests::default();
        let token = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");

        requests.complete(PreflightRequestKind::ExtractPlan, "main", "request", &token);

        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "request"));
        assert!(lock_unpoisoned(&requests.registry)
            .pending_cancellations
            .is_empty());
        assert_eq!(requests.release_window("main"), 0);
    }

    #[test]
    fn late_cancellation_does_not_poison_a_later_begin() {
        let requests = PreflightRequests::default();
        let completed = requests.begin(PreflightRequestKind::ExtractPlan, "main", "one-shot");
        requests.complete(
            PreflightRequestKind::ExtractPlan,
            "main",
            "one-shot",
            &completed,
        );

        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "one-shot"));
        let later = requests.begin(PreflightRequestKind::ExtractPlan, "main", "one-shot");
        assert!(!later.is_cancelled());
        requests.complete(
            PreflightRequestKind::ExtractPlan,
            "main",
            "one-shot",
            &later,
        );
    }

    #[test]
    fn releasing_a_window_only_cancels_its_requests() {
        let requests = PreflightRequests::default();
        let first = requests.begin(PreflightRequestKind::ExtractPlan, "main", "one");
        let second = requests.begin(PreflightRequestKind::CreateDestination, "main", "two");
        let other = requests.begin(PreflightRequestKind::ExtractPlan, "task-1", "one");

        assert_eq!(requests.release_window("main"), 2);
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert!(!other.is_cancelled());
        assert_eq!(requests.cancel_all(), 1);
        assert!(other.is_cancelled());
    }

    #[test]
    fn released_windows_reject_late_requests() {
        let requests = PreflightRequests::default();

        assert_eq!(requests.release_window("main"), 0);
        let late = requests.begin(PreflightRequestKind::ExtractPlan, "main", "late");

        assert!(late.is_cancelled());
        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "late"));
        requests.complete(PreflightRequestKind::ExtractPlan, "main", "late", &late);
    }

    #[test]
    fn shutdown_rejects_late_requests_without_reopening_drain() {
        let requests = Arc::new(PreflightRequests::default());

        assert_eq!(requests.cancel_all(), 0);
        let late = requests.begin_request(PreflightRequestKind::ExtractPlan, "main", "late");

        assert!(late.control().is_cancelled());
        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "late"));

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiting = Arc::clone(&requests);
        let waiter = std::thread::spawn(move || {
            waiting.wait_idle();
            let _ = done_tx.send(());
        });
        let drained_while_lease_is_held = done_rx.recv_timeout(Duration::from_secs(1)).is_ok();

        drop(late);
        assert!(waiter.join().is_ok());
        assert!(drained_while_lease_is_held);
    }

    #[test]
    fn destination_progress_flushes_the_exact_latest_bytes() {
        let events = Arc::new(RecordingEvents::default());
        let sink = DestinationInspectionProgress::new(
            Arc::clone(&events) as Arc<dyn EventSink>,
            "request-7".to_owned(),
        );

        sink.on_progress(64, 0, &EntryPath::from_utf8("old.zip"));
        sink.on_progress(128, 0, &EntryPath::from_utf8("old.zip"));
        sink.flush();

        let values = lock_unpoisoned(&events.values);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["processed_bytes"], 64);
        assert_eq!(values[1]["processed_bytes"], 128);
        assert_eq!(values[1]["total_bytes"], 0);
        assert_eq!(values[1]["current"], "old.zip");
        assert_eq!(values[1]["request_id"], "request-7");
        assert_eq!(values[1]["phase"], "destination");
    }

    #[test]
    fn cancel_all_does_not_leave_live_tokens() {
        let requests = PreflightRequests::default();
        let token = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");

        assert_eq!(requests.cancel_all(), 1);
        assert!(token.is_cancelled());
        assert_eq!(requests.cancel_all(), 0);
        requests.complete(PreflightRequestKind::ExtractPlan, "main", "request", &token);
        requests.wait_idle();
    }

    #[test]
    fn wait_idle_tracks_replaced_and_cancelled_workers_until_completion() {
        let requests = Arc::new(PreflightRequests::default());
        let first = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");
        let replacement = requests.begin(PreflightRequestKind::ExtractPlan, "main", "request");
        assert!(first.is_cancelled());

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiting = Arc::clone(&requests);
        let waiter = std::thread::spawn(move || {
            waiting.wait_idle();
            let _ = done_tx.send(());
        });

        requests.complete(PreflightRequestKind::ExtractPlan, "main", "request", &first);
        assert!(done_rx.try_recv().is_err());
        requests.complete(
            PreflightRequestKind::ExtractPlan,
            "main",
            "request",
            &replacement,
        );
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(waiter.join().is_ok());
    }

    #[test]
    fn request_lease_completes_the_worker_when_dropped() {
        let requests = Arc::new(PreflightRequests::default());
        let request = requests.begin_request(PreflightRequestKind::ExtractPlan, "main", "request");
        let control = request.control();

        assert!(requests.cancel(PreflightRequestKind::ExtractPlan, "main", "request"));
        assert!(control.is_cancelled());
        drop(request);
        requests.wait_idle();
        assert!(!requests.cancel(PreflightRequestKind::ExtractPlan, "main", "request"));
    }
}
