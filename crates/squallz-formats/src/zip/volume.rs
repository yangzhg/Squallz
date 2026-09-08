use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use squallz_format_api::{
    ArchiveSourceSet, ControlToken, FormatError, PhysicalFileIdentity, ReadSeek,
};

use crate::stable_source::{self, BoundSourceSet, PrivateStagingDir, SourceIdentity};

const MAX_SPLIT_ZIP_VOLUME_COUNT: u64 = 1_000_000;
const EOCD_MIN_LEN: usize = 22;
const EOCD_MAX_COMMENT_LEN: usize = u16::MAX as usize;
const ZIP64_LOCATOR_LEN: u64 = 20;
const ZIP64_EOCD_MIN_LEN: usize = 56;
const ZIP64_EOCD_MIN_BODY_LEN: u64 = 44;
const ZIP64_EOCD_MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const ZIP64_LOCATOR_MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SplitZipMetadata {
    final_disk: u64,
    central_directory_disk: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Zip64Locator {
    final_disk: u64,
    record_disk: u64,
    record_offset: u64,
    locator_offset: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SplitZipEnd {
    Standard(SplitZipMetadata),
    Zip64 {
        locator: Zip64Locator,
        central_directory_disk: Option<u64>,
    },
}

impl SplitZipEnd {
    fn final_disk(self) -> u64 {
        match self {
            Self::Standard(metadata) => metadata.final_disk,
            Self::Zip64 { locator, .. } => locator.final_disk,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SplitZipName {
    Data { base: OsString, number: u64 },
    Final { base: OsString },
}

impl SplitZipName {
    fn base(&self) -> &OsStr {
        match self {
            Self::Data { base, .. } | Self::Final { base } => base,
        }
    }
}

#[derive(Debug)]
struct Candidate {
    index: u64,
    path: PathBuf,
    selected: bool,
    identity: SourceIdentity,
}

pub(super) struct DiscoveredSplitZipSet {
    metadata: SplitZipMetadata,
    candidates: BTreeMap<u64, Candidate>,
}

impl DiscoveredSplitZipSet {
    fn source_set(&self) -> Result<ArchiveSourceSet, FormatError> {
        let primary = self
            .candidates
            .get(&self.metadata.final_disk)
            .map(|candidate| candidate.path.clone())
            .ok_or_else(|| {
                FormatError::CorruptArchive("split ZIP set has no final .zip volume".into())
            })?;
        ArchiveSourceSet::from_primary_and_ordered_members(
            primary,
            self.candidates
                .values()
                .map(|candidate| candidate.path.clone())
                .collect(),
        )
    }
}

pub(super) enum BoundZipSource {
    Single(Box<dyn ReadSeek>),
    Split(DiscoveredSplitZipSet, Box<dyn ReadSeek>),
}

pub(super) struct StagedSplitZipSet {
    root: PrivateStagingDir,
    primary: PathBuf,
    metadata: SplitZipMetadata,
    source_set: BoundSourceSet,
}

impl StagedSplitZipSet {
    #[cfg(test)]
    pub(super) fn from_discovered(
        discovered: DiscoveredSplitZipSet,
        selected_src: Box<dyn ReadSeek>,
    ) -> Result<Self, FormatError> {
        Self::from_discovered_with_control(discovered, selected_src, &ControlToken::default())
    }

    pub(super) fn from_discovered_with_control(
        discovered: DiscoveredSplitZipSet,
        mut selected_src: Box<dyn ReadSeek>,
        control: &ControlToken,
    ) -> Result<Self, FormatError> {
        control.checkpoint()?;
        let source_set = discovered.source_set()?;
        let bindings = discovered
            .candidates
            .values()
            .map(|candidate| (candidate.path.clone(), candidate.identity.clone()))
            .collect();
        let source_set = BoundSourceSet::new(source_set, bindings)?;
        let root = stable_source::create_private_staging_dir("zip-volume")?;
        let primary = root.join("archive.zip");
        let staged = Self {
            root,
            primary,
            metadata: discovered.metadata,
            source_set,
        };

        let mut selected_staged = false;
        for candidate in discovered.candidates.values() {
            control.checkpoint()?;
            let staged_path = staged.root.join(canonical_staged_name(
                candidate.index,
                discovered.metadata.final_disk,
            )?);
            if candidate.selected {
                if selected_staged {
                    return Err(FormatError::CorruptArchive(
                        "split ZIP volume selection is ambiguous".into(),
                    ));
                }
                stable_source::copy_selected_stream(&mut *selected_src, &staged_path, control)?;
                stable_source::verify_source_binding(
                    &candidate.path,
                    &candidate.identity,
                    "ZIP volume",
                )?;
                selected_staged = true;
            } else {
                stable_source::copy_stable_source(
                    &candidate.path,
                    &candidate.identity,
                    &staged_path,
                    "ZIP volume",
                    control,
                )?;
            }
        }
        if !selected_staged {
            return Err(FormatError::CorruptArchive(
                "selected split ZIP volume was not staged".into(),
            ));
        }

        let mut staged_final =
            stable_source::open_regular_file_no_follow(&staged.primary, "staged ZIP volume")?;
        let staged_end = inspect_split_zip_end(&mut staged_final)?.ok_or_else(|| {
            FormatError::CorruptArchive(
                "staged split ZIP final volume has no multi-disk directory".into(),
            )
        })?;
        let staged_metadata = resolve_split_zip_end(staged_end, |index| {
            control.checkpoint()?;
            let path = staged.root.join(canonical_staged_name(
                index,
                discovered.metadata.final_disk,
            )?);
            stable_source::open_regular_file_no_follow(&path, "staged ZIP volume")
        })?;
        if staged_metadata != staged.metadata {
            return Err(FormatError::CorruptArchive(
                "split ZIP directory metadata changed while volumes were staged".into(),
            ));
        }
        for candidate in discovered.candidates.values() {
            control.checkpoint()?;
            stable_source::verify_source_binding(
                &candidate.path,
                &candidate.identity,
                "ZIP volume",
            )?;
        }
        control.checkpoint()?;
        Ok(staged)
    }

    pub(super) fn path(&self) -> &Path {
        &self.primary
    }

    pub(super) fn source_set(&self) -> &ArchiveSourceSet {
        self.source_set.source_set()
    }

    pub(super) fn verify_source_set(&self, control: &ControlToken) -> Result<(), FormatError> {
        self.source_set.verify_current("ZIP volume", control)
    }

    pub(super) fn remap_external_error(&self, error: FormatError) -> FormatError {
        if let Some(missing) = error.missing_volume_path() {
            if let Some(index) = missing
                .file_name()
                .and_then(|name| canonical_staged_index(name, self.metadata.final_disk))
            {
                if let Some(path) = self.source_set().members().get(index) {
                    return FormatError::missing_volume(path.clone());
                }
            }
        }
        self.redact_staging_error(error)
    }

    fn redact_staging_error(&self, error: FormatError) -> FormatError {
        let redact = |text: String| {
            text.replace(
                self.root.to_string_lossy().as_ref(),
                "[private ZIP staging]",
            )
        };
        match error {
            FormatError::Io(error) => {
                let kind = error.kind();
                FormatError::from(io::Error::new(kind, redact(error.to_string())))
            }
            FormatError::Unsupported(text) => FormatError::Unsupported(redact(text)),
            FormatError::CorruptArchive(_) => FormatError::CorruptArchive(
                "ZIP backend could not read the staged split archive".into(),
            ),
            FormatError::PathTraversal(text) => FormatError::PathTraversal(redact(text)),
            FormatError::SymlinkBreakout(text) => FormatError::SymlinkBreakout(redact(text)),
            FormatError::ResourceLimitExceeded(text) => {
                FormatError::ResourceLimitExceeded(redact(text))
            }
            FormatError::UnsafeFileName(text) => FormatError::UnsafeFileName(redact(text)),
            FormatError::DependencyMissing(text) => FormatError::DependencyMissing(redact(text)),
            FormatError::Other(_) => {
                FormatError::Other("ZIP backend failed while reading the staged archive".into())
            }
            other => other,
        }
    }
}

#[cfg(test)]
pub(super) fn bind_file(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: Box<dyn ReadSeek>,
) -> Result<BoundZipSource, FormatError> {
    bind_file_with_control(source_path, source_identity, src, &ControlToken::default())
}

pub(super) fn bind_file_with_control(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    mut src: Box<dyn ReadSeek>,
    control: &ControlToken,
) -> Result<BoundZipSource, FormatError> {
    match discover_bound_set(source_path, source_identity, &mut *src, control)? {
        Some(discovered) => Ok(BoundZipSource::Split(discovered, src)),
        None => Ok(BoundZipSource::Single(src)),
    }
}

pub(super) fn probe_bound_file(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: &mut dyn ReadSeek,
) -> Result<Option<ArchiveSourceSet>, FormatError> {
    probe_bound_file_with_control(source_path, source_identity, src, &ControlToken::default())
}

pub(super) fn probe_bound_file_with_control(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: &mut dyn ReadSeek,
    control: &ControlToken,
) -> Result<Option<ArchiveSourceSet>, FormatError> {
    discover_bound_set(source_path, source_identity, src, control)?
        .map(|discovered| discovered.source_set())
        .transpose()
}

fn discover_bound_set(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: &mut dyn ReadSeek,
    control: &ControlToken,
) -> Result<Option<DiscoveredSplitZipSet>, FormatError> {
    control.checkpoint()?;
    let Some(source_name) = source_path.file_name().and_then(parse_split_zip_name) else {
        return Ok(None);
    };
    let selected_end = if matches!(source_name, SplitZipName::Final { .. }) {
        let selected_end = inspect_split_zip_end(src)?;
        control.checkpoint()?;
        match selected_end {
            Some(end) => Some(end),
            None => return Ok(None),
        }
    } else {
        None
    };
    let expected_identity = source_identity.ok_or_else(|| {
        FormatError::CorruptArchive(
            "native split ZIP discovery requires an opened-file identity".into(),
        )
    })?;
    let (selected_path, selected_identity) = stable_source::resolve_selected_regular_path(
        source_path,
        expected_identity,
        "ZIP volume",
        control,
    )?;
    let selected_name = selected_path
        .file_name()
        .and_then(parse_split_zip_name)
        .ok_or_else(|| FormatError::CorruptArchive("selected ZIP volume name is invalid".into()))?;
    if std::mem::discriminant(&selected_name) != std::mem::discriminant(&source_name) {
        return Err(FormatError::CorruptArchive(
            "selected ZIP volume name changed after it was opened".into(),
        ));
    }

    discover_directory_set(
        selected_path,
        selected_identity,
        selected_name,
        selected_end,
        control,
    )
    .map(Some)
}

fn discover_directory_set(
    selected_path: PathBuf,
    selected_identity: SourceIdentity,
    selected_name: SplitZipName,
    selected_end: Option<SplitZipEnd>,
    control: &ControlToken,
) -> Result<DiscoveredSplitZipSet, FormatError> {
    let source_parent = stable_source::parent_or_current(&selected_path).to_path_buf();
    let base = selected_name.base().to_os_string();
    let mut data_candidates = BTreeMap::<u64, Candidate>::new();
    let mut final_candidate = None;
    let mut final_end = selected_end;
    let mut selected_seen = false;

    for entry in fs::read_dir(&source_parent)? {
        control.checkpoint()?;
        let entry = entry?;
        let Some(parsed) = parse_split_zip_name(&entry.file_name()) else {
            continue;
        };
        if parsed.base() != base {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if !stable_source::is_regular_source_metadata(&metadata) {
            return Err(FormatError::CorruptArchive(
                "a split ZIP member is not a regular file".into(),
            ));
        }
        let selected = path == selected_path;
        let identity = if selected {
            selected_seen = true;
            stable_source::verify_source_binding(&path, &selected_identity, "ZIP volume")?;
            selected_identity.clone()
        } else {
            let file = stable_source::open_regular_file_no_follow(&path, "ZIP volume")?;
            let identity = SourceIdentity::from_file(&file)?;
            stable_source::verify_source_binding(&path, &identity, "ZIP volume")?;
            identity
        };

        match parsed {
            SplitZipName::Data { number, .. } => {
                ensure_volume_number(number)?;
                if data_candidates
                    .insert(
                        number,
                        Candidate {
                            index: number - 1,
                            path,
                            selected,
                            identity,
                        },
                    )
                    .is_some()
                {
                    return Err(FormatError::CorruptArchive(format!(
                        "split ZIP data volume {number} appears more than once"
                    )));
                }
            }
            SplitZipName::Final { .. } => {
                if final_candidate.is_some() {
                    return Err(FormatError::CorruptArchive(
                        "split ZIP set has more than one final .zip volume".into(),
                    ));
                }
                if !selected {
                    final_end = Some(inspect_bound_final(&path, &identity, control)?);
                }
                final_candidate = Some(Candidate {
                    index: 0,
                    path,
                    selected,
                    identity,
                });
            }
        }
    }
    control.checkpoint()?;
    if !selected_seen {
        return Err(FormatError::CorruptArchive(
            "selected ZIP volume changed before its sibling set was discovered".into(),
        ));
    }
    let final_candidate = final_candidate
        .ok_or_else(|| FormatError::missing_volume(source_parent.join(final_source_name(&base))))?;
    let final_end = final_end.ok_or_else(|| {
        FormatError::CorruptArchive("split ZIP final volume has no valid directory".into())
    })?;
    let final_disk = final_end.final_disk();
    ensure_final_disk_bounds(final_disk)?;

    if let Some(extra) = data_candidates
        .keys()
        .copied()
        .find(|number| *number > final_disk)
    {
        return Err(FormatError::CorruptArchive(format!(
            "split ZIP data volume {extra} appears after the final disk"
        )));
    }
    for number in 1..=final_disk {
        control.checkpoint()?;
        if !data_candidates.contains_key(&number) {
            return Err(FormatError::missing_volume(
                source_parent.join(data_source_name(&base, number)),
            ));
        }
    }

    let mut candidates = BTreeMap::new();
    for candidate in data_candidates.into_values() {
        candidates.insert(candidate.index, candidate);
    }
    let mut final_candidate = final_candidate;
    final_candidate.index = final_disk;
    if candidates.insert(final_disk, final_candidate).is_some() {
        return Err(FormatError::CorruptArchive(
            "split ZIP final disk conflicts with a data volume".into(),
        ));
    }
    for candidate in candidates.values() {
        control.checkpoint()?;
        stable_source::verify_source_binding(&candidate.path, &candidate.identity, "ZIP volume")?;
    }
    let metadata = resolve_split_zip_end(final_end, |index| {
        control.checkpoint()?;
        let candidate = candidates.get(&index).ok_or_else(|| {
            FormatError::CorruptArchive(
                "split ZIP64 end record refers to an unavailable volume".into(),
            )
        })?;
        open_bound_candidate(candidate)
    })?;
    ensure_metadata_bounds(metadata)?;
    for candidate in candidates.values() {
        control.checkpoint()?;
        stable_source::verify_source_binding(&candidate.path, &candidate.identity, "ZIP volume")?;
    }
    control.checkpoint()?;

    Ok(DiscoveredSplitZipSet {
        metadata,
        candidates,
    })
}

fn inspect_bound_final(
    path: &Path,
    expected: &SourceIdentity,
    control: &ControlToken,
) -> Result<SplitZipEnd, FormatError> {
    control.checkpoint()?;
    let mut file = stable_source::open_regular_file_no_follow(path, "ZIP volume")?;
    let initial = SourceIdentity::from_file(&file)?;
    if &initial != expected {
        return Err(FormatError::CorruptArchive(
            "split ZIP final volume changed before its directory was inspected".into(),
        ));
    }
    stable_source::verify_source_binding(path, &initial, "ZIP volume")?;
    let result = inspect_split_zip_end(&mut file)?.ok_or_else(|| {
        FormatError::CorruptArchive("split ZIP final volume has no multi-disk directory".into())
    });
    control.checkpoint()?;
    let final_identity = SourceIdentity::from_file(&file)?;
    if final_identity != initial {
        return Err(FormatError::CorruptArchive(
            "split ZIP final volume changed while its directory was inspected".into(),
        ));
    }
    stable_source::verify_source_binding(path, expected, "ZIP volume")?;
    control.checkpoint()?;
    result
}

fn open_bound_candidate(candidate: &Candidate) -> Result<File, FormatError> {
    let file = stable_source::open_regular_file_no_follow(&candidate.path, "ZIP volume")?;
    let identity = SourceIdentity::from_file(&file)?;
    if identity != candidate.identity {
        return Err(FormatError::CorruptArchive(
            "a split ZIP volume changed before its directory metadata was read".into(),
        ));
    }
    stable_source::verify_source_binding(&candidate.path, &identity, "ZIP volume")?;
    Ok(file)
}

fn inspect_split_zip_end(src: &mut dyn ReadSeek) -> Result<Option<SplitZipEnd>, FormatError> {
    let original = src.stream_position()?;
    let result = inspect_split_zip_end_inner(src);
    let rewind = src.seek(SeekFrom::Start(original));
    match (result, rewind) {
        (Ok(value), Ok(_)) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(FormatError::from(error)),
    }
}

fn inspect_split_zip_end_inner(src: &mut dyn ReadSeek) -> Result<Option<SplitZipEnd>, FormatError> {
    let file_len = src.seek(SeekFrom::End(0))?;
    let Some(eocd) = find_eocd(src, file_len)? else {
        return Ok(None);
    };
    let disk_16 = le_u16(&eocd.bytes[4..6]) as u64;
    let directory_disk_16 = le_u16(&eocd.bytes[6..8]) as u64;
    if disk_16 == 0 && directory_disk_16 == 0 {
        return Ok(None);
    }

    if disk_16 != u16::MAX as u64 && directory_disk_16 != u16::MAX as u64 {
        let metadata = SplitZipMetadata {
            final_disk: disk_16,
            central_directory_disk: directory_disk_16,
        };
        ensure_metadata_bounds(metadata)?;
        return Ok(Some(SplitZipEnd::Standard(metadata)));
    }

    let locator = read_zip64_locator(src, eocd.offset)?;
    if disk_16 != u16::MAX as u64 && disk_16 != locator.final_disk {
        return Err(FormatError::CorruptArchive(
            "split ZIP disk number disagrees with its ZIP64 end locator".into(),
        ));
    }
    let central_directory_disk =
        (directory_disk_16 != u16::MAX as u64).then_some(directory_disk_16);
    if let Some(central_directory_disk) = central_directory_disk {
        ensure_metadata_bounds(SplitZipMetadata {
            final_disk: locator.final_disk,
            central_directory_disk,
        })?;
    }
    Ok(Some(SplitZipEnd::Zip64 {
        locator,
        central_directory_disk,
    }))
}

struct EocdRecord {
    offset: u64,
    bytes: [u8; EOCD_MIN_LEN],
}

fn find_eocd(src: &mut dyn ReadSeek, file_len: u64) -> Result<Option<EocdRecord>, FormatError> {
    if file_len < EOCD_MIN_LEN as u64 {
        return Ok(None);
    }
    let tail_len = file_len.min((EOCD_MIN_LEN + EOCD_MAX_COMMENT_LEN) as u64) as usize;
    let tail_start = file_len - tail_len as u64;
    let mut tail = vec![0u8; tail_len];
    src.seek(SeekFrom::Start(tail_start))?;
    src.read_exact(&mut tail)?;

    for offset in (0..=tail_len - EOCD_MIN_LEN).rev() {
        if tail[offset..offset + 4] != super::EOCD_MAGIC {
            continue;
        }
        let comment_len = le_u16(&tail[offset + 20..offset + 22]) as usize;
        if offset + EOCD_MIN_LEN + comment_len != tail_len {
            continue;
        }
        let mut bytes = [0u8; EOCD_MIN_LEN];
        bytes.copy_from_slice(&tail[offset..offset + EOCD_MIN_LEN]);
        return Ok(Some(EocdRecord {
            offset: tail_start + offset as u64,
            bytes,
        }));
    }
    Ok(None)
}

fn read_zip64_locator(
    src: &mut dyn ReadSeek,
    eocd_offset: u64,
) -> Result<Zip64Locator, FormatError> {
    let locator_offset = eocd_offset
        .checked_sub(ZIP64_LOCATOR_LEN)
        .ok_or_else(|| FormatError::CorruptArchive("split ZIP64 end locator is missing".into()))?;
    let mut locator = [0u8; ZIP64_LOCATOR_LEN as usize];
    src.seek(SeekFrom::Start(locator_offset))?;
    read_archive_exact(src, &mut locator, "split ZIP64 end locator")?;
    if locator[..4] != ZIP64_LOCATOR_MAGIC {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 end locator signature is invalid".into(),
        ));
    }
    let record_disk = le_u32(&locator[4..8]) as u64;
    let record_offset = le_u64(&locator[8..16]);
    let total_disks = le_u32(&locator[16..20]) as u64;
    if total_disks == 0 {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 locator reports no disks".into(),
        ));
    }
    let final_disk = total_disks - 1;
    ensure_final_disk_bounds(final_disk)?;
    if record_disk > final_disk {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 end record starts after the final disk".into(),
        ));
    }
    Ok(Zip64Locator {
        final_disk,
        record_disk,
        record_offset,
        locator_offset,
    })
}

