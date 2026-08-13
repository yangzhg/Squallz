//! Split-volume support (`x.zip.001` byte-split semantics, 7-Zip style):
//! volume-set discovery, a `Read + Seek` view over the concatenated
//! volumes, and the create-side splitter.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::api::NoProgress;
use crate::api::{
    split_volume_name, ArchiveFormat, ControlToken, EntryPath, FormatError, NativeVolumeWriter,
    ProgressPhase, ProgressSink, ResourceOptions,
};
#[cfg(test)]
use crate::destination_guard::verify_destination_guard;
use crate::destination_guard::{path_state_digest, verify_destination_guard_binding};
#[cfg(windows)]
use crate::filesystem_identity::path_change_time;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow_read_write, path_identity, PathIdentity,
    RegularFileState,
};
use crate::stored_os_string::StoredOsString;
use crate::{
    parent_or_current, sync_directory, CreateArtifactKind, CreateCommitPolicy,
    CreateDestinationGuard,
};

/// Split sizes below this are rejected (pathological volume counts).
pub(crate) const MIN_SPLIT_SIZE: u64 = 1024;
/// Hard resource boundary shared by split creation and SQZV discovery.
const MAX_SPLIT_VOLUME_COUNT: u64 = 1_000_000;
/// Extra free bytes required beyond the exact estimate.
const SPACE_SLACK: u64 = 1024 * 1024;
/// Default copy chunk for the splitter.
const COPY_CHUNK: usize = 64 * 1024;
const SQZ_MAGIC: &[u8; 8] = b"SQZARCH\x1A";
const SQZ_HEADER_LEN: usize = 64;
const SQZ_HEADER_FLAG_SPLIT: u32 = 1 << 3;
const SQZV_MAGIC: &[u8; 4] = b"SQZV";
const SQZV_HEADER_LEN: usize = 32;
const SQZV_HEADER_LEN_U64: u64 = SQZV_HEADER_LEN as u64;
const SQZR_MAGIC: &[u8; 4] = b"SQZR";
const SQZR_HEADER_LEN: usize = 64;
const SQZR_HEADER_LEN_U64: u64 = SQZR_HEADER_LEN as u64;
const SQZR_VERSION: u16 = 1;
const SQZR_ALGO_XOR_SINGLE: u16 = 1;
const SQZR_ALGO_XOR_WEIGHTED: u16 = 2;
const SQZR_ALGO_XOR_QUADRATIC: u16 = 3;
static SPLIT_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SPLIT_JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const SPLIT_TRANSACTION_VERSION: u32 = 1;
const SPLIT_TRANSACTION_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SplitOutputBudget {
    pub final_output_bytes: u64,
    pub additional_space_bytes: u64,
    pub volume_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitArtifacts {
    pub volumes: Vec<PathBuf>,
    pub primary_volume_index: usize,
    pub sidecars: Vec<PathBuf>,
    pub preserved_outputs: Vec<PathBuf>,
    pub total_output_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct SplitLayout {
    logical_volume_size: u64,
    count: u64,
    write_sqzv: bool,
    write_weighted_parity: bool,
    write_quadratic_parity: bool,
}

fn split_layout(base: &Path, total: u64, volume_size: u64) -> Result<SplitLayout, FormatError> {
    if volume_size < MIN_SPLIT_SIZE {
        return Err(FormatError::Unsupported(format!(
            "split size below the {MIN_SPLIT_SIZE}-byte minimum: {volume_size}"
        )));
    }
    let write_sqzv = is_sqz_base(base);
    let logical_volume_size = if write_sqzv {
        if volume_size <= SQZV_HEADER_LEN_U64 {
            return Err(FormatError::Unsupported(format!(
                "split size must leave room for the {SQZV_HEADER_LEN_U64}-byte SQZV header: {volume_size}"
            )));
        }
        volume_size - SQZV_HEADER_LEN_U64
    } else {
        volume_size
    };
    let count = total.div_ceil(logical_volume_size).max(1);
    if count > MAX_SPLIT_VOLUME_COUNT || usize::try_from(count).is_err() {
        return Err(FormatError::ResourceLimitExceeded(
            "split archive would create too many volumes".into(),
        ));
    }
    Ok(SplitLayout {
        logical_volume_size,
        count,
        write_sqzv,
        write_weighted_parity: write_sqzv && count > 2 && count <= u64::from(u8::MAX),
        write_quadratic_parity: write_sqzv && count > 3 && count <= u64::from(u8::MAX),
    })
}

pub(crate) fn split_output_budget(
    base: &Path,
    total: u64,
    volume_size: u64,
) -> Result<SplitOutputBudget, FormatError> {
    let layout = split_layout(base, total, volume_size)?;
    let mut final_output_bytes = split_final_output_bytes(total, volume_size, layout);
    if layout.write_sqzv && layout.count > u64::from(u8::MAX) {
        let last_full_recovery_set = layout
            .logical_volume_size
            .saturating_mul(u64::from(u8::MAX))
            .min(total);
        let recovery_layout = split_layout(base, last_full_recovery_set, volume_size)?;
        final_output_bytes = final_output_bytes.max(split_final_output_bytes(
            last_full_recovery_set,
            volume_size,
            recovery_layout,
        ));
    }
    Ok(SplitOutputBudget {
        final_output_bytes,
        additional_space_bytes: final_output_bytes.saturating_add(SPACE_SLACK),
        volume_count: layout.count,
    })
}

pub(crate) fn native_split_output_budget(
    format: &dyn ArchiveFormat,
    archive_bytes: u64,
    entry_count: u64,
    volume_size: u64,
) -> Result<SplitOutputBudget, FormatError> {
    let estimate = format.native_volume_budget(archive_bytes, entry_count, volume_size)?;
    Ok(SplitOutputBudget {
        final_output_bytes: estimate.output_bytes,
        additional_space_bytes: estimate.output_bytes.saturating_add(SPACE_SLACK),
        volume_count: estimate.volume_count,
    })
}

fn split_final_output_bytes(total: u64, volume_size: u64, layout: SplitLayout) -> u64 {
    let mut final_output_bytes = total;
    if layout.write_sqzv {
        final_output_bytes =
            final_output_bytes.saturating_add(layout.count.saturating_mul(SQZV_HEADER_LEN_U64));
        if layout.count > 1 {
            final_output_bytes = final_output_bytes
                .saturating_add(volume_size)
                .saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            if layout.write_weighted_parity {
                final_output_bytes = final_output_bytes
                    .saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            }
            if layout.write_quadratic_parity {
                final_output_bytes = final_output_bytes
                    .saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            }
        }
    }
    final_output_bytes
}

fn fixed_field<const N: usize>(
    bytes: &[u8],
    range: Range<usize>,
    field: &str,
) -> Result<[u8; N], FormatError> {
    let start = range.start;
    let end = range.end;
    let slice = bytes.get(range).ok_or_else(|| {
        FormatError::CorruptArchive(format!("truncated {field}: expected bytes {start}..{end}"))
    })?;
    if slice.len() != N {
        return Err(FormatError::CorruptArchive(format!(
            "invalid {field} width: expected {N} bytes, got {}",
            slice.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

fn le_u16(bytes: &[u8], range: Range<usize>, field: &str) -> Result<u16, FormatError> {
    Ok(u16::from_le_bytes(fixed_field(bytes, range, field)?))
}

fn le_u32(bytes: &[u8], range: Range<usize>, field: &str) -> Result<u32, FormatError> {
    Ok(u32::from_le_bytes(fixed_field(bytes, range, field)?))
}

fn le_u64(bytes: &[u8], range: Range<usize>, field: &str) -> Result<u64, FormatError> {
    Ok(u64::from_le_bytes(fixed_field(bytes, range, field)?))
}

fn filename_or_empty(path: &Path) -> String {
    let mut name = String::new();
    if let Some(file_name) = path.file_name() {
        name = file_name.to_string_lossy().into_owned();
    }
    name
}

#[cfg(test)]
fn part_path(path: &Path) -> PathBuf {
    let name = filename_or_empty(path);
    path.with_file_name(format!("{name}.part"))
}

#[derive(Debug, Clone, Copy)]
struct SplitStagingId(u64);

impl SplitStagingId {
    fn new() -> Self {
        Self(SPLIT_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug)]
struct SplitStagingPath {
    path: PathBuf,
    identity: SplitPathIdentity,
    file: File,
}

fn reserve_split_staging_file(
    final_path: &Path,
    staging_id: SplitStagingId,
) -> Result<(SplitStagingPath, File), FormatError> {
    let name = final_path
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("split output path has no file name".into()))?;
    let parent = parent_or_current(final_path);
    for attempt in 0..1000u32 {
        let mut candidate_name = OsString::from(".");
        candidate_name.push(name);
        candidate_name.push(format!(
            ".split-stage-{}-{}-{attempt}.tmp.",
            std::process::id(),
            staging_id.0
        ));
        candidate_name.push(name);
        let candidate = parent.join(candidate_name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            };

            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        match options.open(&candidate) {
            Ok(file) => {
                let identity = split_file_identity(&file)?;
                if split_path_identity(&candidate).ok() != Some(identity) {
                    return Err(FormatError::Io(io::Error::other(format!(
                        "split staging changed while it was reserved and was left untouched: {}",
                        candidate.display()
                    ))));
                }
                return Ok((
                    SplitStagingPath {
                        path: candidate,
                        identity,
                        file: file.try_clone()?,
                    },
                    file,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not reserve split staging output next to {}",
        final_path.display()
    )))
}

fn split_staging_output_name(name: &str) -> Option<&str> {
    let name = name.strip_prefix('.')?;
    let (output_name, staging_suffix) = name.rsplit_once(".split-stage-")?;
    let (identity, echoed_output_name) = staging_suffix.split_once(".tmp.")?;
    let mut identity_parts = identity.split('-');
    let valid_identity = (0..3).all(|_| {
        identity_parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && identity_parts.next().is_none();
    (valid_identity && output_name == echoed_output_name).then_some(output_name)
}

fn split_transaction_output_name<'a>(name: &'a str, purpose: &str) -> Option<&'a str> {
    let name = name.strip_prefix('.')?;
    let marker = format!(".{purpose}-");
    let (output_name, transaction_suffix) = name.rsplit_once(&marker)?;
    let (identity, echoed_output_name) = transaction_suffix.split_once(".tmp.")?;
    let mut identity_parts = identity.split('-');
    let valid_identity = (0..2).all(|_| {
        identity_parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && identity_parts.next().is_none();
    (valid_identity && output_name == echoed_output_name).then_some(output_name)
}

fn managed_split_output_name(base_name: &str, name: &str, include_recovery: bool) -> bool {
    if name == base_name {
        return true;
    }
    if split_volume_name(name).is_some_and(|(candidate, _)| candidate == base_name) {
        return true;
    }
    if native_zip_volume_name(base_name, name) {
        return true;
    }
    if native_wim_volume_number(base_name, name).is_some() {
        return true;
    }
    include_recovery
        && sqz_recovery_suffix(name).is_some_and(|(suffix, _)| {
            name.strip_suffix(suffix)
                .is_some_and(|candidate| candidate == base_name)
        })
}

pub(crate) fn expected_managed_split_output_path(
    base: &Path,
    name: &str,
    include_recovery: bool,
) -> Option<PathBuf> {
    let base_name = base.file_name()?.to_str()?;
    if name == base_name {
        return Some(base.to_path_buf());
    }
    if split_volume_name(name).is_some() {
        let suffix = name.rsplit_once('.')?.1;
        return Some(base.with_file_name(format!("{base_name}.{suffix}")));
    }
    if native_zip_volume_name(base_name, name) {
        let extension = Path::new(name).extension()?;
        return Some(base.with_extension(extension));
    }
    if let Some(number) = native_wim_volume_number(base_name, name) {
        let stem = base.file_stem()?.to_str()?;
        let extension = base.extension()?.to_str()?;
        return Some(base.with_file_name(format!("{stem}{number}.{extension}")));
    }
    if include_recovery {
        let (suffix, _) = sqz_recovery_suffix(name)?;
        return Some(base.with_file_name(format!("{base_name}{suffix}")));
    }
    None
}

fn native_zip_volume_name(base_name: &str, name: &str) -> bool {
    let base = Path::new(base_name);
    if !base
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        return false;
    }
    let candidate = Path::new(name);
    if candidate.file_stem() != base.file_stem() {
        return false;
    }
    let Some(extension) = candidate
        .extension()
        .and_then(|extension| extension.to_str())
    else {
        return false;
    };
    let bytes = extension.as_bytes();
    bytes.len() >= 3
        && bytes[0].eq_ignore_ascii_case(&b'z')
        && bytes[1..].iter().all(u8::is_ascii_digit)
        && extension[1..].parse::<u64>().is_ok_and(|number| number > 0)
}

fn native_wim_volume_number(base_name: &str, name: &str) -> Option<u32> {
    let base = Path::new(base_name);
    if !base
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("swm"))
    {
        return None;
    }
    let candidate = Path::new(name);
    if !candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("swm"))
    {
        return None;
    }
    let base_stem = base.file_stem()?.to_str()?;
    let suffix = candidate.file_stem()?.to_str()?.strip_prefix(base_stem)?;
    if suffix.is_empty() || (suffix.len() > 1 && suffix.starts_with('0')) {
        return None;
    }
    suffix
        .parse::<u32>()
        .ok()
        .filter(|number| (2..=u32::from(u16::MAX)).contains(number))
}

fn split_staging_matches_final(base_name: &str, staged_name: &str, final_name: &str) -> bool {
    split_staging_output_name(staged_name).is_some_and(|staged_output| {
        staged_output == final_name
            || (final_name == base_name && native_zip_volume_name(base_name, staged_output))
    })
}

fn transaction_backup_matches_output_family(
    base: &Path,
    original_name: &str,
    backup_name: &str,
    identity: SplitPathIdentity,
    include_recovery: bool,
) -> bool {
    let Some(base_name) = base.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if managed_split_output_name(base_name, original_name, include_recovery) {
        return true;
    }
    let Some(expected_original) =
        expected_managed_split_output_path(base, original_name, include_recovery)
    else {
        return false;
    };
    let parent = parent_or_current(base);
    let original = parent.join(original_name);
    if split_path_identity(&original).is_ok_and(|observed| observed == identity)
        && crate::same_path_entry(&expected_original, &original)
    {
        return true;
    }

    let Some(without_dot) = backup_name.strip_prefix('.') else {
        return false;
    };
    let Some((_output, suffix)) = without_dot.rsplit_once(".split-backup-") else {
        return false;
    };
    let Some((transaction_identity, _echoed_output)) = suffix.split_once(".tmp.") else {
        return false;
    };
    let Some(expected_name) = expected_original.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let expected_backup = parent.join(format!(
        ".{expected_name}.split-backup-{transaction_identity}.tmp.{expected_name}"
    ));
    let backup = parent.join(backup_name);
    split_path_identity(&backup).is_ok_and(|observed| observed == identity)
        && crate::same_path_entry(&expected_backup, &backup)
}

pub(crate) fn matches_split_staging_path(base: &Path, path: &Path, include_recovery: bool) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(output_name) = split_staging_output_name(name) else {
        return false;
    };
    managed_split_output_name(&filename_or_empty(base), output_name, include_recovery)
}

pub(crate) fn matches_split_complete_staging_path(base: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(output_name) = split_transaction_output_name(name, "split") else {
        return false;
    };
    output_name == filename_or_empty(base)
        && crate::same_path_entry(parent_or_current(base), parent_or_current(path))
}

pub(crate) fn matches_split_transaction_path(
    base: &Path,
    path: &Path,
    include_recovery: bool,
) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if let Some(isolated_name) = split_transaction_output_name(name, "split-cleanup") {
        if split_staging_output_name(isolated_name).is_some_and(|output_name| {
            managed_split_output_name(&filename_or_empty(base), output_name, include_recovery)
        }) || matches_split_complete_staging_path(base, Path::new(isolated_name))
        {
            return true;
        }
    }
    ["split-backup", "split-rollback-preserved"]
        .into_iter()
        .filter_map(|purpose| split_transaction_output_name(name, purpose))
        .any(|output_name| {
            managed_split_output_name(&filename_or_empty(base), output_name, include_recovery)
        })
}

/// Formats the volume suffix (7-Zip convention: three digits minimum).
fn volume_path(base: &Path, index: u64) -> PathBuf {
    let name = filename_or_empty(base);
    base.with_file_name(format!("{name}.{index:03}"))
}

pub(crate) fn first_volume_path(base: &Path) -> PathBuf {
    volume_path(base, 1)
}

/// Optional SQZ tail mirror sidecar. It stores a normal SQZV volume image for
/// the tail, so the existing volume reader can validate and consume it.
fn recovery_volume_path(base: &Path, index: u64) -> PathBuf {
    let name = filename_or_empty(base);
    base.with_file_name(format!("{name}.rev{index:03}"))
}

pub(crate) fn sqz_recovery_suffix(name: &str) -> Option<(&str, u64)> {
    let marker = name.rfind('.')?;
    let suffix = &name[marker..];
    let prefix = suffix.get(..4)?;
    let digits = suffix.get(4..)?;
    if !prefix.eq_ignore_ascii_case(".rev")
        || digits.len() < 3
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let index = digits.parse::<u64>().ok()?;
    (index > 0).then_some((suffix, index))
}

/// First SQZ external recovery volume. This stores XOR parity across all
/// physical SQZV volumes and can reconstruct one missing volume.
fn recovery_parity_volume_path(base: &Path) -> PathBuf {
    recovery_volume_path(base, 1)
}

/// Second SQZ external recovery volume. It stores GF(256)-weighted parity
/// across all physical SQZV volumes and can combine with `.rev001` to recover
/// two missing physical volumes when the split set has <= 255 volumes.
fn recovery_weighted_parity_volume_path(base: &Path) -> PathBuf {
    recovery_volume_path(base, 2)
}

/// Third SQZ external recovery volume. It stores GF(256)-weighted parity
/// using the squared volume index as coefficient and can combine with
/// `.rev001/.rev002` to recover three missing physical volumes.
fn recovery_quadratic_parity_volume_path(base: &Path) -> PathBuf {
    recovery_volume_path(base, 3)
}

/// Returns the volume base (`x.zip.003` → `x.zip`) when `path` names a
/// split volume, `None` otherwise.
pub(crate) fn volume_base(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let (base, _) = split_volume_name(name)?;
    Some(path.with_file_name(base))
}

/// Collects the complete, gap-free volume set for `volume` (any volume of
/// the set). A missing volume yields [`FormatError::CorruptArchive`] whose
/// detail names the first missing volume path.
pub fn collect_volume_set(volume: &Path) -> Result<VolumeSet, FormatError> {
    collect_volume_set_with_checkpoint(volume, || Ok(()))
}

/// Controlled variant of [`collect_volume_set`].
pub fn collect_volume_set_with_control(
    volume: &Path,
    control: &ControlToken,
) -> Result<VolumeSet, FormatError> {
    collect_volume_set_with_checkpoint(volume, || control.checkpoint())
}

fn collect_volume_set_with_checkpoint<C>(
    volume: &Path,
    mut checkpoint: C,
) -> Result<VolumeSet, FormatError>
where
    C: FnMut() -> Result<(), FormatError>,
{
    checkpoint()?;
    let selected_index = volume
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(split_volume_name)
        .map(|(_, index)| u64::from(index))
        .ok_or_else(|| {
            FormatError::Unsupported(format!("not a split volume: {}", volume.display()))
        })?;
    let base = volume_base(volume).ok_or_else(|| {
        FormatError::Unsupported(format!("not a split volume: {}", volume.display()))
    })?;
    let base_name = filename_or_empty(&base);
    // Highest index present on disk for this base.
    let mut present = BTreeMap::new();
    if let Ok(mut entries) = fs::read_dir(parent_or_current(&base)) {
        loop {
            checkpoint()?;
            let Some(entry) = entries.next() else {
                break;
            };
            let Ok(entry) = entry else {
                continue;
            };
            if let Some(name) = entry.file_name().to_str() {
                if let Some((b, idx)) = split_volume_name(name) {
                    if b == base_name {
                        present.insert(u64::from(idx), entry.path());
                    }
                }
            }
        }
    }
    checkpoint()?;
    let max_index = present.keys().next_back().copied().unwrap_or(0);
    if max_index == 0 {
        return Err(FormatError::missing_volume(volume_path(&base, 1)));
    }

    if is_sqz_base(&base) {
        if let Some(set) = collect_sqzv_volume_set(&base, &present, &mut checkpoint)? {
            return Ok(set);
        }
    }

    let expected_upper_bound = max_index.max(selected_index);
    let mut parts = Vec::with_capacity(present.len());
    let mut expected_index = 1u64;
    for (index, part) in present {
        checkpoint()?;
        if index != expected_index {
            return Err(FormatError::missing_volume(volume_path(
                &base,
                expected_index,
            )));
        }
        if !part.is_file() {
            return Err(FormatError::missing_volume(&part));
        }
        checkpoint()?;
        let logical_len = fs::metadata(&part)?.len();
        checkpoint()?;
        parts.push(VolumePart {
            path: part,
            data_offset: 0,
            logical_len,
            source: VolumePartSource::File,
        });
        expected_index += 1;
    }
    if expected_index <= expected_upper_bound {
        return Err(FormatError::missing_volume(volume_path(
            &base,
            expected_index,
        )));
    }
    checkpoint()?;
    Ok(VolumeSet { parts })
}

#[derive(Clone, Debug)]
pub struct VolumeSet {
    parts: Vec<VolumePart>,
}

impl VolumeSet {
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &PathBuf> {
        self.parts.iter().map(|part| &part.path)
    }

    fn parts(&self) -> &[VolumePart] {
        &self.parts
    }
}

#[derive(Clone, Debug)]
struct VolumePart {
    path: PathBuf,
    data_offset: u64,
    logical_len: u64,
    source: VolumePartSource,
}

#[derive(Clone, Debug)]
enum VolumePartSource {
    File,
    MissingZero,
    Reconstructed {
        source: ReconstructedSource,
        peers: Vec<PeerVolume>,
    },
}

#[derive(Clone, Debug)]
enum ReconstructedSource {
    SingleXor {
        recovery_path: PathBuf,
    },
    DualWeighted {
        xor_path: PathBuf,
        weighted_path: PathBuf,
        target_coeff: u8,
        other_coeff: u8,
    },
    TripleWeighted {
        xor_path: PathBuf,
        weighted_path: PathBuf,
        quadratic_path: PathBuf,
        target_coeff: u8,
        other_coeffs: [u8; 2],
    },
}

#[derive(Clone, Debug)]
struct PeerVolume {
    index: u64,
    path: PathBuf,
    physical_len: u64,
}

fn collect_sqzv_volume_set<C>(
    base: &Path,
    present: &BTreeMap<u64, PathBuf>,
    checkpoint: &mut C,
) -> Result<Option<VolumeSet>, FormatError>
where
    C: FnMut() -> Result<(), FormatError>,
{
    checkpoint()?;
    let mut headers = HashMap::new();
    let mut total = None;
    let mut uuid = None;
    for (index, path) in present {
        checkpoint()?;
        let mut file = File::open(path)?;
        let header = read_sqzv_header(&mut file)?;
        checkpoint()?;
        let Some(header) = header else {
            continue;
        };
        if u64::from(header.index) != *index {
            return Err(FormatError::CorruptArchive(format!(
                "SQZV volume header mismatch: index {} in {}",
                header.index,
                path.display()
            )));
        }
        if let Some(total) = total {
            if total != header.total {
                return Err(FormatError::CorruptArchive(
                    "SQZV volume total mismatch".into(),
                ));
            }
        } else {
            total = Some(header.total);
        }
        if let Some(uuid) = uuid {
            if uuid != header.uuid() {
                return Err(FormatError::CorruptArchive(
                    "SQZV volume UUID mismatch".into(),
                ));
            }
        } else {
            uuid = Some(header.uuid());
        }
        headers.insert(*index, header);
    }
    let Some(total) = total else {
        return Ok(None);
    };
    let Some(uuid) = uuid else {
        return Err(FormatError::CorruptArchive(
            "SQZV volume UUID missing".into(),
        ));
    };
    let total_u64 = u64::from(total);
    if total_u64 == 0 {
        return Err(FormatError::CorruptArchive(
            "SQZV volume count must be greater than zero".into(),
        ));
    }
    if total_u64 > MAX_SPLIT_VOLUME_COUNT {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "SQZV volume count {total_u64} exceeds the {MAX_SPLIT_VOLUME_COUNT}-volume limit"
        )));
    }
    for index in headers.keys() {
        checkpoint()?;
        if *index > total_u64 {
            return Err(FormatError::CorruptArchive(
                "SQZV volume index exceeds the declared volume count".into(),
            ));
        }
    }
    let mut raw_missing = Vec::new();
    for index in 1..=total_u64 {
        checkpoint()?;
        if !headers.contains_key(&index) {
            raw_missing.push(index);
        }
    }
    let tail_mirror = if raw_missing.contains(&total_u64) {
        let tail_mirror = sqzv_recovery_volume_part(base, total_u64, total, uuid)?;
        checkpoint()?;
        tail_mirror
    } else {
        None
    };
    let mut reconstructable_missing = Vec::new();
    for index in raw_missing.iter().copied() {
        checkpoint()?;
        if !(index == total_u64 && tail_mirror.is_some()) {
            reconstructable_missing.push(index);
        }
    }
    let single_parity = if !reconstructable_missing.is_empty() {
        let parity = read_sqzr_header(base, 1, SQZR_ALGO_XOR_SINGLE, total, uuid)?;
        checkpoint()?;
        parity
    } else {
        None
    };
    let dual_parity = if reconstructable_missing.len() >= 2
        && reconstructable_missing.len() <= 3
        && total_u64 <= u64::from(u8::MAX)
    {
        let parity = read_sqzr_header(base, 2, SQZR_ALGO_XOR_WEIGHTED, total, uuid)?;
        checkpoint()?;
        parity
    } else {
        None
    };
    let triple_parity = if reconstructable_missing.len() == 3 && total_u64 <= u64::from(u8::MAX) {
        let parity = read_sqzr_header(base, 3, SQZR_ALGO_XOR_QUADRATIC, total, uuid)?;
        checkpoint()?;
        parity
    } else {
        None
    };
    let mut full_logical_len = None;
    for (index, path) in present {
        checkpoint()?;
        if *index >= total_u64 {
            continue;
        }
        let Some(logical_len) = fs::metadata(path)
            .ok()
            .and_then(|meta| meta.len().checked_sub(SQZV_HEADER_LEN_U64))
        else {
            continue;
        };
        full_logical_len =
            Some(full_logical_len.map_or(logical_len, |current: u64| current.max(logical_len)));
    }

    let mut parts = Vec::with_capacity(total as usize);
    for index in 1..=total_u64 {
        checkpoint()?;
        let path = volume_path(base, index);
        if let Some(header) = headers.get(&index) {
            validate_sqzv_header(header, index as u32, total)?;
            let physical_len = fs::metadata(&path)?.len();
            checkpoint()?;
            if physical_len < SQZV_HEADER_LEN_U64 {
                return Err(FormatError::CorruptArchive(format!(
                    "truncated SQZV volume: {}",
                    path.display()
                )));
            }
            parts.push(VolumePart {
                path,
                data_offset: SQZV_HEADER_LEN_U64,
                logical_len: physical_len - SQZV_HEADER_LEN_U64,
                source: VolumePartSource::File,
            });
        } else {
            if index == total_u64 {
                if let Some(part) = tail_mirror.clone() {
                    parts.push(part);
                    continue;
                }
            }
            if let Some(reconstructed) = reconstruct_sqzv_part(
                base,
                index,
                total_u64,
                &reconstructable_missing,
                single_parity.as_ref(),
                dual_parity.as_ref(),
                triple_parity.as_ref(),
                tail_mirror.as_ref(),
                checkpoint,
            )? {
                parts.push(reconstructed);
            } else {
                if index == total_u64 {
                    return Err(FormatError::CorruptArchive(format!(
                        "missing SQZV tail volume: {}",
                        path.display()
                    )));
                }
                let full_logical_len = full_logical_len.ok_or_else(|| {
                    FormatError::CorruptArchive(
                        "cannot infer missing SQZV volume size from only the tail volume".into(),
                    )
                })?;
                parts.push(VolumePart {
                    path,
                    data_offset: SQZV_HEADER_LEN_U64,
                    logical_len: full_logical_len,
                    source: VolumePartSource::MissingZero,
                });
            }
        }
    }
    checkpoint()?;
    Ok(Some(VolumeSet { parts }))
}

fn sqzv_recovery_volume_part(
    base: &Path,
    index: u64,
    total: u32,
    uuid: (u64, u64),
) -> Result<Option<VolumePart>, FormatError> {
    let path = recovery_volume_path(base, index);
    if !path.is_file() {
        return Ok(None);
    }
    let mut file = File::open(&path)?;
    let Some(header) = read_sqzv_header(&mut file)? else {
        return Err(FormatError::CorruptArchive(format!(
            "missing SQZV recovery volume header: {}",
            path.display()
        )));
    };
    validate_sqzv_header(&header, index as u32, total)?;
    if header.uuid() != uuid {
        return Err(FormatError::CorruptArchive(
            "SQZV recovery volume UUID mismatch".into(),
        ));
    }
    let physical_len = file.metadata()?.len();
    if physical_len < SQZV_HEADER_LEN_U64 {
        return Err(FormatError::CorruptArchive(format!(
            "truncated SQZV recovery volume: {}",
            path.display()
        )));
    }
    Ok(Some(VolumePart {
        path,
        data_offset: SQZV_HEADER_LEN_U64,
        logical_len: physical_len - SQZV_HEADER_LEN_U64,
        source: VolumePartSource::File,
    }))
}

#[derive(Clone, Copy, Debug)]
struct SqzrHeader {
    total: u32,
    uuid_hi: u64,
    uuid_lo: u64,
    physical_volume_size: u64,
    tail_physical_len: u64,
    parity_len: u64,
}

impl SqzrHeader {
    fn physical_len_for(&self, index: u64, total: u64) -> Result<u64, FormatError> {
        let len = if index == total {
            self.tail_physical_len
        } else {
            self.physical_volume_size
        };
        if len < SQZV_HEADER_LEN_U64 || len > self.parity_len {
            return Err(FormatError::CorruptArchive(format!(
                "invalid SQZ recovery volume length for index {index}"
            )));
        }
        Ok(len)
    }
}

fn read_sqzr_header(
    base: &Path,
    recovery_index: u64,
    expected_algorithm: u16,
    expected_total: u32,
    expected_uuid: (u64, u64),
) -> Result<Option<SqzrHeader>, FormatError> {
    let path = recovery_volume_path(base, recovery_index);
    if !path.is_file() {
        return Ok(None);
    }
    let mut file = File::open(&path)?;
    let mut header = [0u8; SQZR_HEADER_LEN];
    file.read_exact(&mut header)?;
    if header.get(0..4) != Some(SQZR_MAGIC.as_slice()) {
        return Ok(None);
    }
    let expected = le_u32(&header, 52..56, "SQZR header CRC")?;
    let actual = crc32c::crc32c(&header[..52]);
    if expected != actual {
        return Err(FormatError::CorruptArchive(
            "SQZ recovery volume header CRC-32C mismatch".into(),
        ));
    }
    let version = le_u16(&header, 4..6, "SQZR version")?;
    let algorithm = le_u16(&header, 6..8, "SQZR algorithm")?;
    if version != SQZR_VERSION || algorithm != expected_algorithm {
        return Err(FormatError::Unsupported(
            "unsupported SQZ recovery volume version or algorithm".into(),
        ));
    }
    let parsed = SqzrHeader {
        total: le_u32(&header, 8..12, "SQZR total")?,
        uuid_hi: le_u64(&header, 12..20, "SQZR UUID high")?,
        uuid_lo: le_u64(&header, 20..28, "SQZR UUID low")?,
        physical_volume_size: le_u64(&header, 28..36, "SQZR physical volume size")?,
        tail_physical_len: le_u64(&header, 36..44, "SQZR tail physical length")?,
        parity_len: le_u64(&header, 44..52, "SQZR parity length")?,
    };
    if parsed.total != expected_total || parsed.uuid() != expected_uuid {
        return Err(FormatError::CorruptArchive(
            "SQZ recovery volume identity mismatch".into(),
        ));
    }
    let physical_len = file.metadata()?.len();
    if physical_len != SQZR_HEADER_LEN_U64 + parsed.parity_len {
        return Err(FormatError::CorruptArchive(format!(
            "truncated SQZ recovery volume: {}",
            path.display()
        )));
    }
    Ok(Some(parsed))
}

impl SqzrHeader {
    fn uuid(&self) -> (u64, u64) {
        (self.uuid_hi, self.uuid_lo)
    }
}

fn sqzr_weighted_coeff(index: u64) -> Result<u8, FormatError> {
    if index == 0 || index > u64::from(u8::MAX) {
        return Err(FormatError::Unsupported(
            "SQZ split recovery currently supports at most 255 volumes".into(),
        ));
    }
    Ok(index as u8)
}

fn sqzr_quadratic_coeff(index: u64) -> Result<u8, FormatError> {
    let coeff = sqzr_weighted_coeff(index)?;
    Ok(gf256_mul(coeff, coeff))
}

fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= 0x1D;
        }
        b >>= 1;
    }
    product
}

