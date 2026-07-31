//! Native PKWARE split-ZIP writer.
//!
//! The normal ZIP encoder first produces one caller-owned seekable archive.
//! This module then streams its existing local records into physical volumes
//! and rebuilds only the central-directory positions and end records.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use squallz_format_api::{
    ControlToken, EntryPath, FormatError, NativeVolumeWriter, ProgressSink, ReadSeek,
};

const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const CENTRAL_MAGIC: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const EOCD_MAGIC: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_EOCD_MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const ZIP64_LOCATOR_MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const SPLIT_MAGIC: [u8; 4] = [0x50, 0x4b, 0x07, 0x08];
const ZIP64_EXTRA_ID: u16 = 0x0001;
const LOCAL_FIXED_LEN: usize = 30;
const CENTRAL_FIXED_LEN: usize = 46;
const EOCD_FIXED_LEN: usize = 22;
const ZIP64_EOCD_LEN: u64 = 56;
const ZIP64_LOCATOR_LEN: u64 = 20;
const COPY_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DiskPosition {
    disk: u32,
    offset: u64,
}

struct OutputProgress<'a> {
    sink: &'a dyn ProgressSink,
    current: &'a EntryPath,
    written: u64,
    total: u64,
}

impl<'a> OutputProgress<'a> {
    fn new(sink: &'a dyn ProgressSink, current: &'a EntryPath, total: u64) -> Self {
        sink.on_progress(0, total, current);
        Self {
            sink,
            current,
            written: 0,
            total,
        }
    }

    fn write(
        &mut self,
        output: &mut dyn NativeVolumeWriter,
        bytes: &[u8],
    ) -> Result<(), FormatError> {
        output.write_spanning(bytes)?;
        self.written = self
            .written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| corrupt("native ZIP output progress overflow"))?;
        self.sink
            .on_progress(self.written.min(self.total), self.total, self.current);
        Ok(())
    }
}

#[derive(Debug)]
struct CentralEntry {
    bytes: Vec<u8>,
    local_offset: u64,
}

#[derive(Debug)]
struct Directory {
    entries: Vec<CentralEntry>,
    offset: u64,
    size: u64,
    comment: Vec<u8>,
}

pub(super) fn volume_path(
    destination: &Path,
    disk_index: u32,
    final_volume: bool,
) -> Result<PathBuf, FormatError> {
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            FormatError::Unsupported(
                "native ZIP volumes require a destination ending in .zip".into(),
            )
        })?;
    if !extension.eq_ignore_ascii_case("zip") {
        return Err(FormatError::Unsupported(
            "native ZIP volumes require a destination ending in .zip".into(),
        ));
    }
    if destination.file_stem().is_none_or(|value| value.is_empty()) {
        return Err(FormatError::Unsupported(
            "native ZIP volume destination has no base name".into(),
        ));
    }
    if final_volume {
        return Ok(destination.to_path_buf());
    }
    let number = disk_index
        .checked_add(1)
        .ok_or_else(|| volume_limit_error(u32::MAX))?;
    let mut path = destination.to_path_buf();
    path.set_extension(format!("z{number:02}"));
    Ok(path)
}

