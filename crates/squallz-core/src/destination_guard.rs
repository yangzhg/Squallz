use std::cmp::Ordering;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::api::{
    split_volume_name, ControlToken, EntryPath, FormatError, NoProgress, ProgressSink,
};
#[cfg(windows)]
use crate::filesystem_identity::path_change_time;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};

const TOKEN_PREFIX: &str = "sqcg1_";
const TOKEN_BYTES: usize = 65;
const DIGEST_BUFFER_BYTES: usize = 256 * 1024;
const MAX_TREE_DEPTH: usize = 64;
const MAX_TREE_ENTRIES: usize = 200_000;
const MAX_DIRECTORY_SNAPSHOT_ENTRIES: usize = MAX_TREE_ENTRIES;
const COMPOUND_CREATE_EXTENSIONS: &[&str] = &[
    "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "tar.lz4", "tar.br", "tar.lzma",
];

/// Physical output family protected by a create replacement authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateArtifactKind {
    Archive,
    SplitArchive,
    SfxSingleFile,
    SfxMacosApp,
}

impl CreateArtifactKind {
    fn tag(self) -> u8 {
        match self {
            Self::Archive => 1,
            Self::SplitArchive => 2,
            Self::SfxSingleFile => 3,
            Self::SfxMacosApp => 4,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Archive),
            2 => Some(Self::SplitArchive),
            3 => Some(Self::SfxSingleFile),
            4 => Some(Self::SfxMacosApp),
            _ => None,
        }
    }
}

/// Opaque, fixed-size authorization for replacing one observed destination.
///
/// Its serialized form is intentionally a single versioned string. Callers
/// must treat it as a secret-adjacent capability and exclude it from task
/// snapshots and logs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CreateDestinationGuard {
    bytes: [u8; TOKEN_BYTES],
}

impl CreateDestinationGuard {
    pub(crate) fn kind(self) -> CreateArtifactKind {
        CreateArtifactKind::from_tag(self.bytes[0]).unwrap_or(CreateArtifactKind::Archive)
    }

    pub(crate) fn path_digest(self) -> [u8; 32] {
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&self.bytes[1..33]);
        digest
    }

    pub(crate) fn state_digest(self) -> [u8; 32] {
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&self.bytes[33..65]);
        digest
    }
}

impl fmt::Debug for CreateDestinationGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CreateDestinationGuard([redacted])")
    }
}

impl Serialize for CreateDestinationGuard {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_guard(self.bytes))
    }
}

impl<'de> Deserialize<'de> for CreateDestinationGuard {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_guard(&value).map_err(de::Error::custom)
    }
}

/// Result of inspecting the exact output family a create operation manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateDestinationState {
    pub conflict: bool,
    pub guard: Option<CreateDestinationGuard>,
}

/// Reports whether the core-managed output family currently contains any
/// entry, without reading existing file contents.
pub fn create_destination_has_conflict(
    destination: &Path,
    kind: CreateArtifactKind,
) -> Result<bool, FormatError> {
    let destination = normalized_destination(destination, kind)?;
    let canonical = canonical_requested_path(&destination)?;
    if kind == CreateArtifactKind::SplitArchive {
        return crate::volumes::collect_managed_split_outputs(
            &canonical,
            crate::volumes::is_sqz_base(&canonical),
        )
        .map(|managed| !managed.is_empty());
    }
    match fs::symlink_metadata(canonical) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Returns the first automatically numbered destination that does not
/// conflict with the core-managed output family observed in one directory
/// snapshot. This is an advisory name selection; final publication must still
/// use [`CreateCommitPolicy::NoReplace`] to reject a late conflict.
pub fn find_available_create_destination(
    destination: &Path,
    kind: CreateArtifactKind,
) -> Result<PathBuf, FormatError> {
    find_available_create_destination_with(destination, kind, DirectorySnapshot::read)
}

fn find_available_create_destination_with<R>(
    destination: &Path,
    kind: CreateArtifactKind,
    read_snapshot: R,
) -> Result<PathBuf, FormatError>
where
    R: FnOnce(&Path) -> Result<DirectorySnapshot, FormatError>,
{
    find_available_create_destination_with_match(
        destination,
        kind,
        read_snapshot,
        snapshot_entry_matches,
    )
}

fn find_available_create_destination_with_match<R, M>(
    destination: &Path,
    kind: CreateArtifactKind,
    read_snapshot: R,
    mut entries_match: M,
) -> Result<PathBuf, FormatError>
where
    R: FnOnce(&Path) -> Result<DirectorySnapshot, FormatError>,
    M: FnMut(&Path, &Path) -> bool,
{
    let requested_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| FormatError::Unsupported("destination path has no file name".into()))?;
    let numbering_base = normalized_destination(destination, kind)?;
    let canonical_parent = fs::canonicalize(parent_or_current(destination))?;
    let snapshot = read_snapshot(&canonical_parent)?;
    if !snapshot.has_conflict_with(
        &canonical_parent.join(requested_name),
        kind,
        &mut entries_match,
    )? {
        return Ok(destination.to_path_buf());
    }

    let occupied_numbers = snapshot.numbered_conflicts_with(
        &canonical_parent,
        &numbering_base,
        kind,
        &mut entries_match,
    )?;

    let mut number = 2_usize;
    loop {
        let candidate = numbered_create_destination(&numbering_base, number);
        if !occupied_numbers.contains(&number) {
            return Ok(candidate);
        }
        number = number.checked_add(1).ok_or_else(|| {
            FormatError::ResourceLimitExceeded(
                "create destination suffix space is exhausted".into(),
            )
        })?;
    }
}

#[derive(Debug)]
struct DirectorySnapshot {
    entries: Vec<PathBuf>,
}

impl DirectorySnapshot {
    fn read(parent: &Path) -> Result<Self, FormatError> {
        Self::from_paths(
            fs::read_dir(parent)?
                .map(|entry| entry.map(|entry| entry.path()).map_err(FormatError::from)),
        )
    }

    fn from_paths<I>(paths: I) -> Result<Self, FormatError>
    where
        I: IntoIterator<Item = Result<PathBuf, FormatError>>,
    {
        let mut entries = Vec::new();
        for path in paths {
            if entries.len() >= MAX_DIRECTORY_SNAPSHOT_ENTRIES {
                return Err(FormatError::ResourceLimitExceeded(format!(
                    "destination directory snapshot exceeds {MAX_DIRECTORY_SNAPSHOT_ENTRIES} entries"
                )));
            }
            entries.push(path?);
        }
        Ok(Self { entries })
    }

    fn has_conflict_with<M>(
        &self,
        destination: &Path,
        kind: CreateArtifactKind,
        entries_match: &mut M,
    ) -> Result<bool, FormatError>
    where
        M: FnMut(&Path, &Path) -> bool,
    {
        let destination = normalized_destination(destination, kind)?;
        if kind != CreateArtifactKind::SplitArchive {
            return Ok(self
                .entries
                .iter()
                .any(|entry| entries_match(&destination, entry)));
        }

        let include_recovery = crate::volumes::is_sqz_base(&destination);
        for entry in &self.entries {
            let matches_base = entries_match(&destination, entry);
            let matches_member = entry
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(|name| {
                    crate::volumes::expected_managed_split_output_path(
                        &destination,
                        name,
                        include_recovery,
                    )
                })
                .is_some_and(|expected| entries_match(&expected, entry));
            if !matches_base && !matches_member {
                continue;
            }
            // Any occupied managed path reserves this candidate. The final
            // no-replace commit still rejects abnormal output types without
            // touching them.
            return Ok(true);
        }
        Ok(false)
    }

