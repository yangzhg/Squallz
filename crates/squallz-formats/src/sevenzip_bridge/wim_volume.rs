use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, SeekFrom};
use std::path::{Path, PathBuf};

use squallz_format_api::{
    ArchiveSourceSet, ControlToken, FormatError, PhysicalFileIdentity, ReadSeek,
};

use crate::stable_source::{self, BoundSourceSet, PrivateStagingDir, SourceIdentity};

const WIM_HEADER_PREFIX_SIZE: usize = 44;
const WIM_HEADER_MIN_SIZE: u32 = 208;
const WIM_SPANNED_FLAG: u32 = 0x0000_0008;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WimHeader {
    header_size: u32,
    version: u32,
    flags: u32,
    chunk_size: u32,
    guid: [u8; 16],
    part_number: u16,
    total_parts: u16,
}

impl WimHeader {
    fn is_split(self) -> bool {
        self.flags & WIM_SPANNED_FLAG != 0 || self.part_number > 1 || self.total_parts > 1
    }

    fn validate_split(self) -> Result<Self, FormatError> {
        if self.total_parts < 2
            || self.part_number == 0
            || self.part_number > self.total_parts
            || self.guid.iter().all(|byte| *byte == 0)
        {
            return Err(FormatError::CorruptArchive(
                "Split WIM header has inconsistent part metadata".into(),
            ));
        }
        Ok(self)
    }

    fn belongs_to(self, expected: Self) -> bool {
        self.header_size == expected.header_size
            && self.version == expected.version
            && self.flags == expected.flags
            && self.chunk_size == expected.chunk_size
            && self.guid == expected.guid
            && self.total_parts == expected.total_parts
    }
}

#[derive(Debug)]
struct Candidate {
    path: PathBuf,
    selected: bool,
    identity: SourceIdentity,
    header: WimHeader,
}

pub(super) struct DiscoveredSplitWimSet {
    candidates: BTreeMap<u16, Candidate>,
}

impl DiscoveredSplitWimSet {
    fn source_set(&self) -> Result<ArchiveSourceSet, FormatError> {
        ArchiveSourceSet::from_ordered_members(
            self.candidates
                .values()
                .map(|candidate| candidate.path.clone())
                .collect(),
        )
    }
}

pub(super) enum BoundWimSource {
    Single(Box<dyn ReadSeek>),
    Split(DiscoveredSplitWimSet, Box<dyn ReadSeek>),
}

pub(super) struct StagedSplitWimSet {
    root: PrivateStagingDir,
    primary: PathBuf,
    source_set: BoundSourceSet,
}

pub(super) struct GeneratedWimPart {
    pub(super) path: PathBuf,
    pub(super) identity: SourceIdentity,
    pub(super) len: u64,
}

impl StagedSplitWimSet {
    #[cfg(test)]
    pub(super) fn from_discovered(
        discovered: DiscoveredSplitWimSet,
        selected_src: Box<dyn ReadSeek>,
    ) -> Result<Self, FormatError> {
        Self::from_discovered_with_control(discovered, selected_src, &ControlToken::default())
    }