pub(super) fn write_native_volumes(
    source: &mut dyn ReadSeek,
    output: &mut dyn NativeVolumeWriter,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let current = EntryPath::from_utf8(String::new());
    progress.on_progress(0, 0, &current);
    let copy_buffer_size = output.stream_buffer_size(COPY_CHUNK)?;
    let directory = read_directory(source, ctl)?;
    let total = native_output_size(&directory)?;
    let mut output_progress = OutputProgress::new(progress, &current, total);
    let mut local_order = directory
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.local_offset, index))
        .collect::<Vec<_>>();
    local_order.sort_unstable_by_key(|(offset, _)| *offset);
    for pair in local_order.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(corrupt(
                "ZIP central directory repeats a local-header offset",
            ));
        }
    }

    output.ensure_record_capacity(SPLIT_MAGIC.len() as u64)?;
    output_progress.write(output, &SPLIT_MAGIC)?;

    let first_local_offset = local_order
        .first()
        .map_or(directory.offset, |entry| entry.0);
    if first_local_offset > directory.offset {
        return Err(corrupt(
            "ZIP local-header offset is beyond the central directory",
        ));
    }
    copy_range(
        source,
        output,
        0,
        first_local_offset,
        &mut output_progress,
        copy_buffer_size,
        ctl,
    )?;

    let mut local_positions = vec![None; directory.entries.len()];
    for (order_index, (source_offset, central_index)) in local_order.iter().copied().enumerate() {
        ctl.checkpoint()?;
        let source_end = local_order
            .get(order_index + 1)
            .map_or(directory.offset, |entry| entry.0);
        if source_end <= source_offset {
            return Err(corrupt("ZIP local records overlap or are out of order"));
        }
        source.seek(SeekFrom::Start(source_offset))?;
        let mut fixed = [0u8; LOCAL_FIXED_LEN];
        source.read_exact(&mut fixed)?;
        if fixed[..4] != LOCAL_MAGIC {
            return Err(corrupt("ZIP local-header signature is invalid"));
        }
        let name_len = read_u16(&fixed, 26, "local file name length")? as u64;
        let extra_len = read_u16(&fixed, 28, "local extra-field length")? as u64;
        let header_len = (LOCAL_FIXED_LEN as u64)
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
            .ok_or_else(|| corrupt("ZIP local-header length overflow"))?;
        if source_offset
            .checked_add(header_len)
            .is_none_or(|header_end| header_end > source_end)
        {
            return Err(corrupt("ZIP local header extends into the next record"));
        }
        output.ensure_record_capacity(header_len)?;
        local_positions[central_index] = Some(DiskPosition {
            disk: output.disk_index(),
            offset: output.disk_offset(),
        });
        output_progress.write(output, &fixed)?;
        copy_current(
            source,
            output,
            name_len + extra_len,
            &mut output_progress,
            copy_buffer_size,
            ctl,
        )?;
        copy_current(
            source,
            output,
            source_end - source_offset - header_len,
            &mut output_progress,
            copy_buffer_size,
            ctl,
        )?;
    }

    let mut central_start = None;
    let mut central_size = 0u64;
    let mut entries_per_disk = Vec::<u64>::new();
    for (index, entry) in directory.entries.iter().enumerate() {
        ctl.checkpoint()?;
        let position = local_positions
            .get(index)
            .and_then(|position| *position)
            .ok_or_else(|| corrupt("ZIP local-header position was not recorded"))?;
        let mut bytes = entry.bytes.clone();
        let disk = u16::try_from(position.disk).map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "native ZIP creation currently supports at most 65,535 volumes".into(),
            )
        })?;
        if disk == u16::MAX {
            return Err(FormatError::ResourceLimitExceeded(
                "native ZIP creation currently supports at most 65,535 volumes".into(),
            ));
        }
        let offset = u32::try_from(position.offset).map_err(|_| {
            FormatError::ResourceLimitExceeded(
                "native ZIP volume offsets must fit in 32 bits".into(),
            )
        })?;
        bytes[34..36].copy_from_slice(&disk.to_le_bytes());
        bytes[42..46].copy_from_slice(&offset.to_le_bytes());
        output.ensure_record_capacity(bytes.len() as u64)?;
        let record_position = DiskPosition {
            disk: output.disk_index(),
            offset: output.disk_offset(),
        };
        central_start.get_or_insert(record_position);
        increment_disk_count(&mut entries_per_disk, record_position.disk)?;
        output_progress.write(output, &bytes)?;
        central_size = central_size
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| corrupt("ZIP central-directory size overflow"))?;
    }

    let central_start = central_start.unwrap_or(DiskPosition {
        disk: output.disk_index(),
        offset: output.disk_offset(),
    });
    if central_size != directory.size {
        return Err(corrupt(
            "ZIP central-directory size changed while native volumes were written",
        ));
    }
    write_end_records(
        output,
        &directory.comment,
        central_start,
        central_size,
        directory.entries.len() as u64,
        &entries_per_disk,
        &mut output_progress,
    )?;
    ctl.checkpoint()?;
    if output_progress.written != total {
        return Err(FormatError::Other(
            "native ZIP output progress did not match the bytes written".into(),
        ));
    }
    Ok(())
}