    fn numbered_conflicts_with<M>(
        &self,
        parent: &Path,
        numbering_base: &Path,
        kind: CreateArtifactKind,
        entries_match: &mut M,
    ) -> Result<HashSet<usize>, FormatError>
    where
        M: FnMut(&Path, &Path) -> bool,
    {
        let include_recovery = crate::volumes::is_sqz_base(numbering_base);
        let mut occupied = HashSet::new();
        for entry in &self.entries {
            let Some(name) = entry.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if let Some(number) = numbered_create_destination_index(name) {
                let candidate = canonical_numbered_candidate(parent, numbering_base, number)?;
                if entries_match(&candidate, entry) {
                    occupied.insert(number);
                }
            }
            if kind != CreateArtifactKind::SplitArchive {
                continue;
            }
            let managed_base_name = split_volume_name(name).map(|(base, _)| base).or_else(|| {
                include_recovery
                    .then(|| crate::volumes::sqz_recovery_suffix(name))
                    .flatten()
                    .and_then(|(suffix, _)| name.strip_suffix(suffix))
            });
            let Some(number) = managed_base_name.and_then(numbered_create_destination_index) else {
                continue;
            };
            let candidate = canonical_numbered_candidate(parent, numbering_base, number)?;
            let Some(expected_member) = crate::volumes::expected_managed_split_output_path(
                &candidate,
                name,
                include_recovery,
            ) else {
                continue;
            };
            if entries_match(&expected_member, entry) {
                occupied.insert(number);
            }
        }
        Ok(occupied)
    }
}

fn canonical_numbered_candidate(
    parent: &Path,
    numbering_base: &Path,
    number: usize,
) -> Result<PathBuf, FormatError> {
    let candidate = numbered_create_destination(numbering_base, number);
    let name = candidate.file_name().ok_or_else(|| {
        FormatError::Unsupported("create destination candidate has no file name".into())
    })?;
    Ok(parent.join(name))
}

fn numbered_create_destination_index(name: &str) -> Option<usize> {
    let close = name.rfind(')')?;
    let suffix = name.get(close + 1..)?;
    if !suffix.is_empty() && !suffix.starts_with('.') {
        return None;
    }
    let open = name.get(..close)?.rfind(" (")?;
    let digits = name.get(open + 2..close)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<usize>().ok().filter(|number| *number >= 2)
}

fn snapshot_entry_matches(expected: &Path, observed: &Path) -> bool {
    if expected == observed {
        return true;
    }
    let (Some(expected_name), Some(observed_name)) = (expected.file_name(), observed.file_name())
    else {
        return false;
    };
    if !crate::entry_names_may_alias(expected_name, observed_name) {
        return false;
    }
    matches!(
        (path_identity(expected), path_identity(observed)),
        (Ok(expected), Ok(observed)) if expected == observed
    )
}

fn create_destination_name_parts(path: &Path) -> (String, Option<String>) {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower_name = name.to_ascii_lowercase();
    for extension in COMPOUND_CREATE_EXTENSIONS {
        let suffix = format!(".{extension}");
        if lower_name.ends_with(&suffix) && name.len() > suffix.len() {
            let stem_len = name.len().saturating_sub(suffix.len());
            return (
                name[..stem_len].to_owned(),
                Some(name[stem_len + 1..].to_owned()),
            );
        }
    }
    (
        path.file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path.extension()
            .map(|value| value.to_string_lossy().into_owned()),
    )
}

fn numbered_create_destination(path: &Path, number: usize) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let (stem, extension) = create_destination_name_parts(path);
    let name = match extension {
        Some(extension) if !extension.is_empty() => format!("{stem} ({number}).{extension}"),
        _ => format!("{stem} ({number})"),
    };
    parent.join(name)
}

/// Final publication policy for archive and SFX creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateCommitPolicy {
    /// Compatibility behavior for callers that explicitly rely on replacing
    /// whatever occupies the destination at commit time.
    ReplaceExisting,
    /// Publish only when the managed destination family is still absent.
    NoReplace,
    /// Replace only the exact destination state previously inspected.
    ReplaceIfUnchanged(CreateDestinationGuard),
}

/// Captures the existing destination the user is about to authorize for
/// replacement. Missing destinations return `conflict = false` and no guard.
pub fn inspect_create_destination(
    destination: &Path,
    kind: CreateArtifactKind,
) -> Result<CreateDestinationState, FormatError> {
    inspect_create_destination_with_progress(
        destination,
        kind,
        &NoProgress,
        &ControlToken::default(),
    )
}

