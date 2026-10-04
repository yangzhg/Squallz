//! Private, bounded temporary files used by archive-entry preview.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use squallz_core::api::{ControlToken, FormatError};

use crate::nested::{write_archive_entry_limited, PREVIEW_ENTRY_TOO_LARGE_DETAIL};
use crate::preview_workspace::{PreviewFile, PreviewWorkspace};
use crate::state::AppState;
use squallz_core::lock_unpoisoned;

pub(crate) const MAX_PREVIEW_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PREVIEW_RESOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ACTIVE_PREVIEW_RESOURCES: usize = 8;
const MAX_RETAINED_EXTERNAL_SESSIONS: usize = 64;
const PREVIEW_ACTIVE_CAPACITY_DETAIL: &str = "preview active-session capacity is full";
const PREVIEW_RETAINED_CAPACITY_DETAIL: &str = "preview external-file capacity is full";
const PREVIEW_STORAGE_CAPACITY_DETAIL: &str = "preview temporary-file storage is full";
const PREVIEW_WORKSPACE_UNAVAILABLE_DETAIL: &str = "preview workspace is unavailable";
const PREVIEW_SESSION_UNAVAILABLE_DETAIL: &str = "preview session is no longer available";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreviewFailureKind {
    EntryTooLarge,
    ActiveCapacity,
    RetainedExternalCapacity,
    StorageCapacity,
    WorkspaceUnavailable,
    SessionUnavailable,
}

impl PreviewFailureKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::EntryTooLarge => "error.preview_entry_too_large",
            Self::ActiveCapacity => "error.preview_active_capacity",
            Self::RetainedExternalCapacity => "error.preview_retained_capacity",
            Self::StorageCapacity => "error.preview_storage_capacity",
            Self::WorkspaceUnavailable => "error.preview_workspace_unavailable",
            Self::SessionUnavailable => "error.preview_session_unavailable",
        }
    }
}

pub(crate) fn preview_failure_kind(error: &FormatError) -> Option<PreviewFailureKind> {
    let detail = match error {
        FormatError::ResourceLimitExceeded(detail) | FormatError::Other(detail) => detail.as_str(),
        _ => return None,
    };
    match detail {
        PREVIEW_ENTRY_TOO_LARGE_DETAIL => Some(PreviewFailureKind::EntryTooLarge),
        PREVIEW_ACTIVE_CAPACITY_DETAIL => Some(PreviewFailureKind::ActiveCapacity),
        PREVIEW_RETAINED_CAPACITY_DETAIL => Some(PreviewFailureKind::RetainedExternalCapacity),
        PREVIEW_STORAGE_CAPACITY_DETAIL => Some(PreviewFailureKind::StorageCapacity),
        PREVIEW_WORKSPACE_UNAVAILABLE_DETAIL => Some(PreviewFailureKind::WorkspaceUnavailable),
        PREVIEW_SESSION_UNAVAILABLE_DETAIL => Some(PreviewFailureKind::SessionUnavailable),
        _ => None,
    }
}

pub(crate) struct PreparedPreview {
    pub id: String,
    pub display_name: String,
    pub size: u64,
}

struct PreviewSession {
    owner: String,
    file: PreviewFile,
    size: u64,
    sequence: u64,
    sticky_external_pin: bool,
    pending_external_uses: usize,
    release_requested: bool,
}

impl PreviewSession {
    fn is_pinned(&self) -> bool {
        self.sticky_external_pin || self.pending_external_uses > 0
    }
}

#[derive(Default)]
struct PreviewResources {
    sessions: HashMap<String, PreviewSession>,
    in_flight_count: usize,
    in_flight_bytes: u64,
    lease_count: usize,
    lease_bytes: u64,
    owner_generations: HashMap<String, u64>,
    closing: bool,
}

impl PreviewResources {
    fn active_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| !session.sticky_external_pin)
            .count()
            .saturating_add(self.in_flight_count)
            .saturating_add(self.lease_count)
    }

    fn retained_or_pending_external_count(&self) -> usize {
        self.sessions
            .values()
            .filter(|session| session.sticky_external_pin || session.pending_external_uses > 0)
            .count()
    }

    fn bytes(&self) -> u64 {
        session_bytes(&self.sessions)
            .saturating_add(self.in_flight_bytes)
            .saturating_add(self.lease_bytes)
    }

    fn owner_generation(&self, owner: &str) -> u64 {
        self.owner_generations.get(owner).copied().unwrap_or(0)
    }
}

