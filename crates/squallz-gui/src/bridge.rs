//! Worker ↔ UI question bridge. When the engine needs a decision in the
//! middle of a job (overwrite conflict, password), the worker thread emits
//! an event and parks on a condition variable here until the frontend
//! answers through `answer_conflict` / `answer_password`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// Answer produced by the frontend modal.
#[derive(Debug, Clone)]
pub enum AskAnswer {
    /// Conflict dialog: `decision` ∈ overwrite|skip|rename|abort.
    Conflict {
        /// Chosen action
        decision: String,
        /// Apply to all remaining conflicts of this job
        apply_all: bool,
    },
    /// Password dialog: `None` = the user cancelled.
    Password(Option<String>),
}

#[derive(Default)]
struct Slot {
    answer: Mutex<Option<AskAnswer>>,
    cv: Condvar,
}

/// Registry of jobs currently waiting for an answer.
#[derive(Default)]
pub struct AskBridge {
    slots: Mutex<HashMap<u64, Arc<Slot>>>,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait_timeout_unpoisoned<'a, T>(
    cv: &Condvar,
    guard: MutexGuard<'a, T>,
    duration: Duration,
) -> MutexGuard<'a, T> {
    match cv.wait_timeout(guard, duration) {
        Ok((guard, _timeout)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}

fn remove_slot(slots: &Mutex<HashMap<u64, Arc<Slot>>>, job_id: u64) {
    lock_unpoisoned(slots).remove(&job_id);
}

impl AskBridge {
    /// Blocks the calling worker thread until the frontend answers or
    /// `cancelled()` turns true (returns `None`). The caller emits the
    /// matching event *before* calling this.
    pub fn wait(&self, job_id: u64, cancelled: &dyn Fn() -> bool) -> Option<AskAnswer> {
        let slot = {
            let mut slots = lock_unpoisoned(&self.slots);
            Arc::clone(
                slots
                    .entry(job_id)
                    .or_insert_with(|| Arc::new(Slot::default())),
            )
        };
        let mut answer = lock_unpoisoned(&slot.answer);
        loop {
            if let Some(a) = answer.take() {
                remove_slot(&self.slots, job_id);
                return Some(a);
            }
            if cancelled() {
                remove_slot(&self.slots, job_id);
                return None;
            }
            // Bounded wait so a cancel without an answer still unblocks.
            answer = wait_timeout_unpoisoned(&slot.cv, answer, Duration::from_millis(100));
        }
    }

    /// Delivers the frontend's answer to a waiting job (no-op when the job
    /// is not waiting, e.g. it was cancelled meanwhile).
    pub fn answer(&self, job_id: u64, answer: AskAnswer) {
        let slot = lock_unpoisoned(&self.slots).get(&job_id).cloned();
        if let Some(slot) = slot {
            *lock_unpoisoned(&slot.answer) = Some(answer);
            slot.cv.notify_all();
        }
    }

    /// Wakes a waiting job so it can observe its cancellation predicate
    /// without waiting for the bounded polling interval to expire.
    pub fn wake_cancelled(&self, job_id: u64) {
        let slot = lock_unpoisoned(&self.slots).get(&job_id).cloned();
        if let Some(slot) = slot {
            slot.cv.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn wait_for_slot(bridge: &AskBridge, job_id: u64) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if lock_unpoisoned(&bridge.slots).contains_key(&job_id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("bridge slot {job_id} was not registered");
    }

    #[test]
    fn answer_unblocks_waiting_worker() {
        let bridge = Arc::new(AskBridge::default());
        let b = Arc::clone(&bridge);
        let worker = std::thread::spawn(move || b.wait(7, &|| false));
        std::thread::sleep(Duration::from_millis(50));
        bridge.answer(
            7,
            AskAnswer::Conflict {
                decision: "skip".into(),
                apply_all: true,
            },
        );
        match worker.join().unwrap() {
            Some(AskAnswer::Conflict {
                decision,
                apply_all,
            }) => {
                assert_eq!(decision, "skip");
                assert!(apply_all);
            }
            other => panic!("unexpected answer: {other:?}"),
        }
    }

    #[test]
    fn cancel_unblocks_without_answer() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let bridge = Arc::new(AskBridge::default());
        let flag = Arc::new(AtomicBool::new(false));
        let b = Arc::clone(&bridge);
        let f = Arc::clone(&flag);
        let worker = std::thread::spawn(move || b.wait(8, &|| f.load(Ordering::Relaxed)));
        std::thread::sleep(Duration::from_millis(30));
        flag.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().is_none());
    }

    #[test]
    fn cancel_wake_unblocks_without_poll_delay() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let polling_bridge = Arc::new(AskBridge::default());
        let polling_flag = Arc::new(AtomicBool::new(false));
        let b = Arc::clone(&polling_bridge);
        let f = Arc::clone(&polling_flag);
        let polling_worker = std::thread::spawn(move || b.wait(10, &|| f.load(Ordering::Relaxed)));
        wait_for_slot(&polling_bridge, 10);
        let polling_start = Instant::now();
        polling_flag.store(true, Ordering::Relaxed);
        assert!(polling_worker.join().unwrap().is_none());
        let polling_ms = polling_start.elapsed().as_millis();

        let wake_bridge = Arc::new(AskBridge::default());
        let wake_flag = Arc::new(AtomicBool::new(false));
        let b = Arc::clone(&wake_bridge);
        let f = Arc::clone(&wake_flag);
        let wake_worker = std::thread::spawn(move || b.wait(11, &|| f.load(Ordering::Relaxed)));
        wait_for_slot(&wake_bridge, 11);
        let wake_start = Instant::now();
        wake_flag.store(true, Ordering::Relaxed);
        wake_bridge.wake_cancelled(11);
        assert!(wake_worker.join().unwrap().is_none());
        let wake_ms = wake_start.elapsed().as_millis();

        println!("BRIDGE_METRIC bridge_cancel_poll_wait_ms={polling_ms}");
        println!("BRIDGE_METRIC bridge_cancel_wake_wait_ms={wake_ms}");
        assert!(
            wake_ms <= 50,
            "cancel wake took {wake_ms}ms; expected immediate bridge wake"
        );
    }

    #[test]
    fn bridge_recovers_after_slots_lock_poison() {
        let bridge = Arc::new(AskBridge::default());
        let poison_bridge = Arc::clone(&bridge);
        let _ = std::thread::spawn(move || {
            let _guard = poison_bridge.slots.lock().unwrap();
            panic!("poison slots");
        })
        .join();

        let b = Arc::clone(&bridge);
        let worker = std::thread::spawn(move || b.wait(9, &|| false));
        std::thread::sleep(Duration::from_millis(50));
        bridge.answer(9, AskAnswer::Password(Some("secret".into())));

        match worker.join().unwrap() {
            Some(AskAnswer::Password(Some(password))) => assert_eq!(password, "secret"),
            other => panic!("unexpected answer: {other:?}"),
        }
    }

    #[test]
    fn bridge_recovers_after_slot_answer_lock_poison() {
        let bridge = Arc::new(AskBridge::default());
        let slot = Arc::new(Slot::default());
        bridge.slots.lock().unwrap().insert(10, Arc::clone(&slot));
        let _ = std::thread::spawn(move || {
            let _guard = slot.answer.lock().unwrap();
            panic!("poison answer");
        })
        .join();

        let b = Arc::new(bridge);
        let worker_bridge = Arc::clone(&b);
        let worker = std::thread::spawn(move || worker_bridge.wait(10, &|| false));
        std::thread::sleep(Duration::from_millis(50));
        b.answer(
            10,
            AskAnswer::Conflict {
                decision: "overwrite".into(),
                apply_all: false,
            },
        );

        match worker.join().unwrap() {
            Some(AskAnswer::Conflict {
                decision,
                apply_all,
            }) => {
                assert_eq!(decision, "overwrite");
                assert!(!apply_all);
            }
            other => panic!("unexpected answer: {other:?}"),
        }
    }
}