/// Captures the existing destination while reporting the bytes read to bind
/// its contents. The total remains unknown because split families and bundle
/// trees can change while they are inspected.
pub fn inspect_create_destination_with_progress(
    destination: &Path,
    kind: CreateArtifactKind,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateDestinationState, FormatError> {
    ctl.checkpoint()?;
    let destination = normalized_destination(destination, kind)?;
    let canonical = canonical_requested_path(&destination)?;
    ctl.checkpoint()?;
    if kind != CreateArtifactKind::SplitArchive {
        match fs::symlink_metadata(&canonical) {
            Ok(metadata) => validate_inspected_artifact_type(&canonical, kind, &metadata)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CreateDestinationState {
                    conflict: false,
                    guard: None,
                });
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut digest_progress = DigestProgress::new(progress, ctl);
    let Some(state_digest) =
        artifact_state_digest_with_progress(&canonical, kind, &mut digest_progress)?
    else {
        return Ok(CreateDestinationState {
            conflict: false,
            guard: None,
        });
    };
    let path_digest = requested_path_digest(&canonical, kind);
    let mut bytes = [0u8; TOKEN_BYTES];
    bytes[0] = kind.tag();
    bytes[1..33].copy_from_slice(&path_digest);
    bytes[33..65].copy_from_slice(&state_digest);
    Ok(CreateDestinationState {
        conflict: true,
        guard: Some(CreateDestinationGuard { bytes }),
    })
}

fn validate_inspected_artifact_type(
    destination: &Path,
    kind: CreateArtifactKind,
    metadata: &Metadata,
) -> Result<(), FormatError> {
    let valid = match kind {
        CreateArtifactKind::Archive | CreateArtifactKind::SfxSingleFile => {
            metadata.file_type().is_file()
        }
        CreateArtifactKind::SfxMacosApp => metadata.file_type().is_dir(),
        CreateArtifactKind::SplitArchive => true,
    };
    if valid {
        return Ok(());
    }
    let expected = match kind {
        CreateArtifactKind::Archive => "archive file",
        CreateArtifactKind::SfxSingleFile => "single-file self-extractor",
        CreateArtifactKind::SfxMacosApp => "macOS app directory",
        CreateArtifactKind::SplitArchive => "split archive member",
    };
    Err(FormatError::Unsupported(format!(
        "existing destination is not a replaceable {expected}: {}",
        destination.display()
    )))
}

/// Verifies a replacement authorization against the currently managed output
/// family. This is only the pre-move check; commit transactions must retain
/// and revalidate the same content binding after moving the old output.
pub(crate) fn verify_destination_guard(
    destination: &Path,
    kind: CreateArtifactKind,
    guard: CreateDestinationGuard,
) -> Result<[u8; 32], FormatError> {
    verify_destination_guard_with_progress(
        destination,
        kind,
        guard,
        &NoProgress,
        &ControlToken::default(),
    )
}

/// Verifies only the artifact kind and canonical requested path encoded by a
/// replacement authorization. Callers must compare the returned state digest
/// with a fresh content snapshot before moving or replacing any output.
pub(crate) fn verify_destination_guard_binding(
    destination: &Path,
    kind: CreateArtifactKind,
    guard: CreateDestinationGuard,
) -> Result<[u8; 32], FormatError> {
    let destination = normalized_destination(destination, kind)?;
    let canonical = match canonical_requested_path(&destination) {
        Ok(canonical) => canonical,
        Err(FormatError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Err(FormatError::destination_changed(destination));
        }
        Err(error) => return Err(error),
    };
    if guard.kind() != kind || guard.path_digest() != requested_path_digest(&canonical, kind) {
        return Err(FormatError::destination_changed(destination));
    }
    Ok(guard.state_digest())
}

pub(crate) fn verify_destination_guard_with_progress(
    destination: &Path,
    kind: CreateArtifactKind,
    guard: CreateDestinationGuard,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<[u8; 32], FormatError> {
    ctl.checkpoint()?;
    let destination = normalized_destination(destination, kind)?;
    let canonical = match canonical_requested_path(&destination) {
        Ok(canonical) => canonical,
        Err(FormatError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Err(FormatError::destination_changed(destination));
        }
        Err(error) => return Err(error),
    };
    let mut digest_progress = DigestProgress::new(progress, ctl);
    let observed = match artifact_state_digest_with_progress(&canonical, kind, &mut digest_progress)
    {
        Ok(observed) => observed,
        Err(FormatError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            return Err(FormatError::destination_changed(destination));
        }
        Err(FormatError::Unsupported(_) | FormatError::ResourceLimitExceeded(_)) => {
            return Err(FormatError::destination_changed(destination));
        }
        Err(error) => return Err(error),
    };
    ctl.checkpoint()?;
    if guard.kind() != kind
        || guard.path_digest() != requested_path_digest(&canonical, kind)
        || observed != Some(guard.state_digest())
    {
        return Err(FormatError::destination_changed(destination));
    }
    Ok(guard.state_digest())
}

pub(crate) fn verify_moved_path_state(
    guard: CreateDestinationGuard,
    current_path: &Path,
    reported_destination: &Path,
) -> Result<(), FormatError> {
    if guarded_path_state_digest(current_path, reported_destination)? != Some(guard.state_digest())
    {
        return Err(FormatError::destination_changed(
            reported_destination.to_path_buf(),
        ));
    }
    Ok(())
}

pub(crate) fn verify_path_state_digest(
    expected: [u8; 32],
    current_path: &Path,
    reported_destination: &Path,
) -> Result<(), FormatError> {
    if guarded_path_state_digest(current_path, reported_destination)? != Some(expected) {
        return Err(FormatError::destination_changed(
            reported_destination.to_path_buf(),
        ));
    }
    Ok(())
}

fn guarded_path_state_digest(
    current_path: &Path,
    reported_destination: &Path,
) -> Result<Option<[u8; 32]>, FormatError> {
    match path_state_digest(current_path) {
        Ok(digest) => Ok(digest),
        Err(FormatError::Io(error))
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            Err(FormatError::destination_changed(
                reported_destination.to_path_buf(),
            ))
        }
        Err(FormatError::Unsupported(_) | FormatError::ResourceLimitExceeded(_)) => Err(
            FormatError::destination_changed(reported_destination.to_path_buf()),
        ),
        Err(error) => Err(error),
    }
}

pub(crate) fn path_state_digest(path: &Path) -> Result<Option<[u8; 32]>, FormatError> {
    let sink = NoProgress;
    let ctl = ControlToken::default();
    let mut progress = DigestProgress::new(&sink, &ctl);
    path_state_digest_with_progress(path, &mut progress)
}

fn path_state_digest_with_progress(
    path: &Path,
    progress: &mut DigestProgress<'_>,
) -> Result<Option<[u8; 32]>, FormatError> {
    progress.checkpoint()?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let stability_before = tree_stability_digest(path, progress)?;
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"squallz-destination-entry-v1\0");
            let mut budget = TreeDigestBudget::new(MAX_TREE_ENTRIES);
            hash_entry(path, OsStr::new(""), &mut hasher, 0, &mut budget, progress)?;
            let stability_after = tree_stability_digest(path, progress)?;
            if stability_after != stability_before {
                return Err(FormatError::destination_changed(path.to_path_buf()));
            }
            Ok(Some(*hasher.finalize().as_bytes()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn tree_stability_digest(
    path: &Path,
    progress: &mut DigestProgress<'_>,
) -> Result<[u8; 32], FormatError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-destination-tree-stability-v1\0");
    let mut budget = TreeDigestBudget::new(MAX_TREE_ENTRIES);
    hash_tree_stability_entry(path, OsStr::new(""), &mut hasher, 0, &mut budget, progress)?;
    progress.checkpoint()?;
    Ok(*hasher.finalize().as_bytes())
}

fn split_family_state_digest_from_paths_with_progress(
    members: &[(OsString, PathBuf)],
    progress: &mut DigestProgress<'_>,
) -> Result<[u8; 32], FormatError> {
    let mut ordered = members.to_vec();
    ordered.sort_unstable_by(|(left, _), (right, _)| compare_os_str(left, right));
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-destination-split-family-v1\0");
    hash_usize(&mut hasher, ordered.len());
    for (logical_name, current_path) in ordered {
        progress.checkpoint()?;
        hash_os_str(&mut hasher, &logical_name);
        let digest = path_state_digest_with_progress(&current_path, progress)?
            .ok_or_else(|| FormatError::destination_changed(current_path.to_path_buf()))?;
        hasher.update(&digest);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn artifact_state_digest_with_progress(
    destination: &Path,
    kind: CreateArtifactKind,
    progress: &mut DigestProgress<'_>,
) -> Result<Option<[u8; 32]>, FormatError> {
    progress.checkpoint()?;
    if kind != CreateArtifactKind::SplitArchive {
        return path_state_digest_with_progress(destination, progress);
    }
    let current = EntryPath::from_utf8(destination.to_string_lossy().into_owned());
    progress.enter(&current)?;
    let include_recovery = crate::volumes::is_sqz_base(destination);
    let managed = crate::volumes::collect_managed_split_outputs_with_checkpoint(
        destination,
        include_recovery,
        || progress.checkpoint(),
    )?;
    progress.checkpoint()?;
    if managed.is_empty() {
        return Ok(None);
    }
    let members = managed
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .ok_or_else(|| FormatError::Unsupported("split output has no file name".into()))?;
            Ok((name.to_os_string(), path))
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    split_family_state_digest_from_paths_with_progress(&members, progress).map(Some)
}

fn normalized_destination(
    destination: &Path,
    kind: CreateArtifactKind,
) -> Result<PathBuf, FormatError> {
    if kind != CreateArtifactKind::SplitArchive {
        return Ok(destination.to_path_buf());
    }
    let name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| FormatError::Unsupported("invalid split output file name".into()))?;
    Ok(match split_volume_name(name) {
        Some((base, _)) => destination.with_file_name(base),
        None => destination.to_path_buf(),
    })
}

fn canonical_requested_path(path: &Path) -> Result<PathBuf, FormatError> {
    let name = path
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("destination path has no file name".into()))?;
    Ok(fs::canonicalize(parent_or_current(path))?.join(name))
}

fn requested_path_digest(path: &Path, kind: CreateArtifactKind) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-create-destination-path-v1\0");
    hasher.update(&[kind.tag()]);
    hash_os_str(&mut hasher, path.as_os_str());
    *hasher.finalize().as_bytes()
}

struct TreeDigestBudget {
    visited_entries: usize,
    pending_entries: usize,
    max_entries: usize,
}

impl TreeDigestBudget {
    fn new(max_entries: usize) -> Self {
        Self {
            visited_entries: 0,
            pending_entries: 0,
            max_entries,
        }
    }

    fn enter(&mut self) -> Result<(), FormatError> {
        self.visited_entries = self.visited_entries.saturating_add(1);
        if self.visited_entries > self.max_entries {
            return Err(self.limit_error());
        }
        Ok(())
    }

    fn remaining_collection_capacity(&self) -> usize {
        self.max_entries
            .saturating_sub(self.visited_entries)
            .min(self.max_entries.saturating_sub(self.pending_entries))
    }

    fn reserve_pending(&mut self, entries: usize) -> Result<(), FormatError> {
        self.pending_entries = self.pending_entries.checked_add(entries).ok_or_else(|| {
            FormatError::ResourceLimitExceeded(
                "destination tree pending-entry budget overflowed".into(),
            )
        })?;
        if self.pending_entries > self.max_entries {
            return Err(self.limit_error());
        }
        Ok(())
    }

    fn release_pending(&mut self, entries: usize) {
        self.pending_entries = self.pending_entries.saturating_sub(entries);
    }

    fn limit_error(&self) -> FormatError {
        FormatError::ResourceLimitExceeded(format!(
            "destination tree exceeds {} entries",
            self.max_entries
        ))
    }
}

fn hash_entry(
    path: &Path,
    relative: &OsStr,
    hasher: &mut blake3::Hasher,
    depth: usize,
    budget: &mut TreeDigestBudget,
    progress: &mut DigestProgress<'_>,
) -> Result<(), FormatError> {
    let current = EntryPath::from_utf8(path.to_string_lossy().into_owned());
    progress.enter(&current)?;
    if depth > MAX_TREE_DEPTH {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "destination tree exceeds {MAX_TREE_DEPTH} levels"
        )));
    }
    budget.enter()?;

    let metadata = fs::symlink_metadata(path)?;
    let identity = path_identity(path)?;
    #[cfg(windows)]
    let windows_change_time = path_change_time(path)?;
    hash_os_str(hasher, relative);
    if metadata.file_type().is_symlink() {
        hasher.update(&[3]);
        hash_identity(hasher, identity);
        hash_stable_metadata(hasher, &metadata);
        let target = fs::read_link(path)?;
        hash_os_str(hasher, target.as_os_str());
        progress.checkpoint()?;
        let after = fs::symlink_metadata(path)?;
        #[cfg(windows)]
        let change_time_changed = path_change_time(path)? != windows_change_time;
        #[cfg(not(windows))]
        let change_time_changed = false;
        if path_identity(path)? != identity
            || !after.file_type().is_symlink()
            || stable_metadata_digest(&after) != stable_metadata_digest(&metadata)
            || fs::read_link(path)? != target
            || change_time_changed
        {
            return Err(FormatError::destination_changed(path.to_path_buf()));
        }
        return Ok(());
    }
    if metadata.is_file() {
        hasher.update(&[1]);
        hash_regular_file(path, identity, &metadata, hasher, &current, progress)?;
        #[cfg(windows)]
        if path_change_time(path)? != windows_change_time {
            return Err(FormatError::destination_changed(path.to_path_buf()));
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(FormatError::Unsupported(format!(
            "destination contains an unsupported filesystem entry: {}",
            path.display()
        )));
    }

    hasher.update(&[2]);
    hash_identity(hasher, identity);
    hash_stable_metadata(hasher, &metadata);
    let stable_state = directory_stability_digest(&metadata);
    let mut children = Vec::new();
    let remaining_entries = budget.remaining_collection_capacity();
    for entry in fs::read_dir(path)? {
        progress.checkpoint()?;
        let entry = entry?;
        if children.len() >= remaining_entries {
            return Err(budget.limit_error());
        }
        let child = entry.path();
        let state = directory_member_state(entry.file_name(), &child)?;
        children.push((state, child));
    }
    let reserved_children = children.len();
    budget.reserve_pending(reserved_children)?;
    children.sort_unstable_by(|(left, _), (right, _)| compare_os_str(&left.name, &right.name));
    progress.checkpoint()?;
    let result = (|| {
        hash_usize(hasher, children.len());
        for (state, child) in &children {
            let child_relative = append_relative(relative, &state.name);
            hash_entry(child, &child_relative, hasher, depth + 1, budget, progress)?;
        }
        progress.checkpoint()?;
        let mut seen = Vec::new();
        seen.try_reserve_exact(children.len())
            .map_err(|_| budget.limit_error())?;
        seen.resize(children.len(), false);
        for entry in fs::read_dir(path)? {
            progress.checkpoint()?;
            let entry = entry?;
            let child = entry.path();
            let observed = directory_member_state(entry.file_name(), &child)?;
            let Ok(index) = children
                .binary_search_by(|(expected, _)| compare_os_str(&expected.name, &observed.name))
            else {
                return Err(FormatError::destination_changed(path.to_path_buf()));
            };
            if seen[index] || children[index].0 != observed {
                return Err(FormatError::destination_changed(path.to_path_buf()));
            }
            seen[index] = true;
        }
        progress.checkpoint()?;
        let after = fs::symlink_metadata(path)?;
        #[cfg(windows)]
        let change_time_changed = path_change_time(path)? != windows_change_time;
        #[cfg(not(windows))]
        let change_time_changed = false;
        if path_identity(path)? != identity
            || !after.is_dir()
            || directory_stability_digest(&after) != stable_state
            || seen.iter().any(|observed| !observed)
            || change_time_changed
        {
            return Err(FormatError::destination_changed(path.to_path_buf()));
        }
        progress.checkpoint()?;
        Ok(())
    })();
    budget.release_pending(reserved_children);
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryMemberState {
    name: OsString,
    identity: PathIdentity,
    entry_type: DirectoryMemberType,
    regular_state: Option<RegularFileState>,
    metadata_digest: [u8; 32],
    symlink_target: Option<PathBuf>,
    #[cfg(windows)]
    windows_change_time: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectoryMemberType {
    File,
    Directory,
    Symlink,
    Other,
}

fn directory_member_state(
    name: OsString,
    path: &Path,
) -> Result<DirectoryMemberState, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    let identity = path_identity(path)?;
    let file_type = metadata.file_type();
    let entry_type = if file_type.is_symlink() {
        DirectoryMemberType::Symlink
    } else if file_type.is_file() {
        DirectoryMemberType::File
    } else if file_type.is_dir() {
        DirectoryMemberType::Directory
    } else {
        DirectoryMemberType::Other
    };
    let regular_state = metadata
        .is_file()
        .then(|| RegularFileState::from_metadata(&metadata));
    let metadata_digest = directory_stability_digest(&metadata);
    let symlink_target = file_type
        .is_symlink()
        .then(|| fs::read_link(path))
        .transpose()?;
    let state = DirectoryMemberState {
        name,
        identity,
        entry_type,
        regular_state,
        metadata_digest,
        symlink_target,
        #[cfg(windows)]
        windows_change_time: path_change_time(path)?,
    };
    if !state.matches_path(path)? {
        return Err(FormatError::destination_changed(path.to_path_buf()));
    }
    Ok(state)
}

impl DirectoryMemberState {
    fn update_stability_digest(&self, hasher: &mut blake3::Hasher) {
        hash_os_str(hasher, &self.name);
        hasher.update(&[self.entry_type.tag()]);
        hash_identity(hasher, self.identity);
        hasher.update(&self.metadata_digest);
        match &self.symlink_target {
            Some(target) => {
                hasher.update(&[1]);
                hash_os_str(hasher, target.as_os_str());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        #[cfg(windows)]
        hasher.update(&self.windows_change_time.to_le_bytes());
    }

    fn matches_path(&self, path: &Path) -> Result<bool, FormatError> {
        let after = fs::symlink_metadata(path)?;
        let symlink_target_matches = match &self.symlink_target {
            Some(expected) => fs::read_link(path)? == *expected,
            None => true,
        };
        #[cfg(windows)]
        let change_time_matches = path_change_time(path)? == self.windows_change_time;
        #[cfg(not(windows))]
        let change_time_matches = true;
        Ok(path_identity(path)? == self.identity
            && directory_member_type(&after) == self.entry_type
            && directory_stability_digest(&after) == self.metadata_digest
            && self
                .regular_state
                .as_ref()
                .is_none_or(|expected| expected.matches(&after))
            && symlink_target_matches
            && change_time_matches)
    }
}

impl DirectoryMemberType {
    fn tag(self) -> u8 {
        match self {
            Self::File => 1,
            Self::Directory => 2,
            Self::Symlink => 3,
            Self::Other => 4,
        }
    }
}

fn hash_tree_stability_entry(
    path: &Path,
    relative: &OsStr,
    hasher: &mut blake3::Hasher,
    depth: usize,
    budget: &mut TreeDigestBudget,
    progress: &mut DigestProgress<'_>,
) -> Result<(), FormatError> {
    let current = EntryPath::from_utf8(path.to_string_lossy().into_owned());
    progress.enter(&current)?;
    if depth > MAX_TREE_DEPTH {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "destination tree exceeds {MAX_TREE_DEPTH} levels"
        )));
    }
    budget.enter()?;

    let state = directory_member_state(relative.to_os_string(), path)?;
    state.update_stability_digest(hasher);
    if state.entry_type != DirectoryMemberType::Directory {
        return Ok(());
    }

    let mut children = Vec::new();
    let remaining_entries = budget.remaining_collection_capacity();
    for entry in fs::read_dir(path)? {
        progress.checkpoint()?;
        let entry = entry?;
        if children.len() >= remaining_entries {
            return Err(budget.limit_error());
        }
        children.push((entry.file_name(), entry.path()));
    }
    let reserved_children = children.len();
    budget.reserve_pending(reserved_children)?;
    children.sort_unstable_by(|(left, _), (right, _)| compare_os_str(left, right));
    progress.checkpoint()?;
    let result = (|| {
        hash_usize(hasher, children.len());
        for (name, child) in &children {
            let child_relative = append_relative(relative, name);
            hash_tree_stability_entry(child, &child_relative, hasher, depth + 1, budget, progress)?;
        }
        progress.checkpoint()?;
        if !state.matches_path(path)? {
            return Err(FormatError::destination_changed(path.to_path_buf()));
        }
        Ok(())
    })();
    budget.release_pending(reserved_children);
    result
}

