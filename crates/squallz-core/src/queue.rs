//! Task queue: jobs run on worker threads with a configurable parallel-job
//! ceiling and CPU budget, per-job [`ControlToken`]s for pause/resume/cancel,
//! per-job progress snapshots and state-change subscriptions. A one-worker
//! queue remains strictly ordered. The GUI drives its task panel from this
//! module; the CLI does not use it yet.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::api::{ControlToken, EntryPath, FormatError, ProgressPhase, ProgressSink};
use crate::lock_unpoisoned;

/// A queued unit of work. It receives the job's own control token and a
/// progress sink that feeds the queue's per-job progress snapshot.
pub type Job =
    Box<dyn FnOnce(&ControlToken, &dyn ProgressSink) -> Result<(), FormatError> + Send + 'static>;

/// CPU capacity a queued job reserves while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobResources {
    /// Logical CPU threads expected to be occupied by the job.
    pub cpu_threads: usize,
}

impl JobResources {
    /// Builds a resource request, clamping an empty request to one thread.
    pub fn new(cpu_threads: usize) -> Self {
        Self {
            cpu_threads: cpu_threads.max(1),
        }
    }
}

impl Default for JobResources {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Why a queued job cannot start with the queue's current resource state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueWaitReason {
    /// The configured simultaneous-job ceiling is already reserved.
    ParallelLimit,
    /// Starting this job would exceed the logical CPU-thread budget.
    CpuBudget,
    /// An earlier queued job is waiting for resources and retains FIFO priority.
    QueueOrder,
}

/// Live scheduling information for one unpaused queued job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedJobStatus {
    /// Queue identifier.
    pub id: JobId,
    /// One-based position among unpaused queued jobs.
    pub position: usize,
    /// Logical CPU threads the scheduler will reserve for this job.
    pub cpu_threads: usize,
    /// `None` means enough capacity exists for a worker to claim the job now.
    pub wait_reason: Option<QueueWaitReason>,
}

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
    interruptible: bool,
    resources: JobResources,
    job: Option<Job>,
}

struct Inner {
    slots: Mutex<HashMap<JobId, Slot>>,
    queue: Mutex<VecDeque<JobId>>,
    wakeup: Condvar,
    /// Signals idleness changes to [`JobQueue::wait_idle`].
    idle: Condvar,
    listeners: Mutex<Vec<Listener>>,
    running: AtomicUsize,
    cpu_threads_in_use: AtomicUsize,
    max_running: AtomicUsize,
    max_workers: usize,
    cpu_thread_budget: usize,
    shutdown: AtomicBool,
}

impl Inner {
    fn new(max_workers: usize, max_running: usize, cpu_thread_budget: usize) -> Self {
        let max_workers = max_workers.max(1);
        Self {
            slots: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            wakeup: Condvar::new(),
            idle: Condvar::new(),
            listeners: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            cpu_threads_in_use: AtomicUsize::new(0),
            max_running: AtomicUsize::new(max_running.clamp(1, max_workers)),
            max_workers,
            cpu_thread_budget: cpu_thread_budget.max(1),
            shutdown: AtomicBool::new(false),
        }
    }
}

impl Default for Inner {
    fn default() -> Self {
        Self::new(1, 1, 1)
    }
}