fn gf256_pow(mut value: u8, mut exponent: u16) -> u8 {
    let mut result = 1u8;
    while exponent != 0 {
        if exponent & 1 != 0 {
            result = gf256_mul(result, value);
        }
        value = gf256_mul(value, value);
        exponent >>= 1;
    }
    result
}

fn gf256_inv(value: u8) -> Option<u8> {
    (value != 0).then(|| gf256_pow(value, 254))
}

#[allow(clippy::too_many_arguments)] // recovery math needs the three parity layers and tail mirror together
fn reconstruct_sqzv_part<C>(
    base: &Path,
    index: u64,
    total: u64,
    missing: &[u64],
    single_parity: Option<&SqzrHeader>,
    dual_parity: Option<&SqzrHeader>,
    triple_parity: Option<&SqzrHeader>,
    tail_mirror: Option<&VolumePart>,
    checkpoint: &mut C,
) -> Result<Option<VolumePart>, FormatError>
where
    C: FnMut() -> Result<(), FormatError>,
{
    checkpoint()?;
    if !missing.contains(&index) {
        return Ok(None);
    }
    let Some(single_parity) = single_parity else {
        return Ok(None);
    };
    let physical_len = single_parity.physical_len_for(index, total)?;
    let source = if missing.len() == 1 {
        ReconstructedSource::SingleXor {
            recovery_path: recovery_parity_volume_path(base),
        }
    } else if missing.len() == 2 {
        let Some(dual_parity) = dual_parity else {
            return Ok(None);
        };
        let other = missing
            .iter()
            .copied()
            .find(|candidate| *candidate != index)
            .ok_or_else(|| FormatError::CorruptArchive("missing SQZ recovery peer index".into()))?;
        let target_coeff = sqzr_weighted_coeff(index)?;
        let other_coeff = sqzr_weighted_coeff(other)?;
        if dual_parity.physical_len_for(index, total)? != physical_len {
            return Err(FormatError::CorruptArchive(
                "SQZ recovery volume length mismatch".into(),
            ));
        }
        ReconstructedSource::DualWeighted {
            xor_path: recovery_parity_volume_path(base),
            weighted_path: recovery_weighted_parity_volume_path(base),
            target_coeff,
            other_coeff,
        }
    } else if missing.len() == 3 {
        let (Some(dual_parity), Some(triple_parity)) = (dual_parity, triple_parity) else {
            return Ok(None);
        };
        let others: Vec<u64> = missing
            .iter()
            .copied()
            .filter(|candidate| *candidate != index)
            .collect();
        if others.len() != 2 {
            return Err(FormatError::CorruptArchive(
                "missing SQZ recovery peer indices".into(),
            ));
        }
        let target_coeff = sqzr_weighted_coeff(index)?;
        let other_coeffs = [
            sqzr_weighted_coeff(others[0])?,
            sqzr_weighted_coeff(others[1])?,
        ];
        if dual_parity.physical_len_for(index, total)? != physical_len
            || triple_parity.physical_len_for(index, total)? != physical_len
        {
            return Err(FormatError::CorruptArchive(
                "SQZ recovery volume length mismatch".into(),
            ));
        }
        ReconstructedSource::TripleWeighted {
            xor_path: recovery_parity_volume_path(base),
            weighted_path: recovery_weighted_parity_volume_path(base),
            quadratic_path: recovery_quadratic_parity_volume_path(base),
            target_coeff,
            other_coeffs,
        }
    } else {
        return Ok(None);
    };
    let mut peers = Vec::with_capacity(total.saturating_sub(1) as usize);
    for peer_index in 1..=total {
        checkpoint()?;
        if missing.contains(&peer_index) {
            continue;
        }
        let path = volume_path(base, peer_index);
        if path.is_file() {
            peers.push(PeerVolume {
                index: peer_index,
                physical_len: fs::metadata(&path)?.len(),
                path,
            });
            checkpoint()?;
        } else if peer_index == total {
            if let Some(part) = tail_mirror {
                peers.push(PeerVolume {
                    index: peer_index,
                    path: part.path.clone(),
                    physical_len: part.logical_len + part.data_offset,
                });
            } else {
                return Err(FormatError::CorruptArchive(format!(
                    "SQZ recovery volume missing peer {}",
                    path.display()
                )));
            }
        } else {
            return Err(FormatError::CorruptArchive(format!(
                "SQZ recovery volume missing peer {}",
                path.display()
            )));
        }
    }
    checkpoint()?;
    Ok(Some(VolumePart {
        path: volume_path(base, index),
        data_offset: SQZV_HEADER_LEN_U64,
        logical_len: physical_len - SQZV_HEADER_LEN_U64,
        source: VolumePartSource::Reconstructed { source, peers },
    }))
}

/// `Read + Seek` over the concatenation of the volume files.
pub(crate) struct MultiVolumeReader {
    parts: Vec<PartReader>,
    /// Start offset of each volume within the logical stream.
    offsets: Vec<u64>,
    /// Start offset of logical archive bytes inside each physical volume.
    data_offsets: Vec<u64>,
    logical_lens: Vec<u64>,
    total: u64,
    pos: u64,
}

enum PartReader {
    File(File),
    MissingZero,
    Reconstructed(ReconstructedVolumeReader),
}

struct ReconstructedVolumeReader {
    source: ReconstructedReaderSource,
    peers: Vec<PeerReader>,
}

enum ReconstructedReaderSource {
    SingleXor {
        recovery: File,
    },
    DualWeighted {
        xor: File,
        weighted: File,
        other_coeff: u8,
        denominator_inv: u8,
    },
    TripleWeighted {
        xor: File,
        weighted: File,
        quadratic: File,
        other_coeffs: [u8; 2],
        denominator_inv: u8,
    },
}

struct PeerReader {
    coeff: u8,
    quadratic_coeff: u8,
    file: File,
    physical_len: u64,
}

impl ReconstructedVolumeReader {
    fn new(source: &ReconstructedSource, peers: &[PeerVolume]) -> Result<Self, FormatError> {
        let source = match source {
            ReconstructedSource::SingleXor { recovery_path } => {
                ReconstructedReaderSource::SingleXor {
                    recovery: File::open(recovery_path)?,
                }
            }
            ReconstructedSource::DualWeighted {
                xor_path,
                weighted_path,
                target_coeff,
                other_coeff,
            } => {
                let denominator = target_coeff ^ other_coeff;
                let Some(denominator_inv) = gf256_inv(denominator) else {
                    return Err(FormatError::CorruptArchive(
                        "SQZ recovery volume has duplicate weighted coefficients".into(),
                    ));
                };
                ReconstructedReaderSource::DualWeighted {
                    xor: File::open(xor_path)?,
                    weighted: File::open(weighted_path)?,
                    other_coeff: *other_coeff,
                    denominator_inv,
                }
            }
            ReconstructedSource::TripleWeighted {
                xor_path,
                weighted_path,
                quadratic_path,
                target_coeff,
                other_coeffs,
            } => {
                let denominator = gf256_mul(
                    target_coeff ^ other_coeffs[0],
                    target_coeff ^ other_coeffs[1],
                );
                let Some(denominator_inv) = gf256_inv(denominator) else {
                    return Err(FormatError::CorruptArchive(
                        "SQZ recovery volume has duplicate quadratic coefficients".into(),
                    ));
                };
                ReconstructedReaderSource::TripleWeighted {
                    xor: File::open(xor_path)?,
                    weighted: File::open(weighted_path)?,
                    quadratic: File::open(quadratic_path)?,
                    other_coeffs: *other_coeffs,
                    denominator_inv,
                }
            }
        };
        Ok(Self {
            source,
            peers: peers
                .iter()
                .map(|peer| {
                    Ok(PeerReader {
                        coeff: sqzr_weighted_coeff(peer.index)?,
                        quadratic_coeff: sqzr_quadratic_coeff(peer.index)?,
                        file: File::open(&peer.path)?,
                        physical_len: peer.physical_len,
                    })
                })
                .collect::<Result<Vec<_>, FormatError>>()?,
        })
    }

    fn read_physical(&mut self, physical_offset: u64, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        match &mut self.source {
            ReconstructedReaderSource::SingleXor { recovery } => {
                recovery.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                recovery.read_exact(out)?;
                let mut peer_buf = vec![0u8; out.len()];
                for peer in &mut self.peers {
                    if physical_offset >= peer.physical_len {
                        continue;
                    }
                    let available = (peer.physical_len - physical_offset) as usize;
                    let take = out.len().min(available);
                    peer.file.seek(SeekFrom::Start(physical_offset))?;
                    peer.file.read_exact(&mut peer_buf[..take])?;
                    for (dst, src) in out.iter_mut().zip(&peer_buf[..take]) {
                        *dst ^= *src;
                    }
                    peer_buf[..take].fill(0);
                }
            }
            ReconstructedReaderSource::DualWeighted {
                xor,
                weighted,
                other_coeff,
                denominator_inv,
            } => {
                xor.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                xor.read_exact(out)?;
                let mut weighted_buf = vec![0u8; out.len()];
                weighted.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                weighted.read_exact(&mut weighted_buf)?;
                let mut peer_buf = vec![0u8; out.len()];
                for peer in &mut self.peers {
                    if physical_offset >= peer.physical_len {
                        continue;
                    }
                    let available = (peer.physical_len - physical_offset) as usize;
                    let take = out.len().min(available);
                    peer.file.seek(SeekFrom::Start(physical_offset))?;
                    peer.file.read_exact(&mut peer_buf[..take])?;
                    for ((xor_byte, weighted_byte), peer_byte) in out
                        .iter_mut()
                        .zip(weighted_buf.iter_mut())
                        .zip(&peer_buf[..take])
                    {
                        *xor_byte ^= *peer_byte;
                        *weighted_byte ^= gf256_mul(peer.coeff, *peer_byte);
                    }
                    peer_buf[..take].fill(0);
                }
                for (xor_byte, weighted_byte) in out.iter_mut().zip(weighted_buf) {
                    let numerator = weighted_byte ^ gf256_mul(*other_coeff, *xor_byte);
                    *xor_byte = gf256_mul(numerator, *denominator_inv);
                }
            }
            ReconstructedReaderSource::TripleWeighted {
                xor,
                weighted,
                quadratic,
                other_coeffs,
                denominator_inv,
            } => {
                xor.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                xor.read_exact(out)?;
                let mut weighted_buf = vec![0u8; out.len()];
                weighted.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                weighted.read_exact(&mut weighted_buf)?;
                let mut quadratic_buf = vec![0u8; out.len()];
                quadratic.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
                quadratic.read_exact(&mut quadratic_buf)?;
                let mut peer_buf = vec![0u8; out.len()];
                for peer in &mut self.peers {
                    if physical_offset >= peer.physical_len {
                        continue;
                    }
                    let available = (peer.physical_len - physical_offset) as usize;
                    let take = out.len().min(available);
                    peer.file.seek(SeekFrom::Start(physical_offset))?;
                    peer.file.read_exact(&mut peer_buf[..take])?;
                    for (((xor_byte, weighted_byte), quadratic_byte), peer_byte) in out
                        .iter_mut()
                        .zip(weighted_buf.iter_mut())
                        .zip(quadratic_buf.iter_mut())
                        .zip(&peer_buf[..take])
                    {
                        *xor_byte ^= *peer_byte;
                        *weighted_byte ^= gf256_mul(peer.coeff, *peer_byte);
                        *quadratic_byte ^= gf256_mul(peer.quadratic_coeff, *peer_byte);
                    }
                    peer_buf[..take].fill(0);
                }
                let bc = gf256_mul(other_coeffs[0], other_coeffs[1]);
                let b_xor_c = other_coeffs[0] ^ other_coeffs[1];
                for ((xor_byte, weighted_byte), quadratic_byte) in
                    out.iter_mut().zip(weighted_buf).zip(quadratic_buf)
                {
                    let numerator = quadratic_byte
                        ^ gf256_mul(b_xor_c, weighted_byte)
                        ^ gf256_mul(bc, *xor_byte);
                    *xor_byte = gf256_mul(numerator, *denominator_inv);
                }
            }
        }
        Ok(out.len())
    }
}

impl MultiVolumeReader {
    /// Opens every volume of the set.
    #[cfg(test)]
    pub(crate) fn open(set: &VolumeSet) -> Result<Self, FormatError> {
        Self::open_with_control(set, &ControlToken::default())
    }

    pub(crate) fn open_with_control(
        set: &VolumeSet,
        control: &ControlToken,
    ) -> Result<Self, FormatError> {
        control.checkpoint()?;
        let mut readers = Vec::with_capacity(set.len());
        let mut offsets = Vec::with_capacity(set.len());
        let mut data_offsets = Vec::with_capacity(set.len());
        let mut logical_lens = Vec::with_capacity(set.len());
        let mut total = 0u64;
        let mut sqzv_total = None;
        let mut sqzv_uuid = None;
        for (i, part) in set.parts().iter().enumerate() {
            control.checkpoint()?;
            let (reader, physical_len, header) = match &part.source {
                VolumePartSource::MissingZero => {
                    offsets.push(total);
                    data_offsets.push(part.data_offset);
                    logical_lens.push(part.logical_len);
                    total += part.logical_len;
                    readers.push(PartReader::MissingZero);
                    continue;
                }
                VolumePartSource::File => {
                    let mut file = File::open(&part.path)?;
                    let physical_len = file.metadata()?.len();
                    let header = read_sqzv_header(&mut file)?;
                    (PartReader::File(file), physical_len, header)
                }
                VolumePartSource::Reconstructed { source, peers } => {
                    let mut reader = ReconstructedVolumeReader::new(source, peers)?;
                    let physical_len = part.logical_len + part.data_offset;
                    let mut header_bytes = [0u8; SQZV_HEADER_LEN];
                    reader.read_physical(0, &mut header_bytes)?;
                    let header = parse_sqzv_header(&header_bytes)?;
                    (PartReader::Reconstructed(reader), physical_len, header)
                }
            };
            control.checkpoint()?;
            let data_offset = match (sqzv_total, header) {
                (None, None) => 0,
                (None, Some(header)) => {
                    validate_sqzv_header(&header, i as u32 + 1, set.len() as u32)?;
                    sqzv_total = Some(header.total);
                    sqzv_uuid = Some(header.uuid());
                    SQZV_HEADER_LEN_U64
                }
                (Some(_), Some(header)) => {
                    validate_sqzv_header(&header, i as u32 + 1, set.len() as u32)?;
                    if sqzv_uuid != Some(header.uuid()) {
                        return Err(FormatError::CorruptArchive(
                            "SQZV volume UUID mismatch".into(),
                        ));
                    }
                    SQZV_HEADER_LEN_U64
                }
                (Some(_), None) => {
                    return Err(FormatError::CorruptArchive(format!(
                        "missing SQZV header: {}",
                        part.path.display()
                    )));
                }
            };
            if physical_len < data_offset {
                return Err(FormatError::CorruptArchive(format!(
                    "truncated SQZV volume: {}",
                    part.path.display()
                )));
            }
            let logical_len = physical_len - data_offset;
            offsets.push(total);
            data_offsets.push(data_offset);
            logical_lens.push(logical_len);
            total += logical_len;
            readers.push(reader);
        }
        control.checkpoint()?;
        Ok(Self {
            parts: readers,
            offsets,
            data_offsets,
            logical_lens,
            total,
            pos: 0,
        })
    }
}