fn directory_member_type(metadata: &Metadata) -> DirectoryMemberType {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        DirectoryMemberType::Symlink
    } else if file_type.is_file() {
        DirectoryMemberType::File
    } else if file_type.is_dir() {
        DirectoryMemberType::Directory
    } else {
        DirectoryMemberType::Other
    }
}

fn hash_regular_file(
    path: &Path,
    expected_identity: PathIdentity,
    path_metadata: &Metadata,
    hasher: &mut blake3::Hasher,
    current: &EntryPath,
    progress: &mut DigestProgress<'_>,
) -> Result<(), FormatError> {
    let mut file = open_regular_file_no_follow(path)?;
    let identity = file_identity(&file)?;
    let metadata = file.metadata()?;
    if identity != expected_identity || !metadata.is_file() {
        return Err(FormatError::destination_changed(path.to_path_buf()));
    }
    let state = RegularFileState::from_metadata(&metadata);
    hash_identity(hasher, identity);
    hash_stable_metadata(hasher, path_metadata);
    let mut buffer = vec![0u8; DIGEST_BUFFER_BYTES];
    loop {
        progress.checkpoint()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        progress.advance(read, current)?;
    }
    progress.checkpoint()?;
    let after = file.metadata()?;
    if file_identity(&file)? != identity
        || path_identity(path)? != identity
        || !state.matches(&after)
        || stable_metadata_digest(&after) != stable_metadata_digest(path_metadata)
    {
        return Err(FormatError::destination_changed(path.to_path_buf()));
    }
    Ok(())
}

