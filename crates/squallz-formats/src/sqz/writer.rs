use std::fs::{self, File, OpenOptions as FsOpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reed_solomon_erasure::galois_8::ReedSolomon;
use squallz_format_api::{
    ArchiveFormat, ArchiveWriter, CompressionLevel, Compressor, CreateOptions, EntryMeta,
    EntryPath, EntryType, FormatError, ResourceOptions, SqzCreateOptions, WriteSeek,
};

use crate::{sevenz::SevenZFormat, tar::TarFormat, zip::ZipFormat};

use super::{
    crc32c_bytes, empty_hash, put_u16, put_u32, put_u64, BLOCK_SIZE, FOOTER_LEN,
    HEADER_FLAG_RECOVERY, HEADER_LEN, INDEX_MAGIC, KIND_DIR, KIND_FILE, KIND_HARDLINK, KIND_OTHER,
    KIND_SYMLINK, MAGIC, RECOVERY_ALGO_RS_GF8, RECOVERY_DATA_SHARDS, RECOVERY_MAGIC,
    RECOVERY_PARITY_SHARDS, RECOVERY_PROTECTION_MAGIC, RECOVERY_PROTECTION_TRAILER_LEN,
    RECOVERY_PROTECTION_VERSION, RECOVERY_VERSION, TAIL_MAGIC, VERSION_MAJOR, VERSION_MINOR,
};

const COPY_CHUNK: usize = 64 * 1024;
const ABSENT_TIMESTAMP_SECS: u64 = u64::MAX;
const ABSENT_UNIX_MODE: u32 = u32::MAX;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct PendingRecord {
    meta: EntryMeta,
    data_offset: u64,
    data_size: u64,
    hash: [u8; 32],
    crc32c: u32,
}

pub(super) struct SqzArchiveWriter {
    dst: Box<dyn WriteSeek>,
    records: Vec<PendingRecord>,
    uuid_hi: u64,
    uuid_lo: u64,
    recovery: RecoveryEncoder,
}

pub(super) fn create(
    dst: Box<dyn WriteSeek>,
    opts: &CreateOptions,
) -> Result<Box<dyn ArchiveWriter>, FormatError> {
    match opts.sqz.inner_format.as_str() {
        "sqz" => Ok(Box::new(SqzArchiveWriter::new(dst, &opts.sqz)?)),
        "zip" => Ok(Box::new(InnerArchiveSqzWriter::new(
            dst,
            opts,
            InnerProfile::Zip,
        )?)),
        "tar" => Ok(Box::new(InnerArchiveSqzWriter::new(
            dst,
            opts,
            InnerProfile::Tar,
        )?)),
        "7z" => Ok(Box::new(InnerArchiveSqzWriter::new(
            dst,
            opts,
            InnerProfile::SevenZ,
        )?)),
        "zstd" => Ok(Box::new(InnerArchiveSqzWriter::new(
            dst,
            opts,
            InnerProfile::TarZstd,
        )?)),
        other => Err(FormatError::Unsupported(format!(
            "unsupported sqz inner format: {other}"
        ))),
    }
}

impl SqzArchiveWriter {
    pub(super) fn new(
        mut dst: Box<dyn WriteSeek>,
        opts: &SqzCreateOptions,
    ) -> Result<Self, FormatError> {
        let (uuid_hi, uuid_lo) = new_container_uuid();
        let descriptor = descriptor_tlv(opts)?;
        let header = build_header(uuid_hi, uuid_lo, descriptor.len() as u64);
        dst.write_all(&header)?;
        dst.write_all(&descriptor)?;
        let payload_start = dst.stream_position()?;
        Ok(Self {
            dst,
            records: Vec::new(),
            uuid_hi,
            uuid_lo,
            recovery: RecoveryEncoder::new(payload_start, opts),
        })
    }
}

impl ArchiveWriter for SqzArchiveWriter {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        match &meta.entry_type {
            EntryType::File => {
                let data = data.ok_or_else(|| {
                    FormatError::Other(format!("file entry without data: {}", meta.path))
                })?;
                let data_offset = self.dst.stream_position()?;
                let mut remaining = meta.size;
                let mut written = 0u64;
                let mut crc = 0u32;
                let mut hasher = blake3::Hasher::new();
                let mut buf = vec![0u8; COPY_CHUNK];
                loop {
                    let n = data.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    self.dst.write_all(&buf[..n])?;
                    self.recovery.push_bytes(&buf[..n])?;
                    hasher.update(&buf[..n]);
                    crc = crc32c::crc32c_append(crc, &buf[..n]);
                    written += n as u64;
                    remaining = remaining.saturating_sub(n as u64);
                }
                if written != meta.size || remaining != 0 {
                    return Err(FormatError::CorruptArchive(format!(
                        "input size changed while writing {}: expected {}, wrote {}",
                        meta.path, meta.size, written
                    )));
                }
                self.records.push(PendingRecord {
                    meta: meta.clone(),
                    data_offset,
                    data_size: written,
                    hash: *hasher.finalize().as_bytes(),
                    crc32c: crc,
                });
            }
            EntryType::Dir
            | EntryType::Symlink { .. }
            | EntryType::Hardlink { .. }
            | EntryType::Other => {
                self.records.push(PendingRecord {
                    meta: meta.clone(),
                    data_offset: 0,
                    data_size: 0,
                    hash: empty_hash(),
                    crc32c: crc32c_bytes(&[]),
                });
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
        let index = build_index(&self.records)?;
        let recovery_section =
            std::mem::replace(&mut self.recovery, RecoveryEncoder::empty()).finish(&index)?;
        let recovery_offset = self.dst.stream_position()?;
        let recovery_length = recovery_section.len() as u64;
        if !recovery_section.is_empty() {
            self.dst.write_all(&recovery_section)?;
        }
        let footer_index_offset = self.dst.stream_position()?;
        self.dst.write_all(&index)?;
        let footer = build_footer(
            footer_index_offset,
            index.len() as u64,
            recovery_offset,
            recovery_length,
            self.uuid_hi,
            self.uuid_lo,
        );
        debug_assert_eq!(footer.len(), FOOTER_LEN);
        self.dst.write_all(&footer)?;
        self.dst.flush()?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum InnerProfile {
    Zip,
    Tar,
    SevenZ,
    TarZstd,
}

impl InnerProfile {
    fn temp_extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::SevenZ => "7z",
            Self::TarZstd => "tar",
        }
    }

    fn payload_name(self) -> &'static str {
        match self {
            Self::Zip => "__sqz_inner.zip",
            Self::Tar => "__sqz_inner.tar",
            Self::SevenZ => "__sqz_inner.7z",
            Self::TarZstd => "__sqz_inner.tar.zst",
        }
    }

    fn create_inner_writer(
        self,
        file: File,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        match self {
            Self::Zip => ZipFormat.create(Box::new(file), opts),
            Self::Tar => TarFormat.create(Box::new(file), opts),
            Self::SevenZ => SevenZFormat.create(Box::new(file), opts),
            Self::TarZstd => TarFormat.create(Box::new(file), opts),
        }
    }

    fn needs_zstd_compression(self) -> bool {
        matches!(self, Self::TarZstd)
    }
}

struct InnerArchiveSqzWriter {
    outer: SqzArchiveWriter,
    inner: Box<dyn ArchiveWriter>,
    temp: TempPathGuard,
    profile: InnerProfile,
    level: CompressionLevel,
    resources: ResourceOptions,
}

impl InnerArchiveSqzWriter {
    fn new(
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
        profile: InnerProfile,
    ) -> Result<Self, FormatError> {
        let outer = SqzArchiveWriter::new(dst, &opts.sqz)?;
        let (temp, file) = create_temp_inner_file(profile.temp_extension())?;
        let mut inner_opts = opts.clone();
        inner_opts.sqz = SqzCreateOptions::default();
        inner_opts.split_size = None;
        let inner = profile.create_inner_writer(file, &inner_opts)?;
        Ok(Self {
            outer,
            inner,
            temp,
            profile,
            level: opts.level,
            resources: opts.resources,
        })
    }
}

impl ArchiveWriter for InnerArchiveSqzWriter {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        self.inner.add_entry(meta, data)
    }

    fn finish(self: Box<Self>) -> Result<(), FormatError> {
        let InnerArchiveSqzWriter {
            mut outer,
            inner,
            temp,
            profile,
            level,
            resources,
        } = *self;
        inner.finish()?;
        let compressed_payload = if profile.needs_zstd_compression() {
            let (compressed_temp, compressed_file) = create_temp_inner_file("tar.zst")?;
            zstd_compress_file(temp.path(), compressed_file, level, resources)?;
            Some(compressed_temp)
        } else {
            None
        };
        let payload_path = selected_payload_path(compressed_payload.as_ref(), &temp);
        let size = fs::metadata(payload_path)?.len();
        let modified = fs::metadata(payload_path)
            .and_then(|meta| meta.modified())
            .ok();
        let mut file = File::open(payload_path)?;
        let meta = EntryMeta {
            path: EntryPath::from_utf8(profile.payload_name()),
            entry_type: EntryType::File,
            size,
            compressed_size: Some(size),
            modified,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        outer.add_entry(&meta, Some(&mut file))?;
        Box::new(outer).finish()
    }
}

fn zstd_compress_file(
    src: &Path,
    dst: File,
    level: CompressionLevel,
    resources: ResourceOptions,
) -> Result<(), FormatError> {
    let mut src = File::open(src)?;
    let compressor = crate::stream::Zstd;
    let mut sink = compressor.compress_writer(Box::new(dst), level, &resources)?;
    std::io::copy(&mut src, &mut sink)?;
    sink.finish()
}

struct TempPathGuard {
    path: PathBuf,
}

impl TempPathGuard {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create_temp_inner_file(ext: &str) -> Result<(TempPathGuard, File), FormatError> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let seed = now_since_epoch_or_zero().as_nanos();
    for _ in 0..100 {
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("squallz-inner-{pid}-{seed}-{count}.{ext}"));
        match FsOpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((TempPathGuard { path }, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(FormatError::Io(err)),
        }
    }
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate temporary SQZ inner archive path",
    )))
}

