//! ZIP read side: entry listing, single-entry streaming and integrity
//! testing. Extraction comes from the shared engine via the default
//! [`ArchiveReader::extract`] implementation (never overridden here).

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::{Arc, Mutex, MutexGuard};

use crc32fast::Hasher;
use flate2::read::DeflateDecoder;
use squallz_format_api::{
    ArchiveReader, ControlToken, EntryMeta, EntryPath, EntryType, FormatError, OpenOptions,
    Password, ProgressSink, ReadSeek, TestReport,
};
use zip::read::ZipFile;
use zip::{ZipArchive, ZipReadOptions};

use super::datetime::from_zip_datetime;
use super::encoding::{decode_entry_name, resolve_fallback_encoding};
use super::error::map_zip_error;

/// Opens a ZIP reader. Normal archives use the zip crate's central
/// directory reader; damaged archives that still contain local file headers
/// fall back to a limited scanner for readable entries.
pub(super) fn open(
    src: Box<dyn ReadSeek>,
    opts: &OpenOptions,
) -> Result<Box<dyn ArchiveReader>, FormatError> {
    let shared = SharedReadSeek::new(src);
    match ZipArchiveReader::open_shared(shared.clone(), opts) {
        Ok(reader) => Ok(Box::new(reader)),
        Err(original) => match original {
            FormatError::CorruptArchive(_) | FormatError::Io(_) | FormatError::Other(_) => {
                match LocalZipArchiveReader::open(shared, opts) {
                    Ok(reader) => Ok(Box::new(reader)),
                    Err(_) => Err(original),
                }
            }
            _ => Err(original),
        },
    }
}

#[derive(Clone)]
struct SharedReadSeek {
    inner: Arc<Mutex<Box<dyn ReadSeek>>>,
}

impl SharedReadSeek {
    fn new(inner: Box<dyn ReadSeek>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, Box<dyn ReadSeek>>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("ZIP shared reader lock poisoned"))
    }
}

impl Read for SharedReadSeek {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.lock()?.read(buf)
    }
}

impl Seek for SharedReadSeek {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.lock()?.seek(pos)
    }
}

/// Read handle over a ZIP archive.
pub(super) struct ZipArchiveReader {
    archive: ZipArchive<SharedReadSeek>,
    password: Option<Password>,
    /// Index-aligned decoded entry paths.
    paths: Vec<EntryPath>,
    /// Raw entry-name bytes → archive index (raw bytes are the lookup key,
    /// per the encoding model).
    index_by_raw: HashMap<Vec<u8>, usize>,
}

impl ZipArchiveReader {
    /// Opens the archive and resolves entry-name encodings up front.
    fn open_shared(src: SharedReadSeek, opts: &OpenOptions) -> Result<Self, FormatError> {
        let mut archive = ZipArchive::new(src).map_err(map_zip_error)?;
        let raw_names: Vec<Vec<u8>> = (0..archive.len())
            .map(|i| {
                archive
                    .by_index_raw(i)
                    .map(|f| f.name_raw().to_vec())
                    .map_err(map_zip_error)
            })
            .collect::<Result<_, _>>()?;
        let fallback = resolve_fallback_encoding(&raw_names, opts.encoding_override.as_deref());
        let paths: Vec<EntryPath> = raw_names
            .iter()
            .map(|raw| decode_entry_name(raw, fallback))
            .collect();
        let index_by_raw = raw_names
            .into_iter()
            .enumerate()
            .map(|(i, raw)| (raw, i))
            .collect();
        Ok(Self {
            archive,
            password: opts.password.clone(),
            paths,
            index_by_raw,
        })
    }