struct DigestProgress<'a> {
    sink: &'a dyn ProgressSink,
    ctl: &'a ControlToken,
    bytes_read: u64,
}

impl<'a> DigestProgress<'a> {
    fn new(sink: &'a dyn ProgressSink, ctl: &'a ControlToken) -> Self {
        Self {
            sink,
            ctl,
            bytes_read: 0,
        }
    }

    fn checkpoint(&self) -> Result<(), FormatError> {
        self.ctl.checkpoint()
    }

    fn enter(&self, current: &EntryPath) -> Result<(), FormatError> {
        self.checkpoint()?;
        self.sink.on_progress(self.bytes_read, 0, current);
        self.checkpoint()
    }

    fn advance(&mut self, bytes: usize, current: &EntryPath) -> Result<(), FormatError> {
        self.bytes_read = self.bytes_read.checked_add(bytes as u64).ok_or_else(|| {
            FormatError::ResourceLimitExceeded(
                "destination contents exceed the supported progress range".into(),
            )
        })?;
        self.sink.on_progress(self.bytes_read, 0, current);
        self.checkpoint()
    }
}

fn hash_identity(hasher: &mut blake3::Hasher, identity: PathIdentity) {
    identity.update_digest(hasher);
}

fn hash_stable_metadata(hasher: &mut blake3::Hasher, metadata: &Metadata) {
    hasher.update(&metadata.len().to_le_bytes());
    hasher.update(&[u8::from(metadata.permissions().readonly())]);
    hash_modified(hasher, metadata.modified().ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        hasher.update(&metadata.mode().to_le_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        hasher.update(&metadata.file_attributes().to_le_bytes());
    }
}

fn stable_metadata_digest(metadata: &Metadata) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_stable_metadata(&mut hasher, metadata);
    *hasher.finalize().as_bytes()
}

fn directory_stability_digest(metadata: &Metadata) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_stable_metadata(&mut hasher, metadata);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        hasher.update(&metadata.ctime().to_le_bytes());
        hasher.update(&metadata.ctime_nsec().to_le_bytes());
    }
    *hasher.finalize().as_bytes()
}

fn hash_modified(hasher: &mut blake3::Hasher, modified: Option<std::time::SystemTime>) {
    use std::time::UNIX_EPOCH;

    match modified {
        None => {
            hasher.update(&[0]);
        }
        Some(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => {
                hasher.update(&[1]);
                hasher.update(&duration.as_secs().to_le_bytes());
                hasher.update(&duration.subsec_nanos().to_le_bytes());
            }
            Err(error) => {
                let duration = error.duration();
                hasher.update(&[2]);
                hasher.update(&duration.as_secs().to_le_bytes());
                hasher.update(&duration.subsec_nanos().to_le_bytes());
            }
        },
    };
}