impl Read for MultiVolumeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total || buf.is_empty() {
            return Ok(0);
        }
        // Volume containing the current position.
        let idx = match self.offsets.binary_search(&self.pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let within = self.pos - self.offsets[idx];
        let remaining = (self.logical_lens[idx] - within) as usize;
        let want = buf.len().min(remaining);
        let n = match &mut self.parts[idx] {
            PartReader::File(file) => {
                file.seek(SeekFrom::Start(self.data_offsets[idx] + within))?;
                file.read(&mut buf[..want])?
            }
            PartReader::MissingZero => {
                buf[..want].fill(0);
                want
            }
            PartReader::Reconstructed(reader) => {
                reader.read_physical(self.data_offsets[idx] + within, &mut buf[..want])?
            }
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for MultiVolumeReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(o) => Some(o),
            SeekFrom::End(d) => self.total.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let target = target.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "seek before start of stream")
        })?;
        self.pos = target; // seeking past EOF is allowed (reads return 0)
        Ok(self.pos)
    }
}

fn split_sqz_uuid(sqz_uuid: Option<(u64, u64)>, target: &str) -> Result<(u64, u64), FormatError> {
    sqz_uuid.ok_or_else(|| {
        FormatError::CorruptArchive(format!("SQZ UUID missing while writing {target}"))
    })
}

fn normalized_split_output_base(base: &Path) -> Result<PathBuf, FormatError> {
    let name = base
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid split output file name".into()))?;
    Ok(split_volume_name(name)
        .map(|(base_name, _index)| base.with_file_name(base_name))
        .unwrap_or_else(|| base.to_path_buf()))
}

/// Splits the finished temporary archive at `tmp` into `base.001`,
/// `base.002`, ... volumes of `volume_size` bytes (the last volume holds
/// the remainder), with a disk-space pre-check and transactional finish.
/// Each output is fully written to a private sibling staging file before the
/// previous managed output set is backed up and the new set is installed.
/// `tmp` is consumed before the output transaction begins.
/// Replacing an existing set reports the old transaction-owned backups in the
/// returned artifacts. They are deliberately not deleted through replaceable
/// path names.
#[cfg(test)]
pub(crate) fn split_into_volumes(
    tmp: &Path,
    base: &Path,
    volume_size: u64,
    ctl: &ControlToken,
) -> Result<SplitArtifacts, FormatError> {
    split_into_volumes_with_commit_policy(
        tmp,
        base,
        volume_size,
        ctl,
        CreateCommitPolicy::ReplaceExisting,
    )
}

#[cfg(test)]
pub(crate) fn split_into_volumes_with_commit_policy(
    tmp: &Path,
    base: &Path,
    volume_size: u64,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<SplitArtifacts, FormatError> {
    split_into_volumes_with_commit_policy_inner(
        tmp,
        None,
        None,
        base,
        volume_size,
        &ResourceOptions::default(),
        &NoProgress,
        ctl,
        commit_policy,
    )
}

#[allow(clippy::too_many_arguments)] // source binding and progress are independent transaction inputs
pub(crate) fn split_into_volumes_with_commit_policy_and_source_identity(
    tmp: &Path,
    source_file: File,
    source_identity: PathIdentity,
    base: &Path,
    volume_size: u64,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<SplitArtifacts, FormatError> {
    split_into_volumes_with_commit_policy_inner(
        tmp,
        Some(source_file),
        Some(source_identity),
        base,
        volume_size,
        resources,
        progress,
        ctl,
        commit_policy,
    )
}

struct NativeSplitSink<'a> {
    base: &'a Path,
    format: &'a dyn ArchiveFormat,
    volume_size: u64,
    max_volumes: u32,
    resources: ResourceOptions,
    staging_id: SplitStagingId,
    parts: Vec<SplitStagingPath>,
    offset: u64,
}

impl<'a> NativeSplitSink<'a> {
    fn new(
        base: &'a Path,
        format: &'a dyn ArchiveFormat,
        volume_size: u64,
        max_volumes: u32,
        resources: ResourceOptions,
    ) -> Self {
        Self {
            base,
            format,
            volume_size,
            max_volumes,
            resources,
            staging_id: SplitStagingId::new(),
            parts: Vec::new(),
            offset: 0,
        }
    }

    fn start_volume(&mut self) -> Result<(), FormatError> {
        if self.parts.len() >= self.max_volumes as usize {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "native {} volume count exceeds {}",
                self.format.id(),
                self.max_volumes
            )));
        }
        if let Some(current) = self.parts.last() {
            current.file.sync_all()?;
        }
        let disk_index = u32::try_from(self.parts.len()).map_err(|_| {
            FormatError::ResourceLimitExceeded("native volume count exceeds 32-bit indexing".into())
        })?;
        let placeholder = self
            .format
            .native_volume_path(self.base, disk_index, false)?;
        validate_native_volume_path(self.base, &placeholder)?;
        let (part, file) = reserve_split_staging_file(&placeholder, self.staging_id)?;
        drop(file);
        self.parts.push(part);
        self.offset = 0;
        Ok(())
    }

    fn sync_active(&mut self) -> Result<(), FormatError> {
        if self.parts.is_empty() {
            return Err(FormatError::Other(
                "native volume writer produced no output".into(),
            ));
        }
        if let Some(current) = self.parts.last_mut() {
            current.file.sync_all()?;
        }
        Ok(())
    }

    fn into_parts(self) -> Vec<SplitStagingPath> {
        self.parts
    }
}

impl NativeVolumeWriter for NativeSplitSink<'_> {
    fn volume_size(&self) -> u64 {
        self.volume_size
    }

    fn stream_buffer_size(&self, default: usize) -> Result<usize, FormatError> {
        self.resources.stream_buffer_size(default)
    }

    fn disk_index(&self) -> u32 {
        self.parts.len().saturating_sub(1) as u32
    }

    fn disk_offset(&self) -> u64 {
        self.offset
    }

    fn ensure_record_capacity(&mut self, record_len: u64) -> Result<(), FormatError> {
        if record_len > self.volume_size {
            return Err(FormatError::Unsupported(format!(
                "a {record_len}-byte {} record does not fit the {}-byte native volume",
                self.format.id(),
                self.volume_size
            )));
        }
        if self.parts.is_empty()
            || self
                .offset
                .checked_add(record_len)
                .is_none_or(|end| end > self.volume_size)
        {
            self.start_volume()?;
        }
        Ok(())
    }

    fn write_spanning(&mut self, mut bytes: &[u8]) -> Result<(), FormatError> {
        if self.parts.is_empty() {
            self.start_volume()?;
        }
        while !bytes.is_empty() {
            if self.offset == self.volume_size {
                self.start_volume()?;
            }
            let remaining = self.volume_size - self.offset;
            let count = bytes.len().min(remaining as usize);
            let current = self.parts.last_mut().ok_or_else(|| {
                FormatError::Other("native volume writer lost its active output".into())
            })?;
            current.file.write_all(&bytes[..count])?;
            self.offset += count as u64;
            bytes = &bytes[count..];
        }
        Ok(())
    }

    fn begin_volume(&mut self) -> Result<(), FormatError> {
        self.start_volume()
    }

    fn write_current_volume(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        let current = self.parts.last_mut().ok_or_else(|| {
            FormatError::Other("native volume writer has no active output".into())
        })?;
        current.file.write_all(bytes)?;
        self.offset = self.offset.checked_add(bytes.len() as u64).ok_or_else(|| {
            FormatError::ResourceLimitExceeded(
                "native physical volume length exceeds 64-bit accounting".into(),
            )
        })?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn split_into_native_volumes_with_commit_policy_and_source_identity(
    tmp: &Path,
    source_file: File,
    expected_source_identity: PathIdentity,
    base: &Path,
    volume_size: u64,
    format: &dyn ArchiveFormat,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<SplitArtifacts, FormatError> {
    let base = normalized_split_output_base(base)?;
    let base = base.as_path();
    validate_split_output_base(base)?;
    let limits = format.native_volume_limits().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "format {} does not support native volume creation",
            format.id()
        ))
    })?;
    if !(limits.min_volume_size..=limits.max_volume_size).contains(&volume_size) {
        return Err(FormatError::Unsupported(format!(
            "native {} volume size must be between {} and {} bytes",
            format.id(),
            limits.min_volume_size,
            limits.max_volume_size
        )));
    }

    let mut reader = source_file;
    let source_identity = file_identity(&reader)?;
    if source_identity != expected_source_identity
        || path_identity(tmp)? != source_identity
        || !reader.metadata()?.is_file()
    {
        return Err(FormatError::Io(io::Error::other(
            "complete native-volume staging archive changed after writing",
        )));
    }
    let total = reader.metadata()?.len();
    let runtime_budget = format.native_volume_budget(total, 0, volume_size)?;
    if fs4::available_space(parent_or_current(base))?
        < runtime_budget.output_bytes.saturating_add(SPACE_SLACK)
    {
        return Err(FormatError::DiskFull);
    }
    reader.seek(SeekFrom::Start(0))?;

    let mut sink = NativeSplitSink::new(base, format, volume_size, limits.max_volumes, *resources);
    progress.on_phase(ProgressPhase::OutputSplit, true);
    if let Err(error) = format.write_native_volumes(&mut reader, &mut sink, progress, ctl) {
        let parts = sink.into_parts();
        return Err(split_staging_failure(
            error,
            tmp,
            &reader,
            source_identity,
            base,
            &parts,
            &[],
        ));
    }
    let source_stability = file_identity(&reader).and_then(|reader_identity| {
        path_identity(tmp).map(|path_identity| {
            reader_identity == source_identity && path_identity == source_identity
        })
    });
    let source_error = match source_stability {
        Ok(true) => None,
        Ok(false) => Some(FormatError::Io(io::Error::other(
            "complete native-volume staging archive changed while it was read",
        ))),
        Err(error) => Some(error.into()),
    };
    if let Some(error) = source_error {
        let parts = sink.into_parts();
        return Err(split_staging_failure(
            error,
            tmp,
            &reader,
            source_identity,
            base,
            &parts,
            &[],
        ));
    }
    if let Err(error) = sink.sync_active() {
        let parts = sink.into_parts();
        return Err(split_staging_failure(
            error,
            tmp,
            &reader,
            source_identity,
            base,
            &parts,
            &[],
        ));
    }
    let parts = sink.into_parts();
    let last_index = parts
        .len()
        .checked_sub(1)
        .ok_or_else(|| FormatError::Other("native volume writer produced no output".into()))?;
    let volume_count = match u32::try_from(parts.len()) {
        Ok(count) => count,
        Err(_) => {
            return Err(split_staging_failure(
                FormatError::ResourceLimitExceeded(
                    "native volume count exceeds 32-bit indexing".into(),
                ),
                tmp,
                &reader,
                source_identity,
                base,
                &parts,
                &[],
            ));
        }
    };
    let primary_index = match format
        .native_volume_primary_index(volume_count)
        .and_then(|index| {
            usize::try_from(index).map_err(|_| {
                FormatError::ResourceLimitExceeded(
                    "native primary volume index exceeds platform limits".into(),
                )
            })
        }) {
        Ok(index) => index,
        Err(error) => {
            return Err(split_staging_failure(
                error,
                tmp,
                &reader,
                source_identity,
                base,
                &parts,
                &[],
            ));
        }
    };
    if primary_index > last_index {
        return Err(split_staging_failure(
            FormatError::Other(
                "native format selected a primary member outside the output set".into(),
            ),
            tmp,
            &reader,
            source_identity,
            base,
            &parts,
            &[],
        ));
    }
    let mut volumes = Vec::with_capacity(parts.len());
    for index in 0..parts.len() {
        let disk_index = match u32::try_from(index) {
            Ok(index) => index,
            Err(_) => {
                return Err(split_staging_failure(
                    FormatError::ResourceLimitExceeded(
                        "native volume count exceeds 32-bit indexing".into(),
                    ),
                    tmp,
                    &reader,
                    source_identity,
                    base,
                    &parts,
                    &[],
                ));
            }
        };
        let final_path = match format.native_volume_path(base, disk_index, index == primary_index) {
            Ok(path) => path,
            Err(error) => {
                return Err(split_staging_failure(
                    error,
                    tmp,
                    &reader,
                    source_identity,
                    base,
                    &parts,
                    &[],
                ));
            }
        };
        if let Err(error) = validate_native_volume_path(base, &final_path) {
            return Err(split_staging_failure(
                error,
                tmp,
                &reader,
                source_identity,
                base,
                &parts,
                &[],
            ));
        }
        if volumes.iter().any(|path| path == &final_path) {
            return Err(split_staging_failure(
                FormatError::Other("native volume format returned duplicate output names".into()),
                tmp,
                &reader,
                source_identity,
                base,
                &parts,
                &[],
            ));
        }
        volumes.push(final_path);
    }
    let staged_outputs = parts
        .into_iter()
        .zip(volumes.iter().cloned())
        .map(|(part, final_path)| StagedSplitOutput {
            part: part.path,
            final_path,
            identity: part.identity,
            file: part.file,
        })
        .collect();
    commit_staged_split_outputs(
        tmp,
        &reader,
        source_identity,
        base,
        staged_outputs,
        volumes,
        primary_index,
        Vec::new(),
        false,
        progress,
        ctl,
        commit_policy,
    )
}