fn read_directory(source: &mut dyn ReadSeek, ctl: &ControlToken) -> Result<Directory, FormatError> {
    ctl.checkpoint()?;
    let end = source.seek(SeekFrom::End(0))?;
    let search_len = end.min((u16::MAX as u64) + EOCD_FIXED_LEN as u64);
    source.seek(SeekFrom::Start(end - search_len))?;
    let tail_len = usize::try_from(search_len)
        .map_err(|_| corrupt("ZIP end-record search window is too large"))?;
    let mut tail = vec![0u8; tail_len];
    source.read_exact(&mut tail)?;
    ctl.checkpoint()?;
    let eocd_index = find_eocd(&tail)?;
    let eocd_offset = end - search_len + eocd_index as u64;
    let eocd = tail
        .get(eocd_index..)
        .ok_or_else(|| corrupt("ZIP end record is truncated"))?;
    let disk = read_u16(eocd, 4, "end-record disk number")?;
    let central_disk = read_u16(eocd, 6, "central-directory disk number")?;
    let entries_on_disk = read_u16(eocd, 8, "entries on final disk")?;
    let total_entries16 = read_u16(eocd, 10, "total entry count")?;
    let central_size32 = read_u32(eocd, 12, "central-directory size")?;
    let central_offset32 = read_u32(eocd, 16, "central-directory offset")?;
    let comment_len = read_u16(eocd, 20, "archive comment length")? as usize;
    let comment = eocd
        .get(EOCD_FIXED_LEN..EOCD_FIXED_LEN + comment_len)
        .ok_or_else(|| corrupt("ZIP archive comment is truncated"))?
        .to_vec();

    let needs_zip64 = disk == u16::MAX
        || central_disk == u16::MAX
        || entries_on_disk == u16::MAX
        || total_entries16 == u16::MAX
        || central_size32 == u32::MAX
        || central_offset32 == u32::MAX;
    let (total_entries, central_size, central_offset) = if needs_zip64 {
        read_zip64_directory(source, eocd_offset)?
    } else {
        if disk != 0 || central_disk != 0 || entries_on_disk != total_entries16 {
            return Err(corrupt(
                "native ZIP conversion requires a complete single-file source archive",
            ));
        }
        (
            total_entries16 as u64,
            central_size32 as u64,
            central_offset32 as u64,
        )
    };
    if central_offset
        .checked_add(central_size)
        .is_none_or(|central_end| central_end > eocd_offset)
    {
        return Err(corrupt(
            "ZIP central directory extends beyond its end record",
        ));
    }

    source.seek(SeekFrom::Start(central_offset))?;
    let capacity = usize::try_from(total_entries)
        .map_err(|_| FormatError::ResourceLimitExceeded("ZIP entry count is too large".into()))?;
    let mut entries = Vec::with_capacity(capacity);
    let mut consumed = 0u64;
    for _ in 0..total_entries {
        ctl.checkpoint()?;
        let mut fixed = [0u8; CENTRAL_FIXED_LEN];
        source.read_exact(&mut fixed)?;
        if fixed[..4] != CENTRAL_MAGIC {
            return Err(corrupt("ZIP central-directory signature is invalid"));
        }
        let name_len = read_u16(&fixed, 28, "central file name length")? as usize;
        let extra_len = read_u16(&fixed, 30, "central extra-field length")? as usize;
        let comment_len = read_u16(&fixed, 32, "central comment length")? as usize;
        let variable_len = name_len
            .checked_add(extra_len)
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| corrupt("ZIP central record length overflow"))?;
        let mut variable = vec![0u8; variable_len];
        source.read_exact(&mut variable)?;
        let extra_start = name_len;
        let extra_end = extra_start + extra_len;
        let extra = variable
            .get(extra_start..extra_end)
            .ok_or_else(|| corrupt("ZIP central extra field is truncated"))?;
        let local_offset32 = read_u32(&fixed, 42, "local-header offset")?;
        let disk_start16 = read_u16(&fixed, 34, "local-header disk")?;
        let (local_offset, disk_start) =
            read_central_zip64_position(&fixed, extra, local_offset32, disk_start16)?;
        if disk_start != 0 {
            return Err(corrupt(
                "native ZIP conversion requires a single-file source archive",
            ));
        }
        let mut bytes = Vec::with_capacity(CENTRAL_FIXED_LEN + variable_len);
        bytes.extend_from_slice(&fixed);
        bytes.extend_from_slice(&variable);
        consumed = consumed
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| corrupt("ZIP central-directory size overflow"))?;
        entries.push(CentralEntry {
            bytes,
            local_offset,
        });
    }
    if consumed != central_size {
        return Err(corrupt(
            "ZIP central-directory size does not match its entry records",
        ));
    }
    Ok(Directory {
        entries,
        offset: central_offset,
        size: central_size,
        comment,
    })
}