fn new_container_uuid() -> (u64, u64) {
    let now = now_since_epoch_or_zero().as_nanos();
    let hi = (now >> 64) as u64;
    let lo = (now as u64) ^ ((std::process::id() as u64) << 32);
    (hi, lo)
}

fn descriptor_tlv(opts: &SqzCreateOptions) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    put_tlv(&mut out, 0x0001, opts.inner_format.as_bytes())?;
    let recovery_hint = format!(
        "transparent;recovery=rs-gf8-{}+{};requested-recovery={}%",
        SqzCreateOptions::DATA_SHARDS,
        opts.parity_shards(),
        opts.recovery_percent
    );
    put_tlv(&mut out, 0x0006, recovery_hint.as_bytes())?;
    let created = now_since_epoch_or_zero().as_secs().to_le_bytes();
    put_tlv(&mut out, 0x0005, &created)?;
    Ok(out)
}

fn now_since_epoch_or_zero() -> Duration {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration,
        Err(_) => Duration::ZERO,
    }
}

fn selected_payload_path<'a>(
    compressed_payload: Option<&'a TempPathGuard>,
    uncompressed_payload: &'a TempPathGuard,
) -> &'a Path {
    match compressed_payload {
        Some(guard) => guard.path(),
        None => uncompressed_payload.path(),
    }
}

