use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_format_api::{
    ArchiveSourceSet, ControlToken, FormatError, PhysicalFileIdentity, ReadSeek,
};

use crate::stable_source::{self, BoundSourceSet, PrivateStagingDir, SourceIdentity};

use super::{RAR4_MAGIC, RAR5_MAGIC};

const MAX_RAR_VOLUME_COUNT: u64 = 1_000_000;
const MAX_RAR5_HEADER_SIZE: u64 = 2 * 1024 * 1024;
const RAR5_HEADER_EXTRA: u64 = 0x0001;
const RAR5_HEADER_DATA: u64 = 0x0002;
const RAR5_MAIN_HEADER: u64 = 1;
const RAR5_ENCRYPTION_HEADER: u64 = 4;
const RAR5_END_HEADER: u64 = 5;
const RAR5_MAIN_VOLUME: u64 = 0x0001;
const RAR5_MAIN_VOLUME_NUMBER: u64 = 0x0002;
const RAR5_END_NEXT_VOLUME: u64 = 0x0001;
const RAR4_MAIN_HEADER: u8 = 0x73;
const RAR4_FILE_HEADER: u8 = 0x74;
const RAR4_SERVICE_HEADER: u8 = 0x7a;
const RAR4_END_HEADER: u8 = 0x7b;
const RAR4_LONG_BLOCK: u16 = 0x8000;
const RAR4_MAIN_VOLUME: u16 = 0x0001;
const RAR4_MAIN_PASSWORD: u16 = 0x0080;
const RAR4_MAIN_FIRST_VOLUME: u16 = 0x0100;
const RAR4_FILE_LARGE: u16 = 0x0100;
const RAR4_END_NEXT_VOLUME: u16 = 0x0001;
const RAR4_MAIN_FIELDS_SIZE: usize = 6;
const RAR4_FILE_FIELDS_SIZE: usize = 25;
const RAR4_FILE_LARGE_FIELDS_SIZE: usize = 33;
const RAR4_HIGH_PACK_SIZE_OFFSET: usize = 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RarVersion {
    Rar4,
    Rar5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RarVolumeMetadata {
    version: RarVersion,
    is_volume: bool,
    index: Option<u64>,
    has_next: Option<bool>,
    headers_encrypted: bool,
    first_volume: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RarSetEvidence {
    PublicHeaders,
    EncryptedHeaders(RarVersion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModernPadding {
    Unpadded,
    Fixed(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModernName {
    base: OsString,
    index: u64,
    digits: String,
    part_prefix: String,
    rar_extension: String,
}

struct ModernNameShape {
    base: OsString,
    digits: String,
    part_prefix: String,
    rar_extension: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyName {
    base: OsString,
    index: u64,
    rar_extension: String,
    r_prefix: String,
}

impl ModernName {
    fn padding_candidates(&self) -> Vec<ModernPadding> {
        if self.digits.len() > 1 && self.digits.starts_with('0') {
            vec![ModernPadding::Fixed(self.digits.len())]
        } else {
            let mut candidates = vec![ModernPadding::Unpadded];
            candidates.extend((2..=self.digits.len()).map(ModernPadding::Fixed));
            candidates
        }
    }
}

#[derive(Clone, Debug)]
struct ModernScheme {
    base: OsString,
    padding_candidates: Vec<ModernPadding>,
    part_prefix: String,
    rar_extension: String,
}

#[derive(Clone, Debug)]
struct LegacyScheme {
    base: OsString,
    rar_extension: String,
    r_prefix: String,
}

#[derive(Clone, Debug)]
enum VolumeScheme {
    Modern(ModernScheme),
    Legacy(LegacyScheme),
}

#[derive(Debug)]
struct Candidate {
    index: u64,
    path: PathBuf,
    selected: bool,
    identity: SourceIdentity,
}

struct DiscoveredRarSet {
    scheme: VolumeScheme,
    candidates: BTreeMap<u64, Candidate>,
    source_parent: PathBuf,
    evidence: RarSetEvidence,
}

pub(super) struct StagedRarSet {
    root: PrivateStagingDir,
    primary: PathBuf,
    native_scheme: Option<VolumeScheme>,
    source_parent: Option<PathBuf>,
    source_set: Option<BoundSourceSet>,
    externally_verified_volume_count: Option<u64>,
}

impl StagedRarSet {
    #[cfg(test)]
    pub(super) fn single(src: Box<dyn ReadSeek>) -> Result<Self, FormatError> {
        Self::single_with_control(src, &ControlToken::default())
    }

    pub(super) fn single_with_control(
        mut src: Box<dyn ReadSeek>,
        control: &ControlToken,
    ) -> Result<Self, FormatError> {
        control.checkpoint()?;
        let root = create_private_staging_dir()?;
        let primary = root.join("archive.rar");
        copy_read_seek(&mut *src, &primary, control)?;
        control.checkpoint()?;
        Ok(Self {
            root,
            primary,
            native_scheme: None,
            source_parent: None,
            source_set: None,
            externally_verified_volume_count: None,
        })
    }

    #[cfg(test)]
    pub(super) fn from_bound_file(
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
    ) -> Result<Self, FormatError> {
        Self::from_bound_file_with_control(
            source_path,
            source_identity,
            src,
            &ControlToken::default(),
        )
    }

    pub(super) fn from_bound_file_with_control(
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        mut src: Box<dyn ReadSeek>,
        control: &ControlToken,
    ) -> Result<Self, FormatError> {
        control.checkpoint()?;
        let Some(discovered) =
            discover_bound_set(source_path, source_identity, &mut *src, control)?
        else {
            return Self::single_with_control(src, control);
        };
        control.checkpoint()?;
        let DiscoveredRarSet {
            scheme,
            candidates,
            source_parent,
            evidence,
        } = discovered;
        let candidate_count = u64::try_from(candidates.len()).map_err(|_| volume_limit_error())?;
        let root = create_private_staging_dir()?;
        let mut staged = Self {
            root,
            primary: PathBuf::new(),
            native_scheme: Some(scheme.clone()),
            source_parent: Some(source_parent),
            source_set: None,
            externally_verified_volume_count: matches!(
                evidence,
                RarSetEvidence::EncryptedHeaders(_)
            )
            .then_some(candidate_count),
        };

        let mut selected_src = Some(src);
        let mut previous_metadata: Option<(u64, RarVolumeMetadata)> = None;
        for candidate in candidates.values() {
            control.checkpoint()?;
            let staged_name = scheme.canonical_name(candidate.index)?;
            let staged_path = staged.root.join(staged_name);
            if candidate.selected {
                let selected = selected_src.as_mut().ok_or_else(|| {
                    FormatError::CorruptArchive("RAR volume selection is ambiguous".into())
                })?;
                copy_read_seek(&mut **selected, &staged_path, control)?;
                verify_source_binding(&candidate.path, &candidate.identity)?;
            } else {
                copy_stable_source(&candidate.path, &candidate.identity, &staged_path, control)?;
            }

            let mut staged_file = File::open(&staged_path)?;
            match evidence {
                RarSetEvidence::PublicHeaders => {
                    let metadata = inspect_rar(&mut staged_file)?;
                    validate_volume_metadata(candidate.index, metadata)?;
                    validate_sequence_member(previous_metadata, metadata)?;
                    previous_metadata = Some((candidate.index, metadata));
                }
                RarSetEvidence::EncryptedHeaders(version) => {
                    validate_encrypted_rar_header(&mut staged_file, version, candidate.index)?;
                }
            }
            if candidate.index == 0 {
                staged.primary = staged_path;
            }
        }

        if staged.primary.as_os_str().is_empty() {
            return Err(FormatError::CorruptArchive(
                "RAR volume set has no first volume".into(),
            ));
        }
        if evidence == RarSetEvidence::PublicHeaders {
            let Some((last_index, metadata)) = previous_metadata else {
                return Err(FormatError::CorruptArchive(
                    "RAR volume set is empty".into(),
                ));
            };
            if metadata.has_next == Some(true) {
                let missing_index = last_index.checked_add(1).ok_or_else(volume_limit_error)?;
                let expected = staged.source_path_for_index(missing_index)?;
                return Err(FormatError::missing_volume(expected));
            }
        }
        for candidate in candidates.values() {
            control.checkpoint()?;
            verify_source_binding(&candidate.path, &candidate.identity)?;
        }
        let bindings = candidates
            .values()
            .map(|candidate| (candidate.path.clone(), candidate.identity.clone()))
            .collect();
        let source_set = ArchiveSourceSet::from_ordered_members(
            candidates
                .into_values()
                .map(|candidate| candidate.path)
                .collect(),
        )?;
        staged.source_set = Some(BoundSourceSet::new(source_set, bindings)?);
        control.checkpoint()?;
        Ok(staged)
    }

    #[cfg(test)]
    pub(super) fn from_file(
        source_path: &Path,
        src: Box<dyn ReadSeek>,
    ) -> Result<Self, FormatError> {
        let file = open_regular_file_no_follow(source_path)?;
        let identity = SourceIdentity::from_file(&file)?.physical_identity();
        Self::from_bound_file(source_path, Some(identity), src)
    }

    pub(super) fn path(&self) -> &Path {
        &self.primary
    }

    pub(super) fn len(&self) -> Result<u64, FormatError> {
        Ok(fs::metadata(&self.primary)?.len())
    }

    pub(super) fn is_native_multivolume(&self) -> bool {
        self.native_scheme.is_some()
    }

    pub(super) fn source_set(&self) -> Option<&ArchiveSourceSet> {
        self.source_set.as_ref().map(BoundSourceSet::source_set)
    }

    pub(super) fn verify_source_set(
        &self,
        control: &squallz_format_api::ControlToken,
    ) -> Result<(), FormatError> {
        match &self.source_set {
            Some(source_set) => source_set.verify_current("RAR volume", control),
            None => control.checkpoint(),
        }
    }

    pub(super) fn validate_external_volume_properties(
        &self,
        properties: Option<crate::sevenzip_bridge::SevenZipArchiveProperties>,
    ) -> Result<(), FormatError> {
        let Some(expected_count) = self.externally_verified_volume_count else {
            return Ok(());
        };
        let properties = properties.ok_or_else(|| {
            FormatError::CorruptArchive(
                "the encrypted RAR backend did not report archive properties".into(),
            )
        })?;
        match properties.multivolume {
            Some(false) if expected_count == 1 => return Ok(()),
            Some(true) => {}
            Some(false) => {
                return Err(FormatError::CorruptArchive(
                    "RAR files have encrypted headers but 7-Zip did not identify a volume set"
                        .into(),
                ));
            }
            None => {
                return Err(FormatError::CorruptArchive(
                    "7-Zip did not report whether the encrypted RAR is multi-volume".into(),
                ));
            }
        }
        if properties.volume_index != Some(0) {
            return Err(FormatError::CorruptArchive(
                "7-Zip did not open the first encrypted RAR volume".into(),
            ));
        }
        match properties.volume_count {
            Some(actual_count) if actual_count == expected_count => Ok(()),
            Some(actual_count) => Err(FormatError::CorruptArchive(format!(
                "encrypted RAR volume count mismatch: expected {expected_count}, 7-Zip reported {actual_count}"
            ))),
            None => Err(FormatError::CorruptArchive(
                "7-Zip did not report the encrypted RAR volume count".into(),
            )),
        }
    }

    pub(super) fn remap_external_error(&self, error: FormatError) -> FormatError {
        if let Some(missing) = error.missing_volume_path() {
            if let (Some(file_name), Some(scheme)) = (missing.file_name(), &self.native_scheme) {
                if let Some(index) = scheme.canonical_index(file_name) {
                    if let Ok(path) = self.source_path_for_index(index) {
                        return FormatError::missing_volume(path);
                    }
                }
            }
        }
        self.redact_staging_error(error)
    }

    fn redact_staging_error(&self, error: FormatError) -> FormatError {
        let redact = |text: String| {
            text.replace(
                self.root.to_string_lossy().as_ref(),
                "[private RAR staging]",
            )
        };
        match error {
            FormatError::Io(error) => {
                let kind = error.kind();
                FormatError::from(io::Error::new(kind, redact(error.to_string())))
            }
            FormatError::Unsupported(text) => FormatError::Unsupported(redact(text)),
            FormatError::CorruptArchive(_) => {
                FormatError::CorruptArchive("RAR backend could not read the staged archive".into())
            }
            FormatError::PathTraversal(text) => FormatError::PathTraversal(redact(text)),
            FormatError::SymlinkBreakout(text) => FormatError::SymlinkBreakout(redact(text)),
            FormatError::ResourceLimitExceeded(text) => {
                FormatError::ResourceLimitExceeded(redact(text))
            }
            FormatError::UnsafeFileName(text) => FormatError::UnsafeFileName(redact(text)),
            FormatError::DependencyMissing(text) => FormatError::DependencyMissing(redact(text)),
            FormatError::Other(_) => {
                FormatError::Other("RAR backend failed while reading the staged archive".into())
            }
            other => other,
        }
    }

    fn source_path_for_index(&self, index: u64) -> Result<PathBuf, FormatError> {
        let scheme = self.native_scheme.as_ref().ok_or_else(|| {
            FormatError::CorruptArchive("RAR volume set has no source naming scheme".into())
        })?;
        let parent = self.source_parent.as_deref().ok_or_else(|| {
            FormatError::CorruptArchive("RAR volume set has no source directory".into())
        })?;
        Ok(parent.join(scheme.source_name(index)?))
    }
}

impl VolumeScheme {
    fn source_name(&self, index: u64) -> Result<OsString, FormatError> {
        match self {
            Self::Modern(scheme) => scheme.name(index, false),
            Self::Legacy(scheme) => scheme.name(index, false),
        }
    }

    fn canonical_name(&self, index: u64) -> Result<OsString, FormatError> {
        match self {
            Self::Modern(scheme) => scheme.name(index, true),
            Self::Legacy(scheme) => scheme.name(index, true),
        }
    }

    fn canonical_index(&self, name: &OsStr) -> Option<u64> {
        match self {
            Self::Modern(scheme) => {
                let parsed = parse_modern_name(name)?;
                let padding = scheme.resolved_padding().ok()?;
                (parsed.base == OsStr::new("archive")
                    && format_volume_number(parsed.index + 1, padding) == parsed.digits)
                    .then_some(parsed.index)
            }
            Self::Legacy(_) => {
                let parsed = parse_legacy_name(name)?;
                (parsed.base == OsStr::new("archive")).then_some(parsed.index)
            }
        }
    }
}

impl ModernScheme {
    fn name(&self, index: u64, canonical: bool) -> Result<OsString, FormatError> {
        self.name_with_padding(index, canonical, self.resolved_padding()?)
    }

    fn name_with_padding(
        &self,
        index: u64,
        canonical: bool,
        padding: ModernPadding,
    ) -> Result<OsString, FormatError> {
        ensure_volume_index(index)?;
        let number = index.checked_add(1).ok_or_else(volume_limit_error)?;
        let mut name = if canonical {
            OsString::from("archive")
        } else {
            self.base.clone()
        };
        name.push(".");
        name.push(if canonical { "part" } else { &self.part_prefix });
        name.push(format_volume_number(number, padding));
        name.push(".");
        name.push(if canonical {
            "rar"
        } else {
            &self.rar_extension
        });
        Ok(name)
    }

    fn resolve_candidate_padding(&mut self, digits: &str, index: u64) -> Result<(), FormatError> {
        let number = index.checked_add(1).ok_or_else(volume_limit_error)?;
        self.padding_candidates
            .retain(|padding| format_volume_number(number, *padding) == digits);
        if self.padding_candidates.is_empty() {
            Err(FormatError::CorruptArchive(
                "RAR volume set mixes numbering widths".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn unambiguous_source_name(&self, index: u64) -> Result<OsString, FormatError> {
        let mut names = self
            .padding_candidates
            .iter()
            .copied()
            .map(|padding| self.name_with_padding(index, false, padding));
        let first = names.next().ok_or_else(|| {
            FormatError::CorruptArchive("RAR volume numbering has no valid width".into())
        })??;
        for name in names {
            if name? != first {
                return Err(FormatError::CorruptArchive(
                    "RAR volume numbering width cannot be determined from the available members"
                        .into(),
                ));
            }
        }
        Ok(first)
    }

    fn resolved_padding(&self) -> Result<ModernPadding, FormatError> {
        match self.padding_candidates.as_slice() {
            [padding] => Ok(*padding),
            [] => Err(FormatError::CorruptArchive(
                "RAR volume numbering has no valid width".into(),
            )),
            _ => Err(FormatError::CorruptArchive(
                "RAR volume numbering width cannot be determined from the available members".into(),
            )),
        }
    }
}

impl LegacyScheme {
    fn name(&self, index: u64, canonical: bool) -> Result<OsString, FormatError> {
        ensure_volume_index(index)?;
        if index > 100 {
            return Err(FormatError::Unsupported(
                "legacy RAR volume names after .r99 are not supported".into(),
            ));
        }
        let mut name = if canonical {
            OsString::from("archive")
        } else {
            self.base.clone()
        };
        if index == 0 {
            name.push(".");
            name.push(if canonical {
                "rar"
            } else {
                &self.rar_extension
            });
        } else {
            name.push(".");
            name.push(if canonical { "r" } else { &self.r_prefix });
            name.push(format!("{:02}", index - 1));
        }
        Ok(name)
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
    match discover_bound_set(source_path, source_identity, src, control)? {
        Some(discovered) => Ok(Some(ArchiveSourceSet::from_ordered_members(
            discovered
                .candidates
                .into_values()
                .map(|candidate| candidate.path)
                .collect(),
        )?)),
        None => Ok(None),
    }
}

fn discover_bound_set(
    source_path: &Path,
    source_identity: Option<PhysicalFileIdentity>,
    src: &mut dyn ReadSeek,
    control: &ControlToken,
) -> Result<Option<DiscoveredRarSet>, FormatError> {
    control.checkpoint()?;
    let file_name = match source_path.file_name() {
        Some(file_name) => file_name,
        None => return Ok(None),
    };
    let modern_name = parse_modern_name_shape(file_name).is_some();
    let legacy_index = parse_legacy_name(file_name).map(|name| name.index);
    if !modern_name && legacy_index.is_none() {
        return Ok(None);
    }

    let inspected = inspect_rar(src);
    control.checkpoint()?;
    let (selected_metadata, evidence) = match inspected {
        Ok(metadata) => {
            let evidence = if metadata.headers_encrypted {
                RarSetEvidence::EncryptedHeaders(metadata.version)
            } else {
                RarSetEvidence::PublicHeaders
            };
            (Some(metadata), evidence)
        }
        Err(FormatError::PasswordRequired) if modern_name => {
            (None, RarSetEvidence::EncryptedHeaders(RarVersion::Rar5))
        }
        Err(_) if !modern_name && legacy_index == Some(0) => return Ok(None),
        Err(error) => return Err(error),
    };
    if let Some(metadata) = selected_metadata {
        if !metadata.is_volume {
            return Ok(None);
        }
    }
    let expected_identity = source_identity.ok_or_else(|| {
        FormatError::Unsupported(
            "native RAR volume discovery requires an opened-file identity".into(),
        )
    })?;
    let (selected_path, selected_identity) =
        resolve_selected_regular_path(source_path, expected_identity, control)?;
    let scheme = match selected_metadata {
        Some(metadata) => native_scheme(&selected_path, metadata)?,
        None => Some(encrypted_modern_scheme(&selected_path)?),
    };
    let Some(mut scheme) = scheme else {
        return Ok(None);
    };
    let candidates = discover_candidates(&selected_path, selected_identity, &mut scheme, control)?;
    let source_parent = parent_or_current(&selected_path).to_path_buf();
    match (selected_metadata, evidence) {
        (Some(metadata), RarSetEvidence::PublicHeaders) => {
            validate_candidate_headers(&candidates, metadata, &scheme, &source_parent, control)?;
        }
        (_, RarSetEvidence::EncryptedHeaders(version)) => {
            validate_encrypted_candidate_headers(&candidates, version, control)?;
        }
        _ => {
            return Err(FormatError::CorruptArchive(
                "RAR volume evidence is inconsistent".into(),
            ));
        }
    }
    control.checkpoint()?;
    Ok(Some(DiscoveredRarSet {
        scheme,
        candidates,
        source_parent,
        evidence,
    }))
}

fn validate_encrypted_candidate_headers(
    candidates: &BTreeMap<u64, Candidate>,
    version: RarVersion,
    control: &ControlToken,
) -> Result<(), FormatError> {
    for candidate in candidates.values() {
        control.checkpoint()?;
        if candidate.selected {
            continue;
        }
        let mut file = open_regular_file_no_follow(&candidate.path)?;
        let initial = SourceIdentity::from_file(&file)?;
        if initial != candidate.identity {
            return Err(FormatError::CorruptArchive(
                "a RAR volume changed before its encrypted header was inspected".into(),
            ));
        }
        verify_source_binding(&candidate.path, &initial)?;
        validate_encrypted_rar_header(&mut file, version, candidate.index)?;
        control.checkpoint()?;
        let final_identity = SourceIdentity::from_file(&file)?;
        if final_identity != initial {
            return Err(FormatError::CorruptArchive(
                "a RAR volume changed while its encrypted header was inspected".into(),
            ));
        }
        verify_source_binding(&candidate.path, &final_identity)?;
    }
    control.checkpoint()
}

fn validate_candidate_headers(
    candidates: &BTreeMap<u64, Candidate>,
    selected_metadata: RarVolumeMetadata,
    scheme: &VolumeScheme,
    source_parent: &Path,
    control: &ControlToken,
) -> Result<(), FormatError> {
    let mut previous_metadata = None;
    for candidate in candidates.values() {
        control.checkpoint()?;
        let metadata = if candidate.selected {
            selected_metadata
        } else {
            inspect_stable_candidate(candidate, control)?
        };
        validate_volume_metadata(candidate.index, metadata)?;
        validate_sequence_member(previous_metadata, metadata)?;
        previous_metadata = Some((candidate.index, metadata));
    }
    let Some((last_index, metadata)) = previous_metadata else {
        return Err(FormatError::CorruptArchive(
            "RAR volume set is empty".into(),
        ));
    };
    if metadata.has_next == Some(true) {
        let missing_index = last_index.checked_add(1).ok_or_else(volume_limit_error)?;
        return Err(FormatError::missing_volume(
            source_parent.join(scheme.source_name(missing_index)?),
        ));
    }
    for candidate in candidates.values() {
        control.checkpoint()?;
        verify_source_binding(&candidate.path, &candidate.identity)?;
    }
    control.checkpoint()
}

fn inspect_stable_candidate(
    candidate: &Candidate,
    control: &ControlToken,
) -> Result<RarVolumeMetadata, FormatError> {
    control.checkpoint()?;
    let mut file = open_regular_file_no_follow(&candidate.path)?;
    let initial = SourceIdentity::from_file(&file)?;
    if initial != candidate.identity {
        return Err(FormatError::CorruptArchive(
            "a RAR volume changed before its header was inspected".into(),
        ));
    }
    verify_source_binding(&candidate.path, &initial)?;
    let metadata = inspect_rar(&mut file)?;
    control.checkpoint()?;
    let final_identity = SourceIdentity::from_file(&file)?;
    if final_identity != initial {
        return Err(FormatError::CorruptArchive(
            "a RAR volume changed while its header was inspected".into(),
        ));
    }
    verify_source_binding(&candidate.path, &final_identity)?;
    control.checkpoint()?;
    Ok(metadata)
}

fn validate_sequence_member(
    previous_metadata: Option<(u64, RarVolumeMetadata)>,
    metadata: RarVolumeMetadata,
) -> Result<(), FormatError> {
    if let Some((previous_index, previous)) = previous_metadata {
        if previous.version != metadata.version {
            return Err(FormatError::CorruptArchive(
                "RAR volume set mixes archive versions".into(),
            ));
        }
        if previous.has_next == Some(false) {
            return Err(FormatError::CorruptArchive(format!(
                "RAR volume {} is marked as the last member but another volume follows",
                previous_index + 1
            )));
        }
    }
    Ok(())
}

fn native_scheme(
    source_path: &Path,
    metadata: RarVolumeMetadata,
) -> Result<Option<VolumeScheme>, FormatError> {
    if !metadata.is_volume {
        return Ok(None);
    }
    let file_name = source_path
        .file_name()
        .ok_or_else(|| FormatError::CorruptArchive("RAR volume path has no file name".into()))?;
    if let Some(shape) = parse_modern_name_shape(file_name) {
        let name = checked_modern_name(shape)?;
        validate_named_index(name.index, metadata)?;
        let padding_candidates = name.padding_candidates();
        return Ok(Some(VolumeScheme::Modern(ModernScheme {
            base: name.base,
            padding_candidates,
            part_prefix: name.part_prefix,
            rar_extension: name.rar_extension,
        })));
    }
    if let Some(name) = parse_legacy_name(file_name) {
        validate_named_index(name.index, metadata)?;
        if metadata.version == RarVersion::Rar5 && name.index == 0 {
            let has_legacy_sibling = parent_or_current(source_path)
                .read_dir()
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| parse_legacy_name(&entry.file_name()))
                .any(|candidate| candidate.base == name.base && candidate.index > 0);
            if !has_legacy_sibling {
                return Ok(None);
            }
        }
        return Ok(Some(VolumeScheme::Legacy(LegacyScheme {
            base: name.base,
            rar_extension: name.rar_extension,
            r_prefix: name.r_prefix,
        })));
    }
    Err(FormatError::Unsupported(
        "RAR volume uses an unsupported file naming scheme".into(),
    ))
}

fn encrypted_modern_scheme(source_path: &Path) -> Result<VolumeScheme, FormatError> {
    let file_name = source_path
        .file_name()
        .ok_or_else(|| FormatError::CorruptArchive("RAR volume path has no file name".into()))?;
    let shape = parse_modern_name_shape(file_name).ok_or_else(|| {
        FormatError::Unsupported("header-encrypted RAR volumes require partN.rar naming".into())
    })?;
    let name = checked_modern_name(shape)?;
    ensure_volume_index(name.index)?;
    let padding_candidates = name.padding_candidates();
    Ok(VolumeScheme::Modern(ModernScheme {
        base: name.base,
        padding_candidates,
        part_prefix: name.part_prefix,
        rar_extension: name.rar_extension,
    }))
}

fn discover_candidates(
    source_path: &Path,
    selected_identity: SourceIdentity,
    scheme: &mut VolumeScheme,
    control: &ControlToken,
) -> Result<BTreeMap<u64, Candidate>, FormatError> {
    control.checkpoint()?;
    let source_name = source_path
        .file_name()
        .ok_or_else(|| FormatError::CorruptArchive("RAR volume path has no file name".into()))?;
    let selected_index = match scheme {
        VolumeScheme::Modern(_) => parse_modern_name(source_name).map(|name| name.index),
        VolumeScheme::Legacy(_) => parse_legacy_name(source_name).map(|name| name.index),
    }
    .ok_or_else(|| FormatError::CorruptArchive("RAR volume name is invalid".into()))?;

    let mut candidates = BTreeMap::new();
    candidates.insert(
        selected_index,
        Candidate {
            index: selected_index,
            path: source_path.to_path_buf(),
            selected: true,
            identity: selected_identity,
        },
    );

    let parent = parent_or_current(source_path);
    for entry in fs::read_dir(parent)? {
        control.checkpoint()?;
        let entry = entry?;
        let file_name = entry.file_name();
        if file_name == source_name {
            continue;
        }
        let index = match &mut *scheme {
            VolumeScheme::Modern(modern) => {
                let Some(shape) = parse_modern_name_shape(&file_name) else {
                    continue;
                };
                if shape.base != modern.base {
                    continue;
                }
                let parsed = checked_modern_name(shape)?;
                if parsed.part_prefix != modern.part_prefix
                    || parsed.rar_extension != modern.rar_extension
                {
                    return Err(FormatError::CorruptArchive(
                        "RAR volume set mixes extension casing".into(),
                    ));
                }
                modern.resolve_candidate_padding(&parsed.digits, parsed.index)?;
                parsed.index
            }
            VolumeScheme::Legacy(legacy) => {
                let Some(parsed) = parse_legacy_name(&file_name) else {
                    continue;
                };
                if parsed.base != legacy.base {
                    continue;
                }
                let casing_matches = if parsed.index == 0 {
                    parsed.rar_extension == legacy.rar_extension
                } else {
                    parsed.r_prefix == legacy.r_prefix
                };
                if !casing_matches {
                    return Err(FormatError::CorruptArchive(
                        "RAR volume set mixes legacy extension casing".into(),
                    ));
                }
                parsed.index
            }
        };
        ensure_volume_index(index)?;
        let path = entry.path();
        let file = open_regular_file_no_follow(&path)?;
        let identity = SourceIdentity::from_file(&file)?;
        verify_source_binding(&path, &identity)?;
        if candidates
            .insert(
                index,
                Candidate {
                    index,
                    path,
                    selected: false,
                    identity,
                },
            )
            .is_some()
        {
            return Err(FormatError::CorruptArchive(format!(
                "RAR volume index {} appears more than once",
                index + 1
            )));
        }
        if candidates.len() as u64 > MAX_RAR_VOLUME_COUNT {
            return Err(volume_limit_error());
        }
    }

    let mut expected = 0u64;
    for index in candidates.keys().copied() {
        control.checkpoint()?;
        if index != expected {
            let missing_name = match &*scheme {
                VolumeScheme::Modern(modern) => modern.unambiguous_source_name(expected)?,
                VolumeScheme::Legacy(_) => scheme.source_name(expected)?,
            };
            return Err(FormatError::missing_volume(parent.join(missing_name)));
        }
        expected = expected.checked_add(1).ok_or_else(volume_limit_error)?;
    }
    if let VolumeScheme::Modern(modern) = scheme {
        modern.resolved_padding()?;
    }
    control.checkpoint()?;
    Ok(candidates)
}

fn resolve_selected_regular_path(
    source_path: &Path,
    expected_identity: PhysicalFileIdentity,
    control: &ControlToken,
) -> Result<(PathBuf, SourceIdentity), FormatError> {
    stable_source::resolve_selected_regular_path(
        source_path,
        expected_identity,
        "RAR volume",
        control,
    )
}

fn parse_modern_name(file_name: &OsStr) -> Option<ModernName> {
    checked_modern_name(parse_modern_name_shape(file_name)?).ok()
}

fn parse_modern_name_shape(file_name: &OsStr) -> Option<ModernNameShape> {
    let path = Path::new(file_name);
    let rar_extension = path.extension()?.to_str()?;
    if !rar_extension.eq_ignore_ascii_case("rar") {
        return None;
    }
    let stem = path.file_stem()?;
    let stem_path = Path::new(stem);
    let part = stem_path.extension()?.to_str()?;
    let part_bytes = part.as_bytes();
    if part_bytes.len() <= 4 || !part_bytes[..4].eq_ignore_ascii_case(b"part") {
        return None;
    }
    let digits = &part[4..];
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let base = stem_path.file_stem()?.to_os_string();
    if base.is_empty() {
        return None;
    }
    Some(ModernNameShape {
        base,
        digits: digits.to_owned(),
        part_prefix: part[..4].to_owned(),
        rar_extension: rar_extension.to_owned(),
    })
}

fn checked_modern_name(shape: ModernNameShape) -> Result<ModernName, FormatError> {
    let number = shape
        .digits
        .parse::<u64>()
        .map_err(|_| volume_limit_error())?;
    if number == 0 {
        return Err(FormatError::CorruptArchive(
            "RAR volume numbering starts at part1".into(),
        ));
    }
    let index = number - 1;
    ensure_volume_index(index)?;
    Ok(ModernName {
        base: shape.base,
        index,
        digits: shape.digits,
        part_prefix: shape.part_prefix,
        rar_extension: shape.rar_extension,
    })
}

fn parse_legacy_name(file_name: &OsStr) -> Option<LegacyName> {
    let path = Path::new(file_name);
    let extension = path.extension()?.to_str()?;
    let (index, rar_extension, r_prefix) = if extension.eq_ignore_ascii_case("rar") {
        (0, extension.to_owned(), extension.get(..1)?.to_owned())
    } else {
        let bytes = extension.as_bytes();
        if bytes.len() != 3
            || !bytes[0].eq_ignore_ascii_case(&b'r')
            || !bytes[1..].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        (
            extension[1..].parse::<u64>().ok()?.checked_add(1)?,
            if bytes[0].is_ascii_uppercase() {
                "RAR".to_owned()
            } else {
                "rar".to_owned()
            },
            extension[..1].to_owned(),
        )
    };
    let base = path.file_stem()?.to_os_string();
    (!base.is_empty()).then_some(LegacyName {
        base,
        index,
        rar_extension,
        r_prefix,
    })
}

fn format_volume_number(number: u64, padding: ModernPadding) -> String {
    match padding {
        ModernPadding::Unpadded => number.to_string(),
        ModernPadding::Fixed(width) => format!("{number:0width$}"),
    }
}

fn validate_named_index(named_index: u64, metadata: RarVolumeMetadata) -> Result<(), FormatError> {
    ensure_volume_index(named_index)?;
    if let Some(first_volume) = metadata.first_volume {
        if first_volume != (named_index == 0) {
            return Err(FormatError::CorruptArchive(format!(
                "RAR first-volume flag does not match file index {}",
                named_index + 1
            )));
        }
    }
    if let Some(header_index) = metadata.index {
        ensure_volume_index(header_index)?;
        if header_index != named_index {
            return Err(FormatError::CorruptArchive(format!(
                "RAR volume header index {} does not match file index {}",
                header_index + 1,
                named_index + 1
            )));
        }
    }
    Ok(())
}

fn validate_volume_metadata(
    named_index: u64,
    metadata: RarVolumeMetadata,
) -> Result<(), FormatError> {
    if !metadata.is_volume {
        return Err(FormatError::CorruptArchive(format!(
            "RAR file at volume index {} is not marked as a volume",
            named_index + 1
        )));
    }
    validate_named_index(named_index, metadata)
}

fn inspect_rar(src: &mut dyn ReadSeek) -> Result<RarVolumeMetadata, FormatError> {
    let original = src.stream_position()?;
    src.seek(SeekFrom::Start(0))?;
    let mut signature = [0u8; 8];
    let result = match src.read_exact(&mut signature) {
        Ok(()) if signature == RAR5_MAGIC => inspect_rar5(src),
        Ok(()) if signature[..RAR4_MAGIC.len()] == *RAR4_MAGIC => {
            src.seek(SeekFrom::Start(RAR4_MAGIC.len() as u64))?;
            inspect_rar4(src)
        }
        Ok(()) => Err(FormatError::CorruptArchive(
            "RAR signature is invalid".into(),
        )),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Err(
            FormatError::CorruptArchive("RAR header is truncated".into()),
        ),
        Err(error) => Err(FormatError::from(error)),
    };
    let rewind = src.seek(SeekFrom::Start(original));
    match (result, rewind) {
        (Ok(metadata), Ok(_)) => Ok(metadata),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(FormatError::from(error)),
    }
}

fn validate_encrypted_rar_header(
    src: &mut dyn ReadSeek,
    version: RarVersion,
    named_index: u64,
) -> Result<(), FormatError> {
    match (version, inspect_rar(src)) {
        (RarVersion::Rar5, Err(FormatError::PasswordRequired)) => Ok(()),
        (RarVersion::Rar4, Ok(metadata))
            if metadata.version == RarVersion::Rar4 && metadata.headers_encrypted =>
        {
            validate_volume_metadata(named_index, metadata)
        }
        (_, Ok(_)) => Err(FormatError::CorruptArchive(
            "RAR volume does not use the expected encrypted header format".into(),
        )),
        (_, Err(error)) => Err(error),
    }
}

fn inspect_rar5(src: &mut dyn ReadSeek) -> Result<RarVolumeMetadata, FormatError> {
    let file_len = src.seek(SeekFrom::End(0))?;
    src.seek(SeekFrom::Start(RAR5_MAGIC.len() as u64))?;
    let mut main = None;

    while src.stream_position()? < file_len {
        let block = read_rar5_block(src, file_len)?;
        match block.header_type {
            RAR5_ENCRYPTION_HEADER if main.is_none() => return Err(FormatError::PasswordRequired),
            RAR5_MAIN_HEADER if main.is_none() => {
                let mut fields = Cursor::new(block.specific_fields.as_slice());
                let archive_flags = read_vint(&mut fields, 10)?;
                let is_volume = archive_flags & RAR5_MAIN_VOLUME != 0;
                let has_number = archive_flags & RAR5_MAIN_VOLUME_NUMBER != 0;
                let index = if has_number {
                    Some(read_vint(&mut fields, 10)?)
                } else if is_volume {
                    Some(0)
                } else {
                    None
                };
                if !is_volume && has_number {
                    return Err(FormatError::CorruptArchive(
                        "RAR main header carries a volume number without a volume flag".into(),
                    ));
                }
                main = Some((is_volume, index));
            }
            RAR5_END_HEADER => {
                let (is_volume, index) = main.ok_or_else(|| {
                    FormatError::CorruptArchive("RAR archive has no main header".into())
                })?;
                let mut fields = Cursor::new(block.specific_fields.as_slice());
                let end_flags = read_vint(&mut fields, 10)?;
                return Ok(RarVolumeMetadata {
                    version: RarVersion::Rar5,
                    is_volume,
                    index,
                    has_next: Some(end_flags & RAR5_END_NEXT_VOLUME != 0),
                    headers_encrypted: false,
                    first_volume: None,
                });
            }
            _ => {}
        }
        let next = src
            .stream_position()?
            .checked_add(block.data_size)
            .ok_or_else(|| FormatError::CorruptArchive("RAR data offset overflows".into()))?;
        if next > file_len {
            return Err(FormatError::CorruptArchive(
                "RAR data block is truncated".into(),
            ));
        }
        src.seek(SeekFrom::Start(next))?;
    }
    Err(FormatError::CorruptArchive(
        "RAR archive has no end header".into(),
    ))
}

struct Rar5Block {
    header_type: u64,
    specific_fields: Vec<u8>,
    data_size: u64,
}

fn read_rar5_block(src: &mut dyn ReadSeek, file_len: u64) -> Result<Rar5Block, FormatError> {
    let mut expected_crc = [0u8; 4];
    read_archive_exact(src, &mut expected_crc, "RAR5 header CRC")?;
    let (header_size, size_bytes) = read_vint_bytes(src, 3)?;
    if header_size == 0 || header_size > MAX_RAR5_HEADER_SIZE {
        return Err(FormatError::CorruptArchive(
            "RAR5 header size is invalid".into(),
        ));
    }
    let header_end = src
        .stream_position()?
        .checked_add(header_size)
        .ok_or_else(|| FormatError::CorruptArchive("RAR5 header offset overflows".into()))?;
    if header_end > file_len {
        return Err(FormatError::CorruptArchive(
            "RAR5 header is truncated".into(),
        ));
    }
    let header_len = usize::try_from(header_size)
        .map_err(|_| FormatError::ResourceLimitExceeded("RAR5 header is too large".into()))?;
    let mut header = vec![0u8; header_len];
    read_archive_exact(src, &mut header, "RAR5 header")?;

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&size_bytes);
    hasher.update(&header);
    if hasher.finalize() != u32::from_le_bytes(expected_crc) {
        return Err(FormatError::CorruptArchive(
            "RAR5 header checksum does not match".into(),
        ));
    }

    let mut fields = Cursor::new(header.as_slice());
    let header_type = read_vint(&mut fields, 10)?;
    let header_flags = read_vint(&mut fields, 10)?;
    let extra_size = if header_flags & RAR5_HEADER_EXTRA != 0 {
        let extra_size = read_vint(&mut fields, 10)?;
        if extra_size > header_size {
            return Err(FormatError::CorruptArchive(
                "RAR5 extra area size is invalid".into(),
            ));
        }
        extra_size
    } else {
        0
    };
    let data_size = if header_flags & RAR5_HEADER_DATA != 0 {
        read_vint(&mut fields, 10)?
    } else {
        0
    };
    let offset = usize::try_from(fields.position())
        .map_err(|_| FormatError::CorruptArchive("RAR5 header field offset is invalid".into()))?;
    let extra_len = usize::try_from(extra_size)
        .map_err(|_| FormatError::ResourceLimitExceeded("RAR5 extra area is too large".into()))?;
    let specific_end = header
        .len()
        .checked_sub(extra_len)
        .filter(|end| *end >= offset)
        .ok_or_else(|| FormatError::CorruptArchive("RAR5 extra area is truncated".into()))?;
    let specific_fields = header
        .get(offset..specific_end)
        .ok_or_else(|| FormatError::CorruptArchive("RAR5 header fields are truncated".into()))?
        .to_vec();
    Ok(Rar5Block {
        header_type,
        specific_fields,
        data_size,
    })
}

fn inspect_rar4(src: &mut dyn ReadSeek) -> Result<RarVolumeMetadata, FormatError> {
    let file_len = src.seek(SeekFrom::End(0))?;
    src.seek(SeekFrom::Start(RAR4_MAGIC.len() as u64))?;
    let mut main = None;

    while src.stream_position()? < file_len {
        let mut common = [0u8; 7];
        read_archive_exact(src, &mut common, "RAR4 header")?;
        let expected_crc = u16::from_le_bytes([common[0], common[1]]);
        let header_type = common[2];
        let flags = u16::from_le_bytes([common[3], common[4]]);
        let header_size = u16::from_le_bytes([common[5], common[6]]) as usize;
        if header_size < common.len() {
            return Err(FormatError::CorruptArchive(
                "RAR4 header size is invalid".into(),
            ));
        }
        let remaining_len = header_size - common.len();
        let mut remaining = vec![0u8; remaining_len];
        read_archive_exact(src, &mut remaining, "RAR4 header")?;

        let mut checksum_data = Vec::with_capacity(header_size - 2);
        checksum_data.extend_from_slice(&common[2..]);
        checksum_data.extend_from_slice(&remaining);
        if crc32fast::hash(&checksum_data) as u16 != expected_crc {
            return Err(FormatError::CorruptArchive(
                "RAR4 header checksum does not match".into(),
            ));
        }

        validate_rar4_header_fields(header_type, flags, &remaining)?;
        let data_size = rar4_data_size(header_type, flags, &remaining)?;
        match header_type {
            RAR4_MAIN_HEADER if main.is_none() => {
                let headers_encrypted = flags & RAR4_MAIN_PASSWORD != 0;
                let metadata = RarVolumeMetadata {
                    version: RarVersion::Rar4,
                    is_volume: flags & RAR4_MAIN_VOLUME != 0,
                    index: None,
                    has_next: None,
                    headers_encrypted,
                    first_volume: headers_encrypted.then_some(flags & RAR4_MAIN_FIRST_VOLUME != 0),
                };
                if headers_encrypted {
                    return Ok(metadata);
                }
                main = Some(metadata);
            }
            RAR4_END_HEADER => {
                let mut metadata = main.ok_or_else(|| {
                    FormatError::CorruptArchive("RAR archive has no main header".into())
                })?;
                metadata.has_next = Some(flags & RAR4_END_NEXT_VOLUME != 0);
                return Ok(metadata);
            }
            _ => {}
        }
        let next = src
            .stream_position()?
            .checked_add(data_size)
            .ok_or_else(|| FormatError::CorruptArchive("RAR4 data offset overflows".into()))?;
        if next > file_len {
            return Err(FormatError::CorruptArchive(
                "RAR4 data block is truncated".into(),
            ));
        }
        src.seek(SeekFrom::Start(next))?;
    }
    main.ok_or_else(|| FormatError::CorruptArchive("RAR archive has no main header".into()))
}

fn validate_rar4_header_fields(
    header_type: u8,
    flags: u16,
    remaining: &[u8],
) -> Result<(), FormatError> {
    let minimum = match header_type {
        RAR4_MAIN_HEADER => RAR4_MAIN_FIELDS_SIZE,
        RAR4_FILE_HEADER | RAR4_SERVICE_HEADER if flags & RAR4_FILE_LARGE != 0 => {
            RAR4_FILE_LARGE_FIELDS_SIZE
        }
        RAR4_FILE_HEADER | RAR4_SERVICE_HEADER => RAR4_FILE_FIELDS_SIZE,
        _ if flags & RAR4_LONG_BLOCK != 0 => 4,
        _ => 0,
    };
    if remaining.len() < minimum {
        return Err(FormatError::CorruptArchive(format!(
            "RAR4 header type 0x{header_type:02x} is too short"
        )));
    }
    Ok(())
}

fn rar4_data_size(header_type: u8, flags: u16, remaining: &[u8]) -> Result<u64, FormatError> {
    if flags & RAR4_LONG_BLOCK == 0 {
        return Ok(0);
    }
    let low = remaining
        .get(..4)
        .ok_or_else(|| FormatError::CorruptArchive("RAR4 long block is truncated".into()))?;
    let low = u64::from(u32::from_le_bytes([low[0], low[1], low[2], low[3]]));
    if !matches!(header_type, RAR4_FILE_HEADER | RAR4_SERVICE_HEADER)
        || flags & RAR4_FILE_LARGE == 0
    {
        return Ok(low);
    }
    let high = remaining
        .get(RAR4_HIGH_PACK_SIZE_OFFSET..RAR4_HIGH_PACK_SIZE_OFFSET + 4)
        .ok_or_else(|| {
            FormatError::CorruptArchive("RAR4 large file or service header is truncated".into())
        })?;
    let high = u64::from(u32::from_le_bytes([high[0], high[1], high[2], high[3]]));
    Ok(low | (high << 32))
}

fn read_vint(src: &mut dyn Read, max_bytes: usize) -> Result<u64, FormatError> {
    read_vint_bytes(src, max_bytes).map(|(value, _)| value)
}

fn read_vint_bytes(src: &mut dyn Read, max_bytes: usize) -> Result<(u64, Vec<u8>), FormatError> {
    let mut value = 0u64;
    let mut bytes = Vec::with_capacity(max_bytes);
    for shift_index in 0..max_bytes {
        let mut byte = [0u8; 1];
        read_archive_exact(src, &mut byte, "RAR variable integer")?;
        bytes.push(byte[0]);
        let shift = shift_index * 7;
        let payload = u64::from(byte[0] & 0x7f);
        if (shift >= 64 && payload != 0) || (shift < 64 && payload > (u64::MAX >> shift)) {
            return Err(FormatError::CorruptArchive(
                "RAR variable integer overflows".into(),
            ));
        }
        if shift < 64 {
            value |= payload
                .checked_shl(shift as u32)
                .ok_or_else(|| FormatError::CorruptArchive("RAR integer overflows".into()))?;
        }
        if byte[0] & 0x80 == 0 {
            return Ok((value, bytes));
        }
    }
    Err(FormatError::CorruptArchive(
        "RAR variable integer is too long".into(),
    ))
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

fn copy_read_seek(
    src: &mut dyn ReadSeek,
    destination: &Path,
    control: &ControlToken,
) -> Result<(), FormatError> {
    stable_source::copy_selected_stream(src, destination, control)
}

fn copy_stable_source(
    source: &Path,
    expected: &SourceIdentity,
    destination: &Path,
    control: &ControlToken,
) -> Result<(), FormatError> {
    stable_source::copy_stable_source(source, expected, destination, "RAR volume", control)
}

fn create_private_staging_dir() -> Result<PrivateStagingDir, FormatError> {
    stable_source::create_private_staging_dir("rar-volume")
}

fn open_regular_file_no_follow(path: &Path) -> Result<File, FormatError> {
    stable_source::open_regular_file_no_follow(path, "RAR volume")
}

fn verify_source_binding(path: &Path, expected: &SourceIdentity) -> Result<(), FormatError> {
    stable_source::verify_source_binding(path, expected, "RAR volume")
}

fn parent_or_current(path: &Path) -> &Path {
    stable_source::parent_or_current(path)
}

fn ensure_volume_index(index: u64) -> Result<(), FormatError> {
    if index < MAX_RAR_VOLUME_COUNT {
        Ok(())
    } else {
        Err(volume_limit_error())
    }
}

fn volume_limit_error() -> FormatError {
    FormatError::ResourceLimitExceeded(format!("RAR volume count exceeds {MAX_RAR_VOLUME_COUNT}"))
}

#[cfg(test)]
pub(super) fn test_physical_identity(path: &Path) -> Result<PhysicalFileIdentity, FormatError> {
    let file = open_regular_file_no_follow(path)?;
    Ok(SourceIdentity::from_file(&file)?.physical_identity())
}

#[cfg(test)]
pub(super) fn test_rar5_volume(index: u64, has_next: bool) -> Vec<u8> {
    fn encode_vint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return bytes;
            }
        }
    }

    fn block(header_type: u64, fields: &[u8]) -> Vec<u8> {
        let mut header = encode_vint(header_type);
        header.extend(encode_vint(0));
        header.extend_from_slice(fields);
        let size = encode_vint(header.len() as u64);
        let mut checksum_data = size.clone();
        checksum_data.extend_from_slice(&header);
        let mut block = crc32fast::hash(&checksum_data).to_le_bytes().to_vec();
        block.extend(size);
        block.extend(header);
        block
    }

    let mut bytes = RAR5_MAGIC.to_vec();
    let mut archive_flags = RAR5_MAIN_VOLUME;
    let mut main_fields = Vec::new();
    if index > 0 {
        archive_flags |= RAR5_MAIN_VOLUME_NUMBER;
    }
    main_fields.extend(encode_vint(archive_flags));
    if index > 0 {
        main_fields.extend(encode_vint(index));
    }
    bytes.extend(block(RAR5_MAIN_HEADER, &main_fields));
    bytes.extend(block(RAR5_END_HEADER, &encode_vint(u64::from(has_next))));
    bytes
}

#[cfg(test)]
pub(super) fn test_rar5_encrypted_header() -> Vec<u8> {
    fn encode_vint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return bytes;
            }
        }
    }

    let mut fields = vec![0, 0, 15];
    fields.extend_from_slice(&[0x5a; 16]);
    let mut header = encode_vint(RAR5_ENCRYPTION_HEADER);
    header.extend(encode_vint(0));
    header.extend(fields);
    let size = encode_vint(header.len() as u64);
    let mut checksum_data = size.clone();
    checksum_data.extend_from_slice(&header);
    let mut bytes = RAR5_MAGIC.to_vec();
    bytes.extend(crc32fast::hash(&checksum_data).to_le_bytes());
    bytes.extend(size);
    bytes.extend(header);
    bytes
}

#[cfg(test)]
fn test_rar4_header(header_type: u8, flags: u16, fields: &[u8]) -> Vec<u8> {
    let size = u16::try_from(7 + fields.len()).unwrap_or(u16::MAX);
    let mut checksum_data = vec![header_type];
    checksum_data.extend_from_slice(&flags.to_le_bytes());
    checksum_data.extend_from_slice(&size.to_le_bytes());
    checksum_data.extend_from_slice(fields);
    let mut bytes = (crc32fast::hash(&checksum_data) as u16)
        .to_le_bytes()
        .to_vec();
    bytes.extend(checksum_data);
    bytes
}

#[cfg(test)]
pub(super) fn test_rar4_volume(has_next: bool) -> Vec<u8> {
    let mut bytes = RAR4_MAGIC.to_vec();
    bytes.extend(test_rar4_header(
        RAR4_MAIN_HEADER,
        RAR4_MAIN_VOLUME,
        &[0; RAR4_MAIN_FIELDS_SIZE],
    ));
    bytes.extend(test_rar4_header(
        RAR4_END_HEADER,
        if has_next { RAR4_END_NEXT_VOLUME } else { 0 },
        &[],
    ));
    bytes
}

#[cfg(test)]
pub(super) fn test_rar4_encrypted_volume(first_volume: bool) -> Vec<u8> {
    let mut flags = RAR4_MAIN_VOLUME | RAR4_MAIN_PASSWORD;
    if first_volume {
        flags |= RAR4_MAIN_FIRST_VOLUME;
    }
    let mut bytes = RAR4_MAGIC.to_vec();
    bytes.extend(test_rar4_header(
        RAR4_MAIN_HEADER,
        flags,
        &[0; RAR4_MAIN_FIELDS_SIZE],
    ));
    bytes.extend([0x5a; 8]);
    bytes.extend([0xa5; 16]);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Seek as _;

    fn vint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return bytes;
            }
        }
    }

    fn rar5_block(header_type: u64, header_flags: u64, fields: &[u8], data: &[u8]) -> Vec<u8> {
        let mut header = vint(header_type);
        header.extend(vint(header_flags));
        if header_flags & RAR5_HEADER_DATA != 0 {
            header.extend(vint(data.len() as u64));
        }
        header.extend_from_slice(fields);
        let size = vint(header.len() as u64);
        let mut checksum_data = size.clone();
        checksum_data.extend_from_slice(&header);
        let mut block = crc32fast::hash(&checksum_data).to_le_bytes().to_vec();
        block.extend(size);
        block.extend(header);
        block.extend_from_slice(data);
        block
    }

    fn rar5_block_with_extra(header_type: u64, fields: &[u8], extra: &[u8]) -> Vec<u8> {
        let mut header = vint(header_type);
        header.extend(vint(RAR5_HEADER_EXTRA));
        header.extend(vint(extra.len() as u64));
        header.extend_from_slice(fields);
        header.extend_from_slice(extra);
        let size = vint(header.len() as u64);
        let mut checksum_data = size.clone();
        checksum_data.extend_from_slice(&header);
        let mut block = crc32fast::hash(&checksum_data).to_le_bytes().to_vec();
        block.extend(size);
        block.extend(header);
        block
    }

    fn rar5_volume(index: u64, has_next: bool) -> Vec<u8> {
        let mut bytes = RAR5_MAGIC.to_vec();
        let mut archive_flags = RAR5_MAIN_VOLUME;
        let mut main_fields = Vec::new();
        if index > 0 {
            archive_flags |= RAR5_MAIN_VOLUME_NUMBER;
        }
        main_fields.extend(vint(archive_flags));
        if index > 0 {
            main_fields.extend(vint(index));
        }
        bytes.extend(rar5_block(RAR5_MAIN_HEADER, 0, &main_fields, &[]));
        bytes.extend(rar5_block(
            RAR5_END_HEADER,
            0,
            &vint(u64::from(has_next)),
            &[],
        ));
        bytes
    }

    fn rar5_single() -> Vec<u8> {
        let mut bytes = RAR5_MAGIC.to_vec();
        bytes.extend(rar5_block(RAR5_MAIN_HEADER, 0, &vint(0), &[]));
        bytes.extend(rar5_block(RAR5_END_HEADER, 0, &vint(0), &[]));
        bytes
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "squallz-rar-volume-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn modern_and_legacy_names_are_parsed_without_aliasing_generic_splits() {
        let modern = parse_modern_name(OsStr::new("movie.part002.RAR")).unwrap();
        assert_eq!(modern.base, OsStr::new("movie"));
        assert_eq!(modern.index, 1);
        assert_eq!(modern.padding_candidates(), vec![ModernPadding::Fixed(3)]);
        assert!(parse_modern_name(OsStr::new("movie.rar.001")).is_none());
        assert!(parse_modern_name(OsStr::new("movie.part0.rar")).is_none());

        assert_eq!(
            parse_legacy_name(OsStr::new("movie.rar")),
            Some(LegacyName {
                base: OsString::from("movie"),
                index: 0,
                rar_extension: "rar".into(),
                r_prefix: "r".into(),
            })
        );
        assert_eq!(
            parse_legacy_name(OsStr::new("movie.R09")),
            Some(LegacyName {
                base: OsString::from("movie"),
                index: 10,
                rar_extension: "RAR".into(),
                r_prefix: "R".into(),
            })
        );
        assert!(parse_legacy_name(OsStr::new("movie.r100")).is_none());
    }

    #[test]
    fn fixed_width_inference_survives_numbers_wider_than_the_minimum_width() {
        for (selected, first, expected) in [
            (
                "movie.part100.rar",
                "movie.part01.rar",
                ModernPadding::Fixed(2),
            ),
            (
                "movie.part1000.rar",
                "movie.part001.rar",
                ModernPadding::Fixed(3),
            ),
        ] {
            let selected = parse_modern_name(OsStr::new(selected)).unwrap();
            let first = parse_modern_name(OsStr::new(first)).unwrap();
            let padding_candidates = selected.padding_candidates();
            let mut scheme = ModernScheme {
                base: selected.base,
                padding_candidates,
                part_prefix: selected.part_prefix,
                rar_extension: selected.rar_extension,
            };
            scheme
                .resolve_candidate_padding(&first.digits, first.index)
                .unwrap();
            assert_eq!(scheme.resolved_padding().unwrap(), expected);
        }
    }

    #[test]
    fn rar5_volume_metadata_checks_index_and_tail_flag() {
        let mut first = Cursor::new(rar5_volume(0, true));
        assert_eq!(
            inspect_rar(&mut first).unwrap(),
            RarVolumeMetadata {
                version: RarVersion::Rar5,
                is_volume: true,
                index: Some(0),
                has_next: Some(true),
                headers_encrypted: false,
                first_volume: None,
            }
        );

        let mut third = Cursor::new(rar5_volume(2, false));
        assert_eq!(
            inspect_rar(&mut third).unwrap(),
            RarVolumeMetadata {
                version: RarVersion::Rar5,
                is_volume: true,
                index: Some(2),
                has_next: Some(false),
                headers_encrypted: false,
                first_volume: None,
            }
        );
    }

    #[test]
    fn rar5_extra_area_cannot_supply_missing_main_fields() {
        let mut bytes = RAR5_MAGIC.to_vec();
        bytes.extend(rar5_block_with_extra(
            RAR5_MAIN_HEADER,
            &[],
            &[RAR5_MAIN_VOLUME as u8],
        ));
        bytes.extend(rar5_block(RAR5_END_HEADER, 0, &vint(0), &[]));

        let error = inspect_rar(&mut Cursor::new(bytes)).unwrap_err();
        assert!(matches!(error, FormatError::CorruptArchive(_)));
    }

    #[test]
    fn rar_variable_integer_rejects_high_bits_in_its_tenth_byte() {
        let mut bytes = vec![0x80; 9];
        bytes.push(0x02);
        let error = read_vint(&mut Cursor::new(bytes), 10).unwrap_err();
        assert!(matches!(error, FormatError::CorruptArchive(_)));
    }

    #[test]
    fn rar5_header_volume_index_cannot_overflow_diagnostics() {
        let error = validate_named_index(
            0,
            RarVolumeMetadata {
                version: RarVersion::Rar5,
                is_volume: true,
                index: Some(u64::MAX),
                has_next: Some(false),
                headers_encrypted: false,
                first_volume: None,
            },
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
    }

    #[test]
    fn rar4_encrypted_main_header_preserves_volume_evidence() {
        let first = inspect_rar(&mut Cursor::new(test_rar4_encrypted_volume(true))).unwrap();
        assert_eq!(
            first,
            RarVolumeMetadata {
                version: RarVersion::Rar4,
                is_volume: true,
                index: None,
                has_next: None,
                headers_encrypted: true,
                first_volume: Some(true),
            }
        );

        let later = inspect_rar(&mut Cursor::new(test_rar4_encrypted_volume(false))).unwrap();
        assert_eq!(later.first_volume, Some(false));
        validate_named_index(1, later).unwrap();

        let error = validate_named_index(1, first).unwrap_err();
        assert!(matches!(error, FormatError::CorruptArchive(_)));
    }

    #[test]
    fn rar4_large_file_header_uses_the_high_packed_size() {
        let mut fields = vec![0u8; RAR4_FILE_LARGE_FIELDS_SIZE];
        fields[..4].copy_from_slice(&1u32.to_le_bytes());
        fields[RAR4_HIGH_PACK_SIZE_OFFSET..RAR4_HIGH_PACK_SIZE_OFFSET + 4]
            .copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            rar4_data_size(RAR4_FILE_HEADER, RAR4_LONG_BLOCK | RAR4_FILE_LARGE, &fields).unwrap(),
            (2u64 << 32) | 1
        );
    }

    #[test]
    fn rar4_large_service_header_uses_the_high_packed_size() {
        let mut fields = vec![0u8; RAR4_FILE_LARGE_FIELDS_SIZE];
        fields[..4].copy_from_slice(&1u32.to_le_bytes());
        fields[RAR4_HIGH_PACK_SIZE_OFFSET..RAR4_HIGH_PACK_SIZE_OFFSET + 4]
            .copy_from_slice(&2u32.to_le_bytes());
        assert!(validate_rar4_header_fields(
            RAR4_SERVICE_HEADER,
            RAR4_LONG_BLOCK | RAR4_FILE_LARGE,
            &fields,
        )
        .is_ok());
        assert!(matches!(
            validate_rar4_header_fields(
                RAR4_SERVICE_HEADER,
                RAR4_LONG_BLOCK | RAR4_FILE_LARGE,
                &fields[..RAR4_FILE_LARGE_FIELDS_SIZE - 1],
            ),
            Err(FormatError::CorruptArchive(_))
        ));
        assert_eq!(
            rar4_data_size(
                RAR4_SERVICE_HEADER,
                RAR4_LONG_BLOCK | RAR4_FILE_LARGE,
                &fields,
            )
            .unwrap(),
            (2u64 << 32) | 1
        );
    }

    #[test]
    fn native_modern_set_normalizes_to_first_and_preserves_selected_stream() {
        let dir = temp_dir("modern");
        let first = dir.join("sample.part001.rar");
        let second = dir.join("sample.part002.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        fs::write(&second, b"path content must not be reopened").unwrap();
        let selected_bytes = rar5_volume(1, false);

        let staged =
            StagedRarSet::from_file(&second, Box::new(Cursor::new(selected_bytes.clone())))
                .unwrap();
        assert_eq!(
            staged.path().file_name(),
            Some(OsStr::new("archive.part001.rar"))
        );
        assert_eq!(
            fs::read(staged.root.join("archive.part002.rar")).unwrap(),
            selected_bytes
        );
        let stage_root = staged.root.path().to_path_buf();
        drop(staged);
        assert!(!stage_root.exists());
        assert!(first.exists());
        assert_eq!(
            fs::read(&second).unwrap(),
            b"path content must not be reopened"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_set_requires_the_identity_of_the_engine_opened_file() {
        let dir = temp_dir("selected-without-identity");
        let first = dir.join("sample.part1.rar");
        let selected = dir.join("sample.part2.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        fs::write(&selected, rar5_volume(1, false)).unwrap();
        let opened = File::open(&selected).unwrap();

        let error = match StagedRarSet::from_bound_file(&selected, None, Box::new(opened)) {
            Ok(_) => panic!("native discovery without an opened-file identity must fail closed"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::Unsupported(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_set_rejects_selected_path_replacement_after_engine_open() {
        let dir = temp_dir("selected-replaced");
        let first = dir.join("sample.part1.rar");
        let selected = dir.join("sample.part2.rar");
        let displaced = dir.join("sample.part2.opened");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        fs::write(&selected, rar5_volume(1, false)).unwrap();
        let opened = File::open(&selected).unwrap();
        let identity = SourceIdentity::from_file(&opened)
            .unwrap()
            .physical_identity();
        fs::rename(&selected, &displaced).unwrap();
        fs::write(&selected, rar5_volume(1, false)).unwrap();

        let error = match StagedRarSet::from_bound_file(&selected, Some(identity), Box::new(opened))
        {
            Ok(_) => panic!("replaced selected path must not be mixed with sibling volumes"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::CorruptArchive(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_fixed_width_set_opens_from_part100() {
        let dir = temp_dir("fixed-width-part100");
        for number in 1..=100u64 {
            let path = dir.join(format!("sample.part{number:03}.rar"));
            fs::write(&path, rar5_volume(number - 1, number < 100)).unwrap();
        }
        let selected = dir.join("sample.part100.rar");
        let selected_bytes = rar5_volume(99, false);

        let staged =
            StagedRarSet::from_file(&selected, Box::new(Cursor::new(selected_bytes))).unwrap();
        assert_eq!(
            staged.path().file_name(),
            Some(OsStr::new("archive.part001.rar"))
        );
        assert!(staged.root.join("archive.part100.rar").is_file());
        drop(staged);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_probe_returns_ordered_set_from_any_member() {
        let dir = temp_dir("probe-ordered");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        let first_bytes = rar5_volume(0, true);
        let second_bytes = rar5_volume(1, false);
        fs::write(&first, &first_bytes).unwrap();
        fs::write(&second, &second_bytes).unwrap();
        let mut selected = File::open(&second).unwrap();
        let identity = SourceIdentity::from_file(&selected)
            .unwrap()
            .physical_identity();

        let source_set = probe_bound_file(&second, Some(identity), &mut selected)
            .unwrap()
            .unwrap();
        assert_eq!(source_set.primary(), first);
        assert_eq!(source_set.members(), &[first, second]);
        assert_eq!(selected.stream_position().unwrap(), 0);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_probe_does_not_group_similarly_named_single_rars() {
        let dir = temp_dir("probe-independent");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        fs::write(&first, rar5_single()).unwrap();
        fs::write(&second, rar5_single()).unwrap();
        let mut selected = File::open(&first).unwrap();
        let identity = SourceIdentity::from_file(&selected)
            .unwrap()
            .physical_identity();

        assert!(probe_bound_file(&first, Some(identity), &mut selected)
            .unwrap()
            .is_none());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_reports_exact_missing_middle_volume() {
        let dir = temp_dir("missing-middle");
        let first = dir.join("sample.part1.rar");
        let third = dir.join("sample.part3.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        fs::write(&third, rar5_volume(2, false)).unwrap();

        let error =
            match StagedRarSet::from_file(&third, Box::new(Cursor::new(rar5_volume(2, false)))) {
                Ok(_) => panic!("gapped RAR set must fail"),
                Err(error) => error,
            };
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("sample.part2.rar").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_reports_exact_missing_first_volume() {
        let dir = temp_dir("missing-first");
        let second = dir.join("sample.part2.rar");
        let second_bytes = rar5_volume(1, false);
        fs::write(&second, &second_bytes).unwrap();

        let error = match StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))) {
            Ok(_) => panic!("RAR set without its first volume must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("sample.part1.rar").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_reports_exact_missing_tail_volume() {
        let dir = temp_dir("missing-tail");
        let first = dir.join("sample.part1.rar");
        let first_bytes = rar5_volume(0, true);
        fs::write(&first, &first_bytes).unwrap();

        let error = match StagedRarSet::from_file(&first, Box::new(Cursor::new(first_bytes))) {
            Ok(_) => panic!("RAR set with an explicit next-volume flag must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("sample.part2.rar").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn part_style_name_without_volume_header_stays_single_file() {
        let dir = temp_dir("single-part-name");
        let selected = dir.join("sample.part1.rar");
        let bytes = rar5_single();
        fs::write(&selected, &bytes).unwrap();

        let staged = StagedRarSet::from_file(&selected, Box::new(Cursor::new(bytes))).unwrap();
        assert!(!staged.is_native_multivolume());
        assert_eq!(staged.path().file_name(), Some(OsStr::new("archive.rar")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plain_rar_with_unreadable_volume_metadata_uses_single_file_bridge() {
        let dir = temp_dir("plain-rar");
        let selected = dir.join("sample.rar");
        let bytes = RAR5_MAGIC.to_vec();
        fs::write(&selected, &bytes).unwrap();

        let staged =
            StagedRarSet::from_file(&selected, Box::new(Cursor::new(bytes.clone()))).unwrap();
        assert!(!staged.is_native_multivolume());
        assert_eq!(fs::read(staged.path()).unwrap(), bytes);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn external_errors_do_not_expose_the_private_staging_path() {
        let staged =
            StagedRarSet::single(Box::new(Cursor::new(rar5_single()))).expect("staging must work");
        let root = staged.root.to_string_lossy().into_owned();
        let error = staged.remap_external_error(FormatError::CorruptArchive(format!(
            "backend failed while reading {}",
            staged.path().display()
        )));
        let detail = error.to_string();
        assert!(!detail.contains(&root));
        assert_eq!(
            detail,
            "corrupt archive: RAR backend could not read the staged archive"
        );
    }

    #[test]
    fn native_modern_set_rejects_mixed_numbering_widths() {
        let dir = temp_dir("mixed-width");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part002.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        let second_bytes = rar5_volume(1, false);
        fs::write(&second, &second_bytes).unwrap();

        let error = match StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))) {
            Ok(_) => panic!("mixed RAR volume widths must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::CorruptArchive(_)));
        assert!(error.missing_volume_path().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_rejects_header_and_filename_index_mismatch() {
        let dir = temp_dir("index-mismatch");
        let second = dir.join("sample.part2.rar");
        fs::write(&second, rar5_volume(2, false)).unwrap();
        let error =
            match StagedRarSet::from_file(&second, Box::new(Cursor::new(rar5_volume(2, false)))) {
                Ok(_) => panic!("mismatched RAR index must fail"),
                Err(error) => error,
            };
        assert!(matches!(error, FormatError::CorruptArchive(_)));
        assert!(error.missing_volume_path().is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_rejects_a_premature_terminal_member() {
        let dir = temp_dir("premature-terminal");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        fs::write(&first, rar5_volume(0, false)).unwrap();
        let second_bytes = rar5_volume(1, false);
        fs::write(&second, &second_bytes).unwrap();

        let error = match StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))) {
            Ok(_) => panic!("volume after a terminal member must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::CorruptArchive(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_rejects_mixed_rar_versions() {
        let dir = temp_dir("mixed-version");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        let second_bytes = test_rar4_volume(false);
        fs::write(&second, &second_bytes).unwrap();

        let error = match StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))) {
            Ok(_) => panic!("mixed RAR versions must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::CorruptArchive(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_modern_set_rejects_volume_numbers_above_the_resource_limit() {
        let dir = temp_dir("index-limit");
        let selected = dir.join("sample.part1000001.rar");
        let bytes = test_rar5_volume(MAX_RAR_VOLUME_COUNT, false);
        fs::write(&selected, &bytes).unwrap();
        let error = match StagedRarSet::from_file(&selected, Box::new(Cursor::new(bytes))) {
            Ok(_) => panic!("oversized RAR volume index must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_legacy_set_normalizes_and_binds_original_members() {
        let dir = temp_dir("legacy");
        let first = dir.join("sample.rar");
        let second = dir.join("sample.r00");
        fs::write(&first, test_rar4_volume(true)).unwrap();
        let second_bytes = test_rar4_volume(false);
        fs::write(&second, &second_bytes).unwrap();

        let staged = StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))).unwrap();
        assert_eq!(staged.path().file_name(), Some(OsStr::new("archive.rar")));
        assert!(staged.root.join("archive.r00").is_file());
        staged
            .verify_source_set(&squallz_format_api::ControlToken::default())
            .unwrap();
        fs::remove_file(&first).unwrap();
        fs::write(&first, test_rar4_volume(true)).unwrap();
        assert!(staged
            .verify_source_set(&squallz_format_api::ControlToken::default())
            .unwrap_err()
            .is_input_changed());
        let stage_root = staged.root.path().to_path_buf();
        drop(staged);
        assert!(!stage_root.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_encrypted_legacy_set_requires_external_volume_confirmation() {
        let dir = temp_dir("encrypted-legacy");
        let first = dir.join("sample.rar");
        let second = dir.join("sample.r00");
        fs::write(&first, test_rar4_encrypted_volume(true)).unwrap();
        let second_bytes = test_rar4_encrypted_volume(false);
        fs::write(&second, &second_bytes).unwrap();

        let staged = StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))).unwrap();
        assert_eq!(staged.path().file_name(), Some(OsStr::new("archive.rar")));
        assert!(staged.root.join("archive.r00").is_file());
        let source_set = staged.source_set().unwrap();
        assert_eq!(source_set.primary(), first);
        assert_eq!(source_set.members(), &[first, second]);

        staged
            .validate_external_volume_properties(Some(
                crate::sevenzip_bridge::SevenZipArchiveProperties {
                    multivolume: Some(true),
                    volume_index: Some(0),
                    volume_count: Some(2),
                },
            ))
            .unwrap();
        let error = staged
            .validate_external_volume_properties(Some(
                crate::sevenzip_bridge::SevenZipArchiveProperties {
                    multivolume: Some(true),
                    volume_index: Some(0),
                    volume_count: Some(1),
                },
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            FormatError::CorruptArchive(detail)
                if detail.contains("expected 2, 7-Zip reported 1")
        ));

        let stage_root = staged.root.path().to_path_buf();
        drop(staged);
        assert!(!stage_root.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_legacy_set_accepts_members_without_end_records() {
        let dir = temp_dir("legacy-no-end");
        let first = dir.join("sample.rar");
        let second = dir.join("sample.r00");
        let mut first_bytes = test_rar4_volume(true);
        let mut second_bytes = test_rar4_volume(false);
        first_bytes.truncate(first_bytes.len() - 7);
        second_bytes.truncate(second_bytes.len() - 7);
        fs::write(&first, &first_bytes).unwrap();
        fs::write(&second, &second_bytes).unwrap();

        let staged = StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))).unwrap();
        assert_eq!(staged.path().file_name(), Some(OsStr::new("archive.rar")));
        drop(staged);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_legacy_missing_names_preserve_uppercase_extensions() {
        let dir = temp_dir("legacy-uppercase");
        let second = dir.join("Sample.R00");
        let second_bytes = test_rar4_volume(false);
        fs::write(&second, &second_bytes).unwrap();
        let error = match StagedRarSet::from_file(&second, Box::new(Cursor::new(second_bytes))) {
            Ok(_) => panic!("legacy set without its first volume must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("Sample.RAR").as_path())
        );
        fs::remove_file(&second).unwrap();

        let first = dir.join("Sample.RAR");
        let third = dir.join("Sample.R01");
        fs::write(&first, test_rar4_volume(true)).unwrap();
        let third_bytes = test_rar4_volume(false);
        fs::write(&third, &third_bytes).unwrap();
        let error = match StagedRarSet::from_file(&third, Box::new(Cursor::new(third_bytes))) {
            Ok(_) => panic!("legacy set with a missing middle volume must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.missing_volume_path(),
            Some(dir.join("Sample.R00").as_path())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn selected_member_uses_the_directory_entry_spelling_on_case_insensitive_volumes() {
        let dir = temp_dir("selected-case");
        let first = dir.join("Sample.part1.rar");
        let selected = dir.join("Sample.part2.rar");
        let alias = dir.join("sample.PART2.RAR");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        let selected_bytes = rar5_volume(1, false);
        fs::write(&selected, &selected_bytes).unwrap();
        if !alias.exists() {
            fs::remove_dir_all(dir).unwrap();
            return;
        }

        let staged =
            StagedRarSet::from_file(&alias, Box::new(Cursor::new(selected_bytes))).unwrap();
        assert_eq!(
            staged.path().file_name(),
            Some(OsStr::new("archive.part1.rar"))
        );
        drop(staged);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staging_uses_private_permissions_and_rejects_symlink_members() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let staged = StagedRarSet::single(Box::new(Cursor::new(rar5_volume(0, false)))).unwrap();
        assert_eq!(
            fs::metadata(&staged.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(staged.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(fs::metadata(staged.path()).unwrap().ino() > 0);
        drop(staged);

        let dir = temp_dir("symlink");
        let first = dir.join("sample.part1.rar");
        let second = dir.join("sample.part2.rar");
        let target = dir.join("target.rar");
        fs::write(&first, rar5_volume(0, true)).unwrap();
        fs::write(&target, rar5_volume(1, false)).unwrap();
        symlink(&target, &second).unwrap();
        let error =
            match StagedRarSet::from_file(&first, Box::new(Cursor::new(rar5_volume(0, true)))) {
                Ok(_) => panic!("symlink volume must fail"),
                Err(error) => error,
            };
        assert!(matches!(&error, FormatError::CorruptArchive(_)));
        assert!(!error.to_string().contains(dir.to_string_lossy().as_ref()));

        fs::remove_file(&second).unwrap();
        fs::write(&first, rar5_volume(0, true)).unwrap();
        symlink(&target, &second).unwrap();
        let error =
            match StagedRarSet::from_file(&second, Box::new(Cursor::new(rar5_volume(1, false)))) {
                Ok(_) => panic!("selected symlink volume must fail"),
                Err(error) => error,
            };
        assert!(matches!(&error, FormatError::CorruptArchive(_)));
        assert!(!error.to_string().contains(dir.to_string_lossy().as_ref()));
        fs::remove_dir_all(dir).unwrap();
    }
}
