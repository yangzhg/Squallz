use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, BoundedProblemLog, Compressor, ControlToken, EntryMeta,
    EntryPath, EntryStreamConsumer, EntryType, FormatError, LimitsAccountant, OpenOptions,
    ProgressSink, ReadSeek, RecoverySummary, SafetyLimits, StreamFactory, TestSummary,
    TEST_PROBLEM_PREVIEW_LIMIT,
};

use crate::{sevenz::SevenZFormat, tar::TarFormat, zip::ZipFormat};

use super::{
    crc32c_bytes, empty_hash, encoding_label, fixed_array, read_array, read_exact, read_u16,
    read_u32, read_u64, read_u8, FOOTER_LEN, HEADER_LEN, INDEX_MAGIC, KIND_DIR, KIND_FILE,
    KIND_HARDLINK, KIND_OTHER, KIND_SYMLINK, MAGIC, TAIL_MAGIC, VERSION_MAJOR,
};

const VERIFY_CHUNK: usize = 64 * 1024;

mod recovery;
use recovery::{recover_footer_from_recovery_scan, RecoveryState};

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct SqzRecord {
    meta: EntryMeta,
    data_offset: u64,
    data_size: u64,
    hash: [u8; 32],
    crc32c: u32,
}

pub(super) enum SqzArchiveReader {
    EntrySet(EntrySetSqzReader),
    Inner {
        reader: Box<dyn ArchiveReader>,
        outer_recovery: Option<RecoverySummary>,
    },
}

pub(super) struct EntrySetSqzReader {
    src: Box<dyn ReadSeek>,
    records: Vec<SqzRecord>,
    recovery: Option<Arc<RecoveredPayload>>,
}

impl SqzArchiveReader {
    pub(super) fn open(
        mut src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        ctl.checkpoint()?;
        let len = src.seek(SeekFrom::End(0))?;
        if len < (HEADER_LEN + FOOTER_LEN) as u64 {
            return Err(FormatError::CorruptArchive("sqz file is too small".into()));
        }
        let (footer, recovered_recovery) = match read_footer(&mut *src, len) {
            Ok(footer) => (footer, None),
            Err(footer_err @ FormatError::CorruptArchive(_)) => {
                match recover_footer_from_recovery_scan(&mut *src, len) {
                    Ok(Some(recovered)) => recovered,
                    Ok(None) => return Err(footer_err),
                    Err(FormatError::CorruptArchive(_) | FormatError::Unsupported(_)) => {
                        return Err(footer_err)
                    }
                    Err(other) => return Err(other),
                }
            }
            Err(other) => return Err(other),
        };
        let index_end = footer
            .index_offset
            .checked_add(footer.index_length)
            .ok_or_else(|| FormatError::CorruptArchive("sqz footer index overflows".into()))?;
        if index_end > len.saturating_sub(FOOTER_LEN as u64) {
            return Err(FormatError::CorruptArchive(
                "sqz footer index points outside file".into(),
            ));
        }
        let descriptor = read_descriptor_if_present(&mut *src, &footer)?;
        let mut index = vec![0u8; footer.index_length as usize];
        src.seek(SeekFrom::Start(footer.index_offset))?;
        src.read_exact(&mut index)?;
        let recovery = match recovered_recovery {
            Some(mut recovery) => {
                recovery.repair(&mut *src)?;
                Some(recovery)
            }
            None => RecoveryState::load(&mut *src, &footer)?,
        };
        let index = match &recovery {
            Some(recovery) => recovery.index_bytes(&index)?,
            None => index,
        };
        let records = parse_index(&index)?;
        for record in &records {
            let data_end = record
                .data_offset
                .checked_add(record.data_size)
                .ok_or_else(|| FormatError::CorruptArchive("sqz entry offset overflows".into()))?;
            let inside_recovery_payload = recovery.as_ref().is_none_or(|state| {
                record
                    .data_offset
                    .checked_sub(state.payload_start)
                    .and_then(|start| start.checked_add(record.data_size))
                    .is_some_and(|end| end <= state.payload_length)
            });
            if matches!(record.meta.entry_type, EntryType::File)
                && (data_end > footer.index_offset || !inside_recovery_payload)
            {
                return Err(FormatError::CorruptArchive(format!(
                    "sqz entry points outside payload: {}",
                    record.meta.path
                )));
            }
        }
        src.seek(SeekFrom::Start(0))?;
        let recovery = recovery.map(|state| Arc::new(RecoveredPayload::from(state)));
        if let Some(recovery) = &recovery {
            if !recovery.repaired_blocks.is_empty() {
                src = Box::new(RecoveredReadSeek {
                    inner: src,
                    recovery: Arc::clone(recovery),
                    len,
                    pos: 0,
                });
            }
        }
        let reader = EntrySetSqzReader {
            src,
            records,
            recovery,
        };
        let reader = reader.open_inner_profile(&descriptor.inner_format, opts, ctl)?;
        ctl.checkpoint()?;
        Ok(reader)
    }
}

