//! ZIP update operations: append, delete, and rename through caller-owned
//! streams. Core owns locking, staging, recovery, and publication.
//!
//! Unchanged entries are **raw-copied** (no recompression; encrypted
//! entries stay encrypted without needing the password). Added files are
//! compressed with the usual create options.

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use squallz_format_api::{
    ArchiveWriter, ControlToken, CreateOptions, EntryMeta, EntryPath, FormatError,
    PreparedUpdateAdditions, ProgressSink, ReadSeek, UpdateOp, UpdateOptions, WriteSeek,
};
use zip::ZipArchive;

use super::encoding::decode_entry_name;
use super::error::map_zip_error;
use super::writer::ZipArchiveWriter;
use delete::Deletions;

mod delete;
mod plan;
mod selection;

/// Extra bytes included in the early space estimate for central-directory
/// growth and compression overhead on incompressible additions.
const SPACE_SLACK: u64 = 1024 * 1024;

/// Maximum source read while parsing or copying an existing ZIP. Control
/// checks run for every read; raw-copy progress is emitted at most once per
/// chunk.
const RAW_COPY_CHUNK: usize = 64 * 1024;

pub(super) fn staging_bytes_estimate(source_bytes: u64, addition_bytes: u64) -> u64 {
    source_bytes
        .saturating_add(addition_bytes)
        .saturating_add(SPACE_SLACK)
}

trait AdditionSet {
    fn len(&self) -> usize;
    fn meta(&self, index: usize) -> Option<&EntryMeta>;
    fn add_entry(
        &mut self,
        index: usize,
        writer: &mut dyn ArchiveWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), FormatError>;
}

struct EngineAdditions<'a>(&'a mut dyn PreparedUpdateAdditions);

struct PreparedRewrite<'a> {
    archive: ZipArchive<RawCopySource<'a>>,
    raw_copy: RawCopyTracker,
    plan: plan::UpdatePlan,
}

#[derive(Clone, Default)]
struct RawCopyTracker {
    shared: Arc<RawCopyShared>,
}

#[derive(Default)]
struct RawCopyShared {
    active: AtomicBool,
    current_done: AtomicU64,
    current_total: AtomicU64,
    reported_done: AtomicU64,
    state: Mutex<Option<RawCopyState>>,
}

struct RawCopyState {
    path: EntryPath,
    base: u64,
    total: u64,
}

struct RawCopyProgress {
    path: EntryPath,
    done: u64,
    total: u64,
    current_done: u64,
    current_total: u64,
}

struct RawCopySource<'a> {
    inner: Box<dyn ReadSeek>,
    tracker: RawCopyTracker,
    progress: &'a dyn ProgressSink,
    ctl: &'a ControlToken,
}

impl RawCopyTracker {
    fn begin(&self, path: EntryPath, base: u64, total: u64, current_total: u64) {
        *self.lock() = Some(RawCopyState { path, base, total });
        self.shared.current_done.store(0, Ordering::Relaxed);
        self.shared
            .current_total
            .store(current_total, Ordering::Relaxed);
        self.shared.reported_done.store(0, Ordering::Relaxed);
        self.shared.active.store(true, Ordering::Release);
    }

    fn is_active(&self) -> bool {
        self.shared.active.load(Ordering::Acquire)
    }

    fn record(&self, bytes: u64) -> Option<RawCopyProgress> {
        if bytes == 0 {
            return None;
        }
        let previous = self.shared.current_done.fetch_add(bytes, Ordering::Relaxed);
        let current_done = previous.saturating_add(bytes);
        let current_total = self.shared.current_total.load(Ordering::Relaxed);
        let reported_done = self.shared.reported_done.load(Ordering::Relaxed);
        let report = current_done >= current_total
            || current_done.saturating_sub(reported_done) >= RAW_COPY_CHUNK as u64;
        if !report {
            return None;
        }
        self.shared
            .reported_done
            .store(current_done, Ordering::Relaxed);
        let state = self.lock();
        let active = state.as_ref()?;
        Some(RawCopyProgress {
            path: active.path.clone(),
            done: active.base.saturating_add(current_done).min(active.total),
            total: active.total,
            current_done: current_done.min(current_total),
            current_total,
        })
    }