fn find_eocd(tail: &[u8]) -> Result<usize, FormatError> {
    if tail.len() < EOCD_FIXED_LEN {
        return Err(corrupt("ZIP end record is missing"));
    }
    for index in (0..=tail.len() - EOCD_FIXED_LEN).rev() {
        if tail[index..index + 4] != EOCD_MAGIC {
            continue;
        }
        let comment_len = read_u16(tail, index + 20, "archive comment length")? as usize;
        if index + EOCD_FIXED_LEN + comment_len == tail.len() {
            return Ok(index);
        }
    }
    Err(corrupt("ZIP end record is missing or truncated"))
}

fn read_zip64_directory(
    source: &mut dyn ReadSeek,
    eocd_offset: u64,
) -> Result<(u64, u64, u64), FormatError> {
    let locator_offset = eocd_offset
        .checked_sub(ZIP64_LOCATOR_LEN)
        .ok_or_else(|| corrupt("ZIP64 locator is missing"))?;
    source.seek(SeekFrom::Start(locator_offset))?;
    let mut locator = [0u8; ZIP64_LOCATOR_LEN as usize];
    source.read_exact(&mut locator)?;
    if locator[..4] != ZIP64_LOCATOR_MAGIC {
        return Err(corrupt("ZIP64 locator signature is invalid"));
    }
    let record_disk = read_u32(&locator, 4, "ZIP64 end-record disk")?;
    let record_offset = read_u64(&locator, 8, "ZIP64 end-record offset")?;
    let total_disks = read_u32(&locator, 16, "ZIP64 total disk count")?;
    if record_disk != 0 || total_disks != 1 {
        return Err(corrupt(
            "native ZIP conversion requires a complete single-file source archive",
        ));
    }
    source.seek(SeekFrom::Start(record_offset))?;
    let mut record = [0u8; ZIP64_EOCD_LEN as usize];
    source.read_exact(&mut record)?;
    if record[..4] != ZIP64_EOCD_MAGIC {
        return Err(corrupt("ZIP64 end-record signature is invalid"));
    }
    let body_len = read_u64(&record, 4, "ZIP64 end-record size")?;
    if body_len < 44 {
        return Err(corrupt("ZIP64 end record is too short"));
    }
    let disk = read_u32(&record, 16, "ZIP64 current disk")?;
    let central_disk = read_u32(&record, 20, "ZIP64 central-directory disk")?;
    let entries_on_disk = read_u64(&record, 24, "ZIP64 entries on disk")?;
    let total_entries = read_u64(&record, 32, "ZIP64 total entries")?;
    if disk != 0 || central_disk != 0 || entries_on_disk != total_entries {
        return Err(corrupt(
            "native ZIP conversion requires a complete single-file source archive",
        ));
    }
    Ok((
        total_entries,
        read_u64(&record, 40, "ZIP64 central-directory size")?,
        read_u64(&record, 48, "ZIP64 central-directory offset")?,
    ))
}

fn read_central_zip64_position(
    fixed: &[u8],
    extra: &[u8],
    local_offset32: u32,
    disk_start16: u16,
) -> Result<(u64, u32), FormatError> {
    if local_offset32 != u32::MAX && disk_start16 != u16::MAX {
        return Ok((local_offset32 as u64, disk_start16 as u32));
    }
    let uncompressed32 = read_u32(fixed, 24, "uncompressed size")?;
    let compressed32 = read_u32(fixed, 20, "compressed size")?;
    let mut cursor = 0usize;
    while cursor + 4 <= extra.len() {
        let id = read_u16(extra, cursor, "extra-field identifier")?;
        let len = read_u16(extra, cursor + 2, "extra-field length")? as usize;
        let data_start = cursor + 4;
        let data_end = data_start
            .checked_add(len)
            .ok_or_else(|| corrupt("ZIP extra-field length overflow"))?;
        let data = extra
            .get(data_start..data_end)
            .ok_or_else(|| corrupt("ZIP extra field is truncated"))?;
        if id == ZIP64_EXTRA_ID {
            let mut position = 0usize;
            if uncompressed32 == u32::MAX {
                position = skip_zip64_value(data, position, 8)?;
            }
            if compressed32 == u32::MAX {
                position = skip_zip64_value(data, position, 8)?;
            }
            let local_offset = if local_offset32 == u32::MAX {
                let value = read_u64(data, position, "ZIP64 local-header offset")?;
                position = skip_zip64_value(data, position, 8)?;
                value
            } else {
                local_offset32 as u64
            };
            let disk_start = if disk_start16 == u16::MAX {
                read_u32(data, position, "ZIP64 local-header disk")?
            } else {
                disk_start16 as u32
            };
            return Ok((local_offset, disk_start));
        }
        cursor = data_end;
    }
    Err(corrupt(
        "ZIP64 central entry is missing its local-header position",
    ))
}