impl EntrySetSqzReader {
    fn open_inner_profile(
        self,
        inner_format: &str,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<SqzArchiveReader, FormatError> {
        match inner_format {
            "sqz" | "" => Ok(SqzArchiveReader::EntrySet(self)),
            "zip" | "tar" | "7z" | "zstd" => {
                let record = self
                    .records
                    .iter()
                    .filter(|record| matches!(record.meta.entry_type, EntryType::File))
                    .cloned()
                    .collect::<Vec<_>>();
                let [record] = record.as_slice() else {
                    return Err(FormatError::CorruptArchive(format!(
                        "sqz {inner_format} inner profile requires exactly one payload file"
                    )));
                };
                if self.record_has_unrepaired_block(record) {
                    return Err(FormatError::CorruptArchive(format!(
                        "sqz inner {inner_format} payload has unrepaired damaged data"
                    )));
                }
                let outer_recovery = self.recovery.as_ref().map(|state| state.summary.clone());
                if inner_format == "zstd" {
                    let inner =
                        TarFormat.open_stream(self.zstd_tar_stream_factory(record), opts)?;
                    return Ok(SqzArchiveReader::Inner {
                        reader: inner,
                        outer_recovery,
                    });
                }
                let inner_src: Box<dyn ReadSeek> = Box::new(BoundedReadSeek::new(
                    self.src,
                    record.data_offset,
                    record.data_size,
                ));
                let inner = match inner_format {
                    "zip" => ZipFormat.open(inner_src, opts, ctl)?,
                    "tar" => TarFormat.open(inner_src, opts, ctl)?,
                    "7z" => SevenZFormat.open(inner_src, opts, ctl)?,
                    other => {
                        return Err(FormatError::Unsupported(format!(
                            "unsupported sqz inner format: {other}"
                        )))
                    }
                };
                Ok(SqzArchiveReader::Inner {
                    reader: inner,
                    outer_recovery,
                })
            }
            other => Err(FormatError::Unsupported(format!(
                "unsupported sqz inner format: {other}"
            ))),
        }
    }

    fn record_blocks(&self, record: &SqzRecord) -> Option<(u64, u64)> {
        let recovery = self.recovery.as_ref()?;
        if record.data_size == 0 {
            return None;
        }
        let start = record.data_offset.checked_sub(recovery.payload_start)?;
        let end = start.checked_add(record.data_size - 1)?;
        let block_size = recovery.block_size as u64;
        Some((start / block_size, end / block_size))
    }

    fn zstd_tar_stream_factory(self, record: &SqzRecord) -> StreamFactory {
        let shared = Arc::new(Mutex::new(BoundedReadSeek::new(
            self.src,
            record.data_offset,
            record.data_size,
        )));
        Box::new(move || {
            {
                let mut source = shared.lock().map_err(|_| {
                    FormatError::Other("sqz zstd payload reader lock poisoned".into())
                })?;
                source.seek(SeekFrom::Start(0))?;
            }
            let compressor = crate::stream::Zstd;
            compressor.decompress_reader(Box::new(SharedBoundedRead {
                inner: Arc::clone(&shared),
            }))
        })
    }

    fn record_has_unrepaired_block(&self, record: &SqzRecord) -> bool {
        let Some((start, end)) = self.record_blocks(record) else {
            return false;
        };
        let Some(recovery) = &self.recovery else {
            return false;
        };
        (start..=end).any(|index| recovery.unrepaired_blocks.contains(&index))
    }

    fn test_with_problem_recorder(
        &mut self,
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        mut record_problem: impl FnMut(String),
    ) -> Result<u64, FormatError> {
        let total: u64 = self
            .records
            .iter()
            .filter(|record| matches!(record.meta.entry_type, EntryType::File))
            .map(|record| record.data_size)
            .fold(0, u64::saturating_add);
        let mut done = 0u64;
        let mut entries_tested = 0u64;
        let mut buffer = [0u8; VERIFY_CHUNK];
        let mut accountant = LimitsAccountant::new(*limits);
        for record in self.records.clone() {
            ctl.checkpoint()?;
            accountant.check_entry(&record.meta)?;
            entries_tested += 1;
            if !matches!(record.meta.entry_type, EntryType::File) {
                progress.on_progress(done, total, &record.meta.path);
                continue;
            }
            if self.record_has_unrepaired_block(&record) {
                record_problem(format!(
                    "{}: unrepaired SQZ recovery block damage",
                    record.meta.path
                ));
                progress.on_progress(done, total, &record.meta.path);
                continue;
            }
            let verified = (|| {
                self.src.seek(SeekFrom::Start(record.data_offset))?;
                let mut data = EntryData {
                    inner: &mut *self.src,
                    remaining: record.data_size,
                };
                let mut hash = blake3::Hasher::new();
                let mut crc = 0;
                let mut entry_output_bytes = 0;
                loop {
                    ctl.checkpoint()?;
                    let count = match data.read(&mut buffer) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        result => result?,
                    };
                    if count == 0 {
                        break;
                    }
                    accountant.add_entry_output_bytes(
                        &record.meta,
                        &mut entry_output_bytes,
                        count as u64,
                    )?;
                    hash.update(&buffer[..count]);
                    crc = crc32c::crc32c_append(crc, &buffer[..count]);
                    done = done.saturating_add(count as u64);
                    progress.on_progress(done, total, &record.meta.path);
                }
                Ok::<_, FormatError>(
                    *hash.finalize().as_bytes() == record.hash && crc == record.crc32c,
                )
            })();
            ctl.checkpoint()?;
            match verified {
                Ok(valid) => {
                    if !valid {
                        record_problem(format!(
                            "{}: checksum mismatch (BLAKE3/CRC-32C)",
                            record.meta.path
                        ));
                    }
                }
                Err(error @ (FormatError::Cancelled | FormatError::ResourceLimitExceeded(_))) => {
                    return Err(error)
                }
                Err(e) => record_problem(format!("{}: {e}", record.meta.path)),
            }
        }
        let total = if total == 0 { done } else { total };
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(entries_tested)
    }
}

