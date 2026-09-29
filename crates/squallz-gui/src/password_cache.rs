//! Process-local passwords bound to the physical archive version that was read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use squallz_core::api::{ControlToken, Password};
use squallz_core::{lock_unpoisoned, ArchiveSourceState, Engine};

struct CachedPassword {
    source: ArchiveSourceState,
    password: Password,
}

#[derive(Default)]
struct Registry {
    entries: HashMap<PathBuf, Arc<CachedPassword>>,
    generation: u64,
    closed: bool,
}

/// Captured before password-protected work, never after it. Only a successful
/// decryption may publish a password through this attempt.
pub(crate) struct PasswordAttempt {
    path: PathBuf,
    source: ArchiveSourceState,
    generation: u64,
}

#[derive(Default)]
pub(crate) struct SessionPasswords {
    registry: Mutex<Registry>,
}

impl SessionPasswords {
    pub(crate) fn begin(
        &self,
        engine: &Engine,
        path: &Path,
        control: &ControlToken,
    ) -> Option<PasswordAttempt> {
        let generation = {
            let registry = lock_unpoisoned(&self.registry);
            if registry.closed {
                return None;
            }
            registry.generation
        };
        Some(PasswordAttempt {
            path: path.to_path_buf(),
            source: engine.inspect_archive_source_state(path, control).ok()?,
            generation,
        })
    }

    pub(crate) fn get(&self, engine: &Engine, path: &Path) -> Option<Password> {
        let cached = lock_unpoisoned(&self.registry).entries.get(path).cloned()?;
        // Source probing can touch several volume files; never hold the cache
        // lock while doing I/O or let an old lookup delete a newer entry.
        let source = engine
            .inspect_archive_source_state(path, &ControlToken::default())
            .ok();
        let mut registry = lock_unpoisoned(&self.registry);
        if !registry
            .entries
            .get(path)
            .is_some_and(|current| Arc::ptr_eq(current, &cached))
        {
            return None;
        }
        if source != Some(cached.source) {
            registry.entries.remove(path);
            return None;
        }
        Some(cached.password.clone())
    }

    pub(crate) fn remember(
        &self,
        engine: &Engine,
        attempt: &PasswordAttempt,
        password: &str,
        control: &ControlToken,
    ) {
        if engine
            .inspect_archive_source_state(&attempt.path, control)
            .ok()
            != Some(attempt.source)
        {
            return;
        }
        let mut registry = lock_unpoisoned(&self.registry);
        if registry.closed || registry.generation != attempt.generation || control.is_cancelled() {
            return;
        }
        registry.entries.insert(
            attempt.path.clone(),
            Arc::new(CachedPassword {
                source: attempt.source,
                password: Password::new(password),
            }),
        );
    }

    pub(crate) fn forget(&self, path: &Path) {
        let mut registry = lock_unpoisoned(&self.registry);
        registry.generation = registry.generation.wrapping_add(1);
        registry.entries.remove(path);
    }

    /// Window closure revokes pending writes, while already verified
    /// credentials remain available to the rest of the application session.
    pub(crate) fn invalidate_pending(&self) {
        let mut registry = lock_unpoisoned(&self.registry);
        registry.generation = registry.generation.wrapping_add(1);
    }

    pub(crate) fn shutdown(&self) {
        let mut registry = lock_unpoisoned(&self.registry);
        registry.closed = true;
        registry.generation = registry.generation.wrapping_add(1);
        registry.entries.clear();
    }

    #[cfg(test)]
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        let mut paths = lock_unpoisoned(&self.registry)
            .entries
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Engine, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.zip");
        fs::write(&path, b"source version one").unwrap();
        (dir, Engine::new(squallz_formats::registry()), path)
    }

    #[test]
    fn forget_window_close_and_shutdown_revoke_pending_cache_writes() {
        let (_dir, engine, path) = fixture();
        let cache = SessionPasswords::default();
        let control = ControlToken::default();
        let attempt = cache.begin(&engine, &path, &control).unwrap();
        cache.remember(&engine, &attempt, "verified", &control);
        assert!(cache.get(&engine, &path).is_some());
        cache.forget(&path);
        cache.remember(&engine, &attempt, "verified", &control);
        assert!(cache.get(&engine, &path).is_none());
        let after_forget = cache.begin(&engine, &path, &control).unwrap();
        cache.invalidate_pending();
        cache.remember(&engine, &after_forget, "verified", &control);
        assert!(cache.get(&engine, &path).is_none());
        let after_close = cache.begin(&engine, &path, &control).unwrap();
        cache.remember(&engine, &after_close, "verified", &control);
        cache.shutdown();
        cache.remember(&engine, &after_close, "verified", &control);
        assert!(cache.get(&engine, &path).is_none());
        assert!(cache.begin(&engine, &path, &control).is_none());
    }

    #[test]
    fn changed_missing_or_cancelled_sources_are_never_cached() {
        let (_dir, engine, path) = fixture();
        let cache = SessionPasswords::default();
        let control = ControlToken::default();
        let attempt = cache.begin(&engine, &path, &control).unwrap();
        fs::write(&path, b"source version two, changed").unwrap();
        cache.remember(&engine, &attempt, "old", &control);
        assert!(cache.get(&engine, &path).is_none());
        let current = cache.begin(&engine, &path, &control).unwrap();
        cache.remember(&engine, &current, "verified", &control);
        assert!(cache.get(&engine, &path).is_some());
        fs::remove_file(&path).unwrap();
        assert!(cache.get(&engine, &path).is_none());
        assert!(cache.paths().is_empty());
        fs::write(&path, b"new source").unwrap();
        let current = cache.begin(&engine, &path, &control).unwrap();
        control.cancel();
        cache.remember(&engine, &current, "cancelled", &control);
        assert!(cache.get(&engine, &path).is_none());
    }

    #[test]
    fn password_cache_lock_recovers_after_poison() {
        let (_dir, engine, path) = fixture();
        let cache = Arc::new(SessionPasswords::default());
        let other = Arc::clone(&cache);
        assert!(std::thread::spawn(move || {
            let _guard = other.registry.lock().unwrap();
            panic!("poison password cache");
        })
        .join()
        .is_err());
        let control = ControlToken::default();
        let attempt = cache.begin(&engine, &path, &control).unwrap();
        cache.remember(&engine, &attempt, "verified", &control);
        assert!(cache.get(&engine, &path).is_some());
        cache.forget(&path);
        assert!(cache.get(&engine, &path).is_none());
    }

    #[test]
    fn changing_or_losing_a_later_volume_invalidates_the_password() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("archive.7z.001");
        let second = dir.path().join("archive.7z.002");
        fs::write(&first, b"first volume").unwrap();
        fs::write(&second, b"second volume").unwrap();
        let engine = Engine::new(squallz_formats::registry());
        let cache = SessionPasswords::default();
        let control = ControlToken::default();
        let attempt = cache.begin(&engine, &first, &control).unwrap();
        cache.remember(&engine, &attempt, "verified", &control);
        assert!(cache.get(&engine, &first).is_some());
        fs::write(&second, b"changed second volume").unwrap();
        assert!(cache.get(&engine, &first).is_none());
        let attempt = cache.begin(&engine, &first, &control).unwrap();
        cache.remember(&engine, &attempt, "verified", &control);
        fs::remove_file(&second).unwrap();
        assert!(cache.get(&engine, &first).is_none());
    }
}
