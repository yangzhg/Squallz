//! Compression input collection: walks the input paths into an entry
//! manifest, applying `CreateOptions.excludes` glob pruning. Transaction
//! holders are internal work directories and never become archive inputs.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::api::{ControlToken, EntryPath, EntryType, FormatError};
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};
use crate::filter::PathFilter;

const SOURCE_CLEANUP_HOLDER_PREFIX: &str = ".squallz-trash-hold-";
const UPDATE_HOLDER_PREFIX: &str = ".squallz-update-holder-";

/// One item of the compression input manifest.
#[derive(Debug)]
pub(crate) struct InputItem {
    pub src: PathBuf,
    pub name: EntryPath,
    pub entry_type: EntryType,
    pub size: u64,
    pub unix_mode: Option<u32>,
    pub modified: Option<SystemTime>,
    link_target: Option<PathBuf>,
}

impl AsRef<InputItem> for InputItem {
    fn as_ref(&self) -> &InputItem {
        self
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PreparedSource {
    File {
        identity: PathIdentity,
        state: RegularFileState,
    },
    Directory {
        identity: PathIdentity,
    },
    Symlink {
        identity: PathIdentity,
        target: PathBuf,
    },
}

/// One create input bound to the file-system object observed during the
/// worker's manifest scan. Handles are opened only while preparing and while
/// streaming so large trees do not exhaust the process file-descriptor limit.
#[derive(Debug)]
pub(crate) struct PreparedInputItem {
    item: InputItem,
    source_path: PathBuf,
    source: PreparedSource,
}

impl AsRef<InputItem> for PreparedInputItem {
    fn as_ref(&self) -> &InputItem {
        &self.item
    }
}

impl PreparedInputItem {
    fn prepare(item: InputItem) -> Result<Self, FormatError> {
        let identity = path_identity(&item.src)?;
        let metadata = std::fs::symlink_metadata(&item.src)?;
        let source = match &item.entry_type {
            EntryType::File => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(input_changed(&item));
                }
                let file = open_regular_file_no_follow(&item.src)?;
                let file_metadata = file.metadata()?;
                let state = RegularFileState::from_metadata(&file_metadata);
                if file_identity(&file)? != identity
                    || path_identity(&item.src)? != identity
                    || !state.matches(&metadata)
                    || state.bytes() != item.size
                    || state.modified() != item.modified
                {
                    return Err(input_changed(&item));
                }
                PreparedSource::File { identity, state }
            }
            EntryType::Dir => {
                if metadata.file_type().is_symlink()
                    || !metadata.is_dir()
                    || path_identity(&item.src)? != identity
                {
                    return Err(input_changed(&item));
                }
                PreparedSource::Directory { identity }
            }
            EntryType::Symlink { .. } => {
                let Some(expected_target) = item.link_target.as_ref() else {
                    return Err(FormatError::Other(
                        "collected symbolic-link input lost its target".into(),
                    ));
                };
                if !metadata.file_type().is_symlink()
                    || std::fs::read_link(&item.src)? != *expected_target
                    || path_identity(&item.src)? != identity
                {
                    return Err(input_changed(&item));
                }
                PreparedSource::Symlink {
                    identity,
                    target: expected_target.clone(),
                }
            }
            _ => {
                return Err(FormatError::Unsupported(format!(
                    "unsupported create input type: {}",
                    item.name.display
                )));
            }
        };
        let source_path = normalized_source_path(&item)?;
        if path_identity(&source_path)? != identity {
            return Err(input_changed(&item));
        }
        Ok(Self {
            item,
            source_path,
            source,
        })
    }

    pub(crate) fn item(&self) -> &InputItem {
        &self.item
    }

    pub(crate) fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub(crate) fn open_file(&self) -> Result<File, FormatError> {
        if !matches!(self.source, PreparedSource::File { .. }) {
            return Err(FormatError::Other(
                "prepared create input is not a regular file".into(),
            ));
        }
        self.validate_path()?;
        let file = open_regular_file_no_follow(&self.source_path)?;
        self.validate_file_handle(&file)?;
        self.validate_path()?;
        Ok(file)
    }

    pub(crate) fn validate_after_read(&self, file: &File) -> Result<(), FormatError> {
        self.validate_file_handle(file)?;
        self.validate_path()
    }

    pub(crate) fn validate_non_file(&self) -> Result<(), FormatError> {
        if matches!(self.source, PreparedSource::File { .. }) {
            return Err(FormatError::Other(
                "prepared create input requires a data reader".into(),
            ));
        }
        self.validate_path()
    }

    fn validate_file_handle(&self, file: &File) -> Result<(), FormatError> {
        let PreparedSource::File { identity, state } = &self.source else {
            return Err(FormatError::Other(
                "prepared create input is not a regular file".into(),
            ));
        };
        if file_identity(file)? != *identity || !state.matches(&file.metadata()?) {
            return Err(input_changed(&self.item));
        }
        Ok(())
    }

