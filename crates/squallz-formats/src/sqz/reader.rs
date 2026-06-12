use std::collections::{HashMap, HashSet};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use reed_solomon_erasure::galois_8::ReedSolomon;
use squallz_format_api::{
    ArchiveFormat, ArchiveReader, Compressor, ControlToken, EntryMeta, EntryPath, EntryType,
    FormatError, OpenOptions, ProgressSink, ReadSeek, RecoverySummary, StreamFactory, TestReport,
};

use crate::{sevenz::SevenZFormat, tar::TarFormat, zip::ZipFormat};

use super::{
    crc32c_bytes, empty_hash, encoding_label, fixed_array, read_array, read_exact, read_u16,
    read_u32, read_u64, read_u8, BLOCK_SIZE, FOOTER_LEN, HEADER_LEN, INDEX_MAGIC, KIND_DIR,
    KIND_FILE, KIND_HARDLINK, KIND_OTHER, KIND_SYMLINK, MAGIC, RECOVERY_ALGO_RS_GF8,
    RECOVERY_DATA_SHARDS, RECOVERY_MAGIC, RECOVERY_PARITY_SHARDS, RECOVERY_PROTECTION_MAGIC,
    RECOVERY_PROTECTION_TRAILER_LEN, RECOVERY_PROTECTION_VERSION, RECOVERY_VERSION, TAIL_MAGIC,
    VERSION_MAJOR,
};

const VERIFY_CHUNK: usize = 64 * 1024;
const FOOTER_RECOVERY_SCAN_WINDOW: u64 = 64 * 1024 * 1024;

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
    recovery: Option<RecoveryState>,
}