fn put_tlv(out: &mut Vec<u8>, tag: u16, value: &[u8]) -> Result<(), FormatError> {
    let len: u32 = value
        .len()
        .try_into()
        .map_err(|_| FormatError::Unsupported("sqz descriptor value exceeds 4 GiB".into()))?;
    put_u16(out, tag);
    put_u32(out, len);
    out.extend_from_slice(value);
    Ok(())
}

fn build_header(uuid_hi: u64, uuid_lo: u64, descriptor_len: u64) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[0..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
    header[10..12].copy_from_slice(&VERSION_MINOR.to_le_bytes());
    header[12..16].copy_from_slice(&HEADER_FLAG_RECOVERY.to_le_bytes());
    header[16..24].copy_from_slice(&uuid_hi.to_le_bytes());
    header[24..32].copy_from_slice(&uuid_lo.to_le_bytes());
    header[32..40].copy_from_slice(&(HEADER_LEN as u64).to_le_bytes());
    header[40..48].copy_from_slice(&descriptor_len.to_le_bytes());
    header[48..52].copy_from_slice(&BLOCK_SIZE.to_le_bytes());
    let crc = crc32c_bytes(&header[..52]);
    header[52..56].copy_from_slice(&crc.to_le_bytes());
    header
}

fn build_index(records: &[PendingRecord]) -> Result<Vec<u8>, FormatError> {
    let mut out = Vec::new();
    out.extend_from_slice(INDEX_MAGIC);
    put_u16(&mut out, 1);
    put_u16(&mut out, 0);
    put_u64(&mut out, records.len() as u64);
    for record in records {
        out.push(kind_of(&record.meta.entry_type));
        out.push(u8::from(record.meta.encrypted));
        put_u16(&mut out, 0);
        put_u64(&mut out, record.data_offset);
        put_u64(&mut out, record.data_size);
        put_u64(&mut out, modified_secs(record.meta.modified));
        put_u32(&mut out, unix_mode_field(record.meta.unix_mode));
        put_u32(&mut out, record.crc32c);
        out.extend_from_slice(&record.hash);
        let raw = &record.meta.path.raw;
        let display = record.meta.path.display.as_bytes();
        let encoding = record.meta.path.encoding.as_bytes();
        let link = link_target(&record.meta.entry_type);
        put_len_u32(&mut out, raw.len(), "sqz path")?;
        put_len_u32(&mut out, display.len(), "sqz display path")?;
        put_len_u16(&mut out, encoding.len(), "sqz encoding label")?;
        put_len_u32(&mut out, link.len(), "sqz link target")?;
        out.extend_from_slice(raw);
        out.extend_from_slice(display);
        out.extend_from_slice(encoding);
        out.extend_from_slice(link);
    }
    Ok(out)
}