fn resolve_split_zip_end(
    end: SplitZipEnd,
    open_volume: impl FnMut(u64) -> Result<File, FormatError>,
) -> Result<SplitZipMetadata, FormatError> {
    match end {
        SplitZipEnd::Standard(metadata) => Ok(metadata),
        SplitZipEnd::Zip64 {
            locator,
            central_directory_disk,
        } => read_zip64_record_from_volumes(locator, central_directory_disk, open_volume),
    }
}

fn read_zip64_record_from_volumes(
    locator: Zip64Locator,
    central_directory_disk: Option<u64>,
    mut open_volume: impl FnMut(u64) -> Result<File, FormatError>,
) -> Result<SplitZipMetadata, FormatError> {
    let mut fixed = [0u8; ZIP64_EOCD_MIN_LEN];
    let mut fixed_len = 0usize;
    let mut available = 0u64;
    let mut parsed = None;

    for disk in locator.record_disk..=locator.final_disk {
        let mut file = open_volume(disk)?;
        let initial_identity = SourceIdentity::from_file(&file)?;
        let file_len = file.seek(SeekFrom::End(0))?;
        let start = if disk == locator.record_disk {
            locator.record_offset
        } else {
            0
        };
        let end = if disk == locator.final_disk {
            locator.locator_offset
        } else {
            file_len
        };
        if end > file_len {
            return Err(FormatError::CorruptArchive(
                "split ZIP64 end locator points outside the final volume".into(),
            ));
        }
        if start > end {
            return Err(FormatError::CorruptArchive(
                "split ZIP64 end record offset is outside its start volume".into(),
            ));
        }

        let span = end - start;
        available = available.checked_add(span).ok_or_else(|| {
            FormatError::CorruptArchive("split ZIP64 end record length overflows".into())
        })?;
        if fixed_len < fixed.len() && span != 0 {
            let remaining = fixed.len() - fixed_len;
            let read_len = usize::try_from(span.min(remaining as u64)).map_err(|_| {
                FormatError::ResourceLimitExceeded(
                    "split ZIP64 end record read is too large".into(),
                )
            })?;
            file.seek(SeekFrom::Start(start))?;
            read_archive_exact(
                &mut file,
                &mut fixed[fixed_len..fixed_len + read_len],
                "split ZIP64 end record",
            )?;
            fixed_len += read_len;
        }

        let final_identity = SourceIdentity::from_file(&file)?;
        if final_identity != initial_identity {
            return Err(FormatError::CorruptArchive(
                "a split ZIP volume changed while its directory metadata was read".into(),
            ));
        }

        if fixed_len == fixed.len() && parsed.is_none() {
            parsed = Some(parse_zip64_record(&fixed, locator, central_directory_disk)?);
        }
        if let Some((metadata, declared_len)) = parsed {
            if available >= declared_len {
                return Ok(metadata);
            }
        }
    }

    if fixed_len < fixed.len() {
        Err(FormatError::CorruptArchive(
            "split ZIP64 end record is truncated across its volume set".into(),
        ))
    } else {
        Err(FormatError::CorruptArchive(
            "split ZIP64 end record length exceeds the bytes before its locator".into(),
        ))
    }
}

