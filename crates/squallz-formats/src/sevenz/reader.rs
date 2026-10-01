//! 7Z read side: entry listing, single-entry reads, single-pass conversion,
//! extraction and integrity testing. Solid blocks force sequential decoding,
//! so complete archive operations stream every entry exactly once through
//! `for_each_entries`; `read_entry` (preview path) decodes up to the
//! requested file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use sevenz_rust2::ArchiveEntry;
use squallz_format_api::{
    empty_extract_report, ArchiveReader, BoundedProblemLog, ControlToken, EntryMeta, EntryPath,
    EntryStreamConsumer, EntryType, ExtractOptions, ExtractReport, ExtractSink, FormatError,
    OpenOptions, ProgressSink, ReadSeek, SymlinkPolicy, TestSummary, TEST_PROBLEM_PREVIEW_LIMIT,
};

use super::streams::EntryStreams;
use super::{map_7z_error, FILE_ATTRIBUTE_UNIX_EXTENSION};

/// Chunk size when draining entry data (test pass).
const READ_CHUNK: usize = 64 * 1024;
const MAX_SYMLINK_TARGET_BYTES: usize = u16::MAX as usize;

/// Read handle over a 7z archive.
pub(super) struct SevenZArchiveReader {
    inner: EntryStreams,
    password_supplied: bool,
    control: ControlToken,
}

impl SevenZArchiveReader {
    pub(super) fn open(src: Box<dyn ReadSeek>, opts: &OpenOptions) -> Result<Self, FormatError> {
        Self::open_controlled(src, opts, &ControlToken::default())
    }

    pub(super) fn open_controlled(
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        let password = open_password(opts);
        // Opening a header-encrypted archive without a password surfaces
        // PasswordRequired here.
        let inner = EntryStreams::new(src, password).map_err(map_7z_error)?;
        ctl.checkpoint()?;
        Ok(Self {
            inner,
            password_supplied: opts.password.is_some(),
            control: ctl.clone(),
        })
    }