struct PreviewSessionShared {
    workspace: Option<PreviewWorkspace>,
    resources: Mutex<PreviewResources>,
    next_sequence: AtomicU64,
}

pub(crate) struct PreviewSessionManager {
    shared: Arc<PreviewSessionShared>,
}

/// Pins one external handoff until it succeeds or is dropped. A successful
/// handoff keeps the session file available until application cleanup.
pub(crate) struct PreviewExternalUse {
    shared: Arc<PreviewSessionShared>,
    id: String,
    owner: String,
    path: PathBuf,
    completed: bool,
}

/// A capacity reservation acquired before any preview plaintext is written.
/// Dropping it releases the reserved slot and bytes.
pub(crate) struct PreviewResourceReservation {
    shared: Arc<PreviewSessionShared>,
    owner: String,
    owner_generation: u64,
    active: bool,
}

/// Keeps a persistent nested archive inside the global preview resource
/// budget until its cached archive is closed.
pub(crate) struct PreviewResourceLease {
    shared: Arc<PreviewSessionShared>,
    size: u64,
    active: bool,
}

impl PreviewSessionManager {
    pub fn new() -> io::Result<Self> {
        Self::new_in(&std::env::temp_dir())
    }

    fn new_in(base: &Path) -> io::Result<Self> {
        Ok(Self::from_workspace(Some(PreviewWorkspace::create_in(
            base,
        )?)))
    }

    /// Builds a non-fatal placeholder when the private workspace cannot be
    /// created. Only preview commands fail; the rest of the application stays
    /// available.
    pub(crate) fn unavailable() -> Self {
        Self::from_workspace(None)
    }

    fn from_workspace(workspace: Option<PreviewWorkspace>) -> Self {
        Self {
            shared: Arc::new(PreviewSessionShared {
                workspace,
                resources: Mutex::new(PreviewResources::default()),
                next_sequence: AtomicU64::new(1),
            }),
        }
    }

    pub(crate) fn reserve(&self, owner: &str) -> Result<PreviewResourceReservation, FormatError> {
        if self.shared.workspace.is_none() {
            return Err(preview_unavailable());
        }

        let owner_generation = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            if resources.closing {
                return Err(preview_unavailable());
            }
            loop {
                let active_capacity_full = resources.active_count() >= MAX_ACTIVE_PREVIEW_RESOURCES;
                let storage_capacity_full =
                    resources.bytes().saturating_add(MAX_PREVIEW_ENTRY_BYTES)
                        > MAX_PREVIEW_RESOURCE_BYTES;
                if !active_capacity_full && !storage_capacity_full {
                    break;
                }
                let candidate = resources
                    .sessions
                    .iter()
                    .filter(|(_, session)| !session.is_pinned())
                    .min_by_key(|(_, session)| session.sequence)
                    .map(|(id, _)| id.clone());
                let Some(candidate_id) = candidate else {
                    return Err(if storage_capacity_full {
                        preview_storage_capacity()
                    } else {
                        preview_active_capacity()
                    });
                };
                if let Some(evicted) = resources.sessions.remove(&candidate_id) {
                    close_session_file(evicted)?;
                }
            }
            resources.in_flight_count = resources.in_flight_count.saturating_add(1);
            resources.in_flight_bytes = resources
                .in_flight_bytes
                .saturating_add(MAX_PREVIEW_ENTRY_BYTES);
            resources.owner_generation(owner)
        };

