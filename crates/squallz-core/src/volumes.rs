//! Split-volume support (`x.zip.001` byte-split semantics, 7-Zip style):
//! volume-set discovery, a `Read + Seek` view over the concatenated
//! volumes, and the create-side splitter.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::api::{split_volume_name, ControlToken, FormatError};

/// Split sizes below this are rejected (pathological volume counts).
pub(crate) const MIN_SPLIT_SIZE: u64 = 1024;
/// Extra free bytes required beyond the exact estimate.
const SPACE_SLACK: u64 = 1024 * 1024;
/// Copy chunk for the splitter.
const COPY_CHUNK: usize = 256 * 1024;
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

fn parent_or_current(path: &Path) -> &Path {
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn highest_present_index(present: &HashMap<u64, PathBuf>) -> u64 {
    let mut highest = 0;
    if let Some(index) = present.keys().copied().max() {
        highest = index;
    }
    highest
}

fn part_path(path: &Path) -> PathBuf {
    let name = filename_or_empty(path);
    path.with_file_name(format!("{name}.part"))
}

/// Formats the volume suffix (7-Zip convention: three digits minimum).
fn volume_path(base: &Path, index: u64) -> PathBuf {
    let name = filename_or_empty(base);
    base.with_file_name(format!("{name}.{index:03}"))
}

/// Optional SQZ tail mirror sidecar. It stores a normal SQZV volume image for
/// the tail, so the existing volume reader can validate and consume it.
fn recovery_volume_path(base: &Path, index: u64) -> PathBuf {
    let name = filename_or_empty(base);
    base.with_file_name(format!("{name}.rev{index:03}"))
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
    let base = volume_base(volume).ok_or_else(|| {
        FormatError::Unsupported(format!("not a split volume: {}", volume.display()))
    })?;
    let base_name = filename_or_empty(&base);
    // Highest index present on disk for this base.
    let mut present = HashMap::new();
    if let Ok(entries) = fs::read_dir(parent_or_current(&base)) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some((b, idx)) = split_volume_name(name) {
                    if b == base_name {
                        present.insert(u64::from(idx), entry.path());
                    }
                }
            }
        }
    }
    let max_index = highest_present_index(&present);
    if max_index == 0 {
        return Err(FormatError::CorruptArchive(format!(
            "missing volume: {}",
            volume_path(&base, 1).display()
        )));
    }

    if is_sqz_base(&base) {
        if let Some(set) = collect_sqzv_volume_set(&base, &present)? {
            return Ok(set);
        }
    }

    let mut parts = Vec::with_capacity(max_index as usize);
    for i in 1..=max_index {
        let part = volume_path(&base, i);
        if !part.is_file() {
            return Err(FormatError::CorruptArchive(format!(
                "missing volume: {}",
                part.display()
            )));
        }
        let logical_len = fs::metadata(&part)?.len();
        parts.push(VolumePart {
            path: part,
            data_offset: 0,
            logical_len,
            source: VolumePartSource::File,
        });
    }
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