impl SqzArchiveReader {
    pub(super) fn open(
        mut src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Self, FormatError> {
        let len = src.seek(SeekFrom::End(0))?;
        if len < (HEADER_LEN + FOOTER_LEN) as u64 {
            return Err(FormatError::CorruptArchive("sqz file is too small".into()));
        }
        let (footer, recovered_recovery) = match read_footer(&mut *src, len) {
            Ok(footer) => (footer, None),
            Err(footer_err) => match recover_footer_from_recovery_scan(&mut *src, len) {
                Ok(Some(recovered)) => recovered,
                Ok(None) => return Err(footer_err),
                Err(_) => return Err(footer_err),
            },
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
            if matches!(record.meta.entry_type, EntryType::File) && data_end > footer.index_offset {
                return Err(FormatError::CorruptArchive(format!(
                    "sqz entry points outside payload: {}",
                    record.meta.path
                )));
            }
        }
        src.seek(SeekFrom::Start(0))?;
        let reader = EntrySetSqzReader {
            src,
            records,
            recovery,
        };
        reader.open_inner_profile(&descriptor.inner_format, opts)
    }
}

impl EntrySetSqzReader {
    fn open_inner_profile(
        mut self,
        inner_format: &str,
        opts: &OpenOptions,
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
                let outer_recovery = self.recovery.as_ref().map(RecoveryState::summary);
                if inner_format == "zstd" {
                    let inner =
                        TarFormat.open_stream(self.zstd_tar_stream_factory(record)?, opts)?;
                    return Ok(SqzArchiveReader::Inner {
                        reader: inner,
                        outer_recovery,
                    });
                }
                let inner_src: Box<dyn ReadSeek> = if self.record_has_repaired_block(record) {
                    Box::new(Cursor::new(self.read_record_bytes(record)?))
                } else {
                    Box::new(BoundedReadSeek::new(
                        self.src,
                        record.data_offset,
                        record.data_size,
                    ))
                };
                let inner = match inner_format {
                    "zip" => ZipFormat.open(inner_src, opts)?,
                    "tar" => TarFormat.open(inner_src, opts)?,
                    "7z" => SevenZFormat.open(inner_src, opts)?,
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

    fn zstd_tar_stream_factory(mut self, record: &SqzRecord) -> Result<StreamFactory, FormatError> {
        if self.record_has_repaired_block(record) {
            let payload: Arc<[u8]> = self.read_record_bytes(record)?.into();
            return Ok(Box::new(move || {
                let compressor = crate::stream::Zstd;
                compressor.decompress_reader(Box::new(Cursor::new(Arc::clone(&payload))))
            }));
        }

        let shared = Arc::new(Mutex::new(BoundedReadSeek::new(
            self.src,
            record.data_offset,
            record.data_size,
        )));
        Ok(Box::new(move || {
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
        }))
    }

    fn record_has_repaired_block(&self, record: &SqzRecord) -> bool {
        let Some((start, end)) = self.record_blocks(record) else {
            return false;
        };
        let Some(recovery) = &self.recovery else {
            return false;
        };
        (start..=end).any(|index| recovery.repaired_blocks.contains_key(&index))
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

    fn read_record_bytes(&mut self, record: &SqzRecord) -> Result<Vec<u8>, FormatError> {
        let mut out = Vec::with_capacity(record.data_size as usize);
        if self.recovery.is_none() {
            self.src.seek(SeekFrom::Start(record.data_offset))?;
            let mut limited = (&mut *self.src).take(record.data_size);
            limited.read_to_end(&mut out)?;
            if out.len() as u64 != record.data_size {
                return Err(FormatError::CorruptArchive("truncated file data".into()));
            }
            return Ok(out);
        }

        let mut pos = 0u64;
        while pos < record.data_size {
            let absolute = record
                .data_offset
                .checked_add(pos)
                .ok_or_else(|| FormatError::CorruptArchive("sqz entry offset overflows".into()))?;
            let Some(recovery) = self.recovery.as_ref() else {
                return Err(FormatError::CorruptArchive(
                    "sqz recovery state missing while reading recovered record".into(),
                ));
            };
            let relative = absolute
                .checked_sub(recovery.payload_start)
                .ok_or_else(|| {
                    FormatError::CorruptArchive("sqz entry starts before payload".into())
                })?;
            let block_index = relative / recovery.block_size as u64;
            if recovery.unrepaired_blocks.contains(&block_index) {
                return Err(FormatError::CorruptArchive(format!(
                    "sqz block {block_index} exceeds recovery capacity"
                )));
            }
            let block_offset = (relative % recovery.block_size as u64) as usize;
            let take = (recovery.block_size - block_offset).min((record.data_size - pos) as usize);
            let repaired_block = recovery.repaired_blocks.get(&block_index).cloned();
            if let Some(block) = repaired_block {
                let end = block_offset.checked_add(take).ok_or_else(|| {
                    FormatError::CorruptArchive("sqz repaired block offset overflows".into())
                })?;
                if end > block.len() {
                    return Err(FormatError::CorruptArchive(
                        "sqz repaired block is shorter than requested".into(),
                    ));
                }
                out.extend_from_slice(&block[block_offset..end]);
            } else {
                self.src.seek(SeekFrom::Start(absolute))?;
                let start_len = out.len();
                out.resize(start_len + take, 0);
                self.src.read_exact(&mut out[start_len..])?;
            }
            pos += take as u64;
        }
        Ok(out)
    }
}

impl ArchiveReader for EntrySetSqzReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.records.iter().map(|record| Ok(record.meta.clone())))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
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
        if self.record_has_repaired_block(&record) {
            let data = self.read_record_bytes(&record)?;
            return Ok(Box::new(Cursor::new(data)));
        }
        self.src.seek(SeekFrom::Start(record.data_offset))?;
        Ok(Box::new(EntryData {
            inner: &mut *self.src,
            remaining: record.data_size,
        }))
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut report = TestReport {
            recovery: self.recovery.as_ref().map(RecoveryState::summary),
            ..TestReport::default()
        };
        let total: u64 = self
            .records
            .iter()
            .filter(|r| matches!(r.meta.entry_type, EntryType::File))
            .map(|r| r.data_size)
            .sum();
        let mut done = 0u64;
        for record in self.records.clone() {
            ctl.checkpoint()?;
            report.entries_tested += 1;
            if !matches!(record.meta.entry_type, EntryType::File) {
                progress.on_progress(done, total, &record.meta.path);
                continue;
            }
            if self.record_has_unrepaired_block(&record) {
                report.problems.push(format!(
                    "{}: unrepaired SQZ recovery block damage",
                    record.meta.path
                ));
                progress.on_progress(done, total, &record.meta.path);
                continue;
            }
            match self.read_record_bytes(&record) {
                Ok(data) => {
                    for chunk in data.chunks(VERIFY_CHUNK) {
                        ctl.checkpoint()?;
                        done += chunk.len() as u64;
                        progress.on_progress(done, total, &record.meta.path);
                    }
                    let hash = *blake3::hash(&data).as_bytes();
                    let crc = crc32c::crc32c(&data);
                    if hash != record.hash || crc != record.crc32c {
                        report.problems.push(format!(
                            "{}: checksum mismatch (BLAKE3/CRC-32C)",
                            record.meta.path
                        ));
                    }
                }
                Err(e) => {
                    report.problems.push(format!("{}: {e}", record.meta.path));
                }
            }
        }
        let total = if total == 0 { done } else { total };
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(report)
    }
}

impl ArchiveReader for SqzArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.entries(),
            SqzArchiveReader::Inner { reader, .. } => reader.entries(),
        }
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.read_entry(path),
            SqzArchiveReader::Inner { reader, .. } => reader.read_entry(path),
        }
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        match self {
            SqzArchiveReader::EntrySet(reader) => reader.test(progress, ctl),
            SqzArchiveReader::Inner {
                reader,
                outer_recovery,
            } => {
                let mut report = reader.test(progress, ctl)?;
                if report.recovery.is_none() {
                    report.recovery = outer_recovery.clone();
                }
                Ok(report)
            }
        }
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
        let want = (remaining as usize).min(buf.len());
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
        if self.remaining == 0 {
            return Ok(0);
        }
        let want = (self.remaining as usize).min(buf.len());
        let n = self.inner.read(&mut buf[..want])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