fn build_footer(
    index_offset: u64,
    index_length: u64,
    recovery_offset: u64,
    recovery_length: u64,
    uuid_hi: u64,
    uuid_lo: u64,
) -> [u8; FOOTER_LEN] {
    let mut footer = [0u8; FOOTER_LEN];
    footer[0..8].copy_from_slice(&index_offset.to_le_bytes());
    footer[8..16].copy_from_slice(&index_length.to_le_bytes());
    footer[16..24].copy_from_slice(&recovery_offset.to_le_bytes());
    footer[24..32].copy_from_slice(&recovery_length.to_le_bytes());
    footer[32..40].copy_from_slice(&uuid_hi.to_le_bytes());
    footer[40..48].copy_from_slice(&uuid_lo.to_le_bytes());
    let crc = crc32c_bytes(&footer[..48]);
    footer[48..52].copy_from_slice(&crc.to_le_bytes());
    footer[56..64].copy_from_slice(TAIL_MAGIC);
    footer
}

struct RecoveryEncoder {
    payload_start: u64,
    payload_length: u64,
    block_size: usize,
    data_shards: usize,
    parity_shards: usize,
    current_block: Vec<u8>,
    current_group: Vec<Vec<u8>>,
    block_hashes: Vec<[u8; 32]>,
    parity_groups: Vec<Vec<Vec<u8>>>,
}

impl RecoveryEncoder {
    fn new(payload_start: u64, opts: &SqzCreateOptions) -> Self {
        Self {
            payload_start,
            payload_length: 0,
            block_size: BLOCK_SIZE as usize,
            data_shards: SqzCreateOptions::DATA_SHARDS,
            parity_shards: opts.parity_shards(),
            current_block: Vec::with_capacity(BLOCK_SIZE as usize),
            current_group: Vec::new(),
            block_hashes: Vec::new(),
            parity_groups: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            payload_start: 0,
            payload_length: 0,
            block_size: BLOCK_SIZE as usize,
            data_shards: SqzCreateOptions::DATA_SHARDS,
            parity_shards: 1,
            current_block: Vec::new(),
            current_group: Vec::new(),
            block_hashes: Vec::new(),
            parity_groups: Vec::new(),
        }
    }

    fn push_bytes(&mut self, mut bytes: &[u8]) -> Result<(), FormatError> {
        self.payload_length += bytes.len() as u64;
        while !bytes.is_empty() {
            let free = self.block_size - self.current_block.len();
            let take = free.min(bytes.len());
            self.current_block.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.current_block.len() == self.block_size {
                self.finish_block()?;
            }
        }
        Ok(())
    }