    fn validate_path(&self) -> Result<(), FormatError> {
        let identity_before = path_identity(&self.source_path)?;
        let metadata = std::fs::symlink_metadata(&self.source_path)?;
        let valid = match &self.source {
            PreparedSource::File { identity, state } => {
                identity_before == *identity
                    && !metadata.file_type().is_symlink()
                    && state.matches(&metadata)
                    && path_identity(&self.source_path)? == *identity
            }
            PreparedSource::Directory { identity } => {
                identity_before == *identity
                    && !metadata.file_type().is_symlink()
                    && metadata.is_dir()
                    && path_identity(&self.source_path)? == *identity
            }
            PreparedSource::Symlink { identity, target } => {
                identity_before == *identity
                    && metadata.file_type().is_symlink()
                    && std::fs::read_link(&self.source_path)? == *target
                    && path_identity(&self.source_path)? == *identity
            }
        };
        if !valid {
            return Err(input_changed(&self.item));
        }
        Ok(())
    }
}

trait CollectedInput: AsRef<InputItem> {
    fn deduplication_path(&self) -> Result<PathBuf, FormatError>;

    fn matches_same_source(&self, other: &Self) -> bool {
        let left = self.as_ref();
        let right = other.as_ref();
        left.entry_type == right.entry_type
            && left.size == right.size
            && left.unix_mode == right.unix_mode
            && left.modified == right.modified
            && left.link_target == right.link_target
    }
}

impl CollectedInput for InputItem {
    fn deduplication_path(&self) -> Result<PathBuf, FormatError> {
        normalized_source_path(self)
    }
}

impl CollectedInput for PreparedInputItem {
    fn deduplication_path(&self) -> Result<PathBuf, FormatError> {
        Ok(self.source_path.clone())
    }

    fn matches_same_source(&self, other: &Self) -> bool {
        self.source == other.source
            && self.item.entry_type == other.item.entry_type
            && self.item.size == other.item.size
            && self.item.unix_mode == other.item.unix_mode
            && self.item.modified == other.item.modified
            && self.item.link_target == other.item.link_target
    }
}

struct CollectedCandidate<T> {
    item: T,
    root_index: usize,
}

pub(crate) fn prepare_single_stream_input(
    input: &Path,
    name: EntryPath,
) -> Result<PreparedInputItem, FormatError> {
    // Bare compressors historically accept a symbolic link to a file. Resolve
    // that user-facing alias once, then bind and later open the resolved file
    // without following a new final-component link.
    let source = std::fs::canonicalize(input)?;
    let metadata = std::fs::symlink_metadata(&source)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FormatError::Unsupported(
            "single-stream compression requires one regular file".into(),
        ));
    }
    PreparedInputItem::prepare(InputItem {
        src: source,
        name,
        entry_type: EntryType::File,
        size: metadata.len(),
        unix_mode: unix_mode_of(&metadata),
        modified: metadata.modified().ok(),
        link_target: None,
    })
}

fn normalized_source_path(item: &InputItem) -> Result<PathBuf, FormatError> {
    if !matches!(item.entry_type, EntryType::Symlink { .. }) {
        return Ok(std::fs::canonicalize(&item.src)?);
    }

    let absolute = std::path::absolute(&item.src)?;
    let name = absolute.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "symbolic-link source has no file name: {}",
            item.name.display
        ))
    })?;
    let parent = absolute.parent().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "symbolic-link source has no parent: {}",
            item.name.display
        ))
    })?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn input_changed(item: &InputItem) -> FormatError {
    FormatError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("input changed after preparation: {}", item.name.display),
    ))
}

/// Walks the input paths and produces the entry manifest. Entry names are
/// relative to each input's parent directory (the top-level folder name is
/// kept); symbolic links are not followed. Entries matching `excludes` are
/// pruned (a matched directory is skipped with its whole subtree).
pub(crate) fn collect_inputs(
    inputs: &[PathBuf],
    excludes: &PathFilter,
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_excluding(inputs, excludes, |_| false)
}

/// Walks inputs while pruning file-system paths reserved by the caller.
/// A matched directory is skipped before its children are read. Overlapping
/// roots remain distinct for callers such as checksum and duplicate scans.
pub(crate) fn collect_inputs_excluding(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    mut exclude_path: impl FnMut(&Path) -> bool,
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_mapped_with_progress(
        inputs,
        excludes,
        &mut exclude_path,
        &mut |_count, _path| {},
        &mut Ok,
        false,
    )
}

/// Walks creation inputs and merges overlapping roots after exclusions.
/// Progress reports included candidates before that merge.
pub(crate) fn collect_inputs_excluding_with_progress(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    mut exclude_path: impl FnMut(&Path) -> bool,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_mapped_with_progress(
        inputs,
        excludes,
        &mut exclude_path,
        &mut progress,
        &mut Ok,
        true,
    )
}

