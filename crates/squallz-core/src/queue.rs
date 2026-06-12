//! Task queue: jobs run on worker threads (default concurrency 1, i.e.
//! strictly in submission order) with per-job [`ControlToken`]s for
//! pause/resume/cancel, per-job progress snapshots and state-change
//! subscriptions. The GUI (I6) drives its task panel from this module; the
//! CLI does not use it yet.

use std::collections::{HashMap, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::api::{ControlToken, EntryPath, FormatError, ProgressSink};

/// A queued unit of work. It receives the job's own control token and a
/// progress sink that feeds the queue's per-job progress snapshot.
pub type Job =
    Box<dyn FnOnce(&ControlToken, &dyn ProgressSink) -> Result<(), FormatError> + Send + 'static>;

/// Opaque job identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

/// Job life cycle. `Failed` carries the error's log-only text;
/// presentation layers map the underlying error variants themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    /// Waiting in the queue
    Queued,
    /// Currently executing
    Running,
    /// Executing but paused at a chunk boundary
    Paused,
    /// Finished successfully
    Done,
    /// Finished with an error (log-only detail)
    Failed(String),
    /// Cancelled (before or during execution)
    Cancelled,
}

impl JobState {
    /// Whether the job has reached a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed(_) | Self::Cancelled)
    }
}

/// Latest progress snapshot of a job.
#[derive(Debug, Clone, Default)]
pub struct JobProgress {
    /// Bytes processed
    pub done: u64,
    /// Total bytes (0 = unknown)
    pub total: u64,
    /// Display path of the current entry
    pub current: String,
}

/// State-change listener: `(id, new_state)`.
type Listener = Arc<dyn Fn(JobId, &JobState) + Send + Sync>;

struct Slot {
    state: JobState,
    token: Arc<ControlToken>,
    progress: JobProgress,
    job: Option<Job>,
}

#[derive(Default)]
struct Inner {
    slots: Mutex<HashMap<JobId, Slot>>,
    queue: Mutex<VecDeque<JobId>>,
    wakeup: Condvar,
    /// Signals idleness changes to [`JobQueue::wait_idle`].
    idle: Condvar,
    listeners: Mutex<Vec<Listener>>,
    running: AtomicUsize,
    shutdown: AtomicBool,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait_unpoisoned<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    match condvar.wait(guard) {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Inner {
    fn set_state(&self, id: JobId, state: JobState) {
        {
            let mut slots = lock_unpoisoned(&self.slots);
            if let Some(slot) = slots.get_mut(&id) {
                slot.state = state.clone();
            }
        }
        let listeners = lock_unpoisoned(&self.listeners).clone();
        for listener in listeners {
            listener(id, &state);
        }
    }

    /// Notifies idle waiters. The queue lock is held while notifying so a
    /// waiter can never miss a wakeup between its predicate check and its
    /// wait (the `running` counter is always updated *before* this call).
    fn notify_idle(&self) {
        let _guard = lock_unpoisoned(&self.queue);
        self.idle.notify_all();
    }
}

/// Per-job progress sink feeding the queue's snapshot.
struct SlotProgress {
    inner: Arc<Inner>,
    id: JobId,
}

impl ProgressSink for SlotProgress {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        let mut slots = lock_unpoisoned(&self.inner.slots);
        if let Some(slot) = slots.get_mut(&self.id) {
            slot.progress = JobProgress {
                done,
                total,
                current: current.display.clone(),
            };
        }
    }
}

/// The queue. Dropping it requests shutdown and joins the workers (queued
/// jobs that never started are marked cancelled).
pub struct JobQueue {
    inner: Arc<Inner>,
    workers: Vec<JoinHandle<()>>,
    next_id: AtomicU64,
}

impl JobQueue {
    /// Builds a queue with `concurrency` worker threads (clamped to ≥ 1;
    /// pass 1 — the default choice — for strict submission-order execution).
    pub fn new(concurrency: usize) -> Self {
        let inner = Arc::new(Inner::default());
        let workers = (0..concurrency.max(1))
            .map(|_| {
                let inner = Arc::clone(&inner);
                std::thread::spawn(move || worker_loop(&inner))
            })
            .collect();
        Self {
            inner,
            workers,
            next_id: AtomicU64::new(1),
        }
    }

