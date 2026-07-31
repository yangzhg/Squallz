//! ZIP update operations: append, delete, and rename.
//! Engine updates rewrite through caller-owned streams and let core own the
//! durable target transaction. The legacy direct format entry point retains
//! its same-directory temporary-file wrapper for API compatibility.
//!
//! Unchanged entries are **raw-copied** (no recompression; encrypted
//! entries stay encrypted without needing the password). Added files are
//! compressed with the usual create options.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use squallz_format_api::{
    ArchiveWriter, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType, FormatError,
    PreparedUpdateAdditions, ProgressPhase, ProgressSink, ReadSeek, UpdateOp, WriteSeek,
};
use zip::ZipArchive;

use super::error::map_zip_error;
use super::writer::ZipArchiveWriter;

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

/// One file-system item scheduled for addition.
struct AddItem {
    src: Option<PathBuf>,
    meta: EntryMeta,
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

struct LocalAdditions(Vec<AddItem>);

struct EngineAdditions<'a>(&'a mut dyn PreparedUpdateAdditions);

struct PreparedRewrite<'a> {
    archive: ZipArchive<RawCopySource<'a>>,
    raw_copy: RawCopyTracker,
    deletes: Option<GlobSet>,
    renames: HashMap<String, String>,
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

/// Executes an update run: plan → space pre-check → rewrite into a temp
/// file → atomic rename.
pub(super) fn update_archive(
    src: &Path,
    ops: &[UpdateOp],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let add_excludes = build_path_set(opts.excludes.iter().map(String::as_str))?;
    let mut additions = LocalAdditions(collect_add_items(ops, &add_excludes)?);

    update_archive_impl(src, ops, &mut additions, opts, progress, ctl)
}

pub(super) fn update_archive_with_prepared_additions(
    src: &Path,
    ops: &[UpdateOp],
    additions: &mut dyn PreparedUpdateAdditions,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    update_archive_impl(
        src,
        ops,
        &mut EngineAdditions(additions),
        opts,
        progress,
        ctl,
    )
}

#[allow(clippy::too_many_arguments)] // archive update inputs have distinct roles
pub(super) fn rewrite_archive(
    source: Box<dyn ReadSeek>,
    output: Box<dyn WriteSeek>,
    ops: &[UpdateOp],
    additions: &mut dyn PreparedUpdateAdditions,
    opts: &CreateOptions,
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

fn update_archive_impl(
    src: &Path,
    ops: &[UpdateOp],
    additions: &mut impl AdditionSet,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let src_file = File::open(src)?;
    let src_len = src_file.metadata()?.len();

    // Disk-space pre-check on the volume holding the temporary file.
    let added_bytes = addition_bytes(additions, ctl)?;
    let needed = staging_bytes_estimate(src_len, added_bytes);
    let available = fs4::available_space(update_parent(src))?;
    if available < needed {
        return Err(FormatError::DiskFull);
    }

    let source: Box<dyn ReadSeek> = Box::new(src_file);
    let PreparedRewrite {
        mut archive,
        raw_copy,
        deletes,
        renames,
    } = prepare_update(source, ops, additions, progress, ctl)?;

    let tmp = update_temp_path(src);
    let result = (|| -> Result<(), FormatError> {
        let out = File::create(&tmp)?;
        progress.on_phase(ProgressPhase::UpdateRewrite, true);
        rewrite(
            &mut archive,
            &raw_copy,
            Box::new(out),
            &deletes,
            &renames,
            additions,
            opts,
            progress,
            ctl,
        )
    })();
    match result {
        Ok(()) => {
            // Same-directory rename: atomic on POSIX file systems.
            std::fs::rename(&tmp, src)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)] // archive update inputs have distinct roles
fn rewrite_archive_impl(
    source: Box<dyn ReadSeek>,
    output: Box<dyn WriteSeek>,
    ops: &[UpdateOp],
    additions: &mut impl AdditionSet,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let PreparedRewrite {
        mut archive,
        raw_copy,
        deletes,
        renames,
    } = prepare_update(source, ops, additions, progress, ctl)?;
    rewrite(
        &mut archive,
        &raw_copy,
        output,
        &deletes,
        &renames,
        additions,
        opts,
        progress,
        ctl,
    )
}

fn prepare_update<'a>(
    source: Box<dyn ReadSeek>,
    ops: &[UpdateOp],
    additions: &impl AdditionSet,
    progress: &'a dyn ProgressSink,
    ctl: &'a ControlToken,
) -> Result<PreparedRewrite<'a>, FormatError> {
    ctl.checkpoint()?;
    let deletes = build_path_set(ops.iter().filter_map(|op| match op {
        UpdateOp::Delete { pattern } => Some(pattern.as_str()),
        _ => None,
    }))?;
    let renames = build_rename_map(ops);
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

    // Update targets must be deterministic: no missing rename sources, no
    // accidental overwrite, and no duplicate targets in the same operation.
    validate_update_plan(&mut archive, &deletes, &renames, additions, ctl)?;
    Ok(PreparedRewrite {
        archive,
        raw_copy,
        deletes,
        renames,
    })
}

