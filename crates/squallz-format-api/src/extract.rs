//! Shared safe extraction engine.
//!
//! Two layers:
//! - [`ExtractSink`]: per-entry writing with the full safety model of
//!   PLAN.md §2.3 (Zip-Slip rejection, decompression-bomb guardrails,
//!   symlink-breakout protection, overwrite/symlink policies, permission
//!   restore, byte-accurate progress). Formats with their own iteration
//!   order (single-pass tar streams, solid 7z blocks) drive it directly.
//! - [`extract_entries`]: drives any [`ArchiveReader`] through its
//!   `entries()` + `read_entry()` primitives into an [`ExtractSink`]. This
//!   is the default body of [`ArchiveReader::extract`].

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::entry::{EntryMeta, EntryPath, EntryType};
use crate::error::FormatError;
use crate::links::{resolve_target_path, LinkResolver};
use crate::options::{ConflictDecision, ExtractOptions, OverwritePolicy, SymlinkPolicy};
use crate::progress::{ControlToken, ProgressSink};
use crate::safety::{crosses_created_symlink, sanitize_entry_path, LimitsAccountant};
use crate::traits::ArchiveReader;

/// Copy chunk size; cancellation, limits and progress are checked at this
/// granularity.
const COPY_CHUNK: usize = 64 * 1024;

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
    done: u64,
    total: u64,
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
            done: 0,
            total,
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
        let Some(out_path) = resolve_conflict(&target, meta, self.opts)? else {
            self.done += meta.size;
            progress.on_entry_progress(self.done, self.total, &meta.path, meta.size, meta.size);
            return Ok(None);
        };
        ensure_parent_inside(&self.canonical_dest, &out_path)?;
        Ok(Some(out_path))
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
        let mut out = fs::File::create(out_path)?;
        let mut buf = vec![0u8; self.opts.resources.stream_buffer_size(COPY_CHUNK)?];
        loop {
            ctl.checkpoint()?;
            let n = data.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.accountant.add_output_bytes(n as u64)?;
            out.write_all(&buf[..n])?;
            self.done += n as u64;
            progress.on_entry_progress(
                self.done,
                self.total,
                &meta.path,
                self.done.saturating_sub(entry_start).min(meta.size),
                meta.size,
            );
        }
        drop(out);
        restore_permissions(out_path, meta, self.opts);
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
        let entry_start = self.done;
        let mut out = fs::File::create(out_path)?;
        let mut buf = vec![0u8; self.opts.resources.stream_buffer_size(COPY_CHUNK)?];
        loop {
            ctl.checkpoint()?;
            let n = match data.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    drop(out);
                    let _ = fs::remove_file(out_path);
                    let err = FormatError::Io(e);
                    self.report_problem(&meta.path, &err);
                    return Ok(false);
                }
            };
            if n == 0 {
                break;
            }
            self.accountant.add_output_bytes(n as u64)?;
            out.write_all(&buf[..n])?;
            self.done += n as u64;
            progress.on_entry_progress(
                self.done,
                self.total,
                &meta.path,
                self.done.saturating_sub(entry_start).min(meta.size),
                meta.size,
            );
        }
        drop(out);
        restore_permissions(out_path, meta, self.opts);
        Ok(true)
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
                fs::create_dir_all(&target)?;
                restore_permissions(&target, meta, self.opts);
            }
            EntryType::Symlink { target: link } => match self.opts.symlinks {
                SymlinkPolicy::Skip => {}
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
                    if create_symlink_entry(&self.canonical_dest, &target, meta, link, self.opts)? {
                        self.created_symlinks.insert(rel);
                    }
                }
            },
            EntryType::Hardlink { target: link } => {
                self.link_from_disk(meta, &target, link, true, progress, ctl)?;
            }
            EntryType::Other | EntryType::File => {}
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
        _ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        // Symlink targets are relative to the link's directory, hardlink
        // targets name an entry by its full archive path.
        let resolved = if hard {
            crate::links::normalize_archive_path(link)
        } else {
            resolve_target_path(&meta.path.display, link)
        };
        let Some(target_rel) = resolved else {
            return Ok(()); // absolute / escaping target: skip
        };
        let src = self.dest.join(&target_rel);
        // Only regular files already produced by this extraction qualify;
        // never follow through an on-disk symlink.
        let Ok(src_meta) = fs::symlink_metadata(&src) else {
            return Ok(());
        };
        if !src_meta.is_file() {
            return Ok(());
        }
        let Some(out_path) = resolve_conflict(out_target, meta, self.opts)? else {
            return Ok(());
        };
        ensure_parent_inside(&self.canonical_dest, &out_path)?;
        if hard {
            fs::hard_link(&src, &out_path)?;
        } else {
            let copied = fs::copy(&src, &out_path)?;
            self.accountant.add_output_bytes(copied)?;
            self.done += copied;
            restore_permissions(&out_path, meta, self.opts);
        }
        progress.on_progress(self.done, self.total, &meta.path);
        Ok(())
    }

    /// Final 100% progress report.
    pub fn finish(self, progress: &dyn ProgressSink) {
        let total = if self.total == 0 {
            self.done
        } else {
            self.total
        };
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
    }

    fn report_problem(&self, path: &EntryPath, error: &FormatError) {
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
                            sink.report_problem(&meta.path, &e);
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
                }
            }
            EntryType::Hardlink { .. } => {
                let Some(target) = resolver.resolve_to_file(meta) else {
                    continue;
                };
                match extracted.get(&target.path.raw) {
                    Some(src) => {
                        if let Some(out_path) = sink.file_target(meta, progress, ctl)? {
                            fs::hard_link(src, &out_path)?;
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
    sink.finish(progress);
    Ok(())
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
                sink.report_problem(&link.path, &e);
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

/// Applies the overwrite policy for a file entry. Returns the path to write
/// to, or `None` to skip the entry.
fn resolve_conflict(
    target: &Path,
    meta: &EntryMeta,
    opts: &ExtractOptions,
) -> Result<Option<PathBuf>, FormatError> {
    // symlink_metadata also detects dangling symlinks at the target path.
    if fs::symlink_metadata(target).is_err() {
        return Ok(Some(target.to_path_buf()));
    }
    match opts.overwrite {
        OverwritePolicy::Overwrite => {
            remove_existing(target)?;
            Ok(Some(target.to_path_buf()))
        }
        OverwritePolicy::Skip => Ok(None),
        OverwritePolicy::RenameBoth => Ok(Some(renamed_sibling(target))),
        OverwritePolicy::Ask => match &opts.resolver {
            // No resolver wired (non-interactive context): degrade to Skip.
            None => Ok(None),
            Some(resolver) => match resolver.resolve(target, meta) {
                ConflictDecision::Overwrite => {
                    remove_existing(target)?;
                    Ok(Some(target.to_path_buf()))
                }
                ConflictDecision::Skip => Ok(None),
                ConflictDecision::Rename(name) => {
                    let parent = parent_or_empty(target);
                    Ok(Some(parent.join(name)))
                }
                ConflictDecision::Abort => Err(FormatError::Cancelled),
            },
        },
    }
}

/// Removes an existing file or symlink so it can be overwritten. Existing
/// directories are left alone (a file entry never silently destroys a
/// directory tree); the subsequent create then fails with an I/O error.
fn remove_existing(path: &Path) -> Result<(), FormatError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        fs::remove_file(path)?;
    }
    Ok(())
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

/// Symlink-breakout guard: creates the parent directories of `path`, then
/// canonicalizes the deepest pre-existing ancestor and requires it to stay
/// inside `canonical_dest`. This catches writes routed through symlinks that
/// already existed at the destination (the in-run set guards links created
/// by the current extraction).
fn ensure_parent_inside(canonical_dest: &Path, path: &Path) -> Result<(), FormatError> {
    let parent = parent_or_empty(path);
    fs::create_dir_all(parent)?;
    let canonical_parent = parent.canonicalize()?;
    if !canonical_parent.starts_with(canonical_dest) {
        return Err(FormatError::SymlinkBreakout(
            path.to_string_lossy().into_owned(),
        ));
    }
    Ok(())
}

/// Creates a symlink entry under the Preserve policy. Returns `true` when a
/// link was actually created.
#[cfg(unix)]
fn create_symlink_entry(
    canonical_dest: &Path,
    target: &Path,
    meta: &EntryMeta,
    link: &[u8],
    opts: &ExtractOptions,
) -> Result<bool, FormatError> {
    let Some(out_path) = resolve_conflict(target, meta, opts)? else {
        return Ok(false);
    };
    ensure_parent_inside(canonical_dest, &out_path)?;
    let link_target = PathBuf::from(String::from_utf8_lossy(link).into_owned());
    std::os::unix::fs::symlink(&link_target, &out_path)?;
    Ok(true)
}

#[cfg(windows)]
fn create_symlink_entry(
    canonical_dest: &Path,
    target: &Path,
    meta: &EntryMeta,
    link: &[u8],
    opts: &ExtractOptions,
) -> Result<bool, FormatError> {
    let Some(out_path) = resolve_conflict(target, meta, opts)? else {
        return Ok(false);
    };
    ensure_parent_inside(canonical_dest, &out_path)?;
    let link_target = PathBuf::from(String::from_utf8_lossy(link).into_owned());
    let resolved_target = parent_or_empty(&out_path).join(&link_target);
    let target_is_dir = match fs::metadata(&resolved_target) {
        Ok(metadata) => metadata.is_dir(),
        Err(_) => {
            let text = link_target.to_string_lossy();
            text.ends_with('/') || text.ends_with('\\')
        }
    };
    let result = if target_is_dir {
        std::os::windows::fs::symlink_dir(&link_target, &out_path)
    } else {
        std::os::windows::fs::symlink_file(&link_target, &out_path)
    };
    match result {
        Ok(()) => Ok(true),
        Err(error) if is_windows_symlink_privilege_error(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn is_windows_symlink_privilege_error(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314)
}

/// Symlink restore remains unavailable on non-Unix, non-Windows targets.
#[cfg(not(any(unix, windows)))]
fn create_symlink_entry(
    _canonical_dest: &Path,
    _target: &Path,
    _meta: &EntryMeta,
    _link: &[u8],
    _opts: &ExtractOptions,
) -> Result<bool, FormatError> {
    Ok(false)
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
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
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

        let out = resolve_conflict(&target, &file_meta("note.txt"), &opts)
            .unwrap()
            .unwrap();
        assert_eq!(out, dir.join("manual.txt"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