fn wait_unpoisoned<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    match condvar.wait(guard) {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Inner {
    fn claim_job(&self, id: JobId) -> Option<(Job, Arc<ControlToken>, JobState)> {
        let mut slots = lock_unpoisoned(&self.slots);
        let slot = slots.get_mut(&id)?;
        if slot.state != JobState::Queued {
            return None;
        }
        let job = slot.job.take()?;
        let state = if slot.token.is_paused() {
            JobState::Paused
        } else {
            JobState::Running
        };
        slot.state = state.clone();
        Some((job, Arc::clone(&slot.token), state))
    }

    fn request_cancel_many(&self, ids: &[JobId]) -> (Vec<JobId>, Vec<JobId>) {
        let mut queue = lock_unpoisoned(&self.queue);
        let mut slots = lock_unpoisoned(&self.slots);
        let mut requested = Vec::with_capacity(ids.len());
        let mut cancelled_queued = Vec::new();

        for id in ids {
            let Some(slot) = slots.get_mut(id) else {
                continue;
            };
            if slot.state.is_terminal() || !slot.interruptible || slot.token.is_cancelled() {
                continue;
            }
            slot.token.cancel();
            requested.push(*id);
            if slot.state == JobState::Queued && slot.job.take().is_some() {
                slot.state = JobState::Cancelled;
                cancelled_queued.push(*id);
            }
        }

        if !cancelled_queued.is_empty() {
            let cancelled = cancelled_queued.iter().copied().collect::<HashSet<_>>();
            queue.retain(|id| !cancelled.contains(id));
        }
        (requested, cancelled_queued)
    }

    fn notify_state(&self, id: JobId, state: &JobState) {
        let listeners = lock_unpoisoned(&self.listeners).clone();
        for listener in listeners {
            listener(id, state);
        }
    }

    fn set_state(&self, id: JobId, state: JobState) {
        {
            let mut slots = lock_unpoisoned(&self.slots);
            if let Some(slot) = slots.get_mut(&id) {
                slot.state = state.clone();
            }
        }
        self.notify_state(id, &state);
    }

    /// Notifies workers and idle waiters while holding the queue lock so a
    /// waiter cannot miss a state transition between its check and wait.
    fn notify_waiters(&self) {
        let _guard = lock_unpoisoned(&self.queue);
        self.wakeup.notify_all();
        self.idle.notify_all();
    }

    fn reserve_next_job(&self, queue: &mut VecDeque<JobId>) -> Option<(JobId, usize)> {
        if self.running.load(Ordering::SeqCst) >= self.max_running.load(Ordering::SeqCst) {
            return None;
        }

        let mut stale = Vec::new();
        let mut candidate = None;
        let slots = lock_unpoisoned(&self.slots);
        for (index, id) in queue.iter().copied().enumerate() {
            let Some(slot) = slots.get(&id) else {
                stale.push(index);
                continue;
            };
            if !reorderable_slot(slot) {
                if slot.state.is_terminal() || slot.job.is_none() {
                    stale.push(index);
                }
                continue;
            }

            let reserved_threads = slot.resources.cpu_threads.min(self.cpu_thread_budget);
            let used_threads = self.cpu_threads_in_use.load(Ordering::SeqCst);
            if used_threads > 0
                && used_threads.saturating_add(reserved_threads) > self.cpu_thread_budget
            {
                break;
            }
            candidate = Some((index, reserved_threads));
            break;
        }
        drop(slots);

        let removed_before_candidate = candidate
            .map(|(index, _)| {
                stale
                    .iter()
                    .filter(|stale_index| **stale_index < index)
                    .count()
            })
            .unwrap_or_default();
        for index in stale.into_iter().rev() {
            queue.remove(index);
        }
        let (index, reserved_threads) = candidate?;
        let id = queue.remove(index.saturating_sub(removed_before_candidate))?;
        self.running.fetch_add(1, Ordering::SeqCst);
        self.cpu_threads_in_use
            .fetch_add(reserved_threads, Ordering::SeqCst);
        Some((id, reserved_threads))
    }

    fn release_resources(&self, cpu_threads: usize) {
        let _guard = lock_unpoisoned(&self.queue);
        self.cpu_threads_in_use
            .fetch_sub(cpu_threads, Ordering::SeqCst);
        self.running.fetch_sub(1, Ordering::SeqCst);
        self.wakeup.notify_all();
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
            slot.progress.done = done;
            slot.progress.total = total;
            slot.progress.current.clone_from(&current.display);
        }
    }

    fn on_phase(&self, _phase: ProgressPhase, interruptible: bool) {
        let mut slots = lock_unpoisoned(&self.inner.slots);
        if let Some(slot) = slots.get_mut(&self.id) {
            slot.interruptible = interruptible;
        }
    }
}

