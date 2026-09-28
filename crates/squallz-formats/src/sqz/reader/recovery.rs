use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, SeekFrom};
use std::ops::Bound::{Excluded, Unbounded};

use reed_solomon_erasure::galois_8::ReedSolomon;
use squallz_format_api::{FormatError, ReadSeek, RecoverySummary};

use super::{valid_header_uuid, Footer, VERIFY_CHUNK};
use crate::sqz::{
    crc32c_bytes, fixed_array, read_array, read_exact, read_u16, read_u32, read_u64, BLOCK_SIZE,
    FOOTER_LEN, HEADER_LEN, RECOVERY_ALGO_RS_GF8, RECOVERY_DATA_SHARDS, RECOVERY_MAGIC,
    RECOVERY_PARITY_SHARDS, RECOVERY_PROTECTION_MAGIC, RECOVERY_PROTECTION_TRAILER_LEN,
    RECOVERY_PROTECTION_VERSION, RECOVERY_VERSION,
};

const RECOVERY_HEADER_LEN: usize = 84;
const HASH_CHUNK: usize = BLOCK_SIZE as usize;
const FOOTER_RECOVERY_SCAN_WINDOW: u64 = 64 * 1024 * 1024;

pub(super) struct RecoveryState {
    pub(super) payload_start: u64,
    pub(super) payload_length: u64,
    pub(super) block_size: usize,
    data_shards: usize,
    parity_shards: usize,
    block_hashes: Vec<[u8; 32]>,
    parity_offset: u64,
    section: RecoverySection,
    index_hash: [u8; 32],
    index_mirror: Vec<u8>,
    pub(super) repaired_blocks: HashMap<u64, Vec<u8>>,
    pub(super) unrepaired_blocks: HashSet<u64>,
}

/// Retains reconstructed blocks and fingerprints to verify later metadata/parity reads.
struct RecoverySection {
    start: u64,
    len: u64,
    patch_size: usize,
    patches: BTreeMap<u64, Vec<u8>>,
    verified_length: u64,
    verified_hashes: Vec<[u8; 32]>,
}

impl RecoverySection {
    fn new(start: u64, len: u64) -> Result<Self, FormatError> {
        start
            .checked_add(len)
            .ok_or_else(|| corrupt("sqz recovery section overflows"))?;
        Ok(Self {
            start,
            len,
            patch_size: BLOCK_SIZE as usize,
            patches: BTreeMap::new(),
            verified_length: 0,
            verified_hashes: Vec::new(),
        })
    }

    fn read_exact(
        &self,
        src: &mut dyn ReadSeek,
        mut offset: u64,
        mut bytes: &mut [u8],
    ) -> Result<(), FormatError> {
        if bytes.is_empty() || offset >= self.verified_length {
            return self.read_raw(src, offset, bytes);
        }
        let mut block = vec![0; HASH_CHUNK];
        while !bytes.is_empty() && offset < self.verified_length {
            let index = offset / HASH_CHUNK as u64;
            let start = index * HASH_CHUNK as u64;
            let within = (offset - start) as usize;
            let count = (self.verified_length - start).min(HASH_CHUNK as u64) as usize;
            self.read_raw(src, start, &mut block[..count])?;
            if self.verified_hashes.get(index as usize)
                != Some(blake3::hash(&block[..count]).as_bytes())
            {
                return Err(corrupt(
                    "sqz recovery section changed while opening archive",
                ));
            }
            let copied = bytes.len().min(count - within);
            bytes[..copied].copy_from_slice(&block[within..within + copied]);
            offset += copied as u64;
            bytes = &mut bytes[copied..];
        }
        self.read_raw(src, offset, bytes)
    }