    fn finish(mut self, index: &[u8]) -> Result<Vec<u8>, FormatError> {
        if !self.current_block.is_empty() {
            self.finish_block()?;
        }
        if !self.current_group.is_empty() {
            self.finish_group()?;
        }
        if self.block_hashes.is_empty() && index.is_empty() {
            return Ok(Vec::new());
        }

        let mut primary = Vec::new();
        let index_len: u64 = index
            .len()
            .try_into()
            .map_err(|_| FormatError::Unsupported("sqz index mirror exceeds 16 EiB".into()))?;
        primary.extend_from_slice(RECOVERY_MAGIC);
        put_u16(&mut primary, RECOVERY_VERSION);
        put_u16(&mut primary, RECOVERY_ALGO_RS_GF8);
        put_u32(&mut primary, BLOCK_SIZE);
        put_u16(&mut primary, self.data_shards as u16);
        put_u16(&mut primary, self.parity_shards as u16);
        put_u32(&mut primary, 0);
        put_u64(&mut primary, self.payload_start);
        put_u64(&mut primary, self.payload_length);
        put_u64(&mut primary, self.block_hashes.len() as u64);
        put_u64(&mut primary, index_len);
        primary.extend_from_slice(blake3::hash(index).as_bytes());
        for hash in &self.block_hashes {
            primary.extend_from_slice(hash);
        }
        for group in &self.parity_groups {
            for shard in group {
                primary.extend_from_slice(shard);
            }
        }
        primary.extend_from_slice(index);
        protect_recovery_section(&primary)
    }

    fn finish_block(&mut self) -> Result<(), FormatError> {
        self.block_hashes
            .push(*blake3::hash(&self.current_block).as_bytes());
        let mut shard = std::mem::take(&mut self.current_block);
        shard.resize(self.block_size, 0);
        self.current_group.push(shard);
        self.current_block = Vec::with_capacity(self.block_size);
        if self.current_group.len() == self.data_shards {
            self.finish_group()?;
        }
        Ok(())
    }

    fn finish_group(&mut self) -> Result<(), FormatError> {
        let data_count = self.current_group.len();
        let codec = ReedSolomon::new(data_count, self.parity_shards)
            .map_err(|e| FormatError::Other(format!("sqz recovery encoder init failed: {e}")))?;
        let mut shards = std::mem::take(&mut self.current_group);
        shards.extend((0..self.parity_shards).map(|_| vec![0u8; self.block_size]));
        codec
            .encode(&mut shards)
            .map_err(|e| FormatError::Other(format!("sqz recovery encode failed: {e}")))?;
        self.parity_groups.push(shards.split_off(data_count));
        Ok(())
    }
}