impl ArchiveReader for EntrySetSqzReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.records.iter().map(|record| Ok(record.meta.clone())))
    }

    fn consume_entries(
        mut self: Box<Self>,
        visitor: &mut dyn FnMut(EntryMeta) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        for record in std::mem::take(&mut self.records) {
            visitor(record.meta)?;
        }
        Ok(())
    }

    fn read_entry(
        &mut self,
        path: &EntryPath,
        consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        let record = self
            .records
            .iter()
            .find(|record| record.meta.path.raw == path.raw)
            .cloned()
            .ok_or_else(|| FormatError::Other(format!("entry not found: {path}")))?;
        if !matches!(record.meta.entry_type, EntryType::File) {
            return Err(FormatError::Unsupported(format!(
                "sqz entry is not a file: {path}"
            )));
        }
        if self.record_has_unrepaired_block(&record) {
            return Err(FormatError::CorruptArchive(format!(
                "sqz entry has unrepaired damaged data: {path}"
            )));
        }
        self.src.seek(SeekFrom::Start(record.data_offset))?;
        consume(&mut EntryData {
            inner: &mut *self.src,
            remaining: record.data_size,
        })
    }

    fn test_summary(
        &mut self,
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let recovery = self.recovery.as_ref().map(|state| state.summary.clone());
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries_tested = self.test_with_problem_recorder(limits, progress, ctl, |problem| {
            problems.record(problem)
        })?;
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery,
        })
    }
}

impl ArchiveReader for SqzArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.entries(),
            SqzArchiveReader::Inner { reader, .. } => reader.entries(),
        }
    }

    fn consume_entries(
        self: Box<Self>,
        visitor: &mut dyn FnMut(EntryMeta) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        match *self {
            SqzArchiveReader::EntrySet(reader) => Box::new(reader).consume_entries(visitor),
            SqzArchiveReader::Inner { reader, .. } => reader.consume_entries(visitor),
        }
    }

    fn read_entry(
        &mut self,
        path: &EntryPath,
        consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.read_entry(path, consume),
            SqzArchiveReader::Inner { reader, .. } => reader.read_entry(path, consume),
        }
    }

    fn read_entries(
        &mut self,
        entries: &[EntryMeta],
        consume: &mut EntryStreamConsumer<'_>,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.read_entries(entries, consume, ctl),
            SqzArchiveReader::Inner { reader, .. } => reader.read_entries(entries, consume, ctl),
        }
    }

    fn test_summary(
        &mut self,
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.test_summary(limits, progress, ctl),
            SqzArchiveReader::Inner {
                reader,
                outer_recovery,
            } => {
                let mut report = reader.test_summary(limits, progress, ctl)?;
                if report.recovery.is_none() {
                    report.recovery = outer_recovery.clone();
                }
                Ok(report)
            }
        }
    }
}