        Ok(PreviewResourceReservation {
            shared: Arc::clone(&self.shared),
            owner: owner.to_owned(),
            owner_generation,
            active: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_archive_entry(
        &self,
        owner: &str,
        state: &AppState,
        outer_path: &Path,
        entry_path: &str,
        password: Option<&str>,
        encoding: Option<&str>,
        control: &ControlToken,
    ) -> Result<PreparedPreview, FormatError> {
        let reservation = self.reserve(owner)?;
        let display_name = preview_display_name(entry_path);
        let (file, mut pending) = reservation.create_preview_file(&display_name)?;

        let prepared = write_archive_entry_limited(
            state,
            outer_path,
            entry_path,
            password,
            encoding,
            &mut pending,
            MAX_PREVIEW_ENTRY_BYTES,
            control,
        )?;
        pending.flush()?;
        drop(pending);
        control.checkpoint()?;

        let id = file.id().to_owned();
        let sequence = self.shared.next_sequence.fetch_add(1, Ordering::Relaxed);
        reservation.into_session(
            id.clone(),
            PreviewSession {
                owner: owner.to_owned(),
                file,
                size: prepared.size,
                sequence,
                sticky_external_pin: false,
                pending_external_uses: 0,
                release_requested: false,
            },
        )?;
        prepared.remember_password(state, control);
        Ok(PreparedPreview {
            id,
            display_name,
            size: prepared.size,
        })
    }

    pub fn begin_external_use(
        &self,
        id: &str,
        owner: &str,
    ) -> Result<PreviewExternalUse, FormatError> {
        let (root, path) = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            if resources.closing {
                return Err(session_unavailable());
            }
            let session = session_for_owner(&resources.sessions, id, owner)?;
            let needs_external_slot =
                !session.sticky_external_pin && session.pending_external_uses == 0;
            if needs_external_slot
                && resources.retained_or_pending_external_count() >= MAX_RETAINED_EXTERNAL_SESSIONS
            {
                return Err(preview_retained_capacity());
            }
            let session = session_for_owner_mut(&mut resources.sessions, id, owner)?;
            session.pending_external_uses = session.pending_external_uses.saturating_add(1);
            (
                session.file.root_path().to_path_buf(),
                session.file.path().to_path_buf(),
            )
        };
        let mut external_use = PreviewExternalUse {
            shared: Arc::clone(&self.shared),
            id: id.to_owned(),
            owner: owner.to_owned(),
            path,
            completed: false,
        };
        external_use.path = validate_session_path(&root, &external_use.path)?;
        Ok(external_use)
    }

    /// Releases a WebView-owned session. Files handed to another process or
    /// currently being handed off remain pinned until application exit.
    pub fn release(&self, id: &str, owner: &str) -> Result<bool, FormatError> {
        let session = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            let session = session_for_owner_mut(&mut resources.sessions, id, owner)?;
            if session.is_pinned() {
                session.release_requested = true;
                return Ok(false);
            }
            resources.sessions.remove(id)
        };
        match session {
            Some(session) => {
                close_session_file(session)?;
                Ok(true)
            }
            None => Err(session_unavailable()),
        }
    }

    pub fn release_window(&self, owner: &str) -> usize {
        let sessions = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            let next_generation = resources.owner_generation(owner).saturating_add(1);
            resources
                .owner_generations
                .insert(owner.to_owned(), next_generation);
            let ids = resources
                .sessions
                .iter_mut()
                .filter_map(|(id, session)| {
                    if session.owner != owner {
                        return None;
                    }
                    session.release_requested = true;
                    (!session.is_pinned()).then(|| id.clone())
                })
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| resources.sessions.remove(&id))
                .collect::<Vec<_>>()
        };
        let released = sessions.len();
        for session in sessions {
            let _ = close_session_file(session);
        }
        released
    }

    pub(crate) fn begin_shutdown(&self) {
        let mut resources = lock_unpoisoned(&self.shared.resources);
        resources.closing = true;
    }

    pub fn cleanup(&self) {
        let sessions = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            resources.closing = true;
            resources
                .sessions
                .drain()
                .map(|(_, session)| session)
                .collect::<Vec<_>>()
        };
        for session in sessions {
            let _ = close_session_file(session);
        }
        if let Some(workspace) = &self.shared.workspace {
            workspace.cleanup();
        }
    }

    #[cfg(test)]
    pub(crate) fn root_path(&self) -> Option<&Path> {
        self.shared.workspace.as_ref().map(PreviewWorkspace::path)
    }
}

impl PreviewExternalUse {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn succeed(mut self) {
        self.finish(true);
    }

