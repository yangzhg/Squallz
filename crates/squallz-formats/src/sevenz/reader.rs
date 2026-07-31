//! 7Z read side: entry listing, single-entry reads, single-pass extraction
//! and integrity testing. Solid blocks force sequential decoding, so
//! extraction and testing stream every entry exactly once through
//! `for_each_entries`; `read_entry` (preview path) decodes up to the
//! requested file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::Path;
use std::time::SystemTime;

use sevenz_rust2::ArchiveEntry;
use squallz_format_api::{
    empty_extract_report, ArchiveReader, BoundedProblemLog, ControlToken, EntryMeta, EntryPath,
    EntryType, ExtractOptions, ExtractReport, ExtractSink, FormatError, OpenOptions, ProgressSink,
    ReadSeek, TestReport, TestSummary, TEST_PROBLEM_PREVIEW_LIMIT,
};

use super::{map_7z_error, FILE_ATTRIBUTE_UNIX_EXTENSION};

/// Chunk size when draining entry data (test pass).
const READ_CHUNK: usize = 64 * 1024;

/// Read handle over a 7z archive.
pub(super) struct SevenZArchiveReader {
    inner: sevenz_rust2::ArchiveReader<Box<dyn ReadSeek>>,
    password_supplied: bool,
}

impl SevenZArchiveReader {
    pub(super) fn open(src: Box<dyn ReadSeek>, opts: &OpenOptions) -> Result<Self, FormatError> {
        let password = open_password(opts);
        // Opening a header-encrypted archive without a password surfaces
        // PasswordRequired here.
        let inner = sevenz_rust2::ArchiveReader::new(src, password).map_err(map_7z_error)?;
        Ok(Self {
            inner,
            password_supplied: opts.password.is_some(),
        })
    }

    fn test_with_problem_recorder(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        mut record_problem: impl FnMut(String),
    ) -> Result<u64, FormatError> {
        let total: u64 = self.inner.archive().files.iter().map(|e| e.size()).sum();
        let entry_plans = build_entry_read_plans(self.inner.archive(), None)?;
        let mut entries_tested = 0u64;
        let mut done = 0u64;
        let mut cancelled = false;
        let mut mapping_failure = None;
        let backend_result = self.inner.for_each_entries(|entry, reader| {
            if ctl.checkpoint().is_err() {
                cancelled = true;
                return Ok(false);
            }
            let Some(plan) = entry_plans.get(&entry_identity(entry)).copied() else {
                mapping_failure = Some(FormatError::CorruptArchive(
                    "7z entry is missing from its stream map".into(),
                ));
                return Ok(false);
            };
            entries_tested += 1;
            let meta = meta_of(entry, plan.encrypted);
            let mut hasher = crc32fast::Hasher::new();
            let mut buf = vec![0u8; READ_CHUNK];
            loop {
                if ctl.checkpoint().is_err() {
                    cancelled = true;
                    return Ok(false);
                }
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        hasher.update(&buf[..n]);
                        done += n as u64;
                        progress.on_progress(done, total, &meta.path);
                    }
                    Err(e) => {
                        record_problem(format!("{}: {e}", meta.path));
                        break;
                    }
                }
            }
            if entry.has_crc && u64::from(hasher.finalize()) != entry.crc {
                record_problem(format!("{}: CRC mismatch", meta.path));
            }
            Ok(true)
        });
        if let Some(error) = mapping_failure {
            return Err(error);
        }
        // Decoder failures caused by wrong passwords remain hard errors
        // rather than per-entry archive damage.
        backend_result.map_err(map_7z_error)?;
        if cancelled {
            return Err(FormatError::Cancelled);
        }
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(entries_tested)
    }
}

fn open_password(opts: &OpenOptions) -> sevenz_rust2::Password {
    match opts.password.as_ref() {
        Some(password) => sevenz_rust2::Password::from(password.expose()),
        None => sevenz_rust2::Password::empty(),
    }
}