struct RecoveredPayload {
    summary: RecoverySummary,
    payload_start: u64,
    payload_length: u64,
    block_size: usize,
    repaired_blocks: HashMap<u64, Vec<u8>>,
    unrepaired_blocks: HashSet<u64>,
}

impl From<RecoveryState> for RecoveredPayload {
    fn from(state: RecoveryState) -> Self {
        Self {
            summary: state.summary(),
            payload_start: state.payload_start,
            payload_length: state.payload_length,
            block_size: state.block_size,
            repaired_blocks: state.repaired_blocks,
            unrepaired_blocks: state.unrepaired_blocks,
        }
    }
}

struct RecoveredReadSeek {
    inner: Box<dyn ReadSeek>,
    recovery: Arc<RecoveredPayload>,
    len: u64,
    pos: u64,
}

impl Read for RecoveredReadSeek {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let mut want = (self.len - self.pos).min(buf.len() as u64) as usize;
        if let Some(relative) = self.pos.checked_sub(self.recovery.payload_start) {
            if relative < self.recovery.payload_length {
                let block_size = self.recovery.block_size as u64;
                let block_index = relative / block_size;
                let block_offset = (relative % block_size) as usize;
                want = (self.recovery.payload_length - relative)
                    .min(want as u64)
                    .min(block_size - block_offset as u64) as usize;
                if self.recovery.unrepaired_blocks.contains(&block_index) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "SQZ payload exceeds recovery capacity",
                    ));
                }
                if let Some(block) = self.recovery.repaired_blocks.get(&block_index) {
                    let bytes = block
                        .get(block_offset..block_offset + want)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "truncated SQZ repaired block",
                            )
                        })?;
                    buf[..want].copy_from_slice(bytes);
                    self.pos += want as u64;
                    return Ok(want);
                }
            }
        } else {
            want = (self.recovery.payload_start - self.pos).min(want as u64) as usize;
        }
        self.inner.seek(SeekFrom::Start(self.pos))?;
        let count = self.inner.read(&mut buf[..want])?;
        self.pos += count as u64;
        Ok(count)
    }
}

impl Seek for RecoveredReadSeek {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.pos) + i128::from(offset),
        };
        if next < 0 || next > i128::from(u64::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SQZ payload seek",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

struct BoundedReadSeek {
    inner: Box<dyn ReadSeek>,
    start: u64,
    len: u64,
    pos: u64,
}

impl BoundedReadSeek {
    fn new(inner: Box<dyn ReadSeek>, start: u64, len: u64) -> Self {
        Self {
            inner,
            start,
            len,
            pos: 0,
        }
    }
}

impl Read for BoundedReadSeek {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.len - self.pos;
        let want = remaining.min(buf.len() as u64) as usize;
        let absolute = self.start.checked_add(self.pos).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "bounded SQZ payload offset overflow",
            )
        })?;
        self.inner.seek(SeekFrom::Start(absolute))?;
        let n = self.inner.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for BoundedReadSeek {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.pos) + i128::from(offset),
        };
        if next < 0 || next > i128::from(self.len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside bounded SQZ payload",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

struct SharedBoundedRead {
    inner: Arc<Mutex<BoundedReadSeek>>,
}

impl Read for SharedBoundedRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("sqz zstd payload reader lock poisoned"))?;
        inner.read(buf)
    }
}

struct EntryData<'a> {
    inner: &'a mut dyn ReadSeek,
    remaining: u64,
}

impl Read for EntryData<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let want = self.remaining.min(buf.len() as u64) as usize;
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated SQZ file data",
            ));
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

struct Footer {
    index_offset: u64,
    index_length: u64,
    recovery_offset: u64,
    recovery_length: u64,
    uuid_hi: u64,
    uuid_lo: u64,
}