fn validate_native_volume_path(base: &Path, candidate: &Path) -> Result<(), FormatError> {
    if candidate.file_name().is_none() || parent_or_current(candidate) != parent_or_current(base) {
        return Err(FormatError::Unsupported(format!(
            "native volume output must remain next to {}",
            base.display()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // shared splitter carries optional source binding plus commit context
fn split_into_volumes_with_commit_policy_inner(
    tmp: &Path,
    source_file: Option<File>,
    expected_source_identity: Option<PathIdentity>,
    base: &Path,
    volume_size: u64,
    resources: &ResourceOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<SplitArtifacts, FormatError> {
    let base = normalized_split_output_base(base)?;
    let base = base.as_path();
    validate_split_output_base(base)?;
    let mut reader = match source_file {
        Some(file) => file,
        None => open_regular_file_no_follow_read_write(tmp)?,
    };
    let source_identity = file_identity(&reader)?;
    if expected_source_identity.is_some_and(|expected| expected != source_identity)
        || path_identity(tmp)? != source_identity
        || !reader.metadata()?.is_file()
    {
        return Err(FormatError::Io(io::Error::other(
            "complete split staging archive changed after writing",
        )));
    }
    let total = reader.metadata()?.len();
    let layout = split_layout(base, total, volume_size)?;
    let budget = split_output_budget(base, total, volume_size)?;
    let sqz_uuid = if layout.write_sqzv {
        Some(prepare_sqz_for_split(&mut reader, tmp)?)
    } else {
        None
    };
    reader.seek(SeekFrom::Start(0))?;

    // The volumes coexist with the temporary file until it is removed.
    let available = fs4::available_space(parent_or_current(base))?;
    if available < budget.additional_space_bytes {
        return Err(FormatError::DiskFull);
    }

    progress.on_phase(ProgressPhase::OutputSplit, true);
    progress.on_progress(0, total, &EntryPath::from_utf8(String::new()));
    let mut split_done = 0u64;
    let mut part_paths = Vec::with_capacity(layout.count as usize);
    let mut recovery_part_path = None;
    let mut parity_part_path = None;
    let mut weighted_part_path = None;
    let mut quadratic_part_path = None;
    let staging_id = SplitStagingId::new();
    let result = (|| -> Result<(), FormatError> {
        let mut buf = vec![0u8; resources.stream_buffer_size(COPY_CHUNK)?];
        let mut parity_out = if layout.write_sqzv && layout.count > 1 {
            let parity_volume = recovery_parity_volume_path(base);
            let (parity_part, file) = reserve_split_staging_file(&parity_volume, staging_id)?;
            parity_part_path = Some(parity_part);
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut weighted_out = if layout.write_weighted_parity {
            let parity_volume = recovery_weighted_parity_volume_path(base);
            let (parity_part, file) = reserve_split_staging_file(&parity_volume, staging_id)?;
            weighted_part_path = Some(parity_part);
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut quadratic_out = if layout.write_quadratic_parity {
            let parity_volume = recovery_quadratic_parity_volume_path(base);
            let (parity_part, file) = reserve_split_staging_file(&parity_volume, staging_id)?;
            quadratic_part_path = Some(parity_part);
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut tail_physical_len = 0u64;
        for i in 1..=layout.count {
            let volume = volume_path(base, i);
            let current_volume = EntryPath::from_utf8(
                volume
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| volume.display().to_string()),
            );
            let (part, mut out) = reserve_split_staging_file(&volume, staging_id)?;
            part_paths.push(part);
            let mut recovery_out = if layout.write_sqzv && layout.count > 1 && i == layout.count {
                let recovery_volume = recovery_volume_path(base, i);
                let (recovery_part, file) =
                    reserve_split_staging_file(&recovery_volume, staging_id)?;
                recovery_part_path = Some(recovery_part);
                Some(file)
            } else {
                None
            };
            if layout.write_sqzv {
                let (uuid_hi, uuid_lo) = split_sqz_uuid(sqz_uuid, "SQZV volume header")?;
                let header = sqzv_header(i, layout.count, uuid_hi, uuid_lo)?;
                out.write_all(&header)?;
                if let Some(recovery_out) = &mut recovery_out {
                    recovery_out.write_all(&header)?;
                }
                if let Some(parity_out) = &mut parity_out {
                    xor_sqzr_parity(parity_out, 0, &header)?;
                }
                if let Some(weighted_out) = &mut weighted_out {
                    weighted_sqzr_parity(weighted_out, 0, i, &header)?;
                }
                if let Some(quadratic_out) = &mut quadratic_out {
                    quadratic_sqzr_parity(quadratic_out, 0, i, &header)?;
                }
            }
            let mut physical_written = if layout.write_sqzv {
                SQZV_HEADER_LEN_U64
            } else {
                0
            };
            let mut left = layout
                .logical_volume_size
                .min(total - (i - 1) * layout.logical_volume_size);
            while left > 0 {
                ctl.checkpoint()?;
                let want = buf.len().min(left as usize);
                let n = reader.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(FormatError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "archive shrank while splitting",
                    )));
                }
                out.write_all(&buf[..n])?;
                if let Some(recovery_out) = &mut recovery_out {
                    recovery_out.write_all(&buf[..n])?;
                }
                if let Some(parity_out) = &mut parity_out {
                    xor_sqzr_parity(parity_out, physical_written, &buf[..n])?;
                }
                if let Some(weighted_out) = &mut weighted_out {
                    weighted_sqzr_parity(weighted_out, physical_written, i, &buf[..n])?;
                }
                if let Some(quadratic_out) = &mut quadratic_out {
                    quadratic_sqzr_parity(quadratic_out, physical_written, i, &buf[..n])?;
                }
                physical_written += n as u64;
                left -= n as u64;
                split_done = split_done.saturating_add(n as u64);
                progress.on_progress(split_done.min(total), total, &current_volume);
            }
            tail_physical_len = physical_written;
            out.sync_all()?;
            if let Some(recovery_out) = recovery_out {
                recovery_out.sync_all()?;
            }
        }
        if let Some(parity_out) = &mut parity_out {
            let (uuid_hi, uuid_lo) = split_sqz_uuid(sqz_uuid, "SQZR single parity header")?;
            let header = sqzr_header(
                layout.count,
                uuid_hi,
                uuid_lo,
                volume_size,
                tail_physical_len,
                SQZR_ALGO_XOR_SINGLE,
            )?;
            parity_out.seek(SeekFrom::Start(0))?;
            parity_out.write_all(&header)?;
            parity_out.sync_all()?;
        }
        if let Some(weighted_out) = &mut weighted_out {
            let (uuid_hi, uuid_lo) = split_sqz_uuid(sqz_uuid, "SQZR weighted parity header")?;
            let header = sqzr_header(
                layout.count,
                uuid_hi,
                uuid_lo,
                volume_size,
                tail_physical_len,
                SQZR_ALGO_XOR_WEIGHTED,
            )?;
            weighted_out.seek(SeekFrom::Start(0))?;
            weighted_out.write_all(&header)?;
            weighted_out.sync_all()?;
        }
        if let Some(quadratic_out) = &mut quadratic_out {
            let (uuid_hi, uuid_lo) = split_sqz_uuid(sqz_uuid, "SQZR quadratic parity header")?;
            let header = sqzr_header(
                layout.count,
                uuid_hi,
                uuid_lo,
                volume_size,
                tail_physical_len,
                SQZR_ALGO_XOR_QUADRATIC,
            )?;
            quadratic_out.seek(SeekFrom::Start(0))?;
            quadratic_out.write_all(&header)?;
            quadratic_out.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut staged_paths = part_paths;
        staged_paths.extend(recovery_part_path);
        staged_paths.extend(parity_part_path);
        staged_paths.extend(weighted_part_path);
        staged_paths.extend(quadratic_part_path);
        return Err(split_staging_failure(
            error,
            tmp,
            &reader,
            source_identity,
            base,
            &staged_paths,
            &[],
        ));
    }

    let mut volumes = Vec::with_capacity(layout.count as usize);
    let mut staged_outputs = Vec::with_capacity(
        part_paths.len()
            + recovery_part_path.iter().count()
            + parity_part_path.iter().count()
            + weighted_part_path.iter().count()
            + quadratic_part_path.iter().count(),
    );
    for (i, part) in part_paths.into_iter().enumerate() {
        let final_path = volume_path(base, i as u64 + 1);
        staged_outputs.push(StagedSplitOutput {
            part: part.path,
            final_path: final_path.clone(),
            identity: part.identity,
            file: part.file,
        });
        volumes.push(final_path);
    }
    let mut sidecars = Vec::new();
    if let Some(part) = recovery_part_path {
        let final_path = recovery_volume_path(base, layout.count);
        staged_outputs.push(StagedSplitOutput {
            part: part.path,
            final_path: final_path.clone(),
            identity: part.identity,
            file: part.file,
        });
        sidecars.push(final_path);
    }
    if let Some(part) = parity_part_path {
        let final_path = recovery_parity_volume_path(base);
        staged_outputs.push(StagedSplitOutput {
            part: part.path,
            final_path: final_path.clone(),
            identity: part.identity,
            file: part.file,
        });
        sidecars.push(final_path);
    }
    if let Some(part) = weighted_part_path {
        let final_path = recovery_weighted_parity_volume_path(base);
        staged_outputs.push(StagedSplitOutput {
            part: part.path,
            final_path: final_path.clone(),
            identity: part.identity,
            file: part.file,
        });
        sidecars.push(final_path);
    }
    if let Some(part) = quadratic_part_path {
        let final_path = recovery_quadratic_parity_volume_path(base);
        staged_outputs.push(StagedSplitOutput {
            part: part.path,
            final_path: final_path.clone(),
            identity: part.identity,
            file: part.file,
        });
        sidecars.push(final_path);
    }

    commit_staged_split_outputs(
        tmp,
        &reader,
        source_identity,
        base,
        staged_outputs,
        volumes,
        0,
        sidecars,
        layout.write_sqzv,
        progress,
        ctl,
        commit_policy,
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_staged_split_outputs(
    tmp: &Path,
    reader: &File,
    source_identity: PathIdentity,
    base: &Path,
    staged_outputs: Vec<StagedSplitOutput>,
    volumes: Vec<PathBuf>,
    primary_volume_index: usize,
    mut sidecars: Vec<PathBuf>,
    include_recovery: bool,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<SplitArtifacts, FormatError> {
    let total_output_bytes = match staged_outputs.iter().try_fold(0u64, |total, output| {
        fs::metadata(&output.part).map(|metadata| total.saturating_add(metadata.len()))
    }) {
        Ok(total) => total,
        Err(error) => {
            return Err(split_precommit_failure(
                error.into(),
                tmp,
                reader,
                source_identity,
                base,
                &staged_outputs,
                &[],
            ));
        }
    };

    progress.on_phase(ProgressPhase::OutputCommit, false);
    if let Err(error) = ctl.checkpoint() {
        return Err(split_precommit_failure(
            error,
            tmp,
            reader,
            source_identity,
            base,
            &staged_outputs,
            &[],
        ));
    }

    // The complete temporary archive contains the whole plaintext payload.
    // It must be gone before any existing output is moved out of place.
    let _commit_lock = match lock_split_output_set(base) {
        Ok(lock) => lock,
        Err(error) => {
            return Err(split_precommit_failure(
                error,
                tmp,
                reader,
                source_identity,
                base,
                &staged_outputs,
                &[],
            ));
        }
    };
    let recovered_outputs = match recover_split_transaction(base) {
        Ok(paths) => paths,
        Err(error) => {
            return Err(split_precommit_failure(
                error,
                tmp,
                reader,
                source_identity,
                base,
                &staged_outputs,
                &[],
            ));
        }
    };
    let managed_snapshot = match commit_policy {
        CreateCommitPolicy::NoReplace => {
            let managed = match collect_managed_split_outputs(base, include_recovery) {
                Ok(managed) => managed,
                Err(error) => {
                    return Err(split_precommit_failure(
                        error,
                        tmp,
                        reader,
                        source_identity,
                        base,
                        &staged_outputs,
                        &recovered_outputs,
                    ));
                }
            };
            if !managed.is_empty() {
                return Err(split_precommit_failure(
                    crate::output_exists_error(&managed[0]),
                    tmp,
                    reader,
                    source_identity,
                    base,
                    &staged_outputs,
                    &recovered_outputs,
                ));
            }
            None
        }
        CreateCommitPolicy::ReplaceExisting => {
            match snapshot_managed_split_outputs(base, include_recovery) {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    return Err(split_precommit_failure(
                        error,
                        tmp,
                        reader,
                        source_identity,
                        base,
                        &staged_outputs,
                        &recovered_outputs,
                    ));
                }
            }
        }
        CreateCommitPolicy::ReplaceIfUnchanged(guard) => {
            let snapshot = match verify_guarded_split_snapshot(base, include_recovery, guard) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return Err(split_precommit_failure(
                        error,
                        tmp,
                        reader,
                        source_identity,
                        base,
                        &staged_outputs,
                        &recovered_outputs,
                    ));
                }
            };
            Some(snapshot)
        }
    };
    if let Err(error) =
        remove_split_source_before_commit(tmp, reader, source_identity, &staged_outputs)
    {
        return Err(split_precommit_failure(
            error,
            tmp,
            reader,
            source_identity,
            base,
            &staged_outputs,
            &recovered_outputs,
        ));
    }
    if let Err(error) = sync_directory(parent_or_current(base)) {
        return Err(split_precommit_failure(
            error.into(),
            tmp,
            reader,
            source_identity,
            base,
            &staged_outputs,
            &recovered_outputs,
        ));
    }

    let commit = match managed_snapshot {
        Some(managed) => commit_split_outputs(base, &staged_outputs, include_recovery, managed),
        None => commit_split_outputs_no_replace(&staged_outputs).map(|()| Vec::new()),
    };
    let preserved_outputs = match commit {
        Ok(mut preserved_outputs) => {
            preserved_outputs.extend(recovered_outputs.clone());
            bind_preserved_split_outputs(preserved_outputs)?
        }
        Err(error) => {
            if matches!(commit_policy, CreateCommitPolicy::NoReplace) {
                return Err(split_precommit_failure(
                    error,
                    tmp,
                    reader,
                    source_identity,
                    base,
                    &staged_outputs,
                    &recovered_outputs,
                ));
            }
            return Err(with_recovered_split_debt(error, &recovered_outputs));
        }
    };

    sidecars.sort();
    Ok(SplitArtifacts {
        volumes,
        primary_volume_index,
        sidecars,
        preserved_outputs,
        total_output_bytes,
    })
}

fn split_precommit_failure(
    error: FormatError,
    tmp: &Path,
    tmp_file: &File,
    tmp_identity: PathIdentity,
    base: &Path,
    staged: &[StagedSplitOutput],
    recovered: &[PreservedSplitOutput],
) -> FormatError {
    split_staging_failure(error, tmp, tmp_file, tmp_identity, base, staged, recovered)
}

fn split_staging_failure<B: SplitStagingBinding>(
    error: FormatError,
    tmp: &Path,
    tmp_file: &File,
    tmp_identity: PathIdentity,
    base: &Path,
    staged_paths: &[B],
    recovered: &[PreservedSplitOutput],
) -> FormatError {
    let mut cleanup_errors = Vec::new();
    if let Err(remove_error) = crate::remove_bound_temp_file(tmp, tmp_file, tmp_identity) {
        cleanup_errors.push(format!(
            "could not securely remove complete split staging archive {}: {remove_error}",
            tmp.display()
        ));
    }
    for staged in staged_paths {
        if let Err(remove_error) = remove_bound_split_staging(staged) {
            cleanup_errors.push(remove_error.to_string());
        }
    }
    if let Err(sync_error) = sync_directory(parent_or_current(base)) {
        cleanup_errors.push(format!(
            "could not synchronize split staging cleanup: {sync_error}"
        ));
    }
    with_recovered_split_debt(with_split_cleanup_errors(error, cleanup_errors), recovered)
}

fn remove_bound_split_staging<B: SplitStagingBinding>(staged: &B) -> Result<(), FormatError> {
    let path = staged.staging_path();
    let expected = staged.staging_identity();
    if split_file_identity(staged.staging_file())? != expected {
        return Err(FormatError::Io(io::Error::other(format!(
            "split staging handle identity changed and the path was left untouched: {}",
            path.display()
        ))));
    }
    match split_path_identity(path) {
        Ok(identity) if identity == expected => {
            let quarantine = crate::sibling_temp_path(path, "split-cleanup")?;
            crate::move_path_no_replace(path, &quarantine)?;
            if split_file_identity(staged.staging_file())? != expected
                || split_path_identity(&quarantine).ok() != Some(expected)
            {
                return Err(FormatError::Io(io::Error::other(format!(
                    "a competing split staging entry was isolated and left untouched for recovery: {}",
                    quarantine.display()
                ))));
            }
            fs::remove_file(&quarantine).map_err(|error| {
                FormatError::from(io::Error::new(
                    error.kind(),
                    format!(
                        "could not remove isolated split staging {}: {error}",
                        quarantine.display()
                    ),
                ))
            })?;
            sync_directory(parent_or_current(path)).map_err(|error| {
                FormatError::from(io::Error::new(
                    error.kind(),
                    format!(
                        "could not synchronize cleanup of isolated split staging {}: {error}",
                        quarantine.display()
                    ),
                ))
            })?;
            Ok(())
        }
        Ok(_) => Err(FormatError::Io(io::Error::other(format!(
            "split staging identity changed and the competing path was left untouched: {}",
            path.display()
        )))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FormatError::from(io::Error::new(
            error.kind(),
            format!(
                "split staging ownership could not be verified and the path was left untouched: {}: {error}",
                path.display()
            ),
        ))),
    }
}

fn with_split_cleanup_errors(error: FormatError, cleanup_errors: Vec<String>) -> FormatError {
    if cleanup_errors.is_empty() {
        error
    } else {
        FormatError::Other(format!(
            "{error}; split staging cleanup was incomplete or not durable: {}",
            cleanup_errors.join("; ")
        ))
    }
}

#[cfg(test)]
fn split_staging_failure_with<B, D, S>(
    error: FormatError,
    tmp: &Path,
    tmp_identity: PathIdentity,
    staged_paths: &[B],
    recovered: &[PreservedSplitOutput],
    remove: &mut D,
    sync_parent: &mut S,
) -> FormatError
where
    B: SplitStagingBinding,
    D: FnMut(&Path) -> io::Result<()>,
    S: FnMut() -> io::Result<()>,
{
    let mut cleanup_errors = Vec::new();
    if path_identity(tmp).ok() == Some(tmp_identity) {
        match remove(tmp) {
            Ok(()) => {}
            Err(remove_error) if remove_error.kind() == io::ErrorKind::NotFound => {}
            Err(remove_error) => cleanup_errors.push(format!(
                "could not remove split staging path {}: {remove_error}",
                tmp.display()
            )),
        }
    } else if tmp.exists() {
        cleanup_errors.push(format!(
            "complete split staging identity changed and the competing path was left untouched: {}",
            tmp.display()
        ));
    }
    for staged in staged_paths {
        let path = staged.staging_path();
        let identity = staged.staging_identity();
        if split_file_identity(staged.staging_file()).ok() != Some(identity)
            || split_path_identity(path).ok() != Some(identity)
        {
            if path.exists() {
                cleanup_errors.push(format!(
                    "split staging identity changed and the competing path was left untouched: {}",
                    path.display()
                ));
            }
            continue;
        }
        match remove(path) {
            Ok(()) => {}
            Err(remove_error) if remove_error.kind() == io::ErrorKind::NotFound => {}
            Err(remove_error) => cleanup_errors.push(format!(
                "could not remove split staging path {}: {remove_error}",
                path.display()
            )),
        }
    }
    if let Err(sync_error) = sync_parent() {
        cleanup_errors.push(format!(
            "could not synchronize split staging cleanup: {sync_error}"
        ));
    }
    let error = if cleanup_errors.is_empty() {
        error
    } else {
        FormatError::Other(format!(
            "{error}; split staging cleanup was incomplete or not durable: {}",
            cleanup_errors.join("; ")
        ))
    };
    with_recovered_split_debt(error, recovered)
}

fn with_recovered_split_debt(
    error: FormatError,
    recovered: &[PreservedSplitOutput],
) -> FormatError {
    if recovered.is_empty() {
        return error;
    }
    with_preserved_split_debt(
        error,
        recovered,
        "an interrupted split transaction was recovered and its previous outputs remain at",
    )
}

fn with_preserved_split_debt(
    error: FormatError,
    preserved: &[PreservedSplitOutput],
    context: &str,
) -> FormatError {
    if preserved.is_empty() {
        return error;
    }
    FormatError::Other(format!(
        "{error}; {context}: {}",
        preserved
            .iter()
            .map(|entry| entry.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn bind_preserved_split_outputs(
    preserved: Vec<PreservedSplitOutput>,
) -> Result<Vec<PathBuf>, FormatError> {
    let mut bound = HashMap::with_capacity(preserved.len());
    for entry in preserved {
        let binding = (entry.identity, entry.state_digest);
        if let Some(previous) = bound.insert(entry.path.clone(), binding) {
            if previous != binding {
                return Err(split_transaction_conflict(
                    "two split transactions claimed different bindings at one backup path",
                    [&entry.path, &entry.path],
                ));
            }
        }
    }
    let mut paths = Vec::with_capacity(bound.len());
    for (path, (identity, state_digest)) in bound {
        ensure_split_identity(&path, identity, "previous output backup")?;
        ensure_split_state_binding(&path, state_digest, "previous output backup")?;
        paths.push(path);
    }
    paths.sort();
    Ok(paths)
}

fn bound_transaction_backups(transaction: &ResolvedSplitTransaction) -> Vec<PreservedSplitOutput> {
    transaction
        .backups
        .iter()
        .filter_map(|entry| {
            let identity_matches = matches!(
                split_path_identity(&entry.backup),
                Ok(identity) if identity == entry.identity
            );
            let state_matches = matches!(
                path_state_digest(&entry.backup),
                Ok(Some(actual)) if actual == entry.state_digest
            );
            (identity_matches && state_matches).then(|| PreservedSplitOutput {
                path: entry.backup.clone(),
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
        })
        .collect()
}

#[derive(Debug)]
struct StagedSplitOutput {
    part: PathBuf,
    final_path: PathBuf,
    identity: SplitPathIdentity,
    file: File,
}

trait SplitStagingBinding {
    fn staging_path(&self) -> &Path;
    fn staging_identity(&self) -> SplitPathIdentity;
    fn staging_file(&self) -> &File;
}

impl SplitStagingBinding for SplitStagingPath {
    fn staging_path(&self) -> &Path {
        &self.path
    }

    fn staging_identity(&self) -> SplitPathIdentity {
        self.identity
    }

    fn staging_file(&self) -> &File {
        &self.file
    }
}

impl SplitStagingBinding for StagedSplitOutput {
    fn staging_path(&self) -> &Path {
        &self.part
    }

    fn staging_identity(&self) -> SplitPathIdentity {
        self.identity
    }

    fn staging_file(&self) -> &File {
        &self.file
    }
}

#[derive(Debug)]
struct ManagedSplitOutputSnapshot {
    path: PathBuf,
    identity: SplitPathIdentity,
    state: RegularFileState,
    #[cfg(windows)]
    change_time: i64,
    state_digest: [u8; 32],
}

#[cfg(test)]
#[derive(Debug)]
struct ManagedSplitBackup {
    original: PathBuf,
    backup: PathBuf,
    identity: SplitPathIdentity,
}

#[cfg(test)]
#[derive(Debug)]
struct InstalledSplitOutput {
    final_path: PathBuf,
    identity: SplitPathIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitPathIdentity {
    filesystem: u64,
    entry: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitJournalBackup {
    original: StoredOsString,
    backup: StoredOsString,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitJournalOutput {
    staged: StoredOsString,
    final_path: StoredOsString,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SplitTransactionRecord {
    version: u32,
    base_name: StoredOsString,
    backups: Vec<SplitJournalBackup>,
    outputs: Vec<SplitJournalOutput>,
}

#[derive(Debug)]
struct OpenSplitTransaction {
    path: PathBuf,
    file: File,
    identity: SplitPathIdentity,
    content_digest: [u8; 32],
    record: SplitTransactionRecord,
}

impl SplitStagingBinding for OpenSplitTransaction {
    fn staging_path(&self) -> &Path {
        &self.path
    }

    fn staging_identity(&self) -> SplitPathIdentity {
        self.identity
    }

    fn staging_file(&self) -> &File {
        &self.file
    }
}

#[derive(Debug)]
struct ResolvedSplitBackup {
    original: PathBuf,
    backup: PathBuf,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug)]
struct ResolvedSplitOutput {
    staged: PathBuf,
    final_path: PathBuf,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

#[derive(Debug)]
struct ResolvedSplitTransaction {
    base: PathBuf,
    include_recovery: bool,
    backups: Vec<ResolvedSplitBackup>,
    outputs: Vec<ResolvedSplitOutput>,
}

#[derive(Debug, Clone)]
struct PreservedSplitOutput {
    path: PathBuf,
    identity: SplitPathIdentity,
    state_digest: [u8; 32],
}

fn split_journal_name(path: &Path) -> Result<StoredOsString, FormatError> {
    let name = path.file_name().ok_or_else(|| {
        FormatError::Unsupported("split transaction path has no file name".into())
    })?;
    StoredOsString::from_os_str(name)
}

fn lock_split_output_set(base: &Path) -> Result<File, FormatError> {
    let parent = fs::canonicalize(parent_or_current(base))?;
    let name = base
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("split output path has no file name".into()))?;
    let identity = format!(
        "{}\0{}",
        parent.to_string_lossy().to_lowercase(),
        name.to_string_lossy().to_lowercase()
    );
    let lock_path = std::env::temp_dir().join(format!(
        "squallz-split-commit-{}.lock",
        blake3::hash(identity.as_bytes())
    ));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    fs4::FileExt::lock(&lock)?;
    Ok(lock)
}

fn split_transaction_journal_path(base: &Path) -> Result<PathBuf, FormatError> {
    let name = base
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("split output path has no file name".into()))?;
    let mut journal_name = OsString::from(".");
    journal_name.push(name);
    journal_name.push(".split-transaction.json");
    Ok(parent_or_current(base).join(journal_name))
}

fn snapshot_managed_split_outputs(
    base: &Path,
    include_recovery: bool,
) -> Result<Vec<ManagedSplitOutputSnapshot>, FormatError> {
    let managed = collect_managed_split_outputs(base, include_recovery)?;
    let mut snapshot = Vec::with_capacity(managed.len());
    for path in managed {
        let identity = split_snapshot_identity(&path, base)?;
        let state_digest = path_state_digest(&path)?
            .ok_or_else(|| FormatError::destination_changed(base.to_path_buf()))?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            return Err(FormatError::destination_changed(base.to_path_buf()));
        }
        let state = RegularFileState::from_metadata(&metadata);
        #[cfg(windows)]
        let change_time = path_change_time(&path)?;
        if split_snapshot_identity(&path, base)? != identity {
            return Err(FormatError::destination_changed(base.to_path_buf()));
        }
        snapshot.push(ManagedSplitOutputSnapshot {
            path,
            identity,
            state,
            #[cfg(windows)]
            change_time,
            state_digest,
        });
    }
    Ok(snapshot)
}

fn verify_guarded_split_snapshot(
    base: &Path,
    include_recovery: bool,
    guard: CreateDestinationGuard,
) -> Result<Vec<ManagedSplitOutputSnapshot>, FormatError> {
    let expected = verify_destination_guard_binding(base, CreateArtifactKind::SplitArchive, guard)?;
    verify_guarded_split_snapshot_after_destination_check(base, include_recovery, expected, guard)
}

fn verify_guarded_split_snapshot_after_destination_check(
    base: &Path,
    include_recovery: bool,
    expected: [u8; 32],
    guard: CreateDestinationGuard,
) -> Result<Vec<ManagedSplitOutputSnapshot>, FormatError> {
    let snapshot = snapshot_managed_split_outputs(base, include_recovery)
        .map_err(|error| guarded_split_snapshot_error(base, error))?;
    let members = split_snapshot_members(&snapshot)?;
    if split_snapshot_state_digest(&members, &snapshot)? != expected
        || expected != guard.state_digest()
    {
        return Err(FormatError::destination_changed(base.to_path_buf()));
    }
    verify_split_snapshot_unchanged(base, include_recovery, &snapshot)
        .map_err(|error| guarded_split_snapshot_error(base, error))?;
    Ok(snapshot)
}

fn guarded_split_snapshot_error(base: &Path, error: FormatError) -> FormatError {
    match error {
        FormatError::Unsupported(_) => FormatError::destination_changed(base.to_path_buf()),
        FormatError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            FormatError::destination_changed(base.to_path_buf())
        }
        error => error,
    }
}

fn split_snapshot_members(
    snapshot: &[ManagedSplitOutputSnapshot],
) -> Result<Vec<(OsString, PathBuf)>, FormatError> {
    snapshot
        .iter()
        .map(|entry| {
            let name = entry.path.file_name().ok_or_else(|| {
                FormatError::Unsupported("split output path has no file name".into())
            })?;
            Ok((name.to_os_string(), entry.path.clone()))
        })
        .collect()
}

fn verify_split_snapshot_unchanged(
    base: &Path,
    include_recovery: bool,
    snapshot: &[ManagedSplitOutputSnapshot],
) -> Result<(), FormatError> {
    let current = collect_managed_split_outputs(base, include_recovery)?;
    if current.len() != snapshot.len()
        || snapshot.iter().any(|entry| {
            !current
                .iter()
                .any(|path| crate::same_path_entry(path, &entry.path))
        })
    {
        return Err(FormatError::destination_changed(base.to_path_buf()));
    }
    for entry in snapshot {
        let metadata = fs::symlink_metadata(&entry.path)?;
        #[cfg(windows)]
        let change_time_changed = path_change_time(&entry.path)? != entry.change_time;
        #[cfg(not(windows))]
        let change_time_changed = false;
        if split_snapshot_identity(&entry.path, base)? != entry.identity
            || !entry.state.matches(&metadata)
            || change_time_changed
        {
            return Err(FormatError::destination_changed(base.to_path_buf()));
        }
    }
    Ok(())
}

fn split_snapshot_state_digest(
    members: &[(OsString, PathBuf)],
    snapshot: &[ManagedSplitOutputSnapshot],
) -> Result<[u8; 32], FormatError> {
    if members.len() != snapshot.len() {
        return Err(FormatError::Other(
            "split snapshot member has no content binding".into(),
        ));
    }
    let mut entries = Vec::with_capacity(members.len());
    for ((name, path), entry) in members.iter().zip(snapshot) {
        if !crate::same_path_entry(path, &entry.path) {
            return Err(FormatError::Other(
                "split snapshot member has no content binding".into(),
            ));
        }
        entries.push((split_snapshot_os_key(name), entry.state_digest));
    }
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-destination-split-family-v1\0");
    hasher.update(&(entries.len() as u64).to_le_bytes());
    for (name, state_digest) in entries {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(&name);
        hasher.update(&state_digest);
    }
    Ok(*hasher.finalize().as_bytes())
}

#[cfg(unix)]
fn split_snapshot_os_key(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn split_snapshot_os_key(value: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}

#[cfg(not(any(unix, windows)))]
fn split_snapshot_os_key(value: &std::ffi::OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

fn split_snapshot_identity(
    path: &Path,
    reported_destination: &Path,
) -> Result<SplitPathIdentity, FormatError> {
    split_path_identity(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            FormatError::destination_changed(reported_destination.to_path_buf())
        } else {
            error.into()
        }
    })
}

pub(crate) fn matches_split_transaction_journal(base: &Path, path: &Path) -> bool {
    let Ok(journal) = split_transaction_journal_path(base) else {
        return false;
    };
    if crate::same_path_entry(&journal, path) {
        return true;
    }
    let (Some(journal_name), Some(candidate_name)) = (
        journal.file_name().and_then(|name| name.to_str()),
        path.file_name().and_then(|name| name.to_str()),
    ) else {
        return false;
    };
    if split_transaction_output_name(candidate_name, "split-cleanup") == Some(journal_name) {
        return crate::same_path_entry(parent_or_current(&journal), parent_or_current(path));
    }
    let Some(suffix) = candidate_name
        .strip_prefix('.')
        .and_then(|name| name.strip_prefix(journal_name))
        .and_then(|name| name.strip_prefix(".tmp-"))
    else {
        return false;
    };
    let mut parts = suffix.split('-');
    let valid_identity = (0..2).all(|_| {
        parts
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) && parts.next().is_none();
    valid_identity && crate::same_path_entry(parent_or_current(&journal), parent_or_current(path))
}

fn recover_split_transaction(base: &Path) -> Result<Vec<PreservedSplitOutput>, FormatError> {
    let Some(open) = open_split_transaction(base)? else {
        return Ok(Vec::new());
    };
    let resolved = resolve_split_transaction(base, &open.record)?;
    let preserved = match resume_split_transaction(&resolved, &open) {
        Ok(preserved) => preserved,
        Err(error) => {
            return Err(with_preserved_split_debt(
                error,
                &bound_transaction_backups(&resolved),
                "verified previous outputs currently remain at",
            ));
        }
    };
    if let Err(error) = clear_split_transaction(open) {
        return Err(with_preserved_split_debt(
            error,
            &preserved,
            "the interrupted split transaction was recovered and its previous outputs remain at",
        ));
    }
    Ok(preserved)
}

fn commit_split_outputs(
    base: &Path,
    staged: &[StagedSplitOutput],
    include_recovery: bool,
    managed: Vec<ManagedSplitOutputSnapshot>,
) -> Result<Vec<PreservedSplitOutput>, FormatError> {
    let prepared = (|| {
        validate_staged_split_outputs(staged)?;
        let mut backups = Vec::with_capacity(managed.len());
        for entry in managed {
            backups.push(ResolvedSplitBackup {
                backup: crate::sibling_temp_path(&entry.path, "split-backup")?,
                identity: entry.identity,
                state_digest: entry.state_digest,
                original: entry.path,
            });
        }
        let mut outputs = Vec::with_capacity(staged.len());
        for output in staged {
            if split_file_identity(&output.file).ok() != Some(output.identity)
                || split_path_identity(&output.part).ok() != Some(output.identity)
            {
                return Err(split_transaction_conflict(
                    "the staged split output was replaced after writing",
                    [&output.part],
                ));
            }
            let state_digest = path_state_digest(&output.part)?.ok_or_else(|| {
                split_transaction_conflict(
                    "the staged split output disappeared while it was bound",
                    [&output.part],
                )
            })?;
            if split_file_identity(&output.file)? != output.identity
                || split_path_identity(&output.part)? != output.identity
            {
                return Err(split_transaction_conflict(
                    "the staged split output identity changed while it was bound",
                    [&output.part],
                ));
            }
            outputs.push(ResolvedSplitOutput {
                staged: output.part.clone(),
                final_path: output.final_path.clone(),
                identity: output.identity,
                state_digest,
            });
        }
        let resolved = ResolvedSplitTransaction {
            base: base.to_path_buf(),
            include_recovery,
            backups,
            outputs,
        };
        let record = split_transaction_record(base, &resolved)?;
        let resolved = resolve_split_transaction(base, &record)?;
        Ok::<_, FormatError>((resolved, record))
    })();
    let (resolved, record) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(with_split_cleanup_errors(
                error,
                remove_staged_split_outputs(staged),
            ));
        }
    };
    let open = match write_split_transaction_with_state(base, record) {
        Ok(open) => open,
        Err(failure) => {
            if failure.journal_published {
                return Err(FormatError::Other(format!(
                    "{}; writer-owned split staging was retained because the transaction journal may already be durable",
                    failure.error
                )));
            }
            return Err(with_split_cleanup_errors(
                failure.error,
                remove_staged_split_outputs(staged),
            ));
        }
    };
    let preserved = match resume_split_transaction(&resolved, &open) {
        Ok(preserved) => preserved,
        Err(error) => {
            return Err(with_preserved_split_debt(
                error,
                &bound_transaction_backups(&resolved),
                "verified previous outputs currently remain at",
            ));
        }
    };
    if let Err(error) = clear_split_transaction(open) {
        return Err(with_preserved_split_debt(
            error,
            &preserved,
            "the new split output set was installed and its previous outputs remain at",
        ));
    }
    Ok(preserved)
}

fn split_transaction_record(
    base: &Path,
    transaction: &ResolvedSplitTransaction,
) -> Result<SplitTransactionRecord, FormatError> {
    let base_name = split_journal_name(base)?;
    let backups = transaction
        .backups
        .iter()
        .map(|entry| {
            Ok(SplitJournalBackup {
                original: split_journal_name(&entry.original)?,
                backup: split_journal_name(&entry.backup)?,
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    let outputs = transaction
        .outputs
        .iter()
        .map(|entry| {
            Ok(SplitJournalOutput {
                staged: split_journal_name(&entry.staged)?,
                final_path: split_journal_name(&entry.final_path)?,
                identity: entry.identity,
                state_digest: entry.state_digest,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(SplitTransactionRecord {
        version: SPLIT_TRANSACTION_VERSION,
        base_name,
        backups,
        outputs,
    })
}

#[cfg(test)]
fn write_split_transaction(
    base: &Path,
    record: SplitTransactionRecord,
) -> Result<OpenSplitTransaction, FormatError> {
    write_split_transaction_with_state(base, record).map_err(|failure| failure.error)
}

#[derive(Debug)]
struct SplitTransactionWriteFailure {
    error: FormatError,
    journal_published: bool,
}

fn write_split_transaction_with_state(
    base: &Path,
    record: SplitTransactionRecord,
) -> Result<OpenSplitTransaction, SplitTransactionWriteFailure> {
    let mut journal_published = false;
    let result = (|| -> Result<OpenSplitTransaction, FormatError> {
        let path = split_transaction_journal_path(base)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(FormatError::Io(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "split transaction journal already exists: {}",
                        path.display()
                    ),
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let bytes = serde_json::to_vec(&record)
            .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "split transaction journal exceeds {SPLIT_TRANSACTION_MAX_BYTES} bytes"
            )));
        }
        let parent = parent_or_current(&path);
        let file_name = path
            .file_name()
            .ok_or_else(|| FormatError::Unsupported("split journal has no file name".into()))?;
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            SPLIT_JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let temp_path = parent.join(temp_name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            };

            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(&temp_path)?;
        let identity = split_file_identity(&file)?;
        if split_path_identity(&temp_path).ok() != Some(identity) {
            return Err(FormatError::Io(io::Error::other(format!(
            "split transaction staging changed while it was reserved and was left untouched: {}",
            temp_path.display()
        ))));
        }
        let mut temp = SplitStagingPath {
            path: temp_path,
            identity,
            file,
        };
        if let Err(error) = temp
            .file
            .write_all(&bytes)
            .and_then(|()| temp.file.sync_all())
        {
            let cleanup_errors = remove_bound_split_staging(&temp)
                .err()
                .map(|cleanup| vec![cleanup.to_string()])
                .unwrap_or_default();
            return Err(with_split_cleanup_errors(error.into(), cleanup_errors));
        }
        if split_file_identity(&temp.file)? != temp.identity
            || split_path_identity(&temp.path).ok() != Some(temp.identity)
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "split transaction staging changed after writing and was left untouched: {}",
                temp.path.display()
            ))));
        }
        if let Err(error) = crate::move_path_no_replace(&temp.path, &path) {
            let cleanup_errors = remove_bound_split_staging(&temp)
                .err()
                .map(|cleanup| vec![cleanup.to_string()])
                .unwrap_or_default();
            return Err(with_split_cleanup_errors(error.into(), cleanup_errors));
        }
        journal_published = true;
        if split_file_identity(&temp.file)? != temp.identity
            || split_path_identity(&path).ok() != Some(temp.identity)
        {
            return Err(FormatError::Io(io::Error::other(format!(
            "published split transaction journal no longer matches its writer-owned staging file and was left for recovery: {}",
            path.display()
        ))));
        }
        sync_directory(parent).map_err(|error| {
        FormatError::from(io::Error::new(
            error.kind(),
            format!(
                "published split transaction journal could not be synchronized and was left for recovery at {}: {error}",
                path.display()
            ),
        ))
    })?;
        Ok(OpenSplitTransaction {
            path,
            file: temp.file,
            identity: temp.identity,
            content_digest: *blake3::hash(&bytes).as_bytes(),
            record,
        })
    })();
    result.map_err(|error| SplitTransactionWriteFailure {
        error,
        journal_published,
    })
}

fn open_split_transaction(base: &Path) -> Result<Option<OpenSplitTransaction>, FormatError> {
    let path = split_transaction_journal_path(base)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(format!(
            "split transaction journal must be a regular file: {}",
            path.display()
        )));
    }
    let mut file = open_regular_file_no_follow_read_write(&path)?;
    let identity = split_file_identity(&file)?;
    if split_path_identity(&path)? != identity {
        return Err(FormatError::Io(io::Error::other(
            "split transaction journal changed while it was opened",
        )));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((SPLIT_TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "split transaction journal exceeds {SPLIT_TRANSACTION_MAX_BYTES} bytes"
        )));
    }
    let record = serde_json::from_slice(&bytes)
        .map_err(|error| FormatError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    Ok(Some(OpenSplitTransaction {
        path,
        file,
        identity,
        content_digest: *blake3::hash(&bytes).as_bytes(),
        record,
    }))
}

fn resolve_split_transaction(
    base: &Path,
    record: &SplitTransactionRecord,
) -> Result<ResolvedSplitTransaction, FormatError> {
    if record.version != SPLIT_TRANSACTION_VERSION {
        return Err(FormatError::Unsupported(format!(
            "unsupported split transaction journal version: {}",
            record.version
        )));
    }
    let base_name = record.base_name.to_os_string()?;
    let parent = parent_or_current(base);
    let recorded_base = parent.join(&base_name);
    let requested_journal = split_transaction_journal_path(base)?;
    let recorded_journal = split_transaction_journal_path(&recorded_base)?;
    if !crate::same_path_entry(&requested_journal, &recorded_journal) {
        return Err(FormatError::Unsupported(
            "split transaction journal belongs to another output family".into(),
        ));
    }
    let base_name_utf8 = base_name.to_str().ok_or_else(|| {
        FormatError::Unsupported("split transaction base name must be UTF-8".into())
    })?;
    let include_recovery = is_sqz_base(&recorded_base);
    let mut original_names = HashSet::new();
    let mut backup_names = HashSet::new();
    let mut staged_names = HashSet::new();
    let mut final_names = HashSet::new();
    let mut backup_identities = HashSet::new();
    let mut output_identities = HashSet::new();
    let mut backups = Vec::with_capacity(record.backups.len());
    for entry in &record.backups {
        let original_name = checked_split_journal_component(&entry.original)?;
        let backup_name = checked_split_journal_component(&entry.backup)?;
        let original_utf8 = original_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction output name must be UTF-8".into())
        })?;
        let backup_utf8 = backup_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction backup name must be UTF-8".into())
        })?;
        if !transaction_backup_matches_output_family(
            &recorded_base,
            original_utf8,
            backup_utf8,
            entry.identity,
            include_recovery,
        ) || split_transaction_output_name(backup_utf8, "split-backup") != Some(original_utf8)
            || !original_names.insert(original_name.clone())
            || !backup_names.insert(backup_name.clone())
            || !backup_identities.insert(entry.identity)
        {
            return Err(FormatError::Unsupported(
                "split transaction journal contains an invalid or duplicate backup".into(),
            ));
        }
        backups.push(ResolvedSplitBackup {
            original: parent.join(original_name),
            backup: parent.join(backup_name),
            identity: entry.identity,
            state_digest: entry.state_digest,
        });
    }
    let mut outputs = Vec::with_capacity(record.outputs.len());
    for entry in &record.outputs {
        let staged_name = checked_split_journal_component(&entry.staged)?;
        let final_name = checked_split_journal_component(&entry.final_path)?;
        let staged_utf8 = staged_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction staging name must be UTF-8".into())
        })?;
        let final_utf8 = final_name.to_str().ok_or_else(|| {
            FormatError::Unsupported("split transaction output name must be UTF-8".into())
        })?;
        if !managed_split_output_name(base_name_utf8, final_utf8, include_recovery)
            || !split_staging_matches_final(base_name_utf8, staged_utf8, final_utf8)
            || !staged_names.insert(staged_name.clone())
            || !final_names.insert(final_name.clone())
            || !output_identities.insert(entry.identity)
        {
            return Err(FormatError::Unsupported(
                "split transaction journal contains an invalid or duplicate staged output".into(),
            ));
        }
        outputs.push(ResolvedSplitOutput {
            staged: parent.join(staged_name),
            final_path: parent.join(final_name),
            identity: entry.identity,
            state_digest: entry.state_digest,
        });
    }
    if outputs.is_empty() || !backup_identities.is_disjoint(&output_identities) {
        return Err(FormatError::Unsupported(
            "split transaction journal contains ambiguous output identities".into(),
        ));
    }
    Ok(ResolvedSplitTransaction {
        base: recorded_base,
        include_recovery,
        backups,
        outputs,
    })
}

fn checked_split_journal_component(name: &StoredOsString) -> Result<OsString, FormatError> {
    let name = name.to_os_string()?;
    let path = Path::new(&name);
    if path.file_name() != Some(path.as_os_str())
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err(FormatError::Unsupported(
            "split transaction journal path is not a single file name".into(),
        ));
    }
    Ok(name)
}

fn ensure_open_split_transaction_binding(open: &OpenSplitTransaction) -> Result<(), FormatError> {
    if split_file_identity(&open.file).ok() != Some(open.identity)
        || split_path_identity(&open.path).ok() != Some(open.identity)
    {
        return Err(split_transaction_conflict(
            "the retained transaction journal no longer owns its published path",
            [&open.path],
        ));
    }
    let mut reader = open.file.try_clone().map_err(|error| {
        split_transaction_conflict(
            &format!("the retained transaction journal could not be read: {error}"),
            [&open.path],
        )
    })?;
    reader.seek(SeekFrom::Start(0)).map_err(|error| {
        split_transaction_conflict(
            &format!("the retained transaction journal could not be rewound: {error}"),
            [&open.path],
        )
    })?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take((SPLIT_TRANSACTION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            split_transaction_conflict(
                &format!("the retained transaction journal could not be verified: {error}"),
                [&open.path],
            )
        })?;
    if bytes.len() > SPLIT_TRANSACTION_MAX_BYTES
        || *blake3::hash(&bytes).as_bytes() != open.content_digest
        || split_file_identity(&open.file).ok() != Some(open.identity)
        || split_path_identity(&open.path).ok() != Some(open.identity)
    {
        return Err(split_transaction_conflict(
            "the retained transaction journal changed after it was opened",
            [&open.path],
        ));
    }
    Ok(())
}

fn resume_split_transaction(
    transaction: &ResolvedSplitTransaction,
    open: &OpenSplitTransaction,
) -> Result<Vec<PreservedSplitOutput>, FormatError> {
    ensure_open_split_transaction_binding(open)?;
    let output_identities = transaction
        .outputs
        .iter()
        .map(|entry| (entry.final_path.clone(), entry.identity))
        .collect::<HashMap<_, _>>();
    for entry in &transaction.backups {
        let original = observed_split_identity(&entry.original)?;
        let backup = observed_split_identity(&entry.backup)?;
        let installed_identity = output_identities.get(&entry.original).copied();
        match (original, backup) {
            (Some(original), None) if original == entry.identity => {
                ensure_split_state_binding(&entry.original, entry.state_digest, "previous output")?;
                ensure_open_split_transaction_binding(open)?;
                if let Err(error) = crate::move_path_no_replace(&entry.original, &entry.backup) {
                    return Err(split_transaction_conflict(
                        &format!("the previous output could not be backed up: {error}"),
                        [&entry.original, &entry.backup],
                    ));
                }
                if let Err(error) = sync_directory(parent_or_current(&entry.original)) {
                    return Err(split_transaction_conflict(
                        &format!("the backup rename could not be synchronized: {error}"),
                        [&entry.original, &entry.backup],
                    ));
                }
                ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
                ensure_split_state_binding(
                    &entry.backup,
                    entry.state_digest,
                    "previous output backup",
                )?;
                ensure_split_missing(&entry.original, "previous output path")?;
            }
            (None, Some(backup)) if backup == entry.identity => {
                ensure_split_state_binding(
                    &entry.backup,
                    entry.state_digest,
                    "previous output backup",
                )?;
            }
            (Some(original), Some(backup))
                if original == entry.identity && backup == entry.identity =>
            {
                return Err(split_transaction_conflict(
                    "the previous output exists at both its original and backup paths",
                    [&entry.original, &entry.backup],
                ));
            }
            (Some(original), Some(backup))
                if Some(original) == installed_identity && backup == entry.identity =>
            {
                ensure_split_state_binding(
                    &entry.backup,
                    entry.state_digest,
                    "previous output backup",
                )?;
            }
            (Some(original), Some(backup)) => {
                return Err(split_transaction_conflict(
                    &format!("an output or backup identity changed ({original:?}, {backup:?})"),
                    [&entry.original, &entry.backup],
                ));
            }
            (None, None) => {
                return Err(split_transaction_conflict(
                    "the previous output and its transaction backup are both missing",
                    [&entry.original, &entry.backup],
                ));
            }
            (Some(original), None) => {
                return Err(split_transaction_conflict(
                    &format!("the output path is occupied by another identity ({original:?})"),
                    [&entry.original, &entry.backup],
                ));
            }
            (None, Some(backup)) => {
                return Err(split_transaction_conflict(
                    &format!("the backup path is occupied by another identity ({backup:?})"),
                    [&entry.original, &entry.backup],
                ));
            }
        }
    }

    // A resumed transaction can arrive here with some or all old outputs
    // already renamed. Rebind every backup before publishing another output.
    for entry in &transaction.backups {
        ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
        ensure_split_state_binding(&entry.backup, entry.state_digest, "previous output backup")?;
    }

    for entry in &transaction.outputs {
        let staged = observed_split_identity(&entry.staged)?;
        let final_path = observed_split_identity(&entry.final_path)?;
        match (staged, final_path) {
            (Some(staged), None) if staged == entry.identity => {
                ensure_split_state_binding(
                    &entry.staged,
                    entry.state_digest,
                    "staged split output",
                )?;
                ensure_open_split_transaction_binding(open)?;
                if let Err(error) = crate::move_path_no_replace(&entry.staged, &entry.final_path) {
                    return Err(split_transaction_conflict(
                        &format!("the staged output could not be installed: {error}"),
                        [&entry.staged, &entry.final_path],
                    ));
                }
                if let Err(error) = sync_directory(parent_or_current(&entry.final_path)) {
                    return Err(split_transaction_conflict(
                        &format!("the output rename could not be synchronized: {error}"),
                        [&entry.staged, &entry.final_path],
                    ));
                }
                ensure_split_identity(&entry.final_path, entry.identity, "installed split output")?;
                ensure_split_state_binding(
                    &entry.final_path,
                    entry.state_digest,
                    "installed split output",
                )?;
                ensure_split_missing(&entry.staged, "split staging path")?;
            }
            (None, Some(final_path)) if final_path == entry.identity => {
                ensure_split_state_binding(
                    &entry.final_path,
                    entry.state_digest,
                    "installed split output",
                )?;
            }
            (Some(staged), Some(final_path))
                if staged == entry.identity && final_path == entry.identity =>
            {
                return Err(split_transaction_conflict(
                    "the new output exists at both its staging and final paths",
                    [&entry.staged, &entry.final_path],
                ));
            }
            (Some(staged), Some(final_path)) => {
                return Err(split_transaction_conflict(
                    &format!(
                        "a staged or final output identity changed ({staged:?}, {final_path:?})"
                    ),
                    [&entry.staged, &entry.final_path],
                ));
            }
            (None, None) => {
                return Err(split_transaction_conflict(
                    "the staged and final output are both missing",
                    [&entry.staged, &entry.final_path],
                ));
            }
            (Some(staged), None) => {
                return Err(split_transaction_conflict(
                    &format!("the staging path is occupied by another identity ({staged:?})"),
                    [&entry.staged, &entry.final_path],
                ));
            }
            (None, Some(final_path)) => {
                return Err(split_transaction_conflict(
                    &format!("the final path is occupied by another identity ({final_path:?})"),
                    [&entry.staged, &entry.final_path],
                ));
            }
        }
    }

    ensure_open_split_transaction_binding(open)?;
    sync_transaction_parent(transaction)?;
    for entry in &transaction.backups {
        ensure_split_identity(&entry.backup, entry.identity, "previous output backup")?;
        ensure_split_state_binding(&entry.backup, entry.state_digest, "previous output backup")?;
        if !output_identities.contains_key(&entry.original) {
            ensure_split_missing(&entry.original, "retired previous output path")?;
        }
    }
    for entry in &transaction.outputs {
        ensure_split_identity(&entry.final_path, entry.identity, "installed split output")?;
        ensure_split_state_binding(
            &entry.final_path,
            entry.state_digest,
            "installed split output",
        )?;
        ensure_split_missing(&entry.staged, "split staging path")?;
    }
    ensure_open_split_transaction_binding(open)?;
    ensure_completed_split_family(transaction)?;
    Ok(transaction
        .backups
        .iter()
        .map(|entry| PreservedSplitOutput {
            path: entry.backup.clone(),
            identity: entry.identity,
            state_digest: entry.state_digest,
        })
        .collect())
}

fn sync_transaction_parent(transaction: &ResolvedSplitTransaction) -> Result<(), FormatError> {
    let output = transaction
        .outputs
        .first()
        .ok_or_else(|| FormatError::Other("split transaction has no outputs".into()))?;
    let parent = parent_or_current(&output.final_path);
    sync_directory(parent).map_err(|error| {
        split_transaction_conflict(
            &format!("the completed output set could not be synchronized: {error}"),
            [&output.final_path],
        )
    })
}

fn observed_split_identity(path: &Path) -> Result<Option<SplitPathIdentity>, FormatError> {
    match split_path_identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(split_transaction_conflict(
            &format!("the path identity could not be inspected: {error}"),
            [path],
        )),
    }
}

fn ensure_split_identity(
    path: &Path,
    expected: SplitPathIdentity,
    role: &str,
) -> Result<(), FormatError> {
    match observed_split_identity(path)? {
        Some(identity) if identity == expected => Ok(()),
        Some(identity) => Err(split_transaction_conflict(
            &format!("{role} identity changed ({identity:?})"),
            [path, path],
        )),
        None => Err(split_transaction_conflict(
            &format!("{role} is missing"),
            [path, path],
        )),
    }
}

fn ensure_split_state_binding(
    path: &Path,
    expected: [u8; 32],
    role: &str,
) -> Result<(), FormatError> {
    match path_state_digest(path) {
        Ok(Some(actual)) if actual == expected => Ok(()),
        Ok(Some(_)) => Err(split_transaction_conflict(
            &format!("{role} contents changed"),
            [path],
        )),
        Ok(None) => Err(split_transaction_conflict(
            &format!("{role} is missing"),
            [path],
        )),
        Err(error) => Err(split_transaction_conflict(
            &format!("{role} contents could not be verified: {error}"),
            [path],
        )),
    }
}

fn ensure_completed_split_family(
    transaction: &ResolvedSplitTransaction,
) -> Result<(), FormatError> {
    let managed = collect_managed_split_outputs(&transaction.base, transaction.include_recovery)
        .map_err(|error| {
            split_transaction_conflict(
                &format!("the completed output family could not be enumerated: {error}"),
                [&transaction.base],
            )
        })?;
    if managed.len() != transaction.outputs.len() {
        return Err(split_transaction_conflict(
            "the completed output family contains a missing or unexpected managed member",
            [&transaction.base],
        ));
    }
    for path in managed {
        let Some(output) = transaction
            .outputs
            .iter()
            .find(|output| crate::same_path_entry(&output.final_path, &path))
        else {
            return Err(split_transaction_conflict(
                "the completed output family contains an unexpected managed member",
                [&transaction.base, &path],
            ));
        };
        ensure_split_identity(&path, output.identity, "installed split output")?;
        ensure_split_state_binding(&path, output.state_digest, "installed split output")?;
    }
    Ok(())
}

fn ensure_split_missing(path: &Path, role: &str) -> Result<(), FormatError> {
    match observed_split_identity(path)? {
        None => Ok(()),
        Some(identity) => Err(split_transaction_conflict(
            &format!("{role} is occupied ({identity:?})"),
            [path, path],
        )),
    }
}

fn split_transaction_conflict<const N: usize>(reason: &str, paths: [&Path; N]) -> FormatError {
    let mut paths = paths
        .into_iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    FormatError::Io(io::Error::other(format!(
        "split output transaction requires manual recovery: {reason}; no competing path was removed or overwritten: {}",
        paths.join(", ")
    )))
}

fn clear_split_transaction(open: OpenSplitTransaction) -> Result<(), FormatError> {
    remove_bound_split_staging(&open).map_err(|error| {
        FormatError::Other(format!(
            "could not securely clear split transaction journal {}: {error}",
            open.path.display()
        ))
    })
}

fn commit_split_outputs_no_replace(staged: &[StagedSplitOutput]) -> Result<(), FormatError> {
    let first = staged
        .first()
        .ok_or_else(|| FormatError::Other("split output transaction has no outputs".into()))?;
    let parent = crate::open_parent_directory(&first.final_path)?;
    let commit = commit_split_outputs_no_replace_with(
        staged,
        &mut |from, to| crate::publish_file_no_replace_already_synced(from, to),
        &mut remove_staged_split_outputs,
    );
    let sync = parent.sync_all();
    match (commit, sync) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(FormatError::from(io::Error::new(
            error.kind(),
            format!("split output directory could not be synchronized: {error}"),
        ))),
        (Err(commit_error), Err(sync_error)) => Err(FormatError::Io(io::Error::other(format!(
            "{commit_error}; the partial split output state could not be synchronized: {sync_error}"
        )))),
    }
}

fn commit_split_outputs_no_replace_with<P, C>(
    staged: &[StagedSplitOutput],
    publish: &mut P,
    cleanup: &mut C,
) -> Result<(), FormatError>
where
    P: FnMut(&Path, &Path) -> Result<(), FormatError>,
    C: FnMut(&[StagedSplitOutput]) -> Vec<String>,
{
    if let Err(error) = validate_staged_split_outputs(staged) {
        return Err(with_split_cleanup_errors(error, cleanup(staged)));
    }

    let mut installed = Vec::with_capacity(staged.len());
    for output in staged {
        if split_file_identity(&output.file).ok() != Some(output.identity)
            || split_path_identity(&output.part).ok() != Some(output.identity)
        {
            let error = FormatError::Io(io::Error::other(format!(
                "split staging was replaced after writing and the competing path was left untouched: {}",
                output.part.display()
            )));
            return Err(with_split_cleanup_errors(error, cleanup(staged)));
        }
        if let Err(error) = publish(&output.part, &output.final_path) {
            let error = if installed.is_empty() {
                error
            } else {
                FormatError::Io(io::Error::other(format!(
                    "split output commit failed while installing the new output set: {error}; partial outputs were preserved because deleting path-bound files cannot be made race-free: {}",
                    installed
                        .iter()
                        .map(|path: &PathBuf| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            };
            return Err(with_split_cleanup_errors(error, cleanup(staged)));
        }
        if split_file_identity(&output.file).ok() != Some(output.identity)
            || split_path_identity(&output.final_path).ok() != Some(output.identity)
        {
            let error = FormatError::Io(io::Error::other(format!(
                "installed split output no longer matches the writer-owned staging file: {}",
                output.final_path.display()
            )));
            return Err(with_split_cleanup_errors(error, cleanup(staged)));
        }
        installed.push(output.final_path.clone());
    }
    Ok(())
}

#[cfg(test)]
fn commit_split_outputs_with<R, D>(
    base: &Path,
    staged: &[StagedSplitOutput],
    include_recovery: bool,
    move_no_replace: &mut R,
    remove: &mut D,
) -> Result<Vec<PathBuf>, FormatError>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
    D: FnMut(&Path) -> io::Result<()>,
{
    if let Err(error) = validate_staged_split_outputs(staged) {
        remove_staged_split_outputs_with(staged, remove);
        return Err(error);
    }

    let managed = collect_managed_split_outputs(base, include_recovery)?;
    let mut backups = Vec::with_capacity(managed.len());
    for original in managed {
        backups.push(ManagedSplitBackup {
            backup: crate::sibling_temp_path(&original, "split-backup")?,
            identity: split_path_identity(&original)?,
            original,
        });
    }

    for (backed_up, entry) in backups.iter().enumerate() {
        if let Err(error) = move_no_replace(&entry.original, &entry.backup) {
            let rollback_errors =
                rollback_split_commit(&backups[..backed_up], &[], move_no_replace);
            remove_staged_split_outputs_with(staged, remove);
            return Err(split_commit_error(
                "backing up the previous output set",
                error,
                rollback_errors,
            ));
        }
        match split_path_identity(&entry.backup) {
            Ok(identity) if identity == entry.identity => {}
            Ok(_) => {
                let mut rollback_errors = vec![format!(
                    "backup identity changed at {}; preserved for manual recovery",
                    entry.backup.display()
                )];
                rollback_errors.extend(rollback_split_commit(
                    &backups[..=backed_up],
                    &[],
                    move_no_replace,
                ));
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding the previous output backup",
                    io::Error::other("the moved backup no longer matches the original output"),
                    rollback_errors,
                ));
            }
            Err(error) => {
                let mut rollback_errors = vec![format!(
                    "could not bind backup identity at {}; preserved for manual recovery: {error}",
                    entry.backup.display()
                )];
                rollback_errors.extend(rollback_split_commit(
                    &backups[..=backed_up],
                    &[],
                    move_no_replace,
                ));
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding the previous output backup",
                    error,
                    rollback_errors,
                ));
            }
        }
    }

    let mut installed = Vec::with_capacity(staged.len());
    for output in staged {
        let identity = match (
            split_file_identity(&output.file),
            split_path_identity(&output.part),
        ) {
            (Ok(file_identity), Ok(path_identity))
                if file_identity == output.identity && path_identity == output.identity =>
            {
                output.identity
            }
            (Ok(_), Ok(_)) => {
                let rollback_errors = rollback_split_commit(&backups, &installed, move_no_replace);
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding a staged split output",
                    io::Error::other("the staged split output was replaced after writing"),
                    rollback_errors,
                ));
            }
            (Err(error), _) | (_, Err(error)) => {
                let rollback_errors = rollback_split_commit(&backups, &installed, move_no_replace);
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding a staged split output",
                    error,
                    rollback_errors,
                ));
            }
        };
        if let Err(error) = move_no_replace(&output.part, &output.final_path) {
            let rollback_errors = rollback_split_commit(&backups, &installed, move_no_replace);
            remove_staged_split_outputs_with(staged, remove);
            return Err(split_commit_error(
                "installing the new output set",
                error,
                rollback_errors,
            ));
        }
        installed.push(InstalledSplitOutput {
            final_path: output.final_path.clone(),
            identity,
        });
        match split_path_identity(&output.final_path) {
            Ok(installed_identity) if installed_identity == identity => {}
            Ok(_) => {
                let mut rollback_errors = vec![format!(
                    "installed output identity changed at {}; preserved for manual recovery",
                    output.final_path.display()
                )];
                rollback_errors.extend(rollback_split_commit(
                    &backups,
                    &installed,
                    move_no_replace,
                ));
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding an installed split output",
                    io::Error::other("the installed output no longer matches its staged file"),
                    rollback_errors,
                ));
            }
            Err(error) => {
                let mut rollback_errors = vec![format!(
                    "could not bind installed output identity at {}; preserved for manual recovery: {error}",
                    output.final_path.display()
                )];
                rollback_errors.extend(rollback_split_commit(
                    &backups,
                    &installed,
                    move_no_replace,
                ));
                remove_staged_split_outputs_with(staged, remove);
                return Err(split_commit_error(
                    "binding an installed split output",
                    error,
                    rollback_errors,
                ));
            }
        }
    }

    // Unlinking after an identity check would still leave a check/use race.
    // Return only the transaction-owned paths whose identities remain bound.
    let mut preserved = Vec::with_capacity(backups.len());
    let mut recovery = Vec::with_capacity(backups.len());
    let mut recovery_required = false;
    for entry in &backups {
        match split_path_identity(&entry.backup) {
            Ok(identity) if identity == entry.identity => {
                preserved.push(entry.backup.clone());
                recovery.push(format!("{} (previous output)", entry.backup.display()));
            }
            Ok(_) => {
                recovery_required = true;
                recovery.push(format!(
                    "{} (backup identity changed; competing entry left untouched)",
                    entry.backup.display()
                ));
            }
            Err(error) => {
                recovery_required = true;
                recovery.push(format!(
                    "{} (backup identity could not be verified; path left untouched: {error})",
                    entry.backup.display()
                ));
            }
        }
    }
    if recovery_required {
        return Err(FormatError::Io(io::Error::other(format!(
            "the new split output set was installed, but one or more transaction backups require manual recovery; no path entry was removed or overwritten: {}",
            recovery.join(", ")
        ))));
    }
    Ok(preserved)
}

