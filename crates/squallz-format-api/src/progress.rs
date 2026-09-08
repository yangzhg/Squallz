//! Progress reporting and cancellation/pause control.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::entry::EntryPath;
use crate::error::FormatError;

/// A semantic progress phase for operations whose byte counters cover
/// different work sets and therefore must not be presented as one percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProgressPhase {
    /// Loading source metadata or preparing private recovery inputs.
    RecoveryPrepare,
    /// Checking protected files and recovery blocks.
    RecoveryVerify,
    /// Computing parity or rebuilding protected data.
    RecoveryProcess,
    /// Writing or publishing the completed recovery result.
    RecoveryFinalize,
    /// Replaying a durable output-publication transaction after interruption.
    OutputRecovery,
    /// Writing a completed archive into its physical volume files.
    OutputSplit,
    /// Verifying a completed output and its final destination binding.
    OutputVerify,
    /// Publishing a completed output through its durable transaction.
    OutputCommit,
    /// Verifying and cleaning output-publication artifacts.
    OutputCleanup,
    /// Replaying a durable archive-update transaction after interruption.
    UpdateRecovery,
    /// Rewriting archive entries into a new package.
    UpdateRewrite,
    /// Binding the source and replacement package contents to the transaction.
    UpdateVerify,
    /// Installing the replacement package through durable no-replace moves.
    UpdateCommit,
    /// Verifying and removing transaction artifacts after installation.
    UpdateCleanup,
    /// Verifying the source and isolated copy before macOS SFX signing.
    SfxPublishVerify,
    /// Applying and verifying Developer ID signatures.
    SfxPublishSign,
    /// Waiting for Apple notarization and its accepted log.
    SfxPublishNotarize,
    /// Stapling and validating the notarized macOS app.
    SfxPublishFinalize,
}

/// Progress reporting. Designed as a `Send + Sync` shared reference: multiple
/// worker threads can report concurrently; implementations aggregate with
/// atomics or channels.
pub trait ProgressSink: Send + Sync {
    /// Bytes processed / total bytes / current entry
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath);

    /// Reports an input-manifest scan without reinterpreting the entry count as
    /// bytes. Implementations can opt in when their presentation model can
    /// distinguish scanning from byte transfer.
    fn on_scan_progress(&self, _entries: u64, _current: &EntryPath) {}

    /// Announces a semantic phase before its byte events. `interruptible`
    /// describes whether pause and cancellation requests can still take effect
    /// safely in that phase. Implementations may ignore phase information.
    fn on_phase(&self, _phase: ProgressPhase, _interruptible: bool) {}

    /// Bytes processed for the current entry in addition to the overall
    /// progress. Aggregate-only implementations can keep the default
    /// forwarding behavior.
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
///
/// Clones share the same state so stream adapters can retain control without
/// requiring every archive format to own an `Arc<ControlToken>`.
#[derive(Debug, Clone, Default)]
pub struct ControlToken {
    state: Arc<ControlState>,
}

#[derive(Debug, Default)]
struct ControlState {
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
        self.state.cancelled.store(true, Ordering::Relaxed);
    }

    /// Requests a pause.
    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::Relaxed);
    }

    /// Resumes execution.
    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Relaxed)
    }

    /// Whether currently paused.
    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Relaxed)
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

    #[test]
    fn cloned_control_token_shares_pause_and_cancellation() {
        let original = ControlToken::default();
        let clone = original.clone();

        clone.pause();
        assert!(original.is_paused());
        original.resume();
        assert!(!clone.is_paused());

        original.cancel();
        assert!(clone.is_cancelled());
        assert!(matches!(clone.checkpoint(), Err(FormatError::Cancelled)));
    }
}
