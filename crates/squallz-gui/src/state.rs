//! Tauri-independent application state: the engine, the opened-archive
//! cache with per-directory pagination, and the session password cache.
//! Everything here is plain Rust so it can be unit-tested without a window.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use squallz_core::api::{
    split_volume_name, ControlToken, EntryMeta, EntryType, FormatError, OpenOptions, Password,
    SafetyLimits,
};
use squallz_core::{
    collect_volume_set_with_control, fold_archive_search_path, fold_archive_search_query,
    rank_folded_archive_path, ArchiveListing, Engine,
};
use tempfile::TempPath;

use crate::dto::{ArchiveInfo, EntryDto, Page};
use crate::password_cache::{PasswordAttempt, SessionPasswords};
use crate::preview_sessions::{PreviewResourceLease, PreviewResourceReservation};
use squallz_core::lock_unpoisoned;

/// Default page size of the entry list.
pub const DEFAULT_PAGE_SIZE: usize = 500;
const ARCHIVE_SOURCE_PREFIX: &str = "squallz-archive://";
const ARCHIVE_UNAVAILABLE_DETAIL: &str = "archive is no longer available";
const INDEX_SORT_CHUNK_SIZE: usize = 4_096;

/// One row at a directory level. Real entries borrow their base name from
/// `CachedArchive::entries`; only synthesized directories retain a name.
#[derive(Debug, Clone)]
enum Row {
    Entry { index: usize, is_dir: bool },
    SynthesizedDir(Box<str>),
}

impl Row {
    fn name<'a>(&'a self, entries: &'a [EntryMeta]) -> &'a str {
        match self {
            Self::Entry { index, .. } => entry_base_name_ref(&entries[*index].path.display),
            Self::SynthesizedDir(name) => name,
        }
    }

    fn is_dir(&self) -> bool {
        match self {
            Self::Entry { is_dir, .. } => *is_dir,
            Self::SynthesizedDir(_) => true,
        }
    }

    fn entry_index(&self) -> Option<usize> {
        match self {
            Self::Entry { index, .. } => Some(*index),
            Self::SynthesizedDir(_) => None,
        }
    }
}

enum PendingRow {
    Entry { index: usize, is_dir: bool },
    SynthesizedDir,
}

#[derive(Debug, Clone)]
enum SearchSource {
    Entry(usize),
    SynthesizedDir(Box<str>),
}

struct SortItem<K, V> {
    key: K,
    value: V,
}

impl<K: PartialEq, V> PartialEq for SortItem<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<K: Eq, V> Eq for SortItem<K, V> {}

impl<K: Ord, V> PartialOrd for SortItem<K, V> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord, V> Ord for SortItem<K, V> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SearchSourceSortKey {
    Entry(usize),
    SynthesizedDir(Box<str>),
}

#[derive(Debug, Default)]
struct SearchCache {
    query: String,
    matches: Vec<SearchSource>,
}

/// A fully listed archive kept in memory for browsing.
pub struct CachedArchive {
    /// `None` is reserved for callers of the public, Tauri-independent API.
    /// Window-owned handles always carry the exact WebView label.
    owner_window: Option<String>,
    /// Physical archive location used only by the backend.
    source_path: PathBuf,
    /// Stable user-facing source identity. Nested archives never expose their
    /// private workspace path through this value.
    display_path: String,
    display_name: String,
    read_only: bool,
    /// All entries in archive order
    pub entries: Vec<EntryMeta>,
    /// Directory level → sorted rows ("" = root, otherwise `a/b/`)
    levels: HashMap<String, Vec<Row>>,
    /// Most recent query result, built on demand and reused by virtual-list
    /// page requests. Archives that are only browsed never retain another full
    /// path index.
    search_cache: Mutex<SearchCache>,
    /// Newer frontend generations cancel older archive-wide scans.
    search_generation: AtomicU64,
    /// Keeps a nested archive's private plaintext file and resource lease alive
    /// exactly as long as the cached archive remains open.
    _owned_temp: Option<Arc<OwnedArchiveTemp>>,
}

struct OwnedArchiveTemp {
    _file: TempPath,
    _lease: PreviewResourceLease,
    passwords: Arc<SessionPasswords>,
}

impl Drop for OwnedArchiveTemp {
    fn drop(&mut self) {
        self.passwords.forget(&self._file);
    }
}

struct PendingOwnedArchiveTemp {
    file: TempPath,
    reservation: PreviewResourceReservation,
    size: u64,
}

enum ArchiveBacking {
    Persistent,
    Pending(PendingOwnedArchiveTemp),
    Shared(Arc<OwnedArchiveTemp>),
}

struct ArchiveIdentity {
    display_path: String,
    display_name: Option<String>,
    read_only: bool,
    backing: ArchiveBacking,
}

/// A backend-only resolution of an archive source reference. Holding this
/// value pins a nested archive's plaintext and resource lease for the whole
/// operation, even if its browse handle is closed meanwhile.
pub(crate) struct ResolvedArchiveSource {
    path: PathBuf,
    display_path: String,
    read_only: bool,
    _archive: Option<Arc<CachedArchive>>,
}

#[derive(Default)]
struct ArchiveRegistry {
    archives: HashMap<u64, Arc<CachedArchive>>,
    released_windows: HashSet<String>,
    shutting_down: bool,
}

impl ResolvedArchiveSource {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn display_path(&self) -> &str {
        &self.display_path
    }

    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only
    }
}

/// Shared application state.
pub struct AppState {
    /// The engine (registry of all built-in formats)
    pub engine: Engine,
    archives: Mutex<ArchiveRegistry>,
    next_id: AtomicU64,
    passwords: Arc<SessionPasswords>,
}

impl AppState {
    /// Builds the state with the full built-in format registry.
    pub fn new() -> Self {
        Self {
            engine: Engine::new(squallz_formats::registry()),
            archives: Mutex::new(ArchiveRegistry::default()),
            next_id: AtomicU64::new(1),
            passwords: Arc::new(SessionPasswords::default()),
        }
    }

    /// Opens an archive, lists all entries and caches them under a fresh id.
    pub fn open_archive(
        &self,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
    ) -> Result<ArchiveInfo, FormatError> {
        let control = ControlToken::default();
        self.open_archive_for_owner(
            None,
            path,
            password,
            encoding,
            SafetyLimits::default().max_entries,
            &control,
        )
    }

    #[cfg(test)]
    pub(crate) fn open_archive_for_window(
        &self,
        owner_window: &str,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
    ) -> Result<ArchiveInfo, FormatError> {
        let control = ControlToken::default();
        self.open_archive_for_owner(
            Some(owner_window),
            path,
            password,
            encoding,
            SafetyLimits::default().max_entries,
            &control,
        )
    }

    pub(crate) fn open_archive_for_window_with_entry_limit_and_control(
        &self,
        owner_window: &str,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
        max_entries: u64,
        control: &ControlToken,
    ) -> Result<ArchiveInfo, FormatError> {
        self.open_archive_for_owner(
            Some(owner_window),
            path,
            password,
            encoding,
            max_entries,
            control,
        )
    }

    fn open_archive_for_owner(
        &self,
        owner_window: Option<&str>,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
        max_entries: u64,
        control: &ControlToken,
    ) -> Result<ArchiveInfo, FormatError> {
        self.open_archive_inner(
            owner_window,
            path,
            password,
            encoding,
            max_entries,
            control,
            ArchiveIdentity {
                display_path: path.to_string_lossy().into_owned(),
                display_name: None,
                read_only: false,
                backing: ArchiveBacking::Persistent,
            },
        )
    }

    /// Opens either a filesystem path or an opaque source reference returned
    /// for an already-open nested archive.
    #[cfg(test)]
    pub(crate) fn open_archive_source(
        &self,
        owner_window: &str,
        source: &str,
        password: Option<&str>,
        encoding: Option<&str>,
    ) -> Result<ArchiveInfo, FormatError> {
        let control = ControlToken::default();
        self.open_archive_source_with_entry_limit_and_control(
            owner_window,
            source,
            password,
            encoding,
            SafetyLimits::default().max_entries,
            &control,
        )
    }

    pub(crate) fn open_archive_source_with_entry_limit_and_control(
        &self,
        owner_window: &str,
        source: &str,
        password: Option<&str>,
        encoding: Option<&str>,
        max_entries: u64,
        control: &ControlToken,
    ) -> Result<ArchiveInfo, FormatError> {
        control.checkpoint()?;
        let resolved = self.resolve_archive_source(source, Some(owner_window))?;
        let Some(archive) = resolved._archive.as_ref() else {
            return self.open_archive_for_window_with_entry_limit_and_control(
                owner_window,
                resolved.path(),
                password,
                encoding,
                max_entries,
                control,
            );
        };
        control.checkpoint()?;
        let owned_temp = archive._owned_temp.as_ref().cloned().ok_or_else(|| {
            FormatError::Other("nested archive source is no longer available".to_owned())
        })?;
        self.open_archive_inner(
            Some(owner_window),
            resolved.path(),
            password,
            encoding,
            max_entries,
            control,
            ArchiveIdentity {
                display_path: resolved.display_path.clone(),
                display_name: Some(archive.display_name.clone()),
                read_only: true,
                backing: ArchiveBacking::Shared(owned_temp),
            },
        )
    }