    fn finish(&self) -> u64 {
        self.shared.active.store(false, Ordering::Release);
        let copied = self.shared.current_done.load(Ordering::Relaxed);
        self.lock().take();
        copied
    }

    fn lock(&self) -> MutexGuard<'_, Option<RawCopyState>> {
        match self.shared.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Read for RawCopySource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.ctl.checkpoint().map_err(std::io::Error::other)?;
        let chunk = buf.len().min(RAW_COPY_CHUNK);
        let count = self.inner.read(&mut buf[..chunk])?;
        if self.tracker.is_active() {
            if let Some(event) = self.tracker.record(count as u64) {
                self.progress.on_entry_progress(
                    event.done,
                    event.total,
                    &event.path,
                    event.current_done,
                    event.current_total,
                );
            }
        }
        Ok(count)
    }
}

impl Seek for RawCopySource<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.ctl.checkpoint().map_err(std::io::Error::other)?;
        self.inner.seek(position)
    }
}

#[allow(clippy::too_many_arguments)] // archive update inputs have distinct roles
pub(super) fn rewrite_archive(
    source: Box<dyn ReadSeek>,
    output: Box<dyn WriteSeek>,
    ops: &[UpdateOp],
    additions: &mut dyn PreparedUpdateAdditions,
    opts: &UpdateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    rewrite_archive_impl(
        source,
        output,
        ops,
        &mut EngineAdditions(additions),
        opts,
        progress,
        ctl,
    )
}

#[allow(clippy::too_many_arguments)] // archive update inputs have distinct roles
fn rewrite_archive_impl(
    source: Box<dyn ReadSeek>,
    output: Box<dyn WriteSeek>,
    ops: &[UpdateOp],
    additions: &mut impl AdditionSet,
    opts: &UpdateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let PreparedRewrite {
        mut archive,
        raw_copy,
        plan,
    } = prepare_update(
        source,
        ops,
        additions,
        opts.encoding_override.as_deref(),
        progress,
        ctl,
    )?;
    rewrite(
        &mut archive,
        &raw_copy,
        output,
        &plan.deletes,
        &plan,
        additions,
        &opts.create,
        progress,
        ctl,
    )
}

fn prepare_update<'a>(
    source: Box<dyn ReadSeek>,
    ops: &[UpdateOp],
    additions: &impl AdditionSet,
    encoding: Option<&str>,
    progress: &'a dyn ProgressSink,
    ctl: &'a ControlToken,
) -> Result<PreparedRewrite<'a>, FormatError> {
    ctl.checkpoint()?;
    let raw_copy = RawCopyTracker::default();
    let source = RawCopySource {
        inner: source,
        tracker: raw_copy.clone(),
        progress,
        ctl,
    };
    let mut archive =
        ZipArchive::new(source).map_err(|error| map_controlled_zip_error(error, ctl))?;
    ctl.checkpoint()?;

    let plan = plan::prepare(&mut archive, ops, additions, encoding, ctl)?;
    Ok(PreparedRewrite {
        archive,
        raw_copy,
        plan,
    })
}