pub(crate) fn collect_prepared_inputs_excluding_with_progress(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    mut exclude_path: impl FnMut(&Path) -> bool,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<Vec<PreparedInputItem>, FormatError> {
    collect_inputs_mapped_with_progress(
        inputs,
        excludes,
        &mut exclude_path,
        &mut progress,
        &mut PreparedInputItem::prepare,
        true,
    )
}

pub(crate) fn collect_prepared_input_as(
    input: &Path,
    name: &EntryPath,
    excludes: &PathFilter,
    ctl: &ControlToken,
    mut progress: impl FnMut(&EntryPath),
) -> Result<Vec<PreparedInputItem>, FormatError> {
    ctl.checkpoint()?;
    if excludes.matches(name.display.trim_end_matches('/')) {
        return Ok(Vec::new());
    }
    if is_internal_transaction_artifact(input) {
        return Err(FormatError::Unsupported(format!(
            "internal transaction artifact cannot be archived directly: {}",
            input.display()
        )));
    }
    let mut out = Vec::new();
    walk_named(
        input,
        name.clone(),
        excludes,
        &mut |_| false,
        false,
        Some(ctl),
        &mut |item| {
            let item = PreparedInputItem::prepare(item)?;
            progress(&item.item().name);
            out.push(item);
            ctl.checkpoint()
        },
    )?;
    Ok(out)
}

pub(crate) fn deduplicate_prepared_input_roots(
    items: Vec<PreparedInputItem>,
) -> Result<Vec<PreparedInputItem>, FormatError> {
    let candidates = items
        .into_iter()
        .enumerate()
        .map(|(root_index, item)| CollectedCandidate { item, root_index })
        .collect();
    deduplicate_collected_inputs(candidates)
}

pub(crate) fn collect_inputs_with_progress(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_mapped_with_progress(
        inputs,
        excludes,
        &mut |_| false,
        &mut progress,
        &mut Ok,
        true,
    )
}

fn collect_inputs_mapped_with_progress<T: CollectedInput>(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    exclude_path: &mut impl FnMut(&Path) -> bool,
    progress: &mut impl FnMut(usize, &EntryPath),
    map: &mut impl FnMut(InputItem) -> Result<T, FormatError>,
    deduplicate: bool,
) -> Result<Vec<T>, FormatError> {
    let mut candidates = Vec::new();
    let mut items = Vec::new();
    for (root_index, input) in inputs.iter().enumerate() {
        if is_internal_transaction_artifact(input) {
            return Err(FormatError::Unsupported(format!(
                "internal transaction artifact cannot be archived directly: {}",
                input.display()
            )));
        }
        let base = input.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        let mut emit = |item| {
            let item = map(item)?;
            let count = if deduplicate {
                candidates.len()
            } else {
                items.len()
            }
            .saturating_add(1);
            progress(count, &item.as_ref().name);
            if deduplicate {
                candidates.push(CollectedCandidate { item, root_index });
            } else {
                items.push(item);
            }
            Ok(())
        };
        walk(input, &base, excludes, exclude_path, &mut emit)?;
    }
    if !deduplicate {
        return Ok(items);
    }
    deduplicate_collected_inputs(candidates)
}

fn deduplicate_collected_inputs<T: CollectedInput>(
    candidates: Vec<CollectedCandidate<T>>,
) -> Result<Vec<T>, FormatError> {
    let candidate_count = candidates.len();
    let mut preferred_by_source: HashMap<PathBuf, usize> = HashMap::new();
    let mut keep = vec![false; candidate_count];
    for (index, candidate) in candidates.iter().enumerate() {
        let source_path = candidate.item.deduplication_path()?;
        if let Some(&preferred) = preferred_by_source.get(&source_path) {
            if !candidates[preferred]
                .item
                .matches_same_source(&candidate.item)
            {
                return Err(input_changed(candidate.item.as_ref()));
            }
            if archive_path_depth(&candidate.item.as_ref().name)
                > archive_path_depth(&candidates[preferred].item.as_ref().name)
            {
                keep[preferred] = false;
                keep[index] = true;
                preferred_by_source.insert(source_path, index);
            }
        } else {
            keep[index] = true;
            preferred_by_source.insert(source_path, index);
        }
    }

    let preferred = keep.clone();
    let mut directories = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if !matches!(&candidate.item.as_ref().entry_type, EntryType::Dir) {
            continue;
        }
        directories
            .entry((
                candidate.root_index,
                candidate.item.as_ref().name.display.as_str(),
            ))
            .or_insert(index);
    }
    for (index, candidate) in candidates.iter().enumerate() {
        if !preferred[index] {
            continue;
        }
        let mut name = candidate.item.as_ref().name.display.as_str();
        while let Some((parent, _)) = name.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            if let Some(&directory_index) = directories.get(&(candidate.root_index, parent)) {
                keep[directory_index] = true;
            }
            name = parent;
        }
    }

    Ok(candidates
        .into_iter()
        .zip(keep)
        .filter_map(|(candidate, keep)| keep.then_some(candidate.item))
        .collect())
}

