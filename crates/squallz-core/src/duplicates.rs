//! Duplicate-file detection over local inputs.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use crate::api::{EntryType, FormatError};
use crate::{inputs, PathFilter};

const HASH_BUFFER_SIZE: usize = 128 * 1024;

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
) -> Result<DuplicateScanReport, FormatError> {
    let filter = PathFilter::new(excludes)?;
    let items = inputs::collect_inputs(inputs, &filter)?;
    let entries_scanned = items.len();
    let mut files_by_size: BTreeMap<u64, Vec<PathBuf>> = BTreeMap::new();
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0u64;

    for item in items {
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
    let mut groups_by_hash: BTreeMap<(u64, String), Vec<PathBuf>> = BTreeMap::new();

    for (size, mut paths) in files_by_size {
        if paths.len() < 2 {
            continue;
        }
        paths.sort();
        report.candidate_files += paths.len();
        report.hashed_bytes = report
            .hashed_bytes
            .saturating_add(size.saturating_mul(paths.len() as u64));

        for path in paths {
            let hash = hash_file(&path)?;
            groups_by_hash.entry((size, hash)).or_default().push(path);
        }
    }

    report.groups = groups_by_hash
        .into_iter()
        .filter_map(|((size, hash), mut paths)| {
            if paths.len() < 2 {
                return None;
            }
            paths.sort();
            Some(DuplicateGroup { hash, size, paths })
        })
        .collect();
    report.groups.sort_by(|a, b| {
        b.reclaimable_bytes()
            .cmp(&a.reclaimable_bytes())
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.hash.cmp(&b.hash))
    });
    Ok(report)
}

fn hash_file(path: &PathBuf) -> Result<String, FormatError> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