/// The queue. Dropping it requests shutdown, joins the workers, and discards
/// queued jobs that never started.
pub struct JobQueue {
    inner: Arc<Inner>,
    workers: Vec<JoinHandle<()>>,
    next_id: AtomicU64,
}

impl JobQueue {
    /// Builds a queue with `concurrency` worker threads (clamped to ≥ 1;
    /// pass 1 — the default choice — for strict submission-order execution).
    pub fn new(concurrency: usize) -> Self {
        let concurrency = concurrency.max(1);
        Self::with_resource_limits(concurrency, concurrency, concurrency)
    }

    /// Builds a queue whose worker pool, simultaneous-job ceiling and logical
    /// CPU budget can be tuned independently.
    pub fn with_resource_limits(
        worker_threads: usize,
        max_running: usize,
        cpu_thread_budget: usize,
    ) -> Self {
        let worker_threads = worker_threads.max(1);
        let inner = Arc::new(Inner::new(worker_threads, max_running, cpu_thread_budget));
        let workers = (0..worker_threads)
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
        self.submit_with_resources(job, JobResources::default())
    }

    /// Submits a job with the CPU capacity it should reserve while running.
    pub fn submit_with_resources(&self, job: Job, resources: JobResources) -> JobId {
        let id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        lock_unpoisoned(&self.inner.slots).insert(
            id,
            Slot {
                state: JobState::Queued,
                token: ControlToken::new(),
                progress: JobProgress::default(),
                interruptible: true,
                resources,
                job: Some(job),
            },
        );
        lock_unpoisoned(&self.inner.queue).push_back(id);
        self.inner.wakeup.notify_all();
        id
    }