struct RecoveryState {
    payload_start: u64,
    payload_length: u64,
    block_size: usize,
    data_shards: usize,
    parity_shards: usize,
    block_hashes: Vec<[u8; 32]>,
    parity: Vec<Vec<Vec<u8>>>,
    index_hash: [u8; 32],
    index_mirror: Vec<u8>,
    repaired_blocks: HashMap<u64, Vec<u8>>,
    unrepaired_blocks: HashSet<u64>,
}

impl RecoveryState {
    fn load(src: &mut dyn ReadSeek, footer: &Footer) -> Result<Option<Self>, FormatError> {
        if footer.recovery_length == 0 {
            return Ok(None);
        }
        let recovery_end = footer
            .recovery_offset
            .checked_add(footer.recovery_length)
            .ok_or_else(|| FormatError::CorruptArchive("sqz recovery section overflows".into()))?;
        if recovery_end > footer.index_offset {
            return Err(FormatError::CorruptArchive(
                "sqz recovery section overlaps footer index".into(),
            ));
        }
        let len: usize = footer
            .recovery_length
            .try_into()
            .map_err(|_| FormatError::Unsupported("sqz recovery section is too large".into()))?;
        let mut bytes = vec![0u8; len];
        src.seek(SeekFrom::Start(footer.recovery_offset))?;
        src.read_exact(&mut bytes)?;
        let mut state = Self::parse_protected_or_legacy(&bytes)?;
        state.repair(src)?;
        Ok(Some(state))
    }

    fn parse_protected_or_legacy(section: &[u8]) -> Result<Self, FormatError> {
        let trailer = match parse_recovery_protection_trailer(section) {
            Ok(Some(trailer)) => trailer,
            Ok(None) => return Self::parse(section),
            Err(trailer_err) => {
                if recovery_protection_trailer_crc_mismatch(section) {
                    return Self::parse_with_damaged_protection_trailer(section);
                }
                return Err(trailer_err);
            }
        };
        let primary_len: usize = trailer.primary_length.try_into().map_err(|_| {
            FormatError::Unsupported("sqz recovery primary section is too large".into())
        })?;
        let protection_len: usize = trailer
            .protection_length
            .try_into()
            .map_err(|_| FormatError::Unsupported("sqz recovery protection is too large".into()))?;
        let expected_len = primary_len
            .checked_add(protection_len)
            .and_then(|len| len.checked_add(RECOVERY_PROTECTION_TRAILER_LEN))
            .ok_or_else(|| {
                FormatError::CorruptArchive("sqz recovery protection length overflows".into())
            })?;
        if expected_len != section.len() {
            return Err(FormatError::CorruptArchive(
                "sqz recovery protection length mismatch".into(),
            ));
        }
        let primary = &section[..primary_len];
        if *blake3::hash(primary).as_bytes() == trailer.primary_hash {
            return Self::parse(primary);
        }
        let protection = &section[primary_len..primary_len + protection_len];
        let repaired = repair_protected_recovery(primary, protection, &trailer)?;
        if *blake3::hash(&repaired).as_bytes() != trailer.primary_hash {
            return Err(FormatError::CorruptArchive(
                "sqz recovery section protection could not restore primary hash".into(),
            ));
        }
        Self::parse(&repaired)
    }

    fn parse(section: &[u8]) -> Result<Self, FormatError> {
        let (state, consumed) = Self::parse_prefix(section)?;
        if consumed != section.len() {
            return Err(FormatError::CorruptArchive(
                "trailing bytes in sqz recovery section".into(),
            ));
        }
        Ok(state)
    }