    fn finish(&mut self, succeeded: bool) {
        if self.completed {
            return;
        }
        self.completed = true;
        let session = {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            let should_remove = match resources.sessions.get_mut(&self.id) {
                Some(session) if session.owner == self.owner => {
                    if session.pending_external_uses > 0 {
                        session.pending_external_uses -= 1;
                    }
                    if succeeded {
                        session.sticky_external_pin = true;
                    }
                    session.release_requested && !session.is_pinned()
                }
                _ => false,
            };
            if should_remove {
                resources.sessions.remove(&self.id)
            } else {
                None
            }
        };
        if let Some(session) = session {
            let _ = close_session_file(session);
        }
    }
}

impl Drop for PreviewExternalUse {
    fn drop(&mut self) {
        self.finish(false);
    }
}

impl PreviewResourceReservation {
    fn create_preview_file(
        &self,
        display_name: &str,
    ) -> Result<(PreviewFile, fs::File), FormatError> {
        self.shared
            .workspace
            .as_ref()
            .ok_or_else(preview_unavailable)?
            .create_preview_file(display_name)
            .map_err(FormatError::from)
    }

    pub(crate) fn workspace_path(&self) -> Result<&Path, FormatError> {
        self.shared
            .workspace
            .as_ref()
            .map(PreviewWorkspace::path)
            .ok_or_else(preview_unavailable)
    }

    fn into_session(mut self, id: String, session: PreviewSession) -> Result<(), FormatError> {
        if session.size > MAX_PREVIEW_ENTRY_BYTES {
            return Err(preview_entry_too_large());
        }
        let mut resources = lock_unpoisoned(&self.shared.resources);
        if resources.closing || resources.owner_generation(&self.owner) != self.owner_generation {
            return Err(session_unavailable());
        }
        if resources.sessions.contains_key(&id) {
            return Err(FormatError::Other(
                "preview session identifier collision".to_owned(),
            ));
        }
        release_reservation_usage(&mut resources);
        resources.sessions.insert(id, session);
        self.active = false;
        Ok(())
    }

    pub(crate) fn into_lease(mut self, size: u64) -> Result<PreviewResourceLease, FormatError> {
        if size > MAX_PREVIEW_ENTRY_BYTES {
            return Err(preview_entry_too_large());
        }
        let mut resources = lock_unpoisoned(&self.shared.resources);
        if resources.closing || resources.owner_generation(&self.owner) != self.owner_generation {
            return Err(session_unavailable());
        }
        release_reservation_usage(&mut resources);
        resources.lease_count = resources.lease_count.saturating_add(1);
        resources.lease_bytes = resources.lease_bytes.saturating_add(size);
        self.active = false;
        Ok(PreviewResourceLease {
            shared: Arc::clone(&self.shared),
            size,
            active: true,
        })
    }
}

impl Drop for PreviewResourceReservation {
    fn drop(&mut self) {
        if self.active {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            release_reservation_usage(&mut resources);
            self.active = false;
        }
    }
}

impl Drop for PreviewResourceLease {
    fn drop(&mut self) {
        if self.active {
            let mut resources = lock_unpoisoned(&self.shared.resources);
            if resources.lease_count > 0 {
                resources.lease_count -= 1;
            }
            resources.lease_bytes = resources.lease_bytes.saturating_sub(self.size);
            self.active = false;
        }
    }
}

fn release_reservation_usage(resources: &mut PreviewResources) {
    if resources.in_flight_count > 0 {
        resources.in_flight_count -= 1;
    }
    resources.in_flight_bytes = resources
        .in_flight_bytes
        .saturating_sub(MAX_PREVIEW_ENTRY_BYTES);
}

fn session_for_owner<'a>(
    sessions: &'a HashMap<String, PreviewSession>,
    id: &str,
    owner: &str,
) -> Result<&'a PreviewSession, FormatError> {
    match sessions.get(id) {
        Some(session) if session.owner == owner => Ok(session),
        _ => Err(session_unavailable()),
    }
}

fn session_for_owner_mut<'a>(
    sessions: &'a mut HashMap<String, PreviewSession>,
    id: &str,
    owner: &str,
) -> Result<&'a mut PreviewSession, FormatError> {
    match sessions.get_mut(id) {
        Some(session) if session.owner == owner => Ok(session),
        _ => Err(session_unavailable()),
    }
}

