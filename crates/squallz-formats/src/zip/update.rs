//! ZIP update (append/delete/rename) per the PLAN.md §2.1 contract:
//! rewrite into a temporary file in the same directory, then atomically
//! rename over the original, with a disk-space pre-check.
//!
//! Unchanged entries are **raw-copied** (no recompression; encrypted
//! entries stay encrypted without needing the password). Added files are
//! compressed with the usual create options.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use squallz_format_api::{
    ArchiveWriter, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType, FormatError,
    ProgressSink, UpdateOp,
};
use zip::ZipArchive;

use super::error::map_zip_error;
use super::writer::ZipArchiveWriter;

/// Extra bytes required on top of the estimate (central directory growth,
/// compression overhead on incompressible adds).
const SPACE_SLACK: u64 = 1024 * 1024;

/// One file-system item scheduled for addition.
struct AddItem {
    src: Option<PathBuf>,
    meta: EntryMeta,
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
    let deletes = build_path_set(ops.iter().filter_map(|op| match op {
        UpdateOp::Delete { pattern } => Some(pattern.as_str()),
        _ => None,
    }))?;
    let renames = build_rename_map(ops);
    let add_excludes = build_path_set(opts.excludes.iter().map(String::as_str))?;
    let add_items = collect_add_items(ops, &add_excludes)?;

    let src_file = File::open(src)?;
    let src_len = src_file.metadata()?.len();
    let mut archive = ZipArchive::new(src_file).map_err(map_zip_error)?;

    // Disk-space pre-check on the volume holding the temporary file.
    let added_bytes: u64 = add_items.iter().map(|i| i.meta.size).sum();
    let needed = src_len
        .saturating_add(added_bytes)
        .saturating_add(SPACE_SLACK);
    let available = fs4::available_space(update_parent(src))?;
    if available < needed {
        return Err(FormatError::DiskFull);
    }

    // Update targets must be deterministic: no missing rename sources, no
    // accidental overwrite, and no duplicate targets in the same operation.
    validate_update_plan(&mut archive, &deletes, &renames, &add_items)?;

    let tmp = update_temp_path(src);

    let result = rewrite(
        &mut archive,
        &tmp,
        &deletes,
        &renames,
        &add_items,
        opts,
        progress,
        ctl,
    );
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

/// Writes the updated archive into `tmp`.
#[allow(clippy::too_many_arguments)] // internal plumbing with distinct roles
fn rewrite(
    archive: &mut ZipArchive<File>,
    tmp: &Path,
    deletes: &Option<GlobSet>,
    renames: &HashMap<String, String>,
    add_items: &[AddItem],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let out = File::create(tmp)?;
    let mut writer = ZipArchiveWriter::new(Box::new(out), opts);

    // Progress in bytes: raw (compressed) bytes for copies, plain bytes for
    // additions.
    let copied_total: u64 = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|f| f.compressed_size()))
        .sum();
    let total = copied_total + add_items.iter().map(|i| i.meta.size).sum::<u64>();
    let mut done = 0u64;

    for i in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive.by_index_raw(i).map_err(map_zip_error)?;
        let name = String::from_utf8_lossy(file.name_raw()).into_owned();
        let key = name.trim_end_matches('/').to_string();
        let compressed = file.compressed_size();
        progress.on_progress(done, total, &EntryPath::from_utf8(name.clone()));
        if deletes.as_ref().is_some_and(|set| set.is_match(&key)) {
            continue; // dropped entry
        }
        let rename_to = renames.get(&name).or_else(|| renames.get(&key));
        writer.raw_copy(file, rename_to.map(String::as_str))?;
        done += compressed;
    }

    for item in add_items {
        ctl.checkpoint()?;
        progress.on_progress(done, total, &item.meta.path);
        match item.meta.entry_type {
            EntryType::File => {
                let src = item
                    .src
                    .as_ref()
                    .ok_or_else(|| FormatError::Other("file add missing source path".into()))?;
                let mut data = File::open(src)?;
                writer.add_entry(&item.meta, Some(&mut data))?;
            }
            _ => writer.add_entry(&item.meta, None)?,
        }
        done += item.meta.size;
    }
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    Box::new(writer).finish()
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
fn validate_update_plan(
    archive: &mut ZipArchive<File>,
    deletes: &Option<GlobSet>,
    renames: &HashMap<String, String>,
    add_items: &[AddItem],
) -> Result<(), FormatError> {
    let mut names: Vec<(String, String)> = Vec::with_capacity(archive.len());
    let mut existing = HashSet::new();
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i).map_err(map_zip_error)?;
        let name = String::from_utf8_lossy(file.name_raw()).into_owned();
        let key = archive_key(&name);
        existing.insert(key.clone());
        names.push((name, key));
    }
    for from in renames.keys() {
        let from_key = archive_key(from);
        let found = names
            .iter()
            .any(|(name, key)| name == from || key == &from_key);
        if !found {
            return Err(FormatError::Other(format!(
                "rename source not found in archive: {from}"
            )));
        }
    }
    let mut removed = HashSet::new();
    for (name, key) in &names {
        if deletes.as_ref().is_some_and(|set| set.is_match(key))
            || renames.contains_key(name)
            || renames.contains_key(key)
        {
            removed.insert(key.clone());
        }
    }
    let mut produced = HashMap::new();
    for target in renames.values() {
        validate_update_target(target, &existing, &removed, &mut produced)?;
    }
    for item in add_items {
        validate_update_target(&item.meta.path.display, &existing, &removed, &mut produced)?;
    }
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
    use super::*;

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
