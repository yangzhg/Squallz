//! Shared safe extraction engine.
//!
//! Two layers:
//! - [`ExtractSink`]: per-entry writing with the full safety model of
//!   the extraction safety contract (Zip-Slip rejection, decompression-bomb guardrails,
//!   symlink-breakout protection, overwrite/symlink policies, permission
//!   restore, byte-accurate progress). Formats with their own iteration
//!   order (single-pass tar streams, solid 7z blocks) drive it directly.
//! - [`extract_entries`]: drives any [`ArchiveReader`] through its
//!   `entries()` + `read_entry()` primitives into an [`ExtractSink`]. This
//!   is the default body of [`ArchiveReader::extract`].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::entry::{EntryMeta, EntryPath, EntryType};
use crate::error::FormatError;
use crate::links::LinkResolver;
use crate::options::{ConflictDecision, ExtractOptions, OverwritePolicy, SymlinkPolicy};
use crate::progress::{ControlToken, ProgressSink};
use crate::safety::{crosses_created_symlink, sanitize_entry_path, LimitsAccountant};
use crate::traits::ArchiveReader;

/// Entry outcomes from a completed extraction.
///
/// `created`, `replaced`, and `renamed` count successfully materialized
/// non-directory entries. `directories` counts successfully created or
/// merged directory entries. `failed` is limited to recoverable per-entry
/// failures accepted by best-effort mode; a fatal extraction error is still
/// returned as [`FormatError`] and has no completed report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractReport {
    /// Final destination directory used by the extraction engine.
    pub destination: PathBuf,
    /// Selected archive entries processed by this completed run.
    pub selected_entries: u64,
    /// Non-directory entries materialized at their requested paths without an
    /// observed conflict.
    pub created: u64,
    /// Directory entries successfully created or merged.
    pub directories: u64,
    /// Entries deliberately left unmaterialized by conflict/link policy or
    /// because their link target could not be materialized safely.
    pub skipped: u64,
    /// Entries materialized after an observed destination conflict was
    /// resolved by replacement.
    pub replaced: u64,
    /// Entries materialized under a conflict-free sibling name.
    pub renamed: u64,
    /// Recoverable entry failures accepted by best-effort extraction.
    pub failed: u64,
    /// Bytes successfully committed as file content. Followed symlinks and
    /// hard-link fallbacks contribute when they require a content copy;
    /// metadata-only directories and preserved links contribute zero.
    pub output_bytes: u64,
}

/// Completes an explicitly empty extraction selection without touching the
/// destination filesystem. Optimized readers use this shared result so an
/// empty selection has the same behavior as the default extraction driver.
pub fn empty_extract_report(dest: &Path, progress: &dyn ProgressSink) -> ExtractReport {
    progress.on_progress(0, 0, &EntryPath::from_utf8(""));
    ExtractReport {
        destination: dest.to_path_buf(),
        ..ExtractReport::default()
    }
}

#[derive(Debug, Clone, Copy)]
enum Materialization {
    Created,
    Replaced,
    Renamed,
}

#[derive(Debug)]
struct ResolvedOutput {
    path: PathBuf,
    replace_existing: bool,
    materialization: Materialization,
}

/// Copy chunk size; cancellation, limits and progress are checked at this
/// granularity.
const COPY_CHUNK: usize = 64 * 1024;
const TEMP_PATH_ATTEMPTS: u64 = 1_024;
static TEMP_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn create_pending_value<T>(
    target: &Path,
    mut create: impl FnMut(&Path) -> std::io::Result<T>,
) -> Result<(PathBuf, T), FormatError> {
    let parent = parent_or_empty(target);
    for _ in 0..TEMP_PATH_ATTEMPTS {
        let sequence = TEMP_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".squallz-extract-{}-{sequence}.tmp",
            std::process::id()
        ));
        match create(&path) {
            Ok(value) => return Ok((path, value)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Other(
        "could not allocate an extraction staging file".into(),
    ))
}

/// Same-directory output staged until one archive entry has been read in
/// full. Dropping an unfinished value removes the partial file.
struct PendingOutput {
    path: Option<PathBuf>,
    file: Option<fs::File>,
}

/// Empty destination placeholder used to keep a no-replace decision valid
/// until the staged file is renamed. If the rename fails, the placeholder is
/// removed with the staged file.
struct TargetReservation {
    path: Option<PathBuf>,
}

impl TargetReservation {
    fn create(target: &Path) -> Result<Self, FormatError> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        Ok(Self {
            path: Some(target.to_path_buf()),
        })
    }

    fn keep(mut self) {
        self.path.take();
    }
}

impl Drop for TargetReservation {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl PendingOutput {
    fn create(target: &Path) -> Result<Self, FormatError> {
        let (path, file) = create_pending_value(target, |path| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        })?;
        Ok(Self {
            path: Some(path),
            file: Some(file),
        })
    }

    fn hard_link(source: &Path, target: &Path) -> Result<Self, FormatError> {
        let (path, ()) = create_pending_value(target, |path| fs::hard_link(source, path))?;
        Ok(Self {
            path: Some(path),
            file: None,
        })
    }

    #[cfg(unix)]
    fn symlink(
        link_target: &Path,
        target: &Path,
        _target_is_dir: bool,
    ) -> Result<Option<Self>, FormatError> {
        let (path, ()) =
            create_pending_value(target, |path| std::os::unix::fs::symlink(link_target, path))?;
        Ok(Some(Self {
            path: Some(path),
            file: None,
        }))
    }