    #[allow(clippy::too_many_arguments)] // Every value belongs to the owned archive identity.
    pub(crate) fn open_archive_with_owned_temp_and_entry_limit(
        &self,
        owner_window: &str,
        temp: TempPath,
        reservation: PreviewResourceReservation,
        size: u64,
        display_path: String,
        display_name: String,
        max_entries: u64,
        password: Option<&str>,
        control: &ControlToken,
    ) -> Result<ArchiveInfo, FormatError> {
        let path = temp.to_path_buf();
        let owned_temp = PendingOwnedArchiveTemp {
            file: temp,
            reservation,
            size,
        };
        self.open_archive_inner(
            Some(owner_window),
            &path,
            password,
            None,
            max_entries,
            control,
            ArchiveIdentity {
                display_path,
                display_name: Some(display_name),
                read_only: true,
                backing: ArchiveBacking::Pending(owned_temp),
            },
        )
    }

    #[allow(clippy::too_many_arguments)] // Internal archive-open boundary with explicit policy.
    fn open_archive_inner(
        &self,
        owner_window: Option<&str>,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
        max_entries: u64,
        control: &ControlToken,
        identity: ArchiveIdentity,
    ) -> Result<ArchiveInfo, FormatError> {
        control.checkpoint()?;
        self.ensure_owner_active(owner_window)?;
        let password_attempt = password.and_then(|_| self.password_attempt(path, control));
        let open_opts = OpenOptions {
            password: password
                .map(Password::new)
                .or_else(|| self.password_for(path)),
            encoding_override: encoding.map(str::to_owned),
        };
        let ArchiveListing {
            format,
            entries,
            source_set: native_source_set,
            structure,
            password_verified,
        } = self
            .engine
            .list_with_format_source_set_and_structure_with_entry_limit_and_control(
                path,
                &open_opts,
                max_entries,
                control,
            )?;
        let file_name = identity
            .display_name
            .clone()
            .unwrap_or_else(|| archive_file_name(path));
        let volumes = if identity.read_only {
            None
        } else if let Some(source_set) = native_source_set {
            Some(path_file_names(source_set.members().iter(), control)?)
        } else if split_volume_name(&file_name).is_some() {
            control.checkpoint()?;
            let parts = collect_volume_set_with_control(path, control)?;
            control.checkpoint()?;
            Some(path_file_names(parts.iter(), control)?)
        } else {
            None
        };
        let display_name = archive_display_name(&file_name);
        let encoding = encoding_diagnostics(&entries, encoding, control)?;

        let levels = build_levels(&entries, control)?;
        control.checkpoint()?;
        let owned_temp = match identity.backing {
            ArchiveBacking::Pending(pending) => Some(Arc::new(OwnedArchiveTemp {
                _file: pending.file,
                _lease: pending.reservation.into_lease(pending.size)?,
                passwords: Arc::clone(&self.passwords),
            })),
            ArchiveBacking::Shared(owned) => Some(owned),
            ArchiveBacking::Persistent => None,
        };
        let entry_count = entries.len();
        control.checkpoint()?;
        let mut registry = lock_unpoisoned(&self.archives);
        if control.is_cancelled() {
            return Err(FormatError::Cancelled);
        }
        if !owner_is_active(&registry, owner_window) {
            return Err(archive_unavailable());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let source = if identity.read_only {
            archive_source_for_id(id)
        } else {
            identity.display_path.clone()
        };
        registry.archives.insert(
            id,
            Arc::new(CachedArchive {
                owner_window: owner_window.map(str::to_owned),
                source_path: path.to_path_buf(),
                display_path: identity.display_path.clone(),
                display_name: display_name.clone(),
                read_only: identity.read_only,
                entries,
                levels,
                search_cache: Mutex::new(SearchCache::default()),
                search_generation: AtomicU64::new(0),
                _owned_temp: owned_temp,
            }),
        );
        drop(registry);
        // Remember a freshly supplied, proven-good password only after the
        // owner-bound handle has been published successfully.
        if password_verified {
            if let (Some(pw), Some(attempt)) = (password, password_attempt.as_ref()) {
                self.remember_password(attempt, pw, control);
            }
        }
        let info = ArchiveInfo {
            id,
            path: identity.display_path,
            source,
            name: display_name,
            read_only: identity.read_only,
            format,
            structure: structure.id().to_owned(),
            entry_count,
            volumes,
            non_utf8_name_count: encoding.non_utf8_name_count,
            garbled_count: encoding.garbled_count,
            suggested_encoding: encoding.suggested,
            encoding_override: encoding.override_label,
        };
        Ok(info)
    }

    /// Resolves an opaque nested source without returning its physical path to
    /// the caller-facing DTO. The returned lease keeps the cache alive.
    pub(crate) fn resolve_archive_source(
        &self,
        source: &str,
        owner_window: Option<&str>,
    ) -> Result<ResolvedArchiveSource, FormatError> {
        let Some(raw_id) = source.strip_prefix(ARCHIVE_SOURCE_PREFIX) else {
            self.ensure_owner_active(owner_window)?;
            return Ok(ResolvedArchiveSource {
                path: PathBuf::from(source),
                display_path: source.to_owned(),
                read_only: false,
                _archive: None,
            });
        };
        let id = raw_id.parse::<u64>().map_err(|_| archive_unavailable())?;
        let archive = self.archive_for_owner(id, owner_window)?;
        Ok(ResolvedArchiveSource {
            path: archive.source_path.clone(),
            display_path: archive.display_path.clone(),
            read_only: archive.read_only,
            _archive: Some(archive),
        })
    }

    /// Drops a cached archive.
    pub fn close_archive(&self, id: u64) {
        self.close_archive_for_owner(None, id);
    }

    pub(crate) fn close_archive_for_window(&self, owner_window: &str, id: u64) {
        self.close_archive_for_owner(Some(owner_window), id);
    }

    pub(crate) fn preview_entries_for_window(
        &self,
        owner_window: &str,
        id: u64,
        limit: usize,
    ) -> Result<Vec<EntryDto>, FormatError> {
        let archive = self.archive_for_owner(id, Some(owner_window))?;
        Ok(archive
            .entries
            .iter()
            .take(limit)
            .map(|meta| {
                let path = normalized_entry_path(meta);
                let name = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .to_owned();
                EntryDto::from_meta(meta, path, name)
            })
            .collect())
    }

    fn close_archive_for_owner(&self, owner_window: Option<&str>, id: u64) {
        let archive = {
            let mut registry = lock_unpoisoned(&self.archives);
            match registry.archives.get(&id) {
                Some(archive) if archive.owner_window.as_deref() == owner_window => {
                    archive.search_generation.store(u64::MAX, Ordering::Release);
                    registry.archives.remove(&id)
                }
                _ => None,
            }
        };
        drop(archive);
    }

    /// Advances the search generation without starting another scan.
    pub fn cancel_search(&self, id: u64, generation: u64) -> Result<(), FormatError> {
        self.cancel_search_for_owner(None, id, generation)
    }

    pub(crate) fn cancel_search_for_window(
        &self,
        owner_window: &str,
        id: u64,
        generation: u64,
    ) -> Result<(), FormatError> {
        self.cancel_search_for_owner(Some(owner_window), id, generation)
    }

    fn cancel_search_for_owner(
        &self,
        owner_window: Option<&str>,
        id: u64,
        generation: u64,
    ) -> Result<(), FormatError> {
        let archive = self.archive_for_owner(id, owner_window)?;
        archive
            .search_generation
            .fetch_max(generation, Ordering::AcqRel);
        Ok(())
    }

    /// Pages one directory level of a cached archive. `dir_prefix` is ""
    /// for the root or `a/b/`; `filter` is a case-insensitive substring
    /// match on the base name.
    pub fn list_entries(
        &self,
        id: u64,
        page: usize,
        page_size: usize,
        dir_prefix: &str,
        filter: Option<&str>,
    ) -> Result<Page, FormatError> {
        self.list_entries_for_owner(None, id, page, page_size, dir_prefix, filter)
    }

    /// Resolves a browse location against the current archive's directory index.
    pub(crate) fn resolve_archive_directory_for_window(
        &self,
        owner_window: &str,
        id: u64,
        dir_prefix: &str,
    ) -> Result<String, FormatError> {
        let archive = self.archive_for_owner(id, Some(owner_window))?;
        Ok(resolve_archive_directory(&archive, dir_prefix))
    }

    pub(crate) fn list_entries_for_window(
        &self,
        owner_window: &str,
        id: u64,
        page: usize,
        page_size: usize,
        dir_prefix: &str,
        filter: Option<&str>,
    ) -> Result<Page, FormatError> {
        self.list_entries_for_owner(Some(owner_window), id, page, page_size, dir_prefix, filter)
    }

    pub(crate) fn missing_archive_paths_for_window(
        &self,
        owner_window: &str,
        id: u64,
        paths: &[String],
    ) -> Result<Vec<String>, FormatError> {
        let archive = self.archive_for_owner(id, Some(owner_window))?;
        Ok(paths
            .iter()
            .filter(|path| !archive_path_exists(&archive, path))
            .cloned()
            .collect())
    }

    pub(crate) fn plan_archive_move_for_window(
        &self,
        owner_window: &str,
        id: u64,
        paths: &[String],
        target_dir: &str,
    ) -> Result<squallz_core::ArchiveMovePlan, FormatError> {
        let archive = self.archive_for_owner(id, Some(owner_window))?;
        squallz_core::plan_archive_moves(
            paths,
            target_dir,
            |path| archive_path_exists(&archive, path),
            &ControlToken::new(),
        )
    }

    pub(crate) fn inspect_archive_target_for_window(
        &self,
        owner_window: &str,
        id: u64,
        target: &str,
    ) -> Result<squallz_core::ArchiveTargetInspection, FormatError> {
        let archive = self.archive_for_owner(id, Some(owner_window))?;
        squallz_core::inspect_archive_target(
            target,
            |path| archive_path_exists(&archive, path),
            &ControlToken::new(),
        )
    }

    fn list_entries_for_owner(
        &self,
        owner_window: Option<&str>,
        id: u64,
        page: usize,
        page_size: usize,
        dir_prefix: &str,
        filter: Option<&str>,
    ) -> Result<Page, FormatError> {
        let archive = self.archive_for_owner(id, owner_window)?;
        Ok(page_level(
            &archive,
            page,
            page_size.max(1),
            dir_prefix,
            filter,
        ))
    }

    /// Pages case-insensitive matches across every path in a cached archive.
    /// Exact and prefix file-name matches are returned before broader path
    /// matches while preserving a stable path order inside each group.
    pub fn search_entries(
        &self,
        id: u64,
        page: usize,
        page_size: usize,
        query: &str,
        generation: u64,
    ) -> Result<Option<Page>, FormatError> {
        self.search_entries_for_owner(None, id, page, page_size, query, generation)
    }

    pub(crate) fn search_entries_for_window(
        &self,
        owner_window: &str,
        id: u64,
        page: usize,
        page_size: usize,
        query: &str,
        generation: u64,
    ) -> Result<Option<Page>, FormatError> {
        self.search_entries_for_owner(Some(owner_window), id, page, page_size, query, generation)
    }

    fn search_entries_for_owner(
        &self,
        owner_window: Option<&str>,
        id: u64,
        page: usize,
        page_size: usize,
        query: &str,
        generation: u64,
    ) -> Result<Option<Page>, FormatError> {
        let archive = self.archive_for_owner(id, owner_window)?;
        let current = archive
            .search_generation
            .fetch_max(generation, Ordering::AcqRel);
        if generation < current {
            return Ok(None);
        }
        Ok(page_search(
            &archive,
            page,
            page_size.max(1),
            query,
            generation,
        ))
    }

    fn archive_for_owner(
        &self,
        id: u64,
        owner_window: Option<&str>,
    ) -> Result<Arc<CachedArchive>, FormatError> {
        let registry = lock_unpoisoned(&self.archives);
        if !owner_is_active(&registry, owner_window) {
            return Err(archive_unavailable());
        }
        match registry.archives.get(&id) {
            Some(archive) if archive.owner_window.as_deref() == owner_window => {
                Ok(Arc::clone(archive))
            }
            _ => Err(archive_unavailable()),
        }
    }

    fn ensure_owner_active(&self, owner_window: Option<&str>) -> Result<(), FormatError> {
        if owner_is_active(&lock_unpoisoned(&self.archives), owner_window) {
            Ok(())
        } else {
            Err(archive_unavailable())
        }
    }

    /// Revokes a WebView label and removes every browse handle it owns.
    /// Handles already pinned by an authorized queued job remain alive only
    /// through that job's private `ResolvedArchiveSource`.
    pub fn release_window(&self, owner_window: &str) -> usize {
        let archives = {
            let mut registry = lock_unpoisoned(&self.archives);
            registry.released_windows.insert(owner_window.to_owned());
            self.passwords.invalidate_pending();
            let ids = registry
                .archives
                .iter()
                .filter(|(_, archive)| archive.owner_window.as_deref() == Some(owner_window))
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| {
                    let archive = registry.archives.remove(&id)?;
                    archive.search_generation.store(u64::MAX, Ordering::Release);
                    Some(archive)
                })
                .collect::<Vec<_>>()
        };
        archives.len()
    }

