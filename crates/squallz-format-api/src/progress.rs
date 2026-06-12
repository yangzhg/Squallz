//! Progress reporting and cancellation/pause control.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::entry::EntryPath;
use crate::error::FormatError;

/// Progress reporting. Designed as a `Send + Sync` shared reference: multiple
/// worker threads can report concurrently; implementations aggregate with
/// atomics or channels.
pub trait ProgressSink: Send + Sync {
    /// Bytes processed / total bytes / current entry
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath);

    /// Bytes processed for the current entry in addition to the overall
    /// progress. Implementations that only care about the old aggregate
    /// contract can keep the default forwarding behavior.
    fn on_entry_progress(
        &self,
        done: u64,
        total: u64,
        current: &EntryPath,
        _current_done: u64,
        _current_total: u64,
    ) {
        self.on_progress(done, total, current);
    }
}

/// No-op implementation that discards progress.
#[derive(Debug, Default)]
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}
}

/// Cancellation + pause token. Worker threads call
/// [`ControlToken::checkpoint`] at chunk boundaries.
#[derive(Debug, Default)]
pub struct ControlToken {
    cancelled: AtomicBool,
    paused: AtomicBool,
}

impl ControlToken {
    /// Creates a token that can be shared across threads.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Requests cancellation (irreversible).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Requests a pause.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    /// Resumes execution.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Whether currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Chunk-boundary checkpoint: blocks while paused, returns
    /// [`FormatError::Cancelled`] when cancelled.
    pub fn checkpoint(&self) -> Result<(), FormatError> {
        loop {
            if self.is_cancelled() {
                return Err(FormatError::Cancelled);
            }
            if !self.is_paused() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_token_cancel_and_pause() {
        let ctl = ControlToken::new();
        assert!(ctl.checkpoint().is_ok());
        ctl.pause();
        assert!(ctl.is_paused());
        ctl.resume();
        assert!(ctl.checkpoint().is_ok());
        ctl.cancel();
        assert!(matches!(ctl.checkpoint(), Err(FormatError::Cancelled)));
    }

    #[test]
    fn control_token_cancel_wins_while_paused() {
        let ctl = ControlToken::new();
        ctl.pause();
        ctl.cancel();

        assert!(matches!(ctl.checkpoint(), Err(FormatError::Cancelled)));
        assert!(ctl.is_cancelled());
    }
}