fn archive_path_depth(path: &EntryPath) -> usize {
    path.raw
        .split(|component| *component == b'/')
        .filter(|component| !component.is_empty())
        .count()
}

fn walk(
    path: &Path,
    base: &Path,
    excludes: &PathFilter,
    exclude_path: &mut impl FnMut(&Path) -> bool,
    emit: &mut impl FnMut(InputItem) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    let rel = match path.strip_prefix(base) {
        Ok(rel) => rel,
        Err(_) => path,
    };
    let name = EntryPath::from_utf8(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    );
    walk_named(path, name, excludes, exclude_path, true, None, emit)
}

fn walk_named(
    path: &Path,
    name: EntryPath,
    excludes: &PathFilter,
    exclude_path: &mut impl FnMut(&Path) -> bool,
    stat_before_excludes: bool,
    ctl: Option<&ControlToken>,
    emit: &mut impl FnMut(InputItem) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    input_checkpoint(ctl)?;
    if !stat_before_excludes && excludes.matches(name.display.trim_end_matches('/')) {
        return Ok(());
    }
    if is_internal_transaction_artifact(path) {
        return Ok(());
    }
    if exclude_path(path) {
        return Ok(());
    }
    let metadata = stat_before_excludes
        .then(|| std::fs::symlink_metadata(path))
        .transpose()?;
    if stat_before_excludes && excludes.matches(name.display.trim_end_matches('/')) {
        // Pruned: a matched directory is skipped together with its subtree.
        return Ok(());
    }
    let meta = match metadata {
        Some(metadata) => metadata,
        None => std::fs::symlink_metadata(path)?,
    };
    let unix_mode = unix_mode_of(&meta);
    let modified = meta.modified().ok();

    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        emit(InputItem {
            src: path.to_path_buf(),
            name,
            entry_type: EntryType::Symlink {
                target: target.to_string_lossy().into_owned().into_bytes(),
            },
            size: 0,
            unix_mode,
            modified,
            link_target: Some(target),
        })?;
    } else if meta.is_dir() {
        let child_prefix = name.display.clone();
        emit(InputItem {
            src: path.to_path_buf(),
            name,
            entry_type: EntryType::Dir,
            size: 0,
            unix_mode,
            modified,
            link_target: None,
        })?;
        let children = collect_child_paths(
            std::fs::read_dir(path)?.map(|entry| entry.map(|entry| entry.path())),
            ctl,
        )?;
        for child in children {
            let child_name = child
                .file_name()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| FormatError::UnsafeFileName(child.display().to_string()))?;
            walk_named(
                &child,
                EntryPath::from_utf8(format!("{}/{}", child_prefix, child_name.to_string_lossy())),
                excludes,
                exclude_path,
                stat_before_excludes,
                ctl,
                emit,
            )?;
        }
    } else if meta.is_file() {
        emit(InputItem {
            src: path.to_path_buf(),
            name,
            entry_type: EntryType::File,
            size: meta.len(),
            unix_mode,
            modified,
            link_target: None,
        })?;
    } else {
        return Err(FormatError::Unsupported(format!(
            "unsupported input file type: {}",
            path.display()
        )));
    }
    Ok(())
}

fn collect_child_paths(
    mut entries: impl Iterator<Item = io::Result<PathBuf>>,
    ctl: Option<&ControlToken>,
) -> Result<Vec<PathBuf>, FormatError> {
    let mut children = Vec::new();
    loop {
        input_checkpoint(ctl)?;
        let entry = entries.next();
        input_checkpoint(ctl)?;
        match entry {
            Some(entry) => children.push(entry?),
            None => break,
        }
    }
    children.sort();
    input_checkpoint(ctl)?;
    Ok(children)
}

fn input_checkpoint(ctl: Option<&ControlToken>) -> Result<(), FormatError> {
    if let Some(ctl) = ctl {
        ctl.checkpoint()?;
    }
    Ok(())
}

fn is_internal_transaction_artifact(path: &Path) -> bool {
    // Keep the common walk path lexical; only names in the reserved namespace
    // need an extra metadata lookup to distinguish real work paths from user
    // entries with similar names.
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let holder_shaped = is_source_cleanup_holder_name(name) || is_update_holder_name(name);
    let work_file_shaped = is_update_work_file_name(name);
    if !holder_shaped && !work_file_shaped {
        return false;
    }
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        (holder_shaped && metadata.file_type().is_dir())
            || (work_file_shaped && metadata.file_type().is_file())
    })
}