    /// Opens entry `idx` for decrypted, decompressed reading.
    fn open_entry(&mut self, idx: usize) -> Result<ZipFile<'_, SharedReadSeek>, FormatError> {
        let mut options = ZipReadOptions::new();
        if let Some(pw) = &self.password {
            options = options.password(Some(pw.expose().as_bytes()));
        }
        self.archive
            .by_index_with_options(idx, options)
            .map_err(map_zip_error)
    }

    fn symlink_target_or_empty(&mut self, idx: usize) -> Vec<u8> {
        let result = self.open_entry(idx).and_then(|mut f| {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            Ok(buf)
        });
        let Ok(target) = result else {
            return Vec::new();
        };
        target
    }

    /// Builds the metadata of entry `idx`. Symlink targets are stored as
    /// entry content, so they are read here (they are tiny).
    fn meta_at(&mut self, idx: usize) -> Result<EntryMeta, FormatError> {
        let path = self.paths[idx].clone();
        let file = self.archive.by_index_raw(idx).map_err(map_zip_error)?;
        let is_dir = file.is_dir();
        let is_symlink = file.is_symlink();
        let mut meta = EntryMeta {
            path,
            entry_type: if is_dir {
                EntryType::Dir
            } else {
                EntryType::File
            },
            size: file.size(),
            compressed_size: Some(file.compressed_size()),
            modified: file.last_modified().map(from_zip_datetime),
            unix_mode: file.unix_mode(),
            crc32: Some(file.crc32()),
            encrypted: file.encrypted(),
        };
        drop(file);
        if is_symlink {
            // Reading the target needs decryption/decompression; for an
            // encrypted symlink without a usable password fall back to an
            // empty target (extraction will surface PasswordRequired on
            // file entries anyway).
            let target = self.symlink_target_or_empty(idx);
            meta.entry_type = EntryType::Symlink { target };
        }
        Ok(meta)
    }
}

#[derive(Clone)]
struct LocalZipEntry {
    meta: EntryMeta,
    method: u16,
    flags: u16,
    data_offset: u64,
}

struct LocalZipArchiveReader {
    source: SharedReadSeek,
    entries: Vec<LocalZipEntry>,
    index_by_raw: HashMap<Vec<u8>, usize>,
}

impl LocalZipArchiveReader {
    fn open(mut source: SharedReadSeek, opts: &OpenOptions) -> Result<Self, FormatError> {
        let entries = scan_local_headers(&mut source, opts)?;
        if entries.is_empty() {
            return Err(FormatError::CorruptArchive(
                "no readable ZIP local headers found".into(),
            ));
        }
        let index_by_raw = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| (entry.meta.path.raw.clone(), idx))
            .collect();
        Ok(Self {
            source,
            entries,
            index_by_raw,
        })
    }
}

impl ArchiveReader for LocalZipArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.entries.iter().map(|entry| Ok(entry.meta.clone())))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        let idx = *self
            .index_by_raw
            .get(&path.raw)
            .ok_or_else(|| FormatError::Other(format!("entry not found: {path}")))?;
        let entry = &self.entries[idx];
        if entry.flags & 0x01 != 0 {
            return Err(FormatError::PasswordRequired);
        }
        let compressed_size = entry.meta.compressed_size.ok_or_else(|| {
            FormatError::CorruptArchive(format!(
                "ZIP local-header recovery entry is missing compressed size: {}",
                entry.meta.path
            ))
        })?;
        let expected_crc = entry.meta.crc32.ok_or_else(|| {
            FormatError::CorruptArchive(format!(
                "ZIP local-header recovery entry is missing CRC: {}",
                entry.meta.path
            ))
        })?;
        let reader = BoundedSharedReader {
            source: self.source.clone(),
            offset: entry.data_offset,
            remaining: compressed_size,
            positioned: false,
        };
        let expected_size = entry.meta.size;
        match entry.method {
            0 => Ok(Box::new(CrcCheckingReader::new(
                reader,
                expected_crc,
                expected_size,
            ))),
            8 => Ok(Box::new(CrcCheckingReader::new(
                DeflateDecoder::new(reader),
                expected_crc,
                expected_size,
            ))),
            method => Err(FormatError::Unsupported(format!(
                "ZIP local-header recovery does not support compression method {method}"
            ))),
        }
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut report = TestReport::default();
        let total: u64 = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.meta.entry_type, EntryType::File))
            .map(|entry| entry.meta.size)
            .sum();
        let mut done = 0u64;
        let entries = self.entries.clone();
        for entry in entries {
            ctl.checkpoint()?;
            if !matches!(entry.meta.entry_type, EntryType::File) {
                continue;
            }
            report.entries_tested += 1;
            progress.on_progress(done, total, &entry.meta.path);
            match self.read_entry(&entry.meta.path) {
                Ok(mut data) => {
                    let mut buf = [0u8; 64 * 1024];
                    loop {
                        ctl.checkpoint()?;
                        match data.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                done += n as u64;
                                progress.on_progress(done, total, &entry.meta.path);
                            }
                            Err(e) => {
                                report.problems.push(format!("{}: {e}", entry.meta.path));
                                break;
                            }
                        }
                    }
                }
                Err(e @ (FormatError::PasswordRequired | FormatError::WrongPassword)) => {
                    return Err(e);
                }
                Err(e) => report.problems.push(format!("{}: {e}", entry.meta.path)),
            }
        }
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(report)
    }
}