    fn parse_prefix(section: &[u8]) -> Result<(Self, usize), FormatError> {
        let original_len = section.len();
        let mut buf = section;
        if read_exact(&mut buf, 4)? != RECOVERY_MAGIC {
            return Err(FormatError::CorruptArchive(
                "missing sqz recovery magic".into(),
            ));
        }
        let version = read_u16(&mut buf)?;
        if version != RECOVERY_VERSION {
            return Err(FormatError::Unsupported(format!(
                "unsupported sqz recovery version: {version}"
            )));
        }
        let algo = read_u16(&mut buf)?;
        if algo != RECOVERY_ALGO_RS_GF8 {
            return Err(FormatError::Unsupported(format!(
                "unsupported sqz recovery algorithm: {algo}"
            )));
        }
        let block_size = read_u32(&mut buf)? as usize;
        let data_shards = read_u16(&mut buf)? as usize;
        let parity_shards = read_u16(&mut buf)? as usize;
        let _reserved = read_u32(&mut buf)?;
        let payload_start = read_u64(&mut buf)?;
        let payload_length = read_u64(&mut buf)?;
        let block_count = read_u64(&mut buf)?;
        let index_length = read_u64(&mut buf)?;
        let index_hash = read_array::<32>(&mut buf)?;
        if block_size == 0 || block_size > 16 * 1024 * 1024 {
            return Err(FormatError::CorruptArchive(
                "invalid sqz recovery block size".into(),
            ));
        }
        if data_shards == 0 || parity_shards == 0 || data_shards + parity_shards > 255 {
            return Err(FormatError::CorruptArchive(
                "invalid sqz recovery shard counts".into(),
            ));
        }
        let expected_blocks = if payload_length == 0 {
            0
        } else {
            payload_length.div_ceil(block_size as u64)
        };
        if block_count != expected_blocks {
            return Err(FormatError::CorruptArchive(
                "sqz recovery block count does not match payload length".into(),
            ));
        }
        let block_count_usize: usize = block_count.try_into().map_err(|_| {
            FormatError::Unsupported("sqz recovery block count is too large".into())
        })?;
        let mut block_hashes = Vec::with_capacity(block_count_usize);
        for _ in 0..block_count_usize {
            block_hashes.push(read_array::<32>(&mut buf)?);
        }
        let group_count = block_count_usize.div_ceil(data_shards);
        let mut parity = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            let mut group = Vec::with_capacity(parity_shards);
            for _ in 0..parity_shards {
                group.push(read_exact(&mut buf, block_size)?.to_vec());
            }
            parity.push(group);
        }
        let index_len: usize = index_length.try_into().map_err(|_| {
            FormatError::Unsupported("sqz recovery index mirror is too large".into())
        })?;
        let index_mirror = read_exact(&mut buf, index_len)?.to_vec();
        if *blake3::hash(&index_mirror).as_bytes() != index_hash {
            return Err(FormatError::CorruptArchive(
                "sqz recovery index mirror hash mismatch".into(),
            ));
        }
        let consumed = original_len - buf.len();
        Ok((
            Self {
                payload_start,
                payload_length,
                block_size,
                data_shards,
                parity_shards,
                block_hashes,
                parity,
                index_hash,
                index_mirror,
                repaired_blocks: HashMap::new(),
                unrepaired_blocks: HashSet::new(),
            },
            consumed,
        ))
    }

    fn parse_with_damaged_protection_trailer(section: &[u8]) -> Result<Self, FormatError> {
        let trailer_start = section.len() - RECOVERY_PROTECTION_TRAILER_LEN;
        let (state, primary_len) =
            Self::parse_prefix(section).map_err(|_| damaged_recovery_trailer_fallback_error())?;
        if primary_len > trailer_start {
            return Err(damaged_recovery_trailer_fallback_error());
        }
        let protection_len = trailer_start - primary_len;
        let primary = &section[..primary_len];
        let protection = &section[primary_len..trailer_start];
        if protection_len != recovery_protection_payload_len(primary_len)?
            || !recovery_protection_payload_matches_primary(primary, protection)?
        {
            return Err(damaged_recovery_trailer_fallback_error());
        }
        Ok(state)
    }
}

fn damaged_recovery_trailer_fallback_error() -> FormatError {
    FormatError::CorruptArchive(
        "sqz recovery protection trailer is damaged and primary fallback failed".into(),
    )
}