fn collect_sqzv_volume_set(
    base: &Path,
    present: &HashMap<u64, PathBuf>,
) -> Result<Option<VolumeSet>, FormatError> {
    let mut headers = HashMap::new();
    let mut total = None;
    let mut uuid = None;
    for (index, path) in present {
        let mut file = File::open(path)?;
        let Some(header) = read_sqzv_header(&mut file)? else {
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
    let raw_missing: Vec<u64> = (1..=total_u64)
        .filter(|index| !headers.contains_key(index))
        .collect();
    let tail_mirror = if raw_missing.contains(&total_u64) {
        sqzv_recovery_volume_part(base, total_u64, total, uuid)?
    } else {
        None
    };
    let reconstructable_missing: Vec<u64> = raw_missing
        .iter()
        .copied()
        .filter(|index| !(*index == total_u64 && tail_mirror.is_some()))
        .collect();
    let single_parity = if !reconstructable_missing.is_empty() {
        read_sqzr_header(base, 1, SQZR_ALGO_XOR_SINGLE, total, uuid)?
    } else {
        None
    };
    let dual_parity = if reconstructable_missing.len() >= 2
        && reconstructable_missing.len() <= 3
        && total_u64 <= u64::from(u8::MAX)
    {
        read_sqzr_header(base, 2, SQZR_ALGO_XOR_WEIGHTED, total, uuid)?
    } else {
        None
    };
    let triple_parity = if reconstructable_missing.len() == 3 && total_u64 <= u64::from(u8::MAX) {
        read_sqzr_header(base, 3, SQZR_ALGO_XOR_QUADRATIC, total, uuid)?
    } else {
        None
    };
    let full_logical_len = present
        .iter()
        .filter_map(|(index, path)| {
            (*index < total_u64)
                .then(|| {
                    fs::metadata(path)
                        .ok()
                        .and_then(|meta| meta.len().checked_sub(SQZV_HEADER_LEN_U64))
                })
                .flatten()
        })
        .max();

    let mut parts = Vec::with_capacity(total as usize);
    for index in 1..=total_u64 {
        let path = volume_path(base, index);
        if let Some(header) = headers.get(&index) {
            validate_sqzv_header(header, index as u32, total)?;
            let physical_len = fs::metadata(&path)?.len();
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
fn reconstruct_sqzv_part(
    base: &Path,
    index: u64,
    total: u64,
    missing: &[u64],
    single_parity: Option<&SqzrHeader>,
    dual_parity: Option<&SqzrHeader>,
    triple_parity: Option<&SqzrHeader>,
    tail_mirror: Option<&VolumePart>,
) -> Result<Option<VolumePart>, FormatError> {
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
    pub(crate) fn open(set: &VolumeSet) -> Result<Self, FormatError> {
        let mut readers = Vec::with_capacity(set.len());
        let mut offsets = Vec::with_capacity(set.len());
        let mut data_offsets = Vec::with_capacity(set.len());
        let mut logical_lens = Vec::with_capacity(set.len());
        let mut total = 0u64;
        let mut sqzv_total = None;
        let mut sqzv_uuid = None;
        for (i, part) in set.parts().iter().enumerate() {
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

/// Splits the finished temporary archive at `tmp` into `base.001`,
/// `base.002`, ... volumes of `volume_size` bytes (the last volume holds
/// the remainder), with a disk-space pre-check and an atomic finish: parts
/// are fully written under `.part` names before being renamed into place.
/// `tmp` is consumed (deleted) on success. Returns the volume paths.
pub(crate) fn split_into_volumes(
    tmp: &Path,
    base: &Path,
    volume_size: u64,
    ctl: &ControlToken,
) -> Result<Vec<PathBuf>, FormatError> {
    let total = fs::metadata(tmp)?.len();
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
    let sqz_uuid = if write_sqzv {
        Some(prepare_sqz_for_split(tmp)?)
    } else {
        None
    };
    let write_weighted_parity = write_sqzv && count > 2 && count <= u64::from(u8::MAX);
    let write_quadratic_parity = write_sqzv && count > 3 && count <= u64::from(u8::MAX);

    // The volumes coexist with the temporary file until it is removed.
    let available = fs4::available_space(parent_or_current(base))?;
    let mut required_space = total.saturating_add(SPACE_SLACK);
    if write_sqzv {
        required_space = required_space.saturating_add(count.saturating_mul(SQZV_HEADER_LEN_U64));
        if count > 1 {
            required_space = required_space
                .saturating_add(volume_size)
                .saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            if write_weighted_parity {
                required_space =
                    required_space.saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            }
            if write_quadratic_parity {
                required_space =
                    required_space.saturating_add(SQZR_HEADER_LEN_U64.saturating_add(volume_size));
            }
        }
    }
    if available < required_space {
        return Err(FormatError::DiskFull);
    }

    let mut reader = File::open(tmp)?;
    let mut part_paths = Vec::with_capacity(count as usize);
    let mut recovery_part_path = None;
    let mut parity_part_path = None;
    let mut weighted_part_path = None;
    let mut quadratic_part_path = None;
    let result = (|| -> Result<(), FormatError> {
        let mut buf = vec![0u8; COPY_CHUNK];
        let mut parity_out = if write_sqzv && count > 1 {
            let parity_volume = recovery_parity_volume_path(base);
            let parity_part = part_path(&parity_volume);
            parity_part_path = Some(parity_part.clone());
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&parity_part)?;
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut weighted_out = if write_weighted_parity {
            let parity_volume = recovery_weighted_parity_volume_path(base);
            let parity_part = part_path(&parity_volume);
            weighted_part_path = Some(parity_part.clone());
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&parity_part)?;
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut quadratic_out = if write_quadratic_parity {
            let parity_volume = recovery_quadratic_parity_volume_path(base);
            let parity_part = part_path(&parity_volume);
            quadratic_part_path = Some(parity_part.clone());
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&parity_part)?;
            file.set_len(SQZR_HEADER_LEN_U64 + volume_size)?;
            Some(file)
        } else {
            None
        };
        let mut tail_physical_len = 0u64;
        for i in 1..=count {
            // Append `.part` (with_extension would replace the `.NNN`).
            let volume = volume_path(base, i);
            let part = part_path(&volume);
            let mut out = File::create(&part)?;
            part_paths.push(part);
            let mut recovery_out = if write_sqzv && count > 1 && i == count {
                let recovery_volume = recovery_volume_path(base, i);
                let recovery_part = part_path(&recovery_volume);
                recovery_part_path = Some(recovery_part.clone());
                Some(File::create(recovery_part)?)
            } else {
                None
            };
            if write_sqzv {
                let (uuid_hi, uuid_lo) = split_sqz_uuid(sqz_uuid, "SQZV volume header")?;
                let header = sqzv_header(i, count, uuid_hi, uuid_lo)?;
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
            let mut physical_written = if write_sqzv { SQZV_HEADER_LEN_U64 } else { 0 };
            let mut left = logical_volume_size.min(total - (i - 1) * logical_volume_size);
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
                count,
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
                count,
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
                count,
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
    if let Err(e) = result {
        for part in &part_paths {
            let _ = fs::remove_file(part);
        }
        if let Some(part) = &recovery_part_path {
            let _ = fs::remove_file(part);
        }
        if let Some(part) = &parity_part_path {
            let _ = fs::remove_file(part);
        }
        if let Some(part) = &weighted_part_path {
            let _ = fs::remove_file(part);
        }
        if let Some(part) = &quadratic_part_path {
            let _ = fs::remove_file(part);
        }
        return Err(e);
    }

    // Atomic finish: drop stale higher-numbered volumes from a previous
    // split of the same base, then rename the parts into place.
    remove_stale_volumes(base, count);
    if write_sqzv {
        remove_stale_recovery_volumes(base, if count > 1 { count } else { 0 });
    }
    let mut volumes = Vec::with_capacity(count as usize);
    for (i, part) in part_paths.iter().enumerate() {
        let final_path = volume_path(base, i as u64 + 1);
        fs::rename(part, &final_path)?;
        volumes.push(final_path);
    }
    if let Some(part) = recovery_part_path {
        let final_path = recovery_volume_path(base, count);
        let _ = fs::remove_file(&final_path);
        fs::rename(part, final_path)?;
    }
    if let Some(part) = parity_part_path {
        let final_path = recovery_parity_volume_path(base);
        let _ = fs::remove_file(&final_path);
        fs::rename(part, final_path)?;
    }
    if let Some(part) = weighted_part_path {
        let final_path = recovery_weighted_parity_volume_path(base);
        let _ = fs::remove_file(&final_path);
        fs::rename(part, final_path)?;
    }
    if let Some(part) = quadratic_part_path {
        let final_path = recovery_quadratic_parity_volume_path(base);
        let _ = fs::remove_file(&final_path);
        fs::rename(part, final_path)?;
    }
    fs::remove_file(tmp)?;
    Ok(volumes)
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

fn is_sqz_base(base: &Path) -> bool {
    base.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
}

fn prepare_sqz_for_split(path: &Path) -> Result<(u64, u64), FormatError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
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

/// Removes leftover `base.NNN` volumes with an index above `count` so a
/// re-split never leaves a corrupt mixed set behind.
fn remove_stale_volumes(base: &Path, count: u64) {
    let Some(base_name) = base.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent_or_current(base)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some((b, idx)) = split_volume_name(name) {
            if b == base_name && u64::from(idx) > count {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Removes leftover SQZ recovery sidecars that are not valid for the current
/// split set. For multi-volume SQZ, `rev001` is XOR parity, `rev002` is
/// weighted parity when count is 3..=255, `rev003` is quadratic parity when
/// count is 4..=255, and `revNNN` is the tail mirror; passing `0` removes all
/// sidecars for the base.
fn remove_stale_recovery_volumes(base: &Path, count: u64) {
    let Some(base_name) = base.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent_or_current(base)) else {
        return;
    };
    let prefix = format!("{base_name}.rev");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(index) = suffix.parse::<u64>() else {
            continue;
        };
        let keep = count > 1
            && (index == 1
                || (count > 2 && count <= u64::from(u8::MAX) && index == 2)
                || (count > 3 && count <= u64::from(u8::MAX) && index == 3)
                || index == count);
        if !keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-volumes-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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

        let present: HashMap<u64, PathBuf> = HashMap::new();
        assert_eq!(highest_present_index(&present), 0);
    }

    #[test]
    fn split_and_reassemble_roundtrip() {
        let dir = temp_dir("roundtrip");
        let data: Vec<u8> = (0..10_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let tmp = dir.join("payload.tmp");
        std::fs::write(&tmp, &data).unwrap();
        let base = dir.join("payload.bin");
        let ctl = ControlToken::new();
        let volumes = split_into_volumes(&tmp, &base, 9_000, &ctl).unwrap();
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
    fn missing_volume_is_reported() {
        let dir = temp_dir("missing");
        for i in [1u64, 2, 4] {
            std::fs::write(dir.join(format!("a.zip.{i:03}")), b"x").unwrap();
        }
        let err = collect_volume_set(&dir.join("a.zip.001")).unwrap_err();
        match err {
            FormatError::CorruptArchive(detail) => assert!(
                detail.contains("a.zip.003"),
                "detail should name the missing volume: {detail}"
            ),
            other => panic!("expected CorruptArchive, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