/// Builds the [`EntryMeta`] of one 7z entry (names are UTF-8 strings in the
/// 7z model, decoded from UTF-16 by the backend).
fn meta_of(entry: &ArchiveEntry, encrypted: bool) -> EntryMeta {
    let entry_type = if entry.is_directory() {
        EntryType::Dir
    } else {
        EntryType::File
    };
    // p7zip stores Unix permissions in the high attribute bits.
    let attributes = entry.windows_attributes();
    let unix_mode = (entry.has_windows_attributes
        && attributes & FILE_ATTRIBUTE_UNIX_EXTENSION != 0)
        .then_some((attributes >> 16) & 0o7777);
    EntryMeta {
        path: EntryPath::from_utf8(entry.name()),
        entry_type,
        size: entry.size(),
        compressed_size: Some(entry.compressed_size),
        modified: entry
            .has_last_modified_date
            .then(|| SystemTime::from(entry.last_modified_date())),
        unix_mode,
        crc32: entry.has_crc.then_some(entry.crc as u32),
        encrypted: encrypted && entry.has_stream(),
    }
}

fn drain_entry(reader: &mut dyn Read, ctl: &ControlToken) -> Result<(), FormatError> {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        ctl.checkpoint()?;
        if reader.read(&mut buf)? == 0 {
            return Ok(());
        }
    }
}

#[derive(Clone, Copy, Default)]
struct EntryReadPlan {
    file_index: usize,
    block_index: Option<usize>,
    selected_later_in_block: bool,
    encrypted: bool,
}

fn entry_identity(entry: &ArchiveEntry) -> usize {
    std::ptr::from_ref(entry) as usize
}

fn block_is_encrypted(
    archive: &sevenz_rust2::Archive,
    block_index: usize,
) -> Result<bool, FormatError> {
    let Some(block) = archive.blocks.get(block_index) else {
        return Err(FormatError::CorruptArchive(
            "7z stream map references a missing block".into(),
        ));
    };
    Ok(block
        .coders
        .iter()
        .any(|coder| coder.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256))
}

fn entry_is_encrypted(
    archive: &sevenz_rust2::Archive,
    file_index: usize,
) -> Result<bool, FormatError> {
    let Some(block_index) = archive.stream_map.file_block_index.get(file_index).copied() else {
        return Err(FormatError::CorruptArchive(
            "7z stream map is shorter than its file list".into(),
        ));
    };
    match block_index {
        Some(block_index) => block_is_encrypted(archive, block_index),
        None => Ok(false),
    }
}

fn build_entry_read_plans(
    archive: &sevenz_rust2::Archive,
    wanted: Option<&HashSet<Vec<u8>>>,
) -> Result<HashMap<usize, EntryReadPlan>, FormatError> {
    let mut selected_seen = vec![false; archive.blocks.len()];
    let mut plans = HashMap::with_capacity(archive.files.len());
    for (file_index, entry) in archive.files.iter().enumerate().rev() {
        let Some(block_index) = archive.stream_map.file_block_index.get(file_index).copied() else {
            return Err(FormatError::CorruptArchive(
                "7z stream map is shorter than its file list".into(),
            ));
        };
        let plan = match block_index {
            Some(block_index) => {
                let encrypted = block_is_encrypted(archive, block_index)?;
                let Some(seen) = selected_seen.get_mut(block_index) else {
                    return Err(FormatError::CorruptArchive(
                        "7z stream map references a missing block".into(),
                    ));
                };
                let plan = EntryReadPlan {
                    file_index,
                    block_index: Some(block_index),
                    selected_later_in_block: *seen,
                    encrypted,
                };
                if wanted.is_none_or(|paths| paths.contains(entry.name().as_bytes())) {
                    *seen = true;
                }
                plan
            }
            None => EntryReadPlan {
                file_index,
                ..EntryReadPlan::default()
            },
        };
        plans.insert(entry_identity(entry), plan);
    }
    Ok(plans)
}

type RemainingSelectedByBlock = HashMap<usize, BTreeMap<usize, EntryMeta>>;

fn build_remaining_selected_by_block(
    archive: &sevenz_rust2::Archive,
    plans: &HashMap<usize, EntryReadPlan>,
    wanted: Option<&HashSet<Vec<u8>>>,
) -> Result<RemainingSelectedByBlock, FormatError> {
    let mut remaining = HashMap::<usize, BTreeMap<usize, EntryMeta>>::new();
    for entry in &archive.files {
        if !wanted.is_none_or(|paths| paths.contains(entry.name().as_bytes())) {
            continue;
        }
        let plan = plans.get(&entry_identity(entry)).ok_or_else(|| {
            FormatError::CorruptArchive("7z entry is missing from its stream map".into())
        })?;
        if let Some(block_index) = plan.block_index {
            remaining
                .entry(block_index)
                .or_default()
                .insert(plan.file_index, meta_of(entry, plan.encrypted));
        }
    }
    Ok(remaining)
}