    fn read_raw(
        &self,
        src: &mut dyn ReadSeek,
        mut offset: u64,
        mut bytes: &mut [u8],
    ) -> Result<(), FormatError> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(corrupt("truncated sqz recovery section"));
        }
        while !bytes.is_empty() {
            let block = offset / self.patch_size as u64;
            let within = (offset % self.patch_size as u64) as usize;
            let count = bytes.len().min(VERIFY_CHUNK);
            src.seek(SeekFrom::Start(self.start + offset))?;
            let count = if let Some(patched) = self.patches.get(&block).filter(|p| within < p.len())
            {
                let count = count.min(patched.len() - within);
                bytes[..count].copy_from_slice(&patched[within..within + count]);
                count
            } else {
                let count = match self.patches.range((Excluded(block), Unbounded)).next() {
                    Some((next, _)) => {
                        (next * self.patch_size as u64 - offset).min(count as u64) as usize
                    }
                    None => count,
                };
                src.read_exact(&mut bytes[..count])?;
                count
            };
            offset += count as u64;
            bytes = &mut bytes[count..];
        }
        Ok(())
    }

    fn verify_primary(
        &mut self,
        src: &mut dyn ReadSeek,
        len: u64,
        expected: &[u8; 32],
    ) -> Result<bool, FormatError> {
        let mut hash = blake3::Hasher::new();
        let mut hashes = Vec::new();
        let mut buffer = [0; HASH_CHUNK];
        let mut offset = 0;
        while offset < len {
            let count = (len - offset).min(buffer.len() as u64) as usize;
            self.read_raw(src, offset, &mut buffer[..count])?;
            hash.update(&buffer[..count]);
            hashes.push(*blake3::hash(&buffer[..count]).as_bytes());
            offset += count as u64;
        }
        if hash.finalize().as_bytes() != expected {
            return Ok(false);
        }
        self.verified_length = len;
        self.verified_hashes = hashes;
        Ok(true)
    }
}

impl RecoveryState {
    pub(super) fn load(
        src: &mut dyn ReadSeek,
        footer: &Footer,
    ) -> Result<Option<Self>, FormatError> {
        if footer.recovery_length == 0 {
            return Ok(None);
        }
        let section = RecoverySection::new(footer.recovery_offset, footer.recovery_length)?;
        if section.start + section.len > footer.index_offset {
            return Err(corrupt("sqz recovery section overlaps footer index"));
        }
        let mut state = Self::read_protected(src, section)?;
        state.repair(src)?;
        Ok(Some(state))
    }

    fn read_protected(
        src: &mut dyn ReadSeek,
        mut section: RecoverySection,
    ) -> Result<Self, FormatError> {
        let trailer_start = section
            .len
            .checked_sub(RECOVERY_PROTECTION_TRAILER_LEN as u64)
            .ok_or_else(|| corrupt("sqz recovery section is missing its protection trailer"))?;
        let mut trailer_bytes = [0; RECOVERY_PROTECTION_TRAILER_LEN];
        section.read_exact(src, trailer_start, &mut trailer_bytes)?;
        let trailer = match parse_recovery_protection_trailer(&trailer_bytes) {
            Ok(Some(trailer)) => trailer,
            Ok(None) => {
                return Err(corrupt(
                    "sqz recovery section is missing its protection trailer",
                ))
            }
            Err(error) => {
                if trailer_crc_mismatch(&trailer_bytes) {
                    return Self::read_with_damaged_trailer(src, section);
                }
                return Err(error);
            }
        };
        if trailer
            .primary_length
            .checked_add(trailer.protection_length)
            != Some(trailer_start)
        {
            return Err(corrupt("sqz recovery protection length mismatch"));
        }
        section.patch_size = trailer.block_size;
        if !section.verify_primary(src, trailer.primary_length, &trailer.primary_hash)? {
            repair_protected_recovery(src, &mut section, &trailer)?;
            if !section.verify_primary(src, trailer.primary_length, &trailer.primary_hash)? {
                return Err(corrupt(
                    "sqz recovery section protection could not restore primary hash",
                ));
            }
        }
        let (state, consumed) = Self::read_primary(src, section, trailer.primary_length)?;
        if consumed != trailer.primary_length {
            return Err(corrupt("trailing bytes in sqz recovery section"));
        }
        Ok(state)
    }