    /// Prevents late archive-handle publication while shutdown is beginning.
    pub fn begin_shutdown(&self) {
        self.passwords.shutdown();
        let mut registry = lock_unpoisoned(&self.archives);
        registry.shutting_down = true;
        cancel_archive_searches(registry.archives.values());
    }

    /// Drains every browse handle after queued jobs have released their
    /// private source pins and before the preview workspace is removed.
    pub fn shutdown(&self) -> usize {
        self.passwords.shutdown();
        let archives = {
            let mut registry = lock_unpoisoned(&self.archives);
            registry.shutting_down = true;
            cancel_archive_searches(registry.archives.values());
            registry
                .archives
                .drain()
                .map(|(_, archive)| archive)
                .collect::<Vec<_>>()
        };
        archives.len()
    }

    /// Session password for a path, if one was proven good earlier.
    pub fn password_for(&self, path: &Path) -> Option<Password> {
        self.passwords.get(&self.engine, path)
    }

    /// Verifies a password without adding another opened archive handle.
    pub fn verify_password(
        &self,
        path: &Path,
        password: &str,
        encoding: Option<&str>,
        limits: SafetyLimits,
        control: &ControlToken,
    ) -> Result<bool, FormatError> {
        let open_opts = OpenOptions {
            password: Some(Password::new(password)),
            encoding_override: encoding.map(str::to_owned),
        };
        self.engine
            .verify_password(path, &open_opts, limits, control)
    }

    pub(crate) fn password_attempt(
        &self,
        path: &Path,
        control: &ControlToken,
    ) -> Option<PasswordAttempt> {
        self.passwords.begin(&self.engine, path, control)
    }

    /// Call only after successful decryption, using the attempt captured
    /// before that work. Forgetting or source replacement prevents publication.
    pub(crate) fn remember_password(
        &self,
        attempt: &PasswordAttempt,
        password: &str,
        control: &ControlToken,
    ) {
        self.passwords
            .remember(&self.engine, attempt, password, control);
    }

    /// Removes a session password, used when the user forgets a saved secret.
    pub fn forget_password(&self, path: &Path) {
        self.passwords.forget(path);
    }

    #[cfg(test)]
    pub(crate) fn cached_password_paths(&self) -> Vec<PathBuf> {
        self.passwords.paths()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn archive_source_for_id(id: u64) -> String {
    format!("{ARCHIVE_SOURCE_PREFIX}{id}")
}

fn archive_unavailable() -> FormatError {
    FormatError::Other(ARCHIVE_UNAVAILABLE_DETAIL.to_owned())
}

fn owner_is_active(registry: &ArchiveRegistry, owner_window: Option<&str>) -> bool {
    !registry.shutting_down
        && owner_window.is_none_or(|owner| !registry.released_windows.contains(owner))
}

fn cancel_archive_searches<'a>(archives: impl IntoIterator<Item = &'a Arc<CachedArchive>>) {
    for archive in archives {
        archive.search_generation.store(u64::MAX, Ordering::Release);
    }
}

fn archive_file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn archive_display_name(file_name: &str) -> String {
    match split_volume_name(file_name) {
        Some((base, _)) => base.to_owned(),
        None => file_name.to_owned(),
    }
}

fn path_file_names<'a>(
    paths: impl IntoIterator<Item = &'a PathBuf>,
    control: &ControlToken,
) -> Result<Vec<String>, FormatError> {
    let mut names = Vec::new();
    for path in paths {
        control.checkpoint()?;
        names.push(archive_file_name(path));
    }
    Ok(names)
}

struct EncodingDiagnostics {
    non_utf8_name_count: usize,
    garbled_count: usize,
    suggested: Option<String>,
    override_label: Option<String>,
}

fn encoding_diagnostics(
    entries: &[EntryMeta],
    override_label: Option<&str>,
    control: &ControlToken,
) -> Result<EncodingDiagnostics, FormatError> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut non_utf8_name_count = 0;
    let mut garbled_count = 0;
    for meta in entries {
        control.checkpoint()?;
        if meta.path.display.contains('\u{FFFD}') {
            garbled_count += 1;
        }
        if !meta.path.encoding.eq_ignore_ascii_case("utf-8") {
            non_utf8_name_count += 1;
            *counts.entry(meta.path.encoding.to_owned()).or_default() += 1;
        }
    }
    let suggested = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(encoding, _)| encoding);
    Ok(EncodingDiagnostics {
        non_utf8_name_count,
        garbled_count,
        suggested,
        override_label: override_label.map(str::to_owned),
    })
}