fn record_unprocessed_block_entries(
    sink: &mut ExtractSink<'_>,
    remaining: &mut RemainingSelectedByBlock,
    block_index: usize,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let Some(entries) = remaining.remove(&block_index) else {
        return Ok(());
    };
    let error = FormatError::CorruptArchive(
        "entry was not processed because an earlier item in its 7z block was damaged".into(),
    );
    for meta in entries.into_values() {
        sink.record_best_effort_failure(&meta, &error, ctl)?;
    }
    Ok(())
}

fn best_effort_recoverable(error: &FormatError) -> bool {
    matches!(
        error,
        FormatError::Io(_) | FormatError::CorruptArchive(_) | FormatError::Other(_)
    )
}

struct ReadErrorTracker<'r> {
    inner: &'r mut dyn Read,
    failed: bool,
}

impl Read for ReadErrorTracker<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(read) => Ok(read),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
}

fn classify_entry_read_error(
    error: FormatError,
    encrypted: bool,
    password_supplied: bool,
) -> FormatError {
    if encrypted && matches!(&error, FormatError::Io(_)) {
        if password_supplied {
            FormatError::WrongPassword
        } else {
            FormatError::PasswordRequired
        }
    } else {
        error
    }
}

#[allow(clippy::too_many_arguments)]
fn write_entry(
    sink: &mut ExtractSink<'_>,
    meta: &EntryMeta,
    out_path: &Path,
    reader: &mut dyn Read,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    password_supplied: bool,
) -> Result<(), FormatError> {
    let mut tracked = ReadErrorTracker {
        inner: reader,
        failed: false,
    };
    let result = sink.write_file(meta, out_path, &mut tracked, progress, ctl);
    if tracked.failed {
        result.map_err(|error| classify_entry_read_error(error, meta.encrypted, password_supplied))
    } else {
        result
    }
}

#[allow(clippy::too_many_arguments)]
fn write_entry_best_effort(
    sink: &mut ExtractSink<'_>,
    meta: &EntryMeta,
    out_path: &Path,
    reader: &mut dyn Read,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    password_supplied: bool,
) -> Result<bool, FormatError> {
    sink.write_file_best_effort_classified(meta, out_path, reader, progress, ctl, |error| {
        classify_entry_read_error(error, meta.encrypted, password_supplied)
    })
}