fn update_parent(src: &Path) -> &Path {
    match src.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn update_temp_path(src: &Path) -> PathBuf {
    let file_name = match src.file_name().filter(|name| !name.is_empty()) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "archive".to_owned(),
    };
    src.with_file_name(format!(
        ".{file_name}.sqz-update-{}.tmp",
        std::process::id()
    ))
}

/// Writes the updated archive into the caller-owned output stream.
#[allow(clippy::too_many_arguments)] // internal plumbing with distinct roles
fn rewrite(
    archive: &mut ZipArchive<RawCopySource<'_>>,
    raw_copy: &RawCopyTracker,
    output: Box<dyn WriteSeek>,
    deletes: &Option<GlobSet>,
    renames: &HashMap<String, String>,
    additions: &mut impl AdditionSet,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut writer = ZipArchiveWriter::new_with_control(output, opts, ctl);

    // Progress in bytes: raw (compressed) bytes for copies, plain bytes for
    // additions.
    let mut copied_total = 0u64;
    for index in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(index)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        let name = String::from_utf8_lossy(file.name_raw());
        let key = name.trim_end_matches('/');
        if !deletes.as_ref().is_some_and(|set| set.is_match(key)) {
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
        let name = String::from_utf8_lossy(file.name_raw()).into_owned();
        let key = name.trim_end_matches('/').to_string();
        let compressed = file.compressed_size();
        let path = EntryPath::from_utf8(name.clone());
        progress.on_progress(done, total, &path);
        if deletes.as_ref().is_some_and(|set| set.is_match(&key)) {
            continue; // dropped entry
        }
        let rename_to = renames.get(&name).or_else(|| renames.get(&key));
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

impl AdditionSet for LocalAdditions {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn meta(&self, index: usize) -> Option<&EntryMeta> {
        self.0.get(index).map(|item| &item.meta)
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
        let item = self.0.get(index).ok_or_else(|| {
            FormatError::Other(format!("update entry index {index} is out of range"))
        })?;
        if matches!(item.meta.entry_type, EntryType::File) {
            let src = item
                .src
                .as_ref()
                .ok_or_else(|| FormatError::Other("file add missing source path".into()))?;
            let mut data = File::open(src)?;
            writer.add_entry(&item.meta, Some(&mut data))
        } else {
            writer.add_entry(&item.meta, None)
        }
    }
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

/// Compiles path globs. Each pattern is expanded the same way as the
/// engine-side `PathFilter` so that bare names match at any depth and matched
/// directories prune their subtree.
fn build_path_set<'a>(
    patterns: impl Iterator<Item = &'a str>,
) -> Result<Option<GlobSet>, FormatError> {
    let patterns: Vec<&str> = patterns.collect();
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let p = pattern.trim_end_matches('/');
        let mut variants = vec![p.to_owned(), format!("{p}/**")];
        if !p.contains('/') {
            variants.push(format!("**/{p}"));
            variants.push(format!("**/{p}/**"));
        }
        for variant in variants {
            let glob = GlobBuilder::new(&variant)
                .literal_separator(true)
                .build()
                .map_err(|e| {
                    FormatError::Other(format!("invalid glob pattern '{pattern}': {e}"))
                })?;
            builder.add(glob);
        }
    }
    let set = builder
        .build()
        .map_err(|e| FormatError::Other(format!("invalid glob pattern set: {e}")))?;
    Ok(Some(set))
}

/// Maps old entry names to new ones.
fn build_rename_map(ops: &[UpdateOp]) -> HashMap<String, String> {
    ops.iter()
        .filter_map(|op| match op {
            UpdateOp::Rename { from, to } => Some((from.display.clone(), to.display.clone())),
            _ => None,
        })
        .collect()
}

/// Rejects update plans that would silently overwrite or duplicate entries.
fn validate_update_plan<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    deletes: &Option<GlobSet>,
    renames: &HashMap<String, String>,
    additions: &impl AdditionSet,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    ctl.checkpoint()?;
    let mut names: Vec<(String, String)> = Vec::with_capacity(archive.len());
    let mut exact_names = HashSet::with_capacity(archive.len());
    let mut existing = HashSet::new();
    for i in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(i)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        let name = String::from_utf8_lossy(file.name_raw()).into_owned();
        let key = archive_key(&name);
        existing.insert(key.clone());
        exact_names.insert(name.clone());
        names.push((name, key));
    }
    for from in renames.keys() {
        ctl.checkpoint()?;
        let from_key = archive_key(from);
        let found = exact_names.contains(from) || existing.contains(&from_key);
        if !found {
            return Err(FormatError::Other(format!(
                "rename source not found in archive: {from}"
            )));
        }
    }
    let mut removed = HashSet::new();
    for (name, key) in &names {
        ctl.checkpoint()?;
        if deletes.as_ref().is_some_and(|set| set.is_match(key))
            || renames.contains_key(name)
            || renames.contains_key(key)
        {
            removed.insert(key.clone());
        }
    }
    let mut produced = HashMap::new();
    for target in renames.values() {
        ctl.checkpoint()?;
        validate_update_target(target, &existing, &removed, &mut produced)?;
    }
    for index in 0..additions.len() {
        ctl.checkpoint()?;
        let meta = addition_meta(additions, index)?;
        validate_update_target(&meta.path.display, &existing, &removed, &mut produced)?;
    }
    ctl.checkpoint()?;
    Ok(())
}