    fn listed_entries(&mut self) -> Result<Vec<EntryMeta>, FormatError> {
        self.control.checkpoint()?;
        let archive = self.inner.archive();
        let mut metas = archive
            .files
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                self.control.checkpoint()?;
                if is_symlink(entry) && entry.size() == 0 {
                    return Err(FormatError::CorruptArchive(
                        "7z symlink target is empty".into(),
                    ));
                }
                entry_is_encrypted(archive, index).map(|encrypted| meta_of(entry, encrypted))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let wanted: HashSet<_> = metas
            .iter()
            .filter(|meta| {
                matches!(meta.entry_type, EntryType::Symlink { .. })
                    && (!meta.encrypted || self.password_supplied)
            })
            .map(|meta| meta.path.raw.clone())
            .collect();
        if wanted.is_empty() {
            return Ok(metas);
        }
        let plans = build_entry_read_plans(archive, Some(&wanted))?;
        let blocks: HashSet<_> = archive
            .files
            .iter()
            .filter(|entry| is_symlink(entry) && wanted.contains(entry.name().as_bytes()))
            .filter_map(|entry| {
                plans
                    .get(&entry_identity(entry))
                    .and_then(|plan| plan.block_index)
            })
            .collect();
        let mut failure = None;
        let ctl = &self.control;
        let password_supplied = self.password_supplied;
        let result = self
            .inner
            .for_each_in_blocks(Some(&blocks), |entry, reader| {
                if failure.is_some() {
                    return Ok(false);
                }
                let Some(plan) = plans.get(&entry_identity(entry)) else {
                    failure = Some(FormatError::CorruptArchive(
                        "7z entry is missing from its stream map".into(),
                    ));
                    return Ok(false);
                };
                let result = (|| {
                    ctl.checkpoint()?;
                    if is_symlink(entry) && wanted.contains(entry.name().as_bytes()) {
                        let target = read_symlink_target(reader, ctl)?;
                        let meta = metas.get_mut(plan.file_index).ok_or_else(|| {
                            FormatError::CorruptArchive(
                                "7z entry is missing from its file list".into(),
                            )
                        })?;
                        meta.entry_type = EntryType::Symlink { target };
                    } else if plan.selected_later_in_block {
                        drain_entry(reader, ctl)?;
                    }
                    Ok(plan.selected_later_in_block)
                })();
                match result {
                    Ok(next) => Ok(next),
                    Err(error) => {
                        failure = Some(classify_entry_read_error(
                            error,
                            plan.encrypted,
                            password_supplied,
                        ));
                        Ok(false)
                    }
                }
            });
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(map_7z_error)?;
        Ok(metas)
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
        let password_supplied = self.password_supplied;
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
                        let error = classify_entry_read_error(
                            FormatError::from(e),
                            meta.encrypted,
                            password_supplied,
                        );
                        if matches!(
                            error,
                            FormatError::PasswordRequired | FormatError::WrongPassword
                        ) {
                            mapping_failure = Some(error);
                            return Ok(false);
                        }
                        record_problem(format!("{}: {error}", meta.path));
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

fn unix_attributes(entry: &ArchiveEntry) -> Option<u32> {
    let attributes = entry.windows_attributes();
    (entry.has_windows_attributes && attributes & FILE_ATTRIBUTE_UNIX_EXTENSION != 0)
        .then_some(attributes >> 16)
}

fn is_symlink(entry: &ArchiveEntry) -> bool {
    unix_attributes(entry).is_some_and(|mode| mode & 0o170000 == 0o120000)
}

/// Builds metadata from the header. Link targets are decoded separately
/// from their bounded entry stream, never interpreted as ordinary files.
fn meta_of(entry: &ArchiveEntry, encrypted: bool) -> EntryMeta {
    let entry_type = if is_symlink(entry) {
        EntryType::Symlink { target: Vec::new() }
    } else if entry.is_directory() {
        EntryType::Dir
    } else {
        EntryType::File
    };
    // p7zip stores Unix permissions in the high attribute bits.
    let unix_mode = unix_attributes(entry).map(|mode| mode & 0o7777);
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

#[cfg(test)]
mod tests;

fn read_symlink_target(reader: &mut dyn Read, ctl: &ControlToken) -> Result<Vec<u8>, FormatError> {
    ctl.checkpoint()?;
    let mut target = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        ctl.checkpoint()?;
        let remaining = (MAX_SYMLINK_TARGET_BYTES + 1 - target.len()).min(chunk.len());
        let read = reader.read(&mut chunk[..remaining])?;
        if read == 0 {
            break;
        }
        target.extend_from_slice(&chunk[..read]);
        if target.len() > MAX_SYMLINK_TARGET_BYTES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "7z symlink target exceeds the {MAX_SYMLINK_TARGET_BYTES}-byte limit"
            )));
        }
    }
    if target.is_empty() || target.contains(&0) || std::str::from_utf8(&target).is_err() {
        return Err(FormatError::CorruptArchive(
            "7z symlink target is not a nonempty UTF-8 path".into(),
        ));
    }
    Ok(target)
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