fn is_source_cleanup_holder_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(SOURCE_CLEANUP_HOLDER_PREFIX) else {
        return false;
    };
    let Some((pid, sequence)) = suffix.split_once('-') else {
        return false;
    };
    is_canonical_positive_u32(pid) && is_canonical_positive_u64(sequence)
}

fn is_update_holder_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix(UPDATE_HOLDER_PREFIX) else {
        return false;
    };
    let mut components = suffix.split('-');
    let (Some(key), Some(pid), Some(sequence), None) = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) else {
        return false;
    };
    key.len() == 16
        && key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && is_canonical_positive_u32(pid)
        && is_canonical_positive_u64(sequence)
}

fn is_update_work_file_name(name: &str) -> bool {
    if name
        .strip_prefix(".squallz-update-stage-")
        .and_then(|suffix| suffix.strip_suffix(".tmp"))
        .is_some_and(|suffix| {
            let Some((key, process)) = suffix.split_once('-') else {
                return false;
            };
            is_lower_hex(key, 16) && valid_process_sequence(process)
        })
    {
        return true;
    }
    if name
        .strip_prefix(".squallz-update-journal-")
        .and_then(|suffix| suffix.strip_suffix(".tmp"))
        .is_some_and(|suffix| {
            valid_process_sequence(suffix)
                || suffix.split_once('-').is_some_and(|(key, process)| {
                    is_lower_hex(key, 16) && valid_process_sequence(process)
                })
        })
    {
        return true;
    }
    let Some(record) = name.strip_prefix(".squallz-update-") else {
        return false;
    };
    record
        .strip_suffix(".completed.json")
        .or_else(|| record.strip_suffix(".pending.json"))
        .or_else(|| record.strip_suffix(".json"))
        .is_some_and(|key| is_lower_hex(key, 64))
}