fn validate_staged_split_outputs(staged: &[StagedSplitOutput]) -> Result<(), FormatError> {
    for output in staged {
        let metadata = fs::symlink_metadata(&output.part)?;
        if !metadata.file_type().is_file()
            || split_file_identity(&output.file).ok() != Some(output.identity)
            || split_path_identity(&output.part).ok() != Some(output.identity)
        {
            return Err(FormatError::Unsupported(format!(
                "split staging output changed after writing or is not a regular file: {}",
                output.part.display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn collect_managed_split_outputs(
    base: &Path,
    include_recovery: bool,
) -> Result<Vec<PathBuf>, FormatError> {
    collect_managed_split_outputs_with_checkpoint(base, include_recovery, || Ok(()))
}

pub(crate) fn collect_managed_split_outputs_with_checkpoint<C>(
    base: &Path,
    include_recovery: bool,
    mut checkpoint: C,
) -> Result<Vec<PathBuf>, FormatError>
where
    C: FnMut() -> Result<(), FormatError>,
{
    checkpoint()?;
    let mut managed = Vec::new();
    match fs::symlink_metadata(base) {
        Ok(metadata) => {
            validate_managed_split_output(base, &metadata)?;
            managed.push(base.to_path_buf());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let mut entries = fs::read_dir(parent_or_current(base))?;
    loop {
        checkpoint()?;
        let Some(entry) = entries.next() else {
            break;
        };
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let expected = expected_managed_split_output_path(base, name, include_recovery);
        let Some(expected) = expected else { continue };
        if !crate::same_path_entry(&expected, &entry.path()) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        validate_managed_split_output(&entry.path(), &metadata)?;
        managed.push(entry.path());
    }
    managed.sort();
    managed.dedup();
    Ok(managed)
}

pub(crate) fn validate_managed_split_output(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), FormatError> {
    if metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(());
    }
    Err(FormatError::Unsupported(format!(
        "managed split output must be a regular file or symlink: {}",
        path.display()
    )))
}

#[cfg(unix)]
fn split_path_identity(path: &Path) -> io::Result<SplitPathIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    Ok(SplitPathIdentity {
        filesystem: metadata.dev(),
        entry: metadata.ino(),
    })
}

#[cfg(windows)]
fn split_path_identity(path: &Path) -> io::Result<SplitPathIdentity> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let information = winapi_util::file::information(&file)?;
    Ok(SplitPathIdentity {
        filesystem: information.volume_serial_number(),
        entry: information.file_index(),
    })
}

#[cfg(not(any(unix, windows)))]
fn split_path_identity(_path: &Path) -> io::Result<SplitPathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "split output identity is unavailable on this platform",
    ))
}

#[cfg(unix)]
fn split_file_identity(file: &File) -> io::Result<SplitPathIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(SplitPathIdentity {
        filesystem: metadata.dev(),
        entry: metadata.ino(),
    })
}

#[cfg(windows)]
fn split_file_identity(file: &File) -> io::Result<SplitPathIdentity> {
    let information = winapi_util::file::information(file)?;
    Ok(SplitPathIdentity {
        filesystem: information.volume_serial_number(),
        entry: information.file_index(),
    })
}

#[cfg(not(any(unix, windows)))]
fn split_file_identity(_file: &File) -> io::Result<SplitPathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "split output identity is unavailable on this platform",
    ))
}