fn protect_recovery_section(primary: &[u8]) -> Result<Vec<u8>, FormatError> {
    if primary.is_empty() {
        return Ok(Vec::new());
    }
    let block_size = BLOCK_SIZE as usize;
    let data_shards = RECOVERY_DATA_SHARDS as usize;
    let parity_shards = RECOVERY_PARITY_SHARDS as usize;
    let block_count = primary.len().div_ceil(block_size);
    let mut block_hashes = Vec::with_capacity(block_count);
    let mut parity_groups = Vec::with_capacity(block_count.div_ceil(data_shards));
    let codec = ReedSolomon::new(data_shards, parity_shards)
        .map_err(|e| FormatError::Other(format!("sqz recovery protection init failed: {e}")))?;

    for group_start in (0..block_count).step_by(data_shards) {
        let data_count = (block_count - group_start).min(data_shards);
        let mut shards = Vec::with_capacity(data_shards + parity_shards);
        for local in 0..data_count {
            let block_index = group_start + local;
            let start = block_index * block_size;
            let end = primary.len().min(start + block_size);
            block_hashes.push(*blake3::hash(&primary[start..end]).as_bytes());
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

    let mut protection = Vec::new();
    for hash in &block_hashes {
        protection.extend_from_slice(hash);
    }
    for group in &parity_groups {
        for shard in group {
            protection.extend_from_slice(shard);
        }
    }

    let primary_len: u64 = primary
        .len()
        .try_into()
        .map_err(|_| FormatError::Unsupported("sqz recovery section exceeds 16 EiB".into()))?;
    let block_count_u64: u64 = block_count.try_into().map_err(|_| {
        FormatError::Unsupported("sqz recovery protection block count exceeds 16 EiB".into())
    })?;
    let protection_len: u64 = protection
        .len()
        .try_into()
        .map_err(|_| FormatError::Unsupported("sqz recovery protection exceeds 16 EiB".into()))?;

    let mut trailer = Vec::with_capacity(RECOVERY_PROTECTION_TRAILER_LEN);
    trailer.extend_from_slice(RECOVERY_PROTECTION_MAGIC);
    put_u16(&mut trailer, RECOVERY_PROTECTION_VERSION);
    put_u16(&mut trailer, RECOVERY_ALGO_RS_GF8);
    put_u32(&mut trailer, BLOCK_SIZE);
    put_u16(&mut trailer, RECOVERY_DATA_SHARDS);
    put_u16(&mut trailer, RECOVERY_PARITY_SHARDS);
    put_u32(&mut trailer, 0);
    put_u64(&mut trailer, primary_len);
    put_u64(&mut trailer, block_count_u64);
    put_u64(&mut trailer, protection_len);
    trailer.extend_from_slice(blake3::hash(primary).as_bytes());
    let crc = crc32c_bytes(&trailer);
    put_u32(&mut trailer, crc);
    debug_assert_eq!(trailer.len(), RECOVERY_PROTECTION_TRAILER_LEN);

    let mut out = Vec::with_capacity(primary.len() + protection.len() + trailer.len());
    out.extend_from_slice(primary);
    out.extend_from_slice(&protection);
    out.extend_from_slice(&trailer);
    Ok(out)
}

fn kind_of(entry_type: &EntryType) -> u8 {
    match entry_type {
        EntryType::File => KIND_FILE,
        EntryType::Dir => KIND_DIR,
        EntryType::Symlink { .. } => KIND_SYMLINK,
        EntryType::Hardlink { .. } => KIND_HARDLINK,
        EntryType::Other => KIND_OTHER,
    }
}

fn link_target(entry_type: &EntryType) -> &[u8] {
    match entry_type {
        EntryType::Symlink { target } | EntryType::Hardlink { target } => target,
        _ => &[],
    }
}

fn modified_secs(modified: Option<SystemTime>) -> u64 {
    match modified {
        Some(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs(),
            Err(_) => ABSENT_TIMESTAMP_SECS,
        },
        None => ABSENT_TIMESTAMP_SECS,
    }
}

fn unix_mode_field(unix_mode: Option<u32>) -> u32 {
    match unix_mode {
        Some(mode) => mode,
        None => ABSENT_UNIX_MODE,
    }
}

fn put_len_u32(out: &mut Vec<u8>, len: usize, label: &str) -> Result<(), FormatError> {
    let len: u32 = len
        .try_into()
        .map_err(|_| FormatError::Unsupported(format!("{label} exceeds 4 GiB")))?;
    put_u32(out, len);
    Ok(())
}

fn put_len_u16(out: &mut Vec<u8>, len: usize, label: &str) -> Result<(), FormatError> {
    let len: u16 = len
        .try_into()
        .map_err(|_| FormatError::Unsupported(format!("{label} exceeds 64 KiB")))?;
    put_u16(out, len);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_payload_path_prefers_compressed_payload() {
        let original = TempPathGuard {
            path: PathBuf::from("inner.tar"),
        };
        let compressed = TempPathGuard {
            path: PathBuf::from("inner.tar.zst"),
        };

        assert_eq!(
            selected_payload_path(Some(&compressed), &original),
            Path::new("inner.tar.zst")
        );
        assert_eq!(
            selected_payload_path(None, &original),
            Path::new("inner.tar")
        );
    }

    #[test]
    fn modified_secs_uses_absent_sentinel_for_missing_or_pre_epoch() {
        assert_eq!(modified_secs(None), ABSENT_TIMESTAMP_SECS);
        assert_eq!(
            modified_secs(Some(UNIX_EPOCH - Duration::from_secs(1))),
            ABSENT_TIMESTAMP_SECS
        );
        assert_eq!(
            modified_secs(Some(UNIX_EPOCH + Duration::from_secs(42))),
            42
        );
    }

    #[test]
    fn unix_mode_field_uses_absent_sentinel_only_when_missing() {
        assert_eq!(unix_mode_field(None), ABSENT_UNIX_MODE);
        assert_eq!(unix_mode_field(Some(0o100644)), 0o100644);
    }
}