    /// Changes the simultaneous-job ceiling. Running jobs are never cancelled;
    /// a lower limit takes effect before the next queued job starts.
    pub fn set_max_running(&self, max_running: usize) {
        let _guard = lock_unpoisoned(&self.inner.queue);
        self.inner.max_running.store(
            max_running.clamp(1, self.inner.max_workers),
            Ordering::SeqCst,
        );
        self.inner.wakeup.notify_all();
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

    /// Removes a completed slot after a presentation layer has retained any
    /// result it still needs. Active jobs are never removed.
    pub fn forget_terminal(&self, id: JobId) -> bool {
        let mut slots = lock_unpoisoned(&self.inner.slots);
        if !slots.get(&id).is_some_and(|slot| slot.state.is_terminal()) {
            return false;
        }
        slots.remove(&id);
        true
    }

    /// Registers a state-change listener (kept for the queue's lifetime).
    pub fn subscribe(&self, listener: impl Fn(JobId, &JobState) + Send + Sync + 'static) {
        lock_unpoisoned(&self.inner.listeners).push(Arc::new(listener));
    }

    /// Pauses a job: takes effect at the next chunk boundary of a running job;
    /// a queued job remains queued without occupying a worker until resumed.
    pub fn pause(&self, id: JobId) {
        let _ = self.try_pause(id);
    }

    /// Attempts to pause a job, returning `false` when its current phase can no
    /// longer be interrupted safely or the job is unavailable.
    pub fn try_pause(&self, id: JobId) -> bool {
        let running = {
            let slots = lock_unpoisoned(&self.inner.slots);
            let Some(slot) = slots.get(&id) else {
                return false;
            };
            if slot.state.is_terminal() || !slot.interruptible {
                return false;
            }
            slot.token.pause();
            slot.state == JobState::Running
        };
        if running {
            self.inner.set_state(id, JobState::Paused);
        }
        true
    }

    /// Resumes a paused job.
    pub fn resume(&self, id: JobId) {
        let (paused, queued) = {
            let slots = lock_unpoisoned(&self.inner.slots);
            let Some(slot) = slots.get(&id) else { return };
            slot.token.resume();
            (
                slot.state == JobState::Paused,
                slot.state == JobState::Queued && slot.job.is_some(),
            )
        };
        if paused {
            self.inner.set_state(id, JobState::Running);
        } else if queued {
            self.inner.notify_waiters();
        }
    }

    /// Cancels a job: a queued job is dropped without running, a running
    /// (or paused) one unwinds at its next chunk boundary.
    pub fn cancel(&self, id: JobId) {
        let _ = self.try_cancel(id);
    }

    /// Moves an unpaused queued job one place earlier among the other
    /// reorderable jobs. Jobs already claimed by a worker are never moved.
    pub fn move_queued_earlier(&self, id: JobId) -> bool {
        self.move_queued(id, QueueDirection::Earlier)
    }

    /// Moves an unpaused queued job one place later among the other
    /// reorderable jobs. Jobs already claimed by a worker are never moved.
    pub fn move_queued_later(&self, id: JobId) -> bool {
        self.move_queued(id, QueueDirection::Later)
    }

    /// Places an unpaused queued job immediately before another reorderable
    /// job, or at the end of the reorderable queue when `before` is `None`.
    /// Jobs already claimed by a worker and paused queued jobs keep their
    /// physical queue slots.
    pub fn move_queued_before(&self, id: JobId, before: Option<JobId>) -> bool {
        if before == Some(id) {
            return false;
        }

        // Holding the queue lock prevents a worker from claiming either job
        // while the requested order is validated and applied.
        let mut queue = lock_unpoisoned(&self.inner.queue);
        let slots = lock_unpoisoned(&self.inner.slots);
        let reorderable = reorderable_queue_entries(&queue, &slots);
        let original = reorderable
            .iter()
            .map(|(_, candidate)| *candidate)
            .collect::<Vec<_>>();
        let Some(source_index) = original.iter().position(|candidate| *candidate == id) else {
            return false;
        };
        if before.is_some_and(|target| !original.contains(&target)) {
            return false;
        }

        let mut ordered = original.clone();
        ordered.remove(source_index);
        let insertion_index = match before {
            Some(target) => {
                let Some(index) = ordered.iter().position(|candidate| *candidate == target) else {
                    return false;
                };
                index
            }
            None => ordered.len(),
        };
        ordered.insert(insertion_index, id);
        if ordered == original {
            return true;
        }

        for ((queue_index, _), candidate) in reorderable.iter().zip(ordered) {
            queue[*queue_index] = candidate;
        }
        true
    }

    /// Returns unpaused queued jobs in their current execution order.
    pub fn reorderable_job_ids(&self) -> Vec<JobId> {
        self.queued_job_statuses()
            .into_iter()
            .map(|status| status.id)
            .collect()
    }

    /// Returns live scheduling status for every unpaused queued job.
    ///
    /// Capacity is simulated in queue order while the queue lock is held, so
    /// multiple jobs may be immediately runnable. Once a job is resource
    /// blocked, later jobs retain FIFO priority instead of bypassing it.
    pub fn queued_job_statuses(&self) -> Vec<QueuedJobStatus> {
        let queue = lock_unpoisoned(&self.inner.queue);
        let slots = lock_unpoisoned(&self.inner.slots);
        let mut running = self.inner.running.load(Ordering::SeqCst);
        let max_running = self.inner.max_running.load(Ordering::SeqCst);
        let mut cpu_threads_in_use = self.inner.cpu_threads_in_use.load(Ordering::SeqCst);
        let mut earlier_blocked = false;

        reorderable_queue_entries(&queue, &slots)
            .into_iter()
            .enumerate()
            .filter_map(|(index, (_, id))| {
                let slot = slots.get(&id)?;
                let cpu_threads = slot.resources.cpu_threads.min(self.inner.cpu_thread_budget);
                let wait_reason = if earlier_blocked {
                    Some(QueueWaitReason::QueueOrder)
                } else if running >= max_running {
                    earlier_blocked = true;
                    Some(QueueWaitReason::ParallelLimit)
                } else if cpu_threads_in_use > 0
                    && cpu_threads_in_use.saturating_add(cpu_threads) > self.inner.cpu_thread_budget
                {
                    earlier_blocked = true;
                    Some(QueueWaitReason::CpuBudget)
                } else {
                    running = running.saturating_add(1);
                    cpu_threads_in_use = cpu_threads_in_use.saturating_add(cpu_threads);
                    None
                };
                Some(QueuedJobStatus {
                    id,
                    position: index.saturating_add(1),
                    cpu_threads,
                    wait_reason,
                })
            })
            .collect()
    }

    fn move_queued(&self, id: JobId, direction: QueueDirection) -> bool {
        // The queue lock prevents a worker from claiming the target while its
        // slot and neighbors are checked. No other path holds the slot lock
        // while waiting for the queue lock.
        let mut queue = lock_unpoisoned(&self.inner.queue);
        let slots = lock_unpoisoned(&self.inner.slots);
        let reorderable = reorderable_queue_entries(&queue, &slots);
        let Some(position) = reorderable
            .iter()
            .position(|(_, candidate)| *candidate == id)
        else {
            return false;
        };
        let neighbor = match direction {
            QueueDirection::Earlier => position.checked_sub(1),
            QueueDirection::Later => position
                .checked_add(1)
                .filter(|neighbor| *neighbor < reorderable.len()),
        };
        let Some(neighbor) = neighbor else {
            return false;
        };
        queue.swap(reorderable[position].0, reorderable[neighbor].0);
        true
    }

    /// Attempts to cancel a job, returning `false` when its current phase can
    /// no longer be interrupted safely or the job is unavailable.
    pub fn try_cancel(&self, id: JobId) -> bool {
        !self.try_cancel_many(&[id]).is_empty()
    }

    /// Attempts to cancel a group as one scheduling transaction. Every
    /// cancellable queued job is removed before running work can release
    /// capacity for another job in the same group.
    pub fn try_cancel_many(&self, ids: &[JobId]) -> Vec<JobId> {
        let (requested, cancelled_queued) = self.inner.request_cancel_many(ids);
        for id in &cancelled_queued {
            self.inner.notify_state(*id, &JobState::Cancelled);
        }
        if !cancelled_queued.is_empty() {
            self.inner.notify_waiters();
        }
        requested
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

#[derive(Clone, Copy)]
enum QueueDirection {
    Earlier,
    Later,
}

fn reorderable_slot(slot: &Slot) -> bool {
    slot.state == JobState::Queued && slot.job.is_some() && !slot.token.is_paused()
}

fn reorderable_queue_entries(
    queue: &VecDeque<JobId>,
    slots: &HashMap<JobId, Slot>,
) -> Vec<(usize, JobId)> {
    queue
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            slots
                .get(candidate)
                .is_some_and(reorderable_slot)
                .then_some((index, *candidate))
        })
        .collect()
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        {
            // Publish the predicate under the condvar mutex so a worker cannot
            // miss shutdown between its predicate check and wait.
            let _guard = lock_unpoisoned(&self.inner.queue);
            self.inner.shutdown.store(true, Ordering::SeqCst);
            self.inner.wakeup.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(inner: &Arc<Inner>) {
    loop {
        let (id, reserved_threads) = {
            let mut queue = lock_unpoisoned(&inner.queue);
            loop {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(reservation) = inner.reserve_next_job(&mut queue) {
                    break reservation;
                }
                inner.idle.notify_all();
                queue = wait_unpoisoned(&inner.wakeup, queue);
            }
        };

        // Claiming the closure and publishing its running state are one locked
        // transition. Cancellation therefore sees either a queued closure it
        // can remove or a running job it must stop cooperatively.
        let claimed = inner.claim_job(id);
        let Some((job, token, start_state)) = claimed else {
            inner.release_resources(reserved_threads);
            continue;
        };

        inner.notify_state(id, &start_state);

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
        inner.release_resources(reserved_threads);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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

    #[test]
    fn independent_jobs_run_in_parallel_within_cpu_budget() {
        let queue = JobQueue::with_resource_limits(2, 2, 2);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (release_second_tx, release_second_rx) = mpsc::channel();

        let first_started = started_tx.clone();
        queue.submit_with_resources(
            Box::new(move |_ctl, _progress| {
                first_started.send(1).unwrap();
                release_first_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Ok(())
            }),
            JobResources::new(1),
        );
        queue.submit_with_resources(
            Box::new(move |_ctl, _progress| {
                started_tx.send(2).unwrap();
                release_second_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                Ok(())
            }),
            JobResources::new(1),
        );

        let mut started = vec![
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        started.sort_unstable();
        assert_eq!(started, vec![1, 2]);
        release_first_tx.send(()).unwrap();
        release_second_tx.send(()).unwrap();
        queue.wait_idle();
    }

    #[test]
    fn cpu_budget_holds_the_next_expensive_job() {
        let queue = JobQueue::with_resource_limits(2, 2, 4);
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        queue.submit_with_resources(
            Box::new(move |_ctl, _progress| {
                first_started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(())
            }),
            JobResources::new(3),
        );
        queue.submit_with_resources(
            Box::new(move |_ctl, _progress| {
                second_started_tx.send(()).unwrap();
                Ok(())
            }),
            JobResources::new(2),
        );

        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(second_started_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        release_tx.send(()).unwrap();
        second_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        queue.wait_idle();
    }

    #[test]
    fn queued_status_explains_cpu_and_fifo_waits() {
        let queue = JobQueue::with_resource_limits(3, 3, 4);
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        queue.submit_with_resources(
            Box::new(move |_ctl, _progress| {
                first_started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(())
            }),
            JobResources::new(3),
        );
        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let second =
            queue.submit_with_resources(Box::new(|_ctl, _progress| Ok(())), JobResources::new(2));
        let third =
            queue.submit_with_resources(Box::new(|_ctl, _progress| Ok(())), JobResources::new(1));

        assert_eq!(
            queue.queued_job_statuses(),
            vec![
                QueuedJobStatus {
                    id: second,
                    position: 1,
                    cpu_threads: 2,
                    wait_reason: Some(QueueWaitReason::CpuBudget),
                },
                QueuedJobStatus {
                    id: third,
                    position: 2,
                    cpu_threads: 1,
                    wait_reason: Some(QueueWaitReason::QueueOrder),
                },
            ]
        );

        release_tx.send(()).unwrap();
        queue.wait_idle();
    }

    #[test]
    fn queued_status_explains_parallel_limit() {
        let queue = JobQueue::with_resource_limits(2, 1, 4);
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        queue.submit(Box::new(move |_ctl, _progress| {
            first_started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(())
        }));
        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let second = queue.submit(Box::new(|_ctl, _progress| Ok(())));

        assert_eq!(
            queue.queued_job_statuses(),
            vec![QueuedJobStatus {
                id: second,
                position: 1,
                cpu_threads: 1,
                wait_reason: Some(QueueWaitReason::ParallelLimit),
            }]
        );

        release_tx.send(()).unwrap();
        queue.wait_idle();
    }

    #[test]
    fn increasing_parallel_limit_starts_waiting_work() {
        let queue = JobQueue::with_resource_limits(2, 1, 2);
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (second_started_tx, second_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        queue.submit(Box::new(move |_ctl, _progress| {
            first_started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(())
        }));
        queue.submit(Box::new(move |_ctl, _progress| {
            second_started_tx.send(()).unwrap();
            Ok(())
        }));

        first_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(second_started_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        queue.set_max_running(2);
        second_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        release_tx.send(()).unwrap();
        queue.wait_idle();
    }

    #[test]
    fn lowering_parallel_limit_waits_for_running_jobs_without_cancelling_them() {
        let queue = JobQueue::with_resource_limits(2, 2, 2);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let (release_second_tx, release_second_rx) = mpsc::channel();
        let (third_started_tx, third_started_rx) = mpsc::channel();

        let first_started = started_tx.clone();
        let first = queue.submit(Box::new(move |_ctl, _progress| {
            first_started.send(1).unwrap();
            release_first_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            Ok(())
        }));
        let second = queue.submit(Box::new(move |_ctl, _progress| {
            started_tx.send(2).unwrap();
            release_second_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            Ok(())
        }));

        let mut started = vec![
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        started.sort_unstable();
        assert_eq!(started, vec![1, 2]);

        queue.set_max_running(1);
        let third = queue.submit(Box::new(move |_ctl, _progress| {
            third_started_tx.send(()).unwrap();
            Ok(())
        }));
        release_first_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while queue.state(first) != Some(JobState::Done) {
            assert!(
                Instant::now() < deadline,
                "first running job did not complete"
            );
            std::thread::yield_now();
        }
        assert_eq!(queue.state(second), Some(JobState::Running));
        assert!(third_started_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());

        release_second_tx.send(()).unwrap();
        third_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        queue.wait_idle();
        assert_eq!(queue.state(second), Some(JobState::Done));
        assert_eq!(queue.state(third), Some(JobState::Done));
    }

    #[test]
    fn paused_queued_job_does_not_occupy_a_worker() {
        let queue = JobQueue::new(1);
        let (blocker_started_tx, blocker_started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (order_tx, order_rx) = mpsc::channel();

        queue.submit(Box::new(move |_ctl, _progress| {
            blocker_started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(())
        }));
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let paused_tx = order_tx.clone();
        let paused = queue.submit(Box::new(move |_ctl, _progress| {
            paused_tx.send(2).unwrap();
            Ok(())
        }));
        queue.submit(Box::new(move |_ctl, _progress| {
            order_tx.send(3).unwrap();
            Ok(())
        }));
        assert!(queue.try_pause(paused));

        release_tx.send(()).unwrap();
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 3);
        assert_eq!(queue.state(paused), Some(JobState::Queued));
        queue.resume(paused);
        assert_eq!(order_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        queue.wait_idle();
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

    #[test]
    fn batch_cancel_removes_queued_jobs_before_releasing_capacity() {
        let queue = JobQueue::new(1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let running = queue.submit(Box::new(move |ctl, _progress| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            ctl.checkpoint()
        }));
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let queued_ran = Arc::new(AtomicBool::new(false));
        let first_flag = Arc::clone(&queued_ran);
        let first_queued = queue.submit(Box::new(move |_ctl, _progress| {
            first_flag.store(true, Ordering::SeqCst);
            Ok(())
        }));
        let second_flag = Arc::clone(&queued_ran);
        let second_queued = queue.submit(Box::new(move |_ctl, _progress| {
            second_flag.store(true, Ordering::SeqCst);
            Ok(())
        }));

        assert_eq!(
            queue.try_cancel_many(&[running, first_queued, second_queued]),
            vec![running, first_queued, second_queued]
        );
        assert_eq!(queue.state(first_queued), Some(JobState::Cancelled));
        assert_eq!(queue.state(second_queued), Some(JobState::Cancelled));
        release_tx.send(()).unwrap();
        queue.wait_idle();

        assert_eq!(queue.state(running), Some(JobState::Cancelled));
        assert!(!queued_ran.load(Ordering::SeqCst));
    }

    #[test]
    fn queued_jobs_can_move_without_touching_running_or_paused_work() {
        let queue = JobQueue::new(1);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let order = Arc::new(Mutex::new(Vec::new()));

        let blocker = queue.submit(Box::new(move |_ctl, _progress| {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.recv().unwrap();

        let submit_marker = |marker| {
            let order = Arc::clone(&order);
            queue.submit(Box::new(move |_ctl, _progress| {
                order.lock().unwrap().push(marker);
                Ok(())
            }))
        };
        let first = submit_marker(1);
        let paused = submit_marker(2);
        let last = submit_marker(3);
        assert!(queue.try_pause(paused));
        assert_eq!(queue.state(paused), Some(JobState::Queued));
        assert_eq!(queue.reorderable_job_ids(), vec![first, last]);

        assert!(queue.move_queued_earlier(last));
        assert_eq!(queue.reorderable_job_ids(), vec![last, first]);
        assert!(!queue.move_queued_earlier(last));
        assert!(!queue.move_queued_later(first));
        assert!(!queue.move_queued_earlier(paused));

        queue.resume(paused);
        assert_eq!(queue.reorderable_job_ids(), vec![last, paused, first]);
        assert!(queue.move_queued_later(last));
        assert_eq!(queue.reorderable_job_ids(), vec![paused, last, first]);

        gate_tx.send(()).unwrap();
        queue.wait_idle();
        assert_eq!(queue.state(blocker), Some(JobState::Done));
        assert_eq!(*order.lock().unwrap(), vec![2, 3, 1]);
    }

    #[test]
    fn queued_jobs_can_move_to_an_arbitrary_position_atomically() {
        let queue = JobQueue::new(1);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let order = Arc::new(Mutex::new(Vec::new()));

        queue.submit(Box::new(move |_ctl, _progress| {
            started_tx.send(()).unwrap();
            gate_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.recv().unwrap();

        let submit_marker = |marker| {
            let order = Arc::clone(&order);
            queue.submit(Box::new(move |_ctl, _progress| {
                order.lock().unwrap().push(marker);
                Ok(())
            }))
        };
        let first = submit_marker(1);
        let paused = submit_marker(2);
        let second = submit_marker(3);
        let third = submit_marker(4);
        assert!(queue.try_pause(paused));

        assert!(queue.move_queued_before(third, Some(first)));
        assert_eq!(queue.reorderable_job_ids(), vec![third, first, second]);
        assert!(queue.move_queued_before(third, None));
        assert_eq!(queue.reorderable_job_ids(), vec![first, second, third]);
        assert!(queue.move_queued_before(first, Some(third)));
        assert_eq!(queue.reorderable_job_ids(), vec![second, first, third]);

        assert!(queue.move_queued_before(first, Some(third)));
        assert!(!queue.move_queued_before(first, Some(first)));
        assert!(!queue.move_queued_before(first, Some(paused)));
        assert!(!queue.move_queued_before(paused, Some(first)));

        queue.resume(paused);
        gate_tx.send(()).unwrap();
        queue.wait_idle();
        assert_eq!(*order.lock().unwrap(), vec![3, 2, 1, 4]);
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

    #[test]
    fn claimed_job_is_active_before_cancellation_can_observe_it() {
        let inner = Inner::default();
        let id = JobId(42);
        lock_unpoisoned(&inner.slots).insert(
            id,
            Slot {
                state: JobState::Queued,
                token: ControlToken::new(),
                progress: JobProgress::default(),
                interruptible: true,
                resources: JobResources::default(),
                job: Some(Box::new(|_ctl, _progress| Ok(()))),
            },
        );

        let Some((_job, token, claimed_state)) = inner.claim_job(id) else {
            panic!("queued job was not claimed");
        };
        assert_eq!(claimed_state, JobState::Running);
        assert_eq!(inner.request_cancel_many(&[id]), (vec![id], Vec::new()));
        assert!(token.is_cancelled());
        assert_eq!(
            lock_unpoisoned(&inner.slots)
                .get(&id)
                .map(|slot| slot.state.clone()),
            Some(JobState::Running)
        );
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

    #[test]
    fn durable_phase_rejects_pause_and_cancel_requests() {
        let queue = JobQueue::new(1);
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let id = queue.submit(Box::new(move |_ctl, progress| {
            progress.on_phase(ProgressPhase::UpdateCommit, false);
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));

        entered_rx.recv().unwrap();
        assert!(!queue.try_pause(id));
        assert!(!queue.try_cancel(id));
        assert_eq!(queue.state(id), Some(JobState::Running));

        release_tx.send(()).unwrap();
        queue.wait_idle();
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
    fn forget_terminal_removes_only_finished_slots() {
        let queue = JobQueue::new(1);
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (finish_tx, finish_rx) = mpsc::channel::<()>();
        let active = queue.submit(Box::new(move |_ctl, _progress| {
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            Ok(())
        }));
        started_rx.recv().unwrap();

        assert!(!queue.forget_terminal(active));
        assert_eq!(queue.state(active), Some(JobState::Running));

        finish_tx.send(()).unwrap();
        queue.wait_idle();
        assert!(queue.forget_terminal(active));
        assert_eq!(queue.state(active), None);
        assert!(!queue.forget_terminal(active));
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