#[cfg(test)]
fn rollback_split_commit<R>(
    backups: &[ManagedSplitBackup],
    installed: &[InstalledSplitOutput],
    move_no_replace: &mut R,
) -> Vec<String>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
{
    let mut errors = Vec::new();
    for output in installed.iter().rev() {
        match split_path_identity(&output.final_path) {
            Ok(identity) if identity == output.identity => {
                let preserved = match crate::sibling_temp_path(
                    &output.final_path,
                    "split-rollback-preserved",
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        errors.push(format!(
                            "could not reserve a preservation path for {}; published output left in place: {error}",
                            output.final_path.display()
                        ));
                        continue;
                    }
                };
                if let Err(error) = move_no_replace(&output.final_path, &preserved) {
                    errors.push(format!(
                        "could not preserve published output {} at {}: {error}",
                        output.final_path.display(),
                        preserved.display()
                    ));
                    continue;
                }
                match split_path_identity(&preserved) {
                    Ok(identity) if identity == output.identity => errors.push(format!(
                        "published replacement output preserved at {}",
                        preserved.display()
                    )),
                    Ok(_) => errors.push(format!(
                        "published output changed during recovery; competing entry preserved at {}",
                        preserved.display()
                    )),
                    Err(error) => errors.push(format!(
                        "could not verify the entry preserved at {}: {error}",
                        preserved.display()
                    )),
                }
            }
            Ok(_) => errors.push(format!(
                "published output identity changed at {}; entry and previous backup were preserved",
                output.final_path.display()
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => errors.push(format!(
                "published output disappeared from {}; previous backup was preserved",
                output.final_path.display()
            )),
            Err(error) => errors.push(format!(
                "could not bind published output at {}; entry and previous backup were preserved: {error}",
                output.final_path.display()
            )),
        }
    }
    for entry in backups.iter().rev() {
        match split_path_identity(&entry.backup) {
            Ok(identity) if identity == entry.identity => {
                if let Err(error) = move_no_replace(&entry.backup, &entry.original) {
                    errors.push(format!(
                        "could not restore {} without replacing a competing entry; previous output preserved at {}: {error}",
                        entry.original.display(),
                        entry.backup.display()
                    ));
                    continue;
                }
                match split_path_identity(&entry.original) {
                    Ok(identity) if identity == entry.identity => {}
                    Ok(_) => errors.push(format!(
                        "backup changed during recovery; competing entry preserved at {}",
                        entry.original.display()
                    )),
                    Err(error) => errors.push(format!(
                        "could not verify the restored output at {}: {error}",
                        entry.original.display()
                    )),
                }
            }
            Ok(_) => errors.push(format!(
                "backup identity changed at {}; preserved for manual recovery",
                entry.backup.display()
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => errors.push(format!(
                "previous output backup disappeared from {}",
                entry.backup.display()
            )),
            Err(error) => errors.push(format!(
                "could not bind previous output backup at {}; preserved for manual recovery: {error}",
                entry.backup.display()
            )),
        }
    }
    errors
}

#[cfg(test)]
fn split_commit_error(phase: &str, error: io::Error, rollback_errors: Vec<String>) -> FormatError {
    if rollback_errors.is_empty() {
        return FormatError::from(io::Error::new(
            error.kind(),
            format!("split output commit failed while {phase}: {error}"),
        ));
    }
    FormatError::Io(io::Error::other(format!(
        "split output commit failed while {phase}: {error}; rollback incomplete: {}",
        rollback_errors.join("; ")
    )))
}

fn remove_staged_split_outputs(staged: &[StagedSplitOutput]) -> Vec<String> {
    staged
        .iter()
        .filter_map(|output| {
            remove_bound_split_staging(output)
                .err()
                .map(|error| error.to_string())
        })
        .collect()
}

fn remove_split_source_before_commit(
    tmp: &Path,
    source_file: &File,
    source_identity: PathIdentity,
    staged: &[StagedSplitOutput],
) -> Result<(), FormatError> {
    if let Err(error) = crate::remove_bound_temp_file(tmp, source_file, source_identity) {
        return Err(with_split_cleanup_errors(
            FormatError::Other(format!(
                "failed to securely remove complete split staging archive before commit: {error}"
            )),
            remove_staged_split_outputs(staged),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn remove_split_source_before_commit_with<D>(
    tmp: &Path,
    source_identity: PathIdentity,
    staged: &[StagedSplitOutput],
    remove: &mut D,
) -> Result<(), FormatError>
where
    D: FnMut(&Path) -> io::Result<()>,
{
    if path_identity(tmp).ok() != Some(source_identity) {
        remove_staged_split_outputs_with(staged, remove);
        return Err(FormatError::Io(io::Error::other(
            "complete split staging archive changed before removal and was left untouched",
        )));
    }
    if let Err(error) = remove(tmp) {
        remove_staged_split_outputs_with(staged, remove);
        return Err(FormatError::from(io::Error::new(
            error.kind(),
            format!("failed to remove complete split staging archive before commit: {error}"),
        )));
    }
    Ok(())
}

#[cfg(test)]
fn remove_staged_split_outputs_with<D>(staged: &[StagedSplitOutput], remove: &mut D)
where
    D: FnMut(&Path) -> io::Result<()>,
{
    for output in staged {
        if split_file_identity(&output.file).ok() == Some(output.identity)
            && split_path_identity(&output.part).ok() == Some(output.identity)
        {
            let _ = remove(&output.part);
        }
    }
}

pub(crate) fn validate_split_output_base(base: &Path) -> Result<(), FormatError> {
    match fs::symlink_metadata(base) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(FormatError::Unsupported(format!(
            "split archive base must be a regular file or symlink: {}",
            base.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Copy)]
struct SqzvHeader {
    index: u32,
    total: u32,
    uuid_hi: u64,
    uuid_lo: u64,
}

impl SqzvHeader {
    fn uuid(&self) -> (u64, u64) {
        (self.uuid_hi, self.uuid_lo)
    }
}

pub(crate) fn is_sqz_base(base: &Path) -> bool {
    base.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
}

fn prepare_sqz_for_split(file: &mut File, path: &Path) -> Result<(u64, u64), FormatError> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; SQZ_HEADER_LEN];
    file.read_exact(&mut header)?;
    if &header[0..8] != SQZ_MAGIC {
        return Err(FormatError::CorruptArchive(format!(
            "cannot write SQZV volumes for non-SQZ archive: {}",
            path.display()
        )));
    }
    let mut flags = le_u32(&header, 12..16, "SQZ header flags")?;
    flags |= SQZ_HEADER_FLAG_SPLIT;
    header[12..16].copy_from_slice(&flags.to_le_bytes());
    let crc = crc32c::crc32c(&header[..52]);
    header[52..56].copy_from_slice(&crc.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)?;
    file.sync_all()?;
    Ok((
        le_u64(&header, 16..24, "SQZ UUID high")?,
        le_u64(&header, 24..32, "SQZ UUID low")?,
    ))
}

fn sqzv_header(
    index: u64,
    total: u64,
    uuid_hi: u64,
    uuid_lo: u64,
) -> Result<[u8; SQZV_HEADER_LEN], FormatError> {
    let index: u32 = index
        .try_into()
        .map_err(|_| FormatError::Unsupported("too many SQZ volumes".into()))?;
    let total: u32 = total
        .try_into()
        .map_err(|_| FormatError::Unsupported("too many SQZ volumes".into()))?;
    let mut header = [0u8; SQZV_HEADER_LEN];
    header[0..4].copy_from_slice(SQZV_MAGIC);
    header[4..8].copy_from_slice(&index.to_le_bytes());
    header[8..12].copy_from_slice(&total.to_le_bytes());
    header[12..20].copy_from_slice(&uuid_hi.to_le_bytes());
    header[20..28].copy_from_slice(&uuid_lo.to_le_bytes());
    let crc = crc32c::crc32c(&header[..28]);
    header[28..32].copy_from_slice(&crc.to_le_bytes());
    Ok(header)
}

fn sqzr_header(
    total: u64,
    uuid_hi: u64,
    uuid_lo: u64,
    physical_volume_size: u64,
    tail_physical_len: u64,
    algorithm: u16,
) -> Result<[u8; SQZR_HEADER_LEN], FormatError> {
    let total: u32 = total
        .try_into()
        .map_err(|_| FormatError::Unsupported("too many SQZ volumes".into()))?;
    let mut header = [0u8; SQZR_HEADER_LEN];
    header[0..4].copy_from_slice(SQZR_MAGIC);
    header[4..6].copy_from_slice(&SQZR_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&algorithm.to_le_bytes());
    header[8..12].copy_from_slice(&total.to_le_bytes());
    header[12..20].copy_from_slice(&uuid_hi.to_le_bytes());
    header[20..28].copy_from_slice(&uuid_lo.to_le_bytes());
    header[28..36].copy_from_slice(&physical_volume_size.to_le_bytes());
    header[36..44].copy_from_slice(&tail_physical_len.to_le_bytes());
    header[44..52].copy_from_slice(&physical_volume_size.to_le_bytes());
    let crc = crc32c::crc32c(&header[..52]);
    header[52..56].copy_from_slice(&crc.to_le_bytes());
    Ok(header)
}

fn xor_sqzr_parity(file: &mut File, physical_offset: u64, bytes: &[u8]) -> Result<(), FormatError> {
    if bytes.is_empty() {
        return Ok(());
    }
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    let mut existing = vec![0u8; bytes.len()];
    file.read_exact(&mut existing)?;
    for (dst, src) in existing.iter_mut().zip(bytes) {
        *dst ^= *src;
    }
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    file.write_all(&existing)?;
    Ok(())
}

fn weighted_sqzr_parity(
    file: &mut File,
    physical_offset: u64,
    volume_index: u64,
    bytes: &[u8],
) -> Result<(), FormatError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let coeff = sqzr_weighted_coeff(volume_index)?;
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    let mut existing = vec![0u8; bytes.len()];
    file.read_exact(&mut existing)?;
    for (dst, src) in existing.iter_mut().zip(bytes) {
        *dst ^= gf256_mul(coeff, *src);
    }
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    file.write_all(&existing)?;
    Ok(())
}

fn quadratic_sqzr_parity(
    file: &mut File,
    physical_offset: u64,
    volume_index: u64,
    bytes: &[u8],
) -> Result<(), FormatError> {
    if bytes.is_empty() {
        return Ok(());
    }
    let coeff = sqzr_quadratic_coeff(volume_index)?;
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    let mut existing = vec![0u8; bytes.len()];
    file.read_exact(&mut existing)?;
    for (dst, src) in existing.iter_mut().zip(bytes) {
        *dst ^= gf256_mul(coeff, *src);
    }
    file.seek(SeekFrom::Start(SQZR_HEADER_LEN_U64 + physical_offset))?;
    file.write_all(&existing)?;
    Ok(())
}

fn read_sqzv_header(file: &mut File) -> Result<Option<SqzvHeader>, FormatError> {
    let mut header = [0u8; SQZV_HEADER_LEN];
    file.seek(SeekFrom::Start(0))?;
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    if &header[0..4] != SQZV_MAGIC {
        return Ok(None);
    }
    parse_sqzv_header(&header)
}

fn parse_sqzv_header(header: &[u8; SQZV_HEADER_LEN]) -> Result<Option<SqzvHeader>, FormatError> {
    if header.get(0..4) != Some(SQZV_MAGIC.as_slice()) {
        return Ok(None);
    }
    let expected = le_u32(header, 28..32, "SQZV header CRC")?;
    let actual = crc32c::crc32c(&header[..28]);
    if expected != actual {
        return Err(FormatError::CorruptArchive(
            "SQZV volume header CRC-32C mismatch".into(),
        ));
    }
    Ok(Some(SqzvHeader {
        index: le_u32(header, 4..8, "SQZV index")?,
        total: le_u32(header, 8..12, "SQZV total")?,
        uuid_hi: le_u64(header, 12..20, "SQZV UUID high")?,
        uuid_lo: le_u64(header, 20..28, "SQZV UUID low")?,
    }))
}

fn validate_sqzv_header(
    header: &SqzvHeader,
    expected_index: u32,
    expected_total: u32,
) -> Result<(), FormatError> {
    if header.index != expected_index || header.total != expected_total {
        return Err(FormatError::CorruptArchive(format!(
            "SQZV volume header mismatch: index {} of {}, expected {} of {}",
            header.index, header.total, expected_index, expected_total
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingSplitProgress {
        events: std::sync::Mutex<Vec<(u64, u64, String)>>,
    }

    impl ProgressSink for RecordingSplitProgress {
        fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
            self.events
                .lock()
                .unwrap()
                .push((done, total, current.display.clone()));
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-volumes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn reserve_test_split_staging_file(
        final_path: &Path,
        staging_id: SplitStagingId,
    ) -> (PathBuf, File) {
        let (binding, file) = reserve_split_staging_file(final_path, staging_id).unwrap();
        (binding.path, file)
    }

    #[test]
    fn parentless_volume_helpers_preserve_filename_fallbacks() {
        let base = std::path::Path::new("archive.zip");
        assert_eq!(parent_or_current(base), std::path::Path::new("."));
        assert_eq!(volume_path(base, 1), PathBuf::from("archive.zip.001"));
        assert_eq!(
            recovery_volume_path(base, 2),
            PathBuf::from("archive.zip.rev002")
        );
        assert_eq!(
            part_path(std::path::Path::new("archive.zip.001")),
            PathBuf::from("archive.zip.001.part")
        );
    }

    #[test]
    fn sqz_split_budget_preserves_the_255_volume_recovery_peak() {
        let base = Path::new("archive.sqz");
        let volume_size = 4096;
        let logical_volume_size = volume_size - SQZV_HEADER_LEN_U64;
        let at_255 = logical_volume_size * u64::from(u8::MAX);
        let at_256 = at_255 + 1;

        let layout_255 = split_layout(base, at_255, volume_size).unwrap();
        let layout_256 = split_layout(base, at_256, volume_size).unwrap();
        assert_eq!(layout_255.count, 255);
        assert_eq!(layout_256.count, 256);

        let recovery_peak = split_final_output_bytes(at_255, volume_size, layout_255);
        let upper_layout_only = split_final_output_bytes(at_256, volume_size, layout_256);
        assert!(recovery_peak > upper_layout_only);

        let budget = split_output_budget(base, at_256, volume_size).unwrap();
        assert_eq!(budget.final_output_bytes, recovery_peak);
        assert!(budget.additional_space_bytes >= budget.final_output_bytes + SPACE_SLACK);
        assert_eq!(budget.volume_count, 256);
    }

    #[test]
    fn split_budget_count_uses_the_format_logical_volume_capacity() {
        let total = 2000;
        let volume_size = 1024;

        let zip = split_output_budget(Path::new("archive.zip"), total, volume_size).unwrap();
        let sqz = split_output_budget(Path::new("archive.sqz"), total, volume_size).unwrap();

        assert_eq!(zip.volume_count, 2);
        assert_eq!(sqz.volume_count, 3);
    }

    #[test]
    fn split_staging_reservations_are_private_and_exclusive() {
        let dir = temp_dir("private-staging");
        let base = dir.join("archive.sqz");
        let final_paths = [
            volume_path(&base, 1),
            recovery_volume_path(&base, 4),
            recovery_parity_volume_path(&base),
            recovery_weighted_parity_volume_path(&base),
            recovery_quadratic_parity_volume_path(&base),
        ];
        let fixed_parts: Vec<PathBuf> = final_paths.iter().map(|path| part_path(path)).collect();
        for path in &fixed_parts {
            std::fs::write(path, b"keep").unwrap();
        }

        let staging_id = SplitStagingId::new();
        let mut reserved = Vec::new();
        for final_path in &final_paths {
            let (binding, file) = reserve_split_staging_file(final_path, staging_id).unwrap();
            let path = binding.path;
            drop(file);
            assert!(!fixed_parts.contains(&path));
            assert!(path.exists());
            reserved.push(path);
        }
        let (second_binding, second_file) =
            reserve_split_staging_file(&final_paths[0], staging_id).unwrap();
        let second_path = second_binding.path;
        drop(second_file);
        assert_ne!(reserved[0], second_path);
        reserved.push(second_path);

        reserved.sort();
        reserved.dedup();
        assert_eq!(reserved.len(), final_paths.len() + 1);
        for path in &fixed_parts {
            assert_eq!(std::fs::read(path).unwrap(), b"keep");
        }
        for path in reserved {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_staging_names_only_match_their_echoed_output_family() {
        let base = Path::new("archive.sqz");
        let volume = Path::new(".archive.sqz.001.split-stage-123-4-0.tmp.archive.sqz.001");
        let recovery = Path::new(".archive.sqz.rev001.split-stage-123-4-0.tmp.archive.sqz.rev001");

        assert!(matches_split_staging_path(base, volume, true));
        assert!(matches_split_staging_path(base, recovery, true));
        assert!(!matches_split_staging_path(base, recovery, false));
        assert!(!matches_split_staging_path(
            base,
            Path::new(".archive.sqz.001.split-stage-x-4-0.tmp.archive.sqz.001"),
            true
        ));
        assert!(!matches_split_staging_path(
            base,
            Path::new(".archive.sqz.001.split-stage-123-4-0.tmp.other.sqz.001"),
            true
        ));
    }

    #[test]
    fn split_transaction_names_only_match_well_formed_echoed_output_families() {
        let base = Path::new("archive.sqz");
        let complete_staging = Path::new(".archive.sqz.split-123-4.tmp.archive.sqz");
        let volume = Path::new(".archive.sqz.001.split-backup-123-0.tmp.archive.sqz.001");
        let rollback =
            Path::new(".archive.sqz.rev001.split-rollback-preserved-123-4.tmp.archive.sqz.rev001");

        assert!(matches_split_complete_staging_path(base, complete_staging));
        assert!(!matches_split_complete_staging_path(
            base,
            Path::new(".archive.sqz.split-x-4.tmp.archive.sqz")
        ));
        assert!(!matches_split_complete_staging_path(
            base,
            Path::new(".archive.sqz.split-123-4.tmp.other.sqz")
        ));
        assert!(!matches_split_complete_staging_path(
            base,
            Path::new(".other.sqz.split-123-4.tmp.other.sqz")
        ));
        assert!(!matches_split_complete_staging_path(
            Path::new("root/archive.sqz"),
            Path::new("root/nested/.archive.sqz.split-123-4.tmp.archive.sqz")
        ));
        assert!(matches_split_transaction_path(base, volume, true));
        assert!(matches_split_transaction_path(base, rollback, true));
        assert!(matches_split_transaction_path(
            base,
            Path::new("..archive.sqz.001.split-stage-123-4-0.tmp.archive.sqz.001.split-cleanup-123-0.tmp..archive.sqz.001.split-stage-123-4-0.tmp.archive.sqz.001"),
            true
        ));
        assert!(matches_split_transaction_path(
            base,
            Path::new("..archive.sqz.split-123-4.tmp.archive.sqz.split-cleanup-123-0.tmp..archive.sqz.split-123-4.tmp.archive.sqz"),
            true
        ));
        assert!(!matches_split_transaction_path(base, rollback, false));
        assert!(!matches_split_transaction_path(
            base,
            Path::new(".archive.sqz.001.split-backup-x-0.tmp.archive.sqz.001"),
            true
        ));
        assert!(!matches_split_transaction_path(
            base,
            Path::new(".archive.sqz.001.split-backup-123-0.tmp.archive.sqz.002"),
            true
        ));
        assert!(!matches_split_transaction_path(
            base,
            Path::new(".other.sqz.001.split-backup-123-0.tmp.other.sqz.001"),
            true
        ));
        assert!(matches_split_transaction_journal(
            base,
            Path::new(".archive.sqz.split-transaction.json")
        ));
        assert!(matches_split_transaction_journal(
            base,
            Path::new("..archive.sqz.split-transaction.json.tmp-123-4")
        ));
        assert!(matches_split_transaction_journal(
            base,
            Path::new("..archive.sqz.split-transaction.json.split-cleanup-123-0.tmp..archive.sqz.split-transaction.json")
        ));
        assert!(!matches_split_transaction_journal(
            base,
            Path::new("..archive.sqz.split-transaction.json.tmp-x-4")
        ));
    }

    #[test]
    fn split_runs_preserve_unowned_fixed_part_files() {
        let dir = temp_dir("preserve-fixed-part");
        let base = dir.join("archive.zip");
        let fixed_part = part_path(&volume_path(&base, 1));
        std::fs::write(&fixed_part, b"unowned").unwrap();

        let cancelled_tmp = dir.join("cancelled.tmp");
        std::fs::write(&cancelled_tmp, vec![1; 2048]).unwrap();
        let cancelled = ControlToken::new();
        cancelled.cancel();
        assert!(split_into_volumes(&cancelled_tmp, &base, 1024, &cancelled).is_err());
        assert_eq!(std::fs::read(&fixed_part).unwrap(), b"unowned");

        let complete_tmp = dir.join("complete.tmp");
        std::fs::write(&complete_tmp, vec![2; 2048]).unwrap();
        split_into_volumes(&complete_tmp, &base, 1024, &ControlToken::new()).unwrap();
        assert_eq!(std::fs::read(&fixed_part).unwrap(), b"unowned");
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            entry.is_ok_and(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".split-stage-")
            })
        }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_split_runs_leave_one_complete_output_set() {
        let dir = temp_dir("concurrent-output");
        let base = dir.join("archive.zip");
        let first_tmp = dir.join("first.tmp");
        let second_tmp = dir.join("second.tmp");
        let first = vec![0x11; 4096];
        let second = vec![0x22; 4096];
        std::fs::write(&first_tmp, &first).unwrap();
        std::fs::write(&second_tmp, &second).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        let first_run = {
            let barrier = barrier.clone();
            let base = base.clone();
            std::thread::spawn(move || {
                barrier.wait();
                split_into_volumes(&first_tmp, &base, 1024, &ControlToken::new())
            })
        };
        let second_run = {
            let barrier = barrier.clone();
            let base = base.clone();
            std::thread::spawn(move || {
                barrier.wait();
                split_into_volumes(&second_tmp, &base, 1024, &ControlToken::new())
            })
        };

        let first_report = first_run.join().unwrap().unwrap();
        let second_report = second_run.join().unwrap().unwrap();
        assert_eq!(
            usize::from(first_report.preserved_outputs.is_empty())
                + usize::from(second_report.preserved_outputs.is_empty()),
            1
        );
        let mut reported_backups = first_report.preserved_outputs;
        reported_backups.extend(second_report.preserved_outputs);
        reported_backups.sort();
        let mut actual_backups = split_backup_paths(&dir);
        actual_backups.sort();
        assert_eq!(reported_backups, actual_backups);
        let set = collect_volume_set(&volume_path(&base, 1)).unwrap();
        let mut reader = MultiVolumeReader::open(&set).unwrap();
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert!(output == first || output == second);
        assert!(!split_backup_paths(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_output_collection_only_includes_well_formed_sidecars() {
        let dir = temp_dir("recovery-collection-names");
        let base = dir.join("archive.sqz");
        let stale = dir.join("archive.sqz.rev004");
        let zero = dir.join("archive.sqz.rev000");
        let short = dir.join("archive.sqz.rev01");
        let notes = dir.join("archive.sqz.rev004.notes");
        for path in [&stale, &zero, &short, &notes] {
            std::fs::write(path, b"fixture").unwrap();
        }

        let managed = collect_managed_split_outputs(&base, true).unwrap();

        assert_eq!(managed, vec![stale]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_zip_output_collection_covers_generic_and_native_families() {
        let dir = temp_dir("zip-volume-family-collection");
        let base = dir.join("archive.zip");
        let generic = dir.join("archive.zip.001");
        let native_first = dir.join("archive.z01");
        let native_high = dir.join("archive.z100");
        let final_volume = base.clone();
        let lookalikes = [
            dir.join("archive.z00"),
            dir.join("archive.za1"),
            dir.join("other.z01"),
            dir.join("archive.zip.notes"),
        ];
        for path in [&generic, &native_first, &native_high, &final_volume] {
            std::fs::write(path, b"managed").unwrap();
        }
        for path in &lookalikes {
            std::fs::write(path, b"unmanaged").unwrap();
        }

        let mut managed = collect_managed_split_outputs(&base, false).unwrap();
        managed.sort();
        let mut expected = vec![generic, native_first, native_high, final_volume];
        expected.sort();
        assert_eq!(managed, expected);
        for path in lookalikes {
            assert!(path.exists());
        }
        assert!(split_staging_matches_final(
            "archive.zip",
            ".archive.z03.split-stage-1-2-0.tmp.archive.z03",
            "archive.zip"
        ));
        assert!(!split_staging_matches_final(
            "archive.zip",
            ".other.z03.split-stage-1-2-0.tmp.other.z03",
            "archive.zip"
        ));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_wim_output_collection_uses_standard_numbered_family() {
        let dir = temp_dir("wim-volume-family-collection");
        let base = dir.join("install.swm");
        let second = dir.join("install2.swm");
        let high = dir.join("install65535.swm");
        let lookalikes = [
            dir.join("install02.swm"),
            dir.join("install65536.swm"),
            dir.join("install-old2.swm"),
            dir.join("other2.swm"),
            dir.join("install2.wim"),
        ];
        for path in [&base, &second, &high] {
            std::fs::write(path, b"managed").unwrap();
        }
        for path in &lookalikes {
            std::fs::write(path, b"unmanaged").unwrap();
        }

        let mut managed = collect_managed_split_outputs(&base, false).unwrap();
        managed.sort();
        let mut expected = vec![base, second, high];
        expected.sort();
        assert_eq!(managed, expected);
        for path in lookalikes {
            assert!(path.exists());
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_output_collection_can_cancel_during_directory_enumeration() {
        let dir = temp_dir("collection-cancel");
        let base = dir.join("archive.zip");
        for index in 0..4 {
            std::fs::write(dir.join(format!("unrelated-{index}.bin")), b"fixture").unwrap();
        }
        let ctl = ControlToken::new();
        let mut checkpoints = 0usize;

        let error = collect_managed_split_outputs_with_checkpoint(&base, false, || {
            checkpoints += 1;
            if checkpoints == 3 {
                ctl.cancel();
            }
            ctl.checkpoint()
        })
        .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(checkpoints, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_rolls_back_when_backing_up_an_existing_volume_fails() {
        let dir = temp_dir("commit-backup-rollback");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_second = volume_path(&base, 2);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_second, b"old second").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let blocked = old_second.clone();
        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                if from == blocked {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected occupied output",
                    ));
                }
                crate::move_path_no_replace(from, to)
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("backing up the previous output set"));
        assert_eq!(std::fs::read(&base).unwrap(), b"old unsplit");
        assert_eq!(std::fs::read(&old_first).unwrap(), b"old first");
        assert_eq!(std::fs::read(&old_second).unwrap(), b"old second");
        assert_eq!(std::fs::read(&old_stale).unwrap(), b"old stale");
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(split_backup_paths(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_source_removal_failure_discards_parts_before_the_old_set_is_touched() {
        let dir = temp_dir("source-remove-gate");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let tmp = dir.join("archive.complete.tmp");
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&tmp, b"complete new archive").unwrap();
        let tmp_identity = path_identity(&tmp).unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let error =
            remove_split_source_before_commit_with(&tmp, tmp_identity, &staged, &mut |path| {
                if path == tmp {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected complete staging removal failure",
                    ));
                }
                std::fs::remove_file(path)
            })
            .unwrap_err();

        assert!(error.to_string().contains("before commit"));
        assert_eq!(std::fs::read(&base).unwrap(), b"old unsplit");
        assert_eq!(std::fs::read(&old_first).unwrap(), b"old first");
        assert_eq!(std::fs::read(&tmp).unwrap(), b"complete new archive");
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(split_backup_paths(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn secure_split_staging_cleanup_preserves_a_rebound_competitor() {
        let dir = temp_dir("secure-stage-cleanup-rebound");
        let base = dir.join("archive.zip");
        let staged = staged_output_fixture(&base, &[b"writer output"]);
        let displaced = dir.join("displaced-stage");
        crate::move_path_no_replace(&staged[0].part, &displaced).unwrap();
        std::fs::write(&staged[0].part, b"competitor").unwrap();

        let error = remove_bound_split_staging(&staged[0]).unwrap_err();

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(std::fs::read(&staged[0].part).unwrap(), b"competitor");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"writer output");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn secure_split_source_cleanup_preserves_a_rebound_competitor() {
        let dir = temp_dir("secure-source-cleanup-rebound");
        let base = dir.join("archive.zip");
        let tmp = dir.join("archive.complete.tmp");
        let displaced = dir.join("displaced-complete.tmp");
        std::fs::write(&tmp, b"complete writer output").unwrap();
        let source_file = open_regular_file_no_follow_read_write(&tmp).unwrap();
        let source_identity = file_identity(&source_file).unwrap();
        crate::move_path_no_replace(&tmp, &displaced).unwrap();
        std::fs::write(&tmp, b"competitor").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let error = remove_split_source_before_commit(&tmp, &source_file, source_identity, &staged)
            .unwrap_err();

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(std::fs::read(&tmp).unwrap(), b"competitor");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"complete writer output"
        );
        assert!(!staged.iter().any(|output| output.part.exists()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_replace_precheck_collision_reports_cleanup_sync_failure_and_recovered_debt() {
        let dir = temp_dir("precheck-collision-cleanup-sync");
        let base = dir.join("archive.zip");
        let tmp = dir.join("archive.complete.tmp");
        std::fs::write(&tmp, b"complete new archive").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let debt = dir.join(".archive.zip.001.split-backup-7-0.tmp.archive.zip.001");
        std::fs::write(&debt, b"previous output").unwrap();
        let recovered = vec![PreservedSplitOutput {
            path: debt.clone(),
            identity: split_path_identity(&debt).unwrap(),
            state_digest: path_state_digest(&debt).unwrap().unwrap(),
        }];
        let collision = volume_path(&base, 1);
        let tmp_identity = path_identity(&tmp).unwrap();
        let mut removed = Vec::new();
        let mut sync_calls = 0;

        let error = split_staging_failure_with(
            crate::output_exists_error(&collision),
            &tmp,
            tmp_identity,
            &staged,
            &recovered,
            &mut |path| {
                removed.push(path.to_path_buf());
                std::fs::remove_file(path)
            },
            &mut || {
                sync_calls += 1;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected parent sync failure",
                ))
            },
        );

        assert_eq!(sync_calls, 1);
        assert!(removed.contains(&tmp));
        assert!(staged.iter().all(|output| removed.contains(&output.part)));
        assert!(!tmp.exists());
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(debt.exists());
        assert!(error.to_string().contains("output already exists"));
        assert!(error.to_string().contains("not durable"));
        assert!(error.to_string().contains("injected parent sync failure"));
        assert!(error.to_string().contains(&debt.display().to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_generation_failure_reports_cleanup_sync_failure() {
        let dir = temp_dir("generation-cleanup-sync");
        let tmp = dir.join("archive.complete.tmp");
        let staged_paths = vec![
            dir.join(".archive.zip.001.split-stage-8-0-0.tmp.archive.zip.001"),
            dir.join(".archive.zip.002.split-stage-8-0-1.tmp.archive.zip.002"),
        ];
        std::fs::write(&tmp, b"complete archive").unwrap();
        for path in &staged_paths {
            std::fs::write(path, b"partial volume").unwrap();
        }
        let staged_bindings = staged_paths
            .iter()
            .map(|path| {
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .unwrap();
                SplitStagingPath {
                    path: path.clone(),
                    identity: split_file_identity(&file).unwrap(),
                    file,
                }
            })
            .collect::<Vec<_>>();
        let tmp_identity = path_identity(&tmp).unwrap();
        let mut sync_calls = 0;

        let error = split_staging_failure_with(
            FormatError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "archive shrank while splitting",
            )),
            &tmp,
            tmp_identity,
            &staged_bindings,
            &[],
            &mut |path| std::fs::remove_file(path),
            &mut || {
                sync_calls += 1;
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected generation cleanup sync failure",
                ))
            },
        );

        assert_eq!(sync_calls, 1);
        assert!(!tmp.exists());
        assert!(!staged_paths.iter().any(|path| path.exists()));
        assert!(error.to_string().contains("archive shrank while splitting"));
        assert!(error.to_string().contains("not durable"));
        assert!(error
            .to_string()
            .contains("injected generation cleanup sync failure"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_rolls_back_when_installing_a_later_volume_fails() {
        let dir = temp_dir("commit-install-rollback");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_second = volume_path(&base, 2);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_second, b"old second").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let blocked = staged[1].part.clone();
        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                if from == blocked {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected install failure",
                    ));
                }
                crate::move_path_no_replace(from, to)
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        assert!(error.to_string().contains("installing the new output set"));
        assert_eq!(std::fs::read(&base).unwrap(), b"old unsplit");
        assert_eq!(std::fs::read(&old_first).unwrap(), b"old first");
        assert_eq!(std::fs::read(&old_second).unwrap(), b"old second");
        assert_eq!(std::fs::read(&old_stale).unwrap(), b"old stale");
        assert!(!staged.iter().any(|output| output.part.exists()));
        assert!(split_backup_paths(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replace_split_rollback_preserves_an_output_replaced_after_install() {
        let dir = temp_dir("replace-rollback-output-swap");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let blocked_install = staged[1].part.clone();
        let published_first = staged[0].final_path.clone();

        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                if from == blocked_install {
                    std::fs::remove_file(&published_first)?;
                    std::fs::write(&published_first, b"competitor first")?;
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected later install failure",
                    ));
                }
                crate::move_path_no_replace(from, to)
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        let backups = split_backup_paths(&dir);
        assert!(error.to_string().contains("identity changed"));
        assert_eq!(
            std::fs::read(&published_first).unwrap(),
            b"competitor first"
        );
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"old first");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replace_split_rollback_never_overwrites_a_late_restore_conflict() {
        let dir = temp_dir("replace-rollback-restore-conflict");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let blocked_install = staged[1].part.clone();
        let restore_target = old_first.clone();

        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                if from == blocked_install {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected later install failure",
                    ));
                }
                if from.to_string_lossy().contains("split-backup") && to == restore_target {
                    std::fs::write(&restore_target, b"competitor first")?;
                }
                crate::move_path_no_replace(from, to)
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        let backups = split_backup_paths(&dir);
        assert!(error
            .to_string()
            .contains("without replacing a competing entry"));
        assert_eq!(std::fs::read(&restore_target).unwrap(), b"competitor first");
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"old first");
        assert_eq!(split_rollback_preserved_paths(&dir).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_replace_split_failure_never_removes_published_or_competing_outputs() {
        let dir = temp_dir("no-replace-identity-swap");
        let base = dir.join("archive.zip");
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let first = staged[0].final_path.clone();
        let second = staged[1].final_path.clone();
        let mut published = 0usize;

        let error = commit_split_outputs_no_replace_with(
            &staged,
            &mut |from, to| {
                published += 1;
                if published == 1 {
                    return crate::publish_file_no_replace(from, to);
                }
                std::fs::remove_file(&first)?;
                std::fs::write(&first, b"competitor first")?;
                std::fs::write(&second, b"competitor second")?;
                crate::publish_file_no_replace(from, to)
            },
            &mut |staged| {
                remove_staged_split_outputs_with(staged, &mut |path| std::fs::remove_file(path));
                Vec::new()
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("partial outputs were preserved"));
        assert_eq!(std::fs::read(&first).unwrap(), b"competitor first");
        assert_eq!(std::fs::read(&second).unwrap(), b"competitor second");
        assert!(!staged.iter().any(|output| output.part.exists()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_reports_when_the_filesystem_prevents_a_complete_rollback() {
        let dir = temp_dir("commit-incomplete-rollback");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);
        let blocked_install = staged[1].part.clone();
        let blocked_preserve = staged[0].final_path.clone();

        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                if from == blocked_install
                    || (from == blocked_preserve
                        && to.to_string_lossy().contains("split-rollback-preserved"))
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "injected move failure",
                    ));
                }
                crate::move_path_no_replace(from, to)
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        assert!(error.to_string().contains("rollback incomplete"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(split_backup_paths(&dir).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_returns_preserved_transaction_backups_after_install() {
        let dir = temp_dir("commit-cleanup-debt");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        let old_stale = volume_path(&base, 99);
        std::fs::write(&base, b"old unsplit").unwrap();
        std::fs::write(&old_first, b"old first").unwrap();
        std::fs::write(&old_stale, b"old stale").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first", b"new second"]);

        let mut preserved = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| crate::move_path_no_replace(from, to),
            &mut |path| {
                if path.to_string_lossy().contains("split-backup") {
                    panic!("transaction backups must not be removed by path");
                }
                std::fs::remove_file(path)
            },
        )
        .unwrap();

        assert_eq!(std::fs::read(volume_path(&base, 1)).unwrap(), b"new first");
        assert_eq!(std::fs::read(volume_path(&base, 2)).unwrap(), b"new second");
        assert!(!base.exists());
        assert!(!old_stale.exists());
        preserved.sort();
        let mut backups = split_backup_paths(&dir);
        backups.sort();
        assert_eq!(preserved, backups);
        assert_eq!(preserved.len(), 3);
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old unsplit"));
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old first"));
        assert!(preserved
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"old stale"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_never_deletes_a_transaction_backup_replaced_before_cleanup() {
        let dir = temp_dir("commit-cleanup-race");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first"]);
        let staged_part = staged[0].part.clone();
        let displaced_backup = dir.join("displaced-transaction-backup");
        let mut transaction_backup = None;

        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                crate::move_path_no_replace(from, to)?;
                if from == old_first {
                    transaction_backup = Some(to.to_path_buf());
                } else if from == staged_part {
                    let backup = transaction_backup
                        .as_ref()
                        .ok_or_else(|| io::Error::other("transaction backup was not captured"))?;
                    crate::move_path_no_replace(backup, &displaced_backup)?;
                    std::fs::write(backup, b"competitor backup")?;
                }
                Ok(())
            },
            &mut |path| {
                if path.to_string_lossy().contains("split-backup") {
                    panic!("a replaceable backup path must never be deleted");
                }
                std::fs::remove_file(path)
            },
        )
        .unwrap_err();

        let transaction_backup = transaction_backup.unwrap();
        assert!(error.to_string().contains("backup identity changed"));
        assert!(error.to_string().contains("competing entry left untouched"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(
            std::fs::read(&transaction_backup).unwrap(),
            b"competitor backup"
        );
        assert_eq!(std::fs::read(&displaced_backup).unwrap(), b"old first");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_commit_reports_a_transaction_backup_missing_before_return() {
        let dir = temp_dir("commit-backup-missing");
        let base = dir.join("archive.zip");
        let old_first = volume_path(&base, 1);
        std::fs::write(&old_first, b"old first").unwrap();
        let staged = staged_output_fixture(&base, &[b"new first"]);
        let staged_part = staged[0].part.clone();
        let displaced_backup = dir.join("displaced-transaction-backup");
        let mut transaction_backup = None;

        let error = commit_split_outputs_with(
            &base,
            &staged,
            false,
            &mut |from, to| {
                crate::move_path_no_replace(from, to)?;
                if from == old_first {
                    transaction_backup = Some(to.to_path_buf());
                } else if from == staged_part {
                    let backup = transaction_backup
                        .as_ref()
                        .ok_or_else(|| io::Error::other("transaction backup was not captured"))?;
                    crate::move_path_no_replace(backup, &displaced_backup)?;
                }
                Ok(())
            },
            &mut |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("backup identity could not be verified"));
        assert!(error
            .to_string()
            .contains("no path entry was removed or overwritten"));
        assert_eq!(std::fs::read(&old_first).unwrap(), b"new first");
        assert_eq!(std::fs::read(&displaced_backup).unwrap(), b"old first");
        assert!(!transaction_backup.unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_output_collection_rejects_an_abnormal_stale_volume() {
        let dir = temp_dir("commit-abnormal-stale");
        let base = dir.join("archive.zip");
        let stale = volume_path(&base, 99);
        std::fs::create_dir(&stale).unwrap();

        let error = collect_managed_split_outputs(&base, false).unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert!(stale.is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn successful_run_preserves_unowned_orphaned_split_backups() {
        let dir = temp_dir("orphaned-backup-cleanup");
        let base = dir.join("archive.zip");
        let volume = dir.join(".archive.zip.001.split-backup-123-0.tmp.archive.zip.001");
        let recovery = dir.join(".archive.zip.rev999.split-backup-123-0.tmp.archive.zip.rev999");
        let unsplit = dir.join(".archive.zip.split-backup-123-0.tmp.archive.zip");
        let other = dir.join(".other.zip.001.split-backup-123-0.tmp.other.zip.001");
        let malformed = dir.join(".archive.zip.002.split-backup-123-0.tmp.archive.zip.003");
        for path in [&volume, &recovery, &unsplit, &other, &malformed] {
            std::fs::write(path, b"old backup").unwrap();
        }

        let tmp = dir.join("new.tmp");
        std::fs::write(&tmp, vec![7u8; 2048]).unwrap();
        let artifacts = split_into_volumes(&tmp, &base, 1024, &ControlToken::new()).unwrap();

        assert!(artifacts.preserved_outputs.is_empty());
        assert!(volume.exists());
        assert!(recovery.exists());
        assert!(unsplit.exists());
        assert!(other.exists());
        assert!(malformed.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_split_replaces_the_exact_inspected_family() {
        let dir = temp_dir("guarded-success");
        let base = dir.join("archive.zip");
        let first = volume_path(&base, 1);
        let second = volume_path(&base, 2);
        std::fs::write(&first, b"old first").unwrap();
        std::fs::write(&second, b"old second").unwrap();
        let guard = crate::inspect_create_destination(&first, CreateArtifactKind::SplitArchive)
            .unwrap()
            .guard
            .unwrap();
        let tmp = dir.join("new.tmp");
        let mut expected = vec![3u8; 1024];
        expected.extend(vec![5u8; 1024]);
        std::fs::write(&tmp, &expected).unwrap();

        let artifacts = split_into_volumes_with_commit_policy(
            &tmp,
            &first,
            1024,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
        )
        .unwrap();

        assert_eq!(artifacts.volumes, vec![first.clone(), second.clone()]);
        let mut installed = std::fs::read(&first).unwrap();
        installed.extend(std::fs::read(&second).unwrap());
        assert_eq!(installed, expected);
        let mut previous = artifacts
            .preserved_outputs
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        previous.sort();
        assert_eq!(
            previous,
            vec![b"old first".to_vec(), b"old second".to_vec()]
        );
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_split_rejects_a_same_length_in_place_rewrite_before_commit() {
        let dir = temp_dir("guarded-same-length-rewrite");
        let base = dir.join("archive.zip");
        let first = volume_path(&base, 1);
        std::fs::write(&first, b"old-volume").unwrap();
        let guard = crate::inspect_create_destination(&base, CreateArtifactKind::SplitArchive)
            .unwrap()
            .guard
            .unwrap();
        std::fs::write(&first, b"new-volume").unwrap();
        let tmp = dir.join("new.tmp");
        std::fs::write(&tmp, vec![7u8; 2048]).unwrap();

        let error = split_into_volumes_with_commit_policy(
            &tmp,
            &base,
            1024,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
        )
        .unwrap_err();

        assert!(error.is_destination_changed(), "{error}");
        assert_eq!(std::fs::read(&first).unwrap(), b"new-volume");
        assert!(!tmp.exists());
        assert!(split_backup_paths(&dir).is_empty());
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_split_reports_a_post_verification_type_change_as_stale() {
        let dir = temp_dir("guarded-post-verification-type-change");
        let base = dir.join("archive.zip");
        let first = volume_path(&base, 1);
        std::fs::write(&first, b"old volume").unwrap();
        let guard = crate::inspect_create_destination(&base, CreateArtifactKind::SplitArchive)
            .unwrap()
            .guard
            .unwrap();
        let expected =
            verify_destination_guard(&base, CreateArtifactKind::SplitArchive, guard).unwrap();
        std::fs::remove_file(&first).unwrap();
        std::fs::create_dir(&first).unwrap();

        let error =
            verify_guarded_split_snapshot_after_destination_check(&base, false, expected, guard)
                .unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(error.destination_changed_path(), Some(base.as_path()));
        assert!(first.is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_split_snapshot_rejects_a_member_rewritten_after_hashing() {
        let dir = temp_dir("guarded-post-hash-rewrite");
        let base = dir.join("archive.zip");
        let first = volume_path(&base, 1);
        let second = volume_path(&base, 2);
        std::fs::write(&first, b"old first").unwrap();
        std::fs::write(&second, b"old second").unwrap();
        let original_modified = std::fs::metadata(&first).unwrap().modified().unwrap();
        let snapshot = snapshot_managed_split_outputs(&base, false).unwrap();

        let rewritten_modified = original_modified
            .checked_sub(std::time::Duration::from_secs(2))
            .or_else(|| original_modified.checked_add(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut rewritten = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&first)
            .unwrap();
        rewritten.write_all(b"new first").unwrap();
        rewritten
            .set_times(std::fs::FileTimes::new().set_modified(rewritten_modified))
            .unwrap();
        drop(rewritten);
        assert_ne!(
            std::fs::metadata(&first).unwrap().modified().unwrap(),
            original_modified
        );
        let error = verify_split_snapshot_unchanged(&base, false, &snapshot).unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(error.destination_changed_path(), Some(base.as_path()));
        assert_eq!(std::fs::read(&first).unwrap(), b"new first");
        assert_eq!(std::fs::read(&second).unwrap(), b"old second");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_explicit_first_volume_rejects_member_removal_and_addition() {
        let dir = temp_dir("guarded-explicit-member-changes");
        let base = dir.join("archive.zip");
        let first = volume_path(&base, 1);
        let second = volume_path(&base, 2);
        std::fs::write(&first, b"old first").unwrap();
        std::fs::write(&second, b"old second").unwrap();
        let removed_guard =
            crate::inspect_create_destination(&first, CreateArtifactKind::SplitArchive)
                .unwrap()
                .guard
                .unwrap();
        std::fs::remove_file(&second).unwrap();
        let removed_tmp = dir.join("removed.tmp");
        std::fs::write(&removed_tmp, vec![1u8; 2048]).unwrap();

        let removed_error = split_into_volumes_with_commit_policy(
            &removed_tmp,
            &first,
            1024,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceIfUnchanged(removed_guard),
        )
        .unwrap_err();

        assert!(removed_error.is_destination_changed());
        assert_eq!(
            removed_error.destination_changed_path(),
            Some(base.as_path())
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"old first");

        std::fs::write(&second, b"old second").unwrap();
        let added_guard =
            crate::inspect_create_destination(&first, CreateArtifactKind::SplitArchive)
                .unwrap()
                .guard
                .unwrap();
        let third = volume_path(&base, 3);
        std::fs::write(&third, b"late third").unwrap();
        let added_tmp = dir.join("added.tmp");
        std::fs::write(&added_tmp, vec![2u8; 2048]).unwrap();

        let added_error = split_into_volumes_with_commit_policy(
            &added_tmp,
            &first,
            1024,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceIfUnchanged(added_guard),
        )
        .unwrap_err();

        assert!(added_error.is_destination_changed());
        assert_eq!(std::fs::read(&first).unwrap(), b"old first");
        assert_eq!(std::fs::read(&second).unwrap(), b"old second");
        assert_eq!(std::fs::read(&third).unwrap(), b"late third");
        assert!(split_backup_paths(&dir).is_empty());
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guarded_sqz_split_binds_recovery_sidecars() {
        let dir = temp_dir("guarded-sqz-sidecar");
        let base = dir.join("archive.sqz");
        let first = volume_path(&base, 1);
        let recovery = recovery_volume_path(&base, 1);
        std::fs::write(&first, b"old sqz volume").unwrap();
        std::fs::write(&recovery, b"old recovery").unwrap();
        let guard = crate::inspect_create_destination(&first, CreateArtifactKind::SplitArchive)
            .unwrap()
            .guard
            .unwrap();
        std::fs::write(&recovery, b"new recovery").unwrap();
        let tmp = dir.join("new.tmp");
        write_test_sqz(&tmp, 2048);

        let error = split_into_volumes_with_commit_policy(
            &tmp,
            &first,
            1024,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
        )
        .unwrap_err();

        assert!(error.is_destination_changed(), "{error}");
        assert_eq!(std::fs::read(&first).unwrap(), b"old sqz volume");
        assert_eq!(std::fs::read(&recovery).unwrap(), b"new recovery");
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_transaction_recovers_a_partially_installed_set() {
        let dir = temp_dir("durable-partial-recovery");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        sync_directory(&dir).unwrap();

        let preserved = recover_split_transaction(&base).unwrap();

        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].path, backup);
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert!(!staged.exists());
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_transaction_recovers_through_a_case_alias() {
        let dir = temp_dir("durable-case-alias-recovery");
        let recorded_base = dir.join("Archive.zip");
        let requested_base = dir.join("archive.zip");
        let final_path = volume_path(&recorded_base, 1);
        let requested_final_path = volume_path(&requested_base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        if !crate::same_path_entry(&final_path, &requested_final_path) {
            // A case-only alias cannot address this entry on a case-sensitive volume.
            std::fs::remove_dir_all(dir).unwrap();
            return;
        }

        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: recorded_base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&recorded_base, &transaction).unwrap();
        drop(write_split_transaction(&recorded_base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        sync_directory(&dir).unwrap();

        let preserved = recover_split_transaction(&requested_base).unwrap();

        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].path, backup);
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert!(!staged.exists());
        assert!(!split_transaction_journal_path(&recorded_base)
            .unwrap()
            .exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_recovery_rejects_a_rewritten_backup_with_the_same_length() {
        let dir = temp_dir("durable-rewritten-backup");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old-output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new-output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        std::fs::write(&backup, b"new-backup").unwrap();
        sync_directory(&dir).unwrap();

        let error = recover_split_transaction(&base).unwrap_err();

        assert!(error.to_string().contains("contents changed"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"new-backup");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new-output");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_recovery_rejects_a_rewritten_installed_output_with_the_same_length() {
        let dir = temp_dir("durable-rewritten-output");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new-output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: Vec::new(),
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        assert_eq!(record.version, SPLIT_TRANSACTION_VERSION);
        assert_eq!(
            record.outputs[0].state_digest,
            transaction.outputs[0].state_digest
        );
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&staged, &final_path).unwrap();
        std::fs::write(&final_path, b"bad-output").unwrap();
        sync_directory(&dir).unwrap();

        let error = recover_split_transaction(&base).unwrap_err();

        assert!(error.to_string().contains("contents changed"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"bad-output");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_completion_rejects_a_late_unexpected_family_member() {
        let dir = temp_dir("durable-late-family-member");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        crate::move_path_no_replace(&staged, &final_path).unwrap();
        let late = volume_path(&base, 99);
        std::fs::write(&late, b"late competitor").unwrap();
        sync_directory(&dir).unwrap();

        let error = recover_split_transaction(&base).unwrap_err();

        assert!(error.to_string().contains("unexpected managed member"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"new output");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&late).unwrap(), b"late competitor");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_transaction_journal_collision_is_not_an_output_conflict() {
        let dir = temp_dir("split-journal-collision-category");
        let base = dir.join("archive.zip");
        let journal = split_transaction_journal_path(&base).unwrap();
        std::fs::write(&journal, b"existing transaction state").unwrap();
        let record = split_transaction_record(
            &base,
            &ResolvedSplitTransaction {
                base: base.clone(),
                include_recovery: false,
                backups: Vec::new(),
                outputs: Vec::new(),
            },
        )
        .unwrap();

        let error = write_split_transaction(&base, record).unwrap_err();

        assert!(matches!(
            &error,
            FormatError::Io(io_error) if io_error.kind() == io::ErrorKind::AlreadyExists
        ));
        assert!(!error.is_output_exists());
        assert_eq!(
            std::fs::read(&journal).unwrap(),
            b"existing transaction state"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_split_journal_publication_securely_discards_writer_owned_staging() {
        let dir = temp_dir("split-journal-collision-cleanup");
        let base = dir.join("archive.zip");
        let journal = split_transaction_journal_path(&base).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();
        let staging_id = SplitStagingId::new();
        let staged = [b"new first".as_slice(), b"new second".as_slice()]
            .into_iter()
            .enumerate()
            .map(|(index, contents)| {
                let final_path = volume_path(&base, index as u64 + 1);
                let (part, mut file) = reserve_test_split_staging_file(&final_path, staging_id);
                file.write_all(contents).unwrap();
                file.sync_all().unwrap();
                StagedSplitOutput {
                    identity: split_file_identity(&file).unwrap(),
                    part,
                    final_path,
                    file,
                }
            })
            .collect::<Vec<_>>();

        let error = commit_split_outputs(&base, &staged, false, Vec::new()).unwrap_err();

        assert!(error.to_string().contains("already exists"));
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(!staged.iter().any(|output| output.part.exists()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_journal_cleanup_preserves_a_rebound_competitor() {
        let dir = temp_dir("split-journal-cleanup-rebound");
        let base = dir.join("archive.zip");
        let record = split_transaction_record(
            &base,
            &ResolvedSplitTransaction {
                base: base.clone(),
                include_recovery: false,
                backups: Vec::new(),
                outputs: Vec::new(),
            },
        )
        .unwrap();
        let open = write_split_transaction(&base, record).unwrap();
        let journal = open.path.clone();
        let displaced = dir.join("displaced-journal");
        crate::move_path_no_replace(&journal, &displaced).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();

        let error = clear_split_transaction(open).unwrap_err();

        assert!(error.to_string().contains("left untouched"));
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(std::fs::metadata(&displaced).unwrap().len() > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rebound_split_journal_stops_before_the_first_transaction_move() {
        let dir = temp_dir("split-journal-resume-rebound");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let resolved = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_file_identity(&staged_file).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &resolved).unwrap();
        let open = write_split_transaction(&base, record).unwrap();
        let journal = open.path.clone();
        let displaced_journal = dir.join("displaced-journal");
        crate::move_path_no_replace(&journal, &displaced_journal).unwrap();
        std::fs::write(&journal, b"competing journal").unwrap();

        let error = resume_split_transaction(&resolved, &open).unwrap_err();

        assert!(error.to_string().contains("transaction journal"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old output");
        assert!(!backup.exists());
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        assert_eq!(std::fs::read(&journal).unwrap(), b"competing journal");
        assert!(std::fs::metadata(&displaced_journal).unwrap().len() > 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rewritten_split_journal_stops_before_the_first_transaction_move() {
        let dir = temp_dir("split-journal-resume-rewrite");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let resolved = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_file_identity(&staged_file).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &resolved).unwrap();
        let open = write_split_transaction(&base, record).unwrap();
        std::fs::write(&open.path, b"rewritten journal").unwrap();

        let error = resume_split_transaction(&resolved, &open).unwrap_err();

        assert!(error.to_string().contains("transaction journal changed"));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old output");
        assert!(!backup.exists());
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn next_split_run_reports_recovered_and_current_transaction_backups() {
        let dir = temp_dir("durable-recovery-debt");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"interrupted output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let old_backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: old_backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged,
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &old_backup).unwrap();
        sync_directory(&dir).unwrap();

        let tmp = dir.join("next.tmp");
        std::fs::write(&tmp, b"current output").unwrap();
        let report = split_into_volumes(&tmp, &base, 1024, &ControlToken::new()).unwrap();

        assert_eq!(report.preserved_outputs.len(), 2);
        assert!(report.preserved_outputs.contains(&old_backup));
        let mut prior_contents = report
            .preserved_outputs
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect::<Vec<_>>();
        prior_contents.sort();
        assert_eq!(
            prior_contents,
            vec![b"interrupted output".to_vec(), b"old output".to_vec()]
        );
        assert_eq!(std::fs::read(&final_path).unwrap(), b"current output");
        assert!(!split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn durable_split_recovery_leaves_competing_output_and_bound_paths_untouched() {
        let dir = temp_dir("durable-recovery-competitor");
        let base = dir.join("archive.zip");
        let final_path = volume_path(&base, 1);
        std::fs::write(&final_path, b"old output").unwrap();
        let (staged, mut staged_file) =
            reserve_test_split_staging_file(&final_path, SplitStagingId::new());
        staged_file.write_all(b"new output").unwrap();
        staged_file.sync_all().unwrap();
        drop(staged_file);
        let backup = crate::sibling_temp_path(&final_path, "split-backup").unwrap();
        let transaction = ResolvedSplitTransaction {
            base: base.clone(),
            include_recovery: false,
            backups: vec![ResolvedSplitBackup {
                identity: split_path_identity(&final_path).unwrap(),
                state_digest: path_state_digest(&final_path).unwrap().unwrap(),
                original: final_path.clone(),
                backup: backup.clone(),
            }],
            outputs: vec![ResolvedSplitOutput {
                identity: split_path_identity(&staged).unwrap(),
                state_digest: path_state_digest(&staged).unwrap().unwrap(),
                staged: staged.clone(),
                final_path: final_path.clone(),
            }],
        };
        let record = split_transaction_record(&base, &transaction).unwrap();
        drop(write_split_transaction(&base, record).unwrap());
        crate::move_path_no_replace(&final_path, &backup).unwrap();
        std::fs::write(&final_path, b"late competitor").unwrap();
        sync_directory(&dir).unwrap();

        let error = recover_split_transaction(&base).unwrap_err();

        assert!(error.to_string().contains("manual recovery"));
        assert!(error
            .to_string()
            .contains(&final_path.display().to_string()));
        assert!(error.to_string().contains(&backup.display().to_string()));
        assert_eq!(std::fs::read(&final_path).unwrap(), b"late competitor");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old output");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new output");
        assert!(split_transaction_journal_path(&base).unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn staged_output_fixture(base: &Path, contents: &[&[u8]]) -> Vec<StagedSplitOutput> {
        contents
            .iter()
            .enumerate()
            .map(|(index, contents)| {
                let final_path = volume_path(base, index as u64 + 1);
                let part = part_path(&final_path);
                std::fs::write(&part, contents).unwrap();
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&part)
                    .unwrap();
                let identity = split_file_identity(&file).unwrap();
                StagedSplitOutput {
                    part,
                    final_path,
                    identity,
                    file,
                }
            })
            .collect()
    }

    fn write_test_sqz(path: &Path, len: usize) {
        let mut bytes = vec![0x5au8; len.max(SQZ_HEADER_LEN)];
        bytes[0..8].copy_from_slice(SQZ_MAGIC);
        bytes[16..24].copy_from_slice(&17u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&29u64.to_le_bytes());
        let crc = crc32c::crc32c(&bytes[..52]);
        bytes[52..56].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    fn split_backup_paths(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains("split-backup"))
            .collect()
    }

    fn split_rollback_preserved_paths(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains("split-rollback-preserved"))
            .collect()
    }

    #[test]
    fn recovery_suffix_requires_a_positive_three_digit_index() {
        assert_eq!(
            sqz_recovery_suffix("backup.sqz.rev001"),
            Some((".rev001", 1))
        );
        assert_eq!(
            sqz_recovery_suffix("backup.sqz.rev1000"),
            Some((".rev1000", 1000))
        );
        assert_eq!(
            sqz_recovery_suffix("backup.sqz.REV001"),
            Some((".REV001", 1))
        );
        assert_eq!(sqz_recovery_suffix("backup.sqz.rev000"), None);
        assert_eq!(sqz_recovery_suffix("backup.sqz.rev01"), None);
        assert_eq!(sqz_recovery_suffix("backup.sqz.rev001.notes"), None);
    }

    #[test]
    fn split_and_reassemble_roundtrip() {
        let dir = temp_dir("roundtrip");
        let data: Vec<u8> = (0..10_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let tmp = dir.join("payload.tmp");
        std::fs::write(&tmp, &data).unwrap();
        let base = dir.join("payload.bin");
        let ctl = ControlToken::new();
        let artifacts = split_into_volumes(&tmp, &base, 9_000, &ctl).unwrap();
        let volumes = artifacts.volumes;
        assert_eq!(volumes.len(), 5); // 40_000 bytes / 9_000
        assert!(!tmp.exists(), "temp consumed");
        assert_eq!(std::fs::metadata(&volumes[0]).unwrap().len(), 9_000);
        assert_eq!(std::fs::metadata(&volumes[4]).unwrap().len(), 4_000);

        // Reassemble through the multi-volume reader, with a seek.
        let parts = collect_volume_set(&volumes[2]).unwrap();
        assert_eq!(parts.len(), 5);
        let mut reader = MultiVolumeReader::open(&parts).unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        reader.seek(SeekFrom::Start(8_998)).unwrap();
        let mut four = [0u8; 4];
        reader.read_exact(&mut four).unwrap(); // crosses a volume boundary
        assert_eq!(four, data[8_998..9_002]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn generic_split_honors_the_stream_buffer_limit() {
        let dir = temp_dir("bounded-copy-buffer");
        let data = vec![0x5a; 100_000];
        let tmp = dir.join("payload.tmp");
        std::fs::write(&tmp, &data).unwrap();
        let base = dir.join("payload.bin");
        let resources = ResourceOptions {
            threads: None,
            memory_limit: Some(ResourceOptions::MIN_STREAM_BUFFER_BYTES),
        };
        let progress = RecordingSplitProgress::default();

        let artifacts = split_into_volumes_with_commit_policy_inner(
            &tmp,
            None,
            None,
            &base,
            32 * 1024,
            &resources,
            &progress,
            &ControlToken::new(),
            CreateCommitPolicy::ReplaceExisting,
        )
        .unwrap();

        assert_eq!(artifacts.volumes.len(), 4);
        let events = progress.events.lock().unwrap();
        let mut previous = 0;
        let mut saw_full_buffer = false;
        for (done, _, _) in events.iter().filter(|(_, total, current)| {
            *total == data.len() as u64 && current.starts_with("payload.bin.")
        }) {
            let delta = done.saturating_sub(previous);
            assert!(delta <= ResourceOptions::MIN_STREAM_BUFFER_BYTES);
            saw_full_buffer |= delta == ResourceOptions::MIN_STREAM_BUFFER_BYTES;
            previous = *done;
        }
        assert!(saw_full_buffer);
        assert_eq!(previous, data.len() as u64);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_volume_is_reported() {
        let dir = temp_dir("missing");
        for i in [1u64, 2, 4] {
            std::fs::write(dir.join(format!("a.zip.{i:03}")), b"x").unwrap();
        }
        let err = collect_volume_set(&dir.join("a.zip.001")).unwrap_err();
        assert_eq!(
            err.missing_volume_path(),
            Some(dir.join("a.zip.003").as_path())
        );
        assert!(matches!(err, FormatError::CorruptArchive(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn volume_set_collection_can_cancel_during_directory_enumeration() {
        let dir = temp_dir("source-collection-cancel");
        for index in 1..=4u64 {
            std::fs::write(dir.join(format!("a.7z.{index:03}")), b"x").unwrap();
        }

        let first = dir.join("a.7z.001");
        let mut checkpoints = 0usize;
        let error = collect_volume_set_with_checkpoint(&first, || {
            checkpoints += 1;
            if checkpoints == 3 {
                return Err(FormatError::Cancelled);
            }
            Ok(())
        })
        .unwrap_err();

        assert_eq!(checkpoints, 3);
        assert!(matches!(error, FormatError::Cancelled));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn selected_missing_tail_volume_is_reported() {
        let dir = temp_dir("selected-missing-tail");
        for i in 1..=2u64 {
            std::fs::write(dir.join(format!("a.7z.{i:03}")), b"x").unwrap();
        }

        let missing = dir.join("a.7z.003");
        let err = collect_volume_set(&missing).unwrap_err();

        assert_eq!(err.missing_volume_path(), Some(missing.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sqzv_declared_volume_count_obeys_the_creation_resource_limit() {
        let logical_split_size = MIN_SPLIT_SIZE - SQZV_HEADER_LEN_U64;
        let create_error = split_layout(
            Path::new("a.sqz"),
            (MAX_SPLIT_VOLUME_COUNT + 1) * logical_split_size,
            MIN_SPLIT_SIZE,
        )
        .unwrap_err();
        assert!(matches!(
            create_error,
            FormatError::ResourceLimitExceeded(_)
        ));

        let dir = temp_dir("sqzv-volume-limit");
        let first = dir.join("a.sqz.001");
        let header = sqzv_header(1, MAX_SPLIT_VOLUME_COUNT + 1, 7, 11).unwrap();
        std::fs::write(&first, header).unwrap();

        let err = collect_volume_set(&first).unwrap_err();

        assert!(matches!(err, FormatError::ResourceLimitExceeded(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_volume_does_not_invent_an_unknown_tail() {
        let dir = temp_dir("unknown-tail");
        let first = dir.join("a.zip.001");
        std::fs::write(&first, b"opaque volume bytes").unwrap();

        let set = collect_volume_set(&first).unwrap();

        assert_eq!(set.iter().collect::<Vec<_>>(), vec![&first]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extreme_volume_suffix_reports_the_first_gap_without_large_allocation() {
        let dir = temp_dir("extreme-suffix");
        let first = dir.join("a.7z.001");
        std::fs::write(&first, b"x").unwrap();
        std::fs::write(dir.join("a.7z.4294967295"), b"x").unwrap();

        let err = collect_volume_set(&first).unwrap_err();

        assert_eq!(
            err.missing_volume_path(),
            Some(dir.join("a.7z.002").as_path())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