fn recovery_protection_payload_len(primary_len: usize) -> Result<usize, FormatError> {
    let block_size = BLOCK_SIZE as usize;
    let block_count = primary_len.div_ceil(block_size);
    let hash_len = block_count.checked_mul(32).ok_or_else(|| {
        FormatError::CorruptArchive("sqz recovery protection hash length overflows".into())
    })?;
    let group_count = block_count.div_ceil(RECOVERY_DATA_SHARDS as usize);
    let parity_len = group_count
        .checked_mul(RECOVERY_PARITY_SHARDS as usize)
        .and_then(|len| len.checked_mul(block_size))
        .ok_or_else(|| {
            FormatError::CorruptArchive("sqz recovery protection parity length overflows".into())
        })?;
    hash_len.checked_add(parity_len).ok_or_else(|| {
        FormatError::CorruptArchive("sqz recovery protection length overflows".into())
    })
}

fn recovery_protection_payload_matches_primary(
    primary: &[u8],
    protection: &[u8],
) -> Result<bool, FormatError> {
    if primary.is_empty() {
        return Ok(protection.is_empty());
    }
    let block_size = BLOCK_SIZE as usize;
    let data_shards = RECOVERY_DATA_SHARDS as usize;
    let parity_shards = RECOVERY_PARITY_SHARDS as usize;
    let block_count = primary.len().div_ceil(block_size);
    let expected_len = recovery_protection_payload_len(primary.len())?;
    if protection.len() != expected_len {
        return Ok(false);
    }

    let mut expected = Vec::with_capacity(expected_len);
    let codec = ReedSolomon::new(data_shards, parity_shards)
        .map_err(|e| FormatError::Other(format!("sqz recovery protection init failed: {e}")))?;
    let mut parity_groups = Vec::with_capacity(block_count.div_ceil(data_shards));

    for group_start in (0..block_count).step_by(data_shards) {
        let data_count = (block_count - group_start).min(data_shards);
        let mut shards = Vec::with_capacity(data_shards + parity_shards);
        for local in 0..data_count {
            let block_index = group_start + local;
            let start = block_index * block_size;
            let end = primary.len().min(start + block_size);
            expected.extend_from_slice(blake3::hash(&primary[start..end]).as_bytes());
            let mut shard = primary[start..end].to_vec();
            shard.resize(block_size, 0);
            shards.push(shard);
        }
        shards.extend((data_count..data_shards).map(|_| vec![0u8; block_size]));
        shards.extend((0..parity_shards).map(|_| vec![0u8; block_size]));
        codec.encode(&mut shards).map_err(|e| {
            FormatError::Other(format!("sqz recovery protection encode failed: {e}"))
        })?;
        parity_groups.push(shards.split_off(data_shards));
    }

    for group in parity_groups {
        for shard in group {
            expected.extend_from_slice(&shard);
        }
    }
    Ok(expected == protection)
}

fn recovery_protection_trailer_crc_mismatch(section: &[u8]) -> bool {
    if section.len() < RECOVERY_PROTECTION_TRAILER_LEN {
        return false;
    }
    let trailer_start = section.len() - RECOVERY_PROTECTION_TRAILER_LEN;
    let trailer = &section[trailer_start..];
    if &trailer[..4] != RECOVERY_PROTECTION_MAGIC {
        return false;
    }
    let Ok(expected_bytes) =
        fixed_array::<4>(&trailer[76..80], "sqz recovery protection trailer crc")
    else {
        return false;
    };
    let expected = u32::from_le_bytes(expected_bytes);
    let actual = crc32c_bytes(&trailer[..76]);
    expected != actual
}

impl RecoveryState {
    fn index_bytes(&self, primary: &[u8]) -> Result<Vec<u8>, FormatError> {
        if *blake3::hash(primary).as_bytes() == self.index_hash {
            return Ok(primary.to_vec());
        }
        if self.index_mirror.is_empty() {
            return Err(FormatError::CorruptArchive(
                "sqz footer index hash mismatch and no recovery mirror is available".into(),
            ));
        }
        Ok(self.index_mirror.clone())
    }

    fn repair(&mut self, src: &mut dyn ReadSeek) -> Result<(), FormatError> {
        for group_index in 0..self.parity.len() {
            self.repair_group(src, group_index)?;
        }
        Ok(())
    }

