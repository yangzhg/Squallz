//! Smart-extraction layout analysis: decide whether an archive already has
//! a single root directory (extract directly) or holds loose entries
//! (wrap them in a folder named after the archive).

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::ffi::OsStr;
use std::fs;
use std::hash::Hasher;
use std::path::{Path, PathBuf};

use crate::api::{sanitize_entry_path, ControlToken, EntryMeta, EntryPath, EntryType, FormatError};

const CONTROL_CHECKPOINT_INTERVAL: usize = 256;
const MAX_SNAPSHOT_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_SNAPSHOT_ENUMERATED_ENTRIES: usize = 32_768;
const MAX_SNAPSHOT_CACHED_NODES: usize = 32_768;
const MAX_SNAPSHOT_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_SNAPSHOT_DIRECTORY_STATES: usize = 8_192;
const MIN_EXTRACT_ALLOCATION_GRANULARITY: u64 = 4 * 1024;

/// Selected entry and byte totals for an extraction plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractScope {
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub hardlinks: u64,
    pub other: u64,
    pub total_bytes: u64,
}

/// Read-only extraction preflight shared by CLI and desktop callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractPlan {
    /// Destination requested by the caller before smart layout is applied.
    pub requested_destination: PathBuf,
    /// Final destination after smart layout is applied.
    pub destination: PathBuf,
    /// Smart-layout verdict derived from the complete archive entry list.
    pub layout: SmartLayout,
    /// Totals for the selected extraction scope only.
    pub scope: ExtractScope,
    /// Selected entries whose final path currently conflicts with the
    /// destination snapshot or an earlier selected archive entry.
    pub estimated_conflicts: u64,
}

/// Destination-volume capacity observed for an extraction plan.
///
/// `required_bytes` is a conservative write budget rather than a prediction
/// of the final directory size. It includes selected file data plus one
/// allocation unit for every selected entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractSpace {
    pub required_bytes: u64,
    pub available_bytes: u64,
}

impl ExtractSpace {
    pub fn is_sufficient(self) -> bool {
        self.available_bytes >= self.required_bytes
    }
}

/// Checks free space on the volume that will hold the final extraction
/// destination. Missing destination folders are resolved to their nearest
/// existing directory without creating anything during preflight.
pub fn inspect_extract_space(plan: &ExtractPlan) -> Result<ExtractSpace, FormatError> {
    if plan.scope.entries == 0 {
        return Ok(ExtractSpace {
            required_bytes: 0,
            available_bytes: 0,
        });
    }
    let anchor = nearest_existing_directory(&plan.destination)?;
    let allocation_granularity =
        fs4::allocation_granularity(&anchor)?.max(MIN_EXTRACT_ALLOCATION_GRANULARITY);
    let entry_overhead = plan.scope.entries.saturating_mul(allocation_granularity);
    let required_bytes = plan.scope.total_bytes.saturating_add(entry_overhead);
    let available_bytes = fs4::available_space(anchor)?;
    Ok(ExtractSpace {
        required_bytes,
        available_bytes,
    })
}