struct BoundedSharedReader {
    source: SharedReadSeek,
    offset: u64,
    remaining: u64,
    positioned: bool,
}

impl Read for BoundedSharedReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let mut guard = self.source.lock()?;
        if !self.positioned {
            guard.seek(SeekFrom::Start(self.offset))?;
            self.positioned = true;
        }
        let cap = buf.len().min(self.remaining as usize);
        let n = guard.read(&mut buf[..cap])?;
        self.remaining = self.remaining.saturating_sub(n as u64);
        Ok(n)
    }
}

struct CrcCheckingReader<R> {
    inner: R,
    hasher: Option<Hasher>,
    expected_crc: u32,
    expected_size: u64,
    actual_size: u64,
    verified: bool,
}

impl<R> CrcCheckingReader<R> {
    fn new(inner: R, expected_crc: u32, expected_size: u64) -> Self {
        Self {
            inner,
            hasher: Some(Hasher::new()),
            expected_crc,
            expected_size,
            actual_size: 0,
            verified: false,
        }
    }
}

impl<R: Read> Read for CrcCheckingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            if let Some(hasher) = &mut self.hasher {
                hasher.update(&buf[..n]);
            }
            self.actual_size += n as u64;
            return Ok(n);
        }
        if self.verified {
            return Ok(0);
        }
        self.verified = true;
        let actual_crc = self.hasher.take().map_or(0, |hasher| hasher.finalize());
        if self.actual_size != self.expected_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "ZIP local-header recovery size mismatch: expected {} bytes, read {} bytes",
                    self.expected_size, self.actual_size
                ),
            ));
        }
        if actual_crc != self.expected_crc {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "ZIP local-header recovery CRC mismatch: expected {:#010x}, got {:#010x}",
                    self.expected_crc, actual_crc
                ),
            ));
        }
        Ok(0)
    }
}

fn scan_local_headers(
    source: &mut SharedReadSeek,
    opts: &OpenOptions,
) -> Result<Vec<LocalZipEntry>, FormatError> {
    let len = source.seek(SeekFrom::End(0))?;
    let mut offset = 0u64;
    let mut raw_names = Vec::new();
    let mut candidates = Vec::new();
    while offset + 30 <= len {
        source.seek(SeekFrom::Start(offset))?;
        let mut fixed = [0u8; 30];
        source.read_exact(&mut fixed)?;
        if fixed[0..4] != [0x50, 0x4B, 0x03, 0x04] {
            offset += 1;
            continue;
        }
        let flags = u16::from_le_bytes([fixed[6], fixed[7]]);
        let method = u16::from_le_bytes([fixed[8], fixed[9]]);
        let crc = u32::from_le_bytes([fixed[14], fixed[15], fixed[16], fixed[17]]);
        let compressed_size = u32::from_le_bytes([fixed[18], fixed[19], fixed[20], fixed[21]]);
        let size = u32::from_le_bytes([fixed[22], fixed[23], fixed[24], fixed[25]]);
        let name_len = u16::from_le_bytes([fixed[26], fixed[27]]) as u64;
        let extra_len = u16::from_le_bytes([fixed[28], fixed[29]]) as u64;
        let Some(name_offset) = offset.checked_add(30) else {
            break;
        };
        let Some(extra_offset) = name_offset.checked_add(name_len) else {
            break;
        };
        let Some(data_offset) = extra_offset.checked_add(extra_len) else {
            break;
        };
        if name_len == 0 || data_offset > len {
            offset += 1;
            continue;
        }
        let mut raw_name = vec![0u8; name_len as usize];
        source.seek(SeekFrom::Start(name_offset))?;
        source.read_exact(&mut raw_name)?;
        let mut extra = vec![0u8; extra_len as usize];
        source.seek(SeekFrom::Start(extra_offset))?;
        source.read_exact(&mut extra)?;

        let (crc, compressed_size, size, next_offset) = if flags & 0x08 != 0 {
            let zip64_descriptor = compressed_size == u32::MAX || size == u32::MAX;
            match find_data_descriptor(source, data_offset, len, zip64_descriptor)? {
                Some(descriptor) => descriptor,
                None => break,
            }
        } else {
            let Some((compressed_size, size)) =
                resolve_zip64_local_sizes(&extra, compressed_size, size)
            else {
                break;
            };
            let Some(next_offset) = data_offset.checked_add(compressed_size) else {
                break;
            };
            if next_offset > len {
                break;
            }
            (crc, compressed_size, size, next_offset)
        };
        raw_names.push(raw_name.clone());
        candidates.push((
            raw_name,
            flags,
            method,
            crc,
            compressed_size,
            size,
            data_offset,
        ));
        offset = next_offset;
    }
    let fallback = resolve_fallback_encoding(&raw_names, opts.encoding_override.as_deref());
    Ok(candidates
        .into_iter()
        .map(
            |(raw_name, flags, method, crc, compressed_size, size, data_offset)| {
                let path = decode_entry_name(&raw_name, fallback);
                let is_dir = path.display.ends_with('/');
                LocalZipEntry {
                    meta: EntryMeta {
                        path,
                        entry_type: if is_dir {
                            EntryType::Dir
                        } else {
                            EntryType::File
                        },
                        size,
                        compressed_size: Some(compressed_size),
                        modified: None,
                        unix_mode: None,
                        crc32: Some(crc),
                        encrypted: flags & 0x01 != 0,
                    },
                    method,
                    flags,
                    data_offset,
                }
            },
        )
        .collect())
}