    fn repair_group(
        &mut self,
        src: &mut dyn ReadSeek,
        group_index: usize,
    ) -> Result<(), FormatError> {
        let start_block = group_index * self.data_shards;
        let remaining = self.block_hashes.len().saturating_sub(start_block);
        let data_count = remaining.min(self.data_shards);
        if data_count == 0 {
            return Ok(());
        }

        let mut shards: Vec<Option<Vec<u8>>> = Vec::with_capacity(data_count + self.parity_shards);
        let mut missing_data = Vec::new();
        for local in 0..data_count {
            let block_index = start_block + local;
            let block = self.read_payload_block(src, block_index as u64)?;
            let actual_len = self.block_actual_len(block_index as u64);
            if *blake3::hash(&block[..actual_len]).as_bytes() == self.block_hashes[block_index] {
                shards.push(Some(block));
            } else {
                missing_data.push(block_index as u64);
                shards.push(None);
            }
        }

        if missing_data.is_empty() {
            return Ok(());
        }
        for parity in &self.parity[group_index] {
            shards.push(Some(parity.clone()));
        }

        let codec = ReedSolomon::new(data_count, self.parity_shards)
            .map_err(|e| FormatError::Other(format!("sqz recovery decoder init failed: {e}")))?;
        if codec.reconstruct_data(&mut shards).is_err() {
            self.unrepaired_blocks.extend(missing_data);
            return Ok(());
        }

        for block_index in missing_data {
            let local = (block_index as usize) - start_block;
            let Some(block) = shards[local].take() else {
                self.unrepaired_blocks.insert(block_index);
                continue;
            };
            let actual_len = self.block_actual_len(block_index);
            if *blake3::hash(&block[..actual_len]).as_bytes()
                == self.block_hashes[block_index as usize]
            {
                self.repaired_blocks
                    .insert(block_index, block[..actual_len].to_vec());
            } else {
                self.unrepaired_blocks.insert(block_index);
            }
        }
        Ok(())
    }

    fn read_payload_block(
        &self,
        src: &mut dyn ReadSeek,
        block_index: u64,
    ) -> Result<Vec<u8>, FormatError> {
        let actual_len = self.block_actual_len(block_index);
        let offset = self
            .payload_start
            .checked_add(block_index * self.block_size as u64)
            .ok_or_else(|| FormatError::CorruptArchive("sqz block offset overflows".into()))?;
        let mut block = vec![0u8; self.block_size];
        src.seek(SeekFrom::Start(offset))?;
        if let Err(_e) = src.read_exact(&mut block[..actual_len]) {
            return Ok(block);
        }
        Ok(block)
    }

    fn block_actual_len(&self, block_index: u64) -> usize {
        let consumed = block_index * self.block_size as u64;
        let remaining = self.payload_length.saturating_sub(consumed);
        remaining.min(self.block_size as u64) as usize
    }

    fn summary(&self) -> RecoverySummary {
        let repaired_blocks = self.repaired_blocks.len() as u64;
        let unrepaired_blocks = self.unrepaired_blocks.len() as u64;
        let group_count = self.parity.len() as u64;
        RecoverySummary {
            scheme: "sqz-embedded-rs-gf8".to_string(),
            block_size: self.block_size as u64,
            total_blocks: self.block_hashes.len() as u64,
            data_shards: self.data_shards as u64,
            parity_shards: self.parity_shards as u64,
            recovery_blocks_available: group_count.saturating_mul(self.parity_shards as u64),
            damaged_blocks: repaired_blocks + unrepaired_blocks,
            repaired_blocks,
            unrepaired_blocks,
            repair_possible: unrepaired_blocks == 0,
        }
    }
}

struct RecoveryProtectionTrailer {
    block_size: usize,
    data_shards: usize,
    parity_shards: usize,
    primary_length: u64,
    block_count: u64,
    protection_length: u64,
    primary_hash: [u8; 32],
}