fn valid_process_sequence(value: &str) -> bool {
    let Some((pid, sequence)) = value.split_once('-') else {
        return false;
    };
    !sequence.contains('-') && is_canonical_positive_u32(pid) && is_canonical_positive_u64(sequence)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn has_canonical_positive_integer_syntax(value: &str) -> bool {
    let Some((&first, rest)) = value.as_bytes().split_first() else {
        return false;
    };
    (b'1'..=b'9').contains(&first) && rest.iter().all(|byte| byte.is_ascii_digit())
}

fn is_canonical_positive_u32(value: &str) -> bool {
    has_canonical_positive_integer_syntax(value)
        && value.parse::<u32>().is_ok_and(|value| value != 0)
}

fn is_canonical_positive_u64(value: &str) -> bool {
    has_canonical_positive_integer_syntax(value)
        && value.parse::<u64>().is_ok_and(|value| value != 0)
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-inputs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(items: &[InputItem]) -> Vec<String> {
        items.iter().map(|i| i.name.display.clone()).collect()
    }

    #[test]
    fn collect_inputs_walks_tree_with_top_folder_name() {
        let dir = temp_dir("walk");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::write(root.join("sub/b.txt"), b"world!").unwrap();

        let items = collect_inputs(std::slice::from_ref(&root), &PathFilter::default()).unwrap();
        assert_eq!(
            names(&items),
            vec![
                "project",
                "project/a.txt",
                "project/sub",
                "project/sub/b.txt"
            ]
        );
        let total: u64 = items.iter().map(|i| i.size).sum();
        assert_eq!(total, 11);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlapping_parent_and_child_are_written_once_with_parent_layout() {
        let dir = temp_dir("overlapping-roots-parent-first");
        let root = dir.join("project");
        let child = root.join("README.md");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&child, b"hello").unwrap();

        let mut scanned = 0;
        let items =
            collect_inputs_with_progress(&[root, child], &PathFilter::default(), |count, _| {
                scanned = count;
            })
            .unwrap();
        assert_eq!(names(&items), vec!["project", "project/README.md"]);
        assert_eq!(scanned.saturating_sub(items.len()), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_deduplicating_collection_preserves_overlapping_roots() {
        let dir = temp_dir("overlapping-roots-nondeduplicating");
        let root = dir.join("project");
        let child = root.join("README.md");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&child, b"hello").unwrap();

        let items = collect_inputs(&[root, child], &PathFilter::default()).unwrap();

        assert_eq!(
            names(&items),
            vec!["project", "project/README.md", "README.md"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlapping_child_before_parent_still_uses_parent_layout() {
        let dir = temp_dir("overlapping-roots-child-first");
        let root = dir.join("project");
        let child = root.join("README.md");
        let sibling = dir.join("notes.txt");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&child, b"hello").unwrap();
        std::fs::write(&sibling, b"notes").unwrap();

        let mut scanned = 0;
        let items = collect_inputs_with_progress(
            &[child, sibling, root],
            &PathFilter::default(),
            |count, _| scanned = count,
        )
        .unwrap();
        assert_eq!(
            names(&items),
            vec!["notes.txt", "project", "project/README.md"]
        );
        assert_eq!(scanned.saturating_sub(items.len()), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn similarly_prefixed_roots_remain_distinct() {
        let dir = temp_dir("overlapping-roots-boundary");
        let foo = dir.join("foo");
        let foobar = dir.join("foobar");
        std::fs::create_dir_all(&foo).unwrap();
        std::fs::create_dir_all(&foobar).unwrap();

        let items = collect_inputs_with_progress(&[foo, foobar], &PathFilter::default(), |_, _| {})
            .unwrap();
        assert_eq!(names(&items), vec!["foo", "foobar"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn equivalent_root_spellings_are_deduplicated() {
        let dir = temp_dir("overlapping-roots-equivalent");
        let file = dir.join("report.txt");
        std::fs::write(&file, b"report").unwrap();
        let equivalent = dir.join(".").join("report.txt");

        let mut scanned = 0;
        let items = collect_inputs_with_progress(
            &[file, equivalent],
            &PathFilter::default(),
            |count, _| {
                scanned = count;
            },
        )
        .unwrap();
        assert_eq!(names(&items), vec!["report.txt"]);
        assert_eq!(scanned.saturating_sub(items.len()), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlap_deduplication_runs_after_path_excludes() {
        let dir = temp_dir("overlapping-roots-excludes");
        let root = dir.join("project");
        let child = root.join("README.md");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&child, b"hello").unwrap();
        let excludes = PathFilter::new(&["project/README.md".to_owned()]).unwrap();

        let items = collect_inputs_with_progress(&[root, child], &excludes, |_, _| {}).unwrap();

        assert_eq!(names(&items), vec!["project", "README.md"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_overlap_keeps_directory_metadata_for_the_surviving_layout() {
        let dir = temp_dir("overlapping-directory-excludes");
        let root = dir.join("project");
        let child = root.join("docs");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("guide.txt"), b"guide").unwrap();
        let excludes = PathFilter::new(&["project/docs/guide.txt".to_owned()]).unwrap();

        let items = collect_inputs_with_progress(&[root, child], &excludes, |_, _| {}).unwrap();

        assert_eq!(
            names(&items),
            vec!["project", "project/docs", "docs", "docs/guide.txt"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_links_with_distinct_paths_remain_distinct_archive_entries() {
        let dir = temp_dir("hard-link-roots");
        let first = dir.join("first.txt");
        let second = dir.join("second.txt");
        std::fs::write(&first, b"shared").unwrap();
        std::fs::hard_link(&first, &second).unwrap();

        let items =
            collect_inputs_with_progress(&[first, second], &PathFilter::default(), |_, _| {})
                .unwrap();

        assert_eq!(names(&items), vec!["first.txt", "second.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_manifest_deduplicates_after_sources_are_bound() {
        let dir = temp_dir("prepared-overlapping-roots");
        let root = dir.join("project");
        let child = root.join("README.md");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&child, b"hello").unwrap();

        let mut scanned = 0;
        let items = collect_prepared_inputs_excluding_with_progress(
            &[child, root],
            &PathFilter::default(),
            |_| false,
            |count, _| scanned = count,
        )
        .unwrap();

        assert_eq!(
            items
                .iter()
                .map(|item| item.item().name.display.as_str())
                .collect::<Vec<_>>(),
            vec!["project", "project/README.md"]
        );
        assert_eq!(scanned.saturating_sub(items.len()), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_manifest_rejects_a_source_changed_between_duplicate_scans() {
        let dir = temp_dir("prepared-overlap-change");
        let file = dir.join("report.txt");
        std::fs::write(&file, b"before").unwrap();
        let duplicate = dir.join(".").join("report.txt");

        let error = collect_prepared_inputs_excluding_with_progress(
            &[file.clone(), duplicate],
            &PathFilter::default(),
            |_| false,
            |count, _| {
                if count == 1 {
                    std::fs::write(&file, b"changed after first binding").unwrap();
                }
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::InvalidData
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlapping_empty_directory_tree_keeps_only_the_parent_layout() {
        let dir = temp_dir("overlapping-empty-directories");
        let root = dir.join("project");
        let child = root.join("empty");
        std::fs::create_dir_all(child.join("nested")).unwrap();

        let items = collect_inputs_with_progress(&[child, root], &PathFilter::default(), |_, _| {})
            .unwrap();

        assert_eq!(
            names(&items),
            vec!["project", "project/empty", "project/empty/nested"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn distinct_symbolic_links_to_one_target_remain_distinct_entries() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symbolic-link-roots");
        let target = dir.join("target.txt");
        let first = dir.join("first-link");
        let second = dir.join("second-link");
        std::fs::write(&target, b"target").unwrap();
        symlink(&target, &first).unwrap();
        symlink(&target, &second).unwrap();

        let items =
            collect_inputs_with_progress(&[first, second], &PathFilter::default(), |_, _| {})
                .unwrap();

        assert_eq!(names(&items), vec!["first-link", "second-link"]);
        assert!(items
            .iter()
            .all(|item| matches!(&item.entry_type, EntryType::Symlink { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_inputs_applies_exclude_globs() {
        let dir = temp_dir("excludes");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".git/config"), b"git").unwrap();
        std::fs::write(root.join(".git/objects/x"), b"obj").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("scratch.tmp"), b"tmp").unwrap();

        let excludes = PathFilter::new(&[".git".to_owned(), "*.tmp".to_owned()]).unwrap();
        let items = collect_inputs(std::slice::from_ref(&root), &excludes).unwrap();
        assert_eq!(
            names(&items),
            vec!["project", "project/src", "project/src/main.rs"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_inputs_still_checks_an_excluded_explicit_path() {
        let dir = temp_dir("excluded-missing");
        let missing = dir.join("missing.tmp");
        let excludes = PathFilter::new(&["*.tmp".to_owned()]).unwrap();

        let error = collect_inputs(&[missing], &excludes).unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::NotFound
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_inputs_reports_progress_for_kept_items() {
        let dir = temp_dir("progress");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".git/config"), b"git").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("scratch.tmp"), b"tmp").unwrap();

        let excludes = PathFilter::new(&[".git".to_owned(), "*.tmp".to_owned()]).unwrap();
        let mut progress = Vec::new();
        let items =
            collect_inputs_with_progress(std::slice::from_ref(&root), &excludes, |count, path| {
                progress.push((count, path.display.clone()));
            })
            .unwrap();

        assert_eq!(
            names(&items),
            vec!["project", "project/src", "project/src/main.rs"]
        );
        assert_eq!(
            progress,
            vec![
                (1, "project".to_owned()),
                (2, "project/src".to_owned()),
                (3, "project/src/main.rs".to_owned())
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn child_enumeration_checks_for_cancellation_after_each_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let ctl = ControlToken::new();
        let ctl_for_entries = Arc::clone(&ctl);
        let reads = Arc::new(AtomicUsize::new(0));
        let reads_for_entries = Arc::clone(&reads);
        let entries = std::iter::from_fn(move || {
            let read = reads_for_entries.fetch_add(1, Ordering::SeqCst) + 1;
            if read == 2 {
                ctl_for_entries.cancel();
            }
            Some(Ok(PathBuf::from(format!("entry-{read}"))))
        });

        let error = collect_child_paths(entries, Some(ctl.as_ref())).unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn source_cleanup_holder_names_require_two_canonical_positive_integers() {
        assert!(is_source_cleanup_holder_name(".squallz-trash-hold-1-1"));
        assert!(is_source_cleanup_holder_name(
            ".squallz-trash-hold-4294967295-18446744073709551615"
        ));

        for name in [
            ".squallz-trash-hold-0-1",
            ".squallz-trash-hold-1-0",
            ".squallz-trash-hold-01-1",
            ".squallz-trash-hold-1-01",
            ".squallz-trash-hold-4294967296-1",
            ".squallz-trash-hold-1-18446744073709551616",
            ".squallz-trash-hold-1",
            ".squallz-trash-hold-1-1-extra",
            ".squallz-trash-hold-active-1",
            ".squallz-trash-hold-1-",
        ] {
            assert!(!is_source_cleanup_holder_name(name), "matched {name}");
        }
    }

    #[test]
    fn update_holder_names_require_a_key_and_two_canonical_positive_integers() {
        assert!(is_update_holder_name(
            ".squallz-update-holder-0123456789abcdef-1-1"
        ));
        assert!(is_update_holder_name(
            ".squallz-update-holder-abcdef0123456789-4294967295-18446744073709551615"
        ));

        for name in [
            ".squallz-update-holder-0123456789abcde-1-1",
            ".squallz-update-holder-0123456789abcdef0-1-1",
            ".squallz-update-holder-0123456789ABCDEf-1-1",
            ".squallz-update-holder-0123456789abcdeg-1-1",
            ".squallz-update-holder-0123456789abcdef-0-1",
            ".squallz-update-holder-0123456789abcdef-1-0",
            ".squallz-update-holder-0123456789abcdef-01-1",
            ".squallz-update-holder-0123456789abcdef-1-01",
            ".squallz-update-holder-0123456789abcdef-1-1-extra",
        ] {
            assert!(!is_update_holder_name(name), "matched {name}");
        }
    }

    #[test]
    fn update_transaction_files_are_pruned_but_similar_names_are_kept() {
        let dir = temp_dir("update-transaction-files");
        let root = dir.join("project");
        std::fs::create_dir_all(&root).unwrap();
        let key = "0123456789abcdef".repeat(4);
        let internal = [
            format!(".squallz-update-stage-{}-42-7.tmp", &key[..16]),
            format!(".squallz-update-{key}.json"),
            format!(".squallz-update-{key}.pending.json"),
            format!(".squallz-update-{key}.completed.json"),
            ".squallz-update-journal-42-7.tmp".to_owned(),
            format!(".squallz-update-journal-{}-42-8.tmp", &key[..16]),
        ];
        for name in &internal {
            std::fs::write(root.join(name), b"internal").unwrap();
        }
        let similar = [
            ".squallz-update-notes.json",
            ".squallz-update-stage-0123456789abcde-42-7.tmp",
            ".squallz-update-journal-42-7.tmp.notes",
        ];
        for name in similar {
            std::fs::write(root.join(name), b"keep").unwrap();
        }

        let items = collect_inputs(std::slice::from_ref(&root), &PathFilter::default()).unwrap();
        let names = names(&items);

        for name in internal {
            assert!(!names.iter().any(|entry| entry.ends_with(&name)));
        }
        for name in similar {
            assert!(names.iter().any(|entry| entry.ends_with(name)));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_cleanup_holders_are_pruned_but_similar_user_entries_are_kept() {
        let dir = temp_dir("source-cleanup-holders");
        let root = dir.join("project");
        let active = root.join(format!(
            "{SOURCE_CLEANUP_HOLDER_PREFIX}{}-1",
            std::process::id()
        ));
        let stale = root.join(format!("{SOURCE_CLEANUP_HOLDER_PREFIX}424242-9"));
        let update = root.join(format!(
            "{UPDATE_HOLDER_PREFIX}0123456789abcdef-{}-2",
            std::process::id()
        ));
        let similar = root.join(format!("{SOURCE_CLEANUP_HOLDER_PREFIX}42-7-notes"));
        let zero = root.join(format!("{SOURCE_CLEANUP_HOLDER_PREFIX}0-7"));
        let holder_shaped_file = root.join(format!("{SOURCE_CLEANUP_HOLDER_PREFIX}42-8"));

        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&update).unwrap();
        std::fs::create_dir_all(&similar).unwrap();
        std::fs::create_dir_all(&zero).unwrap();
        std::fs::write(active.join("active.txt"), b"active").unwrap();
        std::fs::write(stale.join("stale.txt"), b"stale").unwrap();
        std::fs::write(update.join("update.txt"), b"update").unwrap();
        std::fs::write(similar.join("notes.txt"), b"similar").unwrap();
        std::fs::write(zero.join("notes.txt"), b"zero").unwrap();
        std::fs::write(&holder_shaped_file, b"ordinary file").unwrap();
        std::fs::write(root.join("payload.txt"), b"payload").unwrap();

        let items = collect_inputs(std::slice::from_ref(&root), &PathFilter::default()).unwrap();
        let names = names(&items);

        assert!(names.iter().any(|name| name == "project/payload.txt"));
        assert!(names
            .iter()
            .any(|name| name.ends_with(".squallz-trash-hold-42-7-notes/notes.txt")));
        assert!(names
            .iter()
            .any(|name| name.ends_with(".squallz-trash-hold-0-7/notes.txt")));
        assert!(names
            .iter()
            .any(|name| name.ends_with(".squallz-trash-hold-42-8")));
        assert!(!names.iter().any(|name| name.contains("active.txt")));
        assert!(!names.iter().any(|name| name.contains("stale.txt")));
        assert!(!names.iter().any(|name| name.contains("update.txt")));
        assert!(!names.iter().any(|name| {
            name.ends_with(&format!(".squallz-trash-hold-{}-1", std::process::id()))
        }));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn explicit_source_cleanup_holder_input_is_rejected() {
        let dir = temp_dir("explicit-source-cleanup-holder");
        let holder = dir.join(format!(
            "{SOURCE_CLEANUP_HOLDER_PREFIX}{}-11",
            std::process::id()
        ));
        std::fs::create_dir(&holder).unwrap();
        std::fs::write(holder.join("source.txt"), b"source").unwrap();

        let error = collect_inputs(std::slice::from_ref(&holder), &PathFilter::default())
            .expect_err("explicit holder input must fail");
        assert!(matches!(
            error,
            FormatError::Unsupported(message)
                if message.contains("internal transaction artifact cannot be archived directly")
        ));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn collect_inputs_rejects_special_file_nodes() {
        use std::os::unix::net::UnixListener;

        let dir = temp_dir("special-file");
        let socket = dir.join("service.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let error =
            collect_inputs(std::slice::from_ref(&socket), &PathFilter::default()).unwrap_err();
        assert!(matches!(
            error,
            FormatError::Unsupported(message)
                if message.contains("unsupported input file type")
        ));

        drop(listener);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