fn resolve_zip64_local_sizes(
    extra: &[u8],
    compressed_size_32: u32,
    size_32: u32,
) -> Option<(u64, u64)> {
    let compressed_size = (compressed_size_32 != u32::MAX).then_some(u64::from(compressed_size_32));
    let size = (size_32 != u32::MAX).then_some(u64::from(size_32));
    if let (Some(compressed_size), Some(size)) = (compressed_size, size) {
        return Some((compressed_size, size));
    }

    let mut offset = 0usize;
    while offset + 4 <= extra.len() {
        let header_id = u16::from_le_bytes([extra[offset], extra[offset + 1]]);
        let data_size = u16::from_le_bytes([extra[offset + 2], extra[offset + 3]]) as usize;
        offset += 4;
        let end = offset.checked_add(data_size)?;
        if end > extra.len() {
            return None;
        }
        if header_id == 0x0001 {
            let payload = &extra[offset..end];
            let mut payload_offset = 0usize;
            let size = if let Some(size) = size {
                size
            } else {
                read_zip64_extra_u64(payload, &mut payload_offset)?
            };
            let compressed_size = if let Some(compressed_size) = compressed_size {
                compressed_size
            } else {
                read_zip64_extra_u64(payload, &mut payload_offset)?
            };
            return Some((compressed_size, size));
        }
        offset = end;
    }
    None
}