/// Writes the updated archive into the caller-owned output stream.
#[allow(clippy::too_many_arguments)] // internal plumbing with distinct roles
fn rewrite(
    archive: &mut ZipArchive<RawCopySource<'_>>,
    raw_copy: &RawCopyTracker,
    output: Box<dyn WriteSeek>,
    deletes: &Deletions,
    plan: &plan::UpdatePlan,
    additions: &mut impl AdditionSet,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut writer = ZipArchiveWriter::new(output, opts, ctl);
    writer.set_raw_comment(archive.comment())?;

    // Progress in bytes: raw (compressed) bytes for copies, plain bytes for
    // additions.
    let mut copied_total = 0u64;
    for index in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(index)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        if !deletes.matches(&decode_entry_name(file.name_raw(), plan.encoding)) {
            copied_total = copied_total.saturating_add(file.compressed_size());
        }
    }
    let total = copied_total.saturating_add(addition_bytes(additions, ctl)?);
    let mut done = 0u64;

    for i in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(i)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        let compressed = file.compressed_size();
        let path = decode_entry_name(file.name_raw(), plan.encoding);
        progress.on_progress(done, total, &path);
        if deletes.matches(&path) {
            continue; // dropped entry
        }
        let rename_to = plan.renames.get(file.name_raw());
        raw_copy.begin(path.clone(), done, total, compressed);
        let result = writer.raw_copy(file, rename_to.map(String::as_str));
        let copied = raw_copy.finish();
        if let Err(error) = result {
            return Err(if ctl.is_cancelled() {
                FormatError::Cancelled
            } else {
                error
            });
        }
        if copied != compressed {
            return Err(FormatError::CorruptArchive(format!(
                "raw ZIP entry '{}' ended after {copied} of {compressed} compressed bytes",
                path.display
            )));
        }
        ctl.checkpoint()?;
        done = done.saturating_add(compressed);
    }

    for index in 0..additions.len() {
        ctl.checkpoint()?;
        let (path, size) = {
            let meta = addition_meta(additions, index)?;
            (meta.path.clone(), meta.size)
        };
        progress.on_entry_progress(done, total, &path, 0, size);
        additions.add_entry(index, &mut writer, progress, ctl, done, total)?;
        done = done.saturating_add(size);
    }
    ctl.checkpoint()?;
    Box::new(writer).finish()?;
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    ctl.checkpoint()?;
    Ok(())
}

fn addition_meta(additions: &impl AdditionSet, index: usize) -> Result<&EntryMeta, FormatError> {
    additions.meta(index).ok_or_else(|| {
        FormatError::Other(format!(
            "prepared update entry index {index} is out of range"
        ))
    })
}

fn map_controlled_zip_error(error: zip::result::ZipError, ctl: &ControlToken) -> FormatError {
    if ctl.is_cancelled() {
        FormatError::Cancelled
    } else {
        map_zip_error(error)
    }
}

fn addition_bytes(additions: &impl AdditionSet, ctl: &ControlToken) -> Result<u64, FormatError> {
    ctl.checkpoint()?;
    let mut bytes = 0u64;
    for index in 0..additions.len() {
        ctl.checkpoint()?;
        bytes = bytes.saturating_add(addition_meta(additions, index)?.size);
    }
    Ok(bytes)
}