fn parse_recovery_protection_trailer(
    section: &[u8],
) -> Result<Option<RecoveryProtectionTrailer>, FormatError> {
    if section.len() < RECOVERY_PROTECTION_TRAILER_LEN {
        return Ok(None);
    }
    let trailer_start = section.len() - RECOVERY_PROTECTION_TRAILER_LEN;
    let trailer = &section[trailer_start..];
    if &trailer[..4] != RECOVERY_PROTECTION_MAGIC {
        return Ok(None);
    }
    let expected = u32::from_le_bytes(fixed_array::<4>(
        &trailer[76..80],
        "sqz recovery protection trailer crc",
    )?);
    let actual = crc32c_bytes(&trailer[..76]);
    if expected != actual {
        return Err(FormatError::CorruptArchive(
            "sqz recovery protection trailer CRC-32C mismatch".into(),
        ));
    }
    let mut buf = trailer;
    let _magic = read_exact(&mut buf, 4)?;
    let version = read_u16(&mut buf)?;
    if version != RECOVERY_PROTECTION_VERSION {
        return Err(FormatError::Unsupported(format!(
            "unsupported sqz recovery protection version: {version}"
        )));
    }
    let algo = read_u16(&mut buf)?;
    if algo != RECOVERY_ALGO_RS_GF8 {
        return Err(FormatError::Unsupported(format!(
            "unsupported sqz recovery protection algorithm: {algo}"
        )));
    }
    let block_size = read_u32(&mut buf)? as usize;
    let data_shards = read_u16(&mut buf)? as usize;
    let parity_shards = read_u16(&mut buf)? as usize;
    let _reserved = read_u32(&mut buf)?;
    let primary_length = read_u64(&mut buf)?;
    let block_count = read_u64(&mut buf)?;
    let protection_length = read_u64(&mut buf)?;
    let primary_hash = read_array::<32>(&mut buf)?;
    let _crc = read_u32(&mut buf)?;
    if block_size == 0 || block_size > 16 * 1024 * 1024 {
        return Err(FormatError::CorruptArchive(
            "invalid sqz recovery protection block size".into(),
        ));
    }
    if data_shards == 0 || parity_shards == 0 || data_shards + parity_shards > 255 {
        return Err(FormatError::CorruptArchive(
            "invalid sqz recovery protection shard counts".into(),
        ));
    }
    let expected_blocks = if primary_length == 0 {
        0
    } else {
        primary_length.div_ceil(block_size as u64)
    };
    if block_count != expected_blocks {
        return Err(FormatError::CorruptArchive(
            "sqz recovery protection block count does not match primary length".into(),
        ));
    }
    Ok(Some(RecoveryProtectionTrailer {
        block_size,
        data_shards,
        parity_shards,
        primary_length,
        block_count,
        protection_length,
        primary_hash,
    }))
}