fn skip_zip64_value(data: &[u8], position: usize, len: usize) -> Result<usize, FormatError> {
    let end = position
        .checked_add(len)
        .ok_or_else(|| corrupt("ZIP64 extra-field length overflow"))?;
    if end > data.len() {
        return Err(corrupt("ZIP64 extra field is truncated"));
    }
    Ok(end)
}

fn write_end_records(
    output: &mut dyn NativeVolumeWriter,
    comment: &[u8],
    central_start: DiskPosition,
    central_size: u64,
    total_entries: u64,
    entries_per_disk: &[u64],
    progress: &mut OutputProgress<'_>,
) -> Result<(), FormatError> {
    let eocd_len = (EOCD_FIXED_LEN as u64)
        .checked_add(comment.len() as u64)
        .ok_or_else(|| corrupt("ZIP end-record length overflow"))?;
    let footer_pair_len = ZIP64_LOCATOR_LEN
        .checked_add(eocd_len)
        .ok_or_else(|| corrupt("ZIP footer length overflow"))?;
    if eocd_len > output.volume_size() || footer_pair_len > output.volume_size() {
        return Err(FormatError::Unsupported(format!(
            "ZIP archive comment is too large for the {}-byte native volume size",
            output.volume_size()
        )));
    }

    let current = DiskPosition {
        disk: output.disk_index(),
        offset: output.disk_offset(),
    };
    let zip64_position = place_record(current, ZIP64_EOCD_LEN, output.volume_size())?;
    let after_zip64 = advance(zip64_position, ZIP64_EOCD_LEN)?;
    let footer_position = place_record(after_zip64, footer_pair_len, output.volume_size())?;
    let final_disk = footer_position.disk;
    let total_disks = final_disk
        .checked_add(1)
        .ok_or_else(|| volume_limit_error(u32::MAX))?;

    output.ensure_record_capacity(ZIP64_EOCD_LEN)?;
    if output.disk_index() != zip64_position.disk || output.disk_offset() != zip64_position.offset {
        return Err(FormatError::Other(
            "native volume sink placed the ZIP64 end record unexpectedly".into(),
        ));
    }
    let zip64_entries = disk_count(entries_per_disk, zip64_position.disk);
    let mut zip64 = Vec::with_capacity(ZIP64_EOCD_LEN as usize);
    zip64.extend_from_slice(&ZIP64_EOCD_MAGIC);
    zip64.extend_from_slice(&44u64.to_le_bytes());
    zip64.extend_from_slice(&45u16.to_le_bytes());
    zip64.extend_from_slice(&45u16.to_le_bytes());
    zip64.extend_from_slice(&zip64_position.disk.to_le_bytes());
    zip64.extend_from_slice(&central_start.disk.to_le_bytes());
    zip64.extend_from_slice(&zip64_entries.to_le_bytes());
    zip64.extend_from_slice(&total_entries.to_le_bytes());
    zip64.extend_from_slice(&central_size.to_le_bytes());
    zip64.extend_from_slice(&central_start.offset.to_le_bytes());
    progress.write(output, &zip64)?;

    output.ensure_record_capacity(footer_pair_len)?;
    if output.disk_index() != footer_position.disk || output.disk_offset() != footer_position.offset
    {
        return Err(FormatError::Other(
            "native volume sink placed the ZIP footer unexpectedly".into(),
        ));
    }
    let mut locator = Vec::with_capacity(ZIP64_LOCATOR_LEN as usize);
    locator.extend_from_slice(&ZIP64_LOCATOR_MAGIC);
    locator.extend_from_slice(&zip64_position.disk.to_le_bytes());
    locator.extend_from_slice(&zip64_position.offset.to_le_bytes());
    locator.extend_from_slice(&total_disks.to_le_bytes());
    progress.write(output, &locator)?;

    let final_entries = disk_count(entries_per_disk, final_disk);
    let mut eocd = Vec::with_capacity(eocd_len as usize);
    eocd.extend_from_slice(&EOCD_MAGIC);
    eocd.extend_from_slice(&classic_u16(final_disk).to_le_bytes());
    eocd.extend_from_slice(&classic_u16(central_start.disk).to_le_bytes());
    eocd.extend_from_slice(&classic_u16_u64(final_entries).to_le_bytes());
    eocd.extend_from_slice(&classic_u16_u64(total_entries).to_le_bytes());
    eocd.extend_from_slice(&classic_u32(central_size).to_le_bytes());
    eocd.extend_from_slice(&classic_u32(central_start.offset).to_le_bytes());
    let comment_len = u16::try_from(comment.len())
        .map_err(|_| corrupt("ZIP archive comment exceeds 65,535 bytes"))?;
    eocd.extend_from_slice(&comment_len.to_le_bytes());
    eocd.extend_from_slice(comment);
    progress.write(output, &eocd)
}