fn read_footer(src: &mut dyn ReadSeek, len: u64) -> Result<Footer, FormatError> {
    let mut footer = [0u8; FOOTER_LEN];
    src.seek(SeekFrom::Start(len - FOOTER_LEN as u64))?;
    src.read_exact(&mut footer)?;
    if &footer[56..64] != TAIL_MAGIC {
        return Err(FormatError::CorruptArchive(
            "missing sqz footer magic".into(),
        ));
    }
    let expected = u32::from_le_bytes(fixed_array::<4>(&footer[48..52], "sqz footer crc")?);
    let actual = crc32c_bytes(&footer[..48]);
    if expected != actual {
        return Err(FormatError::CorruptArchive(
            "sqz footer CRC-32C mismatch".into(),
        ));
    }
    Ok(Footer {
        index_offset: u64::from_le_bytes(fixed_array::<8>(
            &footer[0..8],
            "sqz footer index offset",
        )?),
        index_length: u64::from_le_bytes(fixed_array::<8>(
            &footer[8..16],
            "sqz footer index length",
        )?),
        recovery_offset: u64::from_le_bytes(fixed_array::<8>(
            &footer[16..24],
            "sqz footer recovery offset",
        )?),
        recovery_length: u64::from_le_bytes(fixed_array::<8>(
            &footer[24..32],
            "sqz footer recovery length",
        )?),
        uuid_hi: u64::from_le_bytes(fixed_array::<8>(&footer[32..40], "sqz footer uuid hi")?),
        uuid_lo: u64::from_le_bytes(fixed_array::<8>(&footer[40..48], "sqz footer uuid lo")?),
    })
}

fn valid_header_uuid(src: &mut dyn ReadSeek) -> Result<Option<(u64, u64)>, FormatError> {
    let mut header = [0u8; HEADER_LEN];
    src.seek(SeekFrom::Start(0))?;
    src.read_exact(&mut header)?;
    if &header[0..8] != MAGIC {
        return Ok(None);
    }
    let expected = u32::from_le_bytes(fixed_array::<4>(&header[52..56], "sqz header crc")?);
    let actual = crc32c_bytes(&header[..52]);
    if expected != actual {
        return Ok(None);
    }
    let major = u16::from_le_bytes(fixed_array::<2>(&header[8..10], "sqz header version")?);
    if major != VERSION_MAJOR {
        return Err(FormatError::Unsupported(format!(
            "unsupported sqz major version: {major}"
        )));
    }
    Ok(Some((
        u64::from_le_bytes(fixed_array::<8>(&header[16..24], "sqz header uuid hi")?),
        u64::from_le_bytes(fixed_array::<8>(&header[24..32], "sqz header uuid lo")?),
    )))
}

struct SqzDescriptor {
    inner_format: String,
}

impl Default for SqzDescriptor {
    fn default() -> Self {
        Self {
            inner_format: "sqz".into(),
        }
    }
}

fn read_descriptor_if_present(
    src: &mut dyn ReadSeek,
    footer: &Footer,
) -> Result<SqzDescriptor, FormatError> {
    let mut header = [0u8; HEADER_LEN];
    src.seek(SeekFrom::Start(0))?;
    src.read_exact(&mut header)?;
    if &header[0..8] != MAGIC {
        return Ok(SqzDescriptor::default());
    }
    let expected = u32::from_le_bytes(fixed_array::<4>(&header[52..56], "sqz header crc")?);
    let actual = crc32c_bytes(&header[..52]);
    if expected != actual {
        return Ok(SqzDescriptor::default());
    }
    let major = u16::from_le_bytes(fixed_array::<2>(&header[8..10], "sqz header version")?);
    if major != VERSION_MAJOR {
        return Err(FormatError::Unsupported(format!(
            "unsupported sqz major version: {major}"
        )));
    }
    let uuid_hi = u64::from_le_bytes(fixed_array::<8>(&header[16..24], "sqz header uuid hi")?);
    let uuid_lo = u64::from_le_bytes(fixed_array::<8>(&header[24..32], "sqz header uuid lo")?);
    if uuid_hi != footer.uuid_hi || uuid_lo != footer.uuid_lo {
        return Err(FormatError::CorruptArchive(
            "sqz header/footer UUID mismatch".into(),
        ));
    }
    let descriptor_offset =
        u64::from_le_bytes(fixed_array::<8>(&header[32..40], "sqz descriptor offset")?);
    let descriptor_len =
        u64::from_le_bytes(fixed_array::<8>(&header[40..48], "sqz descriptor length")?);
    if descriptor_offset != HEADER_LEN as u64 {
        return Err(FormatError::CorruptArchive(
            "sqz descriptor offset is invalid".into(),
        ));
    }
    if descriptor_len == 0 {
        return Ok(SqzDescriptor::default());
    }
    let descriptor_end = descriptor_offset
        .checked_add(descriptor_len)
        .ok_or_else(|| FormatError::CorruptArchive("sqz descriptor overflows".into()))?;
    if descriptor_end > footer.index_offset {
        return Err(FormatError::CorruptArchive(
            "sqz descriptor points outside payload".into(),
        ));
    }
    let len: usize = descriptor_len
        .try_into()
        .map_err(|_| FormatError::Unsupported("sqz descriptor is too large".into()))?;
    let mut bytes = vec![0u8; len];
    src.seek(SeekFrom::Start(descriptor_offset))?;
    src.read_exact(&mut bytes)?;
    parse_descriptor(&bytes)
}