fn append_relative(parent: &OsStr, name: &OsStr) -> OsString {
    if parent.is_empty() {
        return name.to_os_string();
    }
    let mut path = PathBuf::from(parent);
    path.push(name);
    path.into_os_string()
}

fn hash_usize(hasher: &mut blake3::Hasher, value: usize) {
    hasher.update(&(value as u64).to_le_bytes());
}

fn hash_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    let bytes = os_sort_key(value);
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
}

#[cfg(unix)]
fn compare_os_str(left: &OsStr, right: &OsStr) -> Ordering {
    use std::os::unix::ffi::OsStrExt;

    left.as_bytes().cmp(right.as_bytes())
}

#[cfg(windows)]
fn compare_os_str(left: &OsStr, right: &OsStr) -> Ordering {
    use std::os::windows::ffi::OsStrExt;

    left.encode_wide().cmp(right.encode_wide())
}

#[cfg(not(any(unix, windows)))]
fn compare_os_str(left: &OsStr, right: &OsStr) -> Ordering {
    left.to_string_lossy().cmp(&right.to_string_lossy())
}

#[cfg(unix)]
fn os_sort_key(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_sort_key(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
fn os_sort_key(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

fn parent_or_current(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn encode_guard(bytes: [u8; TOKEN_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
    encoded.push_str(TOKEN_PREFIX);
    for byte in bytes {
        encoded.push(char::from(HEX[(byte >> 4) as usize]));
        encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    encoded
}

fn decode_guard(value: &str) -> Result<CreateDestinationGuard, &'static str> {
    let Some(hex) = value.strip_prefix(TOKEN_PREFIX) else {
        return Err("unsupported create destination guard version");
    };
    if hex.len() != TOKEN_BYTES * 2 {
        return Err("invalid create destination guard length");
    }
    let mut bytes = [0u8; TOKEN_BYTES];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_nibble(pair[0]).ok_or("invalid create destination guard encoding")?;
        let low = decode_nibble(pair[1]).ok_or("invalid create destination guard encoding")?;
        bytes[index] = (high << 4) | low;
    }
    if CreateArtifactKind::from_tag(bytes[0]).is_none() {
        return Err("invalid create destination guard artifact kind");
    }
    Ok(CreateDestinationGuard { bytes })
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

    struct CancelAfterFirstRead {
        ctl: Arc<ControlToken>,
        first_read: AtomicU64,
    }

    impl ProgressSink for CancelAfterFirstRead {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > 0
                && self
                    .first_read
                    .compare_exchange(0, done, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                self.ctl.cancel();
            }
        }
    }

    #[cfg(unix)]
    struct RebindSiblingOnTrigger {
        root: PathBuf,
        victim: PathBuf,
        replacement: PathBuf,
        retired: PathBuf,
        trigger: PathBuf,
        root_modified: std::time::SystemTime,
        fired: AtomicBool,
    }

    #[cfg(unix)]
    impl ProgressSink for RebindSiblingOnTrigger {
        fn on_progress(&self, done: u64, _total: u64, current: &EntryPath) {
            if done == 0
                || current.display != self.trigger.to_string_lossy()
                || self.fired.swap(true, Ordering::Relaxed)
            {
                return;
            }
            fs::rename(&self.victim, &self.retired).unwrap();
            fs::rename(&self.replacement, &self.victim).unwrap();
            fs::File::open(&self.root)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(self.root_modified))
                .unwrap();
        }
    }

    struct RewriteSiblingOnTrigger {
        victim: PathBuf,
        trigger: PathBuf,
        victim_modified: std::time::SystemTime,
        fired: AtomicBool,
    }

    impl ProgressSink for RewriteSiblingOnTrigger {
        fn on_progress(&self, done: u64, _total: u64, current: &EntryPath) {
            if done == 0
                || current.display != self.trigger.to_string_lossy()
                || self.fired.swap(true, Ordering::Relaxed)
            {
                return;
            }
            fs::write(&self.victim, b"changed").unwrap();
            fs::File::open(&self.victim)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(self.victim_modified))
                .unwrap();
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "squallz-destination-guard-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(unix)]
    fn change_ctime_without_changing_mode(path: &Path, before: &Metadata) -> Metadata {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::time::Duration;

        let original_mode = before.permissions().mode();
        let original_ctime = (before.ctime(), before.ctime_nsec());
        let mut after = fs::symlink_metadata(path).unwrap();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(5));
            let mut changed = before.permissions();
            changed.set_mode(original_mode ^ 0o100);
            fs::set_permissions(path, changed).unwrap();
            let mut restored = before.permissions();
            restored.set_mode(original_mode);
            fs::set_permissions(path, restored).unwrap();
            after = fs::symlink_metadata(path).unwrap();
            if (after.ctime(), after.ctime_nsec()) != original_ctime {
                break;
            }
        }
        after
    }

    #[test]
    fn directory_snapshot_rejects_entries_beyond_its_resource_limit() {
        let paths =
            (0..=MAX_DIRECTORY_SNAPSHOT_ENTRIES).map(|_| Ok::<_, FormatError>(PathBuf::new()));

        let error = DirectorySnapshot::from_paths(paths).unwrap_err();

        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
    }

    #[test]
    fn wide_directory_hits_the_tree_limit_before_hashing_a_child() {
        let dir = temp_dir("wide-tree-limit");
        fs::write(dir.join("first"), b"first").unwrap();
        fs::write(dir.join("second"), b"second").unwrap();
        let mut hasher = blake3::Hasher::new();
        let mut budget = TreeDigestBudget::new(2);
        let sink = NoProgress;
        let ctl = ControlToken::default();
        let mut progress = DigestProgress::new(&sink, &ctl);

        let error = hash_entry(
            &dir,
            OsStr::new(""),
            &mut hasher,
            0,
            &mut budget,
            &mut progress,
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
        assert_eq!(budget.visited_entries, 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nested_directory_reservations_share_one_pending_entry_budget() {
        let dir = temp_dir("nested-pending-budget");
        let first = dir.join("a-directory");
        fs::create_dir(&first).unwrap();
        fs::write(first.join("first"), b"first").unwrap();
        fs::write(first.join("second"), b"second").unwrap();
        fs::write(dir.join("y-file"), b"y").unwrap();
        fs::write(dir.join("z-file"), b"z").unwrap();
        let mut hasher = blake3::Hasher::new();
        let mut budget = TreeDigestBudget::new(4);
        let sink = NoProgress;
        let ctl = ControlToken::default();
        let mut progress = DigestProgress::new(&sink, &ctl);

        let error = hash_entry(
            &dir,
            OsStr::new(""),
            &mut hasher,
            0,
            &mut budget,
            &mut progress,
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::ResourceLimitExceeded(_)));
        assert_eq!(budget.visited_entries, 2);
        assert_eq!(budget.pending_entries, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_member_state_detects_same_name_rebinding() {
        let dir = temp_dir("member-rebinding");
        let member = dir.join("payload.zip");
        let retired = dir.join("retired.zip");
        fs::write(&member, b"payload").unwrap();
        let before = directory_member_state(OsString::from("payload.zip"), &member).unwrap();
        fs::rename(&member, &retired).unwrap();
        fs::write(&member, b"payload").unwrap();

        let after = directory_member_state(OsString::from("payload.zip"), &member).unwrap();

        assert_eq!(before.name, after.name);
        assert_eq!(before.entry_type, after.entry_type);
        assert_ne!(before.identity, after.identity);
        assert_ne!(before, after);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn content_digest_survives_publication_rename() {
        let dir = temp_dir("digest-after-rename");
        let staged = dir.join("staged.app");
        let published = dir.join("published.app");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("payload.bin"), b"payload").unwrap();
        let before = path_state_digest(&staged).unwrap();

        fs::rename(&staged, &published).unwrap();

        assert_eq!(path_state_digest(&published).unwrap(), before);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tree_digest_rejects_a_previously_hashed_member_rebound_during_scan() {
        let dir = temp_dir("member-rebound-during-scan");
        let root = dir.join("Archive.app");
        let victim = root.join("a-payload.zip");
        let trigger = root.join("z-trigger.bin");
        let replacement = dir.join("replacement.zip");
        let retired = dir.join("retired.zip");
        fs::create_dir(&root).unwrap();
        fs::write(&victim, b"payload").unwrap();
        fs::write(&trigger, b"trigger").unwrap();
        fs::write(&replacement, b"payload").unwrap();
        let progress = RebindSiblingOnTrigger {
            root: root.clone(),
            victim,
            replacement,
            retired,
            trigger,
            root_modified: fs::metadata(&root).unwrap().modified().unwrap(),
            fired: AtomicBool::new(false),
        };
        let ctl = ControlToken::default();
        let mut digest_progress = DigestProgress::new(&progress, &ctl);

        let error = path_state_digest_with_progress(&root, &mut digest_progress).unwrap_err();

        assert!(progress.fired.load(Ordering::Relaxed));
        assert_eq!(error.destination_changed_path(), Some(root.as_path()));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tree_digest_rejects_a_previously_hashed_member_rewritten_during_scan() {
        let dir = temp_dir("member-rewritten-during-scan");
        let root = dir.join("Archive.app");
        let victim = root.join("a-payload.zip");
        let trigger = root.join("z-trigger.bin");
        fs::create_dir(&root).unwrap();
        fs::write(&victim, b"payload").unwrap();
        fs::write(&trigger, b"trigger").unwrap();
        let progress = RewriteSiblingOnTrigger {
            victim: victim.clone(),
            trigger,
            victim_modified: fs::metadata(&victim).unwrap().modified().unwrap(),
            fired: AtomicBool::new(false),
        };
        let ctl = ControlToken::default();
        let mut digest_progress = DigestProgress::new(&progress, &ctl);

        let error = path_state_digest_with_progress(&root, &mut digest_progress).unwrap_err();

        assert!(progress.fired.load(Ordering::Relaxed));
        assert_eq!(error.destination_changed_path(), Some(root.as_path()));
        assert_eq!(fs::read(victim).unwrap(), b"changed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn tree_digest_rejects_a_deep_member_rewritten_after_its_subtree_scan() {
        let dir = temp_dir("deep-member-rewritten-during-scan");
        let root = dir.join("Archive.app");
        let nested = root.join("a-directory");
        let victim = nested.join("a-payload.zip");
        let trigger = root.join("z-trigger.bin");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&victim, b"payload").unwrap();
        fs::write(&trigger, b"trigger").unwrap();
        let progress = RewriteSiblingOnTrigger {
            victim: victim.clone(),
            trigger,
            victim_modified: fs::metadata(&victim).unwrap().modified().unwrap(),
            fired: AtomicBool::new(false),
        };
        let ctl = ControlToken::default();
        let mut digest_progress = DigestProgress::new(&progress, &ctl);

        let error = path_state_digest_with_progress(&root, &mut digest_progress).unwrap_err();

        assert!(progress.fired.load(Ordering::Relaxed));
        assert_eq!(error.destination_changed_path(), Some(root.as_path()));
        assert_eq!(fs::read(victim).unwrap(), b"changed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_ctime_is_part_of_file_and_directory_stability() {
        use std::os::unix::fs::MetadataExt;

        let dir = temp_dir("ctime-stability");
        let file = dir.join("payload.zip");
        fs::write(&file, b"payload").unwrap();
        let directory_before = fs::symlink_metadata(&dir).unwrap();
        let file_before = fs::symlink_metadata(&file).unwrap();
        let file_state = RegularFileState::from_metadata(&file_before);

        let directory_after = change_ctime_without_changing_mode(&dir, &directory_before);
        let file_after = change_ctime_without_changing_mode(&file, &file_before);

        assert_ne!(
            (directory_before.ctime(), directory_before.ctime_nsec()),
            (directory_after.ctime(), directory_after.ctime_nsec())
        );
        assert_eq!(
            stable_metadata_digest(&directory_before),
            stable_metadata_digest(&directory_after)
        );
        assert_ne!(
            directory_stability_digest(&directory_before),
            directory_stability_digest(&directory_after)
        );
        assert_ne!(
            (file_before.ctime(), file_before.ctime_nsec()),
            (file_after.ctime(), file_after.ctime_nsec())
        );
        assert!(!file_state.matches(&file_after));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn available_destination_reads_one_snapshot_for_dense_split_families() {
        let dir = temp_dir("dense-available-name");
        let proposed = dir.join("archive.sqz");
        fs::write(&proposed, b"base").unwrap();
        for number in 2..=1024 {
            let candidate = numbered_create_destination(&proposed, number);
            let name = candidate.file_name().unwrap().to_string_lossy();
            let member = if number % 2 == 0 {
                candidate.with_file_name(format!("{name}.001"))
            } else {
                candidate.with_file_name(format!("{name}.rev001"))
            };
            fs::write(member, b"occupied").unwrap();
        }

        let enumerations = Cell::new(0_usize);
        let comparisons = Cell::new(0_usize);
        let available = find_available_create_destination_with_match(
            &proposed,
            CreateArtifactKind::SplitArchive,
            |parent| {
                enumerations.set(enumerations.get() + 1);
                DirectorySnapshot::read(parent)
            },
            |expected, observed| {
                comparisons.set(comparisons.get() + 1);
                snapshot_entry_matches(expected, observed)
            },
        )
        .unwrap();

        assert_eq!(enumerations.get(), 1);
        assert!(comparisons.get() <= 4 * 1024);
        assert_eq!(available, dir.join("archive (1025).sqz"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn available_destination_matches_split_sidecars_and_case_aliases() {
        let dir = temp_dir("available-name-aliases");
        let zip = dir.join("archive.zip");
        fs::write(dir.join("archive.zip.rev001"), b"not managed for zip").unwrap();
        assert_eq!(
            find_available_create_destination(&zip, CreateArtifactKind::SplitArchive).unwrap(),
            zip
        );

        let sqz = dir.join("backup.sqz");
        fs::write(dir.join("backup.sqz.rev001"), b"managed for sqz").unwrap();
        assert_eq!(
            find_available_create_destination(&sqz, CreateArtifactKind::SplitArchive).unwrap(),
            dir.join("backup (2).sqz")
        );

        let mixed_case = dir.join("Mixed.ZIP");
        let observed = dir.join("mixed.zip.001");
        fs::write(&observed, b"case alias").unwrap();
        let filesystem_aliases_case = fs::symlink_metadata(dir.join("MIXED.ZIP.001")).is_ok();
        let expected = if filesystem_aliases_case {
            dir.join("Mixed (2).ZIP")
        } else {
            mixed_case.clone()
        };
        assert_eq!(
            find_available_create_destination(&mixed_case, CreateArtifactKind::SplitArchive)
                .unwrap(),
            expected
        );

        let explicit_first = dir.join("explicit.zip.001");
        fs::write(&explicit_first, b"occupied first volume").unwrap();
        assert_eq!(
            find_available_create_destination(&explicit_first, CreateArtifactKind::SplitArchive,)
                .unwrap(),
            dir.join("explicit (2).zip")
        );

        let abnormal = dir.join("abnormal.zip");
        fs::create_dir(dir.join("abnormal.zip.001")).unwrap();
        assert_eq!(
            find_available_create_destination(&abnormal, CreateArtifactKind::SplitArchive).unwrap(),
            dir.join("abnormal (2).zip")
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guard_round_trips_as_one_fixed_size_redacted_value() {
        let dir = temp_dir("round-trip");
        let target = dir.join("archive.zip");
        fs::write(&target, b"old archive").unwrap();
        let guard = inspect_create_destination(&target, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();

        let json = serde_json::to_string(&guard).unwrap();
        assert_eq!(json.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2 + 2);
        assert_eq!(
            serde_json::from_str::<CreateDestinationGuard>(&json).unwrap(),
            guard
        );
        assert_eq!(format!("{guard:?}"), "CreateDestinationGuard([redacted])");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inspection_rejects_an_existing_directory_for_a_file_artifact() {
        let dir = temp_dir("directory-file-artifact");
        let target = dir.join("archive.zip");
        fs::create_dir(&target).unwrap();

        let error = inspect_create_destination(&target, CreateArtifactKind::Archive).unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inspection_rejects_an_existing_symlink_for_a_file_artifact() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink-file-artifact");
        let referent = dir.join("referent.zip");
        let target = dir.join("archive.zip");
        fs::write(&referent, b"referent").unwrap();
        symlink(&referent, &target).unwrap();

        let error = inspect_create_destination(&target, CreateArtifactKind::Archive).unwrap_err();

        assert!(matches!(error, FormatError::Unsupported(_)));
        assert_eq!(fs::read(&referent).unwrap(), b"referent");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progressive_inspection_reports_real_split_family_bytes() {
        let dir = temp_dir("split-progress");
        let base = dir.join("archive.zip");
        let first = base.with_file_name("archive.zip.001");
        let second = base.with_file_name("archive.zip.002");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"second").unwrap();
        let progress = RecordingProgress::default();

        let state = inspect_create_destination_with_progress(
            &base,
            CreateArtifactKind::SplitArchive,
            &progress,
            &ControlToken::default(),
        )
        .unwrap();

        assert!(state.conflict);
        assert!(state.guard.is_some());
        let events = progress.events.lock().unwrap();
        assert!(events.iter().all(|(_, total, _)| *total == 0));
        assert!(events.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(events.last().map(|event| event.0), Some(9));
        assert!(events
            .iter()
            .any(|event| event.2.ends_with("archive.zip.001")));
        assert!(events
            .iter()
            .any(|event| event.2.ends_with("archive.zip.002")));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progressive_inspection_reports_the_current_bundle_member() {
        let dir = temp_dir("bundle-progress");
        let app = dir.join("Archive.app");
        let resources = app.join("Contents/Resources");
        fs::create_dir_all(&resources).unwrap();
        fs::write(resources.join("payload.zip"), b"payload").unwrap();
        let progress = RecordingProgress::default();

        inspect_create_destination_with_progress(
            &app,
            CreateArtifactKind::SfxMacosApp,
            &progress,
            &ControlToken::default(),
        )
        .unwrap();

        let events = progress.events.lock().unwrap();
        assert_eq!(events.last().map(|event| event.0), Some(7));
        assert!(events
            .iter()
            .any(|event| event.2.ends_with("Contents/Resources/payload.zip")));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn progressive_inspection_cancels_at_a_file_chunk_boundary() {
        let dir = temp_dir("cancel");
        let target = dir.join("archive.zip");
        fs::write(&target, vec![0x5a; DIGEST_BUFFER_BYTES * 2]).unwrap();
        let ctl = ControlToken::new();
        let progress = CancelAfterFirstRead {
            ctl: Arc::clone(&ctl),
            first_read: AtomicU64::new(0),
        };

        let error = inspect_create_destination_with_progress(
            &target,
            CreateArtifactKind::Archive,
            &progress,
            &ctl,
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(
            progress.first_read.load(Ordering::Relaxed),
            DIGEST_BUFFER_BYTES as u64
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guard_rejects_same_length_content_changes() {
        let dir = temp_dir("content-change");
        let target = dir.join("archive.zip");
        fs::write(&target, b"first bytes").unwrap();
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        let guard = inspect_create_destination(&target, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        fs::write(&target, b"other bytes").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();

        let error =
            verify_destination_guard(&target, CreateArtifactKind::Archive, guard).unwrap_err();
        assert_eq!(error.destination_changed_path(), Some(target.as_path()));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guard_binding_checks_path_and_kind_without_reading_artifact_state() {
        let dir = temp_dir("binding-only");
        let target = dir.join("archive.zip");
        fs::write(&target, b"old archive").unwrap();
        let guard = inspect_create_destination(&target, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        fs::remove_file(&target).unwrap();

        assert_eq!(
            verify_destination_guard_binding(&target, CreateArtifactKind::Archive, guard).unwrap(),
            guard.state_digest()
        );
        assert!(verify_destination_guard_binding(
            &target,
            CreateArtifactKind::SfxSingleFile,
            guard
        )
        .unwrap_err()
        .is_destination_changed());
        assert!(verify_destination_guard_binding(
            &dir.join("other.zip"),
            CreateArtifactKind::Archive,
            guard
        )
        .unwrap_err()
        .is_destination_changed());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn guard_reports_a_removed_destination_parent_as_stale() {
        let dir = temp_dir("removed-parent");
        let target = dir.join("archive.zip");
        fs::write(&target, b"old archive").unwrap();
        let guard = inspect_create_destination(&target, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        fs::remove_dir_all(&dir).unwrap();

        let error =
            verify_destination_guard(&target, CreateArtifactKind::Archive, guard).unwrap_err();

        assert!(error.is_destination_changed());
        assert_eq!(error.destination_changed_path(), Some(target.as_path()));
    }

    #[test]
    fn split_guard_covers_the_complete_managed_family() {
        let dir = temp_dir("split-family");
        let base = dir.join("archive.zip");
        fs::write(base.with_file_name("archive.zip.001"), b"one").unwrap();
        let guard = inspect_create_destination(
            &base.with_file_name("archive.zip.001"),
            CreateArtifactKind::SplitArchive,
        )
        .unwrap()
        .guard
        .unwrap();
        fs::write(base.with_file_name("archive.zip.002"), b"two").unwrap();

        assert!(
            verify_destination_guard(&base, CreateArtifactKind::SplitArchive, guard)
                .unwrap_err()
                .is_destination_changed()
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn app_guard_covers_nested_files() {
        let dir = temp_dir("app-tree");
        let app = dir.join("Archive.app");
        let resources = app.join("Contents/Resources");
        fs::create_dir_all(&resources).unwrap();
        fs::write(resources.join("payload.zip"), b"payload one").unwrap();
        let guard = inspect_create_destination(&app, CreateArtifactKind::SfxMacosApp)
            .unwrap()
            .guard
            .unwrap();
        fs::write(resources.join("payload.zip"), b"payload two").unwrap();

        assert!(
            verify_destination_guard(&app, CreateArtifactKind::SfxMacosApp, guard)
                .unwrap_err()
                .is_destination_changed()
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