fn parse_zip64_record(
    record: &[u8; ZIP64_EOCD_MIN_LEN],
    locator: Zip64Locator,
    central_directory_disk: Option<u64>,
) -> Result<(SplitZipMetadata, u64), FormatError> {
    if record[..4] != ZIP64_EOCD_MAGIC {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 end record signature is invalid".into(),
        ));
    }
    let record_body_len = le_u64(&record[4..12]);
    if record_body_len < ZIP64_EOCD_MIN_BODY_LEN {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 end record is too short".into(),
        ));
    }
    let declared_len = record_body_len.checked_add(12).ok_or_else(|| {
        FormatError::CorruptArchive("split ZIP64 end record length overflows".into())
    })?;
    let record_final_disk = le_u32(&record[16..20]) as u64;
    if record_final_disk != locator.final_disk {
        return Err(FormatError::CorruptArchive(
            "split ZIP64 disk count disagrees with its end locator".into(),
        ));
    }
    let metadata = SplitZipMetadata {
        final_disk: locator.final_disk,
        central_directory_disk: central_directory_disk
            .unwrap_or_else(|| le_u32(&record[20..24]) as u64),
    };
    ensure_metadata_bounds(metadata)?;
    Ok((metadata, declared_len))
}

fn read_archive_exact(
    src: &mut dyn Read,
    buffer: &mut [u8],
    field: &str,
) -> Result<(), FormatError> {
    match src.read_exact(buffer) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            Err(FormatError::CorruptArchive(format!("{field} is truncated")))
        }
        Err(error) => Err(FormatError::from(error)),
    }
}