fn nearest_existing_directory(path: &Path) -> Result<PathBuf, FormatError> {
    let mut candidate = path;
    loop {
        match fs::metadata(candidate) {
            Ok(metadata) if metadata.is_dir() => return Ok(candidate.to_path_buf()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if candidate == Path::new(".") {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
        candidate = match candidate.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
    }
}

/// Verdict of [`analyze_extract_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartLayout {
    /// Every entry lives under one root directory: extract as-is.
    DirectExtract,
    /// Loose files at the archive root: wrap them in a folder named after
    /// the archive (the caller derives the name).
    WrapInFolder,
}

/// Analyzes the entry list: [`SmartLayout::DirectExtract`] when all entries
/// share the same first path component *and* that component is a directory
/// (an explicit directory entry, or implicit because every entry is nested
/// below it); [`SmartLayout::WrapInFolder`] otherwise.
pub fn analyze_extract_layout(entries: &[EntryMeta]) -> SmartLayout {
    let result = analyze_extract_layout_checked(entries, || Ok::<(), Infallible>(()));
    match result {
        Ok(layout) => layout,
        Err(never) => match never {},
    }
}

fn analyze_extract_layout_with_control(
    entries: &[EntryMeta],
    control: &ControlToken,
) -> Result<SmartLayout, FormatError> {
    analyze_extract_layout_checked(entries, || control.checkpoint())
}

fn analyze_extract_layout_checked<E>(
    entries: &[EntryMeta],
    mut checkpoint: impl FnMut() -> Result<(), E>,
) -> Result<SmartLayout, E> {
    let mut root: Option<String> = None;
    let mut root_is_dir = false;
    for (index, meta) in entries.iter().enumerate() {
        if index % CONTROL_CHECKPOINT_INTERVAL == 0 {
            checkpoint()?;
        }
        let display = meta.path.display.replace('\\', "/");
        let mut comps = display.split('/').filter(|c| !c.is_empty() && *c != ".");
        let Some(first) = comps.next() else {
            continue; // degenerate entry name; ignore for the verdict
        };
        match &root {
            None => root = Some(first.to_string()),
            Some(r) if r != first => return Ok(SmartLayout::WrapInFolder),
            Some(_) => {}
        }
        if comps.next().is_none() {
            // Single-component entry: only a directory keeps the verdict.
            if matches!(meta.entry_type, EntryType::Dir) {
                root_is_dir = true;
            } else {
                return Ok(SmartLayout::WrapInFolder);
            }
        } else {
            root_is_dir = true; // implicit: something is nested below it
        }
    }
    checkpoint()?;
    Ok(if root.is_some() && root_is_dir {
        SmartLayout::DirectExtract
    } else if root.is_some() {
        SmartLayout::WrapInFolder
    } else {
        // Empty archive: nothing to wrap.
        SmartLayout::DirectExtract
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlannedTarget {
    Directory,
    NonDirectory,
}

pub(crate) fn build_extract_plan(
    requested_destination: &Path,
    archive_folder_name: &str,
    entries: &[EntryMeta],
    selection: Option<&[EntryPath]>,
    smart: bool,
    control: &ControlToken,
) -> Result<ExtractPlan, FormatError> {
    control.checkpoint()?;
    let layout = if smart {
        analyze_extract_layout_with_control(entries, control)?
    } else {
        SmartLayout::DirectExtract
    };
    let explicitly_empty = selection.is_some_and(<[EntryPath]>::is_empty);
    let destination = match layout {
        SmartLayout::DirectExtract => requested_destination.to_path_buf(),
        SmartLayout::WrapInFolder => {
            #[cfg(windows)]
            if !explicitly_empty {
                crate::api::check_windows_portability(archive_folder_name)?;
            }
            requested_destination.join(archive_folder_name)
        }
    };
    if explicitly_empty {
        control.checkpoint()?;
        return Ok(ExtractPlan {
            requested_destination: requested_destination.to_path_buf(),
            destination,
            layout,
            scope: ExtractScope::default(),
            estimated_conflicts: 0,
        });
    }
    if layout == SmartLayout::WrapInFolder {
        validate_wrapped_destination(requested_destination, &destination, control)?;
    }
    let wanted = match selection {
        Some(paths) => {
            let mut wanted = HashSet::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                if index % CONTROL_CHECKPOINT_INTERVAL == 0 {
                    control.checkpoint()?;
                }
                wanted.insert(path.raw.as_slice());
            }
            control.checkpoint()?;
            Some(wanted)
        }
        None => None,
    };
    let mut destination_snapshot = DestinationSnapshot::new(&destination, control)?;
    let planned_paths_case_sensitive = destination_snapshot.planned_paths_case_sensitive;
    let planned_destination = planned_path_identity(&destination, planned_paths_case_sensitive);
    let mut scope = ExtractScope::default();
    let mut planned = HashMap::<PathBuf, PlannedTarget>::new();
    let mut planned_directories = HashSet::<PathBuf>::new();
    let mut estimated_conflicts = 0u64;

    for (index, entry) in entries.iter().enumerate() {
        if index % CONTROL_CHECKPOINT_INTERVAL == 0 {
            control.checkpoint()?;
        }
        if wanted
            .as_ref()
            .is_some_and(|paths| !paths.contains(entry.path.raw.as_slice()))
        {
            continue;
        }
        scope.entries = scope.entries.saturating_add(1);
        match entry.entry_type {
            EntryType::File => {
                scope.files = scope.files.saturating_add(1);
                scope.total_bytes = scope.total_bytes.saturating_add(entry.size);
            }
            EntryType::Dir => scope.directories = scope.directories.saturating_add(1),
            EntryType::Symlink { .. } => scope.symlinks = scope.symlinks.saturating_add(1),
            EntryType::Hardlink { .. } => scope.hardlinks = scope.hardlinks.saturating_add(1),
            EntryType::Other => scope.other = scope.other.saturating_add(1),
        }

        // Match the extraction sink's path validation even for entries such
        // as `Other` that will be deliberately left unmaterialized.
        let relative = sanitize_entry_path(&entry.path)?;
        #[cfg(windows)]
        for component in relative.components() {
            if let std::path::Component::Normal(name) = component {
                crate::api::check_windows_portability(&name.to_string_lossy())?;
            }
        }
        let target_kind = match entry.entry_type {
            EntryType::Dir => Some(PlannedTarget::Directory),
            EntryType::File | EntryType::Symlink { .. } | EntryType::Hardlink { .. } => {
                Some(PlannedTarget::NonDirectory)
            }
            EntryType::Other => None,
        };
        let Some(target_kind) = target_kind else {
            continue;
        };
        let target = destination.join(relative);
        let planned_target = planned_path_identity(&target, planned_paths_case_sensitive);
        let observation = destination_snapshot.conflicts(&target, target_kind)?;
        let mut conflicts = observation.conflicts;
        if planned_path_conflicts(
            &planned_destination,
            &planned_target,
            target_kind,
            &planned,
            &planned_directories,
            control,
        )? {
            conflicts = true;
        }
        let target_remains_directory = target_kind == PlannedTarget::NonDirectory
            && (planned_directories.contains(&planned_target)
                || observation.final_node == SnapshotNode::Directory);
        record_planned_directories(
            &planned_destination,
            &planned_target,
            target_kind,
            &mut planned_directories,
            control,
        )?;
        match planned.entry(planned_target) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(if target_remains_directory {
                    PlannedTarget::Directory
                } else {
                    target_kind
                });
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
        if conflicts {
            estimated_conflicts = estimated_conflicts.saturating_add(1);
        }
    }
    control.checkpoint()?;

    Ok(ExtractPlan {
        requested_destination: requested_destination.to_path_buf(),
        destination,
        layout,
        scope,
        estimated_conflicts,
    })
}

fn validate_wrapped_destination(
    requested_destination: &Path,
    destination: &Path,
    control: &ControlToken,
) -> Result<(), FormatError> {
    control.checkpoint()?;
    let metadata = fs::symlink_metadata(destination);
    control.checkpoint()?;
    let metadata = match metadata {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(FormatError::SymlinkBreakout(
            destination.to_string_lossy().into_owned(),
        ));
    }
    let canonical_requested = requested_destination.canonicalize();
    control.checkpoint()?;
    let canonical_requested = canonical_requested?;
    let canonical_destination = destination.canonicalize();
    control.checkpoint()?;
    let canonical_destination = canonical_destination?;
    if canonical_destination.starts_with(canonical_requested) {
        return Ok(());
    }
    Err(FormatError::SymlinkBreakout(
        destination.to_string_lossy().into_owned(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotNode {
    Directory,
    Missing,
    Symlink,
    NonDirectory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectorySnapshotState {
    Complete,
    CompleteAscii,
    CompleteUnicodeDisjointAscii,
    CompleteEmpty,
    PointLookup,
}

#[derive(Debug, Clone, Copy)]
struct SnapshotBudget {
    directory_entries: usize,
    enumerated_entries: usize,
    cached_nodes: usize,
    cached_path_bytes: usize,
    directory_states: usize,
}

impl Default for SnapshotBudget {
    fn default() -> Self {
        Self {
            directory_entries: MAX_SNAPSHOT_DIRECTORY_ENTRIES,
            enumerated_entries: MAX_SNAPSHOT_ENUMERATED_ENTRIES,
            cached_nodes: MAX_SNAPSHOT_CACHED_NODES,
            cached_path_bytes: MAX_SNAPSHOT_PATH_BYTES,
            directory_states: MAX_SNAPSHOT_DIRECTORY_STATES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConflictObservation {
    conflicts: bool,
    final_node: SnapshotNode,
}

struct DestinationBase {
    destination_exists: bool,
    existing_directory: PathBuf,
}

struct DestinationSnapshot<'a> {
    destination: PathBuf,
    canonical_destination: Option<PathBuf>,
    base_exists: bool,
    planned_paths_case_sensitive: bool,
    nodes: HashMap<PathBuf, SnapshotNode>,
    ascii_case_aliases: HashSet<u64>,
    directories: HashMap<PathBuf, DirectorySnapshotState>,
    enumerated_entries: usize,
    cached_path_bytes: usize,
    budget: SnapshotBudget,
    control: &'a ControlToken,
    #[cfg(test)]
    filesystem_reads: usize,
}

impl<'a> DestinationSnapshot<'a> {
    fn new(destination: &Path, control: &'a ControlToken) -> Result<Self, FormatError> {
        Self::with_budget(destination, control, SnapshotBudget::default())
    }

    fn with_budget(
        destination: &Path,
        control: &'a ControlToken,
        budget: SnapshotBudget,
    ) -> Result<Self, FormatError> {
        let base = validate_destination_base(destination, control)?;
        let planned_paths_case_sensitive =
            destination_volume_is_case_sensitive(&base.existing_directory, control)?;
        let base_exists = base.destination_exists;
        let canonical_destination = if base_exists {
            let canonical = destination.canonicalize();
            control.checkpoint()?;
            Some(canonical?)
        } else {
            None
        };
        Ok(Self {
            destination: destination.to_path_buf(),
            canonical_destination,
            base_exists,
            planned_paths_case_sensitive,
            nodes: HashMap::new(),
            ascii_case_aliases: HashSet::new(),
            directories: HashMap::new(),
            enumerated_entries: 0,
            cached_path_bytes: 0,
            budget,
            control,
            #[cfg(test)]
            filesystem_reads: 0,
        })
    }

    fn conflicts(
        &mut self,
        target: &Path,
        target_kind: PlannedTarget,
    ) -> Result<ConflictObservation, FormatError> {
        self.control.checkpoint()?;
        if !self.base_exists {
            return Ok(ConflictObservation {
                conflicts: false,
                final_node: SnapshotNode::Missing,
            });
        }
        let relative = target.strip_prefix(&self.destination).map_err(|_| {
            FormatError::Other("planned extraction target escaped its destination".into())
        })?;
        let mut current = self.destination.clone();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            self.control.checkpoint()?;
            current.push(component);
            let node = self.node_at(&current)?;
            let final_component = components.peek().is_none();
            match (node, final_component) {
                (SnapshotNode::Missing, _) => {
                    return Ok(ConflictObservation {
                        conflicts: false,
                        final_node: SnapshotNode::Missing,
                    });
                }
                (SnapshotNode::Directory, false) => {}
                (SnapshotNode::Directory, true) => {
                    return Ok(ConflictObservation {
                        conflicts: target_kind != PlannedTarget::Directory,
                        final_node: SnapshotNode::Directory,
                    });
                }
                (SnapshotNode::Symlink, false) => {
                    self.validate_symlink_directory(&current)?;
                }
                (SnapshotNode::NonDirectory, false) => {
                    return Err(not_directory_error(&current));
                }
                (SnapshotNode::Symlink | SnapshotNode::NonDirectory, true)
                    if target_kind == PlannedTarget::Directory =>
                {
                    return Err(not_directory_error(&current));
                }
                (SnapshotNode::Symlink | SnapshotNode::NonDirectory, true) => {
                    return Ok(ConflictObservation {
                        conflicts: true,
                        final_node: node,
                    });
                }
            }
        }
        Ok(ConflictObservation {
            conflicts: false,
            final_node: SnapshotNode::Missing,
        })
    }

    fn node_at(&mut self, path: &Path) -> Result<SnapshotNode, FormatError> {
        self.control.checkpoint()?;
        let identity = snapshot_path_identity(path);
        if let Some(node) = self.nodes.get(&identity).copied() {
            return Ok(node);
        }
        let parent = path.parent().ok_or_else(|| {
            FormatError::Other("planned extraction target has no parent directory".into())
        })?;
        let directory_state = self.directory_state(parent)?;
        if let Some(node) = self.nodes.get(&identity).copied() {
            return Ok(node);
        }
        match directory_state {
            DirectorySnapshotState::CompleteEmpty => return Ok(SnapshotNode::Missing),
            DirectorySnapshotState::CompleteAscii
            | DirectorySnapshotState::CompleteUnicodeDisjointAscii
                if ascii_case_alias_hash(path).is_some_and(|hash| {
                    !self.ascii_case_aliases.contains(&hash)
                        && !ascii_name_may_be_native_alias(path)
                }) =>
            {
                return Ok(SnapshotNode::Missing);
            }
            DirectorySnapshotState::Complete
            | DirectorySnapshotState::CompleteAscii
            | DirectorySnapshotState::CompleteUnicodeDisjointAscii
            | DirectorySnapshotState::PointLookup => {}
        }
        #[cfg(test)]
        {
            self.filesystem_reads = self.filesystem_reads.saturating_add(1);
        }
        let node = inspect_snapshot_node(path, self.control)?;
        self.cache_node(identity, node);
        Ok(node)
    }

    fn validate_symlink_directory(&self, path: &Path) -> Result<(), FormatError> {
        self.control.checkpoint()?;
        let canonical = path.canonicalize();
        self.control.checkpoint()?;
        let canonical = canonical?;
        let canonical_destination = self.canonical_destination.as_deref().ok_or_else(|| {
            FormatError::Other("destination snapshot has no canonical base".into())
        })?;
        if !canonical.starts_with(canonical_destination) {
            return Err(FormatError::SymlinkBreakout(
                path.to_string_lossy().into_owned(),
            ));
        }
        let canonical_is_directory = canonical.is_dir();
        self.control.checkpoint()?;
        if !canonical_is_directory {
            return Err(not_directory_error(path));
        }
        Ok(())
    }

    fn directory_state(&mut self, directory: &Path) -> Result<DirectorySnapshotState, FormatError> {
        self.control.checkpoint()?;
        let identity = snapshot_path_identity(directory);
        if let Some(state) = self.directories.get(&identity).copied() {
            return Ok(state);
        }
        let state_path_bytes = path_storage_bytes(&identity);
        if self.directories.len() >= self.budget.directory_states
            || !self.can_cache_path_bytes(state_path_bytes)
        {
            return Ok(DirectorySnapshotState::PointLookup);
        }
        #[cfg(test)]
        {
            self.filesystem_reads = self.filesystem_reads.saturating_add(1);
        }
        self.control.checkpoint()?;
        let entries = fs::read_dir(directory);
        self.control.checkpoint()?;
        let entries = match entries {
            Ok(entries) => entries,
            Err(_) => {
                self.record_directory_state(
                    identity,
                    state_path_bytes,
                    DirectorySnapshotState::PointLookup,
                );
                return Ok(DirectorySnapshotState::PointLookup);
            }
        };
        let mut entries = entries;
        let mut local = HashMap::new();
        let mut local_ascii_case_aliases = HashSet::new();
        let mut all_names_ascii = true;
        let mut unicode_names_are_disjoint_from_ascii = true;
        let mut local_path_bytes = 0usize;
        loop {
            if self.enumerated_entries >= self.budget.enumerated_entries {
                self.record_directory_state(
                    identity,
                    state_path_bytes,
                    DirectorySnapshotState::PointLookup,
                );
                return Ok(DirectorySnapshotState::PointLookup);
            }
            self.control.checkpoint()?;
            let next = entries.next();
            self.control.checkpoint()?;
            let Some(entry) = next else {
                let state = if local.is_empty() {
                    DirectorySnapshotState::CompleteEmpty
                } else if all_names_ascii {
                    DirectorySnapshotState::CompleteAscii
                } else if unicode_names_are_disjoint_from_ascii {
                    DirectorySnapshotState::CompleteUnicodeDisjointAscii
                } else {
                    DirectorySnapshotState::Complete
                };
                self.cached_path_bytes = self
                    .cached_path_bytes
                    .saturating_add(state_path_bytes)
                    .saturating_add(local_path_bytes);
                self.directories.insert(identity, state);
                self.nodes.extend(local);
                self.ascii_case_aliases.extend(local_ascii_case_aliases);
                return Ok(state);
            };
            self.enumerated_entries = self.enumerated_entries.saturating_add(1);
            if local.len() >= self.budget.directory_entries {
                self.record_directory_state(
                    identity,
                    state_path_bytes,
                    DirectorySnapshotState::PointLookup,
                );
                return Ok(DirectorySnapshotState::PointLookup);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    self.record_directory_state(
                        identity,
                        state_path_bytes,
                        DirectorySnapshotState::PointLookup,
                    );
                    return Ok(DirectorySnapshotState::PointLookup);
                }
            };
            self.control.checkpoint()?;
            let file_type = entry.file_type();
            self.control.checkpoint()?;
            let file_type = match file_type {
                Ok(file_type) => file_type,
                Err(_) => {
                    self.record_directory_state(
                        identity,
                        state_path_bytes,
                        DirectorySnapshotState::PointLookup,
                    );
                    return Ok(DirectorySnapshotState::PointLookup);
                }
            };
            let node = if file_type.is_symlink() {
                SnapshotNode::Symlink
            } else if file_type.is_dir() {
                SnapshotNode::Directory
            } else {
                SnapshotNode::NonDirectory
            };
            let file_name = entry.file_name();
            let name_is_ascii = file_name.as_encoded_bytes().is_ascii();
            all_names_ascii &= name_is_ascii;
            if !name_is_ascii {
                unicode_names_are_disjoint_from_ascii &=
                    unicode_name_is_disjoint_from_ascii(&file_name);
            }
            let entry_identity = snapshot_path_identity(&entry.path());
            let entry_path_bytes = if local.contains_key(&entry_identity) {
                0
            } else {
                path_storage_bytes(&entry_identity)
            };
            if self.nodes.len().saturating_add(local.len()) >= self.budget.cached_nodes
                || !self.can_cache_path_bytes(
                    state_path_bytes
                        .saturating_add(local_path_bytes)
                        .saturating_add(entry_path_bytes),
                )
            {
                self.record_directory_state(
                    identity,
                    state_path_bytes,
                    DirectorySnapshotState::PointLookup,
                );
                return Ok(DirectorySnapshotState::PointLookup);
            }
            local_path_bytes = local_path_bytes.saturating_add(entry_path_bytes);
            if let Some(hash) = ascii_case_alias_hash(&entry_identity) {
                local_ascii_case_aliases.insert(hash);
            }
            local.insert(entry_identity, node);
        }
    }

    fn can_cache_path_bytes(&self, additional: usize) -> bool {
        self.cached_path_bytes
            .checked_add(additional)
            .is_some_and(|total| total <= self.budget.cached_path_bytes)
    }

    fn record_directory_state(
        &mut self,
        identity: PathBuf,
        path_bytes: usize,
        state: DirectorySnapshotState,
    ) {
        if self.directories.len() >= self.budget.directory_states
            || !self.can_cache_path_bytes(path_bytes)
        {
            return;
        }
        self.cached_path_bytes = self.cached_path_bytes.saturating_add(path_bytes);
        self.directories.insert(identity, state);
    }

    fn cache_node(&mut self, identity: PathBuf, node: SnapshotNode) {
        if let Some(existing) = self.nodes.get_mut(&identity) {
            *existing = node;
            return;
        }
        let path_bytes = path_storage_bytes(&identity);
        if self.nodes.len() >= self.budget.cached_nodes || !self.can_cache_path_bytes(path_bytes) {
            return;
        }
        self.cached_path_bytes = self.cached_path_bytes.saturating_add(path_bytes);
        self.nodes.insert(identity, node);
    }
}

fn path_storage_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

fn snapshot_path_identity(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn ascii_case_alias_hash(path: &Path) -> Option<u64> {
    let parent = path.parent()?.as_os_str().as_encoded_bytes();
    let name = path.file_name()?.as_encoded_bytes();
    if !name.is_ascii() {
        return None;
    }
    let mut hasher = DefaultHasher::new();
    hasher.write_usize(parent.len());
    hasher.write(parent);
    for byte in name {
        hasher.write_u8(byte.to_ascii_lowercase());
    }
    Some(hasher.finish())
}

fn ascii_name_may_be_native_alias(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let bytes = name.as_encoded_bytes();
        // Windows permits an explicitly assigned 8.3 alternate name without
        // a tilde, so every short ASCII component needs native confirmation.
        bytes.len() <= 12 || bytes.contains(&b'~')
    })
}

fn unicode_name_is_disjoint_from_ascii(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    // Keep the fast negative path deliberately narrow. Other Unicode names
    // may be folded, normalized, or ignored by APFS, HFS+, NTFS, ext4, or a
    // remote server and therefore require a native lookup.
    name.chars()
        .filter(|character| !character.is_ascii())
        .all(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}'
                    | '\u{4e00}'..='\u{9fff}'
                    | '\u{20000}'..='\u{2a6df}'
                    | '\u{2a700}'..='\u{2b73f}'
                    | '\u{2b740}'..='\u{2b81f}'
                    | '\u{2b820}'..='\u{2ceaf}'
                    | '\u{2ceb0}'..='\u{2ebef}'
                    | '\u{2ebf0}'..='\u{2ee5f}'
                    | '\u{30000}'..='\u{3134f}'
                    | '\u{31350}'..='\u{323af}'
            )
        })
}

fn inspect_snapshot_node(path: &Path, control: &ControlToken) -> Result<SnapshotNode, FormatError> {
    control.checkpoint()?;
    let node = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(SnapshotNode::Symlink),
        Ok(metadata) if metadata.is_dir() => Ok(SnapshotNode::Directory),
        Ok(_) => Ok(SnapshotNode::NonDirectory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SnapshotNode::Missing),
        Err(error) => Err(error.into()),
    };
    control.checkpoint()?;
    node
}

fn validate_destination_base(
    destination: &Path,
    control: &ControlToken,
) -> Result<DestinationBase, FormatError> {
    let mut current = destination.to_path_buf();
    let mut is_destination = true;
    loop {
        control.checkpoint()?;
        let metadata = fs::metadata(&current);
        control.checkpoint()?;
        match metadata {
            Ok(metadata) if metadata.is_dir() => {
                return Ok(DestinationBase {
                    destination_exists: is_destination,
                    existing_directory: current,
                });
            }
            Ok(_) => return Err(not_directory_error(&current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let symlink_metadata = fs::symlink_metadata(&current);
                control.checkpoint()?;
                match symlink_metadata {
                    Ok(_) => {
                        return Err(FormatError::SymlinkBreakout(
                            current.to_string_lossy().into_owned(),
                        ));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
        is_destination = false;
        current = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
    }
}

fn not_directory_error(path: &Path) -> FormatError {
    FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::NotADirectory,
        format!("extraction path is not a directory: {}", path.display()),
    ))
}

fn planned_path_conflicts(
    destination: &Path,
    target: &Path,
    target_kind: PlannedTarget,
    planned: &HashMap<PathBuf, PlannedTarget>,
    planned_directories: &HashSet<PathBuf>,
    control: &ControlToken,
) -> Result<bool, FormatError> {
    control.checkpoint()?;
    if target_kind == PlannedTarget::NonDirectory && planned_directories.contains(target) {
        return Ok(true);
    }
    if let Some(previous) = planned.get(target) {
        return Ok(match (*previous, target_kind) {
            (PlannedTarget::Directory, PlannedTarget::Directory) => false,
            (PlannedTarget::NonDirectory, PlannedTarget::NonDirectory)
            | (PlannedTarget::Directory, PlannedTarget::NonDirectory)
            | (PlannedTarget::NonDirectory, PlannedTarget::Directory) => true,
        });
    }

    let mut ancestor = target.parent();
    while let Some(path) = ancestor {
        control.checkpoint()?;
        if path == destination {
            break;
        }
        if !path.starts_with(destination) {
            break;
        }
        if planned.get(path) == Some(&PlannedTarget::NonDirectory) {
            return Ok(true);
        }
        ancestor = path.parent();
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn destination_volume_is_case_sensitive(
    existing_directory: &Path,
    control: &ControlToken,
) -> Result<bool, FormatError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    control.checkpoint()?;
    let Ok(path) = CString::new(existing_directory.as_os_str().as_bytes()) else {
        return Ok(false);
    };
    // SAFETY: `path` is a live, NUL-terminated copy and `pathconf` does not
    // retain it. Unknown filesystems use the conservative insensitive mode.
    let result = unsafe { libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE) };
    control.checkpoint()?;
    Ok(result == 1)
}

#[cfg(windows)]
fn destination_volume_is_case_sensitive(
    _existing_directory: &Path,
    control: &ControlToken,
) -> Result<bool, FormatError> {
    control.checkpoint()?;
    Ok(false)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn destination_volume_is_case_sensitive(
    _existing_directory: &Path,
    control: &ControlToken,
) -> Result<bool, FormatError> {
    control.checkpoint()?;
    Ok(true)
}

fn planned_path_identity(path: &Path, case_sensitive: bool) -> PathBuf {
    if case_sensitive {
        path.to_path_buf()
    } else {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
}

fn record_planned_directories(
    destination: &Path,
    target: &Path,
    target_kind: PlannedTarget,
    planned_directories: &mut HashSet<PathBuf>,
    control: &ControlToken,
) -> Result<(), FormatError> {
    control.checkpoint()?;
    if target_kind == PlannedTarget::Directory {
        planned_directories.insert(target.to_path_buf());
    }
    let mut ancestor = target.parent();
    while let Some(path) = ancestor {
        control.checkpoint()?;
        if path == destination {
            break;
        }
        if !path.starts_with(destination) {
            break;
        }
        planned_directories.insert(path.to_path_buf());
        ancestor = path.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{EntryPath, EntryType};

    fn meta(name: &str, dir: bool) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type: if dir { EntryType::Dir } else { EntryType::File },
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn planned_path_identity_respects_case_sensitivity() {
        let upper = Path::new("output/Foo/Child.txt");
        let lower = Path::new("output/foo/child.txt");

        assert_ne!(
            planned_path_identity(upper, true),
            planned_path_identity(lower, true)
        );
        assert_eq!(
            planned_path_identity(upper, false),
            planned_path_identity(lower, false)
        );
    }

    #[test]
    fn single_root_directory_extracts_directly() {
        // Explicit directory entry.
        let entries = vec![
            meta("project/", true),
            meta("project/a.txt", false),
            meta("project/sub/b.txt", false),
        ];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);
        // Implicit root (no explicit dir entry).
        let entries = vec![meta("root/a.txt", false), meta("root/b/c.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);
    }

    #[test]
    fn windows_separators_are_normalized_for_layout() {
        let entries = vec![meta("root\\a.txt", false), meta("root\\sub\\b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);

        let entries = vec![meta("root\\a.txt", false), meta("other\\b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
    }

    #[test]
    fn loose_entries_wrap_in_folder() {
        // Multiple roots.
        let entries = vec![meta("a.txt", false), meta("b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
        // Single loose file (not a directory).
        let entries = vec![meta("readme.md", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
        // A root dir plus a stray top-level file.
        let entries = vec![
            meta("root/", true),
            meta("root/a", false),
            meta("x.txt", false),
        ];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
    }

    #[test]
    fn empty_archive_extracts_directly() {
        assert_eq!(analyze_extract_layout(&[]), SmartLayout::DirectExtract);
    }

    #[test]
    fn plan_uses_full_layout_but_selected_scope_and_conflicts() {
        let requested =
            std::env::temp_dir().join(format!("squallz-core-extract-plan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&requested);
        let destination = requested.join("visible");
        fs::create_dir_all(destination.join("selected-dir")).unwrap();
        fs::write(destination.join("selected.txt"), b"existing").unwrap();
        fs::write(destination.join("unselected.txt"), b"existing").unwrap();

        let mut selected_file = meta("selected.txt", false);
        selected_file.size = 11;
        let selected_dir = meta("selected-dir/", true);
        let mut unselected_file = meta("unselected.txt", false);
        unselected_file.size = 99;
        let entries = vec![selected_file, selected_dir, unselected_file];
        let selection = vec![entries[0].path.clone(), entries[1].path.clone()];

        let engine = crate::Engine::new(crate::api::FormatRegistry::new());
        let plan = engine
            .plan_extract_from_entries(
                &requested,
                Path::new("/logical/archive/visible"),
                &entries,
                Some(&selection),
                true,
            )
            .unwrap();

        assert_eq!(plan.requested_destination, requested);
        assert_eq!(plan.destination, destination);
        assert_eq!(plan.layout, SmartLayout::WrapInFolder);
        assert_eq!(
            plan.scope,
            ExtractScope {
                entries: 2,
                files: 1,
                directories: 1,
                total_bytes: 11,
                ..ExtractScope::default()
            }
        );
        assert_eq!(plan.estimated_conflicts, 1);

        fs::remove_dir_all(&requested).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn smart_plan_rejects_a_reserved_nested_archive_folder_name() {
        let requested = Path::new("output");
        let entries = vec![meta("loose.txt", false)];

        let error = build_extract_plan(
            requested,
            "CON",
            &entries,
            None,
            true,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::UnsafeFileName(_)), "{error:?}");
    }

    #[test]
    fn plan_counts_archive_internal_file_and_child_collisions_in_both_orders() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-tree-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);

        let parent_first = vec![meta("node", false), meta("node/child.txt", false)];
        let child_first = vec![meta("node/child.txt", false), meta("node", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let parent_first_plan = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &parent_first, None, false)
            .unwrap();
        let child_first_plan = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &child_first, None, false)
            .unwrap();

        assert_eq!(parent_first_plan.estimated_conflicts, 1);
        assert_eq!(child_first_plan.estimated_conflicts, 1);
        assert!(!requested.exists());
    }

    #[test]
    fn plan_counts_a_file_that_collides_with_an_implicit_planned_directory() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-child-first-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        let entries = vec![meta("node/child.txt", false), meta("node", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let plan = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap();

        assert_eq!(plan.estimated_conflicts, 1);
        assert!(!requested.exists());
    }

    #[test]
    fn plan_rejects_an_existing_non_directory_ancestor_without_following_it() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-ancestor-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join("blocked"), b"file").unwrap();
        let entries = vec![meta("blocked/child.txt", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let error = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap_err();

        assert!(
            matches!(error, FormatError::Io(ref error) if error.kind() == std::io::ErrorKind::NotADirectory),
            "{error:?}"
        );
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn plan_rejects_a_non_directory_destination() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-file-destination-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let requested = root.join("output");
        fs::write(&requested, b"file").unwrap();
        let entries = vec![meta("entry.txt", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let error = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap_err();

        assert!(
            matches!(error, FormatError::Io(ref error) if error.kind() == std::io::ErrorKind::NotADirectory),
            "{error:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn empty_selection_plan_keeps_layout_without_validating_its_destination() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-empty-invalid-destination-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let wrapped_destination = root.join("archive.zip");
        fs::write(&wrapped_destination, b"file").unwrap();
        let entries = vec![meta("loose.txt", false)];
        let selection = Vec::new();
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let plan = engine
            .plan_extract_from_entries(
                &root,
                Path::new("archive.zip"),
                &entries,
                Some(&selection),
                true,
            )
            .unwrap();

        assert_eq!(plan.layout, SmartLayout::WrapInFolder);
        assert_eq!(plan.destination, wrapped_destination);
        assert_eq!(plan.scope, ExtractScope::default());
        assert_eq!(plan.estimated_conflicts, 0);
        assert_eq!(
            inspect_extract_space(&plan).unwrap(),
            ExtractSpace {
                required_bytes: 0,
                available_bytes: 0,
            }
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn smart_wrap_rejects_a_preexisting_symlink_destination() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-smart-symlink-{}",
            std::process::id()
        ));
        let requested = root.join("output");
        let outside = root.join("outside");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&requested).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, requested.join("archive")).unwrap();
        let entries = vec![meta("loose.txt", false)];

        let error = build_extract_plan(
            &requested,
            "archive",
            &entries,
            None,
            true,
            &ControlToken::default(),
        )
        .unwrap_err();

        assert!(
            matches!(error, FormatError::SymlinkBreakout(_)),
            "{error:?}"
        );
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn empty_selection_plan_does_not_validate_an_unused_reserved_folder_name() {
        let selection = Vec::new();
        let entries = vec![meta("loose.txt", false)];

        let plan = build_extract_plan(
            Path::new("output"),
            "CON",
            &entries,
            Some(&selection),
            true,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(plan.destination, Path::new("output").join("CON"));
        assert_eq!(plan.scope, ExtractScope::default());
    }

    #[test]
    fn destination_snapshot_reads_a_large_directory_once() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-large-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        for index in 0..100_000u64 {
            let target = requested.join(format!("entry-{index}.txt"));
            assert!(
                !snapshot
                    .conflicts(&target, PlannedTarget::NonDirectory)
                    .unwrap()
                    .conflicts
            );
        }

        assert_eq!(snapshot.filesystem_reads, 1);
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn nonempty_snapshot_avoids_point_lookups_for_unrelated_ascii_names() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-nonempty-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join(".DS_Store"), b"existing").unwrap();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        for index in 0..100_000u64 {
            let target = requested.join(format!("new-entry-{index}.txt"));
            assert!(
                !snapshot
                    .conflicts(&target, PlannedTarget::NonDirectory)
                    .unwrap()
                    .conflicts
            );
        }

        assert_eq!(snapshot.filesystem_reads, 1);
        assert!(snapshot.ascii_case_aliases.len() <= snapshot.nodes.len());
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn unicode_siblings_do_not_force_point_lookups_for_disjoint_ascii_names() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-unicode-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join("资料.txt"), b"existing").unwrap();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        for index in 0..100_000u64 {
            let target = requested.join(format!("new-entry-{index}.txt"));
            assert!(
                !snapshot
                    .conflicts(&target, PlannedTarget::NonDirectory)
                    .unwrap()
                    .conflicts
            );
        }

        assert_eq!(snapshot.filesystem_reads, 1);
        assert_eq!(
            snapshot
                .directories
                .get(&snapshot_path_identity(&requested)),
            Some(&DirectorySnapshotState::CompleteUnicodeDisjointAscii)
        );
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn complete_snapshot_checks_possible_native_short_name_aliases() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-short-name-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join("Long File Name.txt"), b"existing").unwrap();
        let possible_short_name = requested.join("REPORT.TXT");
        assert!(ascii_name_may_be_native_alias(&possible_short_name));
        assert!(ascii_name_may_be_native_alias(
            &requested.join("LONGFI~1.TXT")
        ));
        assert!(!ascii_name_may_be_native_alias(
            &requested.join("definitely-long-new-name.txt")
        ));
        let native_conflict = fs::symlink_metadata(&possible_short_name).is_ok();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        let observation = snapshot
            .conflicts(&possible_short_name, PlannedTarget::NonDirectory)
            .unwrap();

        assert_eq!(observation.conflicts, native_conflict);
        assert_eq!(snapshot.filesystem_reads, 2);
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn complete_snapshot_checks_unicode_names_that_can_fold_to_ascii() {
        assert!(unicode_name_is_disjoint_from_ascii(OsStr::new("资料.txt")));
        for name in [
            "ẞ.txt",
            "\u{037e}.txt",
            "\u{1fef}.txt",
            "report\u{200c}.txt",
            "report\u{feff}.txt",
        ] {
            assert!(!unicode_name_is_disjoint_from_ascii(OsStr::new(name)));
        }
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-unicode-alias-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join("K.txt"), b"existing").unwrap();
        let folded_name = requested.join("k.txt");
        let native_conflict = fs::symlink_metadata(&folded_name).is_ok();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        let observation = snapshot
            .conflicts(&folded_name, PlannedTarget::NonDirectory)
            .unwrap();

        assert_eq!(observation.conflicts, native_conflict);
        assert_eq!(snapshot.filesystem_reads, 2);
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn wide_destination_switches_to_bounded_exact_point_lookups() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-bounded-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        for name in ["one.txt", "two.txt", "three.txt"] {
            fs::write(requested.join(name), b"existing").unwrap();
        }
        let control = ControlToken::default();
        let budget = SnapshotBudget {
            directory_entries: 2,
            enumerated_entries: 8,
            cached_nodes: 2,
            cached_path_bytes: 4_096,
            directory_states: 2,
        };
        let mut snapshot = DestinationSnapshot::with_budget(&requested, &control, budget).unwrap();

        let missing = requested.join("missing.txt");
        assert!(
            !snapshot
                .conflicts(&missing, PlannedTarget::NonDirectory)
                .unwrap()
                .conflicts
        );
        assert_eq!(
            snapshot
                .directories
                .get(&snapshot_path_identity(&requested)),
            Some(&DirectorySnapshotState::PointLookup)
        );
        assert!(snapshot.enumerated_entries <= budget.enumerated_entries);
        assert!(snapshot.nodes.len() <= budget.cached_nodes);
        assert!(snapshot.cached_path_bytes <= budget.cached_path_bytes);

        let late = requested.join("late.txt");
        fs::write(&late, b"late conflict").unwrap();
        let observation = snapshot
            .conflicts(&late, PlannedTarget::NonDirectory)
            .unwrap();
        assert!(observation.conflicts);
        assert_eq!(observation.final_node, SnapshotNode::NonDirectory);
        assert!(snapshot.nodes.len() <= budget.cached_nodes);
        assert!(snapshot.cached_path_bytes <= budget.cached_path_bytes);
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn complete_snapshot_uses_the_filesystems_case_rules_for_cache_misses() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-case-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        fs::write(requested.join("Existing-Name.txt"), b"existing").unwrap();
        let differently_cased = requested.join("existing-name.txt");
        let native_conflict = fs::symlink_metadata(&differently_cased).is_ok();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();

        let observation = snapshot
            .conflicts(&differently_cased, PlannedTarget::NonDirectory)
            .unwrap();

        assert_eq!(observation.conflicts, native_conflict);
        assert_eq!(
            observation.final_node,
            if native_conflict {
                SnapshotNode::NonDirectory
            } else {
                SnapshotNode::Missing
            }
        );
        fs::remove_dir_all(&requested).unwrap();
    }

    #[test]
    fn cancelled_control_stops_extract_plan_without_touching_destination() {
        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-cancelled-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        let entries = vec![meta("file.txt", false)];
        let control = ControlToken::new();
        control.cancel();
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let error = engine
            .plan_extract_from_entries_with_control(
                &requested,
                Path::new("archive"),
                &entries,
                None,
                false,
                &control,
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert!(!requested.exists());
    }

    #[cfg(unix)]
    #[test]
    fn destination_snapshot_falls_back_when_directory_listing_is_unavailable() {
        use std::os::unix::fs::PermissionsExt;

        let requested = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-unlisted-snapshot-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&requested);
        fs::create_dir_all(&requested).unwrap();
        let control = ControlToken::default();
        let mut snapshot = DestinationSnapshot::new(&requested, &control).unwrap();
        fs::set_permissions(&requested, fs::Permissions::from_mode(0o333)).unwrap();

        let result = snapshot.conflicts(
            &requested.join("known-target.txt"),
            PlannedTarget::NonDirectory,
        );

        fs::set_permissions(&requested, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!result.unwrap().conflicts);
        fs::remove_dir_all(&requested).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn plan_accepts_a_user_selected_destination_symlink_to_a_directory() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-base-symlink-{}",
            std::process::id()
        ));
        let actual = root.join("actual");
        let requested = root.join("chosen");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &requested).unwrap();
        let entries = vec![meta("free.txt", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let plan = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap();

        assert_eq!(plan.destination, requested);
        assert_eq!(plan.estimated_conflicts, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn plan_accepts_an_existing_symlink_ancestor_that_stays_inside_destination() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-internal-symlink-{}",
            std::process::id()
        ));
        let requested = root.join("output");
        let real = requested.join("real");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink("real", requested.join("alias")).unwrap();
        let entries = vec![meta("alias/file.txt", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let plan = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap();

        assert_eq!(plan.estimated_conflicts, 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn plan_rejects_a_symlink_ancestor() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-symlink-{}",
            std::process::id()
        ));
        let requested = root.join("output");
        let outside = root.join("outside");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&requested).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("child.txt"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, requested.join("escape")).unwrap();
        let entries = vec![meta("escape/child.txt", false)];
        let engine = crate::Engine::new(crate::api::FormatRegistry::new());

        let error = engine
            .plan_extract_from_entries(&requested, Path::new("archive"), &entries, None, false)
            .unwrap_err();

        assert!(
            matches!(error, FormatError::SymlinkBreakout(_)),
            "{error:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plan_uses_the_target_volume_case_rules_for_planned_collisions() {
        let root = std::env::temp_dir().join(format!(
            "squallz-core-extract-plan-volume-case-{}",
            std::process::id()
        ));
        let requested = root.join("output");
        let probe = root.join("Squallz-Case-Probe");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(&probe, b"probe").unwrap();
        let native_case_insensitive = fs::symlink_metadata(root.join("squallz-case-probe")).is_ok();
        fs::remove_file(&probe).unwrap();

        let entries = vec![meta("A.txt", false), meta("a.txt", false)];
        let plan = build_extract_plan(
            &requested,
            "archive",
            &entries,
            None,
            false,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(plan.estimated_conflicts, u64::from(native_case_insensitive));
        assert!(!requested.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn plan_treats_case_only_windows_names_as_the_same_target() {
        let requested = Path::new("output");
        let entries = vec![meta("A.txt", false), meta("a.txt", false)];

        let plan = build_extract_plan(
            requested,
            "archive",
            &entries,
            None,
            false,
            &ControlToken::default(),
        )
        .unwrap();

        assert_eq!(plan.estimated_conflicts, 1);
    }
}