/// Normalizes an entry display path: `\` → `/`, no leading `/`, directories
/// (explicit or implied) end with `/`. Shared with the job layer so display
/// path selections match the rows shown in the list.
pub(crate) fn normalized_entry_path(meta: &EntryMeta) -> String {
    normalized_entry_path_ref(meta).into_owned()
}

pub(crate) fn normalized_entry_path_ref(meta: &EntryMeta) -> Cow<'_, str> {
    meta.path
        .normalized_display(matches!(meta.entry_type, EntryType::Dir))
}

fn add_pending_row(
    levels: &mut HashMap<String, HashMap<Box<str>, PendingRow>>,
    parent: &str,
    name: &str,
    row: PendingRow,
) {
    let Some(level) = levels.get_mut(parent) else {
        let mut level = HashMap::new();
        level.insert(Box::from(name), row);
        levels.insert(parent.to_owned(), level);
        return;
    };

    if let Some(existing) = level.get_mut(name) {
        if matches!(existing, PendingRow::SynthesizedDir) && matches!(row, PendingRow::Entry { .. })
        {
            *existing = row;
        }
    } else {
        level.insert(Box::from(name), row);
    }
}

/// Builds the per-directory row index: every entry is attached to its parent
/// level, intermediate directories without explicit entries are synthesized,
/// each level is sorted directories-first then case-insensitively by name.
fn build_levels(
    entries: &[EntryMeta],
    control: &ControlToken,
) -> Result<HashMap<String, Vec<Row>>, FormatError> {
    let mut levels: HashMap<String, HashMap<Box<str>, PendingRow>> = HashMap::new();
    for (idx, meta) in entries.iter().enumerate() {
        control.checkpoint()?;
        let path = normalized_entry_path_ref(meta);
        let is_dir = path.ends_with('/');
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }

        // Synthesize intermediate directories.
        let mut segment_start = 0usize;
        for (slash_index, _) in trimmed.match_indices('/') {
            control.checkpoint()?;
            add_pending_row(
                &mut levels,
                &trimmed[..segment_start],
                &trimmed[segment_start..slash_index],
                PendingRow::SynthesizedDir,
            );
            segment_start = slash_index.saturating_add(1);
        }
        add_pending_row(
            &mut levels,
            &trimmed[..segment_start],
            &trimmed[segment_start..],
            PendingRow::Entry { index: idx, is_dir },
        );
    }

    let mut indexed = HashMap::with_capacity(levels.len());
    for (parent, rows) in levels {
        control.checkpoint()?;
        let mut sortable = Vec::with_capacity(rows.len());
        for (sequence, (name, pending)) in rows.into_iter().enumerate() {
            control.checkpoint()?;
            let folded_name = name.to_lowercase();
            let (is_dir, row) = match pending {
                PendingRow::Entry { index, is_dir } => (is_dir, Row::Entry { index, is_dir }),
                PendingRow::SynthesizedDir => (true, Row::SynthesizedDir(name)),
            };
            sortable.push(SortItem {
                key: (!is_dir, folded_name, sequence),
                value: row,
            });
        }
        let sorted = cancellable_sort(sortable, control)?;
        let mut rows = Vec::with_capacity(sorted.len());
        for (index, sortable) in sorted.into_iter().enumerate() {
            if index % INDEX_SORT_CHUNK_SIZE == 0 {
                control.checkpoint()?;
            }
            rows.push(sortable.value);
        }
        indexed.insert(parent, rows);
    }
    Ok(indexed)
}

fn build_search_matches(
    archive: &CachedArchive,
    query: &str,
    generation: u64,
) -> Option<Vec<SearchSource>> {
    let mut matches = Vec::new();
    let mut sequence = 0usize;
    for (parent, rows) in &archive.levels {
        for row in rows {
            if sequence.is_multiple_of(256)
                && archive.search_generation.load(Ordering::Acquire) != generation
            {
                return None;
            }
            let name = row.name(&archive.entries);
            let full_path = if row.is_dir() {
                format!("{parent}{name}/")
            } else {
                format!("{parent}{name}")
            };
            let folded_path = fold_archive_search_path(&full_path).into_boxed_str();
            let rank = match rank_folded_archive_path(&folded_path, query) {
                Some(rank) => rank,
                None => {
                    sequence += 1;
                    continue;
                }
            };
            let source = match row.entry_index() {
                Some(index) => SearchSource::Entry(index),
                None => SearchSource::SynthesizedDir(full_path.into_boxed_str()),
            };
            let source_key = match &source {
                SearchSource::Entry(index) => SearchSourceSortKey::Entry(*index),
                SearchSource::SynthesizedDir(path) => {
                    SearchSourceSortKey::SynthesizedDir(path.clone())
                }
            };
            matches.push(SortItem {
                key: (rank, folded_path, source_key, sequence),
                value: source,
            });
            sequence += 1;
        }
    }
    let sorted = cancellable_sort_with(matches, || {
        if archive.search_generation.load(Ordering::Acquire) == generation {
            Ok(())
        } else {
            Err(FormatError::Cancelled)
        }
    })
    .ok()?;
    let mut matches = Vec::with_capacity(sorted.len());
    for sortable in sorted {
        matches.push(sortable.value);
    }
    debug_assert!(matches.iter().all(|source| match source {
        SearchSource::Entry(index) => *index < archive.entries.len(),
        SearchSource::SynthesizedDir(_) => true,
    }));
    if archive.search_generation.load(Ordering::Acquire) != generation {
        return None;
    }
    Some(matches)
}

fn cancellable_sort<T: Ord>(values: Vec<T>, control: &ControlToken) -> Result<Vec<T>, FormatError> {
    cancellable_sort_with(values, || control.checkpoint())
}

fn cancellable_sort_with<T: Ord>(
    values: Vec<T>,
    mut checkpoint: impl FnMut() -> Result<(), FormatError>,
) -> Result<Vec<T>, FormatError> {
    checkpoint()?;
    if values.len() <= 1 {
        return Ok(values);
    }

    let chunk_count = values.len().div_ceil(INDEX_SORT_CHUNK_SIZE);
    let mut chunks = Vec::with_capacity(chunk_count);
    let mut remaining = values.into_iter();
    loop {
        checkpoint()?;
        let mut chunk: Vec<T> = remaining.by_ref().take(INDEX_SORT_CHUNK_SIZE).collect();
        if chunk.is_empty() {
            break;
        }
        chunk.sort_unstable();
        checkpoint()?;
        chunks.push(VecDeque::from(chunk));
    }

    let output_capacity = chunks.iter().map(VecDeque::len).sum();
    let mut heap = BinaryHeap::with_capacity(chunks.len());
    for (chunk_index, chunk) in chunks.iter_mut().enumerate() {
        if let Some(value) = chunk.pop_front() {
            heap.push(Reverse((value, chunk_index)));
        }
    }

    let mut sorted = Vec::with_capacity(output_capacity);
    while let Some(Reverse((value, chunk_index))) = heap.pop() {
        if sorted.len() % INDEX_SORT_CHUNK_SIZE == 0 {
            checkpoint()?;
        }
        sorted.push(value);
        if let Some(next) = chunks[chunk_index].pop_front() {
            heap.push(Reverse((next, chunk_index)));
        }
    }
    checkpoint()?;
    Ok(sorted)
}

fn resolve_archive_directory(archive: &CachedArchive, dir_prefix: &str) -> String {
    let mut prefix = dir_prefix.replace('\\', "/").trim_matches('/').to_owned();
    if !prefix.is_empty() {
        prefix.push('/');
    }
    while !prefix.is_empty() && !archive_directory_exists(archive, &prefix) {
        prefix.pop();
        let parent_end = prefix.rfind('/').map_or(0, |index| index + 1);
        prefix.truncate(parent_end);
    }
    prefix
}

fn archive_directory_exists(archive: &CachedArchive, prefix: &str) -> bool {
    if prefix.is_empty() || archive.levels.contains_key(prefix) {
        return true;
    }
    archive_path_exists(archive, prefix)
}

/// Looks up an exact displayed path in the existing directory index.
fn archive_path_exists(archive: &CachedArchive, full_path: &str) -> bool {
    if full_path.is_empty() {
        return true;
    }
    let is_dir = full_path.ends_with('/');
    let path = full_path.strip_suffix('/').unwrap_or(full_path);
    let (parent, name) = path.rsplit_once('/').map_or(("", path), |(parent, name)| {
        (&full_path[..parent.len() + 1], name)
    });
    let Some(rows) = archive.levels.get(parent) else {
        return false;
    };
    let folded = name.to_lowercase();
    let start = rows.partition_point(|row| {
        (row.is_dir() && !is_dir)
            || (row.is_dir() == is_dir && row.name(&archive.entries).to_lowercase() < folded)
    });
    rows[start..]
        .iter()
        .take_while(|row| {
            row.is_dir() == is_dir && row.name(&archive.entries).to_lowercase() == folded
        })
        .any(|row| row.name(&archive.entries) == name)
}

