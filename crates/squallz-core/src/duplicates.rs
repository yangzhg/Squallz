//! Duplicate-file detection over local inputs.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::api::{ControlToken, EntryPath, EntryType, FormatError, ProgressSink};
use crate::filesystem_identity::{
    file_identity_and_links, open_regular_file_no_follow, path_identity, PathIdentity,
    RegularFileState,
};
use crate::{inputs, PathFilter};

const HASH_BUFFER_SIZE: usize = 128 * 1024;
const HASH_PROGRESS_STEP_BYTES: u64 = 1024 * 1024;

/// One representative per physical file with the same byte length and BLAKE3 digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub paths: Vec<PathBuf>,
    /// Files whose representative was their only directory link during the scan.
    pub single_link_files: usize,
}

impl DuplicateGroup {
    pub fn count(&self) -> usize {
        self.paths.len()
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        let removable = self.single_link_files.min(self.count().saturating_sub(1));
        self.size.saturating_mul(removable as u64)
    }
}

/// Summary returned by a duplicate-file scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DuplicateScanReport {
    pub input_count: usize,
    pub entries_scanned: usize,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub candidate_files: usize,
    pub hashed_bytes: u64,
    pub groups: Vec<DuplicateGroup>,
}

impl DuplicateScanReport {
    pub fn duplicate_groups(&self) -> usize {
        self.groups.len()
    }

    pub fn duplicate_files(&self) -> usize {
        self.groups.iter().map(DuplicateGroup::count).sum()
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        self.groups
            .iter()
            .map(DuplicateGroup::reclaimable_bytes)
            .sum()
    }
}

pub(crate) fn find_duplicates(
    inputs: &[PathBuf],
    excludes: &[String],
    min_size: u64,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<DuplicateScanReport, FormatError> {
    ctl.checkpoint()?;
    let filter = PathFilter::new(excludes)?;
    let items = inputs::collect_inputs_with_progress(
        inputs,
        &filter,
        |count, path| progress.on_scan_progress(count as u64, path),
        ctl,
    )?;
    let entries_scanned = items.len();
    let mut files_by_size: BTreeMap<u64, Vec<PathBuf>> = BTreeMap::new();
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0u64;

    for item in items {
        ctl.checkpoint()?;
        if item.entry_type != EntryType::File {
            continue;
        }
        files_scanned += 1;
        bytes_scanned = bytes_scanned.saturating_add(item.size);
        if item.size >= min_size {
            files_by_size.entry(item.size).or_default().push(item.src);
        }
    }

    let mut report = DuplicateScanReport {
        input_count: inputs.len(),
        entries_scanned,
        files_scanned,
        bytes_scanned,
        ..DuplicateScanReport::default()
    };
    let mut candidates = Vec::new();
    for (size, mut paths) in files_by_size {
        ctl.checkpoint()?;
        if paths.len() < 2 {
            continue;
        }
        paths.sort();
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        for path in paths {
            ctl.checkpoint()?;
            let identity = path_identity(&path)?;
            if seen.insert(identity) {
                files.push((path, identity));
            }
        }
        if files.len() < 2 {
            continue;
        }
        report.candidate_files += files.len();
        report.hashed_bytes = report
            .hashed_bytes
            .saturating_add(size.saturating_mul(files.len() as u64));
        candidates.push((size, files));
    }

    let mut done = 0;
    for (size, files) in candidates {
        // Representatives arrive in path order, so each group's paths stay sorted.
        let mut groups_by_hash = BTreeMap::new();
        for (path, identity) in files {
            ctl.checkpoint()?;
            let (hash, links) = hash_file(
                &path,
                identity,
                size,
                progress,
                &mut done,
                report.hashed_bytes,
                ctl,
            )?;
            let (paths, single_link_files) = groups_by_hash
                .entry(hash)
                .or_insert_with(|| (Vec::new(), 0));
            paths.push(path);
            *single_link_files += usize::from(links == 1);
        }
        for (hash, (paths, single_link_files)) in groups_by_hash {
            ctl.checkpoint()?;
            if paths.len() > 1 {
                report.groups.push(DuplicateGroup {
                    hash,
                    size,
                    paths,
                    single_link_files,
                });
            }
        }
    }
    ctl.checkpoint()?;
    report.groups.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.hash.cmp(&b.hash))
    });
    ctl.checkpoint()?;
    Ok(report)
}

fn hash_file(
    path: &Path,
    identity: PathIdentity,
    size: u64,
    progress: &dyn ProgressSink,
    done: &mut u64,
    total: u64,
    ctl: &ControlToken,
) -> Result<(String, u64), FormatError> {
    ctl.checkpoint()?;
    let changed = || FormatError::Other("input changed during duplicate scan".into());
    let mut file = open_regular_file_no_follow(path)?;
    let metadata = file.metadata()?;
    let state = RegularFileState::from_metadata(&metadata);
    let (opened_identity, links) = file_identity_and_links(&file)?;
    if !metadata.is_file() || opened_identity != identity || state.bytes() != size {
        return Err(changed());
    }
    let current = EntryPath::from_utf8(path.display().to_string());
    progress.on_progress(*done, total, &current);
    ctl.checkpoint()?;
    let start = *done;
    let mut last_reported = start;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    let mut reader = (&mut file).take(size);
    loop {
        ctl.checkpoint()?;
        let read = reader.read(&mut buf)?;
        ctl.checkpoint()?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        *done = done.saturating_add(read as u64);
        if done.saturating_sub(last_reported) >= HASH_PROGRESS_STEP_BYTES {
            progress.on_progress(*done, total, &current);
            last_reported = *done;
        }
        ctl.checkpoint()?;
    }
    if *done != last_reported {
        progress.on_progress(*done, total, &current);
    }
    ctl.checkpoint()?;
    if done.saturating_sub(start) != size
        || !state.matches(&file.metadata()?)
        || path_identity(path)? != identity
    {
        return Err(changed());
    }
    Ok((hasher.finalize().to_hex().to_string(), links))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoProgress;

    #[test]
    fn duplicates_distinguish_hard_links_from_independent_copies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("scan");
        std::fs::create_dir(&root).unwrap();
        let first = root.join("a");
        let copy = root.join("copy");
        std::fs::write(&first, b"same").unwrap();
        std::fs::hard_link(&first, root.join("alias")).unwrap();
        let scan = || {
            find_duplicates(
                std::slice::from_ref(&root),
                &[],
                0,
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap()
        };

        let aliases_only = scan();
        assert_eq!(aliases_only.files_scanned, 2);
        assert_eq!(aliases_only.candidate_files, 0);
        assert_eq!(aliases_only.hashed_bytes, 0);
        assert!(aliases_only.groups.is_empty());

        std::fs::write(&copy, b"same").unwrap();
        let duplicates = scan();
        assert_eq!(duplicates.files_scanned, 3);
        assert_eq!(duplicates.candidate_files, 2);
        assert_eq!(duplicates.hashed_bytes, 8);
        assert_eq!(duplicates.duplicate_files(), 2);
        assert_eq!(duplicates.groups[0].paths, [first, copy.clone()]);
        assert_eq!(duplicates.reclaimable_bytes(), 4);

        std::fs::hard_link(copy, dir.path().join("outside")).unwrap();
        let linked_copies = scan();
        assert_eq!(linked_copies.groups[0].count(), 2);
        assert_eq!(linked_copies.reclaimable_bytes(), 0);
    }
}