fn place_record(
    position: DiskPosition,
    record_len: u64,
    volume_size: u64,
) -> Result<DiskPosition, FormatError> {
    if record_len > volume_size {
        return Err(FormatError::Unsupported(format!(
            "a {record_len}-byte ZIP record does not fit the {volume_size}-byte native volume"
        )));
    }
    if position
        .offset
        .checked_add(record_len)
        .is_none_or(|end| end > volume_size)
    {
        return Ok(DiskPosition {
            disk: position
                .disk
                .checked_add(1)
                .ok_or_else(|| volume_limit_error(u32::MAX))?,
            offset: 0,
        });
    }
    Ok(position)
}

fn advance(position: DiskPosition, len: u64) -> Result<DiskPosition, FormatError> {
    Ok(DiskPosition {
        disk: position.disk,
        offset: position
            .offset
            .checked_add(len)
            .ok_or_else(|| corrupt("native ZIP volume offset overflow"))?,
    })
}

fn copy_range(
    source: &mut dyn ReadSeek,
    output: &mut dyn NativeVolumeWriter,
    start: u64,
    len: u64,
    progress: &mut OutputProgress<'_>,
    copy_buffer_size: usize,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    source.seek(SeekFrom::Start(start))?;
    copy_current(source, output, len, progress, copy_buffer_size, ctl)
}

fn copy_current(
    source: &mut dyn ReadSeek,
    output: &mut dyn NativeVolumeWriter,
    mut len: u64,
    progress: &mut OutputProgress<'_>,
    copy_buffer_size: usize,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut buffer = vec![0u8; copy_buffer_size];
    while len > 0 {
        ctl.checkpoint()?;
        let want = buffer.len().min(len as usize);
        let read = source.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "ZIP source archive shrank while native volumes were written",
            )));
        }
        progress.write(output, &buffer[..read])?;
        len -= read as u64;
    }
    Ok(())
}

fn native_output_size(directory: &Directory) -> Result<u64, FormatError> {
    (SPLIT_MAGIC.len() as u64)
        .checked_add(directory.offset)
        .and_then(|size| size.checked_add(directory.size))
        .and_then(|size| size.checked_add(ZIP64_EOCD_LEN))
        .and_then(|size| size.checked_add(ZIP64_LOCATOR_LEN))
        .and_then(|size| size.checked_add(EOCD_FIXED_LEN as u64))
        .and_then(|size| size.checked_add(directory.comment.len() as u64))
        .ok_or_else(|| corrupt("native ZIP output size overflow"))
}

fn increment_disk_count(counts: &mut Vec<u64>, disk: u32) -> Result<(), FormatError> {
    let index = usize::try_from(disk)
        .map_err(|_| FormatError::ResourceLimitExceeded("ZIP disk index is too large".into()))?;
    if counts.len() <= index {
        counts.resize(index + 1, 0);
    }
    counts[index] = counts[index]
        .checked_add(1)
        .ok_or_else(|| corrupt("ZIP entry count overflow"))?;
    Ok(())
}

fn disk_count(counts: &[u64], disk: u32) -> u64 {
    usize::try_from(disk)
        .ok()
        .and_then(|index| counts.get(index))
        .copied()
        .unwrap_or(0)
}

fn classic_u16(value: u32) -> u16 {
    u16::try_from(value)
        .ok()
        .filter(|value| *value != u16::MAX)
        .unwrap_or(u16::MAX)
}

fn classic_u16_u64(value: u64) -> u16 {
    u16::try_from(value)
        .ok()
        .filter(|value| *value != u16::MAX)
        .unwrap_or(u16::MAX)
}

fn classic_u32(value: u64) -> u32 {
    u32::try_from(value)
        .ok()
        .filter(|value| *value != u32::MAX)
        .unwrap_or(u32::MAX)
}