    pub(super) fn from_discovered_with_control(
        discovered: DiscoveredSplitWimSet,
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
        let root = stable_source::create_private_staging_dir("wim-volume")?;
        let primary = root.join("archive.swm");
        let staged = Self {
            root,
            primary,
            source_set,
        };

        let mut selected_staged = false;
        for candidate in discovered.candidates.values() {
            control.checkpoint()?;
            let staged_path = staged.root.join(staged_name(candidate.header.part_number));
            if candidate.selected {
                if selected_staged {
                    return Err(FormatError::CorruptArchive(
                        "Split WIM volume selection is ambiguous".into(),
                    ));
                }
                stable_source::copy_selected_stream(&mut *selected_src, &staged_path, control)?;
                stable_source::verify_source_binding(
                    &candidate.path,
                    &candidate.identity,
                    "WIM volume",
                )?;
                selected_staged = true;
            } else {
                stable_source::copy_stable_source(
                    &candidate.path,
                    &candidate.identity,
                    &staged_path,
                    "WIM volume",
                    control,
                )?;
            }
        }
        if !selected_staged {
            return Err(FormatError::CorruptArchive(
                "selected Split WIM volume was not staged".into(),
            ));
        }

        let expected = discovered
            .candidates
            .get(&1)
            .map(|candidate| candidate.header)
            .ok_or_else(|| FormatError::CorruptArchive("Split WIM has no first part".into()))?;
        for candidate in discovered.candidates.values() {
            control.checkpoint()?;
            let staged_path = staged.root.join(staged_name(candidate.header.part_number));
            let mut file =
                stable_source::open_regular_file_no_follow(&staged_path, "staged WIM volume")?;
            let header = inspect_wim_header(&mut file)?
                .ok_or_else(|| FormatError::CorruptArchive("staged WIM header is missing".into()))?
                .validate_split()?;
            if !header.belongs_to(expected) || header.part_number != candidate.header.part_number {
                return Err(FormatError::CorruptArchive(
                    "Split WIM metadata changed while volumes were staged".into(),
                ));
            }
            stable_source::verify_source_binding(
                &candidate.path,
                &candidate.identity,
                "WIM volume",
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
        self.source_set.verify_current("WIM volume", control)
    }

    pub(super) fn remap_external_error(&self, error: FormatError) -> FormatError {
        let staging = self.root.to_string_lossy();
        let redact = |text: String| text.replace(staging.as_ref(), "[private WIM staging]");
        match error {
            FormatError::Io(error) => {
                FormatError::from(io::Error::new(error.kind(), redact(error.to_string())))
            }
            FormatError::Unsupported(text) => FormatError::Unsupported(redact(text)),
            FormatError::CorruptArchive(_) => FormatError::CorruptArchive(
                "7-Zip could not read the validated Split WIM set".into(),
            ),
            FormatError::PathTraversal(text) => FormatError::PathTraversal(redact(text)),
            FormatError::SymlinkBreakout(text) => FormatError::SymlinkBreakout(redact(text)),
            FormatError::ResourceLimitExceeded(text) => {
                FormatError::ResourceLimitExceeded(redact(text))
            }
            FormatError::UnsafeFileName(text) => FormatError::UnsafeFileName(redact(text)),
            FormatError::DependencyMissing(text) => FormatError::DependencyMissing(redact(text)),
            FormatError::Other(_) => {
                FormatError::Other("7-Zip failed while reading the Split WIM set".into())
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
) -> Result<BoundWimSource, FormatError> {
    bind_file_with_control(source_path, source_identity, src, &ControlToken::default())
}

pub(super) fn bind_file_with_control(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    mut src: Box<dyn ReadSeek>,
    control: &ControlToken,
) -> Result<BoundWimSource, FormatError> {
    match discover_bound_set(source_path, source_identity, &mut *src, control)? {
        Some(discovered) => Ok(BoundWimSource::Split(discovered, src)),
        None => Ok(BoundWimSource::Single(src)),
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

pub(super) fn is_split_wim(src: &mut dyn ReadSeek) -> Result<bool, FormatError> {
    Ok(inspect_wim_header(src)?.is_some_and(WimHeader::is_split))
}

pub(super) fn validate_generated_set(
    first_part: &Path,
) -> Result<Vec<GeneratedWimPart>, FormatError> {
    let mut first_file =
        stable_source::open_regular_file_no_follow(first_part, "generated WIM volume")?;
    stable_source::harden_private_regular_file(&first_file)?;
    let expected = inspect_wim_header(&mut first_file)?
        .ok_or_else(|| FormatError::CorruptArchive("generated WIM header is missing".into()))?;
    if expected.part_number != 1
        || expected.total_parts == 0
        || expected.guid.iter().all(|byte| *byte == 0)
    {
        return Err(FormatError::CorruptArchive(
            "generated WIM has inconsistent part metadata".into(),
        ));
    }
    if expected.total_parts > 1 {
        expected.validate_split()?;
    }
    let naming = SplitWimNaming::from_selected(
        first_part.file_name().ok_or_else(|| {
            FormatError::CorruptArchive("generated WIM part has no file name".into())
        })?,
        1,
    )?;
    let parent = stable_source::parent_or_current(first_part);
    let mut parts = Vec::with_capacity(usize::from(expected.total_parts));
    for part_number in 1..=expected.total_parts {
        let path = parent.join(naming.member_name(u32::from(part_number)));
        let mut file = stable_source::open_regular_file_no_follow(&path, "generated WIM volume")?;
        stable_source::harden_private_regular_file(&file)?;
        let identity = SourceIdentity::from_file(&file)?;
        stable_source::verify_source_binding(&path, &identity, "generated WIM volume")?;
        let header = inspect_wim_header(&mut file)?.ok_or_else(|| {
            FormatError::CorruptArchive(format!(
                "generated WIM member has no WIM header: {}",
                path.display()
            ))
        })?;
        if header.part_number != part_number || !header.belongs_to(expected) {
            return Err(FormatError::CorruptArchive(format!(
                "generated WIM member metadata does not match part {part_number}: {}",
                path.display()
            )));
        }
        if expected.total_parts > 1 {
            header.validate_split()?;
        }
        let len = file.metadata()?.len();
        if len == 0 {
            return Err(FormatError::CorruptArchive(format!(
                "generated WIM member is empty: {}",
                path.display()
            )));
        }
        parts.push(GeneratedWimPart {
            path,
            identity,
            len,
        });
    }
    let extra = parent.join(naming.member_name(u32::from(expected.total_parts) + 1));
    match fs::symlink_metadata(&extra) {
        Ok(_) => Err(FormatError::CorruptArchive(format!(
            "generated WIM has an unexpected extra member: {}",
            extra.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(parts),
        Err(error) => Err(error.into()),
    }
}

fn discover_bound_set(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: &mut dyn ReadSeek,
    control: &ControlToken,
) -> Result<Option<DiscoveredSplitWimSet>, FormatError> {
    control.checkpoint()?;
    let selected_header = inspect_wim_header(src)?;
    control.checkpoint()?;
    let Some(selected_header) = selected_header else {
        return Ok(None);
    };
    if !selected_header.is_split() {
        return Ok(None);
    }
    let selected_header = selected_header.validate_split()?;
    let expected_identity = source_identity.ok_or_else(|| {
        FormatError::CorruptArchive("Split WIM discovery requires an opened-file identity".into())
    })?;
    let (selected_path, selected_identity) = stable_source::resolve_selected_regular_path(
        source_path,
        expected_identity,
        "WIM volume",
        control,
    )?;
    let naming = SplitWimNaming::from_selected(
        selected_path.file_name().ok_or_else(|| {
            FormatError::CorruptArchive("selected WIM volume has no file name".into())
        })?,
        selected_header.part_number,
    )?;
    let parent = stable_source::parent_or_current(&selected_path);
    let mut candidates = BTreeMap::new();

    for part_number in 1..=selected_header.total_parts {
        control.checkpoint()?;
        let path = parent.join(naming.member_name(u32::from(part_number)));
        let selected = path == selected_path;
        let (identity, header) = if selected {
            (selected_identity.clone(), selected_header)
        } else {
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(FormatError::missing_volume(path));
                }
                Err(error) => return Err(error.into()),
            };
            if !stable_source::is_regular_source_metadata(&metadata) {
                return Err(FormatError::CorruptArchive(format!(
                    "Split WIM member is not a regular file: {}",
                    path.display()
                )));
            }
            let mut file = stable_source::open_regular_file_no_follow(&path, "WIM volume")?;
            let identity = SourceIdentity::from_file(&file)?;
            stable_source::verify_source_binding(&path, &identity, "WIM volume")?;
            let header = inspect_wim_header(&mut file)?
                .ok_or_else(|| {
                    FormatError::CorruptArchive(format!(
                        "Split WIM member has no WIM header: {}",
                        path.display()
                    ))
                })?
                .validate_split()?;
            control.checkpoint()?;
            (identity, header)
        };
        if !header.belongs_to(selected_header) || header.part_number != part_number {
            return Err(FormatError::CorruptArchive(format!(
                "Split WIM member metadata does not match part {part_number}: {}",
                path.display()
            )));
        }
        candidates.insert(
            part_number,
            Candidate {
                path,
                selected,
                identity,
                header,
            },
        );
    }

    control.checkpoint()?;
    let extra = parent.join(naming.member_name(u32::from(selected_header.total_parts) + 1));
    match fs::symlink_metadata(&extra) {
        Ok(_) => {
            return Err(FormatError::CorruptArchive(format!(
                "Split WIM has an unexpected extra member: {}",
                extra.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    stable_source::verify_source_binding(&selected_path, &selected_identity, "WIM volume")?;
    control.checkpoint()?;
    Ok(Some(DiscoveredSplitWimSet { candidates }))
}

fn inspect_wim_header(src: &mut dyn ReadSeek) -> Result<Option<WimHeader>, FormatError> {
    let original_position = src.stream_position()?;
    src.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(WIM_HEADER_PREFIX_SIZE);
    let read_result = src
        .take(WIM_HEADER_PREFIX_SIZE as u64)
        .read_to_end(&mut bytes);
    let restore_result = src.seek(SeekFrom::Start(original_position));
    if let Err(error) = read_result {
        let _ = restore_result;
        return Err(error.into());
    }
    restore_result?;
    if bytes.len() < 8 || !bytes.starts_with(b"MSWIM\0\0\0") {
        return Ok(None);
    }
    if bytes.len() < WIM_HEADER_PREFIX_SIZE {
        return Err(FormatError::CorruptArchive(
            "WIM header is truncated before its part metadata".into(),
        ));
    }
    let header_size = u32_at(&bytes, 8);
    if header_size < WIM_HEADER_MIN_SIZE {
        return Err(FormatError::CorruptArchive(
            "WIM header declares an invalid size".into(),
        ));
    }
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&bytes[24..40]);
    Ok(Some(WimHeader {
        header_size,
        version: u32_at(&bytes, 12),
        flags: u32_at(&bytes, 16),
        chunk_size: u32_at(&bytes, 20),
        guid,
        part_number: u16_at(&bytes, 40),
        total_parts: u16_at(&bytes, 42),
    }))
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

struct SplitWimNaming {
    base: OsString,
    extension: OsString,
}

impl SplitWimNaming {
    fn from_selected(name: &OsStr, part_number: u16) -> Result<Self, FormatError> {
        let path = Path::new(name);
        let extension = path.extension().ok_or_else(|| {
            FormatError::CorruptArchive("Split WIM member has no .swm extension".into())
        })?;
        if !extension
            .to_str()
            .is_some_and(|value| value.eq_ignore_ascii_case("swm"))
        {
            return Err(FormatError::CorruptArchive(
                "Split WIM member must use the .swm extension".into(),
            ));
        }
        let stem = path.file_stem().ok_or_else(|| {
            FormatError::CorruptArchive("Split WIM member has an empty name".into())
        })?;
        let base = if part_number == 1 {
            stem.to_os_string()
        } else {
            strip_ascii_suffix(stem, &part_number.to_string()).ok_or_else(|| {
                FormatError::CorruptArchive(format!(
                    "Split WIM part {part_number} name does not end in its part number"
                ))
            })?
        };
        if base.is_empty() {
            return Err(FormatError::CorruptArchive(
                "Split WIM member has an empty base name".into(),
            ));
        }
        Ok(Self {
            base,
            extension: extension.to_os_string(),
        })
    }

    fn member_name(&self, part_number: u32) -> OsString {
        let mut name = self.base.clone();
        if part_number > 1 {
            name.push(part_number.to_string());
        }
        name.push(".");
        name.push(&self.extension);
        name
    }
}

fn staged_name(part_number: u16) -> OsString {
    let mut name = OsString::from("archive");
    if part_number > 1 {
        name.push(part_number.to_string());
    }
    name.push(".swm");
    name
}

#[cfg(unix)]
fn strip_ascii_suffix(value: &OsStr, suffix: &str) -> Option<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let bytes = value.as_bytes();
    let prefix = bytes.strip_suffix(suffix.as_bytes())?;
    Some(OsString::from_vec(prefix.to_vec()))
}

#[cfg(windows)]
fn strip_ascii_suffix(value: &OsStr, suffix: &str) -> Option<OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide = value.encode_wide().collect::<Vec<_>>();
    let suffix = suffix.encode_utf16().collect::<Vec<_>>();
    let prefix = wide.strip_suffix(&suffix)?;
    Some(OsString::from_wide(prefix))
}

#[cfg(not(any(unix, windows)))]
fn strip_ascii_suffix(value: &OsStr, suffix: &str) -> Option<OsString> {
    value.to_str()?.strip_suffix(suffix).map(OsString::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Seek;

    fn temp_dir(tag: &str) -> PathBuf {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "squallz-wim-volume-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn header(part_number: u16, total_parts: u16, guid: [u8; 16]) -> Vec<u8> {
        let mut bytes = vec![0u8; WIM_HEADER_MIN_SIZE as usize];
        bytes[..8].copy_from_slice(b"MSWIM\0\0\0");
        bytes[8..12].copy_from_slice(&WIM_HEADER_MIN_SIZE.to_le_bytes());
        bytes[12..16].copy_from_slice(&0x0001_0d00u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&WIM_SPANNED_FLAG.to_le_bytes());
        bytes[20..24].copy_from_slice(&(32 * 1024u32).to_le_bytes());
        bytes[24..40].copy_from_slice(&guid);
        bytes[40..42].copy_from_slice(&part_number.to_le_bytes());
        bytes[42..44].copy_from_slice(&total_parts.to_le_bytes());
        bytes
    }

    fn physical_identity(path: &Path) -> PhysicalFileIdentity {
        SourceIdentity::from_file(&File::open(path).unwrap())
            .unwrap()
            .physical_identity()
    }

    #[test]
    fn discovers_a_complete_set_from_any_member_in_native_order() {
        let root = temp_dir("discover");
        fs::create_dir_all(&root).unwrap();
        let guid = [0x5au8; 16];
        let paths = [
            root.join("install.swm"),
            root.join("install2.swm"),
            root.join("install3.swm"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, header(index as u16 + 1, 3, guid)).unwrap();
        }

        let selected = &paths[1];
        let mut stream = File::open(selected).unwrap();
        stream.seek(SeekFrom::Start(9)).unwrap();
        let source_set = probe_bound_file(selected, Some(physical_identity(selected)), &mut stream)
            .unwrap()
            .unwrap();

        assert_eq!(stream.stream_position().unwrap(), 9);
        assert_eq!(source_set.primary(), paths[0]);
        assert_eq!(source_set.members(), paths);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_the_exact_missing_native_member() {
        let root = temp_dir("missing");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("media.swm");
        let third = root.join("media3.swm");
        fs::write(&first, header(1, 3, [0x33; 16])).unwrap();
        fs::write(&third, header(3, 3, [0x33; 16])).unwrap();
        let mut stream = File::open(&third).unwrap();

        let error =
            probe_bound_file(&third, Some(physical_identity(&third)), &mut stream).unwrap_err();

        assert_eq!(
            error.missing_volume_path(),
            Some(root.join("media2.swm").as_path())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_member_from_another_split_wim() {
        let root = temp_dir("mismatch");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("image.swm");
        let second = root.join("image2.swm");
        fs::write(&first, header(1, 2, [0x11; 16])).unwrap();
        fs::write(&second, header(2, 2, [0x22; 16])).unwrap();
        let mut stream = File::open(&first).unwrap();

        let error =
            probe_bound_file(&first, Some(physical_identity(&first)), &mut stream).unwrap_err();

        assert!(matches!(error, FormatError::CorruptArchive(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_set_uses_private_names_and_binds_original_members() {
        let root = temp_dir("stage");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("files.swm");
        let second = root.join("files2.swm");
        fs::write(&first, header(1, 2, [0x77; 16])).unwrap();
        fs::write(&second, header(2, 2, [0x77; 16])).unwrap();
        let selected_stream = File::open(&second).unwrap();
        let selected_identity = physical_identity(&second);
        let BoundWimSource::Split(discovered, selected_stream) =
            bind_file(&second, Some(selected_identity), Box::new(selected_stream)).unwrap()
        else {
            panic!("expected Split WIM source");
        };

        let staged = StagedSplitWimSet::from_discovered(discovered, selected_stream).unwrap();

        assert_eq!(staged.path().file_name(), Some(OsStr::new("archive.swm")));
        assert!(staged.root.join("archive2.swm").is_file());
        staged.verify_source_set(&ControlToken::default()).unwrap();
        fs::remove_file(&first).unwrap();
        fs::write(&first, header(1, 2, [0x77; 16])).unwrap();
        assert!(staged
            .verify_source_set(&ControlToken::default())
            .unwrap_err()
            .is_input_changed());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(staged.root.join("archive2.swm"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(staged);
        fs::remove_dir_all(root).unwrap();
    }
}