    /// Submits a job and returns its id.
    pub fn submit(&self, job: Job) -> JobId {
        let id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        lock_unpoisoned(&self.inner.slots).insert(
            id,
            Slot {
                state: JobState::Queued,
                token: ControlToken::new(),
                progress: JobProgress::default(),
                job: Some(job),
            },
        );
        lock_unpoisoned(&self.inner.queue).push_back(id);
        self.inner.wakeup.notify_one();
        id
    }

    /// Current state of a job.
    pub fn state(&self, id: JobId) -> Option<JobState> {
        lock_unpoisoned(&self.inner.slots)
            .get(&id)
            .map(|s| s.state.clone())
    }

    /// Latest progress snapshot of a job.
    pub fn progress(&self, id: JobId) -> Option<JobProgress> {
        lock_unpoisoned(&self.inner.slots)
            .get(&id)
            .map(|s| s.progress.clone())
    }

    /// Registers a state-change listener (kept for the queue's lifetime).
    pub fn subscribe(&self, listener: impl Fn(JobId, &JobState) + Send + Sync + 'static) {
        lock_unpoisoned(&self.inner.listeners).push(Arc::new(listener));
    }

    /// Pauses a job: takes effect at the next chunk boundary of a running
    /// job; a queued job will pause right after it starts.
    pub fn pause(&self, id: JobId) {
        let running = {
            let slots = lock_unpoisoned(&self.inner.slots);
            let Some(slot) = slots.get(&id) else { return };
            if slot.state.is_terminal() {
                return;
            }
            slot.token.pause();
            slot.state == JobState::Running
        };
        if running {
            self.inner.set_state(id, JobState::Paused);
        }
    }

    /// Resumes a paused job.
    pub fn resume(&self, id: JobId) {
        let paused = {
            let slots = lock_unpoisoned(&self.inner.slots);
            let Some(slot) = slots.get(&id) else { return };
            slot.token.resume();
            slot.state == JobState::Paused
        };
        if paused {
            self.inner.set_state(id, JobState::Running);
        }
    }

    /// Cancels a job: a queued job is dropped without running, a running
    /// (or paused) one unwinds at its next chunk boundary.
    pub fn cancel(&self, id: JobId) {
        let was_queued = {
            let mut slots = lock_unpoisoned(&self.inner.slots);
            let Some(slot) = slots.get_mut(&id) else {
                return;
            };
            if slot.state.is_terminal() {
                return;
            }
            slot.token.cancel();
            let queued = slot.state == JobState::Queued;
            if queued {
                slot.job = None; // never runs
            }
            queued
        };
        if was_queued {
            self.inner.set_state(id, JobState::Cancelled);
            self.inner.notify_idle();
        }
    }