fn skip_entry_stream(
    reader: &mut dyn Read,
    sink: &mut ExtractSink<'_>,
    remaining: &mut RemainingSelectedByBlock,
    plan: EntryReadPlan,
    best_effort: bool,
    password_supplied: bool,
    ctl: &ControlToken,
) -> Result<bool, FormatError> {
    ctl.checkpoint()?;
    if !plan.selected_later_in_block {
        return Ok(plan.block_index.is_none());
    }
    match drain_entry(reader, ctl)
        .map_err(|error| classify_entry_read_error(error, plan.encrypted, password_supplied))
    {
        Ok(()) => Ok(true),
        Err(error) if best_effort && best_effort_recoverable(&error) => {
            if let Some(block) = plan.block_index {
                record_unprocessed_block_entries(sink, remaining, block, ctl)?;
            }
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

struct ReadErrorTracker<'r> {
    inner: &'r mut dyn Read,
    failed: bool,
    control: &'r ControlToken,
}

impl Read for ReadErrorTracker<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.control.checkpoint().map_err(std::io::Error::other)?;
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

fn consume_entry_data(
    reader: &mut dyn Read,
    ctl: &ControlToken,
    encrypted: bool,
    password_supplied: bool,
    consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    let mut tracked = ReadErrorTracker {
        inner: reader,
        failed: false,
        control: ctl,
    };
    let result = consume(&mut tracked);
    if tracked.failed {
        result.map_err(|error| classify_entry_read_error(error, encrypted, password_supplied))
    } else {
        result
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
    consume_entry_data(
        reader,
        ctl,
        meta.encrypted,
        password_supplied,
        &mut |data| sink.write_file(meta, out_path, data, progress, ctl),
    )
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
        if !self.inner.archive().files.iter().any(is_symlink) {
            let archive = self.inner.archive();
            return Box::new(archive.files.iter().enumerate().map(|(index, entry)| {
                self.control.checkpoint()?;
                entry_is_encrypted(archive, index).map(|encrypted| meta_of(entry, encrypted))
            }));
        }
        match self.listed_entries() {
            Ok(entries) => Box::new(entries.into_iter().map(Ok)),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }

    fn read_entry(
        &mut self,
        path: &EntryPath,
        consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        self.control.checkpoint()?;
        let archive = self.inner.archive();
        let index = archive
            .files
            .iter()
            .rposition(|entry| entry.name() == path.display)
            .ok_or_else(|| map_7z_error(sevenz_rust2::Error::FileNotFound))?;
        let encrypted = entry_is_encrypted(archive, index)?;
        let block = archive
            .stream_map
            .file_block_index
            .get(index)
            .ok_or_else(|| {
                FormatError::CorruptArchive("7z stream map is shorter than its file list".into())
            })?;
        let Some(block) = *block else {
            return consume(&mut std::io::empty());
        };
        let wanted = entry_identity(&archive.files[index]);
        let ctl = &self.control;
        let password_supplied = self.password_supplied;
        let mut found = false;
        let mut failure = None;
        let result =
            self.inner
                .for_each_in_blocks(Some(&HashSet::from([block])), |entry, reader| {
                    let result = (|| {
                        ctl.checkpoint()?;
                        if entry_identity(entry) != wanted {
                            drain_entry(reader, ctl).map_err(|error| {
                                classify_entry_read_error(error, encrypted, password_supplied)
                            })?;
                            return Ok(true);
                        }
                        found = true;
                        consume_entry_data(reader, ctl, encrypted, password_supplied, consume)?;
                        Ok(false)
                    })();
                    match result {
                        Ok(next) => Ok(next),
                        Err(error) => {
                            // Keep consumer failures out of the decoder's password
                            // classification (for example, a destination write error).
                            failure = Some(error);
                            Ok(false)
                        }
                    }
                });
        if ctl.is_cancelled() {
            return Err(FormatError::Cancelled);
        }
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(|error| {
            classify_entry_read_error(map_7z_error(error), encrypted, password_supplied)
        })?;
        if !found {
            return Err(map_7z_error(sevenz_rust2::Error::FileNotFound));
        }
        Ok(())
    }

    fn read_entries(
        &mut self,
        entries: &[EntryMeta],
        consume: &mut EntryStreamConsumer<'_>,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        ctl.checkpoint()?;
        let archive = self.inner.archive();
        if entries.len() != archive.files.len() {
            return Err(FormatError::Other(
                "7z entry listing does not match the reader".into(),
            ));
        }
        for (meta, entry) in entries.iter().zip(&archive.files) {
            ctl.checkpoint()?;
            let same_type = match meta.entry_type {
                EntryType::Symlink { .. } => is_symlink(entry),
                EntryType::Dir => entry.is_directory() && !is_symlink(entry),
                EntryType::File => !entry.is_directory() && !is_symlink(entry),
                _ => false,
            };
            if meta.path.raw != entry.name().as_bytes() || meta.size != entry.size() || !same_type {
                return Err(FormatError::Other(
                    "7z entry listing does not match the reader".into(),
                ));
            }
        }
        let plans = build_entry_read_plans(archive, None)?;
        let password_supplied = self.password_supplied;
        let mut visited = 0;
        let mut failure = None;
        let result = self.inner.for_each_entries(|entry, data| {
            let result = (|| {
                ctl.checkpoint()?;
                let plan = plans.get(&entry_identity(entry)).ok_or_else(|| {
                    FormatError::CorruptArchive("7z entry is missing from its stream map".into())
                })?;
                let meta = &entries[plan.file_index];
                if is_symlink(entry) {
                    let target = read_symlink_target(data, ctl).map_err(|error| {
                        classify_entry_read_error(error, plan.encrypted, password_supplied)
                    })?;
                    let mut resolved = meta.clone();
                    resolved.entry_type = EntryType::Symlink { target };
                    consume(&resolved, None)?;
                } else {
                    consume_entry_data(
                        data,
                        ctl,
                        plan.encrypted,
                        password_supplied,
                        &mut |data| {
                            if matches!(meta.entry_type, EntryType::File) {
                                consume(meta, Some(data))?;
                            } else {
                                consume(meta, None)?;
                            }
                            drain_entry(data, ctl)
                        },
                    )?;
                }
                ctl.checkpoint()?;
                visited += 1;
                Ok(true)
            })();
            match result {
                Ok(next) => Ok(next),
                Err(error) => {
                    failure = Some(error);
                    Ok(false)
                }
            }
        });
        ctl.checkpoint()?;
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(map_7z_error)?;
        if visited != entries.len() {
            return Err(FormatError::CorruptArchive(
                "7z stream map did not visit every entry".into(),
            ));
        }
        Ok(())
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
                    && !is_symlink(e)
                    && wanted
                        .as_ref()
                        .is_none_or(|w| w.contains(e.name().as_bytes()))
            })
            .map(|e| e.size())
            .sum();
        let entry_plans = build_entry_read_plans(self.inner.archive(), wanted.as_ref())?;
        let data_blocks = self
            .inner
            .archive()
            .files
            .iter()
            .filter(|entry| {
                wanted
                    .as_ref()
                    .is_none_or(|paths| paths.contains(entry.name().as_bytes()))
                    && !(is_symlink(entry) && opts.symlinks == SymlinkPolicy::Skip)
            })
            .filter_map(|entry| {
                entry_plans
                    .get(&entry_identity(entry))
                    .and_then(|plan| plan.block_index)
            })
            .collect();
        let mut remaining_selected =
            build_remaining_selected_by_block(self.inner.archive(), &entry_plans, wanted.as_ref())?;
        let mut sink = ExtractSink::new(dest, opts, total, progress)?;
        let mut failure: Option<FormatError> = None;
        let backend_result =
            self.inner
                .for_each_entries_with_data_blocks(&data_blocks, |entry, reader| {
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
                    let mut meta = meta_of(entry, plan.encrypted);
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
                            skip_entry_stream(
                                reader,
                                &mut sink,
                                &mut remaining_selected,
                                plan,
                                opts.best_effort,
                                password_supplied,
                                ctl,
                            )
                        } else {
                            match meta.entry_type {
                                EntryType::Symlink { .. } => {
                                    if opts.symlinks == SymlinkPolicy::Skip {
                                        sink.write_meta_entry(&meta, progress, ctl)?;
                                        return skip_entry_stream(
                                            reader,
                                            &mut sink,
                                            &mut remaining_selected,
                                            plan,
                                            opts.best_effort,
                                            password_supplied,
                                            ctl,
                                        );
                                    }
                                    match read_symlink_target(reader, ctl).map_err(|error| {
                                        classify_entry_read_error(
                                            error,
                                            meta.encrypted,
                                            password_supplied,
                                        )
                                    }) {
                                        Ok(target) => {
                                            meta.entry_type = EntryType::Symlink { target }
                                        }
                                        Err(error)
                                            if opts.best_effort
                                                && best_effort_recoverable(&error) =>
                                        {
                                            sink.record_best_effort_failure(&meta, &error, ctl)?;
                                            if let Some(block) = plan.block_index {
                                                record_unprocessed_block_entries(
                                                    &mut sink,
                                                    &mut remaining_selected,
                                                    block,
                                                    ctl,
                                                )?;
                                            }
                                            return Ok(false);
                                        }
                                        Err(error) => return Err(error),
                                    }
                                    sink.write_meta_entry(&meta, progress, ctl)?;
                                    Ok(true)
                                }
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
                                                        if let Some(block_index) = plan.block_index
                                                        {
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
                                            None => skip_entry_stream(
                                                reader,
                                                &mut sink,
                                                &mut remaining_selected,
                                                plan,
                                                opts.best_effort,
                                                password_supplied,
                                                ctl,
                                            ),
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
        sink.finish_with_report(progress, ctl)
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