fn preview_unavailable() -> FormatError {
    FormatError::Other(PREVIEW_WORKSPACE_UNAVAILABLE_DETAIL.to_owned())
}

fn session_unavailable() -> FormatError {
    FormatError::Other(PREVIEW_SESSION_UNAVAILABLE_DETAIL.to_owned())
}

fn preview_entry_too_large() -> FormatError {
    FormatError::ResourceLimitExceeded(PREVIEW_ENTRY_TOO_LARGE_DETAIL.to_owned())
}

fn preview_active_capacity() -> FormatError {
    FormatError::ResourceLimitExceeded(PREVIEW_ACTIVE_CAPACITY_DETAIL.to_owned())
}

fn preview_retained_capacity() -> FormatError {
    FormatError::ResourceLimitExceeded(PREVIEW_RETAINED_CAPACITY_DETAIL.to_owned())
}

fn preview_storage_capacity() -> FormatError {
    FormatError::ResourceLimitExceeded(PREVIEW_STORAGE_CAPACITY_DETAIL.to_owned())
}

fn session_bytes(sessions: &HashMap<String, PreviewSession>) -> u64 {
    sessions
        .values()
        .fold(0_u64, |total, session| total.saturating_add(session.size))
}

fn close_session_file(session: PreviewSession) -> Result<(), FormatError> {
    session.file.close().map_err(FormatError::from)
}