fn validate_update_target(
    target: &str,
    existing: &HashSet<String>,
    removed: &HashSet<String>,
    produced: &mut HashMap<String, String>,
) -> Result<(), FormatError> {
    let key = archive_key(target);
    if key.is_empty() {
        return Err(FormatError::Other(
            "update target path cannot be empty".into(),
        ));
    }
    if existing.contains(&key) && !removed.contains(&key) {
        return Err(FormatError::Other(format!(
            "update target already exists in archive: {target}"
        )));
    }
    if let Some(previous) = produced.insert(key, target.to_string()) {
        return Err(FormatError::Other(format!(
            "duplicate update target in archive: {previous} and {target}"
        )));
    }
    Ok(())
}

fn archive_key(name: &str) -> String {
    name.trim_end_matches('/').to_string()
}

/// Walks the `Add` operations into a flat item list (directories
/// recursively, symlinks preserved as link entries), applying create/update
/// excludes to the destination paths inside the archive.
fn collect_add_items(
    ops: &[UpdateOp],
    excludes: &Option<GlobSet>,
) -> Result<Vec<AddItem>, FormatError> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            UpdateOp::Add { src, dest } => walk_add(src, &dest.display, excludes, &mut out)?,
            UpdateOp::AddDir { path } => push_add_dir(&mut out, &path.display, excludes)?,
            _ => {}
        }
    }
    Ok(out)
}