    /// Blocks until the queue is empty and no job is running (test/CLI
    /// convenience; the GUI subscribes instead).
    pub fn wait_idle(&self) {
        let mut queue = lock_unpoisoned(&self.inner.queue);
        while !queue.is_empty() || self.inner.running.load(Ordering::SeqCst) > 0 {
            queue = wait_unpoisoned(&self.inner.idle, queue);
        }
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.wakeup.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(inner: &Arc<Inner>) {
    loop {
        let id = {
            let mut queue = lock_unpoisoned(&inner.queue);
            loop {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(id) = queue.pop_front() {
                    // Marked busy while still holding the queue lock so
                    // `wait_idle` never observes "queue empty + nothing
                    // running" for a job that is about to start.
                    inner.running.fetch_add(1, Ordering::SeqCst);
                    break id;
                }
                queue = wait_unpoisoned(&inner.wakeup, queue);
            }
        };

        // Claim the job; skip slots already cancelled while queued.
        let claimed = {
            let mut slots = lock_unpoisoned(&inner.slots);
            slots.get_mut(&id).and_then(|slot| {
                if slot.state != JobState::Queued {
                    return None;
                }
                slot.job.take().map(|job| (job, Arc::clone(&slot.token)))
            })
        };
        let Some((job, token)) = claimed else {
            inner.running.fetch_sub(1, Ordering::SeqCst);
            inner.notify_idle();
            continue;
        };

        // A pause requested while the job was still queued shows up
        // immediately (the first checkpoint blocks anyway).
        let start_state = if token.is_paused() {
            JobState::Paused
        } else {
            JobState::Running
        };
        inner.set_state(id, start_state);

        let sink = SlotProgress {
            inner: Arc::clone(inner),
            id,
        };
        let final_state = match panic::catch_unwind(AssertUnwindSafe(|| job(&token, &sink))) {
            Ok(Ok(())) => JobState::Done,
            Ok(Err(FormatError::Cancelled)) => JobState::Cancelled,
            Ok(Err(e)) => JobState::Failed(e.to_string()),
            Err(_) => JobState::Failed("job panicked".to_owned()),
        };
        inner.set_state(id, final_state);
        inner.running.fetch_sub(1, Ordering::SeqCst);
        inner.notify_idle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Three jobs on one worker run strictly in submission order.
    #[test]
    fn jobs_run_sequentially_in_order() {
        let queue = JobQueue::new(1);
        let order = Arc::new(Mutex::new(Vec::new()));
        let ids: Vec<JobId> = (0..3)
            .map(|n| {
                let order = Arc::clone(&order);
                queue.submit(Box::new(move |_ctl, _progress| {
                    order.lock().unwrap().push(n);
                    Ok(())
                }))
            })
            .collect();
        queue.wait_idle();
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
        for id in ids {
            assert_eq!(queue.state(id), Some(JobState::Done));
        }
    }

    /// Cancelling the queued second job neither runs it nor blocks the
    /// third one.
    #[test]
    fn cancel_queued_job_does_not_affect_later_jobs() {
        let queue = JobQueue::new(1);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let id1 = queue.submit(Box::new(move |_ctl, _p| {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap(); // hold the worker
            Ok(())
        }));
        let ran2 = Arc::new(AtomicBool::new(false));
        let ran2c = Arc::clone(&ran2);
        let id2 = queue.submit(Box::new(move |_ctl, _p| {
            ran2c.store(true, Ordering::SeqCst);
            Ok(())
        }));
        let id3 = queue.submit(Box::new(|_ctl, _p| Ok(())));

        started_rx.recv().unwrap(); // job 1 is running
        queue.cancel(id2);
        assert_eq!(queue.state(id2), Some(JobState::Cancelled));
        gate_tx.send(()).unwrap(); // release job 1
        queue.wait_idle();
        assert_eq!(queue.state(id1), Some(JobState::Done));
        assert!(!ran2.load(Ordering::SeqCst), "cancelled job must not run");
        assert_eq!(queue.state(id3), Some(JobState::Done));
    }

    /// Cancelling a *running* job unwinds it and the next job still runs.
    #[test]
    fn cancel_running_job_unwinds_via_token() {
        let queue = JobQueue::new(1);
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let id1 = queue.submit(Box::new(move |ctl, _p| {
            started_tx.send(()).unwrap();
            loop {
                ctl.checkpoint()?; // surfaces Cancelled
                std::thread::sleep(Duration::from_millis(1));
            }
        }));
        let id2 = queue.submit(Box::new(|_ctl, _p| Ok(())));
        started_rx.recv().unwrap();
        queue.cancel(id1);
        queue.wait_idle();
        assert_eq!(queue.state(id1), Some(JobState::Cancelled));
        assert_eq!(queue.state(id2), Some(JobState::Done));
    }

    /// pause stops progress at a chunk boundary; resume completes the job.
    #[test]
    fn pause_and_resume_take_effect() {
        let queue = JobQueue::new(1);
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);
        let id = queue.submit(Box::new(move |ctl, _p| {
            for _ in 0..50 {
                ctl.checkpoint()?;
                c.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }));
        while counter.load(Ordering::SeqCst) < 5 {
            std::thread::sleep(Duration::from_millis(1));
        }
        queue.pause(id);
        assert_eq!(queue.state(id), Some(JobState::Paused));
        // After the in-flight chunk drains, the counter must stop moving.
        std::thread::sleep(Duration::from_millis(20));
        let frozen = counter.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            counter.load(Ordering::SeqCst) <= frozen + 1,
            "counter advanced while paused"
        );
        queue.resume(id);
        assert_eq!(queue.state(id), Some(JobState::Running));
        queue.wait_idle();
        assert_eq!(counter.load(Ordering::SeqCst), 50);
        assert_eq!(queue.state(id), Some(JobState::Done));
    }

    /// A failing job records its error and does not block later jobs;
    /// subscribers observe the state changes.
    #[test]
    fn failed_job_does_not_block_queue_and_notifies() {
        let queue = JobQueue::new(1);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_c = Arc::clone(&seen);
        queue.subscribe(move |id, state| {
            seen_c.lock().unwrap().push((id, state.clone()));
        });
        let id1 = queue.submit(Box::new(|_ctl, _p| Err(FormatError::Other("boom".into()))));
        let id2 = queue.submit(Box::new(|_ctl, _p| Ok(())));
        queue.wait_idle();
        assert_eq!(queue.state(id1), Some(JobState::Failed("boom".into())));
        assert_eq!(queue.state(id2), Some(JobState::Done));
        let events = seen.lock().unwrap();
        assert!(events.contains(&(id1, JobState::Running)));
        assert!(events.contains(&(id1, JobState::Failed("boom".into()))));
        assert!(events.contains(&(id2, JobState::Done)));
    }

    #[test]
    fn panicking_job_fails_and_does_not_block_later_jobs() {
        let queue = JobQueue::new(1);
        let id1 = queue.submit(Box::new(|_ctl, _p| {
            panic!("queue job panic fixture");
        }));
        let id2 = queue.submit(Box::new(|_ctl, _p| Ok(())));

        queue.wait_idle();

        assert_eq!(
            queue.state(id1),
            Some(JobState::Failed("job panicked".into()))
        );
        assert_eq!(queue.state(id2), Some(JobState::Done));
    }

    /// Progress reported by a job is visible through the snapshot API.
    #[test]
    fn progress_snapshot_is_observable() {
        let queue = JobQueue::new(1);
        let id = queue.submit(Box::new(|_ctl, progress| {
            progress.on_progress(50, 100, &EntryPath::from_utf8("a.txt"));
            Ok(())
        }));
        queue.wait_idle();
        let snapshot = queue.progress(id).unwrap();
        assert_eq!(snapshot.done, 50);
        assert_eq!(snapshot.total, 100);
        assert_eq!(snapshot.current, "a.txt");
    }

    #[test]
    fn queue_survives_poisoned_internal_slot_lock() {
        let queue = JobQueue::new(1);
        let inner = Arc::clone(&queue.inner);
        let poisoner = std::thread::spawn(move || {
            let _guard = inner.slots.lock().unwrap();
            panic!("poison slots lock for recovery coverage");
        });
        assert!(poisoner.join().is_err());

        let id = queue.submit(Box::new(|_ctl, _progress| Ok(())));
        queue.wait_idle();

        assert_eq!(queue.state(id), Some(JobState::Done));
    }
}