fn preview_display_name(entry_path: &str) -> String {
    entry_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn validate_session_path(root: &Path, path: &Path) -> Result<PathBuf, FormatError> {
    squallz_core::open_directory_no_follow(root)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(session_unavailable());
    }
    let canonical_root = fs::canonicalize(root)?;
    let canonical_path = fs::canonicalize(path)?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(session_unavailable());
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available_root(manager: &PreviewSessionManager) -> &Path {
        manager
            .root_path()
            .expect("preview manager should be available")
    }

    fn insert_test_session(manager: &PreviewSessionManager, owner: &str) -> (String, PathBuf) {
        let reservation = manager.reserve(owner).expect("capacity should be reserved");
        let (file, mut pending) = reservation
            .create_preview_file("说明.txt")
            .expect("preview file should be created");
        pending
            .write_all(b"preview")
            .expect("preview fixture should be written");
        drop(pending);
        let path = file.path().to_path_buf();
        let id = file.id().to_owned();
        let sequence = manager.shared.next_sequence.fetch_add(1, Ordering::Relaxed);
        reservation
            .into_session(
                id.clone(),
                PreviewSession {
                    owner: owner.to_owned(),
                    file,
                    size: 7,
                    sequence,
                    sticky_external_pin: false,
                    pending_external_uses: 0,
                    release_requested: false,
                },
            )
            .expect("preview session should be inserted");
        (id, path)
    }

    #[test]
    fn preview_display_name_uses_only_the_archive_entry_basename() {
        assert_eq!(
            preview_display_name("docs/private/report.pdf"),
            "report.pdf"
        );
        assert_eq!(
            preview_display_name(r"docs\private\report.pdf"),
            "report.pdf"
        );
        assert_eq!(preview_display_name(""), "");
    }

    #[test]
    fn session_ids_do_not_expose_the_entry_name_or_temp_root() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let reservation = manager
            .reserve("main")
            .expect("capacity should be reserved");
        let (file, pending) = reservation
            .create_preview_file("confidential.pdf")
            .expect("preview file should be created");
        drop(pending);
        let id = file.id();

        assert!(!id.contains("confidential"));
        assert!(!id.contains('/'));
        assert!(!id.contains('\\'));
        assert!(!id.contains(available_root(&manager).to_string_lossy().as_ref()));
    }

    #[test]
    fn concurrent_reservations_fail_before_a_third_writer_can_start() {
        let manager =
            Arc::new(PreviewSessionManager::new().expect("preview manager should initialize"));
        let ready = Arc::new(std::sync::Barrier::new(3));
        let release = Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for owner in ["one", "two"] {
            let manager = Arc::clone(&manager);
            let ready = Arc::clone(&ready);
            let release = Arc::clone(&release);
            handles.push(std::thread::spawn(move || {
                let reservation = manager.reserve(owner).expect("reservation should fit");
                ready.wait();
                release.wait();
                drop(reservation);
            }));
        }
        ready.wait();
        assert!(matches!(
            manager.reserve("three"),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
        release.wait();
        for handle in handles {
            handle.join().expect("reservation worker should finish");
        }
        assert!(manager.reserve("after").is_ok());
    }

    #[test]
    fn oldest_unpinned_session_is_evicted_for_a_reservation() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let mut ids = Vec::new();
        let mut paths = Vec::new();
        for _ in 0..MAX_ACTIVE_PREVIEW_RESOURCES {
            let (id, path) = insert_test_session(&manager, "main");
            ids.push(id);
            paths.push(path);
        }

        let reservation = manager
            .reserve("main")
            .expect("oldest session should evict");
        assert!(!paths[0].exists());
        assert!(manager.release(&ids[0], "main").is_err());
        drop(reservation);
    }

    #[test]
    fn retained_external_sessions_do_not_exhaust_active_slots() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        for _ in 0..MAX_ACTIVE_PREVIEW_RESOURCES {
            let (id, _) = insert_test_session(&manager, "main");
            manager
                .begin_external_use(&id, "main")
                .expect("external use should begin")
                .succeed();
        }

        assert!(manager.reserve("main").is_ok());
    }

    #[test]
    fn retained_external_limit_blocks_another_handoff_without_deleting_files() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let mut retained_paths = Vec::new();
        for _ in 0..MAX_RETAINED_EXTERNAL_SESSIONS {
            let (id, path) = insert_test_session(&manager, "main");
            manager
                .begin_external_use(&id, "main")
                .expect("external use should begin")
                .succeed();
            retained_paths.push(path);
        }

        let (pending_id, pending_path) = insert_test_session(&manager, "main");
        let error = manager
            .begin_external_use(&pending_id, "main")
            .err()
            .expect("another retained handoff should be rejected");
        assert_eq!(
            preview_failure_kind(&error),
            Some(PreviewFailureKind::RetainedExternalCapacity)
        );
        assert!(pending_path.exists());
        assert!(retained_paths.iter().all(|path| path.exists()));
        assert!(manager
            .release(&pending_id, "main")
            .expect("unopened pending file should release"));
    }

    #[test]
    fn persistent_leases_share_the_item_limit_and_release_capacity() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let mut leases = Vec::new();
        for _ in 0..MAX_ACTIVE_PREVIEW_RESOURCES {
            let lease = manager
                .reserve("main")
                .expect("lease reservation should fit")
                .into_lease(1)
                .expect("reservation should become a lease");
            leases.push(lease);
        }
        assert!(matches!(
            manager.reserve("main"),
            Err(FormatError::ResourceLimitExceeded(_))
        ));

        leases.pop();
        assert!(manager.reserve("main").is_ok());
    }

    #[test]
    fn release_checks_owner_and_deletes_unopened_file() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let (id, path) = insert_test_session(&manager, "main");

        assert!(manager.release(&id, "other").is_err());
        assert!(path.exists());
        assert!(manager.release(&id, "main").expect("owner release"));
        assert!(!path.exists());
        assert!(!path.parent().expect("preview directory").exists());
    }

    #[cfg(unix)]
    #[test]
    fn external_open_rejects_preview_file_and_directory_symlinks() {
        let base = tempfile::tempdir().expect("test base");
        let manager = PreviewSessionManager::new_in(base.path()).expect("preview manager");
        let (id, path) = insert_test_session(&manager, "main");
        let outside = base.path().join("outside.txt");
        fs::write(&outside, b"outside").expect("outside fixture");
        fs::remove_file(&path).expect("remove preview fixture");
        std::os::unix::fs::symlink(&outside, &path).expect("replace preview with symlink");
        assert!(manager.begin_external_use(&id, "main").is_err());
        {
            let resources = lock_unpoisoned(&manager.shared.resources);
            assert_eq!(resources.retained_or_pending_external_count(), 0);
        }
        assert!(manager.release(&id, "main").is_err());
        assert_eq!(fs::read(&outside).expect("outside remains"), b"outside");
        assert!(manager.reserve("main").is_ok());

        let (id, path) = insert_test_session(&manager, "main");
        let root = path.parent().expect("preview directory");
        let moved = base.path().join("moved");
        fs::rename(root, &moved).expect("move preview directory");
        std::os::unix::fs::symlink(&moved, root).expect("replace preview directory with symlink");
        assert!(manager.begin_external_use(&id, "main").is_err());
        {
            let resources = lock_unpoisoned(&manager.shared.resources);
            assert_eq!(resources.retained_or_pending_external_count(), 0);
        }
        assert!(manager.release(&id, "main").is_err());
        assert_eq!(
            fs::read(moved.join("说明.txt")).expect("moved file remains"),
            b"preview"
        );
        assert!(manager.reserve("main").is_ok());
    }

    #[test]
    fn successful_external_use_stays_pinned_after_a_later_failure() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let (id, path) = insert_test_session(&manager, "main");

        let first = manager
            .begin_external_use(&id, "main")
            .expect("first external use should begin");
        let second = manager
            .begin_external_use(&id, "main")
            .expect("second external use should begin");
        first.succeed();
        assert_eq!(
            lock_unpoisoned(&manager.shared.resources)
                .sessions
                .get(&id)
                .expect("successful session remains available")
                .pending_external_uses,
            1
        );
        drop(second);

        assert!(!manager.release(&id, "main").expect("pinned release"));
        assert!(path.exists());
    }

    #[test]
    fn failed_pending_external_use_completes_a_requested_release() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let (id, path) = insert_test_session(&manager, "main");

        let first = manager
            .begin_external_use(&id, "main")
            .expect("external use should begin");
        let second = manager
            .begin_external_use(&id, "main")
            .expect("simultaneous external use should begin");
        assert!(!manager.release(&id, "main").expect("pending release"));
        drop(first);
        assert!(path.exists());
        drop(second);
        assert!(!path.exists());
        assert!(manager.release(&id, "main").is_err());

        let (id, path) = insert_test_session(&manager, "main");
        let first = manager
            .begin_external_use(&id, "main")
            .expect("external use before window release should begin");
        let second = manager
            .begin_external_use(&id, "main")
            .expect("simultaneous external use before window release should begin");
        assert_eq!(manager.release_window("main"), 0);
        drop(second);
        assert!(path.exists());
        drop(first);
        assert!(!path.exists());
        assert!(manager.release(&id, "main").is_err());

        let (id, path) = insert_test_session(&manager, "main");
        let first = manager
            .begin_external_use(&id, "main")
            .expect("external use before cleanup should begin");
        let second = manager
            .begin_external_use(&id, "main")
            .expect("simultaneous external use before cleanup should begin");
        manager.cleanup();
        assert!(!path.exists());
        drop(first);
        second.succeed();
        assert!(manager.release(&id, "main").is_err());
    }

    #[test]
    fn destroyed_owner_rejects_an_in_flight_session_commit() {
        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let reservation = manager
            .reserve("main")
            .expect("capacity should be reserved");
        let (file, pending) = reservation
            .create_preview_file("说明.txt")
            .expect("preview file should be created");
        drop(pending);
        let path = file.path().to_path_buf();
        let id = file.id().to_owned();

        assert_eq!(manager.release_window("main"), 0);
        let result = reservation.into_session(
            id,
            PreviewSession {
                owner: "main".to_owned(),
                file,
                size: 0,
                sequence: 1,
                sticky_external_pin: false,
                pending_external_uses: 0,
                release_requested: false,
            },
        );
        assert!(result.is_err());
        assert!(!path.exists());
        assert!(manager.reserve("main").is_ok());
    }

    #[test]
    fn unavailable_and_closing_managers_fail_preview_only() {
        let unavailable = PreviewSessionManager::unavailable();
        assert!(unavailable.reserve("main").is_err());

        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let root = available_root(&manager).to_path_buf();
        manager.cleanup();
        assert!(manager.reserve("main").is_err());
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn preview_root_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let manager = PreviewSessionManager::new().expect("preview manager should initialize");
        let mode = fs::metadata(available_root(&manager))
            .expect("preview root metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);

        let (_id, path) = insert_test_session(&manager, "main");
        let entry_root_mode = fs::metadata(path.parent().expect("preview directory"))
            .expect("preview directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(entry_root_mode, 0o700);
        let file_mode = fs::metadata(path)
            .expect("preview file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
    }
}