impl AdditionSet for EngineAdditions<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn meta(&self, index: usize) -> Option<&EntryMeta> {
        self.0.meta(index)
    }

    fn add_entry(
        &mut self,
        index: usize,
        writer: &mut dyn ArchiveWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), FormatError> {
        self.0
            .add_entry(index, writer, progress, ctl, completed_bytes, total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Write};
    use std::sync::atomic::AtomicUsize;

    use squallz_format_api::EntryType;

    use super::*;

    struct CancelAfterFirstRead {
        inner: Cursor<Vec<u8>>,
        control: Arc<ControlToken>,
        cancelled: bool,
    }

    impl Read for CancelAfterFirstRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(buf)?;
            if count > 0 && !self.cancelled {
                self.cancelled = true;
                self.control.cancel();
            }
            Ok(count)
        }
    }

    impl Seek for CancelAfterFirstRead {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }

    struct CancelOnArmedWrite {
        inner: Cursor<Vec<u8>>,
        control: ControlToken,
        armed: Arc<AtomicBool>,
        armed_writes: Arc<AtomicUsize>,
    }

    impl Write for CancelOnArmedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.armed.load(Ordering::SeqCst) {
                let index = self.armed_writes.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    self.control.cancel();
                }
            }
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl Seek for CancelOnArmedWrite {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    struct ArmingAddition {
        meta: EntryMeta,
        armed: Arc<AtomicBool>,
    }

    struct EmptyAdditions;

    impl AdditionSet for EmptyAdditions {
        fn len(&self) -> usize {
            0
        }

        fn meta(&self, _index: usize) -> Option<&EntryMeta> {
            None
        }

        fn add_entry(
            &mut self,
            index: usize,
            _writer: &mut dyn ArchiveWriter,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
            _completed_bytes: u64,
            _total_bytes: u64,
        ) -> Result<(), FormatError> {
            Err(FormatError::Other(format!(
                "empty additions cannot write index {index}"
            )))
        }
    }

    impl AdditionSet for ArmingAddition {
        fn len(&self) -> usize {
            1
        }

        fn meta(&self, index: usize) -> Option<&EntryMeta> {
            (index == 0).then_some(&self.meta)
        }

        fn add_entry(
            &mut self,
            index: usize,
            writer: &mut dyn ArchiveWriter,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
            _completed_bytes: u64,
            _total_bytes: u64,
        ) -> Result<(), FormatError> {
            if index != 0 {
                return Err(FormatError::Other(format!(
                    "unexpected test addition index {index}"
                )));
            }
            writer.add_entry(&self.meta, None)?;
            self.armed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn zip_with_one_entry() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("entry.txt", zip::write::SimpleFileOptions::default())
            .expect("start ZIP entry");
        writer.write_all(b"payload").expect("write ZIP entry");
        writer.finish().expect("finish ZIP").into_inner()
    }

    #[test]
    fn staging_estimate_includes_slack_and_saturates() {
        assert_eq!(staging_bytes_estimate(10, 20), SPACE_SLACK + 30);
        assert_eq!(staging_bytes_estimate(u64::MAX, 1), u64::MAX);
    }

    #[test]
    fn update_open_reports_cancellation_during_central_directory_reads() {
        let control = ControlToken::new();
        let source = CancelAfterFirstRead {
            inner: Cursor::new(zip_with_one_entry()),
            control: Arc::clone(&control),
            cancelled: false,
        };
        let progress = squallz_format_api::NoProgress;
        let additions = EmptyAdditions;

        let result = prepare_update(
            Box::new(source),
            &[],
            &additions,
            None,
            &progress,
            control.as_ref(),
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
    }

    #[test]
    fn cancelled_update_stops_before_addition_sizing() {
        let control = ControlToken::default();
        control.cancel();

        let result = addition_bytes(&EmptyAdditions, &control);

        assert!(matches!(result, Err(FormatError::Cancelled)));
    }

    #[test]
    fn update_final_directory_uses_the_callers_control_token() {
        let control = ControlToken::default();
        let armed = Arc::new(AtomicBool::new(false));
        let armed_writes = Arc::new(AtomicUsize::new(0));
        let output = CancelOnArmedWrite {
            inner: Cursor::new(Vec::new()),
            control: control.clone(),
            armed: Arc::clone(&armed),
            armed_writes: Arc::clone(&armed_writes),
        };
        let mut additions = ArmingAddition {
            meta: EntryMeta {
                path: EntryPath::from_utf8("added/"),
                entry_type: EntryType::Dir,
                size: 0,
                compressed_size: None,
                modified: None,
                unix_mode: None,
                crc32: None,
                encrypted: false,
            },
            armed,
        };

        let result = rewrite_archive_impl(
            Box::new(Cursor::new(zip_with_one_entry())),
            Box::new(output),
            &[],
            &mut additions,
            &UpdateOptions::default(),
            &squallz_format_api::NoProgress,
            &control,
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
        assert_eq!(
            armed_writes.load(Ordering::SeqCst),
            1,
            "ZIP update must stop writing to the caller-owned output after cancellation"
        );
    }
}