    fn read_primary(
        src: &mut dyn ReadSeek,
        section: RecoverySection,
        limit: u64,
    ) -> Result<(Self, u64), FormatError> {
        let mut header = [0; RECOVERY_HEADER_LEN];
        section.read_exact(src, 0, &mut header)?;
        let mut buf = header.as_slice();
        if read_exact(&mut buf, 4)? != RECOVERY_MAGIC {
            return Err(corrupt("missing sqz recovery magic"));
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
        validate_shards(block_size, data_shards, parity_shards)?;
        if block_count != payload_length.div_ceil(block_size as u64) {
            return Err(corrupt(
                "sqz recovery block count does not match payload length",
            ));
        }
        let payload_end = payload_start
            .checked_add(payload_length)
            .ok_or_else(|| corrupt("sqz recovery payload length overflows"))?;
        if payload_end > section.start {
            return Err(corrupt("sqz recovery payload overlaps recovery section"));
        }
        let hash_len = checked_mul(block_count, 32)?;
        let parity_offset = (RECOVERY_HEADER_LEN as u64)
            .checked_add(hash_len)
            .ok_or_else(|| corrupt("sqz recovery hash range overflows"))?;
        let group_count = block_count.div_ceil(data_shards as u64);
        let parity_len = checked_mul(
            checked_mul(group_count, parity_shards as u64)?,
            block_size as u64,
        )?;
        let index_offset = parity_offset
            .checked_add(parity_len)
            .ok_or_else(|| corrupt("sqz recovery parity range overflows"))?;
        let consumed = index_offset
            .checked_add(index_length)
            .filter(|end| *end <= limit && *end <= section.len)
            .ok_or_else(|| corrupt("truncated sqz recovery section"))?;
        let hash_count = usize::try_from(block_count).map_err(|_| {
            FormatError::Unsupported("sqz recovery block count is too large".into())
        })?;
        let mut block_hashes = Vec::new();
        block_hashes.try_reserve_exact(hash_count).map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "sqz recovery block hashes exceed available memory".into(),
            )
        })?;
        let mut hashes = [0; VERIFY_CHUNK];
        let mut offset = RECOVERY_HEADER_LEN as u64;
        while offset < parity_offset {
            let count = (parity_offset - offset).min(hashes.len() as u64) as usize;
            section.read_exact(src, offset, &mut hashes[..count])?;
            block_hashes.extend_from_slice(hashes[..count].as_chunks::<32>().0);
            offset += count as u64;
        }
        let index_len = usize::try_from(index_length).map_err(|_| {
            FormatError::Unsupported("sqz recovery index mirror is too large".into())
        })?;
        let mut index_mirror = Vec::new();
        index_mirror.try_reserve_exact(index_len).map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "sqz recovery index mirror exceeds available memory".into(),
            )
        })?;
        index_mirror.resize(index_len, 0);
        section.read_exact(src, index_offset, &mut index_mirror)?;
        if *blake3::hash(&index_mirror).as_bytes() != index_hash {
            return Err(corrupt("sqz recovery index mirror hash mismatch"));
        }
        Ok((
            Self {
                payload_start,
                payload_length,
                block_size,
                data_shards,
                parity_shards,
                block_hashes,
                parity_offset,
                section,
                index_hash,
                index_mirror,
                repaired_blocks: HashMap::new(),
                unrepaired_blocks: HashSet::new(),
            },
            consumed,
        ))
    }

    fn read_with_damaged_trailer(
        src: &mut dyn ReadSeek,
        section: RecoverySection,
    ) -> Result<Self, FormatError> {
        let trailer_start = section.len - RECOVERY_PROTECTION_TRAILER_LEN as u64;
        let (state, primary_len) =
            Self::read_primary(src, section, trailer_start).map_err(fallback_error)?;
        let Self { mut section, .. } = state;
        if !protection_matches_primary(src, &mut section, primary_len, trailer_start - primary_len)?
        {
            return Err(damaged_trailer_error());
        }
        let (state, consumed) =
            Self::read_primary(src, section, primary_len).map_err(fallback_error)?;
        if consumed != primary_len {
            return Err(damaged_trailer_error());
        }
        Ok(state)
    }

    pub(super) fn index_bytes(&self, primary: &[u8]) -> Result<Vec<u8>, FormatError> {
        if *blake3::hash(primary).as_bytes() == self.index_hash {
            return Ok(primary.to_vec());
        }
        if self.index_mirror.is_empty() {
            return Err(corrupt(
                "sqz footer index hash mismatch and no recovery mirror is available",
            ));
        }
        Ok(self.index_mirror.clone())
    }

    pub(super) fn repair(&mut self, src: &mut dyn ReadSeek) -> Result<(), FormatError> {
        for group_index in 0..self.block_hashes.len().div_ceil(self.data_shards) {
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
        let data_count = (self.block_hashes.len() - start_block).min(self.data_shards);
        let mut shards = Vec::with_capacity(data_count + self.parity_shards);
        let mut missing = Vec::new();
        for local in 0..data_count {
            let block_index = start_block + local;
            let block = self.read_payload_block(src, block_index as u64)?;
            let actual_len = self.block_actual_len(block_index as u64);
            if *blake3::hash(&block[..actual_len]).as_bytes() == self.block_hashes[block_index] {
                shards.push(Some(block));
            } else {
                missing.push(block_index as u64);
                shards.push(None);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let parity_start = self.parity_offset
            + group_index as u64 * self.parity_shards as u64 * self.block_size as u64;
        for parity in 0..self.parity_shards {
            let mut block = vec![0; self.block_size];
            self.section.read_exact(
                src,
                parity_start + parity as u64 * self.block_size as u64,
                &mut block,
            )?;
            shards.push(Some(block));
        }
        let codec = ReedSolomon::new(data_count, self.parity_shards)
            .map_err(|e| FormatError::Other(format!("sqz recovery decoder init failed: {e}")))?;
        if codec.reconstruct_data(&mut shards).is_err() {
            self.unrepaired_blocks.extend(missing);
            return Ok(());
        }
        for block_index in missing {
            let local = block_index as usize - start_block;
            let Some(mut block) = shards[local].take() else {
                self.unrepaired_blocks.insert(block_index);
                continue;
            };
            let actual_len = self.block_actual_len(block_index);
            if *blake3::hash(&block[..actual_len]).as_bytes()
                == self.block_hashes[block_index as usize]
            {
                block.truncate(actual_len);
                self.repaired_blocks.insert(block_index, block);
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
        let offset = self.payload_start + block_index * self.block_size as u64;
        let mut block = vec![0; self.block_size];
        src.seek(SeekFrom::Start(offset))?;
        if let Err(error) = src.read_exact(&mut block[..actual_len]) {
            if error.kind() != io::ErrorKind::UnexpectedEof {
                return Err(error.into());
            }
        }
        Ok(block)
    }

    fn block_actual_len(&self, block_index: u64) -> usize {
        (self.payload_length - block_index * self.block_size as u64).min(self.block_size as u64)
            as usize
    }

    pub(super) fn summary(&self) -> RecoverySummary {
        let repaired_blocks = self.repaired_blocks.len() as u64;
        let unrepaired_blocks = self.unrepaired_blocks.len() as u64;
        RecoverySummary {
            scheme: "sqz-embedded-rs-gf8".to_string(),
            block_size: self.block_size as u64,
            total_blocks: self.block_hashes.len() as u64,
            data_shards: self.data_shards as u64,
            parity_shards: self.parity_shards as u64,
            recovery_blocks_available: self.block_hashes.len().div_ceil(self.data_shards) as u64
                * self.parity_shards as u64,
            damaged_blocks: repaired_blocks + unrepaired_blocks,
            repaired_blocks,
            unrepaired_blocks,
            repair_possible: unrepaired_blocks == 0,
        }
    }
}

fn corrupt(message: &str) -> FormatError {
    FormatError::CorruptArchive(message.into())
}

fn checked_mul(left: u64, right: u64) -> Result<u64, FormatError> {
    left.checked_mul(right)
        .ok_or_else(|| corrupt("sqz recovery size overflows"))
}

fn validate_shards(
    block_size: usize,
    data_shards: usize,
    parity_shards: usize,
) -> Result<(), FormatError> {
    if block_size == 0 || block_size > 16 * 1024 * 1024 {
        return Err(corrupt("invalid sqz recovery block size"));
    }
    if data_shards == 0 || parity_shards == 0 || data_shards + parity_shards > 255 {
        return Err(corrupt("invalid sqz recovery shard counts"));
    }
    Ok(())
}

fn damaged_trailer_error() -> FormatError {
    corrupt("sqz recovery protection trailer is damaged and primary fallback failed")
}

fn fallback_error(error: FormatError) -> FormatError {
    match error {
        FormatError::CorruptArchive(_) | FormatError::Unsupported(_) => damaged_trailer_error(),
        other => other,
    }
}

fn trailer_crc_mismatch(trailer: &[u8; RECOVERY_PROTECTION_TRAILER_LEN]) -> bool {
    &trailer[..4] == RECOVERY_PROTECTION_MAGIC
        && crc32c_bytes(&trailer[..76])
            != u32::from_le_bytes([trailer[76], trailer[77], trailer[78], trailer[79]])
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

fn protection_layout(
    blocks: u64,
    data_shards: usize,
    parity_shards: usize,
    block_size: usize,
) -> Result<(u64, u64), FormatError> {
    let hash_len = checked_mul(blocks, 32)?;
    let parity_len = checked_mul(
        checked_mul(blocks.div_ceil(data_shards as u64), parity_shards as u64)?,
        block_size as u64,
    )?;
    let length = hash_len
        .checked_add(parity_len)
        .ok_or_else(|| corrupt("sqz recovery protection length overflows"))?;
    Ok((hash_len, length))
}

fn protection_matches_primary(
    src: &mut dyn ReadSeek,
    section: &mut RecoverySection,
    primary_len: u64,
    protection_len: u64,
) -> Result<bool, FormatError> {
    let block_size = BLOCK_SIZE as usize;
    let data_shards = RECOVERY_DATA_SHARDS as usize;
    let parity_shards = RECOVERY_PARITY_SHARDS as usize;
    let block_count = primary_len.div_ceil(block_size as u64);
    let (hash_len, expected_len) =
        protection_layout(block_count, data_shards, parity_shards, block_size)?;
    if protection_len != expected_len {
        return Ok(false);
    }
    let codec = ReedSolomon::new(data_shards, parity_shards)
        .map_err(|e| FormatError::Other(format!("sqz recovery protection init failed: {e}")))?;
    let mut hashes = Vec::new();
    for group in 0..block_count.div_ceil(data_shards as u64) {
        let mut shards = Vec::with_capacity(data_shards + parity_shards);
        for local in 0..data_shards {
            let index = group * data_shards as u64 + local as u64;
            let mut block = vec![0; block_size];
            if index < block_count {
                let start = index * block_size as u64;
                let count = (primary_len - start).min(block_size as u64) as usize;
                section.read_exact(src, start, &mut block[..count])?;
                let mut expected_hash = [0; 32];
                section.read_exact(src, primary_len + index * 32, &mut expected_hash)?;
                if *blake3::hash(&block[..count]).as_bytes() != expected_hash {
                    return Ok(false);
                }
                hashes.push(expected_hash);
            }
            shards.push(block);
        }
        shards.extend((0..parity_shards).map(|_| vec![0; block_size]));
        codec.encode(&mut shards).map_err(|e| {
            FormatError::Other(format!("sqz recovery protection encode failed: {e}"))
        })?;
        let parity_start =
            primary_len + hash_len + group * parity_shards as u64 * block_size as u64;
        let mut expected = vec![0; block_size];
        for (index, parity) in shards[data_shards..].iter().enumerate() {
            section.read_exact(
                src,
                parity_start + index as u64 * block_size as u64,
                &mut expected,
            )?;
            if parity != &expected {
                return Ok(false);
            }
        }
    }
    section.verified_length = primary_len;
    section.verified_hashes = hashes;
    Ok(true)
}

fn repair_protected_recovery(
    src: &mut dyn ReadSeek,
    section: &mut RecoverySection,
    trailer: &RecoveryProtectionTrailer,
) -> Result<(), FormatError> {
    let (hash_len, expected_len) = protection_layout(
        trailer.block_count,
        trailer.data_shards,
        trailer.parity_shards,
        trailer.block_size,
    )?;
    if trailer.protection_length != expected_len {
        return Err(corrupt("sqz recovery protection payload length mismatch"));
    }
    for group in 0..trailer.block_count.div_ceil(trailer.data_shards as u64) {
        let start_block = group * trailer.data_shards as u64;
        let mut shards = Vec::with_capacity(trailer.data_shards + trailer.parity_shards);
        let mut missing = Vec::new();
        let mut hashes = Vec::with_capacity(trailer.data_shards);
        for local in 0..trailer.data_shards {
            let index = start_block + local as u64;
            let mut block = vec![0; trailer.block_size];
            if index >= trailer.block_count {
                shards.push(Some(block));
                hashes.push([0; 32]);
                continue;
            }
            let start = index * trailer.block_size as u64;
            let count = (trailer.primary_length - start).min(trailer.block_size as u64) as usize;
            section.read_exact(src, start, &mut block[..count])?;
            let mut hash = [0; 32];
            section.read_exact(src, trailer.primary_length + index * 32, &mut hash)?;
            if *blake3::hash(&block[..count]).as_bytes() == hash {
                shards.push(Some(block));
            } else {
                shards.push(None);
                missing.push(index);
            }
            hashes.push(hash);
        }
        if missing.is_empty() {
            continue;
        }
        let parity_start = trailer.primary_length
            + hash_len
            + group * trailer.parity_shards as u64 * trailer.block_size as u64;
        for index in 0..trailer.parity_shards {
            let mut block = vec![0; trailer.block_size];
            section.read_exact(
                src,
                parity_start + index as u64 * trailer.block_size as u64,
                &mut block,
            )?;
            shards.push(Some(block));
        }
        let codec = ReedSolomon::new(trailer.data_shards, trailer.parity_shards).map_err(|e| {
            FormatError::Other(format!("sqz recovery protection decoder init failed: {e}"))
        })?;
        codec
            .reconstruct_data(&mut shards)
            .map_err(|_| corrupt("sqz recovery section damage exceeds protection capacity"))?;
        for index in missing {
            let local = (index - start_block) as usize;
            let Some(mut block) = shards[local].take() else {
                return Err(corrupt(
                    "sqz recovery protection did not reconstruct a missing block",
                ));
            };
            let count = (trailer.primary_length - index * trailer.block_size as u64)
                .min(trailer.block_size as u64) as usize;
            if *blake3::hash(&block[..count]).as_bytes() != hashes[local] {
                return Err(corrupt(
                    "sqz recovery protection reconstructed block hash mismatch",
                ));
            }
            block.truncate(count);
            section.patches.insert(index, block);
        }
    }
    Ok(())
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

pub(super) fn recover_footer_from_recovery_scan(
    src: &mut dyn ReadSeek,
    len: u64,
) -> Result<Option<(Footer, Option<RecoveryState>)>, FormatError> {
    let Some((uuid_hi, uuid_lo)) = valid_header_uuid(src)? else {
        return Ok(None);
    };
    let scan_start = len.saturating_sub(FOOTER_RECOVERY_SCAN_WINDOW);
    let mut scan_end = len;
    let mut window = vec![0; VERIFY_CHUNK + RECOVERY_PROTECTION_TRAILER_LEN - 1];
    while scan_end > scan_start {
        let start = scan_end.saturating_sub(VERIFY_CHUNK as u64).max(scan_start);
        let read_end = scan_end
            .saturating_add(RECOVERY_PROTECTION_TRAILER_LEN as u64 - 1)
            .min(len);
        let count = (read_end - start) as usize;
        src.seek(SeekFrom::Start(start))?;
        src.read_exact(&mut window[..count])?;
        for pos in (0..count.saturating_sub(RECOVERY_PROTECTION_TRAILER_LEN - 1)).rev() {
            if start + pos as u64 >= scan_end || &window[pos..pos + 4] != RECOVERY_PROTECTION_MAGIC
            {
                continue;
            }
            let Ok(Some(trailer)) = parse_recovery_protection_trailer(
                &window[pos..pos + RECOVERY_PROTECTION_TRAILER_LEN],
            ) else {
                continue;
            };
            let Some(content_len) = trailer
                .primary_length
                .checked_add(trailer.protection_length)
            else {
                continue;
            };
            let Some(section_len) = content_len.checked_add(RECOVERY_PROTECTION_TRAILER_LEN as u64)
            else {
                continue;
            };
            let trailer_abs = start + pos as u64;
            let Some(recovery_offset) = trailer_abs.checked_sub(content_len) else {
                continue;
            };
            let recovery_end = trailer_abs + RECOVERY_PROTECTION_TRAILER_LEN as u64;
            if recovery_offset < HEADER_LEN as u64
                || recovery_end > len.saturating_sub(FOOTER_LEN as u64)
            {
                continue;
            }
            let section = RecoverySection::new(recovery_offset, section_len)?;
            let recovery = match RecoveryState::read_protected(src, section) {
                Ok(recovery) => recovery,
                Err(FormatError::CorruptArchive(_) | FormatError::Unsupported(_)) => continue,
                Err(other) => return Err(other),
            };
            let index_length = recovery.index_mirror.len() as u64;
            if index_length == 0
                || recovery_end
                    .checked_add(index_length)
                    .is_none_or(|end| end > len.saturating_sub(FOOTER_LEN as u64))
            {
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
        scan_end = start;
    }
    Ok(None)
}