fn read_zip64_extra_u64(payload: &[u8], offset: &mut usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let bytes = payload.get(*offset..end)?;
    *offset = end;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

fn find_data_descriptor(
    source: &mut SharedReadSeek,
    data_offset: u64,
    len: u64,
    zip64: bool,
) -> Result<Option<(u32, u64, u64, u64)>, FormatError> {
    let descriptor_len = if zip64 { 20 } else { 12 };
    let signed_descriptor_len = descriptor_len + 4;
    let mut offset = data_offset;
    while let Some(end) = offset
        .checked_add(signed_descriptor_len)
        .filter(|end| *end <= len)
    {
        source.seek(SeekFrom::Start(offset))?;
        let mut signature = [0u8; 4];
        source.read_exact(&mut signature)?;
        if signature == [0x50, 0x4B, 0x07, 0x08] {
            let mut fields = vec![0u8; descriptor_len as usize];
            source.read_exact(&mut fields)?;
            let (crc, compressed_size, size) = parse_data_descriptor_fields(&fields, zip64)?;
            if data_offset.checked_add(compressed_size) == Some(offset) {
                return Ok(Some((crc, compressed_size, size, end)));
            }
        }
        let Some(next_offset) = offset.checked_add(1) else {
            break;
        };
        offset = next_offset;
    }
    let Some(mut next_header) = data_offset.checked_add(descriptor_len) else {
        return Ok(None);
    };
    while next_header.checked_add(4).is_some_and(|end| end <= len) {
        source.seek(SeekFrom::Start(next_header))?;
        let mut signature = [0u8; 4];
        source.read_exact(&mut signature)?;
        if signature == [0x50, 0x4B, 0x03, 0x04] {
            let Some(descriptor_offset) = next_header.checked_sub(descriptor_len) else {
                let Some(after_header) = next_header.checked_add(1) else {
                    break;
                };
                next_header = after_header;
                continue;
            };
            if let Some(descriptor) = read_unsigned_data_descriptor(
                source,
                data_offset,
                descriptor_offset,
                next_header,
                zip64,
            )? {
                return Ok(Some(descriptor));
            }
        }
        let Some(after_header) = next_header.checked_add(1) else {
            break;
        };
        next_header = after_header;
    }
    if data_offset
        .checked_add(descriptor_len)
        .is_some_and(|min_descriptor_end| len >= min_descriptor_end)
    {
        return read_unsigned_data_descriptor(
            source,
            data_offset,
            len - descriptor_len,
            len,
            zip64,
        );
    }
    Ok(None)
}

fn read_unsigned_data_descriptor(
    source: &mut SharedReadSeek,
    data_offset: u64,
    descriptor_offset: u64,
    next_offset: u64,
    zip64: bool,
) -> Result<Option<(u32, u64, u64, u64)>, FormatError> {
    source.seek(SeekFrom::Start(descriptor_offset))?;
    let mut fields = vec![0u8; if zip64 { 20 } else { 12 }];
    source.read_exact(&mut fields)?;
    let (crc, compressed_size, size) = parse_data_descriptor_fields(&fields, zip64)?;
    if data_offset.checked_add(compressed_size) == Some(descriptor_offset) {
        Ok(Some((crc, compressed_size, size, next_offset)))
    } else {
        Ok(None)
    }
}

fn descriptor_field<const N: usize>(
    fields: &[u8],
    range: Range<usize>,
    field: &str,
) -> Result<[u8; N], FormatError> {
    let start = range.start;
    let end = range.end;
    let slice = fields.get(range).ok_or_else(|| {
        FormatError::CorruptArchive(format!(
            "truncated ZIP data descriptor {field}: expected bytes {start}..{end}"
        ))
    })?;
    if slice.len() != N {
        return Err(FormatError::CorruptArchive(format!(
            "invalid ZIP data descriptor {field} width: expected {N} bytes, got {}",
            slice.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

fn descriptor_u32(fields: &[u8], range: Range<usize>, field: &str) -> Result<u32, FormatError> {
    Ok(u32::from_le_bytes(descriptor_field(fields, range, field)?))
}

fn descriptor_u64(fields: &[u8], range: Range<usize>, field: &str) -> Result<u64, FormatError> {
    Ok(u64::from_le_bytes(descriptor_field(fields, range, field)?))
}

fn parse_data_descriptor_fields(
    fields: &[u8],
    zip64: bool,
) -> Result<(u32, u64, u64), FormatError> {
    let crc = descriptor_u32(fields, 0..4, "CRC")?;
    if zip64 {
        let compressed_size = descriptor_u64(fields, 4..12, "compressed size")?;
        let size = descriptor_u64(fields, 12..20, "uncompressed size")?;
        Ok((crc, compressed_size, size))
    } else {
        let compressed_size = descriptor_u32(fields, 4..8, "compressed size")?;
        let size = descriptor_u32(fields, 8..12, "uncompressed size")?;
        Ok((crc, u64::from(compressed_size), u64::from(size)))
    }
}

impl ArchiveReader for ZipArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        let len = self.archive.len();
        let mut idx = 0;
        Box::new(std::iter::from_fn(move || {
            if idx >= len {
                return None;
            }
            let item = self.meta_at(idx);
            idx += 1;
            Some(item)
        }))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        let idx = *self
            .index_by_raw
            .get(&path.raw)
            .ok_or_else(|| FormatError::Other(format!("entry not found: {path}")))?;
        Ok(Box::new(self.open_entry(idx)?))
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut report = TestReport::default();
        let total: u64 = (0..self.archive.len())
            .filter_map(|i| self.archive.by_index_raw(i).ok().map(|f| f.size()))
            .sum();
        let mut done = 0u64;
        for idx in 0..self.archive.len() {
            ctl.checkpoint()?;
            let path = self.paths[idx].clone();
            progress.on_progress(done, total, &path);
            report.entries_tested += 1;
            match self.open_entry(idx) {
                // Missing/wrong password is an input problem, not archive
                // damage: surface it as a hard error.
                Err(e @ (FormatError::PasswordRequired | FormatError::WrongPassword)) => {
                    return Err(e)
                }
                // Anything else is recorded as a per-entry problem
                // (log-only text; presentation layers localize by variant).
                Err(e) => report.problems.push(format!("{path}: {e}")),
                Ok(mut file) => {
                    // Reading to EOF drives the zip crate's CRC32 check.
                    let mut buf = [0u8; 64 * 1024];
                    loop {
                        ctl.checkpoint()?;
                        match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                done += n as u64;
                                progress.on_progress(done, total, &path);
                            }
                            Err(e) => {
                                report.problems.push(format!("{path}: {e}"));
                                break;
                            }
                        }
                    }
                }
            }
        }
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(report)
    }
}