fn push_add_dir(
    out: &mut Vec<AddItem>,
    name: &str,
    excludes: &Option<GlobSet>,
) -> Result<(), FormatError> {
    let normalized = name.trim_end_matches('/');
    if normalized.is_empty() {
        return Err(FormatError::Other("directory path cannot be empty".into()));
    }
    if excludes
        .as_ref()
        .is_some_and(|set| set.is_match(normalized))
    {
        return Ok(());
    }
    out.push(AddItem {
        src: None,
        meta: EntryMeta {
            path: EntryPath::from_utf8(format!("{normalized}/")),
            entry_type: EntryType::Dir,
            size: 0,
            compressed_size: None,
            modified: Some(SystemTime::now()),
            unix_mode: Some(0o755),
            crc32: None,
            encrypted: false,
        },
    });
    Ok(())
}

fn walk_add(
    path: &Path,
    name: &str,
    excludes: &Option<GlobSet>,
    out: &mut Vec<AddItem>,
) -> Result<(), FormatError> {
    let key = name.trim_end_matches('/');
    if excludes.as_ref().is_some_and(|set| set.is_match(key)) {
        return Ok(());
    }
    let fs_meta = std::fs::symlink_metadata(path)?;
    let entry_type = if fs_meta.file_type().is_symlink() {
        EntryType::Symlink {
            target: std::fs::read_link(path)?
                .to_string_lossy()
                .into_owned()
                .into_bytes(),
        }
    } else if fs_meta.is_dir() {
        EntryType::Dir
    } else {
        EntryType::File
    };
    let is_dir = matches!(entry_type, EntryType::Dir);
    out.push(AddItem {
        src: Some(path.to_path_buf()),
        meta: EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type,
            size: if is_dir { 0 } else { fs_meta.len() },
            compressed_size: None,
            modified: fs_meta.modified().ok().or(Some(SystemTime::now())),
            unix_mode: unix_mode_of(&fs_meta),
            crc32: None,
            encrypted: false,
        },
    });
    if is_dir {
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        children.sort();
        for child in children {
            let child_name = child_archive_name(&child)?;
            walk_add(&child, &format!("{name}/{child_name}"), excludes, out)?;
        }
    }
    Ok(())
}

fn child_archive_name(child: &Path) -> Result<String, FormatError> {
    let Some(name) = child.file_name().filter(|name| !name.is_empty()) else {
        return Err(FormatError::UnsafeFileName(child.display().to_string()));
    };
    Ok(name.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn unix_mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode())
}

#[cfg(not(unix))]
fn unix_mode_of(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Write};
    use std::sync::atomic::AtomicUsize;

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
        let additions = LocalAdditions(Vec::new());

        let result = prepare_update(
            Box::new(source),
            &[],
            &additions,
            &progress,
            control.as_ref(),
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
    }

    #[test]
    fn cancelled_update_stops_before_addition_sizing() {
        let control = ControlToken::default();
        control.cancel();

        let result = addition_bytes(&LocalAdditions(Vec::new()), &control);

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
            &CreateOptions::default(),
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

    #[test]
    fn parentless_update_path_uses_current_directory() {
        assert_eq!(update_parent(Path::new("archive.zip")), Path::new("."));
        assert_eq!(
            update_parent(Path::new("nested/archive.zip")),
            Path::new("nested")
        );
    }

    #[test]
    fn update_temp_path_uses_archive_name_or_archive_fallback() {
        let parentless = update_temp_path(Path::new("archive.zip"));
        let parentless_name = parentless
            .file_name()
            .and_then(|name| name.to_str())
            .expect("parentless temp file name");
        assert!(parentless_name.starts_with(".archive.zip.sqz-update-"));
        assert!(parentless_name.ends_with(".tmp"));

        let nameless = update_temp_path(Path::new("/"));
        let nameless_name = nameless
            .file_name()
            .and_then(|name| name.to_str())
            .expect("nameless temp file name");
        assert!(nameless_name.starts_with(".archive.sqz-update-"));
        assert!(nameless_name.ends_with(".tmp"));
    }

    #[test]
    fn child_archive_name_rejects_empty_child_paths() {
        let err = child_archive_name(Path::new("")).expect_err("empty child path rejected");
        assert!(matches!(err, FormatError::UnsafeFileName(_)));
    }
}