fn ensure_metadata_bounds(metadata: SplitZipMetadata) -> Result<(), FormatError> {
    ensure_final_disk_bounds(metadata.final_disk)?;
    if metadata.central_directory_disk > metadata.final_disk {
        return Err(FormatError::CorruptArchive(
            "split ZIP central directory starts after the final disk".into(),
        ));
    }
    Ok(())
}

fn ensure_final_disk_bounds(final_disk: u64) -> Result<(), FormatError> {
    if final_disk == 0 {
        return Err(FormatError::CorruptArchive(
            "split ZIP metadata has no preceding data volume".into(),
        ));
    }
    if final_disk >= MAX_SPLIT_ZIP_VOLUME_COUNT {
        return Err(volume_limit_error());
    }
    Ok(())
}

fn ensure_volume_number(number: u64) -> Result<(), FormatError> {
    if number == 0 || number >= MAX_SPLIT_ZIP_VOLUME_COUNT {
        Err(volume_limit_error())
    } else {
        Ok(())
    }
}

fn volume_limit_error() -> FormatError {
    FormatError::ResourceLimitExceeded(format!(
        "split ZIP volume count exceeds {MAX_SPLIT_ZIP_VOLUME_COUNT}"
    ))
}

fn parse_split_zip_name(file_name: &OsStr) -> Option<SplitZipName> {
    let path = Path::new(file_name);
    let extension = path.extension()?.to_str()?;
    let base = path.file_stem()?.to_os_string();
    if base.is_empty() {
        return None;
    }
    if extension.eq_ignore_ascii_case("zip") {
        return Some(SplitZipName::Final { base });
    }
    let bytes = extension.as_bytes();
    if bytes.len() < 3
        || !bytes[0].eq_ignore_ascii_case(&b'z')
        || !bytes[1..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let number = extension[1..].parse().ok()?;
    if number == 0 {
        return None;
    }
    Some(SplitZipName::Data { base, number })
}

fn data_source_name(base: &OsStr, number: u64) -> OsString {
    let mut name = base.to_os_string();
    name.push(".z");
    name.push(format_volume_number(number));
    name
}

fn final_source_name(base: &OsStr) -> OsString {
    let mut name = base.to_os_string();
    name.push(".zip");
    name
}

fn canonical_staged_name(index: u64, final_disk: u64) -> Result<OsString, FormatError> {
    if index == final_disk {
        return Ok(OsString::from("archive.zip"));
    }
    let number = index.checked_add(1).ok_or_else(volume_limit_error)?;
    ensure_volume_number(number)?;
    Ok(OsString::from(format!(
        "archive.z{}",
        format_volume_number(number)
    )))
}

fn canonical_staged_index(file_name: &OsStr, final_disk: u64) -> Option<usize> {
    match parse_split_zip_name(file_name)? {
        SplitZipName::Final { base } if base == "archive" => usize::try_from(final_disk).ok(),
        SplitZipName::Data { base, number } if base == "archive" => {
            usize::try_from(number.checked_sub(1)?).ok()
        }
        _ => None,
    }
}

fn format_volume_number(number: u64) -> String {
    if number < 100 {
        format!("{number:02}")
    } else {
        number.to_string()
    }
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[cfg(test)]
pub(super) fn test_split_final_volume(final_disk: u16) -> Vec<u8> {
    let mut eocd = vec![0u8; EOCD_MIN_LEN];
    eocd[..4].copy_from_slice(&super::EOCD_MAGIC);
    eocd[4..6].copy_from_slice(&final_disk.to_le_bytes());
    eocd[6..8].copy_from_slice(&final_disk.to_le_bytes());
    eocd
}

#[cfg(test)]
pub(super) fn test_physical_identity(path: &Path) -> Result<PhysicalFileIdentity, FormatError> {
    let file = stable_source::open_regular_file_no_follow(path, "ZIP volume")?;
    Ok(SourceIdentity::from_file(&file)?.physical_identity())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "squallz-split-zip-{tag}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn split_final(final_disk: u16) -> Vec<u8> {
        test_split_final_volume(final_disk)
    }

    fn zip64_record(final_disk: u32, central_directory_disk: u32, body_len: u64) -> Vec<u8> {
        let mut record = vec![0u8; ZIP64_EOCD_MIN_LEN];
        record[..4].copy_from_slice(&ZIP64_EOCD_MAGIC);
        record[4..12].copy_from_slice(&body_len.to_le_bytes());
        record[16..20].copy_from_slice(&final_disk.to_le_bytes());
        record[20..24].copy_from_slice(&central_directory_disk.to_le_bytes());
        record
    }

    fn zip64_final_volume(
        record_disk: u32,
        record_offset: u64,
        total_disks: u32,
        bytes_before_locator: &[u8],
    ) -> Vec<u8> {
        let mut bytes = bytes_before_locator.to_vec();
        bytes.extend_from_slice(&ZIP64_LOCATOR_MAGIC);
        bytes.extend_from_slice(&record_disk.to_le_bytes());
        bytes.extend_from_slice(&record_offset.to_le_bytes());
        bytes.extend_from_slice(&total_disks.to_le_bytes());
        let mut eocd = split_final(u16::MAX);
        eocd[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        bytes.extend_from_slice(&eocd);
        bytes
    }

    #[test]
    fn native_names_use_zip_disk_numbering() {
        assert_eq!(
            parse_split_zip_name(OsStr::new("archive.z01")),
            Some(SplitZipName::Data {
                base: OsString::from("archive"),
                number: 1,
            })
        );
        assert_eq!(
            parse_split_zip_name(OsStr::new("archive.Z100")),
            Some(SplitZipName::Data {
                base: OsString::from("archive"),
                number: 100,
            })
        );
        assert_eq!(
            parse_split_zip_name(OsStr::new("archive.ZIP")),
            Some(SplitZipName::Final {
                base: OsString::from("archive"),
            })
        );
        assert_eq!(parse_split_zip_name(OsStr::new("archive.z1")), None);
        assert_eq!(parse_split_zip_name(OsStr::new("archive.z00")), None);
    }

    #[test]
    fn split_directory_is_discovered_from_any_data_volume() {
        let dir = temp_dir("discover");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        fs::write(&final_path, split_final(2)).unwrap();

        let identity = test_physical_identity(&second).unwrap();
        let mut selected = Cursor::new(b"second".to_vec());
        let source_set = probe_bound_file(&second, Some(identity), &mut selected)
            .unwrap()
            .unwrap();

        assert_eq!(source_set.primary(), final_path);
        assert_eq!(source_set.members(), &[first, second, final_path]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ordinary_zip_with_similar_siblings_stays_single_file() {
        let dir = temp_dir("ordinary");
        let data = dir.join("sample.z01");
        let final_path = dir.join("sample.zip");
        fs::write(&data, b"unrelated").unwrap();
        fs::write(&final_path, split_final(0)).unwrap();

        let identity = test_physical_identity(&final_path).unwrap();
        let mut selected = Cursor::new(split_final(0));
        assert!(probe_bound_file(&final_path, Some(identity), &mut selected)
            .unwrap()
            .is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_middle_volume_reports_the_original_expected_name() {
        let dir = temp_dir("missing");
        let first = dir.join("sample.z01");
        let final_path = dir.join("sample.zip");
        fs::write(&first, b"first").unwrap();
        fs::write(&final_path, split_final(2)).unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        let error = probe_bound_file(&first, Some(identity), &mut selected).unwrap_err();
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("sample.z02").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn data_volume_without_final_zip_reports_the_expected_primary() {
        let dir = temp_dir("missing-final");
        let first = dir.join("sample.z01");
        fs::write(&first, b"first").unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        let error = probe_bound_file(&first, Some(identity), &mut selected).unwrap_err();
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("sample.zip").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extra_data_volume_after_final_disk_is_rejected() {
        let dir = temp_dir("extra");
        let first = dir.join("sample.z01");
        let extra = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        fs::write(&first, b"first").unwrap();
        fs::write(&extra, b"extra").unwrap();
        fs::write(&final_path, split_final(1)).unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        assert!(matches!(
            probe_bound_file(&first, Some(identity), &mut selected),
            Err(FormatError::CorruptArchive(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zip64_end_record_in_the_final_volume_is_discovered() {
        let dir = temp_dir("zip64-final");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        let record = zip64_record(2, 2, ZIP64_EOCD_MIN_BODY_LEN);
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        fs::write(&final_path, zip64_final_volume(2, 0, 3, &record)).unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        let source_set = probe_bound_file(&first, Some(identity), &mut selected)
            .unwrap()
            .unwrap();

        assert_eq!(source_set.primary(), final_path);
        assert_eq!(source_set.members(), &[first, second, final_path]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zip64_end_record_on_an_earlier_volume_is_discovered() {
        let dir = temp_dir("zip64-earlier");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        let record = zip64_record(2, 1, ZIP64_EOCD_MIN_BODY_LEN);
        fs::write(&first, b"first").unwrap();
        let mut second_bytes = b"head".to_vec();
        second_bytes.extend_from_slice(&record);
        fs::write(&second, second_bytes).unwrap();
        fs::write(&final_path, zip64_final_volume(1, 4, 3, &[])).unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        let source_set = probe_bound_file(&first, Some(identity), &mut selected)
            .unwrap()
            .unwrap();

        assert_eq!(source_set.primary(), final_path);
        assert_eq!(source_set.members(), &[first, second, final_path]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zip64_cross_volume_stage_binds_the_original_members() {
        let dir = temp_dir("zip64-cross");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        let record = zip64_record(2, 1, ZIP64_EOCD_MIN_BODY_LEN);
        let split_at = 24;
        fs::write(&first, b"first").unwrap();
        let mut second_bytes = b"head".to_vec();
        second_bytes.extend_from_slice(&record[..split_at]);
        fs::write(&second, &second_bytes).unwrap();
        fs::write(
            &final_path,
            zip64_final_volume(1, 4, 3, &record[split_at..]),
        )
        .unwrap();

        let identity = test_physical_identity(&second).unwrap();
        let staged = match bind_file(&second, Some(identity), Box::new(Cursor::new(second_bytes)))
            .unwrap()
        {
            BoundZipSource::Split(discovered, selected) => {
                StagedSplitZipSet::from_discovered(discovered, selected).unwrap()
            }
            BoundZipSource::Single(_) => panic!("split ZIP must be staged as a set"),
        };

        assert_eq!(staged.source_set().primary(), final_path);
        assert_eq!(
            staged.source_set().members(),
            &[first.clone(), second, final_path]
        );
        assert_eq!(staged.path(), staged.root.join("archive.zip"));
        staged.verify_source_set(&ControlToken::default()).unwrap();
        fs::remove_file(&first).unwrap();
        fs::write(&first, b"first").unwrap();
        assert!(staged
            .verify_source_set(&ControlToken::default())
            .unwrap_err()
            .is_input_changed());
        drop(staged);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zip64_end_record_declared_past_its_locator_is_rejected() {
        let dir = temp_dir("zip64-overrun");
        let first = dir.join("sample.z01");
        let second = dir.join("sample.z02");
        let final_path = dir.join("sample.zip");
        let record = zip64_record(2, 1, ZIP64_EOCD_MIN_BODY_LEN + 1);
        let split_at = 24;
        fs::write(&first, b"first").unwrap();
        let mut second_bytes = b"head".to_vec();
        second_bytes.extend_from_slice(&record[..split_at]);
        fs::write(&second, second_bytes).unwrap();
        fs::write(
            &final_path,
            zip64_final_volume(1, 4, 3, &record[split_at..]),
        )
        .unwrap();

        let identity = test_physical_identity(&first).unwrap();
        let mut selected = Cursor::new(b"first".to_vec());
        assert!(matches!(
            probe_bound_file(&first, Some(identity), &mut selected),
            Err(FormatError::CorruptArchive(_))
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn eocd_signature_inside_the_comment_does_not_hide_the_real_record() {
        let mut bytes = split_final(1);
        let comment = b"note PK\x05\x06";
        bytes[20..22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);

        assert_eq!(
            inspect_split_zip_end(&mut Cursor::new(bytes)).unwrap(),
            Some(SplitZipEnd::Standard(SplitZipMetadata {
                final_disk: 1,
                central_directory_disk: 1,
            }))
        );
    }

    #[cfg(unix)]
    #[test]
    fn staging_uses_private_permissions_and_final_zip_primary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("permissions");
        let first = dir.join("sample.z01");
        let final_path = dir.join("sample.zip");
        fs::write(&first, b"first").unwrap();
        fs::write(&final_path, split_final(1)).unwrap();
        let identity = test_physical_identity(&first).unwrap();

        let staged = match bind_file(
            &first,
            Some(identity),
            Box::new(Cursor::new(b"first".to_vec())),
        )
        .unwrap()
        {
            BoundZipSource::Split(discovered, selected) => {
                StagedSplitZipSet::from_discovered(discovered, selected).unwrap()
            }
            BoundZipSource::Single(_) => panic!("split ZIP must be staged as a set"),
        };
        assert_eq!(
            fs::metadata(&staged.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(staged.root.join("archive.z01"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(staged.path(), staged.root.join("archive.zip"));
        drop(staged);
        fs::remove_dir_all(dir).unwrap();
    }
}