/// Slices one page out of a directory level.
fn page_level(
    archive: &CachedArchive,
    page: usize,
    page_size: usize,
    dir_prefix: &str,
    filter: Option<&str>,
) -> Page {
    let rows = match archive.levels.get(dir_prefix) {
        Some(rows) => rows.as_slice(),
        None => &[],
    };
    let filter_lc = filter
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_lowercase);
    let requested_start = page.saturating_mul(page_size);
    let (total, items) = match filter_lc {
        None => {
            let total = rows.len();
            let start = requested_start.min(total);
            let end = start.saturating_add(page_size).min(total);
            let items = rows[start..end]
                .iter()
                .map(|row| entry_dto_for_row(archive, dir_prefix, row))
                .collect();
            (total, items)
        }
        Some(filter_lc) => {
            let mut total = 0usize;
            let page_capacity = rows
                .len()
                .saturating_sub(requested_start.min(rows.len()))
                .min(page_size);
            let mut items = Vec::with_capacity(page_capacity);
            for row in rows {
                if !row
                    .name(&archive.entries)
                    .to_lowercase()
                    .contains(&filter_lc)
                {
                    continue;
                }
                if total >= requested_start && items.len() < page_size {
                    items.push(entry_dto_for_row(archive, dir_prefix, row));
                }
                total = total.saturating_add(1);
            }
            (total, items)
        }
    };
    Page { total, page, items }
}

fn entry_dto_for_row(archive: &CachedArchive, dir_prefix: &str, row: &Row) -> EntryDto {
    match row {
        Row::Entry { index, .. } => {
            let meta = &archive.entries[*index];
            let full = normalized_entry_path(meta);
            let name = entry_base_name(&full);
            EntryDto::from_meta(meta, full, name)
        }
        Row::SynthesizedDir(name) => {
            let full = format!("{dir_prefix}{name}/");
            EntryDto::synthesized_dir(full, name.as_ref().to_owned())
        }
    }
}

fn page_search(
    archive: &CachedArchive,
    page: usize,
    page_size: usize,
    query: &str,
    generation: u64,
) -> Option<Page> {
    let query = fold_archive_search_query(query);
    if archive.search_generation.load(Ordering::Acquire) != generation {
        return None;
    }
    if query.is_empty() {
        return Some(Page {
            total: 0,
            page,
            items: Vec::new(),
        });
    }

    let (total, page_matches) = {
        let mut cache = lock_unpoisoned(&archive.search_cache);
        if cache.query != query {
            let matches = build_search_matches(archive, &query, generation)?;
            cache.query.clone_from(&query);
            cache.matches = matches;
        }

        let total = cache.matches.len();
        let start = page.saturating_mul(page_size).min(total);
        let end = start.saturating_add(page_size).min(total);
        (total, cache.matches[start..end].to_vec())
    };
    let items = page_matches
        .iter()
        .map(|source| match source {
            SearchSource::Entry(entry_index) => {
                let meta = &archive.entries[*entry_index];
                let full_path = normalized_entry_path(meta);
                EntryDto::from_meta(meta, full_path.clone(), entry_base_name(&full_path))
            }
            SearchSource::SynthesizedDir(path) => {
                EntryDto::synthesized_dir(path.as_ref().to_owned(), entry_base_name(path))
            }
        })
        .collect();
    if archive.search_generation.load(Ordering::Acquire) != generation {
        return None;
    }
    Some(Page { total, page, items })
}

fn entry_base_name(path: &str) -> String {
    entry_base_name_ref(path).to_owned()
}

