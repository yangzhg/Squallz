//! Duplicate-file detection over local inputs.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use crate::api::{ControlToken, EntryPath, EntryType, FormatError, ProgressSink};
use crate::{inputs, PathFilter};

const HASH_BUFFER_SIZE: usize = 128 * 1024;
const HASH_PROGRESS_STEP_BYTES: u64 = 1024 * 1024;

/// One duplicate set. All paths have the same byte length and BLAKE3 digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub paths: Vec<PathBuf>,
}

impl DuplicateGroup {
    pub fn count(&self) -> usize {
        self.paths.len()
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        self.size
            .saturating_mul(self.paths.len().saturating_sub(1) as u64)
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
    for (size, paths) in &files_by_size {
        ctl.checkpoint()?;
        if paths.len() >= 2 {
            report.candidate_files += paths.len();
            report.hashed_bytes = report
                .hashed_bytes
                .saturating_add(size.saturating_mul(paths.len() as u64));
        }
    }
    let mut groups_by_hash: BTreeMap<(u64, String), Vec<PathBuf>> = BTreeMap::new();
    let mut done = 0;

    for (size, mut paths) in files_by_size {
        ctl.checkpoint()?;
        if paths.len() < 2 {
            continue;
        }
        paths.sort();
        ctl.checkpoint()?;
        for path in paths {
            let hash = hash_file(&path, progress, &mut done, report.hashed_bytes, ctl)?;
            groups_by_hash.entry((size, hash)).or_default().push(path);
        }
    }

    for ((size, hash), mut paths) in groups_by_hash {
        ctl.checkpoint()?;
        if paths.len() >= 2 {
            paths.sort();
            ctl.checkpoint()?;
            report.groups.push(DuplicateGroup { hash, size, paths });
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
    path: &PathBuf,
    progress: &dyn ProgressSink,
    done: &mut u64,
    total: u64,
    ctl: &ControlToken,
) -> Result<String, FormatError> {
    ctl.checkpoint()?;
    let mut file = File::open(path)?;
    let current = EntryPath::from_utf8(path.display().to_string());
    progress.on_progress(*done, total, &current);
    ctl.checkpoint()?;
    let mut last_reported = *done;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    loop {
        ctl.checkpoint()?;
        let read = file.read(&mut buf)?;
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
    Ok(hasher.finalize().to_hex().to_string())
}