fn parse_descriptor(mut bytes: &[u8]) -> Result<SqzDescriptor, FormatError> {
    let mut descriptor = SqzDescriptor::default();
    while !bytes.is_empty() {
        let tag = read_u16(&mut bytes)?;
        let len = read_u32(&mut bytes)? as usize;
        let value = read_exact(&mut bytes, len)?;
        if tag == 0x0001 {
            let inner = std::str::from_utf8(value)
                .map_err(|_| FormatError::CorruptArchive("sqz inner format is not UTF-8".into()))?;
            descriptor.inner_format = inner.to_ascii_lowercase();
        }
    }
    Ok(descriptor)
}

fn parse_index(index: &[u8]) -> Result<Vec<SqzRecord>, FormatError> {
    let mut buf = index;
    if read_exact(&mut buf, 4)? != INDEX_MAGIC {
        return Err(FormatError::CorruptArchive(
            "missing sqz index magic".into(),
        ));
    }
    let version = read_u16(&mut buf)?;
    if version != 1 {
        return Err(FormatError::Unsupported(format!(
            "unsupported sqz index version: {version}"
        )));
    }
    let _flags = read_u16(&mut buf)?;
    let count = read_u64(&mut buf)?;
    let mut records = Vec::with_capacity(count.min(4096) as usize);
    for _ in 0..count {
        let kind = read_u8(&mut buf)?;
        let encrypted = read_u8(&mut buf)? != 0;
        let _reserved = read_u16(&mut buf)?;
        let data_offset = read_u64(&mut buf)?;
        let data_size = read_u64(&mut buf)?;
        let modified = match read_u64(&mut buf)? {
            u64::MAX => None,
            secs => Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        };
        let unix_mode = match read_u32(&mut buf)? {
            u32::MAX => None,
            mode => Some(mode),
        };
        let crc32c = read_u32(&mut buf)?;
        let hash = read_array::<32>(&mut buf)?;
        let raw_len = read_u32(&mut buf)? as usize;
        let display_len = read_u32(&mut buf)? as usize;
        let encoding_len = read_u16(&mut buf)? as usize;
        let link_len = read_u32(&mut buf)? as usize;
        let raw = read_exact(&mut buf, raw_len)?.to_vec();
        let display = String::from_utf8(read_exact(&mut buf, display_len)?.to_vec())
            .map_err(|_| FormatError::CorruptArchive("sqz display path is not UTF-8".into()))?;
        let encoding = String::from_utf8(read_exact(&mut buf, encoding_len)?.to_vec())
            .map_err(|_| FormatError::CorruptArchive("sqz encoding label is not UTF-8".into()))?;
        let link = read_exact(&mut buf, link_len)?.to_vec();
        let entry_type = match kind {
            KIND_FILE => EntryType::File,
            KIND_DIR => EntryType::Dir,
            KIND_SYMLINK => EntryType::Symlink { target: link },
            KIND_HARDLINK => EntryType::Hardlink { target: link },
            KIND_OTHER => EntryType::Other,
            other => {
                return Err(FormatError::CorruptArchive(format!(
                    "unknown sqz entry type: {other}"
                )))
            }
        };
        if !matches!(entry_type, EntryType::File) && hash != empty_hash() {
            return Err(FormatError::CorruptArchive(format!(
                "non-file sqz entry carries data hash: {display}"
            )));
        }
        records.push(SqzRecord {
            meta: EntryMeta {
                path: EntryPath::from_raw(raw, display, encoding_label(&encoding)),
                entry_type,
                size: data_size,
                compressed_size: Some(data_size),
                modified,
                unix_mode,
                crc32: None,
                encrypted,
            },
            data_offset,
            data_size,
            hash,
            crc32c,
        });
    }
    if !buf.is_empty() {
        return Err(FormatError::CorruptArchive(
            "trailing bytes in sqz footer index".into(),
        ));
    }
    Ok(records)
}