fn entry_base_name_ref(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_core::api::{
        CompressionLevel, ControlToken, CreateOptions, EntryPath, NoProgress, Password,
    };

    fn make_zip(dir: &Path, names: &[&str]) -> PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for name in names {
            let p = src.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, name.as_bytes()).unwrap();
        }
        let dest = dir.join("test.zip");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .create(
                &dest,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        dest
    }

    fn make_header_encrypted_7z(dir: &Path) -> PathBuf {
        let src = dir.join("secret-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("secret.txt"), b"classified").unwrap();
        let dest = dir.join("secret.7z");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .create(
                &dest,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    password: Some(Password::new("secret")),
                    encrypt_filenames: true,
                    ..CreateOptions::default()
                },
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        dest
    }

    fn file_meta(path: &str, size: u64) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::File,
            size,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn cached_archive(entries: Vec<EntryMeta>, generation: u64) -> CachedArchive {
        let control = ControlToken::default();
        let levels = build_levels(&entries, &control).unwrap();
        CachedArchive {
            owner_window: None,
            source_path: PathBuf::from("test.zip"),
            display_path: "test.zip".to_owned(),
            display_name: "test.zip".to_owned(),
            read_only: false,
            entries,
            levels,
            search_cache: Mutex::new(SearchCache::default()),
            search_generation: AtomicU64::new(generation),
            _owned_temp: None,
        }
    }

    #[test]
    fn archive_path_checks_use_exact_names_types_and_unloaded_directory_rows() {
        let mut empty = file_meta("empty/", 0);
        empty.entry_type = EntryType::Dir;
        let mut entries = (0..1200)
            .map(|index| file_meta(&format!("docs/item-{index:04}.txt"), 1))
            .collect::<Vec<_>>();
        entries.extend([empty, file_meta("docs/Case.txt", 1), file_meta("plain", 1)]);
        let archive = cached_archive(entries, 0);
        for path in [
            "",
            "docs/",
            "empty/",
            "plain",
            "docs/Case.txt",
            "docs/item-1199.txt",
        ] {
            assert!(archive_path_exists(&archive, path), "{path}");
        }
        for path in [
            "docs",
            "empty",
            "docs/case.txt",
            "docs/Case.txt/",
            "missing/",
            "docs/item-1200.txt",
        ] {
            assert!(!archive_path_exists(&archive, path), "{path}");
        }
    }

    #[test]
    fn move_preflight_checks_unloaded_names_and_preserves_zip_contents() {
        let dir = temp_dir("move-full-index");
        let mut names = vec![
            "from/report.txt".to_owned(),
            "destination/report.txt".to_owned(),
            "destination/report copy.txt".to_owned(),
        ];
        names.extend((2..=1200).map(|index| format!("destination/report copy {index}.txt")));
        let zip = make_zip(&dir, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let state = AppState::new();
        let archive = state
            .open_archive_for_window("move-window", &zip, None, None)
            .unwrap();
        let page = state
            .list_entries_for_window("move-window", archive.id, 0, 10, "src/from/", None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let paths = vec![page.items[0].path.clone()];
        assert!(state
            .plan_archive_move_for_window("other-window", archive.id, &paths, "src/destination/")
            .is_err());
        let plan = state
            .plan_archive_move_for_window("move-window", archive.id, &paths, "src/destination/")
            .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(
            plan.items[0].conflict,
            Some(squallz_core::ArchiveMoveConflict::ExistingTarget)
        );
        let target = plan.items[0].keep_both_to.as_deref().unwrap();
        assert_eq!(target, "src/destination/report copy 1201.txt");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .update(
                &zip,
                &[squallz_core::api::UpdateOp::Rename {
                    from: squallz_core::api::EntrySelection::Display(paths[0].clone()),
                    to: EntryPath::from_utf8(target),
                }],
                &squallz_core::api::UpdateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let extracted = dir.join("extracted");
        engine
            .extract(
                &zip,
                &extracted,
                None,
                &OpenOptions::default(),
                &squallz_core::api::ExtractOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_eq!(
            std::fs::read(extracted.join(target)).unwrap(),
            b"from/report.txt"
        );
        for name in &names[1..] {
            assert_eq!(
                std::fs::read(extracted.join("src").join(name)).unwrap(),
                name.as_bytes()
            );
        }
        assert!(!extracted.join(&paths[0]).exists());
        state.close_archive_for_window("move-window", archive.id);
        assert!(state
            .plan_archive_move_for_window("move-window", archive.id, &paths, "src/destination/")
            .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edit_target_inspection_covers_unloaded_entries_and_real_folder_rename_updates() {
        let dir = temp_dir("edit-target-index");
        let mut names = vec![
            "source.txt".to_owned(),
            "blocked".to_owned(),
            "implicit/child.txt".to_owned(),
        ];
        names.extend((0..1000).map(|index| format!("docs/item-{index:04}.txt")));
        let zip = make_zip(&dir, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let state = AppState::new();
        let archive = state
            .open_archive_for_window("edit-window", &zip, None, None)
            .unwrap();
        let page = state
            .list_entries_for_window("edit-window", archive.id, 0, 500, "src/docs/", None)
            .unwrap();
        assert_eq!(page.items.len(), 500);
        assert!(page
            .items
            .iter()
            .all(|item| item.path != "src/docs/item-0999.txt"));
        let inspect = |path| {
            state
                .inspect_archive_target_for_window("edit-window", archive.id, path)
                .unwrap()
        };
        for path in [
            "src/docs/item-0999.txt",
            "src/docs/item-0999.txt/",
            "src/implicit",
            "src/implicit/",
        ] {
            assert!(inspect(path).exists, "{path}");
        }
        assert_eq!(
            inspect("src/blocked/child/").blocked_parent.as_deref(),
            Some("src/blocked")
        );
        assert!(!inspect("src/new-folder/").exists);
        assert!(!inspect("src/new-folder/renamed.txt").exists);
        assert!(state
            .inspect_archive_target_for_window("other-window", archive.id, "src/new-folder/")
            .is_err());
        let engine = Engine::new(squallz_formats::registry());
        engine
            .update(
                &zip,
                &[
                    squallz_core::api::UpdateOp::AddDir {
                        path: EntryPath::from_utf8("src/new-folder/"),
                    },
                    squallz_core::api::UpdateOp::Rename {
                        from: squallz_core::api::EntrySelection::Display("src/source.txt".into()),
                        to: EntryPath::from_utf8("src/new-folder/renamed.txt"),
                    },
                ],
                &squallz_core::api::UpdateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        let extracted = dir.join("extracted");
        engine
            .extract(
                &zip,
                &extracted,
                None,
                &OpenOptions::default(),
                &squallz_core::api::ExtractOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert!(extracted.join("src/new-folder").is_dir());
        assert_eq!(
            std::fs::read(extracted.join("src/new-folder/renamed.txt")).unwrap(),
            b"source.txt"
        );
        assert!(!extracted.join("src/source.txt").exists());
        for name in &names[1..] {
            assert_eq!(
                std::fs::read(extracted.join("src").join(name)).unwrap(),
                name.as_bytes()
            );
        }
        state.close_archive_for_window("edit-window", archive.id);
        assert!(state
            .inspect_archive_target_for_window("edit-window", archive.id, "src/new-folder/")
            .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_directory_resolution_preserves_empty_and_implicit_directories() {
        let mut empty = file_meta("empty", 0);
        empty.entry_type = EntryType::Dir;
        let mut uppercase_empty = file_meta("EMPTY/", 0);
        uppercase_empty.entry_type = EntryType::Dir;
        let mut nested_empty = file_meta("docs/vacant/", 0);
        nested_empty.entry_type = EntryType::Dir;
        let archive = cached_archive(
            vec![
                file_meta("docs/kept/file.txt", 1),
                file_meta(r"资料\有效\文件.txt", 1),
                file_meta("Empty", 1),
                empty,
                uppercase_empty,
                nested_empty,
            ],
            0,
        );
        for (requested, expected) in [
            ("docs/kept/", "docs/kept/"),
            ("docs/missing/deep/", "docs/"),
            ("docs/kept/file.txt/", "docs/kept/"),
            ("empty/", "empty/"),
            ("empty/missing/", "empty/"),
            ("EMPTY/", "EMPTY/"),
            ("Empty/", ""),
            ("EmPtY/", ""),
            ("docs/vacant/", "docs/vacant/"),
            (r"\资料\有效\missing\", "资料/有效/"),
            ("missing/", ""),
            ("/", ""),
            ("", ""),
        ] {
            assert_eq!(resolve_archive_directory(&archive, requested), expected);
        }
        assert_eq!(page_level(&archive, 0, 10, "empty/", None).total, 0);
        assert_eq!(
            resolve_archive_directory(&cached_archive(vec![], 0), "old/"),
            ""
        );
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn make_raw_name_zip(dir: &Path, raw_name: &[u8]) -> PathBuf {
        let data = b"non-utf8 name";
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_len = raw_name.len() as u16;
        let mut out = Vec::new();
        let offset = 0u32;

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(raw_name);
        out.extend_from_slice(data);

        let central_offset = out.len() as u32;
        out.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(raw_name);

        let central_size = out.len() as u32 - central_offset;
        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());

        let dest = dir.join("gbk-names.zip");
        std::fs::write(&dest, out).unwrap();
        dest
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta(path: EntryPath) -> EntryMeta {
        EntryMeta {
            path,
            entry_type: EntryType::File,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn encoding_diagnostics_reports_non_utf8_and_garbled_names() {
        let entries = vec![
            meta(EntryPath::from_utf8("plain.txt")),
            meta(EntryPath::from_raw(
                vec![0xc4, 0xe3],
                "你好.txt".to_owned(),
                "GBK",
            )),
            meta(EntryPath::from_raw(
                vec![0xce, 0xc4],
                "文件.txt".to_owned(),
                "GBK",
            )),
            meta(EntryPath::from_raw(
                vec![0xff],
                "bad\u{FFFD}.txt".to_owned(),
                "windows-1252",
            )),
        ];

        let diag = encoding_diagnostics(&entries, Some("gbk"), &ControlToken::default()).unwrap();
        assert_eq!(diag.non_utf8_name_count, 3);
        assert_eq!(diag.garbled_count, 1);
        assert_eq!(diag.suggested.as_deref(), Some("GBK"));
        assert_eq!(diag.override_label.as_deref(), Some("gbk"));
    }

    #[test]
    fn open_archive_caches_and_reports_info() {
        let dir = temp_dir("open");
        let zip = make_zip(&dir, &["a.txt", "b/c.txt", "b/d/e.txt"]);
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();
        assert_eq!(info.format, "zip");
        assert_eq!(info.structure, "complete");
        assert_eq!(info.name, "test.zip");
        assert!(info.volumes.is_none());
        assert!(info.entry_count >= 3, "files (and maybe dirs) listed");

        // Root level: the single "src" directory.
        let page = state.list_entries(info.id, 0, 500, "", None).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].path, "src/");
        assert_eq!(page.items[0].entry_type, "dir");

        // src/: directory "b" sorts before file "a.txt".
        let page = state.list_entries(info.id, 0, 500, "src/", None).unwrap();
        let names: Vec<&str> = page.items.iter().map(|e| e.display.as_str()).collect();
        assert_eq!(names, vec!["b", "a.txt"]);
        assert_eq!(page.items[0].path, "src/b/");
        assert_eq!(page.items[1].path, "src/a.txt");

        // Unknown handle is a structured error.
        assert!(state.list_entries(999, 0, 500, "", None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_archive_marks_zip_local_header_recovery_views() {
        let dir = temp_dir("open-recovered-zip");
        let zip = make_zip(&dir, &["recoverable.txt"]);
        let mut bytes = std::fs::read(&zip).unwrap();
        let central_start = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory exists");
        bytes.truncate(central_start);
        std::fs::write(&zip, bytes).unwrap();

        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();

        assert_eq!(info.format, "zip");
        assert_eq!(info.structure, "zip_local_headers_recovered");
        assert!(info.entry_count >= 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_archive_open_does_not_publish_a_cache_handle() {
        let dir = temp_dir("cancelled-open");
        let zip = make_zip(&dir, &["one.txt", "nested/two.txt"]);
        let state = AppState::new();
        let control = ControlToken::new();
        control.cancel();

        let error = state
            .open_archive_for_window_with_entry_limit_and_control(
                "window-a",
                &zip,
                None,
                None,
                SafetyLimits::default().max_entries,
                control.as_ref(),
            )
            .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert!(lock_unpoisoned(&state.archives).archives.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_entry_limit_is_checked_before_publishing_a_cache_handle() {
        let dir = temp_dir("entry-limited-open");
        let zip = make_zip(&dir, &["one.txt", "nested/two.txt"]);
        let state = AppState::new();

        let error = state
            .open_archive_for_window_with_entry_limit_and_control(
                "window-a",
                &zip,
                None,
                None,
                1,
                &ControlToken::default(),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::ResourceLimitExceeded(detail)
                if detail == "archive contains more than 1 entries"
        ));
        assert!(lock_unpoisoned(&state.archives).archives.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_open_index_builders_honor_cancellation() {
        let entries = vec![file_meta("one.txt", 1), file_meta("nested/two.txt", 2)];
        let cancelled = ControlToken::new();
        cancelled.cancel();

        assert!(matches!(
            encoding_diagnostics(&entries, None, cancelled.as_ref()),
            Err(FormatError::Cancelled)
        ));
        assert!(matches!(
            build_levels(&entries, cancelled.as_ref()),
            Err(FormatError::Cancelled)
        ));
    }

    #[test]
    fn cancellable_sort_merges_multiple_sorted_chunks() {
        let values: Vec<usize> = (0..INDEX_SORT_CHUNK_SIZE * 2 + 17).rev().collect();
        let sorted = cancellable_sort(values, &ControlToken::default()).unwrap();

        assert_eq!(sorted.len(), INDEX_SORT_CHUNK_SIZE * 2 + 17);
        assert!(sorted.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn cancellable_sort_stops_after_a_bounded_chunk_sort() {
        #[derive(Clone)]
        struct CancellingValue {
            value: usize,
            comparisons: Arc<std::sync::atomic::AtomicUsize>,
            control: Arc<ControlToken>,
        }

        impl PartialEq for CancellingValue {
            fn eq(&self, other: &Self) -> bool {
                self.value == other.value
            }
        }

        impl Eq for CancellingValue {}

        impl PartialOrd for CancellingValue {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for CancellingValue {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                if self.comparisons.fetch_add(1, Ordering::Relaxed) == 100 {
                    self.control.cancel();
                }
                self.value.cmp(&other.value)
            }
        }

        let control = ControlToken::new();
        let comparisons = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let values = (0..INDEX_SORT_CHUNK_SIZE * 2)
            .rev()
            .map(|value| CancellingValue {
                value,
                comparisons: Arc::clone(&comparisons),
                control: Arc::clone(&control),
            })
            .collect();

        assert!(matches!(
            cancellable_sort(values, control.as_ref()),
            Err(FormatError::Cancelled)
        ));
        assert!(comparisons.load(Ordering::Relaxed) < INDEX_SORT_CHUNK_SIZE * 2);
    }

    #[test]
    fn archive_index_rows_keep_a_compact_inline_layout() {
        let three_words = std::mem::size_of::<usize>() * 3;

        assert!(std::mem::size_of::<Row>() <= three_words);
        assert!(std::mem::size_of::<SearchSource>() <= three_words);
    }

    #[test]
    fn large_single_directory_index_keeps_complete_sorted_rows() {
        let entries: Vec<EntryMeta> = (0..50_000)
            .rev()
            .map(|index| file_meta(&format!("root/file-{index:05}.txt"), 1))
            .collect();
        let archive = cached_archive(entries, 0);
        let rows = archive.levels.get("root/").unwrap();
        assert_eq!(rows.len(), 50_000);
        assert_eq!(
            rows.first().map(|row| row.name(&archive.entries)),
            Some("file-00000.txt")
        );
        assert_eq!(
            rows.last().map(|row| row.name(&archive.entries)),
            Some("file-49999.txt")
        );

        assert!(lock_unpoisoned(&archive.search_cache).matches.is_empty());

        let exact = page_search(&archive, 0, 10, "file-49999.txt", 0)
            .expect("current search generation should return a page");
        assert_eq!(exact.total, 1);
        assert_eq!(exact.items[0].path, "root/file-49999.txt");
        assert_eq!(lock_unpoisoned(&archive.search_cache).matches.len(), 1);

        let page = page_search(&archive, 0, 10, "file-", 0)
            .expect("current search generation should return a page");
        assert_eq!(page.total, 50_000);
        assert_eq!(page.items.len(), 10);
        assert_eq!(lock_unpoisoned(&archive.search_cache).matches.len(), 50_000);
    }

    #[test]
    fn native_source_set_names_keep_archive_order() {
        let source_set = squallz_core::api::ArchiveSourceSet::from_ordered_members(vec![
            PathBuf::from("/archives/sample.part001.rar"),
            PathBuf::from("/archives/sample.part002.rar"),
            PathBuf::from("/archives/sample.part003.rar"),
        ])
        .unwrap();

        assert_eq!(
            path_file_names(source_set.members().iter(), &ControlToken::default()).unwrap(),
            vec![
                "sample.part001.rar",
                "sample.part002.rar",
                "sample.part003.rar"
            ]
        );
    }

    #[test]
    fn window_archive_handles_are_isolated_and_revoked_with_their_owner() {
        let dir = temp_dir("window-owner-isolation");
        let zip = make_zip(&dir, &["one.txt", "nested/two.txt"]);
        let state = AppState::new();
        let first = state
            .open_archive_for_window("window-a", &zip, None, None)
            .unwrap();
        let second = state
            .open_archive_for_window("window-b", &zip, None, None)
            .unwrap();
        let first_pin = state.archive_for_owner(first.id, Some("window-a")).unwrap();

        let unavailable = state
            .list_entries_for_window("window-b", first.id, 0, 10, "", None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            unavailable,
            state
                .missing_archive_paths_for_window("window-b", first.id, &["src/".to_owned()])
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            state
                .missing_archive_paths_for_window(
                    "window-a",
                    first.id,
                    &[
                        "src/".to_owned(),
                        "src/nested/".to_owned(),
                        "missing".to_owned()
                    ],
                )
                .unwrap(),
            vec!["missing"]
        );
        assert_eq!(
            unavailable,
            state
                .resolve_archive_directory_for_window("window-b", first.id, "src/nested/")
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            state
                .resolve_archive_directory_for_window("window-a", first.id, "src/nested/removed/")
                .unwrap(),
            "src/nested/"
        );
        assert_eq!(
            unavailable,
            state
                .list_entries_for_window("window-b", u64::MAX, 0, 10, "", None)
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            unavailable,
            state
                .list_entries(first.id, 0, 10, "", None)
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            unavailable,
            state
                .search_entries_for_window("window-b", first.id, 0, 10, "one", 1)
                .unwrap_err()
                .to_string()
        );
        assert_eq!(
            unavailable,
            state
                .cancel_search_for_window("window-b", first.id, 2)
                .unwrap_err()
                .to_string()
        );

        let source = archive_source_for_id(first.id);
        let foreign_source_error = match state.resolve_archive_source(&source, Some("window-b")) {
            Ok(_) => panic!("a foreign owner resolved another window's archive"),
            Err(error) => error.to_string(),
        };
        let ownerless_source_error = match state.resolve_archive_source(&source, None) {
            Ok(_) => panic!("the ownerless API resolved a window-owned archive"),
            Err(error) => error.to_string(),
        };
        let malformed_source_error =
            match state.resolve_archive_source("squallz-archive://invalid", Some("window-b")) {
                Ok(_) => panic!("a malformed archive source unexpectedly resolved"),
                Err(error) => error.to_string(),
            };
        assert_eq!(unavailable, foreign_source_error);
        assert_eq!(unavailable, ownerless_source_error);
        assert_eq!(unavailable, malformed_source_error);

        state.close_archive_for_window("window-b", first.id);
        assert!(state
            .list_entries_for_window("window-a", first.id, 0, 10, "", None)
            .is_ok());
        assert_eq!(state.release_window("window-a"), 1);
        assert_eq!(state.release_window("window-a"), 0);
        assert_eq!(
            first_pin.search_generation.load(Ordering::Acquire),
            u64::MAX
        );
        assert!(page_search(&first_pin, 0, 10, "one", 0).is_none());
        assert_eq!(
            unavailable,
            state
                .list_entries_for_window("window-a", first.id, 0, 10, "", None)
                .unwrap_err()
                .to_string()
        );
        assert!(state
            .open_archive_for_window("window-a", &zip, None, None)
            .is_err());
        assert!(state
            .list_entries_for_window("window-b", second.id, 0, 10, "", None)
            .is_ok());

        let second_pin = state
            .archive_for_owner(second.id, Some("window-b"))
            .unwrap();
        state.begin_shutdown();
        assert_eq!(
            second_pin.search_generation.load(Ordering::Acquire),
            u64::MAX
        );
        assert!(state
            .list_entries_for_window("window-b", second.id, 0, 10, "", None)
            .is_err());
        assert!(state.open_archive(&zip, None, None).is_err());
        assert_eq!(state.shutdown(), 1);
        assert_eq!(state.shutdown(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn supported_formats_include_sqz_container() {
        let state = AppState::new();
        let formats = state.engine.supported_formats();
        let sqz = formats.iter().find(|f| f.id == "sqz").expect("sqz format");
        assert_eq!(sqz.extensions, vec!["sqz"]);
        assert!(sqz.capabilities.can_create);
        assert!(sqz.capabilities.can_extract);
        assert!(sqz.capabilities.can_test);
        assert!(sqz.capabilities.can_split);
        let rar = formats.iter().find(|f| f.id == "rar").expect("rar format");
        assert_eq!(rar.extensions, vec!["rar", "cbr"]);
        assert!(!rar.capabilities.can_create);
        assert!(rar.capabilities.can_extract);
        assert!(rar.capabilities.can_test);
    }

    #[test]
    fn archive_format_label_comes_from_content_instead_of_misleading_suffix() {
        let dir = temp_dir("misleading-format");
        let zip = make_zip(&dir, &["hello.txt"]);
        let renamed = dir.join("backup.rar");
        std::fs::rename(zip, &renamed).unwrap();
        let state = AppState::new();
        let info = state.open_archive(&renamed, None, None).unwrap();
        assert_eq!(info.format, "zip");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_archive_reports_non_utf8_encoding_diagnostics() {
        let dir = temp_dir("encoding-info");
        let zip = make_raw_name_zip(
            &dir,
            &[
                209, 185, 203, 245, 206, 196, 188, 254, 214, 208, 206, 196, 195, 251, 179, 198,
                178, 226, 202, 212, 46, 116, 120, 116,
            ],
        );
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();
        assert_eq!(info.non_utf8_name_count, 1);
        assert_eq!(info.garbled_count, 0);
        assert_eq!(info.suggested_encoding.as_deref(), Some("GBK"));
        assert!(info.encoding_override.is_none());

        let info = state.open_archive(&zip, None, Some("gbk")).unwrap();
        assert_eq!(info.encoding_override.as_deref(), Some("gbk"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_archive_reuses_session_password_cache() {
        let dir = temp_dir("session-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();

        let err = state.open_archive(&archive, None, None).unwrap_err();
        assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");

        let info = state.open_archive(&archive, Some("secret"), None).unwrap();
        assert_eq!(info.format, "7z");
        assert_eq!(
            state.password_for(&archive).as_ref().map(Password::expose),
            Some("secret")
        );

        let reopened = state.open_archive(&archive, None, None).unwrap();
        assert_eq!(reopened.format, "7z");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn readable_directory_does_not_prove_a_supplied_password() {
        let dir = temp_dir("unverified-session-password");
        let archive = make_zip(&dir, &["report.txt"]);
        let state = AppState::new();
        state
            .open_archive(&archive, Some("unverified"), None)
            .unwrap();
        assert!(state.password_for(&archive).is_none());
        let source = dir.join("private.txt");
        std::fs::write(&source, b"encrypted payload").unwrap();
        for format in ["zip", "7z"] {
            let encrypted = dir.join(format!("data-encrypted.{format}"));
            state
                .engine
                .create(
                    &encrypted,
                    std::slice::from_ref(&source),
                    &CreateOptions {
                        password: Some(Password::new("secret")),
                        encrypt_filenames: false,
                        ..CreateOptions::default()
                    },
                    &NoProgress,
                    &ControlToken::default(),
                )
                .unwrap();
            for password in ["wrong", "secret"] {
                state
                    .open_archive(&encrypted, Some(password), None)
                    .unwrap();
                assert!(state.password_for(&encrypted).is_none());
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replacing_an_archive_invalidates_its_session_password() {
        let dir = temp_dir("replaced-session-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();
        state.open_archive(&archive, Some("secret"), None).unwrap();
        assert!(state.password_for(&archive).is_some());
        let replacement = dir.join("replacement.7z");
        std::fs::copy(&archive, &replacement).unwrap();
        std::fs::rename(replacement, &archive).unwrap();
        assert!(state.password_for(&archive).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_cache_lock_recovers_after_poison() {
        let state = std::sync::Arc::new(AppState::new());

        let archive_state = std::sync::Arc::clone(&state);
        assert!(std::thread::spawn(move || {
            let _guard = archive_state.archives.lock().unwrap();
            panic!("poison archive cache");
        })
        .join()
        .is_err());
        assert!(state.list_entries(404, 0, 10, "", None).is_err());
        state.close_archive(404);
    }

    #[test]
    fn list_entries_paginates_and_filters() {
        let dir = temp_dir("paging");
        let names: Vec<String> = (0..23).map(|i| format!("f{i:02}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let zip = make_zip(&dir, &refs);
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();

        // Page size 10 → pages of 10/10/3 under src/.
        let p0 = state.list_entries(info.id, 0, 10, "src/", None).unwrap();
        assert_eq!((p0.total, p0.items.len()), (23, 10));
        assert_eq!(p0.items[0].display, "f00.txt");
        let p2 = state.list_entries(info.id, 2, 10, "src/", None).unwrap();
        assert_eq!(p2.items.len(), 3);
        assert_eq!(p2.items[2].display, "f22.txt");
        // Out-of-range page is empty, not an error.
        let p9 = state.list_entries(info.id, 9, 10, "src/", None).unwrap();
        assert!(p9.items.is_empty());

        // Filter matches the base name, case-insensitively.
        let f = state
            .list_entries(info.id, 0, 10, "src/", Some("F1"))
            .unwrap();
        assert_eq!(f.total, 10); // f10..f19
        assert!(f.items.iter().all(|e| e.display.starts_with("f1")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn search_entries_matches_and_ranks_paths_across_the_archive() {
        let archive = cached_archive(
            vec![
                file_meta("alpha/report.txt", 3),
                file_meta("beta/Summary.pdf", 5),
                file_meta("deep/Quarter/plan.md", 7),
                file_meta("notes/summary.txt", 11),
            ],
            4,
        );

        let summaries = page_search(&archive, 0, 10, "  SUMMARY  ", 4)
            .expect("current search generation should return a page");
        assert_eq!(summaries.total, 2);
        assert_eq!(summaries.items[0].path, "beta/Summary.pdf");
        assert_eq!(summaries.items[0].display, "Summary.pdf");
        assert_eq!(summaries.items[1].path, "notes/summary.txt");

        let quarter = page_search(&archive, 0, 10, "quarter", 4)
            .expect("current search generation should return a page");
        assert_eq!(quarter.total, 2);
        assert_eq!(quarter.items[0].path, "deep/Quarter/");
        assert_eq!(quarter.items[0].entry_type, "dir");
        assert_eq!(quarter.items[1].path, "deep/Quarter/plan.md");

        let page = page_search(&archive, 1, 1, "summary", 4)
            .expect("cached current search should return another page");
        assert_eq!(page.total, 2);
        assert_eq!(page.page, 1);
        assert_eq!(page.items[0].path, "notes/summary.txt");
        assert!(page_search(&archive, 0, 10, "  ", 4)
            .expect("blank current search should return an empty page")
            .items
            .is_empty());
    }

    #[test]
    fn search_entries_rejects_stale_generations_and_saturates_page_bounds() {
        let state = AppState::new();
        lock_unpoisoned(&state.archives).archives.insert(
            42,
            Arc::new(cached_archive(
                vec![
                    file_meta("one/report.txt", 3),
                    file_meta("two/report.txt", 5),
                ],
                0,
            )),
        );

        let first = state
            .search_entries(42, 0, 10, "report", 8)
            .expect("known archive should be searchable")
            .expect("newest generation should return a page");
        assert_eq!(first.total, 2);
        assert!(state
            .search_entries(42, 0, 10, "report", 7)
            .expect("known archive should be searchable")
            .is_none());

        state
            .cancel_search(42, 9)
            .expect("known archive search should be cancellable");
        assert!(state
            .search_entries(42, 0, 10, "report", 8)
            .expect("known archive should remain available")
            .is_none());

        let page = state
            .search_entries(42, usize::MAX, usize::MAX, "report", 9)
            .expect("known archive should be searchable")
            .expect("current generation should return a page");
        assert_eq!(page.total, 2);
        assert_eq!(page.page, usize::MAX);
        assert!(page.items.is_empty());
    }

    #[test]
    fn search_order_is_stable_for_case_insensitive_path_collisions() {
        let archive = cached_archive(
            vec![
                file_meta("docs/README.md", 3),
                file_meta("docs/readme.md", 5),
            ],
            1,
        );

        let page = page_search(&archive, 0, 10, "readme.md", 1)
            .expect("current search generation should return a page");
        let paths: Vec<&str> = page.items.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, vec!["docs/README.md", "docs/readme.md"]);
    }

    #[test]
    fn normalized_entry_paths_borrow_the_common_case_and_preserve_edge_cases() {
        let absolute = file_meta("/folder/file.txt", 3);
        assert!(matches!(
            normalized_entry_path_ref(&absolute),
            Cow::Borrowed("folder/file.txt")
        ));

        let windows = file_meta(r"\folder\file.txt", 3);
        assert_eq!(normalized_entry_path(&windows), "folder/file.txt");
        assert!(matches!(
            normalized_entry_path_ref(&windows),
            Cow::Owned(path) if path == "folder/file.txt"
        ));

        let mut directory = file_meta("folder", 0);
        directory.entry_type = EntryType::Dir;
        assert_eq!(normalized_entry_path(&directory), "folder/");
        assert!(matches!(
            normalized_entry_path_ref(&directory),
            Cow::Owned(path) if path == "folder/"
        ));
    }

    #[test]
    fn levels_synthesize_intermediate_dirs() {
        // An archive listing only a deep file must still expose every level.
        let mut explicit_root = file_meta("a", 0);
        explicit_root.entry_type = EntryType::Dir;
        let metas = vec![file_meta("a/b/c.txt", 3), explicit_root];
        let levels = build_levels(&metas, &ControlToken::default()).unwrap();
        assert_eq!(levels.get("").unwrap().len(), 1);
        assert!(levels.get("").unwrap()[0].is_dir());
        assert_eq!(levels.get("").unwrap()[0].entry_index(), Some(1));
        assert_eq!(levels.get("a/").unwrap()[0].name(&metas), "b");
        assert_eq!(levels.get("a/b/").unwrap()[0].name(&metas), "c.txt");
    }
}