    #[cfg(windows)]
    fn symlink(
        link_target: &Path,
        target: &Path,
        target_is_dir: bool,
    ) -> Result<Option<Self>, FormatError> {
        let result = create_pending_value(target, |path| {
            if target_is_dir {
                std::os::windows::fs::symlink_dir(link_target, path)
            } else {
                std::os::windows::fs::symlink_file(link_target, path)
            }
        });
        match result {
            Ok((path, ())) => Ok(Some(Self {
                path: Some(path),
                file: None,
            })),
            Err(FormatError::Io(error)) if is_windows_symlink_privilege_error(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn symlink(
        _link_target: &Path,
        _target: &Path,
        _target_is_dir: bool,
    ) -> Result<Option<Self>, FormatError> {
        Ok(None)
    }

    fn file_mut(&mut self) -> Result<&mut fs::File, FormatError> {
        self.file
            .as_mut()
            .ok_or_else(|| FormatError::Other("extraction staging file is closed".into()))
    }

    fn path(&self) -> Result<&Path, FormatError> {
        self.path
            .as_deref()
            .ok_or_else(|| FormatError::Other("extraction staging path is unavailable".into()))
    }

    fn commit(self, target: &Path, replace_existing: bool) -> Result<(), FormatError> {
        self.commit_using(
            target,
            replace_existing,
            |source, destination| fs::hard_link(source, destination),
            |source, destination| fs::rename(source, destination),
        )
    }

    fn commit_using(
        mut self,
        target: &Path,
        replace_existing: bool,
        hard_link: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
        rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(), FormatError> {
        self.file.take();
        let staged = self
            .path
            .as_deref()
            .ok_or_else(|| FormatError::Other("extraction staging path is unavailable".into()))?;
        if replace_existing {
            rename(staged, target)?;
        } else {
            match hard_link(staged, target) {
                Ok(()) => fs::remove_file(staged)?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err(error.into());
                }
                // FAT-family and some network filesystems cannot create hard
                // links. Keep them usable with the create-new reservation
                // fallback instead of turning every safe extraction into an
                // unsupported-operation failure.
                Err(_) => {
                    let reservation = TargetReservation::create(target)?;
                    rename(staged, target)?;
                    reservation.keep();
                }
            }
        }
        self.path.take();
        Ok(())
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        self.file.take();
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Stateful per-entry extraction writer enforcing the shared safety model.
///
/// Driving protocol, per entry:
/// - file entries: [`ExtractSink::file_target`] first (admission checks +
///   overwrite policy); when it returns a path, stream the entry's data with
///   [`ExtractSink::write_file`] — data is never opened for skipped entries;
/// - all other entries: [`ExtractSink::write_meta_entry`];
/// - at the end: [`ExtractSink::finish`].
///
/// `total` is the expected number of output bytes for progress reporting;
/// pass 0 when unknown (single-pass streams).
pub struct ExtractSink<'o> {
    dest: PathBuf,
    canonical_dest: PathBuf,
    opts: &'o ExtractOptions,
    accountant: LimitsAccountant,
    /// Relative paths of symlinks created during this run; later entries
    /// must not write through them.
    created_symlinks: HashSet<PathBuf>,
    /// Archive-relative regular files successfully materialized by this run.
    /// Single-pass link entries may only source content from this map, never
    /// from unrelated files that happened to exist in the destination.
    materialized_files: HashMap<Vec<u8>, PathBuf>,
    /// Conflict decision retained for each path returned by `file_target`
    /// until the staged output is committed.
    pending_outputs: HashMap<PathBuf, ResolvedOutput>,
    done: u64,
    total: u64,
    report: ExtractReport,
}

impl<'o> ExtractSink<'o> {
    /// Creates the destination directory and starts an accounting run.
    pub fn new(dest: &Path, opts: &'o ExtractOptions, total: u64) -> Result<Self, FormatError> {
        fs::create_dir_all(dest)?;
        let canonical_dest = dest.canonicalize()?;
        Ok(Self {
            dest: dest.to_path_buf(),
            canonical_dest,
            opts,
            accountant: LimitsAccountant::new(opts.limits),
            created_symlinks: HashSet::new(),
            materialized_files: HashMap::new(),
            pending_outputs: HashMap::new(),
            done: 0,
            total,
            report: ExtractReport {
                destination: dest.to_path_buf(),
                ..ExtractReport::default()
            },
        })
    }

    /// Common admission: checkpoint → sanitize → limits accounting →
    /// symlink-traversal guard. Returns the sanitized relative path.
    fn admit(&mut self, meta: &EntryMeta, ctl: &ControlToken) -> Result<PathBuf, FormatError> {
        ctl.checkpoint()?;
        let rel = sanitize_entry_path(&meta.path)?;
        #[cfg(windows)]
        for comp in rel.components() {
            if let std::path::Component::Normal(os) = comp {
                crate::safety::check_windows_portability(&os.to_string_lossy())?;
            }
        }
        self.accountant.check_entry(meta)?;
        if crosses_created_symlink(&rel, &self.created_symlinks) {
            return Err(FormatError::SymlinkBreakout(meta.path.display.clone()));
        }
        self.report.selected_entries = self.report.selected_entries.saturating_add(1);
        Ok(rel)
    }

    /// Admits a file entry and applies the overwrite policy. Returns the
    /// path to write to, or `None` when the entry is skipped (its size is
    /// then charged to the progress counter).
    pub fn file_target(
        &mut self,
        meta: &EntryMeta,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<Option<PathBuf>, FormatError> {
        let rel = self.admit(meta, ctl)?;
        let target = self.dest.join(&rel);
        progress.on_entry_progress(self.done, self.total, &meta.path, 0, meta.size);
        let Some(resolved) = resolve_conflict_path(&target, meta, self.opts)? else {
            self.done += meta.size;
            self.report.skipped = self.report.skipped.saturating_add(1);
            progress.on_entry_progress(self.done, self.total, &meta.path, meta.size, meta.size);
            return Ok(None);
        };
        ensure_parent_inside(&self.canonical_dest, &resolved.path)?;
        let out_path = resolved.path.clone();
        self.pending_outputs.insert(out_path.clone(), resolved);
        Ok(Some(out_path))
    }

    fn take_pending_output(&mut self, out_path: &Path) -> ResolvedOutput {
        self.pending_outputs
            .remove(out_path)
            .unwrap_or_else(|| ResolvedOutput {
                path: out_path.to_path_buf(),
                replace_existing: false,
                materialization: Materialization::Created,
            })
    }

    /// Abandons a path admitted by [`ExtractSink::file_target`] before its
    /// content writer is entered. This is used when opening the archive entry
    /// itself fails in best-effort mode.
    pub fn abandon_file_target(&mut self, out_path: &Path) {
        self.pending_outputs.remove(out_path);
    }

    fn record_materialization(&mut self, materialization: Materialization, output_bytes: u64) {
        match materialization {
            Materialization::Created => self.report.created = self.report.created.saturating_add(1),
            Materialization::Replaced => {
                self.report.replaced = self.report.replaced.saturating_add(1)
            }
            Materialization::Renamed => self.report.renamed = self.report.renamed.saturating_add(1),
        }
        self.report.output_bytes = self.report.output_bytes.saturating_add(output_bytes);
    }

    fn record_materialized_file(&mut self, meta: &EntryMeta, path: &Path) {
        if let Some(key) = crate::links::normalize_archive_path_raw(&meta.path.raw) {
            self.materialized_files.insert(key, path.to_path_buf());
        }
    }

    fn record_unmaterialized(
        &mut self,
        meta: &EntryMeta,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.admit(meta, ctl)?;
        self.report.skipped = self.report.skipped.saturating_add(1);
        Ok(())
    }

    /// Streams a file entry's data to the path obtained from
    /// [`ExtractSink::file_target`], charging the guardrails for every byte.
    pub fn write_file(
        &mut self,
        meta: &EntryMeta,
        out_path: &Path,
        data: &mut dyn Read,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let entry_start = self.done;
        let resolved = self.take_pending_output(out_path);
        let mut pending = PendingOutput::create(out_path)?;
        let mut buf = vec![0u8; self.opts.resources.stream_buffer_size(COPY_CHUNK)?];
        let mut written = 0u64;
        {
            let out = pending.file_mut()?;
            loop {
                ctl.checkpoint()?;
                let n = data.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                self.accountant.add_output_bytes(n as u64)?;
                out.write_all(&buf[..n])?;
                written = written.saturating_add(n as u64);
                self.done += n as u64;
                progress.on_entry_progress(
                    self.done,
                    self.total,
                    &meta.path,
                    self.done.saturating_sub(entry_start).min(meta.size),
                    meta.size,
                );
            }
        }
        ctl.checkpoint()?;
        restore_permissions(pending.path()?, meta, self.opts);
        pending.commit(out_path, resolved.replace_existing)?;
        self.record_materialized_file(meta, out_path);
        self.record_materialization(resolved.materialization, written);
        Ok(())
    }

    /// Variant used by best-effort extraction. Output creation/write errors
    /// still abort the job, but an entry stream read/integrity error removes
    /// the partial output, records the skipped entry, and lets later entries
    /// continue.
    pub fn write_file_best_effort(
        &mut self,
        meta: &EntryMeta,
        out_path: &Path,
        data: &mut dyn Read,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<bool, FormatError> {
        self.write_file_best_effort_classified(meta, out_path, data, progress, ctl, |error| error)
    }

    /// Best-effort writer with format-specific read-error classification.
    /// Password-aware formats use this to keep authentication errors fatal
    /// while still accepting recoverable corruption in independent entries.
    pub fn write_file_best_effort_classified(
        &mut self,
        meta: &EntryMeta,
        out_path: &Path,
        data: &mut dyn Read,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        classify_read_error: impl FnOnce(FormatError) -> FormatError,
    ) -> Result<bool, FormatError> {
        let entry_start = self.done;
        let resolved = self.take_pending_output(out_path);
        let mut pending = PendingOutput::create(out_path)?;
        let mut buf = vec![0u8; self.opts.resources.stream_buffer_size(COPY_CHUNK)?];
        let mut written = 0u64;
        let mut classify_read_error = Some(classify_read_error);
        {
            let out = pending.file_mut()?;
            loop {
                ctl.checkpoint()?;
                let n = match data.read(&mut buf) {
                    Ok(n) => n,
                    Err(e) => {
                        let classifier = classify_read_error.take().ok_or_else(|| {
                            FormatError::Other(
                                "entry read error classifier was already consumed".into(),
                            )
                        })?;
                        let error = classifier(FormatError::from(e));
                        if best_effort_recoverable(&error) {
                            self.record_problem(&meta.path, &error);
                            return Ok(false);
                        }
                        return Err(error);
                    }
                };
                if n == 0 {
                    break;
                }
                self.accountant.add_output_bytes(n as u64)?;
                out.write_all(&buf[..n])?;
                written = written.saturating_add(n as u64);
                self.done += n as u64;
                progress.on_entry_progress(
                    self.done,
                    self.total,
                    &meta.path,
                    self.done.saturating_sub(entry_start).min(meta.size),
                    meta.size,
                );
            }
        }
        ctl.checkpoint()?;
        restore_permissions(pending.path()?, meta, self.opts);
        pending.commit(out_path, resolved.replace_existing)?;
        self.record_materialized_file(meta, out_path);
        self.record_materialization(resolved.materialization, written);
        Ok(true)
    }

    fn write_hard_link(
        &mut self,
        meta: &EntryMeta,
        source: &Path,
        out_path: &Path,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let resolved = self.take_pending_output(out_path);
        let source = self.validated_materialized_source(source)?.ok_or_else(|| {
            FormatError::Other("materialized hardlink source is unavailable".into())
        })?;
        let pending = PendingOutput::hard_link(&source, out_path)?;
        ctl.checkpoint()?;
        pending.commit(out_path, resolved.replace_existing)?;
        self.record_materialized_file(meta, out_path);
        self.record_materialization(resolved.materialization, 0);
        Ok(())
    }

    fn validated_materialized_source(&self, source: &Path) -> Result<Option<PathBuf>, FormatError> {
        let metadata = match fs::symlink_metadata(source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            return Err(FormatError::SymlinkBreakout(
                source.to_string_lossy().into_owned(),
            ));
        }
        if !metadata.is_file() {
            return Ok(None);
        }
        let canonical = source.canonicalize()?;
        if !canonical.starts_with(&self.canonical_dest) {
            return Err(FormatError::SymlinkBreakout(
                source.to_string_lossy().into_owned(),
            ));
        }
        Ok(Some(canonical))
    }

    /// Writes a data-less entry (directory, symlink, hardlink, other).
    pub fn write_meta_entry(
        &mut self,
        meta: &EntryMeta,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let rel = self.admit(meta, ctl)?;
        let target = self.dest.join(&rel);
        progress.on_progress(self.done, self.total, &meta.path);
        match &meta.entry_type {
            EntryType::Dir => {
                ensure_directory_inside(&self.canonical_dest, &target)?;
                restore_permissions(&target, meta, self.opts);
                self.report.directories = self.report.directories.saturating_add(1);
            }
            EntryType::Symlink { target: link } => match self.opts.symlinks {
                SymlinkPolicy::Skip => {
                    self.report.skipped = self.report.skipped.saturating_add(1);
                }
                SymlinkPolicy::Follow => {
                    // Single-pass drivers (tar/7z) cannot re-read earlier
                    // entries, so Follow materializes the content from the
                    // already-extracted target on disk; unresolvable or
                    // not-yet-extracted targets are skipped. The two-pass
                    // engine ([`extract_entries`]) resolves through the
                    // archive instead and never reaches this branch.
                    self.link_from_disk(meta, &target, link, false, progress, ctl)?;
                }
                SymlinkPolicy::Preserve => {
                    match create_symlink_entry(
                        &self.canonical_dest,
                        &target,
                        meta,
                        link,
                        self.opts,
                    )? {
                        Some(materialization) => {
                            self.created_symlinks.insert(rel);
                            self.record_materialization(materialization, 0);
                        }
                        None => {
                            self.report.skipped = self.report.skipped.saturating_add(1);
                        }
                    }
                }
            },
            EntryType::Hardlink { target: link } => {
                self.link_from_disk(meta, &target, link, true, progress, ctl)?;
            }
            EntryType::Other | EntryType::File => {
                self.report.skipped = self.report.skipped.saturating_add(1);
            }
        }
        Ok(())
    }

    /// Materializes a link entry from its target file *as already extracted
    /// on disk*: hard links via `fs::hard_link`, followed symlinks as a
    /// content copy. Targets that resolve outside the archive, do not exist
    /// (yet) on disk, or are not regular files are skipped.
    fn link_from_disk(
        &mut self,
        meta: &EntryMeta,
        out_target: &Path,
        link: &[u8],
        hard: bool,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        // Symlink targets are relative to the link's directory, hardlink
        // targets name an entry by its full archive path.
        let resolved = if hard {
            crate::links::normalize_archive_path_raw(link)
        } else {
            crate::links::resolve_target_path_raw(&meta.path.raw, link)
        };
        let Some(target_key) = resolved else {
            self.report.skipped = self.report.skipped.saturating_add(1);
            return Ok(()); // absolute / escaping target: skip
        };
        let Some(src) = self.materialized_files.get(&target_key).cloned() else {
            self.report.skipped = self.report.skipped.saturating_add(1);
            return Ok(());
        };
        // Revalidate the recorded output before opening/linking it. This
        // rejects a source swapped to a symlink or routed outside the
        // destination after it was materialized.
        let Some(canonical_src) = self.validated_materialized_source(&src)? else {
            self.report.skipped = self.report.skipped.saturating_add(1);
            return Ok(());
        };
        let src_meta = fs::metadata(&canonical_src)?;
        let Some(resolved) = resolve_conflict_path(out_target, meta, self.opts)? else {
            self.report.skipped = self.report.skipped.saturating_add(1);
            return Ok(());
        };
        ensure_parent_inside(&self.canonical_dest, &resolved.path)?;
        let mut output_bytes = 0u64;
        if hard {
            let pending = PendingOutput::hard_link(&canonical_src, &resolved.path)?;
            ctl.checkpoint()?;
            pending.commit(&resolved.path, resolved.replace_existing)?;
        } else {
            let mut source = fs::File::open(&canonical_src)?;
            let mut pending = PendingOutput::create(&resolved.path)?;
            let mut buf = vec![0u8; self.opts.resources.stream_buffer_size(COPY_CHUNK)?];
            {
                let output = pending.file_mut()?;
                loop {
                    ctl.checkpoint()?;
                    let read = source.read(&mut buf)?;
                    if read == 0 {
                        break;
                    }
                    self.accountant.add_output_bytes(read as u64)?;
                    output.write_all(&buf[..read])?;
                    output_bytes = output_bytes.saturating_add(read as u64);
                    self.done = self.done.saturating_add(read as u64);
                    progress.on_entry_progress(
                        self.done,
                        self.total,
                        &meta.path,
                        output_bytes,
                        src_meta.len(),
                    );
                }
            }
            ctl.checkpoint()?;
            restore_permissions(pending.path()?, meta, self.opts);
            pending.commit(&resolved.path, resolved.replace_existing)?;
        }
        self.record_materialized_file(meta, &resolved.path);
        self.record_materialization(resolved.materialization, output_bytes);
        progress.on_progress(self.done, self.total, &meta.path);
        Ok(())
    }

    /// Final 100% progress report.
    pub fn finish(self, progress: &dyn ProgressSink) {
        self.finish_with_report(progress);
    }

    /// Finishes progress reporting and returns the completed outcome counts.
    pub fn finish_with_report(self, progress: &dyn ProgressSink) -> ExtractReport {
        let total = if self.total == 0 {
            self.done
        } else {
            self.total
        };
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        self.report
    }

    /// Records a selected entry that an optimized best-effort reader could
    /// not reach after a recoverable block failure. Admission still enforces
    /// cancellation, path safety, portability, and entry limits.
    pub fn record_best_effort_failure(
        &mut self,
        meta: &EntryMeta,
        error: &FormatError,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.admit(meta, ctl)?;
        self.record_problem(&meta.path, error);
        Ok(())
    }

    fn record_problem(&mut self, path: &EntryPath, error: &FormatError) {
        self.report.failed = self.report.failed.saturating_add(1);
        if let Some(reporter) = &self.opts.problem_reporter {
            reporter.skipped_entry(path, error);
        }
    }
}

fn best_effort_recoverable(error: &FormatError) -> bool {
    matches!(
        error,
        FormatError::Io(_) | FormatError::CorruptArchive(_) | FormatError::Other(_)
    )
}

fn parent_or_empty(path: &Path) -> &Path {
    let mut parent = Path::new("");
    if let Some(existing) = path.parent() {
        parent = existing;
    }
    parent
}

fn file_stem_or_empty(path: &Path) -> String {
    let mut stem = String::new();
    if let Some(existing) = path.file_stem() {
        stem = existing.to_string_lossy().into_owned();
    }
    stem
}

/// Extracts entries from `reader` into `dest` with the shared safety model.
/// This is the default body of [`ArchiveReader::extract`].
///
/// Flow: collect the *full* metadata list first (the borrow of `entries()`
/// must end before `read_entry()` can stream, and link targets may live
/// outside the selection), then feed each selected entry into an
/// [`ExtractSink`]. With [`SymlinkPolicy::Follow`], symlinks resolve through
/// the archive (chains, cycle detection) and the target's content is
/// extracted in their place; hardlinks link to an already-extracted target
/// or fall back to a content copy.
pub fn extract_entries<R: ArchiveReader + ?Sized>(
    reader: &mut R,
    dest: &Path,
    selection: Option<&[EntryPath]>,
    opts: &ExtractOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    extract_entries_with_report(reader, dest, selection, opts, progress, ctl).map(drop)
}

/// Report-returning variant of [`extract_entries`].
pub fn extract_entries_with_report<R: ArchiveReader + ?Sized>(
    reader: &mut R,
    dest: &Path,
    selection: Option<&[EntryPath]>,
    opts: &ExtractOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<ExtractReport, FormatError> {
    if selection.is_some_and(<[EntryPath]>::is_empty) {
        return Ok(empty_extract_report(dest, progress));
    }
    let all_metas: Vec<EntryMeta> = {
        let mut metas = Vec::new();
        for item in reader.entries() {
            metas.push(item?);
        }
        metas
    };
    let wanted: Option<HashSet<&[u8]>> =
        selection.map(|s| s.iter().map(|p| p.raw.as_slice()).collect());
    let selected = |m: &EntryMeta| {
        wanted
            .as_ref()
            .is_none_or(|w| w.contains(m.path.raw.as_slice()))
    };

    let resolver = LinkResolver::new(&all_metas);
    let total: u64 = all_metas
        .iter()
        .filter(|m| selected(m) && matches!(m.entry_type, EntryType::File))
        .map(|m| m.size)
        .sum();
    let mut sink = ExtractSink::new(dest, opts, total)?;
    // Out paths of files extracted so far, for hardlink reuse.
    let mut extracted: std::collections::HashMap<Vec<u8>, std::path::PathBuf> =
        std::collections::HashMap::new();

    for meta in all_metas.iter().filter(|m| selected(m)) {
        match &meta.entry_type {
            EntryType::File => {
                if let Some(out_path) = sink.file_target(meta, progress, ctl)? {
                    let mut data = match reader.read_entry(&meta.path) {
                        Ok(data) => data,
                        Err(e) if opts.best_effort && best_effort_recoverable(&e) => {
                            sink.abandon_file_target(&out_path);
                            sink.record_problem(&meta.path, &e);
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    let wrote = if opts.best_effort {
                        sink.write_file_best_effort(meta, &out_path, &mut *data, progress, ctl)?
                    } else {
                        sink.write_file(meta, &out_path, &mut *data, progress, ctl)?;
                        true
                    };
                    if wrote {
                        extracted.insert(meta.path.raw.clone(), out_path);
                    }
                }
            }
            EntryType::Symlink { .. } if opts.symlinks == SymlinkPolicy::Follow => {
                // Unresolvable targets (escaping, dangling, cycles) skip.
                if let Some(target) = resolver.resolve_to_file(meta) {
                    materialize_link(&mut sink, reader, meta, target, progress, ctl)?;
                } else {
                    sink.record_unmaterialized(meta, ctl)?;
                }
            }
            EntryType::Hardlink { .. } => {
                let Some(target) = resolver.resolve_to_file(meta) else {
                    sink.record_unmaterialized(meta, ctl)?;
                    continue;
                };
                match extracted.get(&target.path.raw) {
                    Some(src) => {
                        if let Some(out_path) = sink.file_target(meta, progress, ctl)? {
                            sink.write_hard_link(meta, src, &out_path, ctl)?;
                        }
                    }
                    // Target not extracted (e.g. excluded by selection):
                    // fall back to an independent content copy.
                    None => materialize_link(&mut sink, reader, meta, target, progress, ctl)?,
                }
            }
            _ => sink.write_meta_entry(meta, progress, ctl)?,
        }
    }
    Ok(sink.finish_with_report(progress))
}

/// Writes the content of `target` (a file entry) at the link entry's own
/// path — the materialized form of a followed symlink or of a hardlink
/// whose target is not on disk.
fn materialize_link<R: ArchiveReader + ?Sized>(
    sink: &mut ExtractSink<'_>,
    reader: &mut R,
    link: &EntryMeta,
    target: &EntryMeta,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    // The materialized entry carries the link's path but the target's
    // content and mode.
    let meta = EntryMeta {
        path: link.path.clone(),
        entry_type: EntryType::File,
        size: target.size,
        compressed_size: target.compressed_size,
        modified: link.modified.or(target.modified),
        unix_mode: target.unix_mode,
        crc32: target.crc32,
        encrypted: target.encrypted,
    };
    if let Some(out_path) = sink.file_target(&meta, progress, ctl)? {
        let mut data = match reader.read_entry(&target.path) {
            Ok(data) => data,
            Err(e) if sink.opts.best_effort && best_effort_recoverable(&e) => {
                sink.abandon_file_target(&out_path);
                sink.record_problem(&link.path, &e);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if sink.opts.best_effort {
            sink.write_file_best_effort(&meta, &out_path, &mut *data, progress, ctl)?;
        } else {
            sink.write_file(&meta, &out_path, &mut *data, progress, ctl)?;
        }
    }
    Ok(())
}

/// Resolves the configured conflict policy without changing the destination.
/// Every materialized output is staged without removing an existing target.
fn resolve_conflict_path(
    target: &Path,
    meta: &EntryMeta,
    opts: &ExtractOptions,
) -> Result<Option<ResolvedOutput>, FormatError> {
    // symlink_metadata also detects dangling symlinks at the target path.
    match fs::symlink_metadata(target) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(ResolvedOutput {
                path: target.to_path_buf(),
                replace_existing: opts.overwrite == OverwritePolicy::Overwrite,
                materialization: Materialization::Created,
            }));
        }
        Err(error) => return Err(error.into()),
    }
    match opts.overwrite {
        OverwritePolicy::Overwrite => Ok(Some(ResolvedOutput {
            path: target.to_path_buf(),
            replace_existing: true,
            materialization: Materialization::Replaced,
        })),
        OverwritePolicy::Skip => Ok(None),
        OverwritePolicy::RenameBoth => Ok(Some(ResolvedOutput {
            path: renamed_sibling(target),
            replace_existing: false,
            materialization: Materialization::Renamed,
        })),
        OverwritePolicy::Ask => match &opts.resolver {
            // No resolver wired (non-interactive context): degrade to Skip.
            None => Ok(None),
            Some(resolver) => match resolver.resolve(target, meta) {
                ConflictDecision::Overwrite => Ok(Some(ResolvedOutput {
                    path: target.to_path_buf(),
                    replace_existing: true,
                    materialization: Materialization::Replaced,
                })),
                ConflictDecision::Skip => Ok(None),
                ConflictDecision::Rename(name) => {
                    let parent = parent_or_empty(target);
                    Ok(Some(ResolvedOutput {
                        path: parent.join(name),
                        replace_existing: false,
                        materialization: Materialization::Renamed,
                    }))
                }
                ConflictDecision::Abort => Err(FormatError::Cancelled),
            },
        },
    }
}

/// Picks the first free `name (n).ext` sibling for [`OverwritePolicy::RenameBoth`].
fn renamed_sibling(target: &Path) -> PathBuf {
    let parent = parent_or_empty(target);
    let stem = file_stem_or_empty(target);
    let ext = target.extension().map(|e| e.to_string_lossy().into_owned());
    let mut n = 1u64;
    loop {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        n += 1;
    }
}

/// Symlink-breakout guard: validates the deepest pre-existing ancestor before
/// creating any missing parent, then creates and revalidates one directory at
/// a time. This catches writes routed through symlinks already present at the
/// destination without first mutating their targets (the in-run set guards
/// links created by the current extraction).
fn ensure_parent_inside(canonical_dest: &Path, path: &Path) -> Result<(), FormatError> {
    let parent = parent_or_empty(path);
    let mut existing = parent.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(FormatError::Other(
                        "extraction target has no existing ancestor".into(),
                    ));
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(FormatError::Other(
                        "extraction target has no existing ancestor".into(),
                    ));
                };
                existing = parent.to_path_buf();
            }
            Err(error) => return Err(error.into()),
        }
    }
    ensure_existing_path_inside(canonical_dest, &existing, path)?;

    for component in missing.into_iter().rev() {
        existing.push(component);
        match fs::create_dir(&existing) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        ensure_existing_path_inside(canonical_dest, &existing, path)?;
    }
    Ok(())
}

fn ensure_directory_inside(canonical_dest: &Path, path: &Path) -> Result<(), FormatError> {
    ensure_parent_inside(canonical_dest, path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_existing_directory(canonical_dest, path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => ensure_existing_path_inside(canonical_dest, path, path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path)?;
                validate_existing_directory(canonical_dest, path, &metadata)
            }
            Err(error) => Err(error.into()),
        },
        Err(error) => Err(error.into()),
    }
}

fn validate_existing_directory(
    canonical_dest: &Path,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FormatError> {
    if metadata.file_type().is_symlink() {
        return Err(FormatError::SymlinkBreakout(
            path.to_string_lossy().into_owned(),
        ));
    }
    if !metadata.is_dir() {
        // Preserve the previous, actionable filesystem error for an entry
        // whose directory target is occupied by a non-directory node.
        return fs::create_dir(path).map_err(FormatError::from);
    }
    ensure_existing_path_inside(canonical_dest, path, path)
}

fn ensure_existing_path_inside(
    canonical_dest: &Path,
    existing: &Path,
    reported_path: &Path,
) -> Result<(), FormatError> {
    let canonical = existing.canonicalize()?;
    if canonical.starts_with(canonical_dest) {
        return Ok(());
    }
    Err(FormatError::SymlinkBreakout(
        reported_path.to_string_lossy().into_owned(),
    ))
}

/// Creates a symlink entry under the Preserve policy. The link is staged next
/// to its final path so a failed creation or commit cannot remove an existing
/// destination entry.
fn create_symlink_entry(
    canonical_dest: &Path,
    target: &Path,
    meta: &EntryMeta,
    link: &[u8],
    opts: &ExtractOptions,
) -> Result<Option<Materialization>, FormatError> {
    let Some(resolved) = resolve_conflict_path(target, meta, opts)? else {
        return Ok(None);
    };
    ensure_parent_inside(canonical_dest, &resolved.path)?;
    let link_target = PathBuf::from(String::from_utf8_lossy(link).into_owned());
    #[cfg(windows)]
    let resolved_target = parent_or_empty(&resolved.path).join(&link_target);
    #[cfg(windows)]
    let target_is_dir = match fs::metadata(&resolved_target) {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => {
            let text = link_target.to_string_lossy();
            text.ends_with('/') || text.ends_with('\\')
        }
    };
    #[cfg(not(windows))]
    let target_is_dir = false;
    let Some(pending) = PendingOutput::symlink(&link_target, &resolved.path, target_is_dir)? else {
        return Ok(None);
    };
    pending.commit(&resolved.path, resolved.replace_existing)?;
    Ok(Some(resolved.materialization))
}

#[cfg(windows)]
fn is_windows_symlink_privilege_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314)
}

/// Restores Unix permission bits (masked to 0o7777) when requested.
#[cfg(unix)]
fn restore_permissions(path: &Path, meta: &EntryMeta, opts: &ExtractOptions) {
    use std::os::unix::fs::PermissionsExt;
    if !opts.restore_permissions {
        return;
    }
    if let Some(mode) = meta.unix_mode {
        // Best effort: permission failures must not abort extraction.
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777));
    }
}