impl ArchiveReader for SevenZArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        let archive = self.inner.archive();
        Box::new(
            archive
                .files
                .iter()
                .enumerate()
                .map(move |(file_index, entry)| {
                    entry_is_encrypted(archive, file_index)
                        .map(|encrypted| meta_of(entry, encrypted))
                }),
        )
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        // The backend's random-access read decodes the containing block up
        // to the requested file and returns it fully decoded (preview-sized
        // usage; extraction streams instead).
        let encrypted = self
            .inner
            .archive()
            .files
            .iter()
            .position(|entry| entry.name() == path.display)
            .map(|index| entry_is_encrypted(self.inner.archive(), index))
            .transpose()?
            .unwrap_or(false);
        let data = self.inner.read_file(&path.display).map_err(|error| {
            classify_entry_read_error(map_7z_error(error), encrypted, self.password_supplied)
        })?;
        Ok(Box::new(Cursor::new(data)))
    }

    /// Single-pass extraction through the shared safety engine, streaming
    /// every entry in block order (the only efficient order for solid
    /// archives).
    fn extract(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.extract_with_report(dest, selection, opts, progress, ctl)
            .map(drop)
    }

    fn extract_with_report(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ExtractReport, FormatError> {
        if selection.is_some_and(<[EntryPath]>::is_empty) {
            return Ok(empty_extract_report(dest, progress));
        }
        let wanted: Option<HashSet<Vec<u8>>> =
            selection.map(|s| s.iter().map(|p| p.raw.clone()).collect());
        let password_supplied = self.password_supplied;
        let total: u64 = self
            .inner
            .archive()
            .files
            .iter()
            .filter(|e| {
                !e.is_directory()
                    && wanted
                        .as_ref()
                        .is_none_or(|w| w.contains(e.name().as_bytes()))
            })
            .map(|e| e.size())
            .sum();
        let entry_plans = build_entry_read_plans(self.inner.archive(), wanted.as_ref())?;
        let mut remaining_selected =
            build_remaining_selected_by_block(self.inner.archive(), &entry_plans, wanted.as_ref())?;
        let mut sink = ExtractSink::new(dest, opts, total)?;
        let mut failure: Option<FormatError> = None;
        let backend_result = self.inner.for_each_entries(|entry, reader| {
            // The backend may continue with a later non-solid block after
            // a callback returns false. Keep later callbacks side-effect
            // free once the first Squallz error has been recorded.
            if failure.is_some() {
                return Ok(false);
            }
            let Some(plan) = entry_plans.get(&entry_identity(entry)).copied() else {
                failure = Some(FormatError::CorruptArchive(
                    "7z entry is missing from its stream map".into(),
                ));
                return Ok(false);
            };
            let meta = meta_of(entry, plan.encrypted);
            let selected = wanted
                .as_ref()
                .is_none_or(|paths| paths.contains(meta.path.raw.as_slice()));
            if selected {
                if let Some(block_index) = plan.block_index {
                    if let Some(entries) = remaining_selected.get_mut(&block_index) {
                        entries.remove(&plan.file_index);
                    }
                }
            }
            let result = (|| -> Result<bool, FormatError> {
                if !selected {
                    if plan.block_index.is_none() {
                        Ok(true)
                    } else if plan.selected_later_in_block {
                        match drain_entry(reader, ctl).map_err(|error| {
                            classify_entry_read_error(error, meta.encrypted, password_supplied)
                        }) {
                            Ok(()) => Ok(true),
                            Err(error) if opts.best_effort && best_effort_recoverable(&error) => {
                                if let Some(block_index) = plan.block_index {
                                    record_unprocessed_block_entries(
                                        &mut sink,
                                        &mut remaining_selected,
                                        block_index,
                                        ctl,
                                    )?;
                                }
                                Ok(false)
                            }
                            Err(error) => Err(error),
                        }
                    } else {
                        // No later selected entry depends on this block. The
                        // backend can skip it and continue at the next block.
                        Ok(false)
                    }
                } else {
                    match meta.entry_type {
                        EntryType::File => {
                            sink.file_target(&meta, progress, ctl).and_then(|target| {
                                match target {
                                    Some(out_path) if opts.best_effort => {
                                        match write_entry_best_effort(
                                            &mut sink,
                                            &meta,
                                            &out_path,
                                            reader,
                                            progress,
                                            ctl,
                                            password_supplied,
                                        )? {
                                            true => Ok(true),
                                            false => {
                                                if let Some(block_index) = plan.block_index {
                                                    record_unprocessed_block_entries(
                                                        &mut sink,
                                                        &mut remaining_selected,
                                                        block_index,
                                                        ctl,
                                                    )?;
                                                }
                                                Ok(false)
                                            }
                                        }
                                    }
                                    Some(out_path) => write_entry(
                                        &mut sink,
                                        &meta,
                                        &out_path,
                                        reader,
                                        progress,
                                        ctl,
                                        password_supplied,
                                    )
                                    .map(|()| true),
                                    // A skipped solid entry is decoded only when a
                                    // later selected entry shares its block.
                                    None if plan.selected_later_in_block => {
                                        match drain_entry(reader, ctl).map_err(|error| {
                                            classify_entry_read_error(
                                                error,
                                                meta.encrypted,
                                                password_supplied,
                                            )
                                        }) {
                                            Ok(()) => Ok(true),
                                            Err(error)
                                                if opts.best_effort
                                                    && best_effort_recoverable(&error) =>
                                            {
                                                if let Some(block_index) = plan.block_index {
                                                    record_unprocessed_block_entries(
                                                        &mut sink,
                                                        &mut remaining_selected,
                                                        block_index,
                                                        ctl,
                                                    )?;
                                                }
                                                Ok(false)
                                            }
                                            Err(error) => Err(error),
                                        }
                                    }
                                    None => Ok(plan.block_index.is_none()),
                                }
                            })
                        }
                        _ => sink.write_meta_entry(&meta, progress, ctl).map(|()| true),
                    }
                }
            })();
            match result {
                Ok(continue_block) => Ok(continue_block),
                Err(e) => {
                    failure = Some(e);
                    Ok(false)
                }
            }
        });
        // Preserve the first shared-safety error even if the backend also
        // fails while unwinding or preparing a later block.
        if let Some(e) = failure {
            return Err(e);
        }
        backend_result.map_err(map_7z_error)?;
        Ok(sink.finish_with_report(progress))
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut problems = Vec::new();
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.push(problem))?;
        Ok(TestReport {
            entries_tested,
            problems,
            recovery: None,
        })
    }

    fn test_summary(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.record(problem))?;
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery: None,
        })
    }
}