fn read_u16(bytes: &[u8], offset: usize, field: &str) -> Result<u16, FormatError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| corrupt(&format!("ZIP {field} is truncated")))?;
    let mut value = [0u8; 2];
    value.copy_from_slice(slice);
    Ok(u16::from_le_bytes(value))
}

fn read_u32(bytes: &[u8], offset: usize, field: &str) -> Result<u32, FormatError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| corrupt(&format!("ZIP {field} is truncated")))?;
    let mut value = [0u8; 4];
    value.copy_from_slice(slice);
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize, field: &str) -> Result<u64, FormatError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| corrupt(&format!("ZIP {field} is truncated")))?;
    let mut value = [0u8; 8];
    value.copy_from_slice(slice);
    Ok(u64::from_le_bytes(value))
}

fn corrupt(message: &str) -> FormatError {
    FormatError::CorruptArchive(message.to_owned())
}

fn volume_limit_error(limit: u32) -> FormatError {
    FormatError::ResourceLimitExceeded(format!("native ZIP volume count exceeds {limit}"))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex};

    use squallz_format_api::{NativeVolumeWriter, NoProgress, ProgressSink, ResourceOptions};
    use zip::write::SimpleFileOptions;

    use super::*;

    struct MemoryVolumes {
        size: u64,
        resources: ResourceOptions,
        volumes: Vec<Vec<u8>>,
    }

    impl MemoryVolumes {
        fn new(size: u64) -> Self {
            Self {
                size,
                resources: ResourceOptions::default(),
                volumes: Vec::new(),
            }
        }

        fn with_stream_buffer_limit(mut self, limit: u64) -> Self {
            self.resources.memory_limit = Some(limit);
            self
        }

        fn ensure_started(&mut self) {
            if self.volumes.is_empty() {
                self.volumes.push(Vec::new());
            }
        }
    }

    impl NativeVolumeWriter for MemoryVolumes {
        fn volume_size(&self) -> u64 {
            self.size
        }

        fn stream_buffer_size(&self, default: usize) -> Result<usize, FormatError> {
            self.resources.stream_buffer_size(default)
        }

        fn disk_index(&self) -> u32 {
            self.volumes.len().saturating_sub(1) as u32
        }

        fn disk_offset(&self) -> u64 {
            self.volumes.last().map_or(0, |volume| volume.len() as u64)
        }

        fn ensure_record_capacity(&mut self, record_len: u64) -> Result<(), FormatError> {
            if record_len > self.size {
                return Err(FormatError::Unsupported("record is too large".into()));
            }
            self.ensure_started();
            if self.disk_offset() + record_len > self.size {
                self.volumes.push(Vec::new());
            }
            Ok(())
        }

        fn write_spanning(&mut self, mut bytes: &[u8]) -> Result<(), FormatError> {
            self.ensure_started();
            while !bytes.is_empty() {
                let remaining = (self.size - self.disk_offset()) as usize;
                if remaining == 0 {
                    self.volumes.push(Vec::new());
                    continue;
                }
                let count = remaining.min(bytes.len());
                if let Some(volume) = self.volumes.last_mut() {
                    volume.extend_from_slice(&bytes[..count]);
                }
                bytes = &bytes[count..];
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingProgress {
        events: Mutex<Vec<(u64, u64, String)>>,
    }

    impl ProgressSink for RecordingProgress {
        fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
            self.events
                .lock()
                .unwrap()
                .push((done, total, current.display.clone()));
        }
    }

    struct CancelOnOutput {
        ctl: Arc<ControlToken>,
    }

    impl ProgressSink for CancelOnOutput {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > 0 {
                self.ctl.cancel();
            }
        }
    }

    fn source_zip() -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut cursor);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            archive.start_file("first.bin", options).unwrap();
            archive.write_all(&vec![0x5a; 150_000]).unwrap();
            archive.start_file("second.txt", options).unwrap();
            archive.write_all(b"native volumes").unwrap();
            archive.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn record_stays_on_one_volume(volumes: &[Vec<u8>], magic: &[u8; 4], fixed: usize) -> bool {
        volumes.iter().all(|volume| {
            volume
                .windows(4)
                .enumerate()
                .filter(|(_, bytes)| *bytes == magic)
                .all(|(offset, _)| offset + fixed <= volume.len())
        })
    }

    #[test]
    fn native_writer_emits_pkware_names() {
        let destination = Path::new("/tmp/backup.zip");
        assert_eq!(
            volume_path(destination, 0, false).unwrap(),
            Path::new("/tmp/backup.z01")
        );
        assert_eq!(
            volume_path(destination, 99, false).unwrap(),
            Path::new("/tmp/backup.z100")
        );
        assert_eq!(
            volume_path(destination, 2, true).unwrap(),
            Path::new("/tmp/backup.zip")
        );
    }

    #[test]
    fn native_writer_keeps_headers_inside_volume_boundaries() {
        let mut source = Cursor::new(source_zip());
        let mut output = MemoryVolumes::new(64 * 1024);
        write_native_volumes(
            &mut source,
            &mut output,
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();

        assert!(output.volumes.len() >= 3);
        assert!(output.volumes[0].starts_with(&SPLIT_MAGIC));
        assert!(output
            .volumes
            .iter()
            .all(|volume| volume.len() as u64 <= output.size));
        assert!(record_stays_on_one_volume(
            &output.volumes,
            &LOCAL_MAGIC,
            LOCAL_FIXED_LEN
        ));
        assert!(record_stays_on_one_volume(
            &output.volumes,
            &CENTRAL_MAGIC,
            CENTRAL_FIXED_LEN
        ));

        let final_volume = output.volumes.last().unwrap();
        let eocd = final_volume
            .windows(4)
            .rposition(|bytes| bytes == EOCD_MAGIC)
            .unwrap();
        assert_eq!(
            read_u16(final_volume, eocd + 4, "disk").unwrap() as usize,
            output.volumes.len() - 1
        );
        assert!(
            final_volume[eocd - ZIP64_LOCATOR_LEN as usize..eocd].starts_with(&ZIP64_LOCATOR_MAGIC)
        );
    }

    #[test]
    fn native_writer_reports_exact_monotonic_output_bytes() {
        let mut source = Cursor::new(source_zip());
        let mut output = MemoryVolumes::new(64 * 1024);
        let progress = RecordingProgress::default();
        write_native_volumes(
            &mut source,
            &mut output,
            &progress,
            &ControlToken::default(),
        )
        .unwrap();

        let expected_total = output
            .volumes
            .iter()
            .map(|volume| volume.len() as u64)
            .sum::<u64>();
        let events = progress.events.lock().unwrap();
        assert_eq!(events.first(), Some(&(0, 0, String::new())));
        let output_events = events
            .iter()
            .filter(|(_, total, _)| *total > 0)
            .collect::<Vec<_>>();
        assert_eq!(
            output_events.first().map(|event| (event.0, event.1)),
            Some((0, expected_total))
        );
        assert_eq!(
            output_events.last().map(|event| (event.0, event.1)),
            Some((expected_total, expected_total))
        );
        assert!(output_events
            .windows(2)
            .all(|events| events[0].0 <= events[1].0));
        assert!(output_events
            .iter()
            .all(|(_, total, current)| *total == expected_total && current.is_empty()));
    }

    #[test]
    fn native_writer_honors_the_stream_buffer_limit() {
        let mut source = Cursor::new(source_zip());
        let mut output = MemoryVolumes::new(64 * 1024)
            .with_stream_buffer_limit(ResourceOptions::MIN_STREAM_BUFFER_BYTES);
        let progress = RecordingProgress::default();

        write_native_volumes(
            &mut source,
            &mut output,
            &progress,
            &ControlToken::default(),
        )
        .unwrap();

        assert!(!output.volumes.is_empty());
        let events = progress.events.lock().unwrap();
        let mut previous = 0;
        let mut saw_full_buffer = false;
        for (done, _, _) in events.iter().filter(|(_, total, _)| *total > 0) {
            let delta = done.saturating_sub(previous);
            assert!(delta <= ResourceOptions::MIN_STREAM_BUFFER_BYTES);
            saw_full_buffer |= delta == ResourceOptions::MIN_STREAM_BUFFER_BYTES;
            previous = *done;
        }
        assert!(saw_full_buffer);
    }

    #[test]
    fn native_writer_honors_cancellation_during_output() {
        let mut source = Cursor::new(source_zip());
        let mut output = MemoryVolumes::new(64 * 1024);
        let ctl = ControlToken::new();
        let progress = CancelOnOutput { ctl: ctl.clone() };

        let error = write_native_volumes(&mut source, &mut output, &progress, &ctl).unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert!(!output.volumes.is_empty());
    }
}