#[cfg(not(unix))]
fn restore_permissions(_path: &Path, _meta: &EntryMeta, _opts: &ExtractOptions) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::ConflictResolver;
    use crate::{NoProgress, TestReport};
    use std::io::Cursor;
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "squallz-format-api-extract-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_meta(path: &str) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::File,
            size: 3,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn dangling_symlink_meta(path: &str) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::Symlink {
                target: b"missing-target".to_vec(),
            },
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn hardlink_meta(path: &str, target: &str) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::Hardlink {
                target: target.as_bytes().to_vec(),
            },
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[cfg(unix)]
    fn directory_meta(path: &str) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::Dir,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn extract_temp_paths(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".squallz-extract-"))
            })
            .collect()
    }

    struct FailsAfterFirstRead {
        delivered: bool,
        kind: std::io::ErrorKind,
    }

    impl Read for FailsAfterFirstRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.delivered {
                return Err(std::io::Error::new(self.kind, "damaged entry"));
            }
            self.delivered = true;
            buf[..3].copy_from_slice(b"new");
            Ok(3)
        }
    }

    struct UnusedReader;

    impl ArchiveReader for UnusedReader {
        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            panic!("an explicitly empty selection must not list the archive")
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            panic!("an explicitly empty selection must not read an entry")
        }

        fn test(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestReport, FormatError> {
            panic!("an explicitly empty selection must not test the archive")
        }
    }

    struct MetadataReader {
        entries: Vec<EntryMeta>,
    }

    impl ArchiveReader for MetadataReader {
        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            Box::new(self.entries.clone().into_iter().map(Ok))
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            panic!("dangling link tests must not read file content")
        }

        fn test(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestReport, FormatError> {
            Ok(TestReport::default())
        }
    }

    #[test]
    fn explicit_empty_selection_returns_an_empty_report_without_creating_destination() {
        let root = temp_dir("empty-selection");
        let destination = root.join("not-created");
        let selection = Vec::new();

        let report = extract_entries_with_report(
            &mut UnusedReader,
            &destination,
            Some(&selection),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(
            report,
            ExtractReport {
                destination: destination.clone(),
                ..ExtractReport::default()
            }
        );
        assert!(!destination.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unmaterialized_followed_link_still_validates_its_archive_path() {
        let root = temp_dir("unmaterialized-link-path");
        let destination = root.join("output");
        let mut reader = MetadataReader {
            entries: vec![dangling_symlink_meta("../escape")],
        };
        let opts = ExtractOptions {
            symlinks: SymlinkPolicy::Follow,
            ..ExtractOptions::default()
        };

        let error = extract_entries_with_report(
            &mut reader,
            &destination,
            None,
            &opts,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::PathTraversal(_)), "{error:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unmaterialized_followed_links_still_obey_entry_limits() {
        let root = temp_dir("unmaterialized-link-limit");
        let destination = root.join("output");
        let mut reader = MetadataReader {
            entries: vec![
                dangling_symlink_meta("first"),
                dangling_symlink_meta("second"),
            ],
        };
        let opts = ExtractOptions {
            symlinks: SymlinkPolicy::Follow,
            limits: crate::SafetyLimits {
                max_entries: 1,
                ..crate::SafetyLimits::default()
            },
            ..ExtractOptions::default()
        };

        let error = extract_entries_with_report(
            &mut reader,
            &destination,
            None,
            &opts,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(
            matches!(error, FormatError::ResourceLimitExceeded(_)),
            "{error:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unmaterialized_followed_link_still_obeys_cancellation() {
        let root = temp_dir("unmaterialized-link-cancel");
        let destination = root.join("output");
        let mut reader = MetadataReader {
            entries: vec![dangling_symlink_meta("link")],
        };
        let opts = ExtractOptions {
            symlinks: SymlinkPolicy::Follow,
            ..ExtractOptions::default()
        };
        let ctl = ControlToken::new();
        ctl.cancel();

        let error =
            extract_entries_with_report(&mut reader, &destination, None, &opts, &NoProgress, &ctl)
                .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled), "{error:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_pass_hardlink_does_not_source_a_preexisting_destination_file() {
        let root = temp_dir("hardlink-preexisting-source");
        fs::write(root.join("preexisting.txt"), b"private").unwrap();
        let opts = ExtractOptions::default();
        let mut sink = ExtractSink::new(&root, &opts, 0).unwrap();
        let meta = hardlink_meta("copied.txt", "preexisting.txt");

        sink.write_meta_entry(&meta, &NoProgress, &ControlToken::default())
            .unwrap();

        assert!(!root.join("copied.txt").exists());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.created + report.replaced + report.renamed, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_pass_follow_does_not_source_a_preexisting_destination_file() {
        let root = temp_dir("follow-preexisting-source");
        fs::write(root.join("preexisting.txt"), b"private").unwrap();
        let opts = ExtractOptions {
            symlinks: SymlinkPolicy::Follow,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&root, &opts, 0).unwrap();
        let mut meta = dangling_symlink_meta("copied.txt");
        meta.entry_type = EntryType::Symlink {
            target: b"preexisting.txt".to_vec(),
        };

        sink.write_meta_entry(&meta, &NoProgress, &ControlToken::default())
            .unwrap();

        assert!(!root.join("copied.txt").exists());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.output_bytes, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn single_pass_link_does_not_follow_a_preexisting_symlink_ancestor() {
        let root = temp_dir("link-preexisting-symlink-source");
        let destination = root.join("destination");
        let outside = root.join("outside");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"private").unwrap();
        std::os::unix::fs::symlink(&outside, destination.join("escape")).unwrap();
        let opts = ExtractOptions::default();
        let mut sink = ExtractSink::new(&destination, &opts, 0).unwrap();
        let meta = hardlink_meta("copied.txt", "escape/secret.txt");

        sink.write_meta_entry(&meta, &NoProgress, &ControlToken::default())
            .unwrap();

        assert!(!destination.join("copied.txt").exists());
        assert_eq!(fs::read(outside.join("secret.txt")).unwrap(), b"private");
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.skipped, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_pass_links_use_the_actual_renamed_output_from_this_run() {
        let root = temp_dir("link-renamed-provenance");
        fs::write(root.join("source.txt"), b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::RenameBoth,
            symlinks: SymlinkPolicy::Follow,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&root, &opts, 3).unwrap();
        let source_meta = file_meta("source.txt");
        let source_output = sink
            .file_target(&source_meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();
        sink.write_file(
            &source_meta,
            &source_output,
            &mut Cursor::new(b"new"),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        assert_eq!(source_output, root.join("source (1).txt"));

        let hardlink = hardlink_meta("hard.txt", "source.txt");
        sink.write_meta_entry(&hardlink, &NoProgress, &ControlToken::default())
            .unwrap();
        let mut followed = dangling_symlink_meta("followed.txt");
        followed.entry_type = EntryType::Symlink {
            target: b"source.txt".to_vec(),
        };
        sink.write_meta_entry(&followed, &NoProgress, &ControlToken::default())
            .unwrap();

        assert_eq!(fs::read(root.join("source.txt")).unwrap(), b"old");
        assert_eq!(fs::read(root.join("hard.txt")).unwrap(), b"new");
        assert_eq!(fs::read(root.join("followed.txt")).unwrap(), b"new");
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 3);
        assert_eq!(report.renamed, 1);
        assert_eq!(report.created, 2);
        assert_eq!(report.output_bytes, 6);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn skipped_file_is_not_available_as_single_pass_link_provenance() {
        let root = temp_dir("link-skipped-provenance");
        fs::write(root.join("source.txt"), b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Skip,
            symlinks: SymlinkPolicy::Follow,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&root, &opts, 3).unwrap();
        let source_meta = file_meta("source.txt");

        assert!(sink
            .file_target(&source_meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .is_none());
        let hardlink = hardlink_meta("hard.txt", "source.txt");
        sink.write_meta_entry(&hardlink, &NoProgress, &ControlToken::default())
            .unwrap();

        assert!(!root.join("hard.txt").exists());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 2);
        assert_eq!(report.skipped, 2);
        assert_eq!(report.created + report.replaced + report.renamed, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn single_pass_hardlink_provenance_keeps_distinct_raw_entry_names() {
        let root = temp_dir("link-raw-provenance");
        let opts = ExtractOptions::default();
        let mut sink = ExtractSink::new(&root, &opts, 6).unwrap();
        let mut first = file_meta("first.txt");
        first.path = EntryPath::from_raw(vec![0x80], "first.txt".into(), "legacy");
        let mut second = file_meta("second.txt");
        second.path = EntryPath::from_raw(vec![0x81], "second.txt".into(), "legacy");
        for (meta, content) in [(&first, b"one".as_slice()), (&second, b"two".as_slice())] {
            let output = sink
                .file_target(meta, &NoProgress, &ControlToken::default())
                .unwrap()
                .unwrap();
            sink.write_file(
                meta,
                &output,
                &mut Cursor::new(content),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        }
        let mut hardlink = hardlink_meta("hard.txt", "unused");
        hardlink.entry_type = EntryType::Hardlink { target: vec![0x80] };

        sink.write_meta_entry(&hardlink, &NoProgress, &ControlToken::default())
            .unwrap();

        assert_eq!(fs::read(root.join("hard.txt")).unwrap(), b"one");
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn hardlink_source_rebinding_to_a_symlink_is_rejected() {
        let root = temp_dir("hardlink-source-rebinding");
        let outside = root.join("outside.txt");
        fs::write(&outside, b"outside").unwrap();
        let opts = ExtractOptions::default();
        let mut sink = ExtractSink::new(&root, &opts, 3).unwrap();
        let source_meta = file_meta("source.txt");
        let source = sink
            .file_target(&source_meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();
        sink.write_file(
            &source_meta,
            &source,
            &mut Cursor::new(b"new"),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(&outside, &source).unwrap();
        let hardlink = hardlink_meta("hard.txt", "source.txt");
        let output = sink
            .file_target(&hardlink, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        let error = sink
            .write_hard_link(&hardlink, &source, &output, &ControlToken::default())
            .unwrap_err();

        assert!(
            matches!(error, FormatError::SymlinkBreakout(_)),
            "{error:?}"
        );
        assert!(!root.join("hard.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn renamed_sibling_preserves_parent_stem_and_extension() {
        let dir = temp_dir("rename-both");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        fs::write(dir.join("note (1).txt"), b"older").unwrap();

        assert_eq!(renamed_sibling(&target), dir.join("note (2).txt"));
        assert_eq!(parent_or_empty(Path::new("note.txt")), Path::new(""));
        assert_eq!(file_stem_or_empty(Path::new("/")), "");

        fs::remove_dir_all(&dir).unwrap();
    }

    struct RenameResolver;

    impl ConflictResolver for RenameResolver {
        fn resolve(&self, _existing: &Path, _incoming: &EntryMeta) -> ConflictDecision {
            ConflictDecision::Rename("manual.txt".to_owned())
        }
    }

    #[test]
    fn ask_rename_stays_under_target_parent() {
        let dir = temp_dir("ask-rename");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Ask,
            resolver: Some(Arc::new(RenameResolver)),
            ..ExtractOptions::default()
        };

        let out = resolve_conflict_path(&target, &file_meta("note.txt"), &opts)
            .unwrap()
            .unwrap();
        assert_eq!(out.path, dir.join("manual.txt"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overwrite_conflict_resolution_accepts_an_absent_metadata_target() {
        let dir = temp_dir("overwrite-absent-metadata");
        let target = dir.join("note.txt");
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };

        let out = resolve_conflict_path(&target, &file_meta("note.txt"), &opts)
            .unwrap()
            .unwrap();

        assert_eq!(out.path, target);
        assert!(!out.path.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn successful_file_write_replaces_existing_target_after_commit() {
        let dir = temp_dir("atomic-success");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"old");
        sink.write_file(
            &meta,
            &out,
            &mut Cursor::new(b"new"),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(extract_temp_paths(&dir).is_empty());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.destination, dir);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.replaced, 1);
        assert_eq!(report.output_bytes, 3);
        assert_eq!(report.created + report.renamed + report.failed, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_file_read_preserves_existing_target_and_removes_staging_file() {
        let dir = temp_dir("atomic-read-failure");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();
        let error = sink
            .write_file(
                &meta,
                &out,
                &mut FailsAfterFirstRead {
                    delivered: false,
                    kind: std::io::ErrorKind::InvalidData,
                },
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::Io(_)), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_file_write_keeps_destination_unchanged() {
        let dir = temp_dir("atomic-cancel");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let ctl = ControlToken::new();
        let out = sink.file_target(&meta, &NoProgress, &ctl).unwrap().unwrap();
        ctl.cancel();
        let error = sink
            .write_file(&meta, &out, &mut Cursor::new(b"new"), &NoProgress, &ctl)
            .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_limit_failure_preserves_existing_target() {
        let dir = temp_dir("atomic-output-limit");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            limits: crate::SafetyLimits {
                max_output_bytes: 1,
                ..crate::SafetyLimits::default()
            },
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();
        let error = sink
            .write_file(
                &meta,
                &out,
                &mut Cursor::new(b"new"),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(
            matches!(error, FormatError::ResourceLimitExceeded(_)),
            "{error:?}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_commit_preserves_a_target_created_after_conflict_resolution() {
        let dir = temp_dir("atomic-no-replace-race");
        let target = dir.join("note.txt");
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Skip,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        fs::write(&target, b"racer").unwrap();
        let error = sink
            .write_file(
                &meta,
                &out,
                &mut Cursor::new(b"new"),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();

        match error {
            FormatError::Io(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected an existing-target I/O error, got {other:?}"),
        }
        assert_eq!(fs::read(&target).unwrap(), b"racer");
        assert!(extract_temp_paths(&dir).is_empty());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.created, 0);
        assert_eq!(report.replaced, 0);
        assert_eq!(report.renamed, 0);
        assert_eq!(report.output_bytes, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn best_effort_storage_exhaustion_remains_fatal() {
        let dir = temp_dir("best-effort-disk-full");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            best_effort: true,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        let error = sink
            .write_file_best_effort(
                &meta,
                &out,
                &mut FailsAfterFirstRead {
                    delivered: false,
                    kind: std::io::ErrorKind::StorageFull,
                },
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::DiskFull));
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn best_effort_read_failure_counts_failed_without_materialization() {
        let dir = temp_dir("best-effort-report");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            best_effort: true,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        let wrote = sink
            .write_file_best_effort(
                &meta,
                &out,
                &mut FailsAfterFirstRead {
                    delivered: false,
                    kind: std::io::ErrorKind::InvalidData,
                },
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();

        assert!(!wrote);
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.created + report.replaced + report.renamed, 0);
        assert_eq!(report.output_bytes, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn best_effort_open_failure_abandons_pending_target_without_counting_skip() {
        let dir = temp_dir("best-effort-open-report");
        let opts = ExtractOptions {
            best_effort: true,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();
        assert_eq!(sink.pending_outputs.len(), 1);

        sink.abandon_file_target(&out);
        sink.record_problem(
            &meta.path,
            &FormatError::CorruptArchive("damaged entry".into()),
        );

        assert!(sink.pending_outputs.is_empty());
        let report = sink.finish_with_report(&NoProgress);
        assert_eq!(report.selected_entries, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn directory_entry_rejects_preexisting_symlink_ancestor_without_writing_outside() {
        let root = temp_dir("directory-symlink-breakout");
        let destination = root.join("destination");
        let outside = root.join("outside");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, destination.join("escape")).unwrap();
        let opts = ExtractOptions::default();
        let mut sink = ExtractSink::new(&destination, &opts, 0).unwrap();
        let meta = directory_meta("escape/new/leaf");

        let error = sink
            .write_meta_entry(&meta, &NoProgress, &ControlToken::default())
            .unwrap_err();

        assert!(
            matches!(error, FormatError::SymlinkBreakout(_)),
            "{error:?}"
        );
        assert!(!outside.join("new").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn overwrite_commit_replaces_a_target_created_after_conflict_resolution() {
        let dir = temp_dir("atomic-overwrite-race");
        let target = dir.join("note.txt");
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        fs::write(&target, b"racer").unwrap();
        sink.write_file(
            &meta,
            &out,
            &mut Cursor::new(b"new"),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_link_commit_replaces_existing_target() {
        let dir = temp_dir("atomic-hard-link-success");
        let source = dir.join("source.txt");
        let target = dir.join("note.txt");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let out = sink
            .file_target(&meta, &NoProgress, &ControlToken::default())
            .unwrap()
            .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"old");
        sink.write_hard_link(&meta, &source, &out, &ControlToken::default())
            .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        fs::write(&source, b"changed").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"changed");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_link_commit_failure_preserves_existing_target() {
        let dir = temp_dir("atomic-hard-link-commit-failure");
        let source = dir.join("source.txt");
        let target = dir.join("note.txt");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let pending = PendingOutput::hard_link(&source, &target).unwrap();

        let error = pending
            .commit_using(
                &target,
                true,
                |_, _| Ok(()),
                |_, _| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected commit failure",
                    ))
                },
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::Io(_)), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_commit_failure_preserves_existing_target() {
        let dir = temp_dir("atomic-symlink-commit-failure");
        let target = dir.join("note.txt");
        fs::write(&target, b"old").unwrap();
        let pending = PendingOutput::symlink(Path::new("new-target"), &target, false)
            .unwrap()
            .unwrap();

        let error = pending
            .commit_using(
                &target,
                true,
                |_, _| Ok(()),
                |_, _| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected commit failure",
                    ))
                },
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::Io(_)), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_commit_uses_a_reservation_when_hard_links_are_unavailable() {
        let dir = temp_dir("atomic-reservation-fallback");
        let target = dir.join("note.txt");
        let mut pending = PendingOutput::create(&target).unwrap();
        pending.file_mut().unwrap().write_all(b"new").unwrap();

        pending
            .commit_using(
                &target,
                false,
                |_, _| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "hard links unavailable",
                    ))
                },
                |source, destination| fs::rename(source, destination),
            )
            .unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_hard_link_commit_preserves_existing_target() {
        let dir = temp_dir("atomic-hard-link-cancel");
        let source = dir.join("source.txt");
        let target = dir.join("note.txt");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();
        let opts = ExtractOptions {
            overwrite: OverwritePolicy::Overwrite,
            ..ExtractOptions::default()
        };
        let mut sink = ExtractSink::new(&dir, &opts, 3).unwrap();
        let meta = file_meta("note.txt");
        let ctl = ControlToken::new();
        let out = sink.file_target(&meta, &NoProgress, &ctl).unwrap().unwrap();
        ctl.cancel();

        let error = sink
            .write_hard_link(&meta, &source, &out, &ctl)
            .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(extract_temp_paths(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