fn repair_protected_recovery(
    primary: &[u8],
    protection: &[u8],
    trailer: &RecoveryProtectionTrailer,
) -> Result<Vec<u8>, FormatError> {
    let block_count: usize = trailer.block_count.try_into().map_err(|_| {
        FormatError::Unsupported("sqz recovery protection block count is too large".into())
    })?;
    let hash_len = block_count.checked_mul(32).ok_or_else(|| {
        FormatError::CorruptArchive("sqz recovery protection hash length overflows".into())
    })?;
    let group_count = block_count.div_ceil(trailer.data_shards);
    let parity_len = group_count
        .checked_mul(trailer.parity_shards)
        .and_then(|len| len.checked_mul(trailer.block_size))
        .ok_or_else(|| {
            FormatError::CorruptArchive("sqz recovery protection parity length overflows".into())
        })?;
    if protection.len() != hash_len + parity_len {
        return Err(FormatError::CorruptArchive(
            "sqz recovery protection payload length mismatch".into(),
        ));
    }

    let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(block_count);
    for chunk in protection[..hash_len].chunks_exact(32) {
        hashes.push(fixed_array::<32>(
            chunk,
            "sqz recovery protection block hash",
        )?);
    }
    let parity = &protection[hash_len..];
    let mut repaired = primary.to_vec();

    for group_index in 0..group_count {
        let start_block = group_index * trailer.data_shards;
        let remaining = block_count.saturating_sub(start_block);
        let data_count = remaining.min(trailer.data_shards);
        let mut shards: Vec<Option<Vec<u8>>> =
            Vec::with_capacity(trailer.data_shards + trailer.parity_shards);
        let mut missing = Vec::new();

        for local in 0..trailer.data_shards {
            if local >= data_count {
                shards.push(Some(vec![0u8; trailer.block_size]));
                continue;
            }
            let block_index = start_block + local;
            let start = block_index * trailer.block_size;
            let end = primary.len().min(start + trailer.block_size);
            let block = &primary[start..end];
            if *blake3::hash(block).as_bytes() == hashes[block_index] {
                let mut shard = block.to_vec();
                shard.resize(trailer.block_size, 0);
                shards.push(Some(shard));
            } else {
                missing.push(block_index);
                shards.push(None);
            }
        }

        let parity_start = group_index * trailer.parity_shards * trailer.block_size;
        for parity_index in 0..trailer.parity_shards {
            let start = parity_start + parity_index * trailer.block_size;
            let end = start + trailer.block_size;
            shards.push(Some(parity[start..end].to_vec()));
        }

        if missing.is_empty() {
            continue;
        }
        let codec = ReedSolomon::new(trailer.data_shards, trailer.parity_shards).map_err(|e| {
            FormatError::Other(format!("sqz recovery protection decoder init failed: {e}"))
        })?;
        codec.reconstruct_data(&mut shards).map_err(|_| {
            FormatError::CorruptArchive(
                "sqz recovery section damage exceeds protection capacity".into(),
            )
        })?;
        for block_index in missing {
            let local = block_index - start_block;
            let Some(block) = shards[local].take() else {
                return Err(FormatError::CorruptArchive(
                    "sqz recovery protection did not reconstruct a missing block".into(),
                ));
            };
            let start = block_index * trailer.block_size;
            let end = primary.len().min(start + trailer.block_size);
            let actual_len = end - start;
            if *blake3::hash(&block[..actual_len]).as_bytes() != hashes[block_index] {
                return Err(FormatError::CorruptArchive(
                    "sqz recovery protection reconstructed block hash mismatch".into(),
                ));
            }
            repaired[start..end].copy_from_slice(&block[..actual_len]);
        }
    }

    Ok(repaired)
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

fn recover_footer_from_recovery_scan(
    src: &mut dyn ReadSeek,
    len: u64,
) -> Result<Option<(Footer, Option<RecoveryState>)>, FormatError> {
    let Some((uuid_hi, uuid_lo)) = valid_header_uuid(src)? else {
        return Ok(None);
    };
    let window_len = len.min(FOOTER_RECOVERY_SCAN_WINDOW);
    let window_start = len - window_len;
    let mut window = vec![0u8; window_len as usize];
    src.seek(SeekFrom::Start(window_start))?;
    src.read_exact(&mut window)?;
    let mut positions = byte_pattern_positions(&window, RECOVERY_PROTECTION_MAGIC);
    positions.reverse();
    for trailer_pos in positions {
        let trailer_end = trailer_pos.saturating_add(RECOVERY_PROTECTION_TRAILER_LEN);
        if trailer_end > window.len() {
            continue;
        }
        let trailer_section = &window[..trailer_end];
        let Some(trailer) = parse_recovery_protection_trailer(trailer_section)? else {
            continue;
        };
        let section_len = trailer
            .primary_length
            .checked_add(trailer.protection_length)
            .and_then(|len| len.checked_add(RECOVERY_PROTECTION_TRAILER_LEN as u64))
            .ok_or_else(|| {
                FormatError::CorruptArchive("sqz recovery scan length overflows".into())
            })?;
        let trailer_abs = window_start + trailer_pos as u64;
        let Some(recovery_offset) = trailer_abs.checked_sub(
            trailer
                .primary_length
                .checked_add(trailer.protection_length)
                .ok_or_else(|| {
                    FormatError::CorruptArchive("sqz recovery scan length overflows".into())
                })?,
        ) else {
            continue;
        };
        if recovery_offset < HEADER_LEN as u64 {
            continue;
        }
        let recovery_end = recovery_offset + section_len;
        if recovery_end > len.saturating_sub(FOOTER_LEN as u64) {
            continue;
        }
        let section_len_usize: usize = match section_len.try_into() {
            Ok(len) => len,
            Err(_) => continue,
        };
        let mut section = vec![0u8; section_len_usize];
        src.seek(SeekFrom::Start(recovery_offset))?;
        src.read_exact(&mut section)?;
        let recovery = match RecoveryState::parse_protected_or_legacy(&section) {
            Ok(recovery) => recovery,
            Err(_) => continue,
        };
        let index_length = recovery.index_mirror.len() as u64;
        if index_length == 0 {
            continue;
        }
        let index_end = recovery_end.checked_add(index_length).ok_or_else(|| {
            FormatError::CorruptArchive("sqz recovered index range overflows".into())
        })?;
        if index_end > len.saturating_sub(FOOTER_LEN as u64) {
            continue;
        }
        let footer = Footer {
            index_offset: recovery_end,
            index_length,
            recovery_offset,
            recovery_length: section_len,
            uuid_hi,
            uuid_lo,
        };
        return Ok(Some((footer, Some(recovery))));
    }
    Ok(None)
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

fn byte_pattern_positions(bytes: &[u8], pattern: &[u8]) -> Vec<usize> {
    bytes
        .windows(pattern.len())
        .enumerate()
        .filter_map(|(index, window)| (window == pattern).then_some(index))
        .collect()
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
